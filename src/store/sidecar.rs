//! Sidecars and legacy files: the capped readers, the stamped sidecar read, and the legacy out roots.

use super::*;

/// UI-written local settings (used to be a cwd-relative `autoshade.local.json`).
pub fn settings_path() -> PathBuf {
    store_root().join(crate::config::SETTINGS_FILE)
}

/// Candidate legacy ./out roots, most-specific first. Pre-store sidecars were
/// CWD-relative, so where they sit depends on how the app used to be launched:
/// a terminal launch put them under the project dir (= today's cwd when
/// launched the same way), a double-click put them beside the exe. An
/// `AUTOSHADE_LEGACY_OUT` env override covers any other history. Without the
/// exe-dir probe, upgrading users who start the exe from a NEW directory would
/// see every pre-store develop silently vanish.
pub(super) fn legacy_out_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    // env_or_dotenv (L16#3): a .env-set legacy root kept working when the
    // dotenv stopped writing the process environment.
    if let Some(o) = crate::config::env_or_dotenv("AUTOSHADE_LEGACY_OUT") {
        roots.push(PathBuf::from(o));
    }
    roots.push(PathBuf::from("out"));
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let cand = dir.join("out");
        let dup = roots
            .iter()
            .any(|r| std::path::absolute(r).ok() == std::path::absolute(&cand).ok());
        if !dup {
            roots.push(cand);
        }
    }
    roots
}

/// The photo's legacy recipe path: the first candidate root that actually
/// holds one (else the cwd-relative default, so error messages stay sensible).
pub fn legacy_recipe(src: &Path) -> PathBuf {
    let name = format!("{}.recipe.json", crate::pipeline::stem(src));
    if legacy_suppressed(src) {
        suppressed_legacy_path(src, &name)
    } else {
        legacy_file(&name)
    }
}

#[cfg(test)]
pub(super) fn legacy_recipe_in(root: &Path, legacy_root: &Path, src: &Path) -> PathBuf {
    let name = format!("{}.recipe.json", crate::pipeline::stem(src));
    if legacy_suppressed_in(root, src) {
        suppressed_legacy_path_in(root, src, &name)
    } else {
        legacy_root.join(name)
    }
}

pub fn legacy_xmp(src: &Path) -> PathBuf {
    let name = format!("{}.xmp", crate::pipeline::stem(src));
    if legacy_suppressed(src) {
        suppressed_legacy_path(src, &name)
    } else {
        legacy_file(&name)
    }
}

fn legacy_file(name: &str) -> PathBuf {
    let mut fallback = None;
    for root in legacy_out_roots() {
        let p = root.join(name);
        if p.exists() {
            return p;
        }
        fallback.get_or_insert(p);
    }
    fallback.unwrap_or_else(|| PathBuf::from("out").join(name))
}

pub(super) const LEGACY_TOMBSTONE: &[u8] =
    b"{\"v\":1,\"legacy_fallback\":\"suppressed\",\"reason\":\"explicit_clear\"}\n";

fn legacy_tombstone_in(root: &Path, src: &Path) -> PathBuf {
    develop_dir_in(root, src).join("legacy.tombstone")
}

pub(super) fn legacy_suppressed(src: &Path) -> bool {
    legacy_suppressed_in(&store_root(), src)
}

pub(super) fn legacy_suppressed_in(root: &Path, src: &Path) -> bool {
    legacy_tombstone_in(root, src).exists()
}

pub(super) fn suppress_legacy_in(root: &Path, src: &Path) -> std::io::Result<()> {
    durable_write(&legacy_tombstone_in(root, src), LEGACY_TOMBSTONE)
}

fn suppressed_legacy_path(src: &Path, name: &str) -> PathBuf {
    suppressed_legacy_path_in(&store_root(), src, name)
}

pub(super) fn suppressed_legacy_path_in(root: &Path, src: &Path, name: &str) -> PathBuf {
    develop_dir_in(root, src).join(".legacy-suppressed").join(name)
}

/// Does this photo have ANY saved develop — central or legacy, recipe or XMP?
/// Existence-only (no parse): the gallery badge and the web list call this per
/// photo per refresh.
/// The explicit-clear transaction marker (L03): written durably BEFORE
/// the sweep begins, consumed only after the cleared stamp lands. While it
/// exists the develop is "being removed": [`has_develop`] answers false,
/// the newest-intent ranking treats it like the cleared stamp, and
/// recovery completes the sweep on the next locked touch.
pub(super) fn clear_pending(src: &Path) -> PathBuf {
    clear_pending_in(&store_root(), src)
}

pub(super) fn clear_pending_in(root: &Path, src: &Path) -> PathBuf {
    develop_dir_in(root, src).join("clear.pending")
}

pub fn has_develop(src: &Path) -> bool {
    // A PENDING explicit clear means "this develop is being removed" — the
    // surviving files are sweep leftovers, not a save (L03).
    if clear_pending(src).exists() {
        return false;
    }
    // recipe.json.bak covers the retire window: a crash between retire and
    // publish leaves ONLY the .bak — recover_orphan_baks republishes it on
    // the next real read — so answering "nothing saved" here was a lie that
    // could send a caller into a second paid analysis over a save the
    // recovery machinery still guarantees.
    let recipe = recipe_target(src);
    recipe.exists()
        || recipe.with_extension("json.bak").exists()
        || xmp_target(src).exists()
        || legacy_recipe(src).exists()
        || legacy_xmp(src).exists()
}

/// What a bounded sidecar read found. `Missing` and `Unreadable` are NOT the
/// same thing to a caller choosing a merge base: a missing file carries
/// nothing, while an unreadable one may carry the user's Lightroom work — and
/// collapsing both into one "absent" let the XMP writer fall back to
/// our own previous projection with no note, so an over-the-cap Lightroom
/// sidecar lost its properties in exactly the silence the merge note was
/// built to end.
pub enum SidecarRead {
    /// No file at the path.
    Missing,
    /// The file, in full.
    Ok(String),
    /// A file IS there but cannot be used, and this is why (used verbatim in
    /// user-facing notes).
    Unreadable(&'static str),
}

/// The bounded ceiling for the app's own JSON/text sidecars (recipe.json,
/// variants.json, pixels.json, version snapshots, settings). Far above any
/// legitimate file this app writes (recipe strings are clamp-capped; a real
/// variants.json is kilobytes), yet it stops a photo pack's 2 GB
/// "recipe.json" from materialising in RAM on open (L02).
pub const MAX_STORE_JSON: u64 = 16 * 1024 * 1024;

/// Bounded replacement for `std::fs::read_to_string` on files the app itself
/// persists but an untrusted photo pack can replace wholesale. Over the cap →
/// `InvalidData` naming the limit; `NotFound` passes through untouched, so
/// every caller's existing "unreadable ≠ absent" branching keeps its shape.
/// Bytes first, text second — same rationale as [`read_sidecar_checked`]:
/// `read_to_string` on a `Take` that cuts through a multi-byte character
/// reports the wrong reason.
pub fn read_text_capped(path: &Path, cap: u64) -> std::io::Result<String> {
    let buf = read_bytes_capped(path, cap)?;
    String::from_utf8(buf).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} is not readable UTF-8 text", path.display()),
        )
    })
}

/// [`read_text_capped`] for binary payloads (`variants.json`/`pixels.json`
/// parse from slices).
pub fn read_bytes_capped(path: &Path, cap: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    let f = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    let n = f.take(cap + 1).read_to_end(&mut buf)?;
    if n as u64 > cap {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} is larger than the {cap}-byte limit", path.display()),
        ));
    }
    Ok(buf)
}

/// Every read of an XMP sidecar, bounded. A sidecar is metadata a user
/// RECEIVES — from Lightroom, from a shared shoot, from a stranger's delivery —
/// so its size is not ours to trust: a plain `read_to_string` on a 2 GB file
/// named `DSC0001.xmp` materialises 2 GB in a request thread just to be handed
/// to a scanner. Real ones are kilobytes; the biggest Lightroom masks documents
/// are single-digit megabytes. Over the cap the content is refused — restore
/// callers treat that as "no develop", and the export path DISCLOSES it
/// (see [`crate::pipeline::xmp_export_bytes`]) instead of silently merging
/// against a different base.
///
/// `Read::take` rather than a `metadata()` size check: the length is bounded by
/// what was actually read, so a file that grows between the two syscalls cannot
/// widen the allocation.
/// A sidecar's identity as its open HANDLE reports it: (mtime, length). A
/// stage+rename publish re-points the PATH at a different file, but never the
/// handle — so this is the identity the content actually read is bound to,
/// and the only mtime a newest-intent ranking of that content may use.
pub type SidecarStamp = (std::time::SystemTime, u64);

fn handle_stamp(f: &std::fs::File) -> Option<SidecarStamp> {
    f.metadata().ok().and_then(|m| m.modified().ok().map(|t| (t, m.len())))
}

pub fn read_sidecar_checked(path: &Path) -> SidecarRead {
    read_sidecar_stamped(path).0
}

/// [`read_sidecar_checked`] plus the handle identity of what was read. The
/// stamp is fstat'd from the SAME handle before AND after the read: a writer
/// mutating the file in place mid-read used to hand back silently torn text —
/// that now reports as unreadable with the true reason, and the stamp is
/// returned only when both fstats agree. (A same-length in-place rewrite
/// inside one mtime tick of a coarse-granularity filesystem can still slip
/// through; the develop lock covers the app's own writers — this guard is
/// for foreign ones, best-effort by nature.)
pub fn read_sidecar_stamped(path: &Path) -> (SidecarRead, Option<SidecarStamp>) {
    use std::io::Read as _;
    const MAX_SIDECAR: u64 = 16 * 1024 * 1024;
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (SidecarRead::Missing, None),
        Err(_) => return (SidecarRead::Unreadable("it could not be opened"), None),
    };
    let before = handle_stamp(&f);
    // BYTES first, text second. `read_to_string` on a `Take` that cuts through
    // a multi-byte character fails with InvalidData BEFORE any size check can
    // run, so an over-cap sidecar carrying CJK captions or typographic quotes
    // was reported to the user as "not readable UTF-8 text" — a false reason,
    // in a note this round exists to make truthful.
    let mut buf = Vec::new();
    let read = match (&f).take(MAX_SIDECAR + 1).read_to_end(&mut buf) {
        Err(_) => SidecarRead::Unreadable("it could not be read"),
        Ok(n) if n as u64 > MAX_SIDECAR => {
            SidecarRead::Unreadable("it is larger than the 16 MiB sidecar limit")
        }
        Ok(_) => match String::from_utf8(buf) {
            Ok(s) => SidecarRead::Ok(s),
            Err(_) => SidecarRead::Unreadable("it is not readable UTF-8 text"),
        },
    };
    let after = handle_stamp(&f);
    match (before, after) {
        (Some(b), Some(a)) if b == a => (read, Some(a)),
        (Some(_), Some(_)) => {
            (SidecarRead::Unreadable("it changed while it was being read"), None)
        }
        // No handle identity available (exotic filesystem): the text is
        // still the text — callers just get no stamp to verify against.
        _ => (read, None),
    }
}

/// [`read_sidecar_checked`] for the callers to whom missing and unreadable
/// really are the same (restore: either way there is no develop to restore).
pub fn read_sidecar(path: &Path) -> Option<String> {
    match read_sidecar_checked(path) {
        SidecarRead::Ok(s) => Some(s),
        SidecarRead::Missing | SidecarRead::Unreadable(_) => None,
    }
}
