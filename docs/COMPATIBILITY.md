# Compatibility and support evidence

Updated 2026-11-10. Actual tests, builds and live profiles below have different scopes. Native artifact validation passed at `0695641`; the expanded storage workflow passed its full native Linux matrix at `0ee4e9d`. Contract validation supplements live evidence and does not certify a CSI implementation.

| Environment | Capability | Current evidence |
| --- | --- | --- |
| Native macOS ARM64 | Models, SDK/HTTP, reports, CLI, credential/process/terminal handling | 171 locked Rust tests; nineteen pipeline and nine HTTP reports independently verified; full-source coverage 93.74%; Clippy, fuzz smoke, installed Krew configured-query and PTY checks pass |
| Linux ARM64 musl, native ARM64 container on Docker Desktop ARM64 VM | SDK/HTTP/executable/process/terminal handling and static release build | 169 locked Rust tests and three PTY cases pass; owned container removed |
| Linux AMD64 musl, emulated on Docker Desktop ARM64 VM | Same container tests and static release build | 168 tests and three PTY cases pass before one test-only addition; native AMD64 tests/installations subsequently passed in the four-target CI run linked below |
| Native macOS AMD64/ARM64 and Linux AMD64/ARM64 | Release artifacts and installed Krew checks | [Four-target native distribution run at `0695641`](https://github.com/pbsladek/kubectl-view-storage/actions/runs/42440998060) passed; separate from live storage compatibility |
| kind 0.33.0, Kubernetes 1.37.0, local provisioner, Mac ARM64 | Inventory, consumers and restricted RBAC | Repeated complete create/verify/delete cycles, denied PV/class/proxy cases and unchanged user kubeconfigs; new bounded-write and pod-replacement persistence gates pass; unavailable filesystem metrics stay explicit |
| kind 0.33.0, Kubernetes 1.36.4 and 1.35.8, Mac ARM64 | Older API/client/auth inventory compatibility | Inventory/RBAC lifecycles pass using separately selected matching kubectl clients; owned clusters deleted |
| kind, Kubernetes 1.35.8/1.36.4/1.37.0, CSI hostpath 1.19.0 and snapshot-controller 8.7.0, Mac ARM64 | Filesystem byte/inode observations via proxy | Locked driver deployment, bound claim, two ready same-node consumers, fresh attributable tuples and compatible changed counters after a fsynced shared 16 MiB write pass; optimized baseline repeated; all profiles deleted |
| Native Linux hosts, AMD64/ARM64, Kubernetes 1.35–1.37 | Local-path, CSI hostpath and cross-node NFS CSI | [All six native cells passed](https://github.com/pbsladek/kubectl-view-storage/actions/runs/42900675127); all eighteen backend gates and cleanup receipts verified. [Receipt index](storage-e2e-hosted.json) |
| NFS CSI 4.13.6 with userspace Ganesha | Actual multi-node RWX | Mac ARM64 live cross-node write/byte/inode/recovery gate and cleanup passed in 233.86 seconds (`da75058e-0b0c-42bc-a6f2-5fb0f8c12f88`); native Linux amd64/arm64 receipts also pass across all three selected Kubernetes versions. See [storage E2E](STORAGE_E2E.md). |
| Live raw block, cloud CSI, Rook Ceph, Longhorn, OpenEBS | Provider-specific capabilities | Unverified; Block logical/n/a behavior has actual HTTP/executable coverage |

The initial API target is Kubernetes 1.35–1.37. Inventory, byte metrics, inode metrics and Block handling are separate capabilities. Hostpath demo counters do not establish per-claim physical isolation. Earlier live runs preceded the final credential/retry corrections; final release-binary reruns are recorded separately.

## Calculation extension

The uncommitted calculation source identified in [implementation evidence](IMPLEMENTATION_STATUS.md#operator-calculation-extension) passes 189 Rust tests on native macOS arm64 and Linux arm64 GNU, including actual HTTP/export, history comparison and Unix input/process tests. Both hosts' five calculation reports pass the independent integer/Fraction/schema oracle. The final macOS release binary also passes the disposable CSI write/growth and real StatefulSet/shared-claim gate with owned cleanup. [Real captures](../examples/live/calculations/README.md) include MB/GB/TB, JSON and CSV. This does not refresh earlier Intel/macOS or Linux musl release certification; those jobs remain required for the selected release revision.

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

## Shared local/CI storage suite

The [storage E2E harness](STORAGE_E2E.md) adds bounded writes and pod-replacement persistence to local-path, retains the existing CSI telemetry gate, and adds a required cross-node NFS CSI gate. On the current Mac, local-path passed in 87.75 seconds (`fe10fc22-b639-47e4-9934-4dacbb779ace`) and CSI hostpath passed in 171.01 seconds (`63c3e820-ed3c-444b-807d-031177998ab4`), including owned cleanup. NFS CSI passed with Ganesha 9.14 in 233.86 seconds (`da75058e-0b0c-42bc-a6f2-5fb0f8c12f88`), including cleanup. The older Ganesha image failed its real metadata/telemetry gate and was replaced. The final local-path usage rerun also passed (`74c1ab03-e851-42e8-880e-3d4e0c153c69`, 88.90 seconds). A complete default three-backend suite then passed on Kubernetes 1.36.4 in 452.27 seconds (`storage-e2e-3ad4eb12-37fd-42ed-9e8d-09b6b46879da`): local-path 80.67 seconds, CSI hostpath 141.82 seconds, NFS CSI 229.78 seconds, with all owned clusters deleted and user kubeconfigs unchanged. The complete suite also passed on Kubernetes 1.35.8 in 472.72 seconds (`storage-e2e-9a13eeb5-4ae5-42eb-829d-5b887d647516`), including all cleanup and kubeconfig checks. The final cross-node accounting assertions pass on real reports, and the unchanged production source fingerprint matches the captured release binary. The separate [native GitHub matrix](https://github.com/pbsladek/kubectl-view-storage/actions/runs/42900675127) now supplies that evidence; its uploaded receipts verify all six cells independently of the local runs.

## Recording results

Record plugin source/version/hash, host architecture/runtime, kind/node digest, Kubernetes/kubectl versions, driver and sidecar digests, storage class, volume/access mode, RBAC profile, observation age/coverage and run ID. An uncommitted local source hash is not a release commit.

Use passed, failed, unavailable capability, environment preflight failed or not tested. Preserve exact acceptance scope. Skipped/unavailable telemetry cannot pass the required CSI gate. Preserve inventory evidence separately when metrics are unavailable.

Exclude credentials and raw node response bodies. See [local development](DEVELOPMENT.md) for controlled evidence capture.
