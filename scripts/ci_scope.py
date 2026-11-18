#!/usr/bin/env python3
"""Whether a run on the online runners may set the LONG tests aside (v1.18.2).

Owner's ruling, 2026-11-18: run what was touched. A release that changed the desktop window
and the documents waited two rounds of a hundred minutes and a release run of over two hours
for the reverse fit's toy fits, whose every input was the previous release's.

The long tests are the library tests of the modules in `LONG` (about 130 of 2003 ran over a
minute on the v1.18.1 runners, all but four of them here). They are set aside when, and only
when, nothing they can read has changed since a commit on which the `build` workflow ended
green:

  * the base is the nearest commit, this one included, among the first-parent ancestors that
    has a successful `build` run. A green run that set them aside counts: it stands on its own
    base by the same rule, so the chain ends on a run that ran them;
  * every path that differs from the base is NEUTRAL (the desktop window's sources, the
    documents and the site, the workflows, the scripts, the installer script), or is
    Cargo.toml / Cargo.lock differing in this package's version line alone;
  * no source file of a `LONG` module reads a neutral path (`include_str!` and the like). The
    one module that does — `fit_zoned::semantic`, whose test reads a GUI source — is `KEPT`.

Anything else, and anything that cannot be determined (no base within `DEPTH` commits, git or
the API failing), runs the whole suite. Every other library test, the command line's, the
contract tests, the doc tests and the GUI's run on every push whatever this says.

usage: ci_scope.py                     prints `skip_long=0|1` (for $GITHUB_OUTPUT); the reasons go to stderr
       ci_scope.py --filter            stdin: test names, one a line; stdout: those that are not long
environment: CI_FORCE_FULL=true runs everything; GITHUB_REPOSITORY and GH_TOKEN for the API.
"""
import os
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEPTH = 40

# Module paths whose tests are long, and the ones among them that always run.
LONG = ("fit", "fit_zoned", "denoise::linear_chroma")
KEPT = ("fit_zoned::semantic",)

NEUTRAL_DIRS = ("src/bin/gui/", "site/", ".github/", "scripts/", "installer/")
NEUTRAL_FILES = ("README.md", "CONTRIBUTING.md", "LICENSE")
VERSIONED = ("Cargo.toml", "Cargo.lock")


def is_long(name):
    """A library test's name -> whether it is one of the long tests."""
    def under(mods):
        return any(name.startswith(m + "::") for m in mods)
    return under(LONG) and not under(KEPT)


def neutral(path):
    """A repository path -> whether no long test can read it."""
    if path.startswith("docs/"):
        return not path.startswith("docs/images/")   # the fit's figure tests read the pictures
    return path.startswith(NEUTRAL_DIRS) or path in NEUTRAL_FILES


def same_but_version(name, old, new):
    """Cargo.toml / Cargo.lock: whether two texts differ in this package's version line alone."""
    if name == "Cargo.toml":
        pat = re.compile(r'(\A(?:.*\n)*?\[package\]\n(?:(?!\[).*\n)*?version = ")[^"\n]*(")')
    else:
        pat = re.compile(r'(\n\[\[package\]\]\nname = "autoshade"\nversion = ")[^"\n]*(")')
    old, new = old.replace("\r\n", "\n"), new.replace("\r\n", "\n")
    if len(pat.findall(old)) != 1 or len(pat.findall(new)) != 1:
        return False
    return pat.sub(r"\1*\2", old) == pat.sub(r"\1*\2", new)


def long_sources(root):
    """The source files the long tests live in, as repository paths."""
    out = []
    for mod in LONG:
        stem = "src/" + mod.replace("::", "/")
        files = [stem + ".rs"] + sorted(p.relative_to(root).as_posix() for p in (root / stem).rglob("*.rs"))
        for f in files:
            kept = any(f == "src/" + k.replace("::", "/") + ".rs" or f.startswith("src/" + k.replace("::", "/") + "/")
                       for k in KEPT)
            if not kept and (root / f).is_file():
                out.append(f)
    return out


_INCLUDE = re.compile(r'include_(?:str|bytes)!\(\s*"([^"]+)"\s*\)')
_MANIFEST = re.compile(r'CARGO_MANIFEST_DIR"\)\s*,\s*"/([^"]+)"')
_LITERAL = re.compile(r'"((?:docs/(?!images)|site/|scripts/|\.github/|installer/|src/bin/gui/|bin/gui/)[^"]*)"')


def neutral_reads(path, text):
    """What one source file of a long module reads from the neutral paths (must be nothing)."""
    here = os.path.dirname(path)
    hits = []
    for rel in _INCLUDE.findall(text):
        target = os.path.normpath(os.path.join(here, rel)).replace(os.sep, "/")
        if neutral(target):
            hits.append(f"{path}: include of {target}")
    for target in _MANIFEST.findall(text):
        if neutral(target):
            hits.append(f"{path}: reads {target}")
    for lit in _LITERAL.findall(text):
        hits.append(f"{path}: names {lit}")
    return hits


def base_commit(ancestors, has_green_run):
    """The nearest of `ancestors` (HEAD first) with a green build run, or None."""
    for sha in ancestors:
        if has_green_run(sha):
            return sha
    return None


def decide(changed, versions_only, reads, forced=False, base="?"):
    """-> (skip_long, reason). `changed`: paths differing from the base; `versions_only`: {name: bool}."""
    if forced:
        return False, "the whole suite was asked for"
    if base is None:
        return False, f"no commit with a green build run among the last {DEPTH}"
    if reads:
        return False, "a long test's source reads a neutral path: " + "; ".join(reads[:3])
    for path in changed:
        if path in VERSIONED:
            if not versions_only.get(path, False):
                return False, f"{path} differs from {base[:7]} in more than this package's version"
        elif not neutral(path):
            return False, f"{path} differs from {base[:7]}"
    return True, f"nothing the long tests read differs from {base[:7]} ({len(changed)} neutral paths do)"


def _git(*args):
    return subprocess.run(["git", *args], cwd=REPO, capture_output=True, text=True, encoding="utf-8",
                          errors="replace", check=True).stdout


def _has_green_run(sha):
    repo = os.environ["GITHUB_REPOSITORY"]
    out = subprocess.run(
        ["gh", "api", f"repos/{repo}/actions/workflows/build.yml/runs?head_sha={sha}&status=success&per_page=1",
         "--jq", ".total_count"], capture_output=True, text=True, check=True).stdout.strip()
    return int(out) > 0


def main():
    if "--filter" in sys.argv:
        for line in sys.stdin:
            if line.strip() and not is_long(line.strip()):
                sys.stdout.write(line.strip() + "\n")
        return
    try:
        forced = os.environ.get("CI_FORCE_FULL", "").lower() == "true"
        base, changed, versions, reads = "?", [], {}, []
        if not forced:
            base = base_commit(_git("rev-list", "--first-parent", "-n", str(DEPTH), "HEAD").split(), _has_green_run)
        if not forced and base:
            # --no-renames: a library file moved under a neutral folder must show as a file that is gone
            changed = [p for p in _git("diff", "--name-only", "--no-renames", base, "HEAD").split("\n") if p]
            for name in VERSIONED:
                if name in changed:
                    versions[name] = same_but_version(name, _git("show", f"{base}:{name}"), _git("show", f"HEAD:{name}"))
            for f in long_sources(REPO):
                reads += neutral_reads(f, (REPO / f).read_text(encoding="utf-8"))
        skip, why = decide(changed, versions, reads, forced, base)
    except Exception as e:
        # Whatever went wrong (git, the API, a file that will not read), the answer is the whole
        # suite: a scope that cannot be determined must never be the reason a test did not run.
        skip, why = False, f"the scope could not be determined ({type(e).__name__}: {e})"
    verdict = "the long tests are set aside" if skip else "the whole suite runs"
    print(f"scope: {verdict} — {why}", file=sys.stderr)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as f:
            f.write(f"**Test scope:** {verdict} — {why}\n")
    print(f"skip_long={1 if skip else 0}")


if __name__ == "__main__":
    main()
