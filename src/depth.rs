//! The AI lens blur's depth map (2026-10-13): the bridge to `python/depth.py`,
//! which runs MoGe-2 over one picture and writes a 16-bit nearness PNG (see
//! that script's docstring for the format and the model choice, and
//! [`crate::render::LensBlur`] for how the render reads it).

use std::path::Path;

use anyhow::{bail, Context, Result};

/// Run the sidecar `cfg.depth_script` names: `input` (any image file) →
/// `output` (16-bit nearness PNG in `input`'s frame). Through the shared
/// model-sidecar run, so the GPU slot, the bounded wait and "exit 0 is not
/// success" are the family's; and the artifact must DECODE as a raster before
/// anything references it, as a segmentation mask must.
pub fn estimate_depth(cfg: &crate::config::Config, input: &Path, output: &Path) -> Result<()> {
    let script = Path::new(&cfg.depth_script);
    if !script.exists() {
        bail!(
            "depth sidecar not found at {} — run from the project dir or set AUTOSHADE_DEPTH_SCRIPT.",
            script.display()
        );
    }
    let args: Vec<std::ffi::OsString> =
        vec!["-E".into(), script.into(), "--input".into(), input.into(), "--output".into(), output.into()];
    let before = crate::run_model_sidecar_artifact("depth sidecar", &cfg.python_bin, args, output)?;
    if let Err(e) = crate::render::open_mask_bounded(output) {
        crate::denoise::discard_failed_output(output, before);
        return Err(e.context(format!("depth sidecar produced an undecodable map {}", output.display())));
    }
    crate::store::durable_adopt(output).with_context(|| format!("sync depth map {}", output.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A missing script is refused with the variable that fixes it, before
    /// anything is spawned or any file is touched.
    #[test]
    fn a_missing_sidecar_is_named_with_its_override() {
        let mut cfg = crate::config::Config::load();
        cfg.depth_script = "no-such-depth-script.py".into();
        let out = crate::test_dir("depth-missing").join("depth.png");
        let why = format!("{:#}", estimate_depth(&cfg, Path::new("in.png"), &out).unwrap_err());
        assert!(why.contains("AUTOSHADE_DEPTH_SCRIPT"), "the refusal must name its override: {why}");
        assert!(!out.exists(), "nothing may be written when the sidecar is absent");
    }

    /// The bridge spawns nothing itself: the shared run holds the model slot.
    #[test]
    fn the_depth_run_goes_through_the_shared_model_sidecar() {
        let src = include_str!("depth.rs");
        let body = src.split("pub fn estimate_depth(").nth(1).unwrap().split("#[cfg(test)]").next().unwrap();
        assert!(body.contains("crate::run_model_sidecar_artifact("));
        for direct in ["Command::new", "sidecar_command("] {
            assert!(!body.contains(direct), "the bridge must not spawn through {direct}");
        }
    }
}
