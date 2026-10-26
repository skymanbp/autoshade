//! Variants: the record and its entries, reading and writing it, the commit member and the active write.

use super::*;

/// One BACKGROUND variant in a [`VariantsRecord`]: its kind
/// ("original" | "generated" | "fitted" | "edited" | "denoised"), the variant's full develop recipe,
/// and its baked raster origin when the variant is pixel-based. Base pixels
/// are NOT stored — they re-decode from `origin`; source-based variants
/// re-develop the shared source.
///
/// `id` / `name` (R24-2) are ADDITIVE at v=1: this struct is deliberately
/// NOT `deny_unknown_fields` (unlike [`EditRecipe`]), so a build that
/// predates them reads a record carrying them and ignores both — the strip
/// comes back complete, minus the names. `skip_serializing_if` keeps a strip
/// with neither byte-identical to what earlier builds wrote.
///
/// `extra` makes that promise hold in the WRITE direction too (R24 round-end
/// MED-3). Ignoring an unknown field on the way in is only half of additive:
/// [`variants_member`]'s `ActiveWrite::Kind` arm is a READ-MODIFY-WRITE, so
/// without a capture-all an older build would read a newer strip, drop every
/// field it did not know, and publish the truncated record back over the
/// user's file. Serde's flatten catches them instead and they round-trip
/// verbatim. An empty map serialises to NOTHING, so a strip that uses no
/// future field is still byte-identical to what earlier builds wrote (pinned
/// by `variant_ids_and_names_are_additive_in_both_directions`).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct VariantEntry {
    pub kind: String,
    pub recipe: EditRecipe,
    #[serde(default)]
    pub origin: Option<PathBuf>,
    /// Opaque stable identity for this card (see the GUI's `Variant::id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The user's own name for this rendition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Every member a FUTURE build adds, carried verbatim (see above).
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// The strip minus the active variant (whose develop recipe.json owns),
/// mirroring the GUI's navigation-stash shape: `others` + where the active
/// card sits + the active card's kind (the ONE fact about the active variant
/// recipe.json cannot express).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct VariantsRecord {
    /// Format version; readers refuse a future major they cannot honour.
    pub v: u32,
    pub active_kind: String,
    pub active_pos: usize,
    pub others: Vec<VariantEntry>,
    /// The active card's identity + name — the same two facts every entry in
    /// `others` carries, for the ONE card that is not in `others` (R24-2).
    /// Additive at v=1 on the same terms as [`VariantEntry`]'s pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_name: Option<String>,
    /// Record-level capture-all — [`VariantEntry::extra`]'s reason, at the
    /// level `ActiveWrite::Kind` actually rewrites.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// The `variants.json` kind vocabulary — a FORMAT every surface shares (the
/// GUI's `VariantKind::store_str` spells the same words). `"edited"`
/// (2026-09-13) is a develop of the user's own over a generated card's
/// pixels: a generated card is immutable, so any non-neutral develop on AI
/// pixels is an edited card — [`ActiveWrite::DevelopOnAiPixels`] is how a
/// writer without a live strip states that.
fn known_variant_kind(kind: &str) -> bool {
    matches!(kind, "original" | "generated" | "fitted" | "edited" | "denoised")
}

/// Persist the strip record. Origins inside the develop dir are stored by
/// bare name and each entry's Bitmap mask references are relativized — the
/// same relocatability rules recipe.json and pixels.json follow.
pub fn write_variants(src: &Path, rec: &VariantsRecord) -> std::io::Result<()> {
    with_develop_lock(src, DevelopLockMode::Wait, || write_variants_unlocked(src, rec))
}

fn write_variants_unlocked(src: &Path, rec: &VariantsRecord) -> std::io::Result<()> {
    refuse_unresolved_strip(src)?;
    publish_json_sidecar(src, "variants.json", variants_record_bytes(src, rec)?)
}

/// The exact variants.json bytes [`write_variants`] publishes for `rec` —
/// exposed so [`commit_develop`] callers can stage the identical record into
/// a single-generation save. The [`refuse_unresolved_strip`] gate does NOT
/// run here: it belongs to the moment of publication, under the lock.
pub fn variants_record_bytes(src: &Path, rec: &VariantsRecord) -> std::io::Result<Vec<u8>> {
    let dir = develop_dir(src);
    let mut stored = VariantsRecord {
        v: rec.v,
        active_kind: rec.active_kind.clone(),
        active_pos: rec.active_pos,
        others: Vec::with_capacity(rec.others.len()),
        active_id: rec.active_id.clone(),
        active_name: rec.active_name.clone().map(|n| capped_name(&n)),
        // Verbatim, both levels: this rebuild is what a read-modify-write
        // passes through, and dropping the capture-all here would defeat it.
        extra: rec.extra.clone(),
    };
    for e in &rec.others {
        let origin = match &e.origin {
            Some(o) if o.parent() == Some(dir.as_path()) => {
                Some(o.file_name().map(PathBuf::from).unwrap_or_else(|| o.clone()))
            }
            Some(o) => Some(std::path::absolute(o)?),
            None => None,
        };
        let mut recipe = e.recipe.clone();
        relativize_mask_paths(&mut recipe, &dir);
        stored.others.push(VariantEntry {
            kind: e.kind.clone(),
            recipe,
            origin,
            id: e.id.clone(),
            name: e.name.clone().map(|n| capped_name(&n)),
            extra: e.extra.clone(),
        });
    }
    serde_json::to_vec_pretty(&stored).map_err(std::io::Error::other)
}

/// The byte cap every user-typed name in the store obeys — the same 256 the
/// recipe's own clamp applies to mask names (`recipe.rs`). Truncated on a
/// CHAR boundary: a cut mid-codepoint would publish invalid UTF-8 through a
/// serializer that cannot represent it. Applied on the way in AND on the way
/// out, so neither a hand-edited sidecar nor a future caller can hand the UI
/// a megabyte-long label.
pub const MAX_STORE_NAME: usize = 256;

pub(super) fn capped_name(s: &str) -> String {
    if s.len() <= MAX_STORE_NAME {
        return s.to_string();
    }
    let mut end = MAX_STORE_NAME;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// What a strip read actually found — the three states are NOT collapsible:
/// an `Unresolved` file records background variants this build cannot parse,
/// and the save primitives refuse to overwrite or clear it (an ordinary
/// Ctrl+S over a single card used to DELETE the unreadable record plus its
/// `.bak`, silently destroying every background variant it held — 16-lane
/// scan L08).
pub enum VariantsRead {
    /// No `variants.json` — the normal single-card case.
    Absent,
    Strip(VariantsRecord),
    /// The file EXISTS but cannot be honoured (unreadable bytes/JSON,
    /// future format, unknown kind, escaping or network origin).
    Unresolved,
}

/// The photo's persisted strip record, if one exists and parses. Origins and
/// Bitmap mask references come back resolved against the develop dir; their
/// EXISTENCE is deliberately not checked here — the GUI restore is the one
/// place that can degrade per-variant honestly (toast + neutral develop)
/// instead of silently dropping a variant's recipe with its raster.
pub fn read_variants(src: &Path) -> Option<VariantsRecord> {
    match read_variants_checked(src) {
        VariantsRead::Strip(rec) => Some(rec),
        _ => None,
    }
}

/// Are the photo's recorded render-source pixels ALREADY AI-denoised — the
/// desktop app's ◈ denoised card, saved as the active card? Read off the two
/// records that card's save already writes: a recorded pixel source (its
/// master) and a strip whose active card is `"denoised"`. A surface that
/// renders the recorded source runs no second denoise over it (the GUI's own
/// export sits it out); the browser and `auto --denoise` ran SCUNet over
/// SCUNet's output until 2026-10-26.
pub fn active_card_is_denoised(src: &Path) -> bool {
    recorded_pixel_source(src).is_some()
        && matches!(read_variants_checked(src), VariantsRead::Strip(rec) if rec.active_kind == "denoised")
}

/// [`read_variants`] with the absent/unresolved distinction preserved. A
/// missing file is silent (the normal single-card case); an existing file
/// that cannot be honoured warns on stderr and comes back `Unresolved`,
/// exactly like [`read_pixel_source`] degrades — except that save paths
/// treat `Unresolved` as a refusal, never as "nothing to keep".
pub fn read_variants_checked(src: &Path) -> VariantsRead {
    let _ = recover_orphan_baks(src);
    let sidecar = variants_path(src);
    let bytes = match read_bytes_capped(&sidecar, MAX_STORE_JSON) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return VariantsRead::Absent,
        Err(e) => {
            eprintln!(
                "⚠ {} exists but cannot be read ({e}) — the variant strip is not restored",
                sidecar.display()
            );
            return VariantsRead::Unresolved;
        }
    };
    let mut rec = match serde_json::from_slice::<VariantsRecord>(&bytes) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "⚠ {} is unreadable ({e}) — the variant strip is not restored",
                sidecar.display()
            );
            return VariantsRead::Unresolved;
        }
    };
    if rec.v != 1 {
        eprintln!(
            "⚠ {} has format v{} (this build reads v1) — the variant strip is not restored",
            sidecar.display(),
            rec.v
        );
        return VariantsRead::Unresolved;
    }
    if !known_variant_kind(&rec.active_kind)
        || rec.others.iter().any(|entry| !known_variant_kind(&entry.kind))
    {
        eprintln!(
            "⚠ {} contains a variant kind this build does not understand — the variant strip is not restored and the file is left untouched",
            sidecar.display()
        );
        return VariantsRead::Unresolved;
    }
    let dir = develop_dir(src);
    // A name is display text with no downstream meaning, so a hand-edited or
    // corrupt one is CAPPED, never a reason to refuse the whole strip (that
    // would cost the user every background variant over a label).
    rec.active_name = rec.active_name.as_deref().map(capped_name);
    for e in &mut rec.others {
        e.name = e.name.as_deref().map(capped_name);
        if let Some(o) = &e.origin {
            // LEXICAL network/device refusal BEFORE any probe, like the
            // pixel-source origin: a crafted UNC origin must not be touched.
            if remote_or_device_path(o) {
                eprintln!(
                    "⚠ {} contains a network/device variant origin — the variant strip is not restored",
                    sidecar.display()
                );
                return VariantsRead::Unresolved;
            }
            if o.is_relative() {
                let Some(origin) = contained_join(&dir, o) else {
                    eprintln!(
                        "⚠ {} contains a variant origin outside its develop directory — the variant strip is not restored",
                        sidecar.display()
                    );
                    return VariantsRead::Unresolved;
                };
                e.origin = Some(origin);
            }
        }
        resolve_mask_paths(&mut e.recipe, &dir);
    }
    VariantsRead::Strip(rec)
}

/// The save-path floor shared by [`write_variants`] and [`clear_variants`]:
/// an existing strip record this build cannot honour is someone's data, not
/// noise — refusing here protects every caller at once (Ctrl+S, save-all).
/// The EXPLICIT user clear (`clear_develop`) removes the file directly and
/// deliberately does not pass through this gate.
pub(super) fn refuse_unresolved_strip(src: &Path) -> std::io::Result<()> {
    match read_variants_checked(src) {
        VariantsRead::Unresolved => Err(std::io::Error::other(format!(
            "{} exists but cannot be honoured — the background variants it records would be \
             destroyed; fix or delete that file, then save again",
            variants_path(src).display()
        ))),
        _ => Ok(()),
    }
}

/// Forget the persisted strip (the photo went back to a single card). Same
/// two-step as [`clear_pixel_source`], `.bak` first, so a crash between the
/// removals leaves the live record intact instead of resurrection bait.
pub fn clear_variants(src: &Path) -> std::io::Result<()> {
    with_develop_lock(src, DevelopLockMode::Wait, || clear_variants_unlocked(src))
}

fn clear_variants_unlocked(src: &Path) -> std::io::Result<()> {
    refuse_unresolved_strip(src)?;
    let rm = |p: PathBuf| match std::fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    };
    rm(develop_dir(src).join("variants.json.bak"))?;
    rm(variants_path(src))
}

/// One member of a [`DevelopCommit`]: publish these bytes, remove the
/// sidecar (live + retired `.bak`, the detach rule), or leave it exactly as
/// it stands.
pub enum CommitMember {
    Write(Vec<u8>),
    Clear,
    Keep,
}

impl CommitMember {
    pub(super) fn word(&self) -> &'static str {
        match self {
            CommitMember::Write(_) => "write",
            CommitMember::Clear => "clear",
            CommitMember::Keep => "keep",
        }
    }
}

/// What a writer publishes into the ACTIVE slot of a photo's edit strip
/// (R24-4).
///
/// `variants.json`'s active half (`active_kind` / `active_pos` /
/// `active_id` / `active_name`) describes the card whose develop
/// `recipe.json` mirrors. Five production writers used to publish a NEW
/// active develop under `variants: CommitMember::Keep`, leaving that half
/// describing the develop they had just replaced — a CLI `match` over a
/// photo whose strip recorded 「original」 reopened as 「▣ 原片」 holding a
/// reverse-fit. This enum is how each writer states what it actually knows,
/// so ONE primitive ([`variants_member`]) owns the answer instead of five
/// hand-rolled members drifting apart.
pub enum ActiveWrite<'a> {
    /// The caller owns the whole strip and hands it over verbatim — the GUI,
    /// whose in-memory card list IS the truth. `None` means the strip went
    /// trivial (a single Original card, which needs no sidecar): the record
    /// is CLEARED, exactly as Ctrl+S has always done.
    Strip(Option<&'a VariantsRecord>),
    /// The caller knows only the KIND of develop it just published into the
    /// active card ([`VariantEntry::kind`] spellings — a FORMAT value).
    /// Never `"original"`: a photo has exactly ONE base negative, and a
    /// foreign writer claiming that slot would leave a strip carrying two
    /// Original cards, neither of which the strip UI will delete.
    Kind(&'a str),
    /// The caller replaced the develop INSIDE the active card without
    /// learning anything about the card itself (the web save and the batch
    /// paste, over source pixels): the record stands as written.
    Unknown,
    /// The caller publishes `recipe` as the active develop over an
    /// AI-GENERATED master (`master`) and holds no live strip — the web
    /// save, the batch paste. The GUI's immutability rule (2026-09-13,
    /// `VariantKind::Edited`): a generated card is the raster itself and
    /// stays neutral, so a non-neutral develop over it is an EDITED card of
    /// its own with the pristine card kept beside it. The record's word
    /// follows the develop: a neutral one is the pristine generated card
    /// (or an edited card back at neutral, when the master is its own);
    /// anything else takes the active slot as `"edited"` — the generated
    /// card it displaces moves into `others` with its identity and name,
    /// and a master that had no pristine card yet (this session's reimagine
    /// under the web's own edits) gets one. Before this, a paste onto a
    /// generated card relabelled the pristine image as the edit and the GUI
    /// had to unpick it at the door.
    DevelopOnAiPixels { recipe: &'a EditRecipe, master: &'a Path },
}

/// Lexical master identity — `std::path::absolute` on both sides, never the
/// filesystem: the record stores a bare name its reader re-anchors, the
/// caller hands the spelling it was given, and the master may not exist.
fn same_master_path(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (std::path::absolute(a), std::path::absolute(b)),
            (Ok(x), Ok(y)) if x == y
        )
}

/// The `variants` member for a [`DevelopCommit`] — the ONE place that
/// decides what a develop write does to `variants.json` (R24-4).
///
/// The read happens under the CALLER's develop lock (reentrant) and the
/// patched record stages into the SAME generation as the recipe it belongs
/// to, so the two can never disagree across a kill: before the commit marker
/// the previous pair stands whole, after it recovery rolls both forward
/// together.
pub fn variants_member(src: &Path, w: ActiveWrite<'_>) -> std::io::Result<CommitMember> {
    match w {
        ActiveWrite::Strip(Some(rec)) => {
            Ok(CommitMember::Write(variants_record_bytes(src, rec)?))
        }
        ActiveWrite::Strip(None) => Ok(CommitMember::Clear),
        ActiveWrite::Unknown => Ok(CommitMember::Keep),
        ActiveWrite::Kind(kind) => {
            debug_assert!(
                known_variant_kind(kind) && kind != "original",
                "a foreign writer may not claim the base negative's slot ({kind})"
            );
            match read_variants_checked(src) {
                // Nothing to keep truthful: the trivial one-card strip
                // writes no record at all, and minting one HERE would
                // publish a strip nobody has.
                VariantsRead::Absent => Ok(CommitMember::Keep),
                // A record this build cannot honour is someone's data —
                // and a non-Keep member would make `refuse_unresolved_strip`
                // fail the WHOLE save (`commit_develop`), so a sidecar the
                // CLI never even reads must not block the CLI's write.
                VariantsRead::Unresolved => Ok(CommitMember::Keep),
                // Already true — and a no-op member keeps the bytes on disk
                // byte-identical (a rewrite would churn the `.bak` pair for
                // nothing).
                VariantsRead::Strip(rec) if rec.active_kind == kind => Ok(CommitMember::Keep),
                VariantsRead::Strip(mut rec) => {
                    // The card's IDENTITY survives the restatement: the
                    // write replaced the develop INSIDE the card, not the
                    // card — its `active_id` is what version snapshots taken
                    // from it point at (R24-2), and `active_name` is the
                    // user's own label for that slot.
                    rec.active_kind = kind.to_string();
                    Ok(CommitMember::Write(variants_record_bytes(src, &rec)?))
                }
            }
        }
        ActiveWrite::DevelopOnAiPixels { recipe, master } => {
            let mut rec = match read_variants_checked(src) {
                // A trivial strip has no record to keep truthful (the GUI
                // splits a generated card found carrying edits at the door),
                // and an unresolved one is someone's data (the `Kind` rule).
                VariantsRead::Absent | VariantsRead::Unresolved => return Ok(CommitMember::Keep),
                VariantsRead::Strip(rec) => rec,
            };
            // The master the ACTIVE card sat on before this write — the
            // record-level answer, resolvable or not (a develop over a
            // reimagine rendition describes those pixels whether or not
            // the PNG opens right now), read under the caller's lock.
            let recorded = recorded_pixel_source(src).and_then(|(o, g)| g.then_some(o));
            let same_master = recorded.as_deref().is_some_and(|o| same_master_path(o, master));
            if recipe.is_noop() {
                // A neutral develop over AI pixels IS the pristine generated
                // card — or an edited card back at neutral (Ctrl+Z), which
                // keeps its word while the master is its own.
                return match rec.active_kind.as_str() {
                    "generated" => Ok(CommitMember::Keep),
                    "edited" if same_master => Ok(CommitMember::Keep),
                    _ => {
                        rec.active_kind = "generated".to_string();
                        Ok(CommitMember::Write(variants_record_bytes(src, &rec)?))
                    }
                };
            }
            if rec.active_kind == "edited" && same_master {
                // The develop inside the edited card is replaced; the card
                // stands, identity and name included.
                return Ok(CommitMember::Keep);
            }
            let mut pos = rec.active_pos.min(rec.others.len());
            let outgoing_pristine = rec.active_kind == "generated";
            if outgoing_pristine && let Some(o) = &recorded {
                // The pristine card whose slot this develop takes survives
                // as a background card — its identity, name and raster.
                rec.others.insert(
                    pos,
                    VariantEntry {
                        kind: "generated".to_string(),
                        recipe: EditRecipe::default(),
                        origin: Some(o.clone()),
                        id: rec.active_id.take(),
                        name: rec.active_name.take(),
                        extra: Default::default(),
                    },
                );
                pos += 1;
            }
            if !(outgoing_pristine && same_master) {
                // The master this develop sits on had no pristine card of
                // its own yet. When the OUTGOING card was a pristine card
                // with no pixel record to mint its ghost from (pixels.json
                // missing, or not marked generated), its identity and name
                // — what version snapshots point at (R24-2) — move onto
                // this fresh master card rather than vanishing, as they did
                // until 2026-09-24.
                let orphaned = outgoing_pristine && recorded.is_none();
                rec.others.insert(
                    pos,
                    VariantEntry {
                        kind: "generated".to_string(),
                        recipe: EditRecipe::default(),
                        origin: Some(master.to_path_buf()),
                        id: orphaned.then(|| rec.active_id.take()).flatten(),
                        name: orphaned.then(|| rec.active_name.take()).flatten(),
                        extra: Default::default(),
                    },
                );
                pos += 1;
            }
            if rec.active_kind != "edited" {
                // A ▣ / ◭ slot taken by an edited develop is a NEW card: the
                // old identity would attribute that card's snapshots to
                // this one. (An edited card keeps its own — the develop
                // inside it changed, the card did not.)
                rec.active_id = None;
                rec.active_name = None;
            }
            rec.active_kind = "edited".to_string();
            rec.active_pos = pos;
            Ok(CommitMember::Write(variants_record_bytes(src, &rec)?))
        }
    }
}
