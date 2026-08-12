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
///
/// `PartialEq` is load-bearing: [`LocalSettings::names_beyond`] asks whether
/// restricting a file to what its source may supply CHANGED it, so the
/// "warn about what was ignored" check can never drift from the stripping
/// itself (they used to be two hand-maintained field lists).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalSettings {
    pub analysis_provider: Option<String>,
    pub analysis_model: Option<String>,
    pub analysis_api_key: Option<String>,
    /// The endpoint `analysis_api_key` was saved FOR (see `image_api_key_base`).
    pub analysis_api_key_base: Option<String>,
    pub analysis_base_url: Option<String>,
    /// Reasoning effort for the analysis role. Absent ⇒ the environment may
    /// still supply a tier; explicitly `""` ⇒ "send no effort parameter",
    /// silencing the environment too (see [`explicit_or_env`]).
    pub analysis_effort: Option<String>,
    pub image_api_key: Option<String>,
    /// The endpoint `image_api_key` was saved FOR — stamped by both Settings
    /// writers when a key is typed, consulted by [`Config::load`]: a stored
    /// credential authenticates ONE endpoint, and a later provider flip or
    /// base-URL edit must not silently re-route it (the GUI's OAuth flip
    /// swaps the base to a local bridge while the cloud key stayed armed).
    /// Absent ⇒ a pre-v0.23.2 save; the key is allowed anywhere until the
    /// next save records a home. Not a secret and not an endpoint CHOICE —
    /// its only power is to WITHHOLD a key, so it needs no [`SETTINGS`] row.
    pub image_api_key_base: Option<String>,
    pub image_model: Option<String>,
    pub image_base_url: Option<String>,
    pub image_gen_model: Option<String>,
    /// Reasoning effort for the image (vision proposer) role. Same
    /// absent-vs-explicitly-blank semantics as `analysis_effort`.
    pub image_effort: Option<String>,
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
/// breadcrumb: the sources do not deserve the same authority.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsOrigin {
    /// [`local_settings_path`] under a PER-USER store root. The user's own.
    Central,
    /// [`local_settings_path`] under the shared `<temp>/autoshop` fallback
    /// ([`crate::store::RootTrust::SharedFallback`]) — the right PATH, but a
    /// directory every account on the machine can write, so whoever created
    /// the file first is not necessarily this user. Treated as ambient.
    SharedRoot,
    /// A cwd-relative `autoshop.local.json`. AMBIENT: whatever directory the
    /// app happens to be launched from supplies it.
    WorkingDir,
    /// No file was read.
    None,
}

impl SettingsOrigin {
    /// What this origin is allowed to decide. Both ambient origins map to the
    /// same capability — a file nobody can prove the user wrote.
    pub(crate) fn source(self) -> Source {
        match self {
            SettingsOrigin::Central | SettingsOrigin::None => Source::Trusted,
            SettingsOrigin::SharedRoot | SettingsOrigin::WorkingDir => Source::WorkingDirFile,
        }
    }
}

impl LocalSettings {
    /// Drop every field `src` is not allowed to choose.
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
    /// WHICH fields survive is no longer restated here: it is read off
    /// [`SETTINGS`], the one table that also decides what a `.env` may set and
    /// what reaches a child process. Three hand-kept lists used to encode this
    /// single policy in three vocabularies and drifted apart.
    fn restricted_to(mut self, src: Source) -> Self {
        for s in SETTINGS {
            if let Some(field) = s.field
                && !src.may_supply(s.trust)
            {
                *field(&mut self) = None;
            }
        }
        self
    }

    /// Does this file name anything `src` may not supply? Answered by asking
    /// whether the restriction above CHANGES it, so the warning and the
    /// stripping can never disagree.
    fn names_beyond(&self, src: Source) -> bool {
        self.clone().restricted_to(src) != *self
    }

    /// Keep each stored key's HOME (`*_api_key_base`) coherent after a
    /// settings mutation. Runs inside [`update_local_settings`], so BOTH
    /// writers (the GUI form and `serve.rs`'s POST) inherit one rule:
    ///
    /// - A key the writer just typed was stamped BY the writer with the base
    ///   on screen beside it — an existing stamp is never second-guessed.
    /// - A pre-v0.23.2 key with no home inherits the current base, but only
    ///   from a save that did NOT change the base: a base-changing save is
    ///   exactly the re-route this stamp exists to catch, and blessing the
    ///   new pairing there would launder the old key to the new endpoint.
    ///   (Residual, documented: such a key stays home-less — and therefore
    ///   allowed anywhere — until a base-stable save or a re-type.)
    /// - No key ⇒ no home.
    fn reconcile_key_homes(&mut self, before: &LocalSettings) {
        fn one(
            key: &Option<String>,
            home: &mut Option<String>,
            base: &Option<String>,
            before_home: &Option<String>,
            before_base: &Option<String>,
        ) {
            if key.is_none() {
                *home = None;
            } else if home.is_none() && before_home.is_none() && base == before_base {
                *home = base.clone().filter(|s| !s.trim().is_empty());
            }
        }
        one(
            &self.image_api_key,
            &mut self.image_api_key_base,
            &self.image_base_url,
            &before.image_api_key_base,
            &before.image_base_url,
        );
        one(
            &self.analysis_api_key,
            &mut self.analysis_api_key_base,
            &self.analysis_base_url,
            &before.analysis_api_key_base,
            &before.analysis_base_url,
        );
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

/// How a settings file should be NAMED in a diagnostic: its file name plus
/// the ROLE of the folder it sits in. Never the absolute path — see the call
/// site for why (stderr leaks the account name and profile layout).
fn file_label(p: &Path, origin: SettingsOrigin) -> String {
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "autoshop.local.json".to_string());
    let place = match origin {
        SettingsOrigin::Central => "in your Autoshop data folder",
        SettingsOrigin::SharedRoot => "in the shared temp Autoshop folder",
        SettingsOrigin::WorkingDir => "in the current working directory",
        SettingsOrigin::None => "",
    };
    if place.is_empty() { name } else { format!("{name} ({place})") }
}

pub fn load_local_settings_from() -> (LocalSettings, SettingsOrigin) {
    // The central path is only CENTRAL when the root it sits under is
    // per-account. `<temp>/autoshop` is the last-resort root and is writable
    // by every local account, so a settings file found there gets ambient
    // authority — the same treatment as one found in the working directory.
    let (root, trust) = crate::store::store_root_with_trust();
    let central = match trust {
        crate::store::RootTrust::PerUser => SettingsOrigin::Central,
        crate::store::RootTrust::SharedFallback => SettingsOrigin::SharedRoot,
    };
    debug_assert!(local_settings_path().starts_with(&root));
    for (p, origin) in [
        (local_settings_path(), central),
        (PathBuf::from("autoshop.local.json"), SettingsOrigin::WorkingDir),
    ] {
        // These warnings go to stderr — into logs, screenshots and pasted bug
        // reports — so they name the FILE and the FOLDER ROLE, never the full
        // path: `%LOCALAPPDATA%\autoshop\…` spells out the account name and
        // the profile layout. Anyone who needs the real location has it on
        // screen: the Settings panel prints the store root.
        let file = file_label(&p, origin);
        let s = match crate::store::read_text_capped(&p, crate::store::MAX_STORE_JSON) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            // Unreadable ≠ absent (over-cap, permissions): the old silent
            // skip fell through to defaults with nothing to say why the
            // user's keys stopped applying.
            Err(e) => {
                eprintln!("warning: {file} cannot be read ({e}) — ignoring it");
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
                    "warning: {file} is not valid JSON ({e}) — ignoring it{}",
                    match kept.as_deref().and_then(std::path::Path::file_name) {
                        Some(n) => format!(
                            "; your settings were preserved beside it as {}",
                            n.to_string_lossy()
                        ),
                        None => String::new(),
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
    let (s, origin) = load_local_settings_from();
    s.restricted_to(origin.source())
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
        let before = cur.clone();
        mutate(&mut cur);
        // One rule for both writers: see `reconcile_key_homes`.
        cur.reconcile_key_homes(&before);
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
    /// Reasoning effort for the image role, or `None` to send no such
    /// parameter and let the provider pick (the pre-v0.23.2 behaviour, and
    /// the only correct request for a model that does not reason at all).
    /// Validated by [`effort`]; the wire spelling differs per endpoint family
    /// and is applied in `advisor::post_ai_json`.
    pub image_effort: Option<String>,

    // --- analysis role: the verifier (oauth = claude CLI, or api = OpenAI) -----
    /// `"oauth"` (default; the `claude` CLI) or `"api"` (OpenAI-compatible chat).
    pub analysis_provider: String,
    /// Model for the analysis role: a `claude` alias/id for oauth (default
    /// `opus`), or a chat model id for api.
    pub analysis_model: String,
    /// Reasoning effort for the analysis role, or `None` for the provider's
    /// own default. `oauth` passes it as the `claude` CLI's `--effort`
    /// (`low|medium|high|xhigh|max`, measured from `claude --help`); `api`
    /// passes the OpenAI-compatible spelling. See [`effort`].
    pub analysis_effort: Option<String>,
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

// --- the capability table ---------------------------------------------------
//
// ONE policy, declared once. Everything that used to be a hand-kept list —
// which `.env` names are refused, which settings-file fields an ambient file
// loses, which names are withheld from a child process, and which warnings
// fire — is derived from [`SETTINGS`] below.
//
// The three lists it replaces drifted, provably: the guard's own test carried
// a copied 14-name array while the constant had grown to 17, so PYTHONPATH,
// PYTHONHOME and the weight cache went unchecked. Worse, `Config::load` read
// that array BY INDEX (`pre(11)` was AUTOSHOP_OPENAI_MODEL), so inserting or
// removing a single name silently repointed unrelated config fields at the
// wrong variable — which is exactly what narrowing the list required.

/// What a setting DECIDES. This, and only this, determines who may set it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Trust {
    /// Authenticates and bills the user for Autoshop's OWN calls. A planted
    /// one spends a stranger's money on the user's work.
    Secret,
    /// Names WHERE bytes are sent, WHICH account pays, or WHAT program runs:
    /// endpoints, executables, script paths, search paths, the store root, a
    /// child's credentials. A planted one is exfiltration or code execution,
    /// not a preference. `AUTOSHOP_CLAUDE_BIN` and `AUTOSHOP_PYTHON` reach
    /// `Command::new` verbatim (`advisor/claude.rs`, `denoise.rs`,
    /// `segment.rs`) and the script variables become that command's argv.
    Destination,
    /// Picks WHICH model / provider / tuning number. Carries no credential and
    /// no address; the worst a planted one can do is choose a model the user
    /// did not want — visible in Settings and in every rationale.
    Preference,
}

/// Where a value came from, and therefore what it may decide.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Source {
    /// The live process environment, or the settings file under a per-user
    /// store root. The authority the process was started with.
    Trusted,
    /// A `.env`. AMBIENT — dotenvy searches the cwd and every parent
    /// (`find.rs`) — yet also where this project's own key lives.
    DotEnv,
    /// A settings file nobody can prove the user wrote: cwd-relative, or under
    /// the shared `<temp>` root.
    WorkingDirFile,
}

impl Source {
    /// The whole trust policy, in one match.
    pub(crate) fn may_supply(self, t: Trust) -> bool {
        match self {
            Source::Trusted => true,
            // A `.env` keeps SECRETS on purpose: it is where this project's
            // own key lives, and reversing that would break the documented
            // contract (README). The half it loses is Destination — the half
            // that turns a planted file into exfiltration or process
            // execution. A planted key bills the planter, and every Responses
            // call sends `store: false`, so the user's photos do not persist
            // in that account (advisor/openai.rs).
            //
            // Model and provider names are Preference and therefore ALLOWED
            // (user decision, 2026-08-11), which restores what README and
            // ARCHITECTURE §3 have always promised. Note the boundary this
            // draws: a photo pack's `.env` can now flip
            // `AUTOSHOP_ANALYSIS_PROVIDER` from `oauth` to `api` — but the
            // base URL and the key it would need stay Destination/…, so the
            // call still goes to the user's own endpoint with the user's own
            // key. That is a model choice, not endpoint redirection.
            Source::DotEnv => t != Trust::Destination,
            // The working-directory / shared-root file gets no exception: the
            // app writes the CENTRAL file, so this one's only legitimate role
            // is the pre-v0.13 migration affordance.
            Source::WorkingDirFile => t == Trust::Preference,
        }
    }
}

/// One configurable setting: the environment variable that carries it, what it
/// decides, and — when the Settings UI can write it — the [`LocalSettings`]
/// field it binds to.
pub(crate) struct Setting {
    pub(crate) env: &'static str,
    pub(crate) trust: Trust,
    field: Option<fn(&mut LocalSettings) -> &mut Option<String>>,
}

const fn env_only(env: &'static str, trust: Trust) -> Setting {
    Setting { env, trust, field: None }
}

const fn bound(
    env: &'static str,
    trust: Trust,
    field: fn(&mut LocalSettings) -> &mut Option<String>,
) -> Setting {
    Setting { env, trust, field: Some(field) }
}

/// Every setting this program resolves, with what it decides.
///
/// A name absent from this table is `Preference` by default — that is the
/// pass-through third-party knobs depend on (HF_HOME, CUDA_VISIBLE_DEVICES,
/// proxy variables) and it preserves the `.env` contract for anything not
/// listed. New Autoshop settings must be added here; the tests below fail if
/// this file names an `AUTOSHOP_*` variable the table does not classify.
pub(crate) const SETTINGS: &[Setting] = &[
    // --- image role: the vision proposer + generative edits ------------------
    bound("OPENAI_API_KEY", Trust::Secret, |s| &mut s.image_api_key),
    bound("AUTOSHOP_OPENAI_BASE_URL", Trust::Destination, |s| &mut s.image_base_url),
    bound("AUTOSHOP_OPENAI_MODEL", Trust::Preference, |s| &mut s.image_model),
    bound("AUTOSHOP_OPENAI_IMAGE_MODEL", Trust::Preference, |s| &mut s.image_gen_model),
    bound("AUTOSHOP_IMAGE_PROVIDER", Trust::Preference, |s| &mut s.image_provider),
    bound("AUTOSHOP_IMAGE_EFFORT", Trust::Preference, |s| &mut s.image_effort),
    env_only("AUTOSHOP_IMAGE_QUALITY", Trust::Preference),
    env_only("AUTOSHOP_IMAGE_MAX_PX", Trust::Preference),
    // --- analysis role: the verifier -----------------------------------------
    bound("AUTOSHOP_ANALYSIS_API_KEY", Trust::Secret, |s| &mut s.analysis_api_key),
    bound("AUTOSHOP_ANALYSIS_BASE_URL", Trust::Destination, |s| &mut s.analysis_base_url),
    bound("AUTOSHOP_ANALYSIS_PROVIDER", Trust::Preference, |s| &mut s.analysis_provider),
    bound("AUTOSHOP_ANALYSIS_MODEL", Trust::Preference, |s| &mut s.analysis_model),
    bound("AUTOSHOP_ANALYSIS_EFFORT", Trust::Preference, |s| &mut s.analysis_effort),
    env_only("AUTOSHOP_CLAUDE_MODEL", Trust::Preference), // legacy alias for ANALYSIS_MODEL
    env_only("AUTOSHOP_CLAUDE_BIN", Trust::Destination),  // Command::new
    // --- python sidecars ------------------------------------------------------
    env_only("AUTOSHOP_PYTHON", Trust::Destination), // Command::new
    env_only("AUTOSHOP_DENOISE_SCRIPT", Trust::Destination), // that command's argv
    env_only("AUTOSHOP_SEGMENT_SCRIPT", Trust::Destination),
    // A redirected weight cache is a poisoned-model path.
    env_only("AUTOSHOP_DENOISE_CACHE", Trust::Destination),
    env_only("AUTOSHOP_DENOISE_MODEL", Trust::Preference),
    // --- store, tuning knobs ---------------------------------------------------
    // Sites the TRUSTED settings file itself, so it decides where the key is
    // read from — the root of the whole trust story.
    env_only("AUTOSHOP_DATA_DIR", Trust::Destination),
    env_only("AUTOSHOP_STYLE_STRENGTH", Trust::Preference),
    env_only("AUTOSHOP_HTTP_TIMEOUT_SECS", Trust::Preference),
    env_only("AUTOSHOP_SIDECAR_TIMEOUT_SECS", Trust::Preference),
    // Names a directory to READ pre-v0.13 sidecars from during an explicitly
    // user-started import. It writes nothing and runs nothing, so it stays a
    // preference — the strictest reading would make it Destination, but that
    // would silently break the documented `.env` migration knob.
    env_only("AUTOSHOP_LEGACY_OUT", Trust::Preference),
    // --- foreign names this process or its children obey ------------------------
    env_only("PATH", Trust::Destination),
    // Both Python sidecars inherit the environment, and a `.env`'s
    // `PYTHONPATH=.` beside a hostile `numpy.py` is code execution at import
    // time (the sidecars also pass `-E` — defence in both layers).
    env_only("PYTHONPATH", Trust::Destination),
    env_only("PYTHONHOME", Trust::Destination),
    // The `claude` child's own routing. A credential is the routing decision
    // here: with ANTHROPIC_API_KEY present the CLI bills metered API credits
    // instead of the user's subscription (measured 2026-07-17, see
    // advisor/claude.rs), and ANTHROPIC_BASE_URL repoints the verifier
    // outright. Destination, so `.env` values never reach the child.
    env_only("ANTHROPIC_API_KEY", Trust::Destination),
    env_only("ANTHROPIC_AUTH_TOKEN", Trust::Destination),
    env_only("ANTHROPIC_BASE_URL", Trust::Destination),
];

/// What `name` decides. Unlisted ⇒ [`Trust::Preference`] (see [`SETTINGS`]).
pub(crate) fn trust_of(name: &str) -> Trust {
    SETTINGS
        .iter()
        .find(|s| s.env == name)
        .map_or(Trust::Preference, |s| s.trust)
}

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
        // Warn only about what is actually being ignored. Same trigger as the
        // old post-override comparison — the .env names something it may not
        // supply, with a value that differs from the process's own — but the
        // set is now derived, so narrowing the policy narrows the warning in
        // the same edit. Model and provider names no longer trip it: warning
        // about a documented, legitimate `.env` model choice was noise that
        // contradicted README and ARCHITECTURE §3.
        for (k, v) in &map {
            if !Source::DotEnv.may_supply(trust_of(k))
                && env::var(k).ok().as_deref() != Some(v.as_str())
            {
                eprintln!(
                    "warning: ignoring {k} from a .env file — a .env found in the working \
                     directory (or any parent) is not trusted to choose where your API key \
                     is sent, which account pays, or which program is run. Set it in your \
                     own environment. (Model, provider and tuning values from a .env do \
                     still apply.)"
                );
            }
        }
        map
    })
}

/// Resolve `key` with dotenv precedence but WITHOUT environment mutation:
/// the `.env` value wins for names it may supply (the dotenv_override
/// contract this project chose on 2026-08-03 — this machine carries a
/// User-scope OPENAI_API_KEY that must not out-rank the project's own
/// .env key), the live environment is the fallback, and a `Destination`
/// name never resolves from a `.env` at all. Empty/whitespace counts as
/// unset (an empty .env value therefore masks the process value, exactly as
/// override-then-filter did). Out-of-crate consumers of `.env`-honoured
/// names (the HTTP/sidecar timeout knobs, the legacy-out override) go
/// through here, or they silently stop seeing `.env` values.
pub fn env_or_dotenv(key: &str) -> Option<String> {
    resolve_env(key)
}

/// THE environment resolver — one function for every name, protected or not.
/// `Config::load` used to carry two more (`nonempty` for `.env`-honoured
/// names, and a `pre(i)` closure that indexed the protected array
/// POSITIONALLY), which is how moving a name between the two policies could
/// silently repoint an unrelated config field at the wrong variable.
pub(crate) fn resolve_env(name: &str) -> Option<String> {
    let live = env::var(name).ok();
    if Source::DotEnv.may_supply(trust_of(name)) {
        dotenv_map().get(name).cloned().or(live)
    } else {
        live
    }
    .filter(|s| !s.trim().is_empty())
}

/// Foreign environment names a `.env` may push INTO a child process.
///
/// An ALLOWLIST, and it has to be one. [`Trust`] classifies AUTOSHOP's own
/// settings — a CLOSED set, where "not in the table" means "not a setting of
/// ours", so defaulting to `Preference` is safe. A child's environment is an
/// OPEN set, where "not in the table" includes every loader and interpreter
/// hook the platform defines. Reusing the same predicate for both answered the
/// second question with the first one's default: a photo pack's `.env` saying
/// `LD_PRELOAD=./evil.so` rode into both Python sidecars, and `ld.so` loads
/// that library before `-E` — which only filters `PYTHON*` — has any say.
/// (Pre-existing: the 17-name denylist this table replaced did not list it
/// either. Codex R12-BA #2 named the shared cause.)
///
/// Enumerating `LD_PRELOAD`, `LD_AUDIT`, `DYLD_INSERT_LIBRARIES`,
/// `NODE_OPTIONS`, `GCONV_PATH` … is enumerating the attacker's options. This
/// list enumerates OURS, and the membership rule is the same three-way test
/// the table uses: a name qualifies only if it selects COMPUTE BEHAVIOUR —
/// no path, no endpoint, no credential, nothing that loads code. That
/// deliberately excludes the cache knobs the old comment advertised
/// (`HF_HOME`, `TORCH_HOME`): a redirected cache is a poisoned-model path,
/// which is exactly why `AUTOSHOP_DENOISE_CACHE` is `Destination`. It equally
/// excludes the proxy variables: a proxy decides where bytes go.
///
/// The reach this costs is small and recoverable, because a child INHERITS the
/// parent's environment (nothing here calls `env_clear`): a user's own
/// `HF_HOME` or `HTTPS_PROXY` still reaches the sidecars untouched. Only a
/// `.env`'s attempt to ADD or OVERRIDE one is refused — and refused out loud.
const CHILD_ENV_PASSTHROUGH: &[&str] = &[
    // torch / CUDA device selection and allocator tuning. Numbers and device
    // indices; no filesystem path, no host.
    "CUDA_VISIBLE_DEVICES",
    "CUDA_DEVICE_ORDER",
    "PYTORCH_CUDA_ALLOC_CONF",
    // Thread-count knobs every BLAS/OpenMP stack under torch reads.
    "OMP_NUM_THREADS",
    "MKL_NUM_THREADS",
    "NUMEXPR_NUM_THREADS",
    "TOKENIZERS_PARALLELISM",
    // Booleans that only say "do not go online" — strictly more restrictive
    // than the default, and they name nothing.
    "HF_HUB_OFFLINE",
    "TRANSFORMERS_OFFLINE",
];

/// The `.env` entries a CHILD process may receive.
///
/// Under `dotenv_override` these names sat in the process environment and
/// every child inherited them; the owned map reproduces that reach on the
/// child's own block (`Command::envs` writes the CHILD's block, never the
/// parent's) — but only for [`CHILD_ENV_PASSTHROUGH`]. Everything else the
/// `.env` names is dropped, with one warning naming what was dropped: a
/// silently-ignored knob is the failure mode that made the old pass-through
/// feel harmless.
pub fn dotenv_child_env() -> Vec<(String, String)> {
    static WARNED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    let map = dotenv_map();
    let (kept, dropped): (Vec<_>, Vec<_>) = map
        .iter()
        .partition(|(k, _)| CHILD_ENV_PASSTHROUGH.contains(&k.as_str()));
    if !dropped.is_empty() {
        WARNED.get_or_init(|| {
            // The table's own refusals already warned when they were parsed
            // (see `dotenv_map`); this names the rest, so a `.env` knob that
            // stops reaching a sidecar says so instead of just not working.
            let mut names: Vec<&str> = dropped.iter().map(|(k, _)| k.as_str()).collect();
            names.sort_unstable();
            eprintln!(
                "note: these .env names are not passed to the AI/denoise/segment child \
                 processes — a .env is ambient input and a child's environment is where a \
                 planted one becomes code execution: {}. Set them in your own environment \
                 instead; children inherit it.",
                names.join(", ")
            );
        });
    }
    kept.into_iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// Normalise a reasoning-effort value from any source into something safe on
/// a JSON body AND on a child process's argv.
///
/// The vocabulary is deliberately NOT a closed set: `claude --help` documents
/// `low|medium|high|xhigh|max` (measured 2026-08-11) while OpenAI-compatible
/// endpoints carry their own tiers and third-party bridges add more — pinning
/// a list here would make this app the bottleneck on someone else's roadmap.
/// The value is BOUNDED instead: lowercase ASCII, digits, `-`/`_`, at most 32
/// bytes, never leading `-`. An endpoint that does not know the tier answers
/// 400 and the caller negotiates it away (`advisor::post_ai_json`). The bound
/// is the part that matters — this string reaches `Command::args`, where an
/// unbounded `.env`-supplied value would be argv injection.
/// Two API base URLs name the same endpoint, ignoring whitespace and a
/// trailing slash. ONE definition: the GUI's pick-list invalidation and the
/// key-home check below must never disagree about what "same" means.
pub fn same_endpoint(a: &str, b: &str) -> bool {
    a.trim().trim_end_matches('/') == b.trim().trim_end_matches('/')
}

/// Resolve an optional setting where an EXPLICIT empty is itself a choice.
///
/// Both Settings surfaces store a cleared effort field as `""` — "let the
/// provider decide" — but the generic `pick_opt` filters empties out before
/// consulting the environment, so with `AUTOSHOP_*_EFFORT` set the one choice
/// the field exists to express was the one choice it could not make: the
/// blank fell through, the env tier came back, and Settings showed the tier
/// the user had just cleared. Absent (the field was never saved) still defers
/// to the environment.
fn explicit_or_env(file: &Option<String>, env: Option<String>) -> Option<String> {
    match file {
        Some(s) if s.trim().is_empty() => None,
        Some(s) => Some(s.clone()),
        None => env,
    }
}

/// The FILE-stored key for a role — but only at the endpoint it was saved
/// for. A stored credential authenticates one endpoint; when the resolved
/// base has moved (a provider flip's auto-swap, a typed edit, another
/// surface's save), sending the old key there is silent credential
/// misrouting — a cloud key handed to whatever listens on a local bridge
/// port, or a bridge gate token handed to the cloud. A key with no recorded
/// home (a pre-v0.23.2 save) stays allowed until a save records one.
/// Environment keys never pass through here: the env contract has no home
/// concept and is the user's own explicit pairing.
fn file_key_for(
    key: &Option<String>,
    home: &Option<String>,
    resolved_base: &str,
    name: &str,
) -> Option<String> {
    let k = key.clone()?;
    match home {
        Some(h) if !same_endpoint(h, resolved_base) => {
            eprintln!(
                "warning: not sending {name} — it was saved for {h}, but the configured \
                 endpoint is now {resolved_base}. Enter a key for this endpoint in Settings \
                 (or re-save the old one there); requests run without a key until then."
            );
            None
        }
        _ => Some(k),
    }
}

pub(crate) fn effort(v: Option<String>) -> Option<String> {
    let v = v?.trim().to_ascii_lowercase();
    if v.is_empty() {
        return None;
    }
    let shaped = v.len() <= 32
        && !v.starts_with('-')
        && v.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if !shaped {
        eprintln!(
            "warning: ignoring an unusable reasoning-effort value — expected a short tier \
             name such as low / medium / high"
        );
        return None;
    }
    Some(v)
}

/// Can `k` appear in an `Authorization` header value at all?
///
/// Visible ASCII only. RFC 9110 field values also allow SP and HTAB, but no
/// real key contains either — and a space or a newline is exactly what a bad
/// copy/paste adds. Shared with `openai_models::list_models`, which faces the
/// same ureq diagnostic on the `GET /models` probe.
pub(crate) fn key_is_header_safe(k: &str) -> bool {
    !k.is_empty() && k.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// An API key as it will appear in an `Authorization` header, or `None`.
///
/// Trims surrounding whitespace — a key copied out of a web page carries a
/// trailing newline more often than not — and then refuses any remaining byte
/// that cannot appear in a header value.
///
/// Both halves are load-bearing, and this is the ROOT fix for a key leak:
/// ureq builds the header eagerly and its rejection diagnostic quotes the
/// WHOLE header line back, so `Authorization: Bearer <the real key>` ends up
/// inside a `Transport` error (ureq 2.12.1 `header.rs:147`). That error string
/// travels into rationale text, the Settings status line, and any log the user
/// pastes into a bug report. Refusing the malformed key HERE means the
/// diagnostic never exists; redacting at each error site is the second layer.
/// Never prints the key itself.
fn header_safe_key(v: Option<String>, name: &str) -> Option<String> {
    let t = v?.trim().to_string();
    if t.is_empty() {
        return None;
    }
    if !key_is_header_safe(&t) {
        eprintln!(
            "warning: ignoring {name} — it contains characters that cannot appear in an HTTP \
             header (a stray newline or space from a copy/paste?). Re-copy the key and set it \
             again. Requests will run without a key until then."
        );
        return None;
    }
    Some(t)
}

impl Config {
    pub fn load() -> Self {
        // Every environment name — protected or not — goes through the ONE
        // resolver, which applies the capability table per name (.env value
        // first for what a .env may supply, live environment otherwise). The
        // two closures this replaces (`nonempty` and a positional `pre(i)`
        // into the protected array) encoded the same policy twice, by hand.
        let env_val = resolve_env;
        let (local, origin) = load_local_settings_from();
        // An ambient settings file may pick models, never a key or the
        // endpoint a key is sent to. See `LocalSettings::restricted_to`.
        let src = origin.source();
        if src != Source::Trusted && local.names_beyond(src) {
            let where_ = if origin == SettingsOrigin::SharedRoot {
                "in a settings file under the SHARED temp folder (this machine has no per-user \
                 data directory, so every account can write there)"
            } else {
                "in ./autoshop.local.json — a settings file found in the WORKING DIRECTORY"
            };
            eprintln!(
                "warning: ignoring the API key and base-URL fields {where_}: it is not trusted \
                 to choose where your API key is sent. Save your settings in the app, or set \
                 AUTOSHOP_DATA_DIR to a folder only you can write."
            );
        }
        let local = local.restricted_to(src);
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
        // Bases resolve FIRST: each file-stored key is gated on the endpoint
        // it was saved for (`file_key_for`), so the key pick needs the
        // resolved base in hand.
        let image_base =
            pick(&local.image_base_url, env_val("AUTOSHOP_OPENAI_BASE_URL"), default_base);
        let analysis_base =
            pick(&local.analysis_base_url, env_val("AUTOSHOP_ANALYSIS_BASE_URL"), default_base);
        // Bundled sidecar helpers resolve against the PROGRAM's own tree,
        // never the cwd (see `bundled_helper`): a photo pack carrying
        // `python/denoise.py` used to have that file executed as the user
        // the next time denoise ran from inside it. The weight cache
        // defaults to `weights/` beside whichever script answered, so dev
        // builds keep the repo cache and a packaged install stays beside
        // the exe.
        let denoise_script =
            env_val("AUTOSHOP_DENOISE_SCRIPT").unwrap_or_else(|| bundled_helper("python/denoise.py"));
        let denoise_cache = env_val("AUTOSHOP_DENOISE_CACHE").unwrap_or_else(|| {
            Path::new(&denoise_script)
                .parent()
                .map(|d| d.join("weights").to_string_lossy().into_owned())
                .unwrap_or_else(|| bundled_helper("python/weights"))
        });
        let segment_script =
            env_val("AUTOSHOP_SEGMENT_SCRIPT").unwrap_or_else(|| bundled_helper("python/segment.py"));
        Config {
            openai_api_key: header_safe_key(
                pick_opt(
                    &file_key_for(
                        &local.image_api_key,
                        &local.image_api_key_base,
                        &image_base,
                        "the image API key",
                    ),
                    env_val("OPENAI_API_KEY"),
                ),
                "the image API key",
            ),
            openai_model: pick(&local.image_model, env_val("AUTOSHOP_OPENAI_MODEL"), "gpt-5.5"),
            openai_base_url: image_base,
            openai_image_model: pick(
                &local.image_gen_model,
                env_val("AUTOSHOP_OPENAI_IMAGE_MODEL"),
                "gpt-image-1.5",
            ),
            openai_image_quality: env_val("AUTOSHOP_IMAGE_QUALITY")
                .unwrap_or_else(|| "high".to_string()),
            openai_image_max_px: env_val("AUTOSHOP_IMAGE_MAX_PX")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(8_294_400),
            image_provider: pick(
                &local.image_provider,
                env_val("AUTOSHOP_IMAGE_PROVIDER"),
                "api",
            ),
            // `explicit_or_env`, not `pick_opt`: a saved `""` is the explicit
            // choice "send no effort parameter" and must silence an env tier.
            image_effort: effort(explicit_or_env(
                &local.image_effort,
                env_val("AUTOSHOP_IMAGE_EFFORT"),
            )),

            analysis_provider: pick(
                &local.analysis_provider,
                env_val("AUTOSHOP_ANALYSIS_PROVIDER"),
                "oauth",
            ),
            analysis_model: pick(
                &local.analysis_model,
                env_val("AUTOSHOP_ANALYSIS_MODEL")
                    .or_else(|| env_val("AUTOSHOP_CLAUDE_MODEL")),
                "opus",
            ),
            analysis_effort: effort(explicit_or_env(
                &local.analysis_effort,
                env_val("AUTOSHOP_ANALYSIS_EFFORT"),
            )),
            claude_bin: env_val("AUTOSHOP_CLAUDE_BIN").unwrap_or_else(|| "claude".to_string()),
            analysis_api_key: header_safe_key(
                pick_opt(
                    &file_key_for(
                        &local.analysis_api_key,
                        &local.analysis_api_key_base,
                        &analysis_base,
                        "the analysis API key",
                    ),
                    env_val("AUTOSHOP_ANALYSIS_API_KEY"),
                ),
                "the analysis API key",
            ),
            analysis_base_url: analysis_base,

            python_bin: env_val("AUTOSHOP_PYTHON").unwrap_or_else(|| "python".to_string()),
            denoise_model: env_val("AUTOSHOP_DENOISE_MODEL")
                .unwrap_or_else(|| "color_real_psnr".to_string()),
            denoise_script,
            denoise_cache,
            segment_script,
            style_strength: env_val("AUTOSHOP_STYLE_STRENGTH")
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
            // Key homes are POWERLESS alone (they can only WITHHOLD a
            // file-stored key, and the keys above are stripped), so
            // `restricted_to` deliberately leaves them; planted here to keep
            // that reasoning exercised.
            image_api_key_base: Some("https://attacker.example/v1".into()),
            analysis_api_key_base: Some("https://attacker.example/v1".into()),
            // Harmless selectors an ambient file is still allowed to set.
            image_model: Some("gpt-5.6-sol".into()),
            analysis_model: Some("opus".into()),
            image_provider: Some("api".into()),
            analysis_provider: Some("oauth".into()),
            image_gen_model: Some("gpt-image-2".into()),
            image_effort: Some("high".into()),
            analysis_effort: Some("high".into()),
        };
        assert!(
            planted.names_beyond(Source::WorkingDirFile),
            "the warning must fire for this file"
        );

        let safe = planted.clone().restricted_to(Source::WorkingDirFile);
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
        assert_eq!(safe.image_effort.as_deref(), Some("high"), "effort is a preference");

        // A file with only harmless fields must NOT trigger the warning.
        let benign = LocalSettings { image_model: Some("m".into()), ..Default::default() };
        assert!(!benign.names_beyond(Source::WorkingDirFile));

        // The SHARED-root origin is the same capability as the cwd file — the
        // whole point of labelling `<temp>/autoshop` rather than trusting it.
        assert_eq!(SettingsOrigin::SharedRoot.source(), Source::WorkingDirFile);
        assert_eq!(SettingsOrigin::Central.source(), Source::Trusted);
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
    /// Now stated as POLICY against the table rather than as a copy of it:
    /// the old version re-derived its list from the constant it was guarding,
    /// which is why it could not notice the constant being wrong. This list is
    /// the specification — every name on it reaches `Command::new`, becomes
    /// that command's argv, chooses an endpoint, decides which account pays,
    /// or sites the trusted settings file itself.
    #[test]
    fn every_variable_that_names_a_program_or_an_endpoint_is_ambient_unsafe() {
        for name in [
            "AUTOSHOP_OPENAI_BASE_URL",
            "AUTOSHOP_ANALYSIS_BASE_URL",
            "AUTOSHOP_CLAUDE_BIN",
            "AUTOSHOP_PYTHON",
            "AUTOSHOP_DENOISE_SCRIPT",
            "AUTOSHOP_SEGMENT_SCRIPT",
            "AUTOSHOP_DENOISE_CACHE",
            "AUTOSHOP_DATA_DIR",
            "PATH",
            "PYTHONPATH",
            "PYTHONHOME",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
        ] {
            let t = trust_of(name);
            assert_eq!(t, Trust::Destination, "{name} decides a destination or a program");
            assert!(!Source::DotEnv.may_supply(t), "{name} resolved from a .env");
            assert!(!Source::WorkingDirFile.may_supply(t), "{name} resolved from an ambient file");
        }
        // …and the six unblocked on 2026-08-11 (user decision): a `.env` picks
        // models and providers again, exactly as README and ARCHITECTURE §3
        // have always documented. The endpoint and the key stay above.
        for name in [
            "AUTOSHOP_ANALYSIS_PROVIDER",
            "AUTOSHOP_ANALYSIS_MODEL",
            "AUTOSHOP_CLAUDE_MODEL",
            "AUTOSHOP_OPENAI_MODEL",
            "AUTOSHOP_OPENAI_IMAGE_MODEL",
            "AUTOSHOP_IMAGE_PROVIDER",
        ] {
            let t = trust_of(name);
            assert_eq!(t, Trust::Preference, "{name} is a model/provider choice");
            assert!(Source::DotEnv.may_supply(t), "{name} is documented as .env-settable");
        }
        // The asymmetry between the two ambient sources, stated once: a `.env`
        // keeps secrets on purpose (it is where this project's key lives); a
        // file dropped in the working directory never does.
        assert!(Source::DotEnv.may_supply(Trust::Secret));
        assert!(!Source::WorkingDirFile.may_supply(Trust::Secret));

        // And the positional read is gone. `pre(i)` indexed the protected
        // array, so removing one name shifted every later index onto the wrong
        // variable — the exact edit this round had to make.
        let src = include_str!("config.rs");
        let non_test = src.split("#[cfg(test)]").next().unwrap();
        assert!(
            !non_test.contains("pre(0)") && !non_test.contains("AMBIENT_UNSAFE_VARS["),
            "config resolution regained a positional read of the protected list"
        );
    }

    /// The table is only a single source of truth if nothing resolves around
    /// it. Every `AUTOSHOP_*` variable named in this file's non-test code must
    /// be classified — otherwise a new setting silently defaults to
    /// `Preference` and a `.env` can set it.
    #[test]
    fn the_capability_table_classifies_every_variable_this_file_resolves() {
        let src = include_str!("config.rs");
        let non_test = src.split("#[cfg(test)]").next().unwrap();
        let mut unclassified: Vec<&str> = Vec::new();
        let mut rest = non_test;
        while let Some(i) = rest.find("\"AUTOSHOP_") {
            rest = &rest[i + 1..];
            let Some(end) = rest.find('"') else { break };
            let name = &rest[..end];
            if !SETTINGS.iter().any(|s| s.env == name) && !unclassified.contains(&name) {
                unclassified.push(name);
            }
        }
        assert!(
            unclassified.is_empty(),
            "not in SETTINGS, so a .env may set them by default: {unclassified:?}"
        );
        // The out-of-crate consumers resolve through `env_or_dotenv`, which
        // also consults the table — so their knobs must be in it too.
        for knob in
            ["AUTOSHOP_HTTP_TIMEOUT_SECS", "AUTOSHOP_SIDECAR_TIMEOUT_SECS", "AUTOSHOP_LEGACY_OUT"]
        {
            assert!(SETTINGS.iter().any(|s| s.env == knob), "{knob} is unclassified");
        }
        // No duplicate rows: `trust_of` takes the first match, so a second row
        // for the same name would be dead policy that reads as live.
        for (i, s) in SETTINGS.iter().enumerate() {
            assert!(
                !SETTINGS[..i].iter().any(|e| e.env == s.env),
                "{} appears twice in SETTINGS",
                s.env
            );
        }
    }

    /// A key that cannot go in an HTTP header is refused AT THE BOUNDARY.
    ///
    /// ureq builds the header eagerly and quotes the whole line back on
    /// failure — `invalid header 'Authorization: Bearer <the real key>'`
    /// (2.12.1 `header.rs:147`) — and that string then travels as a transport
    /// error into rationale text and the Settings status line. A trailing
    /// newline from a copy/paste was enough. The trim handles the common case;
    /// the refusal handles the rest, and neither ever prints the key.
    #[test]
    fn a_key_that_cannot_ride_a_header_never_reaches_one() {
        assert_eq!(
            header_safe_key(Some("  sk-good-key\n".into()), "k").as_deref(),
            Some("sk-good-key"),
            "surrounding whitespace is the common paste artefact — trim it, don't refuse it"
        );
        for bad in ["sk-with space", "sk-with\nnewline", "sk-with\ttab", "sk-caf\u{e9}"] {
            assert_eq!(
                header_safe_key(Some(bad.into()), "k"),
                None,
                "{bad:?} would have been quoted back inside a transport error"
            );
        }
        assert_eq!(header_safe_key(Some("   ".into()), "k"), None, "blank is unset");
        assert_eq!(header_safe_key(None, "k"), None);
    }

    /// Reasoning effort is bounded, not enumerated: it reaches both a JSON
    /// body and `Command::args`, and the accepted tiers differ per provider
    /// (`claude --help`: low|medium|high|xhigh|max) and change over time.
    #[test]
    fn reasoning_effort_is_bounded_before_it_reaches_argv() {
        assert_eq!(effort(Some(" High ".into())).as_deref(), Some("high"));
        assert_eq!(effort(Some("xhigh".into())).as_deref(), Some("xhigh"));
        assert_eq!(effort(Some("".into())), None, "empty means 'let the provider decide'");
        assert_eq!(effort(None), None);
        // A leading dash would be parsed as another CLI flag; the rest is
        // ordinary argv/JSON hygiene on a value a .env can supply.
        for bad in ["--dangerously-skip-permissions", "high high", "high\nmax", "x".repeat(33).as_str()] {
            assert_eq!(effort(Some(bad.into())), None, "{bad:?} must not reach argv");
        }
    }

    /// GUI review 2026-08-12 F2: both Settings surfaces persist a cleared
    /// effort field as `""` — the explicit choice "send no effort parameter".
    /// The generic `pick_opt` filtered that empty out and fell through to the
    /// environment, so with `AUTOSHOP_IMAGE_EFFORT=high` set, selecting
    /// "provider default" saved, reopened as `high`, and kept sending `high`:
    /// the one choice the field exists to express was the one it could not
    /// make.
    #[test]
    fn an_explicitly_blank_effort_silences_the_environment_tier() {
        let env = || Some("high".to_string());
        // Never saved → the environment may still supply the tier.
        assert_eq!(explicit_or_env(&None, env()).as_deref(), Some("high"));
        // An explicit tier wins outright (file over env, as everywhere).
        assert_eq!(explicit_or_env(&Some("xhigh".into()), env()).as_deref(), Some("xhigh"));
        // An explicit blank is a VALUE, not a hole for the env to fill.
        assert_eq!(explicit_or_env(&Some(String::new()), env()), None);
        assert_eq!(explicit_or_env(&Some("  ".into()), env()), None);
    }

    /// GUI review 2026-08-12 F1 (HIGH): the providers are mutually exclusive
    /// but share one key slot, and flipping the image role to OAuth swaps the
    /// base URL to the local bridge while the CLOUD key stays armed — the
    /// next call sends `Authorization: Bearer <cloud key>` to whatever
    /// listens on the bridge port (and a saved bridge token rides to the
    /// cloud on the way back). The stored credential is now bound to the
    /// endpoint it was saved for.
    #[test]
    fn a_saved_key_is_only_sent_to_the_endpoint_it_was_saved_for() {
        let key = Some("sk-relativized0test".to_string());
        let cloud = "https://api.openai.com/v1";
        let bridge = "http://127.0.0.1:8317/v1";
        // At home (modulo a trailing slash) → sent.
        let home = Some(format!("{cloud}/"));
        assert!(file_key_for(&key, &home, cloud, "k").is_some());
        // Saved for the cloud, endpoint now the bridge → withheld; and the
        // reverse flip withholds a bridge token from the cloud.
        assert_eq!(file_key_for(&key, &Some(cloud.into()), bridge, "k"), None);
        assert_eq!(file_key_for(&key, &Some(bridge.into()), cloud, "k"), None);
        // A pre-v0.23.2 save recorded no home → allowed until one is saved.
        assert!(file_key_for(&key, &None, bridge, "k").is_some());
        // No key → nothing to gate.
        assert_eq!(file_key_for(&None, &Some(cloud.into()), cloud, "k"), None);
    }

    /// The home stamp's lifecycle (`reconcile_key_homes`, shared by both
    /// writers through `update_local_settings`): a writer's own stamp is
    /// never second-guessed; a legacy key inherits a home only from a
    /// base-STABLE save; a base-changing save must not bless the new pairing.
    #[test]
    fn a_key_home_is_grandfathered_only_by_a_base_stable_save() {
        let legacy = LocalSettings {
            image_api_key: Some("sk-relativized0test".into()),
            image_base_url: Some("https://a.example/v1".into()),
            ..Default::default()
        };
        // Base unchanged → the pairing the user had on screen is recorded.
        let mut cur = legacy.clone();
        cur.reconcile_key_homes(&legacy);
        assert_eq!(cur.image_api_key_base.as_deref(), Some("https://a.example/v1"));
        // Base changed in the same save → stays home-less, not re-homed.
        let mut cur = legacy.clone();
        cur.image_base_url = Some("http://127.0.0.1:8317/v1".into());
        cur.reconcile_key_homes(&legacy);
        assert_eq!(cur.image_api_key_base, None);
        // The writer stamped this save's typed key → kept verbatim.
        let mut cur = legacy.clone();
        cur.image_base_url = Some("http://127.0.0.1:8317/v1".into());
        cur.image_api_key_base = Some("http://127.0.0.1:8317/v1".into());
        cur.reconcile_key_homes(&legacy);
        assert_eq!(cur.image_api_key_base.as_deref(), Some("http://127.0.0.1:8317/v1"));
        // No key → no home; and the analysis role rides the same rule.
        let mut cur = LocalSettings {
            image_api_key_base: Some("https://a.example/v1".into()),
            analysis_api_key: Some("sk-relativized0test".into()),
            analysis_base_url: Some("https://b.example/v1".into()),
            ..Default::default()
        };
        cur.reconcile_key_homes(&LocalSettings {
            analysis_base_url: Some("https://b.example/v1".into()),
            ..Default::default()
        });
        assert_eq!(cur.image_api_key_base, None);
        assert_eq!(cur.analysis_api_key_base.as_deref(), Some("https://b.example/v1"));
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
        let base = ambient.clone().restricted_to(SettingsOrigin::WorkingDir.source());
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
                // The provider IS settable from a .env again (user decision,
                // 2026-08-11; README and ARCHITECTURE §3 always said so). It
                // is not an escalation: a `.env` may supply keys by explicit
                // project contract, and a planted OPENAI_API_KEY already
                // routes the image proposer — which uploads the PHOTO — through
                // the planter's account. Choosing which of the user's own
                // endpoints answers adds no capability on top of that, while
                // the endpoint itself stays protected below.
                assert_eq!(
                    cfg.analysis_provider, "api",
                    "a .env may pick the provider — the documented contract"
                );
                assert_eq!(
                    cfg.analysis_base_url, "https://api.openai.com/v1",
                    "…but never the endpoint that provider talks to"
                );
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
                // (d) The child block is an ALLOWLIST, not the complement of
                // the table's denylist. `Trust` classifies OUR settings — a
                // closed set where "unlisted" means "not ours", so defaulting
                // to Preference is safe; a child's environment is an OPEN set
                // where "unlisted" includes every loader hook the platform
                // defines. Reusing one predicate for both let a photo pack's
                // `.env` hand `LD_PRELOAD` to both Python sidecars, where
                // ld.so acts before `-E` (which only filters PYTHON*) can.
                let child_env = super::dotenv_child_env();
                let has = |n: &str| child_env.iter().any(|(k, _)| k == n);
                assert!(
                    child_env.iter().any(|(k, v)| k == "CUDA_VISIBLE_DEVICES" && v == "1"),
                    "an allowlisted compute knob lost its way to the sidecars"
                );
                assert!(!has("LD_PRELOAD"), "a .env loaded code into a child process");
                assert!(!has("HTTP_PROXY"), "a .env chose where a child sends bytes");
                assert!(!has("HF_HOME"), "a .env chose which weights a child loads");
                assert!(
                    !has("PATH") && !has("PYTHONPATH"),
                    "a protected name leaked into the sidecar child block"
                );
                // Not an allowlist of everything harmless-looking: a knob the
                // PARENT reads has no business in the child's block either.
                assert!(!has("AUTOSHOP_SIDECAR_TIMEOUT_SECS"), "parent-side knob pushed at a child");
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
                 PATH=.\n\
                 CUDA_VISIBLE_DEVICES=1\n\
                 LD_PRELOAD=./evil.so\n\
                 HTTP_PROXY=http://attacker.example:8080\n\
                 HF_HOME=./poisoned-weights\n",
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
