#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
mkdir -p artifacts/coverage
cargo llvm-cov --workspace --locked --ignore-filename-regex '/tests/|/fuzz/' --lcov --output-path artifacts/coverage/lcov.info --fail-under-lines 80
cargo llvm-cov report --ignore-filename-regex '/tests/|/fuzz/' --html --output-dir artifacts/coverage/html
cargo llvm-cov report --ignore-filename-regex '/tests/|/fuzz/' --json --output-path artifacts/coverage/llvm.json
python3 scripts/check-coverage.py artifacts/coverage/llvm.json
