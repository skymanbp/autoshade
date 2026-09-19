#!/usr/bin/env python3
"""Download the Bayer half of RawNIND (UCLouvain Dataverse, doi:10.14428/DVN/DEQCIM)
into --root, verifying every file against the sha1 its own name carries.

Order: the held-out scenes first (unknown_sensor + test_reserve — validation can
start while the rest arrives), then the non-Sony cameras, then the Sony A7C
scenes. Resumable: a file already present with the right sha1 is skipped; a
partial or corrupt one is fetched again. N workers in parallel.
Provenance — AutoShade v1.5.0. This is the pipeline that produced
`autoshade-raw-denoise-v1.pth`, the weights `python/denoise_raw.py` ships:
DPIR's released `drunet_color` (KAIR, MIT) fine-tuned for that sidecar's own
transform, so nothing is learned that inference cannot reproduce. The real
training pairs are RawNIND (Brummer & De Vleeschouwer, UCLouvain Dataverse,
doi:10.14428/DVN/DEQCIM, CC BY-SA 4.0); the synthetic half is drawn over
low-ISO frames the operator owns. Every path here is an argument or is derived
from this file's location: none of it knows the machine it was written on.
"""
import argparse
import concurrent.futures as cf
import hashlib
import json
import pathlib
import sys
import time
import urllib.request

import yaml

API = "https://dataverse.uclouvain.be/api/access/datafile/{}"


def sha1_of(path):
    h = hashlib.sha1()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def fetch(item, root, tries=4):
    name, fid, sha1, size = item
    dest = root / name
    if dest.exists() and dest.stat().st_size == size and sha1_of(dest) == sha1:
        return name, "kept", 0
    tmp = dest.with_suffix(dest.suffix + ".part")
    for attempt in range(tries):
        try:
            t0 = time.time()
            with urllib.request.urlopen(API.format(fid), timeout=120) as r, open(tmp, "wb") as out:
                while True:
                    buf = r.read(1 << 20)
                    if not buf:
                        break
                    out.write(buf)
            got = sha1_of(tmp)
            if got != sha1:
                raise IOError(f"sha1 {got} != {sha1}")
            tmp.replace(dest)
            return name, "fetched", time.time() - t0
        except Exception as e:  # noqa: BLE001  why: a network or checksum failure is retried, then reported by name
            last = e
            time.sleep(5 * (attempt + 1))
    return name, f"FAILED {last}", 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", required=True)
    ap.add_argument("--workers", type=int, default=3)
    args = ap.parse_args()
    root = pathlib.Path(args.root)
    (root / "raw").mkdir(parents=True, exist_ok=True)
    files = {f["dataFile"]["filename"]: f["dataFile"] for f in json.load(open(root / "files.json", encoding="utf-8"))}
    scenes = yaml.safe_load(open(root / "dataset.yaml", encoding="utf-8"))["Bayer"]

    def rank(item):
        name, s = item
        imgs = s.get("clean_images", []) + s.get("noisy_images", [])
        sony = any(i["filename"].lower().endswith(".arw") for i in imgs)
        held = s.get("unknown_sensor") or s.get("test_reserve")
        return (0 if held else 1, 1 if sony else 0, name)

    queue, missing = [], []
    for name, s in sorted(scenes.items(), key=rank):
        for i in s.get("clean_images", []) + s.get("noisy_images", []):
            df = files.get(i["filename"])
            if df is None:
                missing.append(i["filename"])
                continue
            queue.append((i["filename"], df["id"], i["sha1"], df["filesize"]))
    total = sum(q[3] for q in queue)
    print(f"{len(queue)} files, {total / 1e9:.1f} GB; {len(missing)} listed in dataset.yaml but absent from the file list", flush=True)
    for m in missing:
        print(f"  absent: {m}", flush=True)
    done_bytes, t0 = 0, time.time()
    with cf.ThreadPoolExecutor(args.workers) as ex:
        futs = {ex.submit(fetch, q, root / "raw"): q for q in queue}
        for n, fut in enumerate(cf.as_completed(futs), 1):
            name, status, dt = fut.result()
            done_bytes += futs[fut][3]
            rate = done_bytes / max(time.time() - t0, 1e-6) / 1e6
            print(f"[{n}/{len(queue)}] {status:8s} {name}  ({done_bytes / 1e9:.1f}/{total / 1e9:.1f} GB, {rate:.1f} MB/s avg)", flush=True)
    print("done", flush=True)


if __name__ == "__main__":
    sys.exit(main())
