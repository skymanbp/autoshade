# -*- coding: utf-8 -*-
"""Acceptance measure for the night reference pair p42 (the star-field frame of the
calibration corpus, whose target was made in the app, not in Lightroom).

usage: python -X utf8 scripts/night_pair_measure.py <candidate> [--target T] [--sky-mask M]
                                                    [--out report.json] [--baseline]

T defaults to <corpus>/p42-target.png and M to <corpus>/mask-region-2-sky.png (the app's
own sky bitmap of that fit), <corpus> being $AUTOSHADE_FIT_CALIBRATION_DIR. The round-trip
gate (scripts/roundtrip_gate.py) imports `measure` from here for p42's own lines.

Every number below reproduces the v1.13.0 final check's scripts (score_sky, fc_score,
wb_regions, star_contrast; 2026-10-19..21), re-implemented to the letter for v1.17.0. Lab
is sRGB D65 with CIE76 distance; the originals used the same formula with 4-digit matrix
entries (difference < 0.01 dE on this pair, checked).

LINE 1 -- whole-frame dE (threshold 5.31). fc_score.py: "mean CIE76 deltaE of each render
  against its target (both resized to 480 px long edge)", Lanczos. 5.31 is fc_score's
  "night sky" number for fc-v7 (2026-10-22). Reported with the same frame split into
  sky / ground by the sky mask. Also the dE with the candidate Lanczos-resampled to the
  target's own size (information only).

LINE 2 -- horizon-glow ripple (threshold 0.63). score_sky.py, verbatim method: analysis
  frame 384x256 (Pillow BILINEAR resize of each file); sky = mask >= 0.95 ("settled");
  rows 100..196 of that frame, keeping a row only when settled[y].sum()*8 >= 384; per row
  the mean Lab of the settled pixels, candidate and target; err = cand - target (L, a, b);
  "ripple = max |error - mean error| over rows 100..196" -- the max runs over the rows AND
  the three channels (np.abs(err - err.mean(0)).max()). It is a measure RELATIVE to the
  target: the target scored against itself is 0 by construction. Also printed:
  max|err| = max row norm of err, and score_sky's mask-weighted sky dE on the same frame.

LINE 3 -- green horizon (rule: candidate a* more than 2 below the target's). The band is
  wb_regions.py's "horizon" band: 768x512 Lanczos frame, sky pixels (mask > 0.5) less
  than 0.18 x frame height above the first non-sky row of their own column. Mean a* of
  that band, candidate and target. (wb_regions read r/g, b/g medians there; a* mean is the
  rule the lane asked for.)

INFO -- star contrast (not a line; ruled out 2026-10-21: the target's starlight is
  painted). star_contrast.py's reading at the target's size: L* (luminance only), local
  maxima of a 5x5 maximum filter inside the sky, background = 21-px median; the 200
  maxima with the highest target peak L*; per star (peak5x5 - median21) of candidate over
  that of target; median ratio reported.

SKY MASK. The app-fitted recipe does NOT embed its masks: its two masks are bitmaps
  ("mask-region-2-sky.png" and the mountain one) kept beside the corpus fixture. --sky-mask
  takes such a bitmap (any size, resized to each frame) and defaults to the sky one when it
  is in the corpus; with --sky-mask none the mask is DERIVED from the target:
  the horizon row is the strongest downward step of the row-mean L* (median-filtered
  columns, 384x256 frame) in the lower half; above it, silhouette pixels (L* of a 5x5
  median below half the sky's row median) connected to the ground are removed. The
  report records which mask was used. Checked 2026-10-27: on p42-target the derived mask
  has IoU 0.995 with the app's sky bitmap and moves every line by <= 0.04.

TARGET. 5.31 and 0.63 were scored against the app's "adjust-2" variant of this frame,
  which is what <corpus>/p42-target.png holds; the "reimagine" variant beside it
  (p42-target-reimagine.png) is a different image and not a target.
"""
import argparse
import json
import os
import sys

import numpy as np
from PIL import Image
from scipy.ndimage import label, maximum_filter, median_filter

CORPUS = os.environ.get('AUTOSHADE_FIT_CALIBRATION_DIR', '')
DEFAULT_TARGET = os.path.join(CORPUS, 'p42-target.png')
DEFAULT_SKY_MASK = os.path.join(CORPUS, 'mask-region-2-sky.png')

DE_LINE, RIPPLE_LINE, GREEN_MARGIN = 5.31, 0.63, 2.0
FC_LONG = 480                       # fc_score frame
SKY_W, SKY_H = 384, 256             # score_sky analysis frame
SKY_ROWS = range(100, 197)          # score_sky rows 100..196
HZ_W, HZ_H, HZ_FRAC = 768, 512, 0.18  # wb_regions horizon band
N_STARS = 200

M = np.array([[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]])


def lab_arr(rgb):
    """cmp_lab.lab() on a float RGB array in 0..1."""
    lin = np.where(rgb <= 0.04045, rgb / 12.92, ((rgb + 0.055) / 1.055) ** 2.4)
    xyz = lin @ M.T / np.array([0.95047, 1.0, 1.08883])
    f = np.where(xyz > 0.008856, np.cbrt(xyz), 7.787 * xyz + 16 / 116)
    return np.stack([116 * f[..., 1] - 16, 500 * (f[..., 0] - f[..., 1]), 200 * (f[..., 1] - f[..., 2])], -1)


def lab(im, size, resample):
    return lab_arr(np.asarray(im.resize(size, resample), np.float64) / 255.0)


def lstar(im, size, resample):
    a = np.asarray(im.resize(size, resample), np.float64) / 255.0
    y = np.where(a <= 0.04045, a / 12.92, ((a + 0.055) / 1.055) ** 2.4) @ M[1]
    f = np.where(y > 0.008856, np.cbrt(y), 7.787 * y + 16 / 116)
    return 116 * f - 16


def mask_at(mask, size):
    """Sky weight 0..1 at a frame size (float mask, bilinear like score_sky)."""
    return np.asarray(Image.fromarray((mask * 255).round().astype(np.uint8)).resize(size, Image.BILINEAR), np.float64) / 255


def derive_sky(target):
    """Sky mask from the target alone (see docstring). Returns (mask 384x256 in 0/1, horizon row)."""
    l = lstar(target, (SKY_W, SKY_H), Image.BILINEAR)
    lm = median_filter(l, size=5)
    prof = np.median(lm, axis=1)
    step = prof[:-2] - prof[2:]                  # drop from row y to row y+2 (sky above, ground below)
    lo = SKY_H // 2
    hz = lo + int(np.argmax(step[lo:])) + 1     # last-ish sky row; ground starts below
    sky = np.zeros_like(l, bool)
    sky[:hz] = True
    sky_med = np.array([np.median(lm[y]) for y in range(SKY_H)])
    dark = lm < 0.5 * sky_med[:, None]
    dark[hz:] = True
    lab_, _ = label(dark)
    ground_ids = np.unique(lab_[hz:])
    silhouette = np.isin(lab_, ground_ids[ground_ids > 0])
    sky &= ~silhouette
    return sky.astype(np.float64), hz


def measure(cand_path, target_path, sky_mask_path=None):
    cand = Image.open(cand_path).convert('RGB')
    tgt = Image.open(target_path).convert('RGB')
    if sky_mask_path:
        mask_src = np.asarray(Image.open(sky_mask_path).convert('L'), np.float64) / 255
        mask_note = 'bitmap given with --sky-mask (%dx%d)' % (mask_src.shape[1], mask_src.shape[0])
        hz_row = None
    else:
        mask_src, hz_row = derive_sky(tgt)
        mask_note = 'derived from the target: horizon at row %d of %d, silhouettes removed' % (hz_row, SKY_H)
    rep = {'candidate': os.path.abspath(cand_path), 'target': os.path.abspath(target_path),
           'candidate_size': list(cand.size), 'target_size': list(tgt.size), 'sky_mask': mask_note}

    # 1. whole-frame dE at the fc_score frame (480 long edge, Lanczos), split by the sky mask
    w, h = cand.size
    fc = (FC_LONG, round(FC_LONG * h / w)) if w >= h else (round(FC_LONG * w / h), FC_LONG)
    d = np.linalg.norm(lab(cand, fc, Image.LANCZOS) - lab(tgt, fc, Image.LANCZOS), axis=-1)
    sk = mask_at(mask_src, fc) >= 0.5
    full = np.linalg.norm(lab(cand, tgt.size, Image.LANCZOS) - lab_arr(np.asarray(tgt, np.float64) / 255), axis=-1)
    rep['dE'] = {'frame': 'fc_score: both Lanczos to %dx%d, CIE76' % fc, 'whole': float(d.mean()),
                 'sky': float(d[sk].mean()), 'ground': float(d[~sk].mean()), 'sky_share': float(sk.mean()),
                 'whole_at_target_size': float(full.mean())}

    # 2. score_sky ripple
    msk = mask_at(mask_src, (SKY_W, SKY_H))
    settled = msk >= 0.95
    lt = lab(tgt, (SKY_W, SKY_H), Image.BILINEAR)
    la = lab(cand, (SKY_W, SKY_H), Image.BILINEAR)
    rows = [y for y in SKY_ROWS if settled[y].sum() * 8 >= SKY_W]
    if not rows:
        sys.exit('no row of %d..%d has 1/8 of its width in the settled sky' % (SKY_ROWS[0], SKY_ROWS[-1]))
    pt = np.array([lt[y, settled[y]].mean(0) for y in rows])
    pa = np.array([la[y, settled[y]].mean(0) for y in rows])
    err = pa - pt
    dev = np.abs(err - err.mean(0))
    iy, ic = np.unravel_index(int(np.argmax(dev)), dev.shape)
    rep['ripple'] = {
        'frame': 'score_sky: BILINEAR to %dx%d, sky = mask >= 0.95' % (SKY_W, SKY_H),
        'band_rows': [rows[0], rows[-1]], 'rows_used': len(rows), 'rows_dropped': [y for y in SKY_ROWS if y not in rows],
        'band_rows_at_target': [round(rows[0] * tgt.size[1] / SKY_H), round((rows[-1] + 1) * tgt.size[1] / SKY_H) - 1],
        'ripple': float(dev.max()), 'ripple_at_row': rows[iy], 'ripple_channel': 'Lab'[ic],
        'max_abs_err': float(np.linalg.norm(err, axis=1).max()),
        'mean_err_Lab': [float(v) for v in err.mean(0)],
        'sky_dE_mask_weighted': float((np.linalg.norm(la - lt, axis=-1) * msk).sum() / msk.sum()),
        'row_err_L_every_12': {str(y): float(err[rows.index(y), 0]) for y in range(rows[0], rows[-1] + 1, 12) if y in rows}}

    # 3. green horizon (wb_regions horizon band)
    hs = mask_at(mask_src, (HZ_W, HZ_H)) > 0.5
    top_land = np.where((~hs).any(0), np.argmax(~hs, 0), HZ_H)
    rr = np.arange(HZ_H)[:, None]
    band = hs & (rr >= top_land[None, :] - HZ_FRAC * HZ_H) & (rr < top_land[None, :])
    at = lab(tgt, (HZ_W, HZ_H), Image.LANCZOS)[band].mean(0)
    ac = lab(cand, (HZ_W, HZ_H), Image.LANCZOS)[band].mean(0)
    rep['green'] = {'frame': 'wb_regions: Lanczos to %dx%d; sky pixels < %.2f x height above their column\'s land' % (HZ_W, HZ_H, HZ_FRAC),
                    'band_share': float(band.mean()), 'target_a': float(at[1]), 'candidate_a': float(ac[1]),
                    'delta_a': float(ac[1] - at[1]), 'target_Lab': [float(v) for v in at], 'candidate_Lab': [float(v) for v in ac],
                    'green': bool(ac[1] < at[1] - GREEN_MARGIN)}

    # 4. star contrast (information only)
    T = tgt.size
    lt_s = lstar(tgt, T, Image.LANCZOS)
    lc_s = lstar(cand, T, Image.LANCZOS)
    sky_s = mask_at(mask_src, T) >= 0.95
    bg_t, pk_t = median_filter(lt_s, size=21), maximum_filter(lt_s, size=5)
    cand_pts = (lt_s == pk_t) & sky_s
    ys, xs = np.nonzero(cand_pts)
    order = np.argsort(-lt_s[ys, xs])[:N_STARS]
    ys, xs = ys[order], xs[order]
    c_t = (pk_t - bg_t)[ys, xs]
    c_c = (maximum_filter(lc_s, size=5) - median_filter(lc_s, size=21))[ys, xs]
    rep['stars_info_only'] = {'n': int(len(ys)), 'target_contrast_median_L': float(np.median(c_t)),
                              'candidate_contrast_median_L': float(np.median(c_c)),
                              'ratio_median': float(np.median(c_c / np.maximum(c_t, 1e-3)))}

    rep['lines'] = {'dE_whole': rep['dE']['whole'], 'ripple': rep['ripple']['ripple'], 'green': rep['green']['green']}
    rep['thresholds'] = {'dE_whole_max': DE_LINE, 'ripple_max': RIPPLE_LINE, 'green_margin_a': GREEN_MARGIN}
    return rep


def show(rep):
    d, r, g, s = rep['dE'], rep['ripple'], rep['green'], rep['stars_info_only']
    print('candidate %s (%dx%d)  vs  target %s' % (rep['candidate'], *rep['candidate_size'], rep['target']))
    print('sky mask: %s' % rep['sky_mask'])
    print('1. dE whole %.2f  (line <= %.2f: %s)   sky %.2f  ground %.2f  [sky share %.2f; %s]   at target size %.2f' % (
        d['whole'], DE_LINE, 'PASS' if d['whole'] <= DE_LINE else 'FAIL', d['sky'], d['ground'], d['sky_share'], d['frame'], d['whole_at_target_size']))
    print('2. ripple %.2f  (line <= %.2f: %s)   at row %d, %s*   rows %d..%d of %d (%d used; %d..%d at target)   max|err| %.2f   sky dE %.2f' % (
        r['ripple'], RIPPLE_LINE, 'PASS' if r['ripple'] <= RIPPLE_LINE else 'FAIL', r['ripple_at_row'], r['ripple_channel'],
        *r['band_rows'], SKY_H, r['rows_used'], *r['band_rows_at_target'], r['max_abs_err'], r['sky_dE_mask_weighted']))
    print('   row error L at rows: ' + ' '.join('%s:%+.1f' % kv for kv in r['row_err_L_every_12'].items()))
    print('3. horizon a* candidate %+.2f  target %+.2f  diff %+.2f  -> %s' % (
        g['candidate_a'], g['target_a'], g['delta_a'], 'GREEN (FAIL)' if g['green'] else 'not green (PASS)'))
    print('4. (info) stars: %d brightest maxima in target sky; contrast median target %.1f L, candidate %.1f L, ratio %.2f' % (
        s['n'], s['target_contrast_median_L'], s['candidate_contrast_median_L'], s['ratio_median']))


def main():
    ap = argparse.ArgumentParser(description='p42 acceptance measure')
    ap.add_argument('candidate')
    ap.add_argument('--target', default=DEFAULT_TARGET)
    ap.add_argument('--sky-mask', default=None,
                    help="sky mask bitmap (white = sky); default: the corpus's own when present, "
                         "'none' to derive it from the target")
    ap.add_argument('--out', default=None)
    ap.add_argument('--baseline', action='store_true', help='tag the report as the reference render\'s baseline')
    a = ap.parse_args()
    mask = a.sky_mask
    if mask is None and os.path.isfile(DEFAULT_SKY_MASK):
        mask = DEFAULT_SKY_MASK
    if mask == 'none':
        mask = None
    rep = measure(a.candidate, a.target, mask)
    rep['role'] = 'baseline' if a.baseline else 'candidate'
    show(rep)
    out = a.out or ('p42_baseline.json' if a.baseline else 'p42_report.json')
    with open(out, 'w', encoding='utf-8') as f:
        json.dump(rep, f, indent=1, ensure_ascii=False)
    print('report ->', os.path.abspath(out))


if __name__ == '__main__':
    main()
