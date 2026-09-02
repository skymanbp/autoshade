#!/usr/bin/env python3
"""The shared diagram canvas, and the overlap checker that refuses to let a
diagram be written when two things sit on top of each other.

Drawing and checking live in one module on purpose: the only code that knows a
text run's exact box is the code that emitted it, so the canvas records the
geometry of every glyph run, every rectangle border and every connector segment
as it draws, and `Canvas.check()` then answers the question the user asked —
"no text or line overlapping anything" — from measurements rather than from a
look at the picture. `scripts/pillar_diagrams.py` and
`scripts/architecture_diagram.py` both build on this and both call
`write_svgs()`, which runs the check and raises before it writes a byte.

The rules, in full (`Canvas.check` implements exactly these):

  R1  Two text runs may not overlap. Every run is inflated first — 2 px on all
      four sides, plus 2.5 % of its own width left and right — so two runs need at least 4 px of clear space between them.
      Runs of one wrapped label are one block and are exempt from each other;
      R0 covers them instead.
  R0  Inside a block, the leading must be at least (ASCENT + DESCENT) x size,
      which is exactly one em — so stacked lines of one label cannot touch.
  R2  A text run must lie inside the rectangle it belongs to, inset by 6 px.
  R3  A text run (inflated) may not touch any rectangle that is neither its
      own nor an ancestor of its own. A caption belongs to its group, so it
      must clear every node in that group; a node's title must clear the tag
      pill inside the same node; free-standing notes belong to nothing and must
      clear every rectangle on the canvas.
  R4  A text run (inflated) may not touch any connector segment, its own
      included — an edge label is placed beside its line, never on it.
  R5  A connector's arrowhead must end on the border of the box it points at
      (within 1 px), and neither its tip nor its triangle may sit inside a text
      run.
  R6  A connector segment may not run through a node it is not attached to.
      Group panels are exempt: a cross-band connector has to cross them.
  R7  Two segments of different connectors may not lie on top of each other
      collinearly for more than 4 px — that is a routing bug that reads as one
      line, not as two.

Two modules carry what the rules are written in.
`scripts/diagram_geometry.py` holds the rectangle and segment predicates and
nothing else, so each of them can be read on its own.

Glyph metrics come from `scripts/diagram_metrics.py`, which is Chrome's own
canvas `measureText` for this font stack at 100 px, kern pairs included — a
naive sum of per-character advances is wrong by up to 2.9 px on one pair, which
is more than the margin above, so the pairs matter. That table is Segoe UI's,
the face the stack resolves to on Windows; the same file renders in SF on macOS
and in whatever a distribution ships on Linux, where individual advances differ
by up to about fifteen per cent. `diagram_geometry.CROSS_FACE` is what pays for
that: on top of the fixed 2 px margin, every run is widened by a further 2.5 % of its own
width, so a label that clears its neighbours here still clears them in a face a
few per cent wider.
"""
import math
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from diagram_geometry import (obstacles, pad_text, _border_distance,
                              _boxes_overlap, _collinear_overlap,
                              _seg_box_length, _seg_hits_box)
from diagram_metrics import FONT_STACK
from diagram_metrics import text_width as _measure

FONT = FONT_STACK

# Vertical extent of one line of text around its baseline, in ems. A run's box
# runs from baseline - ASCENT*size to baseline + DESCENT*size; the two sum to
# one em, which is what R0 then demands of the leading.
ASCENT, DESCENT = 0.78, 0.22
OWNER_INSET = 6.0        # R2: how far inside its own box a label must stay
HEAD_LEN, HEAD_HALF = 8.0, 3.5   # arrowhead triangle, in px

SNAP = (400, 600, 700)   # the weights diagram_metrics was measured at


def text_width(s, size, weight=400):
    """Advance width of `s` in px, kerning included, at the nearest measured
    weight. A character outside the table is charged the widest advance at its
    weight, so an unknown glyph can only make the checker stricter."""
    return _measure(s, size, min(SNAP, key=lambda w: (abs(w - weight), w)))


def wrap_px(text, max_px, size, weight=400):
    """Greedy wrap on spaces to a pixel width. A single word wider than the
    budget still gets its own line — the caller's box is then too narrow, and
    R2 says so by name instead of this function silently shaving the text."""
    words, lines, cur = text.split(), [], ""
    for w in words:
        cand = (cur + " " + w).strip()
        if cur and text_width(cand, size, weight) > max_px:
            lines.append(cur)
            cur = w
        else:
            cur = cand
    if cur:
        lines.append(cur)
    return lines or [""]


def block_height(n_lines, size, leading):
    return (n_lines - 1) * leading + (ASCENT + DESCENT) * size


NODE_PAD = 12.0          # horizontal padding inside a node
NODE_TOP, NODE_BOT = 14.0, 14.0
NUM_BAND = 18.0          # the row the numbered badge takes, when there is one
TAG_H = 22.0            # tall enough that OWNER_INSET fits around 9.5 px text


def node_metrics(w, title, sub=None, tag=None, num=None):
    """The wrapped lines and the height a node needs for them. The generators
    call this before they place anything, so a box is never sized by hand and a
    longer sublabel grows its box instead of spilling out of it."""
    inner = w - 2 * NODE_PAD
    t_lines = wrap_px(title, inner, 13, 600)
    s_lines = wrap_px(sub, inner, 11, 400) if sub else []
    content = block_height(len(t_lines), 13, 16)
    if s_lines:
        content += 4 + block_height(len(s_lines), 11, 14)
    if tag:
        content += 8 + TAG_H
    top = NODE_TOP + (NUM_BAND if num is not None else 0.0)
    return t_lines, s_lines, content, top + content + NODE_BOT


# ── palettes ───────────────────────────────────────────────────────────────
# Colour roles, not colours: the canvas writes {{role}} and a palette resolves
# it, which is what makes "light and dark differ only in colours" true by
# construction rather than by inspection. The `site` palette resolves every
# role to a CSS custom property with the light value as its fallback, so one
# inline copy of the SVG follows the page's own theme tokens.
_LIGHT = {
    "bg": "#ffffff", "box": "#f6f7f9", "box_stroke": "#d5d9e0",
    "side": "#eef1f5", "group": "#fbfcfd", "group_stroke": "#c9d1dc",
    "text": "#12151a", "sub": "#5b6472", "note": "#6b7480", "arrow": "#98a1af",
    "tag": "#4a5361", "tag_bg": "#e8ecf1",
    "accent_blue": "#2f66a8", "accent_rust": "#8f5029", "accent_green": "#2f6b47",
}
_DARK = {
    "bg": "#0d1117", "box": "#161b22", "box_stroke": "#30363d",
    "side": "#12171f", "group": "#0f141b", "group_stroke": "#2b333d",
    "text": "#e6edf3", "sub": "#9aa4b2", "note": "#8b949e", "arrow": "#6e7681",
    "tag": "#aab4c0", "tag_bg": "#1b222b",
    "accent_blue": "#7fb2ea", "accent_rust": "#e6a077", "accent_green": "#6fc294",
}
_SITE = {k: "var(--dg-%s, %s)" % (k.replace("_", "-"), v)
         for k, v in _LIGHT.items()}
PALETTES = {"light": _LIGHT, "dark": _DARK, "site": _SITE}


def esc(t):
    return (str(t).replace("&", "&amp;").replace("<", "&lt;")
            .replace(">", "&gt;").replace('"', "&quot;"))


def _fmt(v):
    """One decimal place, and no trailing '.0' — so the same geometry always
    serialises to the same bytes whether it arrived as an int or a float."""
    r = round(float(v), 1)
    return str(int(r)) if r == int(r) else str(r)


# ── geometry records ───────────────────────────────────────────────────────
class _Rect:
    __slots__ = ("x", "y", "w", "h", "rid", "kind", "parent")

    def __init__(self, x, y, w, h, rid, kind, parent):
        self.x, self.y, self.w, self.h = x, y, w, h
        self.rid, self.kind, self.parent = rid, kind, parent

    @property
    def box(self):
        return (self.x, self.y, self.x + self.w, self.y + self.h)


class _Run:
    __slots__ = ("x0", "y0", "x1", "y1", "s", "owner", "block")

    def __init__(self, x0, y0, x1, y1, s, owner, block):
        self.x0, self.y0, self.x1, self.y1 = x0, y0, x1, y1
        self.s, self.owner, self.block = s, owner, block

    @property
    def box(self):
        return (self.x0, self.y0, self.x1, self.y1)


class _Seg:
    __slots__ = ("x1", "y1", "x2", "y2", "eid", "src", "dst")

    def __init__(self, x1, y1, x2, y2, eid, src, dst):
        self.x1, self.y1, self.x2, self.y2 = x1, y1, x2, y2
        self.eid, self.src, self.dst = eid, src, dst


# ── the canvas ─────────────────────────────────────────────────────────────
class Canvas:
    """Draws, and records what it drew so `check()` can measure it."""

    def __init__(self, width, height, alt, dom_id=None):
        self.w, self.h, self.alt, self.dom_id = width, height, alt, dom_id
        self.parts = []
        self.rects = []
        self.runs = []
        self.segs = []
        self.heads = []      # (tip_x, tip_y, dir_x, dir_y, eid, target_rid)
        self._blocks = 0
        self._auto = 0
        self._head_id = "dg-%s-arrow" % (dom_id or "diagram")

    # -- low level ---------------------------------------------------------
    def _rid(self, prefix, rid):
        if rid is None:
            self._auto += 1
            rid = "%s%d" % (prefix, self._auto)
        return rid

    def rect(self, x, y, w, h, fill, stroke, *, rx=10, sw=1.0, dash=None,
             rid=None, kind="node", parent=None, record=True):
        rid = self._rid("r", rid)
        d = ' stroke-dasharray="%s"' % dash if dash else ""
        self.parts.append(
            '<rect x="%s" y="%s" width="%s" height="%s" rx="%s" fill="{{%s}}" '
            'stroke="{{%s}}" stroke-width="%s"%s/>'
            % (_fmt(x), _fmt(y), _fmt(w), _fmt(h), _fmt(rx), fill, stroke,
               _fmt(sw), d))
        if record:
            self.rects.append(_Rect(x, y, w, h, rid, kind, parent))
        return rid

    def text(self, lines, x, top, *, size, weight, role, anchor="middle",
             leading=None, owner=None, letter=None):
        """One block of text. `top` is the top of the first line's box, not a
        baseline, because every caller lays out from the top down."""
        leading = leading if leading is not None else round(size * 1.24, 1)
        assert leading >= (ASCENT + DESCENT) * size - 1e-6, (
            "R0: leading %.1f is tighter than one em at size %.1f" % (leading, size))
        self._blocks += 1
        block = self._blocks
        ls = ' letter-spacing="%s"' % _fmt(letter) if letter else ""
        for i, line in enumerate(lines):
            base = top + ASCENT * size + i * leading
            wpx = text_width(line, size, weight)
            if letter:
                wpx += letter * max(0, len(line) - 1)
            x0 = {"start": x, "middle": x - wpx / 2, "end": x - wpx}[anchor]
            self.parts.append(
                '<text x="%s" y="%s" text-anchor="%s" font-family="%s" '
                'font-size="%s" font-weight="%d" fill="{{%s}}"%s>%s</text>'
                % (_fmt(x), _fmt(base), anchor, FONT, _fmt(size), weight, role,
                   ls, esc(line)))
            self.runs.append(_Run(x0, top + i * leading, x0 + wpx,
                                  top + i * leading + (ASCENT + DESCENT) * size,
                                  line, owner, block))
        return top + block_height(len(lines), size, leading)

    def path(self, d, *, stroke, sw=1.4, dash=None, head=True):
        marker = ' marker-end="url(#%s)"' % self._head_id if head else ""
        dd = ' stroke-dasharray="%s"' % dash if dash else ""
        self.parts.append(
            '<path d="%s" fill="none" stroke="{{%s}}" stroke-width="%s" '
            'stroke-linecap="round" stroke-linejoin="round"%s%s/>'
            % (d, stroke, _fmt(sw), dd, marker))

    # -- diagram vocabulary ------------------------------------------------
    def node(self, x, y, w, h, *, title, sub=None, tag=None, num=None,
             accent=None, kind="box", rid=None, parent=None):
        """A rounded box with a wrapped title, an optional muted sublabel, an
        optional numbered badge and an optional tag pill. `h` may be None, in
        which case the box grows to its content."""
        pad = NODE_PAD
        t_lines, s_lines, content, need = node_metrics(w, title, sub, tag, num)
        top_pad = NODE_TOP + (NUM_BAND if num is not None else 0.0)
        if h is None:
            h = need
        assert h >= need - 1e-6, (
            "box %r needs %.0f px of height, got %.0f" % (title, need, h))
        rid = self._rid("n", rid)
        fill = "side" if kind == "side" else "box"
        stroke = accent if (kind == "accent" and accent) else "box_stroke"
        self.rect(x, y, w, h, fill, stroke, sw=1.6 if kind == "accent" else 1.0,
                  dash="5 4" if kind == "side" else None, rid=rid, kind="node",
                  parent=parent)
        cy = y + top_pad + (h - top_pad - NODE_BOT - content) / 2
        if num is not None:
            self.text([str(num)], x + pad, y + 12, size=11, weight=700,
                      role=accent or "accent_blue", anchor="start", owner=rid)
        cy = self.text(t_lines, x + w / 2, cy, size=13, weight=600, role="text",
                       leading=16, owner=rid)
        if s_lines:
            cy = self.text(s_lines, x + w / 2, cy + 4, size=11, weight=400,
                           role="sub", leading=14, owner=rid)
        if tag:
            tw = text_width(tag, 9.5, 600) + 18
            tx, ty = x + (w - tw) / 2, cy + 8
            pill = self.rect(tx, ty, tw, TAG_H, "tag_bg", "tag_bg",
                             rx=TAG_H / 2, sw=0.8, kind="pill", parent=rid)
            self.text([tag], x + w / 2, ty + (TAG_H - 9.5) / 2, size=9.5,
                      weight=600, role="tag", owner=pill)
        return rid

    def group(self, x, y, w, h, accent, rid=None):
        """A boundary panel. Its caption is placed later by `group_label`, once
        the connectors are down: a caption belongs on whichever corner of the
        panel nothing crosses, and only the router knows where that is."""
        rid = self._rid("g", rid)
        self.rect(x, y, w, h, "group", accent, rx=14, sw=1.3, dash="7 5",
                  rid=rid, kind="group")
        return rid

    def group_label(self, rid, label, accent, *, pad=16.0, cap=34.0):
        """Put a panel's caption in the first corner of the panel that is
        clear. Both the top and the bottom strip of a panel are kept free of
        boxes, so a caption never has to leave its own panel to find room."""
        r = next(x for x in self.rects if x.rid == rid)
        x0, y0, x1, y1 = r.box
        h = (ASCENT + DESCENT) * 11.5
        cands = []
        for top in (y0 + (cap - h) / 2, y1 - cap + (cap - h) / 2):
            cands += [(x0 + pad, top, "start"), (x1 - pad, top, "end"),
                      ((x0 + x1) / 2, top, "middle")]
        for x, top, anchor in cands:
            _, bad = self.would_collide([label], x, top, size=11.5, weight=700,
                                        anchor=anchor)
            if not bad:
                return self.text([label], x, top, size=11.5, weight=700,
                                 role=accent, anchor=anchor, owner=rid)
        raise OverlapError("no free corner for the panel caption %r" % label)

    def connector(self, points, *, src=None, dst=None, label=None,
                  label_at=None, label_anchor="middle", dashed=False,
                  eid=None, head=True, role="arrow"):
        """An orthogonal or gently rounded polyline. `points` is the full route
        including both endpoints, which must already sit on the borders of the
        boxes named by `src` and `dst`."""
        eid = self._rid("e", eid)
        d = _round_path(points)
        self.path(d, stroke=role, dash="4 4" if dashed else None, head=head)
        for (x1, y1), (x2, y2) in zip(points, points[1:]):
            self.segs.append(_Seg(x1, y1, x2, y2, eid, src, dst))
        if head:
            (px, py), (qx, qy) = points[-2], points[-1]
            n = math.hypot(qx - px, qy - py) or 1.0
            self.heads.append((qx, qy, (qx - px) / n, (qy - py) / n, eid, dst))
        if label:
            lx, ly = label_at
            lines = label if isinstance(label, list) else [label]
            self.text(lines, lx, ly, size=10.5, weight=400, role="note",
                      anchor=label_anchor, leading=13)
        return eid

    def would_collide(self, lines, x, top, *, size, weight, anchor="middle",
                      leading=None):
        """The boxes `text()` would produce here, and whether any of them lands
        on something already recorded. Connector labels are placed with this
        rather than by hand: the caller offers a list of candidate positions and
        takes the first clean one, so a label can never end up on a line."""
        leading = leading if leading is not None else round(size * 1.24, 1)
        boxes = []
        for i, line in enumerate(lines):
            wpx = text_width(line, size, weight)
            x0 = {"start": x, "middle": x - wpx / 2, "end": x - wpx}[anchor]
            y0 = top + i * leading
            boxes.append((x0, y0, x0 + wpx, y0 + (ASCENT + DESCENT) * size))
        for b in boxes:
            ib = pad_text(b)
            if b[0] < 0 or b[1] < 0 or b[2] > self.w or b[3] > self.h:
                return boxes, True
            for r in self.runs:
                if _boxes_overlap(ib, pad_text(r.box)):
                    return boxes, True
            for r in self.rects:
                if any(_boxes_overlap(ib, o) for o in obstacles(r)):
                    return boxes, True
            for s in self.segs:
                if _seg_hits_box(s.x1, s.y1, s.x2, s.y2, ib):
                    return boxes, True
        return boxes, False

    def place_label(self, lines, candidates, *, size=10.5, weight=400,
                    role="note", leading=13, what=""):
        """Draw `lines` at the first candidate `(x, top, anchor)` that collides
        with nothing. Raises when every candidate is taken, naming the label —
        a diagram that cannot place a label is a layout bug, not a warning."""
        for x, top, anchor in candidates:
            _, bad = self.would_collide(lines, x, top, size=size, weight=weight,
                                        anchor=anchor, leading=leading)
            if not bad:
                return self.text(lines, x, top, size=size, weight=weight,
                                 role=role, anchor=anchor, leading=leading)
        raise OverlapError(
            "no free position for label %r%s — %d candidates all collide"
            % (" ".join(lines), (" on " + what) if what else "", len(candidates)))

    def heading(self, x, y, title, sub):
        bottom = self.text([title], x, y, size=17, weight=700, role="text",
                           anchor="start")
        return self.text([sub], x, bottom + 5, size=12, weight=400, role="sub",
                         anchor="start")

    def note(self, x, y, lines, *, anchor="start", size=11, role="note"):
        return self.text(lines, x, y, size=size, weight=400, role=role,
                         anchor=anchor, leading=15)

    # -- the check ---------------------------------------------------------
    def _ancestors(self, rid):
        seen, cur = [], rid
        by_id = {r.rid: r for r in self.rects}
        while cur is not None and cur in by_id:
            cur = by_id[cur].parent
            if cur is not None:
                seen.append(cur)
        return seen

    def check(self):
        v = []
        by_id = {r.rid: r for r in self.rects}

        # R1 — text against text.
        for i, a in enumerate(self.runs):
            ab = pad_text(a.box)
            for b in self.runs[i + 1:]:
                if a.block == b.block:
                    continue
                if _boxes_overlap(ab, pad_text(b.box)):
                    v.append("R1 text %r overlaps text %r near (%.0f, %.0f)"
                             % (a.s, b.s, a.x0, a.y0))

        # R2/R3 — text against rectangles.
        for a in self.runs:
            ab = pad_text(a.box)
            if a.owner is not None:
                o = by_id[a.owner].box
                if not (o[0] + OWNER_INSET <= a.x0 and a.x1 <= o[2] - OWNER_INSET
                        and o[1] + OWNER_INSET <= a.y0
                        and a.y1 <= o[3] - OWNER_INSET):
                    v.append("R2 text %r leaves its own box %s: run "
                             "(%.0f,%.0f)-(%.0f,%.0f) vs box "
                             "(%.0f,%.0f)-(%.0f,%.0f)"
                             % (a.s, a.owner, a.x0, a.y0, a.x1, a.y1, *o))
            skip = set([a.owner] if a.owner else []) | set(
                self._ancestors(a.owner) if a.owner else [])
            for r in self.rects:
                if r.rid in skip:
                    continue
                if any(_boxes_overlap(ab, o) for o in obstacles(r)):
                    v.append("R3 text %r sits on box %s (%.0f,%.0f)-(%.0f,%.0f)"
                             % (a.s, r.rid, *r.box))

        # R4 — text against connector segments.
        for a in self.runs:
            ab = pad_text(a.box)
            for s in self.segs:
                if _seg_hits_box(s.x1, s.y1, s.x2, s.y2, ab):
                    v.append("R4 text %r sits on connector %s segment "
                             "(%.0f,%.0f)-(%.0f,%.0f)"
                             % (a.s, s.eid, s.x1, s.y1, s.x2, s.y2))

        # R5 — arrowheads land on a border, and not inside a label.
        for hx, hy, dx, dy, eid, dst in self.heads:
            if dst is None or dst not in by_id:
                v.append("R5 connector %s points at nothing nameable" % eid)
                continue
            d = _border_distance(hx, hy, by_id[dst])
            if d > 1.0:
                v.append("R5 connector %s ends %.1f px away from the border of "
                         "%s" % (eid, d, dst))
            tri = (min(hx, hx - dx * HEAD_LEN) - HEAD_HALF,
                   min(hy, hy - dy * HEAD_LEN) - HEAD_HALF,
                   max(hx, hx - dx * HEAD_LEN) + HEAD_HALF,
                   max(hy, hy - dy * HEAD_LEN) + HEAD_HALF)
            for a in self.runs:
                if _boxes_overlap(tri, a.box):
                    v.append("R5 arrowhead of %s lands inside text %r"
                             % (eid, a.s))

        # R6 — segments through nodes they are not attached to.
        for s in self.segs:
            for r in self.rects:
                if r.kind != "node":
                    continue
                allowed = {s.src, s.dst}
                allowed |= set(self._ancestors(s.src)) if s.src else set()
                allowed |= set(self._ancestors(s.dst)) if s.dst else set()
                if r.rid in allowed:
                    continue
                ln = _seg_box_length(s.x1, s.y1, s.x2, s.y2, r.box)
                if ln > 0.5:
                    v.append("R6 connector %s runs %.0f px through box %s"
                             % (s.eid, ln, r.rid))

        # R7 — two connectors sharing a line.
        for i, a in enumerate(self.segs):
            for b in self.segs[i + 1:]:
                if a.eid == b.eid:
                    continue
                if _collinear_overlap(a, b) > 4.0:
                    v.append("R7 connectors %s and %s share %.0f px of one line"
                             % (a.eid, b.eid, _collinear_overlap(a, b)))
        return v

    # -- output ------------------------------------------------------------
    def render(self, palette, *, standalone=True):
        p = PALETTES[palette]
        # Four of these end up inlined into one HTML page, so every id has to
        # carry the diagram's name: two elements with id "a" would send every
        # url(#a) in the document to whichever one the parser saw first, and
        # all four diagrams would draw pillar 1's arrowhead.
        tid = "dg-title-%s" % (self.dom_id or "diagram")
        head = ('<marker id="%s" viewBox="0 0 10 10" refX="9" refY="5" '
                'markerWidth="7" markerHeight="7" orient="auto-start-reverse">'
                '<path d="M 0 0 L 10 5 L 0 10 z" fill="{{arrow}}"/></marker>'
                % self._head_id)
        size = (' width="%s" height="%s"' % (_fmt(self.w), _fmt(self.h)))
        cls = ' class="diagram-svg"' if not standalone else ""
        body = (
            '<svg xmlns="http://www.w3.org/2000/svg"%s viewBox="0 0 %s %s"%s '
            'role="img" aria-labelledby="%s" preserveAspectRatio="xMidYMid meet">'
            '<title id="%s">%s</title>'
            '<defs>%s</defs>'
            '<rect width="%s" height="%s" rx="%s" fill="{{bg}}"/>'
            % (cls, _fmt(self.w), _fmt(self.h), size, tid, tid, esc(self.alt),
               head, _fmt(self.w), _fmt(self.h), "0" if standalone else "10")
            + "".join(self.parts) + "</svg>")
        return re.sub(r"\{\{(\w+)\}\}", lambda m: p[m.group(1)], body)


def _round_path(points):
    """An orthogonal route with 6 px rounded corners — gentle enough to read as
    one line, tight enough that the corner never leaves the segment the checker
    measured."""
    r = 6.0
    if len(points) == 2:
        return "M %s %s L %s %s" % (_fmt(points[0][0]), _fmt(points[0][1]),
                                    _fmt(points[1][0]), _fmt(points[1][1]))
    d = ["M %s %s" % (_fmt(points[0][0]), _fmt(points[0][1]))]
    for i in range(1, len(points) - 1):
        (x0, y0), (x1, y1), (x2, y2) = points[i - 1], points[i], points[i + 1]
        a = math.hypot(x1 - x0, y1 - y0)
        b = math.hypot(x2 - x1, y2 - y1)
        ra = min(r, a / 2, b / 2)
        ax, ay = x1 - (x1 - x0) / (a or 1) * ra, y1 - (y1 - y0) / (a or 1) * ra
        bx, by = x1 + (x2 - x1) / (b or 1) * ra, y1 + (y2 - y1) / (b or 1) * ra
        d.append("L %s %s" % (_fmt(ax), _fmt(ay)))
        d.append("Q %s %s %s %s" % (_fmt(x1), _fmt(y1), _fmt(bx), _fmt(by)))
    d.append("L %s %s" % (_fmt(points[-1][0]), _fmt(points[-1][1])))
    return " ".join(d)


_COLOUR_ATTR = re.compile(r'(fill|stroke|stop-color)="[^"]*"')


def same_geometry(a, b):
    """True when two renders differ only in colour — the assertion the light
    and dark pair has to satisfy before either is written."""
    return (_COLOUR_ATTR.sub(r'\1="#"', a) == _COLOUR_ATTR.sub(r'\1="#"', b))


# The ladder a connector caption walks: every horizontal run of the route
# first, longest first, because a caption beside a vertical line is ambiguous
# wherever two verticals run parallel; then outwards from the line in seven-
# pixel steps, because the corridor between two boxes is usually narrower than
# the caption and the empty row gap above or below it is not.
LABEL_FRACTIONS = (0.5, 0.34, 0.66, 0.2, 0.8)
LABEL_OFFSETS = (5.0, 12.0, 19.0, 26.0, 33.0, 40.0, 47.0, 54.0, 61.0, 68.0, 75.0)
LABEL_SIZE, LABEL_LEADING = 10.5, 13.0


def label_candidates(points, lines):
    h = (len(lines) - 1) * LABEL_LEADING + LABEL_SIZE
    segs = sorted(((abs(b[0] - a[0]) + abs(b[1] - a[1]), a, b)
                   for a, b in zip(points, points[1:])),
                  key=lambda s: (abs(s[1][1] - s[2][1]) > 0.5, -s[0]))
    out = []
    for d in LABEL_OFFSETS:
        for length, a, b in segs:
            if length < 24:
                continue
            for f in LABEL_FRACTIONS:
                mx = a[0] + (b[0] - a[0]) * f
                my = a[1] + (b[1] - a[1]) * f
                if abs(a[1] - b[1]) < 0.5:
                    out.append((mx, my - d - h, "middle"))
                    out.append((mx, my + d, "middle"))
                else:
                    out.append((a[0] + d + 3, my - h / 2, "start"))
                    out.append((a[0] - d - 3, my - h / 2, "end"))
    return out


def place_connector_label(canvas, points, text, what, wraps=(320, 200, 130)):
    """Draw a connector's caption beside its own line, at the first candidate
    that collides with nothing already on the canvas. Narrower wraps are tried
    only when the widest one finds no room anywhere."""
    for width in wraps:
        lines = wrap_px(text, width, LABEL_SIZE, 400)
        for x, top, anchor in label_candidates(points, lines):
            _, bad = canvas.would_collide(lines, x, top, size=LABEL_SIZE,
                                          weight=400, anchor=anchor,
                                          leading=LABEL_LEADING)
            if not bad:
                return canvas.text(lines, x, top, size=LABEL_SIZE, weight=400,
                                   role="note", anchor=anchor,
                                   leading=LABEL_LEADING)
    raise OverlapError("no free position for the label %r on %s" % (text, what))


class OverlapError(RuntimeError):
    pass


def write_svgs(canvas, name, out_dirs, *, site_targets=()):
    """Check, then write. `out_dirs` receive `<name>-light.svg` and
    `<name>-dark.svg`; `site_targets` are HTML files carrying a
    `<!-- diagram:NAME -->` … `<!-- /diagram:NAME -->` pair, whose contents are
    replaced with the theme-neutral inline copy."""
    violations = canvas.check()
    if violations:
        raise OverlapError(
            "%s: %d overlap violation(s)\n  " % (name, len(violations))
            + "\n  ".join(violations))
    light = canvas.render("light")
    dark = canvas.render("dark")
    if not same_geometry(light, dark):
        raise OverlapError("%s: light and dark differ by more than colour" % name)
    written = []
    for out in out_dirs:
        os.makedirs(out, exist_ok=True)
        for theme, body in (("light", light), ("dark", dark)):
            path = os.path.join(out, "%s-%s.svg" % (name, theme))
            with open(path, "w", encoding="utf-8", newline="\n") as f:
                f.write(body)
            written.append(path)
    inline = canvas.render("site", standalone=False)
    for target in site_targets:
        written += _splice(target, name, inline)
    return written, len(canvas.runs), len(canvas.segs)


_MARK = "<!-- diagram:%s -->"
_ENDMARK = "<!-- /diagram:%s -->"


def _splice(path, name, inline):
    """Replace what sits between the two markers, byte for byte, keeping the
    file's own line ending and the marker line's own indent."""
    with open(path, "rb") as f:
        data = f.read()
    crlf = b"\r\n" in data
    open_m = _MARK % name
    close_m = _ENDMARK % name
    text = data.decode("utf-8").replace("\r\n", "\n")
    if text.count(open_m) != 1 or text.count(close_m) != 1:
        raise OverlapError("%s: expected exactly one %s / %s marker pair"
                           % (path, open_m, close_m))
    i = text.index(open_m)
    j = text.index(close_m)
    if j < i:
        raise OverlapError("%s: %s closes before it opens" % (path, close_m))
    line_start = text.rfind("\n", 0, i) + 1
    indent = text[line_start:i]
    new = (text[:i + len(open_m)] + "\n" + indent + inline + "\n" + indent
           + text[j:])
    out = new.replace("\n", "\r\n") if crlf else new
    with open(path, "wb") as f:
        f.write(out.encode("utf-8"))
    return [path]
