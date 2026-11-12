# Storage calculations

Use `--view stats` to calculate logical storage totals, size distributions and observed growth. The default breakdown is by storage class. Inventory calculations work without `-u`; add it for filesystem usage. No additional runtime dependencies or monitoring service are required.

```sh
# All visible claims, summed and grouped by storage class.
kubectl view-storage -A --view stats --units GB

# Binary units and all requested/bound/provisioned size distributions.
kubectl view-storage -A --view stats --units GiB --wide

# Volume mode (Filesystem/Block) and access-mode breakdowns.
kubectl view-storage -A --view stats -g volumemode,accessmode

# Pending/bound/released capacity, or a particular class.
kubectl view-storage -A --view stats -g phase
kubectl view-storage -A --view stats --storage-class fast --units TB

# Claims referenced by live pods controlled by this StatefulSet.
kubectl view-storage -n database --statefulset postgres --view stats -u
kubectl view-storage -A --view stats -g statefulset

# Exact machine-readable calculations or a spreadsheet export.
kubectl view-storage -A --view stats -o json
kubectl view-storage -A --view stats --units MB -o csv
```

## Units and totals

`--units` (alias `--unit`) accepts `auto`, `B`, `KB`, `MB`, `GB`, `TB`, `PB`, `KiB`, `MiB`, `GiB`, `TiB`, or `PiB`, case insensitively. MB/GB/TB use powers of 1000; MiB/GiB/TiB use powers of 1024. All units measure bytes. `auto` selects binary units per value. Text and CSV display two decimal places, increasing to six for a small nonzero value that would otherwise round to zero; still smaller values show `<0.000001 UNIT`. Raw byte sums remain exact decimal strings in JSON and CSV.

Requested PVC capacity, bound PVC capacity, provisioned PV capacity and reported filesystem size are separate measurements. Each is summed independently. They must not be added together. Filesystem usage is calculated from fresh, complete, compatible observations using the existing coverage and overlap rules. Utilization is capacity-weighted, not an average of percentages. Raw block volumes contribute inventory capacity and have no filesystem usage.

The total counts each selected record UID once, regardless of pod count or repeated mounts. Pod, node and StatefulSet groups can overlap when they share a claim; their sums cannot be added to reconstruct the total. Existing overlap notices remain visible. These figures do not establish the free or physical capacity of a storage backend.

Each group has sum, known/total counts, minimum, maximum, arithmetic mean, median and nearest-rank P95 for requested, bound and provisioned sizes. Default text shows provisioned size distributions; `--wide` includes all three. Mean and median retain exact rational byte values in JSON; rounding affects display only. Empty selections have zero inventory sums and null distribution values. Missing quantities remain unknown, distinct from an observed zero. Incomplete selection or quantities mark results incomplete; distributions describe the known subset.

The named-class summary calculates these same statistics **across each class's total**, rather than across individual volumes. Its record-assignment counts distinguish named, classless, unassigned and unknown classes. The overall total includes all selected records; class distributions include named classes only. Unknown class assignments make the class statistics incomplete.

Capacity-gap calculations compare records where all three inventory quantities are known: requested above bound can indicate expansion or provisioning work; provisioned above requested shows a logical allocation difference. Neither value asserts reclaimable space or unused backend capacity. Phase grouping also makes Available/Released PV capacity inspectable with `-A --kind pv`.

## Types and StatefulSets

`-g accessmode` groups by the complete, sorted set of declared access modes, for example `ReadWriteOnce` or `ReadOnlyMany+ReadWriteMany`. A record with several modes belongs to one bucket. `-g volumemode` separately groups Filesystem/Block. Access modes describe Kubernetes declarations, not proof that a mount is writable or a filesystem is a particular type; see [Kubernetes volume modes and access modes](https://kubernetes.io/docs/concepts/storage/persistent-volumes/).

`--statefulset NAME` (alias `--sts`) selects claims referenced by active or terminating pods with an `apps/v1` StatefulSet controller owner reference. Filtering matches the declared controller name within one namespace; grouping retains each declared controller UID. It does not infer membership from PVC/pod naming guesses or fetch a StatefulSet object to resolve the current controller generation. No StatefulSet API read permission is needed by the plugin. If pod visibility or ownership is unresolved, selection remains incomplete. Grouping keeps the controller UID in its identity even though text labels omit it.

This scope describes currently referenced storage. Retained claims without a live controlled pod are outside it. It does not estimate future replicas, sum volume-claim templates, or infer StatefulSet ownership from a claim name. Kubernetes [StatefulSet retention behavior](https://kubernetes.io/docs/concepts/workloads/controllers/statefulset/#persistentvolumeclaim-retention) explains why retained claims cannot always be identified through a PVC owner reference.

## Observed growth and time to full

Save a utilization report, then compare a later collection with it:

```sh
kubectl view-storage -n database --statefulset postgres -u -o json > before.json

# Run after the driver has produced newer samples.
kubectl view-storage -n database --statefulset postgres \
  --view stats --compare before.json --units GiB
```

`--compare FILE` requires `--view stats` and implies utilization collection. The baseline must be a regular JSON report file of at most 32 MiB, in `view-storage/v1alpha1`, originally collected with utilization enabled. Context name, namespace/all-namespace scope, inventory kind and selection filters must match. Grouping, sorting, units and output format can differ. Use the same actual cluster; matching context names alone cannot prove two kubeconfigs refer to the same server.

Each pair must preserve record UID, name/namespace, creation timestamp, binding name, class, driver and volume mode, and have fresh complete usable byte tuples at their original collection times. Raw block, overlapping filesystem records, missing/conflicting/stale tuples, repeated/backward sample timestamps, new identities and resized filesystems do not produce rates. Per-record reasons and paired/eligible counts explain what is unavailable. Historical samples do not need to remain fresh at comparison time.

PV inventory reports also retain `pvc_name` and the verified `pvc_uid`. PV comparisons require both to match, so reusing a PV for a replacement claim cannot produce a growth estimate across the two claims. Older PV reports without claim identity do not provide usable comparison pairs.

The calculations use **sample timestamps**, not command execution time:

```text
growth bytes/second = (current used bytes - previous used bytes) / sample interval
time to full       = current available bytes / positive growth bytes/second
```

Negative growth means observed shrinkage; zero means steady. Neither yields a future-full estimate when available space remains. Within a usable pair, zero available bytes means full now. Group growth sums rates over paired records only and exposes completeness. `FIRST FULL IN` is the earliest individual claim estimate, so a shrinking claim cannot hide another claim approaching full. It is never combined free space divided by a combined rate.

These are two-sample linear projections, not trend fitting, guarantees or forecasts of physical backend exhaustion. Cleanup, workload bursts, filesystem reserves and a driver reporting its backing filesystem can change their meaning. No Prometheus connection, background watcher, automatic sampling delay or on-disk history database is introduced. Current collection exit policies remain unchanged; `--require-metrics` checks current metrics, not the completeness of historical pairs.

## Exports and validation

Stats JSON adds the optional `calculations` object, versioned `view-storage/calculations/v1alpha1`, to the existing report. It contains total/group rows, class distributions and optional per-record comparison evidence. Byte counters are strings; means, medians, rates and projected seconds use numerator/denominator string pairs. Arithmetic is checked and overflow is explicit. Very long projections or growth of exact aggregate denominators can exceed u128; affected aggregates stay unavailable rather than becoming floating-point approximations.

Stats CSV uses its own header: `row_type`, `group`, `measure`, `unit`, coverage/completeness fields, exact bytes/rational values, display values, rates, earliest-full time and a unique `group_id`. Rows cover inventory statistics, filesystem sums, capacity gaps, growth and class distributions. Ordinary CSV output without `--view stats` retains its existing record schema.

Rust tests cover accounting, units, unknowns, distributions, ownership, comparisons, overflow, cancellation and real executable exports. [The independent validator](../scripts/validate-calculations.py) recomputes results with Python integers and `Fraction`; it runs over actual HTTP exports in existing CI jobs. The calculation module has the same 90% line-coverage floor as other arithmetic/core modules. The existing CSI kind gate now also validates a real write's growth and a two-replica StatefulSet sharing one claim, with owned cleanup. No extra GitHub Actions job is needed.

See [captured calculation output](../examples/live/calculations/README.md) for real terminal, JSON and CSV examples.
