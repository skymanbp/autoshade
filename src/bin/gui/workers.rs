//! Background workers: spawn, master loads, the Msg pump.

use super::*;

impl AutoshopApp {
    /// Spawn a worker whose PANIC still delivers a terminal message. Every
    /// worker's last act is sending the `Msg` that clears `busy` (or an
    /// inflight counter); a panic before that send unwinds the thread, the
    /// message never arrives, and the whole app soft-locks — every action
    /// gates on `!busy`, and only killing the process recovers. One decode
    /// panic inside `rawler`/`image` on a malformed file is enough. So: run
    /// the body under `catch_unwind` and synthesize the site's failure `Msg`
    /// from the panic payload. `AssertUnwindSafe` is sound here — all
    /// captured state is moved in and dropped on unwind; the UI only ever
    /// observes the channel message.
    pub(crate) fn spawn_worker(
        &self,
        body: impl FnOnce() -> Msg + Send + 'static,
        on_panic: impl FnOnce(anyhow::Error) -> Msg + Send + 'static,
    ) {
        let tx = self.tx.clone();
        let ctx = self.egui_ctx.clone();
        // Starting a worker schedules the next frame itself: the repaint
        // pump's gate (`poll_workers`, at the top of update) has already run
        // by the time a panel can start one in the same frame, so without
        // this the "fetching…"/busy state is not drawn and the 100 ms pump
        // never re-arms until the next input event.
        self.egui_ctx.request_repaint();
        std::thread::spawn(move || {
            let msg = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body))
                .unwrap_or_else(|p| {
                    let s = p
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| p.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic".into());
                    on_panic(anyhow::anyhow!("worker panicked: {s}"))
                });
            let _ = tx.send(msg);
            // An mpsc send does not wake egui: with the pointer held still, a
            // completed result used to sit unread in the channel until the
            // next mouse or key event happened to produce a frame.
            ctx.request_repaint();
        });
    }

    /// Decode a variant's retouched master off-thread — a 61 MP TIFF takes
    /// seconds, so the UI thread never does this inline — and install it at
    /// `preview_edge`, like every other master install (open: `load_active`'s
    /// worker; retouch: the retouch panel). This door used to keep the raster
    /// at SENSOR size: ~0.5 GiB resident, a ~244 MB paint mask from
    /// `refresh_active_pixels`, and a Before texture past the 8192 max
    /// texture dimension of common backends (L02).
    ///
    /// `preview_edge`, not `canvas_edge()`: at cold-switch time the canvas
    /// still holds the OUTGOING variant's raster, so `canvas_edge()` would
    /// key the install to a stale edge — and `preview_edge` is exactly what
    /// the open path files the same master under, so the two caches agree.
    ///
    /// One in-flight decode per (photo, origin): repeat entries (re-clicks
    /// into a still-decoding card, a stash restore) coalesce instead of
    /// stacking a fresh decode thread each time; an already-decoded master
    /// comes straight from `master_cache`. And `decode::load_image`, not
    /// `image::open`: the master rides the same decode budget and
    /// orientation handling as every other source.
    pub(crate) fn spawn_master_load(
        &mut self,
        ctx: &egui::Context,
        photo: PathBuf,
        origin: PathBuf,
    ) {
        let edge = self.preview_edge.clamp(640, 8192);
        // Cache probe first: a revisited card installs synchronously, with
        // no worker and no "loading…" status to flash.
        if let Some(arc) = self.cached_master(&origin, edge) {
            self.install_master(ctx, &photo, &origin, arc);
            return;
        }
        if !self.master_loads.insert((photo.clone(), origin.clone())) {
            return;
        }
        self.status = tr(
            self.lang,
            "loading this variant's retouched master… (showing the source develop meanwhile)",
        )
        .into();
        // Identity BEFORE the read (the curve-memo / remember_base rule,
        // L12-2): a master rewritten DURING the decode must MISS on the next
        // probe — stat'ing at landing filed the OLD pixels under the NEW
        // stamp, a false HIT. The spawn edge rides along too (L12-6): the
        // landing used to read the CURRENT preference, so a mid-flight px
        // change filed these pixels under a resolution they never had.
        let stamp = file_stamp(&origin);
        let (p2, o2) = (photo.clone(), origin.clone());
        self.spawn_worker(
            move || {
                // The same >24 MP permit `request_thumb` takes: a master
                // decode is deliberately not `busy`-gated, so N cold cards
                // with distinct origins could otherwise decode N 60 MP
                // rasters at once.
                let _big_permit = image::ImageReader::open(&origin)
                    .and_then(|r| r.into_dimensions().map_err(std::io::Error::other))
                    .is_ok_and(|(w, h)| w as u64 * h as u64 > 24_000_000)
                    .then(|| big_decode_gate().lock().unwrap_or_else(|p| p.into_inner()));
                // baked-by-construction: `origin` is the develop store's baked master.
                let img = autoshop::decode::load_image(&origin).map(|im| {
                    // GUARDED shrink: plain `thumbnail` UPSCALES, which would
                    // inflate a small master instead of bounding a large one.
                    if im.width().max(im.height()) > edge {
                        im.thumbnail(edge, edge)
                    } else {
                        im
                    }
                });
                Msg::MasterLoaded { photo, origin, edge, stamp, img: Box::new(img) }
            },
            move |e| {
                Msg::MasterLoaded { photo: p2, origin: o2, edge, stamp, img: Box::new(Err(e)) }
            },
        );
    }

    /// Cold-master LRU lookup (most-recent kept last) — same shape and same
    /// staleness guard as `cached_base`.
    pub(crate) fn cached_master(
        &mut self,
        origin: &std::path::Path,
        edge: u32,
    ) -> Option<Arc<image::DynamicImage>> {
        let stamp = file_stamp(origin);
        let pos = self
            .master_cache
            .iter()
            .position(|((p, e, t), _)| p == origin && *e == edge && *t == stamp)?;
        let entry = self.master_cache.remove(pos);
        let hit = entry.1.clone();
        self.master_cache.push(entry);
        Some(hit)
    }

    /// Remember a freshly decoded cold master under its own identity —
    /// `remember_base`'s twin. The stamp is the SPAWN-TIME capture riding
    /// the message (L12-2): the old landing-time stat argued it made a
    /// mid-decode rewrite miss, but it did the opposite — the NEW stamp was
    /// filed with the OLD pixels, so the next probe HIT on pixels the file
    /// never held. Keyed pre-read, a rewrite moves the stamp on and misses.
    pub(crate) fn remember_master(
        &mut self,
        origin: &std::path::Path,
        edge: u32,
        stamp: FileStamp,
        img: Arc<image::DynamicImage>,
    ) {
        self.master_cache.retain(|((p, e, _), _)| !(p == origin && *e == edge));
        self.master_cache.push(((origin.to_path_buf(), edge, stamp), img));
        if self.master_cache.len() > MASTER_CACHE_CAP {
            self.master_cache.remove(0); // least-recent first
        }
    }

    /// Install decoded master pixels into every strip entry that still
    /// references this exact master and still awaits pixels — by IDENTITY,
    /// never by index, so a delete/reorder mid-decode cannot misfile them.
    fn install_master(
        &mut self,
        ctx: &egui::Context,
        photo: &PathBuf,
        origin: &PathBuf,
        arc: Arc<image::DynamicImage>,
    ) {
        if self.src_path.as_ref() != Some(photo) {
            return;
        }
        let mut hit_active = false;
        for (i, v) in self.variants.iter_mut().enumerate() {
            if v.base.is_none() && v.origin.as_ref() == Some(origin) {
                v.base = Some(arc.clone());
                hit_active |= i == self.active;
            }
        }
        if hit_active {
            // The canvas was showing the disclosed source stand-in — swap in
            // the real pixels now that they exist.
            self.refresh_active_pixels(ctx);
            self.set_canvas_status("restored the canvas pixels");
        }
    }

    /// Queue a thumbnail decode for `idx` if it isn't cached/queued and we're
    /// under the concurrency cap. Uses the camera's embedded preview (fast) — the
    /// double-processing concern only applies to the develop base, not a 56px chip.
    /// A persistent disk cache (keyed on path+mtime+size) makes every later
    /// session — and every scroll-back after texture eviction — a ~1 ms JPEG
    /// read instead of a full decode.
    pub(crate) fn request_thumb(&mut self, idx: usize) {
        if self.thumbs.contains_key(&idx) || self.thumb_requested.contains(&idx) {
            return;
        }
        // A BOUNDED retry, because both extremes are wrong. Dropping the
        // marker on failure re-requested every frame — six decode threads per
        // 100 ms for as long as the row was visible. Keeping it forever
        // blanked the row for the session, and the LRU that was supposed to
        // provide the retry only prunes above THUMB_TEX_CAP, so a folder
        // under that size never retried at all. Three attempts covers the
        // transient causes (AV lock, a slow share) and stops dead on a file
        // that simply cannot be decoded.
        if self.thumb_fail.get(&idx).is_some_and(|n| *n >= 3) {
            return;
        }
        if self.thumb_inflight >= MAX_THUMB_INFLIGHT {
            return;
        }
        let Some(path) = self.gallery.get(idx).cloned() else { return };
        self.thumb_requested.insert(idx);
        self.thumb_inflight += 1;
        let generation = self.gallery_gen;
        self.spawn_worker(
            move || {
                let res = (|| -> anyhow::Result<image::DynamicImage> {
                    let cache = thumb_cache_file(&path);
                    if let Some(p) = &cache
                        && let Ok(img) = image::open(p)
                    {
                        return Ok(img);
                    }
                    // Large baked rasters (the app's own 60 MP TIFF/PNG exports)
                    // decode whole before the 160px shrink — ~360 MB each, and
                    // MAX_THUMB_INFLIGHT of them at once spiked to ~2 GB. Gate
                    // decodes above 24 MP behind one permit; the header read is
                    // cheap and RAWs (embedded-JPEG previews) stay concurrent.
                    let _big_permit = if !autoshop::decode::is_raw(&path)
                        && image::ImageReader::open(&path)
                            .and_then(|r| r.into_dimensions().map_err(std::io::Error::other))
                            .is_ok_and(|(w, h)| w as u64 * h as u64 > 24_000_000)
                    {
                        Some(big_decode_gate().lock().unwrap_or_else(|p| p.into_inner()))
                    } else {
                        None
                    };
                    let thumb =
                        autoshop::decode::preview_only(&path)?.thumbnail(THUMB_EDGE, THUMB_EDGE);
                    if let Some(p) = &cache {
                        save_thumb_cache(p, &thumb); // best-effort write-through
                    }
                    Ok(thumb)
                })();
                Msg::Thumb { generation, idx, img: Box::new(res) }
            },
            // The Err handler decrements thumb_inflight like any decode failure.
            move |e| Msg::Thumb { generation, idx, img: Box::new(Err(e)) },
        );
    }

    /// Queue the latest preview state on the single CPU worker. Dispatch is O(1):
    /// base pixels are Arc-shared and the recipe is small. While a frame is in
    /// flight, edits only set `dirty`; no parallel render storm is possible.
    pub(crate) fn start_redevelop(&mut self) {
        if self.develop_inflight {
            return;
        }
        let Some(base) = self.base_preview.clone() else {
            self.dirty = false;
            return;
        };
        let recipe = self.recipe.clone();
        let show_clipping = self.show_clipping;
        self.develop_inflight = true;
        self.dirty = false;
        self.spawn_worker(
            move || Msg::Developed(Box::new(Ok(build_preview(base, recipe, show_clipping)))),
            |e| Msg::Developed(Box::new(Err(e))),
        );
    }

    /// Accept one worker-built frame if it still describes the active base +
    /// recipe. Old frames are dropped without touching textures; `dirty` remains
    /// set by the newer edit and starts next. Texture handles are UPDATED in
    /// place, avoiding a manager allocate/free cycle on every slider tick.
    pub(crate) fn finish_redevelop(&mut self, ctx: &egui::Context, done: anyhow::Result<PreviewDone>) {
        self.develop_inflight = false;
        let frame = match done {
            Ok(frame) => frame,
            // Pure preview work has no ordinary error path; only a caught panic
            // lands here. Keep the last good frame and surface the failure —
            // WITHOUT fail(): that helper also clears the global `busy` flag,
            // which the preview path never set, and a panicking preview worker
            // mid-export used to unlock every busy-gated action.
            Err(e) => {
                let text = format!("{}: {e}", tr(self.lang, "preview develop failed"));
                self.status = text.clone();
                self.toast(ToastKind::Error, text);
                return;
            }
        };
        let current = self
            .base_preview
            .as_ref()
            .is_some_and(|base| Arc::ptr_eq(base, &frame.base))
            && self.recipe == frame.recipe;
        if !current {
            // The frame was solved for a recipe that changed mid-flight. The
            // redispatch gate is `dirty && !develop_inflight`, so a caller
            // that mutated the recipe WITHOUT setting dirty (historically the
            // crop path) would freeze the preview here forever — always
            // re-arm, the gate coalesces to a single fresh develop.
            self.dirty = true;
            ctx.request_repaint();
            return;
        }
        self.develop_count += 1;
        self.histogram = Some(frame.histogram);

        // The ColorImage arrives worker-built (see PreviewDone.after) — the UI
        // thread only uploads. The raw RGB is retained so the clipping toggle
        // can rebuild its overlay later without a redevelop.
        if let Some(tex) = &mut self.after_tex {
            tex.set(frame.after, egui::TextureOptions::LINEAR);
        } else {
            self.after_tex =
                Some(ctx.load_texture("after", frame.after, egui::TextureOptions::LINEAR));
        }
        // The clipping layer follows the CURRENT toggle, not the one baked at
        // dispatch: pressing J changes no recipe field, so an in-flight frame
        // is still accepted — its stale decision used to blank the layer under
        // a lit ▲ (or resurrect a ghost layer after J-off).
        let clip = if self.show_clipping {
            // Crop-blanked like the worker path — the J-mid-flight fallback
            // used to warn outside the crop while the worker's layer did not.
            Some(
                frame
                    .clipping
                    .unwrap_or_else(|| clipping_overlay_for(&frame.rgb, frame.recipe.crop)),
            )
        } else {
            None
        };
        self.last_rgb = Some(frame.rgb);
        match clip {
            Some(clip) => {
                if let Some(tex) = &mut self.clip_tex {
                    tex.set(clip, egui::TextureOptions::NEAREST);
                } else {
                    self.clip_tex =
                        Some(ctx.load_texture("clip", clip, egui::TextureOptions::NEAREST));
                }
            }
            None => self.clip_tex = None,
        }
        if let Some(v) = self.variants.get_mut(self.active) {
            if let Some(tex) = &mut v.thumb {
                tex.set(frame.thumb, egui::TextureOptions::LINEAR);
            } else {
                v.thumb = Some(ctx.load_texture("vthumb", frame.thumb, egui::TextureOptions::LINEAR));
            }
        }
        // A global change can alter a Range Mask's coverage reference. The
        // coverage-aware key makes this a cheap no-op for ordinary local effect
        // sliders, whose coverage is independent of their pixel adjustment.
        self.overlay_stale = true;
        ctx.request_repaint();
    }

    pub(crate) fn poll_workers(&mut self, ctx: &egui::Context) {
        // UI-thread language for status/toast strings built here. Worker RESULT
        // strings (msg / note / s / label) were already localised inside their
        // spawn closures before the thread started, so they arrive ready to show.
        let lang = self.lang;
        // Drain a bounded batch each frame so a burst of thumbnails doesn't take
        // one-per-frame to land (try_recv borrow is released before we mutate).
        for _ in 0..64 {
            let Some(msg) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) else { break };
            match msg {
                Msg::Opened(boxed) => self.on_opened(ctx, lang, boxed),
                Msg::Developed(boxed) => self.finish_redevelop(ctx, *boxed),
                Msg::Analyzed(epoch, boxed) => self.on_analyzed(lang, epoch, boxed),
                Msg::Exported(Ok(outcome)) => {
                    self.batch_progress = None; // the bar belongs to ONE batch run
                    match outcome {
                        ExportOutcome::Single { out, relooked } => {
                            // ABSOLUTE (R22-7): the deliverable exists NOW, and
                            // "exported → out/DSC0001.developed.tif" named a
                            // folder relative to whatever directory the app was
                            // launched from — the single most-reported "where
                            // did my file go". `abs_display` is lexical, so it
                            // costs nothing and yields no `\\?\` prefix.
                            let mut p = abs_display(&out);
                            if relooked {
                                // The export repaired its own COPY of this
                                // canvas's washed base curve (export.rs) —
                                // without repairing the canvas too, export
                                // and preview stay divergent for the rest of
                                // the session (the cross-surface class,
                                // L14-3). Same photo anchor as the worker;
                                // the repair memo is hot, so this is cheap.
                                if let Some(photo) = self.src_path.clone()
                                    && autoshop::pipeline::repair_pre_era_base_curve(
                                        &photo,
                                        &mut self.recipe,
                                    )
                                    .is_some()
                                {
                                    self.dirty = true;
                                }
                                p = format!(
                                    "{p} — {}",
                                    tr(
                                        lang,
                                        "camera base look re-estimated — this photo was saved by a version whose preview sampler ran bright, so its stored base look rendered too dark",
                                    )
                                );
                            }
                            self.done(trf(lang, "exported → {path}", &[("path", &p)]));
                        }
                        ExportOutcome::Batch { ok, errs, renamed, relooked, warns, dest } => {
                            // Where the batch actually DELIVERED becomes the
                            // remembered destination — here, not at the dialog
                            // (R22 M1): a folder inside the photo library is
                            // refused per photo by `guard_readonly`, so a batch
                            // that picked one lands nothing, and remembering it
                            // would leave `ExportDest::LastUsed` aimed at a
                            // folder that can only refuse. `ok > 0` is the proof
                            // that at least one file is really there.
                            if ok > 0 {
                                self.last_export_dir = Some(dest.clone());
                            }
                            // Same-stem photos were kept apart — disclose
                            // WHICH photo took WHICH name, or the user hunts
                            // for an export that "vanished".
                            let renames = if renamed.is_empty() {
                                String::new()
                            } else {
                                trf(
                                    lang,
                                    " · same-name photos kept apart: {list}",
                                    &[("list", &renamed.join(", "))],
                                )
                            };
                            let relook = if relooked == 0 {
                                String::new()
                            } else {
                                trf(
                                    lang,
                                    " · {n} base look(s) re-estimated (a pre-era save rendered too dark)",
                                    &[("n", &relooked.to_string())],
                                )
                            };
                            // The open path's develop warnings, on the batch
                            // outcome (L16-2) — both reply arms, so a partial
                            // failure keeps its disclosures too.
                            let develop_warns = if warns.is_empty() {
                                String::new()
                            } else {
                                trf(
                                    lang,
                                    " · {n} develop warning(s): {detail}",
                                    &[("n", &warns.len().to_string()), ("detail", &warns.join(" / "))],
                                )
                            };
                            if errs.is_empty() {
                                self.done(format!(
                                    "{}{renames}{relook}{develop_warns}",
                                    trf(
                                        lang,
                                        "batch {n} done → {path}",
                                        &[("n", &ok.to_string()), ("path", &abs_display(&dest))],
                                    )
                                ));
                            } else {
                                // A partial failure keeps the error channel,
                                // exactly as the worker-side bail! did.
                                self.fail(
                                    tr(lang, "export failed"),
                                    anyhow::anyhow!(
                                        "{}{renames}{relook}{develop_warns}",
                                        trf(
                                            lang,
                                            "Batch: {ok} succeeded, {fail} failed: {detail}",
                                            &[
                                                ("ok", &ok.to_string()),
                                                ("fail", &errs.len().to_string()),
                                                ("detail", &errs.join("; ")),
                                            ],
                                        )
                                    ),
                                );
                            }
                        }
                    }
                }
                Msg::Exported(Err(e)) => {
                    self.batch_progress = None;
                    self.fail(tr(lang, "export failed"), e);
                }
                Msg::BatchProgress { done, total } => {
                    self.batch_progress = Some((done, total));
                    // No destination here: the tick fires once per photo and the
                    // root was already named when the batch started (and is
                    // named again at the landing). Repeating it 500 times only
                    // pushed the counts out of a truncated status line — and
                    // the old hardcoded "./out" became a lie the moment the
                    // destination became a setting (R22-7).
                    self.status = trf(
                        lang,
                        "Batch-rendering {done}/{total} …",
                        &[("done", &done.to_string()), ("total", &total.to_string())],
                    );
                }
                Msg::StyleInfo(info) => {
                    self.style_info_loading = false;
                    self.style_info = Some(*info);
                }
                Msg::StyleBuildProgress { done, total } => {
                    self.style_build_progress = Some((done, total));
                    // The panel's own line renders these counts; the status bar
                    // gets them too, because a minutes-long job must be visible
                    // from wherever the user is looking.
                    self.status = trf(
                        lang,
                        "Building the style library… {done} / {total} photos",
                        &[("done", &done.to_string()), ("total", &total.to_string())],
                    );
                }
                Msg::StyleBuilt(outcome) => self.on_style_built(lang, *outcome),
                Msg::Segmented(res) => self.on_segmented(lang, res),
                Msg::MaskRefined(res) => self.on_mask_refined(lang, res),
                Msg::Folder(boxed) => self.on_folder(lang, *boxed),
                Msg::Thumb { generation, idx, img } => self.on_thumb(ctx, generation, idx, *img),
                Msg::MasterLoaded { photo, origin, edge, stamp, img } => {
                    self.on_master_loaded(ctx, lang, photo, origin, edge, stamp, *img)
                }
                Msg::Progress(epoch, m) => {
                    // Liveness lines from a running generative worker. Epoch-
                    // gated: after Cancel + an immediate re-run, the abandoned
                    // worker's heartbeats must not overwrite the new task's
                    // status line.
                    if self.busy && self.gen_cancel.is_some() && epoch == self.gen_epoch {
                        self.status = m;
                    }
                }
                Msg::Retouched(epoch, boxed) => self.on_retouched(ctx, lang, epoch, *boxed),
                Msg::Fitted(boxed) => self.on_fitted(ctx, lang, boxed),
                Msg::Styled(boxed) => match *boxed {
                    Ok((prompt, note)) => {
                        // Into the Reimagine prompt: ready to restyle OTHER photos.
                        self.reimagine_prompt = prompt;
                        let s = match &note {
                            StyleNote::SavedCopy => tr(
                                lang,
                                "Style prompt extracted → filled into the Reimagine prompt (also saved ./out/<stem>.style.txt)",
                            )
                            .to_string(),
                            StyleNote::SaveFailed(err) => trf(
                                lang,
                                "Style prompt extracted → filled into the Reimagine prompt (saving ./out/<stem>.style.txt failed: {err})",
                                &[("err", err)],
                            ),
                            StyleNote::NotSaved => tr(
                                lang,
                                "Style prompt extracted → filled into the Reimagine prompt",
                            )
                            .to_string(),
                        };
                        self.done(s);
                    }
                    Err(e) => {
                        self.fail(tr(lang, "Style extraction failed"), e);
                    }
                },
                Msg::Pasted(res) => self.on_pasted(lang, res),
                Msg::LegacyImported(res) => {
                    self.edited_badge.clear(); // imported sidecars light ● badges
                    match res {
                        Ok((n, dir)) => self.done(trf(
                            lang,
                            "Imported saved develops for {n} photo(s) from {path}",
                            &[("n", &n.to_string()), ("path", &dir.display().to_string())],
                        )),
                        Err(e) => self.fail(tr(lang, "import failed"), e),
                    }
                }
                Msg::Models(role, generation, res) => self.on_models(lang, role, generation, res),
            }
        }
    }

    /// Keep the frame loop alive while any worker (analyze/export/thumbs/
    /// models/master decodes) runs — at a 100 ms poll, not frame rate:
    /// worker completion only surfaces through the mpsc poll, and a
    /// full-rate repaint burned CPU for the whole life of a stalled 600 s AI
    /// call. Input still repaints immediately; 100 ms only bounds COMPLETION
    /// latency. `master_loads` is in the gate because a cold-variant master
    /// decode is NOT `busy` (16-lane scan L06). Called at the END of
    /// update() — after every panel had its chance to spawn (L12-3): inside
    /// poll_workers (the top of the frame) it judged a frame whose spawns
    /// had not happened yet, and the first batch of workers ran with no
    /// completion pump armed.
    pub(crate) fn pump_repaint_gate(&mut self, ctx: &egui::Context) {
        if self.busy
            || self.thumb_inflight > 0
            || self.settings.image_models.fetching
            || self.settings.analysis_models.fetching
            || !self.master_loads.is_empty()
            // R23-2: the style-library build is NOT busy-gated (it runs for
            // minutes and the app stays usable), so without it here the
            // "building…" state would only repaint when a progress tick
            // happens to arrive — a single slow 60 MP decode would freeze the
            // counter for as long as it takes. Same reason the two model
            // fetches are on this list.
            || self.style_build_inflight
            || self.style_info_loading
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// `Msg::Opened` landing — body extracted verbatim from the
    /// poll_workers pump (round-12 decomposition; indentation kept).
    fn on_opened(&mut self, ctx: &egui::Context, lang: Lang, boxed: Box<anyhow::Result<OpenedBase>>) {
                    // `keep` distinguishes a fresh open from a preview-resolution
                    // re-decode (the px combo): consumed whether the open
                    // succeeds or fails so a failure can't leak it into a later
                    // open.
                    // The KEEP request is honoured only when the recorded
                    // FACT agrees the flight was same-path: a stale request
                    // must never graft the outgoing photo's whole canvas,
                    // strip and history onto an incoming photo.
                    let keep = std::mem::take(&mut self.keep_recipe) && self.open_same_path;
                    let edge_revert = self.edge_before_flight.take();
                    self.open_in_flight = false; // both arms: the transition ended
                    match *boxed {
                    Ok((base, knots, lens, as_shot, baked, src_ident, baked_note)) => {
                        self.busy = false;
                        // Surfaced BEFORE anything else can early-return
                        // (L11-3): the worker degraded to the un-retouched
                        // source, and stderr is invisible in the windowed
                        // build. Same channel as the MasterLoaded failure.
                        if let Some(err) = &baked_note {
                            let t = trf(
                                lang,
                                "this variant's saved master could not be loaded ({err}) — showing the un-retouched source develop instead",
                                &[("err", err)],
                            );
                            self.toast(ToastKind::Error, t);
                        }
                        if let Some(p) = self.src_path.clone() {
                            self.remember_base(
                                &p,
                                &(
                                    base.clone(),
                                    knots.clone(),
                                    lens.clone(),
                                    as_shot,
                                    baked.clone(),
                                    src_ident.clone(),
                                    // Never cached: a revisit decodes
                                    // nothing, so it must not re-toast.
                                    None,
                                ),
                            );
                            // C1/F10 alias disclosure rides the open (once
                            // per photo per session — the store stashes the
                            // note at first key resolution and this consumes
                            // it). Typed note, rendered HERE in the session
                            // language (the worker-closure i18n rule).
                            if !keep && let Some(note) = autoshop::store::take_alias_note(&p) {
                                let t = match note {
                                    autoshop::store::AliasNote::Adopted { from } => trf(
                                        lang,
                                        "edits saved under an older spelling of this photo's path were adopted ({path})",
                                        &[("path", &from.display().to_string())],
                                    ),
                                    autoshop::store::AliasNote::SecondDevelop { at } => trf(
                                        lang,
                                        "a second saved develop exists at {path} (an older spelling of this photo's path) — it was NOT merged; showing the develop under the photo's true path",
                                        &[("path", &at.display().to_string())],
                                    ),
                                };
                                self.toast(ToastKind::Error, t);
                            }
                        }
                        // Kept for Reset: "reset" = the fresh-open look, so it
                        // needs this photo's knots even when the restored
                        // recipe (legacy save) deliberately carries none.
                        // Deliberately LAST-wins across preview-edge switches
                        // (Reset means the fresh-open look at the CURRENT
                        // edge) while the repair memo pins its FIRST answer
                        // for persistence determinism — the two agree within
                        // the estimator's documented tolerance.
                        self.photo_knots = knots.clone();
                        self.photo_lens = lens.clone();
                        self.photo_as_shot = as_shot;
                        if keep {
                            // Preview-resolution re-decode: the SOURCE pixels just
                            // changed resolution — keep the whole variant set,
                            // recipe, undo history and view (zoom included: you
                            // switched to 4096px to inspect 1:1, losing the zoom
                            // would defeat the point). Refresh the base a
                            // source-based active variant develops from; hand a
                            // baked-raster variant its own master re-decoded —
                            // and when neither is possible, REFUSE the switch
                            // instead of half-applying it.
                            let (mw, mh) = base.dimensions();
                            // Consume the stash entry the same-path open_path
                            // call just wrote: the KEEP branch preserves the
                            // live canvas in place, so the entry is a stale
                            // duplicate — after an undo-to-clean it would
                            // resurrect the pre-undo edits on the next return.
                            // (Both outcomes below keep the canvas in place, so
                            // this is unconditional.)
                            if let Some(p) = self.src_path.clone() {
                                self.nav_stash.remove(&p);
                            }
                            let active_source =
                                self.active_variant().is_none_or(|v| v.base.is_none());
                            // ONE source of truth for "the worker brought the
                            // master this canvas renders": the branch below
                            // destructures the same `baked`, and a guard that
                            // could drift from it is how a canvas ends up at an
                            // edge nobody recorded.
                            let master_hit = baked.as_ref().is_some_and(|(_, borigin, _)| {
                                self.active_variant().is_some_and(|v| {
                                    v.origin.as_deref().is_some_and(|o| same_master(o, borigin))
                                })
                            });
                            if !active_source && !master_hit && let Some(e) = edge_revert {
                                // The switch CANNOT materialize on THIS canvas:
                                // the active variant renders baked pixels
                                // (heal / clone / fill / reimagine) whose master
                                // the worker had nothing to re-decode from —
                                // `pixels.json` is written by SAVE, so an
                                // unsaved retouch records no master at all.
                                // Adopting the new edge anyway was the busy
                                // refusal's residue through the third door: the
                                // status announced a re-decode the canvas never
                                // took, the value persisted into Prefs, and
                                // every bake site would have installed a
                                // new-edge raster under an old-edge canvas. The
                                // re-decoded source is dropped with it, so
                                // canvas and edge cannot disagree in either
                                // direction (a later switch after saving
                                // re-decodes the master properly).
                                self.preview_edge = e;
                                // The remedy is read off THIS CANVAS, never
                                // off the photo's record: a stale broken
                                // record (an ./out cleanup) plus a NEW unsaved
                                // retouch used to be told its situation was
                                // unfixable and denied the one cure that
                                // works. Three causes, three answers:
                                //  · a GENERATED canvas has no save a user can
                                //    be sent to press: Ctrl+S refuses it
                                //    outright (the Analyze auto-save and
                                //    Save-all DO record generated masters, so
                                //    "cannot be recorded" would be false), and
                                //    the message names the route that always
                                //    works instead;
                                //  · a master no longer ON DISK cannot be
                                //    re-decoded however often it is recorded
                                //    (saving would re-record the same broken
                                //    link and the refusal would repeat);
                                //  · otherwise the master is there and merely
                                //    unrecorded (or superseded since the last
                                //    save), which saving fixes.
                                // Residual, bounded and stderr-visible: a
                                // master that exists but will not DECODE takes
                                // the third arm, and `read_pixel_source` /
                                // the worker already name it on stderr.
                                let canvas_master =
                                    self.active_variant().and_then(|v| v.origin.clone());
                                let t = if self.active_is_generated() {
                                    tr(
                                        lang,
                                        "preview resolution kept — a generated variant's pixels come from its own render; switch to a source-based variant to work at another resolution",
                                    )
                                } else if !canvas_master.as_deref().is_some_and(|o| o.exists()) {
                                    tr(
                                        lang,
                                        "preview resolution kept — this canvas's retouch master is no longer on disk, so it cannot be re-decoded at the new size",
                                    )
                                } else {
                                    tr(
                                        lang,
                                        "preview resolution kept — this retouched canvas has no saved master to re-decode at the new size; save the photo, then switch",
                                    )
                                };
                                // The bar still reads "decoding … " (open_path
                                // wrote it and `busy` is already false): every
                                // other outcome of this landing replaces it, so
                                // the refusal must too — a finished decode
                                // announcing itself forever is the same lie one
                                // door along, on the surface that does not
                                // expire.
                                self.status = t.to_string();
                                self.toast(ToastKind::Error, t);
                            } else {
                                self.source_preview = Some(base.clone());
                                if active_source {
                                    let curve = self.recipe.base_curve.clone();
                                    self.set_before(ctx, &base, &curve);
                                    self.rebind_paint_canvas(mw, mh);
                                    // The range-mask reference develop was computed
                                    // from the OLD base — recipe-only keying would
                                    // serve it stale against these new pixels.
                                    self.overlay_ref = None;
                                    self.overlay_stale = true;
                                    self.base_preview = Some(base);
                                } else if master_hit
                                    && let Some((bimg, _, _)) = &baked
                                {
                                    // The active variant renders a RECORDED baked
                                    // master: hand it the master re-decoded at the
                                    // new preview edge too, or 1:1 inspection stays
                                    // at the old resolution. History steps holding
                                    // the old Arc are repointed — same master, new
                                    // resolution — so Ctrl+Z can't flip the canvas
                                    // back to the stale-res pixels.
                                    let old = self
                                        .variants
                                        .get(self.active)
                                        .and_then(|v| v.base.clone());
                                    if let Some(v) = self.variants.get_mut(self.active) {
                                        v.base = Some(bimg.clone());
                                    }
                                    // Bound, stated instead of implied: only
                                    // the Arc the canvas holds NOW is
                                    // re-pointed. A step on a SUPERSEDED master
                                    // (an earlier heal, replaced by this one)
                                    // has no re-decoded twin — the worker
                                    // resolves exactly one master — so undoing
                                    // to it restores that raster at ITS own
                                    // edge. That is why a bake follows the
                                    // canvas (`canvas_edge`), not the
                                    // preference.
                                    if let Some(old) = old {
                                        let repoint = |s: &mut UndoStep| {
                                            let hit = s
                                                .base
                                                .as_ref()
                                                .is_some_and(|b| Arc::ptr_eq(b, &old));
                                            if hit {
                                                s.base = Some(bimg.clone());
                                            }
                                        };
                                        repoint(&mut self.committed);
                                        self.undo_stack.iter_mut().for_each(repoint);
                                        self.redo_stack.iter_mut().for_each(repoint);
                                    }
                                    let (bw, bh) = bimg.dimensions();
                                    // Canvas-curve Before, same rule as every
                                    // restore path: an InPlace master is NEUTRAL
                                    // (needs the camera curve); a Generated
                                    // canvas recipe carries an empty curve.
                                    let curve = self.recipe.base_curve.clone();
                                    self.set_before(ctx, bimg, &curve);
                                    self.rebind_paint_canvas(bw, bh);
                                    self.overlay_ref = None;
                                    self.overlay_stale = true;
                                    self.base_preview = Some(bimg.clone());
                                }
                                self.last_rgb = None; // resolution changed under the frame
                                self.dirty = true;
                                self.status = trf(
                                    lang,
                                    "Preview resolution {px}px — re-decoded",
                                    &[("px", &self.preview_edge.to_string())],
                                );
                            }
                        } else {
                            // Fresh open: a single Original variant, all
                            // per-photo state reset. The recipe starts from the
                            // photo's SAVED develop when a ./out sidecar exists
                            // — the gallery badge already promises "● edited",
                            // so opening must honour it — neutral otherwise.
                            let (mw, mh) = base.dimensions();
                            // before_tex is built AFTER the recipe is settled
                            // below — the Before pane carries the canvas
                            // recipe's own base_curve.
                            self.source_preview = Some(base.clone());
                            self.base_preview = Some(base);
                            let RestoredDevelop {
                                saved,
                                xmp_bad,
                                dropped_masks,
                                carried_globals,
                                clamp: clamp_dropped,
                                lr_unreadable,
                                packet_unreadable,
                            } = self
                                .src_path
                                .as_deref()
                                .map(read_saved_develop)
                                .unwrap_or_default();
                            // W20: restore-time sanitisation is not allowed to
                            // be silent loss — a stored recipe past the caps
                            // opens minus edits, and the user must hear it.
                            if !clamp_dropped.is_empty() {
                                // All four loss kinds — a curve-only or
                                // string-only truncation used to toast
                                // "0 mask(s) and 0 component(s)" (16-lane
                                // scan L14/L16).
                                let t = trf(
                                    lang,
                                    "recipe limits discarded {n} mask(s), {m} component(s), {c} curve point(s) and {s} string byte(s) on restore — the saved file exceeds the app's caps",
                                    &[
                                        ("n", &clamp_dropped.dropped_masks.to_string()),
                                        ("m", &clamp_dropped.dropped_components.to_string()),
                                        ("c", &clamp_dropped.truncated_curve_points.to_string()),
                                        ("s", &clamp_dropped.truncated_string_bytes.to_string()),
                                    ],
                                );
                                self.toast(ToastKind::Error, t);
                            }
                            // LR brush / AI / depth masks have no engine
                            // equivalent — the import skips them BY DESIGN
                            // (the writer skips symmetrically); what was
                            // missing is telling the user their Lightroom
                            // work arrived incomplete. The sidecar keeps
                            // them; only the in-app render lacks them.
                            if dropped_masks > 0 {
                                let t = trf(
                                    lang,
                                    "{n} Lightroom mask(s) (brush/AI/depth) have no engine equivalent and were not imported — they stay in the sidecar untouched",
                                    &[("n", &dropped_masks.to_string())],
                                );
                                self.toast(ToastKind::Error, t);
                            }
                            // R24-5 M0, the GLOBAL half of the same story: the
                            // sidecar's own crs: properties this engine does
                            // not model. The merge keeps every one of them, so
                            // nothing is lost — but the canvas will not match
                            // Lightroom's render, and until now nothing said
                            // why. Named, not counted: 「Texture」 is the whole
                            // answer, 「3 settings」 is not. Capped like the
                            // mask-loss line, for the same reason.
                            if !carried_globals.is_empty() {
                                let shown = carried_globals.len().min(4);
                                let more = carried_globals.len() - shown;
                                let mut list = carried_globals[..shown].join(", ");
                                if more > 0 {
                                    list.push_str(&format!(
                                        ", {}",
                                        trf(lang, "+{n} more", &[("n", &more.to_string())])
                                    ));
                                }
                                let t = trf(
                                    lang,
                                    "this Lightroom sidecar carries {n} global setting(s) the engine does not render (a save keeps them untouched): {list}",
                                    &[("n", &carried_globals.len().to_string()), ("list", &list)],
                                );
                                self.toast(ToastKind::Error, t);
                            }
                            // The decision half lives in persist::resolve_saved_develop —
                            // this handler applies it to the canvas. Two reasons to skip
                            // the repair decode fold here: a GENERATED canvas strips its
                            // calibration below, and a session stash about to override the
                            // canvas wholesale would discard the repair (dirty_vs keeps ●
                            // safe in both directions).
                            let skip_repair = baked.as_ref().is_some_and(|(_, _, g)| *g)
                                || self
                                    .src_path
                                    .as_ref()
                                    .is_some_and(|p| self.nav_stash.contains_key(p));
                            let ResolvedSaved {
                                mut recipe,
                                restored,
                                stamp,
                                mut open_note,
                                unresolved,
                            } = resolve_saved_develop(
                                lang,
                                saved,
                                skip_repair,
                                self.src_path.as_deref(),
                            );
                            if unresolved {
                                self.open_unresolved = true;
                            }
                            // A6 disclosure: numeric settings in the restored
                            // XMP that do not parse imported as SILENT
                            // neutrals — and the next save would write those
                            // neutrals back over the sidecar. Say so on open.
                            if !xmp_bad.is_empty() {
                                let w = trf(
                                    lang,
                                    "{n} XMP numeric setting(s) unreadable ({list}) — restored as neutral; saving would overwrite the sidecar with those neutrals",
                                    &[
                                        ("n", &xmp_bad.len().to_string()),
                                        ("list", &xmp_bad.join(", ")),
                                    ],
                                );
                                open_note = merge_note(open_note, w);
                            }
                            if let Some(why) = lr_unreadable {
                                let w = trf(
                                    lang,
                                    "a Lightroom sidecar sits beside this photo but could not be read ({why}) — any Lightroom edits in it are NOT reflected",
                                    &[("why", why)],
                                );
                                open_note = merge_note(open_note, w);
                            }
                            // Same unreadable-is-not-absent rule for the
                            // packet INSIDE the RAW — only probed when no
                            // other source answered, so this fires exactly
                            // when whatever it holds would have been shown.
                            if let Some(why) = packet_unreadable {
                                let w = trf(
                                    lang,
                                    "this RAW carries an embedded XMP develop that could not be read ({why}) — it is NOT reflected",
                                    &[("why", &why)],
                                );
                                open_note = merge_note(open_note, w);
                            }
                            if stamp {
                                stamp_calibration(&mut recipe, &knots, &lens, as_shot);
                            } else if !stamp
                                && recipe.base_curve.is_empty()
                                && !knots.is_empty()
                                && open_note.is_none()
                            {
                                // A legacy save renders exactly as it was
                                // tuned — on the old dark base. Say so, and
                                // point at the way out, or the photo just
                                // looks broken next to fresh opens.
                                open_note = Some(
                                    tr(
                                        lang,
                                        "saved before the camera base look — renders as originally tuned; Reset switches to the camera-matched base",
                                    )
                                    .into(),
                                );
                            }
                            // The saved develop is the ● baseline; a canvas
                            // stashed when the user navigated away THIS session
                            // outranks it — that stash is their newest work.
                            self.saved_recipe = recipe.clone();
                            let mut from_stash = false;
                            // The stash (this session's newest work) outranks
                            // disk WHOLESALE — recipe and pixel identity both:
                            // a stashed source-based canvas (e.g. the retouch
                            // was undone) must not resurrect disk pixels.
                            // Disk truth for the per-frame ● comparison —
                            // captured BEFORE the stash override below.
                            self.pixels_on_disk = baked.as_ref().map(|(_, o, _)| o.clone());
                            // A RECORDED master that arrived as None failed to
                            // decode/resolve in the worker (it only eprintln'd
                            // there) — a desktop-launched GUI would otherwise
                            // show an apparently normal un-retouched open with
                            // no in-app trace.
                            // has_pixel_source, NOT read_pixel_source: the
                            // latter answers None for "nothing recorded" AND
                            // for "recorded but broken", so asking it again
                            // here made the toast unreachable for the two
                            // commonest causes — a deleted/moved master and a
                            // corrupt pixels.json. Those are precisely the
                            // cases the user must be told about.
                            if baked.is_none()
                                && let Some(p) = self.src_path.as_deref()
                                && autoshop::store::has_pixel_source(p)
                            {
                                let t = tr(
                                    lang,
                                    "the saved retouch master could not be loaded — opened the un-retouched source (Ctrl+S would overwrite the master link)",
                                );
                                self.toast(ToastKind::Error, t);
                            }
                            let mut pixels: Option<BakedBase> = baked;
                            // The stashed active card's master when its decode
                            // was still IN FLIGHT at navigate-away: origin
                            // known, pixels not yet — the identity must
                            // survive the reopen even though the image is not
                            // here yet.
                            let mut pending_master: Option<PathBuf> = None;
                            let mut stash_others: Vec<StashedVariant> = Vec::new();
                            let mut stash_active_pos = 0usize;
                            // The ACTIVE card's three-valued kind, whichever
                            // authority supplies it: the session stash below,
                            // or the persisted strip record read further down
                            // (`Fitted` is source-based, so it has no pixels
                            // arm to ride back on — without this the card
                            // reopened renamed 「▣ 原片」).
                            let mut active_kind = VariantKind::Original;
                            // …and its identity + name, from the same two
                            // authorities (R24-2, hop 6 of 6). The default is
                            // the FIXED base-negative id: the common photo
                            // has one card and no strip record at all, and a
                            // freshly minted id there would leave every
                            // version it ever took unattributed on reopen.
                            let mut active_id = ORIGINAL_VARIANT_ID.to_string();
                            let mut active_name: Option<String> = None;
                            if let Some(st) =
                                self.src_path.as_ref().and_then(|p| self.nav_stash.remove(p))
                            {
                                recipe = st.recipe;
                                active_kind = st.kind;
                                active_id = st.id;
                                active_name = st.name;
                                pixels = match (st.base, st.origin) {
                                    // The bool is the `pixels.json` FORMAT
                                    // flag (R24-1's not-collected list), not
                                    // the parametric predicate.
                                    (Some(b), Some(o)) => {
                                        Some((b, o, st.kind == VariantKind::Generated))
                                    }
                                    // Navigated away while the cold master was
                                    // still decoding: the ORIGIN is the pixel
                                    // link's identity — dropping it here let
                                    // the next Ctrl+S clear pixels.json and
                                    // persist a generated recipe as a source-
                                    // based develop. The base re-decodes
                                    // below; the link and the kind survive.
                                    (None, Some(o)) => {
                                        pending_master = Some(o);
                                        None
                                    }
                                    _ => None,
                                };
                                stash_others = st.others;
                                stash_active_pos = st.active_pos;
                                from_stash = true;
                                // The stash carries the strip VERBATIM — and
                                // when the ORIGINAL open's repair was an
                                // inability, those recipes still hold the
                                // washed era-1 curve. Inabilities exist to
                                // be retried by the next reader, and the
                                // gate above deliberately skipped the
                                // disk-recipe repair in deference to this
                                // override — so the retry happens HERE, on
                                // every recipe that can land on the canvas:
                                // the active one AND each sibling (a strip
                                // entry is one click from BEING the canvas).
                                // The washed-sibling population is ORDINARY,
                                // not residual: push_variant/switch_variant
                                // sync the OUTGOING canvas into the strip,
                                // so a washed Original displaced by an AI
                                // push is a washed sibling (the freshly
                                // PUSHED variants themselves are era-2 and
                                // empty by construction). load_active
                                // repairs on switch as the last line of
                                // defence; this eager pass heals the whole
                                // strip the moment it is restored. ONE
                                // estimate total — the memo keys on the
                                // photo — and already-repaired recipes
                                // short-circuit on their era stamp.
                                // A failed estimate retries on the next
                                // return; the deterministic-inability
                                // population is empty for estimator-written
                                // curves (the too-few-pixels guard is
                                // coeval with base_curve itself). Only
                                // PARAMETRIC cards are repaired (R24-1): a
                                // pixel-state card's curve is empty by
                                // invariant. These two are 2 of the 6 sites
                                // that spelled that binary by hand as
                                // `!= Generated`; they ask
                                // `VariantKind::is_parametric` now, so a
                                // fourth kind cannot inherit the answer.
                                if let Some(p) = self.src_path.clone() {
                                    let mut relooked = active_kind.is_parametric()
                                        && autoshop::pipeline::repair_pre_era_base_curve(
                                            &p, &mut recipe,
                                        )
                                        .is_some();
                                    for sv in &mut stash_others {
                                        if sv.kind.is_parametric() {
                                            relooked |=
                                                autoshop::pipeline::repair_pre_era_base_curve(
                                                    &p,
                                                    &mut sv.recipe,
                                                )
                                                .is_some();
                                        }
                                    }
                                    if relooked {
                                        let w = tr(
                                            lang,
                                            "camera base look re-estimated — this photo was saved by a version whose preview sampler ran bright, so its stored base look rendered too dark",
                                        )
                                        .to_string();
                                        open_note = merge_note(open_note, w);
                                    }
                                }
                            }
                            // The persisted strip record — the disk half of
                            // the session stash. Read even when the stash
                            // outranks it wholesale: it is also the mirror
                            // the background-variant dirty test compares
                            // against (see `open_dirty_variants`).
                            let disk_strip = match self
                                .src_path
                                .as_deref()
                                .map(autoshop::store::read_variants_checked)
                            {
                                Some(autoshop::store::VariantsRead::Strip(rec)) => Some(rec),
                                Some(autoshop::store::VariantsRead::Unresolved) => {
                                    // The store's save floor refuses to touch
                                    // an unresolved strip — say so at OPEN,
                                    // not first at the failing save.
                                    let t = tr(
                                        lang,
                                        "this photo's variant strip (variants.json) cannot be read — background variants stay hidden and saving refuses until the file is fixed or deleted",
                                    )
                                    .to_string();
                                    self.toast(ToastKind::Error, t);
                                    None
                                }
                                _ => None,
                            };
                            if !from_stash && let Some(rec) = &disk_strip {
                                if rec.active_kind == "fitted" {
                                    // Fitted is source-based — it has no pixels
                                    // arm to ride back on; without this the card
                                    // cold-reopened renamed 「▣ 原片」. A recorded
                                    // "generated" needs no hand here: the baked
                                    // pixels arm below upgrades the card exactly
                                    // when the master really decoded.
                                    active_kind = VariantKind::Fitted;
                                }
                                // The persisted strip is also where the active
                                // card's identity + name live (R24-2); a record
                                // written before those fields existed mints one.
                                active_id = variant_id_or_mint(
                                    rec.active_id.as_ref(),
                                );
                                active_name = rec.active_name.clone();
                            }
                            self.recipe = recipe.clone();
                            self.rationale = recipe.rationale.clone();
                            self.rationale_notes.clear(); // disk restore carries no typed notes
                            self.variants = vec![Variant {
                                // A PENDING master keeps its stashed kind: the
                                // Fitted-or-Original collapse renamed a still-
                                // decoding Generated card to 「▣ 原片」, and a
                                // save then persisted that lie.
                                kind: if pending_master.is_some() {
                                    active_kind
                                } else if active_kind == VariantKind::Fitted {
                                    VariantKind::Fitted
                                } else {
                                    VariantKind::Original
                                },
                                id: active_id,
                                name: active_name,
                                recipe,
                                base: None,
                                origin: None,
                                thumb: None,
                            }];
                            self.active = 0;
                            // Before = this photo's starting point under the
                            // canvas recipe's own base calibration: stamped
                            // fresh opens compare bright, legacy saves compare
                            // as originally tuned.
                            if let Some(b) = self.base_preview.clone() {
                                let curve = self.recipe.base_curve.clone();
                                self.set_before(ctx, &b, &curve);
                            }
                            // A fresh, fully-transparent paint mask sized to the
                            // preview — and a brush session armed on the PREVIOUS
                            // photo dies with its plate (this fresh-open arm never
                            // called disarm_tools; the always-visible 「✓ Apply」
                            // would have baked the old photo's strokes into this
                            // photo's mask list).
                            self.rebind_paint_canvas(mw, mh);
                            self.paint_mode = false;
                            if let Some((bimg, borigin, generated)) = pixels {
                                // Restore the baked pixel master (persisted
                                // pixels.json, or this session's stash): the
                                // canvas sits on the retouched pixels again
                                // with the recipe rendering on top. An
                                // in-place master is a NEUTRAL develop — the
                                // recipe's base_curve legitimately renders the
                                // camera look on top of it; Before compares
                                // curve-less, exactly like the InPlace flow
                                // that made it. A GENERATED master's look
                                // already lives in its pixels, so calibration
                                // must be STRIPPED from the canvas recipe
                                // (same rule as analyzing a baked variant) or
                                // curve + lens geometry would cook it twice.
                                let (bw, bh) = bimg.dimensions();
                                if let Some(v) = self.variants.get_mut(0) {
                                    v.base = Some(bimg.clone());
                                    v.origin = Some(borigin);
                                    if generated {
                                        v.kind = VariantKind::Generated;
                                    }
                                }
                                if generated {
                                    self.recipe.base_curve = Vec::new();
                                    self.recipe.lens_profile = Default::default();
                                    self.recipe.as_shot_k = None;
                                    self.recipe.as_shot_tint = None;
                                    if let Some(v) = self.variants.get_mut(0) {
                                        v.recipe = self.recipe.clone();
                                    }
                                    // The ● baseline lives in CANVAS
                                    // coordinates (same rule as the Analyze
                                    // saver): the disk recipe keeps its
                                    // calibration, but comparing the stripped
                                    // canvas against it lit ● on lens_profile
                                    // the instant a clean generated save
                                    // opened.
                                    self.saved_recipe.base_curve = Vec::new();
                                    self.saved_recipe.lens_profile = Default::default();
                                    self.saved_recipe.as_shot_k = None;
                                    self.saved_recipe.as_shot_tint = None;
                                }
                                // Before under the canvas recipe's own curve:
                                // empty for a Generated master (stripped just
                                // above — its pixels carry the look); the
                                // camera curve for an InPlace master, whose
                                // pixels are a NEUTRAL develop (an empty curve
                                // there compared 0.6–1.4 EV dark).
                                let curve = self.recipe.base_curve.clone();
                                self.set_before(ctx, &bimg, &curve);
                                self.base_preview = Some(bimg);
                                self.rebind_paint_canvas(bw, bh);
                            }
                            if let Some(o) = pending_master.clone() {
                                // Re-attach the surviving identity and re-arm
                                // the worker decode (coalesced if the earlier
                                // one is somehow still in flight). Until it
                                // lands the canvas shows the source develop
                                // under spawn_master_load's disclosure.
                                if let Some(v) = self.variants.get_mut(0) {
                                    v.origin = Some(o.clone());
                                }
                                if let Some(photo) = self.src_path.clone() {
                                    self.spawn_master_load(ctx, photo, o);
                                }
                            }
                            // Restore the BACKGROUND variants the stash
                            // carried (H4): the strip used to collapse to
                            // the active canvas alone, silently dropping
                            // every other variant's unsaved work.
                            if from_stash && !stash_others.is_empty() {
                                let active_v = self.variants.remove(0);
                                let mut strip: Vec<Variant> = stash_others
                                    .drain(..)
                                    .map(|sv| Variant {
                                        kind: sv.kind,
                                        // Hop 4 of 6 (R24-2): the stash back
                                        // into the live strip.
                                        id: sv.id,
                                        name: sv.name,
                                        recipe: sv.recipe,
                                        base: sv.base,
                                        origin: sv.origin,
                                        thumb: None,
                                    })
                                    .collect();
                                let pos = stash_active_pos.min(strip.len());
                                strip.insert(pos, active_v);
                                self.variants = strip;
                                self.active = pos;
                            } else if !from_stash
                                && let Some(rec) = &disk_strip
                                && !rec.others.is_empty()
                            {
                                // Cold restore of the persisted strip: the
                                // background variants come back with their
                                // recipes and raster origins; base pixels
                                // re-decode lazily on first switch
                                // (`load_active`), the same deal a stash
                                // restore gets from its held Arcs. Pre-era
                                // curves heal exactly like the stash arm
                                // above — a strip entry is one click from
                                // BEING the canvas. (dirty_vs neutralises
                                // version/base_curve, so healing the live
                                // strip never lights ● against the mirror.)
                                let active_v = self.variants.remove(0);
                                let mut strip = strip_from_record(rec, self.src_path.as_deref());
                                let pos = rec.active_pos.min(strip.len());
                                strip.insert(pos, active_v);
                                self.variants = strip;
                                self.active = pos;
                            }
                            self.saved_strip = disk_strip;
                            self.last_rgb = None; // retained frame was the old photo's
                            self.reset_history(); // a new photo starts a fresh undo history
                            self.region = None; // and a fresh local-edit selection
                            self.region_drag = None;
                            // View + tool state is per-photo.
                            self.zoom = 1.0;
                            self.zoom_target = 1.0; // instant — no glide across photos
                            self.pan = egui::vec2(0.5, 0.5);
                            self.disarm_tools();
                            // The \-latch is per-photo transient state:
                            // carried across an open, the new photo arrived
                            // with every tool locked out (M21).
                            self.before_latch = false;
                            self.sel_mask = None;
                            self.sel_component = None;
                            self.overlay_ref = None; // the reference develop belongs to ONE base
                            self.overlay_stale = true;
                            self.curve_drag = None; // curve_channel is a UI pref, keep it
                            self.wb_picking = false;
                            self.range_picking = None;
                            self.clone_mode = false;
                            self.clone_src = None;
                            self.verdict = None;
                            // R23-6 B: the chosen reverse-fit reference names
                            // a rendition of THE PREVIOUS photo. Carried
                            // across an open it would silently fit this photo
                            // against the last one's export — the exact
                            // "reference is not this frame" case the fit can
                            // now only warn about.
                            self.fit_ref = None;
                            // (rationale already restored alongside the recipe)
                            self.refresh_versions(); // version snapshots are per-photo
                            self.dirty = true; // render the (restored or neutral) after
                            self.status = if from_stash {
                                tr(
                                    lang,
                                    "ready — restored this session's unsaved edits (● not saved yet; Ctrl+S)",
                                )
                                .into()
                            } else {
                                match restored {
                                    Some(kind) => trf(
                                        lang,
                                        "ready — restored saved edits ({kind}); Reset returns to neutral",
                                        &[("kind", kind)],
                                    ),
                                    None => {
                                        tr(lang, "ready — adjust sliders or run AI Analyze").into()
                                    }
                                }
                            };
                            // The third door: a stash restore reinstalls the
                            // raster this photo was LEFT with, at its own
                            // edge, while this open decoded the source at the
                            // current preference — which may have moved while
                            // the user was on another photo.
                            if let Some(px) = self.baked_canvas_edge() {
                                let extra = trf(
                                    lang,
                                    " · restored pixels stay at {px}px (their own bake)",
                                    &[("px", &px.to_string())],
                                );
                                self.status.push_str(&extra);
                            }
                            if let Some(note) = open_note {
                                // Sidecar anomalies must be seen, not scrolled
                                // away in the status line.
                                self.toast(ToastKind::Error, note);
                            }
                        }
                    }
                    Err(e) => {
                        self.fail(tr(lang, "could not open"), e);
                        if !keep {
                            // A FRESH open failed: open_path already re-pointed
                            // src_path to the file that wouldn't decode, but the
                            // previous photo's variants / pixels are still live —
                            // a mismatch that would misdirect a later fit / heal /
                            // XMP (they key off src_path). Drop to a clean
                            // no-photo state so src_path and the variants can never
                            // disagree. (A preview-res re-decode failure — keep —
                            // leaves the still-open photo untouched.)
                            self.src_path = None;
                            self.variants.clear();
                            self.active = 0;
                            self.base_preview = None;
                            self.source_preview = None;
                            self.before_tex = None;
                            self.after_tex = None;
                            self.selected = None;
                            // …and its HISTORY, which this arm used to leave
                            // standing: the steps hold the dead photo's
                            // rasters and its recipe, so an undo here restored
                            // a canvas that no longer exists (the keyboard
                            // path reached it because it checked only `busy`).
                            // Gating the key is the guard; clearing the stack
                            // is the reason there is nothing to guard. The
                            // photo's unsaved work is NOT lost — open_path
                            // stashed it before the flight.
                            self.reset_history();
                        } else if let Some(p) = self.src_path.clone() {
                            // A preview-res re-decode failed: the photo (and
                            // its live canvas) is untouched — consume the
                            // stash entry the same-path open_path call just
                            // wrote, exactly like the successful keep branch.
                            // Left behind, an undo-to-clean before the next
                            // navigation resurrected these pre-undo edits on
                            // the way back (CX4-4).
                            self.nav_stash.remove(&p);
                            // ...and the switch never materialized: the
                            // canvas kept the OLD edge, so preview_edge
                            // reverts with it (the busy-refusal rule) —
                            // leaving the new value displayed an unreachable
                            // resolution and fed it to every bake site under
                            // an old-edge canvas.
                            if let Some(e) = edge_revert {
                                self.preview_edge = e;
                            }
                        }
                    }
                }
    }

    /// `Msg::Analyzed` landing — body extracted verbatim from the
    /// poll_workers pump (round-12 decomposition; indentation kept).
    fn on_analyzed(
        &mut self,
        lang: Lang,
        epoch: u64,
        boxed: Box<
            anyhow::Result<(
                EditRecipe,
                autoshop::advisor::Verdict,
                Vec<autoshop::rationale::Note>,
            )>,
        >,
    ) {
                    // Cleared whatever the epoch: the wire is free either way,
                    // and this is what re-arms the Analyze button.
                    self.analyze_inflight = false;
                    if epoch != self.gen_epoch {
                        // A cancelled Analyze's late result: the user already
                        // moved on. The Ok arm below also PERSISTS (backup +
                        // recipe.json + XMP) — a stale install would not just
                        // repaint the canvas, it would save over the photo's
                        // develop. Discard entirely (Err is just as silent).
                        return; // was `continue` — the match was the loop's last statement
                    }
                    self.gen_cancel = None;
                    match *boxed {
                    Ok((recipe, verdict, rationale_notes)) => {
                        // Sliders stay live while Analyze runs (10-30 s):
                        // flush any name typed during the wait (the resync
                        // below clears the mask buffer — unflushed, the
                        // rename died with it, M16), then commit any
                        // mid-flight edit as its own undo step NOW, or the
                        // wholesale install below folds it into the
                        // pre-analyze step and Ctrl+Z skips it.
                        self.commit_pending_names();
                        self.commit_now();
                        let accepted =
                            verdict.decision == autoshop::advisor::Decision::Accept;
                        // The recipe arrives with its base_curve already
                        // stamped by produce_recipe (saved-curve-first, else a
                        // fresh estimate) — the single authority all three
                        // surfaces share; the canvas must not override it.
                        // One exception, canvas-side only: a GENERATED variant
                        // already carries the camera look AND the lens
                        // corrections in its pixels, so developing it through
                        // the RAW's curve would cook both twice — the canvas
                        // copy drops them; the persist below still writes the
                        // full stamped recipe (it is the RAW's saved develop,
                        // and reopening lands on the source). An InPlace
                        // retouch master is a NEUTRAL develop (see the
                        // RetouchKind::InPlace arm): its recipe legitimately
                        // keeps the calibration — stripping there rendered the
                        // canvas neutral-dark while the disk recipe kept the
                        // look.
                        let stamped = recipe;
                        let mut canvas = stamped.clone();
                        if self.active_is_generated() {
                            canvas.base_curve = Vec::new();
                            canvas.lens_profile = Default::default();
                            // Anchor follows the strip rule (baked WB).
                            canvas.as_shot_k = None;
                            canvas.as_shot_tint = None;
                        }
                        self.recipe = canvas;
                        // Wholesale replacement: disarm index-carrying tools +
                        // refresh rationale, THEN install the fresh verdict
                        // and the typed notes (resync cleared both).
                        self.resync_recipe_display();
                        self.rationale_notes = rationale_notes;
                        self.verdict =
                            Some((verdict.decision.clone(), verdict.reasons.clone()));
                        self.dirty = true;
                        self.busy = false;
                        // Persist like the CLI and the web do — the badge,
                        // batch render and reopening all read recipe.json, so
                        // an unpersisted analyze silently diverges from every
                        // one of them. An existing explicit save is snapshotted
                        // to v<N> first, never destroyed.
                        self.status = if !accepted {
                            // The verifier itself judged this result not
                            // ready, and a non-Accept verdict may not
                            // auto-save (user decision). The result stays on
                            // the canvas as an UNSAVED edit — ● lit, the nav
                            // stash and the close guard protect it — and the
                            // user decides: Ctrl+S keeps it, Ctrl+Z steps
                            // back to the pre-analyze edit.
                            let t = trf(
                                lang,
                                "AI develop applied — verdict {v}: NOT saved (Ctrl+S keeps it, Ctrl+Z steps back)",
                                &[("v", tr(lang, autoshop::advisor::decision_key(&verdict.decision)))],
                            );
                            self.toast(ToastKind::Success, t.clone());
                            t
                        } else { match self.src_path.clone() {
                            // The backup gate comes FIRST: if the existing save
                            // cannot be snapshotted, it is not overwritten —
                            // the analyze result stays on the canvas only.
                            // Persist `stamped`, not the canvas copy: the
                            // sidecar is the RAW's develop and keeps its curve.
                            // ONE NoWait lock across gate → recipe → pixel
                            // link → XMP (the Ctrl+S pairing): busy must
                            // degrade to "applied but NOT saved", never hang
                            // the UI thread behind another process.
                            Some(p) => match autoshop::store::with_develop_lock(
                                &p,
                                autoshop::store::DevelopLockMode::NoWait,
                                || -> std::io::Result<String> {
                                    Ok(match autoshop::store::backup_saved_develop(&p, Some(&stamped)) {
                                Err(e) => {
                                    let t = trf(
                                        lang,
                                        "AI develop applied — but NOT saved: backing up your existing save failed ({err}); Ctrl+S overwrites explicitly",
                                        &[("err", &e.to_string())],
                                    );
                                    self.toast(ToastKind::Error, t.clone());
                                    t
                                }
                                Ok(backed) => {
                                    // The recipe write ALONE decides the saved
                                    // state (same rule as save_xmp): once it
                                    // lands, reopening restores it regardless
                                    // of the XMP — so the ● baseline follows
                                    // it even when the XMP half fails.
                                    // Analyze is a SAVER (same rule as
                                    // Ctrl+S) — ONE single-generation
                                    // commit (L03): recipe + pixel link
                                    // land whole or not at all, so the
                                    // stash protection stays armed on any
                                    // failure instead of half a save
                                    // declaring everything safe.
                                    // The STRIP lands in the same generation
                                    // (R24-4): Analyze used to commit
                                    // `variants: Keep`, so a record written
                                    // before the user switched cards kept
                                    // describing the OLD active card while
                                    // recipe.json already held this one's AI
                                    // develop — the registered cross-surface
                                    // residual, closed here. This landing
                                    // runs on the UI thread and therefore
                                    // OWNS the live strip: it hands the whole
                                    // record over (`ActiveWrite::Strip`),
                                    // exactly like Ctrl+S, and advances the
                                    // mirror below on success.
                                    let origin = self
                                        .active_variant()
                                        .and_then(|v| v.origin.clone());
                                    let generated = self.active_is_generated();
                                    let strip_rec = self.current_strip_record();
                                    let commit_res: anyhow::Result<()> = (|| {
                                        let recipe_bytes =
                                            autoshop::pipeline::recipe_store_bytes(&p, &stamped)?;
                                        let pixels = match &origin {
                                            Some(o) => autoshop::store::CommitMember::Write(
                                                autoshop::store::pixel_source_record_bytes(
                                                    &p, o, generated,
                                                )?,
                                            ),
                                            None => autoshop::store::CommitMember::Clear,
                                        };
                                        let variants = autoshop::store::variants_member(
                                            &p,
                                            autoshop::store::ActiveWrite::Strip(
                                                strip_rec.as_ref(),
                                            ),
                                        )?;
                                        autoshop::store::commit_develop(
                                            &p,
                                            autoshop::store::DevelopCommit {
                                                recipe: Some(recipe_bytes),
                                                pixels,
                                                variants,
                                            },
                                        )?;
                                        Ok(())
                                    })();
                                    match commit_res {
                                        Ok(()) => {
                                            self.edited_badge.clear();
                                            // The ● baseline lives in CANVAS
                                            // coordinates: on a baked variant
                                            // the canvas copy dropped the
                                            // curve + lens profile, and a
                                            // baseline keeping them (the
                                            // disk form) lit ● the instant a
                                            // successful analyze landed.
                                            self.saved_recipe = self.recipe.clone();
                                            self.nav_stash.remove(&p);
                                            self.pixels_on_disk = origin;
                                            // The strip mirror advances WITH
                                            // the commit (the Ctrl+S rule):
                                            // all-or-nothing, so a failed
                                            // commit leaves it untouched and
                                            // the background-variant unsaved
                                            // protection armed.
                                            self.saved_strip = strip_rec;
                                            self.forget_open_base();
                                            if backed.is_some() {
                                                self.refresh_versions();
                                            }
                                            let mut s = match backed {
                                                Some(n) => trf(
                                                    lang,
                                                    "AI develop applied · saved (previous save backed up as v{n})",
                                                    &[("n", &n.to_string())],
                                                ),
                                                None => tr(lang, "AI develop applied · saved to recipe.json").to_string(),
                                            };
                                            if autoshop::decode::is_raw(&p) {
                                                match autoshop::pipeline::write_xmp(&p, &stamped) {
                                                    // Regenerated-not-merged AND
                                                    // the M6a projection losses:
                                                    // the same disclosure as
                                                    // Ctrl+S, in the same two
                                                    // voices. Analyze is where
                                                    // AI bitmap masks arrive, so
                                                    // this landing needs the
                                                    // export-side line as much
                                                    // as the save does.
                                                    Ok((_, merge_note, losses)) => {
                                                        // A regenerated-not-merged
                                                        // sidecar always
                                                        // interrupts: it dropped
                                                        // the user's own
                                                        // Lightroom properties.
                                                        if let Some(m) = merge_note {
                                                            self.toast(ToastKind::Error, m.clone());
                                                            s = format!("{s} — ⚠ {m}");
                                                        }
                                                        // The projection's own
                                                        // losses, with Ctrl+S's
                                                        // interrupt rule (MED-2 —
                                                        // engine calibration is
                                                        // said quietly, a user's
                                                        // own mask is not).
                                                        let globals =
                                                            autoshop::xmp::global_export_losses(
                                                                &stamped,
                                                            );
                                                        if let Some(m) =
                                                            xmp_loss_line(lang, &losses, &globals)
                                                        {
                                                            if xmp_loss_interrupts(&losses, &globals)
                                                            {
                                                                self.toast(
                                                                    ToastKind::Error,
                                                                    m.clone(),
                                                                );
                                                                s = format!("{s} — ⚠ {m}");
                                                            } else {
                                                                s = format!("{s} · {m}");
                                                            }
                                                        }
                                                    }
                                                    Err(e) => {
                                                        let t = trf(
                                                            lang,
                                                            "recipe saved — but the Lightroom XMP failed: {err}",
                                                            &[("err", &e.to_string())],
                                                        );
                                                        self.toast(ToastKind::Error, t.clone());
                                                        s = t;
                                                    }
                                                }
                                            }
                                            s
                                        }
                                        Err(e) => {
                                            // The old save was already COPIED
                                            // to v<N>, so nothing is lost —
                                            // but this photo's working save
                                            // failed, and a status line alone
                                            // scrolls away.
                                            let t = trf(
                                                lang,
                                                "AI develop applied — but saving the sidecar failed: {err}",
                                                &[("err", &e.to_string())],
                                            );
                                            self.toast(ToastKind::Error, t.clone());
                                            self.refresh_versions();
                                            t
                                        }
                                    }
                                }
                                    })
                                },
                            ) {
                                Ok(status) => status,
                                Err(e) => {
                                    self.persist_postponed(
                                        &e,
                                        "AI develop applied — but NOT saved: this photo is being changed by another Autoshop process ({err}); Ctrl+S retries",
                                        &[],
                                    );
                                    self.status.clone()
                                }
                            },
                            None => tr(lang, "AI develop applied").into(),
                        } };
                    }
                    Err(e) => {
                        self.fail(tr(lang, "analyze failed"), e);
                    }
                }
    }

    /// `Msg::StyleBuilt` landing (R23-2): the typed outcome becomes a sentence
    /// in the language live NOW (L12#4), the button re-arms, and a successful
    /// build both remembers its folder (so a rebuild starts there) and triggers
    /// a fresh status read — the panel must not keep showing the OLD counts.
    fn on_style_built(&mut self, lang: Lang, outcome: StyleBuildOutcome) {
        self.style_build_inflight = false;
        self.style_build_progress = None;
        match outcome {
            StyleBuildOutcome::Saved { total, dir } => {
                let t = trf(
                    lang,
                    "Style library built: {n} of your own edits from {path}",
                    &[("n", &total.to_string()), ("path", &abs_display(&dir))],
                );
                self.status = t.clone();
                self.toast(ToastKind::Success, t);
                self.style_src_dir = Some(dir);
                self.start_style_info();
            }
            // The shared guard's refusal, in the shared WORDING (serve.rs says
            // the same thing to the web): the folder, why it yielded nothing,
            // where to point instead, and that the old library still stands.
            StyleBuildOutcome::NothingIndexed { dir } => {
                let t = trf(
                    lang,
                    "Nothing to index in {path} — no RAW there has its .xmp sidecar beside it (Autoshop keeps its own .xmp in the develop store, never beside the RAW, so point this at the folder you edit in Lightroom). Your existing style library was left untouched.",
                    &[("path", &abs_display(&dir))],
                );
                self.status = t.clone();
                self.toast(ToastKind::Error, t);
            }
            StyleBuildOutcome::Failed { err } => {
                let t = trf(
                    lang,
                    "Building the style library failed: {err}",
                    &[("err", &err)],
                );
                self.status = t.clone();
                self.toast(ToastKind::Error, t);
            }
        }
    }

    /// `Msg::Segmented` landing — body extracted verbatim from the
    /// poll_workers pump (round-12 decomposition; indentation kept).
    fn on_segmented(&mut self, lang: Lang, res: anyhow::Result<(String, PathBuf)>) {
                match res {
                    Ok((label, path)) => {
                        let path_s = path.to_string_lossy().into_owned();
                        // One raster per (photo, target): a re-run refreshed the
                        // SAME file, so re-select the mask that already
                        // references it instead of stacking a duplicate (whose
                        // sliders would all be 0 — an inert-looking copy).
                        // Compare by RASTER NAME, not full string: the stored
                        // reference may be bare ("mask-sky.png"), legacy
                        // ("out/<stem>.mask-sky.png") or absolute, while
                        // `path` is the absolute target — the kind-name is
                        // unique per photo per target by construction. On a
                        // hit, re-point the stored path at the freshly written
                        // raster so a stale reference is revived instead of
                        // stranding the user's sliders on a dead mask.
                        let new_name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or_default()
                            .to_string();
                        let stem_prefix = self
                            .src_path
                            .as_deref()
                            .map(|p| format!("{}.", autoshop::pipeline::stem(p)));
                        let existing = self.recipe.masks.iter_mut().enumerate().find_map(
                            |(i, m)| match &mut m.mask {
                                autoshop::recipe::MaskGeometry::Bitmap { path: p } => {
                                    let stored = std::path::Path::new(p.as_str())
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or_default();
                                    let bare = stem_prefix
                                        .as_deref()
                                        .and_then(|sp| stored.strip_prefix(sp))
                                        .unwrap_or(stored);
                                    // Strip the NEW name symmetrically: a
                                    // legacy-out target carries the stem
                                    // prefix that a central bare name lacks —
                                    // one-sided stripping missed the match
                                    // and stacked an inert duplicate.
                                    let new_bare = stem_prefix
                                        .as_deref()
                                        .and_then(|sp| new_name.strip_prefix(sp))
                                        .unwrap_or(&new_name);
                                    // Compare by name FAMILY ("mask-sky-3.png"
                                    // → "mask-sky"): rasters are claim_raster
                                    // uniques now, so a rerun's new name must
                                    // still REPOINT the existing entry.
                                    (mask_family(bare) == mask_family(new_bare)).then(|| {
                                        *p = path_s.clone();
                                        i
                                    })
                                }
                                _ => None,
                            },
                        );
                        let reused = existing.is_some();
                        match existing {
                            Some(i) => self.sel_mask = Some(i),
                            None => {
                                self.recipe.masks.push(autoshop::recipe::LocalAdjustment {
                                    mask: autoshop::recipe::MaskGeometry::Bitmap {
                                        path: path_s,
                                    },
                                    name: label.clone(),
                                    ..Default::default()
                                });
                                self.sel_mask = Some(self.recipe.masks.len() - 1);
                            }
                        }
                        // The landing MOVED the selection — index-armed tools
                        // bound to another row die with it, exactly like the
                        // row-click sites (Codex L06-1: this async door was
                        // missed and a stranded brush baked into the old mask).
                        if self.disarm_selection_bound_tools(self.sel_mask) {
                            let t = tr(
                                lang,
                                "the AI result took the selection — a brush session on another mask ended; its unbaked strokes were discarded",
                            );
                            self.toast(ToastKind::Error, t.to_string());
                        }
                        self.overlay_stale = true; // fresh raster ⇒ fresh coverage
                        self.dirty = true; // committed-snapshot makes this one undo step
                        self.busy = false;
                        // A rerun REFRESHES the existing mask's raster — its
                        // sliders may already apply; claiming "added, adjust
                        // sliders" there was false on both counts.
                        self.status = if reused {
                            trf(
                                lang,
                                "AI「{what}」mask refreshed — the existing mask now uses the new selection (its sliders still apply)",
                                &[("what", &label)],
                            )
                        } else {
                            trf(
                                lang,
                                "AI「{what}」mask added — adjust its sliders (exposure / contrast / saturation…) to take effect",
                                &[("what", &label)],
                            )
                        };
                    }
                    Err(e) => {
                        self.fail(tr(lang, "AI segmentation failed"), e);
                    }
                }
    }

    /// `Msg::MaskRefined` landing — body extracted verbatim from the
    /// poll_workers pump (round-12 decomposition; indentation kept).
    fn on_mask_refined(&mut self, lang: Lang, res: anyhow::Result<MaskRefineOutcome>) {
                match res {
                    Ok(MaskRefineOutcome::OverBudget { w, h }) => {
                        // Nothing was written (masks.rs refuses before the
                        // claim): the mask keeps the raster it had.
                        self.busy = false;
                        let t = trf(
                            lang,
                            "this source is {w}×{h} — a full-resolution refined mask would exceed the mask budget and could never be read back",
                            &[("w", &w.to_string()), ("h", &h.to_string())],
                        );
                        self.toast(ToastKind::Error, t.clone());
                        self.status = t;
                    }
                    Ok(MaskRefineOutcome::Refined(idx, stored_ref, out)) => {
                        // Index + stored reference validated TOGETHER: the
                        // strip may have been edited while the worker decoded
                        // the full-res source, and a bare path search could
                        // repoint the wrong mask when two masks share one
                        // raster. Index-with-matching-path wins; else the
                        // unique path match; else say so — never guess.
                        let out_s = out.to_string_lossy().into_owned();
                        let at = |m: &autoshop::recipe::LocalAdjustment| {
                            matches!(&m.mask, MaskGeometry::Bitmap { path } if *path == stored_ref)
                        };
                        let hit = match self.recipe.masks.get(idx) {
                            Some(m) if at(m) => Some(idx),
                            _ => {
                                let matches: Vec<usize> = self
                                    .recipe
                                    .masks
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, m)| at(m))
                                    .map(|(i, _)| i)
                                    .collect();
                                // Only an UNAMBIGUOUS survivor is adopted.
                                (matches.len() == 1).then(|| matches[0])
                            }
                        };
                        match hit {
                            Some(i) => {
                                self.recipe.masks[i].mask = MaskGeometry::Bitmap { path: out_s };
                                self.sel_mask = Some(i);
                                // Same rule as the segmentation landing: the
                                // selection moved, selection-bound tools on
                                // another row follow it (Codex L06-1).
                                if self.disarm_selection_bound_tools(self.sel_mask) {
                                    let t = tr(
                                        lang,
                                        "the AI result took the selection — a brush session on another mask ended; its unbaked strokes were discarded",
                                    );
                                    self.toast(ToastKind::Error, t.to_string());
                                }
                                self.dirty = true;
                                self.overlay_stale = true;
                                self.busy = false;
                                self.status = tr(
                                    lang,
                                    "mask refined to full resolution — boundaries now follow the source's own edges",
                                )
                                .into();
                            }
                            None => {
                                self.busy = false;
                                let t = trf(
                                    lang,
                                    "the mask changed while refining — the result was saved at {path} but not applied",
                                    &[("path", &out.display().to_string())],
                                );
                                self.toast(ToastKind::Error, t.clone());
                                self.status = t;
                            }
                        }
                    }
                    Err(e) => {
                        self.fail(tr(lang, "mask refine failed"), e);
                    }
                }
    }

    /// `Msg::Folder` landing — body extracted verbatim from the
    /// poll_workers pump (round-12 decomposition; indentation kept).
    fn on_folder(&mut self, lang: Lang, res: anyhow::Result<(PathBuf, Vec<PathBuf>, usize)>) {
                match res {
                    Ok((dir, list, skipped)) => {
                        let n = list.len();
                        self.gallery = list;
                        self.gallery_dir = Some(dir);
                        self.gallery_gen += 1; // invalidate any in-flight old thumbs
                        self.thumbs.clear();
                        self.thumb_requested.clear();
                        // Keyed by gallery INDEX, so it is meaningless across
                        // folders: without this, index 0 failing three times
                        // here silently denied index 0 a thumbnail in every
                        // folder opened afterwards.
                        self.thumb_fail.clear();
                        self.thumb_inflight = 0;
                        self.edited_badge.clear(); // badge stats belong to the old folder
                        self.selected = None;
                        self.multi_sel.clear(); // indices belong to the old folder
                        self.busy = false;
                        self.status = if n == 0 {
                            // The plural line says "click a thumbnail" — with
                            // zero rows there is none to click.
                            tr(lang, "no photos found in this folder").to_string()
                        } else if n == 1 {
                            tr(lang, "1 photo — click a thumbnail to open").to_string()
                        } else {
                            trf(lang, "{n} photos — click a thumbnail to open", &[("n", &n.to_string())])
                        };
                        // L08: each unreadable entry went to stderr as the
                        // scan hit it — invisible in the windowed GUI, so
                        // the count rides the scan's own status answer.
                        if skipped > 0 {
                            let w = trf(
                                lang,
                                "{n} unreadable item(s) skipped during the folder scan",
                                &[("n", &skipped.to_string())],
                            );
                            self.status = format!("{} — ⚠ {}", self.status, w);
                        }
                        // L12#3 tofu disclosure: a gallery name in a script
                        // no installed face covers renders as boxes — say
                        // so ONCE per script per session, at folder open
                        // (the one cheap point; never per frame).
                        self.disclose_undrawable_names();
                    }
                    Err(e) => {
                        self.fail(tr(lang, "scan failed"), e);
                    }
                }
    }

    /// `Msg::Thumb` landing — body extracted verbatim from the
    /// poll_workers pump (round-12 decomposition; indentation kept).
    fn on_thumb(&mut self, ctx: &egui::Context, generation: u64, idx: usize, img: anyhow::Result<image::DynamicImage>) {
                    // Ignore thumbnails from a previous folder generation (their
                    // inflight count was already discarded when the folder changed).
                    if generation == self.gallery_gen {
                        self.thumb_inflight = self.thumb_inflight.saturating_sub(1);
                        match img {
                            Ok(im) => {
                                let tex = ctx.load_texture(
                                    format!("thumb{idx}"),
                                    to_color_image(&im),
                                    egui::TextureOptions::LINEAR,
                                );
                                self.thumbs.insert(idx, tex);
                            }
                            Err(_) => {
                                // Count the failure and release the queue slot:
                                // `request_thumb` allows three attempts, so a
                                // transient decode failure (AV lock, a slow
                                // share) still recovers on a later frame while
                                // an undecodable file stops for good instead of
                                // respawning decode threads every frame.
                                *self.thumb_fail.entry(idx).or_insert(0) += 1;
                                self.thumb_requested.remove(&idx);
                            }
                        }
                    }
    }

    /// `Msg::MasterLoaded` landing — body extracted verbatim from the
    /// poll_workers pump (round-12 decomposition; indentation kept).
    #[allow(clippy::too_many_arguments)]
    fn on_master_loaded(&mut self, ctx: &egui::Context, lang: Lang, photo: PathBuf, origin: PathBuf, edge: u32, stamp: FileStamp, img: anyhow::Result<image::DynamicImage>) {
                    // The in-flight marker clears on EVERY outcome — photo
                    // mismatch included — or one failed decode would block
                    // all retries for the rest of the session.
                    self.master_loads.remove(&(photo.clone(), origin.clone()));
                    // Install by identity, not index: only while the SAME
                    // photo is open, and only into strip entries that still
                    // reference this exact master and still await pixels.
                    if self.src_path.as_ref() == Some(&photo) {
                        match img {
                            Ok(im) => {
                                let arc = Arc::new(im);
                                // Remember before installing, under the
                                // SPAWN-time edge + stamp (L12-2/L12-6): the
                                // current preference may have moved while
                                // this decoded, and these pixels are the
                                // spawn's, not the preference's.
                                self.remember_master(&origin, edge, stamp, arc.clone());
                                self.install_master(ctx, &photo, &origin, arc);
                            }
                            Err(e) => {
                                let t = trf(
                                    lang,
                                    "this variant's saved master could not be loaded ({err}) — showing the un-retouched source develop instead",
                                    &[("err", &e.to_string())],
                                );
                                self.toast(ToastKind::Error, t);
                            }
                        }
                    }
    }

    /// Render one retouch fact in the CURRENT language (L12#4) — the
    /// worker returns [`RetouchNote`] facts; this runs at landing time.
    fn render_retouch_note(lang: Lang, n: &RetouchNote) -> String {
        match n {
            RetouchNote::Filled(p) => trf(
                lang,
                "filled → {path} (updated current variant)",
                &[("path", &p.display().to_string())],
            ),
            RetouchNote::Healed { n, out, ai_prose, notes } => {
                let mut s = trf(
                    lang,
                    "healed {n} spot(s) → {path}",
                    &[("n", &n.to_string()), ("path", &out.display().to_string())],
                );
                if !ai_prose.is_empty() || !notes.is_empty() {
                    // AI prose stays raw (the model's own text); the typed
                    // notes render localized (L12#2B).
                    let mut tail = ai_prose.clone();
                    for note in notes {
                        let args: Vec<(&str, &str)> =
                            note.args.iter().map(|(k, v)| (*k, v.as_str())).collect();
                        tail.push_str(&trf(lang, note.key, &args));
                    }
                    s = format!("{s} — ⚠ {tail}");
                }
                s
            }
            RetouchNote::Denoised(p) => trf(
                lang,
                "AI denoised → {path} (updated current variant)",
                &[("path", &p.display().to_string())],
            ),
            RetouchNote::Cloned { n, out } => trf(
                lang,
                "Cloned {n} spot(s) → {path}",
                &[("n", &n.to_string()), ("path", &out.display().to_string())],
            ),
            RetouchNote::Reimagined(p) => trf(
                lang,
                "「AI generated」variant created → {path} · keep tweaking or 「Reverse-fit」",
                &[("path", &p.display().to_string())],
            ),
        }
    }

    /// `Msg::Retouched` landing — body extracted verbatim from the
    /// poll_workers pump (round-12 decomposition; indentation kept).
    // pub(crate): the L06#4 commit-ordering test drives this landing directly.
    pub(crate) fn on_retouched(&mut self, ctx: &egui::Context, lang: Lang, epoch: u64, done: RetouchDone) {
                    if epoch != self.gen_epoch {
                        // A cancelled task's late result: the user already
                        // moved on — never let it mutate the canvas. And the
                        // cancel toast PROMISES "the late result is
                        // discarded" (L12-7): a late SUCCESS's ./out artifact
                        // is unreferenced by construction (unique_out claimed
                        // a fresh name; nothing installed it), so it is
                        // removed instead of accumulating one orphan
                        // full-res file per cancel. Err stays silent.
                        if let Ok((_, _, saved, _)) = &done {
                            let _ = std::fs::remove_file(saved);
                        }
                        return; // was `continue` — the match was the loop's last statement
                    }
                    self.gen_cancel = None;
                    match done {
                    Ok((img, note, saved, kind)) => {
                        let msg = Self::render_retouch_note(lang, &note);
                        self.clear_mask();
                        match kind {
                            RetouchKind::NewGenerated => {
                                // Whole-frame reimagine → a NEW「AI 生成」variant
                                // whose base IS this raster. Auto-switch to it, so
                                // editing works on the generated pixels — a slider
                                // no longer reverts to the source develop. Its path
                                // is the reverse-fit / full-res export source.
                                self.push_variant(
                                    Variant {
                                        kind: VariantKind::Generated,
                                        id: new_variant_id(),
                                        name: None,
                                        recipe: EditRecipe::default(),
                                        base: Some(Arc::new(img)),
                                        origin: Some(saved),
                                        thumb: None,
                                    },
                                    ctx,
                                );
                            }
                            RetouchKind::InPlace => {
                                // fill/heal/clone/denoise: a pixel touch-up of the
                                // NEUTRAL-DEVELOP base (never the developed
                                // rendition — the recipe keeps rendering ON TOP,
                                // so baking developed pixels would cook the tone
                                // twice). Bake it into the active variant's base
                                // AND repoint its origin at the saved full-res
                                // artifact, so export / reverse-fit / a further
                                // retouch all follow the retouched pixels.
                                //
                                // Sliders stay live while a retouch runs (minutes
                                // for a generative fill) and `current_step` reads
                                // the variant's base — committing only AFTER the
                                // swap below folded a mid-flight recipe edit into
                                // the pixel step, so one Ctrl+Z reverted both and
                                // there was no way to keep the slider move while
                                // dropping the retouch. Same rule, same reason as
                                // the Analyze landing: commit (and flush every
                                // typed name) BEFORE the state swap.
                                self.commit_pending_names();
                                self.commit_now();
                                let img = Arc::new(img);
                                let (mw, mh) = img.dimensions();
                                if let Some(v) = self.variants.get_mut(self.active) {
                                    v.base = Some(img.clone());
                                    v.origin = Some(saved);
                                }
                                // The InPlace master is a NEUTRAL develop (see
                                // above) — Before shows it under the recipe's
                                // own camera curve, exactly like the source
                                // compare; an empty curve here sat 0.6–1.4 EV
                                // under After's starting point.
                                let curve = self.recipe.base_curve.clone();
                                self.set_before(ctx, &img, &curve);
                                // New baked base ⇒ the cached masks-cleared
                                // reference develop no longer matches it.
                                self.overlay_ref = None;
                                self.overlay_stale = true;
                                self.base_preview = Some(img);
                                // Keep the paint canvas sized to the new base (the
                                // retouch result can differ in dimensions — e.g. a
                                // non-square reimagine origin) — and end a brush
                                // session armed on the replaced plate.
                                self.rebind_paint_canvas(mw, mh);
                                self.dirty = true;
                                // The pixel swap must enter history THIS frame
                                // — a same-frame Ctrl+Z otherwise undoes an
                                // older recipe step over the new pixels.
                                self.commit_now();
                                // The decoded-base cache now describes pixels
                                // this photo no longer shows.
                                self.forget_open_base();
                            }
                        }
                        self.done(msg);
                    }
                    Err(e) => {
                        self.fail(tr(lang, "retouch failed"), e);
                    }
                    }
    }

    /// Render one reverse-fit landing fact in the CURRENT language — the
    /// worker returns [`FitNote`] facts, and this runs at landing time, so
    /// a language switch during the multi-minute fit renders fresh (L12#4).
    pub(crate) fn render_fit_note(lang: Lang, n: &FitNote) -> String {
        match n {
            FitNote::IncludesSkyZone => tr(
                lang,
                " · includes sky-zone correction (adjustable in the mask panel; XMP carries the global part only)",
            )
            .to_string(),
            FitNote::NotPersistedCommit(e) => trf(
                lang,
                " · NOT persisted: saving the develop failed ({err}) — Ctrl+S to save explicitly",
                &[("err", e)],
            ),
            FitNote::XmpWritten(p) => {
                trf(lang, " · XMP → {path}", &[("path", &p.display().to_string())])
            }
            FitNote::XmpMergeNote(m) => trf(lang, " · ⚠ {note}", &[("note", m)]),
            FitNote::XmpFailed(e) => {
                let mut s = String::from(" · ");
                s.push_str(&trf(
                    lang,
                    "recipe saved — but the Lightroom XMP failed: {err}",
                    &[("err", e)],
                ));
                s
            }
            FitNote::BackedUpAs(n) => {
                trf(lang, " · previous save backed up as v{n}", &[("n", &n.to_string())])
            }
            FitNote::NotPersistedBackup(e) => trf(
                lang,
                " · NOT persisted: backing up your existing save failed ({err}) — Ctrl+S to save explicitly",
                &[("err", e)],
            ),
            FitNote::NotPersistedLock(e) => trf(
                lang,
                " · NOT persisted: the develop store could not be locked ({err}) — Ctrl+S to save explicitly",
                &[("err", e)],
            ),
            // R20 opt-in AI review. The critique is model English by the
            // rationale contract (args ride verbatim, like {err} text).
            FitNote::AiReview { score, critique, hint } => {
                let mut s = trf(
                    lang,
                    " · AI review: match {score}/100 — {critique}",
                    &[("score", &format!("{score:.0}")), ("critique", critique)],
                );
                // R23-6: the third of the three things the paid call
                // returned. R20 kept the score and the critique and dropped
                // this one with no reason on record. Labelled as a
                // SUGGESTION because on this path nothing acted on it.
                if let Some(h) = hint.as_deref().map(str::trim).filter(|h| !h.is_empty()) {
                    s.push_str(&trf(
                        lang,
                        " · it suggests: {hint} (nothing was changed — tick 「deep」 to let it try)",
                        &[("hint", h)],
                    ));
                }
                s
            }
            FitNote::AiReviewFailed(e) => trf(
                lang,
                " · AI review unavailable ({err}) — the fit itself already landed",
                &[("err", e)],
            ),
            // R23-6 A-3: loud, because "the reverse-fit did nothing" is what
            // it means and a rationale line was not carrying it.
            FitNote::FitReset => tr(
                lang,
                " · ⚠ THE REVERSE-FIT WAS DISCARDED: every version of it rendered farther from the target than your untouched photo, so the recipe was reset to neutral — this is the same as not having reverse-fitted at all",
            )
            .to_string(),
            // R23 review LOW-3: names the CROP, which at a 2% aspect tolerance
            // is the likeliest cause by far on a reference the user exported
            // themselves — "two different photos" sent them hunting a mix-up
            // they had not made. The verdict is the same either way.
            FitNote::ReferenceNotSameFrame => tr(
                lang,
                " · ⚠ the reference's proportions do not match this photo — it was cropped, or it is not the same frame; either way the two distributions are not comparable",
            )
            .to_string(),
            // R23 review LOW-4: "found nothing to act on" is the answer to
            // ONE of the no-retry paths. A move the app selected and then
            // could not run is a different fact and gets its own sentence —
            // the user has paid for a review either way.
            FitNote::DeepFit { action, outcome } => match outcome {
                DeepFitOutcome::NothingActionable => {
                    tr(lang, " · deep: the review found nothing this app can act on — the plain fit stands")
                        .to_string()
                }
                DeepFitOutcome::ActionDidNotRun => trf(
                    lang,
                    " · deep: the review asked for {action}, which could not be carried out — the plain fit stands",
                    &[("action", action)],
                ),
                DeepFitOutcome::Adopted => trf(
                    lang,
                    " · deep: tried {action} on the review's suggestion and kept it (it re-scored at least as high)",
                    &[("action", action)],
                ),
                DeepFitOutcome::Discarded => trf(
                    lang,
                    " · deep: tried {action} on the review's suggestion and discarded it (it re-scored lower)",
                    &[("action", action)],
                ),
            },
        }
    }

    /// `Msg::Fitted` landing — facts arrive typed and are rendered HERE
    /// with the landing-time language (L12#4); the rationale's typed notes
    /// install after `push_variant`'s reload cleared them (L12#2B).
    fn on_fitted(&mut self, ctx: &egui::Context, lang: Lang, boxed: Box<anyhow::Result<FitOutcome>>) {
                match *boxed {
                    // Either way the worker may have persisted a recipe.json
                    // (an Err can land after that write) — recompute badges.
                    Ok(out) => {
                        self.edited_badge.clear();
                        // Advance the ● baseline ONLY when the store actually
                        // holds the fit. A backup-gate refusal means nothing
                        // was written: leaving the baseline alone keeps
                        // ● unsaved lit and lets nav_stash protect the fit —
                        // the ordinary unsaved-edit path takes over.
                        if out.persisted {
                            self.saved_recipe = out.recipe.clone();
                            self.nav_stash.remove(
                                &self.src_path.clone().unwrap_or_default(),
                            );
                            // The worker cleared pixels.json alongside the
                            // recipe (a fit is source-based) — keep the
                            // per-frame ● pixel comparison's disk mirror in
                            // step with it.
                            self.pixels_on_disk = None;
                        }
                        self.refresh_versions();
                        let mut note = trf(
                            lang,
                            "Reverse-fit done: look residual {before}→{after} · created a「Reverse-fit」variant (editable / XMP / full-res)",
                            &[
                                ("before", &format!("{:.3}", out.err_before)),
                                ("after", &format!("{:.3}", out.err_after)),
                            ],
                        );
                        for n in &out.status {
                            note.push_str(&Self::render_fit_note(lang, n));
                        }
                        // The generated look, solved back into an editable recipe,
                        // becomes a NEW「反推」variant: base = the source neutral
                        // (same negative as Original), look carried by the recipe —
                        // so it is fully editable, exports XMP and renders at full
                        // resolution. Auto-switch to it.
                        self.push_variant(
                            Variant {
                                kind: VariantKind::Fitted,
                                id: new_variant_id(),
                                name: None,
                                recipe: out.recipe,
                                base: None,
                                origin: None,
                                thumb: None,
                            },
                            ctx,
                        );
                        // AFTER the variant load (which cleared them): the
                        // fit rationale's typed copy, for draw-time zh.
                        self.rationale_notes = out.rationale_notes;
                        self.done(note);
                    }
                    Err(e) => {
                        self.edited_badge.clear();
                        // The pre-fit snapshot may exist even when the fit
                        // errored — keep the Versions list truthful.
                        self.refresh_versions();
                        self.fail(tr(lang, "Reverse-fit failed"), e);
                    }
                }
    }

    /// `Msg::Pasted` landing — body extracted verbatim from the
    /// poll_workers pump (round-12 decomposition; indentation kept).
    fn on_pasted(&mut self, lang: Lang, res: anyhow::Result<PasteOutcome>) {
                    // Sidecars were written (possibly partially on error) —
                    // recompute the gallery badges either way.
                    self.edited_badge.clear();
                    match res {
                        // A partial failure keeps the error channel (as the
                        // worker-side bail! did) — it must never read as a
                        // clean success, and the ● baseline must not advance.
                        Ok(out) if out.errs.is_empty() => {
                            // The open photo's paste is on disk: advance the
                            // ● baseline so the marker doesn't cry wolf. Only
                            // on FULL success — a partial failure could be
                            // exactly the open photo, which must keep warning.
                            // Guard on the photo still being open, and compare
                            // nothing: edits made while the worker ran leave
                            // recipe != saved_recipe, correctly re-lighting ●.
                            if let Some((p, r)) = self.pasted_open.take()
                                && self.src_path.as_ref() == Some(&p)
                            {
                                self.saved_recipe = r;
                                self.nav_stash.remove(&p);
                            }
                            let mut s = trf(
                                lang,
                                "Recipe pasted to {ok} photos ({xmp} XMP) → develop store",
                                &[("ok", &out.ok.to_string()), ("xmp", &out.xmp.to_string())],
                            );
                            if !out.xmp_fails.is_empty() {
                                let d = brief_list(&out.xmp_fails);
                                s.push_str(&trf(
                                    lang,
                                    " — ⚠ {n} XMP projection(s) failed (those pastes ARE saved): {detail}",
                                    &[("n", &out.xmp_fails.len().to_string()), ("detail", &d)],
                                ));
                            }
                            if !out.xmp_notes.is_empty() {
                                let d = brief_list(&out.xmp_notes);
                                s.push_str(&trf(
                                    lang,
                                    " — {n} sidecar(s) regenerated rather than merged (Lightroom-only properties dropped): {detail}",
                                    &[("n", &out.xmp_notes.len().to_string()), ("detail", &d)],
                                ));
                            }
                            self.done(s);
                        }
                        Ok(out) => {
                            self.pasted_open = None;
                            self.fail(
                                tr(lang, "batch paste"),
                                anyhow::anyhow!(
                                    "{}",
                                    trf(
                                        lang,
                                        "{ok} succeeded, {fail} failed: {detail}",
                                        &[
                                            ("ok", &out.ok.to_string()),
                                            ("fail", &out.errs.len().to_string()),
                                            ("detail", &out.errs.join(" · ")),
                                        ],
                                    )
                                ),
                            );
                        }
                        Err(e) => {
                            self.pasted_open = None;
                            self.fail(tr(lang, "batch paste"), e);
                        }
                    }
    }

    /// `Msg::Models` landing. Keyed by ROLE: the two roles have their own
    /// endpoint and key, so a result only ever fills the catalogue of the
    /// role that asked for it — and by GENERATION: a completion is installed
    /// only if nothing invalidated the catalogue while it flew.
    pub(crate) fn on_models(&mut self, lang: Lang, role: ModelRole, generation: u64, res: anyhow::Result<Vec<String>>) {
        // Single-flight per role (`fetch_models` refuses to start a second
        // while one flies), so whatever lands, THE flight for this role is
        // over — stale or not, the flag must clear or the fetch button and
        // the auto-probe stay disarmed for the rest of the session.
        self.catalogue_mut(role).fetching = false;
        // …but the ids belong to the world the fetch was LAUNCHED in. A key
        // replaced or a URL swapped mid-flight bumped `gen` (via `clear`);
        // installing this completion would resurrect the ids the OLD
        // credential could see and offer them under the new one's name.
        if generation != self.catalogue_mut(role).generation {
            self.settings.status =
                tr(lang, "model list discarded — settings changed while it was being fetched")
                    .into();
            return;
        }
        match res {
            Ok(ids) => {
                let chat: Vec<String> = ids
                    .iter()
                    .filter(|s| autoshop::openai_models::is_chat_model(s))
                    .cloned()
                    .collect();
                let imgs: Vec<String> = ids
                    .iter()
                    .filter(|s| autoshop::openai_models::is_image_model(s))
                    .cloned()
                    .collect();
                self.settings.status = trf(
                    lang,
                    "fetched {n} models ({chat} chat · {img} image)",
                    &[
                        ("n", &ids.len().to_string()),
                        ("chat", &chat.len().to_string()),
                        ("img", &imgs.len().to_string()),
                    ],
                );
                let cat = self.catalogue_mut(role);
                cat.chat = chat;
                // Only the image role generates images; keeping an image list
                // on the analysis catalogue would let a picker offer ids from
                // the wrong endpoint.
                cat.image_gen = if role == ModelRole::Image { imgs } else { Vec::new() };
            }
            Err(e) => {
                self.settings.status =
                    trf(lang, "fetch failed: {err}", &[("err", &e.to_string())]);
            }
        }
    }

}
