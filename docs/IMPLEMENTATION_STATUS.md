# Implementation evidence

Updated 2026-11-10. The phases 0–5 objective remains in final validation. The executable and library are implemented; remaining work is acceptance reconciliation, current-source installation/live checks and native CI. The [acceptance audit](ACCEPTANCE_STATUS.md) retains A01–A58 and every [phase exit](IMPLEMENTATION_PLAN.md). No release has been published.

| Phase | Evidence available | Remaining exit work |
| --- | --- | --- |
| 0 | Rust/Cargo 1.99.1; thirteen-runtime-crate guard; upstream maintenance/features review; resolved cargo-deny checks; genuine four-target builds; repeated inventory/CSI lifecycles on Kubernetes 1.35–1.37 | Final per-target provenance and acceptance audit |
| 1 | Typed boundaries, exact arithmetic, eleven passing randomized laws, full production coverage floors and both parser fuzz smoke targets | Remaining native platform checks |
| 2 | Real SDK/HTTP paths, pagination, fatal base failures, strict selection and live TLS/service-account inventory/RBAC | Per-case current-source acceptance reconciliation |
| 3 | Four views, 36 goldens, JSON/CSV, independent reports, configured kubectl discovery and pipe/terminal handling | Complete per-case audit and native installation matrix |
| 4 | Real CSI byte/inode observations and shared 16 MiB write; HTTP pressure/conflict/partial/Block/cap cases | Final negative-live-telemetry acceptance reconciliation |
| 5 | Locked tests, coverage, controlled faults, scale fixtures, fuzz smoke, owned credential supervision and local Krew installation | Remaining A01–A58 scopes, final source-package install and native CI |

## Current storage compatibility evidence

The [shared local/CI storage suite](STORAGE_E2E.md) now passes local-path, CSI hostpath and cross-node NFS CSI on native GitHub Linux amd64/arm64 across Kubernetes 1.35.8, 1.36.4 and 1.37.0. All eighteen hosted backend gates and owned cleanup passed, with exact binary/source reuse checked. [The receipt index](storage-e2e-hosted.json) preserves all six cells. Equivalent local gates pass on the Mac, including complete oldest/previous-minor suites. The earlier phase table's native Linux storage-lifecycle items are therefore satisfied; broader acceptance and release work remains separate.

## Locked production checks

The human terminal presentation now uses aligned columns, explicit units, short percentages, per-family validity, readable findings, and plain-language collection notes. The latest local coverage and test counts below include that change; earlier platform/build receipts retain their original source scope. No runtime dependency was added.

Native macOS ARM64 passes 171 Rust tests: 126 library, six executable-module, eleven randomized Proptest laws, two CLI and 26 real HTTP/executable tests. Formatting, Clippy with warnings denied, toolchain/dependency guards and Python harness checks pass. At the earlier presentation checkpoint, ARM64 Linux passes 169 tests; AMD64 Linux passes 168 under ARM-hosted user-mode emulation, before one test-only addition. Both Linux runs include actual terminal-helper checks and release musl builds. Emulation is not native AMD64 certification.

All nine contract scenarios run through the real SDK, HTTP server and binary in JSON, CSV and four text views. Their nine captured HTTP reports pass the independent verifier, as do nineteen reports from the compiled collector/finalization pipeline. Original examples/schema, 36 text goldens and seven semantic rejection checks also pass. Real HTTP text checks cover semantic fields and ordering; they do not claim byte-for-byte identity with older illustrative diagnostic prose.

Executable regressions cover explicit namespace `cluster` versus `-A`, forbidden base/page-two inventory, distinct logical/filesystem capacities, Block n/a semantics, denied-pod selection, byte/inode zero versus missing/invalid fields, conflicting candidates, mixed-node timeout/success, oversized success/error bodies and unfamiliar PV phase/mode values. Actual wire requests verify retry and consumed-byte accounting.

Three reproduced production defects are fixed: discarded error bodies now count toward consumed bytes; an oversized HTTP 401 stays fatal before body consumption; and kube's automatic retry layer is disabled so the collector owns and reports each retry. The 300-claim HTTP budget regression verifies one denied PV list, 254 targeted reads and the shared 256-attempt ceiling.

Credential execution has an owned process supervisor. Actual helpers exercise startup/refresh deadlines, stubborn descendants, Ctrl-C, sanitized failures and cooperative partial reports. The parent restores descriptor flags and terminal foreground ownership and reaps children. Three real PTY cases pass on macOS and both Linux containers. JSON/CSV/text broken-pipe tests run with direct and supervised credentials. The design records platform-specific process observation rules.

## Full production coverage

cargo-llvm-cov 0.9.1 with LLVM 23 instruments every production module, including main, the SDK adapter, CSV and credential supervision. The locked suite meets the 80% overall and 90% core floors:

| Scope | Covered lines |
| --- | --- |
| All production source | 6097/6504, 93.74% |
| qty | 285/295, 96.61% |
| join | 636/654, 97.25% |
| group | 403/411, 98.05% |
| findings | 364/378, 96.30% |
| policy | 82/88, 93.18% |

Ignored `artifacts/coverage` contains LCOV, HTML and JSON. Controlled fault checks and both 10,000-claim scale fixtures passed through the real locked package at their recorded source checkpoint. Scale measurements describe an in-memory debug-test workload, not network throughput or release ceilings. Parser fuzz smoke uses separate workspace-local nightly tools and AddressSanitizer; bounded runs are not exhaustive parser-safety evidence. Historical receipts retain their own source and lock hashes.

## Dependencies and target builds

Cargo.lock contains 216 package entries. Thirteen direct runtime dependencies remain the enforced budget. Nix 0.31.3 supplies safe process/signal/terminal/monotonic APIs, with event support scoped to macOS; Jiff is 0.2.40. [The maintenance review](DEPENDENCY_REVIEW.md) records upstream ownership/activity, features, security-policy evidence and costs. cargo-deny 0.19.0 passes advisories, bans, licenses and sources; duplicate-version warnings remain documented. The reviewed ar_archive_writer license exception is package/version scoped.

Four genuine release binaries and deterministic binary/LICENSE archives exist under ignored `artifacts/release-final`; the complete checksummed manifest passes verification. Earlier packaging-checkpoint production-source fingerprint: `66610aed31f91c837769e3eb9b98020a6daff6aec0073acc4e534db321c9ba4b`.

| Target | Actual evidence | Runtime scope |
| --- | --- | --- |
| macOS ARM64 | 135 compiled package IDs; 7,775,248-byte release; current warm rebuild 46.76 s | Native tests, configured plugin, terminal helpers, isolated Krew installation |
| macOS AMD64 | 136 compiled package IDs; 8,511,984-byte release; current warm cross-build 57.43 s | Local baseline cross-build; native Intel tests and installation subsequently passed in CI at `0695641` |
| Linux ARM64 musl | Full locked tests, terminal helpers and static release build | Native ARM64 container on Docker Desktop ARM64 VM |
| Linux AMD64 musl | Locked tests, terminal helpers and static release build | AMD64 emulation on the same ARM64 VM |

These times are local warm-cache measurements; separate initial-build receipts preserve their cache conditions. Native four-target distribution CI subsequently passed at `0695641`, including actual source installation and installed Krew checks. Local warm-build timings above remain separate from hosted measurements.

## Disposable clusters

Digest-pinned kind 0.33.0 inventory profiles pass administrator and short-lived restricted-account queries: default namespace, independent claim/consumer labels, shared/unscheduled/held-terminating pods, Available/Released PVs and denied PV/class/proxy with exits 0/4. Baseline lifecycle run IDs are `76a3a328-3132-455b-8380-1ef7a06ae35c` and `71e83224-61f9-4287-b32b-00f5c75ff1a3`.

Inventory also passes Kubernetes 1.36.4 (`9106ba01-1c74-4ec1-a2f2-e06ad4b3fa78`) and 1.35.8 (`9da3d1f9-fa19-4a2e-aafc-60bd24c0a9f7`) using separate matching kubectl clients. Owned profiles were deleted and normal-user kubeconfig fingerprints stayed unchanged.

The reviewed CSI lock pins source revisions, licenses, manifests and nine image indexes with ARM64/AMD64 platform digests. Filesystem/inode telemetry and the fsynced 16 MiB shared-write gate pass on Kubernetes 1.37 (`e66f6547-bda3-46ed-bd24-7b895d33e6c0`, repeated optimized run `dbdfc25a-360a-4dc8-a06a-00410629a6c0`), 1.36.4 (`50c63562-1e2b-42b8-b59e-20d0f9aca31a`) and 1.35.8 (`5bc4692e-e9ac-4bad-8864-a3e52074cc0d`). All those profiles were deleted. They are earlier source checkpoints; final release-binary live reruns are recorded separately.

These runs establish single-node CSI hostpath Filesystem behavior on this Mac, Docker Desktop 29.9.2, a Linux ARM64 VM, four Docker CPUs and 3.825 GiB memory. These historical runs alone established neither minimum resource requirements nor cross-node RWX, raw-block telemetry, cloud-provider support or native Linux-host lifecycle success. The newer shared suite now adds native Linux-host lifecycle and real cross-node NFS RWX evidence for three Kubernetes minors and both architectures; raw Block and cloud-provider certification remain unverified. See [the capability matrix](COMPATIBILITY.md).

## Installation and remaining work

The complete artifact set passes isolated Krew installation on native macOS ARM64. Installed-binary checks include seven help/version/argument/configured-query cases and three PTY credential cases. SHA-verified Krew 0.5.0 is workspace-local; the user's Krew installation is untouched. Twelve packaging regressions pass, including real Cargo file-selection canaries.

Root-anchored Cargo include patterns exclude generated credentials, upstream checkouts and runtime/build artifacts. An earlier source package passed compiler verification and local Cargo installation; final-source packaging/install evidence must be refreshed after documentation reconciliation. No source publication or Krew index submission has occurred.

Finish the acceptance audit, final source installation, current-release positive/negative CSI checks and native CI. Preserve failures and explicitly label unavailable capabilities. Coverage and emulation do not replace remaining phase exits.
