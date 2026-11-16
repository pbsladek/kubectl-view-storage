# Repository guidance

## Project and sources of truth

This is a Rust kubectl plugin for persistent storage inventory, consumers, usage,
findings, statistics, and observed growth. The Cargo package is
`kubectl-view-storage`, the library import is `kubectl_view_storage`, and the
executable is **`kubectl-view_storage`**. kubectl discovers it as
`kubectl view-storage`. Preserve these distinct names in scripts and packaging.

Read the relevant contract before changing behavior:

- [docs/SPEC.md](docs/SPEC.md): CLI, collection budgets, accounting, exports,
  diagnostics, exit precedence, and acceptance cases.
- [docs/DESIGN.md](docs/DESIGN.md): architecture and design decisions.
- [docs/CODE_QUALITY.md](docs/CODE_QUALITY.md): enforced complexity ceiling and
  Rust design rules.
- [docs/schema/report.schema.json](docs/schema/report.schema.json) and
  [examples/contract/README.md](examples/contract/README.md): report structure
  and independent expected examples.
- [docs/CALCULATIONS.md](docs/CALCULATIONS.md) and
  [docs/OBSERVATION.md](docs/OBSERVATION.md): statistics, comparison, watch,
  Prometheus, and workload contracts.
- [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md),
  [docs/STORAGE_E2E.md](docs/STORAGE_E2E.md), and
  [docs/DISTRIBUTION.md](docs/DISTRIBUTION.md): development and validation.

Historical receipts and phase plans retain their original dates and source scopes.
Check current source, workflows, [docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md),
and [docs/ACCEPTANCE_STATUS.md](docs/ACCEPTANCE_STATUS.md) before treating planned
work as missing or claiming a gate passed. Workflow definitions describe checks;
execution receipts establish platform and storage compatibility.

## Architecture

The binary in `src/main.rs` owns configuration, signals, sequential watch cycles,
collection, finalization, output, and exit status. `src/lib.rs` exposes collection
and accounting independently of terminal I/O. Keep these boundaries:

| Area | Files and responsibilities |
| --- | --- |
| CLI | `src/cli.rs`: Clap flags, validation, resolved `Query`, filters, grouping, and sort options. |
| Production I/O | `src/kubernetes.rs`: kubeconfig, SDK authentication/TLS/proxy, separately configured Prometheus client, streaming body limits, and consumed-byte counts. Implements `collect::Transport`. |
| Credential lifecycle | `src/exec_supervisor.rs`: process isolation, terminal ownership, interruption, and reaping for SDK credential helpers. |
| Collection | `src/collect.rs`: inventory, pagination, caching, retries, request budgets, and owned async workers. Private `src/collect/enrichment.rs` handles prerequisites and membership; `src/collect/telemetry.rs` handles provider requests and observation processing. Depends on injected `Transport` and `Clock`. |
| API projection and identity | `src/transport.rs`: bounded JSON decoding and API object projections; `src/join.rs`: UID-based graph orchestration and validation, with private record and consumer modules under `src/join/`; `src/workloads.rs`: verified controller chains. |
| Domain and arithmetic | `src/model.rs`: validated identities and explicit metadata/metric states; `src/qty.rs`: byte/inode types, Kubernetes quantities, exact ratios, and thresholds. |
| Telemetry | `src/metrics.rs`: Summary API attribution, candidate validation, freshness, and selection; `src/prometheus.rs`: explicit provider scope and raw sample normalization. |
| Accounting and policy | `src/group.rs`: member unions, totals, and coverage; `src/findings.rs`: evidence-based health rules; `src/policy.rs`: exit decisions; `src/sort.rs`: deterministic ordering. |
| Finalization | `src/pipeline.rs`: shared graph validation, findings, exit policy, sorting, grouping, report adaptation, and optional calculations. All views use this path. |
| Wire contract | `src/report.rs`: explicit domain-to-JSON adapter and selection metadata; `src/calculations.rs`: exact statistics and bounded saved-report comparisons. |
| Presentation | `src/output.rs`: rendering entrypoints and CSV rows; private `src/output/views.rs` and `src/output/statistics.rs`: text views and statistics, with `src/output/details.rs` for costs/timing/guidance; binary modules `src/csv_output.rs` and `src/stdio.rs`: CSV serialization and cancellable output with descriptor flag restoration. |
| Work control | `src/clock.rs`: monotonic versus UTC clocks; `src/work.rs`: cooperative checkpoints, controlled sorting, and owned CPU workers. |

Collection normalizes and selects the graph before finalization. Renderers consume
the finalized report; they must not fetch resources, recompute selection, or make
independent health decisions. Statistics extend the same selected report.

## Invariants to preserve

- Key base records by PVC/PV UID and consumers by pod UID. Verify binding and
  controller references; names alone cannot establish continuity across object
  recreation. One record-to-pod edge retains all declared mounts/devices.
- Compute group and root totals from distinct member-ID unions. Multiple pods,
  nodes, mounts, or overlapping child groups must not multiply claim capacity.
  Keep requested, bound, provisioned, and filesystem capacities separate.
- Preserve missing, invalid, unknown, absent, unassigned, and actual zero states.
  Optional failures retain inventory and explicit diagnostics. Failed or partial
  required inventory is fatal, never a successful empty result. Unknown filter
  membership remains separate from known matches.
- Use checked integer accounting and exact ratios: per-record counters are bounded
  `u64`, aggregate arithmetic uses `u128`, and calculation rates retain rational
  values. Round for presentation only. Never average utilization percentages.
- Keep byte and inode families independent. Aggregate compatible fresh complete
  tuples; retain stale, partial, conflicting, or uncertain candidates explicitly.
  Known Block volumes have no filesystem metrics. Known shared CSI filesystem
  identities are excluded from additive metrics, while individual observations
  remain visible. Logical per-claim sums are not physical backend totals.
- Preserve staged request priorities and deterministic budget allocation. Current
  defaults are 60 seconds overall, 10 seconds per request, concurrency 8, list
  pages of 500, and 256 optional enrichment attempts. Response limits are 32 MiB,
  JSON nesting 128 levels, and quantity/counter scalars 256 UTF-8 bytes. Consult
  `docs/SPEC.md` for retries, bulk lookup thresholds, and stage deadlines.
- Use monotonic time for budgets, retries, and shutdown; UTC only for observation
  timestamps and freshness. New long CPU loops must participate in `work.rs`
  checkpoints. Cancel and join owned tasks and output workers before returning;
  aborting a blocking task does not stop its CPU work. Preserve bounded async
  shutdown and credential-helper process cleanup on Linux and macOS.
- Keep deterministic sorting, including identity ties and unknown values, across
  inputs, concurrency, views, and formats. Unknown sort values remain last in
  either direction. Watch cycles are sequential; watch JSON is newline-delimited.
- Keep JSON/CSV stdout parseable and diagnostics on stderr. JSON schema
  `view-storage/v1alpha1` uses decimal strings for counters and null for unknown
  quantities. Ordinary CSV has one row per distinct record; stats has its own
  header. Update schema, fixtures, validators, and docs together for contract
  changes; incompatible changes require a new schema identifier and release notes.
- Exit codes are `0` success, `1` fatal collection/output failure, `2` invalid
  invocation, `3` findings, `4` inconclusive strict policy, and `130` interruption.
  Strict uncertainty takes precedence over findings. Codes 3/4 still emit a
  complete report document. Keep policy centralized in `src/policy.rs`.
- Never log raw Summary API bodies, kubeconfig, credentials, helper stderr, or
  unrelated namespace data. Prometheus credentials are separate from Kubernetes
  credentials. Preserve redaction in reports, errors, and captured artifacts.

Build metadata comes from std-only `build.rs` and `src/build_info.rs`; keep the
concise `--version` stable. Completion assets under `completions/` are generated
from the Clap command by dev-only `dev/completion*.rs` and embedded by the binary.
Include these files in source packages and isolated build-source copies.

## Toolchain and dependency policy

Use the compiler pinned in `rust-toolchain.toml` and its bundled Cargo, currently
Rust 1.100.0 with edition 2024 and matching `Cargo.toml` `rust-version`. Production
Cargo commands use `--locked`; retain `Cargo.lock`. Fuzzing has a separate nightly
pin, package, and `fuzz/Cargo.lock`.

Unsafe Rust is forbidden. Production Clippy denies `unwrap` and `expect`; tests
have explicit local allowances. Follow existing typed error and diagnostic paths
for external data. Run rustfmt rather than introducing a separate style.

Production functions must have cyclomatic complexity at most 20; aim for 10 or
less in new code. `make complexity` enforces this with pinned Lizard over all
`src/**/*.rs`. Extract distinct responsibilities into private helpers/modules;
do not suppress violations, raise the limit, or split code into arbitrary chunks.
Keep checkpoints and typed error propagation intact across extracted helpers.

`scripts/check-dependencies.py` enforces thirteen reviewed runtime packages,
crates.io sources, disabled default features, and narrow feature selections.
Dependency additions/expansions need a recorded maintenance and build-cost
rationale in [docs/DEPENDENCY_REVIEW.md](docs/DEPENDENCY_REVIEW.md), with policy
changes where needed. Do not bypass the guard by changing its budget alone.
`deny.toml` governs advisories, licenses, bans, and sources.

## Local checks and tests

Run commands from the repository root. Build with `cargo build --locked`;
the executable is `target/debug/kubectl-view_storage`. Python validation uses
Python 3.12+; CI uses 3.14 and the pinned packages in `dev/requirements-ci.txt`.
`uv run` supplies the contract validator's declared dependency.

| Purpose | Command |
| --- | --- |
| Local checks with fresh independently validated reports | `make verify` |
| Broad local checks | `make check` |
| Regenerate shell completions after CLI changes | `make completions` (freshness tested by Cargo) |
| Production function complexity | `make complexity` and `uv run scripts/test-complexity.py` |
| Full Rust unit/property/CLI/HTTP suite | `cargo test --locked --all-targets` or `make test` |
| Formatting and lint | `cargo fmt --all --check` and `cargo clippy --locked --all-targets -- -D warnings` |
| Independent checked-in contract validation | `make contract` |
| Production coverage | `make coverage` |
| Real kubectl discovery/help/version smoke | `make plugin-smoke` (requires kubectl and a built binary) |
| Release version/locks/completions preparation | `make release-prepare VERSION=X.Y.Z` |
| Native release build and verified download | `make release VERSION=X.Y.Z` (pushes the version tag and dispatches CI) |
| Explicit GitHub Release publication | `make release-publish VERSION=X.Y.Z` (`DRAFT=1` for a draft) |
| Controlled fault detection | `python3 scripts/check-mutations.py` |
| Dependency policy and resolved audit | `python3 scripts/check-dependencies.py` and `cargo deny --locked check advisories bans licenses sources` |
| Workflow protocol regressions | `uv run --with PyYAML==6.0.3 python scripts/test-ci-workflows.py` |
| Bounded parser fuzzing | `sh scripts/fuzz-smoke.sh` (requires pinned nightly and cargo-fuzz) |

`make verify` runs `make check`, workflow protocol tests, contract/schema validation,
and independent validators against fresh test-generated reports. It writes a receipt
under `artifacts/verify-*`. `make check` alone omits those extra validations. Neither
command covers the entire CI matrix: coverage, mutations, advisories, plugin
installation, and live storage have separate entrypoints. Run checks appropriate
to the change and report what actually ran.

Unit tests live in `tests/unit/*.rs` and are included from source modules with
`#[cfg(test)]`/`#[path]`; they run through `--lib`, not as individual integration
targets. Use e.g. `cargo test --locked --lib qty::tests` for focused iteration.
`tests/arithmetic_properties.rs` and `tests/unit/properties.rs` cover arithmetic,
identity, grouping, and ordering laws; `tests/unit/performance.rs` exercises the
10,000-claim/50,000-reference request-cost fixture.

`tests/cli.rs` runs the binary for invocation behavior. `tests/client.rs` and
`tests/support/` exercise the real SDK/executable against loopback wiremock APIs,
including budgets, credential processes, watch, Prometheus, and workloads.
`scripts/test-credential-terminal.py` adds real PTY/helper lifecycle tests.
These cluster-free tests still require local socket/process support.

Expected contract fixtures are independent oracles: do not regenerate their
numbers or rules from implementation output to make tests pass. To validate
actual produced reports, export both report directories during a full test run:

```sh
VIEW_STORAGE_TEST_REPORTS="$PWD/artifacts/api-reports" \
VIEW_STORAGE_HTTP_REPORTS="$PWD/artifacts/http-reports" \
  cargo test --locked --all-targets
uv run scripts/validate-produced-reports.py artifacts/api-reports
uv run scripts/validate-produced-reports.py --http artifacts/http-reports
```

Coverage uses cargo-llvm-cov and compatible LLVM tools. `scripts/coverage.sh`
enforces 80% overall production line coverage and 90% separately for `qty`,
`join`, `group`, `findings`, `policy`, and `calculations`. Test/fuzz source is
excluded; all production source must be represented. Keep the independent
integer/Fraction calculation oracle and controlled-fault assertions intact.

## Disposable storage tests

Use the owned harness and reviewed tools/images in `dev/versions.json`,
`dev/csi/lock.json`, and `dev/nfs/lock.json`. After building,
`python3 scripts/e2e-storage.py` runs local-path, CSI hostpath, and cross-node NFS
CSI serially and cleans up. `--profiles nfs-csi` selects one backend.

For a cluster retained for debugging, use
`python3 scripts/dev-cluster.py create PROFILE`, then `verify PROFILE` and
`delete PROFILE`, where PROFILE is `inventory`, `csi`, or `nfs`. Generated
kubeconfigs and ownership state live under ignored `.local/`; reports and
receipts live under ignored `artifacts/`.

Keep kubeconfig selection explicit. Never switch the user's current context,
adopt an unowned cluster, delete arbitrary containers, or clean arbitrary
kubeconfigs. Preserve run-ID/container-identity checks and failure cleanup.
Use bounded writes; never fill the host/Docker disk to test pressure. Mock exact
exhaustion and conflict cases. Inventory success does not establish telemetry
support. The NFS fixture restarts its node driver to refresh an upstream stats
cache; it does not establish normal cache-refresh latency.

## CI/CD and release preparation

- `.github/workflows/ci.yml`: runs on main pushes, PRs, and manual dispatch.
  The `contracts` job always checks Python policies, harnesses, workflow protocols,
  and fixtures. `scripts/ci-scope.py` selects Rust and distribution work separately.
  Documentation-only changes can skip compilation; compiled fixtures, schema,
  unknown paths, and missing history conservatively select Rust checks. Main
  comparisons include changes since a successful ancestor, including failed or
  cancelled pushes.
- Selected CI runs Linux/macOS Clippy, Linux formatting and controlled faults,
  full Linux coverage plus independently verified produced reports, and the
  reusable native distribution workflow. The final `required` job validates
  selected jobs and permitted skips; preserve it as the aggregate gate.
- `.github/workflows/native-distribution.yml`: native tests, PTY/plugin checks,
  release builds, verified Cargo source packages and actual source installation,
  deterministic archives, combined manifest/checksums, and isolated Krew installs.
  Targets are Linux x86_64/aarch64 musl and macOS x86_64/aarch64 Darwin. Archives
  contain the canonical executable and exact LICENSE; do not substitute renamed
  host binaries, synthetic headers, or `cargo package --no-verify` for real builds.
- `.github/workflows/compatibility.yml` (Storage): builds GNU release binaries
  once per native Linux architecture, verifies revision/architecture/hash before
  reuse, and runs all three storage backends. PRs use baseline Kubernetes on both
  architectures; weekly/default manual runs use all three reviewed versions.
- `.github/workflows/maintenance.yml`: fresh cargo-deny advisory checks; weekly
  and manual latest-stable checks on Linux/macOS run compatibility tests when
  stable differs from the pin. `.github/workflows/fuzz.yml` runs the three parser
  targets under pinned nightly/ASan, with longer weekly runs.

The composite actions cache dependency builds by compiler/target/profile and
exact-version tools, never successful test outcomes. Preserve separation between
coverage, faults, fuzz, native, and GNU compatibility caches, immutable external
action pins where used, and the safe fallback in change selection.

The workflow named Release prepares and tests artifacts; it does not publish
GitHub releases or submit a Krew index entry. `scripts/release.py` wraps version
preparation, tag/build/download, and a separate explicit publication command via
the Makefile. It requires clean committed source for build/publication, preserves
existing tags/directories, and verifies source/run/file identity before upload.
Its offline regressions run in `make check` and CI. Follow `docs/DISTRIBUTION.md` and
[docs/KREW_READINESS.md](docs/KREW_READINESS.md) for archive/manifest verification
and `make release-smoke`. Keep native execution evidence distinct from
cross-compilation/emulation and local installation distinct from published
download verification.
