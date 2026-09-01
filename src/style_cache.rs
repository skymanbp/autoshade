//! The style index's PER-EXEMPLAR content cache — what a rebuild does not have
//! to pay for twice.
//!
//! Until this module every `style-index` build redid all of it for every
//! photograph: the full-resolution embedded-preview decode, the 14-dim hand
//! feature, the SigLIP image vector, the vocabulary scores and the SigLIP text
//! vector. Only the Qwen DESCRIPTION survived, in
//! [`crate::describe::DescriptionCache`]. Adding ONE photograph to a
//! 2,000-photograph library therefore cost the whole library again.
//!
//! **The key is the same shape as the description cache's**, deliberately: the
//! SHA-256 of the STAGED FRAME's bytes ([`crate::describe::frame_digest`]).
//! Content, not path — a library that gained one photograph must not
//! re-measure every photograph after it, and a photograph that was renamed or
//! moved is the same pixels and keeps its answers.
//!
//! **…plus a SOURCE stamp, which is what makes the decode skippable.** The
//! frame digest cannot be known without decoding the RAW to produce the frame,
//! so a digest-only cache would still pay the ~1 s decode per photograph — and
//! could not legitimately cache the 14-dim feature at all, because that
//! feature is a function of the FILE (EXIF + histogram + the photographer's
//! saved rotation), not of the staged frame. Each entry therefore also records
//! the identity of the file it was measured from ([`SourceStamp`]). A build
//! that finds an exact match may reuse the whole entry and never open the RAW;
//! anything else decodes, stages, hashes, and looks the digest up as before —
//! so a photograph that was merely touched still skips every model call.
//!
//! That fast path trusts the filesystem's mtime for "these pixels have not
//! changed", which the digest half never does. It is the only thing here that
//! does, it is bounded to one library's own files, and the build discloses how
//! many photographs took it (`reused N, recomputed M`).
//!
//! **Provenance gates, entry by entry.** An entry is only an answer to the
//! question THIS build is asking: [`CachedExemplar::version`] must be the
//! index's current feature semantics, [`CachedExemplar::provenance`] must be
//! this build's embedding provenance (checkpoint, tokenizer and vocabulary
//! version), and the stored description carries
//! [`crate::describe::CachedDescription`]'s own model/revision/prompt stamp.
//! Anything that does not match is dropped at load, so `len()` is the number
//! of entries that can actually be served.
//!
//! **It is a CACHE, not an index**: losing it costs time, never correctness.
//! The file mechanics — the byte cap, the four degradations, the atomic
//! publish — are [`crate::content_cache`]'s, shared with the description
//! cache.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

/// The cache's file name inside a build's scratch directory. One spelling,
/// because [`cache_path`] and every test that isolates itself must name the
/// same file.
pub const CACHE_FILE: &str = "style-exemplars.json";

/// What [`crate::content_cache`] calls this file in the sentences it prints.
const CACHE_LABEL: &str = "style exemplar cache";

/// How many entries the file may hold: the RAW library's own cap
/// (`style::MAX_STYLE_EXEMPLARS`). The cache serves exactly one library's
/// rebuild, and an entry it can never be asked for is a file this build has to
/// parse for nothing.
pub const MAX_CACHE_ENTRIES: usize = 5_000;

/// A generous bound on ONE entry: two 768-float unit vectors as JSON text, the
/// 14 features, the 33 vocabulary scores and a 512-character description. The
/// same 40 KiB an INDEX exemplar is budgeted at (`style::MAX_EXEMPLAR_BYTES`),
/// for the same contents.
pub const MAX_CACHE_ENTRY_BYTES: usize = 40 * 1024;

/// The whole file's byte cap. Over it the cache is REBUILT and the fact
/// disclosed — a cache never decides whether a user gets a library.
pub const MAX_CACHE_BYTES: usize = MAX_CACHE_ENTRIES * MAX_CACHE_ENTRY_BYTES;

/// The cache inside a NAMED directory.
///
/// A parameter rather than a global read, for the reason every other seam in
/// this tree is: `cargo test` runs with `AUTOSHADE_DATA_DIR` pointing at a real
/// store, so a build driven by a test would otherwise write stub vectors into
/// the user's own cache and a later live build would serve them back.
pub fn cache_path_in(dir: &Path) -> PathBuf {
    dir.join(CACHE_FILE)
}

/// The production location: the per-user store, beside the style index itself.
pub fn cache_path() -> PathBuf {
    cache_path_in(&crate::store::store_root())
}

/// WHICH FILE an entry was measured from, exactly enough to know that the file
/// has not changed since.
///
/// The path is case-folded on Windows, the rule `store::photo_key` already
/// follows, so `d:\pics\a.arw` and `D:\Pics\A.ARW` are one photograph. The
/// saved quarter-turns are part of the identity because the build decodes
/// through them (`decode::decode_raw_turned`): a photograph the user rotated
/// since the last build is a different frame and a different aspect feature,
/// and reusing its vectors would index a portrait as the landscape it no
/// longer is.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceStamp {
    pub path: String,
    pub len: u64,
    pub mtime_ns: u64,
    pub turns: u8,
}

impl SourceStamp {
    /// The stamp of `raw` right now, or `None` when the filesystem cannot
    /// answer (no metadata, no mtime, a pre-1970 mtime). `None` simply means
    /// "no fast path for this photograph" — it decodes, as it always did.
    pub fn of(raw: &Path, turns: u8) -> Option<SourceStamp> {
        let meta = std::fs::metadata(raw).ok()?;
        let mtime = meta
            .modified()
            .ok()?
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        let path = std::path::absolute(raw).ok()?.display().to_string();
        Some(SourceStamp {
            path: if cfg!(windows) { path.to_lowercase() } else { path },
            len: meta.len(),
            mtime_ns: u64::try_from(mtime).ok()?,
            turns: turns % 4,
        })
    }
}

/// One photograph's measured exemplar, and the provenance that makes it
/// re-usable.
///
/// `desc` is [`crate::describe::CachedDescription`] rather than a bare string
/// so the description's OWN provenance rule (checkpoint, revision, prompt
/// version) is the one that decides — ONE definition of "is this prose still
/// an answer to the question we are asking", shared with the description cache
/// instead of restated here.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct CachedExemplar {
    pub source: SourceStamp,
    /// `style::CURRENT_INDEX_VERSION` at write time — what the FOURTEEN
    /// features meant when they were measured.
    pub version: u32,
    pub feat: Vec<f32>,
    #[serde(default)]
    pub embed: Option<Vec<f32>>,
    #[serde(default)]
    pub vocab_scores: Option<Vec<f32>>,
    /// `style::embed_provenance_string()` at write time. Stamps `embed`,
    /// `vocab_scores` AND `desc_embed`: all three come out of the same
    /// checkpoint and the same phrase list.
    #[serde(default)]
    pub provenance: Option<String>,
    #[serde(default)]
    pub desc: Option<crate::describe::CachedDescription>,
    /// The EXACT text `desc_embed` is the vector of (`style::desc_text`'s
    /// answer: the description, or the tag string when there is none). Stored
    /// rather than re-derived, because it is the only way a build can tell
    /// whether the stored vector still describes what this record now says.
    #[serde(default)]
    pub desc_text: Option<String>,
    #[serde(default)]
    pub desc_embed: Option<Vec<f32>>,
}

impl CachedExemplar {
    /// Are the stored EMBEDDING answers this build's answers? A vector from
    /// another checkpoint, tokenizer or phrase list answers a different
    /// question, and serving it would make an index claim provenance it does
    /// not have.
    pub fn embedding_is_current(&self, provenance: &str) -> bool {
        self.provenance.as_deref() == Some(provenance)
    }

    /// The description, when one is stored AND it came from the checkpoint and
    /// prompt this build speaks.
    pub fn current_desc(&self) -> Option<&str> {
        self.desc.as_ref().filter(|d| d.is_current()).map(|d| d.desc.as_str())
    }
}

/// The bands an entry's numbers must sit inside to be served.
///
/// Supplied by [`crate::style`] rather than restated here: they are the same
/// numbers `style::exemplar_is_finite` enforces at the INDEX door, and two
/// copies of a bound is how one of them drifts. A cache is a file on disk, so
/// this module's threat model is the index's ("invariants at the door"), not
/// "we wrote it, so it is fine" — a bit-rotted vector served into an index
/// makes that index refuse to LOAD, turning a cache defect into a destroyed
/// library.
#[derive(Clone, Copy, Debug)]
pub struct Bands {
    pub ndim: usize,
    pub feature_abs: f32,
    pub embed_dim: usize,
    pub vocab: usize,
    pub desc_chars: usize,
    /// The index feature-semantics version this build serves.
    pub version: u32,
}

impl Bands {
    fn admits(&self, e: &CachedExemplar) -> bool {
        // L2-normalised by construction, so the elements AND the norm are
        // checked — the index door's reasoning, verbatim: element bounds alone
        // still admit a 768-dim vector of 1.0s, which the cosine would treat
        // as overwhelmingly similar to everything.
        let unit = |v: &Vec<f32>| {
            v.len() == self.embed_dim
                && v.iter().all(|x| x.is_finite() && x.abs() <= 1.0 + 1e-3)
                && (v.iter().map(|&x| x as f64 * x as f64).sum::<f64>().sqrt() - 1.0).abs() < 1e-3
        };
        e.version == self.version
            && e.feat.len() == self.ndim
            && e.feat.iter().all(|v| v.is_finite() && v.abs() <= self.feature_abs)
            && e.embed.as_ref().is_none_or(unit)
            && e.desc_embed.as_ref().is_none_or(unit)
            && e.vocab_scores
                .as_ref()
                .is_none_or(|v| v.len() == self.vocab && v.iter().all(|x| x.is_finite()))
            && e.desc.as_ref().is_none_or(|d| d.desc.chars().count() <= self.desc_chars)
            // `desc_text` is EITHER a description (already capped at
            // `desc_chars`) or the record's tag string, whose length is a
            // property of the phrase list rather than of this bound — so this
            // one is a sanity cap against unbounded text, not a semantic
            // limit, and a vocabulary that grows must not start invalidating
            // every entry.
            && e.desc_text.as_ref().is_none_or(|t| t.chars().count() <= self.desc_chars.max(2048))
    }
}

/// `<store>/style-exemplars.json` — what this machine has already measured,
/// keyed by the SHA-256 of the frame that produced it.
#[derive(Default)]
pub struct ExemplarCache {
    entries: BTreeMap<String, CachedExemplar>,
    /// Source identity → digest, so a build can ask "have I measured THIS
    /// FILE, unchanged?" before it decodes anything.
    by_source: HashMap<SourceStamp, String>,
    /// How many entries came off disk. The retirement count the build reports
    /// is a difference against this, so it has to survive the inserts.
    loaded: usize,
}

impl ExemplarCache {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many entries this cache was LOADED with, before this build's own
    /// inserts.
    pub fn loaded(&self) -> usize {
        self.loaded
    }

    /// Load the cache at `path`, or an empty one with a disclosed reason.
    ///
    /// Entries outside `bands`, entries whose key is not a digest this cache
    /// could have written, and entries from another feature version are
    /// dropped HERE rather than filtered at every lookup — so `len()` is the
    /// number that can actually be used.
    pub fn load(path: &Path, bands: Bands) -> Self {
        let mut cache = Self::default();
        let parsed: BTreeMap<String, CachedExemplar> =
            crate::content_cache::read_map(path, CACHE_LABEL, MAX_CACHE_BYTES);
        for (key, value) in parsed {
            if crate::content_cache::is_digest(&key) && bands.admits(&value) {
                cache.insert(key, value);
            }
        }
        cache.loaded = cache.entries.len();
        cache
    }

    /// What this cache holds for one frame's content.
    pub fn get(&self, digest: &str) -> Option<&CachedExemplar> {
        self.entries.get(digest)
    }

    /// What this cache holds for one FILE, unchanged since it was measured —
    /// the answer that lets a build skip the decode entirely.
    ///
    /// The entry has to agree that it is about THIS file. Two photographs with
    /// identical pixels (the same shot exported twice, a duplicated roll)
    /// share one frame digest and therefore one entry, and the last one
    /// written owns it — their VECTORS are interchangeable, but the 14
    /// features are not: those come from EXIF and the histogram, which the two
    /// files need not share. Without this check the second file would silently
    /// index under the first one's focal length, hour and aspect.
    pub fn warm(&self, stamp: &SourceStamp) -> Option<(&str, &CachedExemplar)> {
        let digest = self.by_source.get(stamp)?;
        let entry = self.entries.get(digest).filter(|e| &e.source == stamp)?;
        Some((digest.as_str(), entry))
    }

    /// Remember one measured exemplar.
    pub fn insert(&mut self, digest: String, entry: CachedExemplar) {
        self.by_source.insert(entry.source.clone(), digest.clone());
        self.entries.insert(digest, entry);
    }

    /// How many entries this build's `keep` set RETIRES — photographs no
    /// longer in the library (deleted, moved out, or re-edited into a
    /// different frame).
    pub fn retired(&self, keep: &BTreeSet<String>) -> usize {
        self.entries.keys().filter(|k| !keep.contains(*k)).count()
    }

    /// Publish the cache, PRUNED to what this build used.
    ///
    /// Pruned, unlike [`crate::describe::DescriptionCache`], which tops its
    /// retention set back up to its cap: an entry here is ~16 KiB of vector
    /// text against that one's ~200 bytes, and it is only ever asked for by a
    /// build of THIS library. Keeping another library's exemplars alive would
    /// make every build parse tens of megabytes to pay for the case where a
    /// user alternates between two folders — which costs exactly one rebuild.
    pub fn save(&self, path: &Path, keep: &BTreeSet<String>) -> Result<()> {
        let chosen: BTreeMap<&String, &CachedExemplar> =
            self.entries.iter().filter(|(k, _)| keep.contains(*k)).collect();
        if chosen.len() > MAX_CACHE_ENTRIES {
            // Not reachable through a legal build (the library caps at
            // MAX_CACHE_ENTRIES exemplars), so this is a guard, not a policy:
            // refuse rather than keep an arbitrary subset of a library this
            // size and let the next build think it measured the rest.
            bail!(
                "refusing to write a {}-entry {CACHE_LABEL} over {} (cap {MAX_CACHE_ENTRIES})",
                chosen.len(),
                path.display()
            );
        }
        crate::content_cache::publish_map(path, CACHE_LABEL, &chosen, MAX_CACHE_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bands() -> Bands {
        Bands { ndim: 14, feature_abs: 1e3, embed_dim: 4, vocab: 3, desc_chars: 512, version: 5 }
    }

    fn key(byte: u8) -> String {
        format!("{byte:02x}").repeat(32)
    }

    fn entry(path: &str) -> CachedExemplar {
        CachedExemplar {
            source: SourceStamp { path: path.to_string(), len: 10, mtime_ns: 20, turns: 0 },
            version: 5,
            feat: vec![0.5; 14],
            embed: Some(vec![1.0, 0.0, 0.0, 0.0]),
            vocab_scores: Some(vec![0.1, 0.2, 0.3]),
            provenance: Some("model@rev tok v1".into()),
            desc: None,
            desc_text: Some("warm, lifted shadows".into()),
            desc_embed: Some(vec![0.0, 1.0, 0.0, 0.0]),
        }
    }

    fn write_cache(dir: &Path, map: BTreeMap<String, CachedExemplar>) -> PathBuf {
        let path = cache_path_in(dir);
        std::fs::write(&path, serde_json::to_string(&map).unwrap()).unwrap();
        path
    }

    /// MUTATION: drop the `bands.admits` filter in [`ExemplarCache::load`] and
    /// this fails — a bit-rotted vector would reach the index, which then
    /// refuses to LOAD at all (`exemplar_is_finite`), turning a cache defect
    /// into a destroyed library.
    #[test]
    fn the_cache_door_refuses_what_the_index_door_would() {
        let dir = crate::test_dir("style-cache-door");
        let mut poisoned = entry("a");
        // Finite, elementwise in band, and NOT a unit vector — exactly what
        // the cosine would read as overwhelmingly similar to everything.
        poisoned.embed = Some(vec![1.0, 1.0, 1.0, 1.0]);
        let mut wrong_version = entry("b");
        wrong_version.version = 4;
        let mut short_feat = entry("c");
        short_feat.feat = vec![0.5; 13];
        let path = write_cache(
            &dir,
            [
                (key(1), poisoned),
                (key(2), wrong_version),
                (key(3), short_feat),
                (key(4), entry("d")),
                // A key this cache could not have written.
                ("not-a-digest".to_string(), entry("e")),
            ]
            .into_iter()
            .collect(),
        );
        let cache = ExemplarCache::load(&path, bands());
        assert_eq!(cache.len(), 1, "only the well-formed current entry is served");
        assert!(cache.get(&key(4)).is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The SOURCE index: a file whose length, mtime, rotation or path moved is
    /// not the file that was measured.
    ///
    /// MUTATION: drop `turns` from [`SourceStamp`] and the third case fails —
    /// a photograph the user rotated would keep the vectors of the orientation
    /// it no longer has.
    #[test]
    fn the_source_stamp_is_the_whole_identity_of_the_file() {
        let mut cache = ExemplarCache::default();
        let e = entry("lib/a.arw");
        cache.insert(key(7), e.clone());
        assert!(cache.warm(&e.source).is_some(), "the same file is warm");
        for changed in [
            SourceStamp { len: 11, ..e.source.clone() },
            SourceStamp { mtime_ns: 21, ..e.source.clone() },
            SourceStamp { turns: 1, ..e.source.clone() },
            SourceStamp { path: "lib/b.arw".into(), ..e.source.clone() },
        ] {
            assert!(cache.warm(&changed).is_none(), "a changed file is not warm: {changed:?}");
        }
    }

    /// MUTATION: make [`ExemplarCache::save`] top the retention set back up
    /// the way the description cache does, and this fails — a library that
    /// lost a photograph would keep an exemplar for a file that is gone.
    #[test]
    fn a_photograph_that_left_the_library_is_retired_from_the_cache() {
        let dir = crate::test_dir("style-cache-prune");
        let path = cache_path_in(&dir);
        let mut cache = ExemplarCache::default();
        cache.insert(key(1), entry("a"));
        cache.insert(key(2), entry("b"));
        let keep: BTreeSet<String> = [key(1)].into_iter().collect();
        assert_eq!(cache.retired(&keep), 1, "b is what this build no longer holds");
        cache.save(&path, &keep).unwrap();
        let back = ExemplarCache::load(&path, bands());
        assert_eq!(back.len(), 1, "only what this build used survives");
        assert!(back.get(&key(1)).is_some());
        assert!(back.get(&key(2)).is_none());
        assert_eq!(back.loaded(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The provenance gate, per entry: a vector from another checkpoint or
    /// another phrase list is not this build's answer.
    ///
    /// MUTATION: make [`CachedExemplar::embedding_is_current`] answer `true`
    /// unconditionally and this fails.
    #[test]
    fn an_embedding_from_another_checkpoint_is_not_reusable() {
        let e = entry("a");
        assert!(e.embedding_is_current("model@rev tok v1"));
        assert!(!e.embedding_is_current("model@OTHER tok v1"));
        assert!(!e.embedding_is_current("model@rev tok v2"));
        let mut none = e.clone();
        none.provenance = None;
        assert!(!none.embedding_is_current("model@rev tok v1"));
    }

    /// A description is only reusable under the checkpoint and prompt that
    /// wrote it — the rule the description cache already owns, borrowed rather
    /// than restated.
    ///
    /// MUTATION: drop the `is_current` filter in
    /// [`CachedExemplar::current_desc`] and this fails.
    #[test]
    fn a_description_from_another_prompt_is_not_reusable() {
        let mut e = entry("a");
        assert_eq!(e.current_desc(), None, "no description stored");
        e.desc = Some(crate::describe::CachedDescription::current("warm and lifted".into()));
        assert_eq!(e.current_desc(), Some("warm and lifted"));
        if let Some(d) = e.desc.as_mut() {
            d.prompt_version += 1;
        }
        assert_eq!(e.current_desc(), None, "another prompt is another answer");
    }
}
