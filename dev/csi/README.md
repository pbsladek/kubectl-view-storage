# Reviewed CSI deployment lock

The CSI profile has a reviewed [deployment/image lock](lock.json), vendored canonical JSON resources and preserved upstream licenses. The real single-node Filesystem byte/inode gate passed on macOS ARM64 with kind 0.33.0 across Kubernetes 1.35.8, 1.36.4 and 1.37.0. Repeated baseline runs include debug and optimized binaries. See the versioned run IDs in [the compatibility record](../../docs/COMPATIBILITY.md). Linux CI, raw block and cross-node RWX remain separate gates. The profile refuses creation without the lock and its matching SHA-256 in [versions.json](../versions.json).

Use CSI hostpath v1.19.0 and the selected Kubernetes e2e BusyBox `registry.k8s.io/e2e-test-images/busybox:1.37.0-2` for the probe. The workload image selection follows [Kubernetes v1.37's image manifest](https://github.com/kubernetes/kubernetes/blob/v1.37.0/test/utils/image/manifest.go). All nine image indexes and their linux/amd64 and linux/arm64 platform manifests were resolved from the registries; ARM64 execution passed locally. AMD64 execution remains unverified.

The [upstream v1.19 deployment](https://github.com/kubernetes-csi/csi-driver-host-path/blob/v1.19.0/deploy/kubernetes-1.35/hostpath/csi-hostpath-plugin.yaml) currently defaults its hostpath container to v1.18.0. Review and replace that container reference with the selected v1.19.0 digest. A release-tagged deployment file alone does not establish the runtime version. The lock checker requires the StatefulSet's `hostpath` container to use the exact selected driver image.

The reviewed bundle includes snapshot CRDs/controller, driver resources and required RBAC. [review-bundle.py](review-bundle.py) reproduces it from clean local upstream checkouts at the exact commits recorded in the script and lock. Run it with `uv run dev/csi/review-bundle.py`; PyYAML 6.0.3 is isolated to review-time conversion. Runtime setup uses JSON and needs no YAML parser dependency.

The transformation keeps the hostpath deployment and CSIDriver, extracts the upstream Role/ClusterRole declarations for its own service-account bindings, and removes bindings for the upstream-disabled optional snapshot-metadata service. It corrects the tagged hostpath deployment's v1.18.0 image to v1.19.0 and aligns the snapshot-controller image from v8.6.0 to the selected snapshotter source release v8.7.0. It includes the three v1 snapshot CRDs and the full snapshot-controller RBAC/deployment. The upstream socat/NodePort debugging workload is not part of this telemetry profile. The lock records input-file hashes, source commits, preserved LICENSE hashes, generated JSON hashes and registry digests.

`lock.json` has these required fields:

| Field | Required content |
| --- | --- |
| `status` | `reviewed`, after actual source/registry/license review |
| `driver_release` | The release selected in versions.json |
| `source_revision` | Exact 40-hex driver source commit |
| `snapshot_dependency_revisions` | Component name → exact 40-hex source commit; nonempty |
| `driver_image` | Selected hostpath tag plus immutable `@sha256:` index digest |
| `workload_image` | Selected BusyBox tag plus immutable index digest |
| `images` | Every driver/sidecar/controller/workload reference, with `image` and `platform_digests` mapping `amd64` and `arm64` to their reviewed `sha256:` platform manifest digests |
| `manifests` | Ordered `{file, sha256}` descriptors for local Kubernetes JSON files |
| `licenses` | Nonempty `{file, sha256}` descriptors for preserved upstream licenses |
| `snapshot_controller` | Its deployed `namespace` and `name` |

Keep CRD-only files first. The harness waits for establishment after each such file, before applying dependent resources. Required resources include the three VolumeSnapshot CRDs, the `hostpath.csi.k8s.io` CSIDriver, `default/csi-hostpathplugin` StatefulSet, and the selected snapshot-controller Deployment. All container references in manifests must appear in the image lock. Files must remain within this directory and match their hashes. The checker validates reviewed lock consistency; it does not independently query registries or establish upstream maintenance.

After review, pin the complete lock file's SHA-256 in `dev/versions.json:csi_lock_sha256`. That reference is the canonical CSI environment lock; revisions and image digests live in its hashed content. Keep inventory fixture image locking as a separate phase 0 requirement.

With the real locked plugin build and Docker available:

```sh
python3 scripts/dev-cluster.py create csi
python3 scripts/dev-cluster.py verify csi
python3 scripts/dev-cluster.py delete csi
```

Every operation uses the owned cluster's explicit kubeconfig. Setup applies [csi.json](../fixtures/csi.json) with its locked workload image: one 1Gi Filesystem claim and two unprivileged, same-node consumers. This developer fixture uses its administrator config to create resources and write the probe; the plugin's collection interface remains read-only.

The verification gate requires current driver/snapshot-controller rollouts, a bound and correctly attributed volume, both ready consumers, complete selection, one logical node telemetry target, and correct shared-capacity accounting. It removes any previous probe, waits for a fresh attributable complete byte tuple, then writes exactly 16 MiB with fsync. Both consumers must see that file size. The later tuple must advance beyond the baseline and write completion, remain fresh, retain capacity, and show compatible changed used/available counters. This verifies reported filesystem observations; a PVC request is not a dedicated backend filesystem size.

The monotonic polling deadline is six minutes; each plugin query has its own bounded subprocess timeout. Insufficient free space, absent/stale/unchanged telemetry, wrong identities, false required-metrics success, or cleanup failure fails the gate. Probe cleanup runs on success, error, and interruption; cleanup failures invalidate the gate. Before/after selected reports, environment and validation evidence go under `artifacts/run-ID`; kubeconfigs and raw node Summary responses are excluded. Inode coverage is recorded separately from the required byte gate.

The offline `scripts/test-csi-environment.py` checks lock rejection, workload shape, ownership/cleanup, readiness, provenance, clock/quantity boundaries and failed telemetry outcomes with synthetic data. It does not run Kubernetes, verify an image architecture, or establish a working production binary. Live macOS Filesystem lifecycles pass for all three selected Kubernetes versions; these offline checks supplement that evidence. Native Linux CI and raw-block/cross-node capabilities remain separate requirements.
