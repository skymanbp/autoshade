#!/usr/bin/env python3
"""Build assets/fonts/LucideIcons-autoshade.ttf: Lucide icons under the GUI's
own symbol codepoints.

The GUI labels its verbs with Unicode symbols ("✨ AI edit", "🗑", "⚙"), drawn
by a mix of Noto faces and egui's bundled emoji font — five different hands,
weights and baselines in one toolbar. This face redraws the icon-like ones in
ONE consistent stroke family, Lucide (https://lucide.dev, ISC licence), WITHOUT
touching a single label: each Lucide outline is copied under the symbol's own
codepoint, and the face goes FIRST in egui's font chain, so it wins exactly the
codepoints in ICONS and every other character falls through to the chain as
before. Prose symbols (→ × ± ° ≈ ≤ ≥ ← ↑ ↓ ● ⌘ ∩ …) are deliberately not
mapped: they are text, not icons.

Each icon is scaled from Lucide's 1000-unit square to BOX em and lowered so
its centre sits at CENTRE em, the middle of a lowercase-plus-cap line, and
gets SIDE em of air either side.

Donor: lucide-static@0.460.0 (npm), `font/lucide.ttf` and `font/info.json`.

Usage:
    python scripts/build_icon_font.py --lucide <dir with lucide.ttf and info.json>

Requires: fonttools >= 4.x
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTFont

REPO = Path(__file__).resolve().parent.parent
OUT = REPO / "assets" / "fonts" / "LucideIcons-autoshade.ttf"

UPM = 1000
BOX = 0.96      # icon height and width, em
CENTRE = 0.35   # icon centre above the baseline, em (Ubuntu cap 0.693, x 0.517)
SIDE = 0.06     # air either side, em
# Line metrics copied from egui's text face, Ubuntu-Light: epaint puts each
# glyph's baseline at ITS OWN face's ascent below the row top, so any other
# ascent would lift or drop every icon off the text's baseline.
ASCENT, DESCENT, LINE_GAP = 932, -189, 28

# GUI codepoint -> Lucide icon name. The meaning the GUI gives the symbol
# picks the icon, not the symbol's Unicode name.
ICONS = {
    "✨": "sparkles",            # AI-generated / AI edit
    "⚠": "triangle-alert",      # warning
    "▣": "image",               # the original card
    "✎": "pencil",              # an edited card
    "✕": "x",                   # close / delete / clear
    "◈": "droplets",            # a denoised master
    "▦": "layers",              # a stacked master
    "🤖": "bot",                 # AI heal / AI masks
    "🖌": "brush",               # paint area
    "＋": "plus",                # add a mask
    "💧": "pipette",             # sample a colour in the image
    "🗂": "folder-open",         # pick a folder
    "⎘": "stamp",               # clone stamp
    "▨": "layout-grid",         # bitmap mask
    "✓": "check",               # selected / done
    "🔄": "refresh-cw",          # build / fetch again
    "↺": "rotate-ccw",          # clear / reset
    "↻": "rotate-cw",           # redraw
    "🗑": "trash-2",             # remove
    "▾": "chevron-down",        # menu
    "↶": "undo-2",              # undo
    "↷": "redo-2",              # redo
    "⭯": "rotate-ccw-square",   # turn left
    "⭮": "rotate-cw-square",    # turn right
    "◫": "columns-2",           # compare side by side
    "⬛": "square",              # single view
    "⌨": "keyboard",            # shortcuts
    "👁": "eye",                 # show / hide a mask
    "◭": "git-compare-arrows",  # reverse-fit
    "🎯": "focus",               # pick focus
    "⚙": "settings",            # settings
    "🖼": "image",               # empty canvas
    "▲": "triangle",            # clipping overlay
    "☰": "grip-vertical",       # drag handle
    "⬆": "arrow-up",            # move a mask up
    "⬇": "arrow-down",          # move a mask down
    "⧉": "copy",                # duplicate
    "ⓘ": "info",                # a folded explanation (headings::explainer)
}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--lucide", required=True, type=Path)
    args = ap.parse_args()
    donor = TTFont(args.lucide / "lucide.ttf")
    info = json.loads((args.lucide / "info.json").read_text(encoding="utf-8"))
    dcmap = donor.getBestCmap()
    dglyphs = donor.getGlyphSet()
    dupm = donor["head"].unitsPerEm

    scale = BOX * UPM / dupm
    dx = SIDE * UPM
    dy = CENTRE * UPM - BOX * UPM / 2
    advance = round(BOX * UPM + 2 * SIDE * UPM)

    order = [".notdef"]
    glyphs = {".notdef": TTGlyphPen(None).glyph()}
    metrics = {".notdef": (advance, 0)}
    cmap = {}
    for ch, name in ICONS.items():
        code = int(info[name]["encodedCode"].lstrip("\\"), 16)
        gname = f"lucide_{name.replace('-', '_')}"
        if gname not in glyphs:
            pen = TTGlyphPen(None)
            dglyphs[dcmap[code]].draw(TransformPen(pen, (scale, 0, 0, scale, dx, dy)))
            glyphs[gname] = pen.glyph()
            metrics[gname] = (advance, round(dx))
            order.append(gname)
        cmap[ord(ch)] = gname

    fb = FontBuilder(UPM, isTTF=True)
    fb.setupGlyphOrder(order)
    fb.setupCharacterMap(cmap)
    fb.setupGlyf(glyphs)
    fb.setupHorizontalMetrics(metrics)
    fb.setupHorizontalHeader(ascent=ASCENT, descent=DESCENT, lineGap=LINE_GAP)
    fb.setupNameTable({"familyName": "AutoShade Lucide Icons", "styleName": "Regular"})
    fb.setupOS2(sTypoAscender=ASCENT, sTypoDescender=DESCENT, sTypoLineGap=LINE_GAP, usWinAscent=ASCENT, usWinDescent=-DESCENT)
    fb.setupPost()
    fb.save(OUT)
    print(f"{OUT.name}: {len(cmap)} codepoints, {len(order) - 1} icons, {OUT.stat().st_size} bytes")


if __name__ == "__main__":
    main()
