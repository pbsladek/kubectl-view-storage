# Implementation plan

Status: Draft 0.9, 2026-11-11. This plan orders implementation of [SPEC.md](SPEC.md). The specification remains the behavioral contract; [DESIGN.md](DESIGN.md) owns product and architecture decisions. P0, P1, and P2 describe release scope, while numbered phases describe execution order.

The accepted [operator calculation extension](CALCULATIONS.md) spans phases 2–3 (consumer ownership, query scope, grouping and reporting) and phase 5 (comparison validation, exact arithmetic, exports, independent checks and live gates). Its implementation is complete locally; A59–A65 extend the release acceptance set. Publication remains phase 6, with the selected release revision requiring fresh native CI evidence.

The Rust executable/library, schema, contract scenarios and independent validator exist. Locked production tests, real HTTP/CSV exports, fuzz smoke and disposable inventory/CSI checks run successfully on the recorded platforms. The current acceptance audit still has open phase exits, including the final live negative telemetry gate and remaining native/CI distribution matrix. [Implementation evidence](IMPLEMENTATION_STATUS.md) records the current partial work and environment limits; the [acceptance audit](ACCEPTANCE_STATUS.md) tracks every A01–A65 case and phase exit. A phase is complete only after its exit evidence exists; the table below records planned work, not completed implementation.

## Phase overview

| Phase | Release scope | Deliverable | Dependency | Exit evidence |
| --- | --- | --- | --- | --- |
| 0 | P0 | Reproducible developer environment and cluster lifecycle | Existing contract | Pinned tools/manifests; inventory and CSI profiles create, verify, and delete cleanly |
| 1 | P0 | Typed Rust boundaries, exact quantities, graph, and CLI skeleton | 0 | Offline fixtures/property tests and coverage gates for implemented core modules; help/version require no cluster |
| 2 | P0 | First working inventory path | 1 | Namespace PVC and cluster PV inventory → normalized JSON → default tree on disposable cluster |
| 3 | P0 | Relationships, filters, grouping, and remaining outputs | 2 | Shared/terminating consumers, all views, wide details, CSV, and selection completeness verified |
| 4 | P0 | Kubelet usage and strict findings | 3 | Fresh CSI observations, exact thresholds, conflict exit policies, and missing telemetry behavior |
| 5 | P0 | Resilience and support regression suite | 4 | Failure cases, coverage/fuzzing evidence, request budgets, RBAC, and Kubernetes compatibility matrix |
| 6 | P0 | Packaging and first release | 5 | Four-platform archives, checksums, Cargo package, local Krew installation, and all P0 acceptance cases |
| 7 | P1 | Prometheus and refresh mode | 6 | Explicit provider identity/auth; identical accounting; cancellation and watch exit policies |
| 8 | P1 | Operational and storage context | 7 | Workload/events/attachments/resize, then quotas/snapshots/topology, each with evidence and permissions |
| 9 | P2 | Backend adapters and planning | 8 | Separate reviewed adapter contracts and live fixtures before each support claim |

## Phase 0 Developer environment

Use native Rust with Docker Engine and kind. On the current Apple Silicon Mac, use the installed Docker Desktop runtime and its desktop-linux context; Linux CI uses Docker Engine. The [development guide](DEVELOPMENT.md) defines pinned node images, separate inventory and CSI profiles, isolated kubeconfigs, and resource limits.

Deliver:

- Verify the checked-in Rust 1.99.1 pin against latest stable at execution time; use bundled Cargo, edition 2024, current-stable rust-version baseline, Cargo.lock, and optional mise aliases. Add pinned and current-stable CI checks plus a weekly stable/dependency update workflow.
- Select latest compatible maintained crates with minimal features and document each runtime dependency. Enforce the thirteen-direct-runtime budget and toolchain/pin consistency in local and CI checks. Establish per-target dependency/feature/build-size baselines and cargo-deny advisory checks from DEVELOPMENT.md. Keep test tools dev-only; do not claim a graph audit until Cargo.toml and its lockfile exist.
- Pinned cargo-llvm-cov plus matching llvm-tools-preview, LCOV/HTML/JSON artifact generation, and a core-module coverage checker. Keep any fuzz-only toolchain separate from the normal build/MSRV toolchain.
- Cluster create, fixture apply, readiness/telemetry checks, evidence capture, and named-cluster deletion tasks.
- A reviewed deployment of CSI hostpath v1.19.0 plus required snapshot dependencies, locked to manifest revisions and image digests.
- An environment lock recording the tool versions, node image digests, storage driver release, dependency images, and fixture workload images. The initial selections are in [dev/versions.json](../dev/versions.json); unresolved CSI image digests must be filled before the CSI setup gate passes.

Exit: the current stable Rust/Cargo baseline builds the locked manifest with reviewed maintenance/features and recorded dependency cost, and repeated cluster create → verify → delete cycles succeed without changing the user's normal kubeconfig. Run the same lifecycle on Linux CI and confirm arm64 availability for CSI dependencies. Installed tools and version selections alone do not constitute passing build or cluster evidence.

## Phase 1 Rust foundation

Create one Cargo package with library and thin executable. Implement private-field newtypes for units/IDs/thresholds, enums for validity/lifecycle/selection, and explicit transport → domain → report conversions. Use fallible constructors for invalid data while retaining valid sibling measurements. Define collection clock and provider seams for deterministic tests. Keep accounting pure and snapshots borrowed immutably; isolate exit decisions in the policy module.

Implement the shared arithmetic API and canonical ExactRatio, including equality/order consistency and overflow-safe cross-products. Define the Clock interface with independent monotonic and UTC readings, integrated with Tokio's test clock. Preserve open external status values and tolerate unknown input fields while keeping internal policy matching exhaustive.

Implement exact quantity parsing, u64 individual counters, checked u128 totals, exact decimal threshold comparisons, and overflow-safe aggregate ratios. Evaluate bounded std arithmetic first; justify any maintained helper by correctness, complexity, and dependency cost. Keep thresholds as decimal strings in serialized queries. Implement the shared fixture loader and dev-only proptest invariants before transport.

Start the quantity-parser fuzz target with valid/fractional/exponent/overflow seeds when that parser lands. Preserve minimized failures as deterministic regressions. Coverage gates activate for each implemented core module; future modules are recorded as absent, not assigned fictitious coverage.

Exit: fixtures validate against the schema, A22/A25/A41/A44/A52/A55 pass in Rust, and exact values on both sides of each threshold are classified correctly despite equal displayed percentages. Clock unit tests separate monotonic from UTC changes. Help, version, and invalid invocation paths make no requests. Implemented core modules meet 90% line coverage individually and implemented production source meets 80% overall; uncovered failure decisions require review even when those floors pass.

## Phase 2 Working inventory path

Implement kubeconfig/context/namespace resolution, required paginated base inventory, and bounded metadata collection. Support namespace PVC mode and explicit cluster PV mode. Build the query-dependency planner, completeness counters, and diagnostics before optimizing request count.

Connect inventory to JSON and the default tree. Use the inventory kind profile for Pending, Bound, Released, classless, and unreferenced objects. Retain unknown capacities under reduced permissions.

Exercise unknown extra fields and unfamiliar phase strings in mocked inventory. Preserve representable open values, diagnose unsupported dependent classifications, and verify strict/filter behavior without coercing a healthy state or losing unrelated inventory. Retain the fatal boundary for missing required identity and malformed required documents.

Exit: a user can run the real executable against the disposable cluster and obtain useful inventory without node proxy permission. Verify A01–A07, A14, A23, A31–A32, A46, and A58. This is the first usable implementation checkpoint; revise contract examples only when evidence justifies a change.

## Phase 3 Consumers and presentation

Add app/init/ephemeral container references, mounts/devices, active/terminating/terminal states, exact label/name/node/driver filters, reference-aware grouping, and deterministic sorting. Preserve actual pod-to-node edges when dimensions are reordered.

Finish table, summary, issues, --wide, and flat CSV. Keep the normalized JSON query independent of text view. Reuse all contract scenarios, including multiple mounts on one edge and shared claims across node branches.

Compose the record comparator from availability, the primary sort direction, and ascending identity ties. A55 must cover both sort directions, equivalent fractions, and randomized input permutations through the real grouping/rendering path.

Exit: A08–A10, A19, A24, A27, A29, A36–A37, and A45 pass. Root capacities remain invariant when consumers or grouping dimensions change. Terminating references are visible and remain eligible collection targets.

## Phase 4 Usage and findings

Implement Summary API collection, candidate attribution, field validation, exact freshness/range comparisons, and per-field findings. Add byte/inode observations, raw block n/a behavior, source timestamps, partial metrics, disagreement, and strict failure precedence.

One supervisor owns bounded workers and cancellation; workers return owned observations for deterministic reduction. Start a statistics-parser fuzz target when the parser lands, covering absent/wrong-type counters, truncated JSON, oversized values, and unknown fields. Assert malformed optional responses preserve inventory and valid siblings wherever parsing permits.

Enforce production byte/depth/scalar caps and cancellation checks around bounded decode steps. Normalization yields/checks at least every 64 objects/candidates. Add CPU-heavy decode/normalization tests and ensure any blocking offload cooperates and is joined. Exercise UTC jumps during active retries and use monotonic time for all budget/shutdown decisions.

Use CSI hostpath to prove real end-to-end volume statistics on one node. Use bounded file writes and compare reported observations before/after; do not fill the Docker VM filesystem. Use mocks for exact 80/90% boundaries, zero available/free fields, malformed responses, clock skew, and conflicting nodes.

Exit: A11–A18, A20–A21, A25–A26, A28, A33–A35, A42–A43, and A53 pass. Confirm that a fresh 90%/99% conflict cannot pass a critical check and that the driver emits the required fields before declaring this profile ready.

## Phase 5 Resilience and support

Use wiremock to verify exact HTTP paths, continuation tokens, retries, cancellation, response caps, query dependency priorities, and transport counters. Exercise a node error while another worker is running and while output is being consumed. Use a controlled clock for deadlines and budget tests.

Bound SDK credential-helper execution as well as async HTTP tasks. The reproduced synchronous-helper timeout and refresh-shutdown gaps now have owned-process supervision and executable/PTY regressions. Retain direct checks for process termination/reaping, terminal input contracts, sanitized failures, inherited flags and ordinary/strict partial-report exits on every native platform. A timeout around the SDK future alone cannot provide this ownership. Keep SDK credential parsing/authentication; avoid a second bespoke authentication implementation.

Add restricted service accounts for inventory-only, denied PV enrichment, and denied node proxy. Test current-namespace behavior, flags after the plugin name, label filtering, nonterminal output, missing metrics versus empty inventory, empty node names, and repeated PVC references. Follow the source-linked [df-pv lessons](DF_PV_LESSONS.md).

Run the pinned baseline and older-node matrix from [DEVELOPMENT.md](DEVELOPMENT.md). Complete all A01–A65 cases and property requirements, including clock-jump and CPU-cancellation tests, unknown-value handling, and request budgets on the 10,000-claim fixture. A skipped metrics test is not a successful metrics-support result. Record versioned evidence in [COMPATIBILITY.md](COMPATIBILITY.md).

Enforce at least 90% line coverage for each of qty, join, group, findings, policy, and calculations, and 80% for production source overall, using the cluster-free unit/property/mock/CLI suite. Publish LCOV, HTML, and JSON evidence. Direct tests must catch reversed comparisons, duplicate capacity sums, swallowed lookup errors, and incomplete worker cancellation; controlled fault/mutation checks demonstrate those tests fail for the intended reason. Run bounded fuzz smoke tests on parser changes and longer scheduled runs, retaining reproducible regressions.

Exit: full P0 behavior, coverage floors, required fault cases, and parser fuzz regression/smoke checks pass. No outstanding false-success, cancellation, accounting, unscoped-log, or known parser-crash regression remains. Record runtime and peak memory on a named runner before adopting performance targets.

## Phase 6 Distribution

Build Linux amd64/arm64 and macOS amd64/arm64 binaries. Verify authentication, help, version, archive contents, license inclusion, SHA-256 checksums, Cargo package installation, and local Krew manifest installation. Generate platform entries from actual artifacts.

Before release, recheck latest stable Rust/bundled Cargo and current maintained dependency versions, rerun the locked checks, and review graph/features/binary-size changes against the baseline. Resolve advisory/unmaintained findings or document a specific temporary exception and expiry; keep implementation support obligations explicit.

Publish installation, RBAC, missing-telemetry troubleshooting, and tested-driver documentation. Verify a clean install without a running test cluster.

Exit: A30, all platform checks, the phase 5 coverage floors, and required failure-case gates pass on the release revision. P0 is ready for an initial v0.1 release. Krew index submission and external publication are separate release actions.

## Phase 7 Prometheus and refresh

Implement the explicit Prometheus provider, cluster labels, credential files, sample timestamps, scrape deduplication, and lifecycle-aware attribution. Reuse the same report and severity rules. Then implement bounded sequential refresh, redirected output behavior, and strict exits.

Exit: provider fixtures and a dedicated disposable Prometheus deployment agree with the P0 accounting model. No implicit provider fallback or endpoint discovery occurs.

## Phase 8 Operational context

First add workload owner chains, events, attachments, and resize evidence. Then add ResourceQuota, VolumeSnapshot relationships, and CSIStorageCapacity/topology context.

Exit: each enrichment has its own capability/permission result, source-linked evidence, fixtures, and missing-API behavior. It cannot make inventory disappear or mix quota, snapshot, pool, and filesystem accounting.

## Phase 9 Backend integrations

Write separate adapter contracts for the backend selected for implementation. Specify identity mappings, logical/physical semantics, replication, pool scope, timestamps, permissions, and compatibility evidence before writing the adapter.

Exit: a dedicated disposable environment and recorded provider versions validate each supported adapter. Historical growth and cost estimates require explicit inputs and assumptions.
