#!/usr/bin/env python3
"""Doc-drift gate: what the living documents name must exist in the tree.

The 2026-10-02 sweep after v1.7.0 found nine facts that had drifted from the
code — a renamed function still named, a tier count one short, a model credited
with a job it never did, a stage called "last" after another was added. Each
was found by a scripted scan; this script is those scans, run on every push
(`.github/workflows/build.yml`) and inside the release battery
(`scripts/release_battery.sh`), so the next drift fails a build instead of
waiting for a reader.

Checks, each over the LIVING documents only (released notes, the archived
roadmap and the ledger's dated entries are records of their day and are not
rewritten):

1. paths    — every repository path a document names (`src/…`, `python/…`,
               `scripts/…`, `docs/…`, `site/…`, `.github/…`, `assets/…`,
               `tests/…`, `installer/…`) exists, and a `path:line` stays
               inside the file;
2. symbols  — every code symbol in backticks (`a::b`, `snake_case()`,
               `snake_case`, `CamelCase`) occurs as a word in the tracked
               sources; names a document keeps on purpose as the record of a
               rename or retirement are listed in HISTORY with their reason;
3. links    — every relative link reaches a tracked file and every `#anchor`
               names a heading of its target (GitHub's slug rule);
4. tiers    — the `RenderedNotExported` row of ARCHITECTURE's tier table states
               the number of such globals in `RECIPE_CONTROLS` and names each
               of them and each such local in `LOCAL_CONTROLS`.

The manual's CLI synopsis is checked flag for flag against the binary's own
`--help` by the CLI test `the_manuals_cli_synopsis_matches_every_commands_help`
(tests/cli.rs), which needs the built binary and so lives with the CLI tests.

Exit 0 when clean; 1 with one line per finding otherwise.
usage: python scripts/check_doc_drift.py [--root DIR]
"""
from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

LIVE_MD = [
    "README.md", "CONTRIBUTING.md", ".github/SECURITY.md", ".github/PULL_REQUEST_TEMPLATE.md",
    "docs/USER_MANUAL.md", "docs/ARCHITECTURE.md", "docs/TECH_STACK.md", "docs/SHOWCASE.md",
    "site/README.md", "assets/fonts/README.md",
]
LIVE_HTML = ["site/index.html", "site/architecture.html", "site/404.html"]
# ROADMAP is the dated ledger: its links are checked, but its line-number anchors
# (`#L392`) point into the file as it was on that entry's day.
LEDGER = ["docs/ROADMAP.md"]

# Names a living document keeps as the RECORD of a rename or retirement — the
# sentence around each says so. Every entry carries the document and why.
HISTORY = {
    "a_develop_commit_lands_all_three_or_nothing": "ARCHITECTURE: a v1.3.3 count clause records it rewritten",
    "gui_reverse_fit_uses_the_panel_strength": "ARCHITECTURE: a v1.3.2 count clause records it rewritten",
    "bitmap_masks_do_not_come_back_from_xmp": "ARCHITECTURE: a count clause records its rename",
    "a_divergent_zone_is_still_attached_in_atmosphere_mode": "ARCHITECTURE: a count clause records its rename",
    "cast_curves_must_not_fan_a_coherent_sky_across_luminance": "ARCHITECTURE: a count clause records its replacement",
    "a_rim_that_cannot_be_shrunk_is_dropped_with_its_own_note": "ARCHITECTURE: a count clause records its rename",
    "legacy_autoshop_sidecar_": "ARCHITECTURE: a count clause records a renamed test prefix",
}

REPO_PATH = re.compile(
    r"(?<![\w./-])((?:src|python|scripts|docs|site|\.github|assets|tests|installer)/[\w./-]*[\w/])(?::(\d+))?")
TICK = re.compile(r"`([^`\n]{2,120})`")
IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def tracked(root: Path) -> list[str]:
    out = subprocess.run(["git", "ls-files"], cwd=root, capture_output=True, text=True, encoding="utf-8",
                         check=True).stdout
    return [p for p in out.split("\n") if p]


def read(root: Path, rel: str) -> str:
    return (root / rel).read_text(encoding="utf-8", errors="replace")


def ignored(root: Path, paths: list[str]) -> set[str]:
    """The paths .gitignore covers: a document may name a local-only place
    (the weights directory, a planning memo kept out of the public tree) as
    long as the repository itself says it is kept out."""
    if not paths:
        return set()
    # NUL-separated bytes: a text-mode pipe on Windows writes CRLF, and git
    # then asks about `python/weights\r`, which nothing ignores. A directory
    # pattern (`.gstack/`) matches only the spelling with the slash, so each
    # path is asked about both ways.
    asked = [q for p in paths for q in (p, p + "/")]
    out = subprocess.run(["git", "check-ignore", "--no-index", "-z", "--stdin"], cwd=root,
                         input="\0".join(asked).encode("utf-8"), capture_output=True).stdout
    return {p.rstrip("/") for p in out.decode("utf-8").split("\0") if p}


def check_paths(root: Path, files: set[str], dirs: set[str]) -> list[str]:
    bad, absent = [], []
    for doc in LIVE_MD + LIVE_HTML:
        for n, line in enumerate(read(root, doc).splitlines(), 1):
            for m in REPO_PATH.finditer(line):
                p, ln = m.group(1).rstrip("/."), m.group(2)
                if "*" in p or "<" in p or "{" in p:
                    continue
                if not (p in files or p in dirs):
                    # a glob stem such as `python/requirements-*.txt` ends at the dash
                    if not any(f.startswith(p) for f in files):
                        absent.append((doc, n, p))
                    continue
                if ln and p in files:
                    total = len(read(root, p).splitlines())
                    if int(ln) > total:
                        bad.append(f"paths: {doc}:{n} cites `{p}:{ln}`, past the file's {total} lines")
    kept_out = ignored(root, sorted({p for _, _, p in absent}))
    bad += [f"paths: {doc}:{n} names `{p}`, which is neither in the tree nor kept out by .gitignore"
            for doc, n, p in absent if p not in kept_out]
    return bad


def symbol_candidates(span: str) -> list[str]:
    s = span.strip()
    if "::" in s:
        return [t for t in re.split(r"::|[^A-Za-z0-9_:]", s) if t and IDENT.fullmatch(t)]
    if re.fullmatch(r"[a-z_][a-z0-9_]*\(\)?.*", s) and "_" in s.split("(")[0]:
        return [s.split("(")[0]]
    if re.fullmatch(r"[a-z][a-z0-9]*(_[a-z0-9]+)+", s):
        return [s]
    if re.fullmatch(r"[A-Z][a-z0-9]+([A-Z][a-z0-9]*)+", s):
        return [s]
    return []


def check_symbols(root: Path, tracked_files: list[str]) -> list[str]:
    src = [p for p in tracked_files
           if re.match(r"(src/|python/|scripts/|\.github/|installer/|build\.rs$|Cargo\.toml$)", p)
           and p.endswith((".rs", ".py", ".yml", ".yaml", ".iss", ".toml", ".sh", ".ps1", ".js", ".html", ".json"))]
    words = set(IDENT.findall("\n".join(read(root, p) for p in src)))
    bad = []
    for doc in LIVE_MD + ["site/index.html"]:
        for n, line in enumerate(read(root, doc).splitlines(), 1):
            for m in TICK.finditer(line):
                for c in symbol_candidates(m.group(1)):
                    if len(c) > 3 and c not in words and c not in HISTORY:
                        bad.append(f"symbols: {doc}:{n} names `{c}`, which no tracked source defines or uses")
    return bad


def slugs(md: str) -> set[str]:
    out: set[str] = set()
    seen: dict[str, int] = {}
    fence = False
    for line in md.splitlines():
        if line.startswith("```"):
            fence = not fence
        if fence:
            continue
        m = re.match(r"#{1,6}\s+(.*?)\s*#*$", line)
        if not m:
            continue
        t = re.sub(r"<[^>]+>", "", m.group(1))
        t = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", t).replace("`", "")
        s = re.sub(r"[^\w\- ]", "", t.lower()).replace(" ", "-")
        k = seen.get(s, 0)
        out.add(s if k == 0 else f"{s}-{k}")
        seen[s] = k + 1
    out.update(re.findall(r'<a\s+(?:name|id)="([^"]+)"', md))
    return out


def check_links(root: Path) -> list[str]:
    bad, cache = [], {}
    for doc in LIVE_MD + LIVE_HTML + LEDGER:
        text = read(root, doc)
        for link in re.findall(r"\]\(([^)\s]+)\)", text) + re.findall(r'href="([^"]+)"', text):
            if "github.com/skymanbp/autoshade/blob/main/" in link:
                link = "/" + link.split("/blob/main/", 1)[1]
            elif re.match(r"(https?:|mailto:|data:|/)", link):
                continue
            path, _, anchor = link.partition("#")
            path = path.split("?")[0]
            if link.startswith("/"):
                target = root / path.lstrip("/")
            else:
                target = (root / doc).parent / path if path else root / doc
            target = target.resolve()
            if not target.exists():
                bad.append(f"links: {doc} links `{link}`, which does not exist")
                continue
            if not anchor or target.suffix != ".md":
                continue
            if doc in LEDGER and re.fullmatch(r"L\d+(-L\d+)?", anchor):
                continue
            if target not in cache:
                cache[target] = slugs(target.read_text(encoding="utf-8"))
            if anchor not in cache[target]:
                bad.append(f"links: {doc} links `{link}`, but its target has no such heading")
    return bad


def tier_members(catalogue: str, array: str) -> list[str]:
    m = re.search(r"pub const " + array + r": \[Control; \d+\] = \[(.*?)\n\];", catalogue, re.S)
    if not m:
        raise SystemExit(f"tiers: `{array}` not found in src/advisor/catalogue.rs")
    names = []
    for row in m.group(1).split("Control {")[1:]:
        if "tier: Some(Tier::RenderedNotExported)" in row:
            names.append(re.search(r'name: "([^"]+)"', row).group(1))
    return names


def check_tiers(root: Path) -> list[str]:
    cat = read(root, "src/advisor/catalogue.rs")
    glob, loc = tier_members(cat, "RECIPE_CONTROLS"), tier_members(cat, "LOCAL_CONTROLS")
    row = next((l for l in read(root, "docs/ARCHITECTURE.md").splitlines()
                if l.startswith("| `RenderedNotExported` |")), None)
    if row is None:
        return ["tiers: docs/ARCHITECTURE.md has no `RenderedNotExported` row in its tier table"]
    bad = []
    m = re.search(r"\*\*(\d+) global\*\*", row)
    if not m or int(m.group(1)) != len(glob):
        bad.append(f"tiers: ARCHITECTURE's RenderedNotExported row states {m.group(1) if m else 'no'} globals; "
                   f"RECIPE_CONTROLS has {len(glob)} ({', '.join(glob)})")
    for name in glob + loc:
        if f"`{name}`" not in row:
            bad.append(f"tiers: ARCHITECTURE's RenderedNotExported row does not name `{name}`")
    return bad


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--root", default=str(Path(__file__).resolve().parent.parent))
    root = Path(ap.parse_args(argv).root)
    files_list = tracked(root)
    files = set(files_list)
    dirs: set[str] = set()
    for p in files:
        d = p
        while "/" in d:
            d = d.rsplit("/", 1)[0]
            dirs.add(d)
    findings = (check_paths(root, files, dirs) + check_symbols(root, files_list)
                + check_links(root) + check_tiers(root))
    for f in findings:
        print(f)
    print(f"doc drift: {len(findings)} finding(s) over {len(LIVE_MD) + len(LIVE_HTML) + len(LEDGER)} documents")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
