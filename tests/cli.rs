//! Real executable smoke tests; help and invalid flags never require kubeconfig.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::process::Command;

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kubectl-view_storage"));
    command.env("KUBECONFIG", "/nonexistent/view-storage-kubeconfig");
    command
}
#[test]
fn help_and_version_succeed_without_configuration_or_network() {
    for flag in ["--help", "--version"] {
        let output = command().arg(flag).output().unwrap();
        assert!(output.status.success());
        assert!(!output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("--metrics-source"));
        assert!(output.stderr.is_empty());
    }
}
#[test]
fn invalid_invocations_fail_before_configuration() {
    for args in [
        vec!["--kind", "pv"],
        vec!["-A", "-n", "demo"],
        vec!["--warn-used", "90", "--critical-used", "80"],
        vec!["--pvc", "one", "--pvc", "two"],
        vec!["--concurrency", "0"],
        vec!["--metrics-source", "prometheus"],
        vec!["--units", "GB"],
        vec!["--view", "stats", "--units", "not-a-unit"],
        vec!["--compare", "/nonexistent/report.json"],
        vec!["--view", "stats", "--compare", "/nonexistent/report.json"],
        vec!["-A", "--statefulset", "database"],
        vec!["--statefulset", ""],
    ] {
        let output = command().args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("Cannot read kubeconfig"));
    }
}
