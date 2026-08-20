//! Autoshop — AI-assisted automatic development of RAW photographs.
//!
//! Architecture in one line: the AI advisor looks at a RAW preview + metadata
//! and emits an [`recipe::EditRecipe`]; a deterministic render engine applies
//! that recipe. See `docs/ARCHITECTURE.md` for the full design. Shared advise +
//! output logic lives in [`pipeline`]; the CLI here and the web UI ([`serve`])
//! both call it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use image::GenericImageView;

// The engine modules now live in the `autoshop` library crate (src/lib.rs),
// shared with the native GUI binary (src/bin/gui.rs).
use autoshop::{decode, denoise, eval, fit, generative, pipeline, render, retouch, serve};
use autoshop::advisor::Verdict;
use autoshop::config::Config;
use autoshop::pipeline::{
    default_out, ensure_parent, find_raws, produce_recipe, stem, write_recipe, write_xmp,
    GradeRequest,
};
use autoshop::recipe::{EditRecipe, GradeStrength};
use autoshop::style::StyleIndex;

#[derive(Parser)]
#[command(
    name = "autoshop",
    version,
    about = "AI-assisted automatic development of RAW photographs",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Decode a RAW: extract its embedded preview, EXIF, and histogram.
    /// Reads the RAW only; writes the preview to ./out (never beside the source).
    Decode {
        /// Path to the RAW file (e.g. .ARW, .DNG).
        raw: PathBuf,
        /// Preview output path (default: ./out/<stem>.preview.jpg).
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Decode a RAW, ask the AI advisor to propose an edit, have Claude verify
    /// it, and write the recipe JSON + a Lightroom .xmp sidecar (no render).
    /// R20: also runs the visual closed loop — the proposal is RENDERED and
    /// judged by the vision model, which may buy ONE guided revision (or up to
    /// three, with --deep or a high --strength; extra vision cost per run,
    /// batch/eval skip this entirely).
    Analyze {
        /// Path to the RAW file.
        raw: PathBuf,
        /// Where to write the recipe JSON (default: this photo's develop-store
        /// dir, printed as `recipe -> …` on completion).
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Optional direction for the AI, e.g. "warmer and moodier, lift the
        /// shadows, keep skin natural".
        #[arg(long)]
        guidance: Option<String>,
        /// How strongly to follow your historical edit style, 0..1 (needs a built
        /// `style-index`). Omit to use AUTOSHOP_STYLE_STRENGTH (default 0.3).
        #[arg(long, value_parser = unit_interval)]
        style: Option<f32>,
        /// How COMMITTED the grade should be, 0..1 — a different axis from
        /// `--style`: 0.5 is the calibrated baseline every guardrail was tuned
        /// at, and the default 0.65 pushes a little further. Above 0.7 the AI is
        /// told to commit; the clipping/white-point safeguards never move.
        #[arg(long, value_parser = unit_interval)]
        strength: Option<f32>,
        /// DEEP THINKING: the proposer first states the scene, decides each tool
        /// family explicitly and names the look it is going for, then critiques
        /// its own answer (printed here in full); its reasoning tier goes up one
        /// step, and the visual judge may run up to 2-3 guided rounds instead of
        /// one. COSTS MORE: a normal analyze is at worst 11 API calls (6 with
        /// images, 8 high-detail); with --deep at a committed strength it is at
        /// worst 17 calls (10 with images, 14 high-detail), plus ~10-20% more
        /// output tokens per proposal. `batch` never does this.
        #[arg(long)]
        deep: bool,
    },
    /// Render an existing EditRecipe onto a RAW and save the developed image.
    Apply {
        /// Path to the RAW file.
        raw: PathBuf,
        /// Path to the recipe JSON produced by `analyze`.
        recipe: PathBuf,
        /// Output image path (extension selects format: .jpg / .png / .tif).
        #[arg(short, long)]
        out: PathBuf,
    },
    /// End-to-end for one RAW: analyze (recipe + xmp) then render an image.
    Auto {
        /// Path to the RAW file.
        raw: PathBuf,
        /// Output image path (default: ./out/<stem>.developed.tif, 16-bit).
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Optional direction for the AI (e.g. "warmer and moodier").
        #[arg(long)]
        guidance: Option<String>,
        /// How strongly to follow your historical edit style, 0..1 (needs a built
        /// `style-index`). Omit for AUTOSHOP_STYLE_STRENGTH (default 0.3).
        #[arg(long, value_parser = unit_interval)]
        style: Option<f32>,
        /// How COMMITTED the grade should be, 0..1 (see `analyze --strength`);
        /// default 0.65, and 0.5 is the calibrated baseline.
        #[arg(long, value_parser = unit_interval)]
        strength: Option<f32>,
        /// DEEP THINKING (see `analyze --deep`): structured working + a raised
        /// reasoning tier + a multi-round visual judge. Costs more per photo.
        #[arg(long)]
        deep: bool,
        /// Run AI denoise (SCUNet, GPU) before developing — for high-ISO/astro.
        #[arg(long)]
        denoise: bool,
        /// Denoise strength 0..1 (blend with original); default 1.0.
        #[arg(long, requires = "denoise", value_parser = unit_interval)]
        denoise_strength: Option<f32>,
        /// SCUNet model: color_real_psnr (default) / color_real_gan / color_15|25|50.
        #[arg(long, requires = "denoise")]
        denoise_model: Option<String>,
    },
    /// AI-denoise a RAW or an already-baked image (PNG/TIFF/JPEG) into a clean
    /// 16-bit master in ./out. Manual, GPU-accelerated (SCUNet sidecar). Default
    /// off everywhere else — this is the explicit "denoise now" command.
    Denoise {
        /// RAW (.arw/.dng/...) or image (.png/.tif/.jpg) to denoise.
        input: PathBuf,
        /// Output path (default: ./out/<stem>.denoised.tif).
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Strength 0..1 (blend with original); default 1.0.
        #[arg(long, value_parser = unit_interval)]
        strength: Option<f32>,
        /// SCUNet model tier (see `auto --denoise-model`).
        #[arg(long)]
        model: Option<String>,
    },
    /// Batch-process every RAW under a folder (resumes by skipping RAWs that
    /// already have a saved develop). Renders go to ./out; develop state goes
    /// to the per-user store — the photo library stays read-only.
    Batch {
        /// Folder to scan recursively for RAW files (24 formats — the same set
        /// `decode::is_raw` defines and the GUI and web open).
        dir: PathBuf,
        /// Also render a 16-bit developed TIFF per RAW (slower, large files).
        #[arg(long)]
        render: bool,
        /// Max RAWs to process this run (cost guard; raise to do more).
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Also process already-baked photos (PNG/TIFF/JPEG/WebP/BMP/GIF), which
        /// the GUI and web open but this scan skips by default. OPT-IN on
        /// purpose: shooting RAW+JPEG is common, and including baked sources by
        /// default would analyze — and BILL — the camera JPEG beside every RAW.
        #[arg(long)]
        include_baked: bool,
        /// Photos to develop CONCURRENTLY. Each photo is dominated by blocking
        /// network round trips, so workers overlap that wait; the default 3 is
        /// the concurrency this command has shipped with since R26. Whatever
        /// you ask for, a memory budget caps it — one 61 MP photo peaks at
        /// ~1.8 GB (`jobs::PER_PHOTO_PEAK_COMMIT_MB`) — and the run says so
        /// when it does.
        #[arg(long, default_value_t = 3)]
        jobs: usize,
    },
    /// Evaluate AI edits against your own: for RAWs that have a sibling .xmp
    /// (your Lightroom/ACR edit), run the AI and report per-field error + bias.
    Eval {
        /// Folder to scan recursively for RAW + .xmp pairs.
        dir: PathBuf,
        /// Max photos to evaluate (cost guard; each one runs the AI).
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Photos to evaluate CONCURRENTLY. Default 1 = the serial harness,
        /// unchanged. Above 1 the per-photo lines are held and printed in index
        /// order, and the report is folded in index order too, so the table and
        /// the gap score do not depend on which photo finished first. A memory
        /// budget caps this — one 61 MP photo peaks at ~1.8 GB — and the run
        /// says so when it does.
        #[arg(long, default_value_t = 1)]
        jobs: usize,
    },
    /// Build the style index from your edited library (RAW+.xmp pairs) → the
    /// advisor then references your edits on similar shots. Run once / on update.
    StyleIndex {
        /// Folder to scan recursively for RAW + .xmp pairs (your edits).
        dir: PathBuf,
    },
    /// EXPERIMENTAL: full-frame generative restyle via OpenAI Images (low-res,
    /// lossy re-render — NOT a master; the XMP/render path is the real workflow).
    Reimagine {
        /// Path to the RAW file.
        raw: PathBuf,
        /// What to do (e.g. "moody cinematic, deepen shadows, warm highlights").
        #[arg(long)]
        prompt: String,
        /// "high" keeps it recognizably the same photo; "low" = free rein.
        #[arg(long, default_value = "high")]
        fidelity: String,
        /// Output quality tier: low | medium | high | auto (higher = more detail,
        /// higher cost). Defaults to AUTOSHOP_IMAGE_QUALITY (config default: high).
        #[arg(long)]
        quality: Option<String>,
        /// Output PNG (default: ./out/<stem>.reimagine.png).
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Reverse-fit a LOOK into an editable recipe: given the SAME shot twice —
    /// the source and a target rendition (e.g. the `reimagine` output, or any
    /// finished reference of this frame) — solve for the EditRecipe that
    /// reproduces the target through the deterministic engine, and write the
    /// recipe JSON + Lightroom XMP. No pixels are copied, so the result applies
    /// at FULL sensor resolution. Deterministic; no API key needed.
    Match {
        /// Source RAW (or baked image) the look should be fitted onto.
        raw: PathBuf,
        /// The look to match — e.g. ./out/<stem>.reimagine.png.
        target: PathBuf,
        /// Also render the fitted recipe at full resolution
        /// (./out/<stem>.matched.tif, 16-bit).
        #[arg(long)]
        render: bool,
        /// Add a sky-to-sky ZONED correction on top of the global fit: segment
        /// the sky in both images (python sidecar, local) and attach a
        /// bitmap-masked local adjustment. Rendered in-app / by our engine;
        /// the Lightroom XMP carries the global fit only (classic sidecars
        /// cannot hold raster masks). Falls back to the plain global fit with
        /// a note if segmentation is unavailable.
        #[arg(long)]
        zoned: bool,
        /// Also extract a reusable style PROMPT from the pair via the vision
        /// model (./out/<stem>.style.txt; needs OPENAI_API_KEY).
        #[arg(long)]
        style_prompt: bool,
        /// R20: after the fit, have the vision model SCORE how faithfully the
        /// fitted render matches the target look (0-100 + critique — LLM as a
        /// judge). One paid vision call; needs OPENAI_API_KEY. Informational:
        /// a review failure never fails the fit.
        #[arg(long)]
        ai_judge: bool,
        /// R23-6: let that review ACT — one bounded guided retry, kept only if
        /// it re-scores at least as high (the reviewer picks WHICH of this
        /// app's moves to try, never the values). Implies --ai-judge; up to
        /// two paid vision calls. Ordering is already correct here: the CLI
        /// has always evaluated before it writes.
        #[arg(long)]
        deep: bool,
        /// Named recipe JSON artifact for `apply` (default:
        /// ./out/<stem>.matched.json). The canonical recipe.json in this
        /// photo's develop store — what the GUI and web restore — is ALWAYS
        /// written too.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// EXPERIMENTAL: generative object removal via OpenAI Images. The mask is an
    /// RGBA PNG; transparent pixels mark the region to regenerate.
    Retouch {
        /// Path to the RAW file.
        raw: PathBuf,
        /// RGBA PNG mask (transparent = region to edit).
        #[arg(long)]
        mask: PathBuf,
        /// What to do (e.g. "remove the trash can, fill with pavement").
        #[arg(long)]
        prompt: String,
        /// Output quality tier: low | medium | high | auto (higher = more detail,
        /// higher cost). Defaults to AUTOSHOP_IMAGE_QUALITY (config default: high).
        #[arg(long)]
        quality: Option<String>,
        /// Composite onto the full-sensor develop (e.g. 61 MP) instead of a
        /// ≤2048px develop — the untouched area keeps native resolution. Slow;
        /// the regenerated patch is upscaled. No effect on baked PNG/TIFF sources.
        #[arg(long)]
        full_res: bool,
        /// Output PNG (default: ./out/<stem>.retouch.png).
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// OPTIONAL pixel-retouch mode — AI-driven HEAL (spot / blemish / dust
    /// removal) that edits pixels directly by sampling SURROUNDING REAL pixels.
    /// Retouching, NOT generation: no gpt-image, no invented content. Non-XMP;
    /// writes a pixel master to ./out. Targeting is hybrid (AI auto-detect and/or
    /// a painted mask).
    Heal {
        /// RAW or baked image (.png/.tif/.jpg) to retouch.
        src: PathBuf,
        /// Optional painted RGBA mask (transparent = heal here) to ADD manual targets.
        #[arg(long)]
        mask: Option<PathBuf>,
        /// Skip AI auto-detection (heal only the painted mask).
        #[arg(long)]
        no_auto: bool,
        /// Retouch at FULL resolution instead of a ≤2048px base — the
        /// full-sensor develop (e.g. 61 MP) for a RAW, the image itself for
        /// a baked PNG/TIFF. Slow. WITHOUT it a baked input is thumbnailed
        /// to 2048px and the saved master IS that thumbnail.
        #[arg(long)]
        full_res: bool,
        /// Output image (default: ./out/<stem>.heal.png).
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Start the local web UI (open the printed URL in a browser).
    Serve {
        /// Photo library folder to browse (scanned recursively for .ARW).
        dir: PathBuf,
        /// Port to listen on.
        #[arg(short, long, default_value_t = 8080)]
        port: u16,
    },
    /// Print the default EditRecipe as JSON — the exact shape the AI must emit.
    RecipeSchema,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Decode { raw, out } => decode_cmd(&raw, out),
        Command::Analyze { raw, out, guidance, style, strength, deep } => {
            analyze_cmd(&raw, out, guidance, style, strength, deep)
        }
        Command::Apply { raw, recipe, out } => apply_cmd(&raw, &recipe, &out),
        Command::Auto {
            raw,
            out,
            guidance,
            style,
            strength,
            deep,
            denoise,
            denoise_strength,
            denoise_model,
        } => {
            auto_cmd(
                &raw, out, guidance, style, strength, deep, denoise, denoise_strength,
                denoise_model,
            )
        }
        Command::Denoise { input, out, strength, model } => denoise_cmd(&input, out, strength, model),
        Command::Batch { dir, render, limit, include_baked, jobs } => {
            batch_cmd(&dir, render, limit, include_baked, jobs)
        }
        Command::Eval { dir, limit, jobs } => eval::run(&dir, limit, jobs),
        Command::StyleIndex { dir } => style_index_cmd(&dir),
        Command::Reimagine { raw, prompt, fidelity, quality, out } => {
            let cfg = Config::load();
            let out = out.unwrap_or_else(|| default_out(&raw, "reimagine", "png"));
            // Full pre-pay preflight (L10 family): the first API call comes
            // only after a full-size decode+resize+encode, so every CERTAIN
            // failure — key, closed-set tiers, output path — refuses first.
            pipeline::preflight_out(&out, &raw)?; // includes the read-only-library guard
            require_choice("--fidelity", &fidelity, &["high", "low"])?;
            let q = quality.unwrap_or_else(|| cfg.openai_image_quality.clone());
            require_choice("--quality (or the configured default)", &q, &["low", "medium", "high", "auto"])?;
            require_image_key(&cfg, "reimagine")?;
            generative::reimagine(&cfg, &raw, &prompt, &fidelity, &q, &out)
        }
        Command::Match { raw, target, render, zoned, style_prompt, ai_judge, deep, out } => {
            // --deep IS the review, iterated: asking for the loop without the
            // reviewer is not a configuration, it is a typo.
            match_cmd(&raw, &target, render, zoned, style_prompt, ai_judge || deep, deep, out)
        }
        Command::Retouch { raw, mask, prompt, quality, full_res, out } => {
            let cfg = Config::load();
            let out = out.unwrap_or_else(|| default_out(&raw, "retouch", "png"));
            // Full pre-pay preflight — see Reimagine.
            pipeline::preflight_out(&out, &raw)?; // includes the read-only-library guard
            if !mask.is_file() {
                anyhow::bail!(
                    "--mask {} does not exist — checked before any decode or paid call",
                    mask.display()
                );
            }
            let q = quality.unwrap_or_else(|| cfg.openai_image_quality.clone());
            require_choice("--quality (or the configured default)", &q, &["low", "medium", "high", "auto"])?;
            require_image_key(&cfg, "retouch")?;
            generative::retouch(&cfg, &raw, &mask, &prompt, &q, full_res, &out)
        }
        Command::Heal { src, mask, no_auto, full_res, out } => heal_cmd(&src, mask, no_auto, full_res, out),
        Command::Serve { dir, port } => serve::serve(&dir, port),
        Command::RecipeSchema => {
            let template = EditRecipe::default();
            println!("{}", serde_json::to_string_pretty(&template)?);
            Ok(())
        }
    }
}

fn style_index_cmd(dir: &Path) -> Result<()> {
    let index = StyleIndex::build(dir)?;
    // Central per-user location: the index describes the user's whole library,
    // so it must not depend on which directory the command ran from.
    let out = autoshop::store::style_index_path();
    index.save(&out)?;
    println!(
        "style index → {} ({} exemplars). The advisor will now reference your edits on similar shots.",
        out.display(),
        index.exemplars.len()
    );
    Ok(())
}

fn decode_cmd(raw: &Path, out: Option<PathBuf>) -> Result<()> {
    let decoded = decode::decode_any(raw)?;

    let out = out.unwrap_or_else(|| default_out(raw, "preview", "jpg"));
    pipeline::guard_readonly(&out, raw)?;
    ensure_parent(&out)?;
    let preview = decoded.preview_resized(1536);
    let format = image::ImageFormat::from_path(&out)
        .with_context(|| format!("unsupported preview format {}", out.display()))?;
    render::stage_and_publish(&out, |staged| {
        preview
            .save_with_format(staged, format)
            .with_context(|| format!("save preview {}", out.display()))
    })?;
    let (pw, ph) = preview.dimensions();

    let m = &decoded.meta;
    let dash = || "-".to_string();
    println!("RAW: {}", raw.display());
    println!("  camera : {} {}", m.make, m.model);
    println!("  lens   : {}", m.lens.as_deref().unwrap_or("-"));
    println!(
        "  expo   : ISO {}  {}  f/{}  {}mm  EV{:+.1}",
        m.iso.map(|v| v.to_string()).unwrap_or_else(dash),
        m.shutter.as_deref().unwrap_or("-"),
        m.aperture.map(|v| format!("{v:.1}")).unwrap_or_else(dash),
        m.focal_length_mm.map(|v| format!("{v:.0}")).unwrap_or_else(dash),
        m.exposure_bias_ev.unwrap_or(0.0),
    );
    // R27: `meta.width/height` describe the DELIVERED preview (the L05-6 rule
    // in decode.rs), not the sensor — the old `sensor :` label printed a
    // Pentax as 640 x 424. Labelled for what it is.
    println!("  preview: {} x {}", m.width, m.height);
    println!(
        "  wb     : [{:.3}, {:.3}, {:.3}, {:.3}]",
        m.as_shot_wb_coeffs[0], m.as_shot_wb_coeffs[1], m.as_shot_wb_coeffs[2], m.as_shot_wb_coeffs[3],
    );
    println!("  date   : {}", m.date_time.as_deref().unwrap_or("-"));

    let h = &decoded.histogram;
    println!(
        "  clip   : black {:.2}%   white {:.2}%   ({} px sampled)",
        h.clip_black_pct, h.clip_white_pct, h.sample_pixels,
    );
    println!("  luma   : {}", sparkline(&h.luma));
    println!(
        "  xmp    : {}",
        if decoded.embedded_xmp.is_some() { "embedded packet present" } else { "none" },
    );
    println!("  preview -> {} ({} x {})", out.display(), pw, ph);
    Ok(())
}

/// Clap parser for the 0..=1 strength flags: finite AND in-domain. "NaN"
/// parsed as a valid f32 and silently disabled style; `--style 2` reported
/// 200% while clamping to the same result as 1 (16-lane scan L09).
fn unit_interval(s: &str) -> Result<f32, String> {
    let v: f32 = s.parse().map_err(|_| format!("`{s}` is not a number"))?;
    if !v.is_finite() || !(0.0..=1.0).contains(&v) {
        return Err(format!("`{s}` is outside 0..=1 (a finite fraction)"));
    }
    Ok(v)
}

/// Same file? canonicalize when it exists (kills case/alias spellings — the
/// dangerous overwrite case implies existence), else lexical absolute. The
/// canonical-vs-`-o` decision must not depend on how the path was spelled.
fn same_path(a: &Path, b: &Path) -> bool {
    let n = |p: &Path| {
        // Lexical `..` fold + deepest-existing-ancestor canonicalization:
        // plain canonicalize fails when the LEAF is absent (an XMP-only save
        // has no recipe.json yet), and a case-flipped or junction-aliased -o
        // then classified as redirected and bypassed the backup gate.
        pipeline::resolve_existing_pub(&pipeline::normalize_lexical(
            &std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()),
        ))
    };
    let (na, nb) = (n(a), n(b));
    if na == nb {
        return true;
    }
    // Canonicalization fixes casing only for the EXISTING ancestor chain —
    // absent tail components come back spelled verbatim. On Windows a
    // case-flipped absent leaf ("Recipe.json" for a first, not-yet-written
    // recipe.json) still names the same future file (NTFS is
    // case-insensitive), and classifying it as redirected bypassed the
    // backup gate. Folding case widens equality toward the safe side —
    // canonical treatment means the gate applies — EXCEPT inside an NTFS
    // dir with per-directory case sensitivity enabled (WSL-created,
    // exotic), where two genuinely distinct case-variant leaves conflate:
    // accepted trade-off, since default-insensitive NTFS is the case that
    // bypassed the gate.
    if cfg!(windows) {
        return na.to_string_lossy().to_lowercase() == nb.to_string_lossy().to_lowercase();
    }
    false
}

/// What the photo's own Lightroom sidecar costs on the way IN, as one English
/// sentence, or `None` when there is no such sidecar / nothing was lost.
///
/// R27 (L-07) converts the R25 P9 registration「CLI discloses nothing about
/// mask import loss」into a channel. `xmp::describe_import_losses` had exactly
/// two callers, both under `bin/gui`: a CLI run over a photo whose sidecar
/// holds a brush mask, a Subtract component or a rotation this build cannot
/// fold said nothing at all about it, while the GUI said it on every open.
///
/// The sentence is a property of the FILE, so it is computed from the same
/// source `pipeline::write_xmp` merges over — Lightroom's own sidecar beside
/// the RAW — and printed on the same stderr channel the export-side loss line
/// uses (`pipeline::write_xmp_doc`). ONE producer, so no CLI command can print
/// a different (or no) version of it.
///
/// `None` for a baked source (a PNG's neighbouring `.xmp` is someone else's
/// file — the line `store::lightroom_sidecar` and `write_xmp` both draw) and
/// for an UNREADABLE sidecar, which `write_xmp` already discloses with the
/// reason.
fn lightroom_import_note(raw: &Path) -> Option<String> {
    if !decode::is_raw(raw) {
        return None;
    }
    let lr = raw.with_extension("xmp");
    let autoshop::store::SidecarRead::Ok(text) = autoshop::store::read_sidecar_checked(&lr) else {
        return None;
    };
    let losses = autoshop::xmp::import_losses(&text);
    // The count the reader WOULD carry — the same number the GUI's open path
    // passes (`bin/gui/export.rs`), so the two surfaces cannot report the same
    // file differently. Taken through the CLAMPED door (R28 2b) because the
    // third sentence below needs what the size caps cut, and re-parsing the
    // document to learn it would be a second producer of one fact.
    let (parsed, clamped) = autoshop::xmp::xmp_to_recipe_clamped(&text);
    let imported = parsed.masks.len();
    // The mask sentence and the CROP sentence (R27) ride together: a file can
    // lose either, both or neither, and a `?` on the first would have swallowed
    // the second on every sidecar whose masks arrived whole. The CLAMP
    // sentence (R28 2b) joins them on the same terms: `import_losses` reads
    // the DOCUMENT and cannot see what the recipe's own size caps then cut —
    // a 393 KB dab stream truncated to 256 KiB is a real change to the mask
    // this photo will render, and it used to be said nowhere at all.
    let line = [
        autoshop::xmp::describe_import_losses(imported, &losses),
        autoshop::xmp::crop_import_note(&text),
        (!clamped.is_empty())
            .then(|| format!("recipe limits then discarded {}", clamped.describe())),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    (!line.is_empty()).then(|| format!("reading {}: {}", lr.display(), line.join("; ")))
}

/// The non-output half of the pre-pay preflight (the L09#1 rule made whole
/// — 16-lane scan, L10 family): a requirement the command is CERTAIN to hit
/// is checked before the first paid call or heavyweight decode.
fn require_image_key(cfg: &Config, what: &str) -> Result<()> {
    if cfg.openai_api_key.is_none() {
        anyhow::bail!(
            "{what} needs the image API and no OPENAI_API_KEY (or stored image key) is \
             configured — checked before any decode or paid call"
        );
    }
    Ok(())
}

/// Closed-set CLI values are refused at the door (L10-10): the server used
/// to reject an unknown tier only AFTER the decode/resize/encode work.
fn require_choice(flag: &str, value: &str, allowed: &[&str]) -> Result<()> {
    if allowed.contains(&value) {
        return Ok(());
    }
    anyhow::bail!("{flag} must be one of {} (got {value:?})", allowed.join("|"))
}

/// The two TASTE dials, resolved from `analyze`/`auto`'s flags — ONE place, so
/// the two single-photo commands can never diverge (R23-3).
///
/// `--style` omitted falls back to the configured `AUTOSHOP_STYLE_STRENGTH`, as
/// it always has. `--strength` omitted is the SHIPPED default (0.65), not the
/// calibration point: `autoshop analyze` and a double-clicked GUI must develop
/// the same photo the same way when neither is told otherwise.
fn analyze_request(
    style: Option<f32>,
    strength: Option<f32>,
    deep: bool,
    cfg: &Config,
) -> GradeRequest {
    GradeRequest {
        style: style.unwrap_or(cfg.style_strength),
        send_reference_image: false,
        strength: GradeStrength::from_optional(strength),
        // R23-4: opt-in per invocation, and per invocation only — `batch`
        // builds its request through `GradeRequest::with_style`, which has no
        // way to reach this flag.
        think: deep,
    }
}

fn analyze_cmd(
    raw: &Path,
    out: Option<PathBuf>,
    guidance: Option<String>,
    style: Option<f32>,
    strength: Option<f32>,
    deep: bool,
) -> Result<()> {
    let cfg = Config::load();
    if let Some(o) = &out {
        // Full preflight BEFORE the paid propose+verify (L09#1) — an `-o`
        // pointing at a directory used to bill first and bail at
        // write_recipe, losing the recipe entirely. write_recipe keeps its
        // own idempotent checks for the canonical (out = None) branch.
        pipeline::preflight_out(o, raw)?;
    }
    // CLI analyze always proposes from the original (base = None); the refine /
    // "adjust current edit" path is a web-UI affordance. judge = true: an
    // explicitly invoked single-photo analyze gets the visual closed loop
    // (batch passes false — spend never multiplies silently).
    let req = analyze_request(style, strength, deep, &cfg);
    let (recipe, verdict, _notes) =
        produce_recipe(raw, &cfg, true, guidance.as_deref(), None, req, true)?;
    // Remember whether -o redirected the recipe: the XMP has to follow it (below)
    // so one develop never splits across two folders. `-o` POINTING AT the
    // canonical path IS a canonical write — out.is_some() alone let that
    // spelling skip the backup gate and drop the XMP beside it as
    // recipe.xmp instead of the canonical <stem>.xmp.
    let redirected =
        out.as_ref().is_some_and(|o| !same_path(o, &autoshop::store::recipe_target(raw)));

    println!("\n--- proposed recipe ---");
    println!("{}", serde_json::to_string_pretty(&recipe)?);
    println!("\n--- verdict: {:?} ---", verdict.decision);
    for reason in &verdict.reasons {
        println!("  - {reason}");
    }
    // A non-Accept verdict may not auto-save (user decision): the verifier
    // itself judged the result not ready, so the canonical develop stays
    // untouched. A redirected -o is an explicit destination that never
    // touches the develop — it still writes, whatever the verdict.
    if !redirected && verdict.decision != autoshop::advisor::Decision::Accept {
        println!("\nNOT saved: verdict {:?} — a non-Accept verdict never auto-saves.", verdict.decision);
        println!("  keep it anyway: save the JSON above to a file and render it via  autoshop apply,");
        println!("  or steer with  --guidance \"…\"  and re-run analyze.");
        return Ok(());
    }
    // The v<N> backup gate (store.rs "any surface" contract): a programmatic
    // canonical overwrite snapshots an existing explicit save first and is
    // REFUSED when the snapshot fails — the GUI and web already do this; the
    // CLI used to overwrite ungated. A redirected -o write leaves the
    // canonical develop untouched and needs no gate.
    let save = || -> Result<()> {
    if !redirected
        && let Err(e) = autoshop::store::backup_saved_develop(raw, Some(&recipe))
    {
        anyhow::bail!("refusing to overwrite the saved develop: backing it up failed ({e})");
    }
    let recipe_path = write_recipe(raw, &recipe, out)?;
    println!("\nrecipe -> {}", recipe_path.display());
    // XMP only for a RAW; a baked source (PNG/TIFF) gets the recipe JSON only.
    if decode::is_raw(raw) {
        // With -o the XMP goes BESIDE the redirected recipe (same dir + stem),
        // not to ./out: a lone out/<stem>.xmp is what the GUI/web would restore
        // instead of this recipe, silently dropping everything classic sidecars
        // cannot carry (bitmap masks, recolour gains).
        // The recipe write ALONE decides the saved state (the cross-surface
        // rule): failing the command here reported "analyze failed" for a
        // develop every reader already restores, and scripts then re-ran a
        // paid analysis that had in fact succeeded.
        let projected = if redirected {
            // `-o edit.xmp` makes the derived XMP path the RECIPE's own path:
            // the projection then overwrote the recipe JSON written moments
            // ago while both were reported as separate successful outputs.
            let side = recipe_path.with_extension("xmp");
            if same_path(&side, &recipe_path) {
                Err(anyhow::anyhow!(
                    "-o names {} — the XMP projection would overwrite the recipe; \
                     pass a .json path (the XMP is written beside it)",
                    recipe_path.display()
                ))
            } else {
                pipeline::write_xmp_at(side, &recipe, Some(raw))
            }
        } else {
            write_xmp(raw, &recipe)
        };
        match projected {
            // The merge note AND the mask-projection loss line (if any)
            // already went to stderr in write_xmp_doc — one place, so no CLI
            // command can print a different (or no) version of them.
            Ok((p, _, _)) => println!("xmp    -> {}", p.display()),
            Err(e) => eprintln!("  ⚠ recipe saved, but the Lightroom XMP failed: {e:#}"),
        }
        // …and the IMPORT half of the same disclosure (R27 L-07): what this
        // photo's own Lightroom sidecar would cost on the way in.
        if let Some(m) = lightroom_import_note(raw) {
            eprintln!("⚠ {m}");
        }
        if redirected {
            // Post-store, ./out is only a legacy read fallback — the place the
            // GUI/web actually restore from is the photo's develop dir, so the
            // hint must name THAT path or following it does nothing.
            println!("  (written beside the -o recipe so the pair stays together; the GUI/web restore from");
            println!(
                "   {} — copy both files there to make them see it)",
                autoshop::store::develop_dir(raw).display()
            );
        }
        // R27 A3: this line used to end in a literal ".ARW", so
        // `autoshop analyze shot.CR3` told the photographer to put the sidecar
        // next to a "shot.ARW" that does not exist. The source's OWN file name
        // is the only correct answer — and a sidecar's name must match the
        // photo it belongs to for Lightroom to find it at all.
        let s = stem(raw);
        let photo = raw
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| s.to_string());
        println!("  (the library is read-only — copy {s}.xmp next to {photo} to open it in Lightroom)");
    } else {
        println!("  (baked source — recipe JSON only; XMP applies to RAW in Lightroom)");
    }
    Ok(())
    };
    if redirected {
        save()
    } else {
        autoshop::store::with_develop_lock(
            raw,
            autoshop::store::DevelopLockMode::Wait,
            save,
        )
    }
}

fn apply_cmd(raw: &Path, recipe_path: &Path, out: &Path) -> Result<()> {
    let text = autoshop::store::read_text_capped(recipe_path, autoshop::store::MAX_STORE_JSON)
        .with_context(|| format!("read recipe {}", recipe_path.display()))?;
    let mut recipe: EditRecipe =
        serde_json::from_str(&text).with_context(|| format!("parse recipe {}", recipe_path.display()))?;
    // Store-written recipes reference their rasters by bare file name — anchor
    // them to the recipe's own directory (legacy cwd-relative refs untouched).
    if let Some(base) = recipe_path.parent() {
        autoshop::store::resolve_mask_paths(&mut recipe, base);
    }
    // A recipe FILE may predate the coordinate-frame era, so its crop and
    // masks may be drawn against the sensor frame of a rotated RAW. Migrated
    // HERE, where the file is read — `render_source_checked` below is the
    // shared render funnel and also serves recipes that arrived from a
    // browser (already display-frame), which must never be turned.
    if let Some(c) = pipeline::migrate_recipe_coord_frame(raw, &mut recipe) {
        println!("note: {}", pipeline::coord_migration_note(c));
    }
    // Untrusted input, like any other recipe source: an enormous finite
    // exposure (hand-edited JSON) otherwise reaches powf unbounded.
    let dropped = recipe.clamp();
    if !dropped.is_empty() {
        // describe(): only the non-zero losses — a curve-only truncation used
        // to print "0 mask(s) and 0 mask component(s)" (16-lane scan L16).
        eprintln!(
            "warning: importing {} discarded {}",
            recipe_path.display(),
            dropped.describe()
        );
    }
    // The guard FIRST: a refused -o must not pay a RAW decode for the repair
    // below, nor print a disclosure for a render that never runs.
    pipeline::guard_readonly(out, raw)?;
    // The deliverable-format refusal joins it (L10-11, mirroring auto_cmd):
    // `apply -o x.xyz` used to render minutes of full-resolution pixels and
    // only then learn nothing can encode them.
    image::ImageFormat::from_path(out)
        .with_context(|| format!("unsupported output format {}", out.display()))?;
    // Render from the SAME source every other deliverable uses (auto_cmd,
    // serve, the GUI batch): a saved heal/clone or generative master IS this
    // develop's source, and a recorded master that cannot be honoured refuses
    // with the remedy — `apply` was the one surface that rendered the
    // untouched RAW, silently dropping every baked pixel edit while
    // reporting success. The funnel repairs a washed pre-era curve (after its
    // generated strip) and hands the disclosure back, so the note prints
    // exactly when a repaired curve reaches pixels — the advisory peek this
    // replaces re-ran read_pixel_source for the same answer, doubling its
    // stderr warnings and its .bak recovery on the very path that refuses.
    let (src, relook) = autoshop::store::render_source_checked(raw, &mut recipe)
        .map_err(|m| anyhow::anyhow!(m))?;
    if let Some(note) = relook {
        println!("note: {note}");
    }
    if src != raw {
        println!("  (rendering the saved pixel master {})", src.display());
    }
    // AFTER the master check: a refused apply must not leave a freshly
    // created, empty output directory behind.
    ensure_parent(out)?;
    // AI masks are resolved HERE — at develop time, immediately before the
    // render that needs them — not at import. Importing a Lightroom library
    // must not spawn a segmentation model run per photo, and a re-render costs
    // nothing because the alpha is cached in the photo's develop dir
    // (`segment::resolve_ai_masks`). A failure leaves the mask carried and
    // inert, and says so; it never fails the develop.
    let ai = autoshop::segment::resolve_ai_masks(&Config::load(), &src, &mut recipe);
    if let Some(line) = ai.describe() {
        println!("ai mask : {line}");
    }
    println!("rendering {} with {} ...", raw.display(), recipe_path.display());
    let (w, h) = render::render_to_file(&src, &recipe, out, None, None)?;
    println!("render -> {} ({} x {})", out.display(), w, h);
    Ok(())
}

#[allow(clippy::too_many_arguments)] // one clap subcommand's own flag set
fn auto_cmd(
    raw: &Path,
    out: Option<PathBuf>,
    guidance: Option<String>,
    style: Option<f32>,
    strength: Option<f32>,
    deep: bool,
    denoise: bool,
    denoise_strength: Option<f32>,
    denoise_model: Option<String>,
) -> Result<()> {
    let cfg = Config::load();
    // Validate the render target BEFORE the PAID AI call (and before touching
    // the saved develop): the old comment CLAIMED this, but only the
    // read-only guard was hoisted — the directory refusal and the parent
    // creation still ran post-pay, and an ensure_parent failure lost the
    // billed analysis with nothing saved (L09#1). The deliverable-format
    // refusal is hoisted too, mirroring render_to_file's own post-pay
    // check: `auto -o x.xyz` used to pay, save the develop, then fail the
    // render (refuse-not-degrade for deliverables).
    // Default to a 16-bit TIFF master (highest fidelity); pass -o foo.jpg for a
    // smaller 8-bit file.
    let out = out.unwrap_or_else(|| default_out(raw, "developed", "tif"));
    pipeline::preflight_out(&out, raw)?;
    image::ImageFormat::from_path(&out)
        .with_context(|| format!("unsupported output format {}", out.display()))?;
    let req = analyze_request(style, strength, deep, &cfg);
    // judge = true: `auto` is the explicit one-shot develop of ONE photo —
    // same interactive class as analyze (batch passes false).
    let (recipe, verdict, _notes) =
        produce_recipe(raw, &cfg, true, guidance.as_deref(), None, req, true)?;
    let accepted = verdict.decision == autoshop::advisor::Decision::Accept;
    // Opt-in AI denoise runs inside the render, before tone/sharpen.
    let dn = denoise
        .then(|| denoise::DenoiseOpts::from_config(&cfg, denoise_model, denoise_strength.unwrap_or(1.0)));
    println!(
        "verdict: {:?}; rendering full-resolution ({}){} ...",
        verdict.decision,
        // Bit depth follows the chosen extension — claiming "16-bit" for a
        // requested .jpg was a lie the line above explicitly disclaims.
        // LOWERCASED first: the renderer matches the extension
        // case-insensitively, so `-o shot.JPG` encoded an 8-bit JPEG while
        // this banner announced a 16-bit TIFF — the one line the photographer
        // reads to know what they are getting.
        match out
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("jpg" | "jpeg") => "8-bit JPEG",
            Some("png") => "16-bit PNG",
            _ => "16-bit TIFF",
        },
        if denoise { " with AI denoise" } else { "" }
    );
    // Render from the SAME source the GUI and web use: a saved heal/clone or
    // generative master IS this develop's source (store::render_source_checked, which
    // also strips calibration for a generated master). Rendering the
    // untouched RAW silently dropped every baked pixel edit and produced a
    // file that disagreed with what reopening the photo shows.
    let mut render_recipe = recipe.clone();
    // ONE lock across the persistence compound — the gate, recipe.json, the
    // XMP projection AND the render-source capture. Each store call locks
    // internally, but separately: another process's save could land between
    // the gate and the write (overwritten unversioned), or between the write
    // and the XMP (their newer sidecar stomped by ours), and the source could
    // change under the multi-minute render. The render itself runs OUTSIDE,
    // on the source captured here; its result is printed below in the same
    // order as before.
    // DELIVERABLE: a recorded master that cannot be honoured refuses with the
    // remedy instead of silently rendering the un-retouched RAW (A6).
    let (src, relook, xmp_result) = autoshop::store::with_develop_lock(
        raw,
        autoshop::store::DevelopLockMode::Wait,
        || -> Result<_> {
            if accepted {
                // Same backup gate as analyze: `auto` is a programmatic
                // canonical writer.
                if let Err(e) = autoshop::store::backup_saved_develop(raw, Some(&recipe)) {
                    anyhow::bail!(
                        "refusing to overwrite the saved develop: backing it up failed ({e})"
                    );
                }
                write_recipe(raw, &recipe, None)?;
            }
            let (src, relook) = autoshop::store::render_source_checked(raw, &mut render_recipe)
                .map_err(|m| anyhow::anyhow!(m))?;
            let xmp_result = (accepted && decode::is_raw(raw)).then(|| write_xmp(raw, &recipe));
            Ok((src, relook, xmp_result))
        },
    )?;
    if let Some(note) = relook {
        // Normally None — produce_recipe already repaired through
        // saved_recipe_snapshot — but a recipe that slipped past still gets
        // its disclosure.
        println!("note: {note}");
    }
    if src != raw {
        println!("  (rendering the saved pixel master {})", src.display());
    }
    // Same develop-time resolution as `apply` (see there). `render_recipe` is
    // the render's own copy, so the resolved raster paths never reach the
    // saved recipe from here — the cache is keyed by the photo and the intent,
    // so the next develop finds the same file anyway.
    let ai = autoshop::segment::resolve_ai_masks(&cfg, &src, &mut render_recipe);
    if let Some(line) = ai.describe() {
        println!("ai mask : {line}");
    }
    let (w, h) = render::render_to_file(&src, &render_recipe, &out, dn.as_ref(), None)?;
    println!("render -> {} ({} x {})", out.display(), w, h);
    // XMP only for a RAW (Lightroom reads it beside the RAW); a baked source
    // (PNG/TIFF) gets the recipe JSON only. A projection failure is a WARNING:
    // recipe.json (and the rendered file) already committed.
    if !accepted {
        // A non-Accept verdict may not auto-save (user decision): the render
        // above is this command's explicit deliverable, but the develop and
        // its XMP stay untouched. Print the recipe so nothing is lost.
        println!("develop NOT saved: verdict {:?} — a non-Accept verdict never auto-saves.", verdict.decision);
        println!("--- proposed recipe (render it again via  autoshop apply) ---");
        println!("{}", serde_json::to_string_pretty(&recipe)?);
    } else {
        // Written under the lock above; only SAID here, in the old order.
        match xmp_result {
            // Notes + mask-loss line: stderr, from write_xmp_doc (as above).
            Some(Ok((xmp_path, _, _))) => println!("xmp    -> {}", xmp_path.display()),
            Some(Err(e)) => eprintln!("  ⚠ recipe saved, but the Lightroom XMP failed: {e:#}"),
            None => println!("(baked source — recipe.json only, no XMP)"),
        }
        // The import half (R27 L-07), same producer as `analyze`.
        if let Some(m) = lightroom_import_note(raw) {
            eprintln!("⚠ {m}");
        }
    }
    Ok(())
}

/// Standalone AI denoise: RAW → neutral-developed denoised master, or a baked
/// PNG/TIFF/JPEG → denoised copy. Always writes to ./out (library read-only).
fn denoise_cmd(
    input: &Path,
    out: Option<PathBuf>,
    strength: Option<f32>,
    model: Option<String>,
) -> Result<()> {
    let cfg = Config::load();
    let out = out.unwrap_or_else(|| default_out(input, "denoised", "tif"));
    pipeline::guard_readonly(&out, input)?;
    ensure_parent(&out)?;
    let opts = denoise::DenoiseOpts::from_config(&cfg, model, strength.unwrap_or(1.0));
    if decode::is_raw(input) {
        println!("denoising RAW {} (neutral develop) ...", input.display());
        let (w, h) = render::render_to_file(input, &EditRecipe::default(), &out, Some(&opts), None)?;
        println!("denoised -> {} ({} x {})", out.display(), w, h);
    } else {
        println!("denoising image {} ...", input.display());
        // denoise_active routes baked sources through the oriented
        // load_image working copy — the direct denoise_file path let OpenCV
        // ignore EXIF rotation and drop the tag (permanently sideways out).
        denoise::denoise_active(&opts, input, true, &out)?;
        println!("denoised -> {}", out.display());
    }
    Ok(())
}

/// Reverse-fit: solve for the EditRecipe that maps `raw`'s look onto `target`'s
/// (the same frame, differently developed — e.g. the reimagine output). The
/// deliverables are parametric (recipe JSON + XMP + optional full-res render),
/// so the low-res generative experiment becomes a real, adjustable develop.
#[allow(clippy::too_many_arguments)] // one flag per CLI switch; a struct would just rename them
fn match_cmd(
    raw: &Path,
    target: &Path,
    render_full: bool,
    zoned: bool,
    style_prompt: bool,
    ai_judge: bool,
    deep: bool,
    out: Option<PathBuf>,
) -> Result<()> {
    // BEFORE any fitting/segmentation is paid for: `-o` naming the photo's
    // XMP sidecar wrote the promised recipe JSON there, then the projection
    // below overwrote it — both lines printed success while the named
    // artifact no longer existed (16-lane scan L09). Analyze has the
    // equivalent collision check; the RAW gate matches write_xmp's own.
    if decode::is_raw(raw)
        && let Some(o) = out.as_deref()
        && same_path(o, &pipeline::xmp_target(raw))
    {
        anyhow::bail!(
            "-o names this photo's XMP sidecar ({}) — the XMP projection would overwrite the \
             recipe JSON written there; choose a different output path",
            o.display()
        );
    }
    let src = decode::preview_only(raw)?;
    // THE raw-vs-baked dispatch (R22-1). The target is a finished rendition
    // of this frame — usually a baked file, but "another RAW you developed
    // elsewhere" is a legitimate reference and `decode::load_image` refuses a
    // RAW by name, so it went through this one branch (R23-6, matching the
    // desktop app's reference picker). The cap is comfortably above both
    // consumers (the fit analyses at 384, the judge at 1024) and preserves
    // aspect, which the same-frame check reads.
    const MATCH_REF_EDGE: u32 = 2048;
    let tgt = autoshop::render::source_pixels(target, Some(MATCH_REF_EDGE))?;
    if decode::is_raw(target) {
        // Said out loud, because it is the one way this entry can mislead: a
        // RAW carries no look of its own here. `source_pixels` develops it
        // NEUTRALLY — its sidecar, its Lightroom edits and its develop store
        // are not read — so what is being matched is the camera-neutral
        // render, not "how that photo looks in your catalogue".
        println!(
            "  note: {} is a RAW — it is developed NEUTRALLY as the reference; \
             its own develop settings are not read",
            target.display()
        );
    }
    println!("reverse-fitting {} onto the look of {} …", raw.display(), target.display());
    let run_fit = |seg_on: bool| -> Result<fit::FitReport> {
        Ok(if seg_on {
        // Sky mask lands at the GUI's convention (the photo's develop dir,
        // a FRESH claimed `mask-zone-sky*.png` per run — see
        // store::claim_raster: rewriting one fixed name in place left the
        // still-live saved recipe pointing at replaced bytes whenever the
        // canonical write below failed); the GUI shows that raster because
        // the canonical recipe written below REFERENCES this path (masks are
        // attached by reference, never found by filename), so the two must
        // be written together.
        let cfg = Config::load();
        let seg = autoshop::segment::SegmentOpts::from_config(&cfg, "sky");
        // No pre-fit snapshot anymore: claimed unique rasters mean the fit
        // touches nothing the saved develop references, and gating MINUTES
        // before the write left a race window — a save landing during the
        // segmentation was then overwritten unversioned. The canonical write
        // below gates for the zoned path too, immediately before writing.
        let mask = autoshop::store::claim_raster(raw, "mask-zone-sky")?;
        pipeline::guard_readonly(&mask, raw)?;
        println!("  zoned: segmenting the sky in both images (local python sidecar) …");
        autoshop::fit_zoned::fit_recipe_zoned(&src, &tgt, &seg, &mask)
        } else {
            fit::fit_recipe(&src, &tgt)
        })
    };
    let mut rep = run_fit(zoned)?;
    // Calibration stamp, ONE snapshot (produce_recipe's rule): the fit solved
    // against the camera's embedded preview — the very base the base curve
    // approximates — but the deliverable renders from the NEUTRAL sensor
    // develop, so without the curve the fitted deltas landed on a much darker
    // base and the render disagreed with the fit's own numbers. The lens
    // profile and the as-shot WB anchor ride the same snapshot (the fitted
    // recipe was built from EditRecipe::default and silently dropped both).
    // The GUI 反推 worker goes one step further: it COMPOSES the calibration
    // into the solve itself (`fit_recipe_from` + `pipeline::calibration_recipe`,
    // v0.25.0), so its deliverable carries the calibration with no stamp. The
    // CLI keeps the embedded-preview source + this post-stamp on purpose
    // (`preview_only` needs no demosaic, and this command's contract was
    // validated on it).
    // R20 opt-in AI review (LLM-as-a-judge), rendered PRE-stamp on purpose:
    // this fit solved its deltas ON the embedded preview (the base look is in
    // those pixels), so the judge must see develop_preview(preview, deltas) —
    // after the calibration stamp below the same render would apply the base
    // curve a second time. Informational: a failure warns, never errs.
    const JUDGE_EDGE: u32 = 1024; // detail:high tiles at 512 px — 4 tiles read a grade
    let judge_of = |recipe: &EditRecipe| -> Result<autoshop::advisor::Judgement> {
        let cfg = Config::load();
        let enc = |img: &image::DynamicImage| -> Result<Vec<u8>> {
            let mut j = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut j), image::ImageFormat::Jpeg)?;
            Ok(j)
        };
        let fitted =
            autoshop::render::develop_preview(&src.thumbnail(JUDGE_EDGE, JUDGE_EDGE), recipe);
        let t = enc(&tgt.thumbnail(JUDGE_EDGE, JUDGE_EDGE))?;
        let f = enc(&fitted)?;
        Ok(autoshop::advisor::judge_pair(
            &cfg,
            autoshop::advisor::JudgeImages { reference: &t, candidate: &f },
            autoshop::advisor::JudgeTask::FitMatch,
            None,
            // No grade intent: FitMatch scores how closely two renders
            // MATCH, a question the strength axis cannot change (R23-3).
            None,
        )?)
    };
    let judged = if ai_judge {
        match judge_of(&rep.recipe) {
            // --deep (R23-6): let the review ACT, once, bounded. The ordering
            // question that forced a decision in the GUI does not arise here —
            // this command has always evaluated before it writes (everything
            // below the recipe printout) — so `--deep` only adds the retry.
            // Discipline copied from `pipeline::visual_review_round`: the
            // retry must re-score AT LEAST as high, an action that changes
            // nothing short-circuits before a second call is bought, and every
            // failure keeps the plain solve.
            Ok(first) if deep => {
                let action = autoshop::advisor::hint_action(
                    first.hint.as_deref().unwrap_or(""),
                    !rep.recipe.masks.is_empty(),
                    !zoned,
                );
                println!(
                    "  deep: first review {:.0}/100 — trying: {}",
                    first.score,
                    action.tag()
                );
                let candidate = match action {
                    autoshop::advisor::FitAction::Zoned => run_fit(true).ok(),
                    autoshop::advisor::FitAction::Saturation(d) => {
                        let mut r = rep.recipe.clone();
                        r.saturation += d;
                        r.clamp();
                        // RE-DERIVED from the adjusted recipe, never cloned off
                        // the solve — the same defect and the same fix as the
                        // GUI's deep block (R23 review MED-3): a cloned note
                        // set describes the settings the recipe had BEFORE the
                        // step, and this one is persisted into recipe.json.
                        (r.saturation != rep.recipe.saturation)
                            .then(|| fit::rescore_report(&src, &tgt, &r, rep.err_before, &rep.notes))
                    }
                    autoshop::advisor::FitAction::None => None,
                };
                match candidate {
                    Some(mut cand) => match judge_of(&cand.recipe) {
                        Ok(second) if second.score >= first.score => {
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
                            println!(
                                "  deep: the retry re-scored {:.0}/100 — adopted",
                                second.score
                            );
                            rep = cand;
                            Some(Ok(second))
                        }
                        Ok(second) => {
                            println!(
                                "  deep: the retry re-scored {:.0}/100 (lower) — discarded",
                                second.score
                            );
                            Some(Ok(first))
                        }
                        Err(e) => {
                            eprintln!("  ⚠ deep: the retry could not be re-judged ({e:#}) — discarded");
                            Some(Ok(first))
                        }
                    },
                    None => {
                        println!("  deep: nothing to try — the plain fit stands");
                        Some(Ok(first))
                    }
                }
            }
            other => Some(other),
        }
    } else {
        None
    };
    let cal = pipeline::fit_calibration(raw);
    pipeline::stamp_fit_calibration(&mut rep.recipe, cal);
    println!(
        "  look error {:.3} → {:.3}  (0 = identical distributions; masks/local edits are not recoverable)",
        rep.err_before, rep.err_after
    );
    match judged {
        Some(Ok(j)) => println!(
            "  AI review: match {:.0}/100 ({:?}) — {}",
            j.score, j.decision, j.critique
        ),
        Some(Err(e)) => eprintln!("  ⚠ AI review unavailable ({e:#}) — the fit itself stands"),
        None => {}
    }
    println!("--- fitted recipe ---");
    println!("{}", serde_json::to_string_pretty(&rep.recipe)?);

    let out = out.unwrap_or_else(|| default_out(raw, "matched", "json"));
    pipeline::guard_readonly(&out, raw)?;
    // ONE lock across the whole persistence compound (both recipe homes, the
    // pixel-link clear, the XMP): the gates and writers each lock internally
    // but separately, so another process's save could land between a gate and
    // its write and be overwritten unversioned — or leave recipe, master link
    // and sidecar written around a foreign writer's output.
    autoshop::store::with_develop_lock(raw, autoshop::store::DevelopLockMode::Wait, || -> Result<()> {
    // `-o` spelled AS the canonical path is a canonical overwrite: the
    // canonical branch below then SKIPS (recipe_path == canonical), so its
    // gate must run HERE, before the first write destroys the save.
    if same_path(&out, &autoshop::store::recipe_target(raw))
        && let Err(e) = autoshop::store::backup_saved_develop(raw, Some(&rep.recipe))
    {
        anyhow::bail!("refusing to overwrite the saved develop: backing it up failed ({e})");
    }
    // The canonical publish is ONE single-generation commit: recipe + the
    // pixel-link CLEAR (the GUI reverse-fit pairing, L03). This fit was
    // solved from the ORIGINAL source, so it describes the RAW — not a
    // previously saved heal/generative master — and a stale link surviving a
    // kill between the two writes made every later open apply the new look
    // ON TOP of pixels it was never fitted to (16-lane scan L09/L13).
    let commit_canonical = || -> Result<()> {
        autoshop::store::commit_develop(
            raw,
            autoshop::store::DevelopCommit {
                recipe: Some(pipeline::recipe_store_bytes(raw, &rep.recipe)?),
                pixels: autoshop::store::CommitMember::Clear,
                // R24-4: `match` publishes a REVERSE-FIT into the active
                // slot, so a strip record left saying 「original」 would
                // reopen the fit in the GUI as 「▣ 原片」. The CLI owns no
                // strip — the shared primitive restates the one fact this
                // write knows and leaves the card's id/name/position alone
                // (and stays `Keep` when the photo has no strip record).
                variants: autoshop::store::variants_member(
                    raw,
                    autoshop::store::ActiveWrite::Kind("fitted"),
                )?,
            },
        )?;
        Ok(())
    };
    let canonical = autoshop::store::recipe_target(raw);
    let recipe_path = if same_path(&out, &canonical) {
        // `-o` spelled AS the canonical path: the gate above already ran, and
        // the commit below IS the canonical write.
        commit_canonical()?;
        canonical.clone()
    } else {
        write_recipe(raw, &rep.recipe, Some(out))?
    };
    println!("recipe -> {}", recipe_path.display());
    // ALSO write the canonical sidecar. The store's recipe.json is the ONLY
    // recipe the GUI (read_saved_develop) and the web (/api/recipe) read back,
    // and the XMP below cannot carry the zoned result at all (raster masks,
    // colour gains, mask roles are recipe-only) — without this, `match --zoned`
    // produced a full fit that no surface able to render it could ever load.
    // same_path, not string equality: a case-flipped / junction-aliased -o
    // naming the canonical file already wrote it above — writing it a second
    // time (and printing "overwrites any earlier develop") was the string
    // comparison's misclassification.
    if !same_path(&canonical, &recipe_path) {
        pipeline::guard_readonly(&canonical, raw)?;
        // BOTH fit flavours gate here, immediately before the write — the
        // old pre-fit zoned snapshot left the entire segmentation as a race
        // window for a concurrent save.
        if let Err(e) = autoshop::store::backup_saved_develop(raw, Some(&rep.recipe)) {
            anyhow::bail!("refusing to overwrite the saved develop: backing it up failed ({e})");
        }
        commit_canonical()?;
        println!(
            "sidecar-> {} (what the GUI/web restore; overwrites any earlier develop)",
            canonical.display()
        );
    }
    if decode::is_raw(raw) {
        // Warning, not failure: the recipe above already committed.
        match write_xmp(raw, &rep.recipe) {
            // Notes + mask-loss line: stderr, from write_xmp_doc (as above).
            Ok((xmp_path, _, _)) => {
                let s = stem(raw);
                println!("xmp    -> {} (copy {s}.xmp beside {s}.ARW for Lightroom)", xmp_path.display());
            }
            Err(e) => eprintln!("  ⚠ recipe saved, but the Lightroom XMP failed: {e:#}"),
        }
        // The import half (R27 L-07), same producer as `analyze`.
        if let Some(m) = lightroom_import_note(raw) {
            eprintln!("⚠ {m}");
        }
    }
    Ok(())
    })?;
    if render_full {
        let img_out = default_out(raw, "matched", "tif");
        pipeline::guard_readonly(&img_out, raw)?;
        ensure_parent(&img_out)?;
        println!("rendering the fitted recipe at full resolution …");
        let (w, h) = render::render_to_file(raw, &rep.recipe, &img_out, None, None)?;
        println!("render -> {} ({w} x {h})", img_out.display());
    }
    if style_prompt {
        let cfg = Config::load();
        let p_out = default_out(raw, "style", "txt");
        pipeline::guard_readonly(&p_out, raw)?;
        ensure_parent(&p_out)?;
        // Small uploads are plenty for a style read (and cheap): ~0.5 MP each.
        let jpg = |img: &image::DynamicImage| -> Result<Vec<u8>> {
            let mut buf = Vec::new();
            image::DynamicImage::ImageRgb8(img.thumbnail(768, 768).to_rgb8())
                .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Jpeg)
                .context("encode style-prompt jpeg")?;
            Ok(buf)
        };
        println!("extracting a reusable style prompt ({}) …", cfg.openai_model);
        let prompt = autoshop::advisor::describe_style(&cfg, &jpg(&src)?, &jpg(&tgt)?)?;
        render::stage_and_publish(&p_out, |staged| {
            std::fs::write(staged, &prompt)
                .with_context(|| format!("write {}", p_out.display()))
        })?;
        println!("--- style prompt (reusable as a reimagine Direction) ---");
        println!("{prompt}");
        println!("style  -> {}", p_out.display());
    }
    Ok(())
}

/// OPTIONAL pixel-retouch (heal) mode: AI auto-detects and/or a painted mask, and
/// the deterministic engine heals each spot from surrounding real pixels. Writes
/// a pixel master to ./out — non-XMP (pixel edits don't serialise to ACR).
fn heal_cmd(
    src: &Path,
    mask: Option<PathBuf>,
    no_auto: bool,
    full_res: bool,
    out: Option<PathBuf>,
) -> Result<()> {
    let cfg = Config::load();
    let out = out.unwrap_or_else(|| default_out(src, "heal", "png"));
    // Full pre-pay preflight (L10 family): `heal --mask missing.png` used to
    // run the PAID auto-detect first and die at the mask read; `--no-auto`
    // with no mask decoded the whole photo to conclude "nothing to heal".
    // Every certain failure refuses before the paid call and the decode —
    // the same shape analyze_cmd/auto_cmd already have.
    pipeline::preflight_out(&out, src)?; // includes the read-only-library guard
    // DECODED, not merely present (review R12-06): a corrupt mask billed
    // the auto-detect and then died at its own deterministic read. The mask
    // will be decoded anyway; paying its bounded decode up front converts a
    // certain post-pay failure into a free refusal.
    if let Some(m) = &mask {
        autoshop::render::open_mask_bounded(m).with_context(|| {
            format!("--mask {} cannot be used — checked before the paid auto-detect", m.display())
        })?;
    }
    if no_auto && mask.is_none() {
        anyhow::bail!("--no-auto heals only a painted mask, and no --mask was given — nothing to do");
    }
    if !no_auto {
        require_image_key(&cfg, "heal's AI auto-detect (use --no-auto with --mask to heal without it)")?;
    }
    println!(
        "pixel retouch (heal) — {}{} ...",
        if no_auto { "painted mask only" } else { "AI auto-detect" },
        if mask.is_some() && !no_auto { " + painted mask" } else { "" }
    );
    let report = retouch::heal(&cfg, src, mask.as_deref(), !no_auto, full_res, &out)?;
    if !report.rationale.is_empty() {
        println!("  {}", report.rationale);
    }
    println!(
        "healed {} spot(s) -> {} ({} x {})",
        report.spots, out.display(), report.dims.0, report.dims.1
    );
    Ok(())
}

fn batch_cmd(
    dir: &Path,
    render: bool,
    limit: usize,
    include_baked: bool,
    jobs: usize,
) -> Result<()> {
    let cfg = Config::load();
    // R27 P1. `batch` is the one of the three scanners where including baked
    // photos is a real feature rather than a policy break — it develops
    // sources, and a PNG/TIFF/JPEG is a source every other surface already
    // opens. It is nevertheless OPT-IN, because the default would be
    // expensive and surprising: RAW+JPEG is a standard camera setting, so a
    // library folder routinely holds a baked twin of every RAW, and each twin
    // is a paid analysis. `eval` and `style-index` do NOT get this flag — see
    // their own call sites.
    let raws = if include_baked {
        autoshop::pipeline::find_sources(dir)?
    } else {
        find_raws(dir)?
    };
    println!(
        "found {} {} under {}",
        raws.len(),
        if include_baked { "photo(s)" } else { "RAW(s)" },
        dir.display()
    );
    if !include_baked {
        // Say what was NOT scanned, rather than leaving a folder of exports
        // looking empty (the failure mode the format-support map filed as B1).
        let all = autoshop::pipeline::find_sources(dir).map(|v| v.len()).unwrap_or(raws.len());
        if all > raws.len() {
            println!(
                "  ({} already-baked photo(s) skipped — pass --include-baked to develop them too)",
                all - raws.len()
            );
        }
    }

    // "Pending" = no saved develop anywhere — central store OR legacy ./out,
    // recipe OR XMP, OR the sidecar Lightroom itself writes beside the RAW
    // (L13#2: an LR-only photo was "pending", so batch spent a paid analyze
    // on it and then wrote recipe.json over the user's Lightroom work) — so
    // an analyzed library is not silently re-analyzed (and re-billed).
    let pending: Vec<&PathBuf> =
        raws.iter().filter(|r| !autoshop::store::has_develop_or_sidecar(r)).collect();
    let todo = pending.len();
    let n = todo.min(limit);
    println!("{todo} pending; processing {n} this run (--limit {limit}).");
    if todo > n {
        println!("  {} more remain — raise --limit to process them.", todo - n);
    }

    // The work list is fixed up front — exactly the first `n` pending photos,
    // the same set the old sequential `.take(n)` selected — then a small bounded
    // pool works through it. Each photo is dominated by blocking network round
    // trips (GPT propose + verify, plus revision rounds), so a few workers
    // overlap network wait with CPU renders. The count is `--jobs` (default 3,
    // the concurrency shipped since R26) capped by a MEMORY budget: R27 Batch-7
    // measured one 61 MP photo's pass at ~1.77 GB of peak commit, so three
    // unbudgeted workers could ask a tight machine for ~5.3 GB at once — which
    // is the wall a 147-photo `eval` hit. process_one already runs
    // produce_recipe with verbose=false, so workers write nothing to STDOUT
    // until their one completion line below — which is what lets the
    // sequencer make the parallel transcript byte-identical in order to a
    // serial one.
    //
    // STDERR is NOT covered, and this comment claimed it was until R28 2e
    // (adjudication F6). `verbose` gates the progress chatter, not the
    // warnings: pipeline.rs:390 (proposer fallback), :2220 (clamp
    // disclosure), :2454 and :2473, main.rs:1586 below, and the render's own
    // warnings (render.rs:1430 ICC embed, :3083 inert bitmap mask) are
    // ungated `eprintln!`s that land in COMPLETION order and mostly name no
    // photo. `jobs`' module docs disclose the channel; stamping those lines
    // with a stem is registered as R28 Batch-5 5c.
    let work: Vec<&Path> = pending.iter().take(n).map(|p| p.as_path()).collect();
    // Deliverables are stem-keyed and one library can hold two DSC00001.ARW
    // (counter rollover — four review units reported the silent overwrite).
    // Claim every render target UP FRONT in work order, so names stay
    // deterministic regardless of worker scheduling, and disclose the
    // deviations BEFORE the batch runs instead of after a file was destroyed.
    let outs: Vec<Option<PathBuf>> = {
        let mut names = autoshop::pipeline::BatchNames::default();
        let outs: Vec<Option<PathBuf>> = work
            .iter()
            .map(|r| if render { Some(names.claim(r, "developed", "tif")) } else { None })
            .collect();
    // The paid loop must not start against an unusable ./out (review
    // R12-06): ensure_parent used to run per photo AFTER produce_recipe,
    // and its failure threw the billed analysis away. One full preflight of
    // the first claimed name covers the shared parent for the whole run.
    if let (Some(first_out), Some(first_raw)) =
        (outs.iter().flatten().next(), pending.first())
    {
        pipeline::preflight_out(first_out, first_raw)?;
    }

        for line in &names.renamed {
            println!("  same-name RAW keeps a separate deliverable: {line}");
        }
        outs
    };
    let plan = autoshop::jobs::plan(jobs, work.len());
    if let Some(note) = &plan.note {
        println!("{note}");
    }
    // Per-index results: summary counts stay exact and deterministic
    // regardless of completion order.
    #[derive(Clone, Copy, PartialEq)]
    enum Outcome {
        Saved,
        NotAccepted, // produced but NOT saved — a non-Accept verdict never auto-saves
        Failed,
    }
    // Workers write into their own block and the sequencer releases blocks in
    // INDEX order (R27 Batch-7). Before that, each worker took the stdout lock
    // on completion — whole lines, but in whatever order the photos finished,
    // so `[7/50]` could print before `[3/50]` and a re-run of the same folder
    // produced a differently-ordered transcript. A failed photo still reports
    // its error and the batch still continues, exactly as before.
    let results = autoshop::jobs::for_each_indexed(plan.jobs, n, |i, block| {
        use std::fmt::Write;
        let raw = work[i];
        let res = process_one(raw, &cfg, outs[i].as_deref());
        match &res {
            Ok(v) if v.decision == autoshop::advisor::Decision::Accept => {
                let _ = writeln!(block, "[{}/{n}] {} ... {:?}", i + 1, stem(raw), v.decision);
                Outcome::Saved
            }
            Ok(v) => {
                let _ = writeln!(
                    block,
                    "[{}/{n}] {} ... {:?} — NOT saved (a non-Accept verdict never auto-saves)",
                    i + 1,
                    stem(raw),
                    v.decision
                );
                Outcome::NotAccepted
            }
            Err(e) => {
                let _ = writeln!(block, "[{}/{n}] {} ... FAILED: {e}", i + 1, stem(raw));
                Outcome::Failed
            }
        }
    });
    let ok = results.iter().filter(|r| **r == Some(Outcome::Saved)).count();
    let skipped = results.iter().filter(|r| **r == Some(Outcome::NotAccepted)).count();
    let fail = results.iter().filter(|r| **r == Some(Outcome::Failed)).count();
    // What LEAVES the pending set is a develop on disk, which is not the
    // same as a success: since a render failure persists the develop it
    // already paid for (so a re-run cannot re-bill it), such a photo is
    // FAILED and no longer pending. Counting `todo - ok` therefore told the
    // user to re-run for photos a re-run would skip. Ask the store instead of
    // inferring, and keep the re-bill promise scoped to what still holds:
    // a non-Accept photo really did leave no sidecar.
    // The SAME predicate as the up-front filter, or the summary contradicts
    // the selection it reports on.
    let still_pending =
        pending.iter().filter(|p| !autoshop::store::has_develop_or_sidecar(p.as_path())).count();
    // The remedy note keys on the photos that NEED it, counted directly: a
    // FAILED photo whose develop persisted is exactly what `apply` finishes.
    // The old proxy (`still_pending < fail`) compared the library-wide
    // pending count against this run's failures, so any non-Accept photo
    // left pending — the default outcome of a cautious verdict — masked the
    // note for the failure sitting right next to it.
    let failed_developed = (0..results.len())
        .filter(|&i| results[i] == Some(Outcome::Failed) && autoshop::store::has_develop(work[i]))
        .count();
    println!(
        "\nbatch done: {ok} saved, {skipped} not saved (non-Accept), {fail} failed, {still_pending} still pending.",
    );
    if failed_developed > 0 {
        println!(
            "  note: a photo whose RENDER failed keeps the develop it already paid for, so it is \
             no longer pending — re-run `autoshop apply` for its deliverable."
        );
    }
    // The summary printed FIRST (it names every photo); the exit code then
    // tells scripts/CI the truth — an all-FAILED run used to exit 0
    // (16-lane scan L09).
    if fail > 0 {
        anyhow::bail!("{fail} photo(s) failed — see the FAILED lines above");
    }
    Ok(())
}

fn process_one(raw: &Path, cfg: &Config, render_to: Option<&Path>) -> Result<Verdict> {
    // Batch uses the configured style strength (AUTOSHOP_STYLE_STRENGTH).
    // judge = false: a library-sized batch must never silently multiply the
    // paid vision calls — the closed loop is for the interactive surfaces
    // (review R20-M2).
    let (recipe, verdict, _notes) =
        produce_recipe(raw, cfg, false, None, None, GradeRequest::with_style(cfg.style_strength), false)?;
    // A non-Accept verdict may not auto-save (user decision). In a headless
    // batch that means NO sidecars and NO deliverable: the photo stays
    // pending, the caller's summary names it, and a re-run re-attempts it —
    // the same consequence a failed photo already has.
    if verdict.decision != autoshop::advisor::Decision::Accept {
        return Ok(verdict);
    }
    // Every fallible product BEFORE the completion markers: `has_develop`
    // (the batch resume filter) keys on the sidecars, so any failure after
    // one lands would leave a photo both "failed" in this run AND skipped by
    // every later resume. The render is idempotent, so a crash after it but
    // before the sidecars simply re-renders on retry. Between the two
    // sidecars the RECIPE goes first — it is the lossless truth and the
    // cross-surface rule is "the recipe write alone decides the saved state";
    // the old xmp-first order could crash into an XMP-only marker that
    // suppressed every retry while the authoritative recipe never landed.
    // Sidecar persistence, shared by the success path and the failed-render
    // path below: once the verdict is Accept, the PAID analysis must land.
    let persist = |recipe: &EditRecipe| -> Result<()> {
        autoshop::store::with_develop_lock(
            raw,
            autoshop::store::DevelopLockMode::Wait,
            || {
        // Backup gate, same as every surface: cheap Ok(None) in the batch's
        // usual no-develop-yet case, and a save created mid-batch by another
        // process can no longer be silently destroyed.
        if let Err(e) = autoshop::store::backup_saved_develop(raw, Some(recipe)) {
            anyhow::bail!("refusing to overwrite the saved develop: backing it up failed ({e})");
        }
        write_recipe(raw, recipe, None)?;
        // The recipe write ALONE decides the saved state (cross-surface rule):
        // failing the photo here made the batch report it failed while the
        // resume filter — which sees recipe.json — permanently skipped it.
        if let Err(e) = write_xmp(raw, recipe) {
            eprintln!("  ⚠ recipe saved, but the Lightroom XMP failed: {e}");
        }
        Ok(())
            },
        )
    };
    if let Some(out) = render_to {
        // 16-bit master at the batch-claimed name — claimed up front by the
        // caller so same-stem photos in one batch each keep a deliverable
        // and worker scheduling cannot reorder the names.
        ensure_parent(out)?;
        if let Err(e) = render::render_to_file(raw, &recipe, out, None, None) {
            // A render failure must not discard the PAID, verified analysis:
            // with sidecars-last ordering the photo stayed pending and a
            // re-run RE-BILLED it. Persist the develop first — it is
            // independent of the render — then fail the photo loudly; the
            // deliverable re-renders FREE via `autoshop apply`. (A crash
            // mid-render still re-attempts everything: nothing landed.)
            return match persist(&recipe) {
                Ok(()) => Err(e).context(
                    "render failed — the develop WAS saved; re-render it free with \
                     `autoshop apply` (this photo is no longer pending)",
                ),
                Err(pe) => Err(e).context(format!(
                    "render failed, and saving the develop also failed ({pe:#}) — \
                     the photo stays pending"
                )),
            };
        }
    }
    persist(&recipe)?;
    Ok(verdict)
}

/// Render a 256-bin histogram as a compact Unicode block sparkline.
fn sparkline(bins: &[u32]) -> String {
    const BLOCKS: [char; 8] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇'];
    let groups = 48usize;
    let per = bins.len().div_ceil(groups);
    let sums: Vec<u32> = bins.chunks(per).map(|c| c.iter().copied().sum()).collect();
    let max = sums.iter().copied().max().unwrap_or(1).max(1);
    sums.iter()
        .map(|&v| {
            let idx = ((v as f64 / max as f64) * (BLOCKS.len() - 1) as f64).round() as usize;
            BLOCKS[idx.min(BLOCKS.len() - 1)]
        })
        .collect()
}

#[cfg(test)]
mod tests {

    /// A keyless, endpoint-less Config — the shape the preflight tests and the
    /// flag-resolution test both need (no key, so nothing here can reach a
    /// network or bill anything).
    fn cfg_fixture() -> autoshop::config::Config {
        autoshop::config::Config {
            openai_api_key: None,
            openai_model: "m".into(),
            openai_base_url: "http://127.0.0.1:1".into(),
            openai_image_model: "m".into(),
            openai_image_quality: "auto".into(),
            openai_image_max_px: 4_000_000,
            image_provider: "api".into(),
            image_effort: None,
            analysis_provider: "oauth".into(),
            analysis_model: "opus".into(),
            analysis_effort: None,
            claude_bin: "claude".into(),
            analysis_api_key: None,
            analysis_base_url: "http://127.0.0.1:1".into(),
            python_bin: "python".into(),
            denoise_model: "m".into(),
            denoise_script: String::new(),
            denoise_cache: String::new(),
            segment_script: String::new(),
            embed_script: String::new(),
            style_strength: 0.5,
        }
    }

    /// L10 preflight family: certain failures refuse at the door.
    #[test]
    fn the_cli_preflights_refuse_certain_failures_before_any_paid_work() {
        // Closed-set values (L10-10).
        assert!(require_choice("--fidelity", "high", &["high", "low"]).is_ok());
        let e = require_choice("--fidelity", "medium", &["high", "low"])
            .unwrap_err()
            .to_string();
        assert!(e.contains("high|low") && e.contains("medium"), "{e}");

        // Missing image key (L10-13) — a config with no key refuses with the
        // reason, not a late server error.
        let cfg = cfg_fixture();
        let e = require_image_key(&cfg, "reimagine").unwrap_err().to_string();
        assert!(e.contains("OPENAI_API_KEY"), "{e}");
    }

    /// L10-9 + L10-12: heal refuses a missing mask and a maskless --no-auto
    /// BEFORE the paid auto-detect / the decode (the fixture photo does not
    /// even exist, so reaching either would error differently).
    #[test]
    fn heal_refuses_mask_problems_before_the_paid_auto_detect() {
        let root =
            std::env::temp_dir().join(format!("autoshop-heal-preflight-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let src = root.join("library").join("missing.arw");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();

        let e = heal_cmd(
            &src,
            Some(root.join("no-such-mask.png")),
            false,
            false,
            Some(root.join("exports").join("h.png")),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("--mask") && e.contains("cannot be used"), "{e}");

        let e = heal_cmd(&src, None, true, false, Some(root.join("exports").join("h2.png")))
            .unwrap_err()
            .to_string();
        assert!(e.contains("--no-auto") && e.contains("nothing to do"), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    use super::*;

    /// `match --target` names a finished RENDITION of this frame. Until
    /// R23-6 that meant a baked file only, and a RAW target was refused by
    /// name before the fit was paid for (the refusal replaced "The image
    /// format could not be determined", which reads like a corrupt file
    /// rather than the wrong kind of file).
    ///
    /// R23-6 opens the reference to any file the user names — the desktop
    /// app's new reference picker offers RAWs too, and the two commands must
    /// not disagree about what a reference is — so a RAW now goes through
    /// `render::source_pixels` (developed NEUTRALLY; the caller is told so).
    /// What must NOT regress is the readability of the failure when the file
    /// cannot be decoded at all: it still has to name the file and say what
    /// was wrong with it, before anything is written.
    #[test]
    fn match_names_the_file_when_a_raw_target_cannot_be_decoded() {
        let dir = std::env::temp_dir().join(format!("autoshop-match-rawtgt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // The SOURCE is a baked photo so the run reaches the target load at
        // all (a real decode, no API, no writes).
        let src = dir.join("source.png");
        image::RgbImage::from_pixel(8, 6, image::Rgb([120, 120, 120])).save(&src).unwrap();
        let target = dir.join("reference.ARW");
        std::fs::write(&target, b"not really a raw").unwrap();

        let e = format!(
            "{:#}",
            match_cmd(&src, &target, false, false, false, false, false, None)
                .expect_err("an undecodable RAW target must refuse")
        );
        assert!(
            e.contains("reference.ARW"),
            "the refusal must name the file: {e}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(windows)]
    fn same_path_folds_case_of_an_absent_leaf() {
        let dir = std::env::temp_dir().join("autoshop-cli-same-path-case");
        let _ = std::fs::create_dir_all(&dir);
        // The leaf does NOT exist — only the ancestor chain canonicalizes, so
        // equality must come from the case fold, not from canonicalize.
        assert!(same_path(&dir.join("recipe.json"), &dir.join("Recipe.json")));
        // A genuinely different absent leaf must stay different.
        assert!(!same_path(&dir.join("recipe.json"), &dir.join("other.json")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_denoise_operands_are_refused_without_the_denoise_flag() {
        for args in [
            vec!["autoshop", "auto", "photo.arw", "--denoise-strength", "0.5"],
            vec!["autoshop", "auto", "photo.arw", "--denoise-model", "color_real_gan"],
        ] {
            let Err(error) = Cli::try_parse_from(args) else {
                panic!("a denoise operand without --denoise must be refused");
            };
            let message = error.to_string();
            assert!(message.contains("--denoise"), "{message}");
        }
    }

    /// 16-lane scan L09: "--style NaN" parsed as a valid f32 and silently
    /// disabled style; "--style 2" reported 200% while clamping to 1's
    /// effect. The 0..=1 flags refuse out-of-domain input at the parser.
    #[test]
    fn strength_flags_refuse_non_finite_and_out_of_domain_values() {
        for bad in ["NaN", "inf", "2", "-0.1"] {
            assert!(
                Cli::try_parse_from(["autoshop", "analyze", "p.arw", "--style", bad]).is_err(),
                "--style {bad} must be refused"
            );
            assert!(
                Cli::try_parse_from(["autoshop", "denoise", "p.png", "--strength", bad]).is_err(),
                "--strength {bad} must be refused"
            );
        }
        assert!(Cli::try_parse_from(["autoshop", "analyze", "p.arw", "--style", "0.7"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "autoshop", "auto", "p.arw", "--denoise", "--denoise-strength", "1"
            ])
            .is_ok()
        );

        // R23-3: the GRADE strength axis is a THIRD 0..=1 flag on this parser,
        // and it lands on `analyze` and `auto` — the two single-photo commands.
        // Same door as the others (a NaN dial must not read as "most timid").
        for bad in ["NaN", "inf", "2", "-0.1"] {
            for cmd in ["analyze", "auto"] {
                assert!(
                    Cli::try_parse_from(["autoshop", cmd, "p.arw", "--strength", bad]).is_err(),
                    "{cmd} --strength {bad} must be refused"
                );
            }
        }
        for cmd in ["analyze", "auto"] {
            let cli = Cli::try_parse_from(["autoshop", cmd, "p.arw", "--strength", "0.9"])
                .unwrap_or_else(|e| panic!("{cmd} --strength 0.9 must parse: {e}"));
            let got = match cli.command {
                Command::Analyze { strength, .. } | Command::Auto { strength, .. } => strength,
                _ => panic!("`{cmd} --strength` parsed as some other subcommand"),
            };
            assert_eq!(got, Some(0.9), "{cmd} must carry the value, not drop it");
        }
        // …and the flag actually decides the request `analyze`/`auto` build —
        // the PRODUCTION resolver, shared by both commands.
        let cfg = autoshop::config::Config { style_strength: 0.3, ..cfg_fixture() };
        let plain = analyze_request(None, None, false, &cfg);
        assert_eq!(plain.style, 0.3, "omitted --style keeps AUTOSHOP_STYLE_STRENGTH");
        assert_eq!(
            plain.strength.get(),
            GradeStrength::DEFAULT,
            "omitted --strength is the SHIPPED default (0.65) — the CLI and a double-clicked \
             GUI must develop the same photo the same way when neither is told otherwise"
        );
        let dialled = analyze_request(Some(0.1), Some(0.9), false, &cfg);
        assert_eq!((dialled.style, dialled.strength.get()), (0.1, 0.9), "no axis swap");
        assert!(!dialled.send_reference_image, "every non-GUI surface stays on the text reference");
        // 0.5 is the calibration point, one flag away.
        assert_eq!(
            analyze_request(None, Some(0.5), false, &cfg).strength,
            GradeStrength::calibrated()
        );
        // R23-4: `--deep` is the ONLY way this resolver's thinking flag turns
        // on, and it is a THIRD axis — it must not be confusable with either
        // dial (`batch` never reaches this function at all).
        assert!(!plain.think, "omitted --deep is off, like every paid opt-in");
        assert!(analyze_request(None, None, true, &cfg).think, "--deep must reach the request");
        assert!(
            !analyze_request(Some(1.0), Some(1.0), false, &cfg).think,
            "pushing both dials to the maximum must not buy the thinking envelope"
        );
    }
    /// L09#1 ordering: with a nonexistent RAW, a bad `-o` must fail on the
    /// OUTPUT (pre-pay preflight), never on decode or a paid call.
    #[test]
    fn analyze_and_auto_refuse_a_bad_output_before_the_paid_call() {
        let dir = std::env::temp_dir()
            .join(format!("autoshop-prepay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let e = analyze_cmd(Path::new("no-such.arw"), Some(dir.clone()), None, None, None, false)
            .unwrap_err()
            .to_string();
        assert!(e.contains("is a directory"), "analyze: {e}");
        let e = auto_cmd(
            Path::new("no-such.arw"),
            Some(dir.join("x.xyz")),
            None,
            None,
            None,
            false,
            false,
            None,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("unsupported output format"), "auto: {e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R27 Batch-7: `--jobs` exists on both per-photo loops, and its DEFAULT is
    /// each command's pre-Batch-7 concurrency — 1 for `eval` (which was serial)
    /// and 3 for `batch` (which has run a hard-coded three-worker pool since
    /// R26). Anything else would make an unflagged run behave differently from
    /// the release before it, in one direction or the other.
    ///
    /// MUTATION THIS KILLS: copying `default_value_t = 1` onto `batch` (which
    /// reads as the safe choice and silently makes every unflagged library
    /// batch three times slower), or `= 3` onto `eval` (which triples a
    /// measurement run's concurrent spend and peak memory without being asked).
    /// A test that only checked "the flag parses" would pass under both.
    #[test]
    fn jobs_defaults_to_each_commands_existing_concurrency() {
        let jobs_of = |args: [&str; 3]| match Cli::try_parse_from(args)
            .unwrap_or_else(|e| panic!("{args:?} must parse: {e}"))
            .command
        {
            Command::Batch { jobs, .. } | Command::Eval { jobs, .. } => jobs,
            _ => panic!("{args:?} parsed as some other subcommand"),
        };
        assert_eq!(jobs_of(["autoshop", "batch", "dir"]), 3, "batch keeps its R26 pool");
        assert_eq!(jobs_of(["autoshop", "eval", "dir"]), 1, "eval stays serial unless asked");
        // …and the flag is actually wired, on both.
        for cmd in ["batch", "eval"] {
            let cli = Cli::try_parse_from(["autoshop", cmd, "dir", "--jobs", "6"])
                .unwrap_or_else(|e| panic!("{cmd} --jobs 6 must parse: {e}"));
            let got = match cli.command {
                Command::Batch { jobs, .. } | Command::Eval { jobs, .. } => jobs,
                _ => panic!("`{cmd} --jobs` parsed as some other subcommand"),
            };
            assert_eq!(got, 6, "{cmd} must carry --jobs, not drop it");
        }
        // 0 is a typo, not "no workers": the planner floors it at one rather
        // than the parser refusing, so a script that computes the value from
        // `nproc - 1` on a single-core box still runs.
        assert_eq!(autoshop::jobs::plan_with(0, 10, None).jobs, 1);
    }

    /// L09#4: the `heal --full-res` help names the baked-source downsample
    /// consequence and no longer claims "RAW only" (false since b4c6c30 —
    /// the flag honours baked sources; WITHOUT it a baked input is
    /// thumbnailed to 2048px and saved as the pixel master).
    #[test]
    fn heal_full_res_help_names_the_baked_downsample() {
        use clap::CommandFactory as _;
        let cmd = Cli::command();
        let heal = cmd.get_subcommands().find(|c| c.get_name() == "heal").expect("heal subcommand");
        let arg = heal.get_arguments().find(|a| a.get_id() == "full_res").expect("full-res arg");
        let help = arg.get_long_help().or_else(|| arg.get_help()).expect("help text").to_string();
        assert!(help.contains("baked"), "{help}");
        assert!(help.contains("2048"), "{help}");
        assert!(!help.contains("RAW only"), "{help}");
    }

    /// R27 L-07. `xmp::describe_import_losses` had two callers and both were
    /// in the GUI: a CLI run over a photo whose Lightroom sidecar holds a
    /// brush mask printed nothing at all about it, while the window said it on
    /// every open. This is the CLI's half of that channel.
    ///
    /// The fixture is SYNTHETIC (public repo — no user sidecar bytes here).
    /// Its shape follows the reference corpus: a `Correction` whose only
    /// component is a `Mask/Paint`, which Lightroom recomputes from a digest,
    /// so there are no pixels for a third-party reader to take.
    ///
    /// MUTATION THIS CATCHES: drop the `describe_import_losses` call from
    /// `lightroom_import_note` (or gate it on `decode::is_raw` being FALSE)
    /// and the brush arm goes silent again; drop the `is_raw` guard entirely
    /// and the baked arm starts consulting a neighbouring `.xmp` that belongs
    /// to somebody else's file.
    #[test]
    fn a_lossy_lightroom_sidecar_is_disclosed_on_the_cli() {
        let root = std::env::temp_dir()
            .join(format!("autoshop-cli-import-note-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        // One correction, one `Mask/Paint` component: `Unrepresentable`.
        let brush = "\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 5.6-c145\">
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">
  <rdf:Description rdf:about=\"\"
    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"
    crs:Version=\"15.5.1\"
    crs:ProcessVersion=\"15.4\"
    crs:Exposure2012=\"+0.35\"
    crs:HasSettings=\"True\">
   <crs:MaskGroupBasedCorrections>
    <rdf:Seq>
     <rdf:li>
      <rdf:Description
       crs:What=\"Correction\"
       crs:CorrectionAmount=\"1\"
       crs:CorrectionActive=\"true\"
       crs:CorrectionName=\"Brush 1\"
       crs:LocalExposure2012=\"0.1\">
      <crs:CorrectionMasks>
       <rdf:Seq>
        <rdf:li
         crs:What=\"Mask/Paint\"
         crs:MaskActive=\"true\"
         crs:MaskName=\"Brush 1\"
         crs:MaskBlendMode=\"0\"
         crs:MaskInverted=\"false\"
         crs:MaskValue=\"1\"/>
       </rdf:Seq>
      </crs:CorrectionMasks>
      </rdf:Description>
     </rdf:li>
    </rdf:Seq>
   </crs:MaskGroupBasedCorrections>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
";
        let raw = root.join("lossy.arw");
        std::fs::write(&raw, b"not a real ARW - the note never decodes pixels").unwrap();
        std::fs::write(raw.with_extension("xmp"), brush).unwrap();
        let note = lightroom_import_note(&raw).expect("a brush correction is a disclosed loss");
        assert!(note.contains("Brush 1"), "the sentence must name the correction: {note}");
        assert!(
            note.contains("AI / brush correction(s) skipped"),
            "…and say what it cost: {note}"
        );
        assert!(note.contains("lossy.xmp"), "…and which file it read: {note}");

        // A sidecar with global settings only loses nothing, and says nothing.
        let clean = root.join("clean.arw");
        std::fs::write(&clean, b"stub").unwrap();
        std::fs::write(
            clean.with_extension("xmp"),
            brush
                .split("   <crs:MaskGroupBasedCorrections>")
                .next()
                .unwrap()
                .to_string()
                + "  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n",
        )
        .unwrap();
        assert_eq!(lightroom_import_note(&clean), None, "a faithful import says nothing");

        // A baked photo's neighbouring `.xmp` is someone else's file — the
        // line `pipeline::write_xmp` draws before it consults one.
        let baked = root.join("baked.png");
        std::fs::write(&baked, b"stub").unwrap();
        std::fs::write(baked.with_extension("xmp"), brush).unwrap();
        assert_eq!(lightroom_import_note(&baked), None, "a baked source has no LR sidecar");

        // No sidecar at all is silence, not a panic.
        let bare = root.join("bare.arw");
        std::fs::write(&bare, b"stub").unwrap();
        assert_eq!(lightroom_import_note(&bare), None);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// R28 2b, the CLI half: what the recipe's SIZE CAPS cut on import is
    /// disclosed too, and on its own terms.
    ///
    /// `import_losses` reads the DOCUMENT and cannot see this — the caps run
    /// on the recipe the document produced. The fixture's correction imports
    /// perfectly (an ordinary gradient, no named loss at all), so the only
    /// thing this sentence can be reporting is the clamp; a mask NAME 400
    /// bytes long is the cheapest trip of a size cap there is (`MAX_NAME` =
    /// 256), and the same channel carries the 393 KB dab stream the xmp-side
    /// test exercises.
    ///
    /// MUTATION THIS CATCHES: revert `lightroom_import_note` to
    /// `xmp_to_recipe` + a two-element `line`, or revert `xmp_to_recipe`'s
    /// tail to a bare `r.clamp();` — the note goes back to `None` and the
    /// truncation is once again said nowhere.
    #[test]
    fn an_import_truncated_by_the_recipe_caps_is_disclosed_on_the_cli() {
        let root = std::env::temp_dir()
            .join(format!("autoshop-cli-clamp-note-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let long_name = "N".repeat(400);
        let doc = format!(
            "\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 5.6-c145\">
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">
  <rdf:Description rdf:about=\"\"
    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"
    crs:Version=\"15.5.1\"
    crs:ProcessVersion=\"15.4\"
    crs:HasSettings=\"True\">
   <crs:MaskGroupBasedCorrections>
    <rdf:Seq>
     <rdf:li>
      <rdf:Description
       crs:What=\"Correction\"
       crs:CorrectionAmount=\"1\"
       crs:CorrectionActive=\"true\"
       crs:CorrectionName=\"{long_name}\"
       crs:LocalExposure2012=\"0.1\">
      <crs:CorrectionMasks>
       <rdf:Seq>
        <rdf:li
         crs:What=\"Mask/Gradient\"
         crs:MaskActive=\"true\"
         crs:MaskBlendMode=\"0\"
         crs:MaskInverted=\"false\"
         crs:MaskValue=\"1\"
         crs:ZeroX=\"0.1\" crs:ZeroY=\"0.1\"
         crs:FullX=\"0.9\" crs:FullY=\"0.9\"/>
       </rdf:Seq>
      </crs:CorrectionMasks>
      </rdf:Description>
     </rdf:li>
    </rdf:Seq>
   </crs:MaskGroupBasedCorrections>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"
        );
        let raw = root.join("capped.arw");
        std::fs::write(&raw, b"stub").unwrap();
        std::fs::write(raw.with_extension("xmp"), &doc).unwrap();

        // Premise: nothing about this document is a NAMED import loss, so a
        // sentence here can only come from the clamp.
        assert!(
            autoshop::xmp::import_losses(&doc).is_empty(),
            "premise: the gradient imports whole — {:?}",
            autoshop::xmp::import_losses(&doc)
        );
        let note = lightroom_import_note(&raw).expect("a truncated import must not be silent");
        assert!(note.contains("recipe limits then discarded"), "{note}");
        assert!(note.contains("144 string byte(s)"), "400 - 256 = 144: {note}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
