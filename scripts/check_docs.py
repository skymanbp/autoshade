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
import os
import re
import sys
import xml.etree.ElementTree as ET
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


_ARRAY_LEN = re.compile(r"\[\s*&\s*(?:'[A-Za-z_]\w*\s+)?str\s*;\s*(\d+)\s*\]")


def _literals_in_const(src: str, anchor: str) -> list[str]:
    """String literals inside `const <NAME>: [&str; N] = [ ... ];`.

    The bracket scan starts at the `=`, NOT at the anchor: the declaration's
    TYPE carries its own bracket (`[&str; 24]`), so opening on the first `[`
    after the name would measure the type and extract nothing. The span is
    bracket-MATCHED rather than line-matched, so an initialiser that wraps
    across source lines (RAW_EXTS spans three) counts the same as a one-liner.

    Both Rust comment forms are skipped. `//` always was; `/* */` was NOT until
    R28 2c, so a literal commented out with a block comment counted as live and
    the docs were checked against a number the compiler never saw. (Nesting is
    not modelled — Rust's block comments nest, this stops at the first `*/`.
    The residue is then ordinary text to this scanner, so the failure direction
    is a wrong count, which the length cross-check below catches.)

    The count is CROSS-CHECKED against the declared `[&str; N]`, which the
    compiler enforces and this scanner does not: any drift between what the
    lexer sees and what the type says is a broken lexer, and a broken lexer
    that returns a plausible number is exactly what a doc gate must never do.
    """
    at = src.find(anchor)
    if at < 0:
        raise LookupError(f"anchor {anchor!r} not found (const renamed/moved?)")
    i = src.index("[", src.index("=", at))
    declared = _ARRAY_LEN.search(src[at:i])
    if not declared:
        raise LookupError(
            f"{anchor}: no `[&str; N]` type annotation — not a fixed-size array any more"
        )
    depth = 0
    out: list[str] = []
    while i < len(src):
        c = src[i]
        if src.startswith("//", i):
            i = src.index("\n", i)
            continue
        if src.startswith("/*", i):
            shut = src.find("*/", i + 2)
            if shut < 0:
                raise LookupError(f"unterminated block comment in {anchor!r}")
            i = shut + 2
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
                n = int(declared.group(1))
                if len(out) != n:
                    raise LookupError(
                        f"{anchor}: extracted {len(out)} literal(s) but the type "
                        f"declares [&str; {n}] — the scanner and the compiler "
                        "disagree; fix the scanner, not the doc"
                    )
                return out
        i += 1
    raise LookupError(f"unclosed initialiser for {anchor!r}")


# Self-check (the font gate's notdef idiom): a broken scanner must fail HERE,
# never pass by extracting nothing — "0 extensions" would otherwise sail
# through as a number the docs simply never claim.
_PROBE = (
    'pub const P: [&str; 3] = [\n'
    '    "a", "b\\"x", // "not a literal"\n'
    '    /* "nor is this" */ "c",\n'
    "];"
)
assert _literals_in_const(_PROBE, "const P") == ["a", 'b\\"x', "c"], (
    "const-literal self-check: type bracket / escape / comment handling drifted"
)


def _raises_lookup(fn: Callable[[], object]) -> bool:
    """Did `fn` reject its input? The self-checks below assert the FAILING
    direction of a guard, and a guard only ever exercised on input it accepts
    cannot be told apart from a deleted one (R28 2c). LookupError is caught
    rather than swallowed because it is precisely the verdict under test — any
    other exception is a real defect and propagates.
    """
    try:
        fn()
    except LookupError:
        return True
    return False


assert _raises_lookup(lambda: _literals_in_const('pub const Q: [&str; 4] = ["a", "b"];', "const Q")), (
    "const-literal self-check: a count that disagrees with [&str; N] was accepted"
)


def ext_count(rel: str, anchor: str) -> Callable[[argparse.Namespace], Truth]:
    def extract(_args: argparse.Namespace) -> Truth:
        src = text(rel)
        n = len(_literals_in_const(src, anchor))
        line = line_of(src, src.find(anchor))
        return Truth((str(n),), f"{rel}:{line} ({anchor})")

    return extract


def _set_verdict(
    got: list[str],
    want: list[str],
    got_source: str,
    truth_source: str,
) -> tuple[str, list[str]]:
    """Compare named memberships, with duplicate detection and useful deltas."""
    got_norm = [v.lower().lstrip(".") for v in got]
    want_norm = [v.lower().lstrip(".") for v in want]
    got_set, want_set = set(got_norm), set(want_norm)
    duplicates = sorted({v for v in got_norm if got_norm.count(v) > 1})
    missing = sorted(want_set - got_set)
    extra = sorted(got_set - want_set)
    if duplicates or missing or extra:
        detail = [
            f"        doc      {len(got_norm)} item(s) from {got_source}",
            f"        truth    {len(want_set)} member(s) from {truth_source}",
        ]
        if missing:
            detail.append("        missing  " + ", ".join(missing))
        if extra:
            detail.append("        extra    " + ", ".join(extra))
        if duplicates:
            detail.append("        duplicate " + ", ".join(duplicates))
        return "FAIL", detail
    return "PASS", [f"        exact {len(want_set)}-member set"]


def check_readme_raw_ext_membership(
    _args: argparse.Namespace,
) -> tuple[str, list[str]]:
    """README's fenced RAW list must name exactly `decode::RAW_EXTS`."""
    doc = text("README.md")
    m = re.search(
        r"\*\*Camera RAW — \d+ extensions\*\*.*?```text\n(?P<items>.*?)\n```",
        doc,
        re.S,
    )
    if not m:
        raise LookupError("README Camera RAW fenced list moved; re-anchor the set gate")
    got = [v.strip() for v in m.group("items").replace("\n", " ").split(",") if v.strip()]
    source = text("src/decode.rs")
    want = _literals_in_const(source, "const RAW_EXTS")
    return _set_verdict(
        got,
        want,
        f"README.md:{line_of(doc, m.start('items'))}",
        f"src/decode.rs:{line_of(source, source.find('const RAW_EXTS'))} (const RAW_EXTS)",
    )


def check_readme_no_preview_membership(
    _args: argparse.Namespace,
) -> tuple[str, list[str]]:
    """README's no-preview names must equal decode.rs's no-rendition set."""
    doc = text("README.md")
    m = re.search(
        r"\*\*No embedded\s+preview:\*\*.*?They are (?P<items>.*?); Autoshop",
        doc,
        re.S,
    )
    if not m:
        raise LookupError("README no-preview sentence moved; re-anchor the set gate")
    got = re.findall(r"`([A-Za-z0-9]+)`", m.group("items"))

    source = text("src/decode.rs")
    truth = re.search(
        r"overrides none of the three\s+///\s+rendition methods for (?P<items>.*?)\s+"
        r"\(the trait defaults",
        source,
        re.S,
    )
    if not truth:
        raise LookupError(
            "src/decode.rs no-rendition source comment moved; re-anchor the set gate"
        )
    want = re.findall(r"\b[A-Z0-9]{2,4}\b", truth.group("items"))
    return _set_verdict(
        got,
        want,
        f"README.md:{line_of(doc, m.start('items'))}",
        f"src/decode.rs:{line_of(source, truth.start('items'))} (no-rendition set)",
    )


def cargo_version(_args: argparse.Namespace) -> Truth:
    src = text("Cargo.toml")
    at = src.index("[package]")
    end = src.find("\n[", at)
    m = re.search(r'^version = "(\d+\.\d+\.\d+)"', src[at:end], re.M)
    if not m:
        raise LookupError("Cargo.toml: no [package] version = \"X.Y.Z\"")
    return Truth((m.group(1),), f"Cargo.toml:{line_of(src, at + m.start())}")


def latest_published_version(args: argparse.Namespace) -> Truth:
    """Latest already-published release during a pre-release documentation pass.

    W4 deliberately prepares Cargo/docs for the next version before W5 builds
    and publishes it. While README carries that explicit pre-release sentence,
    the issue template must keep offering the actually published release. Once
    W5 removes the sentence, the package version becomes the truth again.
    """
    src = text("README.md")
    m = re.search(r"v(\d+\.\d+\.\d+) remains the latest published release", src)
    if m:
        return Truth(
            (m.group(1),),
            f"README.md:{line_of(src, m.start())} (pre-release publication status)",
        )
    return cargo_version(args)


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


def _mask_toml_strings(line: str) -> str:
    """Blank the CONTENT of every quoted span, keeping the line's shape.

    The depth counter below is a bracket census, and a `[` inside a string
    value is not structure — `note = "see [x"` used to leave the counter stuck
    at depth 1, after which every remaining dependency AND every section header
    was skipped (the `depth == 0` guard covers both). The empty-list guard in
    `check_tech_stack` only catches a TOTAL loss, so a half-lost inventory
    passed as coverage (R28 2c).

    Line-local, and deliberately so: Cargo.toml's strings are basic ones that
    cannot span lines, and this file has no `\"\"\"` literals.
    """
    out: list[str] = []
    quote: str | None = None
    for ch in line:
        if quote is not None:
            if ch == quote:
                quote = None
            out.append(" ")
        elif ch in "\"'":
            quote = ch
            out.append(" ")
        else:
            out.append(ch)
    return "".join(out)


def _dep_names_from(src: str) -> list[str]:
    """[`cargo_dependency_names`] over any TOML text — split out so the
    self-check below can feed it a synthetic table instead of the repo's own
    (a scanner probed only against the file it already handles proves nothing).
    """
    names: list[str] = []
    section: str | None = None
    depth = 0
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
        counted = _mask_toml_strings(line)
        depth = max(0, depth + sum(counted.count(c) for c in "[{")
                    - sum(counted.count(c) for c in "]}"))
    return sorted(set(names))


# A bracket inside a quoted value must not derail the census. Without the mask
# the `"see [x"` line leaves depth at 1 and `beta` is never seen.
_TOML_PROBE = '[dependencies]\nalpha = { version = "1", note = "see [x" }\nbeta = "2"\n'
assert _dep_names_from(_TOML_PROBE) == ["alpha", "beta"], (
    "dependency-census self-check: a bracket inside a quoted value broke the depth counter"
)


def cargo_dependency_names() -> list[str]:
    """Every DIRECT dependency name: [dependencies] (the optional gui crates
    live there behind `optional = true`), the per-target tables, build- and
    dev-dependencies.

    Only column-0 `name =` lines at bracket depth 0 count, so a multi-line
    inline table (`image = { ... features = [\\n "jpeg", ... \\n] }`) yields
    the crate and not its feature strings.
    """
    return _dep_names_from(text("Cargo.toml"))


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


def camera_model_count(_args: argparse.Namespace) -> Truth:
    """rawler's camera-model count, from the source-of-record comment in
    decode.rs (a rawler property this tree asserts once in code and quotes in
    five doc places — R28 doc audit added this row so a rawler bump cannot
    leave the doc copies behind)."""
    src = text("src/decode.rs")
    m = re.search(r"carries (?P<models>\d+) camera models", src)
    if not m:
        raise LookupError(
            "src/decode.rs: no 'carries N camera models' comment — the "
            "source-of-record moved; re-anchor this extractor"
        )
    return Truth((m.group("models"),), f"src/decode.rs:{line_of(src, m.start())}")


def readme_battery_numbers(_args: argparse.Namespace) -> Truth:
    """All README battery numbers, reordered to match ARCHITECTURE's sentence."""
    src = text("README.md")
    m = re.search(
        r"current battery is \*\*(?P<lib>\d+) library "
        r"\((?P<passed>\d+) pass \+ (?P<ignored>\d+) `#\[ignore\]`d forensic probes\) / "
        r"(?P<cli>\d+) CLI / (?P<gui>\d+) GUI / "
        r"(?P<c1>\d+)\+(?P<c2>\d+) contract\*\* tests",
        src,
    )
    if not m:
        raise LookupError("README.md: current battery sentence moved; re-anchor the extractor")
    value = tuple(
        m.group(name)
        for name in ("lib", "cli", "gui", "c1", "c2", "passed", "ignored")
    )
    return Truth(value, f"README.md:{line_of(src, m.start())} (doc-internal battery source)")


def census_counts(_args: argparse.Namespace) -> Truth | Skip:
    """Re-derive the active XMP census when its corpus root is supplied.

    The corpus is user data, not a repository fixture, so the default gate
    reports SKIP rather than pretending the machine-local count is universal.
    CI/release runs that have the corpus set `AUTOSHOP_CENSUS_ROOT` and get a
    first-party count check against the source-of-truth comment.
    """
    raw = os.environ.get("AUTOSHOP_CENSUS_ROOT")
    if not raw:
        return Skip("AUTOSHOP_CENSUS_ROOT is unset: the census corpus is outside the repo")
    root = Path(raw)
    if not root.is_dir():
        raise LookupError(f"AUTOSHOP_CENSUS_ROOT is not a directory: {root}")
    files = list(root.rglob("*.xmp"))
    counts = {"Mask/Aggregate": 0, "Mask/Image": 0, "Mask/Paint": 0, "Mask/*": 0, "Gesture": 0}
    for path in files:
        try:
            document = ET.parse(path)
        except ET.ParseError as exc:
            raise LookupError(f"{path}: XML parse failed: {exc}") from exc
        for element in document.iter():
            for name, value in element.attrib.items():
                if name.rsplit("}", 1)[-1] != "What" or not value.startswith("Mask/"):
                    continue
                counts["Mask/*"] += 1
                counts[value] = counts.get(value, 0) + 1
            if element.tag.rsplit("}", 1)[-1] == "Gesture":
                counts["Gesture"] += 1
    values = (
        str(len(files)),
        str(counts["Mask/Aggregate"]),
        str(counts["Mask/Image"]),
        str(counts["Mask/Paint"]),
        str(counts["Mask/*"]),
        str(counts["Gesture"]),
    )
    return Truth(values, f"{root} (recursive *.xmp census, XML parser)")


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
        "battery numbers — ARCHITECTURE vs README",
        r"(?P<lib>\d+) library \+ (?P<cli>\d+) CLI \+ (?P<gui>\d+) GUI "
        r"\+ (?P<c1>\d+)\+(?P<c2>\d+) contract tests[\s\S]*?"
        r"library result is (?P<passed>\d+) pass \+ (?P<ignored>\d+) `#\[ignore\]`d",
        readme_battery_numbers,
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
    # ── R28 doc-audit additions (2026-08-20): pin the maintained COPIES the
    # original 11 rows could not see — the audit found the bug template's
    # version dropdown had already drifted a whole release behind, which is
    # exactly the failure mode these rows exist to end. ────────────────────
    Claim(
        README,
        "shipped version — download link text",
        r"\[Download v(?P<version>\d+\.\d+\.\d+)\]",
        cargo_version,
    ),
    # The `9/9 at v<version>` alternative was dropped 2026-08-21. It pinned the
    # RAW zoo's COUNT to the shipped version, and those are two different kinds
    # of fact: the version is a doc copy of `Cargo.toml`, while the zoo count is
    # a MEASUREMENT of an environment-gated suite the release chain runs against
    # a corpus that does not ship with the source. Coupling them forced the
    # version bump to assert a result nobody had measured yet — the gate demands
    # the string, the string reads as a finding — which is the exact failure this
    # file exists to prevent, pointed the wrong way. The zoo count now stands
    # unversioned in README and is re-recorded per release (ROADMAP
    # 「发版链 + 环境门套件」: 数字每版实录, 勿抄上一版). The version copies that
    # ARE copies stay pinned below.
    Claim(
        README,
        "shipped version — download link URL + release-gates line",
        r"(?:releases/tag/v|Release gates at v)(?P<version>\d+\.\d+\.\d+)",
        cargo_version,
    ),
    Claim(
        ".github/ISSUE_TEMPLATE/bug_report.yml",
        "latest published version — bug template dropdown",
        r"- v(?P<version>\d+\.\d+\.\d+) \(latest release\)",
        latest_published_version,
    ),
    Claim(
        README,
        "test counts — Release gates list",
        r"(?P<lib>\d+) library \((?:\d+ pass \+ )?\d+ `#\[ignore\]`d forensic probes\) / "
        r"(?P<cli>\d+) CLI / (?P<gui>\d+) GUI / (?P<c1>\d+)\+(?P<c2>\d+) contract",
        battery_test_counts,
    ),
    Claim(
        ARCH,
        "camera models — §4 + §5 (all mentions must agree)",
        r"(?P<models>\d+) camera models",
        camera_model_count,
    ),
    Claim(
        README,
        "camera models — Decoding line",
        r"carries \*\*(?P<models>\d+) camera models\*\*",
        camera_model_count,
    ),
    Claim(
        README,
        "camera models — Tech line (bodies)",
        r"(?P<models>\d+) bodies",
        camera_model_count,
    ),
    Claim(
        ".github/ISSUE_TEMPLATE/bug_report.yml",
        "camera models — bug template",
        r"rawler carries (?P<models>\d+) camera models",
        camera_model_count,
    ),
    Claim(
        README,
        "RAW ext count — formats header",
        r"Camera RAW — (?P<formats>\d+) extensions",
        ext_count("src/decode.rs", "const RAW_EXTS"),
    ),
    Claim(
        README,
        "RAW ext count — no-preview sentence",
        r"of the (?P<formats>\d+) formats store none",
        ext_count("src/decode.rs", "const RAW_EXTS"),
    ),
    Claim(
        README,
        "baked ext count — formats header",
        r"Baked rasters — (?P<baked>\d+) extensions",
        ext_count("src/pipeline.rs", "const BAKED_EXTS"),
    ),
    SetClaim(README, "RAW extension membership", check_readme_raw_ext_membership),
    SetClaim(README, "no-preview format membership", check_readme_no_preview_membership),
    Claim(
        "src/xmp.rs",
        "active XMP census",
        r"Current corpus re-derivation: (?P<sidecars>\d+) sidecars; "
        r"(?P<aggregate>\d+) Mask/Aggregate;\s*"
        r"(?P<mask_image>\d+)\s*///\s*Mask/Image; (?P<mask_paint>\d+) Mask/Paint; "
        r"(?P<mask_any>\d+) Mask/\*; (?P<gesture>\d+) crs:Gesture",
        census_counts,
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
