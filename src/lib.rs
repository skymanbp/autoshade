//! AutoShade engine library — the shared core behind both front-ends.
//!
//! The AI advisor looks at a RAW preview + metadata and emits a
//! [`recipe::EditRecipe`]; the deterministic [`render`] engine applies it.
//! Both the CLI (`bin/autoshade`, i.e. `main.rs`) and the native GUI
//! (`bin/gui.rs`) link this library and call the engine directly — the GUI has
//! NO HTTP server; it invokes `render`/`pipeline`/`decode` in-process.
//!
//! See `docs/ARCHITECTURE.md` for the full design.

pub mod advisor;
pub mod config;
pub mod correspond;
pub mod decode;
pub mod denoise;
pub mod describe;
pub mod diag;
pub mod embed;
pub mod eval;
pub mod fit;
pub(crate) mod fit_field;
pub mod fit_zoned;
pub mod generative;
pub mod jobs;
pub mod lcp;
pub mod lensmeta;
pub mod mask_habit;
pub(crate) mod mask_refine;
pub mod openai_models;
pub mod pipeline;
pub mod rationale;
pub mod recipe;
pub mod render;
pub mod retouch;
pub mod segment;
pub mod sha256;
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

/// SINGLE-FLIGHT over the MODEL sidecars, process-wide: at most one AI model
/// is resident at a time, whatever the caller's concurrency.
///
/// Moved up from `embed.rs` when the correspondence bridge became its second
/// user (step 7a): the budget it enforces was never about SigLIP specifically
/// — the models live in VRAM, which none of the host-RAM budgets
/// (`decode::MAX_CONCURRENT_DECODES`, `jobs`' free-memory division) can see,
/// and a SigLIP (0.75 GB fp16) resident BESIDE an SD 2.1 UNet (~2.4 GB fp16)
/// is exactly the co-residency a per-module slot would have permitted. One
/// process-wide gate makes the budget a single sentence: one model at a time.
/// (The full gate-not-batcher rationale is on `embed::embed_file`.)
///
/// Poison is recovered rather than re-panicked, like every other lock in this
/// tree: one caller panicking inside a sidecar must not turn every other
/// caller's run into a second panic.
pub fn with_model_slot<T>(body: impl FnOnce() -> T) -> T {
    static SLOT: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = SLOT.lock().unwrap_or_else(|p| p.into_inner());
    body()
}

/// A configured sidecar script path that actually NAMES a file.
///
/// Empty is the "not configured" sentinel, so this is two questions in one:
/// was a path set, and is it still there. The three sidecars with a script
/// (describe, embed, correspond) each asked it before doing expensive work —
/// before staging pixels, before the index build's first frame, before the
/// worker pool starts — and each answered it locally until this seam existed.
///
/// It does NOT ask whether Python runs: that is a separate door, and the GUI
/// owns it (`bin/gui/util.rs`).
pub fn sidecar_script_present(script: &std::path::Path) -> bool {
    !script.as_os_str().is_empty() && script.exists()
}

/// Shared executor for the single-artifact MODEL sidecars (`embed.rs`,
/// `correspond.rs`): spawn under [`with_model_slot`], bound the wait, apply
/// the exit-0-is-not-success contract, and hand back the artifact's text.
/// Extracted when the correspondence bridge would have been the FOURTH copy
/// of this sequence — the two callers here are byte-for-byte the same shape;
/// `denoise.rs` and `segment.rs` keep their own variants for now (staged
/// output conversion, stdout report parsing, the probe door — real
/// per-bridge differences, registered as a follow-up rather than flattened
/// in passing).
///
/// The steps, and why:
/// * env ALLOWLIST (`config::dotenv_child_env`) + `-E` in the caller's argv —
///   the two layers against a PYTHON* import hijack;
/// * stdio CAPTURED, never inherited: the release GUI has no console, so an
///   inherited handle would discard the reason a missing dependency failed;
/// * the slot covers the child's whole life — what must be exclusive is the
///   model RESIDENT in the GPU, and the sidecar timeout
///   (`denoise::bounded_child_output`) doubles as the gate's release;
/// * exit 0 alone is not success ([`sidecar_wrote`]) — THIS run must have
///   produced the artifact;
/// * every refusal runs `denoise::discard_failed_output`, so a failed run
///   cannot leave a half-artifact behind masquerading as a result.
pub fn run_model_sidecar(
    who: &str,
    python_bin: &str,
    args: Vec<std::ffi::OsString>,
    output: &std::path::Path,
) -> anyhow::Result<String> {
    run_model_sidecar_bounded(who, python_bin, args, output, None)
}

/// Variant used for structured sidecar outputs whose size is part of the
/// security contract (for example the multi-class manifest).
pub fn run_model_sidecar_bounded(
    who: &str,
    python_bin: &str,
    args: Vec<std::ffi::OsString>,
    output: &std::path::Path,
    max_bytes: Option<u64>,
) -> anyhow::Result<String> {
    use anyhow::{bail, Context};
    crate::pipeline::ensure_parent(output)?;
    // The output name may already exist (a previous run's leftover, or a
    // caller's 0-byte claim file), so the artifact state is sampled BEFORE
    // the spawn.
    let before = artifact_state(output);
    let mut cmd = std::process::Command::new(python_bin);
    cmd.envs(crate::config::dotenv_child_env());
    cmd.envs(crate::config::Config::sidecar_child_env());
    cmd.args(args)
        // Where the model weights live — one policy, appended here so all
        // three sidecars this function launches agree (see `weights_args`).
        .args(crate::config::Config::load().weights_args())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    hide_child_console(&mut cmd);
    arm_kill_group(&mut cmd);
    let discard = || crate::denoise::discard_failed_output(output, before);
    let out = with_model_slot(|| -> anyhow::Result<std::process::Output> {
        let child = cmd.spawn().with_context(|| {
            format!("launch {who} ({python_bin}) — is Python on PATH / AUTOSHADE_PYTHON set?")
        })?;
        let group = assign_kill_group(&child);
        crate::denoise::bounded_child_output(
            child,
            who,
            crate::denoise::sidecar_timeout(),
            "AUTOSHADE_SIDECAR_TIMEOUT_SECS",
            group,
        )
    })
    .inspect_err(|_| discard())?;
    if !out.status.success() {
        discard();
        let reason = match out.status.code() {
            Some(c) => c.to_string(),
            None => "signal".to_string(),
        };
        bail!("{who} exited with {reason}: {}", sidecar_tail(&out.stderr, &out.stdout));
    }
    sidecar_wrote(who, output, before).inspect_err(|_| discard())?;
    if let Some(cap) = max_bytes {
        let len = std::fs::metadata(output)
            .with_context(|| format!("stat {who} output {}", output.display()))?
            .len();
        if len > cap {
            discard();
            bail!("{who} output is too large ({} bytes; cap {} bytes)", len, cap);
        }
        let bytes = std::fs::read(output)
            .with_context(|| format!("read {who} output {}", output.display()))?;
        return String::from_utf8(bytes).with_context(|| format!("read {who} output {}", output.display()));
    }
    std::fs::read_to_string(output).with_context(|| format!("read {who} output {}", output.display()))
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

/// TEST-ONLY: a source file with its test module cut off.
///
/// Several tests pin an arm they cannot EXECUTE — a `cfg(target_os)` branch
/// for another platform — by asserting the arm still exists in the source
/// (`include_str!`). Written naively that assertion is VACUOUS: the needle is
/// spelled inside the assertion, the assertion is inside the file, so the file
/// contains the needle whether or not the arm survives. The macOS store-name
/// mutation passed a green test that way before this existed.
///
/// Cutting the test code off is what makes such a check able to fail, and the
/// cut is made at the first test MODULE rather than at the first `#[cfg(test)]`
/// of any kind. That distinction is not pedantry: `store.rs` carries test-only
/// free functions from line 739 onwards, thousands of lines above its test
/// modules, so cutting at the first attribute threw away most of the
/// production source — and an assertion about an arm defined below that point
/// then failed no matter what the arm said, which is a red that proves nothing.
///
/// It is deliberately the WHOLE rule in one place: the two hand-rolled copies
/// this replaced were correct only because what they searched for happened to
/// sit above every `#[cfg(test)]` in their file.
#[cfg(test)]
pub(crate) fn source_before_tests(src: &str) -> &str {
    const ATTR: &str = "#[cfg(test)]";
    let mut at = 0;
    while let Some(i) = src[at..].find(ATTR) {
        let start = at + i;
        at = start + ATTR.len();
        if src[at..].trim_start().starts_with("mod ") {
            return &src[..start];
        }
    }
    src
}

/// TEST-ONLY: a unique-per-test fixture dir. Fixed names let two concurrent
/// test processes (nextest, a second worktree) delete each other's fixtures
/// mid-run, and a leftover output from an aborted run flips assertions — so
/// the name carries the caller's tag AND the process id. Extracted with
/// [`write_stand_in`] (step 7a) from the per-module `tdir` copies in
/// `denoise.rs` and `advisor/claude.rs`. (The GUI bin crate keeps its own
/// copies — a dependency's `cfg(test)` items are not compiled into it.)
#[cfg(test)]
mod fixture_dir_tests {
    /// Every fixture path a test builds under `temp_dir()` must carry the
    /// process id: fixed names let concurrent test processes (a second
    /// worktree's battery, nextest) delete and overwrite each other's fixtures
    /// mid-run. Measured 2026-08-28 before the sweep: three concurrent
    /// processes looping three fixed-name `store` tests failed 29 of 36 runs
    /// (`Os error 5` and cross-process assertion reads); 0 of 36 after.
    #[test]
    fn test_fixture_dirs_are_process_unique() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")), &mut files);
        let mut offenders = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            for (index, line) in text.lines().enumerate() {
                if line.contains("temp_dir()") && line.contains(concat!(".join(", "\"autoshade-")) {
                    offenders.push(format!("{}:{}", file.display(), index + 1));
                }
            }
        }
        assert!(offenders.is_empty(), "fixed-name fixture dirs (add std::process::id()): {offenders:#?}");
    }
}

#[cfg(test)]
pub(crate) fn test_dir(tag: &str) -> std::path::PathBuf {
    use std::{env, fs, process};
    let dir = env::temp_dir().join(format!("autoshade-{tag}-{}", process::id()));
    fs::remove_dir_all(&dir).ok();
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// TEST-ONLY: write a platform stand-in "interpreter" script into `dir` and
/// return its path — the shared ceremony behind every sidecar-bridge loopback
/// test (a `.bat` on Windows, a `chmod +x` sh script elsewhere). Extracted
/// when `correspond.rs`'s tests would have been the third copy of the
/// `#[cfg(windows)]` / permissions boilerplate (`denoise.rs`'s harness and
/// `advisor/claude.rs`'s CLI stub carry the earlier two; the claude stub
/// speaks a different protocol and converting it is registered follow-up,
/// not flattened in passing). The BODIES are the test's own — this helper
/// owns only the platform ceremony, so no behaviour moves.
#[cfg(test)]
pub(crate) fn write_stand_in(
    dir: &std::path::Path,
    name: &str,
    bat_body: &str,
    sh_body: &str,
) -> String {
    #[cfg(windows)]
    {
        let _ = sh_body;
        let p = dir.join(format!("{name}.bat"));
        std::fs::write(&p, bat_body).unwrap();
        p.to_string_lossy().into_owned()
    }
    #[cfg(not(windows))]
    {
        let _ = bat_body;
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(format!("{name}.sh"));
        std::fs::write(&p, format!("#!/bin/sh\n{sh_body}")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{artifact_state, sidecar_tail, sidecar_wrote, source_before_tests};

    /// The cut is at the first test MODULE, not the first `#[cfg(test)]`.
    ///
    /// This test exists because the first version cut at the attribute and was
    /// WRONG in the one file that matters: `store.rs` defines test-only free
    /// functions thousands of lines above its test modules, so the cut threw
    /// away most of the production source and an assertion about an arm below
    /// it failed whatever the arm said. A source assertion that cannot pass and
    /// one that cannot fail are the same defect wearing different colours.
    #[test]
    fn the_source_cut_keeps_test_only_helpers_and_drops_only_the_test_modules() {
        let src = concat!(
            "fn production() {}\n",
            "#[cfg(test)]\n",
            "fn a_test_only_helper() { NEEDLE }\n",
            "fn more_production() { ARM }\n",
            "#[cfg(test)]\n",
            "mod tests { assert!(src.contains(\"ARM\")); }\n",
        );
        let kept = source_before_tests(src);
        assert!(kept.contains("ARM"), "production below a test-only fn must survive the cut");
        assert!(kept.contains("NEEDLE"), "the cut is not at the attribute");
        assert!(!kept.contains("mod tests"), "the test module itself must be gone");
        // And a file with no test module at all is returned whole, rather than
        // silently becoming empty.
        assert_eq!(source_before_tests("fn only_production() {}"), "fn only_production() {}");
    }

    /// M-D2: the `len == 0` refusal weakened; M-D3: the `before` comparison
    /// dropped. All four arms of the contract, each on its own unique dir so
    /// concurrent test processes cannot clobber each other's fixtures.
    #[test]
    fn a_sidecar_result_must_be_this_runs_own_nonempty_write() {
        let dir = std::env::temp_dir()
            .join(format!("autoshade-lib-test-wrote-{}-{}", std::process::id(), line!()));
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
