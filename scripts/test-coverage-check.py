#!/usr/bin/env python3
"""Regression checks for weighted thresholds and missing coverage evidence."""
import importlib.util
from pathlib import Path
import unittest
import tempfile

spec = importlib.util.spec_from_file_location("coverage_check", Path(__file__).with_name("check-coverage.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class CoverageChecks(unittest.TestCase):
    def test_complete_gate_rejects_unmeasured_executable_or_adapter(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in module.CORE:
                (root / f"{name}.rs").write_text("pub fn implemented() {}\n")
            (root / "main.rs").write_text("fn main() {}\n")
            report = self.report()
            for file in report["data"][0]["files"]:
                file["filename"] = str(root / Path(file["filename"]).name)
            with self.assertRaisesRegex(ValueError, "main.rs"):
                module.measure(report, root, require_complete=True)
            self.assertEqual(module.measure(report, root)["overall"], (450, 500))
    def report(self):
        return {"data": [{"files": [
            {"filename": f"/tmp/source/{name}.rs", "summary": {"lines": {"count": 100, "covered": 90}}}
            for name in module.CORE
        ]}]}

    def test_weighted_not_average_and_duplicates_not_double_counted(self):
        report = self.report()
        report["data"][0]["files"].extend([
            {"filename": "/tmp/source/output.rs", "summary": {"lines": {"count": 1000, "covered": 0}}},
            report["data"][0]["files"][0],
            {"filename": "/tmp/dependencies/fake.rs", "summary": {"lines": {"count": 90000, "covered": 90000}}},
        ])
        self.assertEqual(module.measure(report, Path("/tmp/source"))["overall"], (450, 1500))

    def test_missing_and_inconsistent_evidence_fails(self):
        for mutate in [
            lambda r: r["data"][0]["files"].pop(),
            lambda r: r["data"][0]["files"].append({"filename": "/tmp/source/qty.rs", "summary": {"lines": {"count": 100, "covered": 99}}}),
            lambda r: r["data"].clear(),
        ]:
            report = self.report()
            mutate(report)
            with self.assertRaises(ValueError):
                module.measure(report, Path("/tmp/source"))


if __name__ == "__main__":
    unittest.main()
