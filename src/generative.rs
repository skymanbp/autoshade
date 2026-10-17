//! Generative image editing (V2_PLAN §5) — a SEPARATE, EXPERIMENTAL concern from
//! the parametric develop pipeline. Calls OpenAI's Images `edits` endpoint
//! (gpt-image-*), which RE-GENERATES pixels.
//!
//! Phase 4 raises the pixel quality of this path ("give GPT higher-level pixels"):
//!   * **Flexible high-res sizing** — models with arbitrary-size support
//!     (gpt-image-2: any WIDTHxHEIGHT, edges ×16, ratio ≤3:1, ≤8 294 400 px) get
//!     the largest aspect-correct size inside `Config::openai_image_max_px`;
//!     models that reject it fall back to the fixed 1024/1536 enum on the API's
//!     400, so no model list is hard-coded. For a RAW whose embedded preview is
//!     smaller than the flexible target, the input is a full-sensor neutral
//!     develop (sharp real detail in, instead of an upscaled ~1.6 MP preview).
//!     The size is planned from the frame that is SENT: a RAW's sensor frame
//!     (`decode::frame_size`), never its embedded preview, which may carry an
//!     in-camera aspect crop (a 4:3 preview over a 3:2 sensor) — and such a
//!     preview is never the input either.
//!   * **Aspect-correct enum fallback** — 1536×1024 / 1024×1536 / 1024×1024 by
//!     orientation instead of squashing every photo into a 1:1 square.
//!   * **Configurable quality tier** (`low|medium|high|auto`, default `high`).
//!   * **`retouch` composites back onto the engine's own develop** — only the
//!     masked (inpainted) region carries generative pixels; the rest keeps the
//!     base pixels, with a feathered seam. For a RAW the base is the neutral
//!     develop (≤2048px, or the full sensor with `full_res`) — never the
//!     camera's baked 8-bit preview, so the master stays on the same tone
//!     chain as the canvas; a baked PNG/TIFF is its own base.
//!
//! `reimagine` = full-frame restyle (no mask) → still a generative re-render at
//! the chosen size, so it stays a low-res experiment / preview, NOT a master.
//! `retouch` = object removal / generative fill (RGBA mask; transparent pixels =
//! the region to regenerate; a blank prompt is the removal instruction,
//! [`REMOVE_PROMPT`]) → preview-resolution composite where only the masked
//! region is generative; the rest is the untouched source preview.
//! `adjust_onto` = prompt-only edit of a caller-developed image, no mask →
//! the whole returned frame, resized to the base dimensions, with measured D.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, RgbImage, Rgba, RgbaImage};

use crate::config::Config;
use crate::{decode, pipeline};

const BOUNDARY: &str = "----autoshadeBoundaryX7MA4YWxkTrZu0gW";

/// The faithfulness scaffold every `fidelity = "high"` reimagine prompt is
/// composed onto. The REQUEST parameter that used to carry this intent
/// (`input_fidelity`) is negotiated AWAY on models that reject it — which
/// gpt-image-2, the flexible-size flagship, does — so on the primary model
/// the prompt is the ONLY faithfulness signal that reaches the wire; the
/// parameter still rides along where it is accepted. `fidelity = "low"`
/// deliberately skips the scaffold: that mode is documented free rein.
const FAITHFUL_SCAFFOLD: &str =
    "Re-develop this exact photograph. Keep the camera angle, framing, geometry \
     and every physical object exactly as they are: do not add, remove, move, or \
     invent any content, structure, or texture. Change only the photographic \
     development - exposure, contrast, white balance, colour grading, tone \
     curves, local light and atmosphere. The result must read as the same \
     photograph, developed differently.";

/// Compose the wire prompt for a whole-frame reimagine. The user's direction
/// used to REPLACE the faithfulness sentence (the GUI's empty-field fallback
/// was the only place it existed), so any typed style wish silently dropped
/// the one preservation instruction — composed here, one mechanism for every
/// caller, the preservation clause is unconditional under "high".
fn hardened_prompt(user: &str, fidelity: &str) -> String {
    if fidelity.eq_ignore_ascii_case("low") {
        return user.to_string();
    }
    let user = user.trim();
    if user.is_empty() {
        FAITHFUL_SCAFFOLD.to_string()
    } else {
        format!("{FAITHFUL_SCAFFOLD}\nStyle direction: {user}")
    }
}

/// The instruction a generative fill sends when the user typed nothing. An
/// empty prompt used to be refused at both front ends ("write what should
/// fill the painted area") while the mask alone already said what the user
/// meant: the painted area is what should GO. Blank = remove, one rule for
/// the CLI, the browser and the GUI (user decision 2026-09-15): the model
/// continues the surroundings into the area and invents nothing new there.
pub const REMOVE_PROMPT: &str =
    "Remove everything inside the masked area and fill it with a seamless \
     continuation of the surrounding scene - the same background, surfaces, \
     lighting, texture, perspective and grain - so the area reads as if \
     nothing had ever been there. Do not add any new object, person, animal, \
     text or watermark. Leave everything outside the mask exactly as it is.";

/// The wire prompt for a fill: the user's own words, or [`REMOVE_PROMPT`]
/// when they wrote none. Trimmed, so a field of spaces is a blank field.
pub fn fill_prompt(user: &str) -> String {
    let user = user.trim();
    if user.is_empty() {
        REMOVE_PROMPT.to_string()
    } else {
        user.to_string()
    }
}

/// What one [`reimagine`] measured about its own result.
#[derive(Debug)]
pub struct ReimagineReport {
    /// Structural divergence between the input actually sent and the KEPT
    /// result — the same statistic (same function, same raster, same neutral
    /// base) the reverse-fit's mode selector will compute if the user asks
    /// for a recipe of this artifact, so `d >= fit::DIVERGENCE_GLOBAL` here
    /// means that fit will run in atmosphere mode.
    pub divergence: crate::fit::Divergence,
    /// `Some(d)` of the DISCARDED attempt when the opt-in retry bought a
    /// second generation (whichever of the two measured farther).
    pub first_divergence: Option<f32>,
}

/// The generation-side fidelity reading: decode both PNGs and measure the
/// reverse-fit's structural-divergence statistic between them. The two frames
/// need not share dimensions — `canonical_generated_png` admits a same-aspect
/// endpoint cap below the requested size (`size_is_an_endpoint_cap`) — and
/// they align because `fit::divergence_raster` resamples BOTH onto its fixed
/// 384×256 grid. What makes that alignment meaningful is the aspect, which
/// that door does refuse to let differ: a different aspect, or a LARGER
/// frame, is a different picture.
fn generation_divergence(sent_png: &[u8], generated_png: &[u8]) -> Result<crate::fit::Divergence> {
    let sent = image::load_from_memory(sent_png)
        .context("decode the sent input for the fidelity reading")?;
    let generated = image::load_from_memory(generated_png)
        .context("decode the generated result for the fidelity reading")?;
    // The analysis raster is a fixed 384x256 with an all-ones mask, so the
    // instrument resolves it; an abstention would mean the statistic changed
    // shape under this caller, and that is an error rather than a fidelity of
    // zero the caller would report as a perfect match.
    crate::fit::structure_divergence_for(
        &sent,
        &generated,
        &crate::recipe::EditRecipe::default(),
        None,
    )
    .context("the fidelity reading found no resolvable structural core")
}

/// Overall HTTP deadline for one BLOCKING `images/edits` POST — the fallback
/// used only when the model/bridge rejects streaming. gpt-image-2 at quality
/// `high` with a near-8 MP flexible size legitimately runs for several
/// minutes; the previous 300 s budget (calibrated for the 1-1.5 MP
/// gpt-image-1 era) fired mid-generation on real requests, and a real
/// quality=high request then outran 600 s too — which is why the PRIMARY path
/// is now streaming under a stall deadline (below) instead of a third
/// recalibration. `AUTOSHADE_HTTP_TIMEOUT_SECS` still overrides
/// (see `advisor::post_with_timeout`).
const IMAGES_EDIT_TIMEOUT_SECS: u64 = 600;



/// Inactivity (stall) deadline for a STREAMING `images/edits` POST — the
/// primary path. The request asks for `partial_images` progress frames, so a
/// healthy server proves liveness at roughly each generation quarter and the
/// longest silent gap in a healthy run is ~total/4: a 600 s stall budget
/// admits ~40-minute generations, while a dead endpoint still fails within
/// the same 600 s the old TOTAL deadline gave (GUI soft-lock protection is
/// unchanged). `AUTOSHADE_HTTP_TIMEOUT_SECS` overrides
/// (see `advisor::post_with_stall_timeout`).
const IMAGES_EDIT_STALL_SECS: u64 = 600;

/// Partial frames requested per stream — the liveness cadence. 3 is the API
/// maximum (`partial_images` must be 0–3, per the published request schema);
/// each partial costs a small extra fee, negligible next to losing a finished
/// multi-minute generation to a timeout.
const IMAGES_EDIT_PARTIALS: u32 = 3;

/// Hard sanity ceiling on one images/edits RESPONSE, whichever shape it takes.
/// Even four full-size base64 frames of an ~8 MP PNG (3 partials + the final)
/// fit in well under 256 MiB. Beyond this the stream is broken or hostile; the
/// cap also bounds the SSE framer's per-line String growth instead of letting
/// one endless line eat all memory. Shared by the streaming and blocking arms
/// so a server that answers plain JSON is not held to a smaller budget than
/// one that streams the identical payload.
const STREAM_CAP: u64 = 512 * 1024 * 1024;

// --- GUI worker wiring (progress heartbeats + cooperative cancel) -----------

/// Hooks a GUI worker installs on ITS OWN thread before a generative call:
/// `progress` receives the human-readable liveness lines (partial-image
/// heartbeats, negotiation notes) that otherwise reach only stderr; `cancel`
/// is a cooperative stop flag checked between negotiation attempts, per
/// stream event, and before the composite. Thread-local by design — the whole
/// call runs on the caller's thread, the web server threads requests, and a
/// process-wide global would cross-wire concurrent callers.
pub struct WorkerHooks {
    pub progress: Box<dyn FnMut(String)>,
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

thread_local! {
    static WORKER_HOOKS: std::cell::RefCell<Option<WorkerHooks>> =
        const { std::cell::RefCell::new(None) };
}

/// Install this thread's worker hooks. Workers run on a fresh thread each
/// (`spawn_worker`), so a hook can never leak onto another task — it dies
/// with the thread.
pub fn set_worker_hooks(hooks: Option<WorkerHooks>) {
    WORKER_HOOKS.with(|h| *h.borrow_mut() = hooks);
}

/// Mirror a liveness line into the installed GUI hook (no-op elsewhere).
/// Callers keep their own stderr/stdout prints — CLI output stays identical.
fn gui_progress(line: &str) {
    WORKER_HOOKS.with(|h| {
        if let Some(hooks) = h.borrow_mut().as_mut() {
            (hooks.progress)(line.trim_start().to_string());
        }
    });
}

/// Cooperative cancel: true once the user hit Cancel in the GUI.
fn cancelled() -> bool {
    WORKER_HOOKS.with(|h| {
        h.borrow().as_ref().is_some_and(|hooks| {
            hooks.cancel.load(std::sync::atomic::Ordering::Relaxed)
        })
    })
}

/// Full-frame generative restyle (the user's experiment). `fidelity` = "high"
/// composes the prompt onto [`FAITHFUL_SCAFFOLD`] so the model re-develops
/// the same photograph; "low" gives the model free rein (the user's prompt
/// goes to the wire verbatim). `quality` is the output tier
/// (low|medium|high|auto). After generation the structural divergence D
/// against the sent input is measured and disclosed; `retry_on_divergence`
/// (an explicit opt-in — it buys a SECOND generation) retries once when
/// `D >= fit::DIVERGENCE_GLOBAL` and keeps whichever result measures closer.
pub fn reimagine(
    cfg: &Config,
    raw_path: &Path,
    prompt: &str,
    fidelity: &str,
    quality: &str,
    retry_on_divergence: bool,
    out: &Path,
) -> Result<ReimagineReport> {
    // FIRST, before any decode and before the BILLED image call (L09#1):
    // the parent used to be created 8 lines after paying, and its failure
    // dropped the generated bytes on the floor — pure money loss. Living in
    // the library fn covers every caller; for GUI/web it is a no-op
    // (unique_out already prepared the path).
    pipeline::preflight_out(out, raw_path)?;
    let src = decode::preview_only(raw_path)?;
    let preview = src.dimensions();
    // The plan sizes the frame that is SENT, because the pixels are resized
    // to it exactly. A RAW sends its sensor frame — `frame_size`'s
    // DefaultCropSize rectangle in display orientation, the frame the develop
    // below and `match` both use — never the embedded preview's: a body set
    // to a 4:3 aspect writes a 4:3 preview over its 3:2 sensor, and a plan
    // sized from that preview stretched the 3:2 develop into 4:3 on the wire
    // (v1.2.2; the target then came back 4:3 against a 3:2 develop).
    let frame = sent_frame(raw_path, preview)?;
    let sizes = SizePlan::for_source(cfg, frame.0, frame.1);
    let (sw, sh) = parse_size(sizes.try_first());

    // Input pixels: when the flexible target outresolves the embedded preview of
    // a RAW — or the preview is not the sensor frame at all — feed a full-sensor
    // neutral develop instead: real detail in, not an upscaled ~1.6 MP preview
    // (input quality bounds faithful-region output), in the frame the plan sized.
    let base = if develop_is_sent(decode::is_raw(raw_path), (sw, sh), preview, frame) {
        println!("  developing full sensor for a sharp high-res input …");
        gui_progress("developing full sensor for a sharp high-res input …");
        // Capped at 2× the API target edge: visually native for the final
        // Lanczos downsample, ~16× lighter than a full 61 MP develop.
        crate::render::render_to_image(
            raw_path,
            &crate::recipe::EditRecipe::default(),
            None,
            Some(sw.max(sh).saturating_mul(2)),
        )?
    } else {
        src
    };
    let small = DynamicImage::ImageRgb8(base.resize_exact(sw, sh, FilterType::Lanczos3).to_rgb8());
    let png = encode_png(&small)?;
    // Enum-size fallback input — aspect must match the size requested (L09-2).
    let fallback = sizes
        .flexible
        .is_some()
        .then(|| -> Result<Vec<u8>> {
            let (ew, eh) = parse_size(sizes.enum_size);
            encode_png(&DynamicImage::ImageRgb8(
                base.resize_exact(ew, eh, FilterType::Lanczos3).to_rgb8(),
            ))
        })
        .transpose()?;
    let primary_size = sizes.try_first().to_string();
    println!(
        "⚠ EXPERIMENTAL generative re-render via {} ({}, quality={quality} — regenerated pixels, not a master)",
        cfg.openai_image_model,
        sizes.try_first(),
    );
    let wire_prompt = hardened_prompt(prompt, fidelity);
    // Named so the opt-in retry below can ask for inputs a second time.
    let mut provide = |size: &str| -> Result<SizedInputs> {
        if size != primary_size
            && let Some(fb) = &fallback
        {
            return Ok((fb.clone(), None));
        }
        Ok((png.clone(), None))
    };
    let GeneratedImage { bytes: mut result, requested: used, accepted: mut on_disk } =
        call_images_edit(cfg, &mut provide, &wire_prompt, fidelity, &sizes, quality)?;
    if cancelled() {
        // The epoch already discards the UI-side result — writing the
        // artifact anyway left a persistent ./out master for an operation
        // the user abandoned (and kept its claimed name occupied; the
        // caller's 0-byte probe releases it once we bail here).
        return Err(anyhow!("cancelled by user"));
    }

    // The fidelity reading: D against the input the model was actually sent
    // (the enum-fallback bytes when that size was the one accepted).
    let sent_for = |used: &str| -> &[u8] {
        if used != primary_size
            && let Some(fb) = &fallback
        {
            return fb.as_slice();
        }
        png.as_slice()
    };
    let threshold = crate::fit::DIVERGENCE_GLOBAL;
    let mut divergence = generation_divergence(sent_for(&used), &result)?;
    let line = format!(
        "  fidelity: structural divergence D={:.3} vs the sent input (reverse-fit \
         global threshold {threshold:.2})",
        divergence.d
    );
    println!("{line}");
    gui_progress(&line);
    if divergence.d >= threshold {
        let warn = format!(
            "  ⚠ D={:.3} ≥ {threshold:.2}: the generation altered structure/content — a \
             reverse-fit of this result will fall back to atmosphere mode",
            divergence.d
        );
        println!("{warn}");
        gui_progress(&warn);
    }

    let mut first_divergence = None;
    if retry_on_divergence && divergence.d >= threshold {
        if cancelled() {
            return Err(anyhow!("cancelled by user"));
        }
        // A DELIBERATE second purchase, not a re-POST of a lost response:
        // the first generation is kept, measured, and only REPLACED when the
        // second measures closer to the input. Bounded to ONE retry, behind
        // an explicit opt-in. The no-re-POST-past-2xx rule inside
        // `call_images_edit` guards against paying twice for ONE image; this
        // is the user explicitly buying a second one.
        let line = format!(
            "  fidelity retry (opt-in): buying ONE more generation (D={:.3} ≥ \
             {threshold:.2})",
            divergence.d
        );
        println!("{line}");
        gui_progress(&line);
        let second =
            call_images_edit(cfg, &mut provide, &wire_prompt, fidelity, &sizes, quality)?;
        // Measured against what the SECOND call was sent, not the first.
        let d2 = generation_divergence(sent_for(&second.requested), &second.bytes)?;
        let line = format!(
            "  fidelity retry measured D={:.3} (first attempt D={:.3})",
            d2.d, divergence.d
        );
        println!("{line}");
        gui_progress(&line);
        if d2.d < divergence.d {
            first_divergence = Some(divergence.d);
            result = second.bytes;
            on_disk = second.accepted;
            divergence = d2;
        } else {
            first_divergence = Some(d2.d);
        }
        let keep = format!("  fidelity retry: keeping the D={:.3} result", divergence.d);
        println!("{keep}");
        gui_progress(&keep);
        if cancelled() {
            return Err(anyhow!("cancelled by user"));
        }
    }
    // (Parent already prepared by the pre-pay preflight at the top, L09#1.)
    // Stage + rename over the claimed name (the batch-13 export rule): a
    // crash mid-write must never leave a PARTIAL image wearing a name
    // pixels.json or a browser session may already reference.
    let tmp = out.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        crate::store::next_tmp_seq()
    ));
    if let Err(e) = std::fs::write(&tmp, &result) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("write {}", tmp.display()));
    }
    // durable_replace, not bare rename (L03): the pixel-source JSON that
    // will reference this master commits durably — the payload first.
    if let Err(e) = crate::store::durable_replace(&tmp, out) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("publish {}", out.display()));
    }
    // The size on disk, which is not the requested size when the endpoint capped.
    println!("generative -> {} ({on_disk}, generative re-render)", out.display());
    Ok(ReimagineReport { divergence, first_divergence })
}

/// Object removal / generative fill on the SOURCE's own pixels. `mask_path`
/// is an RGBA PNG; transparent (alpha=0) pixels mark the region to
/// regenerate. A blank `prompt` is the removal instruction ([`fill_prompt`]);
/// typed words ride verbatim. The generative result is composited back over
/// the base so only the masked region is re-rendered.
///
/// For a RAW the base is the engine's OWN neutral develop — a ≤2048px
/// thumbnail by default, the full sensor (e.g. 61 MP) with `full_res` so the
/// untouched area keeps native resolution (slow; the regenerated patch is
/// upscaled). It is deliberately NOT the camera's baked 8-bit preview, which
/// this used to composite onto: that swapped the canvas onto camera-curve
/// pixels mid-session and put later edits/exports on a different tone chain
/// (see `retouch::heal`). For a baked PNG/TIFF the base is the full image
/// either way, so `full_res` changes nothing. This is the CLI's and the
/// browser's fill; the GUI hands [`retouch_onto`] the active card's
/// developed picture instead.
pub fn retouch(
    cfg: &Config,
    raw_path: &Path,
    mask_path: &Path,
    prompt: &str,
    quality: &str,
    full_res: bool,
    out: &Path,
) -> Result<()> {
    let raw = decode::is_raw(raw_path);
    // ONE dispatch for both source kinds (`render::source_pixels`). The cap is
    // RAW-only here: preview mode develops a RAW AT ≤2048 (the cap runs before
    // tone/geometry) instead of developing 61 MP and thumbnailing the result,
    // while a baked source is composited at its OWN resolution either way (the
    // doc above) — capping it would shrink the delivered master.
    retouch_onto(
        cfg,
        raw_path,
        || crate::render::source_pixels(raw_path, (raw && !full_res).then_some(2048)),
        if full_res && raw { "full-res" } else { "preview" },
        &FillJob { mask_path, prompt, quality, out, layer: false },
    )
}

/// The four things every fill needs whatever its base: the painted mask, the
/// user's words (blank = remove), the quality tier and the claimed output path.
pub struct FillJob<'a> {
    /// RGBA PNG; transparent (alpha=0) pixels mark the region to regenerate.
    pub mask_path: &'a Path,
    /// What should fill the region; blank is the removal instruction.
    pub prompt: &'a str,
    /// `low` / `medium` / `high` / `auto`.
    pub quality: &'a str,
    /// The claimed `./out` master this fill publishes.
    pub out: &'a Path,
    /// Publish a LAYER instead of a composite (2026-10-01, the GUI): the
    /// generated pixels with the feathered painted area as their alpha and
    /// nothing elsewhere, for `render::PixelLayer` to lay over the card's
    /// develop. The same paste onto a transparent frame instead of the base.
    pub layer: bool,
}

/// The fill on a base its CALLER produces. `base()` runs INSIDE the
/// full-resolution section (it is the whole-frame local phase), the model
/// call runs outside it, and the composite re-enters it. [`retouch`] hands
/// this the source's neutral develop; the GUI hands it the active card's
/// developed picture, so the model is shown what the user sees and the
/// answer is that picture with the painted area regenerated. On the neutral
/// base (every build through v1.3.4) the model was shown a develop 0.6–1.4 EV
/// under the card's Before with none of the card's sliders or masks, and the
/// card's content-keyed corrections then landed on content they were never
/// solved for — the user's 「不是基于当前变体生成的，而是原图」(2026-09-15).
///
/// `source` is the photo the output is guarded against (the read-only-library
/// preflight) and named after; `base_note` is the word the progress line
/// prints for the base; `job` is the fill itself ([`FillJob`]).
pub fn retouch_onto(
    cfg: &Config,
    source: &Path,
    base: impl FnOnce() -> Result<DynamicImage>,
    base_note: &str,
    job: &FillJob<'_>,
) -> Result<()> {
    // FIRST, before the develop and the BILLED call (L09#1).
    pipeline::preflight_out(job.out, source)?;
    // Blank = remove, shared by the CLI, browser and GUI.
    let prompt = fill_prompt(job.prompt);
    edit_onto(cfg, base, base_note, &ImageEditJob {
        keep: Keep::Painted(job.mask_path), prompt: &prompt, quality: job.quality, out: job.out, layer: job.layer,
    })?;
    Ok(())
}

/// A fill whose area the MODEL chooses (user request 2026-10-12: 「跟原先的涂抹后ai生成一个道理，
/// 只不过省去了涂抹这一步骤」): the words say what to change — "remove the tree on the
/// left", "make the sky bluer" — and no mask is painted.
pub struct AutoFillJob<'a> {
    /// What to change; a blank instruction is refused (there is no painted
    /// area to read "remove" off).
    pub prompt: &'a str,
    /// `low` / `medium` / `high` / `auto`.
    pub quality: &'a str,
    /// The claimed `./out` layer this edit publishes.
    pub out: &'a Path,
}

/// What one [`auto_fill_onto`] found.
#[derive(Debug)]
pub struct AutoFillReport {
    /// The share of the frame the model changed and the layer covers, 0..=1.
    pub coverage: f32,
}

/// The wire prompt of an [`AutoFillJob`]: the user's instruction under a
/// clause that keeps the rest of the photograph as it is, because everything
/// the model changes beyond the instruction is read back as part of the edit.
fn auto_fill_prompt(user: &str) -> String {
    format!(
        "Edit this photograph exactly as instructed and change nothing else: keep the framing, \
         geometry, light, colours, grain and every detail the instruction does not concern \
         exactly as they are.\nInstruction: {}",
        user.trim()
    )
}

/// The edit with no painted area: the whole picture goes to the model with
/// no mask, and the area it CHANGED — found by comparing its answer with the
/// picture it was sent ([`found_region`]) — is published as a layer, levelled
/// onto its surroundings like a painted fill. Everything the model left alone
/// stays the card's own develop.
pub fn auto_fill_onto(
    cfg: &Config,
    source: &Path,
    base: impl FnOnce() -> Result<DynamicImage>,
    base_note: &str,
    job: &AutoFillJob<'_>,
) -> Result<AutoFillReport> {
    pipeline::preflight_out(job.out, source)?;
    if job.prompt.trim().is_empty() {
        return Err(anyhow!("an edit without a painted area needs words saying what to change"));
    }
    let prompt = auto_fill_prompt(job.prompt);
    let outcome = edit_onto(cfg, base, base_note, &ImageEditJob {
        keep: Keep::Found, prompt: &prompt, quality: job.quality, out: job.out, layer: true,
    })?;
    Ok(AutoFillReport { coverage: outcome.coverage.context("an edit without a painted area found no area")? })
}

/// A prompt-only edit of the caller's whole picture, without a painted mask.
pub struct AdjustJob<'a> {
    /// What to change. Unlike a fill, a whole-image adjust refuses blank words.
    pub prompt: &'a str,
    /// `low` / `medium` / `high` / `auto`.
    pub quality: &'a str,
    /// The claimed `./out` master this adjust publishes.
    pub out: &'a Path,
}

/// What one whole-image [`adjust_onto`] measured against its SENT input.
#[derive(Debug)]
pub struct AdjustReport {
    /// The same structural D as reimagine, before resizing back to the base.
    pub divergence: f32,
}

/// Edit the caller's picture WITHOUT a mask; keep the whole returned frame,
/// resized back to the base's dimensions. The GUI supplies the generated
/// card's live develop and lands the result as a new, neutral ✨ card.
/// Preflight, admission slots, size negotiation, cancel and durable output
/// are shared with [`retouch_onto`]; neither path buys a divergence retry.
pub fn adjust_onto(
    cfg: &Config,
    source: &Path,
    base: impl FnOnce() -> Result<DynamicImage>,
    base_note: &str,
    job: &AdjustJob<'_>,
) -> Result<AdjustReport> {
    pipeline::preflight_out(job.out, source)?;
    let prompt = job.prompt.trim();
    if prompt.is_empty() {
        return Err(anyhow!("a whole-image adjust needs a non-blank prompt"));
    }
    let outcome = edit_onto(cfg, base, base_note, &ImageEditJob {
        keep: Keep::Whole, prompt, quality: job.quality, out: job.out, layer: false,
    })?;
    Ok(AdjustReport { divergence: outcome.divergence.context("whole-image adjust has no divergence reading")? })
}

/// Which of the model's pixels an edit keeps.
#[derive(Clone, Copy)]
enum Keep<'a> {
    /// The painted area of this RGBA mask (transparent = regenerate); the
    /// mask rides on the request.
    Painted(&'a Path),
    /// The area the model's answer changed ([`found_region`]); no mask is sent.
    Found,
    /// The whole returned frame.
    Whole,
}

/// One shared request/size ladder and output path; `keep` decides what of
/// the answer is published.
struct ImageEditJob<'a> {
    keep: Keep<'a>,
    prompt: &'a str,
    quality: &'a str,
    out: &'a Path,
    layer: bool,
}

/// What [`edit_onto`] measured: D for a whole frame, the found area's share
/// for [`Keep::Found`].
struct EditOutcome {
    divergence: Option<f32>,
    coverage: Option<f32>,
}

fn edit_onto(
    cfg: &Config,
    base: impl FnOnce() -> Result<DynamicImage>,
    base_note: &str,
    job: &ImageEditJob<'_>,
) -> Result<EditOutcome> {
    let ImageEditJob { keep, prompt, quality, out, layer } = *job;
    // A17: the LOCAL full-resolution phase, one at a time process-wide
    // (`crate::full_res_slot`). It is released explicitly before the model call
    // below and re-entered for the composite, because those are the two phases
    // that hold whole frames — holding an admission slot across a call that
    // runs for minutes would stall every export for that entire time.
    let heavy = crate::full_res_slot();
    let base = base()?;
    let (bw, bh) = base.dimensions();
    // A generative tile larger than the base is pointless (it only gets downscaled
    // back onto it) — cap the flexible budget at the base's own pixel count.
    let budget = cfg.openai_image_max_px.min(bw.saturating_mul(bh));
    let sizes = SizePlan::for_budget(bw, bh, budget);
    let (sw, sh) = parse_size(sizes.try_first());

    // API input must be 8-bit (the full-res base is 16-bit). Derive the small
    // image from THIS base so the generated pixels match its look (no seam shift).
    let small = DynamicImage::ImageRgb8(base.resize_exact(sw, sh, FilterType::Lanczos3).to_rgb8());
    let png = encode_png(&small)?;
    drop(small);
    let mask_img = match keep {
        Keep::Painted(path) => Some(
            crate::render::open_mask_bounded(path).with_context(|| format!("open mask {}", path.display()))?,
        ),
        Keep::Found | Keep::Whole => None,
    };
    let mask_at = |w, h| {
        mask_img.as_ref().map(|m| encode_png(&m.resize_exact(w, h, FilterType::Nearest))).transpose()
    };
    let mask_png = mask_at(sw, sh)?;
    // The enum-size fallback pair, built EAGERLY from the same 16-bit base:
    // the base is dropped just below (the A7 memory staging), and a retry
    // must send an input whose aspect matches the size it requests (L09-2).
    let fallback = sizes
        .flexible
        .is_some()
        .then(|| -> Result<(Vec<u8>, Option<Vec<u8>>)> {
            let (ew, eh) = parse_size(sizes.enum_size);
            let fpng = encode_png(&DynamicImage::ImageRgb8(
                base.resize_exact(ew, eh, FilterType::Lanczos3).to_rgb8(),
            ))?;
            Ok((fpng, mask_at(ew, eh)?))
        })
        .transpose()?;
    let primary_size = sizes.try_first().to_string();

    // The composite below needs only 8-bit pixels — take that form NOW and
    // let the 16-bit master go, so the minutes-long model call holds the
    // ~240 MB composite buffer instead of the ~366 MB master, and the
    // post-call peak loses a full 16-bit frame (A7). A cancel that already
    // arrived is honoured BEFORE paying the conversion; an immediately
    // failing call (bad key, rejected request) does pay this sub-second
    // pass first — accepted against the minutes-scale win (Codex batch-35).
    if cancelled() {
        return Err(anyhow!("cancelled by user"));
    }
    // A whole-image adjust keeps none of the base pixels, so only a fill
    // retains this full-frame buffer across the model call.
    // A layer pastes onto a TRANSPARENT frame (`FillJob::layer`): the
    // premultiplied paste below then leaves exactly the generated pixels
    // with the feathered weight as alpha, and the base is not kept at all.
    let composite = (!matches!(keep, Keep::Whole))
        .then(|| if layer { RgbaImage::new(bw, bh) } else { base.to_rgba8() });
    drop(base);
    // Out of the full-resolution section: what survives across the model call
    // is the ~240 MB composite (the A7 staging above), and waiting on the
    // network is not what the slot exists to serialise.
    drop(heavy);

    let (verb, mode) = match keep {
        Keep::Painted(_) => ("fill", "composite"),
        Keep::Found => ("edit", "changed area"),
        Keep::Whole => ("adjust", "whole image"),
    };
    println!(
        "⚠ EXPERIMENTAL generative {verb} via {} ({}, quality={quality}, base={bw}x{bh} {}; {mode})",
        cfg.openai_image_model,
        sizes.try_first(),
        base_note
    );
    // `requested` identifies the input ACTUALLY sent, including the enum
    // fallback. A capped return's delivered size must never select it.
    let result = call_images_edit(
        cfg,
        &mut |size| {
            if size != primary_size
                && let Some(fb) = &fallback
            {
                return Ok(fb.clone());
            }
            Ok((png.clone(), mask_png.clone()))
        },
        prompt,
        "high",
        &sizes,
        quality,
    )?;
    if cancelled() {
        // The user gave up while the model ran; skip the (full-res) composite
        // — at 61 MP it is real work whose result would be discarded anyway.
        return Err(anyhow!("cancelled by user"));
    }
    // The input actually sent, and its mask (the enum fallback's when the
    // negotiation fell back) — what the model's answer is measured against.
    let (sent_png, sent_mask) = match &fallback {
        Some((fpng, fmask)) if result.requested != primary_size => (fpng, fmask.as_ref()),
        _ => (&png, mask_png.as_ref()),
    };

    // Composite the regenerated region back onto the base. Upscale the generative
    // tile to base dimensions; the user's mask (alpha=0 = regenerate) becomes the
    // blend weight, feathered for a soft seam. Buffer LIFETIMES are staged so
    // a 61 MP full-res fill never holds every plane at once (the old
    // one-expression flow peaked ~1.8 GB; the staged flow halved that, and
    // dropping the 16-bit master BEFORE the model call — see above — takes
    // another full frame off this peak): the full-res mask exists only long
    // enough to become the weight plane, the generative frame is never
    // copied to RGBA, and the blur runs in place.
    //
    // STILL FULL-FRAME, deliberately: the generative upscale and the weight
    // plane are each ~241 MB at 61 MP. Banding them top-to-bottom would need
    // a hand-written replica of `image`'s Lanczos3 two-pass that is
    // byte-identical to it (a resampler that differs in the last bit is a
    // rendering change wearing an optimisation's clothes), which is its own
    // piece of work with its own oracle tests — not a comment away.
    // `to_rgb8`, not `to_rgba8`: the blend reads only RGB and writes an
    // opaque alpha, so the RGBA copy was 241 MB of pure waste at 61 MP —
    // and it coexisted with the 181 MB RGB frame it was copied from.
    // Back into the full-resolution section for the COMPOSITE: the generative
    // upscale and the weight plane are ~241 MB each at 61 MP, and this is the
    // phase the slot was scoped to.
    let _heavy = crate::full_res_slot();
    let answer = image::load_from_memory(&result.bytes).context("decode generative result")?;
    let mut outcome = EditOutcome { divergence: None, coverage: None };
    let Some(mut composite) = composite else {
        outcome.divergence = Some(generation_divergence(sent_png, &result.bytes)?.d);
        // No composite and no mask: keep the WHOLE returned frame, at the
        // base's dimensions, using the fill's Lanczos3 filter and RGBA8 writer.
        let whole = answer.resize_exact(bw, bh, FilterType::Lanczos3).to_rgba8();
        publish_master(&whole, out)?;
        println!("generative {verb} -> {} ({bw}x{bh}, {mode})", out.display());
        return Ok(outcome);
    };

    // The model never answers ONLY inside the area: it redraws the whole
    // frame, and the frame it draws is shifted from the one it was sent —
    // measured on a user's fill (2026-10-12) at +42 / +40 / +24 levels in
    // R/G/B right at the feathered edge, which is the seam the user saw. The
    // pixels it was meant to leave alone say by how much, so the kept area is
    // levelled by that shift before it is pasted ([`level_field`]), and the
    // area a maskless edit changed is read off the same comparison.
    let sent = image::load_from_memory(sent_png).context("decode the sent input")?.to_rgb8();
    let (tw, th) = sent.dimensions();
    let answer_at_sent = answer.resize_exact(tw, th, FilterType::Triangle).to_rgb8();
    let region = match keep {
        Keep::Painted(_) => {
            let m = image::load_from_memory(sent_mask.context("a painted fill sends its mask")?)
                .context("decode the sent mask")?
                .to_rgba8();
            Region::from_mask(&m)
        }
        Keep::Found => {
            let found = found_region(&sent, &answer_at_sent);
            let share = found.share();
            if share <= 0.0 {
                return Err(anyhow!(
                    "the model's answer changed nothing this edit could find — paint the area and use Remove / Fill instead"
                ));
            }
            outcome.coverage = Some(share);
            found
        }
        Keep::Whole => unreachable!("a whole-frame edit keeps no composite"),
    };
    let level = level_field(&sent, &answer_at_sent, &region);
    drop(answer_at_sent);
    drop(sent);
    let mut gen_img = answer.resize_exact(bw, bh, FilterType::Lanczos3).to_rgb8();
    drop(answer);
    // Cancel checkpoints through the composite: at 61 MP the decode/weight/
    // blend stages take seconds each, and a cancel arriving during them was
    // ignored — the abandoned result then landed in ./out anyway.
    if cancelled() {
        return Err(anyhow!("cancelled by user"));
    }
    let frame_feather = ((bw.min(bh) as usize) / 100).clamp(2, 64); // ~1% of short side, capped
    let weight = {
        let w = region.weight_at(bw, bh);
        // Feather scales with the SELECTION, not only the frame (L09-3):
        // the frame rule alone put a 20 px blur on a 10 px selection at a
        // 2048 frame — centre weight ~6%, and the fill "looked like it did
        // nothing". The frame rule stays as the CEILING, so large
        // selections keep their exact look.
        let feather = frame_feather.min(selection_feather(&w, bw as usize));
        if feather > 0 { box_blur(w, bw as usize, bh as usize, feather) } else { w }
    };
    if cancelled() {
        return Err(anyhow!("cancelled by user"));
    }
    level.apply(&mut gen_img, &weight);
    composite_in_place(&mut composite, &gen_img, &weight);
    drop(gen_img);
    if cancelled() {
        return Err(anyhow!("cancelled by user"));
    }
    publish_master(&composite, out)?;
    println!("generative {verb} -> {} ({bw}x{bh}, {mode})", out.display());
    Ok(outcome)
}

/// Stage + rename over the claimed name — same rule as reimagine. The
/// staged name cannot carry the real extension, so the format comes from
/// the TARGET path explicitly (PNG for every claimed ./out master). (Parent
/// already prepared by the pre-pay preflight, L09#1.)
fn publish_master(img: &RgbaImage, out: &Path) -> Result<()> {
    let tmp = out.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        crate::store::next_tmp_seq()
    ));
    let fmt = image::ImageFormat::from_path(out).unwrap_or(image::ImageFormat::Png);
    if let Err(e) = img.save_with_format(&tmp, fmt) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("write {}", tmp.display()));
    }
    // durable_replace, not bare rename (L03): the pixel-source JSON that
    // will reference this master commits durably — the payload first.
    if let Err(e) = crate::store::durable_replace(&tmp, out) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("publish {}", out.display()));
    }
    Ok(())
}

/// The area an edit regenerates, at the resolution of the input it SENT:
/// 1 = take the model's pixel, 0 = keep the base's. A painted mask gives it
/// texel for texel; [`found_region`] gives it per cell.
struct Region {
    w: usize,
    h: usize,
    take: Vec<f32>,
}

impl Region {
    /// From the RGBA mask on the wire: transparent = regenerate.
    fn from_mask(m: &RgbaImage) -> Self {
        let take = m.pixels().map(|p| 1.0 - p[3] as f32 / 255.0).collect();
        Region { w: m.width() as usize, h: m.height() as usize, take }
    }

    /// The share of the frame the model's pixels are taken over.
    fn share(&self) -> f32 {
        self.take.iter().sum::<f32>() / self.take.len().max(1) as f32
    }

    /// The weight plane at the base's `bw`×`bh`, by nearest texel — the
    /// painted mask's own rule (`FilterType::Nearest`); the caller feathers.
    fn weight_at(&self, bw: u32, bh: u32) -> Vec<f32> {
        let (bw, bh) = (bw as usize, bh as usize);
        let mut out = Vec::with_capacity(bw * bh);
        for y in 0..bh {
            let sy = (y * self.h / bh.max(1)).min(self.h - 1);
            out.extend((0..bw).map(|x| self.take[sy * self.w + (x * self.w / bw.max(1)).min(self.w - 1)]));
        }
        out
    }
}

/// The cell edge of the levelling and finding grids on a `w`×`h` sent frame:
/// ~1/96 of the short side — coarse enough that the model re-drawing grass or
/// rock in its own texture averages out, fine enough to follow a sky's
/// gradient into the painted area.
fn cell_edge(w: usize, h: usize) -> usize {
    (w.min(h) / 96).max(2)
}

/// A smooth per-channel offset over the frame, in 8-bit levels, stored on a
/// coarse grid of `cell`-px cells of the sent frame.
struct LevelField {
    gw: usize,
    gh: usize,
    v: Vec<[f32; 3]>,
}

/// How far the model's answer sits from the sent input where it was meant to
/// change nothing — `region.take == 0` — as a smooth field that runs on into
/// the area it did change. Each cell holds the mean of (sent − answer) over its
/// kept texels; cells with none are filled from their neighbours by
/// pull-push ([`pull_push`]), the membrane-like interpolation Poisson-style
/// blending reduces to at low frequency. Added to the model's pixels, it puts
/// the regenerated area on the level of what surrounds it while leaving the
/// structure the model drew inside untouched.
fn level_field(sent: &RgbImage, answer: &RgbImage, region: &Region) -> LevelField {
    let (w, h) = (sent.width() as usize, sent.height() as usize);
    let cell = cell_edge(w, h);
    let (gw, gh) = (w.div_ceil(cell), h.div_ceil(cell));
    let mut sum = vec![[0f32; 3]; gw * gh];
    let mut kept = vec![0f32; gw * gh];
    for y in 0..h {
        for x in 0..w {
            let k = 1.0 - region.take[y * w + x];
            if k <= 0.0 {
                continue;
            }
            let i = (y / cell) * gw + x / cell;
            let (s, a) = (sent.get_pixel(x as u32, y as u32), answer.get_pixel(x as u32, y as u32));
            for c in 0..3 {
                sum[i][c] += k * (s[c] as f32 - a[c] as f32);
            }
            kept[i] += k;
        }
    }
    let area = (cell * cell) as f32;
    let mut v = vec![[0f32; 3]; gw * gh];
    let mut conf = vec![0f32; gw * gh];
    for i in 0..gw * gh {
        if kept[i] > 0.0 {
            v[i] = sum[i].map(|s| s / kept[i]);
            conf[i] = (kept[i] / area).min(1.0);
        }
    }
    LevelField { gw, gh, v: pull_push(v, conf, gw, gh) }
}

impl LevelField {
    /// The offset at normalised frame position (u, v), bilinear between cell
    /// centres.
    fn at(&self, u: f32, v: f32) -> [f32; 3] {
        let fx = (u * self.gw as f32 - 0.5).clamp(0.0, (self.gw - 1) as f32);
        let fy = (v * self.gh as f32 - 0.5).clamp(0.0, (self.gh - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.gw - 1), (y0 + 1).min(self.gh - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let p = |x: usize, y: usize| self.v[y * self.gw + x];
        let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
        std::array::from_fn(|k| {
            (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty
        })
    }

    /// Level the model's pixels wherever the composite will take any of them.
    fn apply(&self, img: &mut RgbImage, weight: &[f32]) {
        let (w, h) = img.dimensions();
        for y in 0..h {
            for x in 0..w {
                if weight[(y * w + x) as usize] <= 0.0001 {
                    continue;
                }
                let off = self.at((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                let p = img.get_pixel_mut(x, y);
                for c in 0..3 {
                    p[c] = (p[c] as f32 + off[c]).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

/// Fill a grid's unknown cells from its known ones: pull — halve the grid,
/// each coarse cell the confidence-weighted mean of its four — until one cell
/// is left, then push — every fine cell keeps its own value in proportion to
/// its confidence and takes the rest from the coarser level, read bilinearly.
/// A fully known cell is returned exactly; a hole gets a smooth blend of the
/// values around it, the nearer ones weighing more.
fn pull_push(v: Vec<[f32; 3]>, conf: Vec<f32>, w: usize, h: usize) -> Vec<[f32; 3]> {
    if w <= 1 && h <= 1 {
        return v;
    }
    let (w2, h2) = (w.div_ceil(2), h.div_ceil(2));
    let mut cv = vec![[0f32; 3]; w2 * h2];
    let mut cc = vec![0f32; w2 * h2];
    for y in 0..h {
        for x in 0..w {
            let (i, j) = (y * w + x, (y / 2) * w2 + x / 2);
            for c in 0..3 {
                cv[j][c] += conf[i] * v[i][c];
            }
            cc[j] += conf[i];
        }
    }
    for j in 0..w2 * h2 {
        if cc[j] > 0.0 {
            cv[j] = cv[j].map(|s| s / cc[j]);
        }
        cc[j] = cc[j].min(1.0);
    }
    let coarse = pull_push(cv, cc, w2, h2);
    let mut out = v;
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let c = conf[i].clamp(0.0, 1.0);
            if c >= 1.0 {
                continue;
            }
            let fx = ((x as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (w2 - 1) as f32);
            let fy = ((y as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (h2 - 1) as f32);
            let (x0, y0) = (fx as usize, fy as usize);
            let (x1, y1) = ((x0 + 1).min(w2 - 1), (y0 + 1).min(h2 - 1));
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            let p = |x: usize, y: usize| coarse[y * w2 + x];
            for (k, v) in out[i].iter_mut().enumerate() {
                let up = (p(x0, y0)[k] * (1.0 - tx) + p(x1, y0)[k] * tx) * (1.0 - ty)
                    + (p(x0, y1)[k] * (1.0 - tx) + p(x1, y1)[k] * tx) * ty;
                *v = c * *v + (1.0 - c) * up;
            }
        }
    }
    out
}

/// One [`found_region`] run's cell scores, grid width and height, seed and
/// grow thresholds.
#[cfg(test)]
type FoundScores = (Vec<f32>, usize, usize, f32, f32);

#[cfg(test)]
thread_local! {
    /// The last [`found_region`]'s scores — read by the offline tuning harness.
    static FOUND_SCORES: std::cell::RefCell<Option<FoundScores>> =
        const { std::cell::RefCell::new(None) };
}

/// Per cell of a `cell`-px grid over `sent` and `answer`: how far the answer
/// sits from `expect` — the sent frame moved by a drift — in mean colour, and
/// how far its texture (luma spread) changed, folded into one score.
fn cell_scores(
    sent: &RgbImage,
    answer: &RgbImage,
    cell: usize,
    gw: usize,
    drift: &dyn Fn(usize) -> [f32; 3],
) -> Vec<f32> {
    let (w, h) = (sent.width() as usize, sent.height() as usize);
    let luma = |p: [f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
    // Per cell: Σ(answer − expected) per channel, Σ and Σ² of each luma, count.
    let mut acc = vec![[0f64; 8]; gw * h.div_ceil(cell)];
    for y in 0..h {
        for x in 0..w {
            let i = (y / cell) * gw + x / cell;
            let d = drift(i);
            let (s, a) = (sent.get_pixel(x as u32, y as u32), answer.get_pixel(x as u32, y as u32));
            // Clamped like the answer is: a drift cannot lift a clipped white.
            let e: [f32; 3] = std::array::from_fn(|c| (s[c] as f32 + d[c]).clamp(0.0, 255.0));
            let a: [f32; 3] = std::array::from_fn(|c| a[c] as f32);
            let (le, la) = (luma(e) as f64, luma(a) as f64);
            let t = &mut acc[i];
            for c in 0..3 {
                t[c] += (a[c] - e[c]) as f64;
            }
            t[3] += le;
            t[4] += le * le;
            t[5] += la;
            t[6] += la * la;
            t[7] += 1.0;
        }
    }
    acc.iter()
        .map(|t| {
            let k = t[7].max(1.0);
            let colour = (0..3).map(|c| (t[c] / k).powi(2)).sum::<f64>().sqrt();
            let sd = |sum: f64, sq: f64| (sq / k - (sum / k).powi(2)).max(0.0).sqrt();
            (colour + 0.5 * (sd(t[5], t[6]) - sd(t[3], t[4])).abs()) as f32
        })
        .collect()
}

/// The area a maskless edit CHANGED, read off its answer against the input
/// it was sent, cell by cell ([`cell_edge`]).
///
/// The model moves the whole frame a little (see [`edit_onto`]) and redraws
/// every edge and texture a pixel or two off, so "changed" is read against
/// both. The drift is modelled as little as it can be — one shift per
/// channel, the median over the half of the cells that moved least, then a
/// smooth field ([`pull_push`]) learnt ONLY from the cells that shift leaves
/// unchanged — because a model
/// any freer learns the edit itself: measured on a real "make the sky bluer"
/// (2026-10-12; sky −28 / −9 / +14 levels, ground +7 / +2 / −1), a per-level
/// tone curve absorbed the whole sky as drift and found 1 % of the frame.
/// Texels at 0 or 255 are left out of the drift's estimate, and the expected
/// frame is clamped, so a clipped white is not read as a change.
///
/// Thresholds are the larger of a floor in 8-bit levels and the scatter of
/// the cells taken for drift (median + k·MAD); a seed must ALSO reach a fixed share of the
/// strongest change in the frame, which is what keeps the redrawn edges
/// (scores ~10–25 on the same real answer, the removed tree 69) out of the
/// area. Seeds grow through weaker connected cells, specks of one or two
/// cells are dropped, enclosed holes are filled and the area is grown by one
/// cell so the feather starts outside what changed.
fn found_region(sent: &RgbImage, answer: &RgbImage) -> Region {
    const SEED_FLOOR: f32 = 14.0;
    const GROW_FLOOR: f32 = 7.0;
    const PROMINENCE: f32 = 0.4;
    let (w, h) = (sent.width() as usize, sent.height() as usize);
    let cell = cell_edge(w, h);
    let (gw, gh) = (w.div_ceil(cell), h.div_ceil(cell));
    let n = gw * gh;
    // Per cell: the mean shift over its unclipped texels, and how many.
    let mut shift = vec![[0f64; 4]; n];
    for y in 0..h {
        for x in 0..w {
            let (s, a) = (sent.get_pixel(x as u32, y as u32), answer.get_pixel(x as u32, y as u32));
            if (0..3).any(|c| s[c] <= 2 || s[c] >= 253 || a[c] <= 2 || a[c] >= 253) {
                continue;
            }
            let t = &mut shift[(y / cell) * gw + x / cell];
            for c in 0..3 {
                t[c] += a[c] as f64 - s[c] as f64;
            }
            t[3] += 1.0;
        }
    }
    let measured: Vec<usize> = (0..n).filter(|&i| shift[i][3] > 0.0).collect();
    let mean_shift = |i: usize| -> [f32; 3] { std::array::from_fn(|c| (shift[i][c] / shift[i][3].max(1.0)) as f32) };
    let median = |mut v: Vec<f32>| -> f32 {
        if v.is_empty() {
            return 0.0;
        }
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    };
    // The scatter of the cells taken for drift, not of the frame: an edit as
    // large as the sky makes the scores bimodal, and a MAD over both modes put
    // the thresholds above the edit itself.
    let thresholds = |score: &[f32], drift_cells: &[usize]| -> (f32, f32) {
        let quiet: Vec<f32> = drift_cells.iter().map(|&i| score[i]).collect();
        let med = median(quiet.clone());
        let mad = 1.4826 * median(quiet.iter().map(|s| (s - med).abs()).collect());
        ((med + 8.0 * mad).max(SEED_FLOOR), (med + 4.0 * mad).max(GROW_FLOOR))
    };
    // Pass 1: one shift for the frame, the median over the half of the cells
    // that moved LEAST — the model's drift is the smaller movement (2–7
    // levels on the real answers above, an edit 30 and more), and over all
    // cells a sky-sized edit would be the median itself.
    // By RANK, not "at or below the median": on a frame whose edit moved
    // every one of its cells alike, the ties put the whole edit in.
    let size = |d: [f32; 3]| d.iter().map(|v| v * v).sum::<f32>();
    let mut still = measured.clone();
    still.sort_by(|&i, &j| size(mean_shift(i)).total_cmp(&size(mean_shift(j))));
    still.truncate(still.len().div_ceil(2));
    let global: [f32; 3] = std::array::from_fn(|c| median(still.iter().map(|&i| mean_shift(i)[c]).collect()));
    let first = cell_scores(sent, answer, cell, gw, &|_| global);
    let (_, grow1) = thresholds(&first, &still);
    // Pass 2: a smooth field from the cells pass 1 left alone.
    let conf: Vec<f32> = (0..n).map(|i| if first[i] < grow1 && shift[i][3] > 0.0 { 1.0 } else { 0.0 }).collect();
    let field = if conf.iter().any(|&c| c > 0.0) {
        pull_push((0..n).map(mean_shift).collect(), conf.clone(), gw, gh)
    } else {
        vec![global; n]
    };
    let score = cell_scores(sent, answer, cell, gw, &|i| field[i]);
    let calm: Vec<usize> = (0..n).filter(|&i| conf[i] > 0.0).collect();
    let (seed, grow) = thresholds(&score, if calm.is_empty() { &still } else { &calm });
    let seed = seed.max(PROMINENCE * score.iter().copied().fold(0.0, f32::max));
    #[cfg(test)]
    FOUND_SCORES.with_borrow_mut(|d| *d = Some((score.clone(), gw, gh, seed, grow)));
    // Seeds, then 4-connected growth through the weaker cells.
    let mut on: Vec<bool> = score.iter().map(|&s| s > seed).collect();
    let mut stack: Vec<usize> = (0..n).filter(|&i| on[i]).collect();
    while let Some(i) = stack.pop() {
        let (x, y) = (i % gw, i / gw);
        for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
            if nx < gw && ny < gh {
                let j = ny * gw + nx;
                if !on[j] && score[j] > grow {
                    on[j] = true;
                    stack.push(j);
                }
            }
        }
    }
    // Specks: a component of one or two cells is a redrawn edge or a fleck of
    // texture that crossed the seed line, not an edit (the real "sky bluer"
    // answer left six of them on the ground). Anything an edit can be asked
    // to remove spans more cells than that at ~1/96 of the short side.
    let mut seen = vec![false; n];
    for start in 0..n {
        if !on[start] || seen[start] {
            continue;
        }
        let mut comp = vec![start];
        seen[start] = true;
        let mut k = 0;
        while k < comp.len() {
            let (x, y) = (comp[k] % gw, comp[k] / gw);
            for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
                if nx < gw && ny < gh {
                    let j = ny * gw + nx;
                    if on[j] && !seen[j] {
                        seen[j] = true;
                        comp.push(j);
                    }
                }
            }
            k += 1;
        }
        if comp.len() <= 2 {
            comp.iter().for_each(|&i| on[i] = false);
        }
    }
    // Holes: every off cell the frame's border cannot reach through off cells.
    let mut outside = vec![false; n];
    let mut stack: Vec<usize> = (0..n)
        .filter(|&i| {
            let (x, y) = (i % gw, i / gw);
            !on[i] && (x == 0 || y == 0 || x + 1 == gw || y + 1 == gh)
        })
        .collect();
    stack.iter().for_each(|&i| outside[i] = true);
    while let Some(i) = stack.pop() {
        let (x, y) = (i % gw, i / gw);
        for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
            if nx < gw && ny < gh {
                let j = ny * gw + nx;
                if !on[j] && !outside[j] {
                    outside[j] = true;
                    stack.push(j);
                }
            }
        }
    }
    // One cell of growth, so the feather starts outside what changed.
    let grown: Vec<bool> = (0..n)
        .map(|i| {
            let (x, y) = (i % gw, i / gw);
            let near = |dx: isize, dy: isize| {
                let (nx, ny) = (x as isize + dx, y as isize + dy);
                nx >= 0 && ny >= 0 && (nx as usize) < gw && (ny as usize) < gh && {
                    let j = ny as usize * gw + nx as usize;
                    on[j] || !outside[j]
                }
            };
            (-1..=1).any(|dy| (-1..=1).any(|dx| near(dx, dy)))
        })
        .collect();
    let take = (0..w * h).map(|p| if grown[((p / w) / cell) * gw + (p % w) / cell] { 1.0 } else { 0.0 }).collect();
    Region { w, h, take }
}

/// The frame `reimagine` puts on the wire for `path`: a RAW's sensor frame
/// ([`decode::frame_size`] — DefaultCropSize in display orientation, what the
/// full develop renders), a baked image's own pixels. The size plan is drawn
/// from THIS frame and from nothing else, because the input is resized to the
/// plan exactly: sizing from a RAW's embedded preview while sending its
/// develop stretched the develop whenever the body had written the preview
/// at an in-camera aspect crop.
fn sent_frame(path: &Path, preview: (u32, u32)) -> Result<(u32, u32)> {
    if !decode::is_raw(path) {
        return Ok(preview);
    }
    let (w, h) = decode::frame_size(path)?;
    Ok((w as u32, h as u32))
}

/// Whether the full develop, not the embedded preview, is the input: when the
/// planned size outresolves the preview, and whenever the preview is not the
/// sensor frame (an in-camera aspect crop) — resizing a 4:3 preview onto a
/// 3:2 plan would stretch it exactly as sizing from it stretched the develop.
fn develop_is_sent(raw: bool, planned: (u32, u32), preview: (u32, u32), frame: (u32, u32)) -> bool {
    if !raw {
        return false;
    }
    let outresolved = planned.0.max(planned.1) > preview.0.max(preview.1);
    outresolved || !crate::fit::same_frame_plausible_dims(preview, frame)
}

/// The output-size request strategy: try the flexible high-res size first (when
/// the budget allows one), fall back to the universally-supported enum size when
/// the model 400s it. Carrying both here keeps the retry logic in
/// [`call_images_edit`] mechanical.
struct SizePlan {
    /// Flexible WIDTHxHEIGHT (gpt-image-2-style), when one fits the budget.
    flexible: Option<String>,
    /// The fixed enum size every gpt-image model accepts.
    enum_size: &'static str,
}

impl SizePlan {
    fn for_source(cfg: &Config, w: u32, h: u32) -> Self {
        Self::for_budget(w, h, cfg.openai_image_max_px)
    }

    fn for_budget(w: u32, h: u32, max_px: u32) -> Self {
        Self { flexible: flex_size(w, h, max_px), enum_size: pick_size(w, h) }
    }

    /// The size to request on the first attempt.
    fn try_first(&self) -> &str {
        self.flexible.as_deref().unwrap_or(self.enum_size)
    }
}

/// Largest flexible output size matching the source aspect, under the documented
/// gpt-image-2 constraints (verified 2026-07 API docs): both edges multiples of
/// 16, long edge ≤ 3840, long:short ratio ≤ 3:1, total pixels within
/// [655 360, 8 294 400] — further capped by the user's `max_px` budget. `None`
/// when no size satisfies all of that (caller uses the enum size).
fn flex_size(w: u32, h: u32, max_px: u32) -> Option<String> {
    const API_MIN_PX: f64 = 655_360.0;
    const API_MAX_PX: f64 = 8_294_400.0;
    const MAX_EDGE: f64 = 3840.0;
    if w == 0 || h == 0 {
        return None;
    }
    let budget = (max_px as f64).min(API_MAX_PX);
    if budget < API_MIN_PX {
        return None;
    }
    let r = (w as f64 / h as f64).clamp(1.0 / 3.0, 3.0);
    // Largest (ow, oh) with ow/oh = r and ow·oh = budget, then the edge cap.
    let mut oh = (budget / r).sqrt();
    let mut ow = r * oh;
    let scale = (MAX_EDGE / ow.max(oh)).min(1.0);
    ow *= scale;
    oh *= scale;
    // Round DOWN to ×16 — keeps every ≤ constraint satisfied except the
    // ratio: floored on its own, each edge loses up to 15 px, and at 3:1 the
    // short edge can lose proportionally more (3000×1000 floors to 2992×992,
    // which is 3.016:1, and the API answers such a size with a 400 after a
    // wasted round trip). So the long edge is capped at three times the
    // floored short edge afterwards — a multiple of 16 whenever the short
    // edge is one — which touches only the sizes that broke the bound and
    // can only shrink, so the edge cap and the pixel budget still hold.
    let ow = ((ow / 16.0).floor() * 16.0) as u32;
    let oh = ((oh / 16.0).floor() * 16.0) as u32;
    let (long, short) = (ow.max(oh).min(3 * ow.min(oh)), ow.min(oh));
    if short == 0 || long == 0 || (long as f64) * (short as f64) < API_MIN_PX {
        return None;
    }
    let (ow, oh) = if ow >= oh { (long, short) } else { (short, long) };
    Some(format!("{ow}x{oh}"))
}

/// Pick the supported gpt-image output size whose aspect best matches the source,
/// so we stop squashing every photo into a 1:1 square. Every gpt-image model
/// accepts exactly 1024×1024, 1536×1024 (landscape 3:2) and 1024×1536 (portrait
/// 2:3); newer models additionally take arbitrary sizes (see [`flex_size`]).
fn pick_size(w: u32, h: u32) -> &'static str {
    if h == 0 {
        return "1024x1024";
    }
    let r = w as f32 / h as f32;
    if r >= 1.2 {
        "1536x1024"
    } else if r <= 0.833 {
        "1024x1536"
    } else {
        "1024x1024"
    }
}

fn parse_size(s: &str) -> (u32, u32) {
    s.split_once('x')
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .unwrap_or((1024, 1024))
}

/// Blend `gen_img` into `base` ONLY where `mask` is transparent (alpha→0 =
/// regenerate), feathering the boundary so the seam is soft. All three share
/// dimensions. Untouched areas keep the original `base` pixels; only the
/// inpainted region carries generative pixels.
/// Test-facing wrapper over the mask→weight→blend chain: the production path
/// ([`retouch`]) stages the same pieces with explicit buffer lifetimes so a
/// 61 MP fill never holds every plane at once.
#[cfg(test)]
fn composite_region(
    base: &RgbaImage,
    gen_img: &RgbaImage,
    mask: &RgbaImage,
    feather: usize,
) -> RgbaImage {
    let (w, h) = base.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    // weight = 1 where mask is transparent (regenerate), 0 where opaque (keep base).
    let mut weight: Vec<f32> = mask.pixels().map(|p| 1.0 - p[3] as f32 / 255.0).collect();
    if feather > 0 {
        weight = box_blur(weight, wu, hu, feather);
    }
    let mut out = base.clone();
    // The blend takes RGB (alpha is never read); the test surface keeps its
    // RGBA-facing signature.
    let gen_rgb = RgbImage::from_fn(w, h, |x, y| {
        let p = gen_img.get_pixel(x, y);
        image::Rgb([p[0], p[1], p[2]])
    });
    composite_in_place(&mut out, &gen_rgb, &weight);
    out
}

/// Feather radius from the painted SELECTION's bounding box — an eighth of
/// its short side, at least 1 (see the weight-plane construction in
/// [`retouch`]). An empty selection returns usize::MAX so the caller's frame
/// rule stays the effective bound.
fn selection_feather(weight: &[f32], w: usize) -> usize {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for (i, &v) in weight.iter().enumerate() {
        if v > 0.0 {
            let (x, y) = (i % w, i / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x0 == usize::MAX {
        return usize::MAX;
    }
    let short = (x1 - x0 + 1).min(y1 - y0 + 1);
    (short / 8).max(1)
}

/// The blend pass over an owned base copy. Each pixel is written at most once,
/// so reading back from `out` is exact — no separate source plane needed (the
/// memory-staged full-res path in [`retouch`] rides on that).
fn composite_in_place(out: &mut RgbaImage, gen_img: &RgbImage, weight: &[f32]) {
    let (w, h) = out.dimensions();
    let wu = w as usize;
    for y in 0..h {
        for x in 0..w {
            let a = weight[(y as usize) * wu + x as usize].clamp(0.0, 1.0);
            if a <= 0.0001 {
                continue; // outside the (feathered) mask → keep the full-res original
            }
            let b = *out.get_pixel(x, y);
            let g = gen_img.get_pixel(x, y);
            // PREMULTIPLIED source-over (L09-5 + review R12-09): blending
            // straight RGB and alpha independently made a 1%-weight white
            // edge over a TRANSPARENT base come out (3,3,3,3) — nearly
            // black once composited downstream; the correct straight
            // result is (255,255,255,3). Generated content is opaque, so
            // the source term needs no unpremultiply, and out_a >= a > 0
            // here, so the division is safe. An OPAQUE base (ab = 1)
            // reduces to the old linear mix exactly — no rendering change
            // on normal photos.
            let ab = b[3] as f32 / 255.0;
            let out_a = a + (1.0 - a) * ab;
            let mix = |bc: u8, gc: u8| {
                let premul = a * gc as f32 + (1.0 - a) * bc as f32 * ab;
                (premul / out_a).round().clamp(0.0, 255.0) as u8
            };
            out.put_pixel(
                x,
                y,
                Rgba([
                    mix(b[0], g[0]),
                    mix(b[1], g[1]),
                    mix(b[2], g[2]),
                    (out_a * 255.0).round().clamp(0.0, 255.0) as u8,
                ]),
            );
        }
    }
}

/// Separable box blur with prefix sums — cost is O(w·h), independent of
/// `radius`, so a wide feather on a full-res frame stays cheap. Runs entirely
/// IN PLACE on the caller's plane: the only scratch is one row prefix and one
/// column prefix, so a 61 MP weight field no longer pays a second ~241 MB
/// plane (L02). Both passes are safe in place, and neither changes a value:
///   · horizontal — a row's whole prefix is read before that row is written,
///     and no other row is touched;
///   · vertical — a column's whole prefix is read before that column is
///     written, in the same y=0-upward accumulation order as before.
fn box_blur(mut src: Vec<f32>, w: usize, h: usize, radius: usize) -> Vec<f32> {
    if radius == 0 || w == 0 || h == 0 {
        return src;
    }
    let mut prefix = vec![0.0f32; w + 1];
    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            prefix[x + 1] = prefix[x] + src[row + x];
        }
        for x in 0..w {
            let lo = x.saturating_sub(radius);
            let hi = (x + radius + 1).min(w);
            src[row + x] = (prefix[hi] - prefix[lo]) / (hi - lo) as f32;
        }
    }
    let mut col = vec![0.0f32; h + 1];
    for x in 0..w {
        for y in 0..h {
            col[y + 1] = col[y] + src[y * w + x];
        }
        for y in 0..h {
            let lo = y.saturating_sub(radius);
            let hi = (y + radius + 1).min(h);
            src[y * w + x] = (col[hi] - col[lo]) / (hi - lo) as f32;
        }
    }
    src
}

pub(crate) fn encode_png(img: &DynamicImage) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .context("encode png")?;
    Ok(buf)
}

/// Is `declared` this endpoint's CAP on `expected` rather than a different
/// picture?
///
/// A subscription-tier endpoint (the local Codex bridge the GUI's OAuth mode
/// installs) honours the requested aspect and clamps the pixel count: 3520x2352
/// comes back as 1534x1025 — 1.4966:1 against 1.4966:1, measured 2026-08-30.
/// Smaller in BOTH axes at the same aspect is a resolution loss, which is what
/// that mode documents; anything else (a different aspect, a LARGER frame) is a
/// different picture and still refused, because the caller writes these bytes
/// out as the master.
const MIN_CAPPED_LONG_EDGE: u32 = 1024;

fn size_is_an_endpoint_cap(declared: (u32, u32), expected: (u32, u32)) -> bool {
    if declared.0 == 0 || declared.1 == 0 || expected.1 == 0 {
        return false;
    }
    if declared.0 >= expected.0 || declared.1 >= expected.1 {
        return false;
    }
    // A cap still has to be a usable picture. Without this floor the rule
    // admits ANY smaller frame at the same aspect — the existing contract test
    // caught it by handing 1x1 for a requested 1024x1024, which is not a tier
    // limit but a broken response. 1024 is the smallest edge the image models
    // document, so a genuine cap never falls below it.
    if declared.0.max(declared.1) < MIN_CAPPED_LONG_EDGE {
        return false;
    }
    let (a, b) = (
        f64::from(declared.0) / f64::from(declared.1),
        f64::from(expected.0) / f64::from(expected.1),
    );
    // Half a percent: enough for the rounding an integer clamp introduces
    // (1534/1025 vs 3520/2352 differ by 0.02%), far too tight to admit a
    // neighbouring aspect (3:2 vs 4:3 differ by 12%).
    (a - b).abs() <= b * 0.005
}

/// Returns the canonical PNG and the dimensions it ACTUALLY carries, which may
/// be an endpoint cap below `expected_size` (see [`size_is_an_endpoint_cap`]).
fn canonical_generated_png(bytes: &[u8], expected_size: &str) -> Result<(Vec<u8>, (u32, u32))> {
    let format = image::guess_format(bytes).context("identify generated image")?;
    if format != image::ImageFormat::Png {
        return Err(anyhow!(
            "image API returned {format:?}; the generated-image contract requires PNG"
        ));
    }
    let expected = parse_size(expected_size);
    // Dimensions from the HEADER, before any pixel decode: a tiny hostile
    // PNG can declare a ~480-megapixel frame, and fully decoding it just to
    // reject the mismatch cost hundreds of MiB per response (16-lane scan
    // L02/L16). The full decode below then runs only at the agreed size.
    let declared = image::ImageReader::with_format(
        std::io::Cursor::new(bytes),
        image::ImageFormat::Png,
    )
    .into_dimensions()
    .context("read generated PNG header")?;
    if declared != expected && !size_is_an_endpoint_cap(declared, expected) {
        return Err(anyhow!(
            "image API returned {}x{} for requested {}x{}",
            declared.0,
            declared.1,
            expected.0,
            expected.1
        ));
    }
    let accepted = declared;
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .context("fully decode generated PNG")?;
    // The header is not authority over the pixels: re-check the DECODED size
    // against what the header declared, so a frame that lies about itself is
    // refused even when the declared value passed the cap test above.
    if image.dimensions() != accepted {
        return Err(anyhow!(
            "image API returned {}x{} for requested {}x{}",
            image.width(),
            image.height(),
            expected.0,
            expected.1
        ));
    }
    if !matches!(
        &image,
        DynamicImage::ImageRgb8(_) | DynamicImage::ImageRgba8(_)
    ) {
        return Err(anyhow!(
            "image API returned {:?}; generated output must be 8-bit RGB or RGBA",
            image.color()
        ));
    }
    Ok((encode_png(&image)?, accepted))
}

/// RFC 2046 §5.1.1: the sender must pick a boundary that does not occur in
/// any encapsulated part. [`BOUNDARY`] alone cannot honour that — it is a
/// const in a PUBLIC repo, so a "style prompt" pasted from a stranger could
/// quote the literal delimiter line and forge parts of the user's own billed
/// request (override model / size / n). No privilege boundary is crossed —
/// it is the user's request on the user's key — but pasted text must not get
/// to rewrite the form.
///
/// ONE pass, never a candidate-rescan loop. The first version of this guard
/// appended one `-` per round and re-scanned every part each time, which is
/// quadratic in a value the attacker controls on BOTH factors: a 1 MiB prompt
/// ending in 4 000 dashes measured 7.2 s, and `serve`'s 256 MiB body cap
/// (`RetouchReq::prompt` carries no length cap of its own) extrapolates to
/// hours of spinning inside a request permit — the same defect class as the
/// XMP scanner's, re-introduced by the fix for a different one. Instead:
/// find the LONGEST run of `-` following any occurrence of the base, then
/// jump straight to one dash more. Nothing longer can occur by construction,
/// because such an occurrence would itself be a longer run.
///
/// Past RFC 2046's 70-character ceiling the request is REFUSED, not sent: a
/// boundary that long is invalid, so the server would reject it anyway, and
/// silently sending a body whose delimiter a part can forge is the one
/// outcome this function exists to prevent.
fn choose_boundary(parts: &[&[u8]]) -> Result<String> {
    const RFC2046_MAX: usize = 70;
    let base = BOUNDARY.as_bytes();
    let mut longest: Option<usize> = None;
    for p in parts {
        let mut from = 0;
        while from + base.len() <= p.len() {
            let Some(rel) = p[from..].windows(base.len()).position(|w| w == base) else { break };
            // Resume PAST this hit: `from` only advances, so every byte is
            // examined a bounded number of times and the whole scan is linear.
            from += rel + base.len();
            let run = p[from..].iter().take_while(|&&b| b == b'-').count();
            longest = Some(longest.map_or(run, |m| m.max(run)));
        }
    }
    let Some(run) = longest else { return Ok(String::from(BOUNDARY)) };
    let len = BOUNDARY.len() + run + 1;
    if len > RFC2046_MAX {
        return Err(anyhow!(
            "the prompt or image quotes this request's multipart delimiter followed by {run} \
             dashes, and separating them would need a {len}-character boundary (RFC 2046 allows \
             70) — refusing to send a request whose form fields the content could forge; remove \
             the long run of dashes from the prompt"
        ));
    }
    let mut b = String::with_capacity(len);
    b.push_str(BOUNDARY);
    for _ in 0..=run {
        b.push('-');
    }
    Ok(b)
}

fn part_text(buf: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    buf.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
            .as_bytes(),
    );
}

fn part_file(buf: &mut Vec<u8>, boundary: &str, name: &str, filename: &str, bytes: &[u8]) {
    buf.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .as_bytes(),
    );
    buf.extend_from_slice(bytes);
    buf.extend_from_slice(b"\r\n");
}

/// POST /images/edits, negotiating capability drift instead of hard-coding a
/// model list. Three request features are droppable, each at most once, on the
/// API's own 400 for *that specific parameter*:
///   * STREAMING (`stream` + `partial_images`) — the liveness channel: partial
///     frames keep proving the server is working, so the call runs under an
///     INACTIVITY deadline ([`IMAGES_EDIT_STALL_SECS`]) and a healthy long
///     generation is never killed mid-run. Models/bridges without image
///     streaming reject the parameter; the retry falls back to one blocking
///     POST under the legacy overall deadline.
///   * `input_fidelity` — a gpt-image-1.x knob; newer models (gpt-image-2)
///     reject it (`invalid_input_fidelity_model`).
///   * the FLEXIBLE `size` — a gpt-image-2 capability; older models reject a
///     non-enum size, so we retry with the fixed enum size from the [`SizePlan`].
///
/// Returns the image bytes and BOTH sizes (see [`GeneratedImage`]). `inputs` builds
/// the image (+ optional mask) PNG pair FOR a requested size string — called
/// once per size the negotiation attempts (memoize in the provider if
/// rebuilding is expensive).
type SizedInputs = (Vec<u8>, Option<Vec<u8>>);

/// One billed images/edits call's result.
///
/// The two sizes are DIFFERENT facts and are named apart because they were once
/// a single string, which is how a capped generation came to be measured against
/// pixels it was never sent:
///   * `requested` — the size the negotiation settled on. It selects which input
///     PNG went up the wire, so anything comparing the result to its INPUT must
///     use this one.
///   * `accepted` — the size the endpoint actually returned, which a
///     subscription tier can cap below the request. This is the size of the file
///     on disk, so anything reporting the result uses this one.
///
/// They are equal in the ordinary case.
struct GeneratedImage {
    bytes: Vec<u8>,
    requested: String,
    accepted: String,
}
fn call_images_edit(
    cfg: &Config,
    inputs: &mut dyn FnMut(&str) -> Result<SizedInputs>,
    prompt: &str,
    fidelity: &str,
    sizes: &SizePlan,
    quality: &str,
) -> Result<GeneratedImage> {
    let key = cfg
        .openai_api_key
        .as_ref()
        .ok_or_else(|| anyhow!("OPENAI_API_KEY not set — generative editing needs the OpenAI API"))?;
    // The subscription bridge (OAuth preset) serves OpenAI's own call, so a
    // template saved under the API preset waits there instead of being sent
    // to the bridge.
    if let Some(template) = cfg.image_gen_endpoint.as_ref().filter(|_| !cfg.image_is_oauth()) {
        return call_custom_edit(cfg, template, key, inputs, prompt, sizes.enum_size, quality);
    }

    let url = format!("{}/images/edits", cfg.openai_base_url.trim_end_matches('/'));
    let mut include_fidelity = true;
    let mut use_flexible = sizes.flexible.is_some();
    let mut use_stream = true;

    let (value, used_size): (serde_json::Value, String) = loop {
        if cancelled() {
            return Err(anyhow!("cancelled by user"));
        }
        let size = if use_flexible {
            sizes.flexible.as_deref().unwrap_or(sizes.enum_size)
        } else {
            sizes.enum_size
        };
        // The request's pixels are rebuilt AT the size being attempted
        // (L09-2): the enum retry used to re-send the flexible-ratio input
        // under an enum `size`, and the model's output — squeezed into an
        // aspect the input never had — was then stretched to the base ratio
        // on return: circles came back as ellipses on every panorama or
        // square crop whose ratio is not one of the three enum ratios. With
        // input ratio == requested ratio on every attempt, the return
        // path's exact resize is a true inverse.
        let (image_png, mask_png) = inputs(size)?;
        let image_png = image_png.as_slice();
        let mask_png = mask_png.as_deref();
        // Chosen over EVERY part THIS attempt's body carries — see
        // choose_boundary for why the const alone is not enough.
        let boundary = choose_boundary(&[
            cfg.openai_image_model.as_bytes(),
            prompt.as_bytes(),
            fidelity.as_bytes(),
            size.as_bytes(),
            quality.as_bytes(),
            image_png,
            mask_png.unwrap_or_default(),
        ])?;
        let build_body = |include_fidelity: bool, size: &str, stream: bool| -> Vec<u8> {
            let mut body = Vec::new();
            part_text(&mut body, &boundary, "model", &cfg.openai_image_model);
            part_text(&mut body, &boundary, "prompt", prompt);
            if include_fidelity {
                part_text(&mut body, &boundary, "input_fidelity", fidelity);
            }
            part_text(&mut body, &boundary, "size", size);
            part_text(&mut body, &boundary, "quality", quality);
            if stream {
                part_text(&mut body, &boundary, "stream", "true");
                part_text(&mut body, &boundary, "partial_images", &IMAGES_EDIT_PARTIALS.to_string());
            }
            part_file(&mut body, &boundary, "image", "image.png", image_png);
            if let Some(m) = mask_png {
                part_file(&mut body, &boundary, "mask", "mask.png", m);
            }
            body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
            body
        };
        let body = build_body(include_fidelity, size, use_stream);
        // Each retry in this loop re-posts, so every attempt gets its own full
        // budget. Streaming runs under an inactivity deadline (liveness is
        // observable); the blocking fallback keeps the overall deadline, the
        // only stall protection left when the server sends nothing until done.
        let req = if use_stream {
            crate::advisor::post_with_stall_timeout(
                &url,
                std::time::Duration::from_secs(IMAGES_EDIT_STALL_SECS),
            )
        } else {
            crate::advisor::post_with_timeout(
                &url,
                std::time::Duration::from_secs(IMAGES_EDIT_TIMEOUT_SECS),
            )
        };
        let started = std::time::Instant::now();
        let resp = req
            .set("Authorization", &format!("Bearer {key}"))
            .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
            .send_bytes(&body);
        match resp {
            Ok(r) => {
                // A server may accept `stream` yet answer with plain JSON —
                // dispatch on what actually came back, not on what was asked,
                // and when the header and the body disagree believe the BODY
                // (`read_image_reply`).
                let declared_sse = r.content_type().eq_ignore_ascii_case("text/event-stream");
                // Same number the socket's stall timeout was armed with: the
                // gate turns "byte-alive but eventless" (keep-alive comments)
                // into the same stall the socket calls silence.
                let read = read_image_reply(
                    declared_sse,
                    r.into_reader(),
                    Some(std::time::Duration::from_secs(
                        crate::advisor::effective_stall_secs(IMAGES_EDIT_STALL_SECS),
                    )),
                );
                match read {
                    Ok(v) => break (v, size.to_string()),
                    Err(e) => {
                        // NO re-POST past a 2xx: the endpoint accepted the
                        // request, so the generation (billed PER IMAGE) is
                        // already running or finished server-side — whether
                        // the body then failed to arrive (read) or to parse,
                        // a re-post buys a second image. The old retry did
                        // exactly that, and even used the advisor's 30 s
                        // text window rather than this module's own 3 s
                        // pre-response rule. Only the pre-response transport
                        // arm below may retry. Attribution reads the
                        // OUTERMOST context only (Display, not {:#}): model
                        // failures top out as "image stream error: …" and
                        // cancels as "cancelled by user" — both pass through
                        // with their own messages.
                        let top = e.to_string();
                        // ONE exception, found live: a streaming endpoint
                        // answers 200 and then REFUSES the parameters as an
                        // SSE error event — the same capability signal the
                        // 400 arm below negotiates on, but it used to fall
                        // through here as a terminal "image stream error",
                        // so the size fallback existed in one mode and not
                        // the other (`/api/retouch` died on `Invalid size
                        // '2048x1360'` while the blocking path would have
                        // retried at 1536x1024). A structured VALIDATION
                        // refusal that arrived BEFORE ANY PARTIAL FRAME
                        // means the generation never started, so a re-post
                        // cannot double-bill — and only that spelling
                        // matches the prefix (review R12-08: an error AFTER
                        // a partial reads "image stream error after N
                        // partial(s):", which the prefix rule rejects, so
                        // billable work is never re-POSTed). Same knobs,
                        // same one-shot flags as the 400 arm.
                        if use_flexible && streamed_refusal_blames(&top, "size") {
                            eprintln!(
                                "  note: {} rejected flexible size {size} in-stream — \
                                 falling back to {}",
                                cfg.openai_image_model, sizes.enum_size
                            );
                            use_flexible = false;
                            continue;
                        }
                        if include_fidelity && streamed_refusal_blames(&top, "input_fidelity") {
                            eprintln!(
                                "  note: {} rejected input_fidelity in-stream — retrying \
                                 without it",
                                cfg.openai_image_model
                            );
                            include_fidelity = false;
                            continue;
                        }
                        if top.starts_with("read image stream")
                            || top.starts_with("parse image API response")
                        {
                            let safe = crate::advisor::BoundedUntrustedText::diagnostic(
                                &top,
                                &[key],
                            );
                            return Err(anyhow!(
                                "{safe}: the request was accepted (2xx) and may already be billed — \
                                 not re-posting automatically; run the operation again to retry"
                            ));
                        }
                        let safe =
                            crate::advisor::BoundedUntrustedText::diagnostic(&top, &[key]);
                        return Err(anyhow!(safe.into_string()));
                    }
                }
            }
            Err(ureq::Error::Status(code, r)) => {
                let b = r.into_string().unwrap_or_default();
                // ONE attribution rule for every knob, shared with the text
                // path (advisor::error_blames_param): structured `error.param`
                // wins (exact or dotted child); when absent only a QUOTED
                // mention counts. The old bare-substring fallback here let any
                // negotiable status whose message merely CONTAINED "size" (a
                // common word) silently drop the flexible-size request — the
                // exact false-positive class the quoted rule was built for.
                // Negotiation stays limited to capability-shaped statuses
                // (400/404/422): a 401/429/5xx mentioning a parameter is not
                // a capability signal.
                let blames = |name: &str| crate::advisor::error_blames_param(&b, name);
                let negotiable = matches!(code, 400 | 404 | 422);
                // Each guard flips its own flag, so each retry fires at most once.
                if negotiable && use_stream && (blames("stream") || blames("partial_images")) {
                    eprintln!(
                        "  note: {} rejected streaming — retrying as one blocking call \
                         ({IMAGES_EDIT_TIMEOUT_SECS}s deadline)",
                        cfg.openai_image_model
                    );
                    gui_progress(&format!(
                        "{} rejected streaming — retrying as one blocking call (no progress \
                         events in this mode)",
                        cfg.openai_image_model
                    ));
                    use_stream = false;
                    continue;
                }
                if negotiable && include_fidelity && blames("input_fidelity") {
                    eprintln!(
                        "  note: {} rejected input_fidelity — retrying without it",
                        cfg.openai_image_model
                    );
                    include_fidelity = false;
                    continue;
                }
                if negotiable && use_flexible && blames("size") {
                    eprintln!(
                        "  note: {} rejected flexible size {size} — falling back to {}",
                        cfg.openai_image_model, sizes.enum_size
                    );
                    use_flexible = false;
                    continue;
                }
                let safe = crate::advisor::BoundedUntrustedText::diagnostic(&b, &[key]);
                return Err(anyhow!("image API {code}: {safe}"));
            }
            Err(ureq::Error::Transport(t)) => {
                let elapsed = started.elapsed().as_secs();
                // Even a fast failure can follow server acceptance: elapsed
                // time cannot establish that a paid generation was uncommitted.
                // Surface the ambiguity and let the user decide whether to retry.
                let mut msg = format!("transport: {t}");
                // A read timeout means different things per mode — and the
                // MEASURED elapsed time tells connection failures apart from
                // real stalls (both surface as "timed out reading response").
                if msg.contains("timed out") {
                    // The EFFECTIVE stall (env override included) — blaming
                    // the default budget the user had overridden misdiagnosed
                    // which side gave up.
                    let stall = crate::advisor::effective_stall_secs(IMAGES_EDIT_STALL_SECS);
                    if use_stream && elapsed + 30 < stall {
                        msg.push_str(&format!(
                            " (failed after {elapsed}s — well before the {stall}s stall \
                             budget, so this is a connection/handshake/proxy failure, not a \
                             slow generation)"
                        ));
                    } else if use_stream {
                        msg.push_str(&format!(
                            " (no stream activity for ~{elapsed}s — the server or a \
                             proxy stopped sending; healthy generations stream partial images and \
                             are not time-capped. Raise AUTOSHADE_HTTP_TIMEOUT_SECS if a proxy \
                             buffers server-sent events, or lower AUTOSHADE_IMAGE_QUALITY / \
                             AUTOSHADE_IMAGE_MAX_PX)"
                        ));
                    } else {
                        msg.push_str(&format!(
                            " (hit the HTTP deadline, default {IMAGES_EDIT_TIMEOUT_SECS}s — \
                             large/high-quality generations can run longer; raise \
                             AUTOSHADE_HTTP_TIMEOUT_SECS, or lower AUTOSHADE_IMAGE_QUALITY / \
                             AUTOSHADE_IMAGE_MAX_PX to speed the call up)"
                        ));
                    }
                }
                return Err(anyhow!(msg));
            }
        }
    };

    if let Some(u) = value.get("usage") {
        let safe = crate::advisor::BoundedUntrustedText::diagnostic(&u.to_string(), &[key]);
        eprintln!("  usage: {safe}");
    }
    let b64 = extract_b64(&value).ok_or_else(|| {
        let safe =
            crate::advisor::BoundedUntrustedText::diagnostic(&value.to_string(), &[key]);
        anyhow!("no image payload (b64_json) in response: {safe}")
    })?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .context("decode b64_json")?;
    let (bytes, accepted) = canonical_generated_png(&bytes, &used_size)?;
    let accepted_size = format!("{}x{}", accepted.0, accepted.1);
    if accepted_size != used_size {
        // Say it once, plainly: the file on disk is this size, not the one that
        // was asked for. Reporting the REQUESTED size here would put a wrong
        // number in the terminal line and in the caller's own record.
        let line = format!(
            "  note: {} capped this generation at {accepted_size} (asked for {used_size})",
            cfg.openai_image_model
        );
        eprintln!("{line}");
        gui_progress(line.trim());
    }
    Ok(GeneratedImage { bytes, requested: used_size, accepted: accepted_size })
}

/// [`call_images_edit`] for a user-described endpoint (`image_endpoint`, e.g.
/// Venice's `/image/multi-edit`): one blocking JSON POST at the plan's ENUM
/// size. Such endpoints pick their own output resolution (Venice: ~1 MP
/// unless asked otherwise), and the enum input keeps the base64 body far under
/// the per-image upload limits they set (Venice: 25 MB). Nothing is
/// negotiated — the template IS the request — and nothing is re-posted past a
/// 2xx, the same money rule as the OpenAI path.
fn call_custom_edit(
    cfg: &Config,
    template: &str,
    key: &str,
    inputs: &mut dyn FnMut(&str) -> Result<SizedInputs>,
    prompt: &str,
    size: &str,
    quality: &str,
) -> Result<GeneratedImage> {
    use crate::image_endpoint as ep_mod;
    let ep = ep_mod::ImageEndpoint::parse(template)
        .context("the custom image-gen endpoint in Settings")?;
    if cancelled() {
        return Err(anyhow!("cancelled by user"));
    }
    let (image_png, mask_png) = inputs(size)?;
    if mask_png.is_some() && !ep.takes_mask() {
        let line = "  note: the custom image endpoint takes no mask — the model gets the words and the \
                    whole picture; only the painted area of its answer is kept";
        eprintln!("{line}");
        gui_progress(line.trim());
    }
    let (w, h) = parse_size(size);
    let body = ep_mod::render_body(&ep, &ep_mod::Values {
        model: &cfg.openai_image_model,
        prompt,
        quality,
        width: w,
        height: h,
        image_png: &image_png,
        mask_png: mask_png.as_deref(),
    })?;
    let resp = crate::advisor::post_with_timeout(
        &ep.url(&cfg.openai_base_url),
        std::time::Duration::from_secs(IMAGES_EDIT_TIMEOUT_SECS),
    )
    .set("Authorization", &format!("Bearer {key}"))
    .send_json(body);
    let reply = match resp {
        Ok(r) => {
            use std::io::Read as _;
            let mut buf = Vec::new();
            let read = r.into_reader().take(STREAM_CAP + 1).read_to_end(&mut buf);
            if read.is_err() || buf.len() as u64 > STREAM_CAP {
                return Err(anyhow!(
                    "read image API response failed: the request was accepted (2xx) and may already \
                     be billed — not re-posting automatically; run the operation again to retry"
                ));
            }
            buf
        }
        Err(ureq::Error::Status(code, r)) => {
            let b = r.into_string().unwrap_or_default();
            let safe = crate::advisor::BoundedUntrustedText::diagnostic(&b, &[key]);
            return Err(anyhow!("image API {code}: {safe}"));
        }
        Err(ureq::Error::Transport(t)) => return Err(anyhow!("transport: {t}")),
    };
    let bytes = ep_mod::reply_bytes(&ep, reply).map_err(|e| {
        anyhow!(crate::advisor::BoundedUntrustedText::diagnostic(&format!("{e:#}"), &[key]).into_string())
    })?;
    let (png, got) = ep_mod::to_requested_size(&bytes, (w, h))?;
    if got != (w, h) {
        let line = format!(
            "  note: {} answered {}x{}; resized to the requested {size}",
            cfg.openai_image_model, got.0, got.1
        );
        eprintln!("{line}");
        gui_progress(line.trim());
    }
    Ok(GeneratedImage { bytes: png, requested: size.to_string(), accepted: size.to_string() })
}

/// The image payload lives at `data[0].b64_json` in a blocking JSON response
/// but at the TOP level of a streaming `*.completed` event (verified against
/// the published response schemas) — accept both shapes.
fn extract_b64(value: &serde_json::Value) -> Option<&str> {
    value
        .get("data")
        .and_then(|d| d.get(0))
        .and_then(|x| x.get("b64_json"))
        .or_else(|| value.get("b64_json"))
        .and_then(|s| s.as_str())
}

/// Does a STREAMED image failure (`read_sse_image`'s "image stream error:"
/// message, whose payload is the SSE error event verbatim) blame `param` as a
/// structured validation refusal? Attribution reuses the one shared rule
/// (`advisor::error_blames_param`): a structured `error.param` wins, and only
/// a QUOTED mention counts as a fallback — a message merely containing a
/// common word like "size" is not a capability signal. Only messages carrying
/// the exact prefix qualify; read/parse failures AND post-partial stream
/// errors ("image stream error after N partial(s):", review R12-08) never
/// negotiate — both may follow a billed generation.
fn streamed_refusal_blames(top: &str, param: &str) -> bool {
    top.strip_prefix("image stream error: ")
        .is_some_and(|payload| crate::advisor::error_blames_param(payload, param))
}

/// How far into the body the lead-byte sniff may scan before giving up and
/// letting the SSE reader have it. An event stream legitimately opens with a
/// blank line or a `:` comment; nothing legitimate opens with kilobytes of
/// whitespace, and the scan must not become a second way to hang.
const LEAD_SNIFF_LIMIT: usize = 64;

/// Read one images/edits reply, believing the BODY over the `Content-Type`.
///
/// `declared_sse` is what the header claimed. A proxy or subscription bridge in
/// front of the API may claim `text/event-stream` and then send the ordinary
/// JSON reply (measured against the local Codex bridge the GUI's OAuth mode
/// installs). Trusting the header there fails the call with "ended without a
/// completed event" on a generation the server already produced — and the
/// no-re-POST-past-2xx rule means it cannot be recovered. `{` never opens an
/// SSE frame (those start with a field name or a `:` comment), so one sniffed
/// byte separates the two without weakening the stream path: a real event
/// stream still goes to [`read_sse_image`], keeping partial-frame progress and
/// the in-stream refusal negotiation that reads its error spelling.
fn read_image_reply(
    declared_sse: bool,
    r: impl std::io::Read,
    progress_budget: Option<std::time::Duration>,
) -> Result<serde_json::Value> {
    if !declared_sse {
        return image_reply_as_json(r);
    }
    let (lead, r) = sniff_lead_byte(r)?;
    if lead == Some(b'{') {
        return image_reply_as_json(r);
    }
    read_sse_image(r, progress_budget)
}

/// The whole body as one JSON value.
///
/// Capped like the SSE arm beside it — with the SAME number: an uncapped read
/// runs until the server stops, and an allocation failure aborts the process
/// rather than raising an error. The advisor's own 64 MiB is a TEXT-response
/// budget; one full-size base64 image frame is about that on its own, so
/// borrowing it here threw away already-billed generations.
///
/// A generic fn rather than a closure inside [`read_image_reply`]: the two call
/// sites pass different reader types (the raw body, and the body with its
/// sniffed lead bytes chained back on), and a closure would be monomorphised on
/// whichever came first.
fn image_reply_as_json(r: impl std::io::Read) -> Result<serde_json::Value> {
    crate::advisor::json_from_reader_capped(r, STREAM_CAP)
        .map_err(anyhow::Error::from)
        .context("parse image API response")
}

/// A reader with the bytes examined by [`sniff_lead_byte`] chained back in front.
type SniffedReader<R> = std::io::Chain<std::io::Cursor<Vec<u8>>, R>;

/// The first non-whitespace byte of `r`, and a reader that still yields it.
///
/// Nothing is consumed from the caller's point of view: the bytes read while
/// looking are chained back in front of the remainder.
fn sniff_lead_byte<R: std::io::Read>(
    mut r: R,
) -> Result<(Option<u8>, SniffedReader<R>)> {
    // `Cursor<Vec<u8>>` is a concrete type, so unlike a generic parameter with
    // a `Read` bound it needs the trait in scope — without this, `.chain`
    // resolves to `Iterator::chain` and the file does not compile.
    use std::io::Read as _;
    let mut seen: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    while seen.len() < LEAD_SNIFF_LIMIT {
        match r.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                seen.push(byte[0]);
                if !byte[0].is_ascii_whitespace() {
                    break;
                }
            }
            Err(e) => return Err(anyhow::Error::from(e)).context("read image reply"),
        }
    }
    let lead = seen.iter().copied().find(|b| !b.is_ascii_whitespace());
    Ok((lead, std::io::Cursor::new(seen).chain(r)))
}

/// Drain an image SSE stream: log partial-image events (the liveness signal),
/// fail loudly on an `error` event, and return the final `*.completed` JSON
/// payload. Matches on the event-type SUFFIX so both the `image_edit.*` and
/// `image_generation.*` families parse. Framing (multi-line `data:` payloads,
/// event boundaries, EOF flush, `[DONE]`) lives in the shared
/// `advisor::for_each_sse_json` — one SSE implementation for every stream.
fn read_sse_image(
    r: impl std::io::Read,
    progress_budget: Option<std::time::Duration>,
) -> Result<serde_json::Value> {
    use std::ops::ControlFlow::{Break, Continue};
    let mut partials = 0u32;
    let mut completed: Option<serde_json::Value> = None;
    let mut failure: Option<String> = None;
    let mut was_cancelled = false;
    crate::advisor::for_each_sse_json(r, STREAM_CAP, progress_budget, |v| {
        if cancelled() {
            // Cooperative stop between events — the cheapest safe point to
            // abandon a stream (the job is already billed server-side; this
            // just stops the download and frees the worker).
            was_cancelled = true;
            return Break(());
        }
        if v.get("error").is_some() || v.get("type").and_then(|t| t.as_str()) == Some("error") {
            failure = Some(v.to_string());
            return Break(());
        }
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if ty.ends_with(".partial_image") {
            partials += 1;
            eprintln!("  … streaming: partial image {partials} received (generation alive)");
            gui_progress(&format!(
                "streaming: partial image {partials} received (generation alive)"
            ));
        } else if ty.ends_with(".completed") {
            completed = Some(v);
            return Break(());
        }
        Continue(())
    })
    .context("read image stream")?;
    if was_cancelled {
        return Err(anyhow!("cancelled by user"));
    }
    if let Some(f) = failure {
        // The partial count decides negotiability (review R12-08): a partial
        // frame proves the generation was ALIVE — and billable — before the
        // error, so the caller must never re-POST then. Encoding the count
        // into the message PREFIX makes the existing prefix rule do the
        // gating: `streamed_refusal_blames` only matches the exact
        // zero-partial spelling ("prefix required = billing safety").
        if partials > 0 {
            return Err(anyhow!("image stream error after {partials} partial(s): {f}"));
        }
        return Err(anyhow!("image stream error: {f}"));
    }
    completed.ok_or_else(|| {
        anyhow!("image stream ended without a completed event ({partials} partial(s) received)")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    /// M-S5: the endpoint-cap rule widens, and a DIFFERENT picture starts being
    /// accepted as the master. It must admit exactly one shape — smaller in both
    /// axes, same aspect, still a usable frame — because `reimagine` writes these
    /// bytes straight out as the output file.
    #[test]
    fn only_a_same_aspect_usable_shrink_counts_as_an_endpoint_cap() {
        // Measured live 2026-08-30 against the local Codex bridge the GUI's
        // OAuth mode installs: 3520x2352 asked, 1534x1025 returned.
        assert!(size_is_an_endpoint_cap((1534, 1025), (3520, 2352)));
        // The square tier answer to a square request is the same story.
        assert!(size_is_an_endpoint_cap((1254, 1254), (2048, 2048)));

        // A different aspect is a different picture, however much smaller.
        assert!(!size_is_an_endpoint_cap((1536, 1152), (3520, 2352)), "4:3 for a 3:2 request");
        // Larger in either axis is never a cap.
        assert!(!size_is_an_endpoint_cap((3520, 2352), (3520, 2352)), "equal is not a cap");
        assert!(!size_is_an_endpoint_cap((4096, 2737), (3520, 2352)), "larger");
        assert!(!size_is_an_endpoint_cap((1534, 2352), (3520, 2352)), "one axis only");
        // Degenerate frames are refused, not treated as an extreme cap.
        assert!(!size_is_an_endpoint_cap((1, 1), (1024, 1024)), "1x1 is a broken reply");
        assert!(!size_is_an_endpoint_cap((0, 0), (1024, 1024)), "zero");
        // …and the floor is exactly where it says it is.
        let long = MIN_CAPPED_LONG_EDGE;
        assert!(size_is_an_endpoint_cap((long, long), (long * 2, long * 2)), "at the floor");
        assert!(
            !size_is_an_endpoint_cap((long - 1, long - 1), (long * 2, long * 2)),
            "one below the floor"
        );
    }

    /// M-S6: `requested` and `accepted` are different facts. Swapping them
    /// compiles, reads plausibly, and silently measures a capped generation
    /// against an input the model never received — the defect this batch's
    /// first cut shipped. Driven over the real loopback transport so both
    /// fields come out of one billed call.
    #[test]
    fn a_capped_return_keeps_the_requested_and_the_delivered_size_apart() {
        // Same aspect as the request (2048/1168 == 1024/584) with its long edge
        // at the usable floor: the shape a subscription tier actually returns.
        let capped = encode_png(&DynamicImage::ImageRgba8(RgbaImage::new(1024, 584))).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&capped);
        let (url, _seen, handle) = stub_endpoint(vec![(
            200,
            "application/json",
            format!(r#"{{"data":[{{"b64_json":"{b64}"}}]}}"#),
        )]);
        let cfg = stub_cfg(url);
        let src = encode_png(&DynamicImage::ImageRgba8(RgbaImage::new(8, 8))).unwrap();
        let sizes = SizePlan { flexible: Some("2048x1168".into()), enum_size: "1024x1024" };
        let mut asked: Vec<String> = Vec::new();
        let got = call_images_edit(
            &cfg,
            &mut |size| {
                asked.push(size.to_string());
                Ok((src.clone(), None))
            },
            "brighten the sky",
            "high",
            &sizes,
            "auto",
        )
        .expect("a same-aspect cap is accepted, not refused as a wrong picture");
        // The FLEXIBLE input is what went up the wire, so the fidelity reading
        // must compare against those bytes: `requested` may not carry the cap.
        assert_eq!(asked, vec!["2048x1168".to_string()], "the cap is not a size refusal");
        assert_eq!(got.requested, "2048x1168", "the size the model was sent");
        assert_eq!(got.accepted, "1024x584", "the size that came back — the file on disk");

        let grace = std::time::Instant::now();
        while !handle.is_finished() && grace.elapsed() < std::time::Duration::from_secs(2) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = handle.join();
    }

    /// M-S1: the header is trusted alone again, or the lead-byte sniff is
    /// weakened — a body that is plain JSON must parse even when the
    /// `Content-Type` claims an event stream. Captured live 2026-08-30 from the
    /// local Codex bridge the GUI's OAuth mode installs: it labels the response
    /// `text/event-stream` and then sends the ordinary reply, with zero `event:`
    /// lines. Trusting the label failed the call on a generation the
    /// subscription had already been charged for, and the no-re-POST-past-2xx
    /// rule meant it could not be recovered.
    #[test]
    fn a_json_body_mislabelled_as_an_event_stream_is_still_read() {
        let body = br#"{"created":1788118129,"background":"opaque","data":[{"b64_json":"QUJD"}]}"#;
        let v = read_image_reply(true, &body[..], None).expect("a JSON body must parse");
        assert_eq!(v["data"][0]["b64_json"], "QUJD");

        // Leading whitespace is not a disguise: the sniff skips it.
        let padded = [b"\r\n\r\n  ".as_slice(), body].concat();
        let v = read_image_reply(true, padded.as_slice(), None).expect("leading blank lines");
        assert_eq!(v["data"][0]["b64_json"], "QUJD");

        // And the honest label keeps working.
        let v = read_image_reply(false, &body[..], None).expect("declared JSON");
        assert_eq!(v["data"][0]["b64_json"], "QUJD");
    }

    /// M-S2: the sniff swallows a REAL stream — an event stream must still go
    /// to the SSE reader, keeping its completed event. Guards the fix from
    /// buying the JSON case by breaking the streaming case.
    #[test]
    fn a_real_event_stream_still_reaches_the_streaming_reader() {
        let sse = b"event: image_edit.completed\n\
                    data: {\"type\":\"image_edit.completed\",\"b64_json\":\"QUJD\"}\n\n";
        let v = read_image_reply(true, &sse[..], None).expect("a real stream must complete");
        assert_eq!(v["b64_json"], "QUJD");

        // A stream that opens with a comment line still is not JSON.
        let commented = b": keep-alive\n\
                          event: image_edit.completed\n\
                          data: {\"type\":\"image_edit.completed\",\"b64_json\":\"WFla\"}\n\n";
        let v = read_image_reply(true, &commented[..], None).expect("comment-led stream");
        assert_eq!(v["b64_json"], "WFla");
    }

    /// M-S3: the in-stream refusal spelling is the one `streamed_refusal_blames`
    /// negotiates on, and the sniff must not change it. If this drifts, a
    /// flexible-size refusal stops falling back in streaming mode while the
    /// blocking mode still does — the asymmetry the prefix rule exists to
    /// prevent — or worse, a BILLED generation gets re-POSTed.
    #[test]
    fn a_streamed_refusal_keeps_its_negotiable_spelling_through_the_sniff() {
        let refusal = b"event: error\n\
                        data: {\"type\":\"error\",\"error\":{\"param\":\"size\",\
                        \"message\":\"Invalid size '2048x1360'.\"}}\n\n";
        let e = read_image_reply(true, &refusal[..], None).expect_err("a refusal must fail");
        let top = e.to_string();
        assert!(streamed_refusal_blames(&top, "size"), "not negotiable: {top}");
        assert!(!top.contains("partial(s):"), "zero partials must not read as billed: {top}");
    }

    /// M-S4: the sniff must never become a way to hang or to eat a body. It
    /// gives up after `LEAD_SNIFF_LIMIT` bytes, and every byte it looked at is
    /// handed back to whichever reader wins.
    #[test]
    fn the_lead_sniff_is_bounded_and_puts_back_every_byte_it_read() {
        let whitespace = vec![b' '; LEAD_SNIFF_LIMIT * 4];
        let (lead, mut r) = sniff_lead_byte(whitespace.as_slice()).expect("bounded scan");
        assert_eq!(lead, None, "all-whitespace has no lead byte");
        let mut back = Vec::new();
        std::io::Read::read_to_end(&mut r, &mut back).expect("readable");
        assert_eq!(back.len(), whitespace.len(), "the sniff must not eat bytes");

        let body = b"   {\"a\":1}";
        let (lead, mut r) = sniff_lead_byte(&body[..]).expect("sniff");
        assert_eq!(lead, Some(b'{'));
        let mut back = Vec::new();
        std::io::Read::read_to_end(&mut r, &mut back).expect("readable");
        assert_eq!(back, body, "the chained reader must reproduce the body exactly");
    }

    /// M-G1: `streamed_refusal_blames` prefix check dropped or attribution
    /// weakened — the negotiation must fire on a STREAMED structured size
    /// refusal (the live `/api/retouch` failure, payload verbatim) and must
    /// NOT fire on read/parse failures or unquoted mentions, which may sit
    /// downstream of an already-billed generation.
    #[test]
    fn a_streamed_size_refusal_negotiates_and_a_billed_failure_does_not() {
        // Captured live 2026-08-07: the API refusing a flexible size in-stream.
        let live = r#"image stream error: {"error":{"code":"invalid_value","message":"Invalid size '2048x1360'. Supported sizes are 1024x1024, 1024x1536, 1536x1024, and auto.","param":"size","type":"image_generation_user_error"},"sequence_number":0,"type":"error"}"#;
        assert!(streamed_refusal_blames(live, "size"));
        assert!(!streamed_refusal_blames(live, "input_fidelity"));

        // A stream that died mid-read is NOT a refusal — no negotiation, the
        // caller's "may already be billed" arm owns it.
        assert!(!streamed_refusal_blames("read image stream: connection reset", "size"));
        // …even when the failure text QUOTES the parameter name: without the
        // prefix requirement this would negotiate — and re-post — after a
        // possibly-billed generation.
        assert!(!streamed_refusal_blames(
            r#"read image stream: server closed after "size" line"#,
            "size"
        ));
        // An unquoted mention of a common word is not a capability signal.
        assert!(!streamed_refusal_blames(
            r#"image stream error: {"message":"the size of the queue exceeded a limit"}"#,
            "size"
        ));
    }

    /// The boundary is a const in a PUBLIC repo, so a pasted prompt could
    /// quote the delimiter line and forge its own parts (override model /
    /// size / n on the user's bill). RFC 2046's own rule is the fix: the
    /// chosen boundary must not occur in any part.
    #[test]
    fn a_prompt_quoting_the_boundary_cannot_forge_a_part() {
        let evil = format!(
            "nice sunset--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"n\"\r\n\r\n10\r\n"
        );
        let b = choose_boundary(&[evil.as_bytes()]).expect("one extra dash is well inside RFC 2046");
        assert_ne!(b, BOUNDARY, "a value carrying the const boundary forces a different one");
        assert!(!evil.contains(&b), "the chosen boundary occurs in no part");
        // Serialized: the delimiter appears exactly once (our header), never
        // a second time from inside the hostile value.
        let mut buf = Vec::new();
        part_text(&mut buf, &b, "prompt", &evil);
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(
            s.matches(&format!("--{b}")).count(),
            1,
            "one opening delimiter and no forged one: {s}"
        );
        // An honest request keeps the well-known boundary unchanged.
        assert_eq!(
            choose_boundary(&[b"a plain prompt", &[0x89, b'P', b'N', b'G']]).unwrap(),
            BOUNDARY
        );

        // The FIRST version of this guard appended one dash per round and
        // re-scanned every part each time — quadratic in a value the attacker
        // supplies, and `RetouchReq::prompt` has no length cap under `serve`'s
        // 256 MiB body ceiling. This shape took 7.2 s at 1 MiB; it must now be
        // one linear pass, and the dash run must be jumped in ONE step.
        let long = format!("{}{}{}", "x".repeat(1 << 20), BOUNDARY, "-".repeat(4000));
        let started = std::time::Instant::now();
        let b = choose_boundary(&[long.as_bytes()]);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "choose_boundary is not linear: {:?} for a 1 MiB part",
            started.elapsed()
        );
        // 4000 dashes cannot be separated inside RFC 2046's 70-character
        // ceiling, so the request is REFUSED rather than sent with a
        // delimiter the prompt can forge — or with an invalid boundary.
        let e = b.expect_err("a 4036-character boundary is not a legal request").to_string();
        assert!(e.contains("RFC 2046") || e.contains("70"), "the refusal says why: {e}");

        // A short run is separated in one jump, not one dash per round.
        let short = format!("{BOUNDARY}---");
        assert_eq!(choose_boundary(&[short.as_bytes()]).unwrap(), format!("{BOUNDARY}----"));
    }

    #[test]
    fn pick_size_matches_orientation() {
        assert_eq!(pick_size(6000, 4000), "1536x1024"); // 3:2 landscape
        assert_eq!(pick_size(4000, 6000), "1024x1536"); // 2:3 portrait
        assert_eq!(pick_size(4000, 4000), "1024x1024"); // square
        assert_eq!(pick_size(4000, 0), "1024x1024"); // divide-by-zero guard
    }

    #[test]
    fn parse_size_roundtrips_and_falls_back() {
        assert_eq!(parse_size("1536x1024"), (1536, 1024));
        assert_eq!(parse_size("1024x1536"), (1024, 1536));
        assert_eq!(parse_size("garbage"), (1024, 1024));
    }

    #[test]
    fn flex_size_respects_every_documented_constraint() {
        // Every produced size must satisfy: edges ×16, long edge ≤3840,
        // ratio ≤3:1, 655 360 ≤ area ≤ min(budget, 8 294 400).
        let check = |w: u32, h: u32, budget: u32| -> Option<(u32, u32)> {
            let s = flex_size(w, h, budget)?;
            let (ow, oh) = parse_size(&s);
            assert_eq!(ow % 16, 0, "{s}: width ×16");
            assert_eq!(oh % 16, 0, "{s}: height ×16");
            assert!(ow.max(oh) <= 3840, "{s}: long edge");
            assert!(ow.max(oh) as f64 / ow.min(oh) as f64 <= 3.0 + 1e-6, "{s}: ratio");
            let area = ow as u64 * oh as u64;
            assert!(area >= 655_360, "{s}: area ≥ API min");
            assert!(area <= (budget as u64).min(8_294_400), "{s}: area ≤ budget");
            Some((ow, oh))
        };
        // 3:2 landscape at the full budget → ~8.2 MP, 5× the 1536×1024 enum.
        let (ow, oh) = check(6000, 4000, u32::MAX).unwrap();
        assert!(ow as u64 * oh as u64 > 8_000_000, "full budget should near the API max");
        assert!(ow > oh, "landscape stays landscape");
        // Portrait mirrors it.
        let (pw, ph) = check(4000, 6000, u32::MAX).unwrap();
        assert_eq!((pw, ph), (oh, ow));
        // Square hits the max exactly (2880² = 8 294 400).
        assert_eq!(flex_size(4000, 4000, u32::MAX).as_deref(), Some("2880x2880"));
        // Extreme pano is clamped to 3:1 and the 3840 edge.
        check(12_000, 3_000, u32::MAX).unwrap();
        // A tighter budget is honoured.
        let (bw, bh) = check(6000, 4000, 2_000_000).unwrap();
        assert!((bw as u64 * bh as u64) <= 2_000_000);
        // Below the API minimum → no flexible size (enum fallback).
        assert_eq!(flex_size(6000, 4000, 100_000), None);
        assert_eq!(flex_size(0, 4000, u32::MAX), None);
    }

    /// The 3:1 bound holds for EVERY aspect and budget, exactly. Flooring the
    /// two edges to ×16 independently let a source at or beyond 3:1 come out
    /// over it — 3000×1000 became 2992×992 = 3.016:1 — and the API answers
    /// such a size with a 400 after a wasted round trip.
    ///
    /// MUTATION: drop the `.min(3 * ow.min(oh))` cap on the long edge in
    /// `flex_size` and the 3 MP budget at 3:1 fails by name (2992×992).
    #[test]
    fn flex_size_never_exceeds_the_ratio_bound_for_any_aspect_or_budget() {
        // The shipped defect, stated first so the sweep has a shape to differ
        // from: 3:1 at a 3 MP budget floors to 992 short, and 3 × 992 = 2976.
        assert_eq!(flex_size(3000, 1000, 3_000_000).as_deref(), Some("2976x992"));
        assert_eq!(flex_size(1000, 3000, 3_000_000).as_deref(), Some("992x2976"));
        // Aspects from 1:16 to 16:1 in odd strides (so the clamp and both
        // orientations are crossed), budgets from below the API minimum to
        // past its maximum in a prime stride (so the ×16 floors land on every
        // phase).
        let mut produced = 0usize;
        for a in (100u32..=400).step_by(37) {
            for b in (100u32..=1600).step_by(53) {
                for budget in (600_000u32..=9_000_000).step_by(97_003) {
                    for (w, h) in [(b, a), (a, b)] {
                        let Some(s) = flex_size(w, h, budget) else { continue };
                        produced += 1;
                        let (ow, oh) = parse_size(&s);
                        let (lo, hi) = (ow.min(oh) as u64, ow.max(oh) as u64);
                        assert!(hi <= 3 * lo, "{w}x{h} @ {budget}: {s} exceeds 3:1");
                        assert_eq!(ow % 16, 0, "{s}: width ×16");
                        assert_eq!(oh % 16, 0, "{s}: height ×16");
                        assert!(hi <= 3840, "{s}: long edge");
                        assert!(lo * hi >= 655_360, "{s}: area ≥ API min");
                        assert!(lo * hi <= (budget as u64).min(8_294_400), "{s}: area ≤ budget");
                        // Rounding may square a near-square source, never
                        // turn a landscape into a portrait or back.
                        assert!(
                            if w >= h { ow >= oh } else { ow <= oh },
                            "{w}x{h}: {s} flipped the orientation"
                        );
                    }
                }
            }
        }
        assert!(produced > 1000, "sweep non-vacuity: {produced} sizes checked");
    }

    /// v1.2.2: the size plan follows the frame that is SENT. A body set to a
    /// 4:3 aspect writes a 4:3 embedded preview over its 3:2 sensor; the plan
    /// used to be sized from that preview while the pixels sent were the 3:2
    /// develop, `resize_exact`-ed into 4:3 — and the generated target came
    /// back 4:3 against the 3:2 develop the reverse-fit pairs it with.
    /// `develop_is_sent` is the other half: a preview that is not the sensor
    /// frame is never the input, however large it is.
    #[test]
    fn the_size_plan_follows_the_sent_frame_not_the_cropped_preview() {
        // Sized from the 3:2 sensor frame, as the develop is sent.
        let plan = SizePlan::for_budget(6000, 4000, u32::MAX);
        let (sw, sh) = parse_size(plan.try_first());
        assert!((sw as f64 / sh as f64 - 1.5).abs() < 0.02, "{sw}x{sh} is not 3:2");
        // Sized from the 4:3 preview instead, the same budget lands on 4:3 —
        // the shipped defect, stated so the fix has a shape to differ from.
        let (cw, ch) = parse_size(SizePlan::for_budget(1440, 1080, u32::MAX).try_first());
        assert!((cw as f64 / ch as f64 - 4.0 / 3.0).abs() < 0.02, "{cw}x{ch} is not 4:3");
        // A 4:3 preview over a 3:2 frame: the develop is sent even when the
        // preview would outresolve the plan.
        assert!(develop_is_sent(true, (1536, 1024), (1600, 1200), (6000, 4000)));
        // The usual case: a preview in the frame's aspect, outresolved.
        assert!(develop_is_sent(true, (sw, sh), (1616, 1080), (6000, 4000)));
        // Same aspect and not outresolved: the preview is the input.
        assert!(!develop_is_sent(true, (1536, 1024), (1616, 1080), (6000, 4000)));
        // A baked image is its own frame.
        assert!(!develop_is_sent(false, (sw, sh), (1440, 1080), (1440, 1080)));
    }

    /// L02: the in-place box blur must be BIT-identical to the previous
    /// two-plane implementation, which is inlined here as the oracle — a
    /// memory optimisation that shifts a seam by one LSB is a rendering
    /// change, not an optimisation.
    #[test]
    fn in_place_box_blur_is_bit_identical_to_the_two_plane_form() {
        fn two_plane(src: Vec<f32>, w: usize, h: usize, radius: usize) -> Vec<f32> {
            if radius == 0 || w == 0 || h == 0 {
                return src;
            }
            let mut tmp = vec![0.0f32; src.len()];
            let mut prefix = vec![0.0f32; w + 1];
            for y in 0..h {
                let row = y * w;
                for x in 0..w {
                    prefix[x + 1] = prefix[x] + src[row + x];
                }
                for x in 0..w {
                    let lo = x.saturating_sub(radius);
                    let hi = (x + radius + 1).min(w);
                    tmp[row + x] = (prefix[hi] - prefix[lo]) / (hi - lo) as f32;
                }
            }
            let mut out = src;
            let mut col = vec![0.0f32; h + 1];
            for x in 0..w {
                for y in 0..h {
                    col[y + 1] = col[y] + tmp[y * w + x];
                }
                for y in 0..h {
                    let lo = y.saturating_sub(radius);
                    let hi = (y + radius + 1).min(h);
                    out[y * w + x] = (col[hi] - col[lo]) / (hi - lo) as f32;
                }
            }
            out
        }
        // Deterministic pseudo-random plane (no rand dependency): a
        // multiplicative hash spread over [0,1), so the comparison is not
        // over a constant field where any blur agrees trivially.
        for (w, h) in [(1usize, 1usize), (7, 3), (13, 11), (64, 5), (5, 64)] {
            let plane: Vec<f32> = (0..w * h)
                .map(|i| ((i as u32).wrapping_mul(2_654_435_761) % 10_007) as f32 / 10_007.0)
                .collect();
            for radius in [0usize, 1, 2, 7, 64] {
                assert_eq!(
                    box_blur(plane.clone(), w, h, radius).iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    two_plane(plane.clone(), w, h, radius).iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    "in-place blur diverged at {w}x{h} radius {radius}"
                );
            }
        }
    }

    #[test]
    fn composite_keeps_base_outside_mask_and_gen_inside() {
        let (w, h) = (8u32, 4u32);
        let base = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 255])); // black original
        let gen_img = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255])); // white generative
        // Left half transparent (regenerate), right half opaque (keep original).
        let mut mask = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 255]));
        for y in 0..h {
            for x in 0..w / 2 {
                mask.put_pixel(x, y, Rgba([0, 0, 0, 0]));
            }
        }
        let out = composite_region(&base, &gen_img, &mask, 0); // no feather → crisp boundary
        assert_eq!(out.get_pixel(0, 0)[0], 255, "inside mask should be generative");
        assert_eq!(out.get_pixel(w - 1, 0)[0], 0, "outside mask should stay original");
    }

    #[test]
    fn feather_softens_the_seam() {
        let (w, h) = (16u32, 1u32);
        let base = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 255]));
        let gen_img = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
        let mut mask = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 255]));
        for x in 0..w / 2 {
            mask.put_pixel(x, 0, Rgba([0, 0, 0, 0]));
        }
        let out = composite_region(&base, &gen_img, &mask, 3);
        let mid = out.get_pixel(w / 2, 0)[0];
        assert!(mid > 0 && mid < 255, "seam pixel should feather to gray, got {mid}");
    }

    #[test]
    fn sse_stream_returns_completed_event_skipping_noise() {
        // Comments, event: lines, CRLF endings, non-JSON payloads and partial
        // frames must all be tolerated; the completed event's JSON comes back.
        let body = concat!(
            ": keep-alive comment\r\n",
            "event: image_edit.partial_image\r\n",
            "data: {\"type\":\"image_edit.partial_image\",\"b64_json\":\"AAAA\",\"partial_image_index\":0}\r\n",
            "\r\n",
            "data: not-json\n",
            "\n",
            "data: [DONE]\n",
            "\n",
            "event: image_edit.completed\n",
            "data: {\"type\":\"image_edit.completed\",\"b64_json\":\"QUJD\",\"usage\":{\"total_tokens\":7}}\n",
            "\n",
        );
        let v = read_sse_image(body.as_bytes(), None).unwrap();
        assert_eq!(extract_b64(&v), Some("QUJD"));
        assert_eq!(v["usage"]["total_tokens"], 7);
    }

    #[test]
    fn sse_multi_line_data_frames_join_per_spec_and_eof_flushes() {
        // SSE allows one event's payload to arrive as several data: lines the
        // consumer must join with \n — a split completed event must still
        // parse. This body also ends WITHOUT a trailing blank line, so the
        // final event is flushed by EOF, not by a boundary.
        let body = concat!(
            "data: {\"type\":\"image_edit.completed\",\n",
            "data:  \"b64_json\":\"QUJD\"}\n",
        );
        let v = read_sse_image(body.as_bytes(), None).unwrap();
        assert_eq!(extract_b64(&v), Some("QUJD"));
    }

    #[test]
    fn sse_stream_error_event_fails_loudly() {
        let body = "data: {\"type\":\"error\",\"error\":{\"message\":\"boom\"}}\n\n";
        let err = read_sse_image(body.as_bytes(), None).unwrap_err().to_string();
        assert!(err.contains("boom"), "{err}");
    }

    #[test]
    fn sse_stream_without_completed_event_fails_with_partial_count() {
        let body = "data: {\"type\":\"image_edit.partial_image\",\"b64_json\":\"AAAA\"}\n\n";
        let err = read_sse_image(body.as_bytes(), None).unwrap_err().to_string();
        assert!(err.contains("without a completed event"), "{err}");
        assert!(err.contains("1 partial"), "{err}");
    }

    #[test]
    fn extract_b64_accepts_both_response_shapes() {
        // Blocking JSON: payload nested under data[0].
        let blocking = serde_json::json!({"data": [{"b64_json": "Zm9v"}]});
        assert_eq!(extract_b64(&blocking), Some("Zm9v"));
        // Streaming completed event: payload at the top level.
        let streamed = serde_json::json!({"type": "image_edit.completed", "b64_json": "YmFy"});
        assert_eq!(extract_b64(&streamed), Some("YmFy"));
        assert_eq!(extract_b64(&serde_json::json!({"ok": true})), None);
    }

    #[test]
    fn generated_images_must_match_the_requested_contract() {
        let one = DynamicImage::ImageRgba8(RgbaImage::new(1, 1));
        let png = encode_png(&one).unwrap();
        let err = canonical_generated_png(&png, "1024x1024")
            .unwrap_err()
            .to_string();
        assert!(err.contains("returned 1x1"), "{err}");

        let (canonical, accepted) = canonical_generated_png(&png, "1x1").unwrap();
        assert_eq!(image::guess_format(&canonical).unwrap(), image::ImageFormat::Png);
        assert_eq!(accepted, (1, 1), "an exact match reports its own size");
    }

    /// Local copy of the advisor tests' scripted loopback endpoint (test
    /// modules cannot share helpers across files without a fixture crate).
    /// Bodies are recorded LOSSY — multipart carries PNG bytes, and the
    /// part names asserted on are ASCII. Bind failure panics: a sandbox
    /// that forbids loopback must fail this test, never skip it.
    fn stub_endpoint(
        script: Vec<(u16, &'static str, String)>,
    ) -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        std::thread::JoinHandle<()>,
    ) {
        let server = tiny_http::Server::http("127.0.0.1:0")
            .unwrap_or_else(|e| panic!("bind loopback stub endpoint: {e}"));
        let url = format!(
            "http://{}",
            server.server_addr().to_ip().expect("loopback stub has an IP address")
        );
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = std::sync::Arc::clone(&seen);
        let handle = std::thread::spawn(move || {
            for (status, ctype, body) in script {
                let Ok(mut req) = server.recv() else { return };
                let mut raw = Vec::new();
                let _ = std::io::Read::read_to_end(req.as_reader(), &mut raw);
                recorder
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&raw).into_owned());
                let resp = tiny_http::Response::from_string(body)
                    .with_status_code(status)
                    .with_header(
                        tiny_http::Header::from_bytes(&b"Content-Type"[..], ctype.as_bytes())
                            .expect("static content-type header"),
                    );
                let _ = req.respond(resp);
            }
        });
        (url, seen, handle)
    }

    /// R12-08: a streamed refusal that arrives AFTER a partial frame is
    /// TERMINAL — the partial proves billable generation, and the message
    /// prefix is what carries that fact into the negotiation gate.
    #[test]
    fn a_streamed_refusal_after_a_partial_never_renegotiates() {
        let refusal = r#"{"type":"error","error":{"param":"size","message":"Invalid size"}}"#;
        assert!(
            streamed_refusal_blames(&format!("image stream error: {refusal}"), "size"),
            "the zero-partial spelling still negotiates"
        );
        assert!(
            !streamed_refusal_blames(
                &format!("image stream error after 2 partial(s): {refusal}"),
                "size"
            ),
            "a post-partial refusal must never re-POST"
        );

        // And read_sse_image really emits the post-partial spelling.
        let sse = concat!(
            "data: {\"type\":\"image_edit.partial_image\",\"b64_json\":\"eA==\"}\n\n",
            "data: {\"type\":\"error\",\"error\":{\"param\":\"size\",\"message\":\"Invalid size\"}}\n\n",
        );
        let e = read_sse_image(std::io::Cursor::new(sse.as_bytes().to_vec()), None)
            .expect_err("the error event fails the stream");
        assert!(
            e.to_string().starts_with("image stream error after 1 partial(s):"),
            "{e}"
        );
    }

    /// L09-2: the enum retry rebuilds its input AT the enum size — sending
    /// the flexible-ratio pixels under an enum size request is how circles
    /// came back as ellipses. Counted over a real loopback transport.
    #[test]
    fn the_enum_retry_requests_freshly_sized_input() {
        let png_1024 =
            encode_png(&DynamicImage::ImageRgba8(RgbaImage::new(1024, 1024))).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_1024);
        let (url, _seen, handle) = stub_endpoint(vec![
            (
                400,
                "application/json",
                r#"{"error":{"param":"size","message":"unsupported size"}}"#.into(),
            ),
            (200, "application/json", format!(r#"{{"data":[{{"b64_json":"{b64}"}}]}}"#)),
        ]);
        let cfg = Config {
            openai_api_key: Some("test-key".into()),
            openai_model: "test-chat".into(),
            openai_base_url: url,
            openai_image_model: "test-image".into(),
            openai_image_quality: "auto".into(),
            openai_image_max_px: 4_000_000,
            image_provider: "api".into(),
            image_effort: None,
            image_api_family: "auto".into(),
            image_gen_endpoint: None,
            analysis_provider: "oauth".into(),
            analysis_model: "opus".into(),
            analysis_effort: None,
            claude_bin: "claude".into(),
            analysis_api_key: None,
            analysis_base_url: "http://127.0.0.1:1".into(),
            python_bin: "python".into(),
            denoise_model: "scunet_color_real_psnr".into(),
            denoise_script: String::new(),
            denoise_raw_script: String::new(),
            weights_dir: String::new(),
            segment_script: String::new(),
            depth_script: String::new(),
            embed_script: String::new(),
            correspond_script: String::new(),
            describe_script: String::new(),
            style_strength: 0.5,
            send_reference_image: false,
        };
        let src = encode_png(&DynamicImage::ImageRgba8(RgbaImage::new(8, 8))).unwrap();
        let sizes = SizePlan { flexible: Some("1600x912".into()), enum_size: "1024x1024" };
        let mut asked: Vec<String> = Vec::new();
        let got = call_images_edit(
            &cfg,
            &mut |size| {
                asked.push(size.to_string());
                Ok((src.clone(), None))
            },
            "brighten the sky",
            "high",
            &sizes,
            "auto",
        )
        .expect("the enum retry succeeds");
        assert_eq!(got.requested, "1024x1024");
        assert!(
            asked.contains(&"1600x912".to_string()) && asked.contains(&"1024x1024".to_string()),
            "each attempted size asked for its own input: {asked:?}"
        );
        let grace = std::time::Instant::now();
        while !handle.is_finished() && grace.elapsed() < std::time::Duration::from_secs(2) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = handle.join();
    }

    /// 2026-10-17: a custom endpoint (Venice's shape, JSON reply variant) gets
    /// ONE JSON POST at the enum size with the template filled in, and its
    /// answer — another resolution at the same aspect — comes back at the
    /// requested size. A non-2xx is reported, never retried.
    #[test]
    fn a_custom_endpoint_posts_the_template_and_resizes_the_answer() {
        let answer = encode_png(&DynamicImage::ImageRgba8(RgbaImage::new(1264, 848))).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&answer);
        let (url, seen, handle) = stub_endpoint(vec![
            (200, "application/json", format!(r#"{{"data":[{{"b64_json":"{b64}"}}]}}"#)),
            (404, "application/json", r#"{"error":"Not found"}"#.into()),
            (500, "application/json", r#"{"error":"bridge"}"#.into()),
        ]);
        let mut cfg = Config {
            openai_api_key: Some("test-key".into()),
            openai_model: "test-chat".into(),
            openai_base_url: url,
            openai_image_model: "test-image-edit".into(),
            openai_image_quality: "auto".into(),
            openai_image_max_px: 4_000_000,
            image_provider: "api".into(),
            image_effort: None,
            image_api_family: "auto".into(),
            image_gen_endpoint: Some(
                r#"{"path":"/image/multi-edit","body":{"modelId":"{model}","prompt":"{prompt}",
                    "images":["{image}","{mask}"]},"reply":"/data/0/b64_json"}"#
                    .into(),
            ),
            analysis_provider: "oauth".into(),
            analysis_model: "opus".into(),
            analysis_effort: None,
            claude_bin: "claude".into(),
            analysis_api_key: None,
            analysis_base_url: "http://127.0.0.1:1".into(),
            python_bin: "python".into(),
            denoise_model: "scunet_color_real_psnr".into(),
            denoise_script: String::new(),
            denoise_raw_script: String::new(),
            weights_dir: String::new(),
            segment_script: String::new(),
            depth_script: String::new(),
            embed_script: String::new(),
            correspond_script: String::new(),
            describe_script: String::new(),
            style_strength: 0.5,
            send_reference_image: false,
        };
        let src = encode_png(&DynamicImage::ImageRgba8(RgbaImage::new(8, 8))).unwrap();
        let sizes = SizePlan { flexible: Some("1600x1072".into()), enum_size: "1536x1024" };
        let mut asked: Vec<String> = Vec::new();
        let got = call_images_edit(
            &cfg,
            &mut |size| {
                asked.push(size.to_string());
                Ok((src.clone(), None))
            },
            "brighten the sky",
            "high",
            &sizes,
            "auto",
        )
        .expect("the custom endpoint answers");
        assert_eq!(asked, ["1536x1024"], "one request, at the enum size");
        assert_eq!((got.requested.as_str(), got.accepted.as_str()), ("1536x1024", "1536x1024"));
        assert_eq!(image::load_from_memory(&got.bytes).unwrap().dimensions(), (1536, 1024));
        let body: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
        assert_eq!(body["modelId"], "test-image-edit");
        assert_eq!(body["prompt"], "brighten the sky");
        assert_eq!(body["images"].as_array().unwrap().len(), 1, "no mask was painted");

        let Err(e) = call_images_edit(&cfg, &mut |_| Ok((src.clone(), None)), "x", "high", &sizes, "auto")
        else {
            panic!("a 404 is an error")
        };
        assert!(e.to_string().starts_with("image API 404"), "{e}");

        // Under the subscription-bridge preset the template waits: the bridge
        // gets OpenAI's own multipart call.
        cfg.image_provider = "oauth".into();
        let Err(e) = call_images_edit(&cfg, &mut |_| Ok((src.clone(), None)), "x", "high", &sizes, "auto")
        else {
            panic!("the stub answers 500")
        };
        assert!(e.to_string().starts_with("image API 500"), "{e}");
        assert!(seen.lock().unwrap()[2].contains("Content-Disposition: form-data"), "OpenAI's multipart call");
        let _ = handle.join();
    }

    /// L09-3: the feather follows the painted selection's size, and an empty
    /// selection defers to the caller's frame rule.
    #[test]
    fn the_feather_follows_the_selection_not_the_frame() {
        let (w, h) = (100usize, 100usize);
        let mut weight = vec![0.0f32; w * h];
        for y in 40..50 {
            for x in 40..50 {
                weight[y * w + x] = 1.0;
            }
        }
        assert_eq!(selection_feather(&weight, w), 1, "a 10 px selection feathers by 1 px");
        assert_eq!(selection_feather(&vec![0.0f32; w * h], w), usize::MAX, "empty defers");
        let _ = h;
    }

    /// L09-5: the blend carries the base ALPHA — a 1%-weight feathered edge
    /// on a transparent base must stay (nearly) transparent, not become an
    /// opaque halo.
    #[test]
    fn a_feathered_edge_on_a_transparent_base_stays_transparent() {
        let mut out = RgbaImage::from_pixel(2, 1, image::Rgba([0, 0, 0, 0]));
        let gen_img = RgbImage::from_pixel(2, 1, image::Rgb([255, 255, 255]));
        composite_in_place(&mut out, &gen_img, &[0.01, 1.0]);
        assert!(out.get_pixel(0, 0)[3] <= 5, "1% weight ≈ transparent: {:?}", out.get_pixel(0, 0));
        // R12-09: STRAIGHT colour survives — independent RGB/alpha mixing
        // gave (3,3,3,3), a dark fringe once composited downstream.
        assert!(
            out.get_pixel(0, 0)[0] >= 250,
            "straight RGB stays the generated colour: {:?}",
            out.get_pixel(0, 0)
        );
        assert_eq!(out.get_pixel(1, 0)[3], 255, "full weight is opaque");
    }

    /// L14#2 (images side): each images/edits POST is billed PER IMAGE, so
    /// a second POST may only ever follow a capability refusal — and the
    /// dropped flag must actually be gone from the retry body. Counted over
    /// a real loopback transport; the old guard was a source grep for an
    /// identifier that no longer exists.
    #[test]
    fn the_images_edit_negotiation_never_re_bills_a_dropped_flag() {
        let png_1024 =
            encode_png(&DynamicImage::ImageRgba8(RgbaImage::new(1024, 1024))).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_1024);
        let (url, seen, handle) = stub_endpoint(vec![
            (
                400,
                "application/json",
                r#"{"error":{"param":"stream","message":"streaming unsupported"}}"#.into(),
            ),
            (200, "application/json", format!(r#"{{"data":[{{"b64_json":"{b64}"}}]}}"#)),
        ]);
        let cfg = Config {
            openai_api_key: Some("test-key".into()),
            openai_model: "test-chat".into(),
            openai_base_url: url,
            openai_image_model: "test-image".into(),
            openai_image_quality: "auto".into(),
            openai_image_max_px: 4_000_000,
            image_provider: "api".into(),
            image_effort: None,
            image_api_family: "auto".into(),
            image_gen_endpoint: None,
            analysis_provider: "oauth".into(),
            analysis_model: "opus".into(),
            analysis_effort: None,
            claude_bin: "claude".into(),
            analysis_api_key: None,
            analysis_base_url: "http://127.0.0.1:1".into(),
            python_bin: "python".into(),
            denoise_model: "scunet_color_real_psnr".into(),
            denoise_script: String::new(),
            denoise_raw_script: String::new(),
            weights_dir: String::new(),
            segment_script: String::new(),
            depth_script: String::new(),
            embed_script: String::new(),
            correspond_script: String::new(),
            describe_script: String::new(),
            style_strength: 0.5,
            send_reference_image: false,
        };
        let src = encode_png(&DynamicImage::ImageRgba8(RgbaImage::new(8, 8))).unwrap();
        let sizes = SizePlan { flexible: None, enum_size: "1024x1024" };
        let got =
            call_images_edit(&cfg, &mut |_| Ok((src.clone(), None)), "brighten the sky", "high", &sizes, "auto")
                .expect("the blocking retry succeeds");
        assert_eq!(got.requested, "1024x1024");
        assert!(!got.bytes.is_empty(), "the accepted retry returns the image");

        // Fully-consumed script, bounded wait (the join_bounded philosophy).
        let grace = std::time::Instant::now();
        while !handle.is_finished() && grace.elapsed() < std::time::Duration::from_secs(2) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(handle.is_finished(), "the stub script was not fully consumed");
        let _ = handle.join();

        let bodies = seen.lock().unwrap().clone();
        assert_eq!(bodies.len(), 2, "one refusal, one retry — never a third billed POST");
        assert!(
            bodies[0].contains("name=\"stream\"")
                && bodies[0].contains("name=\"partial_images\""),
            "the first attempt streams"
        );
        assert!(
            !bodies[1].contains("name=\"stream\"")
                && !bodies[1].contains("name=\"partial_images\""),
            "the dropped flag stays dropped"
        );
        assert!(
            bodies[1].contains("name=\"input_fidelity\""),
            "only the BLAMED flag drops — fidelity survives the stream retry"
        );
    }
    /// Step-6 (a): the faithfulness scaffold is UNCONDITIONAL under "high" —
    /// a typed style direction rides WITH it instead of replacing it (the old
    /// GUI behaviour lost the preservation clause the moment the user typed
    /// anything), and "low" stays documented free rein. Supervisor mutation
    /// MA (hardened_prompt degraded to the user text verbatim) goes red here
    /// and on the wire test below.
    #[test]
    fn the_faithfulness_scaffold_is_unconditional_under_high_fidelity() {
        let wired = hardened_prompt("golden-hour glow", "high");
        assert!(wired.contains("Re-develop this exact photograph"), "{wired}");
        assert!(wired.contains("Style direction: golden-hour glow"), "{wired}");
        let empty = hardened_prompt("   ", "high");
        assert!(empty.contains("Re-develop this exact photograph"));
        assert!(!empty.contains("Style direction"), "{empty}");
        assert_eq!(
            hardened_prompt("full repaint as a watercolor", "low"),
            "full repaint as a watercolor",
            "low stays documented free rein"
        );
    }

    /// Blank = remove: a fill whose user typed nothing sends the removal
    /// instruction; anything typed rides verbatim (no scaffold on this path:
    /// the mask is the faithfulness contract, the composite keeps every
    /// unmasked pixel). A field of spaces is a blank field.
    #[test]
    fn a_blank_fill_prompt_is_the_removal_instruction() {
        assert_eq!(fill_prompt(""), REMOVE_PROMPT);
        assert_eq!(fill_prompt(" \t\n"), REMOVE_PROMPT);
        assert_eq!(fill_prompt("  extend the sky  "), "extend the sky");
        assert!(!fill_prompt("extend the sky").contains("masked area"));
    }

    /// The rule on the wire: one loopback fill with an EMPTY prompt sends the
    /// removal instruction in the multipart prompt part, with the mask part
    /// beside it, and its composite lands; a second fill with typed words
    /// sends those words and no removal text. The GUI and the browser no
    /// longer refuse a blank field, so this is the only gate on the rule.
    #[test]
    fn a_fill_with_no_prompt_sends_the_removal_instruction() {
        let dir = std::env::temp_dir().join(format!("autoshade-fill-wire-{}", std::process::id()));
        // The library layout the read-only guard expects (see the reimagine
        // wire test): the source in its OWN folder, outputs in a sibling.
        std::fs::create_dir_all(dir.join("library")).unwrap();
        let input = dir.join("library").join("input.png");
        structured_frame(512, 512).save(&input).unwrap();
        // An RGBA mask: a transparent block = regenerate, opaque elsewhere.
        let mask = dir.join("mask.png");
        RgbaImage::from_fn(512, 512, |x, y| {
            let hole = (128..256).contains(&x) && (128..256).contains(&y);
            Rgba([0, 0, 0, if hole { 0 } else { 255 }])
        })
        .save(&mask)
        .unwrap();
        // The enum size the plan requests for a square (the endpoint contract
        // refuses any other): the composite resizes it onto the base.
        let echo = encode_png(&structured_frame(1024, 1024)).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&echo);
        let reply = format!(r#"{{"data":[{{"b64_json":"{b64}"}}]}}"#);
        let (url, seen, handle) = stub_endpoint(vec![
            (200, "application/json", reply.clone()),
            (200, "application/json", reply),
        ]);
        let cfg = stub_cfg(url);
        let removed = dir.join("exports").join("removed.png");
        retouch(&cfg, &input, &mask, "   ", "auto", false, &removed)
            .expect("the loopback fill with a blank prompt succeeds");
        assert!(removed.is_file(), "the removal composite landed");
        let filled = dir.join("exports").join("filled.png");
        retouch(&cfg, &input, &mask, "extend the sky", "auto", false, &filled)
            .expect("the loopback fill with typed words succeeds");
        bounded_join(handle);
        let bodies = seen.lock().unwrap().clone();
        assert_eq!(bodies.len(), 2, "one billed POST per fill");
        assert!(
            bodies[0].contains("Remove everything inside the masked area"),
            "blank = remove reached the wire"
        );
        assert!(bodies[0].contains("name=\"mask\""), "the mask rides with it");
        assert!(bodies[1].contains("extend the sky"), "typed words ride verbatim");
        assert!(
            !bodies[1].contains("Remove everything inside the masked area"),
            "typed words carry no removal text"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `retouch_onto` composites onto the base its CALLER produced — the
    /// GUI's developed card picture — never the file on disk: outside the
    /// mask the output is that base, inside it the model's answer.
    #[test]
    fn the_fill_composites_onto_the_base_the_caller_produced() {
        let dir = std::env::temp_dir().join(format!("autoshade-fill-onto-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("library")).unwrap();
        let input = dir.join("library").join("input.png");
        structured_frame(256, 256).save(&input).unwrap();
        let mask = dir.join("mask.png");
        RgbaImage::from_fn(256, 256, |x, y| {
            let hole = (96..160).contains(&x) && (96..160).contains(&y);
            Rgba([0, 0, 0, if hole { 0 } else { 255 }])
        })
        .save(&mask)
        .unwrap();
        // The model answers at the enum size the plan asks for (the endpoint
        // contract refuses any other size): the caller's base where it was
        // told to keep it, red over the painted block.
        let green = image::Rgb([20, 200, 20]);
        let red = DynamicImage::ImageRgb8(RgbImage::from_fn(1024, 1024, |x, y| {
            if (384..640).contains(&x) && (384..640).contains(&y) { image::Rgb([200, 20, 20]) } else { green }
        }));
        let b64 = base64::engine::general_purpose::STANDARD.encode(encode_png(&red).unwrap());
        let (url, _seen, handle) = stub_endpoint(vec![(
            200,
            "application/json",
            format!(r#"{{"data":[{{"b64_json":"{b64}"}}]}}"#),
        )]);
        let cfg = stub_cfg(url);
        let out = dir.join("exports").join("onto.png");
        // The caller's base: a flat green frame the file on disk never had.
        retouch_onto(
            &cfg,
            &input,
            || Ok(DynamicImage::ImageRgb8(RgbImage::from_pixel(256, 256, green))),
            "caller",
            &FillJob { mask_path: &mask, prompt: "", quality: "auto", out: &out, layer: false },
        )
        .expect("the loopback fill onto the caller's base succeeds");
        bounded_join(handle);
        let got = image::open(&out).unwrap().to_rgb8();
        assert_eq!(got.dimensions(), (256, 256), "the composite is the base's size");
        let near = |a: [u8; 3], b: [u8; 3]| a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 1);
        assert!(
            near(got.get_pixel(8, 8).0, green.0),
            "outside the mask: the caller's base, not the file ({:?})",
            got.get_pixel(8, 8)
        );
        assert!(
            near(got.get_pixel(128, 128).0, [200, 20, 20]),
            "inside the mask: the model's answer ({:?})",
            got.get_pixel(128, 128)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The model's answer, as it really comes back: the sent frame shifted by
    /// `drift` levels everywhere (user report 2026-10-12: +42 / +40 / +24
    /// measured at a fill's edge) and `inside` drawn over the block.
    fn drifted_answer(sent: &RgbImage, drift: [i16; 3], block: (u32, u32, u32, u32), inside: [u8; 3]) -> RgbImage {
        RgbImage::from_fn(sent.width(), sent.height(), |x, y| {
            let (x0, y0, x1, y1) = block;
            if (x0..x1).contains(&x) && (y0..y1).contains(&y) {
                return image::Rgb(inside);
            }
            let p = sent.get_pixel(x, y);
            image::Rgb(std::array::from_fn(|c| (p[c] as i16 + drift[c]).clamp(0, 255) as u8))
        })
    }

    /// User report 2026-10-12 (「涂抹生成后的区域跟周围还是有很明显的差距」): the
    /// model redraws the whole frame shifted, so a fill pasted as it came
    /// back sat +42 levels off its surroundings. Levelled by what the model
    /// was meant to leave alone, the fill meets the base, and what the model
    /// drew inside keeps its own structure.
    ///
    /// MUTATION THIS CATCHES: the kept texels left out of `level_field` — the
    /// area reads the drift again (`level.apply` dropped from `edit_onto` is
    /// caught by the wire test below, which reads the published layer).
    #[test]
    fn a_fill_is_levelled_onto_its_surroundings() {
        let sent = structured_frame(384, 256).to_rgb8();
        let hole = (160u32, 96u32, 240u32, 176u32);
        let in_hole = |x: u32, y: u32| (hole.0..hole.2).contains(&x) && (hole.1..hole.3).contains(&y);
        let mask = RgbaImage::from_fn(384, 256, |x, y| Rgba([0, 0, 0, if in_hole(x, y) { 0 } else { 255 }]));
        // The model shifted everything and drew stripes inside.
        let drift = [42i16, 40, 24];
        let answer = RgbImage::from_fn(384, 256, |x, y| {
            let s = sent.get_pixel(x, y);
            let stripe = if in_hole(x, y) { if (x / 8) % 2 == 0 { 30 } else { -30 } } else { 0 };
            image::Rgb(std::array::from_fn(|c| (s[c] as i16 + drift[c] + stripe).clamp(0, 255) as u8))
        });
        let region = Region::from_mask(&mask);
        let level = level_field(&sent, &answer, &region);
        let weight = region.weight_at(384, 256);
        let mut levelled = answer.clone();
        level.apply(&mut levelled, &weight);
        let mean_off = |img: &RgbImage| -> [f32; 3] {
            let (mut sum, mut n) = ([0f32; 3], 0f32);
            for y in hole.1..hole.3 {
                for x in hole.0..hole.2 {
                    let (a, b) = (img.get_pixel(x, y), sent.get_pixel(x, y));
                    for c in 0..3 {
                        sum[c] += a[c] as f32 - b[c] as f32;
                    }
                    n += 1.0;
                }
            }
            sum.map(|s| s / n)
        };
        let before = mean_off(&answer);
        let after = mean_off(&levelled);
        assert!(before[0] > 35.0, "the fixture carries the drift: {before:?}");
        assert!(after.iter().all(|d| d.abs() < 4.0), "levelled inside the area: {after:?} (was {before:?})");
        let (a, b) = (levelled.get_pixel(196, 136)[1] as i32, levelled.get_pixel(204, 136)[1] as i32);
        assert!((a - b).abs() >= 50, "the model's own stripes are kept: {a} vs {b}");
        assert_eq!(levelled.get_pixel(8, 8), answer.get_pixel(8, 8), "nothing outside the area is touched");
    }

    /// Not a check — the tuning bench for [`found_region`] on REAL answers:
    /// `AUTOSHADE_FOUND_SENT=<sent.png> AUTOSHADE_FOUND_ANSWERS=<a.png;b.png>
    /// AUTOSHADE_FOUND_OUT=<dir> cargo test found_region_bench -- --ignored
    /// --nocapture` prints each answer's share and score spread and writes its
    /// score map and area.
    #[test]
    #[ignore = "reads real model answers from the environment"]
    fn found_region_bench() {
        let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k}"));
        let sent = image::open(var("AUTOSHADE_FOUND_SENT")).unwrap().to_rgb8();
        let out = std::path::PathBuf::from(var("AUTOSHADE_FOUND_OUT"));
        for a in var("AUTOSHADE_FOUND_ANSWERS").split(';') {
            let answer = image::open(a).unwrap().resize_exact(sent.width(), sent.height(), FilterType::Triangle).to_rgb8();
            let found = found_region(&sent, &answer);
            let (score, gw, gh, seed, grow) = FOUND_SCORES.with_borrow(|d| d.clone().unwrap());
            let mut sorted = score.clone();
            sorted.sort_by(f32::total_cmp);
            let q = |f: f32| sorted[((sorted.len() - 1) as f32 * f) as usize];
            println!(
                "{a}: share {:.3}  seed {seed:.1} grow {grow:.1}  score p50 {:.1} p90 {:.1} p99 {:.1} max {:.1}",
                found.share(), q(0.5), q(0.9), q(0.99), q(1.0)
            );
            let stem = std::path::Path::new(a).file_stem().unwrap().to_string_lossy().to_string();
            let top = q(1.0).max(1.0);
            image::GrayImage::from_fn(gw as u32, gh as u32, |x, y| {
                image::Luma([(255.0 * score[y as usize * gw + x as usize] / top).min(255.0) as u8])
            })
            .save(out.join(format!("{stem}_score.png")))
            .unwrap();
            image::GrayImage::from_fn(found.w as u32, found.h as u32, |x, y| {
                image::Luma([(255.0 * found.take[y as usize * found.w + x as usize]) as u8])
            })
            .save(out.join(format!("{stem}_area.png")))
            .unwrap();
        }
    }

    /// A hole in the grid is filled from its rim by pull-push, and a fully
    /// known grid comes back exactly as it went in.
    #[test]
    fn pull_push_keeps_known_cells_and_fills_holes_from_their_rim() {
        let (w, h) = (9usize, 7usize);
        let v: Vec<[f32; 3]> = (0..w * h).map(|i| [(i % w) as f32, 0.0, 5.0]).collect();
        assert_eq!(pull_push(v.clone(), vec![1.0; w * h], w, h), v, "a known grid is returned exactly");
        let mut conf = vec![1.0; w * h];
        for y in 2..5 {
            for x in 3..6 {
                conf[y * w + x] = 0.0;
            }
        }
        let holes: Vec<[f32; 3]> = v.iter().zip(&conf).map(|(p, &c)| if c > 0.0 { *p } else { [999.0; 3] }).collect();
        let got = pull_push(holes, conf.clone(), w, h);
        for i in (0..w * h).filter(|&i| conf[i] == 0.0) {
            let x = (i % w) as f32;
            assert!((got[i][0] - x).abs() < 1.5, "cell {i}: the ramp runs on into the hole: {:?}", got[i]);
            assert!((got[i][2] - 5.0).abs() < 1e-3, "a constant channel stays constant: {:?}", got[i]);
        }
    }

    /// A maskless edit's area is what the model CHANGED: a block it redrew is
    /// found; the frame-wide drift and a re-drawn texture everywhere else are
    /// not, and pure drift finds nothing at all.
    ///
    /// MUTATION THIS CATCHES: the drift left out of the score — every cell
    /// then reads +30 and the whole frame is flagged.
    #[test]
    fn the_found_area_is_the_changed_block_not_the_drift() {
        let sent = structured_frame(480, 320).to_rgb8();
        let block = (300u32, 60u32, 380u32, 140u32);
        let mut answer = drifted_answer(&sent, [30, 28, 18], block, [250, 250, 250]);
        // Re-drawn texture: up to ±3 levels of hash noise over the frame.
        for (x, y, p) in answer.enumerate_pixels_mut() {
            let n = ((x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) >> 13) % 7;
            for c in 0..3 {
                p[c] = (p[c] as i16 + n as i16 - 3).clamp(0, 255) as u8;
            }
        }
        let found = found_region(&sent, &answer);
        let at = |x: usize, y: usize| found.take[y * found.w + x];
        assert_eq!(at(340, 100), 1.0, "the redrawn block is found");
        assert_eq!(at(20, 300), 0.0, "a drifted corner is not");
        assert_eq!(at(100, 200), 0.0, "nor the drifted middle");
        let block_share = (80.0 * 80.0) / (480.0 * 320.0);
        let share = found.share();
        assert!(share >= block_share && share < 3.0 * block_share, "the area is the block and its rim: {share} vs {block_share}");
        let echo = drifted_answer(&sent, [30, 28, 18], (0, 0, 0, 0), [0, 0, 0]);
        assert_eq!(found_region(&sent, &echo).share(), 0.0, "pure drift is no edit");
    }

    /// An edit as large as the sky is the EDIT, not the drift — the real
    /// "make the sky bluer" of 2026-10-12, in its own numbers: the top 55 %
    /// moved −28 / −9 / +14 levels, the ground +7 / +2 / −1. Over all cells
    /// the sky is the median, and a drift read there found 1 % of the frame;
    /// read off the half that moved least, the sky is found and the ground is
    /// not.
    ///
    /// MUTATION THIS CATCHES: the drift's median taken over every measured
    /// cell instead of the stiller half.
    #[test]
    fn a_sky_sized_edit_is_the_edit_not_the_drift() {
        let sent = structured_frame(480, 320).to_rgb8();
        let horizon = 176u32;
        let answer = RgbImage::from_fn(480, 320, |x, y| {
            let p = sent.get_pixel(x, y);
            let d: [i16; 3] = if y < horizon { [-28, -9, 14] } else { [7, 2, -1] };
            image::Rgb(std::array::from_fn(|c| (p[c] as i16 + d[c]).clamp(0, 255) as u8))
        });
        let found = found_region(&sent, &answer);
        let at = |x: usize, y: usize| found.take[y * found.w + x];
        assert_eq!(at(240, 60), 1.0, "the sky is found");
        assert_eq!(at(240, 300), 0.0, "the drifted ground is not");
        let share = found.share();
        assert!((0.5..0.7).contains(&share), "the area is the sky and its rim: {share}");
    }

    /// On the wire: a maskless edit sends NO mask part, rides the user's
    /// words under the keep-the-rest clause, and publishes a LAYER that is
    /// transparent wherever the model only drifted. A blank instruction is
    /// refused before the base is developed or a call is paid.
    #[test]
    fn an_auto_fill_sends_no_mask_and_layers_only_what_changed() {
        let dir = std::env::temp_dir().join(format!("autoshade-auto-fill-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("library")).unwrap();
        let input = dir.join("library/input.png");
        structured_frame(192, 128).save(&input).unwrap();
        // What the plan sends at the enum size, drifted, one block repainted.
        let sent = structured_frame(192, 128).resize_exact(1536, 1024, FilterType::Lanczos3).to_rgb8();
        let answer = drifted_answer(&sent, [20, 20, 20], (1000, 200, 1300, 500), [250, 250, 250]);
        let b64 = base64::engine::general_purpose::STANDARD.encode(encode_png(&DynamicImage::ImageRgb8(answer)).unwrap());
        let (url, seen, handle) = stub_endpoint(vec![(
            200, "application/json", format!(r#"{{"data":[{{"b64_json":"{b64}"}}]}}"#),
        )]);
        let out = dir.join("exports/auto.png");
        let report = auto_fill_onto(
            &stub_cfg(url.clone()), &input, || Ok(structured_frame(192, 128)), "caller",
            &AutoFillJob { prompt: "remove the sign", quality: "high", out: &out },
        ).expect("the loopback maskless edit succeeds");
        bounded_join(handle);
        let bodies = seen.lock().unwrap().clone();
        assert_eq!(bodies.len(), 1, "one edit buys one generation");
        assert!(!bodies[0].contains("name=\"mask\""), "a maskless edit sends no mask part");
        assert!(bodies[0].contains("Instruction: remove the sign"), "the user's words ride under the clause");
        assert!(bodies[0].contains("change nothing else"), "the keep-the-rest clause rides with them");
        let layer = image::open(&out).unwrap().to_rgba8();
        assert_eq!(layer.dimensions(), (192, 128), "the layer is the caller's base size");
        // The block (1000..1300, 200..500 of 1536x1024) is ~(125..162, 25..62) here.
        assert_eq!(layer.get_pixel(143, 43)[3], 255, "the changed block is covered");
        // Levelled: the model's +20 drift is taken back off what it drew.
        let p = layer.get_pixel(143, 43);
        assert!((0..3).all(|c| p[c].abs_diff(230) <= 3), "the block is levelled by the drift: {p:?}");
        assert_eq!(layer.get_pixel(20, 110)[3], 0, "where the model only drifted, nothing is pasted");
        assert!(report.coverage > 0.03 && report.coverage < 0.2, "coverage reads the block: {}", report.coverage);

        let err = auto_fill_onto(
            &stub_cfg(url), &input, || panic!("blank must be refused before the base"), "caller",
            &AutoFillJob { prompt: "  ", quality: "high", out: &dir.join("exports/blank.png") },
        ).unwrap_err();
        assert!(err.to_string().contains("needs words"), "blank is named: {err:#}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_adjust_without_a_region_sends_no_mask_and_keeps_the_whole_returned_frame() {
        let dir = std::env::temp_dir().join(format!("autoshade-adjust-whole-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("library")).unwrap();
        let input = dir.join("library/input.png");
        structured_frame(192, 128).save(&input).unwrap();
        let returned = noise_frame(1536, 1024);
        let b64 = base64::engine::general_purpose::STANDARD.encode(encode_png(&returned).unwrap());
        let (url, seen, handle) = stub_endpoint(vec![(
            200, "application/json", format!(r#"{{"data":[{{"b64_json":"{b64}"}}]}}"#),
        )]);
        let out = dir.join("exports/adjust.png");
        adjust_onto(
            &stub_cfg(url), &input, || Ok(structured_frame(192, 128)), "caller",
            &AdjustJob { prompt: "make the sky bluer", quality: "medium", out: &out },
        ).expect("the loopback whole-image adjust succeeds");
        bounded_join(handle);
        let bodies = seen.lock().unwrap();
        assert_eq!(bodies.len(), 1, "one adjust buys one generation");
        assert!(!bodies[0].contains("name=\"mask\""), "a whole-image adjust must send no mask part");
        assert!(bodies[0].contains("make the sky bluer"), "the adjust's prompt reached the wire");
        assert!(bodies[0].contains("\r\nmedium\r\n"), "the adjust's own quality reached the wire");
        let got = image::open(&out).unwrap();
        assert_eq!(got.dimensions(), (192, 128), "the master is the caller's base dimensions");
        assert_eq!(got.color(), image::ColorType::Rgba8, "the fill's own master encoder");
        assert_eq!(
            got.to_rgba8(), returned.resize_exact(192, 128, FilterType::Lanczos3).to_rgba8(),
            "every returned pixel survives the resize; none is composited with the base"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_blank_prompt_is_refused_for_a_whole_image_adjust() {
        let dir = std::env::temp_dir().join(format!("autoshade-adjust-blank-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("library")).unwrap();
        let input = dir.join("library/input.png");
        let out = dir.join("exports/adjust.png");
        // A live loopback socket witnesses NO call. The blank guard must run
        // even before decoding; there is deliberately no input file here.
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let cfg = stub_cfg(format!("http://{}", server.server_addr().to_ip().unwrap()));
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = calls.clone();
        let handle = std::thread::spawn(move || {
            if let Some(req) = server.recv_timeout(std::time::Duration::from_secs(1)).unwrap() {
                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let _ = req.respond(tiny_http::Response::from_string("unexpected paid call").with_status_code(500));
            }
        });
        let developed = std::cell::Cell::new(false);
        let err = adjust_onto(
            &cfg, &input, || { developed.set(true); Ok(structured_frame(64, 64)) }, "caller",
            &AdjustJob { prompt: " \t\n ", quality: "high", out: &out },
        ).unwrap_err();
        bounded_join(handle);
        assert!(err.to_string().contains("non-blank prompt"), "the blank prompt is named: {err:#}");
        assert!(!developed.get(), "blank is refused before the base is developed");
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0, "blank buys no call");
        assert!(!out.exists(), "blank writes no master");

        // Preflight is FIRST, including before the base and a non-blank call.
        std::fs::create_dir_all(&out).unwrap();
        let err = adjust_onto(
            &cfg, &input, || panic!("preflight must precede the base"), "caller",
            &AdjustJob { prompt: "bluer sky", quality: "high", out: &out },
        ).unwrap_err();
        assert!(err.to_string().contains("refusing to overwrite"), "preflight names the bad output: {err:#}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_adjust_reports_its_divergence_against_the_sent_input() {
        let dir = std::env::temp_dir().join(format!("autoshade-adjust-divergence-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("library")).unwrap();
        let input = dir.join("library/input.png");
        // The source file differs from the caller's base: measuring against
        // it instead of the SENT develop cannot pass the echo case.
        noise_frame(256, 256).save(&input).unwrap();
        let sent = structured_frame(256, 256).resize_exact(1024, 1024, FilterType::Lanczos3);
        for different in [false, true] {
            let answer = if different { noise_frame(1024, 1024) } else { sent.clone() };
            let b64 = base64::engine::general_purpose::STANDARD.encode(encode_png(&answer).unwrap());
            let (url, seen, handle) = stub_endpoint(vec![(
                200, "application/json", format!(r#"{{"data":[{{"b64_json":"{b64}"}}]}}"#),
            )]);
            let out = dir.join(format!("exports/adjust-{different}.png"));
            let report = adjust_onto(
                &stub_cfg(url), &input, || Ok(structured_frame(256, 256)), "caller",
                &AdjustJob { prompt: "bluer sky", quality: "high", out: &out },
            ).unwrap();
            bounded_join(handle);
            assert_eq!(seen.lock().unwrap().len(), 1, "divergence never buys a retry");
            if different {
                assert!(report.divergence >= crate::fit::DIVERGENCE_GLOBAL,
                    "an unrelated frame must cross the structural threshold: D={}", report.divergence);
            } else {
                assert!(report.divergence.abs() < 1e-5,
                    "an unchanged SENT input has D near zero: D={}", report.divergence);
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A deterministic structured frame: luma gradient + checker + a block,
    /// so the 384-raster divergence statistic has real gradients to
    /// correlate. Used by both step-6 loopback tests.
    fn structured_frame(w: u32, h: u32) -> DynamicImage {
        let img = RgbImage::from_fn(w, h, |x, y| {
            let g = (255.0 * x as f32 / w.max(1) as f32) as i32;
            let checker = if ((x / 32) + (y / 32)) % 2 == 0 { 40 } else { -40 };
            let block =
                if x > w / 4 && x < w / 2 && y > h / 4 && y < h / 2 { 70 } else { 0 };
            let v = (g + checker + block).clamp(0, 255) as u8;
            image::Rgb([v, ((v as u16 * 3) / 4) as u8, 255 - v])
        });
        DynamicImage::ImageRgb8(img)
    }

    /// Deterministic pseudo-noise (multiplicative hash, no rand dep) —
    /// structurally unrelated to [`structured_frame`], so D reads far past
    /// the threshold.
    fn noise_frame(w: u32, h: u32) -> DynamicImage {
        let img = RgbImage::from_fn(w, h, |x, y| {
            let i = y.wrapping_mul(w).wrapping_add(x);
            let v = (i.wrapping_mul(2_654_435_761) >> 8) as u8;
            image::Rgb([v, v.wrapping_mul(31), v.wrapping_add(97)])
        });
        DynamicImage::ImageRgb8(img)
    }

    /// The step-6 loopback Config: enum sizes only (the pixel budget sits
    /// under the flexible-size API minimum), everything else inert.
    fn stub_cfg(url: String) -> Config {
        Config {
            openai_api_key: Some("test-key".into()),
            openai_model: "test-chat".into(),
            openai_base_url: url,
            openai_image_model: "test-image".into(),
            openai_image_quality: "auto".into(),
            openai_image_max_px: 100_000,
            image_provider: "api".into(),
            image_effort: None,
            image_api_family: "auto".into(),
            image_gen_endpoint: None,
            analysis_provider: "oauth".into(),
            analysis_model: "opus".into(),
            analysis_effort: None,
            claude_bin: "claude".into(),
            analysis_api_key: None,
            analysis_base_url: "http://127.0.0.1:1".into(),
            python_bin: "python".into(),
            denoise_model: "scunet_color_real_psnr".into(),
            denoise_script: String::new(),
            denoise_raw_script: String::new(),
            weights_dir: String::new(),
            segment_script: String::new(),
            depth_script: String::new(),
            embed_script: String::new(),
            correspond_script: String::new(),
            describe_script: String::new(),
            style_strength: 0.5,
            send_reference_image: false,
        }
    }

    /// The exact input bytes a reimagine of `input` will send at the enum
    /// size — same pipeline (preview_only → Lanczos resize_exact → RGB8 →
    /// PNG), so an "echo" response is pixel-identical to what was sent.
    fn sent_png_for(input: &Path) -> Vec<u8> {
        let base = crate::decode::preview_only(input).unwrap();
        encode_png(&DynamicImage::ImageRgb8(
            base.resize_exact(1024, 1024, FilterType::Lanczos3).to_rgb8(),
        ))
        .unwrap()
    }

    fn bounded_join(handle: std::thread::JoinHandle<()>) {
        let grace = std::time::Instant::now();
        while !handle.is_finished() && grace.elapsed() < std::time::Duration::from_secs(2) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // Assert BEFORE joining (the negotiation test's rule): a stub whose
        // script was not fully consumed still blocks in recv(), and joining
        // it unconditionally HANGS the test instead of failing it — measured
        // live when a mutation made the retry consume one response of two.
        assert!(handle.is_finished(), "the stub script was not fully consumed");
        let _ = handle.join();
    }

    /// Step-6 (a)+(b) on the wire: one loopback reimagine — the multipart
    /// prompt part carries the scaffold AND the user's direction, the report
    /// carries a measured divergence (an echoed input reads faithful), and no
    /// second POST is bought without the opt-in.
    #[test]
    fn a_reimagine_hardens_the_prompt_and_measures_its_result() {
        let dir = std::env::temp_dir().join(format!("autoshade-s6-wire-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // The library layout the read-only guard expects: the source in its
        // OWN folder, outputs in a sibling (see pipeline's preflight tests).
        std::fs::create_dir_all(dir.join("library")).unwrap();
        let input = dir.join("library").join("input.png");
        structured_frame(512, 512).save(&input).unwrap();
        let echo = sent_png_for(&input);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&echo);
        let (url, seen, handle) = stub_endpoint(vec![(
            200,
            "application/json",
            format!(r#"{{"data":[{{"b64_json":"{b64}"}}]}}"#),
        )]);
        let cfg = stub_cfg(url);
        let out = dir.join("exports").join("out.png");
        let report = reimagine(&cfg, &input, "brighten the mood", "high", "auto", false, &out)
            .expect("the loopback reimagine succeeds");
        assert!(out.is_file(), "the artifact landed");
        assert!(
            report.divergence.d < crate::fit::DIVERGENCE_GLOBAL,
            "an echoed input is structurally faithful: D={:.3}",
            report.divergence.d
        );
        assert_eq!(report.first_divergence, None, "no retry was bought");
        bounded_join(handle);
        let bodies = seen.lock().unwrap().clone();
        assert_eq!(bodies.len(), 1, "one billed POST");
        assert!(
            bodies[0].contains("Re-develop this exact photograph"),
            "the scaffold reached the wire"
        );
        assert!(
            bodies[0].contains("Style direction: brighten the mood"),
            "the user's direction rides WITH the scaffold, not instead of it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Step-6 (c): the divergence retry is an explicit OPT-IN, bounded to
    /// one, and keeps whichever result measures closer. First response:
    /// structural noise (diverged); second: the echoed input. Opted in, the
    /// second is bought and the artifact on disk IS the closer result;
    /// opted out, the diverged first answer stands on ONE billed POST — and
    /// its report proves the divergence really was measured (supervisor
    /// mutation MB, the reading degraded to matched, goes red on that line;
    /// MC — retry ignoring the opt-in — buys a POST the stub never scripted
    /// and fails the opt-out arm; MD — keeping the farther result — breaks
    /// the byte-for-byte artifact pin).
    #[test]
    fn the_divergence_retry_is_opt_in_bounded_and_keeps_the_closer_result() {
        let dir = std::env::temp_dir().join(format!("autoshade-s6-retry-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join("library")).unwrap();
        let input = dir.join("library").join("input.png");
        structured_frame(512, 512).save(&input).unwrap();
        let echo = sent_png_for(&input);
        let noise = encode_png(&noise_frame(1024, 1024)).unwrap();
        let echo64 = base64::engine::general_purpose::STANDARD.encode(&echo);
        let noise64 = base64::engine::general_purpose::STANDARD.encode(&noise);

        // Opt-IN: noise then echo → two POSTs, the echo (closer) is kept.
        let (url, seen, handle) = stub_endpoint(vec![
            (200, "application/json", format!(r#"{{"data":[{{"b64_json":"{noise64}"}}]}}"#)),
            (200, "application/json", format!(r#"{{"data":[{{"b64_json":"{echo64}"}}]}}"#)),
        ]);
        let out = dir.join("exports").join("out.png");
        let cfg = stub_cfg(url);
        let report = reimagine(&cfg, &input, "", "high", "auto", true, &out)
            .expect("the opted-in retry succeeds");
        bounded_join(handle);
        assert_eq!(seen.lock().unwrap().len(), 2, "the opt-in bought exactly one more");
        let first = report.first_divergence.expect("the discarded attempt is disclosed");
        assert!(
            first >= crate::fit::DIVERGENCE_GLOBAL,
            "premise: the noise result diverges (D={first:.3})"
        );
        assert!(
            report.divergence.d < first,
            "the kept result measures closer: {:.3} vs {first:.3}",
            report.divergence.d
        );
        let (expected, _) = canonical_generated_png(&echo, "1024x1024").unwrap();
        assert_eq!(
            std::fs::read(&out).unwrap(),
            expected,
            "the artifact on disk IS the closer result"
        );

        // Opt-OUT: the same diverged answer stands on ONE POST, measured.
        let (url2, seen2, handle2) = stub_endpoint(vec![(
            200,
            "application/json",
            format!(r#"{{"data":[{{"b64_json":"{noise64}"}}]}}"#),
        )]);
        let out2 = dir.join("exports").join("out2.png");
        let cfg2 = stub_cfg(url2);
        let report2 = reimagine(&cfg2, &input, "", "high", "auto", false, &out2)
            .expect("the un-opted call succeeds");
        bounded_join(handle2);
        assert_eq!(seen2.lock().unwrap().len(), 1, "no un-opted second purchase");
        assert_eq!(report2.first_divergence, None);
        assert!(
            report2.divergence.d >= crate::fit::DIVERGENCE_GLOBAL,
            "the divergence is truly measured, not assumed: D={:.3}",
            report2.divergence.d
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L09#1 ordering: the output preflight runs BEFORE the source is even
    /// touched — with a nonexistent input and a directory output, the error
    /// must name the directory, not a decode failure (and certainly not a
    /// billed API call; no key, no network here).
    #[test]
    fn reimagine_refuses_a_directory_output_before_touching_the_source() {
        let dir = std::env::temp_dir()
            .join(format!("autoshade-reimagine-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            openai_api_key: None,
            openai_model: String::new(),
            openai_base_url: "http://127.0.0.1:1".into(),
            openai_image_model: String::new(),
            openai_image_quality: "auto".into(),
            openai_image_max_px: 1_000_000,
            image_provider: "api".into(),
            image_effort: None,
            image_api_family: "auto".into(),
            image_gen_endpoint: None,
            analysis_provider: "oauth".into(),
            analysis_model: "opus".into(),
            analysis_effort: None,
            claude_bin: "claude".into(),
            analysis_api_key: None,
            analysis_base_url: "http://127.0.0.1:1".into(),
            python_bin: "python".into(),
            denoise_model: String::new(),
            denoise_script: String::new(),
            denoise_raw_script: String::new(),
            weights_dir: String::new(),
            segment_script: String::new(),
            depth_script: String::new(),
            embed_script: String::new(),
            correspond_script: String::new(),
            describe_script: String::new(),
            style_strength: 0.0,
            send_reference_image: false,
        };
        let e = reimagine(&cfg, Path::new("no-such-input.arw"), "p", "high", "auto", false, &dir)
            .unwrap_err()
            .to_string();
        assert!(e.contains("is a directory"), "preflight must answer first: {e}");
        let e = retouch(&cfg, Path::new("no-such-input.arw"), Path::new("m.png"), "p", "auto", false, &dir)
            .unwrap_err()
            .to_string();
        assert!(e.contains("is a directory"), "retouch too: {e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
