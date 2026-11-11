# CI performance and runner usage

Audit date: 2026-11-08 UTC. This audit uses completed GitHub Actions jobs, compiler/test logs, and the actual workflow graph. [Machine-readable baseline](ci-baseline.json) retains run URLs, revisions, step durations, queue time, and conclusions. The first hosted storage/build-reuse run has passed; controlled cold/warm measurements remain required before claiming a cache speedup.

## Measured baseline

Two complete successful source revisions had no Rust dependency or tool caches:

| Workflow | f799f81 | 13836d7 |
| --- | ---: | ---: |
| Rust and contracts | 15.07 runner-min | 14.07 runner-min |
| Four-target native distribution and installed checks | 29.70 runner-min | 36.72 runner-min |
| Dependency advisories | 3.77 runner-min | 3.78 runner-min |
| **Total** | **48.53 runner-min** | **54.57 runner-min** |
| Approximate total after rounding each job up | 57 job-min | 62 job-min |
| Distribution workflow elapsed time | 11m56s | 16m36s |

These are sums of job wall time, including parallel jobs, not CPU time or a billing statement. Dependency wait is not runner queue time. Across the native workflows, actual job-created → job-started delays totaled only 21–27 seconds: lack of job parallelism was not the principal bottleneck.

The repository is public. [GitHub's billing documentation](https://docs.github.com/en/actions/concepts/billing-and-usage) says standard hosted runners are free for public repositories. This work reduces feedback latency, compute demand, cache/artifact churn, and exposure to future private-repository costs. Larger paid runners and private-repository pricing need separate accounting; no dollar saving is inferred from these runs.

The later 0695641 revision also passed all native targets and installations. Across the three workflows it used 50.05 runner-minutes; its step evidence is included in the same baseline file.

## Where the time goes

In revision 0695641's ordinary CI logs, the 171 tests execute in about 18–23 seconds on the usual pinned/Linux jobs. Clippy plus test compilation takes 82–103 seconds on the pinned/Linux jobs; a slower stable Mac job took about 176 seconds. The coverage tests execute in about 21 seconds after about 62 seconds of compilation. Installing cargo-llvm-cov costs another 65–74 seconds on each cold run.

The older 13836d7 Intel macOS distribution job is more expensive:

| Operation | Measured time |
| --- | ---: |
| Compile test executables | 5m37s |
| Execute library/property/CLI/HTTP tests | about 39s |
| Compile release binary | 4m53s |
| Cargo package compiler verification | 1m35s |
| Actual source-package release installation | 2m13s |

The advisory job spends about 220–223 seconds installing cargo-deny and checking the graph. Most of that step is tool compilation. Rust tests already use libtest's parallel execution. Splitting this small test suite across more jobs would duplicate compiler work; it is a poor first optimization.

Nine full pinned test executions previously occurred for the same source change when pinned and stable were the same release: four ordinary matrix cells, four distribution targets, and one coverage run. The compatibility workflow separately recompiled and reran the identical cluster-free suite for every architecture × Kubernetes-version cell.

## Implemented workflow changes

1. **Cache compiled dependencies, not successful test outcomes.** A pinned Swatinem/rust-cache action separates host lint, instrumentation, isolated faults, nightly fuzzing, native targets, and GNU compatibility builds. Its compiler/environment/manifest/lock keys prevent mixing targets or incompatible profiles. Workspace artifacts are not retained as an alternative to compiling the changed project. Cache writes for dependency builds come from main; pull requests restore compatible data. Cargo still builds and executes every selected check.
2. **Cache exact-version CI tools.** cargo-deny, cargo-llvm-cov, and cargo-fuzz use an OS/architecture/tool-version/toolchain-keyed binary cache. A cache miss installs with Cargo's locked graph. Every hit still verifies the tool version. The advisory database is not cached as a substitute for fetching current advisories.
3. **Run each compiler for a reason.** Pinned formatting/Clippy runs on Linux and macOS. Coverage executes the full GNU pinned suite, including independent produced-report checks. The four native distribution targets retain all their tests, real helpers, source compiler verification, actual Cargo installation, archive checks and installed Krew behavior. Latest-stable jobs resolve the current compiler on both OSes, and run their full compatibility suite when it differs from the tested pin. When the releases match, repeating that same compiler's tests is unnecessary. This reduces full pinned-suite executions from nine to five without removing the native matrix or coverage suite.
4. **Separate independent work.** Lint, current-stable resolution, coverage, controlled faults, and the native matrix start after the short contract/policy job. Fault detection no longer extends the ordinary lint/test job's critical path. The existing four-case mutation assertions remain required. Pull-request matrices cancel remaining cells after a failure; main/manual runs retain full platform failure evidence. Cargo uses each native runner's normal CPU parallelism; the CI-only two-job build cap is removed. The resource-limited local Docker harness keeps its own limits.
5. **Select work from validated history.** Main runs once for a push to main; feature branches use pull-request checks, avoiding push+PR duplication. Pure Markdown/live-output updates run the contract/policy job and final required gate, without compiling the project. Compiled contract examples, schema, Rust sources, compiler settings, smoke utilities and unknown paths conservatively trigger full checks. On main, comparison starts at the latest successful ancestor of the CI workflow, so a docs-only follow-up cannot skip code from an earlier failed/cancelled push. Missing history/API information selects a full run. Pull requests compare against their merge base.
6. **Cancel obsolete work.** Per-workflow/ref concurrency replaces old runs with the newest revision. The cumulative change selector keeps this compatible with documentation shortcuts. The final `required` job fails on failed/cancelled prerequisites or accidentally skipped required Rust jobs, and accepts only intentional documentation skips. Use this stable aggregate status for branch protection if protection is enabled.
7. **Build compatibility binaries once per architecture.** Two native GNU build/test jobs produce the binaries; all six version/architecture cluster jobs reuse their matching binary. Each consumer verifies the exact revision, host architecture, SHA-256 and regular archive member before execution. All three Kubernetes versions, both architectures, inventory/RBAC cases, CSI freshness/write gates, and cleanup remain. Inventory/CSI profiles stay serial within each runner. Full compatibility also runs weekly and can be dispatched explicitly.
8. **Keep cached and cold evidence distinct.** Normal distribution builds may restore dependencies, and their receipts record actual compiler units/cache conditions. Manual `cold_build=true` disables dependency restoration and requires a fresh release profile. Neither path uses `cargo package --no-verify` or substitutes mock installation for real installation.
9. **Reduce storage and update churn.** Release/report/coverage artifacts retain seven days, temporary compatibility binaries three. Duplicate release archives are omitted from validation-only uploads. Cargo patch updates and action minor/patch updates are grouped; Rust minor/major updates remain separate, since a 0.x minor bump may change APIs. Python validator wheels use a pinned-requirements cache.

The dependency and tool actions are pinned to reviewed immutable release commits. No Rust runtime dependency, production optimization profile, threshold logic, parser limit, existing Rust/property test-case count, or coverage floor changed for this work. New CI protocol regressions were added.

## Measure the new runs

```sh
python3 scripts/ci-timings.py RUN_ID... --output artifacts/ci-timings/measurement.json
```

Use complete runs of the same source revision for cold/warm comparison. The first new main run populates caches. A manual `ci.yml` dispatch at the same revision deliberately runs the full Rust/native checks and can measure restored caches. A direct `native-distribution.yml` dispatch supports `cold_build=true` for a separate cold-build experiment. The latter is explicit extra work, not part of every push.

The script sums completed job time, reports per-job rounded-minute estimates, records real queue time separately, and retains step-level evidence. In-progress runs are labeled incomplete. Preserve the run URL, revision, success status, cache-hit log, compiler/tool versions, and build receipts. Do not publish a warm-cache speedup based on a cold first run, estimate a private bill from public runner time, or confuse reduced wall time with reduced total runner consumption.

The structural savings are known: four duplicate compiler-suite executions disappear, dependency/tool caches are introduced, six compatibility compiles become two, and documentation does not trigger native distribution. Actual warm-cache time and cache-transfer cost still need measurement. Intel source compilation and release linking remain work even after dependency hits.

The maintenance audit also found that Rust channel metadata names Cargo by its crate version, which differs from the bundled executable release. The latest-stable check now verifies the binary release and official Cargo commit; its previous version comparison produced a false failure.

## Storage compatibility scheduling

All three storage backends share one local/CI suite. PR compatibility uses only the baseline Kubernetes version on both Linux architectures. Scheduled/manual runs use all three reviewed versions on both architectures. Each cell reuses its architecture build and creates/deletes backend clusters serially, bounding Docker memory. The NFS driver has an upstream ten-minute stats cache; the fixture exercises controlled node-driver restarts to refresh counters rather than sleeping through that cache period. See [storage E2E](STORAGE_E2E.md) for the exact limits of that assertion.

## Further experiments, not defaults

- Benchmark release `codegen-units=1` against a higher count with thin LTO, recording clean/warm build time, final binary size, and representative query/normalization/rendering performance. Do not change the release profile solely because one compile is faster.
- Consider sccache only if workspace code generation is still dominant after the new dependency caches. It adds storage/key/tool complexity and does not eliminate linking or all source-install recompilation.
- Consider nextest or test sharding if test execution, rather than compilation, becomes the critical path. Retain process/PTY and cancellation ownership behavior when changing the test runner.
- Inspect cache restore/save sizes and hit rates after several revisions. A cache whose transfer time exceeds the compilation saved should be reduced or removed.
- Keep Docker image reuse separate from Rust caching. Uploading entire kind/CSI image sets to Actions artifacts may cost more transfer/storage than the pulls it replaces; measure it before adopting it.


## Local validation of the workflow refactor

Official actionlint 1.7.12 (checksum-verified workspace-local binary) accepts the workflows. Six change-selection tests cover unknown paths, missing history, ancestry, and cancelled-source/docs follow-ups. Two timing tests separate parallel runner time, job rounding and real queue time. Five protocol tests exercise Cargo external-subcommand version invocation, matching-client outputs, pinned-nightly environment lines, the final required gate, and successful/stale/wrong-architecture/corrupt/link compatibility artifacts using the actual inline workflow code. Existing dependency, inventory/CSI harness, packaging, loopback and independent contract checks pass. Hosted cache/reuse measurements remain separate evidence.

```sh
python3 scripts/test-ci-scope.py
python3 scripts/test-ci-timings.py
uv run --with PyYAML==6.0.3 python scripts/test-ci-workflows.py
```

## References

- [GitHub dependency caching](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching): keys, cache scope, and restore behavior.
- [Rust cache implementation and inputs](https://github.com/Swatinem/rust-cache/tree/v2.9.3): compiler/lock/environment keys, dependency pruning, and workspace behavior.
- [GitHub workflow concurrency](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/control-workflow-concurrency): replacement of obsolete runs.
- [Cargo build configuration](https://doc.rust-lang.org/cargo/reference/config.html): compiler parallelism and incremental settings.

## First hosted storage verification

[Run 42900675127](https://github.com/pbsladek/kubectl-view-storage/actions/runs/42900675127) passed two native architecture builds and all six live cells, with all three backends in every cell. [The receipt index](storage-e2e-hosted.json) verifies revision/architecture/binary reuse, eighteen backend validations and cleanup. Backend execution per cell took 469.23–494.36 seconds; that is not the whole workflow duration.

The initial ordinary CI coverage job exposed a Cargo external-command invocation bug in the new tool wrapper: `cargo-llvm-cov` requires `llvm-cov` as its first argument. The wrapper now invokes the exact cached binary with the Cargo subcommand token, and a regression executes the actual action shell with correct/wrong-version fixtures for all three tools, including paths with spaces. Actual workspace-local tool binaries also pass that invocation. This fix changes no Rust source, dependency, coverage floor or test selection.
