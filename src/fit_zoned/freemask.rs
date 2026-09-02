use image::{GrayImage, Luma};

use super::*;
use crate::fit_field::LocalField;

pub(super) const FREE_MASK_MAX_ATTACHMENTS: usize = 2;

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
    /// The structural instrument could not read this component's footprint, so
    /// nothing testifies that its structure survived — which is exactly what
    /// the divergence gate requires. Never a matched reading.
    StructureUnmeasured,
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
            Self::StructureUnmeasured => "structure-unmeasurable",
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

/// One same-sign remainder component that the ACCEPTED-TILE ALPHA withheld from
/// the candidate set — a proposal the exclusion filter dropped before it could
/// be ranked, scored or refused by name. It used to leave no trace at all: the
/// filter lives inside the seed predicate, so a region an accepted tile already
/// covers simply never became a component, and the report said nothing about
/// work it had declined to look at. Carried so the filter discloses its own
/// drops with the share they cover.
#[derive(Clone, Debug)]
pub(super) struct WithheldComponent {
    pub(super) pixels: usize,
    pub(super) share: (f32, f32),
}

#[derive(Debug)]
struct ProposalSearch {
    proposals: Vec<(usize, FreeMaskProposal)>,
    refusals: Vec<FreeMaskRefusal>,
    withheld: Vec<(usize, WithheldComponent)>,
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
            withheld: Vec::new(),
        };
    }
    // The candidate rule, split from the exclusion so the filter can say what
    // it removed. `carries_residual` is the whole test the field's own reading
    // makes; `alpha` is how much of the pixel an accepted tile already corrects.
    let carries_residual = |i: usize| {
        field.weight[i] > 0.0
            && field.remainder[i].abs() > spatial::SPATIAL_RESIDUAL_MIN
    };
    let sign_of =
        |i: usize| if field.remainder[i].is_sign_negative() { -1.0f32 } else { 1.0f32 };
    let alpha = |i: usize| excluded.get(i).copied().unwrap_or(0.0);
    let offered = |i: usize| (carries_residual(i) && alpha(i) < 0.5).then(|| sign_of(i));
    let withheld_by_tiles =
        |i: usize| (carries_residual(i) && alpha(i) >= 0.5).then(|| sign_of(i));
    let grow = |sign_at: &dyn Fn(usize) -> Option<f32>| -> Vec<Component> {
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
        components
    };
    let components = grow(&offered);
    // What the exclusion filter dropped, in the same shape and by the same
    // ranking, so a reader sees the size of the work it declined to look at.
    // Only components that would have cleared the footprint floor are named —
    // a smaller one would have been refused by name anyway — and only as many
    // as the producer could have attached, which bounds the rationale.
    let withheld = grow(&withheld_by_tiles)
        .into_iter()
        .filter(|component| component.indices.len() >= MIN_MASK_PIXELS)
        .take(cap)
        .enumerate()
        .map(|(rank, component)| {
            let mut geometry = vec![0.0f32; n];
            for &i in &component.indices {
                geometry[i] = 1.0;
            }
            let scoped = spatial::scoped_mask_evidence(tgt_px, evidence, &geometry);
            (rank + 1, WithheldComponent {
                pixels: component.indices.len(),
                share: (scoped.source_share, scoped.target_share),
            })
        })
        .collect::<Vec<_>>();
    if components.is_empty() {
        return ProposalSearch {
            proposals: Vec::new(),
            refusals: vec![FreeMaskRefusal { n: 0, why: FreeMaskWhy::NoCandidates }],
            withheld,
        };
    }
    let mut proposals = Vec::new();
    let mut refusals = Vec::new();
    for (rank, component) in components.into_iter().enumerate() {
        let number = rank + 1;
        if component.indices.len() < MIN_MASK_PIXELS {
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
        // An unreadable component is refused, not passed: the arm below needs
        // "the structure survived", and until v1.2.4 an abstention arrived here
        // as `Divergence::matched()` and cleared it for free.
        let Some(divergence) = fit::structure_divergence(
            src_px, tgt_px, field.width, field.height, &geometry,
        ) else {
            refusals
                .push(FreeMaskRefusal { n: number, why: FreeMaskWhy::StructureUnmeasured });
            continue;
        };
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
    ProposalSearch { proposals, refusals, withheld }
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
