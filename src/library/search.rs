//! Searching the library: which finished photos have a scene like this one.

use std::path::Path;

use super::{Entry, Library};

/// How many finished photos one develop leans on.
pub const REFERENCE_COUNT: usize = 3;

/// Two entries whose image vectors are at least this similar are versions of
/// ONE shot (an export and its re-edit, a noise-reduced copy), and take one
/// reference slot between them (user ruling 2026-10-14). Measured on the
/// photographer's 94-photo album: the three version pairs read 0.985, 0.991
/// and 1.000, the closest two DIFFERENT frames (one sunset, two exposures
/// minutes apart) 0.947; the line sits in that gap.
const SAME_SHOT: f64 = 0.97;

/// What a search asks with.
#[derive(Debug, Clone, Copy)]
pub struct Query<'a> {
    /// The develop's image vector.
    pub image: &'a [f32],
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
}

fn cosine(a: &[f32], b: &[f32]) -> Option<f64> {
    (a.len() == b.len() && !a.is_empty()).then(|| a.iter().zip(b).map(|(&x, &y)| f64::from(x) * f64::from(y)).sum())
}

fn same_file(a: &str, b: &Path) -> bool {
    let b = std::path::absolute(b).unwrap_or_else(|_| b.to_path_buf()).display().to_string();
    if cfg!(windows) { a.eq_ignore_ascii_case(&b) } else { a == b }
}

impl Library {
    /// The `k` entries whose image is closest to `q`'s, closest first.
    ///
    /// Only the scene is asked: a develop guided by a typed direction does not
    /// search the library at all (user ruling 2026-10-14 — library and
    /// direction are either/or). Ties keep library order, so a search is
    /// deterministic. Versions of one shot ([`SAME_SHOT`]) count once: the
    /// best-ranked stands for them and the next different photo takes the
    /// freed slot. The develop's own finished version is NOT dropped by this —
    /// it is compared with the other references, not with the query, so it
    /// leads when it is the closest (user ruling 2026-10-14).
    pub fn search<'a>(&'a self, q: &Query<'_>, k: usize) -> Vec<Hit<'a>> {
        let mut hits: Vec<Hit<'a>> = self
            .entries
            .iter()
            .filter(|e| q.exclude.is_none_or(|x| !same_file(&e.path, x)))
            .filter_map(|e| Some(Hit { entry: e, scene: cosine(q.image, &e.image)? }))
            .collect();
        hits.sort_by(|a, b| b.scene.total_cmp(&a.scene));
        let mut kept: Vec<Hit<'a>> = Vec::with_capacity(k);
        for h in hits {
            if kept.len() == k {
                break;
            }
            let another_version =
                kept.iter().any(|t| cosine(&t.entry.image, &h.entry.image).is_some_and(|c| c >= SAME_SHOT));
            if !another_version {
                kept.push(h);
            }
        }
        kept
    }
}
