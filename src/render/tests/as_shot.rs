// One part of the engine's tests (src/render/tests.rs includes it): the camera's as-shot white balance read the way Camera Raw reads it (v1.17.0).

/// rawler 0.7.2's ILCE-7RM4A calibration (data/cameras/sony/a7rm4a.toml),
/// the same two matrices as the body's Adobe Standard profile: Standard A
/// (2856 K) and D65 (6504 K).
fn a7rm4a_matrices() -> Vec<(f64, [[f64; 3]; 3])> {
    vec![
        (2856.0, [[0.9054, -0.4504, 0.0215], [-0.4751, 1.2611, 0.2388], [-0.0296, 0.0802, 0.6816]]),
        (6504.0, [[0.7662, -0.2686, -0.066], [-0.524, 1.2965, 0.253], [-0.0796, 0.1508, 0.6167]]),
    ]
}

/// A corpus frame: its name, As Shot levels, Daylight preset levels, and
/// Camera Raw's As Shot (Temperature, Tint).
type AsShotFrame = (&'static str, [f32; 2], [f32; 2], (f32, f32));

/// The calibration corpus's seven frames: As Shot levels and Daylight preset
/// levels (R, B at G 1024, read from each file's SR2 block), and the white
/// Camera Raw renders As Shot at — the Custom Temperature/Tint its As Shot
/// render is pixel-identical to (v1.17.0, Camera Raw by script).
const AS_SHOT_FRAMES: [AsShotFrame; 7] = [
    ("p36", [2092.0, 2060.0], [2564.0, 1728.0], (4100.0, 4.0)),
    ("p37", [2612.0, 1692.0], [2564.0, 1728.0], (5550.0, 13.0)),
    ("p38", [2724.0, 1616.0], [2564.0, 1728.0], (5950.0, 13.0)),
    ("p39", [2564.0, 1728.0], [2564.0, 1728.0], (5400.0, 14.0)),
    ("p40", [3092.0, 1424.0], [2564.0, 1728.0], (7650.0, 16.0)),
    ("p41", [1964.0, 2080.0], [2596.0, 1688.0], (3800.0, -3.0)),
    ("p42", [2940.0, 1484.0], [2668.0, 1664.0], (6450.0, 11.0)),
];

fn levels(rb: [f32; 2]) -> [f32; 3] {
    [rb[0] / 1024.0, 1.0, rb[1] / 1024.0]
}

/// The as-shot reading lands on Camera Raw's own on every frame of the
/// corpus, over three lenses, at Camera Raw's 50 K / whole-tint grid.
///
/// MUTATION: drop the Daylight-preset normalisation (p39 reads 5250 K / +18,
/// p42 6650 K / +17), read the tint as the Krystek Duv, or stop rounding.
#[test]
fn the_as_shot_reading_is_camera_raws_on_every_corpus_frame() {
    let m = a7rm4a_matrices();
    for (name, shot, daylight, want) in AS_SHOT_FRAMES {
        let got = camera_raw_as_shot(levels(shot), Some(levels(daylight)), "ILCE-7RM4A", &m);
        assert_eq!(got, Some(want), "{name}");
    }
}

/// The Daylight preset alone moves the reading: Camera Raw read a copy of
/// p39 whose ONLY change was its Daylight preset rewritten to the Sigma
/// lens's levels at 5100 K / +12. A body with no reference, and a file with
/// no preset, read the levels as they are — the DNG SDK's own white for them.
///
/// MUTATION: normalise by the As Shot levels' own preset row, or apply the
/// reference to an unlisted model.
#[test]
fn the_daylight_preset_and_only_a_listed_body_move_the_reading() {
    let m = a7rm4a_matrices();
    let p39 = levels([2564.0, 1728.0]);
    let sigma = levels([2668.0, 1664.0]);
    assert_eq!(camera_raw_as_shot(p39, Some(sigma), "ILCE-7RM4A", &m), Some((5100.0, 12.0)));
    assert_eq!(camera_raw_as_shot(p39, None, "ILCE-7RM4A", &m), Some((5250.0, 18.0)));
    assert_eq!(camera_raw_as_shot(p39, Some(sigma), "ILCE-7M3", &m), Some((5250.0, 18.0)));
    assert_eq!(camera_raw_as_shot([f32::NAN, 1.0, 1.0], None, "ILCE-7RM4A", &m), None);
    assert_eq!(camera_raw_as_shot([0.0, 1.0, 1.0], None, "ILCE-7RM4A", &m), None);
    assert_eq!(camera_raw_as_shot(p39, None, "ILCE-7RM4A", &[]), None);
}

/// The SR2 cipher round-trips (it is an XOR keystream), and the reader finds
/// tag 0x7820 through DNGPrivateData in a synthetic little-endian ARW skeleton.
///
/// MUTATION: skip the pad's byte swap, or resolve the value offset without
/// the block's base.
#[test]
fn the_sr2_reader_finds_the_daylight_preset_in_an_encrypted_block() {
    let (offset, key) = (64usize, 0x1234_5678u32);
    // The SR2 IFD in clear: one SSHORT×3 entry whose value sits after it.
    let mut block = Vec::new();
    block.extend_from_slice(&1u16.to_le_bytes());
    block.extend_from_slice(&0x7820u16.to_le_bytes());
    block.extend_from_slice(&8u16.to_le_bytes());
    block.extend_from_slice(&3u32.to_le_bytes());
    block.extend_from_slice(&((offset + 18) as u32).to_le_bytes());
    block.extend_from_slice(&0u32.to_le_bytes());
    for v in [2564i16, 1024, 1728] {
        block.extend_from_slice(&v.to_le_bytes());
    }
    block.resize(28, 0);
    let sealed = sony_decrypt(&block, key);
    assert_ne!(sealed, block);
    assert_eq!(sony_decrypt(&sealed, key), block, "the cipher is its own inverse");
    // IFD0 (one entry, DNGPrivateData → 40) and the private IFD at 40.
    let mut file = b"II*\0".to_vec();
    file.extend_from_slice(&8u32.to_le_bytes());
    let ifd = |file: &mut Vec<u8>, entries: &[(u16, u16, u32, u32)]| {
        file.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, kind, count, value) in entries {
            file.extend_from_slice(&tag.to_le_bytes());
            file.extend_from_slice(&kind.to_le_bytes());
            file.extend_from_slice(&count.to_le_bytes());
            file.extend_from_slice(&value.to_le_bytes());
        }
    };
    ifd(&mut file, &[(0xC634, 1, 4, 22)]);
    assert_eq!(file.len(), 22);
    ifd(&mut file, &[(0x7200, 4, 1, offset as u32), (0x7201, 4, 1, 28), (0x7221, 7, 4, key)]);
    file.resize(offset, 0);
    file.extend_from_slice(&sealed);
    let d = sony_daylight_preset(&file).expect("the preset is found");
    assert_eq!(d, [2564.0 / 1024.0, 1.0, 1728.0 / 1024.0]);
    assert_eq!(sony_daylight_preset(&file[..offset + 8]), None, "a truncated block answers None");
    assert_eq!(sony_daylight_preset(b"MM\0*\0\0\0\x08"), None, "big-endian is not an ARW");
}

/// The corpus files themselves: the reader decrypts each frame's Daylight
/// preset, and the whole path from the file reads Camera Raw's As Shot.
/// Skips when `AUTOSHADE_FIT_CALIBRATION_DIR` is unset.
#[test]
fn the_corpus_files_read_camera_raws_as_shot() {
    let Some(dir) = crate::config::live_env_os("AUTOSHADE_FIT_CALIBRATION_DIR").map(std::path::PathBuf::from) else {
        crate::test_skipped("as-shot corpus test", "AUTOSHADE_FIT_CALIBRATION_DIR unset");
        return;
    };
    for (name, _, daylight, want) in AS_SHOT_FRAMES {
        let raw = dir.join(format!("{name}.arw"));
        if !raw.is_file() {
            crate::test_skipped("as-shot corpus test", &format!("{} missing", raw.display()));
            return;
        }
        let bytes = std::fs::read(&raw).unwrap();
        assert_eq!(sony_daylight_preset(&bytes), Some(levels(daylight)), "{name}");
        assert_eq!(as_shot_wb(&raw), Some(want), "{name}");
    }
}
