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
                 'mottle_colourfulness': [.1, .1], 'fine_colourfulness_magnitude': .1415,
                 'mottle_colourfulness_magnitude': .1415, 'glow_correlation': 1.0}
        star = {'faint_retention_percent': 99., 'peak_ratio': .95, 'fwhm_change': .2,
                'core_colour_change': [.01, .01]}
        gates = s.acceptance(noise, star, {}, noise, star, 'unreviewed')
        self.assertEqual(gates['8'], 'FAIL')
        self.assertEqual(set(s.acceptance(noise, star, {}, noise, star, 'clear').values()), {'PASS'})
        candidate = dict(noise, fine_y=.331, fine_y_p10_p90=[.25, .35])
        gates = s.acceptance(candidate, star, {}, noise, star, 'clear')
        self.assertEqual((gates['1'], gates['2']), ('FAIL', 'FAIL'))

    def test_colourfulness_gate_uses_vector_magnitude_not_separate_axes(self):
        noise = {'fine_y': .3, 'fine_y_p10_p90': [.29, .31],
                 'fine_colourfulness': [.2, 0], 'fine_colourfulness_magnitude': .2,
                 'mottle_colourfulness': [.2, 0], 'mottle_colourfulness_magnitude': .2,
                 'glow_correlation': 1.0}
        star = {'faint_retention_percent': 99., 'peak_ratio': .95, 'fwhm_change': .2,
                'core_colour_change': [.01, .01]}
        rotated = dict(noise, fine_colourfulness=[0, .2], mottle_colourfulness=[0, .2])
        gates = s.acceptance(rotated, star, {}, noise, star, 'clear')
        self.assertEqual((gates['3'], gates['4']), ('PASS', 'PASS'))
        larger = dict(rotated, fine_colourfulness_magnitude=.21, mottle_colourfulness_magnitude=.21)
        gates = s.acceptance(larger, star, {}, noise, star, 'clear')
        self.assertEqual((gates['3'], gates['4']), ('FAIL', 'FAIL'))

    def test_tone_map_is_monotone_and_preserves_linear_slope_including_tails(self):
        fit = {'x': [.1, .3, .7], 'y': [.21, .61, 1.41]}
        x = np.array([-.1, .1, .2, .5, 1.0])
        np.testing.assert_allclose(s.apply_tone(x, fit), 2*x+.01, atol=1e-12)
        np.testing.assert_allclose(s.apply_tone(x, fit, derivative=True), 2, atol=1e-12)
        y = s.monotone_knots(np.arange(4), np.array([1., 3., 2., 4.]), np.ones(4))
        np.testing.assert_allclose(y, [1, 2.5, 2.5, 4])

    def test_masked_bright_structure_cannot_change_the_tone_fit(self):
        y, x = np.mgrid[:640, :640]
        a = (.01+x*.0001+y*.00005).astype(np.float32)
        b = 1.5*a+.002
        mask = np.zeros(a.shape, bool); mask[200:320, 200:320] = True
        reference = s.fit_tone_map(a, b, mask, (0, 0))
        a[mask] = 100; b[mask] = -200
        observed = s.fit_tone_map(a, b, mask, (0, 0))
        np.testing.assert_allclose(observed['x'], reference['x'])
        np.testing.assert_allclose(observed['y'], reference['y'])
        np.testing.assert_allclose(s.apply_tone(np.array([.03, .07]), observed), [.047, .107], atol=1e-6)

    @staticmethod
    def _mosaic(stars=(), spikes=(), plane_gain=None):
        """Noise of sigma 20 on 2000, Gaussian stars (sigma 1.5 px) and lone photosites, as (x, y, height)."""
        rng = np.random.default_rng(5)
        m = 2000+rng.normal(0, 20, (512, 512))
        yy, xx = np.mgrid[:512, :512]
        for x, y, height in stars:
            psf = height*np.exp(-.5*((xx-x)**2+(yy-y)**2)/1.5**2)
            if plane_gain:
                (py, px), gain = plane_gain
                psf[py::2, px::2] *= gain
            m += psf
        for x, y, height in spikes:
            m[y, x] += height
        return np.rint(m).astype(np.uint16)

    def test_a_lone_photosite_is_not_a_star_in_the_quad_matched_mosaic(self):
        # Both would clear a 5-sigma render detector: the star's peak is 8 sigma, the lone photosite 12.
        m = self._mosaic(stars=[(200, 200, 160)], spikes=[(300, 300, 240)])
        snr = s.true_star_snr(s.quad_matched(m), np.array([[200, 200], [300, 300], [100, 400]]))
        self.assertGreaterEqual(snr[0], 5)
        self.assertLess(snr[1], 5)
        self.assertLess(snr[2], 5)      # empty sky
        # Too near the edge for its annulus: not judged, and never counted as a star.
        self.assertTrue(np.isnan(s.true_star_snr(s.quad_matched(m), np.array([[10, 10]]))[0]))

    def test_plane_flux_reads_the_plane_that_lost_a_tenth(self):
        sites = [(120, 120, 900), (260, 131, 1200), (390, 250, 700), (141, 380, 1000), (300, 401, 800)]
        xy = np.array([(x, y) for x, y, _ in sites])
        before = self._mosaic(stars=sites)
        after = self._mosaic(stars=sites, plane_gain=((1, 0), .9))
        r = s.plane_retention(s.plane_photometry(before, xy), s.plane_photometry(after, xy), 16383.)
        self.assertEqual(r['stars'], 5)
        np.testing.assert_allclose(r['flux'], [1, 1, .9, 1], atol=.005)
        self.assertAlmostEqual(r['flux_spread'], .1, delta=.005)
        # A star at the white level is left out of all four planes, and with none left the line is refused.
        with self.assertRaises(ValueError):
            s.plane_retention(s.plane_photometry(before, xy), s.plane_photometry(after, xy), 2000.)

    def test_the_cleaner_group_decides_and_a_missing_measurement_cannot_pass(self):
        rendered = {str(i): 'PASS' for i in range(1, 9)}
        rendered.update({'1': 'FAIL', '2': 'FAIL', '5': 'FAIL', '7': 'FAIL'})   # what the ordinary renders read
        lr = {'fine_y': .27, 'fine_y_p10_p90': [.265, .276]}
        flat = {'fine_y': .288, 'fine_y_p10_p90': [.284, .293]}
        true, lrtrue = {'faint_retention_percent': 97.3}, {'faint_retention_percent': 97.5}
        planes = {'flux_spread': .012}
        colour = {'NEW_to_Lightroom': .08, 'input_to_LR_OFF': .09}
        group = s.cleaner_group(rendered, flat, lr, true, lrtrue, planes, colour)
        self.assertEqual(set(group.values()), {'PASS'})
        self.assertEqual(list(group), ['1c', '2c', '3', '4', '5c', '6', '7c', '8'])
        missing = s.cleaner_group(rendered, None, lr, None, None, None, None)
        self.assertEqual([missing[k] for k in ('1c', '2c', '5c', '7c')], ['UNMEASURED']*4)
        self.assertEqual(s.cleaner_group(rendered, dict(flat, fine_y=.301), lr, true, lrtrue, planes, colour)['1c'], 'FAIL')
        self.assertEqual(s.cleaner_group(rendered, flat, lr, {'faint_retention_percent': 96.9}, lrtrue, planes, colour)['5c'], 'FAIL')
        self.assertEqual(s.cleaner_group(rendered, flat, lr, true, lrtrue, {'flux_spread': .021}, colour)['7c'], 'FAIL')
        drifted = {'NEW_to_Lightroom': .101, 'input_to_LR_OFF': .09}
        self.assertEqual(s.cleaner_group(rendered, flat, lr, true, lrtrue, planes, drifted)['7c'], 'FAIL')
        # A line the ordinary renders fail still fails the group when it is one of the four taken as they were.
        self.assertEqual(s.cleaner_group(dict(rendered, **{'6': 'FAIL'}), flat, lr, true, lrtrue, planes, colour)['6'], 'FAIL')

    def test_half_a_pair_is_refused_before_any_image_is_opened(self):
        with tempfile.TemporaryDirectory() as root:
            base = [sys.executable, '-B', str(SCRIPT), '--root', root, '--input', 'a.tif', '--ours', 'b.tif',
                    '--lr-off', 'c.tif', '--lr-on', 'd.tif']
            for half in (['--flat-input', 'e.tif'], ['--mosaic-ours', 'f.png']):
                p = subprocess.run(base+half, capture_output=True, text=True)
                self.assertEqual(p.returncode, 2, p.stderr)
                self.assertIn('given in pairs', p.stderr)


if __name__ == '__main__':
    unittest.main()
