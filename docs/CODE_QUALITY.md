# Rust design and complexity

Production Rust functions have a cyclomatic complexity ceiling of **20**. Aim
for **10 or less** in new functions. Twenty is a ceiling for validation,
orchestration, and other branch-heavy work, not a target for ordinary helpers.

Run the gate from the repository root:

```sh
make complexity
uv run scripts/test-complexity.py
uv run scripts/check-complexity.py --json artifacts/complexity.json
```

The checker uses [Lizard](https://github.com/terryyin/lizard), pinned to 1.26.0
in its script metadata and `dev/requirements-ci.txt`. It scans every `.rs` file
under `src/`, including private submodules. There is no grandfathered baseline
or per-function exemption. Tests live under `tests/` and fuzz harnesses under
`fuzz/`; they are outside this production gate. The checker fails when it finds
no functions, and its regressions cover nested modules, threshold boundaries,
lifetimes, generics, methods, async functions, comments, and strings.

This is Lizard's source-level CCN metric, not a compiler control-flow graph or
Clippy's cognitive-complexity score. It counts Rust error propagation (`?`) and
match alternatives as well as loops and conditions. Keep the analyzer pinned:
a parser change can change the measurement even when the source is unchanged.
Review the measurements and parser regressions when updating it.

`make check` runs both the checker and its regressions. The always-run CI
`contracts` job runs them after installing the pinned Python requirements.
Clippy continues to run with warnings treated as errors; complexity checking
adds no Rust runtime dependency.

## Design rules

- Extract a helper when it owns a distinct operation or decision. Use names
  such as binding resolution, consumer membership, or candidate eligibility.
  Keep stage order visible in orchestration. Avoid arbitrary numbered chunks
  and tiny wrappers created only to lower a score.
- Put related implementation details in private submodules. Keep their
  interfaces visible only to the parent when possible. Do not expand the
  public library API to support an internal refactor.
- Use named structs for related results or shared context when a tuple or long
  argument list obscures meaning. Prefer borrowed indexes and existing domain
  types over cloning full graphs or introducing parallel models.
- Preserve the collection, domain, report, and presentation boundaries.
  Rendering consumes the same finalized report; it does not collect resources
  or decide health independently. Keep exact arithmetic in the accounting
  layer and rounding in presentation.
- Keep cancellation and error propagation explicit across helper boundaries.
  Long loops still need checkpoints. Network workers, CPU workers, credential
  helpers, and output retain their existing ownership and shutdown rules.
- Use behavior tests and independent report oracles to validate refactors.
  Do not replace expected output with implementation-generated expectations or
  lower coverage floors to accommodate moved code.

## Initial review

The initial scan found 28 production functions above 20, with a maximum of
107. After extracting responsibilities, all 449 measured functions meet the
ceiling. These are measurements of this refactor, not a function-count limit.

| Function | Before | After |
| --- | ---: | ---: |
| Graph normalization (`normalize_checked`) | 107 | 16 |
| Collection orchestration (`collect`) | 95 | 18 |
| Text view dispatch (`text_checked`) | 50 | 19 |
| Statistics presentation (`statistics_text`) | 41 | 13 |
| Command setup (`run_command`) | 36 | 17 |
| Credential supervision (`supervise`) | 36 | 20 |

`join/records.rs` owns record projection, binding checks, and known filesystem
overlap. `join/consumers.rs` owns declared pod relationships and lifecycle
counts. `collect/enrichment.rs` owns optional prerequisites and membership
planning; `collect/telemetry.rs` owns provider requests and observation
processing. `output/views.rs` and `output/statistics.rs` separate presentation
by responsibility. The parent modules retain their public entrypoints.

The review retains explicit unknown states, UID checks, union-based accounting,
staged request budgets, deterministic ordering, and common exit policy.
Kubelet and Prometheus now share non-filesystem mode classification while
retaining their distinct identity and timestamp checks.
