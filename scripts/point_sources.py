"""Point sources for the CLEAN side of the RAW cleaner's training.

Why this exists. `autoshade-raw-denoise-v1` never saw a star. Its clean targets
are daytime scenes and the operator's low-ISO frames, its synthetic noise adds
isolated positive spikes as DEFECTS (`noise_synth.synthesize`, the hot-pixel
term), and its loss was L1 in the stabilised domain, whose minimiser is the
conditional median. On one colour plane of a Bayer mosaic a star of 2.6 px FWHM
is one sample wide, so the network learned that a faint isolated positive spike
is noise: on a synthetic star field with known fluxes it kept 10 % of the flux
of a star whose G-plane peak stood 4 sigma over the noise, 73 % at 10 sigma and
97 % at 40 (measured 2026-09-21), and on the operator's star frame the sky's
mean fell by 0.4-1.9 DN per plane, the flux of stars too faint to stand alone.

What a star is that a defect is not: it is in ALL FOUR planes at one place, with
one shape, and its light arrives as photons. So a star is added to the clean
target as signal, before the noise is drawn, and everything about it that the
sensor does not know is drawn per crop or per star:

    one optics per crop     minor FWHM 1.5-5 mosaic px, axis ratio down to 0.4
                            (untracked exposures trail), any angle; R and B
                            wider or narrower than G by up to 20 %, and moved
                            against G by a lateral-CA shift of a few tenths of
                            a pixel; each star's size jitters by 5 %
    one colour per star     R/G and B/G about the raw response to daylight,
                            tilted against each other over the range of
                            stellar temperatures
    brightness              the G-plane peak in units of the noise sigma where
                            the star stands, sqrt(a * background + b)

Two kinds of crop carry stars, and half of all crops carry none, so a scene
without stars keeps the prior it has:

    field   a luminosity function, N(> s) ~ s^-alpha with alpha 0.5-1.6, from
            0.3 sigma up: many stars too faint to stand alone under a few that
            do. This is the case the measurement above is about -- the flux a
            median-seeking cleaner erases belongs to stars no test can find one
            by one, and only an estimator whose prior holds them keeps it.
    sparse  1-25 stars, log-uniform 2-150 sigma: the first stars of dusk.

`add_to_pair` is what training calls. For a SYNTHETIC sample the stars join the
clean crop and the noise model draws photons for the sum. For a REAL pair the
noisy frame already holds its scene's photons, and Poisson counts add: the
stars' own counts, a * Poisson(s / a) at that frame's measured gain, are added
to it, and the stars to its reference.
"""
import math

import numpy as np
import torch

# Mosaic (row, col) of each plane's first photosite, RGGB: R, G1, G2, B.
CFA_OFFSETS = ((0, 0), (0, 1), (1, 0), (1, 1))
# Plane -> colour index into a star's (R, G, B) amplitudes.
PLANE_COLOUR = (0, 1, 1, 2)
# Half-width of the window a star is drawn into, in plane samples (24 mosaic px
# across): 3.1 sigma of the longest major axis `draw` allows.
REACH = 6
LONGEST_MAJOR_FWHM = 9.0
FWHM_TO_SIGMA = 1.0 / (2.0 * math.sqrt(2.0 * math.log(2.0)))


def draw(rng, n, height, width, share=0.5, field_share=0.7, most=3000):
    """The stars of `n` crops of `height` x `width` plane samples.

    Returns a dict of flat numpy arrays, one entry per star: `sample`, centre
    `y`, `x` (mosaic px, photosite centres at integers), `snr` (G-plane peak
    over the local noise sigma), colour ratios `r_g`, `b_g`, size jitter
    `size`; and per-crop optics under `optics` (n x 9: minor sigma, major
    sigma, angle, R scale, B scale, then the lateral-CA shift of R as (y, x)
    and of B as (y, x), all in mosaic px). A crop without stars simply has no
    entries."""
    stars = {k: [] for k in ("sample", "y", "x", "snr", "r_g", "b_g", "size")}
    optics = np.zeros((n, 9), np.float32)
    for i in range(n):
        minor = math.exp(rng.uniform(math.log(1.5), math.log(5.0)))
        ratio = rng.uniform(max(0.4, minor / LONGEST_MAJOR_FWHM), 1.0)
        shifts = np.clip(rng.normal(0.0, 0.3, 4), -0.8, 0.8)
        optics[i] = (minor * FWHM_TO_SIGMA, minor / ratio * FWHM_TO_SIGMA, rng.uniform(0.0, math.pi),
                     rng.uniform(0.92, 1.2), rng.uniform(0.92, 1.2), *shifts)
        if rng.random() >= share:
            continue
        if rng.random() < field_share:
            alpha = rng.uniform(0.5, 1.6)
            over_five = math.exp(rng.uniform(math.log(2.0), math.log(120.0)))
            faintest = max(0.3, 5.0 * (over_five / most) ** (1.0 / alpha))
            count = int(min(most, round(over_five * (5.0 / faintest) ** alpha)))
            snr = np.minimum(faintest * rng.random(count) ** (-1.0 / alpha), 400.0)
        else:
            count = int(rng.integers(1, 26))
            snr = np.exp(rng.uniform(math.log(2.0), math.log(150.0), count))
        tilt = np.clip(rng.normal(0.0, 0.5, count), -1.2, 1.2)
        stars["sample"].append(np.full(count, i, np.int64))
        stars["y"].append(rng.uniform(0.0, 2.0 * height, count))
        stars["x"].append(rng.uniform(0.0, 2.0 * width, count))
        stars["snr"].append(snr)
        stars["r_g"].append(0.45 * np.exp(-0.55 * tilt + rng.normal(0.0, 0.15, count)))
        stars["b_g"].append(0.60 * np.exp(0.55 * tilt + rng.normal(0.0, 0.15, count)))
        stars["size"].append(np.exp(rng.normal(0.0, 0.05, count)))
    out = {k: (np.concatenate(v) if v else np.zeros(0, np.int64 if k == "sample" else np.float64))
           for k, v in stars.items()}
    out["optics"] = optics
    return out


def _inverse_covariance(minor, major, angle, scale):
    """The inverse of R diag(major^2, minor^2) R^T * scale^2 + I/12 (the last
    term is the photosite's own square aperture), as (ixx, ixy, iyy)."""
    c, s = torch.cos(angle), torch.sin(angle)
    a, b = (major * scale) ** 2, (minor * scale) ** 2
    cxx = a * c * c + b * s * s + 1.0 / 12.0
    cyy = a * s * s + b * c * c + 1.0 / 12.0
    cxy = (a - b) * c * s
    det = cxx * cyy - cxy * cxy
    return cyy / det, -cxy / det, cxx / det


def render(stars, peak_g, height, width, device):
    """(n, 4, height, width): every star of `stars` at G-plane peak `peak_g`
    (one value per star, in x units), sampled where each plane's photosites
    stand. float32, >= 0."""
    n = stars["optics"].shape[0]
    out = torch.zeros((n, 4, height, width), device=device)
    count = len(stars["sample"])
    if count == 0:
        return out
    t = lambda k, dt=torch.float32: torch.as_tensor(stars[k], device=device).to(dt)
    sample = t("sample", torch.int64)
    y, x, size = t("y"), t("x"), t("size")
    optics = torch.as_tensor(stars["optics"], device=device)[sample]
    minor, major, angle = optics[:, 0] * size, optics[:, 1] * size, optics[:, 2]
    amplitude = (peak_g * t("r_g"), peak_g, peak_g * t("b_g"))
    reach = torch.arange(-REACH, REACH + 1, device=device)
    dy, dx = torch.meshgrid(reach, reach, indexing="ij")
    dy, dx = dy.reshape(1, -1), dx.reshape(1, -1)
    flat = out.view(-1)
    for plane, (oy, ox) in enumerate(CFA_OFFSETS):
        colour = PLANE_COLOUR[plane]
        scale = (optics[:, 3], torch.ones_like(minor), optics[:, 4])[colour]
        zero = torch.zeros_like(minor)
        sy = (optics[:, 5], zero, optics[:, 7])[colour]
        sx = (optics[:, 6], zero, optics[:, 8])[colour]
        ixx, ixy, iyy = _inverse_covariance(minor, major, angle, scale)
        cy, cx = y + sy, x + sx
        i0 = torch.round((cy - oy) / 2.0).to(torch.int64).view(-1, 1) + dy
        j0 = torch.round((cx - ox) / 2.0).to(torch.int64).view(-1, 1) + dx
        ey = (2 * i0 + oy).float() - cy.view(-1, 1)
        ex = (2 * j0 + ox).float() - cx.view(-1, 1)
        value = amplitude[colour].view(-1, 1) * torch.exp(
            -0.5 * (ixx.view(-1, 1) * ex * ex + 2.0 * ixy.view(-1, 1) * ex * ey + iyy.view(-1, 1) * ey * ey))
        inside = (i0 >= 0) & (i0 < height) & (j0 >= 0) & (j0 < width)
        index = ((sample.view(-1, 1) * 4 + plane) * height + i0) * width + j0
        flat.index_add_(0, index[inside], value[inside])
    return out


def add_to_pair(clean, noisy, a, b, rng, gen, share=0.5):
    """Stars for one batch. `clean` (n,4,H,W); `noisy` is None for samples whose
    noise is still to be drawn, else the real noisy frames (n,4,H,W); `a`, `b`
    (n,4) are the noise model per plane, which sets what "one sigma" is where a
    star stands and, for a real frame, the size of one photon.

    Returns (clean + stars, noisy + the stars' own photon counts or None, the
    stars alone). Everything is clamped at 1, the sensor's ceiling."""
    n, _, height, width = clean.shape
    stars = draw(rng, n, height, width, share)
    count = len(stars["sample"])
    if count == 0:
        return clean, noisy, torch.zeros_like(clean)
    dev = clean.device
    sample = torch.as_tensor(stars["sample"], device=dev)
    # One sigma where the star stands: the G1 sample nearest its centre.
    i0 = torch.as_tensor(np.clip(np.round(stars["y"] / 2.0), 0, height - 1), device=dev).to(torch.int64)
    j0 = torch.as_tensor(np.clip(np.round((stars["x"] - 1.0) / 2.0), 0, width - 1), device=dev).to(torch.int64)
    ground = clean[sample, 1, i0, j0].clamp_min(0.0)
    sigma = torch.sqrt(a[sample, 1] * ground + b[sample, 1])
    peak = torch.as_tensor(stars["snr"], device=dev).float() * sigma
    light = render(stars, peak, height, width, dev)
    if noisy is not None:
        photon = a.view(n, 4, 1, 1)
        noisy = (noisy + torch.poisson(light / photon, generator=gen) * photon).clamp(max=1.0)
    return (clean + light).clamp(max=1.0), noisy, light
