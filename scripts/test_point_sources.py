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


def one_star(y, x, fwhm=3.0, ratio=1.0, angle=0.0, r_shift=(0.0, 0.0), b_shift=(0.0, 0.0), r_g=0.5, b_g=0.6,
             extended=None):
    star = {"sample": np.zeros(1, np.int64), "y": np.array([y]), "x": np.array([x]), "snr": np.array([1.0]),
            "r_g": np.array([r_g]), "b_g": np.array([b_g]), "size": np.array([1.0]),
            "optics": np.array([[fwhm * S, fwhm / ratio * S, angle, 1.0, 1.0, *r_shift, *b_shift]], np.float32)}
    if extended is not None:
        minor, major, angle, share, offset = extended
        star["extended"] = np.array([[1.0, minor * S, major * S, angle, share, offset]], np.float32)
    return star


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

    # ---- the composite prior (v3) ----

    def test_composite_zero_draws_v2s_sky_and_no_extended_component(self):
        """`composite=0` must not touch the random stream: the sky v2 trained on is drawn unchanged."""
        a, b = ps.draw(np.random.default_rng(21), 64, 48, 48), ps.draw(np.random.default_rng(21), 64, 48, 48, composite=0.0)
        for key in a:
            self.assertTrue(np.array_equal(a[key], b[key]), key)
        self.assertEqual(float(np.abs(a["extended"]).max()), 0.0)

    def test_composite_one_gives_every_crop_an_extended_component_of_both_kinds(self):
        stars = ps.draw(np.random.default_rng(11), 200, 48, 48, composite=1.0)
        ext = stars["extended"]
        self.assertTrue(np.all(ext[:, 0] == 1.0))
        trails = ext[:, 5] > 0.0
        self.assertGreater(trails.sum(), 60)
        self.assertGreater((~trails).sum(), 60)
        self.assertTrue(np.all((ext[:, 4] >= 0.2) & (ext[:, 4] <= 0.7)), "the component carries 20-70 % of the peak")
        self.assertTrue(np.all(ext[:, 2] >= ext[:, 1]), "major at least minor")
        self.assertTrue(np.all(ext[:, 1] >= stars["optics"][:, 0]), "never narrower than its core")
        self.assertLessEqual(float(ext[:, 2].max()), ps.LONGEST_EXTENDED_FWHM * S + 1e-6)
        halos = ~trails
        self.assertTrue(np.all(ext[halos, 1] == ext[halos, 2]), "a halo is round")

    def test_a_composite_star_splits_its_peak_between_core_and_component(self):
        """At the core's own G1 sample a centred composite reads the full peak: (1 - share) from the core plus
        share from the component; the component alone carries the far wings."""
        y, x = 60.0, 61.0   # a G1 photosite: row even, col odd
        plain = ps.render(one_star(y, x, 1.8), torch.tensor([1.0]), 64, 64, "cpu")[0, 1].numpy()
        both = ps.render(one_star(y, x, 1.8, extended=(6.0, 6.0, 0.0, 0.4, 0.0)), torch.tensor([1.0]), 64, 64, "cpu")[0, 1].numpy()
        i, j = 30, 30
        self.assertAlmostEqual(float(both[i, j]), float(plain[i, j]), delta=0.02)
        # six samples (12 px) out along x the 1.8 px core is nothing; the 6 px halo at 40 % still shows
        self.assertLess(float(plain[i, j + 6]), 1e-6)
        self.assertGreater(float(both[i, j + 6]), 0.4 * math.exp(-0.5 * (12.0 / (6.0 * S)) ** 2) * 0.8)
        self.assertGreater(both.sum(), 2.0 * plain.sum(), "the component carries most of the flux")

    def test_a_trailed_composite_is_long_along_its_angle_with_the_core_off_centre(self):
        light = ps.render(one_star(60.0, 61.0, 1.8, extended=(3.0, 12.0, 0.0, 0.6, 3.0)), torch.tensor([1.0]), 64, 64, "cpu")[0, 1].numpy()
        h, w = light.shape
        i, j = np.meshgrid(np.arange(h), np.arange(w), indexing="ij")
        cy, cx = centroid(light, ps.CFA_OFFSETS[1])
        var_y = (((2 * i + 0 - cy) ** 2) * light).sum() / light.sum()
        var_x = (((2 * j + 1 - cx) ** 2) * light).sum() / light.sum()
        self.assertGreater(math.sqrt(var_x / var_y), 2.5, "long along x, its angle")
        # the trail's centre sits 3 px back along +x from the core at x = 61, so the light's centroid is left of it
        self.assertLess(cx, 61.0 - 1.0)
        self.assertGreater(cx, 61.0 - 3.0 - 0.5)

    # ---- the comet prior (v4) ----

    def test_comet_zero_draws_v2s_sky(self):
        a, b = ps.draw(np.random.default_rng(21), 64, 48, 48), ps.draw(np.random.default_rng(21), 64, 48, 48, comet=0.0)
        for key in a:
            self.assertTrue(np.array_equal(a[key], b[key]), key)

    def test_a_comet_is_a_narrow_round_core_at_the_head_of_a_flare(self):
        stars = ps.draw(np.random.default_rng(13), 200, 48, 48, comet=1.0)
        opt, ext = stars["optics"], stars["extended"]
        self.assertTrue(np.all(ext[:, 0] == 1.0))
        core = opt[:, 0] / S
        self.assertTrue(np.all((core >= 1.0) & (core <= 2.5)), "the core is 1.0-2.5 px FWHM")
        self.assertTrue(np.all(opt[:, 0] == opt[:, 1]), "and round")
        self.assertLess(float(np.median(core)), 1.8, "log-uniform: half of them under 1.6 px")
        minor, major = ext[:, 1] / S, ext[:, 2] / S
        self.assertTrue(np.all((minor >= 4.0) & (minor <= 9.0)))
        self.assertTrue(np.all((major >= minor - 1e-4) & (major <= ps.LONGEST_EXTENDED_FWHM + 1e-4)))
        self.assertGreater(int((major > 1.5 * minor).sum()), 40, "many flares are long")
        self.assertTrue(np.all(ext[:, 3] == opt[:, 2]), "the flare lies along the optics angle")
        self.assertTrue(np.all((ext[:, 4] >= 0.15) & (ext[:, 4] <= 0.6)))
        self.assertTrue(np.all((ext[:, 5] >= 0.0) & (ext[:, 5] <= 0.6 * major + 1e-4)),
                        "the core rides within 60 % of the flare's length ahead of its centre")
        self.assertGreater(int((ext[:, 5] > 0.3 * major).sum()), 60)

    def test_a_rendered_comet_peaks_at_its_core_with_the_flare_behind(self):
        """Core 1.4 px on a G1 photosite, flare 5 x 12 px along x with the core 6 px ahead of its centre:
        the plane's brightest sample is the core's and stands well over its 3x3 mean (the frame's own
        worst-kept stars read 2.5-4.2 there), and the light's centroid sits behind the core along -x."""
        y, x = 60.0, 61.0
        light = ps.render(one_star(y, x, 1.4, extended=(5.0, 12.0, 0.0, 0.4, 6.0)), torch.tensor([1.0]), 64, 64, "cpu")[0, 1].numpy()
        i, j = np.unravel_index(int(np.argmax(light)), light.shape)
        self.assertEqual((int(i), int(j)), (30, 30))
        self.assertGreater(float(light[30, 30] / light[29:32, 29:32].mean()), 2.5)
        cy, cx = centroid(light, ps.CFA_OFFSETS[1])
        self.assertLess(cx, x - 2.0)
        self.assertAlmostEqual(cy, y, delta=0.3)


if __name__ == "__main__":
    unittest.main()
