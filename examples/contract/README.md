# Contract scenarios

These fixtures define the draft 0.5 report contract. Each scenario supplies the same API inputs for report construction and all four text views. The expected examples are maintained independently of the Rust implementation.

The [manifest](manifest.json) maps each scenario to its input bundle, complete expected report, view files, view options, and expected exit code. All reports conform to the [JSON Schema](../../docs/schema/report.schema.json). The [specification](../../docs/SPEC.md) governs accounting, completeness, findings, and collection behavior beyond structural validation.

| Scenario | Contract covered | Expected exit |
| --- | --- | --- |
| [Healthy](healthy/report.json) | One claim, two container mount declarations, one consumer edge | 0 |
| [Shared RWX](shared-rwx/report.json) | Three consumers on two nodes; root capacity counted once | 0 |
| [Partial permissions](partial-permissions/report.json) | PV get denied; known claim and attributable telemetry remain visible | 0 |
| [Missing and stale metrics](missing-stale/report.json) | Records retained; unavailable aggregates and telemetry findings | 0 |
| [Empty selection](empty/report.json) | Complete zero-match inventory passes required metrics | 0 |
| [Unknown selection](unknown-selection/report.json) | Denied pod access prevents node filtering; strict check is inconclusive | 4 |
| [Field pressure](field-pressure/report.json) | Zero available bytes/free inodes and a high usage pair with missing available bytes | 4 |
| [Ambiguous critical usage](ambiguous-critical/report.json) | Fresh 90%/99% conflict makes the requested critical check inconclusive | 4 |
| [Terminating consumer](terminating-consumer/report.json) | Deleting but nonterminal pod remains a node-filter match and collection target | 0 |

Each directory contains:

- input.json: fixed clock, resolved query, sanitized Kubernetes objects, list resource versions, Summary API responses, and explicit HTTP failures.
- report.json: complete expected normalized report.
- tree.txt, table.txt, summary.txt, and issues.txt: expected content for the corresponding text view. The table examples enable --wide; the manifest records that option.

UIDs are synthetic opaque identifiers. Hosts, driver names, handles, namespaces, and paths contain no credentials or real cluster data. Failure entries override access to the corresponding raw objects; a fixture may retain an object to describe what exists even when the caller cannot read it. Request and byte counters illustrate the report shape; HTTP integration tests must independently verify transport accounting against actual response bodies and retries.

Text layout can evolve, but record counts, distinct-member totals, status labels, declared paths, source evidence, completeness statements, and exit behavior are assertions. The table examples include values from stale observations with explicit stale status; those values must not enter fresh aggregates or pressure findings.

Run structural and semantic validation from the repository root:

```sh
uv run scripts/validate-contract.py
```

Alternatively, run the script with Python 3.12 or newer and jsonschema 4.26.0 installed. The validator checks schema shape, API-to-report quantities and observations, consumer lifecycle, declared mounts, group unions, weighted totals, completeness, field-specific findings, strict exits, and view content. Negative mutations reject dangling references, duplicate edges, inflated RWX totals, false empty success, suppressed pressure findings, and false passing conflict checks. Exact integer/rational checks cover both sides of an 80% boundary that rounds to the same displayed value and percentage rounding ties.

Query thresholds are decimal strings. Each metric family preserves its raw candidates and provenance; displayed percentages and range endpoints are informational and do not replace exact candidate comparisons.

Feed each input bundle into the mocked collection layer, compare its normalized report with the expected report, and pass that report through each renderer. Do not regenerate expectations using the implementation being tested. Additional Rust pipeline regressions export shared-volume identity, class assignment, unfamiliar/wrong-type volume modes, unfamiliar pod phases, and independently known live metrics for the independent schema/semantic verifier. Draft 0.5 preserves unfamiliar mode strings, represents wrong-type modes as null, and adds unknown consumer lifecycle with null activity. Existing known-value examples remain valid. API pagination, budget exhaustion, races, block mappings, invalid observations, and dimension-order property tests remain acceptance requirements in SPEC.md; the nine scenarios do not claim to cover every acceptance case. The [implementation plan](../../docs/IMPLEMENTATION_PLAN.md) orders that work.
