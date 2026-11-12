# Command and output examples

The [live examples](live/README.md) contain actual plugin output from disposable Kubernetes clusters. Each guide identifies its invocation, output representation and capture provenance. The [calculation examples](live/calculations/README.md) include direct executable captures and equivalent kubectl commands, real StatefulSet scope, decimal units and observed growth.

| What to inspect | Command | Actual output |
| --- | --- | --- |
| Default inventory tree | `kubectl view-storage -n view-storage-test` | [Tree](live/inventory/README.md#default-tree) |
| Claims in a table | `kubectl view-storage -n view-storage-test --view table` | [Table](live/inventory/README.md#table) |
| Logical capacity totals | `kubectl view-storage -n view-storage-test --view summary` | [Summary](live/inventory/README.md#summary) |
| Sums, class distributions and units | `kubectl view-storage -A --view stats --units GB` | [Actual unit output](live/calculations/README.md#sums-in-decimal-units) |
| StatefulSet storage and observed growth | `kubectl view-storage -n database --statefulset postgres --view stats` | [Real StatefulSet and growth](live/calculations/README.md) |
| Pending and unreferenced claims | `kubectl view-storage -n view-storage-test --view issues` | [Issues](live/inventory/README.md#issues) |
| Mounts and consumers | `kubectl view-storage -n view-storage-test --pvc data --view table --wide` | [Wide details](live/inventory/README.md#declared-mounts-and-consumers) |
| Shared references | `kubectl view-storage -n view-storage-test --pvc data -g node,pod,pvc` | [Grouping](live/inventory/README.md#grouping-by-node-and-pod) |
| Bound and unbound volumes | `kubectl view-storage -A --kind pv --view table` | [PV inventory](live/inventory/README.md#pv-inventory) |
| Measured filesystem usage | `kubectl view-storage -n view-storage-csi -u --view table` | [Live usage](live/csi/README.md#table) |
| Usage totals and coverage | `kubectl view-storage -n view-storage-csi -u --view summary` | [Usage summary](live/csi/README.md#summary) |
| Structured measurements | `kubectl view-storage -n view-storage-csi -u -o json` | [JSON](live/csi/README.md#json) |
| Spreadsheet export | `kubectl view-storage -n view-storage-csi -u -o csv` | [CSV](live/csi/README.md#csv) |
| Complete metrics check | `kubectl view-storage -n view-storage-csi -u --require-metrics --view summary` | [Exit 0](live/csi/README.md#strict-metrics-check) |
| Missing metrics check | `kubectl view-storage -n view-storage-test --require-metrics --view issues` | [Exit 4](live/inventory/README.md#strict-metrics-check) |

Follow each guide's setup first: the two fixtures use separate kubeconfigs. See [building](../README.md#build-from-source), [development](../docs/DEVELOPMENT.md), and the [CLI specification](../docs/SPEC.md) for prerequisites and complete options.

## Contract scenarios

The separate [contract examples](contract/README.md) are fixed inputs and expected reports for testing. They cover exact thresholds, shared RWX storage, partial permissions, stale/missing observations, and conflicting measurements. They are not recordings from the live clusters.
