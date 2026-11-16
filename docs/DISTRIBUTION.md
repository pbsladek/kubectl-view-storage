# Building and publishing a release

The [phase plan](IMPLEMENTATION_PLAN.md) places publication in phase 6, while A30 remains a phase 5 acceptance gate. This guide covers the Makefile release commands, artifact preparation and isolated local Krew installation. `make release-publish` explicitly publishes to GitHub; the native workflow and other preparation commands do not publish. Krew index submission remains separate.

The [Krew readiness checklist](KREW_READINESS.md) tracks upstream prerequisites and the remaining release steps. Native builds, source-package installation and isolated Krew installation passed on all four platforms at `5f372dc` in [CI run 42975306322](https://github.com/pbsladek/kubectl-view-storage/actions/runs/42975306322). The pinned workspace-local Krew 0.5.0 tool is checksum-verified; local installation uses an owned temporary KREW_ROOT. Fifteen packaging checks include real Cargo file-selection matching, installed-license failures and download-mode command selection. Synthetic header/subprocess fixtures do not establish runtime compatibility. Revalidate the chosen release revision before closing A30; see [implementation evidence](IMPLEMENTATION_STATUS.md) and the [acceptance audit](ACCEPTANCE_STATUS.md).

## Release through Make

Use Python 3.12+, the pinned Cargo toolchain, Git and an authenticated GitHub CLI
(`gh auth login`). `make release-help` prints the commands. The wrapper selects
the GitHub repository from `origin`, so Git and gh must both have access there.

For version 0.1.0:

```sh
make release-prepare VERSION=0.1.0
make verify
make coverage
```

Preparation updates Cargo.toml, the local package version in both Cargo.lock and
fuzz/Cargo.lock, and generated shell completions. It uses offline Cargo resolution
and refuses unrelated dependency changes. A failure restores those input files.
Populate Cargo's dependency caches first if offline resolution reports a missing
package. Other reviewed source changes remain yours to commit.

Review, commit and push the release inputs, then run:

```sh
make release VERSION=0.1.0
```

This requires a clean checkout with matching manifest/lockfile versions. It
creates an annotated `v0.1.0` tag at HEAD if needed, pushes it to origin, dispatches
the native Release workflow at that tag, waits for the full workflow including
four native installation checks, and downloads `verified-local-release-set`.
It finds its own dispatch by a unique workflow run name, verifies the source
commit, checks all four archives and SHA-256 values, and writes a receipt linking
the files to that run. It does not commit source or move existing tags.

The default directory is `artifacts/release-0.1.0`. An existing destination is
preserved; use a fresh `RELEASE_DIRECTORY` for another download. If the connection
or watch is interrupted, the printed run ID lets you resume without dispatching
another build:

```sh
make release VERSION=0.1.0 RUN_ID=123456789
```

After reviewing the artifacts, publish that same verified set:

```sh
make release-publish VERSION=0.1.0
```

Publication rechecks the remote tag, successful workflow attempt, archive layout,
manifest, checksums and downloaded-file hashes, then creates a GitHub Release
with all four archives, `SHA256SUMS` and `view-storage.yaml`. It generates release
notes unless `docs/releases/vVERSION.md` supplies reviewed notes, and marks
versions with a prerelease suffix as prereleases. It does not
rebuild binaries, overwrite existing release assets or submit a Krew index PR.

Use `DRAFT=1` to create an unpublished draft instead:

```sh
make release-publish VERSION=0.1.0 DRAFT=1
```

Finish publishing that draft in GitHub. Pass the same `RELEASE_DIRECTORY` override
to build, verify and publish when using a custom directory. `release-build` is an
alias for the build step. Existing `release-verify` and `release-smoke` targets
also accept VERSION to select its directory; without VERSION they retain the
original `artifacts/release` default. After publication, run the public-URL Krew
checks below before claiming download-based installation support.

The wrapper tests use synthetic archive fixtures and mocked GitHub operations;
they exercise version preparation, rollback, source/run identity, failure
handling and publication arguments without pushing tags or publishing anything.
A hosted run at the selected revision remains the native execution evidence.

## Build inputs

Use the checked-in stable Rust pin, bundled Cargo and the reviewed Cargo.lock. Run formatting, Clippy, tests, full production coverage, advisories and the latest-stable check before approving release inputs. Preserve build revision/toolchain/dependency evidence for each target; archive validation cannot establish source provenance or authentication behavior.

| Platform | Rust target | Native build environment |
| --- | --- | --- |
| Linux amd64 | x86_64-unknown-linux-musl | amd64 Linux with musl C compiler/linker |
| Linux arm64 | aarch64-unknown-linux-musl | arm64 Linux with musl C compiler/linker |
| macOS amd64 | x86_64-apple-darwin | Intel macOS with Apple developer command-line tools |
| macOS arm64 | aarch64-apple-darwin | Apple Silicon macOS with Apple developer command-line tools |

Build each target with `cargo build --release --locked --target TARGET`. Select the native musl linker/CC on Linux, including for the ring dependency. Do not label a host binary as another target. Target availability, libc compatibility, authentication and real runtime behavior require native execution evidence on the chosen release revision.

The canonical Cargo binary and archive entry is `kubectl-view_storage`. The Cargo package/project name is `kubectl-view-storage`. Krew converts the plugin name `view-storage` into the discoverable executable link; see the [Krew manifest contract](https://krew.sigs.k8s.io/docs/developer-guide/plugin-manifest/).

## Archives and manifest

For each genuine target build, run the following with its actual target substituted:

```sh
python3 scripts/package-release.py archive \
  --target aarch64-apple-darwin \
  --binary target/aarch64-apple-darwin/release/kubectl-view_storage \
  --directory artifacts/release
```

The tool reads the version from Cargo.toml. Each deterministic gzip/tar archive contains exactly the executable (mode 0755) and the project LICENSE (0644), with fixed timestamps and ownership. It refuses to overwrite existing archives. Header validation rejects wrong formats/architectures and Linux binaries with an interpreter or DT_NEEDED library dependencies. It does not prove that a binary was built from this source or implements the declared version; runtime checks supply that evidence.

Collect all four archives in one directory, then generate and verify the manifest:

```sh
python3 scripts/package-release.py manifest \
  --repository-url https://github.com/pbsladek/kubectl-view-storage \
  --directory artifacts/release
make release-verify
```

Generation requires all four archives and validates their target, entry names, modes and exact license bytes. It produces `view-storage.yaml` and `SHA256SUMS` using SHA-256 computed from the archive bytes. The manifest has one exact OS/architecture selector per target, the Cargo version with Krew's required v prefix, and corresponding GitHub release asset URLs. It uses the [repository template](../.krew.yaml). JSON syntax is a YAML subset, allowing both files to use Python's standard JSON library without a YAML dependency. Generated URLs describe the intended release location; generation does not upload assets or prove download availability.

`verify` rejects mismatched checksums, versions, platform selectors, binary names, archive layout and licenses. Keep the original build provenance and CI results alongside the verified files. The native distribution workflow builds/tests each target, records its baseline, verifies/packages/installs the source crate, combines only actual archives and runs isolated Krew installation on each native platform. Cargo source files are explicitly root-allowlisted: root-anchored patterns prevent matching nested README/LICENSE files in ignored upstream/runtime directories. Inspect `cargo package --list --locked` for the selected release revision; the allowlist excludes generated credentials and build artifacts. CI packages a committed checkout with compiler verification enabled. Nothing is published by this workflow.

## Actual local Krew gate

Use installed kubectl and Krew with the real native archive among the verified set. Krew's [custom installation directory](https://krew.sigs.k8s.io/docs/user-guide/advanced-configuration/#customize-installation-directory) supports the isolated KREW_ROOT used by this gate:

```sh
make release-smoke
```

The tool follows Krew's [local manifest/archive installation procedure](https://krew.sigs.k8s.io/docs/developer-guide/testing-locally/). It creates an owned temporary KREW_ROOT, removes platform override variables, uses an absent kubeconfig and puts that installation first on PATH. It verifies that Krew's canonical executable link stays inside that root, checks the installed binary hash and exact installed LICENSE bytes, and invokes real `kubectl view-storage --help` and `--version`. It also runs configured direct/plugin query checks and real PTY helper authentication/cancellation against the installed binary. The version must match Cargo.toml exactly. The temporary installation is removed when the gate finishes; the user's Krew installation and kubeconfig are not changed.

`krew-smoke.json` records running, passed or failed status. A failure replaces earlier passing evidence. This gate requires actual Krew; a mocked subprocess or the earlier substituted command harness does not count. Successful local installation does not replace native authentication tests on all four platforms or a download/checksum check after publication.

The local manifest skips index initialization/update, and the gate disables Krew's background upgrade check with KREW_NO_UPGRADE_CHECK. These behaviors follow Krew's [install command](https://github.com/kubernetes-sigs/krew/blob/master/cmd/krew/cmd/install.go) and [root command](https://github.com/kubernetes-sigs/krew/blob/master/cmd/krew/cmd/root.go). By default, the archive override supplies local bytes. After publication, add `--download` to `package-release.py smoke` to install from the manifest's release URI and validate its checksum through Krew; see the [release checklist](KREW_READINESS.md). This mode still requires the approved local artifact set for comparison. For v0.1.0, all six public asset hashes match the approved native run, and native macOS ARM64 passes public-URL installation, configured-query and terminal-authentication checks. The other three native public-URL installation modes remain separate from the passing four-platform pre-publication installation matrix.

Run `python3 scripts/test-release-packaging.py` for the synthetic regressions. They cover reproducibility, target/header validation, static Linux requirements, missing platforms, checksum/manifest tampering, unsafe archive entries, license changes, owned-root cleanup, stale-success invalidation and real Cargo source selection with private/nested canary files. `make check` and CI run these tests; actual installation remains a separate required gate.

Use `python3 scripts/fetch-krew.py` to install the test tool under .local/tools/bin from the four official release hashes in dev/versions.json. The bootstrap preserves the upstream tool license and never changes the user's Krew installation. Pass `--krew .local/tools/bin/kubectl-krew` to the local smoke command.

`record-build-baseline.py` records actual compiler package IDs/features, production input and lock hashes, size and elapsed time. A fresh release profile is enforced when requested; cache state and cross/emulated execution remain explicit. The native workflow uses native Linux amd64/arm64 and macOS Intel/Apple Silicon runners. Local emulation or a successful cross-build supplements that matrix and is not labeled native certification.
