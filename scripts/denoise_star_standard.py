#!/usr/bin/env python3
"""F1 release standard: precomputed TIFFs, no GPU and no inferred home directory.

Required AutoShade develop parity: Sharpness 40, SharpenRadius 1.0,
SharpenDetail 25, SharpenEdgeMasking 0, ColorNoiseReduction 0,
LuminanceSmoothing 0, linear USER point curve, as-shot WB, Adobe Standard and no
lens corrections. KEEP the camera-matched base_curve built by the RAW open
path: a Linear point curve does not disable either camera profile's base look.
Grain return must precede Detail. Lightroom OFF has colour
NR 25 while ON has 0; that remaining confound is reported, not corrected.

--root or AUTOSHADE_FIXTURES_ROOT resolves relative input paths. Four explicit
renders are required; --candidate NAME=PATH adds comparisons with the SAME
input-defined mask/sites. --work controls scratch/cache; --json and --mask-sheet
are explicit outputs. Nothing is written beside an input by default.

The Part 3 detector is a 3x3 local maximum >=5 sigma above the 14--20 px
annular median. With renders rather than mosaics, sigma is the annulus's
pixel MAD/.6745: Haar sigma is not pixel sigma after a correlated demosaic.
No output chooses the mask: input and LR OFF sites are
unioned and each excluded disk has radius max(4, ceil(3*measured FWHM)).
Tiles retain >=70% of their pixels, then the lowest 30% by the Part 3
(box3-box9) variance / fine-Haar MAD-sigma squared qualify. All filters use
only unmasked samples; all four Haar samples must survive. Mottle is box5
minus box33 after removing the tile's fitted plane. Within-image colourfulness
uses Haar/band RMS, not MAD. All noise and colour axes are common LINEAR sRGB
(Adobe RGB is decoded and transformed first). Components remain printed,
but colourfulness gates (3/4) use sqrt(Cb_RMS^2+Cr_RMS^2)/Y_RMS per tile.
Star colour changes (7) and absolute colour (12) also use common linear sRGB;
differences in two different RGB primaries are
not comparable units, even when each is measured against its own input.

Bright profiles use the input-only Part 3 radial affine-response screen:
max error <=5% of LR-OFF peak excess, positive slope, unclipped 5x5 cores
in every compared render and output peaks inside their own input's sampled
radial response range (the Part 3 no-extrapolation condition).
Absolute widths use the minor eigenvector of the positive, background-subtracted
second-moment ellipse within radius 6, and bilinear half-height crossings at
0.1 px spacing. Absolute counts use >=5 local sigma (no upper cut), with
one-to-one nearest matches within 3 px. Detection counts (11) use luminance
after ONE monotone map fitted on the 64-pixel-blurred, star-masked INPUT and
LR OFF; the same map serves every AutoShade candidate. Contrast (10) is
peak excess / local background fine-Haar sigma in those matched units.
The map never changes the input-relative metrics or the review plate.
Faint colour outliers use input SNR
[5,10). Lines 9--12 are reported, never used to select/tune the measurement.

Exit: 0 NEW and mask-validation gates pass, 1 a gate fails, 2 invalid inputs.
Historical --candidate rows are reported but do not decide the exit status.
Line 8's visual banding clause needs --banding clear|visible; otherwise it
cannot pass the release gate. Cached arrays are keyed by source size/mtime,
paths and this script's SHA-256; a changed protocol cannot reuse stale arrays.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import tempfile

import cv2
import numpy as np
from scipy.spatial import cKDTree
import tifffile

cv2.setNumThreads(4)
SKY = (128, 128, 9216, 4096)
GLOW = (2432, 4672, 768, 768)
DY, DX = np.mgrid[-20:21, -20:21]
R = np.hypot(DY, DX)
BG = (R >= 14) & (R <= 20)
BY, BX = np.nonzero(BG)
BY, BX = BY - 20, BX - 20
HY, HX = np.mgrid[-19.5:20:2, -19.5:20:2]
HBG = (np.hypot(HY, HX) >= 14) & (np.hypot(HY, HX) <= 19)
HBY, HBX = np.nonzero(HBG)
HBY, HBX = HBY * 2 - 20, HBX * 2 - 20
RADIAL = [R < .75] + [(R >= k-.5) & (R < k+.5) for k in range(1, 13)]


def rgb_matrix(primaries):
    xyz = np.array([[x/y, 1, (1-x-y)/y] for x, y in primaries]).T
    white = np.array([.3127/.329, 1, (1-.3127-.329)/.329])
    return xyz * np.linalg.solve(xyz, white)[None, :]


SRGB = rgb_matrix(((.64, .33), (.30, .60), (.15, .06)))
ADOBE = rgb_matrix(((.64, .33), (.21, .71), (.15, .06)))
ADOBE_TO_SRGB = np.linalg.solve(SRGB, ADOBE)


def decode(a, adobe=False):
    v = a.astype(np.float32) / 65535
    return v ** (563/256) if adobe else np.where(v <= .04045, v/12.92, ((v+.055)/1.055)**2.4)


def encode(a):
    v = np.clip(a, 0, 1)
    return np.where(v <= .0031308, v*12.92, 1.055*v**(1/2.4)-.055)


def haar(a):
    return (a[::2, ::2]-a[1::2, ::2]-a[::2, 1::2]+a[1::2, 1::2])/2


def mad(a, axis=None):
    return np.median(np.abs(a-np.median(a, axis=axis, keepdims=True)), axis=axis)/.6745


def rms(a):
    return float(np.sqrt(np.mean(np.asarray(a, np.float64)**2)))


def components(rgb, w):
    y = rgb @ w
    return [y, (rgb[..., 2]-y)/(2*(1-w[2])), (rgb[..., 0]-y)/(2*(1-w[0]))]


def widths(p, bg):
    level = (p[:, 20, 20]-bg)*.5
    total = np.zeros(len(p)); valid = level > 0
    for dy, dx in ((0, -1), (0, 1), (-1, 0), (1, 0)):
        v = p[:, 20+dy*np.arange(13), 20+dx*np.arange(13)]-bg[:, None]
        below = v[:, 1:] <= level[:, None]
        valid &= np.any(below, axis=1)
        k = np.argmax(below, axis=1)+1; idx = np.arange(len(p))
        hi, lo = v[idx, k-1], v[idx, k]
        total += k-1+(hi-level)/np.maximum(hi-lo, 1e-20)
    return np.where(valid, total/2, np.nan)


def fields(yplane, sites):
    """Fixed-site Part 3 measurements; coordinates are in this image."""
    x, y = sites[:, 0].astype(int), sites[:, 1].astype(int)
    result = {k: [] for k in ('bg', 'excess', 'sigma', 'fine_sigma', 'fwhm', 'radial')}
    for start in range(0, len(x), 512):
        p = np.asarray(yplane[y[start:start+512, None, None]+DY,
                             x[start:start+512, None, None]+DX], np.float32)
        bg = np.median(p[:, BG], axis=1)
        h = (p[:, :40:2, :40:2]-p[:, 1:40:2, :40:2]-p[:, :40:2, 1:40:2]+p[:, 1:40:2, 1:40:2])/2
        result['bg'].append(bg); result['excess'].append(p[:, 20, 20]-bg)
        result['sigma'].append(mad(p[:, BG], axis=1))
        result['fine_sigma'].append(mad(h[:, HBG], axis=1))
        result['fwhm'].append(widths(p, bg))
        result['radial'].append(np.column_stack([*[p[:, m].mean(axis=1) for m in RADIAL], bg]))
    return {k: np.concatenate(v) for k, v in result.items()}


def detect(yplane, roi, cache):
    if cache.exists():
        return np.load(cache)
    x0, y0, w, h = roi
    patch = np.asarray(yplane[y0-1:y0+h+1, x0-1:x0+w+1])
    maxima = patch[1:-1, 1:-1] == cv2.dilate(patch, np.ones((3, 3), np.uint8))[1:-1, 1:-1]
    ys, xs = np.nonzero(maxima); xs += x0; ys += y0
    selected = []
    # No mean-prefilter: a faint star next to a bright one still gets the
    # annular median test, as in Part 3.
    for start in range(0, len(xs), 4096):
        x, y = xs[start:start+4096], ys[start:start+4096]
        bg = np.median(yplane[y[:, None]+BY, x[:, None]+BX], axis=1)
        sigma = np.maximum(mad(yplane[y[:, None]+BY, x[:, None]+BX], axis=1), 1e-10)
        excess = yplane[y, x]-bg; snr = excess/sigma
        keep = snr >= 5
        selected.append(np.column_stack((x[keep], y[keep], snr[keep])))
        if start % 409600 == 0:
            print('detect', cache.stem, start, '/', len(xs), flush=True)
    sites = np.concatenate(selected)
    f = fields(yplane, sites)
    sites = np.column_stack((sites, f['fwhm']))
    np.save(cache, sites)
    return sites


def masked_box(a, valid, size):
    weight = cv2.blur(valid.astype(np.float32), (size, size))
    value = cv2.blur(np.where(valid, a, 0), (size, size))/np.maximum(weight, 1e-8)
    return value, weight


def tile_stats(rgb, mask, w):
    valid = ~mask; inner = valid[16:-16, 16:-16]
    good = inner[::2, ::2] & inner[1::2, ::2] & inner[::2, 1::2] & inner[1::2, 1::2]
    fy, fx = np.mgrid[:len(mask), :mask.shape[1]].astype(np.float32)
    design = np.column_stack((np.ones(valid.sum()), fx[valid], fy[valid]))
    result = []; hf = []; bands = []
    for c in components(rgb, w):
        diag = haar(c[16:-16, 16:-16])[good]
        result.append(float(mad(diag))); hf.append(rms(diag))
        plane = np.linalg.lstsq(design, c[valid], rcond=None)[0]
        detrend = c-(plane[0]+plane[1]*fx+plane[2]*fy)
        b5, n5 = masked_box(detrend, valid, 5)
        b33, n33 = masked_box(detrend, valid, 33)
        use = inner & (n5[16:-16, 16:-16] >= .5) & (n33[16:-16, 16:-16] >= .5)
        bands.append(rms((b5-b33)[16:-16, 16:-16][use]))
    return np.array(result+bands+hf)


def choose_tiles(y, mask, roi):
    x0, y0, w, h = roi; eligible = []; all_rows = []
    for yy in range(y0, y0+h-255, 256):
        for xx in range(x0, x0+w-255, 256):
            valid = ~mask[yy-16:yy+272, xx-16:xx+272]
            inside = valid[16:-16, 16:-16]; surviving = inside.mean()
            score = None
            if surviving >= .7:
                p = np.asarray(y[yy-16:yy+272, xx-16:xx+272])
                b3, _ = masked_box(p, valid, 3); b9, _ = masked_box(p, valid, 9)
                good = inside[::2, ::2] & inside[1::2, ::2] & inside[::2, 1::2] & inside[1::2, 1::2]
                sigma = float(mad(haar(p[16:-16, 16:-16])[good]))
                score = float(np.var((b3-b9)[16:-16, 16:-16][inside])/max(sigma*sigma, 1e-20))
                eligible.append((score, xx, yy))
            all_rows.append([xx, yy, float(surviving), score])
    if not eligible:
        raise ValueError('No tile has 70% unmasked sky; the standard cannot be measured')
    eligible.sort()
    keep = eligible[:math.ceil(.3*len(eligible))]
    return np.array([[x, yy] for _, x, yy in keep]), all_rows


def match_sites(a, b, radius=3):
    """One-to-one shortest-distance pairing; never count a counterpart twice."""
    tree = cKDTree(b[:, :2]); pairs = []
    for i, js in enumerate(tree.query_ball_point(a[:, :2], radius)):
        for j in js:
            pairs.append((float(np.linalg.norm(a[i, :2]-b[j, :2])), i, j))
    used_a = set(); used_b = set(); matches = []
    for _, i, j in sorted(pairs):
        if i not in used_a and j not in used_b:
            used_a.add(i); used_b.add(j); matches.append((i, j))
    return np.array(matches, dtype=int).reshape(-1, 2)


def minor_width(p, bg):
    """Second-moment direction, then actual half-height width along its minor axis."""
    signal = np.maximum(p-bg, 0); support = (R <= 6) & (signal >= .05*signal[20, 20])
    weight = signal*support; total = weight.sum()
    if total <= 0:
        return float('nan')
    cx, cy = (weight*DX).sum()/total, (weight*DY).sum()/total
    cov = np.array([[(weight*(DX-cx)**2).sum(), (weight*(DX-cx)*(DY-cy)).sum()],
                    [(weight*(DX-cx)*(DY-cy)).sum(), (weight*(DY-cy)**2).sum()]])/total
    _, axes = np.linalg.eigh(cov); vx, vy = axes[:, 0]
    t = np.arange(-8, 8.001, .1, dtype=np.float32)
    values = cv2.remap(p, (20+cx+t*vx).reshape(1, -1).astype(np.float32),
                       (20+cy+t*vy).reshape(1, -1).astype(np.float32), cv2.INTER_LINEAR)[0]-bg
    centre = int(np.argmax(values)); level = values[centre]*.5
    left = np.flatnonzero(values[:centre] <= level)
    right = np.flatnonzero(values[centre+1:] <= level)+centre+1
    if not len(left) or not len(right) or level <= 0:
        return float('nan')
    lo, hi = left[-1], right[0]
    l = t[lo]+.1*(level-values[lo])/max(values[lo+1]-values[lo], 1e-20)
    r = t[hi-1]+.1*(values[hi-1]-level)/max(values[hi-1]-values[hi], 1e-20)
    return float(r-l)


class Image:
    def __init__(self, path, adobe, offset, work):
        self.path = path; self.adobe = adobe; self.offset = np.array(offset)
        self.rgb = tifffile.memmap(path, mode='r')
        if self.rgb.dtype != np.uint16 or self.rgb.ndim != 3 or self.rgb.shape[2] != 3:
            raise ValueError(f'{path}: expected uint16 RGB TIFF')
        self.w = SRGB[1].astype(np.float32)
        stat = path.stat()
        token = str(path.resolve())+str(stat.st_size)+str(stat.st_mtime_ns)
        self.key = hashlib.sha256(token.encode()).hexdigest()[:16]
        yp = work/(self.key+'-y.npy')
        if not yp.exists():
            dest = np.lib.format.open_memmap(yp, mode='w+', dtype=np.float32, shape=self.rgb.shape[:2])
            for top in range(0, len(self.rgb), 128):
                rgb = decode(self.rgb[top:top+128], adobe)
                if adobe:
                    rgb = rgb @ ADOBE_TO_SRGB.T
                dest[top:top+128] = rgb @ self.w
            dest.flush(); del dest
        self.y = np.load(yp, mmap_mode='r')

    def patch(self, x, y, size, halo=0):
        x, y = int(x-self.offset[0]), int(y-self.offset[1])
        rgb = decode(self.rgb[y-halo:y+size+halo, x-halo:x+size+halo], self.adobe)
        return rgb @ ADOBE_TO_SRGB.T if self.adobe else rgb

    def core(self, sites, common_space=False):
        xy = sites[:, :2].astype(int)-self.offset
        rgb = decode(self.rgb[xy[:, 1], xy[:, 0]], self.adobe)
        if common_space and self.adobe:
            rgb = rgb @ ADOBE_TO_SRGB.T
        w = SRGB[1] if common_space or not self.adobe else ADOBE[1]
        y, cb, cr = components(rgb, w)
        return np.column_stack((rgb, y, cb, cr))


def monotone_knots(x, y, weight):
    """Weighted least-squares isotonic fit at strictly increasing x values."""
    blocks = []
    for i, (xx, yy, ww) in enumerate(zip(x, y, weight)):
        blocks.append([i, i+1, float(ww), float(yy*ww)])
        while len(blocks) > 1 and blocks[-2][3]/blocks[-2][2] > blocks[-1][3]/blocks[-1][2]:
            a, b = blocks[-2:]; blocks[-2:] = [[a[0], b[1], a[2]+b[2], a[3]+b[3]]]
    fitted = np.empty(len(x), np.float64)
    for lo, hi, w, total in blocks:
        fitted[lo:hi] = total/w
    return fitted


def apply_tone(values, fit, derivative=False):
    """Piecewise-linear monotone map with linear tails, without a gamut clip."""
    x, y = np.asarray(fit['x']), np.asarray(fit['y'])
    slopes = np.diff(y)/np.diff(x)
    indices = np.clip(np.searchsorted(x, values, side='right')-1, 0, len(slopes)-1)
    return slopes[indices] if derivative else y[indices]+slopes[indices]*(values-x[indices])


def fit_tone_map(ours, reference, mask, offset):
    """Fit on paired 64-px masked means, sampled every 16 px over the overlap.

    Equal-population bins keep the sky from being discarded by bright pixels.
    The map is fixed by the two INPUTS; no candidate selects its own exposure.
    """
    dx, dy = offset; h = min(len(ours)-dy, len(reference)); w = min(ours.shape[1]-dx, reference.shape[1])
    samples = []
    for top in range(32, h-32, 256):
        end = min(top+256, h-32)
        valid = ~mask[top+dy-32:end+dy+32, dx:dx+w]
        a, n = masked_box(np.asarray(ours[top+dy-32:end+dy+32, dx:dx+w]), valid, 64)
        b, _ = masked_box(np.asarray(reference[top-32:end+32, :w]), valid, 64)
        aa, bb, nn = a[32:-32:16, 32:-32:16], b[32:-32:16, 32:-32:16], n[32:-32:16, 32:-32:16]
        keep = (nn >= .5) & np.isfinite(aa) & np.isfinite(bb)
        samples.append(np.column_stack((aa[keep], bb[keep])))
    sample = np.concatenate(samples); sample = sample[np.argsort(sample[:, 0])]
    if len(sample) < 128:
        raise ValueError('too few unmasked paired means for a tone map')
    bins = [a for a in np.array_split(sample, 128) if len(a)]
    x = np.array([np.median(a[:, 0]) for a in bins]); y = np.array([np.median(a[:, 1]) for a in bins])
    weight = np.array([len(a) for a in bins]); keep = np.r_[True, np.diff(x) > 1e-12]
    x, y, weight = x[keep], y[keep], weight[keep]
    if len(x) < 2:
        raise ValueError('constant input cannot define a tone-map slope')
    y = monotone_knots(x, y, weight)
    fit = {'x': x.tolist(), 'y': y.tolist(), 'samples': len(sample), 'blur_px': 64,
           'sample_step_px': 16, 'bins': len(x), 'reference': 'input -> Lightroom OFF'}
    errors = apply_tone(sample[:, 0], fit)-sample[:, 1]
    fit['residual_rms'] = rms(errors); fit['residual_median_abs'] = float(np.median(np.abs(errors)))
    fit['reference_p10_p90'] = np.percentile(sample[:, 1], [10, 90]).tolist()
    fit['residual_p10_p90'] = np.percentile(errors, [10, 90]).tolist()
    return fit


def acceptance(noise, star, absolute, lrnoise, lrstar, banding):
    width = lambda row: row['fine_y_p10_p90'][1]-row['fine_y_p10_p90'][0]
    values = [abs(noise['fine_y']-lrnoise['fine_y']) <= .03,
              width(noise) <= width(lrnoise)+.02,
              noise['fine_colourfulness_magnitude'] <= lrnoise['fine_colourfulness_magnitude'],
              noise['mottle_colourfulness_magnitude'] <= lrnoise['mottle_colourfulness_magnitude'],
              star['faint_retention_percent'] >= lrstar['faint_retention_percent']-.5,
              star['peak_ratio'] >= lrstar['peak_ratio']-.01 and star['fwhm_change'] <= lrstar['fwhm_change'],
              all(a <= b+.01 for a, b in zip(star['core_colour_change'], lrstar['core_colour_change'])),
              noise['glow_correlation'] >= .999 and banding == 'clear']
    return {str(i+1): 'PASS' if value else 'FAIL' for i, value in enumerate(values)}


def run(args, work):
    root = Path(args.root).resolve()
    resolve = lambda p: (root/Path(p)).resolve()
    paths = {'input': resolve(args.input), 'NEW': resolve(args.ours),
             'LR-OFF': resolve(args.lr_off), 'Lightroom': resolve(args.lr_on)}
    for item in args.candidate:
        label, path = item.split('=', 1)
        if label in paths:
            raise ValueError('duplicate/reserved candidate name '+label)
        paths[label] = resolve(path)
    images = {k: Image(path, k in ('LR-OFF', 'Lightroom'),
                       (32, 20) if k in ('LR-OFF', 'Lightroom') else (0, 0), work)
              for k, path in paths.items()}
    roi = SKY
    # Verify the prescribed integer registration independently of noise tiles.
    registration = {}
    for k, im in images.items():
        checks = []
        for x, y in ((2304, 1024), (4800, 1536), (6912, 2048)):
            ref = np.asarray(images['input'].y[y:y+256, x:x+256])
            x1, y1 = np.array((x, y))-im.offset
            target = np.asarray(im.y[y1-3:y1+259, x1-3:x1+259])
            bp = lambda z: cv2.blur(z, (3, 3))-cv2.blur(z, (9, 9))
            _, corr, _, (dx, dy) = cv2.minMaxLoc(cv2.matchTemplate(bp(target), bp(ref), cv2.TM_CCOEFF_NORMED))
            checks.append({'delta': [dx-3, dy-3], 'correlation': float(corr)})
        registration[k] = checks
        if any(c['delta'] != [0, 0] for c in checks):
            raise ValueError(f'{k}: prescribed registration failed: {checks}')
    detections = {}
    for k, im in images.items():
        if k == 'LR-OFF' or k == 'input' or k == 'Lightroom' or k in paths:
            x, y, w, h = roi; dx, dy = im.offset
            a = detect(im.y, (x-dx, y-dy, w, h), work/(im.key+'-stars.npy'))
            a = a.copy(); a[:, :2] += im.offset
            detections[k] = a
            print(k, 'detected', len(a), flush=True)
    mask = np.zeros(images['input'].rgb.shape[:2], np.uint8)
    for k in ('input', 'LR-OFF'):
        for x, y, _, fw in detections[k]:
            radius = max(4, math.ceil(3*fw)) if np.isfinite(fw) else 12
            cv2.circle(mask, (int(x), int(y)), radius, 1, -1)
    mask = mask.astype(bool)
    if args.fixed_sites:
        fixed = resolve(args.fixed_sites)
        mask = np.load(fixed/'star-mask.npy').astype(bool)
        if mask.shape != images['input'].y.shape:
            raise ValueError('fixed mask dimensions do not match the input')
        sites = np.load(fixed/'flat-sites.npy')
        eligibility = [[int(x), int(y), float(np.mean(~mask[y:y+256, x:x+256])), None] for x, y in sites]
    else:
        sites, eligibility = choose_tiles(images['input'].y, mask, roi)
    np.save(work/'star-mask.npy', mask)
    np.save(work/'flat-sites.npy', sites)
    print('eligible', sum(r[2] >= .7 for r in eligibility), 'selected', len(sites), flush=True)
    raw_noise = {}
    for k, im in images.items():
        rows = [tile_stats(im.patch(x, y, 256, 16), mask[y-16:y+272, x-16:x+272], im.w) for x, y in sites]
        raw_noise[k] = np.array(rows)
    noise = {}
    for k, data in raw_noise.items():
        base = raw_noise['LR-OFF' if k in ('Lightroom', 'LR-OFF') else 'input']
        ratio = data[:, :6]/base[:, :6]
        n = {name: float(np.median(ratio[:, j])) for j, name in enumerate(('fine_y', 'fine_cb', 'fine_cr', 'mottle_y', 'mottle_cb', 'mottle_cr'))}
        n['fine_y_p10_p90'] = np.percentile(ratio[:, 0], [10, 90]).tolist()
        n['fine_colourfulness'] = np.median(data[:, 7:9]/data[:, 6, None], axis=0).tolist()
        n['mottle_colourfulness'] = np.median(data[:, 4:6]/data[:, 3, None], axis=0).tolist()
        n['fine_colourfulness_magnitude'] = float(np.median(np.linalg.norm(data[:, 7:9], axis=1)/data[:, 6]))
        n['mottle_colourfulness_magnitude'] = float(np.median(np.linalg.norm(data[:, 4:6], axis=1)/data[:, 3]))
        im = images[k]; base_im = images['LR-OFF' if k in ('Lightroom', 'LR-OFF') else 'input']
        x, y, w, h = GLOW
        first = im.patch(x, y, w, 32) @ im.w
        before = base_im.patch(x, y, w, 32) @ base_im.w
        a = cv2.blur(first, (64, 64))[32:-32, 32:-32]
        b = cv2.blur(before, (64, 64))[32:-32, 32:-32]
        n['glow_correlation'] = float(np.corrcoef(a.ravel(), b.ravel())[0, 1])
        noise[k] = n
    np.savez(work/'noise-tiles.npz', **raw_noise)
    # Fixed sites defined by the original, plus input-only linearity screen.
    starsites = detections['input']; field = {}
    for k, im in images.items():
        field[k] = fields(im.y, starsites[:, :2]-im.offset)
    a, b = field['input']['radial'], field['LR-OFF']['radial']
    u, v = a-a.mean(axis=1, keepdims=True), b-b.mean(axis=1, keepdims=True)
    slope = np.sum(u*v, axis=1)/np.maximum(np.sum(u*u, axis=1), 1e-20)
    error = np.max(np.abs(v-u*slope[:, None]), axis=1)/np.maximum(field['LR-OFF']['excess'], 1e-20)
    linear = (slope > 0) & (error <= .05) & (field['input']['excess'] > 0) & (field['LR-OFF']['excess'] > 0)
    input_linear_count = int(np.sum(linear & (starsites[:, 2] >= 10)))
    for k, im in images.items():
        xy = starsites[:, :2].astype(int)-im.offset
        for dy in range(-2, 3):
            for dx in range(-2, 3):
                linear &= np.max(im.rgb[xy[:, 1]+dy, xy[:, 0]+dx], axis=1) < 65000
        if k not in ('input', 'LR-OFF'):
            baseline = 'LR-OFF' if k == 'Lightroom' else 'input'
            limit = np.max(field[baseline]['radial'], axis=1)-field[baseline]['bg']
            linear &= (field[k]['excess'] >= 0) & (field[k]['excess'] <= limit)
    bright = linear & (starsites[:, 2] >= 10)
    valid = (field['input']['excess'] > 0) & (field['LR-OFF']['excess'] > 0)
    faint = valid & (starsites[:, 2] < 10)
    np.savez(work/'fixed-sites.npz', sites=starsites, linear=linear, bright=bright, faint=faint)
    star = {}
    for k, f in field.items():
        baseline = 'LR-OFF' if k in ('Lightroom', 'LR-OFF') else 'input'; base = field[baseline]
        width_ok = bright & np.isfinite(f['fwhm']) & np.isfinite(base['fwhm'])
        im = images[k]; core = im.core(starsites, True)
        old = images[baseline].core(starsites, True)
        chroma = np.column_stack((core[:, 0]-core[:, 1], core[:, 2]-core[:, 1]))/np.maximum(core[:, 3, None], 1e-10)
        old_chroma = np.column_stack((old[:, 0]-old[:, 1], old[:, 2]-old[:, 1]))/np.maximum(old[:, 3, None], 1e-10)
        # All rows use the same qualified subset. Report clipping as a backstop.
        xy = starsites[:, :2].astype(int)-im.offset
        unclipped = np.max(im.rgb[xy[:, 1], xy[:, 0]], axis=1) < 65000
        kept = int(np.sum(f['excess'][faint] >= .5*base['excess'][faint]))
        star[k] = {'faint_count': int(faint.sum()), 'faint_kept': kept,
                   'faint_retention_percent': 100*kept/max(int(faint.sum()), 1),
                   'bright_qualified': int(bright.sum()), 'width_count': int(width_ok.sum()),
                   'output_core_clipped': int(np.sum(bright & ~unclipped)),
                   'peak_ratio': float(np.mean(f['excess'][bright]/base['excess'][bright])),
                   'fwhm_change': float(np.mean(f['fwhm'][width_ok]-base['fwhm'][width_ok])),
                   'core_colour_change': np.median(np.abs(chroma[bright]-old_chroma[bright]), axis=0).tolist()}
    # One input-defined tone map: never refit to a denoised candidate.
    tone = fit_tone_map(images['input'].y, images['LR-OFF'].y, mask, (32, 20))
    (work/'tone-map.json').write_text(json.dumps(tone, indent=2), encoding='utf-8')
    matched_y = {}
    absolute_detections = {}
    for k, im in images.items():
        if k in ('LR-OFF', 'Lightroom'):
            matched_y[k] = im.y
        else:
            path = work/(im.key+'-tone-y.npy')
            out = np.lib.format.open_memmap(path, mode='w+', dtype=np.float32, shape=im.y.shape)
            for top in range(0, len(out), 128):
                out[top:top+128] = apply_tone(im.y[top:top+128], tone)
            out.flush(); matched_y[k] = out
        x, y, w, h = roi; dx, dy = im.offset
        det = detect(matched_y[k], (x-dx, y-dy, w, h), work/(im.key+'-tone-stars.npy')).copy()
        det[:, :2] += im.offset; absolute_detections[k] = det
    absolute = {}
    for k in paths:
        if k in ('input', 'LR-OFF'):
            continue
        im, lr = images[k], images['Lightroom']
        aa, bb = absolute_detections[k], absolute_detections['Lightroom']
        matched = match_sites(aa, bb); sa, sb = aa[matched[:, 0]], bb[matched[:, 1]]
        fa, fb = fields(im.y, sa[:, :2]-im.offset), fields(lr.y, sb[:, :2]-lr.offset)
        widths_a = []; widths_b = []
        for obj, ss, ff, dest in ((im, sa, fa, widths_a), (lr, sb, fb, widths_b)):
            for j, (x, y, *_rest) in enumerate(ss):
                xx, yy = (np.array((x, y))-obj.offset).astype(int)
                dest.append(minor_width(np.asarray(obj.y[yy-20:yy+21, xx-20:xx+21]), ff['bg'][j]))
        wa, wb = np.array(widths_a), np.array(widths_b)
        usable = np.isfinite(wa) & np.isfinite(wb) & (wa > 0) & (wb > 0)
        ca, cb = im.core(sa, True), lr.core(sb, True)
        ch_a = ca[:, 4:6]/np.maximum(ca[:, 3, None], 1e-10)
        ch_b = cb[:, 4:6]/np.maximum(cb[:, 3, None], 1e-10)
        # Class faint by ORIGINAL local SNR at the LR-matched coordinates,
        # so a denoiser cannot change which stars enter its colour test.
        fi = fields(images['input'].y, sb[:, :2])
        input_snr = fi['excess']/np.maximum(fi['sigma'], 1e-10)
        fm = (input_snr >= 5) & (input_snr < 10)
        colour_excess = np.linalg.norm(ch_a, axis=1)-np.linalg.norm(ch_b, axis=1)
        ta = fields(matched_y[k], sa[:, :2]-im.offset)
        tb = fields(matched_y['Lightroom'], sb[:, :2]-lr.offset)
        contrast_ok = (ta['fine_sigma'] > 0) & (tb['fine_sigma'] > 0) & (tb['excess'] > 0)
        snr_a = ta['excess']/np.maximum(ta['fine_sigma'], 1e-20)
        snr_b = tb['excess']/np.maximum(tb['fine_sigma'], 1e-20)
        absolute[k] = {'matched': len(matched), 'ours_count': len(aa), 'lr_count': len(bb),
                       'count_ratio': len(aa)/max(len(bb), 1),
                       'lr_unmatched': len(bb)-len(matched), 'ours_unmatched': len(aa)-len(matched),
                       'minor_width_count': int(usable.sum()),
                       'minor_ours': np.percentile(wa[usable], [50, 10, 90]).tolist(),
                       'minor_lr': np.percentile(wb[usable], [50, 10, 90]).tolist(),
                       'minor_width_ratio': float(np.median(wa[usable]/wb[usable])),
                       'minor_width_ratio_of_medians': float(np.median(wa[usable])/np.median(wb[usable])),
                       'contrast_count': int(contrast_ok.sum()),
                       'contrast_ratio': float(np.median(snr_a[contrast_ok]/snr_b[contrast_ok])),
                       'contrast_ours': float(np.median(snr_a[contrast_ok])),
                       'contrast_lr': float(np.median(snr_b[contrast_ok])),
                       'core_chroma_median_abs_delta': np.median(np.abs(ch_a-ch_b), axis=0).tolist(),
                       'core_chroma_median_delta_magnitude': float(np.median(np.linalg.norm(ch_a-ch_b, axis=1))),
                       'faint_colour_count': int(fm.sum()),
                       'faint_colour_excess_share': float(np.mean(colour_excess[fm] > .05))}
    gates = {k: acceptance(noise[k], star[k], absolute[k], noise['Lightroom'], star['Lightroom'], args.banding)
             for k in absolute}
    result = {'protocol_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'inputs': {k: str(p) for k, p in paths.items()}, 'root': str(root),
              'registration': registration, 'sky': roi,
              'mask': {'input_sites': len(detections['input']), 'lr_off_sites': len(detections['LR-OFF']),
                       'surviving_sky_fraction': float(np.mean(~mask[128:4224, 128:9344])),
                       'eligible_tiles': sum(r[2] >= .7 for r in eligibility), 'selected_tiles': len(sites),
                       'tile_eligibility': eligibility, 'lr_fine_y_width': float(np.diff(noise['Lightroom']['fine_y_p10_p90'])[0])},
              'noise': noise, 'stars': star, 'absolute': absolute, 'gates': gates,
              'profile_qualification': {'input_affine_bright': input_linear_count,
                                        'common_unclipped_in_range_bright': int(bright.sum())},
              'tone_map': tone, 'fixed_sites': args.fixed_sites,
              'banding_inspection': args.banding, 'work': str(work)}
    if args.mask_sheet:
        mask_sheet(images, mask, sites, Path(args.mask_sheet))
    return result


def mask_sheet(images, mask, tiles, path):
    im = images['input']
    # Full sky, not a crop selected after seeing a result.
    source = decode(im.rgb[128:4224, 128:9344], False)
    thumb = cv2.resize(encode(source*3), (1536, 683), interpolation=cv2.INTER_AREA)
    overlay = thumb.copy()
    small = cv2.resize(mask[128:4224, 128:9344].astype(np.float32), (1536, 683), interpolation=cv2.INTER_AREA)
    overlay = overlay*(1-.65*small[..., None])+np.array([1, .12, .12])*.65*small[..., None]
    canvas = np.full((2260, 1536, 3), 24, np.uint8)
    canvas[30:713] = np.rint(thumb*255).astype(np.uint8)
    canvas[743:1426] = np.rint(overlay*255).astype(np.uint8)
    for x, y in tiles:
        cv2.rectangle(canvas, (int((x-128)/6), 743+int((y-128)/6)),
                      (int((x+256-128)/6), 743+int((y+256-128)/6)), (0, 255, 0), 1)
    cv2.putText(canvas, 'F1 input sky | common linear gain 3, sRGB transfer', (10, 22), cv2.FONT_HERSHEY_SIMPLEX, .6, (240, 240, 240), 1)
    cv2.putText(canvas, 'Red: union star mask, radius >= 3 FWHM / 4 px. Green: qualified tiles.', (10, 735), cv2.FONT_HERSHEY_SIMPLEX, .6, (240, 240, 240), 1)
    for row, (x, y) in enumerate(((2752, 768), tuple(tiles[0]))):
        top = 1440+row*410
        exclusion = mask[y:y+384, x:x+384]
        crops = []
        for name in ('input', 'LR-OFF'):
            p = images[name].patch(x, y, 384)
            crops.append(encode(p*3))
        for col, (label, p, over) in enumerate((('input', crops[0], False), ('LR OFF', crops[1], False),
                                                ('input + mask', crops[0], True), ('LR OFF + mask', crops[1], True))):
            if over:
                p = p*(1-.5*exclusion[..., None])+np.array([1, .12, .12])*.5*exclusion[..., None]
            canvas[top+22:top+406, col*384:(col+1)*384] = np.rint(p*255).astype(np.uint8)
            cv2.putText(canvas, f'{label} | 100% ({x},{y})', (col*384+5, top+17), cv2.FONT_HERSHEY_SIMPLEX, .43, (240, 240, 240), 1)
    cv2.imwrite(str(path), canvas[..., ::-1])


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--root', default=os.environ.get('AUTOSHADE_FIXTURES_ROOT'))
    for name in ('input', 'ours', 'lr-off', 'lr-on'):
        ap.add_argument('--'+name)
    ap.add_argument('--candidate', action='append', default=[])
    ap.add_argument('--work'); ap.add_argument('--json'); ap.add_argument('--mask-sheet')
    ap.add_argument('--fixed-sites', help='reuse an input-defined star-mask.npy and flat-sites.npy directory')
    ap.add_argument('--banding', choices=('clear', 'visible', 'unreviewed'), default='unreviewed')
    args = ap.parse_args()
    if not args.root:
        ap.error('provide --root or AUTOSHADE_FIXTURES_ROOT; no home-directory default is guessed')
    if any(getattr(args, name) is None for name in ('input', 'ours', 'lr_off', 'lr_on')):
        ap.error('--input, --ours, --lr-off and --lr-on are required')
    temp = None
    work = Path(args.work) if args.work else Path((temp := tempfile.TemporaryDirectory(prefix='denoise-standard-')).name)
    work.mkdir(parents=True, exist_ok=True)
    stamp = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()[:16]
    work = work/stamp; work.mkdir(exist_ok=True)
    try:
        result = run(args, work)
    except (ValueError, OSError) as error:
        ap.exit(2, str(error)+'\n')
    if args.json:
        Path(args.json).write_text(json.dumps(result, indent=2, allow_nan=False)+'\n', encoding='utf-8')
    for label, gates in result['gates'].items():
        n, st = result['noise'][label], result['stars'][label]
        ln, ls = result['noise']['Lightroom'], result['stars']['Lightroom']
        width = lambda row: row['fine_y_p10_p90'][1]-row['fine_y_p10_p90'][0]
        pairs = [(n['fine_y'], ln['fine_y']), (width(n), width(ln)),
                 ([n['fine_colourfulness_magnitude'], n['fine_colourfulness']], [ln['fine_colourfulness_magnitude'], ln['fine_colourfulness']]),
                 ([n['mottle_colourfulness_magnitude'], n['mottle_colourfulness']], [ln['mottle_colourfulness_magnitude'], ln['mottle_colourfulness']]),
                 (st['faint_retention_percent'], ls['faint_retention_percent']),
                 ([st['peak_ratio'], st['fwhm_change']], [ls['peak_ratio'], ls['fwhm_change']]),
                 (st['core_colour_change'], ls['core_colour_change']),
                 (n['glow_correlation'], '>=0.999; banding '+args.banding)]
        for i, (value, reference) in enumerate(pairs, 1):
            print(label, i, gates[str(i)], 'value', value, 'Lightroom/limit', reference)
        print(label, 'lines 9--12', json.dumps(result['absolute'][label]))
    mask_ok = result['mask']['lr_fine_y_width'] <= .03
    print('Mask proof', 'PASS' if mask_ok else 'FAIL', ': Lightroom fine-Y p10--p90 width', result['mask']['lr_fine_y_width'], '(limit .03)')
    if temp:
        temp.cleanup()
    return 0 if mask_ok and all(v == 'PASS' for v in result['gates']['NEW'].values()) else 1


if __name__ == '__main__':
    raise SystemExit(main())
