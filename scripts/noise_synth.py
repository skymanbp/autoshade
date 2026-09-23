"""Physics-based RAW noise synthesis on GPU, randomised over a sensor family.

Everything is in the NORMALISED linear unit the sidecar works in,
x = (DN - black) / (white - black), on the four half-resolution planes
(R, G1, G2, B) of a Bayer mosaic, tensors of shape (B, 4, H, W).

The formation model (ELD, Wei et al. 2020/2021, plus the two defects the
2026-09-17 real-set comparison pointed at):

    noisy = a * Poisson(x / a)                 shot noise, a = gain in x units
          + read                               core read noise, Gaussian or
                                               Tukey-lambda (heavy tails), var b
          + row_r + col_c                      banding: one offset per mosaic
                                               row / column, std rho * sqrt(b)
          + hot                                sparse bright defects
          (read + row + col optionally blurred: in-camera spatial filtering)
    then quantised to the sensor's DN step and the top clipped at 1.

Every parameter is drawn per SAMPLE from a wide range, so the network learns
the family rather than one camera. `sample_params` documents each range.

Provenance — AutoShade v1.5.0. This is the pipeline that produced
`autoshade-raw-denoise-v1.pth` and, continued from it with point sources in v1.6.0,
the `autoshade-raw-denoise-v2.pth` that `python/denoise_raw.py` ships:
DPIR's released `drunet_color` (KAIR, MIT) fine-tuned for that sidecar's own
transform, so nothing is learned that inference cannot reproduce. The real
training pairs are RawNIND (Brummer & De Vleeschouwer, UCLouvain Dataverse,
doi:10.14428/DVN/DEQCIM, CC BY-SA 4.0); the synthetic half is drawn over
low-ISO frames the operator owns. Every path here is an argument or is derived
from this file's location: none of it knows the machine it was written on.
"""
import math

import torch
import torch.nn.functional as F


def _logu(lo, hi, n, gen, device):
    return torch.exp(torch.empty(n, device=device).uniform_(math.log(lo), math.log(hi), generator=gen))


def tukey_lambda_var(lam):
    """Variance of the standard Tukey-lambda distribution (lambda > -0.5)."""
    lam = float(lam)
    if abs(lam) < 1e-4:
        return math.pi ** 2 / 3.0
    g = math.lgamma
    return (2.0 / lam ** 2) * (1.0 / (1.0 + 2.0 * lam) - math.exp(2 * g(lam + 1.0) - g(2.0 * lam + 2.0)))


def tukey_lambda_mad_var(lam):
    """(MAD / 0.6745)^2 of the standard Tukey-lambda — what the sidecar's robust
    white-noise estimator reports for it. Quantile function Q(p) =
    (p^l - (1-p)^l) / l is symmetric, so MAD = Q(0.75)."""
    lam = float(lam)
    if abs(lam) < 1e-4:
        q = math.log(0.75 / 0.25)
    else:
        q = (0.75 ** lam - 0.25 ** lam) / lam
    return (q / 0.6745) ** 2


def sample_params(n, gen, device):
    """Per-sample sensor parameters (each a tensor of length n)."""
    p = {}
    # Shot-noise gain in x units. 5e-5 ~ a clean base-ISO full-frame sensor,
    # 6e-3 ~ ISO 25600-51200 on the same; the operator's ILCE-7RM4A measured
    # 3.0e-4 (ISO 1000) .. 1.9e-3 (ISO 6400) on 2026-09-17.
    p["a"] = _logu(5e-5, 6e-3, n, gen, device)
    # Read-noise floor as b / a^2 = read noise in electrons squared:
    # 0.3 .. 60 covers modern BSI (~1-2 e-) to old CCD/CMOS (~7 e-).
    p["r"] = _logu(0.3, 60.0, n, gen, device)
    p["b"] = p["a"] ** 2 * p["r"]
    # Heavy tails on half the samples.
    p["tukey"] = torch.rand(n, generator=gen, device=device) < 0.5
    p["lam"] = torch.empty(n, device=device).uniform_(-0.25, 0.15, generator=gen)
    # Banding.
    p["rho_row"] = torch.where(torch.rand(n, generator=gen, device=device) < 0.6,
                               torch.empty(n, device=device).uniform_(0.0, 0.45, generator=gen),
                               torch.zeros(n, device=device))
    p["rho_col"] = torch.where(torch.rand(n, generator=gen, device=device) < 0.3,
                               torch.empty(n, device=device).uniform_(0.0, 0.12, generator=gen),
                               torch.zeros(n, device=device))
    # Hot pixels.
    p["hot_rate"] = torch.where(torch.rand(n, generator=gen, device=device) < 0.5,
                                _logu(1e-6, 3e-4, n, gen, device), torch.zeros(n, device=device))
    # In-camera spatial filtering of the read noise (std of a Gaussian, plane px).
    p["blur"] = torch.where(torch.rand(n, generator=gen, device=device) < 0.15,
                            torch.empty(n, device=device).uniform_(0.35, 0.9, generator=gen),
                            torch.zeros(n, device=device))
    # 12- or 14-bit DN step.
    p["step"] = torch.where(torch.rand(n, generator=gen, device=device) < 0.3,
                            torch.full((n,), 1.0 / 3500.0, device=device),
                            torch.full((n,), 1.0 / 15871.0, device=device))
    return p


def _tukey(shape, lam, gen, device):
    u = torch.rand(shape, generator=gen, device=device).clamp_(1e-6, 1 - 1e-6)
    if abs(lam) < 1e-4:
        return torch.log(u / (1 - u))
    return (u ** lam - (1 - u) ** lam) / lam


def _blur(x, sigma):
    if sigma <= 0:
        return x
    rad = max(1, int(math.ceil(2.5 * sigma)))
    k = torch.exp(-torch.arange(-rad, rad + 1, device=x.device, dtype=x.dtype) ** 2 / (2 * sigma ** 2))
    k = k / k.sum()
    c = x.shape[1]
    y = F.conv2d(F.pad(x, (rad, rad, 0, 0), mode="reflect"), k.view(1, 1, 1, -1).repeat(c, 1, 1, 1), groups=c)
    y = F.conv2d(F.pad(y, (0, 0, rad, rad), mode="reflect"), k.view(1, 1, -1, 1).repeat(c, 1, 1, 1), groups=c)
    # keep the variance of a white field: sum k^2 over the 2-D kernel
    return y / math.sqrt(float((k ** 2).sum()) ** 2)


def haar_mad_var(field):
    """(MAD / 0.6745)^2 of the finest-scale Haar diagonal of a (C,H,W) field —
    the white-noise variance the sidecar's estimator reads off a flat block.
    Row / column offsets cancel in the diagonal, tails shrink the MAD and a
    spatial blur lowers it, exactly as they do for the real estimator."""
    h, w = field.shape[-2] // 2 * 2, field.shape[-1] // 2 * 2
    f = field[..., :h, :w]
    d = (f[..., 0::2, 0::2] - f[..., 0::2, 1::2] - f[..., 1::2, 0::2] + f[..., 1::2, 1::2]) / 2.0
    d = d.reshape(-1)
    med = d.median()
    return float(((d - med).abs().median() / 0.6745) ** 2)


def synthesize(x, p, gen):
    """x: (B,4,H,W) clean, >= 0. Returns (noisy, b_seen): b_seen is the
    read-noise variance the sidecar's robust estimator would report for this
    sample's signal-independent field (see `haar_mad_var`)."""
    B, C, H, W = x.shape
    dev = x.device
    a = p["a"].view(B, 1, 1, 1)
    shot = torch.poisson((x.clamp_min(0) / a).float(), generator=gen) * a
    read = torch.empty_like(x)
    b_seen = torch.empty(B, device=dev)
    for i in range(B):
        sd = float(torch.sqrt(p["b"][i]))
        if bool(p["tukey"][i]):
            lam = float(p["lam"][i])
            r = _tukey((1, C, H, W), lam, gen, dev) / math.sqrt(tukey_lambda_var(lam))
        else:
            r = torch.randn((1, C, H, W), generator=gen, device=dev)
        r = r * sd
        # Banding: plane rows 0.. map to mosaic rows 2k (R,G1) and 2k+1 (G2,B);
        # plane columns to 2k (R,G2) and 2k+1 (G1,B).
        rr = float(p["rho_row"][i])
        if rr > 0:
            rows = torch.randn((2, H), generator=gen, device=dev) * rr * sd
            r[:, 0] += rows[0].view(H, 1); r[:, 1] += rows[0].view(H, 1)
            r[:, 2] += rows[1].view(H, 1); r[:, 3] += rows[1].view(H, 1)
        rc = float(p["rho_col"][i])
        if rc > 0:
            cols = torch.randn((2, W), generator=gen, device=dev) * rc * sd
            r[:, 0] += cols[0].view(1, W); r[:, 2] += cols[0].view(1, W)
            r[:, 1] += cols[1].view(1, W); r[:, 3] += cols[1].view(1, W)
        bl = float(p["blur"][i])
        if bl > 0:
            r = _blur(r, bl)
        hr = float(p["hot_rate"][i])
        if hr > 0:
            m = torch.rand((1, C, H, W), generator=gen, device=dev) < hr
            r = torch.where(m, r + torch.rand((1, C, H, W), generator=gen, device=dev) * 0.3, r)
        read[i] = r[0]
        b_seen[i] = haar_mad_var(r[0])
    noisy = shot + read
    step = p["step"].view(B, 1, 1, 1)
    noisy = torch.round(noisy / step) * step
    return noisy.clamp(max=1.0), b_seen


# ── the sidecar's transform, in torch ─────────────────────────────────────────

def gat(x, a, b):
    return 2.0 * torch.sqrt(torch.clamp(x / a + 3.0 / 8.0 + b / (a * a), min=0.0))


_S15 = math.sqrt(1.5)


def i_a(z):
    return 0.25 * z ** 2 + 0.25 * _S15 / z - (11.0 / 8.0) / z ** 2 + (5.0 / 8.0) * _S15 / z ** 3 - 0.125


def i_a_prime(z):
    return 0.5 * z - 0.25 * _S15 / z ** 2 + (11.0 / 4.0) / z ** 3 - (15.0 / 8.0) * _S15 / z ** 4


def i_a_inverse(y, iters=4):
    """z >= 1 with I_A(z) = y (I_A is monotone there; I_A(1) = -0.179 lies
    below every target this is asked for, which are x/a + b/a^2 >= 0)."""
    z = 2.0 * torch.sqrt(torch.clamp(y, min=0.0) + 0.125)
    z = torch.clamp(z, min=1.0)
    for _ in range(iters):
        z = torch.clamp(z - (i_a(z) - y) / i_a_prime(z), min=1.0)
    return z


def igat(z, a, b):
    z = torch.clamp(z, min=1e-3)
    return a * (i_a(z) - b / (a * a))
