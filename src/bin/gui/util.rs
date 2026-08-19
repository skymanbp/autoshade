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
/// drop, and any future association, so they can't drift apart.
///
/// R27 A2: this used to be a HAND-TYPED array of 14, and the drift it was
/// meant to prevent had already happened elsewhere (the web `accept` list was
/// missing three formats the gallery opened fine). It is now derived from
/// `decode::RAW_EXTS ∪ pipeline::BAKED_EXTS` — the same two lists the engine,
/// the scanners and the web server ask — so the dialog literally cannot offer
/// a different set from the one that opens.
pub(crate) fn photo_exts() -> Vec<&'static str> {
    autoshop::pipeline::photo_exts()
}

pub(crate) fn is_photo_path(p: &std::path::Path) -> bool {
    autoshop::pipeline::is_source(p)
}

pub(crate) fn photo_file_dialog() -> Option<PathBuf> {
    rfd::FileDialog::new().add_filter("Photos", &photo_exts()).pick_file()
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
    fit_in_capped(tex_size, max_w, avail_y, 4.0)
}

/// [`fit_in`] with the upscale cap as a parameter. The 4× cap exists so a
/// tiny image does not balloon at FIT — but past fit it fought the zoom
/// itself: the visible window shrinks as zoom grows, the cap re-fit it
/// smaller, and deep zoom SHRANK the canvas while cursor anchoring drifted
/// (L13-2). A zoomed canvas passes `f32::INFINITY` and fills the pane box.
pub(crate) fn fit_in_capped(tex_size: egui::Vec2, max_w: f32, avail_y: f32, cap: f32) -> egui::Vec2 {
    let s = (max_w / tex_size.x.max(1.0))
        .min(avail_y.max(1.0) / tex_size.y.max(1.0))
        .clamp(0.01, cap);
    tex_size * s
}

/// A GROUP caption: the weak small line that names one band of sections
/// (#14b). The develop panel's five groups — tone & colour, detail & lens,
/// local & pixel, versions & export, plus the AI panel above it — already had
/// the two `separator` fences that mark three of those boundaries and nothing
/// that said what the fence divided; per-mask sliders got captions in R22-5
/// (the `group` closure in `dev_masks`) and this is the same device one level
/// up. Caption only — the sections keep their own collapsing state, so
/// nothing here can hide a control.
pub(crate) fn group_caption(ui: &mut egui::Ui, title: &str) {
    ui.label(egui::RichText::new(title).weak().small());
}

/// ONE cross-reference sentence appended to a pixel-level AI verb's own
/// tooltip (#4). The AI *develop* verbs (Analyze / Refine / Reimagine /
/// reverse-fit) collected into the AI panel; the pixel-level ones deliberately
/// did NOT move — 「AI select subject」 belongs beside the mask list, denoise
/// beside sharpening, heal in Retouch — so each says where the rest of the AI
/// lives instead of leaving the panel looking like the whole of it.
///
/// Appended to the EXISTING tooltip text rather than folded into every one of
/// those (long, translated) strings: one new catalogue entry instead of five
/// rewritten ones, and one tooltip per widget — a second `on_hover_text` on
/// the same response stacks two bubbles (see `slider_hinted`).
pub(crate) fn ai_xref(lang: Lang, tip: &str) -> String {
    format!("{tip}\n\n{}", tr(lang, "More AI features are in the AI area at the top of this panel"))
}

/// Fold-state override for the two collapsibles that hide a WIDTH-PINNED row
/// (the Reimagine prompt+button row and the Generative Fill prompt): `None` in
/// a real run — the user's own fold state — and `Some(true)` under `cfg(test)`,
/// because a headless frame never clicks a header and an unopened fold makes a
/// width assertion silently vacuous instead of red. The width tests also call
/// egui's own `set_everything_is_visible`; this covers the panels a test drives
/// without it.
pub(crate) fn fold_open_in_tests() -> Option<bool> {
    cfg!(test).then_some(true)
}

/// A prompt text field with a readable width ceiling (#14a).
///
/// Prompts are sentences, so their fields want to be wide — but only up to a
/// point: `desired_width(f32::INFINITY)` (what the Direction and Fill fields
/// used) tracks `available_width()`, which on a panel dragged out to 800 px
/// draws a single-line ribbon the full width of the panel. This clamps
/// DOWNWARD to [`FIELD_W_MAX`] — never asking for more space than the row has,
/// so it cannot reproduce the R19 runaway (that row asked for a SURPLUS every
/// frame and the auto-fitting side panel granted it).
///
/// Two escapes for a prompt longer than the box: while the field has keyboard
/// focus it widens to the full available width (typing needs to see the tail),
/// and hovering shows the whole text — a singleline `TextEdit` clips (it
/// scrolls, it does not wrap), so the tail is otherwise unreadable.
///
/// The id is PREDICTED (`next_auto_id`) and handed to the widget explicitly, so
/// this frame's width can honour last frame's focus without a second widget or
/// a caller-supplied salt.
pub(crate) fn prompt_field(ui: &mut egui::Ui, buf: &mut String, hint: &str) -> egui::Response {
    let id = ui.next_auto_id();
    let focused = ui.memory(|m| m.has_focus(id));
    let avail = ui.available_width();
    let w = if focused { avail } else { avail.min(FIELD_W_MAX) };
    let mut resp = ui.add(
        egui::TextEdit::singleline(buf)
            .id(id)
            .desired_width(w.max(FIELD_W_MIN))
            .hint_text(hint),
    );
    // Allocate the tooltip string only while it is actually being shown (this
    // runs every frame for every prompt field). `buf`'s borrow ended with the
    // `add` above.
    if resp.hovered() && !buf.trim().is_empty() {
        resp = resp.on_hover_text(buf.clone());
    }
    resp
}

/// Section header text with an activity dot when the section holds non-neutral
/// values — a collapsed active adjustment must never be invisible.
///
/// The `active` predicates themselves live beside the sections they describe
/// (`panels/develop.rs`, `panels/ai.rs`) so each one can be read against the
/// field set it covers; two of them are named methods
/// ([`AutoshopApp::ai_section_active`], [`AutoshopApp::masks_section_active`])
/// because a header and something else must agree on the answer.
pub(crate) fn section_title(base: &str, active: bool) -> String {
    if active {
        format!("{base}  ●")
    } else {
        base.to_string()
    }
}

/// "This mask is doing something": the ENGINE's own non-neutrality rule plus the
/// eye (a muted mask renders nothing whatever its sliders say).
///
/// ONE owner for two readers (R22 #16). The mask ROW has shown this dot since
/// L06, but the Local Masks header showed its own `n_masks > 0` — which claimed
/// activity for a list of parked or muted masks, and only ever repeated the
/// count the header already prints. Two hand-written predicates for one question
/// is the drift; this is the deduplication.
pub(crate) fn mask_active(m: &autoshop::recipe::LocalAdjustment) -> bool {
    m.enabled && autoshop::render::engine_active(m)
}

/// The angles a rotation disclosure can NAME, as one comma list — distinct
/// (two masks tilted the same way say it once), in the order the verdicts were
/// raised, and capped with the same `+N more` tail every other disclosure list
/// uses. Shared by both directions: the writer drops OUR angle, the reader
/// drops LIGHTROOM's, and the sentence is built the same way from either.
///
/// `None` when there is nothing to name. `0` is the payload's word for "no
/// angle we could measure" — a `crs:Angle` that is present but unreadable, or
/// a tilt that rounds away — and 「Rotation 0°」 would be a sentence that is
/// wrong about the file. The caller falls back to its plain phrasing.
fn rotation_degrees(lang: Lang, degs: impl Iterator<Item = i32>) -> Option<String> {
    let mut seen: Vec<i32> = Vec::new();
    for d in degs {
        if d != 0 && !seen.contains(&d) {
            seen.push(d);
        }
    }
    if seen.is_empty() {
        return None;
    }
    let shown = seen.len().min(4);
    let mut list = seen[..shown].iter().map(|d| d.to_string()).collect::<Vec<_>>().join(", ");
    let more = seen.len() - shown;
    if more > 0 {
        list.push_str(&format!(", {}", trf(lang, "+{n} more", &[("n", &more.to_string())])));
    }
    Some(list)
}

/// The export-side lossy-projection disclosure (M6a, widened by R24-5 M0), in
/// the UI language: ONE line naming what the Lightroom sidecar just written
/// does NOT carry. Two buckets, one sentence:
///
///   * the MASK bucket, per category — and it NAMES the masks now, not just
///     counts them. "bitmap masks ×3" left the actionable half unsaid ("which
///     of my twelve?"); the CLI's own `xmp::describe_mask_losses` has named
///     them since M6a, and the window was the surface that did not.
///   * the GLOBAL bucket, which did not exist at all: an active control the
///     engine renders and the sidecar has no property for
///     (`xmp::global_export_losses`, derived from the tier registry's
///     `RenderedNotExported` rows).
///
/// Both verdicts come from the WRITER / the registry, never from a list kept
/// here, so the message can never describe a file that was not written or a
/// tier that has moved.
///
/// `None` for a faithful projection: a save that lost nothing must not
/// interrupt (the four import-side disclosures follow the same rule).
///
/// WHETHER the line interrupts is [`xmp_loss_interrupts`]' decision, not this
/// function's: the sentence is the same either way.
pub(crate) fn xmp_loss_line(
    lang: Lang,
    losses: &[autoshop::xmp::MaskLoss],
    globals: &[&'static str],
) -> Option<String> {
    use autoshop::xmp::MaskLossReason as R;
    if losses.is_empty() && globals.is_empty() {
        return None;
    }
    /// The masks in one category, named. Capped: a 64-mask recipe would make
    /// an unreadable line, so the count stays the fact and the first few names
    /// are the pointer (the same 4/「+N more」 shape `describe_mask_losses`
    /// uses for the CLI).
    fn named(lang: Lang, losses: &[autoshop::xmp::MaskLoss], reason: R) -> String {
        let names: Vec<&str> =
            losses.iter().filter(|l| l.reason.same_kind(reason)).map(|l| l.name.as_str()).collect();
        let shown = names.len().min(4);
        let more = names.len() - shown;
        let list = names[..shown]
            .iter()
            // A mask may legitimately have no name; an empty slot in a
            // comma list reads as a rendering bug.
            .map(|n| if n.trim().is_empty() { tr(lang, "(unnamed)").to_string() } else { n.to_string() })
            .collect::<Vec<_>>()
            .join(", ");
        match more {
            0 => list,
            _ => format!("{list}, {}", trf(lang, "+{n} more", &[("n", &more.to_string())])),
        }
    }
    let mut parts: Vec<String> = Vec::new();
    // The ORDER (skips before degradations) and the membership are the
    // writer's own `MaskLossReason::ALL`, not a second copy of it — a reason
    // raised by the writer and missing here would be counted by nobody. The
    // match below stays exhaustive so each arm keeps a literal key the i18n
    // audit can see.
    for reason in R::ALL {
        // By KIND, not by `==`: `Rotation` carries the dropped angle, so two
        // masks tilted differently are two values of one reason (and `ALL`'s
        // placeholder payload equals neither).
        let n = losses.iter().filter(|l| l.reason.same_kind(reason)).count();
        if n == 0 {
            continue;
        }
        let n = n.to_string();
        let head = match reason {
            R::Bitmap => trf(lang, "bitmap masks ×{n}", &[("n", &n)]),
            R::Disabled => trf(lang, "muted masks ×{n}", &[("n", &n)]),
            R::ComponentsFlattened => trf(lang, "shape components flattened ×{n}", &[("n", &n)]),
            // R25 P5: the rotation loss SAYS THE ANGLE and says why. 「radial
            // rotation ×1」 told a photographer that something about rotation
            // was dropped, and left both actionable halves out — how much, and
            // whether it was a bug. The degrees come from the writer's own
            // verdict; when there is no angle to name (an unreadable one, or a
            // tilt under half a degree) the plain count is still the truth.
            R::Rotation(_) => match rotation_degrees(
                lang,
                losses.iter().filter_map(|l| match l.reason {
                    R::Rotation(d) => Some(d),
                    _ => None,
                }),
            ) {
                Some(a) => trf(
                    lang,
                    "Rotation {a}° not written to XMP (frame size unknown)",
                    &[("a", &a)],
                ),
                None => trf(lang, "radial rotation ×{n}", &[("n", &n)]),
            },
            R::Recolour => trf(lang, "recolour gains ×{n}", &[("n", &n)]),
        };
        parts.push(format!("{head} ({})", named(lang, losses, reason)));
    }
    // The global bucket. A control with no label of its own falls back to its
    // registry name rather than vanishing — an unlabelled disclosure still
    // says WHICH control, which is the whole point.
    for g in globals {
        parts.push(match *g {
            "base_curve" => tr(lang, "camera base curve").to_string(),
            "lens_profile" => tr(lang, "lens profile correction").to_string(),
            other => other.to_string(),
        });
    }
    Some(trf(
        lang,
        "the Lightroom XMP does not carry: {list} (recipe.json keeps all of it)",
        &[("list", &parts.join(" · "))],
    ))
}

/// Does the loss line [`xmp_loss_line`] just produced deserve to INTERRUPT the
/// save — an Error toast plus a ⚠ on the status line — or is it a note that
/// belongs in the quiet channel beside it (R24 round-end MED-2)?
///
/// The rule the disclosure broke: "a save that lost nothing must not
/// interrupt". `base_curve` is stamped onto the canvas by every RAW OPEN
/// (`persist::stamp_calibration`), so `RenderedNotExported` is active on
/// essentially every RAW — and every single Ctrl+S therefore raised an error
/// toast about a loss that is real, universal and impossible to act on. Alarm
/// on every save is alarm the user learns to ignore, and the mask half (which
/// they CAN act on) went with it.
///
/// The judgement comes from a bit the registry already carries, not from a new
/// tier or a list kept here: `engine_only` means the value is the ENGINE's own
/// per-photo measurement (the camera base curve, the in-camera lens profile) —
/// nothing the user chose, so nothing they can undo. When every global loss is
/// one of those, the sentence still gets said, quietly. One member that is a
/// user's own choice (the LR-gap batches B2–B5 will add them) puts the toast
/// back for the whole line.
///
/// MASK losses always interrupt: every one of them is a mask the user made.
pub(crate) fn xmp_loss_interrupts(
    losses: &[autoshop::xmp::MaskLoss],
    globals: &[&'static str],
) -> bool {
    !losses.is_empty()
        || globals.iter().any(|g| {
            // A name with no registry row is not something this can vouch for
            // as engine calibration — treat the unknown as the user's, which
            // is the interrupting side.
            autoshop::advisor::catalogue::global_control(g).is_none_or(|c| !c.engine_only)
        })
}

/// The OTHER corner of the disclosure square (R25 B4): the settings this
/// photo carries that LIGHTROOM renders and this canvas does not.
///
/// [`xmp_loss_line`] says "the sidecar cannot carry what you are looking at".
/// This says the reverse — "you are not looking at everything the sidecar
/// carries" — and until R25 nothing did. B2 and B3 made that urgent: twenty-
/// four `Tier::CarriedOnly` controls now round-trip through the sidecar
/// without moving one pixel here (policy SF4-C), and B4 added the pass-through
/// blocks. Each slider says so in its own tooltip; this is the document-level
/// sentence, said at the moment the file is handed to Lightroom.
///
/// NAMED BY SECTION, not by field. `post_crop_vignette_hl` is an internal
/// symbol and R24 pinned those out of user prose; the develop panel's own
/// section headings are the words the photographer already has. The mapping
/// goes through the registry's FAMILY table, so a control that joins a family
/// joins the sentence — and a family nobody labelled here falls back to the
/// registry name (loud, and `the_render_gap_line_appears_only_when_something_
/// is_carried` fails on the underscore) rather than vanishing.
///
/// `None` when nothing is carried: this never interrupts and never toasts —
/// nothing was lost, the sidecar is complete, and it is the canvas that is
/// missing something. The quiet channel only.
///
/// The B4 pass-through blocks do not reach here (`xmp::global_render_gaps`
/// states why): with no interpretation there is no neutral, so they would
/// name themselves on every Lightroom photo. They disclose through the
/// develop panel's own read-only Transform / Calibration section instead.
pub(crate) fn render_gap_line(lang: Lang, gaps: &[&'static str]) -> Option<String> {
    use autoshop::advisor::catalogue::CONTROL_FAMILIES;
    if gaps.is_empty() {
        return None;
    }
    let mut sections: Vec<String> = Vec::new();
    for g in gaps {
        let label = match CONTROL_FAMILIES.iter().find(|f| f.members.contains(g)).map(|f| f.name) {
            Some("effects") => tr(lang, "Effects").to_string(),
            Some("detail_effects") => tr(lang, "Detail").to_string(),
            Some("lens_effects") => tr(lang, "Lens").to_string(),
            _ => (*g).to_string(),
        };
        if !sections.contains(&label) {
            sections.push(label);
        }
    }
    Some(format!(
        "{}: {}",
        tr(lang, "Carried to Lightroom, not rendered here"),
        sections.join(", ")
    ))
}

/// The IMPORT-side twin of [`xmp_loss_line`] (R25 P1): one line saying how
/// many Lightroom masks arrived and, NAMED, what did not come with them.
///
/// The line it replaces counted: "N Lightroom mask(s) (brush/AI/depth) have no
/// engine equivalent and were not imported". That sentence was true of one
/// import defect out of six and silent about the other five — and the five it
/// was silent about refused every ordinary Lightroom mask, so the count it
/// printed was usually the user's ENTIRE catalog with no way to tell why.
///
/// Every verdict comes from the READER (`xmp::MaskImportReason`), iterated in
/// its own `ALL` order, so this can never describe a rule the import does not
/// actually follow. Reasons that share a label share a bullet — three
/// unmodelled sliders are one phrase, not three.
///
/// `None` when the import was faithful: a restore that lost nothing must not
/// interrupt, exactly as on the export side.
pub(crate) fn xmp_import_line(
    lang: Lang,
    imported: usize,
    losses: &[autoshop::xmp::MaskImportLoss],
) -> Option<String> {
    use autoshop::xmp::MaskImportReason as R;
    if losses.is_empty() {
        return None;
    }
    // ONE label per bullet, in `ALL` order, names appended to whichever bullet
    // already exists — `InertLocal`, `UnknownLocalKey` and
    // `CurveRefineSaturation` are all "a knob we do not model" to a reader.
    let mut parts: Vec<(String, Vec<String>)> = Vec::new();
    for reason in R::ALL {
        let names: Vec<String> = losses
            .iter()
            .filter(|l| l.reason.same_kind(reason))
            .map(|l| {
                // A correction may legitimately have no name; an empty slot in
                // a comma list reads as a rendering bug.
                if l.name.trim().is_empty() {
                    tr(lang, "(unnamed)").to_string()
                } else {
                    l.name.clone()
                }
            })
            .collect();
        if names.is_empty() {
            continue;
        }
        // The match stays exhaustive so each arm keeps a literal key the i18n
        // audit can see at its own `tr` call site, and a new variant stops the
        // build here too. Grouping is on the TRANSLATED label, which is a
        // faithful key either way: two reasons sharing an English phrase share
        // its Chinese one.
        let label: String = match reason {
            R::Unrepresentable => tr(
                lang,
                "AI / brush masks cannot be imported — Lightroom recomputes them from a digest",
            )
            .into(),
            R::OutOfModel => tr(lang, "Beyond this engine's model").into(),
            // R25 P5, the import twin of the export line's rotation head: say
            // the angle Lightroom wrote and why it did not survive, instead of
            // a bare category. Falls back to the plain label when there is no
            // angle to name (see `rotation_degrees`).
            R::Rotation(_) => match rotation_degrees(
                lang,
                losses.iter().filter_map(|l| match l.reason {
                    R::Rotation(d) => Some(d),
                    _ => None,
                }),
            ) {
                Some(a) => trf(
                    lang,
                    "Rotation {a}° read as 0 (frame size unknown)",
                    &[("a", &a)],
                ),
                None => tr(lang, "Rotation angle").into(),
            },
            R::BlendMode => tr(lang, "Blend mode").into(),
            R::MultiComponent => tr(lang, "Extra shapes").into(),
            R::ForeignRangeMask => tr(lang, "Range mask (foreign)").into(),
            // R25 P6 narrowed this verdict from "we do not model local point
            // curves" to "this one could not be READ" — the label moved with
            // it, because the old phrase now describes a feature that works.
            R::LocalCurve => tr(lang, "Local point curve (unreadable)").into(),
            R::CurveRefineSaturation | R::InertLocal(_) | R::UnknownLocalKey => {
                tr(lang, "Unmodelled slider").into()
            }
        };
        match parts.iter_mut().find(|(l, _)| *l == label) {
            Some((_, have)) => have.extend(names),
            None => parts.push((label, names)),
        }
    }
    let rendered: Vec<String> = parts
        .into_iter()
        .map(|(label, names)| {
            // Capped like every other disclosure list: the count is the fact,
            // the first few names are the pointer.
            let shown = names.len().min(4);
            let more = names.len() - shown;
            let mut list = names[..shown].join(", ");
            if more > 0 {
                let more = more.to_string();
                list.push_str(&format!(", {}", trf(lang, "+{n} more", &[("n", &more)])));
            }
            format!("{label} ({list})")
        })
        .collect();
    Some(format!(
        "{}: {}",
        trf(
            lang,
            "Imported {n} Lightroom mask(s), {m} feature(s) not modelled",
            &[("n", &imported.to_string()), ("m", &rendered.len().to_string())],
        ),
        rendered.join(" · ")
    ))
}

/// The recipe field behind a curve-editor target + channel index.
///
/// `None` for a `Mask(i)` whose index no longer addresses a mask — deleting
/// the selected mask while its editor is laid out is one frame away, and
/// falling back to the GLOBAL curve would silently point the editor (and its
/// next click) at the wrong four vectors. The editor answers `None` by drawing
/// nothing, which is what an absent mask should look like.
pub(crate) fn curve_points(
    recipe: &EditRecipe,
    target: CurveTarget,
    ch: usize,
) -> Option<&Vec<CurvePoint>> {
    let four = match target {
        CurveTarget::Global => {
            [&recipe.tone_curve, &recipe.red_curve, &recipe.green_curve, &recipe.blue_curve]
        }
        CurveTarget::Mask(i) => {
            let m = recipe.masks.get(i)?;
            [&m.main_curve, &m.red_curve, &m.green_curve, &m.blue_curve]
        }
    };
    let [main, red, green, blue] = four;
    Some(match ch {
        0 => main,
        1 => red,
        2 => green,
        _ => blue,
    })
}

pub(crate) fn curve_points_mut(
    recipe: &mut EditRecipe,
    target: CurveTarget,
    ch: usize,
) -> Option<&mut Vec<CurvePoint>> {
    let four = match target {
        CurveTarget::Global => [
            &mut recipe.tone_curve,
            &mut recipe.red_curve,
            &mut recipe.green_curve,
            &mut recipe.blue_curve,
        ],
        CurveTarget::Mask(i) => {
            let m = recipe.masks.get_mut(i)?;
            [&mut m.main_curve, &mut m.red_curve, &mut m.green_curve, &mut m.blue_curve]
        }
    };
    let [main, red, green, blue] = four;
    Some(match ch {
        0 => main,
        1 => red,
        2 => green,
        _ => blue,
    })
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
        MaskGeometry::Radial {
            top, left, bottom, right, feather, roundness, flipped, angle, midpoint, mask_version,
        } => {
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
                // Carried through, not re-defaulted: this is a VIEW copy of
                // the user's own geometry, and a copy that quietly swaps a
                // field for its neutral is how a display path becomes a data
                // loss the day someone stores its result.
                midpoint,
                mask_version,
            }
        }
    }
}

/// Two API base URLs are the "same endpoint". Delegates to the library's one
/// definition — the pick-list invalidation here and `config`'s key-home gate
/// must never disagree about what "same" means.
pub(crate) fn same_base(a: &str, b: &str) -> bool {
    autoshop::config::same_endpoint(a, b)
}

/// The FORMAT dropdown owns the export container; a typed save-path only
/// picks the name (the Lightroom model, L14-1): render_to_file derives its
/// encoder from the extension, so "photo.png" typed under a JPEG-q60 export
/// silently produced a PNG whose quality slider was ignored. Spellings of
/// the SAME container (jpeg/jfif, tiff) are kept exactly as typed.
pub(crate) fn normalize_export_target(mut p: std::path::PathBuf, ext: &str) -> std::path::PathBuf {
    let typed = p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    let same = matches!(
        (ext, typed.as_deref()),
        ("jpg", Some("jpg" | "jpeg" | "jfif")) | ("tif", Some("tif" | "tiff")) | ("png", Some("png"))
    );
    if !same {
        p.set_extension(ext);
    }
    p
}

/// What the analysis model field should become when the provider radio flips
/// (`None` = keep the user's value). Rewrites only what is KNOWN to belong to
/// the other provider's vocabulary:
///
/// - API-ward, only CLI aliases are rewritten — a full `claude-*` id is
///   legitimate on an OpenAI-compatible bridge.
/// - OAuth-ward, anything that is neither an alias nor a `claude-*` id is
///   swapped for `opus` — but the CLI accepts full ids as well as aliases
///   (config.rs: "a claude alias/id"; claude.rs passes the value verbatim),
///   so "not an alias" must not be read as "not a Claude model": a valid
///   `claude-opus-4-6` used to be silently rewritten on every flip.
pub(crate) fn analysis_model_on_flip(current: &str, to_api: bool) -> Option<&'static str> {
    let m = current.trim();
    let claude_alias = CLAUDE_ALIASES.contains(&m);
    let claude_id = claude_alias || m.to_ascii_lowercase().starts_with("claude-");
    if to_api && claude_alias {
        Some("gpt-5.5")
    } else if !to_api && !claude_id {
        Some("opus")
    } else {
        None
    }
}

/// In-memory stamp for "which credential fetched this pick-list": a plain
/// `DefaultHasher`, session-local only — never persisted, never displayed,
/// and deliberately not a cryptographic commitment (a collision merely keeps
/// a stale pick-list until the next fetch).
pub(crate) fn key_fingerprint(key: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
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

/// Does a delivery root and the OPEN photo's folder sit inside one another —
/// in either direction (R24 round-end LOW-3)?
///
/// Why it matters: `pipeline::guard_readonly` refuses to write into the source
/// RAW's folder ("the photo library is read-only") — but it allows anything
/// under the delivery root FIRST. Pointing the root into the library therefore
/// SILENTLY retires that protection for the overlap: an ancestor root retires
/// it for the whole photo folder, a root nested inside the photo's folder for
/// that subtree. A planted root cannot do this (`AUTOSHOP_OUT_DIR` is
/// `Trust::Destination`, so neither a `.env` nor an ambient settings file may
/// supply it) — but the user choosing it in Settings had nothing on screen
/// saying what it costs.
///
/// LEXICAL and filesystem-free (it runs per frame in a Settings panel):
/// `std::path::absolute` + `pipeline::normalize_lexical`, no `canonicalize`.
/// So it is an ADVISORY warning — it will not see a junction or a case-flipped
/// alias, both of which the guard's own canonical comparison does. A warning
/// that misses an exotic spelling is worth having; a per-frame `canonicalize`
/// on a network path is not.
pub(crate) fn delivery_root_shadows_photo(
    root: &std::path::Path,
    photo: Option<&std::path::Path>,
) -> bool {
    let Some(dir) = photo.and_then(std::path::Path::parent) else { return false };
    let lexical = |p: &std::path::Path| {
        std::path::absolute(p).ok().map(|a| autoshop::pipeline::normalize_lexical(&a))
    };
    let (Some(root), Some(dir)) = (lexical(root), lexical(dir)) else { return false };
    root.starts_with(&dir) || dir.starts_with(&root)
}

/// A [`StashEntry`]'s strip as a persistable record — the quit dialog's
/// Save-all writes one per stashed photo so background variants survive the
/// quit (before v0.22 they were "unsavable" and pinned the dialog forever).
pub(crate) fn stash_strip_record(st: &StashEntry) -> Option<autoshop::store::VariantsRecord> {
    // The ONE triviality judgement (R24-3), shared with the live strip's
    // `current_strip_record`: a lone base negative needs no sidecar UNLESS
    // it carries a name or a minted identity, which nothing else stores.
    if crate::model::strip_is_trivial(st.kind, &st.id, st.name.as_deref(), st.others.len()) {
        return None;
    }
    Some(autoshop::store::VariantsRecord {
        extra: Default::default(),
        v: 1,
        active_kind: st.kind.store_str().to_string(),
        active_pos: st.active_pos,
        others: st
            .others
            .iter()
            .map(|sv| autoshop::store::VariantEntry {
                extra: Default::default(),
                kind: sv.kind.store_str().to_string(),
                recipe: sv.recipe.clone(),
                origin: sv.origin.clone(),
                // Hop 5 of 6 (R24-2): a stashed photo's strip straight to
                // disk — Save-all never rebuilds the live strip, so a name
                // dropped here would die in the quit dialog.
                id: variant_id_field(&sv.id),
                name: sv.name.clone(),
            })
            .collect(),
        active_id: variant_id_field(&st.id),
        active_name: st.name.clone(),
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

/// A path as the user can ACT on it: absolute, and readable (R22-7).
///
/// Every deliverable path the window names goes through here. `./out` is
/// relative to whatever directory the app happened to be launched from, so
/// "exported → out/DSC0001.developed.tif" named a folder the user then had to
/// guess at. Deliberately `std::path::absolute`, not `canonicalize`: the target
/// usually does not exist yet (it is about to be written), and canonicalize
/// hands back Windows' `\\?\` verbatim prefix — correct, and unreadable in a
/// status line. Falls back to the raw spelling if even the lexical form fails
/// (an empty path), because a message must never be worse than before.
pub(crate) fn abs_display(p: &std::path::Path) -> String {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()).display().to_string()
}

/// Show `dir` in the OS file manager (Explorer / Finder / xdg-open).
///
/// NOT a shell invocation: the path is one argv entry, so nothing inside it is
/// interpreted — and the only caller passes a path this app computed itself
/// (the develop store), never text from a file. No new dependency either; the
/// tree had no file-manager call before this one.
///
/// Success is judged by SPAWN, not by exit status: `explorer.exe` returns a
/// non-zero code even when it opens the window, so waiting on it would report a
/// failure that did not happen. Stated cost of not waiting: on Unix the
/// `xdg-open`/`open` child is not reaped until Autoshop exits (one defunct entry
/// per click of a button nobody clicks in a loop). Waiting instead would block
/// the UI thread on a helper whose runtime we do not control.
///
/// COMMA PATHS, fixed in R27 (L-25). `explorer.exe` does not use the C runtime
/// argv rules: it re-parses its own command line and treats a COMMA as an
/// argument separator (which is why its documented switches are spelled
/// `/select,<path>` and `/root,<path>` — the comma IS the delimiter). Rust's
/// `Command::arg` quotes only for spaces and quotes, so a develop dir carrying
/// the photo's stem — a photo called `a,b.arw` reaches it — arrived as two
/// arguments and opened the wrong window.
///
/// The fix is to hand explorer a command line it parses the way we mean:
/// [`explorer_quoted_arg`] wraps the path in double quotes and
/// `CommandExt::raw_arg` puts it on the line verbatim, since Rust's own
/// encoder would escape the quotes back out. Still NOT a shell invocation —
/// `CreateProcess` interprets no metacharacters — and the quoting cannot be
/// escaped from, because `"` is not a legal character in a Windows path;
/// [`explorer_quoted_arg`] returns `None` rather than assume it, and the
/// caller then falls back to the old plain argument.
pub(crate) fn reveal_folder(dir: &std::path::Path) -> std::io::Result<()> {
    #[cfg(windows)]
    let mut cmd = {
        // ABSOLUTE `explorer.exe`, never the bare name. Rust's own program
        // resolution on Windows (`resolve_exe`) mirrors CreateProcess: the
        // executable's directory, then THE CURRENT DIRECTORY, then PATH — so an
        // `explorer.exe` sitting in the folder Autoshop was launched from would
        // run instead of Windows'. That is the v0.18.0 threat model exactly
        // (unpack someone else's photo bundle, run the app there, and their
        // files must not execute). `SystemRoot` / `windir` are SYSTEM
        // environment variables, not user paths; with neither set we fall back
        // to the bare name — the old behaviour, and better than a button that
        // cannot work at all.
        match std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir")) {
            Some(root) => {
                std::process::Command::new(std::path::Path::new(&root).join("explorer.exe"))
            }
            None => std::process::Command::new("explorer.exe"),
        }
    };
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    #[cfg(windows)]
    match explorer_quoted_arg(dir) {
        Some(raw) => {
            use std::os::windows::process::CommandExt as _;
            cmd.raw_arg(raw);
        }
        // A path this function will not vouch for goes the old way: one
        // escaped argv entry. Worse for a comma, no worse than before.
        None => {
            cmd.arg(dir);
        }
    }
    #[cfg(not(windows))]
    cmd.arg(dir);
    cmd.spawn().map(|_| ())
}

/// The exact command-line fragment [`reveal_folder`] hands `explorer.exe` for
/// `dir` — the path in double quotes — or `None` when this function refuses to
/// vouch for the path.
///
/// Two refusals, both narrow and both deliberate:
///   * a path that is not valid Unicode (`to_str` fails). `raw_arg` takes the
///     bytes as written and a lone surrogate has no spelling on a command
///     line; the caller falls back to `Command::arg`, which knows how to pass
///     an `OsStr` through.
///   * a path containing `"`. It cannot happen — `"` is one of the nine
///     characters Windows forbids in a file name — but the whole point of the
///     quotes is that nothing inside them terminates them, and an assumption
///     that guards a raw command line is worth checking rather than asserting.
///
/// Note what is NOT done: no `/select,` and no `/root,`. Both would CHANGE the
/// gesture (select the folder inside its parent / re-root the window), and the
/// button's promise is "show me this folder".
#[cfg(any(windows, test))]
pub(crate) fn explorer_quoted_arg(dir: &std::path::Path) -> Option<String> {
    let s = dir.to_str()?;
    (!s.contains('"')).then(|| format!("\"{s}\""))
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
    {
        // The COMPOSED profile (R25 B3): the manual CA pair folds onto the
        // same per-channel radius knots, so reading the raw profile here
        // would skip it on any photo with no in-camera CA data of its own.
        // Scoped, because the Cow borrows `recipe` and `recipe` is MOVED into
        // the result below (a borrow held by a value with a destructor lives
        // to the end of its scope).
        let geom = autoshop::render::geometry_profile(&recipe);
        if geom.geometry_active() || recipe.lens_distortion != 0.0 {
            after = autoshop::render::apply_lens_geometry(&after, &geom, recipe.lens_distortion);
        }
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
/// (see request_thumb) and the full-resolution mask-refine guide. RAW thumbs
/// never take it.
pub(crate) fn big_decode_gate() -> &'static std::sync::Mutex<()> {
    static GATE: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    GATE.get_or_init(Default::default)
}

/// Is the Python segmentation sidecar actually present in THIS install?
///
/// The release packages carry no `python/` directory, so on a downloaded build
/// 「🤖 AI select subject」/「🤖 AI select sky」 could only ever fail — the button
/// looked available and spent the click on a "sidecar not found at …" toast.
/// Resolved ONCE: `config::bundled_helper` searches the executable's directory
/// and its ancestors (never the cwd), and the env override is read at launch,
/// so the answer cannot change under a running process.
pub(crate) fn segment_helper_available() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        let cfg = autoshop::config::Config::load();
        std::path::Path::new(&cfg.segment_script).exists()
    })
}

/// The same question for the Python DENOISE sidecar (R24 batch 2).
///
/// One helper family, one treatment: `python/denoise.py` ships exactly the way
/// `python/segment.py` does — which is to say a release package carries
/// neither — so 「🤖 AI Denoise now」 had the failure the segmentation buttons
/// were fixed out of a round ago. It spent the click, ran the worker, and
/// surfaced `denoise.rs`'s English "denoise sidecar not found at …" verbatim
/// in a status line the rest of the app renders in the user's language. A
/// capability probe answers before the click and in both languages, and it
/// costs one `exists()` per process for the same reason the segmentation one
/// does: `config::bundled_helper` searches the executable's own directory (not
/// the cwd) and the env override is read at launch.
pub(crate) fn denoise_helper_available() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        let cfg = autoshop::config::Config::load();
        std::path::Path::new(&cfg.denoise_script).exists()
    })
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
    // thumb LOOKS like (v2 = EXIF orientation applied to baked images;
    // v3 = the same for RAW, where the orientation FIRST TAKES EFFECT — every
    // v2 cache entry for a portrait ARW is a sideways thumbnail, because
    // rawler reported `Normal` for it) — an unchanged file otherwise keeps
    // serving its stale sideways thumbnail.
    3u32.hash(&mut h);
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
