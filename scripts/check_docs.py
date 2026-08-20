#!/usr/bin/env python3
"""Doc-drift gate: every NUMBER the docs claim, re-derived from the tree.

The docs in this repo carry hard numbers — the shipped version, the test
counts a release battery produced, how many RAW/baked extensions the engine
accepts, which crates the stack is built from, which toolchain compiled it.
Those numbers were all true the day they were written, and nothing but a
human re-reading them keeps them true afterwards: `Cargo.toml` bumps, a
const grows an entry, a dependency lands, and the prose silently becomes a
confident lie. This is a GATE, not a report — run it before a release and it
exits non-zero when a doc and the tree disagree.

The core is a CLAIMS registry: each row names ONE claim site (doc + regex
with named capture groups) and ONE truth extractor (the source of record for
that number). Three outcomes per row:

  PASS  every occurrence of the pattern carries the truth's value
  FAIL  a value disagrees -> claim name, doc value, truth value, both sources
  FAIL  the pattern matches NOTHING -> the claim site moved (the doc was
        rewritten, the sentence rephrased). This is deliberately NOT a silent
        skip: a gate that quietly stops looking is worse than no gate, because
        it keeps reporting PASS over prose nobody is checking any more.
        Re-anchor the row on the new wording.
  SKIP  the truth is not derivable from the repo alone — today only the test
        counts, which are only provable by RUNNING the battery (pass its
        transcript with --gates).

A pattern may match several times (the same count is claimed in several
sections); ALL occurrences must agree with the truth, so a half-updated doc
fails.

Pure stdlib, no network, strictly READ-ONLY over the repo — it never rewrites
a file, and it never "fixes" a doc for you. The repo has MIXED line endings by
design (per-file `.gitattributes` + `core.autocrlf`), so every file is read as
utf-8 and normalised to LF *inside* the checker only.

    python scripts/check_docs.py
    python scripts/check_docs.py --gates <release-battery transcript>
    python scripts/check_docs.py --list
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import Callable, NamedTuple

REPO = Path(__file__).resolve().parents[1]

_CACHE: dict[Path, str] = {}


def text(rel: str) -> str:
    """Repo file as LF-normalised utf-8. Read-only; nothing is written back."""
    p = REPO / rel
    if p not in _CACHE:
        _CACHE[p] = (
            p.read_text(encoding="utf-8").replace("\r\n", "\n").replace("\r", "\n")
        )
    return _CACHE[p]


def line_of(src: str, idx: int) -> int:
    return src.count("\n", 0, idx) + 1


class Truth(NamedTuple):
    """A claim's source of record: the value, and where it was read from."""

    value: tuple[str, ...]
    source: str


class Skip(NamedTuple):
    reason: str


# ── Truth extractors ────────────────────────────────────────────────────────


def _literals_in_const(src: str, anchor: str) -> list[str]:
    """String literals inside `const <NAME>: [...] = [ ... ];`.

    The bracket scan starts at the `=`, NOT at the anchor: the declaration's
    TYPE carries its own bracket (`[&str; 24]`), so opening on the first `[`
    after the name would measure the type and extract nothing. The span is
    bracket-MATCHED rather than line-matched, so an initialiser that wraps
    across source lines (RAW_EXTS spans three) counts the same as a one-liner.
    """
    at = src.find(anchor)
    if at < 0:
        raise LookupError(f"anchor {anchor!r} not found (const renamed/moved?)")
    i = src.index("[", src.index("=", at))
    depth = 0
    out: list[str] = []
    while i < len(src):
        c = src[i]
        if src.startswith("//", i):
            i = src.index("\n", i)
            continue
        if c == '"':
            j = i + 1
            while src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            out.append(src[i + 1 : j])
            i = j + 1
            continue
        if c == "[":
            depth += 1
        elif c == "]":
            depth -= 1
            if depth == 0:
                return out
        i += 1
    raise LookupError(f"unclosed initialiser for {anchor!r}")


# Self-check (the font gate's notdef idiom): a broken scanner must fail HERE,
# never pass by extracting nothing — "0 extensions" would otherwise sail
# through as a number the docs simply never claim.
_PROBE = 'pub const P: [&str; 3] = [\n    "a", "b\\"x", // "not a literal"\n    "c",\n];'
assert _literals_in_const(_PROBE, "const P") == ["a", 'b\\"x', "c"], (
    "const-literal self-check: type bracket / escape / comment handling drifted"
)


def ext_count(rel: str, anchor: str) -> Callable[[argparse.Namespace], Truth]:
    def extract(_args: argparse.Namespace) -> Truth:
        src = text(rel)
        n = len(_literals_in_const(src, anchor))
        line = line_of(src, src.find(anchor))
        return Truth((str(n),), f"{rel}:{line} ({anchor})")

    return extract


def cargo_version(_args: argparse.Namespace) -> Truth:
    src = text("Cargo.toml")
    at = src.index("[package]")
    end = src.find("\n[", at)
    m = re.search(r'^version = "(\d+\.\d+\.\d+)"', src[at:end], re.M)
    if not m:
        raise LookupError("Cargo.toml: no [package] version = \"X.Y.Z\"")
    return Truth((m.group(1),), f"Cargo.toml:{line_of(src, at + m.start())}")


# ── Cargo dependency inventory (tech-stack completeness) ────────────────────

_DEP_SECTION = re.compile(
    r"^\[(?:target\.[^\]]+\.)?"
    r"(dependencies|dev-dependencies|build-dependencies)"
    r"(?:\.([A-Za-z0-9_-]+))?\]$"
)


def _strip_toml_comment(line: str) -> str:
    """Drop a trailing `# ...` comment. Cargo.toml's comments are prose full of
    brackets (`Vec<[f32;3]>`), which would wreck the depth counter below."""
    out: list[str] = []
    in_str = False
    for ch in line:
        if ch == '"':
            in_str = not in_str
        elif ch == "#" and not in_str:
            break
        out.append(ch)
    return "".join(out)


def cargo_dependency_names() -> list[str]:
    """Every DIRECT dependency name: [dependencies] (the optional gui crates
    live there behind `optional = true`), the per-target tables, build- and
    dev-dependencies.

    Only column-0 `name =` lines at bracket depth 0 count, so a multi-line
    inline table (`image = { ... features = [\\n "jpeg", ... \\n] }`) yields
    the crate and not its feature strings.
    """
    names: list[str] = []
    section: str | None = None
    depth = 0
    src = text("Cargo.toml")
    for raw in src.split("\n"):
        line = _strip_toml_comment(raw)
        if depth == 0:
            stripped = line.strip()
            m = _DEP_SECTION.match(stripped)
            if m:
                section = m.group(1)
                if m.group(2):  # [dependencies.foo] sub-table names the dep
                    names.append(m.group(2))
            elif stripped.startswith("["):
                section = None  # [package] / [lib] / [[bin]] / [features]
            elif section:
                k = re.match(r"([A-Za-z0-9_-]+)\s*=", line)
                if k:
                    names.append(k.group(1))
        depth = max(0, depth + sum(line.count(c) for c in "[{")
                    - sum(line.count(c) for c in "]}"))
    return sorted(set(names))


def check_tech_stack(_args: argparse.Namespace) -> tuple[str, list[str]]:
    """Every direct dependency must be NAMED somewhere in ARCHITECTURE.md.

    Not a value comparison but a coverage one: a crate that entered the tree
    without ever entering the architecture doc is a stack the doc no longer
    describes. Token match (word-ish boundaries over `[A-Za-z0-9_-]`), so
    `serde` inside `serde_json` does not vouch for `serde`.
    """
    names = cargo_dependency_names()
    if not names:
        return "FAIL", ["        Cargo.toml yielded NO dependency names — parser drift"]
    doc = text("docs/ARCHITECTURE.md")
    missing = [
        n
        for n in names
        if not re.search(
            r"(?<![A-Za-z0-9_-])" + re.escape(n) + r"(?![A-Za-z0-9_-])", doc
        )
    ]
    if missing:
        return "FAIL", [
            f"        {len(missing)} of {len(names)} direct deps never named in "
            "docs/ARCHITECTURE.md",
            "        missing  " + ", ".join(missing),
            "        truth    Cargo.toml (dependency tables)",
        ]
    return "PASS", [f"        all {len(names)} direct deps named"]


# ── Release-battery transcript (test counts) ────────────────────────────────

_BLOCK = re.compile(r"^=== (?P<name>.+?) ===\s*$")
_RESULT = re.compile(
    r"^test result: (?P<status>\w+)\. (?P<passed>\d+) passed; "
    r"(?P<failed>\d+) failed; (?P<ignored>\d+) ignored;"
)


def _battery_blocks(path: Path) -> dict[str, list[tuple[str, int, int, int]]]:
    blocks: dict[str, list[tuple[str, int, int, int]]] = {}
    cur: str | None = None
    raw = path.read_text(encoding="utf-8").replace("\r\n", "\n").replace("\r", "\n")
    for line in raw.split("\n"):
        b = _BLOCK.match(line)
        if b:
            cur = b.group("name")
            blocks.setdefault(cur, [])
            continue
        r = _RESULT.match(line)
        if r and cur is not None:
            blocks[cur].append(
                (
                    r.group("status"),
                    int(r.group("passed")),
                    int(r.group("failed")),
                    int(r.group("ignored")),
                )
            )
    return blocks


def battery_test_counts(args: argparse.Namespace) -> Truth | Skip:
    """library / CLI / GUI / contract counts from a release-battery transcript.

    Only the battery can prove these, so without --gates the row SKIPS rather
    than inventing a number. The mapping follows cargo's own target order in
    each block: lib unittests, then the `autoshop` bin, then (gui build only)
    the `autoshop-gui` bin, then the integration-test binaries. The GUI count
    is taken as the DIFFERENCE between the two configurations — that is what
    "GUI tests" means here (the tests the `gui` feature adds), and it survives
    a reordering of the targets that positional indexing would not.
    """
    if not args.gates:
        return Skip("no --gates transcript: test counts are only provable by "
                    "running the release battery")
    path = Path(args.gates)
    blocks = _battery_blocks(path)
    for want in ("test default", "test gui"):
        if want not in blocks:
            raise LookupError(f"{path.name}: no '=== {want} ===' block")
    # A red battery cannot vouch for anything.
    for name in ("test default", "test gui"):
        bad = [r for r in blocks[name] if r[0] != "ok"]
        if bad:
            raise LookupError(f"{path.name}: '{name}' has a non-ok result: {bad[0]}")

    def suites(name: str) -> list[int]:
        rows = blocks[name]
        # Drop trailing EMPTY suites (0 passed / 0 failed / 0 ignored) — cargo's
        # Doc-tests line for a crate with no doctests. An empty suite is not a
        # claimable count, and its presence depends on which targets the
        # battery invoked.
        while rows and rows[-1][1:] == (0, 0, 0):
            rows = rows[:-1]
        if len(rows) < 3:
            raise LookupError(f"{path.name}: '{name}' has only {len(rows)} suites")
        return [r[1] for r in rows]

    default, gui = suites("test default"), suites("test gui")
    lib, cli = default[0], default[1]
    gui_only = sum(gui) - sum(default)
    contract = default[2:]
    if gui_only <= 0:
        raise LookupError(f"{path.name}: gui build adds {gui_only} tests")
    if len(contract) != 2:
        raise LookupError(
            f"{path.name}: {len(contract)} contract suites, the doc claims a pair"
        )
    value = (str(lib), str(cli), str(gui_only), str(contract[0]), str(contract[1]))
    return Truth(value, f"{path.name} (=== test default === / === test gui ===)")


# ── Toolchain ───────────────────────────────────────────────────────────────

_TOOLCHAIN_PINS = ("rust-toolchain.toml", "rust-toolchain")


def toolchain_truth(_args: argparse.Namespace) -> Truth:
    """The toolchain the docs must agree with.

    This repo pins NO toolchain (no rust-toolchain / rust-toolchain.toml —
    `edition = "2024"` in Cargo.toml is an edition, not a compiler version), so
    there is no machine-checkable truth for "which rustc built this". The check
    is therefore DOC-INTERNAL: ARCHITECTURE §5's claim must agree with README's
    Tech line — the two places a reader looks. Compared at major.minor, since
    §5 carries the patch ("1.94.1") and README does not ("1.94"). If a pin ever
    lands in the repo, the branch below promotes it to the source of record
    automatically and the doc-internal fallback stops being used.
    """
    for rel in _TOOLCHAIN_PINS:
        p = REPO / rel
        if p.exists():
            src = text(rel)
            m = re.search(r'channel\s*=\s*"([^"]+)"', src) or re.search(
                r"^\s*(\d+\.\d+(?:\.\d+)?)\s*$", src, re.M
            )
            if not m:
                raise LookupError(f"{rel}: no channel/version found")
            return Truth((m.group(1),), f"{rel}:{line_of(src, m.start())}")
    src = text("README.md")
    m = re.search(r"rustc/cargo (?P<toolchain>\d+(?:\.\d+)+)", src)
    if not m:
        raise LookupError("README.md: Tech line no longer states rustc/cargo X.Y")
    return Truth((m.group("toolchain"),), f"README.md:{line_of(src, m.start())} "
                 "(doc-internal: repo pins no toolchain)")


def major_minor(v: tuple[str, ...]) -> tuple[str, ...]:
    return tuple(".".join(x.split(".")[:2]) for x in v)


# ── The CLAIMS registry ─────────────────────────────────────────────────────


class Claim(NamedTuple):
    doc: str
    name: str
    pattern: str
    truth: Callable[[argparse.Namespace], Truth | Skip]
    norm: Callable[[tuple[str, ...]], tuple[str, ...]] = lambda v: v


class SetClaim(NamedTuple):
    """A coverage claim (a SET must be fully named), not a value claim."""

    doc: str
    name: str
    check: Callable[[argparse.Namespace], tuple[str, list[str]]]

    @property
    def pattern(self) -> str:
        return "(set-coverage check, no pattern)"


ARCH = "docs/ARCHITECTURE.md"
README = "README.md"

CLAIMS: list[Claim | SetClaim] = [
    Claim(
        ARCH,
        "shipped version",
        r"Status: \*\*implemented\*\* \(v(?P<version>\d+\.\d+\.\d+)",
        cargo_version,
    ),
    Claim(
        ARCH,
        "test counts (lib/CLI/GUI/contract)",
        r"(?P<lib>\d+) library \+ (?P<cli>\d+) CLI \+ (?P<gui>\d+) GUI "
        r"\+ (?P<c1>\d+)\+(?P<c2>\d+) contract tests",
        battery_test_counts,
    ),
    Claim(
        ARCH,
        "RAW ext count — §4 milestone table",
        r"RAW decode \+ features \((?P<formats>\d+) formats",
        ext_count("src/decode.rs", "const RAW_EXTS"),
    ),
    Claim(
        ARCH,
        "RAW ext count — §4.1 two-lists note",
        r"`decode::RAW_EXTS` \((?P<formats>\d+)\)",
        ext_count("src/decode.rs", "const RAW_EXTS"),
    ),
    Claim(
        ARCH,
        "RAW ext count — §4.1 no-preview row",
        r"of the (?P<formats>\d+) formats",
        ext_count("src/decode.rs", "const RAW_EXTS"),
    ),
    Claim(
        ARCH,
        "RAW ext count — §6 open question 2",
        r"(?P<formats>\d+) RAW extensions \(`decode::RAW_EXTS`\)",
        ext_count("src/decode.rs", "const RAW_EXTS"),
    ),
    Claim(
        README,
        "RAW ext count — Tech line",
        r"RAW decode, (?P<formats>\d+) formats",
        ext_count("src/decode.rs", "const RAW_EXTS"),
    ),
    Claim(
        ARCH,
        "baked ext count — §4.1 two-lists note",
        r"`pipeline::BAKED_EXTS` \((?P<baked>\d+)\)",
        ext_count("src/pipeline.rs", "const BAKED_EXTS"),
    ),
    Claim(
        ARCH,
        "baked ext count — §6 open question 2",
        r"\+ (?P<baked>\d+) baked \(`pipeline::BAKED_EXTS`\)",
        ext_count("src/pipeline.rs", "const BAKED_EXTS"),
    ),
    SetClaim(ARCH, "tech stack names every direct dependency", check_tech_stack),
    Claim(
        ARCH,
        "toolchain — §5 vs README Tech line",
        r"rustc/cargo \*\*(?P<toolchain>\d+(?:\.\d+)+)\*\*",
        toolchain_truth,
        major_minor,
    ),
]


# ── Runner ──────────────────────────────────────────────────────────────────


def show(v: tuple[str, ...]) -> str:
    return " + ".join(v)


def run_claim(row: Claim, args: argparse.Namespace) -> tuple[str, list[str]]:
    doc = text(row.doc)
    hits = list(re.finditer(row.pattern, doc))
    if not hits:
        # NOT a skip: see the module docstring. A pattern that stopped matching
        # is a claim site that moved, and the gate must be re-anchored on the
        # new wording before it can vouch for anything again.
        return "FAIL", [
            f"        pattern matches NOTHING in {row.doc} — claim site moved;"
            " re-anchor the gate",
            f"        pattern  {row.pattern}",
        ]
    try:
        t = row.truth(args)
    except LookupError as e:
        return "FAIL", [f"        truth extractor failed: {e}"]
    if isinstance(t, Skip):
        return "SKIP", [f"        {t.reason}"]
    want = row.norm(t.value)
    bad = [(m, row.norm(tuple(m.groups()))) for m in hits]
    bad = [(m, got) for m, got in bad if got != want]
    if bad:
        out = [f"        truth    {show(t.value)}   ({t.source})"]
        for m, got in bad:
            out.append(
                f"        doc      {show(tuple(m.groups()))}   "
                f"({row.doc}:{line_of(doc, m.start())})"
            )
        return "FAIL", out
    where = ", ".join(f"{row.doc}:{line_of(doc, m.start())}" for m in hits)
    return "PASS", [f"        {show(t.value)}   ({where})"]


def main() -> int:
    # Windows consoles default to the ANSI codepage; doc values quoted back can
    # carry the section signs and CJK these docs mix in.
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument(
        "--gates",
        metavar="FILE",
        help="release-battery transcript (=== test default === / === test gui "
             "=== blocks). Without it the test-count row SKIPS.",
    )
    ap.add_argument(
        "--list", action="store_true", help="print the claims registry and exit"
    )
    ap.add_argument(
        "-v", "--verbose", action="store_true",
        help="print the detail line for PASS rows too",
    )
    args = ap.parse_args()

    if args.list:
        print(f"{len(CLAIMS)} claims:")
        for row in CLAIMS:
            print(f"  {row.doc:<22} {row.name}")
            print(f"  {'':<22} {row.pattern}")
        return 0

    print(f"doc-drift gate over {REPO.name}/ — "
          f"gates transcript: {args.gates or '(none, test counts will SKIP)'}\n")
    tally = {"PASS": 0, "FAIL": 0, "SKIP": 0}
    for row in CLAIMS:
        if isinstance(row, SetClaim):
            try:
                status, detail = row.check(args)
            except LookupError as e:
                status, detail = "FAIL", [f"        check failed: {e}"]
        else:
            status, detail = run_claim(row, args)
        tally[status] += 1
        print(f"{status}  {row.name}")
        if status != "PASS" or args.verbose:
            for line in detail:
                print(line)
    print(f"\n{len(CLAIMS)} claims: {tally['PASS']} PASS, {tally['FAIL']} FAIL, "
          f"{tally['SKIP']} SKIP")
    return 1 if tally["FAIL"] else 0


if __name__ == "__main__":
    sys.exit(main())
