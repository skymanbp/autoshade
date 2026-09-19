use super::*;

/// The override leads, and only directories that EXIST survive.
///
/// Driven against the real filesystem rather than a mock, because the whole
/// point of the function is the `is_dir` filter: a version that returned every
/// candidate unconditionally would satisfy any test that never put a real
/// directory in front of it. The system temporary directory is used as the
/// "real" one precisely because nothing has to be created to know it is there.
///
/// MUTATION: drop the `retain(is_dir)`, append the override instead of pushing
/// it first, or accept an empty setting as a path.
#[test]
fn the_override_comes_first_and_only_real_directories_survive() {
    // This test owns the variable name — nothing else in the battery reads it.
    let var = "AUTOSHADE_TEST_ADOBE_DIR";
    let real = std::env::temp_dir();
    // SAFETY: the process-wide write is exactly what the function under test
    // reads, and no other test names this variable.
    unsafe { std::env::set_var(var, real.as_os_str()) };
    let roots = camera_raw_roots(var, "Profiles");
    assert_eq!(roots.first(), Some(&real), "the override must lead: {roots:?}");
    assert!(roots.iter().all(|p| p.is_dir()), "a non-directory survived: {roots:?}");

    // A name that does not exist contributes nothing at all — the caller then
    // reaches its own "no roots" refusal instead of a phantom directory.
    let missing = real.join("autoshade-no-such-profile-directory");
    unsafe { std::env::set_var(var, missing.as_os_str()) };
    assert!(
        !camera_raw_roots(var, "Profiles").contains(&missing),
        "a missing directory was kept as a root"
    );

    // An EMPTY setting is not a path: `PathBuf::from("")` is the process's own
    // working directory, which would silently make the repository a profile
    // root on any machine whose Camera Raw is not installed.
    unsafe { std::env::set_var(var, "") };
    assert!(
        !camera_raw_roots(var, "Profiles").contains(&PathBuf::new()),
        "an empty override became a root"
    );

    unsafe { std::env::remove_var(var) };
}

/// The walk is recursive, case-insensitive about the extension, and bounded.
///
/// MUTATION: compare the extension case-sensitively (Adobe ships both
/// spellings — `Sony … Adobe Standard.dcp` and `Leica … Adobe_Standard.dcp`
/// are in the same installed pool), stop descending into subdirectories, or
/// ignore the budget.
#[test]
fn the_walk_is_recursive_case_insensitive_and_bounded() {
    let dir = std::env::temp_dir().join(format!("autoshade-adobe-walk-{}", std::process::id()));
    let deep = dir.join("vendor").join("nested");
    std::fs::create_dir_all(&deep).expect("nested tree");
    for (at, name) in
        [(&dir, "top.lcp"), (&deep, "deep.LCP"), (&deep, "other.dcp"), (&deep, "notes.txt")]
    {
        std::fs::write(at.join(name), b"x").expect("leaf file");
    }

    let found = walk_extension(vec![dir.clone()], "lcp", 1000);
    let mut names: Vec<String> =
        found.iter().filter_map(|p| Some(p.file_name()?.to_str()?.to_string())).collect();
    names.sort();
    assert_eq!(names, vec!["deep.LCP", "top.lcp"], "depth and case: {found:?}");

    // The other extension is reachable through the same walk, so the filter is
    // the ARGUMENT and not a hard-coded kind — which is the whole reason this
    // function was lifted out of the lens module.
    assert_eq!(walk_extension(vec![dir.clone()], "dcp", 1000).len(), 1, "the extension is an input");

    // A budget of zero returns nothing rather than walking anyway…
    assert!(walk_extension(vec![dir.clone()], "lcp", 0).is_empty(), "the budget is ignored");
    // …and a root that is not there at all is a quiet nothing, not a panic.
    assert!(walk_extension(vec![dir.join("gone")], "lcp", 10).is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}
