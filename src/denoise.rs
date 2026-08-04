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

/// Denoise a full-resolution sRGB buffer (`[0,1]` per channel) in place by
/// round-tripping it through the sidecar as a 16-bit TIFF.
pub fn denoise_buffer(opts: &DenoiseOpts, data: &mut [[f32; 3]], w: usize, h: usize) -> Result<()> {
    if data.len() != w * h {
        bail!("denoise_buffer: buffer {} != {}x{}", data.len(), w, h);
    }
    let tmp_in = temp_path("autoshop_dn_in");
    let tmp_out = temp_path("autoshop_dn_out");

    // pack [f32;3] -> 16-bit RGB TIFF
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
    Ok(())
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
