//! The LOOK STATEMENT call: the vision model is shown the neutral frame and
//! what each area of it measures now ([`super::look`]), and answers the
//! finished appearance of each area in the same numbers, plus one sentence
//! naming the look. The engine then moves the frame to those numbers and the
//! reverse fit finds the settings (`pipeline::statement`).
//!
//! The request text is the one the prototype trialled; the photographer's
//! typed direction and a one-line strength clause are added the way the
//! proposer carries them. The reference library's phrase block may always go
//! (fenced and bounded as the proposer sends it); the references' PICTURES and
//! their measured numbers go only when the photographer opted into sending
//! reference images.

use serde_json::{Value, json};

use super::look::{AreaLook, AreaName, AreaStatement};
use super::{AdvisorError, ApiFamily, BoundedUntrustedText, VisionFrame, VisionRequest};
use crate::config::Config;
use crate::recipe::{AdherenceTier, DirectionAdherence, GradeStrength, StrengthTier};

/// The bound on the look sentence carried into the rationale.
const LOOK_MAX_BYTES: usize = 512;

/// The bound on the photographer's direction inside the request (the judge's
/// own bound on the same text).
const DIRECTION_MAX_BYTES: usize = 512;

/// One reference photograph SHOWN to the model (the opt-in is on).
#[derive(Debug, Clone, Copy)]
pub struct ShownReference<'a> {
    /// Its number in the reference block (1-based, closest first).
    pub number: usize,
    /// Its measured areas, or `None` when they could not be measured (it then
    /// goes as picture and phrases without numbers).
    pub areas: Option<&'a [(AreaName, AreaLook)]>,
    /// Its picture, a JPEG at the advisor's edge.
    pub jpeg: &'a [u8],
}

/// Everything one statement request is made of.
#[derive(Debug, Clone, Copy)]
pub struct StatementAsk<'a> {
    /// The neutral frame, a JPEG at the advisor's edge: IMAGE 1.
    pub frame_jpeg: &'a [u8],
    /// The frame's areas and what each measures now, in the order they are
    /// named.
    pub areas: &'a [(AreaName, AreaLook)],
    /// The photographer's own typed direction, when the develop is guided by it.
    pub direction: Option<&'a str>,
    pub adherence: DirectionAdherence,
    pub strength: GradeStrength,
    /// The reference library's phrase block (`library::reference_block`), as is.
    pub reference_block: Option<&'a str>,
    /// How many references the phrase block numbers (the guides the reply may
    /// name are among these).
    pub reference_count: usize,
    /// The references shown as pictures, in order — empty unless the
    /// photographer opted into sending reference images.
    pub shown: &'a [ShownReference<'a>],
}

/// A parsed, validated statement.
#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    /// One sentence naming the finished look (bounded, untrusted model text).
    pub look: String,
    /// The reference numbers the model used as direct guides, each one a
    /// reference the request carried.
    pub guides: Vec<usize>,
    /// The stated numbers, one per area of the request, in its order.
    pub areas: Vec<(AreaName, AreaStatement)>,
}

fn one_decimal(v: &[f32]) -> String {
    let parts: Vec<String> = v.iter().map(|x| format!("{:.1}", x)).collect();
    format!("[{}]", parts.join(", "))
}

/// One area's numbers as the request states them (the prototype's line).
fn area_line(name: AreaName, a: &AreaLook, of: &str) -> String {
    format!(
        "\"{}\" ({:.0}% of {of}): L* at the 2/25/50/75/98% tonal anchors {}; mean (a*, b*) of its shadows {}, midtones {}, highlights {}",
        name.as_str(),
        a.share * 100.0,
        one_decimal(&a.l),
        one_decimal(&a.ab[0]),
        one_decimal(&a.ab[1]),
        one_decimal(&a.ab[2]),
    )
}

/// The one line the strength dial adds, in the proposer's tier words.
fn strength_line(strength: GradeStrength) -> String {
    let pct = strength.pct();
    match strength.tier() {
        StrengthTier::Restrained => {
            "STRENGTH: the photographer asks for a tasteful, restrained finished look — a committed look, not a maximal one.".to_string()
        }
        StrengthTier::Balanced => format!(
            "STRENGTH: the photographer set this develop's strength dial to {pct:.0}% of full (50% = this app's cautious baseline): state a CONFIDENT finished look."
        ),
        StrengthTier::Committed => format!(
            "STRENGTH: the photographer set this develop's strength dial to {pct:.0}% of full (50% = this app's cautious baseline), so a safe, mild result is the WRONG answer: state a BOLD, fully committed look — without crushing shadow detail or blowing highlights."
        ),
    }
}

/// The photographer's typed direction, as the proposer heads it, with the
/// adherence tier in the statement's own terms.
fn direction_line(direction: &str, adherence: DirectionAdherence) -> String {
    let d = BoundedUntrustedText::new(direction, DIRECTION_MAX_BYTES, &[]);
    let how = match adherence.tier() {
        AdherenceTier::Hint => {
            "Treat it as a PREFERENCE: honour it where it agrees with the photographer's references, prefer those where they conflict."
        }
        AdherenceTier::Direct => "It overrides your own taste and the references where they conflict.",
        AdherenceTier::Brief => {
            "It overrides your own taste and the references; every clause of it must show in the numbers you state."
        }
    };
    format!("USER DIRECTION (a specific request from the photographer — follow it closely): {d}  {how}")
}

/// The instruction text and the frames of one statement request, in order:
/// the neutral frame first, then each shown reference.
pub(crate) fn statement_request<'a>(cfg: &'a Config, ask: &StatementAsk<'_>) -> VisionRequest<'a> {
    let names: Vec<String> = ask.areas.iter().map(|(n, _)| format!("\"{}\"", n.as_str())).collect();
    let measured: Vec<String> = ask.areas.iter().map(|(n, a)| format!("- {}", area_line(*n, a, "the frame"))).collect();
    let mut text = String::from(
        "You are a master photo finisher. IMAGE 1 is a photograph developed NEUTRALLY from its raw file: \
flat and unfinished. Decide the finished look a skilled photographer would give it - confident and \
complete, not timid, true to the light of the scene.\n\n\
You do NOT choose sliders. You state what each AREA of the finished picture should LOOK LIKE, as numbers in \
CIELAB (L* 0 = black, 100 = white; a* negative = green, positive = magenta-red; b* negative = blue, positive = \
yellow). A solver will then find the adjustments that produce exactly these numbers, so state the appearance \
you really want to see: if a night sky should be deep blue, give it a clearly negative b*; if a sunset should \
glow, give its highlights the L* and the warm a*, b* of a glowing sky.\n\n\
The areas, and what each measures NOW in the neutral frame:\n",
    );
    text.push_str(&measured.join("\n"));
    text.push_str("\n\n");
    text.push_str(&strength_line(ask.strength));
    if let Some(d) = ask.direction.filter(|d| !d.trim().is_empty()) {
        text.push_str("\n\n");
        text.push_str(&direction_line(d, ask.adherence));
    }
    let mut frames = vec![VisionFrame { b64: b64(ask.frame_jpeg), detail: "high" }];
    if !ask.shown.is_empty() {
        let last = 1 + ask.shown.len();
        text.push_str(&format!(
            "\n\nTHIS PHOTOGRAPHER'S TASTE. IMAGES 2-{last} are their own FINISHED photographs, the nearest scenes in \
their library, each measured in the same areas and the same numbers (the REFERENCE LOOKS block at the end \
gives each one in words). They may or may not share this frame's kind of scene and light - look at each one \
and decide."
        ));
        for (i, r) in ask.shown.iter().enumerate() {
            text.push_str(&format!("\nFinished photograph {} (IMAGE {}):", r.number, i + 2));
            match r.areas {
                Some(areas) => {
                    for (n, a) in areas {
                        text.push_str(&format!("\n    {}", area_line(*n, a, "it")));
                    }
                }
                None => text.push_str(" (its areas could not be measured; judge it by the picture)"),
            }
            frames.push(VisionFrame { b64: b64(r.jpeg), detail: "high" });
        }
        text.push_str(
            "\nState the look THIS photographer would give the frame, not your own preference.\n\
- A finished photograph of the same kind of scene AND the same kind of light as the frame is a direct \
guide: stay close to how it treats each area - how deep the shadows sit and how far they are opened, \
how bright the highlights go, how much tonal range an area keeps, the hue and the strength of colour in \
its shadows, midtones and highlights.\n\
- A finished photograph of a different light (a sunset beside a midday or a night frame, a night beside a \
day) is NOT a guide for hue or for brightness. Take from it only the photographer's hand: how strong \
their colour is, how much contrast they keep, how they open shadows.\n\
- Adapt to this frame's own light; do not copy any one photograph.",
        );
    }
    let guides = if ask.reference_count > 0 {
        "the numbers of the finished photographs you used as direct guides, or an empty list"
    } else {
        "an empty list"
    };
    text.push_str(&format!(
        "\n\nAnswer ONLY one JSON object, no prose around it:\n\
{{\"look\": \"<one sentence naming the finished look>\", \"guides\": [{guides}], \"areas\": {{<for each of {}>: \
{{\"L\": [five numbers, the same anchors], \"ab_shadows\": [a, b], \"ab_mids\": [a, b], \"ab_highlights\": [a, b]}}}}}}\n\
Rules: each L list is non-decreasing; an area keeps a believable tonal range (do not flatten it); \
highlights stay below 98 unless they are light sources.",
        names.join(", ")
    ));
    if let Some(block) = ask.reference_block {
        let block = BoundedUntrustedText::new(block, super::REFERENCE_BUDGET_BYTES, &[]);
        text.push_str("  ");
        text.push_str(super::FENCE_STYLE_REFERENCE);
        text.push_str(&block.to_string());
    }
    VisionRequest { model: &cfg.openai_model, instruction: text, frames, schema: Some(("look_statement", statement_schema(ask.areas))) }
}

fn b64(bytes: &[u8]) -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
}

/// The strict reply schema: `{look, guides, areas: {<each named area>: {L,
/// ab_shadows, ab_mids, ab_highlights}}}`. Strict mode cannot pin an array's
/// length, so the parser checks it.
pub(crate) fn statement_schema(areas: &[(AreaName, AreaLook)]) -> Value {
    let numbers = json!({"type": "array", "items": {"type": "number"}});
    let area = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["L", "ab_shadows", "ab_mids", "ab_highlights"],
        "properties": {"L": numbers, "ab_shadows": numbers, "ab_mids": numbers, "ab_highlights": numbers}
    });
    let mut props = serde_json::Map::new();
    for (n, _) in areas {
        props.insert(n.as_str().to_string(), area.clone());
    }
    let required: Vec<&str> = areas.iter().map(|(n, _)| n.as_str()).collect();
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["look", "guides", "areas"],
        "properties": {
            "look": {"type": "string"},
            "guides": {"type": "array", "items": {"type": "integer"}},
            "areas": {"type": "object", "additionalProperties": false, "required": required, "properties": props}
        }
    })
}

fn refuse(why: impl Into<String>) -> AdvisorError {
    AdvisorError::ModelFailure(format!("the look statement {}", why.into()))
}

fn numbers<const N: usize>(v: &Value, area: &str, key: &str) -> Result<[f64; N], AdvisorError> {
    let arr = v.get(key).and_then(Value::as_array).ok_or_else(|| refuse(format!("has no {key} list for \"{area}\"")))?;
    if arr.len() != N {
        return Err(refuse(format!("gives {} numbers for \"{area}\" {key}, not {N}", arr.len())));
    }
    let mut out = [0.0f64; N];
    for (o, x) in out.iter_mut().zip(arr) {
        *o = x
            .as_f64()
            .filter(|f| f.is_finite())
            .ok_or_else(|| refuse(format!("has a value in \"{area}\" {key} that is not a finite number")))?;
    }
    Ok(out)
}

/// Parse and validate a statement reply against the areas that were asked
/// for and the reference count the request carried.
///
/// A code fence is stripped; every named area must be present and no other;
/// five finite L* each in 0..=100, made non-decreasing; a*, b* finite and
/// clamped to ±100; `guides` kept only where it names a reference the request
/// carried. Anything else is a typed error, never a default.
pub fn parse_statement(
    text: &str,
    areas: &[AreaName],
    reference_count: usize,
    secrets: &[&str],
) -> Result<Statement, AdvisorError> {
    let v: Value = serde_json::from_str(super::strip_code_fence(text))?;
    let look = v.get("look").and_then(Value::as_str).ok_or_else(|| refuse("has no look sentence"))?;
    let look = BoundedUntrustedText::new(look.trim(), LOOK_MAX_BYTES, secrets).into_string();
    let mut guides: Vec<usize> = Vec::new();
    match v.get("guides") {
        None | Some(Value::Null) => {}
        Some(Value::Array(g)) => {
            for n in g.iter().filter_map(Value::as_u64).filter_map(|n| usize::try_from(n).ok()) {
                if (1..=reference_count).contains(&n) && !guides.contains(&n) {
                    guides.push(n);
                }
            }
        }
        Some(_) => return Err(refuse("has a guides value that is not a list")),
    }
    let stated = v.get("areas").and_then(Value::as_object).ok_or_else(|| refuse("has no areas object"))?;
    if let Some(extra) = stated.keys().find(|k| !areas.iter().any(|a| a.as_str() == k.as_str())) {
        return Err(refuse(format!("names an area that was not asked for: {:?}", BoundedUntrustedText::new(extra, 64, secrets).as_str())));
    }
    let mut out = Vec::with_capacity(areas.len());
    for &name in areas {
        let a = stated.get(name.as_str()).ok_or_else(|| refuse(format!("has no numbers for the area \"{}\"", name.as_str())))?;
        let l = numbers::<5>(a, name.as_str(), "L")?;
        if let Some(bad) = l.iter().find(|x| !(0.0..=100.0).contains(*x)) {
            return Err(refuse(format!("states L* {bad} for \"{}\", outside 0..100", name.as_str())));
        }
        let mut run = f64::NEG_INFINITY;
        let l = l.map(|x| {
            run = run.max(x);
            run as f32
        });
        let mut ab = [[0.0f32; 2]; 3];
        for (k, key) in ["ab_shadows", "ab_mids", "ab_highlights"].iter().enumerate() {
            ab[k] = numbers::<2>(a, name.as_str(), key)?.map(|x| x.clamp(-100.0, 100.0) as f32);
        }
        out.push((name, AreaStatement { l, ab }));
    }
    Ok(Statement { look, guides, areas: out })
}

/// ONE statement call on the image role: the request built by
/// [`statement_request`], sent by `post_vision`, the reply validated by
/// [`parse_statement`]. Needs the image-role key.
pub fn ask_statement(cfg: &Config, ask: &StatementAsk<'_>) -> Result<Statement, AdvisorError> {
    let key = cfg.openai_api_key.as_ref().ok_or_else(|| AdvisorError::Missing("OPENAI_API_KEY".into()))?;
    let req = statement_request(cfg, ask);
    let text = super::post_vision(
        &cfg.openai_base_url,
        key,
        &req,
        super::PROPOSE_TIMEOUT_SECS,
        cfg.image_effort.as_deref(),
        ApiFamily::parse(&cfg.image_api_family),
        "could not locate structured output in the look statement response",
    )?;
    let names: Vec<AreaName> = ask.areas.iter().map(|(n, _)| *n).collect();
    parse_statement(&text, &names, ask.reference_count, &[key])
}

#[cfg(test)]
#[path = "statement_tests.rs"]
mod tests;
