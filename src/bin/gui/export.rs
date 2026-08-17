//! Export & save: XMP, rendered files, batch, paste projection.

use super::*;

/// The batch renderer's develop resolution — the OPEN path's precedence
/// (`persist::read_saved_develop_locked`), mirrored onto a
/// [`autoshop::store::DevelopSnapshot`] so the two surfaces answer alike
/// (L13#1: the batch read recipe.json only — an LR-only or XMP-only photo
/// exported neutral, and an older recipe out-ranked a newer Lightroom
/// edit). Order: Lightroom's own sidecar when newest → recipe.json (a
/// NEUTRAL recipe falls through, the open rule) → the store's XMP
/// projection. XMP-restored recipes get the same fresh-calibration stamp
/// the open path applies (`persist::stamp_calibration` shape: knots when
/// non-empty, lens, as-shot pinned only if None). The RAW's embedded packet
/// answers LAST, exactly as on open (`store::embedded_packet_for_restore`
/// carries the clear gate; the snapshot read it under the develop lock).
/// `Ok(None)` = no saved develop (the caller's neutral + base-look fallback).
pub(crate) fn resolve_snapshot_develop(
    p: &std::path::Path,
    snap: &autoshop::store::DevelopSnapshot,
    warns: &mut Vec<String>,
) -> anyhow::Result<Option<(EditRecipe, &'static str)>> {
    // `warns` (L16-2): the disclosures the OPEN path surfaces for the same
    // develop — silent-neutral XMP values, out-of-range clamp drops — used
    // to vanish entirely on this twin: a batch export silently applied a
    // degraded develop the interactive open would have warned about. The
    // caller prefixes each entry with the photo and ships them on the batch
    // outcome. (Full unification with persist::read_saved_develop_locked
    // stays registered on the structure track — the WARNING parity is what
    // this closes.)
    if let Some((text, kind)) = snap.lr_xmp.as_ref()
        && let Some(hit) = xmp_arm(p, text, kind, warns)
    {
        return Ok(Some(hit));
    }
    if let Some((text, from)) = &snap.recipe {
        let mut r = serde_json::from_str::<EditRecipe>(text)?;
        if r.is_noop() {
            // Neutral recipe.json restores NOTHING on the open path — an
            // XMP with real edits may still exist beside it, else the photo
            // renders the fresh baseline (not the neutral recipe's stale
            // calibration).
            if let Some((t, k)) = snap.store_xmp.as_ref()
                && let Some(hit) = xmp_arm(p, t, k, warns)
            {
                return Ok(Some(hit));
            }
            if let Some(t) = snap.packet_xmp.as_ref()
                && let Some(hit) = xmp_arm(p, t, "XMP (embedded in the RAW)", warns)
            {
                return Ok(Some(hit));
            }
            return Ok(None);
        }
        // The one restore path that never went through clamp: a stored
        // recipe with extreme-but-finite geometry rendered NaN weights into
        // a published export. render_to_file now clamps too — this keeps
        // the batch recipe equal to what OPENING the photo would show.
        clamp_disclosed(&mut r, warns);
        // Rasters re-anchor to whichever dir the recipe was read from
        // (central store first, else a legacy ./out sidecar).
        if let Some(base) = from.parent() {
            autoshop::store::resolve_mask_paths(&mut r, base);
        }
        return Ok(Some((r, "recipe.json")));
    }
    if let Some(err) = &snap.recipe_err {
        // An EXISTING save that cannot be read is not absence — exporting
        // neutral over it would silently shed the user's edits.
        anyhow::bail!("{err}");
    }
    if let Some((t, k)) = snap.store_xmp.as_ref()
        && let Some(hit) = xmp_arm(p, t, k, warns)
    {
        return Ok(Some(hit));
    }
    if let Some(t) = snap.packet_xmp.as_ref()
        && let Some(hit) = xmp_arm(p, t, "XMP (embedded in the RAW)", warns)
    {
        return Ok(Some(hit));
    }
    Ok(None)
}

/// One XMP source arm of [`resolve_snapshot_develop`]: import, disclose what
/// the import could not read (the open path's A6 rule), stamp the fresh
/// calibration. `None` = a no-op import, fall through.
fn xmp_arm(
    p: &std::path::Path,
    text: &str,
    kind: &'static str,
    warns: &mut Vec<String>,
) -> Option<(EditRecipe, &'static str)> {
    let mut r = autoshop::xmp::xmp_to_recipe(text);
    // Collected for EVERY consulted file — a no-op import included (the
    // persist.rs rule, Codex 32-#1 + review R12-11): a sidecar whose ONLY
    // edit is corrupt restores nothing, and the next save overwrites it in
    // silence unless the corruption is named here.
    let bad = autoshop::xmp::unparsable_crs_numbers(text);
    if !bad.is_empty() {
        warns.push(format!(
            "{} numeric XMP setting(s) unreadable ({}) — restored as neutral",
            bad.len(),
            bad.join(", ")
        ));
    }
    // …and the open path's second disclosure too (review R12-11): foreign
    // corrections the import cannot model are retained-but-not-applied.
    let dropped_masks = autoshop::xmp::unsupported_corrections(text);
    if dropped_masks > 0 {
        warns.push(format!(
            "{dropped_masks} unsupported Lightroom correction(s) not applied"
        ));
    }
    if r.is_noop() {
        return None;
    }
    clamp_disclosed(&mut r, warns);
    let knots = autoshop::pipeline::photo_base_knots(p);
    if !knots.is_empty() {
        r.base_curve = knots;
    }
    r.lens_profile = autoshop::pipeline::fresh_lens_profile(p);
    if r.as_shot_k.is_none() {
        let (ask, ast) = autoshop::pipeline::fresh_as_shot_wb(p);
        r.as_shot_k = ask;
        r.as_shot_tint = ast;
    }
    Some((r, kind))
}

/// Clamp with the open path's disclosure — a batch drop was silent (L16-2).
fn clamp_disclosed(r: &mut EditRecipe, warns: &mut Vec<String>) {
    let dropped = r.clamp();
    if !dropped.is_empty() {
        warns.push(format!("out-of-range values discarded ({})", dropped.describe()));
    }
}

impl AutoshopApp {
    /// One-line echo of the current delivery settings for the Export /
    /// Download hover — e.g. "JPEG · 2560 px · q95 · sRGB (universal)" — so
    /// the state stays glanceable now that the settings live in the Export
    /// section instead of a toolbar row.
    pub(crate) fn export_summary(&self, lang: Lang) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.push(tr(lang, self.exp_format.label()).to_string());
        parts.push(if self.exp_long_edge == 0 {
            tr(lang, "Original size").to_string()
        } else {
            format!("{} px", self.exp_long_edge)
        });
        if self.exp_format == ExportFormat::Jpeg {
            parts.push(format!("q{:.0}", self.exp_quality));
        }
        if self.exp_sharpen > 0.0 {
            parts.push(format!("{} {:.0}", tr(lang, "Output sharpening"), self.exp_sharpen));
        }
        parts.push(tr(lang, EXPORT_SPACES[(self.exp_space as usize).min(2)]).to_string());
        if self.save_denoise {
            parts.push(tr(lang, "AI Denoise").to_string());
        }
        parts.join(" · ")
    }

    /// `./out/<stem>.developed.{tif|jpg}` — the default export target. The stem
    /// follows the ACTIVE variant's pixel source, so a Generated variant exports
    /// under its reimagine stem (and its AI pixels), not the original's.
    pub(crate) fn default_out(&self) -> PathBuf {
        let src = self.active_source_path();
        let stem = src
            .as_deref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("out")
            .to_string();
        let ext = self.exp_format.ext();
        PathBuf::from("out").join(format!("{stem}.developed.{ext}"))
    }

    /// Render the full-resolution develop to `out` on a worker thread (16-bit
    /// TIFF, or 8-bit JPEG when the path ends in .jpg). Renders the ACTIVE
    /// variant's pixel source (a Generated variant → its full-res reimagine PNG,
    /// developed by the recipe), so what exports matches what's on screen.
    pub(crate) fn start_render_to(&mut self, out: PathBuf) {
        let Some(path) = self.active_source_path() else { return };
        if self.busy {
            return;
        }
        let lang = self.lang;
        // Same library-read-only gate every CLI export runs: without it,
        // Download…'s save dialog was the one door that could drop a
        // developed.tif beside the source RAW. Guard BOTH anchors: `path` is
        // the render source, which for a Generated variant is the ./out PNG —
        // guarding only that would leave the ORIGINAL photo's library folder
        // unprotected exactly when a generated variant is active.
        let mut anchors = vec![path.clone()];
        if let Some(orig) = self.src_path.clone()
            && orig != path
        {
            anchors.push(orig);
        }
        for a in &anchors {
            if let Err(e) = autoshop::pipeline::guard_readonly(&out, a) {
                let t = e.to_string();
                self.status = t.clone();
                self.toast(ToastKind::Error, t);
                return;
            }
        }
        self.busy = true;
        self.status = if self.save_denoise {
            trf(
                lang,
                "rendering + AI denoise → {path} … (GPU sidecar, can take minutes)",
                &[("path", &out.display().to_string())],
            )
        } else {
            trf(lang, "rendering full-resolution → {path} …", &[("path", &out.display().to_string())])
        };
        let recipe = self.recipe.clone();
        let denoise = self.save_denoise;
        let export = self.export_opts();
        let src_photo = self.src_path.clone();
        self.spawn_worker(
            move || {
                let res = (|| {
                    if let Some(p) = out.parent() {
                        std::fs::create_dir_all(p)?;
                    }
                    // Every deliverable repairs (the batch worker's rule):
                    // the canvas normally arrives repaired at open, but when
                    // that repair was an INABILITY (a then-locked file) the
                    // canvas holds the washed curve — and this export shipped
                    // it while a batch export of the SAME canvas repaired it.
                    // Anchored on the PHOTO, like every sibling site: `path`
                    // is the RENDER SOURCE, which for an in-place heal/clone
                    // is the baked master .png — never a RAW — so anchoring
                    // there left the repair permanently dead for exactly the
                    // canvases whose curve still renders (a generated canvas
                    // is stripped and no-ops either way). Off the UI thread,
                    // memo-bounded.
                    let mut recipe = recipe;
                    let relook = src_photo
                        .as_deref()
                        .and_then(|p| {
                            autoshop::pipeline::repair_pre_era_base_curve(p, &mut recipe)
                        })
                        .is_some();
                    // SCUNet AI denoise (python sidecar) runs before the develop when on.
                    let opts = denoise.then(|| {
                        autoshop::denoise::DenoiseOpts::from_config(&autoshop::config::Config::load(), None, 1.0)
                    });
                    autoshop::render::render_to_file(&path, &recipe, &out, opts.as_ref(), Some(&export))?;
                    // FACTS (L12#4): the landing renders the relook note in
                    // the landing-time language.
                    Ok::<ExportOutcome, anyhow::Error>(ExportOutcome::Single {
                        out,
                        relooked: relook,
                    })
                })();
                Msg::Exported(res)
            },
            |e| Msg::Exported(Err(e)),
        );
    }

    /// The delivery options the export UI currently dials in (gap batch F) —
    /// shared by single export, Download… and batch render.
    pub(crate) fn export_opts(&self) -> autoshop::render::ExportOpts {
        autoshop::render::ExportOpts {
            long_edge: (self.exp_long_edge > 0).then_some(self.exp_long_edge),
            sharpen: self.exp_sharpen.clamp(0.0, 100.0),
            jpeg_quality: self.exp_quality.round().clamp(1.0, 100.0) as u8,
            eight_bit: self.exp_format.eight_bit(),
            color_space: match self.exp_space {
                1 => autoshop::render::ExportColorSpace::DisplayP3,
                2 => autoshop::render::ExportColorSpace::AdobeRgb,
                _ => autoshop::render::ExportColorSpace::Srgb,
            },
        }
    }

    /// Batch-render every Ctrl+click-selected photo through its own saved
    /// recipe.json (central store, else legacy ./out; neutral develop when
    /// none exists) with the current export options — "export selected".
    /// Sequential on one worker: each full-res develop is already multi-second
    /// and memory-heavy (61 MP frames), so parallelism would thrash, not speed
    /// up. AI denoise is deliberately excluded (minutes per photo via the GPU
    /// sidecar — run it per-photo from the export panel instead).
    pub(crate) fn start_batch_render(&mut self) {
        if self.busy {
            return;
        }
        let targets: Vec<PathBuf> = {
            let mut idx: Vec<usize> = self.multi_sel.iter().copied().collect();
            idx.sort_unstable(); // report in gallery order, not hash order
            idx.into_iter().filter_map(|i| self.gallery.get(i).cloned()).collect()
        };
        if targets.is_empty() {
            return;
        }
        let ext = self.exp_format.ext().to_string();
        let export = self.export_opts();
        let lang = self.lang; // pre-spawn UI statuses only; results land as FACTS (L12#4)
        self.busy = true;
        self.status = trf(
            lang,
            "Batch-rendering {n} photos → ./out …",
            &[("n", &targets.len().to_string())],
        );
        self.batch_progress = Some((0, targets.len())); // the top-bar progress bar
        // Interim BatchProgress ticks flow through this extra clone; the
        // TERMINAL Msg::Exported is owned by spawn_worker (panic-safe).
        let tx = self.tx.clone();
        let ext = ext.to_string();
        // In-memory state outranks disk WHOLESALE — recipes AND pixel
        // identities, for every photo that has any: the nav stash holds work
        // navigated away from, and the open photo's live canvas outranks even
        // its own stash entry. Without this, batching a photo you edited (or
        // are looking at) renders the stale (or absent) recipe.json / stale
        // saved pixels, visibly diverging from the screen.
        // path → (recipe, Some((master, generated)) | None = source pixels).
        type BatchOverride = (EditRecipe, Option<(PathBuf, bool)>);
        let mut overrides: std::collections::HashMap<PathBuf, BatchOverride> = self
            .nav_stash
            .iter()
            .map(|(p, st)| {
                (
                    p.clone(),
                    (
                        st.recipe.clone(),
                        st.origin.clone().map(|o| (o, st.kind == VariantKind::Generated)),
                    ),
                )
            })
            .collect();
        if let Some(p) = self.src_path.clone() {
            let pix = self
                .active_variant()
                .and_then(|v| v.origin.clone())
                .map(|o| (o, self.active_is_generated()));
            overrides.insert(p, (self.recipe.clone(), pix));
        }
        self.spawn_worker(
            move || {
                // A plain block, not an IIFE: since the summary became typed
                // facts (L12#4) nothing early-returns at this level — the
                // per-photo closure below owns the `?` scope.
                let res = {
                    let total = targets.len();
                    let (mut okn, mut errs) = (0usize, Vec::<String>::new());
                    // A selection can hold two same-stem photos (different
                    // folders). Batch-scope claims keep their deliverables
                    // apart; the summary disloses which photo took which
                    // name. Single exports stay stem-keyed (re-export
                    // replaces in place) — the dedup is batch-level by
                    // decision.
                    let mut names = autoshop::pipeline::BatchNames::default();
                    // Disclosure counter, like `names.renamed`: this worker
                    // was the one reader that repaired with nobody watching.
                    let mut relooked = 0usize;
                    // Per-photo develop disclosures (L16-2) — the open
                    // path's warnings, shipped on the batch outcome.
                    let mut warns: Vec<String> = Vec::new();
                    for p in &targets {
                        let mut photo_warns: Vec<String> = Vec::new();
                        let one = (|| -> anyhow::Result<()> {
                            // ONE locked snapshot per photo when DISK decides
                            // (L01): recipe text, pixel source and the .bak
                            // recovery that must PRECEDE recipe selection all
                            // come from a single develop-lock acquisition —
                            // four independent unlocked reads let a
                            // mid-compound save interleave (recipe.json is
                            // retired to .bak for its whole staged publish,
                            // so exists() read "no develop" mid-save) and
                            // shipped a neutral render of an edited photo, or
                            // an old recipe over new baked pixels. The render
                            // below runs OUTSIDE the lock, on the snapshot —
                            // the CLI's documented contract. A develop whose
                            // pixels are a baked retouch master renders FROM
                            // that master (the recipe composes on top, the
                            // same InPlace contract the canvas uses); the
                            // override's pixel identity wins over disk — an
                            // unsaved retouch/denoise must export exactly
                            // like its canvas.
                            let (recipe, pix) = if let Some((lr, pix)) = overrides.get(p) {
                                (lr.clone(), pix.clone())
                            } else {
                                let snap = autoshop::store::read_develop_snapshot(p)?;
                                if let Some(why) = &snap.lr_unreadable {
                                    // Unreadable is not absent (L08): stderr
                                    // for the log AND the batch outcome for
                                    // the user (L16-2 — the windowed build
                                    // has no console).
                                    eprintln!(
                                        "⚠ {}: a Lightroom sidecar sits beside this photo but could not be read ({why}) — the stored develop decides",
                                        autoshop::pipeline::stem(p)
                                    );
                                    photo_warns
                                        .push(format!("Lightroom sidecar unreadable ({why})"));
                                }
                                if let Some(why) = &snap.packet_unreadable {
                                    // Same rule for the packet INSIDE the RAW.
                                    eprintln!(
                                        "⚠ {}: this RAW carries an embedded XMP develop that could not be read ({why}) — it is NOT reflected",
                                        autoshop::pipeline::stem(p)
                                    );
                                    photo_warns.push(format!("embedded XMP unreadable ({why})"));
                                }
                                let recipe = match resolve_snapshot_develop(p, &snap, &mut photo_warns)? {
                                    Some((r, _kind)) => r,
                                    // No saved develop → export what the canvas
                                    // WOULD show: neutral + the photo's camera-
                                    // matched base look (one extra develop per
                                    // photo; without it the batch export of an
                                    // unedited RAW comes out on the dark base
                                    // while its open canvas shows the bright
                                    // one).
                                    None => EditRecipe {
                                        base_curve: autoshop::pipeline::photo_base_knots(p),
                                        lens_profile: autoshop::pipeline::fresh_lens_profile(p),
                                        ..Default::default()
                                    },
                                };
                                // A recorded-but-unhonourable master:
                                // exporting would silently drop the retouch —
                                // fail THIS photo with the cause instead (the
                                // summary lists it). Checked BEFORE the name
                                // claim now, so a refused photo no longer
                                // consumes a same-stem "(2)" slot.
                                if snap.pixel_source.is_none() && snap.pixel_recorded {
                                    anyhow::bail!(
                                        "the saved retouch master could not be loaded — the export would silently drop the retouch (open the photo for the cause, then re-save or clear it)"
                                    );
                                }
                                (recipe, snap.pixel_source)
                            };
                            let out = names.claim(p, "developed", &ext);
                            autoshop::pipeline::ensure_parent(&out)?;
                            let mut recipe = recipe;
                            if pix.as_ref().is_some_and(|(_, generated)| *generated) {
                                // A GENERATED master's look (camera curve AND
                                // lens corrections) already lives in its
                                // pixels — the stamped disk recipe keeps them
                                // for the RAW's own sake, but rendering the
                                // master through them would cook both twice
                                // (the same strip rule the canvas applies).
                                recipe.base_curve = Vec::new();
                                recipe.lens_profile = Default::default();
                                // The anchor follows the strip rule: baked
                                // pixels carry their WB (relative model).
                                recipe.as_shot_k = None;
                                recipe.as_shot_tint = None;
                            }
                            // Repair AFTER the strip, BEFORE the render — the
                            // load_version ordering, for the same reason: a
                            // generated master's curve is deleted either way,
                            // so repairing first paid a decode for an estimate
                            // nothing used, and the summary then claimed a
                            // correction that never reached a pixel. Counted
                            // only when the export LANDS, so a failure summary
                            // cannot claim corrections it did not ship.
                            // (render_to_file does not go through
                            // store::render_source_checked — batch 60 put the
                            // repair there and this path never saw it.)
                            let repaired = autoshop::pipeline::repair_pre_era_base_curve(
                                p, &mut recipe,
                            )
                            .is_some();
                            let src = pix.map(|(m, _)| m).unwrap_or_else(|| p.clone());
                            autoshop::render::render_to_file(&src, &recipe, &out, None, Some(&export))?;
                            if repaired {
                                relooked += 1;
                            }
                            Ok(())
                        })();
                        match one {
                            Ok(()) => okn += 1,
                            Err(e) => errs.push(format!("{}: {e}", autoshop::pipeline::stem(p))),
                        }
                        if !photo_warns.is_empty() {
                            warns.push(format!(
                                "{}: {}",
                                autoshop::pipeline::stem(p),
                                photo_warns.join("; ")
                            ));
                        }
                        let _ = tx.send(Msg::BatchProgress { done: okn + errs.len(), total });
                    }
                    // FACTS (L12#4): the same-stem renames, the relook count
                    // and the per-photo errors all render at the landing,
                    // with the language live THERE — a batch runs minutes
                    // and the language picker stays reachable throughout.
                    Ok(ExportOutcome::Batch {
                        ok: okn,
                        errs,
                        renamed: names.renamed,
                        relooked,
                        warns,
                    })
                };
                Msg::Exported(res)
            },
            |e| Msg::Exported(Err(e)),
        );
    }

    pub(crate) fn start_export(&mut self) {
        let out = self.default_out();
        self.start_render_to(out);
    }

    /// Save this photo's develop to the central store: recipe.json for every
    /// source type + the Lightroom / Camera-Raw XMP projection for RAW.
    /// An XMP reproduces a look via develop PARAMETERS; a Generated variant's
    /// look lives in its pixels, not the recipe, so there's nothing faithful to
    /// write — steer the user to 反推 (which produces a Fitted variant whose XMP
    /// IS the look). Always keyed to the original RAW `src_path`.
    pub(crate) fn save_xmp(&mut self) {
        // Flush a typed-but-uncommitted mask rename FIRST (U10): every save
        // entry point (Ctrl+S, the Save develop button) must write the name
        // the user sees in the box, not the pre-focus snapshot.
        self.commit_mask_name_buf();
        let lang = self.lang;
        if self.active_is_generated() {
            // A keyboard Ctrl+S refusal must be SEEN — the status line alone
            // scrolls away under the very next message.
            let t = tr(
                lang,
                "A generated variant's look lives in its pixels — there's no parametric recipe to export; run 「Reverse-fit」 first to get an exportable XMP",
            );
            self.status = t.into();
            self.toast(ToastKind::Error, t);
            return;
        }
        let Some(path) = self.src_path.clone() else { return };
        // Reset-then-save means "clear my edits": writing a neutral pair would
        // only pin a misleading ● badge with no in-app way to remove it —
        // delete the working sidecars instead (version snapshots are kept).
        // BOTH homes are cleared: the central store AND any legacy ./out file
        // a pre-store build left behind — a surviving legacy sidecar would
        // resurrect the "cleared" edits through the read fallbacks.
        // A baked pixel retouch IS an edit even under a neutral recipe — a
        // heal on an untouched photo must take the SAVE path (recipe.json +
        // pixels.json), not report "nothing to save" while deleting the very
        // linkage the retouch needs to survive a reopen.
        // A NON-TRIVIAL STRIP is an edit too: clear_develop removes
        // variants.json with everything else, so a neutral canvas sitting in
        // a multi-variant strip must take the save path (noop recipe + strip
        // record) — "clear my edits" must not silently destroy the OTHER
        // cards.
        if self.recipe.is_noop()
            && self.active_variant().is_none_or(|v| v.origin.is_none())
            && self.current_strip_record().is_none()
        {
            // NoWait wrapper: clear_develop locks internally, but with Wait —
            // on this UI thread a develop held by another process must fail
            // into the error arm below, not freeze the window.
            match autoshop::store::with_develop_lock(
                &path,
                autoshop::store::DevelopLockMode::NoWait,
                || autoshop::store::clear_develop(&path),
            ) {
                Ok(o) => {
                    self.edited_badge.clear();
                    // The CANVAS is the baseline, not default(): Reset leaves
                    // the stamped lens profile (and knots) on the canvas, and
                    // a default baseline compared dirty on lens_profile
                    // forever — a permanent ● with quit prompts (R4-8).
                    self.saved_recipe = self.recipe.clone();
                    self.pixels_on_disk = None;
                    self.saved_strip = None;
                    self.nav_stash.remove(&path);
                    self.forget_open_base();
                    match o.marker_warning {
                        // NEVER a plain success: the store copies are gone, but
                        // without the marker a projection copied beside the RAW
                        // out-ranks this clear and restores the edits on reopen.
                        Some(w) => {
                            let t = trf(
                                lang,
                                "saved edits cleared, but the clear could not be marked ({err}) — a sidecar beside the RAW may restore them",
                                &[("err", &w)],
                            );
                            self.status = t.clone();
                            self.toast(ToastKind::Error, t);
                        }
                        None => {
                            self.status = if o.removed {
                                tr(lang, "neutral recipe — saved edits cleared (saved files removed)")
                                    .into()
                            } else {
                                tr(lang, "neutral recipe — nothing to save").into()
                            };
                        }
                    }
                }
                Err(e) => {
                    self.persist_postponed(&e, "could not clear the saved edits: {err}", &[]);
                }
            }
            return;
        }
        // recipe.json is the source of truth for EVERY source type (the badge,
        // batch render and reopening all read it; the XMP alone is lossy — no
        // bitmap masks / recolour gains). The XMP is the RAW-only Lightroom
        // projection on top. The recipe write ALONE decides the saved state:
        // once it lands, reopening restores it regardless of the XMP — so the
        // ● baseline must follow it even when the XMP half fails.
        let raw = autoshop::decode::is_raw(&path);
        // The WRITER's rule (api_xmp, produce_recipe, match, Save-all): a
        // washed pre-era curve must not be re-persisted verbatim. The canvas
        // normally arrives repaired at open, but an open-time INABILITY (a
        // then-locked file) leaves it washed — and Ctrl+S then froze the
        // defect on disk while a single export from the SAME canvas repaired
        // and disclosed. The canvas itself heals here: what is written is
        // what is shown (a generated canvas was refused above, and its curve
        // is empty by invariant).
        // ...and the HISTORY follows it — the Arc-repoint rule (see the
        // preview-resolution repoint below in this file): calibration is not
        // an edit, so every step holding the exact pre-repair pair is
        // re-stamped in place. Without this, commit_if_settled pushed the
        // washed step onto the undo stack and ONE Ctrl+Z after Ctrl+S handed
        // the washed curve back to the canvas invisibly (dirty_vs neutralises
        // both fields, so no ● would have said so) — while disk and every
        // deliverable stayed repaired.
        let before = (self.recipe.version, self.recipe.base_curve.clone());
        let relooked =
            autoshop::pipeline::repair_pre_era_base_curve(&path, &mut self.recipe).is_some();
        if relooked {
            let (after_ver, after_curve) =
                (self.recipe.version, self.recipe.base_curve.clone());
            let restamp = |r: &mut EditRecipe| {
                if r.version == before.0 && r.base_curve == before.1 {
                    r.version = after_ver;
                    r.base_curve = after_curve.clone();
                }
            };
            restamp(&mut self.committed.recipe);
            self.undo_stack.iter_mut().for_each(|s| restamp(&mut s.recipe));
            self.redo_stack.iter_mut().for_each(|s| restamp(&mut s.recipe));
            // The strip entry that IS the canvas follows it, and the preview
            // re-develops under the healed curve.
            if let Some(v) = self.variants.get_mut(self.active) {
                v.recipe = self.recipe.clone();
            }
            // ...and the SIBLINGS follow too (the stash-retry rule): the
            // estimate is already paid, so healing the rest of the strip is
            // a memo hit apiece — a save must not leave the photo split
            // into a bright canvas over washed sibling RECIPES. A healed
            // sibling also drops its CARD: the thumb is a stale render of
            // the washed recipe, and keeping it showed a stops-dark
            // thumbnail beside a bright canvas until the next rebuild.
            let active = self.active;
            for (i, v) in self.variants.iter_mut().enumerate() {
                if i != active
                    && v.kind != VariantKind::Generated
                    && autoshop::pipeline::repair_pre_era_base_curve(&path, &mut v.recipe)
                        .is_some()
                {
                    v.thumb = None;
                }
            }
            self.dirty = true;
        }
        // ONE lock across the whole compound save: recipe.json, the
        // pixels.json link, the strip record and the XMP projection publish
        // as a unit, so another process cannot interleave between the halves.
        // NoWait — a busy develop postpones the save, ● stays lit, the canvas
        // loses nothing, and Ctrl+S retries.
        let locked_save = autoshop::store::with_develop_lock(
            &path,
            autoshop::store::DevelopLockMode::NoWait,
            || -> std::io::Result<()> {
        if self.open_unresolved {
            // The baseline came from an open that could not READ the saved
            // develop — snapshot whatever is on disk before overwriting it
            // (v<N> copy). Refusing on a failed snapshot beats destroying a
            // save nobody ever looked at.
            if let Err(e) = autoshop::store::backup_saved_develop(&path, Some(&self.recipe)) {
                return Err(std::io::Error::other(format!(
                    "the unread develop could not be backed up ({e}) — nothing was overwritten"
                )));
            }
        }
        // ONE single-generation commit (L03): recipe.json, the pixels.json
        // link and the strip record stage together and publish past ONE
        // durable marker — no kill point between them can leave a new recipe
        // over an old (or half-cleared) master link, or a new pair under a
        // stale strip. All-or-nothing replaces the old per-member degrade
        // notes: the save either lands whole (the notes below describe
        // SEMANTICS — what a reopen restores — not write outcomes) or fails
        // whole, with every unsaved protection still armed.
        let origin = self.active_variant().and_then(|v| v.origin.clone());
        let strip_rec = self.current_strip_record();
        let generated = self.active_is_generated();
        let committed: anyhow::Result<()> = (|| {
            let recipe_bytes = autoshop::pipeline::recipe_store_bytes(&path, &self.recipe)?;
            let pixels = match &origin {
                // An in-place heal/clone/fill bakes pixels into the variant's
                // origin raster; parametric recipe/XMP cannot carry them, so
                // the store records the master's path and reopening restores
                // the retouched canvas.
                Some(o) => autoshop::store::CommitMember::Write(
                    autoshop::store::pixel_source_record_bytes(&path, o, generated)?,
                ),
                // A parametric-only save CLEARS any stale record — a
                // surviving one would resurrect an obsolete retouched canvas
                // on the next open.
                None => autoshop::store::CommitMember::Clear,
            };
            let variants = match &strip_rec {
                Some(rec) => autoshop::store::CommitMember::Write(
                    autoshop::store::variants_record_bytes(&path, rec)?,
                ),
                None => autoshop::store::CommitMember::Clear,
            };
            autoshop::store::commit_develop(
                &path,
                autoshop::store::DevelopCommit { recipe: Some(recipe_bytes), pixels, variants },
            )?;
            Ok(())
        })();
        match committed {
            Ok(()) => {
                self.open_unresolved = false;
                let rp = autoshop::store::recipe_target(&path);
                let pixel_note: Option<String> = origin.is_some().then(|| {
                    tr(
                        lang,
                        " · retouched pixels: master linked — reopening restores them (the Lightroom XMP stays parametric-only)",
                    )
                    .to_string()
                });
                // The strip mirror advances WITH the commit (all-or-nothing:
                // a failed commit leaves it untouched and the
                // background-variant unsaved protection armed).
                self.saved_strip = strip_rec;
                self.edited_badge.clear(); // the open photo just gained its badge
                self.saved_recipe = self.recipe.clone();
                self.nav_stash.remove(&path);
                self.pixels_on_disk = origin;
                let mut s = if raw {
                    match autoshop::pipeline::write_xmp(&path, &self.recipe) {
                        Ok((p, merge_note, losses)) => {
                            // A sidecar we could not MERGE was regenerated, and
                            // that drops the user's Lightroom-only properties.
                            // Saying only "saved" is how that loss stayed
                            // invisible until they reopened the catalog.
                            if let Some(m) = &merge_note {
                                self.toast(ToastKind::Error, m.clone());
                            }
                            // M6a: the projection's own lossy edges — assembled
                            // from what the WRITER just skipped/degraded, so the
                            // counts describe this very file. Empty ⇒ silent.
                            let mask_note = mask_loss_line(lang, &losses);
                            if let Some(m) = &mask_note {
                                self.toast(ToastKind::Error, m.clone());
                            }
                            let base = trf(
                                lang,
                                "XMP + recipe saved → {path}",
                                &[("path", &p.display().to_string())],
                            );
                            let mut s = match merge_note {
                                Some(m) => format!("{base} — ⚠ {m}"),
                                None => base,
                            };
                            if let Some(m) = mask_note {
                                s.push_str(&format!(" — ⚠ {m}"));
                            }
                            s
                        }
                        Err(e) => {
                            let t = trf(
                                lang,
                                "recipe saved — but the Lightroom XMP failed: {err}",
                                &[("err", &e.to_string())],
                            );
                            self.toast(ToastKind::Error, t.clone());
                            t
                        }
                    }
                } else {
                    trf(
                        lang,
                        "recipe saved → {path} (XMP applies to RAW only)",
                        &[("path", &rp.display().to_string())],
                    )
                };
                if let Some(n) = pixel_note {
                    s.push_str(&n);
                }
                if relooked {
                    s.push_str(&format!(
                        " · {}",
                        tr(
                            lang,
                            "camera base look re-estimated — this photo was saved by a version whose preview sampler ran bright, so its stored base look rendered too dark",
                        )
                    ));
                }
                self.forget_open_base();
                self.status = s;
            }
            Err(e) => {
                let mut t = trf(lang, "save failed: {err}", &[("err", &e.to_string())]);
                if relooked {
                    // The disclosure travels with the MUTATION, not the
                    // success: the canvas healed above even though the write
                    // failed, and a silently brighter photo under an
                    // unrelated error reads as a glitch.
                    t.push_str(&format!(
                        " · {}",
                        tr(
                            lang,
                            "camera base look re-estimated — this photo was saved by a version whose preview sampler ran bright, so its stored base look rendered too dark",
                        )
                    ));
                }
                self.status = t.clone();
                self.toast(ToastKind::Error, t); // a failed save must be seen
            }
        }
                Ok(())
            },
        );
        if let Err(e) = locked_save {
            self.persist_postponed(
                &e,
                "save postponed: this photo is being changed by another Autoshop process ({err}); your canvas remains unsaved — retry",
                &[],
            );
        }
    }

    /// Paste the copied recipe onto every Ctrl+click-selected photo on a worker
    /// thread — Lightroom's "sync settings", without rendering anything: a
    /// recipe.json per photo (central store) plus an XMP sidecar for RAWs.
    /// Geometry (crop/straighten) is stripped unless `paste_geometry` is on,
    /// because composition rarely transfers between frames. Library files are
    /// never touched (write_recipe / write_xmp land in the store, never beside
    /// the photo).
    pub(crate) fn start_paste(&mut self) {
        let Some(src) = self.copied.clone() else { return };
        if self.busy {
            return;
        }
        let targets: Vec<PathBuf> = {
            let mut idx: Vec<usize> = self.multi_sel.iter().copied().collect();
            idx.sort_unstable(); // report in gallery order, not hash order
            idx.into_iter().filter_map(|i| self.gallery.get(i).cloned()).collect()
        };
        if targets.is_empty() {
            return;
        }
        let mut recipe = src;
        if !self.paste_geometry {
            recipe.crop = None;
            recipe.straighten_deg = 0.0;
        }
        // Bitmap masks reference per-photo rasters keyed to the SOURCE stem —
        // pasted onto ANOTHER photo they point at the wrong file (and classic
        // XMP cannot carry them, so recipe.json and .xmp would disagree).
        // Strip them for foreign targets, visibly — but the photo the
        // clipboard CAME FROM keeps its own masks (pasting back onto itself
        // used to silently destroy valid AI selections).
        let recipe_full = recipe.clone();
        let copied_from = self.copied_from.clone();
        let n_bitmap = recipe
            .masks
            .iter()
            .filter(|m| matches!(m.mask, autoshop::recipe::MaskGeometry::Bitmap { .. }))
            .count();
        let has_foreign_target = targets.iter().any(|t| Some(t) != copied_from.as_ref());
        if n_bitmap > 0 {
            recipe
                .masks
                .retain(|m| !matches!(m.mask, autoshop::recipe::MaskGeometry::Bitmap { .. }));
            if has_foreign_target {
                self.toast(
                    ToastKind::Error,
                    trf(
                        self.lang,
                        "{n} bitmap mask(s) not pasted — their rasters belong to the source photo (re-run AI select on each target)",
                        &[("n", &n_bitmap.to_string())],
                    ),
                );
            }
        }
        // If the open photo is one of the targets, take the paste live in the
        // editor too (undo-able through the usual committed-snapshot step).
        // Remember the pair so Msg::Pasted can advance the ● baseline: the
        // worker writes this exact recipe to the open photo's store, so
        // leaving `saved_recipe` behind kept a false "● unsaved" lit for
        // edits that were already on disk.
        self.pasted_open = None;
        if let Some(open) = self.src_path.clone()
            && targets.iter().any(|t| t == &open)
        {
            // Same base_curve rule the worker applies to every target below,
            // mirrored here so `pasted_open` equals the bytes the worker
            // writes and the ● baseline can advance on success.
            let chosen = if Some(&open) == copied_from.as_ref() { &recipe_full } else { &recipe };
            let mut live = paste_recipe_for(&open, chosen);
            // The DISK form keeps its calibration (the worker writes it
            // below), but an active GENERATED canvas must not render it —
            // its pixels already carry the look (the load_version /
            // open-restore strip rule). The ● baseline (pasted_open) is
            // normalized the same way: canvas coordinates.
            if self.active_is_generated() {
                live.base_curve = Vec::new();
                live.lens_profile = Default::default();
                // Anchor follows the strip rule (baked WB → relative model).
                live.as_shot_k = None;
                live.as_shot_tint = None;
            }
            self.recipe = live.clone();
            self.dirty = true;
            // Wholesale recipe replacement: disarm index-carrying tools and
            // refresh derived display, like every other whole-swap path.
            self.resync_recipe_display();
            self.pasted_open = Some((open, live));
        }
        let lang = self.lang; // pre-spawn UI statuses only; results land as FACTS (L12#4)
        self.busy = true;
        self.status = trf(
            lang,
            "Pasting recipe to {n} photos…",
            &[("n", &targets.len().to_string())],
        );
        self.spawn_worker(
            move || {
                // A plain block, not an IIFE: the typed outcome (L12#4)
                // removed the early bail — the per-target `step` closure
                // owns the `?` scope.
                let res: anyhow::Result<PasteOutcome> = {
                    let (mut okn, mut xmpn) = (0usize, 0usize);
                    let mut errs: Vec<String> = Vec::new();
                    // XMP-half failures are partial successes (recipe-write-
                    // decides), but their REASON used to reach stderr only —
                    // the status said "n XMP" and left the user to notice the
                    // shortfall by subtraction.
                    let mut xmp_fails: Vec<String> = Vec::new();
                    let mut xmp_notes: Vec<String> = Vec::new();
                    for path in &targets {
                        let mut step = || -> anyhow::Result<bool> {
                            // Worker thread ⇒ Wait: queue behind whichever
                            // process holds this target's develop, exactly
                            // like the CLI. ONE lock across gate → recipe →
                            // XMP so the pasted pair publishes as a unit.
                            autoshop::store::with_develop_lock(
                                path,
                                autoshop::store::DevelopLockMode::Wait,
                                || {
                            // Per-target base-look resolution (paste_recipe_for):
                            // paste copies the user edit, never the source
                            // photo's camera calibration over a saved one. The
                            // clipboard's own photo keeps its bitmap masks.
                            let chosen =
                                if Some(path) == copied_from.as_ref() { &recipe_full } else { &recipe };
                            let r = paste_recipe_for(path, chosen);
                            // The programmatic-overwrite gate, like Analyze /
                            // serve / the CLI: one click overwrites MANY saved
                            // develops the user is not even looking at — a
                            // failed snapshot refuses that target.
                            autoshop::store::backup_saved_develop(path, Some(&r))
                                .map_err(|e| anyhow::anyhow!(
                                    "refusing to overwrite the saved develop: backing it up failed ({e})"
                                ))?;
                            autoshop::pipeline::write_recipe(path, &r, None)?;
                            if autoshop::decode::is_raw(path) {
                                // Recipe-write-decides: an XMP failure after
                                // the recipe landed is a partial success —
                                // reopening restores the paste regardless.
                                // The third member (the M6a mask-loss list) is
                                // deliberately dropped HERE: `xmp_notes` is
                                // rendered under one fixed sentence about
                                // regenerated-not-merged sidecars, so folding
                                // another kind of loss into it would mislabel
                                // both. A foreign target has had its bitmap
                                // masks stripped (and said so) before this
                                // worker started; the remaining per-target
                                // losses reach stderr from write_xmp_doc. A
                                // per-target UI list of them needs its own
                                // PasteOutcome member — R23 work, not a
                                // mislabelled fold into this one.
                                match autoshop::pipeline::write_xmp(path, &r) {
                                    Ok((_, None, _)) => {}
                                    // Regenerated-not-merged loses LR-only
                                    // properties — collected for the paste
                                    // summary (stderr already heard it in
                                    // write_xmp_doc).
                                    Ok((_, Some(m), _)) => xmp_notes.push(format!(
                                        "{}: {m}",
                                        autoshop::pipeline::stem(path)
                                    )),
                                    Err(e) => {
                                        eprintln!(
                                            "⚠ {}: recipe pasted, but the Lightroom XMP failed: {e}",
                                            autoshop::pipeline::stem(path)
                                        );
                                        xmp_fails.push(format!(
                                            "{}: {e}",
                                            autoshop::pipeline::stem(path)
                                        ));
                                        return Ok(false);
                                    }
                                }
                                return Ok(true);
                            }
                            Ok(false)
                                },
                            )
                        };
                        match step() {
                            Ok(wrote_xmp) => {
                                okn += 1;
                                xmpn += usize::from(wrote_xmp);
                            }
                            Err(e) => errs.push(format!("{}: {e}", autoshop::pipeline::stem(path))),
                        }
                    }
                    // FACTS (L12#4): counts and detail lists render at the
                    // landing; a partial failure keeps riding the error
                    // channel there, so it never reads as a clean success.
                    Ok(PasteOutcome { ok: okn, xmp: xmpn, errs, xmp_fails, xmp_notes })
                };
                Msg::Pasted(res)
            },
            |e| Msg::Pasted(Err(e)),
        );
    }
}
