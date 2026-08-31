"""Contract tests for `_sidecar.py`, the plumbing describe/embed/correspond
share.

Written the same way `test_segment.py` is — plain `unittest`, importing the
module beside it, excluded from both installers by the `test_*.py` rule — and
for the same reason it matters more here: three scripts used to each own a copy
of these five helpers, so a mistake could only ever break one of them. Now one
mistake breaks all three, and the only thing standing between a rewrite and
three broken sidecars is this file.

Run: python -m unittest test_sidecar -v   (from python/)
"""

import io
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr
from unittest import mock

import _sidecar

MODEL_FLAT = {
    "repo": "vendor/model-name",
    "revision": "0123456789abcdef0123456789abcdef01234567",
    "files": {
        "model.safetensors": {"sha256": "aa" * 32, "bytes": 1000},
        "config.json": {"sha256": "bb" * 32, "bytes": 20},
    },
}

MODEL_NESTED = {
    "repo": "vendor/diffusion",
    "revision": "fedcba9876543210fedcba9876543210fedcba98",
    "files": {
        "unet/config.json": {"sha256": "cc" * 32, "bytes": 30},
        "vae/weights.safetensors": {"sha256": "dd" * 32, "bytes": 4000},
    },
}


class LogTests(unittest.TestCase):
    def test_log_writes_one_tagged_line_to_stderr(self):
        buf = io.StringIO()
        with redirect_stderr(buf):
            _sidecar.log("embed", "staging 3 frames")
        self.assertEqual(buf.getvalue(), "[embed] staging 3 frames\n")

    def test_log_flushes_so_a_running_sidecar_is_readable(self):
        # The Rust side tails these while the child is still alive; a buffered
        # line reports a hang that is not happening.
        flushed = []

        class Recorder(io.StringIO):
            def flush(self):
                flushed.append(True)

        with redirect_stderr(Recorder()):
            _sidecar.log("describe", "loading")
        self.assertTrue(flushed, "log() must flush, not buffer")


class DieTests(unittest.TestCase):
    def test_die_names_the_script_and_exits_two(self):
        buf = io.StringIO()
        with self.assertRaises(SystemExit) as raised, redirect_stderr(buf):
            _sidecar.die("correspond", "no frames were readable")
        # Two, not one: the Rust side already treats "exited 0 and wrote
        # nothing" as its own failure, so a refusal must stay distinguishable
        # from a crash.
        self.assertEqual(raised.exception.code, 2)
        self.assertEqual(buf.getvalue(), "correspond.py: no frames were readable\n")


class ModelDirTests(unittest.TestCase):
    def test_one_directory_per_pinned_revision(self):
        d = _sidecar.model_dir(MODEL_FLAT, "/cache")
        self.assertEqual(os.path.basename(d), "vendor--model-name@0123456789ab")

    def test_a_repin_does_not_reuse_the_old_revisions_cache(self):
        other = dict(MODEL_FLAT, revision="ffffffffffff" + "0" * 28)
        self.assertNotEqual(
            _sidecar.model_dir(MODEL_FLAT, "/cache"),
            _sidecar.model_dir(other, "/cache"),
        )


class FetchModelTests(unittest.TestCase):
    def fetch(self, model, cache="/cache"):
        """Run fetch_model with the download and the mkdir recorded, not done."""
        calls, made = [], []
        with mock.patch.object(_sidecar, "_fetch_verified",
                               lambda *a: calls.append(a)), \
                mock.patch.object(os, "makedirs",
                                  lambda p, exist_ok=False: made.append(p)):
            out = _sidecar.fetch_model(model, cache, "Test Model")
        return out, calls, [p.replace("\\", "/") for p in made]

    def test_makes_the_directory_each_file_goes_in(self):
        # correspond.py's shape, which is the strict superset: a nested pin
        # needs its subdirectory made, not just the model root.
        out, _, made = self.fetch(MODEL_NESTED)
        root = out.replace("\\", "/")
        self.assertIn(root + "/unet", made)
        self.assertIn(root + "/vae", made)

    def test_makes_nothing_outside_the_model_directory(self):
        out, _, made = self.fetch(MODEL_NESTED)
        root = out.replace("\\", "/")
        for p in made:
            self.assertTrue(p == root or p.startswith(root + "/"), p)

    def test_the_model_directory_exists_whatever_the_pin_held(self):
        # The postcondition must not depend on `files` being non-empty.
        out, _, made = self.fetch(dict(MODEL_FLAT, files={}))
        self.assertIn(out.replace("\\", "/"), made)

    def test_passes_each_files_own_digest_and_a_small_cap_slack(self):
        _, calls, _ = self.fetch(MODEL_FLAT)
        by_name = {os.path.basename(dest): (sha, cap, what)
                   for _url, dest, sha, cap, what in calls}
        self.assertEqual(by_name["config.json"][0], "bb" * 32)
        # The same slack denoise.py leaves: an overshoot message should be
        # about the endpoint, not an off-by-one.
        self.assertEqual(by_name["config.json"][1], 20 + 4096)
        self.assertEqual(by_name["model.safetensors"][1], 1000 + 4096)

    def test_the_cap_message_names_the_model_family_and_the_file(self):
        _, calls, _ = self.fetch(MODEL_FLAT)
        whats = {c[4] for c in calls}
        self.assertIn("the Test Model 'config.json'", whats)

    def test_the_url_is_the_pinned_revision_not_a_branch(self):
        _, calls, _ = self.fetch(MODEL_FLAT)
        for url, *_rest in calls:
            self.assertIn("/resolve/" + MODEL_FLAT["revision"] + "/", url)
            self.assertNotIn("/main/", url)


class PublishTests(unittest.TestCase):
    def test_writes_the_payload_and_leaves_no_temp_behind(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "out.json")
            _sidecar.publish(path, '{"vec":[0.5,-0.25]}\n')
            with open(path, encoding="utf-8") as f:
                self.assertEqual(f.read(), '{"vec":[0.5,-0.25]}\n')
            self.assertEqual(os.listdir(d), ["out.json"])

    def test_a_failed_write_does_not_leave_a_half_file_at_the_path(self):
        # The caller stages this file and a build or a recipe reads it, so a
        # partial payload at the real path is worse than no payload.
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "out.json")
            with open(path, "w", encoding="utf-8") as f:
                f.write("previous good payload")
            with mock.patch.object(os, "fsync", side_effect=OSError("disk")):
                with self.assertRaises(OSError):
                    _sidecar.publish(path, "new payload")
            with open(path, encoding="utf-8") as f:
                self.assertEqual(f.read(), "previous good payload")
            self.assertEqual(os.listdir(d), ["out.json"])


class BindingTests(unittest.TestCase):
    """The three sidecars bind this module rather than copying it.

    That fold moved the only thing that still differs per script — WHICH model
    and WHICH name to say — into four one-line delegates each. A crossed wire
    there is invisible in review (`_sidecar.log('embed', msg)` inside
    describe.py reads fine) and shows up only as a progress line attributed to
    the wrong sidecar, or worse, a cache directory pinned to another model's
    revision. So the wiring itself is pinned.
    """

    SCRIPTS = (("describe", "Qwen"), ("embed", "siglip"), ("correspond", "stable-diffusion"))

    def test_each_script_logs_and_dies_under_its_own_name(self):
        for name, _repo in self.SCRIPTS:
            mod = __import__(name)
            buf = io.StringIO()
            with redirect_stderr(buf):
                mod.log("working")
            self.assertEqual(buf.getvalue(), f"[{name}] working\n")
            buf = io.StringIO()
            with self.assertRaises(SystemExit) as raised, redirect_stderr(buf):
                mod.die("refused")
            self.assertEqual(raised.exception.code, 2)
            self.assertEqual(buf.getvalue(), f"{name}.py: refused\n")

    def test_each_script_resolves_its_own_pinned_model(self):
        for name, repo_fragment in self.SCRIPTS:
            mod = __import__(name)
            d = mod.model_dir("/cache").replace("\\", "/")
            self.assertIn(repo_fragment.lower(), d.lower(),
                          f"{name}.py resolved {d}, not its own model")


if __name__ == "__main__":
    unittest.main()
