//! App actions: open/variants/versions/history/quit flow.

use super::*;

use autoshop::advisor::{hint_action, FitAction};
use autoshop::recipe::MaskRole;

/// The persisted develop state belonging to the card that is on the canvas.
///
/// `recipe.json` / `pixels.json` describe whichever card was active at the
/// last save; a different persisted card gets its baseline from the matching
/// `variants.json.others[]` entry. Keeping this pair behind one owner prevents
/// a card switch from being mistaken for an edit by one of the four unsaved
/// consumers.
pub(crate) struct ActiveBaseline<'a> {
    pub(crate) recipe: &'a EditRecipe,
    pub(crate) origin: Option<&'a std::path::Path>,
}

struct PersistedCard<'a> {
    position: usize,
    kind: VariantKind,
    id: Option<&'a str>,
    name: Option<&'a str>,
    recipe: &'a EditRecipe,
    origin: Option<&'a std::path::Path>,
}

impl AutoshopApp {
    /// The panel's Strength dial as the typed axis every consumer reads: the
    /// develop request (R23-3) and the reverse-fit honesty budget (F1) share
    /// ONE reading, so the two cannot drift apart.
    pub(crate) fn panel_strength(&self) -> autoshop::recipe::GradeStrength {
        autoshop::recipe::GradeStrength::new(self.grade_strength)
    }

    /// Restore persisted prefs (last folder, view mode, export options) and
    /// re-open the library the user was browsing. Window geometry itself is
    /// restored by eframe's own persistence layer.
    pub(crate) fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // The REAL event-loop context replaces default()'s headless one:
        // workers wake the loop through this clone (see `spawn_worker`).
        let mut app = Self { egui_ctx: cc.egui_ctx.clone(), ..Self::default() };
        if let Some(prefs) =
            cc.storage.and_then(|s| eframe::get_value::<Prefs>(s, eframe::APP_KEY))
        {
            app.style_strength = prefs.style_strength.clamp(0.0, 1.0);
            app.grade_strength = prefs.grade_strength.clamp(0.0, 1.0);
            app.send_style_ref_image = prefs.send_style_ref_image;
            app.deep_think = prefs.deep_think;
            app.style_embed = prefs.style_embed;
            app.style_describe = prefs.style_describe;
            // Only a folder that still EXISTS is prefilled: a picker opened at
            // a deleted path lands wherever the OS decides (the same rule the
            // gallery restore above follows).
            app.style_src_dir = prefs.style_src_dir.clone().filter(|d| d.is_dir());
            app.looks_src_dir = prefs.looks_src_dir.clone().filter(|d| d.is_dir());
            app.use_looks = prefs.use_looks;
            app.direction_adherence = autoshop::recipe::DirectionAdherence::new(prefs.direction_adherence).get();
            app.exp_format = ExportFormat::from_pref(prefs.exp_format, prefs.save_jpeg);
            app.exp_dest = ExportDest::from_pref(prefs.exp_dest);
            app.last_export_dir = prefs.last_export_dir.clone();
            app.save_denoise = prefs.save_denoise;
            app.zoned_fit = prefs.zoned_fit;
            app.zoned_four_regions = prefs.zoned_four_regions;
            app.fit_ai_judge = prefs.fit_ai_judge;
            app.fit_deep = prefs.fit_deep;
            app.reimagine_retry = prefs.reimagine_retry;
            app.view_mode = prefs.view_mode;
            app.exp_long_edge = prefs.exp_long_edge;
            app.exp_sharpen = prefs.exp_sharpen.clamp(0.0, 100.0);
            app.exp_quality = prefs.exp_quality.clamp(1.0, 100.0);
            app.show_clipping = prefs.show_clipping;
            // Restore the UI language (an older save without this key decoded to
            // `Lang::En` via `#[serde(default)]`, so this is always valid).
            app.lang = prefs.lang;
            // Restore the theme and re-apply it — main() installed the Dark
            // default before prefs were readable (same shape as the greeting).
            app.theme = prefs.theme;
            install_theme(&cc.egui_ctx, app.theme);
            // The greeting was set by default() BEFORE the language was known —
            // re-issue it, or a Chinese install opens with one English line.
            app.status =
                tr(app.lang, "Open a photo, or open a folder to browse your library.").into();
            // Only known color spaces — an out-of-range pref falls back to sRGB.
            if prefs.exp_space <= 2 {
                app.exp_space = prefs.exp_space;
            }
            // Only the known steps — a corrupt pref must not produce a 1-px
            // or 100-MP working preview.
            if [1280, 2560, 4096].contains(&prefs.preview_edge) {
                app.preview_edge = prefs.preview_edge;
            }
            if let Some(dir) = prefs.gallery_dir.filter(|d| d.is_dir()) {
                app.open_folder(dir);
            }
        }
        app
    }

    /// L12#3: disclose gallery names whose script no installed font can
    /// draw — once per script per session. Runs over the CURRENT gallery
    /// (folder-open granularity); pure classification, no file I/O.
    pub(crate) fn disclose_undrawable_names(&mut self) {
        let installed = installed_scripts();
        let stems: Vec<String> = self
            .gallery
            .iter()
            .map(|p| autoshop::pipeline::stem(p).to_string())
            .collect();
        for stem in &stems {
            for (script, sample) in undrawable_scripts(stem, installed) {
                if self.disclosed_scripts.insert(script) {
                    let t = trf(
                        self.lang,
                        "some file names use characters no installed font can draw ({sample}) — they show as boxes",
                        &[("sample", &sample.to_string())],
                    );
                    self.toast(ToastKind::Error, t.clone());
                    self.status = t;
                }
            }
        }
    }

    pub(crate) fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        let text = text.into();
        // An identical LIVE toast refreshes instead of duplicating: undoing
        // through a washed history region fires the same repair disclosure
        // once per healed step, and five copies evicted still-live ERROR
        // toasts from the ring. The refresh MOVES it to the back — it is
        // the newest content, and refreshing in place left it first in
        // line for eviction, the exact loss the dedup exists to prevent.
        if let Some(i) = self.toasts.iter().position(|t| t.text == text && t.kind == kind) {
            self.toasts.remove(i);
        }
        self.toasts.push(Toast { text, kind, born: Instant::now() });
        if self.toasts.len() > 5 {
            self.toasts.remove(0); // keep the stack readable
        }
    }

    /// A worker finished successfully: status line + a success toast, unbusy.
    pub(crate) fn done(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.status = text.clone();
        self.toast(ToastKind::Success, text);
        self.busy = false;
    }

    /// A worker failed: status line + a lingering error toast, unbusy. A single
    /// status line is too easy to miss for a failed export or API call.
    pub(crate) fn fail(&mut self, what: &str, e: impl std::fmt::Display) {
        // `{e:#}`, not `{e}`: anyhow's alternate Display prints the WHOLE
        // chain ("mask refine failed: open image X: The image format could not
        // be determined" instead of just the outermost line). Every `.context`
        // this app attaches — the file, the stage, the cause — was being
        // dropped exactly where the user needed it, in the toast.
        let text = format!("{what}: {e:#}");
        self.status = text.clone();
        self.toast(ToastKind::Error, text);
        self.busy = false;
    }

    /// Arm the cancellable-worker state for a retouch/generative task: a fresh
    /// cancel flag (this shows the status-bar ✕) and the epoch its result must
    /// present to be applied. Streaming generative workers also hand the flag
    /// to the lib so the download itself stops at the next event; local
    /// compute is abandon-only (its late result is discarded by epoch).
    pub(crate) fn arm_cancel(&mut self) -> (u64, Arc<std::sync::atomic::AtomicBool>) {
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.gen_cancel = Some(flag.clone());
        (self.gen_epoch, flag)
    }

    /// The status-bar ✕: stop the running retouch/generative task. The UI
    /// unblocks NOW; the worker sees the flag at its next checkpoint and its
    /// late result arrives under a stale epoch — discarded, never applied.
    pub(crate) fn cancel_generative(&mut self) {
        if let Some(flag) = self.gen_cancel.take() {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
            self.gen_epoch = self.gen_epoch.wrapping_add(1);
            self.busy = false;
            // Honest about BOTH shapes. A generative worker polls the flag
            // between stream events and really does stop; `produce_recipe`
            // takes no flag at all, so an Analyze keeps running — and keeps
            // billing — until it answers or hits its deadline. Claiming a
            // checkpoint that does not exist for the path the user is most
            // likely cancelling is the kind of false status this project
            // treats as a defect.
            self.status = tr(
                self.lang,
                "cancelled — the app is free again and the late result is discarded; a generative call stops at its next checkpoint, while an AI analyze keeps running (and billing) until it finishes or times out",
            )
            .into();
        }
    }

    pub(crate) fn open_path(&mut self, path: PathBuf) {
        if self.busy {
            // Defence-in-depth: since the px combo refuses BEFORE arming, no
            // live caller reaches this guard with keep_recipe set — but a
            // stale request here once misclassified a genuine cross-photo
            // open as a same-path keep-flight, and any future caller that
            // arms-then-calls gets the same protection instead of relying on
            // its own busy check. Said so, instead of claiming a live
            // scenario.
            self.keep_recipe = false;
            // Refusal must be visible (the switch_variant rule: a silent
            // no-op reads as a dead UI) — the EMPTY-STATE picker button has
            // no busy gate, so this arm IS user-reachable (L11-4).
            let t = tr(self.lang, "busy — the photo opens when the current task finishes");
            self.toast(ToastKind::Error, t);
            return;
        }
        // Flush every typed-but-uncommitted name before the stash below
        // snapshots the strip — a thumbnail click / Ctrl+O / arrow-key nav
        // with a name box focused silently stashed the OLD name (U10). Each
        // buffer carries the photo it was seeded on, so the commits land on
        // the OUTGOING photo even though `src_path` moves below.
        self.commit_pending_names();
        // …and the buffers themselves die with the photo (M15): carried
        // across, a same-index mask / same-number version / same-card rename
        // box on the NEXT photo skipped the reseed and greeted it pre-filled
        // with the previous photo's text.
        self.mask_name_buf = None;
        self.version_name_buf = None;
        self.variant_name_buf = None;
        // An armed strip ✕ dies with the photo too — the index it named
        // belongs to a strip that is about to be replaced (R24-4).
        self.variant_delete_confirm = None;
        // The mid-open window marker (see confirm_quit_layer): src_path is
        // re-pointed below while recipe/saved_recipe still describe the old
        // photo; cleared in BOTH Msg::Opened arms. `open_same_path` is the
        // FACT the repair gates key on — keep_recipe is only a request, and
        // a stale request must never classify a flight.
        self.open_in_flight = true;
        self.open_same_path = self.src_path.as_ref() == Some(&path);
        // Leaving a photo with unsaved edits: stash the canvas for this
        // session so navigation can never silently destroy work (arrow keys /
        // thumbnail clicks used to drop the whole develop). Ctrl+S still owns
        // disk; returning to the photo restores the stash over the sidecar.
        // "Unsaved" covers PIXELS too: a baked retouch whose master isn't the
        // one recorded in the store's pixels.json would otherwise vanish from
        // the canvas on the way back even though its raster is on disk.
        // SAME-path reopens stash too (no `old != path` gate): clicking the
        // current thumbnail / re-picking the current file takes the fresh
        // branch below, which resets per-photo state and restores stash-then-
        // disk — without the entry, a dirty canvas was silently replaced by
        // the disk state. The stash entry is consumed by that same restore,
        // so a same-path reopen becomes "reload, preserving unsaved work".
        if let Some(old) = self.src_path.clone() {
            let origin = self.active_variant().and_then(|v| v.origin.clone());
            // Background variants count too (H4): their unsaved work has no
            // sidecar to survive in, and the strip used to collapse to the
            // active canvas alone on navigation. THIS photo's strip only:
            // the cross-photo sum (inactive_dirty_variants) belongs to the
            // quit dialog, and using it here chain-stashed clean photos.
            if self.nav_stash_gate_dirty() {
                let others: Vec<StashedVariant> = self
                    .variants
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != self.active)
                    .map(|(_, v)| StashedVariant {
                        kind: v.kind,
                        // Hop 3 of 6 (R24-2): identity + name ride the stash
                        // out; the workers restore is hop 4 back.
                        id: v.id.clone(),
                        name: v.name.clone(),
                        recipe: v.recipe.clone(),
                        base: v.base.clone(),
                        origin: v.origin.clone(),
                    })
                    .collect();
                let active_pos = self.active.min(others.len());
                self.nav_stash.insert(
                    old,
                    StashEntry {
                        recipe: self.recipe.clone(),
                        base: self.active_variant().and_then(|v| v.base.clone()),
                        origin,
                        kind: self.active_variant().map_or(VariantKind::Original, |v| v.kind),
                        // The ACTIVE card's identity + name travel with the
                        // rest of its state (R24-2): a nav round-trip that
                        // re-minted the id would orphan every version taken
                        // from this card.
                        id: self
                            .active_variant()
                            .map_or_else(|| ORIGINAL_VARIANT_ID.to_string(), |v| v.id.clone()),
                        name: self.active_variant().and_then(|v| v.name.clone()),
                        others,
                        active_pos,
                    },
                );
            }
        }
        let lang = self.lang;
        self.busy = true;
        // Every open starts resolved-until-proven-otherwise; the Opened
        // handler's Unreadable arm re-arms it for THIS photo when the read
        // fails.
        self.open_unresolved = false;
        self.src_path = Some(path.clone());
        // The version list describes the photo `src_path` NAMES, so it dies
        // with the old value of that field — not later, when the decode lands.
        // `refresh_versions` only runs from the completed `Msg::Opened`, and a
        // RAW decode is seconds; for that whole window the Versions panel
        // showed photo A's v1/v2/v3 with live buttons while every action
        // resolved `self.src_path`, which already pointed at photo B. Clicking
        // 🗑 on the stale list called `store::delete_version(B, n)` and
        // PERMANENTLY removed a snapshot of B — recipe and frozen mask
        // rasters, no backup, no undo — then refreshed the list from B so the
        // rows visibly changed and the status line confirmed a delete, hiding
        // what had happened. "＋ Save as version" was the mirror image: it
        // wrote A's canvas into B's develop as B's next version.
        self.versions.clear();
        // …and so does the armed "overwrite the .xmp beside the photo" confirm
        // (R22-8): it is a claim about THIS photo's neighbouring sidecar, and
        // carrying it across would let one click overwrite the NEXT photo's
        // Lightroom sidecar with no warning at all. Same reasoning as the
        // version list above.
        self.xmp_beside_confirm = false;
        self.status = trf(lang, "decoding {path} …", &[("path", &path.display().to_string())]);
        // Working-preview size is a user choice now (gap batch E): 1280 keeps
        // sliders fluid; 2560/4096 trade tick latency for real 1:1 detail when
        // checking focus / noise.
        let edge = self.preview_edge.clamp(640, 8192);
        // Decoded-base LRU: gallery culling flips between neighbours constantly,
        // and a hit skips the multi-second full demosaic entirely. Delivered
        // through the ordinary Msg::Opened channel so the handler stays the one
        // place that resets per-photo state.
        if let Some(hit) = self.cached_base(&path, edge) {
            let _ = self.tx.send(Msg::Opened(Box::new(Ok(hit))));
            return;
        }
        self.spawn_worker(
            move || {
                // Full-frame commit budget (budget.rs): the demosaic transient
                // pays the corpus peak whatever `edge` caps afterwards.
                let _mem = crate::budget::heavy_permit(crate::budget::estimate_mb(Some(&path)));
                // Build a CLEAN preview base by developing the RAW sensor data
                // (downscaled), NOT the camera's already-baked 8-bit JPEG preview:
                // re-developing that double-processes it and amplifies its grain when
                // you push tone/clarity. Baked images (PNG/TIFF/JPEG) are their own
                // source. Demosaic is slow, so this runs off the UI thread.
                let res = (|| -> anyhow::Result<OpenedBase> {
                    // The LRU key, captured BEFORE any read (the curve-memo
                    // rule): filing the decode under completion-time state
                    // handed a mid-open replacement the previous content's
                    // pixels and knots under its own key, and a popup-driven
                    // px change mid-flight mislabelled the edge.
                    let src_ident = (edge, file_stamp(&path), pixel_identity(&path));
                    let (thumb, knots, lens, as_shot) = if autoshop::decode::is_raw(&path) {
                        // Identity BEFORE the read: the primed answer below
                        // is filed under the content it was computed FROM —
                        // stat'ing after the multi-second develop filed a
                        // mid-open replacement's NEW identity with the OLD
                        // content's answer.
                        let ident = autoshop::pipeline::curve_ident(&path);
                        // Developed AT the working edge (the cap runs before
                        // tone/geometry): opening a 61 MP RAW no longer keeps
                        // a full-resolution develop resident just to be
                        // thumbnailed — the knots estimate below is CDF
                        // statistics and reads the same at working res.
                        let full = autoshop::render::render_to_image(
                            &path,
                            &EditRecipe::default(),
                            None,
                            Some(edge),
                        )?;
                        // In-camera lens profile (Sony 0x70xx): one cheap TIFF
                        // parse, stamped all-available-on (the user default).
                        let lens = autoshop::pipeline::fresh_lens_profile(&path);
                        // As-shot WB anchor: one metadata-only decode (no
                        // demosaic) — the Temp slider then speaks absolute
                        // Kelvin for this photo.
                        let as_shot = autoshop::render::as_shot_wb(&path);
                        // Camera-matched base look: CDF-match the neutral
                        // develop against the camera's own rendition — with
                        // the profile VIGNETTE applied first: the camera JPEG
                        // already contains that correction, and matching the
                        // uncorrected neutral would bake the corner lift into
                        // the global curve a second time. Geometry moves
                        // pixels, not their histogram — skipped on purpose.
                        // A RAW with no embedded preview (or a decode hiccup)
                        // just opens without a base look — never a failure.
                        let knots = match autoshop::decode::embedded_preview(&path) {
                            Ok(Some(cam)) => {
                                let est = autoshop::render::estimation_base(&full, &lens);
                                match autoshop::render::camera_base_knots(&est, &cam) {
                                    // The ANSWER primes the repair memo: the
                                    // open already paid this develop, and
                                    // discarding the result made every later
                                    // repair site (Ctrl+Z, variant switch,
                                    // Ctrl+S) pay a second full decode on
                                    // the UI thread. Answers only — an
                                    // inability stays uncached so the next
                                    // reader retries.
                                    Some(k) => {
                                        autoshop::pipeline::prime_curve_memo(
                                            &path,
                                            ident,
                                            k.clone(),
                                        );
                                        k
                                    }
                                    // None = could not judge. For an OPEN
                                    // that is the same as no base look — the
                                    // repair path is where the distinction
                                    // decides whether a SAVED curve may be
                                    // replaced.
                                    None => Vec::new(),
                                }
                            }
                            _ => Vec::new(),
                        };
                        // render_to_image already developed AT the working edge
                        // — `full` IS the thumb. The unconditional
                        // `full.thumbnail(edge, edge)` here duplicated a
                        // ~45 MP frame at the 8192 edge just to copy it; the
                        // resize only runs in the defensive off-spec case.
                        let thumb = if full.width().max(full.height()) > edge {
                            full.thumbnail(edge, edge)
                        } else {
                            full
                        };
                        (thumb, knots, lens, as_shot)
                    } else {
                        (
                            // baked-by-construction: the !is_raw arm of the open (branch above).
                            autoshop::decode::load_image(&path)?.thumbnail(edge, edge),
                            Vec::new(),
                            Default::default(),
                            None,
                        )
                    };
                    // The saved develop's baked pixel master (in-place heal /
                    // clone / fill / denoise), when the store records one and
                    // it still resolves — decoded HERE so the UI thread never
                    // loads a 61 MP PNG. A missing/unreadable master degrades
                    // to the plain source open, never a failure.
                    let mut baked_note = None;
                    let baked = autoshop::store::read_pixel_source(&path).and_then(
                        |(origin, generated)| {
                            // baked-by-construction: the store's baked pixel master (a PNG we wrote).
                            let img = match autoshop::decode::load_image(&origin) {
                                Ok(i) => i,
                                Err(e) => {
                                    // Disclosed at LANDING too, not only in a
                                    // log the windowed build cannot show
                                    // (L11-3). Typed detail; on_opened
                                    // localizes.
                                    eprintln!(
                                        "⚠ baked master {} failed to decode ({e}) — opening the un-retouched source",
                                        origin.display()
                                    );
                                    baked_note =
                                        Some(format!("{}: {e:#}", origin.display()));
                                    return None;
                                }
                            };
                            Some((Arc::new(img.thumbnail(edge, edge)), origin, generated))
                        },
                    );
                    // Arc once here so every downstream sharer (variants, the
                    // preview worker) is an O(1) refcount bump, not a deep copy.
                    Ok((Arc::new(thumb), knots, lens, as_shot, baked, src_ident, baked_note))
                })();
                Msg::Opened(Box::new(res))
            },
            |e| Msg::Opened(Box::new(Err(e))),
        );
    }

    /// Decoded-base LRU lookup (most-recent entry kept last). Missing metadata
    /// (mtime `None`) matches itself, so the cache still works where mtime is
    /// unavailable — it just loses the staleness guard there.
    pub(crate) fn cached_base(&mut self, path: &std::path::Path, edge: u32) -> Option<OpenedBase> {
        let mtime = file_stamp(path);
        let pixels = pixel_identity(path);
        let pos = self.base_cache.iter().position(|((p, e, t, px), _)| {
            p == path && *e == edge && *t == mtime && *px == pixels
        })?;
        let entry = self.base_cache.remove(pos);
        let hit = entry.1.clone();
        self.base_cache.push(entry);
        Some(hit)
    }

    /// Remember a freshly decoded base. The KEY comes from the worker's
    /// pre-read capture (`opened.5`), never from a stat or `preview_edge`
    /// here: the decode takes seconds, an egui dropdown popup outlives
    /// add_enabled_ui(!busy) (so the px combo CAN change mid-flight), and
    /// filing the pixels under completion-time state handed a mid-open
    /// replacement the previous content's pixels AND knots under its own
    /// key — the curve memo's identity-before-read rule, applied to its
    /// missed twin.
    pub(crate) fn remember_base(&mut self, path: &std::path::Path, opened: &OpenedBase) {
        let (edge, mtime, pixels) = opened.5.clone();
        self.base_cache.retain(|((p, e, _, _), _)| !(p == path && *e == edge));
        self.base_cache.push(((path.to_path_buf(), edge, mtime, pixels), opened.clone()));
        if self.base_cache.len() > BASE_CACHE_CAP {
            self.base_cache.remove(0); // least-recent first
        }
    }

    /// Drop the open photo's decoded-base cache entries. Called whenever its
    /// BAKED pixel identity changes (a retouch lands, a save writes/clears
    /// pixels.json) — the source mtime doesn't move then, so the mtime guard
    /// alone would happily serve a hit with stale baked pixels.
    pub(crate) fn forget_open_base(&mut self) {
        if let Some(p) = self.src_path.clone() {
            self.base_cache.retain(|((q, _, _, _), _)| q != &p);
        }
    }

    /// Match a live card to a persisted card. Stable IDs are authoritative
    /// when both sides carry one; an id-less legacy side falls back to kind +
    /// strip position, mirroring [`Self::version_is_from`]'s ID-first rule.
    fn persisted_identity_matches(
        live: &Variant,
        live_position: usize,
        saved_kind: VariantKind,
        saved_position: usize,
        saved_id: Option<&str>,
    ) -> bool {
        if !live.id.is_empty()
            && let Some(id) = saved_id.filter(|id| !id.is_empty())
        {
            return live.id == id;
        }
        live.kind == saved_kind && live_position == saved_position
    }

    fn persisted_card_for(&self, live_position: usize) -> Option<PersistedCard<'_>> {
        let live = self.variants.get(live_position)?;
        let rec = match &self.saved_strip {
            Some(rec) => rec,
            None => {
                let trivial = self.variants.len() == 1
                    && live_position == 0
                    && crate::model::strip_is_trivial(
                        live.kind,
                        &live.id,
                        live.name.as_deref(),
                        0,
                    );
                return trivial.then_some(PersistedCard {
                    position: 0,
                    kind: VariantKind::Original,
                    id: Some(ORIGINAL_VARIANT_ID),
                    name: None,
                    recipe: &self.saved_recipe,
                    origin: self.pixels_on_disk.as_deref(),
                });
            }
        };

        if let Some(kind) = VariantKind::from_store_str(&rec.active_kind)
            && Self::persisted_identity_matches(
                live,
                live_position,
                kind,
                rec.active_pos,
                rec.active_id.as_deref(),
            )
        {
            return Some(PersistedCard {
                position: rec.active_pos,
                kind,
                id: rec.active_id.as_deref(),
                name: rec.active_name.as_deref(),
                recipe: &self.saved_recipe,
                origin: self.pixels_on_disk.as_deref(),
            });
        }

        rec.others.iter().enumerate().find_map(|(i, entry)| {
            let position = if i < rec.active_pos { i } else { i + 1 };
            let kind = VariantKind::from_store_str(&entry.kind)?;
            Self::persisted_identity_matches(
                live,
                live_position,
                kind,
                position,
                entry.id.as_deref(),
            )
            .then_some(PersistedCard {
                position,
                kind,
                id: entry.id.as_deref(),
                name: entry.name.as_deref(),
                recipe: &entry.recipe,
                origin: entry.origin.as_deref(),
            })
        })
    }

    /// The ONE owner for the persisted `(recipe, origin)` of the active card.
    /// A card created after the last save has no baseline and is therefore
    /// unsaved. With no strip record, only the trivial lone Original maps to
    /// `recipe.json` / `pixels.json`.
    pub(crate) fn active_baseline(&self) -> Option<ActiveBaseline<'_>> {
        let saved = self.persisted_card_for(self.active)?;
        Some(ActiveBaseline { recipe: saved.recipe, origin: saved.origin })
    }

    fn active_canvas_dirty(&self) -> bool {
        if self.src_path.is_none() || self.active_variant().is_none() {
            return false;
        }
        self.active_baseline().is_none_or(|saved| {
            dirty_vs(&self.recipe, saved.recipe)
                || !same_master_opt(
                    self.active_variant().and_then(|v| v.origin.as_deref()),
                    saved.origin,
                )
        })
    }

    /// Per-frame status-bar consumer of [`Self::active_baseline`].
    pub(crate) fn unsaved_marker_dirty(&self) -> bool {
        self.active_canvas_dirty()
    }

    /// Window-close consumer of [`Self::active_baseline`].
    pub(crate) fn quit_guard_open_dirty(&self) -> bool {
        self.active_canvas_dirty()
    }

    /// Navigation-stash consumer: active canvas plus this photo's strip work.
    pub(crate) fn nav_stash_gate_dirty(&self) -> bool {
        self.active_canvas_dirty() || self.open_dirty_variants() > 0
    }

    /// Quit-dialog `PendingSave` consumer: the same open-photo state as the
    /// navigation stash, without counting other photos' stash entries.
    pub(crate) fn pending_save_gate_dirty(&self) -> bool {
        self.active_canvas_dirty() || self.open_dirty_variants() > 0
    }

    /// How many persisted-card mismatches hold work that quitting would discard.
    ///
    /// Dirty means: a live card cannot be matched to the union of the saved
    /// active card (`recipe.json` / `pixels.json`) and `variants.json.others`,
    /// or a matched card differs in kind/name/develop, or a persisted card was
    /// deleted. IDs match first; id-less records fall back to kind + position.
    /// Which card is selected is navigation, not an edit. The OPEN photo's
    /// strip only — the per-photo half of
    /// [`Self::inactive_dirty_variants`]; the stash decision in `open_path`
    /// keys on THIS (summing other photos' stashed variants into the gate
    /// chain-stashed every clean photo the user merely visited).
    ///
    /// The pre-v0.22 rule compared each background variant against the
    /// ACTIVE canvas's `saved_recipe`/`pixels_on_disk`. Those describe a
    /// DIFFERENT variant, so any strip whose cards disagree in origin (every
    /// generate→fit flow) counted dirty forever, no save could clear it, and
    /// the quit dialog re-armed until 「Discard」.
    pub(crate) fn open_dirty_variants(&self) -> usize {
        if self.variants.is_empty() {
            return 0;
        }
        let Some(rec) = &self.saved_strip else {
            let active = self.active_variant();
            let trivial = self.variants.len() == 1
                && self.active == 0
                && active.is_some_and(|v| {
                    crate::model::strip_is_trivial(
                        v.kind,
                        &v.id,
                        v.name.as_deref(),
                        0,
                    )
                });
            let background = self.variants.len().saturating_sub(1);
            return if background > 0 {
                background
            } else if trivial {
                0
            } else {
                1
            };
        };

        let mut persisted = Vec::with_capacity(rec.others.len() + 1);
        if let Some(kind) = VariantKind::from_store_str(&rec.active_kind) {
            persisted.push(PersistedCard {
                position: rec.active_pos,
                kind,
                id: rec.active_id.as_deref(),
                name: rec.active_name.as_deref(),
                recipe: &self.saved_recipe,
                origin: self.pixels_on_disk.as_deref(),
            });
        }
        persisted.extend(rec.others.iter().enumerate().filter_map(|(i, entry)| {
            let kind = VariantKind::from_store_str(&entry.kind)?;
            Some(PersistedCard {
                position: if i < rec.active_pos { i } else { i + 1 },
                kind,
                id: entry.id.as_deref(),
                name: entry.name.as_deref(),
                recipe: &entry.recipe,
                origin: entry.origin.as_deref(),
            })
        }));

        let mut dirty = 0usize;
        for (position, live) in self.variants.iter().enumerate() {
            let Some(saved) = persisted.iter().find(|saved| {
                Self::persisted_identity_matches(
                    live,
                    position,
                    saved.kind,
                    saved.position,
                    saved.id,
                )
            }) else {
                dirty += 1;
                continue;
            };
            let develop_dirty = position != self.active
                && (dirty_vs(&live.recipe, saved.recipe)
                    || !same_master_opt(live.origin.as_deref(), saved.origin));
            if live.kind != saved.kind || live.name.as_deref() != saved.name || develop_dirty {
                dirty += 1;
            }
        }

        // Persisted cards with no live identity are deletions. Selection drift
        // never enters this comparison, so merely viewing another card stays
        // clean and reopening still returns to the last SAVED active card.
        dirty
            + persisted
                .iter()
                .filter(|saved| {
                    !self.variants.iter().enumerate().any(|(position, live)| {
                        Self::persisted_identity_matches(
                            live,
                            position,
                            saved.kind,
                            saved.position,
                            saved.id,
                        )
                    })
                })
                .count()
    }

    pub(crate) fn inactive_dirty_variants(&self) -> usize {
        // Photos NAVIGATED AWAY FROM count too. Since batch 47 a photo is
        // stashed when only a BACKGROUND variant is dirty, so the quit dialog
        // lists it. Save-all persists each stashed photo's strip record along
        // with its active canvas (v0.22), so these are savable — the count
        // deliberately stays conservative (every stashed background variant,
        // not only those differing from their photo's on-disk record: that
        // comparison would cost a disk read per stash entry per check).
        let stashed: usize = self.nav_stash.values().map(|st| st.others.len()).sum();
        self.open_dirty_variants() + stashed
    }

    /// The live strip as a persistable [`store::VariantsRecord`]: `None` when
    /// the strip is trivial ([`crate::model::strip_is_trivial`] — recipe.json
    /// plus pixels.json already say everything, and a record would be pure
    /// sidecar noise). Since R24-3 a lone Original card that carries a NAME or
    /// a minted identity is no longer trivial: the record is the only home
    /// either of those has.
    pub(crate) fn current_strip_record(&self) -> Option<autoshop::store::VariantsRecord> {
        let ak = self.active_variant().map_or(VariantKind::Original, |v| v.kind);
        if crate::model::strip_is_trivial(
            ak,
            self.active_variant().map_or("", |v| v.id.as_str()),
            self.active_variant().and_then(|v| v.name.as_deref()),
            self.variants.len().saturating_sub(1),
        ) {
            return None;
        }
        Some(autoshop::store::VariantsRecord {
            extra: Default::default(),
            v: 1,
            active_kind: ak.store_str().to_string(),
            active_pos: self.active,
            others: self
                .variants
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != self.active)
                .map(|(_, v)| autoshop::store::VariantEntry {
                    extra: Default::default(),
                    kind: v.kind.store_str().to_string(),
                    recipe: v.recipe.clone(),
                    origin: v.origin.clone(),
                    // Hop 1 of 6 (R24-2): the live strip's identities + names
                    // to disk. An EMPTY id is written as absent — the record
                    // must not claim an identity the card does not have.
                    id: variant_id_field(&v.id),
                    name: v.name.clone(),
                })
                .collect(),
            active_id: self.active_variant().and_then(|v| variant_id_field(&v.id)),
            active_name: self.active_variant().and_then(|v| v.name.clone()),
        })
    }

    /// Persist the open photo's strip beside recipe.json/pixels.json and
    /// advance the mirror. Trivial strip ⇒ clear (no noise file). An error
    /// leaves the mirror untouched, so the unsaved protection stays armed.
    /// Test-only since the single-generation commit (L03): production saves
    /// stage the strip INTO [`autoshop::store::commit_develop`] and advance
    /// the mirror on the commit's success — this remains the strip-half
    /// primitive the guard tests drive in isolation.
    #[cfg(test)]
    pub(crate) fn persist_strip(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        match self.current_strip_record() {
            Some(rec) => {
                autoshop::store::write_variants(path, &rec)?;
                self.saved_strip = Some(rec);
            }
            None => {
                autoshop::store::clear_variants(path)?;
                self.saved_strip = None;
            }
        }
        Ok(())
    }

    /// The active variant, if a photo is open.
    pub(crate) fn active_variant(&self) -> Option<&Variant> {
        self.variants.get(self.active)
    }

    /// The on-disk PIXEL SOURCE the active variant renders / retouches /
    /// exports FROM. Any variant whose pixels are baked into a ./out raster — a
    /// reimagine (Generated), OR an in-place fill/heal/clone on ANY variant —
    /// carries that full-resolution artifact in `origin` and renders from it;
    /// a pristine source-based variant (原片 / 反推 with no pixel retouch) has
    /// `origin = None` and renders from `src_path` (the RAW / loaded image)
    /// developed by the recipe. Retouch and export key off THIS — not raw
    /// `src_path` — so what exports matches what's on screen (WYSIWYG), never
    /// the untouched negative underneath a generated / retouched rendition.
    pub(crate) fn active_source_path(&self) -> Option<PathBuf> {
        match self.active_variant() {
            Some(v) => v.origin.clone().or_else(|| self.src_path.clone()),
            None => self.src_path.clone(),
        }
    }

    /// Is the active variant an AI-generated raster (look baked into pixels,
    /// not the recipe)? Such a variant has no parametric XMP representation —
    /// exporting a sidecar for it would be a lie; steer the user to 反推 first.
    ///
    /// Kept as an IDENTITY question rather than re-expressed through
    /// [`VariantKind::is_parametric`] (R24-1): several callers feed the
    /// answer straight into the two-valued `pixels.json` format flag, which
    /// must keep naming the kind it spells on disk.
    pub(crate) fn active_is_generated(&self) -> bool {
        self.active_variant().is_some_and(|v| v.kind == VariantKind::Generated)
    }

    /// Uniform failure disclosure for a foreground persist compound — the
    /// single mapper the NoWait develop-lock wrappers report through
    /// (arch item b: five hand-rolled copies had already drifted into
    /// wording bugs). TYPED: `WouldBlock` really is "another Autoshop
    /// process owns this develop right now", so it gets the caller's busy
    /// wording with its retry hint; any other error is real I/O and must
    /// not wear the busy costume — a full disk used to read as "another
    /// process is working on this photo".
    pub(crate) fn persist_postponed(
        &mut self,
        e: &std::io::Error,
        busy_key: &'static str,
        args: &[(&str, &str)],
    ) {
        let es = e.to_string();
        let mut a: Vec<(&str, &str)> = args.to_vec();
        a.push(("err", &es));
        let t = if e.kind() == std::io::ErrorKind::WouldBlock {
            trf(self.lang, busy_key, &a)
        } else {
            trf(self.lang, "not saved — a develop-store write failed: {err}", &a)
        };
        self.status = t.clone();
        self.toast(ToastKind::Error, t);
    }

    /// Make `self.active`'s recipe + base pixels the live working state and
    /// rebuild the before texture. Per-variant transient state (undo history,
    /// local selection, view) restarts — like a soft re-open; what persists is
    /// each variant's recipe + pixels. Shared by switch / push / delete.
    pub(crate) fn load_active(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        let Some(v) = self.variants.get(self.active) else { return };
        self.recipe = v.recipe.clone();
        let vkind = v.kind;
        let vbase = v.base.clone();
        let vorigin = v.origin.clone();
        self.rationale = self.recipe.rationale.clone();
        self.rationale_notes.clear(); // variant recipes carry no typed notes
        // A DISK-restored baked variant (cold strip restore) carries its
        // origin but no decoded base. The master is FULL resolution — a
        // 61 MP TIFF takes seconds — and this runs on the UI thread, so the
        // old inline decode froze the window on the first click into a
        // cold-restored card. Decode on a worker instead: until the pixels
        // land the canvas shows the source develop UNDER A DISCLOSURE (a
        // silent stand-in is the wrong image wearing the right card label),
        // and Msg::MasterLoaded installs by (photo, origin) identity, so a
        // delete/reorder mid-decode discards the late pixels. A re-click
        // while decoding just spawns a redundant decode whose install finds
        // `base` already filled — wasteful once, never wrong.
        if vbase.is_none()
            && let Some(o) = &vorigin
            && let Some(photo) = self.src_path.clone()
        {
            self.spawn_master_load(ctx, photo, o.clone());
        }
        // The strip's READER joins the repair rule: push_variant and
        // switch_variant sync the OUTGOING canvas into the strip, so a
        // washed Original displaced by an AI push comes back through HERE
        // when its card is clicked — no navigation, no stash, no save
        // involved. Memo-bounded; era-2 recipes short-circuit; a PIXEL-STATE
        // card is skipped like at every other ordering site (its curve is
        // empty by invariant — see `VariantKind::is_parametric`, R24-1).
        // Synchronous first-click cost: the same accepted class as the open
        // gate — one estimate per photo per process when it succeeds, and a
        // transient inability early-exits on the failed probe and retries on
        // the next read.
        if vkind.is_parametric()
            // ...and never while a CROSS-PHOTO open is in flight (src_path
            // already points at the INCOMING photo — the apply_step /
            // live-canvas override hazard); a same-path keep-flight stays
            // admitted. Unreachable today through this fn's callers — the
            // busy interlocks exclude an in-flight open during switch /
            // push / delete — so this is defence-in-depth, like the
            // apply_step gate beside it (whose own callers became
            // busy-gated later), and says so instead of claiming a live
            // scenario.
            && (!self.open_in_flight || (self.open_same_path && self.keep_recipe))
            && let Some(p) = self.src_path.clone()
            && autoshop::pipeline::repair_pre_era_base_curve(&p, &mut self.recipe).is_some()
        {
            // The strip entry follows the healed canvas (the Ctrl+S rule),
            // and the user is told BY TOAST: the dominant callers
            // (switch_variant, push_variant) overwrite self.status in the
            // same frame, so a status write here died before a single paint
            // — the exact defect the load_version site documented.
            if let Some(v) = self.variants.get_mut(self.active) {
                v.recipe = self.recipe.clone();
            }
            self.toast(
                ToastKind::Success,
                tr(
                    lang,
                    "camera base look re-estimated — this photo was saved by a version whose preview sampler ran bright, so its stored base look rendered too dark",
                )
                .to_string(),
            );
        }
        // Base pixels: the variant's own baked raster, else the shared source
        // neutral (Original / Fitted re-develop the same negative).
        let base = vbase.or_else(|| self.source_preview.clone());
        if let Some(base) = base {
            let (mw, mh) = base.dimensions();
            // Before compares under the canvas recipe's own base calibration
            // (empty for legacy recipes — deliberate; a v0.24.0+ fit recipe
            // CARRIES its calibration, so a Fitted variant's Before now shows
            // the camera look instead of the 0.6–1.4 EV dark neutral). Covers
            // BAKED variants too: an InPlace master is a NEUTRAL develop whose
            // recipe keeps its curve (Before must show the camera look on the
            // healed pixels, not the dark neutral), while a Generated
            // variant's recipe carries an empty curve by construction — the
            // same expression serves both.
            let curve = self.recipe.base_curve.clone();
            self.set_before(ctx, &base, &curve);
            self.base_preview = Some(base);
            // A fresh transparent paint mask sized to THIS base (a generated
            // raster and the source neutral can differ in dimensions).
            self.mask_paint = Some(image::RgbaImage::new(mw, mh));
            self.mask_tex = None;
            self.mask_dirty = false;
            self.paint_last = None;
        }
        self.reset_history(); // you can't undo across variants
        self.region = None;
        self.region_drag = None;
        self.sel_mask = None;
        self.sel_component = None;
        // Same boundary rule as open_path: the rename buffer belongs to the
        // variant it was typed on (M15). The strip's own rename box is keyed
        // by CARD ID, not by what is on screen, so it deliberately survives
        // a switch — but an armed ✕ does not: it named an index in the strip
        // the user has just moved away from (R24-4).
        self.mask_name_buf = None;
        self.variant_delete_confirm = None;
        self.overlay_ref = None;
        self.overlay_stale = true;
        self.last_rgb = None; // the retained frame belongs to the OLD variant
        // …and so does the After texture (L11-2): its only writers are the
        // develop/retouch landings, so until the new variant's first frame
        // lands the canvas showed the OLD variant's pixels under the NEW
        // variant's controls — seconds, on a large photo. The honest
        // "Preparing preview…" placeholder takes over.
        self.after_tex = None;
        self.disarm_tools();
        self.clone_src = None; // unlike a mere disarm, a variant switch drops the sample
        self.zoom = 1.0;
        self.zoom_target = 1.0; // instant — a swap must not glide from the old view
        // The `1` key reads LAST frame's cached 1:1 solve; stale across a
        // switch it jumped the new canvas to the old variant's ratio (L11-5).
        self.zoom_one_to_one = 1.0;
        self.pan = egui::vec2(0.5, 0.5);
        self.verdict = None;
        self.dirty = true; // re-develop the newly active variant
    }

    /// Switch the active variant losslessly (strip click): in-flight slider
    /// edits are saved back into the variant you leave, then the target's
    /// recipe + pixels become current.
    pub(crate) fn switch_variant(&mut self, idx: usize, ctx: &egui::Context) {
        if idx == self.active || idx >= self.variants.len() {
            return;
        }
        if self.busy {
            // Refusal must be visible — the card stays clickable and a silent
            // no-op reads as a dead UI.
            let t = tr(self.lang, "busy — variants unlock when the current task finishes");
            self.toast(ToastKind::Error, t);
            return;
        }
        // Flush every typed-but-uncommitted name INTO the snapshot below
        // (L11-1) — the same boundary rule open_path and Ctrl+C already
        // apply: commit first, then load_active's M15 clear drops the
        // buffer. Clearing without committing silently lost the rename.
        self.commit_pending_names();
        // Read BEFORE the &mut borrow below. `dirty` = an edit awaiting
        // dispatch; `develop_inflight` = a frame the switch is about to make
        // the acceptance gate reject. Either way no completed frame depicts
        // the recipe snapshotted below.
        let no_frame_landed = self.dirty || self.develop_inflight;
        if let Some(cur) = self.variants.get_mut(self.active) {
            cur.recipe = self.recipe.clone(); // don't lose the edits in progress
            // …and don't keep a CARD that predates them: a background
            // variant's thumb has exactly ONE writer (finish_redevelop,
            // active-only), so an edit that has not landed a frame yet never
            // will, and the card would advertise the pre-edit rendition for
            // the rest of the session (L06#6). The strip's honest "…"
            // placeholder takes over — the same remedy as the healed-sibling
            // card drop in export.rs; switching back re-develops and refills.
            if no_frame_landed {
                cur.thumb = None;
            }
        }
        self.active = idx;
        self.load_active(ctx);
        let lang = self.lang;
        let name = tr(lang, self.variants[self.active].kind.label());
        // A baked variant keeps the resolution it was baked at: switching
        // cannot re-decode a master, so the preference and the canvas can
        // legitimately disagree from here on. Said at the moment the
        // disagreement is created — the combo alone would imply the canvas
        // followed it.
        // (Both arms, like every canvas door — see `set_canvas_status`; this
        // one names the variant, so it writes its own pair.)
        self.status = match self.baked_canvas_edge() {
            None => trf(
                lang,
                "Switched to variant「{name}」 — variants are independent, switching is lossless",
                &[("name", name)],
            ),
            Some(px) => trf(
                lang,
                "Switched to variant「{name}」 — its baked pixels stay at {px}px (their own bake); edits and retouches follow that, not the preview preference",
                &[("name", name), ("px", &px.to_string())],
            ),
        };
    }

    /// Append a variant and switch to it (its recipe/pixels become live).
    /// Saves the outgoing variant's edits first so nothing is lost.
    pub(crate) fn push_variant(&mut self, v: Variant, ctx: &egui::Context) {
        // "Nothing is lost" includes a rename still sitting in its TextEdit:
        // both callers are ASYNC completions (reverse-fit, generative edit)
        // that switch variants while the user may be mid-typing, and the
        // switch's M15 boundary clear then discarded the typed name.
        // (switch_variant now flushes the same way — L11-1 — so every
        // variant boundary commits before the M15 clear.)
        self.commit_pending_names();
        // Same guarded card drop as switch_variant (L06#6): the async
        // completion switches away from a variant whose latest edits may
        // never have landed a frame.
        let no_frame_landed = self.dirty || self.develop_inflight;
        if let Some(cur) = self.variants.get_mut(self.active) {
            cur.recipe = self.recipe.clone();
            if no_frame_landed {
                cur.thumb = None;
            }
        }
        self.variants.push(v);
        self.active = self.variants.len() - 1;
        self.load_active(ctx);
    }

    /// Remove variant `idx` (never the last one — the strip stays non-empty).
    /// Only reloads the working state when the ACTIVE variant's identity moves,
    /// so deleting a background variant can't clobber live edits. Refuses while
    /// busy: an in-flight retouch worker resolves its target by `self.active` at
    /// COMPLETION time, so re-anchoring `active` mid-flight would bake its result
    /// onto the wrong variant.
    pub(crate) fn delete_variant(&mut self, idx: usize, ctx: &egui::Context) {
        if self.busy {
            let t = tr(self.lang, "busy — variants unlock when the current task finishes");
            self.toast(ToastKind::Error, t);
            return;
        }
        if self.variants.len() <= 1 || idx >= self.variants.len() {
            return;
        }
        // ARM, then act (R24-4). Deleting a card is IRREVERSIBLE in a way
        // deleting a VERSION is not the mirror of, and the asymmetry was
        // silent: undo history is per-variant by contract (`reset_history`
        // clears it at every strip move, and `UndoStep` holds one recipe +
        // one pixel identity, never the card list), no registry can bring a
        // card back the way `.deleted-versions.json` keeps a deleted version
        // deleted, and the strip is the only home a background variant's
        // recipe has until the next save. So the ✕ asks: the first click
        // arms this index (the button relabels itself and the status says
        // what the second click will do), the second deletes. The arm dies
        // with any other strip move — `load_active` clears it.
        if self.variant_delete_confirm != Some(idx) {
            self.variant_delete_confirm = Some(idx);
            let lang = self.lang;
            let name = self.variant_label(idx);
            let t = trf(
                lang,
                "Delete variant「{name}」? Click ✕ again to confirm — a deleted variant cannot be brought back (Ctrl+Z does not cross variants)",
                &[("name", &name)],
            );
            self.status = t.clone();
            self.toast(ToastKind::Error, t);
            return;
        }
        self.variant_delete_confirm = None;
        // A rename box aimed at the card being deleted dies WITH it — the
        // version rows' rule, on the strip's key.
        if self
            .variant_name_buf
            .as_ref()
            .is_some_and(|(_, id, ..)| Some(id.as_str()) == self.variants.get(idx).map(|v| v.id.as_str()))
        {
            self.variant_name_buf = None;
        }
        let active_removed = idx == self.active;
        self.variants.remove(idx);
        if self.active > idx {
            self.active -= 1; // same variant, shifted left
        }
        if self.active >= self.variants.len() {
            self.active = self.variants.len() - 1; // removed the tail
        }
        if active_removed {
            self.load_active(ctx); // the active variant changed identity
            // …onto a canvas the user did not choose: deleting the active
            // variant can land on a background BAKED one, whose raster the
            // preference cannot re-decode. Silent before — and it carried the
            // deleted canvas's own disclosure forward as if it still applied.
            self.set_canvas_status("variant removed");
        }
    }

    /// How card `idx` names itself in a sentence: the user's own name when
    /// it has one, else its localized kind label. Owned, so callers can hand
    /// it to `trf` while `self` is borrowed mutably.
    pub(crate) fn variant_label(&self, idx: usize) -> String {
        match self.variants.get(idx) {
            Some(v) => v.name.clone().unwrap_or_else(|| tr(self.lang, v.kind.label()).to_string()),
            None => String::new(),
        }
    }

    /// Where this photo's base negative sits in the strip — `None` when the
    /// strip has none. Not a hypothetical: a cold restore whose record named
    /// a `fitted` active card and listed no Original entry rebuilds exactly
    /// that, so every caller must say something honest instead of assuming
    /// card 0 is the negative.
    pub(crate) fn original_index(&self) -> Option<usize> {
        self.variants.iter().position(|v| v.kind == VariantKind::Original)
    }

    /// R24-3 (#7): copy card `idx`'s develop onto the ▣ Original CARD.
    ///
    /// Two different things the UI has to keep apart: this overwrites the
    /// Original card's develop PARAMETERS (its baked pixels and its raster
    /// origin stay exactly as they were — the card keeps being the same
    /// negative), and it is Ctrl+S that afterwards makes that card's develop
    /// the photo's saved develop (`recipe.json`). The SOURCE card survives —
    /// Lightroom's 「Set Copy as Original」 rule, and the user decision on
    /// record.
    ///
    /// Undo is ONE step, borrowed from the strip's own machinery: the switch
    /// lands the canvas on the Original first (which syncs the outgoing card
    /// and reseeds history with the Original's OWN develop as the head), the
    /// overwrite happens on that canvas, and `commit_now` pushes exactly one
    /// step — so Ctrl+Z restores the develop the Original had.
    ///
    /// A PIXEL-STATE source is refused: its look lives in its raster, and the
    /// canvas recipe over it is stripped of curve, lens and as-shot anchor by
    /// construction, so "applying" it would hand the negative an almost-empty
    /// develop. Same judgement (and same remedy) as Ctrl+S's XMP refusal and
    /// 「＋ Save as version」's.
    pub(crate) fn apply_to_original(&mut self, idx: usize, ctx: &egui::Context) {
        let lang = self.lang;
        if self.busy {
            let t = tr(lang, "busy — variants unlock when the current task finishes");
            self.toast(ToastKind::Error, t);
            return;
        }
        let Some(src) = self.variants.get(idx) else { return };
        if !src.kind.is_parametric() {
            let t = tr(
                lang,
                "A generated variant's look lives in its pixels — there are no develop parameters to copy onto the ▣ Original card; run 「Reverse-fit」 first",
            );
            self.status = t.into();
            self.toast(ToastKind::Error, t);
            return;
        }
        let Some(dst) = self.original_index() else {
            let t = tr(
                lang,
                "this photo's strip holds no ▣ Original card to apply onto",
            );
            self.status = t.into();
            self.toast(ToastKind::Error, t);
            return;
        };
        if dst == idx {
            return; // the negative is already itself
        }
        // A persistence-shaped action: every name box the user has open
        // commits first (U10 / the 10+1 boundary rule).
        self.commit_pending_names();
        // The develop to copy is the LIVE canvas when the source card is the
        // active one — its in-flight slider edits are what the user means by
        // "this look" — and the card's stored recipe otherwise. Captured
        // BEFORE the switch, which writes the canvas back into that card.
        let recipe =
            if idx == self.active { self.recipe.clone() } else { self.variants[idx].recipe.clone() };
        let name = self.variant_label(idx);
        self.switch_variant(dst, ctx);
        if self.active != dst {
            return; // the switch declined (it owns its own refusal message)
        }
        if let Some(o) = self.variants.get_mut(dst) {
            o.recipe = recipe.clone();
            // The card's thumbnail renders the develop we just replaced —
            // the cross-card write rule the strip-healing loop in export.rs
            // already follows. The strip's 「…」 placeholder takes over until
            // the next develop lands.
            o.thumb = None;
        }
        self.recipe = recipe;
        self.resync_recipe_display();
        self.dirty = true;
        self.commit_now();
        self.status = trf(
            lang,
            "「{name}」 copied onto the ▣ Original card (its pixels are untouched) — Ctrl+Z undoes it; Ctrl+S then saves it as this photo's develop",
            &[("name", &name)],
        );
    }

    /// Commit a pending VARIANT rename into the card it was typed on
    /// (R24-3) — the strip half of the `commit_mask_name_buf` rule (U10).
    ///
    /// Keyed by (photo, card id), both captured at seed time: a background
    /// push or delete renumbers strip INDICES while a box is open, so an
    /// index key would cross-commit onto another card (the version buffer's
    /// number key records the same lesson). Memory only — a card's name
    /// reaches disk with the strip record at the next save, which is why
    /// `open_dirty_variants` counts a renamed card as unsaved work.
    pub(crate) fn commit_variant_name_buf(&mut self) {
        let Some((photo, id, seed, buf)) = self.variant_name_buf.clone() else { return };
        if buf == seed {
            return;
        }
        if self.src_path.as_deref() != Some(photo.as_path()) || id.is_empty() {
            // The card belongs to a photo that is no longer open (its strip
            // went with it), or to no identifiable card at all: drop the
            // buffer rather than paint the name onto whatever sits here now.
            self.variant_name_buf = None;
            return;
        }
        // Capped HERE, on the way in, to the same byte limit the store applies
        // on the way out: a longer name would be truncated by
        // `variants_record_bytes` and the live card would then differ from its
        // own saved record for good — a photo permanently counted as unsaved
        // work. Popped char by char so the cut lands on a boundary.
        let mut named = buf.trim().to_string();
        while named.len() > autoshop::store::MAX_STORE_NAME {
            named.pop();
        }
        let named = Some(named).filter(|s| !s.is_empty());
        if let Some(v) = self.variants.iter_mut().find(|v| v.id == id) {
            v.name = named;
        }
        // Re-seed rather than clear (the version rule): the box is very
        // likely still on screen, and a SECOND rename in it has to compare
        // against what was just committed, not against the original.
        self.variant_name_buf = Some((photo, id, buf.clone(), buf));
    }

    /// Perform one deferred variant action — **the one owner** (R24-5).
    ///
    /// The card actions are drawn by two surfaces now (the bottom strip and
    /// the Versions section's edit-state list), and every one of them mutates
    /// `self` while the drawing code still borrows it, so both surfaces defer.
    /// Routing them through a single function is what keeps "what ＋ does"
    /// from being two answers: the strip's own five-way `else if` chain was
    /// already the shape this generalises, and copying it to a second caller
    /// is exactly how the two would drift.
    ///
    /// The arms are mutually exclusive by construction — [`VariantAction`] is
    /// one value — which preserves the chain's guarantee that one frame cannot
    /// switch a card AND delete it.
    pub(crate) fn dispatch_variant_action(&mut self, act: VariantAction, ctx: &egui::Context) {
        match act {
            VariantAction::Switch(i) => self.switch_variant(i, ctx),
            VariantAction::Delete(i) => self.delete_variant(i, ctx),
            VariantAction::Apply(i) => self.apply_to_original(i, ctx),
            VariantAction::Rename(id) => {
                // Opening a box COMMITS whichever one was open first — the
                // mask panel's cross-commit rule, made structural by keying on
                // the card's id (R24-3).
                self.commit_variant_name_buf();
                if let Some(photo) = self.src_path.clone() {
                    let cur = self
                        .variants
                        .iter()
                        .find(|v| v.id == id)
                        .and_then(|v| v.name.clone())
                        .unwrap_or_default();
                    self.variant_name_buf = Some((photo, id, cur.clone(), cur));
                }
            }
            // The buttons are already disabled while busy; the second read is
            // defence in depth against a future caller path, at the cost of
            // one bool. (`save_version` is the only arm that writes a file
            // straight from the live canvas.)
            VariantAction::SaveVersion => {
                if !self.busy {
                    self.save_version();
                }
            }
        }
    }

    /// Flush every typed-but-uncommitted NAME box — masks, versions, variant
    /// cards (U10; the N+1 boundary rule).
    ///
    /// ONE owner on purpose: ELEVEN production boundaries each hand-copying
    /// the growing list is how a box silently stops being flushed the day a
    /// third kind of name appears (R24-2 added the second by copying, R24-3
    /// the third). Every entry point that persists, navigates, snapshots or
    /// quits calls THIS; the "+1" is each box's own lost-focus commit — three
    /// of those, one per box, in `panels/develop.rs`. (Counts re-verified at
    /// the R24 round-end review; the doc had said ten.)
    pub(crate) fn commit_pending_names(&mut self) {
        self.commit_mask_name_buf();
        self.commit_version_name_buf();
        self.commit_variant_name_buf();
    }

    /// Every crop_mode flip goes through here (L13-1): `pan` is stored in
    /// the CURRENT window's coordinates — the committed crop normally, the
    /// full frame while the crop tool is open (`view_uv`) — so the VALUE
    /// must be rebased when the window changes. Left alone, entering the
    /// tool reinterpreted a crop-relative pan as full-frame and the
    /// viewport landed outside the crop box at higher zoom.
    pub(crate) fn set_crop_mode(&mut self, on: bool) {
        if self.crop_mode == on {
            return;
        }
        if let Some(c) = &self.recipe.crop {
            let (l, t) = (c.left.min(c.right), c.top.min(c.bottom));
            let (w, h) =
                ((c.right - c.left).abs().max(1e-6), (c.bottom - c.top).abs().max(1e-6));
            self.pan = if on {
                // crop-window coords → full-frame coords
                egui::vec2(l + self.pan.x * w, t + self.pan.y * h)
            } else {
                // full-frame coords → crop-window coords
                egui::vec2((self.pan.x - l) / w, (self.pan.y - t) / h)
            };
            // view_uv re-clamps against zoom each frame; keep it sane here.
            self.pan = egui::vec2(self.pan.x.clamp(0.0, 1.0), self.pan.y.clamp(0.0, 1.0));
        }
        self.crop_mode = on;
    }

    /// Recipe snapshot path for version `n` — `v<n>.recipe.json` in the photo's
    /// central develop dir (gap batch G, ≈ Lightroom virtual copies: cheap
    /// parametric versions, never touching the library or the working
    /// `recipe.json`).
    pub(crate) fn version_path(src: &std::path::Path, n: u32) -> PathBuf {
        autoshop::store::version_target(src, n)
    }

    /// Re-point every stored mask index (selection, colour-range sampler,
    /// redraw target) after the mask list changed shape — delete, drag-drop
    /// reorder, ⬆/⬇ swap. One place, so an index-carrying tool cannot be
    /// forgotten: a dangling index used to make the range sampler write into
    /// whatever mask slid under it.
    pub(crate) fn remap_mask_indices(&mut self, f: impl Fn(usize) -> Option<usize>) {
        let sel_before = self.sel_mask;
        self.sel_mask = self.sel_mask.and_then(&f);
        // Component selection is meaningful only while the SAME mask stays
        // selected under the same index-space; a moved/vanished mask drops it
        // (its component list travelled with the mask, but every canvas tool
        // re-derives from sel_mask — a stale pair here was the same class of
        // bug as the dangling range sampler).
        if self.sel_mask != sel_before || self.sel_mask.is_none() {
            self.sel_component = None;
        }
        self.range_picking = self.range_picking.and_then(&f);
        self.placing_mask = self.placing_mask.take().and_then(|(k, t)| match t {
            PlaceTarget::NewMask => Some((k, PlaceTarget::NewMask)),
            // A vanished redraw target degrades to a NEW mask placement (the
            // pre-v0.22 `Option<usize>` behaviour, kept); a vanished
            // component target must NOT — appending a component to nothing
            // has no sensible fallback, so the arm disarms.
            PlaceTarget::Redraw(i) => {
                Some((k, f(i).map_or(PlaceTarget::NewMask, PlaceTarget::Redraw)))
            }
            PlaceTarget::Component(i, mode) => {
                f(i).map(|j| (k, PlaceTarget::Component(j, mode)))
            }
        });
        // The pending-rename buffer is index-carrying too: after a delete /
        // reorder its seed-guard (name-at-seed must still match) correctly
        // refused to cross-commit — but the SAME mask at its new index then
        // failed the match as well, and the typed rename silently died.
        self.mask_name_buf =
            self.mask_name_buf.take().and_then(|(j, o, b)| Some((f(j)?, o, b)));
        // The mask-brush session carries its TARGET index: 「Apply」 bakes the
        // stroke into `mask_brush.0`, so after a delete/reorder it painted
        // whatever mask slid under the stale slot — the dangling-range-sampler
        // class again, writing pixel weights instead of range bounds. A moved
        // target follows its mask; a vanished one ends the session exactly
        // like Esc (gray buffer AND paint canvas — see disarm_tools: fill/heal
        // would inherit the strokes as a phantom retouch selection). Not a
        // new-mask fallback: that would resurrect a just-deleted mask.
        if let Some((target, erase)) = self.mask_brush {
            match target.map(&f) {
                None => self.mask_brush = Some((None, erase)),
                Some(Some(j)) => self.mask_brush = Some((Some(j), erase)),
                Some(None) => {
                    self.end_mask_brush();
                }
            }
        }
    }

    /// Rescan the photo's develop dir for version snapshots (cached in
    /// `self.versions`; called on photo open and after saving a version — NOT
    /// every frame). The advisory name/provenance sidecar is read in the same
    /// pass and RESTRICTED to numbers that are actually listed: a crash
    /// between a delete's sweep and its metadata drop can leave a record for
    /// a number nothing lists any more, and that record must never surface as
    /// a phantom row (it can never be re-attached either — the number is
    /// burned in the deleted-version registry).
    pub(crate) fn refresh_versions(&mut self) {
        self.versions.clear();
        self.version_meta.clear();
        let Some(src) = self.src_path.as_deref() else { return };
        // ONE store query for the photo's whole edit-state list (R24-4
        // `store::list_edits`), which joins the version family with its
        // advisory metadata — including the listed-numbers-only restriction
        // this used to spell for itself. The VARIANT half is deliberately
        // dropped here: it describes the LAST SAVED strip, while
        // `self.variants` is the live one (pushes, deletes and renames that
        // have not been saved yet), and an editor must never renumber its own
        // cards from a stale record. For every non-GUI surface that half IS
        // the list.
        for e in autoshop::store::list_edits(src) {
            if let autoshop::store::EditStateKind::Version { n, from_kind, from_id, source } =
                e.state
            {
                self.versions.push(n);
                self.version_meta.insert(
                    n,
                    autoshop::store::VersionMetaEntry {
                        n,
                        name: e.name,
                        from_kind,
                        from_id,
                        origin: source,
                    },
                );
            }
        }
    }

    /// Does version metadata `m` name the given variant as its source
    /// (R24-2)? The ID is authoritative when both sides have one — a card
    /// renamed or re-kinded since the snapshot still matches. KIND is the
    /// fallback for a record written before ids existed on this photo (or
    /// for a card whose own id was never recorded). A record with neither —
    /// or no record at all — is UNATTRIBUTED and matches nothing: the filter
    /// is "only this variant", and quietly including everything unknown
    /// would make it a checkbox that does not filter.
    pub(crate) fn version_is_from(
        m: Option<&autoshop::store::VersionMetaEntry>,
        active_id: &str,
        active_kind: Option<VariantKind>,
    ) -> bool {
        let Some(m) = m else { return false };
        if let Some(id) = m.from_id.as_deref()
            && !active_id.is_empty()
        {
            return id == active_id;
        }
        match (m.from_kind.as_deref(), active_kind) {
            (Some(k), Some(ak)) => k == ak.store_str(),
            _ => false,
        }
    }

    /// Commit a pending version rename to ITS OWN photo and number — the
    /// version half of the `commit_mask_name_buf` rule (U10): a name still
    /// sitting in its TextEdit when the user hits Ctrl+S, clicks another
    /// photo or quits must reach the disk, not die with the buffer. Keyed by
    /// (photo, number), both captured at seed time, so a boundary that has
    /// already re-pointed `src_path` cannot rename the incoming photo's v1.
    /// The write is advisory: a failure is disclosed and the buffer is left
    /// alone so the next boundary retries.
    pub(crate) fn commit_version_name_buf(&mut self) {
        let Some((photo, n, seed, buf)) = self.version_name_buf.clone() else { return };
        if buf == seed {
            return;
        }
        // NoWait wrapper around the store's own Wait lock (the delete_version
        // rule): a develop held by another process must report here, never
        // freeze the UI thread on a name.
        match autoshop::store::with_develop_lock(
            &photo,
            autoshop::store::DevelopLockMode::NoWait,
            || autoshop::store::set_version_name(&photo, n, Some(buf.as_str())),
        ) {
            Ok(()) => {
                let named = Some(buf.trim().to_string()).filter(|s| !s.is_empty());
                // Re-seed rather than clear: the row is very likely still on
                // screen, and a SECOND rename in the same row has to compare
                // against what we just wrote, not the original.
                self.version_name_buf = Some((photo.clone(), n, buf.clone(), buf));
                if self.src_path.as_deref() == Some(photo.as_path()) {
                    self.version_meta
                        .entry(n)
                        .or_insert_with(|| autoshop::store::VersionMetaEntry {
                            n,
                            ..Default::default()
                        })
                        .name = named;
                }
            }
            Err(e) => {
                self.persist_postponed(&e, "Renaming v{n} failed: {err}", &[("n", &n.to_string())]);
            }
        }
    }

    /// Save the CURRENT develop as the next numbered version snapshot.
    pub(crate) fn save_version(&mut self) {
        // A typed-but-uncommitted name belongs in the snapshot — every
        // persistence entry point flushes all of them (the U10 rule; L27).
        self.commit_pending_names();
        let lang = self.lang;
        // A PIXEL-STATE variant has no develop worth snapshotting: its look
        // lives in the raster, and the canvas recipe over it is stripped of
        // curve, lens and as-shot anchor by construction — so "＋ Save as
        // version" wrote a near-empty recipe that restored to nothing (R24-2).
        // Same judgement as Ctrl+S's XMP refusal (`save_xmp`), same remedy:
        // reverse-fit first, then the snapshot has parameters to hold.
        if self.active_is_generated() {
            let t = tr(
                lang,
                "A generated variant's look lives in its pixels — a version snapshot would store an almost-empty recipe; run 「Reverse-fit」 first",
            );
            self.status = t.into();
            self.toast(ToastKind::Error, t);
            return;
        }
        let Some(src) = self.src_path.clone() else { return };
        // Attribution for the record below, read before the lock closure
        // borrows `self`: which card this snapshot is a picture OF (R24-2).
        let from_kind = self.active_variant().map(|v| v.kind.store_str().to_string());
        let from_id = self.active_variant().and_then(|v| variant_id_field(&v.id));
        // ONE NoWait lock across claim → raster freeze → recipe write:
        // another process's delete_version sweeping the just-claimed slot
        // mid-snapshot is a real interleave — and this is a foreground
        // button, so busy must report, not hang the UI.
        let locked = autoshop::store::with_develop_lock(
            &src,
            autoshop::store::DevelopLockMode::NoWait,
            || -> std::io::Result<()> {
        // ATOMICALLY reserved number (store::claim_version): disk-list+1
        // raced the auto-backup (and the web/CLI) to the same N — the later
        // publish silently replaced the earlier snapshot.
        let (n, vpath) = match autoshop::store::claim_version(&src) {
            Ok(v) => v,
            Err(e) => {
                let t = trf(lang, "Save version failed: {err}", &[("err", &e.to_string())]);
                self.status = t.clone();
                self.toast(ToastKind::Error, t);
                return Ok(());
            }
        };
        // Freeze referenced rasters under THIS version (the backup gate's
        // pair): a version whose recipe pointed at another version's frozen
        // raster (load v3 → save as v4) broke the moment v3 was deleted.
        let dev = autoshop::store::develop_dir(&src);
        let mut snap = self.recipe.clone();
        if let Err(e) = autoshop::store::snapshot_rasters(&mut snap, &dev, n) {
            let _ = std::fs::remove_file(&vpath); // release the claim
            let t = trf(lang, "Save version failed: {err}", &[("err", &e.to_string())]);
            self.status = t.clone();
            self.toast(ToastKind::Error, t);
            return Ok(());
        }
        let res = autoshop::pipeline::write_recipe(&src, &snap, Some(vpath.clone()), autoshop::diag::stderr());
        if res.is_err() {
            // Release the claimed slot AND this call's frozen rasters — an
            // empty version must not pollute the list after a failed write.
            autoshop::store::rollback_frozen_rasters(&dev, n);
            let _ = std::fs::remove_file(&vpath);
        }
        match res {
            Ok(p) => {
                // Provenance BEFORE the rescan, so the fresh row already
                // carries「· 来自 …」. Best-effort by contract (R24-2): the
                // snapshot is on disk and complete — an advisory record that
                // could not be written must not turn a successful save into a
                // failure, it only leaves the version unattributed. The
                // develop lock is already held (reentrant).
                let _ = autoshop::store::record_version_meta(
                    &src,
                    &autoshop::store::VersionMetaEntry {
                        n,
                        name: None,
                        from_kind: from_kind.clone(),
                        from_id: from_id.clone(),
                        origin: Some(autoshop::store::VERSION_ORIGIN_USER.to_string()),
                    },
                );
                self.refresh_versions();
                self.status = trf(
                    lang,
                    "Version v{n} saved → {path}",
                    &[("n", &n.to_string()), ("path", &p.display().to_string())],
                );
            }
            Err(e) => {
                let t = trf(lang, "Save version failed: {err}", &[("err", &e.to_string())]);
                self.status = t.clone();
                self.toast(ToastKind::Error, t);
            }
        }
                Ok(())
            },
        );
        if let Err(e) = locked {
            self.persist_postponed(&e, "Save version failed: {err}", &[]);
        }
    }

    /// Load version `n` as the working recipe (one undo step, like AI Analyze).
    pub(crate) fn load_version(&mut self, n: u32) {
        let lang = self.lang;
        let Some(src) = self.src_path.clone() else { return };
        let p = Self::version_path(&src, n);
        // ONE NoWait lock across the whole snapshot read: the version file,
        // the mask re-anchor probes and the raster detach copies are separate
        // file touches, and delete_version sweeps v<N>.* rasters FIRST and
        // removes the recipe LAST — an unlocked load could parse a recipe
        // whose rasters were already swept and the next Ctrl+S then persists
        // dangling paths. Foreground button ⇒ NoWait (the save_version rule);
        // a busy develop reports and leaves the canvas untouched. The clamp
        // toast and the pre-era repair (a seconds-long RAW decode) run
        // OUTSIDE the lock — the read-under-lock / decide-outside split
        // read_saved_develop already draws.
        let locked = autoshop::store::with_develop_lock(
            &src,
            autoshop::store::DevelopLockMode::NoWait,
            || -> std::io::Result<anyhow::Result<(EditRecipe, autoshop::recipe::ClampSummary)>> {
                Ok((|| {
                    // A stale UI row can still name a half-deleted version:
                    // complete pending recovery (killed deletes included)
                    // under OUR lock before reading — reentrant, so this
                    // costs no second kernel acquisition (L03).
                    let _ = autoshop::store::recover_orphan_baks(&src);
                    let s =
                        autoshop::store::read_text_capped(&p, autoshop::store::MAX_STORE_JSON)?;
                    let mut r = serde_json::from_str::<EditRecipe>(&s)?;
                    let dropped = r.clamp();
                    // Snapshots name their rasters by bare file name, like
                    // the working recipe — re-anchor them to the develop dir.
                    if let Some(base) = p.parent() {
                        autoshop::store::resolve_mask_paths(&mut r, base);
                    }
                    // Then DETACH from the snapshot: the frozen v<N>.*.png
                    // files belong to that version and `delete_version`
                    // sweeps them, so a canvas pointing at them lost its
                    // masks the moment the user deleted the version it was
                    // loaded from — and the next save persisted the dangling
                    // path. The loaded state gets its own claimed copies.
                    autoshop::store::detach_rasters(&src, &mut r, "mask-restored");
                    Ok((r, dropped))
                })())
            },
        );
        let outcome = match locked {
            Ok(inner) => inner,
            Err(e) => {
                // The uniform busy/IO mapper every foreground compound uses.
                return self.persist_postponed(
                    &e,
                    "Load v{n} failed: {err}",
                    &[("n", &n.to_string())],
                );
            }
        };
        match outcome {
            Ok((mut r, dropped)) => {
                if !dropped.is_empty() {
                    // Same W20 disclosure as the open restore: a snapshot
                    // past the caps loads minus edits, never silently —
                    // and all four loss kinds, like the open toast.
                    let t = trf(
                        lang,
                        "recipe limits discarded {n} mask(s), {m} component(s), {c} curve point(s) and {s} string byte(s) on restore — the saved file exceeds the app's caps",
                        &[
                            ("n", &dropped.dropped_masks.to_string()),
                            ("m", &dropped.dropped_components.to_string()),
                            ("c", &dropped.truncated_curve_points.to_string()),
                            ("s", &dropped.truncated_string_bytes.to_string()),
                        ],
                    );
                    self.toast(ToastKind::Error, t);
                }
                // A Generated variant's pixels already carry the camera look
                // AND the lens corrections — a source-based snapshot's
                // calibration would cook both twice (same strip rule as the
                // open-restore and Analyze paths).
                // BOTH directions of the calibration rule, through their one
                // owner (R24-3 `reconcile_snapshot_calibration`): stripped
                // onto a pixel-state card, and — the direction nothing
                // covered — re-stamped when a snapshot taken OFF one lands on
                // the negative with no camera base look at all. The photo's
                // estimate is a RAW decode, so it is paid lazily, inside the
                // branch that needs it.
                let restamped = reconcile_snapshot_calibration(
                    &mut r,
                    self.active_is_generated(),
                    || {
                        let (k, t) = autoshop::pipeline::fresh_as_shot_wb(&src);
                        (
                            autoshop::pipeline::photo_base_knots(&src),
                            autoshop::pipeline::fresh_lens_profile(&src),
                            k.zip(t),
                        )
                    },
                );
                // A version snapshot is a saved recipe like any other, and
                // it predates the era stamp by definition — loading one put
                // an unrepaired washed curve straight onto the canvas and
                // into everything exported from it. AFTER the generated
                // strip: a generated canvas keeps no base curve, so there is
                // nothing to repair (paying the estimate just to discard it,
                // and disclosing a re-estimate the strip then deleted, were
                // both wrong). The note rides the SAME status write as the
                // load announcement — a separate earlier write was replaced
                // one statement later, so the promised disclosure never
                // survived to a single rendered frame.
                let relook =
                    autoshop::pipeline::repair_pre_era_base_curve(&src, &mut r).is_some();
                // …and the coordinate frame, for the same reason and from the
                // same file: a `v<N>.recipe.json` written before v0.30.0 holds
                // its crop and masks in the SENSOR frame of a rotated RAW.
                // Ungated by the generated strip above (a pixel-state card
                // still has geometry) and toasted rather than folded into the
                // status line, so the raster caveat has room.
                let reframe = autoshop::pipeline::migrate_recipe_coord_frame(&src, &mut r);
                self.recipe = r;
                self.resync_recipe_display();
                self.dirty = true;
                let loaded = trf(lang, "Loaded version v{n} — Ctrl+Z returns to before the load", &[("n", &n.to_string())]);
                if restamped {
                    // Said out loud, in the same channel the other
                    // calibration correction uses: the snapshot on disk holds
                    // no camera base look, and the canvas now does.
                    self.toast(
                        ToastKind::Success,
                        tr(
                            lang,
                            "this snapshot was taken on a generated variant, whose look lives in its pixels — the photo's own camera base look was applied so it renders on the negative",
                        )
                        .to_string(),
                    );
                }
                if let Some(c) = reframe {
                    self.toast(ToastKind::Success, coord_migration_sentence(lang, c));
                }
                self.status = if relook {
                    // The GUI's OWN sentence, localized: the engine note is
                    // English prose meant for the CLI/HTTP surfaces, and
                    // embedding it verbatim put raw English into a localized
                    // status line.
                    format!(
                        "{loaded} — {}",
                        tr(
                            lang,
                            "camera base look re-estimated — this photo was saved by a version whose preview sampler ran bright, so its stored base look rendered too dark",
                        )
                    )
                } else {
                    loaded
                };
            }
            Err(e) => {
                let t = trf(
                    lang,
                    "Load v{n} failed: {err}",
                    &[("n", &n.to_string()), ("err", &e.to_string())],
                );
                self.status = t.clone();
                self.toast(ToastKind::Error, t);
            }
        }
    }

    /// Open one of the gallery photos by index (keeps the thumbnail highlighted).
    pub(crate) fn open_gallery_index(&mut self, idx: usize) {
        if self.busy {
            return;
        }
        let Some(path) = self.gallery.get(idx).cloned() else { return };
        self.selected = Some(idx);
        self.open_path(path);
    }

    /// Scan `dir` (recursively) for sources off the UI thread and replace the
    /// gallery — folders can hold thousands of RAWs, so this never blocks paint.
    pub(crate) fn open_folder(&mut self, dir: PathBuf) {
        if self.busy {
            return;
        }
        let lang = self.lang;
        self.busy = true;
        self.status = trf(lang, "scanning {path} …", &[("path", &dir.display().to_string())]);
        self.spawn_worker(
            move || {
                let res = autoshop::pipeline::find_sources_counted(&dir)
                    .map(|(list, skipped)| (dir, list, skipped));
                Msg::Folder(Box::new(res))
            },
            |e| Msg::Folder(Box::new(Err(e))),
        );
    }

    /// The live working state as an [`UndoStep`]: current recipe + the active
    /// variant's pixel identity. Arc/path clones only — never pixel copies.
    pub(crate) fn current_step(&self) -> UndoStep {
        UndoStep {
            recipe: self.recipe.clone(),
            base: self.active_variant().and_then(|v| v.base.clone()),
            origin: self.active_variant().and_then(|v| v.origin.clone()),
        }
    }

    /// Reset undo history — call when a brand-new photo opens, when the active
    /// variant changes (you can't undo across either), or when a photo GOES
    /// AWAY: a failed fresh open leaves no canvas for its steps to apply to,
    /// and an undo there restored one that no longer existed. `committed`
    /// becomes the current head — with no photo open that is the empty step,
    /// which is exactly what stops the next settled frame pushing a phantom.
    pub(crate) fn reset_history(&mut self) {
        self.committed = self.current_step();
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    /// Commit the current state as one undo step RIGHT NOW (when it differs
    /// from the head). Used directly by programmatic pixel swaps (the retouch
    /// bake-in): waiting for `commit_if_settled` left a one-frame window where
    /// a same-frame Ctrl+Z saw only the older history and undid a recipe step
    /// while keeping the freshly retouched pixels installed.
    pub(crate) fn commit_now(&mut self) {
        let cur = self.current_step();
        if cur.recipe != self.committed.recipe || !cur.same_pixels(&self.committed) {
            self.undo_stack.push(std::mem::replace(&mut self.committed, cur));
            if self.undo_stack.len() > 100 {
                self.undo_stack.remove(0); // cap history memory
            }
            self.redo_stack.clear();
        }
    }

    /// Commit the current state as ONE undo step once the edit gesture settles
    /// (pointer released) — dragging a slider is one step, not one per frame.
    /// Programmatic edits (Analyze, Reset) land here on the next frame.
    pub(crate) fn commit_if_settled(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.pointer.any_down()) {
            self.commit_now();
        }
    }

    /// Recipe replaced wholesale (undo/redo/Reset/version load/AI apply): the
    /// derived display state must follow, or the panel keeps showing an AI
    /// rationale / verdict that describes a recipe no longer on screen — and
    /// an ARMED index-carrying tool (range sampler, ↻ Redraw) would fire into
    /// whatever mask now happens to sit at its remembered index.
    /// One tool at a time on the canvas — the SINGLE owner of the disarm
    /// list. Ten arming sites used to hand-copy these assignments, and the
    /// comments at two of them record real bugs born from copies drifting;
    /// every arming site now calls this before setting its own flag.
    /// Transient gesture anchors die with the tools: a stale place_start or
    /// crop_drag used to hijack the next drag. clone_src (the sampled source
    /// pin) deliberately survives — samples stay for resuming, like paint.
    pub(crate) fn disarm_tools(&mut self) {
        self.set_crop_mode(false);
        self.paint_mode = false;
        self.clone_mode = false;
        self.wb_picking = false;
        self.range_picking = None;
        self.placing_mask = None;
        self.place_start = None;
        self.crop_drag = None;
        self.mask_drag = None;
        self.paint_last = None;
        self.end_mask_brush();
        // A preset change armed for the crop tool must die with it — the
        // flag survived Esc / Done / a hold-B detour and rewrote the box as
        // a surprise on the next crop entry (CX5-4).
        self.crop_aspect_pending = false;
    }

    /// End a live mask-brush session (the Esc teardown, extracted from
    /// `disarm_tools` so the three former hand-copies stay one owner):
    /// strokes live in the canvas + gray buffer only until 「Apply」 bakes
    /// them into a claimed raster, so dropping both IS the cancel. The
    /// canvas is cleared too — fill/heal would otherwise inherit the
    /// brush-mask strokes as a phantom retouch selection. Returns whether a
    /// session was actually live, so a caller whose teardown is NOT
    /// user-initiated can disclose the discarded strokes.
    pub(crate) fn end_mask_brush(&mut self) -> bool {
        if self.mask_brush.take().is_some() {
            self.mask_brush_gray = None;
            self.paint_mode = false;
            self.clear_mask();
            true
        } else {
            false
        }
    }

    /// The 「Paint mask」 checkbox was just flipped (it writes `paint_mode`
    /// itself; this is the follow-up). Ticking it sweeps the other canvas
    /// tools, exactly as before. UN-ticking it used to do nothing at all —
    /// and `paint_mode` is ALSO the flag a live MASK-brush session paints
    /// through (`start_mask_brush` sets it), so un-ticking left an orphan
    /// session: ⌫ / ✓ Apply / ✕ Cancel still on screen, the brush inert, and
    /// 「Apply」 ready to bake whatever stale weights the buffer still held.
    /// Un-ticking IS a cancel, so it takes the session's one teardown — the
    /// same silent discard ✕ Cancel performs. Lives here, next to that
    /// teardown, so the panel closure holds no logic to drift (R22-3).
    pub(crate) fn paint_mode_toggled(&mut self) {
        if self.paint_mode {
            // Mutual exclusion lives in ONE place (disarm_tools) — the old
            // hand copy at the call site drifted once and made a ticked brush
            // completely inert (dispatch tries the other tools first).
            self.disarm_tools();
            self.paint_mode = true; // re-arm after the sweep
        } else {
            self.end_mask_brush();
        }
    }

    /// Selection moved: every INDEX-ARMED tool whose target is no longer
    /// the selected mask dies with it — `remap_mask_indices`' twin (that
    /// one covers list SHAPE changes; this one covers which row is live).
    /// ↻ Redraw / add-component are the silent pair: their arming buttons
    /// live inside the selected-mask block and the canvas hint discards the
    /// PlaceTarget, so an armed Redraw(j) reads exactly like a fresh
    /// gradient and the next drag rewrites mask j while the user is looking
    /// at another row. A `NewMask` placement and the non-mask tools (crop,
    /// WB picker, clone) are deliberately spared — they are not bound to a
    /// selection. Returns whether a BRUSH session died (its strokes are
    /// user paint), for the caller's disclosure.
    pub(crate) fn disarm_selection_bound_tools(&mut self, keep: Option<usize>) -> bool {
        if matches!(
            self.placing_mask,
            Some((_, PlaceTarget::Redraw(j) | PlaceTarget::Component(j, _))) if Some(j) != keep
        ) {
            self.placing_mask = None;
            self.place_start = None;
        }
        // The colour sampler drops unconditionally — its 🎯 label lives on
        // the row that was just left (the first fix of this class).
        self.range_picking = None;
        if matches!(self.mask_brush, Some((Some(j), _)) if Some(j) != keep) {
            return self.end_mask_brush();
        }
        false
    }

    /// Any canvas tool armed? (the once-hand-written OR list)
    pub(crate) fn tool_armed(&self) -> bool {
        self.crop_mode
            || self.paint_mode
            || self.clone_mode
            || self.wb_picking
            || self.range_picking.is_some()
            || self.placing_mask.is_some()
    }

    /// Commit a pending mask-rename buffer to its own mask (the same
    /// seed-guard the panel's row-switch commit uses: only while that mask's
    /// name still equals the seed-time snapshot). Focus-independent Ctrl+S
    /// runs while the TextEdit still holds focus — without this it saved the
    /// old name and the advanced baseline dropped the rename.
    pub(crate) fn commit_mask_name_buf(&mut self) {
        if let Some((j, orig, buf)) = self.mask_name_buf.clone()
            && let Some(prev) = self.recipe.masks.get_mut(j)
            && prev.name == orig
            && buf != orig
        {
            prev.name = buf.clone();
            // Re-seed so the panel's own lost-focus commit stays a no-op.
            self.mask_name_buf = Some((j, buf.clone(), buf));
        }
    }

    pub(crate) fn resync_recipe_display(&mut self) {
        self.rationale = self.recipe.rationale.clone();
        // Typed notes described the PREVIOUS rationale — a swap without a
        // clear would render a stale zh text over a different develop. The
        // landings that produce fresh notes install them AFTER this call.
        self.rationale_notes.clear();
        self.verdict = None;
        // A wholesale recipe swap (undo/redo/Reset/version load/AI apply)
        // disarms EVERY canvas tool — an armed index-carrying tool would fire
        // into whatever now sits at its remembered index, and a live
        // crop/rotate drag would re-apply its stale start angle over the
        // freshly restored recipe on the very next drag frame.
        self.disarm_tools();
        self.mask_name_buf = None; // stale (index, text) must not cross-commit
        // A curve-point drag in flight when the recipe is swapped (Ctrl+Z
        // mid-drag) kept mutating point i of the RESTORED curve on the next
        // drag frame (M17).
        self.curve_drag = None;
        // A wholesale recipe swap can shrink the mask list — an out-of-range
        // selection must not linger (every consumer bounds-checks, but the
        // panel would silently deselect anyway; do it deterministically).
        // Same-index-different-mask after undo is a known cosmetic residue —
        // mask identity isn't tracked across history steps.
        self.sel_mask = self.sel_mask.filter(|&i| i < self.recipe.masks.len());
        // The component selection rides on the mask's OWN list, which the
        // swap can also reshape — same deterministic bound.
        self.sel_component = self.sel_component.filter(|&c| {
            self.sel_mask
                .and_then(|i| self.recipe.masks.get(i))
                .is_some_and(|m| c < m.components.len())
        });
    }

    /// Apply a history step: recipe always; the active variant's pixels only
    /// when the step's identity differs (so plain slider undos stay pixel-free
    /// and O(1)). A restored `base: None` reverts a retouched source variant
    /// to the shared source neutral.
    pub(crate) fn apply_step(&mut self, step: UndoStep, ctx: &egui::Context) {
        let pixels_differ = !step.same_pixels(&self.committed);
        self.committed = step.clone();
        self.recipe = step.recipe;
        // The history is a READER too: a step recorded while the canvas was
        // washed reinstalls the pre-era pair on undo/redo — and the actor
        // that later promoted the canvas (Reset, Analyze, load_version,
        // Ctrl+S) cannot know which stack entries still hold it (the
        // save-time restamp covers only the pair IT replaced). Repairing on
        // the way back keeps every door to the canvas behind one rule.
        // Cost honesty: when the OPEN succeeded, the worker primed the
        // memo and this is a lookup. But washed steps enter history
        // precisely when the open-time estimate was an INABILITY — uncached
        // by design — so the first traversal here IS the retry point and
        // pays one estimate (the accepted class shared with load_active and
        // the open gate); its answer memoizes and the rest of the washed
        // history heals by lookup. Era-2 installs short-circuit before any
        // I/O; the Generated guard matches every other ordering site; and a
        // CROSS-PHOTO in-flight open is refused — src_path already points
        // at the INCOMING photo while this history belongs to the outgoing
        // one (the live-canvas override records the same hazard), so
        // repairing would estimate the WRONG photo onto this canvas. A
        // same-path preview-resolution re-decode (request AND fact: the
        // keep_recipe request verified against the recorded open_same_path)
        // keeps the pair coherent AND its keep arm preserves history —
        // refusing there made a washed install DURABLE, so it is admitted.
        // A same-path FRESH reopen is NOT: its rebuild discards this repair,
        // so admitting it bought a concurrent decode and a false success
        // toast. `committed` follows the heal, or the next commit_now would
        // push the washed head straight back onto the stack.
        //
        // DEFENCE IN DEPTH since the keyboard undo took the buttons' busy
        // gate: `open_in_flight` implies `busy` (open_path early-returns while
        // busy, sets both, and clears both in one handler), and undo/redo are
        // this fn's only callers — so no live caller reaches here mid-flight
        // any more. The gate stays because ungating the keyboard would
        // resurrect every scenario above, and the test drives it directly.
        if (!self.open_in_flight || (self.open_same_path && self.keep_recipe))
            && self.variants.get(self.active).is_none_or(|v| v.kind.is_parametric())
            && let Some(p) = self.src_path.clone()
            && autoshop::pipeline::repair_pre_era_base_curve(&p, &mut self.recipe).is_some()
        {
            self.committed.recipe = self.recipe.clone();
            if let Some(v) = self.variants.get_mut(self.active) {
                v.recipe = self.recipe.clone();
            }
            let lang = self.lang;
            self.toast(
                ToastKind::Success,
                tr(
                    lang,
                    "camera base look re-estimated — this photo was saved by a version whose preview sampler ran bright, so its stored base look rendered too dark",
                )
                .to_string(),
            );
        }
        if pixels_differ {
            if let Some(v) = self.variants.get_mut(self.active) {
                v.base = step.base;
                v.origin = step.origin;
            }
            self.refresh_active_pixels(ctx);
            // The quietest door: undoing to a SUPERSEDED master installs a
            // raster the preference cannot re-decode (the resolution switch
            // repoints only the Arc the canvas held), and undo/redo wrote no
            // status at all — so the combo was left to imply the canvas had
            // followed it. BOTH directions: the redo back to a reachable
            // canvas has to retract the claim, or it stands there false.
            self.set_canvas_status("restored the canvas pixels");
        }
        self.dirty = true;
        self.resync_recipe_display();
    }

    /// Rebuild the pixel-derived per-variant state after the active variant's
    /// base changed under the same variant (retouch undo/redo): before pane,
    /// paint canvas, overlay reference and the retained frame all described
    /// the old raster. History is deliberately NOT touched here.
    pub(crate) fn refresh_active_pixels(&mut self, ctx: &egui::Context) {
        let Some(v) = self.variants.get(self.active) else { return };
        let base = v.base.clone().or_else(|| self.source_preview.clone());
        if let Some(base) = base {
            let (mw, mh) = base.dimensions();
            // Same rule as load_active: the canvas recipe's own curve serves
            // every variant (a Generated recipe's curve is empty by
            // construction; an InPlace master is neutral and needs it).
            let curve = self.recipe.base_curve.clone();
            self.set_before(ctx, &base, &curve);
            self.base_preview = Some(base);
            self.rebind_paint_canvas(mw, mh);
        }
        self.overlay_ref = None;
        self.overlay_stale = true;
        self.last_rgb = None;
    }

    /// The base plate under the canvas was REPLACED (a preview-resolution
    /// re-decode, a cold master landing, a retouch bake, a fresh open): the
    /// paint canvas is resized to it — and any live mask-brush session dies
    /// with the old plate, DISCLOSED. The session's weight buffer is
    /// dimension-locked to the plate it was armed on (`start_mask_brush`
    /// sizes both together), so a stroke stamped after the swap lands at
    /// the wrong coordinates in it — and the always-visible 「✓ Apply」 row
    /// would bake those misplaced weights into a durably-adopted raster. A
    /// variant switch already gets the teardown via `load_active`'s
    /// `disarm_tools`; the async landings never called it (L06#5). The
    /// toast lives HERE so no future plate-replacement door can forget it;
    /// a second rebind in the same landing finds no session and stays
    /// silent.
    pub(crate) fn rebind_paint_canvas(&mut self, w: u32, h: u32) {
        if self.end_mask_brush() {
            let t = tr(
                self.lang,
                "the mask-brush session ended — the canvas pixels underneath were replaced, so its strokes no longer line up",
            );
            self.toast(ToastKind::Error, t.to_string());
        }
        self.mask_paint = Some(image::RgbaImage::new(w, h));
        self.mask_tex = None;
        self.mask_dirty = false;
        self.paint_last = None;
    }

    /// May the photographer turn this photo right now? (Drives both the
    /// toolbar buttons' enabled state and the plate reconciler's own gate, so
    /// the two can never disagree about which photos rotate.)
    ///
    /// The one exclusion is BAKED PIXELS. A retouched/generated master is a
    /// raster on disk in the frame it was baked in, and this build has no way
    /// to record that a file was written before a turn — so turning the canvas
    /// under it would put the master back sideways on the next open, which is
    /// the silent-corruption shape the whole orientation chain exists to
    /// prevent. Rotate FIRST, bake after: the master is then written in the
    /// turned frame and everything agrees. The whole strip counts, not just
    /// the active card — an inactive variant's master is on disk exactly the
    /// same way. (Registered limitation, R27 A6.)
    ///
    /// An ARMED tool is excluded for a smaller reason: brush and retouch
    /// gestures bind screen coordinates to plate pixels mid-drag, and the
    /// plate is about to move under them.
    pub(crate) fn can_rotate(&self) -> bool {
        self.src_path.is_some()
            && self.base_preview.is_some()
            && !self.busy
            && !self.open_in_flight
            && !self.paint_mode
            && self.region.is_none()
            && self.variants.iter().all(|v| v.base.is_none() && v.origin.is_none())
    }

    /// Turn the open photo by `delta` clockwise quarter turns — ONE undo step,
    /// the whole develop moving together ([`autoshop::pipeline::rotate_recipe`]
    /// owns geometry + raster masks + the turn count).
    ///
    /// The plates are NOT turned here: `sync_base_turns` sees the changed
    /// `recipe.quarter_turns` on the next frame and moves them, which is the
    /// same path an UNDO of this action takes. One mover, so the two
    /// directions cannot drift.
    pub(crate) fn rotate_photo(&mut self, delta: u8, ctx: &egui::Context) {
        if !self.can_rotate() {
            return;
        }
        let Some(src) = self.src_path.clone() else { return };
        let lang = self.lang;
        // The step BEFORE the turn, so one Ctrl+Z puts it back (7.7).
        self.commit_now();
        let mut next = self.recipe.clone();
        match autoshop::pipeline::rotate_recipe(&mut next, &src, delta) {
            Ok(_) => {
                self.recipe = next;
                self.dirty = true;
                self.sync_base_turns(ctx);
                self.resync_recipe_display();
                // …and SEAL it, rather than waiting for `commit_if_settled`
                // on the next frame: a click is already a settled gesture, and
                // the one-frame window let a same-frame Ctrl+Z undo the step
                // BEFORE this one (the hazard `commit_now`'s own doc records).
                self.commit_now();
            }
            Err(e) => {
                // All-or-nothing (see `rotate_recipe`): the recipe is the
                // untouched clone, so nothing moved and nothing needs undoing.
                // The sentence below covers the DISK too since R28 Batch-3 3b —
                // a failed turn used to leave one orphan PNG per attempt in the
                // develop dir while this toast said nothing was changed.
                let t = trf(
                    lang,
                    "could not turn this photo: {err} — nothing was changed",
                    &[("err", &e.to_string())],
                );
                self.toast(ToastKind::Error, t);
            }
        }
    }

    /// Bring the base plates into the frame `recipe.quarter_turns` asks for.
    ///
    /// Called once per frame (`app.rs` update) rather than from each of the
    /// eight places that can change the turn — undo, redo, a variant switch, a
    /// version load, a paste, the rotate buttons. `image`'s rotate is
    /// LOSSLESS and the plate is preview-sized, and the common case is a `u8`
    /// comparison that returns immediately.
    ///
    /// Turning the PIXELS rather than teaching the canvas a rotation is
    /// deliberate, and it is the same decision `render_to_image_in` makes:
    /// every screen↔frame mapping downstream (`view_norm_to_orig`, the crop
    /// handles, the paint canvas, the coverage overlay, the retouch region)
    /// is defined against the plate, so one turn here moves all of them and
    /// none of them needs to know.
    pub(crate) fn sync_base_turns(&mut self, ctx: &egui::Context) {
        let want = self.recipe.quarter_turns % 4;
        let delta = (want + 4 - self.base_turns % 4) % 4;
        if delta == 0 {
            return;
        }
        // A baked master is on disk in its own frame; `can_rotate` refuses to
        // create a delta over one, and this is the same gate seen from the
        // other side — without it a photo that was rotated BEFORE its retouch
        // would have its master turned a second time on every open.
        if self.variants.iter().any(|v| v.base.is_some() || v.origin.is_some()) {
            self.base_turns = want;
            return;
        }
        let turn = |p: &Arc<image::DynamicImage>| {
            Arc::new(autoshop::render::turn_image((**p).clone(), delta))
        };
        self.source_preview = self.source_preview.as_ref().map(&turn);
        self.base_preview = self.base_preview.as_ref().map(&turn);
        self.base_turns = want;
        // Everything derived from the plate's frame. The Before texture is
        // rebuilt here rather than invalidated: `set_before` is the only
        // writer, and leaving a stale one on screen for a frame is the
        // sideways-preview bug in miniature.
        if let Some(b) = self.base_preview.clone() {
            let curve = self.recipe.base_curve.clone();
            self.set_before(ctx, &b, &curve);
        }
        self.overlay_ref = None;
        self.overlay_stale = true;
        self.last_rgb = None;
        self.mask_tex = None;
        self.mask_overlay_tex = None;
        self.overlay_key = None;
        self.dirty = true;
    }

    pub(crate) fn undo(&mut self, ctx: &egui::Context) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.committed.clone());
            self.apply_step(prev, ctx);
        }
    }

    pub(crate) fn redo(&mut self, ctx: &egui::Context) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.committed.clone());
            self.apply_step(next, ctx);
        }
    }

    /// The quit-confirm layer (in-app egui window). Lists what quitting would
    /// lose; 「Save all & quit」 writes each pending develop exactly like
    /// Ctrl+S (explicit user save → no backup gate), 「Discard & quit」 makes
    /// the state clean so the close guard lets the next close through.
    pub(crate) fn confirm_quit_layer(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        let accent = self.theme.colors().accent_text; // Copy — safe in closures
        // Everything quitting would lose: the stash + the open photo's canvas
        // (the live canvas outranks its own stale stash entry). Each entry
        // carries its pixel identity so 「Save all」 persists a baked retouch's
        // master link exactly like Ctrl+S would. (`== Generated` here is the
        // `pixels.json` FORMAT flag — not the R24-1 parametric predicate.)
        let mut pending: Vec<PendingSave> = self
            .nav_stash
            .iter()
            .map(|(p, st)| PendingSave {
                photo: p.clone(),
                recipe: st.recipe.clone(),
                pixels: st.origin.clone().map(|o| (o, st.kind == VariantKind::Generated)),
                strip: stash_strip_record(st),
            })
            .collect();
        // Live-canvas override — skipped while a photo OPEN is in flight:
        // src_path already points at the new photo but recipe/saved_recipe
        // still describe the old one, so acting on the pair would either drop
        // the new photo's stash entry or save the old recipe under the new
        // path. The stash (written by open_path just before the flight) is
        // the correct authority for that window. ONLY the open transition —
        // gating on plain `busy` let any background worker (Export…) empty
        // `pending`, and the empty-pending branch below then CLOSED the app,
        // losing a sole live unsaved canvas (background work still lands
        // under the dialog — the scrim below blocks only pointer input).
        if !self.open_in_flight && let Some(p) = self.src_path.clone() {
            let origin = self.active_variant().and_then(|v| v.origin.clone());
            // The live canvas outranks its own stash entry WHOLESALE: displace
            // it even when the canvas is clean (the user reverted) — the loop
            // below would otherwise write the stale stashed edits to disk.
            pending.retain(|e| e.photo != p);
            // The strip counts as unsaved work of the OPEN photo too — a
            // clean canvas over an unpersisted strip must still be listed,
            // or Save-all had nothing to write and the guard's re-check
            // bounced the close forever (the v0.21 dead-button livelock).
            if self.pending_save_gate_dirty() {
                let pix = origin.map(|o| (o, self.active_is_generated()));
                pending.push(PendingSave {
                    photo: p,
                    recipe: self.recipe.clone(),
                    pixels: pix,
                    strip: self.current_strip_record(),
                });
            }
        }
        // Background variants ride IN `pending` since v0.22: each entry
        // carries its photo's strip record, and Save-all persists it to
        // variants.json beside the develop. The count still gates the
        // empty-pending close below (defence-in-depth — a strip-dirty photo
        // is also pushed into `pending` above).
        let orphan_variants = self.inactive_dirty_variants();
        if pending.is_empty() && orphan_variants == 0 {
            // Saved (or discarded) since the guard fired — nothing to protect.
            self.confirm_quit = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let mut open = true;
        let mut save_quit = false;
        let mut discard_quit = false;
        let mut cancel = false;
        // The global shortcut block is gated off while this layer is up, so
        // the promised keyboard grammar is honoured HERE: Esc = the safe
        // escape (always), Enter = the safe default (save everything) — but
        // only while NO button holds keyboard focus, or Enter on a focused
        // Cancel/Discard would fire Save instead of the focused control.
        let widget_focused = ctx.memory(|m| m.focused()).is_some();
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                cancel = true;
            }
            if !widget_focused && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                save_quit = true;
            }
        });
        // A modal owns the pointer as well as the keyboard (D13): the scrim
        // swallows every click behind the dialog — sliders were still live
        // during the save-or-discard decision — and dims the stakes into
        // view. Background workers are unaffected (see open_in_flight above).
        egui::Area::new(egui::Id::new("confirm_quit_scrim"))
            .order(egui::Order::Middle)
            .fixed_pos(egui::Pos2::ZERO)
            .show(ctx, |ui| {
                let r = ctx.screen_rect();
                ui.allocate_response(r.size(), egui::Sense::click_and_drag());
                ui.painter().rect_filled(r, 0.0, egui::Color32::from_black_alpha(96));
            });
        egui::Window::new(tr(lang, "● Unsaved edits"))
            .id(egui::Id::new("confirm_quit"))
            .order(egui::Order::Foreground)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                if !pending.is_empty() {
                    ui.label(trf(
                        lang,
                        "{n} photo(s) have edits that were never saved:",
                        &[("n", &pending.len().to_string())],
                    ));
                }
                // Parent folder + stem: two DSC_0431 from two shoots must be
                // distinguishable in the one dialog where the stakes are real.
                egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                    for PendingSave { photo: p, .. } in &pending {
                        let folder = p
                            .parent()
                            .and_then(|d| d.file_name())
                            .and_then(|s| s.to_str())
                            .unwrap_or("");
                        ui.label(
                            egui::RichText::new(format!(
                                "{folder}/{}",
                                autoshop::pipeline::stem(p)
                            ))
                            .small()
                            .weak(),
                        );
                    }
                });
                if orphan_variants > 0 {
                    // Informational since v0.22: Save-all persists each
                    // photo's whole variant strip (variants.json), so these
                    // are rescued WITH their photo, not lost.
                    ui.add_space(SPACE_MD);
                    ui.label(
                        egui::RichText::new(trf(
                            lang,
                            "{n} other variant(s) hold edits — 「Save all」 saves each photo's whole variant strip along with its develop.",
                            &[("n", &orphan_variants.to_string())],
                        ))
                        .small()
                        .weak(),
                    );
                }
                ui.add_space(SPACE_MD);
                // Safe escape far left, the destructive action fenced off by
                // space, the primary (save) tinted and last — a 4px slip from
                // Save must not land on an unrecoverable Discard.
                ui.horizontal(|ui| {
                    cancel |= ui.button(tr(lang, "Cancel")).on_hover_text("Esc").clicked();
                    ui.add_space(18.0);
                    discard_quit = ui
                        .button(
                            egui::RichText::new(tr(lang, "Discard & quit"))
                                .color(ui.visuals().warn_fg_color),
                        )
                        .on_hover_text(tr(lang, "Quit WITHOUT saving — these edits are gone for good"))
                        .clicked();
                    save_quit |= ui
                        .button(egui::RichText::new(tr(lang, "Save all & quit")).color(accent))
                        .on_hover_text(tr(lang, "Enter · save every listed develop, then quit"))
                        .clicked();
                });
            });
        if save_quit {
            let mut failed: Option<String> = None;
            // Collected, not fatal — reported on the way out (see below).
            let mut xmp_warns: Vec<String> = Vec::new();
            let mut clear_warns: Vec<String> = Vec::new();
            for PendingSave { photo: p, recipe: r, pixels: pix, strip } in &pending {
                // ONE NoWait lock per photo across its whole persist compound
                // (the Ctrl+S pairing). A develop busy in another process
                // fails THIS photo — the quit bounces and reports below —
                // instead of freezing the dialog behind an unbounded Wait.
                let locked: std::io::Result<()> = autoshop::store::with_develop_lock(
                    p,
                    autoshop::store::DevelopLockMode::NoWait,
                    || {
                // Neutral + no pixel identity + trivial strip = Ctrl+S's
                // "clear my edits": WRITING neutral files here pinned the
                // existence-keyed ● badge that a direct Ctrl+S removes. A
                // non-trivial strip takes the save path below instead —
                // clear_develop would destroy the OTHER cards' work.
                if r.is_noop() && pix.is_none() && strip.is_none() {
                    // The store's ONE clear primitive (both surfaces): it takes
                    // the retired pixels.json.bak with it, which a bare unlink
                    // left behind for the next open to republish.
                    match autoshop::store::clear_develop(p) {
                        Ok(o) => {
                            // clear_develop took variants.json with it — the
                            // open photo's mirror must follow, or the guard's
                            // way-out re-check compares live-trivial against
                            // a stale record and bounces the close.
                            if self.src_path.as_deref() == Some(p.as_path()) {
                                self.saved_strip = None;
                            }
                            // Cleared, but not MARKED — same channel as the XMP
                            // half: quitting silently would let a projection
                            // copied beside the RAW undo this clear unannounced.
                            if let Some(w) = o.marker_warning {
                                clear_warns.push(format!("{}: {w}", autoshop::pipeline::stem(p)));
                            }
                        }
                        Err(e) => {
                            failed = Some(format!("{}: {e}", autoshop::pipeline::stem(p)));
                        }
                    }
                    return Ok(());
                }
                // A GENERATED entry's recipe is the STRIPPED canvas form —
                // persisting it verbatim erased the RAW's calibration from
                // disk (the Analyze saver writes the calibrated form; a
                // master that later fails to decode then falls back to a
                // dark, uncorrected develop). Re-stamp from the same
                // saved-first source produce_recipe uses.
                let mut disk = r.clone();
                if pix.as_ref().is_some_and(|(_, g)| *g) {
                    // ONE snapshot for every half — independent reads could
                    // pair an OLD curve with a NEW profile if another surface
                    // published between them. The era stamp rides WITH the
                    // curve (the paste rule): this WRITES recipe.json, and a
                    // canvas-era stamp over a saved era-1 curve would launder
                    // it past every repair.
                    let cal = autoshop::pipeline::photo_calibration(p);
                    disk.version = cal.version;
                    disk.base_curve = cal.base_curve;
                    disk.lens_profile = cal.lens_profile;
                    disk.as_shot_k = cal.as_shot_k;
                    disk.as_shot_tint = cal.as_shot_tint;
                }
                // The WRITER's rule (Ctrl+S, api_xmp): a stashed canvas whose
                // open-time estimate failed still carries the washed pre-era
                // curve — repaired before it is persisted. Memo-cheap when
                // already repaired, and it also covers the generated arm
                // above, whose calibration snapshot may itself have adopted
                // an unrepaired curve when the store read hit an inability.
                // SAID on the loop's existing quitting-time channel — gated
                // on the RECIPE write, not the compound result below: the
                // pixel-link half can fail AFTER recipe.json already
                // published the re-estimated curve, and dropping the note
                // then hid a mutation that IS on disk (the Ctrl+S rule: the
                // disclosure travels with the mutation).
                let relook_note = autoshop::pipeline::repair_pre_era_base_curve(p, &mut disk);
                let generated = pix.as_ref().is_some_and(|(_, g)| *g);
                // ONE single-generation commit per photo (the Ctrl+S rule,
                // L03): recipe + baked-pixels link + strip record land whole
                // or not at all — the background variants this dialog just
                // listed cannot outlive a save that tore between members.
                let res: anyhow::Result<()> = (|| {
                    let recipe_bytes = autoshop::pipeline::recipe_store_bytes(p, &disk, autoshop::diag::stderr())?;
                    let pixels = match pix {
                        Some((o, g)) => autoshop::store::CommitMember::Write(
                            autoshop::store::pixel_source_record_bytes(p, o, *g)?,
                        ),
                        None => autoshop::store::CommitMember::Clear,
                    };
                    let variants = match strip {
                        Some(rec) => autoshop::store::CommitMember::Write(
                            autoshop::store::variants_record_bytes(p, rec)?,
                        ),
                        None => autoshop::store::CommitMember::Clear,
                    };
                    autoshop::store::commit_develop(
                        p,
                        autoshop::store::DevelopCommit {
                            recipe: Some(recipe_bytes),
                            pixels,
                            variants,
                        },
                    )?;
                    Ok(())
                })();
                match res {
                    Ok(()) => {
                        // The disclosure travels with the mutation (the
                        // Ctrl+S rule): the commit landed whole, so the
                        // re-estimated curve IS on disk.
                        if let Some(note) = relook_note.as_deref() {
                            eprintln!("⚠ {}: {note}", autoshop::pipeline::stem(p));
                        }
                        if self.src_path.as_deref() == Some(p.as_path()) {
                            self.saved_strip = strip.clone();
                            // The FOURTH mirror of the single-save set
                            // (export.rs): pixels.json for the ACTIVE photo
                            // just committed, and without this the ● badge
                            // and the exit re-check compared against a stale
                            // master link — a just-saved retouch was
                            // re-reported as unsaved (L11-6).
                            self.pixels_on_disk = pix.as_ref().map(|(o, _)| o.clone());
                        }
                    }
                    Err(e) => {
                        failed = Some(format!("{}: {e}", autoshop::pipeline::stem(p)));
                        return Ok(());
                    }
                }
                // NO XMP for a generated entry — Ctrl+S refuses those for the
                // same reason: the look lives in baked pixels no parametric
                // sidecar can reproduce, and writing one here overwrote a real
                // Lightroom sidecar with a lie. The recipe + pixels pair alone
                // restores faithfully.
                //
                // OUTSIDE the fatal result: the recipe write alone decides the
                // saved state (cross-surface rule), so a failed XMP projection
                // must not abort the remaining photos or refuse to quit over a
                // develop that IS durably saved.
                if autoshop::decode::is_raw(p) && !generated {
                    // The M6a mask-loss list is dropped HERE for the same
                    // reason as in the batch paste: `xmp_warns` renders under
                    // one fixed "projection(s) failed" sentence, which a
                    // lossy-but-successful projection is not. stderr carries
                    // the per-photo line from write_xmp_doc.
                    match autoshop::pipeline::write_xmp(p, &disk, autoshop::diag::stderr()) {
                        Ok((_, None, _)) => {}
                        // A regenerated (unmerged) sidecar loses LR-only
                        // properties — Save-all's warning list is exactly
                        // where that belongs (round-12 disclosure threading).
                        Ok((_, Some(n), _)) => {
                            xmp_warns.push(format!("{}: {n}", autoshop::pipeline::stem(p)));
                        }
                        Err(e) => {
                            xmp_warns.push(format!("{}: {e}", autoshop::pipeline::stem(p)));
                        }
                    }
                }
                        Ok(())
                    },
                );
                if let Err(e) = locked {
                    failed = Some(format!(
                        "{}: the photo is being changed by another Autoshop process ({e})",
                        autoshop::pipeline::stem(p)
                    ));
                }
                if failed.is_some() {
                    break;
                }
            }
            // ALL said regardless of a later photo's failure (the
            // travels-with-the-mutation rule the repair note above follows),
            // COUNTED (a single-slot accumulator reported only the last of
            // several same-kind failures), and worded per-photo so the
            // sentence stays true on the abort path — the old "develops
            // saved" claimed a completed batch even after a break.
            // stderr keeps the full list for logs; the USER-facing copy is
            // below — an eprintln alone vanished with the closing window,
            // which is exactly the silence W3 exists to end.
            if !xmp_warns.is_empty() {
                eprintln!(
                    "⚠ {} Lightroom XMP projection(s) failed for develop(s) that ARE saved: {}",
                    xmp_warns.len(),
                    xmp_warns.join("; ")
                );
            }
            if !clear_warns.is_empty() {
                eprintln!(
                    "⚠ {} clear(s) succeeded but could not be marked: {} — a sidecar \
                     beside the RAW may restore those edits on the next open",
                    clear_warns.len(),
                    clear_warns.join("; ")
                );
            }
            let mut quit_warns: Vec<String> = Vec::new();
            if !xmp_warns.is_empty() {
                quit_warns.push(trf(
                    lang,
                    "{n} Lightroom XMP projection(s) failed (those develops ARE saved): {detail}",
                    &[("n", &xmp_warns.len().to_string()), ("detail", &brief_list(&xmp_warns))],
                ));
            }
            if !clear_warns.is_empty() {
                quit_warns.push(trf(
                    lang,
                    "{n} clear(s) could not be marked: {detail} — a sidecar beside the RAW may restore those edits on the next open",
                    &[("n", &clear_warns.len().to_string()), ("detail", &brief_list(&clear_warns))],
                ));
            }
            match failed {
                None => {
                    self.nav_stash.clear();
                    self.saved_recipe = self.recipe.clone();
                    self.confirm_quit = false;
                    // The close guard re-checks on the way out. Everything
                    // above just persisted, so the re-check passes — but if
                    // anything is STILL dirty (logic drift, a future writer
                    // this loop misses), the bounce must be SAID: a silently
                    // re-armed dialog is the v0.21 dead-button livelock.
                    let residue = self.inactive_dirty_variants();
                    if residue > 0 {
                        let t = trf(
                            lang,
                            "saved, but {n} variant(s) still count as unsaved — the window stays open; please report this",
                            &[("n", &residue.to_string())],
                        );
                        self.toast(ToastKind::Error, t);
                    } else if !quit_warns.is_empty() {
                        // Everything IS saved (recipe-write-decides), so
                        // refusing to quit would be wrong — but a toast on a
                        // closing window dies unread. Bounce ONCE with the
                        // warnings visible; nothing is dirty any more, so the
                        // next quit passes straight through without this
                        // dialog. One extra click, only in the failure case.
                        let t = trf(
                            lang,
                            "saved with warnings — the window stays open so they can be read; quit again to close: {detail}",
                            &[("detail", &quit_warns.join(" · "))],
                        );
                        self.status = t.clone();
                        self.toast(ToastKind::Error, t);
                    } else {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                Some(e) => {
                    // Stay open: a failed save on quit must never quietly quit.
                    let t = trf(lang, "save failed: {err}", &[("err", &e)]);
                    self.status = t.clone();
                    self.toast(ToastKind::Error, t);
                    self.confirm_quit = false;
                }
            }
        } else if discard_quit {
            self.nav_stash.clear();
            self.saved_recipe = self.recipe.clone();
            // The answer to the guard's question, recorded: these two lines
            // clean the ACTIVE canvas, but a background variant's edits have
            // nowhere to be saved, so nothing can make them "clean" — only
            // the user's decision can end the question.
            self.discard_requested = true;
            self.confirm_quit = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if cancel || !open {
            self.confirm_quit = false;
        }
    }

    /// Migrate every gallery photo's sidecars from a user-picked pre-store
    /// ./out folder (Settings button). Worker-side: one read_dir per photo
    /// over a possibly huge folder is I/O the UI thread must not pay.
    pub(crate) fn start_import_legacy(&mut self, dir: PathBuf) {
        if self.busy {
            return;
        }
        let lang = self.lang;
        let photos = self.gallery.clone();
        if photos.is_empty() {
            self.status = tr(
                lang,
                "Open a folder first — import migrates the photos currently in the gallery",
            )
            .into();
            return;
        }
        self.busy = true;
        self.status = trf(
            lang,
            "Importing develops from {path} …",
            &[("path", &dir.display().to_string())],
        );
        self.spawn_worker(
            move || {
                // One directory scan for the whole gallery — the per-photo
                // migrate_legacy_from re-scanned the (possibly huge) legacy
                // folder once per photo, making import O(photos × entries).
                // FACTS out (L12#4): the landing renders the summary.
                let n = autoshop::store::migrate_legacy_from_many(&dir, &photos);
                Msg::LegacyImported(Ok((n, dir)))
            },
            |e| Msg::LegacyImported(Err(e)),
        );
    }

    /// Read the style library's status onto a worker thread (R23-2).
    ///
    /// Off the UI thread because the file it parses can reach 32 MB, and
    /// CACHED in `style_info` because the panel that shows it draws every
    /// frame. Re-run after a build; nothing else invalidates it (another
    /// process rebuilding the shared index mid-session is the one staleness
    /// this accepts — the alternative is a stat per frame forever).
    pub(crate) fn start_style_info(&mut self) {
        if self.style_info_loading {
            return;
        }
        self.style_info_loading = true;
        self.spawn_worker(
            || Msg::StyleInfo(Box::new(autoshop::style::index_info())),
            // A read that PANICS must still clear the flag, or the status line
            // says "reading…" for the rest of the session. Absent is the
            // honest fallback: no usable library was read.
            |_e| {
                Msg::StyleInfo(Box::new(autoshop::style::StyleIndexInfo {
                    path: autoshop::store::style_index_path(),
                    state: autoshop::style::StyleIndexState::Absent,
                }))
            },
        );
    }

    /// Build (or rebuild) the style library from `dir` — the GUI's missing
    /// production-side entry point (R23-2, feedback #6: building existed only
    /// on the CLI and in the web panel, neither of which a user who
    /// double-clicks the exe can reach).
    ///
    /// Background, with progress: `StyleIndex::build` decodes EVERY RAW in the
    /// folder, minutes on a real library. NOT `busy`-gated — freezing the whole
    /// app for that would be its own defect — so only this button gates on
    /// `style_build_inflight`.
    ///
    /// NO cancel: `StyleIndex::build` has no cancellation checkpoints (its
    /// decode workers run a scoped loop to completion), and an abandon-only ✕
    /// like Analyze's would leave every core decoding for minutes with nothing
    /// to show for it. The button stays disabled until the build lands and the
    /// panel says so; closing the window during a build is safe — the index is
    /// published tmp+rename (`StyleIndex::save`), so a killed process leaves
    /// the previous library intact.
    pub(crate) fn start_style_build(&mut self, dir: PathBuf) {
        if self.style_build_inflight {
            return;
        }
        self.style_build_inflight = true;
        self.style_build_progress = None;
        self.status = trf(
            self.lang,
            "Building the style library from {path} … every RAW is decoded, so a big folder takes minutes",
            &[("path", &abs_display(&dir))],
        );
        // The progress channel is the app's own sender: `spawn_worker` delivers
        // ONE terminal message, and this build has to report while it runs.
        let tx = self.tx.clone();
        let ctx = self.egui_ctx.clone();
        // Resolved HERE, on the UI thread, with the rest of the request the
        // worker will carry: the switch is a value, and a preference flipped
        // mid-build must not change what this build is doing.
        let embed = autoshop::style::EmbeddingSwitch::resolve(None, self.style_embed);
        let describe = autoshop::style::DescribeSwitch::resolve(None, self.style_describe);
        self.spawn_worker(
            move || {
                let progress = |p: autoshop::style::BuildProgress| {
                    let _ = tx.send(Msg::StyleBuildProgress {
                        stage: p.stage,
                        done: p.done,
                        total: p.total,
                    });
                    // An mpsc send does not wake egui (see `spawn_worker`).
                    ctx.request_repaint();
                };
                let index = match autoshop::style::StyleIndex::build_reporting(&dir, embed, describe, &progress) {
                    Ok(ix) => ix,
                    Err(e) => {
                        return Msg::StyleBuilt(Box::new(StyleBuildOutcome::Failed {
                            err: format!("{e:#}"),
                        }))
                    }
                };
                let total = index.exemplars.len();
                // The embedding degradation count (R28 Batch-4 4b). Asked here
                // rather than carried on `StyleIndex`, which is the SERIALISED
                // index: adding a field to it would be a file-format change for
                // a fact that is about this RUN, not about the library.
                //
                // Gated on the switch, because "no exemplar has a vector"
                // means two opposite things — the user never asked for the
                // sidecar (nothing to report), or they did and it failed for
                // every photo (the whole point of reporting).
                let without_embedding = if embed.on() {
                    index.exemplars.iter().filter(|e| e.embed.is_none()).count()
                } else {
                    0
                };
                // The S2 sibling, gated the same way and for the same reason:
                // "no exemplar carries prose" means two opposite things — the
                // user never asked for the pass, or they did and it reached
                // nothing.
                let described = if describe.on() {
                    index.exemplars.iter().filter(|e| e.desc.is_some()).count()
                } else {
                    0
                };
                // The empty-index refusal is NOT re-implemented here: `save`
                // owns it for every caller (an empty write truncates a good
                // index in place). The count only decides WHICH sentence the
                // landing shows for the refusal it hands back.
                match index.save(&autoshop::store::style_index_path()) {
                    Ok(()) => Msg::StyleBuilt(Box::new(StyleBuildOutcome::Saved {
                        total,
                        dir,
                        without_embedding,
                        described,
                    })),
                    Err(_) if total == 0 => {
                        Msg::StyleBuilt(Box::new(StyleBuildOutcome::NothingIndexed { dir }))
                    }
                    Err(e) => Msg::StyleBuilt(Box::new(StyleBuildOutcome::Failed {
                        err: format!("{e:#}"),
                    })),
                }
            },
            |e| Msg::StyleBuilt(Box::new(StyleBuildOutcome::Failed { err: e.to_string() })),
        );
    }

    pub(crate) fn start_looks_build(&mut self, dir: PathBuf) {
        if self.style_build_inflight { return; }
        self.style_build_inflight = true;
        self.style_build_progress = None;
        self.status = trf(self.lang, "Building the look library from {path}", &[("path", &abs_display(&dir))]);
        let tx = self.tx.clone();
        let ctx = self.egui_ctx.clone();
        let embed = autoshop::style::EmbeddingSwitch::resolve(None, self.style_embed);
        let describe = autoshop::style::DescribeSwitch::resolve(None, self.style_describe);
        self.spawn_worker(
            move || {
                let progress = |p: autoshop::style::BuildProgress| {
                    let _ = tx.send(Msg::StyleBuildProgress {
                        stage: p.stage,
                        done: p.done,
                        total: p.total,
                    });
                    ctx.request_repaint();
                };
                match autoshop::style::StyleIndex::build_looks(&dir, embed, describe, &progress) {
                    Ok(index) => {
                        let total = index.looks.len();
                        let described = if describe.on() {
                            index.looks.iter().filter(|l| l.desc.is_some()).count()
                        } else {
                            0
                        };
                        match index.save(&autoshop::store::style_index_path()) {
                            Ok(()) => Msg::StyleBuilt(Box::new(StyleBuildOutcome::LooksSaved { total, dir, described })),
                            Err(e) => Msg::StyleBuilt(Box::new(StyleBuildOutcome::Failed { err: format!("{e:#}") })),
                        }
                    }
                    Err(e) => Msg::StyleBuilt(Box::new(StyleBuildOutcome::Failed { err: format!("{e:#}") })),
                }
            },
            |e| Msg::StyleBuilt(Box::new(StyleBuildOutcome::Failed { err: e.to_string() })),
        );
    }

    pub(crate) fn start_analyze(&mut self, refine: bool) {
        let Some(path) = self.src_path.clone() else { return };
        // `busy` alone stopped guarding this the moment Analyze became
        // cancellable: ✕ clears it while the request is still on the wire and
        // still billing. See `analyze_inflight`.
        if self.busy || self.analyze_inflight {
            return;
        }
        let lang = self.lang;
        self.busy = true;
        self.status = if refine {
            tr(lang, "refining your current edit with AI…").into()
        } else {
            tr(lang, "analyzing with AI (GPT + Claude)…").into()
        };
        // Both style inputs travel as one request (R23-2): the strength, and
        // whether the nearest past photo goes along as a second image. Read
        // HERE, on the UI thread, so the worker cannot pick up a toggle the
        // user flipped after starting the call.
        let req = autoshop::pipeline::GradeRequest {
            style: self.style_strength,
            send_reference_image: self.send_style_ref_image,
            // R23-3: the OTHER axis — how committed the grade should be. Read
            // here for the same reason, and separate from `style` on purpose
            // (「像不像我」 vs 「下手多重」).
            strength: self.panel_strength(),
            // R23-4: read on the UI thread with the rest of the request, for
            // the same reason — a checkbox flipped mid-call must not change
            // what this call is paying for.
            think: self.deep_think,
            adherence: autoshop::recipe::DirectionAdherence::new(self.direction_adherence),
            use_looks: self.use_looks,
            // The GUI's own 「style embedding」 preference, read on the UI
            // thread like every other field above. Until this batch the
            // develop path resolved the switch itself with the preference
            // hard-coded to `false`, so this checkbox reached the index BUILD
            // and never the develop — the two could disagree with nothing said.
            embed: autoshop::style::EmbeddingSwitch::resolve(None, self.style_embed),
            weights: autoshop::style::RetrievalWeights::from_env(),
        };
        // Free-text direction ("warmer, moodier") steers the proposal; with
        // `refine` (its own button now — no pre-armed checkbox), the AI
        // ADJUSTS the current recipe instead of starting from scratch. A
        // box-selected region (if any) folds into the direction so the AI
        // masks exactly there — same prompt the web UI sends.
        let guidance = {
            let g = self.guidance.trim();
            match self.region {
                Some([l, t, r, b]) => Some(format!(
                    "The user SELECTED a target region (normalized 0..1 frame coords): \
                     left={l:.3} top={t:.3} right={r:.3} bottom={b:.3}. Apply the direction ONLY to \
                     that region — emit a mask covering it (a radial mask with those exact \
                     left/top/right/bottom bounds and feather ~0.4 is ideal, or a linear gradient \
                     for a thin edge band). Direction: {}",
                    if g.is_empty() { "make a tasteful local improvement" } else { g }
                )),
                None => (!g.is_empty()).then(|| g.to_string()),
            }
        };
        let base = refine.then(|| {
            // UNSTRIPPED: produce_recipe strips the base look + lens profile
            // from its PROMPT copy itself now, and carry_over_unrepresentable
            // needs the user's REAL profile to preserve unsaved lens toggles
            // — the pre-strip here made Refine revert them to the saved
            // profile (the same defect the web caller had, fixed in B48).
            self.recipe.clone()
        });
        // Every other network starter arms this; Analyze — the one a stalled
        // endpoint holds the longest — did not, so a hung stream left no ✕
        // and only killing the process recovered. Abandon semantics (like the
        // local-compute starters): the advisor has no cancel checkpoints, so
        // ✕ frees the UI immediately, the call dies at its stall/progress
        // deadline, and the stale-epoch result is discarded on arrival.
        let (epoch, _flag) = self.arm_cancel();
        self.analyze_inflight = true;
        self.spawn_worker(
            move || {
                let _mem = crate::budget::heavy_permit(crate::budget::estimate_mb(Some(&path))); // full-frame commit budget (budget.rs)
                // Config is reloaded in-thread (cheap) so we don't need it to be Clone.
                let cfg = autoshop::config::Config::load();
                let res = autoshop::pipeline::produce_recipe(
                    &path,
                    &cfg,
                    false,
                    guidance.as_deref(),
                    base.as_ref(),
                    req,
                    // Interactive single-photo analyze: the visual closed
                    // loop IS the R20 strengthening (cost disclosed in the
                    // button's hover text; batch surfaces pass false).
                    true,
                    autoshop::diag::stderr(),
                );
                Msg::Analyzed(epoch, Box::new(res))
            },
            move |e| Msg::Analyzed(epoch, Box::new(Err(e))),
        );
    }

    /// Reverse-fit ("match"): statistically solve the develop parameters that
    /// map the SOURCE neutral onto a finished rendition of this frame — the
    /// active「AI 生成」variant, or (R23-6) any reference file the user chose.
    /// The result lands as a new「反推」variant (base = source neutral, look in
    /// the recipe), and for a RAW the XMP sidecar is written immediately. The
    /// fit itself is deterministic and local; only the optional review calls
    /// out.
    ///
    /// The base is `source_preview`, NOT `base_preview`: after a reimagine the
    /// active variant's base IS the generated raster, and fitting a rendition
    /// onto itself would recover ~neutral. Fit must map the negative → the look.
    pub(crate) fn start_fit(&mut self) {
        let (Some(base), Some(tgt)) = (self.source_preview.clone(), self.fit_target())
        else {
            return;
        };
        if self.busy {
            return;
        }
        let src_path = self.src_path.clone();
        let fit_strength = self.panel_strength();
        let zoned = self.zoned_fit;
        let zoned_regions = if self.zoned_four_regions {
            autoshop::fit_zoned::semantic::MAX_SEMANTIC_REGIONS
        } else {
            autoshop::fit_zoned::semantic::DEFAULT_SEMANTIC_REGIONS
        };
        let ai_judge = self.fit_ai_judge;
        // The deep path IS the review, iterated — it cannot run without it,
        // and the checkbox is disabled accordingly, but the worker must not
        // depend on a UI gate for a spending decision.
        let deep = self.fit_deep && ai_judge;
        let lang = self.lang;
        self.busy = true;
        let mut running = if zoned {
            tr(lang, "Reverse-fitting… (global + semantic/ranges + spatial tiles)").to_string()
        } else {
            tr(lang, "Reverse-fitting… (statistical fit, local compute)").to_string()
        };
        if deep {
            running.push_str(tr(lang, " · deep: AI review BEFORE saving, up to one guided retry"));
        } else if ai_judge {
            // The review runs AFTER the local fit and is a network call — the
            // busy line must own that extra wait up front.
            running.push_str(tr(lang, " · then AI review (vision call)"));
        }
        self.status = running;
        self.spawn_worker(
            move || {
                // Full-frame commit budget (budget.rs); the reference's own
                // header raises the floor when it is the bigger file.
                let _mem = crate::budget::heavy_permit(crate::budget::estimate_mb(Some(&tgt)));
                let res = (|| -> anyhow::Result<FitOutcome> {
                    // THE raw-vs-baked dispatch (R22-1): the reference may now
                    // be any file the user picked, including a RAW, which
                    // `decode::load_image` refuses by name. `source_pixels`
                    // develops a RAW neutrally and loads a baked file — the
                    // one branch, never hand-copied. The cap is comfortably
                    // above both consumers (the fit analyses at 384, the judge
                    // at 1024) and preserves aspect, which the same-frame
                    // check reads.
                    const FIT_REF_EDGE: u32 = 2048;
                    let target = autoshop::render::source_pixels(&tgt, Some(FIT_REF_EDGE))?;
                    // The gate runs at PERSIST time (below), not up front:
                    // a pre-fit snapshot left the whole multi-minute
                    // segmentation as a race window — an explicit save
                    // landing during the fit was then overwritten
                    // unversioned. (Claimed unique rasters already made the
                    // fit itself harmless to the saved develop.)
                    // Automatic local pass only when enabled AND the photo
                    // has a real path (the semantic raster needs a home).
                    // Segmentation success produces semantic sky/land; an
                    // unavailable sidecar falls through to native luminance
                    // ranges inside fit_recipe_zoned_with. The raster
                    // gets a FRESH unique name per fit (mask-zone-sky.png,
                    // -2, -3, …, create_new-claimed like every master name):
                    // the old fixed name was rewritten IN PLACE before the
                    // recipe landed, so a crash or a failed recipe write left
                    // the still-live saved recipe pointing at the NEW bytes —
                    // the same shared-file corruption class that once made
                    // the fit's cleanup delete 「AI select sky」's raster.
                    // With unique names the saved develop's raster is never
                    // touched, so the pass no longer needs the backup gate;
                    // superseded rasters stay on disk (tiny greyscale PNGs —
                    // v<N> snapshots freeze their own copies regardless).
                    // R15/R16: the photo's calibration composes INTO the
                    // solve — the recipe starts from the calibration-only
                    // base, every closed-loop candidate render is the
                    // canvas's own one-pass user(base(x)), and the
                    // deliverable carries the calibration by construction
                    // (no separate stamp, no two-pass seed render). Fitting
                    // from the raw neutral burned the bounded model on
                    // re-deriving the 0.6–1.4 EV camera look (real pair:
                    // saturation pegged, cast curves vetoed, a helpful sky
                    // zone dropped); the v0.24.0 pre-rendered seed then
                    // clipped saturated channels at the pass boundary —
                    // both retired by pipeline::calibration_recipe +
                    // fit_recipe_from.
                    let fit_base = src_path
                        .as_deref()
                        .map(|p| {
                            autoshop::pipeline::calibration_recipe(
                                autoshop::pipeline::fit_calibration(p),
                            )
                        })
                        .unwrap_or_default();
                    // ONE judge call site for both orderings (R23-6): the
                    // default path reviews AFTER the persist, the deep path
                    // reviews BEFORE it and again after its retry, and a
                    // second hand-written copy of "encode both frames at
                    // detail:high tile size and ask FitMatch" is exactly how
                    // the two would drift into judging different pixels.
                    // Domain: `rep.recipe` renders from the source NEUTRAL
                    // (`base` — the room the composed fit solved in), so that
                    // render against the target is the pair the fit's own
                    // residual measures.
                    const JUDGE_EDGE: u32 = 1024; // detail:high tiles at 512 px — 4 tiles read a grade
                    let judge_of = |recipe: &autoshop::recipe::EditRecipe|
                     -> anyhow::Result<autoshop::advisor::Judgement> {
                        let cfg = autoshop::config::Config::load();
                        let enc = |img: &image::DynamicImage| -> anyhow::Result<Vec<u8>> {
                            let mut j = Vec::new();
                            img.write_to(
                                &mut std::io::Cursor::new(&mut j),
                                image::ImageFormat::Jpeg,
                            )?;
                            Ok(j)
                        };
                        let fitted = autoshop::render::develop_preview(
                            &base.thumbnail(JUDGE_EDGE, JUDGE_EDGE),
                            recipe,
                        );
                        let t_jpeg = enc(&target.thumbnail(JUDGE_EDGE, JUDGE_EDGE))?;
                        let f_jpeg = enc(&fitted)?;
                        Ok(autoshop::advisor::judge_pair(
                            &cfg,
                            autoshop::advisor::JudgeImages {
                                reference: &t_jpeg,
                                candidate: &f_jpeg,
                            },
                            autoshop::advisor::JudgeTask::FitMatch,
                            None,
                            // FitMatch scores a MATCH between two renders —
                            // the strength axis has no bearing on it (R23-3).
                            None,
                        )?)
                    };
                    // Returns the raster this run CLAIMED alongside the report:
                    // `claim_raster` hands out a fresh `mask-zone-sky[-N].png`
                    // and only the recipe that keeps the solve ever references
                    // it, so a discarded candidate would leave a zero-reference
                    // file behind (R23 review LOW-5). The claimed PATH, never a
                    // rebuilt name — the claim may be the `-2` / `-3` suffix.
                    // 完全自动 (user ruling 2026-08-26): the fit's D gate may
                    // consult the local DIFT sidecar on content-divergent
                    // pairs; failures degrade into the rationale.
                    let corr = autoshop::correspond::fit_provider(
                        autoshop::correspond::CorrespondOpts::from_config(
                            &autoshop::config::Config::load(),
                        ),
                    );
                    let run_zoned = |seg_on: bool|
                     -> anyhow::Result<(autoshop::fit::FitReport, Option<std::path::PathBuf>)> {
                        Ok(match (seg_on, &src_path) {
                            (true, Some(p)) => {
                                let cfg = autoshop::config::Config::load();
                                let seg =
                                    autoshop::segment::SegmentOpts::from_config(&cfg, "sky");
                                let mask =
                                    autoshop::store::OwnedRaster::claim(p, "mask-zone-sky")?;
                                let rep = autoshop::fit_zoned::fit_recipe_zoned_with_regions(
                                    &base,
                                    &target,
                                    &seg,
                                    &mask,
                                    &fit_base,
                                    autoshop::fit::FitOptions {
                                        strength: fit_strength,
                                        provider: Some(&corr),
                                    },
                                    zoned_regions,
                                );
                                (rep, Some(mask.into_path()))
                            }
                            _ => (
                                autoshop::fit::fit_recipe_from_with(
                                    &base,
                                    &target,
                                    &fit_base,
                                    autoshop::fit::FitOptions {
                                        strength: fit_strength,
                                        provider: Some(&corr),
                                    },
                                ),
                                None,
                            ),
                        })
                    };
                    // The FIRST solve's claim is deliberately not swept: this
                    // report is the one that ships unless a candidate beats it,
                    // and a candidate can only be the zoned pass when this solve
                    // was NOT zoned (`hint_action`'s `zoned_used` fact), i.e.
                    // when it claimed nothing at all.
                    let (mut rep, _first_claim) = run_zoned(zoned)?;
                    // FACTS, not prose (L12#4): the landing renders these
                    // with the language live when the result LANDS — the old
                    // trf calls here used the language captured at spawn,
                    // minutes stale after a segmentation run.
                    let mut status: Vec<FitNote> = Vec::new();
                    // ── the DEEP path (R23-6 D). Everything here happens
                    // BEFORE the persist below, which is the whole point and
                    // the deliberate revision of R20's ordering — recorded at
                    // the persist site.
                    //
                    // Discipline, copied wholesale from `visual_review_round`
                    // (pipeline.rs) because it is the shape that already
                    // survived review: the retry must re-score AT LEAST as
                    // high to be kept, an action that changes nothing
                    // short-circuits before spending a second call, and every
                    // failure keeps the plain solve and degrades to a note.
                    //
                    // `judged` is THREE-state, the same shape the CLI's
                    // `--deep` path uses: `None` = the deep path never ran,
                    // `Some(Ok(_))` = a verdict describing what ships,
                    // `Some(Err(_))` = the deep review ran and FAILED. The last
                    // one used to be indistinguishable from "never ran", so the
                    // informational block below bought a THIRD network attempt
                    // after two had already failed — past the two-paid-calls
                    // ceiling its own tooltip states (R23 review LOW-6).
                    let mut judged: Option<anyhow::Result<autoshop::advisor::Judgement>> = None;
                    if deep {
                        match judge_of(&rep.recipe) {
                            Ok(first) => {
                                let action = hint_action(
                                    first.hint.as_deref().unwrap_or(""),
                                    !rep.recipe.masks.is_empty(),
                                    !zoned && src_path.is_some(),
                                );
                                let candidate = match action {
                                    // The one action that needs the solver
                                    // again. A claim/segmentation failure is
                                    // not an error here: the plain solve in
                                    // hand is already a valid result.
                                    FitAction::Zoned => run_zoned(true).ok(),
                                    FitAction::Saturation(d) => {
                                        let mut r = rep.recipe.clone();
                                        r.saturation += d;
                                        r.clamp();
                                        // Identical settings ⇒ nothing to
                                        // judge, and no second call bought.
                                        (r.saturation != rep.recipe.saturation).then(|| {
                                            // RE-DERIVED, never cloned off the
                                            // solve (R23 review MED-3): every
                                            // outcome note — the residual pair,
                                            // the joint numbers, the
                                            // unrepresented-controls diagnosis,
                                            // the terminal-reset verdict the
                                            // FitReset check below reads — is a
                                            // statement about the recipe, and
                                            // this recipe is not the one the
                                            // solver reported on.
                                            (
                                                autoshop::fit::rescore_report(
                                                    &base,
                                                    &target,
                                                    &r,
                                                    rep.err_before,
                                                    &rep.notes,
                                                ),
                                                None,
                                            )
                                        })
                                    }
                                    FitAction::None => None,
                                };
                                let outcome = match candidate {
                                    Some((mut cand, claimed)) => {
                                        let verdict = judge_of(&cand.recipe);
                                        let keep = matches!(
                                            &verdict,
                                            Ok(second) if second.score >= first.score
                                        );
                                        if !keep {
                                            // The candidate is gone, so its
                                            // claimed raster is now referenced
                                            // by nothing (R23 review LOW-5).
                                            // ONLY this run's claim is touched,
                                            // by the path `claim_raster`
                                            // returned. Best effort by design:
                                            // a failure leaves an inert
                                            // greyscale PNG the user can delete,
                                            // and reporting it would be noise on
                                            // a fit that succeeded.
                                            if let Some(p) = &claimed {
                                                let _ = std::fs::remove_file(p);
                                            }
                                        }
                                        match verdict {
                                            Ok(second) if keep => {
                                                autoshop::rationale::push_note(
                                                    &mut cand.recipe.rationale,
                                                    &mut cand.notes,
                                                    autoshop::rationale::Note::new(
                                                        autoshop::rationale::keys::FIT_NOTE_DEEP_ADOPTED,
                                                        vec![
                                                            ("score1", format!("{:.0}", first.score)),
                                                            ("score2", format!("{:.0}", second.score)),
                                                            ("action", action.tag().to_string()),
                                                        ],
                                                    ),
                                                );
                                                rep = cand;
                                                judged = Some(Ok(second));
                                                DeepFitOutcome::Adopted
                                            }
                                            // Lower, or un-judgeable: the
                                            // plain solve stands, and the
                                            // FIRST verdict is the one that
                                            // describes what ships.
                                            Ok(_) => {
                                                judged = Some(Ok(first));
                                                DeepFitOutcome::Discarded
                                            }
                                            Err(e) => {
                                                status.push(FitNote::AiReviewFailed(e.to_string()));
                                                judged = Some(Ok(first));
                                                DeepFitOutcome::Discarded
                                            }
                                        }
                                    }
                                    // No candidate, and WHY differs (R23
                                    // review LOW-4): `FitAction::None` is the
                                    // reviewer having named nothing this app
                                    // can do, while a selected action that
                                    // produced no candidate is the app failing
                                    // to carry out a move it did choose — the
                                    // zoned re-solve errored, or the saturation
                                    // step clamped back to the value in hand.
                                    None => {
                                        judged = Some(Ok(first));
                                        match action {
                                            FitAction::None => DeepFitOutcome::NothingActionable,
                                            _ => DeepFitOutcome::ActionDidNotRun,
                                        }
                                    }
                                };
                                status.push(FitNote::DeepFit { action: action.tag(), outcome });
                            }
                            Err(e) => {
                                status.push(FitNote::AiReviewFailed(e.to_string()));
                                judged = Some(Err(e));
                            }
                        }
                    }
                    if rep.recipe.masks.iter().any(|mask| {
                        mask.range.is_none()
                            && matches!(mask.role, MaskRole::ZoneSky | MaskRole::ZoneLand)
                    }) {
                        status.push(FitNote::IncludesSkyZone);
                    }
                    if rep.recipe.masks.iter().any(|mask| mask.range.is_some()) {
                        status.push(FitNote::IncludesRangeMasks);
                    }
                    let tiles = rep
                        .notes
                        .iter()
                        .filter(|note| note.key == autoshop::rationale::keys::TILE_ATTACHED)
                        .count();
                    if tiles > 0 {
                        status.push(FitNote::IncludesSpatialTiles(tiles));
                    }
                    let refinement_kept = rep
                        .notes
                        .iter()
                        .filter(|note| {
                            note.key == autoshop::rationale::keys::MASK_REFINEMENT_KEPT
                        })
                        .count();
                    let refinement_abstained = rep
                        .notes
                        .iter()
                        .filter(|note| {
                            note.key == autoshop::rationale::keys::MASK_REFINEMENT_ABSTAINED
                        })
                        .count();
                    if refinement_kept + refinement_abstained > 0 {
                        status.push(FitNote::MaskRefinement {
                            kept: refinement_kept,
                            abstained: refinement_abstained,
                        });
                    }
                    // R23-6 A-3: the terminal do-no-harm reset is "the
                    // reverse-fit did nothing", and a line inside a rationale
                    // block is not where a user finds that out.
                    if rep.notes.iter().any(|n| n.key == autoshop::rationale::keys::FIT_NOTE_REGRESSED)
                    {
                        status.push(FitNote::FitReset);
                    }
                    if rep
                        .notes
                        .iter()
                        .any(|n| n.key == autoshop::rationale::keys::FIT_NOTE_NOT_SAME_FRAME)
                    {
                        status.push(FitNote::ReferenceNotSameFrame);
                    }
                    let mut persisted = false;
                    if let Some(p) = &src_path {
                        // Worker thread ⇒ Wait, ONE lock across gate →
                        // recipe → pixel-link clear → XMP: still the
                        // narrowest gate window, now with no other process
                        // interleaving between the halves. The
                        // identical-content skip keeps the gate from
                        // spamming versions on repeated fits.
                        let persist = autoshop::store::with_develop_lock(
                            p,
                            autoshop::store::DevelopLockMode::Wait,
                            || -> std::io::Result<()> {
                        let backed = autoshop::store::backup_saved_develop(p, Some(&rep.recipe));
                        // Persist the fit losslessly: recipe.json carries the
                        // bitmap zone masks + recolour gains the XMP cannot,
                        // so reopening the photo restores the whole fit. The
                        // snapshot above is the gate — if it FAILED, skip the
                        // persist entirely (the fit stays on the canvas with
                        // ● unsaved; Ctrl+S overwrites explicitly).
                        match &backed {
                            Ok(backed) => {
                              // ONE single-generation commit (the Ctrl+S
                              // rule, L03): the fit's recipe and the
                              // pixel-link CLEAR land together. The fit is a
                              // SOURCE-based develop by construction (it maps
                              // the source neutral onto the rendition), and a
                              // stale pixels.json link surviving the recipe
                              // write made a reopen render the fit on baked
                              // pixels it was never computed against.
                              let commit_res: anyhow::Result<()> = (|| {
                                  autoshop::store::commit_develop(
                                      p,
                                      autoshop::store::DevelopCommit {
                                          recipe: Some(autoshop::pipeline::recipe_store_bytes(
                                              p,
                                              &rep.recipe,
                                              autoshop::diag::stderr(),
                                          )?),
                                          pixels: autoshop::store::CommitMember::Clear,
                                          // R24-4: this worker publishes a
                                          // REVERSE-FIT into the active slot,
                                          // and the strip record's active
                                          // half has to say so — left `Keep`,
                                          // a photo whose record read
                                          // 「original」 reopened as 「▣ 原片」
                                          // holding the fit. Worker thread:
                                          // no live strip to hand over, so
                                          // the shared primitive restates the
                                          // one fact this writer owns (the
                                          // card's id/name/position stand).
                                          variants: autoshop::store::variants_member(
                                              p,
                                              autoshop::store::ActiveWrite::Kind("fitted"),
                                          )?,
                                      },
                                  )?;
                                  Ok(())
                              })();
                              match commit_res {
                              Err(e) => {
                                // A failed store write must NOT discard the
                                // minutes of segmentation + fitting that
                                // already succeeded — same degrade shape as
                                // the backup-gate branch below: the fit lands
                                // on the canvas unsaved.
                                status.push(FitNote::NotPersistedCommit(e.to_string()));
                              }
                              Ok(()) => {
                                persisted = true;
                                if autoshop::decode::is_raw(p) {
                                    // The recipe write ALONE decides the saved
                                    // state (same rule as Ctrl+S / Analyze):
                                    // an XMP failure must not collapse an
                                    // already-persisted fit into an error —
                                    // reopening WOULD restore it, so the UI
                                    // must agree that it saved.
                                    // The M6a list is dropped HERE: the zoned
                                    // fit's own rationale already states that
                                    // its corrections are bitmap masks the
                                    // Lightroom sidecar cannot carry (the
                                    // ZONE_ATTACHED note), so a second copy
                                    // would say it twice for the same masks.
                                    match autoshop::pipeline::write_xmp(p, &rep.recipe, autoshop::diag::stderr()) {
                                        Ok((x, merge_note, _)) => {
                                            status.push(FitNote::XmpWritten(x));
                                            // Regenerated-not-merged: same
                                            // disclosure as Ctrl+S.
                                            if let Some(m) = merge_note {
                                                status.push(FitNote::XmpMergeNote(m));
                                            }
                                        }
                                        Err(e) => {
                                            status.push(FitNote::XmpFailed(e.to_string()));
                                        }
                                    }
                                }
                                if let Some(n) = backed {
                                    status.push(FitNote::BackedUpAs(*n));
                                }
                              }
                              }
                            }
                            Err(e) => {
                                status.push(FitNote::NotPersistedBackup(e.to_string()));
                            }
                        }
                                Ok(())
                            },
                        );
                        if let Err(e) = persist {
                            status.push(FitNote::NotPersistedLock(e.to_string()));
                        }
                    }
                    // Opt-in AI review (LLM-as-a-judge): the vision model
                    // scores how faithfully the fitted render matches the
                    // target look.
                    //
                    // ORDERING, and its history. R20 decided this review runs
                    // AFTER the persist on purpose — informational, never
                    // gating, delaying or failing the fit's landing, every
                    // failure degrading to a status note (docs/ROADMAP.md's
                    // R20 entry; Opus S5 定案). That is still the DEFAULT
                    // path below, unchanged to the byte.
                    //
                    // R23-6 revises it for one explicitly-chosen case: with
                    // 「deep」 ticked the user has asked the reviewer to act,
                    // which it cannot do after the recipe is on disk. The deep
                    // block above therefore reviews BEFORE the persist and may
                    // buy ONE bounded guided retry (user decision 2026-08-17
                    // ⑥). The R20 rule is not repealed — it is now the
                    // behaviour of the unticked box, which is also the
                    // default, and the historical entry stays as written.
                    //
                    // Domain: rep.recipe renders from the source NEUTRAL
                    // (source_preview — the room the composed fit solved in),
                    // so that render vs the target is exactly the pair the
                    // fit's own residual measures.
                    if ai_judge {
                        // The two-paid-calls ceiling, decided by a pure
                        // function so it is a pinned property and not a shape
                        // one has to re-read this closure to check
                        // (R23 review LOW-6).
                        let verdict = match FitReviewPlan::of(&judged) {
                            FitReviewPlan::Reuse => judged,
                            FitReviewPlan::Skip => None,
                            FitReviewPlan::Call => Some(judge_of(&rep.recipe)),
                        };
                        match verdict {
                            Some(Ok(j)) => status.push(FitNote::AiReview {
                                score: j.score,
                                critique: j.critique,
                                // R23-6: SHOWN, never executed on this path.
                                // R20 dropped it silently and the user paid
                                // for it.
                                hint: j.hint,
                            }),
                            Some(Err(e)) => status.push(FitNote::AiReviewFailed(e.to_string())),
                            None => {}
                        }
                    }
                    Ok(FitOutcome {
                        err_before: rep.err_before,
                        err_after: rep.err_after,
                        rationale_notes: rep.notes,
                        recipe: rep.recipe,
                        status,
                        persisted,
                    })
                })();
                Msg::Fitted(Box::new(res))
            },
            |e| Msg::Fitted(Box::new(Err(e))),
        );
    }
}
