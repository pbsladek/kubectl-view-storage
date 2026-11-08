# kubectl-view-storage specification

Status: Draft 0.8, 2026-11-07. Companion: [design document](DESIGN.md). Product defaults and release priorities are decided. This is a contract for implementation, not a description of an existing executable. P0 means required for v0.1; P1 and P2 behavior is reserved for later milestones. Execution order and completion gates live in the separate [implementation plan](IMPLEMENTATION_PLAN.md); disposable test environments are defined in [local development](DEVELOPMENT.md).

MUST, SHOULD, and MAY express requirements, preferred behavior, and permitted implementation choices. The design explains the choices; this specification defines observable behavior. When drafts disagree, revise both before implementation rather than silently choosing a behavior.

## Invocation and scope

```text
kubectl view-storage [flags]
kubectl-view_storage [flags]
```

P0 MUST provide one query command without required subcommands or positional arguments. The executable is `kubectl-view_storage`; the Krew plugin name is `view-storage`.

The underscore in the executable filename is required for kubectl's dashed command-name discovery. Cargo installs/builds that canonical filename, while the crate/project name remains `kubectl-view-storage`. The [kubectl plugin naming rules](https://kubernetes.io/docs/tasks/extend-kubectl/kubectl-plugins/#names-with-dashes-and-underscores) define this mapping. Direct and plugin help/version/validation behavior must agree.

Place plugin flags after the plugin name, for example `kubectl view-storage --context staging -n demo`. The plugin cannot change kubectl's handling of flags before plugin dispatch. P0 supports the documented context/kubeconfig/namespace subset and kubeconfig credential helpers; do not imply that every kubectl global flag is implemented.

Defaults are PVC inventory, the selected kubeconfig context's namespace (or `default` if absent), `--view tree`, `--group-by storageclass,namespace,pvc`, `--output text`, and metrics disabled.

`--all-namespaces` and an explicitly supplied `--namespace` are mutually exclusive. Namespace names and object names use exact matching; no implicit regular expressions or substring matching. P0 and P1 MUST NOT load a project-specific configuration file or discover external monitoring endpoints. Use CLI flags and explicit credential files. A full interactive terminal UI is outside both milestones; P1 adds refresh mode to the existing views.

### Common flags

| Flag | Default | P0 behavior |
| --- | --- | --- |
| `-n, --namespace NAME` | Kubeconfig namespace | Select a namespace |
| `-A, --all-namespaces` | false | Select all namespaces |
| `--context NAME` | Current context | Select kubeconfig context |
| `--kubeconfig PATH` | Client's standard resolution | Select kubeconfig file |
| `-u, --utilization` | false | Collect filesystem and inode statistics |
| `-g, --group-by KEYS` | `storageclass,namespace,pvc` | Ordered comma-separated grouping dimensions |
| `--view NAME` | `tree` | `tree`, `table`, `summary`, or `issues` |
| `--wide` | false | Add volume, consumer, and declared mount details to text views |
| `-o, --output FORMAT` | `text` | `text`, `json`, or `csv` |
| `--kind KIND` | `pvc` | Base inventory: `pvc` or `pv` |
| `-l, --selector SELECTOR` | None | Kubernetes label selector on base inventory objects |
| `--pvc NAME` | None | PVC name filter; valid only in PVC mode |
| `--pv NAME` | None | PV name filter, or bound PV name in PVC mode |
| `--storage-class NAME` | None | Exact class name; empty string selects explicitly classless objects |
| `--driver NAME` | None | Exact CSI driver or known provisioner identifier |
| `--node NAME` | None | Filter by visible scheduled pod references, not physical storage location |
| `--sort FIELD[:asc\|desc]` | `name:asc` | Order leaf records and sibling groups |
| `-h, --help` | — | Show help without contacting a cluster |
| `-V, --version` | — | Show version without contacting a cluster |

`--kind pv` MUST require `-A` to make cluster scope explicit; it MUST reject `--namespace` and `--pvc`. Its default grouping is `storageclass,pv` when the user has not supplied `-g`. PV mode includes Available, Bound, Released, Failed, and terminating PVs. PVC mode includes all visible PVC phases and terminating claims. Neither mode invents records for objects the caller cannot see.

### Advanced flags

| Flag | Default | P0 behavior |
| --- | --- | --- |
| `--request-timeout DURATION` | `10s` | Deadline per Kubernetes request |
| `--timeout DURATION` | `60s` | Overall collection deadline |
| `--concurrency N` | `8` | Maximum concurrent outbound requests; range 1–64 |
| `--max-metrics-age DURATION` | `5m` | Maximum fresh sample age |
| `--warn-used PERCENT` | `80` | Warning threshold for bytes and inode utilization |
| `--critical-used PERCENT` | `90` | Critical threshold for bytes and inode utilization |
| `--require-metrics` | false | Implies `-u`; require complete selection and complete fresh byte metrics for eligible records |
| `--fail-on LEVEL` | Disabled | Return a finding exit status at `warning` or `critical` severity |
| `--color WHEN` | `auto` | `auto`, `always`, or `never`; text output only |

Durations accept positive integers followed by `ms`, `s`, `m`, or `h`. Thresholds accept ordinary decimal percentages with at most six fractional digits and MUST satisfy `0 < warning < critical <= 100`. Exponents, NaN, and infinity are invalid threshold inputs. Store thresholds as canonical decimal strings in JSON query metadata, with unnecessary trailing fractional zeros removed. Invalid flags or combinations MUST fail before network access.

P1 reserves `--metrics-source kubelet|prometheus`, `--prometheus-url URL`, `--prometheus-cluster LABEL=VALUE`, `--prometheus-token-file PATH`, `--prometheus-ca-file PATH`, and `--watch DURATION`. These MUST NOT appear as supported P0 flags. The Prometheus options imply `-u` and select Prometheus explicitly. P0 utilization always uses kubelet observations.

### Examples

```sh
kubectl view-storage
kubectl view-storage -A -u
kubectl view-storage -n database --pvc data-postgres-0 -u
kubectl view-storage -n database --pvc data-postgres-0 --wide
kubectl view-storage -A -g storageclass,namespace,pvc
kubectl view-storage -A -g node,pvc -u
kubectl view-storage -A -u --view table --sort used-percent:desc
kubectl view-storage -A --view summary
kubectl view-storage -A -u --view issues --fail-on critical
kubectl view-storage -A --kind pv --sort provisioned:desc
kubectl view-storage -n database -l app=postgres -o json
kubectl view-storage -A -o csv
```

## Collection contract

### Base inventory

The collector MUST paginate list responses, finish all pages before declaring inventory complete, and preserve Kubernetes object UIDs. It MUST NOT treat a truncated or failed base list as a complete empty result.

API resource envelopes and their metadata MUST be JSON objects; positional struct arrays are malformed API documents. Required malformed objects fail base inventory. Optional conversion failures retain independently valid sibling objects while making that resource's visibility incomplete. Detect repeated continuation tokens without discarding valid objects already received from optional pages.

Validate namespaced objects against the requested namespace even when its literal name is `cluster`. Display labels MUST NOT act as wildcard scope markers. An object from another namespace fails required inventory or makes optional visibility incomplete under the same malformed-object rules.

In PVC mode, listing PVCs in the selected scope is required. Pods are optional enrichment in the same scope. Referenced PVs and storage classes follow the bounded collection plan below. When PVC inventory succeeds but enrichment is forbidden or unavailable, return inventory plus explicit diagnostics and unknown enrichment fields.

In PV mode, listing PVs is required. Claim references are preserved directly from PVs. Getting referenced PVCs and listing consumer pods are optional enrichment. A failed PVC lookup does not prove a dangling claim unless the API returned an authoritative NotFound response.

Resource lists form a collection interval, not an atomic cross-resource snapshot. Include collection start/end times and source resource versions when available. A join that changes during collection produces a diagnostic and preserves the visible objects; it MUST NOT silently connect mismatched identities.

### Collection plan and budgets

Collection has three stages: required base inventory, optional consumer/metadata enrichment, and requested utilization. Independent requests MAY run concurrently within a stage. Required inventory MUST finish before optional work can consume its time budget.

| Limit | P0 contract |
| --- | --- |
| Overall deadline | `--timeout`, 60 seconds by default, includes all stages and retries |
| Request deadline | `--request-timeout`, 10 seconds by default, including response reading and JSON validation, clipped to stage/overall deadline |
| Concurrency | `--concurrency`, 8 by default, shared across requests |
| List page size | Request `limit=500`; follow the server's continuation tokens |
| Optional enrichment requests | At most 256 HTTP attempts, including retries and list pages |
| Enrichment time | From the end of base inventory, at most min(30 seconds, half the remaining overall time) with `-u`; otherwise at most min(30 seconds, remaining overall time) |
| Decoded response body | At most 32 MiB per response, enforced while streaming; oversized required pages are fatal, optional responses are diagnosed |
| JSON nesting | At most 128 object/array levels; excess depth follows the required/optional parsing failure path |
| Quantity/counter scalar input | At most 256 UTF-8 bytes per scalar, checked before expensive conversion; excess length is invalid data |
| Utilization requests | One logical request per distinct relevant node, at most three attempts per node within the remaining overall time |

Within enrichment, prioritize dependencies of the actual query, then utilization target discovery, then remaining display enrichment. First evaluate inventory-only filters. For remaining candidates, fetch the metadata needed to settle unresolved predicates: a CSI-driver predicate may require PVs, a provisioner predicate may require a storage class, and a node predicate requires consumer references. Fetch only prerequisite claim data before PV-mode consumer joins. Re-evaluate predicates as dependencies arrive and stop enriching known nonmatches.

A known CSI driver needs no class lookup to settle driver membership. Fetch class membership prerequisites only for unresolved drivers whose class provisioner can supply that classification; unavailable or conflicting bound-PV metadata cannot be repaired by reading its intended class. In PV mode resolve these driver prerequisites before fetching claims for potential matches, then recheck membership after claim verification. After consumer discovery, settle node predicates before requesting remaining display classes. Retain metadata that proved a nonmatch so later normalization cannot turn it back into an unknown, and reuse successful class objects for display without another list/get.

After membership dependencies, prioritize pod/node discovery for known matches when `-u` is requested. Then collect remaining consumer, PV, and class display fields. A request needed by several purposes is shared and charged once. Within a priority tier, order resources by API group/resource/scope/name and list pages by continuation sequence; concurrency MUST NOT determine which object receives the last budget slot. If a prerequisite fails, mark its predicates unknown and continue independent candidates instead of repeatedly probing it. The same priority rules apply under all text views and exports; rendering MUST NOT trigger collection.

Cache responses and failures by resource, scope, verb, and name; a denied list MUST NOT imply that get is denied, or vice versa. The report's cost metadata MUST include the priority sequence and the unresolved prerequisite names when a budget or deadline prevents completion. Driver-filtered queries MUST NOT spend their membership budget enumerating unrelated consumer pages before resolving driver metadata.

In PVC mode with `-A`, when at least 32 distinct PVs need enrichment, prefer one paginated PV list over individual gets. Use the same threshold for storage classes. In namespace-scoped mode, use targeted gets; do not automatically widen to cluster lists. A forbidden bulk list is attempted once per resource and falls back to permitted targeted gets within the same budget. Filter bulk results to referenced objects before storing the normalized graph. Required PV-mode listing is reused and MUST NOT be repeated for enrichment.

Budget exhaustion stops optional work with reason `budget_exhausted`; time exhaustion uses `deadline_exceeded`. Fields remain unknown and any affected selection becomes incomplete. Never issue a get for every claim after its budget has been exhausted. The report MUST include chosen strategies, attempt counts, node target counts, and response-byte counts, so request cost is observable.

All request, stage, run, retry, and shutdown durations MUST use a monotonic clock. UTC readings are used for observation/report timestamps and freshness only. Moving wall time forward or backward MUST NOT reset or extend a budget, retry wait, or deadline. Freshness uses one captured UTC reading for the report; invalid/future timestamps remain explicit rather than receiving a fabricated fresh time.

### Relationships

- A PVC record is keyed by its UID. Its display identity includes namespace and name.
- A PV record is keyed by its UID. In PVC mode, attach its capacity only when the PVC volume name matches the PV name and the PV claim reference matches the PVC namespace, name, and UID when populated. If the PV or claim reference is unavailable, retain the PVC's reported volume name as an unverified link.
- A PV claimRef UID mismatch MUST produce `binding_conflict`; do not assign that PV's capacity or observations to the wrong claim.
- Pod consumer edges originate from `spec.volumes[].persistentVolumeClaim`. Repeated references from containers or volume mounts MUST NOT duplicate a consumer or claim.
- If the PVC has a pod owner reference from generic ephemeral provisioning, preserve that origin. Inline ephemeral CSI volumes without PVCs are outside P0 inventory. JSON optionally exports `pod_owner` with the declared pod name, `pod:<uid>` identity when valid, and `relationship=declared_owner`. An owner reference alone does not assert a mounted consumer edge. Base records being deleted optionally retain `deletion_timestamp`.
- Record pod phase, deletion timestamp, and scheduled node when visible. A reference does not assert a successful mount or attachment.
- Consumer reference state is `active` for nonterminal pods without a deletion timestamp, `terminating` for nonterminal pods with a deletion timestamp, and `terminal` for Succeeded/Failed pods. The existing `active` boolean is true only in the first state. Report active and terminating counts separately. Metrics targets, `--node`, and pod/node grouping include scheduled active and terminating references; terminal references remain inspectable but do not supply collection targets.
- Exact active/terminating counts, including zero, MAY be reported only after a complete pod list in the claim's namespace. Otherwise both counts are unknown.
- An unfamiliar pod phase preserves the declared relationship with `reference_state=unknown` and `active=null`. Counts for affected records remain unknown even when enumeration completed. Only recognized live states supply telemetry targets; independent observations from recognized live consumers remain usable. Unrelated claims retain their known classifications and counts.

Each distinct record-to-pod relationship MUST retain the pod volume names and every declared mount or block device mapping in application, init, and ephemeral containers. A mount entry contains container identity and kind, volume name, mount/device path, declared read-only setting when applicable, `subPath`, and `subPathExpr`. Preserve `subPathExpr` literally; P0 MUST NOT execute or guess its expansion. Keep multiple mount entries on one relationship without multiplying record capacity or active consumer count. PVC access modes, volume mode, and known PV reclaim policy are first-class record fields.

Preserve storage-class assignment state explicitly. A PVC's empty `storageClassName` is `classless`; an absent/null field is `unassigned`; an uninterpretable field is `unknown`; a visible name is `known`. An omitted PV class is classless because PVs do not receive PVC default-class assignment. `--storage-class ""` selects classless records only. Classless and unassigned records have null class names but distinct report states, group keys, and text labels; group `value_state=absent` denotes classless, while `value_state=unassigned` denotes an unassigned class.

Relationship `runtime_state` is `unverified` in P0. These fields describe API declarations; they do not confirm a successful mount, writable access, attachment, or bytes used by an individual container or subdirectory.

Consumer joins use names within a namespace because pod claim references do not carry claim UIDs. Summary API PVC references also lack a claim UID. Match the sample's pod UID, volume name, and claim reference; reject samples older than PVC creation, and verify the PV binding when that enrichment is visible. Missing optional PV access alone does not invalidate a compatible pod/claim correlation; a known binding conflict does. When the available checks cannot establish a compatible identity, mark the sample identity uncertain rather than reusing telemetry across a delete/recreate boundary. These checks establish an API correlation, not proof of the mounted device's identity.

### Kubelet utilization

With `-u`, request `/api/v1/nodes/{node}/proxy/stats/summary` once per unique node hosting active or terminating scheduled references to selected records. Never target an empty or absent node name from an unscheduled pod. Do not list or probe unrelated nodes. Do not exclude relevant nodes because of taints.

The response parser MUST tolerate unknown JSON fields and absent optional fields. Map volume samples through PVC references and visible pod identity. Retain source node, source pod UID, volume name, and observation timestamp. The upstream [Summary API types](https://github.com/kubernetes/kubelet/blob/master/pkg/apis/stats/v1alpha1/types.go) define optional volume fields; the [documented access path](https://kubernetes.io/docs/reference/instrumentation/node-metrics/) goes through the API server.

No active or terminating visible consumer means no node target can be inferred in P0. Report `no_live_consumer` as the metric reason if pod enumeration was complete; otherwise `consumers_unknown`. Even an unreferenced or pending claim MUST remain in the report. A terminating reference does not establish an unused claim. No sample from a reachable node is `missing`, not proof of unsupported storage.

The collector MUST enforce request and overall deadlines, concurrency limits, response size limits, and bounded retries. Retry transient transport errors, 429 responses, and selected 5xx responses at most twice, honoring Retry-After within the overall deadline. Do not retry 401/403 repeatedly. A node failure affects its observations, not successfully collected inventory.

An observed HTTP 401 MUST remain a fatal authentication failure even when its body is oversized, truncated, or fails to arrive. Stop at the status without consuming or exposing that error body. For other responses, count bounded body bytes actually consumed by the adapter, including discarded error bodies; unread bytes are not consumed-byte cost. A body-limit or read failure must not downgrade an already observed authentication failure to optional missing telemetry.

All workers and output channels MUST share run cancellation. Every producer exit path closes or drops its stream, and its consumer MUST terminate without waiting indefinitely. When the supervisor observes a deadline, fatal failure, output failure, or user interruption, stop dispatch, cancel pending work, and allow at most one monotonic second for cooperative shutdown before aborting remaining asynchronous tasks. This is a cooperative task-shutdown budget, not synchronous CPU preemption. A single node failure MUST NOT deadlock other collection or erase completed inventory. Repeated metric observations MUST reuse inventory joins rather than trigger a PVC lookup per sample; lookup errors stay explicit.

Decoding MUST enforce input limits while reading and parsing. Numeric normalization MUST bound work by input length and supported range, without allocating power-of-ten buffers proportional to arbitrary exponents. Normalization/graph loops check deadlines/cancellation and yield at least every 64 objects/candidates, with checks before and after each bounded decode step. Blocking workers must cooperate and be joined before return. A timeout wrapper or abort request alone MUST NOT be treated as proof that CPU or blocking work stopped.

At an overall deadline, incomplete base inventory fails the command. Completed base inventory with interrupted optional collection returns a partial report. All clients MUST cancel outstanding work when the run finishes.

## Filters and grouping

Filtering MUST happen before aggregation. Server-side label selection applies only to base objects; a PVC label selector MUST NOT be reused for pods because consumer labels can differ. Independent filters combine with logical AND. Duplicate scalar filter flags are invalid in P0.

Filters use three-valued evaluation: match, no match, or unknown. AND evaluates to no match when any predicate is definitively false, to match when every predicate is true, and to unknown otherwise. Only known matches enter `records` and aggregates. Unknown candidates MUST be preserved in `selection.unknown_records` with ID, display identity, and unresolved filter fields. They MUST NOT be described as known exclusions or as an empty successful selection.

`selection` MUST expose `base_count`, `matched_count`, `excluded_count`, `unknown_count`, and `complete`. The counts partition the completely fetched base inventory after server-side selectors. `complete` is true exactly when unknown_count is zero. A definite true node reference can establish a match even if other consumer pages are missing; establishing no matching node requires complete consumer enumeration. `--node` selects active or terminating scheduled references, not PV node affinity or backend placement.

When selection is incomplete, matched counts and aggregates are lower bounds for the query. Text MUST say `selection incomplete`, show the unknown count, and MUST NOT say `no matching resources` or `no issues found` without qualification. Ordinary queries return partial results and diagnostics. Either strict policy (`--require-metrics` or `--fail-on`) MUST return exit 4 for incomplete selection, even when there are zero known matches. This check precedes finding policy evaluation.

A complete selection with zero matches is a genuine empty result and can exit 0. For example, `--node worker-a -u --require-metrics` with denied pod access and otherwise possible candidates is incomplete and exits 4; the same query after a complete pod list proves no matches exits 0.

### Dimensions

P0 dimensions are `storageclass`, `namespace`, `pvc`, `pv`, `driver`, `node`, and `pod`. `-g` preserves the supplied order, replaces the default list, rejects duplicate or unknown dimensions, and requires at least one dimension. `namespace` means claim namespace. Unknown and explicitly absent class metadata MUST have different group keys and labels.

`node` and `pod` are reference dimensions. A claim can occur in multiple such branches. For each group, compute totals over the set of distinct base record IDs beneath it. Parent totals MUST use the union of descendant record IDs. Set `overlapping_children=true` when siblings share members, and display a note that child totals cannot be added. Consumer branches MUST mark repeated volume references as shared.

When both reference dimensions occur, preserve the actual pod-to-node edges. For example, `-g node,pod,pvc` includes a pod only beneath its own scheduled node. It MUST NOT form a Cartesian product of all nodes and pods referencing a claim. Group labels use names for readability and UIDs for pod identity.

When a dimension cannot be resolved, keep the record under a reason-specific bucket such as `(unknown namespace)`, `(classless)`, or `(no live consumer)`. In PV mode, unbound volumes have no claim namespace or PVC; that absence is explicit. After the last grouping dimension, append base record leaves unless that dimension already uniquely identifies each base record.

### Sorting

Valid P0 fields are `name`, `requested`, `provisioned`, `used`, `available`, `used-percent`, `inode-percent`, and `severity`. Numeric sorting uses raw values, not formatted strings. Unknown values sort last in either direction. Severity order is critical, warning, info, none for descending order. Ties use namespace, name, then UID ascending.

Sorting affects sibling groups and leaves. Group numeric sort keys use group aggregates; invalid or incomplete aggregates sort as unknown. Group ties use their normalized dimension key and full group path ascending. Text table output sorts base records. No output order may depend on API response order or hash-map iteration.

Diagnostics sort by subject ID (unscoped first), code, source, reason, affected-field list, and message, all ascending. Affected-field names sort ascending with duplicates removed; repeated diagnostic occurrences remain inspectable. This order is independent of the requested record sort and of malformed inventory/sample encounter order.

Equivalent ratios MUST compare equal regardless of numerator/denominator representation. Ordering MUST be transitive and consistent with equality across the supported range, including when cross-products exceed u128. Descending order reverses the primary known-value comparison only; it MUST NOT reverse unknown-last placement or ascending identity tie-breaks. Sorting and threshold/severity rules share centralized exact arithmetic.

## Quantity and metric semantics

### Inventory quantities

| Field | Required interpretation |
| --- | --- |
| `requested_bytes` | PVC `spec.resources.requests.storage`; absent for PVs with unavailable claims |
| `bound_bytes` | PVC `status.capacity.storage`; never substituted for PV capacity |
| `provisioned_bytes` | PV `spec.capacity.storage`; unknown when PV enrichment is unavailable |
| `filesystem.capacity_bytes` | Capacity from the selected filesystem observation |
| `filesystem.used_bytes` | Used bytes from the same observation |
| `filesystem.available_bytes` | Available bytes from the same observation; never computed by subtraction |

The quantity parser MUST support Kubernetes decimal and binary suffixes, exponents, and fractional quantities without floating point parsing. Normalize positive fractional byte values by rounding up to whole bytes. Individual normalized byte and inode values use the unsigned 64-bit range, 0 through 18446744073709551615. Aggregates use checked unsigned 128-bit sums, 0 through 340282366920938463463374607431768211455. Negative or out-of-range individual values become null with `invalid_quantity` or `invalid_metrics`; aggregate overflow suppresses that sum with `aggregate_overflow`. Never clamp an invalid value to zero, saturate, wrap, or discard an inventory object because a quantity is invalid. JSON counters are base-10 integer strings, avoiding loss of precision in consumers; null means unknown, never zero.

All thresholds, percentage comparisons, sorting by ratio, disagreement comparisons, and severity decisions MUST use exact integer/rational arithmetic. For threshold coefficient t and decimal scale s, compare `100 * used * 10^s` with `capacity * t`; do not compare rounded display percentages. Individual-counter products fit unsigned 128-bit arithmetic under the six-digit threshold precision limit. Aggregate ratio comparisons MUST use an overflow-safe exact method such as reduced fractions or arbitrary-precision intermediates. Display percentages round half up to at most six decimal places. A value mathematically below 80% MUST remain below its warning threshold even if its displayed percentage is 80.0%.

Actual zero is preserved. A missing field MUST NOT become zero. Provisioned, bound, and requested values may differ during expansion; P0 preserves the facts and conditions. P1 may interpret resize progress using additional evidence.

### Observation selection and freshness

Each eligible Filesystem record has zero or more candidate observations. For each metric family (bytes and inodes), choose the newest attributable fresh candidate with at least one valid field. If none qualifies, retain the newest candidate for inspection with its stale, invalid, or uncertain status. Sort equal timestamps by source node, pod UID, and volume name ascending. Break remaining ties by eligibility in the order fresh, stale, timestamp unknown, clock skew, invalid timestamp, identity uncertain, then lexicographically by capacity/total, used, and available/free. For each field, valid values sort numerically ascending, followed by missing, then invalid. This final tie-breaker is independent of warning/critical thresholds and preserves one complete candidate identity. Never fill missing fields from a different candidate, even when an older complete candidate exists.

Validate fields independently and export `field_status` (`ok`, `missing`, or `invalid`) for every tuple member. Invalid values become null with a diagnostic; the valid fields remain available. A selected family is `ok` when all three fields are valid, `partial` when some are usable and others are missing or invalid, and `invalid` when no reported field is usable. Family freshness and identity failures override `ok`/`partial`. Missing values are null, not zero. Derived percentages require their own valid inputs from this same observation.

Fresh means the provider timestamp is within `--max-metrics-age` of collection end and no more than 30 seconds in the future. Missing timestamps are `timestamp_unknown`; timestamps beyond the future tolerance are `invalid` with reason `clock_skew`. Stale, future, invalid, and identity-uncertain observations are excluded from fresh aggregates and pressure findings. Their values remain inspectable with status. Partial observations MAY support field-specific findings and leaf percentages, but do not satisfy `--require-metrics` or the common-subset aggregate contract.

If fresh complete observations for one claim within a 60-second window differ by at least 5 percentage points of used capacity, emit `metrics_disagreement`. Select the newest tuple deterministically, retain candidate count, each candidate's raw tuple/provenance/status, and the displayed observed range, and mark the selected tuple uncertain. Such tuples MUST be excluded from aggregate metrics, pressure findings, and required-metrics success. Do not sum duplicate samples, compute a maximum from each field, or blend values across nodes.

Uncertainty MUST NOT produce a passing strict severity check when a conflicting fresh, attributable candidate reaches that check's threshold. With `-u --fail-on critical`, a 90%/99% disagreement is inconclusive and exits 4 even though the disagreement finding itself is warning. With `--fail-on warning`, use the warning threshold. A candidate reporting zero available bytes or free inodes can meet either severity level. Evaluate candidate risk using the same field-specific exact rules as normal pressure findings, without asserting that an uncertain observation is the volume's definitive usage. `--require-metrics` remains inconclusive for every uncertain byte tuple regardless of severity. Disagreement wholly below the requested severity does not itself make that severity check inconclusive; normal observed-finding policy still applies.

Filesystem utilization is `100 * used_bytes / capacity_bytes` when capacity is positive and both input field statuses are ok. Inode utilization is `100 * inodes_used / inodes_total` under the corresponding condition. A zero denominator yields null and an `invalid_metrics` diagnostic for the ratio; it does not invalidate independently meaningful available/free fields. Negative or nonfinite values are invalid. When used or available/free exceeds a positive reported capacity/total, invalidate that numerator field and diagnose the inconsistency. Used plus available need not equal capacity and MUST NOT be forced to equal it.

`volumeMode: Block` yields `not_applicable` for filesystem and inode metrics, even when logical capacity is known. A filesystem tuple alone MUST NOT be treated as a raw block device utilization measurement.

### Aggregates and coverage

Inventory totals sum each distinct base record once. Each quantity has known and total record counts; an aggregate with unknown members MUST be marked partial. JSON exposes the known sum and coverage rather than claiming a complete total. Text MUST append a partial marker or show `--` with an explanation.

Byte aggregates use a common subset of distinct records with fresh, complete, unambiguous byte tuples. They expose known sums for capacity, used, and available, and the percentage is `100 * sum(used) / sum(capacity)`. Do not average percentages. Inodes use a separate complete subset and weighted calculation.

The oldest and newest selected sample contributing to a metric aggregate MUST be within 60 seconds. Otherwise suppress that aggregate's numbers with reason `time_skew`; retain per-record observations. No eligible samples yields null sums and null percentage, not zero usage.

Metrics coverage counts all known-match Filesystem base records, including pending and unreferenced records. Block records are counted separately as not applicable. Report fresh complete count, eligible count, and counts by unavailable reason. `--require-metrics` requires complete selection and a usable complete fresh byte tuple for every eligible record. Zero eligible records succeeds only when selection is complete, with explicit `0/0` coverage. Inode availability does not gate that flag. Coverage for an incomplete selection MUST be labeled as coverage of known matches only.

Filesystem sums mean logical observations attributed to claims, not physical backend usage. Known shared backend filesystem identities MUST carry `overlapping_filesystem` and be excluded from additive filesystem and inode aggregates; the individual observations remain visible. In P0 an exact nonempty CSI `(driver, volumeHandle)` match across visible, verified Filesystem records establishes shared volume identity. Do not compare opaque handle prefixes or infer identity from classes, capacities, or missing PV metadata. An overlap marker is omitted when overlap has not been established; omission does not prove physical independence. The aggregate coverage reason is `overlapping_filesystem`, and text explains that these exclusions affect additive totals. `--require-metrics` still tests each individual attributable fresh complete byte tuple; exclusion from sums alone does not fail that check. Where identity is unknown, explain in text that filesystem totals are per claim rather than physical storage totals; JSON exposes `physical_total=false`.

P0 supplies no backend free-capacity total. Resource quotas, CSI capacity advertisements, cloud volumes, and snapshots MUST NOT be added to filesystem capacity or provisioned totals.

## Findings

Every finding MUST contain a stable code, severity, subject ID, evidence, and suggested investigation. It MUST distinguish an observed condition from a hypothesis. Findings are sorted by severity descending and subject identity ascending, then code. Multiple rules may apply to the same record.

Metric finding evidence MUST name the affected family and retain its selected observation's source and timestamp when present. A disagreement finding includes the exact-arithmetic observed percentage range for each conflicting family. Usage findings include displayed percentage plus raw numerator/denominator evidence; display rounding MUST NOT determine severity. Different family failures may produce different findings on the same record, and shared finding codes combine their evidence rather than duplicating the code for that subject.

Evaluate pressure rules independently on a fresh, attributable selected observation with family status `ok` or `partial`. Each rule requires only the ok fields named below, from that observation. A missing or invalid unrelated field MUST NOT suppress a valid finding. Disagreement or uncertain identity still prevents asserting a pressure finding; retain the uncertainty finding and observed range instead.

| P0 code | Severity | Trigger |
| --- | --- | --- |
| `filesystem_usage_high` | warning or critical | Valid used bytes and positive capacity yield a percentage at or above the configured threshold; available bytes not required |
| `inode_usage_high` | warning or critical | Valid used inodes and positive total yield a percentage at or above the configured threshold; free inodes not required |
| `filesystem_space_exhausted` | critical | Valid available bytes equals zero on an attributable Filesystem observation, independently of its used percentage |
| `filesystem_inodes_exhausted` | critical | Valid free inodes equals zero, independently of the used inode percentage |
| `claim_lost` | critical | PVC status phase is Lost |
| `volume_failed` | warning | PV status phase is Failed |
| `binding_conflict` | warning | Visible binding identifiers disagree |
| `claim_pending` | info | PVC status is Pending; include age and binding mode if known |
| `no_active_consumer` | info | Complete pod inventory shows neither active nor terminating references; evidence includes both counts |
| `volume_released` | info | PV status is Released; include reclaim policy without recommending deletion |
| `metrics_missing` | info | Utilization requested but no attributable observation returned |
| `metrics_stale` | info | Only stale observations are available |
| `metrics_unavailable` | info | Forbidden, timeout, unknown consumer visibility, or another collection failure |
| `invalid_metrics` | warning | A provider returned malformed, impossible, or invalid measurements |
| `metrics_disagreement` | warning | The disagreement rule above applies |

Critical thresholds take precedence over warning thresholds for a given usage rule. Low filesystem usage alone is not an issue. Pending claims are informational because delayed binding may be expected. Unused, unreferenced, or released objects MUST NOT be labeled safe to delete.

Exhaustion means the provider reported zero available/free capacity. It does not prove an application write has failed. Evidence MUST retain the specific field, value, unit, observation timestamp, and source. Positive reported capacity with used=80% and available=0 MUST produce the critical exhaustion finding even if the usage rule is only warning. The same applies to free inodes. No new configurable low-space threshold is introduced in P0.

Collection diagnostics and health findings are distinct: a forbidden PV lookup makes provisioned capacity unknown but does not establish unhealthy storage. Terminal issues view includes findings and a separate collection summary; collection diagnostics are also structured in JSON. Not-requested and not-applicable metrics do not create missing-metrics findings.

P1 diagnostics may add provisioning, attachment, mount, resize, topology, quota, and snapshot findings only with explicit supporting events or API state. Each new code needs an acceptance fixture and severity rationale.

## Rendering and export

### Text views

- `tree`: Render the requested group hierarchy, deduplicated totals, and base record leaves.
- `table`: Render one row per distinct base record with relationships summarized as counts; `-g` does not expand rows.
- `summary`: Render one row for each first-level group with recursive distinct-member totals and phase counts. A single root summary precedes the groups.
- `issues`: Render findings with subject, code, evidence, and investigation hints; show an explicit message when no findings exist. Collection failures remain visible.

All text views MUST identify context, selected scope, kind, and metrics state in readable labels. Table, tree, and summary use measured column widths, two spaces between columns, left-aligned labels, and right-aligned numbers. Headers and separators distinguish columns; do not print unpadded space-delimited rows. Preserve complete identities; cluster-wide claim tables include namespace/name. Tree branches show grouping without internal pod UID suffixes.

Use explicit IEC units (`B`, `KiB`, `MiB`, `GiB`, etc.) and at most one decimal place for human byte quantities. Display percentages with one decimal place. These are presentation rules only: exact thresholds, sort order, raw JSON/CSV bytes, and six-place machine percentages remain unchanged. A rounded displayed threshold MUST NOT change severity.

Table rows show binding phase and visible active/deleting pod counts alongside capacities. With usage enabled, show filesystem size, used, provider-reported available, used percent, inode percent, and a readable filesystem sample status. A stale or conflicting inode percentage carries its own status even when bytes are fresh. Never label a missing/partial/stale/conflicting byte tuple as Fresh. Without `-u`, omit usage columns and state that metrics were not requested. Unknown numeric cells use `--`; not-applicable cells use `n/a`. Partial inventory sums carry `*` and an explanation; keep zero distinct from missing.

Footers show selection completeness, fresh byte/inode coverage, unavailable reasons, and the latest aggregate sample timestamp when present. Show a concise finding count when findings exist. Summaries use readable phase counts instead of JSON fragments; each group retains byte coverage. Explain reference overlap and that per-claim filesystem totals are not physical totals in plain language, without schema assignments such as `physical_total=false`. When a driver reports filesystem capacity above provisioned capacity, note that it may describe backing storage; do not claim physical isolation or add a pressure finding solely for that difference.

Issues separate subject/severity/problem, readable evidence fields and observation provenance, and the next investigation step. Retain the structured finding code for lookup. Wide details group PV/driver/access/reclaim information once per record, then list each pod and its declared mounts/devices; avoid repeating the volume metadata for every consumer.

`--wide` adds PV name, driver, volume mode, access modes, known reclaim policy, and declared consumer details to tree/table output. Keep one primary row per base record; append detail lines for pod, node, container, volume name, path, read-only setting, and subpath rather than multiplying capacity rows. In summary/issues views, show these details under relevant groups or subjects. Label the block `Declared consumers (runtime unverified)`. JSON always contains these fields; CSV remains the stable flat record export, so `--wide` has no effect on JSON/CSV. Declared block device paths are labeled separately from filesystem mounts.

Do not depend on color to convey severity. Honor `NO_COLOR` in auto mode. When stdout is not a terminal, auto mode disables ANSI color and uses stable plain output. Escape control characters from object metadata and provider messages before terminal display.

### JSON report

`-o json` MUST emit one complete UTF-8 JSON document to stdout with a trailing newline and no ANSI escape sequences or log prefixes. Logs go to stderr. The report is independent of `--view`: all selected records, groups, findings, and diagnostics remain present. `-g` and filters still determine grouping and selection. No credentials or raw kubeconfig contents may be serialized.

Schema identifier: `view-storage/v1alpha1`. Before 1.0, incompatible changes require a new schema identifier and release notes. Additive optional fields are allowed; consumers should ignore unknown fields. Canonical field names are snake_case.

The complete structural contract is [report.schema.json](schema/report.schema.json), using JSON Schema Draft 2020-12. [Contract scenarios](../examples/contract/README.md) pair sanitized API inputs with complete expected JSON reports and all four text views. Schema validation checks shape; referential, accounting, selection, and rule invariants require semantic validation as well. Draft 0.5 updates the unreleased alpha contract without claiming compatibility with a published executable.

| Top-level field | Contract |
| --- | --- |
| `schema_version` | `view-storage/v1alpha1` |
| `collection` | Start/end timestamps, context name, namespace scope, source completeness and resource versions |
| `query` | Resolved filters, kind, grouping, and metrics request settings; no secrets |
| `selection` | Completeness, matched/excluded/unknown counts, and unresolved candidate identities |
| `records` | Distinct base records, each with ID, metadata, capacities, metrics, links, and consumer references |
| `consumers` | Distinct visible pods with UID, namespace, name, phase, activity, and scheduled node |
| `relationships` | Unique record-to-pod edges with volume names, declared mounts/devices, and unverified runtime state |
| `groups` | Ordered group tree; each group carries member IDs, aggregates, coverage, and overlap status |
| `summary` | Union-based totals, counts by phase, metrics coverage, finding counts, and root overlapping_children marker |
| `findings` | Structured findings in deterministic order |
| `diagnostics` | Collection and interpretation failures, subjects, reasons, and affected fields |

Metric values carry `status`, `reason`, `source`, `observed_at`, `field_status`, and a `candidates` array alongside their tuple. Each candidate preserves the same tuple, field status, source, timestamp, and eligibility status, without nested candidates. `candidate_count` MUST equal the array length. Status is one of `ok`, `partial`, `not_requested`, `not_applicable`, `missing`, `forbidden`, `timeout`, `stale`, `timestamp_unknown`, `invalid`, `identity_uncertain`, or `uncertain`. Reason refines the status, for example `no_live_consumer` or `metrics_disagreement`. Source is an object naming provider, node, pod ID, and pod volume name, or null when no observation exists. Group metrics may additionally have status `unavailable` with reason `time_skew` or `overlapping_filesystem`.

Every `consumer_id` and relationship endpoint MUST resolve within the report, and each edge is unique by `(record_id, consumer_id)`. Record consumer IDs MUST equal the edge endpoints for that record. Multiple declared mount entries remain on that single edge. Group member IDs MUST resolve to records and group totals MUST derive from distinct member IDs. Groups carry `selection_complete`; totals from an incomplete selection remain lower bounds even when every known member has complete fields. Unknown selection candidates have their own identity entries and MUST NOT be included as matched records.

Complete reports, including consumer and candidate arrays, are supplied by the [healthy](../examples/contract/healthy/report.json), [critical-conflict](../examples/contract/ambiguous-critical/report.json), and [terminating-consumer](../examples/contract/terminating-consumer/report.json) scenarios.

Timestamps are RFC 3339 UTC. Percentages are JSON numbers, rounded to at most six decimal places; rules and sorting use unrounded ratios. Byte and inode counters are decimal strings. Null is required for unknown quantities. Arrays follow the deterministic sort contract. IDs use `pvc:<uid>` or `pv:<uid>` and consumers use `pod:<uid>`. Relationship IDs encode record and consumer IDs; group IDs encode their full dimension path using percent-escaped values. Consumers and relationships sort by namespace/name/UID and record/consumer IDs respectively; declared mounts sort by container kind, container name, volume name, and path. Schema examples are illustrative inputs to the future implementation, not output from a working plugin.

### CSV report

`-o csv` emits a header and one row per distinct selected base record, independent of text view and reference grouping. P0 columns, in order:

```text
kind,namespace,name,uid,phase,storage_class,driver,volume_mode,pv_name,requested_bytes,bound_bytes,provisioned_bytes,filesystem_capacity_bytes,used_bytes,available_bytes,used_percent,inodes_total,inodes_used,inodes_free,inode_percent,metrics_status,metrics_reason,metrics_source,observed_at,active_consumer_count,terminating_consumer_count,severity
```

Unknown numeric values are empty cells; actual zero is `0`. Byte strings contain base-10 digits with no units. Fields use standard CSV quoting; do not embed JSON lists or tree indentation. `metrics_status` describes the byte tuple; `metrics_source` is the provider identifier, not the JSON provenance object. Missing inodes remain empty. Headers are present even for empty results. Detailed relationships, per-field diagnostics, selection completeness, and inode provenance are available in JSON. CSV MUST still print incomplete-selection diagnostics to stderr; a strict invocation MUST return exit 4 for incomplete selection even when stdout contains only the header.

## Errors and exit status

Successful empty inventory MUST emit a valid empty report and exit 0. Base inventory forbidden, authentication failure, or an incomplete base list MUST exit 1 with an actionable stderr error. A failed optional collection normally returns a report with diagnostics and exit 0.

| Exit code | Meaning |
| --- | --- |
| `0` | Report produced; requested failure policies did not trigger |
| `1` | Cannot produce required base inventory or serialize/write the report |
| `2` | Invalid invocation |
| `3` | `--fail-on` matched an observed finding |
| `4` | Strict check inconclusive: selection/required classification incomplete, required byte metrics incomplete, or conflicting fresh telemetry could meet the requested severity |
| `130` | User interrupted the run; pending work canceled and no unfinished report claimed as complete |

Precedence is invalid invocation, fatal failure, inconclusive strict check, finding policy, success. Codes 3 and 4 still emit a structurally complete report with explicit data completeness. `--fail-on critical` fails on critical findings; `--fail-on warning` includes warnings and critical findings. Both require complete selection and must account for severity-relevant conflicts when utilization is requested; only `--require-metrics` requires complete byte metrics for every eligible record. Neither option invents a critical finding for missing metrics. JSON/CSV stdout MUST remain parseable when diagnostic messages are written to stderr.

The implementation SHOULD exit without a panic on a closed output pipe. A report that was interrupted or could not be written MUST NOT be described as complete.

### Input validation and failure propagation

Malformed external data MUST use the documented fatal-error or recoverable-diagnostic path without panicking. Invalid invocation and malformed required inventory are fatal; an invalid optional measurement preserves inventory and independently valid sibling fields. Decoding and validation MUST preserve missing, invalid, and actual zero as distinct states. Fallible conversions MUST NOT silently truncate, coerce unknown states to healthy defaults, or substitute zero for an error.

Unknown Kubernetes/provider fields MUST be accepted and ignored unless explicitly supported. Preserve unfamiliar values in open external status fields, such as PVC/PV phase strings, and attach `unsupported_api_value` diagnostics when dependent behavior cannot interpret them. Do not default unfamiliar phases to Bound, Running, or healthy. Unrelated fields and rules remain usable. A filter depending on unsupported classification evaluates to unknown; a requested strict health check requiring it is inconclusive and exits 4. Unused unknown fields alone do not make selection or checks incomplete. Closed report enums require an explicit unsupported-value diagnostic/error or compatible schema evolution, never an invented existing variant. Missing required identity or structurally malformed input follows the existing fatal/optional boundary.

The alpha report preserves unfamiliar string `volume_mode` values. A wrong-type API volume mode produces `volume_mode=null` and `invalid_api_field`; omission or API null retains Kubernetes' Filesystem default. Unclassified modes yield invalid byte/inode families when utilization is requested, with `unsupported_volume_mode` or `invalid_volume_mode`, and never consume observations received for another record's node. Only known Block records count as not applicable; unresolved modes remain in the coverage denominator. With utilization, dependent strict checks exit 4. Inventory-only health checks retain independent phase/lifecycle behavior.

`--fail-on` requires interpretable selected-record and consumer health classifications; an unfamiliar selected pod lifecycle exits 4 while emitting the report. `--require-metrics` evaluates attributable byte observations independently: a recognized live consumer may satisfy it even when another declared consumer has an unfamiliar phase. A node predicate requiring an unknown lifecycle remains unknown unless an independently known live consumer proves the match.

Worker failures MUST be observed by the collection supervisor. After cancellation/shutdown completes, no owned task may issue another request or write late output. Optional request failures retain their documented partial-result behavior; unexpected task failures must surface as an error and cannot yield an apparently complete success. Serialized report values and exit decisions MUST agree with the same normalized data, regardless of worker completion order.

### Diagnostic and support boundaries

Deserialize optional provider counters as optional values; an omitted scalar MUST NOT acquire a zero default. No absent timestamp, omitted inode field, failed name lookup, or unsupported provisioner may be reported as a healthy zero measurement.

At every log level, including trace, log only allowlisted operation, selected resource identity, status, elapsed time, retry, and count metadata. Never log an unfiltered Summary API response body, kubeconfig contents, credentials, or unrelated namespace payloads. Captured support artifacts follow the same rule. A structured report may expose selected metric candidates and declared references, not a raw node dump.

Compatibility claims MUST identify Kubernetes version, driver/provisioner version, volume mode, permissions, and observed byte/inode coverage. Inventory-only success does not establish telemetry support, and Bound status does not prove the backing provider object still exists. The test matrix and support process are specified in [COMPATIBILITY.md](COMPATIBILITY.md) and the [df-pv review](DF_PV_LESSONS.md).

## Permission profiles

P0 MUST document separate inventory and utilization examples; it MUST NOT install or modify RBAC automatically.

| Operation | API permission | Effect if unavailable |
| --- | --- | --- |
| PVC base inventory | `list` persistentvolumeclaims in selected scope | Fatal in PVC mode |
| Pod enrichment | `list` pods in selected scope | Consumer visibility unknown; no inferred node targets |
| Referenced PV enrichment | `get` persistentvolumes | Provisioned capacity and driver may be unknown |
| Optional bulk enrichment in `-A` mode | `list` persistentvolumes and storageclasses.storage.k8s.io | Fall back to targeted gets within the enrichment budget |
| PV base inventory | `list` persistentvolumes | Fatal in PV mode |
| Referenced PVC enrichment | `get` persistentvolumeclaims in claim namespaces | Claim quantities and some joins unknown |
| Class enrichment | `get` storageclasses.storage.k8s.io | Provisioner and binding policy may be unknown |
| Kubelet proxy request | `get` nodes/proxy for relevant nodes | Usage unavailable; inventory retained |

No nodes list is required when node names come from pods. No metrics-server, Secrets, exec, write verbs, or cluster mutation is required by the tool. The client uses the user's configured kubeconfig authentication. A proxy request is authorized at the API server as `nodes/proxy`; narrower direct kubelet permissions are not an equivalent permission for that route. [Kubelet authorization](https://kubernetes.io/docs/reference/access-authn-authz/kubelet-authn-authz/) explains why granting `get nodes/proxy` provides broader capabilities than this tool uses.

## P1 integration contracts

Implementation order is Prometheus and refresh mode first, workload attribution and operational evidence second, and quotas, snapshots, and CSI topology context third. P0 release acceptance does not depend on these integrations.

### Prometheus

Use the explicit endpoint's [query API](https://prometheus.io/docs/prometheus/latest/querying/api/) for `kubelet_volume_stats_capacity_bytes`, `used_bytes`, `available_bytes`, `inodes`, `inodes_used`, and `inodes_free` families. Preserve actual underlying sample timestamps; an instant query evaluation timestamp alone MUST NOT make an old sample fresh. Reuse P0 tuple, coverage, and deduplication rules.

Multi-cluster endpoints require an explicit cluster label selector. Do not assume a kubeconfig context name is a Prometheus cluster label. Match namespace and PVC name inside the selected cluster, verify timestamps against PVC creation, and mark joins uncertain when available labels cannot establish identity safely. Deduplicate scrape replicas before selecting node observations. Never sum series from multiple scrape jobs for one PVC.

Read token files only when selected; validate TLS and support an explicit CA file. Never forward Kubernetes credentials to an arbitrary URL, log tokens, follow cross-origin authenticated redirects, or fall back to kubelet after a Prometheus failure. Auth mechanisms beyond these options require a separate design.

### Refresh and optional Kubernetes APIs

`--watch DURATION` runs sequential bounded collection cycles, waiting the specified duration after a cycle finishes before starting the next. It replaces terminal text after a complete cycle; redirected text appends complete reports without terminal control sequences. It MUST identify each collection interval and never present a previous cycle's sample as freshly observed. JSON mode emits newline-delimited complete reports, one per cycle, with the same schema; CSV watch mode is rejected initially.

Apply exit policies after each structurally complete report. If a strict check is inconclusive or `--fail-on` matches, emit that cycle's report and stop with exit 4 or 3 using the existing precedence. A fatal base-inventory failure stops with exit 1. Otherwise continue until interrupted. Ctrl-C cancels outstanding work and exits 130; it MUST NOT emit an unfinished serialized report as a successful cycle. Optional collection failures retain the normal partial-report behavior.

Workload resolution follows owner UIDs and exposes unresolved owners. Events and attachments supply evidence, not guessed root causes. Snapshot and quota APIs are optional and cannot suppress inventory if absent. CSIStorageCapacity is advertised topology-scoped provisioning information, not an additive backend pool total. Every enrichment MUST declare its own completeness and permissions.

## Validation and acceptance

The implementation MUST reuse the sanitized API inputs in [contract scenarios](../examples/contract/README.md) to build reports and render tree, table, summary, and issues output. Expected reports are independent contract examples, not results generated by the Rust implementation under test. Normalize declared fixed timestamps/context only; do not normalize away missing values, coverage, findings, or selection status. Snapshot tests cover text layout and help, while arithmetic and relationship assertions verify meaning independently of formatting. Mock API tests verify request paths, permissions, pagination, deadlines, and request counts. Live tests supplement fixtures; driver telemetry availability is recorded explicitly.

Malformed input, overflow, incomplete selection, conflicting observations, failed lookup, interrupted worker, and output failure paths require direct assertions of data, diagnostics, and exit behavior. Tests MUST detect reversed threshold comparisons, shared-volume double counting, swallowed lookup errors, and unsuccessful cancellation. Code-coverage percentages do not replace these cases. Rust structure and tool-specific coverage/fuzzing gates belong in [DESIGN.md](DESIGN.md), [DEVELOPMENT.md](DEVELOPMENT.md), and [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md).

| ID | Scenario | Required result |
| --- | --- | --- |
| A01 | Run without flags in a context with a namespace | Only that namespace's PVCs; no metrics requests |
| A02 | `-A` or explicit namespace | Correct API scope; conflicting flags fail before requests |
| A03 | Bound, Pending, Lost, terminating, and unreferenced PVCs | All remain visible, with distinct phases or metadata |
| A04 | `-A --kind pv` with Available and Released PVs | Unbound PVs visible without invented claim quantities |
| A05 | PV or storage class get forbidden | PVC report survives, capacities stay unknown, diagnostic present |
| A06 | Required base list forbidden or page two fails | Exit 1; no complete success report |
| A07 | Pod list forbidden | Consumer count unknown; no false no-consumer finding |
| A08 | One 100Gi RWX claim referenced by three pods on two nodes | Root provisioned total is 100Gi; overlapping reference groups flagged |
| A09 | Same PVC mounted in multiple containers | One claim and one consumer edge per pod |
| A10 | Different labels on PVC and consuming pod | PVC selector still finds all visible consumer references |
| A11 | One node times out, another succeeds | Inventory intact; separate observation statuses and partial coverage |
| A12 | Node proxy forbidden | Exit 0 with diagnostics by default; exit 4 with required metrics |
| A13 | Missing metrics or zero usage | Missing stays null; actual zero stays zero |
| A14 | Requested 100Gi, PV 120Gi, filesystem 118Gi | All quantities remain distinct; no backend free-space calculation |
| A15 | Raw block volume | Logical capacity visible; filesystem metrics n/a; excluded from eligible coverage |
| A16 | Complete, partial, stale, future, and timestamp-free samples | Selection, status, findings, and aggregates obey freshness rules |
| A17 | Duplicate or disagreeing RWX observations | No summing; deterministic selection; uncertainty surfaced |
| A18 | Samples 61 seconds apart in one group | Group metrics suppressed with time_skew; leaves retained |
| A19 | One full small claim and one empty large claim | Weighted group percentage; never average percentages |
| A20 | Different used, available, and capacity arithmetic | Available stays provider-reported; no forced equality |
| A21 | Recreated claim or mismatched claimRef UID | No stale capacity or telemetry attributed to the new object |
| A22 | Decimal, binary, exponent, and fractional quantities | Exact expected bytes; overflow and negative values diagnosed |
| A23 | Empty match, unknown metadata filter, and denied base list | Three distinguishable outcomes; no silent access failure |
| A24 | JSON and CSV with warnings | Valid stdout, deterministic order, null/empty semantics preserved |
| A25 | Exact thresholds, zero denominators, and missing inodes | Correct severity; no divide-by-zero; no invented inode values |
| A26 | Finding policy and required metrics both trigger | Exit 4 with report; finding remains present |
| A27 | Group by pod or node, then change dimension order | Union totals stay invariant; no inflated totals or fabricated pod-to-node edges |
| A28 | Known shared backing filesystem across claims | Individual observations visible; additive metrics exclude overlap |
| A29 | Piped output, NO_COLOR, or control characters | Plain safe terminal output; clean machine serialization |
| A30 | Local Krew install from generated release manifest | Matching binary, archive layout, checksum, version, and help work |
| A31 | Node filter with denied pod access and zero known matches | Selection incomplete; either strict policy exits 4; no empty-success message |
| A32 | Complete query establishes zero matches under a strict policy | Complete empty report, explicit 0/0 coverage, exit 0 |
| A33 | Fresh used/capacity pair is above threshold, available bytes missing | Usage finding remains; aggregate excludes partial tuple; required metrics exits 4 |
| A34 | Available bytes is zero while used percentage is only warning | Critical filesystem_space_exhausted with field/source/timestamp evidence |
| A35 | Free inodes is zero while used inode percentage is low | Critical filesystem_inodes_exhausted independently of byte fields |
| A36 | One pod maps one volume into two containers and paths | One consumer edge, both mount declarations, unchanged capacity and consumer count |
| A37 | Read-only mount, literal subPathExpr, and block device mapping | Correct declared fields; no inferred runtime mount or expression expansion |
| A38 | 10,000 referenced PVs in -A mode | Bulk list strategy; no per-PVC PV get sequence when listing succeeds |
| A39 | Bulk PV list forbidden, targeted gets allowed, budget reached | One denied list, bounded gets, unknown remainder, cost diagnostics |
| A40 | Consumers or metadata exhaust time/request budget | Base inventory retained; affected membership incomplete; utilization retains reserved time |
| A41 | Dangling consumer ID, duplicate edge, or group member ID | Contract semantic validator rejects the report |
| A42 | Valid pressure inputs with an invalid unrelated field | Invalid field diagnosed; valid pressure finding survives |
| A43 | Fresh 90%/99% conflict with -u --fail-on critical | Raw candidates retained, no definitive pressure assertion, exit 4 |
| A44 | Values just below/at/above thresholds with identical rounded display | Exact severity comparisons; no floating-point boundary error |
| A45 | Scheduled terminating pod is the only consumer | Terminating count retained; node filter matches; node telemetry requested |
| A46 | Driver filter and a large unrelated consumer list | Membership prerequisites get priority; budget does not starve driver resolution |
| A47 | Unscheduled pod with empty node name | No empty-name node request; claim and reference remain visible |
| A48 | Repeated samples refer to one claim whose enrichment lookup fails | One cached lookup/failure; explicit missing field and diagnostic |
| A49 | A producer fails while another worker and output consumer are active | No hang; partial inventory preserved where valid; bounded cancellation/shutdown |
| A50 | Trace logging with node data from unrelated namespaces | No raw response, credential, or unrelated payload leakage |
| A51 | Direct and kubectl plugin invocation with context/namespace flags | Correct resolution with flags after plugin name; unsupported placement documented |
| A52 | u64 individual and u128 aggregate boundary/overflow | Exact supported values; diagnosed null on overflow; no clamping/wrap |
| A53 | Required disposable CSI profile produces no fresh byte tuple | Gate fails explicitly; inventory may pass separately; no successful skip |
| A54 | Same cluster type with different provisioners or permissions | Capabilities recorded separately; missing metrics never erase inventory |
| A55 | Equivalent ratios, extreme cross-products, ties, and unknown sort values | Equality/order agree; transitive exact comparisons; unknowns last and identity ties ascending in both directions |
| A56 | UTC clock jumps while requests, retries, and shutdown are in progress | Monotonic deadlines remain unchanged; sample freshness uses captured UTC and diagnoses invalid times |
| A57 | Deep/oversized scalars and cancellation during CPU parsing or normalization | Limits enforced without exponent-sized allocation; bounded checkpoints yield; blocking work joined and no late requests |
| A58 | Unknown API fields and unfamiliar external status strings | Unknown extras tolerated; open values preserved; dependent logic is unknown/inconclusive; unrelated data retained |

P0 property checks MUST verify that adding a consumer does not increase root logical capacity, reordering dimensions preserves root totals, unknown values never become zero, incomplete membership never passes a strict policy, and removing an unrelated metric field never suppresses a pressure finding with valid inputs. Group totals are checked against the member-ID union independently of rendered output. Exact ratio equality/order laws, comparator transitivity, unknown-last placement, and ascending identity ties are checked independently of input permutation and ascending/descending selection.

P0 performance validation uses fixtures for 10,000 claims and 50,000 pod references. Relationship construction SHOULD be linear in objects and edges aside from sorting. For a fixture with 10,000 bound claims, 10,000 distinct PVs, one storage class, 50,000 pods, and 100 relevant nodes, where the server honors page size 500 and all requests succeed, cluster PVC mode MUST use 20 base PVC pages, 100 pod pages, 20 PV pages, one class get, and 100 statistics requests. Thus enrichment uses 121 attempts, within the 256-attempt budget, and no individual PV gets. The mocked clock MUST leave sufficient stage time for this count assertion; separate deadline cases verify truncation.

The same namespace-scoped fixture MUST use targeted PV gets, stop at the 256-attempt enrichment limit, preserve inventory, and expose incomplete metadata. With utilization enabled, at most one logical statistics request is made per distinct relevant scheduled active/terminating node target, with at most three attempts per node; no per-pod metrics request pattern is allowed. Record benchmark runtime and peak memory before setting release runtime/memory budgets. Contract examples include transport counters for illustration; mocked HTTP integration tests MUST verify actual streaming byte counts and all request-budget accounting.

Release smoke tests MUST run on each advertised platform. Inventory live tests can use a local provisioner with no metrics support; usage live tests require a separately documented driver that emits compatible statistics. A missing sample on an untested driver is not a reason to claim universal CSI support.

## Implementation and release traceability

Releases MUST use a tested latest-stable Rust baseline and its bundled Cargo, pinned in [rust-toolchain.toml](../rust-toolchain.toml), with Rust 2024 edition and a committed Cargo.lock. CI and release builds MUST use the locked dependency resolution. Follow the [toolchain and dependency policy](DEVELOPMENT.md#current-toolchain-and-lightweight-dependencies): review upstream maintenance and advisories, enable only needed features, keep test tools outside runtime dependencies, and record dependency/feature/build-size changes. Enforce a reviewed budget of thirteen unique direct runtime packages including target-specific dependencies; expanding that budget requires a written design rationale. New direct packages require review even within the budget. Direct build dependencies and source overrides require an explicit policy change. Manifest and toolchain consistency checks MUST run in CI. Recheck stable releases weekly and before publishing; document temporary update delays. Dependency measurements start with the Cargo package, not the design fixtures. Passing manifest checks alone MUST NOT be described as a successful resolved dependency audit.

Keep this specification, its schema, and shared expected reports synchronized. The separate [implementation plan](IMPLEMENTATION_PLAN.md) maps numbered phases to deliverables and acceptance/coverage gates; [DEVELOPMENT.md](DEVELOPMENT.md) defines test measurement and disposable cluster profiles, and [COMPATIBILITY.md](COMPATIBILITY.md) records real outcomes. All A01–A58 cases, property/failure requirements, Rust coverage floors, platform checks, and required live gates must pass before P0 is declared complete. Planned work and selected environments are not completed evidence.
