# Vendored: MoGe-2 inference code

Source: <https://github.com/microsoft/MoGe> at commit
`74fbce054ebed49800de42d0ad0e83495065719a`, the `moge/` package minus
everything inference of a v2 checkpoint does not import (v1/v3 models, the
sparse refiner, training, scripts, the web demo). `LICENSE` is upstream's: MIT
for MoGe, with the Apache-2.0 text that covers `model/modules/dinov2/`
(Meta's DINOv2).

Why vendored: `pip install moge` pulls gradio, trimesh and a CUDA extension
(FlexGEMM) that has to compile, none of which `python/depth.py` needs.

Files are byte-identical to upstream (LF) except three marked edits, each a
dropped import replaced by `None` with an `# AutoShade:` comment:

- `model/v2.py` — `utils3d` (only `MoGeModel.infer` used it) and
  `huggingface_hub.hf_hub_download` (depth.py loads the sha256-pinned
  checkpoint from disk).
- `utils/geometry_torch.py`, `utils/geometry_numpy.py` — `utils3d` (used only
  by helpers depth.py does not call).

To re-vendor at a new commit: fetch the same file list from
`raw.githubusercontent.com/microsoft/MoGe/<commit>/`, re-apply the three
edits, and re-run `python depth.py --self-test`.
