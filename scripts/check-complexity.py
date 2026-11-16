#!/usr/bin/env python3
# /// script
# requires-python = ">=3.12"
# dependencies = ["lizard==1.26.0"]
# ///
"""Enforce a cyclomatic complexity ceiling for every production Rust function."""

import argparse
from importlib.metadata import version
import json
from pathlib import Path

import lizard

ANALYZER_VERSION = "1.26.0"
MAX_COMPLEXITY = 20
ROOT = Path(__file__).resolve().parents[1]


def comments_without_exemptions(tokens, reader):
    """Use the language reader's comments without Lizard's forgiveness pragmas."""
    for token in tokens:
        comment = reader.get_comment_from_token(token)
        if comment is None:
            yield token
        else:
            yield from ("\n" for _ in comment.splitlines()[1:])


def analyze(source_root):
    if version("lizard") != ANALYZER_VERSION:
        raise ValueError(f"use lizard {ANALYZER_VERSION}; parser versions affect measurements")
    processors = [comments_without_exemptions if processor is lizard.comment_counter else processor
                  for processor in lizard.get_extensions([])]
    analyzer = lizard.FileAnalyzer(processors)
    functions = []
    for path in sorted(source_root.rglob("*.rs")):
        # Read explicitly: FileAnalyzer's file entrypoint swallows I/O failures.
        analyzed = analyzer.analyze_source_code(str(path), path.read_text(encoding="utf-8"))
        for function in analyzed.function_list:
            functions.append({
                "file": path.relative_to(source_root).as_posix(),
                "function": function.name,
                "line": function.start_line,
                "complexity": function.cyclomatic_complexity,
            })
    if not functions:
        raise ValueError(f"no Rust functions found under {source_root}")
    violations = [item for item in functions if item["complexity"] > MAX_COMPLEXITY]
    return {
        "analyzer": f"lizard {ANALYZER_VERSION}",
        "limit": MAX_COMPLEXITY,
        "function_count": len(functions),
        "maximum": max(item["complexity"] for item in functions),
        "violations": violations,
        "functions": functions,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, default=ROOT / "src")
    parser.add_argument("--json", type=Path, help="Write measurements for review")
    args = parser.parse_args()
    try:
        report = analyze(args.source_root)
    except (OSError, ValueError) as error:
        parser.exit(2, f"complexity check error: {error}\n")
    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(report, indent=2) + "\n")
    for item in report["violations"]:
        print(f"FAIL {item['file']}:{item['line']} {item['function']}: "
              f"complexity {item['complexity']} exceeds {MAX_COMPLEXITY}")
    passed = not report["violations"]
    print(f"{'PASS' if passed else 'FAIL'} Rust complexity: "
          f"{report['function_count']} functions; maximum {report['maximum']}; "
          f"limit {MAX_COMPLEXITY}")
    return int(not passed)


if __name__ == "__main__":
    raise SystemExit(main())
