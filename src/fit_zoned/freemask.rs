use image::{GrayImage, Luma};

use super::*;
use crate::fit_field::LocalField;

pub(super) const FREE_MASK_MAX_ATTACHMENTS: usize = 2;
const FREE_MASK_MIN_PIXELS: usize = 64;

#[cfg(test)]
thread_local! {
    pub(super) static FREE_MASK_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(super) static FREE_MASK_BYPASS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub(super) static FREE_MASK_FORCE_ZONE_REFUSAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

thread_local! {
    pub(super) static FREE_MASK_LAST_STAGE: std::cell::RefCell<Option<FreeMaskStage>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn record_stage(stage: FreeMaskStage) {
    FREE_MASK_LAST_STAGE.with(|last| *last.borrow_mut() = Some(stage));
}

#[cfg(not(test))]
pub(super) fn record_stage(_stage: FreeMaskStage) {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FreeMaskWhy {
    NoCandidates,
    Share,
    Divergence,
    Footprint,
    Mass,
    Cap,
    RasterClaim,
    RasterWrite,
    ZoneRefused,
    Frame,
    Rim,
    Unmeasured,
    Inert,
}

impl FreeMaskWhy {
    fn label(self) -> &'static str {
        match self {
            Self::NoCandidates => "no-candidates",
            Self::Share => "share",
            Self::Divergence => "divergence",
            Self::Footprint => "footprint",
            Self::Mass => "mass",
            Self::Cap => "cap",
            Self::RasterClaim => "raster-claim",
            Self::RasterWrite => "raster-write",
            Self::ZoneRefused => "zone-refused",
            Self::Frame => "frame",
            Self::Rim => "boundary-step",
            Self::Unmeasured => "boundary-unmeasurable",
            Self::Inert => "boundary-inert",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct FreeMaskRefusal {
    pub(super) n: usize,
    pub(super) why: FreeMaskWhy,
}

#[derive(Clone, Debug)]
pub(super) struct FreeMaskProposal {
    pub(super) mask: GrayImage,
    pub(super) sign: f32,
    pub(super) mass: f32,
    pub(super) share: (f32, f32),
    pub(super) divergence: fit::Divergence,
    pub(super) pixels: usize,
}

struct Component {
    indices: Vec<usize>,
    sign: f32,
    mass: f32,
    seed: usize,
}

#[derive(Debug)]
struct ProposalSearch {
    proposals: Vec<(usize, FreeMaskProposal)>,
    refusals: Vec<FreeMaskRefusal>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct FreeMaskStage {
    pub(super) ran: bool,
    pub(super) components: usize,
    pub(super) disclosed: usize,
}

fn search_free_masks(
    field: &LocalField,
    excluded: &[f32],
    src_px: &[[f32; 3]],
    tgt_px: &[[f32; 3]],
    evidence: &fit::EvidenceModel,
    cap: usize,
) -> ProposalSearch {
    let (w, h) = (field.width as usize, field.height as usize);
    let n = w.saturating_mul(h);
    if n == 0 || field.remainder.len() != n || field.weight.len() != n {
        return ProposalSearch {
            proposals: Vec::new(),
            refusals: vec![FreeMaskRefusal { n: 0, why: FreeMaskWhy::NoCandidates }],
        };
    }
    let sign_at = |i: usize| {
        let residual = field.remainder[i];
        (field.weight[i] > 0.0
            && residual.abs() > spatial::SPATIAL_RESIDUAL_MIN
            && excluded.get(i).copied().unwrap_or(0.0) < 0.5)
            .then_some(if residual.is_sign_negative() { -1.0 } else { 1.0 })
    };
    let mut visited = vec![false; n];
    let mut components = Vec::new();
    for seed in 0..n {
        let Some(sign) = sign_at(seed) else { continue };
        if visited[seed] { continue; }
        visited[seed] = true;
        let mut stack = vec![seed];
        let mut indices = Vec::new();
        let mut mass = 0.0f64;
        while let Some(i) = stack.pop() {
            indices.push(i);
            mass += field.remainder[i].abs() as f64 * field.weight[i].max(0.0) as f64;
            let (x, y) = (i % w, i / w);
            let neighbours = [
                x.checked_sub(1).map(|nx| y * w + nx),
                (x + 1 < w).then_some(y * w + x + 1),
                y.checked_sub(1).map(|ny| ny * w + x),
                (y + 1 < h).then_some((y + 1) * w + x),
            ];
            for next in neighbours.into_iter().flatten() {
                if !visited[next] && sign_at(next) == Some(sign) {
                    visited[next] = true;
                    stack.push(next);
                }
            }
        }
        components.push(Component { indices, sign, mass: mass as f32, seed });
    }
    components.sort_by(|a, b| b.mass.total_cmp(&a.mass).then_with(|| a.seed.cmp(&b.seed)));
    if components.is_empty() {
        return ProposalSearch {
            proposals: Vec::new(),
            refusals: vec![FreeMaskRefusal { n: 0, why: FreeMaskWhy::NoCandidates }],
        };
    }
    let mut proposals = Vec::new();
    let mut refusals = Vec::new();
    for (rank, component) in components.into_iter().enumerate() {
        let number = rank + 1;
        if component.indices.len() < FREE_MASK_MIN_PIXELS {
            refusals.push(FreeMaskRefusal { n: number, why: FreeMaskWhy::Footprint });
            continue;
        }
        if component.mass <= 0.0 {
            refusals.push(FreeMaskRefusal { n: number, why: FreeMaskWhy::Mass });
            continue;
        }
        let mut geometry = vec![0.0f32; n];
        let mut mask = GrayImage::new(field.width, field.height);
        for &i in &component.indices {
            geometry[i] = 1.0;
            mask.put_pixel((i % w) as u32, (i / w) as u32, Luma([255]));
        }
        let scoped = spatial::scoped_mask_evidence(tgt_px, evidence, &geometry);
        if scoped.source_share < MIN_ZONE_SHARE || scoped.target_share < MIN_ZONE_SHARE {
            refusals.push(FreeMaskRefusal { n: number, why: FreeMaskWhy::Share });
            continue;
        }
        let divergence = fit::structure_divergence(
            src_px, tgt_px, field.width, field.height, &geometry,
        );
        if divergence.d >= fit::DIVERGENCE_ZONE {
            refusals.push(FreeMaskRefusal { n: number, why: FreeMaskWhy::Divergence });
            continue;
        }
        if proposals.len() >= cap {
            refusals.push(FreeMaskRefusal { n: number, why: FreeMaskWhy::Cap });
            continue;
        }
        proposals.push((number, FreeMaskProposal {
            mask,
            sign: component.sign,
            mass: component.mass,
            share: (scoped.source_share, scoped.target_share),
            divergence,
            pixels: component.indices.len(),
        }));
    }
    ProposalSearch { proposals, refusals }
}

// The attachment path calls `search_free_masks` because it also needs every
// typed refusal; keep this proposal-only surface for callers and its exact
// deterministic contract without hiding those refusals in production.
#[cfg(test)]
pub(super) fn propose_free_masks(
    field: &LocalField,
    excluded: &[f32],
    src_px: &[[f32; 3]],
    tgt_px: &[[f32; 3]],
    evidence: &fit::EvidenceModel,
    cap: usize,
) -> Vec<FreeMaskProposal> {
    search_free_masks(field, excluded, src_px, tgt_px, evidence, cap)
        .proposals.into_iter().map(|(_, proposal)| proposal).collect()
}

mod attach;
pub(super) use attach::attach_free_masks;

#[cfg(test)]
mod tests;
