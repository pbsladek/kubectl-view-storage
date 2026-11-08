#!/bin/sh
set -eu
cd "$(dirname "$0")/../fuzz"
# The fuzz directory's nightly pin is independent of the production toolchain.
# cargo-fuzz has no --locked option. Resolve the checked-in graph with Cargo's
# locked check first; its following builds use only the prefetched dependencies.
cargo fetch --locked
export CARGO_NET_OFFLINE=true
lock_before=$(cksum Cargo.lock)
corpus_root=${FUZZ_CORPUS_DIR:-../artifacts/fuzz-corpus}
mkdir -p "$corpus_root/quantity" "$corpus_root/statistics"
# libFuzzer writes new inputs to its first corpus directory; checked-in seeds
# remain separate from the disposable generated corpus.
cargo fuzz run quantity "$corpus_root/quantity" corpus/quantity -- -max_total_time="${FUZZ_SECONDS:-15}" -max_len=1024 -timeout=2
cargo fuzz run statistics "$corpus_root/statistics" corpus/statistics -- -max_total_time="${FUZZ_SECONDS:-15}" -max_len=32768 -timeout=2
if [ "$(cksum Cargo.lock)" != "$lock_before" ]; then
    echo 'FAIL fuzz build changed the checked-in dependency lock' >&2
    exit 1
fi
