"""Contract tests for `denoise_raw.py`, the RAW-domain denoise sidecar.

Written the same way `test_denoise.py` and `test_sidecar.py` are — plain
`unittest`, importing the module beside it, excluded from every installer by
the `test_*.py` rule. Everything except the last case runs without torch and
without the weights; that case skips itself when the pinned weights are not
in the cache.

Run: python -m unittest test_denoise_raw -v   (from python/)
"""

import io
import os
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr

import numpy as np

import denoise_raw

PATTERNS = ("RGGB", "BGGR", "GRBG", "GBRG")


class TheCfaPacking(unittest.TestCase):
    def test_every_bayer_pattern_round_trips_through_the_planes(self):
        rng = np.random.default_rng(1)
        for pattern in PATTERNS:
            phases = denoise_raw.parse_pattern(pattern)
            mosaic = rng.integers(0, 16384, size=(66, 70), dtype=np.uint16)
            planes = denoise_raw.split_planes(mosaic, phases)
            self.assertEqual(sorted(planes), ["B", "G1", "G2", "R"], pattern)
            for p in planes.values():
                self.assertEqual(p.shape, (33, 35), pattern)
            back = denoise_raw.merge_planes(planes, phases, mosaic.shape, mosaic)
            self.assertTrue(np.array_equal(back, mosaic), pattern)

    def test_the_letters_land_on_their_phases(self):
        phases = denoise_raw.parse_pattern("GBRG")
        self.assertEqual(phases["G1"], (0, 0))
        self.assertEqual(phases["B"], (0, 1))
        self.assertEqual(phases["R"], (1, 0))
        self.assertEqual(phases["G2"], (1, 1))

    def test_an_odd_frame_keeps_its_last_row_and_column(self):
        rng = np.random.default_rng(2)
        phases = denoise_raw.parse_pattern("RGGB")
        mosaic = rng.integers(0, 16384, size=(65, 71), dtype=np.uint16)
        planes = denoise_raw.split_planes(mosaic, phases)
        zeroed = {n: np.zeros_like(p) for n, p in planes.items()}
        back = denoise_raw.merge_planes(zeroed, phases, mosaic.shape, mosaic)
        self.assertTrue(np.array_equal(back[64, :], mosaic[64, :]))
        self.assertTrue(np.array_equal(back[:, 70], mosaic[:, 70]))
        self.assertTrue((back[:64, :70] == 0).all())

    def test_a_non_bayer_pattern_is_refused_with_exit_two(self):
        for bad in ("RGGG", "XTRANS", "RGB", "", "RGBG"):
            with self.assertRaises(SystemExit) as cm, redirect_stderr(io.StringIO()):
                denoise_raw.parse_pattern(bad)
            self.assertEqual(cm.exception.code, 2, bad)


class TheVarianceStabilisation(unittest.TestCase):
    def test_the_inverse_is_unbiased_on_the_mean_of_noisy_transforms(self):
        """E[igat(gat(y))] ≈ x for y ~ N(x, a·x + b): the unbiased inverse maps
        the mean of the transformed samples back to the mean of the signal."""
        rng = np.random.default_rng(3)
        a, b = 2.0e-4, 1.5e-7
        for x in (0.005, 0.02, 0.1, 0.5):
            y = x + rng.normal(0.0, np.sqrt(a * x + b), size=400_000)
            z = denoise_raw.gat(y, a, b)
            back = denoise_raw.igat(z.mean(), a, b)
            self.assertAlmostEqual(back / x, 1.0, delta=0.01, msg=f"x={x}")

    def test_the_transform_leaves_unit_variance_noise(self):
        rng = np.random.default_rng(4)
        a, b = 2.0e-4, 1.5e-7
        for x in (0.02, 0.2, 0.7):
            y = x + rng.normal(0.0, np.sqrt(a * x + b), size=400_000)
            self.assertAlmostEqual(denoise_raw.gat(y, a, b).std(), 1.0, delta=0.03, msg=f"x={x}")


class TheNoiseModel(unittest.TestCase):
    def test_the_fit_recovers_an_injected_model_on_a_textured_plane(self):
        rng = np.random.default_rng(5)
        a, b = 2.0e-4, 1.5e-7
        yy, xx = np.mgrid[0:1024, 0:1024]
        # A smooth ramp (textureless) with a textured quarter that must be
        # excluded by the admission rule, not by luck.
        clean = 0.02 + 0.5 * (xx / 1024.0)
        clean[:512, :512] += 0.05 * rng.random((512, 512))
        noisy = clean + rng.normal(0.0, 1.0, clean.shape) * np.sqrt(a * clean + b)
        planes = {n: noisy.astype(np.float32) for n in denoise_raw.PLANES}
        with redirect_stderr(io.StringIO()):
            model = denoise_raw.noise_model(planes)
        for n in denoise_raw.PLANES:
            fa, fb = model[n]
            self.assertAlmostEqual(fa / a, 1.0, delta=0.05, msg=n)
            self.assertGreaterEqual(fb, 0.0, n)
            # b is the variance floor under a·x: on a ramp that starts at
            # x = 0.02 it is 4 % of the darkest block's variance, so the
            # estimator can only place it within a few percent of a·x there —
            # the assertion is that it is not wildly off, not that it is exact.
            self.assertLess(abs(fb - b), 0.02 * a, n)

    def test_it_reads_the_samples_that_sit_below_the_black_level(self):
        """A dark frame's noise straddles the black level — 14–22 % of a 15 s
        ISO-8000 frame is below it. Clamping those at 0 rectified the noise and
        inflated the fitted a by 3–19 % (2026-09-17), so the estimator has to
        take the negative samples as they come."""
        rng = np.random.default_rng(6)
        a, b = 1.5e-3, 4.9e-6
        clean = np.full((1024, 1024), 0.004, np.float32)     # a night sky, near black
        noisy = clean + rng.normal(0.0, 1.0, clean.shape) * np.sqrt(a * clean + b)
        self.assertGreater((noisy < 0).mean(), 0.1, "the fixture must straddle the black level")
        planes = {n: noisy.astype(np.float32) for n in denoise_raw.PLANES}
        with redirect_stderr(io.StringIO()):
            model = denoise_raw.noise_model(planes)
        rectified = {n: np.clip(noisy, 0.0, 1.0).astype(np.float32) for n in denoise_raw.PLANES}
        with redirect_stderr(io.StringIO()):
            clamped = denoise_raw.noise_model(rectified)
        for n in denoise_raw.PLANES:
            # At one signal level a and b trade off, so the variance the model
            # predicts THERE is what has to be right, not a alone.
            var_true = a * 0.004 + b
            var_fit = model[n][0] * 0.004 + model[n][1]
            var_clamped = clamped[n][0] * 0.004 + clamped[n][1]
            self.assertAlmostEqual(var_fit / var_true, 1.0, delta=0.1, msg=n)
            self.assertLess(var_clamped, 0.9 * var_true,
                            f"{n}: the rectified fixture must read LOW — that is the defect")

    def test_a_frame_with_no_flat_area_is_refused(self):
        # Structured texture everywhere (an 8-px stripe grid): its box-5
        # energy is far above its finest-scale Haar energy, so every block
        # fails the admission rule. (Per-pixel random texture would NOT — it
        # is white noise by construction, and the estimator must accept it.)
        yy, xx = np.mgrid[0:256, 0:256]
        stripes = 0.3 + 0.4 * (((xx // 4) + (yy // 4)) % 2)
        planes = {n: stripes.astype(np.float32) for n in denoise_raw.PLANES}
        with self.assertRaises(SystemExit) as cm, redirect_stderr(io.StringIO()):
            denoise_raw.noise_model(planes)
        self.assertEqual(cm.exception.code, 2)


class TheModelAffine(unittest.TestCase):
    """The map into the model's [0,1] has to carry every sample the sensor can
    hold. It was once built from the frame's own 0.05 / 99.95 percentiles,
    which put a hard ceiling at `igat(top)`: on a 15 s ISO-3200 star field that
    ceiling sat at 7.6–14.7 % of full scale per plane and every star came back
    as the same grey dot, keeping 9.7 % of its excess over the sky against
    Lightroom's 100 % (2026-09-17)."""

    MODELS = [
        (2.0e-4, 1.5e-7),   # the measured ISO-640 model of the camera under test
        (5.7e-4, 1.6e-6),   # its measured ISO-3200 model
        (1.5e-3, 4.9e-6),   # its measured ISO-8000 model
        (1.0e-5, 0.0),      # a near-noiseless frame; b = 0 is legal after the clamp
    ]

    def test_it_brackets_every_sample_a_plane_can_hold(self):
        for a, b in self.MODELS:
            ab = {n: (a, b) for n in denoise_raw.PLANES}
            lo, span = denoise_raw.model_affine(ab)
            for x in (0.0, 1e-6, 0.5, 1.0):
                mapped = (float(denoise_raw.gat(x, a, b)) - lo) / span
                self.assertGreaterEqual(mapped, 0.0, f"a={a} x={x}")
                self.assertLessEqual(mapped, 1.0, f"a={a} x={x} maps past the ceiling")

    def test_the_planes_share_one_map_wide_enough_for_the_widest(self):
        """Four planes of different gain still share one affine — the model
        takes one sigma — so it must span the WIDEST plane's range, not the
        first plane's."""
        gains = dict(zip(denoise_raw.PLANES, (5.7e-4, 6.2e-4, 6.2e-4, 3.7e-4)))
        ab = {n: (a, 2.0e-6) for n, a in gains.items()}
        lo, span = denoise_raw.model_affine(ab)
        for n, (a, b) in ab.items():
            self.assertLessEqual(float(denoise_raw.gat(1.0, a, b)), lo + span + 1e-9, n)

    def test_it_reads_no_pixels(self):
        """The signature is the guarantee: an affine that cannot see the frame
        cannot be dragged off it by a handful of bright outliers."""
        import inspect

        self.assertEqual(list(inspect.signature(denoise_raw.model_affine).parameters), ["ab"],
                         "model_affine grew a data argument")

    def test_the_operating_point_is_a_named_constant(self):
        self.assertEqual(denoise_raw.SIGMA_SCALE, 1.0)


class TheStrength(unittest.TestCase):
    def test_zero_reproduces_the_input_integers(self):
        rng = np.random.default_rng(7)
        black, white = 512.0, 16383.0
        v = rng.integers(0, 16384, size=(40, 40), dtype=np.uint16)
        x = (v.astype(np.float32) - black) / (white - black)
        den = np.clip(x + 0.01, 0, 1)
        out = denoise_raw.blend_and_quantise(x, den, 0.0, black, white)
        self.assertTrue(np.array_equal(out, v))

    def test_one_is_the_models_output_and_the_result_stays_inside_the_levels(self):
        black, white = 512.0, 16383.0
        x = np.full((8, 8), 0.5, np.float32)
        den = np.full((8, 8), 1.2, np.float32)  # over-range: the clip must hold
        out = denoise_raw.blend_and_quantise(x, den, 1.0, black, white)
        self.assertEqual(int(out.max()), 16383)
        half = denoise_raw.blend_and_quantise(x, np.full((8, 8), 0.0, np.float32), 0.5, black, white)
        self.assertEqual(int(half[0, 0]), round(0.25 * (white - black) + black))

    def test_dark_blended_noise_is_not_rectified_at_black(self):
        rng = np.random.default_rng(20260920)
        black, white, alpha = 512, 16383, 0.3
        noise = rng.normal(0, 100, (1024, 1024)).astype(np.float32)
        x = noise / (white - black)
        out = denoise_raw.blend_and_quantise(x, np.zeros_like(x), 1-alpha,
                                             black, white).astype(np.float32) - black
        self.assertGreater(float((out < 0).mean()), 0.48)
        self.assertAlmostEqual(float(out.mean()), float(alpha * noise.mean()), delta=0.01)
        self.assertAlmostEqual(float(out.var() / noise.var()), alpha**2, delta=0.0001)

    def test_the_sidecars_own_default_is_the_whole_clean_output(self):
        src = open(denoise_raw.__file__, encoding="utf-8").read()
        self.assertIn('ap.add_argument("--strength", type=float, default=1.0,', src)


class ThePins(unittest.TestCase):
    def test_every_download_has_a_digest_a_byte_count_and_a_pinned_source(self):
        self.assertEqual(
            sorted(denoise_raw.PINS),
            ["autoshade-raw-denoise-v1.pth", "basicblock.py", "network_unet.py"])
        for name, pin in denoise_raw.PINS.items():
            self.assertEqual(len(pin["sha256"]), 64, name)
            self.assertTrue(all(c in "0123456789abcdef" for c in pin["sha256"]), name)
            self.assertGreater(pin["bytes"], 0, name)
            self.assertTrue(pin["url"].startswith("https://"), name)
        for commit in (denoise_raw.NETWORK_COMMIT, denoise_raw.BASICBLOCK_COMMIT):
            self.assertEqual(len(commit), 40)
        self.assertIn(denoise_raw.NETWORK_COMMIT, denoise_raw.PINS["network_unet.py"]["url"])
        self.assertIn(denoise_raw.BASICBLOCK_COMMIT, denoise_raw.PINS["basicblock.py"]["url"])

    def test_the_network_is_built_without_bias_and_loaded_safely(self):
        src = open(denoise_raw.__file__, encoding="utf-8").read()
        self.assertIn("bias=False)", src)
        for line in src.splitlines():
            if "torch.load(" in line:
                self.assertIn("weights_only=True", line)


def _weights_present():
    cache = os.environ.get("AUTOSHADE_WEIGHTS_DIR") or os.path.join(
        os.path.dirname(os.path.abspath(denoise_raw.__file__)), "weights")
    return all(os.path.exists(os.path.join(cache, n)) for n in denoise_raw.PINS), cache


class EndToEnd(unittest.TestCase):
    @unittest.skipUnless(_weights_present()[0], "the pinned DRUNet files are not in the weight cache")
    def test_a_synthetic_mosaic_comes_back_cleaner_at_the_same_size(self):
        import cv2

        _, cache = _weights_present()
        rng = np.random.default_rng(8)
        black, white = 512.0, 16383.0
        a, b = 2.0e-4, 1.5e-7
        yy, xx = np.mgrid[0:512, 0:512]
        clean = 0.05 + 0.4 * (np.sin(xx / 23.0) * np.sin(yy / 31.0) * 0.5 + 0.5)
        noisy = clean + rng.normal(0.0, 1.0, clean.shape) * np.sqrt(a * clean + b)
        mosaic = np.clip(np.round(noisy * (white - black) + black), 0, white).astype(np.uint16)
        with tempfile.TemporaryDirectory() as d:
            src, dst = os.path.join(d, "in.png"), os.path.join(d, "out.png")
            cv2.imwrite(src, mosaic)
            r = subprocess.run(
                [sys.executable, "-E", denoise_raw.__file__, "--input", src, "--output", dst,
                 "--pattern", "RGGB", "--black", "512", "--white", "16383", "--strength", "1.0",
                 "--cache", cache],
                capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stderr[-2000:])
            out = cv2.imread(dst, cv2.IMREAD_UNCHANGED)
        self.assertEqual(out.dtype, np.uint16)
        self.assertEqual(out.shape, mosaic.shape)
        x_out = (out.astype(np.float32) - black) / (white - black)
        err_in = np.abs(noisy - clean).mean()
        err_out = np.abs(x_out - clean).mean()
        self.assertLess(err_out, 0.5 * err_in, f"in {err_in:.5f} out {err_out:.5f}")

    @unittest.skipUnless(_weights_present()[0], "the pinned DRUNet files are not in the weight cache")
    def test_bright_content_rarer_than_a_percentile_survives(self):
        """A star field in miniature: a dark noisy sky at the camera's measured
        ISO-3200 model with a few bright blocks covering 0.04 % of the frame —
        under the 0.05 % the old data-percentile affine cut at. Those blocks
        used to come back at the map's ceiling, `igat(top)`, all of them the
        same value; on the real 15 s ISO-3200 frame that was 38.8 % of the
        stars' brightness against Lightroom's 101 % (2026-09-17)."""
        import cv2

        _, cache = _weights_present()
        rng = np.random.default_rng(9)
        black, white = 512.0, 16383.0
        a, b = 5.7e-4, 1.6e-6
        sky, star = 0.010, 0.35
        clean = np.full((512, 512), sky, np.float32)
        bright = np.zeros(clean.shape, bool)
        for cy, cx in ((80, 120), (300, 64), (200, 400)):
            bright[cy : cy + 6, cx : cx + 6] = True          # 3 x 36 px = 0.041 %
        clean[bright] = star
        noisy = clean + rng.normal(0.0, 1.0, clean.shape).astype(np.float32) * np.sqrt(a * clean + b)
        mosaic = np.clip(np.round(noisy * (white - black) + black), 0, white).astype(np.uint16)
        with tempfile.TemporaryDirectory() as d:
            src, dst = os.path.join(d, "in.png"), os.path.join(d, "out.png")
            cv2.imwrite(src, mosaic)
            r = subprocess.run(
                [sys.executable, "-E", denoise_raw.__file__, "--input", src, "--output", dst,
                 "--pattern", "RGGB", "--black", "512", "--white", "16383", "--strength", "1.0",
                 "--cache", cache],
                capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stderr[-2000:])
            out = cv2.imread(dst, cv2.IMREAD_UNCHANGED)
        x_out = (out.astype(np.float32) - black) / (white - black)
        kept = x_out[bright].mean() / noisy[bright].mean()
        self.assertGreater(kept, 0.85, f"the bright blocks kept only {kept:.1%} of their brightness")
        # and the sky underneath still got denoised, so this is not just an
        # identity pass that happens to preserve highlights.
        sky_in = np.abs(noisy[~bright] - sky).mean()
        sky_out = np.abs(x_out[~bright] - sky).mean()
        self.assertLess(sky_out, 0.6 * sky_in, f"sky {sky_in:.5f} -> {sky_out:.5f}")

    @unittest.skipUnless(_weights_present()[0], "the pinned DRUNet files are not in the weight cache")
    def test_the_samples_below_the_black_level_reach_the_estimator(self):
        """`TheNoiseModel` pins that the ESTIMATOR must read the samples below
        the black level; this pins that `main` still HANDS THEM OVER. A night
        sky at the camera's measured ISO-8000 model sits under a stop above
        the black level, so a quarter of its samples fall below it. Rectifying
        those at 0 — which the normalisation did through v1.4.0 — reads the
        noise low, and the sidecar logs the model it fitted, so the fitted
        variance at the frame's own level is what reads it back."""
        import re

        import cv2

        _, cache = _weights_present()
        rng = np.random.default_rng(11)
        black, white = 512.0, 16383.0
        a, b = 1.5e-3, 4.9e-6
        sky = 0.002
        clean = np.full((512, 512), sky, np.float32)
        noisy = clean + rng.normal(0.0, 1.0, clean.shape).astype(np.float32) * np.sqrt(a * sky + b)
        mosaic = np.clip(np.round(noisy * (white - black) + black), 0, white).astype(np.uint16)
        self.assertGreater((mosaic < black).mean(), 0.15,
                           "the fixture must straddle the black level for this to measure anything")
        with tempfile.TemporaryDirectory() as d:
            src, dst = os.path.join(d, "in.png"), os.path.join(d, "out.png")
            cv2.imwrite(src, mosaic)
            r = subprocess.run(
                [sys.executable, "-E", denoise_raw.__file__, "--input", src, "--output", dst,
                 "--pattern", "RGGB", "--black", "512", "--white", "16383", "--strength", "1.0",
                 "--cache", cache],
                capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr[-2000:])
        # Both spellings of the per-plane line — its own fit, or the pooled one.
        fitted = re.findall(r"plane (\w+):.*?a=([-\d.eE+]+) b=([-\d.eE+]+)", r.stderr)
        self.assertEqual(len(fitted), 4, r.stderr[-2000:])
        var_true = a * sky + b
        for name, fa, fb in fitted:
            var_fit = float(fa) * sky + float(fb)
            self.assertAlmostEqual(var_fit / var_true, 1.0, delta=0.15,
                                   msg=f"{name}: fitted {var_fit:.3e} against {var_true:.3e}")


if __name__ == "__main__":
    unittest.main()
