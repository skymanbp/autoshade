//! The AI advisor layer — the unified provider framework (统一 API 框架).
//!
//! One [`Advisor`] trait, two roles (see `docs/ARCHITECTURE.md` §3):
//!   * **propose** — a vision model looks at the preview and emits an
//!     [`EditRecipe`] (GPT in production; [`HeuristicProposer`] as a no-key
//!     baseline).
//!   * **verify** — Claude, data-only, acceptance-checks the recipe before it
//!     is applied (the "收货验证" role), via the `claude` CLI over OAuth.
//!
//! M1 is synchronous: a single image flows propose → verify sequentially, so we
//! avoid the cost/complexity of an async runtime + `async_trait`. Concurrency
//! (batch, or parallel GPT/Claude) can move this to async later if needed.

mod claude;
mod heuristic;
mod openai;
mod openai_verify;

pub use claude::ClaudeProvider;
pub use heuristic::HeuristicProposer;
pub use openai::{describe_style, OpenAiProvider};
pub use openai_verify::OpenAiVerifier;

use crate::decode::{Histogram, Meta};
use crate::recipe::EditRecipe;

/// JPEG preview bytes handed to a vision advisor.
pub struct Preview {
    pub jpeg: Vec<u8>,
}

/// The verifier's decision on a proposed recipe.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Accept,
    Revise,
    Reject,
}

/// Acceptance-verification outcome (the analyst/verifier role).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Verdict {
    pub decision: Decision,
    #[serde(default)]
    pub reasons: Vec<String>,
    /// When `Revise`/`Reject`, a short instruction for the next propose round.
    #[serde(default)]
    pub revised_hint: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum AdvisorError {
    #[error("missing config: {0}")]
    Missing(String),
    #[error("subprocess io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{bin} exited {code:?}: {stderr}")]
    CliFailed { bin: String, code: Option<i32>, stderr: String },
    #[error("claude reported is_error: {0}")]
    ClaudeError(String),
    #[error("claude CLI envelope was not JSON ({source}); first bytes: {head:?}")]
    BadEnvelope { source: serde_json::Error, head: String },
    #[error("claude's verdict was not valid JSON ({source}); got: {got:?}")]
    BadVerdict { source: serde_json::Error, got: String },
    #[error("http {status}: {body}")]
    Http { status: u16, body: String },
    #[error("http transport: {0}")]
    Transport(String),
    #[error("advisor '{0}' does not serve this role")]
    Unsupported(&'static str),
}

/// One AI advisor. A provider implements the role(s) it serves; the unserved
/// role returns [`AdvisorError::Unsupported`] rather than panicking, so a single
/// registry can hold mixed providers.
pub trait Advisor {
    fn name(&self) -> &'static str;

    /// Image role: preview + features → recipe. `hint` carries the verifier's
    /// revision instruction on a second round (ignored by providers that can't
    /// use it).
    fn propose(
        &self,
        _img: &Preview,
        _meta: &Meta,
        _hist: &Histogram,
        _reference: Option<&str>,
        _guidance: Option<&str>,
        _hint: Option<&str>,
    ) -> Result<EditRecipe, AdvisorError> {
        Err(AdvisorError::Unsupported(self.name()))
    }

    /// Analyst role: data-only acceptance check of a proposed recipe.
    fn verify(
        &self,
        _recipe: &EditRecipe,
        _meta: &Meta,
        _hist: &Histogram,
    ) -> Result<Verdict, AdvisorError> {
        Err(AdvisorError::Unsupported(self.name()))
    }
}

/// Per-call HTTP deadline classes for the analysis endpoints. Recalibrated
/// 2026-08-03: the original 60–120 s budgets predate reasoning-class vision
/// models — a real /responses propose on direct api.openai.com outran 120 s
/// and the client killed a HEALTHY request mid-generation (the analyze path
/// then fell back to "Heuristic baseline (AI vision unavailable: … timed out
/// reading response)"). Same failure class as the images/edits 300→600 s
/// recalibration in generative.rs. `AUTOSHOP_HTTP_TIMEOUT_SECS` still
/// overrides every one of these (see [`post_with_timeout`]).
pub(crate) const PROPOSE_TIMEOUT_SECS: u64 = 360; // high-detail image + strict schema — slowest text call
pub(crate) const STYLE_TIMEOUT_SECS: u64 = 240; // two low-detail images, short prose out
pub(crate) const VERIFY_TIMEOUT_SECS: u64 = 180; // text-only chat, temperature 0

/// Wrap a ureq transport failure as [`AdvisorError::Transport`], appending
/// the actionable deadline note when it is a read timeout: a mid-request kill
/// prints as "Network Error: timed out reading response", which reads like a
/// dead network but usually means the model outran the per-call deadline
/// (the generative path learned this first — see generative.rs).
pub(crate) fn transport_error(t: &ureq::Transport, default_secs: u64) -> AdvisorError {
    let mut msg = t.to_string();
    if msg.contains("timed out") {
        msg.push_str(&format!(
            " (hit the HTTP deadline, default {default_secs}s for this call — \
             reasoning-class models can legitimately run longer; raise \
             AUTOSHOP_HTTP_TIMEOUT_SECS to extend every AI call's deadline)"
        ));
    }
    AdvisorError::Transport(msg)
}

/// POST builder with a hard overall deadline. The default `ureq::post` agent
/// has NO read/overall timeout: a server that accepts the TCP connection and
/// then never responds (a dead local bridge, a stalled proxy) blocks the
/// worker thread FOREVER — and every GUI action gates on that worker's `busy`
/// flag, so the whole app soft-locks. Per-call budgets reflect each
/// endpoint's real latency class (the consts above); `AUTOSHOP_HTTP_TIMEOUT_SECS`
/// overrides all of them for outlier deployments.
pub(crate) fn post_with_timeout(url: &str, overall: std::time::Duration) -> ureq::Request {
    let overall = std::env::var("AUTOSHOP_HTTP_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(overall);
    ureq::builder()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout(overall)
        .build()
        .post(url)
}

/// POST builder with an INACTIVITY (stall) deadline instead of an overall one —
/// for STREAMING endpoints whose healthy service time is unbounded but whose
/// liveness is observable (SSE events keep arriving while the server works).
/// A fixed overall deadline is the wrong tool there: it kills healthy long
/// generations exactly as reliably as dead connections (the images/edits
/// 300→600 s history). Here every socket read — waiting for the response
/// headers or for the next stream chunk — must complete within `stall`, while
/// a stream that keeps sending can run indefinitely. Verified against the
/// ureq 2.12.1 sources: with no overall deadline set, the header wait uses
/// `timeout_read` (stream.rs:436) and body reads re-arm it (response.rs:364)
/// until the connection returns to the pool. `AUTOSHOP_HTTP_TIMEOUT_SECS`
/// overrides this too — one knob for every AI deadline; on a streaming call it
/// bounds SILENCE, not total duration.
pub(crate) fn post_with_stall_timeout(url: &str, stall: std::time::Duration) -> ureq::Request {
    let stall = std::env::var("AUTOSHOP_HTTP_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(stall);
    ureq::builder()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(stall)
        .timeout_write(stall)
        .build()
        .post(url)
}

/// [`transport_error`]'s streaming sibling. Crucially it reports the MEASURED
/// elapsed time: ureq surfaces both a connect-phase kill (≈10 s) and a real
/// read stall with the same "timed out reading response" text, and a real
/// user report showed the old fixed wording blaming a "600 s stall" for a
/// failure that took a dozen seconds. If the call died well before the stall
/// budget it was a connection problem, and the message must say so.
pub(crate) fn stall_transport_error(
    t: &ureq::Transport,
    stall_secs: u64,
    elapsed_secs: u64,
) -> AdvisorError {
    let mut msg = t.to_string();
    if msg.contains("timed out") {
        if elapsed_secs + 30 < stall_secs {
            msg.push_str(&format!(
                " (failed after {elapsed_secs}s — well before the {stall_secs}s stall budget, so \
                 this is a connection/handshake/proxy failure, not a slow model; it was already \
                 retried once)"
            ));
        } else {
            msg.push_str(&format!(
                " (no stream activity for ~{elapsed_secs}s — the server or a proxy stopped \
                 sending; healthy calls stream events and are not time-capped. Raise \
                 AUTOSHOP_HTTP_TIMEOUT_SECS if a proxy buffers server-sent events)"
            ));
        }
    }
    AdvisorError::Transport(msg)
}

/// A transport failure this fast is a connection blip (TLS stall, dropped
/// socket, flaky proxy), not model time — a real analyze died at ~10 s and
/// succeeded end-to-end on the immediate rerun. One retry absorbs those.
pub(crate) const TRANSPORT_RETRY_UNDER_SECS: u64 = 30;

/// SSE framing core, shared by every streaming AI consumer (the text calls
/// below and generative.rs's image stream). Follows the SSE contract: an
/// event's payload may span SEVERAL `data:` lines joined with `\n` and ends at
/// a blank line (or EOF for an unterminated final event); comment/`event:`/
/// `id:` lines and the `[DONE]` sentinel carry no payload; a payload that
/// isn't JSON is skipped at its event boundary. `cap` bounds the total bytes
/// read (and therefore the single-line String growth) against a broken or
/// hostile endless stream. Each complete JSON payload is handed to `on_json`;
/// `Break` stops draining early.
pub(crate) fn for_each_sse_json(
    r: impl std::io::Read,
    cap: u64,
    mut on_json: impl FnMut(serde_json::Value) -> std::ops::ControlFlow<()>,
) -> std::io::Result<()> {
    use std::io::BufRead;
    let mut lines = std::io::BufReader::new(r.take(cap)).lines();
    let mut event_data = String::new();
    loop {
        let line = match lines.next() {
            Some(l) => Some(l?),
            None => None,
        };
        let flush = match &line {
            None => true,                    // EOF flushes an unterminated event
            Some(l) if l.is_empty() => true, // blank line = event boundary
            Some(l) => {
                if let Some(data) = l.strip_prefix("data:") {
                    let data = data.trim();
                    if data != "[DONE]" {
                        if !event_data.is_empty() {
                            event_data.push('\n');
                        }
                        event_data.push_str(data);
                    }
                }
                false // event:/id:/comment lines never end the event
            }
        };
        if flush && !event_data.is_empty() {
            let payload = std::mem::take(&mut event_data);
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&payload)
                && let std::ops::ControlFlow::Break(()) = on_json(v)
            {
                return Ok(());
            }
        }
        if line.is_none() {
            return Ok(());
        }
    }
}

/// Which OpenAI-compatible endpoint family a streaming call talks to — the
/// SSE assembly differs: the Responses API's terminal `response.completed`
/// event carries the full blocking-shape object, while Chat Completions
/// streams token deltas the client must reassemble into the blocking shape.
#[derive(Clone, Copy)]
pub(crate) enum SseFamily {
    Responses,
    Chat,
}

/// Streaming stall floor. Reasoning-class models can be SILENT on the wire
/// while they think: /responses emits `response.created` immediately but no
/// further events until reasoning ends — unless a reasoning-summary stream is
/// granted (requested below, but droppable in negotiation) — and chat models
/// are quiet before their first token. The silence budget must cover the
/// longest healthy quiet phase, not just network jitter, so per-call budgets
/// below this floor bound only the blocking fallback (the 600 s number is the
/// images/edits stall precedent). `AUTOSHOP_HTTP_TIMEOUT_SECS` overrides.
pub(crate) const STREAM_STALL_FLOOR_SECS: u64 = 600;

/// Does this HTTP error body blame the named request parameter? Structured
/// `error.param` wins when present (exact, or a dotted child like
/// `reasoning.summary`); when absent — or JSON null — only a QUOTED mention
/// of the name counts: a bare substring match would let a proxy's
/// "upstream error" blame `stream`.
pub(crate) fn error_blames_param(body: &str, name: &str) -> bool {
    let param = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error")?.get("param")?.as_str().map(str::to_owned));
    match param.as_deref() {
        Some(p) => p == name || p.starts_with(&format!("{name}.")),
        None => body.contains(&format!("'{name}'")) || body.contains(&format!("\"{name}\"")),
    }
}

/// POST a JSON body to an OpenAI-compatible AI endpoint, STREAMING-FIRST.
///
/// `stream: true` is injected so a healthy long call keeps proving liveness
/// through SSE events and runs under an INACTIVITY deadline (floored at
/// [`STREAM_STALL_FLOOR_SECS`]) — the budget bounds SILENCE, not total
/// duration. This is the images/edits lesson (generative.rs) applied to the
/// text calls: reasoning-class models outran every fixed overall deadline we
/// calibrated (120→360 s on /responses, then 360 s again on a real propose),
/// and the next escalation would just be a slower copy of the same bug. On
/// the Responses family a `reasoning: {summary: "auto"}` stream is also
/// requested (when the caller didn't set `reasoning` itself) so the model
/// keeps emitting summary deltas WHILE it reasons — liveness through the
/// otherwise-silent phase.
///
/// Negotiation (each flag drops at most once, on a 400-class status whose
/// error actually blames that parameter — see [`error_blames_param`]):
/// `reasoning` (models without summaries) → retry streaming without it;
/// `stream` (thin OpenAI-compatible bridges) → retry ONCE as a blocking call
/// under `budget_secs` as an OVERALL deadline. A server that accepts `stream`
/// but answers plain JSON is handled by Content-Type dispatch. Returns the
/// endpoint's BLOCKING response shape either way.
pub(crate) fn post_ai_json(
    url: &str,
    key: &str,
    body: serde_json::Value,
    budget_secs: u64,
    family: SseFamily,
) -> Result<serde_json::Value, AdvisorError> {
    let mut use_stream = true;
    let mut use_summary =
        matches!(family, SseFamily::Responses) && body.get("reasoning").is_none();
    let mut transport_retried = false;
    loop {
        // Rebuild from the caller's body each attempt — dropping a negotiated
        // flag must not leave the other attempt's keys behind.
        let mut attempt = body.clone();
        if use_stream {
            attempt["stream"] = serde_json::Value::Bool(true);
            if use_summary {
                attempt["reasoning"] = serde_json::json!({"summary": "auto"});
            }
        }
        let req = if use_stream {
            post_with_stall_timeout(
                url,
                std::time::Duration::from_secs(budget_secs.max(STREAM_STALL_FLOOR_SECS)),
            )
        } else {
            post_with_timeout(url, std::time::Duration::from_secs(budget_secs))
        };
        let started = std::time::Instant::now();
        let resp = req
            .set("Authorization", &format!("Bearer {key}"))
            .set("Content-Type", "application/json")
            .send_json(&attempt);
        match resp {
            Ok(r) => {
                // Dispatch on what actually came back, not on what was asked.
                if r.content_type().eq_ignore_ascii_case("text/event-stream") {
                    let res = assemble_sse(r.into_reader(), family);
                    // A mid-stream READ failure this early is the same
                    // connection blip the pre-response retry absorbs — the
                    // stream died, not the model. Genuine model failures
                    // (error / response.failed events) are NOT retried:
                    // re-posting those would re-bill a real failure.
                    if let Err(AdvisorError::Transport(msg)) = &res
                        && msg.starts_with("read AI stream:")
                        && !transport_retried
                        && started.elapsed().as_secs() < TRANSPORT_RETRY_UNDER_SECS
                    {
                        transport_retried = true;
                        eprintln!("  note: {msg} — retrying once (connection blip, not model time)");
                        continue;
                    }
                    return res;
                }
                return r.into_json().map_err(|e| AdvisorError::Transport(e.to_string()));
            }
            Err(ureq::Error::Status(code, r)) => {
                let b = r.into_string().unwrap_or_default();
                // Only capability-shaped statuses negotiate (bad request /
                // not found / unprocessable). 401/403/429 etc. are NOT — an
                // auth or quota body that happens to mention a parameter must
                // not trigger a re-post.
                let negotiable = matches!(code, 400 | 404 | 422);
                if negotiable && use_stream && use_summary && error_blames_param(&b, "reasoning") {
                    eprintln!(
                        "  note: endpoint rejected the reasoning-summary stream — retrying \
                         without it (silent reasoning phases then count against the stall budget)"
                    );
                    use_summary = false;
                    continue;
                }
                if negotiable && use_stream && error_blames_param(&b, "stream") {
                    eprintln!(
                        "  note: endpoint rejected streaming — retrying as one blocking call \
                         ({budget_secs}s deadline)"
                    );
                    use_stream = false;
                    continue;
                }
                return Err(AdvisorError::Http { status: code, body: b });
            }
            Err(ureq::Error::Transport(t)) => {
                let elapsed = started.elapsed().as_secs();
                // A fast transport failure is a connection blip, not model
                // time — retry once before surfacing (a real analyze died at
                // ~10 s on a TLS/connect stall and succeeded on rerun).
                if !transport_retried && elapsed < TRANSPORT_RETRY_UNDER_SECS {
                    transport_retried = true;
                    eprintln!(
                        "  note: transport failed after {elapsed}s ({t}) — retrying once \
                         (a fast failure is a connection blip, not model time)"
                    );
                    continue;
                }
                return Err(if use_stream {
                    stall_transport_error(&t, budget_secs.max(STREAM_STALL_FLOOR_SECS), elapsed)
                } else {
                    transport_error(&t, budget_secs)
                });
            }
        }
    }
}

/// Reassemble a streamed AI response into the endpoint's BLOCKING shape, so
/// every caller's existing parsing stays untouched. Fails loudly on error /
/// failed / incomplete events and on a stream that ends without a result.
fn assemble_sse(
    r: impl std::io::Read,
    family: SseFamily,
) -> Result<serde_json::Value, AdvisorError> {
    use std::ops::ControlFlow::{Break, Continue};
    // A strict-schema recipe or verdict is a few KB; token deltas at most
    // double-count the final text. 64 MiB is far above any legitimate payload
    // and far below a runaway.
    const CAP: u64 = 64 * 1024 * 1024;
    match family {
        SseFamily::Responses => {
            let mut out: Option<serde_json::Value> = None;
            let mut failure: Option<String> = None;
            for_each_sse_json(r, CAP, |v| {
                if v.get("error").is_some()
                    || v.get("type").and_then(serde_json::Value::as_str) == Some("error")
                {
                    failure = Some(v.to_string());
                    return Break(());
                }
                match v.get("type").and_then(serde_json::Value::as_str).unwrap_or("") {
                    "response.completed" => {
                        out = v.get("response").cloned();
                        Break(())
                    }
                    ty @ ("response.failed" | "response.incomplete") => {
                        failure = Some(format!("{ty}: {v}"));
                        Break(())
                    }
                    _ => Continue(()),
                }
            })
            .map_err(|e| AdvisorError::Transport(format!("read AI stream: {e}")))?;
            if let Some(f) = failure {
                return Err(AdvisorError::Transport(format!("AI stream error: {f}")));
            }
            out.ok_or_else(|| {
                AdvisorError::Transport("AI stream ended without response.completed".into())
            })
        }
        SseFamily::Chat => {
            let mut text = String::new();
            let mut failure: Option<String> = None;
            for_each_sse_json(r, CAP, |v| {
                if v.get("error").is_some() {
                    failure = Some(v.to_string());
                    return Break(());
                }
                // Select the choice whose "index" is 0 (missing index = 0):
                // we never request n>1, but a bridge chunk may order or thin
                // its choices array arbitrarily — array position is not
                // choice identity.
                if let Some(delta) = v
                    .get("choices")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|arr| {
                        arr.iter().find(|c| {
                            c.get("index").and_then(serde_json::Value::as_u64).unwrap_or(0) == 0
                        })
                    })
                    .and_then(|c| c.get("delta"))
                    .and_then(|d| d.get("content"))
                    .and_then(serde_json::Value::as_str)
                {
                    text.push_str(delta);
                }
                Continue(())
            })
            .map_err(|e| AdvisorError::Transport(format!("read AI stream: {e}")))?;
            if let Some(f) = failure {
                return Err(AdvisorError::Transport(format!("AI stream error: {f}")));
            }
            if text.is_empty() {
                return Err(AdvisorError::Transport(
                    "chat stream ended without any content deltas".into(),
                ));
            }
            Ok(serde_json::json!({"choices": [{"message": {"content": text}}]}))
        }
    }
}

/// Compact, prompt-friendly histogram summary (the full 4×256 bins are too
/// large and noisy to put in a prompt). Reports clipping, mean luma, and a
/// 16-bucket luma distribution as percentages.
pub fn hist_summary(h: &Histogram) -> String {
    let total: u64 = h.luma.iter().map(|&v| v as u64).sum::<u64>().max(1);
    let weighted: u64 = h
        .luma
        .iter()
        .enumerate()
        .map(|(i, &v)| i as u64 * v as u64)
        .sum();
    let mean = weighted as f32 / total as f32;

    // 256 -> 16 buckets, each as % of pixels.
    let mut buckets = [0u64; 16];
    for (i, &v) in h.luma.iter().enumerate() {
        buckets[i / 16] += v as u64;
    }
    let dist: Vec<String> = buckets
        .iter()
        .map(|&b| format!("{:.0}", 100.0 * b as f32 / total as f32))
        .collect();

    format!(
        "mean_luma={mean:.0}/255, clip_black={:.2}%, clip_white={:.2}%, luma_16buckets_pct=[{}]",
        h.clip_black_pct,
        h.clip_white_pct,
        dist.join(","),
    )
}

/// Build the data-only verify prompt (shared by the OAuth `claude` verifier and
/// the OpenAI-compatible API verifier). The verifier never sees the image.
pub(crate) fn build_verify_prompt(
    recipe: &EditRecipe,
    meta: &Meta,
    hist: &Histogram,
) -> Result<String, AdvisorError> {
    let recipe_json = serde_json::to_string_pretty(recipe)?;
    let meta_json = serde_json::to_string(meta)?;
    Ok(format!(
        "You are a photo-edit QA verifier. You do NOT see the image — judge ONLY from the data below.\n\
Decide whether this proposed RAW develop recipe is both SAFE and COMMITTED enough to apply. A \
finished photograph is the goal, NOT timidity. Check, concretely:\n\
- every slider is within its documented range (exposure_ev -5..5; most sliders -100..100; sharpening 0..150; confidence 0..1);\n\
- adjustments are consistent with the metadata + histogram:\n\
  * do NOT push exposure/whites further INTO already-clipping highlights, and do NOT crush detail already sitting at the floor — but a few percent of intentional highlight/shadow clipping is normal and fine for a finished look;\n\
  * large, decisive moves are GOOD when the histogram supports them (a flat, low-contrast histogram wants real contrast or an S-curve; a muddy image wants a committed black point). Do NOT penalise a move just for being large;\n\
- CONVERSELY, REVISE a recipe that is too FLAT or TIMID for a finished result: near-zero contrast with no tonal anchor (no S-curve, empty tone_curve, ~0 contrast), or every slider hugging 0 while the histogram clearly has tonal room. Tell it to commit — add contrast / an S-curve, set the white and black points, shape the subject with a dodge/burn mask;\n\
- SYMMETRICALLY, REVISE a recipe that is OVER-COOKED: contrast applied BOTH as a high Contrast slider AND a strong tone_curve S (double contrast — pick one), blacks slammed so negative they crush detail the histogram shows is present, whites blown past the data, or vibrance+saturation+clarity piled together into a cartoonish look. A finished grade is committed but RESTRAINED, not maximal;\n\
- the rationale matches the numbers and confidence is adequate to auto-apply.\n\n\
METADATA: {meta_json}\n\
HISTOGRAM: {hist}\n\
PROPOSED RECIPE:\n{recipe_json}\n\n\
Output ONLY the JSON object: no reasoning, no preamble, no markdown fence. Your entire reply must start with '{{' and end with '}}'. Shape:\n\
{{\"decision\":\"accept\"|\"revise\"|\"reject\",\"reasons\":[\"short reason\", ...],\"revised_hint\":\"a short instruction for the next attempt if revise/reject, else null\"}}",
        meta_json = meta_json,
        hist = hist_summary(hist),
        recipe_json = recipe_json,
    ))
}

/// Strip a leading/trailing markdown code fence if the model wrapped its JSON,
/// then return the inner text. Idempotent for already-bare JSON.
pub(crate) fn strip_code_fence(s: &str) -> &str {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```") {
        // Drop an optional language tag on the first line, and the trailing ```.
        let rest = rest.split_once('\n').map(|x| x.1).unwrap_or(rest);
        rest.trim().strip_suffix("```").unwrap_or(rest).trim()
    } else {
        t
    }
}

/// Return every balanced top-level JSON **object** (`{...}`) in `s`, in order.
///
/// LLMs intermittently wrap their JSON in prose or reasoning that itself
/// contains `[...]` ranges and `{...}` examples, so picking "the first bracket"
/// is wrong. The caller tries these candidates (typically last-first) and keeps
/// the one that deserialises to the target type. String contents and escapes
/// are respected so braces inside strings don't break the depth count.
pub(crate) fn balanced_objects(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let (mut depth, mut start, mut in_str, mut esc) = (0i32, None, false, false);
    for (i, &b) in bytes.iter().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
        } else if b == b'"' {
            in_str = true;
        } else if b == b'{' {
            if depth == 0 {
                start = Some(i);
            }
            depth += 1;
        } else if b == b'}' && depth > 0 {
            depth -= 1;
            if depth == 0
                && let Some(st) = start.take() {
                    out.push(&s[st..=i]);
                }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_objects_finds_real_json_amid_prose() {
        // bare object
        assert_eq!(
            balanced_objects(r#"{"decision":"accept"}"#),
            vec![r#"{"decision":"accept"}"#]
        );
        // prose with a [-5,5] range then the real object — must NOT grab the array
        let chatty = "Range checks: exposure ∈ [-5, 5] ✓\nHere is the verdict:\n```json\n{\"decision\":\"revise\"}\n```";
        assert_eq!(balanced_objects(chatty), vec![r#"{"decision":"revise"}"#]);
        // braces inside a string must not end the object early
        let tricky = r#"prefix {"reasons":["has } brace","ok"]} suffix"#;
        assert_eq!(balanced_objects(tricky), vec![r#"{"reasons":["has } brace","ok"]}"#]);
        // multiple objects: caller picks the last that parses
        let two = r#"example {"a":1} then answer {"decision":"accept"}"#;
        assert_eq!(balanced_objects(two), vec![r#"{"a":1}"#, r#"{"decision":"accept"}"#]);
        assert!(balanced_objects("no json here").is_empty());
    }

    #[test]
    fn sse_chat_stream_reassembles_the_blocking_shape() {
        // Includes a chunk whose ARRAY position 0 is a different choice index —
        // choice identity is the "index" field, not array position — and a
        // usage-style chunk with an empty choices array.
        let body = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":1,\"delta\":{\"content\":\"WRONG\"}},\
{\"index\":0,\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"}}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"total_tokens\":5}}\n\n",
            "data: [DONE]\n\n",
        );
        let v = assemble_sse(body.as_bytes(), SseFamily::Chat).unwrap();
        assert_eq!(v["choices"][0]["message"]["content"], "Hello");
    }

    #[test]
    fn error_blames_param_prefers_structured_and_rejects_bare_substrings() {
        // Structured param wins.
        assert!(error_blames_param(r#"{"error":{"param":"stream","message":"x"}}"#, "stream"));
        // A structured param naming ANOTHER knob must not blame this one even
        // if the message mentions it.
        assert!(!error_blames_param(
            r#"{"error":{"param":"size","message":"not valid for streaming requests"}}"#,
            "stream"
        ));
        // param null → only a QUOTED mention counts…
        assert!(error_blames_param(
            r#"{"error":{"param":null,"message":"Unknown parameter: 'stream'."}}"#,
            "stream"
        ));
        // …so a proxy's "upstream error" can never blame `stream`.
        assert!(!error_blames_param("502 upstream error: connection reset", "stream"));
        // Dotted children match their root parameter.
        assert!(error_blames_param(
            r#"{"error":{"param":"reasoning.summary","message":"unsupported"}}"#,
            "reasoning"
        ));
    }

    #[test]
    fn sse_responses_stream_returns_the_completed_response_object() {
        let body = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"status\":\"in_progress\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial tokens\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"content\":\
[{\"type\":\"output_text\",\"text\":\"FINAL\"}]}]}}\n\n",
        );
        let v = assemble_sse(body.as_bytes(), SseFamily::Responses).unwrap();
        assert_eq!(v["output"][0]["content"][0]["text"], "FINAL");
    }

    #[test]
    fn sse_streams_fail_loudly_on_failure_and_on_no_result() {
        let failed =
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"boom\"}}}\n\n";
        let err = assemble_sse(failed.as_bytes(), SseFamily::Responses).unwrap_err().to_string();
        assert!(err.contains("boom"), "{err}");
        let no_result = "data: {\"type\":\"response.created\",\"response\":{}}\n\n";
        let err = assemble_sse(no_result.as_bytes(), SseFamily::Responses).unwrap_err().to_string();
        assert!(err.contains("without response.completed"), "{err}");
        let empty_chat = "data: {\"choices\":[{\"index\":0,\"delta\":{}}]}\n\ndata: [DONE]\n\n";
        let err = assemble_sse(empty_chat.as_bytes(), SseFamily::Chat).unwrap_err().to_string();
        assert!(err.contains("without any content"), "{err}");
    }
}
