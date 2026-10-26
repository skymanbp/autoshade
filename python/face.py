#!/usr/bin/env python3
"""Face finder for the Portraits mode (v1.14.0): where the faces and eyes are.

  python face.py --input photo.png --output faces.json [--cache DIR] [--max-faces N]
  python face.py --self-test                                  # the pin + the outline sets

Writes one small JSON file describing EVERY face MediaPipe's Face Landmarker
finds, in the input image's own pixel coordinates (x right, y down, the
top-left corner of the top-left pixel at 0,0):

  {"version": 1, "backend": "...", "width": W, "height": H,
   "faces": [{"confidence": 0.97 | null,
              "box": [x0, y0, x1, y1],
              "oval": [[x, y], ...],        # the face outline, in order
              "left_eye": [[x, y], ...],    # MediaPipe's LEFT = the person's left,
              "right_eye": [[x, y], ...]}]} # which is the eye on the image's right

The Rust side (`src/face.rs`) turns the outlines into soft masks; nothing here
decides what is edited. No face found is not an error: the file says
`"faces": []` and the app greys out the face controls.

`confidence` is the face DETECTOR's score for the face the landmarker outlined
— the landmarker itself returns no per-face score. Both models come out of the
one pinned bundle, so the score costs no second download. `null` when no
detection overlaps the outline well enough to be called the same face.

The model is MediaPipe's Face Landmarker bundle, float16, version 1
(`face_landmarker.task`: a BlazeFace short-range detector, Face Mesh V2 and a
blendshape head, all three Apache-2.0 per their model cards). It is fetched
through the family gate — `denoise._fetch_verified`, our mirror first where we
have one, sha256 + byte cap enforced — and handed to MediaPipe as bytes read
from that verified file, never by a path MediaPipe could re-resolve.

The short-range detector is tuned for faces that fill a good part of the frame
(its card: selfie distance). Faces that are small in a wide shot can be missed;
the app says "no face found" rather than guessing.
"""

import argparse
import json
import os
import sys

TAG = "face"
HERE = os.path.dirname(os.path.abspath(__file__))

# MediaPipe Face Landmarker, float16, version 1 — pinned 2026-10-25. The URL is
# VERSIONED (`/1/`), and on that day `/latest/` served the same object (same
# GCS md5 `sOcnSQehZEQE/vZrKN1thQ==`, same 3,758,596 B). The digest is the
# sha256 of the bytes downloaded from the URL below.
MODEL = {
    "url": "https://storage.googleapis.com/mediapipe-models/face_landmarker/face_landmarker/float16/1/face_landmarker.task",
    "dir": "mediapipe--face_landmarker@float16-1",
    "file": "face_landmarker.task",
    "sha256": "64184e229b263107bc2b804c6625db1341ff2bb731874b0bcc2fe6544e0bc9ff",
    "bytes": 3758596,
}
BACKEND = "mediapipe-face-landmarker-float16-1"

# The bundle's own detector, run a second time on its own for the per-face
# score (see the module docstring). A member name of the zip above.
DETECTOR_MEMBER = "face_detector.tflite"

# The longest edge MediaPipe is shown. The detector works at 128x128 and the
# mesh at 256x256 of a face crop, so a 24 MP frame buys nothing but decode
# time; outlines come back normalised and are scaled to the ORIGINAL size.
INFER_EDGE = 2048
# How many faces at most. A group photo past this is a crowd, not a portrait.
MAX_FACES = 8
# Detector and landmarker thresholds — MediaPipe's own defaults, named.
MIN_DETECTION = 0.5
MIN_PRESENCE = 0.5
# A detection and an outline are the same face when their boxes overlap by at
# least this much (intersection over union).
SAME_FACE_IOU = 0.3


def die(msg):
    """Refuse with a sentence on stderr, exit 2 — the family's refusal shape.

    Local rather than `_sidecar.die`: importing `_sidecar` pulls numpy in via
    denoise.py, and `face.py --help` / a missing-dependency refusal should not
    pay for that (the reason `segment.die` is local too)."""
    print(f"{TAG}.py: {msg}", file=sys.stderr)
    sys.exit(2)


def log(msg):
    print(f"[{TAG}] {msg}", file=sys.stderr, flush=True)


# --- geometry: pure Python, no MediaPipe, no numpy (unit-tested) ------------


def chain(edges):
    """Order a closed outline given as an unordered set of (start, end) index
    pairs into one ring of landmark indices, starting at the smallest index.

    MediaPipe publishes its outlines (`FACE_LANDMARKS_FACE_OVAL`, the eye sets)
    as CONNECTIONS — the segments a renderer draws — not as polygons. A
    polygon needs them in order. Every index on a closed outline has exactly
    two neighbours; anything else is not one ring and is refused, because a
    wrongly ordered outline fills as a bow-tie and the mask would cover the
    wrong part of the face without anything failing.
    """
    nbrs = {}
    for a, b in edges:
        if a == b:
            raise ValueError(f"outline edge {a}-{b} is a loop")
        nbrs.setdefault(a, set()).add(b)
        nbrs.setdefault(b, set()).add(a)
    if len(nbrs) < 3:
        raise ValueError("an outline needs at least three points")
    for i, n in nbrs.items():
        if len(n) != 2:
            raise ValueError(f"outline index {i} has {len(n)} neighbours, not 2")
    start = min(nbrs)
    ring, prev, cur = [start], None, start
    while True:
        nxt = min(n for n in nbrs[cur] if n != prev) if prev is not None else min(nbrs[cur])
        if nxt == start:
            break
        ring.append(nxt)
        prev, cur = cur, nxt
        if len(ring) > len(nbrs):
            raise ValueError("outline does not close")
    if len(ring) != len(nbrs):
        raise ValueError(f"outline is {len(nbrs)} points in more than one ring")
    return ring


def edges_of(connections):
    """MediaPipe's `Connection(start=, end=)` objects, or plain pairs, as pairs."""
    return [(c.start, c.end) if hasattr(c, "start") else (c[0], c[1]) for c in connections]


def to_pixels(landmarks, ring, width, height):
    """The ring's landmarks (normalised x, y) in pixels of a `width` x `height`
    frame, rounded to 0.01 px so the file stays small."""
    return [[round(landmarks[i].x * width, 2), round(landmarks[i].y * height, 2)] for i in ring]


def bounds(points):
    xs = [p[0] for p in points]
    ys = [p[1] for p in points]
    return [min(xs), min(ys), max(xs), max(ys)]


def iou(a, b):
    ix = max(0.0, min(a[2], b[2]) - max(a[0], b[0]))
    iy = max(0.0, min(a[3], b[3]) - max(a[1], b[1]))
    inter = ix * iy
    union = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter
    return inter / union if union > 0 else 0.0


def shape_faces(meshes, detections, width, height, rings):
    """The JSON `faces` list for one image.

    `meshes`: one list of normalised landmarks (objects with `.x`, `.y`) per
    face. `detections`: `(score, [x0, y0, x1, y1])` in pixels of the same
    frame. `rings`: `{"oval": [...], "left_eye": [...], "right_eye": [...]}`,
    each an ordered index ring from `chain`. Each face's `confidence` is the
    score of the best-overlapping detection not already given to another face.
    """
    faces, taken = [], set()
    for lm in meshes:
        need = max(max(r) for r in rings.values())
        if len(lm) <= need:
            raise ValueError(f"a face came back with {len(lm)} landmarks; the outlines need {need + 1}")
        face = {k: to_pixels(lm, ring, width, height) for k, ring in rings.items()}
        box = bounds(face["oval"])
        best, score = None, None
        for j, (s, dbox) in enumerate(detections):
            if j in taken:
                continue
            o = iou(box, dbox)
            if o >= SAME_FACE_IOU and (best is None or o > best[0]):
                best, score = (o, j), s
        if best is not None:
            taken.add(best[1])
        faces.append({
            "confidence": None if score is None else round(float(score), 4),
            "box": [round(v, 2) for v in box],
            "oval": face["oval"],
            "left_eye": face["left_eye"],
            "right_eye": face["right_eye"],
        })
    return faces


def document(faces, width, height):
    return {"version": 1, "backend": BACKEND, "width": width, "height": height, "faces": faces}


# --- the model --------------------------------------------------------------


def outline_rings(vision):
    """The three outlines from the sets MediaPipe itself publishes."""
    c = vision.FaceLandmarksConnections
    return {
        "oval": chain(edges_of(c.FACE_LANDMARKS_FACE_OVAL)),
        "left_eye": chain(edges_of(c.FACE_LANDMARKS_LEFT_EYE)),
        "right_eye": chain(edges_of(c.FACE_LANDMARKS_RIGHT_EYE)),
    }


def import_mediapipe():
    try:
        import mediapipe as mp
        from mediapipe.tasks.python import BaseOptions, vision
    except ImportError:
        # ASCII-only: Windows consoles in legacy codepages mangle wide dashes.
        die(
            "finding faces needs MediaPipe -> pip install mediapipe "
            "(Face Landmarker, ~3.8 MB, downloads to python/weights on first run)"
        )
    return mp, BaseOptions, vision


def fetch_model(cache_dir):
    try:
        from denoise import _fetch_verified
    except ImportError as e:
        die(f"the shared sidecar downloader from denoise.py is not importable ({e}) - face.py must sit beside denoise.py in python/")
    d = os.path.join(cache_dir, MODEL["dir"])
    os.makedirs(d, exist_ok=True)
    dest = os.path.join(d, MODEL["file"])
    _fetch_verified(MODEL["url"], dest, MODEL["sha256"], MODEL["bytes"] + 4096, "the Face Landmarker bundle")
    with open(dest, "rb") as f:
        return f.read()


def detector_bytes(bundle):
    import io
    import zipfile

    try:
        with zipfile.ZipFile(io.BytesIO(bundle)) as z:
            return z.read(DETECTOR_MEMBER)
    except (zipfile.BadZipFile, KeyError) as e:
        die(f"the pinned Face Landmarker bundle has no {DETECTOR_MEMBER} ({e})")


def find_faces(rgb, cache_dir, max_faces):
    """The `faces` list for an HxWx3 uint8 RGB array, in its own pixels."""
    mp, BaseOptions, vision = import_mediapipe()
    bundle = fetch_model(cache_dir)
    rings = outline_rings(vision)
    h, w = rgb.shape[:2]
    image = mp.Image(image_format=mp.ImageFormat.SRGB, data=rgb)
    opts = vision.FaceLandmarkerOptions(
        base_options=BaseOptions(model_asset_buffer=bundle),
        running_mode=vision.RunningMode.IMAGE,
        num_faces=max_faces,
        min_face_detection_confidence=MIN_DETECTION,
        min_face_presence_confidence=MIN_PRESENCE,
    )
    with vision.FaceLandmarker.create_from_options(opts) as lm:
        meshes = lm.detect(image).face_landmarks
    detections = []
    if meshes:
        dopts = vision.FaceDetectorOptions(
            base_options=BaseOptions(model_asset_buffer=detector_bytes(bundle)),
            running_mode=vision.RunningMode.IMAGE,
            min_detection_confidence=MIN_DETECTION,
        )
        with vision.FaceDetector.create_from_options(dopts) as det:
            for d in det.detect(image).detections:
                b = d.bounding_box
                score = max((c.score for c in d.categories), default=None)
                if score is not None:
                    detections.append((score, [b.origin_x, b.origin_y, b.origin_x + b.width, b.origin_y + b.height]))
    return shape_faces(meshes, detections, w, h, rings)


def publish(path, text):
    """tmp + fsync + os.replace (L03), as every artifact in this family."""
    tmp = f"{path}.{os.getpid()}.tmp"
    try:
        with open(tmp, "w", encoding="utf-8") as f:
            f.write(text)
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
    digest = MODEL["sha256"]
    if len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
        raise SystemExit("the_pin_has_a_digest: no 64-hex sha256")
    if not isinstance(MODEL["bytes"], int) or MODEL["bytes"] <= 0:
        raise SystemExit("the_pin_has_a_byte_cap")
    print("the_pin_has_a_digest_and_a_byte_cap: PASS")
    if "/latest/" in MODEL["url"]:
        raise SystemExit("the_pin_is_a_version: the URL names a moving alias")
    print("the_pin_is_a_version: PASS")
    try:
        from mediapipe.tasks.python import vision
    except ImportError:
        print("the_published_outlines_chain_into_rings: SKIP (mediapipe not installed)")
        return
    rings = outline_rings(vision)
    print("the_published_outlines_chain_into_rings: PASS "
          + ", ".join(f"{k} {len(v)}" for k, v in rings.items()))


def parse_args():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--self-test", action="store_true", help="check the pin and the outline sets, then exit")
    ap.add_argument("--input", help="the photo, in any format OpenCV reads")
    ap.add_argument("--output", help="where the faces JSON goes")
    ap.add_argument("--max-faces", type=int, default=MAX_FACES, help=f"most faces reported (default {MAX_FACES})")
    ap.add_argument("--cache", default=os.path.join(HERE, "weights"), help="the pinned-weights directory")
    return ap.parse_args()


def main():
    a = parse_args()
    if a.self_test:
        return self_test()
    if not (a.input and a.output):
        die("--input and --output are required")
    if not 1 <= a.max_faces <= 32:
        die("--max-faces must be between 1 and 32")
    try:
        import cv2
        import numpy as np
    except ImportError:
        die("finding faces needs OpenCV + numpy -> pip install opencv-python numpy")

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
    small = bgr
    if s < 1.0:
        # INTER_AREA: a named filter, as every resize in this family.
        small = cv2.resize(bgr, (max(1, round(w * s)), max(1, round(h * s))), interpolation=cv2.INTER_AREA)
    rgb = np.ascontiguousarray(small[:, :, ::-1])
    log(f"Face Landmarker on {small.shape[1]}x{small.shape[0]}")
    faces = find_faces(rgb, a.cache, a.max_faces)
    if s < 1.0:
        # Outlines came back in the shrunken frame's pixels; the contract is
        # the ORIGINAL frame's.
        fx, fy = w / small.shape[1], h / small.shape[0]
        for f in faces:
            for k in ("oval", "left_eye", "right_eye"):
                f[k] = [[round(x * fx, 2), round(y * fy, 2)] for x, y in f[k]]
            x0, y0, x1, y1 = f["box"]
            f["box"] = [round(x0 * fx, 2), round(y0 * fy, 2), round(x1 * fx, 2), round(y1 * fy, 2)]
    publish(a.output, json.dumps(document(faces, w, h), separators=(",", ":")))
    # `face::detect_faces` reads the backend out of the brackets.
    print(f"face.py: {len(faces)} face(s) [{BACKEND}] -> {a.output}")


if __name__ == "__main__":
    main()
