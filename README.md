# kubectl-view-storage

A Rust kubectl plugin for inspecting Kubernetes persistent volumes, claims, consumers, and reported storage usage.

**Experimental:** final validation is in progress. No release or Krew index package has been published. See [project status](docs/IMPLEMENTATION_STATUS.md) and [tested compatibility](docs/COMPATIBILITY.md).

## Features

- PVC and PV inventory with pod, node, and mount relationships.
- Tree, table, summary, and issues views with configurable grouping.
- Filesystem and inode usage with timestamps, coverage, and findings.
- Separate requested, provisioned, and filesystem capacities; shared claims counted once.
- Useful inventory with limited permissions and explicit reasons for missing data.
- Deterministic JSON and CSV output.

## Build from source

Requires Rust/Cargo, kubectl, and a kubeconfig for cluster queries. The tested Rust version is pinned in [rust-toolchain.toml](rust-toolchain.toml).

```sh
cargo build --locked
export PATH="$PWD/target/debug:$PATH"
kubectl view-storage --help
```

See [development](docs/DEVELOPMENT.md) for disposable kind clusters and testing, and [distribution](docs/DISTRIBUTION.md) for Cargo and local Krew installation.

## Usage

```sh
kubectl view-storage -n view-storage-csi -u --view table
```

Captured from a real disposable CSI hostpath cluster:

```text
CONTEXT kind-view-storage-csi   SCOPE view-storage-csi   KIND pvc
PVC REQUESTED PROVISIONED FS-CAP USED AVAIL USE% INODE% STATE
data 1Gi 1Gi 58.37Gi 35.1Gi 20.27Gi 60.132122% 11.211557% Bound [metrics:ok]
SELECTION complete; matched=1 unknown=0
METRICS 1/1 known-match eligible claims fresh and complete
FILESYSTEM TOTALS per-claim reported filesystem totals; physical_total=false
```

By default, inventory is grouped by storage class → namespace → PVC. Usage collection requires `-u`; available metrics depend on the storage driver and permissions.

See [real command output in all views, JSON, and CSV](examples/README.md) and the [CLI specification](docs/SPEC.md).

## Documentation

The [documentation index](docs/README.md) covers design, development, compatibility, validation, and the implementation roadmap. Complete sample inputs, reports, and all four text views are in [examples/contract](examples/contract/README.md).

## Acknowledgments and license

Inspired by [kubectl-view-allocations](https://github.com/davidB/kubectl-view-allocations) for its CLI and Rust project structure, and [kubectl-df-pv](https://github.com/yashbhutwala/kubectl-df-pv) for the persistent-volume usage use case.

Licensed under [Apache-2.0](LICENSE).
