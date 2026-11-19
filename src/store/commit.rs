//! The develop commit: the manifest, the staged members, and resolving a pending commit.

use super::*;

/// A single-generation write of the develop's three files. `recipe` bytes
/// come from [`crate::pipeline::recipe_store_bytes`] (clamped +
/// mask-relativized there), `pixels` from [`pixel_source_record_bytes`],
/// `variants` from [`variants_member`] — the commit publishes, it does not
/// interpret.
///
/// No commit carries an .xmp (2026-11-18: an .xmp is made only by an explicit
/// export). A commit that writes the recipe RETIRES the `<stem>.xmp`
/// ([`xmp_target`]) an earlier build kept beside it, in the same generation:
/// that file was derived from the recipe it stood beside, so under a new
/// recipe it can only be stale. A commit that writes no recipe leaves it.
pub struct DevelopCommit {
    pub recipe: Option<Vec<u8>>,
    pub pixels: CommitMember,
    pub variants: CommitMember,
}

fn commit_dir(src: &Path) -> PathBuf {
    commit_dir_in(&store_root(), src)
}

pub(super) fn commit_dir_in(root: &Path, src: &Path) -> PathBuf {
    develop_dir_in(root, src).join(".commit")
}

/// The staged-generation manifest (`.commit/COMMIT`), whose single durable
/// rename is the transaction's linearization point. Sums are hex FNV-1a of
/// each staged member's bytes — strings, because a u64 does not survive a
/// JSON round trip intact.
#[derive(serde::Serialize, serde::Deserialize)]
struct CommitManifest {
    v: u32,
    recipe: String,
    pixels: String,
    variants: String,
    /// What happens to the store's `<stem>.xmp`: `clear` when this generation
    /// writes the recipe, else `keep`. `write` is only ever READ BACK — from
    /// a stage an earlier build crashed in, which replays as it was staged.
    /// ADDITIVE at v=1: a marker written before the word existed reads as
    /// `keep`, and an older build resolving a newer stage ignores the field
    /// — the file then stays as it was, which the readers tolerate (a
    /// present recipe.json is never out-answered by it).
    #[serde(default = "manifest_keep")]
    xmp: String,
    sums: std::collections::BTreeMap<String, String>,
}

fn manifest_keep() -> String {
    "keep".to_string()
}

/// Publish recipe.json / pixels.json / variants.json as ONE generation (L03).
///
/// Ctrl+S used to run three sequential durable writes under the develop
/// lock; the lock excludes concurrent writers, but every kill point between
/// the renames left a torn generation — a new recipe over an old (or
/// half-cleared) master link, or a new pair under a stale strip — and the
/// per-file `.bak` recovery is generation-blind, so it would republish the
/// OLD pixels link under the NEW recipe. No platform primitive swaps
/// multiple directory entries at once (renameat2 is Linux-only; MoveFileExW
/// cannot replace a directory), so the gap is closed by a MARKER: members
/// stage into `develop_dir/.commit/`, ONE durable rename of `.commit/COMMIT`
/// is the commit point, and recovery rolls FORWARD past it (an unmarked
/// stage is discarded). A crash after COMMIT therefore COMPLETES this save
/// on the photo's next locked touch instead of reverting it.
pub fn commit_develop(src: &Path, commit: DevelopCommit) -> std::io::Result<()> {
    with_develop_lock(src, DevelopLockMode::Wait, || commit_develop_unlocked(src, commit))
}

fn commit_develop_unlocked(src: &Path, commit: DevelopCommit) -> std::io::Result<()> {
    // The batch-S trap, same shape: a PENDING explicit clear must complete
    // before a new generation stages — its recovery would otherwise eat the
    // very save it predates. A pending commit resolves too: gen(n+1) cannot
    // stage over an unresolved gen(n).
    resolve_pending_clear_unlocked(src)?;
    resolve_pending_commit_unlocked(src)?;
    // The save-path floor, at the moment of publication: an unresolved strip
    // refuses BOTH the write and the clear, exactly as the member writers do.
    if !matches!(commit.variants, CommitMember::Keep) {
        refuse_unresolved_strip(src)?;
    }
    if commit.recipe.is_none()
        && matches!(commit.pixels, CommitMember::Keep)
        && matches!(commit.variants, CommitMember::Keep)
    {
        return Ok(());
    }
    // The retire rule: an earlier build's `<stem>.xmp` goes whenever the
    // recipe it was derived from is replaced, and stays put otherwise.
    let xmp = if commit.recipe.is_some() { MemberRef::Clear } else { MemberRef::Keep };
    let dev = develop_dir(src);
    std::fs::create_dir_all(&dev)?;
    let cdir = commit_dir(src);
    let staged = (|| -> std::io::Result<()> {
        // create_dir, not create_dir_all: a leftover `.commit` was resolved
        // above, so an existing directory here is a real error, not residue.
        std::fs::create_dir(&cdir)?;
        // The stage dir's OWN record in the develop dir must be down before
        // COMMIT can promise a generation: without this (unix), a crash
        // after the marker rename could evaporate the whole `.commit` entry
        // and leave the promised generation with no roll-forward. The
        // finish_parent below syncs the entries INSIDE cdir, not this one.
        durable_os::finish_parent(&cdir)?;
        let mut sums = std::collections::BTreeMap::new();
        let mut stage = |name: &str, bytes: &[u8]| -> std::io::Result<()> {
            // The recovery reader caps members at MAX_STORE_JSON — a member
            // staged above it would commit fine and then BRICK the photo on
            // the first crash recovery (refused as unreadable). Refuse the
            // save instead, citing the same constant the reader enforces.
            if bytes.len() as u64 > MAX_STORE_JSON {
                return Err(std::io::Error::other(format!(
                    "{name} is {} bytes, above the {MAX_STORE_JSON}-byte store limit a crash \
                     recovery could read back — the save is refused whole",
                    bytes.len()
                )));
            }
            write_staged(&cdir.join(name), bytes)?;
            sums.insert(name.to_string(), format!("{:016x}", fnv1a64(bytes)));
            Ok(())
        };
        if let Some(b) = &commit.recipe {
            stage("recipe.json", b)?;
        }
        if let CommitMember::Write(b) = &commit.pixels {
            stage("pixels.json", b)?;
        }
        if let CommitMember::Write(b) = &commit.variants {
            stage("variants.json", b)?;
        }
        // The staged entries' directory records go down BEFORE the marker
        // can exist (unix: dir fsync; Windows: finish_parent is a no-op and
        // durability rests on the write-through rename below plus journaled
        // metadata, with the sums as the belt).
        durable_os::finish_parent(&cdir.join("COMMIT"))?;
        let manifest = CommitManifest {
            v: 1,
            recipe: if commit.recipe.is_some() { "write" } else { "keep" }.to_string(),
            pixels: commit.pixels.word().to_string(),
            variants: commit.variants.word().to_string(),
            xmp: if commit.recipe.is_some() { "clear" } else { "keep" }.to_string(),
            sums,
        };
        // THE commit point: one durable rename. Before it, no live file has
        // been touched; after it, the generation is promised.
        durable_write(
            &cdir.join("COMMIT"),
            &serde_json::to_vec_pretty(&manifest).map_err(std::io::Error::other)?,
        )
    })();
    if let Err(e) = staged {
        // Unmarked stage: nothing was promised and nothing was applied — the
        // save simply failed, whole.
        let _ = std::fs::remove_dir_all(&cdir);
        return Err(e);
    }
    if let Err(e) = apply_commit_members(
        src,
        commit.recipe.as_deref().map_or(MemberRef::Keep, MemberRef::Write),
        (&commit.pixels).into(),
        (&commit.variants).into(),
        xmp,
    ) {
        // BEYOND the commit point: the generation is promised and `.commit`
        // stays — recovery completes the apply on the photo's next locked
        // touch, so the caller must not read this Err as "nothing happened".
        return Err(std::io::Error::new(
            e.kind(),
            format!("{e} — the save IS committed and completes on this photo's next open or save"),
        ));
    }
    // Consumed. A failure here only means the (idempotent) replay runs once
    // more on the next touch and consumes it then.
    let _ = consume_commit_stage(&cdir);
    settle_consumed_marker(&cdir);
    Ok(())
}

/// Consume a RESOLVED `.commit` stage: the COMMIT marker dies FIRST — by
/// contract, not by remove_dir_all's unspecified order — so a partial
/// cleanup (an AV scanner holding a member open) leaves an UNMARKED stage
/// the next recovery quietly discards. COMMIT surviving beside missing
/// members would instead be refused by the recovery reader, bricking every
/// subsequent touch of the photo until someone deletes it by hand.
fn consume_commit_stage(cdir: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(cdir.join("COMMIT")) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::fs::remove_dir_all(cdir)
}

/// A borrowed [`CommitMember`], shared by the live apply and the recovery
/// replay (which owns its bytes read back from the stage).
#[derive(Clone, Copy)]
enum MemberRef<'a> {
    Write(&'a [u8]),
    Clear,
    Keep,
}

impl<'a> From<&'a CommitMember> for MemberRef<'a> {
    fn from(m: &'a CommitMember) -> Self {
        match m {
            CommitMember::Write(b) => MemberRef::Write(b),
            CommitMember::Clear => MemberRef::Clear,
            CommitMember::Keep => MemberRef::Keep,
        }
    }
}

/// Apply one committed generation onto the live triple. IDEMPOTENT —
/// replay-safe: a Write member whose live bytes already equal the staged
/// bytes is skipped, so a resumed apply neither re-retires the new
/// generation onto its own `.bak` (destroying the previous generation's
/// crash-recovery copy) nor repeats completed work.
fn apply_commit_members(
    src: &Path,
    recipe: MemberRef<'_>,
    pixels: MemberRef<'_>,
    variants: MemberRef<'_>,
    xmp: MemberRef<'_>,
) -> std::io::Result<()> {
    let dev = develop_dir(src);
    let apply = |live: PathBuf, bak: PathBuf, member: MemberRef<'_>| -> std::io::Result<()> {
        let rm = |p: PathBuf| match std::fs::remove_file(p) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        };
        match member {
            MemberRef::Write(bytes) => {
                if read_bytes_capped(&live, MAX_STORE_JSON).is_ok_and(|cur| cur == bytes) {
                    return Ok(()); // already applied — keep the retired previous generation
                }
                durable_retire_and_write(&live, &bak, bytes)
            }
            // The clear pair, `.bak` first (the clear_pixel_source rule: a
            // crash between the removals leaves the live record, never the
            // resurrection bait). RAW removal, not the public clear helpers
            // — a replay must not re-run publication gates against the
            // half-cleared state it exists to finish.
            MemberRef::Clear => {
                rm(bak)?;
                rm(live)
            }
            MemberRef::Keep => Ok(()),
        }
    };
    apply(recipe_target(src), dev.join("recipe.json.bak"), recipe)?;
    if matches!(recipe, MemberRef::Write(_)) {
        note_source(src); // the write_recipe breadcrumb rides the same member
    }
    apply(pixel_source_path(src), dev.join("pixels.json.bak"), pixels)?;
    apply(variants_path(src), dev.join("variants.json.bak"), variants)?;
    // The store's `<stem>.xmp` LAST, after the recipe: a plain unlink
    // retires the one an earlier build left there. The Write arm is reached
    // only by the replay of a stage an EARLIER build crashed in (this build
    // stages none): it lands as it was staged, is never read while
    // recipe.json exists, and the next commit retires it.
    let live = xmp_target(src);
    match xmp {
        MemberRef::Write(bytes) => {
            if read_bytes_capped(&live, MAX_STORE_JSON).is_ok_and(|cur| cur == bytes) {
                return Ok(());
            }
            durable_write(&live, bytes)
        }
        MemberRef::Clear => match std::fs::remove_file(&live) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        },
        MemberRef::Keep => Ok(()),
    }
}

/// Resolve a `.commit` stage left by a killed [`commit_develop`]. COMMIT
/// present with intact sums ⇒ replay the (idempotent) apply and consume the
/// stage; COMMIT absent ⇒ nothing was promised — discard the stage. COMMIT
/// present but unreadable, unknown, or sum-mismatched is `Err` and the
/// evidence STAYS: the marker was durably renamed in only after every staged
/// member was fsynced, so a mismatch means a spec-broken disk or tampering —
/// rolling back could freeze a half-applied generation as final, and rolling
/// forward would publish bytes nobody wrote. The error names the remedy.
pub(super) fn resolve_pending_commit_unlocked(src: &Path) -> std::io::Result<()> {
    let cdir = commit_dir(src);
    if !cdir.exists() {
        return Ok(());
    }
    let refuse = |why: String| {
        std::io::Error::other(format!(
            "{} holds a crashed develop save that cannot be resolved ({why}) — delete that \
             directory to discard the crashed save and keep the photo's current develop",
            cdir.display()
        ))
    };
    let marker = cdir.join("COMMIT");
    let manifest_bytes = match read_bytes_capped(&marker, MAX_STORE_JSON) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Unmarked stage: the commit point was never reached, nothing
            // was applied — discarding is a true rollback.
            std::fs::remove_dir_all(&cdir)?;
            return Ok(());
        }
        Err(e) => return Err(refuse(format!("unreadable COMMIT marker: {e}"))),
    };
    let m: CommitManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| refuse(format!("corrupt COMMIT marker: {e}")))?;
    if m.v != 1 {
        return Err(refuse(format!("future commit format v{}", m.v)));
    }
    // The recipe member never clears through a commit — only `clear_develop`
    // removes recipe.json, through its own marker.
    if m.recipe == "clear" {
        return Err(refuse("a commit cannot clear the recipe".into()));
    }
    let member = |name: &str, action: &str| -> std::io::Result<Option<Vec<u8>>> {
        match action {
            "keep" | "clear" => Ok(None),
            "write" => {
                let staged = read_bytes_capped(&cdir.join(name), MAX_STORE_JSON)
                    .map_err(|e| refuse(format!("staged {name} unreadable: {e}")))?;
                match m.sums.get(name) {
                    Some(want) if *want == format!("{:016x}", fnv1a64(&staged)) => Ok(Some(staged)),
                    Some(_) => Err(refuse(format!("staged {name} does not match its recorded sum"))),
                    None => Err(refuse(format!("staged {name} has no recorded sum"))),
                }
            }
            other => Err(refuse(format!("unknown {name} action {other:?}"))),
        }
    };
    let recipe = member("recipe.json", &m.recipe)?;
    let pixels = member("pixels.json", &m.pixels)?;
    let variants = member("variants.json", &m.variants)?;
    let xmp = member("projection.xmp", &m.xmp)?;
    let recipe_ref = recipe.as_deref().map_or(MemberRef::Keep, MemberRef::Write);
    let pixels_ref = match (m.pixels.as_str(), &pixels) {
        ("write", Some(b)) => MemberRef::Write(b),
        ("clear", _) => MemberRef::Clear,
        _ => MemberRef::Keep,
    };
    let variants_ref = match (m.variants.as_str(), &variants) {
        ("write", Some(b)) => MemberRef::Write(b),
        ("clear", _) => MemberRef::Clear,
        _ => MemberRef::Keep,
    };
    let xmp_ref = match (m.xmp.as_str(), &xmp) {
        ("write", Some(b)) => MemberRef::Write(b),
        ("clear", _) => MemberRef::Clear,
        _ => MemberRef::Keep,
    };
    apply_commit_members(src, recipe_ref, pixels_ref, variants_ref, xmp_ref)?;
    consume_commit_stage(&cdir)?;
    settle_consumed_marker(&cdir);
    Ok(())
}
