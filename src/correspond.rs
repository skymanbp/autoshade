//! Cross-image correspondence bridge — Rust side of `python/correspond.py`.
//!
//! Fourth sidecar, same shell-out pattern as [`crate::denoise`],
//! [`crate::segment`] and [`crate::embed`]: a local Python process runs the
//! DIFT featurizer (Stable Diffusion 2.1, one UNet pass per noise draw) over
//! TWO renditions of the same frame and writes a JSON *correspondence field* —
//! for every cell of a 48×48 grid over the source, where that content sits in
//! the target and how much the match can be trusted. The weights auto-download
//! to `python/weights` on first run (~2.6 GB, digest pinned) — nothing is
//! stored in this repo.
//!
//! **Why it exists (plan step 7).** The reverse-fit's atmosphere mode fires
//! when the pair's CONTENT diverges (structure divergence `D ≥ 0.35`) — a
//! generated target that replaced the sky or invented a cloud deck. Today that
//! mode compares grid cells at the SAME coordinates, which is exactly what a
//! content-divergent pair breaks. This field is the missing input: which cells
//! still correspond (and where), and which have no counterpart at all. Step 7a
//! ships the instrument + a CLI diagnostic door; the estimator wiring is 7b.
//!
//! **It never generates pixels** — the output is coordinates and confidences.
//! And like the embedding, it is ADDITIVE: absence of the sidecar degrades to
//! today's behaviour, it never turns a working fit off.
//!
//! The run itself is [`crate::run_model_sidecar`] — the shared
//! spawn/bound/exit-0-is-not-success executor — under the process-wide
//! [`crate::with_model_slot`], so this model and the style embedding are
//! never resident together.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config::Config;

/// The feature-grid edge this build expects — `python/correspond.py`'s
/// `GRID`, which is 768 px input / 16 (the UNet `up_blocks[1]` stride). Not a
/// preference: [`parse_field`] refuses any other size, because a checkpoint
/// answering a different grid would silently change which pixels every cell
/// of a saved field refers to.
pub const GRID: usize = 48;

/// Everything one correspondence run needs; built from [`Config`] like
/// [`crate::embed::EmbedOpts`].
pub struct CorrespondOpts {
    pub python_bin: String,
    pub script: PathBuf,
}

impl CorrespondOpts {
    pub fn from_config(cfg: &Config) -> Self {
        CorrespondOpts {
            python_bin: cfg.python_bin.clone(),
            script: PathBuf::from(&cfg.correspond_script),
        }
    }

    /// Is the sidecar even present? The fit will ask BEFORE staging pixels,
    /// so a machine without it degrades once, with a sentence, instead of
    /// erroring per pair.
    pub fn available(&self) -> bool {
        !self.script.as_os_str().is_empty() && self.script.exists()
    }
}

/// The sidecar's argv, as a pure function of what decides it — pinned by a
/// test, like [`crate::embed`]'s. No precision flag: the sidecar picks fp16
/// on CUDA and fp32 on CPU by itself (the field is coordinates, not pixels,
/// and its gates are scale-free — there is no fp32 escape hatch to preserve).
fn sidecar_args(script: &Path, source: &Path, target: &Path, output: &Path) -> Vec<std::ffi::OsString> {
    vec![
        // `-E`: the second layer against a PYTHON* import hijack (the env
        // allowlist in `dotenv_child_env` is the first).
        "-E".into(),
        script.into(),
        "--source".into(),
        source.into(),
        "--target".into(),
        target.into(),
        "--output".into(),
        output.into(),
    ]
}

/// One correspondence field, parsed and gated. Flat arrays over source cells
/// in row-major order (`cell = y * grid_w + x`); `map_x`/`map_y` are TARGET
/// grid coordinates, `confidence` is the sidecar's cyclic×smoothness product
/// in [0, 1], and `sim` is the raw cosine kept for diagnostics only.
#[derive(Debug)]
pub struct CorrespondenceField {
    pub model: String,
    pub revision: String,
    pub grid_w: usize,
    pub grid_h: usize,
    pub input_size: u32,
    pub map_x: Vec<f32>,
    pub map_y: Vec<f32>,
    pub confidence: Vec<f32>,
    pub sim: Vec<f32>,
}

impl CorrespondenceField {
    /// Share of source cells whose match clears `threshold` — the one-number
    /// summary the diagnostic prints and 7b's coverage reasoning will read.
    pub fn coverage(&self, threshold: f32) -> f32 {
        if self.confidence.is_empty() {
            return 0.0;
        }
        self.confidence.iter().filter(|&&c| c >= threshold).count() as f32
            / self.confidence.len() as f32
    }
}

/// Run the sidecar on ONE pair and return the parsed field.
///
/// `output` is the JSON file the sidecar writes — and unlike the embedding's
/// scratch it is NOT deleted on success: for the CLI diagnostic the field
/// file IS the deliverable (the caller names it), and a fit caller that
/// wants it gone owns that lifetime. An UNPARSEABLE field is discarded,
/// though — a file that failed its own gates must not be left behind looking
/// like a result.
pub fn correspond_file(
    opts: &CorrespondOpts,
    source: &Path,
    target: &Path,
    output: &Path,
) -> Result<CorrespondenceField> {
    if !opts.script.exists() {
        bail!(
            "correspondence sidecar not found at {} — run from the project dir or set \
             AUTOSHOP_CORRESPOND_SCRIPT.",
            opts.script.display()
        );
    }
    let before = crate::artifact_state(output);
    let text = crate::run_model_sidecar(
        "correspondence sidecar",
        &opts.python_bin,
        sidecar_args(&opts.script, source, target, output),
        output,
    )?;
    let field = parse_field(&text).with_context(|| {
        format!("correspondence sidecar wrote an unusable field at {}", output.display())
    });
    if field.is_err() {
        crate::denoise::discard_failed_output(output, before);
    }
    field
}

/// One sidecar JSON record → the field, with every invariant a consumer would
/// otherwise have to re-derive. Refuses, rather than repairs: a record that
/// carries an `error`, a grid other than [`GRID`]², an array of the wrong
/// length, a coordinate outside the target grid, a confidence outside [0, 1],
/// and any non-finite element. The coordinate gate is the load-bearing one —
/// 7b will INDEX target rasters with these values, and an out-of-grid entry
/// would be an out-of-bounds read waiting in a saved-recipe path.
pub fn parse_field(text: &str) -> Result<CorrespondenceField> {
    let rec: serde_json::Value =
        serde_json::from_str(text.trim()).context("correspondence output is not JSON")?;
    if let Some(e) = rec.get("error").and_then(|v| v.as_str()) {
        bail!("correspondence sidecar declined this pair: {e}");
    }
    let usize_of = |k: &str| -> Result<usize> {
        rec.get(k)
            .and_then(|v| v.as_u64())
            .with_context(|| format!("correspondence output has no `{k}`"))
            .map(|v| v as usize)
    };
    let grid_w = usize_of("grid_w")?;
    let grid_h = usize_of("grid_h")?;
    if grid_w != GRID || grid_h != GRID {
        bail!(
            "correspondence sidecar answered a {grid_w}x{grid_h} grid; this build reads \
             {GRID}x{GRID} fields, and mixing grids changes which pixels every cell means"
        );
    }
    let cells = grid_w * grid_h;
    let arr = |k: &str| -> Result<Vec<f32>> {
        let a = rec
            .get(k)
            .and_then(|v| v.as_array())
            .with_context(|| format!("correspondence output has no `{k}` array"))?;
        if a.len() != cells {
            bail!("`{k}` holds {} entries for a {cells}-cell grid", a.len());
        }
        let mut v = Vec::with_capacity(cells);
        for x in a {
            let f = x.as_f64().with_context(|| format!("`{k}` holds a non-number"))? as f32;
            if !f.is_finite() {
                bail!("`{k}` holds a non-finite element");
            }
            v.push(f);
        }
        Ok(v)
    };
    let map_x = arr("map_x")?;
    let map_y = arr("map_y")?;
    let confidence = arr("confidence")?;
    let sim = arr("sim")?;
    for (name, v, hi) in [("map_x", &map_x, grid_w as f32), ("map_y", &map_y, grid_h as f32)] {
        if v.iter().any(|&c| !(0.0..hi).contains(&c)) {
            bail!("`{name}` holds a coordinate outside the {grid_w}x{grid_h} target grid");
        }
    }
    if confidence.iter().any(|&c| !(0.0..=1.0).contains(&c)) {
        bail!("`confidence` holds a value outside [0, 1]");
    }
    // Raw cosine of two unit vectors; 1e-3 of slack for the f32 round-trip.
    if sim.iter().any(|&s| !(-1.001..=1.001).contains(&s)) {
        bail!("`sim` holds a value no cosine can reach");
    }
    let str_of = |k: &str| -> Result<String> {
        rec.get(k)
            .and_then(|v| v.as_str())
            .with_context(|| format!("correspondence output names no `{k}`"))
            .map(str::to_string)
    };
    Ok(CorrespondenceField {
        model: str_of("model")?,
        revision: str_of("revision")?,
        grid_w,
        grid_h,
        input_size: rec
            .get("input_size")
            .and_then(|v| v.as_u64())
            .context("correspondence output has no `input_size`")? as u32,
        map_x,
        map_y,
        confidence,
        sim,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record builder with every array the right length, mutable per test.
    fn record(edit: impl FnOnce(&mut serde_json::Value)) -> String {
        let cells = GRID * GRID;
        let mut rec = serde_json::json!({
            "model": "sd2-community/stable-diffusion-2-1",
            "revision": "bb2154823665391b4fb29b0b9cf82a198964ee05",
            "backbone": "dift-sd21",
            "timestep": 261,
            "ensemble": 8,
            "grid_w": GRID,
            "grid_h": GRID,
            "input_size": 768,
            "map_x": vec![1.0; cells],
            "map_y": vec![2.0; cells],
            "confidence": vec![0.5; cells],
            "sim": vec![0.9; cells],
        });
        edit(&mut rec);
        rec.to_string()
    }

    /// The healthy record parses, and the summary the diagnostic prints is
    /// computed from the confidences actually parsed.
    #[test]
    fn a_healthy_field_parses_and_summarises() {
        let f = parse_field(&record(|_| {})).unwrap();
        assert_eq!((f.grid_w, f.grid_h, f.input_size), (GRID, GRID, 768));
        assert_eq!(f.map_x.len(), GRID * GRID);
        assert!((f.coverage(0.5) - 1.0).abs() < 1e-6);
        assert_eq!(f.coverage(0.6), 0.0);
        assert_eq!(f.revision.len(), 40, "the field records the exact checkpoint");
    }

    /// MUTATION: drop the length gate and a truncated `map_x` parses — 7b
    /// would then index cells that do not exist.
    #[test]
    fn a_field_with_a_truncated_array_is_refused() {
        let e = parse_field(&record(|r| {
            r["map_x"] = serde_json::json!(vec![1.0; GRID]);
        }))
        .unwrap_err()
        .to_string();
        assert!(e.contains("48 entries"), "{e}");
    }

    /// MUTATION: drop the coordinate gate and `map_x = 48.0` parses — an
    /// out-of-bounds raster index waiting in a saved-recipe path. The grid
    /// gate itself is exercised too: a 24×24 answer (a different stride, i.e.
    /// a different checkpoint) is refused by SIZE, not misread cell-by-cell.
    #[test]
    fn an_out_of_grid_coordinate_or_grid_is_refused() {
        let e = parse_field(&record(|r| {
            r["map_x"][7] = serde_json::json!(GRID as f32);
        }))
        .unwrap_err()
        .to_string();
        assert!(e.contains("outside the 48x48 target grid"), "{e}");
        let e = parse_field(&record(|r| {
            r["map_y"][0] = serde_json::json!(-0.5);
        }))
        .unwrap_err()
        .to_string();
        assert!(e.contains("outside the 48x48 target grid"), "{e}");
        let e = parse_field(&record(|r| {
            r["grid_w"] = serde_json::json!(24);
        }))
        .unwrap_err()
        .to_string();
        assert!(e.contains("24x48 grid"), "{e}");
    }

    /// MUTATION: drop the [0, 1] gate and a confidence of 40 parses — every
    /// downstream weighting would silently hand one cell the whole field.
    /// Same shape for a cosine no unit vectors can produce.
    #[test]
    fn an_out_of_range_confidence_or_cosine_is_refused() {
        let e = parse_field(&record(|r| {
            r["confidence"][3] = serde_json::json!(1.5);
        }))
        .unwrap_err()
        .to_string();
        assert!(e.contains("outside [0, 1]"), "{e}");
        let e = parse_field(&record(|r| {
            r["sim"][3] = serde_json::json!(2.0);
        }))
        .unwrap_err()
        .to_string();
        assert!(e.contains("no cosine can reach"), "{e}");
    }

    /// MUTATION: drop the `is_finite` guard and a NaN coordinate parses —
    /// NaN fails every comparison, so `!(0.0..hi).contains` is the only gate
    /// shape that catches it, and this pins that shape.
    #[test]
    fn a_non_finite_element_is_refused() {
        for key in ["map_x", "confidence"] {
            let e = parse_field(&record(|r| {
                r[key][0] = serde_json::json!(f64::NAN);
            }))
            .unwrap_err()
            .to_string();
            // serde_json writes NaN as `null`, which is "not a number" here —
            // either sentence is the refusal doing its job.
            assert!(e.contains("non-finite") || e.contains("non-number"), "{key}: {e}");
        }
    }

    /// The per-pair failure shape: a record carrying `error` is a pair the
    /// sidecar declined, not a malformed file, and it must say so in the
    /// sentence the caller prints.
    #[test]
    fn a_declined_record_reports_the_sidecars_own_reason() {
        let e = parse_field(r#"{"error":"cannot read image"}"#).unwrap_err().to_string();
        assert!(e.contains("cannot read image"), "{e}");
    }

    /// The argv contract the sidecar reads: `-E` first (import-hijack
    /// hardening), one `--source`, one `--target`, one `--output`, and NO
    /// precision or device flag — the sidecar owns those choices.
    /// (Single-flight is pinned by `embed.rs`'s observation test, which since
    /// step 7a observes the process-wide `crate::with_model_slot` this bridge
    /// runs under.)
    #[test]
    fn the_sidecar_argv_names_both_frames_and_the_output() {
        let args = sidecar_args(
            Path::new("correspond.py"),
            Path::new("a.png"),
            Path::new("b.png"),
            Path::new("out.json"),
        )
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
        assert_eq!(args.first().map(String::as_str), Some("-E"));
        for flag in ["--source", "--target", "--output"] {
            assert_eq!(args.iter().filter(|a| *a == flag).count(), 1, "{args:?}");
        }
        assert!(!args.iter().any(|a| a.starts_with("--fp")), "{args:?}");
        assert!(!args.iter().any(|a| a == "--cpu"), "{args:?}");
    }

    /// A stand-in interpreter over `crate::write_stand_in`: ignores
    /// `-E <script>` and either copies a pre-written fixture to the
    /// `--output` argument (argv position 8 for THIS bridge's argv) or does
    /// nothing, then exits 0.
    fn stub_opts(dir: &Path, body: Option<&str>) -> CorrespondOpts {
        let python_bin = match body {
            Some(text) => {
                // `copy` from a fixture, not `echo`: a 2304-cell record does
                // not fit a batch line.
                std::fs::write(dir.join("field.json"), text).unwrap();
                crate::write_stand_in(
                    dir,
                    "writing",
                    "@copy /y \"%~dp0field.json\" \"%~8\" >nul\r\n@exit /b 0\r\n",
                    &format!("cp \"{}/field.json\" \"$8\"\nexit 0\n", dir.display()),
                )
            }
            None => crate::write_stand_in(dir, "noop", "@exit /b 0\r\n", "exit 0\n"),
        };
        // The script must merely exist — the bridge refuses a missing one
        // before it ever spawns.
        let script = dir.join("correspond.py");
        std::fs::write(&script, "# stand-in\n").unwrap();
        CorrespondOpts { python_bin, script }
    }

    /// One loopback run against a stand-in with the given fixture `body`;
    /// hands back the output path and the bridge's verdict. The frames need
    /// not exist — the stand-in never reads them, and the bridge does not
    /// preflight them (the CALLER stages pixels; see `main.rs`).
    fn run_stub(tag: &str, body: Option<&str>) -> (std::path::PathBuf, Result<CorrespondenceField>) {
        let dir = crate::test_dir(tag);
        let opts = stub_opts(&dir, body);
        let out = dir.join("field-out.json");
        let verdict = correspond_file(&opts, Path::new("a.png"), Path::new("b.png"), &out);
        (out, verdict)
    }

    /// Positive control through the SHARED executor (`crate::run_model_sidecar`
    /// gained its second caller in this batch): a sidecar that writes a valid
    /// field succeeds end-to-end, and the deliverable STAYS on disk — the
    /// diagnostic's contract, unlike the embedding's deleted scratch.
    #[test]
    fn a_sidecar_that_writes_a_valid_field_succeeds_and_keeps_the_artifact() {
        let (out, verdict) = run_stub("corr-writes", Some(&record(|_| {})));
        assert_eq!(verdict.unwrap().grid_w, GRID);
        assert!(out.is_file(), "the field file is the deliverable and must remain");
        let _ = std::fs::remove_dir_all(out.parent().unwrap());
    }

    /// MUTATION: drop the `sidecar_wrote` call from `crate::run_model_sidecar`
    /// and a sidecar that exits 0 WITHOUT writing is adopted — the exact
    /// failure M-D1 pinned for denoise's own copy of the sequence, re-pinned
    /// here against the extracted shared executor.
    #[test]
    fn a_sidecar_that_exits_zero_without_writing_is_refused() {
        let (out, verdict) = run_stub("corr-noop", None);
        let e = verdict.unwrap_err().to_string();
        assert!(e.contains("exited 0 but wrote no output"), "{e}");
        let _ = std::fs::remove_dir_all(out.parent().unwrap());
    }

    /// MUTATION: drop the parse-failure discard in `correspond_file` and a
    /// file that failed its own gates survives at the output name, looking
    /// like a result to anything that only checks existence.
    #[test]
    fn an_unparseable_field_is_refused_and_not_left_behind() {
        let (out, verdict) = run_stub("corr-garbage", Some("not-json"));
        let e = verdict.unwrap_err().to_string();
        assert!(e.contains("unusable field"), "{e}");
        assert!(!out.exists(), "an unusable field must not remain at the output name");
        let _ = std::fs::remove_dir_all(out.parent().unwrap());
    }

    // --- SIDECAR SOURCE CONTRACTS (correspond.py-specific) -------------------
    //
    // The shared four (digest+byte pins, full-hash revisions, no unpinned
    // upstream code, durable publish) live in `embed.rs`'s family tests,
    // where correspond.py is enrolled. These pin what is specific to THIS
    // sidecar.

    const CORRESPOND_SRC: &str =
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/python/correspond.py"));

    /// The pinned checkpoint is the one whose provenance was established
    /// (official repo delisted; mirror cross-verified byte-identical against
    /// an independent uploader), every model load refuses to resolve a remote
    /// name, and the extraction is deterministic by construction.
    ///
    /// MUTATIONS THIS CATCHES: moving the revision pin without this test
    /// moving with it, dropping `local_files_only` from any of the four
    /// loads, adding a fifth `from_pretrained` (a fifth chance to resolve a
    /// remote name), and deleting any determinism knob.
    #[test]
    fn the_correspondence_sidecar_is_pinned_and_loads_only_local_files() {
        assert!(
            CORRESPOND_SRC.contains("\"revision\": \"bb2154823665391b4fb29b0b9cf82a198964ee05\""),
            "the SD 2.1 revision pin moved without this test moving with it"
        );
        // The provenance paragraph must survive edits — it is the record of
        // WHY a community mirror is acceptable here at all.
        for evidence in ["delisted", "CreativeML Open RAIL++-M", "SfinOe"] {
            assert!(
                CORRESPOND_SRC.contains(evidence),
                "the provenance/licence record must stay in the file: {evidence}"
            );
        }
        let code_lines =
            || CORRESPOND_SRC.lines().map(str::trim).filter(|l| !l.starts_with('#'));
        assert_eq!(
            code_lines().filter(|l| l.contains("local_files_only=True")).count(),
            4,
            "unet, vae, text encoder and tokenizer must all refuse to resolve a remote name"
        );
        assert_eq!(
            code_lines().filter(|l| l.contains("from_pretrained(")).count(),
            4,
            "a fifth from_pretrained would be a fifth chance to resolve a remote name"
        );
        for knob in [
            "torch.backends.cudnn.benchmark = False",
            "torch.backends.cudnn.deterministic = True",
            "torch.backends.cuda.matmul.allow_tf32 = False",
            "torch.backends.cudnn.allow_tf32 = False",
            "manual_seed(",
        ] {
            assert!(CORRESPOND_SRC.contains(knob), "determinism knob missing: {knob}");
        }
    }

    /// The DIFT recipe is the paper's, and the grid this build's parser pins
    /// is the grid the sidecar produces — the two constants must not drift
    /// apart.
    #[test]
    fn the_dift_recipe_matches_the_parsers_grid() {
        for pin in ["TIMESTEP = 261", "INPUT_SIZE = 768", "UP_BLOCK = 1", "FEATURE_DIM = 1280"] {
            assert!(CORRESPOND_SRC.contains(pin), "the DIFT recipe moved: {pin}");
        }
        assert!(
            CORRESPOND_SRC.contains(&format!("GRID = {GRID}")),
            "the sidecar's grid must be the {GRID} this parser refuses to deviate from"
        );
    }

    /// THE PERMUTATION KILLER, pinned at the source: confidence is cyclic
    /// consistency × local flow smoothness, and raw cosine similarity is NOT
    /// a factor. A pixel-shuffle of the same frame (exactly the fit's
    /// atmosphere-budget fixtures) produces individually strong but spatially
    /// incoherent matches — smoothness is what keeps such a pair honestly
    /// unmatchable, so a "simplification" that dropped it would flip those
    /// fixtures from atmosphere to full-fit in 7b.
    ///
    /// MUTATIONS THIS CATCHES: multiplying `sim` into the confidence,
    /// dropping either term, or trading the median for a mean (one wild
    /// neighbour must not drag the reference flow).
    #[test]
    fn the_confidence_is_cyclic_times_smoothness_and_never_raw_similarity() {
        assert!(
            CORRESPOND_SRC.contains("conf = conf_cyc * conf_smooth"),
            "the confidence must be exactly the two scale-free terms"
        );
        assert!(
            !CORRESPOND_SRC.contains("conf_cyc * conf_smooth * sim")
                && !CORRESPOND_SRC.contains("sim * conf_cyc"),
            "raw similarity must not enter the confidence — it is scale-dependent"
        );
        assert!(
            CORRESPOND_SRC.contains("np.median(nx") && CORRESPOND_SRC.contains("np.median(ny"),
            "the neighbourhood reference must be the MEDIAN flow"
        );
        // And the raw cosine stays visible for diagnostics — dropping the
        // `sim` array would blind the CLI door without failing the parser.
        assert!(CORRESPOND_SRC.contains("\"sim\":"), "the diagnostic cosine must stay exported");
    }
}
