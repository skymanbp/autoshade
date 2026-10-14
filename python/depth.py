"""Depth map for AI lens blur (2026-10-13, user: 「AI 景深虚化」).

  python depth.py --input photo.png --output depth.png [--cache DIR] [--cpu]
  python depth.py --self-test                               # pins + vendored code

Writes a 16-bit grayscale PNG of RELATIVE NEARNESS (normalised inverse depth):
65535 is the nearest thing in the frame, 0 the farthest — and everything the
model calls "no surface" (sky, the far haze it masks out) is 0 too, so a
background blur takes it with the horizon. The render reads only differences
from the focus point's value, so the absolute scale the model guessed is not
part of the contract; the GUI's Amount slider is.

The model is MoGe-2 ViT-L with normals (`Ruicheng/moge-2-vitl-normal`, MIT;
its DINOv2 backbone code is Apache-2.0). It was chosen over Depth Anything 3
mono-large on five photos from the maintainer's album, compared side by side
through the same blur: MoGe-2 kept twig, railing and hair-scale edges, and
ordered a palm in front of the people behind it, where DA3 put them level.
Depth Anything V2's Base / Large are CC-BY-NC — not usable in an MIT program
whose users may edit for clients, the same reason SegFormer was refused.

The inference code is VENDORED in `python/_moge/` (see its VENDORED.md): the
upstream package pulls a web UI, a mesh library and a CUDA extension that
must compile, none of which inference needs. The checkpoint is fetched through
the family gate: `_sidecar.fetch_model` -> `denoise._fetch_verified`, our
mirror first, sha256 + byte cap enforced.
"""

import argparse
import os
import sys

try:
    import _sidecar
except ImportError as e:  # pragma: no cover - environment shape, not logic
    print(f"depth.py: cannot import _sidecar.py ({e}) - it must sit beside depth.py in python/.", file=sys.stderr)
    sys.exit(2)
from _device import pick_device

TAG = "depth"
HERE = os.path.dirname(os.path.abspath(__file__))

# MoGe-2 ViT-L + normal head, pinned 2026-10-13. Digest = the Hub's LFS oid for
# model.pt at this revision, and equal to the sha256 of the bytes downloaded
# for the side-by-side trial.
MODEL = {
    "repo": "Ruicheng/moge-2-vitl-normal",
    "revision": "cb0e8bbd6b1e243589717c78e750b1ba4c093acf",
    "files": {
        "model.pt": {
            "sha256": "280741fd09bc3f403ccff9967784c2a391b52d2c0742ae3efdb21d9f90cc1a01",
            "bytes": 1323815904,
        },
    },
}
BACKEND = "moge-2-vitl-normal"

# The long edge the model is fed. MoGe-2 works in ViT tokens, not pixels, so a
# larger input only costs memory; 1536 holds a 3:2 frame in ~3 GB of VRAM in
# fp16 and gives the edge detail the blur needs. The render resamples the map
# to whatever size it draws.
INFER_EDGE = 1536
# Upstream's `resolution_level=9` — the top of the checkpoint's token range,
# the setting the trial was judged at.
RESOLUTION_LEVEL = 9
# Nearness is normalised between these percentiles of the valid pixels, so one
# speck the model put at the lens does not flatten everything else to 0.
NEAR_PCT, FAR_PCT = 99.5, 0.5


def die(msg):
    _sidecar.die(TAG, msg)


def load_model(cache_dir, device):
    import torch
    from _moge.model.v2 import MoGeModel

    d = _sidecar.fetch_model(MODEL, cache_dir, "MoGe-2")
    model = MoGeModel.from_pretrained(os.path.join(d, "model.pt"))
    return model.to(device).eval(), torch


def nearness(model, torch, rgb, device):
    """Relative nearness in 0..1 (1 = nearest, 0 = farthest or no surface)
    for an HxWx3 uint8 RGB array, at its own size."""
    import numpy as np
    from _moge.utils.geometry_torch import recover_focal_shift

    image = torch.tensor(rgb, dtype=torch.float32, device=device).permute(2, 0, 1)[None] / 255.0
    lo, hi = model.num_tokens_range
    num_tokens = int(lo + (RESOLUTION_LEVEL / 9) * (hi - lo))
    with torch.inference_mode():
        # fp16 only on CUDA: upstream's default, and the trial's setting.
        with torch.autocast(device_type=device.split(":")[0], dtype=torch.float16, enabled=device.startswith("cuda")):
            out = model.forward(image, num_tokens=num_tokens)
        points = out["points"].float()
        mask = out["mask"].float() > 0.5 if "mask" in out else torch.ones(points.shape[:-1], dtype=torch.bool, device=points.device)
        # The point map is affine-invariant: its z needs the shift that makes
        # the projection consistent. This is upstream `infer`'s own recovery,
        # minus the intrinsics it would build with utils3d (not vendored, and
        # nothing a depth ordering needs).
        _, shift = recover_focal_shift(points, mask)
        depth = points[..., 2] + shift[..., None, None]
        mask &= depth > 0
    depth = depth[0].cpu().numpy()
    mask = mask[0].cpu().numpy()
    if not mask.any():
        die("the model found no surface in this image")
    inv = np.zeros_like(depth, dtype=np.float64)
    inv[mask] = 1.0 / depth[mask]
    near, far = np.percentile(inv[mask], NEAR_PCT), np.percentile(inv[mask], FAR_PCT)
    if not near > far:
        die("the depth map is flat (every surface at one distance)")
    n = np.clip((inv - far) / (near - far), 0.0, 1.0)
    n[~mask] = 0.0
    return n


def write_png16(path, n):
    """tmp + fsync + os.replace (L03), as every artifact in this family."""
    import cv2
    import numpy as np

    ok, buf = cv2.imencode(".png", np.round(n * 65535.0).astype(np.uint16))
    if not ok:
        die(f"could not encode {path}")
    tmp = f"{path}.{os.getpid()}.tmp"
    try:
        with open(tmp, "wb") as f:
            f.write(buf.tobytes())
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, path)
    finally:
        if os.path.exists(tmp):
            try:
                os.remove(tmp)
            except OSError:
                pass  # why: best-effort cleanup, because the write's own exception is already propagating


def self_test():
    for name, pin in MODEL["files"].items():
        digest = pin.get("sha256", "")
        if len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
            raise SystemExit(f"every_pinned_file_has_a_digest: {name} has no 64-hex sha256")
        if not isinstance(pin.get("bytes"), int) or pin["bytes"] <= 0:
            raise SystemExit(f"every_pinned_file_has_a_byte_cap: {name}")
    print("every_pinned_file_has_a_digest_and_a_byte_cap: PASS")
    for rel in ("_moge/model/v2.py", "_moge/utils/geometry_torch.py", "_moge/LICENSE", "_moge/VENDORED.md"):
        if not os.path.exists(os.path.join(HERE, rel)):
            raise SystemExit(f"the_vendored_model_code_ships: {rel} is missing")
    print("the_vendored_model_code_ships: PASS")
    # The marked edits are what keep the vendored package free of the
    # dependencies it was vendored to avoid.
    with open(os.path.join(HERE, "_moge", "model", "v2.py"), encoding="utf-8") as f:
        v2 = f.read()
    if "import utils3d" in v2 or "from huggingface_hub" in v2:
        raise SystemExit("the_vendored_code_needs_no_extra_package: v2.py imports utils3d / huggingface_hub")
    print("the_vendored_code_needs_no_extra_package: PASS")


def parse_args():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--self-test", action="store_true", help="check the pins and the vendored code, then exit")
    ap.add_argument("--input", help="the photo, in any format OpenCV reads")
    ap.add_argument("--output", help="where the 16-bit nearness PNG goes")
    ap.add_argument("--cpu", action="store_true", help="never use an accelerator")
    ap.add_argument("--cache", default=os.path.join(HERE, "weights"), help="the pinned-weights directory")
    return ap.parse_args()


def main():
    a = parse_args()
    if a.self_test:
        return self_test()
    if not (a.input and a.output):
        die("--input and --output are required")
    import cv2
    import numpy as np

    # Bytes first, then decode: `cv2.imread` cannot open a non-ASCII path on
    # Windows (a store under a Chinese user name, an album called 相册).
    try:
        bgr = cv2.imdecode(np.fromfile(a.input, dtype=np.uint8), cv2.IMREAD_COLOR)
    except OSError:
        bgr = None
    if bgr is None:
        die(f"cannot read {a.input}")
    h, w = bgr.shape[:2]
    s = min(1.0, INFER_EDGE / max(h, w))
    if s < 1.0:
        # INTER_AREA: a named filter, as every resize in this family.
        bgr = cv2.resize(bgr, (max(1, round(w * s)), max(1, round(h * s))), interpolation=cv2.INTER_AREA)
    device = pick_device(a.cpu)
    _sidecar.log(TAG, f"MoGe-2 on {device}, {bgr.shape[1]}x{bgr.shape[0]}")
    model, torch = load_model(a.cache, device)
    write_png16(a.output, nearness(model, torch, bgr[:, :, ::-1].copy(), device))
    # `depth::estimate_depth` reads the backend out of the brackets.
    print(f"depth.py: depth map [{BACKEND}] -> {a.output}")


if __name__ == "__main__":
    main()
