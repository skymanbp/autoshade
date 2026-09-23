#!/usr/bin/env python3
"""Self-test of what v2 added to `train_raw`: the mean-seeking loss and the flux validation.

Run directly: python scripts/test_train_raw.py (CPU, a few seconds; no data, no weights)."""
import math
import pathlib
import sys
import unittest

import numpy as np
import torch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import noise_synth as ns  # noqa: E402  because it is importable only after the sys.path insert above
import train_raw as tr  # noqa: E402  because it is importable only after the sys.path insert above


def batch(n=3, size=24, seed=0):
    g = torch.Generator().manual_seed(seed)
    clean = torch.rand((n, 4, size, size), generator=g) * 0.2 + 0.01
    a = torch.full((n, 4), 6e-4)
    b = torch.full((n, 4), 4e-6)
    noisy = clean + torch.randn((n, 4, size, size), generator=g) * torch.sqrt(6e-4 * clean + 4e-6)
    return noisy, clean, a, b


class MeanSeekingLoss(unittest.TestCase):
    def test_the_exact_answer_costs_nothing(self):
        noisy, clean, a, b = batch()
        _, tn, _, span = tr.make_batch(noisy, clean, a, b)
        loss = tr.loss_mean_seeking(tr.to_triplets(tn), span, noisy, clean, a, b)
        self.assertLess(float(loss), 1e-6)

    def test_it_is_a_squared_error_in_the_units_light_adds_in(self):
        # MUTATION THIS CATCHES: take the square in z and a given shortfall of light costs less on a star than on the
        # sky beside it (z is a square root); here the same shortfall in x costs the same wherever the level reads
        # the same, and four times as much when doubled.
        noisy, clean, a, b = batch()
        _, tn, _, span = tr.make_batch(noisy, clean, a, b)
        def cost(delta):
            _, t, _, _ = tr.make_batch(noisy, clean + delta, a, b)
            return float(tr.loss_mean_seeking(tr.to_triplets(t), span, noisy, clean, a, b))
        one, two = cost(2e-3), cost(4e-3)
        self.assertAlmostEqual(two / one, 4.0, delta=0.02)
        level = torch.nn.functional.avg_pool2d(torch.nn.functional.pad(tr.to_triplets(noisy), (4, 4, 4, 4), mode="reflect"),
                                               9, stride=1).clamp_min(0.0)
        want = float(((2e-3) ** 2 / (6e-4 * level + 4e-6 + 0.375 * 6e-4 ** 2)).mean())
        self.assertAlmostEqual(one / want, 1.0, delta=0.01)

    def test_the_clamp_never_touches_a_target(self):
        """`loss_mean_seeking` floors z at `Z_FLOOR` before inverting the transform. A target
        could only fall below it if `gat` could, and `gat` is smallest at x = 0."""
        floor_at = lambda r: float(2.0 * np.sqrt(0.375 + r))
        # `physical_model` admits b = 0 — the hardest case the measured half can produce.
        self.assertAlmostEqual(floor_at(0.0), 1.2247, places=4)
        # and the synthetic half never draws b/a^2 below 0.3 (`noise_synth.sample_params`).
        self.assertAlmostEqual(floor_at(0.3), 1.6432, places=4)
        # read off the transform the loss actually inverts, not retyped from it
        for a, b in ((4.9e-4, 0.0), (5e-5, 5e-5 ** 2 * 0.3), (6e-3, 6e-3 ** 2 * 60.0)):
            z0 = float(ns.gat(torch.zeros(1), torch.tensor(a), torch.tensor(b))[0])
            self.assertAlmostEqual(z0, floor_at(b / (a * a)), places=5)
            self.assertGreater(z0, tr.Z_FLOOR)

    def test_its_weight_never_reads_the_clean_side(self):
        # The minimiser of E[w (x_hat - x)^2 | y] is E[w x | y] / E[w | y]: the mean only if w is a function of y.
        noisy, clean, a, b = batch()
        _, tn, _, span = tr.make_batch(noisy, clean, a, b)
        out = tr.to_triplets(tn) * 1.01
        first = float(tr.loss_mean_seeking(out, span, noisy, clean, a, b))
        other = clean.clone()
        other[:, :, 8:12, 8:12] += 0.05
        x_hat = ns.igat((out * torch.cat([span, span]).view(-1, 1, 1, 1)).clamp_min(1.0),
                        torch.cat([a[:, [0, 1, 3]], a[:, [0, 2, 3]]], 0)[:, :, None, None],
                        torch.cat([b[:, [0, 1, 3]], b[:, [0, 2, 3]]], 0)[:, :, None, None])
        ratio = ((x_hat - tr.to_triplets(other)) ** 2).sum() / ((x_hat - tr.to_triplets(clean)) ** 2).sum()
        second = float(tr.loss_mean_seeking(out, span, noisy, other, a, b))
        # Were the weight flat the two ratios would be equal; it is not flat, but it is the SAME weight both times,
        # so the second loss is the first with only the errors changed.
        level = torch.nn.functional.avg_pool2d(torch.nn.functional.pad(tr.to_triplets(noisy), (4, 4, 4, 4), mode="reflect"),
                                               9, stride=1).clamp_min(0.0)
        w = 1.0 / (6e-4 * level + 4e-6 + 0.375 * 6e-4 ** 2)
        self.assertAlmostEqual(second, float((w * (x_hat - tr.to_triplets(other)) ** 2).mean()), delta=second * 1e-4)
        self.assertGreater(second, first)
        self.assertGreater(float(ratio), 1.0)

    def test_v4s_core_weight_is_finite_on_a_black_pixel_of_a_pair_without_read_noise(self):
        # 166 of 400 sampled real pairs carry b = 0 in a channel and half the crops hold clean pixels at 0
        # (2026-09-23): the sigma of a x + b alone is 0 there, and 0 / 0 froze four cloud runs
        noisy, clean, a, b = batch()
        b[0] = 0.0
        clean[0, :, :4, :] = 0.0
        light = torch.zeros_like(clean)
        w = tr.core_weight(light, clean, a, b, 6.0)
        self.assertTrue(bool(torch.isfinite(w).all()))
        self.assertEqual(float(w[0, :, :4, :].min()), 1.0)
        self.assertEqual(float(w[0, :, :4, :].max()), 1.0)
        # a star on such a pixel is measured against the transform's own floor, sqrt(3/8) a
        light[0, 1, 2, 2] = 10.0 * math.sqrt(0.375) * 6e-4
        w = tr.core_weight(light, clean, a, b, 6.0)
        self.assertAlmostEqual(float(w[0, 1, 2, 2]), 6.0, places=5)
        _, tn, _, span = tr.make_batch(noisy, clean, a, b)
        loss = tr.loss_mean_seeking(tr.to_triplets(tn) * 1.01, span, noisy, clean, a, b, w)
        self.assertTrue(bool(torch.isfinite(loss)))

    def test_v4s_core_weight_is_one_without_light_and_k_on_a_star(self):
        noisy, clean, a, b = batch()
        self.assertIsNone(tr.core_weight(torch.zeros_like(clean), clean, a, b, 1.0))
        light = torch.zeros_like(clean)
        floor = 0.375 * 6e-4 ** 2                 # the transform's own term, as in the loss's variance
        sigma = math.sqrt(6e-4 * float(clean[0, 1, 5, 5]) + 4e-6 + floor)
        light[0, 1, 5, 5] = 10.0 * sigma          # a 10-sigma core: the whole weight
        sigma6 = math.sqrt(6e-4 * float(clean[0, 1, 5, 6]) + 4e-6 + floor)
        light[0, 1, 5, 6] = 3.5 * sigma6          # half way up the ramp from 2 to 5 sigma
        sigma7 = math.sqrt(6e-4 * float(clean[0, 1, 5, 7]) + 4e-6 + floor)
        light[0, 1, 5, 7] = 2.0 * sigma7          # where the flux-truth lines call a star noise: no weight
        w = tr.core_weight(light, clean, a, b, 6.0)
        self.assertAlmostEqual(float(w[0, 1, 5, 5]), 6.0, places=5)
        self.assertAlmostEqual(float(w[0, 1, 5, 6]), 3.5, places=4)
        self.assertAlmostEqual(float(w[0, 1, 5, 7]), 1.0, places=5)
        self.assertEqual(float(w[1:].max()), 1.0)
        self.assertEqual(float(w[0, 0].max()), 1.0)
        # the loss with a flat weight is v2's; with this one it is the weighted mean of the same costs
        _, tn, _, span = tr.make_batch(noisy, clean, a, b)
        out = tr.to_triplets(tn) * 1.01
        plain = float(tr.loss_mean_seeking(out, span, noisy, clean, a, b))
        flat = float(tr.loss_mean_seeking(out, span, noisy, clean, a, b, torch.ones_like(clean)))
        self.assertAlmostEqual(flat, plain, delta=plain * 1e-5)
        weighted = float(tr.loss_mean_seeking(out, span, noisy, clean, a, b, w))
        wt = tr.to_triplets(w)
        level = torch.nn.functional.avg_pool2d(torch.nn.functional.pad(tr.to_triplets(noisy), (4, 4, 4, 4), mode="reflect"),
                                               9, stride=1).clamp_min(0.0)
        x_hat = ns.igat((out * torch.cat([span, span]).view(-1, 1, 1, 1)).clamp_min(1.0),
                        torch.cat([a[:, [0, 1, 3]], a[:, [0, 2, 3]]], 0)[:, :, None, None],
                        torch.cat([b[:, [0, 1, 3]], b[:, [0, 2, 3]]], 0)[:, :, None, None])
        per_pixel = (x_hat - tr.to_triplets(clean)) ** 2 / (6e-4 * level + 4e-6 + 0.375 * 6e-4 ** 2)
        self.assertAlmostEqual(weighted, float((per_pixel * wt).sum() / wt.sum()), delta=weighted * 1e-4)


class FluxValidation(unittest.TestCase):
    def setUp(self):
        clean = torch.full((4, 4, 96, 96), 0.0154)
        self.noisy, self.a, self.b, self.stars, self.light, self.ground = tr.flux_field(clean, "cpu")

    def test_the_field_is_the_same_every_time(self):
        again = tr.flux_field(torch.full((4, 4, 96, 96), 0.0154), "cpu")
        self.assertTrue(torch.equal(self.noisy, again[0]))
        self.assertGreater(len(self.stars["sample"]), 100)

    def test_all_of_the_light_none_of_it_and_half(self):
        for share in (1.0, 0.0, 0.5):
            got = tr.flux_returned(self.ground + share * self.light, self.stars, self.light, self.ground)
            for key, value in got.items():
                if key == "sky" or value is None:
                    continue
                self.assertAlmostEqual(value, share, delta=1e-4, msg=(share, key))
        self.assertAlmostEqual(tr.flux_returned(self.ground + self.light, self.stars, self.light, self.ground)["sky"], 0.0,
                               delta=1e-6)

    def test_it_is_read_on_the_plane_the_classes_are_defined_on(self):
        # The classes are G-plane peaks, so the flux is G1's: light kept in the greens alone reads as all of it.
        greens = torch.tensor([0.0, 1.0, 1.0, 0.0]).view(1, 4, 1, 1)
        got = tr.flux_returned(self.ground + self.light * greens, self.stars, self.light, self.ground)
        self.assertAlmostEqual(got["6-12"], 1.0, delta=1e-4)

    def test_the_noisy_input_itself_returns_the_light_it_was_given(self):
        # Photon noise is unbiased: before any cleaning, every class reads 1 within its own scatter.
        got = tr.flux_returned(self.noisy, self.stars, self.light, self.ground)
        self.assertAlmostEqual(got["25-400"], 1.0, delta=0.05)
        self.assertAlmostEqual(got["6-12"], 1.0, delta=0.25)


if __name__ == "__main__":
    unittest.main()
