# Local development and disposable clusters

Status: Draft 0.8, 2026-11-07. This guide defines the local and CI environment for the [implementation phases](IMPLEMENTATION_PLAN.md). Cluster configurations and version selections exist; automated workload/CSI installation and the Rust executable are phase deliverables.

## Selected tools

Use native Cargo builds, Docker, kind, kubectl, and versioned Kubernetes manifests. On this Apple Silicon Mac, use the already configured Docker Desktop engine with its desktop-linux context. Linux CI uses Docker Engine with the same kind profiles. The plugin runs on the host against an explicit test kubeconfig.

Use [wiremock](https://docs.rs/wiremock/latest/wiremock/) for HTTP failures, pagination, request ordering, and cancellation tests, and [proptest](https://docs.rs/proptest/latest/proptest/) for quantity, grouping, and identity invariants. Keep cargo test as the standard entry point; mise may add convenience aliases. Python and uv run the existing contract validator.

Use [kind](https://kind.sigs.k8s.io/docs/user/quick-start/) for disposable Kubernetes nodes. Select kind 0.33.0 and baseline Kubernetes 1.37.0, with 1.36.4/1.35.8 for compatibility runs. The [kind release](https://github.com/kubernetes-sigs/kind/releases/tag/v0.33.0) supplies the digest-pinned amd64/arm64 images in [dev/versions.json](../dev/versions.json). Use native host architecture.

The workstation has kind 0.33.0, kubectl 1.37.2, Rust/Cargo 1.99.1 and Docker Desktop 29.9.2. The production suite and inventory/CSI baseline profiles pass on macOS ARM64. Docker and registry commands work through reviewed execution outside the session sandbox. The runs used four Docker CPUs and 3.825 GiB assigned memory serially; this is observed capacity, not a verified minimum. Compatibility jobs must use kubectl matching each cluster minor.

## Current toolchain and lightweight dependencies

Use the latest stable Rust release and the Cargo shipped with that toolchain. On 2026-11-07, the [official release list](https://blog.rust-lang.org/releases/) identifies 1.99.1 as latest stable; [rust-toolchain.toml](../rust-toolchain.toml) pins it with a minimal profile plus rustfmt/Clippy. The current Homebrew compiler/Cargo already match; rustup-managed local/CI builds use the checked-in pin. Phase 0 records that compiler baseline in Cargo.toml rust-version and uses edition 2024. P0 targets the current stable baseline, not an additional older MSRV. Fuzz-only nightly does not become the normal build toolchain.

Recheck stable releases weekly and before publishing. A tested update changes the exact toolchain pin, rust-version baseline, environment record, matching coverage components, and lockfile when resolution changes. Run an additional current-stable CI check to detect a newer release while pinned builds remain reproducible. Compiler updates pass formatting, Clippy, tests, coverage, and advertised-target checks before becoming the release baseline. Record any necessary temporary delay and its resolution in the update PR.

Select latest mutually compatible stable crate versions and keep kube and k8s-openapi aligned. Commit Cargo.lock and use --locked for CI/release builds. Update compatible versions regularly and review major changes separately. A well-maintained stable crate need not release frequently: review upstream ownership/activity, issue/security response, changelog, archived/deprecation status, supported Rust, and transitive feature cost. Reject archived, deprecated, or advisably unmaintained candidates unless a specific temporary exception has an owner and expiry. Record source/version/features/reason for each direct runtime dependency. Use [cargo-deny advisories](https://embarkstudios.github.io/cargo-deny/checks/advisories/index.html) in CI to catch known vulnerabilities and unmaintained-crate advisories; automated checks supplement the upstream review.

The reviewed budget is thirteen normal direct runtime dependencies, including target-specific crates. Twelve cover the original CLI/client/accounting capabilities; the additional Unix-only nix dependency has the cancellation rationale below. Every addition or feature expansion records its purpose and transitive impact; exceeding the target requires a written design rationale. This is a reviewed budget, not permission to hand-roll TLS, Kubernetes authentication, CSV escaping, or incorrect arithmetic. Use std for ordinary collections, terminal detection, paths, and simple formatting. Test/fuzz/coverage/release tools stay in dev dependencies or external tool environments.

| Capability | Initial feature policy |
| --- | --- |
| clap | Explicit parsing/help/usage features; derive only when used; omit unused completion/suggestion capabilities |
| kube and k8s-openapi | Client/kubeconfig and documented auth/proxy needs, one Rustls backend, one compatible API-version selection; no controller runtime or custom-resource derives |
| tokio and tokio-util | Explicit runtime/task/sync/time/signal/cancellation requirements; no full feature bundle; test clock utilities stay test-only |
| serde and serde_json | Serialization actually used by transport/report types; arbitrary_precision preserves valid sibling fields beside oversized counters; unbounded_depth permits the decoder to honor our mandatory pre-parse inclusive 128-level limit rather than Serde's earlier cutoff |
| csv and thiserror | Justified format/error functionality with narrow features |
| jiff | Timestamp parsing/formatting without timezone databases; monotonic deadlines use Tokio's clock |
| http and http-body-util | Reuse the Kubernetes client's HTTP stack; stream response frames to enforce byte caps and count partial bodies |
| nix (Unix only) | Safe fs/process/signal/term/time APIs; macOS-only event observation; defaults/aio/network bundles disabled |
| Exact arithmetic helper | Justify a maintained helper against bounded std algorithms, complexity, correctness, and cost; preserve exact overflow-safe comparisons |
| Future integrations | Gate provider SDKs/heavy clients behind milestone-specific features; audit the explicit release feature set |

The thirteen selected runtime crates are clap, kube, k8s-openapi, tokio, tokio-util, serde, serde_json, thiserror, jiff, http, http-body-util, csv, and Unix-only nix. Their version requirements and exact enabled features live in Cargo.toml. There is no additional logging framework, table/color framework, date/timezone database, backend SDK, or bigint dependency in P0. Structured report diagnostics and std formatting cover the current requirements. The selected version/feature combination now resolves in Cargo.lock and passes the real native production build/tests. The [upstream maintenance review](DEPENDENCY_REVIEW.md) records the current release/owner/activity and issue/security-policy evidence. Remaining target builds and native CI evidence remain required.

The following upstream stable versions were checked on 2026-11-07. Cargo.toml uses these as compatible minimum requirements, allowing subsequent compatible fixes; Cargo.lock must record the actual tested resolution. The linked crate documentation establishes the published release and capability, not a completed security or maintenance audit. The linked [maintenance review](DEPENDENCY_REVIEW.md) records repository ownership/activity, recent closures and security-policy locations. Recheck it on dependency updates and before release.

| Runtime crate | Observed stable release | Reason to retain it |
| --- | --- | --- |
| [clap](https://docs.rs/clap/latest/clap/) | 4.6.7 | Typed CLI parsing, validation, and help |
| [kube](https://docs.rs/kube/latest/kube/) | 5.0.0 | Kubernetes client, kubeconfig/authentication, TLS and proxy support |
| [k8s-openapi](https://docs.rs/k8s-openapi/latest/k8s_openapi/) | 0.29.0 | Kubernetes API types compatible with the selected client |
| [tokio](https://docs.rs/tokio/latest/tokio/) | 1.54.1 | Shared async runtime, bounded jobs, timers and signals |
| [tokio-util](https://docs.rs/tokio-util/latest/tokio_util/) | 0.7.20 | Cancellation tokens shared across collection and CPU workers |
| [serde](https://docs.rs/serde/latest/serde/) | 1.0.229 | Typed transport serialization/deserialization |
| [serde_json](https://docs.rs/serde_json/latest/serde_json/) | 1.0.154 | JSON transport and report contract with exact numeric handling |
| [thiserror](https://docs.rs/thiserror/latest/thiserror/) | 2.0.21 | Explicit typed errors without an additional dynamic error framework |
| [jiff](https://docs.rs/jiff/latest/jiff/) | 0.2.40 | Timestamp parsing and formatting without timezone databases |
| [http](https://docs.rs/http/latest/http/) | 1.5.0 | Request/header types shared with the client stack |
| [http-body-util](https://docs.rs/http-body-util/latest/http_body_util/) | 0.1.5 | Frame streaming for bounded response bodies |
| [csv](https://docs.rs/csv/latest/csv/) | 1.4.0 | Correct escaping and CSV output |
| [nix](https://docs.rs/nix/0.31.3/nix/) (Unix) | 0.31.3 | Safe output flags, owned helper processes/signals, foreground terminal and shared monotonic deadlines |

The thirteenth runtime capability covers safe Unix output and credential-process ownership. Nix replaces the earlier Rustix selection while keeping one Unix API crate and the thirteen-package budget. The output worker enables nonblocking writes, checks cancellation, discards buffered bytes on failure and restores inherited file-description flags. Credential supervision also needs safe process/reaping, signal-mask, terminal and monotonic-clock operations. Enable fs/process/signal/term/time, with event restricted to macOS for non-reaping kqueue observation. Defaults, aio and networking stay disabled. See [the reviewed dependency decision](DEPENDENCY_REVIEW.md) and [process ownership design](DESIGN.md#credential-execution-ownership).

Current native/cross-build records and the initial cold-build checkpoints live under artifacts/dependency-baseline. Label warm source-correction builds explicitly. The actual SDK client has its default retry layer disabled so the collector alone owns retry counts and budgets. Exact 429/503/504 wire tests protect this boundary.

Run `python3 scripts/check-toolchain.py` and `python3 scripts/check-dependencies.py` locally; `make check` and CI include these checks. The toolchain checker verifies Rust, recorded bundled Cargo, edition, and pin alignment. The manifest checker counts unique direct runtime packages across ordinary and target-specific tables, excludes dev tools, enforces the thirteen-package budget, and rejects implicit default features, wildcards, prereleases, non-crates.io sources, and unused broad Tokio/kube capabilities. A new direct package requires updating the reviewed package set and recording its maintenance/cost review, even within the budget. Direct build dependencies and patch/replace source overrides require an explicit policy change; they cannot silently bypass source and build-cost review. It is a manifest guard, not proof of upstream maintenance or a resolved transitive audit. Review transitive build dependencies and unified features with the locked graph as well. Weekly Dependabot and cargo-deny jobs cover updates and advisories; the scheduled stable check compares both compiler and bundled Cargo with Rust's official stable manifest.

Inspect both normal/build dependencies and feature unification using [cargo tree](https://doc.rust-lang.org/cargo/commands/cargo-tree.html). Set default-features = false where the crate's defaults exceed requirements, then explicitly select supported functionality; test kubeconfig helpers, TLS, and proxy behavior so trimming does not break documented auth. Reuse one compatible async/TLS/HTTP stack where practical. Review duplicate versions and remove unused direct crates rather than accumulating parallel helpers.

Once Cargo.toml exists, retain a per-target release dependency baseline: direct and transitive crate/version counts, enabled features, duplicate versions, clean build time, and stripped binary size. Dependency-changing PRs attach the graph delta and explain meaningful growth; tests/build evidence is required before merging updates. Do not set a fabricated total transitive-count ceiling before measuring the Kubernetes client graph. Cargo.lock now resolves 216 package versions after the Jiff 0.2.40 update; the initial checkpoint resolved 219. The initial pre-supervision checkpoint compiled 134 package IDs and produced a 7,691,472-byte ARM64 binary. Current native ARM64 and cross-built Intel macOS records include the reviewed Nix supervision feature set and source/lock hashes; use their measured values rather than treating the initial checkpoint as the final build. Compiled-unit features and lock hashes are under ignored artifacts/dependency-baseline. cargo-deny advisories/bans/licenses/sources pass with the reviewed package/version-scoped LLVM exception in deny.toml. Duplicate-version warnings remain explicit review work. Upstream metadata/retention review is recorded in DEPENDENCY_REVIEW.md; other target cost and runtime evidence remain separate gates.

## Test layers

| Layer | Environment | Purpose | Run policy |
| --- | --- | --- | --- |
| Contract validation | Python, no cluster | Schema, fixtures, references, exact thresholds, expected views | Every contract change |
| Rust unit/property | Native Cargo, no cluster | Quantities, lifecycle, selection, ratios, grouping, findings | Every implementation change |
| Mock HTTP | wiremock on loopback | Paths, pagination, errors, budgets, cancellation, credential boundaries | Every implementation change |
| Inventory integration | Three-node kind, default local provisioner | Real joins, nodes, labels, pending/unreferenced volumes, RBAC | Relevant PRs and release gate |
| CSI telemetry integration | Separate single-node kind, CSI hostpath | Real kubelet byte/inode observations, mounts, same-node sharing | Linux CI gate; targeted Mac runs |
| Provider integration | Dedicated optional profiles | Prometheus, real cross-node file storage, future backends | Corresponding milestones |

Mocks own deterministic thresholds, disagreement, corrupted samples, missing fields, and exact failure cases. Live tests prove collection through real APIs.

CI exports actual collector/finalizer reports with `VIEW_STORAGE_TEST_REPORTS` and runs `scripts/validate-produced-reports.py`. The verifier checks the nine contract scenarios plus six shared-CSI-volume, classless/unassigned, and unfamiliar-classification regressions with the independent schema/arithmetic/relationship oracle. The extra cases verify additive exclusion, individual required metrics/pressure findings, assignment states, and usable independent observations when lifecycle is unknown. Negative checks reject incorrect sums, false-success exits, collapsed assignment states, coerced classifications, false consumer counts/activity, and noncanonical diagnostic/affected-field order. Original expected reports remain independent examples; canonical field ordering follows the specification without regenerating their quantities or rules from implementation output.

## Rust quality and coverage

Use rustfmt and Clippy in CI, with warnings treated as errors on the pinned development toolchain. Review fallible conversions and exhaustive state handling; external input paths must not rely on unwrap/expect, unchecked indexing, or lossy casts. Test the committed resolution on the pinned baseline and the current-stable update check. Compiler/lint checks supplement behavioral tests.

Use [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) and the matching llvm-tools-preview component. Phase 0 pins their versions and adds the coverage job. Measure the same production feature set as the release binary, using the unit, property, mock HTTP, and CLI tests without a disposable cluster. This makes the coverage gate reproducible on CI without driver availability.

| Scope | Required line coverage | Measurement |
| --- | --- | --- |
| Each core module: qty, join, group, findings, policy | At least 90% individually | Sum covered/instrumented production lines within that module and its descendants |
| All production source | At least 80% | Sum covered/instrumented lines across application/library source |
| Required failure and boundary cases | Every specified case passes | Assertions of values, diagnostics, exits, and task cleanup, independently of line coverage |

Percentages are weighted by instrumented line counts, not averaged across files. Include renderers, provider code, error paths, and the CLI in the overall denominator. Dependencies, test/fuzz harnesses, and generated code are outside production-source coverage; record generated-code exclusions explicitly. Keep any other uncovered source visible in reports and close required behavior gaps even if thresholds pass.

Publish LCOV for machine consumption, HTML with uncovered lines for review, and LLVM JSON for a per-module gate. The overall command's threshold does not enforce the separate core-module floors: phase 0 adds a checker over the JSON file/path counts and fails when an implemented required module is omitted. Before Rust source exists, coverage is unmeasured. During incremental phases, check implemented modules; all five core modules are required at phase 5.

Once the Cargo package and coverage tooling exist, the standard collection/report workflow is:

    mkdir -p artifacts/coverage
    cargo llvm-cov --workspace --lcov --output-path artifacts/coverage/lcov.info --fail-under-lines 80
    cargo llvm-cov report --html --output-dir artifacts/coverage/html
    cargo llvm-cov report --json --output-path artifacts/coverage/llvm.json

Use one consistent test build/profile for these artifacts. The CI job also runs the core-module JSON checker and propagates test failures; report generation cannot turn failed tests into a passing gate. The [phase plan](IMPLEMENTATION_PLAN.md) assigns implementation and enforcement of this workflow.

## Property and fuzz testing

Proptest exercises quantity boundaries, consumer/grouping invariants, unknown selection, exact thresholds, and permutation-independent output. Add equality/ordering laws for ExactRatio, overflow-sized cross-products, unknown-last sorting in both directions, and ascending identity ties. Keep example inputs as fixed regressions; test rule meaning separately from snapshots. Targeted fault/mutation checks must detect reversed thresholds, double counting, swallowed lookup errors, and failed cancellation.

Use an injected Clock whose monotonic and UTC readings can move independently. Paused Tokio time controls budgets/retry/shutdown; explicit wall-clock jumps test freshness without extending request allowances. Cancellation tests must cover CPU decoding/normalization and queued work as well as a sleeping HTTP future. Check response/depth/scalar limits and cooperative processing checkpoints, and assert no blocking work or late request survives shutdown. Run unknown-field and unfamiliar-phase cases through transport, domain validation, report serialization, and dependent filter/strict policy.

Add [cargo-fuzz](https://rust-fuzz.github.io/book/cargo-fuzz.html) targets as the Rust quantity and statistics parsers land. Use a separately pinned fuzz toolchain; the normal build/MSRV remains stable. Seed from sanitized valid/invalid contract inputs, cap input sizes and per-run time, run a short smoke budget on parser changes, and run longer scheduled CI sessions. Targets assert no panic/hang, bounded resource use, and either validated values or explicit errors/diagnostics. Where an input decodes into independently valid fields, assert those siblings remain usable.

Minimize failing inputs and retain them as deterministic Rust regression tests. Quantity fuzzing includes suffixes, exponents, fractional bytes, and overflow; statistics fuzzing includes missing fields, wrong numeric types, malformed/truncated JSON, and unknown fields. Fuzzing is measured separately from the release line-coverage gate. The current Python contract validator does not constitute Rust parser fuzzing or Rust code coverage.

## Inventory profile

Use [dev/kind/inventory.yaml](../dev/kind/inventory.yaml): one control plane and two workers. Keep kind's default local provisioner to exercise inventory when telemetry is unavailable. Specify fixture storage classes explicitly.

Phase 0 adds fixtures for Bound/Pending/classless claims, an unmatched storage class, Available/Released PVs, unreferenced claims, unscheduled and terminating pods, shared consumer declarations, and distinct claim/pod labels. Add namespace-only, denied-PV, and denied-proxy credentials.

The checked-in inventory/cases/RBAC manifests now cover these cases. Static Retain volumes provide Available/Released and bound-unreferenced objects; the local provisioner supplies the mounted data claim. A repeated declaration shares that claim, and an unmatched node selector keeps one consumer unscheduled. A long kubelet [preStop sleep hook](https://kubernetes.io/docs/concepts/containers/container-lifecycle-hooks/) holds the terminating consumer during the short verification run; deleting the owned kind cluster removes the fixture. Inventory workload images are now digest-pinned, and two complete live create/verify/delete cycles pass on macOS ARM64. Linux and older-version runs remain required before the full reproducibility gate passes.

`scripts/dev-cluster.py create inventory` applies the workloads and RBAC, waits for readiness, prepares termination, and runs real plugin queries. Build the plugin first; `verify inventory --binary /path/to/kubectl-view_storage` selects another real artifact. Verification checks default namespace, all base claims under reduced privileges, label selection independent of pod labels, shared/unscheduled/terminating consumers, Available/Released PVs, and denied node-proxy behavior with exits 0/4. Fixture presence alone cannot pass. Failed verification replaces stale passing evidence; successful evidence records the binary SHA-256.

The inventory, namespace-only, denied-PV and denied-proxy accounts receive short-lived TokenRequest credentials in separate mode-0600 kubeconfigs under the owned run directory. Only the selected cluster/CA is copied from the administrator config; its user keys are not copied. Credentials never enter report artifacts and named cleanup removes the tracked files. The offline `scripts/test-dev-environment.py` checks permissions, token-config isolation, filesystem modes, cleanup bounds, and failure evidence; it does not establish passing cluster integration.

If this provisioner returns metrics on a particular version, validate them. Missing metrics are a capability outcome, not a reason to drop records. Driver and provisioner versions determine the result; a cluster product name alone does not.

## CSI telemetry profile

Use [dev/kind/csi.yaml](../dev/kind/csi.yaml): one control-plane node, with the [CSI hostpath driver](https://github.com/kubernetes-csi/csi-driver-host-path) release [v1.19.0](https://github.com/kubernetes-csi/csi-driver-host-path/releases/tag/v1.19.0).

Phase 0 reviews and vendors its deployment and required snapshot CRDs/controller, records exact source revisions and multi-platform image digests in [dev/csi/lock.json](../dev/csi/README.md), and pins the completed lock file SHA-256 in [dev/versions.json](../dev/versions.json). The reviewed lock and checksum now exist. The [CSI lock guide](../dev/csi/README.md) defines required resources, licenses, deployment ordering and workload image selection. Do not apply a moving master branch or fetch an unreviewed latest deployment during tests.

The upstream driver is a single-node demo for testing and implements [NodeGetVolumeStats](https://github.com/kubernetes-csi/csi-driver-host-path/blob/master/pkg/hostpath/nodeserver.go). This profile validates API/CSI/kubelet integration, not production-backend behavior or cross-node RWX.

Filesystem statistics can describe backing storage rather than a quota equal to the PVC request. Do not assume a 1Gi claim creates a dedicated 1Gi filesystem. Write at most 32 MiB per test, wait for a fresh observation, and check valid fields and compatible usage changes. Do not fill the Docker VM disk to trigger pressure. Exact byte/inode exhaustion stays mocked.

Readiness requires a bound/mounted filesystem claim and a fresh attributable byte tuple from the node Summary API; record inode coverage separately. Wait for observation timestamps to advance after writes. A missing required tuple fails the CSI gate; it is not an automatic skip or success.

Raw block live tests require driver/node support and must still report filesystem metrics n/a. Same-node multiple consumers validate deduplication; actual cross-node RWX remains in mocks until a dedicated file-storage profile exists.

## Cluster lifecycle

Generated kubeconfigs live under ignored .local/kubeconfigs. Use the owned harness so creation, verification and cleanup share the same run record:

    cargo build --locked
    python3 scripts/dev-cluster.py create inventory
    kubectl --kubeconfig .local/kubeconfigs/inventory.yaml get nodes
    python3 scripts/dev-cluster.py verify inventory
    python3 scripts/dev-cluster.py delete inventory

The CSI entrypoints are `python3 scripts/dev-cluster.py create csi`, `verify csi`, and `delete csi`; `--binary` can select the real built plugin. Creation refuses an unresolved manifest/image lock. Workload, controller readiness, bounded-write/fresh-observation and failure-evidence automation exists, with seventeen offline harness checks. The real single-node byte/inode telemetry and shared-write gate passes on macOS ARM64 across all selected Kubernetes versions (1.35.8, 1.36.4 and 1.37.0), with repeated baseline lifecycles. Native Linux CI and other-provider evidence remain required. A manually created base cluster does not establish a passing CSI profile.

Cleanup validates the recorded container identity before deleting a live kind profile. A failed create that left no cluster or control-plane container can remove its own local state/configuration without deleting any cluster. Existing unowned kubeconfig files and replacement containers are not adopted.

The harness records ownership/run ID, confirms the generated context, and passes explicit kubeconfig on every kubectl/plugin operation. It must not change the user's current context, use Docker Desktop's built-in cluster, delete unnamed/unowned clusters, or clean up an arbitrary kubeconfig.

Start with 4 Docker CPUs and 8 GiB memory, plus disk for pinned images. Run one profile at a time on a smaller workstation. Measure these initial allocations in phase 0; they are not verified minimums.

## Evidence and CI

On failure, capture the environment lock, cluster/node/driver versions, fixture identities, redacted reports, events, and collection counters under ignored artifacts/run-ID. Exclude credentials and unfiltered Summary API bodies. Workers share cancellation and exit within the bounded shutdown period.

The required CSI gate runs on a Linux Docker runner. A developer machine may report a concrete preflight failure, which cannot count as a passing release result. Run the baseline first, then inventory/compatibility jobs on the older images. Record provider-specific outcomes in [COMPATIBILITY.md](COMPATIBILITY.md).

The contract check needs no container runtime:

    uv run scripts/validate-contract.py

The [distribution guide](DISTRIBUTION.md) defines target builds, deterministic archive/manifest verification and make release-smoke with an isolated Krew root. Twelve packaging checks include a real Cargo file-selection regression; the other archive/subprocess cases use synthetic fixtures. The native macOS ARM64 release/archive exists. Remaining target archives and actual installation evidence are still required.

The Makefile exposes check/test/coverage/plugin-smoke and profile-specific create/verify/delete tasks. Cargo builds `target/debug/kubectl-view_storage`; `make plugin-smoke` checks real kubectl discovery and cluster-free help/version/flag forwarding. The production HTTP suite separately verifies configured context/namespace resolution. Locked Cargo checks use the resolved Cargo.lock. Real inventory/RBAC and locked CSI telemetry runs pass on the native Mac. The older-node/Linux compatibility matrix, fuzz smoke and remaining installation gates remain required work. Workspace-local coverage/advisory tools live in .local/tools/bin; add that directory to PATH when using them. The Homebrew Rust installation uses compatible LLVM 23 paths through LLVM_COV and LLVM_PROFDATA for coverage. See [implementation evidence](IMPLEMENTATION_STATUS.md) for the current scope. SPEC.md governs behavior and request budgets.

## Reproducing the current compatibility checks

The harness accepts `--node-image` on create/preflight only, restricted to the reviewed digest pins in dev/versions.json. Verification uses the selected image recorded in owned state and requires its matching kubectl on PATH: baseline 1.37 uses recorded kubectl 1.37.2; older images use clients 1.36.4 and 1.35.8. Download older official clients into workspace-local directories and verify their published checksums; do not replace the user's normal kubectl. For example:

```sh
PATH="$PWD/.local/toolchains/kubernetes-1.36.4:$PATH" python3 scripts/dev-cluster.py create inventory --node-image 'kindest/node:v1.36.4@sha256:099e049362a1526b2db71494e1947aae99bd16290d7c895f2b7ea312e3cbfaed'
PATH="$PWD/.local/toolchains/kubernetes-1.36.4:$PATH" python3 scripts/dev-cluster.py delete inventory
```

The Linux compatibility workflow runs inventory and CSI serially on native amd64/arm64 runners for each selected version, uses a matching client, and removes owned profiles even on failure. A checked-in workflow is not evidence that CI has passed. Local macOS results are recorded separately from Linux execution.

`python3 scripts/check-linux-container.py --arch arm64` runs actual locked Rust tests and a static musl release build inside the digest-pinned official Rust image recorded in dev/versions.json. `--arch amd64` selects that actual platform; Docker Desktop may emulate it, so record the host and avoid claiming native amd64 certification. The source bind is read-only, writable build/registry/evidence directories stay in this workspace, and a named/labeled container is removed on completion. The command limits build jobs, CPUs and memory; run it separately from kind profiles on the current Docker VM. It supplements the native CI matrix.

The quantity/statistics fuzz targets pass actual AddressSanitizer smoke on pinned nightly-2026-11-01 with cargo-fuzz 0.13.2. `scripts/fuzz-smoke.sh` uses a separate checked-in fuzz/Cargo.lock, locked Cargo prefetch, offline fuzz builds and an unchanged-lock check because cargo-fuzz has no `--locked` flag. Generated inputs go under ignored artifacts/fuzz-corpus; named checked-in seeds remain unchanged. The workspace-local Rustup/Cargo homes do not change the Homebrew production Rust/Cargo 1.99.1 bundle. Bounded smoke is not exhaustive fuzz coverage.


## CI performance

[CI performance](CI_PERFORMANCE.md) records measured build/test and runner costs. CI now uses compiler/target/profile-specific dependency caches, exact-version tool caches, a cumulative change selector, parallel lint/coverage/fault/native jobs, and once-per-architecture compatibility binaries. Source compiler verification, native Cargo/Krew installations, coverage floors, parser fuzzing, and actual CSI gates remain required in their selected workflows. Pure documentation changes skip native compilation; unknown paths/history run full checks. Use the final `required` aggregate as the branch-protection check when configuring protection. Local Docker CPU/memory limits are unchanged.

## Shared storage E2E suite

See [disposable storage tests](STORAGE_E2E.md) for `python3 scripts/e2e-storage.py`, backend coverage, CI scheduling, cleanup and NFS cache limitations. The suite runs the same three backend gates locally and on GitHub Linux runners.
