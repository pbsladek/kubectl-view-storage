# Krew readiness

Checked 2026-11-11 against Krew's [new-plugin checklist](https://krew.sigs.k8s.io/docs/developer-guide/release/new-plugin/), [naming guide](https://krew.sigs.k8s.io/docs/developer-guide/develop/naming-guide/), [best practices](https://krew.sigs.k8s.io/docs/developer-guide/develop/best-practices/) and [manifest contract](https://krew.sigs.k8s.io/docs/developer-guide/plugin-manifest/).

The repository is prepared to build and validate a Krew release. It is not yet submission-ready: there is no published version, tag or downloadable release asset set. Publication and an index pull request are separate future actions. The native CI workflow does not publish. The explicit `make release-publish` wrapper can publish its verified artifacts; no command opens a Krew index PR.

## Checklist

| Requirement | Status and evidence |
| --- | --- |
| Appropriate, available name | `view-storage` follows the lowercase, hyphenated verb/resource naming convention. No `plugins/view-storage.yaml` entry exists in the official index as of this audit; recheck before submission. This does not reserve the name. |
| Public source and open source license | [Public repository](https://github.com/pbsladek/kubectl-view-storage), Apache-2.0 [LICENSE](../LICENSE). |
| kubectl integration | Canonical binary `kubectl-view_storage`; real discovery/forwarding checks cover help, version, namespace, all namespaces, context and kubeconfig. CLI options go after `kubectl view-storage`. |
| License included and installed | Every archive contains the executable and exact project LICENSE. Krew's default file selection extracts both. The smoke gate verifies the installed license bytes as well as the executable hash. |
| Platform packages | Linux and macOS, each amd64 and arm64. Native builds, source-package installation and real isolated Krew installation passed at `5f372dc` in [CI run 42975306322](https://github.com/pbsladek/kubectl-view-storage/actions/runs/42975306322). Windows is outside the supported set. |
| Manifest and checksums | [`.krew.yaml`](../.krew.yaml) is a template, not an installable manifest. The packaging tool generates `view-storage.yaml` and `SHA256SUMS` from all four actual archives. It checks selectors, architecture, executable layout, version, license and checksums. |
| Tagged semantic version | Pending. Cargo and both lockfiles select `0.1.0`; tag `v0.1.0` at the exact validated release commit. |
| Public downloads and installation from URLs | Pending publication. The manifest points to versioned GitHub release assets. The `smoke --download` gate is prepared; its success must be established after those assets exist. Local archive installation alone does not prove URL availability. |
| Final acceptance | Finish remaining release acceptance items in [the audit](ACCEPTANCE_STATUS.md) and [implementation plan](IMPLEMENTATION_PLAN.md). Packaging success does not close product acceptance. |
| Krew index submission | Deferred. No index PR has been created. Kubernetes CLA acceptance is an upstream contribution prerequisite when submission is eventually requested. |

During this audit, archive verification and actual isolated Krew installation passed again on macOS arm64 using the downloaded `verified-local-release-set` from run 42975306322. The new installed-license check, seven discovery/configured-query cases and three terminal-authentication cases passed. All fifteen packaging regressions passed. Download-mode command selection is regression-tested; public-URL installation remains pending publication.

## Prepare a candidate without publishing

1. Run `make release-prepare VERSION=0.1.0` (substitute the selected version). It refreshes the manifest, both lockfiles and completions. Review and commit the release inputs. Rerun the locked tests, coverage, advisories and latest Rust/Cargo checks. Close the release acceptance gates for that revision.
2. Run `make release VERSION=0.1.0` from that clean committed revision. It creates/pushes the matching tag, runs native CI, waits for success, downloads the approved artifact set and verifies it. The reusable [Release workflow](../.github/workflows/native-distribution.yml) builds all four native archives, verifies and installs the Cargo source package, generates the full manifest, and tests actual Krew installations on all four native platforms. It can also be dispatched manually. A cold build is optional for fresh build provenance, not required for every edit.
3. The wrapper saves `verified-local-release-set` under `artifacts/release-0.1.0` with a run/file receipt. For a manually dispatched workflow, download its artifact yourself. Keep `view-storage.yaml`, `SHA256SUMS` and all four archives together. Artifact retention is seven days; preserve the approved set before it expires. Keep the run URL and build/installation evidence with the candidate.
4. Run the preparation checks below. The directory must hold archives for the current Cargo version. This performs no publication or index submission.

```sh
python3 scripts/package-release.py verify --directory artifacts/release-0.1.0
python3 scripts/fetch-krew.py
python3 scripts/package-release.py smoke \
  --directory artifacts/release-0.1.0 \
  --krew .local/tools/bin/kubectl-krew
```

No hand-edited checksums or placeholder URLs should enter the final manifest. If generating it outside CI, use the actual repository:

```sh
python3 scripts/package-release.py manifest \
  --repository-url https://github.com/pbsladek/kubectl-view-storage \
  --directory artifacts/release-0.1.0
```

## Remaining steps when publication is requested

Run `make release-publish VERSION=0.1.0` against the wrapper-downloaded set, or use `DRAFT=1` for review in GitHub. It creates a GitHub Release for the matching semantic version tag with the approved four archives, `SHA256SUMS` and generated `view-storage.yaml` as GitHub release assets. See [Makefile release commands](DISTRIBUTION.md#release-through-make) for prerequisites, resume and custom directories. Reuse the validated bytes; rebuilding requires validating the replacement artifacts and regenerating checksums. Release notes should link tested compatibility and explain usage permissions and missing telemetry. Update the README's release status and installation instructions once publication is real.

From the same tagged checkout and approved artifact set, test installation using the manifest's public URLs:

```sh
python3 scripts/package-release.py smoke \
  --directory artifacts/release-0.1.0 \
  --krew .local/tools/bin/kubectl-krew \
  --download
```

This removes the local archive override; Krew must download the archive at its manifest URI and validate the checksum. The gate still compares the installed bytes with the approved local archive, verifies the installed LICENSE, and runs discovery, configured-query and terminal-authentication checks. It uses a temporary KREW_ROOT and removes it afterward. Run this on each of the four native platforms before claiming published installation support; platform overrides cannot establish native runtime behavior. The receipt records `installation_source: manifest-uri` only after success.

Only after publication and download checks, and when explicitly requested, copy the generated **`view-storage.yaml`** to `plugins/view-storage.yaml` in an index PR. Do not submit the root template. Recheck name availability and upstream contribution requirements then. Installation through the default Krew index becomes available after that PR is merged; a GitHub release alone does not register the plugin.
