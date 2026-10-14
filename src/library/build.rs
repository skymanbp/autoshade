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
/// photo), then one sidecar process each for image vectors and look phrases
/// over every photo that needs it, then — with a RAW folder — the sidecar
/// settings check (this process, per photo with a candidate RAW).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStage {
    Frames,
    Images,
    Phrases,
    Settings,
}

/// What a UI calls each [`BuildStage`], in English (the GUI translates it).
const STAGE_LABELS: [&str; 4] = ["decoding", "image vectors", "look phrases", "matching settings"];

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
    raw_root: Option<&Path>,
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
    let complete = |e: &&Entry| !describe || e.phrases.is_some();
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
            Err(e) => eprintln!("  reference library: look phrases unavailable ({e:#}) — the references go as images only"),
        }
        report(BuildStage::Phrases, total);
    }

    let mut entries: Vec<Entry> = slots.into_iter().map(|s| s.entry).collect();
    if let Some(root) = raw_root {
        attach_settings(root, &mut entries, &|done| on_progress(BuildProgress { stage: BuildStage::Settings, done, total }))?;
    }
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
    let entry = Entry { id, path, stamp, image: Vec::new(), phrases: None, settings: None, settings_checked: None };
    Ok(Slot { entry, frame: Some(frame) })
}

/// The sidecar settings check over every entry whose candidates changed since
/// it was last checked ([`super::settings`]). Per entry the closest candidate
/// under [`super::MATCH_DE_MAX`] wins; none, and the entry carries none. A
/// photo that fails to decode or develop is stepped over with a line — the
/// settings are an enrichment, never a reason to fail the build.
fn attach_settings(root: &Path, entries: &mut [Entry], report: &dyn Fn(usize)) -> Result<()> {
    let index = super::settings::raw_index(root)?;
    let pairing = crate::xmp_pair::XmpPairing::new(root, None);
    report(0);
    let (mut matched, mut rejected) = (0usize, 0usize);
    for (i, e) in entries.iter_mut().enumerate() {
        let finished = Path::new(&e.path).to_path_buf();
        let cands = super::settings::candidates(&finished, &index);
        let mut sig = String::new();
        for raw in &cands {
            if let Some(x) = pairing.find(raw) {
                let st = Stamp::of(&x).map(|s| format!("{}:{}", s.len, s.modified)).unwrap_or_default();
                sig.push_str(&format!("{}|{st};", x.display()));
            }
        }
        if e.settings_checked.as_deref() == Some(sig.as_str()) {
            report(i + 1);
            continue;
        }
        e.settings = None;
        if !sig.is_empty() {
            match best_settings(root, &finished, &cands) {
                Ok(best) => {
                    if best.is_some() { matched += 1 } else { rejected += 1 }
                    e.settings = best;
                }
                Err(err) => eprintln!("  reference library: settings check failed for one photo ({err:#})"),
            }
        }
        e.settings_checked = Some(sig);
        report(i + 1);
    }
    if matched + rejected > 0 {
        println!("  reference library: {matched} photo(s) matched their RAW's settings, {rejected} did not (the sidecar develops to a different picture)");
    }
    Ok(())
}

/// Every candidate tried; the closest under the line wins.
fn best_settings(root: &Path, finished: &Path, cands: &[&Path]) -> Result<Option<super::Settings>> {
    let frame = {
        let _permit = crate::decode::DecodePermit::acquire();
        let d = crate::decode::decode_any(finished).with_context(|| format!("decode {}", finished.display()))?;
        d.preview.resize(frames::FRAME_EDGE, frames::FRAME_EDGE, image::imageops::FilterType::Triangle)
    };
    let mut best: Option<super::Settings> = None;
    for raw in cands {
        // No permit here: the develop inside takes the decoder's own.
        let Some(s) = super::settings::try_candidate(raw, root, &frame).with_context(|| format!("develop {}", raw.display()))? else { continue };
        if s.match_de <= super::MATCH_DE_MAX && best.as_ref().is_none_or(|b| s.match_de < b.match_de) {
            best = Some(s);
        }
    }
    Ok(best)
}
