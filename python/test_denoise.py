"""Contract tests for `denoise._download`, the one downloader all five
sidecars fetch through.

`_sidecar.fetch_model` imports `denoise._fetch_verified`, which calls this
function, so describe / embed / correspond / segment reach the network through
exactly these forty lines too. A mistake here is not one broken sidecar, it is
five — and one shipped for six releases: the short-download guard compared the
DECODED byte count against a Content-Length that counts WIRE bytes, so every
gzip-serving endpoint looked like a truncated transfer. `network_scunet.py`
(2908 gzipped, 11445 decoded) could not be fetched onto a cold cache at all.

Written the same way `test_sidecar.py` and `test_segment.py` are — plain
`unittest`, importing the module beside it, excluded from every installer by
the `test_*.py` rule.

Run: python -m unittest test_denoise -v   (from python/)
"""

import io
import os
import sys
import tempfile
import types
import unittest
from contextlib import redirect_stderr
from unittest import mock

import denoise

# The real numbers behind the regression, kept verbatim so the test says what
# the bug was: raw.githubusercontent.com serves the pinned SCUNet network file
# gzipped, and sends the COMPRESSED length in the header.
NETWORK_WIRE_BYTES = 2908
NETWORK_DECODED_BYTES = denoise.NETWORK_BYTES  # 11445
NETWORK_CAP = denoise.NETWORK_BYTES + 4096


class FakeResponse:
    def __init__(self, body, headers):
        self.body = body
        self.headers = headers

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False

    def raise_for_status(self):
        pass

    def iter_content(self, chunk_size=1):
        for i in range(0, len(self.body), chunk_size):
            yield self.body[i:i + chunk_size]


def fake_requests(body, headers, recorder=None):
    """A `requests` stand-in whose `get` records its kwargs and replays `body`.

    Injected through `sys.modules` rather than patched onto the real package:
    `_download` imports requests inside the function, and these tests are about
    what it asks the server for, not about having the package installed.
    """
    def get(url, **kwargs):
        if recorder is not None:
            recorder.append((url, kwargs))
        return FakeResponse(body, headers)

    module = types.ModuleType("requests")
    module.get = get
    return module


def download(body, headers, dest, max_bytes=NETWORK_CAP, recorder=None):
    with mock.patch.dict(sys.modules, {"requests": fake_requests(body, headers, recorder)}), \
            redirect_stderr(io.StringIO()):
        denoise._download("https://example.invalid/pinned.py", dest, max_bytes)


class DownloadTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.dest = os.path.join(self.dir.name, "network_scunet.py")

    def test_a_gzipped_response_is_not_a_short_download(self):
        # THE regression. Content-Length is the compressed size, iter_content
        # hands back the decompressed body; comparing them refused a download
        # that was byte-for-byte correct.
        download(b"x" * NETWORK_DECODED_BYTES,
                 {"Content-Length": str(NETWORK_WIRE_BYTES), "Content-Encoding": "gzip"},
                 self.dest)
        self.assertEqual(os.path.getsize(self.dest), NETWORK_DECODED_BYTES)

    def test_the_request_asks_for_no_content_encoding(self):
        # The fix is a request header, not a relaxed comparison: identity makes
        # Content-Length mean the same thing as what we write, which is what
        # keeps the guard below able to catch a real truncation.
        calls = []
        download(b"x" * 10, {"Content-Length": "10"}, self.dest, recorder=calls)
        self.assertEqual(calls[0][1]["headers"]["Accept-Encoding"], "identity")

    def test_a_real_short_download_is_still_refused(self):
        with self.assertRaises(SystemExit) as raised:
            download(b"x" * 5000, {"Content-Length": str(NETWORK_DECODED_BYTES)}, self.dest)
        self.assertIn("stopped at 5000 of 11445 bytes", str(raised.exception.code))
        self.assertFalse(os.path.exists(self.dest))

    def test_a_stream_past_the_pinned_size_is_still_refused(self):
        # The cap is in decoded bytes on both sides — the pinned tables count
        # the file, not the transfer — so `identity` leaves it exactly as it was.
        with self.assertRaises(SystemExit) as raised:
            download(b"x" * (NETWORK_CAP + 1), {}, self.dest)
        self.assertIn("exceeded its pinned size", str(raised.exception.code))
        self.assertFalse(os.path.exists(self.dest))

    def test_a_refused_download_leaves_no_part_file_behind(self):
        with self.assertRaises(SystemExit):
            download(b"x" * (NETWORK_CAP + 1), {}, self.dest)
        self.assertEqual(os.listdir(self.dir.name), [])

    def test_an_endpoint_that_sends_no_length_still_publishes(self):
        # huggingface.co gzips the pinned JSON files and sends no
        # Content-Length; that path must keep working, unchecked but not refused.
        download(b"x" * 4096, {"Content-Encoding": "gzip"}, self.dest)
        self.assertEqual(os.path.getsize(self.dest), 4096)


def _luma(x):
    wr, wg, wb = denoise._LUMA
    return wr * x[..., 0] + wg * x[..., 1] + wb * x[..., 2]


class BlendLawTests(unittest.TestCase):
    """`blend_luma_chroma` — the strength's ONE meaning (2026-09-13).

    Luminance follows the strength linearly; chroma comes from the model at
    min(1, 2*strength). The endpoints are the old blend's endpoints, so a
    strength of 0 or 1 changes nothing for anyone who used them.
    """

    def setUp(self):
        import numpy as np

        rng = np.random.default_rng(7)
        self.rgb = rng.random((4, 5, 3), dtype=np.float32) * 0.6 + 0.2
        self.den = rng.random((4, 5, 3), dtype=np.float32) * 0.6 + 0.2
        self.np = np

    def test_the_endpoints_are_the_input_and_the_model(self):
        np = self.np
        np.testing.assert_array_equal(denoise.blend_luma_chroma(self.den, self.rgb, 0.0), self.rgb)
        np.testing.assert_array_equal(denoise.blend_luma_chroma(self.den, self.rgb, 1.0), self.den)

    def test_luminance_follows_the_strength_linearly(self):
        np = self.np
        for s in (0.25, 0.5, 0.8):
            out = denoise.blend_luma_chroma(self.den, self.rgb, s)
            want = s * _luma(self.den) + (1 - s) * _luma(self.rgb)
            np.testing.assert_allclose(_luma(out), want, atol=1e-5)

    def test_chroma_is_the_models_from_half_strength_up(self):
        np = self.np
        for s in (0.5, 0.7, 0.99):
            out = denoise.blend_luma_chroma(self.den, self.rgb, s)
            # chroma differences equal the model's exactly: the luma moved,
            # the colour did not follow the input back
            np.testing.assert_allclose(out[..., 0] - _luma(out), self.den[..., 0] - _luma(self.den), atol=1e-5)
            np.testing.assert_allclose(out[..., 2] - _luma(out), self.den[..., 2] - _luma(self.den), atol=1e-5)

    def test_chroma_eases_in_twice_as_fast_as_luma_below_half(self):
        np = self.np
        s = 0.25
        out = denoise.blend_luma_chroma(self.den, self.rgb, s)
        c = 0.5
        for ch in (0, 2):
            want = c * (self.den[..., ch] - _luma(self.den)) + (1 - c) * (self.rgb[..., ch] - _luma(self.rgb))
            np.testing.assert_allclose(out[..., ch] - _luma(out), want, atol=1e-5)

    def test_a_flat_neutral_field_stays_neutral_at_every_strength(self):
        # A grey input and a grey model output must not pick up a colour
        # cast from the reconstruction of G' (the inverse is exact).
        np = self.np
        grey_in = np.full((3, 3, 3), 0.3, dtype=np.float32)
        grey_den = np.full((3, 3, 3), 0.31, dtype=np.float32)
        for s in (0.1, 0.5, 0.9):
            out = denoise.blend_luma_chroma(grey_den, grey_in, s)
            np.testing.assert_allclose(out[..., 0], out[..., 1], atol=1e-6)
            np.testing.assert_allclose(out[..., 1], out[..., 2], atol=1e-6)




# ── Where the bytes come from (2026-09-19) ──────────────────────────────────
#
# `_mirror` gives every pinned download a copy of ours to try before the host
# the pin names. These tests hold the two halves of that: ours is FIRST, and
# the digest — not the order — is what decides whether bytes are kept.

import hashlib

import _mirror

PINNED = b"the pinned bytes"
PINNED_SHA = hashlib.sha256(PINNED).hexdigest()
IMPOSTOR = b"the wrong bytes!"  # same length, so only the digest can tell
assert len(IMPOSTOR) == len(PINNED)

# A real pin, so the table is exercised the way the sidecar exercises it.
UPSTREAM = "https://github.com/cszn/KAIR/releases/download/v1.0/scunet_color_15.pth"
# A real address the table does not cover: v1's weights, published on the
# v1.5.0 release and never mirrored (v1.6.0's v2 replaced them in the pin).
UNMIRRORED = (
    "https://github.com/skymanbp/autoshade/releases/download/v1.5.0/"
    "autoshade-raw-denoise-v1.pth"
)


def fake_hosts(bodies, recorder):
    """A `requests` stand-in that answers per URL, in call order.

    A value may be bytes (every request to that URL gets them) or a list (one
    per request, popped in order). A URL that is absent raises the way an
    unreachable or 404 host does — which is the case the fallback exists for.
    """
    def get(url, **kwargs):
        recorder.append(url)
        body = bodies.get(url)
        if isinstance(body, list):
            body = body.pop(0) if body else None
        if body is None:
            raise RuntimeError(f"404 Not Found: {url}")
        return FakeResponse(body, {"Content-Length": str(len(body))})

    module = types.ModuleType("requests")
    module.get = get
    return module


class MirrorTableTests(unittest.TestCase):
    """The table covers what the sidecars actually fetch — every FILE, not
    just every repo. A pin that rewrites onto nothing is a model with one host
    again, which is the failure this module exists to prevent."""

    def test_every_pinned_model_file_rewrites_onto_a_copy_of_ours(self):
        import correspond
        import describe
        import embed
        import segment

        pinned = 0
        for model in (segment.BIREFNET, segment.SKY, segment.SAM,
                      embed.MODEL, describe.MODEL, correspond.MODEL):
            for name in model["files"]:
                url = (f"https://huggingface.co/{model['repo']}/resolve/"
                       f"{model['revision']}/{name}")
                ours = _mirror.mirror_of(url)
                self.assertIsNotNone(ours, f"no copy of ours for {url}")
                self.assertTrue(ours.startswith(
                    f"https://huggingface.co/{_mirror.MIRROR_OWNER}/"), ours)
                self.assertTrue(ours.endswith("/" + name), ours)
                pinned += 1
        self.assertGreaterEqual(pinned, 44, "extractor non-vacuity")

    def test_every_pinned_url_in_the_denoisers_rewrites_too(self):
        import denoise_raw

        urls = list(denoise.WEIGHT_URLS.values()) + [denoise.NETWORK_URL]
        urls += [pin["url"] for pin in denoise_raw.PINS.values()]
        # Five SCUNet weight sets, SCUNet's network file, DRUNet's and its
        # block library, and our own fine-tuned .pth — since v1.6.0 the one
        # file we publish ourselves has a second host of ours as well.
        self.assertEqual(len(urls), 9)
        for url in urls:
            ours = _mirror.mirror_of(url)
            self.assertIsNotNone(ours, f"no copy of ours for {url}")
            self.assertTrue(ours.endswith("/" + url.rsplit("/", 1)[-1]), ours)

    def test_our_own_release_asset_is_tried_from_our_copy_first(self):
        import denoise_raw

        url = denoise_raw.PINS["autoshade-raw-denoise-v2.pth"]["url"]
        ours = _mirror.mirror_of(url)
        self.assertTrue(ours.startswith(
            f"https://huggingface.co/{_mirror.MIRROR_OWNER}/"), ours)
        self.assertEqual(_mirror.sources(url), [ours, url, url])

    def test_a_url_with_no_entry_keeps_its_re_try(self):
        # No entry — and the source list is then exactly the two attempts
        # the fetch always made.
        self.assertIsNone(_mirror.mirror_of(UNMIRRORED))
        self.assertEqual(_mirror.sources(UNMIRRORED), [UNMIRRORED, UNMIRRORED])

    def test_a_mirrored_url_puts_ours_first_and_keeps_the_upstream_after_it(self):
        self.assertEqual(
            _mirror.sources(UPSTREAM),
            [_mirror.mirror_of(UPSTREAM), UPSTREAM, UPSTREAM],
        )


class FetchSourceOrderTests(unittest.TestCase):
    """`_fetch_verified` over that list. The pin is the authority throughout:
    the order only decides who is ASKED first."""

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.dest = os.path.join(self.dir.name, "scunet_color_15.pth")
        self.ours = _mirror.mirror_of(UPSTREAM)

    def fetch(self, bodies, url=UPSTREAM):
        asked = []
        with mock.patch.dict(sys.modules, {"requests": fake_hosts(bodies, asked)}), \
                redirect_stderr(io.StringIO()):
            denoise._fetch_verified(url, self.dest, PINNED_SHA,
                                    len(PINNED) + 4096, "the pinned test file")
        return asked

    def test_our_copy_is_the_only_host_asked_when_it_serves(self):
        asked = self.fetch({self.ours: PINNED, UPSTREAM: PINNED})
        self.assertEqual(asked, [self.ours], "the upstream must not be touched")
        with open(self.dest, "rb") as f:
            self.assertEqual(f.read(), PINNED)

    def test_an_unreachable_mirror_falls_through_to_the_upstream(self):
        asked = self.fetch({UPSTREAM: PINNED})
        self.assertEqual(asked, [self.ours, UPSTREAM])
        self.assertTrue(os.path.exists(self.dest))

    def test_a_mirror_serving_the_wrong_bytes_is_refused_not_trusted(self):
        # THE reason the order is safe: our copy is judged by the same digest
        # the upstream is, so being first buys it nothing.
        asked = self.fetch({self.ours: IMPOSTOR, UPSTREAM: PINNED})
        self.assertEqual(asked, [self.ours, UPSTREAM])
        with open(self.dest, "rb") as f:
            self.assertEqual(f.read(), PINNED)

    def test_an_unmirrored_url_still_gets_its_one_re_try(self):
        asked = self.fetch({UNMIRRORED: [IMPOSTOR, PINNED]}, url=UNMIRRORED)
        self.assertEqual(asked, [UNMIRRORED, UNMIRRORED])
        self.assertTrue(os.path.exists(self.dest))

    def test_when_no_source_serves_the_refusal_names_every_one_it_tried(self):
        with self.assertRaises(SystemExit) as raised:
            self.fetch({})
        message = str(raised.exception.code)
        self.assertIn("huggingface.co/Azng0", message)
        self.assertIn("github.com/cszn", message)
        self.assertFalse(os.path.exists(self.dest), "nothing unverified is left behind")

    def test_a_cache_that_matches_the_pin_asks_nobody(self):
        with open(self.dest, "wb") as f:
            f.write(PINNED)
        self.assertEqual(self.fetch({}), [])


if __name__ == "__main__":
    unittest.main()
