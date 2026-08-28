"""Pixel ruler for reverse-fit candidates: a rendered candidate against the
target on a 384-wide box-reduced raster (the fit's analysis geometry), whole
frame and split by a sky mask. Reports mean |dsRGB| (0-255) and mean CIE76 dE.
The numbers quoted in docs/ARCHITECTURE.md and docs/TECH_STACK.md for the
calibration pair come from this script.
Usage: python scripts/pixel_ruler.py <target.jpg> <sky-mask.png> <label=render.png>...
"""
import sys
import numpy as np
from PIL import Image

target_path, mask_path, *specs = sys.argv[1:]
W = 384


def reduce(img, mode):
    img = img.convert(mode)
    h = round(img.height * W / img.width)
    return np.asarray(img.resize((W, h), Image.BOX), dtype=np.float64)


def srgb_to_lab(rgb):
    c = rgb / 255.0
    lin = np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)
    m = np.array([[0.4124564, 0.3575761, 0.1804375],
                  [0.2126729, 0.7151522, 0.0721750],
                  [0.0193339, 0.1191920, 0.9503041]])
    xyz = lin @ m.T / np.array([0.95047, 1.0, 1.08883])
    f = np.where(xyz > 0.008856, np.cbrt(xyz), 7.787 * xyz + 16 / 116)
    L = 116 * f[..., 1] - 16
    a = 500 * (f[..., 0] - f[..., 1])
    b = 200 * (f[..., 1] - f[..., 2])
    return np.stack([L, a, b], axis=-1)


tgt = reduce(Image.open(target_path), "RGB")
sky = reduce(Image.open(mask_path), "L") / 255.0
rows = min(tgt.shape[0], sky.shape[0])
print(f"target {tgt.shape[1]}x{tgt.shape[0]}  mask {sky.shape[1]}x{sky.shape[0]}  compared rows {rows}")
print(f"{'label':<16}{'|dRGB| all':>12}{'sky':>8}{'land':>8}{'dE76 all':>11}{'sky':>8}{'land':>8}")
for spec in specs:
    label, path = spec.split("=", 1)
    ren = reduce(Image.open(path), "RGB")
    r = min(rows, ren.shape[0])
    t, x, m = tgt[:r], ren[:r], sky[:r]
    d = np.abs(x - t).mean(axis=-1)
    de = np.linalg.norm(srgb_to_lab(x) - srgb_to_lab(t), axis=-1)
    land = 1.0 - m
    def part(v, w):
        return (v * w).sum() / max(w.sum(), 1e-6)
    print(f"{label:<16}{d.mean():>12.2f}{part(d, m):>8.2f}{part(d, land):>8.2f}{de.mean():>11.2f}{part(de, m):>8.2f}{part(de, land):>8.2f}")
