#!/usr/bin/env python3
"""A documentation shortcut must not hide compiler, schema or smoke changes."""
import importlib.util
import subprocess
from pathlib import Path
from types import SimpleNamespace
import contextlib
import io
import os
import tempfile
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('ci_scope', Path(__file__).with_name('ci-scope.py'))
scope = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scope)


class ScopeTests(unittest.TestCase):
    def test_documentation_only_avoids_compilation(self):
        self.assertFalse(scope.rust_needed(['README.md', 'docs/SPEC.md', 'examples/live/csi/report.json']))

    def test_compiled_contracts_and_unknown_paths_are_not_documentation(self):
        for name in ('src/output.rs', 'Cargo.lock', 'rust-toolchain.toml', 'examples/contract/healthy/table.txt',
                     'docs/schema/report.schema.json', 'scripts/check-plugin.py', 'scripts/smoke_http.py',
                     'scripts/package-release.py', 'scripts/test-credential-terminal.py', 'scripts/test-smoke-http.py', '.github/actions/cargo-tool/action.yml', 'new-compiler-input'):
            with self.subTest(name=name):
                self.assertTrue(scope.rust_needed(['README.md', name]))

    def test_tool_only_changes_keep_rust_checks_without_rebuilding_distributions(self):
        for name in scope.CI_TOOL_ONLY:
            with self.subTest(name=name):
                self.assertTrue(scope.rust_needed([name]))
                self.assertFalse(scope.distribution_needed(['README.md', name]))
                self.assertTrue(scope.distribution_needed([name, 'src/main.rs']))

    def test_release_and_unknown_inputs_still_require_distribution_validation(self):
        for name in ('Cargo.lock', 'rust-toolchain.toml', 'src/output.rs',
                     '.github/workflows/ci.yml', '.github/workflows/native-distribution.yml',
                     '.github/actions/rust-cache/action.yml', 'scripts/check-plugin.py',
                     'scripts/package-release.py', 'scripts/test-credential-terminal.py',
                     'scripts/smoke_http.py', 'new-compiler-input'):
            with self.subTest(name=name):
                self.assertTrue(scope.distribution_needed([name]))
        self.assertFalse(scope.distribution_needed(['README.md', 'docs/ci-baseline.json']))

    def test_main_emits_independent_rust_and_distribution_flags(self):
        for paths, expected in [(None, (True, True)), (['README.md'], (False, False)),
                                (['.github/actions/cargo-tool/action.yml'], (True, False)),
                                (['src/main.rs', '.github/actions/cargo-tool/action.yml'], (True, True))]:
            with self.subTest(paths=paths), tempfile.TemporaryDirectory() as temporary:
                output, summary = Path(temporary)/'output', Path(temporary)/'summary'
                with mock.patch.object(scope.argparse.ArgumentParser, 'parse_args', return_value=SimpleNamespace(event=None, event_name='push')), mock.patch.object(scope, 'changed_paths', return_value=paths), mock.patch.dict(os.environ, {'GITHUB_OUTPUT': str(output), 'GITHUB_STEP_SUMMARY': str(summary)}), contextlib.redirect_stdout(io.StringIO()):
                    scope.main()
                self.assertEqual(output.read_text(), f'rust={str(expected[0]).lower()}\ndistribution={str(expected[1]).lower()}\n')

    def test_force_push_or_missing_history_runs_full_checks(self):
        with mock.patch.object(scope.subprocess, 'run', side_effect=subprocess.CalledProcessError(1, 'git')):
            self.assertIsNone(scope.changed_paths({'before': 'abc', 'after': 'def'}, 'push'))
        with mock.patch.object(scope, 'verified_push_base', return_value=None):
            self.assertIsNone(scope.changed_paths({'before': '0'*40, 'after': 'def'}, 'push'))
        self.assertIsNone(scope.changed_paths({}, 'workflow_dispatch'))

    def test_push_diff_starts_at_successful_ancestor_not_last_unverified_push(self):
        event = {'before': 'untested-source', 'after': 'docs-followup'}
        result = subprocess.CompletedProcess([], 0, stdout=b'src/output.rs\0README.md\0')
        with mock.patch.object(scope, 'verified_push_base', return_value='last-green'), mock.patch.object(scope.subprocess, 'run', return_value=result) as call:
            self.assertTrue(scope.rust_needed(scope.changed_paths(event, 'push')))
        self.assertIn('last-green..docs-followup', call.call_args.args[0])

    def test_successful_nonancestor_is_not_a_validation_baseline(self):
        runs = subprocess.CompletedProcess([], 0, stdout='{"workflow_runs":[{"head_sha":"other-branch"},{"head_sha":"ancestor"}]}')
        with mock.patch.dict(scope.os.environ, {'GITHUB_REPOSITORY':'owner/repo'}), mock.patch.object(scope.subprocess, 'run', side_effect=[runs, subprocess.CompletedProcess([],1), subprocess.CompletedProcess([],0)]):
            self.assertEqual(scope.verified_push_base('head'), 'ancestor')

    def test_pull_requests_compare_with_merge_base_and_preserve_deleted_paths(self):
        event = {'pull_request': {'base': {'sha': 'base'}, 'head': {'sha': 'head'}}}
        result = subprocess.CompletedProcess([], 0, stdout=b'src/deleted.rs\0README.md\0')
        with mock.patch.object(scope.subprocess, 'run', return_value=result) as call:
            self.assertEqual(scope.changed_paths(event, 'pull_request'), ['src/deleted.rs', 'README.md'])
        self.assertIn('base...head', call.call_args.args[0])


if __name__ == '__main__':
    unittest.main()
