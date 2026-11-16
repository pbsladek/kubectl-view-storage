# kubectl-view-storage

A Rust kubectl plugin for inspecting Kubernetes persistent volumes, claims, consumers, and reported storage usage.

**Experimental:** see [project status](docs/IMPLEMENTATION_STATUS.md) and [tested compatibility](docs/COMPATIBILITY.md) for validation scope. Krew index registration remains pending.

## Features

- PVC and PV inventory with pod, node, and mount relationships.
- Tree, table, summary, issues and stats views with configurable grouping.
- Filesystem and inode usage with timestamps, coverage, and findings.
- Separate requested, provisioned, and filesystem capacities; shared claims counted once.
- Useful inventory with limited permissions and explicit reasons for missing data.
- Compact, newline-terminated JSON and deterministic CSV output.
- Consistent decimal/binary text units, collection costs in wide output, and watch timing.
- Storage sums and statistics by class, volume/access mode and StatefulSet; observed growth from saved reports.
- Sequential watch mode with automatic growth history and newline-delimited JSON.
- Explicit Prometheus telemetry with separate authentication and preserved sample timestamps.
- Verified Deployment, DaemonSet, StatefulSet, Job and CronJob ownership, filtering and grouping.

## Build from source

Tagged binaries for Linux/macOS AMD64 and ARM64, `SHA256SUMS`, and the generated
Krew manifest are distributed through [GitHub Releases](https://github.com/pbsladek/kubectl-view-storage/releases).
Verify the downloaded archive's checksum, extract it, and put `kubectl-view_storage`
on PATH. The command is `kubectl view-storage`.

Requires Rust/Cargo, kubectl, and a kubeconfig for cluster queries. The tested Rust version is pinned in [rust-toolchain.toml](rust-toolchain.toml).

```sh
cargo build --locked
export PATH="$PWD/target/debug:$PATH"
kubectl view-storage --help
```

See [development](docs/DEVELOPMENT.md) for disposable kind clusters and testing, [distribution](docs/DISTRIBUTION.md) for Cargo and local Krew installation, and [Krew readiness](docs/KREW_READINESS.md) for the release checklist.

## Usage

```sh
kubectl view-storage -n view-storage-csi -u --view table
```

Captured from a real disposable CSI hostpath cluster:

```text
Persistent volume claims
Context: kind-view-storage-csi  |  Namespace: view-storage-csi

PVC   PHASE  PODS  REQUESTED  PROVISIONED   FS SIZE      USED  AVAILABLE   USE%  INODE%  FS SAMPLE
----  -----  ----  ---------  -----------  --------  --------  ---------  -----  ------  ---------
data  Bound     2      1 GiB        1 GiB  58.4 GiB  35.1 GiB   20.3 GiB  60.1%   11.2%  Fresh

Selection: complete (1 matched)
Metrics: 1/1 claims fresh and complete; inodes: 1/1
Latest sample: 2026-11-08T02:56:43Z
Note: filesystem size exceeds provisioned size on 1 volume(s); the driver may report backing storage.
Filesystem totals are per claim, not physical storage totals.
```

By default, inventory is grouped by storage class → namespace → PVC. Usage collection requires `-u`; available metrics depend on the storage driver and permissions.

See [real command output in all views, JSON, and CSV](examples/README.md) and the [CLI specification](docs/SPEC.md).

Choose units in any text view, for example `kubectl view-storage -A --view table --units GB`. Add `--wide` to see request attempts, bytes read, collection duration, and confirmed enrichment-budget exhaustion. When usage is unavailable, the footer suggests checks based on the reported reason. JSON/CSV retain their exact counters.

For totals and size statistics, use `kubectl view-storage -A --view stats --units GB`. See [calculations](docs/CALCULATIONS.md) for workload scope, units and fill-rate comparisons.

For continuous observation, use `kubectl view-storage -n database --view stats --watch 10s`. Select a workload with `--workload deployment/api`, or group all visible claims with `-A -g workload`. See [watch, Prometheus and workload attribution](docs/OBSERVATION.md) for provider configuration, permissions and examples.

For local build provenance, run `kubectl view-storage --build-info`. It prints the
version, Git revision when available, compiler, and report schema without loading
kubeconfig. `--version` retains its concise output.

Generate shell completions with `kubectl-view_storage --completions bash`, `zsh`,
or `fish`. See [completion setup](docs/COMPLETIONS.md).

Run `make verify` for cluster-free checks plus independent validation of freshly
produced reports. It needs the pinned Rust toolchain, Python 3.12+, and uv.

Maintainers can prepare and validate a release with `make release-prepare VERSION=0.1.0`,
`make verify` and `make coverage`. After committing the reviewed inputs,
`make release VERSION=0.1.0` tags/builds and downloads the verified artifacts;
`make release-publish VERSION=0.1.0` publishes them. See `make release-help` and the
[release guide](docs/DISTRIBUTION.md#release-through-make).

## Documentation

The [documentation index](docs/README.md) covers design, development, compatibility, validation, and the implementation roadmap. Complete sample inputs, reports, and all four text views are in [examples/contract](examples/contract/README.md).

## Acknowledgments and license

Inspired by [kubectl-view-allocations](https://github.com/davidB/kubectl-view-allocations) for its CLI and Rust project structure, and [kubectl-df-pv](https://github.com/yashbhutwala/kubectl-df-pv) for the persistent-volume usage use case.

Licensed under [Apache-2.0](LICENSE).
