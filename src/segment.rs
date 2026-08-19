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
    /// `"subject"` (U²-Net salient object) or `"sky"` (OneFormer ADE20K
    /// Swin-L). Both backends are licence-pinned on the Python side — see
    /// `python/segment.py`'s module docstring: the sky model was SegFormer-B0
    /// until R27 Batch-4, whose weights are licensed for "research or
    /// evaluation purposes only", and the subject backend now names its rembg
    /// session explicitly so an upstream default change cannot swap a
    /// pay-to-use model into a public build.
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
    let wrote = crate::sidecar_wrote("segmentation sidecar", output, before);
    if wrote.is_err() {
        crate::denoise::discard_failed_output(output, before);
        return wrote;
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
    // The mask must be durable BEFORE any recipe references it (L03). The
    // sidecar fsyncs before its os.replace now — this adopt is the belt for
    // an older script on disk, at the cost of one flush.
    crate::store::durable_adopt(output)
        .with_context(|| format!("sync segmentation output {}", output.display()))
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
/// `(photo identity, subtype, reference point, backend generation)` — so a
/// re-render of the same recipe reuses the file, a moved click recomputes, and
/// a photo's two AI masks never collide. The NAME is deliberately absent: it is
/// a localised label the photographer may rename without changing one pixel.
///
/// `AI_BACKEND_GENERATION` is bumped whenever the segmenter's own output would
/// change — a model re-pin, a preprocessing change, the mask-size cap. Without
/// it the cache would happily serve an alpha produced by a model this build no
/// longer runs, which is the one way a cache can lie about provenance.
const AI_BACKEND_GENERATION: u32 = 1;

fn ai_cache_key(src: &Path, subtype: u32, ref_x: f32, ref_y: f32) -> String {
    // FNV-1a over the identity string: small, stable across runs and platforms
    // (unlike `DefaultHasher`, whose output std explicitly does not guarantee
    // between releases — a cache name that moved between builds would silently
    // re-run the model on every upgrade).
    let ident = format!(
        "{}|{subtype}|{ref_x:.6}|{ref_y:.6}|{AI_BACKEND_GENERATION}",
        crate::store::photo_key(src)
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
/// **A failure is never silent and never fatal.** A missing sidecar, a missing
/// dependency, a declined mask (the sidecar's exit 3) or an undecodable output
/// all leave `raster` at `None` — which the render treats as "this mask needs
/// pixels and has none" and SKIPS the whole adjustment rather than applying it
/// at weight 0 (a 0 under `inverted` would apply the edit to the entire
/// frame). The reason is carried out in `unresolved`.
pub fn resolve_ai_masks(
    cfg: &Config,
    src: &Path,
    recipe: &mut crate::recipe::EditRecipe,
) -> AiMaskResolution {
    use crate::recipe::MaskGeometry;

    let mut out = AiMaskResolution::default();
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
            let target = crate::store::raster_target(src, &ai_cache_key(src, subtype, rx, ry));
            // CACHE HIT, and it is checked by DECODING, not by `exists()`: a
            // half-written or truncated PNG on disk is exactly the file a
            // cheap existence test would serve forever.
            if target.exists() && crate::render::open_mask_bounded(&target).is_ok() {
                *raster = Some(target.to_string_lossy().into_owned());
                out.resolved += 1;
                out.cached += 1;
                continue;
            }
            let Some(opts) = SegmentOpts::for_ai_mask(cfg, subtype, rx, ry) else {
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
            // The preview, decoded at most once per call.
            let input = preview.get_or_insert_with(|| stage_source_frame(src));
            let Some(input) = input.as_ref() else {
                out.unresolved
                    .push((mask_name.clone(), "the source frame could not be decoded".into()));
                continue;
            };
            match segment_file(&opts, input, &target) {
                Ok(()) => {
                    *raster = Some(target.to_string_lossy().into_owned());
                    out.resolved += 1;
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

/// Stage the photo's ORIGINAL-frame preview as a PNG for the sidecar, or
/// `None` if it cannot be decoded.
///
/// The ORIGINAL frame specifically: a `crs:ReferencePoint` is normalised to it,
/// and the mask this produces is sampled in the same normalised space
/// (`render::sample_gray_norm`), so both ends agree without either one knowing
/// the photo's pixel dimensions.
fn stage_source_frame(src: &Path) -> Option<PathBuf> {
    let decoded = crate::decode::decode_raw_turned(
        src,
        crate::store::saved_quarter_turns(src),
    )
    .map_err(|e| {
        eprintln!("⚠ AI mask: cannot decode {} for segmentation: {e:#}", src.display());
    })
    .ok()?;
    let out = std::env::temp_dir()
        .join(format!("autoshop-ai-src-{}-{}.png", std::process::id(), crate::pipeline::stem(src)));
    decoded
        .preview
        .to_rgb8()
        .save(&out)
        .map_err(|e| eprintln!("⚠ AI mask: cannot stage the source frame {}: {e}", out.display()))
        .ok()?;
    Some(out)
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
    /// the subtype (which backend runs), the click (where it is prompted) and
    /// the backend generation. Nothing else — the mask NAME is a localised
    /// label the photographer may rename without changing one pixel.
    ///
    /// MUTATION-LINED. Verified red by dropping `ref_x`/`ref_y` from
    /// `ai_cache_key`'s identity string (transcript in the batch report): two
    /// masks clicked on opposite sides of the same photo then share one cached
    /// alpha, so the second one silently renders the first one's selection.
    #[test]
    fn the_ai_cache_key_covers_the_photo_the_subtype_and_the_click() {
        let a = Path::new("D:/rolls/2024/DSC0001.ARW");
        let b = Path::new("D:/rolls/2024/DSC0002.ARW");
        let base = ai_cache_key(a, 2, 0.5, 0.5);
        assert_eq!(base, ai_cache_key(a, 2, 0.5, 0.5), "the key is stable across calls");
        assert_ne!(base, ai_cache_key(b, 2, 0.5, 0.5), "a different photo, a different alpha");
        assert_ne!(base, ai_cache_key(a, 0, 0.5, 0.5), "a different backend, a different alpha");
        assert_ne!(base, ai_cache_key(a, 2, 0.9, 0.5), "a moved click, a different alpha");
        assert_ne!(base, ai_cache_key(a, 2, 0.5, 0.9), "…in either axis");
        // A plain file name, safe to hand `store::raster_target`.
        assert!(
            base.starts_with("ai-mask-")
                && base[8..].chars().all(|c| c.is_ascii_hexdigit())
                && base.len() == 8 + 16,
            "the key must be a bare, filesystem-safe stem: {base}"
        );
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
        let got = resolve_ai_masks(&cfg, Path::new("D:/rolls/2024/DSC0001.ARW"), &mut r);
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
        let got = resolve_ai_masks(&cfg, Path::new("D:/nope/nothing-here.ARW"), &mut r);
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
}
