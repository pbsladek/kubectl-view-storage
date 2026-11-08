# Lessons from df-pv implementation and support

Review date: 2026-11-07. Scope: the fetched [root.go implementation](https://raw.githubusercontent.com/yashbhutwala/kubectl-df-pv/refs/heads/master/pkg/df-pv/root.go), its [README](https://github.com/yashbhutwala/kubectl-df-pv), and linked user reports. The source URL tracks a branch; observations apply to the fetched code, not every release. Reports identify regression scenarios, not proof that every current build has the defect.

## Implementation lessons

The inspected source builds rows from node observations, uses nonoptional scalar metrics, divides floating-point counters, performs per-sample PVC lookups with a discarded error, and traces node payloads. Its producer shutdown/channel closure also merits a cancellation regression.

| Observation | Requirement for view-storage | Acceptance |
| --- | --- | --- |
| Rows start from node samples | Inventory preserves claims without samples or consumers | A03, A13, A23 |
| Absent scalars can resemble zero | Optional counters preserve null and actual zero | A13, A25, A42 |
| Direct division and negative-size clamping | Exact comparisons, denominator checks, no invalid-to-zero conversion | A22, A25, A44, A52 |
| Per-sample lookup discards an error | Cached joins with explicit lookup diagnostics | A05, A39, A48 |
| Producer/error and channel lifecycle | Shared cancellation and bounded worker shutdown | A11, A49 |
| Node payload tracing | Allowlisted diagnostics; no unfiltered node response logs | A50 |

## Support lessons

| Report | Decision and regression |
| --- | --- |
| [Duplicate PVs, issue 10](https://github.com/yashbhutwala/kubectl-df-pv/issues/10) | Distinct record IDs and unique edges; consumer count never inflates root capacity, A08/A09/A27 |
| [Volumes without pods, issue 13](https://github.com/yashbhutwala/kubectl-df-pv/issues/13) | Inventory is independent of observations; Bound status does not prove a backend disk still exists, A03/A14 |
| [Machine output, issue 31](https://github.com/yashbhutwala/kubectl-df-pv/issues/31) | Versioned JSON, flat CSV, stderr diagnostics, A24 |
| [Sorting, issue 30](https://github.com/yashbhutwala/kubectl-df-pv/issues/30) | Deterministic raw quantity/ratio sorting with unknowns last, A19/A24 |
| [Piped colors, issue 27](https://github.com/yashbhutwala/kubectl-df-pv/issues/27) | TTY detection, NO_COLOR, plain exports, A29 |
| [Label selection, issue 38](https://github.com/yashbhutwala/kubectl-df-pv/issues/38) | PVC selectors preserve differently labeled consumers, A10 |
| [Current namespace, issue 42](https://github.com/yashbhutwala/kubectl-df-pv/issues/42) | Current kubeconfig namespace and explicit cluster scope, A01/A02 |
| [kubectl flag placement, issue 21](https://github.com/yashbhutwala/kubectl-df-pv/issues/21) | Document flags after the plugin name and verify context resolution, A51 |

The inspected source already uses generic kubectl configuration flags and a manual color-disable flag. An old issue should not be described as a missing feature without checking implementation.

The README's provisioner-specific examples motivate a capability matrix. Track inventory, byte metrics, inodes, volume modes, driver versions, and permissions separately. Kind with a local provisioner and kind with a CSI driver are different combinations.

## Support information

Distinguish empty inventory, incomplete selection, no live consumer, failed enrichment, absent samples, stale timestamps, forbidden proxy access, and invalid responses. Errors identify the operation and affected fields and suggest investigation; cluster-admin is not a generic remedy.

Capture plugin/Kubernetes versions, driver/provisioner identity, scope, relevant UIDs, diagnostics/counters, and redacted report snippets. Provide reproducible fixture instructions. Do not request credentials or full node Summary API bodies in an issue.

Live evidence belongs in [COMPATIBILITY.md](COMPATIBILITY.md). Profiles and phases are defined in [DEVELOPMENT.md](DEVELOPMENT.md) and [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md).
