"""Arithmetic and refusal tests for the precomputed-render denoise gate."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import numpy as np

SCRIPT = Path(__file__).with_name('denoise_star_standard.py')
SPEC = importlib.util.spec_from_file_location('standard', SCRIPT)
s = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(s)


class StarStandardTests(unittest.TestCase):
    def test_root_is_explicit(self):
        env = os.environ.copy()
        env.pop('AUTOSHADE_FIXTURES_ROOT', None)
        p = subprocess.run([sys.executable, '-B', str(SCRIPT)], env=env, capture_output=True, text=True)
        self.assertEqual(p.returncode, 2)
        self.assertIn('--root or AUTOSHADE_FIXTURES_ROOT', p.stderr)

    def test_masked_stars_cannot_enter_any_noise_statistic(self):
        rng = np.random.default_rng(17)
        rgb = .1+rng.normal(0, .003, (288, 288, 3)).astype(np.float32)
        mask = np.zeros((288, 288), bool)
        mask[90:120, 110:130] = True
        reference = s.tile_stats(rgb, mask, s.SRGB[1])
        rgb[mask] = [100, -100, 1000]
        np.testing.assert_allclose(s.tile_stats(rgb, mask, s.SRGB[1]), reference, rtol=1e-7, atol=1e-10)

    def test_minor_width_measures_the_core_not_the_trail(self):
        widths = []
        for theta in (0, np.pi/4, np.pi/2):
            along = s.DX*np.cos(theta)+s.DY*np.sin(theta)
            across = -s.DX*np.sin(theta)+s.DY*np.cos(theta)
            p = (.02+.3*np.exp(-.5*((along/4.5)**2+(across/1.7)**2))).astype(np.float32)
            width = s.minor_width(p, .02)
            self.assertAlmostEqual(width, 2.35482*1.7, delta=.3)
            widths.append(width)
        self.assertLess(max(widths)-min(widths), .25)

    def test_counterpart_matching_is_one_to_one(self):
        a = np.array([[10, 10], [11, 10], [40, 40]])
        b = np.array([[10, 10], [41, 40]])
        matches = s.match_sites(a, b)
        self.assertEqual(len(matches), 2)
        self.assertEqual(len(set(matches[:, 0])), 2)
        self.assertEqual(len(set(matches[:, 1])), 2)

    def test_rgb_spaces_compare_the_same_physical_core(self):
        rgb = np.array([.2, .3, .1])
        adobe = np.linalg.solve(s.ADOBE, s.SRGB@rgb)
        np.testing.assert_allclose(adobe@s.ADOBE_TO_SRGB.T, rgb, atol=1e-12)
        self.assertAlmostEqual(float(adobe@s.ADOBE[1]), float(rgb@s.SRGB[1]), places=12)

    def test_core_colour_change_has_common_units_across_rgb_primaries(self):
        before = np.array([.2, .3, .1])
        after = np.array([.19, .29, .11])
        changes = []
        for adobe in (False, True):
            cores = []
            for rgb in (before, after):
                obj = s.Image.__new__(s.Image)
                obj.adobe = adobe; obj.offset = np.array([0, 0])
                obj.w = (s.ADOBE if adobe else s.SRGB)[1]
                native = np.linalg.solve(s.ADOBE, s.SRGB@rgb) if adobe else rgb
                encoded = native**(256/563) if adobe else s.encode(native)
                obj.rgb = np.rint(encoded[None, None, :]*65535).astype(np.uint16)
                c = obj.core(np.array([[0, 0]]), True)[0]
                cores.append(np.array([c[0]-c[1], c[2]-c[1]])/c[3])
            changes.append(cores[1]-cores[0])
        np.testing.assert_allclose(changes[0], changes[1], atol=1e-4)

    def test_visual_review_is_required_and_numeric_misses_stay_red(self):
        noise = {'fine_y': .3, 'fine_y_p10_p90': [.29, .31], 'fine_colourfulness': [.1, .1],
                 'mottle_colourfulness': [.1, .1], 'glow_correlation': 1.0}
        star = {'faint_retention_percent': 99., 'peak_ratio': .95, 'fwhm_change': .2,
                'core_colour_change': [.01, .01]}
        gates = s.acceptance(noise, star, {}, noise, star, 'unreviewed')
        self.assertEqual(gates['8'], 'FAIL')
        self.assertEqual(set(s.acceptance(noise, star, {}, noise, star, 'clear').values()), {'PASS'})
        candidate = dict(noise, fine_y=.331, fine_y_p10_p90=[.25, .35])
        gates = s.acceptance(candidate, star, {}, noise, star, 'clear')
        self.assertEqual((gates['1'], gates['2']), ('FAIL', 'FAIL'))


if __name__ == '__main__':
    unittest.main()
