# Watch, Prometheus and workload attribution

These features use the same inventory, UID accounting, coverage, findings, and exit policies as single collections. They add no runtime dependencies and perform no cluster writes.

## Watch and automatic history

```sh
kubectl view-storage -n database --view stats --watch 10s --units GiB
kubectl view-storage -A -u --view table --watch 30s
kubectl view-storage -n database --view stats --watch 10s --watch-count 6 -o json
```

Cycles run sequentially. Each collection gets its own `--timeout`; the interval starts after that cycle has finished writing. `--watch-count N` optionally bounds the number of cycles. Ctrl-C cancels the current operation or interval and exits 130. A fatal collection stops with exit 1. A strict finding or inconclusive strict check writes its completed report, then stops with exit 3 or 4.

Terminal text replaces the preceding view after a complete collection. Redirected text appends reports without terminal controls. JSON watch output contains one compact complete JSON document per line; ordinary JSON remains pretty printed. CSV watch output is rejected. Each watch report contains cycle/interval metadata and retains its actual collection and observation times.

Stats watch implies utilization collection. The first cycle has no growth estimate; each subsequent cycle compares with the preceding completed report. `--compare FILE` can seed the first cycle. History retains one preceding report in memory and creates no history files. Missing, conflicting or repeated samples do not produce rates. Two consecutive usable observations are needed after a gap. See [the calculation contract](CALCULATIONS.md) for sample pairing, exact rational rates, and projections.

## Explicit Prometheus collection

```sh
kubectl view-storage -n database --view stats \
  --metrics-source prometheus \
  --prometheus-url https://prometheus.example.org \
  --prometheus-cluster cluster=production \
  --prometheus-token-file /path/to/token \
  --prometheus-ca-file /path/to/ca.pem
```

Selecting Prometheus implies `-u`. An explicit URL and cluster scope are required. Use `--prometheus-cluster LABEL=VALUE` for an exact cluster label matcher. For an endpoint containing only this Kubernetes cluster, `--prometheus-single-cluster` explicitly asserts that scope instead. A kubeconfig context name is never used as a monitoring label. No endpoint is discovered and no credentials are taken from Kubernetes configuration.

The provider uses the [Prometheus query API](https://prometheus.io/docs/prometheus/latest/querying/api/) with a range-vector selector for the six `kubelet_volume_stats_*` byte and inode metrics. Raw matrix sample timestamps are preserved; query evaluation time never substitutes for sample observation time. The range is the maximum sample age plus the existing 30-second clock-skew allowance. Requests and response reading use the existing deadlines, retry policy and 32 MiB body cap.

Each tuple belongs to one claim, scrape label set and timestamp. Fields from different scrapes are not assembled into a fabricated complete observation. Identical replica observations are deduplicated, retaining the newest matching tuple; differing tuples go through the existing conflict rules. Replicas and scrape jobs are never summed. Filesystem totals remain logical per-claim measurements, with shared-backend overlap exclusions unchanged.

Integral decimal and scientific sample strings are parsed exactly into u64 counters. Nonintegral, negative, nonfinite and out-of-range values remain invalid rather than being rounded to zero. Prometheus stores float samples, so precision already lost by the telemetry provider cannot be recovered by the plugin.

Samples match the selected namespace/PVC and cluster. Sample times must not precede the current PVC creation time. When present, `pvc_uid` or `persistentvolumeclaim_uid` must match the current PVC UID. Multiple observed cluster labels under a purported single-cluster scope make attribution uncertain. Missing creation/binding evidence, stale samples and conflicting observations cannot pass required metrics. Standard metrics may lack PVC UIDs: their association relies on explicit scope, claim name and creation-time checks; they cannot prove backend identity independently.

Prometheus provenance names the provider and claim. It never invents a pod UID or pod volume reference. Node labels are retained when available. Workload/consumer information still comes from Kubernetes.

Bearer-token files are optional, UTF-8, nonempty regular files of at most 64 KiB. They are read when this provider is selected and the client is constructed. Token rotation requires restarting the command. Tokens require HTTPS except for explicit loopback test endpoints. Custom CA files are bounded to 1 MiB; certificate verification remains enabled. The separate client follows no HTTP redirects, carries no Kubernetes authentication, and performs no kubelet fallback after a provider failure. Provider failures preserve inventory, diagnostics and normal strict-metrics exit behavior.

## Verified workload owners

```sh
kubectl view-storage -n apps --workload deployment/api -u --view stats
kubectl view-storage -n batch --workload cronjob/backup --view table
kubectl view-storage -n apps --workload daemonset/agent
kubectl view-storage -n batch --workload job/backup-run
kubectl view-storage -A -g workload,namespace,pvc --view stats
```

`--workload KIND/NAME` supports `deployment`, `replicaset`, `daemonset`, `statefulset`, `job`, and `cronjob`. Filtering requires PVC mode in a single namespace and selects claims referenced by active or terminating pods. Any verified controller in the chain can match: a CronJob-owned Job can be selected by its Job name or CronJob name. It does not identify retained claims without a live pod reference or guess ownership from names or labels.

`-g workload` groups by the highest resolved supported controller, retaining its namespace, kind, name and UID. A shared claim appears once per group and once in the root total. Workload groups can overlap; do not add their totals together.

Resolution follows [controller owner references](https://kubernetes.io/docs/concepts/overview/working-with-objects/owners-dependents/): Pod → ReplicaSet → Deployment, Pod → Job → CronJob, or direct DaemonSet/StatefulSet/Job ownership. Every fetched object must match the referenced namespace, kind, API version, name and UID. Recreated names, inaccessible parents, invalid references, cycles and chains longer than eight remain unresolved. Reports expose verified partial chains and unknown owner state. Strict filtering never turns an unresolved owner into a successful empty selection.

Controller reads occur only when the workload filter or grouping is selected. They share the existing optional lookup budget, cache and deadlines; unrelated pods do not consume owner lookups. Relevant `get` permissions are needed for `deployments`, `replicasets`, `daemonsets`, and `statefulsets` in `apps`, and `jobs` and `cronjobs` in `batch`. No controller list or write permission is needed. The existing `--statefulset` option retains its declared-pod-owner semantics and does not require these additional reads.

## Validation

The executable HTTP suite covers watch framing, automatic history, Ctrl-C, terminal replacement, strict stops, controller chains and UID replacement, separate provider credentials, replica accounting, freshness, conflicts and redirect rejection. New reports are checked by the existing independent schema/arithmetic/relationship validators.

For a real Prometheus process with a controlled Kubernetes HTTP fixture and generated private TLS CA:

```sh
cargo build --locked
python3 scripts/verify-prometheus-live.py \
  --prometheus-binary /path/to/a/verified/prometheus
```

This starts only an owned native Prometheus and loopback fixture, validates trusted/untrusted TLS, actual scrape/query timestamps, workload selection and automatic growth arithmetic, then cleans up its processes and temporary files. It uses no user kubeconfig or Docker daemon. Evidence goes to `artifacts/prometheus-live`. This test does not establish compatibility with a live Kubernetes cluster or storage driver.
