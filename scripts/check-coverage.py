#!/usr/bin/env python3
"""Enforce weighted production coverage from cargo-llvm-cov's LLVM JSON export."""
import argparse
import json
import re
from pathlib import Path

CORE = ("qty", "join", "group", "findings", "policy", "calculations")


def measure(report, source_root, require_complete=False):
    source_root = source_root.resolve()
    files = {}
    for data in report["data"]:
        for item in data["files"]:
            filename = Path(item["filename"]).resolve()
            if not filename.is_relative_to(source_root):
                continue
            lines = item["summary"]["lines"]
            count, covered = lines["count"], lines["covered"]
            if not isinstance(count, int) or not isinstance(covered, int) or not 0 <= covered <= count:
                raise ValueError(f"invalid line counters: {filename}")
            # A file can appear in several binaries. LLVM's merged export must
            # provide one consistent summary; counting repetitions inflates gates.
            if filename in files and files[filename] != (count, covered):
                raise ValueError(f"inconsistent merged coverage: {filename}")
            files[filename] = (count, covered)
    if not files:
        raise ValueError("no production source coverage found")
    if require_complete:
        expected = {path.resolve() for path in source_root.rglob("*.rs") if re.search(r"\bfn\s+\w+\s*[<(]", path.read_text())}
        missing = expected - files.keys()
        if missing:
            raise ValueError("missing production source files: " + ", ".join(str(p.relative_to(source_root)) for p in sorted(missing)))
    results = {}
    scopes = {"overall": list(files)}
    for module in CORE:
        scopes[module] = [p for p in files if p.relative_to(source_root).parts[0] in (module, module + ".rs")]
    for name, paths in scopes.items():
        count = sum(files[p][0] for p in paths)
        covered = sum(files[p][1] for p in paths)
        if count == 0:
            raise ValueError(f"missing instrumented production lines: {name}")
        results[name] = (covered, count)
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    parser.add_argument("--source-root", type=Path, default=Path(__file__).resolve().parents[1] / "src")
    parser.add_argument("--allow-missing-source", action="store_true", help="Preliminary subset evidence only; not a production coverage gate")
    args = parser.parse_args()
    try:
        results = measure(json.loads(args.report.read_text()), args.source_root, require_complete=not args.allow_missing_source)
        if args.allow_missing_source:
            print("PRELIMINARY subset coverage; complete production-source gate not enforced")
        failed = False
        for name, (covered, count) in results.items():
            required = 80 if name == "overall" else 90
            passed = covered * 100 >= required * count
            print(f"{'PASS' if passed else 'FAIL'} {name}: {covered}/{count} ({covered / count:.2%}); required {required}%")
            failed |= not passed
        return int(failed)
    except (ValueError, KeyError, OSError) as error:
        parser.exit(1, f"coverage error: {error}\n")


if __name__ == "__main__":
    raise SystemExit(main())
