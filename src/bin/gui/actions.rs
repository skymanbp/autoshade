//! App actions: open/variants/versions/history/quit flow.

use super::*;

impl AutoshopApp {
    /// Restore persisted prefs (last folder, view mode, export options) and
    /// re-open the library the user was browsing. Window geometry itself is
    /// restored by eframe's own persistence layer.
    pub(crate) fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self::default();
        if let Some(prefs) =
            cc.storage.and_then(|s| eframe::get_value::<Prefs>(s, eframe::APP_KEY))
        {
            app.style_strength = prefs.style_strength.clamp(0.0, 1.0);
            app.save_jpeg = prefs.save_jpeg;
            app.save_denoise = prefs.save_denoise;
            app.zoned_fit = prefs.zoned_fit;
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
        let text = format!("{what}: {e}");
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
            return;
        }
        // Flush a typed-but-uncommitted mask rename before the stash below
        // snapshots the recipe — a thumbnail click / Ctrl+O / arrow-key nav
        // with the name box focused silently stashed the OLD name (U10).
        self.commit_mask_name_buf();
        // …and the buffer itself dies with the photo: carried across, a
        // same-index mask on the NEXT photo skipped the reseed and the box
        // showed the previous photo's name text (M15).
        self.mask_name_buf = None;
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
            // Compared BOTH ways: a canvas that dropped its baked master
            // (undo back to source) is as unsaved as one that gained it —
            // without the stash, reopening would resurrect the disk pixels.
            // The WHOLE identity counts, generated flag included: the
            // classification decides whether calibration is stripped on
            // restore, so a flag drift is as unsaved as a path drift.
            let recorded = autoshop::store::read_pixel_source(&old);
            let live_generated = self.active_is_generated();
            let pixels_unsaved = !(same_master_opt(
                recorded.as_ref().map(|(p, _)| p.as_path()),
                origin.as_deref(),
            ) && recorded.as_ref().map(|(_, g)| *g)
                == origin.as_ref().map(|_| live_generated));
            // Background variants count too (H4): their unsaved work has no
            // sidecar to survive in, and the strip used to collapse to the
            // active canvas alone on navigation. THIS photo's strip only:
            // the cross-photo sum (inactive_dirty_variants) belongs to the
            // quit dialog, and using it here chain-stashed clean photos.
            let background_dirty = self.open_dirty_variants() > 0;
            if dirty_vs(&self.recipe, &self.saved_recipe) || pixels_unsaved || background_dirty
            {
                let others: Vec<StashedVariant> = self
                    .variants
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != self.active)
                    .map(|(_, v)| StashedVariant {
                        kind: v.kind,
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
                    let baked = autoshop::store::read_pixel_source(&path).and_then(
                        |(origin, generated)| {
                            let img = match autoshop::decode::load_image(&origin) {
                                Ok(i) => i,
                                Err(e) => {
                                    // Disclosed, not silent: "the canvas came
                                    // back un-retouched" must have a traceable
                                    // cause in the log.
                                    eprintln!(
                                        "⚠ baked master {} failed to decode ({e}) — opening the un-retouched source",
                                        origin.display()
                                    );
                                    return None;
                                }
                            };
                            Some((Arc::new(img.thumbnail(edge, edge)), origin, generated))
                        },
                    );
                    // Arc once here so every downstream sharer (variants, the
                    // preview worker) is an O(1) refcount bump, not a deep copy.
                    Ok((Arc::new(thumb), knots, lens, as_shot, baked, src_ident))
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

    /// How many BACKGROUND variants hold work that quitting would discard.
    ///
    /// Dirty means: the live strip minus the active card differs from the
    /// photo's persisted `variants.json` mirror ([`Self::saved_strip`]) — in
    /// kind, recipe, raster origin, count, or in the active card's own
    /// identity (kind/position, which the record also persists). A `None`
    /// mirror means nothing is persisted, so every background card is
    /// unsaved work. The OPEN photo's strip only — the per-photo half of
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
        let live: Vec<&Variant> = self
            .variants
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != self.active)
            .map(|(_, v)| v)
            .collect();
        let Some(rec) = &self.saved_strip else {
            return live.len();
        };
        let mut n = 0usize;
        for (i, v) in live.iter().enumerate() {
            let matches = rec.others.get(i).is_some_and(|e| {
                VariantKind::from_store_str(&e.kind) == Some(v.kind)
                    && !dirty_vs(&v.recipe, &e.recipe)
                    && same_master_opt(v.origin.as_deref(), e.origin.as_deref())
            });
            if !matches {
                n += 1;
            }
        }
        // Cards persisted but no longer live: the deletion is unsaved too —
        // quitting now would resurrect them on the next open.
        n += rec.others.len().saturating_sub(live.len());
        // Active-card identity drift: a changed kind or position reopens as
        // a different strip even when every background card matches.
        let ak = self.active_variant().map_or(VariantKind::Original, |v| v.kind);
        if VariantKind::from_store_str(&rec.active_kind) != Some(ak)
            || rec.active_pos != self.active
        {
            n = n.max(1);
        }
        n
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
    /// the strip is trivial (one Original card — recipe.json + pixels.json
    /// already say everything, and a record would be pure sidecar noise).
    pub(crate) fn current_strip_record(&self) -> Option<autoshop::store::VariantsRecord> {
        let ak = self.active_variant().map_or(VariantKind::Original, |v| v.kind);
        if self.variants.len() <= 1 && ak == VariantKind::Original {
            return None;
        }
        Some(autoshop::store::VariantsRecord {
            v: 1,
            active_kind: ak.store_str().to_string(),
            active_pos: self.active,
            others: self
                .variants
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != self.active)
                .map(|(_, v)| autoshop::store::VariantEntry {
                    kind: v.kind.store_str().to_string(),
                    recipe: v.recipe.clone(),
                    origin: v.origin.clone(),
                })
                .collect(),
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
        // involved. Memo-bounded; era-2 recipes short-circuit; a Generated
        // entry is skipped like every other ordering site (its curve is
        // empty by invariant).
        // Synchronous first-click cost: the same accepted class as the open
        // gate — one estimate per photo per process when it succeeds, and a
        // transient inability early-exits on the failed probe and retries on
        // the next read.
        if vkind != VariantKind::Generated
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
            // (empty for legacy / fit recipes — deliberate). This covers
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
        // variant it was typed on (M15).
        self.mask_name_buf = None;
        self.overlay_ref = None;
        self.overlay_stale = true;
        self.last_rgb = None; // the retained frame belongs to the OLD variant
        self.disarm_tools();
        self.clone_src = None; // unlike a mere disarm, a variant switch drops the sample
        self.zoom = 1.0;
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
        if let Some(cur) = self.variants.get_mut(self.active) {
            cur.recipe = self.recipe.clone(); // don't lose the edits in progress
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
        // switch's M15 boundary clear then discarded the typed name. A
        // USER-initiated switch keeps the deliberate M15 drop — this flush
        // is the async thief's, committed into the recipe snapshotted below.
        self.commit_mask_name_buf();
        if let Some(cur) = self.variants.get_mut(self.active) {
            cur.recipe = self.recipe.clone();
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
        if let Some((target, erase)) = self.mask_brush.take() {
            match target.map(&f) {
                None => self.mask_brush = Some((None, erase)),
                Some(Some(j)) => self.mask_brush = Some((Some(j), erase)),
                Some(None) => {
                    self.mask_brush_gray = None;
                    self.paint_mode = false;
                    self.paint_last = None;
                    self.clear_mask();
                }
            }
        }
    }

    /// Rescan the photo's develop dir for version snapshots (cached in
    /// `self.versions`; called on photo open and after saving a version — NOT
    /// every frame).
    pub(crate) fn refresh_versions(&mut self) {
        self.versions.clear();
        let Some(src) = self.src_path.as_deref() else { return };
        self.versions = autoshop::store::list_versions(src);
    }

    /// Save the CURRENT develop as the next numbered version snapshot.
    pub(crate) fn save_version(&mut self) {
        // A typed-but-uncommitted mask rename belongs in the snapshot —
        // every persistence entry point flushes it (the U10 rule; L27).
        self.commit_mask_name_buf();
        let lang = self.lang;
        let Some(src) = self.src_path.clone() else { return };
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
        let res = autoshop::pipeline::write_recipe(&src, &snap, Some(vpath.clone()));
        if res.is_err() {
            // Release the claimed slot AND this call's frozen rasters — an
            // empty version must not pollute the list after a failed write.
            autoshop::store::rollback_frozen_rasters(&dev, n);
            let _ = std::fs::remove_file(&vpath);
        }
        match res {
            Ok(p) => {
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
                if self.active_is_generated() {
                    r.base_curve = Vec::new();
                    r.lens_profile = Default::default();
                    // The anchor too: its pixels carry a BAKED white balance,
                    // so an absolute-Kelvin claim over them would be false —
                    // generated canvases keep the relative 5500 model.
                    r.as_shot_k = None;
                    r.as_shot_tint = None;
                }
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
                self.recipe = r;
                self.resync_recipe_display();
                self.dirty = true;
                let loaded = trf(lang, "Loaded version v{n} — Ctrl+Z returns to before the load", &[("n", &n.to_string())]);
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
        self.crop_mode = false;
        self.paint_mode = false;
        self.clone_mode = false;
        self.wb_picking = false;
        self.range_picking = None;
        self.placing_mask = None;
        self.place_start = None;
        self.crop_drag = None;
        self.mask_drag = None;
        self.paint_last = None;
        // The mask-brush session dies with its paint mode (Esc = cancel):
        // strokes live in the canvas + gray buffer only until 「Apply」bakes
        // them into a claimed raster, so dropping both IS the cancel. The
        // canvas is cleared too — fill/heal would otherwise inherit the
        // brush-mask strokes as a phantom retouch selection.
        if self.mask_brush.take().is_some() {
            self.mask_brush_gray = None;
            self.clear_mask();
        }
        // A preset change armed for the crop tool must die with it — the
        // flag survived Esc / Done / a hold-B detour and rewrote the box as
        // a surprise on the next crop entry (CX5-4).
        self.crop_aspect_pending = false;
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
            && !self
                .variants
                .get(self.active)
                .is_some_and(|v| v.kind == VariantKind::Generated)
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
            self.mask_paint = Some(image::RgbaImage::new(mw, mh));
            self.mask_tex = None;
            self.mask_dirty = false;
            self.paint_last = None;
        }
        self.overlay_ref = None;
        self.overlay_stale = true;
        self.last_rgb = None;
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
        // master link exactly like Ctrl+S would.
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
            // Both directions count (gained OR dropped master) — see open_path.
            let recorded = autoshop::store::read_pixel_source(&p);
            let live_generated = self.active_is_generated();
            let pixels_unsaved = !(same_master_opt(
                recorded.as_ref().map(|(q, _)| q.as_path()),
                origin.as_deref(),
            ) && recorded.as_ref().map(|(_, g)| *g)
                == origin.as_ref().map(|_| live_generated));
            // The strip counts as unsaved work of the OPEN photo too — a
            // clean canvas over an unpersisted strip must still be listed,
            // or Save-all had nothing to write and the guard's re-check
            // bounced the close forever (the v0.21 dead-button livelock).
            if dirty_vs(&self.recipe, &self.saved_recipe)
                || pixels_unsaved
                || self.open_dirty_variants() > 0
            {
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
                    ui.add_space(6.0);
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
                ui.add_space(6.0);
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
                    let recipe_bytes = autoshop::pipeline::recipe_store_bytes(p, &disk)?;
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
                    match autoshop::pipeline::write_xmp(p, &disk) {
                        Ok((_, None)) => {}
                        // A regenerated (unmerged) sidecar loses LR-only
                        // properties — Save-all's warning list is exactly
                        // where that belongs (round-12 disclosure threading).
                        Ok((_, Some(n))) => {
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
                let n = autoshop::store::migrate_legacy_from_many(&dir, &photos);
                Msg::LegacyImported(Ok(trf(
                    lang,
                    "Imported saved develops for {n} photo(s) from {path}",
                    &[("n", &n.to_string()), ("path", &dir.display().to_string())],
                )))
            },
            |e| Msg::LegacyImported(Err(e)),
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
        let style = self.style_strength;
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
                // Config is reloaded in-thread (cheap) so we don't need it to be Clone.
                let cfg = autoshop::config::Config::load();
                let res = autoshop::pipeline::produce_recipe(
                    &path,
                    &cfg,
                    false,
                    guidance.as_deref(),
                    base.as_ref(),
                    style,
                );
                Msg::Analyzed(epoch, Box::new(res))
            },
            move |e| Msg::Analyzed(epoch, Box::new(Err(e))),
        );
    }

    /// Reverse-fit ("match"): statistically solve the develop parameters that map
    /// the SOURCE neutral onto the active「AI 生成」variant — the result lands as
    /// a new「反推」variant (base = source neutral, look in the recipe), and for a
    /// RAW the XMP sidecar is written immediately. Deterministic, no API call.
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
        let zoned = self.zoned_fit;
        let lang = self.lang;
        self.busy = true;
        self.status = if zoned {
            tr(lang, "Reverse-fitting… (statistical fit + sky segmentation; first run downloads the model)").into()
        } else {
            tr(lang, "Reverse-fitting… (statistical fit, local compute)").into()
        };
        self.spawn_worker(
            move || {
                let res = (|| -> anyhow::Result<(EditRecipe, String, bool)> {
                    let target = autoshop::decode::load_image(&tgt)?;
                    // The gate runs at PERSIST time (below), not up front:
                    // a pre-fit snapshot left the whole multi-minute
                    // segmentation as a race window — an explicit save
                    // landing during the fit was then overwritten
                    // unversioned. (Claimed unique rasters already made the
                    // fit itself harmless to the saved develop.)
                    // Zoned sky pass only when enabled AND the photo has a
                    // real path (the mask raster needs a home). The raster
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
                    let rep = match (zoned, &src_path) {
                        (true, Some(p)) => {
                            let cfg = autoshop::config::Config::load();
                            let seg =
                                autoshop::segment::SegmentOpts::from_config(&cfg, "sky");
                            let mask = autoshop::store::claim_raster(p, "mask-zone-sky")?;
                            autoshop::fit_zoned::fit_recipe_zoned(&base, &target, &seg, &mask)
                        }
                        _ => autoshop::fit::fit_recipe(&base, &target),
                    };
                    let mut note = trf(
                        lang,
                        "Reverse-fit done: look residual {before}→{after} · created a「Reverse-fit」variant (editable / XMP / full-res)",
                        &[
                            ("before", &format!("{:.3}", rep.err_before)),
                            ("after", &format!("{:.3}", rep.err_after)),
                        ],
                    );
                    if !rep.recipe.masks.is_empty() {
                        note.push_str(tr(
                            lang,
                            " · includes sky-zone correction (adjustable in the mask panel; XMP carries the global part only)",
                        ));
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
                                          )?),
                                          pixels: autoshop::store::CommitMember::Clear,
                                          variants: autoshop::store::CommitMember::Keep,
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
                                note.push_str(&trf(
                                    lang,
                                    " · NOT persisted: saving the develop failed ({err}) — Ctrl+S to save explicitly",
                                    &[("err", &e.to_string())],
                                ));
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
                                    match autoshop::pipeline::write_xmp(p, &rep.recipe) {
                                        Ok((x, merge_note)) => {
                                            note.push_str(&format!(" · XMP → {}", x.display()));
                                            // Regenerated-not-merged: same
                                            // disclosure as Ctrl+S.
                                            if let Some(m) = merge_note {
                                                note.push_str(&format!(" · ⚠ {m}"));
                                            }
                                        }
                                        Err(e) => {
                                            note.push_str(" · ");
                                            note.push_str(&trf(
                                                lang,
                                                "recipe saved — but the Lightroom XMP failed: {err}",
                                                &[("err", &e.to_string())],
                                            ));
                                        }
                                    }
                                }
                                if let Some(n) = backed {
                                    note.push_str(&trf(
                                        lang,
                                        " · previous save backed up as v{n}",
                                        &[("n", &n.to_string())],
                                    ));
                                }
                              }
                              }
                            }
                            Err(e) => {
                                note.push_str(&trf(
                                    lang,
                                    " · NOT persisted: backing up your existing save failed ({err}) — Ctrl+S to save explicitly",
                                    &[("err", &e.to_string())],
                                ));
                            }
                        }
                                Ok(())
                            },
                        );
                        if let Err(e) = persist {
                            note.push_str(&trf(
                                lang,
                                " · NOT persisted: the develop store could not be locked ({err}) — Ctrl+S to save explicitly",
                                &[("err", &e.to_string())],
                            ));
                        }
                    }
                    Ok((rep.recipe, note, persisted))
                })();
                Msg::Fitted(Box::new(res))
            },
            |e| Msg::Fitted(Box::new(Err(e))),
        );
    }
}
