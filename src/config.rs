//! Runtime configuration for the AI providers.
//!
//! Resolution order (later wins): built-in defaults → environment (a gitignored
//! `.env` via `dotenvy`) → the UI-written local file `autoshop.local.json`
//! (also gitignored). Secrets (API keys) come only from `.env` or that local
//! file, never from a committed source and never logged.
//!
//! Two AI roles (see `docs/ARCHITECTURE.md` §3), each independently configurable:
//!   * **analysis** (the verifier) — provider `oauth` (the `claude` CLI, no key)
//!     or `api` (an OpenAI-compatible chat endpoint).
//!   * **image** (the vision proposer) — `api` only: the `claude` CLI has no
//!     image input in print mode, so this role needs an OpenAI-compatible vision
//!     endpoint (point `image_base_url` anywhere that speaks the API).
//!
//! Style is handled by similarity retrieval (`src/style.rs`), not a global
//! calibration baked in here.

use std::env;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The UI-written / hand-edited local config. Every field is optional; a present
/// value overrides the environment. Lives in [`local_settings_path`] (gitignored).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct LocalSettings {
    pub analysis_provider: Option<String>,
    pub analysis_model: Option<String>,
    pub analysis_api_key: Option<String>,
    pub analysis_base_url: Option<String>,
    pub image_api_key: Option<String>,
    pub image_model: Option<String>,
    pub image_base_url: Option<String>,
    pub image_gen_model: Option<String>,
    /// `"oauth"` (ChatGPT-subscription via a local Codex bridge, e.g. CLIProxyAPI)
    /// or `"api"` (a real OpenAI-compatible key). Purely a UI/preset selector —
    /// both resolve to the same OpenAI-compatible HTTP path, differing only in
    /// `image_base_url`/`image_api_key`. Absent ⇒ `"api"` (prior behaviour).
    pub image_provider: Option<String>,
}

/// Path to the local settings file — the per-user store root, so the SAME
/// settings load no matter which directory the app was launched from (this
/// used to be a cwd-relative `autoshop.local.json`).
pub fn local_settings_path() -> PathBuf {
    crate::store::settings_path()
}

/// Read the local settings file, if present. A missing or malformed file yields
/// defaults (we never block startup on it). A pre-store cwd-relative
/// `autoshop.local.json` is still honoured as a read fallback; the next save
/// writes the central file, which then wins.
pub fn load_local_settings() -> LocalSettings {
    for p in [local_settings_path(), PathBuf::from("autoshop.local.json")] {
        if let Ok(s) = std::fs::read_to_string(&p) {
            return serde_json::from_str(&s).unwrap_or_default();
        }
    }
    LocalSettings::default()
}

/// Persist the local settings file (the POST /api/settings target).
/// tmp + rename: `fs::write` truncates in place, so a crash / disk-full mid-
/// write left partial JSON that `load_local_settings` silently turned into
/// complete defaults — losing every saved key and model choice.
pub fn save_local_settings(s: &LocalSettings) -> std::io::Result<PathBuf> {
    let p = local_settings_path();
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // ONE open claims AND holds the file (create_new + 0600 in the same
    // OpenOptions): the earlier split — a claim loop, then a separate
    // create_new write — met its OWN claim with AlreadyExists and failed
    // every settings save outright. pid+seq names avoid cross-process
    // collisions; the loop skips a crashed predecessor's stale tmp from a
    // recycled PID (bounded: 16 tries, then surface the error).
    // The file carries API keys: on Unix it is CREATED 0600 — a chmod after
    // the write left a world-readable window. Windows AppData is per-user.
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut tmp = std::path::PathBuf::new();
    let mut file: Option<std::fs::File> = None;
    for _ in 0..16 {
        tmp = p.with_extension(format!(
            "json.tmp{}-{}",
            std::process::id(),
            TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        match opts.open(&tmp) {
            Ok(f) => {
                file = Some(f);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    let Some(mut f) = file else {
        return Err(std::io::Error::other(
            "could not claim a settings temp file (16 stale .tmp siblings?)",
        ));
    };
    let write = {
        use std::io::Write as _;
        f.write_all(serde_json::to_string_pretty(s).unwrap_or_default().as_bytes())
    };
    drop(f); // close before rename — Windows cannot rename an open file
    if let Err(e) = write {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, &p) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(p)
}

// Clone: the web server snapshots the config OUT of its RwLock before long AI
// calls — holding the read guard across a multi-minute retouch blocked every
// settings save for the whole stall window.
#[derive(Clone)]
pub struct Config {
    // --- image role: the vision proposer (OpenAI-compatible API only) ---------
    /// API key for the image (vision) role + generative edits. `None` ⇒ the
    /// proposer falls back to the heuristic baseline.
    pub openai_api_key: Option<String>,
    pub openai_model: String,
    pub openai_base_url: String,
    /// Image model for generative retouch/reimagine (V2_PLAN §5).
    pub openai_image_model: String,
    /// Output quality tier for generative edits: low | medium | high | auto.
    pub openai_image_quality: String,
    /// Pixel budget for FLEXIBLE-size generative output. gpt-image-2 accepts any
    /// WIDTHxHEIGHT (edges ×16, ratio ≤3:1, ≤8 294 400 px total — the API max);
    /// cost scales with pixels, so lower this to spend less per image. Models
    /// without flexible sizing 400 the request and we fall back to the fixed
    /// 1024/1536 enum automatically.
    pub openai_image_max_px: u32,
    /// UI/preset selector for the image role: `"oauth"` (ChatGPT-subscription via
    /// a local Codex bridge) or `"api"` (a real OpenAI-compatible key). The engine
    /// path is identical for both — this only distinguishes how the endpoint above
    /// was populated, so the Settings UI can restore the right mode.
    pub image_provider: String,

    // --- analysis role: the verifier (oauth = claude CLI, or api = OpenAI) -----
    /// `"oauth"` (default; the `claude` CLI) or `"api"` (OpenAI-compatible chat).
    pub analysis_provider: String,
    /// Model for the analysis role: a `claude` alias/id for oauth (default
    /// `opus`), or a chat model id for api.
    pub analysis_model: String,
    /// Path/name of the `claude` executable (oauth analysis, reuses Claude OAuth).
    pub claude_bin: String,
    /// API key + base for the `api` analysis provider (independent of the image key).
    pub analysis_api_key: Option<String>,
    pub analysis_base_url: String,

    // --- AI denoise sidecar ---------------------------------------------------
    /// Python interpreter for the AI sidecars (`python/denoise.py`, `segment.py`).
    pub python_bin: String,
    /// SCUNet weight set (color_real_psnr default; see python/denoise.py).
    pub denoise_model: String,
    pub denoise_script: String,
    pub denoise_cache: String,
    /// AI segmentation sidecar (`python/segment.py`) — subject/sky bitmap masks.
    pub segment_script: String,

    /// How strongly to lean on the user's historical edit style, 0.0..1.0.
    pub style_strength: f32,
}

impl Config {
    pub fn load() -> Self {
        // .env first (absence is fine; never prints the key), then the local file.
        // 2026-08-03: `dotenv_override`, not `dotenv`. Why: plain `dotenv()`
        // leaves an already-set process var alone, and this machine carries a
        // User-scope OPENAI_API_KEY — so a run launched from a shell holding it
        // silently billed that global key instead of this project's own .env
        // key. Project .env is the default; the global stays the fallback for
        // names .env does not define.
        let _ = dotenvy::dotenv_override();
        let nonempty = |k: &str| env::var(k).ok().filter(|s| !s.trim().is_empty());
        let local = load_local_settings();
        // local-file value wins over env; `pick` returns the first non-empty.
        let pick = |file: &Option<String>, e: Option<String>, default: &str| -> String {
            file.as_ref()
                .filter(|s| !s.trim().is_empty())
                .cloned()
                .or(e)
                .unwrap_or_else(|| default.to_string())
        };
        let pick_opt = |file: &Option<String>, e: Option<String>| -> Option<String> {
            file.as_ref()
                .filter(|s| !s.trim().is_empty())
                .cloned()
                .or(e)
        };

        let default_base = "https://api.openai.com/v1";
        Config {
            openai_api_key: pick_opt(&local.image_api_key, nonempty("OPENAI_API_KEY")),
            openai_model: pick(&local.image_model, nonempty("AUTOSHOP_OPENAI_MODEL"), "gpt-5.5"),
            openai_base_url: pick(&local.image_base_url, nonempty("AUTOSHOP_OPENAI_BASE_URL"), default_base),
            openai_image_model: pick(
                &local.image_gen_model,
                nonempty("AUTOSHOP_OPENAI_IMAGE_MODEL"),
                "gpt-image-1.5",
            ),
            openai_image_quality: nonempty("AUTOSHOP_IMAGE_QUALITY").unwrap_or_else(|| "high".to_string()),
            openai_image_max_px: nonempty("AUTOSHOP_IMAGE_MAX_PX")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(8_294_400),
            image_provider: pick(
                &local.image_provider,
                nonempty("AUTOSHOP_IMAGE_PROVIDER"),
                "api",
            ),

            analysis_provider: pick(
                &local.analysis_provider,
                nonempty("AUTOSHOP_ANALYSIS_PROVIDER"),
                "oauth",
            ),
            analysis_model: pick(
                &local.analysis_model,
                nonempty("AUTOSHOP_ANALYSIS_MODEL").or_else(|| nonempty("AUTOSHOP_CLAUDE_MODEL")),
                "opus",
            ),
            claude_bin: nonempty("AUTOSHOP_CLAUDE_BIN").unwrap_or_else(|| "claude".to_string()),
            analysis_api_key: pick_opt(&local.analysis_api_key, nonempty("AUTOSHOP_ANALYSIS_API_KEY")),
            analysis_base_url: pick(
                &local.analysis_base_url,
                nonempty("AUTOSHOP_ANALYSIS_BASE_URL"),
                default_base,
            ),

            python_bin: nonempty("AUTOSHOP_PYTHON").unwrap_or_else(|| "python".to_string()),
            denoise_model: nonempty("AUTOSHOP_DENOISE_MODEL")
                .unwrap_or_else(|| "color_real_psnr".to_string()),
            denoise_script: nonempty("AUTOSHOP_DENOISE_SCRIPT")
                .unwrap_or_else(|| "python/denoise.py".to_string()),
            denoise_cache: nonempty("AUTOSHOP_DENOISE_CACHE")
                .unwrap_or_else(|| "python/weights".to_string()),
            segment_script: nonempty("AUTOSHOP_SEGMENT_SCRIPT")
                .unwrap_or_else(|| "python/segment.py".to_string()),
            style_strength: nonempty("AUTOSHOP_STYLE_STRENGTH")
                .and_then(|s| s.parse::<f32>().ok())
                // "NaN" parses as a valid f32 and SURVIVES clamp (clamp keeps
                // NaN) — a non-finite blend strength would poison the style
                // math downstream, so it falls back to the default instead.
                .filter(|v| v.is_finite())
                .unwrap_or(0.3)
                .clamp(0.0, 1.0),
        }
    }

    /// True if the analysis role is configured to use the OpenAI-compatible API.
    pub fn analysis_is_api(&self) -> bool {
        self.analysis_provider.eq_ignore_ascii_case("api")
    }

    /// True when the image role points at a ChatGPT-subscription Codex bridge
    /// (OAuth preset) rather than a real OpenAI-compatible key (API preset).
    pub fn image_is_oauth(&self) -> bool {
        self.image_provider.eq_ignore_ascii_case("oauth")
    }
}
