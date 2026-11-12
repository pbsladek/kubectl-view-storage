#!/usr/bin/env python3
"""Offline checks for the live calculation gate, arithmetic and owned cleanup."""
import copy
import json
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import patch

import dev_calculations as calc
from dev_csi import timestamp_ns


def reports():
    before = {'records': [{'id': 'pvc:claim', 'filesystem': {'used_bytes': '100', 'observed_at': '2026-11-11T00:00:00Z'}}]}
    after = {'selection': {'complete': True}, 'records': [{'id': 'pvc:claim', 'filesystem': {
        'used_bytes': '116', 'available_bytes': '884', 'observed_at': '2026-11-11T00:00:04Z'}}],
        'calculations': {'rows': [{'statistics': {'requested': {'sum_bytes': str(1024**3)}}, 'growth': {'complete': True}}],
                         'comparison': {'records': [{'status': 'growing', 'growth_bytes_per_second': {'numerator': '4', 'denominator': '1'},
                                                    'time_to_full_seconds': {'numerator': '221', 'denominator': '1'}}]}}}
    return before, after


class CalculationGates(unittest.TestCase):
    def test_growth_gate_checks_raw_bytes_actual_interval_available_and_deduplication(self):
        before, after = reports()
        self.assertEqual(calc.validate_growth(after, before, timestamp_ns)['delta_used_bytes'], '16')
        for pointer, value in (
            (('calculations', 'rows', 0, 'statistics', 'requested', 'sum_bytes'), str(2 * 1024**3)),
            (('calculations', 'comparison', 'records', 0, 'growth_bytes_per_second', 'numerator'), '5'),
            (('calculations', 'comparison', 'records', 0, 'time_to_full_seconds', 'numerator'), '225'),
            (('records', 0, 'filesystem', 'observed_at'), '2026-11-11T00:00:00Z'),
            (('records', 0, 'filesystem', 'used_bytes'), '100'),
            (('selection', 'complete'), False),
        ):
            altered = copy.deepcopy(after)
            parent = altered
            for key in pointer[:-1]:
                parent = parent[key]
            parent[pointer[-1]] = value
            with self.subTest(pointer=pointer), self.assertRaises(RuntimeError):
                calc.validate_growth(altered, before, timestamp_ns)

    def test_query_bounds_runtime_uses_explicit_config_and_sanitizes_failed_payloads(self):
        response = SimpleNamespace(returncode=0, stdout='{"ok":true}', stderr='')
        with patch.object(calc.subprocess, 'run', return_value=response) as call:
            self.assertEqual(calc.query('plugin', 'owned-config', 'fixture', '--pvc', 'data'), {'ok': True})
            self.assertIn('owned-config', call.call_args.args[0])
            self.assertEqual(call.call_args.kwargs['timeout'], 30)
        response.returncode = 1
        response.stderr = 'PRIVATE_TEST_DATA'
        with patch.object(calc.subprocess, 'run', return_value=response):
            with self.assertRaises(RuntimeError) as error:
                calc.query('plugin', 'owned-config', 'fixture')
            self.assertNotIn('PRIVATE_TEST_DATA', str(error.exception))

    def gate(self, root, *, bad_scope=False, fail_create=False):
        before, after = reports()
        (root / 'csi-before-report.json').write_text(json.dumps(before))
        run_id = '11111111-1111-4111-8111-111111111111'
        name = 'calculation-db-' + run_id[:8]
        scoped = {'records': [{}], 'calculations': after['calculations'],
                  'consumers': [{'statefulset': {'uid': 'owner'}}] * (1 if bad_scope else 2),
                  'groups': [{'value': f'fixture/{name}@owner'}]}
        calls = []

        def kubectl(config, *args):
            self.assertEqual(config, 'owned-config')
            calls.append(args)
            if args[0] == 'create' and fail_create:
                raise RuntimeError('create failed')
            return json.dumps({'metadata': {'uid': 'owner'}})

        def query(*args, **kwargs):
            if kwargs.get('output') in ('text', 'csv'):
                return 'captured output\n'
            return scoped if '--statefulset' in args else after

        with patch.object(calc, 'query', side_effect=query):
            try:
                return calc.verify_csi(kubectl, 'owned-config', 'plugin', root, run_id, 'fixture', 'pinned-test-image', timestamp_ns), calls
            finally:
                deletes = [call for call in calls if 'delete' in call]
                self.assertEqual(len(deletes), 0 if fail_create else 1)
                if deletes:
                    self.assertIn('--cascade=foreground', deletes[0])
                    self.assertIn(name, deletes[0])

    def test_owned_statefulset_cleanup_and_capture_after_success(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            result, _ = self.gate(root)
            self.assertEqual(result['statefulset']['pod_count'], 2)
            self.assertTrue((root / 'calculations-growth.json').exists())
            self.assertTrue((root / 'calculations-statefulset.csv').exists())
            fixture = json.loads((root / 'calculations-statefulset-fixture.json').read_text())
            self.assertEqual(fixture['spec']['template']['spec']['terminationGracePeriodSeconds'], 1)

    def test_scope_failure_still_deletes_only_the_owned_statefulset(self):
        with tempfile.TemporaryDirectory() as temporary, self.assertRaises(RuntimeError):
            self.gate(Path(temporary), bad_scope=True)

    def test_failed_create_does_not_delete_an_existing_statefulset(self):
        with tempfile.TemporaryDirectory() as temporary, self.assertRaises(RuntimeError):
            self.gate(Path(temporary), fail_create=True)


if __name__ == '__main__':
    unittest.main()
