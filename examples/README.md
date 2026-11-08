# Usage examples

Run these commands after following the [source build instructions](../README.md#build-from-source). Cluster queries use the current kubeconfig unless `--kubeconfig` or `--context` is supplied.

## Inventory and relationships

```sh
kubectl view-storage
kubectl view-storage -n production
kubectl view-storage -A --kind pv
kubectl view-storage --pvc data-postgres-0 --wide
kubectl view-storage -A -g storageclass,namespace,pvc
```

The default is PVC inventory in the current namespace. `-A` selects all namespaces; PV mode includes volumes without a bound claim. Grouping and mount details describe visible references without multiplying a claim's capacity.

## Usage and views

```sh
kubectl view-storage -A -u
kubectl view-storage -A -u --view table --wide
kubectl view-storage -A -u --view summary
kubectl view-storage -A -u --view issues
```

`-u` requests live filesystem and inode observations. Missing permissions or unsupported driver metrics remain explicit in the report. Requested, provisioned, and filesystem capacity represent different measurements.

## Exports and checks

```sh
kubectl view-storage -A -u -o json
kubectl view-storage -A -u -o csv
kubectl view-storage -A --require-metrics
kubectl view-storage -A -u --fail-on critical
```

Strict checks preserve the report. Inconclusive checks exit 4; a triggered finding policy exits 3. See the [specification](../docs/SPEC.md) for complete exit precedence and options.

## Sample reports

The [contract scenarios](contract/README.md) include sanitized inputs, complete JSON reports, and tree, table, summary, and issues output. Start with [healthy storage](contract/healthy/report.json), [shared RWX storage](contract/shared-rwx/report.json), or [partial permissions](contract/partial-permissions/report.json).

These are fixed examples for the report contract. They do not require a running cluster and do not establish support for a particular storage provider. See [compatibility](../docs/COMPATIBILITY.md) for live evidence.
