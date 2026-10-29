//! In-camera lens-correction metadata → engine-space [`LensProfile`].
//!
//! Sony bodies write the manufacturer's exact correction profile for the
//! mounted lens at the shot's focal length/aperture into every ARW as three
//! spline-knot arrays (`0x7032` vignetting, `0x7035` chromatic aberration,
//! `0x7037` distortion — the same 0x70xx block rawler models as `ExifTag`
//! variants). That makes a lensfun database unnecessary for Sony files: the
//! per-shot profile ships inside the file.
//!
//! Conversion formulas follow RawTherapee's shipped implementation
//! (`rtengine/lensmetadata.cc`), cross-checked against the independent
//! reverse-engineering write-up at <https://stannum.io/blog/0PwljB>:
//!
//! * vignette gain  `1 / 2^(0.5 − 2^(v·2⁻¹³ − 1))`  (v = 0 ⇒ exactly 1.0)
//! * distortion radius factor  `v·2⁻¹⁴ + 1`
//! * CA radius factor  `v·2⁻²¹ + 1` (16 red knots then 16 blue; multiplies
//!   the distortion/green map per channel)
//!
//! Vignette and CA retain the engine's established canonical knot convention.
//! The Sony distortion array is converted here, at the source boundary, twice:
//!
//! * for the RENDER, the way Adobe Camera Raw reads it
//!   ([`sony_render_distortion`]: knots at `i/(n−1)` of the SENSOR
//!   half-diagonal, a least-squares odd polynomial through the camera's
//!   radius map, evaluated at the zoomed output radius), resampled onto the
//!   engine's canonical `(i+0.5)/(n−1)` grid;
//! * for the Lightroom mask-transport solve, D2's native `(i+1)/16` reading,
//!   resampled onto the same canonical grid ([`resample_sony_distortion`]) —
//!   that solve was measured on its own and is left as measured.
//!
//! Each array's FIRST element is the value count; a malformed count degrades
//! that component to "absent", never an error (a photo without correction
//! data is a normal photo).
//!
//! # The MASK WARP, and why it is solved here (R29 Batch-3)
//!
//! [`LensProfile::mask_warp`] is the radial magnification between the frame
//! Lightroom STORES a mask's coordinates in and the frame it EXPORTS that mask
//! into (`render`'s mask-warp block header carries the measurements). It has
//! two possible sources and this function is where the choice is made, because
//! this is the one place that already holds the camera's own knots:
//!
//! * **Source A** — the in-camera `0x7037` spline just read above, inverted by
//!   [`crate::render::mask_warp_from_camera_knots`]. Preferred whenever it
//!   exists: it is the maker's calibration for THIS shot, needs no external
//!   file, and scored 2.30 px against Adobe's own 2.11 px on the 138-patch
//!   pixel field.
//! * **Source B** — an Adobe `.lcp` on this machine ([`crate::lcp`]), for
//!   bodies that write no knots at all.
//!
//! Every failure is TAGGED, never silent: [`crate::recipe::MaskWarpSource`]
//! names which source answered or which of the five refusals applies.
//!
//! **The warp depends on the frame's ASPECT only, not its pixel count.** Both
//! solves normalise radius by the half-diagonal and the `.lcp` reference length
//! is proportional to the width, so the frame's size cancels out of `m(r)`. The
//! dimensions read below are therefore a shape, and a preview-sized render and
//! a full-resolution export share one map — which is what lets it be solved
//! once, here, and stored.

use std::path::Path;

use rawler::formats::tiff::reader::TiffReader;
use rawler::formats::tiff::{GenericTiffReader, Value};
use rawler::tags::{DngTag, ExifTag};

use crate::recipe::{LensProfile, MASK_WARP_KNOTS, MaskWarpCenter, MaskWarpSource};

/// Dense enough that representing either measured 16-knot Sony distortion
/// spline on the engine's offset canonical grid has <1e-5 maximum factor
/// error, including the awkward r=1 endpoint between the last two canonical
/// nodes. The contract test records the measured maximum.
pub(crate) const SONY_DISTORTION_CANONICAL_KNOTS: usize = 2048;

/// Canonical knots the Sony RENDER distortion is resampled onto: the most
/// [`LensProfile::clamp`] keeps (it truncates every profile spline to 32, and
/// a longer one would lose its outer half). The target is a smooth even
/// polynomial, so linear interpolation on this grid departs from it by an
/// eighth of its curvature times the node spacing squared; the contract test
/// (`the_render_resample_is_faithful_to_the_camera_raw_law`) records the
/// measured maximum on the five fixture lenses.
pub(crate) const SONY_RENDER_DISTORTION_KNOTS: usize = 32;

/// What a sidecar adds to the lens lookup: the Adobe profile Lightroom named
/// and the focus distance it recorded (ApproximateFocusDistance). A RAW alone
/// states neither.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LensQuery {
    pub named: Option<NamedProfile>,
    pub focus_distance_m: Option<f32>,
}

/// A profile a sidecar names (`crate::lcp::open_named`). `custom` is
/// crs:LensProfileSetup="Custom": when the name resolves to nothing Camera Raw
/// then draws no profile at all, where under LensDefaults or Auto it falls
/// back to the profile it would choose by itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NamedProfile {
    pub name: String,
    pub filename: Option<String>,
    pub custom: bool,
}

/// Read the in-camera lens correction profile from a RAW file. Every failure
/// path returns an EMPTY component (profile with nothing to apply) — absent
/// metadata is the common case for non-Sony files, not an error.
pub fn read(path: &Path) -> LensProfile {
    read_for(path, &LensQuery::default())
}

/// [`read`] with what a sidecar adds ([`LensQuery`]).
///
/// The VIGNETTING is the Adobe profile's whenever this machine has one for the
/// lens (`lcp::LensProfileFile::vignette_gains`), and the camera's own knots
/// only when it has none. Camera Raw never renders the camera's vignette knots
/// (tag 0x7032 and its SR2 copy rewritten to constants render bit-identically
/// under every lens setup, with the profile on or off, and DNG Converter writes
/// no vignette opcode); with the profile on, its gain is the profile's
/// VignetteModel. The camera's knots stay as the fallback because a machine
/// with no Adobe profile has no Camera Raw reading to follow, and they are the
/// maker's own correction for the shot.
pub fn read_for(path: &Path, query: &LensQuery) -> LensProfile {
    let mut out = LensProfile::default();
    if !crate::decode::is_raw(path) {
        return out;
    }
    let Ok(file) = std::fs::File::open(path) else { return out };
    let mut reader = std::io::BufReader::new(file);
    // `Some(16)`, not `None`: with no chain cap, rawler 0.7.2's walker
    // (formats/tiff/reader.rs:164-179) keeps no visited-offset set and its
    // only `break` sits inside `if let Some(max)` — a self-referential
    // next_ifd loops forever, pushing an IFD per iteration. 16 is rawler's
    // own test value and above its internal `new_root` cap of 10; real RAWs
    // carry ≤4 top-level IFDs, and this reader only consults `root_ifd()`,
    // so a truncated chain tail is unobservable here.
    let Ok(tiff) = GenericTiffReader::new(&mut reader, 0, 0, Some(16), &[]) else {
        return out;
    };
    let root = tiff.root_ifd();
    let knots = |tag: ExifTag| -> Vec<i16> {
        let Some(entry) = root.get_entry_recursive(tag) else { return Vec::new() };
        let vals: Vec<i16> = match &entry.value {
            Value::SShort(v) => v.clone(),
            Value::Short(v) => v.iter().map(|&x| x as i16).collect(),
            _ => return Vec::new(),
        };
        // First element = value count; guard the shape instead of trusting it.
        match vals.first().map(|&n| n as usize) {
            Some(n) if (2..=64).contains(&n) && vals.len() > n => vals[1..=n].to_vec(),
            _ => Vec::new(),
        }
    };

    out.vignette = knots(ExifTag::VignettingCorrParams)
        .iter()
        .map(|&v| vignette_gain(v))
        .collect();
    let sony_distortion: Vec<f32> = knots(ExifTag::DistortionCorrParams)
        .iter()
        .map(|&v| v as f32 * (-14f32).exp2() + 1.0)
        .collect();
    // The render spline is Camera Raw's reading of these knots, which needs
    // the frame (`DefaultCropSize`) and the sensor array it was cut from. A
    // file that states no frame keeps the knots as they are — the legacy
    // reading, for a file that is malformed anyway.
    out.distortion = match frame_dims(root) {
        Some(crop) if !sony_distortion.is_empty() => sony_render_distortion(
            &sony_distortion,
            crop,
            sensor_dims(root),
            SONY_RENDER_DISTORTION_KNOTS,
        ),
        _ => sony_distortion.clone(),
    };
    let lr_mask_distortion =
        resample_sony_distortion(&sony_distortion, SONY_DISTORTION_CANONICAL_KNOTS);
    let ca = knots(ExifTag::ChromaticAberrationCorrParams);
    // The pair split needs an even count AND enough knots per channel for a
    // meaningful radial spline (real bodies write 16+16; a malformed 2-value
    // array would otherwise become two 1-knot "constants" instead of
    // degrading to absent, per the malformed-metadata contract).
    if ca.len() % 2 == 0 && ca.len() >= 8 {
        let half = ca.len() / 2;
        let f = |v: &i16| *v as f32 * (-21f32).exp2() + 1.0;
        let (r, b): (Vec<f32>, Vec<f32>) =
            (ca[..half].iter().map(f).collect(), ca[half..].iter().map(f).collect());
        // Camera Raw's reading of the pair (see `sony_render_ca`) needs the
        // frame, like the distortion's; no frame keeps the knots as they are.
        match frame_dims(root) {
            Some(crop) => {
                let sensor = sensor_dims(root);
                let n = SONY_RENDER_DISTORTION_KNOTS;
                out.ca_r = sony_render_ca(&r, &sony_distortion, crop, sensor, n);
                out.ca_b = sony_render_ca(&b, &sony_distortion, crop, sensor, n);
            }
            None => (out.ca_r, out.ca_b) = (r, b),
        }
    }

    // --- the mask warp (R29 Batch-3) -------------------------------------
    let text = |tag: ExifTag| -> String {
        root.get_entry_recursive(tag)
            .and_then(|e| e.value.as_string().cloned())
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let number = |tag: ExifTag| -> Option<f32> {
        root.get_entry_recursive(tag)?.value.get_f32(0).ok().flatten().filter(|v| v.is_finite())
    };
    // `DefaultCropSize` FIRST, and for the reason `xmp`'s frame doc spells out
    // at length: it is the frame every normalised coordinate in a sidecar is
    // measured against, where `ImageWidth/Length` is the raw sensor array
    // including the masked border. Only the ASPECT of this reaches the answer,
    // but the two aspects differ by ~0.3 % on a real ARW and the fallback is
    // the second-best shape, not an equal one.
    let dims = frame_dims(root);
    out.mask_warp_center = mask_warp_center(root, dims);
    // The Adobe profile for this lens, looked up once: source B of the mask
    // warp for a body with no knots, and the vignetting for every body.
    let lens = {
        let m = text(ExifTag::LensModel);
        if m.is_empty() { text(ExifTag::LensMake) } else { m }
    };
    let auto = || crate::lcp::open(None, &text(ExifTag::Make), &lens);
    let adobe = match &query.named {
        None => auto(),
        Some(n) => match crate::lcp::open_named(&n.name, n.filename.as_deref()) {
            Err(_) if !n.custom => auto(),
            found => found,
        },
    };
    out.adobe = adobe.as_ref().ok().map(|f| crate::recipe::AdobeLensProfile {
        name: f.profile_name.clone(),
        filename: f.filename.clone(),
    });
    match dims {
        Some(dims) if !lr_mask_distortion.is_empty() => {
            let w = crate::render::mask_warp_from_camera_knots(
                &lr_mask_distortion,
                dims,
                MASK_WARP_KNOTS,
            );
            if w.len() >= 2 {
                out.mask_warp = w;
                out.mask_warp_src = MaskWarpSource::CameraMetadata;
            } else {
                // The inversion is only unsolvable for a spline that folds
                // inside the frame — a corrupt array, not a real lens.
                out.mask_warp_src = MaskWarpSource::Unparseable;
            }
        }
        Some(dims) => {
            // No in-camera knots: this is exactly the body source B exists for.
            let focal = number(ExifTag::FocalLength);
            match adobe.as_ref().map_err(|r| *r).and_then(|f| crate::lcp::solve_in(f, focal, dims, MASK_WARP_KNOTS)) {
                Ok(maps) => {
                    out.mask_warp = maps.mask_warp;
                    out.mask_warp_src = MaskWarpSource::Lcp;
                    // v1.5.0: the same node also RENDERS. Only into a component
                    // the camera left empty — an in-camera spline is this
                    // photo's own measurement and outranks a lens-model average
                    // from a profile database, so this fills a hole and never
                    // overwrites. A body with no correction metadata at all
                    // used to render with no lens correction while Lightroom
                    // rendered one; that was the gap, and it closes here rather
                    // than at a render surface, because every surface already
                    // reads these two fields.
                    //
                    // The RENDER spline is solved at the clamp's 32 knots, not
                    // the mask warp's 64: `LensProfile::clamp` keeps 32, and a
                    // 64-knot spline lost its outer half there — the map then
                    // held its mid-frame value out to the corner.
                    let render = adobe
                        .as_ref()
                        .ok()
                        .and_then(|f| f.model_at(focal).ok())
                        .and_then(|m| m.distortion_knots(dims, SONY_RENDER_DISTORTION_KNOTS));
                    if out.distortion.is_empty()
                        && let Some(d) = render
                    {
                        out.distortion = d;
                    }
                }
                Err(r) => out.mask_warp_src = r.into(),
            }
        }
        // A file that declares no frame declares no aspect, and the map is a
        // function of the aspect. Refusing beats guessing 3:2.
        None => out.mask_warp_src = MaskWarpSource::Absent,
    }

    // --- the vignetting (see `read_for`) ----------------------------------
    // Gains over the default crop, in the frame before the lens geometry —
    // the frame `render::develop` applies them in — normalised by the SENSOR
    // width, which is what Camera Raw's reading of the profile measures.
    let shot = crate::lcp::Shot {
        focal_mm: number(ExifTag::FocalLength),
        aperture_av: number(ExifTag::FNumber).and_then(crate::lcp::Shot::av_of_f_number),
        focus_distance_m: query.focus_distance_m,
    };
    if let (Ok(file), Some(crop)) = (&adobe, dims) {
        let sensor_width = sensor_dims(root).map_or(crop.0, |s| s.0);
        if let Some(g) = file.vignette_gains(shot, crop, sensor_width, SONY_RENDER_DISTORTION_KNOTS) {
            out.vignette = g;
        }
    }

    out.clamp(); // same defensive ranges as a hand-edited recipe
    out
}

/// The frame this photo's normalised coordinates are measured against:
/// `DefaultCropSize` when the file states one, else the raw array.
///
/// Split out so the precedence is one statement rather than a nested `match`
/// inside `read` — and so a test can state it.
fn frame_dims(root: &rawler::formats::tiff::IFD) -> Option<(f32, f32)> {
    let pair = |a: Option<f32>, b: Option<f32>| -> Option<(f32, f32)> {
        let (w, h) = (a?, b?);
        (w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0).then_some((w, h))
    };
    let dng = root.get_entry_recursive(DngTag::DefaultCropSize).map(|e| &e.value);
    if let Some(v) = dng
        && let Some(d) = pair(v.get_f32(0).ok().flatten(), v.get_f32(1).ok().flatten())
    {
        return Some(d);
    }
    pair(
        root.get_entry_recursive(ExifTag::ImageWidth)?.value.get_f32(0).ok().flatten(),
        root.get_entry_recursive(ExifTag::ImageHeight)?.value.get_f32(0).ok().flatten(),
    )
}

/// The raw sensor array the default crop was cut from (`ImageWidth/Length`),
/// the frame the camera's own lens maps are stated in.
fn sensor_dims(root: &rawler::formats::tiff::IFD) -> Option<(f32, f32)> {
    let w = root.get_entry_recursive(ExifTag::ImageWidth)?.value.get_f32(0).ok().flatten()?;
    let h = root.get_entry_recursive(ExifTag::ImageHeight)?.value.get_f32(0).ok().flatten()?;
    (w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0).then_some((w, h))
}

/// Sony `0x7037` distortion knots → the engine's render spline, read the way
/// Adobe Camera Raw reads them (ledger item 13, 2026-10-28).
///
/// The law, from causal probes through Adobe DNG Converter 18.7 (which writes
/// Camera Raw's own reading of the knots into the file as the LensDistortInfo
/// polynomial) and through Camera Raw renders — reproduced, not fitted:
///
/// * Camera Raw reads the copy of the knots in the encrypted SR2 private
///   block (tag 0x7982, the same sixteen values as `0x7037`); knot `i` sits at
///   `t_i = i/(n−1)`, `t` in units of the SENSOR half-diagonal;
/// * the camera's map is `source = t·K(t)`, and Camera Raw replaces it by the
///   least-squares odd polynomial `t·Q(t)`, `Q = c₁ + c₃t² + c₅t⁴ + c₇t⁶`,
///   through the knots `i ≥ 1` (unweighted; the centre knot's row is zero);
/// * the output is zoomed by `Z` so that no edge pixel samples outside the
///   frame: output radius `u`, `source = Z·u·Q(Z·u)`, with the maximum of
///   `Z·Q(Z·u)` over the frame edge equal to 1 — a constant map therefore
///   renders as the identity, and a barrel map pins the short edge's midpoint
///   where a pincushion one pins the corner.
///
/// Measured: the DNG's polynomial `(Z·c₁, Z³·c₃, Z⁵·c₅, Z⁷·c₇)` for the
/// sensor frame matches this within 2.1e-6 per coefficient on five real
/// frames and 1e-5 on fourteen synthetic knot sets (6e-5 on a single-knot
/// step); its constant term sits 0.5–4.5e-5 below (2.1e-5 above on the
/// step), as if Adobe's zoom stopped short of the exact root by up to
/// 1.5·2⁻¹⁵ — 0.005 % of radius, not modelled. A Camera Raw render of a single-knot step shows the polynomial's
/// ringing, not the knots' step, so the polynomial is what renders.
///
/// The old reading evaluated the knots themselves at the UNZOOMED output
/// radius, `K(u)/max K`: for a map rising to `K = 1.054` at the corner that
/// indexes every knot about 5 % too far out — the 0.06–0.26 % magnification
/// the five-frame registration measured on the FE 24–105 frames.
///
/// The engine's render spline (`render::profile_knot_interp`: canonical
/// `(j+0.5)/(N−1)` in DEFAULT-CROP half-diagonals, divided by its own edge
/// maximum) draws that map when its knots are `k(r) = Q(Z·ρ·r)` with
/// `ρ = |crop| / |sensor|` and `Z` solved against the crop's edge — the
/// division by the edge maximum then IS the zoom. What it cannot carry is the
/// centre: Camera Raw's map is centred on the sensor, which on the ILCE-7RM4A
/// sits 16 px right of the default crop's centre; that offset moves pixels by
/// under a pixel and no radius, and is left to the frame-centre work.
pub(crate) fn sony_render_distortion(
    native: &[f32],
    crop: (f32, f32),
    sensor: Option<(f32, f32)>,
    n: usize,
) -> Vec<f32> {
    if n < 2 {
        return Vec::new();
    }
    let Some(q) = sony_radius_polynomial(native) else {
        // Too few knots to fit four terms: keep them as they are.
        return native.to_vec();
    };
    let (rho, rmin) = crop_in_sensor(crop, sensor);
    let z = sony_render_zoom(&q, rho, rmin);
    (0..n)
        .map(|j| {
            let r = (j as f64 + 0.5) / (n - 1) as f64;
            even_poly(&q, z * rho * r) as f32
        })
        .collect()
}

/// `(ρ, rmin)` for a default crop cut from `sensor`: `ρ = |crop| / |sensor|`,
/// the crop's half-diagonal in sensor half-diagonals, and `rmin` the crop's
/// short-edge midpoint in crop half-diagonals. A sensor smaller than the crop
/// is not a sensor; the crop is read as the whole array then (`ρ = 1`).
fn crop_in_sensor(crop: (f32, f32), sensor: Option<(f32, f32)>) -> (f64, f64) {
    let diag = |(w, h): (f32, f32)| f64::from((w * w + h * h).sqrt());
    let rho = sensor
        .map(|s| diag(crop) / diag(s).max(1e-6))
        .filter(|r| r.is_finite() && *r > 0.0 && *r <= 1.0)
        .unwrap_or(1.0);
    let rmin = (f64::from(crop.0.min(crop.1)) / diag(crop).max(1e-6)).clamp(0.0, 1.0);
    (rho, rmin)
}

/// One channel of the Sony `0x7035` chromatic-aberration pair (red's sixteen
/// knots, or blue's) → the engine's CA spline, read the way Adobe Camera Raw
/// reads it (2026-10-28).
///
/// The law, from Adobe DNG Converter 18.7, which writes Camera Raw's reading
/// of the pair into the file as a WarpRectilinear opcode (red and blue planes;
/// green the identity) centred on the sensor — reproduced, not fitted:
///
/// * the knots sit where the distortion knots sit, `t_i = i/(n−1)` of the
///   SENSOR half-diagonal, and Camera Raw replaces them by the same
///   least-squares odd polynomial `t·P(t)` through the knots `i ≥ 1` that
///   [`sony_render_distortion`] fits — no zoom of its own;
/// * `t` is the radius of the GREEN sample in the raw frame, where the
///   opcode runs before the distortion map: the red sample for an output
///   pixel whose green sample lies at `t` lies at `t·P(t)`.
///
/// Measured: the polynomial matches the DNG's planes within 7.6e-6 of a
/// factor over the whole radius on ten channels of five real frames; the old
/// reading (knots at `(i+0.5)/(n−1)` of the output radius) missed them by
/// 2.9e-5 to 1.1e-4 — up to 0.1 px at a 61 MP corner, the colour fringe a
/// registration of Camera Raw's render against the engine's measured.
///
/// The engine multiplies its green map by this spline at the OUTPUT radius
/// (`render::apply_lens_geometry`), so knot `j` is `P` at the green source
/// radius of canonical output radius `r_j`: `t_j = Z·ρ·r_j·Q(Z·ρ·r_j)` under
/// Camera Raw's distortion map ([`sony_render_distortion`]'s `Q` and `Z`),
/// `ρ·r_j` for a frame with no distortion knots. That bakes the distortion
/// into the CA spline: with the distortion toggled off the spline is read a
/// few per cent of radius off, which moves `P` by under 1e-5 on these lenses.
pub(crate) fn sony_render_ca(
    native: &[f32],
    distortion: &[f32],
    crop: (f32, f32),
    sensor: Option<(f32, f32)>,
    n: usize,
) -> Vec<f32> {
    if n < 2 {
        return Vec::new();
    }
    let Some(p) = sony_radius_polynomial(native) else {
        return native.to_vec();
    };
    let (rho, rmin) = crop_in_sensor(crop, sensor);
    let green = sony_radius_polynomial(distortion).map(|q| (sony_render_zoom(&q, rho, rmin), q));
    (0..n)
        .map(|j| {
            let u = rho * (j as f64 + 0.5) / (n - 1) as f64;
            let t = match &green {
                Some((z, q)) => z * u * even_poly(q, z * u),
                None => u,
            };
            even_poly(&p, t) as f32
        })
        .collect()
}

/// `Q`'s coefficients `(c₁, c₃, c₅, c₇)` of [`sony_render_distortion`]: the
/// least-squares fit of `t·K_i` by `c₁t + c₃t³ + c₅t⁵ + c₇t⁷` at the knots
/// `t_i = i/(n−1)`, `i ≥ 1`, solved by the normal equations in f64. `None`
/// for fewer than five knots (four terms need four points beyond the centre)
/// or a singular system.
fn sony_radius_polynomial(knots: &[f32]) -> Option<[f64; 4]> {
    let n = knots.len();
    if n < 5 {
        return None;
    }
    let mut a = [[0f64; 5]; 4]; // the normal equations, augmented
    for (i, k) in knots.iter().enumerate().skip(1) {
        let t = i as f64 / (n - 1) as f64;
        let basis = [t, t.powi(3), t.powi(5), t.powi(7)];
        let y = t * f64::from(*k);
        for r in 0..4 {
            for c in 0..4 {
                a[r][c] += basis[r] * basis[c];
            }
            a[r][4] += basis[r] * y;
        }
    }
    // Gauss–Jordan with partial pivoting: a 4×4 system, symmetric positive
    // definite whenever the points are distinct.
    for col in 0..4 {
        let piv = (col..4).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        a.swap(col, piv);
        if a[col][col].abs() < 1e-15 {
            return None;
        }
        let pivot = a[col];
        for (r, row) in a.iter_mut().enumerate() {
            if r != col {
                let f = row[col] / pivot[col];
                for (x, p) in row.iter_mut().zip(pivot.iter()).skip(col) {
                    *x -= f * p;
                }
            }
        }
    }
    let q = [a[0][4] / a[0][0], a[1][4] / a[1][1], a[2][4] / a[2][2], a[3][4] / a[3][3]];
    q.iter().all(|v| v.is_finite()).then_some(q)
}

/// `Q(x) = c₁ + c₃x² + c₅x⁴ + c₇x⁶`.
fn even_poly(q: &[f64; 4], x: f64) -> f64 {
    let x2 = x * x;
    q[0] + x2 * (q[1] + x2 * (q[2] + x2 * q[3]))
}

/// The zoom `Z` of [`sony_render_distortion`]: the root of
/// `Z · max Q(Z·ρ·r) = 1` over the edge radii `r ∈ [rmin, 1]`, the maximum
/// taken on a 1024-step sweep (`Q` is a smooth sextic; the sweep's error is
/// below 1e-9 of a factor). The left side rises with `Z` for every map whose
/// `t·Q(t)` is itself monotone, so bisection on a wide bracket finds it; a
/// map with no root in the bracket falls back to `Z = 1 / Q(ρ)`, the zoom
/// that pins the corner.
fn sony_render_zoom(q: &[f64; 4], rho: f64, rmin: f64) -> f64 {
    const SWEEP: usize = 1024;
    let edge_max = |z: f64| -> f64 {
        (0..=SWEEP)
            .map(|i| rmin + (1.0 - rmin) * i as f64 / SWEEP as f64)
            .map(|r| even_poly(q, z * rho * r))
            .fold(f64::MIN, f64::max)
    };
    let h = |z: f64| z * edge_max(z) - 1.0;
    let (mut a, mut b) = (0.25f64, 4.0f64);
    if h(a) > 0.0 || h(b) < 0.0 {
        return 1.0 / even_poly(q, rho).max(1e-3);
    }
    for _ in 0..80 {
        let m = 0.5 * (a + b);
        if h(m) < 0.0 {
            a = m;
        } else {
            b = m;
        }
    }
    0.5 * (a + b)
}

/// Resample the Sony distortion spline from its native `(i+1)/n` radii onto
/// the canonical grid consumed by `render::profile_knot_interp`.
pub(crate) fn resample_sony_distortion(native: &[f32], n: usize) -> Vec<f32> {
    if native.is_empty() || n < 2 {
        return Vec::new();
    }
    (0..n)
        .map(|i| {
            let r = (i as f32 + 0.5) / (n - 1) as f32;
            sony_distortion_interp(native, r)
        })
        .collect()
}

/// Sony `0x7037` piecewise-linear interpolation at native radius `(i+1)/n`,
/// clamped before the first and after the last sample.
fn sony_distortion_interp(knots: &[f32], r: f32) -> f32 {
    let n = knots.len();
    if n == 0 {
        return 1.0;
    }
    if n == 1 {
        return knots[0];
    }
    let t = r * n as f32 - 1.0;
    if t <= 0.0 {
        return knots[0];
    }
    if t >= (n - 1) as f32 {
        return knots[n - 1];
    }
    let i = t.floor() as usize;
    let f = t - i as f32;
    knots[i] * (1.0 - f) + knots[i + 1] * f
}

/// Full-raw-frame centre expressed in the stored/default-crop frame.
/// Requires both first-party facts; missing either preserves legacy centre
/// behaviour through `LensProfile::mask_warp_center = None`.
fn mask_warp_center(
    root: &rawler::formats::tiff::IFD,
    stored_dims: Option<(f32, f32)>,
) -> Option<MaskWarpCenter> {
    let pair = |value: &Value| -> Option<(f32, f32)> {
        let x = value.get_f32(0).ok().flatten()?;
        let y = value.get_f32(1).ok().flatten()?;
        (x.is_finite() && y.is_finite()).then_some((x, y))
    };
    let number = |tag: ExifTag| -> Option<f32> {
        root.get_entry_recursive(tag)?.value.get_f32(0).ok().flatten().filter(|v| v.is_finite())
    };
    let full = (number(ExifTag::ImageWidth)?, number(ExifTag::ImageHeight)?);
    let origin = pair(&root.get_entry_recursive(DngTag::DefaultCropOrigin)?.value)?;
    let centre = [full.0 * 0.5 - origin.0, full.1 * 0.5 - origin.1];
    let (stored_w, stored_h) = stored_dims?;
    (centre[0] >= 0.0 && centre[1] >= 0.0).then_some(MaskWarpCenter {
        stored_px: centre,
        stored_dims: [stored_w, stored_h],
    })
}

/// Sony vignetting knot → linear-light gain (RawTherapee's formula).
fn vignette_gain(v: i16) -> f32 {
    1.0 / (0.5 - (v as f32 * (-13f32).exp2() - 1.0).exp2()).exp2()
}

#[cfg(test)]
mod tests {
    // These are decimal transcriptions of decoded integer knots and solved D2
    // fixture outputs; keep the report's digits rather than rewriting them to
    // clippy's display-oriented f32 spelling.
    #![allow(clippy::excessive_precision)]

    use super::*;

    const WALL_DISTORTION: [f32; 16] = [
        1.0007934570,
        0.9998779297,
        0.9981079102,
        0.9959716797,
        0.9927368164,
        0.9890136719,
        0.9846191406,
        0.9800415039,
        0.9749145508,
        0.9696044922,
        0.9641113281,
        0.9589843750,
        0.9538574219,
        0.9492187500,
        0.9448242188,
        0.9412231445,
    ];
    const DSC_DISTORTION: [f32; 16] = [
        1.0007934570,
        0.9998779297,
        0.9982299805,
        0.9961547852,
        0.9932250977,
        0.9898071289,
        0.9856567383,
        0.9813842773,
        0.9766235352,
        0.9719238281,
        0.9668579102,
        0.9622802734,
        0.9576416016,
        0.9538574219,
        0.9503173828,
        0.9478149414,
    ];

    #[test]
    fn sony_distortion_native_domain_resamples_to_the_d2_mask_warp_fixtures() {
        const RADII: [f32; 6] = [0.0, 0.2, 1.0 / 3.0, 0.5, 0.75, 1.0];
        const EXPECTED: [[f32; 6]; 2] = [
            [
                0.97478093,
                0.97767276,
                0.98359925,
                0.99524465,
                1.01848750,
                1.03647513,
            ],
            [
                0.97644290,
                0.97922226,
                0.98473749,
                0.99560184,
                1.01649349,
                1.03102158,
            ],
        ];
        for (name, native, expected) in [
            ("wall", &WALL_DISTORTION[..], EXPECTED[0]),
            ("P26", &DSC_DISTORTION[..], EXPECTED[1]),
        ] {
            let dense = resample_sony_distortion(native, SONY_DISTORTION_CANONICAL_KNOTS);
            assert_eq!(dense.len(), SONY_DISTORTION_CANONICAL_KNOTS);

            let mut worst = 0.0f32;
            for i in 0..=65_536 {
                let r = i as f32 / 65_536.0;
                worst = worst.max(
                    (crate::render::profile_knot_interp(&dense, r)
                        - sony_distortion_interp(native, r))
                    .abs(),
                );
            }
            assert!(worst < 1e-5, "{name}: canonical resampling error {worst}");

            let warp = crate::render::mask_warp_from_camera_knots(
                &dense,
                (9504.0, 6336.0),
                MASK_WARP_KNOTS,
            );
            assert_eq!(warp.len(), MASK_WARP_KNOTS);
            for (r, want) in RADII.into_iter().zip(expected) {
                let got = crate::render::mask_warp_factor(&warp, r);
                assert!((got - want).abs() < 2e-5, "{name} r={r}: {got} vs {want}");
            }
        }
    }

    #[test]
    fn conversion_matches_the_real_a7riv_vectors() {
        // Ground truth: P26.ARW (A7RIV + FE 24-105mm F4 G OSS @ 24mm),
        // dumped with rawler on this machine. Centre knots must be exact
        // identity; corner knots match independently computed values.
        assert!((vignette_gain(0) - 1.0).abs() < 1e-6, "centre vignette gain is exactly 1");
        assert!(
            (vignette_gain(8320) - 1.4249).abs() < 1e-3,
            "corner gain ≈ +0.51 EV, got {}",
            vignette_gain(8320)
        );
        let dist = |v: i16| v as f32 * (-14f32).exp2() + 1.0;
        assert!((dist(13) - 1.000793).abs() < 1e-5);
        assert!((dist(-855) - 0.947815).abs() < 1e-5, "barrel corner pulls source inward");
        let ca = |v: i16| v as f32 * (-21f32).exp2() + 1.0;
        assert!((ca(1024) - 1.000488).abs() < 1e-6);
        assert!((ca(-256) - 0.999878).abs() < 1e-6);
    }

    /// Camera Raw's reading of Sony distortion knots, pinned against Adobe's
    /// own: each case is a knot array (raw `0x7037` integers) and the
    /// LensDistortInfo numerators (over 2^30) that Adobe DNG Converter 18.7
    /// wrote for it. Five are real frames (ILCE-7RM4A; FE 24–105 F4 G OSS at
    /// 45, 105, 80 and 73 mm; Sigma 14–24 F2.8 DG DN at 20 mm); three are
    /// synthetic knot sets written into a copy of a raw (`K = 1 ± 0.04 t²`,
    /// `1 + 0.04 t⁴`), the minus one being the barrel case whose zoom pins the
    /// short edge's midpoint rather than the corner.
    const CAMERA_RAW_CASES: [(&str, [i16; 16], [i64; 4]); 8] = [
        ("24-105 @45", [2, 2, 5, 13, 23, 38, 56, 79, 106, 140, 181, 230, 288, 356, 437, 526], [1042950219, 17540105, 14758258, -1524500]),
        ("24-105 @105", [2, 4, 13, 29, 50, 79, 115, 158, 207, 264, 327, 397, 474, 559, 652, 753], [1030164927, 42426635, 510247, 620176]),
        ("24-105 @80", [0, 4, 14, 32, 58, 90, 129, 178, 235, 300, 374, 456, 550, 652, 765, 889], [1023657049, 45339227, 5216195, -481157]),
        ("24-105 @73", [2, 4, 13, 29, 54, 86, 126, 173, 228, 291, 361, 442, 534, 636, 748, 871], [1024629197, 43679741, 5576561, -160897]),
        ("14-24 @20", [-8, -6, -4, 0, 5, 13, 22, 31, 40, 48, 57, 66, 76, 92, 112, 137], [1064525330, 13991147, -15117750, 10320273]),
        ("1+0.04t^2", [0, 3, 12, 26, 47, 73, 105, 143, 186, 236, 291, 352, 419, 492, 571, 655], [1035283098, 38195685, 453016, -211779]),
        ("1+0.04t^4", [0, 0, 0, 1, 3, 8, 17, 31, 53, 85, 129, 190, 268, 370, 497, 655], [1037564924, -35260, 36306855, -101121]),
        ("1-0.04t^2", [0, -3, -12, -26, -47, -73, -105, -143, -186, -236, -291, -352, -419, -492, -571, -655], [1087307776, -44257680, -579079, 298645]),
    ];

    fn knot_factors(raw: &[i16; 16]) -> Vec<f32> {
        raw.iter().map(|&v| v as f32 * (-14f32).exp2() + 1.0).collect()
    }

    /// The engine's own radial factor `r_src / r_dst` for a canonical spline
    /// in a frame of `dims`: the knots over their edge maximum, exactly as
    /// `render::profile_fill_scale` divides them (dense sweep of the edge).
    fn engine_factor(dense: &[f32], dims: (f32, f32), r: f32) -> f32 {
        let rmin = dims.0.min(dims.1) / (dims.0 * dims.0 + dims.1 * dims.1).sqrt();
        let edge = (0..=4096)
            .map(|i| crate::render::profile_knot_interp(dense, rmin + (1.0 - rmin) * i as f32 / 4096.0))
            .fold(f32::MIN, f32::max);
        crate::render::profile_knot_interp(dense, r) / edge
    }

    #[test]
    fn the_render_distortion_is_camera_raws_polynomial_per_case() {
        // In the SENSOR frame (crop = sensor) the engine's map must be the
        // DNG's polynomial itself. Measured worst 4.6e-5: the constant-term
        // margin the doc of `sony_render_distortion` reports plus the 32-node
        // resample; the old reading
        // missed by 1.9e-3 on the 80 mm frame.
        const SENSOR: (f32, f32) = (9600.0, 6376.0);
        let mut worst = 0.0f32;
        for (name, raw, num) in CAMERA_RAW_CASES {
            let dense =
                sony_render_distortion(&knot_factors(&raw), SENSOR, Some(SENSOR), SONY_RENDER_DISTORTION_KNOTS);
            assert_eq!(dense.len(), SONY_RENDER_DISTORTION_KNOTS);
            let k: Vec<f64> = num.iter().map(|&n| n as f64 / (1u64 << 30) as f64).collect();
            for r in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
                let x = f64::from(r);
                let adobe = (k[0] + k[1] * x.powi(2) + k[2] * x.powi(4) + k[3] * x.powi(6)) as f32;
                let got = engine_factor(&dense, SENSOR, r);
                worst = worst.max((got - adobe).abs());
                assert!((got - adobe).abs() < 6e-5, "{name} r={r}: engine {got} vs Camera Raw {adobe}");
            }
        }
        eprintln!("worst |engine − Camera Raw| factor: {worst:e}");
    }

    #[test]
    fn the_render_resample_is_faithful_to_the_camera_raw_law() {
        // The canonical grid against the polynomial it samples, in the
        // ILCE-7RM4A's own crop/sensor pair. Measured worst 5.3e-5 (0.005 % of
        // radius), on 32 nodes.
        let (crop, sensor) = ((9504.0, 6336.0), (9600.0, 6376.0));
        let rho = (9504f64.hypot(6336.0)) / 9600f64.hypot(6376.0);
        let rmin = 6336.0 / 9504f64.hypot(6336.0);
        let mut worst = 0.0f64;
        for (name, raw, _) in CAMERA_RAW_CASES {
            let knots = knot_factors(&raw);
            let dense = sony_render_distortion(&knots, crop, Some(sensor), SONY_RENDER_DISTORTION_KNOTS);
            let q = sony_radius_polynomial(&knots).expect("sixteen knots fit");
            let z = sony_render_zoom(&q, rho, rmin);
            for i in 0..=8192 {
                let r = i as f64 / 8192.0;
                let d = (f64::from(crate::render::profile_knot_interp(&dense, r as f32)) - even_poly(&q, z * rho * r)).abs();
                worst = worst.max(d);
                assert!(d < 6e-5, "{name} r={r}: resample error {d}");
            }
        }
        eprintln!("worst canonical resample error: {worst:e}");
        // The spline must survive the profile's own defensive clamp whole.
        let dense = sony_render_distortion(&knot_factors(&CAMERA_RAW_CASES[2].1), crop, Some(sensor), SONY_RENDER_DISTORTION_KNOTS);
        let mut p = LensProfile { distortion: dense.clone(), ..LensProfile::default() };
        p.clamp();
        assert_eq!(p.distortion, dense, "clamp must keep the render spline whole");
    }

    /// The `0x7035` pair (red sixteen, then blue) of the five real frames of
    /// [`CAMERA_RAW_CASES`] (same order), and the WarpRectilinear planes
    /// `(k₀, k₁, k₂, k₃)` Adobe DNG Converter 18.7 wrote for red and blue.
    #[allow(clippy::type_complexity)] // because a fixture table reads best inline
    const CAMERA_RAW_CA: [([i16; 32], [f64; 4], [f64; 4]); 5] = [
        (
            [512, 512, 512, 512, 512, 384, 384, 384, 384, 256, 256, 256, 256, 256, 128, 256, 128, 128, 128, 128, 128, 128, 128, 256, 256, 256, 256, 256, 256, 256, 128, 128],
            [1.0002373637274475, -0.0002489610220068883, -1.3931351906748546e-05, 0.00013816649935449732],
            [1.0000247510471891, 0.0005173276850358732, -0.0007280815185470857, 0.00024268582135178463],
        ),
        (
            [-512, -384, -384, -384, -384, -384, -512, -512, -512, -512, -640, -640, -640, -640, -640, -512, -256, -256, -256, -256, -256, -256, -256, -256, -256, -256, -128, -128, -128, -128, -128, -128],
            [0.9998143476247164, -0.000186766565001939, -0.00031572447760730427, 0.00043448191939354786],
            [0.9998570113717264, 7.52299668462797e-05, 0.0002065587422119819, -0.00020504145854793876],
        ),
        (
            [0, 0, 0, 0, 0, -128, -128, -128, -256, -256, -384, -256, -256, -256, -128, 0, 0, 0, -128, -128, -256, -256, -256, -256, -256, -256, -256, -256, -256, -256, -384, -512],
            [1.000033724665486, -0.0007488541342031141, 0.0008181476131809973, -0.00010299337092334326],
            [0.999919657666892, -0.00040201288971175586, 0.0010475621984067374, -0.0008181196924343936],
        ),
        (
            [0, 128, 128, 128, 128, 0, 0, -128, -128, -128, -128, -128, -128, -128, 0, 128, 0, 0, 0, -128, -128, -128, -128, -256, -256, -128, -128, -128, -128, -256, -256, -384],
            [1.0000600419157486, -0.0005104836951934644, 0.0004398788915498386, 7.17183857474879e-05],
            [0.9999343369222968, -0.00017514074376112632, 0.0005718846543860678, -0.0005212718110030323],
        ),
        (
            [-384, -256, -256, -256, -128, -128, 0, 0, 0, 0, 0, 0, 0, 0, 0, -128, 512, 512, 512, 512, 512, 384, 384, 256, 256, 256, 256, 256, 256, 256, 256, 384],
            [0.9999279632473395, 0.00026716644080782004, -0.0001607886339325404, -8.837193737498694e-05],
            [1.0002391794038092, -0.0004587287110471294, 0.00043332530711559514, -3.484620151539275e-05],
        ),
    ];

    fn ca_factors(raw: &[i16]) -> Vec<f32> {
        raw.iter().map(|&v| v as f32 * (-21f32).exp2() + 1.0).collect()
    }

    fn plane(k: &[f64; 4], x: f64) -> f64 {
        k[0] + k[1] * x.powi(2) + k[2] * x.powi(4) + k[3] * x.powi(6)
    }

    #[test]
    fn the_render_ca_is_camera_raws_polynomial_per_channel() {
        // In the sensor frame with no distortion, green's source radius IS the
        // output radius, so the engine's spline must be the DNG's plane
        // itself. The old reading missed by up to 1.1e-4.
        const SENSOR: (f32, f32) = (9600.0, 6376.0);
        let mut worst = 0.0f64;
        for (i, (raw, red, blue)) in CAMERA_RAW_CA.iter().enumerate() {
            for (half, adobe) in [(&raw[..16], red), (&raw[16..], blue)] {
                let knots = sony_render_ca(&ca_factors(half), &[], SENSOR, Some(SENSOR), SONY_RENDER_DISTORTION_KNOTS);
                assert_eq!(knots.len(), SONY_RENDER_DISTORTION_KNOTS);
                for s in 0..=200 {
                    let r = s as f64 / 200.0;
                    let d = (f64::from(crate::render::profile_knot_interp(&knots, r as f32)) - plane(adobe, r)).abs();
                    worst = worst.max(d);
                    assert!(d < 1e-5, "frame {i} r={r}: engine CA off Camera Raw's by {d}");
                }
            }
        }
        eprintln!("worst |engine − Camera Raw| CA factor: {worst:e}");
    }

    #[test]
    fn the_render_ca_is_read_at_greens_source_radius() {
        // In the ILCE-7RM4A's crop, under each frame's own distortion: red over
        // green at output radius r must be the plane at green's raw-frame
        // radius, as the engine's distortion spline actually draws it.
        let (crop, sensor) = ((9504.0, 6336.0), (9600.0, 6376.0));
        let rho = 9504f64.hypot(6336.0) / 9600f64.hypot(6376.0);
        let mut worst = 0.0f64;
        for (i, (raw, red, blue)) in CAMERA_RAW_CA.iter().enumerate() {
            let dist = knot_factors(&CAMERA_RAW_CASES[i].1);
            let dense = sony_render_distortion(&dist, crop, Some(sensor), SONY_RENDER_DISTORTION_KNOTS);
            for (half, adobe) in [(&raw[..16], red), (&raw[16..], blue)] {
                let knots = sony_render_ca(&ca_factors(half), &dist, crop, Some(sensor), SONY_RENDER_DISTORTION_KNOTS);
                for s in 0..=100 {
                    let r = s as f32 / 100.0;
                    let t = rho * f64::from(r * engine_factor(&dense, crop, r));
                    let d = (f64::from(crate::render::profile_knot_interp(&knots, r)) - plane(adobe, t)).abs();
                    worst = worst.max(d);
                    assert!(d < 1e-5, "frame {i} r={r}: CA read at the wrong radius by {d}");
                }
            }
        }
        eprintln!("worst CA error at green's source radius: {worst:e}");
        // The defensive clamp keeps the CA spline whole too.
        let knots = sony_render_ca(&ca_factors(&CAMERA_RAW_CA[0].0[..16]), &[], crop, Some(sensor), SONY_RENDER_DISTORTION_KNOTS);
        let mut p = LensProfile { ca_r: knots.clone(), ca_b: knots.clone(), ..LensProfile::default() };
        p.clamp();
        assert_eq!(p.ca_r, knots, "clamp must keep the CA spline whole");
    }

    #[test]
    fn a_constant_camera_map_renders_as_the_identity() {
        // The zoom divides a constant map out exactly; the old reading did
        // too, so this pins the zoom's normalisation, not the change.
        let dense = sony_render_distortion(&[1.03; 16], (9504.0, 6336.0), Some((9600.0, 6376.0)), 64);
        for r in [0.0f32, 0.3, 0.7, 1.0] {
            assert!((engine_factor(&dense, (9504.0, 6336.0), r) - 1.0).abs() < 1e-6);
        }
        // Fewer than five knots cannot carry four terms: left as they are.
        assert_eq!(sony_render_distortion(&[1.0, 1.01, 1.02], (3.0, 2.0), None, 64), vec![1.0, 1.01, 1.02]);
    }

    #[test]
    fn malformed_counts_degrade_to_absent() {
        // The count guard is exercised through `read` on real files; here the
        // conversion helpers must at least stay finite on extreme inputs.
        for v in [i16::MIN, -1, 0, 1, i16::MAX] {
            assert!(vignette_gain(v).is_finite());
        }
    }

    /// Real-machine probe (ignored in CI): AUTOSHADE_PROBE_RAW=<file.arw>
    /// cargo test --release probe_real_lens_metadata -- --ignored --nocapture
    #[test]
    #[ignore]
    fn probe_real_lens_metadata() {
        let Some(path) = crate::config::live_env("AUTOSHADE_PROBE_RAW") else {
            eprintln!("AUTOSHADE_PROBE_RAW unset — skipping");
            return;
        };
        let p = read(Path::new(&path));
        eprintln!(
            "vignette {} knots (corner gain {:?}), distortion {} (corner {:?}), ca {}+{}",
            p.vignette.len(),
            p.vignette.last(),
            p.distortion.len(),
            p.distortion.last(),
            p.ca_r.len(),
            p.ca_b.len()
        );
        eprintln!(
            "mask warp: {:?} ({} knots, centre {:?}, corner {:?})",
            p.mask_warp_src,
            p.mask_warp.len(),
            p.mask_warp.first(),
            p.mask_warp.last()
        );
        assert!(!p.vignette.is_empty() && !p.distortion.is_empty(), "A7RIV files carry all three");
        assert!((p.vignette[0] - 1.0).abs() < 0.02, "centre gain ~1.0");
        assert_eq!(p.mask_warp_src, MaskWarpSource::CameraMetadata);
        assert_eq!(p.mask_warp.len(), MASK_WARP_KNOTS);
    }
}
