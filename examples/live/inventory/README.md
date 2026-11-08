# Real inventory examples

These are unedited outputs from the compiled plugin, invoked through kubectl against an actual disposable kind cluster. They are separate from the fixed [contract scenarios](../../contract/README.md).

The fixture contains two Bound and two Pending claims, four references to the `data` claim, Available/Released PVs, and a deliberately missing storage class. Diagnostics and unknown values below are preserved from the run.

## Environment

Captured 2026-11-08T02:58:12.436788+00:00 using Kubernetes v1.37.0, kubectl v1.37.2, and kubectl-view-storage 0.1.0-dev on macOS ARM64. [Capture metadata](capture.json) records the exact base revision, exact production-source fingerprint, binary hash, commands, exit codes, and stdout hashes. Names, UIDs, node placement, and ages will differ on another run. The capture records the uncommitted presentation revision through its production-source fingerprint. Each command performs its own query.

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
Persistent volume claims
Context: kind-view-storage-inventory  |  Namespace: view-storage-test

STORAGE                          COUNT  REQUESTED  PROVISIONED
-------------------------------  -----  ---------  -----------
Class: deliberately-unavailable      1    128 MiB          --*
`- Namespace: view-storage-test      1    128 MiB          --*
   `- PVC: pending                   1    128 MiB          --*
Class: view-storage-local            1    128 MiB      128 MiB
`- Namespace: view-storage-test      1    128 MiB      128 MiB
   `- PVC: data                      1    128 MiB      128 MiB
Class: view-storage-static           1    128 MiB      128 MiB
`- Namespace: view-storage-test      1    128 MiB      128 MiB
   `- PVC: unreferenced              1    128 MiB      128 MiB
Class: (classless)                   1    128 MiB          --*
`- Namespace: view-storage-test      1    128 MiB          --*
   `- PVC: classless                 1    128 MiB          --*

Selection: complete (4 matched)
Metrics: not requested (use -u to collect usage).
* Partial capacity: sum of known values; unknown values are excluded.
Findings: 5 info (details: --view issues)
Collection note [not_found]: storageclasses collection failed (HTTP status 404); inspect permissions, API availability, and collection limits.
```

Exit code: `0`.

## Table

```sh
kubectl view-storage -n view-storage-test --view table
```

```text
Persistent volume claims
Context: kind-view-storage-inventory  |  Namespace: view-storage-test

PVC           PHASE    PODS  REQUESTED  PROVISIONED
------------  -------  ----  ---------  -----------
classless     Pending     0    128 MiB           --
data          Bound    3+1d    128 MiB      128 MiB
pending       Pending     0    128 MiB           --
unreferenced  Bound       0    128 MiB      128 MiB
Pods: active + deleting (d); references do not multiply volume capacity.

Selection: complete (4 matched)
Metrics: not requested (use -u to collect usage).
* Partial capacity: sum of known values; unknown values are excluded.
Findings: 5 info (details: --view issues)
Collection note [not_found]: storageclasses collection failed (HTTP status 404); inspect permissions, API availability, and collection limits.
```

Exit code: `0`.

## Summary

The four claims request 512 MiB in total. Only two have known provisioned capacity, totaling 256 MiB; the report keeps that total partial.

```sh
kubectl view-storage -n view-storage-test --view summary
```

```text
Persistent volume claims
Context: kind-view-storage-inventory  |  Namespace: view-storage-test

GROUP                            COUNT  REQUESTED  PROVISIONED  PHASES
-------------------------------  -----  ---------  -----------  ------------------
TOTAL                                4    512 MiB     256 MiB*  2 Bound, 2 Pending
Class: deliberately-unavailable      1    128 MiB          --*  1 Pending
Class: view-storage-local            1    128 MiB      128 MiB  1 Bound
Class: view-storage-static           1    128 MiB      128 MiB  1 Bound
Class: (classless)                   1    128 MiB          --*  1 Pending

Selection: complete (4 matched)
Metrics: not requested (use -u to collect usage).
* Partial capacity: sum of known values; unknown values are excluded.
Findings: 5 info (details: --view issues)
Collection note [not_found]: storageclasses collection failed (HTTP status 404); inspect permissions, API availability, and collection limits.
```

Exit code: `0`.

## Issues

These newly created Pending and unreferenced claims produce informational findings. The missing class is a collection diagnostic; it does not make base inventory disappear.

```sh
kubectl view-storage -n view-storage-test --view issues
```

```text
Persistent volume claims
Context: kind-view-storage-inventory  |  Namespace: view-storage-test

info  view-storage-test/classless: Claim pending [claim_pending]
  Phase: Pending
  Age seconds: 37
  Observed: 2026-11-08T02:58:12.915534Z via -- on --
  Next: Inspect provisioning and delayed binding conditions.

info  view-storage-test/classless: No active consumer [no_active_consumer]
  Active consumer count: 0
  Terminating consumer count: 0
  Next: Review visible pod references and ownership; this does not establish deletion safety.

info  view-storage-test/pending: Claim pending [claim_pending]
  Phase: Pending
  Age seconds: 37
  Observed: 2026-11-08T02:58:12.915534Z via -- on --
  Next: Inspect provisioning and delayed binding conditions.

info  view-storage-test/pending: No active consumer [no_active_consumer]
  Active consumer count: 0
  Terminating consumer count: 0
  Next: Review visible pod references and ownership; this does not establish deletion safety.

info  view-storage-test/unreferenced: No active consumer [no_active_consumer]
  Active consumer count: 0
  Terminating consumer count: 0
  Next: Review visible pod references and ownership; this does not establish deletion safety.


Selection: complete (4 matched)
Metrics: not requested (use -u to collect usage).
* Partial capacity: sum of known values; unknown values are excluded.
Findings: 5 info (details: --view issues)
Collection note [not_found]: storageclasses collection failed (HTTP status 404); inspect permissions, API availability, and collection limits.
```

Exit code: `0`.

## Declared mounts and consumers

The shared consumer declares two paths, while retaining one consumer edge. The terminating and unscheduled references remain visible.

```sh
kubectl view-storage -n view-storage-test --pvc data --view table --wide
```

```text
Persistent volume claims
Context: kind-view-storage-inventory  |  Namespace: view-storage-test

PVC   PHASE  PODS  REQUESTED  PROVISIONED
----  -----  ----  ---------  -----------
data  Bound  3+1d    128 MiB      128 MiB
Pods: active + deleting (d); references do not multiply volume capacity.

Declared consumers (runtime unverified)
  view-storage-test/data
    PV: pvc-5c33af42-0f0f-417a-b054-bd4d98eadd41  |  Driver: rancher.io/local-path
    Mode: Filesystem  |  Access: ReadWriteOnce  |  Reclaim: Delete
    Pod: view-storage-test/shared-consumer  |  Node: view-storage-inventory-worker  |  State: active
      Container: app  |  Volume: alias
      Mount: /alias  |  Read-only: yes
      Container: app  |  Volume: data
      Mount: /data  |  Read-only: yes
    Pod: view-storage-test/terminating-consumer  |  Node: view-storage-inventory-worker  |  State: terminating
      Container: app  |  Volume: data
      Mount: /data  |  Read-only: yes
    Pod: view-storage-test/unscheduled-consumer  |  Node: --  |  State: active
      Container: app  |  Volume: data
      Mount: /data  |  Read-only: yes
    Pod: view-storage-test/consumer  |  Node: view-storage-inventory-worker  |  State: active
      Container: app  |  Volume: data
      Mount: /data  |  Read-only: yes

Selection: complete (1 matched)
Metrics: not requested (use -u to collect usage).
```

Exit code: `0`.

## Grouping by node and pod

The same claim appears under multiple references. Sibling reference groups cannot be added together; the distinct root capacity stays 128 MiB.

```sh
kubectl view-storage -n view-storage-test --pvc data -g node,pod,pvc
```

```text
Persistent volume claims
Context: kind-view-storage-inventory  |  Namespace: view-storage-test

STORAGE                                         COUNT  REQUESTED  PROVISIONED
----------------------------------------------  -----  ---------  -----------
Node: view-storage-inventory-worker                 1    128 MiB      128 MiB
|- Pod: view-storage-test/consumer                  1    128 MiB      128 MiB
|  `- PVC: data                                     1    128 MiB      128 MiB
|- Pod: view-storage-test/shared-consumer           1    128 MiB      128 MiB
|  `- PVC: data                                     1    128 MiB      128 MiB
`- Pod: view-storage-test/terminating-consumer      1    128 MiB      128 MiB
   `- PVC: data                                     1    128 MiB      128 MiB
Node: (no live consumer)                            1    128 MiB      128 MiB
`- Pod: view-storage-test/unscheduled-consumer      1    128 MiB      128 MiB
   `- PVC: data                                     1    128 MiB      128 MiB

Selection: complete (1 matched)
Metrics: not requested (use -u to collect usage).
Shared references: do not add sibling totals; the root counts each volume once.
```

Exit code: `0`.

## PV inventory

Available and Released volumes have no invented claim request quantities.

```sh
kubectl view-storage -A --kind pv --view table
```

```text
Persistent volumes
Context: kind-view-storage-inventory  |  Namespaces: all

PV                                                          PHASE      PODS  REQUESTED  PROVISIONED
----------------------------------------------------------  ---------  ----  ---------  -----------
view-storage-test/pvc-5c33af42-0f0f-417a-b054-bd4d98eadd41  Bound      3+1d    128 MiB      128 MiB
view-storage-available                                      Available     0         --      128 MiB
view-storage-test/view-storage-released                     Released      0         --      128 MiB
view-storage-test/view-storage-unreferenced                 Bound         0    128 MiB      128 MiB
Pods: active + deleting (d); references do not multiply volume capacity.

Selection: complete (4 matched)
Metrics: not requested (use -u to collect usage).
* Partial capacity: sum of known values; unknown values are excluded.
Findings: 4 info (details: --view issues)
Collection note [not_found]: persistentvolumeclaims collection failed (HTTP status 404); inspect permissions, API availability, and collection limits.
```

Exit code: `0`.

## CSV

This is the complete captured CSV, including its header. Empty numeric cells represent unknown values; they are not zero. Detailed relationships and diagnostics are available in JSON.

```sh
kubectl view-storage -n view-storage-test -o csv
```

```csv
kind,namespace,name,uid,phase,storage_class,driver,volume_mode,pv_name,requested_bytes,bound_bytes,provisioned_bytes,filesystem_capacity_bytes,used_bytes,available_bytes,used_percent,inodes_total,inodes_used,inodes_free,inode_percent,metrics_status,metrics_reason,metrics_source,observed_at,active_consumer_count,terminating_consumer_count,severity
pvc,view-storage-test,classless,0ad6e8f4-59a6-4050-aeb1-698220f1051b,Pending,,,Filesystem,,134217728,,,,,,,,,,,not_requested,,,,0,0,info
pvc,view-storage-test,data,5c33af42-0f0f-417a-b054-bd4d98eadd41,Bound,view-storage-local,rancher.io/local-path,Filesystem,pvc-5c33af42-0f0f-417a-b054-bd4d98eadd41,134217728,134217728,134217728,,,,,,,,,not_requested,,,,3,1,
pvc,view-storage-test,pending,89bf05bd-57d6-44bc-a7fb-6d5fbacae6d3,Pending,deliberately-unavailable,,Filesystem,,134217728,,,,,,,,,,,not_requested,,,,0,0,info
pvc,view-storage-test,unreferenced,5265f963-7be8-47e3-a266-b8f7ec6f1504,Bound,view-storage-static,kubernetes.io/no-provisioner,Filesystem,view-storage-unreferenced,134217728,134217728,134217728,,,,,,,,,not_requested,,,,0,0,info
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
Persistent volume claims
Context: kind-view-storage-inventory  |  Namespace: view-storage-test

info  view-storage-test/classless: Claim pending [claim_pending]
  Phase: Pending
  Age seconds: 38
  Observed: 2026-11-08T02:58:13.23084Z via -- on --
  Next: Inspect provisioning and delayed binding conditions.

info  view-storage-test/classless: Metrics missing [metrics_missing]
  Filesystem status: missing
  Filesystem reason: no_live_consumer
  Inodes status: missing
  Inodes reason: no_live_consumer
  Next: Inspect observation provenance, freshness, visibility, and collector diagnostics.

info  view-storage-test/classless: No active consumer [no_active_consumer]
  Active consumer count: 0
  Terminating consumer count: 0
  Next: Review visible pod references and ownership; this does not establish deletion safety.

info  view-storage-test/data: Metrics missing [metrics_missing]
  Filesystem status: missing
  Filesystem reason: no_sample
  Inodes status: missing
  Inodes reason: no_sample
  Next: Inspect observation provenance, freshness, visibility, and collector diagnostics.

info  view-storage-test/pending: Claim pending [claim_pending]
  Phase: Pending
  Age seconds: 38
  Observed: 2026-11-08T02:58:13.23084Z via -- on --
  Next: Inspect provisioning and delayed binding conditions.

info  view-storage-test/pending: Metrics missing [metrics_missing]
  Filesystem status: missing
  Filesystem reason: no_live_consumer
  Inodes status: missing
  Inodes reason: no_live_consumer
  Next: Inspect observation provenance, freshness, visibility, and collector diagnostics.

info  view-storage-test/pending: No active consumer [no_active_consumer]
  Active consumer count: 0
  Terminating consumer count: 0
  Next: Review visible pod references and ownership; this does not establish deletion safety.

info  view-storage-test/unreferenced: Metrics missing [metrics_missing]
  Filesystem status: missing
  Filesystem reason: no_live_consumer
  Inodes status: missing
  Inodes reason: no_live_consumer
  Next: Inspect observation provenance, freshness, visibility, and collector diagnostics.

info  view-storage-test/unreferenced: No active consumer [no_active_consumer]
  Active consumer count: 0
  Terminating consumer count: 0
  Next: Review visible pod references and ownership; this does not establish deletion safety.


Selection: complete (4 matched)
Metrics: 0/4 claims fresh and complete; inodes: 0/4
Bytes unavailable: 3 no live consumer, 1 no sample
Inodes unavailable: 3 no live consumer, 1 no sample
* Partial capacity: sum of known values; unknown values are excluded.
Findings: 9 info (details: --view issues)
Collection note [not_found]: storageclasses collection failed (HTTP status 404); inspect permissions, API availability, and collection limits.
Filesystem totals are per claim, not physical storage totals.
```

Exit code: `4`.

## Cleanup

```sh
python3 scripts/dev-cluster.py delete inventory
```

The cluster used for these captures was deleted after collection. This fixture demonstrates inventory and missing-telemetry behavior; it does not establish filesystem metric support for the local provisioner.
