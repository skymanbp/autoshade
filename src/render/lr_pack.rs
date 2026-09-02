//! The `me6-2026-09` Lightroom export pack as an INSTRUMENT: render all 46
//! sidecars through the production import + develop path so
//! [`scripts/lr_mask_parity.py`] can measure this engine's mask laws against
//! Lightroom's own exported pixels.
//!
//! The pack is one wall capture (46 byte-identical `.ARW` copies, sha256
//! `866ff7bc…`), one `.xmp` per export carrying exactly one mask idea, and the
//! 46 JPEGs Lightroom 9.4 / PV 15.4 wrote from them. Every mask sets the same
//! local exposure (−1.00 EV, `crs:LocalExposure2012="-0.25"`), so the exported
//! frame is the mask's own α painted onto a flat wall — which is why the
//! REF-subtracted difference field below recovers α without inverting the tone
//! chain.
//!
//! # Why a probe test and not a binary
//!
//! Same shape as [`crate::fit_field`]'s NumPy cross-check: the ONE producer of
//! the numbers lives in the tree, gated `#[ignore]` so it never runs in CI, and
//! writes its artefacts where a Python analysis script can read them. A
//! standalone binary would need its own copy of the import shape, and the copy
//! is what drifts.
//!
//! # The layout `AUTOSHADE_LR_PACK` names
//!
//! ```text
//! <pack>/base.ARW            one copy of the capture
//! <pack>/xmp/<code>.xmp      46 sidecars
//! <pack>/lr/<code>.jpg       46 Lightroom exports (q100 sRGB, 6240×4160)
//! <pack>/pack-spec.json      machine-readable spec of all 46
//! ```
//!
//! The frame is **6240×4160**. The sidecars' `tiff:ImageWidth/ImageLength`
//! header says 9504×6336 — stale metadata from another capture on the same
//! body — but the RAW's own `SubIFD0` is 6304×4180 photosites and every
//! exported JPEG's SOF is 6240×4160. Both are 3:2, so no fraction-based mask
//! geometry moves; only pixel conversions use the real frame.
//!
//! [`scripts/lr_mask_parity.py`]: https://github.com/skymanbp/autoshade

use std::path::{Path, PathBuf};

use crate::recipe::EditRecipe;

/// The pack root, or `None` with the reason printed — the calibration-corpus
/// convention, so a run without the fixture is a visible skip rather than a
/// silent pass.
pub(super) fn pack_root() -> Option<PathBuf> {
    let Some(dir) = crate::config::live_env_os("AUTOSHADE_LR_PACK") else {
        println!("skipped: AUTOSHADE_LR_PACK is unset (the me6-2026-09 Lightroom pack)");
        return None;
    };
    let root = PathBuf::from(dir);
    if !root.join("base.ARW").is_file() || !root.join("xmp").is_dir() || !root.join("lr").is_dir() {
        println!(
            "skipped: AUTOSHADE_LR_PACK={} has no base.ARW + xmp/ + lr/",
            root.display()
        );
        return None;
    }
    Some(root)
}

/// Every export code in the pack, sorted — the `xmp/` directory IS the list, so
/// a pack with a sidecar the spec forgot still measures it.
pub(super) fn codes(root: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(root.join("xmp"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension()?.eq_ignore_ascii_case("xmp"))
                .then(|| p.file_stem()?.to_str().map(str::to_owned))?
        })
        .collect();
    out.sort();
    out
}

/// The PRODUCTION import shape for a Lightroom sidecar, in one place.
///
/// This is `bin/gui/export.rs`'s `imported_recipe` reduced to what pixels
/// depend on: the sidecar's own edits, the photograph's base look and as-shot
/// white balance, and — the load-bearing one — the SIDECAR-AWARE lens profile,
/// which is the only form that can tell "no warp because Lightroom drew no
/// correction" (`crs:LensProfileEnable="0"`) from "no warp because nobody could
/// solve one".
///
/// `calibration` is the photograph's half — identical for all 46 exports here,
/// since they share one capture — so the caller estimates it once instead of
/// paying a develop per sidecar.
pub(super) fn import(xmp: &str, base: &Path, calibration: &Calibration) -> EditRecipe {
    let mut r = crate::xmp::xmp_to_recipe_for_photo(xmp, base);
    r.base_curve = calibration.base_curve.clone();
    r.as_shot_k = calibration.as_shot_k;
    r.as_shot_tint = calibration.as_shot_tint;
    r.lens_profile = crate::pipeline::fresh_lens_profile_for_sidecar(base, Some(xmp));
    r
}

/// The photograph's own calibration — estimated once for the whole pack.
pub(super) struct Calibration {
    base_curve: Vec<[f32; 2]>,
    as_shot_k: Option<f32>,
    as_shot_tint: Option<f32>,
}

impl Calibration {
    pub(super) fn of(base: &Path) -> Self {
        let (as_shot_k, as_shot_tint) = crate::pipeline::fresh_as_shot_wb(base);
        Self { base_curve: crate::pipeline::photo_base_knots(base), as_shot_k, as_shot_tint }
    }
}

/// The engine's mask α over one frame, evaluated with the SAME primitives
/// `apply_masks` uses — `combined_mask_weight` at pixel centres, through the
/// caller's [`super::MaskFrame`] — and combined across the recipe's masks the
/// way overlapping coverage composes (`1 − Π(1 − αᵢ·amountᵢ)`).
///
/// A 16-bit grey raster, so the quantisation floor is 1/65535 in α rather than
/// the 8-bit overlay's 1/255 — the measurement's own floor is Lightroom's
/// 8-bit JPEG, and an instrument must not sit AT the floor it is measuring.
fn alpha_raster(
    recipe: &EditRecipe,
    frame: super::MaskFrame<'_>,
    dims: (u32, u32),
) -> image::ImageBuffer<image::Luma<u16>, Vec<u16>> {
    let (w, h) = dims;
    let fdims = (w as f32, h as f32);
    let mut out = image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::new(w, h);
    // One prepared mask per adjustment, exactly as `apply_masks` prepares them:
    // LINEAR's corrections-off handle transport happens once, above the pixel
    // loop, never per sample.
    let prepared: Vec<_> =
        recipe.masks.iter().map(|m| frame.linear_handles_to_raw(m, fdims)).collect();
    let unwarp = frame.unwarp(fdims);
    let unwarp = unwarp.as_ref();
    for (x, y, px) in out.enumerate_pixels_mut() {
        let nx = (x as f32 + super::MASK_SAMPLE_CENTRE) / w as f32;
        let ny = (y as f32 + super::MASK_SAMPLE_CENTRE) / h as f32;
        let mut open = 1.0f32;
        for m in &prepared {
            let m = m.as_ref();
            if !m.enabled {
                continue;
            }
            let mut wgt = super::combined_mask_weight(m, nx, ny, None, &[], unwarp, fdims);
            if m.inverted {
                wgt = 1.0 - wgt;
            }
            open *= 1.0 - (wgt * m.amount.clamp(0.0, 1.0)).clamp(0.0, 1.0);
        }
        *px = image::Luma([((1.0 - open).clamp(0.0, 1.0) * 65535.0).round() as u16]);
    }
    out
}

/// Render every sidecar in the pack at FULL resolution and write the pixels
/// `scripts/lr_mask_parity.py` measures.
///
/// Output, under `<AUTOSHADE_DATA_DIR>/lr-probe/`:
/// * `<code>.png` — the engine's own export, 16-bit RGB, no output sharpening
///   and no long-edge resample, which is `apply --long-edge 0` exactly.
/// * `<code>.alpha-stored.png` — the engine's mask α in the frame Lightroom
///   rasterises in, BEFORE this engine's own geometry stage. This is the mask
///   LAW on its own: no tone chain, no lens geometry, no orientation.
/// * `<code>.alpha.png` — the same α carried through `apply_lens_geometry`,
///   which is the coverage the render actually DELIVERS.
/// * `meta.json` — per export: the frame, how many masks survived import, what
///   the import refused, and the lens-profile facts the frame law depends on.
///
/// Renders run one at a time. Each develop is already row-parallel over the
/// whole machine (rayon), a 26 MP develop holds ~300 MB of f32 planes, and this
/// probe shares the box with other work — so a second concurrent render buys
/// nothing and costs memory.
#[test]
#[ignore = "renders 46 full-resolution developes from the me6-2026-09 Lightroom pack (~15 min)"]
fn export_lr_pack_renders_for_the_mask_measurement() {
    let Some(root) = pack_root() else { return };
    let base = root.join("base.ARW");
    let out = crate::store::store_root().join("lr-probe");
    std::fs::create_dir_all(&out).expect("create the probe output directory");

    let calibration = Calibration::of(&base);
    // A comma-separated subset, for re-rendering a handful of codes under an
    // experimental engine change without paying for the other forty. Unset
    // means the whole pack, which is what the measurement runs on.
    let only = crate::config::live_env_os("AUTOSHADE_LR_PACK_ONLY")
        .map(|v| v.to_string_lossy().split(',').map(|s| s.trim().to_owned()).collect::<Vec<_>>());
    let mut meta = serde_json::Map::new();
    for code in codes(&root) {
        if only.as_ref().is_some_and(|keep| !keep.contains(&code)) {
            continue;
        }
        let started = std::time::Instant::now();
        let xmp = std::fs::read_to_string(root.join("xmp").join(format!("{code}.xmp")))
            .unwrap_or_else(|e| panic!("read the {code} sidecar: {e}"));
        let recipe = import(&xmp, &base, &calibration);
        let losses = crate::xmp::import_losses_for_photo(&xmp, &base);
        let img = super::render_to_image(&base, &recipe, None, None)
            .unwrap_or_else(|e| panic!("render {code}: {e}"));
        let (w, h) = (img.width(), img.height());
        img.to_rgb16()
            .save(out.join(format!("{code}.png")))
            .unwrap_or_else(|e| panic!("write the {code} render: {e}"));
        drop(img);

        // The α pair, in the develop's own working frame — which is the
        // DELIVERED frame: `render_to_image_in` orients the demosaiced buffer
        // (`orient_f32`) before `apply_develop`, so the mask loop and this
        // raster see the same rectangle. `geometry_profile` +
        // `MaskFrame::downstream` are the two lines the develop itself runs, so
        // the raster below is the coverage the render applied, not a second
        // opinion about it.
        //
        // Until W14 this line TRANSPOSED the delivered dims to reach
        // Lightroom's landscape frame, because the render came out portrait
        // (the sidecar's `tiff:Orientation` was not read, so `quarter_turns`
        // stayed 0 and the RAW's EXIF 8 turned the frame Lightroom does not
        // turn). The compensation is gone with the defect: 6240 × 4160 either
        // way, and now for the right reason.
        let dims = (w, h);
        let geom = super::geometry_profile(&recipe);
        let stored = alpha_raster(
            &recipe,
            super::MaskFrame::downstream(&geom, recipe.lens_distortion),
            dims,
        );
        stored
            .save(out.join(format!("{code}.alpha-stored.png")))
            .unwrap_or_else(|e| panic!("write the {code} stored α: {e}"));
        // CA is dropped for the delivered α on purpose: it scales the red and
        // blue planes against green, so keeping it would hand back three
        // DIFFERENT α fields for one mask. Distortion — the part that moves a
        // mask — is kept exactly as the render applies it.
        let geometry = crate::recipe::LensProfile {
            ca_on: false,
            ca_r: Vec::new(),
            ca_b: Vec::new(),
            ..geom.as_ref().clone()
        };
        let carried = super::apply_lens_geometry(
            &image::DynamicImage::ImageLuma16(stored),
            &geometry,
            recipe.lens_distortion,
        );
        carried
            .to_luma16()
            .save(out.join(format!("{code}.alpha.png")))
            .unwrap_or_else(|e| panic!("write the {code} delivered α: {e}"));

        let profile = &recipe.lens_profile;
        meta.insert(
            code.clone(),
            serde_json::json!({
                "width": w,
                "height": h,
                "masks": recipe.masks.len(),
                "import_losses": losses.len(),
                "lens_profile_enable": crate::xmp::lens_profile_enabled(&xmp),
                "mask_warp_src": format!("{:?}", profile.mask_warp_src),
                "mask_warp_knots": profile.mask_warp.len(),
                "linear_handle_warp_knots": profile.linear_handle_warp.len(),
                "distortion_on": profile.distortion_on,
                "vignette_on": profile.vignette_on,
                "ca_on": profile.ca_on,
                "seconds": started.elapsed().as_secs_f32(),
            }),
        );
        println!("{code}: {w}x{h}, {} mask(s), {:.1} s", recipe.masks.len(), started.elapsed().as_secs_f32());
    }
    let written = meta.len();
    std::fs::write(
        out.join("meta.json"),
        serde_json::to_string_pretty(&serde_json::Value::Object(meta)).expect("serialise meta"),
    )
    .expect("write meta.json");
    println!("wrote {written} renders to {}", out.display());
}

// ---------------------------------------------------------------------------
// What the pack PINS.
//
// These run in the ordinary lib suite whenever `AUTOSHADE_LR_PACK` names the
// fixture and print a skip reason when it does not. None of them renders: the
// import shape and the mask primitives are what the exports convicted, and both
// are reachable without a develop. The full numbers, and the script that
// reproduces them, are in `scripts/lr_mask_parity.py`.
// ---------------------------------------------------------------------------

/// The pack's own spec, as the tests below read it.
fn spec(root: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(root.join("pack-spec.json")).expect("read pack-spec.json");
    serde_json::from_str(&text).expect("parse pack-spec.json")
}

/// One export's green plane, full size. Lightroom wrote q100 sRGB JPEGs, so
/// this is the delivered pixel and nothing else.
fn lr_green(root: &Path, code: &str) -> Vec<u8> {
    let path = root.join("lr").join(format!("{code}.jpg"));
    let img = image::open(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    img.to_rgb8().pixels().map(|p| p.0[1]).collect()
}

/// The largest per-pixel difference between two exports' green planes.
fn max_delta(a: &[u8], b: &[u8]) -> u8 {
    assert_eq!(a.len(), b.len(), "two exports of one capture must be the same size");
    a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)
}

/// The engine's α along the horizontal ray from a mask's centre, as the
/// normalised x where the coverage passes ½. Feather 0 makes that a hard edge,
/// so the bisection lands ON the boundary rather than inside a ramp.
fn alpha_half_crossing_x(m: &crate::recipe::LocalAdjustment, dims: (f32, f32)) -> f32 {
    let (mut lo, mut hi) = (0.5f32, 1.0f32);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if super::combined_mask_weight(m, mid, 0.5, None, &[], None, dims) >= 0.5 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Every correction Lightroom left ACTIVE imports, and the only ones this
/// engine names as drops are the ones Lightroom itself switched off.
///
/// This is the test the pack was worth having. Before it, group D's nine
/// exports — a plain box (`Left < Right`, `Top < Bottom`) at `crs:Angle="30"`,
/// which folds to `b = −0.020096` — were all refused by an `a > 0 ∧ b > 0`
/// guard in [`crate::xmp`]'s radial decode, and nine of the pack's forty-six
/// masks rendered as nothing at all (pooled rms(α) 0.3931 against Lightroom).
/// Lightroom draws the ellipse of the folded MAGNITUDES: imported that way the
/// same nine score 0.0069, the same order as every other family in the pack.
/// The refusal was the engine's, not the file's.
///
/// MUTATION THIS CATCHES: restore the sign half of that guard and this counts
/// 9 missing masks and 9 unexpected drops.
#[test]
fn the_pack_imports_every_active_correction_and_names_only_the_inactive() {
    let Some(root) = pack_root() else { return };
    let base = root.join("base.ARW");
    let spec = spec(&root);
    let exports = spec["exports"].as_array().expect("exports[]");
    assert_eq!(exports.len(), 46, "the me6-2026-09 pack is 46 exports");
    for export in exports {
        let code = export["code"].as_str().expect("code");
        let written = export["corrections"].as_array().expect("corrections[]").len();
        let xmp = std::fs::read_to_string(root.join("xmp").join(format!("{code}.xmp")))
            .unwrap_or_else(|e| panic!("read the {code} sidecar: {e}"));
        // The sidecar, not the spec, says which corrections are switched off:
        // `E-CLICK.xmp` was rewritten by Lightroom after the user hid a mask,
        // and the spec row still describes what was asked for.
        let inactive = xmp.matches("crs:CorrectionActive=\"false\"").count();
        let recipe = crate::xmp::xmp_to_recipe_for_photo(&xmp, &base);
        assert_eq!(
            recipe.masks.len(),
            written - inactive,
            "{code}: {written} correction(s) written, {inactive} switched off"
        );
        assert_eq!(
            crate::xmp::import_losses_for_photo(&xmp, &base).len(),
            inactive,
            "{code}: the only named drops are the corrections Lightroom switched off"
        );
    }
}

/// A rotation made in Lightroom survives the import, so the engine delivers
/// Lightroom's own frame.
///
/// The pack is the measurement that convicted this: `base.ARW` carries IFD0
/// `Orientation = 8` (`Rotate270`, portrait), all 46 sidecars declare
/// `tiff:Orientation="1"`, and Lightroom exported 6240 × 4160 LANDSCAPE from
/// every one of them — so Lightroom honours the sidecar over the RAW's EXIF,
/// and `E-CLICK.xmp`, the one sidecar Lightroom 9.4 rewrote itself, wrote that
/// same `"1"` back beside corrected 6240 × 4160 dimensions.
///
/// The import therefore has to bring back `quarter_turns = 1`, which composes
/// with `Rotate270` to `Normal` — the landscape frame the exports are in
/// (`render::quarter_turns_between` is the law and the sign convention).
/// Before W14 it brought back 0 and all 46 renders came out 4160 × 6240
/// portrait; the probe's own `meta.json` from the W10 round says so, 46 for
/// 46.
///
/// MUTATION THIS CATCHES: stop reading `tiff:Orientation` on import (or hand
/// `honour_declared_orientation` the declared state as its own EXIF) and all
/// 46 come back `quarter_turns = 0`, delivering the portrait frame Lightroom
/// never produced.
#[test]
fn the_packs_lightroom_rotation_survives_the_import() {
    let Some(root) = pack_root() else { return };
    let base = root.join("base.ARW");
    let ((sw, sh), exif) =
        crate::decode::source_frame(&base).expect("read the pack RAW's source frame");
    assert_eq!(exif, rawler::Orientation::Rotate270, "the pack's RAW is IFD0 Orientation = 8");
    assert_eq!((sw, sh), (6240, 4160), "the pack's source rectangle");
    let codes = codes(&root);
    assert_eq!(codes.len(), 46, "the me6-2026-09 pack is 46 sidecars");
    for code in codes {
        let xmp = std::fs::read_to_string(root.join("xmp").join(format!("{code}.xmp")))
            .unwrap_or_else(|e| panic!("read the {code} sidecar: {e}"));
        assert!(xmp.contains("tiff:Orientation=\"1\""), "{code} declares Lightroom's rotation");
        let recipe = crate::xmp::xmp_to_recipe_for_photo(&xmp, &base);
        assert_eq!(recipe.quarter_turns, 1, "{code}: the sidecar's rotation did not survive");
        // …and that turn is what makes the delivered frame Lightroom's. This
        // is `decode::frame_size_turned`'s own arithmetic, run off the one
        // header read above instead of re-reading the RAW forty-six times.
        let delivered = crate::render::compose_orientation(exif, recipe.quarter_turns);
        let dims = if crate::decode::orientation_transposes(delivered) { (sh, sw) } else { (sw, sh) };
        assert_eq!(dims, (6240, 4160), "{code}: the delivered frame is not Lightroom's");
    }

    // …and the other half of the law, on the same capture: a declaration this
    // engine has no stage to deliver. `tiff:Orientation="5"` is `Transpose`, a
    // MIRROR, and the capture's own `Rotate270` is not — no quarter turn
    // crosses that, so the tag is refused WHOLE and the refusal is NAMED on
    // the diagnostics channel the rest of the import discloses through. The
    // photograph's own EXIF then decides, exactly as it does for a sidecar
    // that declares nothing, so the mask still lands on the part of the
    // picture Lightroom put it on and only the handedness is lost.
    let mirrored = std::fs::read_to_string(root.join("xmp").join("C-A12-F25.xmp"))
        .expect("read the C-A12-F25 sidecar")
        .replace("tiff:Orientation=\"1\"", "tiff:Orientation=\"5\"");
    let sink = crate::diag::Collector::new();
    let recipe = crate::xmp::xmp_to_recipe_with_diag(
        &mirrored,
        &crate::diag::Diag::about(&sink, &base),
    );
    assert_eq!(recipe.quarter_turns, 0, "a refused tag leaves the photo's own EXIF in charge");
    let said: Vec<String> = sink.take().into_iter().map(|l| l.text).collect();
    assert_eq!(said.len(), 1, "exactly one line, and it is the refusal: {said:?}");
    assert!(
        said[0].contains("tiff:Orientation=\"5\"")
            && said[0].contains("mirrors the frame")
            && said[0].contains("EXIF orientation kept"),
        "the refusal must name the value, the reason and what was kept: {}",
        said[0]
    );
}

/// Roundness moves NOTHING. Lightroom's own exports at Roundness −100, 0 and
/// +100 are the same pixels — `max|Δ| = 0` over 26 Mpx, three times, at
/// feather 25/50/75 on a tilted 2:1 ellipse — so this engine's documented
/// no-op is the correct model and not an approximation. It also retires the
/// "±4 DN zero-mean dither" the earlier round wrote down: at q100 there is none.
///
/// MUTATION THIS CATCHES: make `roundness` do anything at all in
/// [`super::mask_weight`] and the engine half of this test separates.
#[test]
fn lightroom_and_the_engine_both_draw_one_ellipse_for_every_roundness() {
    let Some(root) = pack_root() else { return };
    let base = root.join("base.ARW");
    for feather in ["25", "50", "75"] {
        let codes = [
            format!("D-R-100-F{feather}"),
            format!("D-R+0-F{feather}"),
            format!("D-R+100-F{feather}"),
        ];
        let lr: Vec<Vec<u8>> = codes.iter().map(|c| lr_green(&root, c)).collect();
        assert_eq!(max_delta(&lr[0], &lr[1]), 0, "F{feather}: Lightroom moved a pixel from R−100 to R0");
        assert_eq!(max_delta(&lr[1], &lr[2]), 0, "F{feather}: Lightroom moved a pixel from R0 to R+100");

        // …and the engine agrees, on the same three sidecars, on a grid coarse
        // enough to stay a unit test and fine enough that a one-pixel edge
        // shift at 6240 px would show as an α difference.
        let dims = (6240.0f32, 4160.0f32);
        let alpha = |code: &String| {
            let xmp = std::fs::read_to_string(root.join("xmp").join(format!("{code}.xmp")))
                .unwrap_or_else(|e| panic!("read the {code} sidecar: {e}"));
            let recipe = crate::xmp::xmp_to_recipe_for_photo(&xmp, &base);
            assert_eq!(recipe.masks.len(), 1, "{code} did not import its one mask");
            let m = recipe.masks[0].clone();
            (0..312 * 208)
                .map(|i| {
                    let (x, y) = ((i % 312) as f32, (i / 312) as f32);
                    super::combined_mask_weight(
                        &m,
                        (x + super::MASK_SAMPLE_CENTRE) / 312.0,
                        (y + super::MASK_SAMPLE_CENTRE) / 208.0,
                        None,
                        &[],
                        None,
                        dims,
                    )
                })
                .collect::<Vec<f32>>()
        };
        let engine: Vec<Vec<f32>> = codes.iter().map(alpha).collect();
        assert_eq!(engine[0], engine[1], "F{feather}: the engine moved α from R−100 to R0");
        assert_eq!(engine[1], engine[2], "F{feather}: the engine moved α from R0 to R+100");
    }
}

/// ONE LAW, THREE FACES: the CLI, the GUI and the `xmp` round trip all save
/// through `pipeline::write_xmp` and nothing else (`bin/gui/actions.rs`,
/// `bin/gui/export.rs` and `bin/gui/workers.rs` each call it directly, and the
/// GUI has no XMP writer of its own), so this drives that one door on the
/// pack's real bytes.
///
/// Two facts, and they pull against each other. A save that never touched
/// `tiff:Orientation` could not carry a rotation made here to Lightroom; a
/// save that always rewrote it would move a real Lightroom sidecar's bytes on
/// every round trip. So: an unchanged recipe composes back to the value
/// already in the file and nothing is written, and a turn made here composes
/// to a new value that IS written and comes back on the next import.
///
/// The capture is HARD-LINKED, not copied — 53 MB, and a copy would be the
/// only expensive line here.
///
/// MUTATION THIS CATCHES: drop the `quarter_turns` arm from `write_xmp_doc`'s
/// frame gate and the LAST save below — a recipe carrying nothing but the turn
/// — writes a sidecar that declares no orientation at all, so a photo the user
/// only rotated leaves the app with its rotation lost. (The masked saves above
/// survive that mutation: their geometry opens the same gate.)
#[test]
fn one_law_three_faces_a_turn_saved_here_reaches_lightroom_and_comes_back() {
    let Some(root) = pack_root() else { return };
    let dir = crate::store::store_root().join("w14-orientation-round-trip");
    std::fs::create_dir_all(&dir).expect("create the round-trip directory");
    let raw = dir.join("base.ARW");
    let _ = std::fs::remove_file(&raw);
    if std::fs::hard_link(root.join("base.ARW"), &raw).is_err() {
        std::fs::copy(root.join("base.ARW"), &raw).expect("place the pack's capture");
    }
    // The Lightroom sidecar, beside the RAW, which is where `write_xmp` looks
    // for its merge base.
    let lr = dir.join("base.xmp");
    std::fs::copy(root.join("xmp").join("C-A12-F25.xmp"), &lr).expect("place the pack's sidecar");
    let text = std::fs::read_to_string(&lr).expect("read the placed sidecar");

    // FACE 1 — import. The sidecar's rotation arrives as the photographer's.
    let recipe = crate::xmp::xmp_to_recipe_for_photo(&text, &raw);
    assert_eq!(recipe.quarter_turns, 1, "the sidecar's rotation did not survive the import");
    assert_eq!(recipe.masks.len(), 1, "and its one radial came with it");

    // FACE 2 — save it back unchanged. The tag Lightroom wrote is still the
    // tag in the file, exactly once.
    let save = |r: &EditRecipe| {
        let (out, _, _) = crate::pipeline::write_xmp(&raw, r, crate::diag::silent())
            .expect("write the sidecar projection");
        std::fs::read_to_string(&out).expect("read the sidecar projection back")
    };
    let unchanged = save(&recipe);
    assert_eq!(unchanged.matches("tiff:Orientation").count(), 1, "{unchanged}");
    assert!(unchanged.contains("tiff:Orientation=\"1\""), "{unchanged}");
    assert_eq!(
        crate::xmp::xmp_to_recipe_for_photo(&unchanged, &raw).quarter_turns,
        1,
        "the round trip lost the turn"
    );

    // FACE 3 — turn it here, the way the toolbar does (`pipeline::rotate_recipe`
    // is what `gui::actions::rotate_photo` calls), and save again. The
    // declaration follows the composed state: one more clockwise turn on top
    // of `Normal` is `Rotate90`, which is `tiff:Orientation="6"`.
    let mut turned = recipe.clone();
    crate::pipeline::rotate_recipe(&mut turned, &raw, 1).expect("turn the recipe");
    assert_eq!(turned.quarter_turns, 2, "one turn on top of the sidecar's own");
    let after = save(&turned);
    assert_eq!(after.matches("tiff:Orientation").count(), 1, "{after}");
    assert!(after.contains("tiff:Orientation=\"6\""), "the turn did not reach Lightroom: {after}");
    assert_eq!(
        crate::xmp::xmp_to_recipe_for_photo(&after, &raw).quarter_turns,
        2,
        "the turn did not come back"
    );

    // …and a turn with NOTHING ELSE on it. A recipe holding no coordinate at
    // all still has a frame fact to declare, and `write_xmp_doc`'s gate has to
    // fetch the photograph's frame for it — otherwise the sidecar of a photo
    // the user only rotated declares no orientation and the rotation never
    // leaves the app. Three quarter turns on top of the capture's `Rotate270`
    // is `Rotate180`, i.e. `tiff:Orientation="3"`.
    let bare = EditRecipe { quarter_turns: 3, ..Default::default() };
    let bare_doc = save(&bare);
    assert_eq!(bare_doc.matches("tiff:Orientation").count(), 1, "{bare_doc}");
    assert!(
        bare_doc.contains("tiff:Orientation=\"3\""),
        "a turn with no geometry did not reach Lightroom: {bare_doc}"
    );
    assert_eq!(
        crate::xmp::xmp_to_recipe_for_photo(&bare_doc, &raw).quarter_turns,
        3,
        "the bare turn did not come back"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// `crs:CorrectionActive="false"` — the mask eye — read three ways.
///
/// `E-EYE-OFF` is the key written by hand into a Lightroom sidecar; `E-CLICK`
/// is the SAME key written by Lightroom 9.4 itself after the user hid the mask
/// and pressed Ctrl+S. Lightroom renders the two identically (`max|Δ| = 0`),
/// and both differ from `E-BOTH` by 73 DN, so the key is the whole difference
/// and hand-writing it is a faithful edit. This engine drops the correction on
/// import and names it, which is the same render; and the sidecar's own bytes
/// survive a merge, so the eye a user set in Lightroom is still set after a
/// round trip through this app.
///
/// MUTATION THIS CATCHES: stop reading `crs:CorrectionActive` in
/// [`crate::xmp`] and `E-CLICK` imports two masks instead of one.
#[test]
fn an_inactive_correction_is_skipped_by_both_engines_and_survives_a_merge() {
    let Some(root) = pack_root() else { return };
    let base = root.join("base.ARW");
    let (both, off, click) =
        (lr_green(&root, "E-BOTH"), lr_green(&root, "E-EYE-OFF"), lr_green(&root, "E-CLICK"));
    assert_eq!(max_delta(&off, &click), 0, "Lightroom read our key and its own key differently");
    assert_eq!(max_delta(&both, &off), 73, "the hidden mask is worth 73 DN in Lightroom's export");
    assert_eq!(max_delta(&both, &click), 73, "…and the same 73 DN when Lightroom wrote the key");

    let read = |code: &str| {
        let xmp = std::fs::read_to_string(root.join("xmp").join(format!("{code}.xmp")))
            .unwrap_or_else(|e| panic!("read the {code} sidecar: {e}"));
        let n = crate::xmp::xmp_to_recipe_for_photo(&xmp, &base).masks.len();
        (xmp, n)
    };
    let (both_xmp, n_both) = read("E-BOTH");
    let (click_xmp, n_click) = read("E-CLICK");
    assert_eq!(n_both, 2, "E-BOTH's two corrections are both active");
    assert_eq!(n_click, 1, "E-CLICK's hidden correction must not render");
    assert!(!both_xmp.contains("crs:CorrectionActive=\"false\""), "E-BOTH hides nothing");

    // The merge path preserves a foreign mask group verbatim, so a develop
    // that adds no masks of its own hands the eye back exactly as Lightroom
    // left it. This is the round trip of the key itself.
    let merged = crate::xmp::merge_recipe_into_xmp(
        &click_xmp,
        &EditRecipe { exposure_ev: 0.25, ..Default::default() },
    )
    .expect("the pack's sidecar is mergeable");
    assert!(
        merged.doc.contains("crs:CorrectionActive=\"false\""),
        "the mask eye did not survive the merge"
    );
}

/// The radial boundary is the STORED ellipse, at every mask size.
///
/// Group A holds three feather-0 radials centred on the frame — 0.30 × 0.32,
/// 0.50 × 0.50 and 0.70 × 0.63 of it — with the lens correction off. The
/// engine puts its α edge on the stored ellipse for all three; Lightroom puts
/// its own at ρ = 0.99880 / 0.99875 / 0.99872 of it, which is a pure SCALE and
/// not a dilation that grows with radius, so the residual is
/// −1.12 / −1.96 / −2.79 px purely because the ellipse is bigger. (With the
/// correction ON the residual grows to −5.55 / −17.53 / −38.46 px, which is
/// the lens map sampled at the mask's own radius — a different mechanism, and
/// the one the `.lcp` experiment in the W10 report refuted an alternative for.)
///
/// The 0.99876 mean scale is the standing 2026-08-19 ruling
/// `LR_MASK_FRAME_SCALE = 1.0` re-measured on a second frame, not a new fact.
///
/// MUTATION THIS CATCHES: put any `k ≠ 1` back into `LR_MASK_FRAME_SCALE` and
/// all three boundaries leave the stored ellipse together.
#[test]
fn the_packs_radial_boundary_is_the_stored_ellipse_at_every_size() {
    let Some(root) = pack_root() else { return };
    let base = root.join("base.ARW");
    let dims = (6240.0f32, 4160.0f32);
    // (code, stored right edge, Lightroom's measured ρ, its residual in px)
    let sizes = [
        ("A-S-OFF", 0.65f32, 0.99880f32, -1.12f32),
        ("A-M-OFF", 0.75, 0.99875, -1.96),
        ("A-L-OFF", 0.85, 0.99872, -2.79),
    ];
    for (code, right, rho_lr, residual_px) in sizes {
        let xmp = std::fs::read_to_string(root.join("xmp").join(format!("{code}.xmp")))
            .unwrap_or_else(|e| panic!("read the {code} sidecar: {e}"));
        let recipe = crate::xmp::xmp_to_recipe_for_photo(&xmp, &base);
        assert_eq!(recipe.masks.len(), 1, "{code} did not import its one mask");
        let got = alpha_half_crossing_x(&recipe.masks[0], dims);
        let error_px = (got - right).abs() * dims.0;
        assert!(error_px < 0.05, "{code}: the engine's edge is {error_px:.4}px off the stored ellipse");
        // …and the published residual IS that scale times this semi-axis, so
        // the table and the constant cannot drift apart unnoticed.
        let semi_axis_px = (right - 0.5) * dims.0;
        let want = (rho_lr - 1.0) * semi_axis_px;
        assert!(
            (want - residual_px).abs() < 0.05,
            "{code}: ρ {rho_lr} on a {semi_axis_px:.1}px semi-axis is {want:.2}px, table says {residual_px:.2}px"
        );
    }
}

/// The shipped LINEAR warp reproduces the pack's measured half-coverage line.
///
/// Group B's twelve gradients put Lightroom's α = ½ contour at
/// t = 0.5411 (the six vertical ones) and t = 0.5459 (the six horizontal),
/// against t = ½ for a plain Hermite smoothstep — +34.2 px and +38.2 px on an
/// 832 px handle span, identical across the three positions and across the
/// lens-correction twins. [`super::LINEAR_FALLOFF_WARP`] is the exponent that
/// closes it, and this is the arithmetic that ties the constant to the
/// measurement: `smoothstep(t^q) = ½` at `t = ½^(1/q)`.
///
/// No fixture: the twelve numbers are constants, so this runs everywhere.
///
/// MUTATION THIS CATCHES: set `LINEAR_FALLOFF_WARP` back to 1.0 and the
/// crossing returns to 0.5000, 0.041 away from the vertical gradients' mean.
#[test]
fn the_shipped_linear_warp_lands_on_the_packs_measured_half_coverage() {
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if super::linear_coverage(mid, super::LINEAR_FALLOFF) < 0.5 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let t50 = 0.5 * (lo + hi);
    // The two axes' means and the 0.0092 between-gradient spread the twelve
    // gradients measured: the engine must sit inside that instrument, not just
    // near it.
    for (axis, measured) in [("vertical", 0.5411f32), ("horizontal", 0.5459)] {
        assert!(
            (t50 - measured).abs() < 0.01,
            "{axis} gradients measured t50 {measured}, the shipped profile crosses at {t50:.4}"
        );
    }
    // …and the unwarped smoothstep does NOT, which is what made the warp
    // necessary rather than cosmetic.
    assert!(
        (0.5f32 - 0.5411).abs() > 0.01,
        "premise: an unwarped smoothstep would already be inside the instrument"
    );
}
