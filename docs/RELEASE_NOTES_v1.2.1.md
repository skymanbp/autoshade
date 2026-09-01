# AutoShade v1.2.1 — a migration that could never have succeeded

v1.2.0 was declared final. This release exists because one of its migrations
was not merely broken but *unfixable by retrying*, and the retry is exactly
what it promised the user on every launch.

Nothing about the renderer, the reverse fit, the style retrieval or the sidecar
contracts changes here. If you are on v1.2.0 and never saw the warning below,
this release changes nothing you can observe.

## The defect

Every Windows machine upgrading from a pre-rename build printed this at every
launch of the desktop app:

```
warning: could not move your preferences from Autoshop to AutoShade.
Still using the old folder, so nothing is lost; the next launch tries again.
```

The disclosure was accurate about the safe part — no preference was ever lost,
and the app kept reading and writing the old folder — and wrong about the
hopeful part. The next launch did try again, and failed identically, forever.

## Root cause

`eframe::storage_dir` is **two levels deep** on Windows. From
`eframe-0.29.1/src/native/file_storage.rs:53`:

```rust
OS::Windows => roaming_appdata().map(|p| p.join(app_id).join("data")),
```

So the two directories `adopt_prefs_between` was asked to rename were
`%APPDATA%\Autoshop\data` and `%APPDATA%\AutoShade\data` — and the
destination's *parent*, `%APPDATA%\AutoShade`, does not exist on a machine that
has never run the new name. `MoveFileEx` answers `ERROR_PATH_NOT_FOUND`, the
function reports `FellBack`, and the next launch reproduces the same state.

The fix is to make that parent before renaming.

## Why nothing caught it

The test covering this function built its two directories as **siblings under
one existing base** — `base/Autoshop` and `base/AutoShade`, one level deep — a
shape Windows never produces. Worse, its fourth arm explicitly asserted that a
missing destination parent *should* fall back:

```rust
// A refused rename (destination parent missing) falls back to the
// legacy key for the session instead of resetting anyone's prefs.
let orphan = base.join("missing-parent").join("AutoShade");
assert_eq!(adopt_prefs_between(&orphan, &legacy, true), PrefsAdoption::FellBack);
```

That is the production failure, written down as the desired outcome. The test
was green on every platform for the whole life of the rename and could not have
gone red.

This is the same class as the two defects v1.2.0 itself shipped a fix for: a
code path whose test models something other than what production runs. It is
now three, and the shared lesson is the one already in `ARCHITECTURE.md` —
when a decision is injected as a parameter, make the battery pass the values
production passes, not the values that are convenient to construct.

## What changed

- `src/bin/gui/main.rs` — `adopt_prefs_between` creates the destination's
  parent before renaming, and falls back if that itself fails. The two sibling
  adoptions (`store::adopt_pre_rename_root`,
  `serve::adopted_export_registry_root`) were checked and need no change: both
  rename *within* a directory that already exists, which is why this was one
  defect and not three.
- `src/bin/gui/tests.rs` — the test now drives the real
  `<roaming>/<key>/data` shape with the destination's parent deliberately
  absent, and asserts that absence before the move. Its unrescuable arm is now
  a regular file sitting where the parent directory would go, so
  `create_dir_all` genuinely fails and `FellBack` still has a real case.

Falsifiability was checked both ways rather than assumed:

| Mutation | Expected | Result |
|---|---|---|
| fix removed, new test kept | red | red (exit 101) |
| fix removed, test reverted to the old sibling shape | green | green |

The second row is the point: it demonstrates first-hand that the old test could
not have caught the shipped defect.

## Upgrading

Install over v1.2.0; the installer upgrades in place. On first launch of the
desktop app the preferences folder moves and prints:

```
note: your window/library preferences moved from Autoshop to AutoShade
(the app was renamed). Nothing was copied or duplicated.
```

If both folders somehow exist, the new one is used and the old one is left
untouched — that arm is unchanged. Develops, style indexes, and the
`%LOCALAPPDATA%\autoshade` store are not involved: this is the eframe window
and view preferences file only.

## Verification

The gates that ran for v1.2.0 ran again unchanged. The site's showcase was
consolidated in the same window — twelve figures to seven, with the five
demoted ones kept at full length in `docs/SHOWCASE.md`, which the site now
links to for the first time — but that is presentation, not behaviour.
