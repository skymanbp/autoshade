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


if __name__ == "__main__":
    unittest.main()
