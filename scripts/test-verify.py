#!/usr/bin/env python3
"""Verify fresh evidence directories, failure propagation, and report validation."""
import json
from pathlib import Path
import runpy
import subprocess
import tempfile
import unittest
from unittest.mock import patch

verify = runpy.run_path(str(Path(__file__).with_name("verify.py")))["verify"]


class VerificationTests(unittest.TestCase):
    def test_reports_belong_to_current_run_and_both_oracles_run(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run:
                self.assertEqual(verify(root), 0)
                first = sorted((root / "artifacts").iterdir())[0]
                self.assertEqual(verify(root), 0)
            directories = list((root / "artifacts").iterdir())
            self.assertEqual(len(directories), 2)
            calls = run.call_args_list[:5]
            environment = calls[0].kwargs["env"]
            self.assertEqual(Path(environment["VIEW_STORAGE_TEST_REPORTS"]).parent, first)
            self.assertEqual(Path(environment["VIEW_STORAGE_HTTP_REPORTS"]).parent, first)
            self.assertIn("validate-produced-reports.py", " ".join(calls[3].args[0]))
            self.assertIn("--http", calls[4].args[0])
            self.assertTrue(json.loads((first / "verification.json").read_text())["passed"])

    def test_stops_and_retains_receipt_when_a_check_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 7)) as run:
                self.assertEqual(verify(root), 7)
                self.assertEqual(run.call_count, 1)
            receipt = json.loads(next((root / "artifacts").glob("*/verification.json")).read_text())
            self.assertFalse(receipt["passed"])
            self.assertEqual(receipt["checks"][0]["exit_code"], 7)


if __name__ == "__main__":
    unittest.main()
