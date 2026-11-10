#!/usr/bin/env python3
"""Cargo crate versions differ from the bundled executable release number."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("check_toolchain", Path(__file__).with_name("check-toolchain.py"))
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)


class StableBundle(unittest.TestCase):
    def test_manifest_crate_version_is_not_binary_release(self):
        manifest = {"pkg": {"rust": {"version": "1.99.1 (58cf8f9c3 2026-10-13)"}, "cargo": {"version": "0.100.0 (5f94df478 2026-08-27)"}}}
        verbose = "cargo 1.99.1 (5f94df478 2026-08-27)\nrelease: 1.99.1\ncommit-hash: 5f94df4789f005f9a352888e8355ffc645b7ed0e\n"
        check.latest_bundle("1.99.1", "1.99.1", manifest, verbose)
        for broken in (verbose.replace("release: 1.99.1", "release: 1.99.0"), verbose.replace("5f94df4789", "0000000000"), "missing metadata"):
            with self.assertRaises(ValueError): check.latest_bundle("1.99.1", "1.99.1", manifest, broken)
        with self.assertRaisesRegex(ValueError, "tested update"):
            check.latest_bundle("1.99.0", "1.99.1", manifest, verbose)


if __name__ == "__main__":
    unittest.main()
