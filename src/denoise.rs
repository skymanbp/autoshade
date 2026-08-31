//! AI denoise bridge — Rust side of the SCUNet sidecar (`python/denoise.py`).
//!
//! Like [`crate::generative`] (which shells out to OpenAI), this shells out to a
//! local Python process that runs a real-photo denoiser on the GPU. It is a
//! manual, opt-in pixel pre-process for high-ISO / astro / low-light frames — the
//! AI never decides *edits* here, it only removes sensor noise. The deterministic
//! develop pipeline (tone/colour/sharpen) runs in Rust afterward, so denoise
//! always happens BEFORE sharpening (the order that matters).
//!
//! Two entry points:
//!   * [`denoise_buffer`] — denoise an in-memory full-res RGB buffer (used by the
//!     render engine, so develop happens on already-clean pixels).
//!   * [`denoise_file`]   — denoise an image file in place to another file (used
//!     when the source is an already-baked PNG/TIFF/JPEG).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};
use image::{DynamicImage, GenericImageView, ImageBuffer, Rgb};

use crate::config::Config;

const SIDECAR_DEFAULT_TIMEOUT_SECS: u64 = 30 * 60;
const SIDECAR_OUTPUT_CAP: usize = 1024 * 1024;

/// The sidecar budget is its OWN variable, not `AUTOSHADE_HTTP_TIMEOUT_SECS`:
/// that one tunes API latency, while a sidecar's first run legitimately spends
/// many minutes downloading a model — a user shortening API timeouts must not
/// silently cap the model download at the same number.
pub(crate) fn sidecar_timeout() -> std::time::Duration {
    // env_or_dotenv: .env-set sidecar budgets kept working when the
    // dotenv stopped writing the process environment (L16#3).
    let seconds = crate::config::env_or_dotenv("AUTOSHADE_SIDECAR_TIMEOUT_SECS")
        .and_then(|s| s.parse().ok())
        .filter(|s: &u64| *s > 0)
        .unwrap_or(SIDECAR_DEFAULT_TIMEOUT_SECS);
    std::time::Duration::from_secs(seconds)
}

/// Wait for a piped child without allowing either time or captured bytes to
/// grow without bound (`Command::output()` offered neither — a stalled helper
/// hung the CLI or GUI worker forever). Keeping the TAIL of each stream
/// preserves the traceback line `sidecar_tail` reports.
///
/// `budget_env` names the env var the timeout message tells the user to
/// raise — hard-coding AUTOSHADE_SIDECAR_TIMEOUT_SECS here sent the claude
/// verifier's users to the wrong knob (it reads AUTOSHADE_HTTP_TIMEOUT_SECS).
/// `group` is the child's [`crate::KillGroup`]: on the kill paths the WHOLE
/// tree dies (before the reaping `wait`, the unix pgid rule), and on every
/// path the group drops here — on Windows that close reaps any straggler
/// still holding the pipes after a normal exit.
pub(crate) fn bounded_child_output(
    mut child: std::process::Child,
    who: &str,
    budget: std::time::Duration,
    budget_env: &str,
    group: Option<crate::KillGroup>,
) -> Result<std::process::Output> {
    fn drain<R: std::io::Read + Send + 'static>(
        reader: Option<R>,
    ) -> std::thread::JoinHandle<(Vec<u8>, bool)> {
        std::thread::spawn(move || {
            let Some(mut reader) = reader else {
                return (Vec::new(), false);
            };
            let mut tail = std::collections::VecDeque::with_capacity(SIDECAR_OUTPUT_CAP);
            // 8 KiB reads: each chunk is far below the cap, so the tail logic
            // below never sees a single over-cap read.
            let mut chunk = [0u8; 8192];
            let mut truncated = false;
            loop {
                let count = match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => count,
                };
                let excess =
                    tail.len().saturating_add(count).saturating_sub(SIDECAR_OUTPUT_CAP);
                if excess > 0 {
                    tail.drain(..excess);
                    truncated = true;
                }
                tail.extend(&chunk[..count]);
            }
            (tail.into_iter().collect(), truncated)
        })
    }

    fn mark_truncated(mut bytes: Vec<u8>, truncated: bool) -> Vec<u8> {
        if !truncated {
            return bytes;
        }
        const MARKER: &[u8] = b"[... output exceeded 1 MiB; tail follows ...]\n";
        let trim = bytes
            .len()
            .saturating_add(MARKER.len())
            .saturating_sub(SIDECAR_OUTPUT_CAP);
        if trim > 0 {
            bytes.drain(..trim);
        }
        let mut marked = Vec::with_capacity(MARKER.len() + bytes.len());
        marked.extend_from_slice(MARKER);
        marked.extend_from_slice(&bytes);
        marked
    }

    // Join a drain thread, but never forever: the DIRECT child being gone
    // does not close the pipes — a descendant it spawned (a launcher script's
    // python, a library worker) inherits them and can hold them open
    // indefinitely, and an unconditional join then hung this caller exactly
    // like the un-deadlined child this function exists to bound. Past the
    // grace the thread is DETACHED (it parks in a blocking read and dies
    // with the pipe or the process); its output is forfeited, disclosed as
    // truncated, to keep the deadline honest. std has no join-with-timeout,
    // so this polls is_finished the same way the try_wait loop below polls
    // the child.
    let join_bounded = |handle: std::thread::JoinHandle<(Vec<u8>, bool)>,
                        stream: &str|
     -> (Vec<u8>, bool) {
        let grace = std::time::Duration::from_secs(2);
        let start = std::time::Instant::now();
        while !handle.is_finished() {
            if start.elapsed() >= grace {
                eprintln!(
                    "⚠ a descendant process still holds the sidecar's {stream} after it exited — \
                     abandoning the drain (output truncated)"
                );
                return (Vec::new(), true);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        handle.join().unwrap_or_default()
    };

    let stdout_thread = drain(child.stdout.take());
    let stderr_thread = drain(child.stderr.take());
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() >= budget => {
                // The TREE first (before the reaping `wait` — the unix pgid
                // rule), then the direct child as the belt.
                if let Some(g) = &group {
                    g.kill_tree();
                }
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_bounded(stdout_thread, "stdout");
                let _ = join_bounded(stderr_thread, "stderr");
                bail!(
                    "{who} timed out after {:.1}s and was killed \
                     (raise {budget_env} if your runs are legitimately slower)",
                    budget.as_secs_f32()
                );
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(200)),
            Err(error) => {
                if let Some(g) = &group {
                    g.kill_tree();
                }
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_bounded(stdout_thread, "stdout");
                let _ = join_bounded(stderr_thread, "stderr");
                return Err(error).with_context(|| format!("poll {who}"));
            }
        }
    };

    let (stdout, stdout_truncated) = join_bounded(stdout_thread, "stdout");
    let (stderr, stderr_truncated) = join_bounded(stderr_thread, "stderr");
    Ok(std::process::Output {
        status,
        stdout: mark_truncated(stdout, stdout_truncated),
        stderr: mark_truncated(stderr, stderr_truncated),
    })
}

/// A failed run may remove a fresh file or a zero-byte claim, but must never
/// delete a pre-existing nonempty deliverable the user could still recover.
pub(crate) fn discard_failed_output(
    output: &Path,
    before: Option<(u64, Option<std::time::SystemTime>)>,
) {
    if matches!(before, None | Some((0, _))) {
        let _ = std::fs::remove_file(output);
    }
}

/// The unique staged sibling a denoise run writes before publishing: the
/// promised deliverable name is shared and deterministic
/// (`out/<stem>.denoised.tif`, keyed by stem alone), so the sidecar must
/// never write it directly. The REAL extension stays LAST — the python
/// sidecar picks cv2's encoder from it. Litter, disclosed: a hard kill can
/// orphan one of these beside the deliverable — the same exposure class as
/// the pre-existing `.part` siblings, and no sweeper exists for either.
fn staged_sibling(out: &Path) -> PathBuf {
    let ext = out.extension().and_then(|e| e.to_str()).unwrap_or("png");
    let stem = out.file_stem().and_then(|s| s.to_str()).unwrap_or("denoise");
    out.with_file_name(format!(
        "{stem}.dnstage.{}-{}.{ext}",
        std::process::id(),
        crate::store::next_tmp_seq()
    ))
}

/// Release the caller's 0-byte claim at the promised name — and ONLY a
/// 0-byte claim (the GUI's `unique_out` pre-creates one; failed runs used to
/// eat the 999-name cap). A NONEMPTY file there is a concurrent run's
/// published deliverable and must survive this run's failure — exactly the
/// loss `discard_failed_output` inflicted at the shared name whenever its
/// pre-spawn snapshot predated that publish. The GUI helper's lib-side twin.
///
/// Registered residual (Codex R12 #4): the probe and the remove are two
/// steps, so a concurrent publish renaming onto the name BETWEEN them is
/// still deleted. Closing that fully needs a handle-owned delete
/// disposition, which stable std does not expose on Windows; this window is
/// microseconds where the old shared-name cleanup deleted unconditionally
/// across the whole sidecar runtime.
fn release_empty_claim(p: &Path) {
    if std::fs::metadata(p).is_ok_and(|m| m.len() == 0) {
        let _ = std::fs::remove_file(p);
    }
}

/// Everything the sidecar needs for one run. Built from [`Config`] so the render
/// engine stays decoupled from config/env.
pub struct DenoiseOpts {
    pub python_bin: String,
    pub script: PathBuf,
    pub cache: PathBuf,
    pub model: String,
    /// 0..1 blend with the original (1.0 = full denoise).
    pub strength: f32,
}

impl DenoiseOpts {
    /// `model_override` lets the CLI pick a SCUNet tier; `None` uses the config
    /// default (`color_real_psnr`).
    pub fn from_config(cfg: &Config, model_override: Option<String>, strength: f32) -> Self {
        DenoiseOpts {
            python_bin: cfg.python_bin.clone(),
            script: PathBuf::from(&cfg.denoise_script),
            cache: PathBuf::from(&cfg.denoise_cache),
            model: model_override.unwrap_or_else(|| cfg.denoise_model.clone()),
            // clamp KEEPS NaN — a non-finite strength would ship "--strength
            // NaN" to the sidecar instead of honouring the 0..1 contract.
            strength: if strength.is_finite() { strength.clamp(0.0, 1.0) } else { 1.0 },
        }
    }
}

/// Denoise a full-resolution working buffer (sRGB-ENCODED values, nominally
/// `[0,1]` per channel) in place by round-tripping it through the sidecar as
/// a 16-bit PNG (see `temp_path` for why PNG, not TIFF). Boundary, disclosed:
/// the pack clamps to [0,1], so wide-gamut base colours carried as
/// out-of-range values do not survive an AI-denoise round trip — a denoised
/// export of such a photo clips those colours toward sRGB (recorded with the
/// single-working-space item).
pub fn denoise_buffer(opts: &DenoiseOpts, data: &mut [[f32; 3]], w: usize, h: usize) -> Result<()> {
    if data.len() != w * h {
        bail!("denoise_buffer: buffer {} != {}x{}", data.len(), w, h);
    }
    // Strength 0 is the IDENTITY, not "denoise then blend nothing": the
    // 16-bit round trip below clamps to [0,1] and quantises, so a
    // zero-strength pass changed bytes it promised not to touch — wide-gamut
    // values carried out of range were clipped by a no-op (L09-10).
    if opts.strength <= 0.0 {
        return Ok(());
    }
    let tmp_in = temp_path("autoshade_dn_in")?;
    let tmp_out = match temp_path("autoshade_dn_out") {
        Ok(path) => path,
        Err(error) => {
            let _ = std::fs::remove_file(&tmp_in);
            return Err(error);
        }
    };

    // pack [f32;3] -> 16-bit RGB PNG (see temp_path) — clamps to [0,1]
    let mut buf16: Vec<u16> = Vec::with_capacity(w * h * 3);
    for px in data.iter() {
        buf16.push(to_u16(px[0]));
        buf16.push(to_u16(px[1]));
        buf16.push(to_u16(px[2]));
    }
    let img: ImageBuffer<Rgb<u16>, _> = ImageBuffer::from_raw(w as u32, h as u32, buf16)
        .ok_or_else(|| anyhow!("denoise: pack buffer size mismatch"))?;
    let result = (|| -> Result<DynamicImage> {
        DynamicImage::ImageRgb16(img)
            .save(&tmp_in)
            .with_context(|| format!("write denoise input {}", tmp_in.display()))?;
        run_sidecar(opts, &tmp_in, &tmp_out)?;
        // read 16-bit result back into the buffer. The expected dimensions
        // are already known, so derive real limits from them instead of
        // lifting every limit — a wrong-shaped helper output is rejected by
        // the decoder instead of allocated first (the old `no_limits()` let a
        // lying header drive an aborting allocation before the size check).
        let mut reader = image::ImageReader::open(&tmp_out)
            .with_context(|| format!("open denoise output {}", tmp_out.display()))?;
        reader.limits(denoise_output_limits(w, h));
        reader
            .decode()
            .with_context(|| format!("decode denoise output {}", tmp_out.display()))
    })();
    // BOTH temp files go regardless of WHERE the pipeline failed — a partial
    // input/output from a failed save / sidecar / decode used to leak
    // full-resolution frames into the temp dir on every error path.
    let _ = std::fs::remove_file(&tmp_in);
    let _ = std::fs::remove_file(&tmp_out);
    let out = result?;
    let (ow, oh) = out.dimensions();
    if ow as usize != w || oh as usize != h {
        bail!("denoise changed dimensions: {ow}x{oh} != {w}x{h}");
    }
    let rgb16 = out.to_rgb16();
    for (i, px) in rgb16.pixels().enumerate() {
        data[i] = [
            px[0] as f32 / 65535.0,
            px[1] as f32 / 65535.0,
            px[2] as f32 / 65535.0,
        ];
    }
    Ok(())
}

/// Denoise an image file to another file (the sidecar reads/writes the
/// pixels, preserving bit depth). For already-baked PNG/TIFF/JPEG. The
/// input's ICC profile is carried onto the STAGED product before the one
/// publish (review R12-07) — see `carry_icc_onto_staged`.
pub fn denoise_file(opts: &DenoiseOpts, input: &Path, output: &Path) -> Result<()> {
    crate::pipeline::ensure_parent(output)?;
    run_sidecar_carrying(opts, input, output, true, sidecar_timeout())
}

/// The sidecar copies pixel NUMBERS, not colour metadata (its python side
/// has no ICC handling at all) — so a ProPhoto/AdobeRGB input used to come
/// back UNTAGGED and every downstream reader interpreted the wide-gamut
/// numbers as sRGB (L09-11). Re-encodes the already-ACCEPTED decoded pixels
/// onto the STAGED path with the input's profile attached (review R12-07:
/// post-publish re-tagging could fail and leave an untagged product at the
/// promised name). The numbers are still the input-space numbers, so the
/// input's profile is their correct description. `ext` comes from the
/// PROMISED name — the staging sibling's suffix is machinery.
fn carry_icc_onto_staged(
    input: &Path,
    staged: &Path,
    output: &Path,
    img: &image::DynamicImage,
) -> Result<()> {
    use image::ImageEncoder as _;
    let profile = {
        let mut dec = image::ImageReader::open(input)
            .with_context(|| format!("open {}", input.display()))?
            .into_decoder()
            .with_context(|| format!("read image header {}", input.display()))?;
        image::ImageDecoder::icc_profile(&mut dec)
            .with_context(|| format!("read the colour profile of {}", input.display()))?
    };
    let Some(profile) = profile else { return Ok(()) };
    let ext = output.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "tif" | "tiff") {
        eprintln!(
            "⚠ the denoise product .{ext} cannot carry the source ICC profile — its colours \
             may be read as sRGB"
        );
        return Ok(());
    }
    let file = std::fs::File::create(staged)
        .with_context(|| format!("re-create {}", staged.display()))?;
    let mut w = std::io::BufWriter::new(file);
    match ext.as_str() {
        "png" => {
            let mut enc = image::codecs::png::PngEncoder::new(&mut w);
            let _ = enc.set_icc_profile(profile.clone());
            enc.write_image(img.as_bytes(), img.width(), img.height(), img.color().into())
                .context("re-encode the denoise product with the source profile")?;
        }
        "jpg" | "jpeg" => {
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, 95);
            let _ = enc.set_icc_profile(profile.clone());
            enc.write_image(img.as_bytes(), img.width(), img.height(), img.color().into())
                .context("re-encode the denoise product with the source profile")?;
        }
        _ => {
            let mut enc = image::codecs::tiff::TiffEncoder::new(&mut w);
            let _ = enc.set_icc_profile(profile.clone());
            enc.write_image(img.as_bytes(), img.width(), img.height(), img.color().into())
                .context("re-encode the denoise product with the source profile")?;
        }
    }
    use std::io::Write as _;
    w.flush().context("flush the re-encoded product")
}

/// Denoise the ACTIVE working pixels for an on-canvas result (the GUI's
/// interactive "AI Denoise now"): a RAW goes through a neutral develop first —
/// the full sensor with `full_res`, else a ≤2048 px working copy (the same
/// base contract as retouch: never the camera's baked preview, so the develop
/// chain keeps running on the engine's own tone pipeline) — a baked PNG/TIFF
/// is denoised as-is. The caller bakes the output into the current variant,
/// exactly like a heal result.
pub fn denoise_active(
    opts: &DenoiseOpts,
    input: &Path,
    full_res: bool,
    out: &Path,
) -> Result<()> {
    if crate::decode::is_raw(input) && full_res {
        // The only arm that needs no pixels in memory: the full-res develop
        // denoises INSIDE the engine and writes `out` itself (16-bit).
        crate::render::render_to_file(
            input,
            &crate::recipe::EditRecipe::default(),
            out,
            Some(opts),
            None,
            // The shipped channel (R29-1): this is the GUI's interactive
            // "denoise now", one photo at a time on a worker thread of its
            // own — there is no pooled transcript for it to be ordered into.
            crate::diag::stderr(),
        )?;
        return Ok(());
    }
    // Everything else takes the ONE source dispatch (`render::source_pixels`):
    // a RAW working copy is developed AT ≤2048 (not full-res then thumbnailed
    // — the cap runs before tone/geometry, skipping the 61 MP transients), and
    // a baked source is decoded rather than handed straight to the sidecar
    // because cv2 ignores EXIF orientation (and imwrite drops the tag), so a
    // portrait phone JPEG came back as a permanently sideways master.
    let img = crate::render::source_pixels(input, (!full_res).then_some(2048))?;
    let tmp = temp_path("autoshade_denoise_base")?;
    if let Err(e) = img.save(&tmp) {
        // A failed save can still have created a partial file — don't leak it
        // into the temp dir.
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("write denoise input {}", tmp.display()));
    }
    let res = denoise_file(opts, &tmp, out);
    let _ = std::fs::remove_file(&tmp);
    res
}

fn run_sidecar(opts: &DenoiseOpts, input: &Path, output: &Path) -> Result<()> {
    run_sidecar_carrying(opts, input, output, false, sidecar_timeout())
}

fn run_sidecar_carrying(
    opts: &DenoiseOpts,
    input: &Path,
    output: &Path,
    carry_icc: bool,
    budget: std::time::Duration,
) -> Result<()> {
    if !opts.script.exists() {
        bail!(
            "denoise sidecar not found at {} — run from the project dir or set \
             AUTOSHADE_DENOISE_SCRIPT.",
            opts.script.display()
        );
    }
    // The sidecar writes a STAGED sibling, never the promised name: that
    // name is shared and deterministic, so two concurrent runs used to
    // interleave into ONE file — and a failing run's cleanup deleted the
    // deliverable a concurrent run had JUST published (the failer's
    // pre-spawn snapshot predated the publish, so it read as removable).
    // The snapshot now guards the staged path — fresh by construction, so
    // the exit-0-no-write refusal keeps its full strength.
    let staged = staged_sibling(output);
    let before = crate::artifact_state(&staged);
    let mut cmd = Command::new(&opts.python_bin);
    // What a `.env` may push at this child is an ALLOWLIST, not "everything
    // the capability table did not refuse" — see `config::dotenv_child_env`.
    // Compute knobs (CUDA_VISIBLE_DEVICES, thread counts) pass; anything that
    // names a path, a host, a credential or a library to load does not. That
    // is what stops a photo pack's `.env` from handing this process
    // LD_PRELOAD, which ld.so would honour BEFORE the `-E` below — `-E` only
    // filters PYTHON*, so it was never the guard for that class. The user's
    // OWN environment still reaches the child: nothing calls env_clear.
    cmd.envs(crate::config::dotenv_child_env());
    cmd.envs(crate::config::Config::sidecar_child_env());
    // `-E`: ignore PYTHON* environment variables — a cwd .env's
    // `PYTHONPATH=.` beside a hostile `numpy.py` is code execution at
    // import time (config.rs also protects those vars; two layers).
    cmd.arg("-E")
        .arg(&opts.script)
        .arg("--input")
        .arg(input)
        .arg("--output")
        .arg(&staged)
        .arg("--model")
        .arg(&opts.model)
        .arg("--strength")
        .arg(format!("{:.4}", opts.strength))
        .arg("--cache")
        .arg(&opts.cache)
        // CAPTURE, never inherit (same reason as the segmentation sidecar): the
        // release GUI has no console, so inherited handles silently discard the
        // sidecar's reason for failing. Cost of the capture: the CLI no longer
        // sees the live model-download progress, only the tail on failure.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Don't flash a console window when the windowed GUI spawns the sidecar.
    crate::hide_child_console(&mut cmd);
    // The whole process TREE dies with the budget, not just the launcher
    // (L11#7): torch workers survived the old direct-child kill.
    crate::arm_kill_group(&mut cmd);
    let run = (|| -> Result<std::process::Output> {
        let child = cmd.spawn().with_context(|| {
            format!(
                "launch denoise sidecar ({} {}) — is Python on PATH / AUTOSHADE_PYTHON set?",
                opts.python_bin,
                opts.script.display()
            )
        })?;
        let group = crate::assign_kill_group(&child);
        bounded_child_output(
            child,
            "denoise sidecar",
            budget,
            "AUTOSHADE_SIDECAR_TIMEOUT_SECS",
            group,
        )
    })();
    let out = match run {
        Ok(out) => out,
        Err(error) => {
            // A killed/failed run cleans its OWN staging and releases the
            // caller's 0-byte claim at the promised name — never a nonempty
            // file there, which is a concurrent run's published deliverable
            // now that the sidecar cannot write the shared name.
            discard_failed_output(&staged, before);
            release_empty_claim(output);
            return Err(error);
        }
    };
    if !out.status.success() {
        discard_failed_output(&staged, before);
        release_empty_claim(output);
        bail!(
            "denoise sidecar exited with {}: {}",
            out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
            crate::sidecar_tail(&out.stderr, &out.stdout)
        );
    }
    // Exit 0 alone is NOT success: THIS run must have produced the artifact
    // (`crate::sidecar_wrote`). Without the pre-spawn snapshot, a sidecar
    // that wrote nothing still "succeeded" whenever the deterministic
    // deliverable name already held an EARLIER export — the check passed on
    // the stale file and the CLI presented last week's pixels as this run's.
    let wrote = crate::sidecar_wrote("denoise sidecar", &staged, before);
    if let Err(e) = wrote {
        discard_failed_output(&staged, before);
        release_empty_claim(output);
        return Err(e);
    }
    // A written artifact is still not a DELIVERABLE (L09-12, tightened by
    // review R12-07): the product must FULLY DECODE (a truncated IDAT walks
    // straight past a header check), match the input's dimensions, keep its
    // CHANNEL COUNT (RGB16-for-RGBA16 silently destroys alpha) and its
    // per-channel bit depth. The decoded pixels are reused for the ICC
    // carriage below, so the full decode is not a second pass.
    let accepted = (|| -> Result<image::DynamicImage> {
        let (iw, ih) = image::image_dimensions(input)
            .with_context(|| format!("read dimensions of {}", input.display()))?;
        let img =
            image::open(&staged).context("the denoise product does not fully decode")?;
        if (img.width(), img.height()) != (iw, ih) {
            bail!(
                "the denoise product is {}x{} but the input is {iw}x{ih}",
                img.width(),
                img.height()
            );
        }
        let in_color = {
            let dec = image::ImageReader::open(input)?.into_decoder()?;
            image::ImageDecoder::color_type(&dec)
        };
        let out_color = img.color();
        if out_color.channel_count() < in_color.channel_count() {
            bail!(
                "the denoise product has {} channel(s) where the input has {} — dropped \
                 channels (alpha) must not be published",
                out_color.channel_count(),
                in_color.channel_count()
            );
        }
        let per = |c: image::ColorType| c.bits_per_pixel() / u16::from(c.channel_count());
        if per(out_color) < per(in_color) {
            bail!(
                "the denoise product dropped to {}-bit channels from the input's {}-bit — \
                 the sidecar contract preserves bit depth",
                per(out_color),
                per(in_color)
            );
        }
        Ok(img)
    })();
    let decoded = match accepted {
        Ok(img) => img,
        Err(e) => {
            discard_failed_output(&staged, before);
            release_empty_claim(output);
            return Err(e.context("denoise sidecar product rejected"));
        }
    };
    // ICC carriage happens on the STAGED file, BEFORE the one publish
    // (review R12-07): the old post-publish re-encode could fail and leave
    // an untagged product at the promised name behind an error return.
    if carry_icc
        && let Err(e) = carry_icc_onto_staged(input, &staged, output, &decoded)
    {
        discard_failed_output(&staged, before);
        release_empty_claim(output);
        return Err(e.context("denoise product could not carry the source colour profile"));
    }
    drop(decoded);
    // Publish: ONE rename moves the finished bytes onto the promised name,
    // replacing the caller's 0-byte claim (or an older deliverable) in a
    // single step — concurrent runs can no longer interleave into the
    // shared name, and this run's failure paths never touch it.
    // durable_replace (L03): the staged result is fsynced before the
    // rename and the dir entry after — a denoise master referenced by a
    // durably-saved develop must survive the same power cut the JSON does.
    if let Err(e) = crate::store::durable_replace(&staged, output) {
        // The staged result SURVIVES a failed publish and the error names
        // it (Codex R12 #5): on Windows a viewer holding the destination
        // open without delete sharing fails this rename, and deleting the
        // staging here turned that transient conflict into total loss of a
        // minutes-long GPU result.
        release_empty_claim(output);
        return Err(e).with_context(|| {
            format!(
                "publish denoise output {} — the finished result is kept at {}",
                output.display(),
                staged.display()
            )
        });
    }
    Ok(())
}

fn temp_path(tag: &str) -> Result<PathBuf> {
    for _ in 0..1024 {
        let mut path = std::env::temp_dir();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        // 16-bit PNG interchange: unambiguous for both cv2 and the image crate
        // (no TIFF predictor-tag mismatch), still lossless and full bit depth.
        // `create_new` CLAIMS the name atomically — a predictable PID+counter
        // path in the shared temp dir could be pre-created by another local
        // account (and a crash-orphaned twin from a recycled PID could be
        // read back as this photo's result).
        path.push(format!(
            "{tag}_{}_{}_{}.png",
            std::process::id(),
            stamp,
            unique()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("claim denoise temp {}", path.display()));
            }
        }
    }
    bail!("could not claim a unique denoise temporary file after 1024 attempts")
}

/// Monotonic-ish suffix so two buffers in one process don't collide (no RNG dep).
fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

/// Decoder limits derived from the frame we EXPECT back — a wrong-shaped or
/// lying-header helper output is refused by the decoder instead of allocated.
fn denoise_output_limits(w: usize, h: usize) -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(u32::try_from(w).unwrap_or(u32::MAX));
    limits.max_image_height = Some(u32::try_from(h).unwrap_or(u32::MAX));
    let pixels = u64::try_from(w)
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::try_from(h).unwrap_or(u64::MAX));
    // Six bytes hold RGB16; the remaining allowance covers decoder staging
    // without returning to the old unlimited allocation policy.
    limits.max_alloc = Some(
        pixels
            .saturating_mul(16)
            .saturating_add(16 * 1024 * 1024),
    );
    limits
}

fn to_u16(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * 65535.0).round() as u16
}

#[cfg(test)]
mod tests {

    fn write_png(path: &std::path::Path, w: u32, h: u32) {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb([90, 90, 90])))
            .save(path)
            .unwrap();
    }

    /// A stand-in "sidecar" that copies a prepared product onto the staged
    /// path (arg %6 / $6 — python_bin is invoked as
    /// `<bin> -E <script> --input <in> --output <staged> …`).
    fn copying_stand_in(dir: &std::path::Path, product: &str) -> std::path::PathBuf {
        #[cfg(windows)]
        {
            let p = dir.join("copy.bat");
            std::fs::write(
                &p,
                format!("@copy /y \"%~dp0{product}\" \"%~6\" >nul\r\n@exit /b 0\r\n"),
            )
            .unwrap();
            p
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let p = dir.join("copy.sh");
            std::fs::write(
                &p,
                format!("#!/bin/sh\ncp \"$(dirname \"$0\")/{product}\" \"$6\"\nexit 0\n"),
            )
            .unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        }
    }

    fn stand_in_opts(dir: &std::path::Path, bin: std::path::PathBuf) -> DenoiseOpts {
        let script = dir.join("denoise.py");
        std::fs::write(&script, "# stand-in\n").unwrap();
        DenoiseOpts {
            python_bin: bin.to_string_lossy().into_owned(),
            script,
            cache: dir.join("cache"),
            model: "stand-in".into(),
            strength: 1.0,
        }
    }

    /// R12-07: a product that HEADER-parses but cannot fully decode (a
    /// truncated IDAT) must be refused — header checks alone published it.
    #[test]
    fn a_truncated_denoise_product_is_refused() {
        let dir = std::env::temp_dir()
            .join(format!("autoshade-denoise-trunc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A real 8×8 PNG, cut 20 bytes short: the header (and its declared
        // dimensions) survive, the pixel data does not.
        let whole = dir.join("whole.png");
        write_png(&whole, 8, 8);
        let bytes = std::fs::read(&whole).unwrap();
        std::fs::write(dir.join("product.png"), &bytes[..bytes.len() - 20]).unwrap();
        let opts = stand_in_opts(&dir, copying_stand_in(&dir, "product.png"));
        let input = dir.join("in.png");
        write_png(&input, 8, 8);
        let output = dir.join("out.png");

        let err = denoise_file(&opts, &input, &output).unwrap_err();
        assert!(
            format!("{err:#}").contains("fully decode"),
            "the acceptance decodes the whole product: {err:#}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R12-07: a product that silently DROPS channels (RGB for an RGBA
    /// input destroys alpha) must be refused — bits-per-channel alone
    /// compared RGB16 and RGBA16 as equal.
    #[test]
    fn a_channel_dropping_denoise_product_is_refused() {
        let dir = crate::test_dir("denoise-chan");
        write_png(&dir.join("product.png"), 8, 8); // RGB product
        let opts = stand_in_opts(&dir, copying_stand_in(&dir, "product.png"));
        let input = dir.join("in.png"); // RGBA input
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([90, 90, 90, 128]),
        ))
        .save(&input)
        .unwrap();
        let output = dir.join("out.png");

        let err = denoise_file(&opts, &input, &output).unwrap_err();
        assert!(
            format!("{err:#}").contains("channel"),
            "dropped alpha is refused: {err:#}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L09-10: strength 0 is byte-identical identity — no clamp, no
    /// quantisation, no sidecar (the opts point at nothing runnable, which
    /// would fail loudly if the pipeline ran).
    #[test]
    fn zero_strength_denoise_is_the_identity() {
        let opts = DenoiseOpts {
            python_bin: "definitely-not-a-real-python".into(),
            script: std::path::PathBuf::from("no-such-script.py"),
            cache: std::path::PathBuf::new(),
            model: "m".into(),
            strength: 0.0,
        };
        let mut data = [[1.2f32, 0.5, -0.1]];
        denoise_buffer(&opts, &mut data, 1, 1).expect("identity needs no sidecar");
        assert_eq!(
            data,
            [[1.2f32, 0.5, -0.1]],
            "out-of-range values survive untouched — nothing was clamped or quantised"
        );
    }

    /// L09-12: a product whose dimensions differ from the input is refused —
    /// a 1×1 PNG must never be published as this photo's master.
    #[test]
    fn a_wrong_size_denoise_product_is_refused() {
        let dir = crate::test_dir("denoise-accept");
        write_png(&dir.join("product.png"), 1, 1);
        let opts = stand_in_opts(&dir, copying_stand_in(&dir, "product.png"));
        let input = dir.join("in.png");
        write_png(&input, 8, 8);
        let output = dir.join("out.png");

        let err = denoise_file(&opts, &input, &output).unwrap_err();
        assert!(
            format!("{err:#}").contains("but the input is 8x8"),
            "the acceptance names the mismatch: {err:#}"
        );
        assert!(
            !output.exists() || std::fs::metadata(&output).unwrap().len() == 0,
            "the wrong-size product was not published"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L09-11: the input's ICC profile rides onto the product — the sidecar
    /// copies input-space numbers, and untagged wide-gamut numbers read as
    /// sRGB everywhere downstream.
    #[test]
    fn the_denoise_product_carries_the_inputs_icc_profile() {
        use image::ImageEncoder as _;
        let dir =
            std::env::temp_dir().join(format!("autoshade-denoise-icc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // The stand-in's product: same dims, NO profile (the sidecar's own
        // output never carries one).
        write_png(&dir.join("product.png"), 8, 8);
        let opts = stand_in_opts(&dir, copying_stand_in(&dir, "product.png"));

        // The input carries a (dummy but structurally present) ICC profile.
        let profile = b"fake-icc-profile-bytes".to_vec();
        let input = dir.join("in.png");
        {
            let file = std::fs::File::create(&input).unwrap();
            let mut enc = image::codecs::png::PngEncoder::new(std::io::BufWriter::new(file));
            enc.set_icc_profile(profile.clone()).unwrap();
            let img = image::RgbImage::from_pixel(8, 8, image::Rgb([90, 90, 90]));
            enc.write_image(img.as_raw(), 8, 8, image::ExtendedColorType::Rgb8).unwrap();
        }
        let output = dir.join("out.png");
        denoise_file(&opts, &input, &output).expect("the stand-in product is accepted");

        let mut dec = image::ImageReader::open(&output)
            .unwrap()
            .into_decoder()
            .unwrap();
        let carried = image::ImageDecoder::icc_profile(&mut dec).unwrap();
        assert_eq!(
            carried.as_deref(),
            Some(profile.as_slice()),
            "the product now carries the input's profile"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    use super::*;

    /// A unique-per-test fixture dir — `crate::test_dir` carries the
    /// concurrent-test-process rationale.
    fn tdir(tag: &str) -> PathBuf {
        crate::test_dir(&format!("dn-test-{tag}"))
    }

    /// A stand-in "python" that exits 0 and, if `writes`, writes the sidecar's
    /// `--output` argument (argv position 6 — position 1 is the `-E`
    /// isolation flag the real spawn passes). Platform ceremony lives in
    /// `crate::write_stand_in` since step 7a.
    fn stand_in(dir: &Path, writes: bool) -> String {
        if writes {
            crate::write_stand_in(
                dir,
                "writing",
                "@echo denoised-bytes>\"%~6\"\r\n@exit /b 0\r\n",
                "printf denoised-bytes > \"$6\"\nexit 0\n",
            )
        } else {
            crate::write_stand_in(dir, "noop", "@exit /b 0\r\n", "exit 0\n")
        }
    }

    /// A stand-in that models a CONCURRENT publisher: it writes fixed
    /// bytes to the FINAL deliverable name (ignoring the staged `--output`
    /// it was handed) and exits with `code`.
    fn stand_in_publishing(dir: &Path, target: &Path, code: i32) -> String {
        crate::write_stand_in(
            dir,
            &format!("publish{code}"),
            &format!(
                "@echo another runs pixels>\"{}\"\r\n@exit /b {code}\r\n",
                target.display()
            ),
            &format!("printf 'another runs pixels' > \"{}\"\nexit {code}\n", target.display()),
        )
    }

    fn opts_for(dir: &Path, writes: bool) -> DenoiseOpts {
        // The script must merely exist — run_sidecar refuses a missing one
        // before it ever spawns.
        let script = dir.join("denoise.py");
        std::fs::write(&script, "# stand-in\n").unwrap();
        DenoiseOpts {
            python_bin: stand_in(dir, writes),
            script,
            cache: dir.join("cache"),
            model: "color_real_psnr".into(),
            strength: 1.0,
        }
    }

    /// M-D1: the `crate::sidecar_wrote` call removed from `run_sidecar` — a
    /// sidecar that exits 0 WITHOUT writing must be an error, not
    /// "denoised -> <a file that does not exist>". (Found live by an E2E
    /// canary: the CLI printed success for a file that was never written.)
    #[test]
    fn a_sidecar_that_exits_zero_without_writing_is_refused() {
        let dir = tdir("noop");
        let opts = opts_for(&dir, false);
        let input = dir.join("in.png");
        std::fs::write(&input, b"not-really-a-png").unwrap();

        let err = run_sidecar(&opts, &input, &dir.join("out.png")).unwrap_err().to_string();
        assert!(err.contains("exited 0 but wrote no output"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The MISSING-sidecar arm, pinned as a returned `Err` (R24 batch 2).
    ///
    /// The round's material gathering reported this arm printing `Error: …`
    /// while the process exited 0 — an exit code scripts and CI read as
    /// success. It does not reproduce: `denoise_cmd` propagates both arms with
    /// `?`, `main` returns `anyhow::Result`, and a live run refuses with exit 1
    /// on a baked input and on a RAW, in bash and in PowerShell alike. What
    /// there WAS no coverage for is the property that makes that true, and the
    /// one historical instance of this class in this codebase (`batch_cmd`
    /// swallowing every per-photo failure into exit 0, 16-lane scan L09) was a
    /// missing test, not a missing `?`. So the property is a test now: the
    /// refusal must arrive as `Err` at the PUBLIC entry the CLI calls, not as
    /// an `eprintln` plus `Ok(())` further in.
    #[test]
    fn a_missing_sidecar_script_is_an_error_not_a_printed_warning() {
        let dir = tdir("nosidecar");
        let mut opts = opts_for(&dir, true);
        opts.script = dir.join("this-script-does-not-exist.py");
        let input = dir.join("in.png");
        write_png(&input, 8, 8);
        let out = dir.join("out.png");

        // `denoise_active` is what `main::denoise_cmd`'s baked arm calls…
        let err = denoise_active(&opts, &input, true, &out).unwrap_err().to_string();
        assert!(err.contains("denoise sidecar not found"), "{err}");
        // …and no deliverable may be left behind claiming otherwise.
        assert!(!out.exists(), "a refused denoise must not publish an output");
        // The RAW arm reaches the same refusal through `render_to_file`, so
        // the arm below it is pinned directly rather than through a sensor
        // fixture this repo does not carry.
        assert!(denoise_file(&opts, &input, &out).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// M-D4: the check replaced by an unconditional refusal (or pointed at
    /// the wrong path) — a sidecar that DOES write its output must succeed.
    /// Without this positive control the negative tests pass under a mutant
    /// that fails every real denoise.
    #[test]
    fn a_sidecar_that_writes_its_output_succeeds() {
        let dir = tdir("writes");
        // Real images since L09-12: the acceptance now verifies the product
        // decodes and matches the input's dimensions, so the old junk-bytes
        // fixture would (rightly) be refused.
        write_png(&dir.join("product.png"), 8, 8);
        let opts = stand_in_opts(&dir, copying_stand_in(&dir, "product.png"));
        let input = dir.join("in.png");
        write_png(&input, 8, 8);
        let output = dir.join("out.png");

        run_sidecar(&opts, &input, &output).unwrap();
        assert!(std::fs::metadata(&output).unwrap().len() > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// M-D3: the pre-spawn snapshot dropped — the CLI's deliverable names are
    /// deterministic (`out/<stem>.denoised.tif`), so an EARLIER export at the
    /// path made the plain existence check "succeed" while the sidecar wrote
    /// nothing, presenting last run's pixels as this run's result.
    #[test]
    fn a_stale_deliverable_cannot_stand_in_for_a_missing_write() {
        let dir = tdir("stale");
        let opts = opts_for(&dir, false);
        let input = dir.join("in.png");
        std::fs::write(&input, b"not-really-a-png").unwrap();
        let output = dir.join("out.png");
        std::fs::write(&output, b"an earlier successful export").unwrap();

        let err = run_sidecar(&opts, &input, &output).unwrap_err().to_string();
        // Staged now: a missing write is refused as "no output" (the staged
        // sibling is fresh by construction) — and the stale deliverable
        // SURVIVES, where the old direct-write cleanup could delete it.
        assert!(err.contains("exited 0 but wrote no output"), "{err}");
        assert!(
            std::fs::read(&output).unwrap().starts_with(b"an earlier successful export"),
            "a failed run must not touch the promised name"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L01/L10: the promised deliverable name is shared — a concurrent run's
    /// just-published file used to be DELETED by this run's failure cleanup
    /// (the failer's pre-spawn snapshot predated the publish, so it read as
    /// removable), and an exit-0-no-write sidecar had the other run's bytes
    /// accepted as its own result. The "concurrent publisher" is modelled by
    /// a stand-in that writes the FINAL name and fails — no threads, no
    /// timing luck.
    #[test]
    fn another_runs_published_deliverable_survives_this_runs_failure() {
        let dir = tdir("crossrun");
        let output = dir.join("out.png");
        let input = dir.join("in.png");
        std::fs::write(&input, b"not-really-a-png").unwrap();

        let mut opts = opts_for(&dir, false);
        opts.python_bin = stand_in_publishing(&dir, &output, 1);
        assert!(run_sidecar(&opts, &input, &output).is_err());
        assert!(
            std::fs::read(&output).unwrap().starts_with(b"another runs pixels"),
            "a failing run deleted a concurrent run's published deliverable"
        );

        // The exit-0 variant: the other run's bytes must not be presented
        // as THIS run's result — and they still survive the refusal.
        opts.python_bin = stand_in_publishing(&dir, &output, 0);
        let err = run_sidecar(&opts, &input, &output).unwrap_err().to_string();
        assert!(err.contains("exited 0 but wrote no output"), "{err}");
        assert!(std::fs::read(&output).unwrap().starts_with(b"another runs pixels"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// M-D2 at the integration layer: the GUI claims its output name first
    /// (`unique_out` creates a 0-byte file), so the arm a GUI user actually
    /// hits is "exists but empty" — it must refuse too.
    #[test]
    fn a_preclaimed_empty_output_the_sidecar_never_fills_is_refused() {
        let dir = tdir("claimed");
        let opts = opts_for(&dir, false);
        let input = dir.join("in.png");
        std::fs::write(&input, b"not-really-a-png").unwrap();
        let output = dir.join("out.png");
        std::fs::write(&output, b"").unwrap(); // the claim file

        let err = run_sidecar(&opts, &input, &output).unwrap_err().to_string();
        // Staged now: refused as "no output" (the fresh staged sibling was
        // never written), and the 0-byte claim is RELEASED so failed runs
        // stop eating the 999-name cap.
        assert!(err.contains("exited 0 but wrote no output"), "{err}");
        assert!(!output.exists(), "the empty claim must be released on failure");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stalled_sidecar_is_killed_and_its_claim_is_released() {
        let dir = tdir("timeout");
        #[cfg(windows)]
        let stalled = {
            let path = dir.join("stalled.bat");
            std::fs::write(&path, "@echo off\r\n:again\r\ngoto again\r\n").unwrap();
            path.to_string_lossy().into_owned()
        };
        #[cfg(not(windows))]
        let stalled = {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join("stalled.sh");
            std::fs::write(&path, "#!/bin/sh\nwhile :; do :; done\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.to_string_lossy().into_owned()
        };

        let mut opts = opts_for(&dir, false);
        opts.python_bin = stalled;
        let input = dir.join("in.png");
        std::fs::write(&input, b"not-really-a-png").unwrap();
        let output = dir.join("out.png");
        std::fs::write(&output, b"").unwrap();

        let started = std::time::Instant::now();
        let error = run_sidecar_carrying(
            &opts,
            &input,
            &output,
            false,
            std::time::Duration::from_millis(100),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(
            !output.exists(),
            "a killed run must release its unreferenced output claim"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn denoise_decode_limits_are_derived_from_the_expected_frame() {
        let limits = denoise_output_limits(320, 200);
        assert_eq!(limits.max_image_width, Some(320));
        assert_eq!(limits.max_image_height, Some(200));
        assert_eq!(
            limits.max_alloc,
            Some(320u64 * 200 * 16 + 16 * 1024 * 1024)
        );
    }

    #[test]
    fn denoise_temp_paths_are_atomically_claimed() {
        let path = temp_path("autoshade-dn-test-claim").unwrap();
        assert!(path.exists(), "temp_path must return an already-owned claim");
        let error = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        let _ = std::fs::remove_file(path);
    }

    // ── L11 sidecar-hardening source invariants ─────────────────────────────
    // The fixes live in python/denoise.py and this repo has no Python test
    // runner, so the gate is the established source-invariant idiom
    // (include_str! + non-vacuity assertions, as advisor/mod.rs:1344 does for
    // its own file): a regression edit to the sidecar fails a Rust test.
    const SIDECAR_SRC: &str =
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/python/denoise.py"));

    /// Every dict key on the same line as an entry of the named table, e.g.
    /// `"color_25": …` between `NAME = {` and the closing `}`.
    fn py_table_keys(name: &str) -> Vec<String> {
        let start = SIDECAR_SRC
            .find(&format!("{name} = {{"))
            .unwrap_or_else(|| panic!("{name} table missing"));
        let body = &SIDECAR_SRC[start..];
        // The closing brace at line start — an f-string VALUE's own `}`
        // (WEIGHT_URLS embeds {_BASE}) must not truncate the table.
        let end = body.find("\n}").expect("table closes");
        body[..end]
            .lines()
            .filter_map(|l| {
                let l = l.trim();
                let key = l.strip_prefix('"')?;
                Some(key[..key.find('"')?].to_string())
            })
            .collect()
    }

    /// L11#3: every downloadable weight is SHA-256 pinned AND byte-capped —
    /// a partial pin would leave live unpinned download paths reachable from
    /// the --model flag.
    #[test]
    fn scunet_weight_downloads_are_all_sha256_pinned() {
        let urls = py_table_keys("WEIGHT_URLS");
        let pins = py_table_keys("WEIGHT_SHA256");
        let sizes = py_table_keys("WEIGHT_BYTES");
        assert!(urls.len() >= 5, "extractor non-vacuity: {urls:?}");
        assert_eq!(urls, pins, "every URL key has a digest, none extra");
        assert_eq!(urls, sizes, "every URL key has a byte cap, none extra");
        // Every VALUE in the digest table is 64 lowercase hex chars —
        // validated per entry, not filtered by length (a filter let a
        // 63-char typo pass silently; Codex L11-4).
        let start = SIDECAR_SRC.find("WEIGHT_SHA256 = {").unwrap();
        let body = &SIDECAR_SRC[start..];
        let body = &body[..body.find("\n}").unwrap()];
        let mut digests = 0;
        for line in body.lines() {
            let t = line.trim();
            if t.starts_with('"')
                && let Some(v) = t.rsplit('"').nth(1)
                && !pins.iter().any(|k| k == v)
            {
                digests += 1;
                assert_eq!(v.len(), 64, "digest wrong length: {v}");
                assert!(
                    v.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                    "digest not lowercase hex: {v}"
                );
            }
        }
        assert_eq!(digests, pins.len(), "one validated digest per pinned model");
    }

    /// L11#3/#5: no weight ever goes through the raw downloader — the
    /// verified fetch (which also re-hashes an EXISTING cache) is the only
    /// channel, and it carries the byte cap.
    #[test]
    fn the_denoise_sidecar_never_downloads_a_weight_unverified() {
        assert!(
            !SIDECAR_SRC.contains("_download(WEIGHT_URLS["),
            "a weight download bypasses the verified fetch"
        );
        assert!(
            SIDECAR_SRC.matches("_fetch_verified(").count() >= 3,
            "def + the network file + the weights"
        );
        assert!(
            SIDECAR_SRC.contains("def _download(url, dest, max_bytes):"),
            "the downloader takes its cap"
        );
        assert!(
            SIDECAR_SRC.contains("done > max_bytes"),
            "the cap is enforced in-stream"
        );
    }

    /// L11#5: a server that closes early must not publish a SHORT file onto
    /// the cache name — the refusal sits textually BEFORE the publish.
    #[test]
    fn a_truncated_weight_download_is_never_published() {
        let refusal = SIDECAR_SRC
            .find("done != total")
            .expect("the truncation refusal exists");
        let publish = SIDECAR_SRC
            .find("os.replace(tmp, dest)")
            .expect("the publish exists");
        assert!(refusal < publish, "the refusal must guard the publish");
    }

    /// L11#5: day-old orphaned `.part` files (a hard kill skips the python
    /// `finally`) are reclaimed — on the dest prefix ONLY, so the sweep can
    /// never touch the image-output .part names in the user's out/ dir.
    #[test]
    fn orphaned_download_parts_are_reclaimed() {
        assert!(SIDECAR_SRC.contains("def _reclaim_stale_parts(dest):"));
        assert!(
            SIDECAR_SRC.contains("_reclaim_stale_parts(dest)\n    for attempt in"),
            "the sweep runs at the top of every verified fetch"
        );
        assert!(
            SIDECAR_SRC.contains("glob.escape(dest)"),
            "globbing on the dest prefix, never a bare *.part"
        );
    }

    /// L11#4: a torch too old for weights_only=True is REFUSED, never
    /// degraded to an unsandboxed pickle load (a log line on a captured
    /// stderr that only surfaces on failure reaches nobody).
    #[test]
    fn denoise_sidecar_never_loads_weights_without_weights_only() {
        let loads: Vec<&str> = SIDECAR_SRC
            .lines()
            .filter(|l| l.contains("torch.load("))
            .collect();
        assert!(!loads.is_empty(), "extractor non-vacuity");
        for l in &loads {
            assert!(l.contains("weights_only=True"), "unsandboxed load: {l}");
        }
        assert!(SIDECAR_SRC.contains("refusing to load the SCUNet weights"));
        assert!(!SIDECAR_SRC.contains("loading the weight pickle unsandboxed"));
    }

    /// L11#7b: every sidecar spawn is wrapped in a kill-group — the timeout
    /// must reap the whole tree (torch workers survived the direct-child
    /// kill), and a NEW spawn site must join the rule.
    #[test]
    fn every_sidecar_spawn_is_wrapped_in_a_kill_group() {
        for (name, src) in [
            ("denoise.rs", include_str!("denoise.rs")),
            ("segment.rs", include_str!("segment.rs")),
            ("advisor/claude.rs", include_str!("advisor/claude.rs")),
        ] {
            // Assembled needles: denoise.rs scans its own text, so literal
            // needles here would match themselves (the advisor/mod.rs rule).
            let arm = format!("arm{}kill_group", '_');
            let assign = format!("assign{}kill_group", '_');
            assert!(src.contains(&arm), "{name} spawns without arming a kill-group");
            assert!(src.contains(&assign), "{name} spawns without assigning a kill-group");
        }
    }
}
