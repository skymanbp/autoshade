//! Orientation and coordinate frames: quarter turns, the composed orientation, `CoordFrame`, and turning or shifting a recipe's stored geometry.

use super::*;

/// Apply the RAW's stored orientation so portraits/flips display correctly.
/// pub(crate): the decode side orients the embedded previews with the SAME
/// function, so GUI display and render pipeline can never disagree about
/// which way is up.
pub(crate) fn oriented(img: DynamicImage, o: Orientation) -> DynamicImage {
    match o {
        Orientation::Normal | Orientation::Unknown => img,
        Orientation::HorizontalFlip => img.fliph(),
        Orientation::Rotate180 => img.rotate180(),
        Orientation::VerticalFlip => img.flipv(),
        Orientation::Rotate90 => img.rotate90(),
        Orientation::Rotate270 => img.rotate270(),
        // The rotate must allocate (dims swap) but the flip runs IN PLACE:
        // the old rotate().fliph() chain held a THIRD full frame while the
        // source was still alive (~2.2 GB transient on a 61 MP Rgb32F frame).
        Orientation::Transpose => {
            let mut r = img.rotate90();
            drop(img);
            flip_h_in_place(&mut r);
            r
        }
        Orientation::Transverse => {
            let mut r = img.rotate270();
            drop(img);
            flip_h_in_place(&mut r);
            r
        }
    }
}

/// [`oriented`]'s public door, for the photographer's own quarter turns.
///
/// The GUI is a separate crate and cannot reach `oriented` (deliberately
/// `pub(crate)`: the EIGHT-state transform belongs to the decode/render pair,
/// and a UI that could pass an arbitrary `Orientation` could mirror a photo by
/// accident). A quarter turn is the one rotation a UI legitimately asks for,
/// so that is the shape of the door. Lossless — a pure axis swap on the
/// image's own pixel type.
pub fn turn_image(img: DynamicImage, quarter_turns: u8) -> DynamicImage {
    oriented(img, quarter_turn_orientation(quarter_turns))
}

/// [`oriented`]'s coordinate twin: where a NORMALISED point of the sensor
/// frame lands in the display frame.
///
/// Derived from [`oriented`] itself, state by state, so the two can never
/// disagree — `image`'s `rotate90` is CLOCKWISE, mapping pixel `(x, y)` of a
/// `W×H` frame to `(H−1−y, x)` of the `H×W` result, i.e. `(u, v) → (1−v, u)`
/// normalised; `Transpose`/`Transverse` compose that with the horizontal flip
/// exactly as `oriented` does. Every state is its own bijection of the unit
/// square, which is what makes the era-0 → era-1 recipe migration lossless
/// and round-trippable (`orient_point_round_trips_through_its_inverse`).
///
/// Points OUTSIDE [0,1] are mapped by the same affine rule — mask gradients
/// legitimately live off-frame (ACR geometry), and clamping them here would
/// silently shorten a gradient's falloff.
pub fn orient_point(o: Orientation, u: f32, v: f32) -> (f32, f32) {
    match o {
        Orientation::Normal | Orientation::Unknown => (u, v),
        Orientation::HorizontalFlip => (1.0 - u, v),
        Orientation::Rotate180 => (1.0 - u, 1.0 - v),
        Orientation::VerticalFlip => (u, 1.0 - v),
        Orientation::Transpose => (v, u),
        Orientation::Rotate90 => (1.0 - v, u),
        Orientation::Transverse => (1.0 - v, 1.0 - u),
        Orientation::Rotate270 => (v, 1.0 - u),
    }
}

/// The LINEAR part of [`orient_point`]: where a DISPLACEMENT measured in the
/// source frame lands in the display frame. `orient_point` is affine, so this
/// is `orient_point(o, p + d) − orient_point(o, p)` for every `p` — spelled
/// out per state rather than computed as that difference, so a small vector
/// is not read off a subtraction of two numbers near 1 (pinned by
/// `orient_vector_is_the_linear_part_of_orient_point`). It is what the era-2
/// recipe migration turns the develop window's move by.
pub fn orient_vector(o: Orientation, dx: f32, dy: f32) -> (f32, f32) {
    match o {
        Orientation::Normal | Orientation::Unknown => (dx, dy),
        Orientation::HorizontalFlip => (-dx, dy),
        Orientation::Rotate180 => (-dx, -dy),
        Orientation::VerticalFlip => (dx, -dy),
        Orientation::Transpose => (dy, dx),
        Orientation::Rotate90 => (-dy, dx),
        Orientation::Transverse => (-dy, -dx),
        Orientation::Rotate270 => (dy, -dx),
    }
}

/// The orientation of `quarter_turns` CLOCKWISE quarter turns on their own —
/// the photographer's half of [`compose_orientation`].
///
/// Clockwise because [`oriented`] is: `image`'s `rotate90` turns clockwise
/// (its derivation is spelled out on [`orient_point`]), so `Rotate90` here and
/// a click on the 「turn right」 button mean the same motion. Values outside
/// 0..=3 fold (`% 4`), the same residue-class rule
/// `EditRecipe::clamp` applies to the field itself.
pub fn quarter_turn_orientation(quarter_turns: u8) -> Orientation {
    match quarter_turns % 4 {
        1 => Orientation::Rotate90,
        2 => Orientation::Rotate180,
        3 => Orientation::Rotate270,
        _ => Orientation::Normal,
    }
}

/// The ONE orientation every consumer reads: the camera's EXIF state followed
/// by the photographer's `quarter_turns` clockwise quarter turns, composed
/// into a single [`Orientation`].
///
/// This is the skeleton's root insight (ROADMAP 7.2): the eight EXIF states
/// ARE the dihedral group of the square, which is CLOSED under composition, so
/// a user turn on top of a `Transpose` file lands on a state [`oriented`],
/// [`orient_point`] and [`orient_recipe_coords`] already handle exactly. No
/// second rotation stage anywhere in the pipeline, no new geometry code.
///
/// **Composition order is `exif` FIRST**: `orient_point(compose(e, k), p) ==
/// orient_point(R90^k, orient_point(e, p))`. That is the order the pixels take
/// — `render_to_image_in` orients the sensor buffer into the display frame and
/// the user's turn is a turn OF that display frame — and it is asserted
/// exhaustively over all 9×4 (state, turn) pairs by
/// `compose_orientation_is_the_composition_of_the_two_coordinate_maps`.
///
/// **Implementation.** Each state is `(swap, flip_h, flip_v)` with the flips
/// taken in the SOURCE frame and the swap last — rawler's own `to_flips`
/// contract ("flipping must be done before transposing"), which is
/// bit-for-bit the convention [`orient_point`] was independently derived in
/// (checked state by state in the test above). Composing two such triples:
/// the flips XOR, and when the first map swaps, the second map's flips arrive
/// on exchanged axes and cross over. `Unknown` is [`Orientation::Normal`]'s
/// twin on the way in and never appears on the way out — the group has eight
/// elements, not nine.
pub fn compose_orientation(exif: Orientation, quarter_turns: u8) -> Orientation {
    compose_two(exif, quarter_turn_orientation(quarter_turns))
}

/// The INVERSE of [`compose_orientation`] in its second argument: the quarter
/// turns that carry a photograph's own EXIF state to `want`, or `None` when no
/// quarter turn can.
///
/// **THE SIDECAR ORIENTATION LAW, written once and cited from both sides of
/// the XMP boundary.** Lightroom keeps a photograph's current orientation in
/// the sidecar's `tiff:Orientation`, and THAT value — not the RAW's own EXIF
/// tag — is the frame it delivers. Measured on the me6-2026-09 pack: the RAW
/// carries IFD0 `Orientation = 8` (`Rotate270`), all 46 sidecars declare
/// `tiff:Orientation="1"`, and Lightroom exported 6240 x 4160 LANDSCAPE from
/// every one of them. `E-CLICK.xmp` — the one sidecar Lightroom 9.4 rewrote
/// itself, after the user hid a mask and pressed Ctrl+S — wrote
/// `tiff:Orientation="1"` back beside corrected 6240 x 4160 dimensions, so the
/// value Lightroom honours is also the value Lightroom writes.
///
/// This engine's delivered frame is `compose_orientation(EXIF, quarter_turns)`
/// ([`crate::decode::decode_raw_turned`], `render_to_image_in`), so the two
/// engines agree exactly when
///
/// ```text
///     compose_orientation(the RAW's EXIF, quarter_turns) == the sidecar's tiff:Orientation
/// ```
///
/// and this function solves that for `quarter_turns`. The import side reads it
/// ([`crate::xmp`]'s `honour_declared_orientation`); the export side composes
/// it forward again ([`crate::pipeline::photo_frame_aspect`] feeding
/// `xmp::frame_declaration`). One equation, two directions, no second model.
///
/// **When there is no answer.** The eight EXIF states are the dihedral group
/// of the square; the four quarter turns are its rotation subgroup, of index
/// two. `want` is therefore reachable exactly when it has the same HANDEDNESS
/// as `exif` — both mirrored (`tiff:Orientation` 2/4/5/7) or both not (1/3/6/8)
/// — and a mirrored sidecar over an un-mirrored capture asks for a reflection
/// this engine has no stage to perform. `quarter_turns` is a count of quarter
/// turns and not a whole [`Orientation`] on purpose ([`quarter_turn_orientation`]
/// says why), so it cannot encode one either. `None` says so, and the importer
/// refuses that tag BY NAME instead of mapping it to the nearest rotation,
/// which would mirror every mask in the file without telling anyone.
///
/// Solved by SEARCH over the four turns rather than by a second piece of group
/// algebra: `compose_two` is the only composition in this build, and a closed
/// form here would be a second one to keep in step with it. Four comparisons.
pub fn quarter_turns_between(exif: Orientation, want: Orientation) -> Option<u8> {
    // `Unknown` is `Normal`'s twin on the way in everywhere else
    // (`decode::raw_orientation_of`), and `compose_orientation` never returns
    // it, so it has to be folded before the comparison or an unreadable tag
    // would answer `None` and be reported as a refused mirror.
    let fold = |o| if matches!(o, Orientation::Unknown) { Orientation::Normal } else { o };
    let want = fold(want);
    (0u8..4).find(|k| compose_orientation(fold(exif), *k) == want)
}

/// `b ∘ a` in coordinate terms — apply `a`, then `b`. Private because the only
/// composition the pipeline needs is [`compose_orientation`]'s; exposing a
/// general group operation would invite a second place to decide the order.
fn compose_two(a: Orientation, b: Orientation) -> Orientation {
    let (t1, h1, v1) = a.to_flips();
    let (t2, h2, v2) = b.to_flips();
    // `a` did not swap: `b`'s flips act on the same axes, so they simply XOR
    // and `b`'s swap is the composed swap. `a` DID swap: `b`'s horizontal flip
    // now lands on what was the vertical source axis (and vice versa), so the
    // two cross before XOR-ing, and the swaps XOR.
    let (h, v) = if t1 { (h1 ^ v2, v1 ^ h2) } else { (h1 ^ h2, v1 ^ v2) };
    Orientation::from_flips((t1 ^ t2, h, v))
}

/// Does this orientation MIRROR the frame (an odd number of reflections)?
/// The four reversing states are the ones whose `to_flips` triple has an odd
/// parity, and they are the only ones that flip the SIGN of a rotation angle
/// — the ellipse-`angle` half of [`orient_recipe_coords`].
pub(super) fn orientation_mirrors(o: Orientation) -> bool {
    matches!(
        o,
        Orientation::HorizontalFlip
            | Orientation::VerticalFlip
            | Orientation::Transpose
            | Orientation::Transverse
    )
}

/// The frame a recipe's coordinates are CURRENTLY measured against, reduced to
/// the one number a turn needs from it: `W/H`.
///
/// Every other geometry in a recipe is normalised TWICE — `x` against the width
/// and `y` against the height — so turning the unit square carries it with no
/// knowledge of the frame's shape at all ([`orient_point`] is aspect-free by
/// construction). A BRUSH is the exception: `crs:Radius` and the dab stream's
/// `r` token are in WIDTH units while a dab is a circle in PIXELS
/// (`rasterise_brush_group`'s `aspect`), so a quarter turn — which exchanges
/// `W` and `H` — has to rescale every radius by `W/H` or the strokes come back
/// elliptical. That missing input is what R29 Batch-6b had to register and what
/// this type supplies.
///
/// **Which frame.** The one the coordinates are in BEFORE the turn, always:
/// the SENSOR rectangle for the `coord_era` migration and for an XMP import
/// (era-0 numbers and `crs:` numbers are both source-frame), the CURRENT
/// display rectangle for a photographer's rotate and for the export projection
/// back into the source frame.
///
/// `None` from [`new`](Self::new) for anything that is not a positive, finite
/// rectangle: a zero dimension makes the rescale singular, and guessing at one
/// would move a mask by an unbounded factor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoordFrame {
    /// `W/H` of the frame the coordinates are in before the turn.
    aspect: f32,
}

impl CoordFrame {
    /// The frame of a `w × h` pixel rectangle. `f64` in, because every caller
    /// has pixel counts (`decode::source_frame`, `xmp::FrameAspect`) and
    /// narrowing them at the boundary is what loses a 61 MP dimension's last
    /// digits before the division rather than after.
    pub fn new(w: f64, h: f64) -> Option<Self> {
        (w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0)
            .then(|| CoordFrame { aspect: (w / h) as f32 })
            .filter(|f| f.aspect.is_finite() && f.aspect > 0.0)
    }

    /// What a brush radius must be multiplied by so its dab stays the SAME
    /// circle in pixels after the turn.
    ///
    /// A dab's pixel radius is `r · W`. The turned frame's width is `H` for the
    /// four transposing states and `W` for the other four, so `r' = r · W / W'`
    /// is `W/H` and `1` respectively. Nothing else in the stroke is spatial —
    /// `f` (flow) and `h` (hardness) are deposit laws, and `MaskValue` is a
    /// density.
    fn brush_radius_scale(self, o: Orientation) -> f32 {
        if crate::decode::orientation_transposes(o) { self.aspect } else { 1.0 }
    }
}

/// Lightroom's own precision for a brush number: six decimal places, FIXED.
///
/// Measured on the user's library rather than assumed — `P49.xmp` writes
/// `r 0.218584`, `d 0.067120 0.096097` and `crs:Radius="0.216487"`, trailing
/// zeros included, and every token in the 22,966-token census has that shape.
/// Re-emitting a TURNED stream in the same form is what keeps the rewrite
/// invisible in the file, and on the pure rotations it is also what makes the
/// rewrite invertible in DECIMAL: `1 − 0.096097` is `0.903903` to six places
/// and back again, so a portrait capture's import → export round trip hands
/// Lightroom its own digits instead of an f32's shortest form.
const LR_DAB_DECIMALS: usize = 6;

/// One brush number as the sidecar spells it, or `None` for a value that has
/// overflowed out of the grammar (`xmp::dab_token_is_known` refuses a
/// non-finite token, so writing one would make the stream un-importable).
fn lr_dab_str(v: f32) -> Option<String> {
    v.is_finite().then(|| format!("{v:.prec$}", prec = LR_DAB_DECIMALS))
}

/// The same six-place grid applied to a stored `f32` — `BrushStroke::radius`,
/// which is a FIELD and not text, so it is quantised rather than formatted.
///
/// One rule for both halves of a stroke: the attribute and the `r` tokens are
/// two spellings of the same quantity, and letting them drift apart by the
/// width of a formatter would make a re-imported sidecar disagree with the
/// `recipe.json` it came from. Falls back to the un-quantised value when the
/// scaling by `1e6` overflows, which only a hand-written recipe can reach.
fn lr_dab_round(v: f32) -> f32 {
    let q = (v * 1e6).round() / 1e6;
    if q.is_finite() { q } else { v }
}

/// One `crs:Dabs` token rewritten into the turned frame — or `None` when it is
/// not one of the two SPATIAL forms, in which case the caller carries the token
/// through unchanged.
///
/// `d <x> <y>` moves through [`orient_point`] like every other coordinate in
/// this file. `r <f>` is rescaled ONLY when the turn exchanges the axes, so a
/// half turn or a mirror leaves every radius token byte-identical. `f` and `h`
/// are never spatial and never touched.
///
/// A malformed token is `None` and therefore VERBATIM, matching `brush_dabs`'
/// own rule: the XMP boundary already refuses anything outside the grammar, and
/// a hand-edited `recipe.json` must not have its stream silently shortened by a
/// migration. An overflowed result is `None` for the same reason.
fn turned_dab_token(token: &str, o: Orientation, radius_scale: f32) -> Option<String> {
    let mut it = token.split_whitespace();
    match it.next()? {
        "d" => {
            let (x, y) = (brush_token_num(&mut it)?, brush_token_num(&mut it)?);
            if it.next().is_some() {
                return None;
            }
            let (x, y) = orient_point(o, x, y);
            Some(format!("d {} {}", lr_dab_str(x)?, lr_dab_str(y)?))
        }
        "r" if radius_scale != 1.0 => {
            let v = brush_token_num(&mut it)?;
            if it.next().is_some() {
                return None;
            }
            Some(format!("r {}", lr_dab_str(v * radius_scale)?))
        }
        _ => None,
    }
}

/// Turn a brush's strokes: every dab coordinate through [`orient_point`], every
/// radius through [`CoordFrame::brush_radius_scale`].
///
/// **The stream is REBUILT, not edited.** `split('\n')` is the exact inverse of
/// the join `xmp::parse_dabs` performs (a token carrying a newline is refused
/// there), so an EMPTY stream comes back empty, a stream of nothing but `f`/`h`
/// state comes back byte for byte, and the token count cannot change.
fn turn_brush_strokes(
    strokes: &mut [crate::recipe::BrushStroke],
    o: Orientation,
    frame: CoordFrame,
) {
    let scale = frame.brush_radius_scale(o);
    for s in strokes.iter_mut() {
        // The stroke ATTRIBUTE is the stream's initial state (102 real
        // components carry no `r` token at all), so it rides the same scale.
        if scale != 1.0 {
            s.radius = lr_dab_round(s.radius * scale);
        }
        let mut out = String::with_capacity(s.dabs.len() + 16);
        for (i, token) in s.dabs.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            match turned_dab_token(token, o, scale) {
                Some(t) => out.push_str(&t),
                None => out.push_str(token),
            }
        }
        s.dabs = out;
    }
}

/// One `crs:Dabs` token translated by `(du, dv)` — the era-2 twin of
/// [`turned_dab_token`]: only `d <x> <y>` is spatial under a translation (a
/// radius is a length, and a translation changes no length), and a malformed
/// or overflowed token is `None` and therefore carried verbatim, for the
/// reasons that function states.
fn shifted_dab_token(token: &str, du: f32, dv: f32) -> Option<String> {
    let mut it = token.split_whitespace();
    if it.next()? != "d" {
        return None;
    }
    let (x, y) = (brush_token_num(&mut it)?, brush_token_num(&mut it)?);
    if it.next().is_some() {
        return None;
    }
    Some(format!("d {} {}", lr_dab_str(x + du)?, lr_dab_str(y + dv)?))
}

/// Translate a brush's strokes by `(du, dv)`: every dab coordinate, nothing
/// else — the stream rebuilt token by token exactly as [`turn_brush_strokes`]
/// rebuilds it, so the token count cannot change.
fn shift_brush_strokes(strokes: &mut [crate::recipe::BrushStroke], du: f32, dv: f32) {
    for s in strokes.iter_mut() {
        let mut out = String::with_capacity(s.dabs.len() + 16);
        for (i, token) in s.dabs.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            match shifted_dab_token(token, du, dv) {
                Some(t) => out.push_str(&t),
                None => out.push_str(token),
            }
        }
        s.dabs = out;
    }
}

/// Rewrite a recipe's stored GEOMETRY from the sensor frame into the display
/// frame — the deterministic, bijective half of the `coord_era` 0 → 1
/// migration (`pipeline::migrate_recipe_coord_frame` owns the gating).
///
/// Moves the crop rectangle, every mask geometry (base + components) and the
/// Range-Mask colour sample point through [`orient_point`]. Returns `false`
/// for the identity orientations, so the caller can tell "nothing to do" from
/// "moved".
///
/// **Ellipse angle.** `MaskGeometry::Radial`'s `top/left/bottom/right` is a
/// centre+radii carrier, not a true bounding box, and `angle` rotates the
/// ellipse inside it (see `mask_weight`). Mapping the two corners through
/// `orient_point` already swaps the radii for the four transposing states,
/// which is exactly equivalent to leaving them alone and adding ±90° to the
/// angle — an ellipse rotated a quarter turn IS the same ellipse with its
/// axes exchanged. So a pure ROTATION needs no angle change at all; a
/// MIRROR needs the angle negated, because a reflection reverses the sense of
/// rotation. Verified algebraically against `mask_weight`'s own quadratic
/// form and pinned by `rotated_radial_mask_covers_the_rotated_pixels`.
///
/// **Not moved here: `MaskGeometry::Bitmap`.** A raster mask is a FILE of
/// pixels sampled in normalised coordinates, not a coordinate. This function
/// leaves its path alone; the callers that own the file re-write it —
/// `pipeline::rotate_recipe` and, since v1.6.0, the `coord_era` migration
/// (`pipeline::migrate_raster_file`), which until then could only disclose it.
///
/// **Migrated since R29 C1: `MaskGeometry::Brush`, by NUMERICALLY REWRITING its
/// dab stream** (and an `AiMask`'s `crs:Gesture` strokes, which are the same
/// payload under a different parent). Until this batch the stream was carried
/// verbatim so a republished sidecar was byte-faithful to Lightroom's, and the
/// brush rendered nothing, so an un-turned stream was invisible; R29 Batch-6b
/// made the brush DRAW, which turned that verbatim carry into a mask left at
/// its old coordinates while every parametric shape beside it moved. The user's
/// ruling (2026-08-21) is that the render is what must be right: coordinates
/// turn, radii rescale by the frame aspect, and a rotated photo's republished
/// dab stream is no longer byte-identical to the one Lightroom wrote — it is
/// still legal, still six decimal places, and still says the same mask about
/// the frame the document declares. An UNROTATED photo is untouched: the
/// identity orientations return before any of this, so their streams cannot
/// change even by a formatter.
///
/// This is the one arm that needs `frame`, and the reason is a unit mismatch,
/// not the coordinates: see [`CoordFrame`].
///
/// **The straighten angle** (R27, closing the R24 registration
/// 「`straighten≠0` 时 crop 迁移一阶近似」). `Crop` is normalised against the
/// STRAIGHTENED frame — `render_pipeline` runs `rotate_straighten` before
/// `apply_crop` — so migrating the rectangle correctly means knowing what the
/// straighten does under the same turn. Two facts settle it:
///
/// * [`inscribed_dims`] is SWAP-EQUIVARIANT: `inscribed_dims(h, w, deg)` is
///   `inscribed_dims(w, h, deg)` with its two outputs exchanged. The general
///   branch is `((w·c − h·s)/cos2, (h·c − w·s)/cos2)`, visibly so; the thin
///   branch's `if w >= h` looks asymmetric but is unreachable at `w == h`,
///   because that branch needs `short ≤ sin(2a)·long`, i.e. `sin 2a ≥ 1`, i.e.
///   exactly 45°, where `s == c` makes its two outputs equal anyway. So the
///   inscribed rectangle of the TURNED frame is the turn of the inscribed
///   rectangle, and normalised coordinates inside it map by `orient_point`
///   with nothing left over.
/// * Rotations commute (`rot(deg) ∘ R90 == R90 ∘ rot(deg)`), so for the four
///   pure rotations the migration was ALREADY exact. Reflections do not:
///   `rot(deg) ∘ M == M ∘ rot(−deg)`. Leaving the angle alone through a
///   MIRROR therefore straightened the frame the wrong way by `2·deg` — the
///   approximation R24 registered — and every crop coordinate then indexed
///   content that had been rotated out from under it.
///
/// The fix is the rule already applied to the ellipse `angle` just below: a
/// reflection reverses the sense of a rotation, so negate it. With that, all
/// eight states are exact and the migration stays the bijection its
/// round-trip test claims.
///
/// **Not migrated, and correctly so: `Radial::midpoint`** (R25 P5). It is a
/// ratio along the ellipse's own falloff axis, not a point in the frame — the
/// same status a tone-curve point has — so turning the frame leaves it
/// meaning exactly what it meant. Said out loud because the next reader will
/// scan this function for "every geometry field" and find one it skips.
///
/// **Not migrated, and correctly so: the four local point curves** (R25 P6,
/// `LocalAdjustment::main_curve` …). Their points are `{input, output}` pairs
/// on the 0..255 TONE axis, not positions in the frame: rotating a photo
/// changes which pixels a mask covers, never what value 128 maps to. Stated
/// here for the same reason as `midpoint` — they are fields on a mask, and a
/// reader auditing "did the migration cover every mask field?" must find the
/// answer rather than a silence.
///
/// **`frame`** is the shape of the rectangle the coordinates are in BEFORE the
/// turn, and only the brush arm reads it ([`CoordFrame`]). `None` means "not
/// known here", and the honest consequence is that a brush's dabs are left
/// where they were — every production caller supplies one when it can.
/// `pipeline::rotate_recipe`, which reads the photo's header lazily, passes
/// `None` exactly when the recipe holds no brush stroke at all
/// ([`recipe_has_brush_strokes`]); the sidecar import (W14) passes it for a
/// document that declares an orientation but no rectangle, beside a
/// photograph whose own rectangle cannot be read, and takes that consequence
/// knowingly — the boxes and points turn, a brush's dabs stay.
pub fn orient_recipe_coords(
    r: &mut EditRecipe,
    o: Orientation,
    frame: Option<CoordFrame>,
) -> bool {
    if matches!(o, Orientation::Normal | Orientation::Unknown) {
        return false;
    }
    let mirrors = orientation_mirrors(o);
    // The straighten rides the same sign rule as an ellipse angle, and for the
    // same reason (see the doc comment): a reflection reverses the sense of a
    // rotation, and `rotate_straighten` runs BEFORE `apply_crop`, so getting
    // this wrong moves the content under every crop coordinate below.
    if mirrors {
        r.straighten_deg = -r.straighten_deg;
    }
    if let Some(c) = r.crop.as_mut() {
        let (x0, y0) = orient_point(o, c.left, c.top);
        let (x1, y1) = orient_point(o, c.right, c.bottom);
        *c = Crop {
            left: x0.min(x1),
            right: x0.max(x1),
            top: y0.min(y1),
            bottom: y0.max(y1),
        };
    }
    let turn = |g: &mut MaskGeometry, owned_alpha: bool| match g {
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y }
        | MaskGeometry::Shaped { zero_x, zero_y, full_x, full_y, .. } => {
            (*zero_x, *zero_y) = orient_point(o, *zero_x, *zero_y);
            (*full_x, *full_y) = orient_point(o, *full_x, *full_y);
        }
        MaskGeometry::Radial { top, left, bottom, right, angle, .. } => {
            let (x0, y0) = orient_point(o, *left, *top);
            let (x1, y1) = orient_point(o, *right, *bottom);
            (*left, *right) = (x0.min(x1), x0.max(x1));
            (*top, *bottom) = (y0.min(y1), y0.max(y1));
            if mirrors {
                *angle = -*angle;
            }
        }
        // Raster masks carry no coordinates — see the doc comment.
        MaskGeometry::Bitmap { .. } => {}
        // A brush group carries thousands of coordinates inside `crs:Dabs`,
        // and since R29 C1 they TURN — numerically, token by token. The
        // registration this arm used to hold ("un-turned, because the function
        // is handed an `Orientation` and nothing else") is closed by `frame`;
        // the doc comment above carries the ruling and the cost.
        MaskGeometry::Brush { strokes, .. } => {
            if let Some(f) = frame {
                turn_brush_strokes(strokes, o, f);
            }
        }
        // An AI mask carries a reference coordinate and may carry gesture dab
        // coordinates, so both turn like every other point in the frame. Its
        // cached alpha does not: that raster was segmented in the OLD frame, so
        // rotating it is not a coordinate migration, it is a re-render. The
        // cache is DROPPED and the next develop recomputes it at the turned
        // point (`segment::resolve_ai_masks`). A TRANSLATION keeps it instead
        // (`shift_recipe_coords`): the same pixels, moved, are still the right
        // alpha, and the file is re-written by the migration.
        //
        // `owned_alpha` is the ONE exception, and it is the zoned reverse-fit's
        // sky/land pair ([`crate::recipe::MaskRole::is_zone`]). Their alpha is
        // a cache of nothing: the fit rendered it, claimed it under a unique
        // name and measured its dials against it, and no re-segmentation
        // reproduces it from the recipe. So it is TURNED with the `Bitmap`
        // rasters instead of dropped (`LocalAdjustment::
        // turnable_raster_paths_mut`, `pipeline::rotate_recipe` phase 1) —
        // dropping it would have left both zone corrections inert until a model
        // run, and inert forever on a machine with no segmentation sidecar.
        //
        // The `gesture` strokes are `BrushStroke`s under a different parent and
        // ride the SAME rewrite (R29 C1). The renderer does not composite them;
        // subtype 0 sends their `d` points to the segmenter. They are also
        // written back, so leaving them in the old frame would hand Lightroom a
        // refinement stroke beside a moved reference point.
        MaskGeometry::AiMask { ref_x, ref_y, raster, gesture, .. } => {
            (*ref_x, *ref_y) = orient_point(o, *ref_x, *ref_y);
            if !owned_alpha {
                *raster = None;
            }
            if let Some(f) = frame {
                turn_brush_strokes(gesture, o, f);
            }
        }
    };
    for m in r.masks.iter_mut() {
        // A component of a zone mask belongs to that zone: the role is the
        // ADJUSTMENT's, so it answers for every geometry the adjustment holds.
        let owned_alpha = m.role.is_zone();
        turn(&mut m.mask, owned_alpha);
        for c in m.components.iter_mut() {
            turn(&mut c.geometry, owned_alpha);
        }
        // The colour Range Mask's `(px, py)` is Lightroom's sample MARKER —
        // cosmetic, but it is a point in the original frame like any other,
        // and leaving it behind would put the marker on the wrong subject.
        if let Some(RangeMask::Color { px, py, .. }) = m.range.as_mut() {
            (*px, *py) = orient_point(o, *px, *py);
        }
    }
    // v1.5.0 F9. Lightroom states a retouch area in the UN-ROTATED SENSOR
    // frame, exactly like a crop rectangle and a brush dab, and this is not a
    // corner case: 7 of the 25 retouched photographs in the reference library
    // are `tiff:Orientation="8"`. Left un-turned, every removed object on a
    // portrait capture would be repaired a quarter turn away from where it is
    // — healing a clean patch of sky and leaving the power line untouched.
    //
    // Centres and the donor point are points in the picture plane and turn
    // through `orient_point` like every other coordinate above. The ellipse's
    // half-extents are in WIDTH units on BOTH axes (the convention
    // `BrushDab::r` records for `crs:Radius`, and the one the library's own
    // ellipses measure to: 161.5 px / 9504 = 0.016994 against a stored
    // 0.016938), so a quarter turn rescales them by exactly the factor a
    // stroke's radius takes — `brush_radius_scale` — because "width" names a
    // different edge afterwards.
    //
    // No `lr_dab_round` here, unlike the stroke arm: that rounding exists so a
    // dab returns to the sidecar on the decimal grid Lightroom wrote it on,
    // and this engine never writes `crs:RetouchAreas` back (the merge carries
    // the photographer's own block through verbatim). Rounding a number nobody
    // re-emits would only lose precision.
    for a in r.retouch.iter_mut() {
        if let Some(d) = a.donor.as_mut() {
            let (x, y) = orient_point(o, d[0], d[1]);
            *d = [x, y];
        }
        match &mut a.shape {
            crate::retouch::RetouchShape::Ellipse { cx, cy, size_x, size_y } => {
                (*cx, *cy) = orient_point(o, *cx, *cy);
                if let Some(f) = frame {
                    let scale = f.brush_radius_scale(o);
                    *size_x *= scale;
                    *size_y *= scale;
                }
            }
            // The same strokes the mask side carries, so the same rewrite —
            // one dab grammar and one turn, not two.
            crate::retouch::RetouchShape::Brush(strokes) => {
                if let Some(f) = frame {
                    turn_brush_strokes(strokes, o, f);
                }
            }
        }
    }
    // R33 §G. The colour field's spatial axes are the FRAME's, so a turn
    // permutes its cells exactly as it permutes every other coordinate here —
    // and a quarter turn swaps the two axis LENGTHS with them, because a 12x8
    // grid over a landscape frame is an 8x12 grid over the portrait one.
    //
    // The five parameters ride unchanged: EV, three channel gains and a slope
    // against the pixel's own guide luma are all photometric, none of them a
    // direction in the picture plane. The luma axis rides unchanged too.
    //
    // Cell CENTRES are what turn, not cell corners: the permutation has to be
    // a bijection on cells, and a corner lands on a boundary where the floor
    // below could send two cells to one slot and leave another empty.
    if let Some(field) = r.colour_field.as_mut().filter(|f| f.renderable()) {
        let swaps = crate::decode::orientation_transposes(o);
        let (nx, ny) = if swaps { (field.y, field.x) } else { (field.x, field.y) };
        let mut turned = vec![[0.0f32; 5]; nx * ny * field.b];
        for j in 0..field.y {
            for i in 0..field.x {
                let (u, v) = (
                    (i as f32 + 0.5) / field.x as f32,
                    (j as f32 + 0.5) / field.y as f32,
                );
                let (tu, tv) = orient_point(o, u, v);
                let ti = ((tu * nx as f32).floor().max(0.0) as usize).min(nx - 1);
                let tj = ((tv * ny as f32).floor().max(0.0) as usize).min(ny - 1);
                for b in 0..field.b {
                    turned[(tj * nx + ti) * field.b + b] =
                        field.grid[(j * field.x + i) * field.b + b];
                }
            }
        }
        (field.x, field.y, field.grid) = (nx, ny, turned);
    }
    true
}

/// Translate a recipe's stored GEOMETRY by `(du, dv)` in the display frame —
/// the deterministic half of the `coord_era` 1 → 2 migration
/// (`pipeline::migrate_recipe_coord_frame` owns the gating and the vector,
/// which is the develop window's move of 2026-09-21 turned by
/// [`orient_vector`]). Returns `false` for the zero vector, so the caller can
/// tell "nothing to do" from "moved".
///
/// Everything [`orient_recipe_coords`] turns, translated: the crop rectangle,
/// every mask geometry (base + components), the Range-Mask colour sample
/// point, every brush dab and gesture dab (token by token, through
/// [`shift_brush_strokes`]), retouch centres and donor points, and the colour
/// field — resampled at its cell centres, because a translation of a third of
/// a percent is a fraction of a cell and no permutation of cells. Lengths and
/// angles ride unchanged: a radius, an ellipse's half-extents, the straighten,
/// a feather. Raster masks carry no coordinates and are re-written as files by
/// the caller.
///
/// **The crop is clamped to the frame** after the move. The strip a crop at
/// the frame's edge would now reach past it is the sensor margin the old
/// window held and the declared crop does not; it is not in this frame to
/// keep. Gradient and radial handles are NOT clamped — they legitimately live
/// off-frame ([`orient_point`]), and clamping would shorten a falloff.
pub fn shift_recipe_coords(r: &mut EditRecipe, du: f32, dv: f32) -> bool {
    if du == 0.0 && dv == 0.0 {
        return false;
    }
    if let Some(c) = r.crop.as_mut() {
        *c = Crop {
            left: (c.left + du).clamp(0.0, 1.0),
            right: (c.right + du).clamp(0.0, 1.0),
            top: (c.top + dv).clamp(0.0, 1.0),
            bottom: (c.bottom + dv).clamp(0.0, 1.0),
        };
    }
    let shift = |g: &mut MaskGeometry| match g {
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y }
        | MaskGeometry::Shaped { zero_x, zero_y, full_x, full_y, .. } => {
            *zero_x += du;
            *zero_y += dv;
            *full_x += du;
            *full_y += dv;
        }
        MaskGeometry::Radial { top, left, bottom, right, .. } => {
            *left += du;
            *right += du;
            *top += dv;
            *bottom += dv;
        }
        // A file, re-written by the caller (`pipeline::migrate_raster_file`).
        MaskGeometry::Bitmap { .. } => {}
        MaskGeometry::Brush { strokes, .. } => shift_brush_strokes(strokes, du, dv),
        // The cached alpha is KEPT, unlike under a turn: the caller re-writes
        // the file translated, and a translated alpha is the alpha of the
        // translated picture.
        MaskGeometry::AiMask { ref_x, ref_y, gesture, .. } => {
            *ref_x += du;
            *ref_y += dv;
            shift_brush_strokes(gesture, du, dv);
        }
    };
    for m in r.masks.iter_mut() {
        shift(&mut m.mask);
        for c in m.components.iter_mut() {
            shift(&mut c.geometry);
        }
        if let Some(RangeMask::Color { px, py, .. }) = m.range.as_mut() {
            *px += du;
            *py += dv;
        }
    }
    for a in r.retouch.iter_mut() {
        if let Some(d) = a.donor.as_mut() {
            d[0] += du;
            d[1] += dv;
        }
        match &mut a.shape {
            crate::retouch::RetouchShape::Ellipse { cx, cy, .. } => {
                *cx += du;
                *cy += dv;
            }
            crate::retouch::RetouchShape::Brush(strokes) => shift_brush_strokes(strokes, du, dv),
        }
    }
    // The colour field's spatial axes are the frame's (R33 §G). Each cell of
    // the moved field reads the OLD field where its centre came from —
    // bilinear between the four nearest old cell centres, clamped to the
    // field's edge — so a shift of a fraction of a cell blends neighbours by
    // that fraction instead of snapping to the nearest cell or moving none.
    if let Some(field) = r.colour_field.as_mut().filter(|f| f.renderable()) {
        let (nx, ny, nb) = (field.x, field.y, field.b);
        let mut moved = vec![[0.0f32; 5]; nx * ny * nb];
        for j in 0..ny {
            for i in 0..nx {
                let fx = (i as f32 - du * nx as f32).clamp(0.0, (nx - 1) as f32);
                let fy = (j as f32 - dv * ny as f32).clamp(0.0, (ny - 1) as f32);
                let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
                let (x1, y1) = ((x0 + 1).min(nx - 1), (y0 + 1).min(ny - 1));
                let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                for b in 0..nb {
                    let at = |x: usize, y: usize| field.grid[(y * nx + x) * nb + b];
                    let (v00, v10, v01, v11) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
                    let out = &mut moved[(j * nx + i) * nb + b];
                    for p in 0..5 {
                        out[p] = (v00[p] * (1.0 - tx) + v10[p] * tx) * (1.0 - ty)
                            + (v01[p] * (1.0 - tx) + v11[p] * tx) * ty;
                    }
                }
            }
        }
        field.grid = moved;
    }
    true
}

/// A greyscale raster translated by `(dx, dy)` pixels: `out(x, y) =
/// in(x − dx, y − dy)`, bilinear, with samples past the edge clamped to it —
/// the file half of the era-2 migration (`pipeline::migrate_raster_file`
/// says why the edge continues rather than going to zero). An integer move
/// is an exact copy.
pub fn shift_luma_raster(img: &image::GrayImage, dx: f32, dy: f32) -> image::GrayImage {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return img.clone();
    }
    let sample = |x: f32, y: f32| -> f32 {
        let x = x.clamp(0.0, (w - 1) as f32);
        let y = y.clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let at = |px: u32, py: u32| img.get_pixel(px, py).0[0] as f32;
        (at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx) * (1.0 - ty)
            + (at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx) * ty
    };
    image::GrayImage::from_fn(w, h, |x, y| {
        image::Luma([sample(x as f32 - dx, y as f32 - dy).round().clamp(0.0, 255.0) as u8])
    })
}

/// Does this recipe hold any geometry the `coord_era` migration would move?
/// Used for the disclosure: a recipe with nothing but global sliders is
/// re-stamped in silence, because nothing about it changed.
///
/// A mask counts for its GEOMETRY only. Its point curves (R25 P6) are
/// frame-independent tone values and do not qualify — a mask carrying nothing
/// but a curve still counts here through its geometry, so the distinction is
/// invisible in the answer and easy to misread as an omission. It is not one;
/// see [`orient_recipe_coords`].
///
/// Retouch areas (v1.5.0 F9) and a renderable colour field (R33 §G) count
/// since v1.6.0: both move under a turn and under a translation, and a recipe
/// carrying nothing else used to be migrated in silence.
///
/// `straighten_deg` is deliberately NOT counted (R27 L-16c), even though the
/// migration now reverses it under a mirror. It moves for FOUR of the eight
/// states and this predicate cannot see which one is coming, so counting it
/// would raise the note on every rotated photo that has a tilt and nothing
/// else — an alarm that is wrong three times in four. The states that do move
/// it (`HorizontalFlip`/`VerticalFlip`/`Transpose`/`Transverse`) are ones no
/// camera writes; a recipe that also holds a crop or a mask — i.e. any recipe
/// where the tilt has something to be wrong about — is disclosed by that.
pub fn recipe_has_frame_coords(r: &EditRecipe) -> bool {
    r.crop.is_some()
        || !r.retouch.is_empty()
        || r.colour_field.as_ref().is_some_and(|f| f.renderable())
        || r.masks.iter().any(|m| {
            // Bitmap is the ONE geometry that does not move (its pixels are a
            // file). Brush left this exclusion in R29 C1: its dab stream is
            // rewritten numerically now, so counting it here is the true
            // statement, not the flattering one.
            let turnable = |g: &MaskGeometry| !matches!(g, MaskGeometry::Bitmap { .. });
            turnable(&m.mask)
                || m.components.iter().any(|c| turnable(&c.geometry))
                || matches!(m.range, Some(RangeMask::Color { .. }))
        })
}

/// Does this recipe carry a brush stroke anywhere — a [`MaskGeometry::Brush`]
/// group or an [`MaskGeometry::AiMask`]'s `crs:Gesture` refinement?
///
/// The predicate exists for ONE caller: `pipeline::rotate_recipe` needs the
/// photo's frame shape ([`CoordFrame`]) only for these, and reading it costs a
/// metadata walk of the RAW (a `RawSource` slurp, 60–120 MB for a 61 MP ARW).
/// Asking this first is what keeps a rotate of an ordinary develop as cheap as
/// it was — and what makes `orient_recipe_coords`' `None` arm unreachable with
/// a brush in hand instead of merely unlikely.
///
/// A group with an EMPTY stroke list counts as nothing: there is no coordinate
/// to move, so no frame is needed to move it.
pub fn recipe_has_brush_strokes(r: &EditRecipe) -> bool {
    let brushed = |g: &MaskGeometry| match g {
        MaskGeometry::Brush { strokes, .. } => !strokes.is_empty(),
        MaskGeometry::AiMask { gesture, .. } => !gesture.is_empty(),
        _ => false,
    };
    r.masks
        .iter()
        .any(|m| brushed(&m.mask) || m.components.iter().any(|c| brushed(&c.geometry)))
}
