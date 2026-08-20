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
//! ([`with_model_slot`]) so at most one model is resident whatever the caller's
//! concurrency. Together, 4 concurrent × 1.50 GB becomes 1 × 0.75 GB.
//!
//! **The embedding is additive, in both directions.** An index built without
//! this sidecar loads and retrieves exactly as it did before (the field is
//! `None` and the cosine block contributes nothing), and a build whose sidecar
//! fails keeps going with the 14-dim feature alone. That is deliberate: a
//! 1.5 GB download must never be able to turn a working Style panel off.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::config::Config;

/// The vector width this build expects. Not a preference — [`parse_vector`]
/// refuses any other width, because a checkpoint that answered 1152 dims (the
/// so400m tier) would silently produce an index whose exemplars are not
/// comparable to each other.
pub const EMBED_DIM: usize = 768;

/// Everything one embedding run needs; built from [`Config`] like
/// [`crate::segment::SegmentOpts`].
pub struct EmbedOpts {
    pub python_bin: String,
    pub script: PathBuf,
}

impl EmbedOpts {
    pub fn from_config(cfg: &Config) -> Self {
        EmbedOpts {
            python_bin: cfg.python_bin.clone(),
            script: PathBuf::from(&cfg.embed_script),
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
/// multiplied by `W_EMB_DEFAULT = 2.0` against a 14-dim block whose weights
/// sum to 14.5. fp16 carries ~4.9e-4 of relative precision per element
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

/// The sidecar's argv, as a pure function of the four things that decide it —
/// so the flags this build passes can be pinned by a test instead of being
/// visible only to a running Python.
fn sidecar_args(script: &Path, input: &Path, output: &Path, fp16: bool) -> Vec<std::ffi::OsString> {
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
    v
}

/// SINGLE-FLIGHT over the sidecar PROCESS: at most one model is resident at a
/// time, whatever the caller's concurrency.
///
/// The fan-out this closes (adjudication F3): `StyleIndex::build` runs up to
/// `decode::MAX_CONCURRENT_DECODES` = 4 workers and each one used to spawn its
/// own sidecar, so four SigLIP loads could be live at once — 6.0 GB of fp32
/// weights, 3.0 GB even with `--fp16`, against a consumer GPU that commonly
/// has 4. Nothing in the tree serialised them, because every budget in this
/// codebase is shaped like host RAM (`MAX_CONCURRENT_DECODES` counts 181 MB
/// decodes, `jobs` divides `GlobalMemoryStatusEx`) and the model does not live
/// in host RAM at all.
///
/// A GATE, not a batcher, chosen over wiring `embed.py --manifest`: the
/// manifest path helps only the index build, while this covers the develop-time
/// query too (`pipeline::produce_recipe` under `batch --jobs 3` is three
/// concurrent single-image calls that no manifest can merge), and it needs no
/// new record format, no per-line failure mapping and no second staging
/// lifetime. The cost is honest and bounded: the embedding arm of a build runs
/// serially, while the decode that dominates it still runs four-wide.
///
/// Poison is recovered rather than re-panicked, like every other lock in this
/// tree: one worker panicking inside the sidecar must not turn every other
/// worker's embed into a second panic.
fn with_model_slot<T>(body: impl FnOnce() -> T) -> T {
    static SLOT: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = SLOT.lock().unwrap_or_else(|p| p.into_inner());
    body()
}

/// Run the sidecar on ONE image and return its L2-normalised vector.
///
/// `scratch` is the JSON file the sidecar writes; the caller owns its lifetime
/// (the style build hands over a per-photo temp name so parallel workers never
/// share one). The file-based contract is the same one `denoise.py` and
/// `segment.py` use — stdout stays a diagnostic channel, never the payload.
///
/// SERIALISED against every other call in this process — see
/// [`with_model_slot`]. The staging the caller did (writing the PNG) is
/// deliberately OUTSIDE that gate: it is disk work, it costs no model, and
/// holding the slot across it would serialise the cheap half too.
pub fn embed_file(opts: &EmbedOpts, input: &Path, scratch: &Path) -> Result<Vec<f32>> {
    if !opts.script.exists() {
        bail!(
            "style-embedding sidecar not found at {} — run from the project dir or set \
             AUTOSHOP_EMBED_SCRIPT.",
            opts.script.display()
        );
    }
    crate::pipeline::ensure_parent(scratch)?;
    // Same "exit 0 is not success" rule the other two sidecars enforce: the
    // scratch name may already exist (a previous run's leftover), so the
    // artifact state is sampled BEFORE the spawn.
    let before = crate::artifact_state(scratch);
    let mut cmd = Command::new(&opts.python_bin);
    // ALLOWLIST, not "everything the capability table did not refuse" — see
    // `config::dotenv_child_env`.
    cmd.envs(crate::config::dotenv_child_env());
    cmd.args(sidecar_args(&opts.script, input, scratch, fp16_wanted()))
        // CAPTURE, never inherit: the release GUI is windows_subsystem="windows"
        // and has no console, so an inherited handle discards the reason a
        // missing dependency failed.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::hide_child_console(&mut cmd);
    crate::arm_kill_group(&mut cmd);
    // THE SLOT covers the child's whole life, not just the spawn: what has to
    // be exclusive is the model RESIDENT in the GPU, which exists from the
    // load until the process exits. The sidecar's own timeout
    // (`bounded_child_output`) therefore doubles as this gate's release —
    // a hung sidecar cannot park the pool forever.
    let run = with_model_slot(|| -> Result<std::process::Output> {
        let child = cmd.spawn().with_context(|| {
            format!(
                "launch style-embedding sidecar ({} {}) — is Python on PATH / AUTOSHOP_PYTHON set?",
                opts.python_bin,
                opts.script.display()
            )
        })?;
        let group = crate::assign_kill_group(&child);
        crate::denoise::bounded_child_output(
            child,
            "style-embedding sidecar",
            crate::denoise::sidecar_timeout(),
            "AUTOSHOP_SIDECAR_TIMEOUT_SECS",
            group,
        )
    });
    let out = match run {
        Ok(out) => out,
        Err(error) => {
            crate::denoise::discard_failed_output(scratch, before);
            return Err(error);
        }
    };
    if !out.status.success() {
        crate::denoise::discard_failed_output(scratch, before);
        bail!(
            "style-embedding sidecar exited with {}: {}",
            out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
            crate::sidecar_tail(&out.stderr, &out.stdout)
        );
    }
    let wrote = crate::sidecar_wrote("style-embedding sidecar", scratch, before);
    if wrote.is_err() {
        crate::denoise::discard_failed_output(scratch, before);
        return wrote.map(|()| Vec::new());
    }
    let text = std::fs::read_to_string(scratch)
        .with_context(|| format!("read style-embedding output {}", scratch.display()))?;
    let v = parse_vector(&text).with_context(|| {
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
    Ok(v)
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
            sidecar_args(Path::new("embed.py"), Path::new("in.png"), Path::new("out.json"), fp16)
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
    /// MUTATION THIS KILLS: delete the lock in `with_model_slot` (make it call
    /// `body()` directly) and the four threads below all enter together — the
    /// observed maximum becomes 4 and this fails. That is exactly the state
    /// F3 measured: four workers, four resident SigLIP models, 6.0 GB.
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
                    with_model_slot(|| {
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

    /// Every pinned model file carries BOTH a sha256 and a byte count, in both
    /// sidecars. A partial pin leaves a live unpinned download path — a
    /// revision pin fixes WHICH tree is fetched; only a digest proves the
    /// BYTES.
    ///
    /// MUTATION: delete one `"bytes":` line from either `MODEL` / `SAM` table
    /// and this fails.
    #[test]
    fn every_pinned_sidecar_download_has_a_digest_and_a_byte_cap() {
        for (what, src) in [("embed.py", EMBED_SRC), ("segment.py", SEGMENT_SRC)] {
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
    #[test]
    fn every_hugging_face_revision_pin_is_a_full_commit_hash() {
        let mut found = 0;
        for (what, src) in [("embed.py", EMBED_SRC), ("segment.py", SEGMENT_SRC)] {
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
        assert!(found >= 2, "extractor non-vacuity: found {found} revision pins");
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
        for (what, src) in [("embed.py", EMBED_SRC), ("segment.py", SEGMENT_SRC)] {
            // The CALL, not the word: both docstrings name `trust_remote_code`
            // to explain why it is never used, and a test that banned the
            // token would push that explanation out of the file.
            for banned in ["trust_remote_code=", "hf_hub_download(", "snapshot_download("] {
                assert!(!src.contains(banned), "{what} must not use {banned}");
            }
        }
        // And the fetch itself goes through the ONE verified downloader.
        assert!(EMBED_SRC.contains("_fetch_verified("), "embed.py must fetch through the gate");
        assert!(SEGMENT_SRC.contains("_fetch_verified("), "segment.py must fetch through the gate");
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
        // rembg is still used for `subject`, but ONLY with an explicitly named
        // session — a bare `remove(` would resolve to whatever that install's
        // default is.
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

    /// L03 durability: every sidecar publishes through `tmp` + `fsync` +
    /// `os.replace`, so a payload a recipe or an index references cannot vanish
    /// with the page cache.
    #[test]
    fn the_sidecars_publish_durably() {
        for (what, src) in [("embed.py", EMBED_SRC), ("segment.py", SEGMENT_SRC)] {
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
