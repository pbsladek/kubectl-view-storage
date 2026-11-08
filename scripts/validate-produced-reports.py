#!/usr/bin/env python3
# /// script
# requires-python = ">=3.12"
# dependencies = ["jsonschema==4.26.0"]
# ///
"""Validate actual Rust pipeline reports against the schema and independent oracle."""
import argparse
import copy
from pathlib import Path
import runpy

from jsonschema import Draft202012Validator, FormatChecker
from jsonschema.exceptions import ValidationError


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--http", action="store_true", help="Validate nine real HTTP reports with their captured fixture bodies/reference clocks")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    oracle = runpy.run_path(str(root / "scripts/validate-contract.py"))
    load = oracle["load"]
    schema = Draft202012Validator(load(root / "docs/schema/report.schema.json"), format_checker=FormatChecker())
    for scenario in load(root / "examples/contract/manifest.json")["scenarios"]:
        report = load(args.directory / (scenario["id"] + ".json"))
        schema.validate(report)
        oracle["check_semantics"](report, scenario["expected_exit"])
        raw_input = args.directory / (scenario["id"] + "-input.json") if args.http else root / "examples/contract" / scenario["input"]
        oracle["check_inputs"](report, load(raw_input))
        print(f"PASS Rust report: {scenario['id']}")
    if args.http:
        print("PASS nine real HTTP/executable reports against the independent schema/input/accounting oracle")
        return
    overlap = load(args.directory / "known-overlap.json")
    schema.validate(overlap)
    oracle["check_semantics"](overlap, 0)
    oracle["check_inputs"](overlap, load(args.directory / "known-overlap-input.json"))
    # The independent verifier must reject a fabricated additive total and a
    # successful required-metrics exit when one individual tuple is missing.
    for fault in ("additive_overlap", "missing_required_tuple", "diagnostic_order", "affected_field_order"):
        mutated = copy.deepcopy(overlap)
        if fault == "additive_overlap":
            mutated["summary"]["filesystem"]["used_bytes"] = "85899345920"
            expected_reason = "common-subset sum"
        elif fault == "missing_required_tuple":
            metric = mutated["records"][0]["filesystem"]
            metric.update(status="missing", reason="no_sample", source=None, observed_at=None,
                          candidate_count=0, candidates=[], observed_range_percent=None, used_percent=None)
            for field in oracle["BYTE_FIELDS"]:
                metric[field] = None
                metric["field_status"][field] = "missing"
            # First prove this is a valid missing-data report with exit 4.
            oracle["check_semantics"](mutated, 4)
            expected_reason = "strict-policy exit"
        elif fault == "diagnostic_order":
            assert len(mutated["diagnostics"]) >= 2
            mutated["diagnostics"].reverse()
            expected_reason = "diagnostic order"
        else:
            next(d for d in mutated["diagnostics"] if len(d["affected_fields"]) >= 2)["affected_fields"].reverse()
            expected_reason = "diagnostic field names"
        try:
            oracle["check_semantics"](mutated, 0)
        except AssertionError as error:
            assert expected_reason in str(error), f"wrong rejection for {fault}: {error}"
        else:
            raise AssertionError(f"overlap verifier missed {fault}")
    print("PASS Rust report: known-overlap; additive/required-tuple/diagnostic-order negative checks")
    classes = load(args.directory / "classless-unassigned.json")
    raw_classes = load(args.directory / "classless-unassigned-input.json")
    schema.validate(classes)
    oracle["check_semantics"](classes, 0)
    oracle["check_inputs"](classes, raw_classes)
    mutated = copy.deepcopy(classes)
    next(record for record in mutated["records"] if record["storage_class_state"] == "unassigned")["storage_class_state"] = "classless"
    try:
        oracle["check_inputs"](mutated, raw_classes)
    except AssertionError as error:
        assert "storage class state" in str(error), error
    else:
        raise AssertionError("class verifier missed collapsed unassigned/classless states")
    print("PASS Rust report: classless-unassigned; collapsed-state negative check")
    for name, exit_code in (("volume-mode", 4), ("invalid-volume-mode", 4), ("pod-phase", 4), ("shared-lifecycle", 0)):
        report = load(args.directory / f"unsupported-{name}.json")
        raw = load(args.directory / f"unsupported-{name}-input.json")
        schema.validate(report)
        oracle["check_semantics"](report, exit_code)
        oracle["check_inputs"](report, raw)
        if name != "shared-lifecycle":
            try:
                oracle["check_semantics"](report, 0)
            except AssertionError as error:
                assert "strict-policy exit" in str(error), error
            else:
                raise AssertionError("unsupported classification passed a dependent strict check")
        mutated = copy.deepcopy(report)
        if name in ("pod-phase", "shared-lifecycle"):
            record = next(record for record in mutated["records"] if record["name"] == "data")
            record.update(consumer_visibility="complete", active_consumer_count=0, terminating_consumer_count=0)
            try:
                oracle["check_semantics"](mutated, exit_code)
            except AssertionError as error:
                assert "unknown lifecycle" in str(error), error
            else:
                raise AssertionError("unknown lifecycle acquired false zero counts")
            mutated = copy.deepcopy(report)
            next(pod for pod in mutated["consumers"] if pod["reference_state"] == "unknown")["active"] = False
            try:
                schema.validate(mutated)
            except ValidationError:
                pass
            else:
                raise AssertionError("schema accepted unknown activity as false")
        else:
            next(record for record in mutated["records"] if record["name"] == "data")["volume_mode"] = "Filesystem"
            try:
                oracle["check_inputs"](mutated, raw)
            except AssertionError as error:
                assert "volume mode" in str(error), error
            else:
                raise AssertionError("unsupported mode was coerced to Filesystem")
        print(f"PASS Rust report: unsupported-{name}; classification/strict-policy negative checks")
    for name in ("distinct-capacity-pvc", "distinct-capacity-pv", "raw-block-pvc", "raw-block-pv"):
        report = load(args.directory / f"{name}.json")
        raw = load(args.directory / f"{name}-input.json")
        schema.validate(report)
        oracle["check_semantics"](report, 0)
        oracle["check_inputs"](report, raw)
        faults = ("copied_request", "copied_bound", "copied_provisioned", "derived_available") if name.startswith("distinct") else ("block_status", "block_observation")
        for fault in faults:
            mutated = copy.deepcopy(report)
            record = mutated["records"][0]
            if fault == "copied_request":
                record["requested_bytes"] = record["provisioned_bytes"]
                verifier, expected_reason = "check_inputs", "API request mismatch"
            elif fault == "copied_bound":
                record["bound_bytes"] = record["requested_bytes"]
                verifier, expected_reason = "check_inputs", "API bound capacity mismatch"
            elif fault == "copied_provisioned":
                record["provisioned_bytes"] = record["bound_bytes"]
                verifier, expected_reason = "check_inputs", "API provisioned capacity mismatch"
            elif fault == "derived_available":
                metric = record["filesystem"]
                metric["available_bytes"] = str(int(metric["capacity_bytes"]) - int(metric["used_bytes"]))
                verifier, expected_reason = "check_inputs", "API observation mismatch"
            elif fault == "block_status":
                record["filesystem"]["status"] = "ok"
                verifier, expected_reason = "check_semantics", "raw block filesystem metrics"
            else:
                record["inodes"]["inodes_used"] = "0"
                verifier, expected_reason = "check_semantics", "raw block acquired filesystem observations"
            try:
                oracle[verifier](mutated, raw if verifier == "check_inputs" else 0)
            except AssertionError as error:
                assert expected_reason in str(error), f"wrong rejection for {name}/{fault}: {error}"
            else:
                raise AssertionError(f"verifier missed {name}/{fault}")
        print(f"PASS Rust report: {name}; independent quantity/block negative checks")


if __name__ == "__main__":
    main()
