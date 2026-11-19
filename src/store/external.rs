//! External XMP: the file beside a RAW (or the packet inside it) that the app reports but never adopts, and the answered stamp that silences it.
//!
//! An `.xmp` beside the photo is another program's file (usually Lightroom's).
//! The app reads one only when the user asks — an import — and writes one
//! only when the user asks — an export. What this module answers is the one
//! question in between: "is there an external XMP this app has not dealt with
//! yet?", so a surface can ASK instead of adopting. It never adopts, never
//! writes beside the photo, and never touches the develop dir while it looks.

use super::*;

/// Content tag of an XMP document: FNV-1a 64 of its bytes, hex. The one
/// spelling the detector, the answered stamp and the export all compare.
pub fn xmp_tag(text: &str) -> String {
    tag_of(text.as_bytes())
}

fn tag_of(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a64(bytes))
}

/// An external XMP this app has not dealt with yet.
pub enum ExternalXmp {
    /// Nothing to ask about.
    None,
    /// `<dir>/<stem>.xmp` beside the RAW — the same spelling
    /// [`xmp_beside_target`] writes, so our own export is recognised.
    Beside { path: PathBuf, text: String, tag: String },
    /// The packet another program baked INTO the RAW (a DNG, usually). The
    /// lowest priority: asked about only when no file sits beside the photo,
    /// the store holds no develop, and no explicit clear stands — a file this
    /// app never writes cannot be retired by a clear, so the clear markers are
    /// what keep it from coming back after the user said "no edits".
    Embedded { text: String, tag: String },
}

/// The answered stamp: the tag of the last external XMP the user dealt with
/// for this photo — imported it, declined it, or had this app write it.
fn answered_path(src: &Path) -> PathBuf {
    develop_dir(src).join("external-xmp.answered")
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Answered {
    v: u32,
    tag: String,
}

/// The stamp's tag, or `None` when there is none or it cannot be read. An
/// unreadable stamp fails toward ASKING: a question too many costs a click,
/// a question swallowed hides a file the user has never seen.
fn answered_tag(src: &Path) -> Option<String> {
    let text = read_text_capped(&answered_path(src), MAX_STORE_JSON).ok()?;
    serde_json::from_str::<Answered>(&text).ok().map(|a| a.tag)
}

/// Remember that the external XMP with this `tag` has been dealt with —
/// imported, declined, or written by this app's own export — so it is not
/// asked about again until its content changes.
///
/// The stamp is NOT a develop: [`has_develop`] does not count it (a photo
/// whose only store file is this stays un-badged) and `clear_sweep` leaves
/// it standing (clearing edits does not un-answer a question). Under the
/// develop lock in the caller's `mode`, like every other store write; the
/// lock creates the develop dir when there is none yet.
pub fn answer_external_xmp(src: &Path, tag: &str, mode: DevelopLockMode) -> std::io::Result<()> {
    with_develop_lock(src, mode, || {
        let json = serde_json::to_vec(&Answered { v: 1, tag: tag.to_string() })
            .map_err(std::io::Error::other)?;
        durable_write(&answered_path(src), &json)
    })
}

/// [`answer_external_xmp`] for bytes this app itself just wrote beside the
/// photo (`deliver_xmp_beside`).
pub(super) fn answer_external_bytes(src: &Path, bytes: &[u8], mode: DevelopLockMode) -> std::io::Result<()> {
    answer_external_xmp(src, &tag_of(bytes), mode)
}

/// Is there an external XMP the user should be asked about?
///
/// The file beside the RAW is reported when it holds adjustments (the SILENT
/// reader says it is not a no-op), it is not byte-equal to this app's own
/// stored XMP (builds before this one told users to copy that file beside the
/// photo), its tag is not the answered one, and it is NEWER than every file
/// this app has written about the photo — the recipe, its own old XMP, the
/// legacy pair, the explicit-clear markers. A file older than the user's own
/// AutoShade work is not news. With no file beside the photo, the RAW's
/// embedded packet is reported under the narrower rule [`ExternalXmp::Embedded`]
/// states.
///
/// Side-effect free, by contract and by test: it takes no lock (the lock
/// creates the develop dir), and it reads through `xmp_to_recipe_for_photo`,
/// the reader that never places a payload's rasters. A file that cannot be
/// read, or that changed while it was read, is simply not reported — nothing
/// is adopted from it, so there is nothing to warn about, and the explicit
/// import names the error if the user picks it.
pub fn external_xmp(src: &Path) -> ExternalXmp {
    // Only a camera RAW has the sidecar convention; a baked PNG/TIFF's
    // neighbouring .xmp is some other program's business.
    if !crate::decode::is_raw(src) {
        return ExternalXmp::None;
    }
    // A clear in progress: whatever stands is being removed — nothing to ask.
    if clear_pending(src).exists() {
        return ExternalXmp::None;
    }
    let beside = src.with_extension("xmp");
    match read_sidecar_stamped(&beside) {
        (SidecarRead::Ok(text), stamp) => rank_external(src, &beside, text, stamp),
        (SidecarRead::Unreadable(_), _) => ExternalXmp::None,
        (SidecarRead::Missing, _) => embedded_external(src),
    }
}

/// The ranking half of [`external_xmp`], split so a test can hand it a
/// deliberately mismatched identity. `stamp` is the HANDLE identity of the
/// text read; the time compared is that handle's, never a fresh path stat
/// that may describe a different file after a swap.
pub(super) fn rank_external(
    src: &Path,
    path: &Path,
    text: String,
    stamp: Option<SidecarStamp>,
) -> ExternalXmp {
    let tag = xmp_tag(&text);
    if answered_tag(src).as_deref() == Some(tag.as_str()) {
        return ExternalXmp::None;
    }
    for ours in [xmp_target(src), legacy_xmp(src)] {
        if read_sidecar(&ours).is_some_and(|t| t == text) {
            return ExternalXmp::None;
        }
    }
    let file_t = match stamp {
        // The path must still name the very file the handle read; a swap or
        // an unlink after the read means the text describes nothing beside
        // the photo any more.
        Some(s) => {
            let now = std::fs::metadata(path)
                .ok()
                .and_then(|m| m.modified().ok().map(|t| (t, m.len())));
            if now != Some(s) {
                return ExternalXmp::None;
            }
            s.0
        }
        None => match std::fs::metadata(path).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => return ExternalXmp::None,
        },
    };
    let own_t = [
        recipe_target(src),
        xmp_target(src),
        legacy_recipe(src),
        legacy_xmp(src),
        develop_dir(src).join("cleared.txt"),
        clear_pending(src),
    ]
    .iter()
    .filter_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
    .max();
    if own_t.is_some_and(|o| file_t <= o) {
        return ExternalXmp::None;
    }
    // Last, because it parses: a ratings-only or otherwise neutral file has
    // nothing to import, so there is nothing to ask.
    if crate::xmp::xmp_to_recipe_for_photo(&text, src).is_noop() {
        return ExternalXmp::None;
    }
    ExternalXmp::Beside { path: path.to_path_buf(), text, tag }
}

fn embedded_external(src: &Path) -> ExternalXmp {
    if has_develop(src) || develop_dir(src).join("cleared.txt").exists() {
        return ExternalXmp::None;
    }
    let Ok(Some(text)) = crate::decode::embedded_xmp(src) else {
        return ExternalXmp::None;
    };
    let tag = xmp_tag(&text);
    if answered_tag(src).as_deref() == Some(tag.as_str())
        || crate::xmp::xmp_to_recipe_for_photo(&text, src).is_noop()
    {
        return ExternalXmp::None;
    }
    ExternalXmp::Embedded { text, tag }
}
