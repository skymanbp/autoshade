#!/usr/bin/env python3
"""Draw the three pillar diagrams, light and dark, from one description each.

archify (github.com/tt-a1i/archify), which drew the architecture PNG the README
used to carry, is not installable on this machine any more (`npm i -g git+…`
finds no package.json; the npm package of that name is a different project), so
these are authored here in the same visual language the architecture picture
now uses: rounded boxes, a numbered flow, muted sublabels, one accent per
pillar. `scripts/architecture_diagram.py` is the fourth diagram and the same
shared canvas, `scripts/diagram_check.py`, measures all four.

Boxes are placed by hand here — a pillar is an argument in a fixed order, not a
graph to be laid out — but nothing else is. Every box takes the height its own
wrapped text needs, every row takes the tallest box in it, and every connector
label goes wherever the checker says nothing else already is. Two defects the
previous hand-placed version carried are gone with that: three captions sat on
top of the vertical arrows they named, and pillar 1's "the block is a reference"
arrow pointed at empty canvas instead of at the advisor box.
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)

from diagram_check import (Canvas, node_metrics,
                           place_connector_label, write_svgs)

OUT_DIRS = ["docs/images"]
SITE_TARGET = "site/index.html"

W = 1200.0
PAD_X = 28.0
ROW_GAP = 72.0
HEAD_TOP = 20.0
ACCENT = {"pillar-analysis": "accent_blue",
          "pillar-reimagine-fit": "accent_rust",
          "pillar-lightroom-math": "accent_green"}


class Sheet:
    """A canvas plus the row bookkeeping the three pillars share: give it rows
    of boxes and it sizes each one from its own text, aligns the row on the
    tallest, and remembers the geometry so the connector calls below can name
    an edge of a box instead of a coordinate."""

    def __init__(self, name, alt, title, sub):
        self.name, self.alt, self.title, self.sub = name, alt, title, sub
        self.accent = ACCENT[name]
        self.rows = []
        self.box_top, self.box_h, self.box_x = {}, {}, {}

    def row(self, top, boxes):
        self.rows.append((top, boxes))
        return [b["rid"] for b in boxes]

    @staticmethod
    def _row_height(boxes):
        return max(node_metrics(b["w"], b["title"], b.get("sub"), None,
                                b.get("num"))[3] for b in boxes)

    def build(self, extra_bottom):
        bottom = max(top + self._row_height(boxes) for top, boxes in self.rows)
        c = Canvas(W, bottom + extra_bottom, self.alt, dom_id=self.name)
        c.heading(PAD_X, HEAD_TOP, self.title, self.sub)
        for top, boxes in self.rows:
            rh = self._row_height(boxes)
            for b in boxes:
                c.node(b["x"], top, b["w"], rh, title=b["title"],
                       sub=b.get("sub"), num=b.get("num"), accent=self.accent,
                       kind="accent" if b.get("accent") else "box",
                       rid=b["rid"])
                self.box_top[b["rid"]] = top
                self.box_h[b["rid"]] = rh
                self.box_x[b["rid"]] = (b["x"], b["w"])
        return c

    def left(self, rid, f=0.5):
        return (self.box_x[rid][0], self.box_top[rid] + self.box_h[rid] * f)

    def right(self, rid, f=0.5):
        x, w = self.box_x[rid]
        return (x + w, self.box_top[rid] + self.box_h[rid] * f)

    def top(self, rid, f=0.5):
        x, w = self.box_x[rid]
        return (x + w * f, self.box_top[rid])

    def bottom(self, rid, f=0.5):
        x, w = self.box_x[rid]
        return (x + w * f, self.box_top[rid] + self.box_h[rid])


def chain(c, s, ids):
    """Left-to-right arrows between neighbouring boxes of one row."""
    for a, b in zip(ids, ids[1:]):
        c.connector([s.right(a), s.left(b)], src=a, dst=b)


# ── Pillar 1 — AI analysis develop ─────────────────────────────────────────
def pillar_analysis():
    s = Sheet(
        "pillar-analysis",
        "Diagram: a RAW plus XMP library becomes exemplars carrying a "
        "14-dimension feature, SigLIP 2 image vectors and Qwen3-VL sentences; "
        "a query retrieves four neighbours by the hybrid distance; their "
        "habits reach the proposer behind an untrusted-data fence and a capped "
        "pull moves the result toward the photographer's means",
        "Pillar 1 — AI analysis develop",
        "your own library decides what “your style” means; the model only "
        "proposes")
    first = s.row(92, [
        dict(rid="raw", x=28, w=190, num="01", title="This RAW",
             sub="EXIF, histogram, preview"),
        dict(rid="sim", x=258, w=210, num="02", accent=True,
             title="Similarity, four terms",
             sub="14-dim hand feature; image, text and sentence vectors"),
        dict(rid="knn", x=508, w=200, num="03", title="K = 4 neighbours",
             sub="from YOUR RAW + .xmp library"),
        dict(rid="ref", x=748, w=230, num="04", title="Reference block",
             sub="sliders, curve, colour families, look, mask habits")])
    y2 = 92 + Sheet._row_height(s.rows[0][1]) + ROW_GAP
    second = s.row(y2, [
        dict(rid="advisor", x=258, w=230, num="05", accent=True,
             title="Advisor proposes a recipe", sub="bounded by a schema"),
        dict(rid="verify", x=528, w=200, num="06", title="Verifier",
             sub="numbers only, never pixels; two revisions"),
        dict(rid="blend", x=768, w=210, num="07", title="Style blend",
             sub="toward the neighbours’ means, capped"),
        dict(rid="recipe", x=994, w=178, accent=True, title="EditRecipe",
             sub="renderable, exports to XMP")])
    c = s.build(extra_bottom=76)
    chain(c, s, first)
    chain(c, s, second)
    # 04 -> 05: down out of the reference block, left across the sheet, into
    # the advisor's top edge. The hand-placed version drew this straight down
    # from the block and stopped in empty canvas, 260 px from any box.
    down, into = s.bottom(first[-1]), s.top(second[0], 0.62)
    ref_to_advisor = [down, (down[0], down[1] + 30), (into[0], down[1] + 30),
                      into]
    c.connector(ref_to_advisor, src=first[-1], dst=second[0])
    # 06 -> 05: the revision loop, under the row it belongs to.
    loop_y = s.box_top[second[1]] + s.box_h[second[1]] + 32
    a, b = s.bottom(second[1], 0.35), s.bottom(second[0], 0.62)
    revise = [a, (a[0], loop_y), (b[0], loop_y), b]
    c.connector(revise, src=second[1], dst=second[0])
    place_connector_label(c, ref_to_advisor,
                          "the block is a reference, never a copy", "04 to 05")
    place_connector_label(c, revise, "revise", "06 back to 05")
    c.note(PAD_X, y2 + 24, ["Every model input is", "text and numbers."])
    c.note(PAD_X, y2 + 76, ["The photographs stay", "on your disk unless",
                            "you send one on purpose."])
    return c


# ── Pillar 2 — generation and reverse fit ──────────────────────────────────
def pillar_reimagine():
    s = Sheet(
        "pillar-reimagine-fit",
        "Diagram: a generated target is measured against the input by the "
        "structural-divergence statistic D, which selects either a full solve "
        "or a bounded atmosphere mode; a Tukey-biweight tone regression and "
        "gated local stages produce a recipe, and only the recipe reaches the "
        "full-resolution render",
        "Pillar 2 — AI generates the look, the engine recovers the recipe",
        "the generated pixels are a TARGET, never the delivery")
    top = s.row(92, [
        dict(rid="neutral", x=28, w=176, num="01", title="Neutral render",
             sub="the RAW, no edits"),
        dict(rid="target", x=240, w=210, num="02", accent=True,
             title="Generated target",
             sub="fidelity-hardened prompt; it may invent content"),
        dict(rid="diverge", x=486, w=214, num="03",
             title="Structural divergence D",
             sub="gradient correlation + pyramid energy"),
        dict(rid="mode", x=736, w=214, num="04", accent=True,
             title="Which fit is honest", sub="full fit, or atmosphere only"),
        dict(rid="global", x=986, w=186, num="05", title="Global fit",
             sub="64-bin Tukey-IRLS + evidence")])
    y2 = 92 + Sheet._row_height(s.rows[0][1]) + ROW_GAP
    bot = s.row(y2, [
        dict(rid="regions", x=28, w=214, num="06",
             title="Semantic zones OR ranges",
             sub="four regions, or bands — never both"),
        dict(rid="tiles", x=278, w=206, num="07",
             title="Evidence quadtree tiles",
             sub="frozen evidence, depth 2, four tiles"),
        dict(rid="free", x=520, w=196, num="08", title="Residual free mask",
             sub="what the layers left unexplained"),
        dict(rid="budget", x=752, w=198, num="09", accent=True,
             title="Honesty budget", sub="strength decides what may move"),
        dict(rid="out", x=986, w=186, accent=True, title="EditRecipe + XMP",
             sub="no pixel is delivered")])
    c = s.build(extra_bottom=40)
    chain(c, s, top)
    chain(c, s, bot)
    # 05 -> 06: one polyline with one head, where the hand-placed version drew
    # three separate arrows and so put an arrowhead at each corner.
    a, into = s.bottom(top[-1]), s.top(bot[0])
    lane = a[1] + 32
    carry = [a, (a[0], lane), (into[0], lane), into]
    c.connector(carry, src=top[-1], dst=bot[0])
    place_connector_label(c, carry,
                          "each layer must earn its place against the evidence",
                          "05 to 06")
    return c


# ── Pillar 3 — the Lightroom mathematics ───────────────────────────────────
def pillar_lightroom():
    s = Sheet(
        "pillar-lightroom-math",
        "Diagram: a Lightroom sidecar is read by a scoped XML layer, its "
        "slider domains are measured rather than assumed, mask coordinates "
        "cross measured frame laws, and each result is published with its "
        "residual",
        "Pillar 3 — the mathematics of matching Lightroom",
        "measured against Adobe’s own output, not guessed from documentation")
    xmp, rec, eng = s.row(104, [
        dict(rid="xmp", x=28, w=200, accent=True, title="Lightroom .xmp",
             sub="crs: fields, masks, curves"),
        dict(rid="recipe", x=500, w=200, accent=True, title="EditRecipe",
             sub="one typed model, both directions"),
        dict(rid="engine", x=972, w=200, accent=True, title="Engine render",
             sub="deterministic f32 pipeline")])
    y2 = 104 + Sheet._row_height(s.rows[0][1]) + ROW_GAP + 30
    laws = s.row(y2, [
        dict(rid="frame", x=28, w=268, num="01", title="Mask-frame law",
             sub="radial through Lightroom’s own inverse; linear handles "
                 "transported"),
        dict(rid="lens", x=330, w=250, num="02", title="Lens geometry",
             sub="profile ungeom; image centre derived, not tuned"),
        dict(rid="tone", x=614, w=250, num="03", title="Tone and falloff",
             sub="monotone tone LUT; C¹ smoothstep falloff, fitted"),
        dict(rid="brush", x=898, w=274, num="04", title="Brush kernel",
             sub="k(ρ;h) = (1 − ρ^m)^n, fitted to held-out strokes")])
    c = s.build(extra_bottom=30)
    read = [s.right(xmp, 0.34), s.left(rec, 0.34)]
    write = [s.left(rec, 0.72), s.right(xmp, 0.72)]
    render = [s.right(rec), s.left(eng)]
    c.connector(read, src=xmp, dst=rec)
    c.connector(write, src=rec, dst=xmp)
    c.connector(render, src=rec, dst=eng)
    # All four laws are measured against the recipe, so the arrow leaves the
    # recipe and fans into all four — the old one left the empty middle of the
    # sheet and ended between two boxes, naming neither. The trunk and the bus
    # bar carry no arrowhead; the four drops each end on their own box.
    down = s.bottom(rec)
    bus = down[1] + 34
    trunk = [down, (down[0], bus)]
    c.connector(trunk, src=rec, head=False)
    ends = [s.top(rid) for rid in laws]
    law = [(ends[0][0], bus), (ends[-1][0], bus)]
    c.connector(law, head=False)
    for rid, end in zip(laws, ends):
        c.connector([(end[0], bus), end], dst=rid)
    place_connector_label(c, read,
                          "read: own scope, nested Look, as-shot rule", "read")
    place_connector_label(c, write,
                          "write: conservative merge, named losses", "write")
    place_connector_label(c, render, "render", "render")
    place_connector_label(c, law,
                          "every law is a measured claim with a residual", "laws")
    return c


def main():
    os.chdir(ROOT)
    for name, fn in (("pillar-analysis", pillar_analysis),
                     ("pillar-reimagine-fit", pillar_reimagine),
                     ("pillar-lightroom-math", pillar_lightroom)):
        canvas = fn()
        written, runs, segs = write_svgs(canvas, name, OUT_DIRS,
                                         site_targets=[SITE_TARGET])
        print("%s %.0fx%.0f — %d text runs, %d connector segments, 0 overlaps"
              % (name, canvas.w, canvas.h, runs, segs))
        for p in written:
            print(" ", p, os.path.getsize(p), "B")


if __name__ == "__main__":
    main()
