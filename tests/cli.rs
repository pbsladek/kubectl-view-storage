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
        if flag == "--help" {
            assert!(String::from_utf8_lossy(&output.stdout).contains("--metrics-source"));
        }
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn build_information_and_completions_are_available_without_configuration() {
    let output = command().arg("--build-info").output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with(concat!(
        "kubectl-view-storage ",
        env!("CARGO_PKG_VERSION"),
        "\n"
    )));
    assert!(text.contains("Revision: ") && text.contains("Compiler: rustc "));
    assert!(text.contains(kubectl_view_storage::report::SCHEMA_VERSION));
    for (shell, expected) in [
        (
            "bash",
            include_str!("../completions/kubectl-view_storage.bash"),
        ),
        ("zsh", include_str!("../completions/_kubectl-view_storage")),
        (
            "fish",
            include_str!("../completions/kubectl-view_storage.fish"),
        ),
    ] {
        let output = command().args(["--completions", shell]).output().unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert_eq!(output.stdout, expected.as_bytes());
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
        vec!["--units", "not-a-unit"],
        vec!["--view", "stats", "--units", "not-a-unit"],
        vec!["--compare", "/nonexistent/report.json"],
        vec!["--view", "stats", "--compare", "/nonexistent/report.json"],
        vec!["-A", "--statefulset", "database"],
        vec!["--statefulset", ""],
        vec!["--watch", "1s", "-o", "csv"],
        vec!["--watch-count", "2"],
        vec!["--watch", "0s"],
    ] {
        let output = command().args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("Cannot read kubeconfig"));
    }
}
