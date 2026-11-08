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
kubectl view-storage                       # PVCs in the current namespace
kubectl view-storage -A -u                 # All namespaces with live usage
kubectl view-storage -A -u --view issues   # Storage findings
kubectl view-storage -A -o json            # Structured report
```

By default, inventory is grouped by storage class → namespace → PVC. Usage collection requires `-u`; available metrics depend on the storage driver and permissions.

See [more commands and sample reports](examples/README.md) and the [CLI specification](docs/SPEC.md).

## Documentation

The [documentation index](docs/README.md) covers design, development, compatibility, validation, and the implementation roadmap. Complete sample inputs, reports, and all four text views are in [examples/contract](examples/contract/README.md).

## Acknowledgments and license

Inspired by [kubectl-view-allocations](https://github.com/davidB/kubectl-view-allocations) for its CLI and Rust project structure, and [kubectl-df-pv](https://github.com/yashbhutwala/kubectl-df-pv) for the persistent-volume usage use case.

Licensed under [Apache-2.0](LICENSE).
