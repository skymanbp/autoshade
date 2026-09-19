//! Where Adobe Camera Raw keeps its data on THIS machine, and how to walk it.
//!
//! Two questions that used to live inside [`crate::lcp`] and now have a second
//! caller in [`crate::dcp`]: which directories hold a given kind of Camera Raw
//! resource, and which files under them carry a given extension. Lens profiles
//! (`LensProfiles/*.lcp`) and camera profiles (`CameraProfiles/*.dcp`) sit in
//! sibling directories of the same tree and are found by the same rules, so
//! those rules belong in one place rather than in two that can drift apart.
//!
//! Nothing here is bundled or redistributed. These are Adobe's files, installed
//! by the user's own Camera Raw; the engine reads them where they already are.

use std::path::{Path, PathBuf};

/// The Camera Raw directories named `subdir`, from the ENVIRONMENT — never a
/// hard-coded drive letter. A machine whose `%ProgramData%` is not on `C:` is
/// ordinary, and a literal path would degrade there while claiming to have
/// looked.
///
/// `env_var` is consulted FIRST when set: it is how an acceptance test points
/// at a fixture without an Adobe install, and how a user with resources
/// somewhere else names that place.
///
/// On a build with no Adobe variable in the environment this answers empty and
/// every caller degrades through its own named refusal.
pub fn camera_raw_roots(env_var: &str, subdir: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(dir) = crate::config::live_env(env_var)
        && !dir.is_empty()
    {
        out.push(PathBuf::from(dir));
    }
    // macOS: Camera Raw keeps the same layout machine-wide and per-user. Both
    // are spelled out as absolute literals BECAUSE macOS has no
    // `ProgramData`/`APPDATA` analogue to derive them from — they are fixed OS
    // locations, identical on every Mac, and the `is_dir` filter below means an
    // install without Camera Raw simply contributes nothing.
    #[cfg(target_os = "macos")]
    {
        out.push(Path::new("/Library/Application Support/Adobe/CameraRaw").join(subdir));
        if let Some(home) = std::env::var_os("HOME") {
            let mut p = Path::new(&home).to_path_buf();
            for part in ["Library", "Application Support", "Adobe", "CameraRaw", subdir] {
                p.push(part);
            }
            out.push(p);
        }
    }
    for var in ["ProgramData", "APPDATA"] {
        if let Ok(base) = std::env::var(var)
            && !base.is_empty()
        {
            out.push(Path::new(&base).join("Adobe").join("CameraRaw").join(subdir));
        }
    }
    out.retain(|p| p.is_dir());
    out
}

/// Every file under `roots` whose extension is `ext`, case-insensitively.
///
/// `budget` bounds the walk: a symlink loop under a profile root must not hang
/// a render. An exhausted budget returns what was found rather than failing —
/// a partial index produces a worse match, not a broken one, and the installed
/// pools this walks are two orders of magnitude under any budget a caller sets.
pub fn walk_extension(roots: Vec<PathBuf>, ext: &str, mut budget: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = roots;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            if budget == 0 {
                return out;
            }
            budget -= 1;
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(p),
                Ok(t) if t.is_file() => {
                    if p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)) {
                        out.push(p);
                    }
                }
                _ => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
