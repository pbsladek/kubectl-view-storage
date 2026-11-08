#!/usr/bin/env python3
"""Prove targeted regression tests reject four concrete faults in isolated source copies."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
CASES = [
    ("reversed_threshold", "src/qty.rs", "r >= self.ratio()", "r <= self.ratio()", "qty::tests::thresholds_are_exact_and_canonical"),
    ("duplicated_capacity", "src/group.rs", "sum.checked_add(value.get() as u128)", "sum.checked_add((value.get() as u128)*2)", "group::tests::repeated_consumers_and_dimension_order_preserve_union_totals"),
    ("swallowed_lookup_failure", "src/collect.rs", r"self\.diagnostics\.push\(Diagnostic\{[^\n]*\}\);", "", "collect::tests::denied_optional_pv_keeps_inventory_and_live_bytes"),
    ("unbounded_worker_shutdown", "src/collect.rs", "now.checked_add(Duration::from_secs(1))", "now.checked_add(Duration::from_secs(60))", "collect::tests::shutdown_budget_aborts_uncooperative_async_worker"),
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--harness", type=Path, help="Explicit cached-dependency harness for preliminary local evidence only")
    args = parser.parse_args()
    evidence = ROOT / "artifacts/mutations"
    evidence.mkdir(parents=True, exist_ok=True)
    results = []
    with tempfile.TemporaryDirectory(prefix="view-storage-mutations-") as directory:
        work = Path(directory) / "source"
        work.mkdir()
        for name in ("src", "tests", "examples"):
            shutil.copytree(ROOT / name, work / name)
        for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "README.md", "LICENSE"):
            if (ROOT / name).exists():
                shutil.copy2(ROOT / name, work / name)
        if args.harness:
            harness = Path(directory) / "harness"
            harness.mkdir()
            # This copies the caller's explicit test harness; the production
            # manifest and selected versions in the workspace are never edited.
            (harness / "Cargo.toml").write_text(args.harness.read_text().replace(str(ROOT / "src/lib.rs"), str(work / "src/lib.rs")))
            if args.harness.with_name("Cargo.lock").exists():
                shutil.copy2(args.harness.with_name("Cargo.lock"), harness / "Cargo.lock")
            manifest = harness / "Cargo.toml"
        else:
            if not (work / "Cargo.lock").exists():
                parser.exit(1, "A resolved production Cargo.lock is required; use --harness only for explicitly preliminary local checks.\n")
            manifest = work / "Cargo.toml"
        env = dict(os.environ, CARGO_TARGET_DIR=str(evidence / "target"))
        env.pop("VIEW_STORAGE_TEST_REPORTS", None)
        for name, filename, before, after, test in CASES:
            path = work / filename
            original = path.read_text()
            command = ["cargo", "test", "--manifest-path", str(manifest), "--locked", "--lib", test, "--", "--exact"]
            if args.harness:
                command.insert(2, "--offline")
            baseline = subprocess.run(command, cwd=work, env=env, text=True, capture_output=True, timeout=180)
            if baseline.returncode or "1 passed" not in baseline.stdout:
                (evidence / f"{name}.baseline.log").write_text(baseline.stdout + baseline.stderr)
                parser.exit(1, f"Baseline failed or test was absent: {test}\n")
            if name == "swallowed_lookup_failure":
                mutated, count = re.subn(before, after, original)
            else:
                count = original.count(before)
                mutated = original.replace(before, after)
            if count != 1:
                parser.exit(1, f"Mutation source changed; review {name} (matches={count}).\n")
            path.write_text(mutated)
            try:
                fault = subprocess.run(command, cwd=work, env=env, text=True, capture_output=True, timeout=180)
            finally:
                path.write_text(original)
            output = fault.stdout + fault.stderr
            (evidence / f"{name}.log").write_text(output)
            caught = fault.returncode != 0 and "panicked at" in output and f"{test} ... FAILED" in output
            results.append({"mutation": name, "test": test, "caught_by_assertion": caught, "original_sha256": hashlib.sha256(original.encode()).hexdigest()})
            print(f"{'PASS' if caught else 'FAIL'} fault detection: {name}", flush=True)
            if not caught:
                parser.exit(1, f"The mutation was not rejected by its behavioral assertion: {name}\n")
    (evidence / "results.json").write_text(json.dumps({"dependency_scope": "cached harness; preliminary" if args.harness else "locked production package", "results": results}, indent=2) + "\n")


if __name__ == "__main__":
    main()
