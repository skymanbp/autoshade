//! The measured areas of a reference photograph, cached in the store per
//! library entry: `look-cache/<entry id>.json` under the store root, beside the
//! store's other derived caches. The library file itself is never changed.
//!
//! An entry's id is the hash of its staged frame and its stamp the file's size
//! and modified time; a cached measurement is used only when both still match.
//! Best effort both ways: a cache that cannot be read or written is measured
//! again next time.

use crate::advisor::look::{AreaLook, AreaName};
use crate::library::{Entry, Stamp};

/// Bumped when the measurement changes, so an older cache is measured again.
const VERSION: u32 = 1;

#[derive(serde::Serialize, serde::Deserialize)]
struct Cached {
    version: u32,
    stamp: Stamp,
    areas: Vec<(AreaName, AreaLook)>,
}

fn path_of(entry: &Entry) -> Option<std::path::PathBuf> {
    // The id names a file: only a plain hex-like token may (the library file
    // is data, and a crafted id must not reach outside the cache folder).
    let ok = !entry.id.is_empty() && entry.id.len() <= 128 && entry.id.chars().all(|c| c.is_ascii_alphanumeric());
    ok.then(|| crate::store::store_root().join("look-cache").join(format!("{}.json", entry.id)))
}

/// The cached areas of `entry`, when its measurement is current.
pub(super) fn load(entry: &Entry) -> Option<Vec<(AreaName, AreaLook)>> {
    let text = std::fs::read_to_string(path_of(entry)?).ok()?;
    let c: Cached = serde_json::from_str(&text).ok()?;
    (c.version == VERSION && c.stamp == entry.stamp && !c.areas.is_empty()).then_some(c.areas)
}

/// Keep `areas` as `entry`'s measurement (written to a temp name, then renamed).
pub(super) fn store(entry: &Entry, areas: &[(AreaName, AreaLook)]) {
    let Some(path) = path_of(entry) else { return };
    let Some(dir) = path.parent() else { return };
    let Ok(text) = serde_json::to_string(&Cached { version: VERSION, stamp: entry.stamp, areas: areas.to_vec() }) else {
        return;
    };
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    // Best effort: a failed write only means the reference is measured again.
    if std::fs::create_dir_all(dir).is_ok() && std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}
