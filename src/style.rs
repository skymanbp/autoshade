//! Style similarity-retrieval reference (V2_PLAN §3).
//!
//! For a photo being edited, find the user's edits on the most SIMILAR past
//! photos (by EXIF + histogram features) and feed those to the advisor as SOFT
//! reference. This deliberately replaces the earlier global-bias "distillation":
//! different photo TYPES are edited differently, so we condition on similar
//! context instead of averaging everything. The retrieved edits are reference,
//! not a target to copy.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::decode::{self, Histogram, Meta};
// The typed `crs:` readers (R28 Batch-5 5d): the trait carries the methods,
// `Scope` declares that a whole-sidecar read is a subtree read.
use crate::xmp::CrsSource;
use crate::pipeline;
use crate::recipe::EditRecipe;

const NDIM: usize = 14;
/// Unbounded (log / ratio) dims to z-score; the rest are already ~bounded.
const ZSCORE_DIMS: [usize; 4] = [0, 1, 2, 10];
/// Per-dim distance weights (scene-type discriminators heavier).
const WEIGHTS: [f32; NDIM] = [
    1.5, 1.0, 1.0, 0.5, 0.5, 1.5, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.5,
];
/// Slider keys shown as the reference (crs key → label). Tint/Saturation/Dehaze
/// were added in index v2 so the style blend captures the user's colour habits.
/// Bump when the FEATURE semantics change (v3: display-frame Meta dims, i.e.
/// the EXIF orientation reaching the aspect feature at last; v4: the COMPOSED
/// orientation — v3 plus the photographer's own `quarter_turns`, R27, so a
/// hand-rotated shot is retrieved as the portrait/landscape it now IS;
/// **v5: the optional SigLIP 2 embedding block** — a v5 exemplar MAY carry
/// [`StyleExemplar::embed`] and the distance MAY gain a cosine term, which
/// changes the RANKING and therefore the version).
const CURRENT_INDEX_VERSION: u32 = 5;
/// Index versions this build will serve. v4 is accepted alongside v5 on
/// purpose: the v5 change is PURELY ADDITIVE (an absent `embed` contributes
/// nothing to the distance, [`embed_distance`]), so refusing a v4 index would
/// leave every existing user with a dead Style panel for the length of an
/// hour-long rebuild in exchange for nothing. The FEATURE semantics of v4 and
/// v5 are identical — that is what makes this safe, and it is why v3 is still
/// refused (its aspect dim means something else).
const READABLE_INDEX_VERSIONS: [u32; 2] = [4, CURRENT_INDEX_VERSION];
// Load-time bounds: an index is disk input that reaches the model prompt, so
// its size, shape and values get invariants at the door (there is no
// exfiltration channel — the response is strict json_schema — but an
// unbounded index means an unbounded paid request and a steerable grade).
//
// RE-DERIVED TOGETHER in R27 Batch-5, because they had stopped agreeing, and
// again here (F-5) because the look library had joined the file without
// joining the arithmetic. The whole derivation, measured rather than argued:
//
//   * one embedding, as serialised: 768 elements at `str(np.float32(x))`
//     precision measured **12.41 bytes each** over three real SigLIP 2
//     vectors (9,529 bytes of array text). The conservative bound is a
//     15-character shortest-decimal plus its comma = 16 B, so
//     768 x 16 = 12,288 B = 12 KiB.
//   * the LARGEST record either side can produce is a maximal RAW exemplar:
//     TWO such vectors (image + description), the 14-dim feature, 33
//     vocabulary scores, four bounded tags, a `MAX_DESC_CHARS` description, a
//     full settings map, curve, family summary and its stem/path envelope.
//     `capacity_constants_hold_two_vectors_and_the_scores` serialises exactly
//     that record and measures it: **20,940 B** (a maximal LOOK record, which
//     has no feature/settings/curve, measures 20,331 B). 40 KiB is therefore a
//     ~2x bound, which is the room JSON escaping of a hostile stem/description
//     would need.
//   * the file must hold BOTH populations at once, because `save` merges them
//     into one document: 5,000 RAW exemplars + `MAX_LOOK_EXEMPLARS` look
//     records. 5,500 x 40 KiB = 225,280,000 B = 214.84 MiB, and the 228 MiB
//     file cap leaves 13.16 MiB for the top-level envelope (two 14-element
//     normalisation vectors, source/looks dir, provenance).
//
// Before F-5 the `const _` gate counted the RAW population only while `load`
// admitted 5,000 looks BESIDE it — 10,000 x 40 KiB = 390 MiB against a 228 MiB
// cap. The gate was true and meaningless; the look cap is what makes it mean
// something, and it is enforced at the door like the RAW one.
const MAX_STYLE_INDEX_BYTES: usize = 228 * 1024 * 1024;
const MAX_STYLE_EXEMPLARS: usize = 5_000;
/// The look population's own cap. A look library is a CURATED set of finished
/// photos (the reference grades a photographer would point at), not a whole
/// archive — the 5,000-entry product limit belongs to the RAW library it sits
/// beside, and giving looks the same number is what broke the byte envelope.
const MAX_LOOK_EXEMPLARS: usize = 500;
/// The per-record bound both caps above are derived from.
const MAX_EXEMPLAR_BYTES: usize = 40 * 1024;
/// Longest description a record may carry, at the door and in the prompt.
pub const MAX_DESC_CHARS: usize = 512;

// R18 CANNOT RECUR, because this is a BUILD gate and not a test: moving either
// cap without the other stops compilation with the sentence below. A runtime
// test would have been the weaker choice — clippy's own `assertions_on_constants`
// points at exactly this, and a constant-vs-constant invariant belongs where
// the constants are.
//
// MUTATION: set `MAX_STYLE_EXEMPLARS` back to 50_000, raise
// `MAX_LOOK_EXEMPLARS` to the RAW cap, or set `MAX_STYLE_INDEX_BYTES` back to
// 32 MiB, and `cargo check` fails here.
const _: () = assert!(
    (MAX_STYLE_EXEMPLARS + MAX_LOOK_EXEMPLARS) * MAX_EXEMPLAR_BYTES <= MAX_STYLE_INDEX_BYTES,
    "both populations x the per-record bound exceed the index file cap — the constants \
     moved apart again (R18; the look cap joined them in S1-fix F-5)"
);
// …and the per-record bound must actually HOLD a maximal record, or the line
// above would be true and meaningless. 768 elements at the 16-byte worst case
// of a shortest-round-trip f32 decimal plus its comma is 12 KiB; the runtime
// half of this check is `capacity_constants_hold_two_vectors_and_the_scores`,
// which serialises the record and measures it.
const _: () = assert!(
    MAX_EXEMPLAR_BYTES >= crate::embed::EMBED_DIM * 16 * 2
        + LOOK_VOCAB.len() * 16
        + MAX_DESC_CHARS * 6
        + 4096,
    "the per-record bound cannot hold two embeddings, scores, and description"
);

/// The six environment overrides, spelled once.
///
/// Each is read in EXACTLY ONE place ([`EmbeddingSwitch::resolve`],
/// [`DescribeSwitch::resolve`] and [`RetrievalWeights::from_env`]) and never
/// again: everything downstream takes
/// the resolved VALUE. That is not tidiness. `cargo test` runs tests on
/// parallel threads in one process, so a retrieval that read the process
/// environment could be reconfigured mid-run by an unrelated test — which is
/// exactly what the S1 tests did, with 14 unsafe environment writes between them.
/// The switch used to be *implemented* by mutating the environment too
/// (`--no-embed` wrote `AUTOSHOP_STYLE_EMBED=0` into the process), so a CLI
/// flag was a global side effect rather than an argument.
const ENV_EMBED: &str = "AUTOSHOP_STYLE_EMBED";
const ENV_DESCRIBE: &str = "AUTOSHOP_STYLE_DESCRIBE";
const ENV_EMBED_WEIGHT: &str = "AUTOSHOP_STYLE_EMBED_WEIGHT";
const ENV_TEXT_WEIGHT: &str = "AUTOSHOP_STYLE_TEXT_WEIGHT";
const ENV_DESC_WEIGHT: &str = "AUTOSHOP_STYLE_DESC_WEIGHT";
const ENV_LOOK_WEIGHT: &str = "AUTOSHOP_STYLE_LOOK_WEIGHT";

/// Weight of the image-embedding block in retrieval — re-confirmed by the S2
/// recalibration on the described index (`scripts/calibrate_style_retrieval.py`,
/// 169 exemplars / 156 settings-bearing queries, 196 grid rows per proxy).
/// It is the whole of the text-free improvement: `(4, 0, 0)` scores 0.695233
/// against the 14-dim baseline's 0.713143, CI [+0.006902, +0.034956].
/// Setting it to zero reproduces the feature-only ranking exactly: the cosine
/// term is added separately and never folded into the existing sum.
pub const W_EMB_DEFAULT: f64 = 4.0;
/// Weight of the direction-text ↔ exemplar-IMAGE term.
///
/// S1 shipped this at 0 because no grid point with a non-zero `W_TXT` beat the
/// best `W_TXT = 0` row — measured with a proxy that was the exemplar's TAG
/// STRING, because no exemplar had a description to be the query text. S2 gave
/// every exemplar real prose and re-ran the grid under BOTH proxies, and the
/// answer changed: with prose as the query text the standardised text terms
/// beat the same variant's text-free row `(4, 0, 0)` with a paired 95 % CI of
/// [+0.001589, +0.055436], and 4.0 is the winning weight. Under the TAG-string
/// proxy nothing beats the text-free row in either variant — so it is the
/// prose, not the vocabulary, that earns this term.
///
/// DISCLOSED LIMITATION: the query-text proxy is the held-out exemplar's OWN
/// description, i.e. a text that describes the query photograph perfectly. A
/// user's typed Direction is not that, so this weight is calibrated on a
/// friendlier query than it will see.
pub const W_TXT_DEFAULT: f64 = 4.0;
/// Weight of the direction-text ↔ exemplar-DESCRIPTION term.
///
/// S1 shipped 4.0 on a grid whose `desc_embed` vectors — on BOTH sides — were
/// the TAG STRING, because no exemplar had a description yet. S2 gave every
/// exemplar real prose from `describe.py` and re-ran the grid, and that number
/// did not survive contact with the data it was supposed to describe: under
/// the shipped-until-now raw variant `(4, 0, 4)` now scores 0.698491, which is
/// WORSE than the text-free `(4, 0, 0)` at 0.695233. Leaving it at 4 was
/// therefore not an option either way.
///
/// 0.5 is the winning weight of the winning row, `(4, 4, 0.5)` standardised,
/// MAE 0.664818 against the 14-dim baseline's 0.713143 — improvement
/// +0.048325, paired bootstrap 95 % CI [+0.024290, +0.078587]. Most of that
/// gain is `W_TXT`; this term adds the last +0.015 on top of `(4, 4, 0)`.
pub const W_DESC_DEFAULT: f64 = 0.5;
/// Weight of the look-library image term. It is the ONLY term that ranks looks
/// against EACH OTHER (the description term reranks them, but through the same
/// candidate set), so its SCALE cannot change their order — pinned by
/// `look_weight_scale_does_not_change_look_order`. 1.0 is therefore a
/// normalisation, not a tuned number, and the harness never evaluated it: the
/// look library carries no settings, so the leave-one-out settings objective
/// cannot see it at all.
pub const W_LOOK_DEFAULT: f64 = 1.0;
/// Does the ranking Z-SCORE the two direction-text terms over the candidate
/// set before weighting them, or weight the raw `1 − cos` gaps?
///
/// F-14 built the standardised variant because SigLIP image↔text cosines are
/// tiny and tightly clustered, so a raw term barely reorders anything and a
/// grid over it can "find" 0 for the wrong reason. S1 measured both variants
/// and the raw one won — on a grid whose query text was the exemplar's tag
/// string. S2's recalibration, with real prose on both sides, reverses it:
/// best standardised `(4, 4, 0.5)` = 0.664818 against best raw `(4, 2, 0)` =
/// 0.693811, and the raw variant's own text terms are indistinguishable from
/// having none (paired CI against `(4, 0, 0)` = [−0.001834, +0.004821]) while
/// the standardised variant's are not ([+0.001589, +0.055436]).
///
/// DISCLOSED LIMITATION: the variant head-to-head itself is NOT significant —
/// paired (raw-best − standardised-best) 95 % CI [−0.000205, +0.054341]
/// includes 0, barely. The choice therefore rests on which variant won and on
/// which variant's text terms earn their keep, not on a significant difference
/// between the two best rows. `scripts/calibrate_style_retrieval.py` prints
/// both comparisons on every run, so re-deciding it is a re-read, not a
/// re-derivation.
///
/// Pinned by `the_shipped_text_variant_is_the_measured_one`.
pub const STANDARDISE_TEXT_TERMS: bool = true;

/// The four retrieval weights as ONE value, resolved once at the top of a run
/// and passed down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetrievalWeights {
    pub emb: f64,
    pub txt: f64,
    pub desc: f64,
    pub look: f64,
}

impl Default for RetrievalWeights {
    fn default() -> Self {
        Self::SHIPPED
    }
}

impl RetrievalWeights {
    /// What the harness shipped.
    pub const SHIPPED: Self = RetrievalWeights {
        emb: W_EMB_DEFAULT,
        txt: W_TXT_DEFAULT,
        desc: W_DESC_DEFAULT,
        look: W_LOOK_DEFAULT,
    };
    /// Every cosine term off — the 14-dim ranking, bit for bit. The look
    /// weight keeps its normalisation because a zero there would tie every
    /// look against every other rather than removing a term.
    pub const FEATURE_ONLY: Self =
        RetrievalWeights { emb: 0.0, txt: 0.0, desc: 0.0, look: W_LOOK_DEFAULT };

    /// The shipped weights with the environment's overrides applied — the ONE
    /// place this process reads those variables.
    pub fn from_env() -> Self {
        Self::resolve(|k| std::env::var(k).ok())
    }

    /// [`from_env`](Self::from_env) over an explicit environment — the seam the
    /// tests drive, so no test has to mutate the process to state a rule.
    ///
    /// Non-finite and negative values fall back to the shipped weight: a
    /// negative weight would rank the LEAST similar photo first, which is not a
    /// tuning choice.
    pub fn resolve(get: impl Fn(&str) -> Option<String>) -> Self {
        let one = |name: &str, default: f64| -> f64 {
            get(name)
                .and_then(|s| s.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v >= 0.0)
                .unwrap_or(default)
        };
        RetrievalWeights {
            emb: one(ENV_EMBED_WEIGHT, W_EMB_DEFAULT),
            txt: one(ENV_TEXT_WEIGHT, W_TXT_DEFAULT),
            desc: one(ENV_DESC_WEIGHT, W_DESC_DEFAULT),
            look: one(ENV_LOOK_WEIGHT, W_LOOK_DEFAULT),
        }
    }
}

/// Is the embedding sidecar wanted for this run? A VALUE, resolved once from
/// flag > environment > preference and then carried as an argument.
///
/// Opt-in because the first run downloads **1.50 GB** of weights and every
/// call re-loads them (~seconds), which is not a cost an index build or a
/// develop may take without being asked. An index built with it off is a
/// perfectly good v5 index — the block is simply absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingSwitch(bool);

impl EmbeddingSwitch {
    pub const ON: Self = EmbeddingSwitch(true);
    pub const OFF: Self = EmbeddingSwitch(false);

    pub fn on(self) -> bool {
        self.0
    }

    /// flag > environment > preference, reading the process environment once.
    ///
    /// `flag` is the CLI's `--embed` / `--no-embed` (`None` when neither was
    /// given). An environment variable that is SET wins over the preference
    /// whatever its value, including `0` — that is what makes it an override.
    pub fn resolve(flag: Option<bool>, pref: bool) -> Self {
        Self::resolve_with(flag, pref, |k| {
            std::env::var_os(k).map(|v| v.to_string_lossy().into_owned())
        })
    }

    /// [`resolve`](Self::resolve) over an explicit environment — the tested
    /// seam.
    pub fn resolve_with(
        flag: Option<bool>,
        pref: bool,
        get: impl Fn(&str) -> Option<String>,
    ) -> Self {
        if let Some(f) = flag {
            return EmbeddingSwitch(f);
        }
        match get(ENV_EMBED) {
            Some(v) => EmbeddingSwitch(!matches!(v.trim(), "" | "0" | "false" | "off")),
            None => EmbeddingSwitch(pref),
        }
    }
}

/// Is the LOCAL description model wanted for this build? A VALUE, resolved
/// once from flag > environment > preference and then carried as an argument —
/// the same shape as [`EmbeddingSwitch`], and for the same reason (a switch
/// implemented by writing the process environment is a shared mutable global
/// that `cargo test`'s parallel threads can reconfigure under each other).
///
/// Opt-in because the first run downloads **4.3 GB** of Qwen3-VL weights and
/// the pass costs seconds per photograph on top of the embedding. An index
/// built with it off is a perfectly good v5 index — `desc` is simply absent
/// and `desc_embed` falls back to the tag string, which is exactly what S1
/// shipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DescribeSwitch(bool);

impl DescribeSwitch {
    pub const ON: Self = DescribeSwitch(true);
    pub const OFF: Self = DescribeSwitch(false);

    pub fn on(self) -> bool {
        self.0
    }

    /// flag > environment > preference, reading the process environment once.
    ///
    /// `flag` is the CLI's `--describe` (`None` when it was not given). An
    /// environment variable that is SET wins over the preference whatever its
    /// value, including `0` — that is what makes it an override.
    pub fn resolve(flag: Option<bool>, pref: bool) -> Self {
        Self::resolve_with(flag, pref, |k| {
            std::env::var_os(k).map(|v| v.to_string_lossy().into_owned())
        })
    }

    /// [`resolve`](Self::resolve) over an explicit environment — the tested
    /// seam.
    pub fn resolve_with(
        flag: Option<bool>,
        pref: bool,
        get: impl Fn(&str) -> Option<String>,
    ) -> Self {
        if let Some(f) = flag {
            return DescribeSwitch(f);
        }
        match get(ENV_DESCRIBE) {
            Some(v) => DescribeSwitch(!matches!(v.trim(), "" | "0" | "false" | "off")),
            None => DescribeSwitch(pref),
        }
    }
}

/// What a stored embedding block was produced BY, in one string.
///
/// The checkpoint alone was not enough to tell two incomparable indices apart:
/// the text tower's numbers also depend on which TOKENIZER produced the ids,
/// and this batch's own root cause (F-11) was two doors on one checkpoint
/// answering vectors at cosine 0.72-0.78 of each other. An index built through
/// the other door is not comparable with this one, and now it does not look
/// identical either.
///
/// The revision is abbreviated to 12 characters after the tokenizer class
/// because it is the same pinned checkpoint revision spelled in full earlier in
/// the same string — the field says WHICH tokenizer, not a second pin.
pub fn embed_provenance_string() -> String {
    format!(
        "{}@{} tokenizer={}@{} vocab-v{}",
        crate::embed::MODEL_REPO,
        crate::embed::MODEL_REVISION,
        crate::embed::TEXT_TOKENIZER_CLASS,
        &crate::embed::MODEL_REVISION[..12],
        LOOK_VOCAB_VERSION,
    )
}

/// The `vocab-vN` stamp out of a provenance string, when it carries one.
///
/// `None` covers both "no stamp" (an index written before the field existed)
/// and "unparseable"; the loader treats those as unknown, never as a match.
pub fn vocab_version_of(provenance: &str) -> Option<u32> {
    provenance
        .split_whitespace()
        .find_map(|field| field.strip_prefix("vocab-v"))
        .and_then(|n| n.parse().ok())
}

/// ONE retrieval query: the vectors it was able to produce and what those
/// vectors are worth.
///
/// They travel together because they are only meaningful together — a text
/// vector with `txt = 0` contributes nothing, and a weight without a vector
/// weights nothing. Passing them as one value is also what keeps
/// [`StyleIndex::retrieve_with_embed`] and [`StyleIndex::distance_components`]
/// inside clippy's argument budget without an `allow`.
#[derive(Debug, Clone, Copy, Default)]
pub struct StyleQuery<'a> {
    /// The query photo's SigLIP image vector, when the sidecar produced one.
    pub image: Option<&'a [f32]>,
    /// The direction text's SigLIP text vector, when there was a direction.
    pub text: Option<&'a [f32]>,
    pub weights: RetrievalWeights,
}

impl<'a> StyleQuery<'a> {
    /// A query with NO vectors — the 14-dim ranking. The weights still ride
    /// along because `retrieve_looks` reads `look` even here (it decides
    /// nothing when there is no vector, but the value is the same one).
    pub const FEATURES_ONLY: Self =
        StyleQuery { image: None, text: None, weights: RetrievalWeights::SHIPPED };

    pub fn new(image: Option<&'a [f32]>, text: Option<&'a [f32]>, weights: RetrievalWeights) -> Self {
        StyleQuery { image, text, weights }
    }
}

/// SigLIP-style attribute captions. Changing a phrase changes all stored
/// scores, therefore the version is persisted in each index envelope.
pub const LOOK_VOCAB_VERSION: u32 = 1;
pub const LOOK_VOCAB: [&str; 33] = [
    // white-balance lean
    "a photo with warm golden tones", "a photo with cool blue tones", "a photo with neutral white balance",
    // tonality
    "a photo with deep blacks", "a photo with lifted matte shadows", "a photo with bright airy high-key tones", "a photo with dark moody low-key tones",
    // contrast
    "a photo with punchy high contrast", "a photo with soft low contrast",
    // saturation
    "a photo with vivid saturated colours", "a photo with muted desaturated colours", "a photo with pastel colours",
    // colour treatment
    "a photo with a teal-and-orange split tone", "a monochrome black-and-white photo", "a photo with sepia toning", "a photo with a cross-processed colour treatment",
    // finishing
    "a photo with a soft hazy glow", "a photo with crisp clarity", "a photo with film-like grain", "a photo with a clean digital finish",
    // light
    "a golden-hour photo", "a blue-hour photo", "an overcast flat-light photo", "a harsh midday-light photo", "a night photo",
    // extra stable descriptors to keep the vocabulary useful across scenes
    "a photo with gentle natural light", "a photo with dramatic directional light", "a photo with rich shadow detail", "a photo with restrained colour", "a photo with luminous highlights", "a photo with cinematic tones", "a photo with a soft editorial grade", "a photo with a neutral documentary grade",
];
pub const LOOK_TAGS_K: usize = 4;
const LOOK_GROUPS: &[&[usize]] = &[&[0,1,2], &[3,4,5,6], &[7,8], &[9,10,11], &[12,13,14,15], &[16,17,18,19], &[20,21,22,23,24], &[25,26,27,28,29,30,31,32]];

fn walkdir(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for ent in std::fs::read_dir(&dir).with_context(|| format!("scan {}", dir.display()))? {
            let p = ent?.path();
            if p.is_dir() { stack.push(p); } else { out.push(p); }
        }
    }
    Ok(out)
}

/// Long edge the preview is reduced to before the embedding sidecar sees it —
/// the ONE frame both the index build and the query go through.
///
/// It exists so the two cannot disagree. The sidecar squashes whatever it is
/// given to 384x384, so the only way a photo's index vector and its query
/// vector could differ is if the two paths handed it different pixels: the
/// build starts from a 61 MP embedded preview and the develop path from an
/// advisor-sized one. Both resize through THIS constant and THIS filter first.
const EMBED_FRAME_EDGE: u32 = 512;

/// How many similar past shots one develop leans on. Named since R23-2
/// because the number is now DISCLOSED ("the 4 most similar shots"), so the
/// retrieval and the sentence about it must read the same constant.
pub const RETRIEVE_K: usize = 4;

/// Disclosure bounds for [`neighbour_stems`]: the rationale is persisted,
/// shown in three UIs and capped, so the "which photos did it reference?"
/// answer is bounded at the source rather than trusting `RETRIEVE_K` and the
/// user's file naming to stay small.
const MAX_DISCLOSED_NEIGHBOURS: usize = 4;
const MAX_STEM_CHARS: usize = 48;

/// The legacy, cwd-relative index a pre-store build wrote. Kept readable so
/// an index built before the central store existed keeps working; nothing
/// writes here any more.
pub const LEGACY_INDEX_PATH: &str = "out/style-index.json";

/// Physical band for feature / normalization values (L04-3): every real
/// dimension is a ln() of a physical quantity or a bounded ratio (|v| ≲ 200
/// by construction — see `feature_vector`), so 1e3 leaves 5× headroom. A
/// merely-FINITE 1e38 passed the old check, overflowed `(v - mean)/std` to
/// ±inf in `normalize`, and two exemplars straddling the mean then handed
/// the retrieval sort a mixed NaN/inf key set — an unspecified ranking (a
/// silently wrong style reference in a PAID prompt) or a sort panic.
const MAX_FEATURE_ABS: f32 = 1e3;

/// Slider keys the reference block shows, as `(crs attribute, label)`.
///
/// A CURATED SUBSET of `advisor::catalogue::RECIPE_CONTROLS`, not a derivation
/// of it (R23-1): the index learns a per-key MEAN across retrieved exemplars,
/// and that only says something for the tone/colour sliders a photographer
/// applies habitually. The registry-consistency test below pins every
/// attribute spelling and field name here to the registry, so a renamed
/// control cannot leave this table pointing at a key nothing writes; the
/// colour FAMILIES are carried as summary statistics instead (see
/// [`StyleExemplar::families`]), because averaging 38 per-band keys across
/// four exemplars is mush.
const REF_KEYS: [(&str, &str); 12] = [
    ("Exposure2012", "exposure"),
    ("Contrast2012", "contrast"),
    ("Highlights2012", "highlights"),
    ("Shadows2012", "shadows"),
    ("Whites2012", "whites"),
    ("Blacks2012", "blacks"),
    ("Vibrance", "vibrance"),
    ("Clarity2012", "clarity"),
    ("Temperature", "temperature_K"),
    ("Tint", "tint"),
    ("Saturation", "saturation"),
    ("Dehaze", "dehaze"),
];

/// 14-dim feature vector from capture metadata + histogram.
pub fn feature_vector(meta: &Meta, hist: &Histogram) -> [f32; NDIM] {
    let lnpos = |v: f32| if v > 0.0 { v.ln() } else { 0.0 };
    let total: f64 = hist.luma.iter().map(|&v| v as f64).sum::<f64>().max(1.0);
    let mean_of = |b: &[u32]| -> f32 {
        let s: f64 = b.iter().enumerate().map(|(i, &v)| i as f64 * v as f64).sum();
        (s / total) as f32
    };
    let mean_l = mean_of(&hist.luma);
    let var: f64 = hist
        .luma
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let d = i as f64 - mean_l as f64;
            d * d * v as f64
        })
        .sum::<f64>()
        / total;
    let std_l = var.sqrt() as f32;
    let (mr, mg, mb) = (mean_of(&hist.r), mean_of(&hist.g), mean_of(&hist.b));
    let hour = parse_hour(meta.date_time.as_deref());
    let (w, h) = (meta.width.max(1) as f32, meta.height.max(1) as f32);
    let wb = meta.as_shot_wb_coeffs;
    let warmth = if wb[0] > 0.0 && wb[2] > 0.0 { (wb[0] / wb[2]).ln() } else { 0.0 };
    let ang = std::f32::consts::TAU * hour / 24.0;
    [
        lnpos(meta.focal_length_mm.unwrap_or(35.0)),
        lnpos(meta.iso.unwrap_or(100) as f32),
        lnpos(meta.aperture.unwrap_or(5.6)),
        ang.sin(),
        ang.cos(),
        mean_l / 255.0,
        hist.clip_black_pct / 100.0,
        hist.clip_white_pct / 100.0,
        (mr - mg) / 255.0,
        (mb - mg) / 255.0,
        warmth,
        w / h,
        std_l / 255.0,
        if h > w { 1.0 } else { 0.0 },
    ]
}

fn parse_hour(dt: Option<&str>) -> f32 {
    // EXIF "2023:06:01 14:30:00" → 14
    dt.and_then(|s| s.split(' ').nth(1))
        .and_then(|t| t.split(':').next())
        .and_then(|h| h.parse::<f32>().ok())
        // EXIF is other-software input: a NaN/absurd hour would poison the
        // hour-angle feature and with it every retrieval distance.
        .filter(|h| h.is_finite() && (0.0..24.0).contains(h))
        .unwrap_or(12.0)
}

fn read_settings(xmp: &str) -> BTreeMap<String, f32> {
    // As-shot provenance (same rule as the eval harness): under
    // WhiteBalance="As Shot", crs:Temperature/Tint record the CAMERA's
    // values, not a user edit — learning them taught the style index to
    // "prefer" whatever Kelvin the user's camera metered. Any other value
    // (Custom / a preset) is a user decision; absent = non-LR, kept.
    // Same scope rule as every other whole-document reader
    // (`xmp::crs_own_scope`): a nested creative Look's baked parameters belong
    // to the PROFILE, not the photographer, and learning them would teach the
    // index that this user "prefers" whatever look Adobe ships — the same
    // provenance error the as-shot rule above guards against, one container
    // deeper.
    let xmp = crate::xmp::crs_own_scope(xmp);
    // A `Scope`, spelled out (R28 Batch-5 5d): subtree-wide first-match is what
    // a whole-sidecar read means, and the type is what says so now.
    let xmp = crate::xmp::Scope::new(xmp.as_ref());
    let user_wb = xmp.crs_str("WhiteBalance").as_deref() != Some("As Shot");
    REF_KEYS
        .iter()
        .filter(|(k, _)| user_wb || !matches!(*k, "Temperature" | "Tint"))
        .filter_map(|(k, label)| xmp.crs_f32(k).map(|v| (label.to_string(), v)))
        .collect()
}

/// Short human tag like "tele/bright/midday" for the reference block.
fn derive_tag(f: &[f32; NDIM]) -> String {
    let focal = f[0].exp();
    let lens = if focal < 24.0 { "ultrawide" } else if focal < 45.0 { "wide" } else if focal < 90.0 { "normal" } else { "tele" };
    let tone = if f[5] < 0.33 { "dark" } else if f[5] > 0.6 { "bright" } else { "mid" };
    let hour = (f[3].atan2(f[4]) / std::f32::consts::TAU * 24.0 + 24.0) % 24.0;
    let tod = if !(5.0..20.0).contains(&hour) { "night" } else if (5.0..9.0).contains(&hour) || (17.0..20.0).contains(&hour) { "goldenish" } else { "midday" };
    let orient = if f[13] > 0.5 { "portrait" } else { "landscape" };
    format!("{lens}/{tone}/{tod}/{orient}")
}

#[derive(Serialize, Deserialize, Clone)]
pub struct StyleExemplar {
    pub stem: String,
    pub feat: Vec<f32>,
    pub tag: String,
    pub settings: BTreeMap<String, f32>,
    /// The user's master tone-curve shape `[black_lift, s_strength]` (0..255
    /// scale), if they drew one — the curve "habit" the flat sliders can't carry.
    /// `#[serde(default)]` keeps v1 index files loadable.
    #[serde(default)]
    pub curve: Option<[f32; 2]>,
    /// Absolute source path — self-exclusion identity (two rolls can hold two
    /// DSC00001.ARW, and stem-based exclusion dropped BOTH whenever either
    /// was being edited). `#[serde(default)]`: older indexes lack it and fall
    /// back to stem exclusion, over-exclusion being the safe direction.
    #[serde(default)]
    pub path: Option<String>,
    /// How hard this user pushes the colour FAMILIES the flat `settings` map
    /// cannot carry — the HSL mixer, the grade wheels, the per-channel curves
    /// (R23-1, feedback #12: the reference block was blind to all three, so
    /// the AI had no signal about a photographer who shapes colour per band).
    /// Summary statistics, not the 38 keys — see [`crate::eval::FamilySummary`].
    ///
    /// `#[serde(default)]` keeps every pre-R23 index loadable, and it is an
    /// ADDED field only: no existing key changes meaning, so the index version
    /// deliberately does NOT bump (a bump forces every user to rebuild an hour-
    /// long index for a field that degrades to "no summary line").
    #[serde(default)]
    pub families: Option<crate::eval::FamilySummary>,
    /// SigLIP 2 image embedding — 768 dims, L2-normalised, produced by
    /// `python/embed.py` (R27 Batch-5, index v5).
    ///
    /// **Beside the 14-dim feature, never fused into it.** The hand feature is
    /// EXIF + histogram: focal length, hour of day, clipping, warmth. The
    /// embedding is what the frame LOOKS like. They answer different questions
    /// and they are on incomparable scales, so the distance keeps them as two
    /// blocks with their own weights ([`embed_distance`]) rather than
    /// concatenating them into one vector where `WEIGHTS` would stop meaning
    /// anything.
    ///
    /// Deliberately NOT z-scored, unlike [`ZSCORE_DIMS`]: a per-dim mean and
    /// σ over ~150 exemplars is a degenerate estimate at 768 dims, and
    /// `normalize`'s `(v−mean)/σ` would amplify whichever dims happened to be
    /// flat. A cosine over unit vectors needs no per-dim statistics at all,
    /// which is the second reason the two blocks stay separate.
    ///
    /// `#[serde(default)]` and `Option`, so this is optional in BOTH
    /// directions: a v4 index has none (and the cosine term contributes
    /// nothing), and a v5 index built without the sidecar has none either.
    /// Retrieval must tolerate an index built either way — that is the
    /// contract, not a fallback.
    #[serde(default)]
    pub embed: Option<Vec<f32>>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub vocab_scores: Option<Vec<f32>>,
    #[serde(default)]
    pub desc: Option<String>,
    #[serde(default)]
    pub desc_embed: Option<Vec<f32>>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct LookExemplar {
    pub stem: String,
    pub path: String,
    pub embed: Vec<f32>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub vocab_scores: Option<Vec<f32>>,
    #[serde(default)]
    pub desc: Option<String>,
    #[serde(default)]
    pub desc_embed: Option<Vec<f32>>,
}

/// The embedding half of the retrieval distance: `W_EMB · (1 − cos(q, e))`,
/// or `0.0` when either side has no vector.
///
/// Both sides are L2-normalised by the sidecar (and re-checked at the door by
/// [`crate::embed::parse_vector`] / [`exemplar_is_finite`]), so the cosine is a
/// plain dot product. Accumulated in f64 for the same reason the 14-dim block
/// is: the ranking must be deterministic, and `total_cmp` can only order keys
/// that were computed the same way every time.
///
/// A width mismatch answers 0 rather than truncating: two vectors of different
/// widths are not comparable, and silently comparing their common prefix is
/// how an index that mixed models would produce a plausible, wrong ranking.
fn embed_distance(q: Option<&[f32]>, e: Option<&[f32]>, w: f64) -> f64 {
    cosine_gap(q, e).map_or(0.0, |gap| w * gap)
}

/// `1 − cos(q, e)`, or `None` when the pair is NOT COMPARABLE — either side
/// missing, or two different widths (see [`embed_distance`]).
///
/// The `None` is the load-bearing part: it is what lets the standardisation
/// above distinguish "this candidate scored badly" from "this candidate was
/// never measured", which a bare 0.0 cannot.
fn cosine_gap(q: Option<&[f32]>, e: Option<&[f32]>) -> Option<f64> {
    let (Some(q), Some(e)) = (q, e) else { return None };
    if q.len() != e.len() || q.is_empty() {
        return None;
    }
    let dot: f64 = q.iter().zip(e).map(|(&a, &b)| a as f64 * b as f64).sum();
    // Unit vectors put the dot in [-1, 1]; the clamp is against the ~1e-7
    // round-trip slack the JSON text carries, not against a real value.
    Some(1.0 - dot.clamp(-1.0, 1.0))
}

/// One photo's embedding frame, staged on disk and OWNED: dropping it removes
/// both temp files.
///
/// It exists so the ~181 MB preview and the multi-second sidecar call stop
/// overlapping (R28 Batch-4 4b). `embed_preview` did both in one call, which
/// meant the caller's decode-sized buffer — and, in `StyleIndex::build`, its
/// `DecodePermit` — stayed alive for the whole model load. Staging is a
/// 512x512 PNG; once it exists, the frame the sidecar needs is 200 KB on disk
/// rather than 181 MB in RAM.
///
/// RAII rather than the two `remove_file` calls it replaces: the staged files
/// are intermediates, and every early return out of the embed path used to be
/// a chance to leak one into the user's temp directory.
pub struct StagedFrame {
    img: PathBuf,
    json: PathBuf,
}

impl StagedFrame {
    /// The staged PNG a sidecar reads. Exposed since S2, when the model calls
    /// moved OUT of the per-photo loop: a batch door takes a manifest of
    /// paths, so the frame has to outlive the worker that staged it.
    pub fn image(&self) -> &Path {
        &self.img
    }

    /// …and the same path as the string a JSONL manifest carries. One
    /// spelling, because the answer is mapped back by this exact text.
    pub fn image_path(&self) -> String {
        self.img.display().to_string()
    }
}

impl Drop for StagedFrame {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.img);
        let _ = std::fs::remove_file(&self.json);
    }
}

/// Reduce a camera preview to THE embedding frame and write it out.
///
/// The reduction is the reason both the index build and the develop-time query
/// go through one function: the sidecar squashes whatever it is given to
/// 384x384, so the only way a photo's stored vector and its query vector could
/// differ is if the two paths handed it different pixels.
pub fn stage_embed_frame(
    preview: &image::DynamicImage,
    dir: &Path,
    tag: &str,
) -> Result<StagedFrame> {
    // `Triangle` is bilinear, NAMED rather than defaulted for the same reason
    // the sidecar names PIL's resample: a filter that changed under us would
    // move every vector without a line of this repo changing.
    let small = preview.resize(EMBED_FRAME_EDGE, EMBED_FRAME_EDGE, image::imageops::FilterType::Triangle);
    // pid + seq + tag: two workers (or two processes) must never share one
    // name. The pid+tag pair was NOT enough — `pipeline::produce_recipe` passes
    // the constant tag "query" for every photo, so the moment two develops ran
    // concurrently in one process (the batch pool, which has shipped at three
    // workers since R26; the web server's request threads) they staged into the
    // same `autoshop-embed-<pid>-query.png` and the same `.json`: one worker
    // embedded the other's frame and got a style vector for the wrong
    // photograph, and either one's cleanup deleted the other's file mid-run.
    // The seq belongs HERE rather than at the call site — that is the fix that
    // holds for every caller, including the next one (`StyleIndex::build`
    // already passed a unique `idx-{i}` and was never affected).
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let stem = format!(
        "autoshop-embed-{}-{}-{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        tag
    );
    // Constructed BEFORE the write, so a failed `save` still cleans up the
    // partial file it may have created.
    let staged =
        StagedFrame { img: dir.join(format!("{stem}.png")), json: dir.join(format!("{stem}.json")) };
    small
        .to_rgb8()
        .save(&staged.img)
        .with_context(|| format!("stage embedding input {}", staged.img.display()))?;
    Ok(staged)
}

/// Hand a [`StagedFrame`] to the sidecar. Serialised process-wide against
/// every other embed (`embed::with_model_slot`) — one resident model, whatever
/// the caller's concurrency.
pub fn embed_staged(opts: &crate::embed::EmbedOpts, staged: &StagedFrame) -> Result<Vec<f32>> {
    crate::embed::embed_file(opts, &staged.img, &staged.json)
}

pub fn embed_staged_record(opts: &crate::embed::EmbedOpts, staged: &StagedFrame) -> Result<crate::embed::EmbedRecord> {
    crate::embed::embed_file_record(opts, &staged.img, &staged.json)
}

/// One photo's SigLIP 2 vector, from the camera's embedded preview.
///
/// The ONE entry point for a caller that has nothing useful to do between the
/// two halves — the develop-time query, which needs the vector before it can
/// build the advisor request at all. `StyleIndex::build` splits them instead,
/// so its decode buffer and its decode permit are released before the sidecar
/// runs.
///
/// Errors are the CALLER's to degrade: the build skips the vector for that
/// photo and keeps the exemplar, the develop path retrieves on the 14 dims
/// alone. Neither may fail the run — a style index is not worth an aborted
/// develop, and a 1.5 GB download must never be able to break one.
pub fn embed_preview(
    opts: &crate::embed::EmbedOpts,
    preview: &image::DynamicImage,
    dir: &Path,
    tag: &str,
) -> Result<Vec<f32>> {
    let staged = stage_embed_frame(preview, dir, tag)?;
    embed_staged(opts, &staged)
}

/// The text whose vector becomes [`StyleExemplar::desc_embed`]: the record's
/// own description when it has one, otherwise its tag string.
///
/// ONE rule, used by both builders, so a RAW exemplar and a look record can
/// never end up describing themselves in two different vocabularies. `None`
/// means there is nothing to embed — a record with no description and no tags
/// keeps `desc_embed: None`, and the W_DESC term is simply absent for it
/// (`embed_distance`'s existing rule).
/// The phrase-list scratch file for ONE builder in ONE process.
///
/// Named by `who` as well as the pid because both builders wrote
/// `autoshop-look-vocab-<pid>.txt` and both DELETED it when finished: two
/// builds in one process (the web server's request threads) shared a path, and
/// whichever finished first took the other's vocabulary out from under it. A
/// sequence number covers the same builder running twice.
fn vocab_scratch_path(dir: &Path, who: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    dir.join(format!(
        "autoshop-look-vocab-{who}-{}-{}.txt",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

fn desc_text(desc: Option<&str>, tags: &[String]) -> Option<String> {
    match desc.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => Some(d.chars().take(MAX_DESC_CHARS).collect()),
        None if !tags.is_empty() => Some(tags.join(", ")),
        None => None,
    }
}

/// One text vector per record, from ONE sidecar process.
///
/// This is the whole of F-12. The look build used to call the sidecar once PER
/// PHOTOGRAPH to embed that photograph's own tag string — 1.5 GB of weights
/// re-loaded per record — and the RAW build did not compute the vector at all,
/// so every one of its exemplars carried `desc_embed: None` and the W_DESC
/// term was dead. Here the whole build's texts go out as one JSONL manifest
/// and come back in order.
///
/// `texts[i] == None` contributes no line and gets no vector back; the answer
/// is mapped onto the records by position, and a batch whose length does not
/// line up is refused by [`crate::embed::parse_text_vectors`] rather than
/// zipped onto the wrong records.
fn embed_desc_texts(
    opts: &crate::embed::EmbedOpts,
    dir: &Path,
    texts: &[Option<String>],
) -> Result<Vec<Option<Vec<f32>>>> {
    let live: Vec<usize> = texts.iter().enumerate().filter(|(_, t)| t.is_some()).map(|(i, _)| i).collect();
    let mut out: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
    if live.is_empty() {
        return Ok(out);
    }
    std::fs::create_dir_all(dir)?;
    static TEXT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let stem = format!(
        "autoshop-embed-desc-{}-{}",
        std::process::id(),
        TEXT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let manifest = dir.join(format!("{stem}.jsonl"));
    let scratch = dir.join(format!("{stem}.json"));
    let body: String = live
        .iter()
        .map(|&i| serde_json::json!({ "text": texts[i].as_deref().unwrap_or_default() }).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&manifest, body + "\n")
        .with_context(|| format!("write description text manifest {}", manifest.display()))?;
    let vectors = crate::embed::embed_text_batch(opts, &manifest, &scratch, live.len());
    let _ = std::fs::remove_file(&manifest);
    for (slot, v) in live.into_iter().zip(vectors?) {
        out[slot] = Some(v);
    }
    Ok(out)
}

/// The two record shapes that carry a description vector, behind one door — so
/// the RAW builder and the look builder cannot drift into two rules about what
/// `desc_embed` holds.
trait DescribedRecord {
    fn desc(&self) -> Option<&str>;
    fn tags(&self) -> &[String];
    fn set_desc_embed(&mut self, v: Option<Vec<f32>>);
}

impl DescribedRecord for StyleExemplar {
    fn desc(&self) -> Option<&str> { self.desc.as_deref() }
    fn tags(&self) -> &[String] { &self.tags }
    fn set_desc_embed(&mut self, v: Option<Vec<f32>>) { self.desc_embed = v; }
}

impl DescribedRecord for LookExemplar {
    fn desc(&self) -> Option<&str> { self.desc.as_deref() }
    fn tags(&self) -> &[String] { &self.tags }
    fn set_desc_embed(&mut self, v: Option<Vec<f32>>) { self.desc_embed = v; }
}

/// Fill `desc_embed` for a whole build in ONE sidecar call.
///
/// DEGRADES, never fails: a text batch that could not run leaves the vectors
/// absent, the W_DESC term inert for this index, and one line on stderr saying
/// so — the same contract the image arm has kept since R27 Batch-5. An
/// hour-long index build must not be lost to the text half of it.
fn attach_desc_embeddings<R: DescribedRecord>(
    opts: &crate::embed::EmbedOpts,
    dir: &Path,
    records: &mut [R],
    what: &str,
) {
    if records.is_empty() {
        return;
    }
    let texts: Vec<Option<String>> =
        records.iter().map(|r| desc_text(r.desc(), r.tags())).collect();
    let live = texts.iter().filter(|t| t.is_some()).count();
    match embed_desc_texts(opts, dir, &texts) {
        Ok(vectors) => {
            for (record, vector) in records.iter_mut().zip(vectors) {
                record.set_desc_embed(vector);
            }
            if live > 0 {
                println!("  {what}: {live} description vector(s) in one text call");
            }
        }
        Err(e) => eprintln!(
            "  {what}: description text embedding unavailable ({e:#}) — the description \
             term stays inert for this index"
        ),
    }
}

/// The three things an index build needs from OUTSIDE itself: the two model
/// sidecars, and the directory it may write in.
///
/// A struct rather than three parameters because the SCRATCH DIRECTORY is the
/// load-bearing one and it used to be read from a global. `cargo test` runs
/// with `AUTOSHOP_DATA_DIR` pointing at a real store (and, without it, at
/// `%LOCALAPPDATA%/autoshop`), so a build driven by a test wrote its staged
/// frames — and, once S2 added one, its DESCRIPTION CACHE — into the user's
/// own store, where a later live build would have served the stub sentences
/// back. Observed, not theorised: a test run on 2026-08-30 left 16 entries of
/// `"a stubbed grade sentence"` in `%LOCALAPPDATA%/autoshop/style-descriptions.json`.
pub struct BuildSidecars {
    pub embed: crate::embed::EmbedOpts,
    pub describe: crate::describe::DescribeOpts,
    /// Where staged frames and the description cache live. Production passes
    /// [`crate::store::store_root`]; a test passes its own directory.
    pub scratch: PathBuf,
}

impl BuildSidecars {
    /// What the production callers use: both sidecars from [`Config`], and the
    /// per-user store as the scratch root.
    pub fn from_config(cfg: &crate::config::Config) -> Self {
        BuildSidecars {
            embed: crate::embed::EmbedOpts::from_config(cfg),
            describe: crate::describe::DescribeOpts::from_config(cfg),
            scratch: crate::store::store_root(),
        }
    }
}

/// Which phase of an index build a [`BuildProgress`] belongs to.
///
/// The build used to be one loop with one counter, because every model call
/// happened inside it. Since S2 it is four phases over the WHOLE library, and
/// only the first of them has per-record granularity — the other three are one
/// sidecar process each, and this process genuinely cannot see inside them. A
/// bare `(done, total)` would have had to either lie about that or reset to
/// zero three times with nothing to say why; the stage is what makes the pair
/// readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStage {
    /// Decode every source and stage its 512-px frame — the only phase this
    /// process runs itself, and the only one that reports per record.
    Frames,
    /// ONE SigLIP call over every staged frame (image vector + vocabulary
    /// scores).
    Embed,
    /// ONE Qwen call over the frames this machine has not already described.
    /// `done` at entry is the number the cache already answered.
    Describe,
    /// ONE SigLIP text call over every description-or-tag string.
    Text,
}

impl BuildStage {
    /// The stage's name for a UI, in English — the GUI puts it through `tr()`
    /// like every other string it shows.
    pub fn label(self) -> &'static str {
        match self {
            BuildStage::Frames => "decoding",
            BuildStage::Embed => "embedding",
            BuildStage::Describe => "describing",
            BuildStage::Text => "text vectors",
        }
    }
}

/// One progress report from an index build: which phase, and how far into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildProgress {
    pub stage: BuildStage,
    pub done: usize,
    pub total: usize,
}

fn report(on_progress: &dyn Fn(BuildProgress), stage: BuildStage, done: usize, total: usize) {
    on_progress(BuildProgress { stage, done, total });
}

/// ONE SigLIP image call for a whole build: N staged frames in, N records out,
/// in order.
///
/// `frames[i] == None` (this record's staging failed) contributes no manifest
/// line and gets `None` back, exactly like [`embed_desc_texts`]'s text arm.
/// The answer is mapped back by PATH rather than by position, because
/// `embed.py`'s batch door reports a malformed manifest line as soon as it
/// reads it and the rest in loop order — a positional zip would attach one
/// photograph's vector to another's exemplar.
fn embed_frames(
    opts: &crate::embed::EmbedOpts,
    dir: &Path,
    frames: &[Option<StagedFrame>],
    what: &str,
) -> Result<Vec<Option<crate::embed::EmbedRecord>>> {
    let mut out: Vec<Option<crate::embed::EmbedRecord>> = (0..frames.len()).map(|_| None).collect();
    let live: Vec<(usize, &StagedFrame)> =
        frames.iter().enumerate().filter_map(|(i, f)| f.as_ref().map(|f| (i, f))).collect();
    if live.is_empty() {
        return Ok(out);
    }
    std::fs::create_dir_all(dir)?;
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let stem = format!(
        "autoshop-embed-frames-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let manifest = dir.join(format!("{stem}.jsonl"));
    let scratch = dir.join(format!("{stem}.out.jsonl"));
    let body: String = live
        .iter()
        .map(|(_, f)| serde_json::json!({ "path": f.image_path() }).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&manifest, body + "\n")
        .with_context(|| format!("write {what} embedding manifest {}", manifest.display()))?;
    let answered = crate::embed::embed_image_batch(opts, &manifest, &scratch);
    let _ = std::fs::remove_file(&manifest);
    let answered = answered?;
    let by_path: std::collections::HashMap<&str, &crate::embed::EmbedBatchRecord> =
        answered.iter().map(|r| (r.path.as_str(), r)).collect();
    let mut ok = 0usize;
    for (slot, frame) in live {
        let key = frame.image_path();
        match by_path.get(key.as_str()) {
            Some(rec) => match &rec.record {
                Some(record) => {
                    out[slot] = Some(record.clone());
                    ok += 1;
                }
                None => eprintln!(
                    "  {what}: no style embedding for one frame ({}) — indexed on the 14-dim \
                     feature alone",
                    rec.error.as_deref().unwrap_or("the sidecar gave no reason")
                ),
            },
            None => eprintln!(
                "  {what}: the embedding sidecar answered nothing for one staged frame — that \
                 record is indexed on the 14-dim feature alone"
            ),
        }
    }
    println!("  {what}: {ok} image vector(s) in one sidecar call");
    Ok(out)
}

/// The two record shapes that carry a DESCRIPTION, behind one door — so the
/// RAW builder and the look builder cannot drift into two rules about what
/// `desc` holds. Sibling of [`DescribedRecord`], which owns the vector.
trait DescribableRecord {
    fn set_desc(&mut self, desc: Option<String>);
    fn has_desc(&self) -> bool;
}

impl DescribableRecord for StyleExemplar {
    fn set_desc(&mut self, desc: Option<String>) { self.desc = desc; }
    fn has_desc(&self) -> bool { self.desc.is_some() }
}

impl DescribableRecord for LookExemplar {
    fn set_desc(&mut self, desc: Option<String>) { self.desc = desc; }
    fn has_desc(&self) -> bool { self.desc.is_some() }
}

/// STAGE 3 of a build: fill `desc` for every record whose frame this machine
/// can describe, in ONE sidecar call.
///
/// DEGRADES, never fails, at every step — the same contract the embedding arm
/// has kept since R27 Batch-5, and for the same reason: an hour-long index
/// build must not be lost to an optional field. A missing switch, a missing
/// script, an unreadable frame, a sidecar that refused: each leaves `desc`
/// absent for the records it touched, prints one sentence saying so, and the
/// build carries on with the tags.
///
/// The CACHE is content-keyed ([`crate::describe::frame_digest`]), so a
/// library that gained one photograph describes one photograph. It is written
/// once, at the end, with the keys this build used — never per record, which
/// would publish a 78 MiB-capped file 169 times.
#[allow(clippy::too_many_arguments)] // one stage's whole input; see BuildSidecars
fn attach_descriptions<R: DescribableRecord>(
    opts: &crate::describe::DescribeOpts,
    describe: DescribeSwitch,
    dir: &Path,
    cache_path: &Path,
    frames: &[Option<StagedFrame>],
    records: &mut [R],
    what: &str,
    on_progress: &dyn Fn(BuildProgress),
) {
    if !describe.on() || records.is_empty() {
        return;
    }
    let total = records.len();
    if !opts.available() {
        eprintln!(
            "  look descriptions requested but the sidecar is not at {} — the index carries \
             attribute tags only (set AUTOSHOP_DESCRIBE_SCRIPT, or run from the project dir)",
            opts.script.display()
        );
        return;
    }
    println!(
        "  look descriptions ON ({}) — first run downloads ~4.3 GB of Qwen3-VL weights",
        opts.script.display()
    );
    let mut cache = crate::describe::DescriptionCache::load(cache_path);
    // The digest of every frame this build can describe, and the description
    // the cache already holds for it.
    let mut digests: Vec<Option<String>> = Vec::with_capacity(total);
    let mut hits = 0usize;
    for frame in frames.iter() {
        let Some(frame) = frame else {
            digests.push(None);
            continue;
        };
        match crate::describe::frame_digest(frame.image()) {
            Ok(d) => digests.push(Some(d)),
            Err(e) => {
                eprintln!("  {what}: one frame could not be hashed ({e:#}) — it is not described");
                digests.push(None);
            }
        }
    }
    for (record, digest) in records.iter_mut().zip(&digests) {
        if let Some(desc) = digest.as_deref().and_then(|d| cache.get(d)) {
            record.set_desc(Some(desc.to_string()));
            hits += 1;
        }
    }
    report(on_progress, BuildStage::Describe, hits, total);
    // The MISSES, in record order. A frame the cache already answered is not
    // sent again — that is the whole reason the cache exists.
    let misses: Vec<(usize, &StagedFrame)> = frames
        .iter()
        .enumerate()
        .filter(|(i, _)| digests[*i].is_some() && !records[*i].has_desc())
        .filter_map(|(i, f)| f.as_ref().map(|f| (i, f)))
        .collect();
    let mut keep: std::collections::BTreeSet<String> =
        digests.iter().flatten().cloned().collect();
    if misses.is_empty() {
        println!("  {what}: {hits} description(s), all from the content cache");
        report(on_progress, BuildStage::Describe, total, total);
        // Still republished: the cache's retention set is what this build
        // used, so a build that hit 100 % keeps those entries alive.
        if let Err(e) = cache.save(cache_path, &keep) {
            eprintln!("  {what}: the description cache could not be published ({e:#})");
        }
        return;
    }
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let stem = format!(
        "autoshop-describe-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let manifest = dir.join(format!("{stem}.jsonl"));
    let scratch = dir.join(format!("{stem}.out.jsonl"));
    let body: String = misses
        .iter()
        .map(|(_, f)| serde_json::json!({ "path": f.image_path() }).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    if let Err(e) = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&manifest, body + "\n")) {
        eprintln!("  {what}: the description manifest could not be written ({e}) — no prose this build");
        return;
    }
    let answered = crate::describe::describe_manifest(opts, &manifest, &scratch);
    let _ = std::fs::remove_file(&manifest);
    let answered = match answered {
        Ok(a) => a,
        Err(e) => {
            eprintln!(
                "  {what}: look descriptions unavailable ({e:#}) — the index carries attribute \
                 tags only"
            );
            report(on_progress, BuildStage::Describe, total, total);
            return;
        }
    };
    let by_path: std::collections::HashMap<&str, &crate::describe::DescribeRecord> =
        answered.iter().map(|r| (r.path.as_str(), r)).collect();
    let mut fresh = 0usize;
    let mut refused = 0usize;
    for (slot, frame) in misses {
        let key = frame.image_path();
        match by_path.get(key.as_str()).and_then(|r| r.desc.clone()) {
            Some(desc) => {
                if let Some(d) = digests[slot].clone() {
                    cache.insert(d.clone(), desc.clone());
                    keep.insert(d);
                }
                records[slot].set_desc(Some(desc));
                fresh += 1;
            }
            None => refused += 1,
        }
    }
    if refused > 0 {
        eprintln!(
            "  {what}: {refused} frame(s) got no description — those exemplars carry their \
             attribute tags alone"
        );
    }
    println!("  {what}: {fresh} new description(s) in one sidecar call, {hits} from the cache");
    if let Err(e) = cache.save(cache_path, &keep) {
        eprintln!("  {what}: the description cache could not be published ({e:#})");
    }
    report(on_progress, BuildStage::Describe, total, total);
}

pub fn embed_preview_with_text(
    opts: &crate::embed::EmbedOpts,
    preview: &image::DynamicImage,
    dir: &Path,
    tag: &str,
    text: Option<&str>,
) -> Result<crate::embed::EmbedRecord> {
    let staged = stage_embed_frame(preview, dir, tag)?;
    let mut o = crate::embed::EmbedOpts { python_bin: opts.python_bin.clone(), script: opts.script.clone(), text_file: None, vocab_file: opts.vocab_file.clone() };
    let text_path = if let Some(t) = text.filter(|s| !s.trim().is_empty()) {
        static TEXT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = dir.join(format!("autoshop-embed-{}-{}-{tag}.txt", std::process::id(), TEXT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::write(&p, t)?;
        o.text_file = Some(p.clone());
        Some(p)
    } else { None };
    let result = embed_staged_record(&o, &staged);
    if let Some(p) = text_path { let _ = std::fs::remove_file(p); }
    result
}

/// The ONE degradation line the index build prints when a photo ends up
/// without a vector — shared by the staging arm and the sidecar arm so the two
/// failures read identically to the user, who cannot tell them apart and does
/// not need to.
fn note_no_embedding(raw: &Path, e: &anyhow::Error) {
    eprintln!(
        "  {}: no style embedding ({e:#}) — indexed on the 14-dim feature alone",
        pipeline::stem(raw)
    );
}

/// Is this exemplar the QUERY photo itself? Path identity when the exemplar
/// records one (case-folded on Windows, like `store::photo_key`); stem
/// fallback for pre-path indexes — there, over-exclusion is the safe
/// direction (an unrelated same-stem exemplar loses one retrieval slot; a
/// self-reference would teach the AI to copy the photo's own edit back).
fn is_self(e: &StyleExemplar, query_path: &str, query_stem: &str) -> bool {
    match &e.path {
        Some(p) => {
            if cfg!(windows) {
                p.to_lowercase() == query_path.to_lowercase()
            } else {
                p == query_path
            }
        }
        None => e.stem == query_stem,
    }
}

/// Every number an exemplar carries must be FINITE: serde_json writes a NaN
/// as `null`, and the next LOAD of the index then fails wholesale — one bad
/// EXIF field or curve point published an UNLOADABLE index. Features are
/// additionally BOUNDED ([`MAX_FEATURE_ABS`], L04-3): finite-but-huge
/// values overflow the z-score arithmetic downstream, and no legitimately
/// built index can produce one (every dim is a ln or a bounded ratio) —
/// only a tampered or bit-rotted file on disk, this file's own stated
/// threat model ("invariants at the door").
fn exemplar_is_finite(e: &StyleExemplar) -> bool {
    e.feat.iter().all(|v| v.is_finite() && v.abs() <= MAX_FEATURE_ABS)
        && e.settings.values().all(|v| v.is_finite())
        && e.curve.is_none_or(|c| c.iter().all(|v| v.is_finite()))
        && e.families.is_none_or(|f| f.is_finite())
        // The embedding gets a TIGHTER band than the 14 dims, for free: it is
        // L2-normalised by construction, so every element is in [-1, 1] and
        // the whole vector's norm is 1. Checking the elements alone would
        // still admit a 768-dim vector of 1.0s (norm 27.7), which the cosine
        // would then treat as overwhelmingly similar to everything. The width
        // is pinned too — a foreign width is silently ignored by
        // `embed_distance`, and an index that carries one should say so at the
        // door instead of retrieving as if the block were absent.
        && e.embed.as_ref().is_none_or(|v| {
            v.len() == crate::embed::EMBED_DIM
                && v.iter().all(|x| x.is_finite() && x.abs() <= 1.0 + 1e-3)
                && (v.iter().map(|&x| x as f64 * x as f64).sum::<f64>().sqrt() - 1.0).abs() < 1e-3
        })
        && e.vocab_scores.as_ref().is_none_or(|v| v.len() == LOOK_VOCAB.len() && v.iter().all(|x| x.is_finite()))
        && e.desc.as_ref().is_none_or(|d| d.chars().count() <= MAX_DESC_CHARS)
        && e.desc_embed.as_ref().is_none_or(|v| {
            v.len() == crate::embed::EMBED_DIM
                && v.iter().all(|x| x.is_finite() && x.abs() <= 1.0 + 1e-3)
                && (v.iter().map(|&x| x as f64 * x as f64).sum::<f64>().sqrt() - 1.0).abs() < 1e-3
        })
}

/// How many comparable candidates a query needs before its text terms are
/// standardised. Below this the mean and σ of the candidate set are not an
/// estimate of anything, so the raw gap is used and the fact is disclosed.
const MIN_STANDARDISATION_CANDIDATES: usize = 3;

/// One cosine term for every candidate, standardised over the candidate set.
///
/// **Why the text terms need this and the image term does not.** SigLIP's
/// image↔image cosines spread across the library the way a distance should;
/// its image↔TEXT cosines do not. Measured on the shipped index, a direction's
/// text vector scores 0.02–0.03 against every neighbour, so `w·(1−cos)` sat at
/// 7.72–7.85 for all four of them: a spread of 0.13 against a 14-dim spread of
/// 2.5. The term was numerically enormous and informationally inert, and a
/// calibration over it would "find" W_TXT ≈ 0 for the wrong reason — not
/// because text says nothing, but because its raw scale hides what it says.
///
/// The z-score is an AFFINE map of the raw gap within one query, so it adds no
/// ordering power the raw term did not have; what it does is put the term on
/// the 14-dim block's scale, which is what makes the weight a measurable
/// quantity rather than a unit conversion. It is per QUERY, never global: the
/// absolute level of a text cosine is a property of the phrase, not of the
/// photograph, and only the ordering across candidates is evidence.
///
/// A candidate with no vector scores 0 — the candidate-set MEAN on this scale,
/// i.e. "no evidence", the same reading the raw term's 0 had.
struct StandardisedTerm {
    terms: Vec<f64>,
    standardised: bool,
}

/// The weighted RAW gaps — `w · (1 − cos)`, and an exact zero where the pair
/// was not comparable or the weight is off.
fn raw_term(raw: &[Option<f64>], w: f64) -> StandardisedTerm {
    // A zero weight is the term's ABSENCE, bit for bit — never `0.0 * z`,
    // which would be a signed zero riding on the sum.
    if w == 0.0 {
        return StandardisedTerm { terms: vec![0.0; raw.len()], standardised: false };
    }
    StandardisedTerm { terms: raw.iter().map(|r| r.map_or(0.0, |v| w * v)).collect(), standardised: false }
}

/// The term the RANKING uses: [`standardise`] or [`raw_term`], per
/// [`STANDARDISE_TEXT_TERMS`]. One door, so the diagnostic and the pipeline
/// cannot disagree about which variant is in force.
fn text_term(raw: &[Option<f64>], w: f64) -> StandardisedTerm {
    if STANDARDISE_TEXT_TERMS { standardise(raw, w) } else { raw_term(raw, w) }
}

/// `raw[i] = Some(1 − cos)` when the pair is comparable, `None` when it is not.
fn standardise(raw: &[Option<f64>], w: f64) -> StandardisedTerm {
    if w == 0.0 {
        return raw_term(raw, w);
    }
    let plain = || raw_term(raw, w);
    let live: Vec<f64> = raw.iter().flatten().copied().collect();
    if live.len() < MIN_STANDARDISATION_CANDIDATES {
        return plain();
    }
    let n = live.len() as f64;
    let mean = live.iter().sum::<f64>() / n;
    let sd = (live.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / n).sqrt();
    // NaN is caught by the finiteness test, so the second comparison is a
    // plain one (clippy::neg_cmp_op_on_partial_ord: `!(sd > 0.0)` also
    // swallowed NaN, silently and by accident).
    if !sd.is_finite() || sd <= 0.0 {
        return plain();
    }
    StandardisedTerm {
        terms: raw.iter().map(|r| r.map_or(0.0, |v| w * (v - mean) / sd)).collect(),
        standardised: true,
    }
}

/// The additive components of one candidate's distance, as the retrieval
/// computed them — so a diagnostic can print the production numbers instead of
/// maintaining a second ranking path.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DistanceTerms {
    /// The 14-dim hand-feature block (0 for a look record, which has none).
    pub d14: f64,
    /// `W_EMB·(1−cos(q_img, e_img))` — raw, never standardised (its cosines are
    /// already spread). For a look record this is the `W_LOOK` term.
    pub emb: f64,
    /// The standardised direction-text ↔ exemplar-image term.
    pub txt: f64,
    /// The standardised direction-text ↔ exemplar-description term.
    pub desc: f64,
    /// The raw `1−cos` behind [`Self::txt`], when the pair was comparable.
    pub txt_gap: Option<f64>,
    /// The raw `1−cos` behind [`Self::desc`], when the pair was comparable.
    pub desc_gap: Option<f64>,
    /// False when the candidate set was too small (or degenerate) to
    /// standardise and the raw gap was used — disclosed, never silent.
    pub txt_standardised: bool,
    pub desc_standardised: bool,
}

impl DistanceTerms {
    pub fn total(&self) -> f64 {
        self.d14 + self.emb + self.txt + self.desc
    }
}

fn tags_from_scores(scores: &[f32]) -> Vec<String> {
    if scores.len() != LOOK_VOCAB.len() { return Vec::new(); }
    let mut chosen = Vec::new();
    for group in LOOK_GROUPS {
        if let Some(&idx) = group.iter().max_by(|&&a, &&b| scores[a].total_cmp(&scores[b])) {
            chosen.push((scores[idx], LOOK_VOCAB[idx]));
        }
    }
    chosen.sort_by(|a,b| b.0.total_cmp(&a.0));
    chosen.into_iter().take(LOOK_TAGS_K).map(|(_, s)| s.strip_prefix("a photo with ").or_else(|| s.strip_prefix("an ")).unwrap_or(s).to_string()).collect()
}

#[derive(Serialize, Deserialize, Clone)]
pub struct StyleIndex {
    pub version: u32,
    pub mean: Vec<f32>,
    pub std: Vec<f32>,
    pub exemplars: Vec<StyleExemplar>,
    /// Absolute folder this index was built from (the user's edited-RAW library),
    /// so the UI can show its provenance. `#[serde(default)]` keeps old index
    /// files (written before this field) loadable.
    #[serde(default)]
    pub source_dir: Option<String>,
    #[serde(default)]
    pub looks: Vec<LookExemplar>,
    #[serde(default)]
    pub looks_dir: Option<String>,
    #[serde(default)]
    pub embed_provenance: Option<String>,
}

impl StyleIndex {
    /// Scan a folder for RAW+.xmp pairs (the user's own edits) and build the
    /// index, reporting nothing but the historical stdout lines.
    pub fn build(dir: &Path, embed: EmbeddingSwitch, describe: DescribeSwitch) -> Result<StyleIndex> {
        Self::build_reporting(dir, embed, describe, &|_| {})
    }

    /// Build the finished-photo look library. Looks are embedding-only and
    /// never participate in settings targets or recipe blending.
    pub fn build_looks(
        dir: &Path,
        embed: EmbeddingSwitch,
        describe: DescribeSwitch,
        on_progress: &dyn Fn(BuildProgress),
    ) -> Result<StyleIndex> {
        Self::build_looks_with(
            BuildSidecars::from_config(&crate::config::Config::load()),
            dir,
            embed,
            describe,
            on_progress,
        )
    }

    /// [`build_looks`](Self::build_looks) over an explicit sidecar
    /// configuration — the seam its refusal test drives.
    ///
    /// The test used to point `AUTOSHOP_EMBED_SCRIPT` at a nonexistent file
    /// with an unsafe environment write and put it back afterwards. `cargo test` runs on
    /// parallel threads in one process, so for the duration of that test EVERY
    /// other test's idea of where the sidecar lives was wrong. Same rule as the
    /// switch and the weights: pass the value.
    pub fn build_looks_with(
        sidecars: BuildSidecars,
        dir: &Path,
        embed: EmbeddingSwitch,
        describe: DescribeSwitch,
        on_progress: &dyn Fn(BuildProgress),
    ) -> Result<StyleIndex> {
        let BuildSidecars { embed: opts, describe: describe_opts, scratch } = sidecars;
        if !opts.available() || !embed.on() {
            anyhow::bail!("look library requires the style-embedding sidecar; enable embedding and rebuild")
        }
        let mut files = Vec::new();
        for entry in walkdir(dir)? {
            if pipeline::BAKED_EXTS.iter().any(|e| entry.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case(e))) {
                files.push(entry);
            }
        }
        files.sort();
        if files.is_empty() { anyhow::bail!("look library contains no finished photos") }
        // Refused BEFORE the first decode, not after an hour of them: the file
        // cap is derived from this number (`MAX_LOOK_EXEMPLARS`), and a build
        // that overran it would produce an index its own loader refuses.
        if files.len() > MAX_LOOK_EXEMPLARS {
            anyhow::bail!(
                "{} holds {} finished photos and the look library caps at {} — point it at a \
                 curated folder of reference grades, not a whole archive",
                dir.display(), files.len(), MAX_LOOK_EXEMPLARS
            )
        }
        let total = files.len();
        report(on_progress, BuildStage::Frames, 0, total);
        std::fs::create_dir_all(&scratch)?;
        let vocab_path = vocab_scratch_path(&scratch, "looks");
        std::fs::write(&vocab_path, LOOK_VOCAB.join("\n"))?;
        let mut opts = opts;
        opts.vocab_file = Some(vocab_path.clone());
        // STAGE 1 — decode every finished photo and stage its frame. Nothing
        // else: the model stages below each run ONCE for the whole library.
        let mut looks = Vec::new();
        let mut frames: Vec<Option<StagedFrame>> = Vec::new();
        for (i, path) in files.iter().enumerate() {
            let _permit = decode::DecodePermit::acquire();
            let decoded = decode::decode_any(path).with_context(|| format!("decode look {}", path.display()))?;
            let frame = stage_embed_frame(&decoded.preview, &scratch, &format!("look-{i}"))?;
            drop(_permit);
            let path_abs = std::path::absolute(path)?.display().to_string();
            looks.push(LookExemplar {
                stem: pipeline::stem(path).to_string(),
                path: path_abs,
                embed: Vec::new(),
                tags: Vec::new(),
                vocab_scores: None,
                desc: None,
                desc_embed: None,
            });
            frames.push(Some(frame));
            report(on_progress, BuildStage::Frames, i + 1, total);
        }
        // STAGE 2 — ONE SigLIP image call for the whole library. A look
        // WITHOUT a vector is not a look (the library is embedding-only), so
        // unlike the RAW build this one fails the record rather than degrading
        // it.
        report(on_progress, BuildStage::Embed, 0, total);
        let vectors = embed_frames(&opts, &scratch, &frames, "look library")?;
        let _ = std::fs::remove_file(vocab_path);
        for (i, (look, record)) in looks.iter_mut().zip(&vectors).enumerate() {
            let record = record.as_ref().ok_or_else(|| {
                anyhow::anyhow!("embed look {}: the sidecar returned no vector", files[i].display())
            })?;
            look.embed = record.vector.clone();
            look.tags = record.vocab_scores.as_deref().map(tags_from_scores).unwrap_or_default();
            look.vocab_scores = record.vocab_scores.clone();
        }
        report(on_progress, BuildStage::Embed, total, total);
        // STAGE 3 — ONE Qwen call over the frames that are not already
        // described, then STAGE 4, ONE SigLIP text call over the whole set.
        attach_descriptions(
            &describe_opts,
            describe,
            &scratch,
            &crate::describe::cache_path_in(&scratch),
            &frames,
            &mut looks,
            "look library",
            on_progress,
        );
        drop(frames);
        report(on_progress, BuildStage::Text, 0, total);
        attach_desc_embeddings(&opts, &scratch, &mut looks, "look library");
        report(on_progress, BuildStage::Text, total, total);
        Ok(StyleIndex { version: CURRENT_INDEX_VERSION, mean: vec![0.0; NDIM], std: vec![1.0; NDIM], exemplars: Vec::new(), source_dir: None, looks, looks_dir: std::path::absolute(dir).ok().map(|p| p.display().to_string()), embed_provenance: Some(embed_provenance_string()) })
    }

    /// [`build`](StyleIndex::build) with a progress callback — one
    /// [`BuildProgress`] per record inside the decode stage, and one at each
    /// end of every model stage.
    ///
    /// Called on the CALLER's thread (from the result-collector loop), not
    /// from a decode worker: that keeps the callback free of `Send + Sync`
    /// bounds, which matters because the GUI's callback carries an mpsc
    /// `Sender` (`Send`, NOT `Sync`). The stdout lines stay in the workers,
    /// byte-identical to what the CLI has always printed.
    ///
    /// **STAGES, not per-photo model calls (step 14 / S2).** Until this batch
    /// the image half of the embedding ran ONE SIDECAR PROCESS PER PHOTOGRAPH:
    /// 169 loads of a 1.50 GB checkpoint for the photographer's own library,
    /// measured at 5,618 s. `embed.py --manifest-jsonl` has always embedded N
    /// images in one process (the TEXT half already went out as one batch in
    /// S1-fix F-12), so the build is now four phases over the WHOLE library —
    /// decode+stage every frame, ONE SigLIP image call, ONE Qwen description
    /// call, ONE SigLIP text call. One process per model per build; the model
    /// slot ([`crate::with_model_slot`]) still guarantees one resident model at
    /// a time, and the per-record fail-soft contract is unchanged: a
    /// photograph whose frame or vector failed keeps its 14-dim exemplar and
    /// says why.
    pub fn build_reporting(
        dir: &Path,
        embed: EmbeddingSwitch,
        describe: DescribeSwitch,
        on_progress: &dyn Fn(BuildProgress),
    ) -> Result<StyleIndex> {
        // R27 P1, deliberate NON-action: RAW-only, for `eval`'s reason (see
        // that call site) plus one of its own. The index learns the user's
        // style from RAW+sidecar pairs, and its exemplars are compared against
        // the CAMERA's own rendition — `decode::embedded_preview`, the one
        // door that keeps the strict "camera pixels or nothing" contract. A
        // baked source has no camera rendition at all, so it could contribute
        // an exemplar to the index but never a comparable one.
        let raws = pipeline::find_raws(dir)?;
        let pairs: Vec<_> = raws.iter().filter(|r| r.with_extension("xmp").exists()).collect();
        println!("building style index from {} RAW+.xmp pairs ...", pairs.len());
        // Decode in parallel: each pair pays ~1s of full-res embedded-JPEG decode
        // and the old serial scan left every other core idle (a 2000-pair library
        // took the better part of an hour). The worker count IS the process-wide
        // decode cap (decode::MAX_CONCURRENT_DECODES; each in-flight decode can
        // hold ~180 MB), and each decode also takes a DecodePermit — so two
        // concurrent builds (the web server's request threads) share one budget
        // instead of stacking to ~1.4 GB, while a single build never blocks. An
        // atomic counter hands out indices, and each result lands in its own
        // slot so the exemplar ORDER stays identical to the serial version.
        use std::sync::atomic::{AtomicUsize, Ordering};
        let cfg = crate::config::Config::load();
        // The two sidecars, resolved ONCE before the pool starts. `None` when
        // the user has not asked for one, or when its script is not on disk —
        // announced here, not discovered 169 times.
        let mut embedder: Option<crate::embed::EmbedOpts> = embed
            .on()
            .then(|| crate::embed::EmbedOpts::from_config(&cfg))
            .filter(|o| {
                if o.available() {
                    println!(
                        "  style embedding ON ({}) — first run downloads ~1.50 GB of SigLIP 2 \
                         weights",
                        o.script.display()
                    );
                    true
                } else {
                    eprintln!(
                        "  style embedding requested but the sidecar is not at {} — building the \
                         14-dim index only (set AUTOSHOP_EMBED_SCRIPT, or run from the project dir)",
                        o.script.display()
                    );
                    false
                }
            });
        let describer = crate::describe::DescribeOpts::from_config(&cfg);
        let embed_dir = crate::store::store_root();
        let vocab_path = vocab_scratch_path(&embed_dir, "raw");
        if let Some(opts) = embedder.as_mut()
            && std::fs::create_dir_all(&embed_dir).is_ok()
            && std::fs::write(&vocab_path, LOOK_VOCAB.join("\n")).is_ok() {
                opts.vocab_file = Some(vocab_path.clone());
        }
        let total = pairs.len();
        let mut slots: Vec<Option<(StyleExemplar, Option<StagedFrame>)>> = Vec::new();
        slots.resize_with(total, || None);
        let next = AtomicUsize::new(0);
        let done = AtomicUsize::new(0);
        let workers = total.clamp(1, decode::MAX_CONCURRENT_DECODES);
        report(on_progress, BuildStage::Frames, 0, total);
        std::thread::scope(|s| {
            let (tx, rx) = std::sync::mpsc::channel();
            for _ in 0..workers {
                let tx = tx.clone();
                let (pairs, next, done) = (&pairs, &next, &done);
                let (embedder, embed_dir) = (&embedder, &embed_dir);
                s.spawn(move || loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&raw) = pairs.get(i) else { break };
                    // --- UNDER THE DECODE PERMIT: the decode itself, and the
                    // staging of the embedding frame FROM the decoded buffer.
                    // Nothing else. R28 Batch-4 4b (adjudication F3) shortened
                    // this scope from what it used to be — it ran to the end of
                    // the photo, so a non-decode process (the sidecar, seconds
                    // of model load) sat inside the DECODE budget and the
                    // ~181 MB preview stayed alive underneath it. Since S2 no
                    // sidecar runs in this loop at all.
                    //
                    // The staging stays INSIDE, deliberately: it reads the
                    // preview, so releasing first would let this worker hold
                    // 181 MB while another started a fresh decode — and across
                    // two concurrent builds (the web server's request threads)
                    // that is exactly the ~1.4 GB stack `MAX_CONCURRENT_DECODES`
                    // exists to bound. Inside, the invariant is unchanged: a
                    // 61 MP preview is alive only while its permit is.
                    let permit = decode::DecodePermit::acquire();
                    // Destructured, not bound whole: bound as `d`, the entire
                    // `Decoded` — rawler's sensor buffers included — would stay
                    // alive across the staging below, when all this loop wants
                    // is `meta`, `histogram` and (~181 MB at 61 MP) `preview`.
                    // In the frame the photo's own saved develop asks for
                    // (R27): the aspect feature is a retrieval discriminator
                    // with weight 1.5, so indexing a hand-rotated shot as the
                    // landscape it no longer is biases every neighbour it
                    // answers. `saved_quarter_turns` is one small read beside
                    // a ~1 s decode, and answers 0 for the common case (a
                    // foreign Lightroom library this app has never developed).
                    let staged = match decode::decode_raw_turned(
                        raw,
                        crate::store::saved_quarter_turns(raw),
                    ) {
                        Ok(decode::Decoded { meta, histogram, preview, .. }) => {
                            let feat = feature_vector(&meta, &histogram);
                            // 181 MB in, 200 KB out: after this the sidecars
                            // need a PATH, not the buffer.
                            let frame = embedder.as_ref().and_then(|_| {
                                match stage_embed_frame(&preview, embed_dir, &format!("idx-{i}")) {
                                    Ok(f) => Some(f),
                                    // DEGRADE, never fail — see the sidecar arm
                                    // below; the two failures are one outcome.
                                    Err(e) => {
                                        note_no_embedding(raw, &e);
                                        None
                                    }
                                }
                            });
                            drop(preview);
                            Some((feat, frame))
                        }
                        Err(e) => {
                            // println!/eprintln! are line-atomic, so per-photo
                            // prints stay whole across workers.
                            eprintln!("  skip {}: {e}", pipeline::stem(raw));
                            None
                        }
                    };
                    // --- OUT of the decode budget. Everything below is a
                    // small file read; the model sidecars run once each, after
                    // the pool has finished.
                    drop(permit);
                    let ex = staged.and_then(|(feat, frame)| {
                        // An unreadable sidecar must SKIP the photo, not
                        // produce a settings-free exemplar that dilutes
                        // retrieval (the pair scan guaranteed the .xmp
                        // exists — a read failure here is a real error).
                        match crate::store::read_sidecar(&raw.with_extension("xmp")) {
                            Some(xmp) => {
                                let ex = StyleExemplar {
                                    stem: pipeline::stem(raw).to_string(),
                                    path: std::path::absolute(raw)
                                        .ok()
                                        .map(|p| p.display().to_string()),
                                    tag: derive_tag(&feat),
                                    feat: feat.to_vec(),
                                    settings: read_settings(&xmp),
                                    curve: crate::eval::user_curve_shape(&xmp)
                                        .map(|(b, s)| [b, s]),
                                    families: crate::eval::user_family_summary(&xmp),
                                    embed: None,
                                    tags: Vec::new(),
                                    vocab_scores: None,
                                    desc: None,
                                    desc_embed: None,
                                };
                                if exemplar_is_finite(&ex) {
                                    Some((ex, frame))
                                } else {
                                    eprintln!(
                                        "  skip {}: non-finite or out-of-band metadata/settings \
                                         (would corrupt the index)",
                                        pipeline::stem(raw)
                                    );
                                    None
                                }
                            }
                            None => {
                                eprintln!(
                                    "  skip {}: xmp unreadable or over the sidecar size cap",
                                    pipeline::stem(raw)
                                );
                                None
                            }
                        }
                    });
                    // Progress counts COMPLETED photos (completion order differs
                    // from index order under parallelism) so it stays monotonic.
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if n % 20 == 0 {
                        println!("  {} / {}", n, total);
                    }
                    let _ = tx.send((i, ex));
                });
            }
            drop(tx); // workers hold the remaining senders; rx ends when they exit
            let mut received = 0usize;
            for (i, ex) in rx {
                slots[i] = ex;
                received += 1;
                report(on_progress, BuildStage::Frames, received, total);
            }
        });
        // Failed decodes left None slots — drop them in order, like the serial
        // `continue` did.
        let (mut exemplars, frames): (Vec<StyleExemplar>, Vec<Option<StagedFrame>>) =
            slots.into_iter().flatten().unzip();
        let live = exemplars.len();
        if let Some(opts) = embedder.as_ref() {
            // STAGE 2 — ONE SigLIP image call for the whole library.
            report(on_progress, BuildStage::Embed, 0, live);
            match embed_frames(opts, &embed_dir, &frames, "style index") {
                Ok(records) => {
                    for (ex, record) in exemplars.iter_mut().zip(records) {
                        // DEGRADE, never fail: a photo without a vector keeps
                        // its 14-dim exemplar and the index becomes a
                        // legitimate mixed one (see `retrieve_with_embed`). The
                        // GUI COUNTS these (R28 4b) — an all-failed build used
                        // to land the same toast as an all-embedded one.
                        let Some(record) = record else { continue };
                        ex.tags = record.vocab_scores.as_deref().map(tags_from_scores).unwrap_or_default();
                        ex.vocab_scores = record.vocab_scores;
                        ex.embed = Some(record.vector);
                    }
                }
                Err(e) => eprintln!(
                    "  style index: no style embeddings ({e:#}) — every exemplar is indexed on \
                     the 14-dim feature alone"
                ),
            }
            report(on_progress, BuildStage::Embed, live, live);
            // STAGE 3 — ONE Qwen call over the frames that are not already
            // described.
            attach_descriptions(
                &describer,
                describe,
                &embed_dir,
                &crate::describe::cache_path_in(&embed_dir),
                &frames,
                &mut exemplars,
                "style index",
                on_progress,
            );
            // STAGE 4 — ONE SigLIP text call for the whole library. It runs
            // AFTER the two above because the text it embeds is what they just
            // produced, and because one call is the entire point.
            report(on_progress, BuildStage::Text, 0, live);
            attach_desc_embeddings(opts, &embed_dir, &mut exemplars, "style index");
            report(on_progress, BuildStage::Text, live, live);
        }
        drop(frames);
        let (mean, std) = compute_norm(&exemplars);
        // Record where this index was built from, for UI provenance / other users.
        let source_dir = std::path::absolute(dir).map(|p| p.display().to_string()).ok();
        // v2: exemplars now carry tint/saturation/dehaze + tone-curve shape.
        let _ = std::fs::remove_file(&vocab_path);
        let embed_provenance = embedder
            .as_ref()
            .filter(|_| exemplars.iter().any(|e| e.embed.is_some()))
            .map(|_| embed_provenance_string());
        Ok(StyleIndex { version: CURRENT_INDEX_VERSION, mean, std, exemplars, source_dir, looks: Vec::new(), looks_dir: None, embed_provenance })
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        // An empty index is a FAILED build, not a result: `fs::write`
        // truncates in place, so saving it would silently destroy a good index
        // that took an hour to build (every surface's Style slider then goes
        // inert with nothing to say why). The web handler had this guard; the
        // CLI didn't — enforcing it HERE covers every caller for good.
        if self.exemplars.is_empty() && self.looks.is_empty() {
            anyhow::bail!(
                "refusing to save an EMPTY style index over {} — no RAW had its .xmp sidecar \
                 beside it (Autoshop keeps its own .xmp in the develop store, never beside your \
                 RAWs, so an Autoshop output folder always yields 0). Point the build at your \
                 Lightroom-edited folder; the existing index was left untouched.",
                path.display()
            );
        }
        pipeline::ensure_parent(path)?;
        let mut value = self.clone();
        if self.exemplars.is_empty() || self.looks.is_empty() {
            match Self::load(path) {
                Ok(existing) => {
                    if value.exemplars.is_empty() { value.exemplars = existing.exemplars; value.mean = existing.mean; value.std = existing.std; value.source_dir = existing.source_dir; }
                    if value.looks.is_empty() { value.looks = existing.looks; value.looks_dir = existing.looks_dir; }
                }
                Err(err) if path.exists() => {
                    eprintln!("existing style index {} is unusable ({err:#}); replacing it", path.display());
                }
                Err(_) => {}
            }
        }
        // Publish atomically (tmp + rename): fs::write truncates in place, so
        // a disk-full/interrupt mid-write left the previous good index as a
        // corrupt partial file — the empty-index guard above can't catch that.
        // pid+seq-unique tmp: a pid-only name still collided between two
        // concurrent builders in the SAME process (the web server's threads),
        // and a FIXED name let cross-process builders truncate each other.
        static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let tmp = path.with_extension(format!(
            "json.tmp{}-{}",
            std::process::id(),
            TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&tmp, serde_json::to_string(&value)?)
            .with_context(|| format!("write style index {}", tmp.display()))?;
        // NO pre-remove: rename replaces the destination on Windows too, and
        // deleting first meant a failed rename (or a crash between the two)
        // had already destroyed the last good index.
        // DURABLE replace (L03): staged-bytes fsync + parent-dir fsync
        // around the rename — tmp+rename alone leaves a post-crash window
        // where the live name points at bytes the disk never received,
        // which is exactly the corrupt-index state this staging exists to
        // prevent.
        if let Err(e) = crate::store::durable_replace(&tmp, path) {
            let _ = std::fs::remove_file(&tmp); // don't leak the staging file
            return Err(e)
                .with_context(|| format!("publish style index {}", path.display()));
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<StyleIndex> {
        use std::io::Read as _;

        let file = std::fs::File::open(path)
            .with_context(|| format!("read style index {}", path.display()))?;
        let mut bytes = Vec::new();
        file.take((MAX_STYLE_INDEX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .with_context(|| format!("read style index {}", path.display()))?;
        if bytes.len() > MAX_STYLE_INDEX_BYTES {
            anyhow::bail!(
                "style index {} exceeds the {}-byte limit",
                path.display(),
                MAX_STYLE_INDEX_BYTES
            );
        }
        let text = std::str::from_utf8(&bytes)
            .with_context(|| format!("style index {} is not UTF-8", path.display()))?;
        let mut idx: StyleIndex = serde_json::from_str(text)
            .with_context(|| format!("parse style index {}", path.display()))?;
        // Version gate: v3 = display-frame Meta dims (the portrait feature).
        // An older index recorded portrait RAWs as landscape — serving it
        // silently would bias every retrieval until a manual rebuild.
        //
        // v5 does NOT refuse v4 (see `READABLE_INDEX_VERSIONS`): the embedding
        // block is additive and an absent one contributes exactly nothing, so
        // a v4 index still ranks the way it always did. What is refused is a
        // version whose 14 FEATURES mean something else.
        if !READABLE_INDEX_VERSIONS.contains(&idx.version) {
            anyhow::bail!(
                "style index {} is version {} (this build reads {:?}) — rebuild it: \
                 autoshop style-index <dir>",
                path.display(),
                idx.version,
                READABLE_INDEX_VERSIONS
            );
        }
        if idx.exemplars.is_empty() && idx.looks.is_empty() {
            anyhow::bail!("style index {} contains no exemplars", path.display());
        }
        if idx.exemplars.len() > MAX_STYLE_EXEMPLARS {
            anyhow::bail!(
                "style index {} contains {} exemplars (limit {})",
                path.display(),
                idx.exemplars.len(),
                MAX_STYLE_EXEMPLARS
            );
        }
        if idx.looks.len() > MAX_LOOK_EXEMPLARS {
            anyhow::bail!(
                "style index {} contains {} look exemplars (limit {})",
                path.display(), idx.looks.len(), MAX_LOOK_EXEMPLARS
            );
        }
        // The stored vocabulary version, ENFORCED. It was recorded in
        // `embed_provenance` from the start and checked nowhere: a phrase list
        // that changed meaning would have left every `vocab_scores` array (and
        // so every derived tag, and so every description vector) describing a
        // vocabulary this build no longer has, while the file looked fine.
        //
        // The LOOKS are dropped, not the index: the RAW half's features,
        // settings and image vectors are unaffected by the phrase list, and
        // refusing the whole file would cost a user their hour-long RAW build
        // over the half of it that is cheap to rebuild.
        if let Some(stored) = idx.embed_provenance.as_deref().and_then(vocab_version_of)
            && stored != LOOK_VOCAB_VERSION
            && !idx.looks.is_empty()
        {
            eprintln!(
                "style index {} was built with look vocabulary v{stored} and this build speaks                  v{LOOK_VOCAB_VERSION} — its {} look record(s) are being ignored; rebuild the                  look library (autoshop style-index --looks <dir> --embed) to use them again",
                path.display(),
                idx.looks.len()
            );
            idx.looks.clear();
            idx.looks_dir = None;
        }
        if idx.mean.len() != NDIM || idx.std.len() != NDIM {
            anyhow::bail!(
                "style index {} has normalization vectors with the wrong dimension",
                path.display()
            );
        }
        // Finite AND bounded (L04-3): a finite 1e38 mean overflowed
        // `(v - mean)/std` to ±inf with the guaranteed-small divisor, and
        // the retrieval sort then ordered NaN keys. With these bands the
        // worst normalize() output is (1e3+1e3)/1e-4 = 2e7 — ~22 decades of
        // f32 headroom on the summed distance.
        if !idx.mean.iter().all(|v| v.is_finite() && v.abs() <= MAX_FEATURE_ABS)
            || !idx.std.iter().all(|v| v.is_finite() && (1e-4..=MAX_FEATURE_ABS).contains(v))
        {
            anyhow::bail!(
                "style index {} has invalid normalization values (each must be finite \
                 and within ±{MAX_FEATURE_ABS})",
                path.display()
            );
        }

        for (i, exemplar) in idx.exemplars.iter_mut().enumerate() {
            if exemplar.feat.len() != NDIM {
                anyhow::bail!(
                    "style index {} exemplar {i} has {} features (expected {NDIM})",
                    path.display(),
                    exemplar.feat.len()
                );
            }
            if !exemplar_is_finite(exemplar) {
                anyhow::bail!(
                    "style index {} exemplar {i} contains a non-finite number",
                    path.display()
                );
            }

            // The tag is free text that reaches the model prompt; only the
            // exact `derive_tag` vocabulary is a tag, anything else is not.
            let mut tag = exemplar.tag.split('/');
            let valid_tag = matches!(
                tag.next(),
                Some("ultrawide" | "wide" | "normal" | "tele")
            ) && matches!(tag.next(), Some("dark" | "mid" | "bright"))
                && matches!(tag.next(), Some("night" | "goldenish" | "midday"))
                && matches!(tag.next(), Some("portrait" | "landscape"))
                && tag.next().is_none();
            if !valid_tag {
                anyhow::bail!(
                    "style index {} exemplar {i} has an invalid scene tag",
                    path.display()
                );
            }

            // Same bands the recipe's own clamp() enforces (temperature_k
            // 2000..40000, sliders ±100) — one source of truth, so a stored
            // exemplar can never carry a value the engine would re-clamp.
            for (key, value) in &mut exemplar.settings {
                let (lo, hi) = match key.as_str() {
                    "exposure" => (-5.0, 5.0),
                    "temperature_K" => (2000.0, 40000.0),
                    "contrast" | "highlights" | "shadows" | "whites" | "blacks"
                    | "vibrance" | "clarity" | "tint" | "saturation" | "dehaze" => {
                        (-100.0, 100.0)
                    }
                    _ => anyhow::bail!(
                        "style index {} exemplar {i} has an unsupported setting key",
                        path.display()
                    ),
                };
                *value = value.clamp(lo, hi);
            }

            if let Some(curve) = &mut exemplar.curve {
                curve[0] = curve[0].clamp(0.0, 255.0);
                // These are the extrema of (out@191 - 191) - (out@64 - 64)
                // when both curve outputs remain in 0..=255.
                curve[1] = curve[1].clamp(-382.0, 128.0);
            }

            // The family summary reaches the prompt like everything else here,
            // so it gets the same door treatment (means bounded to 0..100, the
            // curve count to 3).
            if let Some(families) = &mut exemplar.families {
                families.clamp();
            }
        }

        for (i, look) in idx.looks.iter_mut().enumerate() {
            let norm = look.embed.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>().sqrt();
            if look.embed.len() != crate::embed::EMBED_DIM
                || !look.embed.iter().all(|v| v.is_finite())
                || (norm - 1.0).abs() > 1e-3
            {
                anyhow::bail!("style index {} look exemplar {i} has an invalid embedding", path.display());
            }
            if look.vocab_scores.as_ref().is_some_and(|v| v.len() != LOOK_VOCAB.len() || !v.iter().all(|x| x.is_finite())) {
                anyhow::bail!("style index {} look exemplar {i} has invalid vocabulary scores", path.display());
            }
            if look.tags.len() > LOOK_TAGS_K || look.tags.iter().any(|t| t.chars().count() > 128) {
                anyhow::bail!("style index {} look exemplar {i} has invalid tags", path.display());
            }
            if let Some(desc) = &mut look.desc
                && desc.chars().count() > MAX_DESC_CHARS {
                    *desc = desc.chars().take(MAX_DESC_CHARS).collect();
            }
            if look.desc_embed.as_ref().is_some_and(|v| {
                v.len() != crate::embed::EMBED_DIM
                    || !v.iter().all(|x| x.is_finite() && x.abs() <= 1.0 + 1e-3)
                    || (v.iter().map(|&x| x as f64 * x as f64).sum::<f64>().sqrt() - 1.0).abs() > 1e-3
            }) {
                anyhow::bail!("style index {} look exemplar {i} has invalid description embedding", path.display());
            }
        }

        Ok(idx)
    }

    /// k nearest exemplars to (meta,hist), excluding the query photo itself
    /// when it is a corpus member (see [`is_self`]).
    ///
    /// The 14-dim block only. [`retrieve_with_embed`] is the same call with a
    /// query embedding; this one is exactly that call with `None`, so a caller
    /// that has no vector (and an index that carries none) ranks precisely as
    /// it did before R27 Batch-5.
    pub fn retrieve(&self, meta: &Meta, hist: &Histogram, k: usize, exclude: &Path) -> Vec<&StyleExemplar> {
        // The weights are immaterial with no query vectors — every cosine term
        // is absent — so the shipped ones are passed rather than a second
        // "no weights" spelling that could drift from them.
        self.retrieve_with_embed(meta, hist, StyleQuery::FEATURES_ONLY, k, exclude)
    }

    /// [`retrieve`](StyleIndex::retrieve) with the query photo's SigLIP 2
    /// embedding folded in as a second distance block.
    ///
    /// Tolerant in both directions by construction: `query_embed = None`, or an
    /// exemplar with no `embed`, contributes 0 to that pair's distance
    /// ([`embed_distance`]). So one index may legitimately hold a mix of
    /// exemplars with and without vectors — which is what happens when a build
    /// runs with the sidecar on and one photo's sidecar call fails.
    ///
    /// KNOWN, DELIBERATE ASYMMETRY: in such a mixed index the exemplars WITHOUT
    /// a vector get a 0 cosine term while the ones with a vector get
    /// `W_EMB·(1−cos) ≥ 0`, so a vector-less exemplar is never penalised and is
    /// mildly favoured. Normalising that away would mean inventing a distance
    /// for a comparison we did not make; the honest reading is that a missing
    /// vector is "no evidence", and no evidence does not push a candidate down.
    pub fn retrieve_with_embed(
        &self,
        meta: &Meta,
        hist: &Histogram,
        query: StyleQuery<'_>,
        k: usize,
        exclude: &Path,
    ) -> Vec<&StyleExemplar> {
        let mut scored = self.score_candidates(meta, hist, query, exclude);
        // total_cmp, never partial_cmp-with-Equal-fallback: a NaN key made
        // the comparator non-transitive — std documents an UNSPECIFIED
        // order (and may panic) for a non-total comparator, i.e. a silently
        // scrambled style reference. total_cmp orders every bit pattern.
        scored.sort_by(|a, b| a.1.total().total_cmp(&b.1.total()));
        scored.into_iter().take(k).map(|(e, _)| e).collect()
    }

    /// Every candidate this query admits, with its terms, in INDEX order.
    ///
    /// The one place the distance is computed. `retrieve_with_embed` sorts it,
    /// `distance_components` reads one row out of it, and neither can drift
    /// from the other — which matters more than it used to, because the text
    /// terms are standardised over the candidate SET and so cannot be computed
    /// one candidate at a time at all.
    fn score_candidates(
        &self,
        meta: &Meta,
        hist: &Histogram,
        query: StyleQuery<'_>,
        exclude: &Path,
    ) -> Vec<(&StyleExemplar, DistanceTerms)> {
        let StyleQuery { image: query_embed, text: query_text, weights } = query;
        let q = normalize(feature_vector(meta, hist), &self.mean, &self.std);
        let ex_path = std::path::absolute(exclude)
            .unwrap_or_else(|_| exclude.to_path_buf())
            .display()
            .to_string();
        let ex_stem = pipeline::stem(exclude);
        let candidates: Vec<&StyleExemplar> = self
            .exemplars
            .iter()
            .filter(|e| !is_self(e, &ex_path, ex_stem) && e.feat.len() == NDIM)
            .collect();
        let txt_gaps: Vec<Option<f64>> =
            candidates.iter().map(|e| cosine_gap(query_text, e.embed.as_deref())).collect();
        let desc_gaps: Vec<Option<f64>> =
            candidates.iter().map(|e| cosine_gap(query_text, e.desc_embed.as_deref())).collect();
        let txt = text_term(&txt_gaps, weights.txt);
        let desc = text_term(&desc_gaps, weights.desc);
        candidates
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let mut ef = [0.0f32; NDIM];
                ef.copy_from_slice(&e.feat);
                let en = normalize(ef, &self.mean, &self.std);
                // f64 accumulator (L04-3, second belt): even a bounds-check
                // bypass cannot overflow ((2·3.4e38)² · 1.5 · 14 ≈ 1e79 ≪
                // 1.8e308), so the ranking stays deterministic; on real data
                // this only removes f32 rounding in the sum.
                let d14 = (0..NDIM)
                    .map(|j| {
                        let d = (q[j] - en[j]) as f64;
                        WEIGHTS[j] as f64 * d * d
                    })
                    .sum::<f64>();
                // ADDED, never folded in: the 14-dim sum above is untouched, so
                // setting `W_EMB = 0` leaves every historical ranking bit-for-bit.
                (
                    *e,
                    DistanceTerms {
                        d14,
                        emb: embed_distance(query_embed, e.embed.as_deref(), weights.emb),
                        txt: txt.terms[i],
                        desc: desc.terms[i],
                        txt_gap: txt_gaps[i],
                        desc_gap: desc_gaps[i],
                        txt_standardised: txt.standardised,
                        desc_standardised: desc.standardised,
                    },
                )
            })
            .collect()
    }

    /// Retrieve finished-photo looks. Returns empty when no query embedding is
    /// available; callers disclose that condition instead of silently falling
    /// back to RAW exemplars.
    pub fn retrieve_looks(&self, query: StyleQuery<'_>, k: usize) -> Vec<&LookExemplar> {
        self.retrieve_looks_with_terms(query, k)
            .into_iter()
            .map(|(e, _)| e)
            .collect()
    }

    /// [`retrieve_looks`](Self::retrieve_looks) with the terms, for the
    /// diagnostic — the same call, so the printed numbers are the ranked ones.
    pub fn retrieve_looks_with_terms(
        &self,
        query: StyleQuery<'_>,
        k: usize,
    ) -> Vec<(&LookExemplar, DistanceTerms)> {
        let StyleQuery { image: query_img, text: query_text, weights } = query;
        if query_img.is_none() && query_text.is_none() {
            return Vec::new();
        }
        let txt_gaps: Vec<Option<f64>> =
            self.looks.iter().map(|e| cosine_gap(query_text, Some(&e.embed))).collect();
        let desc_gaps: Vec<Option<f64>> =
            self.looks.iter().map(|e| cosine_gap(query_text, e.desc_embed.as_deref())).collect();
        let txt = text_term(&txt_gaps, weights.txt);
        let desc = text_term(&desc_gaps, weights.desc);
        let mut scored: Vec<(&LookExemplar, DistanceTerms)> = self
            .looks
            .iter()
            .enumerate()
            .map(|(i, e)| {
                (
                    e,
                    DistanceTerms {
                        // A finished photo has no camera rendition, so it has
                        // no 14-dim feature and never gets a d14 term.
                        d14: 0.0,
                        emb: embed_distance(query_img, Some(&e.embed), weights.look),
                        txt: txt.terms[i],
                        desc: desc.terms[i],
                        txt_gap: txt_gaps[i],
                        desc_gap: desc_gaps[i],
                        txt_standardised: txt.standardised,
                        desc_standardised: desc.standardised,
                    },
                )
            })
            .collect();
        scored.sort_by(|a, b| a.1.total().total_cmp(&b.1.total()));
        scored.into_iter().take(k).collect()
    }

    /// The additive distance components used by retrieval, for ONE exemplar.
    /// Keeping this calculation public lets the offline diagnostic print
    /// exactly the terms the production path used instead of maintaining a
    /// second ranking path.
    ///
    /// It re-derives the whole candidate set because the text terms are
    /// standardised over it: asking for one candidate's term in isolation is
    /// not a question with an answer. An exemplar that is not IN the candidate
    /// set (the excluded query photo itself) answers all-zero terms.
    pub fn distance_components(
        &self,
        meta: &Meta,
        hist: &Histogram,
        query: StyleQuery<'_>,
        exclude: &Path,
        e: &StyleExemplar,
    ) -> DistanceTerms {
        self.score_candidates(meta, hist, query, exclude)
            .into_iter()
            .find(|(c, _)| std::ptr::eq(*c, e))
            .map(|(_, t)| t)
            .unwrap_or_default()
    }

    /// Did the DIRECTION take any part in choosing this look?
    ///
    /// Only when a direction produced a text vector AND at least one text
    /// weight is non-zero. At the shipped defaults both are 0, so the look is
    /// ranked by its image vector alone and a block claiming it was chosen
    /// "for this frame and direction" is a false receipt the model would then
    /// reason from — which is why this is a computed bit and not a sentence.
    pub fn look_ranked_by_direction(query: StyleQuery<'_>) -> bool {
        query.text.is_some() && (query.weights.txt > 0.0 || query.weights.desc > 0.0)
    }

    /// Render the look-library block appended to a proposer instruction.
    ///
    /// `by_direction` is [`Self::look_ranked_by_direction`] for the query that
    /// produced `looks`: it decides whether the block may say the direction had
    /// anything to do with the choice.
    pub fn render_look_reference(&self, looks: &[&LookExemplar], by_direction: bool) -> Option<String> {
        let first = looks.first()?;
        let tags = first.tags.iter().take(LOOK_TAGS_K).cloned().collect::<Vec<_>>().join(", ");
        // Through the SAME door the reference block uses (S2): a bare
        // `take(MAX_DESC_CHARS)` bounded the LENGTH and nothing else, so a
        // description carrying a newline could forge a line of this block.
        let desc = first.desc.as_deref().and_then(crate::describe::sanitize_desc);
        let mut out = format!(
            "LOOK REFERENCE (from the photographer's LOOK LIBRARY — the finished photo closest to this frame{}; match its grade, not its content): [{}] look: {}",
            if by_direction { " and direction" } else { "" },
            first.stem.chars().take(MAX_STEM_CHARS).collect::<String>(), tags
        );
        if let Some(d) = desc.filter(|d| !d.is_empty()) {
            out.push_str("; ");
            out.push_str(&d);
        }
        Some(out)
    }

    /// Render retrieved exemplars as a SOFT reference block for the advisor prompt.
    ///
    /// The pipeline passes the Style axis here for GATE 5 (R23-3); the
    /// historical `strength` parameter name remains for API compatibility.
    /// This block's two
    /// "…and not stronger / do not exceed it" clauses were the OTHER half of the
    /// binary style gate. Whatever the strength dial said, retrieving a reference
    /// re-imposed a ceiling — so the two sliders were not independent axes, and a
    /// user who built a library got MORE restraint by asking for more personal
    /// style. At [`StrengthTier::Committed`] the same measured habits become a
    /// FLOOR instead. The NUMBERS in this block never change: they are what the
    /// photographer actually did, and rewriting a measurement to match a dial
    /// would be a fabrication.
    pub fn render_reference(
        &self,
        ex: &[&StyleExemplar],
        strength: crate::recipe::GradeStrength,
    ) -> Option<String> {
        if ex.is_empty() {
            return None;
        }
        let bold = strength.get() >= 0.85;
        let lines: Vec<String> = ex
            .iter()
            .map(|e| {
                let s: Vec<String> = e
                    .settings
                    .iter()
                    .map(|(k, v)| format!("{k} {v:+.0}"))
                    .collect();
                // `look: <tags> — <desc>` (S2): the tags stay FIRST because
                // they are a bounded vocabulary the proposer has seen in every
                // other block, and the prose is appended only when the
                // exemplar carries one. Both halves go through the same
                // bounds the index door applied — the description is model
                // output about the user's photograph, i.e. untrusted text
                // reaching a prompt, and a second door costs nothing.
                let desc = e.desc.as_deref().and_then(crate::describe::sanitize_desc);
                let look = match (e.tags.is_empty(), desc) {
                    (true, None) => String::new(),
                    (true, Some(d)) => format!(" · look: {d}"),
                    (false, None) => format!(" · look: {}", e.tags.join(", ")),
                    (false, Some(d)) => format!(" · look: {} — {d}", e.tags.join(", ")),
                };
                format!("[{}] {}{}", e.tag, s.join(", "), look)
            })
            .collect();
        // Average the retrieved exemplars' tone-curve SHAPE (those who drew one),
        // so the AI shapes its tone_curve the way this user habitually does.
        let curves: Vec<[f32; 2]> = ex.iter().filter_map(|e| e.curve).collect();
        let curve_note = if !curves.is_empty() {
            let n = curves.len() as f32;
            let bl = curves.iter().map(|c| c[0]).sum::<f32>() / n;
            let ss = curves.iter().map(|c| c[1]).sum::<f32>() / n;
            let aim = if bold {
                "— shape your `tone_curve` at least this strongly; you MAY go further."
            } else {
                "— shape your `tone_curve` to a similar gentleness, not stronger."
            };
            format!(
                "  THEIR TYPICAL MASTER TONE CURVE: black-lift {bl:+.0}, S-strength {ss:+.0} \
(0..255 scale) {aim}"
            )
        } else {
            String::new()
        };
        // The colour FAMILIES, as the same kind of averaged habit (R23-1). Only
        // over exemplars that carry a summary: a pre-R23 index has none, and an
        // all-neutral sidecar records none, so a zero here would be a claim we
        // did not measure.
        let fams: Vec<crate::eval::FamilySummary> = ex.iter().filter_map(|e| e.families).collect();
        let family_note = if fams.is_empty() {
            String::new()
        } else {
            let n = fams.len() as f32;
            let mean = |f: fn(&crate::eval::FamilySummary) -> f32| {
                fams.iter().map(f).sum::<f32>() / n
            };
            let aim = if bold {
                "treat this LEVEL of colour shaping as your FLOOR — you may go beyond it."
            } else {
                "match this LEVEL of colour shaping, do not exceed it."
            };
            format!(
                "  THEIR TYPICAL COLOUR SHAPING ({} of {} similar shots): HSL mixer mean |hue| \
{:.0}, |sat| {:.0}, |lum| {:.0} across the 8 bands; colour-grade strongest wheel saturation \
{:.0}, mean |wheel lum| {:.0}; per-channel RGB curves on {:.1} of 3 channels — {aim}",
                fams.len(),
                ex.len(),
                mean(|f| f.hsl[0]),
                mean(|f| f.hsl[1]),
                mean(|f| f.hsl[2]),
                mean(|f| f.grade[0]),
                mean(|f| f.grade[1]),
                mean(|f| f.rgb_curves as f32),
            )
        };
        // The LOOK the retrieved shots share, as a habit in its own right
        // (task book section 1c). The per-exemplar `look:` clause inside `lines` is a
        // list of four descriptions; this is what they have in COMMON, and at
        // Style >= 0.85 it is stated as part of the TARGET rather than as
        // background. Without it the bold header named settings, curve and
        // colour families and left the vocabulary tags as decoration on lines
        // the model was told not to copy.
        let look_note = {
            let mut freq: BTreeMap<&str, usize> = BTreeMap::new();
            for tag in ex.iter().flat_map(|e| e.tags.iter()) {
                *freq.entry(tag.as_str()).or_default() += 1;
            }
            if freq.is_empty() {
                String::new()
            } else {
                let mut ranked: Vec<(&str, usize)> = freq.into_iter().collect();
                // Most shared first; ties by phrase, so the block is stable.
                ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
                let shared: Vec<String> = ranked
                    .iter()
                    .take(LOOK_TAGS_K)
                    .map(|(tag, n)| format!("{tag} ({n}/{})", ex.len()))
                    .collect();
                let aim = if bold {
                    "— REPRODUCE this look; it is the target, and you may push past it."
                } else {
                    "— the look their edits tend toward; stay within it, do not exceed it."
                };
                format!("  THEIR SHARED LOOK across these shots: {} {aim}", shared.join(", "))
            }
        };
        if bold {
            Some(format!(
                "STYLE REFERENCE — TARGET style to reproduce (the retrieved shots define the settings, curve habit, colour \
                 families and LOOK to reproduce; the scene differs): {}{}{}{}",
                lines.join("  |  "), curve_note, family_note, look_note
            ))
        } else {
            Some(format!(
                "STYLE REFERENCE — how this user edited SIMILAR past shots (for consistency with their \
                 taste; reference, do NOT copy verbatim, the scene differs): {}{}{}{}",
                lines.join("  |  "), curve_note, family_note, look_note
            ))
        }
    }

    /// Style-axis spelling used by the advisor pipeline. Kept separate from
    /// the historical GradeStrength entry point so existing gate fixtures do
    /// not change their API while Style >= 0.85 gets target wording.
    pub fn render_reference_for_style(
        &self,
        ex: &[&StyleExemplar],
        style: f32,
    ) -> Option<String> {
        self.render_reference(ex, crate::recipe::GradeStrength::new(style))
    }
}

/// Which index file answered, and whether one could be used at all.
///
/// ONE loader for every surface (R23-2): the CLI, the web handler, the GUI's
/// status line and `pipeline::produce_recipe` used to each spell the
/// central-then-legacy walk themselves, and the pipeline's copy additionally
/// decided "is this worth telling the user about?" from `central.exists()` —
/// which is why a fresh install got a Style slider that silently did nothing.
/// The three states here are exactly the three answers a surface needs.
pub enum EffectiveIndex {
    /// A usable index, and the file it came from (absolute).
    Loaded(StyleIndex, std::path::PathBuf),
    /// No index file anywhere — nothing has been built yet.
    Absent,
    /// A file EXISTS but cannot be used (version gate, corruption, size cap,
    /// permissions). `err` is the loader's own message chain.
    Unusable { path: std::path::PathBuf, err: String },
}

/// The user's style index: the central store first, the legacy cwd-relative
/// file as a fallback (see [`LEGACY_INDEX_PATH`]).
pub fn load_effective() -> EffectiveIndex {
    load_effective_at(&crate::store::style_index_path(), Path::new(LEGACY_INDEX_PATH))
}

/// [`load_effective`] against explicit paths — the seam the tests drive (the
/// real one reads a per-user store location that no test may depend on).
pub fn load_effective_at(central: &Path, legacy: &Path) -> EffectiveIndex {
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    // Both loads are attempted before anything is CLASSIFIED: a corrupt
    // central file with a good legacy one beside it must still serve the
    // legacy index, exactly as it did before this function existed.
    let central_err = match StyleIndex::load(central) {
        Ok(ix) => return EffectiveIndex::Loaded(ix, abs(central)),
        Err(e) => e,
    };
    let legacy_err = match StyleIndex::load(legacy) {
        Ok(ix) => return EffectiveIndex::Loaded(ix, abs(legacy)),
        Err(e) => e,
    };
    // Nothing answered. "A file exists but cannot be used" and "no library at
    // all" are DIFFERENT facts: the first deserves the loader's error (it
    // carries the version-gate rebuild instruction), the second is a fresh
    // install that needs an entry point, not an error message.
    if central.exists() {
        EffectiveIndex::Unusable { path: abs(central), err: format!("{central_err:#}") }
    } else if legacy.exists() {
        EffectiveIndex::Unusable { path: abs(legacy), err: format!("{legacy_err:#}") }
    } else {
        EffectiveIndex::Absent
    }
}

/// Everything a UI shows ABOUT the style library, as typed facts — the
/// shared read behind the web's `/api/style-info` and the GUI's status line
/// (R23-2: the GUI had no production-side entry at all, and the display logic
/// existed only inside the web handler).
#[derive(Debug, Clone, PartialEq)]
pub struct StyleIndexInfo {
    /// The file that answered — or, when none did, the central path a build
    /// would write.
    pub path: std::path::PathBuf,
    pub state: StyleIndexState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StyleIndexState {
    Built {
        /// How many of the user's own edits it holds.
        total: usize,
        version: u32,
        /// The folder it was built FROM (`None` for indexes written before
        /// the field existed).
        source_dir: Option<String>,
        /// The most common scene tags, most-frequent first, at most 6.
        scenes: Vec<(String, usize)>,
        /// Number of RAW exemplars carrying a valid SigLIP vector.
        with_embedding: usize,
        /// Number of finished-photo look records in the separate library.
        looks: usize,
        looks_dir: Option<String>,
        /// How long ago the file was written, measured at read time. A
        /// RELATIVE age on purpose: this tree carries no calendar library, and
        /// "built 3 days ago" is the question a photographer is asking anyway.
        age: Option<std::time::Duration>,
    },
    /// Nothing built yet.
    Absent,
    /// A file exists but cannot be used — the loader's message.
    Unusable { err: String },
}

/// Read the style library's status (see [`StyleIndexInfo`]).
pub fn index_info() -> StyleIndexInfo {
    index_info_at(&crate::store::style_index_path(), Path::new(LEGACY_INDEX_PATH))
}

/// [`index_info`] against explicit paths — the tested seam.
pub fn index_info_at(central: &Path, legacy: &Path) -> StyleIndexInfo {
    match load_effective_at(central, legacy) {
        EffectiveIndex::Loaded(ix, path) => {
            let mut tags: BTreeMap<&str, usize> = BTreeMap::new();
            for e in &ix.exemplars {
                *tags.entry(e.tag.as_str()).or_default() += 1;
            }
            let mut scenes: Vec<(String, usize)> =
                tags.into_iter().map(|(t, n)| (t.to_string(), n)).collect();
            // Count first, then the tag itself: a pure count sort left ties in
            // an order that could differ between two reads of the same file.
            scenes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            scenes.truncate(6);
            let age = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| std::time::SystemTime::now().duration_since(t).ok());
            let state = StyleIndexState::Built {
                total: ix.exemplars.len(),
                version: ix.version,
                source_dir: ix.source_dir.clone(),
                scenes,
                age,
                with_embedding: ix.exemplars.iter().filter(|e| e.embed.is_some()).count(),
                looks: ix.looks.len(),
                looks_dir: ix.looks_dir.clone(),
            };
            StyleIndexInfo { path, state }
        }
        EffectiveIndex::Absent => StyleIndexInfo {
            path: std::path::absolute(central).unwrap_or_else(|_| central.to_path_buf()),
            state: StyleIndexState::Absent,
        },
        EffectiveIndex::Unusable { path, err } => {
            StyleIndexInfo { path, state: StyleIndexState::Unusable { err } }
        }
    }
}

/// The file names of the exemplars ONE retrieval actually used — the answer
/// to "which library is it referencing, and which shots?" (feedback #6, the
/// user's stated top pain: the reference was invisible).
///
/// Stems only, never full paths: the rationale is persisted and shown in
/// three UIs, and the folder layout is not the point. Bounded on both axes
/// ([`MAX_DISCLOSED_NEIGHBOURS`], [`MAX_STEM_CHARS`]) so a long-named library
/// cannot crowd out the rest of the rationale.
pub fn neighbour_stems(ex: &[&StyleExemplar]) -> Vec<String> {
    ex.iter()
        .take(MAX_DISCLOSED_NEIGHBOURS)
        .map(|e| {
            let mut s: String = e.stem.chars().take(MAX_STEM_CHARS).collect();
            if s.chars().count() < e.stem.chars().count() {
                s.push('…');
            }
            s
        })
        .collect()
}

/// `(the settings label from [`REF_KEYS`]) → (the `EditRecipe` field name)`.
///
/// A named function since R23-1 so the registry-consistency test can read it:
/// this and `REF_KEYS` were two independent hand-kept lists, and nothing tied
/// either of them to the control they name.
const fn style_targets_map() -> [(&'static str, &'static str); 12] {
    [
        ("exposure", "exposure_ev"),
        ("contrast", "contrast"),
        ("highlights", "highlights"),
        ("shadows", "shadows"),
        ("whites", "whites"),
        ("blacks", "blacks"),
        ("vibrance", "vibrance"),
        ("clarity", "clarity"),
        ("temperature_K", "temperature_k"),
        ("tint", "tint"),
        ("saturation", "saturation"),
        ("dehaze", "dehaze"),
    ]
}

/// Mean of the retrieved exemplars' slider settings, keyed by the matching
/// [`EditRecipe`] field name. This is the "distill toward my historical style"
/// target — applied as a *gentle, capped* pull by [`blend_toward`], never a full
/// override (per the user's "use as reference, not a target" decision).
pub fn style_targets(ex: &[&StyleExemplar]) -> BTreeMap<&'static str, f32> {
    let mut out = BTreeMap::new();
    for (label, field) in style_targets_map() {
        let vals: Vec<f32> = ex.iter().filter_map(|e| e.settings.get(label).copied()).collect();
        if !vals.is_empty() {
            out.insert(field, vals.iter().sum::<f32>() / vals.len() as f32);
        }
    }
    out
}

/// Pull `recipe`'s global sliders a fraction `t` (0..1) toward `targets` (your
/// historical style means). `t = 0` is a no-op; the caller caps `t` so even
/// "100% style" never fully overrides the AI's scene-specific proposal.
pub fn blend_toward(recipe: &mut EditRecipe, targets: &BTreeMap<&'static str, f32>, t: f32) {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 || targets.is_empty() {
        return;
    }
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    for (field, &target) in targets {
        match *field {
            "exposure_ev" => recipe.exposure_ev = lerp(recipe.exposure_ev, target),
            "contrast" => recipe.contrast = lerp(recipe.contrast, target),
            "highlights" => recipe.highlights = lerp(recipe.highlights, target),
            "shadows" => recipe.shadows = lerp(recipe.shadows, target),
            "whites" => recipe.whites = lerp(recipe.whites, target),
            "blacks" => recipe.blacks = lerp(recipe.blacks, target),
            "vibrance" => recipe.vibrance = lerp(recipe.vibrance, target),
            "clarity" => recipe.clarity = lerp(recipe.clarity, target),
            "tint" => {
                // Tint pairs with Temperature (a tint is tuned AT a Kelvin,
                // and the index records the pair only under Custom WB) —
                // pulling tint onto an as-shot recipe applied HALF of a WB
                // decision as a floating colour cast. Same as-shot-stays
                // rule as temperature_k below.
                if recipe.temperature_k.is_some() {
                    recipe.tint = lerp(recipe.tint, target);
                }
            }
            "saturation" => recipe.saturation = lerp(recipe.saturation, target),
            "dehaze" => recipe.dehaze = lerp(recipe.dehaze, target),
            "temperature_k" => {
                // Blending needs a base Kelvin: an as-shot recipe (None) has
                // nothing to lerp FROM — `unwrap_or(target)` applied the
                // exemplar's temperature IN FULL at any nonzero strength and
                // silently flipped as-shot into custom WB. As-shot stays.
                if let Some(cur) = recipe.temperature_k {
                    recipe.temperature_k = Some(lerp(cur, target));
                }
            }
            _ => {}
        }
    }
}

/// Style axis pull: preserve the shipped 0.3 default's historical 0.18 pull,
/// while allowing Style 1.0 to reach the retrieved target fully.
pub fn style_pull(style: f32) -> f32 {
    let s = style.clamp(0.0, 1.0);
    if s >= 0.5 { s } else { s * 0.6 }
}

fn normalize(mut v: [f32; NDIM], mean: &[f32], std: &[f32]) -> [f32; NDIM] {
    for &d in &ZSCORE_DIMS {
        let s = std.get(d).copied().unwrap_or(1.0).max(1e-4);
        v[d] = (v[d] - mean.get(d).copied().unwrap_or(0.0)) / s;
    }
    v
}

fn compute_norm(ex: &[StyleExemplar]) -> (Vec<f32>, Vec<f32>) {
    let mut mean = vec![0.0f32; NDIM];
    let mut std = vec![1.0f32; NDIM];
    if ex.is_empty() {
        return (mean, std);
    }
    let n = ex.len() as f32;
    for &d in &ZSCORE_DIMS {
        let m: f32 = ex.iter().map(|e| e.feat[d]).sum::<f32>() / n;
        let var: f32 = ex.iter().map(|e| (e.feat[d] - m).powi(2)).sum::<f32>() / n;
        mean[d] = m;
        std[d] = var.sqrt().max(1e-4);
    }
    (mean, std)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hour_reads_exif() {
        assert_eq!(parse_hour(Some("2023:06:01 14:30:00")), 14.0);
        assert_eq!(parse_hour(None), 12.0);
    }

    /// R28 Batch-4 4b: staging is the whole mechanism that lets the ~181 MB
    /// preview and the seconds-long sidecar stop overlapping, so the frame must
    /// really land on disk (the sidecar reads a PATH), must be the REDUCED
    /// frame, and must take both temp names with it when it drops.
    ///
    /// MUTATION THIS KILLS: deleting the `Drop` impl — the PNG then survives in
    /// the user's temp directory once per photo of every index build, which is
    /// the leak the two hand-written `remove_file` calls used to prevent on the
    /// happy path only.
    #[test]
    fn a_staged_embedding_frame_is_reduced_and_cleans_up_after_itself() {
        let dir = std::env::temp_dir().join(format!("autoshop-stage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        // Deliberately larger than the embedding frame, and not square, so a
        // "staged the source instead" mutant is visible in both dimensions.
        let src = image::DynamicImage::ImageRgb8(image::RgbImage::new(2000, 1000));
        let (img, json) = {
            let staged = stage_embed_frame(&src, &dir, "probe").expect("staging writes the frame");
            assert!(staged.img.exists(), "the sidecar is handed a path, so the file must exist");
            let on_disk = image::open(&staged.img).expect("the staged frame is a readable PNG");
            assert_eq!(
                (on_disk.width(), on_disk.height()),
                (EMBED_FRAME_EDGE, EMBED_FRAME_EDGE / 2),
                "the frame is reduced to the long edge, aspect kept"
            );
            (staged.img.clone(), staged.json.clone())
        };
        assert!(!img.exists(), "the staged PNG must not outlive the frame that owns it");
        assert!(!json.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tag_describes_a_bright_tele_landscape() {
        let mut f = [0.0f32; NDIM];
        f[0] = 120.0_f32.ln(); // tele
        f[5] = 0.7; // bright
        // The hour rides a (sin, cos) pair: atan2(0, 1) = 0 → hour 0 → night.
        f[3] = 0.0;
        f[4] = 1.0;
        f[13] = 0.0; // landscape
        // The WHOLE tag: the old starts_with/ends_with pair never read the
        // time-of-day component, so the one field this fixture sets on purpose
        // was the one field it could not judge.
        assert_eq!(derive_tag(&f), "tele/bright/night/landscape");
    }

    #[test]
    fn style_blend_pulls_toward_historical_mean() {
        let mk = |exp: f32, con: f32, sat: f32| StyleExemplar {
            stem: "x".into(),
            feat: vec![0.0; NDIM],
            tag: "t".into(),
            settings: BTreeMap::from([
                ("exposure".to_string(), exp),
                ("contrast".to_string(), con),
                ("saturation".to_string(), sat),
                ("dehaze".to_string(), 8.0),
            ]),
            curve: Some([5.0, 12.0]),
            path: None,
            families: None,
            embed: None,
            tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
        };
        let (a, b) = (mk(0.4, 20.0, 10.0), mk(0.6, 40.0, 30.0));
        let targets = style_targets(&[&a, &b]);
        assert_eq!(targets.get("exposure_ev").copied(), Some(0.5)); // mean(0.4,0.6)
        assert_eq!(targets.get("contrast").copied(), Some(30.0)); // mean(20,40)
        assert_eq!(targets.get("saturation").copied(), Some(20.0)); // mean(10,30) — v2 field
        assert_eq!(targets.get("dehaze").copied(), Some(8.0)); // v2 field

        let mut r = EditRecipe::default();
        blend_toward(&mut r, &targets, 0.5); // pull halfway from 0
        assert!((r.exposure_ev - 0.25).abs() < 1e-5, "{}", r.exposure_ev);
        assert!((r.contrast - 15.0).abs() < 1e-4, "{}", r.contrast);
        assert!((r.saturation - 10.0).abs() < 1e-4, "{}", r.saturation); // halfway to 20
        assert!((r.dehaze - 4.0).abs() < 1e-4, "{}", r.dehaze); // halfway to 8

        let before = r.clone();
        blend_toward(&mut r, &targets, 0.0); // strength 0 = no-op
        assert_eq!(r, before);
    }

    #[test]
    fn style_pull_is_full_at_one_and_unchanged_at_default() {
        assert_eq!(style_pull(1.0), 1.0);
        assert_eq!(style_pull(0.3), 0.18);
    }

    #[test]
    fn reference_wording_becomes_target_at_high_style() {
        let idx = StyleIndex { version: 0, mean: Vec::new(), std: Vec::new(), exemplars: Vec::new(), source_dir: None, looks: Vec::new(), looks_dir: None, embed_provenance: None };
        let ex = StyleExemplar {
            stem: "x".into(), feat: Vec::new(), tag: "wide/mid/midday/landscape".into(),
            settings: BTreeMap::from([("contrast".to_string(), 15.0)]), curve: None,
            path: None, families: None, embed: None,
            tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
        };
        let low = idx.render_reference(&[&ex], crate::recipe::GradeStrength::new(0.65)).unwrap();
        let high = idx.render_reference(&[&ex], crate::recipe::GradeStrength::new(0.9)).unwrap();
        assert!(high.contains("TARGET style to reproduce"));
        assert!(!low.contains("TARGET style to reproduce"));
    }

    #[test]
    fn default_style_reference_is_byte_identical_to_head() {
        let ex = StyleExemplar {
            stem: "fixed".into(),
            feat: vec![0.0; NDIM],
            tag: "wide/mid/midday/landscape".into(),
            settings: BTreeMap::from([("contrast".to_string(), 15.0)]),
            curve: None,
            path: None,
            families: None,
            embed: None,
            tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
        };
        let idx = StyleIndex {
            version: CURRENT_INDEX_VERSION,
            mean: vec![0.0; NDIM],
            std: vec![1.0; NDIM],
            exemplars: vec![],
            source_dir: None,
            looks: Vec::new(), looks_dir: None, embed_provenance: None,
        };
        let rendered = idx
            .render_reference(&[&ex], crate::recipe::GradeStrength::new(0.30))
            .unwrap();
        assert_eq!(
            rendered,
            "STYLE REFERENCE — how this user edited SIMILAR past shots (for consistency with their taste; reference, do NOT copy verbatim, the scene differs): [wide/mid/midday/landscape] contrast +15"
        );
        let bold = idx
            .render_reference(&[&ex], crate::recipe::GradeStrength::new(0.90))
            .unwrap();
        assert!(bold.contains("TARGET style to reproduce"));
    }

    #[test]
    fn tint_never_lands_alone_on_an_as_shot_recipe() {
        let targets = BTreeMap::from([("tint", 20.0f32), ("temperature_k", 6000.0f32)]);
        let mut as_shot = EditRecipe::default(); // temperature_k = None
        blend_toward(&mut as_shot, &targets, 0.5);
        assert_eq!(as_shot.temperature_k, None, "as-shot stays as-shot");
        assert_eq!(as_shot.tint, 0.0, "no floating half-WB cast");
        let mut custom = EditRecipe { temperature_k: Some(5000.0), ..Default::default() };
        blend_toward(&mut custom, &targets, 0.5);
        assert_eq!(custom.temperature_k, Some(5500.0));
        assert_eq!(custom.tint, 10.0, "the pair moves together under custom WB");
    }

    #[test]
    fn self_exclusion_prefers_path_and_falls_back_to_stem() {
        let mk = |stem: &str, path: Option<&str>| StyleExemplar {
            stem: stem.into(),
            path: path.map(str::to_string),
            families: None,
            embed: None,
            tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
            feat: vec![0.0; NDIM],
            tag: "t".into(),
            settings: BTreeMap::new(),
            curve: None,
        };
        let q_path = "D:\\roll-a\\DSC1.ARW";
        // Path identity: the same file, case-flipped, is SELF on Windows.
        let same = mk("DSC1", Some("d:\\roll-a\\dsc1.arw"));
        assert_eq!(is_self(&same, q_path, "DSC1"), cfg!(windows));
        let exact = mk("DSC1", Some("D:\\roll-a\\DSC1.ARW"));
        assert!(is_self(&exact, q_path, "DSC1"));
        // A same-stem photo from ANOTHER roll is not self (the old stem rule
        // dropped it too — one retrieval slot lost for nothing).
        let other = mk("DSC1", Some("D:\\roll-b\\DSC1.ARW"));
        assert!(!is_self(&other, q_path, "DSC1"));
        // Pre-path (legacy index) exemplars keep the stem fallback.
        let legacy = mk("DSC1", None);
        assert!(is_self(&legacy, q_path, "DSC1"));
    }

    #[test]
    fn non_finite_exemplars_are_refused() {
        let mut e = StyleExemplar {
            stem: "x".into(),
            path: None,
            families: None,
            embed: None,
            tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
            feat: vec![0.0; NDIM],
            tag: "t".into(),
            settings: BTreeMap::new(),
            curve: Some([f32::NAN, 0.0]),
        };
        assert!(!exemplar_is_finite(&e), "NaN curve shape refused");
        e.curve = None;
        assert!(exemplar_is_finite(&e));
        e.feat[0] = f32::INFINITY;
        assert!(!exemplar_is_finite(&e), "non-finite feature refused");
    }

    #[test]
    fn reference_surfaces_the_users_curve_habit() {
        let ex = StyleExemplar {
            stem: "x".into(),
            feat: vec![0.0; NDIM],
            tag: "wide/mid/midday/landscape".into(),
            settings: BTreeMap::from([("contrast".to_string(), 15.0)]),
            curve: Some([6.0, 20.0]),
            path: None,
            families: None,
            embed: None,
            tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
        };
        let idx = StyleIndex {
            version: CURRENT_INDEX_VERSION,
            mean: vec![0.0; NDIM],
            std: vec![1.0; NDIM],
            exemplars: vec![],
            source_dir: None,
            looks: Vec::new(), looks_dir: None, embed_provenance: None,
        };
        let r = idx.render_reference(&[&ex], crate::recipe::GradeStrength::calibrated()).unwrap();
        assert!(r.contains("TYPICAL MASTER TONE CURVE"), "{r}");
        assert!(r.contains("S-strength +20"), "{r}");
    }

    /// R23-1: the reference block's key set is a curated SUBSET of the control
    /// registry, and every spelling in it must still be the registry's own — a
    /// renamed control would otherwise leave this table reading a `crs` key
    /// nothing writes (silently learning nothing) or mapping onto a recipe
    /// field `blend_toward` no longer has.
    #[test]
    fn ref_keys_and_map_agree_with_the_control_registry() {
        use crate::advisor::catalogue::{global_control, RECIPE_CONTROLS};
        // MAP's field names are the registry's field names…
        let map = style_targets_map();
        for (label, field) in map {
            let c = global_control(field)
                .unwrap_or_else(|| panic!("MAP maps `{label}` onto `{field}`, not a control"));
            assert!(!c.engine_only, "`{field}` is engine-only — the AI cannot be pulled toward it");
            // …and each label's crs attribute is that control's own attribute.
            let (key, _) = REF_KEYS
                .iter()
                .find(|(_, l)| l == &label)
                .unwrap_or_else(|| panic!("MAP label `{label}` has no REF_KEYS row"));
            assert_eq!(
                c.crs.attr(),
                Some(*key),
                "`{field}`: REF_KEYS reads crs:{key}, the registry says {:?}",
                c.crs
            );
        }
        // The two tables are the same size and cover the same labels (they were
        // two independent hand-kept lists — the drift #12 reported).
        assert_eq!(map.len(), REF_KEYS.len());
        for (_, label) in REF_KEYS {
            assert!(map.iter().any(|(l, _)| *l == label), "REF_KEYS label `{label}` is unmapped");
        }
        // The families are deliberately NOT in REF_KEYS (per-band means are
        // mush) — they ride as summary statistics instead.
        for c in RECIPE_CONTROLS.iter() {
            if matches!(c.name, "hsl" | "color_grade") {
                assert!(
                    !REF_KEYS.iter().any(|(k, _)| Some(*k) == c.crs.attr()),
                    "{} must not be a flat REF_KEYS row",
                    c.name
                );
            }
        }
    }

    /// The family summary is an ADDED optional field: a pre-R23 index (no
    /// `families` key at all) still loads, out-of-band values are bounded at
    /// the door, and the reference block only claims a habit it measured.
    #[test]
    fn family_summaries_are_optional_bounded_and_surfaced() {
        let path =
            std::env::temp_dir().join(format!("autoshop-style-fam-{}.json", std::process::id()));
        // A LEGACY index file, written verbatim without the new key.
        let legacy = format!(
            "{{\"version\":{CURRENT_INDEX_VERSION},\"mean\":{m},\"std\":{s},\"exemplars\":[{{\
             \"stem\":\"photo\",\"feat\":{m},\"tag\":\"wide/mid/midday/landscape\",\
             \"settings\":{{}}}}]}}",
            m = serde_json::to_string(&vec![0.0f32; NDIM]).unwrap(),
            s = serde_json::to_string(&vec![1.0f32; NDIM]).unwrap(),
        );
        std::fs::write(&path, &legacy).unwrap();
        let loaded = StyleIndex::load(&path).expect("a pre-R23 index still loads");
        assert_eq!(loaded.exemplars[0].families, None, "and contributes no summary");

        // Out-of-band summary values are clamped at the door (they reach a paid
        // prompt), and a non-finite one is refused outright.
        let mut idx = loaded;
        idx.exemplars[0].families = Some(crate::eval::FamilySummary {
            hsl: [500.0, -3.0, 20.0],
            grade: [900.0, 10.0],
            rgb_curves: 9,
        });
        std::fs::write(&path, serde_json::to_string(&idx).unwrap()).unwrap();
        let bounded = StyleIndex::load(&path).unwrap().exemplars[0].families.unwrap();
        assert_eq!(bounded.hsl, [100.0, 0.0, 20.0]);
        assert_eq!(bounded.grade, [100.0, 10.0]);
        assert_eq!(bounded.rgb_curves, 3);
        idx.exemplars[0].families =
            Some(crate::eval::FamilySummary { hsl: [f32::NAN, 0.0, 0.0], ..Default::default() });
        assert!(!exemplar_is_finite(&idx.exemplars[0]), "a NaN summary is refused");
        let _ = std::fs::remove_file(&path);

        // The reference block reports the measured habit, and says over how
        // many of the retrieved shots it was measured.
        let mk = |families| StyleExemplar {
            stem: "x".into(),
            feat: vec![0.0; NDIM],
            tag: "wide/mid/midday/landscape".into(),
            settings: BTreeMap::from([("contrast".to_string(), 15.0)]),
            curve: None,
            path: None,
            families,
            embed: None,
            tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
        };
        let with = mk(Some(crate::eval::FamilySummary {
            hsl: [2.0, 18.0, 6.0],
            grade: [20.0, 4.0],
            rgb_curves: 2,
        }));
        let without = mk(None);
        let idx = StyleIndex {
            version: CURRENT_INDEX_VERSION,
            mean: vec![0.0; NDIM],
            std: vec![1.0; NDIM],
            exemplars: vec![],
            source_dir: None,
            looks: Vec::new(), looks_dir: None, embed_provenance: None,
        };
        let r = idx.render_reference(&[&with, &without], crate::recipe::GradeStrength::calibrated()).unwrap();
        assert!(r.contains("THEIR TYPICAL COLOUR SHAPING (1 of 2 similar shots)"), "{r}");
        assert!(r.contains("|sat| 18"), "{r}");
        assert!(r.contains("strongest wheel saturation 20"), "{r}");
        assert!(r.contains("on 2.0 of 3 channels"), "{r}");
        // No summary anywhere = no claim.
        let plain = idx.render_reference(&[&without], crate::recipe::GradeStrength::calibrated()).unwrap();
        assert!(!plain.contains("COLOUR SHAPING"), "{plain}");
    }

    /// GATE 5 of the six the strength axis must pass (R23-3, feedback #5).
    ///
    /// This block's two "…and not stronger / do not exceed it" clauses were the
    /// other half of the binary style gate: retrieving a reference re-imposed a
    /// CEILING no matter what the strength dial said, so asking for more personal
    /// style bought more restraint (and a user with a library could not ask for a
    /// bolder grade at all). At the committed band the same measured habits
    /// become a FLOOR — while the NUMBERS stay identical, because they are what
    /// the photographer actually did and a dial must not rewrite a measurement.
    #[test]
    fn the_style_reference_flips_from_a_ceiling_to_a_floor_on_the_strength_axis() {
        use crate::recipe::GradeStrength;
        let ex = StyleExemplar {
            stem: "x".into(),
            feat: vec![0.0; NDIM],
            tag: "wide/mid/midday/landscape".into(),
            settings: BTreeMap::from([("contrast".to_string(), 15.0)]),
            curve: Some([6.0, 20.0]),
            path: None,
            families: Some(crate::eval::FamilySummary {
                hsl: [2.0, 18.0, 6.0],
                grade: [20.0, 4.0],
                rgb_curves: 2,
            }),
            embed: None,
            tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
        };
        let idx = StyleIndex {
            version: CURRENT_INDEX_VERSION,
            mean: vec![0.0; NDIM],
            std: vec![1.0; NDIM],
            exemplars: vec![],
            source_dir: None,
            looks: Vec::new(), looks_dir: None, embed_provenance: None,
        };
        let at = |s: f32| idx.render_reference(&[&ex], GradeStrength::new(s)).unwrap();
        let (calib, default, bold) = (at(0.5), at(GradeStrength::DEFAULT), at(0.9));

        // Below the committed band: the shipped ceiling wording, verbatim.
        for (name, text) in [("calibrated", &calib), ("default", &default)] {
            assert!(text.contains("to a similar gentleness, not stronger."), "{name}: {text}");
            assert!(text.contains("do not exceed it."), "{name}: {text}");
            assert!(!text.contains("FLOOR"), "{name} must not raise the floor: {text}");
        }
        // At it: the same habit read as a floor.
        assert!(bold.contains("at least this strongly; you MAY go further."), "{bold}");
        assert!(bold.contains("treat this LEVEL of colour shaping as your FLOOR"), "{bold}");
        assert!(!bold.contains("do not exceed it"), "a floor and a ceiling cannot both hold: {bold}");

        // The MEASURED numbers are byte-identical in both — this is a wording
        // axis, never a licence to restate the photographer's own history.
        for text in [&calib, &bold] {
            assert!(text.contains("black-lift +6, S-strength +20"), "{text}");
            assert!(text.contains("|sat| 18"), "{text}");
            assert!(text.contains("strongest wheel saturation 20"), "{text}");
            assert!(text.contains("contrast +15"), "{text}");
        }
    }

    #[test]
    fn parse_hour_rejects_non_finite_and_out_of_range_hours() {
        assert_eq!(parse_hour(Some("2023:06:01 NaN:30:00")), 12.0);
        assert_eq!(parse_hour(Some("2023:06:01 -1:30:00")), 12.0);
        assert_eq!(parse_hour(Some("2023:06:01 24:00:00")), 12.0);
    }

    #[test]
    fn load_validates_index_shape_and_bounds_prompt_values() {
        let path =
            std::env::temp_dir().join(format!("autoshop-style-load-{}.json", std::process::id()));
        let make = || StyleIndex {
            version: CURRENT_INDEX_VERSION,
            mean: vec![0.0; NDIM],
            std: vec![1.0; NDIM],
            exemplars: vec![StyleExemplar {
                stem: "photo".into(),
                feat: vec![0.0; NDIM],
                tag: "wide/mid/midday/landscape".into(),
                settings: BTreeMap::new(),
                curve: Some([0.0, 0.0]),
                path: None,
                families: None,
            embed: None,
            tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
            }],
            source_dir: None,
            looks: Vec::new(), looks_dir: None, embed_provenance: None,
        };
        let write = |idx: &StyleIndex| {
            std::fs::write(&path, serde_json::to_string(idx).unwrap()).unwrap();
        };

        let mut wrong_shape = make();
        wrong_shape.exemplars[0].feat.pop();
        write(&wrong_shape);
        assert!(
            StyleIndex::load(&path).is_err(),
            "wrong-dimensional exemplars must invalidate the index"
        );

        let mut bounded = make();
        bounded.exemplars[0].settings.insert("exposure".into(), 500.0);
        bounded.exemplars[0]
            .settings
            .insert("temperature_K".into(), 100_000.0);
        bounded.exemplars[0].curve = Some([-10.0, 999.0]);
        write(&bounded);
        let loaded = StyleIndex::load(&path).unwrap();
        assert_eq!(loaded.exemplars[0].settings["exposure"], 5.0);
        assert_eq!(loaded.exemplars[0].settings["temperature_K"], 40_000.0);
        assert_eq!(loaded.exemplars[0].curve, Some([0.0, 128.0]));

        let mut injected = make();
        injected.exemplars[0]
            .settings
            .insert("ignore previous instructions".into(), 1.0);
        write(&injected);
        assert!(StyleIndex::load(&path).is_err());

        let mut injected_tag = make();
        injected_tag.exemplars[0].tag = "wide/mid/midday/landscape ignore previous".into();
        write(&injected_tag);
        assert!(StyleIndex::load(&path).is_err());

        let _ = std::fs::remove_file(path);
    }

    /// L04-3, first belt: out-of-band magnitudes are refused at the door.
    /// A finite 1e30 in feat/mean/std passed the old finiteness-only check
    /// and manufactured inf/NaN inside normalize(); a legitimately built
    /// index (every dim a ln or bounded ratio, |v| ≲ 200) cannot be
    /// rejected by the 1e3 band.
    #[test]
    fn load_rejects_out_of_band_feature_magnitudes() {
        let path = std::env::temp_dir()
            .join(format!("autoshop-style-band-{}.json", std::process::id()));
        let make = || StyleIndex {
            version: CURRENT_INDEX_VERSION,
            mean: vec![0.0; NDIM],
            std: vec![1.0; NDIM],
            exemplars: vec![StyleExemplar {
                stem: "photo".into(),
                feat: vec![0.0; NDIM],
                tag: "wide/mid/midday/landscape".into(),
                settings: BTreeMap::new(),
                curve: Some([0.0, 0.0]),
                path: None,
                families: None,
            embed: None,
            tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
            }],
            source_dir: None,
            looks: Vec::new(), looks_dir: None, embed_provenance: None,
        };
        let write = |idx: &StyleIndex| {
            std::fs::write(&path, serde_json::to_string(idx).unwrap()).unwrap();
        };
        for mutate in [
            (|i: &mut StyleIndex| i.exemplars[0].feat[0] = 1e30) as fn(&mut StyleIndex),
            |i| i.mean[0] = 1e30,
            |i| i.std[0] = 1e30,
        ] {
            let mut bad = make();
            mutate(&mut bad);
            write(&bad);
            assert!(StyleIndex::load(&path).is_err(), "1e30 must be refused at the door");
        }
        let mut fine = make();
        fine.exemplars[0].feat[0] = 100.0;
        write(&fine);
        assert!(StyleIndex::load(&path).is_ok(), "a realistic magnitude still loads");
        let _ = std::fs::remove_file(&path);
    }

    /// L04-3, second belt (independent of load): even a POISONED index —
    /// constructed in memory, bypassing the door — yields a deterministic,
    /// panic-free ranking, because the distance accumulates in f64 and the
    /// sort uses total_cmp (a total order for every bit pattern; the old
    /// partial_cmp-Equal fallback broke transitivity on NaN keys, which
    /// std documents as unspecified-order-and-may-panic).
    #[test]
    fn retrieve_ranking_is_total_under_poisoned_normalization() {
        let ex = |stem: &str, f0: f32| StyleExemplar {
            stem: stem.into(),
            feat: {
                let mut f = vec![0.0f32; NDIM];
                f[0] = f0;
                f
            },
            tag: "t".into(),
            settings: BTreeMap::new(),
            curve: None,
            path: None,
            families: None,
            embed: None,
            tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
        };
        let idx = StyleIndex {
            version: CURRENT_INDEX_VERSION,
            mean: {
                let mut m = vec![0.0f32; NDIM];
                m[0] = 1e38; // poisoned: finite, past any physical band
                m
            },
            std: vec![1e-4; NDIM],
            exemplars: vec![ex("a", 3e38), ex("b", -3e38)],
            source_dir: None,
            looks: Vec::new(), looks_dir: None, embed_provenance: None,
        };
        let meta = crate::decode::Meta {
            make: "T".into(),
            model: "T".into(),
            lens: None,
            iso: Some(100),
            shutter: None,
            aperture: None,
            focal_length_mm: None,
            exposure_bias_ev: None,
            date_time: None,
            width: 100,
            height: 100,
            as_shot_wb_coeffs: [1.0; 4],
        };
        let hist = crate::decode::Histogram {
            luma: vec![1; 256],
            r: vec![1; 256],
            g: vec![1; 256],
            b: vec![1; 256],
            clip_black_pct: 0.0,
            clip_white_pct: 0.0,
            sample_pixels: 1,
        };
        let first = idx.retrieve(&meta, &hist, 2, Path::new("elsewhere.arw"));
        assert_eq!(first.len(), 2, "both exemplars return — no panic, none lost");
        let second = idx.retrieve(&meta, &hist, 2, Path::new("elsewhere.arw"));
        let names =
            |v: &[&StyleExemplar]| v.iter().map(|e| e.stem.clone()).collect::<Vec<_>>();
        assert_eq!(names(&first), names(&second), "the ranking is deterministic");
    }

    /// R23-2: the ONE status read every surface shares, in all three states.
    /// Driven through the explicit-path seam — the production entry reads a
    /// per-user store location, and a test that depended on it would be
    /// testing the developer's own library.
    #[test]
    fn the_shared_status_read_reports_absent_built_and_unusable() {
        let dir = std::env::temp_dir()
            .join(format!("autoshop-style-info-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        let central = dir.join("style-index.json");
        let legacy = dir.join("legacy-style-index.json");

        // ── Absent: no file anywhere. NOT an error — this is the fresh
        // install whose Style slider used to sit there doing nothing in
        // silence, and the path reported is where a build WOULD write.
        let info = index_info_at(&central, &legacy);
        assert_eq!(info.state, StyleIndexState::Absent);
        assert!(info.path.is_absolute(), "the UI shows a real path: {:?}", info.path);
        assert!(matches!(load_effective_at(&central, &legacy), EffectiveIndex::Absent));

        // ── Built: counts, version, source folder and the scene histogram.
        let ex = |tag: &str, stem: &str| StyleExemplar {
            stem: stem.into(),
            feat: vec![0.0; NDIM],
            tag: tag.into(),
            settings: BTreeMap::new(),
            curve: None,
            path: None,
            families: None,
            embed: None,
            tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
        };
        let built = StyleIndex {
            version: CURRENT_INDEX_VERSION,
            mean: vec![0.0; NDIM],
            std: vec![1.0; NDIM],
            exemplars: vec![
                ex("wide/mid/midday/landscape", "a"),
                ex("wide/mid/midday/landscape", "b"),
                ex("tele/bright/goldenish/portrait", "c"),
            ],
            source_dir: Some("D:\\photos\\edited".into()),
            looks: Vec::new(), looks_dir: None, embed_provenance: None,
        };
        built.save(&central).expect("a non-empty index saves");
        let info = index_info_at(&central, &legacy);
        match &info.state {
            StyleIndexState::Built { total, version, source_dir, scenes, age, .. } => {
                assert_eq!(*total, 3);
                assert_eq!(*version, CURRENT_INDEX_VERSION);
                assert_eq!(source_dir.as_deref(), Some("D:\\photos\\edited"));
                assert_eq!(scenes[0], ("wide/mid/midday/landscape".to_string(), 2));
                assert_eq!(scenes[1], ("tele/bright/goldenish/portrait".to_string(), 1));
                assert!(
                    age.is_some_and(|a| a < std::time::Duration::from_secs(600)),
                    "a file written milliseconds ago must read as new: {age:?}"
                );
            }
            other => panic!("expected Built, got {other:?}"),
        }

        // ── Unusable: the file exists and cannot be used. Distinguished from
        // Absent, because only one of the two is worth an error message.
        std::fs::write(&central, "{not json").unwrap();
        match index_info_at(&central, &legacy).state {
            StyleIndexState::Unusable { err } => {
                assert!(err.contains("parse style index"), "{err}")
            }
            other => panic!("expected Unusable, got {other:?}"),
        }
        // …and a good LEGACY file beside a broken central one still answers
        // (the precedence this refactor had to preserve).
        built.save(&legacy).unwrap();
        match load_effective_at(&central, &legacy) {
            EffectiveIndex::Loaded(_, p) => assert_eq!(p, std::path::absolute(&legacy).unwrap()),
            _ => panic!("the legacy fallback must still serve"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R23-2 (the user's top pain — "I have no idea which library it is
    /// referencing"): the retrieval discloses the SHOTS it used, bounded.
    #[test]
    fn the_disclosed_neighbours_are_stems_capped_in_count_and_length() {
        let mk = |stem: &str| StyleExemplar {
            stem: stem.into(),
            feat: vec![0.0; NDIM],
            tag: "wide/mid/midday/landscape".into(),
            settings: BTreeMap::new(),
            curve: None,
            // A full path exists on the exemplar; the disclosure must NOT use it.
            path: Some(format!("D:\\rolls\\2024\\{stem}.ARW")),
            families: None,
            embed: None,
            tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
        };
        let long = "x".repeat(MAX_STEM_CHARS + 20);
        let all = [mk("DSC0001"), mk("DSC0002"), mk("DSC0003"), mk("DSC0004"), mk(&long)];
        let refs: Vec<&StyleExemplar> = all.iter().collect();
        let got = neighbour_stems(&refs);
        assert_eq!(got.len(), MAX_DISCLOSED_NEIGHBOURS, "count is bounded: {got:?}");
        assert_eq!(got[0], "DSC0001");
        assert!(
            !got.iter().any(|s| s.contains("D:\\rolls")),
            "a persisted, displayed rationale must not carry folder layout: {got:?}"
        );
        // The length cap engages on the 5th…-if it were in range; assert it
        // directly on a single long stem so the bound is pinned either way.
        let one = neighbour_stems(&[&mk(&long)]);
        assert_eq!(one[0].chars().count(), MAX_STEM_CHARS + 1, "capped + ellipsis: {one:?}");
        assert!(one[0].ends_with('…'), "truncation is visible: {one:?}");
        assert!(neighbour_stems(&[]).is_empty(), "nothing retrieved ⇒ nothing claimed");
    }

    /// L04-3: the f32→f64 accumulator switch is a no-op on real data — the
    /// nearest exemplar for a well-formed index is unchanged (only sub-1e-7
    /// rounding ties could ever reorder, and those were already arbitrary).
    #[test]
    fn retrieve_distance_is_unchanged_for_a_well_formed_index() {
        let ex = |stem: &str, f0: f32| StyleExemplar {
            stem: stem.into(),
            feat: {
                let mut f = vec![0.1f32; NDIM];
                f[0] = f0;
                f
            },
            tag: "t".into(),
            settings: BTreeMap::new(),
            curve: None,
            path: None,
            families: None,
            embed: None,
            tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
        };
        let idx = StyleIndex {
            version: CURRENT_INDEX_VERSION,
            mean: vec![0.0; NDIM],
            std: vec![1.0; NDIM],
            exemplars: vec![ex("far", 150.0), ex("near", 0.5), ex("mid", 30.0)],
            source_dir: None,
            looks: Vec::new(), looks_dir: None, embed_provenance: None,
        };
        let meta = crate::decode::Meta {
            make: "T".into(),
            model: "T".into(),
            lens: None,
            iso: Some(100),
            shutter: None,
            aperture: None,
            focal_length_mm: None,
            exposure_bias_ev: None,
            date_time: None,
            width: 100,
            height: 100,
            as_shot_wb_coeffs: [1.0; 4],
        };
        let hist = crate::decode::Histogram {
            luma: vec![1; 256],
            r: vec![1; 256],
            g: vec![1; 256],
            b: vec![1; 256],
            clip_black_pct: 0.0,
            clip_white_pct: 0.0,
            sample_pixels: 1,
        };
        let got = idx.retrieve(&meta, &hist, 1, Path::new("elsewhere.arw"));
        assert_eq!(got.len(), 1);
        // feature_vector's dim 0 for this hist/meta is a small number, so
        // the exemplar nearest in dim 0 wins under EITHER accumulator.
        assert_eq!(got[0].stem, "near", "the f64 accumulator picks the same neighbour");
    }

    // R18's own gate is a pair of module-level `const _: () = assert!(…)`
    // above, not a test here: a constant-vs-constant invariant that fails the
    // BUILD is strictly stronger than one that fails a test run, and clippy's
    // `assertions_on_constants` says so too.

    /// A unit vector of the pinned width — the shape every door check accepts.
    fn unit_embed() -> Vec<f32> {
        let e = 1.0f32 / (crate::embed::EMBED_DIM as f32).sqrt();
        vec![e; crate::embed::EMBED_DIM]
    }

    /// This file's PRODUCTION half.
    ///
    /// A source-invariant test that counts a pattern must not count the string
    /// literal it uses to search: both of the counts below were off by exactly
    /// one for that reason the first time they ran.
    fn production_source() -> &'static str {
        // Split on the module header alone: the newline in between is the
        // CHECKOUT's, and a source invariant that depends on `core.autocrlf`
        // is the same trap `.gitattributes` documents for the sidecars.
        include_str!("style.rs").split("mod tests {").next().unwrap()
    }

    fn fixture_meta() -> crate::decode::Meta {
        crate::decode::Meta {
            make: "T".into(), model: "T".into(), lens: None, iso: Some(100), shutter: None,
            aperture: None, focal_length_mm: None, exposure_bias_ev: None, date_time: None,
            width: 100, height: 100, as_shot_wb_coeffs: [1.0; 4],
        }
    }

    fn fixture_histogram() -> crate::decode::Histogram {
        crate::decode::Histogram {
            luma: vec![1; 256], r: vec![1; 256], g: vec![1; 256], b: vec![1; 256],
            clip_black_pct: 0.0, clip_white_pct: 0.0, sample_pixels: 1,
        }
    }

    fn plain_exemplar(stem: &str) -> StyleExemplar {
        StyleExemplar {
            stem: stem.into(),
            feat: vec![0.0; NDIM],
            tag: "wide/mid/midday/landscape".into(),
            settings: BTreeMap::new(),
            curve: None,
            path: None,
            families: None,
            embed: None,
            tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
        }
    }

    /// The embedding block is ADDITIVE: an absent vector on either side costs
    /// nothing, so a v4 index and a v5 index built without the sidecar rank
    /// exactly as they always did — and `W_EMB = 0` reproduces the old ranking
    /// even when both sides HAVE vectors (R17).
    ///
    /// MUTATION: make `embed_distance` return `w` when a side is `None` and
    /// the first two asserts fail — a mixed index would then rank vector-less
    /// exemplars by a number nobody measured.
    #[test]
    fn the_embedding_distance_is_additive_and_tolerates_a_mixed_index() {
        let u = unit_embed();
        let mut opposite = u.clone();
        for v in opposite.iter_mut() {
            *v = -*v;
        }
        assert_eq!(embed_distance(None, Some(&u), 2.0), 0.0, "no query vector, no term");
        assert_eq!(embed_distance(Some(&u), None, 2.0), 0.0, "no exemplar vector, no term");
        assert_eq!(embed_distance(Some(&u), Some(&u), 0.0), 0.0, "W_EMB = 0 contributes nothing");
        // cos(v, v) = 1 => distance 0; cos(v, -v) = -1 => distance 2 * W_EMB.
        // The tolerance is 1e-5, not 0: `1/sqrt(768)` is not exact in f32, so
        // the self-dot lands within ~1e-7 of 1 and the term within ~2e-7 of 0.
        // That is the same slack `embed::parse_vector`'s norm gate allows, and
        // it is why the cosine is CLAMPED to [-1, 1] before the subtraction.
        assert!(embed_distance(Some(&u), Some(&u), 2.0).abs() < 1e-5, "identical vectors: ~0");
        assert!(
            (embed_distance(Some(&u), Some(&opposite), 2.0) - 4.0).abs() < 1e-6,
            "opposed unit vectors span the full 2 x W_EMB"
        );
        // A width mismatch answers 0 rather than comparing a common prefix:
        // two vectors of different widths are not comparable at all.
        assert_eq!(embed_distance(Some(&u), Some(&u[..4]), 2.0), 0.0, "mixed widths: no term");
    }

    /// The index door treats an embedding as the UNIT vector it claims to be.
    ///
    /// MUTATION: drop the norm test from `exemplar_is_finite` and the third
    /// assert passes — an all-ones 768-vector (norm 27.7) would then be
    /// accepted, and the retrieval's bare-dot-product cosine would rank it
    /// ahead of every real photo regardless of what either depicts.
    #[test]
    fn a_malformed_embedding_is_refused_at_the_index_door() {
        let mut ok = plain_exemplar("ok");
        ok.embed = Some(unit_embed());
        assert!(exemplar_is_finite(&ok), "a unit vector of the pinned width is accepted");

        let mut short = plain_exemplar("short");
        short.embed = Some(vec![1.0]);
        assert!(!exemplar_is_finite(&short), "a foreign width is refused, not ignored");

        let mut unnormalised = plain_exemplar("big");
        unnormalised.embed = Some(vec![1.0; crate::embed::EMBED_DIM]);
        assert!(!exemplar_is_finite(&unnormalised), "an unnormalised vector is refused");

        let mut nan = plain_exemplar("nan");
        let mut v = unit_embed();
        v[0] = f32::NAN;
        nan.embed = Some(v);
        assert!(!exemplar_is_finite(&nan), "a NaN element is refused");
    }

    /// v5 reads v4 (the embedding is additive, so a v4 index ranks the way it
    /// always did) and still refuses v3 (whose aspect FEATURE means something
    /// else). Bumping the version must not brick a working Style panel for the
    /// length of an hour-long rebuild — R19.
    ///
    /// MUTATION: replace the `READABLE_INDEX_VERSIONS.contains` gate with the
    /// old `!= CURRENT_INDEX_VERSION` and the v4 arm fails.
    #[test]
    fn a_v4_index_still_loads_and_a_v3_one_still_does_not() {
        let dir = std::env::temp_dir().join(format!("autoshop-style-v5-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |version: u32| {
            let idx = StyleIndex {
                version,
                mean: vec![0.0; NDIM],
                std: vec![1.0; NDIM],
                exemplars: vec![plain_exemplar("a")],
                source_dir: None,
                looks: Vec::new(), looks_dir: None, embed_provenance: None,
            };
            let p = dir.join(format!("v{version}.json"));
            std::fs::write(&p, serde_json::to_string(&idx).unwrap()).unwrap();
            p
        };
        assert!(StyleIndex::load(&write(4)).is_ok(), "a v4 index still serves");
        assert!(StyleIndex::load(&write(CURRENT_INDEX_VERSION)).is_ok(), "v5 serves");
        // `StyleIndex` is not `Debug`, so `unwrap_err` is out — match instead.
        let e = match StyleIndex::load(&write(3)) {
            Ok(_) => panic!("a v3 index must not load: its aspect FEATURE means something else"),
            Err(e) => e.to_string(),
        };
        assert!(e.contains("version 3"), "v3 is still refused by name: {e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The retrieval's cosine block actually MOVES the ranking, and the 14-dim
    /// block alone is what decides it when the weight is 0 — the two halves of
    /// R17 in one test.
    ///
    /// MUTATION: drop the `+ embed_distance(...)` term from
    /// `retrieve_with_embed` and the first assert fails (the embedding stops
    /// mattering at all).
    #[test]
    fn the_query_embedding_reorders_retrieval_and_a_zero_weight_does_not() {
        let u = unit_embed();
        let mut away = u.clone();
        for (i, v) in away.iter_mut().enumerate() {
            // Orthogonal: flip half the signs, so cos = 0 and the block
            // contributes W_EMB instead of 0.
            if i % 2 == 0 {
                *v = -*v;
            }
        }
        let mut near = plain_exemplar("near-in-embedding");
        near.embed = Some(u.clone());
        let mut far = plain_exemplar("far-in-embedding");
        far.embed = Some(away);
        // IDENTICAL 14-dim features, so the embedding is the ONLY thing that
        // can separate them, and `far` is listed FIRST so a stable sort with
        // no embedding term would answer `far`.
        let idx = StyleIndex {
            version: CURRENT_INDEX_VERSION,
            mean: vec![0.0; NDIM],
            std: vec![1.0; NDIM],
            exemplars: vec![far, near],
            source_dir: None,
            looks: Vec::new(), looks_dir: None, embed_provenance: None,
        };
        let meta = crate::decode::Meta {
            make: "T".into(),
            model: "T".into(),
            lens: None,
            iso: Some(100),
            shutter: None,
            aperture: None,
            focal_length_mm: None,
            exposure_bias_ev: None,
            date_time: None,
            width: 100,
            height: 100,
            as_shot_wb_coeffs: [1.0; 4],
        };
        let hist = crate::decode::Histogram {
            luma: vec![1; 256],
            r: vec![1; 256],
            g: vec![1; 256],
            b: vec![1; 256],
            clip_black_pct: 0.0,
            clip_white_pct: 0.0,
            sample_pixels: 1,
        };
        // The weight is an ARGUMENT, not an unsafe environment write this test has to
        // put back afterwards: `cargo test` runs these on parallel threads in
        // one process, so the old spelling reconfigured every other retrieval
        // test that happened to be running.
        let w = RetrievalWeights { emb: 2.0, ..RetrievalWeights::FEATURE_ONLY };
        let got = idx.retrieve_with_embed(&meta, &hist, StyleQuery::new(Some(&u), None, w), 1, Path::new("q.arw"));
        assert_eq!(got[0].stem, "near-in-embedding", "the cosine block decides the tie");
        // The same query with NO vector: the two exemplars are exactly tied,
        // the sort is stable, and the first listed one wins — i.e. an index
        // whose query has no embedding ranks precisely as it did before.
        let none = idx.retrieve_with_embed(&meta, &hist, StyleQuery::new(None, None, w), 1, Path::new("q.arw"));
        assert_eq!(none[0].stem, "far-in-embedding", "no query vector leaves the old ranking");
        // ...and a zero weight is the term's ABSENCE, so the same query WITH a
        // vector ranks like the query without one.
        let off = idx.retrieve_with_embed(
            &meta, &hist, StyleQuery::new(Some(&u), None, RetrievalWeights::FEATURE_ONLY), 1,
            Path::new("q.arw"),
        );
        assert_eq!(off[0].stem, "far-in-embedding", "W_EMB=0 removes the term entirely");
    }

    /// The vocabulary is a PARTITION of itself by group, every phrase is
    /// distinct, and its version is what the index stores.
    ///
    /// `assert!(LOOK_VOCAB_VERSION > 0)` used to stand here: a constant-valued
    /// assertion, which clippy refuses because it can only ever restate the
    /// literal it reads. The version's real property is that the BUILDERS
    /// stamp it and the LOADER checks it, which is what the last clause says.
    ///
    /// MUTATION: drop an index from `LOOK_GROUPS`, or list one in two groups,
    /// and the partition assertions fail.
    #[test]
    fn look_vocab_has_one_definition_and_a_version() {
        assert!((24..=40).contains(&LOOK_VOCAB.len()));
        let mut unique = std::collections::BTreeSet::new();
        assert!(LOOK_VOCAB.iter().all(|phrase| unique.insert(*phrase)));
        // Every phrase belongs to EXACTLY one group: `tags_from_scores` takes
        // the winner per group, so a phrase in no group can never be tagged
        // and a phrase in two competes with itself.
        let mut seen = std::collections::BTreeSet::new();
        for group in LOOK_GROUPS {
            assert!(!group.is_empty(), "an empty group can never yield a tag");
            for &i in *group {
                assert!(i < LOOK_VOCAB.len(), "group index {i} is outside the vocabulary");
                assert!(seen.insert(i), "phrase {i} is in two groups");
            }
        }
        assert_eq!(seen.len(), LOOK_VOCAB.len(), "the groups must cover the vocabulary");
        // The version is not a number this test can restate — it is the number
        // an index carries and the loader enforces.
        assert!(
            vocab_version_of(&embed_provenance_string()) == Some(LOOK_VOCAB_VERSION),
            "the provenance string must carry the vocabulary version the loader checks"
        );
    }

    #[test]
    fn tags_take_at_most_one_phrase_per_group() {
        let mut scores = vec![0.0f32; LOOK_VOCAB.len()];
        for (group_no, group) in LOOK_GROUPS.iter().enumerate() {
            for (offset, &idx) in group.iter().enumerate() {
                scores[idx] = (group_no * 10 + offset) as f32;
            }
        }
        let tags = tags_from_scores(&scores);
        assert!(tags.len() <= LOOK_TAGS_K);
        let chosen = LOOK_VOCAB
            .iter()
            .enumerate()
            .filter(|(_, phrase)| tags.iter().any(|tag| phrase.contains(tag)))
            .map(|(idx, _)| idx)
            .collect::<Vec<_>>();
        for group in LOOK_GROUPS {
            assert!(chosen.iter().filter(|idx| group.contains(idx)).count() <= 1);
        }
    }

    #[test]
    fn zero_text_and_desc_weights_reproduce_the_v5_ranking_bit_for_bit() {
        let w = RetrievalWeights { txt: 0.0, desc: 0.0, ..RetrievalWeights::SHIPPED };
        let mut a = plain_exemplar("a");
        a.embed = Some(unit_embed());
        a.desc_embed = Some(unit_embed());
        let mut b = plain_exemplar("b");
        b.embed = Some({ let mut v = unit_embed(); v[0] = -v[0]; v });
        b.desc_embed = Some({ let mut v = unit_embed(); v[0] = -v[0]; v });
        let idx = StyleIndex { version: CURRENT_INDEX_VERSION, mean: vec![0.0; NDIM], std: vec![1.0; NDIM], exemplars: vec![a, b], source_dir: None, looks: Vec::new(), looks_dir: None, embed_provenance: None };
        let meta = crate::decode::Meta { make: "T".into(), model: "T".into(), lens: None, iso: Some(100), shutter: None, aperture: None, focal_length_mm: None, exposure_bias_ev: None, date_time: None, width: 100, height: 100, as_shot_wb_coeffs: [1.0; 4] };
        let hist = crate::decode::Histogram { luma: vec![1; 256], r: vec![1; 256], g: vec![1; 256], b: vec![1; 256], clip_black_pct: 0.0, clip_white_pct: 0.0, sample_pixels: 1 };
        let old = idx.retrieve(&meta, &hist, 2, Path::new("q.arw")).iter().map(|e| e.stem.clone()).collect::<Vec<_>>();
        let text = { let mut v = unit_embed(); v[0] = -v[0]; v };
        let now = idx.retrieve_with_embed(&meta, &hist, StyleQuery::new(Some(&unit_embed()), Some(&text), w), 2, Path::new("q.arw")).iter().map(|e| e.stem.clone()).collect::<Vec<_>>();
        assert_eq!(old, now);
        // Bit for bit, not merely same-order: with both text weights at 0 the
        // terms are literal `0.0`, never `0.0 * z` (which would put a signed
        // zero into the sum).
        for (e, t) in idx.score_candidates(&meta, &hist, StyleQuery::new(Some(&unit_embed()), Some(&text), w), Path::new("q.arw")) {
            assert_eq!(t.txt.to_bits(), 0.0f64.to_bits(), "{} carries a signed zero", e.stem);
            assert_eq!(t.desc.to_bits(), 0.0f64.to_bits(), "{} carries a signed zero", e.stem);
        }
    }

    #[test]
    fn retrieval_terms_vanish_when_either_side_lacks_the_vector() {
        let u = unit_embed();
        assert_eq!(embed_distance(None, Some(&u), 2.0), 0.0);
        assert_eq!(embed_distance(Some(&u), None, 2.0), 0.0);
        assert_eq!(embed_distance(Some(&u), Some(&u[..4]), 2.0), 0.0);
    }

    /// The stored provenance names the TOKENIZER as well as the checkpoint, and
    /// the vocabulary version it records is ENFORCED on load.
    ///
    /// Two indices built from one checkpoint through two different tokenizer
    /// doors have text vectors at cosine 0.72-0.78 of each other (this batch's
    /// F-11) and used to carry byte-identical provenance. And the `vocab-vN`
    /// stamp was written from the first release and checked nowhere, so a
    /// phrase-list change would have left every stored score describing a
    /// vocabulary this build no longer has.
    ///
    /// The LOOKS are dropped on a mismatch, not the whole file: the RAW half's
    /// features, settings and image vectors do not depend on the phrase list,
    /// and refusing the file would cost an hour-long build over the half that
    /// is cheap to rebuild.
    ///
    /// MUTATION: drop the `tokenizer=` field, or the version check in `load`,
    /// and this fails.
    #[test]
    fn index_provenance_names_the_tokenizer_and_the_vocabulary_version_is_enforced() {
        let p = embed_provenance_string();
        assert!(p.contains(crate::embed::MODEL_REPO), "{p}");
        assert!(p.contains(crate::embed::MODEL_REVISION), "{p}");
        assert!(
            p.contains(&format!("tokenizer={}@", crate::embed::TEXT_TOKENIZER_CLASS)),
            "the door must be recorded, not only the checkpoint: {p}"
        );
        assert_eq!(vocab_version_of(&p), Some(LOOK_VOCAB_VERSION));
        // An index written before the field existed, or with an unparseable
        // stamp, is UNKNOWN - never a match.
        assert_eq!(vocab_version_of("google/x@abc"), None);
        assert_eq!(vocab_version_of("google/x@abc vocab-vNaN"), None);

        let dir = crate::test_dir("style-vocab-version");
        let path = dir.join("style-index.json");
        let look = LookExemplar {
            stem: "finished".into(), path: "finished.jpg".into(), embed: unit_embed(),
            tags: vec!["warm golden tones".into()], vocab_scores: None, desc: None,
            desc_embed: None,
        };
        let write = |provenance: &str| {
            StyleIndex {
                version: CURRENT_INDEX_VERSION, mean: vec![0.0; NDIM], std: vec![1.0; NDIM],
                exemplars: vec![plain_exemplar("raw")], source_dir: Some("raws".into()),
                looks: vec![look.clone()], looks_dir: Some("looks".into()),
                embed_provenance: Some(provenance.to_string()),
            }
            .save(&path)
            .unwrap();
        };
        // Matching version: both halves load.
        write(&embed_provenance_string());
        let ok = StyleIndex::load(&path).unwrap();
        assert_eq!(ok.exemplars.len(), 1);
        assert_eq!(ok.looks.len(), 1, "a matching vocabulary keeps the looks");
        // A FUTURE (or past) vocabulary: the looks go, the RAW half stays.
        write(&embed_provenance_string().replace(
            &format!("vocab-v{LOOK_VOCAB_VERSION}"),
            &format!("vocab-v{}", LOOK_VOCAB_VERSION + 7),
        ));
        let stale = StyleIndex::load(&path).unwrap();
        assert_eq!(stale.exemplars.len(), 1, "the RAW half does not depend on the phrase list");
        assert!(stale.looks.is_empty(), "a stale look vocabulary must not be scored against");
        assert!(stale.looks_dir.is_none(), "…and its provenance goes with it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A looks-only index stays readable by a build that predates the look
    /// block — the v1.0.0 guard rail, checked instead of assumed.
    ///
    /// `save` merges the two halves, so a file with looks and no RAW exemplars
    /// only arises on a machine that built a look library before ever building
    /// the RAW one. main v1.0.0's reader has five fields, no
    /// `deny_unknown_fields` and no empty-exemplar refusal, so the three added
    /// fields are additive in the FORMAT sense too — but "serde ignores unknown
    /// fields by default" is a claim about a derive attribute nobody re-reads.
    /// This deserialises a real looks-only index through a struct with exactly
    /// main's shape, and reads main's own five-field file back through this
    /// one, so the compatibility is a test rather than a comment.
    ///
    /// MUTATION: bump `CURRENT_INDEX_VERSION` past main's
    /// `READABLE_INDEX_VERSIONS` and the version assertion fails.
    #[test]
    fn a_looks_only_index_stays_readable_by_a_pre_look_build() {
        // main v1.0.0's `StyleIndex`, field for field (cfc8b3d src/style.rs).
        #[derive(serde::Deserialize)]
        struct V1Index {
            version: u32,
            mean: Vec<f32>,
            std: Vec<f32>,
            exemplars: Vec<serde_json::Value>,
            #[serde(default)]
            source_dir: Option<String>,
        }
        let looks_only = StyleIndex {
            version: CURRENT_INDEX_VERSION,
            mean: vec![0.0; NDIM],
            std: vec![1.0; NDIM],
            exemplars: Vec::new(),
            source_dir: None,
            looks: vec![LookExemplar {
                stem: "finished".into(), path: "finished.jpg".into(), embed: unit_embed(),
                tags: vec!["warm golden tones".into()], vocab_scores: None, desc: None,
                desc_embed: None,
            }],
            looks_dir: Some("looks".into()),
            embed_provenance: Some(embed_provenance_string()),
        };
        let json = serde_json::to_string(&looks_only).expect("serialise");
        let old: V1Index =
            serde_json::from_str(&json).expect("a pre-look build must still parse this index");
        // main reads versions 4 and 5 and refuses anything else outright, so the
        // look block may not ride on a version bump.
        assert!(matches!(old.version, 4 | 5), "version {} is outside main's readable set", old.version);
        assert_eq!(old.mean.len(), NDIM);
        assert_eq!(old.std.len(), NDIM);
        assert!(old.exemplars.is_empty(), "the RAW half really is empty in this file");
        assert!(old.source_dir.is_none());
        // …and the other direction: main's own five-field file loads here with
        // the look block defaulting to absent, not to a phantom library.
        let v1 = format!(
            "{{\"version\":5,\"mean\":{m},\"std\":{s},\"exemplars\":[],\"source_dir\":\"raws\"}}",
            m = serde_json::to_string(&vec![0.0f32; NDIM]).unwrap(),
            s = serde_json::to_string(&vec![1.0f32; NDIM]).unwrap(),
        );
        let back: StyleIndex =
            serde_json::from_str(&v1).expect("main's own shape must parse here");
        assert!(back.looks.is_empty());
        assert!(back.looks_dir.is_none());
        assert!(back.embed_provenance.is_none());
        assert_eq!(back.source_dir.as_deref(), Some("raws"));
    }

    /// The shipped weights and the shipped text VARIANT are the ones the
    /// harness measured, and the two halves of the file agree about them.
    ///
    /// S1 shipped `W_EMB = 4, W_TXT = 0, W_DESC = 4` in the RAW variant, off a
    /// grid whose query-text proxy and `desc_embed` vectors were both the
    /// exemplar's TAG STRING — no exemplar had a description to be either.
    /// S2's recalibration (`scripts/calibrate_style_retrieval.py --index
    /// <store>/style-index.json`, 169 described exemplars / 156
    /// settings-bearing queries, 196 grid rows per proxy, seeded paired
    /// bootstrap) sweeps BOTH proxies — the exemplar's own prose, and its tag
    /// string — and recommends `W_EMB=4, W_TXT=4, W_DESC=0.5,
    /// variant=standardised` under the PROSE proxy: MAE 0.664818 against the
    /// 14-dim baseline 0.713143, improvement +0.048325, CI
    /// [+0.024290, +0.078587]. Under the TAG proxy nothing beats the text-free
    /// row in either variant, which is the answer to S2's own question: it is
    /// the prose, not the vocabulary, that earns the text terms.
    ///
    /// The old `(4, 0, 4)` raw point is now 0.698491 — WORSE than the
    /// text-free `(4, 0, 0)` at 0.695233 — so the previous numbers could not
    /// simply be left in place.
    ///
    /// MUTATION: flip `STANDARDISE_TEXT_TERMS`, or move any of the three
    /// weights, and this fails — which is the point: the numbers in
    /// TECH_STACK and README are then stale too.
    #[test]
    fn the_shipped_text_variant_is_the_measured_one() {
        assert_eq!(W_EMB_DEFAULT, 4.0);
        assert_eq!(W_TXT_DEFAULT, 4.0);
        assert_eq!(W_DESC_DEFAULT, 0.5);
        assert_eq!(RetrievalWeights::SHIPPED.emb, W_EMB_DEFAULT);
        assert_eq!(RetrievalWeights::SHIPPED.txt, W_TXT_DEFAULT);
        assert_eq!(RetrievalWeights::SHIPPED.desc, W_DESC_DEFAULT);
        assert_eq!(RetrievalWeights::SHIPPED.look, W_LOOK_DEFAULT);

        // The VARIANT is stated behaviourally, never as `assert!(CONST)`:
        // a constant assertion is the vacuous falsifier this batch removed
        // elsewhere (and clippy refuses it). The shipped door must Z-SCORE the
        // gaps over the candidate set and say that it did.
        let gaps = [Some(0.10), Some(0.20), Some(0.60)];
        let shipped = text_term(&gaps, 2.0);
        assert!(shipped.standardised, "the shipped variant must standardise");
        assert!(
            (shipped.terms.iter().sum::<f64>()).abs() < 1e-12,
            "a z-scored term is centred on the candidate set: {:?}",
            shipped.terms
        );
        // The other variant is not dead code — it is one flag away, and it
        // still weights the raw gap.
        let other = raw_term(&gaps, 2.0);
        assert!(!other.standardised);
        assert_eq!(other.terms, vec![0.2, 0.4, 1.2]);
        // A non-zero W_DESC is what makes the look block's "and direction"
        // wording true, so the shipped defaults must license it.
        let q = StyleQuery::new(None, Some(&[1.0]), RetrievalWeights::SHIPPED);
        assert!(
            StyleIndex::look_ranked_by_direction(q),
            "with W_DESC shipping non-zero, a direction really does rank the looks"
        );
    }

    /// The two builders' vocabulary scratch files cannot collide.
    ///
    /// Both wrote `autoshop-look-vocab-<pid>.txt` and both DELETED it when
    /// finished, so two builds in one process (the web server's request
    /// threads) shared one path and whichever finished first took the other's
    /// phrase list away mid-run.
    ///
    /// MUTATION: drop `who` (or the sequence number) from
    /// `vocab_scratch_path` and the distinctness assertions fail.
    #[test]
    fn the_vocabulary_scratch_file_is_named_per_builder_and_run() {
        let dir = Path::new("scratch");
        let a = vocab_scratch_path(dir, "raw");
        let b = vocab_scratch_path(dir, "looks");
        let c = vocab_scratch_path(dir, "raw");
        assert_ne!(a, b, "the two builders must not share a path");
        assert_ne!(a, c, "…nor two runs of the same builder");
        for p in [&a, &b, &c] {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.contains(&std::process::id().to_string()), "{name}");
            assert!(name.starts_with("autoshop-look-vocab-"), "{name}");
        }
    }

    /// F-14: the text term is a Z-SCORE over the candidate set, not a raw
    /// `1 - cos`.
    ///
    /// SigLIP image-to-text cosines are tiny and tightly clustered: in the S1
    /// transcripts the raw term sat at 7.72-7.85 for all four neighbours - a
    /// spread of 0.13 against a 14-dim spread of 2.5 - so the weight bought
    /// almost no reordering and a calibration over it would "find" 0 for the
    /// wrong reason. Standardising makes the term commensurate with the block
    /// it is added to.
    ///
    /// MUTATION: return `plain()` unconditionally from `standardise` and the
    /// mean/spread assertions fail; make the ranking standardise while
    /// `STANDARDISE_TEXT_TERMS` is off (or the reverse) and the agreement
    /// assertion fails.
    #[test]
    fn text_term_is_standardised_over_the_candidate_set() {
        // Four candidates whose text cosines are tightly clustered, exactly
        // like the real ones.
        let text = unit_embed();
        let mut idx_exemplars = Vec::new();
        for (i, scale) in [0.980f32, 0.976, 0.972, 0.968].iter().enumerate() {
            let mut e = plain_exemplar(&format!("c{i}"));
            // A vector whose cosine with `text` is `scale`, built by mixing in
            // one orthogonal direction.
            let mut v = unit_embed();
            let ortho = (1.0f32 - scale * scale).sqrt();
            v.iter_mut().for_each(|x| *x *= scale);
            v[0] += ortho;
            let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            v.iter_mut().for_each(|x| *x /= n);
            e.embed = Some(v);
            idx_exemplars.push(e);
        }
        let idx = StyleIndex {
            version: CURRENT_INDEX_VERSION, mean: vec![0.0; NDIM], std: vec![1.0; NDIM],
            exemplars: idx_exemplars, source_dir: None, looks: Vec::new(), looks_dir: None,
            embed_provenance: None,
        };
        let meta = fixture_meta();
        let hist = fixture_histogram();
        let w = RetrievalWeights { emb: 0.0, txt: 1.0, desc: 0.0, look: 1.0 };
        let scored = idx.score_candidates(
            &meta, &hist, StyleQuery::new(None, Some(&text), w), Path::new("q.arw"),
        );
        assert_eq!(scored.len(), 4);
        // Premise: the RAW gaps really are clustered - else this proves nothing.
        let raw: Vec<f64> = scored.iter().map(|(_, t)| t.txt_gap.unwrap()).collect();
        let raw_spread = raw.iter().cloned().fold(f64::MIN, f64::max)
            - raw.iter().cloned().fold(f64::MAX, f64::min);
        assert!(raw_spread < 0.05, "premise: the raw cosines are clustered ({raw_spread})");
        // Standardising THESE gaps — the ones the ranking really produced, not
        // a toy triple — gives z-scores: mean 0, unit spread, and a spread the
        // ranking can act on (~2 instead of ~0.03).
        let z = standardise(&raw.iter().copied().map(Some).collect::<Vec<_>>(), 1.0);
        let mean = z.terms.iter().sum::<f64>() / z.terms.len() as f64;
        let sd = (z.terms.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / z.terms.len() as f64).sqrt();
        assert!(mean.abs() < 1e-9, "the standardised term is centred: {mean}");
        assert!((sd - 1.0).abs() < 1e-9, "…and scaled to unit spread: {sd}");
        assert!(z.standardised);
        let spread = z.terms.iter().cloned().fold(f64::MIN, f64::max)
            - z.terms.iter().cloned().fold(f64::MAX, f64::min);
        assert!(spread > 1.0, "the standardised term actually separates candidates: {spread}");
        // …and the RANKING reports the variant it actually used. The harness
        // measured the raw variant as the better one on the real corpus
        // (`the_shipped_text_variant_is_the_measured_one`), so this test tracks
        // the flag rather than asserting the design argument's preference: what
        // must never happen is a ranking that standardises while the diagnostic
        // says it did not, or the reverse.
        assert!(scored.iter().all(|(_, t)| t.txt_standardised == STANDARDISE_TEXT_TERMS));
        if !STANDARDISE_TEXT_TERMS {
            for ((_, t), gap) in scored.iter().zip(&raw) {
                assert!((t.txt - gap).abs() < 1e-12, "the raw variant weights the gap itself");
            }
        }
        // The ORDER is unchanged - standardisation is affine within a query,
        // which is the point: it rescales the term, it does not invent one.
        let by_gap: Vec<&str> = {
            let mut v: Vec<_> = scored.iter().collect();
            v.sort_by(|a, b| a.1.txt_gap.unwrap().total_cmp(&b.1.txt_gap.unwrap()));
            v.into_iter().map(|(e, _)| e.stem.as_str()).collect()
        };
        let by_term: Vec<&str> = {
            let mut v: Vec<_> = scored.iter().collect();
            v.sort_by(|a, b| a.1.txt.total_cmp(&b.1.txt));
            v.into_iter().map(|(e, _)| e.stem.as_str()).collect()
        };
        assert_eq!(by_gap, by_term);
    }

    /// …and below three comparable candidates it falls back to the RAW gap and
    /// SAYS SO, instead of dividing by a spread that does not exist.
    ///
    /// MUTATION: drop the `live.len() < MIN_STANDARDISATION_CANDIDATES` guard
    /// and the two-candidate case divides by a one-sample spread; drop the
    /// `standardised` flag and the disclosure assertion fails.
    #[test]
    fn standardisation_falls_back_to_raw_below_three_candidates_and_discloses() {
        // Two live gaps: below the floor.
        let two = standardise(&[Some(0.10), Some(0.20), None], 2.0);
        assert!(!two.standardised, "two candidates cannot define a spread");
        assert_eq!(two.terms, vec![0.2, 0.4, 0.0], "the RAW gap, weighted");
        // Three, but all identical: a zero spread is the other degenerate case.
        let flat = standardise(&[Some(0.3), Some(0.3), Some(0.3)], 2.0);
        assert!(!flat.standardised);
        assert_eq!(flat.terms, vec![0.6, 0.6, 0.6]);
        // Three distinct: standardised.
        let ok = standardise(&[Some(0.1), Some(0.2), Some(0.4)], 1.0);
        assert!(ok.standardised);
        // A zero weight is the term's ABSENCE in every arm, with no signed zero.
        for raw in [
            vec![Some(0.1), Some(0.2)],
            vec![Some(0.1), Some(0.2), Some(0.4)],
            vec![None, None, None],
        ] {
            let off = standardise(&raw, 0.0);
            assert!(!off.standardised);
            assert!(off.terms.iter().all(|v| v.to_bits() == 0.0f64.to_bits()));
        }
        // …and the fallback reaches the REPORT, so a reader of `style-query`
        // can tell a z-score from a raw gap.
        let mut a = plain_exemplar("a");
        a.embed = Some(unit_embed());
        let mut b = plain_exemplar("b");
        b.embed = Some({ let mut v = unit_embed(); v[0] = -v[0]; v });
        let idx = StyleIndex {
            version: CURRENT_INDEX_VERSION, mean: vec![0.0; NDIM], std: vec![1.0; NDIM],
            exemplars: vec![a, b], source_dir: None, looks: Vec::new(), looks_dir: None,
            embed_provenance: None,
        };
        let text = unit_embed();
        let w = RetrievalWeights { emb: 0.0, txt: 1.0, desc: 0.0, look: 1.0 };
        let scored = idx.score_candidates(
            &fixture_meta(), &fixture_histogram(),
            StyleQuery::new(None, Some(&text), w), Path::new("q.arw"),
        );
        assert_eq!(scored.len(), 2);
        assert!(scored.iter().all(|(_, t)| !t.txt_standardised), "two candidates: disclosed as raw");
        assert!(scored.iter().all(|(_, t)| t.txt_gap.is_some()), "…and the raw gap is carried");
    }

    /// W_LOOK's SCALE cannot change which look wins, because no other term
    /// ranks looks against each other while W_TXT and W_DESC ship at 0.
    ///
    /// This is what makes 1.0 a normalisation rather than an uncalibrated
    /// guess - the harness never measured it and does not need to. If either
    /// text weight ever ships non-zero, W_LOOK becomes a real ratio, and this
    /// test starts failing for that reason.
    ///
    /// MUTATION: give `retrieve_looks_with_terms` a second look-ranking term
    /// that does not scale with `weights.look`, and the orders diverge.
    #[test]
    fn look_weight_scale_does_not_change_look_order() {
        let q = unit_embed();
        let looks: Vec<LookExemplar> = [0.99f32, 0.5, 0.1, -0.4]
            .iter()
            .enumerate()
            .map(|(i, scale)| {
                let mut v = unit_embed();
                let ortho = (1.0f32 - scale * scale).max(0.0).sqrt();
                v.iter_mut().for_each(|x| *x *= scale);
                v[0] += ortho;
                let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                v.iter_mut().for_each(|x| *x /= n);
                LookExemplar {
                    stem: format!("look-{i}"), path: format!("{i}.jpg"), embed: v,
                    tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
                }
            })
            .collect();
        let idx = StyleIndex {
            version: CURRENT_INDEX_VERSION, mean: vec![0.0; NDIM], std: vec![1.0; NDIM],
            exemplars: Vec::new(), source_dir: None, looks, looks_dir: None,
            embed_provenance: None,
        };
        let order = |look: f64| -> Vec<String> {
            let w = RetrievalWeights { emb: 0.0, txt: 0.0, desc: 0.0, look };
            idx.retrieve_looks(StyleQuery::new(Some(&q), None, w), 4)
                .into_iter()
                .map(|l| l.stem.clone())
                .collect()
        };
        let one = order(1.0);
        assert_eq!(one.len(), 4, "premise: all four looks are ranked");
        for scale in [0.25, 1.0, 3.0, 17.5] {
            assert_eq!(order(scale), one, "W_LOOK={scale} must not reorder the look library");
        }
        // …and the shipped weight IS 1.0, i.e. the normalisation this states.
        assert_eq!(W_LOOK_DEFAULT, 1.0);
    }

    /// One scoring helper, used by the ranking AND by the diagnostic.
    ///
    /// `style_query_uses_the_pipeline_retrieval_path` used to be a grep for the
    /// string "pipeline::retrieve_style(" in main.rs, which says nothing about
    /// the NUMBERS: `distance_components` re-implemented the 14-dim sum, so the
    /// diagnostic could print terms the ranking never used. Now both read
    /// `score_candidates`, and this compares them on a fixture.
    ///
    /// MUTATION: give `distance_components` its own 14-dim loop again (say,
    /// with `WEIGHTS[j]` dropped) and the totals stop matching.
    #[test]
    fn the_diagnostic_prints_the_terms_the_ranking_used() {
        let mut ex = Vec::new();
        for i in 0..5 {
            let mut e = plain_exemplar(&format!("e{i}"));
            e.feat = (0..NDIM).map(|j| (i * NDIM + j) as f32 * 0.03).collect();
            let mut v = unit_embed();
            v[i] = -v[i];
            e.embed = Some(v.clone());
            e.desc_embed = Some(v);
            ex.push(e);
        }
        let idx = StyleIndex {
            version: CURRENT_INDEX_VERSION, mean: vec![0.0; NDIM], std: vec![1.0; NDIM],
            exemplars: ex, source_dir: None, looks: Vec::new(), looks_dir: None,
            embed_provenance: None,
        };
        let (meta, hist) = (fixture_meta(), fixture_histogram());
        let img = unit_embed();
        let text = { let mut v = unit_embed(); v[3] = -v[3]; v };
        let w = RetrievalWeights { emb: 4.0, txt: 2.0, desc: 1.0, look: 1.0 };
        let query = StyleQuery::new(Some(&img), Some(&text), w);
        let ranked = idx.retrieve_with_embed(&meta, &hist, query, 5, Path::new("q.arw"));
        assert_eq!(ranked.len(), 5, "premise: every exemplar is a candidate");
        // The diagnostic's own totals, in the ranking's order, must be
        // non-decreasing - i.e. they ARE the keys the sort used.
        let mut previous = f64::NEG_INFINITY;
        for e in &ranked {
            let t = idx.distance_components(&meta, &hist, query, Path::new("q.arw"), e);
            assert!(
                t.total() >= previous,
                "{} scores {} after {previous}: the diagnostic is not the ranking",
                e.stem,
                t.total()
            );
            previous = t.total();
            // …and every printed part is really additive to the printed whole.
            assert!((t.total() - (t.d14 + t.emb + t.txt + t.desc)).abs() < 1e-12);
        }
        // An exemplar that is NOT a candidate (the query itself) answers
        // all-zero rather than a distance nobody computed.
        let self_terms = idx.distance_components(
            &meta, &hist, query, Path::new("e0.arw"), &idx.exemplars[0],
        );
        assert_eq!(self_terms, DistanceTerms::default());
    }

    /// A stand-in embedding sidecar that answers the BATCH TEXT door and
    /// records what it was asked.
    ///
    /// It writes a call line per invocation and copies the manifest it was
    /// given, so a test can state both halves of F-12: what the builders send,
    /// and how many processes it took.
    fn text_stub(dir: &Path, vectors: usize) -> crate::embed::EmbedOpts {
        // A valid `{"text_vectors":[...]}` payload the stub just copies out:
        // the vectors themselves are the identity of nothing, but they must
        // pass the bridge's width / finiteness / unit-norm gate.
        let one = format!("[{}]", vec![
            format!("{:.10}", 1.0f32 / (crate::embed::EMBED_DIM as f32).sqrt());
            crate::embed::EMBED_DIM
        ].join(","));
        let payload = format!(
            "{{\"model\":\"stub\",\"dim\":{},\"norm\":\"l2\",\"text_vectors\":[{}]}}\n",
            crate::embed::EMBED_DIM,
            vec![one; vectors].join(",")
        );
        std::fs::write(dir.join("vectors.json"), payload).unwrap();
        let script = dir.join("embed.py");
        std::fs::write(&script, "# stand-in\n").unwrap();
        // argv is `-E <script> --text-manifest <manifest> --output <out>`, so
        // %4 is the manifest and %6 the output.
        let python_bin = crate::write_stand_in(
            dir,
            "embed-text-stub",
            "@echo call>>\"%~dp0calls.log\"\r\n\
             @copy /y \"%~4\" \"%~dp0manifest.seen\" >nul\r\n\
             @copy /y \"%~dp0vectors.json\" \"%~6\" >nul\r\n\
             @exit /b 0\r\n",
            &format!(
                "echo call >> \"{d}/calls.log\"\ncp \"$4\" \"{d}/manifest.seen\"\n\
                 cp \"{d}/vectors.json\" \"$6\"\nexit 0\n",
                d = dir.display()
            ),
        );
        crate::embed::EmbedOpts { python_bin, script, text_file: None, vocab_file: None }
    }

    fn stub_calls(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("calls.log"))
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    }

    fn stub_manifest(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("manifest.seen"))
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["text"].as_str().unwrap().to_string())
            .collect()
    }

    /// F-12, the RULE: `desc_embed` is the vector of the description when a
    /// record has one, of the TAG STRING when it does not, and absent when the
    /// record has neither.
    ///
    /// Before this batch `StyleIndex::build` wrote `desc_embed: None` for every
    /// RAW exemplar, so the W_DESC term was structurally dead on the RAW index
    /// and its calibrated weight was a number about nothing.
    ///
    /// MUTATION: make `desc_text` return the tags even when a description
    /// exists (or `None` when only tags exist) and the manifest assertion
    /// fails.
    #[test]
    fn desc_embed_is_the_tag_string_vector_when_no_desc() {
        let dir = crate::test_dir("style-desc-embed");
        // Two records get a text, the third has neither desc nor tags.
        let opts = text_stub(&dir, 2);
        let mut records = vec![
            LookExemplar {
                stem: "with-desc".into(), path: "a.jpg".into(), embed: unit_embed(),
                tags: vec!["warm golden tones".into(), "deep blacks".into()],
                vocab_scores: None, desc: Some("a hazy dawn over water".into()), desc_embed: None,
            },
            LookExemplar {
                stem: "tags-only".into(), path: "b.jpg".into(), embed: unit_embed(),
                tags: vec!["vivid saturated colours".into(), "crisp clarity".into()],
                vocab_scores: None, desc: None, desc_embed: None,
            },
            LookExemplar {
                stem: "neither".into(), path: "c.jpg".into(), embed: unit_embed(),
                tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
            },
        ];
        attach_desc_embeddings(&opts, &dir, &mut records, "test");
        assert_eq!(
            stub_manifest(&dir),
            vec![
                "a hazy dawn over water".to_string(),
                "vivid saturated colours, crisp clarity".to_string(),
            ],
            "the description wins; the TAG STRING stands in when there is none"
        );
        assert!(records[0].desc_embed.is_some(), "the described record gets a vector");
        assert!(records[1].desc_embed.is_some(), "the tags-only record gets a vector");
        assert!(records[2].desc_embed.is_none(), "a record with no text gets no vector");
        // …and the vectors land on the RIGHT records: the manifest carries only
        // the live ones, so a naive zip would have given record 2 record 1's
        // vector.
        assert_eq!(stub_calls(&dir), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F-12, the COST: one sidecar process per BUILD, not per record.
    ///
    /// `build_looks` used to call `embed_preview_with_text` once per
    /// photograph purely to embed that photograph's own tag string - a fresh
    /// 1.5 GB model load and a second full image forward pass each time. The
    /// behavioural half is the call count below; the source half pins that
    /// neither builder can grow a per-record text call again without this
    /// test noticing.
    ///
    /// MUTATION: call `embed_desc_texts` once per record inside
    /// `attach_desc_embeddings` and the call count becomes 3.
    #[test]
    fn build_never_reinvokes_the_sidecar_per_look() {
        let dir = crate::test_dir("style-desc-onecall");
        let opts = text_stub(&dir, 3);
        let mut records: Vec<LookExemplar> = (0..3)
            .map(|i| LookExemplar {
                stem: format!("look-{i}"), path: format!("{i}.jpg"), embed: unit_embed(),
                tags: vec![format!("tag {i}")], vocab_scores: None, desc: None, desc_embed: None,
            })
            .collect();
        attach_desc_embeddings(&opts, &dir, &mut records, "test");
        assert_eq!(stub_calls(&dir), 1, "three records, ONE sidecar process");
        assert!(records.iter().all(|r| r.desc_embed.is_some()), "all three got vectors");
        // The source half: both builders reach the batch door, and the
        // per-photo loop in `build_looks` holds no text call of its own.
        let me = production_source();
        assert_eq!(
            me.matches("attach_desc_embeddings(").count() - me.matches("fn attach_desc_embeddings(").count(),
            2,
            "exactly the two builders call the batch door, and nothing else does"
        );
        // The LOOP lives in `build_looks_with` (the wrapper above it only
        // resolves the sidecar options), so that is the body to read: split
        // on the wrapper and this assertion would pass over four lines that
        // contain no loop at all.
        let looks_body = me
            .split("pub fn build_looks_with(")
            .nth(1)
            .and_then(|b| b.split("\n    /// ").next())
            .expect("build_looks_with body");
        assert!(
            looks_body.contains("for (i, f) in files.iter().enumerate()")
                || looks_body.contains("for "),
            "the body read here must be the one with the per-photo loop"
        );
        assert!(
            !looks_body.contains("embed_preview_with_text("),
            "the per-photo loop must not embed text; that is what the batch door replaced"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A look that IS retrieved still contributes nothing to the settings
    /// targets or the blend.
    ///
    /// The old spelling of this test built an index with a look in it and then
    /// called `style_targets(&[])` and `blend_toward(&mut r, &empty_map, 1.0)`
    /// — the look never entered either call, so both assertions held for an
    /// index with no look at all and the test could not fail for its own
    /// reason. This one retrieves through the production path with a query
    /// vector present, ASSERTS the look came back (so the exclusion is a
    /// filter, not an absence), and then checks the targets.
    ///
    /// MUTATION: make `style_targets` fold `LookExemplar::tags` in, or let
    /// `retrieve_style` return looks in the exemplar list, and this fails.
    #[test]
    fn look_library_never_reaches_style_targets_or_blend() {
        let look = LookExemplar {
            stem: "finished".into(), path: "finished.jpg".into(), embed: unit_embed(),
            tags: vec!["warm golden tones".into()],
            vocab_scores: Some(vec![0.0; LOOK_VOCAB.len()]),
            desc: Some("warm".into()), desc_embed: Some(unit_embed()),
        };
        // A RAW exemplar WITH settings sits beside it, so "the targets are
        // empty" cannot be the trivial answer either.
        let mut raw = plain_exemplar("raw-with-settings");
        raw.settings = BTreeMap::from([("contrast".to_string(), 20.0)]);
        raw.embed = Some(unit_embed());
        let idx = StyleIndex {
            version: CURRENT_INDEX_VERSION, mean: vec![0.0; NDIM], std: vec![1.0; NDIM],
            exemplars: vec![raw], source_dir: None, looks: vec![look],
            looks_dir: Some("looks".into()), embed_provenance: None,
        };
        let meta = fixture_meta();
        let hist = fixture_histogram();
        let u = unit_embed();
        let query = StyleQuery::new(Some(&u), None, RetrievalWeights::SHIPPED);
        let ex = idx.retrieve_with_embed(&meta, &hist, query, 4, Path::new("q.arw"));
        let looks = idx.retrieve_looks(query, 2);
        // Premise: BOTH populations answered this query.
        assert_eq!(ex.len(), 1, "the RAW exemplar must be retrieved");
        assert_eq!(looks.len(), 1, "the look must be retrieved - else this proves nothing");
        // The targets come from the RAW side and only from it.
        let targets = style_targets(&ex);
        assert_eq!(targets.get("contrast"), Some(&20.0), "the RAW exemplar's own setting");
        assert_eq!(targets.len(), 1, "a look carries no settings and must add none: {targets:?}");
        // …and the blend moves exactly that one field.
        let mut recipe = EditRecipe::default();
        let before = recipe.clone();
        blend_toward(&mut recipe, &targets, 1.0);
        assert_eq!(recipe.contrast, 20.0);
        assert_eq!(EditRecipe { contrast: before.contrast, ..recipe.clone() }, before);
    }

    #[test]
    fn looks_are_unreachable_without_a_query_vector_and_disclosed() {
        let idx = StyleIndex { version: CURRENT_INDEX_VERSION, mean: vec![0.0; NDIM], std: vec![1.0; NDIM], exemplars: Vec::new(), source_dir: None, looks: vec![LookExemplar { stem: "finished".into(), path: "finished.jpg".into(), embed: unit_embed(), tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None }], looks_dir: None, embed_provenance: None };
        assert!(idx.retrieve_looks(StyleQuery::FEATURES_ONLY, 2).is_empty());
        assert!(crate::rationale::keys::STYLE_LOOKS_UNREACHABLE.contains("look library"));
    }

    /// flag > environment > preference, over an EXPLICIT environment — the
    /// same rule the old test stated by mutating the process (and, for the
    /// duration, every other test in the binary).
    ///
    /// MUTATION: make `resolve_with` ignore `flag`, or read the preference
    /// before the environment, and one of these fails.
    #[test]
    fn embedding_effective_env_wins_when_set_else_pref() {
        let unset = |_: &str| None;
        fn set(v: &str) -> impl Fn(&str) -> Option<String> + '_ {
            move |_: &str| Some(v.to_string())
        }
        // No variable: the preference answers.
        assert!(EmbeddingSwitch::resolve_with(None, true, unset).on());
        assert!(!EmbeddingSwitch::resolve_with(None, false, unset).on());
        // Set: it wins over the preference, in BOTH directions - that is what
        // makes it an override rather than a second default.
        assert!(!EmbeddingSwitch::resolve_with(None, true, set("0")).on());
        assert!(EmbeddingSwitch::resolve_with(None, false, set("1")).on());
        // The CLI flag wins over both.
        assert!(EmbeddingSwitch::resolve_with(Some(true), false, set("0")).on());
        assert!(!EmbeddingSwitch::resolve_with(Some(false), true, set("1")).on());
    }

    /// The retrieval switch and weights are read from the environment in ONE
    /// place each, and no surface IMPLEMENTS a flag by writing that
    /// environment.
    ///
    /// A source invariant, not a behaviour test, because the failure it guards
    /// is a race: `cargo test` runs the binary's tests on parallel threads in
    /// one process, so a single environment write reconfigures every retrieval running
    /// at that moment. Scanning the CLI and the pipeline is what makes the
    /// absence checkable — a behavioural test can only ever sample the schedule
    /// that happened to occur.
    ///
    /// MUTATION: put the flag back as an environment write in `main.rs`
    /// (an unsafe write of ENV_EMBED into the process environment) and this fails.
    #[test]
    fn no_surface_implements_the_embedding_switch_by_writing_the_environment() {
        // BUILT, never written: spelling the banned call out here would put
        // it in this file's own source, and the census below reads this
        // file. A pattern that matches itself is not a census.
        const BAN: &str = concat!("set_", "var");
        for (name, src) in [
            ("main.rs", include_str!("main.rs")),
            ("pipeline.rs", include_str!("pipeline.rs")),
            ("serve.rs", include_str!("serve.rs")),
            ("bin/gui/actions.rs", include_str!("bin/gui/actions.rs")),
            // …and this file's own production half, where the switch and
            // the weights are declared and read.
            ("style.rs (production half)", production_source()),
        ] {
            // CODE lines only, the same rule `embed.rs`'s pinned censuses use:
            // the prose in these files EXPLAINS the environment writes that
            // were removed, and counting an explanation would make the
            // invariant drift with the comment that documents it.
            let offenders: Vec<&str> = src
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with("//"))
                .filter(|l| l.contains(BAN))
                .collect();
            assert!(
                offenders.is_empty(),
                "{name} writes the process environment; the switch and the weights are values: {offenders:?}"
            );
            // The four weight variables are not merely un-WRITTEN outside
            // this file, they are not NAMED outside it. (`ENV_EMBED` is
            // exempt: `main.rs` mentions it in prose and snapshots it in the
            // test that proves the flag no longer writes it. This file itself
            // is exempt for the obvious reason - it is where they are DECLARED
            // and read.)
            if name.starts_with("style.rs") {
                continue;
            }
            for banned in [ENV_EMBED_WEIGHT, ENV_TEXT_WEIGHT, ENV_DESC_WEIGHT, ENV_LOOK_WEIGHT] {
                assert_eq!(
                    src.matches(banned).count(),
                    0,
                    "{name} names {banned}; it is read only in style.rs"
                );
            }
        }
        // …and the reads themselves are single-sited here.
        let me = production_source();
        assert_eq!(me.matches("std::env::var(k).ok()").count(), 1, "one weight read");
        // TWO switch reads since S2, and exactly two: `EmbeddingSwitch::resolve`
        // and `DescribeSwitch::resolve`, one site each. A third would mean a
        // surface had grown its own read of a switch that is supposed to be a
        // VALUE passed down from the command's door.
        assert_eq!(me.matches("std::env::var_os(k)").count(), 2, "one read per switch");
    }

    /// The weights come from the environment in ONE place, and a value that
    /// would invert the ranking is refused there.
    ///
    /// MUTATION: drop the `>= 0.0` filter in `RetrievalWeights::resolve` and
    /// the negative case fails — a negative weight ranks the LEAST similar
    /// photo first.
    #[test]
    fn retrieval_weights_come_from_one_place_and_refuse_a_ranking_inversion() {
        let env = |k: &str| match k {
            ENV_EMBED_WEIGHT => Some("2.5".to_string()),
            ENV_TEXT_WEIGHT => Some("-1".to_string()),
            ENV_DESC_WEIGHT => Some("nonsense".to_string()),
            ENV_LOOK_WEIGHT => Some("inf".to_string()),
            _ => None,
        };
        let w = RetrievalWeights::resolve(env);
        assert_eq!(w.emb, 2.5, "a parseable non-negative override is taken");
        assert_eq!(w.txt, W_TXT_DEFAULT, "a negative weight falls back to the shipped one");
        assert_eq!(w.desc, W_DESC_DEFAULT, "an unparseable weight falls back");
        assert_eq!(w.look, W_LOOK_DEFAULT, "a non-finite weight falls back");
        assert_eq!(RetrievalWeights::resolve(|_| None), RetrievalWeights::SHIPPED);
    }

    /// The runtime half of the two `const _` capacity gates: the per-record
    /// bound must actually hold the LARGEST record either population can
    /// produce, or the compile-time arithmetic above it is true and
    /// meaningless.
    ///
    /// Both populations are measured, because `save` merges them into ONE
    /// document and the file cap has to hold the sum. Before F-5 only the RAW
    /// population was counted while `load` admitted `MAX_STYLE_EXEMPLARS`
    /// looks beside it.
    ///
    /// MUTATION: raise `MAX_DESC_CHARS` without raising `MAX_EXEMPLAR_BYTES`,
    /// or drop a field from either maximal record, and the byte assert fails.
    #[test]
    fn capacity_constants_hold_two_vectors_and_the_scores() {
        // Worst-case f32 text, not a round number: serde_json writes the
        // shortest round-tripping decimal, and this is the longest one a
        // normalised embedding element can take.
        let worst = vec![-1.234_567_8e-38f32; crate::embed::EMBED_DIM];
        let max_look = LookExemplar {
            // 255 = the filesystem's own name cap; MAX_STEM_CHARS is the
            // DISCLOSURE truncation and does not bound what is stored.
            stem: "s".repeat(255), path: "p".repeat(512), embed: worst.clone(),
            tags: vec!["t".repeat(128); LOOK_TAGS_K],
            vocab_scores: Some(vec![-1.234_567_8e-38; LOOK_VOCAB.len()]),
            desc: Some("d".repeat(MAX_DESC_CHARS)), desc_embed: Some(worst.clone()),
        };
        // A maximal RAW exemplar is the bigger of the two: it carries
        // everything a look does PLUS the 14-dim feature, the settings map,
        // the curve and the family summary.
        let max_raw = StyleExemplar {
            stem: "s".repeat(255),
            feat: vec![-1.234_567_8e-38f32; NDIM],
            tag: "ultrawide/bright/goldenish/landscape".into(),
            settings: [
                "exposure", "temperature_K", "contrast", "highlights", "shadows", "whites",
                "blacks", "vibrance", "clarity", "tint", "saturation", "dehaze",
            ]
            .iter()
            .map(|k| ((*k).to_string(), -1.234_567_8e-38f32))
            .collect(),
            curve: Some([-1.234_567_8e-38; 2]),
            path: Some("p".repeat(512)),
            families: Some(crate::eval::FamilySummary {
                hsl: [-1.234_567_8e-38; 3], grade: [-1.234_567_8e-38; 2], rgb_curves: 3,
            }),
            embed: Some(worst.clone()),
            tags: vec!["t".repeat(128); LOOK_TAGS_K],
            vocab_scores: Some(vec![-1.234_567_8e-38; LOOK_VOCAB.len()]),
            desc: Some("d".repeat(MAX_DESC_CHARS)),
            desc_embed: Some(worst),
        };
        let look_bytes = serde_json::to_vec(&max_look).unwrap().len();
        let raw_bytes = serde_json::to_vec(&max_raw).unwrap().len();
        // Printed so the comment on `MAX_EXEMPLAR_BYTES` can quote measured
        // numbers instead of asserted ones (`cargo test -- --nocapture`).
        println!("maximal RAW exemplar {raw_bytes} B, maximal look {look_bytes} B, bound {MAX_EXEMPLAR_BYTES} B");
        assert!(raw_bytes <= MAX_EXEMPLAR_BYTES, "maximal RAW exemplar is {raw_bytes} bytes (bound {MAX_EXEMPLAR_BYTES})");
        assert!(look_bytes <= MAX_EXEMPLAR_BYTES, "maximal look record is {look_bytes} bytes (bound {MAX_EXEMPLAR_BYTES})");
        // The sum of BOTH capped populations, which is what the file holds.
        assert!(
            MAX_STYLE_EXEMPLARS * raw_bytes + MAX_LOOK_EXEMPLARS * look_bytes
                <= MAX_STYLE_INDEX_BYTES,
            "both maximal populations together exceed the file cap"
        );
    }

    #[test]
    fn look_build_refuses_without_the_sidecar_and_says_why() {
        let dir = std::env::temp_dir().join(format!("autoshop-look-refuse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let absent_describer = || crate::describe::DescribeOpts {
            python_bin: "python".into(),
            script: "this-sidecar-does-not-exist.py".into(),
        };
        let err = match StyleIndex::build_looks_with(
            BuildSidecars {
                embed: crate::embed::EmbedOpts {
                    python_bin: "python".into(),
                    script: "this-sidecar-does-not-exist.py".into(),
                    text_file: None,
                    vocab_file: None,
                },
                describe: absent_describer(),
                scratch: dir.join("scratch"),
            },
            &dir,
            EmbeddingSwitch::ON,
            DescribeSwitch::OFF,
            &|_| {},
        ) {
            Ok(_) => panic!("a look build without a sidecar must refuse"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("requires the style-embedding sidecar"), "{err}");
        // …and the switch alone refuses too, with a PRESENT sidecar: the two
        // halves of the guard are separate conditions.
        let present = crate::embed::EmbedOpts {
            python_bin: "python".into(),
            script: std::path::PathBuf::from(file!()),
            text_file: None,
            vocab_file: None,
        };
        assert!(present.available(), "premise: this script exists");
        let off = match StyleIndex::build_looks_with(
            BuildSidecars { embed: present, describe: absent_describer(), scratch: dir.join("scratch") },
            &dir,
            EmbeddingSwitch::OFF,
            DescribeSwitch::OFF,
            &|_| {},
        ) {
            Ok(_) => panic!("a look build with the switch off must refuse"),
            Err(err) => err.to_string(),
        };
        assert!(off.contains("requires the style-embedding sidecar"), "{off}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn look_build_rewrites_only_the_looks_block() {
        let dir = std::env::temp_dir().join(format!("autoshop-look-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir); std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("style-index.json");
        let raw = plain_exemplar("raw");
        StyleIndex { version: CURRENT_INDEX_VERSION, mean: vec![0.0; NDIM], std: vec![1.0; NDIM], exemplars: vec![raw], source_dir: Some("raws".into()), looks: Vec::new(), looks_dir: None, embed_provenance: None }.save(&path).unwrap();
        let look = LookExemplar { stem: "look".into(), path: "look.jpg".into(), embed: unit_embed(), tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None };
        StyleIndex { version: CURRENT_INDEX_VERSION, mean: vec![0.0; NDIM], std: vec![1.0; NDIM], exemplars: Vec::new(), source_dir: None, looks: vec![look], looks_dir: Some("looks".into()), embed_provenance: None }.save(&path).unwrap();
        let merged = StyleIndex::load(&path).unwrap();
        assert_eq!(merged.exemplars.len(), 1); assert_eq!(merged.looks.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    // --- S2: the staged build, and the description pass ----------------------

    /// The IMAGE and TEXT doors of `embed.py`, stubbed in one script — the two
    /// are one process's two modes, and a stub per mode could not observe that
    /// a build called each of them exactly once.
    ///
    /// It ECHOES the manifest back: the answer is mapped by PATH, and the
    /// staged frame names carry a pid and a sequence number, so a fixed
    /// pre-written answer could not name them.
    fn image_text_stub(dir: &Path, texts: usize) -> crate::embed::EmbedOpts {
        std::fs::create_dir_all(dir).unwrap();
        let e = format!("{:.10}", 1.0f32 / (crate::embed::EMBED_DIM as f32).sqrt());
        let vector = vec![e; crate::embed::EMBED_DIM].join(",");
        let scores = vec!["0.5".to_string(); LOOK_VOCAB.len()].join(",");
        // No trailing newline: the stub `type`s this and then echoes the
        // manifest line's own `"path":...}` tail onto the SAME line.
        std::fs::write(
            dir.join("prefix.txt"),
            format!("{{\"dim\":{},\"norm\":\"l2\",\"vector\":[{vector}],\"vocab_scores\":[{scores}],",
                    crate::embed::EMBED_DIM),
        )
        .unwrap();
        std::fs::write(
            dir.join("vectors.json"),
            format!(
                "{{\"model\":\"stub\",\"dim\":{},\"norm\":\"l2\",\"text_vectors\":[{}]}}\n",
                crate::embed::EMBED_DIM,
                vec![format!("[{vector}]"); texts].join(",")
            ),
        )
        .unwrap();
        let script = dir.join("embed.py");
        std::fs::write(&script, "# stand-in\n").unwrap();
        let python_bin = crate::write_stand_in(
            dir,
            "embed-stub",
            "@echo off\r\n\
             setlocal enabledelayedexpansion\r\n\
             echo %~3>>\"%~dp0calls.log\"\r\n\
             copy /y \"%~4\" \"%~dp0manifest.seen\" >nul\r\n\
             if \"%~3\"==\"--text-manifest\" (\r\n\
             copy /y \"%~dp0vectors.json\" \"%~6\" >nul\r\n\
             exit /b 0\r\n\
             )\r\n\
             if exist \"%~6\" del \"%~6\"\r\n\
             for /f \"usebackq delims=\" %%L in (\"%~4\") do (\r\n\
             set \"L=%%L\"\r\n\
             type \"%~dp0prefix.txt\" >>\"%~6\"\r\n\
             echo !L:~1!>>\"%~6\"\r\n\
             )\r\n\
             exit /b 0\r\n",
            &format!(
                "D=\"{d}\"\n\
                 echo \"$3\" >> \"$D/calls.log\"\n\
                 cp \"$4\" \"$D/manifest.seen\"\n\
                 if [ \"$3\" = \"--text-manifest\" ]; then cp \"$D/vectors.json\" \"$6\"; exit 0; fi\n\
                 : > \"$6\"\n\
                 while IFS= read -r L; do\n\
                 [ -n \"$L\" ] || continue\n\
                 TAIL=${{L#?}}\n\
                 printf '%s' \"$(cat \"$D/prefix.txt\")\" >> \"$6\"\n\
                 printf '%s\\n' \"$TAIL\" >> \"$6\"\n\
                 done < \"$4\"\n\
                 exit 0\n",
                d = dir.display()
            ),
        );
        crate::embed::EmbedOpts { python_bin, script, text_file: None, vocab_file: None }
    }

    /// `describe.py`, stubbed the same way: it echoes each manifest path back
    /// with a fixed sentence, so the caller's path mapping is exercised rather
    /// than bypassed.
    fn describe_stub(dir: &Path, desc: &str) -> crate::describe::DescribeOpts {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("prefix.txt"),
            format!(
                "{{\"model\":\"{}\",\"revision\":\"{}\",\"prompt_version\":{},\"desc\":\"{desc}\",",
                crate::describe::MODEL_REPO,
                crate::describe::MODEL_REVISION,
                crate::describe::PROMPT_VERSION
            ),
        )
        .unwrap();
        let script = dir.join("describe.py");
        std::fs::write(&script, "# stand-in\n").unwrap();
        let python_bin = crate::write_stand_in(
            dir,
            "describe-stub",
            "@echo off\r\n\
             setlocal enabledelayedexpansion\r\n\
             echo call>>\"%~dp0calls.log\"\r\n\
             copy /y \"%~4\" \"%~dp0manifest.seen\" >nul\r\n\
             if exist \"%~6\" del \"%~6\"\r\n\
             for /f \"usebackq delims=\" %%L in (\"%~4\") do (\r\n\
             set \"L=%%L\"\r\n\
             type \"%~dp0prefix.txt\" >>\"%~6\"\r\n\
             echo !L:~1!>>\"%~6\"\r\n\
             )\r\n\
             exit /b 0\r\n",
            &format!(
                "D=\"{d}\"\n\
                 echo call >> \"$D/calls.log\"\n\
                 cp \"$4\" \"$D/manifest.seen\"\n\
                 : > \"$6\"\n\
                 while IFS= read -r L; do\n\
                 [ -n \"$L\" ] || continue\n\
                 TAIL=${{L#?}}\n\
                 printf '%s' \"$(cat \"$D/prefix.txt\")\" >> \"$6\"\n\
                 printf '%s\\n' \"$TAIL\" >> \"$6\"\n\
                 done < \"$4\"\n\
                 exit 0\n",
                d = dir.display()
            ),
        );
        crate::describe::DescribeOpts { python_bin, script }
    }

    /// Three tiny PNGs, distinct pixel by pixel so their frame digests differ.
    fn baked_corpus(dir: &Path, n: usize) -> Vec<PathBuf> {
        std::fs::create_dir_all(dir).unwrap();
        (0..n)
            .map(|i| {
                let p = dir.join(format!("look-{i}.png"));
                let mut img = image::RgbImage::new(48, 32);
                for (x, y, px) in img.enumerate_pixels_mut() {
                    *px = image::Rgb([
                        ((x * 5 + i as u32 * 40) % 256) as u8,
                        ((y * 7 + i as u32 * 11) % 256) as u8,
                        ((x + y + i as u32 * 3) % 256) as u8,
                    ]);
                }
                img.save(&p).unwrap();
                p
            })
            .collect()
    }

    /// A build's three sidecars, scratch included. The scratch directory is
    /// the test's OWN: the description cache lives in it, and a test that let
    /// it default to the store root would both collide with every other test
    /// (identical fixtures hash identically) and write the user's live store.
    fn staged(
        embed: crate::embed::EmbedOpts,
        describe: crate::describe::DescribeOpts,
        scratch: &Path,
    ) -> BuildSidecars {
        BuildSidecars { embed, describe, scratch: scratch.to_path_buf() }
    }

    fn calls_at(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }

    /// THE COST CONTRACT (S2, supervisor's ruling 2026-08-30): a build runs
    /// ONE process per model, not one per photograph.
    ///
    /// Until this batch the IMAGE half of the embedding was a sidecar call per
    /// record — 169 loads of a 1.50 GB checkpoint for the photographer's own
    /// library, 5,618 s measured (S1 report §3). The text half was already
    /// batched (S1-fix F-12) and `embed.py --manifest-jsonl` had always
    /// existed; nothing was wired to it.
    ///
    /// MUTATION THIS KILLS: move `embed_frames` (or `attach_descriptions`)
    /// back inside the per-photo loop and the counts below become 3.
    #[test]
    fn build_invokes_each_sidecar_once_per_build() {
        let root = crate::test_dir("style-stage-once");
        let photos = baked_corpus(&root.join("photos"), 3);
        assert_eq!(photos.len(), 3, "premise: three finished photos");
        let embed_dir = root.join("embed");
        let describe_dir = root.join("describe");
        let index = StyleIndex::build_looks_with(
            staged(
                image_text_stub(&embed_dir, 3),
                describe_stub(&describe_dir, "a stubbed grade sentence"),
                &root.join("scratch"),
            ),
            &root.join("photos"),
            EmbeddingSwitch::ON,
            DescribeSwitch::ON,
            &|_| {},
        )
        .expect("the staged look build succeeds against the stubs");

        assert_eq!(
            calls_at(&embed_dir),
            vec!["--manifest-jsonl".to_string(), "--text-manifest".to_string()],
            "three photos: ONE image call and ONE text call, in that order"
        );
        assert_eq!(calls_at(&describe_dir).len(), 1, "three photos, ONE describe call");
        assert_eq!(index.looks.len(), 3);
        assert!(index.looks.iter().all(|l| l.embed.len() == crate::embed::EMBED_DIM));
        assert!(index.looks.iter().all(|l| l.desc.as_deref() == Some("a stubbed grade sentence")));
        assert!(index.looks.iter().all(|l| l.desc_embed.is_some()));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The SWITCH is the only thing that starts the description pass — not the
    /// script being on disk, not the embedding being on.
    ///
    /// MUTATION THIS KILLS: drop the `!describe.on()` guard at the top of
    /// `attach_descriptions` (the stub is then invoked and the count is 1), or
    /// make `DescribeSwitch::resolve` default to ON.
    #[test]
    fn describe_never_runs_without_the_switch() {
        let root = crate::test_dir("style-describe-gate");
        baked_corpus(&root.join("photos"), 2);
        let embed_dir = root.join("embed");
        let describe_dir = root.join("describe");
        let index = StyleIndex::build_looks_with(
            staged(
                image_text_stub(&embed_dir, 2),
                describe_stub(&describe_dir, "never written"),
                &root.join("scratch"),
            ),
            &root.join("photos"),
            EmbeddingSwitch::ON,
            DescribeSwitch::OFF,
            &|_| {},
        )
        .expect("a build with the description pass off still succeeds");
        assert!(calls_at(&describe_dir).is_empty(), "the sidecar was never invoked");
        assert!(index.looks.iter().all(|l| l.desc.is_none()), "no record carries prose");
        // …and the vectors still landed: OFF removes the prose, not the build.
        assert!(index.looks.iter().all(|l| l.desc_embed.is_some()));
        // The resolver's own rule, on an explicit environment so no test has
        // to write the process's.
        let unset = |_: &str| None;
        assert!(!DescribeSwitch::resolve_with(None, false, unset).on());
        assert!(DescribeSwitch::resolve_with(None, true, unset).on());
        assert!(!DescribeSwitch::resolve_with(None, true, |_| Some("0".into())).on());
        assert!(DescribeSwitch::resolve_with(None, false, |_| Some("1".into())).on());
        assert!(DescribeSwitch::resolve_with(Some(true), false, |_| Some("0".into())).on());
        assert!(!DescribeSwitch::resolve_with(Some(false), true, |_| Some("1".into())).on());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// S2's half of the F-12 rule: when a record has PROSE, that is what the
    /// text tower embeds — the tag string only stands in when there is none.
    ///
    /// Asserted on the manifest the batch text door actually received, from a
    /// REAL staged build, so it covers the wiring as well as `desc_text`.
    ///
    /// MUTATION THIS KILLS: make `desc_text` prefer the tags (the manifest
    /// then carries the vocabulary phrases), or stop writing `desc` in
    /// `attach_descriptions` (same symptom, one stage earlier).
    #[test]
    fn desc_embed_prefers_prose_over_tags() {
        let root = crate::test_dir("style-prose-over-tags");
        baked_corpus(&root.join("photos"), 2);
        let embed_dir = root.join("embed");
        let described = StyleIndex::build_looks_with(
            staged(
                image_text_stub(&embed_dir, 2),
                describe_stub(&root.join("describe"), "a warm hazy grade"),
                &root.join("scratch"),
            ),
            &root.join("photos"),
            EmbeddingSwitch::ON,
            DescribeSwitch::ON,
            &|_| {},
        )
        .unwrap();
        // The LAST manifest the embed stub saw is the text one.
        let seen = stub_manifest(&embed_dir);
        assert_eq!(seen, vec!["a warm hazy grade".to_string(); 2], "the prose is what is embedded");
        assert!(described.looks.iter().all(|l| !l.tags.is_empty()), "premise: the tags exist too");

        // Same corpus, description pass off: now the TAG STRING stands in.
        let root2 = crate::test_dir("style-prose-over-tags-off");
        baked_corpus(&root2.join("photos"), 2);
        let embed_dir2 = root2.join("embed");
        let plain = StyleIndex::build_looks_with(
            staged(
                image_text_stub(&embed_dir2, 2),
                describe_stub(&root2.join("describe"), "never written"),
                &root2.join("scratch"),
            ),
            &root2.join("photos"),
            EmbeddingSwitch::ON,
            DescribeSwitch::OFF,
            &|_| {},
        )
        .unwrap();
        let seen2 = stub_manifest(&embed_dir2);
        assert!(!seen2.is_empty() && seen2.iter().all(|t| *t == plain.looks[0].tags.join(", ")),
                "with no prose the tag string is embedded: {seen2:?}");
        assert_ne!(seen, seen2, "the two builds embedded different text");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&root2);
    }

    /// The reference blocks carry the prose AFTER the tags, through the same
    /// bounded door the index used.
    ///
    /// The description is model output about the user's own photograph and it
    /// is going into a proposer prompt, so the block must not be the place
    /// that trusts it: a newline in a description would forge a line of the
    /// block, and `sanitize_desc` is what stops that on both surfaces.
    ///
    /// MUTATION THIS KILLS: append the prose BEFORE the tags, drop the ` — `
    /// join, or render `e.desc` directly instead of through the door.
    #[test]
    fn reference_block_carries_prose_after_tags() {
        let mut ex = plain_exemplar("shot");
        ex.tags = vec!["warm golden tones".into(), "deep blacks".into()];
        ex.desc = Some("a warm, hazy grade\nwith lifted shadows".into());
        let idx = StyleIndex {
            version: CURRENT_INDEX_VERSION, mean: vec![0.0; NDIM], std: vec![1.0; NDIM],
            exemplars: Vec::new(), source_dir: None, looks: Vec::new(), looks_dir: None,
            embed_provenance: None,
        };
        let block = idx
            .render_reference(&[&ex], crate::recipe::GradeStrength::new(0.5))
            .expect("a reference block renders");
        assert!(
            block.contains("look: warm golden tones, deep blacks — a warm, hazy grade with lifted shadows"),
            "tags first, then the prose, on ONE line: {block}"
        );
        assert!(!block.contains("grade\nwith"), "the newline must not survive into the block");
        // …and with no prose the suffix is exactly what S1 shipped.
        let mut tags_only = ex.clone();
        tags_only.desc = None;
        let plain = idx
            .render_reference(&[&tags_only], crate::recipe::GradeStrength::new(0.5))
            .unwrap();
        // The SUFFIX, not the whole block: the block's own prose carries an
        // em dash of its own ("— the look their edits tend toward"), so a bare
        // search for one would pass on any input.
        //
        // The suffix ENDS at the first run of two spaces, which is how
        // `render_reference` joins its trailing notes onto the exemplar lines
        // — and a run of two spaces cannot occur inside the suffix itself,
        // because `sanitize_desc` collapses every whitespace run to one space
        // before the description is allowed near the block.
        let suffix = |b: &str| {
            b.lines()
                .find(|l| l.contains("· look:"))
                .map(|l| {
                    l.split("· look:")
                        .nth(1)
                        .unwrap()
                        .split("  ")
                        .next()
                        .unwrap()
                        .trim()
                        .to_string()
                })
                .expect("the reference line carries a look suffix")
        };
        assert_eq!(suffix(&plain), "warm golden tones, deep blacks", "{plain}");
        assert_eq!(
            suffix(&block),
            "warm golden tones, deep blacks — a warm, hazy grade with lifted shadows",
            "{block}"
        );

        // The LOOK block, same rule.
        let look = LookExemplar {
            stem: "finished".into(), path: "f.jpg".into(), embed: unit_embed(),
            tags: vec!["vivid saturated colours".into()], vocab_scores: None,
            desc: Some("a punchy\tcool grade".into()), desc_embed: None,
        };
        let idx2 = StyleIndex { looks: vec![look], ..idx };
        let lb = idx2.render_look_reference(&[&idx2.looks[0]], false).unwrap();
        assert!(lb.contains("look: vivid saturated colours; a punchy cool grade"), "{lb}");
    }
}
