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
        build = inline_python('compatibility.yml', 'build', 'Test and build once per native architecture')
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

    def test_required_gate_rejects_accidentally_skipped_rust_jobs(self):
        code = inline_python('ci.yml', 'required', 'Required checks, including intentional documentation skips')
        names = ('lint', 'stable', 'coverage', 'faults', 'distribution')
        for needed in (True, False):
            results = {name: {'result':'skipped'} for name in names}
            results['contracts'] = {'result':'success','outputs':{'rust':str(needed).lower()}}
            with mock.patch.dict(os.environ, {'RESULTS':json.dumps(results)}):
                if needed:
                    with self.assertRaises(AssertionError): exec(compile(code,'required-gate','exec'), {})
                else:
                    exec(compile(code,'required-gate','exec'), {})
            if needed:
                for name in names: results[name]['result'] = 'success'
                with mock.patch.dict(os.environ, {'RESULTS':json.dumps(results)}):exec(compile(code,'required-gate','exec'), {})


if __name__ == '__main__':
    unittest.main()
