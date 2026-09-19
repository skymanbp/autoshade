use super::*;

// --- a profile, built byte by byte -----------------------------------------

/// Build a `.dcp`-shaped little-endian TIFF from `(tag, type, payload)`.
///
/// The parser reads FILES, so the tests hand it files. A builder that produced
/// a [`Profile`] directly would leave the container half — the offsets, the
/// inline-versus-external rule, the byte order — completely undriven, and that
/// half is where a reader of someone else's bytes actually goes wrong.
fn build(entries: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
    let ifd_at = 8usize;
    let data_at = ifd_at + 2 + entries.len() * 12 + 4;
    let mut out = Vec::from(b"II".as_slice());
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&(ifd_at as u32).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut blob = Vec::new();
    for (tag, kind, payload) in entries {
        let count = (payload.len() / type_size(*kind).max(1)) as u32;
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        if payload.len() <= 4 {
            let mut inline = payload.clone();
            inline.resize(4, 0);
            out.extend_from_slice(&inline);
        } else {
            out.extend_from_slice(&((data_at + blob.len()) as u32).to_le_bytes());
            blob.extend_from_slice(payload);
        }
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // no second IFD
    out.extend_from_slice(&blob);
    out
}

fn floats(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn longs(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// SRATIONAL, which is how every matrix in a real profile is stored.
fn srational(v: &[f32]) -> Vec<u8> {
    v.iter()
        .flat_map(|x| {
            let n = (x * 1_000_000.0).round() as i32;
            [n.to_le_bytes(), 1_000_000i32.to_le_bytes()].concat()
        })
        .collect()
}

fn ascii(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

/// A table whose every entry states its OWN coordinates, so a reader that
/// walks the axes in the wrong order lands on a cell that says so.
///
/// `hue shift = h`, `sat scale = 1 + s/100`, `val scale = 1 + v/1000`.
fn coordinate_table(h: u32, s: u32, v: u32) -> Vec<f32> {
    let mut out = Vec::with_capacity((h * s * v * 3) as usize);
    for vi in 0..v {
        for hi in 0..h {
            for si in 0..s {
                out.extend_from_slice(&[
                    hi as f32,
                    1.0 + si as f32 / 100.0,
                    1.0 + vi as f32 / 1000.0,
                ]);
            }
        }
    }
    out
}

fn sony_shaped() -> Vec<u8> {
    build(&[
        (tag::UNIQUE_CAMERA_MODEL, 2, ascii("ILCE-7RM4A")),
        (tag::COLOR_MATRIX_1, 10, srational(&[0.9, -0.3, -0.1, -0.4, 1.2, 0.2, 0.0, 0.1, 0.5])),
        (tag::CALIBRATION_ILLUMINANT_1, 3, 17u16.to_le_bytes().to_vec()),
        (tag::CALIBRATION_ILLUMINANT_2, 3, 21u16.to_le_bytes().to_vec()),
        (tag::PROFILE_NAME, 2, ascii("Adobe Standard")),
        (tag::HUE_SAT_MAP_DIMS, 4, longs(&[4, 3, 1])),
        (tag::HUE_SAT_MAP_DATA_1, 11, floats(&coordinate_table(4, 3, 1))),
        (tag::PROFILE_TONE_CURVE, 11, floats(&[0.0, 0.0, 0.5, 0.6, 1.0, 1.0])),
        (
            tag::FORWARD_MATRIX_1,
            10,
            srational(&[0.44, 0.35, 0.17, 0.21, 0.74, 0.05, 0.07, 0.0, 0.75]),
        ),
        (tag::LOOK_TABLE_DIMS, 4, longs(&[4, 2, 3])),
        (tag::LOOK_TABLE_DATA, 11, floats(&coordinate_table(4, 2, 3))),
        (tag::LOOK_TABLE_ENCODING, 4, longs(&[1])),
        (tag::BASELINE_EXPOSURE_OFFSET, 10, srational(&[-0.35])),
    ])
}

/// `got` is within `tol` of `want`, reported with the name of the thing.
fn near(what: &str, got: f32, want: f32, tol: f32) {
    assert!((got - want).abs() <= tol, "{what}: got {got}, want {want} (±{tol})");
}

// --- the container ---------------------------------------------------------

/// Every tag this module claims to read IS read, in the shapes a real profile
/// uses them in: ASCII with its NUL, SHORT inline, LONG arrays, SRATIONAL
/// matrices, FLOAT tables.
///
/// MUTATION: drop any tag from `parse`, read SRATIONAL as a raw integer pair,
/// or keep the ASCII NUL inside the string.
#[test]
fn a_profile_reads_its_name_matrices_tables_and_offsets() {
    let p = parse(&sony_shaped()).expect("a well-formed profile");
    assert_eq!(p.name, "Adobe Standard", "the name is what `crs:CameraProfile` matches on");
    assert_eq!(p.unique_model, "ILCE-7RM4A");
    assert_eq!(p.illuminant, [Some(17), Some(21)], "Standard A and D65, as Sony's profiles state");
    assert!(p.color_matrix[1].is_none(), "a tag that is not there must not be invented");
    assert_eq!(p.tone_curve, vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]]);
    assert!(!p.has_third_illuminant);
    let cm = p.color_matrix[0].expect("ColorMatrix1");
    let fm = p.forward_matrix[0].expect("ForwardMatrix1");
    near("ColorMatrix1[0][0]", cm[0][0], 0.9, 1e-5);
    near("ColorMatrix1[2][2]", cm[2][2], 0.5, 1e-5);
    near("ForwardMatrix1[1][1]", fm[1][1], 0.74, 1e-5);
    near("BaselineExposureOffset", p.baseline_exposure_offset, -0.35, 1e-5);

    let hsm = p.hue_sat_map[0].as_ref().expect("HueSatMapData1");
    assert_eq!((hsm.hue, hsm.sat, hsm.val), (4, 3, 1));
    assert!(!hsm.srgb_value, "no encoding tag for the hue/sat map means linear");
    assert!(p.hue_sat_map[1].is_none(), "Data2 is absent in this fixture");
    let lut = p.look_table.as_ref().expect("LookTableData");
    assert_eq!((lut.hue, lut.sat, lut.val), (4, 2, 3));
    assert!(lut.srgb_value, "ProfileLookTableEncoding = 1 means the value axis is sRGB");
}

/// The named refusals, each driven by the defect it names.
///
/// A reader of files the USER installed must refuse, not panic: a truncated
/// profile is an ordinary thing to find on a disk and it must not take a render
/// down. MUTATION: index the byte slices directly instead of through `get`.
#[test]
fn a_broken_profile_is_refused_by_name_and_never_panics() {
    assert!(matches!(parse(b"not a tiff at all"), Err(Refusal::NotAProfile(_))));
    assert!(matches!(parse(b""), Err(Refusal::NotAProfile(_))), "empty bytes");
    // A header that promises an IFD past the end of the file.
    let mut short = Vec::from(b"II".as_slice());
    short.extend_from_slice(&42u16.to_le_bytes());
    short.extend_from_slice(&9_999_999u32.to_le_bytes());
    assert!(matches!(parse(&short), Err(Refusal::NotAProfile(_))));
    // A whole profile, truncated mid-table: a tag's offset now points past the
    // end, and the answer is a refusal rather than a read of nothing.
    let full = sony_shaped();
    assert!(
        matches!(parse(&full[..full.len() - 40]), Err(Refusal::NotAProfile(_))),
        "truncated file"
    );
    // Dimensions that disagree with the data they describe.
    let bad = build(&[
        (tag::PROFILE_NAME, 2, ascii("Broken")),
        (tag::LOOK_TABLE_DIMS, 4, longs(&[8, 8, 8])),
        (tag::LOOK_TABLE_DATA, 11, floats(&coordinate_table(2, 2, 2))),
    ]);
    assert!(matches!(parse(&bad), Err(Refusal::TableMismatch(_))), "dims against data");
    // A zero division is a mismatch too — `axis` would otherwise be asked for
    // the sample before the first one.
    let zero = build(&[
        (tag::LOOK_TABLE_DIMS, 4, longs(&[0, 8, 8])),
        (tag::LOOK_TABLE_DATA, 11, floats(&[0.0, 1.0, 1.0])),
    ]);
    assert!(matches!(parse(&zero), Err(Refusal::TableMismatch(_))), "zero division");
}

// --- the table -------------------------------------------------------------

/// THE layout claim: `index = (val * hue + h) * sat + s`.
///
/// This is the one thing in the module worth measuring twice, and the module's
/// own documentation says why: a table with `val == 1` cannot tell this reading
/// apart from "hue slowest, value fastest". So the fixture is three-dimensional
/// with THREE DIFFERENT extents (4 × 2 × 3), where every candidate reading
/// lands on a different cell, and each cell states its own coordinates.
///
/// MUTATION: swap the hue and value strides in `Table::at`.
#[test]
fn the_table_is_indexed_value_slowest_hue_then_saturation_fastest() {
    let p = parse(&sony_shaped()).expect("profile");
    let t = p.look_table.as_ref().expect("look table");
    for (h, s, v) in [(0u32, 0u32, 0u32), (3, 1, 2), (1, 0, 2), (2, 1, 1)] {
        let e = t.at(h, s, v);
        assert_eq!(e[0], h as f32, "cell ({h},{s},{v}) came back from hue {}", e[0]);
        near("saturation", e[1], 1.0 + s as f32 / 100.0, 1e-6);
        near("value", e[2], 1.0 + v as f32 / 1000.0, 1e-6);
    }
    // …and the achromatic-plane rule that settled the layout against the
    // installed pool holds here for the same reason it holds there: `sat = 0`
    // is the plane a reader lands on ONLY under the documented index.
    for h in 0..t.hue {
        for v in 0..t.val {
            assert_eq!(t.at(h, 0, v)[1], 1.0, "sat 0 must not scale saturation");
        }
    }
}

/// Hue is CYCLIC and the other two axes CLAMP, which is what makes 359° a
/// neighbour of 1° rather than an extrapolation off the end of the array.
///
/// MUTATION: clamp the hue axis like the other two, or let the last hue
/// division interpolate toward itself.
#[test]
fn hue_wraps_around_the_circle_while_saturation_and_value_clamp() {
    let t = Table {
        hue: 4,
        sat: 2,
        val: 1,
        srgb_value: false,
        // Hue shifts 0, 10, 20, 30, at both saturations.
        data: (0..4).flat_map(|h| [[h as f32 * 10.0, 1.0, 1.0]; 2]).collect(),
    };
    // Between the last division (270°, shift 30) and the first (0°, shift 0):
    // halfway round is 315°, and the answer has to be the average of the two
    // ENDS rather than a run off the end of the table.
    near("the wrap midpoint", t.lookup(315.0, 0.5, 0.5)[0], 15.0, 1e-4);
    near("just short of the wrap", t.lookup(359.999, 0.5, 0.5)[0], 0.0, 0.01);
    // 360° IS 0°, and a negative angle is the same colour as its positive twin.
    assert_eq!(t.lookup(360.0, 0.5, 0.5), t.lookup(0.0, 0.5, 0.5));
    assert_eq!(t.lookup(-90.0, 0.5, 0.5), t.lookup(270.0, 0.5, 0.5));
    // Saturation past the end clamps instead of wrapping to the other end.
    assert_eq!(t.lookup(90.0, 5.0, 0.5), t.lookup(90.0, 1.0, 0.5), "saturation must clamp");
    assert_eq!(t.lookup(90.0, -5.0, 0.5), t.lookup(90.0, 0.0, 0.5));
    // A single value division is a constant axis, not a division by zero.
    assert!(t.lookup(90.0, 0.5, 0.0)[0].is_finite() && t.lookup(90.0, 0.5, 1.0)[0].is_finite());
}

/// WHERE the saturation and value samples sit: the divisions span [0, 1]
/// INCLUSIVE, so the step is `1/(n-1)` and the last sample is exactly at 1.
///
/// Hue is the one axis whose divisions span a circle and therefore step by
/// `1/n`; saturation and value are the other rule, and the two are one
/// character apart in the source. The difference is invisible at the ends — a
/// `1/n` reading still reaches the top sample, because the index clamps — and
/// invisible again on a table whose samples all agree. It shows only BETWEEN
/// samples, which is where a photograph actually lands, so that is where this
/// probes.
///
/// Written because the F7 mutation sweep found the law unpinned: dividing by
/// `n` left `hue_wraps_around_the_circle_while_saturation_and_value_clamp`
/// green, since its two saturation samples carry the same shift.
///
/// MUTATION: `axis` scales by `n` rather than `n - 1`.
#[test]
fn the_saturation_and_value_divisions_span_zero_to_one_inclusive() {
    // One hue, one value, three saturations — so `lookup` reads the saturation
    // axis alone and the entry it lands on is unambiguous.
    let t = Table {
        hue: 1,
        sat: 3,
        val: 1,
        srgb_value: false,
        data: vec![[0.0, 1.0, 1.0], [10.0, 1.0, 1.0], [100.0, 1.0, 1.0]],
    };
    // The three samples themselves, at 0, 1/2 and 1.
    near("saturation 0", t.lookup(0.0, 0.0, 0.5)[0], 0.0, 1e-4);
    near("saturation 1/2 IS the middle sample", t.lookup(0.0, 0.5, 0.5)[0], 10.0, 1e-4);
    near("saturation 1 IS the last sample", t.lookup(0.0, 1.0, 0.5)[0], 100.0, 1e-4);
    // …and halfway between two of them, which is the reading `1/n` gets wrong:
    // it would place 0.25 three quarters of the way up the first interval.
    near("a quarter of the way", t.lookup(0.0, 0.25, 0.5)[0], 5.0, 1e-4);
    near("three quarters", t.lookup(0.0, 0.75, 0.5)[0], 55.0, 1e-4);

    // The same law on the VALUE axis, which is the one that also carries an
    // encoding flag — so the flag and the spacing are separate facts.
    let v = Table {
        hue: 1,
        sat: 1,
        val: 3,
        srgb_value: false,
        data: vec![[0.0, 1.0, 1.0], [10.0, 1.0, 1.0], [100.0, 1.0, 1.0]],
    };
    near("value 1/2 IS the middle sample", v.lookup(0.0, 0.5, 0.5)[0], 10.0, 1e-4);
    near("value 1 IS the last sample", v.lookup(0.0, 0.5, 1.0)[0], 100.0, 1e-4);
}

/// `apply` composes the three factors the way the file means them: the hue
/// shift ADDS in degrees, the saturation scale MULTIPLIES, and the value scale
/// multiplies in the table's OWN encoding.
///
/// The encoding flag is the half a plain multiply gets wrong: a 0.5 scale on an
/// ENCODED value is a far bigger darkening in linear light than a 0.5 scale on
/// the linear one, so a reader that ignored the flag would render every
/// `Camera *` profile too dark.
///
/// MUTATION: ignore `srgb_value`, or multiply the hue instead of adding it.
#[test]
fn apply_adds_the_hue_scales_the_saturation_and_honours_the_value_encoding() {
    let flat =
        |srgb| Table { hue: 2, sat: 2, val: 1, srgb_value: srgb, data: vec![[30.0, 0.5, 0.5]; 4] };
    let mut hsv = [350.0, 0.8, 0.5];
    flat(false).apply(&mut hsv);
    near("hue, added and wrapped", hsv[0], 20.0, 1e-3);
    near("saturation, scaled", hsv[1], 0.4, 1e-6);
    near("value, scaled in linear light", hsv[2], 0.25, 1e-6);

    let mut enc = [350.0, 0.8, 0.5];
    flat(true).apply(&mut enc);
    // 0.5 linear encodes to 0.73536; half of that decodes to 0.11128 — far
    // darker than the 0.25 the linear reading gives, which is exactly why the
    // flag cannot be ignored.
    near("value, scaled in the sRGB encoding", enc[2], 0.111_28, 1e-4);
    assert!(enc[2] < 0.25, "the encoded reading must differ from the linear one");
    // Saturation cannot leave [0, 1] however hard a table pushes.
    let mut hot = [0.0, 1.0, 1.0];
    Table { hue: 1, sat: 1, val: 1, srgb_value: false, data: vec![[0.0, 99.0, 1.0]] }
        .apply(&mut hot);
    assert_eq!(hot[1], 1.0, "saturation clamped");
}

// --- discovery -------------------------------------------------------------

/// The file-name filter reads Adobe's two spellings of the same profile.
///
/// The installed pool holds `Sony ILCE-7RM4A Adobe Standard.dcp` AND
/// `Leica D-Lux 7 Adobe_Standard.dcp`; a byte comparison would report "no
/// profile installed" about the second while it sat in the directory.
///
/// MUTATION: compare case-sensitively, or drop the underscore folding.
#[test]
fn the_name_filter_reads_both_of_adobes_spellings() {
    assert_eq!(fold("Adobe_Standard"), "adobe standard");
    assert_eq!(fold("ILCE-7RM4A"), "ilce-7rm4a");
    for stem in ["Sony ILCE-7RM4A Adobe Standard", "Leica D-Lux 7 Adobe_Standard"] {
        assert!(fold(stem).ends_with("adobe standard"), "{stem}");
    }
    // …and a different profile of the same body does NOT end with that name,
    // so the filter separates the eight `Camera *` profiles from the Standard.
    assert!(!fold("Sony ILCE-7RM4A Camera Vivid").ends_with("adobe standard"));
}

/// What this machine's own install answers, and what a machine without one
/// answers — both asserted, because the module has to behave on both.
///
/// On a developer machine with Camera Raw this parses a REAL Adobe file and
/// checks the invariant the layout was settled on. On CI, where no Adobe
/// install exists, it asserts the answer is a NAMED refusal and not a panic or
/// a silently empty profile.
#[test]
fn the_installed_pool_answers_or_says_why_not() {
    match find("Sony", "ILCE-7RM4A", "Adobe Standard") {
        Ok((path, p)) => {
            assert_eq!(fold(&p.name), "adobe standard", "{}", path.display());
            assert!(!p.unique_model.is_empty(), "a real profile names its body");
            assert!(
                p.color_matrix[0].is_some() || p.forward_matrix[0].is_some(),
                "a camera profile carries at least one calibration matrix"
            );
            // The achromatic rule, on Adobe's own bytes rather than a fixture.
            for t in p.hue_sat_map.iter().flatten().chain(p.look_table.iter()) {
                for h in 0..t.hue {
                    for v in 0..t.val {
                        assert_eq!(
                            t.at(h, 0, v)[1],
                            1.0,
                            "an installed profile scales saturation on the achromatic plane at \
                             ({h},0,{v}) — the table layout would be wrong"
                        );
                    }
                }
            }
        }
        Err(e) => assert!(
            matches!(e, Refusal::NoRoots | Refusal::NotFound),
            "a machine without the profile must say which, not {e}"
        ),
    }
}
