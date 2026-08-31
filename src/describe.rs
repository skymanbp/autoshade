//! Look-description bridge — Rust side of the sidecar (`python/describe.py`).
//!
//! Fifth member of the sidecar family, same shell-out pattern as
//! [`crate::denoise`], [`crate::segment`], [`crate::embed`] and
//! [`crate::correspond`]: a local Python process runs Qwen3-VL-2B-Instruct
//! over the staged 512-px frames an index build already produces and writes
//! one short sentence per frame about that photograph's GRADE — white-balance
//! lean, tonality, contrast, saturation, finishing, mood. The weights
//! auto-download to `python/weights` on first run (~4.3 GB, digest pinned) —
//! nothing is stored in this repo.
//!
//! **Why prose beside the tags.** S1 gives every exemplar a SigLIP zero-shot
//! score against [`crate::style::LOOK_VOCAB`], a bounded 33-phrase vocabulary.
//! That is the always-on baseline and it stays: it costs one forward pass the
//! image tower is making anyway. A sentence can say what the vocabulary has no
//! phrase for, and — through the SigLIP TEXT tower — it becomes
//! [`crate::style::StyleExemplar::desc_embed`], the vector the direction text
//! is ranked against (`W_DESC`, the calibrated winner). So the prose is
//! additive in both directions: an index built without it retrieves exactly as
//! it did, and a build whose sidecar fails keeps every other field.
//!
//! **The description is UNTRUSTED text.** It is model output about the user's
//! own photograph, and it reaches a proposer prompt. [`sanitize_desc`] is the
//! door: single line, no control characters, no invisible Cf formatting
//! characters, bounded to [`crate::style::MAX_DESC_CHARS`], non-empty or
//! absent. The sidecar applies the same rule before writing — a bound that
//! protects a prompt has to hold even when the program on disk is replaced.
//!
//! **One model at a time.** The run goes through [`crate::run_model_sidecar`]
//! under the process-wide [`crate::with_model_slot`], so Qwen (4.0 GiB in
//! bf16) is never resident beside SigLIP, SD 2.1, SCUNet or OneFormer.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config::Config;

/// The checkpoint this build speaks to, spelled on the Rust side because it
/// goes into the description cache's key: a cache filled by another checkpoint
/// (or another prompt) is not the same answer and must not be served as one.
pub const MODEL_REPO: &str = "Qwen/Qwen3-VL-2B-Instruct";
pub const MODEL_REVISION: &str = "89644892e4d85e24eaac8bacfd4f463576704203";
/// `python/describe.py`'s `PROMPT_VERSION`. It is part of the cache key, so an
/// edited prompt re-describes rather than serving the old prompt's answers for
/// ever. `the_prompt_version_is_the_sidecars_own` pins the two together.
pub const PROMPT_VERSION: u32 = 1;

/// How many descriptions the on-disk cache may hold, and the per-entry bound
/// the file cap is derived from.
///
/// Same shape as the style index's own caps (`style.rs`): a cap on the RECORD
/// and a cap on the FILE, with a `const` assertion that the second really
/// holds the first — a file cap that could not hold a full cache would be a
/// number nobody could satisfy, and the loader would refuse a cache the
/// builder had just written.
pub const MAX_CACHE_ENTRIES: usize = 20_000;
/// 64-hex key + quotes + the four fields. The worst case is a description of
/// [`crate::style::MAX_DESC_CHARS`] characters that JSON escapes to six bytes
/// each (`\uXXXX`), which is what the runtime half of this check —
/// `a_maximal_cache_entry_fits_its_own_bound` — actually serialises and
/// measures.
pub const MAX_CACHE_ENTRY_BYTES: usize = 4096;
pub const MAX_CACHE_BYTES: usize = MAX_CACHE_ENTRIES * MAX_CACHE_ENTRY_BYTES;
const _: () = assert!(
    MAX_CACHE_ENTRY_BYTES >= crate::style::MAX_DESC_CHARS * 6 + 256,
    "the per-entry bound cannot hold a maximally escaped description plus its key"
);

/// Everything one description run needs; built from [`Config`] like
/// [`crate::embed::EmbedOpts`] and [`crate::correspond::CorrespondOpts`].
pub struct DescribeOpts {
    pub python_bin: String,
    pub script: PathBuf,
}

impl DescribeOpts {
    pub fn from_config(cfg: &Config) -> Self {
        DescribeOpts {
            python_bin: cfg.python_bin.clone(),
            script: PathBuf::from(&cfg.describe_script),
        }
    }

    /// Is the sidecar even present? The index build asks BEFORE it stages a
    /// single frame, so a machine without it says so once instead of failing
    /// per photograph.
    pub fn available(&self) -> bool {
        crate::sidecar_script_present(&self.script)
    }
}

/// Load the model in BFLOAT16 unless the user asked otherwise.
///
/// ON by default, and measured rather than argued: on this machine's RTX
/// 4060 Ti (8 GB) a bf16 run peaks at **4.03 GiB allocated / 4.07 GiB
/// reserved**, so it fits with room for the rest of the desktop; fp32 would
/// need ~8.5 GiB of weights alone and would not. bf16 is also the precision
/// the checkpoint's own `config.json` declares (`"dtype": "bfloat16"`), i.e.
/// what it was trained and released in.
///
/// The escape hatch is real rather than decorative: `AUTOSHADE_DESCRIBE_FP32`
/// forces the fp32 load, which is the only way to describe on a machine whose
/// GPU disagrees with bf16 — and on CPU the sidecar ignores the flag by its
/// own construction (`describe.py`: `bf16 and device.startswith("cuda")`).
fn bf16_wanted() -> bool {
    !crate::config::live_env("AUTOSHADE_DESCRIBE_FP32")
        .map(|v| !matches!(v.trim(), "" | "0" | "false" | "off"))
        .unwrap_or(false)
}

/// The sidecar's argv, as a pure function of the three things that decide it —
/// so the flags this build passes can be pinned by a test instead of being
/// visible only to a running Python.
fn sidecar_args(
    script: &Path,
    manifest: &Path,
    output: &Path,
    bf16: bool,
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
    if bf16 {
        v.push("--bf16".into());
    }
    v
}

/// One line of the sidecar's JSONL answer: the frame it describes, and either
/// a description or the sidecar's own reason for declining it.
#[derive(Debug, Clone)]
pub struct DescribeRecord {
    pub path: String,
    pub desc: Option<String>,
    pub error: Option<String>,
}

/// Run the sidecar over a JSONL manifest of frame paths and return one record
/// per line, IN THE SIDECAR'S ORDER.
///
/// The caller maps them back by PATH, never by position: `describe.py` reports
/// a malformed manifest line immediately and the rest in loop order, so a
/// positional zip would attach one photograph's description to another's
/// exemplar — the same failure `embed::parse_text_vectors` refuses a short
/// batch to avoid.
///
/// ONE process for a whole build. That is the entire point of the manifest
/// door: the expensive part of this sidecar is the 4.3 GB model LOAD, and a
/// per-photograph call would pay it 169 times for the user's own library.
pub fn describe_manifest(
    opts: &DescribeOpts,
    manifest: &Path,
    scratch: &Path,
) -> Result<Vec<DescribeRecord>> {
    if !opts.script.exists() {
        bail!(
            "look-description sidecar not found at {} — run from the project dir or set \
             AUTOSHADE_DESCRIBE_SCRIPT.",
            opts.script.display()
        );
    }
    let text = crate::run_model_sidecar(
        "look-description sidecar",
        &opts.python_bin,
        sidecar_args(&opts.script, manifest, scratch, bf16_wanted()),
        scratch,
    )?;
    let out = parse_records(&text).with_context(|| {
        format!("look-description sidecar wrote an unusable batch at {}", scratch.display())
    })?;
    // The scratch file is an INTERMEDIATE, not an artifact: the descriptions
    // are what the caller keeps, and leaving it behind would litter the store
    // with a JSONL nothing reads again.
    let _ = std::fs::remove_file(scratch);
    Ok(out)
}

/// The sidecar's JSONL → records, each description through [`sanitize_desc`].
///
/// A line that is not JSON is a FAILURE of the whole batch, not of one record:
/// the sidecar writes its output atomically and every line it writes is one it
/// composed with `json.dumps`, so unparseable text means the file is not the
/// answer it claims to be. A line that carries `error` IS a per-record
/// failure and is kept as one — that is the fail-soft contract.
pub fn parse_records(text: &str) -> Result<Vec<DescribeRecord>> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let rec: serde_json::Value = serde_json::from_str(line.trim())
            .with_context(|| format!("look-description line {} is not JSON", i + 1))?;
        let path = rec
            .get("path")
            .and_then(|v| v.as_str())
            .with_context(|| format!("look-description line {} names no path", i + 1))?
            .to_string();
        if let Some(e) = rec.get("error").and_then(|v| v.as_str()) {
            out.push(DescribeRecord { path, desc: None, error: Some(e.to_string()) });
            continue;
        }
        // The record must declare the checkpoint AND the prompt it came from,
        // and both must be the ones this build caches under. A sidecar that
        // answered from another revision would fill the cache with entries the
        // key says are ours.
        let says = |key: &str| rec.get(key).and_then(|v| v.as_str()).unwrap_or_default();
        if says("model") != MODEL_REPO || says("revision") != MODEL_REVISION {
            bail!(
                "look-description sidecar answered from {}@{}, and this build caches \
                 descriptions under {MODEL_REPO}@{MODEL_REVISION}",
                says("model"),
                says("revision")
            );
        }
        let version = rec.get("prompt_version").and_then(|v| v.as_u64());
        if version != Some(PROMPT_VERSION as u64) {
            bail!(
                "look-description sidecar answered prompt version {:?}, and this build caches \
                 descriptions under v{PROMPT_VERSION}",
                version
            );
        }
        let desc = rec.get("desc").and_then(|v| v.as_str()).and_then(sanitize_desc);
        match desc {
            Some(d) => out.push(DescribeRecord { path, desc: Some(d), error: None }),
            None => out.push(DescribeRecord {
                path,
                desc: None,
                error: Some("the sidecar's description was empty after the door's bounds".into()),
            }),
        }
    }
    Ok(out)
}

/// THE DOOR: one model answer → the single bounded line an index may carry.
///
/// Four rules, each a real failure mode rather than tidiness:
/// * control characters (`\n` included) become spaces — a description is
///   appended to a proposer block as one field of one line, and a newline in
///   it would forge a new line of that block;
/// * the invisible Cf block (soft hyphen, zero-width joiners, the bidi
///   overrides, the interlinear annotations, the BOM) is stripped — those are
///   exactly what a payload hides behind, and they survive a "printable"
///   filter;
/// * runs of whitespace collapse, so the bound below counts content;
/// * the result is cut to [`crate::style::MAX_DESC_CHARS`] CHARACTERS (never
///   bytes — a byte slice would split a multi-byte codepoint) and an empty
///   result is `None`, because an empty description is an absent one.
pub fn sanitize_desc(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len().min(crate::style::MAX_DESC_CHARS * 4));
    let mut pending_space = false;
    for ch in text.chars() {
        let drop_it = is_invisible(ch);
        let space = ch.is_whitespace() || (ch as u32) < 0x20 || ch == '\u{7f}';
        if drop_it || space {
            // Only mark a gap once we have content: leading whitespace is
            // trimmed by construction rather than by a second pass.
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        if out.chars().count() >= crate::style::MAX_DESC_CHARS {
            break;
        }
        out.push(ch);
    }
    (!out.is_empty()).then_some(out)
}

/// The Cf (format) characters [`sanitize_desc`] removes, by CODE POINT — the
/// characters themselves are invisible in an editor, so a literal set could
/// not be reviewed. Mirrors `describe.py`'s `_INVISIBLE`.
fn is_invisible(ch: char) -> bool {
    matches!(
        ch,
        '\u{00ad}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}'
    )
}

/// The CONTENT key of one staged frame: the SHA-256 of its bytes.
///
/// Content, not path: the build names its staged frames `…-idx-<n>.png`, and
/// `<n>` is the position in THIS build's file list. Keying on the name would
/// mean a library that gained one photograph re-described every photograph
/// after it, which is exactly the cost the cache exists to remove.
pub fn frame_digest(path: &Path) -> Result<String> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("read staged frame {}", path.display()))?;
    let mut s = crate::sha256::Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("read staged frame {}", path.display()))?;
        if n == 0 {
            break;
        }
        s.update(&buf[..n]);
    }
    Ok(s.finish())
}

// --- the content-keyed cache ------------------------------------------------

/// One cached description and the provenance that makes it re-usable.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct CachedDescription {
    pub desc: String,
    pub model: String,
    pub revision: String,
    pub prompt_version: u32,
}

impl CachedDescription {
    /// Is this entry an answer to the question THIS build is asking? The
    /// checkpoint, the revision and the prompt version all have to match — a
    /// description written by another prompt is a different answer to a
    /// different question, and serving it would make an index claim provenance
    /// it does not have.
    pub fn is_current(&self) -> bool {
        self.model == MODEL_REPO
            && self.revision == MODEL_REVISION
            && self.prompt_version == PROMPT_VERSION
    }
}

/// `<store>/style-descriptions.json` — the descriptions this machine has
/// already paid for, keyed by the SHA-256 of the frame that produced them.
///
/// It is a CACHE, not an index: losing it costs time, never correctness, so
/// every failure path here degrades to "describe again" with a sentence rather
/// than failing a build. A corrupt file is REBUILT and the fact disclosed —
/// refusing to build because a cache would not parse would be the cache
/// deciding whether the user gets a library.
#[derive(Default)]
pub struct DescriptionCache {
    entries: std::collections::BTreeMap<String, CachedDescription>,
}

/// The cache's file name inside a build's scratch directory. One spelling,
/// because [`cache_path`] and every test that isolates itself must name the
/// same file.
pub const CACHE_FILE: &str = "style-descriptions.json";

/// The cache inside a NAMED directory.
///
/// A parameter rather than a global read, for the reason every other seam in
/// this tree is: `cargo test` runs with `AUTOSHADE_DATA_DIR` pointing at a real
/// store, so a build driven by a test would otherwise write stub descriptions
/// into the user's own cache and a later live build would serve them.
pub fn cache_path_in(dir: &Path) -> PathBuf {
    dir.join(CACHE_FILE)
}

/// The production location: the per-user store, beside the style index itself.
pub fn cache_path() -> PathBuf {
    cache_path_in(&crate::store::store_root())
}

impl DescriptionCache {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Load the cache at `path`, or an empty one with a disclosed reason.
    ///
    /// Three degradations, all silent-by-design only in the "no cache yet"
    /// case: an absent file is empty and says nothing; an over-cap or
    /// unparseable file is empty AND prints why; entries from another
    /// checkpoint or prompt are dropped at load rather than filtered at every
    /// lookup, so `len()` is the number of entries that can actually be used.
    pub fn load(path: &Path) -> Self {
        use std::io::Read as _;
        if !path.exists() {
            return Self::default();
        }
        let mut cache = Self::default();
        let text = match std::fs::File::open(path).and_then(|f| {
            let mut bytes = Vec::new();
            f.take((MAX_CACHE_BYTES + 1) as u64).read_to_end(&mut bytes)?;
            Ok(bytes)
        }) {
            Ok(bytes) if bytes.len() > MAX_CACHE_BYTES => {
                eprintln!(
                    "  description cache {} exceeds the {}-byte cap — rebuilding it",
                    path.display(),
                    MAX_CACHE_BYTES
                );
                return cache;
            }
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(t) => t,
                Err(_) => {
                    eprintln!(
                        "  description cache {} is not UTF-8 — rebuilding it",
                        path.display()
                    );
                    return cache;
                }
            },
            Err(e) => {
                eprintln!(
                    "  description cache {} is unreadable ({e}) — rebuilding it",
                    path.display()
                );
                return cache;
            }
        };
        let parsed: std::collections::BTreeMap<String, CachedDescription> =
            match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!(
                        "  description cache {} is unusable ({e}) — rebuilding it",
                        path.display()
                    );
                    return cache;
                }
            };
        for (key, value) in parsed {
            if is_digest(&key)
                && value.is_current()
                && let Some(desc) = sanitize_desc(&value.desc)
            {
                cache.entries.insert(key, CachedDescription { desc, ..value });
            }
        }
        cache
    }

    /// The description for this frame, if this machine has already paid for it
    /// under the same checkpoint and prompt.
    pub fn get(&self, digest: &str) -> Option<&str> {
        self.entries.get(digest).map(|e| e.desc.as_str())
    }

    /// Remember one description. The value is stamped with THIS build's
    /// provenance, never with whatever the caller happened to hold.
    pub fn insert(&mut self, digest: String, desc: String) {
        self.entries.insert(
            digest,
            CachedDescription {
                desc,
                model: MODEL_REPO.to_string(),
                revision: MODEL_REVISION.to_string(),
                prompt_version: PROMPT_VERSION,
            },
        );
    }

    /// Publish the cache, bounded and atomically.
    ///
    /// `keep` is the set of keys THIS build used; when the cache is over
    /// [`MAX_CACHE_ENTRIES`] those are retained first and the remainder is
    /// filled in ascending key order. Deterministic on purpose: an LRU would
    /// need a clock in every entry, and two machines evicting differently is a
    /// difference nobody could reproduce.
    pub fn save(&self, path: &Path, keep: &std::collections::BTreeSet<String>) -> Result<()> {
        let mut chosen: std::collections::BTreeMap<&String, &CachedDescription> =
            self.entries.iter().filter(|(k, _)| keep.contains(*k)).collect();
        if chosen.len() > MAX_CACHE_ENTRIES {
            // A build that produced more entries than the cap is not a state
            // the library caps allow (5,000 RAW + 500 looks), so this is a
            // guard rather than a policy: keep the first N by key.
            let over: Vec<&String> = chosen.keys().skip(MAX_CACHE_ENTRIES).copied().collect();
            for k in over {
                chosen.remove(k);
            }
        }
        for (k, v) in &self.entries {
            if chosen.len() >= MAX_CACHE_ENTRIES {
                break;
            }
            chosen.entry(k).or_insert(v);
        }
        let text = serde_json::to_string(&chosen).context("serialise the description cache")?;
        if text.len() > MAX_CACHE_BYTES {
            bail!(
                "refusing to write a {}-byte description cache over {} (cap {} bytes)",
                text.len(),
                path.display(),
                MAX_CACHE_BYTES
            );
        }
        crate::pipeline::ensure_parent(path)?;
        // tmp + durable replace, like the style index itself: this file is
        // read whole, so a half-written one would be a cache that disables
        // itself on the next build.
        static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let tmp = path.with_extension(format!(
            "json.tmp{}-{}",
            std::process::id(),
            TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&tmp, text)
            .with_context(|| format!("write description cache {}", tmp.display()))?;
        if let Err(e) = crate::store::durable_replace(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("publish description cache {}", path.display()));
        }
        Ok(())
    }
}

/// A 64-hex lowercase key, i.e. something this cache could have written. A
/// foreign key is dropped at load rather than trusted as a digest.
fn is_digest(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESCRIBE_SRC: &str =
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/python/describe.py"));
    /// The shared plumbing describe.py binds rather than copies — including
    /// the walk across the pin table this module's own contract below checks.
    const SIDECAR_SHARED_SRC: &str =
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/python/_sidecar.py"));

    /// MUTATION: weaken any one rule in `sanitize_desc` and this fails. The
    /// description reaches a proposer prompt, so each rule is a real boundary
    /// rather than tidiness.
    #[test]
    fn describe_output_is_bounded_single_line_at_the_door() {
        // A newline would forge a new line of the reference block.
        assert_eq!(
            sanitize_desc("warm, lifted\nshadows\r\nand grain").as_deref(),
            Some("warm, lifted shadows and grain")
        );
        // Control characters and the invisible Cf block never survive.
        assert_eq!(
            sanitize_desc("cool\u{202e}blue\u{200b}\u{feff} tones\u{7f}").as_deref(),
            Some("cool blue tones")
        );
        // The bound counts CHARACTERS, and a multi-byte codepoint is never
        // split (a byte slice at 512 would panic on this input).
        let long: String = "é".repeat(crate::style::MAX_DESC_CHARS + 40);
        let cut = sanitize_desc(&long).expect("a long description is kept, cut");
        assert_eq!(cut.chars().count(), crate::style::MAX_DESC_CHARS);
        // Empty in, absent out — an empty description is not a description.
        assert!(sanitize_desc("   \u{200b}\n ").is_none());
        assert!(sanitize_desc("").is_none());
    }

    /// The cache is a CONTENT key: the same frame bytes hit however the file
    /// is named, and a prompt-version bump misses so the answers of the OLD
    /// prompt can never be served as the new one's.
    #[test]
    fn description_cache_hits_by_content_and_misses_on_prompt_version() {
        let dir = crate::test_dir("describe-cache");
        let a = dir.join("frame-idx-0.png");
        let b = dir.join("frame-idx-7.png");
        std::fs::write(&a, b"the same staged pixels").unwrap();
        std::fs::write(&b, b"the same staged pixels").unwrap();
        let other = dir.join("frame-idx-9.png");
        std::fs::write(&other, b"different staged pixels").unwrap();

        let (da, db, dother) = (
            frame_digest(&a).unwrap(),
            frame_digest(&b).unwrap(),
            frame_digest(&other).unwrap(),
        );
        assert_eq!(da, db, "identical bytes under two names are one cache entry");
        assert_ne!(da, dother);

        let path = dir.join("style-descriptions.json");
        let mut cache = DescriptionCache::default();
        cache.insert(da.clone(), "a warm, lifted grade".into());
        let keep: std::collections::BTreeSet<String> = [da.clone()].into_iter().collect();
        cache.save(&path, &keep).unwrap();

        let reloaded = DescriptionCache::load(&path);
        assert_eq!(reloaded.get(&db), Some("a warm, lifted grade"), "content key hits");
        assert_eq!(reloaded.get(&dother), None, "a different frame is a miss");

        // A prompt-version bump: the SAME digest, a stale entry, and the
        // loader must drop it rather than serve the old prompt's answer.
        let stale = serde_json::json!({
            &da: {
                "desc": "an answer to a question we no longer ask",
                "model": MODEL_REPO,
                "revision": MODEL_REVISION,
                "prompt_version": PROMPT_VERSION + 1,
            }
        });
        std::fs::write(&path, serde_json::to_string(&stale).unwrap()).unwrap();
        assert_eq!(DescriptionCache::load(&path).get(&da), None, "a newer prompt version misses");

        // …and so does another checkpoint at the same prompt version.
        let foreign = serde_json::json!({
            &da: {
                "desc": "written by another model",
                "model": "someone/else",
                "revision": MODEL_REVISION,
                "prompt_version": PROMPT_VERSION,
            }
        });
        std::fs::write(&path, serde_json::to_string(&foreign).unwrap()).unwrap();
        assert_eq!(DescriptionCache::load(&path).get(&da), None, "another checkpoint misses");

        // A CORRUPT cache is rebuilt, never fatal.
        std::fs::write(&path, b"{not json at all").unwrap();
        assert!(DescriptionCache::load(&path).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The runtime half of the `const` assertion above: a MAXIMAL entry —
    /// 512 characters that each JSON-escape to six bytes — really serialises
    /// inside [`MAX_CACHE_ENTRY_BYTES`], so the file cap can hold a full
    /// cache. The compile-time check only compares two numbers; this one
    /// measures the bytes serde actually writes.
    #[test]
    fn a_maximal_cache_entry_fits_its_own_bound() {
        let worst: String = "\u{7}".repeat(crate::style::MAX_DESC_CHARS);
        let entry = CachedDescription {
            desc: worst,
            model: MODEL_REPO.to_string(),
            revision: MODEL_REVISION.to_string(),
            prompt_version: PROMPT_VERSION,
        };
        let key = "f".repeat(64);
        let one: std::collections::BTreeMap<&str, &CachedDescription> =
            [(key.as_str(), &entry)].into_iter().collect();
        let bytes = serde_json::to_string(&one).unwrap().len();
        assert!(
            bytes <= MAX_CACHE_ENTRY_BYTES,
            "a maximal entry is {bytes} B, over the {MAX_CACHE_ENTRY_BYTES} B bound the file \
             cap is derived from"
        );
    }

    /// The sidecar's `--self-test` covers EVERY file it downloads, and it
    /// covers them by walking the pin table rather than by naming a hand-kept
    /// subset.
    ///
    /// This is the contract that makes a partial cache loud instead of
    /// mysterious: a Qwen checkpoint whose `model.safetensors` is a 4.1 GB
    /// truncated download still loads its config and its tokenizer, and the
    /// failure it produces at generate time names a tensor, not a file. The
    /// self-test compares each pinned file's size on disk against its byte
    /// count, so the truncation is named where it happened.
    ///
    /// MUTATION THIS KILLS: change either loop in `describe.py` to iterate a
    /// literal list of a few file names instead of `MODEL["files"]`, or drop
    /// the size comparison from the self-test.
    #[test]
    fn describe_sidecar_self_test_pins_every_file() {
        // The table is the sidecar's own, so count it out of the source: a
        // test that hard-coded 10 would pass a table someone had halved.
        let table = DESCRIBE_SRC
            .split("\"files\": {")
            .nth(1)
            .expect("describe.py declares a pinned file table");
        let pinned = table.matches("\"sha256\":").count();
        assert!(pinned >= 8, "extractor non-vacuity: {pinned} pinned files");
        assert_eq!(pinned, table.matches("\"bytes\":").count(), "a pin without a byte count");

        // Every walk is over the table ITSELF — a hand-kept subset is how a
        // pinned file quietly stops being checked. The FETCH's walk moved into
        // `_sidecar.fetch_model` when the three single-artifact sidecars
        // stopped each keeping a copy, so it is checked there, over the table
        // describe.py hands it; the self-test's two walks are still its own.
        let walk = "for name, pin in MODEL[\"files\"].items():";
        let fetch = SIDECAR_SHARED_SRC
            .split("def fetch_model(")
            .nth(1)
            .expect("_sidecar.py has a fetch")
            .split("\ndef ")
            .next()
            .unwrap();
        assert!(
            fetch.contains("for name, pin in model[\"files\"].items():"),
            "the fetch must walk the pin table, not a subset"
        );
        assert!(
            DESCRIBE_SRC.contains("_sidecar.fetch_model(MODEL, cache_dir,"),
            "describe.py must hand its own MODEL table to that fetch"
        );
        let selftest = DESCRIBE_SRC
            .split("def self_test(")
            .nth(1)
            .expect("describe.py has a --self-test")
            .split("\ndef ")
            .next()
            .unwrap();
        assert_eq!(
            selftest.matches(walk).count(),
            2,
            "the self-test walks the table twice: the digest/byte shape, then the sizes on disk"
        );
        assert!(
            selftest.contains("for name in MODEL[\"files\"]"),
            "…and a third walk decides whether the cache is present at all"
        );
        assert!(
            selftest.contains("os.path.getsize(") && selftest.contains("pin[\"bytes\"]"),
            "the self-test must compare each pinned file's size on disk against its pin"
        );
        assert!(
            selftest.contains("the_pinned_files_are_present_at_their_sizes"),
            "the size check must report under a named heading"
        );
        // A missing checkpoint is a SKIP, not a pass: a green self-test on a
        // machine with no weights must not read as a verified one.
        assert!(
            selftest.contains("the_pinned_files_are_present_at_their_sizes: SKIP"),
            "an absent cache must skip aloud rather than silently pass"
        );
        // The fetch goes through the ONE verified downloader, with the repo
        // and revision this build pins. (`SIDECARS` in `embed.rs` holds the
        // family-wide half of this; here it is named for the fifth member.)
        // Reached through `_sidecar`, which is the only caller left that
        // names it — so both halves are checked: this script binds that fetch,
        // and that fetch is the verified one.
        assert!(
            DESCRIBE_SRC.contains("import _sidecar")
                && DESCRIBE_SRC.contains("_sidecar.fetch_model(MODEL, cache_dir,"),
            "the fetch must go through the gate"
        );
        assert!(
            SIDECAR_SHARED_SRC.contains("_fetch_verified("),
            "…and the shared fetch must be the verified one"
        );
        assert!(
            DESCRIBE_SRC.contains("local_files_only=True"),
            "loading must never reach the network behind the gate"
        );
    }

    /// ONE MODEL RESIDENT AT A TIME. The description sidecar runs through the
    /// SHARED executor, so it takes the process-wide model slot that the
    /// embedding and correspondence sidecars take.
    ///
    /// This is a hardware contract, not tidiness: Qwen3-VL-2B in bf16 peaks at
    /// 4.03 GiB and SigLIP 2 at about 1.5 GB on the 8 GB card this was measured
    /// on, and a build runs BOTH — the description stage between the image and
    /// text stages. Spawning the second while the first is still resident is
    /// how a library rebuild turns into an out-of-memory failure two hundred
    /// photographs in.
    ///
    /// The slot's own behaviour is pinned by
    /// `embed::tests::the_model_slot_admits_one_caller_at_a_time`; what is
    /// pinned HERE is that this bridge is inside it.
    ///
    /// MUTATION THIS KILLS: replace `crate::run_model_sidecar` in
    /// `describe_manifest` with a direct `Command` spawn — the sidecar still
    /// works, and two checkpoints become co-resident.
    #[test]
    fn the_description_sidecar_runs_inside_the_shared_model_slot() {
        // include_str! carries the checkout's line endings (LF on CI,
        // CRLF under autocrlf); normalize or the trimming split silently
        // returns the whole tail.
        let me = include_str!("describe.rs").replace("\r\n", "\n");
        let body = me
            .split("pub fn describe_manifest(")
            .nth(1)
            .expect("the bridge exists")
            .split("\npub fn ")
            .next()
            .unwrap();
        assert!(
            body.contains("crate::run_model_sidecar("),
            "the description run must go through the shared executor"
        );
        for direct in ["Command::new", "std::process::Command"] {
            assert!(!body.contains(direct), "the bridge must not spawn {direct} itself");
        }
        // …and the shared executor really is what holds the slot.
        let lib = include_str!("lib.rs").replace("\r\n", "\n");
        let exec = lib
            .split("pub fn run_model_sidecar_bounded(")
            .nth(1)
            .expect("the shared executor exists")
            .split("\npub fn ")
            .next()
            .unwrap();
        assert!(
            exec.contains("with_model_slot("),
            "the shared executor must take the process-wide model slot"
        );
    }

    /// The two halves of the cache key that live in two files must agree.
    ///
    /// MUTATION: bump `PROMPT_VERSION` in `describe.py` (or edit the prompt
    /// without bumping it) and this fails — which is the only thing standing
    /// between an edited prompt and a cache that serves the old prompt's
    /// answers for ever.
    #[test]
    fn the_prompt_version_is_the_sidecars_own() {
        assert!(
            DESCRIBE_SRC.contains(&format!("PROMPT_VERSION = {PROMPT_VERSION}")),
            "python/describe.py must declare PROMPT_VERSION = {PROMPT_VERSION}"
        );
        assert!(
            DESCRIBE_SRC.contains(&format!("\"repo\": \"{MODEL_REPO}\"")),
            "python/describe.py must pin the repo this build caches under"
        );
        assert!(
            DESCRIBE_SRC.contains(&format!("\"revision\": \"{MODEL_REVISION}\"")),
            "python/describe.py must pin the revision this build caches under"
        );
        // …and the prompt itself must still be the one the version names: a
        // silent edit is exactly what the version exists to make impossible.
        for phrase in [
            "Describe ONLY the photographic grade",
            "Do not name subjects, places, objects or people",
            "Output one plain sentence",
        ] {
            assert!(DESCRIBE_SRC.contains(phrase), "the prompt lost {phrase:?}");
        }
    }

    /// R28-shaped precision contract, for the fifth sidecar: this build passes
    /// `--bf16` unconditionally, so the sidecar must accept it, apply it on
    /// CUDA only, and the escape hatch must really escape.
    ///
    /// MUTATION THIS KILLS: dropping the `--bf16` push (every description
    /// would then load 8.5 GB of fp32 weights, which does not fit the 8 GB
    /// card this was measured on), or wiring the env var the wrong way round
    /// so `AUTOSHADE_DESCRIBE_FP32` turned bf16 ON.
    #[test]
    fn the_describe_argv_carries_the_precision_flag_and_the_manifest_door() {
        let args = |bf16| {
            sidecar_args(
                Path::new("describe.py"),
                Path::new("frames.jsonl"),
                Path::new("out.jsonl"),
                bf16,
            )
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
        };
        let on = args(true);
        assert!(on.contains(&"--bf16".to_string()), "{on:?}");
        assert!(!args(false).contains(&"--bf16".to_string()));
        // `-E` first: import-hijack hardening, not decoration.
        assert_eq!(on.first().map(String::as_str), Some("-E"));
        // The BATCH door and nothing else — a per-image `--input` here would
        // reload 4.3 GB of weights per photograph.
        assert_eq!(on.iter().filter(|a| *a == "--manifest-jsonl").count(), 1);
        assert!(!on.iter().any(|a| a == "--input"), "{on:?}");
        assert!(DESCRIBE_SRC.contains("\"--bf16\""), "describe.py must accept --bf16");
        assert!(
            DESCRIBE_SRC.contains("bf16 and device.startswith(\"cuda\")"),
            "half precision must stay CUDA-only — a CPU box gets the fp32 model"
        );
    }

    /// The parse door: a record from another checkpoint, another prompt
    /// version, or with no path is REFUSED for the whole batch; a record
    /// carrying `error` is one photograph's failure and is kept as one.
    #[test]
    fn a_description_batch_must_declare_the_checkpoint_it_answered_from() {
        let good = format!(
            "{{\"path\":\"a.png\",\"model\":\"{MODEL_REPO}\",\"revision\":\"{MODEL_REVISION}\",\
             \"prompt_version\":{PROMPT_VERSION},\"desc\":\"a warm grade\"}}\n\
             {{\"path\":\"b.png\",\"error\":\"cannot read image\"}}\n"
        );
        let recs = parse_records(&good).expect("a well-formed batch parses");
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].desc.as_deref(), Some("a warm grade"));
        assert!(recs[1].desc.is_none() && recs[1].error.is_some());

        let foreign = format!(
            "{{\"path\":\"a.png\",\"model\":\"someone/else\",\"revision\":\"{MODEL_REVISION}\",\
             \"prompt_version\":{PROMPT_VERSION},\"desc\":\"x\"}}\n"
        );
        let e = parse_records(&foreign).unwrap_err().to_string();
        assert!(e.contains("someone/else"), "{e}");

        let stale = format!(
            "{{\"path\":\"a.png\",\"model\":\"{MODEL_REPO}\",\"revision\":\"{MODEL_REVISION}\",\
             \"prompt_version\":{},\"desc\":\"x\"}}\n",
            PROMPT_VERSION + 1
        );
        let e = parse_records(&stale).unwrap_err().to_string();
        assert!(e.contains("prompt version"), "{e}");

        // A description that sanitises to nothing is that photograph's
        // failure, with a reason — never a silent empty string in the index.
        let blank = format!(
            "{{\"path\":\"a.png\",\"model\":\"{MODEL_REPO}\",\"revision\":\"{MODEL_REVISION}\",\
             \"prompt_version\":{PROMPT_VERSION},\"desc\":\"  \u{200b} \"}}\n"
        );
        let recs = parse_records(&blank).expect("a blank description is a per-record failure");
        assert!(recs[0].desc.is_none() && recs[0].error.is_some());
    }
}
