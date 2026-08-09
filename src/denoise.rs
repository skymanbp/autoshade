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

/// The sidecar budget is its OWN variable, not `AUTOSHOP_HTTP_TIMEOUT_SECS`:
/// that one tunes API latency, while a sidecar's first run legitimately spends
/// many minutes downloading a model — a user shortening API timeouts must not
/// silently cap the model download at the same number.
pub(crate) fn sidecar_timeout() -> std::time::Duration {
    let seconds = std::env::var("AUTOSHOP_SIDECAR_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|s: &u64| *s > 0)
        .unwrap_or(SIDECAR_DEFAULT_TIMEOUT_SECS);
    std::time::Duration::from_secs(seconds)
}

/// Wait for a piped child without allowing either time or captured bytes to
/// grow without bound (`Command::output()` offered neither — a stalled helper
/// hung the CLI or GUI worker forever). Keeping the TAIL of each stream
/// preserves the traceback line `sidecar_tail` reports.
pub(crate) fn bounded_child_output(
    mut child: std::process::Child,
    who: &str,
    budget: std::time::Duration,
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
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_bounded(stdout_thread, "stdout");
                let _ = join_bounded(stderr_thread, "stderr");
                bail!(
                    "{who} timed out after {:.1}s and was killed \
                     (raise AUTOSHOP_SIDECAR_TIMEOUT_SECS if the first model download is legitimately slower)",
                    budget.as_secs_f32()
                );
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(200)),
            Err(error) => {
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
    let tmp_in = temp_path("autoshop_dn_in")?;
    let tmp_out = match temp_path("autoshop_dn_out") {
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

/// Denoise an image file to another file (no decode/encode here — the sidecar
/// reads/writes it, preserving bit depth). For already-baked PNG/TIFF/JPEG.
pub fn denoise_file(opts: &DenoiseOpts, input: &Path, output: &Path) -> Result<()> {
    crate::pipeline::ensure_parent(output)?;
    run_sidecar(opts, input, output)
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
    if crate::decode::is_raw(input) {
        if full_res {
            crate::render::render_to_file(
                input,
                &crate::recipe::EditRecipe::default(),
                out,
                Some(opts),
                None,
            )?;
            return Ok(());
        }
        // Working copy developed AT ≤2048 (not full-res then thumbnailed):
        // the cap runs before tone/geometry, skipping the 61 MP transients.
        let img = crate::render::render_to_image(
            input,
            &crate::recipe::EditRecipe::default(),
            None,
            Some(2048),
        )?;
        let tmp = temp_path("autoshop_denoise_base")?;
        if let Err(e) = img.save(&tmp) {
            // A failed save can still have created a partial file — don't
            // leak it into the temp dir.
            let _ = std::fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("write denoise input {}", tmp.display()));
        }
        let res = denoise_file(opts, &tmp, out);
        let _ = std::fs::remove_file(&tmp);
        res
    } else {
        // Baked sources go through load_image, NOT straight to the sidecar:
        // cv2 ignores EXIF orientation (and imwrite drops the tag), so a
        // portrait phone JPEG came back as a permanently sideways master.
        // load_image applies the orientation; the working copy also honours
        // the same full-or-≤2048 contract as the RAW arm.
        let img = crate::decode::load_image(input)?;
        let img = if full_res { img } else { img.thumbnail(2048, 2048) };
        let tmp = temp_path("autoshop_denoise_base")?;
        if let Err(e) = img.save(&tmp) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("write denoise input {}", tmp.display()));
        }
        let res = denoise_file(opts, &tmp, out);
        let _ = std::fs::remove_file(&tmp);
        res
    }
}

fn run_sidecar(opts: &DenoiseOpts, input: &Path, output: &Path) -> Result<()> {
    run_sidecar_with_budget(opts, input, output, sidecar_timeout())
}

fn run_sidecar_with_budget(
    opts: &DenoiseOpts,
    input: &Path,
    output: &Path,
    budget: std::time::Duration,
) -> Result<()> {
    if !opts.script.exists() {
        bail!(
            "denoise sidecar not found at {} — run from the project dir or set \
             AUTOSHOP_DENOISE_SCRIPT.",
            opts.script.display()
        );
    }
    // BEFORE the spawn: what already sits at the promised path (a stale
    // deliverable, or the 0-byte claim file the GUI's unique_out creates).
    let before = crate::artifact_state(output);
    let mut cmd = Command::new(&opts.python_bin);
    cmd.arg(&opts.script)
        .arg("--input")
        .arg(input)
        .arg("--output")
        .arg(output)
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
    let run = (|| -> Result<std::process::Output> {
        let child = cmd.spawn().with_context(|| {
            format!(
                "launch denoise sidecar ({} {}) — is Python on PATH / AUTOSHOP_PYTHON set?",
                opts.python_bin,
                opts.script.display()
            )
        })?;
        bounded_child_output(child, "denoise sidecar", budget)
    })();
    let out = match run {
        Ok(out) => out,
        Err(error) => {
            // A killed/failed run must not leave the store holding a claim no
            // recipe references (nor a partial write at the promised name).
            discard_failed_output(output, before);
            return Err(error);
        }
    };
    if !out.status.success() {
        discard_failed_output(output, before);
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
    let wrote = crate::sidecar_wrote("denoise sidecar", output, before);
    if wrote.is_err() {
        discard_failed_output(output, before);
    }
    wrote
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
    use super::*;

    /// A unique-per-test fixture dir: fixed names let two concurrent test
    /// processes (nextest, a second worktree) remove_dir_all each other
    /// mid-run, and a leftover output from an aborted run flips assertions.
    fn tdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("autoshop-dn-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A stand-in "python" that exits 0 and, if `writes`, writes the sidecar's
    /// `--output` argument (argv position 5). Windows: .bat; elsewhere: sh.
    fn stand_in(dir: &Path, writes: bool) -> String {
        #[cfg(windows)]
        {
            let p = dir.join(if writes { "writing.bat" } else { "noop.bat" });
            let body = if writes {
                "@echo denoised-bytes>\"%~5\"\r\n@exit /b 0\r\n"
            } else {
                "@exit /b 0\r\n"
            };
            std::fs::write(&p, body).unwrap();
            p.to_string_lossy().into_owned()
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let p = dir.join(if writes { "writing.sh" } else { "noop.sh" });
            let body = if writes {
                "#!/bin/sh\nprintf denoised-bytes > \"$5\"\nexit 0\n"
            } else {
                "#!/bin/sh\nexit 0\n"
            };
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p.to_string_lossy().into_owned()
        }
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

    /// M-D4: the check replaced by an unconditional refusal (or pointed at
    /// the wrong path) — a sidecar that DOES write its output must succeed.
    /// Without this positive control the negative tests pass under a mutant
    /// that fails every real denoise.
    #[test]
    fn a_sidecar_that_writes_its_output_succeeds() {
        let dir = tdir("writes");
        let opts = opts_for(&dir, true);
        let input = dir.join("in.png");
        std::fs::write(&input, b"not-really-a-png").unwrap();
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
        assert!(err.contains("predates this run"), "{err}");
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
        assert!(err.contains("is empty"), "{err}");
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
        let error = run_sidecar_with_budget(
            &opts,
            &input,
            &output,
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
        let path = temp_path("autoshop-dn-test-claim").unwrap();
        assert!(path.exists(), "temp_path must return an already-owned claim");
        let error = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        let _ = std::fs::remove_file(path);
    }
}
