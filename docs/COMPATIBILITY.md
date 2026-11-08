# Compatibility and support evidence

Updated 2026-11-07. Actual tests, builds and live profiles below have different scopes. Native CI has been prepared but has not run. Contract validation supplements live evidence and does not certify a CSI implementation.

| Environment | Capability | Current evidence |
| --- | --- | --- |
| Native macOS ARM64 | Models, SDK/HTTP, reports, CLI, credential/process/terminal handling | 169 locked Rust tests; nineteen pipeline and nine HTTP reports independently verified; full-source coverage 93.44%; Clippy, fuzz smoke, installed Krew configured-query and PTY checks pass |
| Linux ARM64 musl, native ARM64 container on Docker Desktop ARM64 VM | SDK/HTTP/executable/process/terminal handling and static release build | 169 locked Rust tests and three PTY cases pass; owned container removed |
| Linux AMD64 musl, emulated on Docker Desktop ARM64 VM | Same container tests and static release build | 168 tests and three PTY cases pass before one test-only addition; native AMD64 validation remains pending |
| macOS AMD64 | Release artifact | Genuine cross-build succeeds; native Intel runtime and installation unverified |
| kind 0.33.0, Kubernetes 1.37.0, local provisioner, Mac ARM64 | Inventory, consumers and restricted RBAC | Repeated complete create/verify/delete cycles, denied PV/class/proxy cases and unchanged user kubeconfigs; this provisioner's filesystem metrics are not established |
| kind 0.33.0, Kubernetes 1.36.4 and 1.35.8, Mac ARM64 | Older API/client/auth inventory compatibility | Inventory/RBAC lifecycles pass using separately selected matching kubectl clients; owned clusters deleted |
| kind, Kubernetes 1.35.8/1.36.4/1.37.0, CSI hostpath 1.19.0 and snapshot-controller 8.7.0, Mac ARM64 | Filesystem byte/inode observations via proxy | Locked driver deployment, bound claim, two ready same-node consumers, fresh attributable tuples and compatible changed counters after a fsynced shared 16 MiB write pass; optimized baseline repeated; all profiles deleted |
| Native Linux hosts, AMD64/ARM64, Kubernetes 1.35–1.37 | Inventory/RBAC and CSI lifecycle | Six-job CI workflow prepared; not yet executed |
| Dedicated cross-node file storage | Actual multi-node RWX | Deferred; mocked accounting cases pass |
| Live raw block, cloud CSI, Rook Ceph, Longhorn, OpenEBS | Provider-specific capabilities | Unverified; Block logical/n/a behavior has actual HTTP/executable coverage |

The initial API target is Kubernetes 1.35–1.37. Inventory, byte metrics, inode metrics and Block handling are separate capabilities. Hostpath demo counters do not establish per-claim physical isolation. Earlier live runs preceded the final credential/retry corrections; final release-binary reruns are recorded separately.

## Live receipts

| Profile | Kubernetes | Run IDs |
| --- | --- | --- |
| Inventory | 1.37.0 | `76a3a328-3132-455b-8380-1ef7a06ae35c`, `71e83224-61f9-4287-b32b-00f5c75ff1a3` |
| Inventory | 1.36.4 | `9106ba01-1c74-4ec1-a2f2-e06ad4b3fa78` |
| Inventory | 1.35.8 | `9da3d1f9-fa19-4a2e-aafc-60bd24c0a9f7` |
| CSI | 1.37.0 | `e66f6547-bda3-46ed-bd24-7b895d33e6c0`, `dbdfc25a-360a-4dc8-a06a-00410629a6c0` |
| CSI | 1.36.4 | `50c63562-1e2b-42b8-b59e-20d0f9aca31a` |
| CSI | 1.35.8 | `5bc4692e-e9ac-4bad-8864-a3e52074cc0d` |

Sanitized reports and receipts are under corresponding ignored `artifacts/run-ID` directories. Docker Desktop supplied four CPUs and 3.825 GiB memory, with profiles run serially. This is observed capacity, not a tested minimum.

## Recording results

Record plugin source/version/hash, host architecture/runtime, kind/node digest, Kubernetes/kubectl versions, driver and sidecar digests, storage class, volume/access mode, RBAC profile, observation age/coverage and run ID. An uncommitted local source hash is not a release commit.

Use passed, failed, unavailable capability, environment preflight failed or not tested. Preserve exact acceptance scope. Skipped/unavailable telemetry cannot pass the required CSI gate. Preserve inventory evidence separately when metrics are unavailable.

Exclude credentials and raw node response bodies. See [local development](DEVELOPMENT.md) for controlled evidence capture.
