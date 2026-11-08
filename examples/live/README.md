# Captured live output

These examples were captured from the compiled plugin through actual `kubectl view-storage` invocations against disposable kind clusters on macOS ARM64. They use real Kubernetes API responses and real kubelet/CSI observations, rather than mocked or handwritten reports.

- [Inventory](inventory/README.md): default tree, table, summary, issues, wide mount declarations, node/pod grouping, PVs, JSON, CSV, and a strict check with missing metrics.
- [CSI usage](csi/README.md): measured filesystem and inode counters, all four text views, JSON, CSV, and a successful strict check.

The clusters use Kubernetes 1.37.0, kind 0.33.0, and kubectl 1.37.2. The CSI fixture uses the reviewed hostpath 1.19.0 driver and snapshot-controller 8.7.0 bundle in [dev/csi](../../dev/csi/README.md). Both fixtures are owned by the development harness and were deleted after capture.

## Reading the captures

The Markdown command/output blocks copy captured stdout without changing values, names, UIDs, timestamps, whitespace, or diagnostics. JSON projections explicitly show the jq filter; complete original JSON is retained alongside them. Each profile's `capture.json` records commands, exit codes, timestamps, source revision, binary hash, and stdout hashes. Captures use plain text with `NO_COLOR=1`.

Each command performs its own collection. Values and timestamps may differ between formats, and will change on a fresh cluster. This hostpath demo reports the backing filesystem's counters; those are distinct from requested/provisioned PVC capacity and do not establish physical backend totals or per-claim isolation.

## Reproduce

Build the plugin using the [source instructions](../../README.md#build-from-source), put it on PATH, and follow the selected profile's setup and commands. Run the inventory and CSI clusters serially and delete each owned profile after use. The harness uses explicit private kubeconfigs; no credentials or raw node response bodies are included here.

The captured revision is the implementation used for the examples, not a promise that a later release has identical text layout. The [specification](../../docs/SPEC.md) defines behavior, and [compatibility evidence](../../docs/COMPATIBILITY.md) records separately tested capabilities.
