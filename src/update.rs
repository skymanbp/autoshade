//! Is there a newer AutoShade? (v1.18.5.)
//!
//! One question, asked of one place: GitHub's "latest release" record for this
//! repository. This module asks it, reads the answer, and compares the answer
//! with the version of the build that is running. It never downloads or
//! installs anything, and it holds no GUI types — the desktop app decides when
//! to ask and what to show (`src/bin/gui/update_check.rs`).
//!
//! What goes out is exactly one `GET` of [`LATEST_API`] with two headers of our
//! own, `User-Agent: AutoShade/<version>` and
//! `Accept: application/vnd.github+json`. No key (the advisor's request
//! builders are not used, because they attach keys), no identifier, nothing
//! about the user or their photos. The HTTP client adds the `Host` line and,
//! because its gzip feature is on, `Accept-Encoding: gzip`; neither carries
//! anything about the user. Redirects are not followed: the address is fixed,
//! so a redirect is an answer we did not ask for.
//!
//! The answer is UNTRUSTED. At most [`MAX_BODY`] bytes are read; a longer body
//! is refused, not truncated and parsed. Only two fields are used:
//! `tag_name`, accepted only as `v?MAJOR.MINOR.PATCH` in plain digits and
//! compared number by number, and `html_url`, used only when it points inside
//! this repository on github.com — otherwise the fixed [`RELEASES_PAGE`].
//! Nothing else from the reply reaches a caller, so no reply text can reach
//! the screen.

use std::io::Read;
use std::time::Duration;

/// The one address asked.
pub const LATEST_API: &str = "https://api.github.com/repos/skymanbp/autoshade/releases/latest";

/// The page shown when the reply's own page is missing or points elsewhere.
pub const RELEASES_PAGE: &str = "https://github.com/skymanbp/autoshade/releases/latest";

/// A reply's `html_url` is used only when it starts with this.
const REPO_PAGE_PREFIX: &str = "https://github.com/skymanbp/autoshade/";

/// The most of a reply read: GitHub's release record is a few kilobytes
/// (a few tens with long notes), so 1 MiB is generous and still bounded.
pub const MAX_BODY: u64 = 1 << 20;

/// How long a connection may take to open.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the whole request may take, reply included.
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(10);

/// A release number: three plain decimal fields. The derived order compares
/// `major`, then `minor`, then `patch`, each as a number — so 1.18.10 is
/// newer than 1.18.9.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl Version {
    /// `v?MAJOR.MINOR.PATCH`, each field one or more ASCII digits and nothing
    /// else — no pre-release or build suffix, no spaces, no sign, no fourth
    /// field. A field too large for `u64` is refused too.
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.strip_prefix('v').unwrap_or(s);
        let mut fields = s.split('.');
        let mut next = || {
            let f = fields.next()?;
            if f.is_empty() || !f.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            f.parse::<u64>().ok()
        };
        let v = Version { major: next()?, minor: next()?, patch: next()? };
        if fields.next().is_some() {
            return None;
        }
        Some(v)
    }

    /// The version of the build that is running.
    pub fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate's own version is MAJOR.MINOR.PATCH")
    }
}

/// What the latest release is, as far as the reply says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// A page inside this repository on github.com — the reply's own, or
    /// [`RELEASES_PAGE`].
    pub page: String,
}

/// Why a check could not answer. The three cases a person can act on; no
/// reply text rides along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckError {
    /// No answer came: no network, the name did not resolve, the connection
    /// failed or timed out.
    Offline,
    /// GitHub answered with something other than success (403, 404, 429, a
    /// redirect, a server error).
    Refused,
    /// An answer came but is not a release record we can read: too long, not
    /// JSON, no `tag_name`, or a tag that is not a plain version.
    Unreadable,
}

/// Where and how patiently to ask. [`Endpoint::default`] is the real one;
/// tests point it at a local stub with short timeouts.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub url: String,
    pub connect_timeout: Duration,
    pub timeout: Duration,
}

impl Default for Endpoint {
    fn default() -> Self {
        Endpoint { url: LATEST_API.to_owned(), connect_timeout: CONNECT_TIMEOUT, timeout: TOTAL_TIMEOUT }
    }
}

/// Ask `ep` for the latest release. Blocking — call it off the UI thread.
pub fn fetch_latest(ep: &Endpoint) -> Result<Release, CheckError> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(ep.connect_timeout)
        .timeout(ep.timeout)
        .redirects(0)
        .build();
    let resp = agent
        .get(&ep.url)
        .set("User-Agent", &format!("AutoShade/{}", env!("CARGO_PKG_VERSION")))
        .set("Accept", "application/vnd.github+json")
        .call();
    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(..)) => return Err(CheckError::Refused),
        Err(ureq::Error::Transport(_)) => return Err(CheckError::Offline),
    };
    // A redirect arrives here as an Ok (redirects are off).
    if !(200..300).contains(&resp.status()) {
        return Err(CheckError::Refused);
    }
    let mut body = Vec::new();
    // One byte past the cap tells "exactly the cap" from "longer".
    if resp.into_reader().take(MAX_BODY + 1).read_to_end(&mut body).is_err() {
        // The connection broke or timed out mid-reply.
        return Err(CheckError::Offline);
    }
    if body.len() as u64 > MAX_BODY {
        return Err(CheckError::Unreadable);
    }
    parse_release(&body)
}

/// Read a release record: its `tag_name` as a plain version, and its
/// `html_url` only when it points inside this repository.
pub fn parse_release(body: &[u8]) -> Result<Release, CheckError> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| CheckError::Unreadable)?;
    let tag = v.get("tag_name").and_then(|t| t.as_str()).ok_or(CheckError::Unreadable)?;
    let version = Version::parse(tag).ok_or(CheckError::Unreadable)?;
    let page = v
        .get("html_url")
        .and_then(|u| u.as_str())
        .filter(|u| page_is_ours(u))
        .unwrap_or(RELEASES_PAGE)
        .to_owned();
    Ok(Release { version, page })
}

/// A page the app may open: inside this repository on github.com, and
/// nothing a browser could read as more than one address.
fn page_is_ours(u: &str) -> bool {
    u.starts_with(REPO_PAGE_PREFIX) && u.len() <= 512 && u.bytes().all(|b| b.is_ascii_graphic())
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
