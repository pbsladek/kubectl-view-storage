# Local distribution preparation

The [phase plan](IMPLEMENTATION_PLAN.md) places publication in phase 6, while A30 remains a phase 5 acceptance gate. This guide covers artifact preparation and an isolated local Krew installation. It does not publish a release or submit an index entry.

Current evidence includes genuine native macOS ARM64, cross-built Intel macOS and Linux musl artifacts, with source/lock/feature and runtime scopes recorded separately. The pinned workspace-local Krew 0.5.0 tool is checksum-verified; local installation uses an owned temporary KREW_ROOT. Twelve packaging checks include real Cargo file-selection matching. Synthetic header/subprocess fixtures do not establish runtime compatibility. Current source-package installation, the complete current artifact set and all native CI jobs must pass before A30 is closed; see IMPLEMENTATION_STATUS.md for completed gate receipts.

## Build inputs

Use the checked-in stable Rust pin, bundled Cargo and the reviewed Cargo.lock. Run formatting, Clippy, tests, full production coverage, advisories and the latest-stable check before approving release inputs. Preserve build revision/toolchain/dependency evidence for each target; archive validation cannot establish source provenance or authentication behavior.

| Platform | Rust target | Native build environment |
| --- | --- | --- |
| Linux amd64 | x86_64-unknown-linux-musl | amd64 Linux with musl C compiler/linker |
| Linux arm64 | aarch64-unknown-linux-musl | arm64 Linux with musl C compiler/linker |
| macOS amd64 | x86_64-apple-darwin | Intel macOS with Apple developer command-line tools |
| macOS arm64 | aarch64-apple-darwin | Apple Silicon macOS with Apple developer command-line tools |

Build each target with `cargo build --release --locked --target TARGET`. Select the native musl linker/CC on Linux, including for the ring dependency. Do not label a host binary as another target. Target availability, libc compatibility, authentication and real runtime behavior require native execution evidence; those gates remain open.

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

Collect all four archives in one directory. Replace OWNER below with the actual GitHub repository owner, then generate and verify the manifest:

```sh
python3 scripts/package-release.py manifest \
  --repository-url https://github.com/OWNER/kubectl-view-storage \
  --directory artifacts/release
make release-verify
```

Generation requires all four archives and validates their target, entry names, modes and exact license bytes. It produces `view-storage.yaml` and `SHA256SUMS` using SHA-256 computed from the archive bytes. The manifest has one exact OS/architecture selector per target, the Cargo version with Krew's required v prefix, and corresponding GitHub release asset URLs. It uses the [repository template](../.krew.yaml). JSON syntax is a YAML subset, allowing both files to use Python's standard JSON library without a YAML dependency. Generated URLs describe the intended release location; generation does not upload assets or prove download availability.

`verify` rejects mismatched checksums, versions, platform selectors, binary names, archive layout and licenses. Keep the original build provenance and CI results alongside the verified files. The native distribution workflow now builds/tests each target, records its baseline, verifies/packages/installs the source crate, combines only actual archives and runs isolated Krew installation on each native platform. It is prepared locally; execution on the public repository remains separate evidence. Cargo source files are explicitly root-allowlisted: root-anchored patterns prevent matching nested README/LICENSE files in ignored upstream/runtime directories. The earlier inspected package had 122 files; inspect the current package list after source additions. The allowlist excludes generated credentials and build artifacts. Local --allow-dirty packaging is used only for this uncommitted checkout; compiler verification remains enabled and nothing is published.

## Actual local Krew gate

Use installed kubectl and Krew with the real native archive among the verified set. Krew's [custom installation directory](https://krew.sigs.k8s.io/docs/user-guide/advanced-configuration/#customize-installation-directory) supports the isolated KREW_ROOT used by this gate:

```sh
make release-smoke
```

The tool follows Krew's [local manifest/archive installation procedure](https://krew.sigs.k8s.io/docs/developer-guide/testing-locally/). It creates an owned temporary KREW_ROOT, removes platform override variables, uses an absent kubeconfig and puts that installation first on PATH. It verifies that Krew's canonical executable link stays inside that root, checks the installed binary hash, and invokes real `kubectl view-storage --help` and `--version`. It also runs configured direct/plugin query checks and real PTY helper authentication/cancellation against the installed binary. The version must match Cargo.toml exactly. The temporary installation is removed when the gate finishes; the user's Krew installation and kubeconfig are not changed.

`krew-smoke.json` records running, passed or failed status. A failure replaces earlier passing evidence. This gate requires actual Krew; a mocked subprocess or the earlier substituted command harness does not count. Successful local installation does not replace native authentication tests on all four platforms or a download/checksum check after publication.

The local manifest skips index initialization/update, and the gate disables Krew's background upgrade check with KREW_NO_UPGRADE_CHECK. These behaviors follow Krew's [install command](https://github.com/kubernetes-sigs/krew/blob/master/cmd/krew/cmd/install.go) and [root command](https://github.com/kubernetes-sigs/krew/blob/master/cmd/krew/cmd/root.go). The archive override supplies the local bytes; the gate does not fetch a release asset.

Run `python3 scripts/test-release-packaging.py` for the synthetic regressions. They cover reproducibility, target/header validation, static Linux requirements, missing platforms, checksum/manifest tampering, unsafe archive entries, license changes, owned-root cleanup, stale-success invalidation and real Cargo source selection with private/nested canary files. `make check` and CI run these tests; actual installation remains a separate required gate.

Use `python3 scripts/fetch-krew.py` to install the test tool under .local/tools/bin from the four official release hashes in dev/versions.json. The bootstrap preserves the upstream tool license and never changes the user's Krew installation. Pass `--krew .local/tools/bin/kubectl-krew` to the local smoke command.

`record-build-baseline.py` records actual compiler package IDs/features, production input and lock hashes, size and elapsed time. A fresh release profile is enforced when requested; cache state and cross/emulated execution remain explicit. The native workflow uses native Linux amd64/arm64 and macOS Intel/Apple Silicon runners. Local emulation or a successful cross-build supplements that matrix and is not labeled native certification.
