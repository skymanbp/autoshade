#!/usr/bin/env python3
"""Rectangle and segment predicates, with no idea what a diagram is.

`scripts/diagram_check.py` writes its overlap rules in terms of these, and
keeping them apart is what lets each one be read on its own: `pad_text` is the
only place the checker's two margins are applied, `obstacles` the only place a
group panel is treated as a container rather than a solid, and the two
Liang-Barsky clippers the only place a segment meets a box.
"""
import math

TEXT_MARGIN = 2.0        # inflation of a text run, per side, in px
CROSS_FACE = 0.025       # extra horizontal allowance, as a share of the run


def _inflate(b, m):
    return (b[0] - m, b[1] - m, b[2] + m, b[3] + m)


def pad_text(b):
    """A text run's box as the checker sees it: the fixed margin on all four
    sides, plus CROSS_FACE of its own width left and right for the faces this
    stack resolves to on the other two platforms."""
    m = TEXT_MARGIN + CROSS_FACE * (b[2] - b[0])
    return (b[0] - m, b[1] - TEXT_MARGIN, b[2] + m, b[3] + TEXT_MARGIN)


def obstacles(r):
    """What a text run has to keep off. A node or a tag pill is solid — nothing
    may sit on it. A group panel is a container: its interior is exactly where
    its own nodes and the connector labels between them live, so only its
    dashed outline, two pixels either side, counts as an obstacle."""
    x0, y0, x1, y1 = r.box
    if r.kind != "group":
        return [(x0, y0, x1, y1)]
    t = 2.0
    return [(x0 - t, y0 - t, x1 + t, y0 + t), (x0 - t, y1 - t, x1 + t, y1 + t),
            (x0 - t, y0 - t, x0 + t, y1 + t), (x1 - t, y0 - t, x1 + t, y1 + t)]


def _boxes_overlap(a, b):
    return a[0] < b[2] and b[0] < a[2] and a[1] < b[3] and b[1] < a[3]


def _seg_hits_box(x1, y1, x2, y2, b):
    """Liang-Barsky: does the segment touch the closed box at all?"""
    x0, y0, x3, y3 = b
    dx, dy = x2 - x1, y2 - y1
    t0, t1 = 0.0, 1.0
    for p, q in ((-dx, x1 - x0), (dx, x3 - x1), (-dy, y1 - y0), (dy, y3 - y1)):
        if p == 0:
            if q < 0:
                return False
            continue
        r = q / p
        if p < 0:
            if r > t1:
                return False
            t0 = max(t0, r)
        else:
            if r < t0:
                return False
            t1 = min(t1, r)
    return t0 <= t1


def _seg_box_length(x1, y1, x2, y2, b):
    """Length of the part of the segment that lies inside the box."""
    x0, y0, x3, y3 = b
    dx, dy = x2 - x1, y2 - y1
    t0, t1 = 0.0, 1.0
    for p, q in ((-dx, x1 - x0), (dx, x3 - x1), (-dy, y1 - y0), (dy, y3 - y1)):
        if p == 0:
            if q < 0:
                return 0.0
            continue
        r = q / p
        if p < 0:
            if r > t1:
                return 0.0
            t0 = max(t0, r)
        else:
            if r < t0:
                return 0.0
            t1 = min(t1, r)
    if t1 <= t0:
        return 0.0
    return (t1 - t0) * math.hypot(dx, dy)


def _border_distance(px, py, r):
    """Distance from a point to the border (not the interior) of a rectangle."""
    x0, y0, x1, y1 = r.box
    inside = x0 <= px <= x1 and y0 <= py <= y1
    d = min(abs(px - x0), abs(px - x1), abs(py - y0), abs(py - y1))
    if inside:
        return d
    dx = max(x0 - px, 0, px - x1)
    dy = max(y0 - py, 0, py - y1)
    return math.hypot(dx, dy)


def _collinear_overlap(a, b):
    """Length shared by two segments that lie on the same line, else 0."""
    ax, ay = a.x2 - a.x1, a.y2 - a.y1
    bx, by = b.x2 - b.x1, b.y2 - b.y1
    if abs(ax * by - ay * bx) > 1e-6:
        return 0.0
    cx, cy = b.x1 - a.x1, b.y1 - a.y1
    if abs(ax * cy - ay * cx) > 1e-6:
        return 0.0
    n = math.hypot(ax, ay)
    if n < 1e-9:
        return 0.0
    ux, uy = ax / n, ay / n
    pa = sorted((0.0, n))
    tb = sorted(((b.x1 - a.x1) * ux + (b.y1 - a.y1) * uy,
                 (b.x2 - a.x1) * ux + (b.y2 - a.y1) * uy))
    return max(0.0, min(pa[1], tb[1]) - max(pa[0], tb[0]))
