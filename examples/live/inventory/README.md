# Real inventory examples

These are unedited outputs from the compiled plugin, invoked through kubectl against an actual disposable kind cluster. They are separate from the fixed [contract scenarios](../../contract/README.md).

The fixture contains two Bound and two Pending claims, four references to the `data` claim, Available/Released PVs, and a deliberately missing storage class. Diagnostics and unknown values below are preserved from the run.

## Environment

Captured 2026-11-08T02:31:42.866041+00:00 using Kubernetes v1.37.0, kubectl v1.37.2, and kubectl-view-storage 0.1.0-dev on macOS ARM64. [Capture metadata](capture.json) records the exact source revision, binary hash, commands, exit codes, and stdout hashes. Names, UIDs, node placement, and ages will differ on another run. Each command performs its own query.

To reproduce from the repository root, build the plugin as described in the [README](../../../README.md#build-from-source), then create the owned fixture:

```sh
python3 scripts/dev-cluster.py create inventory --binary target/debug/kubectl-view_storage
export KUBECONFIG="$PWD/.local/kubeconfigs/inventory.yaml"
```

The capture used the release binary recorded in `capture.json`; the commands below work with the development build on PATH too. The explicit fixture kubeconfig keeps queries away from other clusters.

## Default tree

```sh
kubectl view-storage -n view-storage-test
```

```text
CONTEXT kind-view-storage-inventory   SCOPE view-storage-test   KIND pvc
STORAGE RECORDS REQUESTED PROVISIONED
storageclass=deliberately-unavailable 1 128Mi -- (partial)
  namespace=view-storage-test 1 128Mi -- (partial)
    pvc=pending 1 128Mi -- (partial)
storageclass=view-storage-local 1 128Mi 128Mi
  namespace=view-storage-test 1 128Mi 128Mi
    pvc=data 1 128Mi 128Mi
storageclass=view-storage-static 1 128Mi 128Mi
  namespace=view-storage-test 1 128Mi 128Mi
    pvc=unreferenced 1 128Mi 128Mi
storageclass=(classless) 1 128Mi -- (partial)
  namespace=view-storage-test 1 128Mi -- (partial)
    pvc=classless 1 128Mi -- (partial)
SELECTION complete; matched=4 unknown=0
metrics: not requested
DIAGNOSTIC not_found: storageclasses collection failed (HTTP status 404); inspect permissions, API availability, and collection limits.
```

Exit code: `0`.

## Table

```sh
kubectl view-storage -n view-storage-test --view table
```

```text
CONTEXT kind-view-storage-inventory   SCOPE view-storage-test   KIND pvc
PVC REQUESTED PROVISIONED STATE
classless 128Mi -- Pending
data 128Mi 128Mi Bound
pending 128Mi -- Pending
unreferenced 128Mi 128Mi Bound
SELECTION complete; matched=4 unknown=0
metrics: not requested
DIAGNOSTIC not_found: storageclasses collection failed (HTTP status 404); inspect permissions, API availability, and collection limits.
```

Exit code: `0`.

## Summary

The four claims request 512 MiB in total. Only two have known provisioned capacity, totaling 256 MiB; the report keeps that total partial.

```sh
kubectl view-storage -n view-storage-test --view summary
```

```text
CONTEXT kind-view-storage-inventory   SCOPE view-storage-test   KIND pvc
GROUP RECORDS REQUESTED PROVISIONED
ROOT 4 512Mi 256Mi (partial) phases={"Bound":2,"Pending":2}
storageclass=deliberately-unavailable 1 128Mi -- (partial) phases={"Pending":1}
storageclass=view-storage-local 1 128Mi 128Mi phases={"Bound":1}
storageclass=view-storage-static 1 128Mi 128Mi phases={"Bound":1}
storageclass=(classless) 1 128Mi -- (partial) phases={"Pending":1}
SELECTION complete; matched=4 unknown=0
metrics: not requested
DIAGNOSTIC not_found: storageclasses collection failed (HTTP status 404); inspect permissions, API availability, and collection limits.
```

Exit code: `0`.

## Issues

These newly created Pending and unreferenced claims produce informational findings. The missing class is a collection diagnostic; it does not make base inventory disappear.

```sh
kubectl view-storage -n view-storage-test --view issues
```

```text
CONTEXT kind-view-storage-inventory   SCOPE view-storage-test   KIND pvc
SEVERITY OBJECT CODE EVIDENCE
info view-storage-test/classless claim_pending phase=Pending age_seconds=65
  Investigate: Inspect provisioning and delayed binding conditions.
info view-storage-test/classless no_active_consumer active_consumer_count=0 terminating_consumer_count=0
  Investigate: Review visible pod references and ownership; this does not establish deletion safety.
info view-storage-test/pending claim_pending phase=Pending age_seconds=65
  Investigate: Inspect provisioning and delayed binding conditions.
info view-storage-test/pending no_active_consumer active_consumer_count=0 terminating_consumer_count=0
  Investigate: Review visible pod references and ownership; this does not establish deletion safety.
info view-storage-test/unreferenced no_active_consumer active_consumer_count=0 terminating_consumer_count=0
  Investigate: Review visible pod references and ownership; this does not establish deletion safety.
SELECTION complete; matched=4 unknown=0
metrics: not requested
DIAGNOSTIC not_found: storageclasses collection failed (HTTP status 404); inspect permissions, API availability, and collection limits.
```

Exit code: `0`.

## Declared mounts and consumers

The shared consumer declares two paths, while retaining one consumer edge. The terminating and unscheduled references remain visible.

```sh
kubectl view-storage -n view-storage-test --pvc data --view table --wide
```

```text
CONTEXT kind-view-storage-inventory   SCOPE view-storage-test   KIND pvc
PVC REQUESTED PROVISIONED STATE
data 128Mi 128Mi Bound
DECLARED CONSUMERS (runtime unverified)
  view-storage-test/data pv=pvc-5c022956-2086-4c92-b410-093ad56ce1e4 driver=rancher.io/local-path mode=Filesystem access=ReadWriteOnce reclaim=Delete
    pod=consumer state=active node=view-storage-inventory-worker2 container=app volume=data mount=/data readOnly=true subPath=-- subPathExpr=--
  view-storage-test/data pv=pvc-5c022956-2086-4c92-b410-093ad56ce1e4 driver=rancher.io/local-path mode=Filesystem access=ReadWriteOnce reclaim=Delete
    pod=terminating-consumer state=terminating node=view-storage-inventory-worker2 container=app volume=data mount=/data readOnly=true subPath=-- subPathExpr=--
  view-storage-test/data pv=pvc-5c022956-2086-4c92-b410-093ad56ce1e4 driver=rancher.io/local-path mode=Filesystem access=ReadWriteOnce reclaim=Delete
    pod=shared-consumer state=active node=view-storage-inventory-worker2 container=app volume=alias mount=/alias readOnly=true subPath=-- subPathExpr=--
    pod=shared-consumer state=active node=view-storage-inventory-worker2 container=app volume=data mount=/data readOnly=true subPath=-- subPathExpr=--
  view-storage-test/data pv=pvc-5c022956-2086-4c92-b410-093ad56ce1e4 driver=rancher.io/local-path mode=Filesystem access=ReadWriteOnce reclaim=Delete
    pod=unscheduled-consumer state=active node=-- container=app volume=data mount=/data readOnly=true subPath=-- subPathExpr=--
SELECTION complete; matched=1 unknown=0
metrics: not requested
```

Exit code: `0`.

## Grouping by node and pod

The same claim appears under multiple references. Sibling reference groups cannot be added together; the distinct root capacity stays 128 MiB.

```sh
kubectl view-storage -n view-storage-test --pvc data -g node,pod,pvc
```

```text
CONTEXT kind-view-storage-inventory   SCOPE view-storage-test   KIND pvc
STORAGE RECORDS REQUESTED PROVISIONED
node=view-storage-inventory-worker2 1 128Mi 128Mi
  pod=view-storage-test/consumer@12c4bcce-1d2d-422d-8973-653348e2b9ce 1 128Mi 128Mi
    pvc=data 1 128Mi 128Mi
  pod=view-storage-test/shared-consumer@71b2a844-308f-49cf-b73b-994a80f5f61e 1 128Mi 128Mi
    pvc=data 1 128Mi 128Mi
  pod=view-storage-test/terminating-consumer@23a69dcf-c0f9-418c-a197-daab0c6444bc 1 128Mi 128Mi
    pvc=data 1 128Mi 128Mi
node=(no live consumer) 1 128Mi 128Mi
  pod=view-storage-test/unscheduled-consumer@76fcbe34-7676-4acc-a789-1f5eb86c8a3a 1 128Mi 128Mi
    pvc=data 1 128Mi 128Mi
SELECTION complete; matched=1 unknown=0
metrics: not requested
Shared references: sibling group totals cannot be added; root totals use distinct records.
```

Exit code: `0`.

## PV inventory

Available and Released volumes have no invented claim request quantities.

```sh
kubectl view-storage -A --kind pv --view table
```

```text
CONTEXT kind-view-storage-inventory   SCOPE all namespaces   KIND pv
PV REQUESTED PROVISIONED STATE
pvc-5c022956-2086-4c92-b410-093ad56ce1e4 128Mi 128Mi Bound
view-storage-available -- 128Mi Available
view-storage-released -- 128Mi Released
view-storage-unreferenced 128Mi 128Mi Bound
SELECTION complete; matched=4 unknown=0
metrics: not requested
DIAGNOSTIC not_found: persistentvolumeclaims collection failed (HTTP status 404); inspect permissions, API availability, and collection limits.
```

Exit code: `0`.

## CSV

This is the complete captured CSV, including its header. Empty numeric cells represent unknown values; they are not zero. Detailed relationships and diagnostics are available in JSON.

```sh
kubectl view-storage -n view-storage-test -o csv
```

```csv
kind,namespace,name,uid,phase,storage_class,driver,volume_mode,pv_name,requested_bytes,bound_bytes,provisioned_bytes,filesystem_capacity_bytes,used_bytes,available_bytes,used_percent,inodes_total,inodes_used,inodes_free,inode_percent,metrics_status,metrics_reason,metrics_source,observed_at,active_consumer_count,terminating_consumer_count,severity
pvc,view-storage-test,classless,d6a9315f-26bc-42a1-be54-02272a52f9c3,Pending,,,Filesystem,,134217728,,,,,,,,,,,not_requested,,,,0,0,info
pvc,view-storage-test,data,5c022956-2086-4c92-b410-093ad56ce1e4,Bound,view-storage-local,rancher.io/local-path,Filesystem,pvc-5c022956-2086-4c92-b410-093ad56ce1e4,134217728,134217728,134217728,,,,,,,,,not_requested,,,,3,1,
pvc,view-storage-test,pending,559266f5-b76c-4310-a6ef-64c8028c5fd1,Pending,deliberately-unavailable,,Filesystem,,134217728,,,,,,,,,,,not_requested,,,,0,0,info
pvc,view-storage-test,unreferenced,df4de0ed-40d9-4aa4-8b0d-7fe8222f096e,Bound,view-storage-static,kubernetes.io/no-provisioner,Filesystem,view-storage-unreferenced,134217728,134217728,134217728,,,,,,,,,not_requested,,,,0,0,info
```

## JSON

```sh
kubectl view-storage -n view-storage-test -o json
```

[Complete unmodified JSON stdout](report.json) contains the query, collection costs, record/consumer relationships, groups, findings, and diagnostics. The [live CSI examples](../csi/README.md#json) show a readable JSON projection with measured filesystem and inode values.

## Strict metrics check

Inventory succeeds even though this local provisioner did not report a filesystem sample for `data`, and the other claims have no live consumers. Requiring complete metrics is inconclusive and returns exit 4 while retaining the report.

```sh
kubectl view-storage -n view-storage-test --require-metrics --view issues
```

```text
CONTEXT kind-view-storage-inventory   SCOPE view-storage-test   KIND pvc
SEVERITY OBJECT CODE EVIDENCE
info view-storage-test/classless claim_pending phase=Pending age_seconds=65
  Investigate: Inspect provisioning and delayed binding conditions.
info view-storage-test/classless metrics_missing filesystem.status=missing filesystem.reason=no_live_consumer inodes.status=missing inodes.reason=no_live_consumer
  Investigate: Inspect observation provenance, freshness, visibility, and collector diagnostics.
info view-storage-test/classless no_active_consumer active_consumer_count=0 terminating_consumer_count=0
  Investigate: Review visible pod references and ownership; this does not establish deletion safety.
info view-storage-test/data metrics_missing filesystem.status=missing filesystem.reason=no_sample inodes.status=missing inodes.reason=no_sample
  Investigate: Inspect observation provenance, freshness, visibility, and collector diagnostics.
info view-storage-test/pending claim_pending phase=Pending age_seconds=65
  Investigate: Inspect provisioning and delayed binding conditions.
info view-storage-test/pending metrics_missing filesystem.status=missing filesystem.reason=no_live_consumer inodes.status=missing inodes.reason=no_live_consumer
  Investigate: Inspect observation provenance, freshness, visibility, and collector diagnostics.
info view-storage-test/pending no_active_consumer active_consumer_count=0 terminating_consumer_count=0
  Investigate: Review visible pod references and ownership; this does not establish deletion safety.
info view-storage-test/unreferenced metrics_missing filesystem.status=missing filesystem.reason=no_live_consumer inodes.status=missing inodes.reason=no_live_consumer
  Investigate: Inspect observation provenance, freshness, visibility, and collector diagnostics.
info view-storage-test/unreferenced no_active_consumer active_consumer_count=0 terminating_consumer_count=0
  Investigate: Review visible pod references and ownership; this does not establish deletion safety.
SELECTION complete; matched=4 unknown=0
METRICS 0/4 known-match eligible claims fresh and complete
DIAGNOSTIC not_found: storageclasses collection failed (HTTP status 404); inspect permissions, API availability, and collection limits.
FILESYSTEM TOTALS per-claim reported filesystem totals; physical_total=false
```

Exit code: `4`.

## Cleanup

```sh
python3 scripts/dev-cluster.py delete inventory
```

The cluster used for these captures was deleted after collection. This fixture demonstrates inventory and missing-telemetry behavior; it does not establish filesystem metric support for the local provisioner.
