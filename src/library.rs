//! The REFERENCE LIBRARY: finished photographs whose look a develop may follow
//! (2026-10-13 redesign, user ruling — "只使用成片是对的").
//!
//! One kind of entry, one file, one question. An entry is a finished photo
//! and two things measured from it once:
//!
//! * an IMAGE vector — SigLIP 2 over the 512-px frame: what the photo looks
//!   like, scene and grade together;
//! * a few short LOOK PHRASES — Qwen3-VL writing the grade the way a
//!   photographer types a request ("warm, faded, low contrast"), the words
//!   the advisor reads beside each reference image.
//!
//! A search asks one thing of every entry ([`search`]): is its scene like
//! this photo. The best few go to the advisor as images and as words. A
//! develop is guided EITHER by the library OR by a typed direction, never
//! both (user ruling 2026-10-14, [`crate::pipeline::Guide`]), so no typed
//! text ever meets the library.
//!
//! The library is pixels in, look out. A sidecar `.xmp` is never TRUSTED for
//! being there — a finished photo may have had work no `.xmp` records (a
//! Photoshop pass), and an `.xmp` may be stale or one of several — but when
//! the photographer names their RAW folder, a sidecar that develops to the
//! finished photo is attached as that reference's settings ([`settings`]).
//!
//! The file IS the cache: a rebuild keeps every entry whose file is unchanged
//! (same path, size and modified time) and every entry whose pixels it has
//! already measured (same frame digest, a moved or renamed file), so adding a
//! photo to the folder measures one photo.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

mod build;
mod frames;
mod search;
mod settings;
mod switches;
mod voice;

pub use build::{BuildProgress, BuildStage, Sidecars, build};
pub use frames::query_vector;
pub use search::{Hit, Query, REFERENCE_COUNT};
pub use settings::{MATCH_DE_MAX, MAX_SETTINGS_CHARS, Settings, settings_line};
pub use switches::{DescribeSwitch, EmbeddingSwitch};
pub use voice::{STYLE_TARGET_MIN, StyleVoice, look_summary, reference_block};

/// The library file's format. Bumped only when an entry's fields change
/// meaning; a library of another version is rebuilt, never reinterpreted.
pub const VERSION: u32 = 1;

/// Where the library lives in the per-user store.
pub const FILE_NAME: &str = "reference-library.json";

/// The most finished photos one library holds. A search is a linear pass
/// over one 768-float dot product per entry, so the cap is about the file
/// (≈ 16 KB of JSON per entry), not the search.
pub const MAX_ENTRIES: usize = 3_000;

/// The file cap the loader enforces before it parses anything: the entry cap
/// at a generous per-entry size, so a valid library always fits and a
/// runaway file is refused unread.
pub const MAX_FILE_BYTES: usize = MAX_ENTRIES * 24 * 1024;

/// The longest look-phrase list an entry may carry, in characters — the
/// describe sidecar's own cap, restated at the door because the file is data.
pub const MAX_PHRASE_CHARS: usize = crate::describe::MAX_DESC_CHARS;

/// The library: the folder it was built from, the model that measured it, and
/// its entries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Library {
    pub version: u32,
    /// Absolute folder the entries were found under (shown in the UI).
    pub dir: String,
    /// What measured the vectors ([`switches::model_stamp`]). Vectors from
    /// another model are not comparable, so a rebuild under a different stamp
    /// keeps nothing.
    pub model: String,
    pub entries: Vec<Entry>,
}

/// One finished photo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// SHA-256 of the staged 512-px frame — the entry's identity, which a
    /// move or a rename does not change.
    pub id: String,
    /// Absolute path of the finished photo.
    pub path: String,
    /// The file as it was when measured, so an unchanged file is never
    /// decoded again.
    pub stamp: Stamp,
    /// SigLIP 2 image vector, 768 floats, unit length.
    pub image: Vec<f32>,
    /// The look in a few comma-separated phrases, when the describe model ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phrases: Option<String>,
    /// The settings of the RAW sidecar that develops to this photo, when one
    /// was found and matched ([`settings`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Settings>,
    /// What the last sidecar check looked at — each candidate sidecar's path
    /// and stamp — so a rebuild checks again only when that changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_checked: Option<String>,
}

/// A file's size and modified time, the cheap identity a rebuild checks
/// before it decodes anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub len: u64,
    /// Modified time in whole seconds since the Unix epoch (0 when the
    /// filesystem gives none, which never matches a real file again).
    pub modified: u64,
}

impl Stamp {
    pub fn of(path: &Path) -> Result<Self> {
        let meta = std::fs::metadata(path).with_context(|| format!("read {}", path.display()))?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        Ok(Stamp { len: meta.len(), modified })
    }
}

/// The library file in the per-user store.
pub fn path() -> PathBuf {
    crate::store::store_root().join(FILE_NAME)
}

/// What [`load`] found.
#[derive(Debug)]
pub enum Loaded {
    /// No library has been built.
    Absent,
    /// A file is there and cannot be used; the reason is for the user.
    Unusable(String),
    Ready(Library),
}

/// The library at [`path`].
pub fn load() -> Loaded {
    load_from(&path())
}

/// What a status line shows about the library, read off the file once —
/// never per frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Summary {
    Absent,
    /// A file is there and cannot be used; the reason is for the user.
    Unusable(String),
    Built {
        photos: usize,
        /// How many carry look phrases (the optional describe pass).
        phrased: usize,
        /// How many carry matched sidecar settings.
        with_settings: usize,
        /// The folder it was built from.
        dir: String,
        /// Time since the file was last written, when the filesystem says.
        age: Option<std::time::Duration>,
    },
}

/// [`Summary`] of the library at [`path`].
pub fn summary() -> Summary {
    summary_of(&path())
}

/// [`summary`] against an explicit file.
pub fn summary_of(file: &Path) -> Summary {
    match load_from(file) {
        Loaded::Absent => Summary::Absent,
        Loaded::Unusable(why) => Summary::Unusable(why),
        Loaded::Ready(lib) => Summary::Built {
            photos: lib.entries.len(),
            phrased: lib.entries.iter().filter(|e| e.phrases.is_some()).count(),
            with_settings: lib.entries.iter().filter(|e| e.settings.is_some()).count(),
            dir: lib.dir,
            age: std::fs::metadata(file).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()),
        },
    }
}

/// Build the library from `dir` into [`path`], keeping everything the current
/// file already measured — the one call every surface makes, so none of them
/// re-implements "load the old one, build, save".
pub fn rebuild(
    dir: &Path,
    sidecars: &Sidecars,
    describe: DescribeSwitch,
    raw_root: Option<&Path>,
    on_progress: &dyn Fn(BuildProgress),
) -> Result<Library> {
    let previous = match load() {
        Loaded::Ready(lib) => Some(lib),
        _ => None,
    };
    let lib = build(dir, sidecars, previous.as_ref(), describe, raw_root, on_progress)?;
    lib.save(&path())?;
    Ok(lib)
}

/// [`load`] against an explicit file — the seam tests drive.
pub fn load_from(file: &Path) -> Loaded {
    if !file.exists() {
        return Loaded::Absent;
    }
    match Library::read(file) {
        Ok(lib) => Loaded::Ready(lib),
        Err(e) => Loaded::Unusable(format!("{e:#}")),
    }
}

impl Library {
    /// Read and check a library file. Every entry is checked at the door —
    /// the file is data on disk, and one non-finite or wrong-width vector
    /// would turn a search into an unspecified ranking.
    pub fn read(file: &Path) -> Result<Self> {
        use std::io::Read as _;
        let mut bytes = Vec::new();
        std::fs::File::open(file)
            .and_then(|f| f.take(MAX_FILE_BYTES as u64 + 1).read_to_end(&mut bytes))
            .with_context(|| format!("read reference library {}", file.display()))?;
        anyhow::ensure!(
            bytes.len() <= MAX_FILE_BYTES,
            "reference library {} exceeds the {MAX_FILE_BYTES}-byte limit",
            file.display()
        );
        let lib: Library = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse reference library {}", file.display()))?;
        lib.check().with_context(|| format!("reference library {}", file.display()))?;
        Ok(lib)
    }

    fn check(&self) -> Result<()> {
        anyhow::ensure!(
            self.version == VERSION,
            "it is format v{}, this build reads v{VERSION} — rebuild it",
            self.version
        );
        anyhow::ensure!(
            self.entries.len() <= MAX_ENTRIES,
            "it holds {} entries, the cap is {MAX_ENTRIES}",
            self.entries.len()
        );
        for e in &self.entries {
            anyhow::ensure!(is_unit_vector(&e.image), "an entry ({}) has no usable image vector", e.path);
            if let Some(p) = &e.phrases {
                anyhow::ensure!(
                    p.chars().count() <= MAX_PHRASE_CHARS,
                    "an entry ({}) carries over {MAX_PHRASE_CHARS} characters of phrases",
                    e.path
                );
            }
            if let Some(s) = &e.settings {
                anyhow::ensure!(
                    s.line.chars().count() <= MAX_SETTINGS_CHARS && s.match_de.is_finite(),
                    "an entry ({}) carries unusable settings",
                    e.path
                );
            }
        }
        Ok(())
    }

    /// Publish the library atomically. An empty library is a failed build,
    /// never a result: writing it would replace a good one with nothing.
    pub fn save(&self, file: &Path) -> Result<()> {
        anyhow::ensure!(
            !self.entries.is_empty(),
            "refusing to save an empty reference library over {}",
            file.display()
        );
        self.check()?;
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_vec(self)?;
        crate::store::durable_write(file, &text)
            .with_context(|| format!("write reference library {}", file.display()))
    }
}

/// A vector this build can compare: the model's width, finite, unit length
/// within the slack a JSON round trip leaves.
fn is_unit_vector(v: &[f32]) -> bool {
    if v.len() != crate::embed::EMBED_DIM || !v.iter().all(|x| x.is_finite()) {
        return false;
    }
    let norm: f64 = v.iter().map(|&x| f64::from(x) * f64::from(x)).sum::<f64>().sqrt();
    (norm - 1.0).abs() < 1e-3
}

#[cfg(test)]
mod tests;
