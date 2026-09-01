//! ONE mechanism for every content-keyed cache this app keeps on disk.
//!
//! Two caches share it — [`crate::describe::DescriptionCache`] (the Qwen prose
//! a build has already paid for) and [`crate::style_cache::ExemplarCache`]
//! (the decode, the 14-dim feature and the SigLIP vectors it has already
//! paid for) — and both are the same file shape: a JSON object mapping a
//! SHA-256 content digest to a value, read whole and published atomically.
//!
//! Split out when the second cache arrived, rather than copied: the four
//! degradations below are the load-bearing part of "a cache is never allowed
//! to decide whether the user gets a library", and two copies of that rule is
//! how one of them quietly stops degrading.
//!
//! **Every read failure is an EMPTY cache plus one sentence, never an error.**
//! Losing a cache costs time, never correctness — so an absent file is empty
//! and says nothing, and an over-cap, non-UTF-8, unreadable or unparseable
//! file is empty AND prints why. Refusing to build because a cache would not
//! parse would be the cache deciding whether the user gets a library.
//!
//! **Every write is tmp + durable replace.** These files are read WHOLE, so a
//! half-written one is a cache that disables itself on the next build;
//! `fs::write` truncates in place, so a disk-full or an interrupt mid-write
//! would leave exactly that. The staging name carries the pid and a sequence
//! number because a fixed name let two concurrent builders — the web server's
//! request threads, or two processes — truncate each other's staging file.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};

/// Read a `<digest> -> value` map from `path`, or an EMPTY map with a
/// disclosed reason.
///
/// `what` names the cache in those sentences ("description cache", "style
/// exemplar cache"), so each caller's degradation reads the way it always
/// did. Admission of individual ENTRIES is the caller's — this door only
/// decides whether the file as a whole can be believed.
pub fn read_map<V: serde::de::DeserializeOwned>(
    path: &Path,
    what: &str,
    max_bytes: usize,
) -> BTreeMap<String, V> {
    use std::io::Read as _;
    let empty = BTreeMap::new();
    if !path.exists() {
        return empty;
    }
    let text = match std::fs::File::open(path).and_then(|f| {
        let mut bytes = Vec::new();
        f.take((max_bytes + 1) as u64).read_to_end(&mut bytes)?;
        Ok(bytes)
    }) {
        Ok(bytes) if bytes.len() > max_bytes => {
            eprintln!(
                "  {what} {} exceeds the {max_bytes}-byte cap — rebuilding it",
                path.display()
            );
            return empty;
        }
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(t) => t,
            Err(_) => {
                eprintln!("  {what} {} is not UTF-8 — rebuilding it", path.display());
                return empty;
            }
        },
        Err(e) => {
            eprintln!("  {what} {} is unreadable ({e}) — rebuilding it", path.display());
            return empty;
        }
    };
    match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("  {what} {} is unusable ({e}) — rebuilding it", path.display());
            empty
        }
    }
}

/// Publish `chosen` over `path`, bounded and atomically.
///
/// WHICH entries survive is the caller's retention policy — the description
/// cache tops its keep-set back up to a cap, the exemplar cache prunes to it —
/// and that difference is the whole reason this takes an already-chosen map
/// rather than a cache and a keep-set.
pub fn publish_map<V: serde::Serialize>(
    path: &Path,
    what: &str,
    chosen: &BTreeMap<&String, &V>,
    max_bytes: usize,
) -> Result<()> {
    let text = serde_json::to_string(chosen).with_context(|| format!("serialise the {what}"))?;
    // Checked BEFORE anything is staged: the read door above refuses an
    // over-cap file, so publishing one would be a cache that disables itself
    // for good, and there is no reason to have written a byte of it.
    if text.len() > max_bytes {
        bail!(
            "refusing to write a {}-byte {what} over {} (cap {max_bytes} bytes)",
            text.len(),
            path.display()
        );
    }
    crate::pipeline::ensure_parent(path)?;
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "json.tmp{}-{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&tmp, text).with_context(|| format!("write {what} {}", tmp.display()))?;
    // A failed replace must not leak the staging file beside the live one.
    crate::store::durable_replace(&tmp, path)
        .inspect_err(|_| drop(std::fs::remove_file(&tmp)))
        .with_context(|| format!("publish {what} {}", path.display()))
}

/// A 64-hex lowercase key, i.e. something one of these caches could have
/// written. A foreign key is dropped at load rather than trusted as a digest —
/// these keys reach `Path::join`, and through the description cache they reach
/// a proposer prompt.
pub fn is_digest(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MUTATION: turn any one of the four degradations into a `panic!`/`?` and
    /// this fails. They are the shared half of "a cache never decides whether
    /// the user gets a library", and this is where that is now proven once.
    #[test]
    fn every_unreadable_cache_file_is_an_empty_cache_and_a_sentence() {
        let dir = crate::test_dir("content-cache-degrade");
        let path = dir.join("c.json");
        // Absent.
        assert!(read_map::<u32>(&path, "test cache", 1024).is_empty());
        // Not JSON.
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(read_map::<u32>(&path, "test cache", 1024).is_empty());
        // Not UTF-8.
        std::fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();
        assert!(read_map::<u32>(&path, "test cache", 1024).is_empty());
        // Over the cap.
        std::fs::write(&path, "x".repeat(64)).unwrap();
        assert!(read_map::<u32>(&path, "test cache", 8).is_empty());
        // …and a good one round-trips.
        let one = "a".repeat(64);
        let seven = 7u32;
        let chosen: BTreeMap<&String, &u32> = [(&one, &seven)].into_iter().collect();
        publish_map(&path, "test cache", &chosen, 1024).unwrap();
        assert_eq!(read_map::<u32>(&path, "test cache", 1024).get(&one), Some(&7));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// MUTATION: drop the byte cap in [`publish_map`] and this fails — the
    /// read side would then refuse the very file the write side produced, so
    /// the cache would silently disable itself for good.
    #[test]
    fn a_cache_is_never_published_larger_than_it_can_be_read_back() {
        let dir = crate::test_dir("content-cache-cap");
        let path = dir.join("c.json");
        let key = "b".repeat(64);
        let big = "x".repeat(500);
        let chosen: BTreeMap<&String, &String> = [(&key, &big)].into_iter().collect();
        let err = publish_map(&path, "test cache", &chosen, 100).expect_err("over cap");
        assert!(format!("{err:#}").contains("refusing to write"), "{err:#}");
        assert!(!path.exists(), "and nothing was left behind");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The key rule, in one place for both caches.
    #[test]
    fn only_a_64_hex_lowercase_key_is_a_digest() {
        assert!(is_digest(&"0123456789abcdef".repeat(4)));
        assert!(!is_digest(&"0123456789ABCDEF".repeat(4)), "upper case is not ours");
        assert!(!is_digest("../../etc/passwd"));
        assert!(!is_digest(&"a".repeat(63)));
        assert!(!is_digest(&"a".repeat(65)));
        assert!(!is_digest(&"g".repeat(64)));
    }
}
