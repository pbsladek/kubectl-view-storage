#!/usr/bin/env python3
"""Exercise inline workflow protocols, not just YAML syntax."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock

import yaml

ROOT = Path(__file__).resolve().parents[1]


def inline_python(workflow, job, step_name):
    document = yaml.safe_load((ROOT / '.github/workflows' / workflow).read_text())
    step = next(step for step in document['jobs'][job]['steps'] if step.get('name') == step_name)
    blocks = re.findall(r"python3 - <<'PY'\n(.*?)\nPY(?:\n|$)", step['run'], re.S)
    if len(blocks) != 1:
        raise AssertionError('expected exactly one inline Python block')
    return blocks[0]


class WorkflowProtocols(unittest.TestCase):
    def test_cached_cargo_tools_receive_the_cargo_subcommand_and_verify_versions(self):
        action = yaml.safe_load((ROOT / '.github/actions/cargo-tool/action.yml').read_text())
        code = next(step['run'] for step in action['runs']['steps'] if step.get('name') == 'Verify version and expose tool')
        for tool, version in [('cargo-llvm-cov', '0.9.1'), ('cargo-deny', '0.19.0'), ('cargo-fuzz', '0.13.2')]:
            for fault in (False, True):
                with self.subTest(tool=tool, wrong_version=fault), tempfile.TemporaryDirectory(prefix='cargo tool ') as temporary:
                    root = Path(temporary)
                    directory = root / '.local/ci-tools' / tool / 'bin'
                    directory.mkdir(parents=True)
                    executable = directory / tool
                    # Cargo passes its external subcommand as argv[1]. The
                    # llvm-cov binary rejects direct --version without it.
                    executable.write_text('#!' + sys.executable + '\nimport os,sys\nassert sys.argv[1:]==[os.environ["CI_TOOL"].removeprefix("cargo-"),"--version"]\nprint(os.environ["CI_TOOL"], "0.0.0" if os.environ["WRONG_VERSION"]=="true" else os.environ["CI_VERSION"])\n')
                    executable.chmod(0o755)
                    output = root / 'github-path'
                    env = dict(os.environ, CI_TOOL=tool, CI_VERSION=version, GITHUB_WORKSPACE=str(root), GITHUB_PATH=str(output), WRONG_VERSION=str(fault).lower())
                    result = subprocess.run(['bash', '-eo', 'pipefail', '-c', code], env=env, text=True, capture_output=True)
                    if fault:
                        self.assertNotEqual(result.returncode, 0)
                        self.assertFalse(output.exists())
                    else:
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertEqual(output.read_text(), str(directory) + '\n')

    def test_matching_clients_emit_two_distinct_github_outputs(self):
        code = inline_python('compatibility.yml', 'live', 'Select reviewed node and matching client')
        for version in ('1.37.0', '1.36.4', '1.35.8'):
            with self.subTest(version=version), tempfile.TemporaryDirectory() as temporary:
                output = Path(temporary) / 'output'
                env = dict(os.environ, SELECTED_KUBERNETES=version, GITHUB_OUTPUT=str(output))
                subprocess.run([sys.executable, '-c', code], cwd=ROOT, env=env, check=True, capture_output=True)
                lines = output.read_text().splitlines()
                self.assertEqual(len(lines), 2)
                self.assertTrue(lines[0].startswith(f'image=kindest/node:v{version}@sha256:'))
                self.assertTrue(lines[1].startswith('kubectl=v'))

    def test_binary_reuse_checks_revision_architecture_hash_and_regular_member(self):
        build = inline_python('compatibility.yml', 'build', 'Build once per native architecture')
        consume = inline_python('compatibility.yml', 'live', 'Verify same-revision binary built for this architecture')
        for fault in (None, 'revision', 'architecture', 'hash', 'link'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                binary = root / 'target/release/kubectl-view_storage'
                binary.parent.mkdir(parents=True)
                binary.write_bytes(b'owned fixture bytes')
                (root / 'artifacts/compatibility-build').mkdir(parents=True)
                env = dict(os.environ, GITHUB_SHA='current-revision')
                subprocess.run([sys.executable, '-c', build], cwd=root, env=env, check=True, capture_output=True)
                receipt = json.loads((root / 'artifacts/compatibility-build/source.json').read_text())
                source = root / '.local/compatibility-artifact'
                source.mkdir(parents=True)
                if fault == 'revision': receipt['revision'] = 'stale-revision'
                if fault == 'architecture': receipt['architecture'] = 'different-architecture'
                if fault == 'hash': receipt['binary_sha256'] = '0'*64
                (source / 'source.json').write_text(json.dumps(receipt)+'\n')
                with tarfile.open(source / 'binary.tar.gz', 'w:gz') as archive:
                    if fault == 'link':
                        member = tarfile.TarInfo('kubectl-view_storage')
                        member.type, member.linkname = tarfile.SYMTYPE, '/outside'
                        archive.addfile(member)
                    else:
                        archive.add(binary, arcname='kubectl-view_storage')
                result = subprocess.run([sys.executable, '-c', consume], cwd=root, env=env, capture_output=True)
                installed = root / '.local/compatibility-bin/kubectl-view_storage'
                if fault is None:
                    self.assertEqual(result.returncode, 0, result.stderr.decode())
                    self.assertEqual(hashlib.sha256(installed.read_bytes()).hexdigest(), receipt['binary_sha256'])
                    self.assertTrue(os.access(installed, os.X_OK))
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(installed.exists())

    def test_nightly_selection_writes_a_real_environment_line(self):
        code = inline_python('fuzz.yml', 'fuzz', 'Select pinned isolated fuzz compiler')
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / 'env'
            env = dict(os.environ, GITHUB_ENV=str(output))
            subprocess.run([sys.executable, '-c', code], cwd=ROOT, env=env, check=True, capture_output=True)
            self.assertEqual(output.read_text(), 'RUSTUP_TOOLCHAIN=nightly-2026-11-01\n')

    def test_required_gate_checks_rust_and_release_scopes_independently(self):
        code = inline_python('ci.yml', 'required', 'Required checks')
        for rust, release in ((False, False), (True, False), (False, True), (True, True)):
            expected = {'lint', 'coverage'} if rust else set()
            if release: expected.add('distribution')
            results = {name: {'result':'success' if name in expected else 'skipped'} for name in ('lint', 'coverage', 'distribution')}
            results['contracts'] = {'result':'success','outputs':{'rust':str(rust).lower(), 'distribution':str(release).lower()}}
            with self.subTest(rust=rust, release=release), mock.patch.dict(os.environ, {'RESULTS':json.dumps(results)}):
                exec(compile(code, 'required-gate', 'exec'), {})
            for name in expected | {'contracts'}:
                broken = json.loads(json.dumps(results))
                broken[name]['result'] = 'skipped'
                with self.subTest(skipped=name, rust=rust, release=release), mock.patch.dict(os.environ, {'RESULTS':json.dumps(broken)}):
                    with self.assertRaises(AssertionError): exec(compile(code, 'required-gate', 'exec'), {})
            for missing in ('rust', 'distribution'):
                broken = json.loads(json.dumps(results))
                broken['contracts']['outputs'].pop(missing)
                with mock.patch.dict(os.environ, {'RESULTS':json.dumps(broken)}):
                    with self.assertRaises(AssertionError): exec(compile(code, 'required-gate', 'exec'), {})

    def test_faults_remain_in_the_required_linux_lint_job(self):
        jobs = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())['jobs']
        self.assertNotIn('faults', jobs)
        self.assertNotIn('stable', jobs)
        self.assertIn('lint', jobs['required']['needs'])
        lint = jobs['lint']
        self.assertEqual({row['os'] for row in lint['strategy']['matrix']['include']}, {'ubuntu-24.04', 'macos-15'})
        fault = next(step for step in lint['steps'] if step.get('run') == 'python3 scripts/check-mutations.py')
        self.assertEqual(fault['if'], "matrix.os == 'ubuntu-24.04'")

    def test_storage_build_preserves_release_and_terminal_checks_without_duplicate_suite(self):
        jobs = yaml.safe_load((ROOT / '.github/workflows/compatibility.yml').read_text())['jobs']
        command = next(step['run'] for step in jobs['build']['steps'] if step.get('name') == 'Build once per native architecture')
        self.assertNotIn('cargo test', command)
        self.assertIn('cargo build --locked --release', command)
        self.assertIn('scripts/test-credential-terminal.py --binary target/release/kubectl-view_storage', command)
        self.assertEqual({row['os'] for row in jobs['build']['strategy']['matrix']['include']}, set(jobs['live']['strategy']['matrix']['os']))

    def test_stable_compatibility_selects_only_a_different_compiler(self):
        code = inline_python('maintenance.yml', 'stable', 'Resolve latest stable and avoid testing the same compiler twice')
        import tomllib
        pinned = tomllib.loads((ROOT / 'rust-toolchain.toml').read_text())['toolchain']['channel']
        for version, differs in ((pinned, False), ('9.99.0', True)):
            with self.subTest(version=version), tempfile.TemporaryDirectory() as temporary:
                output, summary = Path(temporary)/'output', Path(temporary)/'summary'
                with mock.patch.dict(os.environ, {'GITHUB_OUTPUT':str(output), 'GITHUB_STEP_SUMMARY':str(summary)}), mock.patch.object(subprocess, 'check_output', return_value=f'rustc {version} (test revision)'):
                    exec(compile(code, 'stable-selection', 'exec'), {})
                self.assertEqual(output.read_text(), 'differs=' + str(differs).lower() + '\n')
        jobs = yaml.safe_load((ROOT / '.github/workflows/maintenance.yml').read_text())['jobs']
        self.assertEqual({row['os'] for row in jobs['stable']['strategy']['matrix']['include']}, {'ubuntu-24.04', 'macos-15'})
        self.assertEqual(jobs['stable']['if'], "github.event_name == 'schedule' || github.event_name == 'workflow_dispatch'")

    def test_release_keeps_native_and_installed_validation_for_all_four_targets(self):
        jobs = yaml.safe_load((ROOT / '.github/workflows/native-distribution.yml').read_text())['jobs']
        expected = {'x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl', 'x86_64-apple-darwin', 'aarch64-apple-darwin'}
        for name in ('native', 'installed'):
            self.assertEqual({row['target'] for row in jobs[name]['strategy']['matrix']['include']}, expected)
        for name, dependency in (('manifest', 'native'), ('installed', 'manifest')):
            needs = jobs[name]['needs']
            self.assertIn(dependency, [needs] if isinstance(needs, str) else needs)


if __name__ == '__main__':
    unittest.main()
