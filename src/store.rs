//! Central per-user develop store — WHERE a photo's develop state lives.
//!
//! Until v0.12.0 every sidecar (recipe.json / .xmp / v<N> snapshots / mask
//! rasters) was keyed by the photo's bare file STEM inside a cwd-relative
//! `./out`. Two same-named photos in different folders therefore shared one
//! sidecar (silent cross-clobber), and launching the app from a different
//! directory hid every existing edit. This module fixes both by giving each
//! photo its own directory under a per-user root, keyed by the photo's
//! ABSOLUTE path:
//!
//! ```text
//! <root>/develops/<stem>-<fnv1a64(abs path)>/
//!     recipe.json          the working develop (single source of truth)
//!     <stem>.xmp           Lightroom projection (name kept: LR needs <stem>.xmp)
//!     v<N>.recipe.json     numbered snapshots
//!     <kind>.png           mask rasters (mask-sky, mask-zone-sky, …)
//!     source.txt           breadcrumb: which photo this dir belongs to
//! ```
//!
//! The root resolves at runtime (never hardcoded): `AUTOSHOP_DATA_DIR` env
//! override → `%LOCALAPPDATA%/autoshop` (the thumb cache already lives there)
//! → the system temp dir. EXPORTS (developed/retouch/heal/… images) are user
//! deliverables and deliberately STAY in ./out.
//!
//! Mask rasters are referenced from recipe.json by a path string. Inside the
//! store that string is the BARE file name, resolved against the recipe's own
//! directory at load time ([`resolve_mask_paths`]) and relativized back at
//! write time ([`relativize_mask_paths`]) — so a develop dir is relocatable
//! and legacy cwd-relative "out/…" references keep working unchanged.

use std::path::{Path, PathBuf};

use crate::recipe::{EditRecipe, MaskGeometry};

/// Per-user store root: `AUTOSHOP_DATA_DIR` env override (tests, portable
/// setups) → `%LOCALAPPDATA%/autoshop` → `<temp>/autoshop`. Absolute, so keys
/// and targets never depend on the process cwd.
pub fn store_root() -> PathBuf {
    let root = std::env::var_os("AUTOSHOP_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("LOCALAPPDATA")
            .map(|d| PathBuf::from(d).join("autoshop"))
            .unwrap_or_else(|| std::env::temp_dir().join("autoshop"))
    });
    std::path::absolute(&root).unwrap_or(root)
}

/// FNV-1a 64-bit. Deliberately hand-rolled: `DefaultHasher` is NOT stable
/// across Rust releases, and this hash names PERSISTENT directories — a
/// changed hash would orphan every existing develop on a toolchain bump.
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET, |h, b| (h ^ u64::from(*b)).wrapping_mul(PRIME))
}

/// Stable per-photo key: `<stem>-<16 hex>` from the photo's absolute path.
/// The stem prefix keeps the store browsable; the hash disambiguates
/// same-named photos in different folders. Windows paths are case-insensitive
/// (NTFS), so the key hashes the lowercased path there — one file must never
/// produce two keys just because it was opened as `D:\` once and `d:\` once.
pub fn photo_key(src: &Path) -> String {
    let abs = std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf());
    let mut s = abs.to_string_lossy().into_owned();
    if cfg!(windows) {
        s = s.to_lowercase();
    }
    format!("{}-{:016x}", crate::pipeline::stem(src), fnv1a64(s.as_bytes()))
}

/// This photo's develop directory (not created here).
pub fn develop_dir(src: &Path) -> PathBuf {
    develop_dir_in(&store_root(), src)
}

/// Root-parameterized core of [`develop_dir`] so tests can use a temp root
/// without mutating process-global env (set_var is unsafe + racy in 2024).
fn develop_dir_in(root: &Path, src: &Path) -> PathBuf {
    root.join("develops").join(photo_key(src))
}

/// The working recipe — the single source of truth for a photo's develop.
pub fn recipe_target(src: &Path) -> PathBuf {
    develop_dir(src).join("recipe.json")
}

/// The Lightroom XMP projection. Keeps the `<stem>.xmp` name so copying it
/// beside the RAW for Lightroom needs no rename.
pub fn xmp_target(src: &Path) -> PathBuf {
    develop_dir(src).join(format!("{}.xmp", crate::pipeline::stem(src)))
}

/// Numbered snapshot `v<n>.recipe.json` (GUI versions + programmatic backups).
pub fn version_target(src: &Path, n: u32) -> PathBuf {
    develop_dir(src).join(format!("v{n}.recipe.json"))
}

/// A mask raster (`mask-sky.png`, `mask-zone-sky.png`, …) inside the photo's
/// develop dir. Recipes reference it by bare file name (see module docs).
pub fn raster_target(src: &Path, kind: &str) -> PathBuf {
    develop_dir(src).join(format!("{kind}.png"))
}

/// The style reference index — one per user, not per photo.
pub fn style_index_path() -> PathBuf {
    store_root().join("style-index.json")
}

/// UI-written local settings (used to be a cwd-relative `autoshop.local.json`).
pub fn settings_path() -> PathBuf {
    store_root().join("autoshop.local.json")
}

/// Legacy (pre-store) sidecar locations in cwd-relative ./out — still read as
/// fallbacks and consumed by [`migrate_legacy`].
pub fn legacy_recipe(src: &Path) -> PathBuf {
    PathBuf::from("out").join(format!("{}.recipe.json", crate::pipeline::stem(src)))
}

pub fn legacy_xmp(src: &Path) -> PathBuf {
    PathBuf::from("out").join(format!("{}.xmp", crate::pipeline::stem(src)))
}

/// Does this photo have ANY saved develop — central or legacy, recipe or XMP?
/// Existence-only (no parse): the gallery badge and the web list call this per
/// photo per refresh.
pub fn has_develop(src: &Path) -> bool {
    recipe_target(src).exists()
        || xmp_target(src).exists()
        || legacy_recipe(src).exists()
        || legacy_xmp(src).exists()
}

/// Snapshot numbers present in the photo's develop dir, sorted ascending.
pub fn list_versions(src: &Path) -> Vec<u32> {
    let mut out = Vec::new();
    if let Ok(dir) = std::fs::read_dir(develop_dir(src)) {
        for e in dir.flatten() {
            let name = e.file_name();
            let Some(name) = name.to_str() else { continue };
            if let Some(rest) = name.strip_prefix("v")
                && let Some(nums) = rest.strip_suffix(".recipe.json")
                && let Ok(n) = nums.parse::<u32>()
            {
                out.push(n);
            }
        }
    }
    out.sort_unstable();
    out
}

/// Resolve relative Bitmap mask paths against `base` (the directory the recipe
/// was loaded from). Only rewrites a reference whose file actually EXISTS
/// under `base` — anything else is left untouched, so legacy cwd-relative
/// "out/…" references keep resolving exactly as before this module existed.
pub fn resolve_mask_paths(r: &mut EditRecipe, base: &Path) {
    for m in &mut r.masks {
        if let MaskGeometry::Bitmap { path } = &mut m.mask {
            let p = Path::new(path.as_str());
            if p.is_relative() {
                let cand = base.join(p);
                if cand.exists() {
                    *path = cand.to_string_lossy().into_owned();
                }
            }
        }
    }
}

/// Inverse of [`resolve_mask_paths`] at write time: an absolute Bitmap path
/// that lives DIRECTLY inside `base` is stored as its bare file name, keeping
/// the develop dir relocatable. Paths elsewhere are stored as given.
pub fn relativize_mask_paths(r: &mut EditRecipe, base: &Path) {
    for m in &mut r.masks {
        if let MaskGeometry::Bitmap { path } = &mut m.mask {
            let p = Path::new(path.as_str());
            if p.is_absolute()
                && p.parent() == Some(base)
                && let Some(name) = p.file_name().and_then(|n| n.to_str())
            {
                *path = name.to_string();
            }
        }
    }
}

/// Best-effort breadcrumb so a human browsing the hashed store can tell which
/// photo a develop dir belongs to. Never fails the caller.
pub fn note_source(src: &Path) {
    let dir = develop_dir(src);
    let marker = dir.join("source.txt");
    if marker.exists() {
        return;
    }
    let abs = std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf());
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(&marker, format!("{}\n", abs.display()));
    }
}

/// `fs::rename`, falling back to copy+delete: ./out and the store root are
/// routinely on DIFFERENT volumes (project on D:, %LOCALAPPDATA% on C:), where
/// rename fails with a cross-device error.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
}

/// One-time, per-photo migration of legacy ./out sidecars into the central
/// develop dir. Idempotent and best-effort per file: a file that cannot move
/// stays where it was and the legacy read fallbacks keep serving it (nothing
/// is ever deleted without its copy landing first). Returns true when at
/// least one file was migrated.
///
/// Recipe files are parsed so their raster references can move along and be
/// rewritten to bare names; an UNPARSABLE recipe is moved byte-for-byte —
/// the loud `Unreadable` handling at read time stays intact.
///
/// Concurrency: two processes migrating the same photo at once (GUI + `serve`)
/// race benignly — both derive from the same legacy bytes, so whichever write
/// lands carries the same content, and the per-file existence checks plus
/// resumability finish any remainder on the next touch. No lock: the store is
/// single-user by construction, and identical-content races cannot corrupt.
pub fn migrate_legacy(src: &Path) -> bool {
    migrate_legacy_in(&store_root(), src)
}

fn migrate_legacy_in(root: &Path, src: &Path) -> bool {
    let stem = crate::pipeline::stem(src);
    let dev = develop_dir_in(root, src);

    // Collect (legacy file, new name) pairs. Versions are found by scan.
    let mut jobs: Vec<(PathBuf, String, bool)> = Vec::new(); // (from, to-name, is_recipe)
    let lr = legacy_recipe(src);
    if lr.exists() {
        jobs.push((lr, "recipe.json".into(), true));
    }
    let lx = legacy_xmp(src);
    if lx.exists() {
        jobs.push((lx, format!("{stem}.xmp"), false));
    }
    let vprefix = format!("{stem}.v");
    if let Ok(dir) = std::fs::read_dir("out") {
        for e in dir.flatten() {
            let name = e.file_name();
            let Some(name) = name.to_str() else { continue };
            if let Some(rest) = name.strip_prefix(&vprefix)
                && let Some(nums) = rest.strip_suffix(".recipe.json")
                && nums.parse::<u32>().is_ok()
            {
                jobs.push((e.path(), format!("v{nums}.recipe.json"), true));
            }
        }
    }
    if jobs.is_empty() {
        return false;
    }
    if std::fs::create_dir_all(&dev).is_err() {
        return false;
    }
    let abs_src = std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf());
    let marker = dev.join("source.txt");
    if !marker.exists() {
        let _ = std::fs::write(&marker, format!("{}\n", abs_src.display()));
    }

    let mut moved = false;
    for (from, to_name, is_recipe) in jobs {
        let to = dev.join(&to_name);
        if to.exists() {
            // The central copy is the post-migration truth — never clobber it
            // with an older legacy file. The legacy file stays for the read
            // fallbacks (deleting user data it never copied would be worse).
            continue;
        }
        let ok = if is_recipe {
            migrate_one_recipe(&from, &to, stem, &dev)
        } else {
            move_file(&from, &to).is_ok()
        };
        moved |= ok;
    }
    moved
}

/// Move one legacy recipe file, carrying its `./out/<stem>.<kind>.png` raster
/// references into the develop dir (rewritten to bare `<kind>.png` names).
/// Rasters shared by several recipe files move once; later files just rewrite.
///
/// Failure-ordering contract (a migration must never make things WORSE than
/// not migrating): rasters are STAGED as copies first, the rewritten recipe is
/// published next, and the legacy originals are deleted only after the recipe
/// landed. Any earlier failure rolls the staged copies back and leaves every
/// legacy file byte-identical — the read fallbacks keep serving it.
fn migrate_one_recipe(from: &Path, to: &Path, stem: &str, dev: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(from) else { return false };
    let Ok(mut r) = serde_json::from_str::<EditRecipe>(&text) else {
        // Unparsable (interrupted write / newer schema): move byte-for-byte so
        // the read path can keep reporting it loudly as Unreadable.
        return move_file(from, to).is_ok();
    };
    let raster_prefix = format!("{stem}.");
    // (legacy raster to delete on success, staged central copy to roll back on failure)
    let mut staged: Vec<(PathBuf, PathBuf)> = Vec::new();
    for m in &mut r.masks {
        if let MaskGeometry::Bitmap { path } = &mut m.mask {
            let p = PathBuf::from(path.as_str());
            if p.is_absolute() {
                continue; // foreign reference — not ours to move
            }
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else { continue };
            let Some(bare) = name.strip_prefix(&raster_prefix) else { continue };
            let dest = dev.join(bare);
            if dest.exists() {
                *path = bare.to_string(); // already migrated by an earlier file
            } else if p.exists() && std::fs::copy(&p, &dest).is_ok() {
                staged.push((p.clone(), dest));
                *path = bare.to_string();
            }
            // Raster missing/uncopyable: keep the old reference — the engine's
            // missing-raster contract (inert + warning) reports it.
        }
    }
    // Publish the rewritten recipe via tmp+rename (`to` is known absent): a
    // crash mid-write must not leave a half-written central recipe shadowing
    // the intact legacy one.
    let publish = serde_json::to_string_pretty(&r).ok().and_then(|json| {
        let tmp = to.with_extension("json.tmp");
        std::fs::write(&tmp, json).ok()?;
        std::fs::rename(&tmp, to).ok()
    });
    if publish.is_none() {
        for (_, dest) in &staged {
            let _ = std::fs::remove_file(dest); // roll back — legacy stays whole
        }
        return false;
    }
    let _ = std::fs::remove_file(from);
    for (legacy, _) in &staged {
        let _ = std::fs::remove_file(legacy);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::LocalAdjustment;

    #[test]
    fn photo_key_disambiguates_same_stem_different_folder() {
        // The exact bug this module exists to fix: DSC001.ARW in two folders
        // must never share one develop dir.
        let a = photo_key(Path::new("D:/trip-a/DSC001.ARW"));
        let b = photo_key(Path::new("D:/trip-b/DSC001.ARW"));
        assert!(a.starts_with("DSC001-"), "{a}");
        assert!(b.starts_with("DSC001-"), "{b}");
        assert_ne!(a, b);
        // Stable across calls (persistent directory names).
        assert_eq!(a, photo_key(Path::new("D:/trip-a/DSC001.ARW")));
    }

    #[test]
    fn fnv1a64_is_the_reference_function() {
        // Pin the published FNV-1a test vectors: this hash names directories
        // on disk, so it must never drift.
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn resolve_and_relativize_round_trip() {
        let base = std::env::temp_dir().join("autoshop-store-test-roundtrip");
        std::fs::create_dir_all(&base).unwrap();
        let raster = base.join("mask-sky.png");
        std::fs::write(&raster, b"png").unwrap();

        let mut r = EditRecipe::default();
        r.masks.push(LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: "mask-sky.png".into() },
            ..Default::default()
        });
        r.masks.push(LocalAdjustment {
            // Not under base and not existing → must stay untouched.
            mask: MaskGeometry::Bitmap { path: "out/other.mask.png".into() },
            ..Default::default()
        });
        resolve_mask_paths(&mut r, &base);
        let MaskGeometry::Bitmap { path } = &r.masks[0].mask else { panic!() };
        assert_eq!(Path::new(path), raster.as_path(), "existing bare name resolves to base");
        let MaskGeometry::Bitmap { path } = &r.masks[1].mask else { panic!() };
        assert_eq!(path, "out/other.mask.png", "missing reference left alone");

        relativize_mask_paths(&mut r, &base);
        let MaskGeometry::Bitmap { path } = &r.masks[0].mask else { panic!() };
        assert_eq!(path, "mask-sky.png", "absolute-under-base collapses back to bare");
        let _ = std::fs::remove_file(&raster);
    }

    #[test]
    fn migrate_legacy_moves_recipe_xmp_versions_and_rasters() {
        // Runs against the real cwd ./out (like the render/fit tests) with
        // unique _store_mig_* names, and a temp store root via the _in APIs —
        // no process-global env mutation.
        let stem = "_store_mig_photo";
        let src = PathBuf::from(format!("D:/nowhere/{stem}.ARW"));
        let root = std::env::temp_dir().join("autoshop-store-test-migrate");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all("out").unwrap();

        let raster = PathBuf::from(format!("out/{stem}.mask-sky.png"));
        std::fs::write(&raster, b"png").unwrap();
        let mut r = EditRecipe { exposure_ev: 0.5, ..Default::default() };
        r.masks.push(LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: raster.to_string_lossy().into_owned() },
            ..Default::default()
        });
        let rj = PathBuf::from(format!("out/{stem}.recipe.json"));
        std::fs::write(&rj, serde_json::to_string_pretty(&r).unwrap()).unwrap();
        let xmp = PathBuf::from(format!("out/{stem}.xmp"));
        std::fs::write(&xmp, "<x:xmpmeta/>").unwrap();
        let v2 = PathBuf::from(format!("out/{stem}.v2.recipe.json"));
        std::fs::write(&v2, serde_json::to_string_pretty(&EditRecipe::default()).unwrap()).unwrap();

        assert!(migrate_legacy_in(&root, &src));
        let dev = develop_dir_in(&root, &src);
        assert!(dev.join("recipe.json").exists());
        assert!(dev.join(format!("{stem}.xmp")).exists());
        assert!(dev.join("v2.recipe.json").exists());
        assert!(dev.join("mask-sky.png").exists(), "raster moved along");
        assert!(dev.join("source.txt").exists(), "breadcrumb written");
        assert!(!rj.exists() && !xmp.exists() && !v2.exists() && !raster.exists(), "legacy consumed");

        // The migrated recipe references the raster by bare name.
        let back: EditRecipe =
            serde_json::from_str(&std::fs::read_to_string(dev.join("recipe.json")).unwrap()).unwrap();
        let MaskGeometry::Bitmap { path } = &back.masks[0].mask else { panic!() };
        assert_eq!(path, "mask-sky.png");

        // Idempotent: a second call finds nothing legacy and reports false.
        assert!(!migrate_legacy_in(&root, &src));
        let _ = std::fs::remove_dir_all(&root);
    }
}
