//! Human-readable provenance, independent of kubeconfig and network access.
pub fn text() -> String {
    format!(
        "kubectl-view-storage {}\nRevision: {}\nCompiler: {}\nReport schema: {}\n",
        env!("CARGO_PKG_VERSION"),
        env!("VIEW_STORAGE_REVISION"),
        env!("VIEW_STORAGE_COMPILER"),
        crate::report::SCHEMA_VERSION
    )
}
