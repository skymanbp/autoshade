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
    let tmp_in = temp_path("autoshop_dn_in");
    let tmp_out = temp_path("autoshop_dn_out");
    // PIDs recycle and the unique() counter restarts at 0, so a crash-orphaned
    // tmp_out from a dead process can sit at exactly this path — and a
    // same-dimension orphan would be decoded as this photo's result. Clear it
    // so anything read back is provably this run's write.
    let _ = std::fs::remove_file(&tmp_out);

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
        // read 16-bit result back into the buffer
        // A 60 MP 16-bit image exceeds the decoder's default memory cap, so lift
        // the limit explicitly for this trusted, self-produced file.
        let mut reader = image::ImageReader::open(&tmp_out)
            .with_context(|| format!("open denoise output {}", tmp_out.display()))?;
        reader.limits(image::Limits::no_limits());
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
        let tmp = temp_path("autoshop_denoise_base");
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
        let tmp = temp_path("autoshop_denoise_base");
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
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Don't flash a console window when the windowed GUI spawns the sidecar.
    crate::hide_child_console(&mut cmd);
    let out = cmd
        .output()
        .with_context(|| {
            format!(
                "launch denoise sidecar ({} {}) — is Python on PATH / AUTOSHOP_PYTHON set?",
                opts.python_bin,
                opts.script.display()
            )
        })?;
    if !out.status.success() {
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
    crate::sidecar_wrote("denoise sidecar", output, before)
}

fn temp_path(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    // 16-bit PNG interchange: unambiguous for both cv2 and the image crate (no
    // TIFF predictor-tag mismatch), still lossless and full bit depth.
    p.push(format!("{tag}_{}_{}.png", std::process::id(), unique()));
    p
}

/// Monotonic-ish suffix so two buffers in one process don't collide (no RNG dep).
fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
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
}
