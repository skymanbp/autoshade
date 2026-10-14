//! The library's file work and its three model calls: walking a folder,
//! staging the 512-px frame both a build and a query embed, and ONE sidecar
//! process per model per build.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};

/// Long edge a photo is reduced to before a model sees it — the ONE frame a
/// build and a query both go through, so a stored vector and a query vector
/// can only differ by what the photos show.
pub(super) const FRAME_EDGE: u32 = 512;

/// The scratch-file prefix every intermediate here carries.
const PREFIX: &str = "autoshade-library-";

/// Intermediates older than this belong to a build that never unwound (a
/// killed process) and are swept by the next build. A day: longer than any
/// real build, and the cost of waiting is a few dead PNGs.
const STALE_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// A unique scratch name: pid + sequence, so two builds or two develops in
/// one process (the web server's threads, the batch pool) never share a file.
fn scratch_name(tag: &str, ext: &str) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!("{PREFIX}{}-{}-{tag}.{ext}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed))
}

/// A scratch file removed when dropped — every early return out of a build
/// or a query cleans up after itself.
pub(super) struct Scratch(PathBuf);

impl Scratch {
    fn new(dir: &Path, tag: &str, ext: &str) -> Self {
        Scratch(dir.join(scratch_name(tag, ext)))
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }

    /// The path as the text a JSONL manifest carries; answers are mapped back
    /// by this exact string.
    pub(super) fn key(&self) -> String {
        self.0.display().to_string()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Reduce a preview to THE frame and write it. `Triangle` is named, not
/// defaulted: a filter that changed under us would move every vector.
pub(super) fn stage(preview: &image::DynamicImage, dir: &Path, tag: &str) -> Result<Scratch> {
    std::fs::create_dir_all(dir)?;
    let frame = Scratch::new(dir, tag, "png");
    preview
        .resize(FRAME_EDGE, FRAME_EDGE, image::imageops::FilterType::Triangle)
        .to_rgb8()
        .save(frame.path())
        .with_context(|| format!("stage frame {}", frame.path().display()))?;
    Ok(frame)
}

/// Every finished photo under `root`, sorted. Each directory is entered once
/// by its canonical identity, so a link back into an ancestor cannot loop; a
/// link whose target is gone is stepped over with one line.
pub(super) fn finished_photos(root: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let first_visit = std::fs::canonicalize(&dir).map_or(true, |c| seen.insert(c));
        if !first_visit {
            continue;
        }
        let listing = std::fs::read_dir(&dir).with_context(|| format!("scan {}", dir.display()))?;
        for item in listing {
            let p = item?.path();
            if p.is_dir() {
                pending.push(p);
            } else if !p.exists() {
                eprintln!("⚠ {} points nowhere (a link whose target is missing) — skipped", p.display());
            } else if is_finished_photo(&p) {
                found.push(p);
            }
        }
    }
    found.sort();
    Ok(found)
}

fn is_finished_photo(p: &Path) -> bool {
    p.extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| crate::pipeline::BAKED_EXTS.iter().any(|e| x.eq_ignore_ascii_case(e)))
}

/// Delete intermediates an interrupted build left — never this process's
/// own — and say how many.
pub(super) fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mine = format!("{PREFIX}{}-", std::process::id());
    let removed = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with(PREFIX) && !name.starts_with(&mine)
        })
        .filter(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|age| age >= STALE_AGE))
        })
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count();
    if removed > 0 {
        println!("  reference library: removed {removed} staged file(s) left by an interrupted build");
    }
}

/// A JSONL manifest for the length of one sidecar call.
fn manifest(dir: &Path, tag: &str, lines: impl Iterator<Item = serde_json::Value>) -> Result<Scratch> {
    let m = Scratch::new(dir, tag, "jsonl");
    let body: String = lines.map(|v| v.to_string() + "\n").collect();
    std::fs::write(m.path(), body).with_context(|| format!("write manifest {}", m.path().display()))?;
    Ok(m)
}

fn frame_lines<'a>(frames: &'a [&Scratch]) -> impl Iterator<Item = serde_json::Value> + 'a {
    frames.iter().map(|f| serde_json::json!({ "path": f.key() }))
}

/// One answer per frame from a batch sidecar, or a line saying why not.
fn keep_answer<T>(out: &mut std::collections::HashMap<String, T>, path: String, value: Option<T>, why: Option<&str>, what: &str) {
    match value {
        Some(v) => {
            out.insert(path, v);
        }
        None => eprintln!(
            "  reference library: no {what} for one photo ({})",
            why.unwrap_or("the sidecar gave no reason")
        ),
    }
}

/// ONE SigLIP image call over `frames`: the image vector of each, keyed by
/// frame. Mapped back by path, never by position — the sidecar reports a bad
/// line as soon as it reads it.
pub(super) fn image_vectors(
    opts: &crate::embed::EmbedOpts,
    dir: &Path,
    frames: &[&Scratch],
) -> Result<std::collections::HashMap<String, Vec<f32>>> {
    let mut out = std::collections::HashMap::new();
    if frames.is_empty() {
        return Ok(out);
    }
    let m = manifest(dir, "images", frame_lines(frames))?;
    let answer = Scratch::new(dir, "images-out", "jsonl");
    for rec in crate::embed::embed_image_batch(opts, m.path(), answer.path())? {
        keep_answer(&mut out, rec.path, rec.record.map(|r| r.vector), rec.error.as_deref(), "image vector");
    }
    Ok(out)
}

/// ONE Qwen call over `frames`: the look phrases of each, keyed by frame.
pub(super) fn look_phrases(
    opts: &crate::describe::DescribeOpts,
    dir: &Path,
    frames: &[&Scratch],
) -> Result<std::collections::HashMap<String, String>> {
    let mut out = std::collections::HashMap::new();
    if frames.is_empty() {
        return Ok(out);
    }
    let m = manifest(dir, "phrases", frame_lines(frames))?;
    let answer = Scratch::new(dir, "phrases-out", "jsonl");
    for rec in crate::describe::describe_manifest(opts, m.path(), answer.path())? {
        keep_answer(&mut out, rec.path, rec.desc, rec.error.as_deref(), "look phrases");
    }
    Ok(out)
}

/// ONE SigLIP text call over `texts`, answered in order.
pub(super) fn text_vectors(opts: &crate::embed::EmbedOpts, dir: &Path, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let m = manifest(dir, "texts", texts.iter().map(|t| serde_json::json!({ "text": t })))?;
    let answer = Scratch::new(dir, "texts-out", "json");
    crate::embed::embed_text_batch(opts, m.path(), answer.path(), texts.len())
}

/// What a develop asks the library with: the photo's image vector and, when
/// the photographer typed a direction, its text vector.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueryVectors {
    pub image: Vec<f32>,
    pub prompt: Option<Vec<f32>>,
}

/// Embed a develop's preview (and its direction, if any) in ONE sidecar call,
/// through the same frame a build stages.
pub fn query_vectors(
    opts: &crate::embed::EmbedOpts,
    preview: &image::DynamicImage,
    direction: Option<&str>,
    dir: &Path,
) -> Result<QueryVectors> {
    let frame = stage(preview, dir, "query")?;
    let prompt = match direction.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => {
            let p = Scratch::new(dir, "prompt", "txt");
            std::fs::write(p.path(), d)?;
            Some(p)
        }
        None => None,
    };
    let opts = crate::embed::EmbedOpts { text_file: prompt.as_ref().map(|p| p.path().to_path_buf()), ..opts.clone() };
    let answer = Scratch::new(dir, "query-out", "json");
    let record = crate::embed::embed_file_record(&opts, frame.path(), answer.path())?;
    Ok(QueryVectors { image: record.vector, prompt: record.text_vector })
}
