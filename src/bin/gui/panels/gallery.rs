//! Gallery panel: folder thumbnails and selection.

use crate::*;

impl AutoshopApp {
    /// Left-most panel: the working-folder thumbnail gallery. Only visible rows
    /// are laid out (show_rows) and only their thumbnails are queued to decode.
    pub(crate) fn gallery_panel(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        ui.horizontal(|ui| {
            ui.heading(tr(lang, "Library"));
            if ui.button(tr(lang, "Open folder…")).clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                self.open_folder(dir);
            }
        });
        if let Some(d) = &self.gallery_dir {
            let dir = d.display().to_string();
            let cnt = self.gallery.len().to_string();
            ui.label(
                egui::RichText::new(trf(lang, "{dir} · {count} photos", &[("dir", &dir), ("count", &cnt)]))
                    .weak()
                    .small(),
            );
        }
        // Batch: copy the open photo's recipe → Ctrl+click a selection → paste.
        // Lightroom's "sync settings" for the whole working folder. Wrapped:
        // four dynamically sized controls in a fixed row clipped at narrow
        // panel widths.
        ui.horizontal_wrapped(|ui| {
            // Wrap BETWEEN buttons, never inside one: without this the last
            // button in a cramped row wrapped its own label one character
            // per line (a vertical "渲染选中" pillar at the default width).
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            // `!busy` like both siblings below. `open_path` re-points
            // `src_path` immediately but `recipe` only refreshes when the
            // decode lands, so during an open this pairs photo A's recipe
            // (below) with photo B's path — and `copied_from` is what
            // suppresses the "bitmap mask(s) not pasted" warning, so the
            // mismatch INVERTS that guard and writes A's raster masks into
            // B's saved develop.
            // These chips keep the compact Small label but wear a full button
            // frame: egui's small_button hard-zeroes vertical padding AND
            // skips the interact-size floor, so the boxes came out squat and
            // no style token could talk them out of it.
            let chip = |text: String| egui::Button::new(egui::RichText::new(text).small());
            ui.add_enabled_ui(self.src_path.is_some() && !self.busy, |ui| {
                if ui
                    .add(chip(tr(lang, "⎘ Copy recipe").to_string()))
                    .on_hover_text(tr(lang, "Copy every develop setting from the current photo"))
                    .clicked()
                {
                    // Flush a pending mask rename first — the clipboard must
                    // carry the name the user sees in the box (U10; CX5-9).
                    self.commit_mask_name_buf();
                    self.copied = Some(self.recipe.clone());
                    self.copied_from = self.src_path.clone();
                    self.status = tr(lang, "Recipe copied — Ctrl/⌘+click to pick several, then “Paste to selected”").to_string();
                }
            });
            let n = self.multi_sel.len();
            ui.add_enabled_ui(self.copied.is_some() && n > 0 && !self.busy, |ui| {
                let n_s = n.to_string();
                if ui
                    .add(chip(trf(lang, "⇩ Paste to selected ({n})", &[("n", &n_s)])))
                    .on_hover_text(tr(lang, "Writes each photo's develop into your develop store (recipe JSON; RAW also gets a Lightroom XMP). Leaves library files untouched, renders nothing."))
                    .clicked()
                {
                    self.start_paste();
                }
            });
            ui.add_enabled_ui(n > 0 && !self.busy, |ui| {
                let n_s = n.to_string();
                if ui
                    .add(chip(trf(lang, "🖼 Render selected ({n})", &[("n", &n_s)])))
                    .on_hover_text(tr(
                        lang,
                        "Each renders by its own saved develop from the store (neutral develop if none) → ./out/<name>.developed.*, using the current format / long-edge / sharpening / quality; AI Denoise sits out the batch.",
                    ))
                    .clicked()
                {
                    self.start_batch_render();
                }
            });
            if n > 0 && ui.add(chip("✕".to_string())).on_hover_text(tr(lang, "Clear selection")).clicked() {
                self.multi_sel.clear();
            }
        });
        if self.copied.is_some() {
            ui.checkbox(&mut self.paste_geometry, tr(lang, "Include crop / straighten when pasting"))
                .on_hover_text(tr(lang, "Off by default — composition rarely transfers between photos"));
        }
        ui.separator();
        if self.gallery.is_empty() {
            // Two different situations, two different messages: "no folder"
            // invites opening one; "empty folder" says the folder itself has
            // nothing to show (the old shared line read as a broken open).
            let hint = if self.gallery_dir.is_some() {
                tr(lang, "No photos in this folder — RAW / JPEG / PNG / TIFF would show up here.")
            } else {
                tr(lang, "Open a folder to browse your photos here.")
            };
            ui.add_space(SPACE_MD);
            ui.label(egui::RichText::new(hint).weak());
            return;
        }

        let count = self.gallery.len();
        // Borrow only the fields the row closure reads; collect actions to apply
        // after (request_thumb / open_gallery_index both need &mut self).
        let thumbs = &self.thumbs;
        let edited_badge = &mut self.edited_badge;
        let gallery = &self.gallery;
        let selected = self.selected;
        let multi_sel = &self.multi_sel;
        let colors = self.theme.colors();
        let mut to_open: Option<usize> = None;
        let mut to_toggle: Option<usize> = None;
        let mut to_request: Vec<usize> = Vec::new();
        let mut visible: std::ops::Range<usize> = 0..0;

        let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(i) = self.gallery_scroll_to.take() {
            // Keyboard navigation moved the selection: jump the viewport so the
            // highlighted row sits roughly centred. show_rows only lays out
            // visible rows, so scroll_to_me can never reach an off-screen row —
            // the offset (rows are fixed-height) is the reliable route.
            let target = (i as f32 * GALLERY_ROW_H - ui.available_height() * 0.4).max(0.0);
            scroll = scroll.vertical_scroll_offset(target);
        }
        scroll
            .show_rows(ui, GALLERY_ROW_H, count, |ui, range| {
                visible = range.clone();
                for i in range {
                    let path = &gallery[i];
                    let is_sel = selected == Some(i);
                    let is_multi = multi_sel.contains(&i);
                    let fill = if is_sel {
                        colors.sel_bg
                    } else if is_multi {
                        colors.sel_bg_dim
                    } else {
                        egui::Color32::TRANSPARENT
                    };
                    let resp = egui::Frame::none()
                        .fill(fill)
                        .rounding(RADIUS_SM) // selected-row fill follows the thumb radius
                        .inner_margin(egui::Margin::same(3.0))
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.horizontal(|ui| {
                                if let Some(t) = thumbs.get(&i) {
                                    ui.add(
                                        egui::Image::new(SizedTexture::new(
                                            t.id(),
                                            egui::vec2(THUMB_W, THUMB_H),
                                        ))
                                        .rounding(RADIUS_SM),
                                    );
                                } else {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(THUMB_W, THUMB_H),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().rect_filled(rect, RADIUS_SM, colors.thumb_placeholder);
                                    to_request.push(i);
                                }
                                ui.vertical(|ui| {
                                    let mut name = egui::RichText::new(autoshop::pipeline::stem(path)).small();
                                    if is_sel {
                                        name = name.strong().color(colors.accent_text);
                                    }
                                    // Truncate, never wrap: show_rows assumes
                                    // every row is exactly GALLERY_ROW_H, and a
                                    // wrapped long filename would misalign the
                                    // whole virtualized scroll.
                                    ui.add(egui::Label::new(name).truncate());
                                    // Cached: 2 filesystem stats per visible row
                                    // per repaint (continuous while thumbnails
                                    // decode) added up to thousands of syscalls
                                    // per second. Dropped whenever this app
                                    // writes a sidecar or the folder changes.
                                    let edited = *edited_badge.entry(i).or_insert_with(|| {
                                        // Central store OR legacy ./out — a
                                        // pre-migration library keeps its ● —
                                        // OR Lightroom's own sidecar (L13#2:
                                        // an LR-edited photo showed no badge
                                        // yet opened with its LR develop).
                                        autoshop::store::has_develop_or_sidecar(path)
                                    });
                                    let baked = !autoshop::decode::is_raw(path);
                                    ui.horizontal(|ui| {
                                        if is_multi {
                                            ui.label(egui::RichText::new(tr(lang, "✓ selected")).color(colors.accent_text).small());
                                        }
                                        if baked {
                                            ui.label(egui::RichText::new("PNG/TIFF").color(colors.accent_text).small());
                                        }
                                        if edited {
                                            ui.label(egui::RichText::new(tr(lang, "● edited")).color(colors.accent_text).small());
                                        }
                                    });
                                });
                            });
                        })
                        .response
                        .interact(egui::Sense::click());
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        // Ctrl+click toggles the batch selection; plain click opens.
                        if ui.input(|inp| inp.modifiers.command) {
                            to_toggle = Some(i);
                        } else {
                            to_open = Some(i);
                        }
                    }
                }
            });

        for i in to_request {
            self.request_thumb(i);
        }
        // Texture LRU (bounded GPU memory): every scrolled-past row otherwise
        // pins its 160px texture until the folder changes (~68 KB each — a
        // 10k-photo folder reached ~0.7 GB). Keep a generous window around the
        // viewport; evicted indices drop their `requested` marker so a
        // scroll-back re-materialises from the disk cache in ~1 ms.
        const THUMB_TEX_CAP: usize = 1500;
        if self.thumbs.len() > THUMB_TEX_CAP {
            let keep = visible.start.saturating_sub(600)..visible.end + 600;
            self.thumbs.retain(|i, _| keep.contains(i));
            self.thumb_requested.retain(|i| keep.contains(i));
        }
        if let Some(i) = to_toggle
            && !self.multi_sel.remove(&i)
        {
            self.multi_sel.insert(i);
        }
        if let Some(i) = to_open {
            self.open_gallery_index(i);
        }
    }
}
