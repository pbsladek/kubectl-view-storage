# kubectl-view-storage design

Status: Draft 0.8, 2026-11-07. Product defaults and release priorities are decided below. Target: an initial v0.1 release followed by incremental integrations. Companion: [behavioral specification](SPEC.md). Execution phases live in [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md); local test environments live in [DEVELOPMENT.md](DEVELOPMENT.md).

`kubectl-view-storage` will be a Rust kubectl plugin for investigating Kubernetes persistent storage from one command. It will connect capacity, actual usage, consumers, and operational findings while remaining useful without a monitoring stack or cluster administrator permissions. The initial experience will be a readable tree, with flags for other views and machine output.

## Product goals

The command should answer these questions quickly:

1. Which claims and volumes exist, including claims that are pending or have no active consumers?
2. How much capacity was requested and provisioned, and what filesystem usage is actually reported?
3. Which pods and nodes reference each claim?
4. Which claims are full, missing telemetry, lost, or otherwise need investigation?
5. How do those answers change when grouped by storage class, namespace, driver, or consumer?

Success means a user can get useful inventory with `kubectl view-storage`, add usage with `-u`, and move between operational questions by changing a small number of flags. Reliability of the numbers matters more than the number of integrations.

## Reference projects and adaptation

[kubectl-view-allocations](https://github.com/davidB/kubectl-view-allocations) supplies the interaction model: one command, composable `-g` grouping, optional `-u` utilization, and machine output. Its source separates collection, metrics, quantities, trees, sorting, and display. Adopt that separation for storage. Its repository also supplies useful examples of release metadata and developer tooling; the selected setup below adapts those ideas rather than duplicating its current dependency versions. See its [source layout](https://github.com/davidB/kubectl-view-allocations/tree/master/src), [Cargo manifest](https://github.com/davidB/kubectl-view-allocations/blob/master/Cargo.toml), and [Krew template](https://github.com/davidB/kubectl-view-allocations/blob/master/.krew.yaml).

[kubectl-df-pv](https://github.com/yashbhutwala/kubectl-df-pv) documents a focused persistent volume filesystem usage view, namespace filtering, node API permissions, and provisioner compatibility requirements. This proposal expands that use case with inventory independent of metrics, explicit data quality, relationships, grouping, operational findings, and integration boundaries. These are proposed capabilities, not claims of measured superiority or an exhaustive audit of df-pv.

The source-linked [implementation and support review](DF_PV_LESSONS.md) turns concrete lessons into regression requirements: optional metric fields, exact ratios, surfaced lookup errors, bounded cancellation, unfiltered-response logging prevention, flag-placement guidance, and driver-specific support evidence. Existing deduplication, unreferenced inventory, selectors, exports, and color handling also address reported user needs.

| Idea | Storage adaptation |
| --- | --- |
| Resource request aggregation | Sum PVC requests once per claim, including pending claims |
| Optional live utilization | Kubelet volume statistics first; Prometheus later |
| Composable grouping | Storage class, namespace, PVC, PV, driver, pod, node |
| Tree presentation | Group totals and volume leaves, with reference-aware consumer groups |
| Free and allocatable resources | Replace with explicit filesystem available bytes; backend capacity is a separate integration |
| Node filtering | Include tainted and control-plane nodes when relevant; taints do not remove storage consumers |
| Rust library and CLI separation | Keep collection and report construction independent of terminal rendering |

## Core features and milestones

P0 is the v0.1 release contract. P1 follows the first release. P2 requires a separate integration proposal and fixtures before implementation.

| ID | Feature | Milestone | User value |
| --- | --- | --- | --- |
| F01 | PVC inventory and explicit cluster PV inventory | P0 | Find pending, bound, lost, available, and released objects even without telemetry |
| F02 | Tree, table, summary, and issues views | P0 | Choose a presentation suited to capacity review or investigation |
| F03 | Ordered grouping and filters | P0 | Slice by storage class, namespace, driver, claim, volume, pod, or node |
| F04 | Kubelet filesystem and inode usage | P0 | Identify capacity and inode pressure where supported |
| F05 | Pod/node reference graph and declared mount details | P0 | Identify consumers, volume names, container paths, subpaths, and shared claims |
| F06 | Selection completeness, coverage, freshness, and provenance | P0 | Distinguish unknown query membership from missing telemetry and actual empty results |
| F07 | Evidence-backed findings and script exit behavior | P0 | Surface pressure, lost claims, pending state, and telemetry problems |
| F08 | Versioned JSON and flat CSV | P0 | Support scripts without parsing terminal trees |
| F09 | Rust binary and repeatable Krew releases | P0 | Easy installation and a maintainable repository |
| F10 | Prometheus provider and bounded refresh mode | P1 | Use existing telemetry access and revisit reports over time |
| F11 | Workload attribution, events, attachments, and resize context | P1 | Explain provisioning, attachment, and expansion problems with evidence |
| F12 | Quotas, snapshot relationships, and CSI topology context | P1 | Connect capacity planning, retention, and scheduling information |
| F13 | Backend pool health and capacity adapters | P2 | Add provider-specific context without confusing it with claim usage |
| F14 | Historical growth, exhaustion estimates, and cost context | P2 | Support planning with explicit assumptions and data provenance |

## Command experience

The public API is the CLI. Normal inspection uses one command with composable flags. Use the same grammar for small and large reports:

```sh
kubectl view-storage [flags]
kubectl-view_storage [flags]

kubectl view-storage -n payments
kubectl view-storage -n payments --pvc data-postgres-0 --wide
kubectl view-storage -A -u
kubectl view-storage -A -g storageclass,namespace,pvc
kubectl view-storage -A -u --view table --sort used-percent:desc
kubectl view-storage -A --view summary
kubectl view-storage -A -u --view issues
kubectl view-storage -A --kind pv
kubectl view-storage -n payments --pvc data-postgres-0 -u -o json
```

The four main controls are `-g` for grouping, `-u` for live usage, `--view` for presentation, and `-o` for serialization. Standard namespace, context, and selector flags follow kubectl conventions. Advanced collection controls stay out of the quick start.

Selected defaults: current kubeconfig namespace, PVC inventory, tree view grouped by `storageclass,namespace,pvc`, and no metrics requests. `-A` explicitly expands to all namespaces. Namespace scope makes the first run useful with namespace permissions. The tree exposes storage relationships immediately, and explicit `-u` keeps privileged collection and its latency predictable.

### Operator-facing terminal views

Text output uses aligned columns with right-aligned numbers, explicit IEC units, and one decimal place for percentages. PVC binding phase, consumer counts, provisioned capacity, filesystem size, available bytes, inode pressure, and sample validity remain separate concepts. Human display rounding never participates in accounting or alert decisions.

The renderer measures column widths before writing rows and observes cancellation in both passes. It uses a small private table type and the existing dependencies. Cluster-wide claim identities include namespaces. Tree connectors preserve grouping and shared-reference warnings; internal pod UID suffixes stay out of the ordinary view. Summary rows carry readable phase counts and coverage. Issue cards give evidence, source/timestamp, and the next action; wide blocks show volume metadata once and nest consumer/mount declarations below it.

[Captured real output](../examples/live/README.md) shows all four views, strict checks, and machine formats. The independently specified [contract views](../examples/contract/README.md) cover partial, stale, conflicting, and unavailable observations. Text footers explain missing values and backing-filesystem semantics without exposing report-schema assignments.

Used plus available need not equal filesystem capacity. Report the values independently; filesystem reservations and provider semantics matter.

Summary view shows distinct claim counts, phases, requested and provisioned totals, and metrics coverage by the requested groups. It must label partial sums. Issues view shows evidence and suggested investigation steps without implying that the plugin can repair resources.

`--wide` appends declared consumer details while preserving one capacity row per claim or volume. Include pod, node, container, pod volume name, mount/device path, read-only setting, and literal subpath declarations, plus PV, driver, access modes, volume mode, and reclaim policy. Label runtime state unverified: declarations do not prove mounts succeeded or describe bytes used by a container or subdirectory.

### Representative reports

[Contract scenarios](../examples/contract/README.md) provide shared sanitized API inputs, complete expected JSON reports, and expected tree, table, summary, and issues output. Healthy, shared RWX, partial permissions, missing/stale telemetry, genuine empty selection, unresolved selection, and field-specific pressure cases exercise the same model across all presentations. These are design fixtures for the future plugin.

The [JSON Schema](schema/report.schema.json) defines report structure. A normalized `consumers` collection resolves every consumer ID, and unique record-to-pod `relationships` carry declared mounts and devices. Shape validation is supplemented by arithmetic, identity, selection, and finding invariants. Text spacing is flexible; the counts, labels, evidence, and completeness statements are contract requirements.

## Storage accounting model

Keep the following quantities distinct:

| Quantity | Source | Meaning |
| --- | --- | --- |
| Requested | PVC spec storage request | Desired logical claim capacity |
| Bound capacity | PVC status capacity | Capacity acknowledged in claim status |
| Provisioned | PV spec storage capacity | Kubernetes advertised logical volume capacity |
| Filesystem capacity, used, available | Selected volume statistics observation | Filesystem view reported by the provider |
| Backend capacity and allocation | Future backend adapter | Pool or provider view with its own identity and accounting |

Kubernetes exposes separate PV capacity, claim requests, volume modes, and binding state. A raw block volume has no Kubernetes-mounted filesystem; it must not receive an invented filesystem fullness percentage. See [Persistent Volumes](https://kubernetes.io/docs/concepts/storage/persistent-volumes/).

Storage classes are provisioning policies, not finite disks. A sum of PV sizes is logical provisioned capacity, not backend pool consumption. A PVC request is not a billing measurement. The tool must never label provisioned minus requested as free storage or automatically recommend deleting an apparently idle volume.

### Identity and shared consumers

Build a graph with Kubernetes UIDs for identity and names for display. PVC-to-PV joins use the PVC volume name and verify the PV claim reference when available. Pod references are relationships, not independent copies of volume capacity. Include generic ephemeral volumes that materialize as PVCs and identify their pod ownership; defer inline CSI and node ephemeral storage accounting.

For an RWX claim referenced by three pods, the report contains one claim and three consumer edges. Overall and namespace totals count that claim once. Pod and node groups can overlap; each group deduplicates its own members, and parents recompute unions instead of summing child totals. Explain this in text and machine output. Track active, terminating, and terminal references separately; scheduled terminating pods remain metrics targets and node-filter matches. Referenced, scheduled, attached, and confirmed mounted are separate facts; a pod spec alone proves only a reference.

Different claims can also report a shared backing filesystem, especially with file storage. Filesystem aggregates are explicitly sums of observations attributed to distinct claims, never physical backend totals. Flag known overlaps; where a backend adapter proves overlap, exclude overlapping observations from additive filesystem totals and expose the reason. Do not infer backend identity from matching capacities alone.

P0 detects duplicate exact CSI driver/volume-handle pairs on visible verified volumes. Kubernetes defines the handle as the driver's unique volume name in its [CSI persistent volume source](https://github.com/kubernetes/api/blob/master/core/v1/types.go). Keep the opaque identifier internal; expose an overlap flag and diagnostics without the handle. Suppress additive filesystem/inode totals for those records while preserving their per-claim observations and pressure checks. Missing identities and different handles remain unknown with respect to backend sharing. Future backend adapters can establish additional overlap identities.

Conflict selection sorts candidates deterministically, then maintains a ratio multiset over an inclusive 60-second window of fresh complete tuples. Window extrema determine whether the exact gap reaches five percentage points. This costs O(n log n) time and O(n) auxiliary space instead of pairwise O(n²) work for a heavily shared claim. Tests compare the result with an independent small pairwise oracle and include 10,000 repeated observations. This algorithm does not replace the separate cooperative cancellation/checkpoint requirement.

### Missing observations

Inventory drives the report, so missing metrics never removes a claim. Distinguish not requested, not applicable, forbidden, timed out, missing, stale, invalid, and ambiguous observations. A missing sample alone does not prove that a driver lacks support or that a volume is unmounted.

Use fresh samples from one configured provider per run. Deduplicate duplicate node or pod observations of the same claim; select deterministically and keep provenance. Aggregate percentages use summed numerators and denominators from the same eligible subset. Unknown values remain null, including in JSON. The specification defines freshness and aggregation rules.

Equal timestamps and source identities can still carry different tuples. Use eligibility and the three typed field state/raw-value keys as a final canonical comparison, without formatting, floats, or an extra dependency. Keep every candidate and select one whole observation. A provider's array order must not choose a warning, exhaustion finding, or exit code. Canonical order is a reproducibility rule; it does not establish which contradictory observation is physically correct. The separate disagreement policy still applies.

Validate observation fields independently. A fresh used/capacity pair can support a usage alert even when available bytes are missing. Independently report zero available bytes or zero free inodes as critical provider-reported exhaustion. This does not assert that application writes failed. Common-subset totals and `--require-metrics` still require complete fresh byte observations, keeping partial measurements visibly partial.

Preserve raw candidates when observations disagree. A conflicting fresh candidate at the requested severity makes a strict check inconclusive, even when pressure findings cannot assert a definitive value. Use exact integer/rational arithmetic for thresholds, disagreements, and ratio sorting, with display rounding applied afterward. Individual counters are u64; logical aggregate sums are checked u128, with exact overflow-safe ratio intermediates.

### Selection completeness

Track query membership separately from metrics coverage. Filters evaluate to match, no match, or unknown. Only known matches contribute to totals; retain unresolved candidates and reasons separately. Under an incomplete selection, displayed counts are lower bounds and `0` known matches does not establish an empty query result.

Either strict policy (`--require-metrics` or `--fail-on`) returns exit 4 when selection is incomplete. Metrics requirements are then evaluated over a complete selection. Thus denied pod access cannot turn `--node worker-a --require-metrics` into a passing empty check. A complete query that genuinely selects no records can succeed.

## Data collection and permissions

P0 inventory uses the Kubernetes API through the user's kubeconfig. PVC and pod lists are namespace-scoped unless `-A` is supplied. Use targeted referenced PV and storage class gets for namespace queries. For `-A` queries with at least 32 referenced objects of a kind, prefer a paginated bulk list and retain only relevant objects. A denied bulk list falls back to targeted gets within the enrichment budget. Required PV-mode inventory is reused. Missing enrichment permissions produce visible partial results.

Represent request scope with `ApiScope::Cluster` or `ApiScope::Namespace(String)`. Keep report labels separate from validation; a namespace named `cluster` has the same isolation as every other namespace. Recheck object namespaces during both list and targeted-object projection. Apply already-resolved inventory predicates before optional planning: a known CSI driver mismatch in PV mode needs no claim, class, pod, or telemetry request.

Finish required inventory before optional work. Optional consumer and metadata enrichment is capped at 256 HTTP attempts and 30 seconds, reserving at least half the remaining overall time for explicitly requested utilization. The shared concurrency limit is 8 by default; decoded responses are limited to 32 MiB each. Relevant nodes have one logical statistics request each with bounded retries. Exhausted budgets leave unknown fields and diagnostics; they never remove base inventory silently. Record strategy and request/response counters in every report.

Prioritize query membership dependencies, then requested telemetry target discovery, then remaining display metadata. A driver filter gets its PV/class prerequisites before unrelated consumer pages consume its budget. Share prerequisite requests and schedule deterministic budget allocation. Shared cancellation and bounded worker shutdown apply to every failure path. Logs retain allowlisted operation/status/count information and never dump unfiltered node payloads.

Separate PV, class-membership, and class-display enrichment. Derive class prerequisites from normalized unresolved drivers and usable binding metadata. Keep every fetched membership class, including evidence that excludes a candidate; dropping it during display enrichment would change a proved nonmatch into unknown. Remove already-fetched class names from display plans and append new objects on the owned, checked inventory worker. Recheck PV membership after claim verification and apply node predicates after consumer discovery before spending remaining display budget.

`-u` queries `/api/v1/nodes/{node}/proxy/stats/summary` for nodes identified through visible scheduled consumers. Query each relevant node once, with bounded concurrency. The documented [Summary API access path](https://kubernetes.io/docs/reference/instrumentation/node-metrics/) supports volume statistics. The [resource metrics pipeline](https://kubernetes.io/docs/tasks/debug/debug-cluster/resource-usage-monitoring/) supplies CPU and memory metrics; metrics-server is not a PVC usage provider for this project.

The binary performs observation only: it does not patch, delete, mount, exec, or install agents. However, `nodes/proxy` authorization is powerful and is not a narrowly read-only permission. Keep that permission in a separate opt-in RBAC example and describe its implications, as documented in [kubelet authorization](https://kubernetes.io/docs/reference/access-authn-authz/kubelet-authn-authz/). Inventory must work without it. Do not equate direct kubelet `nodes/stats` authorization with API server proxy authorization.

Do not silently fall back to exec, privileged pods, direct kubelet connections, cloud credentials, or another metrics provider. Kubernetes authentication may execute the user's configured kubeconfig credential helper through the Kubernetes client; it must not invoke unrelated commands.

## Integrations

| Integration | Stage | Inputs and behavior | Boundary |
| --- | --- | --- | --- |
| Kubernetes API and kubeconfig | P0 | PVCs, PVs, pods, storage classes; current context and auth | Required inventory plus optional enrichment |
| Kubelet Summary API | P0 | Volume filesystem and inode observations through API server proxy | Explicit `-u`; depends on permissions and driver observations |
| Prometheus and compatible query APIs | P1 | Explicit endpoint and cluster identity; `kubelet_volume_stats_*` metrics | No endpoint discovery or silent provider mixing |
| Workload APIs, Events, VolumeAttachment | P1 | Resolve owner chains and attach event or attachment evidence | Missing permissions preserve the base report |
| ResourceQuota | P1 | Storage request quotas and per-storage-class request quotas | Quota headroom is not filesystem or backend free space |
| VolumeSnapshot APIs | P1 | Readiness, source links, retention metadata, and restore size | CRDs are optional; restore size is not incremental physical storage |
| CSIStorageCapacity and topology | P1 | Advertised provisioning capacity by class and topology | Overlapping segments are not summed into a pool |
| Rook Ceph and Longhorn | P2 candidates | Backend health, pools, replication, and logical-to-physical relationships | Separate adapters, opt-in endpoints or CRDs, validated mappings |
| OpenEBS engines and cloud block providers | P2 candidates | Engine-specific pools, volume properties, billing inputs | Each engine or cloud needs a distinct contract |
| Grafana links and cost tools | P2 candidates | User-configured links or price sources | No embedded credentials or inferred provider pricing |

Prometheus has a documented [query HTTP API](https://prometheus.io/docs/prometheus/latest/querying/api/); volume series are defined in the [kubelet collector](https://github.com/kubernetes/kubernetes/blob/master/pkg/kubelet/metrics/collectors/volume_stats.go). Kubernetes [snapshots](https://kubernetes.io/docs/concepts/storage/volume-snapshots/) and [CSI capacity tracking](https://kubernetes.io/docs/concepts/storage/storage-capacity/) have distinct semantics and must remain separate from live filesystem usage.

No external integration is necessary to ship P0. Each later adapter must specify identity, scope, freshness, permission needs, fixtures, and failure behavior before it becomes supported. Prometheus is the first follow-up because it can serve users who already have telemetry access but cannot use the node proxy.

## Rust architecture

Use one package with a small library and thin CLI. Start with internal modules; split crates only when independent consumers justify it.

```text
CLI options and kubeconfig
          |
          v
Collection plan --> inventory and optional metrics providers
          |                       |
          +------> normalized graph and observations
                                  |
                                  v
                     three-valued selection and field-specific findings
                                  |
                                  v
                     deduplication and grouping
                                  |
                                  v
                         versioned report
                                  |
                    text / JSON / CSV renderers
```

Modules are `cli`, `collect`, `model`, `join`, `metrics`, `qty`, `findings`, `policy`, `group`, `sort`, `report`, and `output`. Provider code returns typed observations and collection diagnostics. Accounting, grouping, findings, and exit policy are pure functions over normalized data. Renderers never fetch data or implement capacity arithmetic.

Use the latest stable Rust and its bundled Cargo, with the exact tested release pinned in [rust-toolchain.toml](../rust-toolchain.toml). The initial pin is 1.99.1, verified against the [official release list](https://blog.rust-lang.org/releases/) on 2026-11-07. Use Rust 2024 edition. Update the pin and declared compiler baseline together after tests pass for each stable release; P0 does not maintain a separate historical-compiler support matrix. Coverage tooling uses the matching compiler components; fuzz-only nightly tooling remains isolated.

Keep normal dependencies limited to justified capabilities: CLI parsing, Kubernetes client/types, async execution/cancellation, serialization/CSV, typed errors, timestamps, and bounded HTTP streaming. The thirteen selected crates are clap, kube/k8s-openapi, tokio/tokio-util, serde/serde_json, csv, thiserror, jiff, http, http-body-util, and Unix-only rustix. Rustix supplies safe nonblocking descriptor status operations so a full pipe can honor cancellation; DEVELOPMENT.md records the explicit budget expansion and required cost measurement. Choose current compatible stable versions after reviewing maintenance evidence and the feature graph. Enable only used features and one Rustls backend; omit controller/runtime/derive capabilities from kube and omit tokio full. Use std for formatting and collections. Wiremock and proptest are dev-only. Exact arithmetic uses bounded std algorithms; adding a bigint or logging framework requires a specific correctness/cost justification. The [dependency policy](DEVELOPMENT.md) defines the checked thirteen-direct-runtime-dependency budget, maintenance checks, and measured graph/build-size review. Manifest checks cannot substitute for a resolved lockfile or upstream maintenance review.

A minimal facade exposes asynchronous collection from a borrowed client/request into an owned snapshot, report construction from `&Snapshot` and `&Query`, and rendering from `&Report` into an injected writer. This is an implementation seam, not a promised stable public SDK in v0.1.

### Domain types and boundaries

Use private-field newtypes for `Bytes`, `Inodes`, `RecordId`, `ConsumerId`, and validated decimal thresholds. Separate aggregate counter types preserve the u128 sum range. Constructors and [TryFrom conversions](https://doc.rust-lang.org/std/convert/trait.TryFrom.html) enforce units, ranges, and threshold precision at the boundary. IDs from different domains must not be interchangeable strings in accounting code.

Use enums and exhaustive matching for field availability, observation freshness/identity, consumer lifecycle, and query match/no-match/unknown. A known field carries its value; missing and invalid variants carry their reasons. Keep valid zero representable. Model observation data and eligibility separately so a partial tuple can support a valid field-specific finding. Convert these types to the schema's status/field/value representation in one report adapter. Rust's [enum model](https://doc.rust-lang.org/book/ch06-00-enums.html) supports carrying the data appropriate to each state.

Use `Option` for genuinely optional metadata; use a richer enum when the distinction between missing, invalid, and unavailable affects accounting or policy. Keep validation fields private so callers cannot bypass constructors or create contradictory states directly.

Keep three explicit layers: API transport types → validated domain types → serialized report types. Use serde at the transport/report boundaries with explicit wire names. Transport metric fields must retain missing and malformed values long enough for independent validation; one invalid numeric field must not discard valid siblings. Do not deserialize untrusted input directly into domain newtypes or derive the external report contract from internal enum layout.

Keep unknown classifications typed through all three layers. `VolumeMode::Unsupported(String)` preserves a future API value, while `Invalid` retains a wrong-type failure without inventing a string or default. The alpha report admits open string/null volume modes and explicit unknown consumer lifecycle with nullable activity. Unsupported lifecycle makes only related counts unknown; graph validation rejects complete visibility or numeric counts for those references. Exhaustive policy matching separates strict health classification from independently attributable byte metrics. These states retain the report and diagnostic rather than turning unrelated inventory into a serialization failure.

Accept unknown incoming Kubernetes/provider fields. Decode open external status values as strings or `Other(String)` and retain the original value; internal policy enums remain closed and exhaustively matched. Validate classifications needed by a rule or filter. An unsupported classification produces a structured diagnostic and unknown result for dependent logic, preserving unrelated fields. Required identities and structurally invalid documents still follow the fatal/optional boundary in SPEC.md. A closed report enum requires an explicit unsupported-value path or a versioned schema change before emitting a new value.

### Arithmetic and ordering API

Centralize counter validation, checked sums, threshold comparison, disagreement, ratio ordering, and display rounding in qty. Expose named checked operations on the numeric newtypes and a validated `ExactRatio`; callers do not recreate division or overflow handling. Construct ratios with a positive denominator, reduce to canonical form, and make equivalent fractions equal. Normalize zero numerators consistently; a zero denominator returns a validation error.

Implement `Eq`, `PartialOrd`, and `Ord` consistently on `ExactRatio`, following Rust's [Ord contract](https://doc.rust-lang.org/std/cmp/trait.Ord.html). The implementation uses canonical fractions and Euclidean continued-fraction comparison, keeping ordering exact across u128 without cross-product overflow or an arbitrary-precision dependency. Bounded decimal long division supplies half-up display rounding separately. Compose a query-specific comparator: availability first, requested numeric/ratio direction next, ascending identity tie-breaks last. Reverse only the primary value comparison for descending order, preserving unknown-last and ascending ties. Test equivalent fractions, extreme counters, transitivity, and input permutations.

### Clock interface

Define one injectable Clock interface with separate monotonic and UTC readings. Use monotonic instants for deadlines, retry waits, and shutdown elapsed time; use UTC for provider/report timestamps and freshness. Map the monotonic implementation to Tokio's timer source so paused-time tests control it. Never compute a duration allowance by subtracting wall-clock timestamps. Evaluate freshness against one captured UTC reading at collection end; diagnose clock anomalies under the existing timestamp rules. Tests move wall time forward/backward while monotonic time advances independently.

### Ownership and errors

Collection owns one normalized snapshot. Pure accounting functions and renderers borrow immutable data; relationships and groups refer to stable IDs rather than copying full Kubernetes objects. Workers return owned results to one coordinator, which builds the graph. Share immutable client/configuration state and cached transport documents with Arc when tasks require them. Typed objects remain independently owned; a cache hit must not recursively clone a document on the async executor. Record the first fatal failure once instead of rescanning all negative cache entries on every lookup. Add small clock/provider seams where substitution is needed; use concrete types elsewhere and keep one Cargo package.

Use typed `Result` errors, implemented with `thiserror`, for failures that prevent a required result. Expected optional failures become structured diagnostics with their affected fields and collection completeness. Preserve error sources internally and expose safe context at the CLI boundary. Avoid `unwrap`/`expect`, unchecked indexing, and lossy casts on external data; malformed input follows the specification's error/diagnostic paths. Use exhaustive matching to keep new states visible to rule and policy code.

### Task ownership and verification

One supervisor owns collection tasks through [Tokio JoinSet](https://docs.rs/tokio/latest/tokio/task/struct.JoinSet.html), enforces a bounded task window and shared request permits, and propagates a [CancellationToken](https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html). Reserve request/budget slots before dispatch so scheduling cannot choose winners. Drain completed results, surface task failures, and cancel/abort remaining tasks under the specified shutdown bound. Sort normalized results independently of completion order.

Bound CPU work as well as I/O: enforce production byte/depth/scalar limits before expensive conversion, avoid allocation proportional to a numeric exponent, and normalize in batches of at most 64 objects/candidates before checking cancellation/deadlines and yielding. A [Tokio timeout](https://docs.rs/tokio/latest/tokio/time/fn.timeout.html) does not preempt synchronous work that never yields. The one-second shutdown allowance starts when the supervisor observes cancellation; parsing checkpoints and work caps bound the work before observation. Blocking offload requires cooperative cancellation and joining, rather than relying on abort of an already running blocking task. Stress tests cover cancellation during decoding/normalization as well as waiting HTTP futures.

Keep arithmetic and rule helpers pure and synchronous. The coordinating collector/report builder performs cancellation checks and yields between bounded helper batches, preserving deterministic computation without putting timers or runtime calls inside numeric operations.

The collector's inventory normalization, membership selection, node-target discovery, and statistics normalization run in sequential owned `spawn_blocking` jobs. One `Checkpoint` seam specializes pure callers to `Infallible` and command work to typed stop reasons. A command worker checks cancellation and monotonic time and yields its OS thread at least every 64 work items; nested mounts, owner references, volume samples, candidate windows and merge steps share that budget. Stable sorting uses bounded runs of 64 items followed by checked merges, without an additional sorting dependency. The command awaits each worker handle, including on interruption; aborting an asynchronous wrapper is not a substitute for joining CPU work.

Finalize diagnostic order in the shared report path using the same checked sorting primitive. Canonicalize each affected-field set before comparing full diagnostic keys and retain repeated occurrences. Inventory and malformed-sample permutations must produce identical reports, exits, all text views, and flat CSV rows. Nested-field cancellation checks apply before wire conversion as well as during serialization.

Decode each received body within its originating request/stage/overall deadline on a joined CPU worker. Apply the lexical byte/depth/scalar guard and a maintained Serde parser through bounded 8 KiB reads, with immediate cancellation/deadline checks at those boundaries. The pure/fuzz decoder uses the same parser with an infallible control. Keep parsing owned by the collector supervisor; abortable node tasks return raw bounded bodies and never own blocking parsers.

Project API objects on owned workers with checked traversal of metadata labels, owner references and opaque JSON containers, plus UTF-8-safe bounded string copies. Preserve the specified missing/null/type behavior; compare projection against an independent Serde DTO in regression tests. Page projection retains valid optional siblings, including pages preceding or exposing a repeated continuation token. Plan claim, PV and class prerequisites on owned inventory, indexing selected UIDs within their resource kind. PVC display names cannot establish PV membership or add unrelated class lookups. Build server-derived request paths before dispatching abortable HTTP tasks.

After required base inventory completes, an elapsed collection deadline stops new network dispatch while finite projection of retained inventory and already validated observations drains cooperatively. User cancellation still stops that projection. This preserves the specified partial report instead of losing base objects to a report-building timeout. Production verification of projection/planning, output interruption, and end-to-end decode behavior remains an implementation gate.

Unit/property tests cover pure logic, wiremock tests cover transport behavior, CLI tests cover serialization and exit policy, and kind validates real integration. Use cargo-llvm-cov for coverage: every core module (`qty`, `join`, `group`, `findings`, `policy`) must reach 90% line coverage and production source overall must reach 80%. Required boundary/error cases remain independent gates. The [development guide](DEVELOPMENT.md) defines measurement, artifacts, and exclusions; the [implementation phases](IMPLEMENTATION_PLAN.md) define when gates activate. Parser fuzzing begins as each Rust parser lands.

## Repository setup

This layout guides implementation. The documentation, schema, and contract examples exist before the Rust executable.

```text
Cargo.toml / Cargo.lock           Package, binary, library, pinned dependency resolution
rust-toolchain.toml               Pinned toolchain, rustfmt, clippy
mise.toml                        Optional task runner; standard cargo commands still work
.github/workflows/ci.yml          Formatting, linting, tests, feature and platform checks
.github/workflows/release.yml     Build archives, checksums, validate Krew manifest
.github/dependabot.yml            Dependency and GitHub Actions updates
.krew.yaml                       Release manifest template
release-plz.toml / cliff.toml     Release PR and changelog automation
src/main.rs / src/lib.rs          Thin executable and reusable application core
src/collect/ / src/metrics/       Kubernetes collection and provider implementations
src/model.rs / src/qty.rs         Typed data and exact quantities
src/join.rs / src/group.rs        Identity relationships and deduplicated aggregates
src/findings.rs / src/policy.rs   Pure finding rules and exit policy
src/report.rs                    Explicit versioned report adapter
src/output/                      Tree, table, summary, issues, JSON, CSV
tests/fixtures/                  Sanitized API objects and provider samples
tests/                           CLI, fixture, arithmetic, and mocked API tests
fuzz/                            Parser fuzz targets and regression corpus as parsers land
docs/schema/report.schema.json   Structural report contract
examples/contract/               Shared API scenarios and expected JSON/text reports
scripts/validate-contract.py     Schema and semantic checks on the design fixtures
examples/                        Reports, RBAC profiles, integration walkthroughs
docs/DESIGN.md / docs/SPEC.md     Product decisions and behavioral contract
docs/IMPLEMENTATION_PLAN.md      Numbered phases, dependencies, and exit gates
docs/DEVELOPMENT.md              Local tools, disposable clusters, and CI strategy
docs/DF_PV_LESSONS.md            Source-linked implementation/support regressions
docs/COMPATIBILITY.md            Driver/version capability evidence
dev/kind/ / dev/versions.json    Disposable topology configurations and version selections
README.md / CHANGELOG.md          Quick start and release notes
LICENSE / CONTRIBUTING.md        Apache-2.0 license and contributor workflow
```

Use the reference project's compact Cargo structure and release template pattern. Standard Cargo commands are the required development interface; mise provides optional task aliases. Use release-plz for release PRs, git-cliff for changelog generation, and GitHub Actions for checksummed archives and the generated Krew manifest. Commit Cargo.lock for repeatable binary builds. License the project under Apache-2.0 and preserve notices for any reused code. Dependency compatibility and the actual release workflow remain implementation checks.

CI should run formatting, Clippy, unit and fixture tests, CLI contract checks, and mocked Kubernetes API tests. Reuse each contract scenario's API inputs to verify report construction and all four text views; do not generate the expected values with the implementation under test. Property checks cover deduplication, grouping-order invariance, three-valued selection, and field-specific findings. Integration tests use a disposable cluster; generic local provisioners validate inventory and graceful absence of metrics, while a fixture suite and a metrics-capable CSI environment validate usage. Record tested combinations rather than promising every CSI driver works.

Release targets: Linux amd64/arm64 and macOS amd64/arm64 for P0; Windows amd64 after CLI, authentication, and archive smoke tests pass. Use x86_64/aarch64 unknown-linux-musl targets on Linux and the matching apple-darwin targets on macOS. Build one self-contained binary per target, archive it with the license, publish SHA-256 checksums, and generate platform entries from actual artifacts. The [distribution procedure](DISTRIBUTION.md) defines deterministic archives, target/header/static-Linux validation, manifest checks and an isolated real-Krew installation gate. Header/archive checks do not establish source provenance or native runtime/authentication behavior. Follow the [Krew manifest contract](https://krew.sigs.k8s.io/docs/developer-guide/plugin-manifest/). Published installation is intended to be `kubectl krew install view-storage`; local manifest testing precedes any index submission.

## Release scope and implementation phases

P0 includes inventory, relationships, all four views, exports, kubelet usage, findings, resilience, and distribution. P1 starts with Prometheus and refresh, then adds operational evidence and storage context. P2 covers separately validated backend adapters and planning features.

The [implementation plan](IMPLEMENTATION_PLAN.md) owns the numbered phases, dependencies, deliverables, and exit evidence. P0 is complete when A01–A58, the property/failure requirements, the Rust coverage gates, required disposable-cluster gates, and four-platform release checks pass. Runtime and memory claims require measured evidence.

## Development environment decision

Use Docker Desktop and kind on the current Mac, native Rust builds, and Docker Engine/kind in Linux CI. Separate a three-node inventory profile from a single-node CSI hostpath 1.19.0 telemetry profile. Use wiremock for deterministic HTTP failures and proptest for arithmetic/graph invariants. Baseline Kubernetes is 1.37.0, with 1.36.4 and 1.35.8 compatibility profiles; checked-in images are digest-pinned. [DEVELOPMENT.md](DEVELOPMENT.md) defines setup, isolation, workload limits, and evidence requirements, and [COMPATIBILITY.md](COMPATIBILITY.md) records actual outcomes.

## Recorded decisions

These choices are the implementation baseline. Changes require updating this decision table and the corresponding specification together.

| Decision | Selected behavior | Rationale |
| --- | --- | --- |
| Namespace scope | Current kubeconfig namespace; `-A` for all namespaces | Supports a useful first run with namespace permissions and familiar kubectl scope |
| Initial presentation | Tree grouped by `storageclass,namespace,pvc` | Makes capacity hierarchy and claim identity visible in the first report |
| Other presentations | Table, summary, and issues in P0 | Covers detailed inspection, capacity review, and investigation with the same query |
| Usage collection | Explicit `-u`; kubelet provider in P0 | Makes privileged collection, driver dependence, and network cost deliberate |
| First follow-up integration | Prometheus, with refresh mode, at the start of P1 | Reuses the report model and offers a telemetry path for users without node proxy access |
| Remaining P1 order | Workload and operational evidence, then quotas, snapshots, and topology | Builds investigation context on top of working inventory and telemetry |
| License | Apache-2.0 | Provides an explicit patent grant and clear contribution and redistribution terms |
| Configuration file | None through P1; use CLI flags and explicit credential files | Keeps invocation reproducible and endpoint selection visible |
| Interactive terminal UI | Outside P0 and P1; P1 provides `--watch DURATION` | Refresh mode addresses recurring observation while preserving the existing views |
| Repository workflow | Cargo, committed lockfile, optional mise aliases, release-plz, git-cliff, GitHub Actions | Keeps local builds straightforward and release artifacts repeatable |
| Initial platforms | Linux and macOS, each on amd64 and arm64 | Defines a concrete four-platform release gate; Windows remains a later expansion |
| Local development | Native Cargo, existing Docker Desktop, kind; matching Docker Engine CI | Reuses installed tools and disposable cluster configurations |
| Test storage | Default local provisioner for inventory; separate CSI hostpath profile for telemetry | Separates unavailable-metrics behavior from real statistics collection |
| Rust domain model | Validated newtypes, enums, explicit API/domain/report conversions | Makes units and data validity enforceable before accounting |
| Ownership and task lifecycle | Immutable snapshots, pure borrowed computation, supervised bounded tasks | Keeps collection failure and completion order out of accounting |
| Code coverage | 90% for each core module; 80% production source overall | Measures test reach alongside required acceptance/error cases |
| Rust and Cargo | Latest stable, exact tested toolchain pin, Rust 2024 edition | Keeps development current and builds reproducible |
| Dependency scope | Maintained crates, minimal features, one TLS backend, dev-only test tools | Controls transitive cost while retaining proven protocol and correctness code |

The schema, example reports, and current stable toolchain pin establish the initial baseline. Concrete crate versions/features, live compatibility checks, and measured dependency/build/runtime budgets remain implementation deliverables as the Cargo package is created.

## Credential execution ownership

Keep configured credential execution inside an owned command process on the supported Unix targets. The parent resolves the selected kubeconfig/context, validates the stdin contract and selects the direct or isolated path through an explicit load outcome. Static credentials use the direct path. Exec/auth-provider configurations use a child running the same executable and arguments; kube continues to own credential parsing, TLS, proxy and authentication behavior. The public command API stays unchanged.

RAII guards own the child process group, original descriptor flags, foreground terminal and private 0700 startup directory. Its 0600 control files store readiness/stage markers; the shared budget uses a process-independent monotonic clock. Block SIGTTOU only around each synchronous terminal handoff, then restore the calling thread's signal mask before any await. Relay terminal-generated Ctrl-C to the parent, distinguish parent-initiated shutdown, allow at most one cooperative second, then terminate and reap the owned group. Observe the leader without reaping it before the final group signal to prevent PID reuse. Linux adopts/reaps orphaned helper descendants; macOS observes exit through kqueue.

The parent bounds synchronous SDK configuration. After configuration, the collector retains its request/stage deadlines and partial-inventory behavior. Report finalization/output keep the direct path's cancellation semantics. An isolated worker joins application-owned collection/CPU/output work, flushes/restores stdio and exits with the computed result, allowing the parent to clean SDK-internal blocking refresh jobs that outlived cancelled requests. A configuration timeout is fatal; missing optional telemetry still retains the report with the appropriate ordinary/strict exit.

Use one safe Unix API crate for these operations. The dependency review records Nix's selected features and cost; project-owned unsafe FFI remains forbidden. Disable kube's default retry middleware so one adapter call corresponds to one wire attempt and the collector remains the sole retry/budget owner.
