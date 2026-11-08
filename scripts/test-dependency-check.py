#!/usr/bin/env python3
"""Regressions for the dependency budget, target scope, and feature policy."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("dependency_check", Path(__file__).with_name("check-dependencies.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def dependency(**options):
    return {"version": "1.0", "default-features": False, **options}


class DependencyChecks(unittest.TestCase):
    def test_target_dependencies_count_but_development_tools_do_not(self):
        manifest = {"dependencies": {f"crate{i}": dependency() for i in range(module.MAX_RUNTIME_DEPENDENCIES)},
                    "dev-dependencies": {"test-only": dependency()},
                    "target": {"cfg(unix)": {"dependencies": {"extra": dependency()}}}}
        runtime, errors = module.inspect_manifest(manifest)
        self.assertEqual(len(runtime), module.MAX_RUNTIME_DEPENDENCIES + 1)
        self.assertNotIn("test-only", runtime)
        self.assertTrue(any("budget" in error for error in errors))

    def test_alias_and_repeated_target_are_one_package(self):
        manifest = {"dependencies": {"alias": dependency(package="http")},
                    "target": {"cfg(unix)": {"dependencies": {"http": dependency()}}}}
        self.assertEqual(module.inspect_manifest(manifest), ({"http"}, []))

    def test_unreviewed_defaults_sources_and_features_fail(self):
        for name, options in [("clap", "4"), ("tokio", dependency(features=["full"])),
                              ("kube", dependency(features=["runtime"])),
                              ("helper", dependency(version="*")),
                              ("helper", dependency(version="1.0.0-rc.1")),
                              ("nix", dependency(features=["aio"])),
                              ("helper", dependency(git="https://example.invalid/helper"))]:
            with self.subTest(name=name, options=options):
                self.assertTrue(module.inspect_manifest({"dependencies": {name: options}})[1])

    def test_package_replacement_requires_review_even_within_budget(self):
        runtime, errors = module.inspect_manifest({"dependencies": {"unreviewed-helper": dependency()}})
        self.assertEqual(runtime, {"unreviewed-helper"})
        self.assertTrue(any("maintenance/cost review" in error for error in errors))

    def test_source_overrides_cannot_bypass_release_source_policy(self):
        for table, value in (("patch", {"crates-io": {"http": {"path": "local-http"}}}),
                             ("replace", {"http:1.5.0": {"git": "https://example.invalid/http"}})):
            with self.subTest(table=table):
                self.assertTrue(any("source overrides" in error for error in module.inspect_manifest({table: value})[1]))

    def test_build_dependencies_require_explicit_cost_review(self):
        for scope in ({"build-dependencies": {"builder": dependency()}},
                      {"target": {"cfg(unix)": {"build-dependencies": {"builder": dependency()}}}}):
            with self.subTest(manifest=scope):
                self.assertTrue(any("build-cost rationale" in error for error in module.inspect_manifest(scope)[1]))


if __name__ == "__main__":
    unittest.main()
