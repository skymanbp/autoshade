//! Durable writes: staging and syncing, the durable replace and rename, the OS primitives, and no-clobber publishing.

use super::*;

/// Copy `from` to `to` ATOMICALLY: stage beside the destination, then rename.
/// A bare `fs::copy` can leave a PARTIAL destination on failure — which the
/// migration/backup existence checks would then trust as a completed artifact
/// forever. Clobbers an existing `to` (callers gate on their own semantics).
pub(super) fn copy_atomic(from: &Path, to: &Path) -> std::io::Result<()> {
    let tmp = sibling_tmp(to);
    let result = std::fs::copy(from, &tmp)
        .and_then(|_| sync_staged(&tmp))
        .and_then(|_| durable_os::replace(&tmp, to))
        .and_then(|_| durable_os::finish_parent(to));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map(|_| ())
}

/// Process-wide sequence for temporary-file names. EVERY tmp minter that
/// emits into the shared `<name>.tmp.<pid>.<seq>` lexical namespace must
/// draw from this ONE counter: independent per-site counters (each starting
/// at 0) let two SAME-process writers — e.g. a version backup racing a
/// legacy-migration publish over the same v<N>.recipe.json — mint the SAME
/// tmp path and truncate each other's staged bytes before rename. (Probe
/// note: fs::rename DOES replace an existing file on Windows — verified
/// empirically — which is exactly why a corrupted tmp gets published.)
pub(crate) fn next_tmp_seq() -> u64 {
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Private staging name beside `to` (pid + the shared process-wide seq — a
/// FIXED name would let two processes truncate each other's staging file).
pub(super) fn sibling_tmp(to: &Path) -> PathBuf {
    let mut name = to.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".tmp.{}.{}", std::process::id(), next_tmp_seq()));
    to.with_file_name(name)
}

pub(super) fn write_staged(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Flush a staged file we did not write ourselves. The handle must carry
/// WRITE access: Windows backs `sync_all` with `FlushFileBuffers`, which
/// returns ERROR_ACCESS_DENIED on a read-only handle (probed) — unlike
/// `fsync`, which Unix accepts on a read-only fd.
pub(super) fn sync_staged(path: &Path) -> std::io::Result<()> {
    OpenOptions::new().write(true).open(path)?.sync_all()
}

/// Durability tail for a staged publish performed OUTSIDE this module (the
/// settings file, the style index): fsync the staged bytes, rename them over
/// the live name, fsync the parent directory. tmp+rename ALONE leaves a
/// post-crash window where the live name points at bytes the disk never
/// received (L03) — and modes are untouched, so a 0600-claimed staging
/// carries its mode to the live name.
pub(crate) fn durable_replace(staged: &Path, live: &Path) -> std::io::Result<()> {
    sync_staged(staged)?;
    durable_os::replace(staged, live)?;
    durable_os::finish_parent(live)
}

/// Durably MOVE an existing file to a new name. The bytes are already
/// whatever the disk holds — nothing was staged — so only the directory
/// entry needs to survive. NOT [`durable_replace`]: that fsyncs its source
/// through a WRITE handle, which a read-only or exclusively-held file
/// refuses — turning a move that used to succeed into a failure (L03).
pub(crate) fn durable_rename(from: &Path, to: &Path) -> std::io::Result<()> {
    durable_os::replace(from, to)?;
    durable_os::finish_parent(to)
}

/// Make an already-written payload durable where it stands: fsync its
/// bytes and its parent directory. For binary payloads (mask rasters,
/// sidecar masks) written straight onto their claimed name and about to be
/// REFERENCED by a durably-committed JSON — the JSON's own fsync is
/// meaningless if the payload it names can still vanish with the page
/// cache (L03). `pub`: the GUI binary's mask writers consume it.
pub fn durable_adopt(path: &Path) -> std::io::Result<()> {
    sync_staged(path)?;
    durable_os::finish_parent(path)
}

/// Publish complete bytes through the one durable write protocol.
pub(crate) fn durable_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    durable_write_tracked(path, bytes).1
}

/// [`durable_write`] plus the one fact a caller occasionally has to know: did
/// the bytes reach the LIVE name before the error?
///
/// `true` = the file at `path` IS the new, complete payload and only the
/// durability tail failed ([`durable_os::finish_parent`] — the sole step after
/// the rename). A caller that "rolls back" on that error destroys a delivered
/// file instead of cleaning up a half-write; the delivery beside the photo
/// ([`deliver_xmp_beside`]) is the one that used to (R22 L2).
pub(super) fn durable_write_tracked(path: &Path, bytes: &[u8]) -> (bool, std::io::Result<()>) {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return (false, Err(e));
    }
    let tmp = sibling_tmp(path);
    let mut published = false;
    let result = (|| {
        write_staged(&tmp, bytes)?;
        durable_os::replace(&tmp, path)?;
        published = true;
        durable_os::finish_parent(path)
    })();
    if result.is_err() {
        // A no-op once the rename has consumed the stage — it only survives a
        // failure BEFORE that.
        let _ = std::fs::remove_file(&tmp);
    }
    (published, result)
}

/// Retire the live file to `<name>.bak`, durably publish its replacement, and
/// restore the retired bytes if publication fails.
///
/// The retired copy SURVIVES a successful publish. It is not a rollback buffer
/// that has served its purpose — it is this store's crash-recovery contract:
/// [`recover_orphan_baks`] republishes `<name>.bak` whenever the live file is
/// missing, which is precisely the state a crash mid-publish leaves behind.
/// Removing it on success would reinstate the "a crash leaves nothing at all"
/// case the retire step exists to prevent. Only [`clear_develop`] and
/// [`clear_pixel_source`] delete one, and they must — otherwise a develop the
/// user explicitly cleared is resurrected on the next read. Each retire
/// supersedes the previous `.bak` rather than accumulating.
pub(crate) fn durable_retire_and_write(
    path: &Path,
    bak: &Path,
    bytes: &[u8],
) -> std::io::Result<()> {
    let mut retired = false;
    if path.exists() {
        if bak.exists() {
            std::fs::remove_file(bak)?;
            durable_os::finish_parent(bak)?;
        }
        durable_os::replace(path, bak)?;
        durable_os::finish_parent(bak)?;
        retired = true;
    } else if bak.exists() {
        // Live missing but a survivor present: adopt it as the retired copy
        // through a round trip so the publish below still has something to
        // restore, and so a crash here cannot leave BOTH names empty.
        durable_os::replace(bak, path)?;
        durable_os::finish_parent(path)?;
        durable_os::replace(path, bak)?;
        durable_os::finish_parent(bak)?;
        retired = true;
    }

    if let Err(e) = durable_write(path, bytes) {
        let mut note = String::new();
        if retired
            && (durable_os::replace(bak, path).is_err()
                || durable_os::finish_parent(path).is_err())
        {
            note = format!(
                " (restoring the previous file ALSO failed — it survives at {})",
                bak.display()
            );
        }
        return Err(std::io::Error::new(e.kind(), format!("{e}{note}")));
    }

    // The retired copy STAYS. It is not a rollback buffer that has served its
    // purpose — it is this store's crash-recovery contract: `recover_orphan_baks`
    // republishes `<name>.bak` whenever the live file is missing, which is
    // exactly the state a crash mid-publish leaves behind. Deleting it here
    // reinstated the "crash leaves nothing at all" case the retire step exists
    // to prevent, and `clear_develop` / `clear_pixel_source` are the only
    // things allowed to remove one (they must, or a cleared develop
    // resurrects). Each retire supersedes the previous `.bak` above.
    Ok(())
}

#[cfg(unix)]
pub(super) mod durable_os {
    use std::{fs::File, io, path::Path};

    pub(in crate::store) fn replace(from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    pub(in crate::store) fn finish_parent(path: &Path) -> io::Result<()> {
        let Some(parent) = path.parent() else { return Ok(()) };
        File::open(parent)?.sync_all()
    }
}

#[cfg(windows)]
pub(super) mod durable_os {
    use std::{
        io,
        os::windows::ffi::OsStrExt as _,
        path::Path,
    };

    const MOVEFILE_REPLACE_EXISTING: u32 = 1;
    const MOVEFILE_WRITE_THROUGH: u32 = 8;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        #[link_name = "MoveFileExW"]
        fn move_file_ex_w(from: *const u16, to: *const u16, flags: u32) -> i32;
    }

    fn wide(path: &Path) -> io::Result<Vec<u16>> {
        let mut out: Vec<u16> = path.as_os_str().encode_wide().collect();
        if out.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains an embedded NUL",
            ));
        }
        out.push(0);
        Ok(out)
    }

    pub(in crate::store) fn replace(from: &Path, to: &Path) -> io::Result<()> {
        let from = wide(from)?;
        let to = wide(to)?;
        // SAFETY: both buffers are NUL-terminated and remain live through the
        // synchronous call.
        let ok = unsafe {
            move_file_ex_w(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub(in crate::store) fn finish_parent(_: &Path) -> io::Result<()> {
        // Windows does not expose a useful `File::open(directory).sync_all()`
        // equivalent here. `replace` therefore uses MOVEFILE_WRITE_THROUGH,
        // which waits for the move operation to be flushed before returning.
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
pub(super) mod durable_os {
    use std::{io, path::Path};

    pub(in crate::store) fn replace(from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    pub(in crate::store) fn finish_parent(_: &Path) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "durable directory publication is unsupported on this platform",
        ))
    }
}

/// Publish `tmp` at `to` WITHOUT ever replacing another writer's file.
/// `fs::rename` REPLACES an existing destination on every platform (verified
/// empirically on Windows), so the previous create_new claim + rename kept a
/// claim→rename window in which a concurrent save landing on `to` was
/// silently overwritten. `fs::hard_link` is the one std primitive with true
/// no-replace semantics (fails `AlreadyExists`); `tmp` sits in `to`'s own
/// directory, so the link never crosses volumes. Filesystems without hard
/// links (exFAT) fall back to the claim + rename dance with that documented
/// microsecond residual. `tmp` is consumed on every path. Ok(true) =
/// published, Ok(false) = someone else owns `to`.
pub(super) fn publish_no_clobber(tmp: &Path, to: &Path) -> std::io::Result<bool> {
    #[cfg(unix)]
    let link = |from: &Path, dest: &Path| std::fs::hard_link(from, dest);
    #[cfg(not(unix))]
    let link = |_: &Path, _: &Path| {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "use the write-through claim fallback",
        ))
    };
    publish_no_clobber_with(tmp, to, &link)
}

pub(super) fn publish_no_clobber_with(
    tmp: &Path,
    to: &Path,
    link: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<bool> {
    if let Err(e) = sync_staged(tmp) {
        let _ = std::fs::remove_file(tmp);
        return Err(e);
    }
    match link(tmp, to) {
        Ok(()) => {
            let _ = std::fs::remove_file(tmp);
            durable_os::finish_parent(to)?;
            return Ok(true);
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(tmp);
            return Ok(false);
        }
        Err(_) => {}
    }

    match OpenOptions::new().write(true).create_new(true).open(to) {
        Ok(file) => drop(file),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(tmp);
            return Ok(false);
        }
        Err(e) => {
            let _ = std::fs::remove_file(tmp);
            return Err(e);
        }
    }

    match durable_os::replace(tmp, to).and_then(|_| durable_os::finish_parent(to)) {
        Ok(()) => Ok(true),
        Err(e) => {
            let _ = std::fs::remove_file(tmp);
            if std::fs::metadata(to).is_ok_and(|m| m.len() == 0) {
                let _ = std::fs::remove_file(to);
                let _ = durable_os::finish_parent(to);
            }
            Err(e)
        }
    }
}

/// Move that never CLOBBERS an existing destination: stage a full copy
/// beside `to` (copying also covers the routine cross-volume case — project
/// on D:, %LOCALAPPDATA% on C:), then [`publish_no_clobber`] it. `Ok(false)`
/// = someone else owns `to` — exactly the "already migrated" outcome. The
/// source is deliberately retained: its stem-only identity is ambiguous, so
/// this photo cannot prove that it owns the legacy bytes.
pub(super) fn move_file_no_clobber(from: &Path, to: &Path) -> std::io::Result<bool> {
    let tmp = sibling_tmp(to);
    if let Err(e) = std::fs::copy(from, &tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    publish_no_clobber(&tmp, to)
}
