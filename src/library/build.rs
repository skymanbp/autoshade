//! Building a library from a folder of finished photos.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::frames::{self, Scratch};
use super::switches::model_stamp;
use super::{DescribeSwitch, Entry, Library, MAX_ENTRIES, Stamp, VERSION};

/// The two model sidecars a build runs, and the directory it may write its
/// intermediates in — passed in, never read from a global, so a test's build
/// cannot write into the user's store.
#[derive(Clone)]
pub struct Sidecars {
    pub embed: crate::embed::EmbedOpts,
    pub describe: crate::describe::DescribeOpts,
    pub scratch: PathBuf,
}

impl Sidecars {
    /// Both sidecars from [`crate::config::Config`], the store as scratch.
    pub fn from_config(cfg: &crate::config::Config) -> Self {
        Sidecars {
            embed: crate::embed::EmbedOpts::from_config(cfg),
            describe: crate::describe::DescribeOpts::from_config(cfg),
            scratch: crate::store::store_root(),
        }
    }
}

/// The phases of a build, in order: decode-and-stage (this process, per
/// photo), then one sidecar process each for image vectors, look phrases and
/// phrase vectors over every photo that needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStage {
    Frames,
    Images,
    Phrases,
    Text,
}

/// What a UI calls each [`BuildStage`], in English (the GUI translates it).
const STAGE_LABELS: [&str; 4] = ["decoding", "image vectors", "look phrases", "phrase vectors"];

impl BuildStage {
    pub fn label(self) -> &'static str {
        STAGE_LABELS[self as usize]
    }
}

/// One progress report: which phase, how far into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildProgress {
    pub stage: BuildStage,
    pub done: usize,
    pub total: usize,
}

/// One photo's place in a build: the entry so far, and its staged frame when
/// a model still has to see it.
struct Slot {
    entry: Entry,
    frame: Option<Scratch>,
}

/// Build the library for `dir`, keeping what `previous` already measured.
///
/// An entry is kept whole when its file is unchanged (same path and stamp) or
/// its pixels were measured under another name (same frame digest) — and the
/// phrases it needs are there. Everything else is decoded once and measured
/// in one sidecar call per model. A photo the image model could not answer is
/// left out with a line; a build with no entries at all is an error.
pub fn build(
    dir: &Path,
    sidecars: &Sidecars,
    previous: Option<&Library>,
    describe: DescribeSwitch,
    on_progress: &dyn Fn(BuildProgress),
) -> Result<Library> {
    anyhow::ensure!(
        sidecars.embed.available(),
        "the reference library needs the image-embedding sidecar ({}), which is not there",
        sidecars.embed.script.display()
    );
    let photos = frames::finished_photos(dir)?;
    anyhow::ensure!(!photos.is_empty(), "{} holds no finished photos", dir.display());
    anyhow::ensure!(
        photos.len() <= MAX_ENTRIES,
        "{} holds {} finished photos and a reference library holds at most {MAX_ENTRIES} — \
         point it at a chosen folder of looks, not a whole archive",
        dir.display(),
        photos.len()
    );
    let describe = describe.on() && sidecars.describe.available();
    let model = model_stamp();
    let kept: Vec<&Entry> = previous.filter(|p| p.model == model).map(|p| p.entries.iter().collect()).unwrap_or_default();
    let complete = |e: &&Entry| !describe || e.phrases_vec.is_some();
    let by_path: HashMap<&str, &Entry> = kept.iter().copied().filter(complete).map(|e| (e.path.as_str(), e)).collect();
    let by_id: HashMap<&str, &Entry> = kept.iter().copied().filter(complete).map(|e| (e.id.as_str(), e)).collect();

    std::fs::create_dir_all(&sidecars.scratch)?;
    frames::sweep(&sidecars.scratch);
    let total = photos.len();
    let report = |stage, done| on_progress(BuildProgress { stage, done, total });
    report(BuildStage::Frames, 0);
    let mut slots = Vec::with_capacity(total);
    for (i, photo) in photos.iter().enumerate() {
        slots.push(slot_for(photo, &sidecars.scratch, &by_path, &by_id)?);
        report(BuildStage::Frames, i + 1);
    }

    report(BuildStage::Images, 0);
    let need: Vec<&Scratch> = slots.iter().filter(|s| s.entry.image.is_empty()).filter_map(|s| s.frame.as_ref()).collect();
    let vectors = frames::image_vectors(&sidecars.embed, &sidecars.scratch, &need)?;
    for s in &mut slots {
        if let Some(v) = s.frame.as_ref().and_then(|f| vectors.get(&f.key())) {
            s.entry.image = v.clone();
        }
    }
    slots.retain(|s| !s.entry.image.is_empty());
    report(BuildStage::Images, total);

    if describe {
        report(BuildStage::Phrases, 0);
        let need: Vec<&Scratch> = slots.iter().filter(|s| s.entry.phrases.is_none()).filter_map(|s| s.frame.as_ref()).collect();
        match frames::look_phrases(&sidecars.describe, &sidecars.scratch, &need) {
            Ok(phrases) => {
                for s in &mut slots {
                    if let Some(p) = s.frame.as_ref().and_then(|f| phrases.get(&f.key())) {
                        s.entry.phrases = Some(p.clone());
                    }
                }
            }
            Err(e) => eprintln!("  reference library: look phrases unavailable ({e:#}) — searches ask the scene question only"),
        }
        report(BuildStage::Phrases, total);
        report(BuildStage::Text, 0);
        attach_phrase_vectors(&sidecars.embed, &sidecars.scratch, &mut slots);
        report(BuildStage::Text, total);
    }

    let entries: Vec<Entry> = slots.into_iter().map(|s| s.entry).collect();
    anyhow::ensure!(!entries.is_empty(), "no photo in {} got an image vector", dir.display());
    let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf()).display().to_string();
    Ok(Library { version: VERSION, dir, model, entries })
}

/// Reuse an entry for `photo` if one fits, otherwise decode and stage it.
fn slot_for(photo: &Path, scratch: &Path, by_path: &HashMap<&str, &Entry>, by_id: &HashMap<&str, &Entry>) -> Result<Slot> {
    let path = std::path::absolute(photo)?.display().to_string();
    let stamp = Stamp::of(photo)?;
    if let Some(e) = by_path.get(path.as_str()).filter(|e| e.stamp == stamp) {
        return Ok(Slot { entry: (*e).clone(), frame: None });
    }
    let decoded = {
        let _permit = crate::decode::DecodePermit::acquire();
        crate::decode::decode_any(photo).with_context(|| format!("decode {}", photo.display()))?
    };
    let frame = frames::stage(&decoded.preview, scratch, "frame")?;
    drop(decoded);
    let id = crate::describe::frame_digest(frame.path())?;
    if let Some(e) = by_id.get(id.as_str()) {
        let entry = Entry { path, stamp, ..(*e).clone() };
        return Ok(Slot { entry, frame: None });
    }
    let entry = Entry { id, path, stamp, image: Vec::new(), phrases: None, phrases_vec: None };
    Ok(Slot { entry, frame: Some(frame) })
}

/// The text vector of every entry's phrases that lacks one, in one call. A
/// failure leaves those entries without the prompt question, with a line.
fn attach_phrase_vectors(embed: &crate::embed::EmbedOpts, scratch: &Path, slots: &mut [Slot]) {
    let todo: Vec<usize> = (0..slots.len())
        .filter(|&i| slots[i].entry.phrases.is_some() && slots[i].entry.phrases_vec.is_none())
        .collect();
    let texts: Vec<&str> = todo.iter().filter_map(|&i| slots[i].entry.phrases.as_deref()).collect();
    match frames::text_vectors(embed, scratch, &texts) {
        Ok(vectors) => {
            for (i, v) in todo.into_iter().zip(vectors) {
                slots[i].entry.phrases_vec = Some(v);
            }
        }
        Err(e) => eprintln!("  reference library: phrase vectors unavailable ({e:#}) — searches ask the scene question only"),
    }
}
