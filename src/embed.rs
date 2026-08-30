//! Style-embedding bridge — Rust side of the sidecar (`python/embed.py`).
//!
//! Same shell-out pattern as [`crate::denoise`] and [`crate::segment`]: a local
//! Python process runs SigLIP 2 over one image (or a manifest of them) and
//! writes a JSON record holding a 768-dim L2-normalised vector, which
//! [`crate::style`] stores beside each exemplar's 14-dim hand feature. The
//! weights auto-download to `python/weights` on first run (~1.50 GB, digest
//! pinned) — nothing is stored in this repo.
//!
//! **Budgeted, since R28 Batch-4.** The model is the largest thing this tree
//! loads and it does not live in host RAM, so neither of the two budgets that
//! bound everything else (`decode::MAX_CONCURRENT_DECODES`, `jobs`' free-memory
//! division) ever saw it. Two rules close that: the sidecar is asked for
//! [`fp16_wanted`] half precision, and calls are SINGLE-FLIGHTED
//! ([`crate::with_model_slot`]) so at most one model is resident whatever the caller's
//! concurrency. Together, 4 concurrent × 1.50 GB becomes 1 × 0.75 GB.
//!
//! **The embedding is additive, in both directions.** An index built without
//! this sidecar loads and retrieves exactly as it did before (the field is
//! `None` and the cosine block contributes nothing), and a build whose sidecar
//! fails keeps going with the 14-dim feature alone. That is deliberate: a
//! 1.5 GB download must never be able to turn a working Style panel off.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config::Config;

/// The vector width this build expects. Not a preference — [`parse_vector`]
/// refuses any other width, because a checkpoint that answered 1152 dims (the
/// so400m tier) would silently produce an index whose exemplars are not
/// comparable to each other.
pub const EMBED_DIM: usize = 768;
pub const MODEL_REPO: &str = "google/siglip2-base-patch16-384";
pub const MODEL_REVISION: &str = "f775b65a79762255128c981547af89addcfe0f88";
/// The ONE tokenizer class `python/embed.py` instantiates, and the class the
/// pinned `tokenizer_config.json` names.
///
/// Spelled on this side because it goes into the stored index provenance
/// ([`crate::style::embed_provenance_string`]): the checkpoint alone did not
/// distinguish two indices whose text vectors came from different doors, and
/// this batch's own root cause was exactly two doors on one checkpoint
/// answering vectors at cosine 0.72-0.78 of each other.
/// `the_embedding_sidecar_has_exactly_one_tokenizer_door` pins it against the
/// sidecar's own constant.
pub const TEXT_TOKENIZER_CLASS: &str = "GemmaTokenizer";

/// Everything one embedding run needs; built from [`Config`] like
/// [`crate::segment::SegmentOpts`].
pub struct EmbedOpts {
    pub python_bin: String,
    pub script: PathBuf,
    pub text_file: Option<PathBuf>,
    pub vocab_file: Option<PathBuf>,
}

impl EmbedOpts {
    pub fn from_config(cfg: &Config) -> Self {
        EmbedOpts {
            python_bin: cfg.python_bin.clone(),
            script: PathBuf::from(&cfg.embed_script),
            text_file: None,
            vocab_file: None,
        }
    }

    /// Is the sidecar even present? The style index build asks BEFORE the
    /// worker pool starts, so a build without the script says so once instead
    /// of failing 150 times.
    pub fn available(&self) -> bool {
        !self.script.as_os_str().is_empty() && self.script.exists()
    }
}

/// Load the model in HALF precision unless the user asked otherwise.
///
/// ON by default (R28 Batch-4 4b, adjudication F3): `python/embed.py` has
/// implemented `--fp16` since R27 Batch-5 and nothing ever passed it, so every
/// sidecar call has been loading 375.5 M parameters at 4 B each — 1.50 GB per
/// process — when 0.75 GB would do. It is a no-op on CPU by the sidecar's own
/// construction (`embed.py`: `if fp16 and device.startswith("cuda")`), so a
/// machine without CUDA is unaffected in every direction.
///
/// **Why the consumers tolerate it.** The sidecar casts the pooled output back
/// to fp32 and normalises THERE (`v = v.float()` before the norm), so the
/// record it writes is still float32 text declaring `"norm":"l2"` — no
/// invariant `parse_vector` checks moves, including the ±1e-3 norm gate. The
/// only consumer of the elements themselves is `style::embed_distance`, which
/// is a ranking: a dot product of two unit vectors, clamped to [-1, 1] and
/// multiplied by the configured embedding weight against a 14-dim block.
/// fp16 carries ~4.9e-4 of relative precision per element
/// (10 explicit mantissa bits), and the quantity built from them is an order
/// COMPARISON, not a measurement.
///
/// **Registered as unverified**: the resulting cosine drift is argued, not
/// measured — this batch ran no GPU sidecar. The escape hatch is therefore
/// real rather than decorative: `AUTOSHOP_EMBED_FP32` forces the old load, so
/// an index built before this change can be queried in exactly the arithmetic
/// it was built with.
fn fp16_wanted() -> bool {
    !std::env::var("AUTOSHOP_EMBED_FP32")
        .map(|v| !matches!(v.trim(), "" | "0" | "false" | "off"))
        .unwrap_or(false)
}

/// The BATCH TEXT door's argv — N strings in one process, no image.
///
/// It exists because the two index builders each need one text vector per
/// record, and the alternative was one sidecar PROCESS per record: the look
/// build used to re-invoke `embed.py` — a 1.5 GB model load — once per
/// photograph purely to embed that photograph's own tag string, and the RAW
/// build did not compute the vector at all. One manifest, one load, N vectors,
/// in order.
fn text_sidecar_args(
    script: &Path,
    manifest: &Path,
    output: &Path,
    fp16: bool,
) -> Vec<std::ffi::OsString> {
    let mut v: Vec<std::ffi::OsString> = vec![
        "-E".into(),
        script.into(),
        "--text-manifest".into(),
        manifest.into(),
        "--output".into(),
        output.into(),
    ];
    if fp16 {
        v.push("--fp16".into());
    }
    v
}

/// Run the sidecar over a JSONL text manifest and return the vectors IN ORDER.
///
/// `expect` is the number of lines the caller wrote: a short or long answer is
/// refused rather than zipped, because a text vector that landed on the wrong
/// record is a silently wrong ranking and nothing else.
pub fn embed_text_batch(
    opts: &EmbedOpts,
    manifest: &Path,
    scratch: &Path,
    expect: usize,
) -> Result<Vec<Vec<f32>>> {
    if !opts.script.exists() {
        bail!(
            "style-embedding sidecar not found at {} — run from the project dir or set \
             AUTOSHOP_EMBED_SCRIPT.",
            opts.script.display()
        );
    }
    let text = crate::run_model_sidecar(
        "style-embedding sidecar",
        &opts.python_bin,
        text_sidecar_args(&opts.script, manifest, scratch, fp16_wanted()),
        scratch,
    )?;
    let out = parse_text_vectors(&text, expect).with_context(|| {
        format!("style-embedding sidecar wrote an unusable text batch at {}", scratch.display())
    })?;
    let _ = std::fs::remove_file(scratch);
    Ok(out)
}

/// One `{"text_vectors": [...]}` record → the vectors, each through the same
/// width / finiteness / unit-norm gate a single vector goes through.
pub fn parse_text_vectors(text: &str, expect: usize) -> Result<Vec<Vec<f32>>> {
    let rec: serde_json::Value =
        serde_json::from_str(text.trim()).context("style-embedding text output is not JSON")?;
    if let Some(e) = rec.get("error").and_then(|v| v.as_str()) {
        bail!("style-embedding sidecar declined this text batch: {e}");
    }
    let rows = rec
        .get("text_vectors")
        .and_then(|v| v.as_array())
        .context("style-embedding text output has no `text_vectors` array")?;
    if rows.len() != expect {
        bail!(
            "style-embedding sidecar answered {} text vectors for {expect} texts — a batch that \
             does not line up would attach a vector to the wrong record",
            rows.len()
        );
    }
    rows.iter()
        .map(|row| {
            let arr = row.as_array().context("`text_vectors` holds a non-array")?;
            parse_unit_vector(arr, "text_vectors")
        })
        .collect()
}

/// One JSON array → a checked unit vector of [`EMBED_DIM`] elements. The ONE
/// place the three vector-bearing keys (`vector`, `text_vector`,
/// `text_vectors`) agree about width, finiteness and norm.
fn parse_unit_vector(arr: &[serde_json::Value], key: &str) -> Result<Vec<f32>> {
    if arr.len() != EMBED_DIM {
        bail!("style-embedding `{key}` has wrong width {}", arr.len());
    }
    let mut out = Vec::with_capacity(arr.len());
    for x in arr {
        let f = x.as_f64().context("style-embedding vector holds a non-number")? as f32;
        if !f.is_finite() {
            bail!("style-embedding `{key}` holds a non-finite element");
        }
        out.push(f);
    }
    let n = out.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>().sqrt();
    if (n - 1.0).abs() > 1e-3 {
        bail!("style-embedding `{key}` is not L2-normalised (norm {n:.6})");
    }
    Ok(out)
}

/// One line of `embed.py --manifest-jsonl`'s JSONL answer: the frame, and
/// either its record or the sidecar's own reason for declining it.
#[derive(Debug, Clone)]
pub struct EmbedBatchRecord {
    pub path: String,
    pub record: Option<EmbedRecord>,
    pub error: Option<String>,
}

/// The BATCH IMAGE door's argv — N frames in one process, one model load.
///
/// It exists for the same reason the batch TEXT door does, one step later:
/// until S2 `StyleIndex::build` called this sidecar once PER PHOTOGRAPH on the
/// image side too, so a 169-photo library re-loaded 1.50 GB of weights 169
/// times — 5,618 s measured on the photographer's own corpus. `embed.py` has
/// implemented `--manifest-jsonl` since R27 Batch-5; nothing was wired to it.
fn image_batch_args(
    script: &Path,
    manifest: &Path,
    output: &Path,
    fp16: bool,
    vocab_file: Option<&Path>,
) -> Vec<std::ffi::OsString> {
    let mut v: Vec<std::ffi::OsString> = vec![
        // `-E`: the second layer against a PYTHON* import hijack (the env
        // allowlist in `dotenv_child_env` is the first).
        "-E".into(),
        script.into(),
        "--manifest-jsonl".into(),
        manifest.into(),
        "--output".into(),
        output.into(),
    ];
    if fp16 {
        v.push("--fp16".into());
    }
    if let Some(p) = vocab_file {
        v.extend(["--vocab-file".into(), p.into()]);
    }
    v
}

/// Run the sidecar over a JSONL manifest of IMAGE paths and return one record
/// per line, in the sidecar's own order.
///
/// The caller maps them back by PATH, never by position: `embed.py` reports a
/// malformed manifest line as soon as it reads it and the rest in batch order,
/// so a positional zip would attach one photograph's vector to another's
/// exemplar — the failure [`parse_text_vectors`] refuses a short batch to
/// avoid.
pub fn embed_image_batch(
    opts: &EmbedOpts,
    manifest: &Path,
    scratch: &Path,
) -> Result<Vec<EmbedBatchRecord>> {
    if !opts.script.exists() {
        bail!(
            "style-embedding sidecar not found at {} — run from the project dir or set \
             AUTOSHOP_EMBED_SCRIPT.",
            opts.script.display()
        );
    }
    let text = crate::run_model_sidecar(
        "style-embedding sidecar",
        &opts.python_bin,
        image_batch_args(
            &opts.script,
            manifest,
            scratch,
            fp16_wanted(),
            opts.vocab_file.as_deref(),
        ),
        scratch,
    )?;
    let out = parse_batch_records(&text).with_context(|| {
        format!("style-embedding sidecar wrote an unusable image batch at {}", scratch.display())
    })?;
    let _ = std::fs::remove_file(scratch);
    Ok(out)
}

/// The sidecar's JSONL → records, each vector through the same width /
/// finiteness / unit-norm gate a single record goes through.
///
/// A line that is not JSON fails the WHOLE batch: every line the sidecar
/// writes it composed itself, and text it did not compose means the file is
/// not the answer it claims to be. A line carrying `error` is one
/// photograph's failure and is kept as one — the fail-soft contract.
pub fn parse_batch_records(text: &str) -> Result<Vec<EmbedBatchRecord>> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line.trim())
            .with_context(|| format!("style-embedding batch line {} is not JSON", i + 1))?;
        let path = value
            .get("path")
            .and_then(|v| v.as_str())
            .with_context(|| format!("style-embedding batch line {} names no path", i + 1))?
            .to_string();
        if let Some(e) = value.get("error").and_then(|v| v.as_str()) {
            out.push(EmbedBatchRecord { path, record: None, error: Some(e.to_string()) });
            continue;
        }
        match parse_record(line.trim()) {
            Ok(record) => out.push(EmbedBatchRecord { path, record: Some(record), error: None }),
            // A record that fails the DOOR (wrong width, unnormalised, a
            // non-finite element) is this photograph's failure, not the
            // batch's: the index is allowed to be a mixed one, and refusing
            // 168 good vectors over one bad line would be the opposite of the
            // contract the per-line `error` arm keeps.
            Err(e) => out.push(EmbedBatchRecord {
                path,
                record: None,
                error: Some(format!("{e:#}")),
            }),
        }
    }
    Ok(out)
}

/// The sidecar's argv, as a pure function of the four things that decide it —
/// so the flags this build passes can be pinned by a test instead of being
/// visible only to a running Python.
fn sidecar_args(script: &Path, input: &Path, output: &Path, fp16: bool, text_file: Option<&Path>, vocab_file: Option<&Path>) -> Vec<std::ffi::OsString> {
    let mut v: Vec<std::ffi::OsString> = vec![
        // `-E`: the second layer against a PYTHON* import hijack (the env
        // allowlist in `dotenv_child_env` is the first).
        "-E".into(),
        script.into(),
        "--input".into(),
        input.into(),
        "--output".into(),
        output.into(),
    ];
    if fp16 {
        v.push("--fp16".into());
    }
    if let Some(p) = text_file { v.extend(["--text-file".into(), p.into()]); }
    if let Some(p) = vocab_file { v.extend(["--vocab-file".into(), p.into()]); }
    v
}

/// Run the sidecar on ONE image and return its L2-normalised vector.
///
/// `scratch` is the JSON file the sidecar writes; the caller owns its lifetime
/// (the style build hands over a per-photo temp name so parallel workers never
/// share one). The run itself is [`crate::run_model_sidecar`] — the shared
/// spawn/bound/exit-0-is-not-success executor, extracted when the
/// correspondence bridge (step 7a) would have been the fourth copy of it.
///
/// SERIALISED against every other model sidecar in this process — see
/// [`crate::with_model_slot`]. The fan-out that gate closes here
/// (adjudication F3): `StyleIndex::build` runs up to
/// `decode::MAX_CONCURRENT_DECODES` = 4 workers and each one used to spawn
/// its own sidecar, so four SigLIP loads could be live at once — 6.0 GB of
/// fp32 weights, 3.0 GB even with `--fp16`, against a consumer GPU that
/// commonly has 4. Nothing else in the tree serialised them, because every
/// other budget is shaped like host RAM (`MAX_CONCURRENT_DECODES` counts
/// 181 MB decodes, `jobs` divides `GlobalMemoryStatusEx`) and the model does
/// not live in host RAM at all.
///
/// A GATE, not a batcher, chosen over wiring `embed.py --manifest`: the
/// manifest path helps only the index build, while the gate covers the
/// develop-time query too (`pipeline::produce_recipe` under `batch --jobs 3`
/// is three concurrent single-image calls that no manifest can merge), and it
/// needs no new record format, no per-line failure mapping and no second
/// staging lifetime. The cost is honest and bounded: the embedding arm of a
/// build runs serially, while the decode that dominates it still runs
/// four-wide. The staging the caller did (writing the PNG) stays OUTSIDE the
/// gate: it is disk work, it costs no model, and holding the slot across it
/// would serialise the cheap half too.
pub fn embed_file(opts: &EmbedOpts, input: &Path, scratch: &Path) -> Result<Vec<f32>> {
    Ok(embed_file_record(opts, input, scratch)?.vector)
}

#[derive(Debug, Clone, Default)]
pub struct EmbedRecord {
    pub vector: Vec<f32>,
    pub text_vector: Option<Vec<f32>>,
    pub vocab_scores: Option<Vec<f32>>,
}

pub fn embed_file_record(opts: &EmbedOpts, input: &Path, scratch: &Path) -> Result<EmbedRecord> {
    if !opts.script.exists() {
        bail!(
            "style-embedding sidecar not found at {} — run from the project dir or set \
             AUTOSHOP_EMBED_SCRIPT.",
            opts.script.display()
        );
    }
    let text = crate::run_model_sidecar(
        "style-embedding sidecar",
        &opts.python_bin,
        sidecar_args(&opts.script, input, scratch, fp16_wanted(), opts.text_file.as_deref(), opts.vocab_file.as_deref()),
        scratch,
    )?;
    let v = parse_record(&text).with_context(|| {
        format!("style-embedding sidecar wrote an unusable record at {}", scratch.display())
    })?;
    // The scratch file is an INTERMEDIATE, not an artifact: the vector is what
    // the caller keeps. Leaving it behind would litter the develop store with
    // 10 KB JSON files nothing ever reads again.
    let _ = std::fs::remove_file(scratch);
    Ok(v)
}

/// One sidecar JSON record → the vector, with every invariant the index's own
/// door check would otherwise have to re-derive.
///
/// Refuses, rather than repairs, four things: a record that carries an
/// `error`, a width other than [`EMBED_DIM`], a non-finite element, and a
/// vector whose L2 norm is not ~1. The last one is the load-bearing check —
/// the retrieval treats these as unit vectors and computes a cosine as a plain
/// dot product, so an unnormalised vector would not be *wrong by a little*, it
/// would silently outweigh every other exemplar.
pub fn parse_vector(text: &str) -> Result<Vec<f32>> {
    Ok(parse_record(text)?.vector)
}

pub fn parse_record(text: &str) -> Result<EmbedRecord> {
    let rec: serde_json::Value = serde_json::from_str(text.trim())
        .context("style-embedding output is not JSON")?;
    if let Some(e) = rec.get("error").and_then(|v| v.as_str()) {
        bail!("style-embedding sidecar declined this image: {e}");
    }
    let arr = rec
        .get("vector")
        .and_then(|v| v.as_array())
        .context("style-embedding output has no `vector` array")?;
    if arr.len() != EMBED_DIM {
        bail!(
            "style-embedding sidecar returned a {}-dim vector; this build indexes {EMBED_DIM}-dim \
             vectors and mixing widths in one index makes its exemplars incomparable",
            arr.len()
        );
    }
    let mut v = Vec::with_capacity(EMBED_DIM);
    for x in arr {
        let f = x.as_f64().context("style-embedding vector holds a non-number")? as f32;
        if !f.is_finite() {
            bail!("style-embedding vector holds a non-finite element");
        }
        v.push(f);
    }
    let norm = v.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>().sqrt();
    // 1e-3 is generous against the f32 round-trip of 768 shortest-decimal
    // elements (~1e-7 each) and tight enough that an unnormalised vector — the
    // only failure this is guarding — never passes.
    if (norm - 1.0).abs() > 1e-3 {
        bail!("style-embedding vector is not L2-normalised (norm {norm:.6})");
    }
    let parse_vec = |key: &str| -> Result<Option<Vec<f32>>> {
        let Some(arr) = rec.get(key).and_then(|v| v.as_array()) else { return Ok(None) };
        parse_unit_vector(arr, key).map(Some)
    };
    let vocab_scores = rec.get("vocab_scores").and_then(|v| v.as_array()).map(|arr| {
        if arr.len() != crate::style::LOOK_VOCAB.len() {
            return Err(anyhow::anyhow!("style-embedding `vocab_scores` has wrong width {} (expected {})", arr.len(), crate::style::LOOK_VOCAB.len()));
        }
        arr.iter().map(|x| x.as_f64().map(|v| v as f32).filter(|v| v.is_finite()).context("style-embedding vocab score is not finite")).collect::<Result<Vec<_>>>()
    }).transpose()?;
    Ok(EmbedRecord { vector: v, text_vector: parse_vec("text_vector")?, vocab_scores })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_record(dim: usize) -> String {
        // A genuinely unit vector of the requested width: every element
        // 1/sqrt(dim).
        let e = 1.0f32 / (dim as f32).sqrt();
        let body: Vec<String> = (0..dim).map(|_| e.to_string()).collect();
        format!("{{\"dim\":{dim},\"norm\":\"l2\",\"vector\":[{}]}}", body.join(","))
    }

    /// MUTATION: drop the `arr.len() != EMBED_DIM` refusal and this passes with
    /// a 512-dim vector — an index that mixes widths ranks nothing.
    #[test]
    fn a_vector_of_the_wrong_width_is_refused() {
        assert_eq!(parse_vector(&unit_record(EMBED_DIM)).unwrap().len(), EMBED_DIM);
        let e = parse_vector(&unit_record(512)).unwrap_err().to_string();
        assert!(e.contains("512-dim"), "{e}");
    }

    #[test]
    fn vocab_scores_width_is_the_vocab_len() {
        let vector = unit_record(EMBED_DIM);
        let scores = (0..crate::style::LOOK_VOCAB.len())
            .map(|i| format!("{:.1}", i as f32))
            .collect::<Vec<_>>()
            .join(",");
        let text = vector.trim_end_matches('}').to_owned()
            + &format!(",\"vocab_scores\":[{scores}]}}");
        let record = parse_record(&text).expect("a full vocabulary score vector parses");
        assert_eq!(record.vocab_scores.as_ref().map(Vec::len), Some(crate::style::LOOK_VOCAB.len()));
        let short_scores = (0..crate::style::LOOK_VOCAB.len() - 1)
            .map(|i| format!("{:.1}", i as f32))
            .collect::<Vec<_>>()
            .join(",");
        let short = vector.trim_end_matches('}').to_owned()
            + &format!(",\"vocab_scores\":[{short_scores}]}}");
        let error = parse_record(&short).unwrap_err().to_string();
        assert!(error.contains("wrong width"), "{error}");
    }

    /// MUTATION: drop the norm check and an unnormalised vector is adopted —
    /// the retrieval's cosine is a bare dot product, so a vector of norm 40
    /// would beat every real exemplar regardless of what it depicts.
    #[test]
    fn an_unnormalised_vector_is_refused() {
        let body: Vec<String> = (0..EMBED_DIM).map(|_| "0.5".to_string()).collect();
        let text = format!("{{\"vector\":[{}]}}", body.join(","));
        let e = parse_vector(&text).unwrap_err().to_string();
        assert!(e.contains("not L2-normalised"), "{e}");
    }

    /// MUTATION: drop the `is_finite` guard and a NaN reaches the index, where
    /// serde writes it as `null` and the NEXT load of the whole index fails —
    /// one bad photo costs the user an hour-long rebuild (the exact failure
    /// `style::exemplar_is_finite` exists for).
    #[test]
    fn a_non_finite_element_is_refused() {
        let mut body: Vec<String> = (0..EMBED_DIM).map(|_| "0.0".to_string()).collect();
        body[0] = "1e40".into(); // f64 -> f32 overflows to inf
        let text = format!("{{\"vector\":[{}]}}", body.join(","));
        let e = parse_vector(&text).unwrap_err().to_string();
        assert!(e.contains("non-finite"), "{e}");
    }

    /// The batch contract's per-line failure shape: a record carrying `error`
    /// is a photo the sidecar declined, not a malformed file, and it must say
    /// so in the sentence the build prints.
    #[test]
    fn a_declined_record_reports_the_sidecars_own_reason() {
        let e = parse_vector("{\"path\":\"a.png\",\"error\":\"cannot read image\"}")
            .unwrap_err()
            .to_string();
        assert!(e.contains("cannot read image"), "{e}");
    }

    /// R28 Batch-4 4b: the half-precision flag the sidecar has implemented
    /// since R27 Batch-5 is actually PASSED, and the escape hatch really
    /// escapes.
    ///
    /// MUTATION THIS KILLS: dropping the `--fp16` push (the state this batch
    /// found the tree in — every call loading 1.50 GB of fp32 weights when
    /// 0.75 GB would do), or wiring the env var the wrong way round so
    /// `AUTOSHOP_EMBED_FP32` turned fp16 ON.
    #[test]
    fn the_sidecar_argv_carries_the_half_precision_flag() {
        let args = |fp16| {
            sidecar_args(Path::new("embed.py"), Path::new("in.png"), Path::new("out.json"), fp16, None, None)
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        let on = args(true);
        assert!(on.contains(&"--fp16".to_string()), "{on:?}");
        // The contract the sidecar reads: one `--input`, one `--output`, and
        // `-E` first (import-hijack hardening, not decoration).
        assert_eq!(on.first().map(String::as_str), Some("-E"));
        assert_eq!(on.iter().filter(|a| *a == "--input").count(), 1);
        assert_eq!(on.iter().filter(|a| *a == "--output").count(), 1);
        // …and never `--manifest`: this door is the single-image one, and a
        // sidecar handed both refuses (`give exactly one of --input or
        // --manifest`).
        assert!(!on.iter().any(|a| a == "--manifest"), "{on:?}");
        assert!(!args(false).contains(&"--fp16".to_string()));
    }

    /// SINGLE-FLIGHT, pinned by observation rather than by reading the code:
    /// however many threads call it, the gate never lets two bodies overlap.
    ///
    /// MUTATION THIS KILLS: delete the lock in `crate::with_model_slot`
    /// (make it call `body()` directly) and the four threads below all enter
    /// together — the observed maximum becomes 4 and this fails. That is
    /// exactly the state F3 measured: four workers, four resident SigLIP
    /// models, 6.0 GB. Since step 7a the slot is PROCESS-WIDE in `lib.rs`
    /// (the correspondence bridge shares it), so this observation now pins
    /// the crate-level gate.
    #[test]
    fn the_model_slot_admits_one_caller_at_a_time() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let live = AtomicUsize::new(0);
        let most = AtomicUsize::new(0);
        let entered = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..4 {
                let (live, most, entered) = (&live, &most, &entered);
                s.spawn(move || {
                    crate::with_model_slot(|| {
                        let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                        most.fetch_max(now, Ordering::SeqCst);
                        entered.fetch_add(1, Ordering::SeqCst);
                        // Long enough that an ungated version really would
                        // overlap: thread spawn is microseconds, this is
                        // milliseconds.
                        std::thread::sleep(std::time::Duration::from_millis(30));
                        live.fetch_sub(1, Ordering::SeqCst);
                    });
                });
            }
        });
        assert_eq!(entered.load(Ordering::SeqCst), 4, "every caller must get through");
        assert_eq!(most.load(Ordering::SeqCst), 1, "two models were resident at once");
        assert_eq!(live.load(Ordering::SeqCst), 0);
    }

    // --- SIDECAR SOURCE CONTRACTS (D1 section 8) -----------------------------
    //
    // The fixes live in Python and this repo has no Python test runner, so the
    // gate is the established source-invariant idiom (`include_str!` plus
    // non-vacuity assertions) that `denoise.rs` already uses for its own
    // sidecar: a regression edit to either script fails a Rust test.

    const EMBED_SRC: &str =
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/python/embed.py"));
    const SEGMENT_SRC: &str =
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/python/segment.py"));
    const CORRESPOND_SRC: &str =
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/python/correspond.py"));
    const DESCRIBE_SRC: &str =
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/python/describe.py"));

    /// The family roster, in one place: a sidecar that fails to enrol here
    /// escapes all four shared contracts below. `describe.py` (S2) is the
    /// fifth member of the family and the fourth on this roster —
    /// `denoise.py` holds the `_fetch_verified` gate itself and is pinned by
    /// its own module's tests.
    const SIDECARS: [(&str, &str); 4] = [
        ("embed.py", EMBED_SRC),
        ("segment.py", SEGMENT_SRC),
        ("correspond.py", CORRESPOND_SRC),
        ("describe.py", DESCRIBE_SRC),
    ];

    /// Every pinned model file carries BOTH a sha256 and a byte count, in both
    /// sidecars. A partial pin leaves a live unpinned download path — a
    /// revision pin fixes WHICH tree is fetched; only a digest proves the
    /// BYTES.
    ///
    /// MUTATION: delete one `"bytes":` line from either `MODEL` / `SAM` table
    /// and this fails.
    #[test]
    fn every_pinned_sidecar_download_has_a_digest_and_a_byte_cap() {
        for (what, src) in SIDECARS {
            let sha = src.matches("\"sha256\":").count();
            let bytes = src.matches("\"bytes\":").count();
            assert!(sha >= 3, "{what}: extractor non-vacuity — found {sha} digests");
            assert_eq!(sha, bytes, "{what}: {sha} digests but {bytes} byte caps");
            // No placeholders left behind from the design document.
            assert!(
                !src.contains("TAKE AT IMPLEMENTATION TIME") && !src.contains("\"sha256\": None"),
                "{what}: a digest placeholder survived into the shipped source"
            );
        }
    }

    /// Every HF `revision` is a FULL 40-hex commit hash. The Hub documents that
    /// a 7-character hash is not accepted, and a BRANCH name resolves to
    /// whatever upstream has at download time — which is the whole thing the
    /// pinning exists to stop.
    ///
    /// R29 C3/C4 extracted a THIRD spelling here, `SKY_CLASS_INFO_REVISION` —
    /// the class table's own dataset repo. R29 收口 deleted that repo rather
    /// than pinning it (ruling 11): the table is ours and ships in `python/`,
    /// so there is no revision left to hold to this standard, and its gate is
    /// the digest in `the_sky_class_table_is_ours_and_matches_its_pin`.
    #[test]
    fn every_hugging_face_revision_pin_is_a_full_commit_hash() {
        let mut found = 0;
        for (what, src) in SIDECARS {
            for line in src.lines() {
                let l = line.trim();
                let Some(rest) = l
                    .strip_prefix("\"revision\": \"")
                    .or_else(|| l.strip_prefix("SKY_REVISION = \""))
                else {
                    continue;
                };
                let hex: String = rest.chars().take_while(|c| *c != '"').collect();
                assert_eq!(hex.len(), 40, "{what}: revision {hex:?} is not a full commit hash");
                assert!(
                    hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                    "{what}: revision {hex:?} is not lowercase hex"
                );
                found += 1;
            }
        }
        assert!(found >= 3, "extractor non-vacuity: found {found} revision pins");
    }

    /// `trust_remote_code` downloads and EXECUTES upstream Python through HF's
    /// own cache, which our digest gate never sees — the exact hazard
    /// `denoise.py` was hardened against. It must never appear, and neither
    /// must the unverified HF download helpers.
    ///
    /// MUTATION: add `trust_remote_code=True` to either sidecar and this
    /// fails.
    #[test]
    fn the_sidecars_never_execute_unpinned_upstream_code() {
        for (what, src) in SIDECARS {
            // The CALL, not the word: the docstrings name `trust_remote_code`
            // to explain why it is never used, and a test that banned the
            // token would push that explanation out of the file.
            for banned in ["trust_remote_code=", "hf_hub_download(", "snapshot_download("] {
                assert!(!src.contains(banned), "{what} must not use {banned}");
            }
        }
        // And the fetch itself goes through the ONE verified downloader.
        for (what, src) in SIDECARS {
            assert!(src.contains("_fetch_verified("), "{what} must fetch through the gate");
        }
    }

    /// ONE tokenizer door, and no padding mask anywhere near the text tower.
    ///
    /// The sidecar used to try the `Auto*` tokenizer factory and fall back to a
    /// hand-written adapter over `tokenizer.json`. On transformers 5.2.0 the
    /// factory always raised (its SigLIP mapping resolves to `None`), so the
    /// fallback always ran — and the fallback returned a padding mask that
    /// `embed_texts` forwarded into the tower. Same ids, different pooled
    /// vectors (cosine 0.73 / 0.76 / 0.73 / 0.78 / 0.72 over five phrases): two
    /// doors are two vector spaces in one index, and a cosine cannot survive
    /// that. The three banned spellings are the three ways the second door
    /// comes back.
    ///
    /// MUTATION: re-add the factory import, the adapter class, or a mask tensor
    /// to `embed.py` and this fails.
    #[test]
    fn the_embedding_sidecar_has_exactly_one_tokenizer_door() {
        for banned in ["AutoTokenizer", "attention_mask", "PinnedFastTokenizer"] {
            assert!(
                !EMBED_SRC.contains(banned),
                "embed.py must not carry `{banned}` — the text tower takes input_ids alone"
            );
        }
        assert_eq!(
            EMBED_SRC.matches("GemmaTokenizerFast.from_pretrained(").count(),
            1,
            "there must be exactly ONE tokenizer construction in embed.py"
        );
        // The mask exclusion lives in the PIN, and the pin is checked at load.
        assert!(
            EMBED_SRC.contains("TEXT_MODEL_INPUT_NAMES = [\"input_ids\"]"),
            "the one input tensor must be a named constant"
        );
        assert!(
            EMBED_SRC.contains("\"model_input_names\": TEXT_MODEL_INPUT_NAMES"),
            "the pinned tokenizer config must be REFUSED when it stops saying input_ids only"
        );
        // …and the tower is called by name, never splatted.
        assert!(
            EMBED_SRC.contains("model.text_model(input_ids=ids)"),
            "the text tower must be fed input_ids by name"
        );
        assert!(
            EMBED_SRC.contains("TEXT_GOLDEN_IDS = {"),
            "the tokenizer's own output must be pinned, not just its config"
        );
        // The class name is stamped into every index's provenance, so the two
        // spellings of it must agree or an index would claim a door it did not
        // use.
        assert!(
            EMBED_SRC.contains(&format!("TEXT_TOKENIZER_CLASS = \"{TEXT_TOKENIZER_CLASS}\"")),
            "the sidecar's tokenizer class must be the one the provenance records"
        );
        // `do_lower_case` is a config value this door does not act on. The
        // sidecar states that by ENCODING both cases and requiring different
        // ids; a constant that merely restated the pinned file could not be
        // wrong and so could not be a check.
        assert!(
            !EMBED_SRC.contains("TEXT_DO_LOWER_CASE"),
            "the lower-case RECEIPT is gone; the behaviour is asserted instead"
        );
        assert!(
            EMBED_SRC.contains("def _case_is_preserved_self_test("),
            "the real behaviour behind do_lower_case must be self-tested"
        );
        // …and the self-test must reach the tower, not stop at the tokenizer.
        assert!(
            EMBED_SRC.contains("def _text_forward_pass_self_test("),
            "--self-test must run one text forward pass, not two file reads"
        );
    }

    /// The four SigLIP tokenizer files are pinned by digest AND byte count,
    /// like every other file this repo lets a sidecar download.
    ///
    /// The image half of this checkpoint was already gated
    /// (`model.safetensors`, `config.json`, `preprocessor_config.json`); the
    /// tokenizer tree arrived with S1 and had no Rust-side invariant at all, so
    /// a re-pin could have moved the text vectors of every index in the field
    /// with nothing in this crate noticing. These are the digests
    /// `python/embed.py` verifies each download against.
    ///
    /// MUTATION: drop any of the four digests (or the `_fetch_verified` call
    /// that uses them) from `embed.py` and this fails.
    #[test]
    fn the_siglip_tokenizer_files_are_digest_gated() {
        for (what, digest) in [
            ("tokenizer.json",
             "cb9140fae3ac5122c972d37adf83e1248471a38147ad76f8215c8872c6fd8322"),
            ("tokenizer.model",
             "61a7b147390c64585d6c3543dd6fc636906c9af3865a5548f27f31aee1d4c8e2"),
            ("tokenizer_config.json",
             "14afe629fe4959b9e0d51e1852b8d9f7ad074f90a1a7125a4fcdd17f06e78fc8"),
            ("special_tokens_map.json",
             "baec30ea10906f16adb8c18af7a34023002c1746542612b8b41c9f09e1351351"),
        ] {
            assert!(
                EMBED_SRC.contains(digest),
                "the pinned sha256 for the SigLIP 2 tokenizer file '{what}' is not the one \
                 S1 verified — a re-pin moves every text vector in every index"
            );
            assert!(
                EMBED_SRC.contains(&format!("\"{what}\"")),
                "'{what}' must stay in the pinned file list"
            );
        }
        // Digests are only a gate if something checks them: the whole pinned
        // set goes through the shared verified-fetch door.
        assert!(
            EMBED_SRC.contains("for name, pin in MODEL[\"files\"].items():")
                && EMBED_SRC.contains("pin[\"sha256\"],"),
            "every pinned file must be fetched through _fetch_verified with its digest"
        );
    }

    /// R1 / R2, pinned so they cannot come back: the NON-COMMERCIAL SegFormer
    /// weights, and rembg's default-model selector (upstream moved its default
    /// to a model that needs a paid agreement for commercial use, so a
    /// `pip install -U rembg` could swap it in with no change to our source).
    #[test]
    fn the_licence_regressions_stay_closed() {
        // The QUOTED model id, not the word: the docstring names
        // `nvidia/segformer-…` in backticks to record WHY it was removed, and
        // a test that banned the token would delete the provenance with it.
        assert!(
            !SEGMENT_SRC.contains("\"nvidia/segformer"),
            "the NC-licensed SegFormer weights must not come back (R27 Batch-4)"
        );
        assert!(
            SEGMENT_SRC.contains("nvidia/segformer"),
            "…and the record of WHY it was removed must stay in the file"
        );
        // rembg is still used for `subject` — the FALLBACK tier since R29 B4 —
        // but ONLY with an explicitly named session: a bare `remove(` would
        // resolve to whatever that install's default is, and upstream's default
        // is now a model that needs a paid agreement for commercial use. A
        // fallback that fires on the machines least likely to be watched is the
        // last place this may be left to chance.
        assert!(
            SEGMENT_SRC.contains("new_session(\"u2net\")"),
            "the rembg session must stay explicitly named"
        );
        for line in SEGMENT_SRC.lines() {
            let l = line.trim();
            if l.starts_with('#') || l.starts_with('*') {
                continue;
            }
            assert!(
                !l.contains("remove(img") || l.contains("session="),
                "an unsessioned rembg call lets upstream pick the model: {l}"
            );
        }
    }

    /// R29 B4 — the subject backend is the PINNED BiRefNet, and the file this
    /// sidecar EXECUTES is pinned by digest like the weights are.
    ///
    /// `birefnet.py` is not data: `segment.py` loads it through `importlib` and
    /// `exec_module` runs it. That makes it the highest-stakes download in the
    /// tree — a revision pin would fix which tree upstream serves, only the
    /// sha256 proves the bytes that reach the interpreter. The general
    /// checkpoint is named explicitly because R27's design document picked
    /// `BiRefNet_HR-matting`, which R29 B4 measured returning an EMPTY alpha on
    /// 4 of 9 of the user's own photographs; a silent drift back to it would
    /// delete masks rather than approximate them.
    ///
    /// MUTATIONS THIS CATCHES: changing any pinned digit of the model or the
    /// code digest, moving the revision, swapping the repo for HR-matting, or
    /// deleting the fallback tier / its disclosure.
    #[test]
    fn the_subject_backend_is_the_pinned_general_birefnet_with_a_named_fallback() {
        // The repo and revision the user ruled for, verbatim.
        assert!(
            SEGMENT_SRC.contains("\"repo\": \"ZhengPeng7/BiRefNet\""),
            "the subject backend must be the GENERAL BiRefNet checkpoint"
        );
        assert!(
            SEGMENT_SRC
                .contains("\"revision\": \"e2bf8e4460fc8fa32bba5ea4d94b3233d367b0e4\""),
            "the BiRefNet revision pin moved without this test moving with it"
        );
        // HR-matting is named in the docstring (the record of WHY it lost) and
        // must never be the thing that is FETCHED — the same shape of
        // assertion the SegFormer removal above uses.
        assert!(
            !SEGMENT_SRC.contains("\"repo\": \"ZhengPeng7/BiRefNet_HR-matting\""),
            "HR-matting returns an empty alpha on 4/9 real photographs (R29 B4)"
        );
        assert!(
            SEGMENT_SRC.contains("BiRefNet_HR-matting"),
            "…and the record of why it lost must stay in the file"
        );
        // The weights AND the executed source, both by digest.
        for (what, digest) in [
            ("model.safetensors",
             "9ab37426bf4de0567af6b5d21b16151357149139362e6e8992021b8ce356a154"),
            ("birefnet.py",
             "208771ae626f653d64128fbf2d6ac9f8e645c5cc5e286258a73ec3322bbfe5ef"),
            ("BiRefNet_config.py",
             "e7b8c2a74f6cea6a59553d517f71d47f2c1d90e670a13416af17c25fe2f3dc52"),
        ] {
            assert!(
                SEGMENT_SRC.contains(digest),
                "the pinned sha256 for {what} is not the one R29 B4 verified"
            );
        }
        // `exec_module` on upstream Python is only acceptable BECAUSE of the
        // digest above; the two must not drift apart.
        assert!(
            SEGMENT_SRC.contains("exec_module(") && SEGMENT_SRC.contains("_fetch_verified("),
            "birefnet.py is executed, so it must arrive through the verified fetch"
        );
        // THE FALLBACK TIER, and the two states kept apart. The ruling keeps
        // U²-Net for machines that cannot run BiRefNet; a degradation that did
        // not announce itself would be the sidecar lying about provenance.
        assert!(
            SEGMENT_SRC.contains("U2NET_LABEL") && SEGMENT_SRC.contains("BIREFNET_LABEL"),
            "the two subject backends must be separately LABELLED"
        );
        assert!(
            SEGMENT_SRC.contains("FALLBACK - BiRefNet did not run"),
            "a run that fell back must say so in the label it prints"
        );
        assert!(
            SEGMENT_SRC.contains("WARNING - the BiRefNet subject backend did not run"),
            "…and must warn on stderr, which segment.rs forwards on the success path"
        );
        // R29 C3/C4 — and stdout is now a PARSED contract, not just prose.
        // `segment::SegmentReport::parse` takes the label from the brackets and
        // the dependency verdict from the second line; the GUI's status, the
        // fallback warning and the alpha cache's re-derivation rule all hang
        // off those two spellings, so the format lives here as well as there.
        assert!(
            SEGMENT_SRC.contains("{a.target} mask [{backend}] -> {a.output}"),
            "the backend label must stay inside brackets — segment.rs parses it out"
        );
        assert!(
            SEGMENT_SRC.contains("subject backend deps [{'missing' if deps_missing else 'ok'}]"),
            "the dependency verdict line is what makes a stuck fallback re-derivable"
        );
        assert!(
            SEGMENT_SRC.contains("--probe-backend"),
            "the cache must be able to ask this machine's capability without segmenting"
        );
        assert!(
            SEGMENT_SRC.contains("def birefnet_deps_error("),
            "one dependency list for the run and the probe, or the two can disagree"
        );
    }

    /// R29 C3/C4 — the sky backend joins the digest gate, and the SECOND
    /// repository it was quietly fetching from is pinned with it.
    ///
    /// `sky` was the last backend on a revision pin alone: `from_pretrained`
    /// resolved the name and `transformers` fetched. Worse, and invisible to
    /// `SKY_REVISION`, `OneFormerImageProcessor.__init__` ends in
    /// `load_metadata(repo_path, class_info_file)`, which falls through to
    /// `hf_hub_download("shi-labs/oneformer_demo", …, repo_type="dataset")` —
    /// a different repo, at its moving `main`, on every sky mask. Pointing
    /// `repo_path` at the verified directory is what makes that call take its
    /// local branch instead.
    ///
    /// MUTATIONS THIS CATCHES: dropping any of the seven pinned digests,
    /// dropping the class-table pin, removing `local_files_only`, letting
    /// `repo_path` fall back to the dataset repo, or unpinning `use_fast`
    /// (transformers 5.2 defaults it to the Fast processor, whose own warning
    /// says outputs may differ — measured max |Δ| 0.0175 on the normalised
    /// tensor, i.e. a silent library-version-dependent change to the BYTES of a
    /// mask a saved recipe references).
    #[test]
    fn the_sky_backend_is_digest_gated_and_loads_only_local_files() {
        for (what, digest) in [
            ("pytorch_model.bin",
             "c0b2fe11dfecee6f2f1f315f466946e96f4e94813f3f6d660ff3747b83c28cc9"),
            ("config.json",
             "27452b656a467dbdebdf879dc413d6f3facd2bfe3643824ae66c32c22884b4bd"),
            ("preprocessor_config.json",
             "49e2c8f207405d063cf7824f97c2814fa864f8f19ea9e02c9e20a9ff539c6d49"),
            ("merges.txt",
             "9fd691f7c8039210e0fced15865466c65820d09b63988b0174bfe25de299051a"),
            ("vocab.json",
             "e089ad92ba36837a0d31433e555c8f45fe601ab5c221d4f607ded32d9f7a4349"),
            ("tokenizer_config.json",
             "968a6126200b3c8f68fe955d61da20f3537e641a1deb538dc39fdad142248d72"),
            ("special_tokens_map.json",
             "c4864a9376a8401918425bed71fc14fc0e81f9b59ec45c1cf96cccb2df508eac"),
            // Ours since R29 收口 — the digest of a file in this repo, not of a
            // download. `the_sky_class_table_is_ours_and_matches_its_pin`
            // checks it against the shipped bytes.
            ("ade20k_class_table.json",
             "8b93934a55524e5a9320875336cb8bc6ba2a9e6307796e9f22e0cebbc89428d8"),
        ] {
            assert!(
                SEGMENT_SRC.contains(digest),
                "the pinned sha256 for the OneFormer '{what}' is not the one R29 C3/C4 verified"
            );
        }
        // The tokenizer tree is the reason this was never a four-line copy of
        // `_birefnet_cache`; naming it here keeps a future trim from "tidying
        // away" files the processor actually opens.
        for name in ["merges.txt", "vocab.json", "tokenizer_config.json"] {
            assert!(SEGMENT_SRC.contains(name), "the CLIP tokenizer tree must stay pinned: {name}");
        }
        // Fetched by US, loaded from disk, and the class table pointed at the
        // verified directory rather than a remote repo.
        assert!(SEGMENT_SRC.contains("def _sky_cache("), "sky must fetch through its own gate");
        assert!(
            SEGMENT_SRC.contains("def _install_class_table("),
            "the class table must be INSTALLED from python/, not fetched (R29 收口, ruling 11)"
        );
        // The dataset repo R29 C3/C4 pinned is gone, not merely unused: the URL
        // and the pin that built it must not survive anywhere in the source, or
        // a later edit could quietly re-enable the unlicensed fetch.
        for gone in ["oneformer_demo", "SKY_CLASS_INFO_REVISION", "SKY_CLASS_INFO_PIN"] {
            for line in SEGMENT_SRC.lines() {
                assert!(
                    !line.contains(gone) || line.trim_start().starts_with('#'),
                    "the unlicensed class-table repo must not come back as CODE: {line}"
                );
            }
        }
        assert!(
            SEGMENT_SRC.contains("oneformer_demo"),
            "…and the record of WHY it was removed must stay in the file"
        );
        // CODE lines only — the prose above these calls explains the flag, and
        // counting the explanation would make the count drift with the comment.
        let code_lines = || SEGMENT_SRC.lines().map(str::trim).filter(|l| !l.starts_with('#'));
        assert_eq!(
            code_lines().filter(|l| l.contains("local_files_only=True")).count(),
            5,
            "both single- and multi-class sky loads and SAM's must refuse to resolve a remote name"
        );
        assert_eq!(
            code_lines().filter(|l| l.contains("from_pretrained(")).count(),
            5,
            "an unpinned from_pretrained would be another chance to resolve a remote name"
        );
        assert!(
            SEGMENT_SRC.contains("repo_path=d"),
            "without repo_path the processor downloads its class table from a moving main"
        );
        assert!(
            SEGMENT_SRC.contains("use_fast=False"),
            "an unpinned processor class changes the mask bytes across transformers versions"
        );
    }

    /// R29 收口 (ruling 11): the ADE20K class table is OURS, and this is the
    /// contract `OneFormerImageProcessor` will hold it to.
    ///
    /// It replaced a file fetched from `shi-labs/oneformer_demo`, a DATASET repo
    /// with no declared licence — the one asset in this tree that had never been
    /// through the file's own licence criterion. The replacement was rebuilt from
    /// the MIT model repo's own `config.json` `id2label` (names) and
    /// `preprocessor_config.json` `metadata.thing_ids` (the thing/stuff split),
    /// cross-checked against SHI-Labs/OneFormer's MIT `ADE20K_150_CATEGORIES`,
    /// and proved equivalent at the PIXEL level — two sky runs on one frame, old
    /// table against ours, byte-identical mask PNGs.
    ///
    /// The byte-exact gate is the sha256 in `_install_class_table`, which runs on
    /// every sky mask. THIS test is the half that needs no weights: the shape
    /// `prepare_metadata` requires (`image_processing_oneformer.py:367`), the
    /// byte count the pin declares, and no CR — `python/*.json` is `eol=lf` in
    /// `.gitattributes`, and a checkout that converted it would fail the runtime
    /// digest on a tree git considers identical.
    ///
    /// MUTATIONS THIS CATCHES: reordering or renumbering the entries (the key
    /// order IS `class_names`), dropping `isthing` (a `KeyError` inside
    /// transformers on every sky mask), renaming class 2 (`sky_mask` resolves
    /// its plane by exact label match, so the mask would silently become another
    /// class), any insert/delete against the declared byte count, and a CRLF
    /// checkout.
    #[test]
    fn the_sky_class_table_is_ours_and_matches_its_pin() {
        const TABLE: &str =
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/python/ade20k_class_table.json"));

        // The pin, read out of the sidecar so the two can never drift apart.
        let pin = SEGMENT_SRC
            .split_once("SKY_CLASS_TABLE_PIN = {")
            .expect("segment.py must declare SKY_CLASS_TABLE_PIN")
            .1;
        let pin = pin.split_once('}').expect("SKY_CLASS_TABLE_PIN must be a dict").0;
        let field = |k: &str| -> String {
            let v = pin
                .split_once(&format!("\"{k}\": "))
                .unwrap_or_else(|| panic!("SKY_CLASS_TABLE_PIN has no {k}"))
                .1;
            v.trim_start_matches('"').split(['"', ',']).next().unwrap().trim().to_string()
        };
        let sha = field("sha256");
        assert_eq!(sha.len(), 64, "the class-table digest {sha:?} is not a sha256");
        assert!(
            sha.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "the class-table digest {sha:?} is not lowercase hex"
        );
        let want: usize = field("bytes").parse().expect("the byte count must be a number");
        assert!(!TABLE.contains('\r'), "the class table must stay LF — see .gitattributes");
        assert_eq!(
            TABLE.len(),
            want,
            "the shipped class table is {} B but segment.py pins {want} B — the sidecar \
             would refuse it at run time",
            TABLE.len()
        );

        // The shape `prepare_metadata` reads, and NOTHING else: string keys
        // "0".."149" in ascending order (the order IS `class_names`), each an
        // object with `name` and `isthing`.
        let v: serde_json::Value = serde_json::from_str(TABLE).expect("the class table must parse");
        let obj = v.as_object().expect("the class table must be a JSON object");
        assert_eq!(obj.len(), 150, "ADE20K has 150 classes, found {}", obj.len());
        // serde_json's map sorts, so FILE order is read off the text itself.
        let order: Vec<&str> = TABLE
            .lines()
            .filter_map(|l| l.trim().strip_prefix('"'))
            .filter_map(|l| l.split_once('"'))
            .map(|(k, _)| k)
            .collect();
        let want_order: Vec<String> = (0..150).map(|i| i.to_string()).collect();
        assert_eq!(order, want_order, "the entries must run 0..149 in order, one per line");

        let mut things = 0;
        for (k, e) in obj {
            let e = e.as_object().unwrap_or_else(|| panic!("entry {k} is not an object"));
            let mut keys: Vec<&str> = e.keys().map(String::as_str).collect();
            keys.sort_unstable();
            assert_eq!(keys, ["isthing", "name"], "entry {k} carries fields nobody reads");
            assert!(
                e["name"].as_str().is_some_and(|n| !n.is_empty()),
                "entry {k} has no name — `class_names` would carry a hole"
            );
            match e["isthing"].as_u64() {
                Some(0) => {}
                Some(1) => things += 1,
                other => panic!("entry {k} has isthing {other:?}, which is neither 0 nor 1"),
            }
        }
        assert_eq!(things, 100, "ADE20K panoptic splits 150 into 100 things + 50 stuff");

        // The one row this backend actually depends on: `sky_mask` resolves its
        // plane by EXACT label match, and ADE20K also has `skyscraper`.
        assert_eq!(obj["2"]["name"], "sky", "class 2 is the plane the sky mask reads");
        assert_eq!(obj["2"]["isthing"], 0, "sky is stuff, not a thing");
        assert_eq!(
            obj.values().filter(|e| e["name"] == "sky").count(),
            1,
            "exactly one class may be named `sky`, or the exact match picks arbitrarily"
        );
    }

    /// L03 durability: every sidecar publishes through `tmp` + `fsync` +
    /// `os.replace`, so a payload a recipe or an index references cannot vanish
    /// with the page cache.
    #[test]
    fn the_sidecars_publish_durably() {
        for (what, src) in SIDECARS {
            assert!(src.contains("os.fsync("), "{what} must fsync before publishing");
            assert!(src.contains("os.replace("), "{what} must publish atomically");
            let fsync = src.find("os.fsync(").unwrap();
            let replace = src[fsync..].find("os.replace(");
            assert!(replace.is_some(), "{what}: the fsync must PRECEDE the replace");
        }
    }

    /// The embedding contract the Rust side depends on and cannot re-derive:
    /// the vector is L2-normalised IN THE SIDECAR, and the record says so.
    ///
    /// MUTATION: delete the `v / n` normalisation in `embed_batch` and
    /// `parse_vector`'s norm gate rejects every record — a silent
    /// un-normalisation would otherwise make one exemplar dominate the cosine.
    #[test]
    fn the_embedding_sidecar_normalises_and_declares_it() {
        assert!(EMBED_SRC.contains("\"norm\":\"l2\""), "the record must declare its norm");
        assert!(
            EMBED_SRC.contains("v.norm(dim=-1, keepdim=True)") && EMBED_SRC.contains("v = v / n"),
            "the sidecar must L2-normalise before writing"
        );
        // And the width it declares is the width this build indexes.
        assert!(
            EMBED_SRC.contains(&format!("\"dim\": {EMBED_DIM}")),
            "the sidecar's declared dim must match EMBED_DIM = {EMBED_DIM}"
        );
    }

    /// The OTHER half of the fp16 contract: this build passes `--fp16`
    /// unconditionally, so the sidecar must still (a) accept the flag, (b)
    /// apply it on CUDA only — a CPU-only machine must not be handed a half
    /// model — and (c) keep the normalisation in fp32, which is what lets
    /// `parse_vector`'s norm gate stay a ±1e-3 check.
    ///
    /// MUTATION THIS KILLS: removing `--fp16` from `embed.py`'s parser (the
    /// Rust side would then pass an argument argparse rejects and EVERY embed
    /// would fail), or moving `v.float()` after the norm.
    #[test]
    fn the_sidecar_still_accepts_the_flag_this_build_always_passes() {
        assert!(EMBED_SRC.contains("\"--fp16\""), "embed.py must accept --fp16");
        assert!(
            EMBED_SRC.contains("if fp16 and device.startswith(\"cuda\")"),
            "half precision must stay CUDA-only — a CPU box gets the fp32 model"
        );
        let float = EMBED_SRC.find("v = v.float()").expect("the fp32 re-cast must exist");
        let norm = EMBED_SRC.find("v.norm(dim=-1").expect("the norm must exist");
        assert!(float < norm, "the vector must be back in fp32 BEFORE it is normalised");
    }
}
