# Phases 0–5 acceptance audit

Original 2026-11-07 checkpoint against [the specification](SPEC.md) and [phase exits](IMPLEMENTATION_PLAN.md). Final validation remains in progress. Native macOS ARM64 and Linux ARM64 pass 169 Rust tests; emulated Linux AMD64 passes 168 before one test-only addition. Full-source coverage is 93.44%. Eleven randomized laws, both parser fuzz smoke targets, nineteen pipeline reports and nine actual HTTP reports pass. Inventory/RBAC and CSI lifecycles pass on Kubernetes 1.35–1.37. Four genuine target archives and a native macOS ARM64 isolated Krew install pass. [Implementation evidence](IMPLEMENTATION_STATUS.md) records source/platform limits.

## Operator calculations (A59–A65)

Updated 2026-11-11. The calculation extension adds seven accepted cases to the original audit below. Local evidence passes on the source fingerprint recorded in [implementation evidence](IMPLEMENTATION_STATUS.md#operator-calculation-extension); four-platform hosted validation remains required on the selected release revision. Earlier A01–A58 evidence retains its original scope.

| ID | Evidence examined | Remaining release verification |
| --- | --- | --- |
| A59 | Unit/large-counter/unknown tests; actual executable exports; real MB/GB/TB captures | Fresh native release matrix |
| A60 | Independent Python integer/Fraction distribution oracle, corrupted-stat rejection checks and real class exports | Fresh native release matrix |
| A61 | Canonical mode-set/Block/phase group tests and actual HTTP type breakdowns | Fresh native release matrix; raw-block telemetry remains outside scope |
| A62 | Owner/unknown/duplicate-controller tests, real HTTP filter, real two-replica kind StatefulSet with one shared claim and owned deletion | Fresh hosted storage matrix |
| A63 | Positive/zero/negative/repeated/resized/conflicting growth tests, actual saved-report CLI comparison, independently checked CSI write | Fresh native/hosted matrix; two-sample estimates only |
| A64 | Invalid scope/filter/file/size/FIFO inputs, checked overflow, cancellation and process/output tests | Fresh native release matrix |
| A65 | 189 Rust tests on native macOS/Linux arm64; five calculation reports independently validated per host; module coverage 97.50%; real text/JSON/CSV captures | Fresh native amd64/macOS/Linux release matrix; no publication claimed |

## Phase exits

| Phase | Evidence available | Exit still missing |
| --- | --- | --- |
| 0 | Real Rust/Cargo 1.99.1 locked build, thirteen-crate guard and resolved checks; native feature/size baseline; pinned fixtures and reviewed CSI lock; two inventory lifecycles and CSI telemetry on Mac ARM64 | Final per-target provenance reconciliation and native Linux CI lifecycle; upstream review and repeated older-version/CSI evidence now exist |
| 1 | Typed boundaries, exact arithmetic/clocks/graphs; actual CLI build/help/version; eleven randomized property laws; full production coverage floors | Remaining per-case/native platform verification; both bounded parser fuzz targets pass |
| 2 | Real kube adapter and actual mock-HTTP paths/counters; live TLS/service-account inventory/RBAC verification | Final per-case audit and native Linux compatibility jobs; older-version live checks pass |
| 3 | All views/36 goldens, CSV encoder, ordering properties and real output cancellation; real kubectl discovery | Final serialization/selection evidence audit and native installation matrix |
| 4 | Real fresh CSI byte/inode observations and shared 16 MiB write; pressure/conflict/overlap and active clock/cancellation tests | Final release-binary positive/negative CSI gate and native Linux CSI jobs; live Block/RWX remain unverified |
| 5 | Locked test/coverage/report evidence, live RBAC/CSI, native release/archive and source-package preparation | Outstanding A01–A58 scopes below; final source installation, live negative gate and native CI; local Krew and all four artifact builds pass |

Phase 5 now requires all A01–A65, including A30. Phase 6's publication work does not remove A30 from this goal. Local preparation/installation remains required; external publication is a separate action.

## Acceptance items

`unit/` and property links now refer to the real locked production run. Twenty-six HTTP/executable tests now pass on native macOS and ARM64 Linux. Evidence for individual cases varies; the remaining column preserves scenarios not yet established through the required path or platform. Passing coverage does not close those gaps.

| ID | Evidence examined | Remaining verification/work |
| --- | --- | --- |
| A01 | Real context/namespace HTTP and restricted-account inventory checks pass on all three selected Kubernetes minors | Native Linux-host compatibility jobs pending |
| A02 | Actual explicit namespace, namespace literally named `cluster`, `-A`, empty strict coverage and invalid-flag tests pass | Native matrix pending |
| A03 | [Lifecycle/owner normalization](../tests/unit/join.rs); [inventory fixture](../dev/fixtures/inventory-cases.json) | All stated PVC lifecycle cases through actual inventory/outputs |
| A04 | [PV normalization](../tests/unit/join.rs); real cluster PV query retains Available/Released records | Complete null-claim-quantity inspection and older/Linux matrix |
| A05 | [Denied PV pipeline](../tests/unit/collect.rs); live namespace-only/denied-PV accounts retain inventory and unknown capacities | Complete class diagnostic/export assertions and older/Linux matrix |
| A06 | Actual forbidden base/page-two binary queries exit 1 with empty stdout | Native matrix pending |
| A07 | Actual denied-pod HTTP report retains unknown consumer counts and avoids false no-consumer findings | Native matrix pending |
| A08 | [Group unions](../tests/unit/group.rs); [shared consumer-addition law](../tests/support/mod.rs), anchored across active/terminating/terminal/unknown lifecycle | Randomized laws and actual shared-fixture HTTP outputs pass; native CI matrix remains |
| A09 | [Mount/relationship normalization](../tests/unit/join.rs) | Real repeated container references without duplicate edges/capacity |
| A10 | [Real selector HTTP test](../tests/client.rs); live claim selector retains four consumers with independent pod labels | Native baseline verified; older/Linux matrix remains |
| A11 | Real mixed-node timeout/success HTTP query retains inventory, good observations and partial totals; wire costs checked | Native matrix pending |
| A12 | Actual denied-proxy HTTP and live restricted queries retain inventory with ordinary/required exits 0/4 | Native Linux-host matrix pending |
| A13 | [Metric field validation](../tests/unit/metrics.rs); [quantity model](../tests/unit/model.rs) | Complete end-to-end missing versus exact zero observations |
| A14 | Actual PVC/PV JSON and CSV preserve 100Gi request, 103Gi bound, 120Gi PV, 118Gi filesystem, 47Gi used and provider 60Gi available | No physical-backend capacity claim; native matrix pending |
| A15 | Actual PVC/PV Block JSON/CSV retain logical capacity and device declarations; bytes/inodes n/a and zero node reads | Live Block provider unverified; native matrix pending |
| A16 | [Family selection/freshness](../tests/unit/metrics.rs); [family findings](../tests/unit/findings.rs) | Production transport cases for every freshness/state variant |
| A17 | Actual conflicting-candidate HTTP export and deterministic input/candidate permutations pass; no summing | Final per-case audit and native matrix pending |
| A18 | [Weighted/skew totals](../tests/unit/group.rs) | Actual multi-node 61-second skew export with retained leaves |
| A19 | [Weighted totals](../tests/unit/group.rs); [exact ratios](../tests/unit/qty.rs) | Explicit full-small/empty-large whole-report case and randomized accounting |
| A20 | Actual distinct-capacity JSON/CSV retain provider available independently of capacity minus used | Native matrix pending |
| A21 | [Binding/recreation checks](../tests/unit/join.rs); [identity metrics](../tests/unit/metrics.rs) | Real HTTP identity-race/recreation scenarios and no stale attribution |
| A22 | [Exact quantity regressions](../tests/unit/qty.rs) and eleven-law locked Proptest run | Both bounded parser fuzz targets pass; remaining native target matrix |
| A23 | Actual complete-empty, unknown-selection and forbidden-base HTTP/executable outcomes are distinct | Native matrix pending |
| A24 | Actual warning-bearing JSON/CSV parse successfully; null/zero/order assertions and nine independent HTTP reports pass | Native matrix pending |
| A25 | Exact threshold/zero-denominator randomized/unit laws and actual missing/zero byte/inode JSON/CSV exports pass | Native matrix pending |
| A26 | Actual partial/invalid-field pressure queries retain findings and required-metrics exit 4 | Native matrix pending |
| A27 | [Union/group dimension tests](../tests/unit/group.rs); [shared dimension/permutation law](../tests/support/mod.rs) checks exact sums, child unions and actual node/pod correlations | Randomized laws and actual HTTP outputs pass; native CI matrix remains |
| A28 | [Known-overlap pipeline](../tests/unit/pipeline.rs); [independent negative checks](../scripts/validate-produced-reports.py) | Production adapter/export execution; no physical-backend capability claim |
| A29 | Actual JSON/CSV/text broken-pipe and descriptor restoration pass with direct/supervised credentials on Mac and both Linux containers | Complete control-character/NO_COLOR binary export audit and native matrix pending |
| A30 | All four genuine binary/LICENSE archives, complete verified manifest and actual isolated Krew install on Mac ARM64; seven configured/plugin checks and three installed PTY cases | Refresh final source-package install; native four-platform runtime/install CI pending; no publication claimed |
| A31 | Actual denied-pod node filter exits 4 under both strict options with incomplete selection | Native matrix pending |
| A32 | Actual complete-empty binary query reports explicit 0/0 coverage and strict exit 0 | Native matrix pending |
| A33 | Actual missing-available HTTP JSON/CSV retain pressure, exclude partial tuple and exit 4 when required | Native matrix pending |
| A34 | Actual zero-available export retains critical byte-exhaustion source/field/timestamp evidence | Native matrix pending |
| A35 | Actual zero-free-inode export retains independent critical inode-exhaustion evidence | Native matrix pending |
| A36 | Shared-mount fixture runs through actual HTTP JSON/CSV/four views; declarations and capacity/count semantics checked | Final declaration audit and native matrix pending |
| A37 | Actual fixture outputs include read-only, literal subpath expression and Block device declarations | Final declaration audit and native matrix pending |
| A38 | Real compiled 10,000-claim collector scale fixtures verify bulk strategy and per-node request counts; separate actual HTTP tests verify pagination/cost counters | Scale fixture is in-memory transport, not a 10,000-object network benchmark; native CI pending |
| A39 | Real HTTP 300-claim case passes: one denied PV list, 254 permitted gets, 256 enrichment attempts, exact body bytes and retained unknown remainder | Native baseline verified; older/Linux matrix remains |
| A40 | [Collector budget/deadline/cancel cases](../tests/unit/collect.rs) | Actual mixed enrichment/time truncation and reserved telemetry time |
| A41 | Locked contract/pipeline suite and seven semantic rejection checks pass with independent graph validation | Native matrix pending |
| A42 | Actual out-of-range counter JSON/CSV diagnose invalid sibling while preserving valid zero-pressure findings | Native matrix pending |
| A43 | Actual fresh 90%/99% conflict retains raw candidates, suppresses definitive pressure and exits 4 | Native matrix pending |
| A44 | [Rounded exact boundaries](../tests/unit/qty.rs); [independent Fraction oracle](../scripts/validate-contract.py) | Locked/randomized and actual output boundary assertions |
| A45 | [Terminating fixture](../examples/contract/terminating-consumer/report.json); [node targets](../tests/unit/metrics.rs) | Actual held-terminating pod remains a matching telemetry target |
| A46 | [Driver dependency/request-order cases](../tests/unit/collect.rs) | HTTP large-unrelated-page/budget scenario and live query |
| A47 | [Unscheduled/source attribution](../tests/unit/metrics.rs); [inventory fixture](../dev/fixtures/inventory-cases.json) | Actual no-empty-name request and retained unscheduled reference |
| A48 | [Shared cache/failure tests](../tests/unit/collect.rs) | Real repeated-sample/enrichment-failure request counter assertion |
| A49 | [Joined CPU/HTTP/output failure sources](../tests/unit/collect.rs); [output integration source](../tests/client.rs) | Actual producer failure while another request and output reader are active; bounded completion |
| A50 | Actual trace/private-payload tests plus v1/v1beta1 credential success/failure, interactive input and stubborn helper lifecycle checks pass | Remaining native target checks |
| A51 | Seven configured discovery/help/version/flag/direct-versus-plugin checks pass on release and Krew-installed Mac ARM64 binary | Native platform matrix pending |
| A52 | [u64/u128 exact arithmetic](../tests/unit/qty.rs); [aggregate overflow](../tests/unit/group.rs) | Latest locked/property runs and production serialized null/diagnostic checks |
| A53 | Positive fresh CSI byte/inode/write gates repeat on Kubernetes 1.35–1.37; seventeen offline no-skip/failure checks pass | Final release-binary positive and negative live missing-tuple gate; native Linux lifecycle pending |
| A54 | Capability matrix separates live local-provisioner inventory, restricted proxy and hostpath byte/inode support on all three selected minors | Native Linux-host matrix pending; live cross-node RWX/Block and other providers unverified |
| A55 | Locked randomized record/group quantity/ratio laws, extreme cross-products, ties/unknowns, both directions and renderer/CSV-row permutations | Remaining native target matrix and full export audit |
| A56 | Actual locked active retry/request/overall/shutdown UTC-jump regressions; fresh/stale/future samples and no-late-work assertions | Remaining adapter failure combinations and native targets |
| A57 | Bounded decoder/CPU cancellation tests, actual oversized success/error streams and owned helper/output shutdown checks pass; both ASan parser fuzz targets pass | Remaining native matrix and final per-case audit |
| A58 | Actual PV unfamiliar phase/mode and unknown extra-field HTTP queries preserve open values and unrelated capacity; strict exit 4; malformed-required-field tests remain fatal | Native matrix pending |

## Audit rules

The eleven Proptest laws have compiled and run on native macOS/ARM64 Linux and emulated AMD64 Linux. Parser fuzz smoke, upstream review, older-version live lifecycles and native Mac Krew installation are completed evidence at their recorded checkpoints. They must not be listed as absent merely because earlier documents predated those runs.

Rows retaining a final per-case audit are not signed off solely from coverage. Check assertions against the specification, preserve narrower source/runtime scopes, and retain any genuine gap. Native Intel execution, native Linux-host lifecycle, final source installation and the negative live telemetry gate remain separate requirements. No unavailable provider capability is converted into a successful metrics-support result.
