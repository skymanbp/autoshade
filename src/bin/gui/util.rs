//! Pure helpers: geometry maps, curve edits, previews, thumb cache.

use super::*;

/// Build the option list for a model ComboBox: the live-fetched ids if we have
/// them, else a grounded fallback; the current value is always included so a
/// custom/manual id is never dropped from the menu.
pub(crate) fn model_opts(fetched: &[String], fallback: &[&str], current: &str) -> Vec<String> {
    let mut v: Vec<String> = if fetched.is_empty() {
        fallback.iter().map(|s| s.to_string()).collect()
    } else {
        fetched.to_vec()
    };
    if !current.trim().is_empty() && !v.iter().any(|x| x == current) {
        v.insert(0, current.to_string());
    }
    v
}

/// Every source type the app opens — ONE list shared by the file dialog, drag &
/// drop, and any future association, so they can't drift apart. Must cover
/// everything `decode::is_raw` accepts (the gallery + decoder already open
/// ORF/RW2/RAW; the dialog refusing them was pure drift).
pub(crate) const PHOTO_EXTS: [&str; 14] = [
    "arw", "dng", "raf", "nef", "cr2", "cr3", "orf", "rw2", "raw", "png", "tif", "tiff", "jpg",
    "jpeg",
];

pub(crate) fn is_photo_path(p: &std::path::Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| PHOTO_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

pub(crate) fn photo_file_dialog() -> Option<PathBuf> {
    rfd::FileDialog::new().add_filter("Photos", &PHOTO_EXTS).pick_file()
}

/// Visualise a mask geometry on the image: linear = the zero→full vector with
/// end bars (solid = full-effect side); radial = the ellipse outline. Clipped
/// to the image rect by the painter.
/// `adj_inverted` is the owning LocalAdjustment's Invert flag: the engine
/// flips the coverage with it, so the directional markers must flip too —
/// otherwise the outline points at exactly the wrong side while the red
/// coverage layer contradicts it in the same frame.
pub(crate) fn draw_mask_overlay(
    ui: &egui::Ui,
    xf: ViewXform,
    geom: &MaskGeometry,
    adj_inverted: bool,
    lang: Lang,
) {
    let p = ui.painter_at(xf.rect);
    let stroke = egui::Stroke::new(2.0, ACCENT);
    match geom {
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => {
            let a = xf.to_screen(*zero_x, *zero_y);
            let b = xf.to_screen(*full_x, *full_y);
            // Invert swaps which end carries the full effect.
            let (full, zero) = if adj_inverted { (a, b) } else { (b, a) };
            p.line_segment([a, b], stroke);
            let v = b - a;
            let len = v.length().max(1.0);
            let n = egui::vec2(-v.y / len, v.x / len) * 28.0;
            p.line_segment([zero - n, zero + n], egui::Stroke::new(1.0, ACCENT));
            p.line_segment([full - n, full + n], stroke);
            p.circle_filled(full, 4.0, ACCENT); // full-effect end
            p.circle_stroke(zero, 4.0, stroke); // untouched end
        }
        MaskGeometry::Radial { top, left, bottom, right, flipped, angle, .. } => {
            let (cx, cy) = ((left + right) / 2.0, (top + bottom) / 2.0);
            let c = xf.to_screen(cx, cy);
            if *angle == 0.0 {
                let rx = (xf.to_screen(*right, 0.0).x - xf.to_screen(*left, 0.0).x).abs() / 2.0;
                let ry = (xf.to_screen(0.0, *bottom).y - xf.to_screen(0.0, *top).y).abs() / 2.0;
                p.add(egui::Shape::ellipse_stroke(c, egui::vec2(rx, ry), stroke));
            } else {
                // Rotated ellipse: sample parametrically in NORMALISED frame
                // coords — the same space the engine rotates in (recipe.rs
                // `MaskGeometry::Radial`), so the outline is the true weight
                // boundary on any frame aspect.
                let (rx, ry) = ((right - left) / 2.0, (bottom - top) / 2.0);
                let (s, co) = angle.to_radians().sin_cos();
                let pts: Vec<egui::Pos2> = (0..48)
                    .map(|k| {
                        let th = k as f32 / 48.0 * std::f32::consts::TAU;
                        let (ex, ey) = (rx * th.cos(), ry * th.sin());
                        xf.to_screen(cx + ex * co - ey * s, cy + ex * s + ey * co)
                    })
                    .collect();
                p.add(egui::Shape::closed_line(pts, stroke));
            }
            // Filled centre = the effect fills the INSIDE; hollow = the
            // geometry's own `flipped` and/or the adjustment's Invert put the
            // effect OUTSIDE the ellipse (the two compose, matching the engine).
            if flipped ^ adj_inverted {
                p.circle_stroke(c, 3.0, stroke);
            } else {
                p.circle_filled(c, 3.0, ACCENT);
            }
        }
        // Raster masks have no parametric outline to draw — mark the selection
        // with a badge instead of pretending a shape (rendering the raster as a
        // live translucent overlay is the A② follow-up).
        MaskGeometry::Bitmap { .. } => {
            p.text(
                xf.rect.left_top() + egui::vec2(10.0, 10.0),
                egui::Align2::LEFT_TOP,
                tr(lang, "▨ Bitmap mask"),
                egui::FontId::proportional(14.0),
                ACCENT,
            );
        }
    }
}

/// Screen-space knob positions for on-image mask editing (geometry given in
/// VIEW space, i.e. already through geom_to_view). Handle ids: 0 = move
/// (linear midpoint / radial centre); linear 1 = zero end, 2 = full end;
/// radial 1..4 = left/top/right/bottom AXIS endpoints (rotated with the
/// mask's own angle) and 5 = the rotation grip, parked outside the top axis
/// endpoint so it never shadows it. Bitmap masks carry no parametric knobs
/// (empty).
pub(crate) fn mask_handle_points(geom: &MaskGeometry, xf: ViewXform) -> Vec<(u8, egui::Pos2)> {
    match *geom {
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => {
            let a = xf.to_screen(zero_x, zero_y);
            let b = xf.to_screen(full_x, full_y);
            vec![(0, a + (b - a) * 0.5), (1, a), (2, b)]
        }
        MaskGeometry::Radial { top, left, bottom, right, angle, .. } => {
            let (cx, cy) = ((left + right) / 2.0, (top + bottom) / 2.0);
            // Rotate about the centre in NORMALISED coords — the engine's own
            // rotation space, so knobs sit on the true weight boundary.
            let (s, c) = angle.to_radians().sin_cos();
            let rot = |x: f32, y: f32| {
                let (dx, dy) = (x - cx, y - cy);
                (cx + dx * c - dy * s, cy + dx * s + dy * c)
            };
            let l = rot(left, cy);
            let t = rot(cx, top);
            let r = rot(right, cy);
            let b = rot(cx, bottom);
            let grip = rot(cx, top - (bottom - top).abs() * 0.18);
            vec![
                (0, xf.to_screen(cx, cy)),
                (1, xf.to_screen(l.0, l.1)),
                (2, xf.to_screen(t.0, t.1)),
                (3, xf.to_screen(r.0, r.1)),
                (4, xf.to_screen(b.0, b.1)),
                (5, xf.to_screen(grip.0, grip.1)),
            ]
        }
        MaskGeometry::Bitmap { .. } => Vec::new(),
    }
}

/// Scale `tex_size` to fit a `max_w` × `avail_y` box (both dimensions — width
/// alone lets a portrait overflow the panel), never upscaling past 4×.
pub(crate) fn fit_in(tex_size: egui::Vec2, max_w: f32, avail_y: f32) -> egui::Vec2 {
    let s = (max_w / tex_size.x.max(1.0))
        .min(avail_y.max(1.0) / tex_size.y.max(1.0))
        .clamp(0.01, 4.0);
    tex_size * s
}

/// Section header text with an activity dot when the section holds non-neutral
/// values — a collapsed active adjustment must never be invisible.
pub(crate) fn section_title(base: &str, active: bool) -> String {
    if active {
        format!("{base}  ●")
    } else {
        base.to_string()
    }
}

/// The recipe field behind a curve-editor channel index.
pub(crate) fn curve_points(recipe: &EditRecipe, ch: usize) -> &Vec<CurvePoint> {
    match ch {
        0 => &recipe.tone_curve,
        1 => &recipe.red_curve,
        2 => &recipe.green_curve,
        _ => &recipe.blue_curve,
    }
}

pub(crate) fn curve_points_mut(recipe: &mut EditRecipe, ch: usize) -> &mut Vec<CurvePoint> {
    match ch {
        0 => &mut recipe.tone_curve,
        1 => &mut recipe.red_curve,
        2 => &mut recipe.green_curve,
        _ => &mut recipe.blue_curve,
    }
}

/// Insert a curve control point keeping inputs sorted and UNIQUE — a second
/// point at the same input overwrites instead of duplicating (the engine's
/// piecewise-linear interp needs distinct inputs). Returns the point's index.
pub(crate) fn insert_curve_point(pts: &mut Vec<CurvePoint>, input: u8, output: u8) -> usize {
    match pts.binary_search_by_key(&input, |p| p.input) {
        Ok(i) => {
            pts[i].output = output;
            i
        }
        Err(i) => {
            pts.insert(i, CurvePoint { input, output });
            i
        }
    }
}

/// Move point `i` to (input, output), clamping input STRICTLY between its
/// neighbours so the control points always stay sorted with unique inputs.
pub(crate) fn drag_curve_point(pts: &mut [CurvePoint], i: usize, input: u8, output: u8) {
    let lo = if i > 0 { pts[i - 1].input.saturating_add(1) } else { 0 };
    let hi = if i + 1 < pts.len() { pts[i + 1].input.saturating_sub(1) } else { 255 };
    // lo > hi would need two neighbours ≤ 2 apart around an existing point —
    // unreachable via insert/drag above; keep the current input if it happens.
    if lo <= hi {
        pts[i].input = input.clamp(lo, hi);
    }
    pts[i].output = output;
}

/// Drag-reorder bookkeeping: element `from` moves to sit before `insert`
/// (both indices in the PRE-move order; `insert == len` appends). Returns the
/// element's final index plus the remap for every OTHER stored index (e.g.
/// the selection), composed as remove-at-`from` then insert-at-`to`.
/// `insert == from` and `insert == from + 1` are the two no-op drop slots —
/// callers skip the move entirely for those.
pub(crate) fn reorder_move(from: usize, insert: usize) -> (usize, impl Fn(usize) -> usize) {
    let to = if insert > from { insert - 1 } else { insert };
    (to, move |s: usize| {
        if s == from {
            to
        } else {
            let after_rm = if s > from { s - 1 } else { s };
            if after_rm >= to { after_rm + 1 } else { after_rm }
        }
    })
}

// --- geometric coordinate mapping (straighten + distortion) ------------------
// When straighten_deg ≠ 0 or lens_distortion ≠ 0 the After view shows the
// geometrically transformed frame (original → distortion-corrected →
// rotated + auto-cropped, see render.rs's C2 contract), but recipe masks, the
// paint canvas, fill/heal masks and base_preview pixels all live in the
// ORIGINAL frame (the engine applies masks before it remaps; fill/heal edit
// source pixels). These two maps convert between the spaces at the data
// boundaries, sharing the engine's own inscribed_dims / distort_norm formulas
// and rotation convention (clockwise-positive, y-down) so GUI and render can
// never disagree. recipe.crop stays in the view space — the export applies
// the user crop AFTER the geometric chain, so the crop tool needs no mapping.
// All maps are the identity when both controls are zero.

/// View normalized point → original-frame normalized point. Thin wrapper over
/// the engine's shared C2 interaction map (`render::view_to_original_norm`) —
/// the web server routes analyze region boxes through the SAME function, so
/// the two surfaces cannot drift.
pub(crate) fn view_norm_to_orig(nx: f32, ny: f32, dims: (f32, f32), deg: f32, dist: &LensArg) -> (f32, f32) {
    autoshop::render::view_to_original_norm(nx, ny, dims, deg, &dist.profile, dist.amount)
}

/// Original-frame normalized point → view normalized point (engine-shared;
/// NOT clamped — the painter clips overlays to the image rect anyway).
pub(crate) fn orig_norm_to_view(nx: f32, ny: f32, dims: (f32, f32), deg: f32, dist: &LensArg) -> (f32, f32) {
    autoshop::render::original_to_view_norm(nx, ny, dims, deg, &dist.profile, dist.amount)
}

/// A mask geometry mapped from the ORIGINAL frame into the view for on-screen
/// display (identity when straighten and distortion are both zero). Linear /
/// Radial anchor points map exactly; the radial ellipse is shown as the
/// bounding box of its transformed corners — display-only, and tight at the
/// small tilt angles and gentle distortions the sliders allow.
pub(crate) fn geom_to_view(geom: &MaskGeometry, dims: (f32, f32), deg: f32, dist: &LensArg) -> MaskGeometry {
    // Off = the composed map moves nothing, INCLUDING the CA composite
    // fill (L04-2/Codex AL F1): a CA-only overshoot zooms the frame, so
    // an identity early-out here desynced mask anchors from the pixels.
    let geom_off = !autoshop::render::geometry_moves_frame(&dist.profile, dist.amount);
    if deg == 0.0 && geom_off {
        return geom.clone();
    }
    match *geom {
        // Raster masks carry no parametric anchor points to remap; their
        // overlay is a screen-anchored badge (see draw_mask_overlay).
        MaskGeometry::Bitmap { .. } => geom.clone(),
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => {
            let a = orig_norm_to_view(zero_x, zero_y, dims, deg, dist);
            let b = orig_norm_to_view(full_x, full_y, dims, deg, dist);
            MaskGeometry::Linear { zero_x: a.0, zero_y: a.1, full_x: b.0, full_y: b.1 }
        }
        MaskGeometry::Radial { top, left, bottom, right, feather, roundness, flipped, angle } => {
            let pts = [
                orig_norm_to_view(left, top, dims, deg, dist),
                orig_norm_to_view(right, top, dims, deg, dist),
                orig_norm_to_view(left, bottom, dims, deg, dist),
                orig_norm_to_view(right, bottom, dims, deg, dist),
            ];
            let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for (x, y) in pts {
                l = l.min(x);
                t = t.min(y);
                r = r.max(x);
                b = b.max(y);
            }
            // The bbox is the straighten/distortion approximation (see the
            // placement comment); the mask's OWN rotation rides through
            // unchanged — the outline sampler and handles re-apply it.
            MaskGeometry::Radial {
                top: t,
                left: l,
                bottom: b,
                right: r,
                feather,
                roundness,
                flipped,
                angle,
            }
        }
    }
}

/// Two API base URLs are the "same endpoint" ignoring whitespace / a trailing slash.
/// Used so model ids fetched with the image key are only offered to the analysis
/// picker when both roles point at the same server.
pub(crate) fn same_base(a: &str, b: &str) -> bool {
    a.trim().trim_end_matches('/') == b.trim().trim_end_matches('/')
}

/// A model picker: a dropdown of `options` that writes the chosen id into `value`,
/// next to a text field so any custom id can still be typed. Both edit `value`.
pub(crate) fn model_picker(ui: &mut egui::Ui, salt: &str, value: &mut String, options: &[String], lang: Lang) {
    let sel = if value.trim().is_empty() { tr(lang, "Select…").to_owned() } else { value.clone() };
    egui::ComboBox::from_id_salt(salt)
        .selected_text(sel)
        .width(200.0)
        .show_ui(ui, |ui| {
            for opt in options {
                ui.selectable_value(value, opt.clone(), opt.as_str());
            }
        });
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(170.0)
            .hint_text(tr(lang, "or type a custom id")),
    );
}

/// A reasoning-effort picker: the tiers this provider documents, plus an
/// explicit "provider default" that clears the value, plus a text field for a
/// tier some endpoint adds later.
///
/// Empty is a REAL choice, not a missing one — it means "send no effort
/// parameter at all", which is the only correct request for a model that does
/// not reason. So the dropdown names it rather than leaving the field blank
/// and ambiguous.
pub(crate) fn effort_picker(
    ui: &mut egui::Ui,
    salt: &str,
    value: &mut String,
    tiers: &[&str],
    lang: Lang,
) {
    let sel = if value.trim().is_empty() {
        tr(lang, "provider default").to_owned()
    } else {
        value.clone()
    };
    egui::ComboBox::from_id_salt(salt)
        .selected_text(sel)
        .width(200.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(value, String::new(), tr(lang, "provider default"));
            for t in tiers {
                ui.selectable_value(value, (*t).to_string(), *t);
            }
        });
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(170.0)
            .hint_text(tr(lang, "or type a tier")),
    )
    .on_hover_text(tr(
        lang,
        "How hard the model is asked to think. Higher tiers cost more and take longer; blank leaves the choice to the provider. An endpoint that does not know the tier is retried without it.",
    ));
}

/// A [`StashEntry`]'s strip as a persistable record — the quit dialog's
/// Save-all writes one per stashed photo so background variants survive the
/// quit (before v0.22 they were "unsavable" and pinned the dialog forever).
pub(crate) fn stash_strip_record(st: &StashEntry) -> Option<autoshop::store::VariantsRecord> {
    if st.others.is_empty() && st.kind == VariantKind::Original {
        return None;
    }
    Some(autoshop::store::VariantsRecord {
        v: 1,
        active_kind: st.kind.store_str().to_string(),
        active_pos: st.active_pos,
        others: st
            .others
            .iter()
            .map(|sv| autoshop::store::VariantEntry {
                kind: sv.kind.store_str().to_string(),
                recipe: sv.recipe.clone(),
                origin: sv.origin.clone(),
            })
            .collect(),
    })
}

/// First FREE ./out artifact path for `tag` — the SHARED atomic claim
/// (`pipeline::unique_out`, also used by the web fill/heal handlers so the
/// two surfaces can never hand out colliding master names).
pub(crate) fn unique_out(path: &std::path::Path, tag: &str) -> Option<PathBuf> {
    autoshop::pipeline::unique_out(path, tag)
}

/// Mean of the 5×5 neighbourhood around a normalised point, as 0..1 sRGB;
/// `None` when the window falls entirely outside the image.
///
/// Both pickers — the WB eyedropper and the colour-range sample — read the
/// 8-bit texture the user is AIMING at, which is the deliberate choice (U14):
/// the aim surface IS the display. Per-pixel reads on 25 samples also beat
/// the full-frame `to_rgb8()` copy the first versions allocated per click.
pub(crate) fn sample_5x5_mean(img: &image::DynamicImage, nx: f32, ny: f32) -> Option<[f32; 3]> {
    use image::GenericImageView as _;
    let (w, h) = img.dimensions();
    let (cx, cy) = (
        (nx * (w.saturating_sub(1)) as f32).round() as i64,
        (ny * (h.saturating_sub(1)) as f32).round() as i64,
    );
    let (mut acc, mut n) = ([0.0f32; 3], 0.0f32);
    for dy in -2..=2i64 {
        for dx in -2..=2i64 {
            let (x, y) = (cx + dx, cy + dy);
            if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
                let p = img.get_pixel(x as u32, y as u32);
                for c in 0..3 {
                    acc[c] += p[c] as f32 / 255.0;
                }
                n += 1.0;
            }
        }
    }
    (n > 0.0).then(|| [acc[0] / n, acc[1] / n, acc[2] / n])
}

/// Give back an atomically claimed output name when nothing real landed.
///
/// The claim is a 0-byte placeholder, so an empty file means the run failed
/// before writing anything: without this, failed runs consumed the 999-name
/// cap and littered ./out with empty files. A NON-empty partial is left in
/// place for diagnosis. Every retouch worker calls this on both failure paths
/// — the normal error tail and the panic handler, where the tail never ran.
pub(crate) fn release_empty_claim(p: &std::path::Path) {
    if std::fs::metadata(p).is_ok_and(|m| m.len() == 0) {
        let _ = std::fs::remove_file(p);
    }
}

/// Do two spellings name the same baked master?
///
/// An in-session retouch records its ./out artifact RELATIVE
/// (`pipeline::default_out` -> `out/<stem>.heal.png`), while
/// `store::write_pixel_source` stores the same file ABSOLUTIZED and
/// `read_pixel_source` hands the absolute form back. Plain `PathBuf`
/// equality therefore compared `out/x.png` against `<cwd>/out/x.png` and
/// missed: a save-then-switch left 1:1 inspection stuck at the old
/// resolution — the outcome the re-decode branch exists to prevent — and
/// now would draw a refusal telling the user to save what they just saved.
/// Lexical only: `std::path::absolute` normalizes without touching the
/// filesystem, and both sides go through the same normalization the writer
/// itself used.
pub(crate) fn same_master(a: &std::path::Path, b: &std::path::Path) -> bool {
    a == b
        || matches!(
            (std::path::absolute(a), std::path::absolute(b)),
            (Ok(x), Ok(y)) if x == y
        )
}

/// Same master identity? `None` means "no baked master", which matches only
/// itself; the path halves go through [`same_master`], so the two spellings
/// the store and the GUI produce for ONE file cannot read as a difference.
///
/// EVERY master comparison goes through here — the five saved-pixel guards
/// (navigation stash gate, ● marker, strip dirty count, quit dialog, close
/// guard) and the history step's pixel identity. Fixing only the site that
/// happened to be under review left the same bug at those five, where it was
/// worse: a SAVED retouch was re-reported as unsaved by all of them,
/// permanently, because nothing rewrites the in-memory origin at save time.
pub(crate) fn same_master_opt(a: Option<&std::path::Path>, b: Option<&std::path::Path>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => same_master(x, y),
        _ => false,
    }
}

/// Per-call temp-file counter: a cancelled worker and its replacement run
/// CONCURRENTLY in one process, so pid-only names collide.
pub(crate) static GUI_TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn gui_tmp_png(kind: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "autoshop_gui_{kind}_{}_{}.png",
        std::process::id(),
        GUI_TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

/// First three entries joined by " · ", an ellipsis when more were cut — the
/// bounded form every multi-failure status/toast shares (a 40-photo batch
/// must not pour 40 error lines into one toast).
pub(crate) fn brief_list(v: &[String]) -> String {
    let mut d = v.iter().take(3).cloned().collect::<Vec<_>>().join(" · ");
    if v.len() > 3 {
        d.push_str(" …");
    }
    d
}

/// 256-bin RGB+luma histogram of the already-packed preview (one bin per
/// 8-bit code value, Lightroom-grade resolution). One worker-side RGB8 buffer
/// feeds histogram, clipping and texture staging alike.
///
/// All four channels share ONE vertical scale (the global max), the way
/// Lightroom draws it — per-channel normalisation made relative bar heights
/// meaningless (a channel holding 200 pixels drew as tall as one holding two
/// million, so a colour cast read as balanced).
pub(crate) fn compute_histogram_rgb(rgb: &image::RgbImage) -> Vec<[f32; 4]> {
    const BINS: usize = 256;
    let mut counts = vec![[0u32; 4]; BINS];
    for px in rgb.pixels() {
        let (r, g, b) = (px[0] as usize, px[1] as usize, px[2] as usize);
        let luma = (0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32) as usize;
        counts[r][0] += 1;
        counts[g][1] += 1;
        counts[b][2] += 1;
        counts[luma.min(BINS - 1)][3] += 1;
    }
    let mut max = 1u32;
    for bins in &counts {
        for &v in bins {
            max = max.max(v);
        }
    }
    counts
        .iter()
        .map(|bins| std::array::from_fn(|ch| bins[ch] as f32 / max as f32))
        .collect()
}

/// RGB8 → egui texture-ready colour image, without an intermediate RGBA image.
pub(crate) fn rgb_to_color_image(rgb: &image::RgbImage) -> egui::ColorImage {
    egui::ColorImage::from_rgb([rgb.width() as usize, rgb.height() as usize], rgb.as_raw())
}

/// `DynamicImage` compatibility helper for cold paths (open / variant switch).
pub(crate) fn to_color_image(img: &image::DynamicImage) -> egui::ColorImage {
    rgb_to_color_image(&img.to_rgb8())
}

/// Clipping-warning layer over the DEVELOPED RGB8 preview (what export clips).
pub(crate) fn clipping_overlay_rgb(rgb: &image::RgbImage) -> egui::ColorImage {
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut rgba = vec![0u8; w * h * 4];
    for (i, p) in rgb.pixels().enumerate() {
        let px = &mut rgba[i * 4..i * 4 + 4];
        if p[0] >= 254 || p[1] >= 254 || p[2] >= 254 {
            px.copy_from_slice(&[255, 40, 40, 255]);
        } else if p[0] <= 1 && p[1] <= 1 && p[2] <= 1 {
            px.copy_from_slice(&[70, 110, 255, 255]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba)
}

/// The clipping layer with the crop's blanking applied — outside the crop
/// nothing exports, so warnings there are noise. Shared by the J fast path
/// and the mid-flight-toggle fallback; the worker build (build_preview)
/// applies the same blanking with its own crop_px (it feeds the histogram
/// too). The fast paths used to skip the blanking and contradict the
/// worker's layer within one session (L18).
pub(crate) fn clipping_overlay_for(
    rgb: &image::RgbImage,
    crop: Option<autoshop::recipe::Crop>,
) -> egui::ColorImage {
    let mut overlay = clipping_overlay_rgb(rgb);
    let Some(c) = crop else { return overlay };
    let (w, h) = (rgb.width() as f32, rgb.height() as f32);
    let x0 = (c.left.clamp(0.0, 1.0) * w) as u32;
    let y0 = (c.top.clamp(0.0, 1.0) * h) as u32;
    let x1 = ((c.right.clamp(0.0, 1.0) * w) as u32).max(x0 + 1).min(rgb.width());
    let y1 = ((c.bottom.clamp(0.0, 1.0) * h) as u32).max(y0 + 1).min(rgb.height());
    let transparent = egui::Color32::TRANSPARENT;
    for (i, px) in overlay.pixels.iter_mut().enumerate() {
        let (cx, cy) = (i as u32 % overlay.size[0] as u32, i as u32 / overlay.size[0] as u32);
        if cx < x0 || cx >= x1 || cy < y0 || cy >= y1 {
            *px = transparent;
        }
    }
    overlay
}

/// Pure CPU preview build, safe to run off the egui thread. It deliberately
/// excludes texture-manager calls: `TextureHandle::set` stays on the UI thread.
pub(crate) fn build_preview(
    base: Arc<image::DynamicImage>,
    recipe: EditRecipe,
    show_clipping: bool,
) -> PreviewDone {
    let mut after = autoshop::render::develop_preview(&base, &recipe);
    if recipe.lens_profile.geometry_active() || recipe.lens_distortion != 0.0 {
        after = autoshop::render::apply_lens_geometry(
            &after,
            &recipe.lens_profile,
            recipe.lens_distortion,
        );
    }
    if recipe.straighten_deg != 0.0 {
        after = autoshop::render::rotate_straighten(&after, recipe.straighten_deg);
    }
    // into_rgb8 MOVES the buffer in the common no-geometry case (develop_preview
    // returns ImageRgb8) — to_rgb8() deep-copied ~3.3 MB per tick at 1280.
    let rgb = after.into_rgb8();
    // Histogram + clipping share the export's CROP: `recipe.crop` is
    // normalised on this same post-distortion/straighten frame (render.rs —
    // distortion → straighten → crop), so the statistics cover exactly the
    // region that ships. The pixel VALUES, however, are the 8-bit preview at
    // working resolution, not the 16-bit full-res export — downscaling can
    // average a small blown specular below the clip threshold (recorded
    // deferral; export-exact statistics need the full-res pipeline). The
    // preview image itself stays full-frame on purpose (immediate whole-frame
    // slider feedback).
    let crop_px = recipe.crop.map(|c| {
        let (w, h) = (rgb.width() as f32, rgb.height() as f32);
        let x0 = (c.left.clamp(0.0, 1.0) * w) as u32;
        let y0 = (c.top.clamp(0.0, 1.0) * h) as u32;
        let x1 = ((c.right.clamp(0.0, 1.0) * w) as u32).max(x0 + 1).min(rgb.width());
        let y1 = ((c.bottom.clamp(0.0, 1.0) * h) as u32).max(y0 + 1).min(rgb.height());
        (x0, y0, x1 - x0, y1 - y0)
    });
    let histogram = match crop_px {
        Some((x, y, w, h)) => {
            compute_histogram_rgb(&image::imageops::crop_imm(&rgb, x, y, w, h).to_image())
        }
        None => compute_histogram_rgb(&rgb),
    };
    let clipping = show_clipping.then(|| {
        let mut overlay = clipping_overlay_rgb(&rgb);
        // Outside the crop nothing exports — blank those warnings so a blown
        // sky the user already cropped away stops shouting.
        if let Some((x, y, w, h)) = crop_px {
            let transparent = egui::Color32::TRANSPARENT;
            for (i, px) in overlay.pixels.iter_mut().enumerate() {
                let (cx, cy) = (i as u32 % overlay.size[0] as u32, i as u32 / overlay.size[0] as u32);
                if cx < x || cx >= x + w || cy < y || cy >= y + h {
                    *px = transparent;
                }
            }
        }
        overlay
    });
    let thumb_rgb = image::imageops::thumbnail(&rgb, 96, 96);
    let thumb = rgb_to_color_image(&thumb_rgb);
    let after = rgb_to_color_image(&rgb);
    PreviewDone { base, recipe, rgb, after, histogram, clipping, thumb }
}

/// Stamp a filled brush dot into the paint mask (painted = translucent red).
/// One brush dot writing an arbitrary pixel — the shared kernel behind the
/// classic red paint stamp and the mask-brush ERASE stamp (fully clear).
pub(crate) fn stamp_dot_px(m: &mut image::RgbaImage, c: (f32, f32), r: f32, px: image::Rgba<u8>) {
    let (w, h) = (m.width() as i32, m.height() as i32);
    let (cx, cy) = c;
    let r2 = r * r;
    let x0 = (cx - r).floor().max(0.0) as i32;
    let x1 = ((cx + r).ceil() as i32).min(w - 1);
    let y0 = (cy - r).floor().max(0.0) as i32;
    let y1 = ((cy + r).ceil() as i32).min(h - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            if dx * dx + dy * dy <= r2 {
                m.put_pixel(x as u32, y as u32, px);
            }
        }
    }
}

/// The mask-brush gray-buffer twin of [`stamp_dot_px`]: same disc, writing
/// the WEIGHT (255 = selected, 0 = erased) into the greyscale source of
/// truth the Apply bakes to a raster.
pub(crate) fn stamp_dot_gray(g: &mut image::GrayImage, c: (f32, f32), r: f32, v: u8) {
    let (w, h) = (g.width() as i32, g.height() as i32);
    let (cx, cy) = c;
    let r2 = r * r;
    let x0 = (cx - r).floor().max(0.0) as i32;
    let x1 = ((cx + r).ceil() as i32).min(w - 1);
    let y0 = (cy - r).floor().max(0.0) as i32;
    let y1 = ((cy + r).ceil() as i32).min(h - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            if dx * dx + dy * dy <= r2 {
                g.put_pixel(x as u32, y as u32, image::Luma([v]));
            }
        }
    }
}

/// Stamp a brush stroke between two points (interpolated dots — no gaps).
pub(crate) fn stamp_line_px(
    m: &mut image::RgbaImage,
    a: (f32, f32),
    b: (f32, f32),
    r: f32,
    px: image::Rgba<u8>,
) {
    let dist = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    let steps = (dist / (r * 0.5).max(1.0)).ceil().max(1.0) as i32;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        stamp_dot_px(m, (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t), r, px);
    }
}

pub(crate) fn stamp_line_gray(g: &mut image::GrayImage, a: (f32, f32), b: (f32, f32), r: f32, v: u8) {
    let dist = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    let steps = (dist / (r * 0.5).max(1.0)).ceil().max(1.0) as i32;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        stamp_dot_gray(g, (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t), r, v);
    }
}

/// One process-wide permit serialising >24 MP baked-raster thumbnail decodes
/// (see request_thumb). RAW thumbs never take it.
pub(crate) fn big_decode_gate() -> &'static std::sync::Mutex<()> {
    static GATE: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    GATE.get_or_init(Default::default)
}

/// Where the persistent 160px thumbnail for `src` lives, or `None` when the
/// source can't be stat'ed (no stable key → no caching). Rooted at the same
/// per-user store as develops/settings (`store_root()` honours the
/// AUTOSHOP_DATA_DIR override — a portable setup must not split its data
/// across two roots) and keyed by absolute path + mtime + size, so an
/// edited/replaced source misses. DefaultHasher is fine HERE (unlike the
/// develop keys): a hash that drifts across Rust releases only costs a
/// one-time re-decode.
pub(crate) fn thumb_cache_file(src: &std::path::Path) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let meta = std::fs::metadata(src).ok()?;
    let dir = autoshop::store::store_root().join("thumbs");
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // Decoder-generation salt: bump when decode semantics change what a
    // thumb LOOKS like (v2 = EXIF orientation applied to baked images) — an
    // unchanged file otherwise keeps serving its stale sideways thumbnail.
    2u32.hash(&mut h);
    std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf()).hash(&mut h);
    meta.modified().ok().hash(&mut h);
    meta.len().hash(&mut h);
    Some(dir.join(format!("{:016x}.jpg", h.finish())))
}

/// Write-through for the thumbnail disk cache — best effort: a failed write
/// only costs a re-decode next session, so it warns rather than failing the
/// thumb. Once per session, an oversized cache dir (>10k files) is pruned by
/// age on a background thread.
pub(crate) fn save_thumb_cache(cache: &std::path::Path, thumb: &image::DynamicImage) {
    let Some(dir) = cache.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    if let Err(e) = thumb.to_rgb8().save_with_format(cache, image::ImageFormat::Jpeg) {
        eprintln!("⚠ thumb cache write failed ({e}) — will re-decode next session");
        return;
    }
    static PRUNED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    let dir = dir.to_path_buf();
    PRUNED.get_or_init(|| {
        std::thread::spawn(move || {
            let Ok(rd) = std::fs::read_dir(&dir) else { return };
            let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    let t = e.metadata().and_then(|m| m.modified()).ok()?;
                    Some((t, p))
                })
                .collect();
            if files.len() > 10_000 {
                files.sort_by_key(|(t, _)| *t);
                for (_, p) in files.iter().take(files.len() - 10_000) {
                    let _ = std::fs::remove_file(p);
                }
            }
        });
    });
}

/// mtime + size of a file, or None when it cannot be stat'ed.
pub(crate) fn file_stamp(path: &std::path::Path) -> FileStamp {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok().map(|t| (t, m.len())))
}

/// The develop's current baked-pixel identity (see [`BaseCacheKey`]). The
/// master's own stamp is included so RE-BAKING to the same path still misses.
pub(crate) fn pixel_identity(path: &std::path::Path) -> PixelIdentity {
    let (master, generated) = autoshop::store::read_pixel_source(path)?;
    let stamp = file_stamp(&master);
    Some((master, generated, stamp))
}

/// The pointer's press origin via a Response (`handle_paint` has no `ui`).
pub(crate) fn ui_press_origin(resp: &egui::Response) -> Option<egui::Pos2> {
    resp.ctx.input(|i| i.pointer.press_origin())
}

/// A claimed raster name's FAMILY: "mask-sky-3.png" → "mask-sky" (strip the
/// extension, then a trailing "-<digits>" claim counter). Lets a re-run's
/// fresh unique name still match — and repoint — the mask entry its previous
/// run created.
pub(crate) fn mask_family(bare: &str) -> &str {
    let stem = bare.strip_suffix(".png").unwrap_or(bare);
    // A version-frozen raster ("v3.mask-sky") is the same family — a recipe
    // loaded from a version snapshot references frozen names, and a rerun
    // must still repoint that mask instead of appending a duplicate.
    let stem = match stem.split_once('.') {
        Some((v, rest))
            if v.len() > 1
                && v.starts_with('v')
                && v[1..].bytes().all(|b| b.is_ascii_digit()) =>
        {
            rest
        }
        _ => stem,
    };
    match stem.rsplit_once('-') {
        Some((head, tail)) if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => stem,
    }
}

/// One frame of the zoom glide (阶段5 手感): exponential approach with a
/// ~45 ms time constant — visually ~120 ms to settle, frame-rate
/// independent (dt-driven, clamped so a hitch cannot overshoot a frame's
/// worth of travel). Snaps once within 1e-3 so the animation terminates
/// exactly. Pure, so the convergence contract is testable headlessly.
pub(crate) fn glide_step(cur: f32, target: f32, dt: f32) -> f32 {
    let k = 1.0 - (-dt.clamp(0.0, 0.05) / 0.045).exp();
    let next = cur + (target - cur) * k;
    if (target - next).abs() < 1e-3 { target } else { next }
}
