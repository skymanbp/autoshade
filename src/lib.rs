//! Autoshop engine library — the shared core behind both front-ends.
//!
//! The AI advisor looks at a RAW preview + metadata and emits a
//! [`recipe::EditRecipe`]; the deterministic [`render`] engine applies it.
//! Both the CLI (`bin/autoshop`, i.e. `main.rs`) and the native GUI
//! (`bin/gui.rs`) link this library and call the engine directly — the GUI has
//! NO HTTP server; it invokes `render`/`pipeline`/`decode` in-process.
//!
//! See `docs/ARCHITECTURE.md` for the full design.

pub mod advisor;
pub mod config;
pub mod decode;
pub mod denoise;
pub mod eval;
pub mod fit;
pub mod fit_zoned;
pub mod generative;
pub mod lensmeta;
pub mod openai_models;
pub mod pipeline;
pub mod recipe;
pub mod render;
pub mod retouch;
pub mod segment;
pub mod serve;
pub mod store;
pub mod style;
pub mod xmp;

/// Stop a spawned **console** child (the `claude` CLI, the python denoise sidecar)
/// from popping its own console window when the parent is the windowed desktop GUI
/// (built with `windows_subsystem = "windows"`, so it has no console of its own).
///
/// Sets Windows' `CREATE_NO_WINDOW` flag; it only suppresses a *new* window and
/// does NOT touch stdio — each caller decides that. The python sidecars pipe
/// theirs (see [`sidecar_tail`]) precisely because the windowed GUI has no
/// console for an inherited handle to reach. A no-op on non-Windows targets.
pub fn hide_child_console(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// A kill-group over a sidecar child AND every descendant it spawns (L11#7).
///
/// The three sidecar kill sites used `Child::kill` — TerminateProcess /
/// SIGKILL on the DIRECT child only — so a launcher's real python or a
/// GPU-holding torch worker survived the timeout, kept the VRAM and its
/// half-written `.part`, and held the inherited pipes open (the very case
/// `denoise::bounded_child_output`'s bounded-join detach discloses; the
/// group makes that belt mostly unreachable, and the belt stays).
///
/// Windows: a Job Object with `KILL_ON_JOB_CLOSE` — the whole tree dies
/// when this handle closes, even if THIS process crashes first. Unix: the
/// child leads its own process group (armed pre-spawn) and [`Self::kill_tree`]
/// signals the group; there is no close-kills equivalent, so descendants
/// surviving a NORMAL exit are not reaped there (registered — secondary
/// platform, and the pipes still bound the drain).
pub struct KillGroup {
    #[cfg(windows)]
    job: std::os::windows::io::OwnedHandle,
    #[cfg(unix)]
    pgid: i32,
}

impl KillGroup {
    /// Kill every process in the group NOW — best-effort, idempotent. Call
    /// BEFORE reaping the direct child (`Child::wait`): on unix the group id
    /// is only guaranteed unrecycled while its leader is un-reaped.
    pub fn kill_tree(&self) {
        #[cfg(windows)]
        // SAFETY: the handle is a live Job Object owned by `self`; terminating
        // a job this process created is exactly its documented use.
        unsafe {
            use std::os::windows::io::AsRawHandle;
            windows_sys::Win32::System::JobObjects::TerminateJobObject(
                self.job.as_raw_handle(),
                1,
            );
        }
        #[cfg(unix)]
        // SAFETY: plain syscall; a stale/invalid pgid returns ESRCH harmlessly.
        unsafe {
            libc::killpg(self.pgid, libc::SIGKILL);
        }
    }
}

/// Pre-spawn half of the kill-group: on unix the group must be created at
/// `fork` time (`process_group(0)` makes the child its own leader). A no-op
/// on Windows, where the group is a Job assigned after the spawn.
pub fn arm_kill_group(cmd: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = cmd;
    }
}

/// Post-spawn half: wrap `child` (and every process it will spawn) in a
/// [`KillGroup`]. `None` = the platform refused (e.g. an outer Job that
/// forbids assignment) — DISCLOSED here once, and the sidecar keeps running
/// unwrapped: the group is containment telemetry, not a deliverable, so a
/// refusal must not take AI denoise down with it.
pub fn assign_kill_group(child: &std::process::Child) -> Option<KillGroup> {
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        // SAFETY: straight Win32 sequence — create an anonymous job, set the
        // close-kills limit, assign the freshly spawned (not yet reaped)
        // child. Every failure path closes what was opened via OwnedHandle.
        unsafe {
            let raw = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if raw.is_null() {
                eprintln!("⚠ sidecar kill-group unavailable (CreateJobObjectW failed) — descendants of a timed-out sidecar are not reaped");
                return None;
            }
            let job = OwnedHandle::from_raw_handle(raw);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
                || AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) == 0
            {
                eprintln!("⚠ sidecar kill-group unavailable (job assignment failed) — descendants of a timed-out sidecar are not reaped");
                return None;
            }
            Some(KillGroup { job })
        }
    }
    #[cfg(unix)]
    {
        // `arm_kill_group` made the child its own group leader, so the group
        // id IS the child's pid.
        Some(KillGroup { pgid: child.id() as i32 })
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = child;
        None
    }
}

/// Snapshot of a sidecar's promised artifact path BEFORE the sidecar runs:
/// `None` = nothing there, `Some((len, mtime))` = what already exists — a
/// stale deliverable from an earlier export (the CLI's `default_out` names
/// are deterministic and re-export over themselves by design), or the 0-byte
/// claim file `pipeline::unique_out` / `store::claim_raster` create to
/// reserve the name. [`sidecar_wrote`] compares against this so a pre-existing
/// file cannot masquerade as this run's result.
pub fn artifact_state(p: &std::path::Path) -> Option<(u64, Option<std::time::SystemTime>)> {
    std::fs::metadata(p).ok().map(|m| (m.len(), m.modified().ok()))
}

/// The sidecar success contract, shared by the denoise and segmentation
/// bridges: **exit 0 alone is not success** — THIS run must have produced the
/// artifact. Three refusals, each a real failure mode found live:
/// * missing — a sidecar exited 0 without writing; the CLI then printed
///   `denoised -> path` for a file that did not exist;
/// * empty — the callers that pre-claim the name (`unique_out`,
///   `claim_raster`) hand the sidecar an existing 0-byte file, so a bare
///   `exists()` check (segment.rs's original guard) passed without a write;
/// * untouched — the CLI's deterministic output names mean an EARLIER
///   deliverable can already sit at the path; same length + same mtime as
///   before the spawn is that stale file, not a result.
///
/// `who` names the bridge for the message ("denoise sidecar", …).
pub fn sidecar_wrote(
    who: &str,
    output: &std::path::Path,
    before: Option<(u64, Option<std::time::SystemTime>)>,
) -> anyhow::Result<()> {
    use anyhow::bail;
    let Some((len, mtime)) = artifact_state(output) else {
        bail!("{who} exited 0 but wrote no output at {}", output.display());
    };
    if len == 0 {
        bail!("{who} exited 0 but the output at {} is empty", output.display());
    }
    if let Some((pre_len, pre_mtime)) = before {
        // mtime unavailable (exotic filesystem) ⇒ compare length only — a
        // disclosed weakening, never a false refusal.
        if pre_len == len && pre_mtime.is_some() && pre_mtime == mtime {
            bail!(
                "{who} exited 0 but did not write the output at {} — the file there \
                 predates this run, and presenting it as the result would hide the failure",
                output.display()
            );
        }
    }
    Ok(())
}

/// Last ~400 chars of a failed sidecar's output, for the `bail!` message.
///
/// The python sidecars report their real cause (a missing dependency prints the
/// exact `pip install` line) on stderr, falling back to stdout. Both streams are
/// CAPTURED, not inherited, because the windowed GUI has no console to inherit —
/// so this string is the only way that text reaches the user. Lossy UTF-8: the
/// child may emit anything, and a decode error must never hide the error.
pub fn sidecar_tail(stderr: &[u8], stdout: &[u8]) -> String {
    const MAX: usize = 400;
    let pick = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
    let mut text = pick(stderr);
    if text.is_empty() {
        text = pick(stdout);
    }
    if text.is_empty() {
        return "(the sidecar printed nothing)".to_string();
    }
    // Char-boundary safe: count CHARS, never slice bytes mid-codepoint.
    let n = text.chars().count();
    if n > MAX {
        let cut: String = text.chars().skip(n - MAX).collect();
        format!("...{cut}")
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::{artifact_state, sidecar_tail, sidecar_wrote};

    /// M-D2: the `len == 0` refusal weakened; M-D3: the `before` comparison
    /// dropped. All four arms of the contract, each on its own unique dir so
    /// concurrent test processes cannot clobber each other's fixtures.
    #[test]
    fn a_sidecar_result_must_be_this_runs_own_nonempty_write() {
        let dir = std::env::temp_dir()
            .join(format!("autoshop-lib-test-wrote-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // missing → refused
        let missing = dir.join("never_written.png");
        let err = sidecar_wrote("x sidecar", &missing, None).unwrap_err().to_string();
        assert!(err.contains("wrote no output"), "{err}");

        // empty → refused (the GUI's unique_out claim file is exactly this)
        let empty = dir.join("claimed.png");
        std::fs::write(&empty, b"").unwrap();
        let before = artifact_state(&empty);
        let err = sidecar_wrote("x sidecar", &empty, before).unwrap_err().to_string();
        assert!(err.contains("is empty"), "{err}");

        // pre-existing NON-EMPTY file, untouched by the run → refused: this is
        // the stale-deliverable hole (deterministic out/<stem>.denoised.tif)
        let stale = dir.join("stale.tif");
        std::fs::write(&stale, b"an earlier export").unwrap();
        let before = artifact_state(&stale);
        let err = sidecar_wrote("x sidecar", &stale, before).unwrap_err().to_string();
        assert!(err.contains("predates this run"), "{err}");

        // a real write over a stale file → accepted (mtime and/or len moved)
        std::fs::write(&stale, b"fresh pixels, longer than before").unwrap();
        assert!(sidecar_wrote("x sidecar", &stale, before).is_ok());

        // a fresh write with no predecessor → accepted
        let real = dir.join("real.png");
        std::fs::write(&real, b"png-bytes").unwrap();
        assert!(sidecar_wrote("x sidecar", &real, None).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sidecar_tail_surfaces_the_real_cause() {
        // stderr wins (where both sidecars report failures)...
        assert_eq!(sidecar_tail(b"segment.py: needs rembg -> pip install rembg\n", b"progress"),
                   "segment.py: needs rembg -> pip install rembg");
        // ...stdout is the fallback when stderr is empty...
        assert_eq!(sidecar_tail(b"  \n", b"boom"), "boom");
        // ...and a silent child still yields a usable message.
        assert_eq!(sidecar_tail(b"", b""), "(the sidecar printed nothing)");
        // Long output keeps the TAIL (the traceback's last line) and never
        // splits a multi-byte char (a byte slice here would panic).
        let long = "é".repeat(500) + "END";
        let tail = sidecar_tail(long.as_bytes(), b"");
        assert!(tail.ends_with("END") && tail.starts_with("..."));
        assert_eq!(tail.chars().count(), 403); // "..." + 400 chars
    }
}
