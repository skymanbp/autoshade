//! AI segmentation bridge — Rust side of the sidecar (`python/segment.py`).
//!
//! Same shell-out pattern as [`crate::denoise`] (SCUNet): a local Python
//! process does the model inference and writes an 8-bit grayscale mask PNG
//! (white = selected, soft edges = the model's own alpha), which the app then
//! attaches to the recipe as a [`crate::recipe::MaskGeometry::Bitmap`] local
//! adjustment. The AI picks *where*; every actual edit stays a deterministic
//! recipe slider. Models auto-download on first run — into `python/weights`
//! (BiRefNet, OneFormer, SAM 2.1, each behind a sha256 gate) or `~/.u2net` for
//! the subject fallback tier, which gates its own — no weights in the repo.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::config::Config;

/// Everything one segmentation run needs; built from [`Config`] like
/// [`crate::denoise::DenoiseOpts`].
pub struct SegmentOpts {
    pub python_bin: String,
    pub script: PathBuf,
    /// `"subject"` (BiRefNet, with U²-Net as the fallback tier) or `"sky"`
    /// (OneFormer ADE20K Swin-L). Both backends are licence-pinned on the
    /// Python side — see `python/segment.py`'s module docstring: the sky model
    /// was SegFormer-B0 until R27 Batch-4, whose weights are licensed for
    /// "research or evaluation purposes only", and the subject fallback names
    /// its rembg session explicitly so an upstream default change cannot swap a
    /// pay-to-use model into a public build.
    ///
    /// **`"subject"` has TWO possible backends and the sidecar says which one
    /// ran** (R29 B4): the general BiRefNet checkpoint, or U²-Net when
    /// torchvision/timm/einops are missing or the pinned weights do not verify.
    /// The sidecar prints the backend on stdout and warns on stderr when it
    /// degraded; [`segment_file`] forwards that stderr on the success path so
    /// the two states cannot be read as one.
    pub target: String,
    /// `crs:ReferencePoint` as the sidecar spells it — two space-separated
    /// normalised floats. REQUIRED by `--target object` and ignored by the
    /// other two (R27 Batch-5): SAM 2.1 is point-prompted, and the point is
    /// the one spatial fact a Lightroom `Mask/Image` component carries.
    pub reference_point: Option<String>,
}

impl SegmentOpts {
    pub fn from_config(cfg: &Config, target: &str) -> Self {
        SegmentOpts {
            python_bin: cfg.python_bin.clone(),
            script: PathBuf::from(&cfg.segment_script),
            target: target.to_string(),
            reference_point: None,
        }
    }

    /// [`from_config`](SegmentOpts::from_config) for one imported AI mask: the
    /// backend its `crs:MaskSubType` names, prompted at its own click.
    ///
    /// The subtype mapping is the measured one — **0 = Object/Background,
    /// 1 = Subject, 2 = Sky** — and the 0 case is where the honesty lives:
    /// Object and Background share that value and the sidecar does not
    /// separate them, so the segmenter is pointed at the object under the
    /// click in both cases and the possibility that the photographer meant its
    /// complement is DISCLOSED rather than guessed at.
    pub fn for_ai_mask(cfg: &Config, subtype: u32, ref_x: f32, ref_y: f32) -> Option<Self> {
        let target = match subtype {
            0 => "object",
            1 => "subject",
            2 => "sky",
            // Not reachable from an imported mask (`xmp::parse_ai_mask`
            // refuses anything else), but this is a public constructor and a
            // subtype we cannot route is not one to guess a backend for.
            _ => return None,
        };
        Some(SegmentOpts {
            python_bin: cfg.python_bin.clone(),
            script: PathBuf::from(&cfg.segment_script),
            target: target.to_string(),
            // Space-separated with 6 decimals: the sidecar's own spelling
            // (`"0.517578 0.260997"`), so the value that reaches the model is
            // the value the file wrote to the precision the file wrote it at.
            reference_point: Some(format!("{ref_x:.6} {ref_y:.6}")),
        })
    }
}

/// The word the sidecar puts inside [`SegmentReport::backend`] when the run
/// took the fallback tier instead of the model the user ruled for.
///
/// It is `python/segment.py`'s own `U2NET_LABEL`
/// (`"U^2-Net (FALLBACK - BiRefNet did not run)"`), and this matches the ONE
/// word in it that states the condition rather than the model — so a future
/// fallback under a different name still reads as a fallback here.
/// `the_backend_label_survives_the_sidecar_line` pins the pairing from this
/// side; the constant's comment in `segment.py` names this file from the other.
const FALLBACK_MARK: &str = "FALLBACK";

/// What the sidecar said about the run that produced the mask.
///
/// Returned instead of `()` because `--target subject` has TWO possible
/// answers (R29 B4) and every surface downstream — the GUI's status line, the
/// alpha cache's provenance marker — needs to know which one ran. Before this,
/// the only carrier was a stderr line the Rust side forwarded to a console the
/// windowed GUI does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentReport {
    /// The backend named in the sidecar's own brackets, e.g.
    /// `"BiRefNet e2bf8e4460fc"`. EMPTY when the line could not be found —
    /// an older `segment.py` on disk, which this bridge tolerates elsewhere
    /// too (see the `durable_adopt` belt at the end of [`segment_file`]).
    pub backend: String,
    /// Whether BiRefNet's imports resolved on this machine for this run.
    /// `None` for the single-tier backends, which have no such question, and
    /// for a sidecar too old to print the line.
    pub birefnet_deps: Option<bool>,
}

/// Does this backend label name the fallback tier? See [`FALLBACK_MARK`].
///
/// Free-standing so the GUI can ask it of the bare label it carries in
/// `Msg::Segmented` without reconstituting a [`SegmentReport`] — the one
/// definition of "this is a degraded run", used on both sides.
pub fn backend_is_fallback(backend: &str) -> bool {
    backend.contains(FALLBACK_MARK)
}

impl SegmentReport {
    /// Did this run take the fallback tier? See [`backend_is_fallback`].
    pub fn is_fallback(&self) -> bool {
        backend_is_fallback(&self.backend)
    }

    /// Parse the sidecar's stdout. Two lines matter and both are contracts
    /// (`python/segment.py`'s `main`):
    ///
    /// ```text
    /// segment.py: subject mask [BiRefNet e2bf8e4460fc] -> C:\…\mask.png
    /// segment.py: subject backend deps [ok]
    /// ```
    ///
    /// Split on `\r` as well as `\n` for the reason the stderr forwarder does:
    /// a progress bar rewrites one physical line, so a newline-only split can
    /// hand back a single enormous "line".
    ///
    /// A path with a `]` in it cannot confuse the first line — the label is
    /// taken from the FIRST `[` to the FIRST `]` after it, and both sit left of
    /// the ` -> ` the path follows.
    fn parse(stdout: &str) -> SegmentReport {
        let bracketed = |line: &str| -> Option<String> {
            let open = line.find('[')?;
            let rest = &line[open + 1..];
            let close = rest.find(']')?;
            Some(rest[..close].to_string())
        };
        let mut out = SegmentReport { backend: String::new(), birefnet_deps: None };
        for line in stdout.split(['\n', '\r']) {
            let line = line.trim();
            let Some(body) = line.strip_prefix("segment.py:") else { continue };
            let body = body.trim();
            if body.contains(" mask [") {
                if let Some(label) = bracketed(body) {
                    out.backend = label;
                }
            } else if body.starts_with("subject backend deps") {
                out.birefnet_deps = match bracketed(body).as_deref() {
                    Some("ok") => Some(true),
                    Some("missing") => Some(false),
                    // A third word is a sidecar this build does not understand;
                    // "unknown" is the honest answer, not a coin flip.
                    _ => None,
                };
            }
        }
        out
    }
}

/// Run the sidecar: `input` (any image file) → `output` (8-bit grayscale PNG).
/// The mask is in the INPUT's frame — feed it the original-frame preview so it
/// lands in the same space recipe masks live in.
pub fn segment_file(opts: &SegmentOpts, input: &Path, output: &Path) -> Result<SegmentReport> {
    if !opts.script.exists() {
        bail!(
            "segmentation sidecar not found at {} — run from the project dir or set \
             AUTOSHOP_SEGMENT_SCRIPT.",
            opts.script.display()
        );
    }
    crate::pipeline::ensure_parent(output)?;
    // BEFORE the spawn: both call sites (`store::claim_raster`,
    // `pipeline::unique_out`) reserve the name by CREATING a 0-byte file, so
    // "the mask exists" is true before the sidecar ever runs — the original
    // `exists()` guard here never fired for them.
    let before = crate::artifact_state(output);
    let mut cmd = Command::new(&opts.python_bin);
    // What a `.env` may push at this child is an ALLOWLIST, not "everything
    // the capability table did not refuse" — see `config::dotenv_child_env`.
    // Compute knobs (CUDA_VISIBLE_DEVICES, thread counts) pass; anything that
    // names a path, a host, a credential or a library to load does not, which
    // is what stops a photo pack's `.env` from handing this process
    // LD_PRELOAD. The user's OWN environment is unaffected: nothing calls
    // env_clear, so the child still inherits it. `-E` below is the second
    // layer for PYTHON* specifically.
    cmd.envs(crate::config::dotenv_child_env());
    // `-E`: ignore PYTHON* environment variables — same import-hijack
    // guard as the denoise sidecar (config.rs protects them too).
    cmd.arg("-E")
        .arg(&opts.script)
        .arg("--input")
        .arg(input)
        .arg("--output")
        .arg(output)
        .arg("--target")
        .arg(&opts.target);
    if let Some(p) = &opts.reference_point {
        cmd.arg("--reference-point").arg(p);
    }
    cmd
        // CAPTURE, never inherit: the release GUI is windows_subsystem="windows"
        // and has NO console, so inherited handles discard the sidecar's output —
        // the reason a missing dependency used to surface as a bare exit code.
        // The tail goes into the error instead, which reaches the GUI toast.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Don't flash a console window when the windowed GUI spawns the sidecar.
    crate::hide_child_console(&mut cmd);
    // Tree-wide kill on timeout — same rule as the denoise sidecar (L11#7).
    crate::arm_kill_group(&mut cmd);
    let run = (|| -> Result<std::process::Output> {
        let child = cmd.spawn().with_context(|| {
            format!(
                "launch segmentation sidecar ({} {}) — is Python on PATH / AUTOSHOP_PYTHON set?",
                opts.python_bin,
                opts.script.display()
            )
        })?;
        let group = crate::assign_kill_group(&child);
        crate::denoise::bounded_child_output(
            child,
            "segmentation sidecar",
            crate::denoise::sidecar_timeout(),
            "AUTOSHOP_SIDECAR_TIMEOUT_SECS",
            group,
        )
    })();
    let out = match run {
        Ok(out) => out,
        Err(error) => {
            crate::denoise::discard_failed_output(output, before);
            return Err(error);
        }
    };
    if !out.status.success() {
        crate::denoise::discard_failed_output(output, before);
        bail!(
            "segmentation sidecar exited with {}: {}",
            out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
            crate::sidecar_tail(&out.stderr, &out.stdout)
        );
    }
    // Exit 0 alone is not success — THIS run must have produced a non-empty
    // mask (see `crate::sidecar_wrote` for the three refusals).
    if let Err(e) = crate::sidecar_wrote("segmentation sidecar", output, before) {
        crate::denoise::discard_failed_output(output, before);
        return Err(e);
    }
    // Byte-level acceptance is not MASK acceptance (L08-8): a sidecar that
    // wrote garbage (a truncated PNG, an HTML error body) used to be adopted
    // here and persist as the recipe's raster. The product must DECODE as a
    // raster before anything references it.
    if let Err(e) = crate::render::open_mask_bounded(output) {
        crate::denoise::discard_failed_output(output, before);
        return Err(e.context(format!(
            "segmentation sidecar produced an undecodable mask {}",
            output.display()
        )));
    }
    // WHAT THE SIDECAR SAID ON A RUN THAT SUCCEEDED, forwarded instead of
    // discarded (R29 B4). Until now `out.stderr` was read only on the failure
    // path, so a successful run could carry news no surface ever showed — and
    // R29 B4 created news worth showing: `--target subject` degrades to the
    // U²-Net fallback tier when BiRefNet cannot run, and the sidecar announces
    // that degradation here and nowhere else. "BiRefNet rendered this" and
    // "U²-Net rendered this because BiRefNet could not" are two different
    // sentences about the photographer's mask and must not collapse into one.
    // Also surfaces the first-run 444 MB download, which used to look like a
    // hang. Silent when the sidecar was silent.
    //
    // OUR OWN LINES ONLY, and the filter is a contract rather than a guess:
    // every line this sidecar family writes deliberately carries one of two
    // prefixes — `segment.py:` from this script's own `print`/`die`, and
    // `[denoise]` from the shared verified downloader it borrows. Everything
    // else on that stream belongs to a library. Measured, on the object
    // backend: `transformers` writes a 245 KB carriage-return progress bar per
    // mask, which forwarding wholesale would dump on the photographer's console
    // and bury the one line that matters. The failure path is unfiltered and
    // stays that way — a crash's message can come from anywhere.
    //
    // NOT through `sidecar_tail` either: that helper keeps the LAST 400
    // characters, which is right for a crash (the message ends the traceback)
    // and wrong here — a warning puts its headline first, and the 400-character
    // tail cut "WARNING - the BiRefNet subject backend did not run" off the
    // front of the very disclosure this block exists to carry.
    let said = String::from_utf8_lossy(&out.stderr);
    // `\r` as well as `\n`: progress bars rewrite one physical line, so
    // splitting on newlines alone can hand back a single 245 KB "line".
    for line in said.split(['\n', '\r']) {
        let line = line.trim();
        if line.starts_with("segment.py:") || line.starts_with("[denoise]") {
            eprintln!("[segment] {line}");
        }
    }
    // The mask must be durable BEFORE any recipe references it (L03). The
    // sidecar fsyncs before its os.replace now — this adopt is the belt for
    // an older script on disk, at the cost of one flush.
    crate::store::durable_adopt(output)
        .with_context(|| format!("sync segmentation output {}", output.display()))?;
    // STDOUT, which until now was read only to pad a failure message. It
    // carries the one fact the mask itself cannot: which of `--target
    // subject`'s two backends actually answered.
    Ok(SegmentReport::parse(&String::from_utf8_lossy(&out.stdout)))
}

/// Can THIS machine run the BiRefNet subject backend right now?
///
/// `Some(true)` / `Some(false)` from the sidecar's `--probe-backend`, `None`
/// when the question could not be asked at all (no sidecar, no Python, a
/// sidecar too old to know the flag) — and `None` must never be read as "no",
/// because acting on it would recompute alphas on evidence nobody has.
///
/// **Memoised for the process, and the memo is the point.** The probe costs one
/// interpreter start plus three imports: measured 4.3 s where torchvision IS
/// installed (it pulls torch in), ~0.1 s where it is not, against 0.06 s for a
/// bare `python -c pass`. A recipe with four AI masks must pay that at most
/// once, and [`resolve_ai_masks`] spends it at most once per develop — only
/// when a CACHED alpha's marker says it came from the fallback tier on a
/// machine that could not do better. On a machine that never fell back, or one
/// that still cannot, this is never called or returns in ~0.1 s.
///
/// The memo is keyed on nothing: a dependency appearing mid-session is a case
/// the NEXT session picks up, which is the same granularity `resolve_ai_masks`
/// already gives the cache.
fn birefnet_deps_available(opts: &SegmentOpts) -> Option<bool> {
    use std::sync::OnceLock;
    static PROBED: OnceLock<Option<bool>> = OnceLock::new();
    *PROBED.get_or_init(|| {
        if !opts.script.exists() {
            return None;
        }
        let mut cmd = Command::new(&opts.python_bin);
        // Same child-environment discipline as the segmentation spawn: an
        // allowlisted `.env`, `-E` against PYTHON* import hijacking, no
        // inherited console, and a kill group so a wedged probe cannot outlive
        // the develop.
        cmd.envs(crate::config::dotenv_child_env());
        cmd.arg("-E")
            .arg(&opts.script)
            .arg("--target")
            .arg("subject")
            .arg("--probe-backend")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::hide_child_console(&mut cmd);
        crate::arm_kill_group(&mut cmd);
        let child = cmd.spawn().ok()?;
        let group = crate::assign_kill_group(&child);
        let out = crate::denoise::bounded_child_output(
            child,
            "segmentation backend probe",
            crate::denoise::sidecar_timeout(),
            "AUTOSHOP_SIDECAR_TIMEOUT_SECS",
            group,
        )
        .ok()?;
        if !out.status.success() {
            return None;
        }
        SegmentReport::parse(&String::from_utf8_lossy(&out.stdout)).birefnet_deps
    })
}

/// The provenance marker written beside one cached AI alpha.
///
/// **Why a sidecar FILE and not a term in the cache key.** The key is spelled
/// by the Rust side BEFORE the sidecar chooses a backend, so it cannot contain
/// the answer — that is exactly the limitation R29 B4 registered and this
/// closes. The marker is written AFTER the run, by the side that has just been
/// told which backend answered.
///
/// It lives at `<alpha>.backend`, i.e. `ai-mask-<hash>.backend` beside
/// `ai-mask-<hash>.png` in the photo's own develop dir, so it is swept, cleared
/// and snapshotted with the alpha it describes and never outlives it in a way
/// that matters (a stray marker with no PNG is simply never read). It is NOT a
/// recipe reference, so nothing relativizes it and no schema grows a field.
///
/// Three lines, `key=value`, version-stamped — the same shape every other
/// small store record in this tree carries:
///
/// ```text
/// autoshop-ai-mask-backend v1
/// backend=U^2-Net (FALLBACK - BiRefNet did not run)
/// birefnet-deps=missing
/// ```
struct BackendMarker {
    backend: String,
    /// The dependency verdict AT THE TIME the alpha was written — not now.
    /// This is what makes the re-derivation rule terminate: see
    /// [`should_renew_cached_alpha`].
    birefnet_deps: Option<bool>,
}

const BACKEND_MARKER_HEADER: &str = "autoshop-ai-mask-backend v1";

/// A marker is three short lines; anything larger is not one. Bounded for the
/// reason every read in the store module is (R28 2a): the develop dir is not
/// always written by this app.
const MAX_BACKEND_MARKER: u64 = 4 * 1024;

fn backend_marker_path(alpha: &Path) -> PathBuf {
    alpha.with_extension("backend")
}

impl BackendMarker {
    fn render(&self) -> String {
        let deps = match self.birefnet_deps {
            Some(true) => "ok",
            Some(false) => "missing",
            None => "unknown",
        };
        format!("{BACKEND_MARKER_HEADER}\nbackend={}\nbirefnet-deps={deps}\n", self.backend)
    }

    fn parse(text: &str) -> Option<BackendMarker> {
        let mut lines = text.lines();
        // The header is a GATE, not decoration: without it this would happily
        // read provenance out of any file that happened to sit at the name.
        if lines.next()?.trim() != BACKEND_MARKER_HEADER {
            return None;
        }
        let mut m = BackendMarker { backend: String::new(), birefnet_deps: None };
        for line in lines {
            if let Some(v) = line.strip_prefix("backend=") {
                m.backend = v.trim_end().to_string();
            } else if let Some(v) = line.strip_prefix("birefnet-deps=") {
                m.birefnet_deps = match v.trim() {
                    "ok" => Some(true),
                    "missing" => Some(false),
                    _ => None,
                };
            }
        }
        Some(m)
    }

    fn read(alpha: &Path) -> Option<BackendMarker> {
        let p = backend_marker_path(alpha);
        let text = crate::store::read_text_capped(&p, MAX_BACKEND_MARKER).ok()?;
        BackendMarker::parse(&text)
    }

    /// Best effort, and deliberately so: the ALPHA is the artifact, the marker
    /// is provenance about it. A marker that could not be written costs one
    /// missed re-derivation opportunity; refusing the mask over it would cost
    /// the photographer their selection.
    fn write(alpha: &Path, report: &SegmentReport) {
        let m = BackendMarker {
            backend: report.backend.clone(),
            birefnet_deps: report.birefnet_deps,
        };
        let _ = crate::store::durable_write(&backend_marker_path(alpha), m.render().as_bytes());
    }
}

/// Should a cache HIT be thrown away and re-segmented?
///
/// Exactly one case says yes, and it is the one R29 B4 registered as missing:
/// **the alpha was produced by the fallback tier BECAUSE this machine could
/// not run BiRefNet, and it now can.** The photographer installed torchvision
/// since; serving them the softer U²-Net edges forever, under a cache key that
/// records neither model, is the cache lying about provenance.
///
/// **Why the recorded verdict and not just "is it a fallback".** A machine that
/// has the dependencies but fails for another reason — a digest mismatch, a
/// card that will not hold the weights — falls back too, and re-running it on
/// every develop would be an infinite retry loop that never improves. Comparing
/// the verdict RECORDED WITH THE ALPHA against today's makes the rule fire only
/// on an actual CHANGE in this machine's capability, so it fires once and then
/// stops: the re-run rewrites the marker with today's verdict either way.
///
/// **Everything unknown means NO.** No marker (an alpha written before this
/// build), an unparseable one, or a probe that could not run all leave the
/// cache alone. That costs nothing real: `AI_BACKEND_GENERATION` 2 has not
/// shipped, so every alpha that survives into v0.35.0 re-derives once anyway
/// and is written by this build, with a marker.
fn should_renew_cached_alpha(alpha: &Path, opts: &SegmentOpts) -> bool {
    let Some(m) = BackendMarker::read(alpha) else { return false };
    // The MARKER half first, so the 4.3 s probe is never spent on an alpha
    // whose own record already says the answer cannot change.
    if !backend_is_fallback(&m.backend) || m.birefnet_deps != Some(false) {
        return false;
    }
    marker_is_stale(&m, birefnet_deps_available(opts))
}

/// The decision of [`should_renew_cached_alpha`] with today's verdict handed
/// IN, so the rule can be exercised by a test instead of being reachable only
/// through a real interpreter and a process-wide memo — the same split
/// [`ai_cache_key_gen`] makes for the generation term.
fn marker_is_stale(m: &BackendMarker, deps_now: Option<bool>) -> bool {
    backend_is_fallback(&m.backend) && m.birefnet_deps == Some(false) && deps_now == Some(true)
}

/// What one [`resolve_ai_masks`] pass did, for the disclosure channels.
///
/// Counts, never a boolean: "3 AI masks re-derived, 1 declined" is a different
/// sentence from "some AI masks did not render", and the photographer needs
/// the first one.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AiMaskResolution {
    /// Masks that now have an alpha — cache hits included.
    pub resolved: usize,
    /// Masks reused from the cache without running the model.
    pub cached: usize,
    /// Masks whose CACHED alpha was discarded and re-segmented because this
    /// machine gained the primary backend since it was written
    /// ([`should_renew_cached_alpha`]). Counted separately from `resolved`'s
    /// plain re-derivations: "your machine can now do better, so this mask was
    /// redone" is a different sentence from "this mask had no alpha yet", and
    /// the photographer paying seconds of GPU is owed the first one.
    pub renewed: usize,
    /// Masks with no alpha, each with the reason, in recipe order. These
    /// render INERT and raise [`crate::xmp::MaskImportReason::AiMaskUnresolved`].
    pub unresolved: Vec<(String, String)>,
}

impl AiMaskResolution {
    /// One English line for the CLI, or `None` when there was nothing to say.
    ///
    /// It always names the RECOMPUTATION, never just the count: a
    /// photographer who sees "3 AI masks rendered" and does not see "by the
    /// local segmenter, not Adobe's" has been told the wrong thing.
    pub fn describe(&self) -> Option<String> {
        if self.resolved == 0 && self.unresolved.is_empty() {
            return None;
        }
        let mut s = String::new();
        if self.resolved > 0 {
            s.push_str(&format!(
                "{} AI mask(s) re-derived by the local segmenter (NOT Adobe's own raster — the \
                 sidecar carries no pixels, so the alpha is an approximation of the \
                 photographer's intent)",
                self.resolved
            ));
            if self.cached > 0 {
                s.push_str(&format!("; {} reused from the develop cache", self.cached));
            }
            if self.renewed > 0 {
                s.push_str(&format!(
                    "; {} re-segmented because this machine can now run the BiRefNet subject \
                     backend it previously fell back from",
                    self.renewed
                ));
            }
        }
        for (name, why) in &self.unresolved {
            if !s.is_empty() {
                s.push_str("; ");
            }
            s.push_str(&format!("AI mask \"{name}\" carried but NOT rendered ({why})"));
        }
        Some(s)
    }
}

/// The cache key for one AI mask's recomputed alpha: everything that decides
/// the pixels, and nothing that does not.
///
/// `(photo identity, subtype, reference point, FRAME, backend generation)` — so
/// a re-render of the same recipe reuses the file, a moved click recomputes, a
/// TURNED photo recomputes, and a photo's two AI masks never collide. The NAME
/// is deliberately absent: it is a localised label the photographer may rename
/// without changing one pixel.
///
/// **The frame term is R28 Batch-3 (adjudication F1-A), and the sentence above
/// was FALSE without it.** The segmenter runs on the frame
/// [`stage_source_frame`] stages, and that frame is chosen by the recipe's
/// `quarter_turns` — so the turn decides the pixels as surely as the click
/// does. A rotate normally moves the click too
/// ([`crate::render::orient_recipe_coords`] turns `ref_x`/`ref_y` with the
/// frame), which hid the omission behind a key that changed anyway — except at
/// the ONE point every quarter turn leaves where it was, the frame centre
/// `(0.5, 0.5)`. A mask clicked there kept its key across a rotate and the
/// cache served the alpha segmented in the OLD frame, sideways, with every
/// surface reporting a normal cache hit.
///
/// **Behaviour disclosure: every alpha cached by v0.33 and earlier re-derives
/// once.** Adding a field re-spells every key, so the first develop after this
/// build runs the segmenter again per mask (seconds of GPU) and reports it
/// through [`AiMaskResolution::describe`] as the re-derivation it is. That is
/// cache invalidation doing its job — the alternative is serving alphas whose
/// frame nobody recorded. The superseded PNGs stay in the develop dir, the
/// same accepted residue as any superseded raster ([`crate::store::claim_raster`]).
///
/// `AI_BACKEND_GENERATION` is bumped whenever the segmenter's own output would
/// change — a model re-pin, a preprocessing change, the mask-size cap. Without
/// it the cache would happily serve an alpha produced by a model this build no
/// longer runs, which is the one way a cache can lie about provenance. It is
/// deliberately NOT bumped for the frame term: the model did not change, the
/// key's shape did, and conflating the two would leave the next reader thinking
/// this build's segmenter differs from v0.33's.
///
/// **Generation 2 (R29 B4, user ruling five of 2026-08-21): the SUBJECT backend
/// is now `ZhengPeng7/BiRefNet` and every subject alpha cached by v0.34 and
/// earlier re-derives once.** This is the case the constant was written for — a
/// model re-pin, measured on the photographer's own library and ruled on rather
/// than guessed. The sky and object backends did not move; they re-derive too,
/// because one constant covers all three, and the alternative (a generation per
/// backend) buys three fields to save one re-run of a model whose alpha is
/// cached beside the develop anyway.
///
/// **The limitation this constant CANNOT express, and where it is handled
/// instead (R29 C3/C4).** Generation 2 means "BiRefNet, or U²-Net if this
/// machine cannot run BiRefNet" — the Rust side spells the key before the
/// sidecar chooses the backend, so no term of this string can record which one
/// answered, and widening the generation would not help: it is one number for
/// every machine, and the fact in question differs per machine. A machine that
/// fell back and LATER installs torchvision used to keep being served its
/// U²-Net alphas forever, with "delete the develop's `ai-mask-*.png`" as the
/// only remedy — a manual step nobody could know they needed. That is now
/// automatic and lives one layer out, in the per-alpha [`BackendMarker`] that
/// [`should_renew_cached_alpha`] reads: provenance belongs beside the artifact,
/// not in its name.
const AI_BACKEND_GENERATION: u32 = 2;

fn ai_cache_key(raw: &Path, subtype: u32, ref_x: f32, ref_y: f32, quarter_turns: u8) -> String {
    ai_cache_key_gen(raw, subtype, ref_x, ref_y, quarter_turns, AI_BACKEND_GENERATION)
}

/// [`ai_cache_key`] with the generation handed IN, so the term can be exercised
/// by a test instead of being the one field in the identity string that only a
/// release could prove. Nothing but [`ai_cache_key`] and its tests calls it, and
/// nothing but [`AI_BACKEND_GENERATION`] is ever passed in production.
fn ai_cache_key_gen(
    raw: &Path,
    subtype: u32,
    ref_x: f32,
    ref_y: f32,
    quarter_turns: u8,
    generation: u32,
) -> String {
    // FNV-1a over the identity string: small, stable across runs and platforms
    // (unlike `DefaultHasher`, whose output std explicitly does not guarantee
    // between releases — a cache name that moved between builds would silently
    // re-run the model on every upgrade).
    //
    // `% 4` because that is what the RENDER folds the field by
    // (`render::quarter_turn_orientation`): a hand-edited `quarter_turns: 5`
    // renders as one turn, and the key must not invent a second frame for it.
    let ident = format!(
        "{}|{subtype}|{ref_x:.6}|{ref_y:.6}|t{}|{generation}",
        crate::store::photo_key(raw),
        quarter_turns % 4
    );
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in ident.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("ai-mask-{h:016x}")
}

/// Give every [`crate::recipe::MaskGeometry::AiMask`] in `recipe` an alpha,
/// running the segmentation sidecar only for the ones that do not already have
/// a usable cached raster.
///
/// **This is a RECOMPUTATION, not an import, and the whole design turns on
/// that.** Lightroom's `Mask/Image` component carries no raster and no
/// geometry — only the intent (`MaskSubType` + `ReferencePoint`) and
/// provenance digests. So the only way to render one is to run a segmenter of
/// our own, whose alpha differs from Adobe's at every edge and can differ
/// grossly on a hard scene. Every surface that shows the result must say so;
/// [`AiMaskResolution::describe`] is this function's half of that, and
/// [`crate::xmp::MaskImportReason::AiMaskRecomputed`] is the parser's.
///
/// **Lazy and cached.** Importing a library must not spawn a model run per
/// photo, so nothing happens at parse time; this runs at DEVELOP time, and a
/// cache hit costs one `open_mask_bounded` instead of seconds of GPU. The
/// raster lands in the photo's own develop dir under a name derived from
/// [`ai_cache_key`], so it is swept, snapshotted and relativized with every
/// other mask raster (`LocalAdjustment::bitmap_paths_mut`).
///
/// **A cache hit is not unconditional.** Each alpha carries a
/// [`BackendMarker`] naming the backend that made it, and one written by the
/// fallback tier on a machine that could not run BiRefNet is thrown away once
/// that machine can — see [`should_renew_cached_alpha`] for why that rule
/// terminates instead of retrying forever.
///
/// **TWO paths, and they are not interchangeable (R28 Batch-3, adjudication
/// F1-C).** `raw` is the PHOTO — the identity every cache decision is made on:
/// [`ai_cache_key`] hashes it and [`crate::store::raster_target`] homes the
/// alpha in ITS develop dir, which is what makes the file relativizable,
/// snapshottable and sweepable. `src` is the PIXELS the render will use, and
/// that may be a baked retouch master (`store::render_source_checked`) living
/// somewhere else entirely. Both call sites used to hand the master in as the
/// only argument, so the key and the cache home were computed from the master's
/// path — a develop directory belonging to no photo — and the staging decoder
/// (RAW-only, then) simply failed on it.
///
/// **The frame is the RECIPE's, not the disk's (adjudication F1-B).** The
/// staged frame is decoded at `recipe.quarter_turns`, the same number
/// `render::render_to_image_in` and `render::render_baked_to_image` turn by, so
/// the alpha is segmented in the frame it will be sampled in. Reading the turn
/// back off the SAVED develop instead (`store::saved_quarter_turns`) made a
/// recipe that disagrees with the store — an external `apply`, a rotate not yet
/// saved — segment one frame and render another, and cached the result under a
/// key that recorded neither.
///
/// **A failure is never silent and never fatal.** A missing sidecar, a missing
/// dependency, a declined mask (the sidecar's exit 3) or an undecodable output
/// all leave `raster` at `None` — which the render treats as "this mask needs
/// pixels and has none" and SKIPS the whole adjustment rather than applying it
/// at weight 0 (a 0 under `inverted` would apply the edit to the entire
/// frame). The reason is carried out in `unresolved`.
pub fn resolve_ai_masks(
    cfg: &Config,
    raw: &Path,
    src: &Path,
    recipe: &mut crate::recipe::EditRecipe,
) -> AiMaskResolution {
    use crate::recipe::MaskGeometry;

    let mut out = AiMaskResolution::default();
    // Read the frame ONCE, before the mask walk borrows the recipe mutably —
    // and read it from the recipe being resolved, which is the whole point of
    // handing this function the authoritative recipe in the first place.
    let turns = recipe.quarter_turns % 4;
    // The source frame the sidecar segments in. Produced ONCE and only when
    // something actually needs it — a recipe with no unresolved AI mask must
    // not pay a RAW decode.
    let mut preview: Option<Option<PathBuf>> = None;

    for m in recipe.masks.iter_mut() {
        let mask_name = if m.name.is_empty() { "unnamed".to_string() } else { m.name.clone() };
        for g in std::iter::once(&mut m.mask).chain(m.components.iter_mut().map(|c| &mut c.geometry))
        {
            let MaskGeometry::AiMask { subtype, ref_x, ref_y, raster, .. } = g else { continue };
            let (subtype, rx, ry) = (*subtype, *ref_x, *ref_y);
            // KEYED AND HOMED ON THE PHOTO, never on the pixel source (F1-C).
            let target =
                crate::store::raster_target(raw, &ai_cache_key(raw, subtype, rx, ry, turns));
            // Built BEFORE the cache probe now, because the probe needs it —
            // `should_renew_cached_alpha` asks the sidecar whether this machine
            // has gained the primary backend. Still cheap (two clones and a
            // format), and a subtype with no backend keeps its old behaviour:
            // the cache still answers for it, and only a MISS reports it.
            let opts = SegmentOpts::for_ai_mask(cfg, subtype, rx, ry);
            // CACHE HIT, and it is checked by DECODING, not by `exists()`: a
            // half-written or truncated PNG on disk is exactly the file a
            // cheap existence test would serve forever.
            let mut renewing = false;
            if target.exists() && crate::render::open_mask_bounded(&target).is_ok() {
                renewing = opts.as_ref().is_some_and(|o| should_renew_cached_alpha(&target, o));
                if !renewing {
                    *raster = Some(target.to_string_lossy().into_owned());
                    out.resolved += 1;
                    out.cached += 1;
                    continue;
                }
                // Cleared BEFORE the re-run, both files. `sidecar_wrote`
                // refuses an output whose length AND mtime are unchanged, so a
                // stale alpha left in place could turn a real re-run into a
                // reported failure; and a marker left behind a re-run that
                // fails would keep claiming a provenance nothing produced.
                let _ = std::fs::remove_file(&target);
                let _ = std::fs::remove_file(backend_marker_path(&target));
            }
            let Some(opts) = opts else {
                out.unresolved.push((mask_name.clone(), format!("no backend for subtype {subtype}")));
                continue;
            };
            if !opts.script.exists() {
                out.unresolved.push((
                    mask_name.clone(),
                    format!("the segmentation sidecar is not at {}", opts.script.display()),
                ));
                continue;
            }
            // The preview, decoded at most once per call — from the RENDER
            // SOURCE's pixels, in the RECIPE's frame.
            let input = preview.get_or_insert_with(|| stage_source_frame(src, turns));
            let Some(input) = input.as_ref() else {
                out.unresolved
                    .push((mask_name.clone(), "the source frame could not be decoded".into()));
                continue;
            };
            match segment_file(&opts, input, &target) {
                Ok(report) => {
                    // The marker lands with the alpha, not before it: a
                    // provenance record for a mask that does not exist would be
                    // read by the next develop and believed.
                    BackendMarker::write(&target, &report);
                    *raster = Some(target.to_string_lossy().into_owned());
                    out.resolved += 1;
                    // Counted on the SUCCESS arm, so `renewed` means "was
                    // re-segmented", which is what the disclosure says. A
                    // renewal whose re-run failed is an `unresolved`, and
                    // claiming it as a re-derivation would be the counter
                    // reporting an intention rather than an outcome.
                    if renewing {
                        out.renewed += 1;
                    }
                }
                Err(e) => {
                    // `segment_file` already discarded whatever it wrote, so
                    // the cache is not poisoned with a failed run.
                    out.unresolved.push((mask_name.clone(), format!("{e:#}")));
                }
            }
        }
    }
    // The staged source frame is an intermediate — the MASKS are the artifacts.
    if let Some(Some(p)) = preview.as_ref() {
        let _ = std::fs::remove_file(p);
    }
    out
}

/// Stage the render source's preview as a PNG for the sidecar, in the frame
/// `quarter_turns` produces — or `None` if it cannot be decoded.
///
/// **The RECIPE's frame, and the same door the render uses.**
/// [`crate::decode::decode_any_turned`] dispatches RAW → `decode_raw_turned`
/// (EXIF composed with the turn) and baked → `decode_baked` + the turn, which
/// is exactly the pair `render::render_to_image_in` / `render::
/// render_baked_to_image` apply — so the frame the segmenter sees IS the frame
/// the alpha will be sampled in, for a camera RAW and for a baked retouch
/// master alike. Before R28 Batch-3 this called the RAW-only decoder with the
/// turn read off the saved develop: a baked master could not be decoded at all
/// (the mask was carried, disclosed, and never rendered), and a recipe whose
/// turn differed from the store's produced a correct-looking alpha in the wrong
/// frame (adjudication F1-B/C).
///
/// A `crs:ReferencePoint` is normalised to the frame the photographer clicked
/// in, and `orient_recipe_coords` turns it with every rotate, so the click and
/// this frame stay in the same space; the mask is sampled in that same
/// normalised space (`render::sample_gray_norm`), so both ends agree without
/// either one knowing the photo's pixel dimensions.
fn stage_source_frame(src: &Path, quarter_turns: u8) -> Option<PathBuf> {
    let decoded = crate::decode::decode_any_turned(src, quarter_turns)
        .map_err(|e| {
            eprintln!("⚠ AI mask: cannot decode {} for segmentation: {e:#}", src.display());
        })
        .ok()?;
    let out = staging_path(src);
    decoded
        .preview
        .to_rgb8()
        .save(&out)
        .map_err(|e| eprintln!("⚠ AI mask: cannot stage the source frame {}: {e}", out.display()))
        .ok()?;
    Some(out)
}

/// A FRESH temp name for one staged source frame.
///
/// pid + seq + stem, the shape `style::embed_preview` settled on
/// (`style.rs:400`) after the pid + tag pair proved insufficient there for
/// exactly this reason. `resolve_ai_masks` has two single-photo callers today
/// (`apply`, `auto`) and neither runs concurrently, so the collision is LATENT
/// rather than live — which is precisely why the sequence belongs here and not
/// in a future incident report: the day a pool calls this (the `--jobs`
/// sequencer already runs `process_one` three-wide) two workers staging the
/// same stem would overwrite each other's frame and each other's cleanup, and
/// the symptom would be one photo's mask computed on another photo's pixels.
/// R28 Batch-3, adjudication F1-D.
fn staging_path(src: &Path) -> PathBuf {
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "autoshop-ai-src-{}-{}-{}.png",
        std::process::id(),
        TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        crate::pipeline::stem(src)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M-S1: the `crate::sidecar_wrote` call removed (or reverted to the old
    /// bare `exists()` guard) — the pre-claimed 0-byte mask file that
    /// `claim_raster`/`unique_out` create must NOT count as a written mask
    /// when the sidecar exits 0 without filling it.
    #[test]
    fn a_preclaimed_empty_mask_the_sidecar_never_fills_is_refused() {
        let dir = std::env::temp_dir()
            .join(format!("autoshop-seg-test-claimed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Exits 0, ignores every argument, writes nothing.
        #[cfg(windows)]
        let bin = {
            let p = dir.join("noop.bat");
            std::fs::write(&p, "@exit /b 0\r\n").unwrap();
            p
        };
        #[cfg(not(windows))]
        let bin = {
            use std::os::unix::fs::PermissionsExt;
            let p = dir.join("noop.sh");
            std::fs::write(&p, "#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let script = dir.join("segment.py");
        std::fs::write(&script, "# stand-in\n").unwrap();
        let opts = SegmentOpts {
            python_bin: bin.to_string_lossy().into_owned(),
            script,
            target: "sky".into(),
            reference_point: None,
        };
        let input = dir.join("in.png");
        std::fs::write(&input, b"not-really-a-png").unwrap();
        let output = dir.join("mask.png");
        std::fs::write(&output, b"").unwrap(); // the claim file

        let err = segment_file(&opts, &input, &output).unwrap_err().to_string();
        assert!(err.contains("is empty"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R29 C3/C4 — the sidecar's stdout is a CONTRACT and this is its parser.
    ///
    /// `--target subject` has two possible backends and until this batch the
    /// only thing that knew which one ran was a stderr line forwarded to a
    /// console the windowed GUI does not have. The two lines parsed here are
    /// written by `python/segment.py`'s `main`; changing either spelling
    /// without changing this test is the drift it exists to catch.
    ///
    /// MUTATION-LINED: taking the label from the LAST `]` instead of the first
    /// fails on the path case; dropping the `\r` from the split fails on the
    /// progress-bar case; treating an unknown deps word as `false` fails the
    /// last case and would silently re-derive every cached alpha.
    #[test]
    fn the_backend_label_survives_the_sidecar_line() {
        let primary = SegmentReport::parse(
            "segment.py: subject mask [BiRefNet e2bf8e4460fc] -> C:\\x\\mask.png\n\
             segment.py: subject backend deps [ok]\n",
        );
        assert_eq!(primary.backend, "BiRefNet e2bf8e4460fc");
        assert_eq!(primary.birefnet_deps, Some(true));
        assert!(!primary.is_fallback(), "the pinned model is not a fallback");

        let fell_back = SegmentReport::parse(
            "segment.py: subject mask [U^2-Net (FALLBACK - BiRefNet did not run)] -> m.png\n\
             segment.py: subject backend deps [missing]\n",
        );
        assert!(fell_back.is_fallback(), "the sidecar's own word for it must read as one here");
        assert_eq!(fell_back.birefnet_deps, Some(false));

        // A path containing a bracket must not eat the label — the label ends
        // at the FIRST `]`, and the path is right of the ` -> `.
        let bracketed = SegmentReport::parse(
            "segment.py: sky mask [OneFormer ADE20K Swin-L 4a5bac8e64f8] -> D:\\a[1]\\m.png\n",
        );
        assert_eq!(bracketed.backend, "OneFormer ADE20K Swin-L 4a5bac8e64f8");
        assert_eq!(bracketed.birefnet_deps, None, "sky has no fallback tier to report");

        // A progress bar rewrites ONE physical line with `\r`, so a
        // newline-only split can hand back a single enormous "line" with the
        // contract line buried inside it.
        let after_bar = SegmentReport::parse(
            "loading 10%\rloading 90%\rsegment.py: object mask [SAM 2.1 Hiera-Large 665f8e2ad61c] -> m.png\r",
        );
        assert_eq!(after_bar.backend, "SAM 2.1 Hiera-Large 665f8e2ad61c");

        // An older sidecar says nothing we understand: that is UNKNOWN, and
        // every consumer must treat unknown as "leave it alone".
        let silent = SegmentReport::parse("some library wrote this\n");
        assert!(silent.backend.is_empty());
        assert_eq!(silent.birefnet_deps, None);
        let odd = SegmentReport::parse("segment.py: subject backend deps [perhaps]\n");
        assert_eq!(odd.birefnet_deps, None, "a word this build does not know is not a verdict");
    }

    /// The label reaches the CALLER, not just the parser — the transport item 1
    /// of R29 C3/C4 needed. A stand-in sidecar prints the contract line and
    /// publishes a real PNG; `segment_file` must hand the bracketed text back.
    ///
    /// MUTATION-LINED: returning `Ok(SegmentReport::default())` instead of
    /// parsing `out.stdout`, or reverting the signature to `Result<()>`, fails
    /// here and in the GUI's `Msg::Segmented` arm.
    #[test]
    fn a_successful_run_hands_the_backend_label_back() {
        let dir = std::env::temp_dir()
            .join(format!("autoshop-seg-test-label-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // A REAL 1×1 grey PNG, because `segment_file` decodes before adopting.
        let png = dir.join("seed.png");
        image::GrayImage::from_pixel(1, 1, image::Luma([128u8])).save(&png).unwrap();
        let png_arg = png.to_string_lossy().replace('/', "\\");

        // Copies the seed PNG over the --output argument, prints the two
        // contract lines, exits 0. Argument 6 is `--output`'s value: the
        // command line is `-E <script> --input <in> --output <out> --target <t>`.
        #[cfg(windows)]
        let bin = {
            let p = dir.join("label.bat");
            std::fs::write(
                &p,
                format!(
                    "@copy /y \"{png_arg}\" \"%~6\" >nul\r\n\
                     @echo segment.py: subject mask [U^^2-Net (FALLBACK - BiRefNet did not run)] -^> %~6\r\n\
                     @echo segment.py: subject backend deps [missing]\r\n\
                     @exit /b 0\r\n"
                ),
            )
            .unwrap();
            p
        };
        #[cfg(not(windows))]
        let bin = {
            use std::os::unix::fs::PermissionsExt;
            let p = dir.join("label.sh");
            std::fs::write(
                &p,
                format!(
                    "#!/bin/sh\ncp '{}' \"$6\"\n\
                     echo 'segment.py: subject mask [U^2-Net (FALLBACK - BiRefNet did not run)] -> '\"$6\"\n\
                     echo 'segment.py: subject backend deps [missing]'\nexit 0\n",
                    png.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let script = dir.join("segment.py");
        std::fs::write(&script, "# stand-in\n").unwrap();
        let opts = SegmentOpts {
            python_bin: bin.to_string_lossy().into_owned(),
            script,
            target: "subject".into(),
            reference_point: None,
        };
        let input = dir.join("in.png");
        std::fs::write(&input, b"src bytes").unwrap();
        let output = dir.join("mask.png");

        let report = segment_file(&opts, &input, &output).expect("the stand-in run succeeds");
        assert_eq!(report.backend, "U^2-Net (FALLBACK - BiRefNet did not run)");
        assert!(report.is_fallback(), "the GUI keys its warning on exactly this");
        assert_eq!(report.birefnet_deps, Some(false));

        // And the marker the cache writes from it round-trips.
        BackendMarker::write(&output, &report);
        let back = BackendMarker::read(&output).expect("the marker is readable");
        assert_eq!(back.backend, report.backend);
        assert_eq!(back.birefnet_deps, Some(false));
        assert_eq!(
            backend_marker_path(&output).file_name().unwrap(),
            std::ffi::OsStr::new("mask.backend"),
            "the marker sits beside the alpha, sharing its stem"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The marker's own gate: a file that is not one of ours is not read as
    /// provenance. The develop dir is not always written by this app (R28 2a),
    /// and "no marker" and "a marker saying nothing" must both mean UNKNOWN.
    #[test]
    fn a_foreign_file_at_the_marker_name_is_not_read_as_provenance() {
        assert!(BackendMarker::parse("backend=BiRefNet\nbirefnet-deps=ok\n").is_none(),
            "without the header line this is not a marker");
        assert!(BackendMarker::parse("").is_none());
        let m = BackendMarker::parse(&format!("{BACKEND_MARKER_HEADER}\n")).expect("header alone parses");
        assert!(m.backend.is_empty());
        assert_eq!(m.birefnet_deps, None);
        let m = BackendMarker::parse(&format!(
            "{BACKEND_MARKER_HEADER}\nbackend=BiRefNet e2bf8e4460fc\nbirefnet-deps=ok\n"
        ))
        .expect("a real marker parses");
        assert_eq!(m.backend, "BiRefNet e2bf8e4460fc");
        assert_eq!(m.birefnet_deps, Some(true));
    }

    /// R29 C3/C4 item 2 — the whole decision table for re-deriving a cached
    /// alpha, and the two properties that matter are the ones that are easy to
    /// get wrong: it FIRES on the machine that gained the dependency, and it
    /// TERMINATES on the machine that has the dependency and still falls back.
    ///
    /// MUTATION-LINED: dropping the `birefnet_deps == Some(false)` term (i.e.
    /// renewing on "is it a fallback" alone) turns row 3 true, which is an
    /// infinite re-run loop — a model run per develop, forever, on a machine
    /// where it can never improve. Treating `None` as `true` turns rows 4 and 5
    /// true and re-derives every alpha on a machine that cannot answer.
    #[test]
    fn a_fallback_alpha_is_renewed_only_when_this_machine_gained_the_backend() {
        let mk = |backend: &str, deps: Option<bool>| BackendMarker {
            backend: backend.to_string(),
            birefnet_deps: deps,
        };
        let fallback = "U^2-Net (FALLBACK - BiRefNet did not run)";
        let primary = "BiRefNet e2bf8e4460fc";

        // 1. Fell back for want of the dependency, and the dependency landed.
        assert!(marker_is_stale(&mk(fallback, Some(false)), Some(true)));
        // 2. Same alpha, machine still cannot run it — nothing to gain.
        assert!(!marker_is_stale(&mk(fallback, Some(false)), Some(false)));
        // 3. THE TERMINATION CASE: the deps were there and it fell back anyway
        //    (a digest mismatch, a card that will not hold the weights). The
        //    re-run rewrote the marker with today's verdict, so the next
        //    develop must NOT try again.
        assert!(!marker_is_stale(&mk(fallback, Some(true)), Some(true)));
        // 4. The probe could not run at all: unknown is not "yes".
        assert!(!marker_is_stale(&mk(fallback, Some(false)), None));
        // 5. An older marker with no verdict recorded is likewise not "yes".
        assert!(!marker_is_stale(&mk(fallback, None), Some(true)));
        // 6. The pinned model's own alphas are never thrown away.
        assert!(!marker_is_stale(&mk(primary, Some(true)), Some(true)));
        assert!(!marker_is_stale(&mk(primary, Some(false)), Some(true)));
    }

    /// An alpha with NO marker beside it keeps its cache hit. The whole
    /// population this could matter for is empty for released builds —
    /// `AI_BACKEND_GENERATION` 2 has not shipped, so every alpha surviving into
    /// v0.35.0 re-derives once and is written by this build, with a marker —
    /// and guessing "it was probably a fallback" would spend a model run per
    /// mask on evidence nobody has.
    #[test]
    fn an_unmarked_cached_alpha_is_left_alone() {
        let dir = std::env::temp_dir()
            .join(format!("autoshop-seg-test-unmarked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let alpha = dir.join("ai-mask-0123456789abcdef.png");
        std::fs::write(&alpha, b"pretend png").unwrap();
        let opts = SegmentOpts {
            // A python_bin that cannot exist, so a probe that DID run would be
            // visible as a hang or a panic rather than passing by accident.
            python_bin: "autoshop-no-such-interpreter".into(),
            script: dir.join("does-not-exist.py"),
            target: "subject".into(),
            reference_point: None,
        };
        assert!(!should_renew_cached_alpha(&alpha, &opts), "no marker ⇒ no re-derivation");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L08-8: exit 0 + changed bytes is still not a mask — a sidecar that
    /// writes garbage is refused, and the garbage does not persist.
    #[test]
    fn a_garbage_mask_from_the_sidecar_is_refused() {
        let dir = std::env::temp_dir()
            .join(format!("autoshop-seg-test-garbage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Writes non-PNG bytes to the --output argument (%6 / $6) and exits 0.
        #[cfg(windows)]
        let bin = {
            let p = dir.join("garbage.bat");
            std::fs::write(&p, "@echo this is not a png> \"%~6\"\r\n@exit /b 0\r\n").unwrap();
            p
        };
        #[cfg(not(windows))]
        let bin = {
            use std::os::unix::fs::PermissionsExt;
            let p = dir.join("garbage.sh");
            std::fs::write(&p, "#!/bin/sh\necho not a png > \"$6\"\nexit 0\n").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let script = dir.join("segment.py");
        std::fs::write(&script, "# stand-in\n").unwrap();
        let opts = SegmentOpts {
            python_bin: bin.to_string_lossy().into_owned(),
            script,
            target: "sky".into(),
            reference_point: None,
        };
        let input = dir.join("in.png");
        std::fs::write(&input, b"src bytes").unwrap();
        let output = dir.join("mask.png");
        std::fs::write(&output, b"").unwrap(); // the claim file

        let err = segment_file(&opts, &input, &output).unwrap_err().to_string();
        assert!(err.contains("undecodable mask"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn ai_geometry(subtype: u32, ref_x: f32, ref_y: f32) -> crate::recipe::MaskGeometry {
        crate::recipe::MaskGeometry::AiMask {
            name: "Sky 1".into(),
            subtype,
            ref_x,
            ref_y,
            blend_mode: 0,
            value: 1.0,
            inverted: false,
            mask_version: 1,
            provenance: Vec::new(),
            gesture: Vec::new(),
            raster: None,
        }
    }

    fn ai_recipe(g: crate::recipe::MaskGeometry) -> crate::recipe::EditRecipe {
        crate::recipe::EditRecipe {
            masks: vec![crate::recipe::LocalAdjustment {
                name: "Mask 1".into(),
                mask: g,
                exposure_ev: 0.5,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// The cache key is exactly the things that decide the PIXELS: the photo,
    /// the subtype (which backend runs), the click (where it is prompted), the
    /// FRAME it is segmented in and the backend generation. Nothing else — the
    /// mask NAME is a localised label the photographer may rename without
    /// changing one pixel.
    ///
    /// MUTATION-LINED. Verified red by dropping `ref_x`/`ref_y` from
    /// `ai_cache_key`'s identity string (transcript in the batch report): two
    /// masks clicked on opposite sides of the same photo then share one cached
    /// alpha, so the second one silently renders the first one's selection.
    #[test]
    fn the_ai_cache_key_covers_the_photo_the_subtype_and_the_click() {
        let a = Path::new("D:/rolls/2024/DSC0001.ARW");
        let b = Path::new("D:/rolls/2024/DSC0002.ARW");
        let base = ai_cache_key(a, 2, 0.5, 0.5, 0);
        assert_eq!(base, ai_cache_key(a, 2, 0.5, 0.5, 0), "the key is stable across calls");
        assert_ne!(base, ai_cache_key(b, 2, 0.5, 0.5, 0), "a different photo, a different alpha");
        assert_ne!(base, ai_cache_key(a, 0, 0.5, 0.5, 0), "a different backend, a different alpha");
        assert_ne!(base, ai_cache_key(a, 2, 0.9, 0.5, 0), "a moved click, a different alpha");
        assert_ne!(base, ai_cache_key(a, 2, 0.5, 0.9, 0), "…in either axis");
        // A plain file name, safe to hand `store::raster_target`.
        assert!(
            base.starts_with("ai-mask-")
                && base[8..].chars().all(|c| c.is_ascii_hexdigit())
                && base.len() == 8 + 16,
            "the key must be a bare, filesystem-safe stem: {base}"
        );
    }

    /// R28 Batch-3 (adjudication F1-A) — THE fixed-point pin.
    ///
    /// A rotate turns the reference point with the frame, so for almost every
    /// click the key moves on its own and the missing frame term stayed
    /// invisible. `(0.5, 0.5)` is the one point all three quarter turns leave
    /// exactly where it is (`render::orient_point`), so a subject clicked at
    /// the frame centre — the default a point-prompted `Mask/Image` lands on —
    /// kept its key across a rotate and the cache served an alpha segmented in
    /// the OLD frame, sideways, reported as an ordinary cache hit.
    ///
    /// MUTATION THIS CATCHES: drop the `t{quarter_turns}` field from
    /// `ai_cache_key`'s identity string and the four keys below collapse into
    /// one.
    #[test]
    fn the_ai_cache_key_separates_the_frames_even_at_the_rotation_fixed_point() {
        let a = Path::new("D:/rolls/2024/DSC0001.ARW");
        // The fixed point of every quarter turn — verified against the
        // coordinate map rather than asserted from memory.
        for t in 1u8..4 {
            let o = crate::render::quarter_turn_orientation(t);
            assert_eq!(crate::render::orient_point(o, 0.5, 0.5), (0.5, 0.5), "{o:?}");
        }
        let keys: Vec<String> = (0u8..4).map(|t| ai_cache_key(a, 2, 0.5, 0.5, t)).collect();
        for i in 0..keys.len() {
            for j in (i + 1)..keys.len() {
                assert_ne!(
                    keys[i], keys[j],
                    "{i} and {j} quarter turns segment different pixels and must not share an alpha"
                );
            }
        }
        // …and the fold matches the render's own `% 4`, so a hand-edited
        // `quarter_turns: 5` cannot mint a second cache entry for one frame.
        assert_eq!(keys[1], ai_cache_key(a, 2, 0.5, 0.5, 5));
    }

    /// R29 B4 — the backend generation is IN the key, and it is 2.
    ///
    /// Two assertions and they catch different mutations. The first pins the
    /// VALUE: the user's ruling five (2026-08-21) swapped the subject backend
    /// from U²-Net to `ZhengPeng7/BiRefNet`, which is exactly the "model re-pin"
    /// [`AI_BACKEND_GENERATION`]'s own doc comment says obliges a bump — and a
    /// swap that shipped without one would serve every v0.34 subject alpha
    /// forever, from a model this build no longer runs, while every surface
    /// reported an ordinary cache hit. The second pins the TERM: drop
    /// `{generation}` from the identity string and the two keys below collapse.
    ///
    /// MUTATIONS THIS CATCHES: `AI_BACKEND_GENERATION = 1` (the swap without
    /// the bump), and `ai_cache_key_gen` ignoring its `generation` argument.
    #[test]
    fn the_backend_generation_is_in_the_key_and_the_birefnet_swap_bumped_it() {
        assert_eq!(
            AI_BACKEND_GENERATION, 2,
            "R29 B4 re-pinned the subject model (BiRefNet), so the generation must be 2"
        );
        let a = Path::new("D:/rolls/2024/DSC0001.ARW");
        let g1 = ai_cache_key_gen(a, 1, 0.5, 0.5, 0, 1);
        let g2 = ai_cache_key_gen(a, 1, 0.5, 0.5, 0, 2);
        assert_ne!(g1, g2, "a re-pinned segmenter must not be served yesterday's alpha");
        // …and the production key IS the generation-2 key, not some third thing.
        assert_eq!(g2, ai_cache_key(a, 1, 0.5, 0.5, 0));
    }

    /// R28 Batch-3 (adjudication F1-D) — two staged frames never share a name.
    ///
    /// LATENT, not live: today's only callers (`apply`, `auto`) run one photo
    /// at a time. The pin is here because the collision is invisible until a
    /// pool calls this, and then it presents as one photo's mask computed on
    /// another photo's pixels.
    ///
    /// MUTATION THIS CATCHES: drop the sequence from `staging_path` and the two
    /// paths below become the same file.
    #[test]
    fn two_staged_source_frames_never_share_a_name() {
        let p = Path::new("D:/rolls/2024/DSC0001.ARW");
        let (a, b) = (staging_path(p), staging_path(p));
        assert_ne!(a, b, "same process, same stem — the names must still differ");
        // Both still name THIS process and THIS photo: the sequence is an
        // addition to the identity, not a replacement for it.
        for path in [&a, &b] {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.starts_with(&format!("autoshop-ai-src-{}-", std::process::id())), "{name}");
            assert!(name.ends_with("-DSC0001.png"), "{name}");
        }
    }

    /// A sidecar that cannot run leaves the mask CARRIED and names the reason.
    /// Never a silent zero mask, never a crash, never a failed develop — the
    /// three failure modes this arm is not allowed to have.
    ///
    /// MUTATION-LINED. Verified red by making `resolve_ai_masks` set
    /// `*raster = Some(target)` regardless of the sidecar's verdict
    /// (transcript in the batch report): the mask then points at a 0-byte
    /// claim file and renders as nothing while the report claims a success.
    #[test]
    fn a_failed_segmentation_leaves_the_ai_mask_carried_and_says_why() {
        let mut cfg = Config::load();
        // A script path that does not exist is the FIRST gate — no python is
        // launched, so this test never depends on the machine's interpreter.
        cfg.segment_script = "no-such-segment-script-for-this-test.py".into();
        let mut r = ai_recipe(ai_geometry(2, 0.5, 0.5));
        let raw = Path::new("D:/rolls/2024/DSC0001.ARW");
        let got = resolve_ai_masks(&cfg, raw, raw, &mut r);
        assert_eq!(got.resolved, 0);
        assert_eq!(got.unresolved.len(), 1, "{got:?}");
        assert_eq!(got.unresolved[0].0, "Mask 1", "the sentence names WHICH mask");
        assert!(
            got.unresolved[0].1.contains("no-such-segment-script-for-this-test.py"),
            "and WHY: {got:?}"
        );
        // The geometry is untouched — carried, exactly as it was imported.
        let crate::recipe::MaskGeometry::AiMask { raster, subtype, .. } = &r.masks[0].mask else {
            panic!("the geometry must survive the failure");
        };
        assert!(raster.is_none(), "no alpha, and no pretence of one");
        assert_eq!(*subtype, 2);
        let line = got.describe().unwrap();
        assert!(line.contains("carried but NOT rendered"), "{line}");
    }

    /// A recipe with no AI mask must not pay a RAW decode — the resolve pass is
    /// lazy in the strong sense, not just cached.
    #[test]
    fn a_recipe_without_an_ai_mask_does_no_work_at_all() {
        let cfg = Config::load();
        let mut r = crate::recipe::EditRecipe {
            masks: vec![crate::recipe::LocalAdjustment {
                mask: crate::recipe::MaskGeometry::Linear {
                    zero_x: 0.5,
                    zero_y: 0.8,
                    full_x: 0.5,
                    full_y: 0.2,
                },
                exposure_ev: 0.5,
                ..Default::default()
            }],
            ..Default::default()
        };
        // The path does not exist: if anything tried to decode it, this would
        // print a warning and still answer "nothing to do".
        let nope = Path::new("D:/nope/nothing-here.ARW");
        let got = resolve_ai_masks(&cfg, nope, nope, &mut r);
        assert_eq!(got, AiMaskResolution::default(), "{got:?}");
        assert!(got.describe().is_none(), "nothing happened, so nothing is claimed");
    }

    /// Tier-3 probe: the REAL SAM 2.1 backend, behind an environment gate like
    /// `AUTOSHOP_RAW_ZOO` / `AUTOSHOP_MB_FIXTURES`. Silent when unset (a bare
    /// `cargo test` must not download 898 MB of weights); LOUD when set and the
    /// backend does not deliver.
    ///
    /// Point `AUTOSHOP_SEG_PROBE` at an image file. It runs the point-prompted
    /// object backend at the frame centre and asserts the contract the render
    /// depends on: an 8-bit grey raster, decodable through the mask budget
    /// gate, with a coverage that is neither empty nor the whole frame.
    #[test]
    fn seg_probe_object_backend_produces_a_usable_soft_mask() {
        let Ok(input) = std::env::var("AUTOSHOP_SEG_PROBE") else { return };
        let input = std::path::PathBuf::from(&input);
        assert!(input.is_file(), "AUTOSHOP_SEG_PROBE is set but is not a file: {}", input.display());
        let cfg = Config::load();
        let opts = SegmentOpts::for_ai_mask(&cfg, 0, 0.5, 0.5)
            .expect("subtype 0 must route to a backend");
        assert!(
            opts.script.exists(),
            "AUTOSHOP_SEG_PROBE is set but the sidecar is not at {} — set AUTOSHOP_SEGMENT_SCRIPT",
            opts.script.display()
        );
        let out = std::env::temp_dir()
            .join(format!("autoshop-seg-probe-{}.png", std::process::id()));
        let _ = std::fs::remove_file(&out);
        segment_file(&opts, &input, &out).expect("the pinned SAM 2.1 backend must produce a mask");
        let img = crate::render::open_mask_bounded(&out)
            .expect("the mask must decode through the budget gate")
            .to_luma8();
        let n = (img.width() as u64) * (img.height() as u64);
        let sum: u64 = img.pixels().map(|p| p.0[0] as u64).sum();
        let coverage = sum as f64 / (n as f64 * 255.0);
        let soft = img.pixels().any(|p| (1..=254).contains(&p.0[0]));
        println!(
            "AUTOSHOP_SEG_PROBE — {} -> {}x{} coverage={coverage:.4} soft={soft}",
            input.display(),
            img.width(),
            img.height()
        );
        assert!(
            (0.0005..0.999).contains(&coverage),
            "a point prompt at the frame centre must select SOMETHING and not everything \
             (coverage {coverage:.4})"
        );
        assert!(img.width().max(img.height()) <= 4096, "the long-edge cap must hold");
        let _ = std::fs::remove_file(&out);
    }

    /// Tier-3 probe, R29 C3/C4: the REAL sky backend end to end, through the
    /// digest gate this batch put in front of it. Same `AUTOSHOP_SEG_PROBE`
    /// gate and the same silence when unset — a bare `cargo test` must not
    /// fetch 881 MB.
    ///
    /// **This is the live fire that proves the gate rather than the model.**
    /// Set `HF_HUB_OFFLINE=1` alongside the probe and it also proves the
    /// negative: with `local_files_only=True` on both halves AND the class
    /// table read from the verified directory, nothing on this path resolves a
    /// remote name. Before this batch the same run died with
    /// `OfflineModeIsEnabled` on
    /// `datasets/shi-labs/oneformer_demo/resolve/main/ade20k_panoptic.json` —
    /// a second, unpinned repository `SKY_REVISION` never covered.
    ///
    /// R29 收口 (ruling 11) then removed that repository rather than pinning
    /// it: the class table is now `python/ade20k_class_table.json`, rebuilt from
    /// the MIT model repo's own label map and thing/stuff split. **No
    /// `AI_BACKEND_GENERATION` bump goes with it** — the two tables were run
    /// against each other on two photographs and the mask PNGs came out
    /// byte-identical, so no cached alpha changes and none needs re-deriving.
    #[test]
    fn seg_probe_sky_backend_produces_a_usable_soft_mask() {
        let Ok(input) = std::env::var("AUTOSHOP_SEG_PROBE") else { return };
        let input = std::path::PathBuf::from(&input);
        assert!(input.is_file(), "AUTOSHOP_SEG_PROBE is set but is not a file: {}", input.display());
        let cfg = Config::load();
        let opts = SegmentOpts::for_ai_mask(&cfg, 2, 0.5, 0.5)
            .expect("subtype 2 must route to a backend");
        assert_eq!(opts.target, "sky", "subtype 2 is Sky");
        assert!(
            opts.script.exists(),
            "AUTOSHOP_SEG_PROBE is set but the sidecar is not at {} — set AUTOSHOP_SEGMENT_SCRIPT",
            opts.script.display()
        );
        let out =
            std::env::temp_dir().join(format!("autoshop-seg-probe-sky-{}.png", std::process::id()));
        let _ = std::fs::remove_file(&out);
        let report = segment_file(&opts, &input, &out).expect("the sky backend must produce a mask");
        assert!(
            report.backend.starts_with("OneFormer ADE20K Swin-L "),
            "the sky run must name the model it used, got {:?}",
            report.backend
        );
        assert_eq!(report.birefnet_deps, None, "sky has no fallback tier to report");
        let img = crate::render::open_mask_bounded(&out)
            .expect("the mask must decode through the budget gate")
            .to_luma8();
        let n = (img.width() as u64) * (img.height() as u64);
        let sum: u64 = img.pixels().map(|p| p.0[0] as u64).sum();
        let coverage = sum as f64 / (n as f64 * 255.0);
        let soft = img.pixels().any(|p| (1..=254).contains(&p.0[0]));
        println!(
            "AUTOSHOP_SEG_PROBE sky — {} -> {}x{} coverage={coverage:.4} soft={soft} [{}]",
            input.display(),
            img.width(),
            img.height(),
            report.backend
        );
        assert!(soft, "the class probability must stay SOFT — the render multiplies it in");
        assert!(img.width().max(img.height()) <= 4096, "the long-edge cap must hold");
        let _ = std::fs::remove_file(&out);
    }

    /// Tier-3 probe, R29 B4: the REAL subject backend end to end through the
    /// bridge. Same gate as the object probe (`AUTOSHOP_SEG_PROBE`), same
    /// silence when it is unset — a bare `cargo test` must not fetch 444 MB.
    ///
    /// This is the arm the batch's live fire ran: point `AUTOSHOP_SEG_PROBE` at
    /// a photograph with a subject in it and `AUTOSHOP_PYTHON` at an
    /// interpreter that has torchvision + timm + einops, and the assertions
    /// below are the contract the render depends on — an 8-bit grey raster,
    /// decodable through the mask budget gate, soft-edged (a hard 0/1 mask
    /// would put a hard edge on every adjustment) and covering neither nothing
    /// nor everything. On an interpreter WITHOUT those packages it still
    /// passes, through the U²-Net fallback tier, and the sidecar's stderr
    /// (forwarded by `segment_file`) is what tells the two runs apart.
    #[test]
    fn seg_probe_subject_backend_produces_a_usable_soft_mask() {
        let Ok(input) = std::env::var("AUTOSHOP_SEG_PROBE") else { return };
        let input = std::path::PathBuf::from(&input);
        assert!(input.is_file(), "AUTOSHOP_SEG_PROBE is set but is not a file: {}", input.display());
        let cfg = Config::load();
        // Through `for_ai_mask`, not a hand-built `SegmentOpts`: the route from
        // `MaskSubType = 1` to `--target subject` is half of what this proves.
        let opts = SegmentOpts::for_ai_mask(&cfg, 1, 0.5, 0.5)
            .expect("subtype 1 must route to a backend");
        assert_eq!(opts.target, "subject", "subtype 1 is Subject");
        assert!(
            opts.script.exists(),
            "AUTOSHOP_SEG_PROBE is set but the sidecar is not at {} — set AUTOSHOP_SEGMENT_SCRIPT",
            opts.script.display()
        );
        let out = std::env::temp_dir()
            .join(format!("autoshop-seg-probe-subject-{}.png", std::process::id()));
        let _ = std::fs::remove_file(&out);
        segment_file(&opts, &input, &out).expect("the subject backend must produce a mask");
        let img = crate::render::open_mask_bounded(&out)
            .expect("the mask must decode through the budget gate")
            .to_luma8();
        let n = (img.width() as u64) * (img.height() as u64);
        let sum: u64 = img.pixels().map(|p| p.0[0] as u64).sum();
        let coverage = sum as f64 / (n as f64 * 255.0);
        let soft = img.pixels().any(|p| (1..=254).contains(&p.0[0]));
        println!(
            "AUTOSHOP_SEG_PROBE subject — {} -> {}x{} coverage={coverage:.4} soft={soft}",
            input.display(),
            img.width(),
            img.height()
        );
        assert!(
            (0.0005..0.999).contains(&coverage),
            "a subject segmentation must select SOMETHING and not everything \
             (coverage {coverage:.4})"
        );
        assert!(soft, "the alpha must be SOFT — the render multiplies it in unthresholded");
        assert!(img.width().max(img.height()) <= 4096, "the long-edge cap must hold");
        let _ = std::fs::remove_file(&out);
    }
}
