//! Claude provider — the data-only acceptance verifier ("收货验证").
//!
//! Shells out to the local `claude` CLI in print mode, reusing the user's
//! Claude Code OAuth (no API key). Invocation and envelope shape were verified
//! live against `claude` 2.1.210 on this machine:
//!   `claude -p --setting-sources "" --strict-mcp-config
//!    --disable-slash-commands --model <m> --output-format json "<prompt>"`
//!   → `{"type":"result","is_error":false,"result":"<model text>", ...}`
//! Isolation flags instead of `--bare`: since at least 2.1.210, `--bare`
//! documents "Anthropic auth is strictly ANTHROPIC_API_KEY or apiKeyHelper via
//! --settings (OAuth and keychain are never read)" — under `--bare` this
//! provider can never bill the user's subscription (it fails "Not logged in",
//! or worse, silently bills a stray API key). The three flags above reproduce
//! what `--bare` protected against — plugins/skills/hooks auto-loading into
//! the session (0 plugins enabled, 0 hooks registered, no user skills, clean
//! stderr; measured 2026-07-17) — while keeping the stored OAuth usable.

use std::process::Command;

use crate::config::Config;
use crate::decode::{Histogram, Meta};
use crate::recipe::EditRecipe;

use super::{build_verify_prompt, parse_verdict, Advisor, AdvisorError, Verdict};

pub struct ClaudeProvider {
    bin: String,
    model: String,
    /// The analysis role's reasoning-effort tier, or `None` for the CLI's own
    /// default. Passed as `--effort <level>`; `claude --help` documents
    /// `low, medium, high, xhigh, max` (measured 2026-08-11). The value is
    /// bounded by `config::effort` before it ever reaches argv.
    effort: Option<String>,
}

/// Did the CLI refuse the run because it does not know `--effort`?
///
/// The flag exists in current builds, but this repo is public and an older
/// `claude` must DEGRADE rather than fail every verification: clap answers an
/// unknown flag with a non-zero exit and `unexpected argument '--effort'`
/// (or `unknown option`), which arrives here as `CliFailed`. Matching the
/// refusal text is the CLI-side equivalent of `error_blames_param` on the
/// HTTP paths — attribution, not a blanket retry.
fn cli_rejected_effort(e: &AdvisorError) -> bool {
    let AdvisorError::CliFailed { stderr, .. } = e else {
        return false;
    };
    let s = stderr.to_ascii_lowercase();
    s.contains("--effort")
        && (s.contains("unexpected argument")
            || s.contains("unknown option")
            || s.contains("unrecognized"))
}

impl ClaudeProvider {
    pub fn new(cfg: &Config) -> Self {
        Self {
            bin: cfg.claude_bin.clone(),
            model: cfg.analysis_model.clone(),
            effort: cfg.analysis_effort.clone(),
        }
    }

    /// The child's full SHAPE at this provider's CONFIGURED effort — argv,
    /// env, cwd, console — with no spawn. The isolation contract lives here
    /// as one inspectable value (`get_args` / `get_envs` / `get_current_dir`
    /// back the tests that pin it); `run_verify` adds only the run mechanics
    /// (stdio, kill group, spawn, drain).
    ///
    /// Test-only because the run path must name the effort it is ATTEMPTING —
    /// the negotiation retry re-spawns at a different one — while the tests
    /// pin the shape a configured provider actually produces.
    #[cfg(test)]
    fn verify_command(&self) -> Command {
        self.verify_command_with(self.effort.as_deref())
    }

    /// The same shape with an EXPLICIT effort, so the negotiation retry can
    /// rebuild the child without it.
    fn verify_command_with(&self, effort: Option<&str>) -> Command {
        let mut cmd = Command::new(&self.bin);
        cmd.args([
            "-p",
            // NOT `--bare`: it never reads the stored OAuth login (see the
            // module docs) — these flags give the same isolation while
            // keeping the subscription auth.
            "--setting-sources",
            "",
            "--strict-mcp-config",
            "--disable-slash-commands",
            // NO TOOLS. This subprocess is a pure data-in / verdict-out call:
            // it receives numbers and returns JSON, and never needs to touch
            // the filesystem. But the prompt embeds the recipe's `rationale`
            // — text an upstream model wrote — so leaving the built-in tools
            // enabled left a path from model output to local file reads whose
            // contents could then travel to the verifier's model. `--tools ""`
            // is the CLI's documented way to disable all of them ("Use \"\"
            // to disable all tools"); measured on this machine, the verifier
            // still answers normally with it set.
            "--tools",
            "",
            "--model",
            &self.model,
            "--output-format",
            "json",
        ]);
        // Opt-in only: with no tier configured the flag is absent and the
        // invocation is byte-identical to every prior release, so a CLI that
        // predates `--effort` is never handed one unless the user asked for a
        // tier — and even then `cli_rejected_effort` retries without it.
        if let Some(e) = effort.filter(|e| !e.trim().is_empty()) {
            cmd.args(["--effort", e]);
        }
        // The prompt goes over STDIN, never argv (L11#7a): a recipe at the
        // sanitize caps pretty-prints to ~90-100 KB, 3x past CreateProcessW's
        // 32767-char command-line ceiling (measured: 32700 spawns, 33000
        // raises WinError 206) — and argv is world-readable (/proc/<pid>/
        // cmdline, WMI), while this prompt embeds the user's full recipe and
        // model-authored rationale text. The CLI documents the channel:
        // "Input must be provided either through stdin or as a prompt
        // argument when using --print" (claude 2.1.210, measured).
        // The .env's unprotected names travel to the child EXPLICITLY
        // (L16#3): under dotenv_override they sat in the process environment
        // and were inherited; the owned map never writes the parent, so the
        // reach is reproduced on the child's own block.
        //
        // What that block may contain is decided by the capability table, not
        // here: `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` and
        // `ANTHROPIC_BASE_URL` are `Trust::Destination` — for this child the
        // credential IS the routing decision — so a photo pack's `.env` can no
        // longer repoint or re-bill the verifier. It used to: the filter named
        // only Autoshop's own variables, and these three are the CLI's.
        //
        // The env_remove below is the OTHER half and stays: it strips an
        // INHERITED key, which no table can reach. A live-environment
        // `ANTHROPIC_BASE_URL` is deliberately left alone — that is a real
        // user's corporate gateway, set in their own environment.
        cmd.envs(crate::config::dotenv_child_env());
        // This provider bills the user's Claude subscription via the stored
        // OAuth login by design. A stray ANTHROPIC_API_KEY in the inherited
        // environment (e.g. a machine-wide env var meant for other tools)
        // takes precedence over that login and silently re-routes billing to
        // metered API credits — measured live 2026-07-17: with the key present
        // the identical invocation fails 400 "Credit balance is too low";
        // without it, the subscription answers. Strip it from the child.
        cmd.env_remove("ANTHROPIC_API_KEY");
        // Run the child from a neutral cwd. Headless `claude` treats its cwd as
        // the workspace: if that directory carries a `.claude/settings.json`
        // (any project checkout) and the workspace was never trusted
        // interactively, the CLI errors out — "Ignoring N permissions.allow
        // entries … this workspace has not been trusted" + exit 1 — and the
        // whole analysis fails. Verified live 2026-07-14: identical invocation
        // from `D:/Projects/Autoshop` prints the trust error, from a fresh temp
        // dir it does not. The verifier is a pure stdin/stdout call that never
        // touches files, so the temp dir (always present) is a safe workspace.
        cmd.current_dir(std::env::temp_dir());
        // Don't flash a console window when the windowed GUI spawns this CLI child.
        crate::hide_child_console(&mut cmd);
        cmd
    }
}

impl Advisor for ClaudeProvider {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn verify(
        &self,
        recipe: &EditRecipe,
        meta: &Meta,
        hist: &Histogram,
    ) -> Result<Verdict, AdvisorError> {
        let prompt = build_verify_prompt(recipe, meta, hist)?;
        // No tier configured ⇒ nothing to negotiate, and no second copy of a
        // ~100 KB prompt to hold.
        let Some(effort) = self.effort.as_deref() else {
            return self.run_verify(prompt, None);
        };
        let retry = prompt.clone();
        match self.run_verify(prompt, Some(effort)) {
            Err(e) if cli_rejected_effort(&e) => {
                eprintln!(
                    "  note: this `claude` build does not accept --effort — retrying without it \
                     (the CLI's own default applies)"
                );
                self.run_verify(retry, None)
            }
            other => other,
        }
    }
}

impl ClaudeProvider {
    /// ONE verification run at a fixed effort. Split out of `verify` so the
    /// unknown-flag negotiation above can repeat it — the retry re-spawns a
    /// child that never received the flag, rather than trying to edit an
    /// already-built `Command`.
    fn run_verify(&self, prompt: String, effort: Option<&str>) -> Result<Verdict, AdvisorError> {
        let mut cmd = self.verify_command_with(effort);
        // A hung `claude` child (network stall inside the CLI) used to block
        // the analysis worker FOREVER — `output()` has no deadline, while
        // every HTTP advisor path carries one. Spawn + poll with a hard
        // budget (same env override as the HTTP paths); on expiry the child
        // is killed and the error says exactly what happened.
        // env_or_dotenv: same .env-honouring rule as the HTTP builders
        // (the owned-map dotenv never touches the process environment).
        let budget = crate::config::env_or_dotenv("AUTOSHOP_HTTP_TIMEOUT_SECS")
            .and_then(|s| s.parse().ok())
            // 0 would kill the child on arrival — same guard as the HTTP
            // builders' zero filter.
            .filter(|s: &u64| *s > 0)
            .unwrap_or(300u64);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        // Tree-wide kill on timeout, like both python sidecars (L11#7b).
        crate::arm_kill_group(&mut cmd);
        let mut child = cmd.spawn()?;
        let group = crate::assign_kill_group(&child);
        // The prompt writer is a DEDICATED thread, not an inline write_all:
        // the prompt routinely exceeds the ~64 KiB pipe buffer, so an inline
        // write blocks until the child drains it while the child may already
        // be blocked writing to a stdout nobody drains yet — a two-pipe
        // deadlock. Dropping the handle closes stdin → the child sees EOF. A
        // broken pipe (child exited early) is `let _ =` by design: the real
        // diagnosis is the exit status plus the drained stderr. Joined AFTER
        // bounded_child_output, never before — a child that ignores stdin
        // must not park this caller past its own deadline.
        let mut sink = child.stdin.take();
        let payload = prompt.into_bytes();
        let writer = std::thread::spawn(move || {
            if let Some(mut s) = sink.take() {
                let _ = std::io::Write::write_all(&mut s, &payload);
            }
        });
        // The bounded drain + poll used to live here as a PRIVATE, weaker
        // copy: an unbounded `read_to_end` Vec (the repo's last one), and an
        // unconditional post-kill join whose "the pipes are closed" comment
        // denoise.rs documents as false — a descendant holding the inherited
        // pipe hung the analysis worker past its own deadline (L11#6). The
        // shared primitive caps each stream at 1 MiB (16x above the 64 KiB
        // verdict ceiling, so no legitimate run can regress), detaches a
        // stuck drain after a bounded grace, and discloses truncation.
        let outcome = crate::denoise::bounded_child_output(
            child,
            "claude verifier",
            std::time::Duration::from_secs(budget),
            "AUTOSHOP_HTTP_TIMEOUT_SECS",
            group,
        );
        // Joined on BOTH paths (Codex L11-1: the `?` used to skip the join on
        // the error path, leaking a parked thread per failed verification) —
        // and BOUNDED, the join_bounded philosophy: after the kill the pipe is
        // closed and write_all errors out, but an escaped descendant still
        // holding the read end would park the writer forever, and joining it
        // then hangs this worker past the deadline it just enforced.
        let grace = std::time::Instant::now();
        while !writer.is_finished()
            && grace.elapsed() < std::time::Duration::from_secs(2)
        {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if writer.is_finished() {
            let _ = writer.join();
        } else {
            eprintln!(
                "⚠ a descendant still holds the claude verifier's stdin — abandoning the prompt writer"
            );
        }
        let output = outcome.map_err(|e| AdvisorError::ClaudeError(e.to_string()))?;

        // The CLI envelope; we only need these fields (serde ignores the rest).
        #[derive(serde::Deserialize)]
        struct Envelope {
            is_error: bool,
            result: String,
        }

        if !output.status.success() {
            // A failed headless run usually exits 1 with an EMPTY stderr and
            // the real reason inside the stdout JSON envelope ("Credit balance
            // is too low", "Not logged in · Please run /login", …) — measured
            // 2026-07-17. Surface that text instead of a blank CliFailed.
            if let Ok(env) = serde_json::from_slice::<Envelope>(&output.stdout)
                && env.is_error
            {
                return Err(AdvisorError::ClaudeError(
                    super::BoundedUntrustedText::diagnostic(&env.result, &[]).into_string(),
                ));
            }
            let mut stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.is_empty() {
                let head: String =
                    String::from_utf8_lossy(&output.stdout).chars().take(300).collect();
                stderr = format!("(empty stderr) stdout: {head}");
            }
            return Err(AdvisorError::CliFailed {
                bin: self.bin.clone(),
                code: output.status.code(),
                stderr: super::BoundedUntrustedText::diagnostic(&stderr, &[]).into_string(),
            });
        }
        let env: Envelope =
            serde_json::from_slice(&output.stdout).map_err(|source| AdvisorError::BadEnvelope {
                source,
                head: super::BoundedUntrustedText::diagnostic(
                    &String::from_utf8_lossy(&output.stdout),
                    &[],
                )
                .into_string(),
            })?;
        if env.is_error {
            return Err(AdvisorError::ClaudeError(
                super::BoundedUntrustedText::diagnostic(&env.result, &[]).into_string(),
            ));
        }

        // `result` is the model's text — instructed to be exactly the Verdict
        // JSON, but LLMs intermittently add a fence or a reasoning preamble. Try
        // the bare (fence-stripped) text first; otherwise scan every balanced
        // {...} object and keep the LAST one that parses as a Verdict (the
        // model's final answer, past any example/prose objects).
        parse_verdict(&env.result, &[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::advisor::Decision;

    /// Unique-per-test fixture dir (the denoise stand-in idiom).
    fn tdir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("autoshop-claude-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A stand-in "claude" that records its argv, copies its STDIN to a
    /// file, and prints the given envelope file — all through absolute
    /// paths, because the provider runs its child from a neutral cwd.
    fn stand_in(dir: &std::path::Path, envelope: &str) -> String {
        let env_file = dir.join("envelope.json");
        std::fs::write(&env_file, envelope).unwrap();
        #[cfg(windows)]
        {
            let p = dir.join("claude.bat");
            let body = format!(
                "@echo off\r\necho %* > \"{argv}\"\r\nfindstr \"^\" > \"{prompt}\"\r\ntype \"{env}\"\r\n",
                argv = dir.join("argv.txt").display(),
                prompt = dir.join("prompt.txt").display(),
                env = env_file.display(),
            );
            std::fs::write(&p, body).unwrap();
            p.to_string_lossy().into_owned()
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let p = dir.join("claude.sh");
            let body = format!(
                "#!/bin/sh\nprintf '%%s ' \"$@\" > \"{argv}\"\ncat > \"{prompt}\"\ncat \"{env}\"\n",
                argv = dir.join("argv.txt").display(),
                prompt = dir.join("prompt.txt").display(),
                env = env_file.display(),
            );
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p.to_string_lossy().into_owned()
        }
    }

    fn fixture_meta() -> Meta {
        Meta {
            make: "TestCam".into(),
            model: "T1".into(),
            lens: None,
            iso: Some(100),
            shutter: Some("1/125".into()),
            aperture: Some(4.0),
            focal_length_mm: Some(50.0),
            exposure_bias_ev: Some(0.0),
            date_time: None,
            width: 6000,
            height: 4000,
            as_shot_wb_coeffs: [1.0; 4],
        }
    }

    fn fixture_hist() -> Histogram {
        Histogram {
            luma: vec![0; 256],
            r: vec![0; 256],
            g: vec![0; 256],
            b: vec![0; 256],
            clip_black_pct: 0.0,
            clip_white_pct: 0.0,
            sample_pixels: 1,
        }
    }

    const ACCEPT_ENVELOPE: &str = r#"{"type":"result","is_error":false,"result":"{\"decision\":\"accept\",\"reasons\":[]}"}"#;

    /// L11#7a: the prompt travels on STDIN, never argv — argv is capped at
    /// 32767 chars by CreateProcessW (a caps-level recipe pretty-prints to
    /// ~3x that) and is world-readable, while the prompt embeds the user's
    /// full recipe. The positive control rides along: a well-behaved
    /// envelope still parses through the bounded drain (L11#6).
    #[test]
    fn claude_verifier_sends_its_prompt_on_stdin_not_argv() {
        let dir = tdir("stdin");
        let provider = ClaudeProvider {
            bin: stand_in(&dir, ACCEPT_ENVELOPE),
            model: "test-model".into(),
            effort: None,
        };
        let recipe = EditRecipe {
            rationale: "MARKER_RATIONALE_ON_STDIN_XYZ".into(),
            exposure_ev: 0.5,
            ..Default::default()
        };
        let verdict = provider
            .verify(&recipe, &fixture_meta(), &fixture_hist())
            .expect("the stand-in answers a valid envelope");
        assert!(matches!(verdict.decision, Decision::Accept));
        let prompt = std::fs::read_to_string(dir.join("prompt.txt")).expect("stdin was written");
        assert!(
            prompt.contains("MARKER_RATIONALE_ON_STDIN_XYZ"),
            "the recipe reaches the child on stdin"
        );
        let argv = std::fs::read_to_string(dir.join("argv.txt")).unwrap_or_default();
        assert!(
            !argv.contains("MARKER_RATIONALE_ON_STDIN_XYZ"),
            "the recipe must NOT travel on argv: {argv}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A recipe at the sanitize caps produces a prompt past the Windows
    /// 32767-char argv ceiling (measured: 32700 spawns, 33000 raises
    /// WinError 206) — it must still reach the verifier. The premise is
    /// asserted so it cannot rot.
    #[test]
    fn an_oversized_verify_prompt_still_reaches_the_verifier() {
        use crate::recipe::{CurvePoint, LocalAdjustment, MaskGeometry};
        let dir = tdir("oversized");
        let provider = ClaudeProvider {
            bin: stand_in(&dir, ACCEPT_ENVELOPE),
            model: "test-model".into(),
            effort: None,
        };
        let recipe = EditRecipe {
            rationale: "r".repeat(4096),
            tone_curve: (0..=255)
                .map(|i| CurvePoint { input: i as u8, output: i as u8 })
                .collect(),
            masks: (0..64)
                .map(|i| LocalAdjustment {
                    mask: MaskGeometry::Radial {
                        top: 0.1,
                        left: 0.1,
                        bottom: 0.9,
                        right: 0.9,
                        feather: 0.5,
                        roundness: 0.0,
                        flipped: false,
                        angle: 0.0,
                    },
                    name: format!("mask {i}"),
                    exposure_ev: 0.1,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let prompt = build_verify_prompt(&recipe, &fixture_meta(), &fixture_hist()).unwrap();
        assert!(
            prompt.len() > 32_767,
            "premise: a caps-level prompt exceeds the argv ceiling (got {})",
            prompt.len()
        );
        let verdict = provider
            .verify(&recipe, &fixture_meta(), &fixture_hist())
            .expect("an oversized prompt still verifies (argv would refuse at spawn)");
        assert!(matches!(verdict.decision, Decision::Accept));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L11#6: a child that floods stdout can neither exhaust memory nor
    /// hang the analysis worker — the shared bounded drain caps the stream
    /// at 1 MiB and the truncation is disclosed in the error, not silent.
    #[test]
    fn claude_verifier_output_is_bounded_and_cannot_hang_the_worker() {
        let dir = tdir("flood");
        // ~1.2 MiB of noise instead of an envelope.
        let big = dir.join("big.txt");
        let line = format!("{}\n", "x".repeat(4095));
        std::fs::write(&big, line.repeat(300)).unwrap();
        #[cfg(windows)]
        let bin = {
            let p = dir.join("claude.bat");
            std::fs::write(
                &p,
                format!("@echo off\r\nfindstr \"^\" > nul\r\ntype \"{}\"\r\n", big.display()),
            )
            .unwrap();
            p.to_string_lossy().into_owned()
        };
        #[cfg(not(windows))]
        let bin = {
            use std::os::unix::fs::PermissionsExt;
            let p = dir.join("claude.sh");
            std::fs::write(
                &p,
                format!("#!/bin/sh\ncat > /dev/null\ncat \"{}\"\n", big.display()),
            )
            .unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p.to_string_lossy().into_owned()
        };
        let provider = ClaudeProvider { bin, model: "test-model".into(), effort: None };
        let err = provider
            .verify(&EditRecipe::default(), &fixture_meta(), &fixture_hist())
            .expect_err("a flooded stdout is not a verdict");
        let text = err.to_string();
        assert!(
            text.contains("exceeded 1 MiB"),
            "the truncation is disclosed, not silent: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The effort tier is OPT-IN on the wire: with none configured the argv
    /// must be byte-identical to every prior release, so a `claude` build
    /// that predates `--effort` is never handed one. With a tier configured
    /// it is appended as a flag/value pair at the END, leaving the pinned
    /// isolation sequence above untouched.
    #[test]
    fn the_effort_tier_reaches_argv_only_when_configured() {
        let args = |p: &ClaudeProvider, e: Option<&str>| -> Vec<String> {
            p.verify_command_with(e)
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };
        let plain = ClaudeProvider { bin: "claude-test".into(), model: "m".into(), effort: None };
        assert!(
            !args(&plain, None).iter().any(|a| a == "--effort"),
            "an unconfigured tier must not change the invocation at all"
        );
        // Blank is the "provider default" choice the UI writes — same thing.
        assert!(!args(&plain, Some("  ")).iter().any(|a| a == "--effort"));

        let tiered =
            ClaudeProvider { bin: "claude-test".into(), model: "m".into(), effort: Some("xhigh".into()) };
        let got = args(&tiered, tiered.effort.as_deref());
        assert_eq!(
            got[got.len() - 2..],
            ["--effort".to_string(), "xhigh".to_string()],
            "the tier is appended last: {got:?}"
        );
        // …and the negotiation retry rebuilds the SAME child without it.
        assert_eq!(args(&tiered, None), args(&plain, None), "the retry drops only the tier");

        // Attribution, not a blanket retry: only a refusal that names the
        // flag re-runs, and only from the CLI-failure shape.
        let refused = AdvisorError::CliFailed {
            bin: "claude".into(),
            code: Some(2),
            stderr: "error: unexpected argument '--effort' found".into(),
        };
        assert!(cli_rejected_effort(&refused));
        let unrelated = AdvisorError::CliFailed {
            bin: "claude".into(),
            code: Some(1),
            stderr: "Credit balance is too low".into(),
        };
        assert!(!cli_rejected_effort(&unrelated), "an unrelated failure must not re-run the call");
        assert!(!cli_rejected_effort(&AdvisorError::ClaudeError("--effort".into())));
    }

    /// L14#3: the isolation contract is the argv itself, so the test pins
    /// the EXACT sequence — flag order, both empty-string values in their
    /// adjacent positions, and no positional prompt (it travels on stdin).
    /// A `contains` check would miss a dropped `""` or a reordered pair.
    #[test]
    fn the_verifier_child_argv_pins_every_isolation_flag() {
        let provider = ClaudeProvider { bin: "claude-test".into(), model: "m-1".into(), effort: None };
        let cmd = provider.verify_command();
        let args: Vec<String> =
            cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_eq!(
            refs,
            [
                "-p",
                "--setting-sources",
                "",
                "--strict-mcp-config",
                "--disable-slash-commands",
                "--tools",
                "",
                "--model",
                "m-1",
                "--output-format",
                "json",
            ],
            "the argv IS the isolation contract — no plugins/skills/hooks, no MCP, \
             no slash commands, no tools, and no trailing prompt argument"
        );
        assert!(
            !args.iter().any(|a| a == "--bare"),
            "--bare never reads the stored OAuth login (module docs) — it must not come back"
        );
        assert_eq!(cmd.get_program(), std::ffi::OsStr::new("claude-test"));
    }

    /// A stray ANTHROPIC_API_KEY re-routes billing from the user's
    /// subscription to metered credits, and a project-checkout cwd makes
    /// headless `claude` exit 1 on workspace trust — both guards are part
    /// of the child's shape and both are pinned here.
    #[test]
    fn the_verifier_child_strips_the_api_key_and_runs_from_a_neutral_cwd() {
        let provider = ClaudeProvider { bin: "claude-test".into(), model: "m".into(), effort: None };
        let cmd = provider.verify_command();
        // get_envs order is unspecified — assert membership of the
        // (key, None) removal pair, never a position.
        assert!(
            cmd.get_envs()
                .any(|(k, v)| k == std::ffi::OsStr::new("ANTHROPIC_API_KEY") && v.is_none()),
            "the child must strip ANTHROPIC_API_KEY (billing isolation)"
        );
        // Computed with the same call the builder makes — TMP/TEMP redirects
        // must not fail this spuriously.
        assert_eq!(
            cmd.get_current_dir(),
            Some(std::env::temp_dir().as_path()),
            "the child runs from the neutral temp dir (workspace-trust isolation)"
        );
    }
}
