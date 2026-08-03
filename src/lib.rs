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
    use super::sidecar_tail;

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
