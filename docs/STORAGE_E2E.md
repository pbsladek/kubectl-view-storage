# Disposable storage tests

Local development and `.github/workflows/compatibility.yml` use the same Python
standard-library harness and real plugin binary. Each profile creates its own
kind cluster and kubeconfig, verifies behavior, and deletes only the cluster
owned by that run. Profiles run serially to fit a small Docker VM; CI jobs run
in parallel across native architectures and Kubernetes versions.

| Backend | Fixture | Required assertions |
| --- | --- | --- |
| Local-path | kind provisioner, 128Mi RWO claim, two consumers on one node | Dynamic binding and provider identity; 16MiB shared write and SHA-256; persistence after replacing the writer; deduplicated claim capacity; unknown counters remain null and strict metrics fail when unavailable. Includes the existing inventory and restricted-RBAC cases. |
| CSI hostpath | Reviewed upstream driver, 1Gi RWO Filesystem claim | Two mounted consumers; fresh attributable byte tuples before/after a bounded fsynced write; shared content; request budgets and logical capacity. Inode availability is recorded independently. |
| NFS CSI | Reviewed upstream driver, containerized userspace Ganesha server, 1Gi RWX claim | Required anti-affinity places consumers on two worker nodes; cross-node size and SHA-256; fresh byte and inode candidates from both nodes; counters change after writing; one logical capacity and no claimed physical total. |

These are filesystem tests. They do not certify raw Block, cloud disks, snapshots,
resize, quotas, server failover, or production performance. A PVC's requested
capacity does not establish a filesystem quota: local-path and NFS may report
the larger backing filesystem.

## Run locally

Install the tool versions in `dev/versions.json`, start Docker, and build once:

```sh
cargo build --locked
python3 scripts/e2e-storage.py
```

Use a release binary or select individual backends:

```sh
python3 scripts/e2e-storage.py --binary target/release/kubectl-view_storage
python3 scripts/e2e-storage.py --profiles nfs-csi --binary target/debug/kubectl-view_storage
```

With a prebuilt executable, cluster execution requires Docker, kind, Python and
the matching kubectl; it does not install or compile Rust. Without `--binary`,
the default executable is `target/debug/kubectl-view_storage`. Standalone
`dev-cluster.py preflight` still checks the pinned development Rust toolchain.

Older Kubernetes versions use the reviewed `--node-image` digest and the exact
matching client on `PATH`, as documented in [development](DEVELOPMENT.md).
Normal user kubeconfig files are fingerprinted before and after the suite.
Existing profiles are refused and preserved. To keep a cluster for debugging,
use the profile entrypoints instead of the automatically cleaning suite:

```sh
python3 scripts/dev-cluster.py create nfs --binary target/debug/kubectl-view_storage
python3 scripts/dev-cluster.py verify nfs --binary target/debug/kubectl-view_storage
python3 scripts/dev-cluster.py delete nfs
```

The equivalent profile names are `inventory` (including local-path data tests),
`csi`, and `nfs`. These commands always select the profile's private kubeconfig.

## GitHub Actions

The compatibility workflow builds and tests the binary once per native Linux
architecture (`ubuntu-24.04` and `ubuntu-24.04-arm`). Live jobs verify the artifact's
revision, architecture, SHA-256 and regular tar member before executing it.
Each live job runs all three backends. PRs run the baseline Kubernetes version
on both architectures; weekly and manual runs cover all three reviewed versions
on both architectures. The clusters are disposable, and no cloud storage
credentials or external NFS service are required.

Each profile records sanitized plugin reports and a validation receipt under
`artifacts/<run-id>/`. The suite receipt under `artifacts/storage-e2e-<suite-id>/`
links those runs, records durations, binary identity and cleanup outcomes, and
fails if any required gate, cleanup, or kubeconfig-preservation check fails.
Timeouts and missing metrics cannot become successful skips. CI uploads this
evidence on failure as well as success; kubeconfigs and bearer tokens stay in
the private `.local` tree and are not uploaded.

## NFS fixture decisions

`dev/nfs/lock.json` pins the CSI release source, vendored JSON and license hashes,
and immutable image indexes with amd64/arm64 platform digests. The runtime
harness validates this lock without downloading manifests or requiring YAML
packages. The server runs userspace Ganesha plus its bundled RPC services;
it needs no kernel NFS server module. Only fixture containers receive the
privileges required by the driver/server; workload containers run without root
or service-account tokens.

The driver uses the released v4.13.6 manifests with snapshotter/resizer containers
omitted, provisioner operation timeout bounded to 60 seconds, and image tags
replaced with reviewed digests. The original RBAC is retained. Ganesha exports
a disk-backed `emptyDir` for the lifetime of its server pod, with a 128Mi
kubelet size limit. Device-based filesystem identifiers identify its bind
mount separately from the container overlay. The test writes only 16MiB; the limit does not
create an isolated 128Mi filesystem or quota. Ganesha excludes tmpfs exports. The server image is the maintained Piraeus CSI NFS server v1.13.0, containing
Ganesha 9.14 on Debian. Only its Ganesha binary and RPC services run here; no
LINSTOR, DRBD reactor or systemd service starts. The source recipe is preserved
as a reference, not represented as a verified image build attestation.

The older SIG image contains Ganesha 6.5, which returns an incorrect response to
`GET_DIR_DELEGATION` on newer Linux clients. The local test reproduced the
resulting directory metadata and CSI-stat failures. The maintained image fixes
[that upstream bug](https://github.com/nfs-ganesha/nfs-ganesha/issues/1385),
allowing NFSv4.1 tests without global host module changes or a protocol downgrade.

This driver release hardcodes a ten-minute volume-statistics cache and exposes
no CLI override. To avoid adding ten minutes to every CI cell, the NFS gate
restarts its owned node driver after clearing the probe and again after writing,
then requires fresh kubelet observations from both nodes. Existing mounts must
remain usable. The disposable fixture uses `soft,timeo=50,retrans=2` mounts so a
failed server cannot hang runner teardown indefinitely; content hashes and
fsynced writes still have to pass. These are test-specific mount settings.
Cleanup removes clients while the server is available before deleting kind nodes. This proves write visibility, metrics after driver recovery and
deduplication; it does **not** measure normal cache-refresh latency. In general,
an observation timestamp is the kubelet sample time and cannot reveal every
driver's internal cache age.

Review updates explicitly, outside CI. Check out the exact release source in
`.local/upstream/csi-driver-nfs`, update `dev/nfs/review-bundle.py` after reviewing
upstream changes, then run `uv run dev/nfs/review-bundle.py` with Docker access.
Review the resulting manifests, transforms, image platforms and lock diff.
Do not resolve mutable tags inside a test run.

Offline gate regressions run with `python3 scripts/test-storage-e2e.py` and cover
lock tampering, architecture omissions, provenance, stale/missing node samples,
shared-capacity inflation, strict missing-metrics behavior, timeouts and cleanup
ownership. Live passing receipts remain required evidence for backend support.

## Recorded local results

Docker Desktop ARM64 supplied four CPUs and 3.825 GiB. All passing runs deleted
owned clusters and preserved normal kubeconfig fingerprints.

| Kubernetes | Scope | Local-path / CSI hostpath / NFS CSI | Suite receipt |
| --- | --- | --- | --- |
| 1.37.0 | Separate passing profiles across two attempts | 88.90s / 171.01s / 233.86s | See profile IDs in [compatibility](COMPATIBILITY.md) |
| 1.36.4 | Complete default suite, 452.27s | 80.67s / 141.82s / 229.78s | `storage-e2e-3ad4eb12-37fd-42ed-9e8d-09b6b46879da` |
| 1.35.8 | Complete default suite, 472.72s | 93.99s / 152.22s / 226.51s | `storage-e2e-9a13eeb5-4ae5-42eb-829d-5b887d647516` |

The first 1.37.0 attempt failed with the older Ganesha fixture; the replacement
server passed its real cross-node gate. These are local timings and evidence,
not GitHub runner measurements or native-host certification. The production
source fingerprint matches the tested captured release binary; no Rust runtime
source or dependency changed. GitHub-native evidence remains pending publication.
The offline harness regressions also pass under Python 3.14, matching CI.
