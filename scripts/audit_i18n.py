#!/usr/bin/env python3
"""i18n alignment audit: tr/trf call sites in gui.rs vs ZH_ENTRIES in i18n.rs.

The GUI's i18n is an English-skeleton catalogue (see src/bin/i18n.rs): the
English literal at each `tr`/`trf` call site IS the lookup key, and a missing
Chinese entry silently falls back to English. This audit makes that silence
visible:

  * keys with NO zh translation      -> untranslated UI (add a pair)
  * zh entries matching NO call site -> dead or drifted keys (byte-for-byte
                                        mismatch = the translation never fires)
  * dynamic (non-literal) call sites -> keys arriving through variables; the
                                        DYNAMIC_KEYS map below mirrors those
                                        sources and must be kept in sync

Exit code 1 when anything is missing/dead, so it can gate a release.
Run: python scripts/audit_i18n.py

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

# Keys that reach `tr` through a variable rather than a literal — one entry
# per dynamic call site, mirroring the constant/label source it reads.
# KEEP IN SYNC with gui.rs (the audit prints every dynamic site it finds).
DYNAMIC_KEYS = [
    # CURVE_CHANNELS names (curve_editor picker)
    "Master", "Red", "Green", "Blue",
    # HSL_BANDS (colour mixer rows)
    "Orange", "Yellow", "Aqua", "Purple", "Magenta",
    # GRADE_REGIONS (colour grading region picker)
    "Shadows", "Midtones", "Highlights", "Global",
    # EXPORT_SPACES (export colour-space combo)
    "sRGB (universal)", "Adobe RGB (print)", "Display P3 (wide-gamut screens)",
    # CROP_ASPECTS first entry ("Free"; numeric ratios are not translated)
    "Free",
    # ThemePref::label()
    "Dark", "Light",
    # VariantKind::label()
    "▣ Original", "✨ AI generated", "◭ Reverse-fit",
    # MaskRole::en_name() (reverse-fit zone masks)
    "Sky (reverse-fit)", "Land (reverse-fit)",
    # AI segmentation labels (segment_mask / add_ai_mask `label` argument)
    "Sky", "Subject",
    # set_canvas_status(plain) callers
    "variant removed", "restored the canvas pixels",
    # fill-quality combo literal array
    "high", "medium", "low",
    # clipping-triangle `what`
    "highlight clip", "shadow crush",
    # zone table (Original, Some(0.0)) rows / base-mask default name
    "Original",
]


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


def tr_keys(src: str) -> tuple[set[str], list[tuple[int, str]]]:
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
            line = src.count("\n", 0, m.start()) + 1
            dynamic.append((line, src[m.start() : m.start() + 60].splitlines()[0]))
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


def main() -> int:
    keys, dynamic = tr_keys(GUI)
    zh = zh_entries(I18N)
    zh_set = set(zh)
    dupes = sorted({k for k in zh if zh.count(k) > 1})
    wanted = keys | set(DYNAMIC_KEYS)
    missing = sorted(k for k in wanted if k not in zh_set)
    dead = sorted(k for k in zh_set if k not in wanted)

    print(f"{len(keys)} literal keys + {len(DYNAMIC_KEYS)} dynamic-map keys; "
          f"{len(zh)} zh entries; {len(dynamic)} dynamic call sites")
    for title, items in [("DUPLICATE zh keys", dupes),
                         ("keys with NO zh translation", missing),
                         ("zh entries matching NO call site", dead)]:
        print(f"\n== {len(items)} {title} ==")
        for k in items:
            print("  " + repr(k[:100]))
    print(f"\n== dynamic call sites (must be mirrored in DYNAMIC_KEYS) ==")
    for line, snip in dynamic:
        print(f"  gui.rs:{line}: {snip}")
    return 1 if (dupes or missing or dead) else 0


if __name__ == "__main__":
    sys.exit(main())
