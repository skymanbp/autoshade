//! The button vocabulary (R38). Every button in the app is one of four
//! kinds, and every kind stands exactly one row tall — `interact_size.y`,
//! the floor egui gives a framed button — so a row never mixes heights:
//!
//!  * [`primary`]  — the ONE main verb of its group (Export, Open photo,
//!    AI Analyze, Generate, Reverse-fit, Build, Apply, Save): solid [`PILL`]
//!    gold under near-black text on both themes (8.8:1 on the raw fill;
//!    `both_themes_pass_contrast_checks` pins the pair).
//!  * [`action`]   — every other text verb: the stock frame.
//!  * [`glyph`]    — an icon-only verb (＋ ▣ ✕ 🗑 ⧉ ⬆ ⬇ ↺ ⚙ ⌨): an exact
//!    square of the row height, so it lines up beside text verbs instead of
//!    sitting squat among them (egui's `small_button` zeroes the vertical
//!    padding AND skips the interact-size floor, which is where every
//!    uneven row came from).
//!  * [`toggle`] / [`glyph_toggle`] — an armed or selected state (tools,
//!    the eye 👁, the clipping ▲): the selection frame while on. A text
//!    toggle that needs no cell keeps egui's own `selectable_label`, which
//!    wears the same frame and the same floor.
//!
//! The top bar adds three composite shapes of the same row height
//! (2026-11-17): [`chip`] (the mode's name — a pill that says where you
//! are), [`segmented`] (the view switch as one control) and
//! [`split_button`] (Export and its ▾ as one pill, enabled or not).
//!
//! A glyph PREFIX on a text verb is vocabulary, not decoration: it stays
//! only where the same glyph marks the same meaning on at least two
//! buttons — 🤖 an AI verb outside the AI panel, ✓ finish, ✕ cancel, ＋
//! add, ↺/↻ reset & redraw, 🗂 a folder, 🖌 paint, 💧 pick from the image,
//! ⎘ the stamp, 🔄 fetch & rebuild, ✨ generated pixels, and the toolbar's
//! own pairs ↶↷ ⭯⭮ ◫⬛. The one-off glyphs (◌ ⊕ ⊖ ⇱ 🎛 📝 🖼 ⇩ 📷 ▭ ◯ ⌫
//! ⛶ 🎯) are gone; those buttons say their verb.
//!
//! Rows of equal verbs in the side panels lay out on [`columns`]: every
//! button in the row takes the same width, so the row lands in aligned
//! columns, left-aligned within the same [`FIELD_W_MAX`] readable ceiling
//! as the prompt fields, even when the panel is wider. Since 2026-09-27 (user
//! decision, after a probe of every drawn button) EVERY text verb in a side
//! panel is laid in a cell: a row's one verb fills the row ([`action_row`] /
//! [`primary_row`]), two or three share it equally, and a verb beside a
//! checkbox or a label takes one cell of the two-column grid measured at the
//! row's start — so every row of a panel ends on the same right edge, and no
//! verb is sized by the length of its own label. The toolbar, the dialogs and
//! the Settings window keep own-width verbs, as toolbars do. The toolbar wraps only
//! between its [`group`]s, each measured from its labels so egui can place it whole.
//! `every_button_stands_one_row_tall_at_the_default_widths` renders every
//! panel in both languages over three frames and pins the height, the
//! squares, that no label wraps inside its cell, and that neither side
//! panel leaves its default width.
//!
//! The other inputs of a side panel follow the same grid (2026-11-17), so a
//! fold's rows share one left edge and one right edge whatever stands on
//! them: a dropdown with its name in front takes [`row_label`] and
//! [`rest_of_row`]; a dropdown alone takes a cell of [`columns`]; a checkbox
//! is [`check`] / [`check_if`] (a square box — the round one is a radio); a
//! single-line text field takes `util::field_margin` and so stands one row
//! tall. A disabled button keeps a plate and a selected toggle keeps its
//! round corners ([`place`]). `every_input_of_a_kind_has_one_look` and
//! `a_disabled_button_has_a_plate_and_a_selected_toggle_has_round_corners`
//! pin these.

use super::*;

/// Text on the solid gold primary fill — the same near-black on both
/// themes, because the fill is the same gold on both.
pub(crate) const PRIMARY_FG: egui::Color32 = egui::Color32::from_gray(26);

/// One row: the height every button in this vocabulary stands.
pub(crate) fn row_h(ui: &egui::Ui) -> f32 {
    ui.spacing().interact_size.y
}

/// What a cell or a dropdown gives up of its exact share of the row, so that
/// the things laid across a row never sum past it by a float's rounding
/// error — a thousandth of a point, far under a pixel.
const ROW_GUARD: f32 = 0.001;

/// The size each of `n` equal cells takes across the rest of the row: the
/// exact share, less [`ROW_GUARD`]. Not whole points (2026-11-17): rounding
/// each cell down left a three-cell row up to two points short of the right
/// edge the two-cell rows, the rails and the dropdowns end on, so a fold's
/// rows did not share one right edge.
pub(crate) fn columns(ui: &egui::Ui, n: usize) -> egui::Vec2 {
    let n = n.max(1);
    let gaps = ui.spacing().item_spacing.x * (n - 1) as f32;
    let width = ui.available_width().min(FIELD_W_MAX);
    egui::vec2(((width - gaps) / n as f32 - ROW_GUARD).max(0.0), row_h(ui))
}

/// The width of a dropdown that takes what is left of its row — after the
/// row's label — so a labelled dropdown ends on the row's right edge, where
/// the sliders, the fields and the button cells end (2026-11-17). Each
/// dropdown used to carry a width of its own (70, 90, 96, 110, 130, 170 px),
/// so the Export fold alone had four right edges; and one of them, with its
/// label and two verbs, ran the English mask rows 5 px past their card. A
/// dropdown alone on its row takes a cell of [`columns`] like a button.
pub(crate) fn rest_of_row(ui: &egui::Ui) -> f32 {
    (ui.available_width().min(FIELD_W_MAX) - ROW_GUARD).max(FIELD_W_MIN)
}

/// The name in front of a labelled control — a dropdown with its name — in a
/// cell as wide as the widest name of its `block` (the names of the rows that
/// stand together; empty for a row on its own). With [`rest_of_row`] on the
/// control, the dropdowns of a block start on one x and end on one x
/// (2026-11-17): each used to start wherever its own name ended, so the
/// Export fold's four dropdowns had three left edges, and the names came in
/// three looks — body, small, and small-and-weak. This is the one look: body
/// size, in the colour a slider's name and a checkbox's text are drawn in,
/// so every control's name in a fold reads the same.
pub(crate) fn row_label(ui: &mut egui::Ui, text: &str, block: &[&str]) -> egui::Response {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let wide = |t: &str| ui.fonts(|f| f.layout_no_wrap(t.to_owned(), font.clone(), egui::Color32::PLACEHOLDER).size().x);
    let w = block.iter().copied().chain([text]).map(wide).fold(0.0, f32::max).ceil();
    let name = ui.visuals().widgets.inactive.text_color();
    ui.allocate_ui_with_layout(egui::vec2(w, row_h(ui)), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_width(w);
        ui.label(egui::RichText::new(text).color(name))
    })
    .inner
}

/// Every button this vocabulary drew on this thread — read by the
/// one-row-tall pin after a frame.
#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct Drawn {
    pub(crate) kind: &'static str,
    pub(crate) label: String,
    pub(crate) rect: egui::Rect,
    /// Whether the label fit its cell on one line — measured, not assumed.
    pub(crate) fits: bool,
    /// Whether the button was laid in a grid cell (the side-panel rule) or at
    /// its own width (the toolbar's).
    pub(crate) cell: bool,
}

#[cfg(test)]
thread_local! {
    pub(crate) static DRAWN: std::cell::RefCell<Vec<Drawn>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The one place a button reaches the screen: sized into `cell` when the
/// row is a grid, at its own width otherwise; disabled through egui's own
/// fade, never hidden.
///
/// A cell is ONE allocation of the row (`allocate_ui_with_layout`), so a
/// wrapped row carries the whole cell to its next line. Not a `scope`
/// around `add_sized`: a scope is laid at the cursor with whatever width is
/// left there and never wraps, so a cell past its line's end stood outside
/// the panel and widened the auto-fitting side panel every frame (R19's
/// runaway, met again as +47 px/frame while this vocabulary went in).
fn place(
    ui: &mut egui::Ui,
    kind: &'static str,
    cell: Option<egui::Vec2>,
    enabled: bool,
    text: egui::WidgetText,
    style: impl FnOnce(egui::Button<'static>) -> egui::Button<'static>,
) -> egui::Response {
    // A glyph fills its square edge to edge: a 24 px symbol plus egui's
    // 2 x 4 px of horizontal padding asks for 32 px, which the justified
    // layout cannot centre inside a 26 px square — it overflows it. A text
    // label keeps the padding on both sides.
    let square = kind.starts_with("glyph");
    #[cfg(test)]
    let label = text.text().to_owned();
    #[cfg(test)]
    let fits = cell.is_none_or(|cell| {
        let width = ui.fonts(|f| {
            f.layout_no_wrap(label.clone(), egui::TextStyle::Button.resolve(ui.style()), egui::Color32::WHITE)
                .size()
                .x
        });
        let padding = if square { 0.0 } else { 2.0 * ui.spacing().button_padding.x };
        width + padding <= cell.x
    });
    // The look (2026-10-14, after shadcn/ui's variants): a text verb is a
    // tonal fill with no outline — the outline made every row a grid of
    // boxes; an icon-only verb at rest is a GHOST, no fill and no edge, and
    // gets its plate only under the pointer (painted into a slot reserved
    // BEFORE the button, so it lies beneath the icon). A selected glyph
    // keeps its selection frame.
    let ghost = square && !matches!(kind, "glyph_toggle_on");
    // The corner is set HERE for every state (2026-11-17): egui draws a
    // `selected` button with square corners unless it is told a rounding, so
    // an armed tool (Brush, 3-way, the eye) stood as a sharp box in a row of
    // rounded ones. `style` runs after, so a shape of its own (the chip's
    // pill, the split's two halves) still wins.
    let corner = ui.visuals().widgets.inactive.rounding;
    let base = egui::Button::new(text).rounding(corner);
    // A text verb that cannot be pressed draws NO fill of its own; its plate
    // is painted under it below, at the resting fill. Left to itself egui
    // fades a disabled button's fill halfway to the panel colour — on the
    // dark themes that is the card's own grey, give or take a level — and
    // paints it opaque, over anything put beneath it.
    let base = if !ghost && !enabled { base.fill(egui::Color32::TRANSPARENT) } else { base };
    let button = style(base);
    let button = if ghost { button.frame(false) } else { button.stroke(egui::Stroke::NONE) };
    // A cell's label TRUNCATES rather than extends (2026-10-18, user
    // screenshot): a row's default wrap mode is Extend, so a label wider
    // than its cell — a side panel dragged narrower than the default the
    // `fits` pin measures at — grew the button past the cell and pushed the
    // row's last button out of its card. The cell is the width contract.
    let button = if cell.is_some() { button.truncate() } else { button };
    let plate = ui.painter().add(egui::Shape::Noop);
    let response = match cell {
        Some(cell) => {
            let layout = egui::Layout::centered_and_justified(ui.layout().main_dir());
            ui.allocate_ui_with_layout(cell, layout, |ui| {
                if square {
                    ui.spacing_mut().button_padding.x = 0.0;
                }
                ui.add_enabled(enabled, button)
            })
            .inner
        }
        None => ui.add_enabled(enabled, button),
    };
    if ghost && enabled && (response.hovered() || response.has_focus()) {
        let w = &ui.visuals().widgets;
        let fill = if response.is_pointer_button_down_on() { w.active.weak_bg_fill } else { w.hovered.weak_bg_fill };
        ui.painter().set(plate, egui::Shape::rect_filled(response.rect, w.hovered.rounding, fill));
    }
    // A text verb that cannot be pressed KEEPS its plate (2026-11-17, user:
    // the panel's buttons 「还是很奇怪」). egui fades everything a disabled
    // widget paints halfway to the panel colour, and a tonal plate faded that
    // far is the card's own grey: 「AI edit」, 「Reverse-fit recipe」 and every
    // other waiting verb stood as loose text in the middle of an invisible
    // cell, beside neighbours that were plainly buttons. The plate is painted
    // into the slot under the button at the resting fill — by this `Ui`'s
    // painter, which does not fade — and the button itself was given no fill
    // above, so only the label dims. Where the resting fill is transparent
    // (the toolbar's quiet verbs) this paints nothing, as there; a shape that
    // sets a fill of its own in `style` (the toolbar's split) keeps it.
    if !ghost && !enabled {
        let w = &ui.visuals().widgets.inactive;
        ui.painter().set(plate, egui::Shape::rect_filled(response.rect, corner, w.weak_bg_fill));
    }
    #[cfg(test)]
    let kind = if kind == "glyph_toggle_on" { "glyph_toggle" } else { kind };
    #[cfg(test)]
    DRAWN.with_borrow_mut(|d| d.push(Drawn { kind, label, rect: response.rect, fits, cell: cell.is_some() }));
    response
}

/// One toolbar group as ONE allocation of the wrapped toolbar row, so a
/// narrow window carries the whole group to the next line instead of
/// splitting Undo from Redo. egui wraps only between allocations, and a
/// `horizontal` child is laid at the cursor at whatever width is left
/// there, so the group's width is measured up front from its `items`
/// ([`group_width`] — the same galleys its buttons draw, plus their padding)
/// and the allocation is held to exactly that width, so the bar's right
/// zone can be placed flush right by arithmetic.
///
/// `divider`: the hairline after the group, between it and its neighbour in
/// the same zone (2026-11-17, replacing the wider gap that was the only
/// thing telling twelve same-weight labels apart). It lives INSIDE the
/// allocation, at its end: a separator of its own would wrap to a line start
/// and sit there orphaned. 1 px, 16 px tall, centred in the row, with
/// `SPACE_MD` of air on each side (the row's own item spacing after the
/// allocation is part of the right-hand air).
pub(crate) fn group<R>(
    ui: &mut egui::Ui,
    items: &[Item],
    divider: bool,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let width = group_width(ui, items, divider);
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    ui.allocate_ui_with_layout(egui::vec2(width, row_h(ui)), layout, |ui| {
        // Held to the measured width from the START: egui extends a min
        // width from the cursor, so set after the content it would double.
        ui.set_min_width(width);
        let r = add(ui);
        if divider {
            // The cursor stands one item spacing past the last widget.
            let content_right = ui.cursor().left() - ui.spacing().item_spacing.x;
            let x = (content_right + SPACE_MD).round() + 0.5;
            let mid = ui.max_rect().center().y;
            let edge = current_theme().colors().widget_edge;
            ui.painter().vline(x, egui::Rangef::new(mid - 8.0, mid + 8.0), egui::Stroke::new(1.0, edge));
        }
        r
    })
    .inner
}

/// The app mark at the bar's left end: the window icon, this many points
/// square.
pub(crate) const MARK_PT: f32 = 18.0;

/// How far a [`segmented`] control's chosen segment sits inside its
/// container.
pub(crate) const SEG_INSET: f32 = 2.0;

/// One item of a toolbar group, for measuring the group before it is laid —
/// the same widths the drawing helpers below produce, so a measured group is
/// exactly as wide as what it draws (the right zone's flush-right gap depends
/// on that).
#[derive(Clone, Copy)]
pub(crate) enum Item<'a> {
    /// A text verb at its own width ([`action`], [`primary`], [`chip`]).
    Text(&'a str),
    /// A [`glyph`] square of the row height.
    Glyph,
    /// The app mark ([`MARK_PT`] square).
    Mark,
    /// A [`segmented`] control of these segment labels.
    Segmented(&'a [&'a str]),
    /// A [`split_button`]: the label's half and a chevron square.
    Split(&'a str),
}

/// A label's width as a text button lays it: the galley and the button's
/// padding.
fn text_button_w(ui: &egui::Ui, label: &str) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let text = ui.fonts(|f| f.layout_no_wrap(label.to_owned(), font, egui::Color32::WHITE).size().x);
    text + 2.0 * ui.spacing().button_padding.x
}

/// One item's width (see [`Item`]).
pub(crate) fn item_width(ui: &egui::Ui, item: &Item) -> f32 {
    match item {
        Item::Text(label) => text_button_w(ui, label),
        Item::Glyph => row_h(ui),
        Item::Mark => MARK_PT,
        Item::Segmented(labels) => labels.iter().map(|l| text_button_w(ui, l)).sum::<f32>() + 2.0 * SEG_INSET,
        Item::Split(label) => text_button_w(ui, label) + row_h(ui),
    }
}

/// The hairline between two groups of one toolbar zone, and its air: the
/// part of the divider a group's allocation carries. The row's own item
/// spacing follows the allocation, so the line has `SPACE_MD` on each side.
fn divider_part(ui: &egui::Ui) -> f32 {
    2.0 * SPACE_MD + 1.0 - ui.spacing().item_spacing.x
}

/// A toolbar group's width: its items, the row spacing between them, and its
/// divider when it draws one. ONE measure for [`group`] and for the bar's
/// right-zone arithmetic, so the two cannot disagree.
pub(crate) fn group_width(ui: &egui::Ui, items: &[Item], divider: bool) -> f32 {
    let widths: f32 = items.iter().map(|i| item_width(ui, i)).sum();
    let gaps = ui.spacing().item_spacing.x * items.len().saturating_sub(1) as f32;
    (widths + gaps + if divider { divider_part(ui) } else { 0.0 }).ceil()
}

/// The toolbar's display of a verb whose catalogue entry carries both
/// languages (「导出 · Export」): the part before " · ". The Export fold's
/// heading uses the same key and keeps both, like its sibling folds; the bar
/// has no room for a second language, so only the bar trims it.
pub(crate) fn bar_label(s: &str) -> &str {
    s.split(" · ").next().unwrap_or(s)
}

/// The mode chip: the mode's name as a pill — the workspace's "you are here"
/// — a plate at rest that brightens under the pointer, unlike the bar's
/// quiet verbs, in the strong text colour.
pub(crate) fn chip(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let c = current_theme().colors();
    ui.scope(|ui| {
        let strong = ui.visuals().strong_text_color();
        let w = &mut ui.visuals_mut().widgets;
        // The bar plate, not `widget_fill`: on Light that is the bar's own
        // grey and the chip read as plain text (`ThemeColors::bar_plate`).
        w.inactive.weak_bg_fill = c.bar_plate();
        w.hovered.weak_bg_fill = c.widget_hover;
        w.active.weak_bg_fill = c.widget_hover;
        let pill = row_h(ui) / 2.0;
        place(ui, "chip", None, true, egui::RichText::new(text).color(strong).into(), |b| b.rounding(pill))
    })
    .inner
}

/// A segmented control: ONE container a row tall holding the `items` —
/// (label, chosen, hover text) — side by side. The chosen segment wears the
/// app's chosen-state look (the selection fill, accent text) inset by
/// [`SEG_INSET`]; the others are plain text that brightens under the
/// pointer. Each segment senses the container's full height. Returns the
/// segment clicked this frame.
pub(crate) fn segmented(ui: &mut egui::Ui, items: &[(&str, bool, &str)]) -> Option<usize> {
    let c = current_theme().colors();
    let h = row_h(ui);
    let widths: Vec<f32> = items.iter().map(|(l, ..)| text_button_w(ui, l)).collect();
    let total = widths.iter().sum::<f32>() + 2.0 * SEG_INSET;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(total, h), egui::Sense::hover());
    ui.painter().rect(rect, RADIUS_MD, ui.visuals().extreme_bg_color, egui::Stroke::new(1.0, c.widget_edge));
    // The container itself, so a pin can find the segments inside it.
    #[cfg(test)]
    DRAWN.with_borrow_mut(|d| {
        let label = items.iter().map(|(l, ..)| *l).collect::<Vec<_>>().join(" | ");
        d.push(Drawn { kind: "segmented", label, rect, fits: true, cell: false });
    });
    let font = egui::TextStyle::Button.resolve(ui.style());
    let mut x = rect.left() + SEG_INSET;
    let mut clicked = None;
    for (k, ((label, chosen, hover), w)) in items.iter().zip(widths).enumerate() {
        let seg = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w, h));
        x += w;
        let resp = ui.interact(seg, ui.id().with(("segment", *label)), egui::Sense::click());
        let ink = if *chosen {
            ui.painter().rect_filled(seg.shrink2(egui::vec2(0.0, SEG_INSET)), RADIUS_SM, ui.visuals().selection.bg_fill);
            c.accent_text
        } else if resp.hovered() {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().text_color()
        };
        let galley = ui.painter().layout_no_wrap((*label).to_owned(), font.clone(), ink);
        ui.painter().galley(seg.center() - galley.size() / 2.0, galley, ink);
        #[cfg(test)]
        DRAWN.with_borrow_mut(|d| d.push(Drawn { kind: "segment", label: (*label).to_owned(), rect: seg, fits: true, cell: false }));
        if resp.on_hover_text(*hover).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            clicked = Some(k);
        }
    }
    clicked
}

/// A split button: the primary verb `text` and a chevron square that opens
/// `menu`, joined into ONE pill in every state — enabled, the accent fill
/// under near-black ink with a faint divider between the halves; disabled,
/// the ordinary plate across both halves, so the chevron never floats alone.
/// Outer corners round, inner corners square, no gap. Returns the main
/// half's response and the chevron's.
pub(crate) fn split_button(
    ui: &mut egui::Ui,
    enabled: bool,
    text: &str,
    menu: impl FnOnce(&mut egui::Ui),
) -> (egui::Response, egui::Response) {
    let c = current_theme().colors();
    let r = ui.visuals().widgets.inactive.rounding.nw;
    // Disabled: the bar plate across both halves (`ThemeColors::bar_plate`),
    // so the chevron never floats alone on Light either.
    let fill = if enabled { c.accent_fill } else { c.bar_plate() };
    let ink = if enabled { PRIMARY_FG } else { ui.visuals().text_color() };
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let main = place(ui, "primary", None, enabled, egui::RichText::new(text).color(ink).into(), |b| {
            b.fill(fill).rounding(egui::Rounding { nw: r, sw: r, ne: 0.0, se: 0.0 })
        });
        let h = row_h(ui);
        // `add_enabled_ui`: the menu helper takes no enabled flag.
        let chevron = ui
            .add_enabled_ui(enabled, |ui| {
                // No side padding: the chevron is a square of the row height,
                // and egui's padding around ▾ asked for more than that.
                ui.spacing_mut().button_padding.x = 0.0;
                let button = egui::Button::new(egui::RichText::new("▾").color(ink))
                    .fill(fill)
                    .stroke(egui::Stroke::NONE)
                    .rounding(egui::Rounding { nw: 0.0, sw: 0.0, ne: r, se: r })
                    .min_size(egui::vec2(h, h));
                egui::menu::menu_custom_button(ui, button, menu).response
            })
            .inner;
        if enabled {
            let x = main.rect.right();
            let y = main.rect.y_range().shrink(SPACE_MD);
            ui.painter().vline(x, y, egui::Stroke::new(1.0, PRIMARY_FG.gamma_multiply(0.25)));
        }
        #[cfg(test)]
        DRAWN.with_borrow_mut(|d| d.push(Drawn { kind: "glyph", label: "▾".into(), rect: chevron.rect, fits: true, cell: false }));
        (main, chevron)
    })
    .inner
}

fn primary_text(text: impl Into<String>) -> egui::WidgetText {
    egui::RichText::new(text).color(PRIMARY_FG).into()
}

/// A secondary text verb at its own width.
pub(crate) fn action(ui: &mut egui::Ui, enabled: bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    place(ui, "action", None, enabled, text.into(), |b| b)
}

/// A secondary text verb filling one grid cell of the row ([`columns`]).
pub(crate) fn action_in(
    ui: &mut egui::Ui,
    cell: egui::Vec2,
    enabled: bool,
    text: impl Into<egui::WidgetText>,
) -> egui::Response {
    place(ui, "action", Some(cell), enabled, text.into(), |b| b)
}

/// The group's one main verb at its own width.
pub(crate) fn primary(ui: &mut egui::Ui, enabled: bool, text: impl Into<String>) -> egui::Response {
    place(ui, "primary", None, enabled, primary_label(text, enabled), |b| primary_fill(b, enabled))
}

/// The group's one main verb filling one grid cell of the row.
pub(crate) fn primary_in(ui: &mut egui::Ui, cell: egui::Vec2, enabled: bool, text: impl Into<String>) -> egui::Response {
    place(ui, "primary", Some(cell), enabled, primary_label(text, enabled), |b| primary_fill(b, enabled))
}

/// A disabled primary is an ordinary disabled button (2026-10-13): egui's
/// disabled fade over the gold fill made a muddy brown slab — the panel's
/// loudest colour on a control that cannot be pressed.
fn primary_fill(b: egui::Button<'static>, enabled: bool) -> egui::Button<'static> {
    if enabled { b.fill(current_theme().colors().accent_fill) } else { b }
}

/// …and its label takes the theme's text colour then, not the near-black
/// made for the gold.
fn primary_label(text: impl Into<String>, enabled: bool) -> egui::WidgetText {
    if enabled { primary_text(text) } else { egui::WidgetText::from(text.into()) }
}

/// A row's one secondary verb, filling the row (one cell of [`columns`] 1).
pub(crate) fn action_row(ui: &mut egui::Ui, enabled: bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    let cell = columns(ui, 1);
    action_in(ui, cell, enabled, text)
}

/// A row's one main verb, filling the row.
pub(crate) fn primary_row(ui: &mut egui::Ui, enabled: bool, text: impl Into<String>) -> egui::Response {
    let cell = columns(ui, 1);
    primary_in(ui, cell, enabled, text)
}

/// The primary verb as a builder, for the one row that sizes its button by
/// hand (the Reimagine prompt row measures it against its field).
pub(crate) fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(primary_text(text)).fill(current_theme().colors().accent_fill)
}

/// An armed / selected text state filling one grid cell of the row.
pub(crate) fn toggle_in(ui: &mut egui::Ui, cell: egui::Vec2, on: bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    place(ui, "toggle", Some(cell), true, text.into(), |b| b.selected(on))
}

/// An icon-only verb: an exact square of the row height.
pub(crate) fn glyph(ui: &mut egui::Ui, enabled: bool, g: impl Into<egui::WidgetText>) -> egui::Response {
    let h = row_h(ui);
    place(ui, "glyph", Some(egui::vec2(h, h)), enabled, g.into(), |b| b)
}

/// A checkbox: egui's own, with a SQUARE box (2026-11-17). The app rounds
/// every control by `RADIUS_MD` (6 px), and on egui's 14 px box that is a
/// circle — an unticked checkbox and an unpicked radio were the same drawing,
/// so a row could not say whether its choices exclude one another. The box
/// takes `RADIUS_SM` here; the radios stay round. The rounding is changed on
/// this `Ui` for the one widget and put back — no child `Ui`, so the box
/// lays out in its row exactly as `ui.checkbox` does. EVERY checkbox of the
/// app goes through here or [`check_if`]; `every_input_of_a_kind_has_one_look`
/// reads the sources for one that does not.
pub(crate) fn check(ui: &mut egui::Ui, on: &mut bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    check_if(ui, true, on, text)
}

/// [`check`] that can be greyed: `ui.add_enabled(enabled, Checkbox)` with the
/// same square box.
pub(crate) fn check_if(
    ui: &mut egui::Ui,
    enabled: bool,
    on: &mut bool,
    text: impl Into<egui::WidgetText>,
) -> egui::Response {
    let saved = ui.visuals().widgets.clone();
    let w = &mut ui.visuals_mut().widgets;
    for state in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.rounding = RADIUS_SM.into();
    }
    let response = ui.add_enabled(enabled, egui::Checkbox::new(on, text));
    ui.visuals_mut().widgets = saved;
    response
}

/// An icon-only armed / selected state: the same square, the selection
/// frame while on.
pub(crate) fn glyph_toggle(ui: &mut egui::Ui, on: bool, g: impl Into<egui::WidgetText>) -> egui::Response {
    let h = row_h(ui);
    let kind = if on { "glyph_toggle_on" } else { "glyph_toggle" };
    place(ui, kind, Some(egui::vec2(h, h)), true, g.into(), |b| b.selected(on))
}
