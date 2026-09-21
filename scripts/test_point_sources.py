#!/usr/bin/env python3
"""Self-test of `point_sources`: what makes a star a star is what these pin.

Run directly: python scripts/test_point_sources.py (CPU, a few seconds)."""
import math
import pathlib
import sys
import unittest

import numpy as np
import torch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import point_sources as ps  # noqa: E402  why: importable only after the sys.path insert

S = ps.FWHM_TO_SIGMA


def one_star(y, x, fwhm=3.0, ratio=1.0, angle=0.0, r_shift=(0.0, 0.0), b_shift=(0.0, 0.0), r_g=0.5, b_g=0.6):
    return {"sample": np.zeros(1, np.int64), "y": np.array([y]), "x": np.array([x]), "snr": np.array([1.0]),
            "r_g": np.array([r_g]), "b_g": np.array([b_g]), "size": np.array([1.0]),
            "optics": np.array([[fwhm * S, fwhm / ratio * S, angle, 1.0, 1.0, *r_shift, *b_shift]], np.float32)}


def centroid(plane, offset):
    """Intensity-weighted centre of one plane, in mosaic pixels."""
    h, w = plane.shape
    i, j = np.meshgrid(np.arange(h), np.arange(w), indexing="ij")
    total = plane.sum()
    return ((2 * i + offset[0]) * plane).sum() / total, ((2 * j + offset[1]) * plane).sum() / total


class PointSources(unittest.TestCase):
    def test_a_star_is_in_all_four_planes_at_one_place(self):
        # MUTATION THIS CATCHES: sample every plane at the R photosites (drop the CFA offsets) and the G1, G2 and B
        # centroids move by up to a pixel.
        for y, x in ((40.3, 51.7), (41.0, 50.0), (39.6, 52.45)):
            light = ps.render(one_star(y, x), torch.tensor([1.0]), 48, 48, "cpu")[0].numpy()
            for plane, offset in enumerate(ps.CFA_OFFSETS):
                cy, cx = centroid(light[plane], offset)
                self.assertLess(math.hypot(cy - y, cx - x), 0.12, (plane, cy, cx))

    def test_each_plane_holds_a_quarter_of_the_light(self):
        # The sum over one plane's samples, times the four mosaic pixels each stands for, is the integral of the
        # star: amplitude * 2 pi sqrt(det(covariance + I/12)). Measured over four sub-pixel positions: within 0.65 %
        # from 2.6 px FWHM up (a 1.5 px star lands mostly on one photosite, and a plane then holds 67-139 % of its
        # quarter -- which is the sensor's doing, not an error). MUTATION THIS CATCHES: drop the photosite's own
        # aperture from one axis and the 2.6 px row reads 3 % high.
        for fwhm, ratio in ((2.6, 1.0), (3.0, 0.5), (5.0, 0.8)):
            light = ps.render(one_star(60.2, 61.4, fwhm, ratio, 0.6), torch.tensor([1.0]), 64, 64, "cpu")[0].numpy()
            minor, major = (fwhm * S) ** 2 + 1 / 12, (fwhm / ratio * S) ** 2 + 1 / 12
            whole = 2 * math.pi * math.sqrt(minor * major)
            for plane, colour in enumerate((0.5, 1.0, 1.0, 0.6)):
                self.assertAlmostEqual(4 * light[plane].sum() / (whole * colour), 1.0, delta=0.015, msg=(fwhm, plane))

    def test_lateral_ca_moves_r_and_b_and_leaves_g(self):
        still = ps.render(one_star(40.0, 40.0), torch.tensor([1.0]), 48, 48, "cpu")[0].numpy()
        moved = ps.render(one_star(40.0, 40.0, r_shift=(0.6, -0.4), b_shift=(-0.5, 0.7)), torch.tensor([1.0]),
                          48, 48, "cpu")[0].numpy()
        for plane, want in ((0, (0.6, -0.4)), (1, (0.0, 0.0)), (2, (0.0, 0.0)), (3, (-0.5, 0.7))):
            a, b = centroid(still[plane], ps.CFA_OFFSETS[plane]), centroid(moved[plane], ps.CFA_OFFSETS[plane])
            self.assertAlmostEqual(b[0] - a[0], want[0], delta=0.08)
            self.assertAlmostEqual(b[1] - a[1], want[1], delta=0.08)

    def test_a_trail_is_long_along_its_angle(self):
        light = ps.render(one_star(60.0, 60.0, 2.4, 0.4, 0.0), torch.tensor([1.0]), 64, 64, "cpu")[0, 1].numpy()
        h, w = light.shape
        i, j = np.meshgrid(np.arange(h), np.arange(w), indexing="ij")
        cy, cx = centroid(light, ps.CFA_OFFSETS[1])
        var_y = (((2 * i + 0 - cy) ** 2) * light).sum() / light.sum()
        var_x = (((2 * j + 1 - cx) ** 2) * light).sum() / light.sum()
        # angle 0 puts the major axis along x: 6 px FWHM against 2.4
        self.assertAlmostEqual(math.sqrt(var_x / var_y), 2.5, delta=0.35)

    def test_the_peak_is_so_many_sigmas_where_the_star_stands(self):
        clean = torch.full((6, 4, 64, 64), 0.02)
        a = torch.full((6, 4), 5e-4)
        b = torch.full((6, 4), 6e-6)
        same = ps.draw(np.random.default_rng(7), 6, 64, 64, share=1.0)
        lit, noisy, light = ps.add_to_pair(clean, None, a, b, np.random.default_rng(7), None, share=1.0)
        self.assertIsNone(noisy)
        sigma = math.sqrt(5e-4 * 0.02 + 6e-6)
        want = ps.render(same, torch.as_tensor(same["snr"] * sigma).float(), 64, 64, "cpu")
        self.assertTrue(torch.allclose(light, want, rtol=1e-5, atol=1e-9))
        self.assertTrue(torch.allclose(lit, (clean + want).clamp(max=1.0)))
        self.assertGreater(len(same["sample"]), 6)

    def test_half_the_crops_carry_none(self):
        stars = ps.draw(np.random.default_rng(3), 2000, 32, 32)
        self.assertAlmostEqual(len(np.unique(stars["sample"])) / 2000, 0.5, delta=0.04)
        self.assertLessEqual(np.bincount(stars["sample"]).max(), 3000)

    def test_a_field_holds_many_more_faint_stars_than_bright_ones(self):
        stars = ps.draw(np.random.default_rng(5), 400, 96, 96, share=1.0, field_share=1.0)
        snr = stars["snr"]
        self.assertGreater((snr < 3).sum(), 3 * (snr > 10).sum())
        self.assertGreater((snr > 10).sum(), 100)
        self.assertLessEqual(snr.max(), 400.0)

    def test_a_real_frame_gets_the_stars_own_photons(self):
        clean = torch.full((4, 4, 64, 64), 0.05)
        noisy = clean.clone()
        a = torch.full((4, 4), 8e-4)
        b = torch.full((4, 4), 1e-6)
        gen = torch.Generator().manual_seed(11)
        lit, seen, light = ps.add_to_pair(clean, noisy, a, b, np.random.default_rng(9), gen, share=1.0)
        added = (seen - noisy).double()
        total = float(light.double().sum())
        # photon counts: unbiased, variance a * light, and whole multiples of one photon
        self.assertLess(abs(float(added.sum()) - total), 5 * math.sqrt(8e-4 * total))
        self.assertTrue(torch.allclose(added / 8e-4, torch.round(added / 8e-4), atol=1e-3))
        self.assertTrue(torch.equal(lit, (clean + light).clamp(max=1.0)))

    def test_nothing_passes_the_sensor_ceiling(self):
        clean = torch.full((3, 4, 48, 48), 0.999)
        a = torch.full((3, 4), 2e-3)
        b = torch.full((3, 4), 1e-5)
        gen = torch.Generator().manual_seed(2)
        lit, seen, _ = ps.add_to_pair(clean, clean.clone(), a, b, np.random.default_rng(1), gen, share=1.0)
        self.assertLessEqual(float(lit.max()), 1.0)
        self.assertLessEqual(float(seen.max()), 1.0)

    def test_the_same_seed_draws_the_same_sky(self):
        a, b = ps.draw(np.random.default_rng(21), 16, 48, 48), ps.draw(np.random.default_rng(21), 16, 48, 48)
        for key in a:
            self.assertTrue(np.array_equal(a[key], b[key]), key)


if __name__ == "__main__":
    unittest.main()
