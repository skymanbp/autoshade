//! ONE rule for pairing a photograph with the PHOTOGRAPHER's XMP sidecar.
//!
//! Until this module the rule was open-coded as `raw.with_extension("xmp")` at
//! six sites — the style-index pair scan and its sidecar read, `eval`'s pair
//! scan, its per-photo read, its resume hash, its forensic probe, and the CLI's
//! Lightroom import note. Two defects followed from having no owner:
//!
//! * **A separate sidecar tree could not be used at all.** A photographer who
//!   keeps `.xmp` files beside the RAWs is the only one the index could learn
//!   from; anyone whose sidecars live in a parallel folder (a read-only photo
//!   volume, a catalogue exported elsewhere) got "0 RAW+.xmp pairs" and an
//!   empty-index refusal, with nothing to point at the real folder.
//! * **`with_extension` writes a LOWERCASE `.xmp`.** On Windows `Path::exists`
//!   folds case, so `SHOT.XMP` paired anyway; on macOS and Linux it does not,
//!   so the same library indexed on Windows and indexed on the Mac build (which
//!   has shipped since v1.1.0) produced different pair counts from the same
//!   files, and said nothing about it.
//!
//! **The rule.** Given a RAW, the sidecar is the file whose STEM is the RAW's
//! stem and whose EXTENSION is `xmp` in any ASCII case, looked for in three
//! places in order:
//!
//! 1. `<xmp_dir>/<the RAW's folder relative to the library root>/<stem>.xmp` —
//!    a sidecar tree that MIRRORS the library, which is what an exported
//!    catalogue and every "keep metadata off the photo volume" workflow
//!    produces;
//! 2. `<xmp_dir>/<stem>.xmp` — the FLAT mirror, one folder of sidecars for a
//!    nested library;
//! 3. `<the RAW's own folder>/<stem>.xmp` — the sibling, which is what
//!    Lightroom writes and the only place the old code looked.
//!
//! Mirror before flat before sibling, because each is more specific about
//! WHICH library this sidecar belongs to than the next: a flat `<stem>.xmp`
//! can be claimed by two RAWs of the same name in two subfolders, and the
//! sibling is a file the user may not even have meant to hand us.
//!
//! **The extension is matched case-insensitively; the STEM is not.** The
//! extension is the half the world spells inconsistently (Lightroom writes
//! `.xmp`, several camera utilities and older ACR builds write `.XMP`), while
//! every writer of a sidecar — Lightroom, ACR and this app's own
//! [`crate::store::xmp_beside_target`] — reproduces the photograph's stem byte
//! for byte. Folding the stem too would let `shot.arw` claim `Shot.xmp` on a
//! case-sensitive volume, i.e. one photograph's edit scored against another's
//! pixels, which is a worse failure than the one being fixed.
//!
//! **Directory listing, never `Path::exists`.** Case-insensitive matching by
//! probing `<stem>.XMP`, `<stem>.Xmp`, … is eight stats per photograph and
//! still only covers the casings someone thought of; and on a case-insensitive
//! volume `exists()` answers about a name the FS chose, not the one on disk.
//! One `read_dir` per FOLDER, memoised, answers exactly and once.
//!
//! **What this module deliberately does NOT own.** The develop chain's own
//! sidecar sites — [`crate::store::lightroom_sidecar`],
//! `store::has_develop_or_sidecar` and `pipeline::write_xmp`'s merge base —
//! read a path that is also the path AutoShade WRITES back to
//! ([`crate::store::xmp_beside_target`]). Resolving those reads
//! case-insensitively without moving the write would split one photograph's
//! sidecar into two files (`SHOT.XMP` read, `shot.xmp` written), so that pair
//! is a separate change with its own risk and is registered, not smuggled in
//! here. Everything this module owns is READ-ONLY: nothing is ever written to
//! a path it returns.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// The sidecar extension in the spelling every WRITER uses. Compared with
/// [`str::eq_ignore_ascii_case`], written as-is.
const XMP_EXT: &str = "xmp";

/// One library scan's pairing rule, with the folder listings it has already
/// paid for.
///
/// Held by ONE thread for the length of a scan: the memo behind it is a
/// [`RefCell`], deliberately, because every caller resolves its pairs up front
/// on the calling thread and carries the resolved paths into whatever pool
/// comes next. That ordering is what keeps a 2,000-photograph library at one
/// `read_dir` per folder instead of one per photograph.
pub struct XmpPairing<'a> {
    root: Option<&'a Path>,
    xmp_dir: Option<&'a Path>,
    listed: RefCell<HashMap<PathBuf, HashMap<OsString, OsString>>>,
}

impl<'a> XmpPairing<'a> {
    /// The pairing for a library scan: `root` is the folder the RAWs were
    /// found under, `xmp_dir` the `--xmp-dir` the user gave (or `None`).
    pub fn new(root: &'a Path, xmp_dir: Option<&'a Path>) -> Self {
        XmpPairing { root: Some(root), xmp_dir, listed: RefCell::new(HashMap::new()) }
    }

    /// The convention with no separate sidecar tree: the sibling file alone.
    ///
    /// For the single-photograph callers (the CLI's Lightroom import note),
    /// which have no library root to be relative to. They still go through
    /// this module rather than `with_extension`, because the case rule is the
    /// whole point and a photograph does not stop having a `.XMP` sidecar
    /// because it was named on a command line.
    pub fn beside() -> XmpPairing<'static> {
        XmpPairing { root: None, xmp_dir: None, listed: RefCell::new(HashMap::new()) }
    }

    /// This RAW's sidecar, or `None` when no folder in the rule holds one.
    ///
    /// The returned path names a file that EXISTED when its folder was listed.
    /// It is never a name to write to — see the module docs.
    pub fn find(&self, raw: &Path) -> Option<PathBuf> {
        let stem = raw.file_stem()?;
        for dir in self.search_dirs(raw) {
            if let Some(name) = self.lookup(&dir, stem) {
                return Some(dir.join(name));
            }
        }
        None
    }

    /// The three folders of the rule, in order, deduplicated — a flat
    /// `--xmp-dir` pointed at the library root is ONE folder, not three
    /// listings of it.
    fn search_dirs(&self, raw: &Path) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = Vec::with_capacity(3);
        if let Some(x) = self.xmp_dir {
            if let Some(rel) = self.root.and_then(|r| raw.parent()?.strip_prefix(r).ok()) {
                dirs.push(x.join(rel));
            }
            let flat = x.to_path_buf();
            if !dirs.contains(&flat) {
                dirs.push(flat);
            }
        }
        if let Some(p) = raw.parent() {
            let sibling = p.to_path_buf();
            if !dirs.contains(&sibling) {
                dirs.push(sibling);
            }
        }
        dirs
    }

    /// The sidecar named `<stem>.<any case of xmp>` in `dir`, from the
    /// memoised listing.
    fn lookup(&self, dir: &Path, stem: &OsStr) -> Option<OsString> {
        let mut listed = self.listed.borrow_mut();
        let names = listed.entry(dir.to_path_buf()).or_insert_with(|| list_xmps(dir));
        names.get(stem).cloned()
    }
}

/// Every `<stem> -> <file name>` this folder offers, by ONE listing.
///
/// An unreadable folder (absent, denied, a `--xmp-dir` the user mistyped)
/// answers an EMPTY map rather than failing: a missing sidecar folder means
/// "no pair from here", which the next candidate folder — and, failing that,
/// the build's own skipped-for-sidecar disclosure — already reports honestly.
fn list_xmps(dir: &Path) -> HashMap<OsString, OsString> {
    // `Path::new("")` is what `Path::parent` answers for a bare file name, and
    // `read_dir("")` fails on every platform. The CWD is what
    // `with_extension("xmp").exists()` resolved such a name against, so it is
    // what this resolves against too.
    let target = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    let mut out: HashMap<OsString, OsString> = HashMap::new();
    let Ok(rd) = std::fs::read_dir(target) else { return out };
    for entry in rd.flatten() {
        // A FOLDER called `shot.xmp` is not a sidecar. Cheap here (the entry's
        // type is already in hand on both platforms) and the alternative is an
        // unreadable-file error several frames later.
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name();
        let as_path = Path::new(&name);
        if !as_path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case(XMP_EXT))
        {
            continue;
        }
        let Some(stem) = as_path.file_stem() else { continue };
        match out.entry(stem.to_os_string()) {
            Entry::Vacant(v) => {
                v.insert(name);
            }
            Entry::Occupied(mut o) => {
                if outranks(&name, o.get()) {
                    o.insert(name);
                }
            }
        }
    }
    out
}

/// Which of two same-stem candidates a folder offers is THE sidecar.
///
/// Only reachable on a case-SENSITIVE volume, where `shot.xmp` and `shot.XMP`
/// can both exist. Exactly-`xmp` wins, because that is the spelling every
/// writer produces and so the one the photographer's live editor is keeping
/// current; between two equally-odd casings the lexicographically smaller name
/// wins, so the answer does not depend on the order the OS happened to list
/// the folder in.
fn outranks(candidate: &OsStr, current: &OsStr) -> bool {
    let exact = |n: &OsStr| Path::new(n).extension().is_some_and(|e| e == OsStr::new(XMP_EXT));
    match (exact(candidate), exact(current)) {
        (true, false) => true,
        (false, true) => false,
        _ => candidate < current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"<x:xmpmeta/>").unwrap();
    }

    /// One library tree and one sidecar tree, both present and both empty,
    /// under a per-test root: `(root to delete, library, sidecars)`.
    ///
    /// Both folders are CREATED even when a case does not use one, so an
    /// absent folder is never what a negative assertion is really measuring.
    fn fixture(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = crate::test_dir(tag);
        let (root, side) = (base.join("library"), base.join("sidecars"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&side).unwrap();
        (base, root, side)
    }

    /// MUTATION: drop the `eq_ignore_ascii_case` in [`list_xmps`] and this
    /// fails. It is the whole macOS/Linux half of the defect — the same
    /// library paired on Windows and did not on the shipped Mac build.
    #[test]
    fn the_sidecar_extension_is_matched_in_any_case_on_every_platform() {
        let dir = crate::test_dir("xmp-pair-case");
        let raw = dir.join("shot-a.arw");
        touch(&raw);
        touch(&dir.join("shot-a.XMP"));
        let pairing = XmpPairing::new(&dir, None);
        assert_eq!(
            pairing.find(&raw).and_then(|p| p.file_name().map(|n| n.to_os_string())),
            Some(OsString::from("shot-a.XMP")),
            "an upper-case sidecar is the same sidecar"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// MUTATION: drop the `strip_prefix` mirror candidate and this fails.
    #[test]
    fn an_xmp_dir_mirrors_the_librarys_own_tree() {
        let (base, root, side) = fixture("xmp-pair-mirror");
        let raw = root.join("2019/spring/shot-c.arw");
        touch(&raw);
        touch(&side.join("2019/spring/shot-c.xmp"));
        let pairing = XmpPairing::new(&root, Some(side.as_path()));
        assert_eq!(pairing.find(&raw), Some(side.join("2019/spring/shot-c.xmp")));
        std::fs::remove_dir_all(&base).ok();
    }

    /// MUTATION: drop the flat `<xmp_dir>/<stem>.xmp` candidate and this fails.
    #[test]
    fn an_xmp_dir_also_answers_as_a_flat_mirror() {
        let (base, root, side) = fixture("xmp-pair-flat");
        let raw = root.join("2019/spring/shot-d.arw");
        touch(&raw);
        touch(&side.join("shot-d.xmp"));
        let pairing = XmpPairing::new(&root, Some(side.as_path()));
        assert_eq!(pairing.find(&raw), Some(side.join("shot-d.xmp")));
        std::fs::remove_dir_all(&base).ok();
    }

    /// MUTATION: drop the sibling candidate and this fails — the sibling is
    /// what Lightroom writes and what every build before `--xmp-dir` used.
    #[test]
    fn the_sibling_is_the_last_resort_even_when_an_xmp_dir_was_given() {
        let (base, root, side) = fixture("xmp-pair-sibling");
        let raw = root.join("shot-e.arw");
        touch(&raw);
        touch(&root.join("shot-e.xmp"));
        let pairing = XmpPairing::new(&root, Some(side.as_path()));
        assert_eq!(pairing.find(&raw), Some(root.join("shot-e.xmp")));
        std::fs::remove_dir_all(&base).ok();
    }

    /// The ORDER, in one fixture: all three places hold a sidecar and the
    /// mirror wins. MUTATION: reorder [`XmpPairing::search_dirs`] and this
    /// fails.
    #[test]
    fn the_mirror_outranks_the_flat_mirror_which_outranks_the_sibling() {
        let (base, root, side) = fixture("xmp-pair-order");
        let raw = root.join("roll/shot-f.arw");
        touch(&raw);
        touch(&root.join("roll/shot-f.xmp"));
        touch(&side.join("shot-f.xmp"));
        touch(&side.join("roll/shot-f.xmp"));
        let pairing = XmpPairing::new(&root, Some(side.as_path()));
        assert_eq!(pairing.find(&raw), Some(side.join("roll/shot-f.xmp")));
        // Take the mirror away and the flat one answers; take that away too
        // and the sibling does.
        std::fs::remove_file(side.join("roll/shot-f.xmp")).unwrap();
        let pairing = XmpPairing::new(&root, Some(side.as_path()));
        assert_eq!(pairing.find(&raw), Some(side.join("shot-f.xmp")));
        std::fs::remove_file(side.join("shot-f.xmp")).unwrap();
        let pairing = XmpPairing::new(&root, Some(side.as_path()));
        assert_eq!(pairing.find(&raw), Some(root.join("roll/shot-f.xmp")));
        std::fs::remove_dir_all(&base).ok();
    }

    /// A RAW with no sidecar anywhere pairs with NOTHING — the fact the build
    /// now discloses instead of silently dropping.
    #[test]
    fn a_raw_with_no_sidecar_anywhere_pairs_with_nothing() {
        let (base, root, side) = fixture("xmp-pair-none");
        let raw = root.join("shot-g.arw");
        touch(&raw);
        // A sidecar for a DIFFERENT photograph in each folder, so the folders
        // exist and are listed: the answer is about the stem, not the folder.
        touch(&root.join("shot-h.xmp"));
        touch(&side.join("shot-h.xmp"));
        let pairing = XmpPairing::new(&root, Some(side.as_path()));
        assert_eq!(pairing.find(&raw), None);
        // …and with no --xmp-dir at all.
        assert_eq!(XmpPairing::beside().find(&raw), None);
        std::fs::remove_dir_all(&base).ok();
    }

    /// A mistyped `--xmp-dir` is not an error, it is "no pair from there" —
    /// and the sibling still answers.
    #[test]
    fn an_absent_xmp_dir_degrades_to_the_sibling() {
        let (base, root, _side) = fixture("xmp-pair-absent");
        let raw = root.join("shot-i.arw");
        touch(&raw);
        touch(&root.join("shot-i.xmp"));
        let missing = base.join("no-such-folder");
        let pairing = XmpPairing::new(&root, Some(missing.as_path()));
        assert_eq!(pairing.find(&raw), Some(root.join("shot-i.xmp")));
        std::fs::remove_dir_all(&base).ok();
    }

    /// The tie-break, as a pure rule: it is only reachable on a
    /// case-sensitive volume, which the battery does not always run on, so it
    /// is proven here rather than through a fixture that cannot exist on
    /// Windows. MUTATION: drop the exact-`xmp` preference and this fails.
    #[test]
    fn the_exact_lowercase_spelling_outranks_every_other_casing() {
        let lower = OsString::from("shot.xmp");
        let upper = OsString::from("shot.XMP");
        let mixed = OsString::from("shot.Xmp");
        assert!(outranks(&lower, &upper), "the spelling every writer produces wins");
        assert!(!outranks(&upper, &lower));
        // Between two equally-odd casings the answer is the folder-order-free
        // one — the lexicographically smaller NAME — so two machines listing
        // the same folder in different orders still agree.
        assert!(outranks(&upper, &mixed), "'shot.XMP' < 'shot.Xmp' by byte order");
        assert!(!outranks(&mixed, &upper));
    }
}
