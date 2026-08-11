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
    // Under the cross-process settings lock, NoWait: `bytes` were read some
    // time ago, and the remove inside acts on whatever sits at `path` NOW.
    // Unguarded, a settings writer that atomically replaced the corrupt file
    // in between (both writers hold this same lock for their whole cycle) had
    // its GOOD file — keys and all — deleted by this stale rescuer. A busy
    // lock skips the rescue outright: the holder is about to overwrite the
    // corrupt file anyway, and its own load preserved the bytes first.
    crate::store::with_settings_lock(crate::store::DevelopLockMode::NoWait, || {
        rescue_if_unchanged(path, bytes)
    })
}

/// The rescue body, entered only with the settings lock held: re-verify the
/// live file still holds exactly the corrupt bytes that were read, then MOVE
/// it under a unique `.corrupt` name. A rename, never copy-then-delete
/// (Codex R12 #3): the lock serializes the app's own writers, but the file
/// is documented as hand-editable — an external editor's atomic replace
/// landing inside the microsecond verify→act window used to be DELETED
/// while the rescue copy held only the stale corrupt bytes. A rename
/// preserves whatever is live: worst case the new content lands under the
/// rescue name, which the warning prints, and nothing is ever destroyed.
fn rescue_if_unchanged(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let live = crate::store::read_bytes_capped(path, crate::store::MAX_STORE_JSON)?;
    if live != bytes {
        return Err(std::io::Error::other(
            "the settings file changed while being rescued — nothing was removed",
        ));
    }
    static CORRUPT_SEQ: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    for _ in 0..16 {
        let kept = path.with_extension(format!(
            "json.corrupt.{}-{}",
            std::process::id(),
            CORRUPT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        // Claim the name first (a crashed predecessor's recycled pid+seq
        // must not be overwritten), then rename the live file over the
        // claim — fs::rename replaces the destination on both platforms.
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        match opts.open(&kept) {
            Ok(file) => drop(file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
        // durable_rename, not bare rename (L03) and not durable_replace:
        // the parent-dir fsync keeps "preserved at <kept>" true after a
        // power cut, while replace's source-sync would open the LIVE file
        // for write — read-only or exclusively held, that fails a move
        // that used to succeed. The bytes are already on disk as-is.
        if let Err(e) = crate::store::durable_rename(path, &kept) {
            // Release the claim ONLY when the live file demonstrably did
            // not move (the settings lock is held, so this probe races no
            // cooperating writer). Once the rename itself succeeded, `kept`
            // IS the only copy of the user's keys — a finish_parent
            // failure must not have this cleanup delete the sole survivor.
            if path.exists() {
                let _ = std::fs::remove_file(&kept);
            }
            return Err(e);
        }
        // The rename carried the live file's own mode over the 0600 claim;
        // the file holds API keys, so restore the tight mode (best-effort —
        // Windows AppData is per-user already).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&kept, std::fs::Permissions::from_mode(0o600));
        }
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
        let s = match crate::store::read_text_capped(&p, crate::store::MAX_STORE_JSON) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            // Unreadable ≠ absent (over-cap, permissions): the old silent
            // skip fell through to defaults with nothing to say why the
            // user's keys stopped applying.
            Err(e) => {
                eprintln!("warning: {} cannot be read ({e}) — ignoring it", p.display());
                continue;
            }
        };
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
    // Reentrant under `update_local_settings`' own lock; a DIRECT call is a
    // lone publish and takes the lock here — no caller can slip an
    // unserialized rename between another process's load and save.
    crate::store::with_settings_lock(crate::store::DevelopLockMode::Wait, || {
        save_local_settings_unlocked(s)
    })
}

fn save_local_settings_unlocked(s: &LocalSettings) -> std::io::Result<PathBuf> {
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
    // DURABLE replace (L03): fsync the staged bytes and the parent dir
    // around the rename. tmp+rename alone left a post-crash window where
    // the live name pointed at bytes the disk never received — and this
    // file holds the API keys. The 0600 claim's mode rides the rename.
    if let Err(e) = crate::store::durable_replace(&tmp, &p) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(p)
}

/// ONE settings read-modify-write, closed against every other writer in every
/// process: load, hand the caller the merge, save — all under
/// [`crate::store::with_settings_lock`]. Both writers (serve's POST route and
/// the GUI's Settings panel) go through here; an unlocked (or in-process-only
/// locked) cycle let the OTHER process's save land between this one's load
/// and rename and erased it — the file carries the API keys. Wait mode: the
/// critical section is one small file read and rewrite.
pub fn update_local_settings(
    mutate: impl FnOnce(&mut LocalSettings),
) -> std::io::Result<PathBuf> {
    crate::store::with_settings_lock(crate::store::DevelopLockMode::Wait, || {
        let mut cur = load_local_settings();
        mutate(&mut cur);
        save_local_settings(&cur)
    })
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

// A `.env` is AMBIENT INPUT by the same argument as a working-directory
// settings file, and a stronger one: dotenvy searches the cwd and every
// parent (`find.rs`), and dotenv precedence beats a variable the user
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
pub(crate) const AMBIENT_UNSAFE_VARS: [&str; 17] = [
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

/// The `.env`, parsed ONCE per process into an OWNED map (L16#3): the
/// process environment is NEVER written. The old path ran
/// `dotenv_override` + a 17-name restore loop inside a OnceLock whose
/// safety comment claimed "the first load happens on the main thread
/// before any worker exists" — false for the GUI binary, whose `main()`
/// never calls `Config::load` and which spawns a gallery worker inside
/// the creation closure, so the `unsafe { env::set_var }` raced every
/// concurrent `getenv` (UB on unix). With the owned map the `unsafe` is
/// GONE rather than re-justified, `Config::load` is callable from any
/// thread at any time, and no `.env` name can influence the live
/// environment at all — which also closes the unlisted-name gap
/// (e.g. LOCALAPPDATA siting the store root) for free. Residual
/// unchanged: a `.env` edited mid-session applies on the next launch.
static DOTENV: std::sync::OnceLock<std::collections::HashMap<String, String>> =
    std::sync::OnceLock::new();

fn dotenv_map() -> &'static std::collections::HashMap<String, String> {
    DOTENV.get_or_init(|| {
        // dotenv_iter: the SAME cwd-upward file search as dotenv_override,
        // minus every setenv (dotenvy 0.15 lib.rs/iter.rs). take_while(ok)
        // reproduces override's error order: it applied every line BEFORE
        // the first malformed one and none after (flatten alone would skip
        // the bad line and keep applying — Codex AL F5). KNOWN, ACCEPTED
        // divergence, documented on purpose: dotenvy's $VAR interpolation
        // inside the iterator resolves against the live environment only,
        // no longer against earlier .env lines (override used to setenv as
        // it went); no .env of this project uses interpolation, and full
        // fidelity would mean vendoring the parser.
        let map: std::collections::HashMap<String, String> = dotenvy::dotenv_iter()
            .map(|it| it.take_while(Result::is_ok).flatten().collect())
            .unwrap_or_default();
        for k in AMBIENT_UNSAFE_VARS {
            // Same trigger as the old post-override comparison: the .env
            // names a protected variable with a value that differs from the
            // process's own.
            if let Some(v) = map.get(k)
                && env::var(k).ok().as_deref() != Some(v.as_str())
            {
                eprintln!(
                    "warning: ignoring {k} from a .env file — a .env found in the working \
                     directory (or any parent) is not trusted to choose where your API key \
                     is sent or which program is run. Set it in your own environment."
                );
            }
        }
        map
    })
}

/// Resolve `key` with dotenv precedence but WITHOUT environment mutation:
/// the `.env` value wins for unprotected names (the dotenv_override
/// contract this project chose on 2026-08-03 — this machine carries a
/// User-scope OPENAI_API_KEY that must not out-rank the project's own
/// .env key), the live environment is the fallback, and a PROTECTED name
/// never resolves from a `.env` at all. Empty/whitespace counts as unset
/// (an empty .env value therefore masks the process value, exactly as
/// override-then-filter did). Out-of-crate consumers of `.env`-honoured
/// names (the HTTP/sidecar timeout knobs, the legacy-out override) go
/// through here, or they silently stop seeing `.env` values.
pub fn env_or_dotenv(key: &str) -> Option<String> {
    let live = || env::var(key).ok();
    if AMBIENT_UNSAFE_VARS.contains(&key) {
        return live().filter(|s| !s.trim().is_empty());
    }
    dotenv_map()
        .get(key)
        .cloned()
        .or_else(live)
        .filter(|s| !s.trim().is_empty())
}

/// The `.env`'s UNPROTECTED entries, for a CHILD process's environment
/// block. Under dotenv_override these names sat in the process
/// environment and every child inherited them (third-party knobs like
/// HF_HOME / CUDA_VISIBLE_DEVICES / proxy variables); the owned map keeps
/// exactly that reach — `Command::envs` writes the CHILD's block, never
/// the parent's — while the protected 17 are filtered precisely as the
/// old restore loop kept them out of the parent.
pub fn dotenv_child_env() -> Vec<(String, String)> {
    dotenv_map()
        .iter()
        .filter(|(k, _)| !AMBIENT_UNSAFE_VARS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

impl Config {
    pub fn load() -> Self {
        // .env first (absence is fine; never prints the key), then the local
        // file. See `dotenv_map` / `env_or_dotenv` above for the ownership
        // and precedence story (L16#3: parsed once, owned, no setenv).
        let dotenv = dotenv_map();
        let nonempty = |k: &str| {
            dotenv
                .get(k)
                .cloned()
                .or_else(|| env::var(k).ok())
                .filter(|s| !s.trim().is_empty())
        };
        // A variable an ambient file must not own resolves from the LIVE
        // environment — which, with no setenv anywhere, IS the authority the
        // process started with. Indices follow `AMBIENT_UNSAFE_VARS`.
        let pre = |i: usize| {
            env::var(AMBIENT_UNSAFE_VARS[i]).ok().filter(|s: &String| !s.trim().is_empty())
        };
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
    /// Re-derived from the CONSTANT itself (the old hand-copied list froze
    /// at 14 entries while the constant grew to 17 — PYTHONPATH/PYTHONHOME/
    /// AUTOSHOP_DENOISE_CACHE went uncovered while the assertion message
    /// still claimed "14 entries"; the self-drift guard had drifted).
    #[test]
    fn every_variable_that_names_a_program_or_an_endpoint_is_ambient_unsafe() {
        let src = include_str!("config.rs");
        for name in super::AMBIENT_UNSAFE_VARS {
            // A protected name must never resolve through the .env-honouring
            // resolvers: `nonempty` (map-first inside load) or a stray
            // `env_or_dotenv` literal — only `pre(i)` / live env reads.
            assert!(
                !src.contains(&format!("nonempty(\"{name}\")")),
                "{name} decides an endpoint or a program to run, so it must resolve from the \
                 live (never-mutated) environment, not through the .env-honouring resolver"
            );
        }
        // env_or_dotenv itself must refuse protected names at runtime, so
        // even an out-of-crate consumer cannot hand one to a .env.
        assert!(
            src.contains("if AMBIENT_UNSAFE_VARS.contains(&key)"),
            "env_or_dotenv lost its protected-name refusal"
        );
    }

    /// L16#3: `Config::load` never writes the process environment — the old
    /// dotenv_override + restore loop did (unsafe `env::set_var`, racing
    /// every concurrent getenv on unix; its safety comment's "first load on
    /// the main thread before any worker" premise was FALSE for the GUI).
    /// Source-invariant: this file carries no set_var/remove_var at all, and
    /// the .env is consumed through dotenv_iter only.
    #[test]
    fn no_setenv_survives_on_the_config_load_path() {
        let src = include_str!("config.rs");
        // Assembled so this test's own literals don't match themselves.
        for (prefix, name) in
            [("env::", "set_var"), ("env::", "remove_var"), ("dotenvy::", "dotenv_override")]
        {
            let call = format!("{prefix}{name}(");
            assert!(
                !src.contains(&call),
                "config.rs regained a process-environment write ({name})"
            );
        }
        assert!(src.contains("dotenv_iter"), "the owned-map parse is gone");
    }

    /// L16#5: the out-of-config consumers of `.env`-honoured knobs resolve
    /// through `env_or_dotenv` — a direct `env::var` there silently stops
    /// seeing `.env` values now that nothing writes the environment. The
    /// child-side reach survives via `dotenv_child_env` on all three spawns.
    #[test]
    fn out_of_config_consumers_still_honour_a_dotenv_knob() {
        let consumers: [(&str, &str, &str); 4] = [
            ("advisor/mod.rs", include_str!("advisor/mod.rs"), "AUTOSHOP_HTTP_TIMEOUT_SECS"),
            ("advisor/claude.rs", include_str!("advisor/claude.rs"), "AUTOSHOP_HTTP_TIMEOUT_SECS"),
            ("denoise.rs", include_str!("denoise.rs"), "AUTOSHOP_SIDECAR_TIMEOUT_SECS"),
            ("store.rs", include_str!("store.rs"), "AUTOSHOP_LEGACY_OUT"),
        ];
        for (file, src, knob) in consumers {
            assert!(
                !src.contains(&format!("env::var(\"{knob}\")"))
                    && !src.contains(&format!("env::var_os(\"{knob}\")")),
                "{file}: {knob} is read directly from the environment — .env values are lost"
            );
            assert!(
                src.contains(&format!("env_or_dotenv(\"{knob}\")")),
                "{file}: {knob} no longer resolves through env_or_dotenv"
            );
        }
        for (file, src) in [
            ("advisor/claude.rs", include_str!("advisor/claude.rs")),
            ("denoise.rs", include_str!("denoise.rs")),
            ("segment.rs", include_str!("segment.rs")),
        ] {
            assert!(
                src.contains("dotenv_child_env()"),
                "{file}: the child lost the .env's unprotected names"
            );
        }
    }

    /// L16: `Config::load` is callable from any thread at any time — the
    /// whole point of the owned map. Eight threads interleave loads with
    /// PATH reads; PATH must be byte-identical afterwards (under the old
    /// code the restore loop rewrote it concurrently with the readers).
    #[test]
    fn config_load_is_reentrant_from_many_threads() {
        let before = env::var_os("PATH");
        let threads: Vec<_> = (0..8)
            .map(|i| {
                std::thread::spawn(move || {
                    for _ in 0..10 {
                        if i % 2 == 0 {
                            let _ = Config::load();
                        } else {
                            let _ = env::var_os("PATH");
                        }
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(env::var_os("PATH"), before, "a load mutated the environment");
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

        /// L16#3, the owned-map contract end to end (child process, like the
        /// authority test above — environment assertions must not race the
        /// rest of the suite): parsing the .env mutates NOTHING — not even
        /// UNPROTECTED names enter the process environment — while dotenv
        /// precedence still holds (`.env` beats a process-set unprotected
        /// var) and `env_or_dotenv` serves the map to out-of-config
        /// consumers.
        #[test]
        fn dotenv_parsing_never_mutates_the_process_environment() {
            const CHILD: &str = "AUTOSHOP_DOTENV_OWNEDMAP_TEST_CHILD";
            if env::var_os(CHILD).is_some() {
                let cfg = Config::load();
                // (a) The unprotected .env name is in the MAP, not the env.
                assert_eq!(
                    env::var_os("AUTOSHOP_DENOISE_MODEL"),
                    Some("env-model".into()),
                    "parsing the .env mutated an UNPROTECTED process variable"
                );
                // (b) …and the .env still WINS for config resolution — the
                // dotenv_override precedence this project chose (a machine-
                // wide var must not out-rank the project's own .env).
                assert_eq!(cfg.denoise_model, "dotenv-model");
                // (c) out-of-config consumers see the map through the shared
                // resolver, protected names never.
                assert_eq!(
                    super::env_or_dotenv("AUTOSHOP_SIDECAR_TIMEOUT_SECS").as_deref(),
                    Some("123"),
                    "env_or_dotenv lost the .env value"
                );
                assert_ne!(
                    super::env_or_dotenv("PATH").as_deref(),
                    Some("."),
                    "a protected name resolved from the .env"
                );
                // (d) …and the child block for sidecars carries the
                // unprotected names, minus every protected one.
                let child_env = super::dotenv_child_env();
                assert!(
                    child_env.iter().any(|(k, v)| k == "AUTOSHOP_SIDECAR_TIMEOUT_SECS" && v == "123"),
                    "the sidecar child block lost the .env's unprotected names"
                );
                assert!(
                    !child_env.iter().any(|(k, _)| k == "PATH" || k == "PYTHONPATH"),
                    "a protected name leaked into the sidecar child block"
                );
                return;
            }

            let dir = env::temp_dir().join(format!(
                "autoshop-dotenv-ownedmap-{}-{}",
                std::process::id(),
                crate::store::next_tmp_seq()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(".env"),
                "AUTOSHOP_DENOISE_MODEL=dotenv-model\n\
                 AUTOSHOP_SIDECAR_TIMEOUT_SECS=123\n\
                 PATH=.\n",
            )
            .unwrap();
            let output = std::process::Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "config::tests::dotenv_parsing_never_mutates_the_process_environment",
                    "--nocapture",
                ])
                .current_dir(&dir)
                .env(CHILD, "1")
                // A process-set UNPROTECTED var the .env must beat — and
                // whose value must survive in the environment untouched.
                .env("AUTOSHOP_DENOISE_MODEL", "env-model")
                .env_remove("AUTOSHOP_SIDECAR_TIMEOUT_SECS")
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

            // The live file carries the corrupt bytes each time — the
            // rescue re-verifies before it removes (the race test below).
            std::fs::write(&path, b"{first").unwrap();
            let first = rescue_if_unchanged(&path, b"{first").unwrap();
            std::fs::write(&path, b"{second").unwrap();
            let second = rescue_if_unchanged(&path, b"{second").unwrap();
            assert_ne!(first, second, "a later incident reused the old rescue name");
            assert_eq!(std::fs::read(first).unwrap(), b"{first");
            assert_eq!(std::fs::read(second).unwrap(), b"{second");
            assert!(!path.exists(), "a verified rescue consumes the live file");

            let _ = std::fs::remove_dir_all(dir);
        }

        /// L01: the rescue acts on whatever sits at the path NOW. If another
        /// process replaced the corrupt file between the loader's read and
        /// this rescue — settings writers hold the same lock for their whole
        /// cycle, so this models the stale-rescuer side — the live file must
        /// survive untouched and no rescue copy may be minted.
        #[test]
        fn a_replaced_settings_file_survives_a_stale_rescuer() {
            let dir = env::temp_dir().join(format!(
                "autoshop-corrupt-settings-race-{}-{}",
                std::process::id(),
                crate::store::next_tmp_seq()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("autoshop.local.json");
            std::fs::write(&path, b"{\"analysis_model\":\"good\"}").unwrap();

            let res = rescue_if_unchanged(&path, b"{corrupt bytes read earlier");
            assert!(res.is_err(), "a changed live file must refuse the rescue");
            assert_eq!(
                std::fs::read(&path).unwrap(),
                b"{\"analysis_model\":\"good\"}",
                "the replacement file was deleted by a stale rescuer"
            );
            let minted = std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().contains("corrupt"))
                .count();
            assert_eq!(minted, 0, "no rescue copy may claim bytes that are no longer live");

            let _ = std::fs::remove_dir_all(dir);
        }
}
