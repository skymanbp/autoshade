//! The update check (v1.18.5): when the desktop app asks whether a newer
//! AutoShade exists, and what it does with the answer. The asking and reading
//! is the library's (`autoshade::update`); this file holds the state, the
//! background thread, and the rules:
//!
//! * The AUTOMATIC check runs at most once every [`AUTO_SPACING_SECS`],
//!   started on the first frame (the window is up) on its own thread, so it
//!   never delays startup or blocks the UI thread — a name lookup on a slow
//!   network has taken 11 s. It is off when the user unticks it, or when
//!   [`ENV_OFF`] is set to anything non-empty, and it never runs under
//!   `cfg(test)`. It is silent on every failure. A newer version raises ONE
//!   notice per check, unless the user chose to skip that version.
//! * 「Check now」 always asks and says the outcome in Settings → About.
//! * Nothing is downloaded or installed; 「Open the download page」 hands the
//!   page to the system browser.
//!
//! The fetch and the clock are injected (`UpdateState::fetch`, the `now`
//! arguments), so the tests drive every rule with no network and no window.

use super::*;
use autoshade::update::{CheckError, Release, Version};

/// The automatic check's spacing: once a day at most.
pub(crate) const AUTO_SPACING_SECS: u64 = 24 * 60 * 60;

/// Set to anything non-empty, this turns the AUTOMATIC check off (「Check
/// now」 still works).
pub(crate) const ENV_OFF: &str = "AUTOSHADE_NO_UPDATE_CHECK";

/// What persists (inside `Prefs`, key `update_check`). `#[serde(default)]`
/// both here and on `Prefs`, so a prefs file from before v1.18.5 — or one
/// missing any single key — loads with these defaults.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct UpdatePrefs {
    /// 「Check for updates automatically」. On by default.
    pub(crate) auto: bool,
    /// When the last automatic check started, in seconds since 1970 (0 =
    /// never). Stamped at the START, so a failing network is not asked again
    /// on every launch.
    pub(crate) last_auto_check: u64,
    /// The version the user chose to skip: the automatic check does not
    /// announce it again (About still shows it).
    pub(crate) skipped: Option<String>,
    /// The newest version a check found, so About can still name it after a
    /// restart inside the 24 hours — without asking again. Dropped on load
    /// once this build is that version or newer.
    pub(crate) known_newer: Option<String>,
}

impl Default for UpdatePrefs {
    fn default() -> Self {
        UpdatePrefs { auto: true, last_auto_check: 0, skipped: None, known_newer: None }
    }
}

/// What Settings → About says about updates.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UpdateRow {
    NotChecked,
    Checking,
    /// This build is the latest (it names this build's version).
    UpToDate(Version),
    Newer(Release),
    Failed(CheckError),
}

/// Who started a check: the automatic one is silent on failure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CheckKind {
    Auto,
    Manual,
}

/// The fetch a check runs on its thread — the real request, or a test's.
pub(crate) type Fetch = Arc<dyn Fn() -> Result<Release, CheckError> + Send + Sync>;

pub(crate) struct UpdateState {
    pub(crate) prefs: UpdatePrefs,
    pub(crate) row: UpdateRow,
    pub(crate) fetch: Fetch,
    /// This build's version (a test sets its own).
    pub(crate) current: Version,
    /// The check in flight. A new check replaces it, so an older answer lands
    /// in a dropped receiver and is never read.
    inflight: Option<(CheckKind, Receiver<Result<Release, CheckError>>)>,
    /// The first frame has decided about the automatic check.
    started: bool,
    /// A newer version the automatic check found, waiting to be announced:
    /// the mode page opens first on every launch and draws no toasts, so the
    /// notice waits for the workspace instead of fading out unseen.
    pub(crate) notice: Option<Version>,
    /// [`ENV_OFF`] is set: the automatic check is off whatever the stored
    /// switch says, and About draws the switch as off. Read once at startup
    /// (a test sets it).
    pub(crate) env_off: bool,
}

impl Default for UpdateState {
    fn default() -> Self {
        UpdateState {
            prefs: UpdatePrefs::default(),
            row: UpdateRow::NotChecked,
            // Under `cfg(test)` a check nobody injected a fetch for answers
            // "no connection" instead of reaching the network.
            fetch: if cfg!(test) {
                Arc::new(|| Err(CheckError::Offline))
            } else {
                Arc::new(|| autoshade::update::fetch_latest(&autoshade::update::Endpoint::default()))
            },
            current: Version::current(),
            inflight: None,
            started: false,
            notice: None,
            env_off: env_switched_off(),
        }
    }
}

impl UpdateState {
    /// Take the persisted half; a remembered newer version shows in About
    /// only while it is still newer than this build.
    pub(crate) fn restore(&mut self, prefs: UpdatePrefs) {
        self.prefs = prefs;
        let known = self.prefs.known_newer.as_deref().and_then(Version::parse).filter(|v| *v > self.current);
        match known {
            Some(version) => {
                let page = autoshade::update::RELEASES_PAGE.to_owned();
                self.row = UpdateRow::Newer(Release { version, page });
            }
            None => self.prefs.known_newer = None,
        }
    }

    /// Whether the automatic check is due at `now` (seconds since 1970). A
    /// clock set back before the last check counts as due, once — the check
    /// re-stamps it.
    pub(crate) fn auto_due(&self, now: u64) -> bool {
        let last = self.prefs.last_auto_check;
        last == 0 || now < last || now - last >= AUTO_SPACING_SECS
    }

    pub(crate) fn checking(&self) -> bool {
        self.inflight.is_some()
    }
}

/// Seconds since 1970 on this computer's clock.
pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Whether [`ENV_OFF`] is set to something non-empty.
pub(crate) fn env_switched_off() -> bool {
    switched_off(std::env::var_os(ENV_OFF).as_deref())
}

/// The rule [`env_switched_off`] applies to the variable's value: set and
/// non-empty means off.
pub(crate) fn switched_off(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|v| !v.is_empty())
}

impl AutoShadeApp {
    /// Every frame: on the first, decide whether the automatic check runs —
    /// never under `cfg(test)`; on all, land an answer that has arrived, and
    /// announce a found version once the workspace is showing.
    pub(crate) fn update_check_frame(&mut self) {
        if !std::mem::replace(&mut self.update.started, true) && !cfg!(test) {
            self.maybe_start_auto_check(unix_now(), self.update.env_off);
        }
        self.poll_update_check();
        if self.mode_chosen_this_launch
            && let Some(v) = self.update.notice.take()
        {
            let name = v.to_string();
            let t = trf(self.lang, "AutoShade {version} is available — see Settings → About.", &[("version", &name)]);
            self.toast(ToastKind::Success, t);
        }
    }

    /// Start the automatic check if the switches allow it and it is due at
    /// `now`. True when it started.
    pub(crate) fn maybe_start_auto_check(&mut self, now: u64, env_off: bool) -> bool {
        let u = &self.update;
        if env_off || !u.prefs.auto || !u.auto_due(now) || u.checking() {
            return false;
        }
        self.update.prefs.last_auto_check = now;
        self.prefs_save_due = true;
        self.start_update_check(CheckKind::Auto);
        true
    }

    /// 「Check now」: always asks; the row says 「Checking…」 until it lands.
    pub(crate) fn start_manual_update_check(&mut self) {
        self.update.row = UpdateRow::Checking;
        self.start_update_check(CheckKind::Manual);
    }

    fn start_update_check(&mut self, kind: CheckKind) {
        let (tx, rx) = std::sync::mpsc::channel();
        let fetch = Arc::clone(&self.update.fetch);
        let ctx = self.egui_ctx.clone();
        std::thread::spawn(move || {
            // A panic inside the fetch is contained and read as an
            // unreadable answer, not announced as a crash.
            let _contained = autoshade::panic_guard::Recoverable::enter();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fetch()))
                .unwrap_or(Err(CheckError::Unreadable));
            // The window may have closed, or a newer check replaced this
            // one: then nobody is listening, and that is fine.
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.update.inflight = Some((kind, rx));
    }

    /// Land the answer of the check in flight, if it has arrived.
    pub(crate) fn poll_update_check(&mut self) {
        let Some((kind, rx)) = &self.update.inflight else { return };
        let (kind, r) = match rx.try_recv() {
            Ok(r) => (*kind, r),
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => (*kind, Err(CheckError::Unreadable)),
        };
        self.update.inflight = None;
        self.land_update_check(kind, r);
    }

    fn land_update_check(&mut self, kind: CheckKind, r: Result<Release, CheckError>) {
        let current = self.update.current;
        match r {
            Ok(rel) if rel.version > current => {
                let version = rel.version;
                let name = version.to_string();
                if self.update.prefs.known_newer.as_deref() != Some(name.as_str()) {
                    self.update.prefs.known_newer = Some(name.clone());
                    self.prefs_save_due = true;
                }
                self.update.row = UpdateRow::Newer(rel);
                if kind == CheckKind::Auto && self.update.prefs.skipped.as_deref() != Some(name.as_str()) {
                    self.update.notice = Some(version);
                }
            }
            Ok(_) => {
                self.update.row = UpdateRow::UpToDate(current);
                if self.update.prefs.known_newer.take().is_some() {
                    self.prefs_save_due = true;
                }
            }
            // The automatic check is silent and leaves the row as it was.
            Err(e) => {
                if kind == CheckKind::Manual {
                    self.update.row = UpdateRow::Failed(e);
                }
            }
        }
    }

    /// 「Skip this version」: the automatic check stops announcing it.
    pub(crate) fn skip_update_version(&mut self, version: Version) {
        self.update.prefs.skipped = Some(version.to_string());
        self.prefs_save_due = true;
    }
}
