#!/usr/bin/env python3
"""Regenerate the embedded GUI symbol fonts (assets/fonts/*.ttf).

The GUI's font chain is: egui's bundled fonts -> these embedded subsets ->
a system CJK font found at runtime. egui's bundle has no glyphs for most of
the toolbar/panel symbols (⧉ ⊖ ◭ ▭ ◯ ◌ ✓ ✕ 🖌 ...), and leaning on system
fonts for them made the UI machine-dependent (tofu boxes on a stock Windows
install, because DengXian lacks them too). So we ship tiny subsets of four
OFL-licensed Noto faces covering every symbol the GUI actually renders.

Donor fonts (download from https://github.com/google/fonts, ofl/ tree):
    NotoSansSymbols2-Regular.ttf   geometric shapes, technical, dingbats
    NotoSansSymbols[wght].ttf      enclosed alphanumerics ② and ⎘
    NotoSansMath-Regular.ttf       math operators ⊕ ⊖ ∩ ≡ + arrows ↶ ↷ ⇄ ⇒ ⇱
    NotoEmoji[wght].ttf            monochrome emoji missing from egui's subset
    NotoSansSC[wght].ttf           the Chinese UI's own hanzi (see below)

Usage:
    python scripts/subset_gui_fonts.py --fonts-dir <dir with the five donors>

The needed-glyph list is NOT hand-maintained: it is extracted from the string
literals of src/bin/gui.rs + src/bin/i18n.rs (the same extraction the GUI's
`embedded_fonts_cover_every_ui_symbol` test performs in Rust).

CJK is subsetted the same way, and deliberately WITHOUT margin blocks: only
the hanzi the Chinese UI actually renders are embedded. A full CJK face is
~16 MB, but the exact set the translations use is a few hundred glyphs, so
the Chinese UI stops depending on the machine having a system CJK font — it
used to render as an entire window of tofu boxes on a system without one.
The runtime system-CJK fallback stays in the chain for user-supplied text
(file names, folder paths), which this static extraction cannot know.
Margin blocks (full symbol ranges the donors carry) are included so most
future symbol picks won't need a re-run; if the Rust coverage test ever fails,
re-run this script and commit the refreshed assets/fonts/.

Requires: fonttools >= 4.x  (pip install fonttools)
"""

from __future__ import annotations

import argparse
import io
import sys
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

REPO = Path(__file__).resolve().parent.parent
# The GUI crate is a module tree (round 12 split); glob it so strings moved
# into new module files never silently fall out of the embedded subset.
SOURCES = sorted((REPO / "src" / "bin" / "gui").glob("**/*.rs"))
OUT_DIR = REPO / "assets" / "fonts"

# CJK + fullwidth ranges. Embedded from the exact translated strings (no
# margin blocks — a whole block here would be megabytes), so the Chinese UI
# renders without a system CJK font.
CJK_RANGES = [(0x2E80, 0x9FFF), (0xF900, 0xFAFF), (0xFF00, 0xFFEF)]

# (output name, donor file, variable?, margin blocks)
DONORS = [
    ("NotoSansSymbols2-autoshop.ttf", "NotoSansSymbols2-Regular.ttf", False,
     [(0x2190, 0x21FF), (0x2300, 0x23FF), (0x25A0, 0x25FF), (0x2600, 0x26FF),
      (0x2700, 0x27BF), (0x29C0, 0x29FF), (0x2B00, 0x2BFF)]),
    ("NotoSansSymbols-autoshop.ttf", "NotoSansSymbols[wght].ttf", True,
     [(0x2300, 0x23FF), (0x2460, 0x24FF)]),
    ("NotoSansMath-autoshop.ttf", "NotoSansMath-Regular.ttf", False,
     [(0x2190, 0x21FF), (0x2200, 0x22FF), (0x2980, 0x29FF), (0x2A00, 0x2AFF)]),
    ("NotoEmoji-autoshop.ttf", "NotoEmoji[wght].ttf", True, []),
    # Last: the symbol donors above claim the shared punctuation first, so a
    # glyph the UI already renders identically everywhere keeps doing so.
    ("NotoSansSC-autoshop.ttf", "NotoSansSC[wght].ttf", True, []),
]


def is_cjk(cp: int) -> bool:
    return any(lo <= cp <= hi for lo, hi in CJK_RANGES)


def string_literal_chars(src: str) -> set[str]:
    """Non-ASCII chars appearing inside Rust string/char literals only.

    Comments are deliberately excluded — a symbol in a comment never renders.
    Mirrors the Rust-side extractor in gui.rs (keep the two in sync).
    """
    out: set[str] = set()
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c == "/" and i + 1 < n and src[i + 1] == "/":
            j = src.find("\n", i)
            i = n if j == -1 else j + 1
        elif c == "/" and i + 1 < n and src[i + 1] == "*":
            depth, i = 1, i + 2
            while i < n and depth:
                if src.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif src.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
        elif c == "r" and i + 1 < n and src[i + 1] in '#"':
            j = i + 1
            hashes = 0
            while j < n and src[j] == "#":
                hashes, j = hashes + 1, j + 1
            if j < n and src[j] == '"':
                close = '"' + "#" * hashes
                k = src.find(close, j + 1)
                k = n if k == -1 else k
                out.update(ch for ch in src[j + 1 : k] if ord(ch) > 127)
                i = k + len(close)
            else:
                i += 1
        elif c == '"':
            i += 1
            while i < n:
                if src[i] == "\\":
                    # A `\u{...}` escape renders exactly like the literal
                    # char, so it needs the same glyph. Decoded in STRING
                    # literals only: the sole char-literal escape in the tree
                    # is the coverage test's deliberately-unassigned notdef
                    # sentinel, which must stay out of the needed set.
                    if src[i + 1 : i + 3] == "u{":
                        close = src.find("}", i + 3)
                        if close != -1:
                            try:
                                cp = int(src[i + 3 : close], 16)
                            except ValueError:
                                cp = -1
                            if cp > 127:
                                out.add(chr(cp))
                            i = close + 1
                            continue
                    i += 2
                elif src[i] == '"':
                    i += 1
                    break
                else:
                    if ord(src[i]) > 127:
                        out.add(src[i])
                    i += 1
        elif c == "'":
            # char literal 'x' (maybe escaped); anything else is a lifetime
            if i + 2 < n and src[i + 1] != "\\" and src[i + 2] == "'":
                if ord(src[i + 1]) > 127:
                    out.add(src[i + 1])
                i += 3
            elif i + 3 < n and src[i + 1] == "\\" and src[i + 3] == "'":
                i += 4
            else:
                i += 1
        else:
            i += 1
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--fonts-dir", required=True, type=Path,
                    help="directory holding the four donor TTFs")
    args = ap.parse_args()

    needed = set()
    for p in SOURCES:
        needed |= string_literal_chars(p.read_text(encoding="utf-8"))
    n_cjk = sum(1 for ch in needed if is_cjk(ord(ch)))
    print(
        f"{len(needed) - n_cjk} symbols + {n_cjk} CJK codepoints "
        "referenced by GUI string literals"
    )

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    remaining = {ord(ch) for ch in needed}
    total_bytes = 0
    for out_name, donor_name, variable, blocks in DONORS:
        donor_path = args.fonts_dir / donor_name
        if not donor_path.exists():
            print(f"ERROR: donor missing: {donor_path}", file=sys.stderr)
            return 1
        font = TTFont(donor_path)
        if variable:
            # updateFontNames, or the shipped face keeps the DEFAULT instance's
            # name while carrying wght=400 outlines. NotoSansSC defaults to
            # wght=100, so without this the subset ships labelled "Thin".
            font = instancer.instantiateVariableFont(
                font, {"wght": 400}, updateFontNames=True
            )
        cmap = font.getBestCmap()
        take = {cp for cp in remaining if cp in cmap}
        for lo, hi in blocks:
            take |= {cp for cp in cmap if lo <= cp <= hi}
        remaining -= take
        opts = subset.Options()
        opts.no_hinting = True          # egui rasterizes unhinted anyway
        opts.name_IDs = "*"             # keep full attribution/license names
        subsetter = subset.Subsetter(options=opts)
        subsetter.populate(unicodes=sorted(take))
        subsetter.subset(font)
        buf = io.BytesIO()
        font.save(buf)
        out_path = OUT_DIR / out_name
        out_path.write_bytes(buf.getvalue())
        total_bytes += len(buf.getvalue())
        print(f"  {out_name}: {len(take)} codepoints, {len(buf.getvalue()):,} bytes")

    if remaining:
        # Not fatal by itself: egui's own bundle may cover these (the Rust test
        # is the real gate, it checks the WHOLE chain). Report them honestly.
        print("left to egui's bundled fonts (verify via the Rust coverage test):")
        for cp in sorted(remaining):
            print(f"  U+{cp:05X} {chr(cp)}")
    print(f"total embedded: {total_bytes:,} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
