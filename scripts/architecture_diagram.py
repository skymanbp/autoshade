#!/usr/bin/env python3
"""Draw the architecture picture from the architecture source.

`docs/architecture/autoshade.architecture.json` is the model of the program's
own parts: twenty components, three boundaries, nineteen connections. This
script is its only renderer — archify, which drew the PNG that used to sit in
the README, is not installable on this machine any more (`npm i -g git+…` finds
no package.json; the npm package of that name is a different project), and its
output put four edge labels on top of each other over the region border. So the
positions are computed here, and `scripts/diagram_check.py` refuses to write the
file when any two things touch.

The layout, in five steps:

  1. Column — longest path over the connection graph ranks every component, so
     each connection points rightwards. A component with nothing feeding it is
     then pulled right to one column before its earliest consumer, so a source
     sits beside what it serves instead of floating at the left edge.
  2. Band — each boundary in the JSON becomes a horizontal band. The region is
     the middle one by definition; a security group whose connections mostly
     leave it (a supplier) goes below, one that mostly receives goes above.
  3. Row — components sharing a column need different rows. Six alternating
     sweeps give every column the row assignment that minimises the squared
     distance to its neighbours' rows, chosen exhaustively per column because a
     column holds at most four boxes.
  4. Metric — every box takes the height its own wrapped text needs; a row takes
     the tallest box in it, a column the fixed box width plus a corridor.
  5. Route — each connection is an orthogonal polyline through the empty
     corridors between columns and bands, entering its target on a border where
     the approach is clear; each label takes the first position from a candidate
     list that collides with nothing already drawn.
"""
import itertools
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)

from diagram_check import (Canvas, node_metrics,
                           place_connector_label, text_width,
                           write_svgs)

SRC = os.path.join("docs", "architecture", "autoshade.architecture.json")
OUT_DIRS = ["docs/images"]
SITE_TARGETS = ["site/index.html", "site/architecture.html"]

NODE_W = 196.0
COL_GAP = 58.0
ROW_GAP = 46.0
BAND_GAP = 92.0
BAND_PAD = 20.0          # horizontal panel padding around its boxes
BAND_CAP = 34.0          # caption strip, kept free of boxes at BOTH panel edges
MARGIN = 32.0
EDGE = 12.0              # canvas margin outside the band panels
HEAD_H = 70.0
FOOT_H = 36.0
LANE = 20.0              # pitch of the long-edge corridor inside a band
ACCENTS = ("accent_blue", "accent_green", "accent_rust")
VLANE_OFFSETS = (0.0, -13.0, 13.0, -21.0, 21.0)
HLANE_OFFSETS = (0.0, -17.0, 17.0, -32.0, 32.0)


# ── 1. columns ─────────────────────────────────────────────────────────────
def rank_columns(order, edges):
    pred = {v: [] for v in order}
    succ = {v: [] for v in order}
    for e in edges:
        succ[e["from"]].append(e["to"])
        pred[e["to"]].append(e["from"])
    indeg = {v: len(pred[v]) for v in order}
    rank = {v: 0 for v in order}
    queue = [v for v in order if indeg[v] == 0]
    seen = 0
    while queue:
        v = queue.pop(0)
        seen += 1
        for s in succ[v]:
            rank[s] = max(rank[s], rank[v] + 1)
            indeg[s] -= 1
            if indeg[s] == 0:
                queue.append(s)
    assert seen == len(order), "the connection graph has a cycle"
    for v in order:
        if not pred[v] and succ[v]:
            rank[v] = min(rank[s] for s in succ[v]) - 1
    used = sorted(set(rank.values()))
    remap = {r: i for i, r in enumerate(used)}
    return {v: remap[rank[v]] for v in order}, pred, succ


# ── 2. bands ───────────────────────────────────────────────────────────────
def band_stack(bnds, edges, band_of):
    """Top-to-bottom order of the boundary panels."""
    net = []
    for i in range(len(bnds)):
        out = sum(1 for e in edges
                  if band_of[e["from"]] == i and band_of[e["to"]] != i)
        inn = sum(1 for e in edges
                  if band_of[e["to"]] == i and band_of[e["from"]] != i)
        net.append(out - inn)
    main = [i for i, b in enumerate(bnds) if b["kind"] == "region"]
    assert len(main) == 1, "exactly one boundary must be the region"
    m = main[0]
    above = sorted((i for i in range(len(bnds)) if i != m and net[i] <= 0),
                   key=lambda i: (net[i], i))
    below = sorted((i for i in range(len(bnds)) if i != m and net[i] > 0),
                   key=lambda i: (-net[i], i))
    return above + [m] + below


# ── 3. rows ────────────────────────────────────────────────────────────────
def assign_rows(members, col, internal, order, outside=None):
    cols = {}
    for v in members:
        cols.setdefault(col[v], []).append(v)
    nrows = max(len(x) for x in cols.values())
    row = {}
    for c in sorted(cols):
        for i, v in enumerate(cols[c]):
            row[v] = i
    if not internal or nrows == 1:
        # Nothing flows inside this band, so there is no order to show: it
        # becomes one row, and its boxes take their x from their neighbours.
        return {v: 0 for v in members}, 1
    nb = {v: [] for v in members}
    for a, b in internal:
        nb[a].append(b)
        nb[b].append(a)
    pull = {v: [] for v in members}
    for v, above in (outside or []):
        pull[v].append(-0.5 if above else nrows - 0.5)
    for sweep in range(6):
        for c in sorted(cols, reverse=bool(sweep % 2)):
            vs = cols[c]
            bary = []
            for v in vs:
                ns = [row[u] for u in nb[v] if col[u] != c] + pull[v]
                bary.append(sum(ns) / len(ns) if ns else float(row[v]))
            idx = sorted(range(len(vs)),
                         key=lambda i: (bary[i], order.index(vs[i])))
            vs = [vs[i] for i in idx]
            bary = [bary[i] for i in idx]
            cols[c] = vs
            best = min(itertools.combinations(range(nrows), len(vs)),
                       key=lambda s: sum((a - b) ** 2 for a, b in zip(s, bary)))
            for v, s in zip(vs, best):
                row[v] = s
    return row, nrows


# ── 4. metric ──────────────────────────────────────────────────────────────
def spread(wants, pitch, lo, hi):
    """Centres as close as possible to `wants` (already sorted), never closer
    than `pitch`, inside [lo, hi]. Isotonic regression: substituting
    u_i = c_i - i*pitch turns the separation constraint into "u must not
    decrease", which pool-adjacent-violators solves exactly."""
    blocks = []
    for i, w in enumerate(wants):
        blocks.append([w - i * pitch, 1])
        while (len(blocks) > 1
               and blocks[-2][0] / blocks[-2][1] > blocks[-1][0] / blocks[-1][1]):
            b = blocks.pop()
            blocks[-1][0] += b[0]
            blocks[-1][1] += b[1]
    u = []
    for s, n in blocks:
        u += [s / n] * n
    c = [ui + i * pitch for i, ui in enumerate(u)]
    for i in range(len(c)):
        c[i] = max(c[i], lo if i == 0 else c[i - 1] + pitch)
    for i in range(len(c) - 1, -1, -1):
        c[i] = min(c[i], hi if i == len(c) - 1 else c[i + 1] - pitch)
    assert all(c[i + 1] - c[i] >= pitch - 1e-6 for i in range(len(c) - 1)), (
        "the band is too narrow for its own boxes")
    return c


def layout(doc):
    comps = {c["id"]: c for c in doc["components"]}
    order = [c["id"] for c in doc["components"]]
    edges = doc["connections"]
    bnds = doc["boundaries"]

    band_of = {}
    for i, b in enumerate(bnds):
        for v in b["wraps"]:
            assert v in comps and v not in band_of, v
            band_of[v] = i
    assert set(band_of) == set(order), (
        "every component must sit in exactly one boundary")

    col, pred, succ = rank_columns(order, edges)
    stack = band_stack(bnds, edges, band_of)
    depth = {b: i for i, b in enumerate(stack)}
    ncols = max(col.values()) + 1
    col_x = [MARGIN + c * (NODE_W + COL_GAP) for c in range(ncols)]

    height = {v: node_metrics(NODE_W, comps[v]["label"], comps[v].get("sublabel"),
                              comps[v].get("tag"))[3] for v in order}

    rows, nrows = {}, {}
    for bi in range(len(bnds)):
        members = [v for v in order if band_of[v] == bi]
        internal = [(e["from"], e["to"]) for e in edges
                    if band_of[e["from"]] == bi and band_of[e["to"]] == bi]
        outside = []
        for e in edges:
            for near, far in ((e["from"], e["to"]), (e["to"], e["from"])):
                if band_of[near] == bi != band_of[far]:
                    outside.append((near, depth[band_of[far]] < depth[bi]))
        r, n = assign_rows(members, col, internal, order, outside)
        rows.update(r)
        nrows[bi] = n

    main = next(i for i, b in enumerate(bnds) if b["kind"] == "region")
    x = {v: col_x[col[v]] for v in order if band_of[v] == main}
    for bi in range(len(bnds)):
        if bi == main:
            continue
        members = [v for v in order if band_of[v] == bi]
        want = {}
        for v in members:
            ns = [u for u in succ[v] + pred[v] if band_of[u] == main]
            want[v] = (sum(x[u] + NODE_W / 2 for u in ns) / len(ns) if ns
                       else col_x[col[v]] + NODE_W / 2)
        members.sort(key=lambda v: (want[v], order.index(v)))
        centres = spread([want[v] for v in members], NODE_W + COL_GAP,
                         MARGIN + NODE_W / 2, col_x[-1] + NODE_W / 2)
        for v, c in zip(members, centres):
            x[v] = c - NODE_W / 2

    long_edges = {bi: [e for e in edges
                       if band_of[e["from"]] == bi == band_of[e["to"]]
                       and col[e["to"]] - col[e["from"]] >= 2]
                  for bi in range(len(bnds))}

    y, band_box, node_y, long_lane_y = MARGIN + HEAD_H, {}, {}, {}
    for bi in stack:
        members = [v for v in order if band_of[v] == bi]
        top = y
        cur = top + BAND_CAP
        for r in range(nrows[bi]):
            tall = max(height[v] for v in members if rows[v] == r)
            for v in members:
                if rows[v] == r:
                    node_y[v] = cur + (tall - height[v]) / 2
            cur += tall + ROW_GAP
        cur -= ROW_GAP
        n_long = len(long_edges[bi])
        long_lane_y[bi] = [cur + 14 + k * LANE for k in range(n_long)]
        bottom = cur + (n_long * LANE + 10 if n_long else 0) + BAND_CAP
        x0 = min(x[v] for v in members) - BAND_PAD
        x1 = max(x[v] for v in members) + NODE_W + BAND_PAD
        need = text_width(bnds[bi]["label"], 11.5, 700) + 32
        if x1 - x0 < need:
            mid = (x0 + x1) / 2
            x0, x1 = mid - need / 2, mid + need / 2
        if x0 < EDGE:
            x1, x0 = x1 + (EDGE - x0), EDGE
        band_box[bi] = (x0, top, x1, bottom)
        y = bottom + BAND_GAP

    width = max(b[2] for b in band_box.values()) + EDGE
    height_px = y - BAND_GAP + FOOT_H + MARGIN
    return dict(comps=comps, order=order, edges=edges, bnds=bnds, col=col,
                ncols=ncols,
                col_x=col_x, band_of=band_of, stack=stack, depth=depth,
                rows=rows, nrows=nrows, x=x, y=node_y, h=height,
                band_box=band_box, long_edges=long_edges,
                long_lane_y=long_lane_y, pred=pred, succ=succ,
                w=width, hgt=height_px, main=main)


# ── 5. routes ──────────────────────────────────────────────────────────────
class Lanes:
    """The claim book for connector segments. Every horizontal run claims a
    span on its own y line and every vertical run a span on its own x line, so
    two connectors can never end up drawn on top of each other — which is R7,
    enforced while routing instead of only reported afterwards."""

    PAD = 8.0

    def __init__(self):
        self.claims = {}

    def free(self, key, a, b):
        a, b = min(a, b), max(a, b)
        return all(b + self.PAD < s or a - self.PAD > e
                   for s, e in self.claims.get(key, []))

    def claim(self, key, a, b):
        self.claims.setdefault(key, []).append((min(a, b), max(a, b)))

    @staticmethod
    def keys(points):
        for a, b in zip(points, points[1:]):
            if abs(a[1] - b[1]) < 0.5 and abs(a[0] - b[0]) > 0.5:
                yield ("y", round(a[1], 1)), a[0], b[0]
            elif abs(a[0] - b[0]) < 0.5 and abs(a[1] - b[1]) > 0.5:
                yield ("x", round(a[0], 1)), a[1], b[1]

    def fits(self, points):
        return all(self.free(k, a, b) for k, a, b in self.keys(points))

    def commit(self, points):
        for k, a, b in self.keys(points):
            self.claim(k, a, b)


def _clean(points):
    out = []
    for p in points:
        if out and abs(out[-1][0] - p[0]) < 1e-6 and abs(out[-1][1] - p[1]) < 1e-6:
            continue
        out.append(p)
    i = 1
    while i < len(out) - 1:
        (ax, ay), (bx, by), (cx, cy) = out[i - 1], out[i], out[i + 1]
        if abs((bx - ax) * (cy - ay) - (by - ay) * (cx - ax)) < 1e-6:
            del out[i]
        else:
            i += 1
    return out


class Router:
    def __init__(self, L):
        self.L = L
        self.lanes = Lanes()
        self.side = {}
        self.attach = {}

    def blocked(self, ids, x0, x1, y0, y1):
        """Does any box other than `ids` sit in this window?"""
        L = self.L
        y0, y1 = min(y0, y1), max(y0, y1)
        for v in L["order"]:
            if v in ids:
                continue
            vx, vy, vh = L["x"][v], L["y"][v], L["h"][v]
            if vx < x1 and x0 < vx + NODE_W and vy < y1 and y0 < vy + vh:
                return True
        return False

    def decide_sides(self):
        L = self.L
        for e in L["edges"]:
            s, t = e["from"], e["to"]
            if L["band_of"][s] == L["band_of"][t]:
                d = L["col"][t] - L["col"][s]
                if d >= 2:
                    ss, ts = "B", "L"
                    if self.blocked({s}, L["x"][s], L["x"][s] + NODE_W,
                                    L["y"][s] + L["h"][s],
                                    L["band_box"][L["band_of"][s]][3]):
                        ss = "R" if L["col"][s] + 1 < L["ncols"] else "L"
                elif d == 1:
                    ss, ts = "R", "L"
                elif d == 0:
                    ss, ts = ("B", "T") if L["rows"][t] > L["rows"][s] else ("T", "B")
                else:
                    ss, ts = "L", "R"
            else:
                up = L["depth"][L["band_of"][t]] < L["depth"][L["band_of"][s]]
                assert abs(L["depth"][L["band_of"][t]]
                           - L["depth"][L["band_of"][s]]) == 1, (
                    "a connection crosses a band it does not touch")
                gap = self.band_gap(L["band_of"][s], L["band_of"][t])
                ss = "T" if up else "B"
                sy0 = L["y"][s] if up else L["y"][s] + L["h"][s]
                if self.blocked({s}, L["x"][s], L["x"][s] + NODE_W, sy0, gap):
                    ss = "R" if L["col"][s] + 1 < L["ncols"] else "L"
                ts = "B" if up else "T"
                ty0 = L["y"][t] + L["h"][t] if up else L["y"][t]
                ts = ts if not self.blocked(
                    {t}, L["x"][t], L["x"][t] + NODE_W, ty0, gap) else "L"
                if ts == "L" and L["col"][t] == 0:
                    ts = "R"
            self.side[e["id"]] = (ss, ts)

    def band_gap(self, a, b):
        """Mid-height of the empty strip between two adjacent band panels."""
        L = self.L
        lo, hi = sorted((a, b), key=lambda i: L["depth"][i])
        return (L["band_box"][lo][3] + L["band_box"][hi][1]) / 2

    def fan(self):
        """Spread the connectors that share one border of one box, ordered by
        where their other end sits, so three arrows into one box arrive at
        three points instead of one."""
        L = self.L
        groups = {}
        for e in L["edges"]:
            ss, ts = self.side[e["id"]]
            groups.setdefault((e["from"], ss), []).append((e, "out"))
            groups.setdefault((e["to"], ts), []).append((e, "in"))
        for (v, side), items in sorted(groups.items()):
            def other(item, side=side):
                e, way = item
                u = e["to"] if way == "out" else e["from"]
                return (L["y"][u] + L["h"][u] / 2 if side in "LR"
                        else L["x"][u] + NODE_W / 2)
            items = sorted(items, key=lambda it: (other(it), it[0]["id"]))
            n = len(items)
            for i, (e, way) in enumerate(items):
                f = (i + 1) / (n + 1)
                if side == "L":
                    p = (L["x"][v], L["y"][v] + L["h"][v] * f)
                elif side == "R":
                    p = (L["x"][v] + NODE_W, L["y"][v] + L["h"][v] * f)
                elif side == "T":
                    p = (L["x"][v] + NODE_W * f, L["y"][v])
                else:
                    p = (L["x"][v] + NODE_W * f, L["y"][v] + L["h"][v])
                self.attach[(e["id"], way)] = p

    def vlanes(self, c):
        centre = self.L["col_x"][c] + NODE_W + COL_GAP / 2
        return [centre + o for o in VLANE_OFFSETS]

    def choose(self, candidates):
        """The first polyline that shares no line with one already drawn. When
        every candidate is taken the first is used anyway and the checker says
        so by name — a silent nudge would hide a corridor that has run out."""
        for pts in candidates:
            pts = _clean(pts)
            if self.lanes.fits(pts):
                self.lanes.commit(pts)
                return pts
        pts = _clean(candidates[0])
        self.lanes.commit(pts)
        return pts

    def route(self, e):
        L = self.L
        s, t = e["from"], e["to"]
        ss, ts = self.side[e["id"]]
        p0 = self.attach[(e["id"], "out")]
        p1 = self.attach[(e["id"], "in")]
        if L["band_of"][s] == L["band_of"][t]:
            d = L["col"][t] - L["col"][s]
            if d == 1 and abs(p0[1] - p1[1]) < 0.5:
                return self.choose([[p0, p1]])
            if d == 1:
                return self.choose([[p0, (lx, p0[1]), (lx, p1[1]), p1]
                                    for lx in self.vlanes(L["col"][s])])
            if d >= 2:
                bi = L["band_of"][s]
                ly = L["long_lane_y"][bi][L["long_edges"][bi].index(e)]
                out = []
                for cx in self.vlanes(L["col"][t] - 1):
                    if ss == "R":
                        for ex in self.vlanes(L["col"][s]):
                            out.append([p0, (ex, p0[1]), (ex, ly), (cx, ly),
                                        (cx, p1[1]), p1])
                    else:
                        out.append([p0, (p0[0], ly), (cx, ly), (cx, p1[1]), p1])
                return self.choose(out)
            return self.choose([[p0, p1]])
        gap = self.band_gap(L["band_of"][s], L["band_of"][t])
        starts = ([[p0]] if ss in "TB" else
                  [[p0, (ex, p0[1])] for ex in
                   self.vlanes(L["col"][s] if ss == "R" else L["col"][s] - 1)])
        ends = ([[p1]] if ts in "TB" else
                [[(cx, p1[1]), p1] for cx in
                 self.vlanes(L["col"][t] - 1 if ts == "L" else L["col"][t])])
        out = []
        for gy in [gap + o for o in HLANE_OFFSETS]:
            for a in starts:
                for b in ends:
                    sx = a[-1][0]
                    tx = b[0][0]
                    out.append(a + [(sx, gy), (tx, gy)] + b)
        return self.choose(out)


# ── drawing ────────────────────────────────────────────────────────────────
def draw(L):
    doc_title = "AutoShade — the architecture, drawn from its own source"
    alt = ("AutoShade architecture: three front ends over one Rust library "
           "holding RAW decode, the AI advisor, the recipe, the render engine, "
           "the style index, reverse-fit, the local producers and the "
           "local-field analyzer; five local Python sidecars below it; two "
           "opt-in external AI services above it")
    c = Canvas(L["w"], L["hgt"], alt, dom_id="architecture")
    kinds = {}
    for comp in L["comps"].values():
        kinds[comp["type"]] = kinds.get(comp["type"], 0) + 1
    counted = ", ".join("%d %s" % (n, k) for k, n in kinds.items())
    c.heading(EDGE + 6, MARGIN - 12, doc_title,
              "%d components (%s) · %d connections · %d boundaries, all read "
              "from docs/architecture/autoshade.architecture.json"
              % (len(L["comps"]), counted, len(L["edges"]), len(L["bnds"])))

    accent_of = {}
    for i, bi in enumerate(L["stack"]):
        accent_of[bi] = ACCENTS[i % len(ACCENTS)]
    for bi in L["stack"]:
        x0, y0, x1, y1 = L["band_box"][bi]
        c.group(x0, y0, x1 - x0, y1 - y0, accent_of[bi], rid="band-%d" % bi)
    for v in L["order"]:
        comp = L["comps"][v]
        c.node(L["x"][v], L["y"][v], NODE_W, L["h"][v], title=comp["label"],
               sub=comp.get("sublabel"), tag=comp.get("tag"),
               accent=accent_of[L["band_of"][v]],
               kind="accent" if comp["type"] == "external" else "box",
               rid=v, parent="band-%d" % L["band_of"][v])

    router = Router(L)
    router.decide_sides()
    router.fan()
    routes = {}
    for e in L["edges"]:
        pts = router.route(e)
        routes[e["id"]] = pts
        c.connector(pts, src=e["from"], dst=e["to"], eid=e["id"],
                    dashed=e.get("variant") == "security")
    for bi in L["stack"]:
        c.group_label("band-%d" % bi, L["bnds"][bi]["label"], accent_of[bi],
                      cap=BAND_CAP)
    for e in L["edges"]:
        if e.get("label"):
            place_connector_label(c, routes[e["id"]], e["label"], e["id"])
    c.note(EDGE + 6, L["hgt"] - MARGIN - 12,
           ["Every box is a directory or a module in this repository; every "
            "arrow is a call that exists. Generated by "
            "scripts/architecture_diagram.py — no position in this picture was "
            "chosen by hand."])
    return c


def main():
    os.chdir(ROOT)
    with open(SRC, encoding="utf-8") as f:
        doc = json.load(f)
    L = layout(doc)
    canvas = draw(L)
    written, runs, segs = write_svgs(canvas, "architecture", OUT_DIRS,
                                     site_targets=SITE_TARGETS)
    print("architecture %.0fx%.0f — %d text runs, %d connector segments, "
          "0 overlaps" % (L["w"], L["hgt"], runs, segs))
    for p in written:
        print(" ", p, os.path.getsize(p), "B")


if __name__ == "__main__":
    main()
