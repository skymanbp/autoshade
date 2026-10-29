//! The camera's own numbers: its colour matrix, calibration and profile, and the calibrated camera-native buffer.

use super::*;

/// The camera's xyz→cam matrix (3-colour), by rawler's own selection rule:
/// the D65-illuminant matrix first, else whichever exists.
pub(super) fn camera_matrix(rawimage: &rawler::RawImage) -> Result<[[f32; 3]; 3]> {
    let cm = rawimage
        .color_matrix
        .iter()
        .find(|(i, _)| **i == rawler::imgop::xyz::Illuminant::D65)
        .or_else(|| rawimage.color_matrix.iter().next())
        .map(|(_, m)| m)
        .ok_or_else(|| anyhow!("no camera colour matrix — the wide-gamut develop needs one"))?;
    mat3_from_slice(cm)
        .ok_or_else(|| anyhow!("camera colour matrix has {} entries (need 9)", cm.len()))
}

/// rawler's AsShotNeutral convention: `wb[0]` NaN ⇒ WB unknown ⇒ neutral
/// [1,1,1]. Anything else passes through for [`validate_calibration`] to
/// judge — an INFINITE coefficient (1/0 from a zero AsShotNeutral, rawler
/// dng.rs), a negative, or a partial NaN is corrupt metadata, not
/// "unknown", and the old `[0].is_nan()`-only guard let all three through.
pub(super) fn normalise_wb(wb: [f32; 4]) -> [f32; 3] {
    if wb[0].is_nan() { [1.0, 1.0, 1.0] } else { [wb[0], wb[1], wb[2]] }
}

/// Validate the FILE-SUPPLIED calibration before it reaches [`inv3`] and
/// the pixel chain (L04-1). `to_u16` clamps silently — NaN quantises to 0,
/// inf saturates to 65535 — so a singular matrix or corrupt WB published a
/// committed all-black/all-white deliverable with `Ok` status. Every bail
/// names the file and the measured value; no output is written.
///
/// The conditioning check runs on the matrix the render ACTUALLY inverts
/// (Codex AL-review F3): `space2cam = xyz2cam × rgb_to_xyz(space)`, row-
/// normalised exactly as [`camera_to_space_matrix`] does — validating the
/// raw xyz2cam rows alone passed a matrix whose D65-weighted product row
/// sums to ~0 and divides into infinities at render time. The raw-row
/// degeneracy check is kept as well: no physical matrix has a zero raw row
/// sum, and refusing garbage early is the cheap direction.
pub(super) fn validate_calibration(
    xyz2cam: &[[f32; 3]; 3],
    wb: [f32; 4],
    space: ExportColorSpace,
    src: &Path,
) -> Result<()> {
    for row in xyz2cam {
        for v in row {
            if !v.is_finite() {
                bail!(
                    "camera colour matrix of {} has a non-finite entry ({v}) — \
                     the file's calibration metadata is corrupt; no output was written",
                    src.display()
                );
            }
        }
    }
    let mut norm = mat_mul3(xyz2cam, &rgb_to_xyz(space_primaries(space), D65_XY));
    for (i, row) in norm.iter_mut().enumerate() {
        let raw = xyz2cam[i][0] + xyz2cam[i][1] + xyz2cam[i][2];
        let s = row[0] + row[1] + row[2];
        if raw.abs() <= 1e-6 || s.abs() <= 1e-6 {
            bail!(
                "camera colour matrix of {} has a degenerate row (raw sum {raw:e}, \
                 white-weighted sum {s:e}) — the file's calibration metadata is \
                 corrupt; no output was written",
                src.display()
            );
        }
        for v in row.iter_mut() {
            *v /= s;
        }
    }
    let c00 = norm[1][1] * norm[2][2] - norm[1][2] * norm[2][1];
    let c01 = norm[1][2] * norm[2][0] - norm[1][0] * norm[2][2];
    let c02 = norm[1][0] * norm[2][1] - norm[1][1] * norm[2][0];
    let det = norm[0][0] * c00 + norm[0][1] * c01 + norm[0][2] * c02;
    // Real camera matrices land at O(0.1–1) here (a sweep of rawler 0.7.2's
    // 1331 bundled matrices bottoms out around 0.22), so 1e-4 leaves ~3
    // decades of headroom.
    if det.abs() < 1e-4 {
        bail!(
            "camera colour matrix of {} is singular or near-singular \
             (row-normalised determinant {det:e}) — inverting it would render \
             the whole frame black; no output was written",
            src.display()
        );
    }
    let wb3 = normalise_wb(wb);
    for v in wb3 {
        if !v.is_finite() || v <= 0.0 {
            bail!(
                "AsShotNeutral/WB coefficients of {} are corrupt ({wb3:?}) — \
                 a zero AsShotNeutral component becomes an infinite multiplier \
                 (all-white frame); no output was written",
                src.display()
            );
        }
    }
    // A REAL fourth coefficient (rawler leaves wb[3] NaN for 3-channel
    // cameras) is consumed by rawler's own develop on 4-colour sensors —
    // validate it the same way (Codex AL-review F4, the shallow half; a
    // true 4-colour intermediate is refused post-develop as before).
    if !wb[3].is_nan() && (!wb[3].is_finite() || wb[3] <= 0.0) {
        bail!(
            "the fourth WB coefficient of {} is corrupt ({}) — \
             no output was written",
            src.display(),
            wb[3]
        );
    }
    Ok(())
}

/// The delivery space's primaries.
pub(super) fn space_primaries(space: ExportColorSpace) -> [[f32; 2]; 3] {
    match space {
        ExportColorSpace::Srgb => SRGB_PRIM,
        ExportColorSpace::DisplayP3 => P3_PRIM,
        ExportColorSpace::AdobeRgb => ADOBE_PRIM,
    }
}

/// (CCT, tint) of the scene illuminant implied by camera WB gains. The gains
/// NEUTRALISE the illuminant, so the illuminant's camera-space colour is
/// their reciprocal; through the camera matrix that becomes XYZ → (x, y) →
/// McCamy's cubic CCT approximation [verified: McCamy 1992,
/// n=(x−0.3320)/(0.1858−y), CCT=449n³+3525n²+6823.3n+5520.33 — reproduces
/// illuminant A at 2856 K and D65 at 6504 K, pinned by test] and a Duv-based
/// tint: the signed CIE-1960 distance from the Planckian locus (Krystek 1985
/// rational fits; above the locus = green), mapped at 3000 tint units per
/// Duv — the scale that lands D65 (Duv ≈ +0.0032) on ≈ +10, ACR's own
/// Daylight-preset tint, which pins both sign and magnitude. ACR's exact
/// model is proprietary — this is a documented approximation (the
/// `local_temp_to_kelvin` stance), never pixel math. Since v1.17.0 the
/// as-shot anchor is Camera Raw's own reading ([`super::as_shot_wb`]); this
/// one is left as the profile stage's fallback temperature.
pub(crate) fn wb_to_kelvin_tint(xyz2cam: &[[f32; 3]; 3], wb: [f32; 3]) -> Option<(f32, f32)> {
    if wb.iter().any(|c| !c.is_finite() || *c <= 0.0) {
        return None;
    }
    let neutral = [1.0 / wb[0], 1.0 / wb[1], 1.0 / wb[2]];
    let xyz = mat_vec3(&inv3(xyz2cam), &neutral);
    let sum = xyz[0] + xyz[1] + xyz[2];
    if !sum.is_finite() || sum <= 0.0 {
        return None;
    }
    let (x, y) = (xyz[0] / sum, xyz[1] / sum);
    let d = 0.1858 - y;
    if d.abs() < 1e-6 {
        return None; // McCamy's pole — no real illuminant lives there
    }
    let n = (x - 0.3320) / d;
    let cct = 449.0 * n * n * n + 3525.0 * n * n + 6823.3 * n + 5520.33;
    // McCamy + Krystek's mutual comfort zone (McCamy degrades toward the
    // extremes; Krystek's stated fit ends at 15000 K). Every WB a camera
    // plausibly meters — tungsten 2500 K through deep blue shade ~12000 K —
    // lives well inside. Outside it the METADATA is junk: refuse, keeping
    // the legacy unknown anchor, rather than stamp a wrong absolute label.
    if !cct.is_finite() || !(1667.0..=15_000.0).contains(&cct) {
        return None;
    }
    let (u, v) = uv1960(x, y);
    let (up, vp) = planck_uv1960(cct);
    let dist = ((u - up).powi(2) + (v - vp).powi(2)).sqrt();
    let duv = if v >= vp { dist } else { -dist };
    let tint = (duv * 3000.0).clamp(-100.0, 100.0);
    // Whole-Kelvin quantisation AT THE SOURCE: the XMP serialises integer
    // Kelvin, so a fractional anchor would round-trip as a target a fraction
    // off the anchor — a shift where none was intended (and below ~2500 K
    // one that escapes apply_wb's 1e-3 neutral short-circuit).
    Some((cct.clamp(2000.0, 40000.0).round(), tint))
}

/// CIE 1960 (u, v) from chromaticity (x, y) — the space Duv is defined in.
fn uv1960(x: f32, y: f32) -> (f32, f32) {
    let den = -2.0 * x + 12.0 * y + 3.0;
    (4.0 * x / den, 6.0 * y / den)
}

/// Planckian locus in CIE 1960 (u, v): Krystek 1985 rational approximation
/// (stated fit range 1000–15000 K; beyond that it extrapolates smoothly —
/// acceptable for a tint DISPLAY value, and daylight lives well inside).
// The literals are Krystek's PUBLISHED coefficients verbatim — truncating to
// f32-representable digits parses to the same bits but breaks checkability
// against the source.
#[allow(clippy::excessive_precision)]
fn planck_uv1960(t: f32) -> (f32, f32) {
    let t2 = t * t;
    let u = (0.860_117_757 + 1.541_182_54e-4 * t + 1.286_412_12e-7 * t2)
        / (1.0 + 8.424_202_35e-4 * t + 7.081_451_63e-7 * t2);
    let v = (0.317_398_726 + 4.228_062_45e-5 * t + 4.204_816_91e-8 * t2)
        / (1.0 - 2.897_418_16e-5 * t + 1.614_560_53e-7 * t2);
    (u, v)
}

/// cam→space by the DNG white-preservation rule: space→cam =
/// xyz2cam · M(space→XYZ) with each row normalised to sum 1 (a
/// white-balanced grey then maps to the SAME grey in every space), inverted.
pub(super) fn camera_to_space_matrix(xyz2cam: &[[f32; 3]; 3], space: ExportColorSpace) -> [[f32; 3]; 3] {
    let mut space2cam = mat_mul3(xyz2cam, &rgb_to_xyz(space_primaries(space), D65_XY));
    for row in &mut space2cam {
        // Unconditional (L04-1): the sole production caller is the
        // calibrate path, whose matrix `validate_calibration` has already
        // refused when any row is degenerate — so the old silent
        // `if s.abs() > 1e-6` skip, which quietly dropped the DNG
        // white-preservation rule for exactly the broken inputs, no longer
        // has a case to hide. (A zero row here would now divide to
        // inf/NaN — loud downstream — instead of passing un-normalised
        // as a plausible-but-wrong calibration.)
        let s = row[0] + row[1] + row[2];
        for v in row {
            *v /= s;
        }
    }
    inv3(&space2cam)
}

/// White-balance + calibrate a camera-native LINEAR buffer into `space`'s
/// primaries and encode the sRGB transfer — WITHOUT a gamut clip: a colour
/// outside sRGB but inside the delivery gamut is exactly what a wide-gamut
/// export exists to carry (rawler's own calibrate kills it). Components
/// outside the DELIVERY gamut go negative here and clip at the final
/// 16-bit pack. Highlights (any component > 1 after white balance) get the
/// same desaturating treatment rawler's develop applies — scale-to-max
/// averaged with the euclidean norm — so wide and sRGB renders treat blown
/// areas alike. The transfer's linear segment covers negatives (no NaN).
pub(super) fn calibrate_camera_buffer(
    data: &mut [[f32; 3]],
    xyz2cam: &[[f32; 3]; 3],
    wb: [f32; 3],
    space: ExportColorSpace,
    profile: Option<&profile::Stage>,
) {
    let m = camera_to_space_matrix(xyz2cam, space);
    data.par_iter_mut().for_each(|px| {
        let v = [px[0] * wb[0], px[1] * wb[1], px[2] * wb[2]];
        // The camera profile acts HERE: linear light, before the transfer
        // encode. Everything downstream — exposure, tone, the mixer, the masks
        // — then sees a frame that already carries the profile, which is the
        // order Lightroom's own panel implies. A profile with a ForwardMatrix
        // takes the white-balanced CAMERA triple and owns the matrix too
        // (`profile::Stage::develop_camera`, the DNG SDK's colour spec); any
        // other profile acts on the working-space pixel this engine's own D65
        // matrix makes — and a render with no profile (no Adobe install, or
        // none named) keeps that matrix alone, as before v1.17.0.
        let mut t = match profile {
            Some(p) if p.owns_camera() => p.develop_camera(v),
            Some(p) => {
                let mut t = mat_vec3(&m, &v);
                p.apply(&mut t);
                t
            }
            None => mat_vec3(&m, &v),
        };
        let max = t[0].max(t[1]).max(t[2]);
        if max > 1.0 {
            // Blown pixels take EXACTLY rawler's treatment, including its
            // negative pre-clip: keeping negatives inside this formula let
            // the positive euclidean term drag an out-of-gamut component
            // back into gamut with a hue shift. Unblown pixels (the branch
            // NOT taken) keep their negatives — the wide-gamut win lives
            // there.
            let t0 = t.map(|c| c.max(0.0));
            let eucl = ((t0[0] * t0[0] + t0[1] * t0[1] + t0[2] * t0[2]) / 3.0).sqrt();
            t = t0.map(|c| (c / max + eucl) / 2.0);
        }
        *px = [linear_to_srgb(t[0]), linear_to_srgb(t[1]), linear_to_srgb(t[2])];
    });
}

/// The camera profile this photograph is developed through, prepared for this
/// frame — or `None`, with the reason disclosed, when there is not one.
///
/// The NAME comes from the sidecar: the Description's own `crs:CameraProfile`
/// when it has one, otherwise the creative Look's base profile, which is what
/// Lightroom falls back to for the fifteen files in the measured library that
/// carry a Look and no top-level profile. The FILE comes from the user's Adobe
/// install ([`crate::dcp`]) and is never bundled.
///
/// Every way this can come up empty is DISCLOSED by name, because "the profile
/// you named was not applied" is exactly the kind of silence that makes a
/// render disagree with Lightroom for no visible reason.
///
/// The second half of the answer is the white-balance multipliers of the
/// recipe's own white when the stage took it in (v1.17.0): a recipe on
/// Camera Raw's law that moves from As Shot names a white
/// ([`super::wb::camera_raw_white`]), and a stage that owns the camera
/// transform is built at that white's camera neutral, as Camera Raw builds
/// its colour spec — the camera triple is balanced to it and the matrices
/// and hue/sat maps are blended at its temperature. `None` there leaves the
/// frame at As Shot and the move to the post-profile adaptation.
pub(super) fn resolve_camera_profile(
    rawimage: &rawler::RawImage,
    r: &EditRecipe,
    space: ExportColorSpace,
    diag: &crate::diag::Diag<'_>,
    calibration: Option<[f32; 3]>,
) -> Option<(profile::Stage, Option<[f32; 3]>)> {
    let name = if !r.camera_profile.is_empty() {
        r.camera_profile.as_str()
    } else {
        r.look.as_ref().map(|l| l.base_profile.as_str()).unwrap_or("")
    };
    if name.is_empty() {
        return None;
    }
    // The creative colour table, read from the Camera Raw install and checked
    // against its fingerprint (`crate::dcp::find_look_table`). Every way it
    // can stay unrendered is named here, where the profile is being applied.
    let creative = r.look.as_ref().filter(|l| !l.table.is_empty()).and_then(|look| {
        let not_rendered = |why: String| {
            diag.warn(format!(
                "creative profile \"{}\": its colour table ({}) is not rendered ({why}) — the \
                 rest of the Look is",
                look.name, look.table
            ));
        };
        // Lightroom scales a Look by its Amount; every Look measured carries
        // 1, and how a table scales below that is not modelled.
        if (look.amount - 1.0).abs() > 1e-3 {
            not_rendered(format!("its amount is {:.2}, and only 1 is modelled", look.amount));
            return None;
        }
        crate::dcp::find_look_table(&look.table).map_err(|e| not_rendered(e.to_string())).ok()
    });
    let (make, model) = (rawimage.camera.make.as_str(), rawimage.camera.model.as_str());
    let (path, prof) = match crate::dcp::find(make, model, name) {
        Ok(found) => found,
        Err(e) => {
            diag.warn(format!("camera profile \"{name}\" is not rendered: {e}"));
            return None;
        }
    };
    if prof.has_third_illuminant {
        diag.warn(format!(
            "camera profile \"{name}\" states three calibration illuminants; rendering with two"
        ));
    }
    let matrix = match camera_matrix(rawimage) {
        Ok(m) => m,
        Err(e) => {
            diag.warn(format!("camera profile \"{name}\" is not rendered: {e}"));
            return None;
        }
    };
    let wb = normalise_wb(rawimage.wb_coeffs);
    let kelvin = wb_to_kelvin_tint(&matrix, wb).map(|(k, _)| k);
    // The camera neutral is the reciprocal of the white-balance multipliers;
    // with the profile's own matrices it fixes the white the SDK's way.
    let as_shot = wb.iter().all(|c| c.is_finite() && *c > 0.0).then(|| wb.map(|c| 1.0 / c));
    let chosen = super::wb::camera_raw_white(r).and_then(|xy| profile::camera_neutral_of(&prof, xy, calibration));
    let neutral = chosen.or(as_shot);
    // Adobe's baseline exposure for the body, which its renderer adds to the
    // profile's own offset. Disclosed either way: inferred, it is a number
    // this engine did not read from the photograph; unknown, it is why the
    // render may sit darker than Lightroom's.
    let baseline = crate::dcp::baseline_exposure(make, model);
    diag.warn(baseline_disclosure(make, model, &baseline));
    let frame = profile::Frame { kelvin, neutral, creative: creative.clone(), calibration };
    let stage = profile::Stage::build_for(&prof, space, frame, baseline.ev());
    if stage.is_none() {
        diag.warn(format!(
            "camera profile \"{name}\" ({}) carries no table or curve this engine renders",
            path.display()
        ));
    }
    let stage = stage?;
    // Only a stage that owns the camera transform balances the camera triple
    // itself; a profile with no `ForwardMatrix` is rebuilt at As Shot and the
    // move stays the post-profile adaptation's.
    if chosen.is_some() && !stage.owns_camera() {
        let frame = profile::Frame { kelvin, neutral: as_shot, creative, calibration };
        return profile::Stage::build_for(&prof, space, frame, baseline.ev()).map(|s| (s, None));
    }
    Some((stage, chosen.map(|n| n.map(|v| 1.0 / v))))
}

/// The one line a profiled render says about the body's baseline exposure.
pub(super) fn baseline_disclosure(make: &str, model: &str, b: &crate::dcp::Baseline) -> String {
    let body = if model.to_ascii_lowercase().starts_with(&make.to_ascii_lowercase()) {
        model.to_string()
    } else {
        format!("{make} {model}")
    };
    match b {
        crate::dcp::Baseline::Inferred { ev, .. } => format!(
            "Adobe's baseline exposure for {body}: {ev:+.2} EV (inferred from its camera profiles)"
        ),
        crate::dcp::Baseline::NoCameraProfiles | crate::dcp::Baseline::Disagree { .. } => format!(
            "Adobe's baseline exposure for {body} is unknown: rendering 0 EV — a \
             Lightroom-written DNG of this body carries it as BaselineExposure"
        ),
    }
}
