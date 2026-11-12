#!/usr/bin/env python3
# /// script
# requires-python = ">=3.12"
# dependencies = ["jsonschema==4.26.0"]
# ///
"""Independently check actual calculation exports with Python integers/Fraction."""
import argparse
import copy
from datetime import datetime, timezone
from fractions import Fraction
import json
from pathlib import Path
import re
from statistics import median

from jsonschema import Draft202012Validator, FormatChecker
from jsonschema.exceptions import ValidationError

ROOT = Path(__file__).resolve().parents[1]


def ratio(value):
    return None if value is None else Fraction(int(value['numerator']), int(value['denominator']))


def ns(value):
    match = re.fullmatch(r'(.*:\d{2})(?:\.(\d{1,9}))?Z', value)
    assert match, 'noncanonical observation timestamp'
    delta = datetime.fromisoformat(match[1]).replace(tzinfo=timezone.utc) - datetime(1970, 1, 1, tzinfo=timezone.utc)
    return (delta.days * 86400 + delta.seconds) * 10**9 + int((match[2] or '').ljust(9, '0'))


def distribution(actual, values, total, complete):
    values = sorted(values)
    expected_sum = sum(values) if values or total == 0 else None
    assert actual['known_count'] == len(values), 'known count'
    assert actual['total_count'] == total, 'total count'
    assert actual['complete'] == (complete and len(values) == total), 'completeness'
    assert actual['sum_bytes'] == (str(expected_sum) if expected_sum is not None else None), 'sum'
    for field, expected in (
        ('min_bytes', min(values) if values else None),
        ('max_bytes', max(values) if values else None),
        ('p95_bytes', values[(95 * len(values) + 99) // 100 - 1] if values else None),
    ):
        assert actual[field] == (str(expected) if expected is not None else None), field
    assert ratio(actual['mean_bytes']) == (Fraction(sum(values), len(values)) if values else None), 'mean'
    # statistics.median would round large integers through float for even sizes.
    expected_median = median([Fraction(value) for value in values]) if values else None
    assert ratio(actual['median_bytes']) == expected_median, 'median'


def validate(report, baseline=None):
    schema = json.loads((ROOT / 'docs/schema/report.schema.json').read_text())
    Draft202012Validator(schema, format_checker=FormatChecker()).validate(report)
    calculations = report['calculations']
    assert calculations['physical_total'] is False, 'physical capacity claim'
    records = {record['id']: record for record in report['records']}
    assert len(records) == len(report['records']), 'duplicate record identity'
    groups = {'total': (set(records), report['summary'])}

    def visit(items):
        for group in items:
            groups[group['id']] = (set(group['member_ids']), group['totals'])
            visit(group['children'])

    visit(report['groups'])
    assert {row['id'] for row in calculations['rows']} == groups.keys(), 'calculation group coverage'
    for row in calculations['rows']:
        ids, totals = groups[row['id']]
        assert row['totals'] == totals, 'calculation totals changed'
        members = [records[identity] for identity in ids]
        for measure in ('requested', 'bound', 'provisioned'):
            values = [int(r[measure + '_bytes']) for r in members if r[measure + '_bytes'] is not None]
            distribution(row['statistics'][measure], values, len(members), totals['selection_complete'])
        pairs = [r for r in members if all(r[key + '_bytes'] is not None for key in ('requested', 'bound', 'provisioned'))]
        gaps = row['capacity_gaps']
        assert gaps['known_count'] == len(pairs), 'gap pairs'
        assert gaps['total_count'] == len(members), 'gap total'
        assert gaps['complete'] == (totals['selection_complete'] and len(pairs) == len(members)), 'gap completeness'
        for field, left, right in (
            ('requested_above_bound_bytes', 'requested_bytes', 'bound_bytes'),
            ('provisioned_above_requested_bytes', 'provisioned_bytes', 'requested_bytes'),
        ):
            expected = str(sum(max(int(r[left]) - int(r[right]), 0) for r in pairs)) if pairs or not members else None
            assert gaps[field] == expected, 'gap sum'
    classes = {}
    states = {}
    for record in records.values():
        state = record['storage_class_state']
        states[state] = states.get(state, 0) + 1
        if state == 'known':
            classes.setdefault(record['storage_class'], []).append(record)
    class_stats = calculations['storage_classes']
    assert class_stats['named_count'] == len(classes), 'class count'
    assert class_stats['record_counts_by_assignment'] == states, 'class assignment counts'
    for measure in ('requested', 'bound', 'provisioned'):
        sums = []
        complete = report['selection']['complete'] and 'unknown' not in states
        for members in classes.values():
            values = [int(r[measure + '_bytes']) for r in members if r[measure + '_bytes'] is not None]
            complete &= len(values) == len(members)
            if values:
                sums.append(sum(values))
        distribution(class_stats['statistics'][measure], sums, len(classes), complete)

    comparison = calculations['comparison']
    if comparison is None:
        return
    assert baseline is not None, 'growth validation requires the original baseline'
    old = {record['id']: record for record in baseline['records']}
    rates = {rate['record_id']: rate for rate in comparison['records']}
    assert rates.keys() == records.keys(), 'rate identity coverage'
    for identity, rate in rates.items():
        if rate['status'] in ('unavailable', 'not_applicable'):
            assert rate['growth_bytes_per_second'] is None and rate['time_to_full_seconds'] is None, 'unavailable rate fabricated'
            continue
        current, previous = records[identity], old[identity]
        for field in ('uid', 'kind', 'namespace', 'name', 'created_at', 'pv_name', 'pvc_name', 'pvc_uid',
                      'storage_class', 'storage_class_state', 'driver', 'volume_mode'):
            assert current.get(field) == previous.get(field), 'identity or binding changed'
        if current['kind'] == 'pv':
            assert all(current.get(field) and previous.get(field) for field in ('pvc_name', 'pvc_uid')), 'unverified PV binding'
        assert current['filesystem']['status'] == previous['filesystem']['status'] == 'ok', 'unusable pair'
        assert not current.get('overlapping_filesystem') and not previous.get('overlapping_filesystem'), 'overlapping pair'
        assert current['filesystem']['capacity_bytes'] == previous['filesystem']['capacity_bytes'], 'resized pair'
        elapsed = ns(current['filesystem']['observed_at']) - ns(previous['filesystem']['observed_at'])
        assert elapsed > 0 and rate['interval_nanoseconds'] == str(elapsed), 'sample interval'
        delta = int(current['filesystem']['used_bytes']) - int(previous['filesystem']['used_bytes'])
        expected = Fraction(delta * 10**9, elapsed)
        assert ratio(rate['growth_bytes_per_second']) == expected, 'growth rate'
        assert rate['status'] == ('growing' if delta > 0 else 'shrinking' if delta < 0 else 'steady'), 'growth sign'
        available = int(current['filesystem']['available_bytes'])
        full = Fraction(0) if available == 0 else Fraction(available) / expected if expected > 0 else None
        if full is not None and full.numerator >= 2**128:
            assert rate['time_to_full_seconds'] is None and rate['reason'] == 'projection_overflow', 'projection overflow'
        else:
            assert ratio(rate['time_to_full_seconds']) == full, 'time to full'
    for row in calculations['rows']:
        ids, totals = groups[row['id']]
        eligible = [identity for identity in ids if records[identity]['volume_mode'] != 'Block']
        paired = [rates[identity] for identity in eligible if rates[identity]['status'] != 'unavailable']
        actual = row['growth']
        assert actual['eligible_count'] == len(eligible), 'growth eligible count'
        assert actual['matched_count'] == len(paired), 'growth pair count'
        expected = sum((ratio(r['growth_bytes_per_second']) for r in paired), Fraction(0)) if paired else None
        assert ratio(actual['growth_bytes_per_second']) == expected, 'group growth'
        times = [ratio(r['time_to_full_seconds']) for r in paired if r['time_to_full_seconds'] is not None]
        assert ratio(actual['first_full_seconds']) == (min(times) if times else None), 'first claim full'
        assert actual['complete'] == (totals['selection_complete'] and len(paired) == len(eligible) and all(r['reason'] != 'projection_overflow' for r in paired)), 'growth completeness'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reports', type=Path, nargs='+')
    parser.add_argument('--baseline', type=Path)
    parser.add_argument('--negative-checks', action='store_true')
    args = parser.parse_args()
    baseline = json.loads(args.baseline.read_text()) if args.baseline else None
    for path in args.reports:
        report = json.loads(path.read_text())
        validate(report, baseline)
        if args.negative_checks:
            for field in ('sum_bytes', 'min_bytes', 'max_bytes', 'p95_bytes', 'mean_bytes', 'median_bytes'):
                corrupted = copy.deepcopy(report)
                stats = corrupted['calculations']['rows'][0]['statistics']['requested']
                stats[field] = {'numerator': '1', 'denominator': '7'} if field in ('mean_bytes', 'median_bytes') else str(int(stats[field] or '0') + 1)
                try:
                    validate(corrupted, baseline)
                except (AssertionError, ValueError, ValidationError):
                    pass
                else:
                    raise AssertionError(f'validator accepted corrupted {field}')
        print(f'PASS independent calculation schema/arithmetic: {path}')


if __name__ == '__main__':
    main()
