//! Whether the two local models may run, resolved once and carried as values,
//! and the stamp naming what measured a library.

/// The two environment overrides, spelled once and read in exactly one place
/// each. Everything downstream takes the resolved VALUE: `cargo test` runs on
/// parallel threads in one process, and a switch read from the environment
/// mid-run could be flipped by an unrelated test.
const ENV_EMBED: &str = "AUTOSHADE_STYLE_EMBED";
const ENV_DESCRIBE: &str = "AUTOSHADE_STYLE_DESCRIBE";

/// flag > environment > preference. An environment variable that is SET wins
/// over the preference whatever its value, including `0` — that is what makes
/// it an override.
fn resolve(flag: Option<bool>, pref: bool, env: &str, get: impl Fn(&str) -> Option<String>) -> bool {
    flag.unwrap_or_else(|| match get(env) {
        Some(v) => !matches!(v.trim(), "" | "0" | "false" | "off"),
        None => pref,
    })
}

fn live(k: &str) -> Option<String> {
    crate::config::live_env_os(k).map(|v| v.to_string_lossy().into_owned())
}

/// May the SigLIP sidecar run? Opt-in: its first run downloads 1.50 GB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingSwitch(bool);

impl EmbeddingSwitch {
    pub const ON: Self = EmbeddingSwitch(true);
    pub const OFF: Self = EmbeddingSwitch(false);

    pub fn on(self) -> bool {
        self.0
    }

    pub fn resolve(flag: Option<bool>, pref: bool) -> Self {
        Self::resolve_with(flag, pref, live)
    }

    /// [`resolve`](Self::resolve) over an explicit environment.
    pub fn resolve_with(flag: Option<bool>, pref: bool, get: impl Fn(&str) -> Option<String>) -> Self {
        EmbeddingSwitch(resolve(flag, pref, ENV_EMBED, get))
    }
}

/// May the Qwen describe sidecar run during a build? Opt-in: its first run
/// downloads 4.3 GB. A library built without it has no look phrases, and its
/// searches ask only the scene question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DescribeSwitch(bool);

impl DescribeSwitch {
    pub const ON: Self = DescribeSwitch(true);
    pub const OFF: Self = DescribeSwitch(false);

    pub fn on(self) -> bool {
        self.0
    }

    pub fn resolve(flag: Option<bool>, pref: bool) -> Self {
        Self::resolve_with(flag, pref, live)
    }

    /// [`resolve`](Self::resolve) over an explicit environment.
    pub fn resolve_with(flag: Option<bool>, pref: bool, get: impl Fn(&str) -> Option<String>) -> Self {
        DescribeSwitch(resolve(flag, pref, ENV_DESCRIBE, get))
    }
}

/// What measured a library, in one string: both pinned checkpoints, the text
/// tokenizer, and the phrase prompt's version. Two libraries with different
/// stamps hold incomparable vectors or differently worded phrases, so a
/// rebuild under a new stamp measures everything again.
pub(super) fn model_stamp() -> String {
    format!(
        "{}@{} tokenizer={} {}@{} phrases-v{}",
        crate::embed::MODEL_REPO,
        crate::embed::MODEL_REVISION,
        crate::embed::TEXT_TOKENIZER_CLASS,
        crate::describe::MODEL_REPO,
        crate::describe::MODEL_REVISION,
        crate::describe::PROMPT_VERSION,
    )
}
