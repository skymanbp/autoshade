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
// shared with the native GUI binary (src/bin/gui/main.rs and its module tree).
use autoshop::{correspond, decode, denoise, eval, fit, generative, pipeline, render, retouch, serve};
use autoshop::advisor::Verdict;
use autoshop::config::Config;
use autoshop::pipeline::{
    default_out, ensure_parent, find_raws, produce_recipe, stem, write_recipe, write_xmp,
    GradeRequest,
};
use autoshop::recipe::{DirectionAdherence, EditRecipe, GradeStrength};
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
    /// Decode a supported RAW or baked image and print metadata and histogram.
    /// RAW inputs use their embedded preview; baked inputs use the image pixels.
    /// Embedded-XMP reporting applies to RAW inputs only.
    /// Writes the preview to ./out (never beside the source).
    Decode {
        /// Path to a supported RAW or baked image.
        #[arg(value_name = "SOURCE")]
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
        /// How closely to follow `--guidance`, 0..1. The value picks a TIER,
        /// and the TIER NAME is what the proposer and the verifier are told:
        /// <=0.40 Hint, 0.40..0.70 Direct, >0.70 Brief. Omitted = 0.65, i.e.
        /// Direct. Prompt intent only — it never moves a render bound — and it
        /// does nothing at all without a direction.
        #[arg(long, value_parser = unit_interval)]
        adherence: Option<f32>,
        /// Let the local SigLIP 2 sidecar answer the style query, so the look
        /// library and the embedding terms can rank. First run downloads
        /// ~1.5 GB of weights into python/weights and every call re-loads them.
        /// Overrides AUTOSHOP_STYLE_EMBED for this run.
        #[arg(long, conflicts_with = "no_embed")]
        embed: bool,
        /// Refuse the embedding sidecar for this run even if
        /// AUTOSHOP_STYLE_EMBED is set: the 14-dim ranking only, and no look
        /// library.
        #[arg(long, conflicts_with = "embed")]
        no_embed: bool,
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
        /// Export at a SIZE: resize the finished render so its LONG edge is
        /// this many pixels (aspect kept, Lanczos3, NEVER upscaled — a value
        /// past the sensor's own long edge saves at full resolution). 0 or
        /// omitted = full resolution, the same spelling the desktop Export
        /// panel uses. The develop itself always runs at full resolution and
        /// the resize is the last pixel stage, so this changes the SIZE of the
        /// deliverable and not its look.
        #[arg(long)]
        long_edge: Option<u32>,
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
        /// How closely to follow `--guidance`, 0..1. The value picks a TIER,
        /// and the TIER NAME is what the proposer and the verifier are told:
        /// <=0.40 Hint, 0.40..0.70 Direct, >0.70 Brief. Omitted = 0.65, i.e.
        /// Direct. Prompt intent only — it never moves a render bound — and it
        /// does nothing at all without a direction.
        #[arg(long, value_parser = unit_interval)]
        adherence: Option<f32>,
        /// Let the local SigLIP 2 sidecar answer the style query, so the look
        /// library and the embedding terms can rank. First run downloads
        /// ~1.5 GB of weights into python/weights and every call re-loads them.
        /// Overrides AUTOSHOP_STYLE_EMBED for this run.
        #[arg(long, conflicts_with = "no_embed")]
        embed: bool,
        /// Refuse the embedding sidecar for this run even if
        /// AUTOSHOP_STYLE_EMBED is set: the 14-dim ranking only, and no look
        /// library.
        #[arg(long, conflicts_with = "embed")]
        no_embed: bool,
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
        /// Export at a SIZE (see `apply --long-edge`): long edge in pixels,
        /// aspect kept, Lanczos3, never upscaled. 0 or omitted = full
        /// resolution.
        #[arg(long)]
        long_edge: Option<u32>,
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
        /// Export at a SIZE (see `apply --long-edge`), applied PER PHOTO: long
        /// edge in pixels, aspect kept, Lanczos3, never upscaled, so a folder
        /// of mixed orientations and sensors comes out with one bounded long
        /// edge each rather than one bounded width. 0 = full resolution.
        /// Requires --render, which is what produces the deliverable at all:
        /// without it this run writes recipes and sidecars and no pixels, and
        /// a size for files that are never written is a typo, not a setting.
        #[arg(long, requires = "render")]
        long_edge: Option<u32>,
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
        /// Discard any saved progress for this folder and measure every photo
        /// again. Without it, a rerun folds in the photographs an interrupted
        /// run already measured and spends nothing on them.
        #[arg(long)]
        fresh: bool,
        /// Where to keep this run's resumable progress. Default: a file named
        /// from the folder + --limit under the app's per-user data directory
        /// (never inside your photo library, which stays read-only).
        #[arg(long)]
        state: Option<PathBuf>,
    },
    /// Build the style index from your edited library (RAW+.xmp pairs) → the
    /// advisor then references your edits on similar shots. Run once / on update.
    StyleIndex {
        /// Folder to scan recursively for RAW + .xmp pairs (your edits).
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Build the LOOK LIBRARY instead: a folder of FINISHED photos
        /// (JPEG/TIFF exports) whose grade the advisor may point at. Looks are
        /// embedding-only — they carry no settings and never enter the recipe
        /// blend — so this needs --embed, and it replaces only the look block
        /// of the stored index, never your RAW exemplars.
        #[arg(long)]
        looks: Option<PathBuf>,
        /// Compute a SigLIP 2 vector per record as well as the 14-dim feature.
        /// First run downloads ~1.5 GB of weights into python/weights.
        /// Overrides AUTOSHOP_STYLE_EMBED for this run.
        #[arg(long, conflicts_with = "no_embed")]
        embed: bool,
        /// Build the 14-dim index only, even if AUTOSHOP_STYLE_EMBED is set.
        #[arg(long, conflicts_with = "embed")]
        no_embed: bool,
    },
    /// Offline retrieval diagnostic: what the style index would answer for one
    /// photo, in the ranking's own numbers.
    ///
    /// Prints the weights in force, then every retrieved neighbour with its
    /// distance broken into the terms that produced it (the 14-dim block, the
    /// image-embedding term, and the two direction-text terms beside the raw
    /// cosines behind them), the reference block the proposer would be
    /// given, the look-library block, and the disclosure notes the develop
    /// would emit. No advisor call, no network, no spend.
    StyleQuery {
        /// The photo to query with (RAW or a baked image).
        photo: PathBuf,
        /// Optional free-text direction, embedded with the text tower so the
        /// direction terms and the look library have something to rank
        /// against. Needs --embed.
        #[arg(long)]
        direction: Option<String>,
        /// The Style axis used to render the reference block, 0..1 (default
        /// 0.3). At >=0.85 the block states the retrieved habits as a TARGET
        /// rather than a ceiling.
        #[arg(long, value_parser = unit_interval)]
        style: Option<f32>,
        /// Ask the SigLIP 2 sidecar for the query vectors. Without it the
        /// diagnostic shows the 14-dim ranking and says the look library is
        /// unreachable.
        #[arg(long)]
        embed: bool,
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
        /// Opt-in: when the result's structural divergence D reaches the
        /// reverse-fit threshold, buy ONE more generation (a second billed
        /// image) and keep whichever result measures closer to the input.
        #[arg(long)]
        fidelity_retry: bool,
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
        /// Fit globally, then add semantic bitmap-region corrections when
        /// local segmentation succeeds. If segmentation is disabled or
        /// unavailable, automatically try evidence-gated native luminance
        /// ranges; otherwise retain the global fit. Then automatically try up
        /// to four frozen-evidence 4x4 spatial bitmap tiles with zero frame
        /// regression. Conservative guided mask refinement may abstain. Bitmap
        /// masks stay engine-only with a named XMP loss; native ranges project
        /// to Lightroom XMP. No network.
        #[arg(long)]
        zoned: bool,
        /// Maximum accepted semantic class regions for zoned fitting (the
        /// default is the historical two-region path; four is opt-in).
        #[arg(long, default_value_t = autoshop::fit_zoned::semantic::DEFAULT_SEMANTIC_REGIONS)]
        regions: usize,
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
        /// The reverse-fit HONESTY BUDGET, 0..1 — the GUI panel's Strength.
        /// At or below the default 0.65 the fit is byte-identical to the
        /// calibrated path; above it the Atmosphere budget widens, an
        /// out-of-budget white balance shrinks along its fitted move instead of
        /// staying as-shot, and from 0.85 unsupported movement is DISCLOSED
        /// (confidence capped) rather than withheld.
        #[arg(long, value_parser = unit_interval)]
        strength: Option<f32>,
        /// Named recipe JSON artifact for `apply` (default:
        /// ./out/<stem>.matched.json). The canonical recipe.json in this
        /// photo's develop store — what the GUI and web restore — is ALWAYS
        /// written too.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// EXPERIMENTAL diagnostic: measure the cross-image CORRESPONDENCE between
    /// two renditions of one frame (DIFT / SD 2.1 python sidecar, local;
    /// ~2.6 GB weight download on first run). Writes a JSON field of per-cell
    /// target coordinates + confidences and prints a summary — the instrument
    /// the reverse-fit's content-divergent mode will read (step 7b). Never
    /// generates pixels; no API key needed.
    Correspond {
        /// Source rendition (RAW or baked image).
        source: PathBuf,
        /// Target rendition of the same frame (RAW or baked image).
        target: PathBuf,
        /// Output JSON (default: ./out/<stem>.correspond.json).
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
        /// Photo folder to browse, scanned recursively for the shared 24 RAW and 8 baked extensions.
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
        Command::Analyze { raw, out, guidance, style, strength, adherence, embed, no_embed, deep } => {
            analyze_cmd(&raw, out, guidance, style, strength, adherence, deep, embed_switch(embed, no_embed))
        }
        Command::Apply { raw, recipe, out, long_edge } => {
            apply_cmd(&raw, &recipe, &out, long_edge)
        }
        Command::Auto {
            raw,
            out,
            guidance,
            style,
            strength,
            adherence,
            embed,
            no_embed,
            deep,
            denoise,
            denoise_strength,
            denoise_model,
            long_edge,
        } => {
            auto_cmd(
                &raw, out, guidance, style, strength, adherence, deep, denoise, denoise_strength,
                denoise_model, long_edge, embed_switch(embed, no_embed),
            )
        }
        Command::Denoise { input, out, strength, model } => denoise_cmd(&input, out, strength, model),
        Command::Batch { dir, render, limit, include_baked, jobs, long_edge } => {
            batch_cmd(&dir, render, limit, include_baked, jobs, long_edge)
        }
        Command::Eval { dir, limit, jobs, fresh, state } => {
            eval::run(&dir, limit, jobs, fresh, state.as_deref())
        }
        Command::StyleIndex { dir, looks, embed, no_embed } => {
            let switch = embed_switch(embed, no_embed);
            if let Some(d) = looks { style_looks_cmd(&d, switch) } else { style_index_cmd(&dir, switch) }
        }
        Command::StyleQuery { photo, direction, style, embed } => style_query_cmd(
            &photo,
            direction.as_deref(),
            style.unwrap_or(0.3),
            embed_switch(embed, false),
        ),
        Command::Reimagine { raw, prompt, fidelity, quality, fidelity_retry, out } => {
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
            // The report's D lines were already printed by the library
            // (they double as the GUI's progress feed) — nothing to add here.
            generative::reimagine(&cfg, &raw, &prompt, &fidelity, &q, fidelity_retry, &out)
                .map(|_report| ())
        }
        Command::Match { raw, target, render, zoned, regions, style_prompt, ai_judge, deep, strength, out } => {
            // --deep IS the review, iterated: asking for the loop without the
            // reviewer is not a configuration, it is a typo.
            match_cmd(&raw, &target, render, zoned, regions, style_prompt, ai_judge || deep, deep, strength, out)
        }
        Command::Correspond { source, target, out } => correspond_cmd(&source, &target, out),
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

fn correspond_cmd(source: &Path, target: &Path, out: Option<PathBuf>) -> Result<()> {
    let cfg = Config::load();
    let out = out.unwrap_or_else(|| default_out(source, "correspond", "json"));
    // Output preflight FIRST (L09#1 ordering): a bad -o must refuse before
    // any decode — and long before a 2.6 GB first-run weight download.
    pipeline::preflight_out(&out, source)?; // includes the read-only-library guard
    let opts = correspond::CorrespondOpts::from_config(&cfg);
    if !opts.available() {
        anyhow::bail!(
            "correspondence sidecar not found at {} — run from the project dir or set \
             AUTOSHOP_CORRESPOND_SCRIPT",
            opts.script.display()
        );
    }
    // Stage both renditions as PNGs the sidecar can read: a RAW goes through
    // its embedded preview — correspondence is about CONTENT, and the sidecar
    // downsizes to 768 px anyway — and a baked image is decoded the same way.
    let staged = |p: &Path, tag: &str| -> Result<PathBuf> {
        let dst = std::env::temp_dir()
            .join(format!("autoshop-correspond-{}-{tag}.png", std::process::id()));
        decode::preview_only(p)?
            .save(&dst)
            .with_context(|| format!("stage {} for the correspondence sidecar", p.display()))?;
        Ok(dst)
    };
    let src_png = staged(source, "src")?;
    let tgt_png = staged(target, "tgt").inspect_err(|_| {
        let _ = std::fs::remove_file(&src_png);
    })?;
    let field = correspond::correspond_file(&opts, &src_png, &tgt_png, &out);
    // The staged PNGs are intermediates whatever the outcome.
    let _ = std::fs::remove_file(&src_png);
    let _ = std::fs::remove_file(&tgt_png);
    let field = field?;
    // The summary 7b's coverage reasoning will read, printed once here.
    let cells = field.confidence.len();
    let mut conf = field.confidence.clone();
    conf.sort_by(|a, b| a.total_cmp(b));
    let mut flow = 0.0f64;
    for i in 0..cells {
        let gx = (i % field.grid_w) as f64;
        let gy = (i / field.grid_w) as f64;
        flow += ((field.map_x[i] as f64 - gx).powi(2) + (field.map_y[i] as f64 - gy).powi(2))
            .sqrt();
    }
    println!(
        "correspondence field -> {} ({}x{} cells, {} @ {})",
        out.display(),
        field.grid_w,
        field.grid_h,
        field.model,
        &field.revision[..12]
    );
    println!(
        "  median confidence {:.3} · coverage(conf>=0.5) {:.1}% · mean |flow| {:.2} cells",
        conf[cells / 2],
        100.0 * field.coverage(0.5),
        flow / cells as f64
    );
    Ok(())
}

fn style_index_cmd(dir: &Path, embed: autoshop::style::EmbeddingSwitch) -> Result<()> {
    let index = StyleIndex::build(dir, embed)?;
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

fn style_looks_cmd(dir: &Path, embed: autoshop::style::EmbeddingSwitch) -> Result<()> {
    let index = StyleIndex::build_looks(dir, embed, &|_, _| {})?;
    let out = autoshop::store::style_index_path();
    index.save(&out)?;
    println!("look library -> {} ({} finished photos)", out.display(), index.looks.len());
    Ok(())
}

/// `--embed` / `--no-embed` as a VALUE, resolved once at the command's door.
///
/// It used to be an `unsafe` write of `AUTOSHOP_STYLE_EMBED` into the process
/// environment at three sites: a CLI flag implemented as a global side effect,
/// read back several call frames later. `cargo test` runs tests on parallel
/// threads in one process, so that made the switch a shared mutable global the
/// retrieval happened to read — see `style::EmbeddingSwitch`.
fn embed_switch(embed: bool, no_embed: bool) -> autoshop::style::EmbeddingSwitch {
    // clap already refuses both at once (`conflicts_with`), so this is a
    // three-state read, not a precedence decision.
    let flag = embed.then_some(true).or(no_embed.then_some(false));
    autoshop::style::EmbeddingSwitch::resolve(flag, false)
}

/// The two standardised text terms as the diagnostic prints them: the term that
/// entered the sum, the raw `1−cos` it came from, and whether the candidate set
/// was large enough to standardise at all.
///
/// `raw=–` marks a pair that had no comparable vectors (no direction text, or an
/// exemplar with no description embedding); such a candidate scores 0 — not a
/// penalty and not a bonus, and under the standardised variant that is exactly
/// the candidate-set mean. `,z` marks a term the standardised variant really
/// z-scored and `,raw-fallback` its disclosed give-up (fewer than three
/// comparable candidates, or a degenerate spread); the shipped RAW variant
/// prints neither, because for it the weighted gap is not a fallback.
fn fmt_text_terms(t: &autoshop::style::DistanceTerms) -> String {
    let one = |label: &str, term: f64, gap: Option<f64>, standardised: bool| -> String {
        let raw = gap.map(|g| format!("{g:.6}")).unwrap_or_else(|| "–".into());
        // `z` marks a term the ranking z-scored; nothing marks the raw
        // variant, which is what ships. `raw-fallback` is reserved for the
        // STANDARDISED variant giving up on a candidate set it could not
        // standardise — printing it on every line of a raw-variant run would
        // report a degradation that did not happen.
        let mark = match (autoshop::style::STANDARDISE_TEXT_TERMS, standardised) {
            (true, true) => ",z",
            (true, false) => ",raw-fallback",
            (false, _) => "",
        };
        format!(" {label}={term:.6}(raw={raw}{mark})")
    };
    format!(
        "{}{}",
        one("txt", t.txt, t.txt_gap, t.txt_standardised),
        one("desc", t.desc, t.desc_gap, t.desc_standardised)
    )
}

/// The offline retrieval diagnostic: what the ranking saw, in the ranking's own
/// numbers.
///
/// It re-derives NOTHING. The neighbour lines print `DistanceTerms` straight
/// out of `distance_components`, and the look lines out of
/// `retrieve_looks_with_terms` — the same call `retrieve_looks` makes. Both the
/// standardised term and the raw `1−cos` behind it are shown, because a
/// standardised term is only readable next to the weights that produced it, so
/// those are the header.
fn style_query_cmd(
    photo: &Path,
    direction: Option<&str>,
    style: f32,
    embed: autoshop::style::EmbeddingSwitch,
) -> Result<()> {
    let decoded = decode::decode_any(photo)?;
    let idx = match autoshop::style::load_effective() {
        autoshop::style::EffectiveIndex::Loaded(ix, _) => ix,
        autoshop::style::EffectiveIndex::Absent => anyhow::bail!("style index is absent"),
        autoshop::style::EffectiveIndex::Unusable { err, .. } => anyhow::bail!("style index unusable: {err}"),
    };
    // Resolved ONCE, printed, then passed to every scorer below: the diagnostic
    // cannot be read against weights it did not state.
    let weights = autoshop::style::RetrievalWeights::from_env();
    println!(
        "weights: W_EMB={:.6} W_TXT={:.6} W_DESC={:.6} W_LOOK={:.6}  text-term variant: {}",
        weights.emb, weights.txt, weights.desc, weights.look,
        if autoshop::style::STANDARDISE_TEXT_TERMS { "standardised (z-scored per query)" } else { "raw (the calibrated winner)" }
    );
    let (mut qi, mut qt) = (None, None);
    let mut embedding_status = "disabled (embedding switch is off)".to_string();
    if embed.on() {
        let opts = autoshop::embed::EmbedOpts::from_config(&Config::load());
        if !opts.available() {
            embedding_status = "unavailable (style-embedding sidecar is not present)".into();
        } else {
            match autoshop::style::embed_preview_with_text(&opts, &decoded.preview, &std::env::temp_dir(), "style-query", direction) {
                Ok(r) => {
                    qi = Some(r.vector);
                    qt = r.text_vector;
                    embedding_status = "ready (image vector plus optional direction text vector)".into();
                }
                Err(e) => embedding_status = format!("unavailable ({e:#}); using 14-D retrieval"),
            }
        }
    }
    println!("embedding: {embedding_status}");
    let query = autoshop::style::StyleQuery::new(qi.as_deref(), qt.as_deref(), weights);
    let (ex, looks_scored) = (
        pipeline::retrieve_style(&idx, &decoded.meta, &decoded.histogram, query, photo, true).0,
        idx.retrieve_looks_with_terms(query, 2),
    );
    let looks: Vec<_> = looks_scored.iter().map(|(e, _)| *e).collect();
    println!("neighbours:");
    for e in &ex {
        let t = idx.distance_components(&decoded.meta, &decoded.histogram, query, photo, e);
        println!("  {} distance={:.6} d14={:.6} emb={:.6}{}", e.stem, t.total(), t.d14, t.emb, fmt_text_terms(&t));
    }
    let reference = idx.render_reference_for_style(&ex, style);
    // The SHARED fence constants, not a second literal: the point of printing
    // the proposer's block is that it is the proposer's block.
    println!("reference (proposer block):\n{}", reference.as_deref()
        .map(|r| format!("{}{r}", autoshop::advisor::FENCE_STYLE_REFERENCE))
        .unwrap_or_else(|| "(none)".into()));
    if looks.is_empty() {
        if idx.looks.is_empty() {
            println!("looks: unreachable (look library is empty)");
        } else {
            println!("looks: unreachable (style embedding is off or no query vector was produced; {} finished photos)", idx.looks.len());
        }
    } else {
        // …and the block is worded from the WEIGHTS, like the develop's:
        // "and direction" appears only when a text weight is non-zero.
        let by_direction = autoshop::style::StyleIndex::look_ranked_by_direction(query);
        println!(
            "look reference (proposer block):\n{}{}",
            autoshop::advisor::FENCE_LOOK_REFERENCE,
            idx.render_look_reference(&looks, by_direction).unwrap_or_default()
        );
        for (l, t) in &looks_scored {
            println!(
                "  look {} distance={:.6} look={:.6}{} tags=[{}]",
                l.stem, t.total(), t.emb, fmt_text_terms(t), l.tags.join(", ")
            );
        }
    }
    println!("disclosure notes:");
    if !ex.is_empty() {
        println!("  {}", autoshop::rationale::keys::STYLE_NEIGHBOURS
            .replace("{files}", &autoshop::style::neighbour_stems(&ex).join(", "))
            .replace("{n}", &ex.len().to_string()));
    }
    if let Some(first) = looks.first() {
        println!("  {}", autoshop::rationale::keys::STYLE_LOOK_REFERENCE
            .replace("{stem}", &first.stem)
            .replace("{tags}", &first.tags.join(", ")));
        // The develop emits this one only when the look ALSO went to the vision
        // model as a second image, which is an opt-in this diagnostic does not
        // have (and never pays for). Printed with its condition stated, so the
        // line is a forecast and not a claim about this run.
        println!(
            "  (with the reference-image option on) {}",
            autoshop::rationale::keys::STYLE_LOOK_IMAGE.replace("{stem}", &first.stem)
        );
    } else if !idx.looks.is_empty() {
        println!("  {}", autoshop::rationale::keys::STYLE_LOOKS_UNREACHABLE
            .replace("{n}", &idx.looks.len().to_string()));
    }
    // DERIVED, through the pipeline's own mapping. It used to print the literal
    // "direct" whatever the dial said — a diagnostic that reported a tier the
    // develop would not have used. `style-query` has no `--adherence` flag, so
    // the dial is the shipped default and the line says which that is.
    let adherence = autoshop::recipe::DirectionAdherence::default();
    if let Some(tier) = pipeline::direction_adherence_tier(direction, adherence) {
        println!(
            "  {}   (from the DEFAULT adherence dial {:.2}; `analyze --adherence` moves it)",
            autoshop::rationale::keys::ADVISOR_NOTE_DIRECTION_ADHERENCE.replace("{tier}", tier),
            adherence.get()
        );
    }
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
    println!("source: {}", raw.display());
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
    let losses = autoshop::xmp::import_losses_for_photo(&text, raw);
    // The count the reader WOULD carry — the same number the GUI's open path
    // passes (`bin/gui/export.rs`), so the two surfaces cannot report the same
    // file differently. Taken through the CLAMPED door (R28 2b) because the
    // third sentence below needs what the size caps cut, and re-parsing the
    // document to learn it would be a second producer of one fact.
    let diag = autoshop::diag::photo(raw);
    let (parsed, clamped) = autoshop::xmp::xmp_to_recipe_clamped_with_diag(&text, &diag);
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
    adherence: Option<f32>,
    deep: bool,
    embed: autoshop::style::EmbeddingSwitch,
    cfg: &Config,
) -> GradeRequest {
    GradeRequest {
        style: style.unwrap_or(cfg.style_strength),
        send_reference_image: false,
        strength: GradeStrength::from_optional(strength),
        adherence: adherence.map(DirectionAdherence::new).unwrap_or_default(),
        use_looks: true,
        // The sidecar switch and the retrieval weights travel WITH the request
        // instead of being read back out of the process environment several
        // frames down (see `style::EmbeddingSwitch`).
        embed,
        weights: autoshop::style::RetrievalWeights::from_env(),
        // R23-4: opt-in per invocation, and per invocation only — `batch`
        // builds its request through `GradeRequest::with_style`, which has no
        // way to reach this flag.
        think: deep,
    }
}

#[allow(clippy::too_many_arguments)] // one clap subcommand's own flag set, like `auto_cmd`
fn analyze_cmd(
    raw: &Path,
    out: Option<PathBuf>,
    guidance: Option<String>,
    style: Option<f32>,
    strength: Option<f32>,
    adherence: Option<f32>,
    deep: bool,
    embed: autoshop::style::EmbeddingSwitch,
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
    let req = analyze_request(style, strength, adherence, deep, embed, &cfg);
    let (recipe, verdict, _notes) =
        produce_recipe(raw, &cfg, true, guidance.as_deref(), None, req, true, autoshop::diag::stderr())?;
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
    let recipe_path = write_recipe(raw, &recipe, out, autoshop::diag::stderr())?;
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
                pipeline::write_xmp_at(side, &recipe, &autoshop::diag::photo(raw))
            }
        } else {
            write_xmp(raw, &recipe, autoshop::diag::stderr())
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

/// The CLI's DELIVERY options, from the one knob the CLI exposes.
///
/// R29 Batch-2. The engine has carried [`render::ExportOpts`] since the gap
/// batches and the desktop Export panel has driven all five of its fields for
/// as long; the CLI passed `None` at every one of its four render sites, so the
/// only way to hand somebody a 2048 px JPEG was to render at full sensor
/// resolution and resize the result in another program — which is what the
/// README's own showcase footnote had to admit.
///
/// **A FLAG, not a recipe field** (user ruling, 2026-08-20). A delivery size is
/// a property of one export, not of the develop: the same recipe legitimately
/// produces a 61 MP master and a 2048 px web copy, and writing the size into
/// `recipe.json` would make those two the same photograph edited two ways.
/// Nothing about the schema moves for this batch.
///
/// `0` is FULL RESOLUTION, not "resize to nothing" — the same spelling the
/// GUI's Export panel uses (`exp_long_edge == 0` → `long_edge: None`,
/// `src/bin/gui/export.rs:445`) and what the engine already does with it
/// (`render::render_to_file`'s `le > 0` guard, `src/render.rs:1560`). The two
/// surfaces must not disagree about what the same number means, so the CLI
/// folds it to `None` HERE rather than leaving one surface's zero to be
/// absorbed by a guard three modules down.
///
/// Every other field stays at its default: this batch adds a size, not a
/// delivery panel, and a flag that silently also moved JPEG quality or the
/// colour space would be a second change wearing the first one's name.
fn export_opts(long_edge: Option<u32>) -> Option<render::ExportOpts> {
    let le = long_edge.filter(|n| *n > 0)?;
    Some(render::ExportOpts { long_edge: Some(le), ..Default::default() })
}

fn apply_cmd(raw: &Path, recipe_path: &Path, out: &Path, long_edge: Option<u32>) -> Result<()> {
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
    //
    // BOTH paths, and they are different things (R28 Batch-3, adjudication
    // F1-C): `raw` is the photo the cache is keyed and homed on, `src` is the
    // pixels the render below will use — a baked retouch master when one is
    // recorded. This site used to pass `src` for both, so a photo with a saved
    // master keyed its alpha on the master's path and looked for the cache in a
    // develop directory belonging to no photo.
    let ai = autoshop::segment::resolve_ai_masks(&Config::load(), raw, &src, &mut recipe);
    if let Some(line) = ai.describe() {
        println!("ai mask : {line}");
    }
    println!("rendering {} with {} ...", raw.display(), recipe_path.display());
    // The delivery size, if one was asked for. The develop above and the render
    // below are unchanged by it — `render_to_file` resizes the FINISHED pixels
    // as its last stage — so `--long-edge` never buys a different look, only a
    // different file size.
    let export = export_opts(long_edge);
    let (w, h) =
        render::render_to_file(&src, &recipe, out, None, export.as_ref(), autoshop::diag::stderr())?;
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
    adherence: Option<f32>,
    deep: bool,
    denoise: bool,
    denoise_strength: Option<f32>,
    denoise_model: Option<String>,
    long_edge: Option<u32>,
    embed: autoshop::style::EmbeddingSwitch,
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
    // Resolved BEFORE the paid call so the banner below can say what is coming
    // (it also costs nothing and cannot fail).
    let export = export_opts(long_edge);
    let req = analyze_request(style, strength, adherence, deep, embed, &cfg);
    // judge = true: `auto` is the explicit one-shot develop of ONE photo —
    // same interactive class as analyze (batch passes false).
    let (recipe, verdict, _notes) =
        produce_recipe(raw, &cfg, true, guidance.as_deref(), None, req, true, autoshop::diag::stderr())?;
    let accepted = verdict.decision == autoshop::advisor::Decision::Accept;
    // Opt-in AI denoise runs inside the render, before tone/sharpen.
    let dn = denoise
        .then(|| denoise::DenoiseOpts::from_config(&cfg, denoise_model, denoise_strength.unwrap_or(1.0)));
    println!(
        "verdict: {:?}; rendering {} ({}){} ...",
        verdict.decision,
        // The SIZE, honestly: this banner said "full-resolution" unconditionally,
        // and with `--long-edge` in the parser that sentence would have been
        // false for exactly the runs that asked for something else. The develop
        // still runs at full resolution — only the delivered file is bounded —
        // and "at most" is the truth for a frame already smaller than the cap,
        // which `render_to_file` saves untouched rather than upscaling.
        match export.and_then(|o| o.long_edge) {
            Some(le) => format!("to at most {le} px on the long edge"),
            None => "full-resolution".to_string(),
        },
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
                write_recipe(raw, &recipe, None, autoshop::diag::stderr())?;
            }
            let (src, relook) = autoshop::store::render_source_checked(raw, &mut render_recipe)
                .map_err(|m| anyhow::anyhow!(m))?;
            let xmp_result =
                (accepted && decode::is_raw(raw)).then(|| write_xmp(raw, &recipe, autoshop::diag::stderr()));
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
    // Same develop-time resolution as `apply` (see there, including the
    // photo-vs-pixels split). `render_recipe` is the render's own copy, so the
    // resolved raster paths never reach the saved recipe from here — the cache
    // is keyed by the photo, the intent and the frame, so the next develop
    // finds the same file anyway.
    let ai = autoshop::segment::resolve_ai_masks(&cfg, raw, &src, &mut render_recipe);
    if let Some(line) = ai.describe() {
        println!("ai mask : {line}");
    }
    let (w, h) = render::render_to_file(
        &src,
        &render_recipe,
        &out,
        dn.as_ref(),
        export.as_ref(),
        autoshop::diag::stderr(),
    )?;
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
///
/// **No `--long-edge` here, and the reason is not "we forgot"** (R29 Batch-2).
/// Two independent ones, either sufficient:
///
/// * This output is a MASTER, not a deliverable — its whole purpose is to be
///   the source of a later develop (`apply`/`auto` read it back through
///   `store::render_source_checked`). Delivering it at 2048 px would hand the
///   next develop a downscaled source and quietly cap every export made from
///   it, which is the opposite of what a master is for.
/// * Only the RAW arm below goes through `render_to_file` at all; the baked arm
///   calls `denoise::denoise_active`, which has no delivery pipeline. A flag on
///   this command would resize `.arw` inputs and silently ignore `.png` ones —
///   one flag with two behaviours decided by a file extension.
///
/// The route for a small denoised deliverable is the honest one: denoise to a
/// master, then `apply … --long-edge N` from it.
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
        let (w, h) =
            render::render_to_file(input, &EditRecipe::default(), &out, Some(&opts), None, autoshop::diag::stderr())?;
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
    regions: usize,
    style_prompt: bool,
    ai_judge: bool,
    deep: bool,
    strength: Option<f32>,
    out: Option<PathBuf>,
) -> Result<()> {
    if !(autoshop::fit_zoned::semantic::DEFAULT_SEMANTIC_REGIONS..=autoshop::fit_zoned::semantic::MAX_SEMANTIC_REGIONS).contains(&regions) {
        anyhow::bail!(
            "--regions must be between {} and {}",
            autoshop::fit_zoned::semantic::DEFAULT_SEMANTIC_REGIONS,
            autoshop::fit_zoned::semantic::MAX_SEMANTIC_REGIONS,
        );
    }
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
    // 完全自动 (user ruling 2026-08-26): every fit may consult the local DIFT
    // sidecar — the fit's own D gate decides IF (content-divergent pairs
    // only), first-run weight download included. Failures degrade into the
    // rationale, never fail the fit.
    let corr = autoshop::correspond::fit_provider(
        autoshop::correspond::CorrespondOpts::from_config(&Config::load()),
    );
    let fit_options = fit::FitOptions { strength: autoshop::recipe::GradeStrength::from_optional(strength), provider: Some(&corr) };
    println!("  reverse-fit strength: {:.0}%", fit_options.strength.get() * 100.0);
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
        let mask = autoshop::store::OwnedRaster::claim(raw, "mask-zone-sky")?;
        pipeline::guard_readonly(mask.path(), raw)?;
        println!(
            "  zoned: global -> semantic regions or native luminance ranges -> spatial tiles"
        );
        autoshop::fit_zoned::fit_recipe_zoned_with_regions(
            &src,
            &tgt,
            &seg,
            &mask,
            &EditRecipe::default(),
            fit_options,
            regions.min(autoshop::fit_zoned::semantic::MAX_SEMANTIC_REGIONS),
        )
        } else {
            fit::fit_recipe_with(&src, &tgt, fit_options)
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
                recipe: Some(pipeline::recipe_store_bytes(raw, &rep.recipe, autoshop::diag::stderr())?),
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
        write_recipe(raw, &rep.recipe, Some(out), autoshop::diag::stderr())?
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
        match write_xmp(raw, &rep.recipe, autoshop::diag::stderr()) {
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
        let (w, h) = render::render_to_file(raw, &rep.recipe, &img_out, None, None, autoshop::diag::stderr())?;
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
    long_edge: Option<u32>,
) -> Result<()> {
    let cfg = Config::load();
    // PER PHOTO, and it has to be: one `ExportOpts` shared by every worker
    // still bounds each photo's own long edge, because the cap is applied to
    // the frame being rendered — a portrait and a landscape in the same folder
    // both come out at N on THEIR long edge, not at one common width.
    let export = export_opts(long_edge);
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
    // STDERR was NOT covered, and this comment claimed it was until R28 2e
    // (adjudication F6). `verbose` gates the progress chatter, not the
    // warnings: the proposer fallback, the clamp disclosure, the XMP merge and
    // mask-loss notes, the "XMP failed" line in `process_one` below and the
    // render's own ICC / inert-raster warnings were all ungated `eprintln!`s
    // landing in COMPLETION order.
    //
    // R28 Batch-5 5c stamped every one of them that had a photograph in scope
    // with its stem, so a reordered line could at least be attributed, and made
    // this closure render `produce_recipe`'s typed note channel into the photo's
    // own block the way `eval` always did. It registered what a prefix could
    // not do — route, suppress or ORDER — as still open.
    //
    // R29-1 closes it. Each worker below hands `process_one` a
    // `diag::Collector` of its own and drains it into THIS photo's block, so
    // the develop chain's disclosures are released by the sequencer in INDEX
    // order with the rest of the photo's transcript. Two consequences worth
    // stating plainly: those lines now appear on STDOUT inside the block rather
    // than on stderr, and a photo's warnings wait for the block that carries
    // them (a slow photo 1 holds 2..k, exactly as its `[i/n]` line already
    // did). The pure-pixel preview arm is typed rather than un-stamped —
    // `diag::Subject::PixelOnly` — and `batch` never reaches it anyway.
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
    // Said UP FRONT, like the same-name deviations above: an unattended run
    // over a library is exactly where a size nobody meant to ask for is
    // expensive to discover afterwards.
    if let Some(le) = export.and_then(|o| o.long_edge) {
        println!("  deliverables capped at {le} px on the long edge (aspect kept, never upscaled).");
    }
    // `plan_for`, not `plan`: `--include-baked` puts Lightroom "Edit in…"
    // exports on this list, and a native-resolution 16-bit TIFF can peak far
    // past the corpus constant on its own (R28 Batch-4 4a). Their headers are
    // free to read, so the budget asks them.
    let plan = autoshop::jobs::plan_for(jobs, &work);
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
        // THIS photo's diagnostics channel (R29-1). One collector per worker is
        // what makes the lines separable at all: a shared sink would hand back
        // three photos' warnings in completion order again, only inside a
        // Vec instead of on a stream.
        let diags = autoshop::diag::Collector::new();
        let res = process_one(raw, &cfg, outs[i].as_deref(), export.as_ref(), &diags);
        let outcome = match &res {
            Ok((v, _)) if v.decision == autoshop::advisor::Decision::Accept => {
                let _ = writeln!(block, "[{}/{n}] {} ... {:?}", i + 1, stem(raw), v.decision);
                Outcome::Saved
            }
            Ok((v, _)) => {
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
        };
        // The typed note channel, rendered into THIS photo's block — the same
        // thing `eval` does with the same notes (eval.rs, its own worker
        // closure), and what `jobs`' module doc asks every pooled caller to do.
        // Above one job the ⚠ lines these notes mirror (the proposer fallback
        // warn in `pipeline::produce_recipe` and its siblings) arrive on stderr in COMPLETION
        // order; the block is the channel that keeps them attached to the
        // photograph. At `--jobs 1` the transcript gains the same line in the
        // same place, so nothing about a serial run's ordering changes.
        if let Ok((_, notes)) = &res
            && !notes.is_empty()
        {
            let text = autoshop::rationale::render_en(notes);
            let text = text.trim();
            if !text.is_empty() {
                let _ = writeln!(block, "       {text}");
            }
        }
        // …and the DIAGNOSTICS channel, drained into the same block. Both
        // arms are rendered — they are different statements of the same
        // events: a note is the rationale fragment the recipe carries, a
        // diagnostic is the raw failure that produced it, and dropping either
        // would leave a surface with less than it has today.
        autoshop::diag::write_into_block(block, "       ", &diags.take());
        outcome
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

/// One batch photo. Returns the verdict AND the deterministic notes
/// `produce_recipe` raised for it (R28 Batch-5 5c).
///
/// The notes used to be dropped on the floor here (`let (.., _notes)`) while
/// `jobs`' module doc told callers to render them into the photo's block, and
/// `eval` — the other pooled caller — did exactly that. So the one surface
/// where a proposer fallback is most likely to happen (an unattended
/// library-sized run) was also the one that threw away the attributed copy of
/// the fact, leaving only the reordered stderr line.
///
/// `sink` is the caller's diagnostics channel for everything this photo's pass
/// raises (R29-1) — the develop chain's, the render's, and the one line this
/// function owns. `batch` passes a per-worker collector; the SUBJECT is bound
/// inside each entry point from `raw`, so nothing here can mis-attribute.
fn process_one(
    raw: &Path,
    cfg: &Config,
    render_to: Option<&Path>,
    export: Option<&render::ExportOpts>,
    sink: &dyn autoshop::diag::Sink,
) -> Result<(Verdict, Vec<autoshop::rationale::Note>)> {
    // Batch uses the configured style strength (AUTOSHOP_STYLE_STRENGTH).
    // judge = false: a library-sized batch must never silently multiply the
    // paid vision calls — the closed loop is for the interactive surfaces
    // (review R20-M2).
    let (recipe, verdict, notes) = produce_recipe(
        raw,
        cfg,
        false,
        None,
        None,
        GradeRequest::with_style(cfg.style_strength),
        false,
        sink,
    )?;
    // A non-Accept verdict may not auto-save (user decision). In a headless
    // batch that means NO sidecars and NO deliverable: the photo stays
    // pending, the caller's summary names it, and a re-run re-attempts it —
    // the same consequence a failed photo already has.
    if verdict.decision != autoshop::advisor::Decision::Accept {
        return Ok((verdict, notes));
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
        write_recipe(raw, recipe, None, sink)?;
        // The recipe write ALONE decides the saved state (cross-surface rule):
        // failing the photo here made the batch report it failed while the
        // resume filter — which sees recipe.json — permanently skipped it.
        if let Err(e) = write_xmp(raw, recipe, sink) {
            // The one disclosure the batch worker itself owns, and the
            // "main.rs:1586" the `jobs` disclosure and the comment above name
            // (both re-located at R28 HEAD). It fires INSIDE the pool, after
            // the develop lock, so at `--jobs 3` it used to land between two
            // other photos' output with nothing to tie it to its own; it now
            // goes to the caller's channel, which puts it in this photo's
            // block. `WarnNested` keeps the "  ⚠ " the CLI's XMP-failure
            // family has always worn.
            autoshop::diag::Diag::about(sink, raw).emit(
                autoshop::diag::Mark::WarnNested,
                format!("recipe saved, but the Lightroom XMP failed: {e}"),
            );
        }
        Ok(())
            },
        )
    };
    if let Some(out) = render_to {
        // 16-bit master at the batch-claimed name — claimed up front by the
        // caller so same-stem photos in one batch each keep a deliverable
        // and worker scheduling cannot reorder the names. `export` is the
        // caller's `--long-edge`, and it bounds THIS photo's own long edge
        // (R29 Batch-2): the develop is unaffected, the delivered file is not.
        ensure_parent(out)?;
        if let Err(e) = render::render_to_file(raw, &recipe, out, None, export, sink) {
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
    // The notes travel with the SUCCESS arm only. A failed photo returns an
    // `Err` whose message the block already prints, and the render-failure arm
    // above says what happened to the analysis — attaching a proposer-fallback
    // note under a FAILED line would read as a second, unrelated failure.
    Ok((verdict, notes))
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
            correspond_script: String::new(),
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
            match_cmd(&src, &target, false, false, 4, false, false, false, None, None)
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
        let dir = std::env::temp_dir().join(format!("autoshop-cli-same-path-case-{}", std::process::id()));
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
        let cli = Cli::try_parse_from(["autoshop", "match", "source.png", "target.png", "--strength", "0.85"])
            .expect("match --strength must parse");
        match cli.command {
            Command::Match { strength, .. } => assert_eq!(strength, Some(0.85)),
            _ => panic!("match parser returned another command"),
        }
        // …and the flag actually decides the request `analyze`/`auto` build —
        // the PRODUCTION resolver, shared by both commands.
        let cfg = autoshop::config::Config { style_strength: 0.3, ..cfg_fixture() };
        // The switch is a VALUE these dial assertions are indifferent to, so it
        // is spelled once rather than resolved from the environment per call.
        const OFF: autoshop::style::EmbeddingSwitch = autoshop::style::EmbeddingSwitch::OFF;
        let plain = analyze_request(None, None, None, false, OFF, &cfg);
        assert_eq!(plain.style, 0.3, "omitted --style keeps AUTOSHOP_STYLE_STRENGTH");
        assert_eq!(
            plain.strength.get(),
            GradeStrength::DEFAULT,
            "omitted --strength is the SHIPPED default (0.65) — the CLI and a double-clicked \
             GUI must develop the same photo the same way when neither is told otherwise"
        );
        let dialled = analyze_request(Some(0.1), Some(0.9), None, false, OFF, &cfg);
        assert_eq!((dialled.style, dialled.strength.get()), (0.1, 0.9), "no axis swap");
        assert!(!dialled.send_reference_image, "every non-GUI surface stays on the text reference");
        // 0.5 is the calibration point, one flag away.
        assert_eq!(
            analyze_request(None, Some(0.5), None, false, OFF, &cfg).strength,
            GradeStrength::calibrated()
        );
        // R23-4: `--deep` is the ONLY way this resolver's thinking flag turns
        // on, and it is a THIRD axis — it must not be confusable with either
        // dial (`batch` never reaches this function at all).
        assert!(!plain.think, "omitted --deep is off, like every paid opt-in");
        assert!(analyze_request(None, None, None, true, OFF, &cfg).think, "--deep must reach the request");
        assert!(
            !analyze_request(Some(1.0), Some(1.0), None, false, OFF, &cfg).think,
            "pushing both dials to the maximum must not buy the thinking envelope"
        );
    }

    #[test]
    fn cli_match_strength_reaches_the_budget() {
        let cli = Cli::try_parse_from([
            "autoshop",
            "match",
            "source.png",
            "target.png",
            "--strength",
            "1.0",
        ])
        .expect("match strength parses");
        let strength = match cli.command {
            Command::Match { strength, .. } => GradeStrength::from_optional(strength),
            _ => panic!("match parser returned another subcommand"),
        };
        let budget = autoshop::fit::FitBudget::for_strength(strength);
        assert_eq!(budget.ev, 2.5);
        assert_eq!(budget.sat, 60.0);
        assert_eq!(budget.vetoes, autoshop::fit::VetoPolicy::Disclose);
    }
    /// L09#1 ordering: with a nonexistent RAW, a bad `-o` must fail on the
    /// OUTPUT (pre-pay preflight), never on decode or a paid call.
    #[test]
    fn analyze_and_auto_refuse_a_bad_output_before_the_paid_call() {
        let dir = std::env::temp_dir()
            .join(format!("autoshop-prepay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let off = autoshop::style::EmbeddingSwitch::OFF;
        let e = analyze_cmd(Path::new("no-such.arw"), Some(dir.clone()), None, None, None, None, false, off)
            .unwrap_err()
            .to_string();
        assert!(e.contains("is a directory"), "analyze: {e}");
        let e = auto_cmd(
            Path::new("no-such.arw"),
            Some(dir.join("x.xyz")),
            None,
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            off,
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("unsupported output format"), "auto: {e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Step 7a, same L09#1 family: `correspond` refuses a bad `-o` on the
    /// OUTPUT — before any decode, and long before the sidecar's first-run
    /// 2.6 GB weight download could start.
    #[test]
    fn correspond_refuses_a_bad_output_before_any_decode() {
        let dir = std::env::temp_dir()
            .join(format!("autoshop-corr-prepay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let e = correspond_cmd(
            Path::new("no-such.arw"),
            Path::new("also-no-such.png"),
            Some(dir.clone()),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("is a directory"), "correspond: {e}");
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

    /// R29 Batch-2: `--long-edge` is on every command that DELIVERS an image,
    /// and on none that does not.
    ///
    /// The engine has carried `ExportOpts` for several rounds and the desktop
    /// Export panel drove all five of its fields, while all four CLI render
    /// sites passed `None` — so the README had to admit, twice, that resizing
    /// for the web happened in another program. The three commands that hand
    /// the photographer a finished picture (`apply`, `auto`, `batch --render`)
    /// now take the size; `denoise` does not, and `denoise_cmd`'s own doc says
    /// why (its output is a MASTER a later develop reads back, and only its RAW
    /// arm goes through `render_to_file` at all — one flag with two behaviours
    /// decided by a file extension).
    ///
    /// MUTATION THIS KILLS: wiring the flag into the parser and dropping it on
    /// the way to `render_to_file` (every "does it parse" assertion still
    /// passes); folding `0` to `Some(0)` instead of `None`, which the engine
    /// currently absorbs but which makes the CLI and the GUI mean different
    /// things by the same number; letting `--long-edge` also move JPEG quality
    /// or the delivery colour space while wearing the size's name.
    #[test]
    fn long_edge_is_the_export_size_on_the_commands_that_deliver_an_image() {
        let parsed = |args: &[&str]| -> Option<u32> {
            match Cli::try_parse_from(args)
                .unwrap_or_else(|e| panic!("{args:?} must parse: {e}"))
                .command
            {
                Command::Apply { long_edge, .. }
                | Command::Auto { long_edge, .. }
                | Command::Batch { long_edge, .. } => long_edge,
                _ => panic!("{args:?} parsed as some other subcommand"),
            }
        };
        // Carried, not dropped, on all three.
        assert_eq!(
            parsed(&["autoshop", "apply", "p.arw", "r.json", "-o", "x.jpg", "--long-edge", "2048"]),
            Some(2048)
        );
        assert_eq!(parsed(&["autoshop", "auto", "p.arw", "--long-edge", "1600"]), Some(1600));
        assert_eq!(
            parsed(&["autoshop", "batch", "dir", "--render", "--long-edge", "1024"]),
            Some(1024)
        );
        // Omitted stays omitted — an unflagged run is byte-for-byte the run the
        // release before this one made.
        assert_eq!(parsed(&["autoshop", "apply", "p.arw", "r.json", "-o", "x.jpg"]), None);
        assert_eq!(parsed(&["autoshop", "auto", "p.arw"]), None);
        assert_eq!(parsed(&["autoshop", "batch", "dir", "--render"]), None);

        // `batch --long-edge` without `--render` writes no pixels at all, so a
        // size for it is a typo rather than a setting: refused at the parser,
        // the same door `--denoise-strength` without `--denoise` uses.
        let Err(e) = Cli::try_parse_from(["autoshop", "batch", "dir", "--long-edge", "1024"])
        else {
            panic!("a delivery size with no deliverable must be refused");
        };
        let e = e.to_string();
        assert!(e.contains("--render"), "the refusal must name the missing flag: {e}");

        // NOT on `denoise` — see `denoise_cmd`'s doc for the two reasons.
        assert!(
            Cli::try_parse_from(["autoshop", "denoise", "p.arw", "--long-edge", "1024"]).is_err(),
            "denoise delivers a master, not a sized deliverable"
        );

        // A negative size is not a size (u32 at the parser, not a late clamp).
        assert!(Cli::try_parse_from(["autoshop", "auto", "p.arw", "--long-edge", "-1"]).is_err());

        // …and the RESOLUTION of the flag into delivery options, which is the
        // half a parse test cannot see.
        assert_eq!(export_opts(None), None, "omitted = the shipped full-resolution path");
        assert_eq!(
            export_opts(Some(0)),
            None,
            "0 = full resolution, the same spelling the GUI Export panel uses \
             (src/bin/gui/export.rs:445) — not a 0 px deliverable"
        );
        let opts = export_opts(Some(2048)).expect("a positive size is a size");
        assert_eq!(opts.long_edge, Some(2048));
        assert_eq!(
            render::ExportOpts { long_edge: None, ..opts },
            render::ExportOpts::default(),
            "the size flag must move the size and nothing else"
        );
    }

    /// The WIRE, EXECUTED — the one assertion the parse test above cannot make.
    ///
    /// Every assertion in `long_edge_is_the_export_size_…` is satisfied by a
    /// parser that carries the number to a `render_to_file` call still passing
    /// `None`, and that is not hypothetical: dropping `export.as_ref()` in
    /// `apply_cmd` leaves the ENTIRE battery green (measured while writing
    /// this — 749 lib / 13 CLI / 2 / 2 all pass, and only `-D warnings`
    /// notices, because the binding goes unused). So this runs the command and
    /// reads the dimensions off the file it wrote.
    ///
    /// `auto` and `batch --render` build their options through the same
    /// `export_opts` and hand them to the same parameter of the same function;
    /// neither is executable in a test, because both begin with a billed API
    /// call. That is stated rather than papered over.
    #[test]
    fn apply_delivers_at_the_size_it_was_asked_for() {
        let root =
            std::env::temp_dir().join(format!("autoshop-long-edge-wire-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let library = root.join("library");
        std::fs::create_dir_all(&library).unwrap();
        // A baked source: the develop engine takes either, and this keeps the
        // test free of a RAW fixture the repo does not carry.
        let src = library.join("frame.png");
        image::RgbImage::from_fn(400, 300, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 233) as u8])
        })
        .save(&src)
        .unwrap();
        let recipe_p = library.join("r.json");
        std::fs::write(&recipe_p, serde_json::to_string(&EditRecipe::default()).unwrap()).unwrap();

        // The size asked for is the size delivered — 400×300 fitted to a 100 px
        // long edge is 100×75, and the file on disk has to say so.
        let out = root.join("deliver").join("small.png");
        apply_cmd(&src, &recipe_p, &out, Some(100)).unwrap();
        assert_eq!(
            image::image_dimensions(&out).unwrap(),
            (100, 75),
            "--long-edge must reach the engine, not just the parser"
        );

        // …and the three ways of asking for "no resize" all deliver the
        // source's own resolution: omitted, 0, and a cap past the frame.
        for (i, le) in [None, Some(0), Some(9999)].into_iter().enumerate() {
            let p = root.join("deliver").join(format!("full{i}.png"));
            apply_cmd(&src, &recipe_p, &p, le).unwrap();
            assert_eq!(
                image::image_dimensions(&p).unwrap(),
                (400, 300),
                "--long-edge {le:?} must deliver the frame untouched"
            );
        }

        // The store-test pattern: this photo hashes to its own develop dir, and
        // nothing here is entitled to leave one behind.
        let _ = std::fs::remove_dir_all(autoshop::store::develop_dir(&src));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The `--long-edge` help has to answer the three questions a photographer
    /// asks before trusting a resize to a batch they will not watch: which
    /// resampler, does it ever ENLARGE, and what does 0 do. All three are
    /// behaviours this repo already has somewhere else in a different spelling
    /// (`serve`'s preview resizes with `Triangle`, `src/serve.rs:860`), so
    /// "read the code" is not an answer.
    ///
    /// MUTATION THIS KILLS: swapping the export resampler in
    /// `render::render_to_file` without touching the sentence that promises
    /// Lanczos3 — the doc-drift failure `scripts/check_docs.py` exists for,
    /// caught here for a claim that lives in `--help` rather than in a `.md`.
    #[test]
    fn the_long_edge_help_names_the_resampler_and_the_two_edge_cases() {
        use clap::CommandFactory as _;
        let cmd = Cli::command();
        let apply =
            cmd.get_subcommands().find(|c| c.get_name() == "apply").expect("apply subcommand");
        let arg =
            apply.get_arguments().find(|a| a.get_id() == "long_edge").expect("--long-edge arg");
        let help = arg.get_long_help().or_else(|| arg.get_help()).expect("help text").to_string();
        assert!(help.contains("Lanczos3"), "the resampler must be named: {help}");
        assert!(help.contains("upscale"), "the never-enlarge rule must be stated: {help}");
        assert!(help.contains('0'), "what 0 means must be stated: {help}");
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

    /// Every flag S1 added carries `--help` text, and `style-query` carries a
    /// description.
    ///
    /// clap prints a bare `--adherence <ADHERENCE>` for an undocumented flag,
    /// which is indistinguishable from a flag that does nothing. The tiers in
    /// particular cannot be guessed from a 0..1 number: the value picks a NAME
    /// the prompt is told, and the name is not monotone in the way a user would
    /// assume, so the help has to say the bands.
    ///
    /// MUTATION: delete any one of the doc comments on those clap fields and
    /// this fails, naming it.
    #[test]
    fn the_new_cli_surface_documents_itself() {
        use clap::CommandFactory;
        let render = |name: &str| -> String {
            let mut cmd = Cli::command();
            let sub = cmd
                .find_subcommand_mut(name)
                .unwrap_or_else(|| panic!("no `{name}` subcommand"));
            sub.render_long_help().to_string()
        };
        let analyze = render("analyze");
        for (flag, phrase) in [
            ("--adherence", "Hint"),
            ("--adherence", "Direct"),
            ("--adherence", "Brief"),
            ("--embed", "SigLIP 2 sidecar"),
            ("--no-embed", "14-dim ranking"),
        ] {
            assert!(analyze.contains(flag), "analyze must offer {flag}");
            assert!(analyze.contains(phrase), "analyze --help must explain {flag}: missing {phrase:?}");
        }
        // `auto` carries the same three flags and must explain them too.
        let auto = render("auto");
        for phrase in ["Hint", "Direct", "Brief", "SigLIP 2 sidecar"] {
            assert!(auto.contains(phrase), "auto --help must explain the flag: missing {phrase:?}");
        }
        // `style-index --looks` says what a look library IS, since it is not
        // the same thing as the RAW library the same command builds.
        let index = render("style-index");
        assert!(index.contains("FINISHED photos"), "{index}");
        assert!(index.contains("--looks"), "{index}");
        // …and `style-query` says what it prints and that it spends nothing.
        let query = render("style-query");
        for phrase in ["weights in force", "No advisor call", "--direction", "--embed"] {
            assert!(query.contains(phrase), "style-query --help must say {phrase:?}: {query}");
        }
        // No flag on any of the three may ship with an empty help line.
        for name in ["analyze", "auto", "style-index", "style-query"] {
            let mut cmd = Cli::command();
            let sub = cmd.find_subcommand_mut(name).unwrap();
            for arg in sub.get_arguments() {
                assert!(
                    arg.get_help().is_some() || arg.get_long_help().is_some(),
                    "`{name} --{}` has no help text",
                    arg.get_id()
                );
            }
        }
    }

    /// The flags reach the REQUEST, as values.
    ///
    /// `--embed` used to be implemented by writing `AUTOSHOP_STYLE_EMBED` into
    /// the process and letting `produce_recipe` read it back, so the flag was a
    /// global side effect and the develop's switch was whatever the last
    /// command (or a parallel test) had written. Now the resolver answers a
    /// value, the request carries it, and the environment is not written at
    /// all — which is what the snapshot below states.
    ///
    /// MUTATION: make `embed_switch` ignore its flag arguments and the
    /// `--no-embed` assertion fails.
    #[test]
    fn cli_adherence_and_embed_flags_reach_the_request() {
        let before = std::env::var_os("AUTOSHOP_STYLE_EMBED");
        let cli = Cli::try_parse_from([
            "autoshop", "analyze", "photo.arw", "--guidance", "warmer",
            "--adherence", "0.2", "--embed",
        ]).expect("the new flags parse");
        match cli.command {
            Command::Analyze { adherence, embed, no_embed, .. } => {
                assert_eq!(adherence, Some(0.2));
                assert!(embed && !no_embed);
                let req = analyze_request(
                    None, None, adherence, false, embed_switch(embed, no_embed), &Config::load(),
                );
                assert_eq!(req.adherence.tier(), autoshop::recipe::AdherenceTier::Hint);
                assert!(req.embed.on(), "--embed must reach the request as a value");
            }
            _ => panic!("expected analyze command"),
        }
        // The opposite flag, and the one that used to WRITE the variable.
        let cli = Cli::try_parse_from(["autoshop", "auto", "photo.arw", "--no-embed"])
            .expect("--no-embed parses");
        match cli.command {
            Command::Auto { embed, no_embed, .. } => {
                assert!(!embed && no_embed);
                assert!(
                    !embed_switch(embed, no_embed).on(),
                    "--no-embed must win over whatever the environment says"
                );
            }
            _ => panic!("expected auto command"),
        }
        assert_eq!(
            before,
            std::env::var_os("AUTOSHOP_STYLE_EMBED"),
            "resolving the flags must not write the process environment"
        );
    }
}
