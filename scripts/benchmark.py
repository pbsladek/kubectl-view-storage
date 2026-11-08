#!/usr/bin/env python3
"""Record the specification-scale in-memory tests on a named Unix runner."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import resource
import subprocess
import sys
import time


def main():
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runner", required=True)
    parser.add_argument("--harness", type=Path, help="Explicit preliminary cached-dependency harness")
    args = parser.parse_args()
    if not args.runner or any(not (c.isalnum() or c in "-_") for c in args.runner):
        parser.error("runner must be a nonempty alphanumeric label with - or _")
    manifest = args.harness or root / "Cargo.toml"
    if not args.harness and not manifest.with_name("Cargo.lock").exists():
        parser.exit(1, "A resolved production Cargo.lock is required.\n")
    command = ["cargo", "test", "--manifest-path", str(manifest), "--locked", "--lib", "--no-run", "--message-format=json"]
    if args.harness:
        command.insert(2, "--offline")
    built = subprocess.run(command, cwd=root, text=True, capture_output=True, timeout=600)
    if built.returncode:
        parser.exit(1, "Benchmark test build failed; no performance result claimed.\n" + built.stderr)
    artifacts = [json.loads(line) for line in built.stdout.splitlines() if line.startswith("{")]
    binaries = [entry["executable"] for entry in artifacts if entry.get("reason") == "compiler-artifact" and entry.get("executable") and "lib" in entry["target"]["kind"]]
    if len(binaries) != 1:
        parser.exit(1, "Cannot identify exactly one library-test executable.\n")
    probe = subprocess.run([sys.executable, str(Path(__file__).resolve()), "--measure-child", binaries[0]], cwd=root, text=True, capture_output=True, timeout=150)
    if probe.returncode:
        parser.exit(1, "Benchmark measurement process failed.\n" + probe.stderr)
    result = json.loads(probe.stdout)
    destination = root / "artifacts/benchmarks"
    destination.mkdir(parents=True, exist_ok=True)
    (destination / f"{args.runner}.log").write_text(result["stdout"] + result["stderr"])
    if result["returncode"] or "2 passed" not in result["stdout"]:
        parser.exit(1, "Both specification-scale cases must pass; inspect the benchmark log.\n")
    digest = hashlib.sha256()
    for path in sorted(root.glob("src/*.rs")) + sorted((root / "tests").rglob("*.rs")):
        digest.update(str(path.relative_to(root)).encode())
        digest.update(path.read_bytes())
    evidence = {
        "runner": args.runner, "platform": platform.system(), "architecture": platform.machine(),
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "dependency_scope": "cached harness; preliminary" if args.harness else "locked production package",
        "transport": "in_memory", "profile": "debug", "source_sha256": digest.hexdigest(),
        "test_seconds": result["seconds"], "peak_test_rss_bytes": result["peak_test_rss_bytes"],
        "measurement_scope": "test executable only; compilation excluded; cases run serially",
        "cases": 2, "claims_per_case": 10000, "pod_references_per_case": 50000,
        "budgets": "no performance ceiling adopted; request-cost assertions enforced",
    }
    (destination / f"{args.runner}.json").write_text(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps(evidence, indent=2))


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--measure-child":
        env = dict(os.environ)
        env.pop("VIEW_STORAGE_TEST_REPORTS", None)
        started = time.perf_counter()
        result = subprocess.run([sys.argv[2], "performance::specification_scale", "--test-threads=1", "--nocapture"], env=env, text=True, capture_output=True, timeout=120)
        elapsed = time.perf_counter() - started
        rss = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
        print(json.dumps({"returncode": result.returncode, "stdout": result.stdout, "stderr": result.stderr, "seconds": elapsed, "peak_test_rss_bytes": rss if platform.system() == "Darwin" else rss * 1024}))
    else:
        main()
