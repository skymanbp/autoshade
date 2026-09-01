#!/usr/bin/env python3
"""Author the three pillar diagrams as SVG, light and dark from one source.

archify (github.com/tt-a1i/archify), which drew docs/images/architecture-*.png,
is not installable on this machine any more (`npm i -g git+...` → no
package.json; the npm package of that name is a different project), so these
are hand-authored with the same visual language: rounded boxes, a numbered
flow, muted sublabels, and one accent per pillar. Two files per diagram, light
and dark, so README's existing <picture> pattern keeps working.
"""
import os
import sys

OUTS = sys.argv[1:] or ["docs/images", "site/images"]

THEMES = {
    "light": dict(bg="#ffffff", box="#f6f7f9", boxstroke="#d5d9e0", text="#12151a",
                  sub="#5b6472", arrow="#98a1af", note="#6b7480", side="#eef1f5"),
    "dark": dict(bg="#0d1117", box="#161b22", boxstroke="#30363d", text="#e6edf3",
                 sub="#9aa4b2", arrow="#6e7681", note="#8b949e", side="#12171f"),
}

ACCENT = {"analysis": "#3f7cc4", "reimagine": "#b1683a", "lightroom": "#4a8a63"}

FONT = ("-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, "
        "'Helvetica Neue', Arial, sans-serif")


def esc(t):
    return t.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def wrap(text, width):
    """Greedy wrap on spaces — the labels are short and hand-written, so this
    never has to be clever; it only has to be deterministic."""
    words, lines, cur = text.split(), [], ""
    for w in words:
        cand = (cur + " " + w).strip()
        if len(cand) > width and cur:
            lines.append(cur)
            cur = w
        else:
            cur = cand
    if cur:
        lines.append(cur)
    return lines


class Svg:
    def __init__(self, w, h, theme, accent):
        self.w, self.h, self.t, self.a = w, h, THEMES[theme], ACCENT[accent]
        self.parts = []

    def box(self, x, y, w, h, title, sub=None, kind="box", num=None):
        c = self.t
        fill = c["side"] if kind == "side" else c["box"]
        stroke = self.a if kind == "accent" else c["boxstroke"]
        sw = 1.6 if kind == "accent" else 1.0
        dash = ' stroke-dasharray="5 4"' if kind == "side" else ""
        self.parts.append(
            f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="10" fill="{fill}" '
            f'stroke="{stroke}" stroke-width="{sw}"{dash}/>'
        )
        ty = y + (26 if sub else h / 2 + 5)
        if num is not None:
            self.parts.append(
                f'<text x="{x + 12}" y="{y + 20}" font-family="{FONT}" font-size="11" '
                f'font-weight="700" fill="{self.a}">{num}</text>'
            )
            ty = y + 40
        for i, line in enumerate(wrap(title, 22)):
            self.parts.append(
                f'<text x="{x + w / 2}" y="{ty + i * 16}" text-anchor="middle" '
                f'font-family="{FONT}" font-size="13" font-weight="600" '
                f'fill="{c["text"]}">{esc(line)}</text>'
            )
        if sub:
            sy = ty + len(wrap(title, 22)) * 16 + 4
            lines = wrap(sub, 20)
            for i, line in enumerate(lines):
                self.parts.append(
                    f'<text x="{x + w / 2}" y="{sy + i * 14}" text-anchor="middle" '
                    f'font-family="{FONT}" font-size="11" fill="{c["sub"]}">{esc(line)}</text>'
                )
            bottom = sy + (len(lines) - 1) * 14
            assert bottom <= y + h - 8, (
                f"box {title!r}: text reaches {bottom:.0f}, box ends at {y + h}")
        for line in wrap(title, 22) + (wrap(sub, 20) if sub else []):
            assert len(line) * 7.2 <= w - 14, f"box {title!r}: line {line!r} too wide for {w}"

    def arrow(self, x1, y1, x2, y2, label=None, dashed=False):
        c = self.t
        d = ' stroke-dasharray="4 4"' if dashed else ""
        self.parts.append(
            f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{c["arrow"]}" '
            f'stroke-width="1.4" marker-end="url(#a)"{d}/>'
        )
        if label:
            mx, my = (x1 + x2) / 2, (y1 + y2) / 2 - 7
            self.parts.append(
                f'<text x="{mx}" y="{my}" text-anchor="middle" font-family="{FONT}" '
                f'font-size="10.5" fill="{c["note"]}">{esc(label)}</text>'
            )

    def title(self, x, y, text, sub):
        c = self.t
        self.parts.append(
            f'<text x="{x}" y="{y}" font-family="{FONT}" font-size="17" font-weight="700" '
            f'fill="{c["text"]}">{esc(text)}</text>'
        )
        self.parts.append(
            f'<text x="{x}" y="{y + 20}" font-family="{FONT}" font-size="12" '
            f'fill="{c["sub"]}">{esc(sub)}</text>'
        )

    def note(self, x, y, text, anchor="start"):
        self.parts.append(
            f'<text x="{x}" y="{y}" text-anchor="{anchor}" font-family="{FONT}" '
            f'font-size="11" fill="{self.t["note"]}">{esc(text)}</text>'
        )

    def render(self, alt):
        c = self.t
        return (
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {self.w} {self.h}" '
            f'width="{self.w}" height="{self.h}" role="img" aria-label="{esc(alt)}">'
            f'<defs><marker id="a" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" '
            f'markerHeight="7" orient="auto-start-reverse">'
            f'<path d="M 0 0 L 10 5 L 0 10 z" fill="{c["arrow"]}"/></marker></defs>'
            f'<rect width="{self.w}" height="{self.h}" fill="{c["bg"]}"/>'
            + "".join(self.parts)
            + "</svg>"
        )


# ── Pillar 1 — AI analysis develop ─────────────────────────────────────────
def pillar_analysis(theme):
    s = Svg(1200, 470, theme, "analysis")
    s.title(28, 34, "Pillar 1 — AI analysis develop",
            "your own library decides what “your style” means; the model only proposes")
    y = 92
    s.box(28, y, 190, 124, "This RAW", "EXIF, histogram, preview", num="01")
    s.box(258, y, 210, 124, "Similarity, four terms",
          "14-dim hand feature; image, text and sentence vectors",
          kind="accent", num="02")
    s.box(508, y, 200, 124, "K = 4 neighbours",
          "from YOUR RAW + .xmp library", num="03")
    s.box(748, y, 230, 124, "Reference block",
          "sliders, curve, colour families, look, mask habits", num="04")
    s.arrow(218, y + 60, 254, y + 60)
    s.arrow(468, y + 60, 504, y + 60)
    s.arrow(708, y + 60, 744, y + 60)

    y2 = 266
    s.box(258, y2, 230, 124, "Advisor proposes a recipe",
          "bounded by a schema", kind="accent", num="05")
    s.box(528, y2, 200, 124, "Verifier",
          "numbers only, never pixels; two revisions", num="06")
    s.box(768, y2, 210, 124, "Style blend",
          "toward the neighbours’ means, capped", num="07")
    s.box(994, y2, 178, 124, "EditRecipe", "renderable, exports to XMP",
          kind="accent")
    s.arrow(863, y + 124, 863, y2 - 6, "the block is a reference, never a copy")
    s.arrow(488, y2 + 60, 524, y2 + 60)
    s.arrow(728, y2 + 60, 764, y2 + 60)
    s.arrow(978, y2 + 60, 1002, y2 + 60)
    s.arrow(628, y2 + 124, 628, y2 + 138)
    s.arrow(628, y2 + 138, 378, y2 + 138)
    s.arrow(378, y2 + 138, 378, y2 + 128, "revise")
    s.note(28, y2 + 34, "Every model input is")
    s.note(28, y2 + 50, "text and numbers.")
    s.note(28, y2 + 72, "The photographs stay")
    s.note(28, y2 + 88, "on your disk unless")
    s.note(28, y2 + 104, "you send one on purpose.")
    return s.render(
        "Pillar 1: this RAW, a four-term similarity, four neighbours from your own "
        "library, a reference block, then propose, verify and blend into a recipe")


# ── Pillar 2 — generation and reverse fit ──────────────────────────────────
def pillar_reimagine(theme):
    s = Svg(1200, 470, theme, "reimagine")
    s.title(28, 34, "Pillar 2 — AI generates the look, the engine recovers the recipe",
            "the generated pixels are a TARGET, never the delivery")
    y = 92
    s.box(28, y, 176, 124, "Neutral render", "the RAW, no edits", num="01")
    s.box(240, y, 210, 124, "Generated target",
          "fidelity-hardened prompt; it may invent content",
          kind="accent", num="02")
    s.box(486, y, 214, 124, "Structural divergence D",
          "gradient correlation + pyramid energy", num="03")
    s.box(736, y, 214, 124, "Which fit is honest",
          "full fit, or atmosphere only", kind="accent", num="04")
    s.box(986, y, 186, 124, "Global fit",
          "64-bin Tukey-IRLS + evidence", num="05")
    s.arrow(204, y + 60, 236, y + 60)
    s.arrow(450, y + 60, 482, y + 60)
    s.arrow(700, y + 60, 732, y + 60)
    s.arrow(950, y + 60, 982, y + 60)

    y2 = 266
    s.box(28, y2, 214, 124, "Semantic zones OR ranges",
          "four regions, or bands — never both", num="06")
    s.box(278, y2, 206, 124, "Evidence quadtree tiles",
          "frozen evidence, depth 2, four tiles", num="07")
    s.box(520, y2, 196, 124, "Residual free mask",
          "what the layers left unexplained", num="08")
    s.box(752, y2, 198, 124, "Honesty budget",
          "strength decides what may move", kind="accent", num="09")
    s.box(986, y2, 186, 124, "EditRecipe + XMP",
          "no pixel is delivered", kind="accent")
    s.arrow(1079, y + 124, 1079, 214)
    s.arrow(1079, 232, 135, 232)
    s.arrow(135, 232, 135, y2 - 6, "each layer must earn its place against the evidence")
    s.arrow(242, y2 + 60, 274, y2 + 60)
    s.arrow(484, y2 + 60, 516, y2 + 60)
    s.arrow(716, y2 + 60, 748, y2 + 60)
    s.arrow(950, y2 + 60, 982, y2 + 60)
    return s.render(
        "Pillar 2: neutral render and a generated target, structural divergence "
        "picking the fitting mode, a global fit, then layered attachment under an "
        "honesty budget, ending in a recipe and XMP")


# ── Pillar 3 — the Lightroom mathematics ───────────────────────────────────
def pillar_lightroom(theme):
    s = Svg(1200, 470, theme, "lightroom")
    s.title(28, 34, "Pillar 3 — the mathematics of matching Lightroom",
            "measured against Adobe’s own output, not guessed from documentation")
    s.box(28, 104, 200, 118, "Lightroom .xmp", "crs: fields, masks, curves",
          kind="accent")
    s.box(500, 104, 200, 118, "EditRecipe", "one typed model, both directions",
          kind="accent")
    s.box(972, 104, 200, 118, "Engine render", "deterministic f32 pipeline",
          kind="accent")
    s.arrow(228, 150, 496, 150, "read: own scope, nested Look, as-shot rule")
    s.arrow(496, 184, 228, 184, "write: conservative merge, named losses")
    s.arrow(700, 163, 968, 163, "render")

    y = 268
    s.box(28, y, 268, 132, "Mask-frame law",
          "radial through Lightroom’s own inverse; linear handles transported", num="01")
    s.box(330, y, 250, 132, "Lens geometry",
          "profile ungeom; image centre derived, not tuned", num="02")
    s.box(614, y, 250, 132, "Tone and falloff",
          "monotone tone LUT; C¹ smoothstep falloff, fitted", num="03")
    s.box(898, y, 274, 132, "Brush kernel",
          "k(ρ;h) = (1 − ρ^m)^n, fitted to held-out strokes",
          num="04")
    s.arrow(600, 222, 600, y - 8, "every law is a measured claim with a residual")
    return s.render(
        "Pillar 3: .xmp and the recipe read and write both ways into the engine, "
        "over four measured laws — mask frames, lens geometry, tone and falloff, "
        "and the brush kernel")


def main():
    for out in OUTS:
        os.makedirs(out, exist_ok=True)
    for name, fn in (("pillar-analysis", pillar_analysis),
                     ("pillar-reimagine-fit", pillar_reimagine),
                     ("pillar-lightroom-math", pillar_lightroom)):
        for theme in ("light", "dark"):
            body = fn(theme)
            for out in OUTS:
                path = os.path.join(out, f"{name}-{theme}.svg")
                with open(path, "w", encoding="utf-8", newline='\n') as f:
                    f.write(body)
                print(path, os.path.getsize(path), "B")


main()
