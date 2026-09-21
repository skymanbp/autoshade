#!/usr/bin/env python3
"""Self-test of what v2 added to `train_raw`: the mean-seeking loss and the flux validation.

Run directly: python scripts/test_train_raw.py (CPU, a few seconds; no data, no weights)."""
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
