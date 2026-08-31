//! XMP sidecar writer — render an [`EditRecipe`] as an Adobe Camera Raw /
//! Lightroom `.xmp` sidecar (the `crs:` namespace), so the AI's edit opens as
//! adjustable develop sliders in the user's catalog.
//!
//! Key names, value conventions, and structure were verified against a real ACR
//! sidecar from the user's own library (`DSC08724.xmp`): `ProcessVersion=15.4`,
//! signed-integer sliders, `Sharpness` on 0..150, tone curve as an `rdf:Seq` of
//! `"x, y"` strings (see `docs/M1_PLAN.md` §5 and §9). We emit only the keys we
//! set; Lightroom fills the rest from defaults.

use crate::recipe::{
    BrushStroke, ColorGrade, Crop, CurvePoint, EditRecipe, Hsl, LocalAdjustment, MaskCombine,
    MaskComponent, MaskGeometry, RangeMask,
};

const MAX_XMP_BYTES: usize = 16 * 1024 * 1024;



/// Format an integer-valued slider the way ACR writes it: explicit `+` for
/// positives (`"+14"`, `"-12"`, `"0"`).
fn signed(v: f32) -> String {
    let i = v.round() as i64;
    if i > 0 {
        format!("+{i}")
    } else {
        i.to_string()
    }
}

fn xml_char_allowed(c: char) -> bool {
    (!c.is_control() || matches!(c, '\t' | '\n' | '\r'))
        && !matches!(c, '\u{FFFE}' | '\u{FFFF}')
}

fn xml_text_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().filter(|&c| xml_char_allowed(c)) {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

fn xml_attr_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().filter(|&c| xml_char_allowed(c)) {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // Attribute-value normalization (XML 1.0 §3.3.3) folds a RAW
            // tab/newline/CR to a space in every compliant parser — a mask
            // name holding one would change on its first round trip through
            // Lightroom. Character references are exempt from normalization,
            // and our own reader's xml_unescape decodes them back.
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            _ => out.push(c),
        }
    }
    out
}

fn attr(buf: &mut String, key: &str, val: &str) {
    let val = xml_attr_escape(val);
    buf.push_str(&format!("\n    crs:{key}=\"{val}\""));
}



/// Format a LOCAL adjustment value the way ACR writes it: a bare decimal, no
/// forced `+` (e.g. `"-0.075"`, `"0"`). Distinct from the global `signed()`.
fn local_fmt(v: f32) -> String {
    if v == 0.0 {
        "0".to_string()
    } else {
        format!("{v}")
    }
}

// ---------------------------------------------------------------------------
// The radial ellipse — Lightroom's rotated-corner box ⇄ this engine's bbox
// ---------------------------------------------------------------------------
//
// v0.32.0. Every constant and every formula below is MEASURED, on the user's
// own twelve-frame controlled Lightroom experiment plus pixel measurement of
// the exports (evidence: `~/.claude/plans/r25-materials/lr-experiment/`,
// `probe4/PROBE4-FINAL.md` §4 is the settled statement, `probe3/
// PROBE3-ADDENDUM.md` §3 the falloff, `probe2/PROBE2-VERDICT.md` §5 the
// eight-frame table, `BBOX-DECODE.md` §2 the corner model's own statistics).
// Nothing here is inferred from a family pattern; where a value is still
// unmeasured it is named as such at the site that uses it.

/// The affine constant Lightroom applies between a radial's stored normalised
/// coordinates and the frame it draws them in: `x_px = W·(k·n − (k−1)/2)`, i.e.
/// the mask lives in a frame `k`× the export, CONCENTRIC with it.
///
/// **MEASURED**, not assumed. `_DSC9687` (`Feather="0"`, so the rendered edge
/// IS the ellipse) puts the semi-axis scale at 1.0326 ± 0.0004 by three
/// independent methods (`PROBE2-VERDICT.md` §3.1); `_DSC9681` — a hard-edged
/// mask whose centre sits 2799 px from the frame centre, which is what
/// separates "scale the axes" from "scale the frame" — puts the map at
/// 1.0315 ± 0.0005 and lands on this value to **3 px on a 2799 px lever**
/// (`PROBE4-FINAL.md` §2.1). "The centre stays put" misses by 88 px and is
/// dead twice over.
///
/// ~~ONE loose end, registered by `PROBE4-FINAL.md` §3 and deliberately not
/// modelled: the SEMI-AXIS scale measures 1.0325 on one hard-edge frame and
/// 1.0065 on another, so a single affine cannot be exactly right for both.~~
/// **The registered TRIGGER FIRED 2026-08-19 (R27 Batch-8)** — the user shot
/// exactly the named exports (centred `Feather = 0`, small at 24 mm, large at
/// 105 mm, plus a third at 34 mm) — **and the answer is that this constant is
/// a PER-FRAME quantity, not a constant** (`batch8-report.md` §4). Measured
/// per-frame scale `s`: 0.98395/0.98398 (24 mm), 0.99956/0.99953 (105 mm),
/// 1.00428/1.00398 (34 mm) — `kx` and `ky` agree to 3e-5 within every frame,
/// ellipse-fit residual 0.07–1.16 px on 720 rays, confirmed by a second
/// estimator sharing no code. The "axis vs centre scale split" (old L-05)
/// DISSOLVES: one scale about the frame centre reproduces centre AND both
/// axes on all three frames to ≤ 2.2 px, where this constant misses by
/// 30–104 px. PROBE2/PROBE4's 1.0325/1.0315 remain valid — as THOSE frames'
/// `s`. Mechanism OPEN: the lens-profile hypothesis has the right sign and
/// magnitude on all five frames but is contradicted by radial uniformity
/// (`s` behaves as a pure similarity, which distortion is not).
///
/// ~~WHY THE VALUE STILL STANDS UNCHANGED: with no mechanism there is nothing
/// principled to derive `s` from…~~ **The trigger above ALSO fired, same day
/// (R27 Batch-10, `batch10-report.md` §5): the mechanism is Adobe's
/// LENS-PROFILE DISTORTION.** Toggling `LensProfileEnable` 1→0 on the same
/// capture and radial moves the implied scale 0.98396 → 0.99826 (batch-8's
/// own edge finder, unchanged); independently 11 disjoint brush dabs are
/// displaced PURELY RADIALLY by `dr = −0.02487·r + 2.285e−9·r³` (rms 2.94 px;
/// any pure scale refuted at 11×, this constant at 30×). Batch-8's
/// "pure similarity" counter-argument dissolves: over one mask's narrow
/// annulus a distortion polynomial is locally indistinguishable from a
/// scale — which is exactly why three frames read 0.984/1.000/1.004 and the
/// probe frames read ≈1.032. So the sidecar's stored geometry lives in the
/// PLAIN frame (measures 0.998 with the profile off), Lightroom rasterises
/// the BRUSH mask before its lens correction, and the export shows it warped
/// by a per-lens per-focal polynomial. ~~this engine does not model (no
/// `.lcp` parser; `crs:LensProfileEnable` is never read…)~~ **Both of those
/// parentheses expired on 2026-08-20 (R29 Batch-3) and are corrected below.**
///
/// **RULED 2026-08-19 (user): `1.0`** — render the geometry the sidecar
/// actually stores (the plain frame, measured 0.998 with the profile off)
/// and leave Adobe's warp to a model rather than to one frame's polynomial
/// sample. Strictly better than 1.032 on every frame measured in Batches 8
/// and 10. The `k` plumbing below is kept; at 1.0 every affine below is the
/// identity.
///
/// # What R29 Batch-3 changed, and what it did NOT
///
/// The two things this comment used to say the engine lacked, it now has:
/// [`crate::lcp`] parses Adobe's `.lcp` profiles, and
/// [`lens_profile_enabled`] reads `crs:LensProfileEnable`. The warp itself is
/// solved into [`crate::recipe::LensProfile::mask_warp`] from either the
/// in-camera knots or an `.lcp`, with
/// [`crate::recipe::MaskWarpSource`] naming which — or which of five refusals
/// applies.
///
/// **This constant stays `1.0`, and now for a POSITIVE reason rather than for
/// want of a model.** This boundary represents the STORED sidecar frame, and
/// therefore preserves the coordinates Lightroom wrote. D2 measured two
/// distinct render laws. RADIAL uses point transport about the full-raw centre.
/// LINEAR stores corrected-frame handles: correction ON evaluates its straight
/// gradient in that frame, while correction OFF maps only Zero/Full forward and
/// rebuilds one straight raw-frame gradient. Neither is an XMP-coordinate
/// conversion. Brush dabs remain pre-correction identity.
///
/// The render owns those frame operations. RADIAL asks each pre-geometry sample
/// at `m_lr⁻¹(T_engine(p))`. Active LINEAR asks it at `T_engine(p)` only;
/// inactive LINEAR maps its two handles once through `D_fwd`. This constant
/// preserves the stored parameters for all three. The 105 mm observation still
/// rejects moving RADIAL with the pixel field blindly: pixels move +87.5 px at
/// r≈3250 while the mask similarity is 0.99956.
///
/// These are deliberate RENDER-BEHAVIOUR changes introduced for the v1.0.0
/// release. See the mask-warp block header in `render.rs` for the frame table,
/// measured magnitudes and regression pins.
/// Nothing about it reaches this constant, which is the point.
const LR_MASK_FRAME_SCALE: f64 = 1.0;

/// The frame every normalised `crs:` coordinate — mask box AND crop rectangle
/// — is measured against: the SOURCE frame's pixel size, plus the turn that
/// carries it into the frame this engine displays.
///
/// **`W, H` are the UN-ROTATED SOURCE frame** = the DNG/RAW `DefaultCropSize`,
/// NOT the raw `ImageWidth/Length` and NOT the exported pixel dimensions
/// (`PROBE2-VERDICT.md` §3.3: decoding in the raw frame splits `k_A` and `k_B`
/// by 0.35 % where the source frame holds them to 0.02 %). Two R27
/// measurements pin the two ways "exported dimensions" was wrong:
///
/// * **Cropped** (`P5-cropped-mask-frame.md` §1, HIGH). `PROBE4-FINAL.md` §4
///   opens with "`W, H` = the exported pixel dimensions ( = DNG
///   `DefaultCropSize`)". Those two readings coincide only while
///   `HasCrop="False"`; they diverge the moment a crop exists, and
///   `DefaultCropSize` is the correct one — the mask is laid out on the
///   UNCROPPED frame and the crop is a window onto it (22 of 23 shared masks
///   byte-identical across a crop change, a matched-filter limit of 0.09 % on
///   any crop-frame coupling against a 6.1 DN positive control). Feeding the
///   exported dimensions of a cropped render into the decode displaces
///   `DSC09401_16.9.JPG`'s five radials by **834–1384 px**.
/// * **Portrait** (`P1-portrait-mask-frame.md` §1, HIGH). For a
///   `tiff:Orientation` 5–8 capture the source frame is the un-rotated SENSOR
///   array (9504 × 6336), and the export is already upright with **no**
///   orientation tag and no `tiff:` at all — so reading the JPEG's own
///   dimensions as the mask frame, "the natural thing to do, and what a
///   decoder does by default", is exactly the defect. 7/7 files pick their
///   true frame by `dSS`, and a pure radial declaring +1.6 EV reads as
///   +1.65 EV under the sensor frame and **−1.98 EV** (wrong sign) under the
///   display one.
///
/// Only the RATIO enters the ellipse projection — the decode multiplies the
/// stored half-extents by `W` and `H` to reach pixels and divides by them
/// again to reach the engine's own normalised frame, so `W` and `H` cancel and
/// `s = W/H` is all that survives. The SIZE is kept anyway because the writer
/// declares it (`tiff:ImageWidth/ImageLength`), which is what lets a document
/// we authored be re-imported in the frame it was written in.
///
/// [`turn`](Self::turn) is the map SOURCE → DISPLAY: the capture's EXIF
/// orientation composed with the photographer's own quarter turns
/// ([`crate::render::compose_orientation`]). The projection decodes in the
/// source frame and then moves the whole recipe through
/// [`crate::render::orient_recipe_coords`], which is the algebra this build
/// already owns for the `coord_era` migration — one turn, every geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameAspect {
    w: f64,
    h: f64,
    turn: rawler::Orientation,
}

impl FrameAspect {
    /// From a SOURCE pixel size whose display frame is the same frame (a
    /// landscape capture, a baked image whose pixels are already upright).
    /// `None` for anything that is not a positive, finite rectangle — a zero
    /// dimension would make the projection singular.
    pub fn from_size(w: f64, h: f64) -> Option<Self> {
        FrameAspect::from_size_turned(w, h, rawler::Orientation::Normal)
    }

    /// [`from_size`](Self::from_size) for a capture whose display frame is the
    /// source frame TURNED — `turn` is the composed orientation
    /// ([`crate::render::compose_orientation`]), i.e. the same state
    /// `orient_recipe_coords` takes.
    pub fn from_size_turned(w: f64, h: f64, turn: rawler::Orientation) -> Option<Self> {
        (w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0)
            .then_some(FrameAspect { w, h, turn })
    }

    /// `s = W/H` in the SOURCE frame — the one number the ellipse projection
    /// needs.
    fn aspect(&self) -> f64 {
        self.w / self.h
    }

    /// The source → display turn. `Normal` when the two frames coincide.
    fn turn(&self) -> rawler::Orientation {
        self.turn
    }

    /// The SAME frame with the turn already folded in: the display rectangle,
    /// declaring no turn of its own. This is what a document that will NOT
    /// carry a `tiff:Orientation` must be projected in — geometry and
    /// declaration have to agree, and a document that declares nothing
    /// declares the frame its own pixels are in.
    fn displayed(&self) -> Self {
        let (w, h) = if crate::decode::orientation_transposes(self.turn) {
            (self.h, self.w)
        } else {
            (self.w, self.h)
        };
        FrameAspect { w, h, turn: rawler::Orientation::Normal }
    }

    /// The frame a document DECLARES, from `tiff:ImageWidth` /
    /// `tiff:ImageLength` / `tiff:Orientation`.
    ///
    /// Read off a scope of their OWN, not the crs one: these are `tiff:`
    /// properties, and while they sit on the same `rdf:Description` as the crs
    /// settings in every Lightroom sidecar seen here, nothing makes that
    /// structural — an XMP packet may carry one Description per namespace.
    ///
    /// **They must come from ONE Description, though** (R28 Batch-5 5d, F4
    /// symptom D). The three used to be three independent first-occurrence
    /// searches over the whole document, so a width from one element could be
    /// paired with a length from another and an orientation from a third — a
    /// frame no element in the file actually declares, handed to the mask and
    /// crop decoders as the coordinate system to fold pixel geometry with.
    /// [`FrameScope::resolve`] picks the first `rdf:Description` that declares
    /// BOTH dimensions and all three are read from THAT scope — a scope that is
    /// a TYPE since R29-2, so a caller cannot skip the narrowing by forgetting
    /// it instead of by meaning to. Its two whole-document fallbacks (a
    /// document with no `rdf:Description` at all; one whose Descriptions never
    /// carry both dimensions together) are intended behaviour and are spelled
    /// out there.
    ///
    /// The declared pair is taken VERBATIM as the source frame, including for
    /// the transposing orientations — `F3-REPORT.md`'s public census is 72/72
    /// that a sidecar's `tiff:ImageWidth/ImageLength` are always the sensor
    /// (landscape) frame, and `P1-portrait-mask-frame.md` §3 confirms it on the
    /// user's own library (`_DSC9312.xmp`: `tiff:Orientation="8"` beside the
    /// ARW's `DefaultCropSize = (9504, 6336)`, against a 6336 × 9504 export).
    /// The pre-R27 comment here called that swap "unmeasured"; it is measured
    /// now, and the swap is the DECODER's job (see [`turn`](Self::turn)), not a
    /// reinterpretation of the declaration.
    ///
    /// A missing `tiff:Orientation` reads as `Normal`, which is the same thing
    /// EXIF itself means by an absent tag.
    fn from_xmp(doc: &str) -> Option<Self> {
        let span = FrameScope::resolve(doc);
        FrameAspect::from_size_turned(
            span.declared_number("tiff:ImageWidth")?,
            span.declared_number("tiff:ImageLength")?,
            span.declared_number("tiff:Orientation")
                .filter(|v| (1.0..=8.0).contains(v))
                .map_or(rawler::Orientation::Normal, |v| {
                    rawler::Orientation::from_u16(v as u16)
                }),
        )
    }
}

/// **The scope a `tiff:` frame read is allowed to see — as a TYPE** (R29-2,
/// finishing R28 Batch-5 5d on the namespaces 5d did not reach).
///
/// 5d put the `crs:` side's scope in the signature ([`Tag`] / [`Scope`]) and
/// left the `tiff:` frame family on the old shape: a free
/// `declared_number(doc: &str, name: &str)` whose `doc` meant "the ONE
/// Description this frame is read from" at one call site and "the whole
/// document" at the next, kept straight only by the CONVENTION that every
/// caller ran the narrowing scan (then `frame_description`) first. That is
/// exactly the per-call-site convention 5d removed next door — and the property
/// it guards is the coordinate system every mask and crop decode folds pixel
/// geometry with ([`lr_to_engine`]).
///
/// So the scope is a type here too. A `FrameScope` is produced by
/// [`resolve`](Self::resolve) and nothing else, so "which span is this?" is
/// answered once, by the resolver, instead of separately in each caller's head.
///
/// A dedicated newtype rather than a second meaning bolted onto 5d's [`Scope`]:
/// `Scope`'s reads are `crs:`-anchored ([`CrsSource`]) and its `new` takes any
/// text, so teaching it `tiff:` would hand all 107 existing `crs:` call sites
/// the power to ask a frame question of an unresolved document — the same
/// convention back again, one type wider.
#[derive(Clone, Copy)]
struct FrameScope<'a>(&'a str);

impl<'a> FrameScope<'a> {
    /// The ONE `rdf:Description` a document's `tiff:` frame is read from — the
    /// first one that declares both `tiff:ImageWidth` and `tiff:ImageLength`
    /// (R28 Batch-5 5d, F4 symptom D).
    ///
    /// The span runs from that element's `<` to the end of its BODY (the close
    /// tag carries no attributes, so it is not needed), which is what keeps
    /// BOTH XMP spellings working: the attribute form Lightroom writes lives on
    /// the start tag, and the property-element form lives in the body.
    ///
    /// **Two fallbacks to the WHOLE document, both intended, both pinned by the
    /// R29-2 fixtures at the bottom of this file:**
    ///
    /// * there is no `rdf:Description` in the text at all. A bare fragment
    ///   cannot mix two elements' declarations, so the narrowing has nothing to
    ///   protect there, and refusing would break every reader that hands this a
    ///   snippet — fixtures in this module do exactly that.
    /// * Descriptions are present, but no single one carries both dimensions.
    ///   That is the shape the pre-5d code read; the aspect is disclosed as
    ///   degraded downstream either way, and inventing a refusal here would drop
    ///   the frame for files nobody has measured. The residue is real and named:
    ///   in THAT document the width and the length may still come from different
    ///   elements — symptom D is removed only for documents where some element
    ///   declares both.
    fn resolve(doc: &'a str) -> Self {
        let mut from = 0;
        while let Some((start, gt, self_closing)) = next_xml_tag(doc, from) {
            from = gt + 1;
            let tag = &doc[start..=gt];
            if tag.starts_with("</") || tag_name(tag) != "rdf:Description" {
                continue;
            }
            let end = if self_closing {
                gt + 1
            } else {
                match find_matching_close(doc, gt + 1) {
                    // `find_matching_close` returns the `<` of the close tag; the
                    // close itself carries no attributes, so the body is enough.
                    Some(close) => close,
                    None => continue,
                }
            };
            let span = FrameScope(&doc[start..end]);
            if span.declared_number("tiff:ImageWidth").is_some()
                && span.declared_number("tiff:ImageLength").is_some()
            {
                return span;
            }
        }
        FrameScope(doc)
    }

    /// The number THIS scope declares for `name`, in either XMP spelling —
    /// `name="9504"` (the attribute form Lightroom writes) or
    /// `<name>9504</name>` (the element form the same property is legal in).
    ///
    /// Scanned with [`find_outside_constructs`], so a comment quoting the
    /// property cannot answer for the scope — the rule this module already
    /// enforces for the merge splice. A hit whose next non-space character is
    /// neither `=` nor `>` (i.e. the needle was the prefix of a LONGER name)
    /// gives up rather than searching on: giving up costs the caller its frame,
    /// which is a disclosed, degraded path, where searching on could answer
    /// from an unrelated property.
    fn declared_number(self, name: &str) -> Option<f64> {
        let doc = self.0;
        let at = find_outside_constructs(doc, name)?;
        let rest = doc[at + name.len()..].trim_start();
        let text = match rest.strip_prefix('=') {
            Some(v) => {
                let v = v.trim_start();
                let quote = v.chars().next().filter(|c| *c == '"' || *c == '\'')?;
                v[quote.len_utf8()..].split(quote).next()?
            }
            None => rest.strip_prefix('>')?.split('<').next()?,
        };
        xml_unescape(text)
            .trim()
            .trim_start_matches('+')
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
    }
}

/// The five numbers Lightroom stores for one `Mask/CircularGradient`, in its
/// own normalised frame. NOT a bounding box — see [`lr_to_engine`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct LrRadial {
    top: f64,
    left: f64,
    bottom: f64,
    right: f64,
    angle_deg: f64,
}

/// The five numbers THIS engine stores for the same ellipse
/// ([`MaskGeometry::Radial`]): an axis-aligned box in the engine's normalised
/// frame plus a rotation applied in that frame. Same shape as [`LrRadial`],
/// deliberately a different type — the two are not interchangeable and mixing
/// them up is exactly the defect this batch closed.
#[derive(Clone, Copy, Debug, PartialEq)]
struct EngineRadial {
    top: f64,
    left: f64,
    bottom: f64,
    right: f64,
    angle_deg: f64,
}

/// What [`lr_to_engine`] could make of one stored radial.
#[derive(Clone, Copy, Debug, PartialEq)]
enum RadialDecode {
    /// Decoded whole: shape, tilt and position all carried.
    Exact(EngineRadial),
    /// The document declares no frame ([`FrameAspect::from_xmp`]), so a
    /// non-zero `crs:Angle` cannot be decoded — the rotation is a PIXEL-frame
    /// tilt and turning it into this engine's normalised-frame one needs the
    /// aspect. The axis-aligned reading rides through (with the frame affine,
    /// which needs no aspect) and the caller raises the rotation disclosure,
    /// exactly as every build before v0.32.0 did for every rotated radial.
    Unrotated(EngineRadial),
    /// The corners do not describe an ellipse at the declared angle — the
    /// decode's own `a > 0 ∧ b > 0` guard. Real Lightroom data satisfies it
    /// 80/80 (`BBOX-DECODE.md` §2.1); a sidecar that does not is either
    /// malformed or uses a convention outside `|θ| < 90°`, and rendering it
    /// anyway would draw a mask the file does not describe.
    Refused,
}

/// The 2×2 SVD in closed form: `m = R(θᵤ)·diag(σ₁, σ₂)·R(θᵥ)ᵀ`, returning
/// `(σ₁, σ₂, θᵤ)` with `θᵤ` in radians and `σ₁ ≥ |σ₂|`.
///
/// `σ₂` comes out NEGATIVE when `det m < 0`; callers take `|σ₂|` and keep
/// `R(θᵤ)` as the rotation. That is not a shortcut — `diag(σ₁, −|σ₂|)` is
/// `diag(σ₁, |σ₂|)` composed with a reflection, and a reflection maps the unit
/// circle to itself, so the ELLIPSE (which is all this is used for) is
/// unchanged. It is also what `ANGLE-MODEL.md` §6.1's "force `det U > 0`"
/// asks for, reached without a branch.
///
/// `m` is `[m00, m01, m10, m11]`.
fn svd2(m: [f64; 4]) -> (f64, f64, f64) {
    let (e, f) = ((m[0] + m[3]) / 2.0, (m[0] - m[3]) / 2.0);
    let (g, h) = ((m[2] + m[1]) / 2.0, (m[2] - m[1]) / 2.0);
    let (q, r) = (e.hypot(h), f.hypot(g));
    (q + r, q - r, (h.atan2(e) + g.atan2(f)) / 2.0)
}

/// Fold an ellipse orientation into Lightroom's own canonical window.
///
/// An ellipse's orientation is defined mod 180°, so `(box, angle)` is redundant
/// twice over and Lightroom resolves it by keeping `|θ| ≤ 45°` and letting the
/// `a > b` / `a < b` distinction carry the other quadrant: all 195 radials in
/// the user's library sit in `[−43.945, +44.793]`, zero rows outside ±45°
/// (`BBOX-DECODE.md` §2.3), and LRTimelapse's author has the flip on the record
/// ("Lightroom flips from +45 to −45"). Returns the folded angle in degrees and
/// whether the semi-axes must be SWAPPED to go with it.
fn canonical_lr_angle(deg: f64) -> (f64, bool) {
    let mut d = deg.rem_euclid(180.0);
    if d > 90.0 {
        d -= 180.0;
    }
    if d.abs() > 45.0 { (d - 90.0 * d.signum(), true) } else { (d, false) }
}

/// Lightroom's stored radial → this engine's `MaskGeometry::Radial` geometry.
///
/// **`crs:Top/Left/Bottom/Right` is not a bounding box.** It is the pair of
/// ROTATED CORNERS of the ellipse's own box, written in the frame's PIXEL
/// coordinates (`BBOX-DECODE.md` §1):
///
/// ```text
///     (Left, Top)     = centre + R(θ)·(−a, −b)
///     (Right, Bottom) = centre + R(θ)·(+a, +b)
/// ```
///
/// so the decode is
///
/// ```text
///     X = (R−L)/2·W        Y = (B−T)/2·H          SIGNED, never abs()
///     a =  X·cos θ + Y·sin θ     b = −X·sin θ + Y·cos θ
/// ```
///
/// Read naively — `rx = (R−L)/2`, which is what every build up to v0.31.2 did
/// and what every other implementation in the searchable world still does —
/// the axis ratio is wrong by a median factor of 1.84 over the user's rotated
/// components, p90 4.86, max 40.7; it frequently assigns the MAJOR axis to the
/// wrong axis; and 16 of 195 components decode to a NEGATIVE semi-axis, i.e.
/// they are not readable at all. The model is not a fit: it makes a sign
/// prediction with no free parameters (`Left > Right` forces `Angle > 0`,
/// `Top > Bottom` forces `Angle < 0`, both at once is impossible) which the
/// library confirms 16/16 at p = 2.5 × 10⁻⁵, and the two rendered subjects
/// `_DSC9689` (8.3 : 1 at +24.35°) and `_DSC9685` (1 : 2 at +29.51°, decoded
/// tilt −60.486° against a measured −60.5°) land on it at the pixel
/// (`PROBE2-VERDICT.md` §1, §5).
///
/// The `(a, b)` here are computed in units of the frame HEIGHT rather than in
/// pixels — `W` and `H` cancel out of the whole projection and only `s = W/H`
/// survives (see [`FrameAspect`]). The `a > 0 ∧ b > 0` guard is unaffected: `W`
/// and `H` are positive, so scaling cannot change a sign.
///
/// [`MaskGeometry::Radial`]: crate::recipe::MaskGeometry::Radial
fn lr_to_engine(lr: LrRadial, frame: Option<FrameAspect>) -> RadialDecode {
    let k = LR_MASK_FRAME_SCALE;
    let (ncx, ncy) = ((lr.left + lr.right) / 2.0, (lr.top + lr.bottom) / 2.0);
    let (xn, yn) = ((lr.right - lr.left) / 2.0, (lr.bottom - lr.top) / 2.0);
    // The centre moves with the frame, not just the axes — this is the half
    // `PROBE4-FINAL.md` settled and the half a "scale the semi-axes" reading
    // gets wrong by 88 px at a frame corner. It needs no aspect.
    let boxed = |rx: f64, ry: f64, angle_deg: f64| EngineRadial {
        left: k * ncx - (k - 1.0) / 2.0 - rx,
        right: k * ncx - (k - 1.0) / 2.0 + rx,
        top: k * ncy - (k - 1.0) / 2.0 - ry,
        bottom: k * ncy - (k - 1.0) / 2.0 + ry,
        angle_deg,
    };
    // An UNROTATED radial decodes identically under both readings (115 of the
    // library's 195 components), and the identity needs no aspect and no SVD —
    // taking it verbatim keeps those masks bit-stable instead of routing them
    // through a numerical fold that can only lose digits.
    //
    // The guard still applies, and at `θ = 0` it reads as `R > L ∧ B > T`: an
    // inverted corner pair at zero rotation decodes to a negative semi-axis
    // (it is one of the 16 of 195 the naive reading could not read either),
    // and the sign law forbids it — `Left > Right` occurs only with
    // `Angle > 0`, 6/6. A DEGENERATE box (`R == L`) is refused by the same
    // test: a zero-width ellipse is not an ellipse, where the renderer's
    // `max(1e-4)` used to draw it as a hairline.
    if lr.angle_deg == 0.0 {
        return if xn > 0.0 && yn > 0.0 {
            RadialDecode::Exact(boxed(k * xn, k * yn, 0.0))
        } else {
            RadialDecode::Refused
        };
    }
    let Some(s) = frame.map(|f| f.aspect()) else {
        // No declared frame: the naive box, as before v0.32.0, with the
        // rotation disclosed rather than silently applied or silently dropped.
        //
        // `abs()` here and nowhere else. A box on this path may legitimately
        // carry `Left > Right` (it is rotated — that is why we are here), and
        // the signed reading of it is exactly what needs the aspect we do not
        // have. So the axis-aligned fallback takes the magnitudes, which is
        // what `mask_weight` would have done with them anyway, and the
        // disclosure says the rotation did not arrive. The cost is byte
        // fidelity on re-export for that one shape: an inverted pair comes back
        // sorted. It comes back describing what this build renders, which the
        // alternative does not.
        return RadialDecode::Unrotated(boxed(k * xn.abs(), k * yn.abs(), 0.0));
    };
    let (sin, cos) = lr.angle_deg.to_radians().sin_cos();
    let (a, b) = (xn * s * cos + yn * sin, -xn * s * sin + yn * cos);
    if !(a > 0.0 && b > 0.0) {
        return RadialDecode::Refused;
    }
    // Into the engine's normalised frame: `rx = k·a/W`, `ry = k·b/H`, which in
    // height units is `k·a/s` and `k·b`.
    let (rx, ry) = (k * a / s, k * b);
    // …and fold the PIXEL-frame rotation into the engine's NORMALISED-frame one
    // (`ANGLE-MODEL.md` §6.1). The two differ by up to 11.2° of rendered tilt
    // over the library's `|angle| ≤ 44°` range (§3.5), measured 28.554° against
    // a normalised-frame prediction of 19.692° on `_DSC9600` (§3.2).
    let m = [rx * cos, -ry * sin / s, rx * s * sin, ry * cos];
    let (s1, s2, tu) = svd2(m);
    RadialDecode::Exact(boxed(s1.abs(), s2.abs(), tu.to_degrees()))
}

/// This engine's radial geometry → Lightroom's stored corners. The exact
/// inverse of [`lr_to_engine`]; `R(θ)` is orthogonal, so the round trip is
/// algebraically exact and the two legal corner arrangements Lightroom writes
/// (`Left < Right` and `Left > Right`) both come back byte-stable.
///
/// `None` for the frame means the caller could not learn the aspect, and a
/// non-zero engine angle then cannot be projected: the unrotated ellipse is
/// returned with the angle it could not write, for the caller to disclose.
/// The frame AFFINE is applied either way — it needs no aspect, and leaving it
/// off would make the writer the inverse of nothing.
fn engine_to_lr(e: EngineRadial, frame: Option<FrameAspect>) -> (LrRadial, Option<f64>) {
    let k = LR_MASK_FRAME_SCALE;
    let (cx, cy) = ((e.left + e.right) / 2.0, (e.top + e.bottom) / 2.0);
    let (rx, ry) = (((e.right - e.left) / 2.0).abs(), ((e.bottom - e.top) / 2.0).abs());
    let (ncx, ncy) = ((cx + (k - 1.0) / 2.0) / k, (cy + (k - 1.0) / 2.0) / k);
    let corners = |xn: f64, yn: f64, angle_deg: f64| LrRadial {
        left: ncx - xn,
        right: ncx + xn,
        top: ncy - yn,
        bottom: ncy + yn,
        angle_deg,
    };
    let unrotated = |withheld: Option<f64>| (corners(rx / k, ry / k, 0.0), withheld);
    if e.angle_deg == 0.0 {
        return unrotated(None);
    }
    let Some(s) = frame.map(|f| f.aspect()) else {
        return unrotated(Some(e.angle_deg));
    };
    // `diag(s, 1)·R(angle)·diag(rx, ry)` — the ellipse carried into the
    // isotropic (pixel-proportional) frame, whose SVD reads off the pixel tilt
    // and the pixel semi-axes in units of the frame height.
    let (sin, cos) = e.angle_deg.to_radians().sin_cos();
    let (s1, s2, tu) = svd2([s * cos * rx, -s * sin * ry, sin * rx, cos * ry]);
    let (mut a, mut b) = (s1.abs(), s2.abs());
    let (deg, swap) = canonical_lr_angle(tu.to_degrees());
    if swap {
        std::mem::swap(&mut a, &mut b);
    }
    // Undo the frame affine on the axes, then re-encode the corners. Do NOT
    // sort or clamp the result: when `tan θ > a/b` this legitimately emits
    // `Left > Right`, which is byte-for-byte what Lightroom itself writes
    // (6/6 such rows in the library carry `Angle > 0`, as the model requires),
    // and normalising the box to min/max destroys the mask.
    let (a, b) = (a / k, b / k);
    let (sin, cos) = deg.to_radians().sin_cos();
    (corners((a * cos - b * sin) / s, a * sin + b * cos, deg), None)
}

/// The five numbers Lightroom stores for the CROP — `crs:Crop{Left,Top,Right,
/// Bottom}` plus `crs:CropAngle`. NOT an axis-aligned rectangle: see
/// [`lr_to_engine_crop`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct LrCrop {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    angle_deg: f64,
}

/// What [`lr_to_engine_crop`] could make of one stored crop.
#[derive(Clone, Copy, Debug, PartialEq)]
enum CropDecode {
    /// Decoded into this engine's composition. `crop` is `None` for the
    /// rectangle that IS the whole straightened frame (the straighten-only
    /// carrier, which the writer emits and this collapses back).
    /// `overshoot_frac` is how far outside the straightened frame the
    /// rectangle reached before being clamped, as a fraction of that frame's
    /// longer side — `0.0` when the conversion was exact, and the only lossy
    /// edge of the whole conversion (see §"the one inexactness" below).
    Read { crop: Option<Crop>, straighten_deg: f64, overshoot_frac: f64 },
    /// Tilted, and the document declares no frame ([`FrameAspect::from_xmp`]):
    /// the rectangle's own side lengths cannot be recovered without `W/H`, so
    /// the crop is dropped and the straighten — which needs no aspect — rides
    /// alone. Disclosed, never silent.
    NoFrame { straighten_deg: f64 },
    /// The corners do not describe a rectangle at the declared angle (the
    /// `p > 0 ∧ q > 0` guard), or they leave `[0,1]`. Real Lightroom data
    /// satisfies both; a document that does not is malformed, and cropping to
    /// it would deliver a frame the file does not describe. The tilt is a
    /// separate attribute and is still readable, so it rides — which is what
    /// every build before R27 did with an unreadable rectangle.
    Refused { straighten_deg: f64 },
}

impl CropDecode {
    /// The rectangle this engine will apply, or `None` — no crop, an
    /// unreadable one, or the straighten-only carrier.
    fn crop(self) -> Option<Crop> {
        match self {
            CropDecode::Read { crop, .. } => crop,
            CropDecode::NoFrame { .. } | CropDecode::Refused { .. } => None,
        }
    }

    /// The straighten this engine will apply, in its own clockwise-positive
    /// degrees. Readable on every arm: it needs no aspect and no rectangle.
    fn straighten_deg(self) -> f64 {
        match self {
            CropDecode::Read { straighten_deg, .. }
            | CropDecode::NoFrame { straighten_deg }
            | CropDecode::Refused { straighten_deg } => straighten_deg,
        }
    }
}

/// The straightened frame's size in units of the SOURCE frame's height — the
/// rectangle [`crate::render::rotate_straighten`] leaves behind, which is the
/// frame this engine's [`Crop`] fractions are measured against.
///
/// `inscribed_dims` is homogeneous of degree 1 in `(w, h)`, so evaluating it at
/// `(s, 1)` gives the same FRACTIONS as evaluating it at `(W, H)` — and the
/// renderer's own call is the one definition, called here rather than copied.
fn inscribed_norm(s: f64, straighten_deg: f64) -> (f64, f64) {
    let (w, h) = crate::render::inscribed_dims(s as f32, 1.0, straighten_deg as f32);
    (w as f64, h as f64)
}

/// Lightroom's stored crop → this engine's `(Crop, straighten_deg)`.
///
/// **`crs:Crop{Left,Top,Right,Bottom}` is not an axis-aligned rectangle.** It
/// is the pair of opposite ROTATED CORNERS of the crop rectangle, written as
/// plain fractions of the un-rotated SOURCE frame — the IDENTICAL encoding
/// [`lr_to_engine`] decodes for `Mask/CircularGradient`, one family, two
/// consumers (`P3-cropangle-model.md` §1, HIGH):
///
/// ```text
///     (Left,  Top)    = centre + R(θ)·(−p, −q)
///     (Right, Bottom) = centre + R(θ)·(+p, +q)
///     X = (R−L)/2·W        Y = (B−T)/2·H          SIGNED, never abs()
///     p =  X·cos θ + Y·sin θ     q = −X·sin θ + Y·cos θ
///     W_out = round(2p)          H_out = round(2q)
/// ```
///
/// 7/7 photographs pixel-exact with ZERO free parameters, max residual 0.6 px,
/// across two signs of `CropAngle`, two `tiff:Orientation` states and two
/// source aspect ratios; seven rival models miss by 165–987 px, including the
/// naive AABB this build used until R27 (485 px, 0/7). There is **no `k`
/// magnification on the crop** — global best-fit scale 1.000006. (R27
/// Batch-10 dissolved the crop-vs-mask asymmetry this once stated: the mask
/// frame carries no `k` either — its 1.032 was one frame's lens-profile warp,
/// and `LR_MASK_FRAME_SCALE` is 1.0 by the 2026-08-19 ruling — so crop and
/// mask coordinates alike are plain fractions of the un-rotated source frame.)
///
/// **The sign** (`P3-cropangle-model.md` §4, HIGH, 34× margin on the weakest of
/// six photographs, 7257× on the best). `rot(source → export) = −CropAngle`:
/// Lightroom turns the CONTENT counter-clockwise by `+CropAngle`, i.e. the crop
/// BOX clockwise. [`crate::render::rotate_straighten`] turns the content
/// CLOCKWISE for a positive angle, so `straighten_deg = −CropAngle` — the
/// negation lives HERE, at the boundary, exactly like `LR_MASK_FRAME_SCALE`,
/// and the engine's own convention does not move. Until R27 the value rode 1:1
/// into a clockwise rotator, so every straightened import was tilted by
/// **2 × CropAngle** the wrong way (6.55° on the library's largest).
///
/// **The composition.** Lightroom has no auto-crop step: the crop rectangle is
/// given explicitly, in the source frame, and one resample produces the output.
/// This engine rotates first (auto-cropping to
/// [`crate::render::inscribed_dims`]) and then applies `Crop` as fractions of
/// THAT frame. The two are the same map wherever both can express the
/// rectangle, and this function is the conversion — `d = R(−θ)·(centre −
/// c_src)`, then `left = (d.x + W_i/2 − p)/W_i` (`P3-cropangle-model.md` §6.2's
/// "if the current order is kept").
///
/// **The one inexactness, measured not asserted.** The inscribed rectangle is
/// CENTRED, so a Lightroom crop pushed against the edge of the rotated frame
/// can reach outside it even when it is smaller. Over P3's seven measured
/// specimens the overshoot is 0.00 px, 0.16 px, 0.94 px, 0.95 px, 5.32 px,
/// 5.89 px and 46.77 px (0.85 % of one edge, `_DSC9443_1`) — so the conversion
/// is exact or sub-pixel on six of seven, and the seventh loses less than one
/// percent of one edge. The rectangle is CLAMPED (never sorted — see below)
/// and the amount is reported, because `EditRecipe::clamp` would otherwise do
/// the same clamp later, silently, at the first save.
///
/// **No ordering guard.** `Left > Right` is legal and reachable: under the
/// corner encoding it means `X < 0`, i.e. `tan θ > p/q`, which a 2:3 crop
/// straightened past +33.69° produces (`P3-cropangle-model.md` §6.3, and
/// `crs:CropAngle`'s own range is ±45°, F3 STRONG). The pre-R27 reader required
/// `left < right && top < bottom` and DISCARDED such a crop in silence. The
/// `[0,1]` half of the guard stays: both stored points are corners of a
/// rectangle Lightroom keeps inside the frame.
fn lr_to_engine_crop(lr: LrCrop, frame: Option<FrameAspect>, ours: bool) -> CropDecode {
    // The engine's straighten is CLOCKWISE-positive; Lightroom's CropAngle is
    // the content's counter-clockwise turn. One negation, one place.
    let straighten_deg = -lr.angle_deg;
    let inside = |v: f64| (0.0..=1.0).contains(&v);
    if ![lr.left, lr.top, lr.right, lr.bottom].iter().copied().all(inside) {
        return CropDecode::Refused { straighten_deg };
    }
    // "The whole frame" with a hair of float slack: the straighten-only
    // carrier comes back through the corner conversion as the inscribed
    // rectangle's own corners, which land on 0/1 to within f32 rounding rather
    // than exactly on it. A 10⁻⁶ window is a tenth of a pixel on a 9504 px
    // frame — below anything a crop can mean.
    const FULL_EPS: f64 = 1e-6;
    let full = |c: &Crop| {
        c.left as f64 <= FULL_EPS
            && c.top as f64 <= FULL_EPS
            && c.right as f64 >= 1.0 - FULL_EPS
            && c.bottom as f64 >= 1.0 - FULL_EPS
    };
    // The rectangle read as the axis-aligned one it looks like. Correct at
    // `θ = 0`, where the corner encoding degenerates to exactly that and the
    // source frame IS the straightened frame — bit-stable, no aspect needed,
    // and the positive-extent guard is the `p > 0 ∧ q > 0` one at θ = 0.
    let verbatim = || {
        if !(lr.right > lr.left && lr.bottom > lr.top) {
            return CropDecode::Refused { straighten_deg };
        }
        let crop = Crop {
            left: lr.left as f32,
            top: lr.top as f32,
            right: lr.right as f32,
            bottom: lr.bottom as f32,
        };
        CropDecode::Read {
            crop: (!full(&crop)).then_some(crop),
            straighten_deg,
            overshoot_frac: 0.0,
        }
    };
    if lr.angle_deg == 0.0 {
        return verbatim();
    }
    let Some(s) = frame.map(|f| f.aspect()) else {
        // PROVENANCE RULE, the third (see `xmp_to_recipe`'s two): a tilted
        // rectangle in a document that declares no frame cannot be placed —
        // the corner decode needs `W/H` — but a document WE wrote without a
        // frame holds the rectangle in the straightened frame it was stored
        // in, because that is what this writer's own frameless arm emits.
        // Reading ours back verbatim is that arm's exact inverse; reading a
        // FOREIGN one that way would silently invent a rectangle out of
        // corners that mean something else, so it is dropped and disclosed.
        return if ours { verbatim() } else { CropDecode::NoFrame { straighten_deg } };
    };
    let (sin, cos) = lr.angle_deg.to_radians().sin_cos();
    // Signed half-extents of the stored corner pair, in units of the source
    // frame's HEIGHT (`W` and `H` cancel out of every fraction below, so the
    // aspect is the whole of what the frame contributes).
    let (xn, yn) = ((lr.right - lr.left) / 2.0 * s, (lr.bottom - lr.top) / 2.0);
    let (p, q) = (xn * cos + yn * sin, -xn * sin + yn * cos);
    if !(p > 0.0 && q > 0.0) {
        return CropDecode::Refused { straighten_deg };
    }
    let (wi, hi) = inscribed_norm(s, straighten_deg);
    if !(wi > 0.0 && hi > 0.0) {
        return CropDecode::Refused { straighten_deg };
    }
    // The crop centre, carried from the source frame into the straightened one
    // by the inverse of the rotation the renderer applies.
    let (ox, oy) = (((lr.left + lr.right) / 2.0 - 0.5) * s, (lr.top + lr.bottom) / 2.0 - 0.5);
    let (dx, dy) = (cos * ox + sin * oy, -sin * ox + cos * oy);
    let (x0, y0) = (wi / 2.0 + dx - p, hi / 2.0 + dy - q);
    let (left, right) = (x0 / wi, (x0 + 2.0 * p) / wi);
    let (top, bottom) = (y0 / hi, (y0 + 2.0 * q) / hi);
    let overshoot = [-left, -top, right - 1.0, bottom - 1.0]
        .into_iter()
        .fold(0.0f64, f64::max);
    let clamp01 = |v: f64| v.clamp(0.0, 1.0) as f32;
    let crop = Crop {
        left: clamp01(left),
        top: clamp01(top),
        right: clamp01(right),
        bottom: clamp01(bottom),
    };
    if !(crop.right > crop.left && crop.bottom > crop.top) {
        return CropDecode::Refused { straighten_deg };
    }
    CropDecode::Read {
        crop: (!full(&crop)).then_some(crop),
        straighten_deg,
        // A fraction of the straightened frame's own axis — the caller turns
        // it into pixels with the frame it has.
        overshoot_frac: overshoot,
    }
}

/// This engine's `(Crop, straighten_deg)` → Lightroom's stored corners. The
/// exact inverse of [`lr_to_engine_crop`], and `R(θ)` is orthogonal, so the
/// round trip is algebraic rather than numerical.
///
/// `None` = this recipe has no crop and no tilt (`crs:HasCrop="False"`).
/// A straighten with no crop encodes as the WHOLE straightened frame — which
/// under the corner model is the inscribed rectangle's own four corners, not
/// `0,0,1,1` — because Adobe applies `CropAngle` only under `HasCrop="True"`.
/// At `straighten_deg == 0` the inscribed rectangle IS the frame and those
/// corners are `0,0,1,1` again, byte-identical to every document this writer
/// has ever produced.
///
/// The frameless arm is the one degraded edge: with a tilt and no aspect the
/// corners cannot be built, so the rectangle goes out in the STRAIGHTENED
/// frame it is stored in. Reachable only when the photo's own frame could not
/// be read (`pipeline::photo_frame_aspect` supplies it for every save that has
/// a photo), and it is what every build before R27 wrote for every crop.
fn engine_to_lr_crop(
    crop: Option<&Crop>,
    straighten_deg: f64,
    frame: Option<FrameAspect>,
) -> Option<LrCrop> {
    if crop.is_none() && straighten_deg == 0.0 {
        return None;
    }
    let c = crop.copied().unwrap_or(Crop { left: 0.0, top: 0.0, right: 1.0, bottom: 1.0 });
    let (l, t, r, b) = (c.left as f64, c.top as f64, c.right as f64, c.bottom as f64);
    let angle_deg = -straighten_deg;
    if straighten_deg == 0.0 {
        return Some(LrCrop { left: l, top: t, right: r, bottom: b, angle_deg });
    }
    let Some(s) = frame.map(|f| f.aspect()) else {
        return Some(LrCrop { left: l, top: t, right: r, bottom: b, angle_deg });
    };
    let (wi, hi) = inscribed_norm(s, straighten_deg);
    let (sin, cos) = angle_deg.to_radians().sin_cos();
    let (p, q) = ((r - l) * wi / 2.0, (b - t) * hi / 2.0);
    let (dx, dy) = (((l + r) / 2.0 - 0.5) * wi, ((t + b) / 2.0 - 0.5) * hi);
    // Back into the source frame: the rotation itself, then the corners.
    let (ox, oy) = (cos * dx - sin * dy, sin * dx + cos * dy);
    let (cx, cy) = (ox + s / 2.0, oy + 0.5);
    let (x, y) = (p * cos - q * sin, p * sin + q * cos);
    Some(LrCrop {
        left: (cx - x) / s,
        right: (cx + x) / s,
        top: cy - y,
        bottom: cy + y,
        angle_deg,
    })
}

/// One document's crop block, decoded ([`lr_to_engine_crop`]). Adobe applies
/// `crs:CropAngle` only under `crs:HasCrop="True"`, so anything else is no crop
/// AND no tilt — importing a stale angle from a disabled crop activated a
/// straighten Adobe itself does not render.
fn read_crop(scope: Scope<'_>, frame: Option<FrameAspect>, ours: bool) -> CropDecode {
    let angle = scope.crs_f32("CropAngle").unwrap_or(0.0) as f64;
    if scope.crs_str("HasCrop").as_deref() != Some("True") {
        return CropDecode::Read { crop: None, straighten_deg: 0.0, overshoot_frac: 0.0 };
    }
    let n = |k: &str| scope.crs_f32(k).map(f64::from);
    let (Some(left), Some(top), Some(right), Some(bottom)) =
        (n("CropLeft"), n("CropTop"), n("CropRight"), n("CropBottom"))
    else {
        // A crop block missing a corner is a rectangle we cannot place; the
        // tilt is a separate attribute and still rides. `unparsable_crs_numbers`
        // names the unreadable ones on its own channel.
        return CropDecode::Refused { straighten_deg: -angle };
    };
    lr_to_engine_crop(LrCrop { left, top, right, bottom, angle_deg: angle }, frame, ours)
}

/// What a document's crop cost on the way in, as one English sentence — the
/// crop half of the import disclosures `unparsable_crs_numbers` and
/// `import_losses` already carry for the global sliders and the masks.
///
/// `None` when the crop arrived whole, which is every uncropped document and
/// every un-straightened crop.
pub fn crop_import_note(xmp: &str) -> Option<String> {
    if xmp.len() > MAX_XMP_BYTES || xmlns_conflict(xmp).is_some() {
        return None;
    }
    let scope = crs_own_scope(xmp);
    let frame = FrameAspect::from_xmp(xmp);
    match read_crop(Scope::new(scope.as_ref()), frame, is_autoshade_sidecar(xmp)) {
        CropDecode::Read { overshoot_frac, .. } if overshoot_frac > 0.0 => {
            // In pixels of the frame the document declares, when it declares
            // one — a fraction means nothing to a photographer.
            let px = frame.map(|f| overshoot_frac * f.w.max(f.h));
            Some(format!(
                "the straightened crop reaches {} outside the frame this build's straighten \
                 leaves behind, and was trimmed to fit",
                match px {
                    Some(px) => format!("{px:.0} px"),
                    None => format!("{:.2} % of one edge", overshoot_frac * 100.0),
                }
            ))
        }
        CropDecode::NoFrame { .. } => Some(
            "the crop rectangle is tilted and the document declares no \
             tiff:ImageWidth/ImageLength, so its corners could not be placed — the straighten \
             was imported, the rectangle was not"
                .to_string(),
        ),
        CropDecode::Refused { .. } => Some(
            "the crop rectangle could not be read as a rectangle and was not imported"
                .to_string(),
        ),
        CropDecode::Read { .. } => None,
    }
}

/// One radial coordinate the way Lightroom spells it: six decimals with the
/// trailing zeros trimmed — `"0.114928"`, `"0.875"`, `"-0.153271"`, `"0"`.
///
/// Six decimals IS Lightroom's precision (every `crs:Top/Left/Bottom/Right/
/// Angle` in the reference sidecars carries at most that many), and the trim is
/// what makes a merged sidecar's untouched radial byte-identical to the one
/// Lightroom wrote instead of merely equal to it — `crs:Bottom="0.875"` must
/// not come back as `"0.875000"`.
fn lr_num(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    // `-0.0000004` prints as `-0.000000` and trims to `-0`, which is a number
    // no writer should emit.
    if s == "-0" || s.is_empty() { "0".to_string() } else { s.to_string() }
}

/// A stable 32-uppercase-hex GUID derived from `seed` (no external uuid dep).
/// Deterministic so re-emitting the same recipe yields the same sidecar; the
/// per-mask seed includes the index so masks within a file stay unique.
fn guid(seed: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h1 = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut h1);
    let a = h1.finish();
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    (seed, a).hash(&mut h2);
    let b = h2.finish();
    format!("{a:016X}{b:016X}")
}

/// The ONE inversion bit an XMP radial carries, out of the TWO this recipe
/// spells (R25 P9). `MaskGeometry::Radial::flipped` and `LocalAdjustment::
/// inverted` are separate flags here and `mask_weight` × the weight loop
/// compose them by XOR (render.rs), so their XOR — and only their XOR — is the
/// fact about the photograph. Lightroom has no such pair: it writes the single
/// bit TWICE, as `crs:MaskInverted` on the component and `crs:Flipped` as that
/// value's complement (census of the user's library: 201/201 radials
/// anti-correlated, 0 exceptions; re-derived here on the 7 M-B sidecars, 23/23).
/// So the projection has to collapse, and this is where it collapses.
///
/// Linear gradients get their direction from Zero→Full and Lightroom writes no
/// `crs:Flipped` on one at all (27/27 in the same sidecars), so the `matches!`
/// covers exactly the geometry that has the second flag.
fn lr_net_inverted(m: &LocalAdjustment) -> bool {
    m.inverted ^ matches!(m.mask, MaskGeometry::Radial { flipped: true, .. })
}

/// `(crs:What value, extra geometry attributes)` for a mask geometry, or
/// `None` for geometries classic ACR XMP cannot express (raster bitmaps —
/// the writer skips those corrections; the render still applies them).
/// Coordinates are written raw (unclamped) — ACR gradients legitimately use
/// values outside [0,1].
///
/// `net_inverted` is [`lr_net_inverted`] for the correction this geometry
/// belongs to — the radial arm needs it to write `crs:Flipped`, and the caller
/// writes the SAME bit into `crs:MaskInverted`, so the pair leaves here in the
/// only shape Lightroom itself ever writes.
///
/// The third element of the tuple is the rotation the projection could NOT
/// write, in degrees — `None` whenever the geometry left here whole. See the
/// radial arm.
fn mask_geom_xml(
    g: &MaskGeometry,
    net_inverted: bool,
    frame: Option<FrameAspect>,
) -> Option<(&'static str, String, Option<f64>)> {
    match g {
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => Some((
            "Mask/Gradient",
            format!(
                " crs:ZeroX=\"{zero_x}\" crs:ZeroY=\"{zero_y}\" crs:FullX=\"{full_x}\" crs:FullY=\"{full_y}\""
            ),
            None,
        )),
        // v0.32.0: `angle` IS projected onto `crs:Angle` now, and the box is
        // the ROTATED-CORNER encoding Lightroom actually reads — see
        // `engine_to_lr` and `lr_to_engine`, which carry the measurement this
        // rests on. The old stance ("export the UNROTATED ellipse, name the
        // dropped angle") existed because the sign and the pivot were
        // unverified; both are measured, so it retires. It survives in ONE
        // narrower place: a document that declares no frame gives the
        // pixel→normalised fold no aspect to fold with, and then the writer
        // still emits the unrotated ellipse and hands the angle back for the
        // caller to disclose.
        // `flipped: _` — NOT written straight out any more (R25 P9). See
        // `lr_flipped` below: this recipe's flip is half of an XOR, and the
        // attribute it used to be copied into is Lightroom's complement of
        // `crs:MaskInverted`, so copying it emitted pairs Lightroom never
        // writes and Lightroom rendered them inverted.
        MaskGeometry::Radial {
            top, left, bottom, right, feather, roundness, flipped: _, angle, midpoint,
            mask_version,
        } => {
            let (lr, withheld) = engine_to_lr(
                EngineRadial {
                    top: *top as f64,
                    left: *left as f64,
                    bottom: *bottom as f64,
                    right: *right as f64,
                    angle_deg: *angle as f64,
                },
                frame,
            );
            Some((
                "Mask/CircularGradient",
                {
                    // Lightroom's crs:Feather lives on a 0..100 scale (reference
                    // sidecars carry integers like 50 / 72); the engine's is 0..1.
                    // The old writer emitted the raw 0..1 value, which Lightroom
                    // read as a nearly hard edge — convert on the boundary.
                    let lr_feather = (feather.clamp(0.0, 1.0) * 100.0).round();
                    // `crs:Flipped` is the COMPLEMENT of the correction's
                    // `crs:MaskInverted`, which the caller writes from the same
                    // `net_inverted` — 201/201 radials in the user's library and
                    // 23/23 in the M-B sidecars carry exactly that pair, and
                    // NEITHER of the other two combinations occurs even once.
                    //
                    // Emitting the observed pair is what makes this projection
                    // safe under BOTH readings of the attribute: whether
                    // Lightroom's renderer consults `Flipped` or `MaskInverted`,
                    // an anti-correlated pair says the same thing to it. The old
                    // writer copied `flipped` here while `MaskInverted` came from
                    // `inverted`, so a mask the user had flipped in this app left
                    // as `Flipped="false" MaskInverted="false"` — a combination
                    // Lightroom never writes, and the one it reads as "not
                    // inverted", i.e. the flip was dropped on the way out.
                    let lr_flipped = !net_inverted;
                    // Midpoint / Version ride out exactly as they rode in (R25
                    // P5): both sit on EVERY Lightroom radial, and a sidecar we
                    // rewrite without them is a sidecar that lost two of the
                    // file's own attributes to a reader that could not name
                    // them. Neither is interpreted — see MaskGeometry::Radial.
                    //
                    // The five geometry numbers go out through `lr_num` —
                    // Lightroom's own spelling, and the precision the round
                    // trip is stable to. The projection now runs in f64 while
                    // the recipe stores f32, so printing the stored value's own
                    // Display (as the writer did while the box passed through
                    // verbatim) would publish the f32's decimal tail as data.
                    format!(
                        " crs:Top=\"{top}\" crs:Left=\"{left}\" crs:Bottom=\"{bottom}\" \
crs:Right=\"{right}\" crs:Angle=\"{angle}\" \
crs:Feather=\"{lr_feather}\" crs:Roundness=\"{roundness}\" crs:Flipped=\"{lr_flipped}\" \
crs:Midpoint=\"{midpoint}\" crs:Version=\"{mask_version}\"",
                        top = lr_num(lr.top),
                        left = lr_num(lr.left),
                        bottom = lr_num(lr.bottom),
                        right = lr_num(lr.right),
                        angle = lr_num(lr.angle_deg),
                    )
                },
                withheld,
            ))
        }
        MaskGeometry::Bitmap { .. } => None,
        // A brush group is not an ATTRIBUTE-form component: it carries a
        // `crs:Masks` child holding its strokes, each with a `crs:Dabs` child
        // of its own, so it cannot be expressed as this function's
        // `(what, attributes)` pair. [`brush_mask_xml`] emits the whole
        // element instead and [`masks_xml`] routes to it BEFORE reaching here,
        // so the `None` below is never the answer that decides anything — it
        // exists so this function stays total.
        MaskGeometry::Brush { .. } => None,
        // Same shape of exception as the brush group, one element deeper: an
        // AI mask may carry a `crs:Gesture` child, so it is not an
        // attribute-form component either. [`ai_mask_xml`] emits the whole
        // element and [`masks_xml`] routes to it BEFORE reaching here.
        MaskGeometry::AiMask { .. } => None,
    }
}

/// The `crs:` attributes a [`MaskGeometry::AiMask`] carries as PROVENANCE — the
/// ones this engine never interprets, listed in the order Lightroom writes
/// them.
///
/// **An allowlist, both ways.** The parser refuses a `Mask/Image` carrying an
/// attribute outside this list plus the modelled ones (the roundness rule: a
/// name we have never seen means a writer we have not measured), and the writer
/// emits only names from this list — so a hand-edited `recipe.json` cannot
/// smuggle a novel attribute, or markup, into a sidecar.
///
/// Current corpus re-derivation: 174 sidecars; 40 Mask/Aggregate; 104
/// Mask/Image; 391 Mask/Paint; 1037 Mask/*; 40 crs:Gesture (recursive `*.xmp`
/// census over the operator-supplied corpus root (env `AUTOSHADE_CENSUS_ROOT`),
/// real XML parser, 0 parse failures; refreshed at the v1.1 release). Measured
/// over this corpus: 104 `Mask/Image` instances,
/// 21 distinct attribute names, of which 7 are modelled fields
/// ([`MaskGeometry::AiMask`]) plus `crs:What` and `crs:MaskActive` (an
/// invariant, `"true"` on 104/104) and `crs:MaskSyncID` (re-minted by this
/// writer like every other component's). These eleven are the rest.
///
/// The counts above are the current scoped measurements; the invariants are
/// enforced independently by the vocabulary and parser tests.
/// Historical figures elsewhere in this file and in recipe.rs — the F2 era and
/// the 177-sidecar 2026-08 snapshot both — are retained only as provenance;
/// the active counts above are the re-derived values.
/// A commanded XML census over the operator-supplied corpus root
/// (`AUTOSHADE_CENSUS_ROOT`) and its recursive `*.xmp` files
/// found 177 sidecars, 42 `Mask/Aggregate`, 105 `Mask/Image`, 398 `Mask/Paint`,
/// 1081 `Mask/*` and 40 `crs:Gesture` blocks — none of the older totals came
/// back, and the R28 MANIFEST reported the same divergence independently. The
/// active values above are now the scoped, re-derived corpus counts; historical
/// values remain only as provenance. Nothing in the code
/// depends on a count — the allowlists and the refusals depend on the
/// vocabulary and the invariants, which BOTH censuses agree on.
const AI_MASK_PROVENANCE_KEYS: [&str; 11] = [
    "MaskSubCategoryID",
    "InputDigest",
    "InputDigestVersion",
    "MaskDigest",
    "LocalInputDigest",
    "LocalInputDigestVersion",
    "WholeImageArea",
    "FullMaskSize",
    "Origin",
    "ModelVersion",
    "ErrorReason",
];

/// One [`MaskGeometry::AiMask`] as a complete `crs:CorrectionMasks` member: the
/// `Mask/Image` element, its carried provenance attributes, and the
/// `crs:Gesture` list when the photographer refined it with a stroke.
///
/// **What this is and is not.** It is the INTENT riding back out verbatim, so a
/// sidecar this app rewrites still tells Lightroom which segmentation the
/// photographer asked for, at which click, from which model — and Lightroom
/// recomputes its own alpha from that, exactly as it does for its own files.
/// It is NOT a claim that our render matched Adobe's: those pixels came from a
/// different segmenter and the disclosure channels say so
/// ([`MaskLossReason::AiMaskRecomputed`]).
///
/// The provenance digests ride out UNCHANGED on purpose. They describe the
/// intent (which input, which model version), not our raster; re-minting them
/// would assert a provenance we did not have, and dropping them would lose the
/// photographer's own. `crs:MaskSyncID` is the one identity this writer does
/// mint, because that is what it does for every component it emits.
fn ai_mask_xml(g: &MaskGeometry, sync_seed: &str) -> Option<String> {
    let MaskGeometry::AiMask {
        name,
        subtype,
        ref_x,
        ref_y,
        blend_mode,
        value,
        inverted,
        mask_version,
        provenance,
        gesture,
        ..
    } = g
    else {
        return None;
    };
    let mut extra = String::new();
    for (k, v) in provenance {
        // The allowlist, enforced at the LAST moment before the bytes exist.
        // `recipe.json` is disk input and this string becomes XML someone
        // else's parser reads.
        if AI_MASK_PROVENANCE_KEYS.contains(&k.as_str()) {
            extra.push_str(&format!(" crs:{k}=\"{}\"", xml_attr_escape(v)));
        }
    }
    // The gesture's strokes reuse the Paint spelling exactly — same element,
    // same nine attributes, same three literals-because-they-are-invariants.
    let mut painted = String::new();
    for (k, s) in gesture.iter().enumerate() {
        let dabs: String = s
            .dabs
            .split('\n')
            .map(|t| format!("               <rdf:li>{}</rdf:li>\n", xml_attr_escape(t)))
            .collect();
        painted.push_str(&format!(
            "            <rdf:li>\n\
             <rdf:Description\n\
              crs:What=\"Mask/Paint\" crs:MaskActive=\"true\" crs:MaskBlendMode=\"0\"\n\
              crs:MaskInverted=\"false\" crs:MaskSyncID=\"{id}\" crs:MaskValue=\"{v}\"\n\
              crs:Radius=\"{r}\" crs:Flow=\"{f}\" crs:CenterWeight=\"{cw}\">\n\
             <crs:Dabs>\n\
              <rdf:Seq>\n\
{dabs}              </rdf:Seq>\n\
             </crs:Dabs>\n\
             </rdf:Description>\n\
            </rdf:li>\n",
            id = guid(&format!("{sync_seed}-gesture-{k}")),
            v = s.value,
            r = s.radius,
            f = s.flow,
            cw = s.center_weight,
        ));
    }
    let head = format!(
        "          <rdf:Description\n\
           crs:What=\"Mask/Image\" crs:MaskActive=\"true\" crs:MaskName=\"{mname}\"\n\
           crs:MaskBlendMode=\"{blend_mode}\" crs:MaskInverted=\"{inverted}\" \
crs:MaskSyncID=\"{id}\"\n\
           crs:MaskValue=\"{value}\" crs:MaskVersion=\"{mask_version}\" \
crs:MaskSubType=\"{subtype}\"\n\
           crs:ReferencePoint=\"{ref_x} {ref_y}\"{extra}",
        mname = xml_attr_escape(name),
        id = guid(sync_seed),
    );
    Some(if painted.is_empty() {
        // Self-closing when there is no gesture — 64 of 104 current instances.
        format!("         <rdf:li>\n{head}/>\n         </rdf:li>\n")
    } else {
        format!(
            "         <rdf:li>\n{head}>\n\
          <crs:Gesture>\n\
           <rdf:Seq>\n\
{painted}           </rdf:Seq>\n\
          </crs:Gesture>\n\
          </rdf:Description>\n\
         </rdf:li>\n"
        )
    })
}

/// One [`MaskGeometry::Brush`] group as a complete `crs:CorrectionMasks`
/// member: the `Mask/Aggregate` element, its `crs:Masks` list, one
/// `Mask/Paint` per stroke and each stroke's `crs:Dabs` token stream.
///
/// **Byte-faithful to the measured shape** (F2 §1.1 / §2, verified against
/// `_DSC9583` Mask 7 → Brush 1): the Aggregate's seven attributes in
/// Lightroom's own order, the Paint's nine in Lightroom's own order, one
/// `<rdf:li>` per dab token. Three of those attributes are LITERALS because
/// they are invariants rather than data — `MaskActive="true"` on both,
/// `MaskBlendMode="0"` and `MaskInverted="false"` on the Paint (398/398), and
/// the reader refuses anything else rather than storing it.
///
/// The numbers go out through plain `Display`, NOT through `local_fmt` or
/// `lr_num`. `f32`'s `Display` prints the shortest decimal that round-trips —
/// so `crs:Radius="0.582157"` comes back as `0.582157`, exactly the string the
/// file used, and `crs:Flow="1"` stays `1` rather than becoming `1.000000`. A
/// rounding formatter would republish a value the photographer never chose.
///
/// That rests on the numbers not being computed on, which R29 C1 narrowed:
/// a TURN rescales `Radius` and rewrites the dab stream
/// (`render::orient_recipe_coords`). The rewrite quantises back onto
/// Lightroom's own six-decimal grid, so a portrait capture's round trip still
/// lands on the file's digits (`a_portrait_captures_brush_turns_on_the_way_in_
/// and_comes_home_on_the_way_out`); what it does NOT promise any more is
/// byte-identity for a frame whose aspect does not close that grid.
///
/// `sync_seed` is hashed into fresh `crs:MaskSyncID`s by the writer's own
/// [`guid`] rule, like every other component this module emits. The IDs the
/// file used are carried in `recipe.json` ([`BrushStroke::sync_id`]) but not
/// re-emitted: a sidecar we rewrite is OUR document, and minting IDs is what
/// the rest of this writer already does.
fn brush_mask_xml(g: &MaskGeometry, sync_seed: &str) -> Option<String> {
    let MaskGeometry::Brush { name, blend_mode, value, inverted, strokes } = g else {
        return None;
    };
    let mut painted = String::new();
    for (k, s) in strokes.iter().enumerate() {
        let dabs: String = s
            .dabs
            .split('\n')
            // The storage form joins tokens with '\n' and the reader refuses a
            // token that contains one, so this split is the exact inverse.
            .map(|t| format!("               <rdf:li>{}</rdf:li>\n", xml_attr_escape(t)))
            .collect();
        painted.push_str(&format!(
            "            <rdf:li>\n\
             <rdf:Description\n\
              crs:What=\"Mask/Paint\" crs:MaskActive=\"true\" crs:MaskBlendMode=\"0\"\n\
              crs:MaskInverted=\"false\" crs:MaskSyncID=\"{id}\" crs:MaskValue=\"{v}\"\n\
              crs:Radius=\"{r}\" crs:Flow=\"{f}\" crs:CenterWeight=\"{cw}\">\n\
             <crs:Dabs>\n\
              <rdf:Seq>\n\
{dabs}              </rdf:Seq>\n\
             </crs:Dabs>\n\
             </rdf:Description>\n\
            </rdf:li>\n",
            id = guid(&format!("{sync_seed}-stroke-{k}")),
            v = s.value,
            r = s.radius,
            f = s.flow,
            cw = s.center_weight,
        ));
    }
    Some(format!(
        "         <rdf:li>\n\
          <rdf:Description\n\
           crs:What=\"Mask/Aggregate\" crs:MaskActive=\"true\" crs:MaskName=\"{mname}\"\n\
           crs:MaskBlendMode=\"{blend_mode}\" crs:MaskInverted=\"{inverted}\" \
crs:MaskSyncID=\"{id}\"\n\
           crs:MaskValue=\"{value}\">\n\
          <crs:Masks>\n\
           <rdf:Seq>\n\
{painted}           </rdf:Seq>\n\
          </crs:Masks>\n\
          </rdf:Description>\n\
         </rdf:li>\n",
        mname = xml_attr_escape(name),
        id = guid(sync_seed),
    ))
}

/// A `Mask/RangeMask` component `<rdf:li>` intersected with the correction's
/// geometric mask (empty string when the adjustment has no range). Component
/// structure and attribute values verified against the user's own Lightroom
/// sidecars (`_DSC9245.xmp` luminance, `_DSC9303.xmp` colour): the intersect
/// encoding is `MaskBlendMode="1" + MaskInverted="true" + MaskValue="0"` —
/// i.e. "paint 0 wherever the range does NOT match", which erases everything
/// outside geometry ∩ range. Luminance uses the attribute form
/// (`crs:LumRange="lo_outer lo hi hi_outer"`); colour uses the child-element
/// form with one `crs:PointModels` entry `"r g b px py 0"` (last three numbers
/// assumed sample-point + reserved; see ROADMAP §A for the verification note).
fn range_mask_xml(range: &Option<RangeMask>, sync_id: &str) -> String {
    let Some(rm) = range else { return String::new() };
    let head = |name: &str| {
        format!(
            "         <rdf:li>\n\
          <rdf:Description\n\
           crs:What=\"Mask/RangeMask\" crs:MaskActive=\"true\" crs:MaskName=\"{name}\"\n\
           crs:MaskBlendMode=\"1\" crs:MaskInverted=\"true\" crs:MaskSyncID=\"{sync_id}\"\n\
           crs:MaskValue=\"0\">\n"
        )
    };
    match rm {
        RangeMask::Luminance { lo_outer, lo, hi, hi_outer } => format!(
            "{}\
           <crs:CorrectionRangeMask\n\
            crs:Version=\"3\"\n\
            crs:Type=\"2\"\n\
            crs:Invert=\"false\"\n\
            crs:SampleType=\"0\"\n\
            crs:LumRange=\"{lo_outer:.6} {lo:.6} {hi:.6} {hi_outer:.6}\"\n\
            crs:LuminanceDepthSampleInfo=\"0 0.500000 0.500000\"/>\n\
          </rdf:Description>\n\
         </rdf:li>\n",
            head("Luminance Range"),
        ),
        RangeMask::Color { r, g, b, amount, px, py } => format!(
            "{}\
           <crs:CorrectionRangeMask>\n\
            <rdf:Description\n\
             crs:Version=\"3\"\n\
             crs:Type=\"1\"\n\
             crs:ColorAmount=\"{amount:.6}\"\n\
             crs:Invert=\"false\"\n\
             crs:SampleType=\"0\">\n\
            <crs:PointModels>\n\
             <rdf:Seq>\n\
              <rdf:li>{r:.6} {g:.6} {b:.6} {px:.6} {py:.6} 0</rdf:li>\n\
             </rdf:Seq>\n\
            </crs:PointModels>\n\
            </rdf:Description>\n\
           </crs:CorrectionRangeMask>\n\
          </rdf:Description>\n\
         </rdf:li>\n",
            head("Color Range"),
        ),
    }
}

/// Why one mask does not reach a classic ACR sidecar intact. Produced by the
/// WRITER itself, one verdict per mask per defect ([`masks_xml`]) — so no
/// consumer has to re-derive the projection rules, and a disclosure can never
/// claim something different from what was actually emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MaskLossReason {
    /// Raster geometry: classic XMP has no encoding for it, so the WHOLE
    /// correction is skipped ([`mask_geom_xml`] returns `None`).
    Bitmap,
    /// The eye toggle is off — the correction is skipped rather than exported
    /// as an active edit the app does not render.
    Disabled,
    /// The mask carries extra Add/Subtract/Intersect shapes; only the base
    /// geometry is projected (the render composes them all).
    ///
    /// SINCE R27 Batch-4 this counts the shapes the projection really drops.
    /// A [`MaskGeometry::Brush`] component is emitted in full (see
    /// [`brush_mask_xml`]), so a mask whose only extra component is a brush
    /// group loses nothing here and this is not raised for it — what that mask
    /// gets instead is [`BrushRendered`], which is a statement about the RENDER
    /// and not about the sidecar.
    ///
    /// [`BrushRendered`]: MaskLossReason::BrushRendered
    ComponentsFlattened,
    /// A brush group (`Mask/Aggregate` + its `Mask/Paint` strokes) rides out
    /// into the sidecar COMPLETE — and the pixels AutoShade showed for it were
    /// drawn by **our** rasteriser from a measured model of Lightroom's brush,
    /// not by Adobe's. The XMP is exact; the alpha is an approximation.
    ///
    /// The odd one out in this enum, deliberately: every other variant names
    /// something the PROJECTION could not carry. This one names something the
    /// projection carries perfectly and the RENDERER reproduces only to within
    /// a measurement, which is the opposite direction of loss and needs saying
    /// in the same breath.
    ///
    /// **This variant used to be `BrushCarried`, and it meant「not drawn at
    /// all」** — weight 0 everywhere, from R27 Batch-4 until R29 Batch-6b. What
    /// changed is that the one missing input arrived: the alpha kernel is not
    /// in the sidecar (`recipe::BrushStroke::dabs` — the file stores the
    /// stroke, never the alpha), so it had to be MEASURED, and R29 Batch-6 did
    /// measure it on 29 controlled Lightroom exports —
    /// `k(ρ;h) = (1 − ρ^m(h))^n(h)` at rms 0.0109 held-out, a one-parameter
    /// flow odds law at κ = 0.1284, screen accumulation, density scaling the
    /// dab. `render::brush_raster` is the implementation. Renaming rather than
    /// keeping both was the honest option: nothing raises「carried, not drawn」
    /// any more, and a disclosure variant with no producer is a claim the code
    /// cannot make (this enum's own header: a disclosure must never say
    /// something other than what was actually emitted).
    ///
    /// What is still worth saying, and is what the label now says: our edges
    /// are not Adobe's. The model reproduces the measured ladder to ~0.01 in α
    /// — an order better than the AI-mask arm's re-derivation, and not zero.
    BrushRendered,
    /// An AI mask (`Mask/Image`) rides out into the sidecar COMPLETE — and the
    /// alpha this engine rendered was **recomputed by our own segmenter**, not
    /// Adobe's, so the XMP Lightroom reads and the pixels AutoShade showed do
    /// not describe the same coverage.
    ///
    /// The second member of [`BrushRendered`]'s odd-one-out class, and the one
    /// where the gap is largest. Both are drawn here by something that is not
    /// Adobe's code, but a brush is drawn from a MEASUREMENT of Adobe's own
    /// rasteriser (R29 Batch-6, ~0.01 in α on the ladder it was fitted to),
    /// while an AI mask is drawn by a different SEGMENTER whose edges have
    /// never been compared to Adobe's at all. The sidecar carries no raster
    /// (current corpus: 105 instances, longest attribute value 55 characters), so
    /// this one is structural and permanent, not a to-do.
    ///
    /// [`BrushRendered`]: MaskLossReason::BrushRendered
    AiMaskRecomputed,
    /// A rotated radial exports as its UNROTATED ellipse. v0.32.0 NARROWED
    /// this to one case: `crs:Angle`'s sign and pivot are measured now and the
    /// projection carries the tilt, so what is left is a document with no
    /// declared frame — the pixel↔normalised fold has no aspect to fold with
    /// ([`FrameAspect`], and see [`mask_geom_xml`]). The payload is the
    /// dropped angle in WHOLE DEGREES, so a disclosure can say how much
    /// rotation the sidecar is missing instead of only that some is (R25 P5).
    /// Rounding is the display's, not the model's: `recipe.json` keeps the
    /// exact `f32`, and a half-degree tilt no reader could act on rounds to
    /// `0` — which the prose channels read as "no angle worth naming" and
    /// answer with their plain phrasing.
    Rotation(i32),
    /// Per-channel recolour gains (`color_gains`) are engine-only: classic ACR
    /// has no counterpart, so the sidecar renders without them.
    Recolour,
}

impl MaskLossReason {
    /// Every reason, in the order the prose channels group them (skips before
    /// degradations) — the ONE list both disclosure surfaces iterate
    /// ([`describe_mask_losses`] and the GUI's `xmp_loss_line`).
    ///
    /// Those two used to carry a hand-written array each. [`en`]'s exhaustive
    /// match stops the BUILD when a variant is added, but an iteration array
    /// does not: the new reason would be raised by the writer, counted by
    /// nobody and printed by neither surface. Pinned by
    /// `mask_loss_reason_all_covers_every_variant`, whose own exhaustive match
    /// is where a new variant lands next.
    ///
    /// [`Rotation`] appears with a ZERO payload, exactly as the import twin's
    /// `InertLocal` appears with an empty one: the payload is one mask's
    /// angle, so no single value can stand for the variant. Grouping is by
    /// [`same_kind`], never by `==`.
    ///
    /// [`en`]: MaskLossReason::en
    /// [`Rotation`]: MaskLossReason::Rotation
    /// [`same_kind`]: MaskLossReason::same_kind
    pub const ALL: [MaskLossReason; 7] = [
        MaskLossReason::Bitmap,
        MaskLossReason::Disabled,
        MaskLossReason::ComponentsFlattened,
        MaskLossReason::BrushRendered,
        MaskLossReason::AiMaskRecomputed,
        MaskLossReason::Rotation(0),
        MaskLossReason::Recolour,
    ];

    /// Same VARIANT, payload ignored — the grouping key both prose channels
    /// use, mirroring [`MaskImportReason::same_kind`]. Two masks rotated by
    /// different amounts are one line in a sentence and two values under `==`,
    /// and `ALL`'s placeholder payload matches neither of them.
    pub fn same_kind(self, other: MaskLossReason) -> bool {
        std::mem::discriminant(&self) == std::mem::discriminant(&other)
    }

    /// English label for the prose channel (CLI stderr / web reply). The GUI
    /// renders the same variants in the UI language instead.
    pub fn en(self) -> &'static str {
        match self {
            MaskLossReason::Bitmap => "bitmap mask(s) skipped",
            MaskLossReason::Disabled => "muted mask(s) skipped",
            MaskLossReason::ComponentsFlattened => "extra shape component(s) flattened",
            MaskLossReason::BrushRendered => {
                "brush mask(s) drawn from AutoShade's measured model of Lightroom's brush - \
                 not Adobe's own rasteriser"
            }
            MaskLossReason::AiMaskRecomputed => {
                "AI mask(s) re-derived by the local segmenter - not Adobe's own raster"
            }
            MaskLossReason::Rotation(_) => "radial rotation dropped",
            MaskLossReason::Recolour => "recolour gains dropped",
        }
    }
}

/// One mask defect the XMP projection could not carry. A single mask can
/// appear more than once (a rotated radial with components and recolour gains
/// loses three separate things).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskLoss {
    /// The name the sidecar uses for this correction (`crs:CorrectionName`):
    /// the user's own label when set, else the generated `AutoShade <n>`.
    pub name: String,
    pub reason: MaskLossReason,
}

/// Why one Lightroom correction does not reach this engine WHOLE — the
/// IMPORT-side twin of [`MaskLossReason`], produced by the READER itself
/// ([`classify_correction`]) so no consumer has to re-derive the import rules
/// and a disclosure can never claim something other than what was imported.
///
/// **The asymmetry this closes (R25 P1).** The export side has named its
/// losses per mask since M6a; the import side answered with a single integer
/// and SIX independent "if I do not recognise this, drop the whole correction"
/// gates. Every one of those gates fired on an ordinary Lightroom file:
/// `crs:Angle` is written on EVERY radial (as `"0"` when unrotated) and
/// `crs:MaskBlendMode` on EVERY component. The result was that every mask in
/// the user's catalog was refused on import, with a count as the only
/// explanation. A knob we do not model is not the same thing as a value we
/// cannot read, and only the second one is a reason to refuse a correction.
///
/// Two variants DROP the correction ([`is_drop`]); the other eight are notes
/// on a correction that DID import.
///
/// [`is_drop`]: MaskImportReason::is_drop
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MaskImportReason {
    /// DROP. No geometry this engine can stand on: an AI / depth component
    /// (`Mask/Image`), a component nested somewhere this reader cannot account
    /// for, or no `crs:CorrectionMasks` at all.
    ///
    /// NARROWED TWICE, and the list it used to carry is worth stating
    /// correctly because two of its four members were never right:
    ///  * `Mask/Aggregate` and `Mask/Paint` LEFT in R27 Batch-4 — a brush
    ///    group is a first-class geometry now ([`MaskGeometry::Brush`]),
    ///    imported, carried and written back, with [`BrushRendered`] as its
    ///    note. It is not a drop and has not been one since.
    ///  * `Mask/Ellipse` was never a member in the first place. It is the
    ///    spot-healing shape and lives under `crs:RetouchAreas` ONLY — 84 of
    ///    84 instances in the reference library, never inside a correction —
    ///    so no correction has ever been refused for one.
    ///
    /// What genuinely lands here is `Mask/Image`: Lightroom stores the INTENT
    /// and recomputes the alpha from a model, so the sidecar holds no pixels
    /// and no geometry to take.
    ///
    /// [`BrushRendered`]: MaskImportReason::BrushRendered
    /// [`MaskGeometry::Brush`]: crate::recipe::MaskGeometry::Brush
    Unrepresentable,
    /// DROP. The values READ fine but land outside this engine's model (an
    /// exposure past ±5 EV, `crs:CorrectionActive="false"`, a component whose
    /// coordinates are not numbers, more masks than the recipe cap holds).
    OutOfModel,
    /// `crs:Angle` is non-zero and the radial still imports as its UNROTATED
    /// ellipse — mirrored from the export side's [`MaskLossReason::Rotation`].
    ///
    /// The REASON narrowed in v0.32.0 and this line moved with it: the sign,
    /// the pivot and the magnitude are all measured now (`ANGLE-MODEL.md`
    /// §6.1), and [`lr_to_engine`] carries the tilt through. The variant fires
    /// only when the DOCUMENT DECLARES NO FRAME — no `tiff:ImageWidth /
    /// ImageLength`, so the pixel→normalised fold has no aspect to fold with
    /// ([`FrameAspect`], [`RadialDecode::Unrotated`]) — or when the attribute
    /// is present but unreadable. The payload is the sidecar's angle in WHOLE
    /// DEGREES, or `0` when there was no angle to name (unreadable still
    /// counts as rotated: we cannot say it is zero), or it rounds away.
    Rotation(i32),
    /// `crs:MaskBlendMode` is not the plain composition we already do, so the
    /// component contributes its base geometry only.
    BlendMode,
    /// More than one geometry component: the base shape imports, the extra
    /// shapes do not (the import twin of [`MaskLossReason::ComponentsFlattened`]).
    ///
    /// PARAMETRIC shapes only. A brush group is imported as a real component
    /// since R27 Batch-4, so it is not one of the "extra shapes that do not"
    /// and does not raise this; [`BrushRendered`] speaks for it.
    ///
    /// [`BrushRendered`]: MaskImportReason::BrushRendered
    MultiComponent,
    /// A `Mask/Aggregate` brush group imported WHOLE — strokes, dab streams,
    /// group blend mode and all — and **drawn here by our own rasteriser**,
    /// from a measured model of Lightroom's brush rather than Adobe's code.
    ///
    /// R27 Batch-4 (L-08) made it importable. Before that, a correction holding
    /// one of these was refused entire under [`Unrepresentable`], which cost
    /// the photographer not only the brush but every gradient standing beside
    /// it: 18 corrections and 14 already-drawable parametric shapes across the
    /// reference library, thrown away because a neighbouring component was a
    /// brush.
    ///
    /// **R29 Batch-6b then made it RENDER, and this variant was renamed from
    /// `BrushCarried` because「carried, not drawn」stopped being true.** The one
    /// input a renderer needs that no sidecar contains is the alpha kernel
    /// (`recipe::BrushStroke::dabs`), so it was measured instead of guessed:
    /// 29 controlled Lightroom exports, `k(ρ;h) = (1 − ρ^m(h))^n(h)` held-out
    /// at rms 0.0109, `D(f) = κf/(1−f+κf)` with κ = 0.1284, screen
    /// accumulation, density scaling the dab (R29 Batch-6; `render::brush_raster`).
    ///
    /// Raised on the IMPORT itself, exactly like [`AiMaskRecomputed`]: the
    /// photographer is being told what KIND of thing arrived, and「the alpha
    /// will be ours, from a measurement」is true the moment the group is read.
    /// A note, not a drop — the correction and its neighbours arrive.
    ///
    /// [`Unrepresentable`]: MaskImportReason::Unrepresentable
    /// [`AiMaskRecomputed`]: MaskImportReason::AiMaskRecomputed
    BrushRendered,
    /// A `Mask/Image` AI mask imported as INTENT and **re-derived on this
    /// machine by a different segmenter** — the dominant refusal before R27
    /// Batch-5, and the one arm that cannot be closed by any parser.
    ///
    /// The current corpus measures 105 instances: the component carries `MaskSubType` +
    /// `ReferencePoint` + `MaskName`, optional gesture region hints, provenance
    /// digests, and the proxy frame Adobe's model ran in — **no raster payload**.
    /// So there is no alpha to import in the sense the other variants mean.
    /// What lands is a RECOMPUTATION: our own subject / sky / point-prompted
    /// segmenter produces its own alpha, which will differ from Adobe's at every
    /// edge; subtype-0 gesture dabs now join its positive point prompt.
    ///
    /// A note, not a drop, and that is the whole gain: 78 corrections across 40
    /// files — 40 % of every file in the reference library that has a mask at
    /// all — were refused entire because of one of these, taking 52
    /// engine-drawable parametric shapes with them.
    ///
    /// When the segmenter has not run or declined, the mask renders INERT and
    /// [`AiMaskUnresolved`] says so instead — two different sentences for two
    /// different states, because "approximated" and "not drawn" are not the
    /// same news.
    ///
    /// [`AiMaskUnresolved`]: MaskImportReason::AiMaskUnresolved
    AiMaskRecomputed,
    /// An AI mask arrived but has NO alpha: the segmentation sidecar has not
    /// run for this photo yet, or it ran and declined. The mask contributes
    /// nothing and the correction's other shapes render normally.
    ///
    /// Separate from [`AiMaskRecomputed`] on purpose. That one says "these
    /// pixels are ours, not Adobe's"; this one says "there are no pixels".
    /// Collapsing them would let a failed model run read as a successful
    /// approximation, which is the one confusion this whole arm exists to
    /// avoid.
    ///
    /// [`AiMaskRecomputed`]: MaskImportReason::AiMaskRecomputed
    AiMaskUnresolved,
    /// A `Mask/RangeMask` we cannot honour (someone else's encoding, or more
    /// than one) — the geometry imports without the range refinement.
    ForeignRangeMask,
    /// A `crs:MainCurve` / `RedCurve` / `GreenCurve` / `BlueCurve` is present
    /// and UNREADABLE: the element never closes, or one of its points is not
    /// an `x,y` pair inside 0..255. The correction imports WITHOUT that curve
    /// — the geometry is still exactly what the file draws, so this costs the
    /// curve and not the mask (the same verdict [`ForeignRangeMask`] gets).
    ///
    /// R25 P1 raised this for every correction that carried a local curve at
    /// all, because the engine modelled none of them and they were vanishing
    /// with no note. R25 P6 models all four (`LocalAdjustment::main_curve` …),
    /// so what remains is the narrower, still-real case above: a knob we do
    /// not model and a value we cannot read are different things (see this
    /// enum's header), and only the second one is a loss once the knob exists.
    ///
    /// [`ForeignRangeMask`]: MaskImportReason::ForeignRangeMask
    LocalCurve,
    /// `crs:LocalCurveRefineSaturation` is off its 100 default.
    CurveRefineSaturation,
    /// A `crs:Local*` slider this engine has no model for carries a non-zero
    /// value; the payload is that slider's key.
    InertLocal(&'static str),
    /// A `crs:Local*` attribute name this build has never seen.
    UnknownLocalKey,
}

impl MaskImportReason {
    /// Every reason, drops before notes — the ONE list both disclosure
    /// surfaces iterate ([`describe_import_losses`] and the GUI's
    /// `xmp_import_line`), exactly as [`MaskLossReason::ALL`] serves the
    /// export half. [`en`]'s exhaustive match stops the BUILD when a variant
    /// is added; an iteration array does not, so a reason raised by the
    /// reader and missing here would be counted by nobody. Pinned by
    /// `mask_import_reason_all_covers_every_variant`.
    ///
    /// [`InertLocal`] appears with an EMPTY payload: the payload names one
    /// slider, so no single value can stand for the variant. Grouping is by
    /// [`same_kind`] (the discriminant), never by `==`.
    ///
    /// [`en`]: MaskImportReason::en
    /// [`InertLocal`]: MaskImportReason::InertLocal
    /// [`same_kind`]: MaskImportReason::same_kind
    pub const ALL: [MaskImportReason; 13] = [
        MaskImportReason::Unrepresentable,
        MaskImportReason::OutOfModel,
        MaskImportReason::Rotation(0),
        MaskImportReason::BlendMode,
        MaskImportReason::MultiComponent,
        MaskImportReason::BrushRendered,
        MaskImportReason::AiMaskRecomputed,
        MaskImportReason::AiMaskUnresolved,
        MaskImportReason::ForeignRangeMask,
        MaskImportReason::LocalCurve,
        MaskImportReason::CurveRefineSaturation,
        MaskImportReason::InertLocal(""),
        MaskImportReason::UnknownLocalKey,
    ];

    /// Did this verdict cost the whole correction? The two `true` answers are
    /// what [`unsupported_corrections`] counts, so "imported + refused" stays
    /// the size of the user's local work (`eval`'s reading) even now that a
    /// correction can import AND carry notes.
    pub fn is_drop(self) -> bool {
        matches!(self, MaskImportReason::Unrepresentable | MaskImportReason::OutOfModel)
    }

    /// Same VARIANT, payload ignored — the grouping key both prose channels
    /// use, because `InertLocal("LocalGrain")` and `InertLocal("LocalMoire")`
    /// are one line in a sentence and two values under `==`.
    pub fn same_kind(self, other: MaskImportReason) -> bool {
        std::mem::discriminant(&self) == std::mem::discriminant(&other)
    }

    /// English label for the prose channel (CLI stderr / batch warnings). The
    /// GUI renders the same variants in the UI language instead.
    pub fn en(self) -> &'static str {
        match self {
            MaskImportReason::Unrepresentable => "AI / brush correction(s) skipped",
            MaskImportReason::OutOfModel => "correction(s) beyond this engine's model skipped",
            MaskImportReason::Rotation(_) => "radial rotation(s) read as 0",
            MaskImportReason::BlendMode => "non-default blend mode(s) ignored",
            MaskImportReason::MultiComponent => "extra shape component(s) dropped",
            MaskImportReason::BrushRendered => {
                "brush mask(s) drawn from AutoShade's measured model of Lightroom's brush - \
                 not Adobe's own rasteriser"
            }
            MaskImportReason::AiMaskRecomputed => {
                "AI mask(s) re-derived by the local segmenter - not Adobe's own raster"
            }
            MaskImportReason::AiMaskUnresolved => {
                "AI mask(s) carried but not yet re-derived - the local segmenter has not run"
            }
            MaskImportReason::ForeignRangeMask => "range mask(s) dropped",
            MaskImportReason::LocalCurve => "local point curve(s) unreadable",
            MaskImportReason::CurveRefineSaturation => "curve refine saturation not modelled",
            MaskImportReason::InertLocal(_) => "unmodelled local slider(s)",
            MaskImportReason::UnknownLocalKey => "unknown local setting(s)",
        }
    }
}

/// One import defect, NAMED. A single correction can appear more than once (a
/// rotated radial whose blend mode is Subtract loses two separate things) —
/// the import twin of [`MaskLoss`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskImportLoss {
    /// `crs:CorrectionName` when the sidecar sets one, else the positional
    /// `Correction <n>` — the same "say WHICH one" rule the export side's
    /// [`MaskLoss::name`] follows.
    pub name: String,
    pub reason: MaskImportReason,
}

/// What this sidecar's mask corrections cost on the way in, named — the
/// import-side counterpart of [`mask_export_losses`]. Empty = every
/// correction arrived whole (or there were none).
pub fn import_losses(xmp: &str) -> Vec<MaskImportLoss> {
    if xmp.len() > MAX_XMP_BYTES {
        return Vec::new();
    }
    let authored_by_autoshade = is_autoshade_sidecar(xmp);
    let scope = crs_own_scope(xmp);
    // The FRAME is read off the whole document — `tiff:` properties live
    // outside the crs Description's own scope in principle, and the decode
    // needs them (see `FrameAspect`).
    mask_summary(scope.as_ref(), authored_by_autoshade, FrameAspect::from_xmp(xmp)).losses
}

/// [`import_losses`] with the photo identity needed to resolve a sibling ACR
/// MaskBrushTable. The structured loss channel is already user-visible, so
/// this re-read is silent; the recipe import emits any named table refusal.
pub fn import_losses_for_photo(xmp: &str, photo: &std::path::Path) -> Vec<MaskImportLoss> {
    if xmp.len() > MAX_XMP_BYTES {
        return Vec::new();
    }
    let authored_by_autoshade = is_autoshade_sidecar(xmp);
    let scope = crs_own_scope(xmp);
    mask_summary_with_source(
        scope.as_ref(),
        authored_by_autoshade,
        FrameAspect::from_xmp(xmp),
        Some(photo),
        None,
    )
    .losses
}

/// Did this sidecar's document have Lightroom's LENS PROFILE CORRECTION
/// switched on? `None` when the document says nothing.
///
/// Read for exactly one purpose (R29 Batch-3): the mask-warp frame. The whole
/// difference between the frame a mask was STORED in and the frame it was
/// EXPORTED into is Lightroom's lens correction, so `crs:LensProfileEnable="0"`
/// means the two frames are the SAME and an identity warp is the right answer —
/// not a missing one. [`crate::recipe::MaskWarpSource::DisabledInSidecar`] is
/// where that distinction is recorded, and
/// [`crate::pipeline::fresh_lens_profile_for_sidecar`] is what applies it.
///
/// This does NOT touch what this engine renders. AutoShade's own geometry stage
/// is driven by the photographer's `lens_profile` toggles, which are theirs to
/// set; reading Lightroom's switch as an instruction would silently overwrite
/// them from a file they may have imported only for its masks.
///
/// Adobe writes `"0"` / `"1"`; `"False"` / `"True"` are accepted too because
/// the surrounding boolean keys in the same namespace use that spelling and a
/// reader that took only one of the two would be right by luck.
pub fn lens_profile_enabled(xmp: &str) -> Option<bool> {
    let scope = crs_own_scope(xmp);
    let raw = Scope::new(scope.as_ref()).crs_str("LensProfileEnable")?;
    match raw.trim() {
        "1" => Some(true),
        "0" => Some(false),
        s if s.eq_ignore_ascii_case("true") => Some(true),
        s if s.eq_ignore_ascii_case("false") => Some(false),
        // A value neither spelling covers is not a "no": saying nothing is
        // honest where guessing would decide a coordinate frame.
        _ => None,
    }
}

/// One English sentence for what an import carried and what it left behind, or
/// `None` when the sidecar's masks arrived whole. Groups by reason in
/// [`MaskImportReason::ALL`] order and names the corrections, so the line is
/// actionable ("which of my 12 masks?") — the import twin of
/// [`describe_mask_losses`].
///
/// BOTH FRONT-ENDS consume this (R27, closing the R25 P9 registration). The
/// GUI reads it through `bin/gui/export.rs` and `bin/gui/persist.rs`; the CLI
/// reads it through `main::lightroom_import_note`, which prints the sentence
/// on stderr beside the `xmp -> …` line of every single-photo command that
/// publishes a projection (`analyze`, `auto`, `match`). Until R27 the CLI had
/// no mask disclosure at all: the losses were computable and nobody computed
/// them.
///
/// STILL NARROWER, and named rather than left to be rediscovered: `eval.rs`
/// uses [`unsupported_corrections`], which counts DROPS only
/// ([`MaskImportReason::is_drop`]) and so cannot see a degradation like
/// `MultiComponent`. That is the eval RULER's own definition — "imported +
/// refused" has to stay the size of the user's local work — not a missing
/// channel, so it is a difference to know about, not a gap to close.
///
/// `batch` prints nothing here on purpose: its work list is
/// `store::has_develop_or_sidecar`-filtered, so a photo that HAS a Lightroom
/// sidecar is never in it.
pub fn describe_import_losses(imported: usize, losses: &[MaskImportLoss]) -> Option<String> {
    if losses.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for reason in MaskImportReason::ALL {
        let names: Vec<&str> = losses
            .iter()
            .filter(|l| l.reason.same_kind(reason))
            .map(|l| l.name.as_str())
            .collect();
        if names.is_empty() {
            continue;
        }
        // Capped like the export sentence, for the same reason: the count is
        // the fact, the first few names are the pointer.
        let shown = names.len().min(4);
        let more = names.len() - shown;
        let list = names[..shown].join(", ");
        let tail = if more > 0 { format!(", +{more} more") } else { String::new() };
        parts.push(format!("{} {} ({list}{tail})", names.len(), reason.en()));
    }
    Some(format!(
        "imported {imported} Lightroom mask(s); {} — the sidecar itself is not modified",
        parts.join("; ")
    ))
}

/// The masks `r` cannot project into a classic ACR sidecar, exactly as the
/// writer judges them while emitting the XML (see [`masks_xml`]) — the ONE
/// source for every surface's export-side disclosure. Empty = a faithful
/// projection.
///
/// For a caller that is also WRITING the document, this is the wrong door: it
/// builds the whole mask block to return the verdict list. Take the pair from
/// [`recipe_to_xmp_with_losses`] / [`MergeOutcome::losses`] instead (R22 NIT-1)
/// — `pipeline::write_xmp` did exactly this and ran the projection twice per
/// save. What is left here is the standalone question ("what would this recipe
/// lose?") with no document wanted; the crate's own tests are its only callers
/// today.
///
/// Judged with NO frame ([`FrameAspect`]), which is the honest answer to a
/// question asked without a photo: a rotated radial counts as a rotation loss
/// here, and the writer that IS given the frame does not lose it. The two
/// cannot disagree about a document, because this one produces none.
pub fn mask_export_losses(r: &EditRecipe) -> Vec<MaskLoss> {
    masks_xml(r, None).1
}

/// One English sentence naming what the sidecar left behind, or `None` when
/// nothing was lost. Groups by reason in [`MaskLossReason`] order and names
/// the masks, so the line is actionable ("which of my 12 masks?").
pub fn describe_mask_losses(losses: &[MaskLoss]) -> Option<String> {
    if losses.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for reason in MaskLossReason::ALL {
        let names: Vec<&str> = losses
            .iter()
            .filter(|l| l.reason.same_kind(reason))
            .map(|l| l.name.as_str())
            .collect();
        if names.is_empty() {
            continue;
        }
        // Names are already length-capped by `EditRecipe::clamp`, but a
        // 64-mask recipe would still make an unreadable line — the count is
        // the fact, the first few names are the pointer.
        let shown = names.len().min(4);
        let more = names.len() - shown;
        let list = names[..shown].join(", ");
        let tail = if more > 0 { format!(", +{more} more") } else { String::new() };
        parts.push(format!("{} {} ({list}{tail})", names.len(), reason.en()));
    }
    Some(format!(
        "the Lightroom XMP does not carry: {} — recipe.json keeps all of it",
        parts.join("; ")
    ))
}

/// **Export-side disclosure, GLOBAL half** (R24-5 M0): the develop controls
/// this recipe is CARRYING that the sidecar cannot express — an active
/// control the engine renders and `owned_attrs` has no `crs:` property for.
///
/// The mask half of this story has existed since M6a ([`MaskLoss`]); the
/// global half never did, so a photo whose look depends on its camera base
/// curve or its lens-profile correction exported a sidecar that renders
/// visibly differently in Lightroom, silently. Reopened in AutoShade it is
/// fine — recipe.json keeps everything — which is exactly why the loss went
/// unnoticed.
///
/// DERIVED, not listed: the members are the registry's `RenderedNotExported`
/// rows ([`crate::advisor::catalogue::Tier`]), and "is it active" is a serde
/// comparison against a default recipe — so moving a control between tiers
/// (the B2–B5 batches) updates this disclosure by itself, and a new
/// unexportable control cannot be added without appearing here.
pub fn global_export_losses(r: &EditRecipe) -> Vec<&'static str> {
    use crate::advisor::catalogue::{Tier, RECIPE_CONTROLS};
    let (Ok(live), Ok(neutral)) =
        (serde_json::to_value(r), serde_json::to_value(EditRecipe::default()))
    else {
        return Vec::new();
    };
    RECIPE_CONTROLS
        .iter()
        .filter(|c| c.tier == Some(Tier::RenderedNotExported))
        .filter(|c| live.get(c.name) != neutral.get(c.name))
        .map(|c| c.name)
        .collect()
}

/// **The MIRROR of [`global_export_losses`]** (R25 B4): the controls this
/// document carries that LIGHTROOM renders and this engine does not.
///
/// The disclosure story had one half. `RenderedNotExported` — we render it,
/// the sidecar cannot carry it — has been named since R24-5 M0. The opposite
/// corner had no surface at all, and R25's B2/B3 batches filled it with
/// twenty-four members: a photographer who imports a Lightroom grain, a
/// post-crop vignette or a colour-noise setting sees a canvas that is missing
/// them, and until now nothing on screen said so. `ARCHITECTURE.md` calls a
/// slider that moves a number and no pixel "the worst kind of bug here"; this
/// is the sentence that keeps the SF4-C whitelist from being exactly that.
///
/// DERIVED from the registry's `CarriedOnly` rows, so a control promoted to
/// `Rendered` leaves this disclosure by itself and a new carried one arrives
/// already named.
///
/// "Active" is measured against [`EditRecipe::default`], NEVER against zero.
/// The de-fringe block's neutral is Adobe's own 30/70/40/60 (R25 B3), so a
/// zero comparison would report every untouched photo as carrying a de-fringe
/// this app does not render — a disclosure that cries wolf on every save is a
/// disclosure nobody reads.
///
/// **`PassThrough` is deliberately NOT here**, though its tier renders nothing
/// either — and the reason is the tier's own definition rather than an
/// oversight. Every other row on this list has a KNOWN neutral, so "active"
/// is a decidable question; a pass-through value is never interpreted, which
/// means we cannot tell `crs:PerspectiveUpright="0"` (a block Lightroom
/// stamps on every file it touches, changing nothing anywhere) from a real
/// Upright correction. Including it would put this sentence on essentially
/// every Lightroom photo, permanently and unactionably, and drown the members
/// that ARE actionable — the same judgement `xmp_loss_interrupts` makes about
/// the base curve. The pass-through disclosure is its own develop-panel
/// section, which shows the sixteen values themselves and says we never read
/// them. Pinned in both directions by
/// `the_render_gaps_name_what_lightroom_renders_and_this_engine_does_not`.
pub fn global_render_gaps(r: &EditRecipe) -> Vec<&'static str> {
    use crate::advisor::catalogue::{Tier, RECIPE_CONTROLS};
    let (Ok(live), Ok(neutral)) =
        (serde_json::to_value(r), serde_json::to_value(EditRecipe::default()))
    else {
        return Vec::new();
    };
    RECIPE_CONTROLS
        .iter()
        .filter(|c| c.tier == Some(Tier::CarriedOnly))
        .filter(|c| live.get(c.name) != neutral.get(c.name))
        .map(|c| c.name)
        .collect()
}

/// **Import-side disclosure, GLOBAL half** (R24-5 M0): the `crs:` properties
/// this sidecar carries on its own `rdf:Description` that AutoShade does not
/// model at all — PointColor, the camera Look, `CameraProfileDigest`,
/// `UprightTransform`. (Global Texture and Grain headed that list until R25
/// B2 modelled them, the whole Defringe block left it in B3, and the
/// Transform / Calibration blocks left in B4 as [`PASSTHROUGH_CRS`] — the
/// list SHRINKING by itself, see the paragraph
/// on the complement below, and the fixture note in
/// `an_imported_sidecar_names_the_globals_the_engine_does_not_render`.)
///
/// The merge PRESERVES all of them (that is what `graft_into` is for), so
/// nothing is destroyed; what was missing is the sentence saying they exist.
/// Until now the only import-side global check was
/// [`unparsable_crs_numbers`], whose universe IS [`owned_attr_keys`] — so a
/// perfectly valid property we simply do not render was invisible to every
/// disclosure surface, and the photo just looked different from Lightroom's
/// render with no explanation on screen.
///
/// The universe is the COMPLEMENT of what we own, so it needs no catalogue of
/// Adobe's property names to rot: the day a batch teaches the engine
/// `crs:Texture`, the key joins `owned_attr_keys` and leaves this list. That
/// day was R25 B2, and it happened with no edit to this function — only to
/// the tests, whose fixtures had used Texture as their unmodelled sample.
///
/// BOTH SPELLINGS, because Lightroom really writes both: the Description's own
/// open-tag ATTRIBUTES, and its top-level PROPERTY-ELEMENT children
/// (`<crs:Texture>+30</crs:Texture>` — the form [`crs_str`] reads and
/// [`merge_recipe_into_xmp`]'s element strip removes, "in plenty of real
/// sidecars"). An attribute-only scan answered EMPTY for an element-form
/// sidecar, which is the shape of a Lightroom catalog export.
///
/// Only THIS Description's own attributes and own top-level children are
/// scanned — mask corrections live inside a child element and have their own
/// disclosure ([`unsupported_corrections`]), a creative Look nests someone
/// else's settings block, and mixing either in would report every
/// `crs:Local*` / baked profile key as an unmodelled global. Quote-aware: a
/// mask name or a `crs:RawFileName` value may contain anything, `crs:Foo=`
/// included.
pub fn unmodelled_global_crs(xmp: &str) -> Vec<String> {
    if xmp.len() > MAX_XMP_BYTES {
        return Vec::new();
    }
    let Some(start) = find_crs_description(xmp) else { return Vec::new() };
    let Some((gt, self_closing)) = scan_tag_end(xmp, start) else { return Vec::new() };
    let tag = &xmp[start..=gt];
    // The universe is the complement of what we OWN, and ownership has two
    // halves: the attribute keys the writer emits and the element-only
    // properties that have no attribute spelling at all
    // ([`OWNED_ELEMENT_ONLY`]). Leaving the second half out would have this
    // disclosure name our own tone curves as Lightroom-only properties.
    let owned: std::collections::BTreeSet<String> = owned_attr_keys()
        .into_iter()
        .chain(OWNED_ELEMENT_ONLY.iter().map(|k| (*k).to_string()))
        .collect();
    let mut found: std::collections::BTreeSet<String> = Default::default();
    let b: Vec<char> = tag.chars().collect();
    let mut quote: Option<char> = None;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
                i += 1;
            }
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                i += 1;
            }
            None => {
                // `crs:` must start a NAME, i.e. not be preceded by an
                // identifier character (`xcrs:Foo` is a different prefix).
                let name_char = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | '.');
                let starts = b[i..].starts_with(&['c', 'r', 's', ':'])
                    && (i == 0 || !name_char(b[i - 1]));
                if starts {
                    let mut j = i + 4;
                    // `-` and `.` are legal XML name characters (NCName). No
                    // Adobe key uses either TODAY, so this changes no current
                    // output — it is here so a future `crs:Foo-Bar` is named
                    // in full instead of being reported as `Foo`.
                    while j < b.len() && (b[j].is_ascii_alphanumeric() || matches!(b[j], '_' | '-' | '.')) {
                        j += 1;
                    }
                    let name: String = b[i + 4..j].iter().collect();
                    // An ATTRIBUTE, so `=` (possibly spaced) must follow.
                    let mut k = j;
                    while k < b.len() && b[k] == ' ' {
                        k += 1;
                    }
                    if !name.is_empty() && b.get(k) == Some(&'=') && !owned.contains(&name) {
                        found.insert(name);
                    }
                    i = j.max(i + 1);
                } else {
                    i += 1;
                }
            }
        }
    }

    // The ELEMENT half, walked with the same primitives `crs_str` uses
    // (`next_xml_tag` + `element_close_start`) rather than a second XML
    // scanner. Every non-self-closing child is JUMPED OVER whole, so nothing
    // nested inside one is ever read as this Description's own property.
    if !self_closing
        && let Some(close) = find_matching_close(xmp, gt + 1)
    {
        let mut from = gt + 1;
        while let Some((s, e, child_self_closing)) = next_xml_tag(xmp, from) {
            if s >= close {
                break; // past this Description's own body
            }
            let child = &xmp[s..=e];
            if child.starts_with("</") {
                // Only reachable on markup this walk cannot account for (the
                // opens above are all skipped past their own close).
                from = e + 1;
                continue;
            }
            let name = tag_name(child);
            if let Some(bare) = name.strip_prefix("crs:")
                && !bare.is_empty()
                && !owned.contains(bare)
            {
                found.insert(bare.to_string());
            }
            from = if child_self_closing {
                e + 1
            } else {
                match element_close_start(xmp, name, e).and_then(|c| scan_tag_end(xmp, c)) {
                    Some((close_gt, _)) => close_gt + 1,
                    // A child that never closes: the rest of the body is not
                    // accountable, and guessing would report a mask's own
                    // items as globals.
                    None => break,
                }
            };
        }
    }
    found.into_iter().collect()
}

/// **The PASS-THROUGH key set** (R25 B4): Lightroom's Transform (Upright /
/// Perspective) and Camera Calibration blocks, carried verbatim between the
/// sidecar and `EditRecipe::passthrough` and NEVER interpreted — the first
/// real payload `Tier::PassThrough` has ever had.
///
/// A NAMED SET, not "everything unknown", and that is the whole design.
/// [`owned_attr_keys`] is a static list and it is the merge's REMOVAL
/// universe: a free-form map would emit keys the merge never strips, so our
/// value would land beside Lightroom's original as a duplicate attribute.
/// Sixteen keys we can name are sixteen keys the strip can name too.
///
/// The complement is not abandoned — it is DISCLOSED. `crs:Look` (a nested
/// element, not a string), `crs:PointColor`, `crs:CameraProfileDigest`,
/// `crs:UprightTransform` and the rest stay outside this list, are preserved
/// by the merge exactly as before, and go on being named by
/// [`unmodelled_global_crs`]. That is the feature, not the omission.
///
/// FIRST-HAND (all seven reference sidecars): every Perspective key is present
/// on every file, `crs:PerspectiveScale="100"` and `crs:PerspectiveX="0.00"`
/// are the resting spellings, one file carries `crs:PerspectiveVertical="-35"`
/// and `crs:PerspectiveRotate="+0.9"` — three different numeric spellings for
/// the same neutral, which is precisely why these are strings. The
/// Calibration keys appear on none of them (Lightroom omits the block at its
/// defaults) and `crs:CameraProfile="Adobe Standard"` on all seven — a NAME,
/// with a space in it, so the map is `String → String` and not a number map.
pub const PASSTHROUGH_CRS: [&str; 16] = [
    // Transform / Upright (8)
    "PerspectiveUpright",
    "PerspectiveVertical",
    "PerspectiveHorizontal",
    "PerspectiveRotate",
    "PerspectiveScale",
    "PerspectiveAspect",
    "PerspectiveX",
    "PerspectiveY",
    // Camera Calibration (8)
    "CameraProfile",
    "CameraCalibrationRedHue",
    "CameraCalibrationRedSaturation",
    "CameraCalibrationGreenHue",
    "CameraCalibrationGreenSaturation",
    "CameraCalibrationBlueHue",
    "CameraCalibrationBlueSaturation",
    "CameraCalibrationShadowTint",
];

/// The `crs:` properties this writer owns that have NO attribute spelling —
/// the four tone curves and the mask block, which reach the sidecar only as
/// child elements. [`owned_attr_keys`] is the ATTRIBUTE writer's universe and
/// names none of them, so every scanner asking "is this ELEMENT ours?" must
/// union the two lists (the merge's own element strip builds exactly this
/// union — see [`merge_recipe_into_xmp`]).
pub(crate) const OWNED_ELEMENT_ONLY: [&str; 5] = [
    "ToneCurvePV2012",
    "ToneCurvePV2012Red",
    "ToneCurvePV2012Green",
    "ToneCurvePV2012Blue",
    "MaskGroupBasedCorrections",
];

/// One of a Correction's four LOCAL point curves (R25 P6), or the empty string
/// when that curve is unset — Lightroom writes only the curves that exist.
///
/// **This is deliberately NOT the global `curve_elem`** in [`owned_children`],
/// and the difference is not cosmetic. Three things separate the two spellings,
/// all read off the user's own sidecars (`DSC09642`, `DSC08761`):
///
///   * the KEY is the bare name — `crs:MainCurve` / `RedCurve` / `GreenCurve` /
///     `BlueCurve`, with no `PV2012` suffix, where the globals are
///     `crs:ToneCurvePV2012{,Red,Green,Blue}`;
///   * the POINT payload is `x,y` with **no space after the comma**, where the
///     globals are `x, y` **with** one;
///   * the ELEMENT nests two levels deeper (inside the correction's
///     `rdf:Description`, not the sidecar's), so the indentation differs.
///
/// Sharing one formatter and parameterising the separator would put the two
/// formats one careless "let's make this consistent" edit apart, which is the
/// whole reason this is a second function with this paragraph on it.
/// `local_curve_serialization_has_no_space_after_the_comma` fails the moment
/// they converge.
///
/// The leading spaces follow the reference sidecar's own nesting. They survive
/// into the output where the correction's ATTRIBUTE block's do not — that
/// literal is written with `\` line continuations, which eat the newline and
/// the indent after it. Whitespace is insignificant here either way; this is
/// the spelling Lightroom writes, so it is the one to match.
fn local_curve_elem(tag: &str, points: &[crate::recipe::CurvePoint]) -> String {
    if points.is_empty() {
        return String::new();
    }
    let pts: String = points
        .iter()
        .map(|p| format!("         <rdf:li>{},{}</rdf:li>\n", p.input, p.output))
        .collect();
    format!("       <crs:{tag}>\n        <rdf:Seq>\n{pts}        </rdf:Seq>\n       </crs:{tag}>\n")
}

/// Build the `<crs:MaskGroupBasedCorrections>` child element (empty string when
/// there are no masks) PLUS the per-mask loss list the export-side disclosure
/// is built from — one loop, so the XML and the claim about it cannot drift.
/// Local sliders convert UI scale → ACR local scale:
/// exposure stops ÷4, every other slider ÷100 (verified against the user's real
/// sidecar; see docs/V2_PLAN.md §2a). All 26 `Local*` fields are emitted (the
/// ones this engine has no model for as 0) as Lightroom expects the full block
/// — `LocalHue` and `LocalSharpness` joined the carried set in R23-1b.
fn masks_xml(r: &EditRecipe, frame: Option<FrameAspect>) -> (String, Vec<MaskLoss>) {
    let mut losses: Vec<MaskLoss> = Vec::new();
    if r.masks.is_empty() {
        return (String::new(), losses);
    }
    let mut items = String::new();
    for (i, m) in r.masks.iter().enumerate() {
        // The name goes first: it identifies this mask in the loss list even
        // on the arms that never reach the emit below.
        let name = if m.name.is_empty() { format!("AutoShade {}", i + 1) } else { m.name.clone() };
        // The eye toggle: a disabled mask applies nothing, so projecting it
        // as an active correction would make Lightroom render an edit the
        // app does not. Skipped like a Bitmap mask (lossy projection —
        // recipe.json keeps it; re-enable is one click). The alternative,
        // crs:CorrectionActive="false", is unverified against a real
        // sidecar, and the writer's "true" above is a fixed literal.
        //
        // ONE skip verdict per mask, in control-flow order: a muted bitmap
        // mask is skipped BECAUSE it is muted, and claiming both would
        // inflate every count the disclosure prints.
        if !m.enabled {
            losses.push(MaskLoss { name, reason: MaskLossReason::Disabled });
            continue;
        }
        let corr_id = guid(&format!("corr-{i}-{name}"));
        let mask_id = guid(&format!("mask-{i}-{name}"));
        // The single inversion bit both attributes below carry (R25 P9) — the
        // geometry's `crs:Flipped` is its complement, the correction's
        // `crs:MaskInverted` is it. Computed ONCE so the two can never drift.
        let net_inv = lr_net_inverted(m);
        // THREE shapes for the base component now, not two (R27 Batch-4):
        //  * a brush group emits its whole `Mask/Aggregate` element;
        //  * a parametric shape emits the attribute-form `<rdf:li/>` below;
        //  * a raster (bitmap) mask has no classic-XMP encoding at all — skip
        //    this correction; the deterministic render still applies it
        //    (§A tradeoff).
        let mut withheld: Option<f64> = None;
        let base_li = if matches!(m.mask, MaskGeometry::Brush { .. }) {
            match brush_mask_xml(&m.mask, &guid(&format!("brush-{i}-{name}"))) {
                Some(li) => li,
                None => unreachable!("brush_mask_xml answers every Brush geometry"),
            }
        // FOUR shapes now (R27 Batch-5): an AI mask emits its whole
        // `Mask/Image` element, carrying the intent Lightroom will recompute
        // from.
        } else if matches!(m.mask, MaskGeometry::AiMask { .. }) {
            match ai_mask_xml(&m.mask, &guid(&format!("ai-{i}-{name}"))) {
                Some(li) => li,
                None => unreachable!("ai_mask_xml answers every AiMask geometry"),
            }
        } else {
            let Some((what, geom, w)) = mask_geom_xml(&m.mask, net_inv, frame) else {
                losses.push(MaskLoss { name, reason: MaskLossReason::Bitmap });
                continue;
            };
            withheld = w;
            format!(
                "         <rdf:li crs:What=\"{what}\" crs:MaskActive=\"true\" crs:MaskName=\"{mname}\"\n\
          crs:MaskBlendMode=\"0\" crs:MaskInverted=\"{inv}\" crs:MaskSyncID=\"{mask_id}\"\n\
          crs:MaskValue=\"1\"{geom}/>\n",
                mname = xml_attr_escape(&format!("{name} mask")),
                // NOT `m.inverted`: a radial's flip is the other half of the
                // same bit (R25 P9, `lr_net_inverted`). Leaving the flip out
                // here is what silently dropped it on export — Lightroom has
                // one inversion flag per correction and this is it.
                inv = net_inv,
            )
        };
        // Extra components. A BRUSH one is emitted in full beside the base —
        // the whole point of L-08 is that a brush mask round-trips — so only
        // the others are flattened, and `ComponentsFlattened` now counts what
        // the projection really drops instead of "this mask had components".
        let mut extra_lis = String::new();
        let mut flattened = 0usize;
        for (k, c) in m.components.iter().enumerate() {
            let li = brush_mask_xml(&c.geometry, &guid(&format!("brush-{i}-{name}-{k}")))
                .or_else(|| ai_mask_xml(&c.geometry, &guid(&format!("ai-{i}-{name}-{k}"))));
            match li {
                Some(li) => extra_lis.push_str(&li),
                None => flattened += 1,
            }
        }
        // DEGRADATIONS (the correction IS emitted, just not whole) — each is
        // read off the same field the emitter above ignores, so adding a
        // projection later deletes its entry here in the same edit.
        if flattened > 0 {
            losses.push(MaskLoss {
                name: name.clone(),
                reason: MaskLossReason::ComponentsFlattened,
            });
        }
        // The other direction of loss: the sidecar gets the brush WHOLE and the
        // pixels this engine showed for it came from OUR rasteriser, working
        // from a measured model of Lightroom's brush (R29 Batch-6b). Raised
        // once per mask that carries one anywhere — base or component —
        // because it is a fact about the mask, not about how many groups it
        // holds.
        if matches!(m.mask, MaskGeometry::Brush { .. })
            || m.components.iter().any(|c| matches!(c.geometry, MaskGeometry::Brush { .. }))
        {
            losses.push(MaskLoss { name: name.clone(), reason: MaskLossReason::BrushRendered });
        }
        // The AI mask's own direction of loss, and it is the sharper one: the
        // sidecar gets the INTENT whole (so Lightroom will reproduce its own
        // mask exactly), while the pixels AutoShade showed came from OUR
        // segmenter. The two will not agree at the edges and can disagree
        // badly on a hard scene. Raised once per mask that carries one
        // anywhere, base or component — a fact about the mask, not a count.
        if matches!(m.mask, MaskGeometry::AiMask { .. })
            || m.components.iter().any(|c| matches!(c.geometry, MaskGeometry::AiMask { .. }))
        {
            losses
                .push(MaskLoss { name: name.clone(), reason: MaskLossReason::AiMaskRecomputed });
        }
        // The rotation verdict now comes FROM THE EMITTER (v0.32.0): it is the
        // one place that knows whether the angle reached the document, and
        // re-deriving it here from `angle != 0.0` is exactly how the writer and
        // its own disclosure would drift once the projection started carrying
        // the tilt. `as i32` saturates, so a corrupt angle degrades to 0 ("no
        // angle worth naming") rather than wrapping into a number that is not
        // the mask's.
        if let Some(deg) = withheld {
            let reason = MaskLossReason::Rotation(deg.round() as i32);
            losses.push(MaskLoss { name: name.clone(), reason });
        }
        // Neutral gains change nothing, so they are no loss — the same
        // is-it-actually-doing-anything test `render::engine_active` applies
        // (and what `EditRecipe::clamp` collapses to `None` anyway).
        if m.color_gains.is_some_and(|g| g != [1.0, 1.0, 1.0]) {
            losses.push(MaskLoss { name: name.clone(), reason: MaskLossReason::Recolour });
        }
        // R25 P6. Position is Lightroom's own and verified on two of the
        // user's sidecars: AFTER the attribute block closes at
        // `crs:LocalCurveRefineSaturation`, BEFORE `<crs:CorrectionMasks>`.
        // Sparse — an unset curve contributes nothing, so a mask that only
        // carries Red writes only `<crs:RedCurve>`. Built HERE rather than as
        // an argument below: a `format!` inside another `format!`'s arguments
        // is what `clippy::format_in_format_args` refuses.
        let curves = format!(
            "{}{}{}{}",
            local_curve_elem("MainCurve", &m.main_curve),
            local_curve_elem("RedCurve", &m.red_curve),
            local_curve_elem("GreenCurve", &m.green_curve),
            local_curve_elem("BlueCurve", &m.blue_curve),
        );
        items.push_str(&format!(
            "     <rdf:li>\n\
      <rdf:Description\n\
       crs:What=\"Correction\" crs:CorrectionAmount=\"{amount}\" crs:CorrectionActive=\"true\"\n\
       crs:CorrectionName=\"{name}\" crs:CorrectionSyncID=\"{corr_id}\"\n\
       crs:LocalExposure=\"0\" crs:LocalHue=\"{hue}\" crs:LocalSaturation=\"{sat}\"\n\
       crs:LocalContrast=\"0\" crs:LocalClarity=\"0\" crs:LocalSharpness=\"{sharp}\"\n\
       crs:LocalBrightness=\"0\" crs:LocalToningHue=\"0\" crs:LocalToningSaturation=\"0\"\n\
       crs:LocalExposure2012=\"{exp}\" crs:LocalContrast2012=\"{con}\"\n\
       crs:LocalHighlights2012=\"{hi}\" crs:LocalShadows2012=\"{sh}\"\n\
       crs:LocalWhites2012=\"{wh}\" crs:LocalBlacks2012=\"{bl}\"\n\
       crs:LocalClarity2012=\"{cl}\" crs:LocalDehaze=\"{dh}\" crs:LocalLuminanceNoise=\"{nr}\"\n\
       crs:LocalMoire=\"0\" crs:LocalDefringe=\"0\" crs:LocalTemperature=\"{temp}\"\n\
       crs:LocalTint=\"{tint}\" crs:LocalTexture=\"{tex}\" crs:LocalGrain=\"0\"\n\
       crs:LocalCurveRefineSaturation=\"100\">\n\
{curves}\
       <crs:CorrectionMasks>\n\
        <rdf:Seq>\n\
{base_li}{extra_lis}{range}\
        </rdf:Seq>\n\
       </crs:CorrectionMasks>\n\
      </rdf:Description>\n\
     </rdf:li>\n",
            range = range_mask_xml(&m.range, &guid(&format!("range-{i}-{name}"))),
            curves = curves,
            amount = local_fmt(m.amount),
            name = xml_attr_escape(&name),


            corr_id = corr_id,
            sat = local_fmt(m.saturation / 100.0),
            // R23-1b: two keys the writer emitted as a literal "0" from the
            // first sidecar on. They take the writer's UNIVERSAL local scale
            // (slider ÷ 100 — the one every non-2012 key here uses, verified on
            // the user's own sidecar for `LocalSaturation`/`LocalTexture`), and
            // `parse_one_correction` reads them back through the same ×100, so
            // OUR round-trip is exact. What no reference sidecar in this repo
            // can settle is Lightroom's own numeric scale for these two: every
            // sample carries "0" (docs/V2_PLAN.md §2a's reference block
            // included), so the ÷100 rests on the family pattern, not on a
            // measured non-zero. Both are re-rendered by Lightroom from its own
            // model in any case, like `manual_vignette_lut` and local texture.
            // ÷180, not ÷100 — see the `q180` read in `parse_one_correction`
            // for the measurement. `sharpness` keeps the family scale: its own
            // Lightroom magnitude is still unmeasured (docs/V2_PLAN.md §7
            // item 10), so it stays where the pattern puts it.
            hue = local_fmt(m.hue / 180.0),
            sharp = local_fmt(m.sharpness / 100.0),
            exp = local_fmt(m.exposure_ev / 4.0),
            con = local_fmt(m.contrast / 100.0),
            hi = local_fmt(m.highlights / 100.0),
            sh = local_fmt(m.shadows / 100.0),
            wh = local_fmt(m.whites / 100.0),
            bl = local_fmt(m.blacks / 100.0),
            cl = local_fmt(m.clarity / 100.0),
            dh = local_fmt(m.dehaze / 100.0),
            temp = local_fmt(m.temperature / 100.0),
            tint = local_fmt(m.tint / 100.0),
            tex = local_fmt(m.texture / 100.0),
            nr = local_fmt(m.noise_reduction / 100.0),
            base_li = base_li,
            extra_lis = extra_lis,
        ));
    }
    // All masks may have been raster-skipped — no empty wrapper block then.
    if items.is_empty() {
        return (String::new(), losses);
    }
    (
        format!(
            "\n   <crs:MaskGroupBasedCorrections>\n    <rdf:Seq>\n{items}    </rdf:Seq>\n   </crs:MaskGroupBasedCorrections>"
        ),
        losses,
    )
}

/// Every crs ATTRIBUTE the writer owns, rendered for `r` — the
/// `\n    crs:K="v"` block. One authority for what AutoShade owns in a
/// sidecar, shared by the fresh-document writer and the merge path
/// ([`merge_recipe_into_xmp`]); the REMOVAL universe lives in
/// [`owned_attr_keys`] and must cover every key this can ever emit.
/// Does a detail/NR COMPANION key ride out at zero because its group's AMOUNT
/// is set? (R27 T4, `P2-feather-k-closures.md` §4.)
///
/// Lightroom's rule is **amount-gated, not per-key** — over 211 Adobe exports
/// carrying `crs:LuminanceSmoothing`, all 4 with it > 0 carry the Detail
/// companion and none of the 207 with it = 0 does; 133/133 with
/// `ColorNoiseReduction > 0` carry Detail + Smoothness and 0 of 64 with it = 0
/// do; `SharpenEdgeMasking="0"` rides on 159 files whose `Sharpness` is set.
/// This writer gated each companion on ITS OWN value, so a recipe with
/// `LuminanceSmoothing="50"` and a contrast of 0 emitted the amount and the
/// Detail but DROPPED `LuminanceNoiseReductionContrast` — a shape no Lightroom
/// file has.
///
/// **Only the companions whose ACR default is ZERO join**, and that is the
/// whole of the alignment. `LuminanceNoiseReductionContrast` and
/// `SharpenEdgeMasking` default to 0 in ACR, so emitting `"0"` says exactly
/// what their absence said and the change is byte-level only. The others
/// (`SharpenRadius` 1.0, `SharpenDetail` 25, the two Detail/Smoothness 50s)
/// default to a NON-zero value, and this recipe's 0 means "never learned one"
/// — emitting it would tell Lightroom to sharpen at radius 0 or drop detail
/// retention from 50 to 0, which is a render change, not a spelling. Those
/// keep reaching Lightroom by ABSENCE, which is the honest encoding and the
/// rule `owned_attrs`' vignette/grain block states for the same reason.
fn amount_carries(key: &str, amount: f32) -> bool {
    matches!(key, "LuminanceNoiseReductionContrast" | "SharpenEdgeMasking") && amount != 0.0
}

fn owned_attrs(r: &EditRecipe, frame: Option<FrameAspect>) -> String {
    let mut a = String::new();
    // The SCHEMA-ERA gate's emission half (R25 P8). It has to sit beside the
    // strip half in [`merge_strip_keys`] and agree with it exactly: strip
    // without emit deletes the base's value, emit without strip leaves two
    // copies of the same attribute in one tag. `era_suppressed_attr_keys`
    // returns the ONE set both consult — empty for every current-era recipe,
    // so this costs nothing on the ordinary path.
    let era_gated = era_suppressed_attr_keys(r);
    let ungated = |key: &str| !era_gated.contains(key);

    // ProcessVersion 15.4 / Version 15.5.1 are the verified current values from
    // the user's real sidecar (not the research's guessed 11.0/15.0).
    attr(&mut a, "Version", "15.5.1");
    attr(&mut a, "ProcessVersion", "15.4");

    // White balance: an explicit temperature means Custom WB — and
    // Temperature is ABSOLUTE Kelvin in both models now that the engine
    // anchors at the stamped as-shot (`as_shot_k`), so the number finally
    // means the same thing to Lightroom. A tint-only edit on a STAMPED photo
    // emits Custom pinned AT the as-shot Kelvin (exactly what Lightroom
    // itself writes for a tint-only move): under "As Shot" Lightroom may
    // re-read camera metadata and ignore the Tint attribute entirely — the
    // old documented lossy edge, closed wherever the engine knows the K.
    // A LEGACY recipe (no stamp) keeps the old honest fallback: "As Shot"
    // plus the tint, still disclosed as lossy; recipe.json carries the tint
    // losslessly either way.
    let wb_kelvin = r.temperature_k.or(if r.tint != 0.0 { r.as_shot_k } else { None });
    if let Some(k) = wb_kelvin {
        attr(&mut a, "WhiteBalance", "Custom");
        attr(&mut a, "Temperature", &(k.round() as i64).to_string());
        attr(&mut a, "Tint", &signed(r.tint));
    } else {
        attr(&mut a, "WhiteBalance", "As Shot");
        if r.tint != 0.0 {
            attr(&mut a, "Tint", &signed(r.tint));
        }
    }

    // Exposure as a plain decimal (Lightroom parses signed or unsigned).
    attr(&mut a, "Exposure2012", &format!("{:.2}", r.exposure_ev));
    attr(&mut a, "Contrast2012", &signed(r.contrast));
    attr(&mut a, "Highlights2012", &signed(r.highlights));
    attr(&mut a, "Shadows2012", &signed(r.shadows));
    attr(&mut a, "Whites2012", &signed(r.whites));
    attr(&mut a, "Blacks2012", &signed(r.blacks));
    attr(&mut a, "Clarity2012", &signed(r.clarity));
    attr(&mut a, "Dehaze", &signed(r.dehaze));
    attr(&mut a, "Vibrance", &signed(r.vibrance));
    attr(&mut a, "Saturation", &signed(r.saturation));
    // Global Texture (R25 B2) — the last Basic-panel slider that had no key.
    // UNCONDITIONAL like its four neighbours above: Lightroom writes
    // `crs:Texture="+26"` in the signed form (verified in the user's library),
    // and a recipe that says 0 is stating a value, not omitting one — UNLESS
    // it is an era-0 recipe that never had the field at all, which is the one
    // case where 0 states nothing (see the gate above).
    if ungated("Texture") {
        attr(&mut a, "Texture", &signed(r.texture));
    }

    // Per-colour HSL / Color mixer (8 ACR bands). Emit only when non-neutral so
    // a plain global recipe still produces a minimal, v1-compatible sidecar.
    if !r.hsl.is_neutral() {
        for (i, band) in crate::recipe::HSL_BANDS.iter().enumerate() {
            attr(&mut a, &format!("HueAdjustment{band}"), &signed(r.hsl.hue[i]));
            attr(&mut a, &format!("SaturationAdjustment{band}"), &signed(r.hsl.saturation[i]));
            attr(&mut a, &format!("LuminanceAdjustment{band}"), &signed(r.hsl.luminance[i]));
        }
    }

    // Colour grading (3-wheel + global). ACR convention VERIFIED against the
    // user's own sidecar: shadow/highlight hue+sat round-trip via the legacy
    // SplitToning* keys; lum, midtone, global, blending via ColorGrade*; balance
    // via SplitToningBalance. Hue/sat/blending are unsigned, lum/balance signed.
    if !r.color_grade.is_neutral() {
        let cg = &r.color_grade;
        let uns = |v: f32| (v.round() as i64).to_string();
        attr(&mut a, "SplitToningShadowHue", &uns(cg.shadow_hue));
        attr(&mut a, "SplitToningShadowSaturation", &uns(cg.shadow_sat));
        attr(&mut a, "SplitToningHighlightHue", &uns(cg.highlight_hue));
        attr(&mut a, "SplitToningHighlightSaturation", &uns(cg.highlight_sat));
        attr(&mut a, "SplitToningBalance", &signed(cg.balance));
        attr(&mut a, "ColorGradeShadowLum", &signed(cg.shadow_lum));
        attr(&mut a, "ColorGradeMidtoneHue", &uns(cg.midtone_hue));
        attr(&mut a, "ColorGradeMidtoneSat", &uns(cg.midtone_sat));
        attr(&mut a, "ColorGradeMidtoneLum", &signed(cg.midtone_lum));
        attr(&mut a, "ColorGradeHighlightLum", &signed(cg.highlight_lum));
        attr(&mut a, "ColorGradeGlobalHue", &uns(cg.global_hue));
        attr(&mut a, "ColorGradeGlobalSat", &uns(cg.global_sat));
        attr(&mut a, "ColorGradeGlobalLum", &signed(cg.global_lum));
        attr(&mut a, "ColorGradeBlending", &uns(cg.blending));
    }

    // 1:1 with the Detail > Sharpening "Amount" slider, whose UI maximum IS
    // 150 — see the reader for the evidence that retired the old ×⅔ rescale.
    // Round + clamp to the same band the recipe row states, no scale change.
    let sharp = (r.sharpening.round() as i64).clamp(0, 150);
    attr(&mut a, "Sharpness", &sharp.to_string());
    // SharpenRadius is the one DECIMAL key in the detail block, and the one
    // Lightroom writes with an explicit `+`: `crs:SharpenRadius="+1.0"` in all
    // seven of the user's sidecars (the integer neighbours are bare —
    // `SharpenDetail="25"`, `SharpenEdgeMasking="0"`). Emitted only when set,
    // so an absent radius stays absent and Lightroom keeps its own 1.0.
    if r.sharpen_radius != 0.0 && ungated("SharpenRadius") {
        attr(&mut a, "SharpenRadius", &format!("{:+.1}", r.sharpen_radius));
    }
    for (key, v, amount) in [
        ("SharpenDetail", r.sharpen_detail, r.sharpening),
        ("SharpenEdgeMasking", r.sharpen_mask, r.sharpening),
    ] {
        if (v != 0.0 || amount_carries(key, amount)) && ungated(key) {
            attr(&mut a, key, &(v.round() as i64).to_string());
        }
    }
    let nr = (r.noise_reduction.round() as i64).clamp(0, 100);
    attr(&mut a, "LuminanceSmoothing", &nr.to_string());
    // The rest of the R25 B3 carried detail axes, in Lightroom's own key
    // order (verified against the user's sidecars: Sharpness, SharpenRadius,
    // SharpenDetail, SharpenEdgeMasking, LuminanceSmoothing, then the colour
    // NR trio). Same per-key "only when non-zero" rule as the B2 effects —
    // and the same reason: zero here means "the sidecar said nothing", and
    // the companions Lightroom itself omits when the amount is zero
    // (ColorNoiseReductionDetail / Smoothness are absent from the two files
    // whose ColorNoiseReduction is 0) must stay absent from ours too.
    for (key, v, amount) in [
        ("LuminanceNoiseReductionDetail", r.nr_detail, r.noise_reduction),
        ("LuminanceNoiseReductionContrast", r.nr_contrast, r.noise_reduction),
        ("ColorNoiseReduction", r.color_nr, 0.0),
        ("ColorNoiseReductionDetail", r.color_nr_detail, r.color_nr),
        ("ColorNoiseReductionSmoothness", r.color_nr_smooth, r.color_nr),
    ] {
        if (v != 0.0 || amount_carries(key, amount)) && ungated(key) {
            attr(&mut a, key, &(v.round() as i64).to_string());
        }
    }

    // Manual lens-vignette correction. `VignetteAmount` name verified against
    // the user's sidecars (present, =0, in 140 of them); the Midpoint companion
    // key follows the documented ACR pair and is only emitted when the amount
    // is set — a zero-amount recipe stays byte-identical to the old writer.
    if r.lens_vignette != 0.0 {
        attr(&mut a, "VignetteAmount", &signed(r.lens_vignette));
        attr(&mut a, "VignetteMidpoint", &(r.lens_vignette_mid.round() as i64).to_string());
    }

    // Manual distortion correction — key name verified against the user's
    // sidecars (`LensManualDistortionAmount="0"` in 148 of them). Same
    // only-when-set policy as the vignette pair. NB: our render's amount→curve
    // gain is our own calibration; Adobe's is unpublished, so LR's slider at
    // the same number may correct a somewhat different physical strength.
    if r.lens_distortion != 0.0 {
        attr(&mut a, "LensManualDistortionAmount", &signed(r.lens_distortion));
    }

    // Manual lateral CA (R25 B3), same only-when-set policy. UNSIGNED: these
    // are the legacy PV2010 integer keys, so they belong to the
    // `Sharpness="40"` family rather than the `Contrast2012="+22"` one. No
    // sidecar in the user's library carries either key (Lightroom's PV2012
    // panel replaced the pair with de-fringe and the auto switch), so the
    // spelling follows the family convention rather than a measurement — and
    // an absent key is what every one of those files already has.
    for (key, v) in [("ChromaticAberrationR", r.ca_r), ("ChromaticAberrationB", r.ca_b)] {
        if v != 0.0 && ungated(key) {
            attr(&mut a, key, &(v.round() as i64).to_string());
        }
    }
    // Adobe's auto-CA switch, written as the 0/1 flag Lightroom writes
    // (`crs:AutoLateralCA="0"` on six of the user's sidecars, `="1"` on the
    // seventh) — and only when ON, so a recipe that never met the key does
    // not start asserting "off" into someone's document.
    if r.auto_lateral_ca && ungated("AutoLateralCA") {
        attr(&mut a, "AutoLateralCA", "1");
    }

    // De-fringe (R25 B3): all six keys, UNCONDITIONALLY, which is the shape
    // Lightroom itself writes — 7 of 7 of the user's sidecars carry the whole
    // block with the amounts at 0 and the hue windows at Adobe's 30/70 and
    // 40/60. Writing only the non-default ones would emit a hue window with
    // no amount beside it (or the reverse), a shape no real document has.
    // Unsigned integers: `DefringePurpleAmount="3"`, never `"+3"` — the
    // `Sharpness` family again.
    for (key, v) in [
        ("DefringePurpleAmount", r.defringe_purple),
        ("DefringePurpleHueLo", r.defringe_purple_lo),
        ("DefringePurpleHueHi", r.defringe_purple_hi),
        ("DefringeGreenAmount", r.defringe_green),
        ("DefringeGreenHueLo", r.defringe_green_lo),
        ("DefringeGreenHueHi", r.defringe_green_hi),
    ] {
        // The era gate releases these six together or not at all (see
        // `era_suppressed_attr_keys`), so this per-key test can never split
        // the block the paragraph above insists on writing whole.
        if ungated(key) {
            attr(&mut a, key, &(v.round() as i64).to_string());
        }
    }

    // The nine CARRIED effects (R25 B2): Lightroom renders them, we do not,
    // and the sidecar is the whole point of modelling them at all.
    //
    // PER-KEY conditional, not the vignette pair's group gate. The pair above
    // needs one because `lens_vignette_mid`'s neutral is 50 and it has no
    // "absent" spelling; every field here is neutral at ZERO, so writing only
    // the non-zero ones IS "write what is non-neutral" — and the three whose
    // ACR default is not zero (Midpoint/Feather 50, Style 1) then reach
    // Lightroom by ABSENCE, which is the honest encoding of "the recipe never
    // learned one" and cannot invent a Midpoint of 0. Verified round-trip
    // against the user's own sidecars: Lightroom writes the companions only
    // when the amount is non-zero, so an imported file comes back the same
    // shape it went in.
    for (key, v) in [
        ("PostCropVignetteAmount", r.post_crop_vignette),
        ("PostCropVignetteRoundness", r.post_crop_vignette_round),
    ] {
        if v != 0.0 && ungated(key) {
            attr(&mut a, key, &signed(v));
        }
    }
    for (key, v) in [
        ("PostCropVignetteMidpoint", r.post_crop_vignette_mid),
        ("PostCropVignetteFeather", r.post_crop_vignette_feather),
        ("PostCropVignetteStyle", r.post_crop_vignette_style),
        ("PostCropVignetteHighlightContrast", r.post_crop_vignette_hl),
        ("GrainAmount", r.grain),
        ("GrainSize", r.grain_size),
        ("GrainFrequency", r.grain_rough),
    ] {
        // `ungated` is redundant for every ZERO-neutral key here — a value
        // that passed `v != 0.0` has already left the default the gate keys
        // on — and it is written all the same, on both loops and on every
        // R25 key below and above: the gate is the LAW for these
        // twenty-seven, and a law spelled at only the sites that need it
        // today is a law the next default change quietly repeals.
        if v != 0.0 && ungated(key) {
            attr(&mut a, key, &(v.round() as i64).to_string());
        }
    }

    // The PASS-THROUGH blocks (R25 B4): Transform / Upright and Camera
    // Calibration, written back as the exact strings they arrived as.
    //
    // In [`PASSTHROUGH_CRS`] order, NOT the map's. A `BTreeMap` iterates
    // alphabetically, which would interleave the two blocks
    // (CameraCalibration… before CameraProfile before Perspective…) and put
    // them in an order no Lightroom file uses — legal XML, unreadable diffs.
    // The declared order is Adobe's own grouping.
    //
    // No formatting whatever: `+0.9`, `0.00` and `Adobe Standard` all go out
    // as themselves. `attr` still XML-escapes, which is transport, not
    // interpretation — and it is why a profile name with an `&` in it
    // survives. A key absent from the map was absent from the document, and
    // stays absent: we do not invent a Calibration block for a file that
    // never had one.
    for key in PASSTHROUGH_CRS {
        if let Some(v) = r.passthrough.get(key) {
            attr(&mut a, key, v);
        }
    }

    // Crop + straighten, as ONE rotated-corner encoding ([`engine_to_lr_crop`],
    // R27). Only applied by Lightroom when HasCrop is True — a non-zero
    // CropAngle under HasCrop="False" is ignored — so a straighten-only recipe
    // still ships HasCrop=True, and what it ships is the STRAIGHTENED frame's
    // own four corners (which are `0,0,1,1` exactly when there is no tilt, so
    // every un-straightened document this writer has ever produced is
    // unchanged to the byte).
    match engine_to_lr_crop(r.crop.as_ref(), r.straighten_deg as f64, frame) {
        Some(c) => {
            attr(&mut a, "HasCrop", "True");
            attr(&mut a, "CropTop", &format!("{:.6}", c.top));
            attr(&mut a, "CropLeft", &format!("{:.6}", c.left));
            attr(&mut a, "CropBottom", &format!("{:.6}", c.bottom));
            attr(&mut a, "CropRight", &format!("{:.6}", c.right));
            // SIX decimals, which is Lightroom's own precision for this key
            // (`-3.274380`) and the four beside it. The `{:.1}` this replaces
            // (R27, `P3-cropangle-model.md` §6.4) turned that specimen into
            // `-3.3` on every re-save: 0.0256° of drift = 4.3 px of
            // edge-to-edge tilt across a 9504 px frame, in a carrier whose
            // whole purpose is a lossless round trip.
            if c.angle_deg != 0.0 {
                attr(&mut a, "CropAngle", &format!("{:.6}", c.angle_deg));
            }
        }
        None => attr(&mut a, "HasCrop", "False"),
    }

    attr(
        &mut a,
        "ToneCurveName2012",
        if r.tone_curve.is_empty() { "Linear" } else { "Custom" },
    );
    // Last, so the fresh-document skeleton stays byte-identical to the
    // pre-merge writer (which hardcoded this right after {attrs}).
    attr(&mut a, "HasSettings", "True");
    a
}

/// Every child ELEMENT the writer owns (tone curves + mask corrections),
/// shared by the fresh-document writer and the merge path — PLUS the per-mask
/// loss verdicts that fall out of emitting them.
///
/// The mask pass runs even when `include_masks` is false (the merge is
/// preserving the base's own block): the caller has to disclose the recipe's
/// losses either way, and running it here is what stops a save from building the
/// mask XML twice (R22 NIT-1).
fn owned_children(
    r: &EditRecipe,
    include_masks: bool,
    frame: Option<FrameAspect>,
) -> (String, Vec<MaskLoss>) {
    let (masks, losses) = masks_xml(r, frame);

    // Tone curves are child elements (rdf:Seq of "x, y" strings), not attributes.
    // One builder for the master + the three per-channel curves (verified key
    // names against the user's sidecar: ToneCurvePV2012Red/Green/Blue).
    let curve_elem = |tag: &str, points: &[crate::recipe::CurvePoint]| -> String {
        if points.is_empty() {
            return String::new();
        }
        let pts: String = points
            .iter()
            .map(|p| format!("     <rdf:li>{}, {}</rdf:li>\n", p.input, p.output))
            .collect();
        format!("\n   <crs:{tag}>\n    <rdf:Seq>\n{pts}    </rdf:Seq>\n   </crs:{tag}>")
    };
    let children = format!(
        "{}{}{}{}{}",
        curve_elem("ToneCurvePV2012", &r.tone_curve),
        curve_elem("ToneCurvePV2012Red", &r.red_curve),
        curve_elem("ToneCurvePV2012Green", &r.green_curve),
        curve_elem("ToneCurvePV2012Blue", &r.blue_curve),
        if include_masks { masks.as_str() } else { "" },
    );
    (children, losses)
}

/// The rationale, made safe for an XML comment. XML comments forbid "--"
/// anywhere inside and "-" as the final char — an AI rationale containing
/// "--" made the WHOLE sidecar unparsable. Swap ASCII hyphens in those
/// positions for U+2011 (display-only text; xml_escape has already run, so
/// no raw markup survives either).
fn safe_rationale(r: &EditRecipe) -> String {
    let s = xml_text_escape(&r.rationale).replace("--", "‑‑");


    s.strip_suffix('-').map(|p| format!("{p}‑")).unwrap_or(s)
}

/// Render `recipe` as a complete, FRESH `.xmp` sidecar document. When a
/// previous document exists, prefer [`merge_recipe_into_xmp`] —
/// regeneration discards everything AutoShade does not model (A11).
///
/// A WRITER that also has to disclose what the projection cost should take
/// [`recipe_to_xmp_with_losses`] instead: the verdicts fall out of the same pass
/// that emits the XML, so asking for them separately builds the mask block a
/// second time.
pub fn recipe_to_xmp(r: &EditRecipe) -> String {
    recipe_to_xmp_with_losses(r).0
}

/// [`recipe_to_xmp_with_losses`] told what frame the photo is — the aspect the
/// radial projection needs to write `crs:Angle` (see [`FrameAspect`]). A fresh
/// document declares no `tiff:ImageWidth/ImageLength` of its own, so without
/// this a rotated radial can only be written as its unrotated ellipse and
/// disclosed; with it the tilt goes out.
pub fn recipe_to_xmp_in_frame(
    r: &EditRecipe,
    frame: Option<FrameAspect>,
) -> (String, Vec<MaskLoss>) {
    let (desc, losses) = crs_description(r, frame);
    (xmp_document(r, &desc), losses)
}

/// [`recipe_to_xmp`] and the writer's own per-mask loss verdicts, from ONE pass
/// over the masks. `write_xmp_doc` used to build the document and then call
/// [`mask_export_losses`], i.e. run `masks_xml` twice per save for a `Vec` the
/// first pass had already produced and thrown away (R22 NIT-1).
pub fn recipe_to_xmp_with_losses(r: &EditRecipe) -> (String, Vec<MaskLoss>) {
    recipe_to_xmp_in_frame(r, None)
}

/// The document skeleton around one `rdf:Description`.
fn xmp_document(r: &EditRecipe, desc: &str) -> String {
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"AutoShade 2\">\n\
 <!-- Generated by AutoShade. AI rationale: {rationale} (confidence {conf:.2}) -->\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  {desc}\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n",
        rationale = safe_rationale(r),
        conf = r.confidence,
    )
}

/// The `rdf:Description` carrying everything this writer owns — the ONE
/// definition, so a fresh document and a spliced-in one cannot drift. Carries
/// the mask-loss verdicts out with it (see [`owned_children`]).
///
/// A FRESH document declares its own frame (R27 A8) and therefore writes its
/// geometry in that frame — see [`frame_declaration`] and [`in_source_frame`].
fn crs_description(r: &EditRecipe, frame: Option<FrameAspect>) -> (String, Vec<MaskLoss>) {
    let r = in_source_frame(r, frame);
    let r = r.as_ref();
    let (children, losses) = owned_children(r, true, frame);
    let desc = format!(
        "<rdf:Description rdf:about=\"\"\n\
    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"{tiff}{attrs}>{children}\n\
  </rdf:Description>",
        tiff = frame_declaration(frame),
        attrs = owned_attrs(r, frame),
    );
    (desc, losses)
}

/// The `tiff:` block that says which frame the `crs:` coordinates in this
/// document are measured against — `ImageWidth` / `ImageLength` (the SOURCE
/// frame, un-rotated, i.e. the RAW's `DefaultCropSize`) and `Orientation` (the
/// turn that carries it to the frame the photographer sees, EXIF composed with
/// their own quarter turns).
///
/// Empty when the frame is unknown, which is also when nothing above needed it.
///
/// **Why a writer declares this at all** (R27, closing `C-rotation-skeleton.md`'s
/// round-trip hole). Until now AutoShade's own fresh sidecars declared no frame,
/// so re-importing one could not decode its own rotated radial — the reader
/// needs `W/H` to fold a pixel-frame tilt into the engine's normalised one, and
/// a document with no declaration hands it nothing. Every real Lightroom
/// sidecar carries these (`_DSC9600.xmp`: `tiff:ImageWidth="9504"
/// tiff:ImageLength="6336"`); ours now do too, and a portrait sidecar's
/// `tiff:Orientation` is what tells the reader the numbers are sensor-frame.
fn frame_declaration(frame: Option<FrameAspect>) -> String {
    let Some(f) = frame else { return String::new() };
    format!(
        "\n    xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\
         \n    tiff:ImageWidth=\"{w}\"\n    tiff:ImageLength=\"{h}\"\n    tiff:Orientation=\"{o}\"",
        w = f.w.round() as i64,
        h = f.h.round() as i64,
        o = f.turn().to_u16().max(1),
    )
}

/// The recipe as the frame it will be WRITTEN in sees it: the inverse of the
/// turn [`xmp_to_recipe`] applies on the way in.
///
/// Borrowed and untouched for every frame whose source and display coincide —
/// every landscape capture, every baked image, and every document that
/// declares no frame at all.
fn in_source_frame<'a>(
    r: &'a EditRecipe,
    frame: Option<FrameAspect>,
) -> std::borrow::Cow<'a, EditRecipe> {
    use rawler::Orientation as O;
    let Some(f) = frame else { return std::borrow::Cow::Borrowed(r) };
    let turn = f.turn();
    if matches!(turn, O::Normal | O::Unknown) {
        return std::borrow::Cow::Borrowed(r);
    }
    // The dihedral group of the square: the two quarter turns are each other's
    // inverse and everything else is an involution.
    let back = match turn {
        O::Rotate90 => O::Rotate270,
        O::Rotate270 => O::Rotate90,
        other => other,
    };
    // The recipe's coordinates are in the DISPLAY frame here — this is the
    // inverse projection — so the aspect a brush rewrite rescales out of is the
    // displayed rectangle, `FrameAspect`'s own `displayed()` (R29 C1,
    // `render::CoordFrame`). Passing the SOURCE aspect would rescale every
    // radius by `H/W` where `W/H` was owed, i.e. by the square of the error.
    let d = f.displayed();
    let mut owned = r.clone();
    crate::render::orient_recipe_coords(
        &mut owned,
        back,
        crate::render::CoordFrame::new(d.w, d.h),
    );
    std::borrow::Cow::Owned(owned)
}

/// First occurrence of `needle` that is real MARKUP — not text quoted inside a
/// comment, a CDATA section or a processing instruction.
///
/// `xmp.rs` already owns this distinction for the close scanner
/// (`a_pathological_sidecar_neither_hangs_nor_believes_a_comment`), and a
/// plain `str::find` here regressed it: a sidecar whose header quotes
/// `<rdf:RDF …>` in a comment got the settings block spliced INSIDE that
/// comment. The merge then "succeeded", so no loss note fired, and the file
/// Lightroom reads carried none of the user's develop — exactly the silence
/// the disclosure exists to end.
///
/// Forward-only and single-sweep, like [`Landmarks`]: `at` never moves
/// backwards, so the scan is linear in the document.
fn find_outside_constructs(doc: &str, needle: &str) -> Option<usize> {
    let mut at = 0;
    loop {
        let hit = at + doc[at..].find(needle)?;
        // The innermost construct that OPENS at or before the hit and has not
        // closed by then swallows it; skip past that construct and resume.
        let swallowing = CONSTRUCTS.iter().filter_map(|(open, close)| {
            let o = doc[at..=hit].rfind(open)? + at;
            // A construct that already closed before the hit does not swallow.
            let end = doc[o + open.len()..].find(close).map(|e| o + open.len() + e);
            match end {
                Some(e) if e > hit => Some(e + close.len()),
                // Unterminated: everything after it is text, so there is no
                // real markup left to find.
                None => Some(doc.len()),
                Some(_) => None,
            }
        });
        match swallowing.max() {
            Some(resume) => at = resume.min(doc.len()),
            None => return Some(hit),
        }
        if at >= doc.len() {
            return None;
        }
    }
}

/// Add our settings to a sidecar that has none, keeping every byte of it.
///
/// The base is a real XMP document — it just carries no camera-raw settings
/// (a ratings/keywords sidecar from exiftool, Bridge or Capture One). Splicing
/// a fresh `rdf:Description` in after the `rdf:RDF` open tag preserves the
/// user's properties AND records ours, so the save is a genuine merge and the
/// caller has no loss to disclose. Returning `None` (no `rdf:RDF`, or a
/// self-closing one) keeps the old regenerate-and-say-so behaviour — the
/// document is then not one we can account for.
fn insert_crs_description(
    existing: &str,
    r: &EditRecipe,
    frame: Option<FrameAspect>,
) -> Option<(String, Vec<MaskLoss>)> {
    let at = find_outside_constructs(existing, "<rdf:RDF")?;
    let (gt, self_closing) = scan_tag_end(existing, at)?;
    if self_closing {
        return None;
    }
    let (desc, losses) = crs_description(r, frame);
    let mut out = String::with_capacity(existing.len() + 512);
    out.push_str(&existing[..=gt]);
    out.push_str("\n  ");
    out.push_str(&desc);
    out.push_str(&existing[gt + 1..]);
    Some((out, losses))
}

/// The crs attribute keys this writer OWNS — the removal universe for the
/// merge. Must cover every key `owned_attrs` can EVER emit, including the
/// conditional ones (a cleared vignette must disappear from a merged
/// document, not linger at its old value).
///
/// `pub(crate)` since R23-1: the control registry's tests assert that every
/// attribute the AI/eval ruler reads is one this writer can write, so a
/// misspelled key in the ruler cannot silently measure nothing.
pub(crate) fn owned_attr_keys() -> Vec<String> {
    let mut keys: Vec<String> = [
        "Version",
        "ProcessVersion",
        "WhiteBalance",
        "Temperature",
        "Tint",
        "Exposure2012",
        "Contrast2012",
        "Highlights2012",
        "Shadows2012",
        "Whites2012",
        "Blacks2012",
        "Clarity2012",
        "Dehaze",
        "Vibrance",
        "Saturation",
        "Texture",
        "SplitToningShadowHue",
        "SplitToningShadowSaturation",
        "SplitToningHighlightHue",
        "SplitToningHighlightSaturation",
        "SplitToningBalance",
        "ColorGradeShadowLum",
        "ColorGradeMidtoneHue",
        "ColorGradeMidtoneSat",
        "ColorGradeMidtoneLum",
        "ColorGradeHighlightLum",
        "ColorGradeGlobalHue",
        "ColorGradeGlobalSat",
        "ColorGradeGlobalLum",
        "ColorGradeBlending",
        "Sharpness",
        "LuminanceSmoothing",
        // The R25 B3 carried detail axes + the manual CA pair + the auto-CA
        // switch + the six de-fringe keys. Same reason as the B2 block below:
        // owning a key is what makes the merge STRIP it before rewriting, and
        // it is also what takes the key OUT of `unmodelled_global_crs` (whose
        // universe is the complement of this list).
        "SharpenRadius",
        "SharpenDetail",
        "SharpenEdgeMasking",
        "LuminanceNoiseReductionDetail",
        "LuminanceNoiseReductionContrast",
        "ColorNoiseReduction",
        "ColorNoiseReductionDetail",
        "ColorNoiseReductionSmoothness",
        "VignetteAmount",
        "VignetteMidpoint",
        "LensManualDistortionAmount",
        "ChromaticAberrationR",
        "ChromaticAberrationB",
        "AutoLateralCA",
        "DefringePurpleAmount",
        "DefringePurpleHueLo",
        "DefringePurpleHueHi",
        "DefringeGreenAmount",
        "DefringeGreenHueLo",
        "DefringeGreenHueHi",
        // The R25 B2 carried effects. Owning a key is what makes the merge
        // STRIP it before rewriting — without these nine the writer's own
        // values would land beside Lightroom's originals as duplicate
        // attributes, and `unmodelled_global_crs` would go on naming keys we
        // now model.
        "PostCropVignetteAmount",
        "PostCropVignetteMidpoint",
        "PostCropVignetteFeather",
        "PostCropVignetteRoundness",
        "PostCropVignetteStyle",
        "PostCropVignetteHighlightContrast",
        "GrainAmount",
        "GrainSize",
        "GrainFrequency",
        "HasCrop",
        "CropTop",
        "CropLeft",
        "CropBottom",
        "CropRight",
        "CropAngle",
        "ToneCurveName2012",
        "HasSettings",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // The R25 B4 PASS-THROUGH blocks. Owning them is what makes the merge
    // strip each key before the writer puts it back — without that, our
    // verbatim copy would land beside Lightroom's original as a duplicate
    // attribute. It is also what takes the sixteen out of
    // `unmodelled_global_crs`, whose universe is this list's complement.
    keys.extend(PASSTHROUGH_CRS.iter().map(|s| (*s).to_string()));
    for band in crate::recipe::HSL_BANDS {
        keys.push(format!("HueAdjustment{band}"));
        keys.push(format!("SaturationAdjustment{band}"));
        keys.push(format!("LuminanceAdjustment{band}"));
    }
    keys
}

/// The index of the `>` ending the tag that opens at `start`, plus whether
/// the tag is self-closing. QUOTE-AWARE: attribute values may legally
/// contain `>` (Lightroom mask names do).
fn scan_tag_end(doc: &str, start: usize) -> Option<(usize, bool)> {
    let mut quote: Option<char> = None;
    let mut prev_nonws = ' ';
    for (i, c) in doc[start..].char_indices() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '>' => return Some((start + i, prev_nonws == '/')),
                _ => {}
            },
        }
        if !c.is_whitespace() {
            prev_nonws = c;
        }
    }
    None
}

struct XmlAttribute<'a> {
    name: &'a str,
    value: &'a str,
    span: std::ops::Range<usize>,
}

fn next_xml_attribute<'a>(tag: &'a str, cursor: &mut usize) -> Option<XmlAttribute<'a>> {
    let bytes = tag.as_bytes();
    let mut i = *cursor;
    if i == 0 && bytes.first() == Some(&b'<') {
        i = 1;
        if bytes.get(i) == Some(&b'/') {
            i += 1;
        }
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && !matches!(bytes[i], b'/' | b'>')
        {
            i += 1;
        }
    }
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() || matches!(bytes[i], b'/' | b'>') {
        return None;
    }

    let start = i;
    while i < bytes.len()
        && !bytes[i].is_ascii_whitespace()
        && !matches!(bytes[i], b'=' | b'/' | b'>')
    {
        i += 1;
    }
    let name_end = i;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if bytes.get(i) != Some(&b'=') {
        return None;
    }
    i += 1;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let quote = *bytes.get(i)?;
    if !matches!(quote, b'"' | b'\'') {
        return None;
    }
    i += 1;
    let value_start = i;
    while i < bytes.len() && bytes[i] != quote {
        i += 1;
    }
    let value_end = i;
    i += 1;
    *cursor = i;
    Some(XmlAttribute {
        name: &tag[start..name_end],
        value: &tag[value_start..value_end],
        span: start..i,
    })
}

fn xml_attribute_raw<'a>(
    tag: &'a str,
    key: &str,
) -> Option<(std::ops::Range<usize>, &'a str)> {
    let mut cursor = 0;
    while let Some(a) = next_xml_attribute(tag, &mut cursor) {
        if a.name == key {
            return Some((a.span, a.value));
        }
    }
    None
}

fn next_xml_tag(doc: &str, mut from: usize) -> Option<(usize, usize, bool)> {
    'scan: loop {
        let start = from + doc[from..].find('<')?;
        let rest = &doc[start..];
        for &(open, close) in &CONSTRUCTS {
            if let Some(after) = rest.strip_prefix(open) {
                from = start + open.len() + after.find(close)? + close.len();
                continue 'scan;
            }
        }
        let (end, self_closing) = scan_tag_end(doc, start)?;
        return Some((start, end, self_closing));
    }
}



/// The first `rdf:Description` opening tag that carries camera-raw settings:
/// it declares `xmlns:crs`, holds a `crs:` attribute, or holds a top-level
/// `crs:` CHILD element. The child rule is what finds a Description whose
/// `xmlns:crs` lives on an ANCESTOR (`rdf:RDF`) and whose settings are all in
/// property-element form — a legal spelling the attribute-only test missed,
/// which sent the merge down the insert path and spliced a SECOND settings
/// Description into the same document. Depth is what keeps the child rule
/// honest: a `crs:` element nested inside a foreign container marks its OWN
/// parent Description, never an outer one — the parent is whatever element is
/// open when the `crs:` child appears (single pass, one open-element stack),
/// so a creative Look's baked parameters can only ever mark the settings
/// Description that contains the Look, which is the right answer anyway.
fn find_crs_description(doc: &str) -> Option<usize> {
    // (name, open-tag start) for every open element. Close-tag mismatches pop
    // nothing — malformed markup degrades to the old attribute-only rule
    // instead of failing a document the flat scan used to find.
    let mut stack: Vec<(&str, usize)> = Vec::new();
    let mut from = 0;
    while let Some((start, end, self_closing)) = next_xml_tag(doc, from) {
        let tag = &doc[start..=end];
        let name = tag_name(tag);
        if tag.starts_with("</") {
            if stack.last().is_some_and(|(n, _)| *n == name) {
                stack.pop();
            }
            from = end + 1;
            continue;
        }
        if name == "rdf:Description" {
            let mut cursor = 0;
            while let Some(a) = next_xml_attribute(tag, &mut cursor) {
                // The declaration only marks the settings Description when it
                // binds the CANONICAL camera-raw URI. The scope-aware gate
                // (R12-03) now lets an UNUSED foreign rebind through as
                // harmless — but the merge keys on this very attribute, and
                // splicing canonical-intent `crs:` settings into a scope
                // where `crs` means something else would corrupt the
                // document the gate just cleared.
                if (a.name == "xmlns:crs" && xml_unescape(a.value).as_ref() == CRS_URI)
                    || a.name.starts_with("crs:")
                {
                    return Some(start);
                }
            }
        } else if name.starts_with("crs:")
            && let Some(&(parent, parent_start)) = stack.last()
            && parent == "rdf:Description"
        {
            return Some(parent_start);
        }
        if !self_closing {
            stack.push((name, start));
        }
        from = end + 1;
    }
    None
}

const CRS_URI: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";
const RDF_URI: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// Every scanner in this module identifies namespaces by the CONVENTIONAL
/// prefixes (`crs:`, `rdf:`) — never by URI. A document that binds either
/// namespace to a different prefix, or binds `crs`/`rdf` to a different URI,
/// is therefore one these scanners silently misread: its settings import as
/// neutral with no disclosure, and the merge — finding "no" crs Description —
/// used to splice OUR settings in beside the foreign-prefixed ones, publishing
/// one document with two contradictory camera-raw blocks and a clean "saved".
/// This is the refusal gate: `Some(reason)` names the binding, the merge
/// refuses (the caller regenerates AND discloses), and the import surfaces the
/// same sentence.
///
/// SCOPE-AWARE (R12-03): bindings are resolved through an element scope
/// stack, XML-semantics style, and the gate fires only where a binding would
/// actually corrupt this document's reading — a `crs:`/`rdf:` NAME (element
/// or attribute) whose in-scope binding is not the canonical URI, a name
/// under some OTHER prefix whose in-scope binding IS a canonical URI, or an
/// unprefixed ELEMENT under a default namespace bound to one (unprefixed
/// attributes take no namespace, per XML). A declaration nobody uses — a
/// nested island rebinding `crs` around content that never says `crs:`, or a
/// foreign alias for the camera-raw URI that no name ever resolves through —
/// no longer refuses the whole document the way the flat scan did. An
/// undeclared `crs:`/`rdf:` prefix still passes: the scanners read by
/// prefix and never required a declaration.
fn xmlns_conflict(doc: &str) -> Option<String> {
    if doc.len() > MAX_XMP_BYTES {
        return None;
    }
    // One frame per OPEN element that declares namespaces, tagged with its
    // depth AND its element name: a close tag pops a frame only when both
    // match, so a surplus or misnamed close (round-13 review R13-01) leaves
    // the frame in place — malformed nesting degrades toward refusal, never
    // toward releasing a foreign binding early. `""` keys the default
    // namespace. The bound counts LIVE DECLARATIONS, not frames (R13-02): a
    // single tag can carry a declaration flood, and `resolve` walks every
    // live declaration per name, so the budget is what keeps an adversarial
    // 16 MiB document from going quadratic.
    const MAX_NS_DECLS: usize = 256;
    struct NsFrame<'a> {
        depth: usize,
        name: &'a str,
        decls: Vec<(&'a str, String)>,
    }
    fn resolve<'a>(frames: &'a [NsFrame<'_>], pfx: &str) -> Option<&'a str> {
        frames.iter().rev().find_map(|f| {
            f.decls.iter().rev().find(|(p, _)| *p == pfx).map(|(_, u)| u.as_str())
        })
    }
    fn against(uri: &str, pfx: Option<&str>) -> Option<String> {
        match pfx {
            Some("crs") => (uri != CRS_URI)
                .then(|| format!("xmlns:crs is bound to {uri}, not the camera-raw namespace")),
            Some("rdf") => (uri != RDF_URI)
                .then(|| format!("xmlns:rdf is bound to {uri}, not the RDF namespace")),
            Some(pfx) if uri == CRS_URI => Some(format!(
                "the camera-raw namespace is bound to the `{pfx}:` prefix; \
                 this build reads only `crs:`"
            )),
            Some(pfx) if uri == RDF_URI => Some(format!(
                "the RDF namespace is bound to the `{pfx}:` prefix; \
                 this build reads only `rdf:`"
            )),
            None if uri == CRS_URI || uri == RDF_URI => Some(format!(
                "the {} namespace is bound as the DEFAULT namespace; this \
                 build reads only the `crs:`/`rdf:` prefixes",
                if uri == CRS_URI { "camera-raw" } else { "RDF" }
            )),
            _ => None,
        }
    }
    let mut depth: usize = 0;
    let mut live_decls: usize = 0;
    let mut frames: Vec<NsFrame> = Vec::new();
    let mut from = 0;
    while let Some((start, end, self_closing)) = next_xml_tag(doc, from) {
        from = end + 1;
        let tag = &doc[start..=end];
        if tag.starts_with("</") {
            if depth > 0 {
                if frames.last().is_some_and(|f| f.depth == depth && f.name == tag_name(tag)) {
                    let f = frames.pop().expect("just matched");
                    live_decls -= f.decls.len();
                }
                depth -= 1;
            }
            continue;
        }
        depth += 1;
        // Declarations bind the element they sit on (and its own other
        // attributes) regardless of attribute order — collect them first.
        let mut decls: Vec<(&str, String)> = Vec::new();
        let mut cursor = 0;
        while let Some(a) = next_xml_attribute(tag, &mut cursor) {
            if let Some(pfx) = a.name.strip_prefix("xmlns:") {
                decls.push((pfx, xml_unescape(a.value).into_owned()));
            } else if a.name == "xmlns" {
                decls.push(("", xml_unescape(a.value).into_owned()));
            }
        }
        let name = tag_name(tag);
        if !decls.is_empty() {
            live_decls += decls.len();
            frames.push(NsFrame { depth, name, decls });
            if live_decls > MAX_NS_DECLS {
                // Beyond the tracking budget the gate cannot prove a binding
                // harmless (or resolve names affordably), so it refuses —
                // conservative, and disclosed.
                return Some(
                    "more xmlns declarations than this build tracks; \
                     namespace bindings cannot be verified"
                        .to_string(),
                );
            }
        }
        // The element's own name…
        let elem_pfx = name.split_once(':').map(|(p, _)| p);
        if let Some(uri) = resolve(&frames, elem_pfx.unwrap_or(""))
            && let Some(why) = against(uri, elem_pfx)
        {
            return Some(why);
        }
        // …then every non-declaration attribute name. Unprefixed attributes
        // take no namespace (not the default one), so only prefixed names
        // resolve here.
        let mut cursor = 0;
        while let Some(a) = next_xml_attribute(tag, &mut cursor) {
            if a.name == "xmlns" || a.name.starts_with("xmlns:") {
                continue;
            }
            if let Some((pfx, _)) = a.name.split_once(':')
                && let Some(uri) = resolve(&frames, pfx)
                && let Some(why) = against(uri, Some(pfx))
            {
                return Some(why);
            }
        }
        if self_closing {
            if frames.last().is_some_and(|f| f.depth == depth) {
                let f = frames.pop().expect("just matched");
                live_decls -= f.decls.len();
            }
            depth -= 1;
        }
    }
    None
}



/// The `</rdf:Description>` closing the element whose opening tag ended just
/// before `from` — DEPTH-COUNTED, because Lightroom nests `rdf:Description`
/// elements inside mask corrections (the batch-3 lesson: naive scans shred
/// nested structures).
/// The text constructs whose contents are NOT markup. A `</rdf:Description>`
/// inside any of them is not a close.
const CONSTRUCTS: [(&str, &str); 3] = [("<!--", "-->"), ("<![CDATA[", "]]>"), ("<?", "?>")];

/// Every landmark this scanner needs, each cached as "the first hit AT OR
/// AFTER `from`" and refreshed only once `from` has passed it.
///
/// This is the whole performance contract. `from` only ever moves forward, so
/// a cached hit stays correct until it is crossed, and each refresh resumes
/// its scan at the new `from` — every landmark therefore sweeps the document
/// at most once and the loop is linear overall. A cursor that has run out
/// (`None`) is never searched again: since `from` only advances, a pattern
/// with no occurrence after `from` has none after any later `from` either.
/// That last rule is load-bearing — re-searching an absent pattern to the end
/// of the document on every iteration is itself quadratic.
struct Landmarks {
    /// First `</rdf:Description>` at or after `from` — required, so not optional.
    close: usize,
    /// First `<rdf:Description` at or after `from`, if any remain.
    open: Option<usize>,
    /// First occurrence of each entry of [`CONSTRUCTS`], if any remain.
    ctor: [Option<usize>; 3],
}

impl Landmarks {
    fn new(doc: &str, from: usize) -> Option<Self> {
        const CLOSE: &str = "</rdf:Description>";
        const OPEN: &str = "<rdf:Description";
        Some(Landmarks {
            close: from + doc[from..].find(CLOSE)?,
            open: doc[from..].find(OPEN).map(|r| from + r),
            ctor: std::array::from_fn(|i| doc[from..].find(CONSTRUCTS[i].0).map(|r| from + r)),
        })
    }

    /// Advance every cursor the new `from` has overtaken. Returns `None` when
    /// no close remains, which sinks the whole scope.
    fn refresh(&mut self, doc: &str, from: usize) -> Option<()> {
        const CLOSE: &str = "</rdf:Description>";
        const OPEN: &str = "<rdf:Description";
        if from > self.close {
            self.close = from + doc[from..].find(CLOSE)?;
        }
        if self.open.is_some_and(|o| from > o) {
            self.open = doc[from..].find(OPEN).map(|r| from + r);
        }
        for (slot, (open, _)) in self.ctor.iter_mut().zip(CONSTRUCTS.iter()) {
            // A cursor already at or ahead of `from` is still the first hit;
            // one that has run out (None) stays out, and re-searching it would
            // sweep to the end of the document on every iteration — the exact
            // shape of the quadratic this cache exists to remove.
            if slot.is_some_and(|p| from > p) {
                *slot = doc[from..].find(open).map(|r| from + r);
            }
        }
        Some(())
    }

    /// The construct that opens before both the next open tag and the close —
    /// the only one that is this iteration's business.
    fn pending_construct(&self) -> Option<(usize, &'static str, &'static str)> {
        self.ctor
            .iter()
            .zip(CONSTRUCTS.iter())
            .filter_map(|(slot, (open, close))| slot.map(|p| (p, *open, *close)))
            .filter(|(p, _, _)| *p < self.close && self.open.is_none_or(|o| *p < o))
            .min_by_key(|(p, _, _)| *p)
    }
}

fn find_matching_close(doc: &str, mut from: usize) -> Option<usize> {
    const CLOSE: &str = "</rdf:Description>";
    let mut depth = 0usize;
    // Two DISTINCT quadratic blowups have lived in this function; both showed
    // up as a sidecar beside a RAW pegging a core inside SAVE_LOCK, holding
    // one of the server's eight request permits, on nothing worse than photo
    // SELECTION. Both are now answered by the same rule — cache every
    // landmark, never re-scan what `from` has not passed (see [`Landmarks`]).
    //
    //   1. Re-running the CLOSE search on every nested open: Θ(depth²).
    //      Measured 2.8 MB of nesting at 55.97 s.
    //   2. Re-running the CONSTRUCT search on every skipped comment / PI /
    //      CDATA — the scan that FIXED (1) introduced this one, and it was
    //      worse per byte because a body of back-to-back comments re-scanned
    //      the whole remaining window three times per construct while `from`
    //      crawled forward one construct at a time. Measured 640 KB of
    //      comments at 8.47 s, against 51 µs before the construct skip
    //      existed at all.
    //
    // Both shapes are now linear: 2.8 MB nested and 640 KB of comments each
    // finish in single-digit milliseconds (see the timed test).
    let mut marks = Landmarks::new(doc, from)?;
    loop {
        marks.refresh(doc, from)?;
        // Comments / PIs / CDATA are TEXT. `crs_scope_inner` and
        // `top_level_owned_spans` already skip all three; this scanner did
        // not, so a sidecar carrying `</rdf:Description>` in a comment
        // reported a bogus close, the body came back truncated mid-construct,
        // and the whole merge fell back to a fresh document — dropping every
        // Lightroom-only property it exists to preserve. (Attribute values
        // cannot trigger it: raw `<` is illegal there in XML, and
        // `scan_tag_end` is quote-aware regardless.)
        if let Some((at, open, close)) = marks.pending_construct() {
            let body_at = at + open.len();
            // An UNTERMINATED construct walks `from` off the end, so the next
            // refresh finds no close and the scope sinks to the whole-document
            // fallback: unbalanced markup is never silently read as tags.
            from = match doc[body_at..].find(close) {
                Some(end_rel) => body_at + end_rel + close.len(),
                None => doc.len(),
            };
            continue;
        }
        match marks.open {
            Some(open_at) if open_at < marks.close => {
                let (end, self_closing) = scan_tag_end(doc, open_at)?;
                if !self_closing {
                    depth += 1;
                }
                from = end + 1;
            }
            _ => {
                if depth == 0 {
                    return Some(marks.close);
                }
                depth -= 1;
                from = marks.close + CLOSE.len();
            }
        }
    }
}

/// Re-stamp the AutoShade rationale comment (older saves embedded it) so a
/// merged document never carries a STALE rationale for a new recipe.
fn refresh_rationale_comment(doc: String, r: &EditRecipe) -> String {
    const MARK: &str = "<!-- Generated by AutoShade. AI rationale: ";
    // Also an ON-DISK token: sidecars written before the rename carry the
    // pre-rename mark, and failing to find it leaves a STALE rationale
    // attached to a NEW recipe — the exact defect this function exists to
    // prevent. Found under either spelling, always rewritten under the
    // current one.
    const MARK_PRE_RENAME: &str = "<!-- Generated by Autoshop. AI rationale: ";
    let Some(start) = doc.find(MARK).or_else(|| doc.find(MARK_PRE_RENAME)) else { return doc };
    let Some(end) = doc[start..].find("-->") else { return doc };
    format!(
        "{}{MARK}{} (confidence {:.2}) {}",
        &doc[..start],
        safe_rationale(r),
        r.confidence,
        &doc[start + end..]
    )
}

/// The element name in a start or end tag: `<crs:Exposure2012 xml:lang="…">`
/// and `</crs:Exposure2012>` both give `crs:Exposure2012`.
fn tag_name(tag: &str) -> &str {
    let t = tag.trim_start_matches('<').trim_start_matches('/');
    let end = t.find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(t.len());
    &t[..end]
}

/// The start of the close tag ending the element whose open tag ends at
/// `open_gt` — matched by NAME through [`next_xml_tag`], so the
/// whitespace-carrying close (`</crs:Key >`, legal XML) and closes quoted
/// inside comments/CDATA/PIs are both handled, and same-name nesting is
/// depth-counted. `None` = the element never closes.
fn element_close_start(doc: &str, name: &str, open_gt: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut from = open_gt + 1;
    while let Some((start, end, self_closing)) = next_xml_tag(doc, from) {
        let tag = &doc[start..=end];
        if tag_name(tag) == name {
            if tag.starts_with("</") {
                if depth == 0 {
                    return Some(start);
                }
                depth -= 1;
            } else if !self_closing {
                depth += 1;
            }
        }
        from = end + 1;
    }
    None
}

/// The body of the first `<{name}>…</{name}>` element in `scope`, matched by
/// tag NAME — so the attribute-carrying spelling
/// (`<crs:ToneCurvePV2012 xml:lang="x-default">`) and a whitespace close both
/// resolve to the same property, exactly as the writer's own strip does
/// (see [`top_level_owned_spans`]; the literal predecessor here was the
/// reader's last exact-string holdout, and every miss read as "absent").
///
/// `Ok(None)` = no such element. `Err(())` = the element OPENS but never
/// closes — present-but-unreadable, which callers must disclose rather than
/// fold into "absent" (a curve that cannot be read imports as a silent
/// neutral, and the next save persists the neutral).
pub(crate) fn owned_element_body<'a>(scope: &'a str, name: &str) -> Result<Option<&'a str>, ()> {
    Ok(owned_element_body_span(scope, name)?.map(|(a, b)| &scope[a..b]))
}

/// [`owned_element_body`]'s answer as a byte SPAN `[start, end)` into `scope`,
/// with the same three verdicts and the same reasons.
///
/// Exists because a caller that has to hand offsets back in the ORIGINAL
/// string's coordinates cannot use a subslice: `base_geometry_at` returns a
/// position inside the whole correction segment while it must SEARCH only the
/// component list (R27 Batch-4 hazard 2), and re-deriving the offset from a
/// slice's address is pointer arithmetic dressed up as parsing.
/// [`owned_element_body`] is written in terms of this one so the two can never
/// answer differently.
fn owned_element_body_span(scope: &str, name: &str) -> Result<Option<(usize, usize)>, ()> {
    let mut from = 0;
    while let Some((start, end, self_closing)) = next_xml_tag(scope, from) {
        let tag = &scope[start..=end];
        if !tag.starts_with("</") && tag_name(tag) == name {
            if self_closing {
                // An empty body, spelled as the empty span just past the tag —
                // `&scope[end+1..end+1]` is `""`, which is what the slicing
                // form has always returned here.
                return Ok(Some((end + 1, end + 1)));
            }
            return match element_close_start(scope, name, end) {
                Some(close) => Ok(Some((end + 1, close))),
                None => Err(()),
            };
        }
        from = end + 1;
    }
    Ok(None)
}

/// One `crs:What`-carrying component found by [`components_in`].
///
/// The `crs:What` may sit on the `<rdf:li>` itself (Lightroom's attribute form
/// for parametric shapes) or on an `<rdf:Description>` inside it (the element
/// form every `Mask/Aggregate`, `Mask/Paint` and `Mask/Image` uses). Both are
/// the same thing to every caller here, so both arrive as one struct.
struct XmlComponent<'a> {
    /// The tag that carries `crs:What`, `<` to `>` inclusive.
    tag: &'a str,
    /// `<` offset of that tag inside the walked body.
    start: usize,
    /// `>` offset of that tag inside the walked body.
    gt: usize,
    self_closing: bool,
    /// How many component elements are OPEN above this one: `0` = a direct
    /// member of the walked list, `1` = a member of a list nested inside one
    /// component (a `Mask/Paint` inside a `Mask/Aggregate`'s `crs:Masks`), and
    /// so on.
    depth: usize,
    what: std::borrow::Cow<'a, str>,
}

/// Every `crs:What` component in `body`, in document order, each tagged with
/// its NESTING DEPTH — the walk R27 Batch-4 replaced a flat scan with, and the
/// reason it had to.
///
/// `classify_correction` used to walk the mask block with a bare
/// [`next_xml_tag`] loop, which sees a `Mask/Paint` inside a `Mask/Aggregate`
/// as a SIBLING of it. That was harmless only while both answers were "refuse
/// the correction": the moment a Paint means something, a flat walk
/// double-counts every stroke as a top-level component and loses the group's
/// own blend mode. Nesting-awareness is step 0 of the brush arm, not a
/// tidy-up.
///
/// Malformed markup degrades toward REFUSAL rather than toward a wrong answer:
/// a close tag that does not match the innermost open element pops nothing
/// (the same recovery [`find_crs_description`] takes), so depth stays high and
/// the components below it read as nested — which every caller here refuses.
fn components_in(body: &str) -> Vec<XmlComponent<'_>> {
    let mut out: Vec<XmlComponent<'_>> = Vec::new();
    // (element name, does it contribute a level of component depth?)
    let mut stack: Vec<(&str, bool)> = Vec::new();
    let mut depth = 0usize;
    let mut from = 0;
    while let Some((start, gt, self_closing)) = next_xml_tag(body, from) {
        let tag = &body[start..=gt];
        from = gt + 1;
        let name = tag_name(tag);
        if tag.starts_with("</") {
            if stack.last().is_some_and(|(n, _)| *n == name)
                && let Some((_, was_component)) = stack.pop()
                && was_component
            {
                depth -= 1;
            }
            continue;
        }
        let what = xml_attribute_raw(tag, "crs:What").map(|(_, raw)| xml_unescape(raw));
        if let Some(w) = &what {
            out.push(XmlComponent {
                tag,
                start,
                gt,
                self_closing,
                depth,
                what: w.clone(),
            });
        }
        if !self_closing {
            stack.push((name, what.is_some()));
            if what.is_some() {
                depth += 1;
            }
        }
    }
    out
}

/// The BODY of one component's own element — `Ok(None)` when the component is
/// self-closing (Lightroom's attribute form, which has no body by
/// construction), `Err(())` when it opens and never closes.
fn component_body<'a>(scope: &'a str, c: &XmlComponent<'_>) -> Result<Option<&'a str>, ()> {
    if c.self_closing {
        return Ok(None);
    }
    match element_close_start(scope, tag_name(c.tag), c.gt) {
        Some(close) => Ok(Some(&scope[c.gt + 1..close])),
        None => Err(()),
    }
}

/// Byte spans of the body's TOP-LEVEL owned property elements, in reverse
/// document order so the caller can splice them out without re-indexing.
///
/// DEPTH-AWARE, and matched by tag NAME. A flat `<crs:Name>` literal scan
/// reached INSIDE the creative Look this merge exists to preserve: Adobe
/// writes a profile's baked parameters as owned-LOOKING children of a nested
/// `rdf:Description` (`<crs:Look><rdf:Description><crs:Parameters>
/// <rdf:Description><crs:Exposure2012>…`), and stripping those gutted the
/// Look — verified by a probe on that exact shape. An owned property belongs
/// to THIS Description; anything deeper belongs to its container. Matching by
/// name also catches the attribute-carrying spelling
/// (`<crs:Exposure2012 xml:lang="x-default">`), which the literal missed —
/// leaving behind the very duplicate the element strip exists to prevent.
///
/// `None` = markup this scanner cannot account for (an unterminated tag, a
/// close with no open, an owned element that never closes). The merge then
/// bails and the caller regenerates the document, which is the pre-merge
/// behaviour.
fn top_level_owned_spans(
    body: &str,
    owned: &std::collections::HashSet<String>,
) -> Option<Vec<(usize, usize)>> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut depth = 0usize;
    // The owned element currently open AT TOP LEVEL: (span start, tag name).
    let mut open: Option<(usize, String)> = None;
    let mut i = 0usize;
    while let Some(rel) = body[i..].find('<') {
        let p = i + rel;
        let rest = &body[p..];
        if let Some(after) = rest.strip_prefix("<!--") {
            i = p + 4 + after.find("-->")? + 3;
            continue;
        }
        if let Some(after) = rest.strip_prefix("<?") {
            i = p + 2 + after.find("?>")? + 2;
            continue;
        }
        // CDATA is TEXT, not markup: its `<`/`>` must not be counted as tags.
        // Counting them left `depth` unbalanced, which bails the whole merge
        // into a full regenerate — and that path replaces the user's sidecar
        // with our own document, taking every foreign property with it. Legal
        // XML must never reach the bail.
        if let Some(after) = rest.strip_prefix("<![CDATA[") {
            i = p + "<![CDATA[".len() + after.find("]]>")? + 3;
            continue;
        }
        let (gt, self_closing) = scan_tag_end(body, p)?;
        let name = tag_name(&body[p..=gt]).to_string();
        if rest.starts_with("</") {
            // A close with no open: not markup this scanner can splice.
            depth = depth.checked_sub(1)?;
            if depth == 0
                && let Some((start, open_name)) = open.take()
            {
                if name != open_name {
                    return None;
                }
                spans.push((start, gt + 1));
            }
            i = gt + 1;
            continue;
        }
        if depth == 0
            && let Some(bare) = name.strip_prefix("crs:")
            && owned.contains(bare)
        {
            // The leading indentation (and the newline before it) goes with
            // the property — the same whitespace hygiene the attribute strip
            // applies, so an untouched document's formatting is preserved.
            let mut start = p;
            while start > 0 && matches!(body.as_bytes()[start - 1], b' ' | b'\t') {
                start -= 1;
            }
            if start > 0 && body.as_bytes()[start - 1] == b'\n' {
                start -= 1;
            }
            if self_closing {
                spans.push((start, gt + 1));
            } else {
                open = Some((start, name.clone()));
            }
        }
        if !self_closing {
            depth += 1;
        }
        i = gt + 1;
    }
    if open.is_some() || depth != 0 {
        return None; // unbalanced: regenerate rather than splice blind
    }
    spans.reverse();
    Some(spans)
}

/// The crs Description's OWN property scope — the text every whole-document
/// READ below must be restricted to: its opening tag plus the top-level
/// children that carry ITS settings.
///
/// The mirror of [`top_level_owned_spans`], for the reader. Adobe writes a
/// creative profile's baked parameters as owned-LOOKING children of a NESTED
/// `rdf:Description` (`<crs:Look><rdf:Description><crs:Parameters>
/// <rdf:Description><crs:Clarity2012>+50…`), and the scanners here
/// ([`crs_str`], [`parse_curve`], [`parse_masks`]) match by name anywhere in
/// the string they are given. Whenever the top-level Description OMITS a key
/// the Look nests, the flat scan therefore answered from the profile — the
/// import turned a camera profile's baked look into user slider values, and
/// the next save persisted them. The WRITER's depth-aware strip exists for
/// exactly this shape; the reader now shares the rule.
///
/// A top-level child that nests an `rdf:Description` is a CONTAINER of
/// someone else's settings and is dropped. `crs:MaskGroupBasedCorrections`
/// nests them too but IS this Description's own property (its nested
/// Descriptions are its mask items), so it is kept by name — dropping it
/// would blind [`parse_masks`].
///
/// `None` = markup this scanner cannot account for; [`crs_own_scope`] then
/// hands back the whole document, which is exactly the pre-fix behaviour.
fn crs_scope_inner(doc: &str) -> Option<String> {
    /// Owned containers that legitimately nest `rdf:Description`.
    const KEEP_NESTED: [&str; 1] = ["crs:MaskGroupBasedCorrections"];
    let start = find_crs_description(doc)?;
    element_own_scope(doc, start, |name, nests_description| {
        !nests_description || KEEP_NESTED.contains(&name)
    })
}

/// One CORRECTION's own property scope — the same law, one level down (R28
/// Batch-5 5d).
///
/// A `crs:What="Correction"` element carries its sliders as attributes on its
/// own tag and its four point curves as child elements, and it carries its
/// COMPONENTS inside `crs:CorrectionMasks`. Every slider read used to scan the
/// whole correction segment, so an attribute named `crs:LocalExposure2012` on a
/// nested `Mask/Paint` — or on anything else inside that component list —
/// answered for the correction whenever the correction itself omitted the key.
///
/// Zero real Lightroom files do that (LR writes the Local* family on the
/// Correction tag and nothing else carries those names), which is why the
/// adjudication rated it LOW and adversarial-input-only. But the nearest real
/// threat is already in the corpus: Lightroom really does put `crs:Local*`
/// NAMES on nested `Mask/Image` components (`LocalInputDigest` and friends, 105
/// measured instances), and today's reader survives that only because those
/// values are strings nobody parses as a number — a coincidence, not a guard.
///
/// The member rule is the difference from [`crs_scope_inner`]: at the top level
/// a child is foreign when it NESTS a settings block; inside a correction the
/// component list is foreign BY NAME, because its members are `rdf:li`s that may
/// carry no `rdf:Description` at all (Lightroom's attribute form for parametric
/// shapes) and would otherwise stay in scope.
fn correction_own_scope(seg: &str) -> std::borrow::Cow<'_, str> {
    let start = match next_xml_tag(seg, 0) {
        Some((s, _, _)) => s,
        None => return std::borrow::Cow::Borrowed(seg),
    };
    match element_own_scope(seg, start, |name, nests_description| {
        !nests_description && name != "crs:CorrectionMasks"
    }) {
        Some(s) => std::borrow::Cow::Owned(s),
        // Markup this scanner cannot account for: fall back to the whole
        // segment, which is the pre-5d behaviour and the same direction
        // `crs_own_scope` degrades in. A correction this malformed is already
        // heading for a refusal in `classify_correction`.
        None => std::borrow::Cow::Borrowed(seg),
    }
}

/// The shared walk behind [`crs_scope_inner`] and [`correction_own_scope`]: an
/// element's opening tag plus the top-level children `keep` accepts, with
/// everything else dropped. `keep(child element name, does it nest an
/// `rdf:Description`)`.
///
/// TWO membership rules on ONE walk, deliberately — the same shape R28 Batch-3
/// gave the rotate/storage raster sets. The alternative was a second copy of
/// this scanner with one predicate changed, which is how the reader's scope law
/// came to hold at the document level and not at the correction level in the
/// first place.
fn element_own_scope(
    doc: &str,
    start: usize,
    keep: impl Fn(&str, bool) -> bool,
) -> Option<String> {
    let (gt, self_closing) = scan_tag_end(doc, start)?;
    let mut out = doc[start..=gt].to_string();
    if self_closing {
        return Some(out); // every setting is an attribute — no children at all
    }
    let close = find_matching_close(doc, gt + 1)?;
    let body = &doc[gt + 1..close];
    let mut depth = 0usize;
    // The top-level child currently open: (span start, tag name, nests an
    // rdf:Description).
    let mut open: Option<(usize, String, bool)> = None;
    let mut i = 0usize;
    while let Some(rel) = body[i..].find('<') {
        let p = i + rel;
        let rest = &body[p..];
        // Comments / PIs / CDATA are TEXT — their `<`/`>` are not tags (the
        // same three skips top_level_owned_spans makes, for the same reason:
        // counting them unbalances `depth` and bails the whole scope).
        if let Some(after) = rest.strip_prefix("<!--") {
            i = p + 4 + after.find("-->")? + 3;
            continue;
        }
        if let Some(after) = rest.strip_prefix("<?") {
            i = p + 2 + after.find("?>")? + 2;
            continue;
        }
        if let Some(after) = rest.strip_prefix("<![CDATA[") {
            i = p + "<![CDATA[".len() + after.find("]]>")? + 3;
            continue;
        }
        let (gt2, self_closing) = scan_tag_end(body, p)?;
        let name = tag_name(&body[p..=gt2]).to_string();
        if rest.starts_with("</") {
            depth = depth.checked_sub(1)?; // a close with no open: unaccountable
            if depth == 0
                && let Some((s, open_name, nested)) = open.take()
            {
                // CROSSED NAMES (`<crs:Look>…</crs:Foo>`): the tag counts
                // balance, the names do not, so this child is markup we cannot
                // account for. It is DROPPED, not bailed on — R25 P8. Bailing
                // returned `None`, and `crs_own_scope`'s `None` hands the
                // scanners THE WHOLE DOCUMENT, which promotes a creative
                // Look's baked `crs:Clarity2012` to a user slider value: the
                // precise defect this function exists to prevent, reachable
                // through its own error path. The writer's mirror
                // (`top_level_owned_spans`) does not bail on this shape at all
                // — it only tracks OWNED children — so the merge went ahead
                // while the read went wrong, and the asymmetry was the bug.
                // Dropping is the safe direction for a READ scope: the worst
                // it can do is leave a property unread, and it can never let a
                // nested settings block answer for the top level.
                if name == open_name && keep(&open_name, nested) {
                    out.push('\n');
                    out.push_str(&body[s..=gt2]);
                }
            }
            i = gt2 + 1;
            continue;
        }
        if depth == 0 {
            if self_closing {
                // No children to inspect — a bare property element is ours,
                // unless the member rule refuses it by NAME (an empty
                // `<crs:CorrectionMasks/>` is still the component list).
                if keep(&name, false) {
                    out.push('\n');
                    out.push_str(&body[p..=gt2]);
                }
            } else {
                open = Some((p, name.clone(), false));
            }
        } else if name == "rdf:Description"
            && let Some(o) = open.as_mut()
        {
            o.2 = true; // this top-level child nests a foreign settings block
        }
        if !self_closing {
            depth += 1;
        }
        i = gt2 + 1;
    }
    if open.is_some() || depth != 0 {
        return None;
    }
    Some(out)
}

/// [`crs_scope_inner`] with the whole-document fallback — what every reader
/// that is handed a complete sidecar must pass to the scanners. Borrowed on
/// the fallback path, so an unmergeable/unparseable document costs nothing.
pub(crate) fn crs_own_scope(xmp: &str) -> std::borrow::Cow<'_, str> {
    if xmp.len() > MAX_XMP_BYTES {
        return std::borrow::Cow::Borrowed("");
    }
    match crs_scope_inner(xmp) {
        Some(s) => std::borrow::Cow::Owned(s),
        None => std::borrow::Cow::Borrowed(xmp),
    }
}



/// Graft `r`'s owned settings INTO an existing sidecar document, preserving
/// every property AutoShade does not model — Lightroom-only globals
/// (Texture), the camera profile / creative Look, Lightroom's lens-profile
/// block, foreign namespaces, the xpacket wrapper. Save XMP used to
/// REGENERATE the whole document, so copying it beside the RAW wiped all of
/// those from Lightroom's own file (A11).
///
/// `None` means "not safely mergeable" (no crs Description, or markup this
/// scanner cannot splice) — the caller falls back to a fresh document,
/// which is exactly the old behaviour.
///
/// Fully supported masks are replaced wholesale as one block. If any
/// correction is unsupported or partial AND the recipe has no masks of its
/// own, the original block remains byte-for-byte in the existing body and no
/// mask projection is prepended; mixing the two would silently turn an
/// unknown composition into an approximation. When the recipe DOES have
/// masks, the recipe wins — the save in hand IS the newest intent, and
/// keeping the base's foreign block instead published a document whose masks
/// were an older pass's while the develop's own never appeared, with no note
/// (L05#4). The base's block is then dropped from the OUTPUT ONLY (the file
/// it came from is not touched here) and the loss is named in
/// [`MergeOutcome::notes`].
///
/// Owned scalar crs
/// properties are stripped in BOTH forms — attribute and property-element
/// (`<crs:Exposure2012>…</crs:Exposure2012>`, a form Lightroom really
/// writes and the reader really accepts); unowned properties survive in
/// either form. Matching is
/// by the CONVENTIONAL prefixes (`rdf:`, `crs:`) — a document binding either
/// namespace to another prefix (or those prefixes to another URI) is REFUSED
/// here by [`xmlns_conflict`], so the caller regenerates and discloses.
/// Degrading instead spliced a second, contradictory settings block into the
/// user's file behind a clean "saved".
pub struct MergeOutcome {
    pub doc: String,
    /// Losses a SUCCESSFUL merge could not avoid, for the caller's note
    /// channel (the whole-document fallback is disclosed by the caller's own
    /// regeneration note; these are the losses that happen inside a merge
    /// that returns `Some`).
    pub notes: Vec<String>,
    /// The writer's per-mask verdicts for THIS recipe, produced by the same
    /// pass that emitted the mask block ([`owned_children`]) — so a caller that
    /// discloses them does not run the projection a second time (R22 NIT-1).
    /// Identical to [`mask_export_losses`] on the same recipe.
    pub losses: Vec<MaskLoss>,
}

/// What the merge REMOVES from the base document before writing `r`'s own
/// properties back — [`owned_attr_keys`], minus the pass-through keys this
/// particular recipe knows nothing about (R25 B4).
///
/// Owning a key normally means the recipe SPEAKS for it: a grain slider back
/// at 0 means "no grain", the writer omits the key, and the merge's strip is
/// what makes the removal real. A pass-through property has no such state.
/// The map is filled ONLY by reading a document, there is no control that can
/// empty it, and an empty entry therefore never means "the user cleared the
/// camera profile" — it means this recipe came from somewhere that never saw
/// one (a v0.30 `recipe.json` written before the field existed, a paste from
/// another photo, a fresh Analyze). Stripping on that would delete the
/// photographer's Upright correction and camera profile from the file beside
/// their RAW, silently, on an ordinary Ctrl+S — the exact defect class this
/// round is closing, not opening.
///
/// Absent from the map ⇒ not stripped ⇒ the base's own bytes stand. Present
/// ⇒ stripped and rewritten verbatim. Either way exactly one copy survives,
/// which is the duplicate-attribute rule the strip exists for.
fn merge_strip_keys(r: &EditRecipe) -> Vec<String> {
    let era_gated = era_suppressed_attr_keys(r);
    owned_attr_keys()
        .into_iter()
        .filter(|k| !PASSTHROUGH_CRS.contains(&k.as_str()) || r.passthrough.contains_key(k))
        .filter(|k| !era_gated.contains(k.as_str()))
        .collect()
}

/// The crs ATTRIBUTE keys R25 added to this writer's ownership, paired with the
/// registry row that carries each — DERIVED, never hand-copied, because a
/// hand-copied list of twenty-seven spellings is a list that drifts.
///
/// Two arms, and the second is why this cannot simply be "the CarriedOnly
/// rows":
///
///   * every `Tier::CarriedOnly` attribute row — the nine B2 effects, the eight
///     B3 detail axes, the auto-CA switch and the six de-fringe keys (24). The
///     tier is R25's own invention and has no pre-R25 members, so it IS the
///     batch.
///   * `texture`, `ca_r`, `ca_b` (3) — R25 keys whose control RENDERS, so the
///     tier cannot name them. Spelled out with the reason rather than inferred
///     from a date nothing in the tree records.
///
/// The B4 PASS-THROUGH sixteen are deliberately absent: they have a stronger
/// law of their own in [`merge_strip_keys`] (present in the map ⇒ ours to
/// rewrite, absent ⇒ never touched), which already answers the question this
/// gate exists for, and answers it for every era.
///
/// Pinned at exactly twenty-seven by
/// `the_era_gate_is_the_twenty_seven_keys_r25_added`.
fn r25_attr_keys() -> Vec<(&'static str, &'static str)> {
    use crate::advisor::catalogue::{Tier, RECIPE_CONTROLS};
    /// The R25 keys the tier cannot name, because their control renders.
    const RENDERED_R25: [&str; 3] = ["texture", "ca_r", "ca_b"];
    RECIPE_CONTROLS
        .iter()
        .filter(|c| c.tier == Some(Tier::CarriedOnly) || RENDERED_R25.contains(&c.name))
        .filter_map(|c| c.crs.attr().map(|k| (c.name, k)))
        .collect()
}

/// The keys a merge must neither STRIP nor EMIT for `r`, because `r` has never
/// seen them — the [`crate::recipe::SCHEMA_ERA`] gate.
///
/// **The defect this closes** (R25 P8, one root cause with the mask-block arm
/// in [`merge_recipe_into_xmp`]): a `recipe.json` written by v0.30 has no key
/// for any of the twenty-seven above, so serde fills them from
/// [`EditRecipe::default`] and the recipe "says" texture 0, no grain, no
/// sharpening radius. Owning a key means the merge STRIPS it before writing
/// ours back, and the writer omits a slider at rest — so an ordinary Ctrl+S on
/// such a recipe deleted `crs:Texture="-20"`, the whole Grain block and the
/// PostCrop/SharpenRadius keys out of the photographer's own Lightroom sidecar,
/// silently, with nothing on screen. Measured on the reference library: three
/// of seven files lost nine keys each. "This file has never held that key" is
/// not "the photographer cleared it", and only the era stamp can tell them
/// apart.
///
/// PER KEY, not per recipe, and that is not a softening — it is what keeps the
/// gate from becoming the next silent loss. An era-0 recipe whose Texture the
/// user has just dragged to +20 differs from the untouched default, and that
/// value IS a statement: suppressing it would mean a legacy photo could never
/// write Texture to its sidecar again, permanently, because nothing ever
/// re-stamps the era of a file. So the gate covers only keys still sitting
/// exactly where serde left them.
///
/// The de-fringe six move as ONE BLOCK: the writer emits all six or none
/// (`owned_attrs` states why — a hue window with no amount beside it is a shape
/// no real document has), so a gate that released three of them would publish
/// exactly that shape.
fn era_suppressed_attr_keys(r: &EditRecipe) -> std::collections::BTreeSet<&'static str> {
    use crate::advisor::catalogue::global_value;
    if r.schema_era >= crate::recipe::SCHEMA_ERA {
        return Default::default();
    }
    let neutral = EditRecipe::default();
    let keys = r25_attr_keys();
    let untouched = |name: &str| global_value(r, name) == global_value(&neutral, name);
    let defringe = |name: &str| name.starts_with("defringe");
    let defringe_untouched = keys.iter().filter(|(n, _)| defringe(n)).all(|(n, _)| untouched(n));
    keys.iter()
        .filter(|(n, _)| if defringe(n) { defringe_untouched } else { untouched(n) })
        .map(|(_, k)| *k)
        .collect()
}

pub fn merge_recipe_into_xmp(existing: &str, r: &EditRecipe) -> Option<MergeOutcome> {
    merge_recipe_into_xmp_in_frame(existing, r, None)
}

/// Which frame a MERGE writes its geometry in, and whether this writer must
/// declare that frame itself (R27 A8).
///
/// The rule is that the coordinates and the declaration can never disagree:
///
/// * the base declares a usable frame ⇒ that one wins and needs no help. It is
///   what Lightroom itself measured this file's coordinates against, and a
///   sidecar and its photo can legitimately disagree (a re-crop, a proxy).
/// * the base mentions `tiff:` but declares no usable pair ⇒ we must not add
///   attributes to its tag (a second `xmlns:tiff`, or a second
///   `tiff:ImageWidth`, is not well-formed XML), so the geometry goes out in
///   the DISPLAY frame — which is what a document that declares nothing means
///   by its own numbers, and what every build before R27 wrote.
/// * the base is silent about `tiff:` ⇒ the photo's SOURCE frame, declared.
///   This is the arm that makes a portrait photo's merged sidecar readable:
///   without the declaration the numbers would be sensor-frame in a document
///   that claims nothing, and our own reader would take them for display-frame.
fn merge_frame(existing: &str, photo: Option<FrameAspect>) -> (Option<FrameAspect>, bool) {
    if let Some(base) = FrameAspect::from_xmp(existing) {
        return (Some(base), false);
    }
    let touched = ["xmlns:tiff", "tiff:ImageWidth", "tiff:ImageLength", "tiff:Orientation"]
        .iter()
        .any(|k| find_outside_constructs(existing, k).is_some());
    if touched {
        (photo.map(|f| f.displayed()), false)
    } else {
        (photo, photo.is_some())
    }
}

/// [`merge_recipe_into_xmp`] with a FALLBACK frame — the photo's own aspect,
/// for the radial projection ([`FrameAspect`]). The base document's own
/// `tiff:ImageWidth/ImageLength` still wins when it has them: those are what
/// Lightroom itself measured this file's mask coordinates against, and a
/// sidecar and its photo can legitimately disagree (a re-crop, a proxy).
pub fn merge_recipe_into_xmp_in_frame(
    existing: &str,
    r: &EditRecipe,
    frame: Option<FrameAspect>,
) -> Option<MergeOutcome> {
    merge_recipe_into_xmp_in_frame_for_photo(existing, r, frame, None)
}

/// Path-aware merge used when the base can carry MaskBrushTable references.
/// An unchanged imported mask compares through the same ACR-backed reader and
/// therefore keeps the base mask block verbatim instead of synthesizing Paints.
pub fn merge_recipe_into_xmp_in_frame_for_photo(
    existing: &str,
    r: &EditRecipe,
    frame: Option<FrameAspect>,
    photo: Option<&std::path::Path>,
) -> Option<MergeOutcome> {
    let (frame, declare_frame) = merge_frame(existing, frame);
    if existing.len() > MAX_XMP_BYTES {
        return None;
    }
    if xmlns_conflict(existing).is_some() {
        return None;
    }
    let mut notes: Vec<String> = Vec::new();
    let Some(desc_start) = find_crs_description(existing) else {


        // A well-formed sidecar that simply carries no camera-raw settings —
        // exiftool / Bridge / Capture One ratings and keywords are the common
        // case. Regenerating over it drops those properties, which the loss
        // note then reported truthfully on EVERY save, forever, with no action
        // the user could take. There is nothing of ours to splice INTO, but
        // there is somewhere to put it: adding our own Description to the
        // existing `rdf:RDF` keeps the file verbatim and makes the merge real.
        return insert_crs_description(existing, r, frame)
            .map(|(doc, losses)| MergeOutcome { doc, notes, losses });
    };
    let (gt, self_closing) = scan_tag_end(existing, desc_start)?;

    // The opening tag: strip every owned crs attribute, then append ours.
    // BOTH quote styles: single-quoted attributes are legal XML, and leaving
    // one behind would duplicate the attribute we append.
    let mut tag = existing[desc_start..=gt].to_string();
    for key in merge_strip_keys(r) {
        let name = format!("crs:{key}");
        while let Some((span, _)) = xml_attribute_raw(&tag, &name) {
            let mut left = span.start;
            while left > 0 && tag.as_bytes()[left - 1].is_ascii_whitespace() {
                left -= 1;
            }
            tag.replace_range(left..span.end, "");
        }
    }


    // The recipe in the frame the OUTPUT document will be read in (R27) — the
    // base's own declaration when it has one, and it is the base's tag we are
    // rewriting, so the turn has to match what that tag says.
    let turned = in_source_frame(r, frame);
    let r = turned.as_ref();

    let closing_len = if self_closing { 2 } else { 1 };
    let head = tag[..tag.len() - closing_len].trim_end().to_string();
    let new_tag = format!(
        "{head}{tiff}{attrs}>",
        tiff = if declare_frame { frame_declaration(frame) } else { String::new() },
        attrs = owned_attrs(r, frame),
    );

    // The element body: drop every owned child block, then prepend ours.
    // Owned blocks never nest themselves, so a whole-span splice is safe —
    // unlike per-item surgery (the reverted batch-3 attempt).
    let (mut body, tail_start) = if self_closing {
        (String::new(), gt + 1)
    } else {
        let close = find_matching_close(existing, gt + 1)?;
        (existing[gt + 1..close].to_string(), close + "</rdf:Description>".len())
    };
    // Curve/mask child blocks AND owned scalars in PROPERTY-ELEMENT form:
    // Lightroom serialises the same settings as
    // `<crs:Exposure2012>+0.65</crs:Exposure2012>` in plenty of real
    // sidecars (crs_str reads that form for exactly that reason), so an
    // attribute-only strip left the old element value in the body beside
    // the attribute we append — one document, two conflicting answers.
    let mask_scope = crs_own_scope(existing);
    let summary = mask_summary_with_source(
        mask_scope.as_ref(),
        is_autoshade_sidecar(existing),
        frame,
        photo,
        None,
    );
    // Preserve the base's own mask block ONLY while this develop has not
    // moved away from it. The recipe in hand is the newest intent by
    // definition — it is what is being saved right now — so once it differs,
    // ours publish and the base's block goes, WITH the note below. (Ranking
    // file mtimes here instead would misfire: every save flow commits
    // recipe.json before projecting the XMP, so the store always looks newer
    // than the sidecar by the time this runs.)
    //
    // R25 P1 — THE trap of the import unlock. `r.masks.is_empty()` was a
    // usable stand-in for "the user has not touched these" only while a lossy
    // sidecar imported NOTHING. Now that it imports, the develop carries a
    // DEGRADED reading of the base's block (a rotation read as 0, a blend
    // mode ignored, a foreign range left behind), and writing that back over
    // an untouched import would delete the parts we cannot express — silently,
    // on an ordinary Ctrl+S, from the user's own Lightroom file. The question
    // is therefore "did anything move since the import", and it is answered by
    // re-reading the base through the SAME importer the develop came through:
    // equal ⇒ we would only be re-emitting our own approximation, so keep the
    // original bytes; different ⇒ the newest intent is the develop in hand, it
    // publishes, and the note below says so.
    //
    // Ordered so the extra parse is only paid where it decides something: a
    // maskless recipe short-circuits (the old arm, unchanged), and a base with
    // nothing to preserve never reaches the comparison at all.
    //
    // R25 P8 — the SECOND half of that trap, and the reason the predicate
    // moved off `summary.preserve_original` entirely. That flag is set by
    // `MaskSummary::record`, i.e. only when the import was LOSSY, and it was
    // never anything more than a stand-in for "the base has a mask block":
    // while every Lightroom block produced defects the two were the same
    // boolean. P1 made LR blocks import cleanly, and the stand-in came apart —
    // a base whose masks import PERFECTLY, merged with a recipe that has none
    // (a v0.30 `recipe.json` predates mask import; every one of them is
    // maskless), reported preserve_original false, stripped the block and
    // published nothing in its place. Measured on the reference library: four
    // corrections destroyed on one file, eight on another, with an empty note
    // list because the disclosure below was gated on the same flag. So the
    // question is asked directly — DOES THE BASE HAVE A BLOCK — and the
    // answer decides both the preserve and the note.
    let preserve_masks = summary.corrections > 0
        && (r.masks.is_empty()
            || r.masks
                == photo.map_or_else(
                    || xmp_to_recipe(existing).masks,
                    |path| xmp_to_recipe_for_photo(existing, path).masks,
                ));
    if summary.corrections > 0 && !preserve_masks {
        // Two shapes, because the trigger now has two shapes. The defect
        // clause was written when only a LOSSY block could reach here and
        // would have read "carries 0 thing(s) this build cannot represent" on
        // a block we understood perfectly — a sentence that says nothing true.
        // What is always true is the count of corrections being replaced.
        let base = if summary.defects > 0 {
            format!(
                "the merge base's mask block carries {} correction(s), {} thing(s) of which this \
                 build cannot represent",
                summary.corrections, summary.defects
            )
        } else {
            format!("the merge base's mask block carries {} correction(s)", summary.corrections)
        };
        notes.push(format!(
            "{base} — it is not in the new file, which carries this develop's {} edited mask(s) \
             instead (the base file itself is not modified)",
            r.masks.len()
        ));
    }
    // [`OWNED_ELEMENT_ONLY`] is the shared list (the import-side disclosure
    // reads the same one), minus the mask block on the arm that keeps the
    // base's foreign masks verbatim.
    let owned_elements: std::collections::HashSet<String> = OWNED_ELEMENT_ONLY
        .iter()
        .filter(|k| !(preserve_masks && **k == "MaskGroupBasedCorrections"))
        .map(|k| (*k).to_string())
        .chain(merge_strip_keys(r))
        .collect();


    // TOP LEVEL ONLY (see `top_level_owned_spans`): the previous flat scan
    // also stripped identically-named children out of the nested Look this
    // merge exists to preserve. Reverse document order — earlier spans keep
    // their indices while later ones are spliced out.
    for (start, end) in top_level_owned_spans(&body, &owned_elements)? {
        body.replace_range(start..end, "");
    }

    let mut out = String::with_capacity(existing.len() + 256);
    out.push_str(&existing[..desc_start]);
    out.push_str(&new_tag);
    let (children, losses) = owned_children(r, !preserve_masks, frame);
    out.push_str(&children);
    out.push_str(body.trim_end());
    out.push_str("\n  </rdf:Description>");
    out.push_str(&existing[tail_start..]);
    Some(MergeOutcome {
        doc: upgrade_era_marker(refresh_rationale_comment(out, r)),
        notes,
        losses,
    })
}

// ───────────────────────── XMP → EditRecipe (reader) ─────────────────────────
//
// The inverse of [`recipe_to_xmp`], so a sidecar written earlier (by us or by
// Lightroom) can be loaded back into the editor. Scan-based like the eval
// harness's parser: classic-ACR values are flat `crs:Key="value"` attributes,
// verified against the user's real LR sidecars, so plain text scanning
// round-trips everything the writer emits without an XML dependency. Fields
// classic XMP cannot carry (bitmap masks, recolour gains, mask roles) simply
// don't come back — the app-internal recipe.json is the lossless sidecar; this
// reader is the recovery path when only an XMP exists.

/// The toolkit strings this app stamps into `x:xmptk`, and the ones it
/// stamped before the AutoShade rename.
///
/// These are ON-DISK FORMAT TOKENS, not display names: every sidecar this app
/// has ever written to a user's library carries one of the pre-rename
/// spellings, and [`is_autoshade_era2`] turns that token into a RENDERING
/// decision (era-1 Temperature is relative to the 5500 K anchor, era-2 is
/// absolute). Teaching the reader only the new spelling would have re-read
/// every existing era-2 sidecar as era-1 and silently shifted its white
/// balance. The writer stamps the current spelling; the readers accept both,
/// for the same one-version grace the environment names get.
const XMPTK_ERA1: &str = "AutoShade";
const XMPTK_ERA2: &str = "AutoShade 2";
const XMPTK_ERA1_PRE_RENAME: &str = "Autoshop";
const XMPTK_ERA2_PRE_RENAME: &str = "Autoshop 2";

/// AutoShade provenance: an ATTRIBUTE-shaped `x:xmptk = "AutoShade"` /
/// `x:xmptk='AutoShade'` match (either quote style, optional whitespace
/// around `=`, and either the current or the pre-rename toolkit name),
/// searched ONLY inside the `<x:xmpmeta …>` start tag — where
/// the attribute actually lives. The old raw-substring test both missed
/// semantically identical XML spellings and matched the literal anywhere in
/// the document (a foreign sidecar's comment could claim our provenance) —
/// and this boolean decides whether an As-Shot tint imports as a real edit.
fn is_autoshade_sidecar(xmp: &str) -> bool {
    // COMMENT-AWARE scan for the first real `<x:xmpmeta` start tag: a plain
    // find lost to a forged tag in a LEADING comment, rfind to one in a
    // TRAILING comment. One pass skipping `<!-- … -->` spans settles both
    // (full XML parsing stays out of scope; this only gates the As-Shot
    // tint import).
    // BYTE scanning throughout: `&xmp[i + 1..]` PANICS when i+1 falls inside a
    // multi-byte char, and a file that opens with a UTF-8 BOM (EF BB BF) hits
    // that on the very first step. Every index this loop keeps lands on `<`
    // or just past `-->` — both ASCII — so the one str slice below is safe.
    let bytes = xmp.as_bytes();
    let mut i = 0usize;
    let mut tag_start: Option<usize> = None;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"<!--") {
            match bytes[i + 4..].windows(3).position(|w| w == b"-->") {
                Some(end) => i += 4 + end + 3,
                None => break, // unterminated comment: nothing real follows
            }
        } else if bytes[i..].starts_with(b"<x:xmpmeta")
            && bytes
                .get(i + "<x:xmpmeta".len())
                .is_none_or(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'/'))
        {
            // Name-boundary check: without it a preceding wrapper whose
            // element name merely STARTS with x:xmpmeta (`<x:xmpmetadata
            // x:xmptk="AutoShade">`) was accepted as the document tag.
            tag_start = Some(i);
            break;
        } else {
            // Advance to the next '<' (or end).
            match bytes[i + 1..].iter().position(|&c| c == b'<') {
                Some(off) => i += 1 + off,
                None => break,
            }
        }
    }
    let Some(tag_start) = tag_start else { return false };
    let tag = &xmp[tag_start..];
    let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
    let mut rest = tag;
    while let Some(i) = rest.find("x:xmptk") {
        let after = rest[i + "x:xmptk".len()..].trim_start();
        if let Some(v) = after.strip_prefix('=') {
            let v = v.trim_start();
            let ours = |r: &str, q: char| {
                [XMPTK_ERA1, XMPTK_ERA2, XMPTK_ERA1_PRE_RENAME, XMPTK_ERA2_PRE_RENAME]
                    .iter()
                    .any(|n| r.strip_prefix(n).is_some_and(|t| t.starts_with(q)))
            };
            if v.strip_prefix('"').is_some_and(|r| ours(r, '"'))
                || v.strip_prefix('\'').is_some_and(|r| ours(r, '\''))
            {
                return true;
            }
        }
        rest = &rest[i + "x:xmptk".len()..];
    }
    false
}

/// Absolute-Kelvin era marker (`x:xmptk="AutoShade 2"`): documents whose
/// Temperature is ABSOLUTE (written by the anchored engine). We only ever
/// serialise the fixed form below, so an exact scan suffices; a hand-edited
/// whitespace variant merely misses the marker and falls back to the
/// old-era 5500 pin — the fail-safe direction (renders as the old engine
/// did) — never to a wrong absolute reinterpretation.
fn is_autoshade_era2(xmp: &str) -> bool {
    [XMPTK_ERA2, XMPTK_ERA2_PRE_RENAME].iter().any(|n| {
        xmp.contains(&format!(r#"x:xmptk="{n}""#)) || xmp.contains(&format!("x:xmptk='{n}'"))
    })
}

/// Upgrade an old AutoShade era marker on a MERGED document: the merge just
/// rewrote every owned WB attribute in absolute-Kelvin semantics, so leaving
/// `x:xmptk="AutoShade"` in place would make the next import pin the 5500
/// anchor onto absolute values. Foreign (Adobe) markers are untouched —
/// foreign import semantics are already absolute.
fn upgrade_era_marker(doc: String) -> String {
    // The closing quote is part of the pattern, so an already-upgraded
    // "AutoShade 2" value can never prefix-match and double-upgrade. Both
    // era-1 spellings upgrade to the CURRENT era-2 name: a pre-rename era-1
    // document left un-upgraded would pin the 5500 anchor onto the absolute
    // values the merge just wrote.
    [XMPTK_ERA1, XMPTK_ERA1_PRE_RENAME].iter().fold(doc, |d, n| {
        d.replacen(&format!(r#"x:xmptk="{n}""#), &format!(r#"x:xmptk="{XMPTK_ERA2}""#), 1)
            .replacen(&format!("x:xmptk='{n}'"), &format!("x:xmptk='{XMPTK_ERA2}'"), 1)
    })
}

/// **The scope a `crs:` read is allowed to see — as a TYPE** (R28 Batch-5 5d,
/// adjudication F4 root 1).
///
/// Every reader below used to take a bare `&str`, and that string carried TWO
/// incompatible meanings depending on which call site produced it: "one
/// element's start tag" (read its attributes) or "a subtree" (first match
/// anywhere inside, children included). The distinction was a per-call-site
/// CONVENTION — nothing in a signature said which was meant, nothing checked,
/// and the difference is exactly the class of defect R27 Batch-4 hardened three
/// sites against by hand (`components_in`, `correction_mask_components`,
/// `base_element`) while `xmp.rs`'s own comment admitted "the older reads stay
/// on `g`" was a survey, not an invariant.
///
/// So the scope is a type now. [`Tag`] can only ever answer from ONE element's
/// attributes; [`Scope`] is the subtree search, and a call site that wants one
/// cannot silently get the other — it has to name it.
pub(crate) trait CrsSource<'a>: Copy {
    /// Raw string value of `crs:<key>` within this source's scope. The `crs:`
    /// anchor makes prefixed cousins unambiguous (`crs:Tint` can never match
    /// inside `crs:LocalTint`).
    fn crs_str(self, key: &str) -> Option<std::borrow::Cow<'a, str>>;

    /// Numeric `crs:` value, tolerating ACR's explicit `+` (`"+22"`). `None`
    /// if the key is absent, unparsable, or NON-FINITE: Rust's f32 parser
    /// accepts "NaN"/"inf", no real sidecar writer emits them, and letting one
    /// through imported a value the recipe clamp then silently neutralised
    /// WITHOUT the unparsable-number disclosure ever firing.
    fn crs_f32(self, key: &str) -> Option<f32> {
        self.crs_str(key)?
            .trim()
            .trim_start_matches('+')
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
    }
}

/// ONE element's own start tag, `<` to `>` inclusive (or a bare attribute list).
///
/// A read against a `Tag` sees that element's ATTRIBUTES and nothing else: no
/// body, no children, no siblings. It is the right scope for every per-component
/// question — "what does THIS `<rdf:li crs:What="Mask/…">` say" — and it is not
/// expressible as a subtree read, which is the point.
#[derive(Clone, Copy)]
pub(crate) struct Tag<'a>(&'a str);

/// A SUBTREE: an element and everything nested inside it, or a whole document.
///
/// A read against a `Scope` takes the FIRST match anywhere inside, children
/// included — which is what a whole-document read of `crs:Exposure2012` needs
/// (the property may be an attribute on the Description or a child element of
/// it) and what a per-element read must never get. When the enclosing element
/// has nested settings that are somebody ELSE's, the scope is narrowed BEFORE
/// it is built: [`crs_own_scope`] does that for the top-level Description and
/// [`correction_own_scope`] for one correction.
#[derive(Clone, Copy)]
pub(crate) struct Scope<'a>(&'a str);

impl<'a> Tag<'a> {
    pub(crate) fn new(tag: &'a str) -> Self {
        Tag(tag)
    }

    /// The underlying text, for the structural helpers (`tag_name`,
    /// `next_xml_attribute`) that work on markup rather than on one property.
    fn text(self) -> &'a str {
        self.0
    }
}

impl<'a> Scope<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        Scope(text)
    }

    /// The underlying text, for the structural helpers (`owned_element_body`,
    /// `parse_curve_checked`, `next_xml_tag`) that walk markup rather than read
    /// one property. `pub(crate)`: `eval`'s tone-curve reader is one of those
    /// helpers and lives in another module.
    pub(crate) fn text(self) -> &'a str {
        self.0
    }

    /// Byte offset of the first tag inside this scope carrying
    /// `crs:<key>="<wanted>"`. Subtree-wide BY DEFINITION — the callers are
    /// "does this block hold a correction" and "where is the component list's
    /// range mask", both of which are questions about a region.
    fn find_value_at(self, key: &str, wanted: &str) -> Option<usize> {
        let xmp = self.0;
        if xmp.len() > MAX_XMP_BYTES {
            return None;
        }
        let name = format!("crs:{key}");
        if !xmp.trim_start().starts_with('<')
            && let Some((_, raw)) = xml_attribute_raw(xmp, &name)
            && xml_unescape(raw).as_ref() == wanted
        {
            return Some(0);
        }

        let mut from = 0;
        while let Some((start, end, _)) = next_xml_tag(xmp, from) {
            let tag = &xmp[start..=end];
            if !tag.starts_with("</")
                && let Some((_, raw)) = xml_attribute_raw(tag, &name)
                && xml_unescape(raw).as_ref() == wanted
            {
                return Some(start);
            }
            from = end + 1;
        }
        None
    }
}

impl<'a> CrsSource<'a> for Tag<'a> {
    /// Attributes only. No tag walk, no element-body form — a start tag HAS no
    /// body, and a `Tag` built from something larger still cannot read past the
    /// first element's attributes, because that is all `xml_attribute_raw`
    /// looks at (`next_xml_attribute` stops at the tag's own `/` or `>`).
    fn crs_str(self, key: &str) -> Option<std::borrow::Cow<'a, str>> {
        if self.0.len() > MAX_XMP_BYTES {
            return None;
        }
        let name = format!("crs:{key}");
        xml_attribute_raw(self.0, &name).map(|(_, raw)| xml_unescape(raw))
    }
}

impl<'a> CrsSource<'a> for Scope<'a> {
    /// First occurrence anywhere in the subtree, in either XMP spelling: an
    /// attribute on any tag, or a `<crs:Key>…</crs:Key>` property element.
    fn crs_str(self, key: &str) -> Option<std::borrow::Cow<'a, str>> {
        let xmp = self.0;
        if xmp.len() > MAX_XMP_BYTES {
            return None;
        }
        let name = format!("crs:{key}");
        if !xmp.trim_start().starts_with('<')
            && let Some((_, raw)) = xml_attribute_raw(xmp, &name)
        {
            return Some(xml_unescape(raw));
        }

        let mut from = 0;
        while let Some((start, end, self_closing)) = next_xml_tag(xmp, from) {
            let tag = &xmp[start..=end];
            if !tag.starts_with("</") {
                if let Some((_, raw)) = xml_attribute_raw(tag, &name) {
                    return Some(xml_unescape(raw));
                }
                if tag_name(tag) == name {
                    if self_closing {
                        return Some(std::borrow::Cow::Borrowed(""));
                    }
                    // By NAME, not the literal `</crs:Key>`: `</crs:Key >` is
                    // the same close in XML, and the literal ran past it into
                    // the next occurrence (or off the document).
                    let close_at = element_close_start(xmp, &name, end)?;
                    return Some(xml_unescape(xmp[end + 1..close_at].trim()));
                }
            }
            from = end + 1;
        }
        None
    }
}



/// Owned crs settings PRESENT in a document whose value does not parse as a
/// number under [`crs_f32`]'s exact rule. Each of these imports as a SILENT
/// neutral in [`xmp_to_recipe`], and the next save then overwrites the
/// sidecar with those neutrals — so restore surfaces disclose them (GUI open
/// note, web X-Recipe-Warning, store derived-snapshot trace). String-typed
/// owned keys are exempt.
pub fn unparsable_crs_numbers(xmp: &str) -> Vec<String> {
    const STRINGY: [&str; 7] = [
        "Version",
        "ProcessVersion",
        "WhiteBalance",
        "HasCrop",
        "ToneCurveName2012",
        "HasSettings",
        // A FLAG, not a number (R25 B3). Lightroom writes 0/1, but "true" is
        // the other spelling a crs boolean takes in the wild and the reader
        // accepts both — naming it here as unparsable would be a disclosure
        // about a value that imported perfectly.
        "AutoLateralCA",
    ];
    if xmp.len() > MAX_XMP_BYTES {
        return vec!["XMP document exceeds the 16 MiB limit".to_string()];
    }
    // A foreign namespace binding means every scanner below is reading the
    // wrong (or no) property — one entry naming the binding beats a silent
    // fully-neutral import. Reaches the GUI open note, the web
    // X-Recipe-Warning and the store trace through the existing plumbing.
    if let Some(conflict) = xmlns_conflict(xmp) {
        return vec![format!("{conflict} — its camera-raw settings were not imported")];
    }

    let scope = crs_own_scope(xmp);
    // ONE `Scope` for every read below: the Description's OWN span, which is
    // what this whole-document scan has always meant (R28 Batch-5 5d makes it
    // say so in the type instead of by convention).
    let scope = Scope::new(scope.as_ref());
    let mut bad: Vec<String> = owned_attr_keys()
        .into_iter()
        .filter(|k| !STRINGY.contains(&k.as_str()))
        // The PASS-THROUGH sixteen are EXEMPT, and not as a special case — as
        // the definition of the tier. This scan exists because an owned key
        // whose value does not parse "imports as a SILENT neutral, and the
        // next save overwrites the sidecar with those neutrals". A
        // pass-through property has no neutral to import as: it is never read
        // as a number, never clamped and never replaced — it goes back out as
        // the same string, in range or out, numeric or not. Naming
        // `crs:CameraProfile="Adobe Standard"` here would be a warning about a
        // value that round-tripped perfectly (R25 B4); so would
        // `crs:PerspectiveX="-140"`, which is out of the ±100 default band
        // this scan falls back to and is a perfectly ordinary Upright result.
        .filter(|k| !PASSTHROUGH_CRS.contains(&k.as_str()))
        .filter(|k| {
            scope.crs_str(k).is_some()
                && scope
                    .crs_f32(k)
                    .is_none_or(|v| !crs_number_is_in_recipe_range(k, v))
        })
        .collect();
    for tag in [
        "ToneCurvePV2012",
        "ToneCurvePV2012Red",
        "ToneCurvePV2012Green",
        "ToneCurvePV2012Blue",
    ] {
        if parse_curve_checked(scope.text(), tag).is_err() {
            bad.push(tag.to_string());
        }
    }
    // A structurally inconsistent crop (HasCrop="True" with a missing
    // coordinate, an out-of-domain value, or inverted ordering) imports as a
    // SILENT None and the next save persists HasCrop="False" — a deletion
    // nobody asked for. Individually unparsable coordinates are named by the
    // generic scan above; absence and ordering are only visible to a check
    // of the structure as a whole (the curve rule, applied to the crop).
    if scope.crs_str("HasCrop").as_deref() == Some("True") {
        let coord = |k: &str| scope.crs_f32(k).filter(|v| (0.0..=1.0).contains(v));
        let consistent = match (
            coord("CropLeft"),
            coord("CropTop"),
            coord("CropRight"),
            coord("CropBottom"),
        ) {
            (Some(l), Some(t), Some(r), Some(b)) => l < r && t < b,
            _ => false,
        };
        if !consistent && !bad.iter().any(|k| k.starts_with("Crop")) {
            bad.push("Crop (HasCrop=\"True\" with missing or inconsistent coordinates)".to_string());
        }
    }
    bad
}

/// The band a `crs:` number must land in to be a value this app can import —
/// DERIVED from the control registry (`catalogue::RECIPE_CONTROLS`) rather than
/// restated here. A new attribute key used to need an edit in TWO places, this
/// table and [`owned_attr_keys`]; miss this one and the key still imports, but
/// [`unparsable_crs_numbers`] never checks it, so a nonsense value arrives as a
/// silent clamp with no disclosure. Now the writer's list is the only edit.
///
/// Two residues stay spelled out, because the registry states no field for
/// them:
///
///   * **the colour-grade wheels** — one registry row (`color_grade`) stands
///     for 14 attributes, so the per-wheel bands come off the field NAME
///     (`ColorGrade::clamp`: hue 0..360, sat + blending 0..100, lum + balance
///     ±100), keyed through [`COLOR_GRADE_CRS`] so a new wheel inherits them.
///   * **the crop rectangle** — one row for four 0..1 coordinates
///     (`Crop::clamp`); `CropAngle` is a scalar row of its own and derives.
///
/// Everything else falls to ±100 — what `Hsl::clamp` enforces for the 24 mixer
/// attributes and what every remaining signed slider uses.
fn crs_number_is_in_recipe_range(key: &str, value: f32) -> bool {
    use crate::advisor::catalogue::{COLOR_GRADE_CRS, RECIPE_CONTROLS};
    let grade_band = |field: &str| {
        if field.ends_with("_hue") {
            (0.0, 360.0)
        } else if field.ends_with("_sat") || field == "blending" {
            (0.0, 100.0)
        } else {
            (-100.0, 100.0)
        }
    };
    let (lo, hi) = if let Some(band) =
        RECIPE_CONTROLS.iter().find(|c| c.crs.attr() == Some(key)).and_then(|c| c.range)
    {
        band
    } else if let Some((field, _)) = COLOR_GRADE_CRS.iter().find(|(_, k)| *k == key) {
        grade_band(field)
    } else if matches!(key, "CropTop" | "CropLeft" | "CropBottom" | "CropRight") {
        (0.0, 1.0)
    } else {
        (-100.0, 100.0)
    };
    (lo..=hi).contains(&value)
}



// `crs_f32` moved onto `CrsSource` in R28 Batch-5 5d — it is the same parse
// applied to whatever `crs_str` answered, and leaving it as a free function
// taking `&str` would have kept the untyped door open beside the typed one.

/// Decode XML character references in one pass. Decoding `&amp;lt;` yields
/// `&lt;`, not `<`, because the logical value must be unescaped exactly once.
fn xml_unescape(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('&') {
        return std::borrow::Cow::Borrowed(s);
    }

    let mut out = String::with_capacity(s.len());
    let mut at = 0;
    while let Some(rel) = s[at..].find('&') {
        let amp = at + rel;
        out.push_str(&s[at..amp]);
        let Some(semi_rel) = s[amp + 1..].find(';') else {
            out.push_str(&s[amp..]);
            return std::borrow::Cow::Owned(out);
        };
        let semi = amp + 1 + semi_rel;
        let entity = &s[amp + 1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => {
                let value = if let Some(hex) =
                    entity.strip_prefix("#x").or_else(|| entity.strip_prefix("#X"))
                {
                    u32::from_str_radix(hex, 16).ok()
                } else if let Some(decimal) = entity.strip_prefix('#') {
                    decimal.parse::<u32>().ok()
                } else {
                    None
                };
                value.and_then(char::from_u32).filter(|&c| xml_char_allowed(c))
            }
        };
        if let Some(c) = decoded {
            out.push(c);
        } else {
            out.push_str(&s[amp..=semi]);
        }
        at = semi + 1;
    }
    out.push_str(&s[at..]);
    std::borrow::Cow::Owned(out)
}



/// The text between `open` and `close` (first occurrence of each, in order).
/// For NON-MARKUP text patterns only (the rationale comment scan) — element
/// lookups go through [`owned_element_body`], which matches by tag NAME so
/// attribute-carrying and whitespace-close spellings resolve; a literal
/// element scan here was the reader's silent-loss blind spot (L05#1).
fn block_between<'a>(xmp: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = xmp.find(open)? + open.len();
    let rest = &xmp[start..];
    Some(&rest[..rest.find(close)?])
}

/// Parse one `<crs:ToneCurvePV2012…>` `rdf:Seq` of `"x, y"` points back into
/// curve control points. A 2-point identity (0,0 → 255,255) collapses to empty:
/// Lightroom ALWAYS writes the master curve (even "Linear"), while our writer
/// omits empty curves — collapsing keeps a re-import equal to a recipe that
/// never touched the curve. An element that opens but never closes is `Err`,
/// not "no curve": present-but-unreadable flows into the same disclosure as a
/// value that does not parse.
fn parse_curve_checked(xmp: &str, tag: &str) -> Result<Vec<CurvePoint>, ()> {
    const MAX_CURVE_POINTS_FROM_XMP: usize = 256;
    let Some(body) = owned_element_body(xmp, &format!("crs:{tag}"))? else {
        return Ok(Vec::new());
    };

    let mut pts = Vec::new();
    // Items are matched by tag NAME through the shared tag scanner: the old
    // literal `"<rdf:li>"` split read a whitespace-spelled `<rdf:li >` item
    // as NO item at all — a present curve imported silently empty and the
    // next save deleted it, while the module's own standard (1719-1723,
    // owned_element_body) says present-but-unreadable must flow into
    // disclosure, never into silence.
    let mut from = 0;
    while let Some((start, end, self_closing)) = next_xml_tag(body, from) {
        let tag = &body[start..=end];
        if tag.starts_with("</") || tag_name(tag) != "rdf:li" {
            from = end + 1;
            continue;
        }
        if self_closing || pts.len() >= MAX_CURVE_POINTS_FROM_XMP {
            return Err(()); // an empty <rdf:li/> holds no "x, y" point
        }
        let close = element_close_start(body, "rdf:li", end).ok_or(())?;
        let mut it = body[end + 1..close].split(',');
        let x = it.next().ok_or(())?.trim().parse::<f32>().map_err(|_| ())?;
        let y = it.next().ok_or(())?.trim().parse::<f32>().map_err(|_| ())?;
        if it.next().is_some() || !x.is_finite() || !y.is_finite() {
            return Err(());
        }
        // Out-of-domain coordinates are as unparsable as non-finite ones:
        // silently saturating "999, -5" to (255, 0) imported a curve that
        // renders nearly black AND persisted it on the next save (16-lane
        // scan L05). Err flows into the same disclosure + drop path.
        let (x, y) = (x.round(), y.round());
        if !(0.0..=255.0).contains(&x) || !(0.0..=255.0).contains(&y) {
            return Err(());
        }
        pts.push(CurvePoint { input: x as u8, output: y as u8 });
        from = close + 1;
    }

    let identity = [CurvePoint { input: 0, output: 0 }, CurvePoint { input: 255, output: 255 }];
    Ok(if pts == identity { Vec::new() } else { pts })
}

fn parse_curve(xmp: &str, tag: &str) -> Vec<CurvePoint> {
    parse_curve_checked(xmp, tag).unwrap_or_default()
}



/// Local-mask corrections back from `<crs:MaskGroupBasedCorrections>` —
/// exactly what [`masks_xml`] can emit, which since R27 Batch-4 is the
/// parametric geometries PLUS `Mask/Aggregate` brush groups. What still has no
/// classic-XMP encoding is our own `Bitmap` rasters (skipped by the writer, so
/// symmetric) and Lightroom's AI `Mask/Image` masks (skipped by this reader,
/// because the sidecar carries no pixels for them to be read from).
fn parse_masks_with_source(
    xmp: &str,
    authored_by_autoshade: bool,
    frame: Option<FrameAspect>,
    photo: Option<&std::path::Path>,
    diag: Option<&crate::diag::Diag<'_>>,
) -> Vec<LocalAdjustment> {
    // Err (present-but-unterminated) imports no masks — the LOSS half of that
    // outcome is `mask_summary`'s to report, and it does.
    let Ok(Some(block)) = owned_element_body(xmp, "crs:MaskGroupBasedCorrections") else {
        return Vec::new();
    };
    let mut brush_reader = MaskBrushReader::new(photo, diag);
    mask_summary_from_block(block, authored_by_autoshade, frame, &mut brush_reader).supported
}

/// How many corrections in this sidecar produced NO mask at all — AI / depth
/// geometry Lightroom recomputes from its own model, plus the ones whose values
/// land outside this engine's model. What was missing before R25 P1 was the
/// DISCLOSURE: a user importing their own Lightroom work lost every one of
/// these with no indication anything had been dropped.
///
/// BRUSH corrections stopped being counted here in R27 Batch-4: a
/// `Mask/Aggregate` group imports, so it is no longer a refusal at all. It
/// carries a `BrushRendered` note instead, which [`import_losses`] reports and
/// this counter deliberately does not (see below).
///
/// COUNTS DROPS ONLY, not every import defect (R25 P1) — a correction that
/// imports carrying a note is not a refusal, and `eval` reads
/// `imported + refused` as the size of the user's local work. The named list
/// of everything, notes included, is [`import_losses`].
pub fn unsupported_corrections(xmp: &str) -> usize {
    if xmp.len() > MAX_XMP_BYTES {
        return 0;
    }
    let authored_by_autoshade = is_autoshade_sidecar(xmp);
    let scope = crs_own_scope(xmp);
    mask_summary(scope.as_ref(), authored_by_autoshade, FrameAspect::from_xmp(xmp)).dropped
}

pub fn unsupported_corrections_for_photo(xmp: &str, photo: &std::path::Path) -> usize {
    if xmp.len() > MAX_XMP_BYTES {
        return 0;
    }
    let authored_by_autoshade = is_autoshade_sidecar(xmp);
    let scope = crs_own_scope(xmp);
    mask_summary_with_source(
        scope.as_ref(),
        authored_by_autoshade,
        FrameAspect::from_xmp(xmp),
        Some(photo),
        None,
    )
    .dropped
}

/// One correction's verdict. R25 P1 retired the third state ("Partial", which
/// meant DISCARDED): a correction we can read the geometry of imports, and
/// whatever it carried that we do not model rides along as a named reason.
enum MaskCorrectionParse {
    /// BOXED, and not for style: `LocalAdjustment` passed 320 bytes when R25
    /// P6 gave every mask four point-curve vectors, while the refusal arm is
    /// one small enum — `clippy::large_enum_variant` refuses that spread, and
    /// the box is the indirection it asks for. Only `mask_summary_from_block`
    /// destructures this, and it moves the adjustment straight into a `Vec`.
    Supported(Box<LocalAdjustment>, Vec<MaskImportReason>),
    /// The one verdict that costs the whole correction — always a
    /// [`MaskImportReason::is_drop`] reason.
    Unsupported(MaskImportReason),
}

#[derive(Default)]
struct MaskSummary {
    supported: Vec<LocalAdjustment>,
    /// Every defect, NAMED — the one list every disclosure surface iterates.
    /// CAPPED: a document's corrections are unbounded and a disclosure is a
    /// sentence, not a log. The two counters beside it are exact.
    losses: Vec<MaskImportLoss>,
    /// Corrections that produced no mask, EXACTLY (past the display cap too):
    /// [`unsupported_corrections`]' answer.
    dropped: usize,
    /// Every defect, exactly — `losses.len()` before the cap. Also the
    /// answer to "was this import LOSSY", which a separate `preserve_original`
    /// boolean used to carry: it was set on exactly this condition, so the two
    /// could only ever agree, and the merge keyed on the boolean until R25 P8
    /// found it was standing in for a different question entirely (see
    /// `corrections` below). One fact, one field.
    defects: usize,
    /// How many `crs:What="Correction"` entries the BASE document's mask block
    /// holds, whatever became of them. The merge's preserve arm and its
    /// disclosure both key off THIS — "does the photographer have a mask block
    /// here", which is what the old boolean was mistaken for; see
    /// [`merge_recipe_into_xmp`] for how the two came apart in R25 P1. A block
    /// that opens and never closes counts as one: there IS a block, we simply
    /// cannot count what is in it.
    corrections: usize,
}

impl MaskSummary {
    /// The ONE door a defect enters by, so the list and the two counters
    /// cannot drift: named, and counted — drop or note.
    ///
    /// It used to raise a `preserve_original` flag here as well, which the
    /// merge read as "keep the base's own mask block". That was true only
    /// while a Lightroom block ALWAYS produced a defect; R25 P1 made those
    /// blocks import cleanly and the flag started answering "no block to
    /// keep" for a file full of them. The merge asks its own question now
    /// (`MaskSummary::corrections`), and a defect is just a defect.
    fn record(&mut self, name: &str, reason: MaskImportReason) {
        /// A sentence, not a log.
        const MAX_IMPORT_LOSSES: usize = 256;
        /// A correction name is untrusted text straight out of the sidecar
        /// (the recipe's own names are capped by `EditRecipe::clamp`; these
        /// never went through it). Truncated on a char boundary by `take`.
        const MAX_NAME_CHARS: usize = 64;
        if reason.is_drop() {
            self.dropped = self.dropped.saturating_add(1);
        }
        self.defects = self.defects.saturating_add(1);
        if self.losses.len() < MAX_IMPORT_LOSSES {
            self.losses.push(MaskImportLoss {
                name: name.chars().take(MAX_NAME_CHARS).collect(),
                reason,
            });
        }
    }
}

fn mask_summary(
    xmp: &str,
    authored_by_autoshade: bool,
    frame: Option<FrameAspect>,
) -> MaskSummary {
    mask_summary_with_source(xmp, authored_by_autoshade, frame, None, None)
}

fn mask_summary_with_source(
    xmp: &str,
    authored_by_autoshade: bool,
    frame: Option<FrameAspect>,
    photo: Option<&std::path::Path>,
    diag: Option<&crate::diag::Diag<'_>>,
) -> MaskSummary {
    match owned_element_body(xmp, "crs:MaskGroupBasedCorrections") {
        Ok(Some(block)) => {
            let mut brush_reader = MaskBrushReader::new(photo, diag);
            mask_summary_from_block(block, authored_by_autoshade, frame, &mut brush_reader)
        }
        Ok(None) => MaskSummary::default(),
        // The group OPENS but never closes: whatever corrections it holds
        // cannot be counted, so the one honest summary is "a loss, and there
        // is a block here" — the old literal finder reported this exact
        // document as zero losses and no block at all, which both hid the
        // drop from the GUI toast and told the merge it was free to delete
        // the block from the user's own sidecar.
        Err(()) => {
            let mut summary = MaskSummary::default();
            summary.record("Correction 1", MaskImportReason::OutOfModel);
            // There IS a block — that is exactly what we just failed to read
            // the end of — so the merge must keep the user's bytes rather
            // than replace an unreadable block with nothing.
            summary.corrections = 1;
            summary
        }
    }
}

/// The label this correction wears in every disclosure: its own
/// `crs:CorrectionName`, else its position in the block. Blank names fall to
/// the positional form — an empty slot in a comma list reads as a bug.
///
/// `own` is the correction's OWN scope ([`correction_own_scope`]), not the
/// whole segment: a nested component carrying a `crs:CorrectionName` would
/// otherwise be able to name the correction it sits inside.
fn correction_name(own: Scope<'_>, position: usize) -> String {
    own.crs_str("CorrectionName")
        .map(|v| v.into_owned())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| format!("Correction {position}"))
}

fn mask_summary_from_block(
    block: &str,
    authored_by_autoshade: bool,
    frame: Option<FrameAspect>,
    brush_reader: &mut MaskBrushReader<'_, '_>,
) -> MaskSummary {
    const MAX_MASKS_FROM_XMP: usize = 64;
    const DESCRIPTION_CLOSE: &str = "</rdf:Description>";

    let mut summary = MaskSummary::default();
    let mut at = 0;
    let mut seen = 0usize;
    while let Some((start, gt, self_closing)) = next_xml_tag(block, at) {
        let tag = &block[start..=gt];
        let correction = tag_name(tag) == "rdf:Description"
            && !tag.starts_with("</")
            && xml_attribute_raw(tag, "crs:What")
                .is_some_and(|(_, raw)| xml_unescape(raw).as_ref() == "Correction");
        if !correction {
            at = gt + 1;
            continue;
        }

        seen += 1;
        if self_closing {
            // No body at all: nothing to read a geometry out of.
            summary.record(&format!("Correction {seen}"), MaskImportReason::Unrepresentable);
            at = gt + 1;
            continue;
        }
        let Some(close) = find_matching_close(block, gt + 1) else {
            summary.record(&format!("Correction {seen}"), MaskImportReason::OutOfModel);
            break;
        };
        let end = close + DESCRIPTION_CLOSE.len();
        let seg = &block[start..end];
        // Built ONCE per correction and handed to both readers below, so the
        // label a disclosure prints and the values the import takes cannot come
        // from two different scopes (R28 Batch-5 5d).
        let own = correction_own_scope(seg);
        let own = Scope::new(own.as_ref());
        let name = correction_name(own, seen);
        match classify_correction(seg, own, authored_by_autoshade, frame, brush_reader) {
            MaskCorrectionParse::Supported(mask, reasons)
                if summary.supported.len() < MAX_MASKS_FROM_XMP =>
            {
                summary.supported.push(*mask);
                for reason in reasons {
                    summary.record(&name, reason);
                }
            }
            // Past the recipe's own mask cap the correction does not import,
            // which is a drop however well it parsed.
            MaskCorrectionParse::Supported(..) => {
                summary.record(&name, MaskImportReason::OutOfModel);
            }
            MaskCorrectionParse::Unsupported(reason) => summary.record(&name, reason),
        }
        at = end;
    }
    if seen == 0 && Scope::new(block).find_value_at("What", "Correction").is_some() {
        summary.record("Correction 1", MaskImportReason::OutOfModel);
        // An attribute-form group is a group: the scanner above walked past
        // it, but the document really does carry corrections and the merge's
        // preserve arm has to know.
        seen = 1;
    }
    // The fact the merge keys off, counted the same way every disclosure in
    // this module is: by the ONE pass that read the block.
    summary.corrections = seen;
    summary
}

/// Generic over the SOURCE (R28 Batch-5 5d): the correction-wide gates ask a
/// [`Scope`] and the per-component gates ask a [`Tag`], and both spellings of
/// "absent is fine, present must be in band" are the same three lines. Being
/// generic is also what stops the two from drifting into two copies with two
/// different scope habits — the shape the untyped `&str` version had.
fn optional_scaled_number_in<'a>(
    src: impl CrsSource<'a>,
    key: &str,
    scale: f32,
    lo: f32,
    hi: f32,
) -> bool {
    match src.crs_str(key) {
        None => true,
        Some(_) => src
            .crs_f32(key)
            .map(|v| v * scale)
            .is_some_and(|v| (lo..=hi).contains(&v)),
    }
}

fn optional_number_is<'a>(src: impl CrsSource<'a>, key: &str, expected: f32) -> bool {
    match src.crs_str(key) {
        None => true,
        Some(_) => src.crs_f32(key).is_some_and(|v| (v - expected).abs() <= 1e-6),
    }
}

/// The correction's own `crs:Local*` settings. `Err` refuses the WHOLE
/// correction; `Ok` imports it carrying one note per knob we do not model.
///
/// The line between the two (R25 P1): a value we can READ but that lands
/// outside the model is a refusal — importing it would render something the
/// file does not say. A knob we simply have no model FOR is not: dropping the
/// user's whole mask because it also carries a Moiré slider loses far more
/// than it protects, and the note says exactly what did not come through.
///
/// `own` is the correction's OWN scope (R28 Batch-5 5d, adjudication F4
/// symptom A) — the correction tag plus its property children, with the
/// component list cut out. It used to be the whole segment, so a nested
/// component carrying any of the names below answered for the correction
/// whenever the correction itself omitted the key: an unknown
/// `crs:LocalExposure2012` on a `Mask/Paint` was read as the correction's
/// exposure, and the very same scan then reported the correction CLEAN, because
/// the unknown-key walk below (correctly) looks only at the correction's own
/// tag. Gate and reader now share one scope, which is the invariant that was
/// missing.
fn correction_value_reasons(own: Scope<'_>) -> Result<Vec<MaskImportReason>, MaskImportReason> {
    const KNOWN_LOCAL: [&str; 28] = [
        "LocalExposure",
        "LocalHue",
        "LocalSaturation",
        "LocalContrast",
        "LocalClarity",
        "LocalSharpness",
        "LocalBrightness",
        "LocalToningHue",
        "LocalToningSaturation",
        "LocalExposure2012",
        "LocalContrast2012",
        "LocalHighlights2012",
        "LocalShadows2012",
        "LocalWhites2012",
        "LocalBlacks2012",
        "LocalClarity2012",
        "LocalDehaze",
        "LocalLuminanceNoise",
        "LocalMoire",
        "LocalDefringe",
        "LocalTemperature",
        "LocalTint",
        "LocalTexture",
        "LocalGrain",
        "LocalCurveRefineSaturation",
        // Lightroom BOOKKEEPING, not sliders — they carry no user intent, so
        // the `UnknownLocalKey` note they used to raise printed "unmodelled
        // slider" about something that is not one, on 12 of the reference
        // set's 31 importable corrections (R25 P1 round-end). Measured in the
        // user's own library: `LocalCorrectedDepth` appears 23 times and is
        // "0" every time (a numeric flag), and the two Digest keys are
        // Lightroom's own recompute ledger — a 32-hex string plus its schema
        // version ("1"). Knowing a key and not modelling it is the honest
        // answer for all three; only the numeric one can also carry a value,
        // so only it joins `INERT_LOCAL` below. The two STRING keys must stay
        // out of that list: `optional_number_is` cannot parse a hex digest, so
        // membership there would raise a false note on every file that has one.
        "LocalCorrectedDepth",
        "LocalInputDigest",
        "LocalInputDigestVersion",
    ];
    // The pre-2012 process-version twins (`LocalExposure` beside
    // `LocalExposure2012`, …) plus the bands this engine has no model for.
    // `LocalHue`/`LocalSharpness` LEFT this list in R23-1b: they have no
    // `*2012` twin, they are read back by `parse_one_correction`, and the
    // writer now emits the recipe's own values — a correction carrying them
    // is fully supported, not a partial import.
    const INERT_LOCAL: [&str; 10] = [
        "LocalExposure",
        "LocalContrast",
        "LocalClarity",
        "LocalBrightness",
        "LocalToningHue",
        "LocalToningSaturation",
        "LocalMoire",
        "LocalDefringe",
        "LocalGrain",
        // Silent at its observed "0", NAMED if a file ever carries another
        // value — the same law every other inert key follows. Being
        // bookkeeping is a reason not to call it an unknown key, not a reason
        // to stop looking at it.
        "LocalCorrectedDepth",
    ];

    if !matches!(
        own.crs_str("CorrectionActive").as_deref(),
        None | Some("true")
    ) || !optional_scaled_number_in(own, "CorrectionAmount", 1.0, 0.0, 1.0)
        || !optional_scaled_number_in(own, "LocalExposure2012", 4.0, -5.0, 5.0)
        || !optional_scaled_number_in(own, "LocalLuminanceNoise", 100.0, 0.0, 100.0)
    {
        return Err(MaskImportReason::OutOfModel);
    }

    for key in [
        "LocalContrast2012",
        "LocalHighlights2012",
        "LocalShadows2012",
        "LocalWhites2012",
        "LocalBlacks2012",
        "LocalClarity2012",
        "LocalDehaze",
        "LocalTexture",
        "LocalSharpness",
        "LocalSaturation",
        "LocalTemperature",
        "LocalTint",
    ] {
        if !optional_scaled_number_in(own, key, 100.0, -100.0, 100.0) {
            return Err(MaskImportReason::OutOfModel);
        }
    }
    // `LocalHue` left the loop above in v0.32.0: its file scale is 180, not
    // 100 (`parse_one_correction`'s `q180`). The gate has to move WITH the
    // reader or it stops meaning "inside Lightroom's own slider": at scale 100
    // the band admitted a file value up to 1.0, which on the measured scale is
    // a hue of 180 — half a turn past the slider's end stop, and a number the
    // reader would hand on for the recipe clamp to crush in silence.
    // (The old pairing was self-consistent, so this is not a refusal the old
    // build made wrongly — it is the one it failed to make.)
    // ±100.001, not ±100: Lightroom writes this key at SIX decimals, and the
    // slider's own end stop ±100 is `±0.555556` there, which reads back as
    // ±100.00008. Gating at exactly 100 would refuse a mask for the wire
    // format's rounding — the band is one wire step wide and nothing else.
    if !optional_scaled_number_in(own, "LocalHue", 180.0, -100.001, 100.001) {
        return Err(MaskImportReason::OutOfModel);
    }
    // Everything below this line is a NOTE, not a refusal.
    let mut reasons: Vec<MaskImportReason> = Vec::new();
    for key in INERT_LOCAL {
        if !optional_number_is(own, key, 0.0) {
            reasons.push(MaskImportReason::InertLocal(key));
        }
    }
    if !optional_number_is(own, "LocalCurveRefineSaturation", 100.0) {
        reasons.push(MaskImportReason::CurveRefineSaturation);
    }
    // The four per-channel local curves are child ELEMENTS, so the attribute
    // scan below never sees them. R25 P1 raised a note for every correction
    // that carried one, because none of them was modelled; R25 P6 models all
    // four, so the note narrowed to the case that is still a real loss: a
    // curve that is PRESENT and cannot be READ.
    //
    // The distinction is the module's own — "a knob we do not model is not the
    // same thing as a value we cannot read" (see `MaskImportReason`) — and the
    // narrowing is what stops the disclosure from claiming a loss that no
    // longer happens. `parse_one_correction` reads the same four keys through
    // the unchecked `parse_curve`, whose `Err` half becomes an empty curve;
    // this loop is what keeps that from being silent. Unreadable costs the
    // CURVE, not the correction — the geometry is still exactly what the file
    // draws, the same verdict `ForeignRangeMask` gets.
    if ["MainCurve", "RedCurve", "GreenCurve", "BlueCurve"]
        .iter()
        .any(|k| parse_curve_checked(own.text(), k).is_err())
    {
        reasons.push(MaskImportReason::LocalCurve);
    }

    // The own scope BEGINS with the correction's start tag by construction
    // (`element_own_scope` copies it first), so this walk sees exactly the
    // attributes it always did.
    let Some((_, gt, _)) = next_xml_tag(own.text(), 0) else {
        return Err(MaskImportReason::OutOfModel);
    };
    let mut cursor = 0;
    while let Some(a) = next_xml_attribute(&own.text()[..=gt], &mut cursor) {
        if let Some(local) = a.name.strip_prefix("crs:")
            && local.starts_with("Local")
            && !KNOWN_LOCAL.contains(&local)
        {
            reasons.push(MaskImportReason::UnknownLocalKey);
            break;
        }
    }
    Ok(reasons)
}

/// One `<rdf:li crs:What="Mask/…">` component. `Err(())` = the component is
/// UNUSABLE, and the caller decides what that costs: a geometry takes the
/// whole correction with it, a range component only takes itself.
fn component_import_reasons(
    tag: Tag<'_>,
    what: &str,
    authored_by_autoshade: bool,
    frame: Option<FrameAspect>,
) -> Result<Vec<MaskImportReason>, ()> {
    let expected_mode = if what == "Mask/RangeMask" { "1" } else { "0" };
    let expected_value = if what == "Mask/RangeMask" { 0.0 } else { 1.0 };

    // A muted component changes what the mask covers — a value we can read but
    // have no model for, so it still refuses. (Compare the knobs below, which
    // are composition, not coverage.)
    if !matches!(tag.crs_str("MaskActive").as_deref(), None | Some("true")) {
        return Err(());
    }
    // Lightroom's SUBTRACT is encoded as a PAIR, and `MaskValue="0"` is HALF of
    // it — not an opacity of zero. Census of every GitHub-indexed `.xmp`
    // carrying `crs:MaskBlendMode` (157 files, 479 attribute instances,
    // 2026-08-18; re-derived with a real XML parser, 0 parse failures):
    //
    //   MaskBlendMode="1"  ⇒  MaskValue="0"    26 / 26, no exceptions
    //   MaskBlendMode="0"  ⇒  MaskValue="1"   436 of 453 (16 are 0, one 0.662178)
    //   MaskBlendMode="1"  with MaskValue="1"   0 / 479
    //
    // `MaskInverted` is orthogonal — inversion occurs only with mode 0 in that
    // corpus, so inverting and subtracting are separate encodings, not one.
    //
    // So a zero MaskValue UNDER a non-default blend mode says "this shape is
    // subtracted", where the same zero on its own says "this shape is muted".
    // Reading the pair as a mute took the whole correction down (the geometry
    // arm turns `Err` into `OutOfModel`), which threw away a mask the file
    // draws perfectly well in order to avoid a composition we merely cannot
    // do — and the composition already has its own disclosure, three lines
    // down. What we must NEVER do is treat the 0 as a strength and multiply it
    // in: that would silently neutralise a mask the file says is fully
    // painted. Nothing here reads `MaskValue` as a magnitude; the adjustment's
    // strength comes from `crs:CorrectionAmount` alone (see
    // `parse_one_correction`).
    //
    // `expected_value != 0.0` keeps our own `Mask/RangeMask` encoding
    // (`MaskBlendMode="1"` + `MaskValue="0"`, which IS this app's intersect
    // spelling) out of the branch — it is already the expected pair there.
    let subtracted = expected_value != 0.0
        && tag.crs_str("MaskBlendMode").is_some_and(|mode| mode.as_ref() != expected_mode)
        && tag.crs_f32("MaskValue").is_some_and(|v| v.abs() <= 1e-6);
    if !subtracted && !optional_number_is(tag, "MaskValue", expected_value) {
        return Err(());
    }
    let mut reasons: Vec<MaskImportReason> = Vec::new();
    // `crs:Angle` sits on EVERY Lightroom radial, written as "0" when the
    // shape was never rotated (13 of the 24 radials in the reference set). A
    // zero angle loses NOTHING, so flagging its mere presence would raise a
    // false loss on half the catalog — the same "alarm on every save is alarm
    // the user learns to ignore" rule R24 applied to the export line. Present
    // but unreadable counts as rotated: we cannot say it is zero.
    //
    // v0.32.0 NARROWED this to what it now costs. The rotation used to be
    // dropped from EVERY radial, because the sign and pivot were unverified;
    // both are measured and `lr_to_engine` carries the tilt through. What is
    // left is one case — a document that declares no `tiff:ImageWidth /
    // ImageLength`, so the pixel→normalised fold has no aspect to fold with
    // (`FrameAspect`). Then, and only then, the ellipse arrives axis-aligned
    // and this says so. Asked of the DECODER rather than re-derived here, so
    // the sentence the user reads and the geometry the render draws cannot
    // disagree.
    if tag.crs_str("Angle").is_some() && tag.crs_f32("Angle").is_none_or(|v| v != 0.0) {
        // The angle rides along so the disclosure can NAME it; an unreadable
        // one is a rotation we cannot measure, and `0` is this payload's word
        // for that (see the variant's doc). `as i32` saturates rather than
        // wrapping.
        let readable = tag.crs_f32("Angle");
        // Two ways the tilt fails to arrive, and the frame narrows only ONE of
        // them: a value we cannot PARSE is a rotation nobody can apply however
        // well the frame is known, and `parse_one_correction` reads it as 0.
        if readable.is_none() || frame.is_none() {
            reasons.push(MaskImportReason::Rotation(readable.map_or(0, |v| v.round() as i32)));
        }
    }
    // `crs:MaskBlendMode` sits on every component Lightroom writes, and the
    // overwhelming majority carry the DEFAULT — the plain composition this
    // engine already does, so accepting it costs nothing. Only a different
    // mode is a loss, and it costs the composition, not the shape. Authorship
    // is deliberately NOT part of this test any more: the value says what it
    // says whoever wrote it, and requiring our own provenance is what refused
    // every Lightroom mask in the first place.
    if tag.crs_str("MaskBlendMode").is_some_and(|mode| mode.as_ref() != expected_mode) {
        reasons.push(MaskImportReason::BlendMode);
    }

    match what {
        "Mask/Gradient" => {
            if matches!(
                tag.crs_str("MaskInverted").as_deref(),
                None | Some("true") | Some("false")
            ) && ["ZeroX", "ZeroY", "FullX", "FullY"]
                .iter()
                .all(|key| tag.crs_f32(key).is_some_and(|v| (-8.0..=8.0).contains(&v)))
            {
                Ok(reasons)
            } else {
                Err(())
            }
        }
        "Mask/CircularGradient" => {
            // `crs:Roundness` is a ±100 INTEGER slider, not a 0..1 aspect
            // ratio. Direct observation, 2026-08-18: every one of the 24
            // radials in the harvested real-sidecar corpus writes it as a bare
            // signed integer, all of them `"0"` (its default), alongside
            // `Feather="+100"` and `Midpoint="+50"` on the same 0..100-style
            // integer footing — while the ONE attribute in those components
            // that really is a 0..1 real, `Feather` inside `crs:RetouchAreas`,
            // is a different field entirely (`0.388672`). ExifTool types the
            // whole `%sCorrectionMask` family as `real`, so the type says
            // nothing; the VALUES say ±100. The old `(0.0..=1.0)` gate was the
            // "bbox aspect" reading, and it refused the WHOLE correction — a
            // Lightroom user who touched this slider lost the mask, silently.
            //
            // Widened to the observed slider domain. No CONVERSION is applied
            // and none is needed: `roundness` is carried, never rendered (see
            // `MaskGeometry::Radial`), so the number rides through the recipe
            // and back into the sidecar unchanged. That is also what settles
            // the one value both readings could claim — `1`. Feather HAS to
            // disambiguate 0..1 from 0..100 because feather is rendered and a
            // wrong guess reshapes the photo; roundness has nothing to
            // disambiguate FOR, so a legacy `"0.25"` we once wrote comes back
            // as 0.25 and a Lightroom `"1"` comes back as 1, each written out
            // exactly as it arrived. Whichever scale either meant, the file
            // gets its own number back.
            if !matches!(
                tag.crs_str("MaskInverted").as_deref(),
                None | Some("true") | Some("false")
            ) || !matches!(
                tag.crs_str("Flipped").as_deref(),
                None | Some("true") | Some("false")
            ) || !["Top", "Left", "Bottom", "Right"]
                .iter()
                .all(|key| tag.crs_f32(key).is_some_and(|v| (-8.0..=8.0).contains(&v)))
                || !tag.crs_f32("Roundness").is_some_and(|v| (-100.0..=100.0).contains(&v))
            {
                return Err(());
            }
            let Some(raw) = tag.crs_f32("Feather") else {
                return Err(());
            };
            let feather = if raw > 1.0 || raw == raw.trunc() { raw / 100.0 } else { raw };
            if !(0.0..=1.0).contains(&feather) {
                return Err(());
            }
            // v0.32.0 — the corner decode's OWN gate, run here so a component
            // that cannot be decoded is refused by the same pass that refuses
            // every other unreadable value, with the geometry arm's cost
            // (`OutOfModel` takes the whole correction). Real Lightroom data
            // clears it 80/80 (`BBOX-DECODE.md` §2.1); what does not is a box
            // that decodes to a NEGATIVE semi-axis at the declared angle, i.e.
            // not an ellipse this model can name. `OutOfModel` is the honest
            // existing reason — "a value we can read that lands outside this
            // engine's model" — and needs no new word in any UI language.
            if matches!(
                lr_to_engine(
                    LrRadial {
                        top: tag.crs_f32("Top").unwrap_or(0.0) as f64,
                        left: tag.crs_f32("Left").unwrap_or(0.0) as f64,
                        bottom: tag.crs_f32("Bottom").unwrap_or(0.0) as f64,
                        right: tag.crs_f32("Right").unwrap_or(0.0) as f64,
                        angle_deg: tag.crs_f32("Angle").unwrap_or(0.0) as f64,
                    },
                    frame,
                ),
                RadialDecode::Refused
            ) {
                return Err(());
            }
            Ok(reasons)
        }
        // Someone else's range encoding is not ours to interpret — but that
        // is a reason to leave the RANGE behind, not the mask (R25 P1). The
        // caller turns this `Err` into a `ForeignRangeMask` note.
        "Mask/RangeMask" => {
            if authored_by_autoshade
                && matches!(tag.crs_str("MaskInverted").as_deref(), None | Some("true"))
            {
                Ok(reasons)
            } else {
                Err(())
            }
        }
        _ => Err(()),
    }
}

/// One `crs:What="Mask/Aggregate"` component → [`MaskGeometry::Brush`], or
/// `Err(())` when the file breaks an invariant this parser refuses to guess
/// past. `scope` is the string `agg`'s offsets are measured in.
///
/// **What is refused, and why refusing beats guessing.** Every gate below has
/// ZERO counter-examples in the 177-sidecar current corpus, so a document
/// that trips one was written by something other than Lightroom and this
/// module has no measurement to model it with. The caller turns the `Err` into
/// [`MaskImportReason::OutOfModel`] — "a value we can read that lands outside
/// this engine's model" — which costs the correction and says so, rather than
/// importing a shape the file does not describe.
///
///  * a child that is not `Mask/Paint` (measured: 398/398 children are Paint,
///    never a Gradient, Radial, RangeMask, Image or another Aggregate);
///  * anything nested BELOW the Paints (measured: maximum component nesting
///    depth in the whole library is exactly 2);
///  * a group with no strokes at all — not a measured shape, and re-emitting
///    an empty `<crs:Masks>` would put a construct into a sidecar that
///    Lightroom never writes;
///  * the per-stroke gates in [`parse_paint_stroke`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskBrushTableRefusal {
    MaskBrushTableUnavailable,
    ContainerInvalid,
    ReferenceMismatch,
    DigestMismatch,
    EncodingUnsupported,
    Corrupt,
    LengthMismatch,
    PayloadUnsupported,
    PayloadInvalid,
}

impl MaskBrushTableRefusal {
    pub fn name(self) -> &'static str {
        match self {
            Self::MaskBrushTableUnavailable => "MaskBrushTableUnavailable",
            Self::ContainerInvalid => "ContainerInvalid",
            Self::ReferenceMismatch => "ReferenceMismatch",
            Self::DigestMismatch => "DigestMismatch",
            Self::EncodingUnsupported => "EncodingUnsupported",
            Self::Corrupt => "Corrupt",
            Self::LengthMismatch => "LengthMismatch",
            Self::PayloadUnsupported => "PayloadUnsupported",
            Self::PayloadInvalid => "PayloadInvalid",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MaskBrushError {
    class: MaskBrushTableRefusal,
    detail: String,
}

impl MaskBrushError {
    fn new(class: MaskBrushTableRefusal, detail: impl Into<String>) -> Self {
        Self { class, detail: detail.into() }
    }
}

// The companion is an object store, not one allocation. Its file gate follows
// the RAW reader's per-file ceiling; the tighter gates below bound everything
// this parser actually materialises.
const MAX_ACR_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ACR_DIRECTORY_ENTRIES: usize = 4_096;
const MAX_MASK_BRUSH_BLOB_BYTES: usize = 16 * 1024 * 1024;
const MAX_MASK_BRUSH_UNCOMPRESSED_BYTES: usize = 16 * 1024 * 1024;
const MAX_MASK_BRUSH_RECORDS: usize = 256;
const MAX_MASK_BRUSH_D_COUNT: usize = 65_536;
const MAX_MASK_BRUSH_TOKENS: usize = 65_536;

#[derive(Clone)]
struct AcrEntry {
    key: [u8; 16],
    len: u64,
    offset: u64,
}

#[derive(Clone)]
struct AcrIndex {
    path: std::path::PathBuf,
    entries: Vec<AcrEntry>,
}

struct MaskBrushReader<'a, 'sink> {
    acr_path: Option<std::path::PathBuf>,
    diag: Option<&'a crate::diag::Diag<'sink>>,
    index: Option<Result<AcrIndex, MaskBrushError>>,
    tables: std::collections::HashMap<usize, Result<Vec<BrushStroke>, MaskBrushError>>,
    reported: std::collections::HashSet<usize>,
}

impl<'a, 'sink> MaskBrushReader<'a, 'sink> {
    fn new(photo: Option<&std::path::Path>, diag: Option<&'a crate::diag::Diag<'sink>>) -> Self {
        Self {
            acr_path: photo.map(|p| p.with_extension("acr")),
            diag,
            index: None,
            tables: std::collections::HashMap::new(),
            reported: std::collections::HashSet::new(),
        }
    }

    fn report(&mut self, owner_at: usize, token: &str, error: &MaskBrushError) {
        if self.reported.insert(owner_at)
            && let Some(diag) = self.diag
        {
            diag.warn(format!(
                "{}: MaskBrushTable {} refused ({})",
                error.class.name(),
                token,
                error.detail
            ));
        }
    }

    fn table(
        &mut self,
        owner_at: usize,
        token: &str,
        expected: usize,
    ) -> Result<Vec<BrushStroke>, MaskBrushError> {
        if let Some(cached) = self.tables.get(&owner_at) {
            return cached.clone();
        }
        let result = self.read_table(token, expected);
        if let Err(error) = &result {
            self.report(owner_at, token, error);
        }
        self.tables.insert(owner_at, result.clone());
        result
    }

    fn read_table(
        &mut self,
        token: &str,
        expected: usize,
    ) -> Result<Vec<BrushStroke>, MaskBrushError> {
        let key = mask_brush_key(token)?;
        if expected > MAX_MASK_BRUSH_UNCOMPRESSED_BYTES {
            return Err(MaskBrushError::new(
                MaskBrushTableRefusal::LengthMismatch,
                format!(
                    "advertised output {expected} exceeds the {MAX_MASK_BRUSH_UNCOMPRESSED_BYTES}-byte limit"
                ),
            ));
        }
        if self.index.is_none() {
            self.index = Some(match &self.acr_path {
                Some(path) => load_acr_index(path),
                None => Err(MaskBrushError::new(
                    MaskBrushTableRefusal::MaskBrushTableUnavailable,
                    "no photo path was available for sibling .acr discovery",
                )),
            });
        }
        let index = match self.index.as_ref().expect("index was initialized") {
            Ok(index) => index,
            Err(error) => return Err(error.clone()),
        };
        let mut matches = index.entries.iter().filter(|entry| entry.key == key);
        let Some(entry) = matches.next() else {
            return Err(MaskBrushError::new(
                MaskBrushTableRefusal::ReferenceMismatch,
                "directory contains no matching key",
            ));
        };
        if matches.next().is_some() {
            return Err(MaskBrushError::new(
                MaskBrushTableRefusal::ReferenceMismatch,
                "directory contains a duplicate matching key",
            ));
        }
        let blob_len = usize::try_from(entry.len).map_err(|_| {
            MaskBrushError::new(MaskBrushTableRefusal::Corrupt, "blob length does not fit memory")
        })?;
        if blob_len > MAX_MASK_BRUSH_BLOB_BYTES {
            return Err(MaskBrushError::new(
                MaskBrushTableRefusal::Corrupt,
                format!("blob exceeds the {MAX_MASK_BRUSH_BLOB_BYTES}-byte limit"),
            ));
        }
        let mut blob = Vec::new();
        blob.try_reserve_exact(blob_len).map_err(|_| {
            MaskBrushError::new(MaskBrushTableRefusal::Corrupt, "blob allocation refused")
        })?;
        blob.resize(blob_len, 0);
        let mut file = std::fs::File::open(&index.path).map_err(|error| {
            MaskBrushError::new(
                MaskBrushTableRefusal::MaskBrushTableUnavailable,
                format!("cannot reopen {}: {error}", index.path.display()),
            )
        })?;
        use std::io::{Read as _, Seek as _};
        file.seek(std::io::SeekFrom::Start(entry.offset)).map_err(|error| {
            MaskBrushError::new(
                MaskBrushTableRefusal::ContainerInvalid,
                format!("cannot seek to object: {error}"),
            )
        })?;
        file.read_exact(&mut blob).map_err(|error| {
            MaskBrushError::new(
                MaskBrushTableRefusal::ContainerInvalid,
                format!("object range became unreadable: {error}"),
            )
        })?;
        if md5::compute(&blob).0 != key {
            return Err(MaskBrushError::new(
                MaskBrushTableRefusal::DigestMismatch,
                "MD5(blob) does not equal the XMP/directory key",
            ));
        }
        if blob.len() < 16 {
            return Err(MaskBrushError::new(
                MaskBrushTableRefusal::EncodingUnsupported,
                "object is shorter than the 16-byte envelope",
            ));
        }
        let envelope = [
            le_u32_at(&blob, 0),
            le_u32_at(&blob, 4),
            le_u32_at(&blob, 8),
            le_u32_at(&blob, 12),
        ];
        let stream_len = blob.len() - 16;
        if envelope != [4, 1, 64_000, stream_len as u32] {
            return Err(MaskBrushError::new(
                MaskBrushTableRefusal::EncodingUnsupported,
                format!("unsupported object envelope {envelope:?}"),
            ));
        }
        let payload = decode_mask_brush_brotli(&blob[16..], expected)?;
        parse_mask_brush_payload(&payload)
    }
}

fn load_acr_index(path: &std::path::Path) -> Result<AcrIndex, MaskBrushError> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path).map_err(|error| {
        MaskBrushError::new(
            MaskBrushTableRefusal::MaskBrushTableUnavailable,
            format!("cannot read sibling {}: {error}", path.display()),
        )
    })?;
    let file_len = file.metadata().map_err(|error| {
        MaskBrushError::new(
            MaskBrushTableRefusal::MaskBrushTableUnavailable,
            format!("cannot inspect sibling {}: {error}", path.display()),
        )
    })?.len();
    if file_len > MAX_ACR_BYTES {
        return Err(MaskBrushError::new(
            MaskBrushTableRefusal::ContainerInvalid,
            format!("ACR file exceeds the {MAX_ACR_BYTES}-byte limit"),
        ));
    }
    let mut header = [0u8; 20];
    file.read_exact(&mut header).map_err(|error| {
        MaskBrushError::new(
            MaskBrushTableRefusal::ContainerInvalid,
            format!("truncated ACR header: {error}"),
        )
    })?;
    if &header[0..4] != b"ACR\0"
        || le_u32_at(&header, 4) != 1
        || &header[8..12] != b"ARW\0"
        || le_u32_at(&header, 16) != 0
    {
        return Err(MaskBrushError::new(
            MaskBrushTableRefusal::ContainerInvalid,
            "header is not the established ACR/1/ARW/reserved-zero shape",
        ));
    }
    let count = le_u32_at(&header, 12) as usize;
    if count > MAX_ACR_DIRECTORY_ENTRIES {
        return Err(MaskBrushError::new(
            MaskBrushTableRefusal::ContainerInvalid,
            format!("directory count exceeds the {MAX_ACR_DIRECTORY_ENTRIES}-entry limit"),
        ));
    }
    let directory_bytes = count.checked_mul(32).ok_or_else(|| {
        MaskBrushError::new(MaskBrushTableRefusal::ContainerInvalid, "directory size overflow")
    })?;
    let directory_end = 20u64.checked_add(directory_bytes as u64).ok_or_else(|| {
        MaskBrushError::new(MaskBrushTableRefusal::ContainerInvalid, "directory end overflow")
    })?;
    if directory_end > file_len {
        return Err(MaskBrushError::new(
            MaskBrushTableRefusal::ContainerInvalid,
            "directory extends past end of file",
        ));
    }
    let mut raw = Vec::new();
    raw.try_reserve_exact(directory_bytes).map_err(|_| {
        MaskBrushError::new(
            MaskBrushTableRefusal::ContainerInvalid,
            "directory allocation refused",
        )
    })?;
    raw.resize(directory_bytes, 0);
    file.read_exact(&mut raw).map_err(|error| {
        MaskBrushError::new(
            MaskBrushTableRefusal::ContainerInvalid,
            format!("truncated ACR directory: {error}"),
        )
    })?;
    let mut entries = Vec::new();
    entries.try_reserve_exact(count).map_err(|_| {
        MaskBrushError::new(
            MaskBrushTableRefusal::ContainerInvalid,
            "entry allocation refused",
        )
    })?;
    for chunk in raw.chunks_exact(32) {
        let mut key = [0u8; 16];
        key.copy_from_slice(&chunk[..16]);
        let len = le_u64_at(chunk, 16);
        let offset = le_u64_at(chunk, 24);
        let end = offset.checked_add(len).ok_or_else(|| {
            MaskBrushError::new(MaskBrushTableRefusal::ContainerInvalid, "object range overflow")
        })?;
        if len == 0 || offset < directory_end || end > file_len {
            return Err(MaskBrushError::new(
                MaskBrushTableRefusal::ContainerInvalid,
                "object range is empty, overlaps the directory, or is out of bounds",
            ));
        }
        entries.push(AcrEntry { key, len, offset });
    }
    let mut ranges = Vec::new();
    ranges.try_reserve_exact(entries.len()).map_err(|_| {
        MaskBrushError::new(
            MaskBrushTableRefusal::ContainerInvalid,
            "range allocation refused",
        )
    })?;
    ranges.extend(entries.iter().map(|entry| (entry.offset, entry.offset + entry.len)));
    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(MaskBrushError::new(
            MaskBrushTableRefusal::ContainerInvalid,
            "directory object ranges overlap",
        ));
    }
    let mut cursor = directory_end;
    for &(start, end) in &ranges {
        let padding = (4 - cursor % 4) % 4;
        if start != cursor + padding {
            return Err(MaskBrushError::new(
                MaskBrushTableRefusal::ContainerInvalid,
                "object gap is not the established four-byte alignment padding",
            ));
        }
        validate_acr_padding(&mut file, cursor, padding)?;
        cursor = end;
    }
    let trailing = (4 - cursor % 4) % 4;
    if file_len != cursor + trailing {
        return Err(MaskBrushError::new(
            MaskBrushTableRefusal::ContainerInvalid,
            "trailing gap is not the established four-byte alignment padding",
        ));
    }
    validate_acr_padding(&mut file, cursor, trailing)?;
    Ok(AcrIndex { path: path.to_path_buf(), entries })
}

fn validate_acr_padding(
    file: &mut std::fs::File,
    offset: u64,
    len: u64,
) -> Result<(), MaskBrushError> {
    use std::io::{Read as _, Seek as _};
    let mut padding = [0u8; 3];
    let len = usize::try_from(len).expect("four-byte alignment padding is at most three bytes");
    file.seek(std::io::SeekFrom::Start(offset)).map_err(|error| {
        MaskBrushError::new(
            MaskBrushTableRefusal::ContainerInvalid,
            format!("cannot seek to alignment padding: {error}"),
        )
    })?;
    file.read_exact(&mut padding[..len]).map_err(|error| {
        MaskBrushError::new(
            MaskBrushTableRefusal::ContainerInvalid,
            format!("cannot read alignment padding: {error}"),
        )
    })?;
    if padding[..len].iter().any(|byte| *byte != 0) {
        return Err(MaskBrushError::new(
            MaskBrushTableRefusal::ContainerInvalid,
            "alignment padding is not zero-filled",
        ));
    }
    Ok(())
}

fn mask_brush_key(token: &str) -> Result<[u8; 16], MaskBrushError> {
    if token.len() != 32 || !token.is_ascii() {
        return Err(MaskBrushError::new(
            MaskBrushTableRefusal::ReferenceMismatch,
            "reference is not exactly 32 ASCII hex bytes",
        ));
    }
    let mut key = [0u8; 16];
    for (i, pair) in token.as_bytes().chunks_exact(2).enumerate() {
        let hex = |b: u8| match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        };
        let (Some(hi), Some(lo)) = (hex(pair[0]), hex(pair[1])) else {
            return Err(MaskBrushError::new(
                MaskBrushTableRefusal::ReferenceMismatch,
                "reference contains a non-hex byte",
            ));
        };
        key[i] = (hi << 4) | lo;
    }
    Ok(key)
}

fn decode_mask_brush_brotli(
    stream: &[u8],
    expected: usize,
) -> Result<Vec<u8>, MaskBrushError> {
    use brotli_decompressor::{
        BrotliDecompressStream, BrotliResult, BrotliState, StandardAlloc,
    };
    let mut state = BrotliState::new(
        StandardAlloc::default(),
        StandardAlloc::default(),
        StandardAlloc::default(),
    );
    let mut available_in = stream.len();
    let mut input_offset = 0usize;
    let mut buffer = [0u8; 4_096];
    let mut available_out = buffer.len();
    let mut output_offset = 0usize;
    let mut total_out = 0usize;
    let mut output = Vec::new();
    output.try_reserve_exact(expected).map_err(|_| {
        MaskBrushError::new(MaskBrushTableRefusal::Corrupt, "output allocation refused")
    })?;
    loop {
        let result = BrotliDecompressStream(
            &mut available_in,
            &mut input_offset,
            stream,
            &mut available_out,
            &mut output_offset,
            &mut buffer,
            &mut total_out,
            &mut state,
        );
        if output.len().saturating_add(output_offset) > expected
            || output.len().saturating_add(output_offset)
                > MAX_MASK_BRUSH_UNCOMPRESSED_BYTES
        {
            return Err(MaskBrushError::new(
                MaskBrushTableRefusal::LengthMismatch,
                "Brotli output exceeded the advertised or implementation limit",
            ));
        }
        output.extend_from_slice(&buffer[..output_offset]);
        output_offset = 0;
        available_out = buffer.len();
        match result {
            BrotliResult::ResultSuccess => {
                if available_in != 0 || input_offset != stream.len() {
                    return Err(MaskBrushError::new(
                        MaskBrushTableRefusal::Corrupt,
                        "Brotli stream has trailing input",
                    ));
                }
                break;
            }
            BrotliResult::NeedsMoreInput if available_in == 0 => {
                return Err(MaskBrushError::new(
                    MaskBrushTableRefusal::Corrupt,
                    "Brotli stream is truncated",
                ));
            }
            BrotliResult::ResultFailure => {
                return Err(MaskBrushError::new(
                    MaskBrushTableRefusal::Corrupt,
                    "Brotli decoder rejected the stream",
                ));
            }
            BrotliResult::NeedsMoreInput | BrotliResult::NeedsMoreOutput => {}
        }
    }
    if output.len() != expected {
        return Err(MaskBrushError::new(
            MaskBrushTableRefusal::LengthMismatch,
            format!("decoded {} bytes, XMP advertises {expected}", output.len()),
        ));
    }
    Ok(output)
}

struct MaskBrushCursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> MaskBrushCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], MaskBrushError> {
        let end = self.at.checked_add(len).ok_or_else(|| {
            MaskBrushError::new(MaskBrushTableRefusal::PayloadInvalid, "payload offset overflow")
        })?;
        let out = self.bytes.get(self.at..end).ok_or_else(|| {
            MaskBrushError::new(MaskBrushTableRefusal::PayloadInvalid, "truncated payload field")
        })?;
        self.at = end;
        Ok(out)
    }

    fn u16(&mut self) -> Result<u16, MaskBrushError> {
        let bytes: [u8; 2] = self.take(2)?.try_into().expect("two-byte slice");
        Ok(u16::from_le_bytes(bytes))
    }

    fn u32(&mut self) -> Result<u32, MaskBrushError> {
        let bytes: [u8; 4] = self.take(4)?.try_into().expect("four-byte slice");
        Ok(u32::from_le_bytes(bytes))
    }
}

fn parse_mask_brush_payload(bytes: &[u8]) -> Result<Vec<BrushStroke>, MaskBrushError> {
    let mut cursor = MaskBrushCursor::new(bytes);
    if cursor.u32()? != 1 {
        return Err(MaskBrushError::new(
            MaskBrushTableRefusal::PayloadUnsupported,
            "table word is not 1",
        ));
    }
    let record_count = cursor.u32()? as usize;
    if record_count > MAX_MASK_BRUSH_RECORDS {
        return Err(MaskBrushError::new(
            MaskBrushTableRefusal::PayloadInvalid,
            format!("record count exceeds the {MAX_MASK_BRUSH_RECORDS}-record limit"),
        ));
    }
    if record_count
        .checked_mul(70)
        .and_then(|n| n.checked_add(8))
        .is_none_or(|minimum| minimum > bytes.len())
    {
        return Err(MaskBrushError::new(
            MaskBrushTableRefusal::PayloadInvalid,
            "record count cannot fit in the payload",
        ));
    }
    let mut records = Vec::new();
    records.try_reserve_exact(record_count).map_err(|_| {
        MaskBrushError::new(MaskBrushTableRefusal::PayloadInvalid, "record allocation refused")
    })?;
    let mut table_tokens = 0usize;
    for _ in 0..record_count {
        let what = cursor.u32()?;
        let active = cursor.u32()?;
        let blend = cursor.u32()?;
        let inverted = cursor.u16()?;
        let id_len = cursor.u32()? as usize;
        if what != 0 || active > 1 || blend != 0 || inverted != 0 || id_len != 32 {
            return Err(MaskBrushError::new(
                MaskBrushTableRefusal::PayloadUnsupported,
                format!(
                    "unsupported record fields What={what}, active={active}, blend={blend}, inverted={inverted}, id_len={id_len}"
                ),
            ));
        }
        let id = cursor.take(id_len)?;
        if !id.is_ascii() {
            return Err(MaskBrushError::new(
                MaskBrushTableRefusal::PayloadUnsupported,
                "MaskSyncID is not ASCII",
            ));
        }
        let sync_id = std::str::from_utf8(id)
            .expect("ASCII is UTF-8")
            .to_string();
        let value = cursor.u32()?;
        let radius = cursor.u32()?;
        let flow = cursor.u32()?;
        let center_weight = cursor.u32()?;
        let d_count = cursor.u32()? as usize;
        if d_count > MAX_MASK_BRUSH_D_COUNT {
            return Err(MaskBrushError::new(
                MaskBrushTableRefusal::PayloadInvalid,
                format!("d-count exceeds the {MAX_MASK_BRUSH_D_COUNT}-dab limit"),
            ));
        }
        let mut d_seen = 0usize;
        let mut dabs = String::new();
        while d_seen < d_count {
            if table_tokens >= MAX_MASK_BRUSH_TOKENS {
                return Err(MaskBrushError::new(
                    MaskBrushTableRefusal::PayloadInvalid,
                    format!("token count exceeds the {MAX_MASK_BRUSH_TOKENS}-token limit"),
                ));
            }
            let opcode = cursor.take(1)?[0];
            let token = match opcode {
                0x01 => format!("r {}", fixed_decimal(cursor.u32()?, 6)),
                0x02 => format!("f {}", fixed_decimal(cursor.u32()?, 4)),
                0x06 => {
                    d_seen += 1;
                    format!(
                        "d {} {}",
                        fixed_decimal(cursor.u32()?, 6),
                        fixed_decimal(cursor.u32()?, 6)
                    )
                }
                _ => {
                    return Err(MaskBrushError::new(
                        MaskBrushTableRefusal::PayloadUnsupported,
                        format!("unsupported opcode 0x{opcode:02X}"),
                    ));
                }
            };
            if !dabs.is_empty() {
                dabs.push('\n');
            }
            dabs.push_str(&token);
            table_tokens += 1;
        }
        records.push(BrushStroke {
            // A valid inactive record remains in table order but contributes no
            // density; the original bytes remain authoritative on write-back.
            value: if active == 0 { 0.0 } else { value as f32 / 1_000_000.0 },
            radius: radius as f32 / 1_000_000.0,
            flow: flow as f32 / 1_000_000.0,
            center_weight: center_weight as f32 / 1_000_000.0,
            sync_id,
            dabs,
        });
    }
    if cursor.at != bytes.len() {
        return Err(MaskBrushError::new(
            MaskBrushTableRefusal::PayloadInvalid,
            format!("{} trailing payload byte(s)", bytes.len() - cursor.at),
        ));
    }
    Ok(records)
}

fn fixed_decimal(value: u32, places: usize) -> String {
    let scale = 10u64.pow(places as u32);
    let value = u64::from(value);
    let whole = value / scale;
    let fraction = value % scale;
    if fraction == 0 {
        return whole.to_string();
    }
    let mut out = format!("{whole}.{fraction:0places$}");
    while out.ends_with('0') {
        out.pop();
    }
    out
}

fn le_u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("validated fixed-width field"))
}

fn le_u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("validated fixed-width field"))
}

fn parse_brush_group(
    scope: &str,
    agg: &XmlComponent<'_>,
    brush_reader: &mut MaskBrushReader<'_, '_>,
) -> Result<MaskGeometry, ()> {
    let tag = Tag::new(agg.tag);
    // `agg.start` is relative to one correction's component block and can be
    // identical in sibling corrections. The tag's address identifies this
    // Aggregate for the lifetime of the shared classify/build reader.
    let owner_id = agg.tag.as_ptr() as usize;
    // A muted component changes what the mask covers — refused for exactly the
    // reason `component_import_reasons` refuses a muted parametric shape.
    if !matches!(tag.crs_str("MaskActive").as_deref(), None | Some("true")) {
        return Err(());
    }
    // The group's three composition attributes, carried VERBATIM. Absent reads
    // as Lightroom's default in each case (it writes all three on 42/42 real
    // Aggregates, so absence is a foreign writer's terseness, not a value).
    let blend_mode = match tag.crs_str("MaskBlendMode") {
        None => 0u32,
        Some(v) => v.trim().parse::<u32>().map_err(|_| ())?,
    };
    let value = match tag.crs_str("MaskValue") {
        None => 1.0f32,
        Some(_) => tag.crs_f32("MaskValue").filter(|v| v.is_finite()).ok_or(())?,
    };
    let inverted = match tag.crs_str("MaskInverted").as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => return Err(()),
    };
    let name = tag.crs_str("MaskName").map(|v| v.into_owned()).unwrap_or_default();

    let mut strokes = Vec::new();
    let table = tag.crs_str("MaskBrushTable");
    let advertised = tag.crs_str("MaskBrushUncompressedBytes");
    if table.is_some() || advertised.is_some() {
        let reference = match table {
            Some(table) => Some(table),
            None => {
                let error = MaskBrushError::new(
                    MaskBrushTableRefusal::ReferenceMismatch,
                    "MaskBrushUncompressedBytes is present without MaskBrushTable",
                );
                brush_reader.report(owner_id, "<missing>", &error);
                None
            }
        };
        if let Some(table) = reference {
            match advertised.and_then(|v| v.trim().parse::<usize>().ok()) {
                Some(expected) => {
                    if let Ok(table_strokes) = brush_reader.table(owner_id, table.trim(), expected) {
                        strokes.extend(table_strokes);
                    }
                }
                None => {
                    let error = MaskBrushError::new(
                        MaskBrushTableRefusal::LengthMismatch,
                        "MaskBrushUncompressedBytes is missing or unreadable",
                    );
                    brush_reader.report(owner_id, table.trim(), &error);
                }
            }
        }
    }
    if let Some(body) = component_body(scope, agg)? {
        let kids = components_in(body);
        if kids.iter().any(|k| k.depth > 0) {
            return Err(());
        }
        for k in &kids {
            if k.what.as_ref() != "Mask/Paint" {
                return Err(());
            }
            strokes.push(parse_paint_stroke(body, k)?);
        }
    }
    if strokes.is_empty() {
        return Err(());
    }
    Ok(MaskGeometry::Brush { name, blend_mode, value, inverted, strokes })
}

/// One `crs:What="Mask/Image"` component → [`MaskGeometry::AiMask`], or
/// `Err(())` when the file breaks an invariant this parser refuses to guess
/// past. `scope` is the string `img`'s offsets are measured in.
///
/// **What is refused, and why refusing beats guessing** — every gate has zero
/// counter-examples in the 105 current instances:
///
///  * a muted component (`MaskActive` other than `"true"`) — 105/105 are
///    `"true"`, and a muted component changes what the mask covers;
///  * a missing or unreadable `MaskSubType` / `ReferencePoint` — both sit on
///    105/105, and they ARE the mask: without the pair there is nothing to
///    point a segmenter at;
///  * a `MaskSubType` outside `{0, 1, 2}` — three values on 105/105, and each
///    routes to a specific backend. A fourth would have no backend and
///    guessing one would invent a selection;
///  * an attribute name outside the modelled set plus
///    [`AI_MASK_PROVENANCE_KEYS`];
///  * a child element other than `crs:Gesture`, or a Gesture holding anything
///    but `Mask/Paint` (40 Gestures, one Paint each).
fn parse_ai_mask(scope: &str, img: &XmlComponent<'_>) -> Result<MaskGeometry, ()> {
    let tag = Tag::new(img.tag);
    if !matches!(tag.crs_str("MaskActive").as_deref(), None | Some("true")) {
        return Err(());
    }
    // The three composition attributes, read exactly as a brush group's are.
    let blend_mode = match tag.crs_str("MaskBlendMode") {
        None => 0u32,
        Some(v) => v.trim().parse::<u32>().map_err(|_| ())?,
    };
    let value = match tag.crs_str("MaskValue") {
        None => 1.0f32,
        Some(_) => tag.crs_f32("MaskValue").filter(|v| v.is_finite()).ok_or(())?,
    };
    let inverted = match tag.crs_str("MaskInverted").as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => return Err(()),
    };
    let name = tag.crs_str("MaskName").map(|v| v.into_owned()).unwrap_or_default();
    // REQUIRED, not defaulted: an absent subtype is not "object", it is a
    // component this reader has never seen.
    let subtype = tag.crs_str("MaskSubType")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|v| (0..=2).contains(v))
        .ok_or(())?;
    // `"0.517578 0.260997"` — space separated, normalised. STRICT: a malformed
    // token used to be the kind of thing a `filter_map` would drop, leaving the
    // remaining value to shift into the wrong field.
    let pt: Option<Vec<f32>> = tag.crs_str("ReferencePoint")
        .map(|s| s.split_whitespace().map(|x| x.parse::<f32>().ok()).collect::<Option<Vec<_>>>())
        .ok_or(())?;
    let pt = pt.filter(|v| v.len() == 2 && v.iter().all(|x| x.is_finite())).ok_or(())?;
    let mask_version = tag.crs_str("MaskVersion")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(1);

    // Every attribute on the element, in document order, split into "modelled
    // above", "carried as provenance", and "refused".
    let mut provenance: Vec<(String, String)> = Vec::new();
    for (key, raw) in crs_attributes(tag.text()) {
        match key.as_str() {
            // Modelled, or an invariant this writer re-emits as a literal.
            "What" | "MaskActive" | "MaskName" | "MaskBlendMode" | "MaskInverted"
            | "MaskSyncID" | "MaskValue" | "MaskVersion" | "MaskSubType" | "ReferencePoint" => {}
            k if AI_MASK_PROVENANCE_KEYS.contains(&k) => {
                provenance.push((key, xml_unescape(raw).into_owned()));
            }
            _ => return Err(()),
        }
    }

    // The optional `crs:Gesture` — the photographer's brush refinement.
    let mut gesture = Vec::new();
    if let Some(body) = component_body(scope, img)? {
        let Some(seq) = owned_element_body(body, "crs:Gesture")? else {
            // A body that is not a Gesture is markup this reader cannot
            // account for — the same verdict `classify_correction` gives a
            // component nested somewhere unmodelled.
            return Err(());
        };
        let kids = components_in(seq);
        if kids.iter().any(|k| k.depth > 0) {
            return Err(());
        }
        for k in &kids {
            if k.what.as_ref() != "Mask/Paint" {
                return Err(());
            }
            gesture.push(parse_paint_stroke(seq, k)?);
        }
        if gesture.is_empty() {
            return Err(());
        }
    }

    Ok(MaskGeometry::AiMask {
        name,
        subtype,
        ref_x: pt[0],
        ref_y: pt[1],
        blend_mode,
        value,
        inverted,
        mask_version,
        provenance,
        gesture,
        // Nothing is resolved at PARSE time: the segmenter runs at develop
        // time (`segment::resolve_ai_masks`), which is what makes this lazy —
        // importing a library must not spawn a model run per photo.
        raster: None,
    })
}

/// Every `crs:`-namespaced attribute on one element tag, as
/// `(local name, RAW value)` in document order.
///
/// Exists so [`parse_ai_mask`] can assert the CLOSED vocabulary its refusal
/// gate depends on. Asking `crs_str` for each known name could only ever tell
/// us which of the names we already knew were present — never that the document
/// carried a name we have not measured, which is the case that means "not
/// Lightroom's writer".
///
/// **R28 Batch-5 5d (F4 root 2 / symptom C): built on [`next_xml_attribute`]
/// instead of a second hand-rolled lexer.** The old one searched for `crs:`,
/// then for `=`, then for a DOUBLE QUOTE — so on a document written with
/// single-quoted attributes (`crs:MaskSubType='0'`, legal XML that every parser
/// but this one accepts) the first `find('"')` ran past the end of the tag or
/// into an unrelated value. What that cost was not academic: the closed-
/// vocabulary loop above saw an empty (or wrongly paired) attribute list, so it
/// could neither refuse an unmeasured name nor CARRY the eleven provenance /
/// digest keys — they were dropped on the way back out, silently, from a file
/// this reader had just accepted.
///
/// `next_xml_attribute` is quote-complete (it takes whichever quote opens the
/// value as the one that closes it), stops at the tag's own `/` or `>` rather
/// than running into a body, and is the SAME reader
/// `correction_value_reasons`' unknown-key walk already used — so the two scans
/// of "what attributes does this element carry" are now one implementation
/// rather than two with different ideas about XML.
fn crs_attributes(tag: &str) -> Vec<(String, &str)> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while let Some(a) = next_xml_attribute(tag, &mut cursor) {
        if let Some(local) = a.name.strip_prefix("crs:") {
            out.push((local.to_string(), a.value));
        }
    }
    out
}

/// One `crs:What="Mask/Paint"` child of a brush group → [`BrushStroke`].
///
/// The three attribute INVARIANTS are gates, not fields: `MaskActive="true"`,
/// `MaskBlendMode="0"` and `MaskInverted="false"` hold on 398/398 real Paints,
/// so a Paint that says otherwise is asserting a composition inside a group
/// that has never been observed to have one. The four NUMBERS are required
/// rather than defaulted for the same reason — all nine attributes sit on
/// 398/398 in-correction instances, with no optional fields and no variation,
/// so a missing one means this is not the encoding we measured.
fn parse_paint_stroke(scope: &str, p: &XmlComponent<'_>) -> Result<BrushStroke, ()> {
    let tag = Tag::new(p.tag);
    if !matches!(tag.crs_str("MaskActive").as_deref(), None | Some("true"))
        || !matches!(tag.crs_str("MaskBlendMode").as_deref(), None | Some("0"))
        || !matches!(tag.crs_str("MaskInverted").as_deref(), None | Some("false"))
    {
        return Err(());
    }
    let num = |k: &str| tag.crs_f32(k).filter(|v| v.is_finite()).ok_or(());
    Ok(BrushStroke {
        value: num("MaskValue")?,
        radius: num("Radius")?,
        flow: num("Flow")?,
        center_weight: num("CenterWeight")?,
        sync_id: tag.crs_str("MaskSyncID").map(|v| v.into_owned()).unwrap_or_default(),
        dabs: parse_dabs(scope, p)?,
    })
}

/// The `crs:Dabs` token stream of one Paint, VERBATIM — one token per line, in
/// the order the file lists its `<rdf:li>` items.
///
/// Each token is VALIDATED against the measured grammar (§[`BrushStroke::dabs`])
/// and then stored unchanged. Validating without parsing is the whole point:
/// the stream is the one payload whose MEANING waits on a measurement the
/// sidecar cannot supply, so this proves it is a stream we recognise and
/// refuses to impose a structure on it.
fn parse_dabs(scope: &str, p: &XmlComponent<'_>) -> Result<String, ()> {
    /// Far past the largest real stroke (645 dabs / 1,267 tokens) and far
    /// past the whole reference library (15,964 dabs), but still a bound — a
    /// hand-written sidecar must not be able to make one correction cost an
    /// unbounded allocation.
    const MAX_DAB_TOKENS: usize = 65_536;
    let Some(body) = component_body(scope, p)? else {
        return Err(());
    };
    let Some(seq) = owned_element_body(body, "crs:Dabs")? else {
        // Present on 398/398. A Paint without one is not a stroke.
        return Err(());
    };
    let mut out = String::new();
    let mut tokens = 0usize;
    let mut from = 0;
    while let Some((start, end, self_closing)) = next_xml_tag(seq, from) {
        let tag = &seq[start..=end];
        if tag.starts_with("</") || tag_name(tag) != "rdf:li" {
            from = end + 1;
            continue;
        }
        if self_closing || tokens >= MAX_DAB_TOKENS {
            return Err(()); // an empty <rdf:li/> holds no token
        }
        let close = element_close_start(seq, "rdf:li", end).ok_or(())?;
        let token = xml_unescape(&seq[end + 1..close]);
        dab_token_is_known(token.as_ref())?;
        if tokens > 0 {
            out.push('\n');
        }
        out.push_str(token.as_ref());
        tokens += 1;
        from = close + 1;
    }
    if tokens == 0 {
        return Err(());
    }
    Ok(out)
}

/// Is this one `crs:Dabs` item a token of the measured grammar? `r <f>`,
/// `f <f>`, `h <f>` or `d <x> <y>`, and nothing else — 22,966 tokens over 382
/// components, zero malformed, four forms.
///
/// The newline check is not decoration. [`parse_dabs`] joins tokens with
/// `'\n'` and the writer splits them back on it, so a token carrying one would
/// silently become two on the round trip. No real token spans a line; this
/// makes the storage form lossless by construction rather than by luck.
fn dab_token_is_known(t: &str) -> Result<(), ()> {
    /// A LENGTH bound as well as a shape one (R28 2b, adjudication F5's
    /// aggravator). Real tokens run ~10-30 bytes over the 22,966-token census
    /// above; 256 is eight times the widest of them, and still comfortably
    /// past the ~100 bytes two full-precision `f32` `Display`s plus the `d `
    /// prefix could occupy, so no token the grammar can legitimately produce
    /// is refused here.
    ///
    /// What it stops: a coordinate written as `0.` plus 300,000 digits parses
    /// to a perfectly finite `f32` (0.111…) and passes every check below,
    /// while the token COUNT gate (`MAX_DAB_TOKENS` = 65,536) never fires —
    /// ONE token then blows the storage-side 256 KiB byte cap by itself.
    /// Refused the way every other malformed token is: the Paint does not
    /// import, which by this parser's all-or-nothing group rule refuses the
    /// Aggregate and discloses it as `OutOfModel`.
    ///
    /// The FRACTIONAL form is the reachable one, and the distinction is not
    /// pedantry: the adjudication's own example — 300,000 integer digits —
    /// overflows to `inf` and was already refused by the finiteness check
    /// below, so only the `0.…` shape ever needed this bound (measured while
    /// writing the mutation test, R28 2b).
    const MAX_TOKEN_BYTES: usize = 256;
    if t.len() > MAX_TOKEN_BYTES {
        return Err(());
    }
    if t.contains('\n') || t.contains('\r') {
        return Err(());
    }
    let mut it = t.split_whitespace();
    let arity = match it.next() {
        Some("r" | "f" | "h") => 1,
        Some("d") => 2,
        _ => return Err(()),
    };
    for _ in 0..arity {
        let v = it.next().ok_or(())?.parse::<f32>().map_err(|_| ())?;
        if !v.is_finite() {
            return Err(());
        }
    }
    if it.next().is_some() {
        return Err(());
    }
    Ok(())
}

/// How a brush group composes onto the coverage built so far — the ONE place
/// `crs:MaskBlendMode` is mapped onto this engine's [`MaskCombine`].
///
/// `1` is Lightroom's subtract (paired with `MaskValue="0"`, observed on 23
/// current Aggregates); every other value, `0` included, is the plain union. The
/// carried `blend_mode` stays the authority for the WRITER, so an unmapped
/// mode still rides back out as itself instead of being normalised to what we
/// happened to render it as.
fn brush_combine(blend_mode: u32) -> MaskCombine {
    if blend_mode == 1 { MaskCombine::Subtract } else { MaskCombine::Add }
}

fn range_values_are_supported(range: &RangeMask) -> bool {
    match range {
        RangeMask::Luminance { lo_outer, lo, hi, hi_outer } => {
            [lo_outer, lo, hi, hi_outer].iter().all(|v| v.is_finite())
                && 0.0 <= *lo_outer
                && *lo_outer <= *lo
                && *lo <= *hi
                && *hi <= *hi_outer
                && *hi_outer <= 1.0
        }
        RangeMask::Color { r, g, b, amount, px, py } => {
            [r, g, b, amount, px, py]
                .iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(*v))
        }
    }
}

fn classify_correction(
    seg: &str,
    own: Scope<'_>,
    authored_by_autoshade: bool,
    frame: Option<FrameAspect>,
    brush_reader: &mut MaskBrushReader<'_, '_>,
) -> MaskCorrectionParse {
    let mut geometry_count = 0usize;
    let mut brush_count = 0usize;
    let mut ai_count = 0usize;
    let mut range_count = 0usize;
    let mut unknown_component = false;
    let mut geometry_unusable = false;
    let mut range_usable = true;
    let mut reasons: Vec<MaskImportReason> = Vec::new();
    // By NAME (attribute-carrying spelling included). No component list at
    // all means no parametric geometry to stand on; an UNTERMINATED one is
    // markup we could not finish reading, which is a different sentence.
    let mask_block = match owned_element_body(seg, "crs:CorrectionMasks") {
        Ok(Some(b)) => b,
        Ok(None) => return MaskCorrectionParse::Unsupported(MaskImportReason::Unrepresentable),
        Err(()) => return MaskCorrectionParse::Unsupported(MaskImportReason::OutOfModel),
    };

    // The ONE component whose shape actually arrives (`base_geometry_at`), by
    // its tag text — this loop walks `mask_block`, whose offsets are not the
    // `seg` offsets the selector returns. Two byte-identical components would
    // both compare equal, which is harmless: they say the same thing.
    let imported_tag = base_geometry_at(seg)
        .and_then(|p| next_xml_tag(seg, p))
        .map(|(s, e, _)| &seg[s..=e]);

    // R27 Batch-4 hazard 1: NESTING-AWARE. This loop used to be a flat
    // `next_xml_tag` walk, which reads the `Mask/Paint` strokes inside a
    // `Mask/Aggregate` as SIBLINGS of it — harmless only while both answers
    // were "refuse", and fatal the moment a Paint means something. Only
    // `depth == 0` components belong to THIS correction's list; everything
    // deeper belongs to the component that contains it, and is validated by
    // that component's own parser (see `parse_brush_group`).
    let components = components_in(mask_block);
    // The extents of the groups that own the nested components, so a nested
    // component sitting somewhere we do NOT model cannot pass unnoticed.
    let mut owned_nesting: Vec<(usize, usize)> = Vec::new();
    for component in components.iter().filter(|c| c.depth == 0) {
        {
            let (tag, what) = (component.tag, &component.what);
            let verdict =
                component_import_reasons(Tag::new(tag), what.as_ref(), authored_by_autoshade, frame);
            match what.as_ref() {
                "Mask/Gradient" | "Mask/CircularGradient" => {
                    geometry_count += 1;
                    match verdict {
                        // R25 P9: `Rotation` is a claim ABOUT THE SHAPE THAT
                        // ARRIVED — "radial rotation(s) read as 0" — so it may
                        // only come from the component that arrived. It used to
                        // come from all of them: `DSC08960` told the user about
                        // four rotations, and three of the four described
                        // radials that never entered the recipe at all (蒙版 5
                        // contributed two on its own). What covers a DROPPED
                        // shape is `MultiComponent`, which says exactly that.
                        //
                        // `BlendMode` deliberately stays component-wide: "one
                        // non-default blend mode was ignored" is true of a
                        // dropped subtract component, and it is the v0.31.1
                        // disclosure for precisely that case — scoping it to
                        // the base, which by definition carries the DEFAULT
                        // mode, would silence it on every file that has one.
                        Ok(rs) => reasons.extend(rs.into_iter().filter(|r| {
                            imported_tag == Some(tag) || !matches!(r, MaskImportReason::Rotation(_))
                        })),
                        Err(()) => geometry_unusable = true,
                    }
                }
                "Mask/RangeMask" => {
                    range_count += 1;
                    if verdict.is_err() {
                        range_usable = false;
                    }
                }
                // BRUSH GROUP — imported since R27 Batch-4 (L-08). The
                // registration that used to stand here said this whole arm was
                // a disclosure-granularity question and not a parser bug; F2's
                // measurement settled it the other way. `Mask/Aggregate` is a
                // one-level container of `Mask/Paint` strokes whose encoding is
                // fully determined by the sidecar (see `MaskGeometry::Brush`),
                // so it is read, carried and written back — and the correction
                // it sits in, plus every parametric shape standing beside it,
                // arrives instead of being thrown away. What is NOT in the
                // sidecar is the alpha kernel — that one was MEASURED rather
                // than guessed (R29 Batch-6), so the render now draws the
                // group from our own model of it and `BrushRendered` says so
                // in both directions.
                //
                // `verdict` is deliberately unused here: the attribute checks
                // it performs are the PARAMETRIC ones (Angle, the subtract
                // pair, `MaskValue == 1`), and a brush group's own attributes
                // mean different things — `MaskValue="0"` on an Aggregate is
                // half of Lightroom's subtract pair and the shared code would
                // read it as a mute. `parse_brush_group` is this component's
                // validator and it is stricter, not looser.
                "Mask/Aggregate" => {
                    brush_count += 1;
                    // The group's EXTENT is recorded whether or not it parses.
                    // A component nested inside a brush group is accounted for
                    // by definition — we know exactly what contains it — so a
                    // group we cannot model must not ALSO make its children
                    // look like markup from nowhere. Getting this backwards
                    // costs the user the accurate sentence: the correction
                    // would be refused as "AI / brush correction(s) skipped"
                    // when what actually happened is "a shape inside the group
                    // that Lightroom has never been observed to write".
                    if let Some(close) =
                        element_close_start(mask_block, tag_name(tag), component.gt)
                    {
                        owned_nesting.push((component.gt, close));
                    }
                    // Same cost as an unreadable parametric component: the
                    // values are legible and the SHAPE is outside the model
                    // this parser measured, which takes the correction.
                    if parse_brush_group(mask_block, component, brush_reader).is_err() {
                        geometry_unusable = true;
                    }
                }
                // AI MASK — imported since R27 Batch-5 (L-08 Arm C), and the
                // dominant refusal until then: 78 corrections across 40 files,
                // 40 % of every file in the reference library that has a mask
                // at all, taking 52 engine-drawable parametric shapes down
                // with them.
                //
                // What arrives is the INTENT (`MaskSubType` + `MaskName` +
                // `ReferencePoint`) and the provenance digests; there is no
                // raster payload and no geometry payload anywhere on one. The
                // alpha is therefore RE-DERIVED by our own segmenter at
                // develop time, not imported, and `AiMaskRecomputed` says so
                // in both directions. That distinction is the whole reason
                // this took a machine-learning work item rather than a parser
                // one — and the reason it can never be silent.
                //
                // `verdict` is deliberately unused, for `Mask/Aggregate`'s
                // reason: the shared checks are the PARAMETRIC ones, and
                // `MaskValue="0"` on an AI component is half of Lightroom's
                // subtract pair, which the shared code would read as a mute.
                // `parse_ai_mask` is this component's validator.
                "Mask/Image" => {
                    ai_count += 1;
                    // The extent is recorded whether or not it parses, exactly
                    // as a brush group's is: a `crs:Gesture` child is nesting
                    // we know the container of, so a component we cannot model
                    // must not ALSO make its own children look like markup from
                    // nowhere.
                    if let Some(close) =
                        element_close_start(mask_block, tag_name(tag), component.gt)
                    {
                        owned_nesting.push((component.gt, close));
                    }
                    if parse_ai_mask(mask_block, component).is_err() {
                        geometry_unusable = true;
                    }
                }
                // Depth / heal / anything else this reader has not measured.
                _ => unknown_component = true,
            }
        }
    }
    // A component nested inside something we did NOT model as a container is
    // markup this reader cannot account for. Refusing it keeps the depth-0
    // filter above honest: without this, a foreign writer could hide a whole
    // second mask inside an element we walked past.
    if components.iter().filter(|c| c.depth > 0).any(|c| {
        !owned_nesting.iter().any(|(open, close)| c.start > *open && c.start < *close)
    }) {
        unknown_component = true;
    }

    if (geometry_count == 0 && brush_count == 0 && ai_count == 0) || unknown_component {
        return MaskCorrectionParse::Unsupported(MaskImportReason::Unrepresentable);
    }
    if geometry_unusable {
        return MaskCorrectionParse::Unsupported(MaskImportReason::OutOfModel);
    }
    // PARAMETRIC extras only. A brush group is imported as a real component
    // now, so it is not one of the "extra shapes that do not" — counting it
    // here would tell the photographer a shape was dropped while the same run
    // writes it back into their sidecar.
    if geometry_count > 1 {
        reasons.push(MaskImportReason::MultiComponent);
    }
    if brush_count > 0 {
        reasons.push(MaskImportReason::BrushRendered);
    }
    // Raised on the IMPORT itself, not on whether the segmenter has run: the
    // photographer is being told what kind of thing arrived, and "the alpha
    // will be ours, not Adobe's" is true the moment the component is read.
    // `AiMaskUnresolved` is the separate, later sentence about a mask that has
    // no alpha yet — `segment::resolve_ai_masks` raises that one, because it
    // is the only place that knows.
    if ai_count > 0 {
        reasons.push(MaskImportReason::AiMaskRecomputed);
    }
    match correction_value_reasons(own) {
        Ok(rs) => reasons.extend(rs),
        Err(reason) => return MaskCorrectionParse::Unsupported(reason),
    }

    let Some(mut parsed) = parse_one_correction_with_reader(seg, own, frame, brush_reader) else {
        return MaskCorrectionParse::Unsupported(MaskImportReason::OutOfModel);
    };
    // A range we cannot honour costs the RANGE, not the mask: the geometry is
    // still exactly what the file draws. Dropping it must be explicit —
    // leaving `parsed.range` in place would import someone else's encoding as
    // if we had understood it.
    if range_count > 0
        && (!range_usable
            || range_count > 1
            || parsed.range.as_ref().is_none_or(|r| !range_values_are_supported(r)))
    {
        parsed.range = None;
        reasons.push(MaskImportReason::ForeignRangeMask);
    }
    // One line per KIND of loss: three components sharing a blend mode is one
    // sentence, not three.
    let mut uniq: Vec<MaskImportReason> = Vec::new();
    for r in reasons {
        if !uniq.contains(&r) {
            uniq.push(r);
        }
    }
    MaskCorrectionParse::Supported(Box::new(parsed), uniq)
}



/// The BASE geometry component of a correction — the byte offset of its tag
/// start within `seg`, or `None` if the correction carries no parametric
/// geometry at all.
///
/// A correction may hold SEVERAL geometry components: Lightroom's Add/Subtract
/// stack, where one shape is the BASE and the rest compose onto it via
/// `crs:MaskBlendMode` (default = the base, `"1"` + `MaskValue="0"` = subtract,
/// the encoding v0.31.1 taught this reader to accept). This engine imports ONE
/// shape and discloses the rest, so which one it takes IS what the photo looks
/// like.
///
/// R25 P9 — it used to take the wrong one, twice over:
///  * the choice was made by KIND. `parse_one_correction` tried
///    `Mask/Gradient` before `Mask/CircularGradient`, so ANY linear anywhere in
///    the correction beat EVERY radial regardless of position. `DSC08960`
///    蒙版 5 is `[CircularGradient, CircularGradient, Gradient]` — a plain
///    3-shape union, every component at the default blend mode — and imported
///    as the TRAILING linear with both radials gone.
///  * and it ignored `crs:MaskBlendMode`, which is the worse half. `DSC08960`
///    蒙版 3 and `_DSC9583` Mask 9 are both `[CircularGradient(base),
///    Gradient(MaskBlendMode="1" MaskValue="0")]`: the importer kept the
///    SUBTRACT shape and dropped the base, i.e. it rendered the region
///    Lightroom uses to carve away from the mask as the ENTIRE mask. That is
///    not a truncation of the user's intent, it is an inversion of it.
///
/// So: prefer the first component at the DEFAULT blend mode (the base), and
/// only when every component is subtractive fall back to the first component at
/// all — a correction made of nothing but subtractions has no base to find, and
/// some shape beats no shape.
///
/// R27 Batch-4 hazard 2 — it used to scan the WHOLE correction segment for the
/// first `crs:What="Mask/Gradient"` / `"Mask/CircularGradient"`, nesting-blind.
/// No Aggregate in the current corpus contains a parametric shape (398/398
/// children are Paint), so it could not fire — but nothing in the code enforced
/// that, and a foreign writer that DID nest a gradient inside a brush group
/// would have had it promoted to the correction's base shape. The search is now
/// over this correction's OWN component list, top level only.
fn base_geometry_at(seg: &str) -> Option<usize> {
    let (block_at, _block, comps) = correction_mask_components(seg)?;
    let (mut first, mut first_base) = (None, None);
    for c in comps.iter().filter(|c| c.depth == 0) {
        if !matches!(c.what.as_ref(), "Mask/Gradient" | "Mask/CircularGradient") {
            continue;
        }
        // Offsets come back in `seg`'s coordinates — every caller slices `seg`.
        let at = block_at + c.start;
        first = first.or(Some(at));
        // Absent counts as default: Lightroom writes the attribute on every
        // component it emits, and a component without one is not asserting a
        // composition (the same reading `component_import_reasons` takes).
        if Tag::new(c.tag).crs_str("MaskBlendMode").is_none_or(|m| m.as_ref() == "0") {
            first_base = first_base.or(Some(at));
        }
    }
    first_base.or(first)
}

/// One correction's OWN `crs:CorrectionMasks` list: `(offset of the list body
/// inside `seg`, the list body, its components)`.
///
/// The one place the "this correction's components" question is answered, so
/// the base selector and the geometry parser cannot disagree about which
/// components those are.
fn correction_mask_components(seg: &str) -> Option<(usize, &str, Vec<XmlComponent<'_>>)> {
    let (b0, b1) = owned_element_body_span(seg, "crs:CorrectionMasks").ok().flatten()?;
    let block = &seg[b0..b1];
    Some((b0, block, components_in(block)))
}

/// The base component's OWN element, from its `<` to just before its close tag
/// — R27 Batch-4 hazard 3.
///
/// `parse_one_correction` read its geometry keys out of `&seg[p..]`, a slice
/// running from the base component to the END of the correction. That was safe
/// only because Lightroom writes the shared attributes (`MaskValue`,
/// `MaskBlendMode`, `MaskInverted`) on EVERY component, so the first hit was
/// always the base's own — a coincidence, not an invariant, and one that stops
/// holding the moment brush components are legal in an imported correction: an
/// Aggregate at `MaskValue="0"` sitting after a base that omitted the attribute
/// would have donated its subtract half-pair to the base's reads.
///
/// Real Lightroom parametric components are self-closing, so this equals the
/// base TAG on every file in the reference library and the change is invisible
/// there. It is the element-form spelling it makes safe.
fn base_element(seg: &str, p: usize) -> &str {
    let Some((s, e, self_closing)) = next_xml_tag(seg, p) else {
        return &seg[p..];
    };
    if self_closing {
        return &seg[s..=e];
    }
    match element_close_start(seg, tag_name(&seg[s..=e]), e) {
        Some(close) => &seg[s..close],
        // Unterminated markup: fall back to the old unbounded slice rather
        // than losing the geometry entirely — a document this malformed is
        // already refused by `owned_element_body`'s own `Err` upstream.
        None => &seg[s..],
    }
}

/// One `crs:What="Correction"` segment → a [`LocalAdjustment`]. Slider scales
/// invert the writer's: exposure ×4 (a power-of-two rescale, exact in binary
/// FP), every other slider ×100 snapped to 4 decimals so `"0.3" → 30.0` lands
/// back on the UI grid instead of 30.000002.
///
/// `own` is the correction's OWN scope — every SLIDER below is read from it,
/// never from the whole segment (R28 Batch-5 5d, F4 symptom A). The segment
/// itself is still what the GEOMETRY reads walk, because the components live
/// inside it and each of those reads is separately bounded to the component it
/// belongs to.
#[cfg(test)]
fn parse_one_correction(
    seg: &str,
    own: Scope<'_>,
    frame: Option<FrameAspect>,
) -> Option<LocalAdjustment> {
    let mut brush_reader = MaskBrushReader::new(None, None);
    parse_one_correction_with_reader(seg, own, frame, &mut brush_reader)
}

fn parse_one_correction_with_reader(
    seg: &str,
    own: Scope<'_>,
    frame: Option<FrameAspect>,
    brush_reader: &mut MaskBrushReader<'_, '_>,
) -> Option<LocalAdjustment> {
    let scaled = |k: &str, scale: f32| {
        own.crs_f32(k).map_or(0.0, |v| (v * scale * 10_000.0).round() / 10_000.0)
    };
    let q100 = |k: &str| scaled(k, 100.0);
    let q180 = |k: &str| scaled(k, 180.0);
    // BRUSH GROUPS (R27 Batch-4). Parsed once, up front, and then split
    // between the base slot and the component list — `parse_brush_group` is
    // the strict validator, so a `?` here refuses the correction exactly as
    // `classify_correction`'s own call did, and the two cannot disagree about
    // what the file says.
    let (_, block, comps) = correction_mask_components(seg)?;
    let mut brushes: Vec<MaskGeometry> = Vec::new();
    let mut ai_masks: Vec<MaskGeometry> = Vec::new();
    for c in comps.iter().filter(|c| c.depth == 0) {
        match c.what.as_ref() {
            "Mask/Aggregate" => brushes.push(parse_brush_group(block, c, brush_reader).ok()?),
            // R27 Batch-5, same discipline: parsed once up front by the strict
            // validator, so a `?` here refuses the correction exactly as
            // `classify_correction`'s own call did and the two cannot disagree
            // about what the file says.
            "Mask/Image" => ai_masks.push(parse_ai_mask(block, c).ok()?),
            _ => {}
        }
    }
    // The geometry component decides the mask shape. `base_geometry_at` picks
    // WHICH parametric component that is when there are several — read its
    // doc, the choice used to invert the user's intent. `None` no longer ends
    // the correction: a brush-only correction takes its first group as the
    // base (F2 §7.3), which is the half of L-08 that rescues the 9 corrections
    // holding nothing but strokes.
    let (mask, base_el) = match base_geometry_at(seg) {
        None => {
            // ORDER MATTERS, and it is not the document's. With no parametric
            // shape to stand on, prefer a geometry the engine can actually
            // DRAW: an AI mask renders (through the recomputed alpha) and a
            // brush group does not, so taking the brush as base in a
            // correction that holds both would make the whole correction
            // inert and the AI mask a component of nothing.
            if !ai_masks.is_empty() {
                // Scope `""` for the inversion read below, for the brush
                // arm's reason: `MaskInverted` is carried INSIDE the geometry
                // and written back from there, so lifting it into
                // `LocalAdjustment::inverted` as well would spell one bit
                // twice — and on a mask whose alpha has not resolved yet, the
                // second spelling would turn zero coverage into a WHOLE-FRAME
                // adjustment.
                (ai_masks.remove(0), Scope::new(""))
            } else if brushes.is_empty() {
                return None;
            } else {
            // Scope `""` for the inversion read below, deliberately. A brush
            // group's `crs:MaskInverted` is CARRIED INSIDE the geometry
            // (`MaskGeometry::Brush::inverted`) and written back from there,
            // so lifting it into `LocalAdjustment::inverted` as well would
            // spell one bit twice — and the second spelling is the one the
            // render's weight loop reads, which on an inert brush would flip a
            // zero-coverage mask into a WHOLE-FRAME adjustment. The same
            // one-bit-one-home rule `lr_net_inverted` enforces for radials.
            (brushes.remove(0), Scope::new(""))
            }
        }
        Some(p) => {
    let base_tag = Tag::new(next_xml_tag(seg, p).map_or(&seg[p..], |(s, e, _)| &seg[s..=e]));
    let base_is_linear = xml_attribute_raw(base_tag.text(), "crs:What")
        .is_some_and(|(_, raw)| xml_unescape(raw).as_ref() == "Mask/Gradient");
    // HAZARD 3 (R27 Batch-4): every geometry read below is bounded to the base
    // component's OWN element instead of running to the end of the correction.
    // See `base_element` for the bleed this closes. It stays a SCOPE and not a
    // `Tag` deliberately (R28 Batch-5 5d): the element-form spelling of a
    // component puts these values in a body, and narrowing a working read on no
    // evidence is how a batch grows a regression — the same judgement the
    // `geom_tag` note below records for the two reads that DO need the tag.
    let g = Scope::new(base_element(seg, p));
    if base_is_linear {
        (
            MaskGeometry::Linear {
                zero_x: g.crs_f32("ZeroX")?,
                zero_y: g.crs_f32("ZeroY")?,
                full_x: g.crs_f32("FullX")?,
                full_y: g.crs_f32("FullY")?,
            },
            g,
        )
    } else {
        // The component's OWN TAG — `base_geometry_at` returns a tag START, so
        // this is the `<rdf:li …/>` that carries `crs:What`, and every geometry
        // attribute Lightroom (and this writer) puts on it. Still narrower than
        // `g` after hazard 3: `g` is the base ELEMENT (tag plus any body), and
        // the two reads below ask for names that recur in a body; see there.
        let geom_tag = base_tag;
        // Lightroom's Feather is 0..100 (reference sidecars: 50 / 72 …); the
        // engine's is 0..1. Three writers share this attribute, disambiguated
        // by TEXT SHAPE:
        //  * > 1.0 — unambiguous LR 0..100 scale;
        //  * ≤ 1.0 WITH a decimal point — our LEGACY 0..1 writer (it printed
        //    floats like "0.5"), passed through verbatim;
        //  * ≤ 1.0 integer text ("0"/"1") — LR's 0..100 (a genuine 1% edge)
        //    AND the CURRENT writer (which rounds to integers): both mean
        //    value/100. The old blanket ≤1.0-verbatim rule made our OWN 1%
        //    XMP round-trip back as a 100% feather.
        // (A legacy sidecar holding EXACTLY 1.0 prints as "1" and now reads
        //  as 1% — the current writer's round-trip wins that corner.)
        // Tested on the parsed VALUE (fractional ⇒ legacy 0..1), not the
        // text: "5e-1" carries no '.' yet is 0.5 — a text-shape test sent
        // it through /100.
        let feather_raw = g.crs_f32("Feather")?;
        let feather = if feather_raw > 1.0 || feather_raw == feather_raw.trunc() {
            feather_raw / 100.0
        } else {
            feather_raw
        };
        // v0.32.0: the stored corners are the ROTATED corners of the ellipse's
        // box in PIXEL space, not a bounding box — decoded (with the `k` frame
        // affine and the pixel→normalised rotation fold) by `lr_to_engine`,
        // whose doc carries the evidence. `Refused` is the decode's own
        // `a > 0 ∧ b > 0` guard: `None` here takes the WHOLE correction, which
        // is what `component_import_reasons` has already independently decided
        // for the same component, so the two arms cannot disagree about what
        // the file says.
        let decoded = lr_to_engine(
            LrRadial {
                top: g.crs_f32("Top")? as f64,
                left: g.crs_f32("Left")? as f64,
                bottom: g.crs_f32("Bottom")? as f64,
                right: g.crs_f32("Right")? as f64,
                // Absent = unrotated. Lightroom writes the attribute on every
                // radial, but an AutoShade sidecar written before v0.32.0 does
                // not, and its box IS the axis-aligned ellipse.
                angle_deg: geom_tag.crs_f32("Angle").unwrap_or(0.0) as f64,
            },
            frame,
        );
        let e = match decoded {
            RadialDecode::Exact(e) | RadialDecode::Unrotated(e) => e,
            RadialDecode::Refused => return None,
        };
        (
            MaskGeometry::Radial {
                top: e.top as f32,
                left: e.left as f32,
                bottom: e.bottom as f32,
                right: e.right as f32,
                feather,
                roundness: g.crs_f32("Roundness")?,
                // NOT `crs:Flipped` (R25 P9 — the defect this batch closed).
                //
                // This engine composes `Radial::flipped` and
                // `LocalAdjustment::inverted` by XOR (render.rs `mask_weight`
                // and the weight loop). Lightroom does not have that pair: it
                // writes ONE inversion bit twice, `crs:MaskInverted` and its
                // complement `crs:Flipped`. Census of the user's library —
                // 201/201 radials anti-correlated, no exceptions; re-derived on
                // the 7 M-B sidecars, 23/23 (16 `Flipped=true MaskInverted=
                // false`, 7 the mirror). Reading BOTH into our two flags XORed
                // a value with its own complement, so the net came out `true`
                // for EVERY imported Lightroom radial regardless of what the
                // file said — AutoShade inverted masks Lightroom does not.
                // Measured cost on `DSC09568`, tone-matched RMS against the
                // real Lightroom export: 0.1099 as imported → 0.0751 with this
                // fixed, and 0.1901 → 0.0869 in blue (E1-verdict §6 defect 2).
                //
                // So the inversion comes from `crs:MaskInverted` ALONE (read
                // into `inverted` below) and this stays false on import. The
                // flag itself is untouched: it is still OUR field, still
                // rendered, still the GUI's Flip checkbox, and a `recipe.json`
                // that carries `flipped: true` renders exactly as before.
                //
                // KNOWN BOUNDARY — a sidecar THIS APP wrote at ≤ v0.31.1 with a
                // flipped radial spelled it `Flipped="true" MaskInverted=
                // "false"`, which now re-imports as not-inverted. That is not a
                // new loss: Lightroom already read that pair as not-inverted,
                // so the old sidecar never carried the flip to Lightroom
                // either. Both directions now agree with Lightroom, which is
                // the whole point. Sidecars written from v0.31.2 on round-trip
                // their net exactly (`mask_geom_xml`).
                flipped: false,
                // v0.32.0: `crs:Angle` IS mapped now. It used to be dropped
                // because its sign and pivot were unverified; both are
                // measured — positive is CLOCKWISE on a y-down screen (three
                // independent determinations, `PROBE2-VERDICT.md` §5's
                // `_DSC9685` decoded −60.486° against a measured −60.5°), the
                // pivot is the ellipse centre (25 px against 218 px for the
                // frame centre, `ANGLE-MODEL.md` §3.4), and the rotation
                // happens in PIXEL space (28.554° measured against 19.692°
                // predicted by the normalised-frame reading, §3.2). What lands
                // here is not `crs:Angle` itself but its fold into this
                // engine's normalised-frame convention — see `lr_to_engine`.
                angle: e.angle_deg as f32,
                // R25 P5: the two attributes on every Lightroom radial that
                // this reader could not see until now. OPTIONAL, not `?`:
                // sidecars we wrote before this batch carry neither, and a
                // missing one means "Lightroom's default", not "unreadable
                // mask" (the ACR neutrals 50 / 2 — `crs:Version` is the
                // component's own schema stamp, never our recipe's).
                //
                // Read from `geom_tag`, the ONE component's own tag, not from
                // `g` (which runs to the end of the correction): `crs:Version`
                // is the first geometry attribute whose NAME recurs further
                // down — our own range-mask component carries
                // `crs:Version="3"` (see `range_mask_xml`), so an unbounded
                // scan would read the RANGE's schema stamp as the ellipse's.
                // The older reads above stay on `g`: no attribute they ask for
                // appears twice inside a correction, and re-scoping a working
                // read on no evidence is how a batch grows a regression.
                midpoint: geom_tag.crs_f32("Midpoint").filter(|v| v.is_finite()).unwrap_or(50.0),
                mask_version: geom_tag.crs_str("Version")
                    .and_then(|v| v.trim().parse::<u32>().ok())
                    .unwrap_or(2),
            },
            g,
        )
    }
        }
    };
    // Every brush group that did NOT become the base rides along as a real
    // component (F2 §7.3): the group's own `crs:MaskBlendMode` is what it
    // composes with, mapped ONCE by `brush_combine`. This is the first time
    // this reader has ever populated `components` — the parametric extras are
    // still dropped and still disclosed as `MultiComponent`, because there is
    // no second parametric shape to keep without changing which shape the
    // photo is (`base_geometry_at`'s whole subject).
    let components: Vec<MaskComponent> = brushes
        .into_iter()
        .chain(ai_masks)
        .map(|geometry| {
            let mode = match &geometry {
                MaskGeometry::Brush { blend_mode, .. }
                // R27 Batch-5: an AI mask composes by the SAME `crs:MaskBlendMode`
                // grammar (0 = union, 1 = subtract paired with `MaskValue="0"`),
                // so it goes through the one mapping rather than a second copy.
                | MaskGeometry::AiMask { blend_mode, .. } => brush_combine(*blend_mode),
                _ => MaskCombine::Add,
            };
            MaskComponent { geometry, mode }
        })
        .collect();
    // Optional range component. Its head repeats `MaskInverted="true"` as part
    // of the intersect ENCODING (see `range_mask_xml`), so user intent is read
    // from the geometry component only — hence the `base_el`-anchored scan.
    //
    // R28 Batch-5 5d (F4 symptom B) — TWO scope defects, one fix. The search
    // used to run over the WHOLE correction segment for the first tag saying
    // `crs:What="Mask/RangeMask"`, so a Mask/RangeMask sitting anywhere —
    // nested inside a brush group's `crs:Masks`, inside an AI mask's
    // `crs:Gesture`, or even outside `crs:CorrectionMasks` entirely — was
    // attached to this correction as its range. And once found, the reader ran
    // from that offset to the END of the segment, so the `rdf:li` the colour
    // arm parses could come from a later component altogether.
    //
    // Both close by asking the ONE component list this correction owns
    // (`correction_mask_components`, the same answer `base_geometry_at` and the
    // parser above use) for a TOP-LEVEL range component, and reading it from
    // its own element. Real Lightroom is unaffected: it writes range masks as
    // members of `crs:CorrectionMasks` and nowhere else.
    let range = comps
        .iter()
        .find(|c| c.depth == 0 && c.what.as_ref() == "Mask/RangeMask")
        .and_then(|c| {
            let r = Scope::new(base_element(block, c.start));
            // STRICT token parse (`collect::<Option<…>>`), not filter_map: a
            // malformed token used to vanish, letting the remaining values shift
            // one field left and still pass the length check.
            if let Some(lum) = r.crs_str("LumRange") {
                let v: Option<Vec<f32>> =
                    lum.split_whitespace().map(|x| x.parse().ok()).collect();
                let v = v?;
                (v.len() == 4).then(|| RangeMask::Luminance {
                    lo_outer: v[0],
                    lo: v[1],
                    hi: v[2],
                    hi_outer: v[3],
                })
            } else if let Some(amount) = r.crs_f32("ColorAmount") {
                // PointModels entry: "r g b px py 0" (writer + LR convention).
                let li = owned_element_body(r.text(), "rdf:li").ok().flatten()?;
                let v: Option<Vec<f32>> =
                    li.split_whitespace().map(|x| x.parse().ok()).collect();
                let v = v?;
                (v.len() >= 5)
                    .then(|| RangeMask::Color { r: v[0], g: v[1], b: v[2], amount, px: v[3], py: v[4] })
            } else {
                None
            }
        });
    Some(LocalAdjustment {
        mask,
        range,
        // Our own writer synthesises "AutoShade <n>" for unnamed masks (the
        // block above needs SOME CorrectionName) — importing that back as a
        // user-given name froze the placeholder and hid the localised
        // role/label. Round-trip it back to "unnamed".
        name: own
            .crs_str("CorrectionName")
            .map(|v| v.into_owned())
            .filter(|n| {
                n.strip_prefix("AutoShade ").is_none_or(|rest| rest.parse::<u32>().is_err())
            })
            .unwrap_or_default(),
        amount: own.crs_f32("CorrectionAmount").unwrap_or(1.0),
        components,
        inverted: base_el.crs_str("MaskInverted").as_deref() == Some("true"),
        exposure_ev: own.crs_f32("LocalExposure2012").unwrap_or(0.0) * 4.0,
        contrast: q100("LocalContrast2012"),
        highlights: q100("LocalHighlights2012"),
        shadows: q100("LocalShadows2012"),
        whites: q100("LocalWhites2012"),
        blacks: q100("LocalBlacks2012"),
        clarity: q100("LocalClarity2012"),
        dehaze: q100("LocalDehaze"),
        texture: q100("LocalTexture"),
        // `q100` here is the MEASURED scale as of 2026-08-19, settled by two
        // Adobe-anchored pairs that no library file supplies: Adobe's own
        // shipped Soften Skin local preset (UI Sharpness +25 -> `0.25`) and
        // MIDI2LR's all-sliders-at-maximum dump (+100 -> `1`). The one wobble
        // on record — the controlled session's `_DSC9594.xmp` reading
        // `crs:LocalSharpness="0.803738"` against a requested +80, briefly
        // read as a falsification when the user recalled TYPING the value —
        // resolves the other way: the divisor a typed 80 would need (99.535)
        // puts +-100 at +-1.0047, outside the endpoint Adobe sits exactly on,
        // and arbitrary 5-6-decimal values are normal Lightroom output (10 of
        // 18 distinct public non-zero values; no quantisation lattice). So
        // 0.803738 is the slider at 80.37 and the recollection gives way —
        // user-accepted ruling, 2026-08-19. Evidence archive:
        // ~/.claude/plans/r27-materials/F3-web-evidence/.
        // See docs/V2_PLAN.md §7 item 10 for the full adjudication.
        sharpness: q100("LocalSharpness"),
        saturation: q100("LocalSaturation"),
        // NOT `q100` — `crs:LocalHue` is the ONE local key measured off its
        // slider on a different scale (v0.32.0). The user's controlled
        // Lightroom export put the mask Hue slider at +50 and the sidecar came
        // back `crs:LocalHue="0.277778"` (`_DSC9594.xmp`, verbatim), and
        // 0.277778 × 180 = 50.00004 — no other simple scale lands on it (÷100
        // would read 27.8, ÷360 would read 100). The recipe's own domain is
        // unchanged at ±100 (`LocalAdjustment::hue`), so this is a boundary
        // conversion exactly like `crs:Feather`'s, not a widening.
        //
        // What is measured is the SCALE, not the meaning: what Lightroom does
        // with a +50 hue is its own model, and this engine keeps rendering the
        // value through `render::apply_masks`'s ±30° rotation — the same
        // honest split `texture` and local `sharpness` already carry.
        hue: q180("LocalHue"),
        temperature: q100("LocalTemperature"),
        tint: q100("LocalTint"),
        noise_reduction: q100("LocalLuminanceNoise"),
        // R25 P6: the four local point curves, read with the machinery the
        // global curves already use — `parse_curve` is
        // `owned_element_body` + `parse_curve_checked`, both name-matched and
        // both already hardened (a whitespace-spelled `<rdf:li >`, an
        // out-of-domain coordinate, an element that never closes). No second
        // parser, and no third scan of the segment.
        //
        // `parse_curve` swallows the `Err` half into an empty curve, which
        // would be silence — so `correction_value_reasons` runs the CHECKED
        // form over the same four keys and raises `LocalCurve` for exactly the
        // curves this line drops. The two must stay in step; the pair is
        // pinned by `an_unreadable_local_curve_is_named_not_swallowed`.
        // The four local point curves are child ELEMENTS of the correction, and
        // they read from its OWN scope for the same reason its sliders do (R28
        // Batch-5 5d): `correction_value_reasons` gates them there through
        // `parse_curve_checked`, and a gate and its reader that disagree about
        // scope are exactly the defect this batch closed one line up. A
        // `<crs:MainCurve>` nested inside a component is that component's, not
        // this correction's.
        main_curve: parse_curve(own.text(), "MainCurve"),
        red_curve: parse_curve(own.text(), "RedCurve"),
        green_curve: parse_curve(own.text(), "GreenCurve"),
        blue_curve: parse_curve(own.text(), "BlueCurve"),
        // color_gains / role are engine-only and never reach a sidecar.
        ..Default::default()
    })
}

/// Parse an ACR / Lightroom `.xmp` sidecar into an [`EditRecipe`] — the inverse
/// of [`recipe_to_xmp`] over every field classic XMP can carry. Absent keys stay
/// neutral, so a foreign XML parses to (nearly) a default recipe rather than
/// erroring. Two provenance rules keep a FOREIGN sidecar honest:
///   * `Temperature` counts only under `WhiteBalance="Custom"` — an "As Shot"
///     sidecar records the CAMERA's Kelvin, which is not an edit, and importing
///     it would visibly shift the render.
///   * Same for `Tint`, except sidecars we wrote ourselves (marked
///     `x:xmptk="AutoShade"`), whose Tint is always a real edit.
///
/// The returned recipe is clamped before it crosses the parser boundary, using
/// the same ranges and size caps as every other untrusted recipe input.
pub fn xmp_to_recipe(xmp: &str) -> EditRecipe {
    xmp_to_recipe_clamped(xmp).0
}

/// Path-aware import for an XMP sidecar that may reference a sibling `.acr`.
/// The diagnostic's photo is the single source of both discovery identity and
/// attribution, matching the rest of the injected diagnostic discipline.
pub fn xmp_to_recipe_with_diag(xmp: &str, diag: &crate::diag::Diag<'_>) -> EditRecipe {
    xmp_to_recipe_clamped_with_diag(xmp, diag).0
}

/// Silent path-aware import used for equality/probe work whose caller owns a
/// separate disclosure channel.
pub fn xmp_to_recipe_for_photo(xmp: &str, photo: &std::path::Path) -> EditRecipe {
    xmp_to_recipe_clamped_impl(xmp, Some(photo), None).0
}

/// [`xmp_to_recipe`] plus WHAT THE CLAMP COST — the door for every surface
/// that DISCLOSES import loss.
///
/// The clamp result used to be dropped on the floor here (`r.clamp();`), and
/// that single discarded value made the whole import-side truncation channel
/// silent: the GUI's own second clamp (`bin/gui/persist.rs`,
/// `bin/gui/export.rs`) ran on the already-cut recipe and correctly reported
/// nothing, so a sidecar that lost 131 KB of brush dabs and a mask component
/// on the way in looked exactly like one that arrived whole (R28 2b,
/// adjudication F5). The summary is a property of the READ, so it is produced
/// where the read is and returned rather than re-derived by anyone.
///
/// Callers that only want the recipe keep using [`xmp_to_recipe`] — the
/// unclamped-summary form stays the exception, not the default, so no caller
/// is obliged to handle a value it has no surface for.
pub fn xmp_to_recipe_clamped(xmp: &str) -> (EditRecipe, crate::recipe::ClampSummary) {
    xmp_to_recipe_clamped_impl(xmp, None, None)
}

pub fn xmp_to_recipe_clamped_with_diag(
    xmp: &str,
    diag: &crate::diag::Diag<'_>,
) -> (EditRecipe, crate::recipe::ClampSummary) {
    xmp_to_recipe_clamped_impl(xmp, diag.photo(), Some(diag))
}

fn xmp_to_recipe_clamped_impl(
    xmp: &str,
    photo: Option<&std::path::Path>,
    diag: Option<&crate::diag::Diag<'_>>,
) -> (EditRecipe, crate::recipe::ClampSummary) {
    if xmp.len() > MAX_XMP_BYTES {
        return (EditRecipe::default(), crate::recipe::ClampSummary::default());
    }
    // The disclosure scan answers a namespace conflict with "its camera-raw
    // settings were not imported" — and every restore surface pairs the two
    // calls. This reader kept importing anyway, reading properties through
    // the very prefixes the gate just declared unreliable: the two faces of
    // one document contradicted each other. Neutral is the only import the
    // disclosure sentence keeps honest.
    if xmlns_conflict(xmp).is_some() {
        return (EditRecipe::default(), crate::recipe::ClampSummary::default());
    }
    let ours = is_autoshade_sidecar(xmp);


    // EVERY setting below is read from this Description's OWN scope, never
    // the raw document: a nested creative Look carries owned-LOOKING crs
    // properties, and the flat scanners answered from them whenever the top
    // level omitted the key (see `crs_own_scope`). The provenance reads
    // (`is_autoshade_sidecar` above, the rationale comment below) deliberately
    // stay on the whole document — they live OUTSIDE the Description.
    let scope = crs_own_scope(xmp);
    // …and that scope is a TYPE now (R28 Batch-5 5d): `Scope` says "subtree,
    // first match wins", which is what a whole-document read means and what a
    // per-element read must never be handed.
    let scope = Scope::new(scope.as_ref());
    // Any EXPLICIT white balance is a user decision, not the camera's: ACR
    // writes Daylight / Cloudy / Shade / Tungsten / Fluorescent / Flash — each
    // with its own Temperature+Tint — and accepting only "Custom" imported all
    // of them as as-shot, dropping a WB the photographer had chosen. (Absent
    // is treated as explicit for the same reason `eval`/`style` do: a sidecar
    // carrying Temperature without the mode is still a stated value.)
    let custom_wb = scope.crs_str("WhiteBalance").as_deref() != Some("As Shot");
    let f = |k: &str| scope.crs_f32(k).unwrap_or(0.0);
    // The engine's own neutrals, for the ONE block whose neutral is not zero
    // (de-fringe, R25 B3) — `f`'s zero fallback would invent a hue window.
    let dflt = EditRecipe::default();
    // The SOURCE frame this document's coordinates are measured against, and
    // the turn that carries them into the frame this engine displays — read
    // ONCE and served to both geometry decodes (crop and masks), because they
    // are the same encoding in the same frame (`FrameAspect`).
    let frame = FrameAspect::from_xmp(xmp);
    // Adobe applies `CropAngle` only under `HasCrop="True"` — importing a
    // stale angle from a DISABLED crop activated a straighten Adobe itself
    // does not render.
    let crop_read = read_crop(scope, frame, ours);

    let mut hsl = Hsl::default();
    for (i, band) in crate::recipe::HSL_BANDS.iter().enumerate() {
        hsl.hue[i] = f(&format!("HueAdjustment{band}"));
        hsl.saturation[i] = f(&format!("SaturationAdjustment{band}"));
        hsl.luminance[i] = f(&format!("LuminanceAdjustment{band}"));
    }
    // A wheel whose HUE is present but unreadable must not keep its paired
    // saturation: the generic zero fallback turned a corrupt hue into finite
    // 0 (= red), so `ShadowHue="bogus"` + a valid Saturation of 50 imported
    // as a STRONG RED grade while the disclosure said "restored as neutral"
    // (16-lane scan L05). The hue itself is already named by
    // `unparsable_crs_numbers`; zeroing the sat makes the wheel colourless.
    let wheel_sat = |hue_key: &str, sat_key: &str| -> f32 {
        if scope.crs_str(hue_key).is_some() && scope.crs_f32(hue_key).is_none() {
            0.0
        } else {
            f(sat_key)
        }
    };
    let color_grade = ColorGrade {
        shadow_hue: f("SplitToningShadowHue"),
        shadow_sat: wheel_sat("SplitToningShadowHue", "SplitToningShadowSaturation"),
        shadow_lum: f("ColorGradeShadowLum"),
        midtone_hue: f("ColorGradeMidtoneHue"),
        midtone_sat: wheel_sat("ColorGradeMidtoneHue", "ColorGradeMidtoneSat"),
        midtone_lum: f("ColorGradeMidtoneLum"),
        highlight_hue: f("SplitToningHighlightHue"),
        highlight_sat: wheel_sat("SplitToningHighlightHue", "SplitToningHighlightSaturation"),
        highlight_lum: f("ColorGradeHighlightLum"),
        global_hue: f("ColorGradeGlobalHue"),
        global_sat: wheel_sat("ColorGradeGlobalHue", "ColorGradeGlobalSat"),
        global_lum: f("ColorGradeGlobalLum"),
        blending: scope.crs_f32("ColorGradeBlending").unwrap_or(ColorGrade::default().blending),
        balance: f("SplitToningBalance"),
    };
    // Our own comment header carries the AI provenance back (best-effort; the
    // escaped rationale cannot contain a raw "-->", so the scan is unambiguous).
    let (rationale, confidence) = block_between(xmp, "AI rationale: ", " -->")
        .and_then(|body| {
            let cut = body.rfind(" (confidence ")?;
            let conf =
                body[cut + " (confidence ".len()..].trim_end_matches(')').parse::<f32>().ok()?;
            Some((xml_unescape(&body[..cut]).into_owned(), conf))
        })
        .unwrap_or_default();

    let mut r = EditRecipe {
        temperature_k: custom_wb.then(|| scope.crs_f32("Temperature")).flatten(),
        tint: if custom_wb || ours { f("Tint") } else { 0.0 },
        exposure_ev: f("Exposure2012"),
        contrast: f("Contrast2012"),
        highlights: f("Highlights2012"),
        shadows: f("Shadows2012"),
        whites: f("Whites2012"),
        blacks: f("Blacks2012"),
        clarity: f("Clarity2012"),
        dehaze: f("Dehaze"),
        vibrance: f("Vibrance"),
        saturation: f("Saturation"),
        texture: f("Texture"),
        // The nine CARRIED effects (R25 B2). `f` answers 0 for an absent key,
        // which is exactly this batch's neutral — so a sidecar that names none
        // of them still imports as a no-op, and one that names a real vignette
        // brings all six values with it.
        post_crop_vignette: f("PostCropVignetteAmount"),
        post_crop_vignette_mid: f("PostCropVignetteMidpoint"),
        post_crop_vignette_feather: f("PostCropVignetteFeather"),
        post_crop_vignette_round: f("PostCropVignetteRoundness"),
        post_crop_vignette_style: f("PostCropVignetteStyle"),
        post_crop_vignette_hl: f("PostCropVignetteHighlightContrast"),
        grain: f("GrainAmount"),
        grain_size: f("GrainSize"),
        grain_rough: f("GrainFrequency"),
        hsl,
        color_grade,
        // 1:1. Lightroom's Detail > Sharpening "Amount" slider runs 0..150 and
        // `crs:Sharpness` stores that UI number unscaled — 15 real sidecars in
        // 2 unrelated repositories, 2 camera bodies and 2 Lightroom generations
        // carry `crs:Sharpness="150"` (`crs:Version` 15.3/17.2), the maximum in
        // 566 observed occurrences (web survey, 2026-08-18). The reader's old
        // ×1.5 and the writer's ×⅔ were both built on a 0..100 ceiling that
        // does not exist: `Sharpness="40"` used to import as 60, `"150"` came
        // in as 225 and was clamped back to 150 while ALSO being reported as
        // an unparsable number, and a rendered 60 was written back as 40.
        sharpening: f("Sharpness"),
        noise_reduction: f("LuminanceSmoothing"),
        // The eight CARRIED detail axes (R25 B3). `f` answers 0 for an absent
        // key, which IS this block's neutral — an untouched sidecar still
        // imports as a no-op, and one that names a real sharpening radius
        // brings the whole triple with it.
        sharpen_radius: f("SharpenRadius"),
        sharpen_detail: f("SharpenDetail"),
        sharpen_mask: f("SharpenEdgeMasking"),
        nr_detail: f("LuminanceNoiseReductionDetail"),
        nr_contrast: f("LuminanceNoiseReductionContrast"),
        color_nr: f("ColorNoiseReduction"),
        color_nr_detail: f("ColorNoiseReductionDetail"),
        color_nr_smooth: f("ColorNoiseReductionSmoothness"),
        lens_vignette: f("VignetteAmount"),
        lens_vignette_mid: scope.crs_f32("VignetteMidpoint").unwrap_or(50.0),
        lens_distortion: f("LensManualDistortionAmount"),
        ca_r: f("ChromaticAberrationR"),
        ca_b: f("ChromaticAberrationB"),
        // A FLAG: Lightroom writes 0/1, and "true" is the other spelling in
        // the wild for a crs boolean — both are accepted, anything else (and
        // absence) is off.
        auto_lateral_ca: matches!(
            scope.crs_str("AutoLateralCA").as_deref().map(str::trim),
            Some("1") | Some("true") | Some("True")
        ),
        // De-fringe, the ONE block that falls back to ADOBE'S DEFAULT rather
        // than to zero. `f` answers 0 for an absent key, and taking that here
        // would import a hue window of 0..0 from a document that never
        // mentioned one — the photo would stop being a no-op, and the next
        // save would write that invented window into the sidecar beside the
        // RAW. `EditRecipe::default()` holds Adobe's 30/70 and 40/60, so a
        // document with no de-fringe block comes back exactly neutral.
        defringe_purple: scope.crs_f32("DefringePurpleAmount").unwrap_or(dflt.defringe_purple),
        defringe_purple_lo: scope.crs_f32("DefringePurpleHueLo").unwrap_or(dflt.defringe_purple_lo),
        defringe_purple_hi: scope.crs_f32("DefringePurpleHueHi").unwrap_or(dflt.defringe_purple_hi),
        defringe_green: scope.crs_f32("DefringeGreenAmount").unwrap_or(dflt.defringe_green),
        defringe_green_lo: scope.crs_f32("DefringeGreenHueLo").unwrap_or(dflt.defringe_green_lo),
        defringe_green_hi: scope.crs_f32("DefringeGreenHueHi").unwrap_or(dflt.defringe_green_hi),
        // Both halves come out of ONE decode (R27, `lr_to_engine_crop`): the
        // rectangle and the tilt are two faces of one rotated-corner encoding,
        // and reading either without the other is what made every straightened
        // import wrong by 2 × CropAngle on top of a frame error.
        straighten_deg: crop_read.straighten_deg() as f32,
        crop: crop_read.crop(),


        tone_curve: parse_curve(scope.text(), "ToneCurvePV2012"),
        red_curve: parse_curve(scope.text(), "ToneCurvePV2012Red"),
        green_curve: parse_curve(scope.text(), "ToneCurvePV2012Green"),
        blue_curve: parse_curve(scope.text(), "ToneCurvePV2012Blue"),
        masks: parse_masks_with_source(scope.text(), ours, frame, photo, diag),

        // The PASS-THROUGH blocks (R25 B4), read as STRINGS and stored
        // verbatim. `crs_str` already reads BOTH spellings — the
        // Description's own attribute and the property-element child — so
        // this loop covers the element form with no second scanner (the R24
        // round-end MED-1 lesson: a third scan arm is how the two forms drift
        // apart). An absent key stays absent from the map, which is how the
        // writer knows not to invent it.
        //
        // Read from `scope`, like every other setting: a creative Look nests
        // its own baked `crs:CameraProfile`, and the flat scan would have
        // imported the PROFILE's name whenever the top level omitted one.
        passthrough: PASSTHROUGH_CRS
            .iter()
            .filter_map(|k| scope.crs_str(k).map(|v| ((*k).to_string(), v.into_owned())))
            .collect(),

        rationale,
        confidence,
        ..Default::default()
    };
    // PROVENANCE RULE 3 (WB-anchor era): a sidecar WE wrote before the
    // absolute-Kelvin engine (x:xmptk="AutoShade", no era-2 marker) carries a
    // Temperature that was tuned RELATIVE to the historical 5500 K anchor.
    // Pin the engine anchor there — the honest encoding of that provenance —
    // so every stamp-if-None call site leaves it alone and the develop
    // renders exactly as it was tuned. Foreign sidecars (Lightroom's
    // Temperature is absolute) and era-2 documents stay unpinned: the caller
    // stamps the camera's real anchor. The pin deliberately leaves
    // as_shot_tint None — "anchor known, camera unknown" — which is also what
    // gates the as-shot caption off for these photos.
    if ours && !is_autoshade_era2(xmp) && r.temperature_k.is_some() {
        r.as_shot_k = Some(5500.0);
    }
    // SOURCE FRAME → DISPLAY FRAME (R27 A7, `P1-portrait-mask-frame.md` §5).
    // Every `crs:` coordinate above — crop rectangle and mask geometry alike —
    // was decoded in the frame the document declares, which for a portrait
    // capture is the UN-ROTATED sensor array (9504 × 6336) while this engine
    // renders the turned one. `orient_recipe_coords` is the algebra that moves
    // a whole recipe between those two frames; it already exists for the
    // `coord_era` migration, and running it HERE is what makes an imported
    // recipe's `coord_era` stamp true instead of a label on sensor-frame
    // numbers. It moves the crop, every mask box, the range-mask sample point
    // and — under a mirror — the straighten's sign, all in one pass.
    //
    // A landscape document (`tiff:Orientation` absent or 1) turns nothing, so
    // this is inert for every frame the twelve-export experiment and the 16
    // reference sidecars contain.
    //
    // The frame handed along is the SOURCE rectangle, because that is the one
    // these coordinates are still in (R29 C1, `render::CoordFrame`): a brush's
    // radii are in width units, so the rewrite has to divide by the width the
    // document declares, not by the one the display frame will have.
    if let Some(f) = frame {
        crate::render::orient_recipe_coords(
            &mut r,
            f.turn(),
            crate::render::CoordFrame::new(f.w, f.h),
        );
    }
    // Independent scalar controls saturate at the recipe contract and are
    // named by `unparsable_crs_numbers` when that changes a foreign value.
    // Compound crop and mask data are rejected earlier because clamping only
    // part of their geometry would silently change coverage.
    //
    // The summary RIDES OUT rather than being discarded (R28 2b): what the
    // SIZE caps cut — dab bytes, strokes, curve knots — is loss no other
    // channel here can see, because `import_losses` reads the document and
    // this reads the recipe the document produced.
    let dropped = r.clamp();
    (r, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atmosphere_xmp_contains_only_representable_global_controls() {
        let recipe = EditRecipe {
            exposure_ev: -0.8,
            temperature_k: Some(9000.0),
            tint: 10.0,
            saturation: 30.0,
            tone_curve: vec![
                CurvePoint { input: 0, output: 0 },
                CurvePoint { input: 64, output: 48 },
                CurvePoint { input: 128, output: 112 },
                CurvePoint { input: 192, output: 208 },
                CurvePoint { input: 255, output: 255 },
            ],
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: "atmosphere-sky.png".into() },
                role: crate::recipe::MaskRole::ZoneSky,
                exposure_ev: -0.5,
                color_gains: Some([1.18, 0.96, 0.85]),
                saturation: -20.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let (doc, losses) = recipe_to_xmp_with_losses(&recipe);
        assert!(doc.contains(r#"crs:Exposure2012="-0.80""#));
        assert!(doc.contains(r#"crs:Temperature="9000""#));
        assert!(doc.contains(r#"crs:Tint="+10""#));
        assert!(doc.contains(r#"crs:Saturation="+30""#));
        assert!(doc.contains("crs:ToneCurvePV2012"));
        assert!(!doc.contains("ToneCurvePV2012Red"));
        assert!(!doc.contains("ToneCurvePV2012Green"));
        assert!(!doc.contains("ToneCurvePV2012Blue"));
        assert!(!doc.contains("MaskGroupBasedCorrections"));
        assert_eq!(losses.len(), 1, "the bitmap zone stays engine-only");

        let projected = xmp_to_recipe(&doc);
        assert_eq!(projected.exposure_ev, -0.8);
        assert_eq!(projected.temperature_k, Some(9000.0));
        assert_eq!(projected.tint, 10.0);
        assert_eq!(projected.saturation, 30.0);
        assert_eq!(projected.tone_curve, recipe.tone_curve);
        assert!(projected.masks.is_empty());
        assert!(projected.red_curve.is_empty());
        assert!(projected.green_curve.is_empty());
        assert!(projected.blue_curve.is_empty());
    }

    /// R24-5 M0, EXPORT direction: the sidecar cannot carry the camera base
    /// curve or the lens-profile correction, and the user has to hear it —
    /// silence there is a photo that renders differently in Lightroom for a
    /// reason nothing on screen names.
    ///
    /// Derived from the tier registry, so this test also pins the derivation:
    /// a neutral recipe discloses nothing, and the disclosed names are exactly
    /// the `RenderedNotExported` rows that are actually set.
    #[test]
    fn the_export_names_the_globals_the_sidecar_cannot_carry() {
        use crate::advisor::catalogue::{Tier, RECIPE_CONTROLS};
        assert!(
            global_export_losses(&EditRecipe::default()).is_empty(),
            "a neutral recipe loses nothing — a save that lost nothing must not interrupt"
        );
        // The engine's own measurement of THIS photo: rendered, unexportable.
        let with_base = EditRecipe {
            base_curve: vec![[0.0, 0.0], [0.5, 0.55], [1.0, 1.0]],
            ..Default::default()
        };
        assert_eq!(global_export_losses(&with_base), vec!["base_curve"]);
        let both = EditRecipe {
            lens_profile: crate::recipe::LensProfile {
                vignette_on: true,
                vignette: vec![1.0, 0.1],
                ..Default::default()
            },
            ..with_base.clone()
        };
        assert_eq!(global_export_losses(&both), vec!["base_curve", "lens_profile"]);
        // An ordinary rendered control is NOT a loss (it has its own crs key).
        let exposed = EditRecipe { exposure_ev: 1.5, ..Default::default() };
        assert!(global_export_losses(&exposed).is_empty());
        // Premise: the tier this is derived from is populated. A registry
        // where nobody is RenderedNotExported would make every case above
        // pass for the wrong reason.
        assert_eq!(
            RECIPE_CONTROLS
                .iter()
                .filter(|c| c.tier == Some(Tier::RenderedNotExported))
                .map(|c| c.name)
                .collect::<Vec<_>>(),
            vec!["base_curve", "lens_profile"],
        );
    }

    /// R24-5 M0, IMPORT direction: a Lightroom sidecar's globals that AutoShade
    /// does not model. The merge keeps them; this is the sentence that says
    /// they are there — the global counterpart of the mask-side
    /// `unsupported_corrections`, which had no partner until now.
    #[test]
    fn an_imported_sidecar_names_the_globals_the_engine_does_not_render() {
        // FIXTURE NOTE, THIRD REVISION. `Texture` / `GrainAmount` were the
        // samples until R25 B2 modelled them; `PerspectiveUpright` took over
        // and B4 has now claimed that too. The samples are `PointColor` and
        // `CameraProfileDigest` — the latter chosen deliberately: it sits one
        // line from `crs:CameraProfile` in every real sidecar, and B4 owns the
        // profile NAME while the digest stays foreign. The list shrinking under
        // the fixtures, batch after batch, IS the complement definition working.
        let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                   <rdf:Description rdf:about=\"\" \
                   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                   crs:Exposure2012=\"+1.00\" crs:Texture=\"+30\" \
                   crs:PointColor=\"0\" crs:PerspectiveUpright=\"1\" \
                   crs:CameraProfile=\"Adobe Standard\" \
                   crs:CameraProfileDigest=\"2D1D4700365C3E2831EEAE0D1A8F9CDF\" \
                   crs:RawFileName=\"crs:NotAnAttribute=1.ARW\"/></rdf:RDF></x:xmpmeta>";
        let found = unmodelled_global_crs(doc);
        assert!(found.contains(&"PointColor".to_string()), "LR's PointColor: {found:?}");
        assert!(found.contains(&"CameraProfileDigest".to_string()), "{found:?}");
        // …and the B4 half of the same claim: the Transform / Calibration
        // blocks are ours now, so they left this list with no edit to it.
        assert!(
            !found.contains(&"PerspectiveUpright".to_string()),
            "PerspectiveUpright is passed through since R25 B4: {found:?}"
        );
        assert!(
            !found.contains(&"CameraProfile".to_string()),
            "the profile NAME is passed through; only its digest is foreign: {found:?}"
        );
        // A control we DO model is not "unmodelled" — the universe is the
        // complement of `owned_attr_keys`, which is what stops this list from
        // needing a catalogue of Adobe property names to keep up to date.
        assert!(!found.contains(&"Exposure2012".to_string()), "{found:?}");
        // …and the B2 half of that claim, which is the whole reason the
        // fixture above had to change: teaching the engine `crs:Texture` took
        // the key OFF this list with no edit to the list itself.
        assert!(
            !found.contains(&"Texture".to_string()),
            "Texture is modelled since R25 B2 and must have left this list: {found:?}"
        );
        // Quote-aware: `crs:` inside an attribute VALUE is text, not a
        // property (a RawFileName or a mask name may contain anything).
        assert!(!found.contains(&"NotAnAttribute".to_string()), "{found:?}");

        // Mask corrections live in a CHILD element and have their own
        // disclosure; reporting every crs:Local* key as an unmodelled global
        // would bury the real ones under sixty names.
        let r = EditRecipe {
            masks: vec![crate::recipe::LocalAdjustment {
                mask: crate::recipe::MaskGeometry::Linear {
                    zero_x: 0.0,
                    zero_y: 0.0,
                    full_x: 0.0,
                    full_y: 1.0,
                },
                enabled: true,
                amount: 1.0,
                exposure_ev: 0.5,
                ..Default::default()
            }],
            // Element-form children of OUR OWN making: the four tone curves
            // have no attribute spelling, so `owned_attr_keys` cannot exclude
            // them and only `OWNED_ELEMENT_ONLY` keeps the element arm below
            // from naming our own curves as Lightroom-only properties.
            tone_curve: vec![
                crate::recipe::CurvePoint { input: 0, output: 0 },
                crate::recipe::CurvePoint { input: 128, output: 140 },
                crate::recipe::CurvePoint { input: 255, output: 255 },
            ],
            red_curve: vec![
                crate::recipe::CurvePoint { input: 0, output: 0 },
                crate::recipe::CurvePoint { input: 255, output: 250 },
            ],
            ..Default::default()
        };
        let ours = recipe_to_xmp(&r);
        let found = unmodelled_global_crs(&ours);
        assert!(
            found.is_empty(),
            "a sidecar WE wrote models everything in it by construction: {found:?}"
        );
        // Nothing to read ⇒ nothing to say (never a panic, never a warning).
        assert!(unmodelled_global_crs("").is_empty());
        assert!(unmodelled_global_crs("<not xml").is_empty());
    }

    /// R24 round-end MED-1: the same disclosure for the PROPERTY-ELEMENT
    /// spelling. `crs_str` reads that form "for exactly that reason" and the
    /// merge strips it, because Lightroom writes it in plenty of real
    /// sidecars — but this scanner walked the Description's open TAG only, so
    /// an element-form catalog export disclosed nothing at all and the photo
    /// just rendered differently from Lightroom with no sentence on screen.
    #[test]
    fn the_import_disclosure_reads_property_element_globals_too() {
        let head = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                    xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">";
        let tail = "</rdf:RDF></x:xmpmeta>";
        // (a) Pure element form — the shape that returned EMPTY before.
        // (Same fixture note as the test above, third revision: `Texture` is
        // modelled since B2 and `PerspectiveUpright` is passed through since
        // B4, so the unmodelled samples are `PointColor` and
        // `UprightTransform` — the Upright SOLVER's own opaque blob, which is
        // in six of the seven reference sidecars and stays foreign.)
        let element = format!(
            "{head}<rdf:Description rdf:about=\"\" \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\">\
             <crs:Exposure2012>+1.00</crs:Exposure2012>\
             <crs:Texture>+30</crs:Texture>\
             <crs:PerspectiveUpright>1</crs:PerspectiveUpright>\
             <crs:PointColor>0</crs:PointColor>\
             <crs:UprightTransform>1.0000000</crs:UprightTransform>\
             </rdf:Description>{tail}"
        );
        let found = unmodelled_global_crs(&element);
        assert_eq!(
            found,
            vec!["PointColor", "UprightTransform"],
            "element-form globals must be named exactly once, and never the modelled \
             Exposure2012 / Texture or the passed-through PerspectiveUpright"
        );

        // (b) MIXED: Lightroom splits the same Description across both forms.
        let mixed = format!(
            "{head}<rdf:Description rdf:about=\"\" \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             crs:Exposure2012=\"+1.00\" crs:PointColor=\"0\">\
             <crs:UprightTransform>1.0000000</crs:UprightTransform>\
             </rdf:Description>{tail}"
        );
        assert_eq!(unmodelled_global_crs(&mixed), vec!["PointColor", "UprightTransform"]);

        // (c) The two exclusions the element walk must keep: a mask block's
        // `crs:Local*` items (their own disclosure) and a creative Look's
        // baked parameters (someone else's settings block, nested inside a
        // child) are NOT this Description's globals. `Look` itself IS one.
        let nested = format!(
            "{head}<rdf:Description rdf:about=\"\" \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\">\
             <crs:PointColor>0</crs:PointColor>\
             <crs:Look><rdf:Description><crs:Parameters><rdf:Description>\
             <crs:LookClarity2012>+50</crs:LookClarity2012>\
             </rdf:Description></crs:Parameters></rdf:Description></crs:Look>\
             <crs:MaskGroupBasedCorrections><rdf:Seq><rdf:li>\
             <rdf:Description crs:LocalExposure2012=\"0.5\" crs:LocalTexture=\"20\"/>\
             </rdf:li></rdf:Seq></crs:MaskGroupBasedCorrections>\
             </rdf:Description>{tail}"
        );
        assert_eq!(unmodelled_global_crs(&nested), vec!["Look", "PointColor"]);

        // (d) NIT-1: `-` and `.` are legal XML name characters. No Adobe key
        // uses either today, so this pins the reading rather than a change:
        // the whole name is reported, not the prefix before the hyphen.
        assert_eq!(
            unmodelled_global_crs(
                "<rdf:Description xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                 crs:Foo-Bar=\"1\" crs:Plain=\"2\"/>"
            ),
            vec!["Foo-Bar", "Plain"]
        );
    }

    /// L03-3: the import gate must MATCH the disclosure sentence — a
    /// conflicting crs binding imports nothing, because the scanners would
    /// read properties through a prefix the document bound elsewhere.
    #[test]
    fn a_conflicting_crs_binding_imports_nothing() {
        let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                   <rdf:Description rdf:about=\"\" xmlns:crs=\"urn:other\" \
                   crs:Exposure2012=\"+2.50\"/></rdf:RDF></x:xmpmeta>";
        assert!(xmlns_conflict(doc).is_some(), "the binding is a conflict");
        let r = xmp_to_recipe(doc);
        assert_eq!(
            r.exposure_ev, 0.0,
            "settings under a conflicting binding must not import — the disclosure says they were not"
        );
        assert!(
            unparsable_crs_numbers(doc)[0].contains("not imported"),
            "and the disclosure names the refusal"
        );
    }

    /// L03-4: the DEFAULT namespace declaration (bare `xmlns=`) bound to the
    /// camera-raw or RDF namespace is the same conflict as a foreign prefix —
    /// it hides settings in unprefixed spellings the scanners cannot see.
    #[test]
    fn a_default_namespace_binding_to_crs_is_a_conflict() {
        let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                   <rdf:Description rdf:about=\"\" \
                   xmlns=\"http://ns.adobe.com/camera-raw-settings/1.0/\">\
                   <Exposure2012>+1.00</Exposure2012>\
                   </rdf:Description></rdf:RDF></x:xmpmeta>";
        let why = xmlns_conflict(doc).expect("a default-namespace binding to crs must refuse");
        assert!(why.contains("DEFAULT namespace"), "the reason names the binding: {why}");
        assert_eq!(xmp_to_recipe(doc).exposure_ev, 0.0);
    }

    /// R12-03: bindings resolve in SCOPE — a nested island that rebinds `crs`
    /// around content that never says `crs:` is somebody else's metadata, not
    /// a reason to throw away the whole document's settings.
    #[test]
    fn an_unused_nested_rebind_no_longer_refuses_the_document() {
        let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                   <rdf:Description rdf:about=\"\" \
                   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                   crs:Exposure2012=\"+0.50\">\
                   <dc:island xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
                   xmlns:crs=\"urn:other\"><dc:note>hi</dc:note></dc:island>\
                   </rdf:Description></rdf:RDF></x:xmpmeta>";
        assert!(xmlns_conflict(doc).is_none(), "an unused rebind is harmless");
        assert_eq!(xmp_to_recipe(doc).exposure_ev, 0.5, "and the settings import");
    }

    /// R12-03: the rebind still refuses wherever a `crs:` name actually
    /// RESOLVES through it — here on a descendant deep inside the island.
    #[test]
    fn a_rebind_refuses_exactly_where_a_name_resolves_through_it() {
        let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                   <dc:island xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
                   xmlns:crs=\"urn:other\">\
                   <dc:inner crs:Shadows2012=\"+10\"/></dc:island>\
                   </rdf:RDF></x:xmpmeta>";
        let why = xmlns_conflict(doc).expect("a name resolving through the rebind refuses");
        assert!(why.contains("urn:other"), "the reason names the binding: {why}");
    }

    /// R12-03: a foreign alias for the camera-raw URI is inert while no name
    /// resolves through it, and a conflict the moment one does — settings
    /// spelled through the alias are invisible to the `crs:` scanners.
    #[test]
    fn a_foreign_alias_for_the_crs_uri_refuses_only_when_used() {
        let head = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                    xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" \
                    xmlns:zzz=\"http://ns.adobe.com/camera-raw-settings/1.0/\">\
                    <rdf:Description rdf:about=\"\"";
        let unused = format!("{head}/></rdf:RDF></x:xmpmeta>");
        assert!(xmlns_conflict(&unused).is_none(), "declared but never used");
        let used = format!("{head} zzz:Exposure2012=\"+1.00\"/></rdf:RDF></x:xmpmeta>");
        let why = xmlns_conflict(&used).expect("a name through the alias refuses");
        assert!(why.contains("`zzz:`"), "the reason names the prefix: {why}");
    }

    /// R12-03: a scope ends at its element's close tag — the island's rebind
    /// must not leak forward onto a following sibling whose `crs:` names
    /// resolve through the document-level canonical binding.
    #[test]
    fn a_closed_scope_releases_its_binding() {
        let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" \
                   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\">\
                   <dc:island xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
                   xmlns:crs=\"urn:other\"><dc:note>hi</dc:note></dc:island>\
                   <rdf:Description rdf:about=\"\" crs:Exposure2012=\"+0.50\"/>\
                   </rdf:RDF></x:xmpmeta>";
        assert!(
            xmlns_conflict(doc).is_none(),
            "the sibling's crs resolves through the canonical ancestor binding"
        );
    }

    /// R12-03 coordination: the scoped gate now clears a Description whose
    /// foreign `xmlns:crs` is unused — so the merge's target finder must not
    /// key on the attribute NAME alone, or it would splice canonical-intent
    /// `crs:` settings into a scope where `crs` means something else.
    #[test]
    fn the_merge_skips_a_description_whose_crs_binding_is_foreign() {
        let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                   <rdf:Description rdf:about=\"\" xmlns:crs=\"urn:other\"/>\
                   </rdf:RDF></x:xmpmeta>";
        assert!(xmlns_conflict(doc).is_none(), "unused foreign binding is cleared");
        assert_eq!(
            find_crs_description(doc),
            None,
            "and the merge must not adopt that Description as its settings target"
        );
    }

    /// R12-03: past the scope-tracking bound the gate cannot prove a binding
    /// harmless, so it refuses conservatively — never silently accepts.
    #[test]
    fn deeper_xmlns_nesting_than_tracked_refuses_conservatively() {
        let mut doc = String::new();
        for _ in 0..1025 {
            doc.push_str("<t xmlns:q=\"urn:x\">");
        }
        let why = xmlns_conflict(&doc).expect("beyond the bound is a refusal");
        assert!(why.contains("more xmlns declarations"), "{why}");
    }

    /// R13-01 (round-13 Codex review): a SURPLUS or MISNAMED close tag must
    /// not release a foreign binding early — pops are paired by name, not by
    /// arithmetic alone, so malformed nesting degrades toward refusal.
    #[test]
    fn a_mismatched_close_does_not_release_a_foreign_binding() {
        let doc = "<rdf:Description \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" \
                   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\">\
                   <island xmlns:crs=\"urn:foreign\"></bogus>\
                   <crs:Exposure2012>+2.0</crs:Exposure2012>\
                   </island></rdf:Description>";
        let why = xmlns_conflict(doc)
            .expect("the crs name still resolves through the un-closed island's rebind");
        assert!(why.contains("urn:foreign"), "the reason names the live binding: {why}");
    }

    /// R13-02 (round-13 Codex review): the tracking bound counts LIVE
    /// DECLARATIONS, not frames — a single tag carrying a declaration flood
    /// is past what the gate can resolve affordably, so it refuses.
    #[test]
    fn a_flat_declaration_flood_refuses_conservatively() {
        let mut doc = String::from("<t");
        for i in 0..257 {
            doc.push_str(&format!(" xmlns:q{i}=\"urn:x\""));
        }
        doc.push('>');
        let why = xmlns_conflict(&doc).expect("a declaration flood is a refusal");
        assert!(why.contains("more xmlns declarations"), "{why}");
    }

    /// L03-7: curve items are matched by tag name — a whitespace-spelled
    /// `<rdf:li >` is a real item, not an invisible one that empties the
    /// curve (and lets the next save delete it).
    #[test]
    fn a_whitespace_spelled_curve_item_is_still_a_curve_point() {
        let scope = "<crs:ToneCurvePV2012><rdf:Seq>\
                     <rdf:li >128, 64</rdf:li >\
                     <rdf:li>255, 255</rdf:li>\
                     </rdf:Seq></crs:ToneCurvePV2012>";
        assert_eq!(
            parse_curve_checked(scope, "ToneCurvePV2012"),
            Ok(vec![
                CurvePoint { input: 128, output: 64 },
                CurvePoint { input: 255, output: 255 },
            ]),
            "both spellings are legal XML for the same element"
        );
    }

    /// L03-9: HasCrop="True" whose coordinates are missing or inverted still
    /// imports as no-crop (clamping half a geometry would change coverage),
    /// but the drop is DISCLOSED — the next save persists HasCrop="False",
    /// and silence made that a deletion nobody asked for.
    #[test]
    fn an_inconsistent_crop_is_disclosed_not_silently_dropped() {
        let head = "<rdf:Description rdf:about=\"\" \
                    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                    crs:HasCrop=\"True\" crs:CropLeft=\"0.1\" crs:CropTop=\"0.1\" \
                    crs:CropRight=\"0.9\"/>";
        assert!(xmp_to_recipe(head).crop.is_none(), "a missing coordinate cannot crop");
        assert!(
            unparsable_crs_numbers(head).iter().any(|k| k.starts_with("Crop")),
            "the missing coordinate is disclosed: {:?}",
            unparsable_crs_numbers(head)
        );

        let inverted = "<rdf:Description rdf:about=\"\" \
                        xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                        crs:HasCrop=\"True\" crs:CropLeft=\"0.8\" crs:CropTop=\"0.1\" \
                        crs:CropRight=\"0.2\" crs:CropBottom=\"0.9\"/>";
        assert!(xmp_to_recipe(inverted).crop.is_none());
        assert!(
            unparsable_crs_numbers(inverted).iter().any(|k| k.starts_with("Crop")),
            "inverted ordering is disclosed"
        );

        let fine = "<rdf:Description rdf:about=\"\" \
                    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                    crs:HasCrop=\"True\" crs:CropLeft=\"0.1\" crs:CropTop=\"0.1\" \
                    crs:CropRight=\"0.9\" crs:CropBottom=\"0.9\"/>";
        assert!(xmp_to_recipe(fine).crop.is_some());
        assert!(
            unparsable_crs_numbers(fine).is_empty(),
            "a consistent crop discloses nothing"
        );
    }

    /// L03-18: raw tab/newline in an attribute value would be folded to
    /// spaces by any compliant parser's attribute-value normalization —
    /// character references survive it, and our reader decodes them back.
    #[test]
    fn attribute_control_characters_survive_as_character_references() {
        assert_eq!(xml_attr_escape("a\tb\nc\rd"), "a&#9;b&#10;c&#13;d");
        assert_eq!(xml_unescape("a&#9;b&#10;c&#13;d").as_ref(), "a\tb\nc\rd");
    }

    use crate::recipe::{CurvePoint, EditRecipe, LocalAdjustment};

    /// The merged document alone — most tests assert on the text; the ones
    /// about [`MergeOutcome::notes`] call the real function.
    fn merged_doc(existing: &str, r: &EditRecipe) -> Option<String> {
        merge_recipe_into_xmp(existing, r).map(|o| o.doc)
    }

    /// The scope scanner meets a sidecar that is hostile rather than merely
    /// unusual. Both halves were real defects: the close search restarted on
    /// every nested open (Θ(k²) — this document took MINUTES before, inside
    /// SAVE_LOCK and holding a server request permit), and a
    /// `</rdf:Description>` inside a COMMENT was read as a real close, which
    /// truncated the body and sank the whole merge to a fresh document,
    /// dropping the Lightroom-only properties the merge exists to preserve.
    #[test]
    fn a_pathological_sidecar_neither_hangs_nor_believes_a_comment() {
        // (a) Deep nesting: linear now, quadratic before. 20 000 opens is
        // ~0.4 MB. MEASURED on this box: 0.02 s with the cached close cursor,
        // 13.62 s when the cache is removed — 680x, on a file a user could
        // receive by opening someone else's shoot. The assertion below pins
        // correctness; the wall clock is the pin on the complexity, so keep
        // the size when editing this test.
        let mut doc = String::from(
            r#"<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="+0.50">"#,
        );
        let gt = doc.len() - 1;
        for _ in 0..20_000 {
            doc.push_str("<rdf:Description>");
        }
        for _ in 0..20_000 {
            doc.push_str("</rdf:Description>");
        }
        doc.push_str("</rdf:Description>");
        let close = find_matching_close(&doc, gt + 1).expect("the outermost close is found");
        assert_eq!(&doc[close..close + 18], "</rdf:Description>");
        assert_eq!(close, doc.len() - 18, "it is the LAST one, not an inner one");

        // (b) A comment holding the close literal is TEXT, not a close.
        let doc = format!(
            "{}<!-- </rdf:Description> --><crs:Texture>25</crs:Texture></rdf:Description>",
            r#"<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">"#
        );
        let gt = doc.find('>').unwrap();
        let close = find_matching_close(&doc, gt + 1).expect("the real close is found");
        assert_eq!(close, doc.len() - 18, "the comment's copy is not a close");
        // …and the scope therefore still carries the child that follows it.
        let scope = crs_own_scope(&doc);
        assert!(scope.contains("crs:Texture"), "the body survived the comment: {scope}");

        // (c) CDATA gets the same treatment.
        let doc = format!(
            "{}<![CDATA[ </rdf:Description> ]]><crs:Texture>25</crs:Texture></rdf:Description>",
            r#"<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">"#
        );
        let gt = doc.find('>').unwrap();
        assert_eq!(find_matching_close(&doc, gt + 1), Some(doc.len() - 18));

        // (d) An UNTERMINATED comment is unaccountable markup: no close at all,
        // so the caller falls back to the whole document rather than guessing.
        let doc = format!(
            "{}<!-- </rdf:Description>",
            r#"<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">"#
        );
        let gt = doc.find('>').unwrap();
        assert_eq!(find_matching_close(&doc, gt + 1), None);
    }

    /// The complexity itself, asserted — because the correctness test above
    /// passes at ANY speed, which is how the second blowup shipped.
    ///
    /// The scanner has had two separate quadratic shapes. The cached close
    /// cursor killed the first (nesting) and the construct skip it shipped
    /// alongside introduced the second, on a shape half the size: measured
    /// with release-mode replicas of the committed code, 640 KB of
    /// back-to-back comments took **8.47 s** and 400 KB of PIs **9.59 s**,
    /// against 51 µs and 90 µs for the code that predated the construct skip.
    /// Quadratic scaling (4x bytes -> 16x time) put the 16 MiB `read_sidecar`
    /// ceiling at roughly an hour and a half — spent inside SAVE_LOCK holding
    /// one of the server's eight request permits, reachable by SELECTING a
    /// photo that has such a sidecar beside it.
    ///
    /// So both shapes are pinned by wall clock here. The budget is deliberately
    /// loose (a debug build on a loaded CI box is not a benchmark); it only has
    /// to separate "linear" from "quadratic", and the gap is five orders of
    /// magnitude. Keep the SIZES if you edit this test — they are the pin.
    #[test]
    fn the_scope_scanner_is_linear_on_both_pathological_shapes() {
        const BUDGET: std::time::Duration = std::time::Duration::from_secs(10);
        let head = r#"<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">"#;

        // Shape 1 — deep nesting (the first blowup). 80 000 opens, ~2.8 MB.
        let mut nested = String::from(head);
        let gt = nested.len() - 1;
        for _ in 0..80_000 {
            nested.push_str("<rdf:Description>");
        }
        for _ in 0..80_000 {
            nested.push_str("</rdf:Description>");
        }
        nested.push_str("</rdf:Description>");

        // Shape 2 — a body of back-to-back comments (the second blowup),
        // and 3 — the same with PIs, which took even longer per byte.
        let commented = format!("{head}{}</rdf:Description>", "<!--x-->".repeat(80_000));
        let pis = format!("{head}{}</rdf:Description>", "<?p?>".repeat(80_000));

        for (name, doc) in [("nested", &nested), ("comments", &commented), ("PIs", &pis)] {
            let started = std::time::Instant::now();
            let close = find_matching_close(doc, gt + 1).expect("the outermost close is found");
            let elapsed = started.elapsed();
            assert_eq!(close, doc.len() - 18, "{name}: it is the LAST close, not an inner one");
            assert!(
                elapsed < BUDGET,
                "{name}: {} bytes scanned in {elapsed:?}, over the {BUDGET:?} budget — \
                 a landmark cursor is being recomputed on every iteration again",
                doc.len()
            );
        }
    }

    /// A creative profile's baked parameters are the PROFILE's, never the
    /// photographer's. Adobe nests them as owned-LOOKING crs children of a
    /// second `rdf:Description` (`<crs:Look><rdf:Description><crs:Parameters>
    /// <rdf:Description><crs:Clarity2012>…`) — the exact shape the WRITER's
    /// depth-aware strip was built for. The reader's flat scan answered from
    /// them whenever the top level omitted the key, so opening such a sidecar
    /// wrote the profile's look into the user's sliders and the next save
    /// persisted it.
    #[test]
    fn a_nested_look_is_not_a_user_edit() {
        let doc = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    crs:Version="15.5.1"
    crs:Exposure2012="+0.20">
   <crs:Look>
    <rdf:Description crs:Name="Adobe Landscape">
     <crs:Parameters>
      <rdf:Description>
       <crs:Clarity2012>+50</crs:Clarity2012>
       <crs:Vibrance>+35</crs:Vibrance>
       <crs:ToneCurvePV2012>
        <rdf:Seq>
         <rdf:li>0, 30</rdf:li>
         <rdf:li>255, 255</rdf:li>
        </rdf:Seq>
       </crs:ToneCurvePV2012>
      </rdf:Description>
     </crs:Parameters>
    </rdf:Description>
   </crs:Look>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;
        let r = xmp_to_recipe(doc);
        assert_eq!(r.exposure_ev, 0.20, "the Description's OWN attribute imports");
        assert_eq!(r.clarity, 0.0, "the Look's Clarity2012 is not a user edit: {}", r.clarity);
        assert_eq!(r.vibrance, 0.0, "the Look's Vibrance is not a user edit: {}", r.vibrance);
        assert!(r.tone_curve.is_empty(), "the Look's baked curve is not a user curve");
        // The disclosure follows the import: a corrupt number the import never
        // reads must not be announced as a setting that will be lost.
        let corrupt = doc.replace("<crs:Clarity2012>+50</crs:Clarity2012>", "<crs:Clarity2012>--</crs:Clarity2012>");
        assert!(
            unparsable_crs_numbers(&corrupt).is_empty(),
            "only settings the import READS may be disclosed: {:?}",
            unparsable_crs_numbers(&corrupt)
        );
        // …and the same key AT top level still imports, in both spellings.
        for own in [
            r#"crs:Exposure2012="+0.20" crs:Clarity2012="+12""#.to_string(),
            r#"crs:Exposure2012="+0.20">
   <crs:Clarity2012>+12</crs:Clarity2012"#
                .to_string(),
        ] {
            let d = doc.replace(r#"crs:Exposure2012="+0.20""#, &own);
            assert_eq!(xmp_to_recipe(&d).clarity, 12.0, "own Clarity2012 must import: {own}");
        }
    }

    /// The scope keeps what the Description really owns — masks (whose nested
    /// Descriptions are its own mask items) and plain property elements — and
    /// falls back to the whole document when the markup cannot be accounted
    /// for, which is the pre-scope behaviour.
    #[test]
    fn the_crs_scope_keeps_owned_children_and_degrades_safely() {
        let mut r = EditRecipe {
            exposure_ev: 0.5,
            tone_curve: vec![CurvePoint { input: 0, output: 12 }, CurvePoint { input: 255, output: 250 }],
            ..Default::default()
        };
        r.masks.push(LocalAdjustment {
            mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.9, full_x: 0.5, full_y: 0.1 },
            name: "sky".into(),
            exposure_ev: -0.4,
            ..Default::default()
        });
        // A full round-trip through the scope: masks and curves are OWNED and
        // must survive it.
        let doc = recipe_to_xmp(&r);
        let back = xmp_to_recipe(&doc);
        assert_eq!(back.tone_curve, r.tone_curve, "the owned tone curve survives the scope");
        assert_eq!(back.masks.len(), 1, "owned mask corrections survive the scope");
        assert_eq!(back.masks[0].name, "sky");
        // Markup the scanner cannot account for (an unclosed element) falls
        // back to the whole document rather than losing every setting.
        let broken = doc.replace("</rdf:Description>", "");
        assert!(crs_scope_inner(&broken).is_none(), "unaccountable markup yields no scope");
        assert_eq!(
            xmp_to_recipe(&broken).exposure_ev,
            0.5,
            "the fallback still reads the document"
        );
    }

    #[test]
    fn straighten_only_activates_crop_and_round_trips_to_no_crop() {
        // Lightroom applies CropAngle only under HasCrop="True" — a
        // straighten-only recipe ships the full frame as its carrier, and the
        // reader collapses that full-frame rectangle back to None.
        let r = EditRecipe { straighten_deg: 2.5, ..Default::default() };
        let x = recipe_to_xmp(&r);
        assert!(x.contains("crs:HasCrop=\"True\""), "straighten must activate the crop state");
        // R27: `crs:CropAngle` is the NEGATION of this engine's clockwise
        // straighten (`P3-cropangle-model.md` §4 — Lightroom turns the content
        // counter-clockwise by +CropAngle, measured on six photographs, 34×
        // margin on the weakest), and it goes out with Lightroom's own six
        // decimals rather than the one this writer used to round to (§6.4).
        assert!(x.contains("crs:CropAngle=\"-2.500000\""), "{x}");
        let back = xmp_to_recipe(&x);
        assert_eq!(back.crop, None, "the full-frame carrier must not become a real crop");
        assert_eq!(back.straighten_deg, 2.5);
        // Control chars in a mask name must not poison the document.
        let dirty = EditRecipe {
            masks: vec![LocalAdjustment { name: "sky\u{0}\u{7}".into(), ..Default::default() }],
            ..Default::default()
        };
        let x = recipe_to_xmp(&dirty);
        assert!(!x.contains('\u{0}') && !x.contains('\u{7}'), "forbidden chars stripped");
    }

    #[test]
    fn renders_local_masks_with_correct_scale() {
        let r = EditRecipe {
            masks: vec![
                LocalAdjustment {
                    mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.35, full_x: 0.5, full_y: 0.0 },
                    name: "sky".into(),
                    exposure_ev: -0.4,  // ÷4 → -0.1
                    highlights: -50.0,  // ÷100 → -0.5
                    ..Default::default()
                },
                LocalAdjustment {
                    mask: MaskGeometry::Radial {
                        top: 0.3, left: 0.35, bottom: 0.7, right: 0.65,
                        feather: 0.5, roundness: 0.0, flipped: false, angle: 0.0,
                        midpoint: 50.0, mask_version: 2,
                    },
                    name: "subject".into(),
                    shadows: 20.0,      // ÷100 → 0.2
                    inverted: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        // Write it out so well-formedness can be validated by an XML parser
        // (out/ is gitignored). Verification aid, not a behavioural assertion.
        std::fs::create_dir_all("out").ok();
        std::fs::write("out/_masks_test.xmp", &xmp).ok();
        assert!(xmp.contains("<crs:MaskGroupBasedCorrections>"));
        assert!(xmp.contains(r#"crs:What="Mask/Gradient""#));
        assert!(xmp.contains(r#"crs:What="Mask/CircularGradient""#));
        // local scale conversions
        assert!(xmp.contains(r#"crs:LocalExposure2012="-0.1""#)); // -0.4 / 4
        assert!(xmp.contains(r#"crs:LocalHighlights2012="-0.5""#)); // -50 / 100
        assert!(xmp.contains(r#"crs:LocalShadows2012="0.2""#)); // 20 / 100
        assert!(xmp.contains(r#"crs:MaskInverted="true""#));
        assert!(xmp.contains(r#"crs:ZeroX="0.5""#));
        // Feather crosses the boundary on Lightroom's 0..100 scale (engine 0.5
        // → crs 50) — the old writer's raw "0.5" read in LR as a hard edge.
        assert!(xmp.contains(r#"crs:Feather="50""#));
        // unset masks ⇒ no mask block (v1-compatible)
        assert!(!recipe_to_xmp(&EditRecipe::default()).contains("MaskGroupBasedCorrections"));
    }

    #[test]
    fn radial_feather_converts_both_ways_and_keeps_legacy_own_scale() {
        // LR-style integer feather imports onto the engine's 0..1 scale…
        //
        // R25 P9: wrapped in a real `<rdf:li …/>`. It used to be a bare
        // attribute run, which worked only because the geometry was located by
        // substring; `base_geometry_at` scans TAGS, because choosing the base
        // out of several components means reading each component's own
        // `crs:MaskBlendMode` and that is a per-tag question. Every production
        // caller already hands whole markup — `classify_correction`, the only
        // one, tag-scans the same block itself — so the fixture was the thing
        // that did not look like a sidecar.
        //
        // R27 Batch-4 took the same step again, one level out: the component
        // list is now located by NAME (`crs:CorrectionMasks`) rather than
        // scanned for anywhere in the segment, because "which components are
        // this correction's own" is the question hazards 1 and 2 turned on.
        // `classify_correction` has always required that element — it returns
        // `Unrepresentable` without one — so the two now agree about where a
        // correction's components live, and the fixture wears the wrapper its
        // production caller always supplies.
        let li = r#"<crs:CorrectionMasks><rdf:Seq><rdf:li crs:What="Mask/CircularGradient" crs:Top="0.2" crs:Left="0.2" crs:Bottom="0.8" crs:Right="0.8" crs:Feather="72" crs:Roundness="0" crs:Flipped="false"/></rdf:Seq></crs:CorrectionMasks>"#;
        // The correction's own scope is empty here BY CONSTRUCTION: this
        // fixture is a bare component list with no correction tag around it,
        // so there are no sliders to read and the geometry is the whole claim.
        let m = parse_one_correction(li, Scope::new(""), None).expect("radial parses");
        let MaskGeometry::Radial { feather, .. } = m.mask else { panic!("radial") };
        assert!((feather - 0.72).abs() < 1e-6, "LR 72 → 0.72, got {feather}");
        // …while a legacy own-writer value (≤ 1.0) passes through verbatim.
        let legacy = li.replace(r#"crs:Feather="72""#, r#"crs:Feather="0.4""#);
        let m =
            parse_one_correction(&legacy, Scope::new(""), None).expect("legacy radial parses");
        let MaskGeometry::Radial { feather, .. } = m.mask else { panic!("radial") };
        assert!((feather - 0.4).abs() < 1e-6, "legacy 0.4 stays 0.4, got {feather}");
    }

    #[test]
    fn renders_manual_vignette_only_when_set() {
        let r = EditRecipe { lens_vignette: 35.0, lens_vignette_mid: 60.0, ..Default::default() };
        let xmp = recipe_to_xmp(&r);
        assert!(xmp.contains(r#"crs:VignetteAmount="+35""#));
        assert!(xmp.contains(r#"crs:VignetteMidpoint="60""#));
        // A neutral recipe emits neither key (byte-compatible with the old writer).
        let neutral = recipe_to_xmp(&EditRecipe::default());
        assert!(!neutral.contains("VignetteAmount") && !neutral.contains("VignetteMidpoint"));
    }

    #[test]
    fn renders_manual_distortion_only_when_set() {
        let r = EditRecipe { lens_distortion: -24.0, ..Default::default() };
        assert!(recipe_to_xmp(&r).contains(r#"crs:LensManualDistortionAmount="-24""#));
        let pos = EditRecipe { lens_distortion: 80.0, ..Default::default() };
        assert!(recipe_to_xmp(&pos).contains(r#"crs:LensManualDistortionAmount="+80""#));
        // Zero amount emits no key at all (byte-compatible with the old writer).
        assert!(!recipe_to_xmp(&EditRecipe::default()).contains("LensManualDistortionAmount"));
    }

    /// R25 B2: global Texture round-trips through its own `crs:Texture` key,
    /// in Lightroom's own signed form.
    ///
    /// READ AND WRITE IN ONE BATCH is not a nicety here. `owned_attr_keys` is
    /// the WRITER's universe and also the merge's STRIP universe, so a
    /// read-only Texture would have left Lightroom's value in the document
    /// beside ours (two answers for one slider), and a write-only one would
    /// have gone on being named by `unmodelled_global_crs` while quietly
    /// rendering. Either half alone is a defect; this pins both.
    #[test]
    fn texture_round_trips_through_xmp() {
        let xmp = recipe_to_xmp(&EditRecipe { texture: 26.0, ..Default::default() });
        assert!(xmp.contains(r#"crs:Texture="+26""#), "the signed form Lightroom writes: {xmp}");
        assert_eq!(xmp_to_recipe(&xmp).texture, 26.0);
        // Negative and neutral, and the key is UNCONDITIONAL like its four
        // Basic-panel neighbours (Clarity2012 / Dehaze / Vibrance / Saturation).
        let neg = recipe_to_xmp(&EditRecipe { texture: -40.0, ..Default::default() });
        assert!(neg.contains(r#"crs:Texture="-40""#));
        assert_eq!(xmp_to_recipe(&neg).texture, -40.0);
        assert!(recipe_to_xmp(&EditRecipe::default()).contains(r#"crs:Texture="0""#));
        // A FOREIGN sidecar's Texture is a real import, not a disclosure line.
        let lr = "<rdf:Description rdf:about=\"\" \
                  xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                  crs:Texture=\"+26\"/>";
        assert_eq!(xmp_to_recipe(lr).texture, 26.0, "the LR value must arrive");
    }

    /// R25 B2, the strip arm: a cleared Texture must DISAPPEAR from a merged
    /// document rather than linger at Lightroom's old value.
    ///
    /// This is what owning a key means. Before B2, `crs:Texture` was foreign
    /// property and the merge preserved it verbatim (there are four tests
    /// above that used it as the example of exactly that). Now the merge
    /// strips it and rewrites ours — and if `owned_attr_keys` had gained the
    /// key without the writer emitting it, or the writer without the key, this
    /// document would answer one slider twice.
    #[test]
    fn a_cleared_texture_disappears_from_a_merged_document() {
        let lr = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
                  <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
                  <rdf:Description rdf:about=\"\"\n    \
                  xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n    \
                  crs:Texture=\"+20\" crs:PointColor=\"0\" crs:HasSettings=\"True\">\n  \
                  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n";
        let merged = merged_doc(lr, &EditRecipe { texture: -8.0, ..Default::default() })
            .expect("a plain LR sidecar is mergeable");
        assert_eq!(merged.matches("crs:Texture=").count(), 1, "one answer only: {merged}");
        assert!(merged.contains(r#"crs:Texture="-8""#), "…and it is OURS: {merged}");
        assert!(merged.contains("crs:PointColor=\"0\""), "an unmodelled global still survives");
        // Cleared to neutral: the old +20 is gone, not resurrected.
        let cleared = merged_doc(lr, &EditRecipe::default()).expect("mergeable");
        assert_eq!(cleared.matches("crs:Texture=").count(), 1);
        assert!(cleared.contains(r#"crs:Texture="0""#), "the stale +20 must not linger: {cleared}");
        assert_eq!(xmp_to_recipe(&cleared).texture, 0.0);
    }

    /// R25 B3: the eight carried DETAIL axes and the manual CA pair
    /// round-trip, each in the spelling Lightroom itself uses.
    ///
    /// The spellings are FIRST-HAND, from all seven sidecars in the user's
    /// library: `SharpenRadius="+1.0"` carries an explicit sign and one
    /// decimal, while every integer neighbour is bare (`SharpenDetail="25"`,
    /// `SharpenEdgeMasking="0"`, `ColorNoiseReduction="25"`). Getting that
    /// backwards is not cosmetic — it is the difference between a sidecar
    /// Lightroom reads as its own and one it merely tolerates.
    #[test]
    fn detail_subcontrols_round_trip() {
        let r = EditRecipe {
            sharpen_radius: 1.0,
            sharpen_detail: 25.0,
            sharpen_mask: 12.0,
            nr_detail: 50.0,
            nr_contrast: 8.0,
            color_nr: 25.0,
            color_nr_detail: 50.0,
            color_nr_smooth: 50.0,
            ca_r: 14.0,
            ca_b: -9.0,
            auto_lateral_ca: true,
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        for want in [
            r#"crs:SharpenRadius="+1.0""#,
            r#"crs:SharpenDetail="25""#,
            r#"crs:SharpenEdgeMasking="12""#,
            r#"crs:LuminanceNoiseReductionDetail="50""#,
            r#"crs:LuminanceNoiseReductionContrast="8""#,
            r#"crs:ColorNoiseReduction="25""#,
            r#"crs:ColorNoiseReductionDetail="50""#,
            r#"crs:ColorNoiseReductionSmoothness="50""#,
            r#"crs:ChromaticAberrationR="14""#,
            r#"crs:ChromaticAberrationB="-9""#,
            r#"crs:AutoLateralCA="1""#,
        ] {
            assert!(xmp.contains(want), "{want} missing from: {xmp}");
        }
        let back = xmp_to_recipe(&xmp);
        for (name, live, want) in [
            ("sharpen_radius", back.sharpen_radius, r.sharpen_radius),
            ("sharpen_detail", back.sharpen_detail, r.sharpen_detail),
            ("sharpen_mask", back.sharpen_mask, r.sharpen_mask),
            ("nr_detail", back.nr_detail, r.nr_detail),
            ("nr_contrast", back.nr_contrast, r.nr_contrast),
            ("color_nr", back.color_nr, r.color_nr),
            ("color_nr_detail", back.color_nr_detail, r.color_nr_detail),
            ("color_nr_smooth", back.color_nr_smooth, r.color_nr_smooth),
            ("ca_r", back.ca_r, r.ca_r),
            ("ca_b", back.ca_b, r.ca_b),
        ] {
            assert_eq!(live, want, "{name} did not survive the round trip");
        }
        assert!(back.auto_lateral_ca, "the auto-CA flag must come back on");
        // A neutral recipe writes NONE of them: an absent key is how
        // Lightroom is told to keep its own default (Radius 1.0, Detail 25,
        // Colour NR 25/50/50), and inventing a zero for each would be a
        // change to the photo, not a faithful silence.
        let neutral = recipe_to_xmp(&EditRecipe::default());
        for key in [
            "SharpenRadius",
            "SharpenDetail",
            "SharpenEdgeMasking",
            "LuminanceNoiseReduction",
            "ColorNoiseReduction",
            "ChromaticAberration",
            "AutoLateralCA",
        ] {
            assert!(!neutral.contains(key), "{key} must be absent from a neutral sidecar");
        }
        // …and the merge STRIPS them, so a cleared value cannot linger at
        // Lightroom's old number beside ours.
        let lr = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
                  <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
                  <rdf:Description rdf:about=\"\"\n    \
                  xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n    \
                  crs:SharpenRadius=\"+1.0\" crs:ColorNoiseReduction=\"25\" \
                  crs:AutoLateralCA=\"1\" crs:HasSettings=\"True\">\n  \
                  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n";
        assert_eq!(xmp_to_recipe(lr).color_nr, 25.0, "premise: the LR values import");
        assert!(xmp_to_recipe(lr).auto_lateral_ca, "premise: so does the flag");
        let cleared = merged_doc(lr, &EditRecipe::default()).expect("a plain LR sidecar merges");
        for key in ["SharpenRadius", "ColorNoiseReduction", "AutoLateralCA"] {
            assert!(!cleared.contains(key), "{key} survived a clear: {cleared}");
        }
    }

    /// v0.31.1: `crs:Sharpness` is Lightroom's Detail > Sharpening **Amount**
    /// slider stored 1:1, and that slider's UI maximum is **150**, not 100.
    ///
    /// EVIDENCE (web survey of GitHub-hosted `.xmp`, 2026-08-18; 566
    /// `crs:Sharpness` occurrences across 636 files, histogram maximum 150):
    /// fifteen REAL sidecars carry `crs:Sharpness="150"` — fourteen from
    /// `maxbordogna/finger_counting` (`tiff:Model="NIKON Z 6"`,
    /// `crs:Version="15.3"` / `ProcessVersion="11.0"`) and one from
    /// `ninjahisser/Fotos` (`NIKON Z 30`, `crs:Version="17.2"` /
    /// `ProcessVersion="15.4"`), the latter with the value sitting inside the
    /// Detail attribute group beside `SharpenRadius="+1.0"` /
    /// `SharpenDetail="25"` / `SharpenEdgeMasking="0"` in a file that names its
    /// own raw. Two repositories, two camera bodies, two Lightroom
    /// generations. The sidecar values are also copied here as TEXT ONLY —
    /// no harvested file enters this repository.
    ///
    /// The 0..100 belief was load-bearing in four places, all deleted with it:
    /// the reader's ×1.5, the writer's ×⅔, the `Sharpness` special case in
    /// `crs_number_is_in_recipe_range`, and an assertion that PINNED
    /// `100 × 1.5 == 150` as an invariant. This test is the replacement pin —
    /// it fails on every one of those four.
    #[test]
    fn a_full_lightroom_sharpening_amount_imports_as_itself() {
        // The maximum a real Lightroom writes. Synthetic document, real value.
        let doc = |v: &str| {
            format!(
                "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF \
                 xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
                 <rdf:Description rdf:about=\"\"\n    \
                 xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n    \
                 crs:Sharpness=\"{v}\" crs:SharpenRadius=\"+1.0\" crs:SharpenDetail=\"25\"\n    \
                 crs:SharpenEdgeMasking=\"0\" crs:HasSettings=\"True\">\n  \
                 </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n"
            )
        };
        // 1. A full 150 arrives as 150. Under the old reader it was 225,
        //    clamped back to 150 by luck — and under the old BAND it was also
        //    reported as an unparsable number, i.e. the app told the user a
        //    perfectly ordinary Lightroom file was broken.
        let full = xmp_to_recipe(&doc("150"));
        assert_eq!(full.sharpening, 150.0, "a full Lightroom Amount is 150 here too");
        assert!(
            unparsable_crs_numbers(&doc("150")).is_empty(),
            "150 is an ordinary value, not a defect: {:?}",
            unparsable_crs_numbers(&doc("150"))
        );
        // 2. The user's own library value. 40 in, 40 out — it used to be 60.
        assert_eq!(xmp_to_recipe(&doc("40")).sharpening, 40.0);
        // 3. WYSIWYG on the way back out: what the engine renders is what the
        //    sidecar says. A rendered 60 used to be written back as 40.
        let sixty = EditRecipe { sharpening: 60.0, ..Default::default() };
        assert!(
            recipe_to_xmp(&sixty).contains(r#"crs:Sharpness="60""#),
            "{}",
            recipe_to_xmp(&sixty)
        );
        // …and 150 survives a full round trip, which the old ×⅔ writer could
        // not do at all: it had no way to SAY 150.
        assert!(recipe_to_xmp(&full).contains(r#"crs:Sharpness="150""#));
        assert_eq!(xmp_to_recipe(&recipe_to_xmp(&full)).sharpening, 150.0);
        // 4. The band still has a ceiling, and it is the row's: 151 is out.
        assert!(
            unparsable_crs_numbers(&doc("151")).iter().any(|k| k == "Sharpness"),
            "past the slider's own maximum is still disclosed"
        );
    }

    /// R25 B3 (policy SF4-C): de-fringe round-trips through BOTH spellings
    /// Lightroom writes — the `rdf:Description` attribute form and the
    /// property-ELEMENT form.
    ///
    /// `crs_str` already reads both (that is why the reader adds no third
    /// scanning arm), and this is the test that keeps it that way. Positive
    /// amounts are UNSIGNED: `DefringePurpleAmount="3"`, never `"+3"` — the
    /// `Sharpness="40"` family, not the `Contrast2012="+22"` one.
    #[test]
    fn defringe_round_trips_both_serialization_forms() {
        let r = EditRecipe {
            defringe_purple: 3.0,
            defringe_purple_lo: 39.0,
            defringe_purple_hi: 79.0,
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        assert!(xmp.contains(r#"crs:DefringePurpleAmount="3""#), "unsigned, like Sharpness: {xmp}");
        assert!(!xmp.contains(r#"DefringePurpleAmount="+3""#), "no `+` on this family");
        let back = xmp_to_recipe(&xmp);
        assert_eq!(
            (back.defringe_purple, back.defringe_purple_lo, back.defringe_purple_hi),
            (3.0, 39.0, 79.0)
        );
        // The ELEMENT form, in the wild on photoprism's canon_eos_6d fixture
        // family (alphabetical, one child per key).
        let elem = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
                    <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
                    <rdf:Description rdf:about=\"\"\n    \
                    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\">\n   \
                    <crs:DefringeGreenAmount>5</crs:DefringeGreenAmount>\n   \
                    <crs:DefringeGreenHueHi>66</crs:DefringeGreenHueHi>\n   \
                    <crs:DefringeGreenHueLo>44</crs:DefringeGreenHueLo>\n   \
                    <crs:DefringePurpleAmount>7</crs:DefringePurpleAmount>\n  \
                    </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n";
        let e = xmp_to_recipe(elem);
        assert_eq!(
            (e.defringe_green, e.defringe_green_lo, e.defringe_green_hi, e.defringe_purple),
            (5.0, 44.0, 66.0, 7.0),
            "the element form must read exactly like the attribute form"
        );
        // The purple WINDOW was not named by that document — it must fall
        // back to Adobe's default, not to the zero `crs_f32` answers.
        assert_eq!(
            (e.defringe_purple_lo, e.defringe_purple_hi),
            (30.0, 70.0),
            "an unnamed hue window is Adobe's default, never 0..0"
        );
    }

    /// R25 B3: a NON-default hue window survives — the case the fallback
    /// above must not swallow.
    ///
    /// 39/79 is the real shape from a Lightroom preset (`lightA1`): the
    /// amount at 3 with the window moved off 30/70. If the reader ever
    /// "normalised" a window it did not recognise, this is what would be
    /// silently rewritten to Adobe's default on the next save.
    #[test]
    fn nondefault_hue_bounds_survive() {
        let lr = "<rdf:Description rdf:about=\"\" \
                  xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                  crs:DefringePurpleAmount=\"3\" crs:DefringePurpleHueLo=\"39\" \
                  crs:DefringePurpleHueHi=\"79\"/>";
        let r = xmp_to_recipe(lr);
        assert_eq!((r.defringe_purple_lo, r.defringe_purple_hi), (39.0, 79.0));
        let round = xmp_to_recipe(&recipe_to_xmp(&r));
        assert_eq!(
            (round.defringe_purple, round.defringe_purple_lo, round.defringe_purple_hi),
            (3.0, 39.0, 79.0),
            "a moved window must not snap back to 30/70 on a save"
        );
        // The green half of the same document said nothing, so it comes back
        // as Adobe's 40/60 — and that is what gets written, which is the
        // shape every real sidecar has.
        assert_eq!((round.defringe_green_lo, round.defringe_green_hi), (40.0, 60.0));
        assert!(recipe_to_xmp(&r).contains(r#"crs:DefringeGreenHueLo="40""#));
    }

    /// R25 B3: the whole de-fringe block is written UNCONDITIONALLY, and a
    /// document carrying exactly Adobe's defaults imports as a NO-OP.
    ///
    /// Both halves of the non-zero-neutral decision in one place. The first
    /// is the shape Lightroom writes — 7 of 7 of the user's sidecars carry
    /// all six keys with the amounts at 0 — so a recipe that says nothing
    /// still produces a document that looks like one Lightroom made. The
    /// second is what stops that from making every photo "edited": the six
    /// values are `EditRecipe::default()`'s own, so `is_noop` still answers
    /// yes. A reader that took `crs_f32`'s absent-key zero instead would
    /// import a 0..0 hue window and fail this, which is exactly the bug the
    /// fallback exists to prevent.
    #[test]
    fn a_real_defringe_block_imports_as_a_noop() {
        let neutral = recipe_to_xmp(&EditRecipe::default());
        for want in [
            r#"crs:DefringePurpleAmount="0""#,
            r#"crs:DefringePurpleHueLo="30""#,
            r#"crs:DefringePurpleHueHi="70""#,
            r#"crs:DefringeGreenAmount="0""#,
            r#"crs:DefringeGreenHueLo="40""#,
            r#"crs:DefringeGreenHueHi="60""#,
        ] {
            assert!(neutral.contains(want), "{want} missing — the block is unconditional: {neutral}");
        }
        // The real shape, verbatim from the user's library (DSC08761 line 139
        // onward), on an otherwise empty document.
        let real = "<rdf:Description rdf:about=\"\" \
                    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                    crs:DefringePurpleAmount=\"0\" crs:DefringePurpleHueLo=\"30\" \
                    crs:DefringePurpleHueHi=\"70\" crs:DefringeGreenAmount=\"0\" \
                    crs:DefringeGreenHueLo=\"40\" crs:DefringeGreenHueHi=\"60\"/>";
        assert!(
            xmp_to_recipe(real).is_noop(),
            "a sidecar carrying only Adobe's own de-fringe defaults is not an edit"
        );
        // …and so is a document that never mentions de-fringe at all — the
        // OTHER direction of the same fallback.
        let silent = "<rdf:Description rdf:about=\"\" \
                      xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"/>";
        assert!(xmp_to_recipe(silent).is_noop(), "no de-fringe keys is no edit either");
        // A REAL de-fringe, by contrast, is one.
        let edited = real.replace("crs:DefringePurpleAmount=\"0\"", "crs:DefringePurpleAmount=\"3\"");
        assert!(!xmp_to_recipe(&edited).is_noop(), "an actual de-fringe IS an edit");
    }

    /// R25 B3, the complement arm: the detail block, the CA pair and
    /// de-fringe have all LEFT `unmodelled_global_crs` — with no edit to that
    /// function, because its universe is the complement of `owned_attr_keys`.
    ///
    /// This is the same "the list shrinks by itself" property B2 proved for
    /// Texture, and it is the reason the writer and the reader had to land in
    /// one commit: a key we read but never write would still be foreign
    /// property, named here and duplicated by the merge.
    #[test]
    fn unmodelled_list_no_longer_names_the_detail_block() {
        let lr = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
                  <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
                  <rdf:Description rdf:about=\"\"\n    \
                  xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n    \
                  crs:SharpenRadius=\"+1.0\" crs:SharpenDetail=\"25\" \
                  crs:SharpenEdgeMasking=\"0\" crs:LuminanceNoiseReductionDetail=\"50\" \
                  crs:LuminanceNoiseReductionContrast=\"0\" crs:ColorNoiseReduction=\"25\" \
                  crs:ColorNoiseReductionDetail=\"50\" crs:ColorNoiseReductionSmoothness=\"50\" \
                  crs:AutoLateralCA=\"1\" crs:ChromaticAberrationR=\"0\" \
                  crs:ChromaticAberrationB=\"0\" crs:DefringePurpleAmount=\"3\" \
                  crs:DefringePurpleHueLo=\"39\" crs:DefringePurpleHueHi=\"79\" \
                  crs:DefringeGreenAmount=\"0\" crs:DefringeGreenHueLo=\"40\" \
                  crs:DefringeGreenHueHi=\"60\" crs:PointColor=\"0\" \
                  crs:HasSettings=\"True\">\n  \
                  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n";
        let found = unmodelled_global_crs(lr);
        for gone in [
            "SharpenRadius",
            "SharpenDetail",
            "SharpenEdgeMasking",
            "LuminanceNoiseReductionDetail",
            "LuminanceNoiseReductionContrast",
            "ColorNoiseReduction",
            "ColorNoiseReductionDetail",
            "ColorNoiseReductionSmoothness",
            "AutoLateralCA",
            "ChromaticAberrationR",
            "ChromaticAberrationB",
            "DefringePurpleAmount",
            "DefringePurpleHueLo",
            "DefringePurpleHueHi",
            "DefringeGreenAmount",
            "DefringeGreenHueLo",
            "DefringeGreenHueHi",
        ] {
            assert!(
                !found.contains(&gone.to_string()),
                "{gone} is modelled since R25 B3 and must have left the list: {found:?}"
            );
        }
        // The premise: the scan really did run and still names what we do
        // NOT model, or the seventeen assertions above prove nothing.
        assert!(
            found.contains(&"PointColor".to_string()),
            "an unmodelled global must still be named: {found:?}"
        );
        // …and the values arrived rather than merely stopping being foreign.
        let r = xmp_to_recipe(lr);
        assert_eq!((r.sharpen_radius, r.color_nr, r.defringe_purple), (1.0, 25.0, 3.0));
        assert!(r.auto_lateral_ca);
    }

    // ───────────────────── R25 B4: the pass-through blocks ──────────────────

    /// A Lightroom Transform / Calibration block, verbatim, in this batch's
    /// own spellings — synthetic, but every value below is copied CHARACTER
    /// FOR CHARACTER out of the seven reference sidecars (a bare `0`, a
    /// decimal `0.00`, a signed `+0.9`, a plain `100`, a negative `-35` and a
    /// profile NAME with a space in it: six different spellings of things a
    /// number formatter would flatten into three).
    fn lr_transform_doc() -> String {
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
         xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
         <rdf:Description rdf:about=\"\" \
         xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
         crs:Exposure2012=\"+1.00\" \
         crs:PerspectiveUpright=\"0\" crs:PerspectiveVertical=\"-35\" \
         crs:PerspectiveHorizontal=\"0\" crs:PerspectiveRotate=\"+0.9\" \
         crs:PerspectiveScale=\"100\" crs:PerspectiveAspect=\"0\" \
         crs:PerspectiveX=\"0.00\" crs:PerspectiveY=\"0.00\" \
         crs:CameraProfile=\"Adobe Standard\" \
         crs:CameraProfileDigest=\"2D1D4700365C3E2831EEAE0D1A8F9CDF\" \
         crs:HasSettings=\"True\"/></rdf:RDF></x:xmpmeta>"
            .to_string()
    }

    /// **The first real payload `Tier::PassThrough` has ever carried**
    /// (`ARCHITECTURE.md` recorded the tier as "none yet" until this batch).
    /// The registry's three-sided law only checks that a ROW exists; this
    /// checks that the value survives the round trip as ITSELF.
    #[test]
    fn passthrough_round_trips_verbatim() {
        let r = xmp_to_recipe(&lr_transform_doc());
        assert_eq!(r.passthrough.len(), 9, "eight Perspective keys + the profile: {:?}", r.passthrough);
        // VERBATIM means the spelling too. Every one of these would have been
        // destroyed by a number round trip: `+0.9` loses its sign marker,
        // `0.00` loses two decimals, `Adobe Standard` is not a number at all.
        for (key, want) in [
            ("PerspectiveUpright", "0"),
            ("PerspectiveVertical", "-35"),
            ("PerspectiveRotate", "+0.9"),
            ("PerspectiveScale", "100"),
            ("PerspectiveX", "0.00"),
            ("CameraProfile", "Adobe Standard"),
        ] {
            assert_eq!(r.passthrough.get(key).map(String::as_str), Some(want), "{key}");
        }
        // The keys OUTSIDE the named sixteen are untouched by all of this:
        // the digest stays foreign, preserved by the merge and named by the
        // import disclosure. "Named set, not everything unknown."
        assert!(!r.passthrough.contains_key("CameraProfileDigest"));
        assert!(
            unmodelled_global_crs(&lr_transform_doc()).contains(&"CameraProfileDigest".to_string())
        );

        // Out and back, through OUR writer.
        let ours = recipe_to_xmp(&r);
        assert!(ours.contains(r#"crs:PerspectiveRotate="+0.9""#), "{ours}");
        assert!(ours.contains(r#"crs:CameraProfile="Adobe Standard""#), "{ours}");
        assert_eq!(xmp_to_recipe(&ours).passthrough, r.passthrough, "a full verbatim round trip");

        // Written in PASSTHROUGH_CRS order, not the BTreeMap's alphabetical
        // one: Adobe groups Transform before Calibration, and a diff against
        // Lightroom's own file has to be readable.
        let at = |k: &str| ours.find(&format!("crs:{k}=")).unwrap_or_else(|| panic!("{k} missing"));
        assert!(at("PerspectiveUpright") < at("PerspectiveY"), "the Transform block keeps its order");
        assert!(at("PerspectiveY") < at("CameraProfile"), "Transform before Calibration");

        // XML transport still applies — escaping is not interpretation, and a
        // profile name really can carry an ampersand.
        let odd = EditRecipe {
            passthrough: [("CameraProfile".to_string(), "Sky & Sea <v2>".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let doc = recipe_to_xmp(&odd);
        assert!(doc.contains("Sky &amp; Sea &lt;v2&gt;"), "escaped on the way out: {doc}");
        assert_eq!(
            xmp_to_recipe(&doc).passthrough.get("CameraProfile").map(String::as_str),
            Some("Sky & Sea <v2>"),
            "…and unescaped back to the very same string"
        );
    }

    /// The empty map is the state of every recipe written before this batch,
    /// and it must change NOTHING: no invented Transform block in the sidecar,
    /// and an older `recipe.json` with no such key still reads.
    #[test]
    fn an_empty_passthrough_map_leaves_the_sidecar_bytes_unchanged() {
        let doc = recipe_to_xmp(&EditRecipe::default());
        for key in PASSTHROUGH_CRS {
            assert!(
                !doc.contains(&format!("crs:{key}=")),
                "{key}: a recipe that never saw a Transform block must not assert one: {doc}"
            );
        }
        // Forward/backward compatibility of the FIELD, which is the other
        // half of "unchanged": a recipe.json from v0.30 has no `passthrough`
        // key at all and must still load, as an empty map.
        let legacy = r#"{"version":2,"exposure_ev":0.25}"#;
        let r: EditRecipe = serde_json::from_str(legacy).expect("a legacy recipe still parses");
        assert!(r.passthrough.is_empty());
        assert_eq!(r.exposure_ev, 0.25);
    }

    /// The merge's own trap, and the reason [`merge_strip_keys`] exists: a
    /// recipe that never SAW a Transform block must not delete one.
    ///
    /// Owning a key normally licenses the strip — that is how a cleared
    /// vignette disappears. Pass-through has no cleared state: nothing in the
    /// app can empty the map, so "absent" only ever means "this recipe came
    /// from somewhere that never read the document" (a v0.30 recipe.json, a
    /// paste from another photo, a fresh Analyze). Stripping on that would
    /// delete the photographer's Upright correction and camera profile from
    /// the file beside their RAW on an ordinary Ctrl+S.
    #[test]
    fn a_recipe_that_never_saw_a_transform_block_does_not_delete_one() {
        let lr = lr_transform_doc();
        // (a) The dangerous case: empty map, real Transform block in the base.
        let blind = EditRecipe { exposure_ev: 0.25, ..Default::default() };
        let merged = merged_doc(&lr, &blind).expect("mergeable");
        for want in [
            r#"crs:PerspectiveVertical="-35""#,
            r#"crs:PerspectiveRotate="+0.9""#,
            r#"crs:CameraProfile="Adobe Standard""#,
        ] {
            assert!(merged.contains(want), "the base's own {want} must survive: {merged}");
        }
        assert_eq!(merged.matches("crs:CameraProfile=").count(), 1, "and exactly once");
        assert!(merged.contains(r#"crs:Exposure2012="0.25""#), "…while ours still publish");

        // (b) The ordinary case: the recipe DID read the block, so ours are
        // stripped and rewritten — one copy, never two.
        let seen = EditRecipe { exposure_ev: 0.25, ..xmp_to_recipe(&lr) };
        let merged = merged_doc(&lr, &seen).expect("mergeable");
        assert_eq!(merged.matches("crs:CameraProfile=").count(), 1, "stripped, then rewritten");
        assert_eq!(merged.matches("crs:PerspectiveScale=").count(), 1);
        assert!(merged.contains(r#"crs:CameraProfile="Adobe Standard""#));
        // (c) …and a CHANGED value replaces rather than duplicates.
        let mut edited = seen.clone();
        edited.passthrough.insert("CameraProfile".to_string(), "Adobe Landscape".to_string());
        let merged = merged_doc(&lr, &edited).expect("mergeable");
        assert_eq!(merged.matches("crs:CameraProfile=").count(), 1);
        assert!(merged.contains(r#"crs:CameraProfile="Adobe Landscape""#), "{merged}");
        assert!(!merged.contains("Adobe Standard\""), "the old value is gone: {merged}");
    }

    /// The regenerate path — the one that "carries none of the base's
    /// properties". It carries these: the sixteen live in the RECIPE now, so
    /// a document rebuilt from scratch still states them. (`pipeline`'s
    /// regeneration note names the creative `Look` instead of the camera
    /// profile for exactly this reason.)
    #[test]
    fn passthrough_survives_a_regenerate() {
        let r = xmp_to_recipe(&lr_transform_doc());
        let fresh = recipe_to_xmp(&r); // no base document at all
        assert!(fresh.contains(r#"crs:CameraProfile="Adobe Standard""#), "{fresh}");
        assert!(fresh.contains(r#"crs:PerspectiveVertical="-35""#), "{fresh}");
        assert_eq!(xmp_to_recipe(&fresh).passthrough, r.passthrough);
    }

    /// Both spellings, one scanner. `crs_str` already reads the
    /// property-ELEMENT form, so the reader needed no second scan arm (R24
    /// round-end MED-1: a third arm is how the two forms drift apart) — and
    /// the SCOPE rule matters more here than anywhere: a creative Look nests
    /// its own baked `crs:CameraProfile`, and a flat scan would import the
    /// PROFILE's name as the photographer's choice.
    #[test]
    fn passthrough_reads_the_element_form_and_never_the_nested_look() {
        let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                   <rdf:Description rdf:about=\"\" \
                   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\">\
                   <crs:CameraProfile>Adobe Standard</crs:CameraProfile>\
                   <crs:PerspectiveScale>110</crs:PerspectiveScale>\
                   <crs:Look><rdf:Description><crs:Parameters><rdf:Description>\
                   <crs:CameraProfile>Camera Landscape</crs:CameraProfile>\
                   <crs:PerspectiveScale>999</crs:PerspectiveScale>\
                   </rdf:Description></crs:Parameters></rdf:Description></crs:Look>\
                   </rdf:Description></rdf:RDF></x:xmpmeta>";
        let r = xmp_to_recipe(doc);
        assert_eq!(
            r.passthrough.get("CameraProfile").map(String::as_str),
            Some("Adobe Standard"),
            "the Description's OWN profile, never the Look's baked one"
        );
        assert_eq!(r.passthrough.get("PerspectiveScale").map(String::as_str), Some("110"));
        // A document with no such block reports none — absence stays absence.
        assert!(xmp_to_recipe("<rdf:Description \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             crs:Exposure2012=\"0.00\"/>")
            .passthrough
            .is_empty());
    }

    /// A pass-through value is never "unparsable", because it is never parsed.
    ///
    /// The trap this closes: the sixteen keys joined `owned_attr_keys`, and
    /// that list IS `unparsable_crs_numbers`' universe — with a ±100 fallback
    /// band for any key the registry states no range for. Without the
    /// exemption, `crs:CameraProfile="Adobe Standard"` (not a number) and
    /// `crs:PerspectiveX="-140"` (an ordinary Upright result, outside ±100)
    /// would both have been reported as values that "import as a silent
    /// neutral" — about the one block in the recipe that has no neutral and
    /// is never replaced.
    #[test]
    fn a_passthrough_value_is_never_called_unparsable() {
        let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                   xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                   <rdf:Description rdf:about=\"\" \
                   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                   crs:CameraProfile=\"Adobe Standard\" crs:PerspectiveX=\"-140\" \
                   crs:PerspectiveScale=\"117.5\" crs:Contrast2012=\"+22\" \
                   crs:HasSettings=\"True\"/></rdf:RDF></x:xmpmeta>";
        assert!(unparsable_crs_numbers(doc).is_empty(), "{:?}", unparsable_crs_numbers(doc));
        // The premise, so the emptiness above is not emptiness for another
        // reason: the same scan still names an OWNED number that is off its
        // band, in the very same document.
        let bad = doc.replace(r#"crs:Contrast2012="+22""#, r#"crs:Contrast2012="+220""#);
        assert_eq!(unparsable_crs_numbers(&bad), vec!["Contrast2012"]);
        // …and the values still arrive, out of band and all.
        let r = xmp_to_recipe(doc);
        assert_eq!(r.passthrough.get("PerspectiveX").map(String::as_str), Some("-140"));
        assert_eq!(r.passthrough.get("PerspectiveScale").map(String::as_str), Some("117.5"));
    }

    /// **The disclosure that had no other half** (R25 B4, work order 4.7):
    /// `global_export_losses` has named "we render it, the sidecar cannot
    /// carry it" since R24-5 M0; this names the opposite corner — Lightroom
    /// renders it, this engine does not — which R25's B2/B3 batches filled
    /// with twenty-four members and B4 with the twenty-fifth.
    #[test]
    fn the_render_gaps_name_what_lightroom_renders_and_this_engine_does_not() {
        use crate::advisor::catalogue::{Tier, RECIPE_CONTROLS};
        // NEUTRAL SAYS NOTHING — and against the DEFAULT, not against zero.
        // The de-fringe block's neutral is Adobe's own 30/70/40/60 (R25 B3),
        // so a zero comparison would report a de-fringe gap on every single
        // photo ever opened, and a disclosure that fires always is read never.
        assert!(
            global_render_gaps(&EditRecipe::default()).is_empty(),
            "a default recipe carries no gap: {:?}",
            global_render_gaps(&EditRecipe::default())
        );
        let untouched = xmp_to_recipe(
            "<rdf:Description xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             crs:DefringePurpleAmount=\"0\" crs:DefringePurpleHueLo=\"30\" \
             crs:DefringePurpleHueHi=\"70\" crs:DefringeGreenAmount=\"0\" \
             crs:DefringeGreenHueLo=\"40\" crs:DefringeGreenHueHi=\"60\"/>",
        );
        assert!(
            global_render_gaps(&untouched).is_empty(),
            "a real sidecar's resting de-fringe block is not a gap: {:?}",
            global_render_gaps(&untouched)
        );
        // One carried value, named.
        let grainy = EditRecipe { grain: 30.0, ..Default::default() };
        assert_eq!(global_render_gaps(&grainy), vec!["grain"]);
        // A RENDERED control is not a gap, however far it is from neutral.
        let bright = EditRecipe { exposure_ev: 2.0, texture: 40.0, ..Default::default() };
        assert!(global_render_gaps(&bright).is_empty(), "{:?}", global_render_gaps(&bright));
        // The B4 row is NOT here, and that is the tier's own definition
        // rather than an omission: we never interpret a pass-through value,
        // so we cannot tell Lightroom's resting `PerspectiveUpright="0"` — on
        // six of the seven reference sidecars, changing nothing anywhere —
        // from a real Upright correction. This sentence would then appear on
        // every Lightroom photo ever opened and drown the members that ARE
        // actionable. Its disclosure is the develop panel's own read-only
        // section, which shows the values instead of guessing at them.
        let upright = EditRecipe {
            passthrough: [("PerspectiveVertical".to_string(), "-35".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        assert!(
            global_render_gaps(&upright).is_empty(),
            "PassThrough has no knowable neutral, so it discloses through its section"
        );
        assert!(
            RECIPE_CONTROLS
                .iter()
                .any(|c| c.name == "passthrough" && c.tier == Some(Tier::PassThrough)),
            "premise: the row exists and renders nothing — the exclusion above is a choice"
        );
        // DERIVATION: the list is exactly the CarriedOnly rows. Moving one to
        // `Rendered` takes it out of the disclosure with no edit here — which
        // is the property this test is really pinning.
        let mut every = serde_json::to_value(EditRecipe::default()).expect("serialises");
        let expect: Vec<&str> = RECIPE_CONTROLS
            .iter()
            .filter(|c| c.tier == Some(Tier::CarriedOnly))
            .map(|c| c.name)
            .collect();
        assert!(
            expect.len() >= 24,
            "premise: B2+B3 put twenty-four CarriedOnly rows here: {expect:?}"
        );
        for name in &expect {
            let shape = RECIPE_CONTROLS
                .iter()
                .find(|c| c.name == *name)
                .map(|c| c.shape)
                .expect("a registry row");
            every[*name] = match shape {
                crate::advisor::catalogue::Shape::Bool => serde_json::json!(true),
                _ => serde_json::json!(7.0),
            };
        }
        let all: EditRecipe = serde_json::from_value(every).expect("in range");
        assert_eq!(global_render_gaps(&all), expect, "every non-rendering row, and only those");
        // The two disclosures are DISJOINT halves of one story, never the
        // same claim twice: a tier renders or it does not.
        for g in global_render_gaps(&all) {
            assert!(
                !global_export_losses(&all).contains(&g),
                "{g} cannot be both a render gap and an export loss"
            );
        }
        // And the CarriedOnly whitelist is fully covered by it — a slider on
        // that list that never reached this sentence would be the silent fork
        // the SF4-C policy needs this disclosure to close.
        for (n, _) in crate::advisor::catalogue::CARRIED_ONLY_GLOBAL {
            assert!(
                global_render_gaps(&all).contains(n),
                "{n} is CarriedOnly but never reaches the render-gap disclosure"
            );
        }
        let _ = Tier::PassThrough; // the tier this batch populated
    }

    /// R25 B2 (policy SF4-C): the nine carried effects reach the sidecar and
    /// the engine renders nothing from them.
    ///
    /// The write rule is PER-KEY — every one of the nine is neutral at zero,
    /// so "write what is non-neutral" needs no group gate, and the three whose
    /// ACR default is not zero (Midpoint/Feather 50, Style 1) reach Lightroom
    /// by ABSENCE rather than by a value we made up. Verified against the
    /// user's own library, where Lightroom writes the companion keys only
    /// alongside a non-zero amount.
    #[test]
    fn carried_effects_round_trip_and_render_nothing() {
        // The shape a real Lightroom sidecar takes (DSC09568 / _DSC9082):
        // an amount plus its five companions, grain likewise.
        let r = EditRecipe {
            post_crop_vignette: -17.0,
            post_crop_vignette_mid: 50.0,
            post_crop_vignette_feather: 50.0,
            post_crop_vignette_round: 0.0,
            post_crop_vignette_style: 1.0,
            post_crop_vignette_hl: 0.0,
            grain: 30.0,
            grain_size: 25.0,
            grain_rough: 50.0,
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        for want in [
            r#"crs:PostCropVignetteAmount="-17""#,
            r#"crs:PostCropVignetteMidpoint="50""#,
            r#"crs:PostCropVignetteFeather="50""#,
            r#"crs:PostCropVignetteStyle="1""#,
            r#"crs:GrainAmount="30""#,
            r#"crs:GrainSize="25""#,
            r#"crs:GrainFrequency="50""#,
        ] {
            assert!(xmp.contains(want), "{want} missing from: {xmp}");
        }
        // The two that are AT their neutral stay out — absence is how
        // Lightroom is told "keep your own default".
        assert!(!xmp.contains("PostCropVignetteRoundness"), "a zero roundness is not written");
        assert!(!xmp.contains("PostCropVignetteHighlightContrast"));
        // Every one comes back as itself — read side and write side in one
        // batch, exactly as for Texture above.
        let back = xmp_to_recipe(&xmp);
        for (name, live, want) in [
            ("post_crop_vignette", back.post_crop_vignette, r.post_crop_vignette),
            ("post_crop_vignette_mid", back.post_crop_vignette_mid, r.post_crop_vignette_mid),
            (
                "post_crop_vignette_feather",
                back.post_crop_vignette_feather,
                r.post_crop_vignette_feather,
            ),
            ("post_crop_vignette_round", back.post_crop_vignette_round, r.post_crop_vignette_round),
            ("post_crop_vignette_style", back.post_crop_vignette_style, r.post_crop_vignette_style),
            ("post_crop_vignette_hl", back.post_crop_vignette_hl, r.post_crop_vignette_hl),
            ("grain", back.grain, r.grain),
            ("grain_size", back.grain_size, r.grain_size),
            ("grain_rough", back.grain_rough, r.grain_rough),
        ] {
            assert_eq!(live, want, "{name} did not survive the round trip");
        }
        // A neutral recipe emits none of the nine (byte-compatible with the
        // pre-B2 writer for every recipe that never touched them).
        let neutral = recipe_to_xmp(&EditRecipe::default());
        for k in ["PostCropVignette", "Grain"] {
            assert!(!neutral.contains(k), "{k} must not appear on a neutral recipe: {neutral}");
        }
        // …and the ENGINE ignores all nine: the developed frame is
        // bit-identical to the neutral one. That is the claim
        // `Tier::CarriedOnly` makes, and it is the half a registry row cannot
        // prove about itself.
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(24, 16, |x, y| {
            image::Rgb([(x * 9) as u8, (y * 13) as u8, (x + y) as u8])
        }));
        assert_eq!(
            crate::render::develop_preview(&img, &r).to_rgb8().into_raw(),
            crate::render::develop_preview(&img, &EditRecipe::default()).to_rgb8().into_raw(),
            "a CarriedOnly control that moved a pixel would be mis-classified"
        );
    }

    /// One parametric mask + one raster mask — the fixture behind BOTH halves
    /// of the raster contract: the writer emits no raster correction, and the
    /// reader therefore returns no phantom for one.
    fn mixed_parametric_and_raster() -> EditRecipe {
        EditRecipe {
            masks: vec![
                LocalAdjustment {
                    mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.35, full_x: 0.5, full_y: 0.0 },
                    exposure_ev: -1.0,
                    ..Default::default()
                },
                LocalAdjustment {
                    mask: MaskGeometry::Bitmap { path: "out/subject.png".into() },
                    exposure_ev: 0.6,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn bitmap_masks_are_skipped_by_the_xmp_writer() {
        use crate::recipe::MaskGeometry;
        let mixed = mixed_parametric_and_raster();
        let xmp = recipe_to_xmp(&mixed);
        assert!(xmp.contains("Mask/Gradient"), "the parametric mask must survive");
        assert_eq!(xmp.matches("crs:What=\"Correction\"").count(), 1, "raster correction skipped");
        assert!(!xmp.contains("subject.png"), "no raster path may leak into the sidecar");
        // All-raster: the whole corrections block disappears (no empty shell).
        let all_bitmap = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: "out/sky.png".into() },
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(!recipe_to_xmp(&all_bitmap).contains("MaskGroupBasedCorrections"));
    }

    /// M6a: the export direction had FOUR silent losses (raster masks skipped,
    /// muted masks skipped, extra shapes flattened, radial rotation +
    /// recolour gains dropped) against four import-side disclosures and zero
    /// export-side ones. The writer now names them while it emits, and the
    /// assertion is a SET comparison, not a count: a rule that fires on the
    /// wrong mask, or twice on one mask, is exactly the bug a count hides.
    #[test]
    fn the_writer_names_every_mask_the_sidecar_cannot_carry() {
        use crate::recipe::{MaskCombine, MaskComponent};
        let radial = |angle: f32| MaskGeometry::Radial {
            top: 0.3,
            left: 0.35,
            bottom: 0.7,
            right: 0.65,
            feather: 0.5,
            roundness: 0.0,
            flipped: false,
            angle,
            midpoint: 50.0,
            mask_version: 2,
        };
        let component = MaskComponent {
            geometry: MaskGeometry::Linear { zero_x: 0.1, zero_y: 0.1, full_x: 0.9, full_y: 0.9 },
            mode: MaskCombine::Subtract,
        };
        let r = EditRecipe {
            masks: vec![
                // Raster geometry: the whole correction goes.
                LocalAdjustment {
                    mask: MaskGeometry::Bitmap { path: "out/sky.png".into() },
                    name: "sky".into(),
                    ..Default::default()
                },
                // Muted — and loaded with every degradation as well: the eye
                // is why it is skipped, so it must produce exactly ONE verdict
                // (this arm is what stops the counts from double-billing).
                LocalAdjustment {
                    mask: radial(30.0),
                    components: vec![component.clone()],
                    color_gains: Some([1.4, 1.0, 0.6]),
                    enabled: false,
                    name: "parked".into(),
                    ..Default::default()
                },
                // Emitted, but only as its base shape.
                LocalAdjustment {
                    components: vec![component.clone(), component.clone()],
                    name: "combo".into(),
                    ..Default::default()
                },
                // Emitted as an UNROTATED ellipse, without the recolour.
                LocalAdjustment {
                    mask: radial(-12.0),
                    color_gains: Some([1.2, 0.95, 0.7]),
                    name: "gold".into(),
                    ..Default::default()
                },
                // Nothing lost: an unrotated radial, no components, neutral
                // gains (which are no recolour at all).
                LocalAdjustment {
                    mask: radial(0.0),
                    color_gains: Some([1.0, 1.0, 1.0]),
                    name: "clean".into(),
                    ..Default::default()
                },
                // Unnamed masks are identified by the name the SIDECAR would
                // have used, not by "".
                LocalAdjustment { mask: MaskGeometry::Bitmap { path: "out/x.png".into() }, ..Default::default() },
            ],
            ..Default::default()
        };
        let mut got: Vec<(String, MaskLossReason)> =
            mask_export_losses(&r).into_iter().map(|l| (l.name, l.reason)).collect();
        got.sort();
        let mut want = vec![
            ("sky".to_string(), MaskLossReason::Bitmap),
            ("parked".to_string(), MaskLossReason::Disabled),
            ("combo".to_string(), MaskLossReason::ComponentsFlattened),
            // …and the rotation verdict CARRIES the angle it dropped (R25
            // P5): −12°, not merely "some rotation". The disclosure surfaces
            // read the number off this payload, so a writer that raised the
            // reason with the wrong angle fails HERE, at the source, rather
            // than printing a plausible wrong number in the window.
            ("gold".to_string(), MaskLossReason::Rotation(-12)),
            ("gold".to_string(), MaskLossReason::Recolour),
            ("AutoShade 6".to_string(), MaskLossReason::Bitmap),
        ];
        want.sort();
        assert_eq!(got, want, "the loss set must name mask AND reason exactly once each");
        // The prose channel (CLI stderr / web reply) covers every category and
        // counts them; "combo" flattened TWO components but is ONE loss.
        let line = describe_mask_losses(&mask_export_losses(&r)).expect("losses ⇒ a line");
        for expect in [
            "2 bitmap mask(s) skipped (sky, AutoShade 6)",
            "1 muted mask(s) skipped (parked)",
            "1 extra shape component(s) flattened (combo)",
            "1 radial rotation dropped (gold)",
            "1 recolour gains dropped (gold)",
        ] {
            assert!(line.contains(expect), "the line must state {expect:?}: {line}");
        }
        // …and the emitted document agrees with the claim: four masks, one
        // skipped as raster, one as muted, and no leaked raster path.
        let doc = recipe_to_xmp(&r);
        assert_eq!(doc.matches("crs:What=\"Correction\"").count(), 3, "3 of 6 project");
        assert!(!doc.contains("sky.png"), "no raster path in a sidecar");

        // Nothing lossy ⇒ nothing said, on both channels (a faithful save
        // must not be interrupted).
        let faithful = EditRecipe { masks: vec![r.masks[4].clone()], ..Default::default() };
        assert!(mask_export_losses(&faithful).is_empty(), "an exportable mask loses nothing");
        assert!(describe_mask_losses(&[]).is_none(), "an empty list has nothing to say");
        assert!(mask_export_losses(&EditRecipe::default()).is_empty(), "no masks, no losses");
    }

    /// R25 P0-0.6: both disclosure surfaces ITERATE `MaskLossReason::ALL`
    /// (here and the GUI's `xmp_loss_line`), so the list is the one place a
    /// reason can be forgotten — and the match below is where a new variant
    /// stops the build, with `ALL` the next thing it has to satisfy.
    #[test]
    fn mask_loss_reason_all_covers_every_variant() {
        // Adding a variant makes THIS match non-exhaustive; the arm you write
        // carries the next rank, and the two asserts then fail until `ALL`
        // lists the newcomer in that position.
        fn rank(r: MaskLossReason) -> usize {
            match r {
                MaskLossReason::Bitmap => 0,
                MaskLossReason::Disabled => 1,
                MaskLossReason::ComponentsFlattened => 2,
                MaskLossReason::BrushRendered => 3,
                MaskLossReason::AiMaskRecomputed => 4,
                MaskLossReason::Rotation(_) => 5,
                MaskLossReason::Recolour => 6,
            }
        }
        for (i, r) in MaskLossReason::ALL.into_iter().enumerate() {
            assert_eq!(rank(r), i, "ALL must list every reason once, in rank order");
            assert!(!r.en().trim().is_empty(), "{r:?} has no label for the prose channel");
            assert!(r.same_kind(r), "same_kind must be reflexive for {r:?}");
        }
        // R25 P5: `Rotation` grew a payload, so the grouping key is the
        // DISCRIMINANT — two masks tilted differently are one line in a
        // sentence and two values under `==`, and `ALL`'s placeholder `0`
        // equals neither of them. Same property the import twin relies on.
        assert!(
            MaskLossReason::Rotation(37).same_kind(MaskLossReason::Rotation(-12)),
            "two tilted masks are one line"
        );
        assert!(
            !MaskLossReason::Rotation(0).same_kind(MaskLossReason::Recolour),
            "different variants are different lines"
        );
        // Every reason the WRITER can raise reaches the prose. The mutation
        // this catches: a sixth reason raised by `masks_xml` and left out of
        // `ALL` would be silently invisible in the sentence.
        let losses: Vec<MaskLoss> = MaskLossReason::ALL
            .into_iter()
            .map(|reason| MaskLoss { name: format!("m{}", rank(reason)), reason })
            .collect();
        let line = describe_mask_losses(&losses).expect("five losses ⇒ a line");
        for r in MaskLossReason::ALL {
            assert!(line.contains(r.en()), "{r:?} never reaches the prose: {line}");
            assert!(line.contains(&format!("m{}", rank(r))), "{r:?} loses its mask name: {line}");
        }
    }

    // ── R25 P1: the import unlock ────────────────────────────────────────
    //
    // FIXTURE POLICY. This is a public repository and the reference sidecars
    // are the user's own photographs, so no line of them is copied in. Every
    // inline fixture below is SYNTHESISED: the attribute set, the value
    // shapes, the attribute order and the element nesting are reproduced
    // exactly as `DSC08761.xmp` / `DSC09642.xmp` write them, and every
    // personal identifier (`crs:CorrectionName`, `crs:MaskName`, the sync
    // GUIDs) carries a neutral test value instead. The real files are
    // exercised by `real_lightroom_sidecars_import_their_parametric_masks`,
    // which reads them from a path given at RUN time and skips when it is
    // not set.

    /// One `crs:What="Correction"` `<rdf:li>`, structurally verbatim: all 25
    /// `crs:Local*` attributes in Lightroom's own order, the sliders on its
    /// own 0..1 scale, `crs:CorrectionMasks` last. `locals` is spliced in
    /// just before `LocalCurveRefineSaturation` (where an unknown key would
    /// really sit); tests that need a DIFFERENT value for an existing key
    /// rewrite it on the returned string, so the fixture can never carry the
    /// same attribute twice.
    fn lr_correction(name: &str, locals: &str, components: &str) -> String {
        lr_correction_with_curves(name, locals, "", components)
    }

    /// The same fixture with the correction's four LOCAL point curves spliced
    /// in (R25 P6) — child ELEMENTS between the attribute block's closing `>`
    /// and `<crs:CorrectionMasks>`, which is where the reference sidecars put
    /// them. `lr_curve` builds one.
    fn lr_correction_with_curves(
        name: &str,
        locals: &str,
        curves: &str,
        components: &str,
    ) -> String {
        format!(
            "     <rdf:li>\n\
             \x20     <rdf:Description\n\
             \x20      crs:What=\"Correction\"\n\
             \x20      crs:CorrectionAmount=\"1\"\n\
             \x20      crs:CorrectionActive=\"true\"\n\
             \x20      crs:CorrectionName=\"{name}\"\n\
             \x20      crs:CorrectionSyncID=\"0000000000000000000000000000000A\"\n\
             \x20      crs:LocalExposure=\"0\"\n\
             \x20      crs:LocalHue=\"0\"\n\
             \x20      crs:LocalSaturation=\"0\"\n\
             \x20      crs:LocalContrast=\"0\"\n\
             \x20      crs:LocalClarity=\"0\"\n\
             \x20      crs:LocalSharpness=\"0\"\n\
             \x20      crs:LocalBrightness=\"0\"\n\
             \x20      crs:LocalToningHue=\"0\"\n\
             \x20      crs:LocalToningSaturation=\"0\"\n\
             \x20      crs:LocalExposure2012=\"0.1\"\n\
             \x20      crs:LocalContrast2012=\"0.43\"\n\
             \x20      crs:LocalHighlights2012=\"0\"\n\
             \x20      crs:LocalShadows2012=\"0\"\n\
             \x20      crs:LocalWhites2012=\"0\"\n\
             \x20      crs:LocalBlacks2012=\"0\"\n\
             \x20      crs:LocalClarity2012=\"0\"\n\
             \x20      crs:LocalDehaze=\"0\"\n\
             \x20      crs:LocalLuminanceNoise=\"0\"\n\
             \x20      crs:LocalMoire=\"0\"\n\
             \x20      crs:LocalDefringe=\"0\"\n\
             \x20      crs:LocalTemperature=\"0.24\"\n\
             \x20      crs:LocalTint=\"0.44\"\n\
             \x20      crs:LocalTexture=\"0\"\n\
             \x20      crs:LocalGrain=\"0\"\n\
             {locals}\
             \x20      crs:LocalCurveRefineSaturation=\"100\">\n\
             {curves}\
             \x20     <crs:CorrectionMasks>\n\
             \x20      <rdf:Seq>\n\
             {components}\
             \x20      </rdf:Seq>\n\
             \x20     </crs:CorrectionMasks>\n\
             \x20     </rdf:Description>\n\
             \x20    </rdf:li>\n"
        )
    }

    /// One local point curve as Lightroom writes it inside a Correction: a
    /// BARE key (`MainCurve`, not `ToneCurvePV2012`) and points spelled `x,y`
    /// with NO space after the comma — structurally verbatim from
    /// `DSC09642.xmp`'s `<crs:RedCurve>` block, with test values.
    fn lr_curve(tag: &str, points: &[(u8, u8)]) -> String {
        let pts: String = points
            .iter()
            .map(|(x, y)| format!("       <rdf:li>{x},{y}</rdf:li>\n"))
            .collect();
        format!(
            "      <crs:{tag}>\n       <rdf:Seq>\n{pts}       </rdf:Seq>\n      </crs:{tag}>\n"
        )
    }

    /// A radial component the way Lightroom writes one — `crs:Angle` and
    /// `crs:MaskBlendMode` included, because it writes them on EVERY radial.
    fn lr_radial(angle: &str, blend: &str) -> String {
        format!(
            "        <rdf:li\n\
             \x20        crs:What=\"Mask/CircularGradient\"\n\
             \x20        crs:MaskActive=\"true\"\n\
             \x20        crs:MaskName=\"Radial Gradient 1\"\n\
             \x20        crs:MaskBlendMode=\"{blend}\"\n\
             \x20        crs:MaskInverted=\"false\"\n\
             \x20        crs:MaskSyncID=\"0000000000000000000000000000000B\"\n\
             \x20        crs:MaskValue=\"1\"\n\
             \x20        crs:Top=\"0.114928\"\n\
             \x20        crs:Left=\"0.590368\"\n\
             \x20        crs:Bottom=\"0.802847\"\n\
             \x20        crs:Right=\"0.921381\"\n\
             \x20        crs:Angle=\"{angle}\"\n\
             \x20        crs:Midpoint=\"50\"\n\
             \x20        crs:Roundness=\"0\"\n\
             \x20        crs:Feather=\"100\"\n\
             \x20        crs:Flipped=\"true\"\n\
             \x20        crs:Version=\"2\"/>\n"
        )
    }

    /// A linear gradient component, same provenance.
    fn lr_gradient(blend: &str) -> String {
        format!(
            "        <rdf:li\n\
             \x20        crs:What=\"Mask/Gradient\"\n\
             \x20        crs:MaskActive=\"true\"\n\
             \x20        crs:MaskName=\"Linear Gradient 1\"\n\
             \x20        crs:MaskBlendMode=\"{blend}\"\n\
             \x20        crs:MaskInverted=\"false\"\n\
             \x20        crs:MaskSyncID=\"0000000000000000000000000000000C\"\n\
             \x20        crs:MaskValue=\"1\"\n\
             \x20        crs:ZeroX=\"0.5\"\n\
             \x20        crs:ZeroY=\"0.8\"\n\
             \x20        crs:FullX=\"0.5\"\n\
             \x20        crs:FullY=\"0.2\"/>\n"
        )
    }

    /// The surrounding document: a Lightroom catalog export, NOT one of ours
    /// (`x:xmptk` is Adobe's), which is the whole point — every gate this
    /// batch reopened keyed on our own provenance.
    fn lr_doc(corrections: &str) -> String {
        format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 5.6-c145\">\n\
             \x20<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             \x20 <rdf:Description rdf:about=\"\"\n\
             \x20   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
             \x20   crs:Version=\"15.5.1\"\n\
             \x20   crs:ProcessVersion=\"15.4\"\n\
             \x20   crs:Exposure2012=\"+0.35\"\n\
             \x20   crs:HasSettings=\"True\">\n\
             \x20  <crs:MaskGroupBasedCorrections>\n\
             \x20   <rdf:Seq>\n\
             {corrections}\
             \x20   </rdf:Seq>\n\
             \x20  </crs:MaskGroupBasedCorrections>\n\
             \x20 </rdf:Description>\n\
             \x20</rdf:RDF>\n\
             </x:xmpmeta>\n"
        )
    }

    /// §0 OF THE ROUND. Lightroom writes `crs:Angle` on every radial — `"0"`
    /// when the shape was never rotated — and the import refused the WHOLE
    /// correction on its mere presence. Every radial mask in the user's
    /// catalog therefore arrived as nothing, and the only thing said about it
    /// was an integer count.
    ///
    /// MUTATION THIS CATCHES: put `tag.crs_str("Angle").is_some()` back into
    /// the refusal and this test goes to zero masks — which is exactly the
    /// state the round opened in.
    #[test]
    fn a_lightroom_radial_with_angle_imports() {
        let doc = lr_doc(&lr_correction("Radial 1", "", &lr_radial("37.412506", "0")));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "a rotated Lightroom radial must import: {:?}", r.masks);
        // The angle itself is NOT mapped here — not because the sign or pivot
        // are unknown (v0.32.0 measured both), but because `lr_doc` declares
        // no frame, which is the one case left. The mask arrives as its
        // axis-aligned ellipse, and the note says so.
        let MaskGeometry::Radial { angle, top, feather, flipped, .. } = r.masks[0].mask else {
            panic!("expected a radial, got {:?}", r.masks[0].mask);
        };
        assert_eq!(angle, 0.0, "crs:Angle is disclosed, not guessed at");
        // v0.32.0: `lr_doc` declares no `tiff:ImageWidth/ImageLength`, so the
        // pixel→normalised fold has no aspect and the tilt is disclosed rather
        // than applied (the assert above, and the `Rotation` note below). The
        // FRAME AFFINE is applied to every radial — and since the 2026-08-19
        // ruling set `LR_MASK_FRAME_SCALE = 1.0` (Batch-10: the sidecar's
        // geometry lives in the PLAIN frame; the old 1.032 was one frame's
        // lens-profile warp), the affine is the identity and the corner is
        // the STORED number. Hand-checked: `cy = 0.4588875`,
        // `ry = 0.3439595`, top = cy − ry = 0.1149280.
        assert!(
            (top as f64 - 0.1149280).abs() < 1e-7,
            "the geometry is the file's, in the plain frame: {top}"
        );
        assert_eq!(feather, 1.0, "crs:Feather=100 is Lightroom's 0..100 scale");
        // R25 P9: `crs:Flipped="true"` beside `crs:MaskInverted="false"` is
        // Lightroom's NOT-inverted spelling — one bit written twice — so the
        // mask must arrive with neither flag set. It used to arrive flipped,
        // which inverted it.
        assert!(!flipped, "crs:Flipped is not a second inversion flag");
        assert!(!r.masks[0].inverted, "and MaskInverted=false is the file's actual verdict");
        assert_eq!(r.masks[0].exposure_ev, 0.4, "0.1 × 4 stops");
        assert_eq!(unsupported_corrections(&doc), 0, "nothing was refused");
        let losses = import_losses(&doc);
        assert_eq!(
            losses,
            vec![MaskImportLoss {
                name: "Radial 1".into(),
                // R25 P5: the verdict carries the sidecar's own angle, rounded
                // to whole degrees for the sentence that prints it. 37.412506
                // → 37; the recipe keeps nothing of it at all (the ellipse
                // imports axis-aligned), which is exactly why the disclosure
                // has to be able to say how much was set aside.
                reason: MaskImportReason::Rotation(37)
            }],
            "the rotation is named, with its angle, and it is the ONLY loss"
        );
    }

    // ── R25 P5: the geometry write-back (B5-B1) ──────────────────────────
    //
    // Same fixture policy as the block above — `lr_radial` reproduces
    // Lightroom's own attribute set, order and value shapes for a radial
    // component, with neutral identifiers. The two numbers that had to be
    // REAL to be worth pinning (the out-of-frame corners) are the measured
    // ones from the reference library.

    /// `crs:Midpoint` and `crs:Version` sit on EVERY Lightroom radial, and
    /// until this batch on neither side of this engine: not read, therefore
    /// not kept, therefore deleted from the photographer's own sidecar the
    /// first time AutoShade rewrote it. Both directions in one test, because
    /// a read without a write is the worse half — it looks like it works.
    ///
    /// The values are deliberately NON-default (37 / 3): 50 / 2 are what a
    /// reader that ignores both attributes also produces.
    ///
    /// MUTATION THIS CATCHES: default either field on the way in, or drop
    /// either attribute on the way out, and 37 / 3 vanish.
    #[test]
    fn radial_midpoint_and_version_round_trip() {
        let comp = lr_radial("0", "0")
            .replace("crs:Midpoint=\"50\"", "crs:Midpoint=\"37\"")
            .replace("crs:Version=\"2\"", "crs:Version=\"3\"");
        let doc = lr_doc(&lr_correction("Radial 1", "", &comp));
        let r = xmp_to_recipe(&doc);
        let MaskGeometry::Radial { midpoint, mask_version, .. } = r.masks[0].mask else {
            panic!("expected a radial, got {:?}", r.masks[0].mask);
        };
        assert_eq!(midpoint, 37.0, "crs:Midpoint is read, not defaulted");
        assert_eq!(mask_version, 3, "crs:Version is read, not defaulted");
        assert!(import_losses(&doc).is_empty(), "carrying a value is not losing it");

        // …and out again, adjacent and in the writer's own order.
        let out = recipe_to_xmp(&r);
        assert!(
            out.contains("crs:Midpoint=\"37\" crs:Version=\"3\""),
            "both ride back out of the writer: {out}"
        );
        assert_eq!(
            xmp_to_recipe(&out).masks[0].mask,
            r.masks[0].mask,
            "our own document re-reads to the same geometry"
        );

        // The collision the bounded read exists for: `range_mask_xml` writes
        // `crs:Version="3"` on the RANGE component, which sits after the
        // geometry inside the same correction. An unbounded scan reads that
        // as the ellipse's schema stamp and quietly promotes every ranged
        // radial from 2 to 3.
        let ranged = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Radial {
                    top: 0.3, left: 0.35, bottom: 0.7, right: 0.65,
                    feather: 0.5, roundness: 0.0, flipped: false, angle: 0.0,
                    midpoint: 50.0, mask_version: 2,
                },
                range: Some(RangeMask::Luminance {
                    lo_outer: 0.1,
                    lo: 0.2,
                    hi: 0.8,
                    hi_outer: 0.9,
                }),
                name: "ranged".into(),
                exposure_ev: 0.5,
                ..Default::default()
            }],
            ..Default::default()
        };
        let back = xmp_to_recipe(&recipe_to_xmp(&ranged));
        let MaskGeometry::Radial { mask_version, .. } = back.masks[0].mask else {
            panic!("expected a radial, got {:?}", back.masks[0].mask);
        };
        assert_eq!(mask_version, 2, "the range component's own Version is not the ellipse's");
    }

    /// The reference library holds radial corners on BOTH sides of the frame
    /// — `crs:Bottom="1.802847"` in one file, `crs:Top="-0.153271"` in
    /// another. ACR geometry is a centre+radii carrier, so this is ordinary,
    /// not corruption: the reader already widened to ±8, and the WRITER must
    /// not quietly pull them back to 0..1 on the way out (a clamp there
    /// shortens the falloff of every off-frame gradient the user placed).
    #[test]
    fn out_of_frame_radial_corners_survive_a_round_trip() {
        let comp = lr_radial("0", "0")
            .replace("crs:Top=\"0.114928\"", "crs:Top=\"-0.153271\"")
            .replace("crs:Bottom=\"0.802847\"", "crs:Bottom=\"1.802847\"");
        let doc = lr_doc(&lr_correction("Radial 1", "", &comp));
        let r = xmp_to_recipe(&doc);
        let MaskGeometry::Radial { top, bottom, .. } = r.masks[0].mask else {
            panic!("expected a radial, got {:?}", r.masks[0].mask);
        };
        // The corners arrive VERBATIM (the frame affine is the identity since
        // the 2026-08-19 `LR_MASK_FRAME_SCALE = 1.0` ruling) — the point of
        // the test is that nothing pulls an off-frame corner back to 0..1,
        // and that holds on both boundaries.
        assert!(
            (top as f64 - -0.153271).abs() < 1e-7,
            "a corner above the frame is a real value: {top}"
        );
        assert!(
            (bottom as f64 - 1.802847).abs() < 1e-7,
            "and so is one below it: {bottom}"
        );
        let out = recipe_to_xmp(&r);
        // …and the WRITER hands the file its own numbers back, to the byte.
        assert!(out.contains("crs:Top=\"-0.153271\""), "written raw, not clamped: {out}");
        assert!(out.contains("crs:Bottom=\"1.802847\""), "written raw, not clamped: {out}");
        assert_eq!(xmp_to_recipe(&out).masks[0].mask, r.masks[0].mask, "and it is stable");
    }

    // ── v0.32.0: the radial geometry projection ─────────────────────────────
    //
    // The fixtures below are the SIDECAR NUMBERS of the user's own twelve-frame
    // controlled Lightroom experiment, transcribed from
    // `~/.claude/plans/r25-materials/lr-experiment/probe2/probe2-extract.txt`
    // and `.../lr-experiment/extract.txt`. No photograph and no export is in
    // the repository — the RENDERED measurements those exports produced are
    // quoted in the assertions, and the fixtures that reproduce them from the
    // real files live behind `AUTOSHADE_LR_PROBE_FIXTURES` (see
    // `the_probe_sidecars_decode_to_their_measured_ellipses`).

    /// A radial component with arbitrary corners, otherwise byte-shaped like
    /// [`lr_radial`] (which is transcribed from a real Lightroom write).
    fn lr_radial_at(t: &str, l: &str, b: &str, r: &str, angle: &str) -> String {
        lr_radial(angle, "0")
            .replace("crs:Top=\"0.114928\"", &format!("crs:Top=\"{t}\""))
            .replace("crs:Left=\"0.590368\"", &format!("crs:Left=\"{l}\""))
            .replace("crs:Bottom=\"0.802847\"", &format!("crs:Bottom=\"{b}\""))
            .replace("crs:Right=\"0.921381\"", &format!("crs:Right=\"{r}\""))
    }

    /// Declare the frame on a document that had none. [`lr_doc`] deliberately
    /// does NOT — that keeps every older test on the no-frame arm, which is a
    /// real arm and has to stay covered — so the tests that need the aspect
    /// add it here. Every real Lightroom sidecar carries these two
    /// (`_DSC9600.xmp`: `tiff:ImageWidth="9504" tiff:ImageLength="6336"`,
    /// which is the ARW's own `DefaultCropSize`).
    fn in_frame(doc: &str, w: u32, h: u32) -> String {
        doc.replace(
            "crs:Version=\"15.5.1\"",
            &format!("tiff:ImageWidth=\"{w}\"\n   tiff:ImageLength=\"{h}\"\n   crs:Version=\"15.5.1\""),
        )
    }

    /// The engine's stored radial re-expressed as the PIXEL ellipse it draws:
    /// `(semi-axis along the tilt, the other one, tilt in degrees)`, both axes
    /// in pixels of a `w × h` frame. This is the quantity the probes measured,
    /// so it is the quantity the assertions can quote.
    fn engine_pixel_ellipse(m: &MaskGeometry, w: f64, h: f64) -> (f64, f64, f64) {
        let MaskGeometry::Radial { top, left, bottom, right, angle, .. } = m else {
            panic!("expected a radial, got {m:?}");
        };
        let (rx, ry) = (
            ((*right as f64 - *left as f64) / 2.0).abs() * w,
            ((*bottom as f64 - *top as f64) / 2.0).abs() * h,
        );
        // The engine rotates in the NORMALISED frame, so the pixel ellipse is
        // `diag(w, h)·R(angle)·diag(rx/w, ry/h)` — written out with the `w`/`h`
        // already folded into `rx`/`ry` above.
        let (sin, cos) = (*angle as f64).to_radians().sin_cos();
        let (s1, s2, tu) = svd2([cos * rx, -sin * ry * w / h, sin * rx * h / w, cos * ry]);
        (s1.abs(), s2.abs(), tu.to_degrees())
    }

    /// §0 OF v0.32.0. `crs:Top/Left/Bottom/Right` are the ROTATED CORNERS of
    /// the ellipse's box in pixel space, and reading them as a bounding box —
    /// which every build up to v0.31.2 did — gets the SHAPE wrong by factors,
    /// not percentages.
    ///
    /// Both rotated probes are here, and they are the two that discriminate:
    ///
    /// | probe | file | `crs:Angle` | naive `a/b` | corner `a/b` | measured |
    /// |---|---|---|---|---|---|
    /// | #4 | `_DSC9689` | +24.348422 | **1.652** | **8.332** | 6.3–9.8, scan optimum 7.92 |
    /// | #8 | `_DSC9685` | +29.513785 | **−0.032** (impossible) | **0.524** | 1.84–2.09, scan optimum 1.907 |
    ///
    /// and `#8`'s decoded MAJOR axis is `b`, so its predicted screen tilt is
    /// `θ + 90 = −60.486°` against a measured **−60.5° ± 0.9** (mean over the
    /// τ ≥ 0.8 level sets). `PROBE2-VERDICT.md` §1, §5.
    ///
    /// MUTATION THIS CATCHES: take `abs()` on `X`/`Y` in `lr_to_engine` and
    /// `#8` — whose `Left > Right` — decodes to the naive sliver again; drop
    /// the `sin`/`cos` mixing and both ratios collapse onto the naive column.
    #[test]
    fn a_rotated_lightroom_radial_decodes_to_the_measured_ellipse() {
        let (w, h) = (9504.0, 6336.0);
        // probe #4 — `_DSC9689`, an 8:1 sliver rotated +24.35°.
        let doc = in_frame(
            &lr_doc(&lr_correction(
                "R",
                "",
                &lr_radial_at("-0.045582", "-0.056408", "0.9708", "1.062771", "24.348422"),
            )),
            9504,
            6336,
        );
        let m = &xmp_to_recipe(&doc).masks[0].mask;
        // The RATIO is the model's zero-free-parameter prediction and carries
        // no `k`; the axes themselves are the decode × the frame scale, so they
        // are quoted against `k · a` and `k · b`.
        let k = LR_MASK_FRAME_SCALE;
        let (a, b, tilt) = engine_pixel_ellipse(m, w, h);
        assert!((a - k * 6172.8).abs() < 0.5, "semi-major {a} px, decoded 6172.8 × k");
        assert!((b - k * 740.8).abs() < 0.5, "semi-minor {b} px, decoded 740.8 × k");
        assert!((a / b - 8.332).abs() < 0.01, "axis ratio {} — the naive read is 1.652", a / b);
        assert!((tilt - 24.348422).abs() < 1e-3, "screen tilt {tilt}°, declared +24.348422");

        // probe #8 — `_DSC9685`, TALL (1:2) and rotated past the inversion
        // point, so Lightroom wrote `Left > Right`. The naive read makes
        // `X = −122 px` and the shape a sliver on the wrong axis.
        let doc = in_frame(
            &lr_doc(&lr_correction(
                "R",
                "",
                &lr_radial_at("-0.088191", "0.520214", "1.113528", "0.494492", "29.513785"),
            )),
            9504,
            6336,
        );
        let m = &xmp_to_recipe(&doc).masks[0].mask;
        let (a, b, tilt) = engine_pixel_ellipse(m, w, h);
        // The SVD reports the MAJOR axis first, and here that is the decoded
        // `b = 3373.2` at `θ + 90`.
        assert!((a - k * 3373.2).abs() < 0.5, "semi-major {a} px, decoded 3373.2 × k");
        assert!((b - k * 1769.1).abs() < 0.5, "semi-minor {b} px, decoded 1769.1 × k");
        assert!((b / a - 0.524).abs() < 0.001, "axis ratio {} — the naive read is −0.032", b / a);
        assert!((tilt - -60.486).abs() < 1e-2, "major-axis tilt {tilt}°, measured −60.5 ± 0.9");
    }

    /// The corner encoding is a bijection, and the WRITER is its other half.
    /// Both of Lightroom's legal corner arrangements go out byte-identical to
    /// the way they came in — `Left < Right` (probe #4) and `Left > Right`
    /// (probe #8, which the model REQUIRES to carry `Angle > 0`, 6/6 in the
    /// user's library, `BBOX-DECODE.md` §2.1). Sorting or clamping the box on
    /// the way out destroys the second one.
    ///
    /// MUTATION THIS CATCHES: `min`/`max` the corners in `engine_to_lr`, or
    /// drop the `|θ| ≤ 45°` canonicalisation, and probe #8 comes back as a
    /// different ellipse.
    #[test]
    fn the_radial_corner_encoding_round_trips_both_lightroom_arrangements() {
        let frame = FrameAspect::from_size(9504.0, 6336.0);
        for (t, l, b, r, angle) in [
            ("-0.045582", "-0.056408", "0.9708", "1.062771", "24.348422"),
            ("-0.088191", "0.520214", "1.113528", "0.494492", "29.513785"),
            // `_DSC9600`, the subject the whole angle model was measured on.
            ("-0.082402", "-0.008723", "1.109604", "1.090228", "28.229232"),
            // …and an UNROTATED one, which must not acquire an angle.
            ("0.069396", "-0.059577", "0.855822", "1.06594", "0"),
        ] {
            let doc = in_frame(
                &lr_doc(&lr_correction("R", "", &lr_radial_at(t, l, b, r, angle))),
                9504,
                6336,
            );
            let recipe = xmp_to_recipe(&doc);
            let out = recipe_to_xmp_in_frame(&recipe, frame).0;
            let shown = &out[out.find("CircularGradient").unwrap_or(0)..];
            let shown = &shown[..shown.len().min(400)];
            // The four CORNERS come back to the byte.
            for (key, want) in [("Top", t), ("Left", l), ("Bottom", b), ("Right", r)] {
                assert!(
                    out.contains(&format!("crs:{key}=\"{want}\"")),
                    "crs:{key} must come back as {want}: {shown}"
                );
            }
            // The ANGLE cannot, and the reason is arithmetic rather than
            // geometric: `MaskGeometry::Radial::angle` is an `f32`, whose
            // ~6 × 10⁻⁸ relative precision is 2 × 10⁻⁶ of a 34° engine angle
            // and 7.6 × 10⁻⁶ of a 73° one — a unit or two in the sixth decimal
            // Lightroom writes. Measured here: 2 × 10⁻⁶ ° on `#4`, the worst
            // 1.4 × 10⁻⁵ ° on `#8` (whose engine angle is −73.198° and whose
            // decode goes through the ±45° axis swap). Bounded at 10⁻⁴ °, four
            // orders under the +0.33° systematic the tilt measurement that
            // calibrated this carries.
            let at = out.find("crs:Angle=\"").expect("an angle is written") + 11;
            let got: f64 = out[at..][..out[at..].find('"').unwrap()].parse().expect("a number");
            let want: f64 = angle.parse().unwrap();
            assert!((got - want).abs() < 1e-4, "crs:Angle {got} vs {want}: {shown}");
        }
    }

    /// The decode's own guard. `a > 0 ∧ b > 0` holds for 80/80 rotated
    /// components in the user's library and is not a fit — it is what the model
    /// PREDICTS (`Left > Right` forces `Angle > 0`, `Top > Bottom` forces
    /// `Angle < 0`, both at once impossible; the library agrees 16/16,
    /// p = 2.5 × 10⁻⁵). A box that violates it is not an ellipse this model can
    /// name, so the correction is refused and NAMED rather than rendered as
    /// some other shape.
    ///
    /// MUTATION THIS CATCHES: delete the guard and the mask imports as a
    /// mirrored ellipse with no disclosure at all.
    #[test]
    fn a_radial_whose_corners_do_not_decode_is_refused_and_named() {
        // `Left > Right` with a NEGATIVE angle — the one combination the sign
        // law forbids.
        let doc = in_frame(
            &lr_doc(&lr_correction(
                "Impossible",
                "",
                &lr_radial_at("-0.088191", "0.520214", "1.113528", "0.494492", "-29.513785"),
            )),
            9504,
            6336,
        );
        assert!(xmp_to_recipe(&doc).masks.is_empty(), "a box that cannot decode must not render");
        assert_eq!(unsupported_corrections(&doc), 1, "and it is counted as a drop");
        assert_eq!(
            import_losses(&doc),
            vec![MaskImportLoss {
                name: "Impossible".into(),
                reason: MaskImportReason::OutOfModel
            }],
            "…and NAMED"
        );
        // The same box with the angle the law requires decodes fine — so the
        // refusal is about the geometry, not about the file being unusual.
        let ok = in_frame(
            &lr_doc(&lr_correction(
                "Fine",
                "",
                &lr_radial_at("-0.088191", "0.520214", "1.113528", "0.494492", "29.513785"),
            )),
            9504,
            6336,
        );
        assert_eq!(xmp_to_recipe(&ok).masks.len(), 1);
    }

    /// The frame affine is the IDENTITY — the 2026-08-19 ruling set
    /// `LR_MASK_FRAME_SCALE = 1.0` after R27 Batches 8+10 proved the old
    /// `k = 1.032` was one frame's LENS-PROFILE WARP mistaken for a constant
    /// (`batch10-report.md` §5: the `LensProfileEnable` toggle moves the
    /// implied scale 0.984 → 0.998, and 11 dabs displace as a radial
    /// distortion polynomial, not a scale). The recipe carries the sidecar's
    /// STORED geometry verbatim.
    ///
    /// The history this test used to pin, kept legible: `_DSC9681`
    /// (`Feather="0"`, centre 2799 px off frame-centre) RENDERS its centre at
    /// **(2571.0, 5060.0)** px (`PROBE4-FINAL.md` §2, 2880-ray edge fit) —
    /// ~88 px from the stored (2638.4, 5002.8), because THAT frame's warp is
    /// ≈1.0315. That warp is now UNMODELLED BY DECISION (batch10 §7.5: no
    /// `.lcp` reader yet), so the recipe must hold the stored centre and the
    /// 88 px is the disclosed residual, not something to bake in.
    ///
    /// MUTATION THIS CATCHES: put any `k ≠ 1` back (1.032, or half-apply it
    /// to axes only) and the verbatim assertions fail by 29–88 px.
    #[test]
    fn the_frame_affine_is_the_identity_since_the_lens_warp_ruling() {
        let (w, h) = (9504.0, 6336.0);
        let doc = in_frame(
            &lr_doc(&lr_correction(
                "R",
                "",
                &lr_radial_at("0.597862", "0.009087", "0.981315", "0.546133", "0"),
            )),
            9504,
            6336,
        );
        let MaskGeometry::Radial { top, left, bottom, right, .. } =
            xmp_to_recipe(&doc).masks[0].mask
        else {
            panic!("expected a radial");
        };
        let (cx, cy) = (
            (left as f64 + right as f64) / 2.0 * w,
            (top as f64 + bottom as f64) / 2.0 * h,
        );
        // The STORED centre, verbatim.
        let stored = ((0.009087 + 0.546133) / 2.0 * w, (0.597862 + 0.981315) / 2.0 * h);
        assert!((cx - stored.0).abs() < 0.01, "centre x {cx} px must be the stored {}", stored.0);
        assert!((cy - stored.1).abs() < 0.01, "centre y {cy} px must be the stored {}", stored.1);
        // …and PROBE4's warped-render measurement stays ~88 px away — the
        // known, disclosed, unmodelled lens warp of that frame.
        assert!(
            (2571.0f64 - cx).hypot(5060.0 - cy) > 80.0,
            "the warp residual on _DSC9681 is real and unmodelled: ({cx}, {cy})"
        );
    }

    /// v0.32.0 narrowed the rotation disclosure to "the document declares no
    /// frame" — but there are TWO ways the tilt fails to arrive, and the frame
    /// narrows only one. An angle that cannot be PARSED is a rotation nobody
    /// can apply however well the frame is known: `parse_one_correction` reads
    /// it as 0, so the mask arrives axis-aligned and the file's own intent is
    /// gone. That has to keep saying so.
    ///
    /// MUTATION THIS CATCHES: gate the reason on `frame.is_none()` alone and
    /// this correction imports rotated-to-zero in silence.
    #[test]
    fn an_unreadable_angle_is_disclosed_even_when_the_frame_is_known() {
        let doc = in_frame(
            &lr_doc(&lr_correction(
                "Garbled",
                "",
                &lr_radial_at("-0.045582", "-0.056408", "0.9708", "1.062771", "twenty-four"),
            )),
            9504,
            6336,
        );
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "the shape is still readable, so the mask arrives");
        let MaskGeometry::Radial { angle, .. } = r.masks[0].mask else { panic!("radial") };
        assert_eq!(angle, 0.0, "an angle we cannot parse is not an angle we can apply");
        assert_eq!(
            import_losses(&doc),
            vec![MaskImportLoss {
                name: "Garbled".into(),
                // `0` is this payload's word for "no angle to name" — see the
                // variant's doc.
                reason: MaskImportReason::Rotation(0)
            }],
            "…and it is NAMED, frame or no frame"
        );
    }

    /// `crs:LocalHue` rides a 180 scale, not the ÷100 every other local key
    /// uses. MEASURED, 2026-08-18: the user's controlled export put the mask
    /// Hue slider at **+50** and Lightroom wrote **`crs:LocalHue="0.277778"`**
    /// (`_DSC9594.xmp`, verbatim). 0.277778 × 180 = 50.00004; ÷100 would read
    /// 27.8 and ÷360 would read 100.
    ///
    /// The gate moves with the reader: at the old 100 scale a slider past
    /// ±55.6 refused the WHOLE correction as out of model.
    ///
    /// MUTATION THIS CATCHES: put `q100("LocalHue")` back and the anchor reads
    /// 27.7778; leave the domain gate on the 100 scale and the ±100 row
    /// refuses.
    #[test]
    fn local_hue_rides_the_measured_180_scale() {
        let doc = lr_doc(&lr_correction("Hue", "", &lr_radial("0", "0")))
            .replace("crs:LocalHue=\"0\"", "crs:LocalHue=\"0.277778\"");
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "the correction imports");
        assert!((r.masks[0].hue - 50.0).abs() < 1e-3, "UI +50 ⇒ {}", r.masks[0].hue);
        // The whole ±100 slider is inside the model — including its end
        // stops, which land on ±0.555556 and read back as ±100.00008 through
        // the wire's six decimals.
        for (text, want) in [("0.555556", 100.0), ("-0.555556", -100.0)] {
            let doc = lr_doc(&lr_correction("Hue", "", &lr_radial("0", "0")))
                .replace("crs:LocalHue=\"0\"", &format!("crs:LocalHue=\"{text}\""));
            let r = xmp_to_recipe(&doc);
            assert_eq!(r.masks.len(), 1, "{text} is inside Lightroom's own slider");
            assert!((r.masks[0].hue - want).abs() < 1e-2, "{text} ⇒ {}", r.masks[0].hue);
        }
        // …and a value PAST it is out of model and refused, which is the half
        // the gate exists for: 0.7 is a hue of 126, half a turn's worth beyond
        // the slider. On the old 100 scale the same file read 70 and sailed
        // through.
        let doc = lr_doc(&lr_correction("Hue", "", &lr_radial("0", "0")))
            .replace("crs:LocalHue=\"0\"", "crs:LocalHue=\"0.7\"");
        assert!(xmp_to_recipe(&doc).masks.is_empty(), "a hue past the slider is out of model");
        assert_eq!(
            import_losses(&doc),
            vec![MaskImportLoss { name: "Hue".into(), reason: MaskImportReason::OutOfModel }],
            "…and NAMED"
        );
        // …and the writer is its inverse.
        let mine = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Radial {
                    top: 0.3, left: 0.35, bottom: 0.7, right: 0.65,
                    feather: 0.5, roundness: 0.0, flipped: false, angle: 0.0,
                    midpoint: 50.0, mask_version: 2,
                },
                hue: 50.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let out = recipe_to_xmp(&mine);
        assert!(out.contains("crs:LocalHue=\"0.2777778\""), "{out}");
        assert!((xmp_to_recipe(&out).masks[0].hue - 50.0).abs() < 1e-3);
    }

    // ── R27: the crop rectangle is the SAME rotated-corner encoding ─────────
    //
    // The seven rows below are the user's own library, transcribed verbatim
    // from `~/.claude/plans/r27-materials/p3-scratch/final_table.py:10-18`
    // (the script `P3-cropangle-model.md` §8 lists as its reproduction). Each
    // is a self-consistent pair: the crop block and the pixels come out of the
    // same file, so a stale sidecar cannot contaminate it. NO photograph and
    // NO export is in this repository — these are the metadata numbers and the
    // exported dimensions they predict.
    //
    // `(name, W, H, L, T, R, B, CropAngle, exported W × H in SENSOR
    //  orientation, px of width and height this engine's composition clamps
    //  away)`.
    type P3Crop = (&'static str, f64, f64, f64, f64, f64, f64, f64, f64, f64, f64, f64);
    const P3_CROPS: [P3Crop; 7] = [
        ("_DSC9558", 9504.0, 6336.0, 0.119389, 0.003478, 0.994441, 0.991209, -0.132584, 8302.0, 6277.0, 0.000, 0.000),
        ("DSC09024_1", 9504.0, 6336.0, 0.00219, 0.007883, 0.99781, 0.992117, -0.303486, 9429.0, 6286.0, 0.000, 0.216),
        ("_DSC9493", 9504.0, 6336.0, 0.013441, 0.0, 0.986559, 1.0, 0.724343, 9328.0, 6219.0, 0.000, 1.243),
        ("_DSC1216_hdr", 9438.0, 6265.0, 0.005237, 0.018724, 0.994763, 0.981276, -0.725680, 9262.0, 6148.0, 0.000, 1.251),
        ("_DSC9138", 9504.0, 6336.0, 0.031274, 0.0, 0.968726, 1.0, 1.728388, 9097.0, 6065.0, 0.000, 6.909),
        ("_DSC9443_1", 9504.0, 6336.0, 0.094153, 0.135618, 0.978448, 0.955195, -1.979145, 8220.0, 5480.0, 0.000, 30.260),
        ("_DSC9298", 9504.0, 6336.0, 0.09115, 0.100566, 1.0, 0.900897, -3.274380, 8334.0, 5556.0, 5.895, 0.000),
    ];

    /// §0 OF R27 BATCH-3, the crop half. `crs:Crop{Left,Top,Right,Bottom}` are
    /// the two opposite ROTATED CORNERS of the crop rectangle in the un-rotated
    /// source frame — the identical encoding `BBOX-DECODE.md` found for
    /// `Mask/CircularGradient`, one family — and the exported dimensions are
    /// that rectangle's own side lengths, `2p × 2q`.
    ///
    /// **7/7 pixel-exact, zero free parameters**, across two signs of
    /// `CropAngle`, two `tiff:Orientation` states and two source aspect ratios
    /// (`P3-cropangle-model.md` §3.2). The rivals, on the same seven rows:
    /// naive AABB (what this build read until R27) 485 px, 0/7; opposite sign
    /// 987 px; rotation in normalised space 165 px; AABB of the rotated rect
    /// 957 px; fractions of the rotated bbox 619 px; of the inscribed rect
    /// 878 px; side length ÷cos θ 477 px. The first two are asserted below, so
    /// restoring either reading fails here rather than in a photograph.
    ///
    /// The last two columns are what this engine's own composition
    /// (rotate → auto-crop to the inscribed rectangle → crop) cannot express:
    /// a Lightroom rectangle pushed against the edge of the rotated frame can
    /// reach outside the CENTRED inscribed rectangle. Measured, per specimen,
    /// and it is 0 on the first row, ≤ 1.3 px on three more, and worst at
    /// 30.3 px = 0.55 % of one edge.
    ///
    /// MUTATION THIS CATCHES: drop the `sin`/`cos` mixing in
    /// `lr_to_engine_crop` (i.e. go back to `W·(R−L) × H·(B−T)`) and every row
    /// but the sign-free ones misses its exported size by tens to hundreds of
    /// pixels; flip the straighten's sign and the predicted sizes swap into the
    /// opposite-sign column.
    #[test]
    fn the_seven_measured_lightroom_crops_reproduce_their_exported_dimensions() {
        for (name, w, h, l, t, r, b, angle, ow, oh, lost_w, lost_h) in P3_CROPS {
            let frame = FrameAspect::from_size(w, h);
            let lr = LrCrop { left: l, top: t, right: r, bottom: b, angle_deg: angle };
            let CropDecode::Read { crop: Some(c), straighten_deg, .. } =
                lr_to_engine_crop(lr, frame, false)
            else {
                panic!("{name}: a real Lightroom crop must decode");
            };
            assert!(
                (straighten_deg + angle).abs() < 1e-12,
                "{name}: the engine's clockwise straighten is −CropAngle"
            );
            // The frame this engine's straighten leaves behind, in pixels.
            let (wi, hi) = inscribed_norm(w / h, straighten_deg);
            let (wi, hi) = (wi * h, hi * h);
            // …and the rectangle's own side lengths inside it, with the
            // clamped edge added back: that IS the model's `2p × 2q`.
            let out_w = (c.right - c.left) as f64 * wi + lost_w;
            let out_h = (c.bottom - c.top) as f64 * hi + lost_h;
            // Half a pixel — `W_out = round(2p)` per axis — on the six
            // specimens whose crop block and pixels come out of the SAME file.
            // `_DSC9138` is the one paired to a SIDECAR, so its block may have
            // moved since the export; `P3-cropangle-model.md` §6.5 registers
            // its −0.61 px height as exactly that (and notes 9097/1.5 =
            // 6064.67, which is what an aspect lock would give).
            let tol = if name == "_DSC9138" { 0.65 } else { 0.5 };
            assert!(
                (out_w - ow).abs() < tol && (out_h - oh).abs() < tol,
                "{name}: model says {out_w:.1} × {out_h:.1}, Lightroom exported {ow} × {oh}"
            );
            // The two headline rivals, refuted on this row's own numbers.
            let naive = ((r - l) * w, (b - t) * h);
            let flipped = {
                let (sin, cos) = angle.to_radians().sin_cos();
                let (x, y) = ((r - l) / 2.0 * w, (b - t) / 2.0 * h);
                (2.0 * (x * cos - y * sin), 2.0 * (x * sin + y * cos))
            };
            if angle.abs() > 0.2 {
                assert!(
                    (naive.0 - ow).abs() > 1.0 || (naive.1 - oh).abs() > 1.0,
                    "{name}: the naive AABB must NOT reproduce {ow} × {oh}"
                );
                assert!(
                    (flipped.0 - ow).abs() > 1.0 || (flipped.1 - oh).abs() > 1.0,
                    "{name}: the opposite sign must NOT reproduce {ow} × {oh}"
                );
            }
        }
    }

    /// The crop codec is an ALGEBRAIC inverse: whatever Lightroom wrote comes
    /// back, to the last decimal it spelled. Run on the four P3 specimens whose
    /// rectangle fits this engine's inscribed frame outright, plus the two
    /// arrangements §6.3 says are legal.
    ///
    /// MUTATION THIS CATCHES: swap `p*cos − q*sin` for `p*cos + q*sin` in
    /// `engine_to_lr_crop` (the natural sign slip) and every rotated row comes
    /// back with different corners.
    #[test]
    fn the_crop_corner_encoding_round_trips_what_lightroom_wrote() {
        for (name, w, h, l, t, r, b, angle, ..) in P3_CROPS {
            let frame = FrameAspect::from_size(w, h);
            let lr = LrCrop { left: l, top: t, right: r, bottom: b, angle_deg: angle };
            let CropDecode::Read { crop: Some(c), straighten_deg, overshoot_frac } =
                lr_to_engine_crop(lr, frame, false)
            else {
                panic!("{name}: must decode");
            };
            let back = engine_to_lr_crop(Some(&c), straighten_deg, frame).expect("a crop");
            // The clamp is the only lossy edge, and it moves an edge by at
            // most its own overshoot — so that is the tolerance, and it is
            // ZERO on the rows that needed no clamp.
            let tol = overshoot_frac + 1e-6;
            for (got, want, key) in [
                (back.left, l, "Left"),
                (back.top, t, "Top"),
                (back.right, r, "Right"),
                (back.bottom, b, "Bottom"),
            ] {
                assert!(
                    (got - want).abs() <= tol,
                    "{name}: crs:Crop{key} {got:.6} vs {want:.6} (tol {tol:.6})"
                );
            }
            assert!((back.angle_deg - angle).abs() < 1e-12, "{name}: the angle is exact");
        }
    }

    /// R27 `P3-cropangle-model.md` §6.3: `Left > Right` is a legal Lightroom
    /// arrangement — under the corner encoding it means `X < 0`, i.e.
    /// `tan θ > p/q`, which a 2:3 crop straightened past +33.69° produces, and
    /// `crs:CropAngle`'s own documented range is ±45°. The pre-R27 reader
    /// required `left < right && top < bottom` and threw such a crop away in
    /// silence.
    ///
    /// The fixture is CONSTRUCTED, not copied: no file in the user's library
    /// reaches the inverted region (the nine non-zero `CropAngle` sidecars all
    /// sit ≥ 30.4° from their own wall), so the encoder builds the corners a
    /// 35° straighten of a tall rectangle would produce and the decoder has to
    /// read them back.
    ///
    /// MUTATION THIS CATCHES: restore either half of the ordering guard and
    /// the inverted arrangement decodes to `None` — a crop silently gone.
    #[test]
    fn an_inverted_crop_arrangement_is_read_rather_than_discarded() {
        // A tall (2:3) rectangle inside a 3:2 frame, straightened 35°.
        let frame = FrameAspect::from_size(9504.0, 6336.0);
        let engine = Crop { left: 0.30, top: 0.10, right: 0.62, bottom: 0.90 };
        let lr = engine_to_lr_crop(Some(&engine), -35.0, frame).expect("corners");
        assert!(
            lr.left > lr.right,
            "the fixture must actually reach the inverted region: {lr:?}"
        );
        let CropDecode::Read { crop: Some(back), straighten_deg, .. } =
            lr_to_engine_crop(lr, frame, false)
        else {
            panic!("an inverted arrangement must still decode: {lr:?}");
        };
        assert!((straighten_deg + 35.0).abs() < 1e-12);
        for (got, want) in [
            (back.left, engine.left),
            (back.top, engine.top),
            (back.right, engine.right),
            (back.bottom, engine.bottom),
        ] {
            assert!((got - want).abs() < 1e-5, "{got} != {want} ({back:?})");
        }
    }

    /// The whole document round trip on a tilted crop, through the reader and
    /// the writer the app actually calls — including the `{:.6}` `CropAngle`
    /// that replaced `{:.1}` (`P3-cropangle-model.md` §6.4: importing
    /// `_DSC9298` and saving it back emitted `-3.3`, a 0.0256° drift = 4.3 px
    /// of edge-to-edge tilt across a 9504 px frame).
    ///
    /// MUTATION THIS CATCHES: put `{:.1}` back and the angle assertion fails
    /// by the exact drift the report measured; drop the negation at either end
    /// and the round trip returns `+3.274380`.
    #[test]
    fn a_tilted_crop_survives_a_whole_document_round_trip() {
        let doc = in_frame(
            &lr_doc(""),
            9504,
            6336,
        )
        .replace(
            "crs:Version=\"15.5.1\"",
            "crs:Version=\"15.5.1\"\n   crs:HasCrop=\"True\"\n   crs:CropLeft=\"0.09115\"\n   \
             crs:CropTop=\"0.100566\"\n   crs:CropRight=\"1\"\n   crs:CropBottom=\"0.900897\"\n   \
             crs:CropAngle=\"-3.274380\"",
        );
        let r = xmp_to_recipe(&doc);
        assert!((r.straighten_deg - 3.27438).abs() < 1e-5, "{}", r.straighten_deg);
        let c = r.crop.expect("the tilted rectangle imports");
        let out = recipe_to_xmp_in_frame(&r, FrameAspect::from_size(9504.0, 6336.0)).0;
        assert!(out.contains("crs:CropAngle=\"-3.274380\""), "{out}");
        // …and the rectangle comes back within the clamp this composition
        // costs on this specimen (5.9 px of 9186 = 0.00064).
        let back = xmp_to_recipe(&out).crop.expect("and again");
        for (got, want) in [
            (back.left, c.left),
            (back.top, c.top),
            (back.right, c.right),
            (back.bottom, c.bottom),
        ] {
            assert!((got - want).abs() < 1e-5, "{got} != {want}");
        }
    }

    /// A tilted crop in a FOREIGN document that declares no frame cannot be
    /// placed — and is dropped and disclosed rather than read as the
    /// axis-aligned rectangle it is not. Ours is read verbatim, because that
    /// is precisely what this writer's frameless arm emits.
    ///
    /// MUTATION THIS CATCHES: read the foreign one verbatim too and the note
    /// disappears while a rectangle appears out of corners that mean something
    /// else.
    #[test]
    fn a_frameless_tilted_crop_is_dropped_for_a_foreign_document_and_kept_for_ours() {
        let crop = "crs:HasCrop=\"True\" crs:CropLeft=\"0.05\" crs:CropTop=\"0\" \
                    crs:CropRight=\"0.95\" crs:CropBottom=\"1\" crs:CropAngle=\"-1.5\"";
        let foreign = format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
             xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description \
             rdf:about=\"\" xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             {crop}/></rdf:RDF></x:xmpmeta>"
        );
        let r = xmp_to_recipe(&foreign);
        assert_eq!(r.crop, None, "a rectangle we cannot place is not invented");
        assert!((r.straighten_deg - 1.5).abs() < 1e-6, "the tilt needs no aspect and rides");
        assert!(
            crop_import_note(&foreign).is_some_and(|n| n.contains("could not be placed")),
            "{:?}",
            crop_import_note(&foreign)
        );
        // Ours: the writer's own frameless arm wrote the rectangle in the
        // straightened frame, so the reader takes it back.
        let mine = EditRecipe {
            crop: Some(Crop { left: 0.05, top: 0.0, right: 0.95, bottom: 1.0 }),
            straighten_deg: 1.5,
            ..Default::default()
        };
        let back = xmp_to_recipe(&recipe_to_xmp(&mine));
        assert_eq!(back.crop, mine.crop, "our own frameless round trip is lossless");
        assert!(crop_import_note(&recipe_to_xmp(&mine)).is_none());
    }

    /// R27 T3, the second half of `P5-cropped-mask-frame.md` §8's parser
    /// lesson: `crs:RetouchAreas` carries its OWN `Mask/Ellipse` (and
    /// `Mask/Paint`) components, which are healing brushes rather than local
    /// adjustments — a flat `crs:What="Mask/…"` count over the packet
    /// disagrees with the correction-scoped parse on **83 of 166** images in
    /// the user's library. This reader has always scoped its scan to
    /// `crs:MaskGroupBasedCorrections` (`mask_summary`); the point of this
    /// test is that it STAYS scoped.
    ///
    /// MUTATION THIS CATCHES: point `mask_summary` at the whole crs scope
    /// instead of the correction block and the retouch ellipse arrives as a
    /// phantom mask (or a phantom loss).
    #[test]
    fn a_retouch_area_is_not_counted_as_a_local_adjustment() {
        let retouch = "  <crs:RetouchAreas>\n   <rdf:Seq>\n    <rdf:li>\n     \
             <rdf:Description crs:SpotType=\"heal\" crs:SourceState=\"sourceSetAutomatically\">\n\
             \x20    <crs:Masks>\n      <rdf:Seq>\n       <rdf:li crs:What=\"Mask/Ellipse\" \
             crs:MaskValue=\"1\" crs:Top=\"0.1\" crs:Left=\"0.1\" crs:Bottom=\"0.2\" \
             crs:Right=\"0.2\"/>\n      </rdf:Seq>\n     </crs:Masks>\n     \
             </rdf:Description>\n    </rdf:li>\n   </rdf:Seq>\n  </crs:RetouchAreas>\n";
        let doc = lr_doc(&lr_correction("R", "", &lr_radial("0", "0")))
            .replace("  <crs:MaskGroupBasedCorrections>", &format!("{retouch}  <crs:MaskGroupBasedCorrections>"));
        assert!(doc.contains("Mask/Ellipse"), "the fixture must carry the retouch component");
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "one correction, one mask — the retouch is not one");
        assert_eq!(unsupported_corrections(&doc), 0, "nor is it a LOSS");
        assert!(import_losses(&doc).is_empty(), "{:?}", import_losses(&doc));
    }

    /// R27 T4, `P2-feather-k-closures.md` §4.3. Lightroom's rule for the
    /// detail/NR companions is **amount-gated**: 4/4 exports with
    /// `LuminanceSmoothing > 0` carry the Detail companion and 0 of 207 with
    /// it at 0 do; `SharpenEdgeMasking="0"` rides on 159 files whose
    /// `Sharpness` is set. This writer gated each companion on its OWN value,
    /// so `DSC09533.JPG`'s shape — `LuminanceSmoothing="50"`,
    /// `…Detail="50"`, `…Contrast="0"` — came out with the Contrast key
    /// missing, which no Lightroom file has.
    ///
    /// Behaviourally neutral by construction: only the companions whose ACR
    /// default is ZERO join the rule (see `amount_carries`), so an emitted
    /// `"0"` says exactly what its absence said.
    ///
    /// MUTATION THIS CATCHES: revert `amount_carries` to `false` and the two
    /// zero-valued companions vanish from a document whose amounts are set;
    /// widen it to every companion and `SharpenRadius="+0.0"` starts asserting
    /// a radius Lightroom would render at.
    #[test]
    fn the_detail_companions_ride_on_their_amount_the_way_lightroom_writes_them() {
        // `DSC09533.JPG`, verbatim (P2 §4.1).
        let r = EditRecipe {
            noise_reduction: 50.0,
            nr_detail: 50.0,
            nr_contrast: 0.0,
            color_nr: 100.0,
            color_nr_detail: 50.0,
            color_nr_smooth: 50.0,
            sharpening: 0.0,
            ..Default::default()
        };
        let x = recipe_to_xmp(&r);
        for want in [
            "crs:LuminanceSmoothing=\"50\"",
            "crs:LuminanceNoiseReductionDetail=\"50\"",
            "crs:LuminanceNoiseReductionContrast=\"0\"",
            "crs:ColorNoiseReduction=\"100\"",
            "crs:ColorNoiseReductionDetail=\"50\"",
            "crs:ColorNoiseReductionSmoothness=\"50\"",
        ] {
            assert!(x.contains(want), "the LR-shaped NR block is missing {want}: {x}");
        }
        assert!(!x.contains("SharpenEdgeMasking"), "no sharpening amount, no companion: {x}");
        assert!(!x.contains("SharpenRadius"), "and never an invented radius: {x}");
        // Sharpening set, masking at rest: Lightroom writes the zero.
        let sharp = EditRecipe { sharpening: 45.0, ..Default::default() };
        let x = recipe_to_xmp(&sharp);
        assert!(x.contains("crs:SharpenEdgeMasking=\"0\""), "{x}");
        assert!(!x.contains("SharpenDetail"), "a 25-default companion stays absent: {x}");
        assert!(!x.contains("SharpenRadius"), "so does the 1.0-default radius: {x}");
        // A recipe with NO noise reduction keeps the whole block absent — the
        // half of the rule the old per-key gate got right.
        let none = recipe_to_xmp(&EditRecipe::default());
        assert!(!none.contains("LuminanceNoiseReductionContrast"), "{none}");
        assert!(!none.contains("SharpenEdgeMasking"), "{none}");
    }

    /// R27 A7. A PORTRAIT capture's `crs:` coordinates are fractions of the
    /// UN-ROTATED SENSOR ARRAY, and the export is already upright
    /// (`P1-portrait-mask-frame.md` §1, HIGH; 7/7 files pick their true frame
    /// by `dSS`, four landscape controls recover the known answer).
    ///
    /// The fixture is `_DSC9527-已增强-NR.JPG`'s Mask 8 (P1 §4.2), verbatim:
    /// a single `Mask/CircularGradient` at `Angle="0"` declaring +1.6 EV, whose
    /// `|b|` far exceeds the frame so it renders as a BAND — and the two
    /// readings put that band on perpendicular axes, 12 628 px apart. P1
    /// measures its fitted gain at **+1.647 ± 0.270** under the sensor frame
    /// against a declared +1.60, and **−1.982** — the wrong SIGN — under the
    /// display one.
    ///
    /// The two numbers asserted here are P1 §4.2's own: the band sits about
    /// display `y = 4748` with a half-extent of `4315` px along that axis.
    /// Under the rejected reading it would be a VERTICAL band about
    /// `x = 3171` with half-width `4453`.
    ///
    /// MUTATION THIS CATCHES: drop the `orient_recipe_coords` call at the end
    /// of `xmp_to_recipe` (or read `tiff:Orientation` as `Normal`) and the band
    /// comes back vertical — the v0.33 defect P1 was written to close.
    #[test]
    fn a_portrait_captures_mask_is_decoded_in_the_sensor_frame() {
        // sensor 9504 × 6336, tiff:Orientation="8" → display 6336 × 9504.
        let doc = in_frame(
            &lr_doc(&lr_correction(
                "Mask 8",
                "",
                &lr_radial_at("-3.21675", "0.060424", "2.073933", "0.940417", "0"),
            )),
            9504,
            6336,
        )
        .replace("tiff:ImageWidth", "tiff:Orientation=\"8\"\n   tiff:ImageWidth");
        let r = xmp_to_recipe(&doc);
        let MaskGeometry::Radial { top, left, bottom, right, .. } = r.masks[0].mask else {
            panic!("a radial");
        };
        // The engine's frame is the DISPLAY one: 6336 × 9504.
        let (cy, ry) = (
            (top as f64 + bottom as f64) / 2.0 * 9504.0,
            (bottom as f64 - top as f64) / 2.0 * 9504.0,
        );
        assert!((cy - 4748.0).abs() < 1.5, "the band's display centre is y = {cy:.0}, not 4748");
        // P1 §4.2's own pixel number was 4315 — measured on the EXPORT, i.e.
        // the k-image (4315 = 1.032 × 4182). Since the 2026-08-19 ruling set
        // `LR_MASK_FRAME_SCALE = 1.0` the recipe carries the STORED extent,
        // 0.4399965 × 9504 = 4181.7; the residual vs the export is that
        // frame's lens-profile warp, unmodelled by decision (batch10 §7.5).
        assert!((ry - 4182.0).abs() < 1.5, "its display half-extent is {ry:.0}, not 4182");
        // …and the OTHER axis is the one that overflows the frame, which is
        // what makes it a band rather than an ellipse.
        let rx = (right as f64 - left as f64) / 2.0 * 6336.0;
        assert!(rx > 6336.0, "the perpendicular half-extent {rx:.0} must exceed the frame");
        // The writer un-turns it: the file's own four numbers come back.
        let out = recipe_to_xmp_in_frame(
            &r,
            FrameAspect::from_size_turned(9504.0, 6336.0, rawler::Orientation::Rotate270),
        )
        .0;
        for (key, want) in [
            ("Top", "-3.21675"),
            ("Left", "0.060424"),
            ("Bottom", "2.073933"),
            ("Right", "0.940417"),
        ] {
            assert!(
                out.contains(&format!("crs:{key}=\"{want}\"")),
                "crs:{key} did not survive the portrait round trip: {out}"
            );
        }
    }

    /// R29 C1, the sidecar boundary — the brush twin of the radial test above.
    ///
    /// A portrait capture's `crs:Dabs` are fractions of the UN-ROTATED SENSOR
    /// array, exactly like its `crs:Top/Left`, so the reader has to turn them
    /// into the display frame and the writer has to turn them back. Until this
    /// batch neither happened: the stream was carried verbatim, which made the
    /// round trip byte-exact and the RENDER wrong by a quarter turn — invisible
    /// while the brush drew nothing (R27 Batch-4) and visible the moment it did
    /// (R29 Batch-6b).
    ///
    /// The rewrite is not free, and this test is where the cost is legible: the
    /// stream that comes back out was COMPUTED, not copied. It survives here
    /// because Lightroom writes six decimals and this writer re-emits six
    /// (`render::LR_DAB_DECIMALS`), so on the pure rotations the decimal grid
    /// is closed under the turn — `1 − 0.800000` is `0.200000` both ways, and
    /// `0.582157 × 1.5 ÷ 1.5` lands back on `0.582157`. It is arithmetic that
    /// happens to be exact, not a carry, and a frame whose aspect is not 3:2
    /// would come home within a millionth instead of on it.
    ///
    /// MUTATIONS THIS CATCHES: drop the frame argument at either boundary (the
    /// import leaves the dabs sensor-frame, so the export turns them once and
    /// they leave the file rotated); hand `in_source_frame` the SOURCE aspect
    /// instead of `displayed()` (the radius comes back 1.31, i.e. 1.5² × the
    /// error).
    #[test]
    fn a_portrait_captures_brush_turns_on_the_way_in_and_comes_home_on_the_way_out() {
        // sensor 9504 × 6336, tiff:Orientation="8" = Rotate270 → display
        // 6336 × 9504. `lr_paint` pins crs:Radius="0.582157" (the F2 specimen).
        let paint = lr_paint(
            "FA7459A9F5626F4881D7B730C3093F95",
            "1",
            "0",
            "false",
            &["r 0.200000", "d 0.100000 0.800000"],
        );
        let group = format!(
            "<rdf:li>\n\
             <rdf:Description crs:What=\"Mask/Aggregate\" crs:MaskActive=\"true\"\n\
             crs:MaskName=\"Brush 1\" crs:MaskBlendMode=\"0\" crs:MaskInverted=\"false\"\n\
             crs:MaskSyncID=\"0000000000000000000000000000000D\" crs:MaskValue=\"1\">\n\
             <crs:Masks>\n<rdf:Seq>\n{paint}</rdf:Seq>\n</crs:Masks>\n\
             </rdf:Description>\n</rdf:li>\n"
        );
        let doc = in_frame(&lr_doc(&lr_correction("Mask 7", "", &group)), 9504, 6336)
            .replace("tiff:ImageWidth", "tiff:Orientation=\"8\"\n   tiff:ImageWidth");
        let r = xmp_to_recipe(&doc);
        let stroke = |r: &EditRecipe| {
            let g = &r.masks[0].mask;
            let MaskGeometry::Brush { strokes, .. } = g else { panic!("a brush, got {g:?}") };
            (strokes[0].dabs.clone(), strokes[0].radius)
        };
        // IN: Rotate270 maps (u,v) -> (v, 1−u), and the source aspect 1.5
        // rescales every width-unit radius.
        let (dabs, radius) = stroke(&r);
        assert_eq!(dabs, "r 0.300000\nd 0.800000 0.900000", "the stream must reach the display frame");
        assert!((radius - 0.873236).abs() < 1e-6, "crs:Radius became {radius}, not 0.873236");

        // OUT: the inverse, against the DISPLAYED aspect — the file's own
        // digits come back.
        let out = recipe_to_xmp_in_frame(
            &r,
            FrameAspect::from_size_turned(9504.0, 6336.0, rawler::Orientation::Rotate270),
        )
        .0;
        for want in [
            "<rdf:li>r 0.200000</rdf:li>",
            "<rdf:li>d 0.100000 0.800000</rdf:li>",
            "crs:Radius=\"0.582157\"",
        ] {
            assert!(out.contains(want), "{want} did not survive the portrait round trip: {out}");
        }
    }

    /// R27 A8, and it closes `C-rotation-skeleton.md`'s round-trip hole: a
    /// document THIS writer produces now declares the frame its coordinates
    /// are measured against, so re-importing one recovers the rotated radial
    /// it wrote. Before R27 a fresh sidecar declared nothing, and the reader —
    /// which needs `W/H` to fold a pixel-frame tilt into the engine's
    /// normalised one — could not read our own file back.
    ///
    /// MUTATION THIS CATCHES: return an empty string from `frame_declaration`
    /// and the re-import loses the tilt (and says so through
    /// `describe_import_losses`), which is exactly the hole.
    #[test]
    fn a_fresh_document_declares_its_frame_and_can_be_read_back() {
        let r = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Radial {
                    top: 0.30, left: 0.20, bottom: 0.62, right: 0.75,
                    feather: 0.5, roundness: 0.0, flipped: false, angle: 21.5,
                    midpoint: 50.0, mask_version: 2,
                },
                exposure_ev: 0.4,
                ..Default::default()
            }],
            crop: Some(Crop { left: 0.08, top: 0.05, right: 0.9, bottom: 0.94 }),
            straighten_deg: 1.25,
            ..Default::default()
        };
        for turn in [rawler::Orientation::Normal, rawler::Orientation::Rotate270] {
            let frame = FrameAspect::from_size_turned(9504.0, 6336.0, turn);
            let doc = recipe_to_xmp_in_frame(&r, frame).0;
            assert!(doc.contains("xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\""), "{doc}");
            assert!(doc.contains("tiff:ImageWidth=\"9504\""), "{doc}");
            assert!(doc.contains("tiff:ImageLength=\"6336\""), "{doc}");
            assert!(
                doc.contains(&format!("tiff:Orientation=\"{}\"", turn.to_u16())),
                "the DISPLAYED orientation, quarter turns included: {doc}"
            );
            let back = xmp_to_recipe(&doc);
            // Compared as the ELLIPSE it draws, in the frame this engine
            // displays — `(box, angle)` is a redundant carrier (a quarter turn
            // of the box with the angle shifted 90° is the SAME ellipse), and
            // Lightroom's own ±45° canonicalisation legitimately picks the
            // other representative on the way through.
            let (dw, dh) = if crate::decode::orientation_transposes(turn) {
                (6336.0, 9504.0)
            } else {
                (9504.0, 6336.0)
            };
            let (a0, b0, t0) = engine_pixel_ellipse(&r.masks[0].mask, dw, dh);
            let (a1, b1, t1) = engine_pixel_ellipse(&back.masks[0].mask, dw, dh);
            assert!(
                (a0 - a1).abs() < 1.0 && (b0 - b1).abs() < 1.0,
                "{turn:?}: semi-axes {a0:.1}/{b0:.1} came back {a1:.1}/{b1:.1}"
            );
            assert!(
                ((t0 - t1).rem_euclid(180.0)).min((t1 - t0).rem_euclid(180.0)) < 1e-2,
                "{turn:?}: tilt {t0} came back as {t1}"
            );
            assert!((back.straighten_deg - 1.25).abs() < 1e-4, "{turn:?}: tilt");
            let c = back.crop.expect("the crop survives");
            assert!(
                (c.left - 0.08).abs() < 2e-3 && (c.bottom - 0.94).abs() < 2e-3,
                "{turn:?}: crop {c:?}"
            );
        }
    }

    /// The REAL probe sidecars, when they are on the machine. Twelve controlled
    /// Lightroom exports live at
    /// `~/.claude/plans/r25-materials/lr-experiment/`; point
    /// `AUTOSHADE_LR_PROBE_FIXTURES` at that directory and this walks every
    /// `.xmp` in it (and its `probe*/` subdirectories), asserting that every
    /// radial imports and round-trips its corners byte-for-byte.
    ///
    /// SILENTLY SKIPPED when the variable is unset — the files are the user's
    /// photographs' metadata and are deliberately not in the repository, so
    /// this cannot be a CI gate. The synthetic fixtures above carry the same
    /// numbers; this is what proves the transcription.
    #[test]
    fn the_probe_sidecars_decode_to_their_measured_ellipses() {
        let Some(dir) = crate::config::live_env("AUTOSHADE_LR_PROBE_FIXTURES") else { return };
        let mut checked = 0usize;
        let mut roots = vec![std::path::PathBuf::from(dir)];
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        while let Some(root) = roots.pop() {
            let Ok(entries) = std::fs::read_dir(&root) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    roots.push(p);
                } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xmp")) {
                    files.push(p);
                }
            }
        }
        files.sort();
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            if !text.contains("Mask/CircularGradient") {
                continue;
            }
            let frame = FrameAspect::from_xmp(&text);
            assert!(frame.is_some(), "{}: a Lightroom sidecar declares its frame", path.display());
            let r = xmp_to_recipe(&text);
            assert!(!r.masks.is_empty(), "{}: its radial must import", path.display());
            let out = recipe_to_xmp_in_frame(&r, frame).0;
            let read = |doc: &str, key: &str| -> Option<String> {
                let name = format!("crs:{key}=\"");
                let at = doc.find(&name)? + name.len();
                Some(doc[at..][..doc[at..].find('"')?].to_string())
            };
            // Every corner the file wrote comes back BYTE-identical — the
            // decode/encode pair is an algebraic inverse and `lr_num` is
            // Lightroom's own spelling.
            for key in ["Top", "Left", "Bottom", "Right"] {
                let Some(want) = read(&text, key) else { continue };
                assert_eq!(
                    read(&out, key).as_deref(),
                    Some(want.as_str()),
                    "{}: crs:{key} did not survive the round trip",
                    path.display()
                );
            }
            // The angle to 10⁻⁴ °, for the `f32` reason the synthetic
            // round-trip test spells out.
            if let (Some(w), Some(g)) = (read(&text, "Angle"), read(&out, "Angle")) {
                let (w, g): (f64, f64) = (w.parse().unwrap(), g.parse().unwrap());
                assert!((w - g).abs() < 1e-4, "{}: crs:Angle {g} vs {w}", path.display());
            }
            // R27: the CROP block travels the same road, and every one of
            // these files carries `crs:HasCrop="False"` — so the crop codec
            // must leave them exactly that, not invent a full-frame carrier.
            for key in ["HasCrop", "CropTop", "CropLeft", "CropBottom", "CropRight", "CropAngle"] {
                assert_eq!(
                    read(&out, key),
                    read(&text, key),
                    "{}: crs:{key} did not survive the round trip",
                    path.display()
                );
            }
            // …and the frame this writer declares is the frame the file
            // declared (A8), which is what makes the round trip re-readable.
            // Both sides are read through the RESOLVED frame scope (R29-2), so
            // this compares what the reader will actually see, not a
            // first-occurrence sweep of two whole documents.
            for key in ["tiff:ImageWidth", "tiff:ImageLength", "tiff:Orientation"] {
                let want = FrameScope::resolve(&text).declared_number(key);
                let got = FrameScope::resolve(&out).declared_number(key);
                assert_eq!(want, got, "{}: {key} {got:?} vs {want:?}", path.display());
            }
            checked += 1;
        }
        assert!(checked > 0, "AUTOSHADE_LR_PROBE_FIXTURES held no radial sidecar");
        eprintln!("AUTOSHADE_LR_PROBE_FIXTURES: {checked} radial sidecar(s) round-tripped");
    }

    /// R25 P5: both rotation verdicts CARRY the angle, so a disclosure can
    /// say how much tilt it set aside instead of only that some existed.
    /// `0` is the payload's word for "no angle to name" — an unreadable
    /// `crs:Angle`, or a tilt that rounds away — and the prose channels fall
    /// back to their plain phrasing on it.
    #[test]
    fn the_rotation_loss_names_the_angle() {
        let radial = |angle: f32| MaskGeometry::Radial {
            top: 0.3, left: 0.35, bottom: 0.7, right: 0.65,
            feather: 0.5, roundness: 0.0, flipped: false, angle,
            midpoint: 50.0, mask_version: 2,
        };
        let with = |angle: f32| EditRecipe {
            masks: vec![LocalAdjustment {
                mask: radial(angle),
                name: "tilted".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            mask_export_losses(&with(37.412506)),
            vec![MaskLoss { name: "tilted".into(), reason: MaskLossReason::Rotation(37) }],
            "the export verdict names the engine's own angle"
        );
        assert_eq!(
            mask_export_losses(&with(-12.0)),
            vec![MaskLoss { name: "tilted".into(), reason: MaskLossReason::Rotation(-12) }],
            "including its sign"
        );
        assert_eq!(
            mask_export_losses(&with(0.4)),
            vec![MaskLoss { name: "tilted".into(), reason: MaskLossReason::Rotation(0) }],
            "a tilt under half a degree is still a loss, with no angle worth naming"
        );

        // The import twin, on the sidecar's own value — the measured negative
        // end of the reference library's range.
        let imported = |angle: &str| {
            import_losses(&lr_doc(&lr_correction("Radial 1", "", &lr_radial(angle, "0"))))
        };
        assert_eq!(
            imported("-43.945287"),
            vec![MaskImportLoss {
                name: "Radial 1".into(),
                reason: MaskImportReason::Rotation(-44)
            }],
            "the import verdict names Lightroom's angle"
        );
        assert_eq!(
            imported("oblique"),
            vec![MaskImportLoss {
                name: "Radial 1".into(),
                reason: MaskImportReason::Rotation(0)
            }],
            "an unreadable angle still counts as rotated — we cannot say it is zero"
        );
        assert!(imported("0").is_empty(), "and an unrotated radial loses nothing at all");
    }

    /// FORENSIC CONCLUSION, REVISED IN R25 P9 — read the revision, it is the
    /// interesting part.
    ///
    /// R24 observed that the reference sidecars carry `crs:Flipped="true"`
    /// BESIDE `crs:MaskInverted="false"` on the same component, concluded the
    /// two were INDEPENDENT (different concepts, both real), and this test
    /// pinned all four combinations importing to distinct flags. The
    /// observation was right and is unchanged; the CONCLUSION was wrong, and
    /// what falsified it was sample size. R24 saw one cell of a 2×2. The R25
    /// census walked the user's whole library — 201 radials — and found
    /// `Flipped` and `MaskInverted` PERFECTLY ANTI-CORRELATED: 155 `(true,
    /// false)`, 46 `(false, true)`, and ZERO of either matching pair. The two
    /// M-B batches agree on the raw bytes and are not in conflict: "the fields
    /// are not mirror copies of each other" (R24) and "their values are always
    /// opposite" (R25) are both true of the same files. Only the reading
    /// "therefore they mean different things" does not survive.
    ///
    /// So Lightroom writes ONE inversion bit TWICE. This engine has TWO flags
    /// composed by XOR (`Radial::flipped` in `mask_weight`, `inverted` in the
    /// weight loop, render.rs), and importing both halves of Lightroom's
    /// redundant pair XORed a value with its own complement: the net came out
    /// `true` for EVERY imported Lightroom radial whatever the file said.
    /// Measured on `DSC09568` against the real Lightroom export, tone-matched
    /// RMS 0.1099 → 0.0751 (blue 0.1901 → 0.0869) once the flip was dropped
    /// (E1-verdict §6 defect 2).
    ///
    /// MUTATION THIS CATCHES: putting `crs:Flipped` back into the geometry
    /// flag re-inverts every Lightroom radial (rows 1 and 2 below), and
    /// writing our own `flipped` straight back out re-emits a pair Lightroom
    /// never writes (the last assertion).
    #[test]
    fn lightroom_spells_one_inversion_bit_twice() {
        // The two pairs Lightroom actually writes, then the two it never does.
        // For the observed pairs the net is `MaskInverted`; for the impossible
        // ones the tie is broken in favour of `MaskInverted`, which is the
        // attribute this reader trusts (see the importer's comment).
        for (flipped, inverted, net) in [
            (true, false, false), // 155 of 201 in the library
            (false, true, true),  //  46 of 201
            (true, true, true),   //   0 of 201 — resolved, not guessed
            (false, false, false), //  0 of 201
        ] {
            let comp = lr_radial("0", "0")
                .replace("crs:Flipped=\"true\"", &format!("crs:Flipped=\"{flipped}\""))
                .replace("crs:MaskInverted=\"false\"", &format!("crs:MaskInverted=\"{inverted}\""));
            let doc = lr_doc(&lr_correction("Radial 1", "", &comp));
            let r = xmp_to_recipe(&doc);
            let MaskGeometry::Radial { flipped: got_f, .. } = r.masks[0].mask else {
                panic!("expected a radial, got {:?}", r.masks[0].mask);
            };
            assert!(
                !got_f,
                "Flipped={flipped} Inverted={inverted}: crs:Flipped must not reach the render flag"
            );
            assert_eq!(
                r.masks[0].inverted, inverted,
                "Flipped={flipped} Inverted={inverted}: MaskInverted is the inversion"
            );
            assert_eq!(
                lr_net_inverted(&r.masks[0]),
                net,
                "Flipped={flipped} Inverted={inverted}: net inversion"
            );
            // …and OUR writer re-emits the pair Lightroom would have written
            // for that net, so the two rows Lightroom really uses round-trip
            // byte-for-byte and the two it never writes are normalised onto
            // the nearest row it does.
            let xmp = recipe_to_xmp(&r);
            assert!(
                xmp.contains(&format!("crs:MaskInverted=\"{net}\"")),
                "Flipped={flipped} Inverted={inverted}: MaskInverted must carry the net"
            );
            assert!(
                xmp.contains(&format!("crs:Flipped=\"{}\"", !net)),
                "Flipped={flipped} Inverted={inverted}: crs:Flipped is its complement"
            );
            assert_eq!(
                lr_net_inverted(&xmp_to_recipe(&xmp).masks[0]),
                net,
                "Flipped={flipped} Inverted={inverted}: the net survives the round trip"
            );
        }
    }

    /// R25 P9, the other direction: a mask THIS APP flipped (the GUI's Flip
    /// checkbox — `flipped: true`, `inverted: false`) used to export as
    /// `crs:Flipped="false" crs:MaskInverted="false"`, a combination Lightroom
    /// never writes and reads as NOT inverted. The flip was dropped at the
    /// border, silently, in the one direction the user cannot check from
    /// inside this app.
    ///
    /// MUTATION THIS CATCHES: any writer that copies `flipped` into
    /// `crs:Flipped` instead of deriving both attributes from the net.
    #[test]
    fn our_own_flip_leaves_as_lightrooms_own_inversion() {
        for (flipped, inverted) in [(true, false), (false, true), (true, true), (false, false)] {
            let m = LocalAdjustment {
                mask: MaskGeometry::Radial {
                    top: 0.3, left: 0.35, bottom: 0.7, right: 0.65,
                    feather: 0.5, roundness: 0.0, flipped, angle: 0.0,
                    midpoint: 50.0, mask_version: 2,
                },
                inverted,
                exposure_ev: -0.5,
                ..Default::default()
            };
            let net = flipped ^ inverted;
            let xmp = recipe_to_xmp(&EditRecipe { masks: vec![m], ..Default::default() });
            // Both attributes read off the SAME component tag — the pair is
            // the claim, and two whole-document `contains` could each be
            // satisfied by a different component.
            let p = Scope::new(xmp.as_str())
                .find_value_at("What", "Mask/CircularGradient")
                .expect("the radial must be emitted");
            let (s, e, _) = next_xml_tag(&xmp, p).expect("its component tag");
            let tag = Tag::new(&xmp[s..=e]);
            assert_eq!(
                tag.crs_str("MaskInverted").as_deref(),
                Some(if net { "true" } else { "false" }),
                "flipped={flipped} inverted={inverted}: the net must reach crs:MaskInverted"
            );
            assert_eq!(
                tag.crs_str("Flipped").as_deref(),
                Some(if net { "false" } else { "true" }),
                "flipped={flipped} inverted={inverted}: and its complement crs:Flipped — a \
                 matching pair is one Lightroom never writes, and it is what makes this \
                 projection safe under BOTH readings of which attribute Lightroom consults"
            );
            assert_eq!(
                lr_net_inverted(&xmp_to_recipe(&xmp).masks[0]),
                net,
                "flipped={flipped} inverted={inverted}: the rendered result must survive"
            );
        }
    }

    /// The other half of §0: `crs:MaskBlendMode` is on every component
    /// Lightroom writes, and the import refused it unless WE had written the
    /// file. Its default value is the plain composition the engine already
    /// does, so accepting it costs nothing — and a lossless import must
    /// report NO loss, or the disclosure cries wolf on every photo.
    #[test]
    fn a_lightroom_gradient_with_blend_mode_zero_imports_losslessly() {
        let doc = lr_doc(&lr_correction("Gradient 1", "", &lr_gradient("0")));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "a plain Lightroom gradient must import: {:?}", r.masks);
        assert!(import_losses(&doc).is_empty(), "a faithful import says nothing");
        assert_eq!(unsupported_corrections(&doc), 0);
        // …and a mode we cannot reproduce is a NOTE on an imported mask, not
        // a refusal: the shape is still exactly what the file draws.
        // (`lr_gradient` pairs the mode with `MaskValue="1"`; that pair does
        // not occur in the wild — see the next test, which uses the one that
        // does — but it isolates the blend-mode arm on its own.)
        let subtract = lr_doc(&lr_correction("Gradient 1", "", &lr_gradient("1")));
        let r2 = xmp_to_recipe(&subtract);
        assert_eq!(r2.masks.len(), 1, "a Subtract component still has a shape");
        assert_eq!(
            import_losses(&subtract),
            vec![MaskImportLoss {
                name: "Gradient 1".into(),
                reason: MaskImportReason::BlendMode
            }]
        );
    }

    /// v0.31.1: Lightroom's SUBTRACT is the PAIR `crs:MaskBlendMode="1"` +
    /// `crs:MaskValue="0"`, and the second half is an ENCODING, not an opacity.
    ///
    /// EVIDENCE (complete census of the GitHub-code-search-indexed population
    /// of `.xmp` files containing `crs:MaskBlendMode` — 157 files, 479
    /// attribute instances, verified twice, by regex and by
    /// `xml.etree.ElementTree`, 0 parse failures, 2026-08-18):
    /// `MaskBlendMode="1"` co-occurs with `MaskValue="0"` in 26 of 26
    /// instances, and `MaskBlendMode="1"` with `MaskValue="1"` in 0 of 479.
    /// The attribute never sits on the `crs:What="Correction"` element — it is
    /// always a per-component value, which is where this reader looks.
    ///
    /// The importer read that zero as "muted", refused the component, and the
    /// geometry arm turned the refusal into `OutOfModel` — the user's whole
    /// correction, thrown away to avoid a composition we could simply have
    /// disclosed. Now the base shape imports and `BlendMode` names what did
    /// not. The zero is never multiplied into anything; strength comes from
    /// `crs:CorrectionAmount`, which this test also pins.
    ///
    /// MUTATION THIS CATCHES: reading `MaskValue` as an opacity (the mask
    /// arrives at strength 0), and dropping the `subtracted` guard (the whole
    /// correction disappears again).
    #[test]
    fn a_real_lightroom_subtract_component_keeps_its_geometry() {
        // The real pair, on a component this reader has a model for.
        let subtract = lr_radial("0", "1").replace("crs:MaskValue=\"1\"", "crs:MaskValue=\"0\"");
        let doc = lr_doc(&lr_correction("Radial 1", "", &subtract));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "the shape the file draws must survive: {:?}", r.masks);
        assert_eq!(unsupported_corrections(&doc), 0, "and the correction is not counted lost");
        assert_eq!(
            import_losses(&doc),
            vec![MaskImportLoss {
                name: "Radial 1".into(),
                reason: MaskImportReason::BlendMode
            }],
            "exactly one note: the composition, not the geometry"
        );
        // The geometry is the file's own, and the zero did NOT become a
        // strength: `CorrectionAmount="1"` is still the master opacity.
        let MaskGeometry::Radial { top, left, .. } = r.masks[0].mask else {
            panic!("expected a radial, got {:?}", r.masks[0].mask);
        };
        // The file's own `(0.114928, 0.590368)`, VERBATIM: the frame affine is
        // the identity since the 2026-08-19 `LR_MASK_FRAME_SCALE = 1.0` ruling
        // (see `a_lightroom_radial_with_angle_imports`).
        assert!(
            (top as f64 - 0.114928).abs() < 1e-7 && (left as f64 - 0.590368).abs() < 1e-7,
            "the real coordinates arrived: {top} {left}"
        );
        assert_eq!(r.masks[0].amount, 1.0, "MaskValue=0 is an encoding, never a pre-multiplier");
        assert_eq!(r.masks[0].contrast, 43.0, "and the sliders are untouched by it");

        // The guard is a PAIR. A zero MaskValue with the DEFAULT blend mode is
        // the muted component it always was, and still refuses.
        let muted = lr_radial("0", "0").replace("crs:MaskValue=\"1\"", "crs:MaskValue=\"0\"");
        let muted_doc = lr_doc(&lr_correction("Radial 1", "", &muted));
        assert!(xmp_to_recipe(&muted_doc).masks.is_empty(), "a muted component still refuses");
        assert_eq!(unsupported_corrections(&muted_doc), 1);

        // …and so does a genuinely PARTIAL value, blend mode or not — that is
        // coverage we can read and have no model for. (0.662178 is the one
        // non-0/1 MaskValue in the whole 479-instance census.)
        for blend in ["0", "1"] {
            let partial = lr_radial("0", blend)
                .replace("crs:MaskValue=\"1\"", "crs:MaskValue=\"0.662178\"");
            let partial_doc = lr_doc(&lr_correction("Radial 1", "", &partial));
            assert!(
                xmp_to_recipe(&partial_doc).masks.is_empty(),
                "MaskBlendMode={blend}: a partial MaskValue is not the subtract encoding"
            );
        }
    }

    /// v0.31.1: `crs:Roundness` is Lightroom's ±100 integer slider, so a user
    /// who moved it no longer loses the mask.
    ///
    /// EVIDENCE (direct observation of the harvested real-sidecar corpus,
    /// 2026-08-18): all 24 `Mask/CircularGradient` components write Roundness
    /// as a bare signed integer, every one at its default `0`, beside
    /// `Feather="+100"` and `Midpoint="+50"` — integers on a 0..100-style
    /// footing, not 0..1 reals. The importer's old `(0.0..=1.0)` gate was the
    /// "bbox aspect ratio" reading of the field, and being a GEOMETRY check it
    /// refused the entire correction.
    ///
    /// The value is CARRIED, not converted (`mask_weight` never reads it), so
    /// the ambiguous `1` needs no ruling: whatever scale it was written on, it
    /// is written back as `1`. That is the difference from `feather`, which is
    /// rendered and therefore must guess.
    #[test]
    fn a_lightroom_roundness_slider_is_carried_not_refused() {
        let with = |v: &str| {
            lr_doc(&lr_correction(
                "Radial 1",
                "",
                &lr_radial("0", "0").replace("crs:Roundness=\"0\"", &format!("crs:Roundness=\"{v}\"")),
            ))
        };
        // Both signs of the real slider, the ambiguous 1, and a value only our
        // own legacy writer could have produced.
        for (text, want) in [("-30", -30.0), ("+45", 45.0), ("1", 1.0), ("0.25", 0.25)] {
            let doc = with(text);
            let r = xmp_to_recipe(&doc);
            assert_eq!(r.masks.len(), 1, "Roundness={text} must not cost the mask: {:?}", r.masks);
            assert!(import_losses(&doc).is_empty(), "Roundness={text}: carrying it loses nothing");
            let MaskGeometry::Radial { roundness, .. } = r.masks[0].mask else {
                panic!("expected a radial, got {:?}", r.masks[0].mask);
            };
            assert_eq!(roundness, want, "Roundness={text}: carried verbatim, never rescaled");
            // …and it goes back out as the same number, through the clamp.
            let back = xmp_to_recipe(&recipe_to_xmp(&r));
            let MaskGeometry::Radial { roundness: round2, .. } = back.masks[0].mask else {
                panic!("expected a radial, got {:?}", back.masks[0].mask);
            };
            assert_eq!(round2, want, "Roundness={text} did not survive our own writer");
        }
        // The gate still has ends: past the slider's own range is unreadable
        // geometry, and that IS a refusal.
        let wild = with("101");
        assert!(xmp_to_recipe(&wild).masks.is_empty(), "101 is off the Lightroom slider");
        assert_eq!(unsupported_corrections(&wild), 1);
    }

    /// A `crs:Local*` slider this engine has no model for used to cost the
    /// user the whole mask. It is a knob, not a coverage change: the shape
    /// and the fifteen sliders we DO model are still exactly the file's.
    #[test]
    fn a_nonzero_inert_local_no_longer_drops_the_whole_correction() {
        let doc = lr_doc(
            &lr_correction("Radial 1", "", &lr_radial("0", "0"))
                .replace("crs:LocalDefringe=\"0\"", "crs:LocalDefringe=\"0.3\""),
        );
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "the mask imports: {:?}", r.masks);
        assert_eq!(r.masks[0].contrast, 43.0, "the modelled sliders came through");
        assert_eq!(
            import_losses(&doc),
            vec![MaskImportLoss {
                name: "Radial 1".into(),
                reason: MaskImportReason::InertLocal("LocalDefringe")
            }],
            "the slider that did not come through is named"
        );
        assert_eq!(unsupported_corrections(&doc), 0);
    }

    /// A `crs:Local*` attribute this build has never seen: same rule, and the
    /// loss carries the CORRECTION's name so the sentence is actionable.
    /// Also pins the two notes stacking on one correction.
    #[test]
    fn unknown_local_key_names_itself() {
        let doc = lr_doc(&lr_correction(
            "Sky",
            "       crs:LocalWhatsit=\"0.5\"\n",
            &lr_radial("0", "0"),
        ))
        .replace("crs:LocalCurveRefineSaturation=\"100\"", "crs:LocalCurveRefineSaturation=\"80\"");
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "an unknown knob is not a reason to lose the mask");
        let losses = import_losses(&doc);
        assert_eq!(losses.len(), 2, "both notes are raised: {losses:?}");
        assert!(losses.iter().all(|l| l.name == "Sky"), "each names the correction: {losses:?}");
        assert!(
            losses.iter().any(|l| l.reason == MaskImportReason::UnknownLocalKey),
            "{losses:?}"
        );
        assert!(
            losses.iter().any(|l| l.reason == MaskImportReason::CurveRefineSaturation),
            "{losses:?}"
        );
    }

    /// R25 P1 round-end. `crs:LocalCorrectedDepth`, `crs:LocalInputDigest`
    /// and `crs:LocalInputDigestVersion` are Lightroom's own BOOKKEEPING —
    /// a numeric flag and a recompute ledger (32-hex digest + schema version)
    /// — and they rode into `UnknownLocalKey`, whose label reads "unmodelled
    /// slider". That sentence was wrong about the thing AND wrong about how
    /// often: 12 notes across the reference library's 31 importable
    /// corrections, on files whose sliders all came through intact.
    ///
    /// Knowing a key and not modelling it is the honest answer for all three.
    /// The numeric one keeps the inert-key law all the same — silent at its
    /// observed 0, NAMED at anything else — while the two string keys stay out
    /// of `INERT_LOCAL`, because `optional_number_is` cannot parse a hex
    /// digest and would raise a false note on every file that carries one.
    #[test]
    fn lightroom_bookkeeping_keys_are_known_not_unmodelled_sliders() {
        // The shapes are Lightroom's, the digest is a neutral test value: a
        // real digest is a hash OF the user's own file (fixture policy).
        let ledger = "       crs:LocalCorrectedDepth=\"0\"\n\
                      \x20      crs:LocalInputDigest=\"0000000000000000000000000000002A\"\n\
                      \x20      crs:LocalInputDigestVersion=\"1\"\n";
        let doc = lr_doc(&lr_correction("Sky", ledger, &lr_radial("0", "0")));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "premise: the correction imports at all");
        assert!(
            import_losses(&doc).is_empty(),
            "Lightroom's own bookkeeping is not a loss: {:?}",
            import_losses(&doc)
        );

        // The inert-key law still holds for the numeric one. MUTATION THIS
        // CATCHES: adding it to `KNOWN_LOCAL` and NOT to `INERT_LOCAL` makes
        // a non-zero value silent, which is the opposite failure.
        let moved = doc.replace("crs:LocalCorrectedDepth=\"0\"", "crs:LocalCorrectedDepth=\"0.5\"");
        assert_eq!(
            import_losses(&moved),
            vec![MaskImportLoss {
                name: "Sky".into(),
                reason: MaskImportReason::InertLocal("LocalCorrectedDepth")
            }],
            "a bookkeeping flag off its observed value is named like any other inert key"
        );
        assert_eq!(xmp_to_recipe(&moved).masks.len(), 1, "and it still costs no mask");

        // The two STRING keys can never be read as numbers — if either were
        // in `INERT_LOCAL`, this document would raise a note for a value that
        // is exactly what Lightroom writes.
        assert!(
            import_losses(&doc.replace(
                "crs:LocalInputDigestVersion=\"1\"",
                "crs:LocalInputDigestVersion=\"2\""
            ))
            .is_empty(),
            "a digest ledger is not a slider at any value"
        );
    }

    /// THE TRAP OF THIS BATCH (data-corruption class). Once a lossy sidecar
    /// imports, `r.masks.is_empty()` stops meaning "the user has not touched
    /// these" — and the merge used that emptiness to decide whether to keep
    /// the base's own mask block. Left alone, an ordinary Ctrl+S would have
    /// written our DEGRADED reading (rotation read as 0, blend mode ignored,
    /// `crs:Midpoint` / `crs:Version` not even read) over the user's own
    /// Lightroom block, silently.
    ///
    /// The whole round trip the app really takes is exercised, not just the
    /// merge: import → serde_json → back (recipe.json is a file, and f32 that
    /// does not survive the text round trip would make the equality fail on
    /// the user's second launch, not in a unit test) → merge → the base's
    /// mask block must come out byte-for-byte.
    #[test]
    fn preserve_masks_survives_a_lossy_import_the_user_did_not_touch() {
        let doc = lr_doc(&format!(
            "{}{}",
            lr_correction("Radial 1", "", &lr_radial("37.412506", "0")),
            lr_correction("Gradient 1", "", &lr_gradient("1")),
        ));
        let imported = xmp_to_recipe(&doc);
        assert_eq!(imported.masks.len(), 2, "premise: the masks really did import");
        assert_eq!(import_losses(&doc).len(), 2, "premise: the import really was lossy");

        // recipe.json in the middle, exactly as the app stores it.
        let json = serde_json::to_string(&imported).expect("serialise");
        let reloaded: EditRecipe = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(
            reloaded.masks, imported.masks,
            "an f32 that does not survive recipe.json would silently arm the overwrite"
        );

        let start = doc.find("<crs:MaskGroupBasedCorrections>").expect("block");
        let end = doc.find("</crs:MaskGroupBasedCorrections>").expect("block close")
            + "</crs:MaskGroupBasedCorrections>".len();
        let original = &doc[start..end];
        let out = merge_recipe_into_xmp(&doc, &reloaded).expect("mergeable");
        assert!(
            out.doc.contains(original),
            "the user's own mask block must survive an untouched save VERBATIM:\n{}",
            out.doc
        );
        assert!(
            out.doc.contains("crs:Angle=\"37.412506\"") && out.doc.contains("crs:Midpoint=\"50\""),
            "the parts we cannot express are exactly the parts this rule protects"
        );
        assert!(
            out.notes.is_empty(),
            "nothing was replaced, so nothing to disclose: {:?}",
            out.notes
        );
        // Our own projection is NOT prepended beside the base's block — one
        // document, two mask groups was the other way to get this wrong.
        assert_eq!(
            out.doc.matches("<crs:MaskGroupBasedCorrections>").count(),
            1,
            "exactly one mask group in the output"
        );
    }

    /// A `recipe.json` in the shape v0.30 wrote them: today's serialisation
    /// MINUS `schema_era` and minus every field R25 added a `crs:` key for.
    /// Built by deletion rather than by hand so the fixture cannot quietly
    /// stop being a subset of what the app really writes — and read back
    /// through the real serde path, because the whole point is what the FIELD
    /// DEFAULTS do with an absent key.
    fn as_v0_30_recipe(r: &EditRecipe) -> EditRecipe {
        let mut v = serde_json::to_value(r).expect("serialise");
        let obj = v.as_object_mut().expect("a recipe is an object");
        assert!(obj.remove("schema_era").is_some(), "the era stamp must have been there to remove");
        for (name, _) in r25_attr_keys() {
            assert!(obj.remove(name).is_some(), "{name} is a recipe field");
        }
        serde_json::from_value(v).expect("deserialise")
    }

    /// A Lightroom sidecar carrying the R25 globals and no masks — the shape
    /// the B2 / B3 keys actually arrive in (values from the reference
    /// library: a negative Texture, the one signed decimal in the detail
    /// block, a real post-crop vignette, a grain triple, and the de-fringe
    /// block at Adobe's own defaults with one non-zero amount).
    fn lr_globals_doc() -> String {
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 5.6-c145\">\n\
         \x20<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
         \x20 <rdf:Description rdf:about=\"\"\n\
         \x20   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
         \x20   crs:Version=\"15.5.1\"\n\
         \x20   crs:ProcessVersion=\"15.4\"\n\
         \x20   crs:Exposure2012=\"+0.35\"\n\
         \x20   crs:Texture=\"-20\"\n\
         \x20   crs:SharpenRadius=\"+1.0\"\n\
         \x20   crs:SharpenDetail=\"25\"\n\
         \x20   crs:PostCropVignetteAmount=\"-17\"\n\
         \x20   crs:PostCropVignetteMidpoint=\"50\"\n\
         \x20   crs:GrainAmount=\"30\"\n\
         \x20   crs:GrainSize=\"25\"\n\
         \x20   crs:GrainFrequency=\"50\"\n\
         \x20   crs:DefringePurpleAmount=\"3\"\n\
         \x20   crs:DefringePurpleHueLo=\"30\"\n\
         \x20   crs:DefringePurpleHueHi=\"70\"\n\
         \x20   crs:DefringeGreenAmount=\"0\"\n\
         \x20   crs:DefringeGreenHueLo=\"40\"\n\
         \x20   crs:DefringeGreenHueHi=\"60\"\n\
         \x20   crs:HasSettings=\"True\"/>\n\
         \x20</rdf:RDF>\n\
         </x:xmpmeta>\n"
            .to_string()
    }

    /// **THE R25 P8 TRAP, mask half** (data-destruction class), and the exact
    /// scenario measured on the reference library: DSC09034 lost four
    /// corrections and DSC09642 lost eight, with an empty note list.
    ///
    /// P1 made Lightroom's own masks import CLEANLY, and the merge's preserve
    /// arm was keyed on a `MaskSummary::preserve_original` flag that only a
    /// LOSSY import ever set (it is gone now; `defects > 0` is what it meant). A v0.30 `recipe.json` is maskless by construction (that
    /// build could not import one), so the pair "clean base + maskless
    /// recipe" answered "nothing to preserve", and an ordinary Ctrl+S stripped
    /// the block and wrote nothing in its place.
    ///
    /// MUTATION THIS CATCHES: put the old flag's condition —
    /// `summary.defects > 0`, which is exactly when `preserve_original` was
    /// raised — back into either the preserve or the note test, and this goes
    /// red on both assertions at once: the block vanishes AND nothing says so.
    #[test]
    fn a_clean_lightroom_mask_block_survives_a_recipe_that_never_saw_it() {
        let doc = lr_doc(&format!(
            "{}{}",
            lr_correction("Radial 1", "", &lr_radial("0", "0")),
            lr_correction("Gradient 1", "", &lr_gradient("0")),
        ));
        let imported = xmp_to_recipe(&doc);
        assert_eq!(imported.masks.len(), 2, "premise: the masks import");
        assert!(
            import_losses(&doc).is_empty(),
            "premise: and they import CLEANLY — that is what broke the old flag: {:?}",
            import_losses(&doc)
        );

        // The v0.30 recipe.json beside that sidecar: no masks, no era stamp.
        let legacy = EditRecipe { masks: Vec::new(), ..as_v0_30_recipe(&imported) };
        assert_eq!(legacy.schema_era, 0, "an absent key is what makes it legacy");

        let start = doc.find("<crs:MaskGroupBasedCorrections>").expect("block");
        let end = doc.find("</crs:MaskGroupBasedCorrections>").expect("block close")
            + "</crs:MaskGroupBasedCorrections>".len();
        let original = &doc[start..end];
        let out = merge_recipe_into_xmp(&doc, &legacy).expect("mergeable");
        assert!(
            out.doc.contains(original),
            "the photographer's own mask block must survive VERBATIM:\n{}",
            out.doc
        );
        assert_eq!(
            out.doc.matches("<crs:MaskGroupBasedCorrections>").count(),
            1,
            "exactly one mask group in the output"
        );
        assert!(out.notes.is_empty(), "nothing was replaced: {:?}", out.notes);
    }

    /// The disclosure half of the same rule, on a base the importer
    /// understands COMPLETELY. Before P8 this arm could not be reached at all
    /// (the note was gated on that same lossy-import flag), so replacing perfectly
    /// readable Lightroom corrections was a silent success — and the sentence
    /// itself had to change, because "carries 0 thing(s) this build cannot
    /// represent" is what the old wording says about a clean block.
    #[test]
    fn replacing_a_clean_mask_block_names_what_it_replaced() {
        let doc = lr_doc(&lr_correction("Radial 1", "", &lr_radial("0", "0")));
        assert!(import_losses(&doc).is_empty(), "premise: a clean base");
        let mut r = xmp_to_recipe(&doc);
        r.masks[0].exposure_ev = 1.25; // the user moved it: the develop is newest
        let out = merge_recipe_into_xmp(&doc, &r).expect("mergeable");
        assert_eq!(out.notes.len(), 1, "the replacement is disclosed: {:?}", out.notes);
        assert!(
            out.notes[0].contains("1 correction(s)")
                && !out.notes[0].contains("0 thing(s)")
                && out.notes[0].contains("1 edited mask(s)"),
            "the note counts what was there, not a defect count of zero: {}",
            out.notes[0]
        );
    }

    /// **THE R25 P8 TRAP, globals half** (data-destruction class): three of the
    /// seven reference sidecars lost nine keys each to this, silently.
    ///
    /// Owning a key means the merge STRIPS it and the writer puts ours back —
    /// and the writer omits a slider at rest. A v0.30 `recipe.json` has no
    /// field for any of the twenty-seven keys R25 added, so serde fills them
    /// from `EditRecipe::default()` and the recipe "says" texture 0, no grain,
    /// no radius. Stripping on that reading deleted `crs:Texture="-20"`, the
    /// whole Grain block and the PostCrop / SharpenRadius keys out of the
    /// photographer's Lightroom file on an ordinary Ctrl+S.
    ///
    /// MUTATION THIS CATCHES: return an empty set from
    /// `era_suppressed_attr_keys` (or drop its `schema_era` test) and every
    /// value below goes to the writer's default.
    #[test]
    fn a_v0_30_recipe_does_not_strip_the_keys_it_never_had() {
        let doc = lr_globals_doc();
        let imported = xmp_to_recipe(&doc);
        assert_eq!(imported.texture, -20.0, "premise: the fixture really carries them");
        assert_eq!((imported.grain, imported.sharpen_radius), (30.0, 1.0));

        let legacy = as_v0_30_recipe(&imported);
        assert_eq!(legacy.schema_era, 0);
        assert_eq!(legacy.texture, 0.0, "premise: serde filled the absent field with the default");

        let out = merge_recipe_into_xmp(&doc, &legacy).expect("mergeable");
        for spelling in [
            "crs:Texture=\"-20\"",
            "crs:SharpenRadius=\"+1.0\"",
            "crs:SharpenDetail=\"25\"",
            "crs:PostCropVignetteAmount=\"-17\"",
            "crs:PostCropVignetteMidpoint=\"50\"",
            "crs:GrainAmount=\"30\"",
            "crs:GrainSize=\"25\"",
            "crs:GrainFrequency=\"50\"",
            "crs:DefringePurpleAmount=\"3\"",
        ] {
            assert!(out.doc.contains(spelling), "{spelling} was deleted from the user's file");
        }
        // Suppressing the strip WITHOUT suppressing the write is the other way
        // to get this wrong: one tag, two answers.
        for key in ["Texture", "GrainAmount", "DefringePurpleAmount", "DefringePurpleHueLo"] {
            assert_eq!(
                out.doc.matches(&format!("crs:{key}=")).count(),
                1,
                "crs:{key} must appear exactly once"
            );
        }
        assert_eq!(xmp_to_recipe(&out.doc).texture, -20.0, "…and it reads back as itself");
        assert!(out.notes.is_empty(), "nothing was lost, so nothing to disclose: {:?}", out.notes);
    }

    /// The CONTROL for the test above, and the reason the gate is an era stamp
    /// rather than a new policy: a CURRENT-era recipe that says texture 0 is
    /// STATING a value, and the merge must still publish it over the base's.
    /// Whatever else this batch changed, it did not change what a save means.
    #[test]
    fn a_current_era_recipe_still_owns_every_key_it_states() {
        let doc = lr_globals_doc();
        let mut r = xmp_to_recipe(&doc);
        assert_eq!(r.schema_era, crate::recipe::SCHEMA_ERA, "an import is current-era");
        r.texture = 0.0;
        r.grain = 0.0;
        let out = merge_recipe_into_xmp(&doc, &r).expect("mergeable");
        assert!(out.doc.contains("crs:Texture=\"0\""), "the cleared slider publishes: {}", out.doc);
        assert!(!out.doc.contains("crs:GrainAmount="), "a zero grain is an omitted key");
        assert_eq!(xmp_to_recipe(&out.doc).texture, 0.0);
    }

    /// The gate is PER KEY, and this is the case that forces it: a legacy
    /// recipe whose Texture the user has just dragged. Nothing ever re-stamps
    /// a file's era, so a whole-recipe gate would mean a v0.30 photo could
    /// never write Texture to its sidecar again — the same silent divergence
    /// class this round is closing, re-introduced by the fix for it.
    #[test]
    fn an_edited_key_leaves_the_era_gate_even_on_a_legacy_recipe() {
        let doc = lr_globals_doc();
        let mut legacy = as_v0_30_recipe(&xmp_to_recipe(&doc));
        legacy.texture = 20.0; // the user moved THIS slider and nothing else
        let out = merge_recipe_into_xmp(&doc, &legacy).expect("mergeable");
        assert!(out.doc.contains("crs:Texture=\"+20\""), "the edit reaches the file: {}", out.doc);
        assert_eq!(out.doc.matches("crs:Texture=").count(), 1, "and only once");
        // Its untouched neighbours are still protected — the gate did not open
        // for the whole recipe.
        assert!(out.doc.contains("crs:GrainAmount=\"30\""), "the grain block stands");
    }

    /// The de-fringe six move as ONE block or not at all: the writer emits all
    /// six unconditionally because a hue window with no amount beside it is a
    /// shape no real document has, and a per-key gate that released three of
    /// them would publish exactly that.
    #[test]
    fn the_era_gate_releases_the_de_fringe_block_whole() {
        let doc = lr_globals_doc();
        let mut legacy = as_v0_30_recipe(&xmp_to_recipe(&doc));
        legacy.defringe_purple = 5.0; // one key of the six
        let out = merge_recipe_into_xmp(&doc, &legacy).expect("mergeable");
        for key in [
            "DefringePurpleAmount",
            "DefringePurpleHueLo",
            "DefringePurpleHueHi",
            "DefringeGreenAmount",
            "DefringeGreenHueLo",
            "DefringeGreenHueHi",
        ] {
            assert_eq!(
                out.doc.matches(&format!("crs:{key}=")).count(),
                1,
                "crs:{key} must be written exactly once when the block moves"
            );
        }
        assert!(out.doc.contains("crs:DefringePurpleAmount=\"5\""), "{}", out.doc);
    }

    /// The era gate's universe, DERIVED and pinned: exactly the twenty-seven
    /// attribute keys R25 gave this writer. A hand-copied list would drift;
    /// this asserts the derivation produces the list, so a new `CarriedOnly`
    /// row arrives inside the gate and a row promoted OUT of the tier leaves
    /// it — with the count as the tripwire either way.
    #[test]
    fn the_era_gate_is_the_twenty_seven_keys_r25_added() {
        let mut keys: Vec<&str> = r25_attr_keys().into_iter().map(|(_, k)| k).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "AutoLateralCA",
                "ChromaticAberrationB",
                "ChromaticAberrationR",
                "ColorNoiseReduction",
                "ColorNoiseReductionDetail",
                "ColorNoiseReductionSmoothness",
                "DefringeGreenAmount",
                "DefringeGreenHueHi",
                "DefringeGreenHueLo",
                "DefringePurpleAmount",
                "DefringePurpleHueHi",
                "DefringePurpleHueLo",
                "GrainAmount",
                "GrainFrequency",
                "GrainSize",
                "LuminanceNoiseReductionContrast",
                "LuminanceNoiseReductionDetail",
                "PostCropVignetteAmount",
                "PostCropVignetteFeather",
                "PostCropVignetteHighlightContrast",
                "PostCropVignetteMidpoint",
                "PostCropVignetteRoundness",
                "PostCropVignetteStyle",
                "SharpenDetail",
                "SharpenEdgeMasking",
                "SharpenRadius",
                "Texture",
            ]
        );
        // Every one of them is a key this writer OWNS — a gated key the merge
        // never strips anyway would be a rule about nothing.
        let owned = owned_attr_keys();
        for k in &keys {
            assert!(owned.contains(&(*k).to_string()), "{k} is not an owned attribute");
        }
        // And the gate really is EMPTY for a current-era recipe: the ordinary
        // save path pays nothing and changes nothing.
        assert!(era_suppressed_attr_keys(&EditRecipe::default()).is_empty());
        assert_eq!(
            era_suppressed_attr_keys(&EditRecipe { schema_era: 0, ..Default::default() }).len(),
            27,
            "an untouched legacy recipe suppresses all twenty-seven"
        );
    }

    /// R25 P8, the READ / WRITE asymmetry: a document whose top-level child
    /// opens and closes under DIFFERENT names balances its tag counts but
    /// crosses its names. `top_level_owned_spans` (the writer's strip) does
    /// not even notice — it tracks OWNED children only — so the merge went
    /// ahead, while `crs_scope_inner` bailed, and a bailed scope hands every
    /// scanner the WHOLE document: the creative Look's baked `crs:Clarity2012`
    /// was then read as the photographer's own slider, which is the one thing
    /// the scope function exists to prevent.
    #[test]
    fn a_crossed_name_look_is_dropped_from_the_scope_not_promoted_by_it() {
        let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core\">\n\
             \x20<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             \x20 <rdf:Description rdf:about=\"\"\n\
             \x20   xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
             \x20   crs:Exposure2012=\"+0.35\"\n\
             \x20   crs:HasSettings=\"True\">\n\
             \x20  <crs:Look>\n\
             \x20   <rdf:Description>\n\
             \x20    <crs:Clarity2012>+50</crs:Clarity2012>\n\
             \x20   </rdf:Description>\n\
             \x20  </crs:Foo>\n\
             \x20 </rdf:Description>\n\
             \x20</rdf:RDF>\n\
             </x:xmpmeta>\n";
        let r = xmp_to_recipe(doc);
        assert_eq!(r.exposure_ev, 0.35, "the top level's own settings still import");
        assert_eq!(
            r.clarity, 0.0,
            "the Look's baked clarity is NOT this photographer's slider: {doc}"
        );
        // The writer really does go ahead on this document — which is what
        // made the reader's bail an asymmetry rather than a shared refusal.
        assert!(
            merge_recipe_into_xmp(doc, &EditRecipe::default()).is_some(),
            "premise: the merge does not refuse this shape"
        );
    }

    /// The other side of the same rule: the moment the user edits a mask, the
    /// develop in hand IS the newest intent, so it publishes — and the base's
    /// block going is a disclosed note, never a silence.
    #[test]
    fn an_edited_mask_overwrites_and_says_so() {
        let doc = lr_doc(&lr_correction("Radial 1", "", &lr_radial("37.412506", "0")));
        let mut r = xmp_to_recipe(&doc);
        r.masks[0].exposure_ev = 1.25;
        let out = merge_recipe_into_xmp(&doc, &r).expect("mergeable");
        assert!(
            !out.doc.contains("crs:Angle=\"37.412506\""),
            "the develop's own projection replaced the block: {}",
            out.doc
        );
        assert_eq!(out.notes.len(), 1, "the replacement is disclosed: {:?}", out.notes);
        assert!(
            out.notes[0].contains("1 thing(s)") && out.notes[0].contains("1 edited mask(s)"),
            "the note names both counts: {}",
            out.notes[0]
        );
        assert_eq!(
            xmp_to_recipe(&out.doc).masks[0].exposure_ev,
            1.25,
            "and the edit is what the file now says"
        );
    }

    /// R25 P9. A correction with several geometry components imports ONE shape
    /// and discloses the rest, and the one it takes must be the BASE — the
    /// first component in the file, which is what Lightroom's Add/Subtract
    /// stack composes onto. It used to be decided by KIND instead: the reader
    /// tried `Mask/Gradient` before `Mask/CircularGradient`, so a trailing
    /// linear beat a leading radial and the imported mask was a shape the
    /// correction merely happened to also contain.
    ///
    /// Measured on the user's library, not invented: `DSC08960` 蒙版 5 is
    /// `[CircularGradient, CircularGradient, Gradient]` and imported as that
    /// trailing LINEAR with both radials gone; `_DSC9583` Mask 9 is
    /// `[CircularGradient, Gradient]` and did the same. The loss was DISCLOSED
    /// throughout (`MultiComponent`, and the dropped radials' own
    /// `Rotation(…)` notes) — so this was never the silent drop it looked
    /// like from the recipe alone — but the surviving shape was the wrong one.
    ///
    /// MUTATION THIS CATCHES: restoring the kind-ordered `if let` chain flips
    /// row 1 back to a linear; ignoring `crs:MaskBlendMode` in the selector
    /// flips rows 3 and 4, the two that are inversions of intent rather than
    /// truncations of it.
    #[test]
    fn a_multi_component_correction_imports_its_base_geometry() {
        // A subtract component, spelled the way Lightroom spells it (the pair
        // v0.31.1 taught this reader to read: mode "1" WITH MaskValue "0").
        let subtract = |c: String| {
            c.replace("crs:MaskValue=\"1\"", "crs:MaskValue=\"0\"")
        };
        let radial = || lr_radial("0", "0");
        for (label, comps, want_radial) in [
            // Kind order used to decide: a TRAILING linear beat a leading
            // radial. `DSC08960` 蒙版 5's real structure, all three at the
            // default blend mode — a plain union.
            ("radial, radial, linear (union)", vec![radial(), radial(), lr_gradient("0")], true),
            ("linear, radial (union)", vec![lr_gradient("0"), radial()], false),
            // Blend mode decides over document order: `DSC08960` 蒙版 3 and
            // `_DSC9583` Mask 9 are both a base radial + a SUBTRACT linear, and
            // the importer kept the shape Lightroom carves away WITH.
            ("radial base, linear subtract", vec![radial(), subtract(lr_gradient("1"))], true),
            ("linear subtract, radial base", vec![subtract(lr_gradient("1")), radial()], true),
            // Nothing but subtractions has no base to find, so the first
            // component stands in — some shape beats no shape.
            (
                "all subtract",
                vec![subtract(lr_gradient("1")), subtract(lr_radial("0", "1"))],
                false,
            ),
        ] {
            let doc = lr_doc(&lr_correction("Stacked 1", "", &comps.concat()));
            let r = xmp_to_recipe(&doc);
            assert_eq!(r.masks.len(), 1, "{label}: one correction imports one shape");
            let got_radial = matches!(r.masks[0].mask, MaskGeometry::Radial { .. });
            assert_eq!(
                got_radial, want_radial,
                "{label}: wrong base — got {:?}",
                r.masks[0].mask
            );
            // …and the components left behind are still named, which is the
            // half of the contract that was already working.
            let losses = import_losses(&doc);
            assert!(
                losses.iter().any(|l| l.reason == MaskImportReason::MultiComponent),
                "{label}: the dropped component must be disclosed: {losses:?}"
            );
        }
    }

    /// R25 P9, Fix B — the disclosure has to describe the shape that ARRIVED.
    /// `Rotation` says "radial rotation(s) read as 0", and
    /// `classify_correction` collected it from EVERY geometry component: on
    /// `DSC08960` three of the four rotation notes named radials that never
    /// entered the recipe (蒙版 5 contributed two by itself). What covers a
    /// dropped shape is `MultiComponent`, which says exactly that.
    ///
    /// `BlendMode` deliberately stays component-wide — see the filter's own
    /// comment: on a dropped subtract component the sentence is true, and it is
    /// the v0.31.1 disclosure for precisely that case.
    ///
    /// MUTATION THIS CATCHES: dropping the filter puts the unimported radial's
    /// rotation back; widening it to `BlendMode` silences the subtract note.
    #[test]
    fn only_the_imported_geometrys_rotation_is_disclosed() {
        // Base = an UNROTATED linear; the dropped radial carries the angle.
        let comps = format!("{}{}", lr_gradient("0"), lr_radial("37.412506", "0"));
        let doc = lr_doc(&lr_correction("Stacked 1", "", &comps));
        let reasons: Vec<_> = import_losses(&doc).into_iter().map(|l| l.reason).collect();
        assert!(
            matches!(r_kind(&doc), MaskKindForTest::Linear),
            "the unrotated linear is the base here"
        );
        assert!(
            !reasons.iter().any(|r| matches!(r, MaskImportReason::Rotation(_))),
            "a dropped radial's rotation must not be reported as read: {reasons:?}"
        );
        assert!(
            reasons.contains(&MaskImportReason::MultiComponent),
            "the dropped shape is disclosed as a dropped shape: {reasons:?}"
        );
        // The mirror: when the ROTATED radial is the base, the note is true and
        // must still fire.
        let comps = format!("{}{}", lr_radial("37.412506", "0"), lr_gradient("0"));
        let doc = lr_doc(&lr_correction("Stacked 1", "", &comps));
        let reasons: Vec<_> = import_losses(&doc).into_iter().map(|l| l.reason).collect();
        assert!(
            reasons.contains(&MaskImportReason::Rotation(37)),
            "the imported radial's own rotation is a real loss: {reasons:?}"
        );
        // And a dropped SUBTRACT component keeps its own note (v0.31.1).
        let comps = format!(
            "{}{}",
            lr_radial("0", "0"),
            lr_gradient("1").replace("crs:MaskValue=\"1\"", "crs:MaskValue=\"0\"")
        );
        let doc = lr_doc(&lr_correction("Stacked 1", "", &comps));
        let reasons: Vec<_> = import_losses(&doc).into_iter().map(|l| l.reason).collect();
        assert!(
            reasons.contains(&MaskImportReason::BlendMode),
            "a dropped subtract component is still a composition we could not do: {reasons:?}"
        );
    }

    enum MaskKindForTest {
        Linear,
        Other,
    }

    fn r_kind(doc: &str) -> MaskKindForTest {
        match xmp_to_recipe(doc).masks.first().map(|m| &m.mask) {
            Some(MaskGeometry::Linear { .. }) => MaskKindForTest::Linear,
            _ => MaskKindForTest::Other,
        }
    }

    /// R25 P1, the import twin of `mask_loss_reason_all_covers_every_variant`:
    /// both disclosure surfaces ITERATE `MaskImportReason::ALL` (here and the
    /// GUI's `xmp_import_line`), so the list is the one place a reason can be
    /// forgotten — and the match below is where a new variant stops the build.
    #[test]
    fn import_loss_reasons_all_reach_the_prose() {
        // Adding a variant makes THIS match non-exhaustive; the arm you write
        // carries the next rank, and the asserts fail until `ALL` lists the
        // newcomer in that position.
        fn rank(r: MaskImportReason) -> usize {
            match r {
                MaskImportReason::Unrepresentable => 0,
                MaskImportReason::OutOfModel => 1,
                MaskImportReason::Rotation(_) => 2,
                MaskImportReason::BlendMode => 3,
                MaskImportReason::MultiComponent => 4,
                MaskImportReason::BrushRendered => 5,
                MaskImportReason::AiMaskRecomputed => 6,
                MaskImportReason::AiMaskUnresolved => 7,
                MaskImportReason::ForeignRangeMask => 8,
                MaskImportReason::LocalCurve => 9,
                MaskImportReason::CurveRefineSaturation => 10,
                MaskImportReason::InertLocal(_) => 11,
                MaskImportReason::UnknownLocalKey => 12,
            }
        }
        for (i, r) in MaskImportReason::ALL.into_iter().enumerate() {
            assert_eq!(rank(r), i, "ALL must list every reason once, in rank order");
            assert!(!r.en().trim().is_empty(), "{r:?} has no label for the prose channel");
            assert!(r.same_kind(r), "same_kind must be reflexive for {r:?}");
        }
        // The payload variantS group by KIND, not by value — the property the
        // prose channels rely on to print one line for two sliders, and (since
        // R25 P5) one line for two differently-tilted radials.
        assert!(
            MaskImportReason::InertLocal("LocalGrain")
                .same_kind(MaskImportReason::InertLocal("LocalMoire")),
            "two unmodelled sliders are one line"
        );
        assert!(
            MaskImportReason::Rotation(37).same_kind(MaskImportReason::Rotation(-44)),
            "two tilted radials are one line"
        );
        assert!(
            !MaskImportReason::Rotation(0).same_kind(MaskImportReason::BlendMode),
            "different variants are different lines"
        );
        // Exactly the two drop verdicts, and they are the ones
        // `unsupported_corrections` counts.
        assert_eq!(
            MaskImportReason::ALL.into_iter().filter(|r| r.is_drop()).count(),
            2,
            "a third drop reason needs `unsupported_corrections`' doc revisited"
        );
        let losses: Vec<MaskImportLoss> = MaskImportReason::ALL
            .into_iter()
            .map(|reason| MaskImportLoss { name: format!("m{}", rank(reason)), reason })
            .collect();
        let line = describe_import_losses(3, &losses).expect("ten losses ⇒ a line");
        assert!(line.contains("imported 3 Lightroom mask(s)"), "{line}");
        for r in MaskImportReason::ALL {
            assert!(line.contains(r.en()), "{r:?} never reaches the prose: {line}");
            assert!(
                line.contains(&format!("m{}", rank(r))),
                "{r:?} loses its correction name: {line}"
            );
        }
    }

    // ── R25 P6: the four LOCAL point curves ──────────────────────────────

    /// The round trip, in both directions, over all four keys and their
    /// SPARSENESS. The fixture reproduces `DSC09642.xmp`'s own shape: Red and
    /// Green present, Main and Blue absent.
    #[test]
    fn local_curves_round_trip() {
        let curves = format!(
            "{}{}",
            lr_curve("RedCurve", &[(0, 0), (239, 255)]),
            lr_curve("GreenCurve", &[(0, 12), (128, 140), (255, 255)]),
        );
        let doc = lr_doc(&lr_correction_with_curves(
            "Radial 1",
            "",
            &curves,
            &lr_radial("0", "0"),
        ));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "the correction must import: {:?}", r.masks);
        let m = &r.masks[0];
        assert_eq!(
            m.red_curve,
            vec![
                CurvePoint { input: 0, output: 0 },
                CurvePoint { input: 239, output: 255 },
            ],
            "crs:RedCurve did not reach the recipe"
        );
        assert_eq!(m.green_curve.len(), 3, "crs:GreenCurve: {:?}", m.green_curve);
        assert!(m.main_curve.is_empty() && m.blue_curve.is_empty(), "absent means absent");

        // …and back out through OUR writer, then in again: the curves survive
        // byte-for-byte as VALUES (the writer's own spelling is pinned by
        // `local_curve_serialization_has_no_space_after_the_comma`).
        let back = xmp_to_recipe(&recipe_to_xmp(&r));
        assert_eq!(back.masks.len(), 1, "the mask survives our own projection");
        assert_eq!(back.masks[0].red_curve, m.red_curve, "red curve lost in the round trip");
        assert_eq!(back.masks[0].green_curve, m.green_curve, "green curve lost");
        assert!(
            back.masks[0].main_curve.is_empty() && back.masks[0].blue_curve.is_empty(),
            "the writer invented a curve the recipe does not hold"
        );
    }

    /// THE FORMAT MUTATION GUARD. Lightroom spells a LOCAL curve point
    /// `x,y` and a GLOBAL one `x, y`. Nothing in the code enforces that but
    /// two separate formatters and this test — and "let's share one helper" /
    /// "let's make the spacing consistent" is exactly the tidy-up a later
    /// reader would make.
    ///
    /// MUTATION THIS CATCHES: adding a space in `local_curve_elem`, or
    /// removing one from `owned_children`'s `curve_elem`. Both halves are
    /// asserted in ONE document, so neither can be satisfied by accident.
    #[test]
    fn local_curve_serialization_has_no_space_after_the_comma() {
        let r = EditRecipe {
            // The global master curve, whose writer uses the SPACED form.
            tone_curve: vec![
                CurvePoint { input: 10, output: 20 },
                CurvePoint { input: 255, output: 255 },
            ],
            masks: vec![LocalAdjustment {
                name: "curved".into(),
                amount: 1.0,
                main_curve: vec![
                    CurvePoint { input: 32, output: 48 },
                    CurvePoint { input: 255, output: 255 },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        assert!(
            xmp.contains("<rdf:li>32,48</rdf:li>"),
            "a LOCAL curve point is spelled `x,y` with no space: {xmp}"
        );
        assert!(
            !xmp.contains("<rdf:li>32, 48</rdf:li>"),
            "the local writer grew the global writer's space: {xmp}"
        );
        assert!(
            xmp.contains("<rdf:li>10, 20</rdf:li>"),
            "the GLOBAL curve keeps its spaced form: {xmp}"
        );
        assert!(
            !xmp.contains("<rdf:li>10,20</rdf:li>"),
            "the global writer lost its space to the local one: {xmp}"
        );
        // The key is the BARE name and the element sits between the attribute
        // block and the mask list — the two other things the reference
        // sidecars pin and a shared helper would get wrong.
        assert!(xmp.contains("<crs:MainCurve>"), "the local key carries no PV2012 suffix: {xmp}");
        let after_refine = xmp
            .split_once("crs:LocalCurveRefineSaturation=\"100\">")
            .expect("the correction's attribute block closes there")
            .1;
        let curve_at = after_refine.find("<crs:MainCurve>").expect("the curve is emitted");
        let masks_at = after_refine.find("<crs:CorrectionMasks>").expect("the mask list is too");
        assert!(curve_at < masks_at, "the curve must precede <crs:CorrectionMasks>: {xmp}");
    }

    /// Sparse in, sparse OUT. Lightroom writes only the curves that exist, and
    /// a writer that emitted all four (as identities, say) would hand the
    /// photographer's sidecar three curves they never drew.
    #[test]
    fn sparse_curves_stay_sparse() {
        let r = EditRecipe {
            masks: vec![LocalAdjustment {
                name: "red only".into(),
                amount: 1.0,
                red_curve: vec![
                    CurvePoint { input: 0, output: 0 },
                    CurvePoint { input: 128, output: 160 },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        assert!(xmp.contains("<crs:RedCurve>"), "the curve that exists is written: {xmp}");
        for absent in ["<crs:MainCurve>", "<crs:GreenCurve>", "<crs:BlueCurve>"] {
            assert!(!xmp.contains(absent), "{absent} was invented out of an empty curve: {xmp}");
        }
        // A mask with NO curve at all writes none of the four — the common
        // case, and the one that keeps every pre-P6 sidecar byte-identical.
        let plain = recipe_to_xmp(&EditRecipe {
            masks: vec![LocalAdjustment { name: "plain".into(), amount: 1.0, ..Default::default() }],
            ..Default::default()
        });
        for absent in ["<crs:MainCurve>", "<crs:RedCurve>", "<crs:GreenCurve>", "<crs:BlueCurve>"] {
            assert!(!plain.contains(absent), "{absent} on a curveless mask: {plain}");
        }
    }

    /// R25 P1 raised `LocalCurve` on every correction that carried one of the
    /// four curve elements, because the engine modelled none of them. P6
    /// models all four — so a correction whose curves READ must now report
    /// NOTHING, or the disclosure cries wolf on 19 of the user's photos.
    #[test]
    fn a_correction_with_curves_no_longer_reports_a_local_curve_loss() {
        let curves = lr_curve("MainCurve", &[(0, 0), (128, 96), (255, 255)]);
        let doc =
            lr_doc(&lr_correction_with_curves("Radial 1", "", &curves, &lr_radial("0", "0")));
        assert!(
            import_losses(&doc).is_empty(),
            "a correction whose curve imported must report no loss: {:?}",
            import_losses(&doc)
        );
        assert_eq!(unsupported_corrections(&doc), 0);
        assert_eq!(
            xmp_to_recipe(&doc).masks[0].main_curve.len(),
            3,
            "premise: the curve really did import (else the silence is a lie)"
        );
    }

    /// The other half of the narrowing: a curve that is PRESENT and cannot be
    /// read is still a loss, and it is still NAMED. `parse_one_correction`
    /// reads the four keys through the unchecked `parse_curve`, whose `Err`
    /// half becomes an empty curve — this is what stops that from being
    /// silence, which is the module's standing rule (`owned_element_body`).
    ///
    /// Costing the CURVE and not the correction is deliberate: the geometry is
    /// still exactly what the file draws, the same verdict a foreign range
    /// mask gets.
    #[test]
    fn an_unreadable_local_curve_is_named_not_swallowed() {
        // "999,-5" is out of the 0..255 domain — the same input
        // `parse_curve_checked` refuses on the global curves (L05).
        let curves = lr_curve("BlueCurve", &[(0, 0), (255, 255)])
            .replace("<rdf:li>255,255</rdf:li>", "<rdf:li>999,-5</rdf:li>");
        let doc =
            lr_doc(&lr_correction_with_curves("Radial 1", "", &curves, &lr_radial("0", "0")));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "the mask still imports — the shape is readable");
        assert!(r.masks[0].blue_curve.is_empty(), "an unreadable curve imports as none");
        assert_eq!(
            import_losses(&doc),
            vec![MaskImportLoss {
                name: "Radial 1".into(),
                reason: MaskImportReason::LocalCurve
            }],
            "the loss must be named, not swallowed"
        );
    }

    // ── R27 Batch-4 (L-08): the brush arm ────────────────────────────────────

    /// One `Mask/Paint` stroke. Attribute VALUES are `_DSC9583` Mask 7 →
    /// Aggregate "Brush 1" verbatim (the F2 anatomy's reference specimen,
    /// `D:/Photography/Raw/2024/24-12-New York-Raw/_DSC9583.xmp`, 75,935 B);
    /// the indentation is not, because whitespace between attributes is
    /// insignificant and a fixture whose mutations depend on counting spaces
    /// is a fixture that tests the spaces.
    fn lr_paint(sync: &str, value: &str, blend: &str, inverted: &str, dabs: &[&str]) -> String {
        let items: String =
            dabs.iter().map(|t| format!("<rdf:li>{t}</rdf:li>\n")).collect();
        format!(
            "<rdf:li>\n\
             <rdf:Description crs:What=\"Mask/Paint\" crs:MaskActive=\"true\"\n\
             crs:MaskBlendMode=\"{blend}\" crs:MaskInverted=\"{inverted}\"\n\
             crs:MaskSyncID=\"{sync}\" crs:MaskValue=\"{value}\"\n\
             crs:Radius=\"0.582157\" crs:Flow=\"1\" crs:CenterWeight=\"0\">\n\
             <crs:Dabs>\n\
             <rdf:Seq>\n\
             {items}\
             </rdf:Seq>\n\
             </crs:Dabs>\n\
             </rdf:Description>\n\
             </rdf:li>\n"
        )
    }

    /// Stroke 1 of `_DSC9583` Mask 7 → Brush 1: `MaskValue="0.439815"`,
    /// `Radius="0.582157"`, and the eight dab tokens §1.1 of the anatomy
    /// prints as its worked example.
    fn lr_paint_specimen() -> String {
        lr_paint(
            "FA7459A9F5626F4881D7B730C3093F95",
            "0.439815",
            "0",
            "false",
            &[
                "r 0.581835",
                "d 0.000684 0.940004",
                "r 0.581172",
                "d 0.113862 0.987261",
                "r 0.580873",
                "d 0.229292 1.011389",
                "r 0.581205",
                "d 0.112441 1.007149",
            ],
        )
    }

    /// The `Mask/Aggregate` group itself — two strokes, the second exercising
    /// the `f` and `h` state tokens. `(MaskBlendMode, MaskValue) = (0, 1)` is
    /// Lightroom's plain ADD, 16 of the 39 real Aggregates; `extra_child` is
    /// spliced as a THIRD member of `crs:Masks` for the nesting tests.
    fn lr_brush_group(group_inverted: &str, extra_child: &str) -> String {
        let s1 = lr_paint_specimen();
        let s2 = lr_paint(
            "1111111111111111111111111111111A",
            "1",
            "0",
            "false",
            &["f 1", "h 1", "d 0.500000 0.500000"],
        );
        format!(
            "<rdf:li>\n\
             <rdf:Description crs:What=\"Mask/Aggregate\" crs:MaskActive=\"true\"\n\
             crs:MaskName=\"Brush 1\" crs:MaskBlendMode=\"0\"\n\
             crs:MaskInverted=\"{group_inverted}\"\n\
             crs:MaskSyncID=\"0000000000000000000000000000000D\" crs:MaskValue=\"1\">\n\
             <crs:Masks>\n\
             <rdf:Seq>\n\
             {extra_child}{s1}{s2}\
             </rdf:Seq>\n\
             </crs:Masks>\n\
             </rdf:Description>\n\
             </rdf:li>\n"
        )
    }

    /// The `_DSC9583` Mask 7 shape — a linear gradient plus the brush group —
    /// imports WHOLE, where before R27 Batch-4 the whole correction was thrown
    /// away and the gradient with it. That is the L-08 registration's own
    /// complaint: 14 already-drawable parametric shapes across the reference
    /// library were being discarded because a NEIGHBOURING component was a
    /// brush.
    ///
    /// MUTATION-LINED. Verified red three independent ways (transcripts in the
    /// batch report): reverting the `"Mask/Aggregate"` arm of
    /// `classify_correction` to `unknown_component = true`; deleting
    /// `parse_one_correction`'s brush-component collection; never pushing
    /// `MaskImportReason::BrushRendered`, which imports the correction SILENTLY
    /// — the failure mode this project treats as worse than the refusal it
    /// replaced.
    /// R29 Batch-3: `crs:LensProfileEnable` is READ, in both spellings, and a
    /// document that says nothing gets no opinion put in its mouth.
    ///
    /// This is the fact that separates "no warp because Lightroom drew no
    /// correction" (the frames coincide; identity is CORRECT) from "no warp
    /// because nobody could solve one" (the frames differ by an unknown
    /// amount). `MaskWarpSource` keeps them apart and this reader is what
    /// supplies the first one.
    #[test]
    fn the_sidecar_lens_profile_switch_is_read_in_both_spellings() {
        let with = |v: &str| {
            lr_doc("").replace("crs:Version=", &format!("crs:LensProfileEnable=\"{v}\"\n    crs:Version="))
        };
        // PREMISE: the substitution really landed, or every case below is
        // reading a document with no such key and agreeing by accident.
        assert!(with("0").contains("crs:LensProfileEnable=\"0\""), "{}", with("0"));
        assert_eq!(lens_profile_enabled(&with("0")), Some(false));
        assert_eq!(lens_profile_enabled(&with("1")), Some(true));
        assert_eq!(lens_profile_enabled(&with("False")), Some(false));
        assert_eq!(lens_profile_enabled(&with("true")), Some(true));
        // Says nothing / says something unreadable ⇒ no opinion. Guessing here
        // would decide a coordinate frame from a value nobody wrote.
        assert_eq!(lens_profile_enabled(&lr_doc("")), None);
        assert_eq!(lens_profile_enabled(&with("maybe")), None);
        // `crs:LensProfileName` must not answer for `crs:LensProfileEnable` —
        // MUTATION THIS KILLS: dropping the `crs:` key anchoring in `crs_str`.
        let named = lr_doc("").replace(
            "crs:Version=",
            "crs:LensProfileName=\"Adobe (Sony FE 24-105mm F4 G OSS)\"\n    crs:Version=",
        );
        assert_eq!(lens_profile_enabled(&named), None);
    }

    /// R29 Batch-3 ACCEPTANCE ④ — the mask warp does NOT touch this boundary.
    ///
    /// `LensProfile::mask_warp` is a RENDER-TIME map. The recipe keeps the
    /// coordinates the sidecar stored, verbatim, so `lr_to_engine` and
    /// `engine_to_lr` stay exact inverses of one another and a republished
    /// sidecar is byte-faithful to what Lightroom wrote — brush dab streams
    /// included, which is the payload with the least tolerance for a rewrite.
    /// The frame here is LANDSCAPE, which is what keeps that claim whole after
    /// R29 C1: a turn does rewrite the dab stream now, and the only thing that
    /// must never reach this boundary is the WARP.
    ///
    /// The document here carries a radial, a gradient and a two-stroke brush
    /// group, and is exported twice: once from a recipe with no warp and once
    /// from the same recipe carrying the full 105 mm warp — the most violent
    /// one measured (`m` from 1.0425 at the centre to 0.9976 at the corner,
    /// ~88 px at r = 3250). The two documents must be EQUAL, byte for byte.
    ///
    /// MUTATION THIS KILLS: applying `render::lr_mask_warp_norm` inside
    /// `masks_xml` / `radial_mask_xml` / `brush_mask_xml`, or anywhere else on
    /// the way out. Any of them makes these two strings differ.
    #[test]
    fn the_mask_warp_never_reaches_the_xmp_boundary() {
        let frame = FrameAspect::from_size(9504.0, 6336.0);
        let doc = in_frame(
            &lr_doc(&format!(
                "{}{}",
                lr_correction("Mask 7", "", &format!("{}{}", lr_gradient("0"), lr_brush_group("false", ""))),
                lr_correction(
                    "R",
                    "",
                    &lr_radial_at("-0.082402", "-0.008723", "1.109604", "1.090228", "28.229232")
                ),
            )),
            9504,
            6336,
        );
        let plain = xmp_to_recipe(&doc);
        // PREMISE: the document really did bring in the geometry whose frame
        // this test is about, or it would prove nothing.
        assert_eq!(plain.masks.len(), 2, "both corrections must import: {:?}", plain.masks);
        assert!(
            plain.masks.iter().any(|m| m
                .components
                .iter()
                .any(|c| matches!(c.geometry, MaskGeometry::Brush { .. }))),
            "the brush group must be in the recipe"
        );
        let mut warped = plain.clone();
        let model = crate::lcp::PerspectiveModel {
            focal_mm: Some(105.0),
            focus_distance: Some(10000.0),
            scale: 0.959207,
            k: [0.961677, 1.182717, -8.218554],
            focal_x: None,
            sensor_format_factor: 1.0,
        };
        let legacy_warp = model
            .mask_warp_knots((9504.0, 6336.0), 16)
            .expect("the legacy 105mm table solves");
        let dense_warp = model
            .mask_warp_knots((9504.0, 6336.0), crate::recipe::MASK_WARP_KNOTS)
            .expect("the dense 105mm table solves");
        let half_diag = 0.5f32 * 9504.0f32.hypot(6336.0);
        let radius = 3250.0f32;
        let rho = radius / half_diag;
        let tabulation_delta =
            (crate::render::mask_warp_factor(&dense_warp, rho)
                - crate::render::mask_warp_factor(&legacy_warp, rho))
            .abs()
                * radius;
        eprintln!("105mm mask-warp n=16→64 delta at r=3250: {tabulation_delta:.4}px");
        assert!(tabulation_delta < 0.35, "105mm survival bound: {tabulation_delta}px");
        warped.lens_profile = crate::recipe::LensProfile {
            mask_warp: dense_warp,
            mask_warp_src: crate::recipe::MaskWarpSource::Lcp,
            ..Default::default()
        };
        // PREMISE: the warp really is a warp — an identity table would make
        // the equality below vacuous.
        let w = &warped.lens_profile.mask_warp;
        assert_eq!(w.len(), crate::recipe::MASK_WARP_KNOTS);
        assert!(w[0] > 1.04 && w[w.len() - 1] < 1.0, "the 105mm warp is not the identity: {w:?}");

        let a = recipe_to_xmp_in_frame(&plain, frame).0;
        let b = recipe_to_xmp_in_frame(&warped, frame).0;
        assert_eq!(a, b, "an active mask warp changed the written sidecar");
        // And the ROUND TRIP still lands on the same recipe geometry, so the
        // equality above is not two identically-broken documents.
        //
        // Compared field by field rather than with one `assert_eq!` on the
        // masks, because `crs:MaskSyncID` legitimately differs: the writer
        // MINTS a fresh identity for every component it emits (see
        // `BrushStroke::sync_id`), so a whole-struct comparison would fail on
        // the one field that is supposed to change and say nothing about the
        // frame.
        let back = xmp_to_recipe(&b);
        assert_eq!(back.masks.len(), plain.masks.len());
        for (got, want) in back.masks.iter().zip(&plain.masks) {
            assert_eq!(got.mask, want.mask, "base geometry moved");
            assert_eq!(got.components.len(), want.components.len());
            for (g, w) in got.components.iter().zip(&want.components) {
                match (&g.geometry, &w.geometry) {
                    (
                        MaskGeometry::Brush { strokes: gs, name: gn, .. },
                        MaskGeometry::Brush { strokes: ws, name: wn, .. },
                    ) => {
                        assert_eq!(gn, wn);
                        assert_eq!(gs.len(), ws.len());
                        for (a, b) in gs.iter().zip(ws) {
                            // The DAB STREAM, token for token — the payload a
                            // coordinate warp would have rewritten.
                            assert_eq!(a.dabs, b.dabs, "a dab stream was rewritten");
                            assert_eq!((a.value, a.radius, a.flow, a.center_weight),
                                (b.value, b.radius, b.flow, b.center_weight));
                        }
                    }
                    (g, w) => assert_eq!(g, w, "component geometry moved"),
                }
            }
        }
    }

    #[test]
    fn a_lightroom_brush_group_imports_beside_the_shapes_it_used_to_take_down() {
        let doc = lr_doc(&lr_correction(
            "Mask 7",
            "",
            &format!("{}{}", lr_gradient("0"), lr_brush_group("false", "")),
        ));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "the correction must import: {:?}", r.masks);
        let m = &r.masks[0];
        // The parametric shape is still the BASE — a brush does not displace a
        // gradient that was there first (`base_geometry_at`).
        assert!(matches!(m.mask, MaskGeometry::Linear { .. }), "{:?}", m.mask);
        assert_eq!(m.components.len(), 1, "the brush group rides as a component");
        assert_eq!(m.components[0].mode, MaskCombine::Add, "MaskBlendMode=0 is a union");
        let MaskGeometry::Brush { name, blend_mode, value, inverted, strokes } =
            &m.components[0].geometry
        else {
            panic!("expected a brush group, got {:?}", m.components[0].geometry);
        };
        assert_eq!((name.as_str(), *blend_mode, *value, *inverted), ("Brush 1", 0, 1.0, false));
        assert_eq!(strokes.len(), 2, "both Mask/Paint children arrive");
        assert_eq!(strokes[0].value, 0.439815);
        assert_eq!(strokes[0].radius, 0.582157);
        assert_eq!(strokes[0].flow, 1.0);
        assert_eq!(strokes[0].center_weight, 0.0);
        assert_eq!(strokes[0].sync_id, "FA7459A9F5626F4881D7B730C3093F95");
        // The dab stream, token for token, in document order.
        assert_eq!(
            strokes[0].dabs,
            "r 0.581835\nd 0.000684 0.940004\nr 0.581172\nd 0.113862 0.987261\n\
             r 0.580873\nd 0.229292 1.011389\nr 0.581205\nd 0.112441 1.007149"
        );
        assert_eq!(strokes[1].dabs, "f 1\nh 1\nd 0.500000 0.500000");
        // And the import SAYS so — imported whole, drawn from our model.
        let losses = import_losses(&doc);
        assert!(
            losses.iter().any(|l| l.reason == MaskImportReason::BrushRendered),
            "a brush drawn from our own model must be disclosed: {losses:?}"
        );
        assert!(
            !losses.iter().any(|l| l.reason.is_drop()),
            "nothing about this correction was dropped: {losses:?}"
        );
    }

    /// A correction whose ONLY component is a brush group imports too — its
    /// first Aggregate becomes the base geometry (F2 §7.3). Nine of the
    /// eighteen corrections this batch rescues have exactly that shape.
    ///
    /// MUTATION-LINED: deleting `parse_one_correction`'s brush fallback (the
    /// `None => { … brushes.remove(0) … }` arm) makes it return `None`, the
    /// correction lands on `OutOfModel`, and this goes to 0 masks.
    #[test]
    fn a_brush_only_correction_takes_its_first_group_as_the_base() {
        let doc = lr_doc(&lr_correction("Mask 1", "", &lr_brush_group("false", "")));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "brush-only corrections import now: {:?}", r.masks);
        assert!(matches!(r.masks[0].mask, MaskGeometry::Brush { .. }));
        assert!(r.masks[0].components.is_empty(), "one group, no extras");
        // Inert, NOT inverted: the group's own inversion bit lives inside the
        // geometry, and lifting it here as well would turn a zero-coverage
        // mask into a whole-frame adjustment.
        assert!(!r.masks[0].inverted);
    }

    /// The write-back is faithful to the measured `Mask/Aggregate` shape:
    /// import → write puts every attribute and every dab token back, and a
    /// second import is a FIXED POINT. (The `crs:MaskSyncID`s are the writer's
    /// own by design — it mints them for every component it emits — so the
    /// comparison that has to be exact is the recipe, not the ID text.)
    ///
    /// MUTATION-LINED: dropping `extra_lis` from the emitted `<rdf:Seq>` makes
    /// the group vanish from the sidecar and the fixed-point half fails.
    #[test]
    fn a_brush_group_round_trips_back_into_the_sidecar() {
        // BOTH slots the group can occupy, because the writer reaches them
        // through different code: as the correction's BASE (a brush-only
        // correction) and as an extra COMPONENT beside a parametric shape
        // (`_DSC9583` Mask 7's own shape). A round-trip test that only ever
        // saw the base would stay green while the component arm dropped the
        // group on the floor.
        for components in [
            lr_brush_group("false", ""),
            format!("{}{}", lr_gradient("0"), lr_brush_group("false", "")),
        ] {
            brush_round_trip_case(&components);
        }
    }

    fn brush_round_trip_case(components: &str) {
        let doc = lr_doc(&lr_correction("Mask 7", "", components));
        let once = xmp_to_recipe(&doc);
        let written = recipe_to_xmp(&once);
        // 1. The dab tokens ride out verbatim, in order.
        for token in [
            "<rdf:li>r 0.581835</rdf:li>",
            "<rdf:li>d 0.000684 0.940004</rdf:li>",
            "<rdf:li>d 0.112441 1.007149</rdf:li>",
            "<rdf:li>f 1</rdf:li>",
            "<rdf:li>h 1</rdf:li>",
        ] {
            assert!(written.contains(token), "missing {token} in:\n{written}");
        }
        // 2. The components' own attributes, in Lightroom's own spelling.
        for attr in [
            r#"crs:What="Mask/Aggregate""#,
            r#"crs:MaskName="Brush 1""#,
            r#"crs:What="Mask/Paint""#,
            r#"crs:MaskValue="0.439815""#,
            r#"crs:Radius="0.582157""#,
            r#"crs:Flow="1""#,
            r#"crs:CenterWeight="0""#,
        ] {
            assert!(written.contains(attr), "missing {attr} in:\n{written}");
        }
        // 3. FIXED POINT, at the level that has to be one — the DOCUMENT.
        // Reading our own sidecar back and writing it again is byte-identical,
        // which is the assertion that fails if any value is reformatted on the
        // way out: an `f32` printed through a rounding formatter, a token
        // re-spaced, a stroke re-ordered, an attribute dropped.
        let twice = xmp_to_recipe(&written);
        assert_eq!(written, recipe_to_xmp(&twice), "the sidecar is not a fixed point");
        assert_eq!(once.masks.len(), twice.masks.len());
        let (a, b) = (&once.masks[0], &twice.masks[0]);
        // The RECIPE is a fixed point too, with exactly one NAMED exception:
        // `crs:MaskSyncID`. The writer mints its own for every component it
        // emits (`guid`), so a group that came in with Lightroom's IDs goes out
        // with ours and comes back carrying those. That is the writer's
        // standing rule rather than anything about brushes — and it is an
        // ACCEPTED COST, stated here so it is a decision and not a surprise:
        // the ID Lightroom used for a stroke survives one save and no more.
        // Everything that describes the STROKE survives every save.
        let strip = |g: &MaskGeometry| match g {
            MaskGeometry::Brush { name, blend_mode, value, inverted, strokes } => {
                let bare: Vec<_> = strokes
                    .iter()
                    .map(|s| BrushStroke { sync_id: String::new(), ..s.clone() })
                    .collect();
                MaskGeometry::Brush {
                    name: name.clone(),
                    blend_mode: *blend_mode,
                    value: *value,
                    inverted: *inverted,
                    strokes: bare,
                }
            }
            other => other.clone(),
        };
        assert_eq!(strip(&a.mask), strip(&b.mask), "the brush geometry is not a fixed point");
        assert_eq!(a.components.len(), b.components.len());
        for (ca, cb) in a.components.iter().zip(&b.components) {
            assert_eq!(ca.mode, cb.mode);
            assert_eq!(strip(&ca.geometry), strip(&cb.geometry));
        }
        // 4. And the WRITER discloses the same fact the reader did.
        let losses = mask_export_losses(&once);
        assert!(
            losses.iter().any(|l| l.reason == MaskLossReason::BrushRendered),
            "the writer must say the brush it emitted was drawn by us: {losses:?}"
        );
        assert!(
            !losses.iter().any(|l| l.reason == MaskLossReason::ComponentsFlattened),
            "nothing was flattened — the brush went out whole: {losses:?}"
        );
    }

    /// HAZARD 1 (`classify_correction` walked the mask block FLAT). A flat walk
    /// sees the `Mask/Paint` strokes inside a group as SIBLINGS of it: they are
    /// none of the four kinds the classifier knows, so each one sets
    /// `unknown_component` and the whole correction is refused — the brush arm
    /// would import nothing at all.
    ///
    /// MUTATION-LINED: replacing the `components.iter().filter(|c| c.depth ==
    /// 0)` walk with the old `next_xml_tag` loop over `mask_block` refuses this
    /// document (`Unrepresentable`, 0 masks).
    #[test]
    fn nested_paint_strokes_are_not_siblings_of_their_group() {
        let doc = lr_doc(&lr_correction("Mask 1", "", &lr_brush_group("false", "")));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "a nested Paint must not read as an unknown component");
        let MaskGeometry::Brush { strokes, .. } = &r.masks[0].mask else { panic!() };
        // Counted ONCE each, as children — a flat walk would also have made
        // them top-level components and double-counted the strokes.
        assert_eq!(strokes.len(), 2);
        // And the depth filter is not a hiding place: a component nested inside
        // a container we do NOT model is markup this reader cannot account for,
        // so it refuses rather than walking past it.
        let smuggled = doc.replace(
            "<crs:Masks>",
            "<crs:Masks>\n<crs:Decoy><rdf:li crs:What=\"Mask/Gradient\" crs:ZeroX=\"0\" \
             crs:ZeroY=\"0\" crs:FullX=\"1\" crs:FullY=\"1\"/></crs:Decoy>",
        );
        assert_ne!(smuggled, doc, "the mutation did not apply");
        assert_eq!(
            xmp_to_recipe(&smuggled).masks.len(),
            0,
            "a component in an unmodelled container is markup we cannot account for"
        );
    }

    /// The F5 chain, end to end (R28 2b): an over-cap dab stream is
    /// TRUNCATED, the truncation is DISCLOSED, and what we republish is still
    /// a document our own reader accepts.
    ///
    /// The construction is the adjudication's: 65,536 `"d 0 0"` tokens sit
    /// exactly on the read side's token ceiling and pass it, then arrive at a
    /// store-side cap counted in BYTES (393,215 vs 262,144). Before this
    /// batch, `cap` cut that inside a token, `xmp_to_recipe` dropped the
    /// `ClampSummary` on the floor so nothing said a word, and the writer —
    /// whose split is the exact inverse of the reader's join — put the
    /// fragment back into the sidecar, where our next read refused the whole
    /// Aggregate and the group's masks vanished.
    ///
    /// MUTATION THIS KILLS, three ways: revert `cap_tokens` to `cap` (a
    /// republished `<rdf:li>d 0 </rdf:li>` fails `dab_token_is_known` below);
    /// revert `xmp_to_recipe`'s tail to a bare `r.clamp();` (the summary is
    /// empty and the disclosure assertion fails); drop the length bound in
    /// `dab_token_is_known` (the single-huge-token arm at the end imports).
    #[test]
    fn an_oversized_dab_stream_is_disclosed_and_republished_whole_token_only() {
        let many = vec!["d 0 0"; 65_536];
        let big = lr_paint("2222222222222222222222222222222B", "1", "0", "false", &many);
        let doc = lr_doc(&lr_correction("Mask 1", "", &lr_brush_group("false", &big)));
        assert!(doc.len() < MAX_XMP_BYTES, "premise: the fixture is a readable document");

        let (r, clamped) = xmp_to_recipe_clamped(&doc);
        assert_eq!(r.masks.len(), 1, "premise: the brush group imports");
        // DISCLOSED, not swallowed — the single fact this whole item exists
        // for. `xmp_to_recipe`'s own clamp used to be the only one that saw
        // it, and it threw the answer away.
        assert!(
            clamped.truncated_string_bytes > 100_000,
            "the import cut ~131 KB of dabs and must say so: {clamped:?}"
        );

        // …and the projection we would hand back to Lightroom carries only
        // tokens of the measured grammar. `dab_token_is_known` is the reader's
        // own judge, so this asserts the round trip against the exact rule
        // that used to reject it.
        let out = recipe_to_xmp(&r);
        let mut checked = 0usize;
        let mut rest = out.as_str();
        while let Some(open) = rest.find("<crs:Dabs>") {
            let close = rest[open..].find("</crs:Dabs>").expect("closed Dabs block") + open;
            let mut seq = &rest[open..close];
            while let Some(i) = seq.find("<rdf:li>") {
                let j = seq[i..].find("</rdf:li>").expect("closed item") + i;
                let token = &seq[i + "<rdf:li>".len()..j];
                assert!(
                    dab_token_is_known(token).is_ok(),
                    "republished a token our own reader refuses: {token:?}"
                );
                checked += 1;
                seq = &seq[j..];
            }
            rest = &rest[close..];
        }
        assert!(checked > 40_000, "premise: the republished stream is the big one ({checked})");

        // The aggravator, same door: ONE token whose coordinate is `0.` plus
        // 300,000 digits parses to a finite `f32` (0.111…), passes every
        // shape check, and blows the byte cap by itself while the token COUNT
        // gate never fires. Refused at the token now — which, by this
        // reader's existing all-or-nothing rule for a group
        // (`parse_brush_group` propagates one bad Paint), refuses the
        // Aggregate and DISCLOSES it. That is the same verdict any other
        // malformed token already earns; the bound only stops the malformed
        // one from being called well-formed.
        //
        // FRACTIONAL, not the adjudication's 300,000 INTEGER digits: those
        // overflow to `inf` and the finiteness check already refused them, so
        // this is the shape that actually needed a length bound.
        let huge = format!("d 0 0.{}", "1".repeat(300_000));
        let mono = lr_paint("3333333333333333333333333333333C", "1", "0", "false", &[&huge]);
        let mono_doc = lr_doc(&lr_correction("Mask 1", "", &lr_brush_group("false", &mono)));
        let (mr, _) = xmp_to_recipe_clamped(&mono_doc);
        assert_eq!(mr.masks.len(), 0, "an unbounded token cannot import as a stroke");
        assert!(
            import_losses(&mono_doc).iter().any(|l| l.reason == MaskImportReason::OutOfModel),
            "and the refusal is named: {:?}",
            import_losses(&mono_doc)
        );
    }

    /// HAZARD 2 (`base_geometry_at` scanned the WHOLE correction segment for a
    /// `crs:What="Mask/Gradient"` tag, nesting-blind). The correction here has
    /// one real component — a SUBTRACT radial in `crs:CorrectionMasks` — and a
    /// nested creative-Look block beside it holding a `Mask/Gradient` of its
    /// own. F2 found that shape in the reference library: one of the 105
    /// `Mask/Image` components lives inside a `crs:Preset`/`crs:Parameters`
    /// block rather than in any correction's component list.
    ///
    /// The old scan starts at byte 0 of the correction and takes the first
    /// default-blend geometry tag it meets, which is the LOOK's gradient — so
    /// the correction imported as a Linear mask built from a profile's baked
    /// parameters, a shape the photographer never drew. The selector now
    /// searches this correction's OWN component list, finds no default-blend
    /// member there, and falls back to the subtract radial (some shape beats no
    /// shape — see the function's doc).
    ///
    /// MUTATION-LINED: reverting `base_geometry_at` to the old flat
    /// `next_xml_tag` scan over `seg` imports the Look's gradient and this
    /// fails on the geometry KIND.
    #[test]
    fn a_shape_nested_beside_the_component_list_is_never_the_corrections_base() {
        // A creative Look's baked parameters — owned-LOOKING crs markup that
        // belongs to the profile, not to this correction (the same trap
        // `top_level_owned_spans` documents for the merge).
        let look = concat!(
            "       <crs:Look>\n        <rdf:Description>\n         <crs:Parameters>\n",
            "         <rdf:Description>\n",
            "          <rdf:li crs:What=\"Mask/Gradient\" crs:MaskActive=\"true\"\n",
            "           crs:MaskBlendMode=\"0\" crs:MaskInverted=\"false\" crs:MaskValue=\"1\"\n",
            "           crs:ZeroX=\"0.9\" crs:ZeroY=\"0.9\" crs:FullX=\"0.1\" crs:FullY=\"0.1\"/>\n",
            "         </rdf:Description>\n        </crs:Parameters>\n",
            "        </rdf:Description>\n       </crs:Look>\n",
        );
        let doc =
            lr_doc(&lr_correction_with_curves("Mask 1", "", look, &lr_radial("0", "1")));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "the correction still imports: {:?}", r.masks);
        assert!(
            matches!(r.masks[0].mask, MaskGeometry::Radial { .. }),
            "the base must come from this correction's OWN component list, never from a \
             nested Look: {:?}",
            r.masks[0].mask
        );
    }

    /// The other half of the same rule, and the one the brush arm needs: a
    /// parametric shape nested INSIDE a brush group is refused rather than
    /// promoted. "An Aggregate whose child is not a Paint" has zero
    /// counter-examples in 177 current sidecars, so a document with one was
    /// written by something other than Lightroom.
    ///
    /// MUTATION-LINED: loosening `parse_brush_group`'s child-kind gate from
    /// `return Err(())` to `continue` imports the correction.
    #[test]
    fn a_shape_nested_inside_a_brush_group_is_refused_not_promoted() {
        let nested = "<rdf:li crs:What=\"Mask/Gradient\" crs:MaskActive=\"true\" \
                      crs:MaskBlendMode=\"0\" crs:MaskInverted=\"false\" crs:MaskValue=\"1\" \
                      crs:ZeroX=\"0.1\" crs:ZeroY=\"0.2\" crs:FullX=\"0.3\" \
                      crs:FullY=\"0.4\"/>\n";
        let doc = lr_doc(&lr_correction("Mask 1", "", &lr_brush_group("false", nested)));
        let r = xmp_to_recipe(&doc);
        assert!(
            r.masks.is_empty(),
            "a gradient inside an Aggregate is a shape Lightroom never writes — refuse it, \
             do not promote it: {:?}",
            r.masks
        );
        assert!(
            import_losses(&doc).iter().any(|l| l.reason == MaskImportReason::OutOfModel),
            "and say which kind of refusal it was — the NESTING is accounted for, it is the \
             shape that is outside the model"
        );
    }

    /// HAZARD 3 (`parse_one_correction` read geometry keys from a slice running
    /// to the END of the correction). The base gradient here omits
    /// `crs:MaskInverted`; the brush group AFTER it carries
    /// `crs:MaskInverted="true"`. The unbounded scan finds the GROUP's bit and
    /// inverts a mask the base never asked to invert.
    ///
    /// MUTATION-LINED: changing `base_element(seg, p)` back to `&seg[p..]`
    /// makes `inverted` read `true` and the first assertion fails.
    #[test]
    fn a_later_components_attribute_cannot_answer_for_the_base_shape() {
        let bare = lr_gradient("0").replace("crs:MaskInverted=\"false\"\n", "");
        assert!(!bare.contains("MaskInverted"), "the base must declare no inversion");
        let group = lr_brush_group("true", "");
        let doc = lr_doc(&lr_correction("Mask 7", "", &format!("{bare}{group}")));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "{:?}", r.masks);
        assert!(
            !r.masks[0].inverted,
            "the base gradient declares no inversion — the group's bit is the GROUP's"
        );
        // And the group keeps its own bit, carried where it belongs.
        let MaskGeometry::Brush { inverted, .. } = &r.masks[0].components[0].geometry else {
            panic!("expected the group as a component")
        };
        assert!(*inverted, "the Aggregate's own MaskInverted rides in the geometry");
    }

    /// The measured INVARIANTS are gates, not fields: a `Mask/Paint` that
    /// asserts a composition, a missing attribute, a dab token outside
    /// `{r,d,f,h}`, a Paint with no `crs:Dabs`. Each has zero counter-examples
    /// in 177 current sidecars, so each costs the correction rather than being
    /// guessed past — the roundness rule, applied to a stroke.
    ///
    /// MUTATION-LINED: loosening any one gate in `parse_brush_group` /
    /// `parse_paint_stroke` / `dab_token_is_known` imports the corresponding
    /// document and fails the matching assertion.
    #[test]
    fn a_brush_group_outside_the_measured_encoding_is_refused_not_guessed() {
        let base = lr_doc(&lr_correction("Mask 1", "", &lr_brush_group("false", "")));
        assert_eq!(xmp_to_recipe(&base).masks.len(), 1, "the control must import");
        for (what, doc) in [
            (
                "a Paint asserting its own blend mode",
                base.replace(
                    "crs:What=\"Mask/Paint\" crs:MaskActive=\"true\"\ncrs:MaskBlendMode=\"0\"",
                    "crs:What=\"Mask/Paint\" crs:MaskActive=\"true\"\ncrs:MaskBlendMode=\"1\"",
                ),
            ),
            (
                "a Paint that inverts itself",
                base.replace(
                    "crs:MaskBlendMode=\"0\" crs:MaskInverted=\"false\"",
                    "crs:MaskBlendMode=\"0\" crs:MaskInverted=\"true\"",
                ),
            ),
            (
                "a Paint missing one of its nine attributes",
                base.replace(" crs:Flow=\"1\"", ""),
            ),
            (
                "a dab token of an unknown form",
                base.replace("<rdf:li>r 0.581835</rdf:li>", "<rdf:li>q 0.581835</rdf:li>"),
            ),
            (
                "a dab token of the wrong arity",
                base.replace("<rdf:li>r 0.581835</rdf:li>", "<rdf:li>r 0.5 0.6</rdf:li>"),
            ),
            (
                "a dab coordinate that is not a number",
                base.replace(
                    "<rdf:li>d 0.000684 0.940004</rdf:li>",
                    "<rdf:li>d 0.000684 nine</rdf:li>",
                ),
            ),
            (
                "a Paint with no Dabs at all",
                base.replace("crs:Dabs>", "crs:NotDabs>"),
            ),
            (
                "an Aggregate with no strokes at all",
                lr_doc(&lr_correction(
                    "Mask 1",
                    "",
                    "<rdf:li>\n<rdf:Description crs:What=\"Mask/Aggregate\" \
                     crs:MaskActive=\"true\" crs:MaskName=\"Brush 1\" crs:MaskBlendMode=\"0\" \
                     crs:MaskInverted=\"false\" crs:MaskValue=\"1\">\n<crs:Masks>\n\
                     <rdf:Seq>\n</rdf:Seq>\n</crs:Masks>\n</rdf:Description>\n</rdf:li>\n",
                )),
            ),
        ] {
            assert_ne!(doc, base, "the mutation for {what:?} did not apply");
            assert!(
                xmp_to_recipe(&doc).masks.is_empty(),
                "{what} must refuse the correction, not be imported as if understood"
            );
        }
    }

    /// A brush group is DRAWN by our own rasteriser and NAMED in both
    /// disclosure channels — it is neither passed off as Adobe's alpha nor
    /// silently approximated.
    ///
    /// R29 Batch-6b rewrote this from `a_carried_brush_is_named_…`: the phrase
    /// it used to require ("carried" + "not yet rendered") was the disclosure
    /// of an engine that drew nothing, and keeping it green would have meant
    /// shipping a sentence the renderer had stopped honouring.
    ///
    /// MUTATION-LINED: reverting either `en()` to the old
    /// 「carried, not yet rendered」wording fails the phrase asserts below, and
    /// dropping either variant from `ALL` fails the first two lines — the lists
    /// every disclosure surface iterates.
    #[test]
    fn a_rendered_brush_is_named_in_both_channels_and_is_not_a_drop() {
        // Import twin and export twin describe the SAME fact, so both `ALL`
        // arrays — the lists every disclosure surface iterates — must hold it.
        assert!(MaskImportReason::ALL.contains(&MaskImportReason::BrushRendered));
        assert!(MaskLossReason::ALL.contains(&MaskLossReason::BrushRendered));
        assert!(!MaskImportReason::BrushRendered.is_drop(), "the correction DID import");
        for phrase in [MaskImportReason::BrushRendered.en(), MaskLossReason::BrushRendered.en()] {
            // Both halves of the sentence, because either one alone misleads:
            // "drawn" without "not Adobe's" reads as a raster round trip, and
            // "not Adobe's" without "drawn" reads as the old refusal.
            assert!(phrase.contains("drawn"), "{phrase}");
            assert!(phrase.contains("measured model"), "{phrase}");
            assert!(phrase.contains("not Adobe's own rasteriser"), "{phrase}");
            assert!(!phrase.contains("not yet rendered"), "the old refusal wording: {phrase}");
        }
        // The RENDER half of the same claim lives in render.rs, where
        // `mask_weight` is: `a_carried_brush_group_draws_its_dabs`.
    }

    /// FORENSIC REGRESSION, run against the user's own Lightroom library.
    /// The inline fixtures above are synthetic by policy, which means they
    /// prove the RULES and not the FILES — and §0 of this round was a defect
    /// nobody's synthetic fixture had caught in four releases.
    ///
    /// Point `AUTOSHADE_MB_FIXTURES` at a directory of `.xmp` / `.xmp.txt`
    /// sidecars and this asserts, per file, that every `crs:What="Correction"`
    /// is accounted for (imported + refused) and that the parametric ones
    /// really do arrive — which is 0 on every one of them before this batch.
    /// Unset, it is a silent no-op: the reference files are photographs, they
    /// are not in this repository, and no path to them appears in this test.
    #[test]
    fn real_lightroom_sidecars_import_their_parametric_masks() {
        let Some(dir) = crate::config::live_env("AUTOSHADE_MB_FIXTURES") else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            panic!("AUTOSHADE_MB_FIXTURES is set but unreadable: {dir}");
        };
        let mut files = 0usize;
        let mut total_imported = 0usize;
        for e in entries.flatten() {
            let p = e.path();
            if !p.to_string_lossy().to_lowercase().contains(".xmp") {
                continue;
            }
            // NAMED, never skipped (R25 P8). `else { continue }` here meant a
            // sidecar this probe could not read simply left the count — and a
            // forensic probe whose files quietly stop arriving is a green
            // test that measures nothing. A `.xmp` in the fixture directory
            // that will not read as UTF-8 is a fact about the fixtures the
            // round report has to hear.
            let text = std::fs::read_to_string(&p)
                .unwrap_or_else(|e| panic!("{}: fixture unreadable ({e})", p.display()));
            files += 1;
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            // The block's OWN corrections, counted the way the reader scopes
            // them — retouch areas and creative Looks carry `Mask/*`
            // components of their own and are not corrections.
            let block = crs_own_scope(&text);
            let corrections = owned_element_body(block.as_ref(), "crs:MaskGroupBasedCorrections")
                .ok()
                .flatten()
                .map(|b| b.matches("crs:What=\"Correction\"").count())
                .unwrap_or(0);
            let t0 = std::time::Instant::now();
            let imported = xmp_to_recipe(&text).masks.len();
            let import_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let refused = unsupported_corrections(&text);
            let losses = import_losses(&text);
            eprintln!(
                "{name}: {corrections} correction(s) → {imported} imported, {refused} refused, \
                 {} loss note(s), parse {import_ms:.2} ms",
                losses.len()
            );
            // The forensic half: WHICH verdict landed on which correction.
            // This is the output the probe exists for — a count says the
            // import worked, this says whether it was right.
            for l in &losses {
                eprintln!("    {:?}  {}", l.reason, l.name);
            }
            // R25 P5: the two carried radial attributes, observed on the real
            // files and then round-tripped through OUR writer. The assertion
            // is the invariant that matters — a value we do not interpret must
            // come back exactly as it went in — and the print is the evidence
            // for the round report (before this batch every radial read 50/2
            // because neither attribute was looked at).
            let mine = xmp_to_recipe(&recipe_to_xmp(&xmp_to_recipe(&text)));
            for (i, m) in xmp_to_recipe(&text).masks.iter().enumerate() {
                let (
                    MaskGeometry::Radial { midpoint, mask_version, .. },
                    Some(LocalAdjustment {
                        mask: MaskGeometry::Radial {
                            midpoint: rt_mid, mask_version: rt_ver, ..
                        },
                        ..
                    }),
                ) = (&m.mask, mine.masks.get(i))
                else {
                    continue;
                };
                eprintln!("    radial {i}: Midpoint={midpoint} Version={mask_version}");
                assert_eq!(
                    (midpoint, mask_version),
                    (rt_mid, rt_ver),
                    "{name}: radial {i} lost a carried attribute in the round-trip"
                );
            }
            // R25 P6: the four LOCAL point curves, per correction. 19 files in
            // the user's library carry 43 of them and every one used to be
            // dropped with a note; the print is the round report's per-file
            // curve list and the assertion is the round trip through OUR
            // writer — the one place the `x,y` spelling could silently drift
            // to the global `x, y` and still look right in a diff.
            let imported_recipe = xmp_to_recipe(&text);
            for (i, (m, rt)) in imported_recipe.masks.iter().zip(&mine.masks).enumerate() {
                for (key, got, round) in [
                    ("MainCurve", &m.main_curve, &rt.main_curve),
                    ("RedCurve", &m.red_curve, &rt.red_curve),
                    ("GreenCurve", &m.green_curve, &rt.green_curve),
                    ("BlueCurve", &m.blue_curve, &rt.blue_curve),
                ] {
                    if got.is_empty() {
                        continue;
                    }
                    let pts: Vec<String> =
                        got.iter().map(|p| format!("{},{}", p.input, p.output)).collect();
                    eprintln!(
                        "    mask {i} crs:{key}: {} point(s) [{}]",
                        got.len(),
                        pts.join(" ")
                    );
                    assert_eq!(got, round, "{name}: mask {i} lost crs:{key} in the round-trip");
                }
            }
            assert_eq!(
                imported + refused,
                corrections,
                "{name}: every correction must be either imported or counted as refused"
            );
            // GATED on the sidecar actually HAVING a correction (R27 Batch-4).
            // The bare `imported > 0` was true of the seven M-B fixtures and
            // false of the assertion's own sentence: pointed at any real
            // catalogue folder it failed on the first sidecar carrying nothing
            // but global sliders, claiming a file "with 0 correction(s) must
            // import at least one". A probe that cannot be aimed at a
            // directory of real photographs is a probe that only ever sees the
            // seven files someone already curated.
            assert!(
                corrections == 0 || imported > 0,
                "{name}: a real Lightroom sidecar with {corrections} correction(s) must import \
                 at least one — importing none is the defect this batch closed"
            );
            total_imported += imported;
        }
        assert!(files > 0, "AUTOSHADE_MB_FIXTURES held no sidecars: {dir}");
        eprintln!("{files} sidecar(s), {total_imported} mask(s) imported in total");
    }

    /// FORENSIC REGRESSION for R25 P9, on the real files — the census this
    /// batch's fix rests on, re-derived from the bytes every time it runs
    /// rather than quoted from a document. Same directory and same silent-skip
    /// rule as the two probes around it.
    ///
    /// Three claims, in order of how much they cost if false:
    ///  1. `crs:Flipped` is the COMPLEMENT of `crs:MaskInverted` on every
    ///     radial in the fixtures (16 `(true,false)` + 7 `(false,true)` = 23/23
    ///     across the 7 M-B sidecars, matching 201/201 over the whole library).
    ///     A fixture set that ever shows a MATCHING pair falsifies the model
    ///     this batch is built on, and this test is where that would surface.
    ///  2. No imported radial carries `flipped` — the inversion is read from
    ///     `crs:MaskInverted` alone. Before this batch the two were XORed and
    ///     the net came out `true` on every radial in every one of these files
    ///     (asserted below as the anti-regression: `!net_before == net_now`
    ///     would have to hold for the 16, which it does not).
    ///  3. Our writer hands the file its own pair back, attribute for
    ///     attribute — so a Lightroom → AutoShade → Lightroom trip renders the
    ///     same mask at both ends.
    #[test]
    fn real_lightroom_radials_carry_one_inversion_bit_spelled_twice() {
        let Some(dir) = crate::config::live_env("AUTOSHADE_MB_FIXTURES") else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            panic!("AUTOSHADE_MB_FIXTURES is set but unreadable: {dir}");
        };
        let (mut files, mut radials) = (0usize, 0usize);
        let (mut flip_true, mut flip_false) = (0usize, 0usize);
        for e in entries.flatten() {
            let p = e.path();
            if !p.to_string_lossy().to_lowercase().contains(".xmp") {
                continue;
            }
            let text = std::fs::read_to_string(&p)
                .unwrap_or_else(|e| panic!("{}: fixture unreadable ({e})", p.display()));
            files += 1;
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            // Scoped the way the importer scopes: this document's OWN
            // corrections. `crs:RetouchAreas` carries `Mask/*` components of
            // its own and is not a correction.
            let scope = crs_own_scope(&text);
            let block = owned_element_body(scope.as_ref(), "crs:MaskGroupBasedCorrections")
                .ok()
                .flatten()
                .unwrap_or_default();
            // CLAIM 1 — the census, on this file's bytes.
            let mut at = 0usize;
            let mut per_file = 0usize;
            while let Some((s, e, _)) = next_xml_tag(block, at) {
                at = e + 1;
                let tag = &block[s..=e];
                if xml_attribute_raw(tag, "crs:What").map(|(_, v)| xml_unescape(v))
                    != Some("Mask/CircularGradient".into())
                {
                    continue;
                }
                let tag = Tag::new(tag);
                let f = tag.crs_str("Flipped").map(|v| v.as_ref() == "true");
                let i = tag.crs_str("MaskInverted").map(|v| v.as_ref() == "true");
                let (Some(f), Some(i)) = (f, i) else {
                    panic!("{name}: a radial without both flags — {f:?} / {i:?}");
                };
                assert_ne!(
                    f, i,
                    "{name}: radial {per_file} carries Flipped={f} MaskInverted={i} — a MATCHING \
                     pair, which no radial in the 201-mask library census does. The one-bit model \
                     R25 P9 is built on does not hold on this file; do not paper over it."
                );
                if f { flip_true += 1 } else { flip_false += 1 }
                per_file += 1;
                radials += 1;
            }
            // CLAIMS 2 and 3 — what the importer and the writer do with them.
            let imported = xmp_to_recipe(&text);
            let round = xmp_to_recipe(&recipe_to_xmp(&imported));
            let mut seen = 0usize;
            for (i, m) in imported.masks.iter().enumerate() {
                let MaskGeometry::Radial { flipped, .. } = m.mask else { continue };
                assert!(
                    !flipped,
                    "{name}: mask {i} imported flipped — crs:Flipped reached the render flag"
                );
                let rt = round.masks.get(i).unwrap_or_else(|| panic!("{name}: mask {i} vanished"));
                assert_eq!(
                    lr_net_inverted(m),
                    lr_net_inverted(rt),
                    "{name}: mask {i} changed its inversion in the round trip"
                );
                seen += 1;
            }
            eprintln!(
                "{name}: {per_file} radial(s) in the file, {seen} imported, all anti-correlated"
            );
        }
        assert!(files > 0, "AUTOSHADE_MB_FIXTURES held no sidecars: {dir}");
        assert!(radials > 0, "no radial reached the census: {dir}");
        // The anti-regression, stated as the arithmetic that made the defect
        // visible: XORing the two flags gives `true` on EVERY radial here,
        // whatever the file says, because they are complements. That is what
        // the old importer did, and why it inverted the `Flipped=true` ones.
        eprintln!(
            "{radials} radial(s): {flip_true} Flipped=true (Lightroom does NOT invert these — \
             the {flip_true} the old importer inverted), {flip_false} Flipped=false"
        );
        assert_eq!(flip_true + flip_false, radials);
        assert!(
            flip_true > 0,
            "the fixtures hold no NOT-inverted radial, so they cannot witness the defect"
        );
    }

    /// FORENSIC REGRESSION for the B2 GLOBALS, same directory and same
    /// silent-skip rule as the mask probe above (the reference files are
    /// photographs and are not in this repository; the inline fixtures beside
    /// this one are synthetic by policy, so they prove the RULES and not the
    /// FILES).
    ///
    /// `DSC09568.xmp` is the strongest case in the user's library: global
    /// Texture +26 — the largest of the seven — beside a real post-crop
    /// vignette. Before B2 every one of those values imported as zero and the
    /// photo simply rendered differently from Lightroom.
    #[test]
    fn real_lightroom_sidecars_import_their_global_effects() {
        let Some(dir) = crate::config::live_env("AUTOSHADE_MB_FIXTURES") else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            panic!("AUTOSHADE_MB_FIXTURES is set but unreadable: {dir}");
        };
        let mut seen_texture = 0usize;
        let mut seen_effects = 0usize;
        let mut seen_auto_ca = 0usize;
        for e in entries.flatten() {
            let p = e.path();
            if !p.to_string_lossy().to_lowercase().contains(".xmp") {
                continue;
            }
            // NAMED, never skipped (R25 P8). `else { continue }` here meant a
            // sidecar this probe could not read simply left the count — and a
            // forensic probe whose files quietly stop arriving is a green
            // test that measures nothing. A `.xmp` in the fixture directory
            // that will not read as UTF-8 is a fact about the fixtures the
            // round report has to hear.
            let text = std::fs::read_to_string(&p)
                .unwrap_or_else(|e| panic!("{}: fixture unreadable ({e})", p.display()));
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            let r = xmp_to_recipe(&text);
            eprintln!(
                "{name}: texture {} · post-crop vignette {}/{}/{}/{}/{}/{} · grain {}/{}/{}",
                r.texture,
                r.post_crop_vignette,
                r.post_crop_vignette_mid,
                r.post_crop_vignette_feather,
                r.post_crop_vignette_round,
                r.post_crop_vignette_style,
                r.post_crop_vignette_hl,
                r.grain,
                r.grain_size,
                r.grain_rough,
            );
            // The B3 block, same forensic line: what the eight detail axes,
            // the CA pair and the six de-fringe keys actually came back as.
            // `sharpening` leads it since v0.31.1 — the ×1.5 that used to sit
            // on this read was the batch's headline defect, and the value it
            // produces is the number this probe exists to show. Six of the
            // seven reference files carry `crs:Sharpness="40"` and one carries
            // `"35"`; before the fix they imported as 60 and 52.5.
            eprintln!(
                "    sharpening {} · detail {}/{}/{} · nr {}/{} · colour nr {}/{}/{} · \
                 ca {}/{} auto {} · defringe {}/{}/{} {}/{}/{}",
                r.sharpening,
                r.sharpen_radius,
                r.sharpen_detail,
                r.sharpen_mask,
                r.nr_detail,
                r.nr_contrast,
                r.color_nr,
                r.color_nr_detail,
                r.color_nr_smooth,
                r.ca_r,
                r.ca_b,
                r.auto_lateral_ca,
                r.defringe_purple,
                r.defringe_purple_lo,
                r.defringe_purple_hi,
                r.defringe_green,
                r.defringe_green_lo,
                r.defringe_green_hi,
            );
            // The de-fringe block is the ONE with a non-zero neutral, and
            // every file in this library carries it at Adobe's defaults — so
            // the fallback is exercised on real bytes here, not only on the
            // synthetic fixtures. A reader that took `crs_f32`'s absent-key
            // zero would land on 0/0 and fail this on all seven.
            assert_eq!(
                (
                    r.defringe_purple_lo,
                    r.defringe_purple_hi,
                    r.defringe_green_lo,
                    r.defringe_green_hi
                ),
                (30.0, 70.0, 40.0, 60.0),
                "{name}: the real de-fringe hue windows must import as themselves"
            );
            // Two named forensic cases from the first-hand scan of these
            // files: every one writes `SharpenRadius="+1.0"`, and DSC09034 is
            // the only one whose auto-CA switch is on.
            assert_eq!(r.sharpen_radius, 1.0, "{name}: crs:SharpenRadius=\"+1.0\" must import as 1.0");
            if name.starts_with("DSC09034") {
                assert!(r.auto_lateral_ca, "{name}: crs:AutoLateralCA=\"1\" must import as on");
                seen_auto_ca += 1;
            }
            // The named case, asserted exactly.
            if name.starts_with("DSC09568") {
                assert_eq!(r.texture, 26.0, "{name}: crs:Texture=\"+26\" must import as 26");
                assert_eq!(r.post_crop_vignette, -17.0, "{name}: its post-crop vignette too");
                assert_eq!(r.post_crop_vignette_style, 1.0, "{name}: Highlight Priority");
                seen_texture += 1;
            }
            if r.texture != 0.0 || r.post_crop_vignette != 0.0 || r.grain != 0.0 {
                seen_effects += 1;
            }
            // Whatever the values are, they must SURVIVE a save: the merge
            // strips these keys now, so a read/write asymmetry would delete
            // them from the file beside the RAW.
            if let Some(merged) = merge_recipe_into_xmp(&text, &r) {
                let round = xmp_to_recipe(&merged.doc);
                assert_eq!(round.texture, r.texture, "{name}: Texture lost on merge");
                assert_eq!(
                    (round.post_crop_vignette, round.grain),
                    (r.post_crop_vignette, r.grain),
                    "{name}: a carried effect was lost on merge"
                );
                assert_eq!(
                    (round.sharpen_radius, round.color_nr, round.auto_lateral_ca),
                    (r.sharpen_radius, r.color_nr, r.auto_lateral_ca),
                    "{name}: a B3 carried detail value was lost on merge"
                );
                assert_eq!(
                    (round.defringe_purple_lo, round.defringe_green_hi),
                    (r.defringe_purple_lo, r.defringe_green_hi),
                    "{name}: the de-fringe hue windows were lost on merge"
                );
            }
        }
        assert!(
            seen_texture > 0,
            "DSC09568.xmp was not in {dir} — the named forensic case never ran"
        );
        assert!(
            seen_auto_ca > 0,
            "DSC09034.xmp was not in {dir} — the B3 auto-CA forensic case never ran"
        );
        eprintln!("{seen_effects} sidecar(s) carried a non-neutral B2 effect");
    }

    /// FORENSIC REGRESSION for **the R25 P8 root cause**, on the seven real
    /// sidecars and in the exact shape the defect takes in the field: a v0.30
    /// `recipe.json` (no `schema_era`, no field for any of the twenty-seven
    /// R25 keys, no masks — that build could not import one) saved back over
    /// the Lightroom file it came from.
    ///
    /// Same directory and same silent-skip rule as the probes above. This is
    /// where the numbers in the round report come from: before the fix, four
    /// corrections were destroyed on DSC09034, eight on DSC09642, and nine
    /// global keys on each of the three files that carry them — every one of
    /// them silently, with an empty note list.
    #[test]
    fn real_lightroom_sidecars_survive_a_v0_30_recipe() {
        let Some(dir) = crate::config::live_env("AUTOSHADE_MB_FIXTURES") else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            panic!("AUTOSHADE_MB_FIXTURES is set but unreadable: {dir}");
        };
        let (mut files, mut masks_held, mut keys_held) = (0usize, 0usize, 0usize);
        for e in entries.flatten() {
            let p = e.path();
            if !p.to_string_lossy().to_lowercase().contains(".xmp") {
                continue;
            }
            let text = std::fs::read_to_string(&p)
                .unwrap_or_else(|e| panic!("{}: fixture unreadable ({e})", p.display()));
            files += 1;
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            let live = xmp_to_recipe(&text);
            let legacy = EditRecipe { masks: Vec::new(), ..as_v0_30_recipe(&live) };
            assert_eq!(legacy.schema_era, 0);
            let Some(out) = merge_recipe_into_xmp(&text, &legacy) else {
                panic!("{name}: the reference sidecars are all mergeable");
            };

            // 1) The mask block, byte for byte.
            let block = text
                .find("<crs:MaskGroupBasedCorrections>")
                .zip(text.find("</crs:MaskGroupBasedCorrections>"))
                .map(|(s, e)| &text[s..e + "</crs:MaskGroupBasedCorrections>".len()]);
            if let Some(original) = block {
                let corrections = original.matches("crs:What=\"Correction\"").count();
                assert!(
                    out.doc.contains(original),
                    "{name}: {corrections} correction(s) did not survive a v0.30 save"
                );
                masks_held += corrections;
            }

            // 2) Every one of the twenty-seven keys the recipe never had: the
            //    VALUE the document arrived with must read back unchanged.
            let round = xmp_to_recipe(&out.doc);
            for (control, key) in r25_attr_keys() {
                let (before, after) = (
                    crate::advisor::catalogue::global_value(&live, control),
                    crate::advisor::catalogue::global_value(&round, control),
                );
                assert_eq!(before, after, "{name}: crs:{key} changed on a v0.30 save");
                if text.contains(&format!("crs:{key}=")) {
                    assert_eq!(
                        out.doc.matches(&format!("crs:{key}=")).count(),
                        1,
                        "{name}: crs:{key} must appear exactly once"
                    );
                    keys_held += 1;
                }
            }
            // 3) …and none of it is a silent success by way of an empty file.
            assert!(out.notes.is_empty(), "{name}: nothing was replaced: {:?}", out.notes);
        }
        assert!(files > 0, "AUTOSHADE_MB_FIXTURES held no sidecars: {dir}");
        eprintln!(
            "{files} sidecar(s): {masks_held} correction(s) and {keys_held} R25 key(s) held \
             through a v0.30-shaped save"
        );
    }

    /// FORENSIC REGRESSION for the B4 PASS-THROUGH blocks, same directory and
    /// same silent-skip rule as the two probes above.
    ///
    /// This is the one batch whose whole promise is "the bytes come back",
    /// and synthetic fixtures cannot prove it: the spellings are the point,
    /// and only Lightroom writes them. First-hand from the seven reference
    /// sidecars — all eight Perspective keys on every file, `CameraProfile`
    /// on every file, and NOT ONE Calibration key anywhere (Lightroom omits
    /// the block at its defaults), which is exactly why an absent key must
    /// stay absent instead of being invented at some neutral we chose.
    #[test]
    fn real_lightroom_sidecars_pass_their_transform_blocks_through() {
        let Some(dir) = crate::config::live_env("AUTOSHADE_MB_FIXTURES") else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            panic!("AUTOSHADE_MB_FIXTURES is set but unreadable: {dir}");
        };
        let mut files = 0usize;
        let mut seen_profile = 0usize;
        let mut seen_upright = 0usize;
        for e in entries.flatten() {
            let p = e.path();
            if !p.to_string_lossy().to_lowercase().contains(".xmp") {
                continue;
            }
            // NAMED, never skipped (R25 P8). `else { continue }` here meant a
            // sidecar this probe could not read simply left the count — and a
            // forensic probe whose files quietly stop arriving is a green
            // test that measures nothing. A `.xmp` in the fixture directory
            // that will not read as UTF-8 is a fact about the fixtures the
            // round report has to hear.
            let text = std::fs::read_to_string(&p)
                .unwrap_or_else(|e| panic!("{}: fixture unreadable ({e})", p.display()));
            files += 1;
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            let r = xmp_to_recipe(&text);
            let shown: Vec<String> =
                PASSTHROUGH_CRS.iter().filter_map(|k| r.passthrough.get(*k).map(|v| format!("{k}={v:?}"))).collect();
            eprintln!("{name}: {} passthrough key(s) — {}", shown.len(), shown.join(" "));
            if r.passthrough.contains_key("CameraProfile") {
                seen_profile += 1;
            }
            if r.passthrough.contains_key("PerspectiveUpright") {
                seen_upright += 1;
            }
            // VERBATIM, on real bytes: every value that arrived must reach the
            // merged document as the identical string, and come back as the
            // identical string. A formatter anywhere in this path would show
            // up here as `+0.9` → `0.9` or `0.00` → `0`.
            //
            // Counted inside `crs_own_scope`, and this probe is what proved
            // the distinction matters: every one of the reference sidecars
            // carries a SECOND `crs:CameraProfile` inside its creative Look's
            // baked `<crs:Parameters>`, which the merge preserves on purpose
            // (`top_level_owned_spans` strips top-level properties only) and
            // the reader is already scoped away from. A flat count over the
            // whole document reports two and is reading someone else's
            // settings block as our duplicate.
            if let Some(merged) = merge_recipe_into_xmp(&text, &r) {
                let scope = crs_own_scope(&merged.doc);
                for (k, v) in &r.passthrough {
                    assert!(
                        scope.contains(&format!("crs:{k}=\"{v}\"")),
                        "{name}: crs:{k} did not reach the merged document as {v:?}"
                    );
                    assert_eq!(
                        scope.matches(&format!("crs:{k}=")).count(),
                        1,
                        "{name}: crs:{k} was written twice — the strip missed the original"
                    );
                }
                assert_eq!(
                    xmp_to_recipe(&merged.doc).passthrough,
                    r.passthrough,
                    "{name}: the pass-through block did not survive its own round trip"
                );
            }
        }
        assert!(files > 0, "AUTOSHADE_MB_FIXTURES held no sidecars: {dir}");
        assert_eq!(seen_profile, files, "every reference sidecar carries crs:CameraProfile");
        assert_eq!(seen_upright, files, "…and the whole Perspective block");
    }

    /// R25 P0-0.1: the bands `unparsable_crs_numbers` judges a document by ARE
    /// the control registry's, not a second hand-written copy — so a new
    /// attribute row arrives with its check already wired, and the families one
    /// row cannot state are the only hand-written numbers left.
    ///
    /// v0.31.1 removed the third residue. `Sharpness` needed its own band only
    /// because the reader scaled it; now that the key is 1:1 with the recipe
    /// row, the row's own 0..150 IS the document's band — the special case died
    /// of the evidence, which is the shape a correct derivation should take.
    #[test]
    fn import_bands_are_the_registry_bands() {
        use crate::advisor::catalogue::RECIPE_CONTROLS;
        let mut checked = 0;
        for c in RECIPE_CONTROLS.iter() {
            let (Some(key), Some((lo, hi))) = (c.crs.attr(), c.range) else { continue };
            checked += 1;
            // `Sharpness` used to be excepted here, on a hand-written 0..100
            // sidecar band — v0.31.1 deleted the exception along with the
            // scale it stood for, so this key now derives like every other.
            // A full span outside each end (never a multiple of the bound: for
            // 2000..40000, `lo * 10` lands back INSIDE).
            let span = (hi - lo).max(1.0);
            assert!(crs_number_is_in_recipe_range(key, lo), "{key}: {lo} is the row's own floor");
            assert!(crs_number_is_in_recipe_range(key, hi), "{key}: {hi} is the row's own ceiling");
            assert!(!crs_number_is_in_recipe_range(key, hi + span), "{key}: above {hi} is out");
            assert!(!crs_number_is_in_recipe_range(key, lo - span), "{key}: below {lo} is out");
        }
        assert!(checked >= 15, "the registry stopped naming attribute rows: {checked}");
        // The three residues the registry cannot state, each derived from the
        // clamp that enforces it.
        for (key, inside, outside) in [
            ("SplitToningShadowHue", 359.0, 361.0),   // ColorGrade::clamp hue 0..360
            ("ColorGradeGlobalSat", 100.0, -1.0),     //                  sat 0..100
            ("ColorGradeBlending", 0.0, 101.0),       //             blending 0..100
            ("SplitToningBalance", -100.0, -101.0),   //              balance ±100
            ("ColorGradeShadowLum", 100.0, 101.0),    //                  lum ±100
            ("CropRight", 1.0, 1.5),                  // Crop::clamp 0..1
            ("HueAdjustmentRed", -100.0, 101.0),      // Hsl::clamp ±100
        ] {
            assert!(crs_number_is_in_recipe_range(key, inside), "{key}: {inside} must be legal");
            assert!(!crs_number_is_in_recipe_range(key, outside), "{key}: {outside} must not be");
        }
        // …and the derivation is what the DISCLOSURE reads: a Contrast2012
        // outside the `contrast` row's band is named, one inside is not.
        let doc = |v: &str| {
            format!(
                "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                 xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                 <rdf:Description rdf:about=\"\" \
                 xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
                 crs:Contrast2012=\"{v}\"/></rdf:RDF></x:xmpmeta>"
            )
        };
        assert!(unparsable_crs_numbers(&doc("150")).iter().any(|k| k == "Contrast2012"));
        assert!(unparsable_crs_numbers(&doc("50")).is_empty(), "an in-band value says nothing");
    }

    #[test]
    fn renders_range_masks_as_intersected_components() {
        use crate::recipe::RangeMask;
        let r = EditRecipe {
            masks: vec![
                LocalAdjustment {
                    mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.35, full_x: 0.5, full_y: 0.0 },
                    range: Some(RangeMask::Luminance { lo_outer: 0.4, lo: 0.5, hi: 1.0, hi_outer: 1.0 }),
                    name: "sky".into(),
                    highlights: -40.0,
                    ..Default::default()
                },
                LocalAdjustment {
                    mask: MaskGeometry::Radial {
                        top: 0.3, left: 0.35, bottom: 0.7, right: 0.65,
                        feather: 0.5, roundness: 0.0, flipped: false, angle: 0.0,
                        midpoint: 50.0, mask_version: 2,
                    },
                    range: Some(RangeMask::Color { r: 0.9, g: 0.6, b: 0.2, amount: 0.5, px: 0.4, py: 0.7 }),
                    name: "subject".into(),
                    saturation: 20.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        // Both range components present, encoded as intersections (the decoded
        // ACR algebra: BlendMode 1 + Inverted true + Value 0 = keep only where
        // the range matches).
        assert_eq!(xmp.matches(r#"crs:What="Mask/RangeMask""#).count(), 2);
        assert_eq!(
            xmp.matches(r#"crs:MaskBlendMode="1" crs:MaskInverted="true""#).count(), 2
        );
        // Luminance: attribute form, LumRange in ACR's 4-number trapezoid.
        assert!(xmp.contains(r#"crs:Type="2""#));
        assert!(xmp.contains(r#"crs:LumRange="0.400000 0.500000 1.000000 1.000000""#));
        // Colour: child-element form with one PointModels entry.
        assert!(xmp.contains(r#"crs:Type="1""#));
        assert!(xmp.contains(r#"crs:ColorAmount="0.500000""#));
        assert!(xmp.contains("<rdf:li>0.900000 0.600000 0.200000 0.400000 0.700000 0</rdf:li>"));
        // A mask WITHOUT a range emits no RangeMask component at all.
        let plain = EditRecipe {
            masks: vec![LocalAdjustment { name: "plain".into(), ..Default::default() }],
            ..Default::default()
        };
        assert!(!recipe_to_xmp(&plain).contains("RangeMask"));
    }

    #[test]
    fn renders_expected_crs_keys() {
        let r = EditRecipe {
            exposure_ev: 0.32,
            contrast: 14.0,
            highlights: -12.0,
            temperature_k: Some(5600.0),
            tint: 3.0,
            sharpening: 45.0, // -> Sharpness 45, 1:1
            tone_curve: vec![
                CurvePoint { input: 0, output: 0 },
                CurvePoint { input: 255, output: 255 },
            ],
            rationale: "warm & contrasty <test> & \"q\"".into(),
            confidence: 0.82,
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        assert!(xmp.contains(r#"crs:ProcessVersion="15.4""#));
        assert!(xmp.contains(r#"crs:Exposure2012="0.32""#));
        assert!(xmp.contains(r#"crs:Contrast2012="+14""#));
        assert!(xmp.contains(r#"crs:Highlights2012="-12""#));
        assert!(xmp.contains(r#"crs:WhiteBalance="Custom""#));
        assert!(xmp.contains(r#"crs:Temperature="5600""#));
        // 1:1 since v0.31.1 — a rendered 45 is written as 45. It used to be
        // written as 30, i.e. what the user saw was not what the sidecar said.
        assert!(xmp.contains(r#"crs:Sharpness="45""#));
        assert!(xmp.contains("<crs:ToneCurvePV2012>"));
        assert!(xmp.contains("<rdf:li>0, 0</rdf:li>"));
        // rationale is XML-escaped in the comment
        assert!(xmp.contains("&lt;test&gt;"));
    }

    #[test]
    fn tint_only_edit_on_a_stamped_photo_pins_custom_at_as_shot() {
        // Stamped photo, tint-only: Custom AT the as-shot Kelvin — Lightroom
        // then applies the Tint instead of ignoring it under "As Shot".
        let r = EditRecipe { tint: 15.0, as_shot_k: Some(4820.0), ..Default::default() };
        let xmp = recipe_to_xmp(&r);
        assert!(xmp.contains(r#"crs:WhiteBalance="Custom""#), "{xmp}");
        assert!(xmp.contains(r#"crs:Temperature="4820""#), "{xmp}");
        assert!(xmp.contains(r#"crs:Tint="+15""#), "{xmp}");
        // Round trip: the import reads Custom back as an absolute target ==
        // the stamp, which the anchored engine renders as "no Kelvin shift".
        let back = xmp_to_recipe(&xmp);
        assert_eq!(back.temperature_k, Some(4820.0));
        assert_eq!(back.tint, 15.0);
        // A legacy recipe (no stamp) keeps the old honest fallback.
        let legacy = EditRecipe { tint: 15.0, ..Default::default() };
        let xmp = recipe_to_xmp(&legacy);
        assert!(xmp.contains(r#"crs:WhiteBalance="As Shot""#), "{xmp}");
        assert!(xmp.contains(r#"crs:Tint="+15""#), "{xmp}");
        // The engine-only stamp itself NEVER appears in a sidecar.
        assert!(!xmp.contains("as_shot"), "{xmp}");
    }

    /// Every sidecar already on a user's disk was stamped `x:xmptk="Autoshop"`
    /// or `"Autoshop 2"`, and that token is a RENDERING decision, not a label:
    /// era-2 Temperature is absolute, era-1 is relative to the 5500 K anchor.
    /// Reading a pre-rename era-2 document as era-1 would pin 5500 onto an
    /// absolute value and shift the white balance of every develop the user
    /// ever saved.
    ///
    /// MUTATION: drop `XMPTK_ERA2_PRE_RENAME` from `is_autoshade_era2` and the
    /// pre-rename era-2 document below comes back with `as_shot_k` pinned.
    #[test]
    fn a_pre_rename_sidecar_keeps_its_era_and_its_provenance() {
        let doc = |tk: &str, temp: &str| {
            format!(
                r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="{tk}">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    crs:WhiteBalance="Custom"
    crs:Temperature="{temp}"
    crs:HasSettings="True">
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#
            )
        };

        // Era 2 under BOTH spellings: absolute Kelvin, no anchor pin.
        for tk in ["AutoShade 2", "Autoshop 2"] {
            let r = xmp_to_recipe(&doc(tk, "5000"));
            assert!(is_autoshade_era2(&doc(tk, "5000")), "{tk} lost its era-2 marker");
            assert!(is_autoshade_sidecar(&doc(tk, "5000")), "{tk} lost its provenance");
            assert_eq!(r.temperature_k, Some(5000.0), "{tk}");
            assert_eq!(r.as_shot_k, None, "{tk}: era-2 Kelvin is absolute — no pin");
        }
        // Era 1 under BOTH spellings: ours, and pinned to the legacy anchor.
        for tk in ["AutoShade", "Autoshop"] {
            let r = xmp_to_recipe(&doc(tk, "5000"));
            assert!(!is_autoshade_era2(&doc(tk, "5000")), "{tk} claimed era 2");
            assert!(is_autoshade_sidecar(&doc(tk, "5000")), "{tk} lost its provenance");
            assert_eq!(r.as_shot_k, Some(5500.0), "{tk}: era-1 pins the legacy anchor");
        }
        // A foreign toolkit is still foreign, and a prefix of ours is not ours.
        assert!(!is_autoshade_sidecar(&doc("Adobe XMP Core 7.0-c000", "5000")));
        assert!(!is_autoshade_sidecar(&doc("AutoShadester", "5000")));
        assert!(!is_autoshade_sidecar(&doc("Autoshopping", "5000")));
    }

    /// A merge rewrites the owned white-balance attributes in ABSOLUTE
    /// semantics, so it must leave an era-2 marker behind whichever era-1
    /// spelling it found — under the CURRENT name, since that is what this
    /// build writes.
    ///
    /// MUTATION: upgrade only the current era-1 spelling and a pre-rename
    /// document keeps its era-1 marker, so the next import pins 5500 onto the
    /// absolute values this merge just wrote.
    #[test]
    fn a_pre_rename_era_marker_upgrades_when_the_merge_makes_it_absolute() {
        for tk in ["AutoShade", "Autoshop"] {
            let doc = format!(r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="{tk}">"#);
            let up = upgrade_era_marker(doc);
            assert_eq!(
                up, r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="AutoShade 2">"#,
                "{tk} did not reach the current era-2 marker"
            );
            assert!(is_autoshade_era2(&up), "{tk} upgraded to something unreadable");
        }
        // Already era 2 under either spelling: untouched, never double-upgraded.
        for tk in ["AutoShade 2", "Autoshop 2"] {
            let doc = format!(r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="{tk}">"#);
            assert_eq!(upgrade_era_marker(doc.clone()), doc, "{tk} was double-upgraded");
        }
    }

    /// The rationale comment is an on-disk token too: a pre-rename sidecar
    /// carries `<!-- Generated by Autoshop. AI rationale: … -->`, and a merge
    /// that cannot find it leaves the OLD reasoning attached to a NEW recipe.
    ///
    /// MUTATION: look for the current mark only and the pre-rename document
    /// below keeps its stale rationale.
    #[test]
    fn a_pre_rename_rationale_comment_is_still_refreshed() {
        let fresh = EditRecipe { confidence: 0.5, rationale: "the new reason".into(), ..Default::default() };
        for mark in ["AutoShade", "Autoshop"] {
            let doc = format!("<x:xmpmeta><!-- Generated by {mark}. AI rationale: the stale reason (confidence 0.90) -->\n</x:xmpmeta>");
            let out = refresh_rationale_comment(doc, &fresh);
            assert!(out.contains("the new reason"), "{mark}: the stale rationale survived");
            assert!(!out.contains("the stale reason"), "{mark}: both rationales are present");
            assert!(
                out.contains("<!-- Generated by AutoShade. AI rationale: "),
                "{mark}: the refreshed comment must carry the current name"
            );
        }
    }

    #[test]
    fn legacy_autoshade_sidecar_kelvin_stays_relative_via_the_anchor_pin() {
        // A sidecar WE wrote before the absolute-Kelvin engine: its
        // Temperature was tuned against the 5500 K anchor. The import pins
        // the anchor there, so every stamp-if-None caller leaves it alone
        // and the develop renders exactly as tuned.
        let old = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="AutoShade">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    crs:WhiteBalance="Custom"
    crs:Temperature="5000"
    crs:HasSettings="True">
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let r = xmp_to_recipe(old);
        assert_eq!(r.temperature_k, Some(5000.0));
        assert_eq!(r.as_shot_k, Some(5500.0), "old-era Kelvin pins the legacy anchor");
        assert_eq!(r.as_shot_tint, None, "the pin claims no camera as-shot");
        // Era-2 documents (what this build writes) stay unpinned — their
        // Temperature is absolute and the caller stamps the real camera K.
        let new =
            recipe_to_xmp(&EditRecipe { temperature_k: Some(5000.0), ..Default::default() });
        assert!(new.contains(r#"x:xmptk="AutoShade 2""#), "{new}");
        let r2 = xmp_to_recipe(&new);
        assert_eq!(r2.temperature_k, Some(5000.0));
        assert_eq!(r2.as_shot_k, None, "era-2 Kelvin is absolute — no pin");
        // Foreign (Lightroom) sidecars are never pinned either.
        let lr = old.replace("AutoShade", "Adobe XMP Core 7.0-c000");
        assert_eq!(xmp_to_recipe(&lr).as_shot_k, None, "foreign Kelvin is absolute");
        // A6 disclosure scanner: corrupt numbers are NAMED; parsable and
        // string-typed keys never flag; our own writer round-trips clean.
        let corrupt = old
            .replace(r#"crs:Temperature="5000""#, r#"crs:Temperature="fivethousand""#)
            .replace(
                r#"crs:HasSettings="True""#,
                "crs:Contrast2012=\"NaNny\"\n    crs:Exposure2012=\"+0.65\"\n    crs:HasSettings=\"True\"",
            );
        let bad = unparsable_crs_numbers(&corrupt);
        assert!(bad.contains(&"Temperature".to_string()), "{bad:?}");
        assert!(bad.contains(&"Contrast2012".to_string()), "{bad:?}");
        assert!(!bad.contains(&"Exposure2012".to_string()), "{bad:?}");
        assert!(!bad.iter().any(|k| k == "WhiteBalance" || k == "HasSettings"), "{bad:?}");
        assert_eq!(xmp_to_recipe(&corrupt).contrast, 0.0, "the silent neutral being disclosed");
        let clean = recipe_to_xmp(&EditRecipe {
            exposure_ev: 0.4,
            temperature_k: Some(5600.0),
            ..Default::default()
        });
        assert!(unparsable_crs_numbers(&clean).is_empty());
        // A MERGE into an old AutoShade document rewrites the WB attributes in
        // absolute semantics — the era marker must upgrade with them.
        let merged = merged_doc(
            old,
            &EditRecipe { temperature_k: Some(6200.0), ..Default::default() },
        )
        .expect("mergeable");
        assert!(merged.contains(r#"x:xmptk="AutoShade 2""#), "{merged}");
        assert!(!merged.contains(r#"x:xmptk="AutoShade""#) || merged.contains("AutoShade 2"));
        assert_eq!(xmp_to_recipe(&merged).as_shot_k, None, "upgraded doc is not pinned");
    }

    #[test]
    fn non_finite_numbers_import_neutral_and_are_disclosed() {
        // Rust's f32 parser accepts "NaN" and "inf"; no real sidecar writer
        // emits them. They must import as neutral AND be named by the
        // disclosure scanner — the old exact-parse mirror read them as
        // "fine", so the silent neutral was never disclosed.
        let lr = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    crs:WhiteBalance="Custom"
    crs:Temperature="NaN"
    crs:Contrast2012="inf"
    crs:Exposure2012="+0.65"
    crs:HasSettings="True">
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let lr_scope = Scope::new(lr);
        assert_eq!(lr_scope.crs_f32("Temperature"), None, "NaN is not a Kelvin");
        assert_eq!(lr_scope.crs_f32("Contrast2012"), None, "inf is not a slider");
        let r = xmp_to_recipe(lr);
        assert_eq!(r.temperature_k, None);
        assert_eq!(r.contrast, 0.0);
        assert_eq!(r.exposure_ev, 0.65, "finite neighbours still import");
        let bad = unparsable_crs_numbers(lr);
        assert!(bad.contains(&"Temperature".to_string()), "{bad:?}");
        assert!(bad.contains(&"Contrast2012".to_string()), "{bad:?}");
        assert!(!bad.contains(&"Exposure2012".to_string()), "{bad:?}");
    }

    /// 16-lane scan L05: "999, -5" used to saturate to (255, 0) — a one-point
    /// master curve that renders nearly black, imported silently and
    /// PERSISTED by the next save. Out-of-domain now takes the same
    /// reject-and-disclose path as a malformed point.
    #[test]
    fn out_of_domain_curve_points_drop_the_curve_and_are_disclosed() {
        let lr = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    crs:Exposure2012="+0.30"
    crs:HasSettings="True">
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>999, -5</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let r = xmp_to_recipe(lr);
        assert!(r.tone_curve.is_empty(), "the out-of-domain curve must not import");
        assert_eq!(r.exposure_ev, 0.30, "finite neighbours still import");
        let bad = unparsable_crs_numbers(lr);
        assert!(bad.contains(&"ToneCurvePV2012".to_string()), "{bad:?}");
        // In-domain float spellings keep rounding like before (a non-identity
        // pair — the 0,0→255,255 identity deliberately collapses to empty).
        assert_eq!(
            parse_curve_checked("<crs:T><rdf:Seq><rdf:li>0, 10</rdf:li><rdf:li>254.6, 255</rdf:li></rdf:Seq></crs:T>", "T"),
            Ok(vec![
                CurvePoint { input: 0, output: 10 },
                CurvePoint { input: 255, output: 255 }
            ])
        );
    }

    /// 16-lane scan L05: a wheel whose HUE is unreadable must not keep its
    /// paired saturation — the zero fallback made "bogus" hue 0 (= RED) and
    /// a valid Saturation of 50 imported as a strong red grade while the
    /// disclosure claimed neutral restoration.
    #[test]
    fn an_unreadable_wheel_hue_zeroes_its_paired_saturation() {
        let lr = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    crs:SplitToningShadowHue="bogus"
    crs:SplitToningShadowSaturation="50"
    crs:SplitToningHighlightHue="45"
    crs:SplitToningHighlightSaturation="20"
    crs:HasSettings="True">
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let r = xmp_to_recipe(lr);
        assert_eq!(
            r.color_grade.shadow_sat, 0.0,
            "an unreadable shadow hue must take its saturation with it"
        );
        assert_eq!(r.color_grade.highlight_hue, 45.0, "the healthy wheel is untouched");
        assert_eq!(r.color_grade.highlight_sat, 20.0);
        assert!(
            unparsable_crs_numbers(lr).contains(&"SplitToningShadowHue".to_string()),
            "and the unreadable hue is named"
        );
    }

    #[test]
    fn renders_hsl_bands_only_when_set() {
        let r = EditRecipe {
            hsl: crate::recipe::Hsl {
                hue: [0.0, 15.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], // orange +15
                saturation: [0.0, 0.0, 0.0, -40.0, 0.0, 0.0, 0.0, 0.0], // green -40
                ..Default::default()
            },
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        assert!(xmp.contains(r#"crs:HueAdjustmentOrange="+15""#));
        assert!(xmp.contains(r#"crs:SaturationAdjustmentGreen="-40""#));
        assert!(xmp.contains(r#"crs:LuminanceAdjustmentRed="0""#)); // full 24-key block
        // A neutral recipe emits NO HSL keys (minimal, v1-compatible sidecar).
        assert!(!recipe_to_xmp(&EditRecipe::default()).contains("HueAdjustment"));
    }

    #[test]
    fn renders_color_grade_with_verified_split_toning_mapping() {
        let r = EditRecipe {
            color_grade: crate::recipe::ColorGrade {
                shadow_hue: 220.0, shadow_sat: 30.0,
                highlight_hue: 45.0, highlight_sat: 20.0,
                midtone_lum: -10.0, balance: 15.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        // shadow/highlight hue+sat round-trip via the legacy SplitToning* keys
        assert!(xmp.contains(r#"crs:SplitToningShadowHue="220""#));
        assert!(xmp.contains(r#"crs:SplitToningShadowSaturation="30""#));
        assert!(xmp.contains(r#"crs:SplitToningHighlightHue="45""#));
        assert!(xmp.contains(r#"crs:SplitToningBalance="+15""#));
        // lum / midtone / global / blending via ColorGrade*
        assert!(xmp.contains(r#"crs:ColorGradeMidtoneLum="-10""#));
        assert!(xmp.contains(r#"crs:ColorGradeBlending="50""#)); // ACR default
        // A neutral recipe emits NO grading keys at all.
        let neutral = recipe_to_xmp(&EditRecipe::default());
        assert!(!neutral.contains("ColorGrade") && !neutral.contains("SplitToning"));
    }

    #[test]
    fn renders_per_channel_rgb_curves() {
        let r = EditRecipe {
            red_curve: vec![CurvePoint { input: 0, output: 10 }, CurvePoint { input: 255, output: 250 }],
            blue_curve: vec![
                CurvePoint { input: 0, output: 0 },
                CurvePoint { input: 128, output: 110 },
                CurvePoint { input: 255, output: 255 },
            ],
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        assert!(xmp.contains("<crs:ToneCurvePV2012Red>"));
        assert!(xmp.contains("<rdf:li>0, 10</rdf:li>"));
        assert!(xmp.contains("<crs:ToneCurvePV2012Blue>"));
        assert!(xmp.contains("<rdf:li>128, 110</rdf:li>"));
        // The empty green channel emits no element.
        assert!(!xmp.contains("ToneCurvePV2012Green"));
        // A neutral recipe emits no per-channel curves at all.
        assert!(!recipe_to_xmp(&EditRecipe::default()).contains("ToneCurvePV2012Red"));
    }

    // ── merge (merge_recipe_into_xmp) ────────────────────────────────────────

    #[test]
    fn merge_preserves_lightroom_only_properties() {
        let lr = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 7.0-c000\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
    crs:Version=\"15.5.1\"\n\
    crs:ProcessVersion='15.4'\n\
    crs:PointColor=\"0\"\n\
    crs:CameraProfile=\"Adobe Color\"\n\
    crs:LensProfileEnable=\"1\"\n\
    crs:LensProfileName=\"Sony FE 24-70 > special\"\n\
    crs:Exposure2012=\"+1.00\"\n\
    crs:HasSettings=\"True\">\n\
   <crs:ToneCurvePV2012>\n\
    <rdf:Seq>\n\
     <rdf:li>0, 0</rdf:li>\n\
     <rdf:li>255, 255</rdf:li>\n\
    </rdf:Seq>\n\
   </crs:ToneCurvePV2012>\n\
   <crs:Look>\n\
    <rdf:Description crs:Name=\"Adobe Color\" crs:Amount=\"1\"/>\n\
   </crs:Look>\n\
  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n";
        let r = EditRecipe {
            exposure_ev: 0.25,
            contrast: 12.0,
            tone_curve: vec![
                CurvePoint { input: 0, output: 10 },
                CurvePoint { input: 255, output: 250 },
            ],
            ..Default::default()
        };
        let merged = merged_doc(lr, &r).expect("a plain LR sidecar is mergeable");
        // Everything AutoShade does not model survives. (The sample used to be
        // `crs:Texture`; R25 B2 models it, so it is no longer an example of an
        // unmodelled global — it is an example of an owned one, which
        // `a_cleared_texture_disappears_from_a_merged_document` covers.)
        assert!(merged.contains("crs:PointColor=\"0\""), "an unmodelled global survives");
        assert!(merged.contains("crs:CameraProfile=\"Adobe Color\""), "camera profile survives");
        assert!(
            merged.contains("crs:LensProfileName=\"Sony FE 24-70 > special\""),
            "LR lens profile survives — even with '>' inside the value"
        );
        assert!(merged.contains("<crs:Look>"), "LR-only child elements survive");
        assert!(merged.contains("xmlns:dc="), "foreign namespaces survive");
        assert!(merged.starts_with("<?xpacket"), "the xpacket wrapper survives");
        // Ours REPLACE, never duplicate — including the single-quoted form
        // (legal XML; leaving it would duplicate the attribute).
        assert_eq!(merged.matches("crs:Exposure2012=").count(), 1);
        assert_eq!(merged.matches("crs:ProcessVersion=").count(), 1);
        assert!(merged.contains("crs:ProcessVersion=\"15.4\""), "replaced in OUR form");
        assert!(merged.contains("crs:Exposure2012=\"0.25\""));
        assert_eq!(merged.matches("<crs:ToneCurvePV2012>").count(), 1);
        assert!(merged.contains("<rdf:li>0, 10</rdf:li>"), "OUR curve, not Lightroom's");
        // The reader sees OUR values in the merged document.
        let back = xmp_to_recipe(&merged);
        assert_eq!((back.exposure_ev, back.contrast), (0.25, 12.0));
        // A second merge over the merged document stays single AND a cleared
        // curve REMOVES the block (a stale slider must not linger).
        let r2 = EditRecipe { exposure_ev: -0.5, ..Default::default() };
        let merged2 = merged_doc(&merged, &r2).expect("re-mergeable");
        assert_eq!(merged2.matches("crs:Exposure2012=").count(), 1);
        assert!(merged2.contains("crs:Exposure2012=\"-0.50\""));
        assert!(merged2.contains("crs:PointColor=\"0\""), "still there after a second merge");
        assert_eq!(merged2.matches("<crs:ToneCurvePV2012>").count(), 0, "cleared curve gone");
        assert!(merged2.contains("ToneCurveName2012=\"Linear\""));
    }

    #[test]
    fn merge_strips_owned_element_form_properties() {
        // Lightroom serialises the SAME settings as property elements in
        // plenty of real sidecars (crs_str accepts that form). The merge
        // must strip the owned element too, or the document answers one
        // slider with two conflicting values — while unowned elements
        // (PointColor; it was Texture until R25 B2 modelled that one)
        // survive untouched.
        let lr = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 7.0-c000\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
    crs:HasSettings=\"True\">\n\
   <crs:Exposure2012>+1.00</crs:Exposure2012>\n\
   <crs:Contrast2012>+22</crs:Contrast2012>\n\
   <crs:PointColor>0</crs:PointColor>\n\
  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n";
        let r = EditRecipe { exposure_ev: 0.25, ..Default::default() };
        let merged = merged_doc(lr, &r).expect("mergeable");
        assert!(!merged.contains("<crs:Exposure2012>"), "owned element stripped: {merged}");
        assert!(!merged.contains("<crs:Contrast2012>"), "owned element stripped");
        assert_eq!(merged.matches("crs:Exposure2012").count(), 1, "ours only: {merged}");
        assert!(merged.contains("crs:Exposure2012=\"0.25\""));
        assert!(
            merged.contains("<crs:PointColor>0</crs:PointColor>"),
            "unowned element survives: {merged}"
        );
        let back = xmp_to_recipe(&merged);
        assert_eq!(back.exposure_ev, 0.25);
        assert_eq!(back.contrast, 0.0, "the old element value must not shadow the cleared slider");
    }

    #[test]
    fn merge_strips_only_top_level_owned_elements() {
        // The strip is a property of THIS Description. Adobe writes a creative
        // profile's baked parameters as owned-LOOKING children of a nested
        // rdf:Description inside <crs:Look>, and a flat scan reached in and
        // gutted them — destroying the very Look this merge exists to
        // preserve. Name matching also catches the attribute-carrying
        // spelling, which the `<crs:Name>` literal missed (leaving exactly the
        // duplicate the element strip exists to prevent).
        let lr = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
    crs:HasSettings=\"True\">\n\
   <crs:Exposure2012 xml:lang=\"x-default\">+1.00</crs:Exposure2012>\n\
   <crs:Look>\n\
    <rdf:Description crs:Name=\"Adobe Landscape\" crs:Amount=\"1\">\n\
     <crs:Parameters>\n\
      <rdf:Description crs:Version=\"15.4\">\n\
       <crs:Exposure2012>+0.35</crs:Exposure2012>\n\
       <crs:ToneCurvePV2012>\n\
        <rdf:Seq><rdf:li>0, 0</rdf:li></rdf:Seq>\n\
       </crs:ToneCurvePV2012>\n\
      </rdf:Description>\n\
     </crs:Parameters>\n\
    </rdf:Description>\n\
   </crs:Look>\n\
  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n";
        let r = EditRecipe { exposure_ev: 0.25, ..Default::default() };
        let merged = merged_doc(lr, &r).expect("mergeable");
        // The Look keeps BOTH of its own baked parameters.
        assert!(
            merged.contains("<crs:Exposure2012>+0.35</crs:Exposure2012>"),
            "the Look's own parameter must survive: {merged}"
        );
        assert!(merged.contains("<rdf:li>0, 0</rdf:li>"), "the Look's own curve must survive");
        assert!(merged.contains("crs:Name=\"Adobe Landscape\""), "and the Look itself");
        // ...while OUR top-level property is stripped in the attribute-carrying
        // spelling too, leaving exactly one answer for the slider.
        assert!(!merged.contains("xml:lang"), "top-level owned element stripped: {merged}");
        assert!(merged.contains("crs:Exposure2012=\"0.25\""), "ours is the attribute");
        assert_eq!(
            merged.matches("crs:Exposure2012").count(),
            3,
            "ours + the Look's open/close, no shadow copy: {merged}"
        );
        assert_eq!(xmp_to_recipe(&merged).exposure_ev, 0.25);
    }

    #[test]
    fn merge_survives_a_cdata_section() {
        // LEGAL XML must never fall back to a full regenerate: that path
        // replaces the user's whole sidecar with our own document and takes
        // every foreign property with it — the data loss the merge exists to
        // prevent. A CDATA section is not a tag; a scanner that counts it as
        // one leaves `depth` unbalanced and bails.
        let lr = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
    crs:HasSettings=\"True\">\n\
   <crs:Exposure2012>+1.00</crs:Exposure2012>\n\
   <dc:description><![CDATA[client <proof> notes]]></dc:description>\n\
   <crs:PointColor>0</crs:PointColor>\n\
  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n";
        let r = EditRecipe { exposure_ev: 0.25, ..Default::default() };
        let merged = merged_doc(lr, &r).expect("a CDATA section must stay mergeable");
        assert!(
            merged.contains("<![CDATA[client <proof> notes]]>"),
            "the foreign CDATA property must survive verbatim: {merged}"
        );
        assert!(merged.contains("<crs:PointColor>0</crs:PointColor>"), "unowned element survives");
        assert!(!merged.contains("<crs:Exposure2012>"), "ours is still stripped: {merged}");
        assert!(merged.contains("crs:Exposure2012=\"0.25\""));
    }

    #[test]
    fn merge_replaces_masks_without_shredding_nested_descriptions() {
        // Lightroom nests rdf:Description elements INSIDE mask corrections —
        // the close-tag search must depth-count (the batch-3 lesson), and the
        // mask block is replaced wholesale while everything AFTER it lives.
        let lr = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
    crs:PointColor=\"0\"\n\
    crs:HasSettings=\"True\">\n\
   <crs:MaskGroupBasedCorrections>\n\
    <rdf:Seq>\n\
     <rdf:li>\n\
      <rdf:Description crs:What=\"Correction\" crs:LocalExposure2012=\"0.1\">\n\
       <crs:CorrectionMasks>\n\
        <rdf:Seq>\n\
         <rdf:li>\n\
          <rdf:Description crs:What=\"Mask/Gradient\" crs:ZeroX=\"0.5\" crs:ZeroY=\"0.4\" crs:FullX=\"0.5\" crs:FullY=\"0.0\"/>\n\
         </rdf:li>\n\
        </rdf:Seq>\n\
       </crs:CorrectionMasks>\n\
      </rdf:Description>\n\
     </rdf:li>\n\
    </rdf:Seq>\n\
   </crs:MaskGroupBasedCorrections>\n\
   <crs:Look>\n\
    <rdf:Description crs:Name=\"Adobe Landscape\"/>\n\
   </crs:Look>\n\
  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n";
        let r = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Radial {
                    top: 0.2,
                    left: 0.2,
                    bottom: 0.8,
                    right: 0.8,
                    feather: 0.5,
                    roundness: 0.0,
                    flipped: false,
                    angle: 0.0,
                    midpoint: 50.0,
                    mask_version: 2,
                },
                exposure_ev: 1.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let merged = merged_doc(lr, &r).expect("mergeable");
        assert_eq!(
            merged.matches("<crs:MaskGroupBasedCorrections>").count(),
            1,
            "one mask block — OURS"
        );
        assert!(merged.contains("Mask/CircularGradient"), "our radial mask is in");
        // The fully supported old correction is replaceable; its old local
        // exposure value must not survive beside the new radial correction.
        assert!(
            !merged.contains("crs:LocalExposure2012=\"0.1\""),
            "LR's fully supported old mask block is replaced"
        );
        assert!(!merged.contains("crs:ZeroX=\"0.5\""), "…including its nested gradient");
        assert!(
            merged.contains("crs:Name=\"Adobe Landscape\""),
            "the element AFTER the mask block survives — nesting was not shredded"
        );
        assert!(merged.contains("crs:PointColor=\"0\""), "unowned attribute survives");
        // The whole document still ends properly (splice did not eat the tail).
        assert!(merged.trim_end().ends_with("</x:xmpmeta>"));
    }

    // ── reader (xmp_to_recipe) ───────────────────────────────────────────────

    #[test]
    fn globals_round_trip_through_xmp() {
        // Values are chosen to survive the writer's documented rounding: integer
        // sliders (`signed()`), 2-decimal exposure, integer Kelvin, 1-decimal
        // straighten, %.6f crop — so the reader must land EXACTLY back.
        let r = EditRecipe {
            exposure_ev: 0.32,
            contrast: 14.0,
            highlights: -12.0,
            shadows: 25.0,
            whites: 8.0,
            blacks: -6.0,
            temperature_k: Some(5600.0),
            tint: 3.0,
            vibrance: 18.0,
            saturation: -5.0,
            clarity: 10.0,
            dehaze: 7.0,
            hsl: crate::recipe::Hsl {
                hue: [0.0, 15.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                saturation: [0.0, 0.0, 0.0, -40.0, 0.0, 0.0, 0.0, 0.0],
                ..Default::default()
            },
            color_grade: crate::recipe::ColorGrade {
                shadow_hue: 220.0,
                shadow_sat: 30.0,
                highlight_hue: 45.0,
                highlight_sat: 20.0,
                midtone_lum: -10.0,
                balance: 15.0,
                ..Default::default()
            },
            // → crs 45 → back as 45. Exact for a different reason than before
            // v0.31.1: not "45 happens to survive ×⅔ then ×1.5", but "there is
            // no scale in either direction any more".
            sharpening: 45.0,
            noise_reduction: 20.0,
            lens_vignette: 35.0,
            lens_vignette_mid: 60.0,
            lens_distortion: -24.0,
            straighten_deg: 1.5,
            crop: Some(Crop { left: 0.05, top: 0.0, right: 0.95, bottom: 1.0 }),
            tone_curve: vec![
                CurvePoint { input: 0, output: 8 },
                CurvePoint { input: 255, output: 247 },
            ],
            red_curve: vec![
                CurvePoint { input: 0, output: 10 },
                CurvePoint { input: 255, output: 250 },
            ],
            rationale: "warm & contrasty <test> & \"q\"".into(),
            confidence: 0.82,
            ..Default::default()
        };
        let back = xmp_to_recipe(&recipe_to_xmp(&r));
        assert_eq!(back, r);
    }

    #[test]
    fn as_shot_tint_round_trips_only_for_our_own_sidecars() {
        // Our writer emits a non-neutral Tint even under "As Shot"; the AutoShade
        // marker tells the reader it is a real edit.
        let r = EditRecipe { tint: 3.0, ..Default::default() };
        assert_eq!(xmp_to_recipe(&recipe_to_xmp(&r)).tint, 3.0);
    }

    #[test]
    fn parametric_masks_round_trip_through_xmp() {
        let r = EditRecipe {
            masks: vec![
                LocalAdjustment {
                    mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.35, full_x: 0.5, full_y: 0.0 },
                    range: Some(RangeMask::Luminance { lo_outer: 0.4, lo: 0.5, hi: 1.0, hi_outer: 1.0 }),
                    name: "sky & sea".into(),
                    amount: 0.75,
                    inverted: true,
                    exposure_ev: -0.4, // ÷4 → ×4 is a power-of-two rescale: exact
                    contrast: 30.0,    // "0.3" ×100 needs the 4-decimal snap: exact
                    highlights: -50.0,
                    shadows: 60.0,
                    whites: 10.0,
                    blacks: -20.0,
                    clarity: 40.0,
                    dehaze: 5.0,
                    texture: 15.0,
                    // R23-1b: two keys the writer used to emit as a literal
                    // "0". They ride the same ÷100 ↔ ×100 pair as their
                    // neighbours, and a sidecar carrying them must still import
                    // as loss-free — `correction_value_reasons` demanded 0
                    // for both until R23-1b, so a non-zero one would have
                    // refused the whole correction and this equality would
                    // fail on every other field too.
                    sharpness: -45.0,
                    saturation: 20.0,
                    hue: 35.0,
                    temperature: 25.0,
                    tint: -10.0,
                    noise_reduction: 30.0,
                    ..Default::default()
                },
                LocalAdjustment {
                    mask: MaskGeometry::Radial {
                        top: 0.3, left: 0.35, bottom: 0.7, right: 0.65,
                        feather: 0.5, roundness: 0.0, flipped: true, angle: 0.0,
                        midpoint: 50.0, mask_version: 2,
                    },
                    range: Some(RangeMask::Color { r: 0.9, g: 0.6, b: 0.2, amount: 0.5, px: 0.4, py: 0.7 }),
                    name: "subject".into(),
                    shadows: 20.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let back = xmp_to_recipe(&recipe_to_xmp(&r));
        // R25 P9: ONE field does not come back where it went in. Lightroom
        // spells a radial's inversion ONCE (`crs:MaskInverted`, with
        // `crs:Flipped` as its complement) where this recipe spells it as the
        // XOR of two flags, so the projection collapses `flipped` into
        // `inverted`. The XOR is the whole of what the pixels see (render.rs
        // `mask_weight` and the weight loop) and it survives exactly; WHICH of
        // our two flags carries it is not a fact about the photograph.
        // Written as an expected value rather than a relaxed comparison so
        // every other field still has to match to the bit.
        let mut expect = r.masks.clone();
        let MaskGeometry::Radial { flipped, .. } = &mut expect[1].mask else {
            panic!("mask 1 is the radial");
        };
        *flipped = false;
        expect[1].inverted = true;
        // …and ONE more, from v0.32.0: the radial's box no longer passes
        // through verbatim. It goes out through the inverse of Lightroom's
        // frame affine and comes back through the affine, and the WIRE carries
        // six decimals — Lightroom's own precision, which is the precision any
        // value that has ever been through Lightroom actually has. So the
        // corners settle by up to half a wire step, measured here at
        // 4.6 × 10⁻⁷ of the frame = 0.005 px on a 9504 px export. It is a
        // one-time settle onto the wire grid, not a drift: the second round
        // trip is exact, which the re-emit below is what pins.
        let approx = |a: &MaskGeometry, b: &MaskGeometry| match (a, b) {
            (
                MaskGeometry::Radial { top: t1, left: l1, bottom: b1, right: r1, .. },
                MaskGeometry::Radial { top: t2, left: l2, bottom: b2, right: r2, .. },
            ) => [(t1, t2), (l1, l2), (b1, b2), (r1, r2)]
                .iter()
                .all(|(x, y)| (**x - **y).abs() < 1e-6),
            _ => false,
        };
        assert!(approx(&back.masks[1].mask, &expect[1].mask), "{:?}", back.masks[1].mask);
        expect[1].mask = back.masks[1].mask.clone();
        assert_eq!(back.masks, expect);
        // The wire grid is a FIXED POINT, not a ratchet: once a box has been
        // through the projection its own re-emit reproduces it to the bit.
        assert_eq!(
            xmp_to_recipe(&recipe_to_xmp(&back)).masks[1].mask,
            back.masks[1].mask,
            "the second round trip is exact"
        );
        for (i, (was, now)) in r.masks.iter().zip(&back.masks).enumerate() {
            assert_eq!(
                lr_net_inverted(was),
                lr_net_inverted(now),
                "mask {i}: the net inversion is the part that must survive"
            );
        }
    }

    #[test]
    fn bitmap_masks_do_not_come_back_from_xmp() {
        // The writer skips raster corrections (no classic-XMP encoding), so the
        // reader must return only the parametric mask — never a phantom.
        let mixed = mixed_parametric_and_raster();
        let back = xmp_to_recipe(&recipe_to_xmp(&mixed));
        assert_eq!(back.masks.len(), 1);
        assert_eq!(back.masks[0].mask, mixed.masks[0].mask);
        assert_eq!(back.masks[0].exposure_ev, -1.0);
    }

    #[test]
    fn foreign_as_shot_sidecar_imports_no_wb_and_drops_identity_curves() {
        // A Lightroom-style sidecar (no AutoShade marker): "As Shot" Temperature
        // and Tint are the CAMERA's values, not edits — they must NOT import.
        // LR also always writes the master curve; the 2-point identity means
        // "no curve" and must collapse to empty.
        let lr = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    crs:WhiteBalance="As Shot"
    crs:Temperature="5150"
    crs:Tint="+10"
    crs:Exposure2012="+0.65"
    crs:Contrast2012="+22"
    crs:Sharpness="40"
    crs:HasSettings="True">
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>0, 0</rdf:li>
     <rdf:li>255, 255</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;
        let r = xmp_to_recipe(lr);
        assert_eq!(r.temperature_k, None, "as-shot Kelvin is not an edit");
        assert_eq!(r.tint, 0.0, "as-shot tint is not an edit");
        assert_eq!(r.exposure_ev, 0.65);
        assert_eq!(r.contrast, 22.0);
        // 1:1 since v0.31.1. This is the value the user's own seven reference
        // sidecars carry, and it used to import as 60.
        assert_eq!(r.sharpening, 40.0);
        assert!(r.tone_curve.is_empty(), "identity curve must collapse");
        // A Custom-WB foreign sidecar DOES import its Kelvin + tint.
        let custom = lr.replace("As Shot", "Custom");
        let rc = xmp_to_recipe(&custom);
        assert_eq!(rc.temperature_k, Some(5150.0));
        assert_eq!(rc.tint, 10.0);
    }

    #[test]
    fn xml_values_round_trip_hostile_text_and_foreign_references_exactly_once() {
        let hostile = r#"& < > " ' literal &lt; masks\Bob's "sky".xmp"#;
        let r = EditRecipe {
            rationale: hostile.into(),
            masks: vec![LocalAdjustment { name: hostile.into(), ..Default::default() }],
            ..Default::default()
        };
        let xmp = recipe_to_xmp(&r);
        assert!(xmp.contains("&quot;sky&quot;"), "attribute quotes are escaped: {xmp}");
        let back = xmp_to_recipe(&xmp);
        assert_eq!(back.rationale, hostile);
        assert_eq!(back.masks[0].name, hostile);

        let foreign = r#"<rdf:Description crs:CorrectionName = "Bob&apos;s &#x3C;sky&#62; &#38; &quot;sea&quot;"/>"#;
        assert_eq!(
            Tag::new(foreign).crs_str("CorrectionName").as_deref(),
            Some(r#"Bob's <sky> & "sea""#)
        );
    }

    #[test]
    fn comments_and_whitespace_cannot_hijack_the_crs_description_or_merge() {
        let fake = r#"<!-- <rdf:Description xmlns:crs="urn:fake" crs:Exposure2012="9"/> -->"#;
        let doc = format!(
            "{fake}\n<rdf:Description rdf:about=\"\" \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             crs:Exposure2012 = \"+0.65\" crs:HasSettings=\"True\">\
             </rdf:Description>"
        );
        assert_eq!(xmp_to_recipe(&doc).exposure_ev, 0.65);
        let merged = merged_doc(
            &doc,
            &EditRecipe { exposure_ev: 0.25, ..Default::default() },
        )
        .expect("the real description is mergeable");
        assert!(merged.contains(fake), "the foreign comment survives verbatim");
        assert_eq!(xmp_to_recipe(&merged).exposure_ev, 0.25);
    }

    /// R25 P1 rewrote this test's premise. It used to read
    /// `partial_and_unsupported_masks_are_not_rendered…` and pin the old
    /// all-or-nothing rule: five corrections in, five losses, zero masks. Two
    /// of those five carry nothing worse than a rotation angle and a DEFAULT
    /// blend mode, which is what every Lightroom radial and every Lightroom
    /// component look like — so the rule refused the user's whole catalog.
    /// Now the readable ones import with a named note, the genuinely
    /// unreadable ones still do not, and the base's block is still preserved
    /// byte-for-byte while the develop has not touched it.
    #[test]
    fn readable_corrections_import_with_a_note_and_their_group_is_still_preserved() {
        let doc = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core">
     <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
        crs:HasSettings="True">
       <crs:MaskGroupBasedCorrections>
        <rdf:Seq>
         <rdf:li><rdf:Description crs:What="Correction" crs:CorrectionActive="true">
          <crs:CorrectionMasks><rdf:Seq>
           <rdf:li crs:What="Mask/Gradient" crs:ZeroX="0.5" crs:ZeroY="0.8" crs:FullX="0.5" crs:FullY="0.2"/>
           <rdf:li crs:What="Mask/Brush"/>
          </rdf:Seq></crs:CorrectionMasks>
         </rdf:Description></rdf:li>
         <rdf:li><rdf:Description crs:What="Correction" crs:CorrectionActive="false">
          <crs:CorrectionMasks><rdf:Seq>
           <rdf:li crs:What="Mask/Gradient" crs:ZeroX="0.4" crs:ZeroY="0.8" crs:FullX="0.4" crs:FullY="0.2"/>
          </rdf:Seq></crs:CorrectionMasks>
         </rdf:Description></rdf:li>
         <rdf:li><rdf:Description crs:What="Correction">
          <crs:CorrectionMasks><rdf:Seq>
           <rdf:li crs:What="Mask/CircularGradient" crs:Top="0.2" crs:Left="0.2" crs:Bottom="0.8" crs:Right="0.8" crs:Feather="50" crs:Roundness="0" crs:Flipped="false" crs:Angle="12"/>
          </rdf:Seq></crs:CorrectionMasks>
         </rdf:Description></rdf:li>
         <rdf:li><rdf:Description crs:What="Correction">
          <crs:CorrectionMasks><rdf:Seq>
           <rdf:li crs:What="Mask/Gradient" crs:MaskBlendMode="0" crs:ZeroX="0.3" crs:ZeroY="0.8" crs:FullX="0.3" crs:FullY="0.2"/>
          </rdf:Seq></crs:CorrectionMasks>
         </rdf:Description></rdf:li>
         <rdf:li><rdf:Description crs:What="Correction">
          <crs:CorrectionMasks><rdf:Seq>
           <rdf:li crs:What="Mask/Brush"/>
          </rdf:Seq></crs:CorrectionMasks>
         </rdf:Description></rdf:li>
        </rdf:Seq>
       </crs:MaskGroupBasedCorrections>
      </rdf:Description>
     </rdf:RDF>
    </x:xmpmeta>"#;

        let parsed = xmp_to_recipe(doc);
        assert_eq!(
            parsed.masks.len(),
            2,
            "the rotated radial and the default-blend-mode gradient are readable: {:?}",
            parsed.masks
        );
        assert_eq!(
            unsupported_corrections(doc),
            3,
            "the brush pair and the muted correction are the only refusals"
        );
        let losses = import_losses(doc);
        assert_eq!(losses.len(), 4, "three refusals plus the rotation note: {losses:?}");
        assert_eq!(
            losses.iter().filter(|l| l.reason == MaskImportReason::Rotation(12)).count(),
            1,
            "crs:Angle=\"12\" is named WITH ITS ANGLE, not silently discarded: {losses:?}"
        );
        assert!(
            !losses.iter().any(|l| l.reason == MaskImportReason::BlendMode),
            "the DEFAULT blend mode costs nothing and must raise nothing: {losses:?}"
        );
        // The rotated radial imports UNROTATED — the reading is honest about
        // being approximate, which is the whole point of naming the loss.
        assert!(
            parsed
                .masks
                .iter()
                .any(|m| matches!(m.mask, MaskGeometry::Radial { angle, .. } if angle == 0.0)),
            "crs:Angle is not mapped onto the engine angle in this batch"
        );

        let start = doc.find("<crs:MaskGroupBasedCorrections>").unwrap();
        let end = doc.find("</crs:MaskGroupBasedCorrections>").unwrap()
            + "</crs:MaskGroupBasedCorrections>".len();
        let original = &doc[start..end];
        let merged = merged_doc(
            doc,
            &EditRecipe { exposure_ev: 0.25, ..Default::default() },
        )
        .expect("the surrounding document remains mergeable");
        assert!(merged.contains(original), "the original mask group is retained verbatim");
        assert!(merged.contains("Mask/Brush"));
        assert!(merged.contains(r#"crs:CorrectionActive="false""#));
        assert!(merged.contains(r#"crs:Angle="12""#));
        assert!(merged.contains(r#"crs:MaskBlendMode="0""#));
    }

    /// L05#4: the preserve rule yields to the recipe's own masks — the save
    /// in hand is the newest intent, so the published document carries THIS
    /// develop's masks, the foreign block goes, and the loss is a note
    /// rather than a silence (before: the output showed an older pass's
    /// masks and none of the develop's, reported as plain success).
    #[test]
    fn a_recipe_with_masks_outranks_the_bases_foreign_mask_block() {
        let doc = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core">
     <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
        crs:HasSettings="True">
       <crs:MaskGroupBasedCorrections>
        <rdf:Seq>
         <rdf:li><rdf:Description crs:What="Correction" crs:CorrectionActive="true">
          <crs:CorrectionMasks><rdf:Seq>
           <rdf:li crs:What="Mask/Brush"/>
          </rdf:Seq></crs:CorrectionMasks>
         </rdf:Description></rdf:li>
        </rdf:Seq>
       </crs:MaskGroupBasedCorrections>
      </rdf:Description>
     </rdf:RDF>
    </x:xmpmeta>"#;
        let mut r = EditRecipe { exposure_ev: 0.25, ..Default::default() };
        r.masks.push(LocalAdjustment {
            mask: MaskGeometry::Radial {
                top: 0.2,
                left: 0.2,
                bottom: 0.8,
                right: 0.8,
                feather: 0.5,
                roundness: 0.0,
                flipped: false,
                angle: 0.0,
                midpoint: 50.0,
                mask_version: 2,
            },
            name: "face".into(),
            exposure_ev: 0.4,
            ..Default::default()
        });
        let out = merge_recipe_into_xmp(doc, &r).expect("mergeable");
        assert!(
            out.doc.contains("Mask/CircularGradient"),
            "the develop's own mask is published: {}",
            out.doc
        );
        assert!(!out.doc.contains("Mask/Brush"), "the foreign block is not resurrected");
        assert_eq!(out.notes.len(), 1, "the replacement is disclosed: {:?}", out.notes);
        assert!(
            out.notes[0].contains("1 thing(s)") && out.notes[0].contains("1 edited mask(s)"),
            "the note names both counts: {}",
            out.notes[0]
        );
        // The mirror case stays preserved-without-note: nothing of the user's
        // is suppressed when the recipe has no masks.
        let out2 = merge_recipe_into_xmp(
            doc,
            &EditRecipe { exposure_ev: 0.25, ..Default::default() },
        )
        .expect("mergeable");
        assert!(out2.doc.contains("Mask/Brush"), "no recipe masks → the block is preserved");
        assert!(out2.notes.is_empty(), "a pure preserve has no loss to note: {:?}", out2.notes);
    }

    /// L05#1: the attribute-carrying spelling of an owned element is the SAME
    /// property (legal XML; the writer's strip already matched it by name) —
    /// the literal reader missed it, imported "no curve", and the merge then
    /// deleted the element from the user's own sidecar with nothing written
    /// in its place.
    #[test]
    fn an_attribute_form_curve_is_read_not_deleted() {
        let doc = r#"<rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
        crs:HasSettings="True">
       <crs:ToneCurvePV2012 xml:lang="x-default"><rdf:Seq>
        <rdf:li>0, 20</rdf:li>
        <rdf:li>255, 240</rdf:li>
       </rdf:Seq></crs:ToneCurvePV2012>
      </rdf:Description>"#;
        let r = xmp_to_recipe(doc);
        assert_eq!(
            r.tone_curve,
            vec![CurvePoint { input: 0, output: 20 }, CurvePoint { input: 255, output: 240 }],
            "the attribute-form curve is read"
        );
        // Merging a NEW curve over it must not leave two curves behind: the
        // attribute-form element is stripped (by name) and ours replaces it.
        let merged = merged_doc(
            doc,
            &EditRecipe {
                tone_curve: vec![
                    CurvePoint { input: 0, output: 5 },
                    CurvePoint { input: 255, output: 250 },
                ],
                ..Default::default()
            },
        )
        .expect("mergeable");
        assert!(!merged.contains("0, 20"), "the old spelling is stripped: {merged}");
        assert_eq!(xmp_to_recipe(&merged).tone_curve[0].output, 5, "the new curve answers");
    }

    /// L05#1: an attribute-form mask GROUP is a real group — reading it as
    /// "absent" reported zero unsupported corrections AND told the merge it
    /// was free to replace the block.
    #[test]
    fn an_attribute_form_mask_group_counts_as_a_loss_and_survives_the_merge() {
        let doc = r#"<rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
        crs:HasSettings="True">
       <crs:MaskGroupBasedCorrections rdf:parseType="Resource">
        <rdf:Seq>
         <rdf:li><rdf:Description crs:What="Correction" crs:CorrectionActive="true">
          <crs:CorrectionMasks><rdf:Seq>
           <rdf:li crs:What="Mask/Brush"/>
          </rdf:Seq></crs:CorrectionMasks>
         </rdf:Description></rdf:li>
        </rdf:Seq>
       </crs:MaskGroupBasedCorrections>
      </rdf:Description>"#;
        assert_eq!(unsupported_corrections(doc), 1, "the brush correction is a counted loss");
        let merged = merged_doc(
            doc,
            &EditRecipe { exposure_ev: 0.25, ..Default::default() },
        )
        .expect("mergeable");
        assert!(merged.contains("Mask/Brush"), "the group survives the merge: {merged}");
    }

    /// A whitespace-carrying close tag (`</crs:Key >`) is the same close in
    /// XML; the literal close scan ran past it.
    #[test]
    fn a_close_tag_with_trailing_space_still_ends_a_property_element() {
        let doc = r#"<rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">
       <crs:Exposure2012>+0.65</crs:Exposure2012 >
      </rdf:Description>"#;
        assert_eq!(Scope::new(doc).crs_f32("Exposure2012"), Some(0.65));
    }

    /// Present-but-unreadable is a DISCLOSED loss, not "no curve": the
    /// attribute-form spelling used to make the element invisible to the
    /// disclosure as well, so bad points imported as a silent neutral.
    #[test]
    fn an_unreadable_attribute_form_curve_is_named_by_unparsable_crs_numbers() {
        let doc = r#"<rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">
       <crs:ToneCurvePV2012 xml:lang="x-default"><rdf:Seq>
        <rdf:li>999, -5</rdf:li>
       </rdf:Seq></crs:ToneCurvePV2012>
      </rdf:Description>"#;
        let bad = unparsable_crs_numbers(doc);
        assert!(bad.iter().any(|v| v == "ToneCurvePV2012"), "disclosed: {bad:?}");
        // An element that never closes is the same disclosed loss.
        let unterminated = r#"<rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">
       <crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 0</rdf:li></rdf:Seq>
      </rdf:Description>"#;
        let bad = unparsable_crs_numbers(unterminated);
        assert!(bad.iter().any(|v| v == "ToneCurvePV2012"), "disclosed: {bad:?}");
    }

    /// L05#7: a document binding the camera-raw namespace to another prefix
    /// (or `crs` to another URI) is one every scanner here misreads — the
    /// merge REFUSES (the caller regenerates and discloses) instead of
    /// splicing a second, contradictory settings block beside the foreign
    /// one, and the import discloses instead of coming back silently neutral.
    #[test]
    fn a_foreign_camera_raw_prefix_refuses_the_merge_and_is_disclosed() {
        let doc = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
     <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about=""
        xmlns:cr="http://ns.adobe.com/camera-raw-settings/1.0/"
        cr:Exposure2012="+1.00" cr:HasSettings="True">
      </rdf:Description>
     </rdf:RDF>
    </x:xmpmeta>"#;
        assert!(
            merge_recipe_into_xmp(doc, &EditRecipe::default()).is_none(),
            "a foreign camera-raw prefix is refused, never duplicated"
        );
        let bad = unparsable_crs_numbers(doc);
        assert_eq!(bad.len(), 1, "one entry naming the binding: {bad:?}");
        assert!(bad[0].contains("`cr:`"), "the prefix is named: {}", bad[0]);

        let crooked = r#"<rdf:Description rdf:about=""
        xmlns:crs="http://example.invalid/ns" crs:Exposure2012="+1.00">
      </rdf:Description>"#;
        assert!(
            merge_recipe_into_xmp(crooked, &EditRecipe::default()).is_none(),
            "a crs prefix bound to a foreign URI is not camera raw"
        );
        assert!(!unparsable_crs_numbers(crooked).is_empty());
    }

    /// L05#7 sub-item 4: `xmlns:crs` may legally live on an ANCESTOR
    /// (`rdf:RDF`) with every setting in property-element form — the
    /// attribute-only test missed that Description, and the merge spliced a
    /// SECOND settings Description into the same document.
    #[test]
    fn a_description_whose_crs_children_declare_the_namespace_upstream_is_still_found() {
        let doc = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
     <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
       xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">
      <rdf:Description rdf:about="">
       <crs:Exposure2012>+0.80</crs:Exposure2012>
      </rdf:Description>
     </rdf:RDF>
    </x:xmpmeta>"#;
        assert_eq!(xmp_to_recipe(doc).exposure_ev, 0.8, "element-form settings are found");
        let merged = merged_doc(
            doc,
            &EditRecipe { exposure_ev: 0.25, ..Default::default() },
        )
        .expect("mergeable");
        assert_eq!(
            merged.matches("<rdf:Description").count(),
            1,
            "spliced in place, not duplicated: {merged}"
        );
        assert_eq!(xmp_to_recipe(&merged).exposure_ev, 0.25);
        assert!(!merged.contains("+0.80"), "the old element spelling is stripped");
    }

    /// The guard the refusal gate must not break: a genuinely settings-free
    /// ratings sidecar still takes the INSERT path (that path exists because
    /// regenerating over one reported an unfixable loss on every save).
    #[test]
    fn a_ratings_only_sidecar_still_takes_the_insert_path() {
        let doc = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
     <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/"
        xmp:Rating="4">
      </rdf:Description>
     </rdf:RDF>
    </x:xmpmeta>"#;
        let out = merge_recipe_into_xmp(
            doc,
            &EditRecipe { exposure_ev: 0.25, ..Default::default() },
        )
        .expect("insertable");
        assert!(out.doc.contains(r#"xmp:Rating="4""#), "the rating survives verbatim");
        assert_eq!(xmp_to_recipe(&out.doc).exposure_ev, 0.25, "our settings are added");
        assert!(out.notes.is_empty(), "a clean insert has no loss: {:?}", out.notes);
    }

    #[test]
    fn xmp_input_is_bounded_and_numeric_groups_follow_recipe_boundaries() {
        let doc = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core">
     <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
        crs:WhiteBalance="Custom"
        crs:Temperature="90000"
        crs:Exposure2012="99"
        crs:Contrast2012="-500"
        crs:Sharpness="200"
        crs:HasCrop="True"
        crs:CropLeft="-1"
        crs:CropTop="0"
        crs:CropRight="1"
        crs:CropBottom="1"
        crs:HasSettings="True">
       <crs:ToneCurvePV2012><rdf:Seq>
        <rdf:li>999, -5</rdf:li>
       </rdf:Seq></crs:ToneCurvePV2012>
       <crs:ToneCurvePV2012Red><rdf:Seq>
        <rdf:li>broken</rdf:li>
       </rdf:Seq></crs:ToneCurvePV2012Red>
       <crs:MaskGroupBasedCorrections><rdf:Seq>
        <rdf:li><rdf:Description crs:What="Correction" crs:LocalExposure2012="9">
         <crs:CorrectionMasks><rdf:Seq>
          <rdf:li crs:What="Mask/Gradient" crs:ZeroX="0.5" crs:ZeroY="0.8" crs:FullX="0.5" crs:FullY="0.2"/>
         </rdf:Seq></crs:CorrectionMasks>
        </rdf:Description></rdf:li>
       </rdf:Seq></crs:MaskGroupBasedCorrections>
      </rdf:Description>
     </rdf:RDF>
    </x:xmpmeta>"#;

        let r = xmp_to_recipe(doc);
        assert_eq!(r.temperature_k, Some(40000.0));
        assert_eq!(r.exposure_ev, 5.0);
        assert_eq!(r.contrast, -100.0);
        assert_eq!(r.sharpening, 150.0);
        assert_eq!(r.crop, None, "invalid compound crop geometry is rejected");
        assert!(
            r.tone_curve.is_empty(),
            "out-of-domain curve coordinates are rejected as a group — the old \
             saturation policy imported '999, -5' as a near-black one-point curve"
        );
        assert!(r.red_curve.is_empty(), "a malformed curve is rejected as a group");
        assert!(r.masks.is_empty(), "an out-of-range local correction is rejected as partial");
        assert_eq!(unsupported_corrections(doc), 1);

        let bad = unparsable_crs_numbers(doc);
        for key in [
            "Temperature",
            "Exposure2012",
            "Contrast2012",
            "Sharpness",
            "CropLeft",
            "ToneCurvePV2012Red",
        ] {
            assert!(bad.iter().any(|v| v == key), "{key} must be disclosed: {bad:?}");
        }

        let oversized = "x".repeat(MAX_XMP_BYTES + 1);
        assert!(crs_own_scope(&oversized).is_empty());
        assert_eq!(xmp_to_recipe(&oversized), EditRecipe::default());
        assert!(merged_doc(&oversized, &EditRecipe::default()).is_none());
        assert_eq!(
            unparsable_crs_numbers(&oversized),
            vec!["XMP document exceeds the 16 MiB limit".to_string()]
        );
    }

    // --- R27 Batch-5: the `Mask/Image` AI arm (L-08 Arm C) -------------------
    //
    // Fixtures below reproduce the shape measured across 105 real `Mask/Image`
    // instances in the user's library on 2026-08-19: 21 distinct attribute
    // names, `MaskActive="true"` on all of them, `MaskVersion="1"` on all of
    // them, `MaskSubType` in {0, 1, 2}, `ReferencePoint` on all of them, and
    // exactly one optional child element (`crs:Gesture`, on 40).

    /// One `Mask/Image` component. `extra` splices additional attributes;
    /// `gesture` splices a `crs:Gesture` child (empty = self-closing, which is
    /// what 65 of the 105 real instances are).
    fn lr_ai_mask(subtype: &str, blend: &str, value: &str, extra: &str, gesture: &str) -> String {
        let head = format!(
            "        <rdf:Description\n\
             \x20        crs:What=\"Mask/Image\"\n\
             \x20        crs:MaskActive=\"true\"\n\
             \x20        crs:MaskName=\"Sky 1\"\n\
             \x20        crs:MaskBlendMode=\"{blend}\"\n\
             \x20        crs:MaskInverted=\"false\"\n\
             \x20        crs:MaskSyncID=\"440777CD3CB8E24BB8E16028893B45DC\"\n\
             \x20        crs:MaskValue=\"{value}\"\n\
             \x20        crs:MaskVersion=\"1\"\n\
             \x20        crs:MaskSubType=\"{subtype}\"\n\
             \x20        crs:ReferencePoint=\"0.605469 0.281525\"{extra}"
        );
        if gesture.is_empty() {
            format!("       <rdf:li>\n{head}/>\n       </rdf:li>\n")
        } else {
            format!(
                "       <rdf:li>\n{head}>\n\
                 \x20       <crs:Gesture>\n\
                 \x20        <rdf:Seq>\n\
                 {gesture}\
                 \x20        </rdf:Seq>\n\
                 \x20       </crs:Gesture>\n\
                 \x20       </rdf:Description>\n\
                 \x20      </rdf:li>\n"
            )
        }
    }

    /// The provenance block Lightroom writes on a real sky mask — every
    /// attribute this engine carries and never interprets.
    const LR_AI_PROVENANCE: &str = "\n         crs:InputDigest=\"D0DAC04EB58F013F49D93EF47D22794E\"\
\n         crs:InputDigestVersion=\"2\"\
\n         crs:MaskDigest=\"00D1A1B68591DF41F6CA3F8F805D0F1B\"\
\n         crs:WholeImageArea=\"0/1,0/1,1920/1,2880/1\"\
\n         crs:Origin=\"0,0\"\
\n         crs:ModelVersion=\"234881976\"";

    /// THE DOMINANT REFUSAL, closed. Before R27 Batch-5 a correction holding a
    /// `Mask/Image` was thrown away entire — 78 corrections across 40 files,
    /// 40 % of every file in the reference library that has a mask at all —
    /// and it took the gradient standing beside it with it.
    ///
    /// MUTATION-LINED. Verified red by reverting the `"Mask/Image"` arm of
    /// `classify_correction` to `unknown_component = true` (transcript in the
    /// batch report): the correction disappears and the gradient with it.
    #[test]
    fn an_ai_mask_imports_beside_the_shapes_it_used_to_take_down() {
        let doc = lr_doc(&lr_correction(
            "Mask 1",
            "",
            &format!("{}{}", lr_gradient("0"), lr_ai_mask("2", "0", "1", LR_AI_PROVENANCE, "")),
        ));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "the correction must import: {:?}", r.masks);
        let m = &r.masks[0];
        // The parametric shape is still the BASE — an AI mask does not displace
        // a gradient that was there first (`base_geometry_at`).
        assert!(matches!(m.mask, MaskGeometry::Linear { .. }), "{:?}", m.mask);
        assert_eq!(m.components.len(), 1, "the AI mask rides as a component");
        assert_eq!(m.components[0].mode, MaskCombine::Add, "MaskBlendMode=0 is a union");
        let MaskGeometry::AiMask {
            name,
            subtype,
            ref_x,
            ref_y,
            blend_mode,
            value,
            inverted,
            mask_version,
            provenance,
            gesture,
            raster,
        } = &m.components[0].geometry
        else {
            panic!("expected an AI mask, got {:?}", m.components[0].geometry);
        };
        assert_eq!(name.as_str(), "Sky 1");
        assert_eq!((*subtype, *blend_mode, *value, *inverted, *mask_version), (2, 0, 1.0, false, 1));
        assert_eq!((*ref_x, *ref_y), (0.605469, 0.281525), "the click arrives verbatim");
        assert!(gesture.is_empty(), "no crs:Gesture on this fixture");
        // NOTHING is resolved at parse time: importing a library must not spawn
        // a model run per photo.
        assert!(raster.is_none(), "the alpha is recomputed at DEVELOP time, not here");
        // Every provenance attribute, in document order, carried and untouched.
        let keys: Vec<&str> = provenance.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "InputDigest",
                "InputDigestVersion",
                "MaskDigest",
                "WholeImageArea",
                "Origin",
                "ModelVersion"
            ]
        );
        assert_eq!(provenance[3].1, "0/1,0/1,1920/1,2880/1");

        // And the import SAYS what kind of thing arrived — a RE-DERIVATION,
        // not Adobe's raster. Importing this silently would be worse than the
        // refusal it replaced.
        let losses = import_losses(&doc);
        assert!(
            losses.iter().any(|l| l.reason == MaskImportReason::AiMaskRecomputed),
            "a re-derived AI mask must be disclosed: {losses:?}"
        );
        let line = describe_import_losses(1, &losses).unwrap_or_default();
        assert!(
            line.contains("re-derived") && line.contains("Adobe"),
            "the prose must name the recomputation, not just the count: {line}"
        );
    }

    /// A correction whose ONLY component is an AI mask imports too, with the
    /// AI mask as its base — the 59 corrections in the census that carry
    /// nothing else.
    #[test]
    fn an_ai_only_correction_takes_the_ai_mask_as_its_base() {
        let doc = lr_doc(&lr_correction(
            "Mask 2",
            "",
            &lr_ai_mask("0", "0", "1", LR_AI_PROVENANCE, ""),
        ));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "an AI-only correction imports: {:?}", r.masks);
        assert!(matches!(r.masks[0].mask, MaskGeometry::AiMask { subtype: 0, .. }));
        assert!(r.masks[0].components.is_empty(), "the base is not also a component");
    }

    /// The `crs:Gesture` child — the photographer's brush refinement of the AI
    /// mask (40 of 105 instances) — is carried, so the corrections whose only
    /// brush content is a gesture arrive whole.
    #[test]
    fn an_ai_masks_gesture_strokes_are_carried() {
        let doc = lr_doc(&lr_correction(
            "Mask 3",
            "",
            &lr_ai_mask("2", "0", "1", LR_AI_PROVENANCE, &lr_paint_specimen()),
        ));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "{:?}", r.masks);
        let MaskGeometry::AiMask { gesture, .. } = &r.masks[0].mask else {
            panic!("expected an AI mask, got {:?}", r.masks[0].mask);
        };
        assert_eq!(gesture.len(), 1, "one Mask/Paint per Gesture, as measured");
        assert_eq!(gesture[0].value, 0.439815);
        assert_eq!(gesture[0].radius, 0.582157);
        assert!(gesture[0].dabs.starts_with("r 0.581835\nd 0.000684 0.940004"));
    }

    /// The write-back re-emits the component shape Lightroom wrote — the
    /// intent verbatim, so Lightroom rebuilds ITS own alpha from it — with a
    /// FRESH `crs:MaskSyncID` per this writer's rule.
    ///
    /// MUTATION-LINED. Verified red by dropping the `ai_mask_xml` routing from
    /// `masks_xml`'s base arm (the correction then exports as a bitmap-skip
    /// and the whole mask disappears from the sidecar) — transcript in the
    /// batch report.
    #[test]
    fn an_ai_mask_round_trips_back_into_the_sidecar() {
        let doc = lr_doc(&lr_correction(
            "Mask 4",
            "",
            &lr_ai_mask("2", "0", "1", LR_AI_PROVENANCE, ""),
        ));
        let r = xmp_to_recipe(&doc);
        let out = recipe_to_xmp(&r);
        assert!(out.contains(r#"crs:What="Mask/Image""#), "the component kind rides out:\n{out}");
        assert!(out.contains(r#"crs:MaskSubType="2""#), "the intent rides out:\n{out}");
        assert!(
            out.contains(r#"crs:ReferencePoint="0.605469 0.281525""#),
            "the click rides out at the file's own precision:\n{out}"
        );
        assert!(out.contains(r#"crs:MaskVersion="1""#), "the schema stamp rides out:\n{out}");
        for (k, v) in [
            ("InputDigest", "D0DAC04EB58F013F49D93EF47D22794E"),
            ("MaskDigest", "00D1A1B68591DF41F6CA3F8F805D0F1B"),
            ("WholeImageArea", "0/1,0/1,1920/1,2880/1"),
            ("ModelVersion", "234881976"),
        ] {
            assert!(
                out.contains(&format!("crs:{k}=\"{v}\"")),
                "provenance {k} must ride out unchanged:\n{out}"
            );
        }
        // FRESH SyncID — a sidecar we rewrite is OUR document, and this writer
        // mints identities for every component it emits.
        assert!(
            !out.contains("440777CD3CB8E24BB8E16028893B45DC"),
            "the file's own MaskSyncID must not be republished:\n{out}"
        );
        // And the EXPORT discloses the other direction of the same gap.
        let losses = mask_export_losses(&r);
        assert!(
            losses.iter().any(|l| l.reason == MaskLossReason::AiMaskRecomputed),
            "the export must say the pixels shown were not Adobe's: {losses:?}"
        );
    }

    /// An attribute outside the measured vocabulary is REFUSED, not carried.
    /// A name we have never seen means a writer we have not measured (the
    /// roundness rule) — and an open-ended attribute bag read off disk and
    /// written back into XML is an injection surface besides.
    ///
    /// MUTATION-LINED. Verified red by replacing `parse_ai_mask`'s
    /// `_ => return Err(())` arm with `_ => {}` (transcript in the batch
    /// report): the unknown attribute is silently dropped and the correction
    /// imports as if the file had said nothing surprising.
    #[test]
    fn an_unmeasured_attribute_on_an_ai_mask_is_refused_not_guessed() {
        let good = lr_doc(&lr_correction("ok", "", &lr_ai_mask("2", "0", "1", "", "")));
        assert_eq!(xmp_to_recipe(&good).masks.len(), 1, "the baseline imports");

        for extra in [
            "\n         crs:SomethingNobodyMeasured=\"1\"",
            // A subtype outside {0,1,2} has no backend, and guessing one would
            // invent a selection.
            "",
        ] {
            let doc = if extra.is_empty() {
                lr_doc(&lr_correction("bad", "", &lr_ai_mask("7", "0", "1", "", "")))
            } else {
                lr_doc(&lr_correction("bad", "", &lr_ai_mask("2", "0", "1", extra, "")))
            };
            assert!(
                xmp_to_recipe(&doc).masks.is_empty(),
                "a Mask/Image outside the measured encoding must not import ({extra:?})"
            );
            let losses = import_losses(&doc);
            assert!(
                losses.iter().any(|l| l.reason.is_drop()),
                "and the refusal must be NAMED: {losses:?}"
            );
        }
    }

    /// `MaskBlendMode="1"` + `MaskValue="0"` is Lightroom's SUBTRACT pair, not
    /// a muted mask — the same reading v0.31.1 taught this parser for
    /// parametric components, mapped once through `brush_combine`.
    #[test]
    fn an_ai_masks_subtract_pair_reads_as_a_subtraction() {
        let doc = lr_doc(&lr_correction(
            "Mask 5",
            "",
            &format!("{}{}", lr_gradient("0"), lr_ai_mask("1", "1", "0", "", "")),
        ));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "{:?}", r.masks);
        assert_eq!(r.masks[0].components.len(), 1);
        assert_eq!(
            r.masks[0].components[0].mode,
            MaskCombine::Subtract,
            "MaskBlendMode=1 carves out"
        );
        let MaskGeometry::AiMask { blend_mode, value, .. } = &r.masks[0].components[0].geometry
        else {
            panic!("expected an AI mask");
        };
        // The pair is CARRIED verbatim for the writer even though the render
        // reads the projected `MaskCombine` — two spellings of one fact.
        assert_eq!((*blend_mode, *value), (1, 0.0));
    }

    // ================================================================
    // R28 Batch-5 5d — THE FOUR ADVERSARIAL SCOPE FIXTURES (F4 A–D)
    //
    // All four are documents no Lightroom writes; the adjudication rated the
    // whole finding "mechanism real, zero sites reachable from real LR". They
    // are here because the DEFENCE used to be a coincidence — real Lightroom
    // happens not to put these names in these places — and a coincidence is not
    // a guard. The typed scope (`Tag` / `Scope`) plus the two narrowed searches
    // make them refusals by construction, and these four fixtures say so.
    // ================================================================

    /// A correction with NO `crs:LocalExposure2012` of its own, one gradient
    /// component, and whatever `extra` / `stray` the caller plants.
    ///
    /// `extra` goes on the COMPONENT (inside `crs:CorrectionMasks`); `stray` is
    /// spliced in as a child of the correction itself, before the component
    /// list. Hand-written rather than built from `lr_correction` deliberately:
    /// the point of A is a correction that OMITS a slider, and the Lightroom
    /// fixture writes every one of them.
    ///
    /// Authored by US (`x:xmptk="AutoShade 2"`), which matters for exactly one
    /// thing: `component_import_reasons` accepts a `Mask/RangeMask` only on our
    /// own documents (someone else's range encoding is not ours to interpret).
    /// A Lightroom-authored fixture would drop the range for THAT reason and
    /// the B control below could not tell the two refusals apart.
    fn scope_bleed_doc(extra: &str, stray: &str) -> String {
        format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"AutoShade 2\">\n\
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             <rdf:Description rdf:about=\"\"\n\
             \x20 xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
             \x20 crs:HasSettings=\"True\">\n\
             \x20<crs:MaskGroupBasedCorrections><rdf:Seq>\n\
             \x20 <rdf:li><rdf:Description crs:What=\"Correction\"\n\
             \x20  crs:CorrectionAmount=\"1\" crs:CorrectionActive=\"true\"\n\
             \x20  crs:CorrectionName=\"Mask 1\" crs:LocalContrast2012=\"0.2\">\n\
             {stray}\
             \x20  <crs:CorrectionMasks><rdf:Seq>\n\
             \x20   <rdf:li crs:What=\"Mask/Gradient\" crs:MaskActive=\"true\"\n\
             \x20    crs:MaskBlendMode=\"0\" crs:MaskInverted=\"false\" crs:MaskValue=\"1\"\n\
             \x20    crs:ZeroX=\"0.5\" crs:ZeroY=\"0.8\" crs:FullX=\"0.5\" crs:FullY=\"0.2\"{extra}/>\n\
             \x20  </rdf:Seq></crs:CorrectionMasks>\n\
             \x20 </rdf:Description></rdf:li>\n\
             \x20</rdf:Seq></crs:MaskGroupBasedCorrections>\n\
             </rdf:Description></rdf:RDF></x:xmpmeta>\n"
        )
    }

    /// **F4 SYMPTOM A** — a slider name on a NESTED component must not answer
    /// for the correction.
    ///
    /// The correction states no `crs:LocalExposure2012`; its gradient component
    /// carries one, at a value (`9`, i.e. +36 EV on the ×4 file scale) far
    /// outside Lightroom's own slider. The pre-5d reader scanned the whole
    /// correction segment for every slider, so it found the component's number
    /// and REFUSED the entire correction as out-of-model — the photographer
    /// lost a mask because of an attribute on a shape.
    ///
    /// The nearest real threat this closes: Lightroom really does write
    /// `crs:Local*` NAMES on nested components (`LocalInputDigest` and friends,
    /// 105 measured instances). They are strings nobody parses as numbers
    /// today, which is why nothing has caught fire — a coincidence, now a
    /// guard.
    ///
    /// MUTATION: hand `correction_value_reasons` / `parse_one_correction` the
    /// whole `seg` again instead of `own`, and the first assertion goes red.
    #[test]
    fn a_nested_components_slider_name_cannot_answer_for_the_correction() {
        let doc = scope_bleed_doc(" crs:LocalExposure2012=\"9\"", "");
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "the correction must still import: {:?}", r.masks);
        assert_eq!(
            r.masks[0].exposure_ev, 0.0,
            "the correction states no exposure; the component's number is not its value"
        );
        // The correction's OWN sliders still arrive — this is a narrowing, not
        // a blindfold.
        assert_eq!(r.masks[0].contrast, 20.0, "crs:LocalContrast2012=0.2 → +20");
        assert_eq!(unsupported_corrections(&doc), 0, "and nothing was dropped");
    }

    /// **F4 SYMPTOM B** — a `Mask/RangeMask` outside `crs:CorrectionMasks` is
    /// not this correction's range.
    ///
    /// The stray element below sits inside the correction but outside its
    /// component list, so no component walk ever counts it — which is exactly
    /// why attaching it was SILENT: `range_count` stayed 0, so the
    /// `ForeignRangeMask` disclosure could not fire either. The old search was
    /// a first-occurrence scan over the whole segment, and the reader then ran
    /// from that offset to the END of the segment, so even the colour arm's
    /// `rdf:li` could come from an unrelated component.
    ///
    /// MUTATION: restore `Scope::new(seg).find_value_at("What",
    /// "Mask/RangeMask")` + `&seg[p..]` and the range comes back.
    #[test]
    fn a_range_mask_outside_the_component_list_is_not_attached() {
        let stray = "\x20 <rdf:li crs:What=\"Mask/RangeMask\" crs:MaskActive=\"true\"\n\
                     \x20  crs:MaskBlendMode=\"1\" crs:MaskValue=\"0\" crs:MaskInverted=\"true\"\n\
                     \x20  crs:LumRange=\"0 0.2 0.8 1\"/>\n";
        let doc = scope_bleed_doc("", stray);
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "the correction still imports: {:?}", r.masks);
        assert!(
            r.masks[0].range.is_none(),
            "a range mask outside the component list is not this correction's: {:?}",
            r.masks[0].range
        );
        // …and the control: the SAME range component, INSIDE the list, is.
        let inside = scope_bleed_doc("", "").replace(
            "</rdf:Seq></crs:CorrectionMasks>",
            "<rdf:li crs:What=\"Mask/RangeMask\" crs:MaskActive=\"true\" \
             crs:MaskBlendMode=\"1\" crs:MaskValue=\"0\" crs:MaskInverted=\"true\" \
             crs:LumRange=\"0 0.2 0.8 1\"/></rdf:Seq></crs:CorrectionMasks>",
        );
        let r = xmp_to_recipe(&inside);
        assert!(
            matches!(r.masks[0].range, Some(RangeMask::Luminance { .. })),
            "an in-list range component must still be read, or this test proves nothing: {:?}",
            r.masks[0].range
        );
    }

    /// **F4 SYMPTOM C** — single-quoted attributes are legal XML, and the AI
    /// mask's closed-vocabulary gate has to run on them.
    ///
    /// `crs_attributes` hand-rolled its own lexer and looked for a DOUBLE
    /// quote, so on this document it found none and returned an empty list.
    /// Two consequences, both silent: the eleven provenance / digest keys were
    /// dropped on write-back (Lightroom's own recompute ledger, gone from a
    /// file we had just accepted), and the refusal loop that is supposed to
    /// reject an unmeasured attribute name never looked at anything.
    ///
    /// MUTATION: restore the `find('"')` lexer — the first half loses the
    /// digests, the second half stops refusing.
    #[test]
    fn single_quoted_attributes_carry_provenance_and_still_refuse_the_unknown() {
        let ai = |extra: &str| {
            format!(
                "       <rdf:li><rdf:Description crs:What='Mask/Image' crs:MaskActive='true'\n\
                 \x20        crs:MaskName='Sky 1' crs:MaskBlendMode='0' crs:MaskInverted='false'\n\
                 \x20        crs:MaskSyncID='440777CD3CB8E24BB8E16028893B45DC' crs:MaskValue='1'\n\
                 \x20        crs:MaskVersion='1' crs:MaskSubType='2'\n\
                 \x20        crs:ReferencePoint='0.605469 0.281525'\n\
                 \x20        crs:InputDigest='D0DAC04EB58F013F49D93EF47D22794E'\n\
                 \x20        crs:ModelVersion='234881976'{extra}/></rdf:li>\n"
            )
        };
        let doc = lr_doc(&lr_correction("Mask 1", "", &ai("")));
        let r = xmp_to_recipe(&doc);
        assert_eq!(r.masks.len(), 1, "a single-quoted AI mask imports: {:?}", r.masks);
        let MaskGeometry::AiMask { provenance, .. } = &r.masks[0].mask else {
            panic!("expected an AI mask, got {:?}", r.masks[0].mask);
        };
        assert_eq!(
            provenance.len(),
            2,
            "both provenance keys must be CARRIED, not silently dropped: {provenance:?}"
        );
        // …and they come back out, which is the loss the photographer feels.
        let out = recipe_to_xmp(&r);
        assert!(
            out.contains("crs:InputDigest=\"D0DAC04EB58F013F49D93EF47D22794E\"")
                && out.contains("crs:ModelVersion=\"234881976\""),
            "the digests must survive the round trip:\n{out}"
        );
        // The refusal loop runs on legal XML now: an unmeasured name costs the
        // correction, exactly as it does with double quotes.
        let bogus = lr_doc(&lr_correction("Mask 1", "", &ai(" crs:Bogus='1'")));
        assert!(
            xmp_to_recipe(&bogus).masks.is_empty(),
            "an attribute outside the measured vocabulary must still refuse"
        );
    }

    /// **F4 SYMPTOM D** — the declared frame comes from ONE `rdf:Description`.
    ///
    /// Width, length and orientation used to be three independent
    /// first-occurrence searches over the whole document, so a packet carrying
    /// a `tiff:ImageWidth` in one element and the real pair in another produced
    /// a frame no element declares — and that frame is the coordinate system
    /// every mask and crop decode folds pixel geometry with (`lr_to_engine`).
    ///
    /// MUTATION: make [`FrameScope::resolve`] return `FrameScope(doc)`
    /// unconditionally — i.e. point the three reads back at the whole document
    /// — and the frame becomes 6000 × 6336, which this file never states.
    #[test]
    fn the_declared_frame_comes_from_one_description() {
        let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             <rdf:Description rdf:about=\"\" xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\n\
             \x20 tiff:ImageWidth=\"6000\"/>\n\
             <rdf:Description rdf:about=\"\" xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\n\
             \x20 tiff:ImageWidth=\"9504\" tiff:ImageLength=\"6336\" tiff:Orientation=\"1\"/>\n\
             </rdf:RDF></x:xmpmeta>";
        let frame = FrameAspect::from_xmp(doc).expect("the second Description declares a frame");
        assert_eq!(
            (frame.w, frame.h),
            (9504.0, 6336.0),
            "both dimensions must come from the element that declares both"
        );
        // A document with no `rdf:Description` at all still reads: the
        // narrowing has nothing to protect in a bare fragment, and fixtures in
        // this module that hand `from_xmp` a snippet rely on it.
        let bare = "tiff:ImageWidth=\"800\" tiff:ImageLength=\"600\"";
        let frame = FrameAspect::from_xmp(bare).expect("a bare fragment still declares a frame");
        assert_eq!((frame.w, frame.h), (800.0, 600.0));
    }

    // ================================================================
    // R29-2 — THE FOUR ADVERSARIAL FRAME-SCOPE FIXTURES
    //
    // R28 Batch-5 5d typed the `crs:` reader's scope and left the `tiff:` frame
    // family on a bare-`&str` `declared_number` whose scope was a per-call-site
    // CONVENTION. `declared_number` is a method on [`FrameScope`] now, so the
    // question can only be asked of a span [`FrameScope::resolve`] produced.
    //
    // Two of the four pin FALLBACKS, not guarantees. `resolve` deliberately
    // widens to the whole document in the two cases its own doc comment names,
    // and both are behaviour a photographer's file can reach; they are fixtures
    // so that a later narrowing has to face them instead of changing the frame
    // — the coordinate system every mask and crop decode folds pixel geometry
    // with — by accident. None of these four documents is one Lightroom writes.
    // ================================================================

    /// **A** — HALF a frame in each of two `rdf:Description`s.
    ///
    /// One element declares only `tiff:ImageWidth`, the next only
    /// `tiff:ImageLength`. No element declares both, so `resolve` falls back to
    /// the whole document and the pair IS assembled across two elements — the
    /// pairing F4 symptom D removes when some element declares both, and which
    /// this document gives the reader no way to avoid. PINNED, not endorsed:
    /// the alternative is dropping the frame for files nobody has measured.
    ///
    /// MUTATION: drop the fallback (`resolve` returning the last candidate span
    /// instead of `doc`) and `from_xmp` returns `None` — the half-declaration
    /// the narrowed span sees is not a frame, which the control below states
    /// directly.
    #[test]
    fn half_a_frame_in_each_description_falls_back_to_the_whole_document() {
        let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             <rdf:Description rdf:about=\"\" xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\n\
             \x20 tiff:ImageWidth=\"9504\"/>\n\
             <rdf:Description rdf:about=\"\" xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\n\
             \x20 tiff:ImageLength=\"6336\" tiff:Orientation=\"1\"/>\n\
             </rdf:RDF></x:xmpmeta>";
        let frame = FrameAspect::from_xmp(doc).expect("the whole-document fallback still reads");
        assert_eq!(
            (frame.w, frame.h),
            (9504.0, 6336.0),
            "the documented fallback assembles the pair across the two elements"
        );
        // CONTROL: half a declaration is not a frame. Neither element on its
        // own answers, which is what makes the assertion above a statement
        // about the FALLBACK rather than about either span.
        let half = "<rdf:Description rdf:about=\"\" tiff:ImageWidth=\"9504\"/>";
        assert!(
            FrameAspect::from_xmp(half).is_none(),
            "a width with no length declares no rectangle"
        );
    }

    /// **B** — the two XMP spellings MIXED inside one `rdf:Description`.
    ///
    /// `tiff:ImageWidth` is an attribute on the start tag (what Lightroom
    /// writes); `tiff:ImageLength` and `tiff:Orientation` are property elements
    /// in the body (the same properties' other legal spelling). The scope runs
    /// from the element's `<` to the end of its body precisely so that one
    /// element declaring both — in either spelling, or one of each — counts as
    /// declaring both.
    ///
    /// The first Description is a DECOY carrying a lone `tiff:ImageWidth="6000"`:
    /// if the narrowing stopped working the whole-document fallback would read
    /// its 6000 first and the frame would be 6000 × 6336.
    ///
    /// MUTATION: make `resolve` end the span at the start tag's `>` (a `Tag`,
    /// not a scope) and the mixed element stops declaring a length, so the
    /// decoy's 6000 wins.
    #[test]
    fn the_frame_scope_sees_both_xmp_spellings_of_one_element() {
        let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             <rdf:Description rdf:about=\"\" xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\n\
             \x20 tiff:ImageWidth=\"6000\"/>\n\
             <rdf:Description rdf:about=\"\" xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\n\
             \x20 tiff:ImageWidth=\"9504\">\n\
             \x20<tiff:ImageLength>6336</tiff:ImageLength>\n\
             \x20<tiff:Orientation>8</tiff:Orientation>\n\
             </rdf:Description>\n\
             </rdf:RDF></x:xmpmeta>";
        let frame = FrameAspect::from_xmp(doc).expect("the mixed-spelling element declares both");
        assert_eq!(
            frame,
            FrameAspect::from_size_turned(9504.0, 6336.0, rawler::Orientation::from_u16(8))
                .expect("a positive rectangle"),
            "all three come from the ONE element that declares the pair, either spelling"
        );
    }

    /// **C** — a bare fragment with no `rdf:Description` at all.
    ///
    /// Both shapes fall back to the whole text, and that is load-bearing: this
    /// module's own fixtures hand `from_xmp` snippets, and a reader given a
    /// fragment cannot be mixing two elements' declarations because there are
    /// no elements to mix. The element-form fragment is the sharper of the two
    /// — it HAS markup, so it pins that the fallback keys on the absence of an
    /// `rdf:Description`, not on the absence of tags.
    ///
    /// MUTATION: make `resolve` return an empty span when no Description
    /// matched and both halves of this test lose their frame.
    #[test]
    fn the_frame_scope_falls_back_to_a_bare_fragment() {
        let attrs = "tiff:ImageWidth=\"800\" tiff:ImageLength=\"600\" tiff:Orientation=\"1\"";
        let frame = FrameAspect::from_xmp(attrs).expect("an attribute fragment declares a frame");
        assert_eq!((frame.w, frame.h), (800.0, 600.0));
        let elems = "<tiff:ImageWidth>800</tiff:ImageWidth>\n\
             <tiff:ImageLength>600</tiff:ImageLength>";
        let frame = FrameAspect::from_xmp(elems).expect("an element fragment declares a frame");
        assert_eq!(
            (frame.w, frame.h),
            (800.0, 600.0),
            "markup without an rdf:Description is still a fragment"
        );
    }

    /// **D** — `rdf:Description`s present, none of them carrying both
    /// dimensions.
    ///
    /// Three elements, three separate properties: an orientation, a width, a
    /// length. `resolve` finds no complete pair, falls back to the whole
    /// document, and the frame is assembled from all THREE — the exact reading
    /// F4 symptom D indicted, kept because refusing would drop the frame for a
    /// document class nobody has measured (the aspect is disclosed as degraded
    /// downstream either way). This is the residue R29-2 names rather than
    /// closes.
    ///
    /// MUTATION: return the FIRST `rdf:Description` seen instead of the
    /// whole-document fallback and the frame disappears — that element declares
    /// only an orientation.
    #[test]
    fn descriptions_without_a_complete_pair_read_as_the_whole_document() {
        let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             <rdf:Description rdf:about=\"\" xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\n\
             \x20 tiff:Orientation=\"8\"/>\n\
             <rdf:Description rdf:about=\"\" tiff:ImageWidth=\"9504\"/>\n\
             <rdf:Description rdf:about=\"\" tiff:ImageLength=\"6336\"/>\n\
             </rdf:RDF></x:xmpmeta>";
        let frame = FrameAspect::from_xmp(doc).expect("the whole-document fallback still reads");
        assert_eq!(
            frame,
            FrameAspect::from_size_turned(9504.0, 6336.0, rawler::Orientation::from_u16(8))
                .expect("a positive rectangle"),
            "no element declares the pair, so all three properties come from the document"
        );
    }

    // Authored synthetic MaskBrushTable payloads, Brotli-compressed once with
    // Python's `brotli` module. No byte below comes from a user specimen.
    const MB_GOOD_A_LEN: usize = 185;
    const MB_GOOD_A: &[u8] = &[
        0x1B, 0xB8, 0x00, 0xF8, 0x8F, 0xC2, 0xB6, 0xB5, 0x73, 0x94, 0x79, 0x28, 0xD3,
        0x42, 0xF8, 0xC9, 0x20, 0x88, 0x9B, 0xDF, 0xC6, 0x02, 0xEA, 0x3A, 0x0F, 0x6C,
        0x2C, 0x91, 0x28, 0x0E, 0x3C, 0xF0, 0x31, 0x51, 0xD6, 0x46, 0xAC, 0x01, 0x14,
        0x4E, 0xC4, 0xC3, 0x06, 0x9C, 0xA8, 0x07, 0x1E, 0xA0, 0x47, 0x32, 0xDD, 0x01,
        0x20, 0x10, 0xC7, 0x27, 0x96, 0x08, 0x80, 0x08, 0x00, 0x08, 0x00, 0x00, 0x58,
        0x10, 0xF9, 0xEE, 0xDC, 0x49, 0x6B, 0xC2, 0x58, 0x07, 0x20, 0x02, 0x42, 0x78,
        0x81, 0x98, 0x81, 0x5A, 0xD8, 0xAA, 0xBC, 0x89, 0xFA, 0x9B, 0xAA, 0x71, 0x28,
        0x13, 0x13, 0xC2, 0x58, 0xB7, 0xC6, 0x30, 0x36, 0x54, 0xBF, 0x44, 0x93, 0x3B,
        0x1A,
    ];
    const MB_GOOD_B_LEN: usize = 87;
    const MB_GOOD_B: &[u8] = &[
        0x1B, 0x56, 0x00, 0xF8, 0x9F, 0x07, 0x76, 0x0C, 0x99, 0x22, 0x68, 0xF8, 0x02,
        0xE9, 0xA5, 0x10, 0x26, 0xF7, 0x24, 0xE1, 0x08, 0xDB, 0x12, 0x4C, 0x23, 0xA8,
        0x84, 0xA0, 0x93, 0xE0, 0x81, 0xBA, 0x12, 0x66, 0x61, 0x03, 0x4E, 0x38, 0x0D,
        0x14, 0x47, 0x5A, 0x66, 0xBF, 0x1C, 0x20, 0xC1, 0x40, 0x03, 0xB5, 0x8C, 0xFB,
        0xE9, 0xD1, 0x02, 0x81, 0xBC, 0x35, 0x01,
    ];
    const MB_UNKNOWN_OPCODE_LEN: usize = 83;
    const MB_UNKNOWN_OPCODE: &[u8] = &[
        0x1B, 0x52, 0x00, 0xF8, 0x07, 0x61, 0x73, 0x13, 0xE9, 0x1A, 0xA2, 0xCD, 0x52,
        0xE5, 0xBC, 0x45, 0xD0, 0x05, 0x99, 0x7A, 0xA4, 0x03, 0x01, 0x80, 0x40, 0xA0,
        0x3C, 0x0C, 0x3E, 0xDD, 0x2B, 0xBD, 0x64, 0x08, 0xE4,
    ];
    const MB_TRAILING_LEN: usize = 88;
    const MB_TRAILING: &[u8] = &[
        0x1B, 0x57, 0x00, 0xF8, 0x9F, 0x07, 0x76, 0x0C, 0x99, 0x22, 0x68, 0xF8, 0x02,
        0xE9, 0xA5, 0x10, 0x26, 0xF7, 0x24, 0xE1, 0x08, 0xDB, 0x12, 0x4C, 0x23, 0xA8,
        0x84, 0xA0, 0x93, 0xE0, 0x81, 0xBA, 0x12, 0x66, 0x61, 0x03, 0x4E, 0x38, 0x0D,
        0x18, 0x37, 0x5A, 0x66, 0xBF, 0x1C, 0x20, 0xC1, 0x40, 0x03, 0xB5, 0x8C, 0xFB,
        0xE9, 0xD1, 0x02, 0x81, 0xBC, 0x35, 0x01,
    ];
    const MB_TABLE_WORD_LEN: usize = 8;
    const MB_TABLE_WORD: &[u8] =
        &[0x1B, 0x07, 0x00, 0xF8, 0xA7, 0x00, 0x04, 0x82, 0x92, 0x40, 0x20];
    const MB_RECORD_BOUND_LEN: usize = 8;
    const MB_RECORD_BOUND: &[u8] =
        &[0x8B, 0x03, 0x80, 0x01, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x03];
    const MB_DCOUNT_BOUND_LEN: usize = 78;
    const MB_DCOUNT_BOUND: &[u8] = &[
        0x1B, 0x4D, 0x00, 0xF8, 0x07, 0xE1, 0x64, 0x17, 0x12, 0x21, 0x6A, 0x4A, 0x35,
        0x1B, 0xD8, 0x80, 0x13, 0x4E, 0x03, 0x87, 0x05, 0x06, 0x5A, 0x4E, 0x07, 0x02,
        0x68, 0xC9, 0xC0, 0x02, 0xFD, 0xBC, 0xDA, 0x4B, 0x35, 0x14, 0x78, 0xAF,
    ];
    const MB_TOKEN_BOUND_LEN: usize = 327_772;
    const MB_TOKEN_BOUND: &[u8] = &[
        0x5B, 0x5B, 0x00, 0x85, 0x7F, 0x28, 0xF0, 0x76, 0x5F, 0x54, 0x42, 0xD4, 0x94,
        0x6A, 0x82, 0x76, 0x44, 0x97, 0xAE, 0x7E, 0x93, 0x08, 0x5C, 0xC0, 0x55, 0x49,
        0x08, 0x10, 0x80, 0x8D, 0x4F, 0xF7, 0x4D, 0xEB, 0xD0, 0x7D, 0xEF, 0x09, 0x20,
        0x84, 0xB1, 0x1F, 0x08,
    ];

    fn mb_object(stream: &[u8]) -> Vec<u8> {
        let mut object = Vec::with_capacity(16 + stream.len());
        for word in [4u32, 1, 64_000, stream.len() as u32] {
            object.extend_from_slice(&word.to_le_bytes());
        }
        object.extend_from_slice(stream);
        object
    }

    fn mb_acr(objects: &[Vec<u8>]) -> (Vec<u8>, Vec<String>) {
        let directory_end = 20 + 32 * objects.len();
        let mut offsets = Vec::with_capacity(objects.len());
        let mut at = directory_end as u64;
        for object in objects {
            offsets.push(at);
            at += object.len() as u64;
            at += (4 - at % 4) % 4;
        }
        let mut acr = Vec::with_capacity(at as usize);
        acr.extend_from_slice(b"ACR\0");
        acr.extend_from_slice(&1u32.to_le_bytes());
        acr.extend_from_slice(b"ARW\0");
        acr.extend_from_slice(&(objects.len() as u32).to_le_bytes());
        acr.extend_from_slice(&0u32.to_le_bytes());
        let mut tokens = Vec::new();
        for (object, offset) in objects.iter().zip(offsets) {
            let digest = md5::compute(object);
            acr.extend_from_slice(&digest.0);
            acr.extend_from_slice(&(object.len() as u64).to_le_bytes());
            acr.extend_from_slice(&offset.to_le_bytes());
            tokens.push(format!("{digest:X}"));
        }
        for object in objects {
            acr.extend_from_slice(object);
            while acr.len() % 4 != 0 {
                acr.push(0);
            }
        }
        (acr, tokens)
    }

    fn mb_temp(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "autoshade-mask-brush-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("synthetic.arw");
        std::fs::write(&raw, b"synthetic raw identity").unwrap();
        (dir, raw)
    }

    fn mb_group(name: &str, token: &str, bytes: usize) -> String {
        format!(
            "<rdf:li crs:What=\"Mask/Aggregate\" crs:MaskActive=\"true\" \
             crs:MaskName=\"{name}\" crs:MaskBlendMode=\"0\" crs:MaskInverted=\"false\" \
             crs:MaskSyncID=\"0000000000000000000000000000000D\" crs:MaskValue=\"1\" \
             crs:MaskBrushTable=\"{token}\" crs:MaskBrushUncompressedBytes=\"{bytes}\"/>\n"
        )
    }

    fn mb_doc(groups: &[(&str, &str, usize)]) -> String {
        let corrections: String = groups
            .iter()
            .map(|(correction, token, bytes)| {
                lr_correction(correction, "", &mb_group("Brush 1", token, *bytes))
            })
            .collect();
        lr_doc(&corrections)
    }

    fn mb_parse(
        raw: &std::path::Path,
        doc: &str,
    ) -> (EditRecipe, Vec<crate::diag::Line>) {
        let collector = crate::diag::Collector::new();
        let diag = crate::diag::Diag::about(&collector, raw);
        let recipe = xmp_to_recipe_with_diag(doc, &diag);
        (recipe, collector.take())
    }

    fn mb_assert_refusal(
        tag: &str,
        acr: Option<&[u8]>,
        token: &str,
        advertised: usize,
        expected: MaskBrushTableRefusal,
    ) {
        let (dir, raw) = mb_temp(tag);
        if let Some(acr) = acr {
            std::fs::write(raw.with_extension("acr"), acr).unwrap();
        }
        let doc = mb_doc(&[("Mask 1", token, advertised)]);
        let (recipe, lines) = mb_parse(&raw, &doc);
        assert!(recipe.masks.is_empty(), "a refused table imported partial geometry");
        let matching: Vec<_> = lines
            .iter()
            .filter(|line| line.text.contains(expected.name()))
            .collect();
        assert_eq!(matching.len(), 1, "named refusal must be loud exactly once: {lines:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mask_brush_tables_import_independently_in_owner_and_table_order() {
        let (dir, raw) = mb_temp("happy-multi");
        let objects = [mb_object(MB_GOOD_A), mb_object(MB_GOOD_B)];
        let (acr, tokens) = mb_acr(&objects);
        std::fs::write(raw.with_extension("acr"), acr).unwrap();
        let doc = mb_doc(&[
            ("First", &tokens[0], MB_GOOD_A_LEN),
            ("Second", &tokens[1], MB_GOOD_B_LEN),
        ]);
        let (recipe, lines) = mb_parse(&raw, &doc);
        assert!(lines.is_empty(), "valid tables emitted diagnostics: {lines:?}");
        assert_eq!(recipe.masks.len(), 2);
        let MaskGeometry::Brush { strokes: first, .. } = &recipe.masks[0].mask else {
            panic!("first table did not stay with its aggregate")
        };
        let MaskGeometry::Brush { strokes: second, .. } = &recipe.masks[1].mask else {
            panic!("second table did not stay with its aggregate")
        };
        assert_eq!(first.len(), 2);
        assert_eq!(second.len(), 1);
        assert_eq!(first[0].sync_id, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        assert_eq!(first[1].sync_id, "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB");
        assert_eq!(second[0].sync_id, "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mask_brush_fixed_point_fields_and_r_f_d_tokens_map_exactly() {
        let (dir, raw) = mb_temp("fixed-point");
        let (acr, tokens) = mb_acr(&[mb_object(MB_GOOD_A)]);
        std::fs::write(raw.with_extension("acr"), acr).unwrap();
        let recipe = mb_parse(&raw, &mb_doc(&[("Mask 1", &tokens[0], MB_GOOD_A_LEN)])).0;
        let MaskGeometry::Brush { strokes, .. } = &recipe.masks[0].mask else { panic!() };
        assert_eq!((strokes[0].value * 1_000_000.0).round() as u32, 51_402);
        assert_eq!((strokes[0].radius * 1_000_000.0).round() as u32, 36_957);
        assert_eq!((strokes[0].flow * 1_000_000.0).round() as u32, 1_000_000);
        assert_eq!(strokes[0].center_weight, 0.0);
        assert_eq!(
            strokes[0].dabs,
            "r 0.123456\nf 0.0103\nd 0.404621 0.692602\nd 0.401151 0.693698"
        );
        assert!(!strokes[0].dabs.lines().any(|token| token.starts_with("h ")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_mask_brush_companion_is_mask_brush_table_unavailable() {
        mb_assert_refusal(
            "unavailable",
            None,
            "00000000000000000000000000000000",
            MB_GOOD_A_LEN,
            MaskBrushTableRefusal::MaskBrushTableUnavailable,
        );
    }

    #[test]
    fn malformed_mask_brush_directory_is_container_invalid() {
        let (mut acr, tokens) = mb_acr(&[mb_object(MB_GOOD_A)]);
        acr[12..16].copy_from_slice(&((MAX_ACR_DIRECTORY_ENTRIES + 1) as u32).to_le_bytes());
        mb_assert_refusal(
            "container",
            Some(&acr),
            &tokens[0],
            MB_GOOD_A_LEN,
            MaskBrushTableRefusal::ContainerInvalid,
        );

        let (mut acr, tokens) = mb_acr(&[mb_object(MB_GOOD_A), mb_object(MB_GOOD_B)]);
        let len = le_u64_at(&acr, 36);
        let offset = le_u64_at(&acr, 44);
        let padding = usize::try_from(offset + len).unwrap();
        assert_eq!(acr[padding], 0, "fixture must have inter-object padding");
        acr[padding] = 1;
        mb_assert_refusal(
            "container-padding",
            Some(&acr),
            &tokens[0],
            MB_GOOD_A_LEN,
            MaskBrushTableRefusal::ContainerInvalid,
        );
    }

    #[test]
    fn absent_mask_brush_key_is_reference_mismatch() {
        let (acr, _) = mb_acr(&[mb_object(MB_GOOD_A)]);
        mb_assert_refusal(
            "reference",
            Some(&acr),
            "00000000000000000000000000000000",
            MB_GOOD_A_LEN,
            MaskBrushTableRefusal::ReferenceMismatch,
        );
    }

    #[test]
    fn changed_mask_brush_blob_is_digest_mismatch() {
        let (mut acr, tokens) = mb_acr(&[mb_object(MB_GOOD_A)]);
        let len = le_u64_at(&acr, 36);
        let offset = le_u64_at(&acr, 44);
        let last = usize::try_from(offset + len - 1).unwrap();
        acr[last] ^= 0x01;
        mb_assert_refusal(
            "digest",
            Some(&acr),
            &tokens[0],
            MB_GOOD_A_LEN,
            MaskBrushTableRefusal::DigestMismatch,
        );
    }

    #[test]
    fn unknown_mask_brush_envelope_is_encoding_unsupported() {
        let mut object = mb_object(MB_GOOD_A);
        object[0..4].copy_from_slice(&5u32.to_le_bytes());
        let (acr, tokens) = mb_acr(&[object]);
        mb_assert_refusal(
            "encoding",
            Some(&acr),
            &tokens[0],
            MB_GOOD_A_LEN,
            MaskBrushTableRefusal::EncodingUnsupported,
        );
    }

    #[test]
    fn invalid_mask_brush_brotli_is_corrupt() {
        let (acr, tokens) = mb_acr(&[mb_object(&[0xFF])]);
        mb_assert_refusal(
            "corrupt",
            Some(&acr),
            &tokens[0],
            1,
            MaskBrushTableRefusal::Corrupt,
        );
    }

    #[test]
    fn wrong_mask_brush_advertised_size_is_length_mismatch() {
        let (acr, tokens) = mb_acr(&[mb_object(MB_GOOD_A)]);
        mb_assert_refusal(
            "length",
            Some(&acr),
            &tokens[0],
            MB_GOOD_A_LEN + 1,
            MaskBrushTableRefusal::LengthMismatch,
        );
    }

    #[test]
    fn binary_h_opcode_is_payload_unsupported() {
        let (acr, tokens) = mb_acr(&[mb_object(MB_UNKNOWN_OPCODE)]);
        mb_assert_refusal(
            "payload-unsupported",
            Some(&acr),
            &tokens[0],
            MB_UNKNOWN_OPCODE_LEN,
            MaskBrushTableRefusal::PayloadUnsupported,
        );
    }

    #[test]
    fn trailing_mask_brush_payload_is_payload_invalid() {
        let (acr, tokens) = mb_acr(&[mb_object(MB_TRAILING)]);
        mb_assert_refusal(
            "payload-invalid",
            Some(&acr),
            &tokens[0],
            MB_TRAILING_LEN,
            MaskBrushTableRefusal::PayloadInvalid,
        );
    }

    #[test]
    fn one_bad_mask_brush_table_does_not_take_down_an_independent_table() {
        let objects = [mb_object(MB_GOOD_B), mb_object(MB_UNKNOWN_OPCODE)];
        let (acr, tokens) = mb_acr(&objects);
        let (dir, raw) = mb_temp("independent-refusal");
        std::fs::write(raw.with_extension("acr"), acr).unwrap();
        let survivor = lr_paint(
            "1111111111111111111111111111111C",
            "1",
            "0",
            "false",
            &["d 0.25 0.75"],
        );
        let bad_group = format!(
            "<rdf:li crs:What=\"Mask/Aggregate\" crs:MaskActive=\"true\" \
             crs:MaskName=\"Bad table\" crs:MaskBlendMode=\"0\" crs:MaskInverted=\"false\" \
             crs:MaskSyncID=\"0000000000000000000000000000000E\" crs:MaskValue=\"1\" \
             crs:MaskBrushTable=\"{}\" crs:MaskBrushUncompressedBytes=\"{}\">\n{}\
             </rdf:li>\n",
            tokens[1], MB_UNKNOWN_OPCODE_LEN, survivor
        );
        let doc = lr_doc(&format!(
            "{}{}",
            lr_correction(
                "Good",
                "",
                &mb_group("Brush 1", &tokens[0], MB_GOOD_B_LEN),
            ),
            lr_correction("Bad", "", &bad_group),
        ));
        let (recipe, lines) = mb_parse(&raw, &doc);
        assert_eq!(
            recipe.masks.len(),
            2,
            "the good table and bad table's text survivor must both import: {recipe:?}"
        );
        let MaskGeometry::Brush { strokes, .. } = &recipe.masks[1].mask else { panic!() };
        assert_eq!(strokes.len(), 1, "a refused table must contribute no partial records");
        assert_eq!(strokes[0].sync_id, "1111111111111111111111111111111C");
        assert!(lines.iter().any(|line| line.text.contains("PayloadUnsupported")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn non_one_mask_brush_table_word_is_payload_unsupported() {
        let (acr, tokens) = mb_acr(&[mb_object(MB_TABLE_WORD)]);
        mb_assert_refusal(
            "table-word",
            Some(&acr),
            &tokens[0],
            MB_TABLE_WORD_LEN,
            MaskBrushTableRefusal::PayloadUnsupported,
        );
    }

    #[test]
    fn mask_brush_record_count_bound_refuses_before_allocation() {
        let (acr, tokens) = mb_acr(&[mb_object(MB_RECORD_BOUND)]);
        mb_assert_refusal(
            "record-bound",
            Some(&acr),
            &tokens[0],
            MB_RECORD_BOUND_LEN,
            MaskBrushTableRefusal::PayloadInvalid,
        );
    }

    #[test]
    fn mask_brush_d_count_bound_refuses_before_token_walk() {
        let (acr, tokens) = mb_acr(&[mb_object(MB_DCOUNT_BOUND)]);
        mb_assert_refusal(
            "d-count-bound",
            Some(&acr),
            &tokens[0],
            MB_DCOUNT_BOUND_LEN,
            MaskBrushTableRefusal::PayloadInvalid,
        );
    }

    #[test]
    fn mask_brush_token_count_bound_covers_unbounded_state_tokens() {
        let (acr, tokens) = mb_acr(&[mb_object(MB_TOKEN_BOUND)]);
        mb_assert_refusal(
            "token-bound",
            Some(&acr),
            &tokens[0],
            MB_TOKEN_BOUND_LEN,
            MaskBrushTableRefusal::PayloadInvalid,
        );
    }

    #[test]
    fn table_import_preserves_residual_aggregate_and_gesture_paints() {
        let (dir, raw) = mb_temp("survivors");
        let (acr, tokens) = mb_acr(&[mb_object(MB_GOOD_B)]);
        std::fs::write(raw.with_extension("acr"), acr).unwrap();
        let aggregate_survivor = lr_brush_group("false", "");
        let gesture = lr_paint(
            "1111111111111111111111111111111B",
            "1",
            "0",
            "false",
            &["h 1.0000", "d 0.25 0.75"],
        )
        .replace(
            "crs:CenterWeight=\"0\">",
            "crs:CenterWeight=\"0\" crs:BrushGestureInterpretation=\"0\">",
        );
        let corrections = format!(
            "{}{}",
            lr_correction(
                "Brushes",
                "",
                &format!(
                    "{}{}",
                    mb_group("Binary", &tokens[0], MB_GOOD_B_LEN),
                    aggregate_survivor
                ),
            ),
            lr_correction("Gesture", "", &lr_ai_mask("0", "0", "1", "", &gesture)),
        );
        let doc = lr_doc(&corrections);
        let recipe = mb_parse(&raw, &doc).0;
        assert_eq!(recipe.masks.len(), 2);
        assert_eq!(recipe.masks[0].components.len(), 1, "aggregate survivor was dropped");
        let MaskGeometry::AiMask { gesture, .. } = &recipe.masks[1].mask else { panic!() };
        assert_eq!(gesture.len(), 1, "gesture survivor was dropped");
        assert!(gesture[0].dabs.starts_with("h 1.0000\n"), "text h token must survive");
        let merged = merge_recipe_into_xmp_in_frame_for_photo(&doc, &recipe, None, Some(&raw))
            .expect("the survivor document merges");
        assert!(merged.doc.contains("crs:BrushGestureInterpretation=\"0\""));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unchanged_table_mask_round_trip_keeps_attributes_without_text_paints() {
        let (dir, raw) = mb_temp("writer-round-trip");
        let (acr, tokens) = mb_acr(&[mb_object(MB_GOOD_A)]);
        std::fs::write(raw.with_extension("acr"), acr).unwrap();
        let doc = mb_doc(&[("Mask 1", &tokens[0], MB_GOOD_A_LEN)]);
        let mut recipe = mb_parse(&raw, &doc).0;
        recipe.exposure_ev = 0.75;
        let merged = merge_recipe_into_xmp_in_frame_for_photo(&doc, &recipe, None, Some(&raw))
            .expect("table-bearing base must merge");
        assert_eq!(merged.doc.matches("crs:MaskBrushTable=").count(), 1);
        assert!(merged.doc.contains(&format!("crs:MaskBrushTable=\"{}\"", tokens[0])));
        assert!(merged.doc.contains(&format!(
            "crs:MaskBrushUncompressedBytes=\"{MB_GOOD_A_LEN}\""
        )));
        assert_eq!(
            merged.doc.matches("crs:What=\"Mask/Paint\"").count(),
            0,
            "table records must not be synthesized as duplicate text Paints"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn env_mask_brush_sample_matches_stage_three_ground_truth() {
        let Some(root) = crate::config::live_env("AUTOSHADE_MB_SAMPLE_ROOT") else {
            eprintln!("SKIP env_mask_brush_sample_matches_stage_three_ground_truth: AUTOSHADE_MB_SAMPLE_ROOT unset");
            return;
        };
        let root = std::path::Path::new(&root);
        let xmp_path = root.join("_DSC8904-rewritten-brushtable.xmp");
        let photo = root.join("_DSC8904-rewritten.arw");
        let text = std::fs::read_to_string(&xmp_path).expect("read rewritten _DSC8904 XMP");
        let collector = crate::diag::Collector::new();
        let diag = crate::diag::Diag::about(&collector, &photo);
        let recipe = xmp_to_recipe_with_diag(&text, &diag);
        assert!(collector.take().is_empty(), "the confirmed specimen must parse without refusal");
        let mut tables: Vec<&Vec<BrushStroke>> = Vec::new();
        for mask in &recipe.masks {
            for geometry in std::iter::once(&mask.mask)
                .chain(mask.components.iter().map(|component| &component.geometry))
            {
                if let MaskGeometry::Brush { strokes, .. } = geometry
                    && matches!(strokes.len(), 54 | 4 | 18)
                {
                    tables.push(strokes);
                }
            }
        }
        assert_eq!(
            tables.iter().map(|table| table.len()).collect::<Vec<_>>(),
            [54, 4, 18],
            "the three table record counts stay in XMP order"
        );
        let d_count: usize = tables
            .iter()
            .flat_map(|table| table.iter())
            .map(|stroke| stroke.dabs.lines().filter(|token| token.starts_with("d ")).count())
            .sum();
        assert_eq!(d_count, 3_043);
        let t2_first = &tables[1][0];
        assert_eq!((t2_first.value * 1_000_000.0).round() as u32, 51_402);
        assert_eq!(t2_first.dabs.lines().next(), Some("d 0.404621 0.692602"));
    }
}
