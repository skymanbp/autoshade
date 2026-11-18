//! The library panel — the left-most column: the working folder, the batch
//! verbs over a pick of its photos, and its thumbnails.
//!
//! ONE LAYOUT IN EVERY STATE (2026-11-17, user: 「最左侧那一栏的按钮布局、美观
//! 还需要再优化」). What stands above the thumbnails keeps one height whatever
//! is copied or picked, so the list never moves under the pointer that is
//! picking from it, and every row of it ends on the column's one right edge:
//!
//!  * the title row — the panel's title under the gold rule the controls
//!    column's panel titles wear, and 「Open folder…」 at the row's far end;
//!  * the folder line — the folder's name and its count on one line, cut with
//!    an ellipsis; the full path is its hover text;
//!  * the batch block (not in Normal, `mode::Shell`) — two rows of two equal
//!    cells, Copy | Paste over Render | Deselect, and the paste's one option
//!    under them, greyed until a recipe is copied.
//!
//! Before this the title and its verb filled their row to the last pixel in
//! Chinese and left it ragged in English; the three batch verbs were three
//! full-width slabs with ✕ alone at the end of the last; the option appeared
//! only once a recipe was copied and pushed the whole list down a row, and its
//! English label was wider than the panel and widened it (`side_panel.rs`).
//! `the_library_column_is_one_layout_in_every_state` pins the height, the
//! right edge, the panel's width and that every label fits its cell.
//!
//! A thumbnail row reads as two quiet lines beside the picture, centred on
//! it: the name at body size, and under it what is true of the file — picked,
//! its format when it is not a RAW, edited. Only the pick and the edited dot
//! take the accent; the format used to be a gold 「PNG/TIFF」 on every file
//! that was not a RAW, a JPEG included.

use crate::*;

impl AutoShadeApp {
    /// Left-most panel: the working-folder thumbnail gallery. Only visible rows
    /// are laid out (show_rows) and only their thumbnails are queued to decode.
    pub(crate) fn gallery_panel(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        self.library_header(ui);
        // The batch verbs stand whenever there is something to run them on —
        // a folder, or a photo — and not over the bare first-run column,
        // where four greyed plates said nothing. Not drawn where the mode
        // runs its batch from the sidebar (Normal, `mode::Shell`).
        let something_open = self.gallery_dir.is_some() || self.src_path.is_some();
        if something_open && self.mode.shell().library_batch_rows {
            self.library_batch(ui);
        }
        if self.gallery.is_empty() {
            // Two different situations, two different messages: "no folder"
            // invites opening one; "empty folder" says the folder itself has
            // nothing to show (the old shared line read as a broken open).
            // Set like the controls column's empty state: centred and quiet.
            let hint = if self.gallery_dir.is_some() {
                tr(lang, "No photos in this folder — RAW / JPEG / PNG / TIFF would show up here.")
            } else {
                tr(lang, "Open a folder to browse your photos here.")
            };
            ui.add_space(SPACE_LG * 2.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("🗂").size(26.0).weak());
                ui.add_space(SPACE_SM);
                ui.label(egui::RichText::new(hint).weak());
            });
            return;
        }
        ui.separator();
        self.library_rows(ui);
    }

    /// The title row and the folder line.
    fn library_header(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let row = ui.horizontal(|ui| {
            // The catalogue's first language only (「图库」, not 「图库 · Library」):
            // the title shares its row with a verb, as the toolbar's labels do
            // (`buttons::bar_label`). Cut, not wrapped, where a host narrower
            // than the app's own panel leaves it less than its width.
            let title = egui::RichText::new(bar_label(tr(lang, "Library"))).heading().strong();
            let left = ui.add(egui::Label::new(title).truncate()).rect.left();
            // The verb at the row's far end, at its own width: it ends on the
            // right edge the button cells below end on.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if action(ui, true, tr(lang, "🗂 Open folder…")).clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                {
                    self.open_folder(dir);
                }
            });
            left
        });
        title_rule(ui, row.inner, row.response.rect.bottom());
        // ONE line, never wrapped (2026-11-17: the full path wrapped mid-way
        // and orphaned the count): the folder's own name and the count, cut
        // with an ellipsis if the panel is narrower still; the full path is
        // the hover text.
        if let Some(d) = &self.gallery_dir {
            let name = d.file_name().map_or_else(|| d.display().to_string(), |n| n.to_string_lossy().into_owned());
            let cnt = self.gallery.len().to_string();
            ui.add(
                egui::Label::new(
                    egui::RichText::new(trf(lang, "{dir} · {count} photos", &[("dir", &name), ("count", &cnt)]))
                        .weak()
                        .small(),
                )
                .truncate(),
            )
            .on_hover_text(d.display().to_string());
        }
    }

    /// The batch verbs — copy the open photo's recipe, Ctrl+click a pick,
    /// paste; Lightroom's "sync settings" for the working folder — as two
    /// rows of two equal cells, and the paste's one option under them.
    ///
    /// The block's height is the same whatever is copied or picked: a verb
    /// that cannot run is greyed in its cell, and the option keeps its row.
    fn library_batch(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let n = self.multi_sel.len();
        let n_s = n.to_string();
        // A verb over the pick names how many photos it would touch; with
        // nothing picked it is the bare verb.
        let paste = if n > 0 { trf(lang, "Paste ({n})", &[("n", &n_s)]) } else { tr(lang, "Paste").to_string() };
        let render = if n > 0 { trf(lang, "Render ({n})", &[("n", &n_s)]) } else { tr(lang, "Render").to_string() };
        let cell = columns(ui, 2);
        // A greyed verb says under the pointer what it is waiting for — the
        // cells are always there now, so three of the four are grey until a
        // recipe is copied and a photo picked. (While a job runs the verbs
        // are grey too, and say nothing: the status bar names the job.)
        let needs_pick = tr(lang, "Ctrl/⌘+click photos to pick them first.");
        ui.horizontal(|ui| {
            // `!busy` like its siblings. `open_path` re-points `src_path`
            // immediately but `recipe` only refreshes when the decode lands,
            // so during an open this pairs photo A's recipe (below) with
            // photo B's path — and `copied_from` is what suppresses the
            // "bitmap mask(s) not pasted" warning, so the mismatch INVERTS
            // that guard and writes A's raster masks into B's saved develop.
            let copy = action_in(ui, cell, self.src_path.is_some() && !self.busy, tr(lang, "Copy recipe"))
                .on_hover_text(tr(lang, "Copy every develop setting from the current photo"));
            let copy = if self.src_path.is_none() {
                copy.on_disabled_hover_text(tr(lang, "Open a photo first — click a thumbnail below."))
            } else {
                copy
            };
            if copy.clicked() {
                // Flush every pending rename first — the clipboard must
                // carry the name the user sees in the box (U10; CX5-9).
                self.commit_pending_names();
                self.copied = Some(self.recipe.clone());
                self.copied_from = self.src_path.clone();
                self.status = tr(lang, "Recipe copied — Ctrl/⌘+click to pick several, then “Paste”").to_string();
            }
            let ready = self.copied.is_some() && n > 0;
            let paste = action_in(ui, cell, ready && !self.busy, paste)
                .on_hover_text(tr(lang, "Writes each photo's develop into your develop store (recipe JSON; RAW also gets a Lightroom XMP). Leaves library files untouched, renders nothing."));
            let paste = if ready {
                paste
            } else {
                paste.on_disabled_hover_text(tr(lang, "Copy a recipe, then Ctrl/⌘+click the photos to paste it onto."))
            };
            if paste.clicked() {
                self.start_paste();
            }
        });
        ui.horizontal(|ui| {
            let render = action_in(ui, cell, n > 0 && !self.busy, render).on_hover_text(tr(
                lang,
                "Each renders by its own saved develop from the store (neutral develop if none) → <Destination>/<name>.developed.*, using the current format / long-edge / sharpening / quality; AI Denoise sits out the batch.",
            ));
            let render = if n > 0 { render } else { render.on_disabled_hover_text(needs_pick) };
            if render.clicked() {
                self.start_batch_render();
            }
            if action_in(ui, cell, n > 0, tr(lang, "Deselect"))
                .on_hover_text(tr(lang, "Unpick every photo — Ctrl/⌘+click picks or unpicks one"))
                .on_disabled_hover_text(needs_pick)
                .clicked()
            {
                self.multi_sel.clear();
            }
        });
        // On ONE line, as the folder line and the cells are: a label wider
        // than the column would be cut, not wrapped onto a second row. At the
        // app's own widths it is never cut — the pin wants it drawn whole at
        // every width of the panel's range, in both languages — so the cut
        // only ever meets a host narrower than the app's panel.
        ui.scope(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            let why = tr(lang, "Off by default — composition rarely transfers between photos");
            check_if(ui, self.copied.is_some(), &mut self.paste_geometry, tr(lang, "Paste crop / straighten too"))
                .on_hover_text(why)
                .on_disabled_hover_text(why);
        });
    }

    /// The thumbnails: one fixed-height row per photo, only the visible ones
    /// laid out.
    fn library_rows(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
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

        // Geometry of the first drawn thumbnail this frame (slot, image) —
        // recorded through a local because the row closure below borrows
        // `self`'s fields piecemeal and cannot touch `self` itself.
        #[cfg(test)]
        let mut thumb_geom: Option<(egui::Rect, egui::Rect)> = None;
        let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(i) = self.gallery_scroll_to.take() {
            // Keyboard navigation moved the selection: jump the viewport so the
            // highlighted row sits roughly centred. show_rows only lays out
            // visible rows, so scroll_to_me can never reach an off-screen row —
            // the offset (rows are fixed-height) is the reliable route.
            let target = (i as f32 * GALLERY_ROW_H - ui.available_height() * 0.4).max(0.0);
            scroll = scroll.vertical_scroll_offset(target);
        }
        // The air between a row's picture and the edge of its plate.
        const ROW_PAD: f32 = 3.0;
        // The two text lines of a row, and the air between them.
        let name_h = ui.text_style_height(&egui::TextStyle::Body);
        let meta_h = ui.text_style_height(&egui::TextStyle::Small);
        let meta_font = egui::TextStyle::Small.resolve(ui.style());
        let weak = ui.visuals().weak_text_color();
        let hover_fill = ui.visuals().widgets.inactive.weak_bg_fill;
        scroll
            .show_rows(ui, GALLERY_ROW_H, count, |ui, range| {
                visible = range.clone();
                for i in range {
                    let path = &gallery[i];
                    let is_sel = selected == Some(i);
                    let is_multi = multi_sel.contains(&i);
                    // The row's plate — the open photo's, a picked photo's, or
                    // the pointer's — is painted UNDER the row once its extent
                    // is known, and reaches ROW_PAD past it on each side. So
                    // the thumbnails stand on the column's own left edge,
                    // where the title and the button cells start, and a plate
                    // still has air around its picture. (The plate used to be
                    // the row's frame, which set every thumbnail 3 px in from
                    // everything above it.)
                    let plate = ui.painter().add(egui::Shape::Noop);
                    let resp = egui::Frame::none()
                        .inner_margin(egui::Margin::symmetric(0.0, ROW_PAD))
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.horizontal(|ui| {
                                if let Some(t) = thumbs.get(&i) {
                                    // CONSTANT SLOT + aspect-preserving inset
                                    // (#17). The old code handed egui a
                                    // SizedTexture claiming the texture WAS
                                    // THUMB_W × THUMB_H; a Texture source is
                                    // drawn at ImageFit::Exact, so that lie
                                    // squashed every portrait into the same
                                    // 1.4:1 landscape box (the decode-side
                                    // orientation chain was never at fault).
                                    // The slot stays fixed — byte-identical to
                                    // the placeholder branch below — because it
                                    // is what keeps the filename column aligned
                                    // down the whole list; a portrait gets a
                                    // ~27 × 40 letterboxed inset inside it.
                                    let (slot, _) = ui.allocate_exact_size(
                                        egui::vec2(THUMB_W, THUMB_H),
                                        egui::Sense::hover(),
                                    );
                                    // Thumbnails decode to THUMB_EDGE (160 px)
                                    // long edge, so the scale is ≤ 0.35 here and
                                    // fit_in's 4× upscale cap never bites.
                                    let draw = fit_in(t.size_vec2(), THUMB_W, THUMB_H);
                                    let rect = egui::Rect::from_center_size(slot.center(), draw);
                                    // painter().image() has no rounding, and the
                                    // rounded corners are the gallery's look —
                                    // so this is egui's own paint_texture_at
                                    // shape (widgets/image.rs), inlined.
                                    ui.painter().add(egui::epaint::RectShape {
                                        rect,
                                        rounding: RADIUS_SM.into(),
                                        fill: egui::Color32::WHITE, // tint: the texture unmodified
                                        stroke: egui::Stroke::NONE,
                                        blur_width: 0.0,
                                        fill_texture_id: t.id(),
                                        uv: egui::Rect::from_min_max(
                                            egui::pos2(0.0, 0.0),
                                            egui::pos2(1.0, 1.0),
                                        ),
                                    });
                                    #[cfg(test)]
                                    {
                                        if thumb_geom.is_none() {
                                            thumb_geom = Some((slot, rect));
                                        }
                                    }
                                } else {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(THUMB_W, THUMB_H),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().rect_filled(rect, RADIUS_SM, colors.thumb_placeholder);
                                    to_request.push(i);
                                }
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
                                    autoshade::store::has_develop_or_sidecar(path)
                                });
                                // What is true of the file, as ONE label: a
                                // row of labels stands a full control row
                                // tall, which hung the line low and pushed
                                // the two lines past the picture.
                                let mut meta = egui::text::LayoutJob::default();
                                let mut say = |text: &str, gap: f32, colour: egui::Color32| {
                                    let lead = if meta.sections.is_empty() { 0.0 } else { gap };
                                    meta.append(text, lead, egui::TextFormat::simple(meta_font.clone(), colour));
                                };
                                if is_multi {
                                    say(tr(lang, "✓ selected"), SPACE_MD, colors.accent_text);
                                }
                                // A file that is not a RAW says what it is —
                                // its pixels are baked, and the develop
                                // treats them so.
                                if !autoshade::decode::is_raw(path)
                                    && let Some(ext) = path.extension().and_then(|e| e.to_str())
                                {
                                    say(&ext.to_uppercase(), SPACE_MD, weak);
                                }
                                if edited {
                                    // The dot in the accent, the word quiet.
                                    let label = tr(lang, "● edited");
                                    let (dot, word) = label.split_once(' ').unwrap_or((label, ""));
                                    say(dot, SPACE_MD, colors.accent_text);
                                    say(word, SPACE_SM, weak);
                                }
                                let has_meta = !meta.sections.is_empty();
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = SPACE_XS;
                                    // The lines are centred on the picture.
                                    let block = if has_meta { name_h + SPACE_XS + meta_h } else { name_h };
                                    ui.add_space(((THUMB_H - block) / 2.0).max(0.0));
                                    let mut name = egui::RichText::new(autoshade::pipeline::stem(path));
                                    if is_sel {
                                        name = name.strong().color(colors.accent_text);
                                    }
                                    // Truncate, never wrap: show_rows assumes
                                    // every row is exactly GALLERY_ROW_H, and a
                                    // wrapped long filename would misalign the
                                    // whole virtualized scroll.
                                    ui.add(egui::Label::new(name).truncate());
                                    if has_meta {
                                        ui.add(egui::Label::new(meta).truncate());
                                    }
                                });
                            });
                        })
                        .response
                        .interact(egui::Sense::click());
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    let fill = if is_sel {
                        Some(colors.sel_bg)
                    } else if is_multi {
                        Some(colors.sel_bg_dim)
                    } else if resp.hovered() {
                        Some(hover_fill)
                    } else {
                        None
                    };
                    if let Some(fill) = fill {
                        // The thumbnail's radius plus the pad it stands inside
                        // the plate: the two corners run parallel.
                        let rect = resp.rect.expand2(egui::vec2(ROW_PAD, 0.0));
                        ui.painter().set(plate, egui::Shape::rect_filled(rect, RADIUS_MD, fill));
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

        #[cfg(test)]
        {
            if let Some((slot, draw)) = thumb_geom {
                self.gallery_slot_rect = Some(slot);
                self.gallery_thumb_rect = Some(draw);
            }
        }
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
