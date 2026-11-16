#!/usr/bin/env python3
# /// script
# requires-python = ">=3.12"
# dependencies = ["lizard==1.26.0"]
# ///
"""Exercise the Rust analyzer and complexity gate on independent source fixtures."""

import json
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
CHECKER = ROOT / "scripts/check-complexity.py"
gate = runpy.run_path(str(CHECKER))


def branches(count):
    return "fn decisions(flag: bool) {\n" + "if flag { action(); }\n" * count + "}\n"


class ComplexityTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def source(self, text, name="fixture.rs"):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def test_ceiling_accepts_twenty_and_rejects_twenty_one(self):
        self.source(branches(19))
        report = gate["analyze"](self.root)
        self.assertEqual(report["maximum"], 20)
        self.assertEqual(report["violations"], [])
        self.source(branches(20))
        report = gate["analyze"](self.root)
        self.assertEqual(report["maximum"], 21)
        self.assertEqual(len(report["violations"]), 1)

    def test_nested_modules_cannot_escape_the_gate(self):
        self.source("fn simple() {}")
        self.source(branches(20), "nested/module.rs")
        report = gate["analyze"](self.root)
        self.assertEqual(report["function_count"], 2)
        self.assertEqual(report["violations"][0]["file"], "nested/module.rs")

    def test_analyzer_forgiveness_comments_do_not_exempt_functions(self):
        for comment in ("#lizard forgives", "#lizard forgive global",
                        "#lizard forgives(CCN)", "GENERATED CODE"):
            with self.subTest(comment=comment):
                self.source(f"// {comment}\n" + branches(20))
                self.assertEqual(len(gate["analyze"](self.root)["violations"]), 1)

    def test_comments_and_strings_are_not_branches(self):
        self.source('fn text() { let s = "if for while && || ?"; /* if for while */ }')
        self.assertEqual(gate["analyze"](self.root)["maximum"], 1)

    def test_lifetimes_generics_async_and_methods_are_all_discovered(self):
        self.source("""
fn borrowed<'a>(v: &'a str) -> &'a str { v }
async fn generic<T>(v: T) -> T { v }
struct Example;
impl Example { fn method(&self) -> &'static str { "value" } }
fn after() { if condition() { action(); } }
""")
        functions = gate["analyze"](self.root)["functions"]
        self.assertEqual({item["function"] for item in functions},
                         {"borrowed", "generic", "method", "after"})

    def test_error_propagation_and_match_branches_are_measured(self):
        self.source("""
fn fallible() -> Result<(), ()> { work()?; Ok(()) }
fn alternatives(n: u8) { match n { 0 => first(), 1 => second(), _ => last() } }
""")
        functions = gate["analyze"](self.root)["functions"]
        self.assertEqual([item["complexity"] for item in functions], [2, 3])

    def test_empty_or_missing_source_fails_closed(self):
        for path in (self.root, self.root / "missing"):
            with self.subTest(path=path), self.assertRaisesRegex(ValueError, "no Rust functions"):
                gate["analyze"](path)

    def test_unreadable_source_is_not_silently_skipped(self):
        self.source("fn simple() {}")
        (self.root / "broken.rs").write_bytes(b"\xff")
        with self.assertRaises(UnicodeError):
            gate["analyze"](self.root)

    def test_cli_fails_and_retains_measurements_for_review(self):
        self.source(branches(20))
        output = self.root / "report.json"
        result = subprocess.run([sys.executable, str(CHECKER), "--source-root",
                                 str(self.root), "--json", str(output)],
                                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("fixture.rs:1", result.stdout)
        self.assertEqual(json.loads(output.read_text())["maximum"], 21)

    def test_ci_and_local_analyzer_versions_match(self):
        pin = f"lizard=={gate['ANALYZER_VERSION']}"
        for path in (ROOT / "dev/requirements-ci.txt", CHECKER, Path(__file__)):
            with self.subTest(path=path):
                self.assertIn(pin, path.read_text())


if __name__ == "__main__":
    unittest.main()
