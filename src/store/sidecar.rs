//! Sidecars and legacy files: the capped readers, the stamped sidecar read, the Lightroom sidecar ranking, and the legacy out roots.

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

/// [`has_develop`] PLUS the sidecar Lightroom itself writes beside the RAW
/// (L13#2). The badge/resume predicate: a photo edited only in Lightroom
/// showed no ● badge yet clicking it restored the Lightroom develop, and
/// the CLI batch resume filter spent a PAID analyze on it and then wrote
/// recipe.json over the user's Lightroom work. Existence-only and
/// deliberately cheap (called per photo per refresh) — reading and RANKING
/// the file is [`lightroom_sidecar`]'s job. Conservative by design: a
/// foreign or neutral sidecar counts as "a develop may exist", because a
/// false SKIP costs one manual analyze while a false ANALYZE costs money
/// and supersedes the user's work. A SIBLING of `has_develop`, never a
/// replacement: [`rank_lightroom_sidecar`] calls `has_develop` to mean "is
/// there a STORE develop to rank against" — folding the sidecar in there
/// would make `LrSidecar::Only` unreachable whenever a sidecar exists.
pub fn has_develop_or_sidecar(src: &Path) -> bool {
    // The pending-clear marker outranks the sidecar probe exactly as it
    // outranks the store files (L03): while the develop is being removed, a
    // projection the user once copied beside the RAW must not resurrect
    // the badge.
    if clear_pending(src).exists() {
        return false;
    }
    has_develop(src)
        || (crate::decode::is_raw(src) && src.with_extension("xmp").exists())
}

/// What the Lightroom sidecar BESIDE the RAW (`<dir>/<stem>.xmp`) means for
/// this photo's restore. That file is the one place LIGHTROOM itself writes,
/// and until this existed no restore surface looked at it — edits made in
/// Lightroom were silently outranked by an older stored develop. Newest
/// intent wins:
///   * byte-identical to our own projection → it is our copied export, not a
///     Lightroom edit (`None`; the app itself tells users to copy the XMP
///     beside the RAW);
///   * no stored develop at all → the sidecar IS the develop (`Only`);
///   * modified after every stored develop file → Lightroom's edit is the
///     newest intent (`NewerThanStore`);
///   * otherwise the store is newer — the user's AutoShade work outranks the
///     older Lightroom pass (`OlderThanStore`).
///
/// Read-only: the store copy is never touched here — only an explicit save
/// adopts the Lightroom edit as the develop.
pub enum LrSidecar {
    None,
    Only(String),
    NewerThanStore(String),
    OlderThanStore,
    /// A sidecar file IS beside the RAW but cannot be read, and this is why
    /// (see [`SidecarRead::Unreadable`]). Folding this into `None` opened the
    /// photo neutral in silence while the user's Lightroom work might sit
    /// right there (L08) — callers disclose it and fall back to the store.
    Unreadable(&'static str),
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
/// callers treat that as "no develop", and the XMP-writing path DISCLOSES it
/// (see [`crate::pipeline::write_xmp`]) instead of silently merging
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

/// The RAW's embedded XMP packet AS A RESTORE SOURCE — strictly the
/// LOWEST-priority answer (a develop Lightroom baked INTO a DNG must not
/// outrank anything the user did since), and gated by the explicit-clear
/// markers: unlike every other source, the packet lives inside a file this
/// app never writes, so `clear_develop` cannot delete it — without the gate,
/// Reset+Save would resurrect the baked develop on the very next open (the
/// exact bug [`lightroom_sidecar`]'s cleared-marker contest was built to
/// close). Known, accepted consequence of "strictly lowest": once any store
/// file exists the packet is never consulted again — re-baking the DNG in
/// Lightroom does not win (that would need an mtime rank a file we never
/// write cannot express).
///
/// `Ok(None)` = no packet, or none a restore may consult. `Err(reason)` = a
/// packet that EXISTS but cannot be read — disclosed by every caller, never
/// folded into absence.
pub fn embedded_packet_for_restore(src: &Path) -> Result<Option<String>, String> {
    if !crate::decode::is_raw(src) {
        return Ok(None);
    }
    if develop_dir(src).join("cleared.txt").exists() || clear_pending(src).exists() {
        return Ok(None);
    }
    crate::decode::embedded_xmp(src).map_err(|e| e.to_string())
}

pub fn lightroom_sidecar(src: &Path) -> LrSidecar {
    // Only camera RAWs have a Lightroom-sidecar convention; a baked
    // PNG/TIFF's neighbouring .xmp (if any) is not ours to interpret.
    if !crate::decode::is_raw(src) {
        return LrSidecar::None;
    }
    let lr = src.with_extension("xmp");
    let (read, stamp) = read_sidecar_stamped(&lr);
    let text = match read {
        SidecarRead::Ok(t) => t,
        SidecarRead::Missing => return LrSidecar::None,
        // A file IS there but cannot answer — never fold that into "absent".
        SidecarRead::Unreadable(why) => return LrSidecar::Unreadable(why),
    };
    rank_lightroom_sidecar(src, &lr, text, stamp)
}

/// The ranking half of [`lightroom_sidecar`], split so a test can hand it a
/// deliberately mismatched identity. `stamp` is the HANDLE identity of the
/// text actually read; the mtime the newest-intent contest uses comes from it
/// — never from a fresh path stat, which after a swap describes a DIFFERENT
/// file than the text in hand (generation-N text ranked under
/// generation-N+1's mtime resurrected stale edits as "newer than the store",
/// and the mirror swap silently discarded fresh Lightroom work as older).
pub(super) fn rank_lightroom_sidecar(
    src: &Path,
    lr: &Path,
    text: String,
    stamp: Option<SidecarStamp>,
) -> LrSidecar {
    for ours in [xmp_target(src), legacy_xmp(src)] {
        if read_sidecar(&ours).is_some_and(|t| t == text) {
            return LrSidecar::None;
        }
    }
    // An explicit CLEAR (Reset + save, in the GUI or the web) is newest
    // intent too: its marker competes by mtime like any store file. Without
    // it, the projection the user once copied beside the RAW — no longer
    // byte-matchable above once the clear deleted our store copies — was
    // reborn as a "foreign Lightroom edit" and resurrected the cleared
    // develop on the very next open. The LIBRARY stays untouched (deleting
    // or rewriting the user's own sidecar would break the read-only
    // contract), and a Lightroom edit made AFTER the clear still wins.
    // A PENDING clear is newest intent too (L03): between the marker and
    // the completed sweep, a projection beside the RAW must not out-rank
    // the clear and resurrect what is being removed.
    let cleared_t = [develop_dir(src).join("cleared.txt"), clear_pending(src)]
        .iter()
        .filter_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .max();
    let lr_t = match stamp {
        Some(s) => {
            // ONE re-verification after the read: the path must still
            // resolve to the very file the handle read. A swap — or an
            // unlink — after the read means the text in hand no longer
            // describes what sits beside the photo: disclosed, never ranked.
            let now = std::fs::metadata(lr)
                .ok()
                .and_then(|m| m.modified().ok().map(|t| (t, m.len())));
            if now != Some(s) {
                return LrSidecar::Unreadable("it was replaced while it was being read");
            }
            Some(s.0)
        }
        // No handle identity was available: the old path stat is all there
        // is — strictly no worse than before this guard existed.
        None => std::fs::metadata(lr).and_then(|m| m.modified()).ok(),
    };
    if !has_develop(src) {
        return match (lr_t, cleared_t) {
            (Some(l), Some(cl)) if l <= cl => LrSidecar::None,
            _ => LrSidecar::Only(text),
        };
    }
    let store_t = [recipe_target(src), xmp_target(src), legacy_recipe(src), legacy_xmp(src)]
        .iter()
        .filter_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .max()
        .max(cleared_t);
    match (lr_t, store_t) {
        (Some(l), Some(s)) if l > s => LrSidecar::NewerThanStore(text),
        // has_develop said a store exists, yet none of its files answers a
        // stat — trust the sidecar, the one file that demonstrably can.
        (Some(_), None) => LrSidecar::NewerThanStore(text),
        _ => LrSidecar::OlderThanStore,
    }
}
