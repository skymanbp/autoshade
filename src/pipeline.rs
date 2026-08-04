//! Shared pipeline core used by both the CLI (`main.rs`) and the web UI
//! (`serve.rs`): run the advise chain for one RAW and write its outputs to the
//! right place. Keeping this in one module means the CLI and the server can
//! never drift apart.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::advisor::{
    Advisor, ClaudeProvider, Decision, HeuristicProposer, OpenAiProvider, OpenAiVerifier, Preview,
    Verdict,
};
use crate::config::Config;
use crate::decode;
use crate::recipe::EditRecipe;
use crate::xmp;

/// Run the full advise chain for one RAW: decode → propose (GPT or heuristic
/// fallback) → Claude verify → optional one revision round. `verbose` prints the
/// proposer/verifier lines (CLI uses true, the server uses false).
/// Run the advise chain for one RAW. `guidance` is an optional user direction
/// (a prompt steering the edit, e.g. "warmer and moodier") woven into the GPT
/// prompt.
pub fn produce_recipe(
    raw: &Path,
    cfg: &Config,
    verbose: bool,
    guidance: Option<&str>,
    base: Option<&EditRecipe>,
    style_strength: f32,
) -> Result<(EditRecipe, Verdict)> {
    // decode_any: a camera RAW, or an already-baked PNG/TIFF/JPEG (PNG-source mode).
    let decoded = decode::decode_any(raw)?;

    // Refine mode: when `base` (the user's CURRENT edit) is given, fold it into
    // the direction so GPT adjusts that edit rather than proposing from scratch.
    // Absent a base, behaviour is unchanged — a fresh proposal from the original.
    let refine_owned: Option<String> = base.map(|b| {
        let base_json = serde_json::to_string(b).unwrap_or_default();
        format!(
            "REFINE the photographer's CURRENT edit instead of starting over — keep its choices and \
             change only what this direction implies. CURRENT EDIT (EditRecipe JSON): {base_json}. \
             Direction: {}",
            guidance.unwrap_or("make a small, tasteful improvement")
        )
    });
    let guidance = refine_owned.as_deref().or(guidance);

    let preview_img = decoded.preview_resized(1568);
    let mut jpeg = Vec::new();
    preview_img
        .write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
        .context("encode preview JPEG for advisor")?;
    let preview = Preview { jpeg };
    // The full-resolution preview buffer is DEAD from here on — only meta and
    // histogram feed the advise chain, which can stall on the network for
    // minutes. Keeping `decoded` whole pinned hundreds of MB of 61MP pixels
    // for that entire window.
    let decode::Decoded { meta, histogram, .. } = decoded;

    // Style influence: retrieve the user's edits on the most SIMILAR past shots
    // (needs `autoshop style-index`). style_strength == 0 disables it entirely;
    // otherwise we inject a soft text reference AND, at higher strength, gently
    // pull the FINAL recipe toward those historical means (the blend below).
    // Central store first; the legacy cwd-relative file keeps an index built
    // before the store existed working unchanged.
    let style = (style_strength > 0.0)
        .then(|| {
            crate::style::StyleIndex::load(&crate::store::style_index_path())
                .or_else(|_| {
                    crate::style::StyleIndex::load(std::path::Path::new("out/style-index.json"))
                })
                .ok()
        })
        .flatten()
        .map(|ix| {
            let ex = ix.retrieve(&meta, &histogram, 4, stem(raw));
            (ix.render_reference(&ex), crate::style::style_targets(&ex))
        });
    let reference: Option<String> = style.as_ref().and_then(|(r, _)| r.clone());
    let ref_str = reference.as_deref();
    if verbose && ref_str.is_some() {
        println!("style    : reference from similar past edits (strength {:.0}%)", style_strength * 100.0);
    }
    if verbose
        && let Some(g) = guidance {
            println!("direction: {g}");
        }

    let (meta, hist) = (&meta, &histogram);

    // GPT vision when a key is set; on failure (quota/network) warn and fall back
    // to the heuristic so we still produce a recipe (disclosure, not masking).
    let openai = OpenAiProvider::new(cfg);
    let (mut recipe, can_revise) = if cfg.openai_api_key.is_some() {
        if verbose {
            println!("proposer : OpenAI ({})", cfg.openai_model);
        }
        match openai.propose(&preview, meta, hist, ref_str, guidance, None) {
            Ok(r) => (r, true),
            Err(e) => {
                eprintln!("⚠ GPT proposer failed ({e})\n  → falling back to the heuristic baseline.");
                // Hand the REAL cause to the heuristic: this stderr line is
                // invisible in the windowed GUI, so the recipe's rationale is
                // the only place the user can learn why the AI didn't run.
                let heuristic = HeuristicProposer { fallback_reason: Some(e.to_string()) };
                (heuristic.propose(&preview, meta, hist, None, None, None)?, false)
            }
        }
    } else {
        if verbose {
            println!("proposer : heuristic baseline (set OPENAI_API_KEY to use GPT vision)");
        }
        let heuristic = HeuristicProposer::default();
        (heuristic.propose(&preview, meta, hist, None, None, None)?, false)
    };

    // Verifier (analysis role): OAuth `claude` CLI by default, or an
    // OpenAI-compatible API when the analysis provider is set to `api`.
    let verifier: Box<dyn Advisor> = if cfg.analysis_is_api() {
        Box::new(OpenAiVerifier::new(cfg))
    } else {
        Box::new(ClaudeProvider::new(cfg))
    };
    if verbose {
        let who = if cfg.analysis_is_api() { "OpenAI-API" } else { "Claude (OAuth)" };
        println!("verifier : {who} ({})", cfg.analysis_model);
    }
    let mut verdict = verifier.verify(&recipe, meta, hist)?;

    // Bounded verify→revise loop (only if GPT actually produced the recipe). With
    // the now-symmetric verifier — which pushes a too-flat edit to commit AND a
    // too-cooked one to ease — a few rounds converge toward a finished look instead
    // of just ratcheting down. Capped at MAX_REVISIONS to bound cost/latency; we
    // stop early on Accept or when the verifier stops giving a revision hint.
    const MAX_REVISIONS: usize = 2;
    let mut round = 0;
    while can_revise && round < MAX_REVISIONS && verdict.decision != Decision::Accept {
        let Some(hint) = verdict.revised_hint.clone() else { break };
        round += 1;
        if verbose {
            println!("verdict {:?} → revision {round}/{MAX_REVISIONS} (hint: {hint})", verdict.decision);
        }
        // Disclosure, not masking (same policy as the propose fallback above): a
        // transient provider failure in a LATER round must not throw away the
        // already-paid-for, already-VERIFIED recipe from the previous round.
        // Keep that (recipe, verdict) pair — they stay consistent, so the
        // returned verdict still describes the returned recipe — and record why
        // the loop stopped in the rationale, the one channel all three surfaces
        // show (the windowed GUI has no console for the CLI's stderr). A
        // FIRST-round failure still errors: there is no good pair to keep.
        let revised = match openai.propose(&preview, meta, hist, ref_str, guidance, Some(&hint)) {
            Ok(r) => r,
            Err(e) => {
                recipe.rationale.push_str(&format!(
                    " [revision round {round} failed ({e}) — keeping the previous verified proposal]"
                ));
                break;
            }
        };
        match verifier.verify(&revised, meta, hist) {
            Ok(v) => {
                recipe = revised;
                verdict = v;
            }
            Err(e) => {
                recipe.rationale.push_str(&format!(
                    " [verification of revision round {round} failed ({e}) — keeping the previous \
                     verified proposal]"
                ));
                break;
            }
        }
    }

    // Distill toward the user's historical style: a gentle, capped pull of the
    // global sliders toward similar past edits. Capped at 60% so even max
    // strength never fully overrides the AI's scene-specific proposal.
    if let Some((_, targets)) = &style {
        let pre_blend = recipe.clone();
        crate::style::blend_toward(&mut recipe, targets, style_strength.clamp(0.0, 1.0) * 0.6);
        recipe.clamp();
        // The blend mutates the recipe AFTER the rationale was written and
        // AFTER the verdict above. When it actually changed something (a pull
        // toward targets the recipe already matches is a no-op — claiming a
        // pull then would be its own honesty bug), disclose it in the
        // rationale — the derivation numbers must not silently contradict the
        // final sliders (a real verifier flagged exactly that on a heuristic
        // fallback whose "exposure +0.0EV" text sat next to blended
        // -0.06EV/-20/+4 values) — then re-verify the FINAL recipe so the
        // returned verdict honestly reflects what will actually be applied.
        if recipe != pre_blend {
            recipe.rationale.push_str(&format!(
                " [style distillation then pulled the global sliders toward this user's past \
                 edits (effective strength {:.0}%) — final values can differ from the \
                 derivation above]",
                style_strength.clamp(0.0, 1.0) * 0.6 * 100.0
            ));
            verdict = verifier.verify(&recipe, meta, hist)?;
        }
    }
    // Base look, stamped in ONE place for every surface: the proposal and the
    // verification above both ran over the camera's embedded preview — the
    // very base the curve approximates — so the AI's JSON round-trip never
    // decides it. A saved recipe.json owns its curve verbatim (a legacy save
    // must keep rendering as it was tuned); otherwise a fresh estimate.
    // Without this, a CLI-written analyze recipe carried an empty curve and
    // the open-time "recipe.json keeps its saved curve" rule then pinned the
    // dark pre-base-look rendering onto that photo forever.
    // ONE read of the saved recipe for BOTH calibration fields: two separate
    // reads could straddle a concurrent writer's retire/publish window (GUI
    // and web are separate processes) and stamp a curve from the saved recipe
    // beside a lens profile from the fresh fallback.
    match saved_recipe_snapshot(raw) {
        Some(saved) => {
            recipe.base_curve = saved.base_curve;
            recipe.lens_profile = saved.lens_profile;
        }
        None => {
            recipe.base_curve = photo_base_knots(raw);
            recipe.lens_profile = fresh_lens_profile(raw);
        }
    }
    Ok((recipe, verdict))
}

/// The photo's saved recipe (central store first, then legacy), parsed once —
/// the calibration-stamping snapshot both fields must come from.
fn saved_recipe_snapshot(raw: &Path) -> Option<EditRecipe> {
    for p in [crate::store::recipe_target(raw), crate::store::legacy_recipe(raw)] {
        if let Ok(text) = std::fs::read_to_string(&p)
            && let Ok(r) = serde_json::from_str::<EditRecipe>(&text)
        {
            return Some(r);
        }
    }
    None
}

/// The base_curve a programmatic writer must carry for `raw`: an existing
/// saved recipe.json owns it verbatim — including the legacy EMPTY curve,
/// which must keep rendering exactly as it was tuned — otherwise the photo's
/// fresh camera-matched estimate ([`photo_base_knots`]).
pub fn photo_base_curve(raw: &Path) -> Vec<[f32; 2]> {
    match saved_recipe_snapshot(raw) {
        Some(r) => r.base_curve,
        None => photo_base_knots(raw),
    }
}

/// BOTH calibration halves from ONE saved-recipe snapshot: two independent
/// [`photo_base_curve`] + [`photo_lens_profile`] reads could pair an OLD
/// curve with a NEW profile when a concurrent publish lands between them
/// (the same single-snapshot rule `produce_recipe` follows).
pub fn photo_calibration(raw: &Path) -> (Vec<[f32; 2]>, crate::recipe::LensProfile) {
    match saved_recipe_snapshot(raw) {
        Some(r) => (r.base_curve, r.lens_profile),
        None => (photo_base_knots(raw), fresh_lens_profile(raw)),
    }
}

/// The lens profile a programmatic writer must carry for `raw` — the same
/// saved-first rule as [`photo_base_curve`]: an existing recipe.json owns its
/// profile verbatim (including the legacy default-off, and any user toggle),
/// otherwise the photo's own in-camera metadata with every available
/// component enabled ([`fresh_lens_profile`]).
pub fn photo_lens_profile(raw: &Path) -> crate::recipe::LensProfile {
    match saved_recipe_snapshot(raw) {
        Some(r) => r.lens_profile,
        None => fresh_lens_profile(raw),
    }
}

/// Fresh in-camera lens profile for `raw`, stamped "all available components
/// on" (the user's chosen default: match the in-camera JPEG, which applies
/// these same corrections). Cheap — one TIFF metadata parse, no decode.
pub fn fresh_lens_profile(raw: &Path) -> crate::recipe::LensProfile {
    let mut p = crate::lensmeta::read(raw);
    p.vignette_on = !p.vignette.is_empty();
    p.distortion_on = !p.distortion.is_empty();
    p.ca_on = !p.ca_r.is_empty() && !p.ca_b.is_empty();
    p
}

/// Fresh camera-matched base-look estimate for `raw`: a neutral develop
/// CDF-matched against the embedded preview (`render::camera_base_knots`).
/// Costs a demosaic for a RAW — callers that already hold a neutral render
/// (the GUI open worker) call the estimator directly instead. Best-effort by
/// design: the base look is an enhancement, so a develop/decode failure here
/// yields "no base look" rather than failing the caller's real operation
/// (whose own render will surface the same error loudly).
pub fn photo_base_knots(raw: &Path) -> Vec<[f32; 2]> {
    if !decode::is_raw(raw) {
        return Vec::new();
    }
    let camera = match decode::embedded_preview(raw) {
        Ok(Some(c)) => c,
        Ok(None) => return Vec::new(),
        Err(e) => {
            eprintln!("⚠ base look skipped: embedded preview of {} failed ({e})", raw.display());
            return Vec::new();
        }
    };
    // The estimate is CDF statistics — a ≤2048 working develop carries the
    // same histogram signal as 61 MP at a fraction of the transients.
    match crate::render::render_to_image(raw, &EditRecipe::default(), None, Some(2048)) {
        Ok(neutral) => {
            // Estimate on the profile-vignette-corrected neutral — the same
            // base a stamped canvas starts from (see render::estimation_base).
            let est = crate::render::estimation_base(&neutral, &fresh_lens_profile(raw));
            crate::render::camera_base_knots(&est, &camera)
        }
        Err(e) => {
            // Disclosed, not silent: the caller's own render will surface the
            // same failure loudly, but the resulting darker-than-canvas output
            // needs a traceable cause in the log.
            eprintln!("⚠ base look skipped: neutral develop of {} failed ({e})", raw.display());
            Vec::new()
        }
    }
}

pub fn write_recipe(raw: &Path, recipe: &EditRecipe, out: Option<PathBuf>) -> Result<PathBuf> {
    let out = out.unwrap_or_else(|| crate::store::recipe_target(raw));
    // `-o some_dir` names an existing DIRECTORY: refuse instead of renaming
    // the whole directory to .bak below and publishing a recipe FILE in its
    // place (the user meant "write into it", which this API does not do).
    if out.is_dir() {
        anyhow::bail!(
            "recipe target {} is a directory — pass a file path (e.g. {}/recipe.json)",
            out.display(),
            out.display()
        );
    }
    ensure_parent(&out)?;
    // Rasters living beside the recipe are stored by bare file name so the
    // develop dir stays relocatable (store::resolve_mask_paths re-anchors them
    // at load). Serialize a relativized COPY — the caller's in-memory recipe
    // keeps its absolute paths for rendering.
    let mut on_disk = recipe.clone();
    if let Some(parent) = out.parent() {
        crate::store::relativize_mask_paths(&mut on_disk, parent);
    }
    // Publish via tmp+rename rather than truncating the AUTHORITATIVE file in
    // place: a crash mid-write used to leave a half-written recipe.json (loud
    // Unreadable, but the develop was gone). The old file is retired to .bak
    // first for CRASH RECOVERY (fs::rename does replace an existing file on
    // Windows — verified empirically) — worst case is a briefly missing file
    // with the intact .tmp beside it, never corrupt JSON.
    // Per-process AND per-call tmp name from the ONE shared store counter
    // (see store::next_tmp_seq): a GUI and a web server saving the same
    // photo used to share one fixed .tmp, the web server threads REQUESTS,
    // and a same-process migration publish mints into the SAME
    // `.json.tmp.<pid>.<seq>` namespace — every site must share one counter.
    // (.bak stays shared — the retire/restore pair below is
    // last-writer-wins by design; one photo has one interactive writer in
    // practice.)
    let tmp = out.with_extension(format!(
        "json.tmp.{}.{}",
        std::process::id(),
        crate::store::next_tmp_seq()
    ));
    std::fs::write(&tmp, serde_json::to_string_pretty(&on_disk)?)
        .with_context(|| format!("write recipe {}", tmp.display()))?;
    // Retire the old file to .bak instead of deleting it: if the publish
    // rename then fails (AV lock, racing writer), the authoritative recipe
    // is RESTORED, not lost. On success the .bak is dropped. A STALE .bak
    // (crash in that window) is cleared first — belt-and-braces so the
    // retire can never trip over an undeletable leftover and wedge every
    // later save (retire fails, had_old reads false, the publish then fails
    // on the still-present target).
    let bak = out.with_extension("json.bak");
    if out.exists() {
        // Clearing a stale .bak is safe ONLY while the live target exists —
        // after a crashed publish whose restore ALSO failed, the .bak is the
        // sole surviving copy of the previous develop, and the unconditional
        // pre-clear deleted it forever.
        let _ = std::fs::remove_file(&bak);
    } else if bak.exists() {
        // Orphaned survivor: put it back as the live file first — the retire
        // below then re-bak's it through the normal recovery chain.
        let _ = std::fs::rename(&bak, &out);
    }
    let had_old = std::fs::rename(&out, &bak).is_ok();
    if let Err(e) = std::fs::rename(&tmp, &out) {
        // A failed restore must be SAID, not swallowed — the authoritative
        // file is then missing and the save survives only at the .bak path.
        let restore_note = if had_old && std::fs::rename(&bak, &out).is_err() {
            format!(" (restoring the previous file ALSO failed — it survives at {})", bak.display())
        } else {
            String::new()
        };
        let _ = std::fs::remove_file(&tmp);
        return Err(e)
            .with_context(|| format!("publish recipe {}{restore_note}", out.display()));
    }
    if had_old {
        let _ = std::fs::remove_file(&bak);
    }
    if out == crate::store::recipe_target(raw) {
        crate::store::note_source(raw); // breadcrumb for the hashed store dir
    }
    Ok(out)
}

/// First FREE ./out artifact path for `tag` (`tag`, `tag-2`, … `tag-999`),
/// CLAIMED atomically (`create_new`): pixels.json links these paths, GUI undo
/// history holds them, and a CANCELLED worker may still be running toward the
/// name it probed — an existence probe alone would hand the same name to the
/// replacement task and let the two write over each other. The claimed
/// placeholder is simply overwritten by the worker's save. `None` past the
/// 999 cap (refuse, never alias). Shared by the GUI's five starters and the
/// web fill/heal handlers — a fixed web output name used to overwrite a
/// master an earlier develop still referenced.
pub fn unique_out(path: &Path, tag: &str) -> Option<PathBuf> {
    // n = 0 → "tag"; n = 1..=998 → "tag-2".."tag-999" (never tag-1000).
    for n in 0..=998u32 {
        let t = if n == 0 { tag.to_string() } else { format!("{tag}-{}", n + 1) };
        let cand = default_out(path, &t, "png");
        if ensure_parent(&cand).is_err() {
            return None;
        }
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&cand) {
            Ok(_) => return Some(cand),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

pub fn write_xmp(raw: &Path, recipe: &EditRecipe) -> Result<PathBuf> {
    write_xmp_at(xmp_target(raw), recipe)
}

/// Write the XMP to an EXPLICIT path. Used when the recipe was redirected with
/// `-o`: the two halves of one develop must stay in the same folder, or the
/// GUI/web would keep restoring an older `out/<stem>.xmp` instead.
pub fn write_xmp_at(out: PathBuf, recipe: &EditRecipe) -> Result<PathBuf> {
    ensure_parent(&out)?;
    // Stage + rename, never truncate in place: `fs::write` opens the LIVE
    // sidecar with O_TRUNC, so a full disk, an interruption or a competing
    // writer left a truncated file where a valid Lightroom sidecar used to
    // be — the previous projection destroyed by the failed attempt to
    // replace it. (fs::rename replaces the destination on every platform.)
    let tmp = out.with_extension(format!(
        "xmp.tmp.{}.{}",
        std::process::id(),
        crate::store::next_tmp_seq()
    ));
    std::fs::write(&tmp, xmp::recipe_to_xmp(recipe))
        .with_context(|| format!("write xmp {}", tmp.display()))?;
    if let Err(e) = std::fs::rename(&tmp, &out) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("publish xmp {}", out.display()));
    }
    Ok(out)
}

/// Where the .xmp for `raw` goes — the photo's central develop dir (see
/// `store`; the photo library itself stays read-only). Kept here because every
/// surface already imports it from pipeline.
pub fn xmp_target(raw: &Path) -> PathBuf {
    crate::store::xmp_target(raw)
}

/// Guarantee the read-only library: refuse to write `out` if it lands inside the
/// source RAW's own folder (or below it). Outputs belong in ./out (exports) or
/// the central develop store (sidecars).
///
/// The PROJECT's ./out and the per-user store root are always writable, even
/// when the source itself lives there (e.g. `match` fitting a look onto a
/// previously exported preview) — the rule protects the photo LIBRARY, not our
/// own output areas. A folder that merely happens to be NAMED "out" inside the
/// library is still refused.
/// Fold `.`/`..` LEXICALLY (no filesystem access — the target may not exist
/// yet). Shared by [`guard_readonly`] and the CLI's canonical-path equality
/// check (`-o` spelled with `dir/../` segments must still classify as the
/// canonical file). See the RootDir note inside for the drive-relative trap.
pub fn normalize_lexical(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut n = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => match n.components().next_back() {
                Some(Component::Normal(_)) => {
                    n.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {
                    // The root's parent IS the root: "D:/../lib" folds to
                    // "D:/lib" exactly as the filesystem resolves it.
                    // Popping the root would yield drive-relative "D:lib",
                    // which dodges starts_with checks.
                }
                _ => n.push(".."),
            },
            Component::CurDir => {}
            other => n.push(other.as_os_str()),
        }
    }
    n
}

/// Canonicalize the deepest EXISTING ancestor, then re-attach the
/// not-yet-created tail: junction/subst/symlink aliases and case-flipped
/// spellings resolve to the true on-disk form even when the LEAF is absent.
/// Shared by [`guard_readonly`] and the CLI's canonical-path equality check.
pub fn resolve_existing_pub(p: &Path) -> PathBuf {
    let mut cur = p;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if let Ok(mut c) = std::fs::canonicalize(cur) {
            for t in tail.iter().rev() {
                c.push(t);
            }
            return c;
        }
        match (cur.parent(), cur.file_name()) {
            (Some(par), Some(name)) if !par.as_os_str().is_empty() => {
                tail.push(name);
                cur = par;
            }
            _ => return p.to_path_buf(),
        }
    }
}

pub fn guard_readonly(out: &Path, raw: &Path) -> Result<()> {
    use std::path::absolute;
    // Fold `.`/`..` LEXICALLY (no filesystem access — the target may not
    // exist yet): `std::path::absolute` keeps `..` segments, so a path like
    // "out/../<library>/x.tif" used to START WITH ./out for the allow-check
    // while the filesystem resolved it INTO the protected library.
    let normalize = normalize_lexical; // extracted for the CLI's same_path
    // Canonicalize the deepest EXISTING ancestor, then re-attach the
    // not-yet-created tail: a junction / subst / symlink alias or a
    // case-flipped spelling ("d:\photography" for "D:\Photography" — NTFS is
    // case-insensitive, starts_with is not) otherwise denotes the library
    // while failing every lexical comparison below. Canonical paths carry the
    // TRUE on-disk casing and resolved links, and all sides go through the
    // same lens, so the \\?\ verbatim prefix cancels out.
    let resolve_existing = resolve_existing_pub; // extracted for same_path
    let (Ok(out_abs), Ok(raw_abs)) = (absolute(out), absolute(raw)) else {
        return Ok(());
    };
    let (out_abs, raw_abs) =
        (resolve_existing(&normalize(&out_abs)), resolve_existing(&normalize(&raw_abs)));
    if out_abs.starts_with(resolve_existing(&crate::store::store_root())) {
        return Ok(());
    }
    if let Ok(own_out) = absolute(Path::new("out"))
        && out_abs.starts_with(resolve_existing(&normalize(&own_out))) {
            return Ok(());
        }
    if let Some(raw_dir) = raw_abs.parent()
        && out_abs.starts_with(raw_dir) {
            anyhow::bail!(
                "refusing to write into the source RAW's folder ({}) — the photo library is \
                 read-only. Write outputs to ./out (the default) instead.",
                raw_dir.display()
            );
        }
    Ok(())
}

/// `./out/<stem>.<kind>.<ext>` — outputs never go beside the source RAW.
pub fn default_out(raw: &Path, kind: &str, ext: &str) -> PathBuf {
    PathBuf::from("out").join(format!("{}.{kind}.{ext}", stem(raw)))
}

pub fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create output dir {}", parent.display()))?;
        }
    Ok(())
}

pub fn stem(p: &Path) -> &str {
    p.file_stem().and_then(|s| s.to_str()).unwrap_or("out")
}

/// Whether a directory entry is a directory, WITHOUT an extra stat per file:
/// `DirEntry::file_type()` comes free with the directory listing (measurable on
/// large libraries / network shares), and only symlinks fall back to the
/// following `Path::is_dir` so junction/symlink traversal behaves as before.
fn entry_is_dir(entry: &std::fs::DirEntry) -> std::io::Result<bool> {
    let ft = entry.file_type()?;
    Ok(if ft.is_symlink() { entry.path().is_dir() } else { ft.is_dir() })
}

/// Recursively collect every camera RAW under `dir`, sorted.
///
/// "A RAW" has exactly ONE definition app-wide — [`decode::is_raw`] (arw, dng,
/// raw, raf, nef, cr2, cr3, orf, rw2), the same predicate [`find_sources`] and
/// the render/decode path use. This scanner used to accept `.arw` alone, which
/// left `batch`, `eval` and `style-index` blind to libraries the GUI and web
/// open fine.
pub fn find_raws(dir: &Path) -> Result<Vec<PathBuf>> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) -> std::io::Result<()> {
        // Same directory-symlink-cycle cap as `find_sources`: entry_is_dir
        // deliberately follows dir links, so an uncapped walk recurses until
        // the OS errors or the stack dies.
        if depth > 64 {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let p = entry.path();
            if entry_is_dir(&entry)? {
                walk(&p, out, depth + 1)?;
            } else if decode::is_raw(&p) {
                out.push(p);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, &mut out, 0).with_context(|| format!("scan {}", dir.display()))?;
    out.sort();
    Ok(out)
}

/// Like [`find_raws`] but also includes already-baked images (PNG/TIFF/JPEG), so
/// the web UI can browse and edit LR/PS-denoised exports alongside RAWs. Sorted.
pub fn find_sources(dir: &Path) -> Result<Vec<PathBuf>> {
    fn is_source(p: &Path) -> bool {
        crate::decode::is_raw(p)
            || p.extension().and_then(|x| x.to_str()).is_some_and(|x| {
                matches!(x.to_ascii_lowercase().as_str(), "png" | "tif" | "tiff" | "jpg" | "jpeg")
            })
    }
    fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) -> std::io::Result<()> {
        // A directory-symlink cycle would recurse forever (entry_is_dir
        // deliberately follows dir symlinks); 64 levels is beyond any real
        // photo library, so the cap only ever trips on a cycle.
        if depth > 64 {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let p = entry.path();
            if entry_is_dir(&entry)? {
                walk(&p, out, depth + 1)?;
            } else if is_source(&p) {
                out.push(p);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, &mut out, 0).with_context(|| format!("scan {}", dir.display()))?;
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_refuses_writes_into_the_source_library() {
        // A RAW living in the (read-only) photo library.
        let raw = Path::new("D:/Photography/Raw/2024/Trip/DSC0001.ARW");
        // Writing a sibling INTO that folder must be refused.
        let sibling = Path::new("D:/Photography/Raw/2024/Trip/DSC0001.developed.tif");
        assert!(guard_readonly(sibling, raw).is_err(), "must refuse a sibling write");
        // A subfolder under the RAW's folder is refused too.
        let under = Path::new("D:/Photography/Raw/2024/Trip/out/DSC0001.tif");
        assert!(guard_readonly(under, raw).is_err(), "must refuse a subfolder write");
        // The default ./out (outside the library) is allowed.
        let safe = default_out(raw, "developed", "tif");
        assert!(guard_readonly(&safe, raw).is_ok(), "./out must be allowed");
        // A source that itself lives in OUR ./out (e.g. `match` on an exported
        // preview) may be written beside — the guard protects the library only.
        let out_src = Path::new("out/DSC0001.preview.jpg");
        let out_dst = Path::new("out/DSC0001.matched.json");
        assert!(guard_readonly(out_dst, out_src).is_ok(), "our ./out is always writable");
        // A `..` bumping against the ROOT must fold like the filesystem does
        // ("D:/../Photography" = "D:/Photography") — popping the root used to
        // produce the drive-relative "D:Photography", which dodged every
        // starts_with check and slipped a library write past the guard.
        let root_dodge = Path::new("D:/../Photography/Raw/2024/Trip/DSC0001.x.tif");
        assert!(
            guard_readonly(root_dodge, raw).is_err(),
            "a root-level .. must not bypass the library guard"
        );
    }

    /// find_raws must accept EVERY format decode::is_raw does (one definition of
    /// "a RAW" app-wide) and nothing else — a Canon/Nikon library used to scan
    /// as empty for batch/eval/style-index.
    #[test]
    fn find_raws_accepts_every_raw_format_the_app_can_decode() {
        let dir = std::env::temp_dir().join(format!("autoshop_find_raws_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).expect("temp dir");
        let raws = ["a.ARW", "b.dng", "c.NEF", "d.cr2", "e.cr3", "f.orf", "g.rw2", "h.raw"];
        for name in raws {
            std::fs::write(dir.join(name), b"").expect("write");
        }
        std::fs::write(dir.join("sub").join("i.raf"), b"").expect("write"); // recursion
        for name in ["note.txt", "baked.png", "export.jpg", "DSC0001.xmp"] {
            std::fs::write(dir.join(name), b"").expect("write");
        }

        let found = find_raws(&dir).expect("scan");
        let mut names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_lowercase())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["a.arw", "b.dng", "c.nef", "d.cr2", "e.cr3", "f.orf", "g.rw2", "h.raw", "i.raf"],
            "find_raws must see every RAW format (case-insensitive, recursive) and no baked/sidecar files"
        );
        assert!(found.iter().all(|p| decode::is_raw(p)), "one RAW predicate, app-wide");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn outputs_always_default_outside_the_library() {
        let raw = Path::new("D:/Photography/Raw/2024/Trip/DSC0001.ARW");
        // Exports (deliverable images) stay in ./out; develop STATE (recipe +
        // XMP sidecars) lives in the photo's central develop dir, keyed by the
        // absolute path. Neither ever lands beside the RAW — the library stays
        // read-only by construction.
        assert!(default_out(raw, "developed", "tif").starts_with("out"));
        assert_eq!(xmp_target(raw), crate::store::develop_dir(raw).join("DSC0001.xmp"));
        assert!(guard_readonly(&xmp_target(raw), raw).is_ok(), "the store is always writable");
        // Same stem in a DIFFERENT folder → a different develop dir (the
        // cross-clobber the store exists to prevent).
        let other = Path::new("D:/Photography/Raw/2025/DSC0001.ARW");
        assert_ne!(xmp_target(raw), xmp_target(other));
    }
}
