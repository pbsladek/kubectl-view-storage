# Real CSI usage examples

These outputs come from actual kubectl plugin queries against the disposable CSI hostpath fixture. One 1 GiB claim is mounted by two ready pods on the same node. The driver reported fresh filesystem and inode counters; the setup gate also verified compatible counter changes after a bounded, fsynced 16 MiB shared write and removed that probe file.

## Environment

Captured 2026-11-08T02:57:02.427206+00:00 using Kubernetes v1.37.0, kubectl v1.37.2, and kubectl-view-storage 0.1.0-dev on macOS ARM64. The driver is hostpath 1.19.0 with snapshot-controller 8.7.0. [Capture metadata](capture.json) records the base revision, exact production-source fingerprint, binary hash, commands, exit codes, and stdout hashes. The capture records the uncommitted presentation revision through its production-source fingerprint. Each command performs a fresh collection; counters and timestamps can change between commands or runs.

To reproduce from the repository root, build the plugin as described in the [README](../../../README.md#build-from-source), then create the owned fixture:

```sh
python3 scripts/dev-cluster.py create csi --binary target/debug/kubectl-view_storage
export KUBECONFIG="$PWD/.local/kubeconfigs/csi.yaml"
```

The capture used the release binary recorded in `capture.json`; the commands below also work with the development build on PATH. The setup gate must produce fresh attributable byte tuples to pass; it cannot pass by skipping unavailable metrics.

## Tree

```sh
kubectl view-storage -n view-storage-csi -u
```

```text
Persistent volume claims
Context: kind-view-storage-csi  |  Namespace: view-storage-csi

STORAGE                         COUNT  REQUESTED  PROVISIONED   FS SIZE      USED  AVAILABLE   USE%  METRICS
------------------------------  -----  ---------  -----------  --------  --------  ---------  -----  -------
Class: view-storage-csi             1      1 GiB        1 GiB  58.4 GiB  35.1 GiB   20.3 GiB  60.1%  Fresh
`- Namespace: view-storage-csi      1      1 GiB        1 GiB  58.4 GiB  35.1 GiB   20.3 GiB  60.1%  Fresh
   `- PVC: data                     1      1 GiB        1 GiB  58.4 GiB  35.1 GiB   20.3 GiB  60.1%  Fresh

Selection: complete (1 matched)
Metrics: 1/1 claims fresh and complete; inodes: 1/1
Latest sample: 2026-11-08T02:56:43Z
Note: filesystem size exceeds provisioned size on 1 volume(s); the driver may report backing storage.
Filesystem totals are per claim, not physical storage totals.
```

Exit code: `0`.

## Table

```sh
kubectl view-storage -n view-storage-csi -u --view table
```

```text
Persistent volume claims
Context: kind-view-storage-csi  |  Namespace: view-storage-csi

PVC   PHASE  PODS  REQUESTED  PROVISIONED   FS SIZE      USED  AVAILABLE   USE%  INODE%  FS SAMPLE
----  -----  ----  ---------  -----------  --------  --------  ---------  -----  ------  ---------
data  Bound     2      1 GiB        1 GiB  58.4 GiB  35.1 GiB   20.3 GiB  60.1%   11.2%  Fresh

Selection: complete (1 matched)
Metrics: 1/1 claims fresh and complete; inodes: 1/1
Latest sample: 2026-11-08T02:56:43Z
Note: filesystem size exceeds provisioned size on 1 volume(s); the driver may report backing storage.
Filesystem totals are per claim, not physical storage totals.
```

Exit code: `0`.

The claim requests 1 GiB and has 1 GiB provisioned, while the driver reports about 58.4 GiB of filesystem capacity. This demo driver exposes its backing filesystem. These are different measurements; the available bytes is neither PVC headroom nor a physical backend free-space total.

## Summary

```sh
kubectl view-storage -n view-storage-csi -u --view summary
```

```text
Persistent volume claims
Context: kind-view-storage-csi  |  Namespace: view-storage-csi

GROUP                    COUNT  REQUESTED  PROVISIONED   FS SIZE      USED  AVAILABLE   USE%  METRICS    PHASES
-----------------------  -----  ---------  -----------  --------  --------  ---------  -----  ---------  -------
TOTAL                        1      1 GiB        1 GiB  58.4 GiB  35.1 GiB   20.3 GiB  60.1%  1/1 Fresh  1 Bound
Class: view-storage-csi      1      1 GiB        1 GiB  58.4 GiB  35.1 GiB   20.3 GiB  60.1%  1/1 Fresh  1 Bound

Selection: complete (1 matched)
Metrics: 1/1 claims fresh and complete; inodes: 1/1
Latest sample: 2026-11-08T02:56:43Z
Note: filesystem size exceeds provisioned size on 1 volume(s); the driver may report backing storage.
Filesystem totals are per claim, not physical storage totals.
```

Exit code: `0`.

## Issues

The captured counters are below the configured warning thresholds, so this real run has no observed findings. For actual Pending/missing-metrics output, see the [inventory issues](../inventory/README.md#issues).

```sh
kubectl view-storage -n view-storage-csi -u --view issues
```

```text
Persistent volume claims
Context: kind-view-storage-csi  |  Namespace: view-storage-csi

No observed findings in selected records.

Selection: complete (1 matched)
Metrics: 1/1 claims fresh and complete; inodes: 1/1
Latest sample: 2026-11-08T02:56:43Z
Note: filesystem size exceeds provisioned size on 1 volume(s); the driver may report backing storage.
Filesystem totals are per claim, not physical storage totals.
```

Exit code: `0`.

## JSON

The complete command and unmodified stdout are retained in [report.json](report.json):

```sh
kubectl view-storage -n view-storage-csi -u -o json
```

For a compact display, this command pipes the same output through the checked-in [jq projection](json-summary.jq). The following is the actual jq output from the captured report; it is a subset of the full schema, not a different plugin format.

```sh
kubectl view-storage -n view-storage-csi -u -o json | jq -f examples/live/csi/json-summary.jq
```

```json
{
  "schema_version": "view-storage/v1alpha1",
  "selection": {
    "base_count": 1,
    "complete": true,
    "excluded_count": 0,
    "matched_count": 1,
    "unknown_count": 0,
    "unknown_records": []
  },
  "records": [
    {
      "name": "data",
      "phase": "Bound",
      "requested_bytes": "1073741824",
      "provisioned_bytes": "1073741824",
      "filesystem": {
        "status": "ok",
        "capacity_bytes": "62671097856",
        "used_bytes": "37685436416",
        "available_bytes": "21768945664"
      },
      "inodes": {
        "status": "ok",
        "inodes_total": "3907584",
        "inodes_used": "438101",
        "inodes_free": "3469483"
      }
    }
  ]
}
```

Byte counts and inode counts are decimal strings, preserving exact integers. The full report also contains timestamps, per-field status, raw observation candidates, relationships, findings, and collection costs.

## CSV

This is the complete captured header and data row. It includes the live observation timestamp and both consumers; CSV omits the detailed relationship graph available in JSON.

```sh
kubectl view-storage -n view-storage-csi -u -o csv
```

```csv
kind,namespace,name,uid,phase,storage_class,driver,volume_mode,pv_name,requested_bytes,bound_bytes,provisioned_bytes,filesystem_capacity_bytes,used_bytes,available_bytes,used_percent,inodes_total,inodes_used,inodes_free,inode_percent,metrics_status,metrics_reason,metrics_source,observed_at,active_consumer_count,terminating_consumer_count,severity
pvc,view-storage-csi,data,efdc6b09-e967-4799-9025-be3cb18546bd,Bound,view-storage-csi,hostpath.csi.k8s.io,Filesystem,pvc-efdc6b09-e967-4799-9025-be3cb18546bd,1073741824,1073741824,1073741824,62671097856,37685436416,21768945664,60.132083,3907584,438101,3469483,11.211557,ok,,kubelet,2026-11-08T02:56:43Z,2,0,
```

Exit code: `0`.

## Strict metrics check

This fixture has one eligible claim with a fresh complete byte tuple, so the check succeeds with exit 0. Missing telemetry produces a different outcome: see the [real exit-4 example](../inventory/README.md#strict-metrics-check).

```sh
kubectl view-storage -n view-storage-csi -u --require-metrics --view summary
```

```text
Persistent volume claims
Context: kind-view-storage-csi  |  Namespace: view-storage-csi

GROUP                    COUNT  REQUESTED  PROVISIONED   FS SIZE      USED  AVAILABLE   USE%  METRICS    PHASES
-----------------------  -----  ---------  -----------  --------  --------  ---------  -----  ---------  -------
TOTAL                        1      1 GiB        1 GiB  58.4 GiB  35.1 GiB   20.3 GiB  60.1%  1/1 Fresh  1 Bound
Class: view-storage-csi      1      1 GiB        1 GiB  58.4 GiB  35.1 GiB   20.3 GiB  60.1%  1/1 Fresh  1 Bound

Selection: complete (1 matched)
Metrics: 1/1 claims fresh and complete; inodes: 1/1
Latest sample: 2026-11-08T02:56:43Z
Note: filesystem size exceeds provisioned size on 1 volume(s); the driver may report backing storage.
Filesystem totals are per claim, not physical storage totals.
```

Exit code: `0`.

## Cleanup

```sh
python3 scripts/dev-cluster.py delete csi
```

The capture cluster was deleted after collection. This is single-node Filesystem evidence for the reviewed hostpath fixture; it does not establish live raw-block, cross-node RWX, or cloud-provider support.
