# Real CSI usage examples

These outputs come from actual kubectl plugin queries against the disposable CSI hostpath fixture. One 1 GiB claim is mounted by two ready pods on the same node. The driver reported fresh filesystem and inode counters; the setup gate also verified compatible counter changes after a bounded, fsynced 16 MiB shared write and removed that probe file.

## Environment

Captured 2026-11-08T02:34:22.538089+00:00 using Kubernetes v1.37.0, kubectl v1.37.2, and kubectl-view-storage 0.1.0-dev on macOS ARM64. The driver is hostpath 1.19.0 with snapshot-controller 8.7.0. [Capture metadata](capture.json) records the source revision, binary hash, commands, exit codes, and stdout hashes. Each command performs a fresh collection; counters and timestamps can change between commands or runs.

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
CONTEXT kind-view-storage-csi   SCOPE view-storage-csi   KIND pvc
STORAGE RECORDS REQUESTED PROVISIONED FS-CAP USED AVAIL USE%
storageclass=view-storage-csi 1 1Gi 1Gi 58.37Gi 35.1Gi 20.27Gi 60.132122% [metrics:ok]
  namespace=view-storage-csi 1 1Gi 1Gi 58.37Gi 35.1Gi 20.27Gi 60.132122% [metrics:ok]
    pvc=data 1 1Gi 1Gi 58.37Gi 35.1Gi 20.27Gi 60.132122% [metrics:ok]
SELECTION complete; matched=1 unknown=0
METRICS 1/1 known-match eligible claims fresh and complete
FILESYSTEM TOTALS per-claim reported filesystem totals; physical_total=false
```

Exit code: `0`.

## Table

```sh
kubectl view-storage -n view-storage-csi -u --view table
```

```text
CONTEXT kind-view-storage-csi   SCOPE view-storage-csi   KIND pvc
PVC REQUESTED PROVISIONED FS-CAP USED AVAIL USE% INODE% STATE
data 1Gi 1Gi 58.37Gi 35.1Gi 20.27Gi 60.132122% 11.211557% Bound [metrics:ok]
SELECTION complete; matched=1 unknown=0
METRICS 1/1 known-match eligible claims fresh and complete
FILESYSTEM TOTALS per-claim reported filesystem totals; physical_total=false
```

Exit code: `0`.

The claim requests 1 GiB and has 1 GiB provisioned, while the driver reports about 58.37 GiB of filesystem capacity. This demo driver exposes its backing filesystem. These are different measurements; the reported 20.27 GiB available is neither PVC headroom nor a physical backend free-space total.

## Summary

```sh
kubectl view-storage -n view-storage-csi -u --view summary
```

```text
CONTEXT kind-view-storage-csi   SCOPE view-storage-csi   KIND pvc
GROUP RECORDS REQUESTED PROVISIONED FS-CAP USED AVAIL USE% METRICS
ROOT 1 1Gi 1Gi 58.37Gi 35.1Gi 20.27Gi 60.132122% 1/1 phases={"Bound":1}
storageclass=view-storage-csi 1 1Gi 1Gi 58.37Gi 35.1Gi 20.27Gi 60.132122% 1/1 phases={"Bound":1}
SELECTION complete; matched=1 unknown=0
METRICS 1/1 known-match eligible claims fresh and complete
FILESYSTEM TOTALS per-claim reported filesystem totals; physical_total=false
```

Exit code: `0`.

## Issues

The captured counters are below the configured warning thresholds, so this real run has no observed findings. For actual Pending/missing-metrics output, see the [inventory issues](../inventory/README.md#issues).

```sh
kubectl view-storage -n view-storage-csi -u --view issues
```

```text
CONTEXT kind-view-storage-csi   SCOPE view-storage-csi   KIND pvc
SEVERITY OBJECT CODE EVIDENCE
No observed findings in selected records.
SELECTION complete; matched=1 unknown=0
METRICS 1/1 known-match eligible claims fresh and complete
FILESYSTEM TOTALS per-claim reported filesystem totals; physical_total=false
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
        "used_bytes": "37685460992",
        "available_bytes": "21768921088"
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
pvc,view-storage-csi,data,942112cf-5edc-4053-bb51-60df4ac424af,Bound,view-storage-csi,hostpath.csi.k8s.io,Filesystem,pvc-942112cf-5edc-4053-bb51-60df4ac424af,1073741824,1073741824,1073741824,62671097856,37685460992,21768921088,60.132122,3907584,438101,3469483,11.211557,ok,,kubelet,2026-11-08T02:34:11Z,2,0,
```

Exit code: `0`.

## Strict metrics check

This fixture has one eligible claim with a fresh complete byte tuple, so the check succeeds with exit 0. Missing telemetry produces a different outcome: see the [real exit-4 example](../inventory/README.md#strict-metrics-check).

```sh
kubectl view-storage -n view-storage-csi -u --require-metrics --view summary
```

```text
CONTEXT kind-view-storage-csi   SCOPE view-storage-csi   KIND pvc
GROUP RECORDS REQUESTED PROVISIONED FS-CAP USED AVAIL USE% METRICS
ROOT 1 1Gi 1Gi 58.37Gi 35.1Gi 20.27Gi 60.132122% 1/1 phases={"Bound":1}
storageclass=view-storage-csi 1 1Gi 1Gi 58.37Gi 35.1Gi 20.27Gi 60.132122% 1/1 phases={"Bound":1}
SELECTION complete; matched=1 unknown=0
METRICS 1/1 known-match eligible claims fresh and complete
FILESYSTEM TOTALS per-claim reported filesystem totals; physical_total=false
```

Exit code: `0`.

## Cleanup

```sh
python3 scripts/dev-cluster.py delete csi
```

The capture cluster was deleted after collection. This is single-node Filesystem evidence for the reviewed hostpath fixture; it does not establish live raw-block, cross-node RWX, or cloud-provider support.
