# Documentation

Start with [usage examples](../examples/README.md) or `kubectl view-storage --help`.

## Design and behavior

- [Design](DESIGN.md): features, architecture, Rust strategies, and recorded decisions.
- [Specification](SPEC.md): CLI, accounting, report semantics, exit codes, and acceptance cases.
- [Report schema](schema/report.schema.json): the machine-readable JSON contract.
- [Real command output](../examples/live/README.md): captured text views, JSON, CSV, and strict checks from disposable clusters.
- [Contract scenarios](../examples/contract/README.md): sample inputs, complete reports, and text views.
- [Lessons from df-pv](DF_PV_LESSONS.md): implementation and support issues addressed by this project.

## Development and installation

- [Development](DEVELOPMENT.md): toolchain, dependency policy, disposable clusters, tests, coverage, and fuzzing.
- [CI performance](CI_PERFORMANCE.md): measured runner/build/test costs, caching, parallel checks, and repeatable comparisons.
- [Dependency review](DEPENDENCY_REVIEW.md): runtime dependency choices, maintenance, features, and costs.
- [Distribution](DISTRIBUTION.md): source installation, release archives, checksums, and local Krew validation.

- [Disposable storage E2E tests](STORAGE_E2E.md): shared local/CI fixtures for local-path, CSI hostpath and cross-node NFS CSI.

## Progress and support

- [Implementation plan](IMPLEMENTATION_PLAN.md): phases and their exit requirements.
- [Implementation evidence](IMPLEMENTATION_STATUS.md): completed checks and remaining work.
- [Acceptance audit](ACCEPTANCE_STATUS.md): evidence and open verification for A01–A58.
- [Compatibility](COMPATIBILITY.md): tested environments and separately recorded storage capabilities.
