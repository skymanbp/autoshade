"""scripts/ci_scope.py: when the online runners may set the long tests aside (v1.18.2).

Run: python -m unittest discover -s scripts -p 'test_*.py'
"""
import os
import sys
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ci_scope  # noqa: E402 — because the import needs the path line above it

REPO = Path(__file__).resolve().parent.parent
TOML = '[package]\nname = "autoshade"\nversion = "1.18.1"\nedition = "2024"\n\n[dependencies]\nrayon = "1.10"\n'
LOCK = ('version = 4\n\n[[package]]\nname = "autocfg"\nversion = "1.5.0"\n\n'
        '[[package]]\nname = "autoshade"\nversion = "1.18.1"\ndependencies = [\n "rayon",\n]\n')


class LongTests(unittest.TestCase):
    def test_the_long_tests_are_the_named_modules_and_nothing_beside_them(self):
        for name in ("fit::tests::a_fit", "fit::joint::tests::a_solve", "fit_zoned::tests::zones",
                     "fit_zoned::range::tests::a_run", "denoise::linear_chroma::tests::grain"):
            self.assertTrue(ci_scope.is_long(name), name)
        for name in ("fit_field::tests::a_field", "refit::tests::x", "pipeline::tests::fit::x", "fit",
                     "denoise::tests::a_denoise", "denoise::linear_chroma_x::tests::y", "xmp::tests::fit::tests::z"):
            self.assertFalse(ci_scope.is_long(name), name)

    def test_the_module_whose_test_reads_a_gui_source_always_runs(self):
        self.assertFalse(ci_scope.is_long("fit_zoned::semantic::tests::semantic_limits_have_one_definition"))
        self.assertTrue(ci_scope.is_long("fit_zoned::semantics::tests::x"))  # a prefix of a name is not the module

    def test_no_long_test_source_reads_a_neutral_path(self):
        """The rule the skip rests on, read off the tree: were a long module to start reading a GUI
        source or a document, the scope would run the whole suite for ever — and this says where."""
        sources = ci_scope.long_sources(REPO)
        self.assertIn("src/fit.rs", sources)
        self.assertIn("src/fit_zoned.rs", sources)
        self.assertTrue(any(s.startswith("src/denoise/linear_chroma") for s in sources), sources)
        self.assertNotIn("src/fit_zoned/semantic.rs", sources)
        hits = []
        for f in sources:
            hits += ci_scope.neutral_reads(f, (REPO / f).read_text(encoding="utf-8"))
        self.assertEqual(hits, [])

    def test_a_read_of_a_neutral_path_is_found_in_each_spelling(self):
        reads = ci_scope.neutral_reads
        self.assertEqual(len(reads("src/fit_zoned/x.rs", 'let s = include_str!("../bin/gui/actions.rs");')), 1)
        self.assertEqual(len(reads("src/fit.rs", 'include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/USER_MANUAL.md"))')), 1)
        self.assertEqual(len(reads("src/fit/tests.rs", 'root.join("docs/USER_MANUAL.md")')), 1)
        self.assertEqual(len(reads("src/fit/tests.rs", 'root.join("site/index.html")')), 1)
        # what a long test MAY read: the library's own sources, the sidecars, the figures
        self.assertEqual(reads("src/fit_zoned.rs", 'include_str!("fit_zoned/accept.rs")'), [])
        self.assertEqual(reads("src/fit/tests.rs", 'root.join("docs/images")'), [])
        self.assertEqual(reads("src/fit_zoned/x.rs", 'include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/python/segment.py"))'), [])


class Paths(unittest.TestCase):
    def test_neutral_is_the_window_the_documents_and_the_machinery(self):
        for p in ("src/bin/gui/app.rs", "src/bin/gui/tests/top_bar.rs", "docs/USER_MANUAL.md", "docs/RELEASE_NOTES_v1.18.2.md",
                  "site/index.html", ".github/workflows/build.yml", "scripts/ci_scope.py", "installer/autoshade.iss",
                  "README.md", "CONTRIBUTING.md", "LICENSE"):
            self.assertTrue(ci_scope.neutral(p), p)

    def test_everything_a_long_test_can_read_is_not_neutral(self):
        for p in ("src/fit.rs", "src/recipe.rs", "src/main.rs", "src/bin/other.rs", "src/fixtures/default-recipe.json",
                  "docs/images/showcase.jpg", "python/segment.py", "assets/icon_256.png", "build.rs", "tests/contract.rs",
                  "rust-toolchain.toml", ".gitattributes", "Cargo.toml", "Cargo.lock", "docs", "README.md.bak"):
            self.assertFalse(ci_scope.neutral(p), p)


class Versions(unittest.TestCase):
    def test_a_version_line_alone_is_no_change(self):
        self.assertTrue(ci_scope.same_but_version("Cargo.toml", TOML, TOML.replace("1.18.1", "1.18.2")))
        self.assertTrue(ci_scope.same_but_version("Cargo.lock", LOCK, LOCK.replace('"autoshade"\nversion = "1.18.1"', '"autoshade"\nversion = "1.18.2"')))
        self.assertTrue(ci_scope.same_but_version("Cargo.toml", TOML.replace("\n", "\r\n"), TOML.replace("1.18.1", "1.18.2")))

    def test_anything_beside_it_is_a_change(self):
        self.assertFalse(ci_scope.same_but_version("Cargo.toml", TOML, TOML.replace('rayon = "1.10"', 'rayon = "1.11"')))
        self.assertFalse(ci_scope.same_but_version("Cargo.toml", TOML, TOML.replace("2024", "2021")))
        self.assertFalse(ci_scope.same_but_version("Cargo.lock", LOCK, LOCK.replace("1.5.0", "1.5.1")))
        self.assertFalse(ci_scope.same_but_version("Cargo.lock", LOCK, LOCK.replace(' "rayon",\n', ' "rayon",\n "serde",\n')))
        self.assertFalse(ci_scope.same_but_version("Cargo.toml", TOML, "[workspace]\n"))


class Decision(unittest.TestCase):
    GUI = ["src/bin/gui/app.rs", "docs/USER_MANUAL.md", "site/index.html", ".github/workflows/build.yml"]

    def test_a_window_and_documents_change_sets_the_long_tests_aside(self):
        skip, why = ci_scope.decide(self.GUI, {}, [], base="efde43c0000")
        self.assertTrue(skip, why)
        skip, _ = ci_scope.decide(self.GUI + ["Cargo.toml", "Cargo.lock"], {"Cargo.toml": True, "Cargo.lock": True}, [], base="a" * 40)
        self.assertTrue(skip)
        self.assertTrue(ci_scope.decide([], {}, [], base="a" * 40)[0])   # the base is this commit

    def test_one_library_path_runs_the_whole_suite_and_is_named(self):
        for path in ("src/recipe.rs", "src/fit/joint.rs", "python/segment.py", "docs/images/a.png", "build.rs"):
            skip, why = ci_scope.decide(self.GUI + [path], {}, [], base="a" * 40)
            self.assertFalse(skip, path)
            self.assertIn(path, why)

    def test_a_dependency_change_runs_the_whole_suite(self):
        self.assertFalse(ci_scope.decide(["Cargo.lock"], {"Cargo.lock": False}, [], base="a" * 40)[0])
        self.assertFalse(ci_scope.decide(["Cargo.toml"], {}, [], base="a" * 40)[0])   # not read: not assumed

    def test_no_base_a_forced_run_and_a_neutral_read_each_run_the_whole_suite(self):
        self.assertFalse(ci_scope.decide(self.GUI, {}, [], base=None)[0])
        self.assertFalse(ci_scope.decide(self.GUI, {}, [], forced=True, base="a" * 40)[0])
        self.assertFalse(ci_scope.decide(self.GUI, {}, ["src/fit.rs: include of src/bin/gui/app.rs"], base="a" * 40)[0])

    def test_the_base_is_the_nearest_commit_with_a_green_run(self):
        green = {"c3", "c5"}
        self.assertEqual(ci_scope.base_commit(["c1", "c2", "c3", "c4", "c5"], green.__contains__), "c3")
        self.assertEqual(ci_scope.base_commit(["c3", "c4"], green.__contains__), "c3")   # this commit itself
        self.assertIsNone(ci_scope.base_commit(["c1", "c2"], green.__contains__))
        self.assertIsNone(ci_scope.base_commit([], green.__contains__))


if __name__ == "__main__":
    unittest.main()
