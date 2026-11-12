# Real storage calculation output

Captured on 2026-11-11 from the final release binary against an owned disposable kind Kubernetes 1.37.0 cluster with CSI hostpath. These are actual plugin results: text and CSV files preserve stdout; JSON files are pretty-printed parsed stdout. The [capture record](capture.json) records file hashes, verified binary/source hashes, successful exit codes, capture times, and the commands reconstructed from the successful harness calls. Paths use repository-relative spelling. The executable was invoked directly; the equivalent kubectl form uses the same arguments after `kubectl view-storage`.

The tested source was uncommitted on base revision `5f372dc`. Its production fingerprint is `fb285936f9dd0b1c5e07e899543715b63275d35f5d161df97a3e3095e1df495d` and binary SHA-256 is `8bb9502af8a297d9c7d8242bdd98bd1f4649c3f31e6de3ed55bb582d109de1f5`. The real StatefulSet had two replicas sharing one 1Gi claim. All checks passed, and its StatefulSet and kind cluster were deleted. The suite also verified that ordinary user kubeconfigs stayed unchanged.

## Sums in decimal units

```sh
kubectl view-storage --kubeconfig .local/kubeconfigs/csi.yaml \
  -n view-storage-csi --view stats --pvc data --units GB
```

Excerpt from actual stdout:

```text
Persistent volume claims
Context: kind-view-storage-csi  |  Namespace: view-storage-csi

Storage calculations (units: GB; logical capacity)

GROUP             COUNT  REQUESTED    BOUND  PROVISIONED
----------------  -----  ---------  -------  -----------
TOTAL                 1    1.07 GB  1.07 GB      1.07 GB
view-storage-csi      1    1.07 GB  1.07 GB      1.07 GB

```

Full output: [MB](mb.txt), [GB](gb.txt), [TB](tb.txt). The TB view preserves the small positive value as `0.001074 TB`, rather than displaying a zero.

## Multiple storage classes and types

A second owned kind inventory/local-path run (`e26a2a7d-ea43-42d1-9ca9-51d829027acc`) captured five real claims across three named classes and one classless bucket. The unavailable provisioner and unbound claims are intentional fixture cases: requested totals remain known, while missing bound/provisioned quantities are marked incomplete. The independent validator checks the exact class distributions against these actual records.

```sh
kubectl view-storage --kubeconfig .local/kubeconfigs/inventory.yaml \
  -A --view stats --units GB
```

Excerpt from actual stdout:

```text
GROUP                     COUNT  REQUESTED     BOUND  PROVISIONED
------------------------  -----  ---------  --------  -----------
TOTAL                         5    0.67 GB  0.40 GB*     0.40 GB*
deliberately-unavailable      1    0.13 GB       --*          --*
view-storage-local            2    0.27 GB   0.27 GB      0.27 GB
view-storage-static           1    0.13 GB   0.13 GB      0.13 GB
(classless)                   1    0.13 GB       --*          --*

```

Full [class statistics](inventory-classes.txt), [class JSON](inventory-classes.json), [class CSV](inventory-classes.csv), [access/volume-mode text](inventory-types.txt) and [type JSON](inventory-types.json) preserve the real results. This live fixture exercises RWO Filesystem declarations; separate actual HTTP tests cover Block and other access modes. The inventory cluster was deleted and user kubeconfigs stayed unchanged.

## StatefulSet storage

The fixture created StatefulSet `calculation-db-7d32aae5` with two real running pods that reference the same claim. Its calculation retained one claim and 1Gi total capacity.

```sh
kubectl view-storage --kubeconfig .local/kubeconfigs/csi.yaml \
  -n view-storage-csi --view stats --statefulset calculation-db-7d32aae5 \
  --units GiB
```

See [actual terminal output](statefulset.txt), [CSV](statefulset.csv), and [JSON with StatefulSet/access-mode/volume-mode groups](statefulset.json). The JSON command explicitly used `-g statefulset,accessmode,volumemode`; text and CSV used the default class breakdown. StatefulSet filtering selects claims and retains their other visible consumers; other live consumers can therefore produce an overlapping `(none)` StatefulSet group in JSON.

## Observed growth

The existing CSI gate removed its probe, collected a [fresh baseline](before.json), performed a bounded fsynced 16MiB write, and waited for advancing byte counters. The calculation then compared the actual later sample with that baseline:

```sh
kubectl view-storage --kubeconfig .local/kubeconfigs/csi.yaml \
  -n view-storage-csi --view stats --pvc data --units GiB \
  --compare BEFORE.json --require-metrics
```

Excerpt from actual stdout:

```text
Observed growth (paired samples; first full means earliest claim)

GROUP             GROWTH / HOUR  FIRST FULL IN  PAIRS  STATUS
----------------  -------------  -------------  -----  --------
TOTAL                  0.73 GiB         1.11 d    1/1  Complete
view-storage-csi       0.73 GiB         1.11 d    1/1  Complete
Two-sample linear estimates assume unchanged growth; they are not forecasts of backend capacity.

```

Full [terminal output](growth.txt), [JSON](growth.json), and [CSV](growth.csv) retain coverage and the projection. The verified JSON pair observed `16916480` additional used bytes over `78000000000` nanoseconds. CSI hostpath reports its backing filesystem, so observed growth can include filesystem overhead and other backing-storage activity. This is a two-sample linear estimate, not an application growth forecast or physical backend total. Separate text/CSV commands can observe later driver samples; each output retains its own reported values.

## Reproduce and validate

```sh
cargo build --locked --release
python3 scripts/e2e-storage.py --profiles csi-hostpath \
  --binary target/release/kubectl-view_storage
```

The suite captures a new baseline and new identifiers under `artifacts/RUN-ID`. Old snapshots cannot establish growth for a recreated claim in another disposable cluster. To validate these committed captures independently:

```sh
uv run scripts/validate-calculations.py \
  examples/live/calculations/growth.json \
  examples/live/calculations/statefulset.json \
  --baseline examples/live/calculations/before.json --negative-checks
```

See [the calculation guide](../../../docs/CALCULATIONS.md) for class-total distributions, access/volume modes, capacity gaps, exact arithmetic and comparison limits.
