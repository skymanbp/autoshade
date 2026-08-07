//! AI segmentation bridge — Rust side of the sidecar (`python/segment.py`).
//!
//! Same shell-out pattern as [`crate::denoise`] (SCUNet): a local Python
//! process does the model inference and writes an 8-bit grayscale mask PNG
//! (white = selected, soft edges = the model's own alpha), which the app then
//! attaches to the recipe as a [`crate::recipe::MaskGeometry::Bitmap`] local
//! adjustment. The AI picks *where*; every actual edit stays a deterministic
//! recipe slider. Models auto-download to the user's home caches on first run
//! (`~/.u2net`, `~/.cache/huggingface`) — no weights in the repo.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::config::Config;

/// Everything one segmentation run needs; built from [`Config`] like
/// [`crate::denoise::DenoiseOpts`].
pub struct SegmentOpts {
    pub python_bin: String,
    pub script: PathBuf,
    /// `"subject"` (U²-Net salient object) or `"sky"` (SegFormer ADE20K).
    pub target: String,
}

impl SegmentOpts {
    pub fn from_config(cfg: &Config, target: &str) -> Self {
        SegmentOpts {
            python_bin: cfg.python_bin.clone(),
            script: PathBuf::from(&cfg.segment_script),
            target: target.to_string(),
        }
    }
}

/// Run the sidecar: `input` (any image file) → `output` (8-bit grayscale PNG).
/// The mask is in the INPUT's frame — feed it the original-frame preview so it
/// lands in the same space recipe masks live in.
pub fn segment_file(opts: &SegmentOpts, input: &Path, output: &Path) -> Result<()> {
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
    cmd.arg(&opts.script)
        .arg("--input")
        .arg(input)
        .arg("--output")
        .arg(output)
        .arg("--target")
        .arg(&opts.target)
        // CAPTURE, never inherit: the release GUI is windows_subsystem="windows"
        // and has NO console, so inherited handles discard the sidecar's output —
        // the reason a missing dependency used to surface as a bare exit code.
        // The tail goes into the error instead, which reaches the GUI toast.
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Don't flash a console window when the windowed GUI spawns the sidecar.
    crate::hide_child_console(&mut cmd);
    let out = cmd.output().with_context(|| {
        format!(
            "launch segmentation sidecar ({} {}) — is Python on PATH / AUTOSHOP_PYTHON set?",
            opts.python_bin,
            opts.script.display()
        )
    })?;
    if !out.status.success() {
        bail!(
            "segmentation sidecar exited with {}: {}",
            out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
            crate::sidecar_tail(&out.stderr, &out.stdout)
        );
    }
    // Exit 0 alone is not success — THIS run must have produced a non-empty
    // mask (see `crate::sidecar_wrote` for the three refusals).
    crate::sidecar_wrote("segmentation sidecar", output, before)
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
        };
        let input = dir.join("in.png");
        std::fs::write(&input, b"not-really-a-png").unwrap();
        let output = dir.join("mask.png");
        std::fs::write(&output, b"").unwrap(); // the claim file

        let err = segment_file(&opts, &input, &output).unwrap_err().to_string();
        assert!(err.contains("is empty"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
