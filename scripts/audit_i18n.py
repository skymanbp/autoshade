#!/usr/bin/env python3
"""i18n alignment audit: tr/trf call sites in gui.rs vs ZH_ENTRIES in i18n.rs.

The GUI's i18n is an English-skeleton catalogue (see src/bin/i18n.rs): the
English literal at each `tr`/`trf` call site IS the lookup key, and a missing
Chinese entry silently falls back to English. This audit makes that silence
visible — and it is a GATE, not a report:

  * keys with NO zh translation      -> untranslated UI (add a pair)
  * zh entries matching NO call site -> dead or drifted keys (byte-for-byte
                                        mismatch = the translation never fires)
  * dynamic (non-literal) call sites -> every one must match a registered
                                        DYNAMIC_SITES pattern, and every
                                        registered source has its key set
                                        EXTRACTED from the Rust source where
                                        that is mechanical (arrays, label fns,
                                        literal call arguments) — so adding an
                                        enum variant or array entry without a
                                        zh pair fails the audit instead of
                                        silently falling back to English
  * UI literals that bypass tr()     -> flagged unless allow-listed (a button
                                        label passed as a bare string never
                                        reaches the catalogue at all)

Exit code 1 when anything is missing/dead/unregistered, so it can gate a
release.  Run: python scripts/audit_i18n.py

Note: en keys may contain CJK-range punctuation (「」 ＋ －) — that renders
via the runtime system-CJK font fallback (installed on every Windows/macOS;
see install_fonts in gui.rs), and is house style, not drift.
"""

from __future__ import annotations

import sys
from pathlib import Path
import re

REPO = Path(__file__).resolve().parent.parent
GUI = (REPO / "src" / "bin" / "gui.rs").read_text(encoding="utf-8")
I18N = (REPO / "src" / "bin" / "i18n.rs").read_text(encoding="utf-8")
RECIPE = (REPO / "src" / "recipe.rs").read_text(encoding="utf-8")

# ── Dynamic-key registry ────────────────────────────────────────────────────
# Every `tr(lang, <non-literal>)` call site must match one of these argument
# patterns; an unmatched site fails the audit, forcing new dynamic sources to
# be registered here (and therefore extracted or declared) instead of slipping
# through as an English fallback nobody notices.
DYNAMIC_SITES = [
    (r"^plain\b", "set_canvas_status callers"),
    (r"^(?:self\.variants\[self\.active\]\.)?kind\.label\(\)", "VariantKind::label"),
    (r"^(?:self\.theme|t)\.label\(\)", "ThemePref::label"),
    (r"^EXPORT_SPACES\[", "EXPORT_SPACES"),
    (r"^GRADE_REGIONS\[", "GRADE_REGIONS"),
    (r"^CROP_ASPECTS\[", "CROP_ASPECTS"),
    (r"^what\b", "clipping-triangle what"),
    (r"^name\b", "named rows (CURVE_CHANNELS / HSL_BANDS / GRADE_REGIONS / MaskRole)"),
    (r"^label\b", "AI segmentation labels"),
    (r"^en\b", "MaskRole::en_name"),
    (r"^\[", "inline literal array"),
]

# Key sets extracted mechanically from the Rust source. Each entry names an
# anchor that must exist (a missing anchor = the source moved/renamed = the
# registry drifted = fail) and yields at least one letter-bearing literal.
#   ("array", src, "const NAME")      literals inside the [...] initialiser
#   ("impl_fn", src_name, "impl Type", "fn name")  literals inside the fn body
#   ("calls", src, "name(")           literal FIRST arguments of name(...)
DYNAMIC_SOURCES = [
    ("array", "gui", "const CURVE_CHANNELS"),
    ("array", "gui", "const HSL_BANDS"),
    ("array", "gui", "const GRADE_REGIONS"),
    ("array", "gui", "const EXPORT_SPACES"),
    ("array", "gui", "const CROP_ASPECTS"),
    ("impl_fn", "gui", "impl VariantKind", "fn label"),
    ("impl_fn", "gui", "impl ThemePref", "fn label"),
    ("impl_fn", "recipe", "impl MaskRole", "fn en_name"),
    ("calls", "gui", "set_canvas_status("),
]

# Dynamic keys whose Rust source offers nothing mechanical to anchor on
# (values assembled inline at scattered sites). Keep this list SHORT — prefer
# a DYNAMIC_SOURCES extractor whenever the values live in one named place.
DECLARED_DYNAMIC = [
    # clipping-triangle `what`
    "highlight clip", "shadow crush",
    # AI segmentation labels (segment_mask / add_ai_mask `label` argument)
    "Sky", "Subject",
    # zone table (Original, Some(0.0)) rows / base-mask default name
    "Original",
]

# ── Literal-bypass allowlist ────────────────────────────────────────────────
# Letter-bearing strings legitimately passed to UI text constructors WITHOUT
# tr(): product names and cross-language technical jargon only. Anything else
# flagged below needs `tr(lang, ...)` + a ZH_ENTRIES pair. A listed literal
# that no longer occurs anywhere is reported as dead so the list cannot rot.
ALLOWED_BYPASS = {
    "Autoshop",   # product name, never translated
    "PNG/TIFF",   # file-format jargon, identical in both languages
    "JPEG",       # file-format jargon
    "2560px",     # pixel-size option; its lettered siblings ARE translated
    "Esc",        # keyboard key name (house style, like the shortcut combos)
}

SRC = {"gui": GUI, "recipe": RECIPE}


def parse_literal(src: str, i: int) -> tuple[str, int]:
    """src[i] == '"' — decode a Rust string literal (escapes + `\\<newline>`
    line continuation, which also swallows leading whitespace)."""
    out: list[str] = []
    i += 1
    while i < len(src):
        c = src[i]
        if c == "\\":
            nxt = src[i + 1]
            if nxt == "\n":
                i += 2
                while i < len(src) and src[i] in " \t":
                    i += 1
                continue
            out.append({"n": "\n", "t": "\t", '"': '"', "\\": "\\", "'": "'"}.get(nxt, "\\" + nxt))
            i += 2
        elif c == '"':
            return "".join(out), i + 1
        else:
            out.append(c)
            i += 1
    raise ValueError("unterminated literal")


def lettered(lit: str) -> bool:
    return any(ch.isascii() and ch.isalpha() for ch in lit)


def skip_call(src: str, i: int) -> int:
    """src[i] == '(' — index just past the matching ')'."""
    depth = 1
    i += 1
    while i < len(src) and depth:
        c = src[i]
        if c == '"':
            _, i = parse_literal(src, i)
            continue
        if src.startswith("//", i):
            i = src.index("\n", i)
            continue
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
        i += 1
    return i


def tr_keys(src: str) -> tuple[set[str], list[tuple[int, str]]]:
    """Literal keys + (line, argument-expression) for every dynamic site."""
    keys: set[str] = set()
    dynamic: list[tuple[int, str]] = []
    for m in re.finditer(r"\btrf?\(", src):
        s = m.start()
        if s > 0 and (src[s - 1].isalnum() or src[s - 1] == "_"):
            continue
        i = m.end()
        depth = 0  # skip the lang argument to its comma at depth 0
        while i < len(src):
            c = src[i]
            if c == "(":
                depth += 1
            elif c == ")":
                if depth == 0:
                    break
                depth -= 1
            elif c == "," and depth == 0:
                break
            i += 1
        if i >= len(src) or src[i] != ",":
            continue
        i += 1
        while i < len(src) and src[i] in " \n\r\t":
            i += 1
        if i < len(src) and src[i] == '"':
            keys.add(parse_literal(src, i)[0])
        else:
            # Capture the full argument expression (to the call's close or the
            # next top-level comma) so DYNAMIC_SITES can match on it exactly.
            j, depth = i, 0
            while j < len(src):
                c = src[j]
                if c == '"':
                    _, j = parse_literal(src, j)
                    continue
                if c in "([{":
                    depth += 1
                elif c in ")]}":
                    if depth == 0:
                        break
                    depth -= 1
                elif c == "," and depth == 0:
                    break
                j += 1
            expr = " ".join(src[i:j].split())
            line = src.count("\n", 0, m.start()) + 1
            dynamic.append((line, expr))
    return keys, dynamic


def zh_entries(src: str) -> list[str]:
    """English keys of ZH_ENTRIES, in order (anchor on the DECLARATION —
    doc comments reference the name earlier in the file)."""
    j = src.index("[", src.index("static ZH_ENTRIES"))
    end = src.index("\n];", j)
    lits: list[str] = []
    while j < end:
        if src[j] == '"':
            lit, j = parse_literal(src, j)
            lits.append(lit)
        elif src[j] == "/" and src[j + 1] == "/":
            j = src.index("\n", j)
        else:
            j += 1
    if len(lits) % 2:
        raise SystemExit("ZH_ENTRIES parse drift: odd literal count")
    return lits[0::2]


def block_literals(src: str, start: int, opener: str, closer: str) -> set[str]:
    """Letter-bearing literals inside the first opener..closer block at/after
    `start` (comments skipped, nesting respected)."""
    i = src.index(opener, start)
    depth = 0
    out: set[str] = set()
    while i < len(src):
        c = src[i]
        if src.startswith("//", i):
            i = src.index("\n", i)
            continue
        if c == '"':
            lit, i = parse_literal(src, i)
            if lettered(lit):
                out.add(lit)
            continue
        if c == opener:
            depth += 1
        elif c == closer:
            depth -= 1
            if depth == 0:
                return out
        i += 1
    raise SystemExit(f"unclosed block after offset {start}")


def extract_source_keys() -> tuple[set[str], list[str]]:
    """Run every DYNAMIC_SOURCES extractor; a missing anchor or an empty
    extraction is registry drift and fails the audit."""
    wanted: set[str] = set()
    problems: list[str] = []
    for spec in DYNAMIC_SOURCES:
        kind, src_name, anchor = spec[0], spec[1], spec[2]
        src = SRC[src_name]
        at = src.find(anchor)
        if at < 0:
            problems.append(f"{src_name}: anchor {anchor!r} not found (source moved/renamed?)")
            continue
        if kind == "array":
            # Scan from the `=`: `const NAME: [&str; 8] = [...]` — starting at
            # the anchor itself would open on the TYPE's `[` and extract nothing.
            got = block_literals(src, src.index("=", at), "[", "]")
        elif kind == "impl_fn":
            fn_at = src.find(spec[3], at)
            if fn_at < 0:
                problems.append(f"{src_name}: {spec[3]!r} not found inside {anchor!r}")
                continue
            got = block_literals(src, fn_at, "{", "}")
        else:  # calls: literal FIRST arguments only
            got = set()
            for m in re.finditer(re.escape(anchor), src):
                i = m.end()
                while i < len(src) and src[i] in " \n\r\t":
                    i += 1
                if i < len(src) and src[i] == '"':
                    lit, _ = parse_literal(src, i)
                    if lettered(lit):
                        got.add(lit)
        if not got:
            problems.append(f"{src_name}: {anchor!r} extracted no keys (drift?)")
        wanted |= got
    return wanted, problems


def literal_bypasses(src: str) -> list[tuple[int, str]]:
    """Letter-bearing literals handed straight to UI text constructors —
    text the catalogue can never translate. Literals inside tr()/trf()/
    format!() sub-calls are the translated path and are skipped."""
    ctor = re.compile(
        r"(?:\.(?:small_button|button|label|heading|selectable_label|on_hover_text"
        r"|checkbox|radio_value|selectable_value|hyperlink_to|text)|RichText::new"
        r"|Window::new)\s*\("
    )
    skip_callee = re.compile(r"(?:\btrf?|format!)\s*\($")
    out: list[tuple[int, str]] = []
    for m in ctor.finditer(src):
        i, depth = m.end(), 1
        while i < len(src) and depth:
            c = src[i]
            if src.startswith("//", i):
                i = src.index("\n", i)
                continue
            if c == '"':
                lit, i = parse_literal(src, i)
                if lettered(lit) and lit not in ALLOWED_BYPASS:
                    out.append((src.count("\n", 0, i) + 1, lit))
                continue
            if c == "(":
                if skip_callee.search(src[max(0, i - 12): i + 1]):
                    i = skip_call(src, i)
                    continue
                depth += 1
            elif c == ")":
                depth -= 1
            i += 1
    return out


def main() -> int:
    keys, dynamic = tr_keys(GUI)
    zh = zh_entries(I18N)
    zh_set = set(zh)
    dupes = sorted({k for k in zh if zh.count(k) > 1})

    extracted, source_problems = extract_source_keys()
    # Inline literal arrays carry their key set IN the site expression.
    inline_keys: set[str] = set()
    unregistered: list[tuple[int, str]] = []
    for line, expr in dynamic:
        hit = next((name for pat, name in DYNAMIC_SITES if re.search(pat, expr)), None)
        if hit is None:
            unregistered.append((line, expr))
        elif hit == "inline literal array":
            j = 0
            while j < len(expr):
                if expr[j] == '"':
                    lit, j = parse_literal(expr, j)
                    if lettered(lit):
                        inline_keys.add(lit)
                else:
                    j += 1

    dynamic_keys = extracted | inline_keys | set(DECLARED_DYNAMIC)
    wanted = keys | dynamic_keys
    missing = sorted(k for k in wanted if k not in zh_set)
    dead = sorted(k for k in zh_set if k not in wanted)
    bypass = literal_bypasses(GUI)
    dead_allow = sorted(a for a in ALLOWED_BYPASS if a not in GUI)

    print(f"{len(keys)} literal keys + {len(dynamic_keys)} dynamic keys "
          f"({len(extracted)} extracted / {len(inline_keys)} inline / "
          f"{len(DECLARED_DYNAMIC)} declared); {len(zh)} zh entries; "
          f"{len(dynamic)} dynamic call sites")
    for title, items in [("DUPLICATE zh keys", dupes),
                         ("keys with NO zh translation", missing),
                         ("zh entries matching NO call site", dead),
                         ("dynamic-source registry problems", source_problems),
                         ("dead ALLOWED_BYPASS entries", dead_allow)]:
        print(f"\n== {len(items)} {title} ==")
        for k in items:
            print("  " + repr(str(k)[:100]))
    print(f"\n== {len(unregistered)} UNREGISTERED dynamic call sites ==")
    for line, expr in unregistered:
        print(f"  gui.rs:{line}: {expr[:80]}")
    print(f"\n== {len(bypass)} UI literals bypassing tr() ==")
    for line, lit in bypass:
        print(f"  gui.rs:{line}: {lit[:80]!r}")
    bad = (dupes or missing or dead or source_problems or dead_allow
           or unregistered or bypass)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
