# Runtime dependency review

Reviewed 2026-11-07 against the actual Cargo.lock. Retain thirteen direct runtime packages with the explicit features in Cargo.toml. Official crates.io release/owner metadata and public upstream repository metadata were collected for every package; all selected versions are published, unyanked stable releases and all repositories were active and unarchived. The selected versions match the observed latest stable releases. This is a maintenance/selection review, not a source security audit or a guarantee of future maintainer response.

| Package | Selected version | Upstream ownership/activity reviewed | Retention decision |
| --- | --- | --- | --- |
| [clap](https://github.com/clap-rs/clap) | 4.6.7 | clap/Rust CLI teams; recent parser fixes and issue/PR closures | Typed CLI and built-in help; selected capabilities only |
| [kube](https://github.com/kube-rs/kube) | 5.0.0 | kube-rs release team; recent client fixes and documented security policy | Delegate kubeconfig, TLS, authentication and proxy behavior; omit controller runtime/derives |
| [k8s-openapi](https://github.com/Arnavion/k8s-openapi) | 0.29.0 | Arnavion; current Kubernetes release/support activity | Compatible API types; select the oldest supported Kubernetes schema feature |
| [tokio](https://github.com/tokio-rs/tokio) | 1.54.1 | Tokio core team; active fixes and documented security policy | One bounded async runtime; omit `full` |
| [tokio-util](https://github.com/tokio-rs/tokio) | 0.7.20 | Same maintained project and release ownership | Shared cancellation tokens |
| [serde](https://github.com/serde-rs/serde) | 1.0.229 | dtolnay/serde-rs publishing team; recent issue and derive maintenance | Typed boundary data |
| [serde_json](https://github.com/serde-rs/json) | 1.0.154 | Same publishing team; numeric/parser fixes and recent closures | Exact JSON number handling and stable report serialization |
| [thiserror](https://github.com/dtolnay/thiserror) | 2.0.21 | dtolnay; maintained error derives and recent fixes/documentation | Typed errors without another error framework |
| [jiff](https://github.com/BurntSushi/jiff) | 0.2.40 | BurntSushi; release and datetime maintenance on review date | UTC parsing/formatting with `std`; no timezone database |
| [http](https://github.com/hyperium/http) | 1.5.0 | Hyper maintainers; current header fixes and reviewed closures | Request/header types shared with kube |
| [http-body-util](https://github.com/hyperium/http-body) | 0.1.5 | Hyper maintainers; maintained framing/docs and release response | Stream bounded body frames |
| [csv](https://github.com/BurntSushi/rust-csv) | 1.4.0 | BurntSushi; recent parser/performance maintenance | Correct CSV encoding; mature release cadence is acceptable |
| [nix](https://github.com/nix-rust/nix) | 0.31.3 | nix-rust maintainers; current libc compatibility work and reviewed issue/PR closures | One safe Unix API for descriptor flags, owned processes/signals, terminal handoff and shared monotonic deadlines; macOS-only event observation |

The review included recent public issue/PR closures for the twelve distinct repositories and checked repository security-policy locations. A missing repository-local policy is recorded as absent at the checked paths, not as proof of abandonment or lack of an organization-wide reporting route. No maintainers were contacted. Public activity cannot establish response-time guarantees.

Jiff 0.2.40 was published during this work. Its bundled changelog documents timestamp conversion improvements and removal of disabled `defmt` dependencies from the default lock graph. Update both the production and separate fuzz locks. The production graph has 216 resolved package versions, down from the initial 219. The direct runtime budget stays at thirteen. Locked tests, Clippy, full-source coverage and cargo-deny checks pass after the update; parser fuzz smoke is also recorded against its separately locked toolchain/graph. Exact compiled-package features and binary cost are measured separately per target; a lockfile count is not a count of shipped packages.

Resolved advisories, unmaintained/yanked checks, licenses, bans and source checks pass with cargo-deny 0.19.0. Keep `unmaintained = "all"`; no advisory ignore was added. The reviewed `ar_archive_writer` 0.5.3 build helper needs a version-scoped `Apache-2.0 WITH LLVM-exception` license approval. Duplicate base64/syn/windows-sys versions remain warnings; they belong to reviewed SDK/build/dev paths and should be reconsidered on updates. This approval does not authorize additional crates or broader feature defaults.

Ignored evidence under `artifacts/dependency-baseline` includes public owner/release/repository metadata, recent closure/policy metadata, current lock hashes, and per-target compiler unit/feature records. Preserve the initial measurements as historical checkpoints. Recheck latest stable Rust/bundled Cargo, upstream status, advisories, unified features and build cost before release and on dependency updates, following [the development policy](DEVELOPMENT.md).

## Unix supervision choice

Nix 0.31.3 replaces the earlier Rustix selection. This retains one Unix syscall crate and the thirteen-runtime-package budget. The required safe APIs cover descriptor restoration, process groups/reaping, thread-local signal masks during terminal handoff, and cross-process monotonic budget transfer. Linux uses non-reaping wait observation and subreaper ownership; macOS uses kqueue because the reviewed Nix release does not expose waitid there. macOS event support is target-scoped. Enabled base features are fs/process/signal/term/time; defaults, aio, networking and broad API bundles stay disabled. The disabled aio dependency path does not add pin-utils to the resolved graph.

The change addresses measured failures: a synchronous SDK helper exceeded a 25 ms configuration budget, SDK refresh jobs could hold runtime shutdown open, and plain thread/future cancellation could not terminate those subprocesses. Current executable tests prove bounded timeout/Ctrl-C cleanup, no surviving owned processes/private startup state, inherited descriptor restoration and retained partial reports. Real PTY checks exercise Always/IfAvailable input, foreground handoff/restoration and a signal-ignoring helper. These are process-lifetime controls; kube retains credential parsing, TLS and authentication.

The kube 5 default retry layer is explicitly disabled. The collector owns retry/backoff, budgets and consumed-byte accounting, and actual HTTP tests verify 429/503/504 attempts against wire requests. Otherwise SDK retries could occur underneath one reported adapter attempt. Current per-target size/features and the earlier cold-build checkpoints remain separately recorded; warm source-correction timings are not presented as cold builds.
