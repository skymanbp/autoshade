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
use std::path::{Path, PathBuf};

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

/// Where a [`LocalSettings`] came from. This is a TRUST label, not a
/// breadcrumb: the two sources do not deserve the same authority.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsOrigin {
    /// [`local_settings_path`] — under the per-user store root. The user's own.
    Central,
    /// A cwd-relative `autoshop.local.json`. AMBIENT: whatever directory the
    /// app happens to be launched from supplies it.
    WorkingDir,
    /// No file was read.
    None,
}

impl LocalSettings {
    /// Drop the fields an AMBIENT file must not be allowed to choose.
    ///
    /// An API key and the endpoint it is sent to are one decision. The web
    /// server already refuses to let a page make it — `serve.rs`'s
    /// cross-origin guard exists because "`/api/settings` would repoint the AI
    /// base URL at the attacker and the next Analyze would hand over the
    /// user's API key". The FILESYSTEM route to that same outcome was open:
    /// resolution is per FIELD, not per file, so a planted
    /// `autoshop.local.json` carrying only `image_base_url` redirected the
    /// endpoint while the real key still came from `.env` / the environment.
    /// Extracting a shared archive and running Autoshop inside it was enough.
    ///
    /// So an ambient file keeps the harmless selectors (models, providers) and
    /// loses both halves of that decision. It cannot supply a key either: a
    /// planted key would bill the user's work to someone else's account and
    /// put their photos in it.
    fn without_ambient_authority(mut self) -> Self {
        self.image_api_key = None;
        self.analysis_api_key = None;
        self.image_base_url = None;
        self.analysis_base_url = None;
        self
    }

    fn names_any_ambient_field(&self) -> bool {
        self.image_api_key.is_some()
            || self.analysis_api_key.is_some()
            || self.image_base_url.is_some()
            || self.analysis_base_url.is_some()
    }
}

/// Read the local settings file, if present, and say WHERE it came from.
/// A missing file yields defaults (we never block startup on it).
///
/// A malformed file does NOT quietly become defaults any more. It used to:
/// `unwrap_or_default()` turned one stray comma in a hand-edited file into a
/// `LocalSettings` with every field `None`, silently — and because both
/// writers (`serve.rs`'s settings route and the GUI's) read-merge-write onto
/// what they were handed and skip blank secrets, the very next Settings save
/// published that emptiness over the user's real keys. The file is documented
/// as hand-editable, so a typo is the expected vector, and the loss was
/// permanent and unannounced. Now the bytes are preserved beside the original
/// and the reader moves on to the next candidate.
fn preserve_corrupt_settings(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write as _;

    static CORRUPT_SEQ: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    for _ in 0..16 {
        let kept = path.with_extension(format!(
            "json.corrupt.{}-{}",
            std::process::id(),
            CORRUPT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        let mut file = match opts.open(&kept) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        if let Err(e) = file.write_all(bytes).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = std::fs::remove_file(&kept);
            return Err(e);
        }
        drop(file);
        // The unique copy now owns the bytes; removing the malformed live
        // file avoids rescuing the same incident again on every launch.
        let _ = std::fs::remove_file(path);
        return Ok(kept);
    }
    Err(std::io::Error::other(
        "could not claim a corrupt-settings rescue file",
    ))
}

pub fn load_local_settings_from() -> (LocalSettings, SettingsOrigin) {
    for (p, origin) in [
        (local_settings_path(), SettingsOrigin::Central),
        (PathBuf::from("autoshop.local.json"), SettingsOrigin::WorkingDir),
    ] {
        let Ok(s) = std::fs::read_to_string(&p) else { continue };
        match serde_json::from_str::<LocalSettings>(&s) {
            Ok(v) => return (v, origin),
            Err(e) => {
                // Keep the bytes: they hold the user's API keys, and a save is
                // about to overwrite this path. Best-effort and once — a
                // second launch must not clobber the first rescue.
                let kept = preserve_corrupt_settings(&p, s.as_bytes()).ok();
                eprintln!(
                    "warning: {} is not valid JSON ({e}) — ignoring it{}",
                    p.display(),
                    if let Some(kept) = &kept {
                        format!("; your settings were preserved at {}", kept.display())
                    } else {
                        String::new()
                    }
                );
            }
        }
    }
    (LocalSettings::default(), SettingsOrigin::None)
}

/// The settings a caller should MERGE INTO when saving (see
/// [`load_local_settings_from`] for why the origin matters elsewhere).
///
/// An ambient file's key and endpoint are stripped HERE too, not only on the
/// read path in [`Config::load`]. Both settings writers (`serve`'s route and
/// the GUI's panel) copy a field only when the incoming request names one, and
/// then save to the CENTRAL path — so a merge onto the raw working-directory
/// values promoted a planted `image_base_url` into the trusted file, where the
/// read-path guard no longer applies (`SettingsOrigin::Central`) and the very
/// next Analyze posts the real key to it. One settings save would have undone
/// the whole guard.
pub fn load_local_settings() -> LocalSettings {
    match load_local_settings_from() {
        (s, SettingsOrigin::WorkingDir) => s.without_ambient_authority(),
        (s, _) => s,
    }
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
        // ONCE per process: dotenv_override mutates the process environment
        // (env::set_var), which is unsound to run concurrently with env reads
        // on non-Windows — and Config::load() is called from request/worker
        // threads (the web Settings hot-reload rebuilds the config). The
        // first load happens on the main thread before any worker exists;
        // later reloads reuse the already-applied environment. Recorded
        // residuals, behaviour otherwise unchanged: the cwd-upward .env
        // search (a run from another checkout's subtree adopts that tree's
        // .env), and a .env edited mid-session now applies on the next
        // launch rather than on the next settings save.
        //
        // A `.env` is AMBIENT INPUT by the same argument as a working-directory
        // settings file, and a stronger one: dotenvy searches the cwd and every
        // parent (`find.rs`), and `dotenv_override` beats a variable the user
        // really set. So a `.env` dropped beside a shared archive of photos —
        // "extract, then run Autoshop in there" — could name the endpoint the
        // key is sent to while the key itself still resolved from the user's
        // own environment. That is the exfiltration route `without_ambient_
        // authority` closes for `autoshop.local.json`, re-opened through the
        // sibling file.
        //
        // The protected set is every variable that decides WHERE something is
        // sent or WHAT gets executed — not just the endpoints. Naming only the
        // base URLs left the strictly worse half open: `AUTOSHOP_CLAUDE_BIN`
        // and `AUTOSHOP_PYTHON` reach `Command::new` directly
        // (`advisor/claude.rs`, `denoise.rs`, `segment.rs`) and the two script
        // variables become that command's argv, so the very scenario this
        // comment describes yielded arbitrary process execution rather than
        // mere endpoint redirection. `autoshop.local.json` cannot reach these
        // at all (`LocalSettings` has no such field), so `.env` is the only
        // route and this is where it closes.
        //
        // Everything else — keys, model names, providers, tuning numbers — is
        // still honoured from `.env`, which is where this project's own key
        // lives. (A planted key can no longer read the user's photos back:
        // every Responses call sends `store: false`, so nothing persists in
        // the key owner's account — see advisor/openai.rs.)
        //
        // PYTHONPATH / PYTHONHOME join the protected set for the same reason
        // as the script variables: both Python sidecars inherit the process
        // environment, and a .env's `PYTHONPATH=.` beside a hostile
        // `numpy.py` is code execution at import time (the sidecars also
        // pass `-E` — defence in both layers). The weight cache joins
        // because a redirected cache is a poisoned-model path.
        const AMBIENT_UNSAFE_VARS: [&str; 17] = [
            "AUTOSHOP_OPENAI_BASE_URL",
            "AUTOSHOP_ANALYSIS_BASE_URL",
            "AUTOSHOP_CLAUDE_BIN",
            "AUTOSHOP_PYTHON",
            "AUTOSHOP_DENOISE_SCRIPT",
            "AUTOSHOP_SEGMENT_SCRIPT",
            "PATH",
            "AUTOSHOP_DATA_DIR",
            "AUTOSHOP_ANALYSIS_PROVIDER",
            "AUTOSHOP_ANALYSIS_MODEL",
            "AUTOSHOP_CLAUDE_MODEL",
            "AUTOSHOP_OPENAI_MODEL",
            "AUTOSHOP_OPENAI_IMAGE_MODEL",
            "AUTOSHOP_IMAGE_PROVIDER",
            "PYTHONPATH",
            "PYTHONHOME",
            "AUTOSHOP_DENOISE_CACHE",
        ];
        static PRE_DOTENV: std::sync::OnceLock<[Option<String>; 17]> = std::sync::OnceLock::new();
        let pre_dotenv = PRE_DOTENV.get_or_init(|| {
            let before = AMBIENT_UNSAFE_VARS.map(|k| env::var(k).ok());
            let _ = dotenvy::dotenv_override();
            for (i, k) in AMBIENT_UNSAFE_VARS.iter().enumerate() {
                if env::var(k).ok() != before[i] {
                    eprintln!(
                        "warning: ignoring {k} from a .env file — a .env found in the working \
                         directory (or any parent) is not trusted to choose where your API key \
                         is sent or which program is run. Set it in your own environment."
                    );
                }
            }
            // Reading protected values from the snapshot is insufficient for
            // PATH and the data root: their consumers consult the live process
            // environment. Restore every protected variable so later consumers
            // see exactly the authority the process started with.
            for (k, value) in AMBIENT_UNSAFE_VARS.iter().zip(before.iter()) {
                // This is the same one-time, pre-worker initialization window
                // in which dotenv_override above mutates the environment.
                unsafe {
                    match value {
                        Some(value) => env::set_var(k, value),
                        None => env::remove_var(k),
                    }
                }
            }
            before
        });
        let nonempty = |k: &str| env::var(k).ok().filter(|s| !s.trim().is_empty());
        // The pre-.env value of a variable an ambient file must not own.
        // Indices follow `AMBIENT_UNSAFE_VARS`.
        let pre = |i: usize| pre_dotenv[i].clone().filter(|s: &String| !s.trim().is_empty());
        let (local, origin) = load_local_settings_from();
        // A cwd-relative settings file is ambient input, not the user's own
        // configuration: it may pick models, never a key or the endpoint a key
        // is sent to. See `LocalSettings::without_ambient_authority`.
        let local = if origin == SettingsOrigin::WorkingDir {
            if local.names_any_ambient_field() {
                eprintln!(
                    "warning: ignoring the API key and base-URL fields in ./autoshop.local.json \
                     — a settings file found in the WORKING DIRECTORY is not trusted to choose \
                     where your API key is sent. Save your settings in the app to store them \
                     under your user profile instead."
                );
            }
            local.without_ambient_authority()
        } else {
            local
        };
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
        // Bundled sidecar helpers resolve against the PROGRAM's own tree,
        // never the cwd (see `bundled_helper`): a photo pack carrying
        // `python/denoise.py` used to have that file executed as the user
        // the next time denoise ran from inside it. The weight cache
        // defaults to `weights/` beside whichever script answered, so dev
        // builds keep the repo cache and a packaged install stays beside
        // the exe.
        let denoise_script =
            pre(4).unwrap_or_else(|| bundled_helper("python/denoise.py"));
        let denoise_cache = pre(16).unwrap_or_else(|| {
            Path::new(&denoise_script)
                .parent()
                .map(|d| d.join("weights").to_string_lossy().into_owned())
                .unwrap_or_else(|| bundled_helper("python/weights"))
        });
        let segment_script =
            pre(5).unwrap_or_else(|| bundled_helper("python/segment.py"));
        Config {
            openai_api_key: pick_opt(&local.image_api_key, nonempty("OPENAI_API_KEY")),
            openai_model: pick(&local.image_model, pre(11), "gpt-5.5"),
            openai_base_url: pick(&local.image_base_url, pre(0), default_base),
            openai_image_model: pick(
                &local.image_gen_model,
                pre(12),
                "gpt-image-1.5",
            ),
            openai_image_quality: nonempty("AUTOSHOP_IMAGE_QUALITY").unwrap_or_else(|| "high".to_string()),
            openai_image_max_px: nonempty("AUTOSHOP_IMAGE_MAX_PX")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(8_294_400),
            image_provider: pick(
                &local.image_provider,
                pre(13),
                "api",
            ),

            analysis_provider: pick(
                &local.analysis_provider,
                pre(8),
                "oauth",
            ),
            analysis_model: pick(
                &local.analysis_model,
                pre(9).or_else(|| pre(10)),
                "opus",
            ),
            claude_bin: pre(2).unwrap_or_else(|| "claude".to_string()),
            analysis_api_key: pick_opt(&local.analysis_api_key, nonempty("AUTOSHOP_ANALYSIS_API_KEY")),
            analysis_base_url: pick(&local.analysis_base_url, pre(1), default_base),

            python_bin: pre(3).unwrap_or_else(|| "python".to_string()),
            denoise_model: nonempty("AUTOSHOP_DENOISE_MODEL")
                .unwrap_or_else(|| "color_real_psnr".to_string()),
            denoise_script,
            denoise_cache,
            segment_script,
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

/// Resolve a bundled helper (sidecar script / weight cache) against the
/// PROGRAM'S OWN tree, never the working directory. The cwd is ambient input
/// — photos arrive in unzipped packs, and a pack carrying `python/denoise.py`
/// satisfied the old cwd-relative default's `exists()` gate, so the pack's
/// script ran as the user. Candidates: the executable's directory (a packaged
/// install) and its first three ancestors (`target/release` and
/// `target/debug/deps` both reach the repo root). When nothing answers, the
/// FIRST candidate still names the expected location, so the sidecar's
/// "not found at <path>" error stays actionable — and never names a path an
/// untrusted pack could satisfy.
fn bundled_helper(rel: &str) -> String {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = env::current_exe()
        && let Some(dir) = exe.parent()
    {
        roots.push(dir.to_path_buf());
        let mut up = dir;
        for _ in 0..3 {
            match up.parent() {
                Some(p) => {
                    roots.push(p.to_path_buf());
                    up = p;
                }
                None => break,
            }
        }
    }
    for root in &roots {
        let cand = root.join(rel);
        if cand.exists() {
            return cand.to_string_lossy().into_owned();
        }
    }
    roots.first().map(|r| r.join(rel).to_string_lossy().into_owned()).unwrap_or_else(|| {
        // current_exe() unavailable (e.g. a chroot without /proc): FAIL
        // CLOSED. Returning `rel` here would quietly reopen the
        // planted-script hole as a cwd-relative lookup (Codex R11 #2). A
        // NUL byte is unrepresentable in real paths on every platform, so
        // this sentinel can never be satisfied by any file an untrusted
        // pack could create — the sidecar's "not found at <path>" refusal
        // fires instead.
        format!("\u{0}install-dir-unavailable/{rel}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A settings file found in the WORKING DIRECTORY may not decide where an
    /// API key goes.
    ///
    /// The exfiltration this closes needs no exotic setup, because resolution
    /// is per FIELD rather than per file: an ambient `autoshop.local.json`
    /// that carries ONLY `image_base_url` redirects the endpoint while the
    /// real key still resolves from `.env` / the environment, so the next
    /// Analyze posts `Authorization: Bearer <real key>` to the planted host.
    /// Extract a shared archive of photos, run Autoshop in it, done.
    /// `serve.rs` already refuses to let a web page make this exact change;
    /// the filesystem route to it was open.
    #[test]
    fn a_working_directory_settings_file_cannot_redirect_your_api_key() {
        let planted = LocalSettings {
            // What the attack needs: the endpoint, and nothing else.
            image_base_url: Some("https://attacker.example/v1".into()),
            analysis_base_url: Some("https://attacker.example/v1".into()),
            // And what it would take if it could.
            image_api_key: Some("planted".into()),
            analysis_api_key: Some("planted".into()),
            // Harmless selectors an ambient file is still allowed to set.
            image_model: Some("gpt-5.6-sol".into()),
            analysis_model: Some("opus".into()),
            image_provider: Some("api".into()),
            analysis_provider: Some("oauth".into()),
            image_gen_model: Some("gpt-image-2".into()),
        };
        assert!(planted.names_any_ambient_field(), "the warning must fire for this file");

        let safe = planted.clone().without_ambient_authority();
        assert_eq!(safe.image_base_url, None, "an ambient file chose the image endpoint");
        assert_eq!(safe.analysis_base_url, None, "an ambient file chose the analysis endpoint");
        assert_eq!(safe.image_api_key, None, "an ambient file supplied the image key");
        assert_eq!(safe.analysis_api_key, None, "an ambient file supplied the analysis key");

        // …and it is a scalpel, not a reset: everything else still applies, so
        // the pre-store cwd file keeps working as the migration affordance it
        // is documented to be.
        assert_eq!(safe.image_model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(safe.analysis_model.as_deref(), Some("opus"));
        assert_eq!(safe.image_provider.as_deref(), Some("api"));
        assert_eq!(safe.analysis_provider.as_deref(), Some("oauth"));
        assert_eq!(safe.image_gen_model.as_deref(), Some("gpt-image-2"));

        // A file with only harmless fields must NOT trigger the warning.
        let benign = LocalSettings { image_model: Some("m".into()), ..Default::default() };
        assert!(!benign.names_any_ambient_field());
    }

    /// The ambient-`.env` guard must cover every variable that decides WHERE
    /// something is sent or WHAT is executed — not just the endpoints.
    ///
    /// The first version of it named the two base URLs and stopped there,
    /// which left the strictly WORSE half of its own stated threat model open:
    /// `AUTOSHOP_CLAUDE_BIN` and `AUTOSHOP_PYTHON` reach `Command::new`
    /// verbatim (`advisor/claude.rs`, `denoise.rs`, `segment.rs`) and the two
    /// script variables become that command's argv — so the very scenario the
    /// comment describes, a `.env` inside a shared archive of photos, still
    /// yielded arbitrary process execution. `.env` is the only route to these
    /// (`LocalSettings` has no such field), so this list is the whole guard.
    ///
    /// This test reads the CONSTANT rather than the behaviour on purpose:
    /// `dotenv_override` mutates the process environment once per process, so
    /// a behavioural test here would fight every other test in the binary.
    #[test]
    fn every_variable_that_names_a_program_or_an_endpoint_is_ambient_unsafe() {
        // Grepped from this file's own resolution block; each is either a
        // base URL or lands in `Command::new`/its argv.
        let must_cover = [
            "AUTOSHOP_OPENAI_BASE_URL",
            "AUTOSHOP_ANALYSIS_BASE_URL",
            "AUTOSHOP_CLAUDE_BIN",
            "AUTOSHOP_PYTHON",
            "AUTOSHOP_DENOISE_SCRIPT",
            "AUTOSHOP_SEGMENT_SCRIPT",
            "PATH",
            "AUTOSHOP_DATA_DIR",
            "AUTOSHOP_ANALYSIS_PROVIDER",
            "AUTOSHOP_ANALYSIS_MODEL",
            "AUTOSHOP_CLAUDE_MODEL",
            "AUTOSHOP_OPENAI_MODEL",
            "AUTOSHOP_OPENAI_IMAGE_MODEL",
            "AUTOSHOP_IMAGE_PROVIDER",
        ];
        let src = include_str!("config.rs");
        // The list in `Config::load` is the guard; if a variable is resolved
        // through the live environment (`nonempty`) instead of the pre-.env
        // snapshot (`pre`), an ambient file owns it.
        for name in must_cover {
            assert!(
                !src.contains(&format!("nonempty(\"{name}\")")),
                "{name} decides an endpoint or a program to run, so it must resolve from the \
                 pre-.env snapshot, not from the live environment a .env can rewrite"
            );
        }
        // And every variable that IS resolved from the snapshot must be in the
        // documented list, so the two cannot drift apart silently.
        assert_eq!(
            must_cover.len(),
            14,
            "AMBIENT_UNSAFE_VARS has 14 entries; update both this list and the constant together"
        );
    }

    /// The guard above lives on the READ path. Saving settings is a WRITE
    /// path, and it read-merge-writes: both writers copy an incoming field
    /// only when the request names one, keep everything else from what
    /// `load_local_settings` handed them, and publish the result to the
    /// CENTRAL file. So merging onto the RAW ambient values laundered a
    /// planted endpoint into the trusted file — where `Config::load` sees
    /// `SettingsOrigin::Central` and rightly does not strip it. One ordinary
    /// "Save settings" click undid the whole guard, permanently.
    ///
    /// The merge base must therefore already be sanitised. Pinned as a
    /// simulation of the writers' exact shape (`if inc.x.is_some()`), because
    /// both live writers are behind a web route and an egui panel.
    #[test]
    fn saving_settings_cannot_launder_an_ambient_endpoint_into_the_trusted_file() {
        let ambient = LocalSettings {
            image_base_url: Some("https://attacker.example/v1".into()),
            analysis_base_url: Some("https://attacker.example/v1".into()),
            image_api_key: Some("planted".into()),
            image_model: Some("gpt-5.6-sol".into()),
            ..Default::default()
        };
        // What load_local_settings now hands a writer for an ambient file.
        let base = match (ambient.clone(), SettingsOrigin::WorkingDir) {
            (s, SettingsOrigin::WorkingDir) => s.without_ambient_authority(),
            (s, _) => s,
        };
        // The writers' merge: the user changed only their analysis model, so
        // every other field rides through from the base into the central file.
        let incoming = LocalSettings { analysis_model: Some("opus".into()), ..Default::default() };
        let mut merged = base;
        if incoming.analysis_model.is_some() {
            merged.analysis_model = incoming.analysis_model.clone();
        }
        if incoming.image_base_url.is_some() {
            merged.image_base_url = incoming.image_base_url.clone();
        }
        if incoming.image_api_key.is_some() {
            merged.image_api_key = incoming.image_api_key.clone();
        }

        assert_eq!(
            merged.image_base_url, None,
            "the ambient endpoint reached the central file, where nothing strips it again"
        );
        assert_eq!(merged.analysis_base_url, None, "…and the analysis endpoint with it");
        assert_eq!(merged.image_api_key, None, "…and the planted key");
        // The user's own edit, and the ambient file's harmless selectors,
        // still survive the save.
        assert_eq!(merged.analysis_model.as_deref(), Some("opus"));
        assert_eq!(merged.image_model.as_deref(), Some("gpt-5.6-sol"));
    }

        #[test]
        fn a_dotenv_cannot_rewrite_process_search_data_or_billing_authority() {
            const CHILD: &str = "AUTOSHOP_DOTENV_AUTHORITY_TEST_CHILD";
            if env::var_os(CHILD).is_some() {
                let path_before = env::var_os("PATH");
                let data_before = env::var_os("AUTOSHOP_DATA_DIR");
                let pythonpath_before = env::var_os("PYTHONPATH");
                let cfg = Config::load();

                assert_eq!(env::var_os("PATH"), path_before, "dotenv rewrote executable search");
                assert_eq!(
                    env::var_os("AUTOSHOP_DATA_DIR"),
                    data_before,
                    "dotenv redirected the trusted settings root"
                );
                assert_eq!(
                    env::var_os("PYTHONPATH"),
                    pythonpath_before,
                    "dotenv planted a Python import path for the sidecars to inherit"
                );
                assert_eq!(cfg.analysis_provider, "oauth", "dotenv initiated an API verifier call");
                assert_eq!(
                    cfg.analysis_api_key.as_deref(),
                    Some("dotenv-key"),
                    "ordinary secret keys in the user's dotenv remain supported"
                );
                assert!(
                    !Path::new(&cfg.denoise_script).is_relative(),
                    "the bundled-script default stayed cwd-relative: {}",
                    cfg.denoise_script
                );
                return;
            }

            let dir = env::temp_dir().join(format!(
                "autoshop-dotenv-authority-{}-{}",
                std::process::id(),
                crate::store::next_tmp_seq()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(".env"),
                "PATH=.\n\
                 AUTOSHOP_DATA_DIR=.\n\
                 PYTHONPATH=.\n\
                 AUTOSHOP_ANALYSIS_PROVIDER=api\n\
                 AUTOSHOP_ANALYSIS_API_KEY=dotenv-key\n",
            )
            .unwrap();
            let trusted_data = dir.join("trusted-data");
            let output = std::process::Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "config::tests::a_dotenv_cannot_rewrite_process_search_data_or_billing_authority",
                    "--nocapture",
                ])
                .current_dir(&dir)
                .env(CHILD, "1")
                .env("PATH", "trusted-search-path")
                .env("AUTOSHOP_DATA_DIR", &trusted_data)
                .env_remove("AUTOSHOP_ANALYSIS_PROVIDER")
                .env_remove("AUTOSHOP_ANALYSIS_API_KEY")
                .output()
                .unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            assert!(
                output.status.success(),
                "child failed:\nstdout={}\nstderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        #[test]
        fn repeated_corrupt_settings_incidents_get_distinct_rescue_files() {
            let dir = env::temp_dir().join(format!(
                "autoshop-corrupt-settings-{}-{}",
                std::process::id(),
                crate::store::next_tmp_seq()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("autoshop.local.json");

            let first = preserve_corrupt_settings(&path, b"{first").unwrap();
            let second = preserve_corrupt_settings(&path, b"{second").unwrap();
            assert_ne!(first, second, "a later incident reused the old rescue name");
            assert_eq!(std::fs::read(first).unwrap(), b"{first");
            assert_eq!(std::fs::read(second).unwrap(), b"{second");

            let _ = std::fs::remove_dir_all(dir);
        }

    // FILE: src/generative.rs  (append inside the existing `mod tests`)
}
