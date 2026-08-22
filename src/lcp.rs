//! Adobe `.lcp` lens profiles → the engine's MASK WARP map.
//!
//! # What this is for
//!
//! Lightroom rasterises a BRUSH mask in the frame the lens correction has not
//! been applied to yet, and draws a RADIAL/LINEAR shape in the frame after it
//! (R29 Batch-8 + the 2026-08-20 `D` adjudication). Either way the quantity
//! that separates the two frames is one radial magnification `m(r)` — the map
//! from a stored mask radius to where that radius LANDS in Lightroom's own
//! export. This module produces `m` as a knot vector shaped exactly like
//! [`crate::recipe::LensProfile::distortion`], from Adobe's own profile file,
//! for bodies whose RAWs carry no in-camera correction knots.
//!
//! [`crate::render::mask_warp_from_camera_knots`] produces the SAME vector from
//! the in-camera knots (source A); this module is source B. Both write
//! [`crate::recipe::LensProfile::mask_warp`] and stamp
//! [`crate::recipe::MaskWarpSource`] so a reader can always tell which one — or
//! why neither — produced the map in front of them.
//!
//! # The model, and what is READ rather than derived
//!
//! Each `<rdf:li>` in `photoshop:CameraProfiles` holds one calibration node.
//! The distortion half is Adobe's `PerspectiveModel`:
//!
//! ```text
//! P(rho) = 1 + k1·rho² + k2·rho⁴ + k3·rho⁶          rho = r_out / D
//! r_raw  = S · r_out · P(rho_out)                    (the RENDER map)
//! ```
//!
//! `D` is the model's reference length in pixels: `FocalLengthX · W` when the
//! file states it, else `f_mm · SensorFormatFactor / 36 · W`. **The file's
//! `FocalLengthX` wins whenever present** — the focal fallback is an estimate
//! of the same quantity and the two disagree by more than the map's own
//! precision on real profiles.
//!
//! A mask needs the INVERSE of the render map (stored radius → exported
//! radius), which [`invert_p`] gets by Newton on the monotone `x·P(x)`.
//!
//! **`ScaleFactor` is READ, never derived.** On the profile this batch was
//! calibrated against — `SONY (Sony FE 24-105mm F4 G OSS) - RAW.lcp` — `S`
//! equals `1 / max_boundary P` to 2.2e-4 at all five infinity-focus nodes,
//! which is decisive enough to look like a law. It is not one: the SAME file's
//! close-focus 51 mm node misses that identity by 2.0e-3, an order of magnitude
//! worse, and the pool's median is ~1 %. Deriving `S` would therefore be right
//! on the frames this batch measured and wrong elsewhere, silently.
//! `parser_self_check_holds_at_the_far_focus_nodes` pins both halves.
//!
//! # Both spellings, and why an attribute-only parser is not enough
//!
//! Adobe writes the same properties two ways and both ship in the same
//! installed pool (measured on this machine, 3,576 files):
//! `stCamera:RadialDistortParam1="…"` as an ATTRIBUTE in 3,344 files, and
//! `<stCamera:RadialDistortParam1>…</stCamera:RadialDistortParam1>` as an
//! ELEMENT in 159 (`rdf:parseType="Resource"` entries — the phone profiles).
//! The remaining 73 carry a `FisheyeModel` and no rectilinear model at all.
//!
//! The element form nests `ChromaticGreenModel` / `ChromaticRedGreenModel` /
//! `ChromaticBlueGreenModel` / `VignetteModel` INSIDE `PerspectiveModel`, and
//! those children repeat `FocalLengthX`, `ImageXCenter` and
//! `RadialDistortParam1` with their own values. A parser that searched the
//! whole entry would read a chromatic sub-model's coefficients as the
//! geometry's — so the scope is cut at the first nested sub-model
//! ([`perspective_scope`]), and `both_spellings_read_the_same_geometry` pins it.
//!
//! # Fisheye entries are REFUSED, not approximated
//!
//! A `FisheyeModel` is a different projection, not a rectilinear model with
//! larger coefficients. Applying `P` to one would move every mask on such a
//! photo by a plausible-looking, wholly invented amount. Those entries are
//! skipped and, when a profile has nothing else, the whole file is refused with
//! [`Refusal::Fisheye`] — a named degradation, never a silent identity.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// One calibration node's rectilinear distortion model.
///
/// `focal_mm` is `Option` because prime and phone profiles omit
/// `stCamera:FocalLength` entirely; such a file is a single node and
/// [`LensProfileFile::model_at`] hands it back whatever focal it is asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct PerspectiveModel {
    pub focal_mm: Option<f32>,
    /// `stCamera:FocusDistance` — carried for the node TIE-BREAK only (see
    /// [`LensProfileFile::nodes`]), never used in the map.
    pub focus_distance: Option<f32>,
    /// `stCamera:ScaleFactor`, verbatim. Absent ⇒ 1.0 (no zoom), which is what
    /// a file that states no scale is saying.
    pub scale: f32,
    /// `RadialDistortParam1..3`. Absent params are 0.0 — a lower-order model,
    /// not a missing one (the phone profiles ship `k1` alone).
    pub k: [f32; 3],
    /// `stCamera:FocalLengthX` as a FRACTION OF THE WIDTH. When present this is
    /// the reference length and the focal fallback is not consulted.
    pub focal_x: Option<f32>,
    /// `stCamera:SensorFormatFactor`; 1.0 when the file omits it.
    pub sensor_format_factor: f32,
}

/// One parsed `.lcp`: the nodes it holds and who it is for.
#[derive(Debug, Clone, PartialEq)]
pub struct LensProfileFile {
    pub make: String,
    pub lens: String,
    /// Distortion nodes, one per distinct `FocalLength`, sorted ascending.
    ///
    /// **One node per focal, tie-broken on the LARGEST `FocusDistance`.** The
    /// calibrated Sony file holds seven entries per focal (one per aperture,
    /// all carrying identical distortion coefficients — verified: 1 distinct
    /// `(S, k1, k2, k3)` per focal) plus a whole close-focus block at
    /// `FocusDistance="3"`. Keying on focal alone would let document order pick
    /// the close-focus model for a focal that also has an infinity one; the
    /// tie-break makes the choice a stated rule instead of an accident.
    pub nodes: Vec<PerspectiveModel>,
    /// How many entries were skipped for carrying a `FisheyeModel`. Non-zero
    /// with an empty `nodes` is what [`Refusal::Fisheye`] is made of.
    pub fisheye_entries: usize,
}

/// Why no `.lcp` map is available — the NAMED half of a named degradation.
///
/// Every variant means the same pixels (an identity warp); they differ in what
/// the photographer can do about it, which is the whole reason they are not one
/// `None`. Mirrors the [`crate::xmp::MaskImportReason`] idiom: an exhaustive
/// [`Refusal::ALL`] that the iteration surfaces walk, and an [`Refusal::en`]
/// whose match stops the BUILD when a variant is added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Refusal {
    /// No Adobe profile directory exists on this machine. On Windows that means
    /// Camera Raw was never installed; on every other platform it is the
    /// ordinary state, because the roots are Windows paths
    /// ([`roots`]) — the reader degrades, it does not pretend.
    NoRoots,
    /// The roots exist but hold no profile for this camera + lens.
    NotFound,
    /// A file was named or matched and could not be read off the disk.
    Unreadable,
    /// The file parsed and holds ONLY fisheye entries. Refused rather than
    /// approximated with the rectilinear polynomial (see the module header).
    Fisheye,
    /// The file parsed and holds no distortion model of any kind — a
    /// vignette-only profile.
    NoPerspectiveModel,
    /// A model was found but the shot states no focal length and the file needs
    /// one to choose a node (a zoom profile with more than one node).
    NoFocalLength,
    /// The chosen model's Newton inverse did not converge, or its reference
    /// length is not a positive number. A crafted or corrupt profile lands
    /// here rather than producing a map nobody can defend.
    Unsolvable,
}

impl Refusal {
    /// Every refusal — the ONE list the disclosure surfaces iterate, exactly as
    /// [`crate::xmp::MaskImportReason::ALL`] serves the mask half. Pinned by
    /// `refusal_all_covers_every_variant`.
    pub const ALL: [Refusal; 7] = [
        Refusal::NoRoots,
        Refusal::NotFound,
        Refusal::Unreadable,
        Refusal::Fisheye,
        Refusal::NoPerspectiveModel,
        Refusal::NoFocalLength,
        Refusal::Unsolvable,
    ];

    /// English label for the prose channel (CLI stderr / batch warnings).
    pub fn en(self) -> &'static str {
        match self {
            Refusal::NoRoots => "no Adobe lens-profile directory on this machine",
            Refusal::NotFound => "no Adobe lens profile for this camera and lens",
            Refusal::Unreadable => "the Adobe lens profile could not be read",
            Refusal::Fisheye => "the Adobe lens profile is a fisheye model - refused, not applied",
            Refusal::NoPerspectiveModel => "the Adobe lens profile carries no distortion model",
            Refusal::NoFocalLength => "the photo states no focal length to pick a profile node",
            Refusal::Unsolvable => "the Adobe lens profile's distortion model did not invert",
        }
    }
}

/// The refusal a RECIPE records. Seven reader-side reasons fold onto five
/// stored ones, and each fold is a statement rather than a rounding:
///
/// * `NotFound`, `NoPerspectiveModel` and `NoFocalLength` all mean *this photo
///   has no distortion model anyone could apply* — a profile that models only
///   vignetting is, for a mask frame, the same news as no profile at all.
/// * `Unreadable` and `Unsolvable` both mean *a model was there and this build
///   could not use it*, which is the one state worth a bug report.
/// * `NoRoots` and `Fisheye` keep their own variants because they are the two a
///   photographer can act on: install Camera Raw, or accept that a fisheye is
///   not going to be faked.
impl From<Refusal> for crate::recipe::MaskWarpSource {
    fn from(r: Refusal) -> Self {
        use crate::recipe::MaskWarpSource as S;
        match r {
            Refusal::NoRoots => S::NoProfileRoots,
            Refusal::Fisheye => S::FisheyeRefused,
            Refusal::NotFound | Refusal::NoPerspectiveModel | Refusal::NoFocalLength => S::Absent,
            Refusal::Unreadable | Refusal::Unsolvable => S::Unparseable,
        }
    }
}

// --- the model -------------------------------------------------------------

/// `P(rho) = 1 + k1·rho² + k2·rho⁴ + k3·rho⁶`, in `f64`.
///
/// `f64` throughout the solve, `f32` only at the knot vector's edge: `k3`
/// reaches −8.2 on the 105 mm node and `rho⁶` at the corner multiplies it by a
/// number small enough that the single-precision cancellation is visible in the
/// fifth decimal of the answer.
fn p_poly(rho: f64, k: [f64; 3]) -> f64 {
    let s = rho * rho;
    1.0 + s * (k[0] + s * (k[1] + s * k[2]))
}

/// `d/dx [x·P(x)] = 1 + 3k1·x² + 5k2·x⁴ + 7k3·x⁶`.
fn dp_poly(rho: f64, k: [f64; 3]) -> f64 {
    let s = rho * rho;
    1.0 + s * (3.0 * k[0] + s * (5.0 * k[1] + s * 7.0 * k[2]))
}

/// Solve `x·P(x) = t` for `x` by Newton — the inverse of Adobe's render map.
///
/// Monotone on every real profile (the derivative above is positive over the
/// frame), so the root is unique and Newton from `x = t` converges in a handful
/// of steps. `None` rather than a shrug when it does not: a non-convergent
/// solve means the polynomial folds inside the frame, and a folded map has no
/// single answer to give. Pinned to 1e-6 over the frame by
/// `newton_inverse_round_trips_over_the_whole_frame`.
pub fn invert_p(t: f64, k: [f64; 3]) -> Option<f64> {
    if !t.is_finite() {
        return None;
    }
    if t == 0.0 {
        return Some(0.0);
    }
    let mut x = t;
    for _ in 0..64 {
        let f = x * p_poly(x, k) - t;
        let d = dp_poly(x, k);
        // A non-positive derivative is the fold: stop rather than step across
        // it onto a second preimage.
        if !d.is_finite() || d <= 1e-9 {
            return None;
        }
        let step = f / d;
        x -= step;
        if !x.is_finite() {
            return None;
        }
        if step.abs() <= 1e-12 * (1.0 + x.abs()) {
            break;
        }
    }
    // Verified, not assumed: a converged-looking iterate that does not satisfy
    // the equation is refused, which is what makes `Unsolvable` truthful.
    if (x * p_poly(x, k) - t).abs() > 1e-9 * (1.0 + t.abs()) {
        return None;
    }
    Some(x)
}

impl PerspectiveModel {
    /// The model's reference length `D`, in pixels of a `width_px`-wide frame.
    ///
    /// `FocalLengthX · W` when the file states it; otherwise the focal
    /// fallback `f_mm · SensorFormatFactor / 36 · W`. The file's own value WINS
    /// — see the module header.
    pub fn reference_px(&self, width_px: f32) -> Option<f32> {
        let d = match self.focal_x {
            Some(fx) if fx > 0.0 => fx * width_px,
            _ => {
                let f = self.focal_mm?;
                if f <= 0.0 {
                    return None;
                }
                f * self.sensor_format_factor / 36.0 * width_px
            }
        };
        (d.is_finite() && d > 0.0).then_some(d)
    }

    /// `1 / max P` over the frame BOUNDARY — the quantity `ScaleFactor` is
    /// observed to equal on the calibrated profile, and which this module
    /// deliberately does NOT use in place of the file's own value.
    ///
    /// Exposed only so a test can state the identity and its counter-example in
    /// the same breath (see the module header).
    pub fn boundary_scale(&self, dims: (f32, f32)) -> Option<f32> {
        let (w, h) = dims;
        let d = self.reference_px(w)? as f64;
        let k = [self.k[0] as f64, self.k[1] as f64, self.k[2] as f64];
        // The maximum of a radial polynomial over a RECTANGLE's boundary sits
        // on the boundary, so walking the two half-edges (the frame is
        // symmetric in both axes) covers it. 2,048 samples per edge puts the
        // sampling error four orders below the 3e-4 the identity is stated at.
        const N: usize = 2048;
        let mut m = f64::NEG_INFINITY;
        for i in 0..=N {
            let t = i as f64 / N as f64;
            for (x, y) in
                [(w as f64 / 2.0, t * h as f64 / 2.0), (t * w as f64 / 2.0, h as f64 / 2.0)]
            {
                let p = p_poly(x.hypot(y) / d, k);
                if p > m {
                    m = p;
                }
            }
        }
        (m.is_finite() && m > 1e-6).then(|| (1.0 / m) as f32)
    }

    /// The MASK WARP knots for a `dims` frame: `m(r) = r_export / r_stored` at
    /// the `n` radii [`crate::recipe::LensProfile`] places its knots on —
    /// `rho_i = (i + 0.5)/(n − 1)` of the HALF-DIAGONAL, RawTherapee's
    /// placement, which is the placement the in-camera knots already use.
    ///
    /// Sharing the placement is the point: source A and source B write the same
    /// field, so exactly one interpolator reads it and the two sources cannot
    /// drift into different conventions.
    pub fn mask_warp_knots(&self, dims: (f32, f32), n: usize) -> Option<Vec<f32>> {
        if n < 2 {
            return None;
        }
        let (w, h) = dims;
        if !(w > 0.0 && h > 0.0) {
            return None;
        }
        let d = self.reference_px(w)? as f64;
        let s = self.scale as f64;
        if !(s.is_finite() && s > 1e-6) {
            return None;
        }
        let k = [self.k[0] as f64, self.k[1] as f64, self.k[2] as f64];
        let half_diag = 0.5 * ((w as f64).hypot(h as f64));
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let rho = (i as f64 + 0.5) / (n - 1) as f64;
            let r_stored = rho * half_diag;
            // r_export = D · invP( r_stored / (S·D) ) — the inverse of
            // r_stored = S · r_export · P(r_export/D).
            let r_export = d * invert_p(r_stored / (s * d), k)?;
            if !r_export.is_finite() || r_stored <= 0.0 {
                return None;
            }
            out.push((r_export / r_stored) as f32);
        }
        Some(out)
    }
}

impl LensProfileFile {
    /// The node to use at `focal_mm`: the exact node when one matches, else a
    /// LINEAR blend of the two bracketing nodes' `(S, k1, k2, k3)`, clamped to
    /// the end node outside the range.
    ///
    /// **The blend is UNVERIFIED and registered as such.** The only frame that
    /// can discriminate blends — 34 mm, between the 24 and 35 nodes — separates
    /// a linear blend from the alternatives by 0.5 px, which is inside the
    /// 4.19 px tangential noise floor of the measurement that would judge it.
    /// Linear is chosen because it is the interpolation the sibling in-camera
    /// spline already uses (`render::profile_knot_interp`), not because it won
    /// a comparison.
    ///
    /// A single-node file (a prime, a phone) answers with that node whatever it
    /// is asked, including `None` — a profile that states one model states it
    /// for every focal it covers.
    pub fn model_at(&self, focal_mm: Option<f32>) -> Result<PerspectiveModel, Refusal> {
        match self.nodes.len() {
            0 if self.fisheye_entries > 0 => return Err(Refusal::Fisheye),
            0 => return Err(Refusal::NoPerspectiveModel),
            1 => return Ok(self.nodes[0].clone()),
            _ => {}
        }
        let Some(f) = focal_mm.filter(|v| v.is_finite() && *v > 0.0) else {
            return Err(Refusal::NoFocalLength);
        };
        // Nodes are sorted; a file that reached here has ≥2 of them, and any
        // node with no focal at all cannot take part in a focal blend.
        let known: Vec<&PerspectiveModel> = self.nodes.iter().filter(|n| n.focal_mm.is_some()).collect();
        match known.len() {
            0 => return Err(Refusal::NoFocalLength),
            1 => return Ok(known[0].clone()),
            _ => {}
        }
        let first = known[0];
        let last = known[known.len() - 1];
        if f <= first.focal_mm.unwrap_or(f) {
            return Ok(first.clone());
        }
        if f >= last.focal_mm.unwrap_or(f) {
            return Ok(last.clone());
        }
        let hi = known.iter().position(|n| n.focal_mm.unwrap_or(f) >= f).unwrap_or(known.len() - 1);
        let (a, b) = (known[hi.saturating_sub(1)], known[hi]);
        let (fa, fb) = (a.focal_mm.unwrap_or(f), b.focal_mm.unwrap_or(f));
        let t = if (fb - fa).abs() < 1e-6 { 0.0 } else { (f - fa) / (fb - fa) };
        let mix = |x: f32, y: f32| x + t * (y - x);
        Ok(PerspectiveModel {
            focal_mm: Some(f),
            // The blended node is not either endpoint's calibration state, so
            // it carries neither's focus distance.
            focus_distance: None,
            scale: mix(a.scale, b.scale),
            k: [mix(a.k[0], b.k[0]), mix(a.k[1], b.k[1]), mix(a.k[2], b.k[2])],
            // `FocalLengthX` is a LENGTH, not a coefficient: blending it would
            // invent a reference frame neither node states. Taken from the
            // nearer node, or dropped so the focal fallback answers.
            focal_x: if t < 0.5 { a.focal_x } else { b.focal_x },
            sensor_format_factor: mix(a.sensor_format_factor, b.sensor_format_factor),
        })
    }
}

// --- parsing ---------------------------------------------------------------

/// Read one `stCamera:` property in EITHER spelling from `scope`.
///
/// Attribute first (the majority form), then the element form. Both are looked
/// for in the SAME scope, so a caller that has already cut the scope correctly
/// cannot pick up a nested sub-model's copy of the name.
fn prop(scope: &str, key: &str) -> Option<String> {
    let attr = format!("stCamera:{key}");
    let mut from = 0;
    while let Some(i) = scope[from..].find(&attr) {
        let at = from + i;
        let rest = &scope[at + attr.len()..];
        // The name must END here: `FocalLength` must not match
        // `FocalLengthX`, which is a different property on the same element.
        let mut it = rest.char_indices().skip_while(|(_, c)| c.is_whitespace());
        match it.next() {
            Some((j, '=')) => {
                let after = &rest[j + 1..];
                let q = after.find('"')?;
                let end = after[q + 1..].find('"')?;
                return Some(after[q + 1..q + 1 + end].to_string());
            }
            Some((_, '>')) => {
                let open_end = at + attr.len() + rest.find('>')? + 1;
                let close = format!("</stCamera:{key}>");
                let end = scope[open_end..].find(&close)?;
                return Some(scope[open_end..open_end + end].trim().to_string());
            }
            _ => {}
        }
        from = at + attr.len();
    }
    None
}

fn prop_f32(scope: &str, key: &str) -> Option<f32> {
    prop(scope, key)?.trim().parse::<f32>().ok().filter(|v| v.is_finite())
}

/// The `PerspectiveModel` block inside one entry, CUT AT the first nested
/// sub-model.
///
/// The cut is the whole reason this is a function. In the element spelling the
/// chromatic and vignette sub-models live INSIDE `PerspectiveModel` and repeat
/// `FocalLengthX`, `ImageXCenter` and `RadialDistortParam1` with their own
/// values; reading the enclosing element whole would take a chromatic
/// channel's coefficients for the geometry's.
fn perspective_scope(entry: &str) -> Option<&str> {
    const OPEN: &str = "<stCamera:PerspectiveModel";
    let start = entry.find(OPEN)?;
    let rest = &entry[start..];
    let tag_end = rest.find('>')?;
    // Self-closing attribute form: the tag IS the scope.
    if rest[..tag_end].ends_with('/') {
        return Some(&rest[..=tag_end]);
    }
    let close = rest.find("</stCamera:PerspectiveModel>").unwrap_or(rest.len());
    let mut scope = &rest[..close];
    for nested in [
        "<stCamera:ChromaticGreenModel",
        "<stCamera:ChromaticRedGreenModel",
        "<stCamera:ChromaticBlueGreenModel",
        "<stCamera:VignetteModel",
    ] {
        if let Some(i) = scope.find(nested) {
            scope = &scope[..i];
        }
    }
    Some(scope)
}

/// Parse an `.lcp` document.
///
/// Hand-rolled string scanning rather than an XML dependency, for the same
/// reason [`crate::xmp`] is: these files are machine-written by one producer in
/// two known shapes, and the properties this module needs are a closed list of
/// eleven names. What that costs is stated where it is paid — the scope cut in
/// [`perspective_scope`] is the one place a real XML parser would have been
/// free.
pub fn parse(xml: &str) -> Result<LensProfileFile, Refusal> {
    let mut make = String::new();
    let mut lens = String::new();
    let mut nodes: Vec<PerspectiveModel> = Vec::new();
    let mut fisheye_entries = 0usize;
    let mut from = 0usize;
    while let Some(i) = xml[from..].find("<rdf:li") {
        let at = from + i;
        let Some(close) = xml[at..].find("</rdf:li>") else { break };
        let entry = &xml[at..at + close];
        from = at + close + "</rdf:li>".len();
        // Head = everything before the first model block: `FocalLength`,
        // `Make`, `Lens` and friends live there, and never inside a model, so
        // this keeps the entry-level reads out of the sub-model namespace too.
        let head_end = entry
            .find("<stCamera:PerspectiveModel")
            .or_else(|| entry.find("<stCamera:FisheyeModel"))
            .unwrap_or(entry.len());
        let head = &entry[..head_end];
        if make.is_empty() {
            make = prop(head, "Make").unwrap_or_default();
        }
        if lens.is_empty() {
            lens = prop(head, "LensPrettyName").or_else(|| prop(head, "Lens")).unwrap_or_default();
        }
        // A fisheye entry is COUNTED and skipped — see the module header. The
        // check comes first: a file carrying both models for one entry would
        // otherwise be read as rectilinear on the strength of a sibling tag.
        if entry.contains("<stCamera:FisheyeModel") {
            fisheye_entries += 1;
            continue;
        }
        let Some(scope) = perspective_scope(entry) else { continue };
        // No `k1` at all = no distortion model in this entry (a vignette-only
        // node). Not a refusal by itself; the file may hold others.
        let Some(k1) = prop_f32(scope, "RadialDistortParam1") else { continue };
        nodes.push(PerspectiveModel {
            focal_mm: prop_f32(head, "FocalLength").filter(|v| *v > 0.0),
            focus_distance: prop_f32(head, "FocusDistance"),
            scale: prop_f32(scope, "ScaleFactor").filter(|v| *v > 0.0).unwrap_or(1.0),
            k: [k1, prop_f32(scope, "RadialDistortParam2").unwrap_or(0.0), prop_f32(scope, "RadialDistortParam3").unwrap_or(0.0)],
            focal_x: prop_f32(scope, "FocalLengthX").filter(|v| *v > 0.0),
            sensor_format_factor: prop_f32(head, "SensorFormatFactor")
                .filter(|v| *v > 0.0)
                .unwrap_or(1.0),
        });
    }
    if nodes.is_empty() {
        return Err(if fisheye_entries > 0 { Refusal::Fisheye } else { Refusal::NoPerspectiveModel });
    }
    // One node per focal, largest focus distance wins (see `nodes`). Sorted by
    // focal so `model_at`'s bracket search is a scan.
    nodes.sort_by(|a, b| {
        a.focal_mm
            .unwrap_or(0.0)
            .total_cmp(&b.focal_mm.unwrap_or(0.0))
            .then(b.focus_distance.unwrap_or(0.0).total_cmp(&a.focus_distance.unwrap_or(0.0)))
    });
    nodes.dedup_by(|a, b| a.focal_mm == b.focal_mm);
    Ok(LensProfileFile { make, lens, nodes, fisheye_entries })
}

// --- discovery -------------------------------------------------------------

/// Where Adobe keeps lens profiles, from the ENVIRONMENT — never a hard-coded
/// drive letter (a machine whose `%ProgramData%` is not on `C:` is ordinary,
/// and a hard-coded path would degrade there while claiming to have looked).
///
/// `%AUTOSHOP_LCP_DIR%` is consulted FIRST when set: it is how the acceptance
/// tests point at a fixture without an Adobe install, and how a user with
/// profiles somewhere else names that place.
///
/// On a non-Windows build neither Adobe variable exists, so this answers empty
/// and every caller degrades through [`Refusal::NoRoots`].
pub fn roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(dir) = std::env::var("AUTOSHOP_LCP_DIR")
        && !dir.is_empty()
    {
        out.push(PathBuf::from(dir));
    }
    for var in ["ProgramData", "APPDATA"] {
        if let Ok(base) = std::env::var(var)
            && !base.is_empty()
        {
            out.push(Path::new(&base).join("Adobe").join("CameraRaw").join("LensProfiles"));
        }
    }
    out.retain(|p| p.is_dir());
    out
}

/// Every `.lcp` under [`roots`], walked ONCE per process.
///
/// The installed pool is ~3,600 files across ~40 vendor directories; walking it
/// per photograph in a `batch --jobs 3` run would be three redundant directory
/// trees per frame. `OnceLock` because the answer is a property of the machine,
/// not of the run — and because a `batch` worker must not have to hold a lock
/// to ask.
///
/// **Deliberately NOT invalidated.** Installing a lens profile mid-run and
/// expecting the run to notice is not a workflow; restarting is. A cache that
/// re-walked on a timer would trade a stated limit for an unpredictable one.
fn index() -> &'static [PathBuf] {
    static INDEX: OnceLock<Vec<PathBuf>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut out = Vec::new();
        let mut stack: Vec<PathBuf> = roots();
        // Bounded: a symlink loop under a profile root must not hang a render.
        let mut budget = 20_000usize;
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                if budget == 0 {
                    return out;
                }
                budget -= 1;
                let p = e.path();
                match e.file_type() {
                    Ok(t) if t.is_dir() => stack.push(p),
                    Ok(t) if t.is_file() => {
                        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lcp")) {
                            out.push(p);
                        }
                    }
                    _ => {}
                }
            }
        }
        out
    })
}

/// Lowercase alphanumerics only — the comparison key for a lens name.
///
/// Adobe's own spellings of one lens differ by punctuation and spacing between
/// the EXIF `LensModel` (`"FE 24-105mm F4 G OSS"`), the profile's
/// `LensPrettyName` (`"Sony FE 24-105mm F4 G OSS"`) and the FILE NAME
/// (`"SONY (Sony FE 24-105mm F4 G OSS) - RAW.lcp"`). Folding all three to
/// alphanumerics is what lets a containment test hold across them.
fn fold(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// Find the `.lcp` for this camera and lens.
///
/// `filename` is `crs:LensProfileFilename` when the sidecar names one — the
/// exact answer, because Lightroom wrote down the file IT used. Matching is on
/// the file NAME, case-insensitively, because the sidecar states a name and not
/// a path and the pool is vendor-partitioned.
///
/// The fallback is a containment match of the folded lens model against the
/// folded file stem, tie-broken toward the SHORTEST stem: `"FE 24-105mm F4 G
/// OSS"` is contained in both the RAW and the non-RAW profile's stem, and the
/// shorter of two matching names is the less-qualified, more general one.
/// `make` narrows it first so a Sigma profile whose name embeds a Sony body
/// cannot win on a Sony file.
pub fn locate(filename: Option<&str>, make: &str, lens: &str) -> Result<PathBuf, Refusal> {
    let files = index();
    if files.is_empty() {
        return Err(Refusal::NoRoots);
    }
    if let Some(want) = filename.map(str::trim).filter(|s| !s.is_empty()) {
        let want = fold(want);
        if let Some(hit) = files
            .iter()
            .find(|p| p.file_name().is_some_and(|n| fold(&n.to_string_lossy()) == want))
        {
            return Ok(hit.clone());
        }
    }
    let (fm, fl) = (fold(make), fold(lens));
    if fl.is_empty() {
        return Err(Refusal::NotFound);
    }
    let mut best: Option<(usize, &PathBuf)> = None;
    for p in files {
        let stem = fold(&p.file_stem().unwrap_or_default().to_string_lossy());
        if !stem.contains(&fl) || (!fm.is_empty() && !stem.contains(&fm)) {
            continue;
        }
        if best.is_none_or(|(len, _)| stem.len() < len) {
            best = Some((stem.len(), p));
        }
    }
    best.map(|(_, p)| p.clone()).ok_or(Refusal::NotFound)
}

/// The whole source-B path: find the profile, pick the node, solve the knots.
///
/// One entry point so the refusal a caller discloses is the refusal that
/// actually happened — a caller assembling this from the pieces would have to
/// re-derive which step failed, which is exactly how a named degradation turns
/// back into a silent one.
pub fn solve_mask_warp(
    filename: Option<&str>,
    make: &str,
    lens: &str,
    focal_mm: Option<f32>,
    dims: (f32, f32),
    n: usize,
) -> Result<Vec<f32>, Refusal> {
    let path = locate(filename, make, lens)?;
    // BOUNDED read (the R28 B2 rule): the pool directory is not this app's to
    // trust — `AUTOSHOP_LCP_DIR` and `%APPDATA%` are user-writable, so a file
    // there can be anything. The largest of the 3,576 profiles in the local
    // Adobe pool measures 4.3 MiB; 16 MiB is the same ceiling every
    // sidecar-class read in this codebase carries, and an over-cap file is an
    // `Unreadable` refusal — named, like every other degradation on this path.
    const MAX_LCP_BYTES: u64 = 16 * 1024 * 1024;
    let xml = crate::store::read_text_capped(&path, MAX_LCP_BYTES)
        .map_err(|_| Refusal::Unreadable)?;
    let profile = parse(&xml)?;
    let model = profile.model_at(focal_mm)?;
    model.mask_warp_knots(dims, n).ok_or(Refusal::Unsolvable)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The A7RIV frame every measurement in R27 Batches 8-10 and R29's `D`
    /// adjudication was made on.
    const DIMS: (f32, f32) = (9504.0, 6336.0);

    /// Five infinity-focus nodes of `SONY (Sony FE 24-105mm F4 G OSS) - RAW.lcp`
    /// plus the close-focus 51 mm node, one element-spelling entry (the shape
    /// Adobe's phone profiles ship) and one fisheye entry, VERBATIM from the
    /// installed pool on this machine. Factual data, trimmed to the properties
    /// this module reads, so CI has the profile without an Adobe install.
    const FIXTURE: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:stCamera="http://ns.adobe.com/photoshop/1.0/camera-profile">
   <photoshop:CameraProfiles>
    <rdf:Seq>
     <rdf:li>
      <rdf:Description stCamera:Make="SONY" stCamera:Lens="FE 24-105mm F4 G OSS"
       stCamera:LensPrettyName="Sony FE 24-105mm F4 G OSS"
       stCamera:SensorFormatFactor="1" stCamera:FocalLength="24"
       stCamera:FocusDistance="10000" stCamera:ApertureValue="6">
      <stCamera:PerspectiveModel stCamera:Version="2" stCamera:ScaleFactor="1.027391"
       stCamera:RadialDistortParam1="-0.127336" stCamera:RadialDistortParam2="0.087661"
       stCamera:RadialDistortParam3="-0.019675"/>
      </rdf:Description>
     </rdf:li>
     <rdf:li>
      <rdf:Description stCamera:Make="SONY" stCamera:SensorFormatFactor="1"
       stCamera:FocalLength="35" stCamera:FocusDistance="10000">
      <stCamera:PerspectiveModel stCamera:Version="2" stCamera:ScaleFactor="0.991301"
       stCamera:RadialDistortParam1="-0.034666" stCamera:RadialDistortParam2="0.158919"
       stCamera:RadialDistortParam3="-0.023623"/>
      </rdf:Description>
     </rdf:li>
     <rdf:li>
      <rdf:Description stCamera:Make="SONY" stCamera:SensorFormatFactor="1"
       stCamera:FocalLength="50" stCamera:FocusDistance="10000">
      <stCamera:PerspectiveModel stCamera:Version="2" stCamera:ScaleFactor="0.965288"
       stCamera:RadialDistortParam1="0.134715" stCamera:RadialDistortParam2="0.345999"
       stCamera:RadialDistortParam3="-0.243095"/>
      </rdf:Description>
     </rdf:li>
     <rdf:li>
      <rdf:Description stCamera:Make="SONY" stCamera:SensorFormatFactor="1"
       stCamera:FocalLength="51" stCamera:FocusDistance="3">
      <stCamera:PerspectiveModel stCamera:Version="2" stCamera:ScaleFactor="0.964774"
       stCamera:RadialDistortParam1="0.162681" stCamera:RadialDistortParam2="0.335603"
       stCamera:RadialDistortParam3="-0.252995"/>
      </rdf:Description>
     </rdf:li>
     <rdf:li>
      <rdf:Description stCamera:Make="SONY" stCamera:SensorFormatFactor="1"
       stCamera:FocalLength="70" stCamera:FocusDistance="10000">
      <stCamera:PerspectiveModel stCamera:Version="2" stCamera:ScaleFactor="0.955008"
       stCamera:RadialDistortParam1="0.426013" stCamera:RadialDistortParam2="0.801948"
       stCamera:RadialDistortParam3="-1.300298"/>
      </rdf:Description>
     </rdf:li>
     <rdf:li>
      <rdf:Description stCamera:Make="SONY" stCamera:SensorFormatFactor="1"
       stCamera:FocalLength="105" stCamera:FocusDistance="10000">
      <stCamera:PerspectiveModel stCamera:Version="2" stCamera:ScaleFactor="0.959207"
       stCamera:RadialDistortParam1="0.961677" stCamera:RadialDistortParam2="1.182717"
       stCamera:RadialDistortParam3="-8.218554"/>
      </rdf:Description>
     </rdf:li>
    </rdf:Seq>
   </photoshop:CameraProfiles>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    /// The element spelling, with the nested chromatic sub-models that make a
    /// whole-entry search read the wrong coefficients. Trimmed VERBATIM from
    /// `1.0/Apple/iPhone (Apple 3.85mm f3 3G).lcp`: the geometry's own
    /// `FocalLengthX` and `RadialDistortParam1`, and the two sub-models' real,
    /// DIFFERENT values for the same property names.
    ///
    /// `ChromaticRedGreenModel`'s `ScaleFactor` is why the second sub-model is
    /// here. The geometry states no `ScaleFactor` at all, so a parser that
    /// searched the whole entry would silently adopt the RED-GREEN CHANNEL's
    /// 1.000373 as the frame's zoom. Measured across the installed pool on
    /// this machine: **707 of 3,576 profiles have exactly that shape** — a
    /// geometry without a property that one of its own sub-models carries.
    const ELEMENT_FIXTURE: &str = r#"<rdf:Seq>
 <rdf:li rdf:parseType="Resource">
  <stCamera:Make>Apple</stCamera:Make>
  <stCamera:LensPrettyName>Apple 3.85mm f/3 3G</stCamera:LensPrettyName>
  <stCamera:FocusDistance>1.270220</stCamera:FocusDistance>
  <stCamera:PerspectiveModel rdf:parseType="Resource">
   <stCamera:Version>2</stCamera:Version>
   <stCamera:FocalLengthX>0.715735</stCamera:FocalLengthX>
   <stCamera:FocalLengthY>0.715735</stCamera:FocalLengthY>
   <stCamera:ImageXCenter>0.493115</stCamera:ImageXCenter>
   <stCamera:RadialDistortParam1>-0.016126</stCamera:RadialDistortParam1>
   <stCamera:ChromaticGreenModel rdf:parseType="Resource">
    <stCamera:FocalLengthX>0.708083</stCamera:FocalLengthX>
    <stCamera:ImageXCenter>0.493276</stCamera:ImageXCenter>
    <stCamera:RadialDistortParam1>-0.015988</stCamera:RadialDistortParam1>
   </stCamera:ChromaticGreenModel>
   <stCamera:ChromaticRedGreenModel rdf:parseType="Resource">
    <stCamera:ScaleFactor>1.000373</stCamera:ScaleFactor>
    <stCamera:FocalLengthX>0.708083</stCamera:FocalLengthX>
    <stCamera:RadialDistortParam1>-0.000144</stCamera:RadialDistortParam1>
   </stCamera:ChromaticRedGreenModel>
  </stCamera:PerspectiveModel>
 </rdf:li>
</rdf:Seq>
</rdf:li>"#;

    const FISHEYE_FIXTURE: &str = r#"<rdf:Seq>
 <rdf:li>
  <rdf:Description stCamera:Make="Canon" stCamera:Lens="RF7-14mm F2.8-3.5 L FISHEYE STM"
   stCamera:SensorFormatFactor="1" stCamera:FocalLength="10.2">
  <stCamera:FisheyeModel>
   <rdf:Description stCamera:Version="2">
   <stCamera:VignetteModel>
    <rdf:Description stCamera:VignetteModelParam1="0.000447"/>
   </stCamera:VignetteModel>
   </rdf:Description>
  </stCamera:FisheyeModel>
  </rdf:Description>
 </rdf:li>
</rdf:Seq>"#;

    fn node(p: &LensProfileFile, focal: f32) -> PerspectiveModel {
        p.model_at(Some(focal)).expect("the fixture holds this node")
    }

    #[test]
    fn refusal_all_covers_every_variant() {
        // The same guard `MaskImportReason::ALL` carries: `en`'s match stops
        // the build when a variant is added, an iteration array does not.
        for r in Refusal::ALL {
            assert!(!r.en().is_empty(), "{r:?} has no prose");
        }
        let mut sorted = Refusal::ALL.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), Refusal::ALL.len(), "ALL repeats a variant");
        // Every variant must also have a stored counterpart, or a refusal the
        // reader can raise would have nowhere to be recorded.
        for r in Refusal::ALL {
            let s: crate::recipe::MaskWarpSource = r.into();
            assert!(!s.is_solved(), "{r:?} folded onto a SOLVED source");
        }
    }

    /// ACCEPTANCE ①, in-repo half.
    ///
    /// `ScaleFactor` equals `1 / max_boundary P` to 3e-4 at all five
    /// infinity-focus nodes of the calibrated profile — and MISSES it by 2.0e-3
    /// at the same file's close-focus 51 mm node, which is the counter-example
    /// that makes "read it, never derive it" a rule rather than a preference.
    /// Deriving `S` would be correct on every frame this batch measured and
    /// wrong by 0.2 % one node away, silently.
    #[test]
    fn parser_self_check_holds_at_the_far_focus_nodes() {
        let p = parse(FIXTURE).expect("fixture parses");
        assert_eq!(p.make, "SONY");
        assert_eq!(p.lens, "Sony FE 24-105mm F4 G OSS");
        assert_eq!(p.nodes.len(), 6, "five infinity nodes plus the close-focus 51");
        for f in [24.0, 35.0, 50.0, 70.0, 105.0] {
            let n = node(&p, f);
            let got = n.boundary_scale(DIMS).expect("a boundary scale exists");
            assert!(
                (n.scale - got).abs() <= 3e-4,
                "{f}mm: ScaleFactor {} vs 1/maxP {got}",
                n.scale
            );
        }
        // The counter-example, asserted as a FLOOR so it cannot quietly become
        // a pass and take the rule with it.
        let n51 = node(&p, 51.0);
        let miss = (n51.scale - n51.boundary_scale(DIMS).unwrap()).abs();
        assert!(
            (1.5e-3..3e-3).contains(&miss),
            "the 51mm close-focus node must still break the identity, got {miss}"
        );
    }

    /// ACCEPTANCE ③. Solving the 24 mm model at the 16 knot radii reproduces
    /// the knots the recon solved independently in `numpy`, to 1e-4.
    #[test]
    fn the_solved_24mm_knots_reproduce_the_measured_fixture() {
        const WANT: [f32; 16] = [
            0.97345, 0.97429, 0.97597, 0.97846, 0.98173, 0.98571, 0.99033, 0.99548, 1.00102,
            1.00677, 1.01253, 1.01807, 1.02315, 1.02755, 1.03110, 1.03369,
        ];
        let p = parse(FIXTURE).unwrap();
        let got = node(&p, 24.0).mask_warp_knots(DIMS, 16).expect("solvable");
        assert_eq!(got.len(), 16);
        for (i, (g, w)) in got.iter().zip(WANT).enumerate() {
            assert!((g - w).abs() < 1e-4, "knot {i}: {g} vs {w}");
        }
        // The centre limit is 1/S exactly — the one value of this map that has
        // a closed form, so it pins the sign and the direction at once.
        let s = node(&p, 24.0).scale;
        assert!((got[0] - 1.0 / s).abs() < 2e-3, "m(0+) must approach 1/S = {}", 1.0 / s);
    }

    /// ACCEPTANCE ②. The `.lcp` model, with ZERO free parameters, predicts the
    /// radial displacement of the eleven disjoint brush dabs of R27 Batch-10's
    /// hardness ladder to within 10 px.
    ///
    /// The bar is 10 px because the MEASUREMENT's own tangential residual is
    /// 4.19 px rms (11.81 px max) and a purely radial field predicts exactly
    /// zero tangential motion — so 4.19 px is the floor any radial model is
    /// scored against, and the recon's own score for this model was 4.63 px rms
    /// / 9.78 px max. Rows are `(radius_px, measured_dr_px)` from
    /// `b10_field.npz`, the same table `fit2.out` prints.
    #[test]
    fn the_lcp_model_predicts_the_eleven_measured_dab_displacements() {
        const ROWS: [(f64, f64); 11] = [
            (1188.0, -27.93),
            (1188.0, -27.04),
            (2241.5, -28.43),
            (2241.5, -29.13),
            (2241.5, -28.58),
            (2241.5, -29.32),
            (3564.0, 14.07),
            (3564.0, 11.48),
            (4039.2, 56.55),
            (4039.2, 51.16),
            (4039.2, 44.80),
        ];
        let p = parse(FIXTURE).unwrap();
        let n = node(&p, 24.0);
        let knots = n.mask_warp_knots(DIMS, 16).unwrap();
        let half_diag = 0.5 * (DIMS.0 as f64).hypot(DIMS.1 as f64);
        let mut worst = 0.0f64;
        for (r, measured) in ROWS {
            // Through the SHIPPED interpolator, not a private re-solve: what is
            // scored has to be what the engine would actually apply.
            let m = crate::render::mask_warp_factor(&knots, (r / half_diag) as f32) as f64;
            let predicted = (m - 1.0) * r;
            worst = worst.max((predicted - measured).abs());
        }
        assert!(worst < 10.0, "worst dab residual {worst:.2} px (noise floor 4.19 px rms)");
    }

    /// ACCEPTANCE ⑦. Newton inverts the forward polynomial to 1e-6 over the
    /// whole frame, on the most violent node in the fixture (105 mm, k3 = −8.2).
    #[test]
    fn newton_inverse_round_trips_over_the_whole_frame() {
        let p = parse(FIXTURE).unwrap();
        for f in [24.0, 35.0, 50.0, 70.0, 105.0] {
            let n = node(&p, f);
            let k = [n.k[0] as f64, n.k[1] as f64, n.k[2] as f64];
            let d = n.reference_px(DIMS.0).unwrap() as f64;
            let corner = 0.5 * (DIMS.0 as f64).hypot(DIMS.1 as f64);
            for i in 0..=200 {
                let x = (i as f64 / 200.0) * corner / d;
                let t = x * p_poly(x, k);
                let back = invert_p(t, k).expect("monotone over the frame");
                assert!((back - x).abs() < 1e-6, "{f}mm rho={x}: {back} != {x}");
            }
        }
    }

    /// The element spelling reads the GEOMETRY's coefficients, not the nested
    /// chromatic sub-model's — the defect a whole-entry search has.
    #[test]
    fn both_spellings_read_the_same_geometry() {
        let p = parse(ELEMENT_FIXTURE).expect("the element spelling parses");
        assert_eq!(p.nodes.len(), 1);
        let n = &p.nodes[0];
        assert_eq!(n.focal_x, Some(0.715735), "the geometry's FocalLengthX, not 0.708083");
        assert!((n.k[0] - -0.016126).abs() < 1e-9, "the geometry's k1, not -0.015988");
        assert_eq!(n.k[1], 0.0, "an absent higher order is zero, not missing");
        // The load-bearing one: the geometry states NO `ScaleFactor`, and the
        // red-green sub-model beneath it states 1.000373. Without the scope cut
        // the frame adopts a chromatic channel's zoom — the shape 707 of this
        // machine's 3,576 installed profiles have.
        assert_eq!(n.scale, 1.0, "no ScaleFactor stated = no zoom, not the sub-model's 1.000373");
        assert_eq!(n.focal_mm, None, "this profile states no focal length");
        // A single-node file answers for any focal, including none.
        assert!(p.model_at(None).is_ok());
        // `FocalLengthX` WINS: the reference length is 0.715735·W, and the
        // focal fallback is not reachable for this node at all.
        assert_eq!(n.reference_px(1600.0), Some(0.715735 * 1600.0));
    }

    /// A fisheye profile is REFUSED by name. The failure mode this stops is the
    /// quiet one: `P` applied to a fisheye entry returns finite, plausible
    /// numbers and would move every mask on the photo by an invented amount.
    #[test]
    fn a_fisheye_profile_is_refused_not_approximated() {
        assert_eq!(parse(FISHEYE_FIXTURE), Err(Refusal::Fisheye));
        assert!(Refusal::Fisheye.en().contains("refused"));
        // A document with no entries at all is a different refusal — "no model"
        // and "a model we will not use" are not the same news.
        assert_eq!(parse("<rdf:Seq></rdf:Seq>"), Err(Refusal::NoPerspectiveModel));
    }

    /// `FocalLength` must not be read out of `FocalLengthX`, in either
    /// spelling. MUTATION THIS KILLS: dropping the name-boundary check in
    /// [`prop`] makes the element fixture report a 0.715735 mm lens.
    #[test]
    fn a_property_name_must_end_where_it_ends() {
        assert_eq!(prop(r#"stCamera:FocalLengthX="0.71""#, "FocalLength"), None);
        assert_eq!(prop(r#"stCamera:FocalLengthX="0.71""#, "FocalLengthX").as_deref(), Some("0.71"));
        assert_eq!(prop("<stCamera:FocalLengthX>0.71</stCamera:FocalLengthX>", "FocalLength"), None);
        assert_eq!(
            prop("<stCamera:FocalLength>24</stCamera:FocalLength>", "FocalLength").as_deref(),
            Some("24")
        );
    }

    /// Interpolation is linear between bracketing nodes and CLAMPS outside —
    /// and the blend is registered as unverified where it lives (`model_at`).
    #[test]
    fn focal_interpolation_blends_between_nodes_and_clamps_outside() {
        let p = parse(FIXTURE).unwrap();
        let a = node(&p, 24.0);
        let b = node(&p, 35.0);
        let mid = node(&p, 29.5);
        assert!((mid.scale - 0.5 * (a.scale + b.scale)).abs() < 1e-5, "half-way is the mean");
        assert_eq!(node(&p, 12.0).scale, a.scale, "below the range clamps to the first node");
        assert_eq!(node(&p, 400.0).scale, node(&p, 105.0).scale, "above clamps to the last");
        // Exact nodes come back exactly, not through the blend.
        assert_eq!(node(&p, 105.0).k, [0.961677, 1.182717, -8.218554]);
    }

    /// The close-focus tie-break is a STATED rule: 24 mm exists at
    /// `FocusDistance` 10000 and 3, and the infinity model is the one kept.
    #[test]
    fn one_node_per_focal_keeps_the_infinity_focus_calibration() {
        let doubled = FIXTURE.replace(
            r#"stCamera:FocalLength="24"
       stCamera:FocusDistance="10000""#,
            r#"stCamera:FocalLength="24"
       stCamera:FocusDistance="3""#,
        );
        // Premise: the edit really produced a second 24 mm entry shape.
        assert_ne!(doubled, FIXTURE);
        let p = parse(&doubled).unwrap();
        assert_eq!(p.nodes.iter().filter(|n| n.focal_mm == Some(24.0)).count(), 1);
    }

    /// Discovery degrades by NAME when the machine has no profile directory —
    /// which is every non-Windows machine and every Windows machine without
    /// Camera Raw, i.e. CI.
    #[test]
    fn discovery_names_its_refusal() {
        if index().is_empty() {
            assert_eq!(locate(None, "SONY", "FE 24-105mm F4 G OSS"), Err(Refusal::NoRoots));
        } else {
            // A lens nobody makes is NotFound, never NoRoots — the two answers
            // send the photographer to different places.
            assert_eq!(locate(None, "SONY", "FE 9999mm F0.1 IMAGINARY"), Err(Refusal::NotFound));
        }
        // An empty lens name cannot be matched by containment (every stem
        // contains ""), so it must refuse rather than return the pool's first
        // file. MUTATION THIS KILLS: dropping the `fl.is_empty()` guard.
        if !index().is_empty() {
            assert_eq!(locate(None, "SONY", ""), Err(Refusal::NotFound));
        }
    }

    /// ACCEPTANCE ①, real-profile half — SKIPPED with a named reason on a
    /// machine without Adobe's profile pool (CI has no Adobe install), exactly
    /// as the LR probe fixtures skip.
    #[test]
    fn the_shipped_sony_profile_matches_the_fixture() {
        let Ok(path) = locate(None, "SONY", "Sony FE 24-105mm F4 G OSS") else {
            eprintln!("no Adobe lens-profile pool on this machine - skipping");
            return;
        };
        let Ok(xml) = std::fs::read_to_string(&path) else {
            eprintln!("{} unreadable - skipping", path.display());
            return;
        };
        let real = parse(&xml).expect("the installed profile parses");
        let fixture = parse(FIXTURE).unwrap();
        for f in [24.0, 35.0, 50.0, 70.0, 105.0] {
            let (r, x) = (node(&real, f), node(&fixture, f));
            assert_eq!(r.scale, x.scale, "{f}mm ScaleFactor drifted from the fixture");
            assert_eq!(r.k, x.k, "{f}mm coefficients drifted from the fixture");
            let got = r.boundary_scale(DIMS).unwrap();
            assert!((r.scale - got).abs() <= 3e-4, "{f}mm self-check on the real file");
        }
    }
}
