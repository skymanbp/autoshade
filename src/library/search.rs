//! Searching the library: two questions per entry, put on one scale.

use std::path::Path;

use super::{Entry, Library};

/// How many finished photos one develop leans on.
pub const REFERENCE_COUNT: usize = 3;

/// How much more the prompt question counts than the scene question when the
/// photographer typed a direction: what they asked for leads, the scene
/// breaks ties between looks that answer it equally well.
const PROMPT_WEIGHT: f64 = 2.0;

/// What a search asks with.
#[derive(Debug, Clone, Copy)]
pub struct Query<'a> {
    /// The develop's image vector.
    pub image: &'a [f32],
    /// The direction's text vector, when one was typed.
    pub prompt: Option<&'a [f32]>,
    /// A photo the search must not return (the one being developed, when it
    /// is itself in the library).
    pub exclude: Option<&'a Path>,
}

/// One entry a search returned, with what it scored.
#[derive(Debug, Clone, Copy)]
pub struct Hit<'a> {
    pub entry: &'a Entry,
    /// Cosine similarity of the two image vectors (1 = same frame).
    pub scene: f64,
    /// Cosine similarity of the prompt to the entry's phrases, when both exist.
    pub prompt: Option<f64>,
    /// The combined, standardised distance the ranking used (lower is closer).
    pub score: f64,
}

fn cosine(a: &[f32], b: &[f32]) -> Option<f64> {
    (a.len() == b.len() && !a.is_empty()).then(|| a.iter().zip(b).map(|(&x, &y)| f64::from(x) * f64::from(y)).sum())
}

/// Each value's distance from the mean of the present ones, in standard
/// deviations, so two questions measured on different spreads add fairly. An
/// absent value sits at the mean — it neither helps nor hurts.
fn standardised(values: &[Option<f64>]) -> Vec<f64> {
    let present: Vec<f64> = values.iter().flatten().copied().collect();
    if present.is_empty() {
        return vec![0.0; values.len()];
    }
    let n = present.len() as f64;
    let mean = present.iter().sum::<f64>() / n;
    let sd = (present.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n).sqrt().max(1e-9);
    values.iter().map(|v| v.map_or(0.0, |v| (v - mean) / sd)).collect()
}

fn same_file(a: &str, b: &Path) -> bool {
    let b = std::path::absolute(b).unwrap_or_else(|_| b.to_path_buf()).display().to_string();
    if cfg!(windows) { a.eq_ignore_ascii_case(&b) } else { a == b }
}

impl Library {
    /// The `k` entries closest to `q`, closest first.
    ///
    /// Scene: image against image. Prompt: the direction's text vector
    /// against the entry's phrase vector. Each is standardised over the
    /// candidates, then summed with the prompt at [`PROMPT_WEIGHT`]; without
    /// a prompt the scene alone ranks. Ties keep library order, so a search
    /// is deterministic.
    pub fn search<'a>(&'a self, q: &Query<'_>, k: usize) -> Vec<Hit<'a>> {
        let candidates: Vec<&Entry> =
            self.entries.iter().filter(|e| q.exclude.is_none_or(|x| !same_file(&e.path, x))).collect();
        let scene: Vec<Option<f64>> = candidates.iter().map(|e| cosine(q.image, &e.image)).collect();
        let prompt: Vec<Option<f64>> = candidates
            .iter()
            .map(|e| q.prompt.zip(e.phrases_vec.as_deref()).and_then(|(p, v)| cosine(p, v)))
            .collect();
        // Similarities rank high-is-close; distances are their negatives.
        let neg = |v: &[Option<f64>]| v.iter().map(|x| x.map(|s| -s)).collect::<Vec<_>>();
        let zs = standardised(&neg(&scene));
        let zp = standardised(&neg(&prompt));
        let weight = if q.prompt.is_some() { PROMPT_WEIGHT } else { 0.0 };
        let mut hits: Vec<Hit<'a>> = candidates
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                Some(Hit { entry: e, scene: scene[i]?, prompt: prompt[i], score: zs[i] + weight * zp[i] })
            })
            .collect();
        hits.sort_by(|a, b| a.score.total_cmp(&b.score));
        hits.truncate(k);
        hits
    }
}
