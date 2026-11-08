#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
fn parse(args: &[&str]) -> Result<Query, String> {
    Cli::try_parse_from(std::iter::once("kubectl-view-storage").chain(args.iter().copied()))
        .map_err(|e| e.to_string())?
        .query("demo")
}
#[test]
fn defaults_and_scope() {
    let q = parse(&[]).unwrap();
    assert_eq!(q.namespace.as_deref(), Some("demo"));
    assert!(!q.utilization);
    assert_eq!(q.group_by.len(), 3);
    let q = parse(&["-A", "--kind", "pv", "--require-metrics"]).unwrap();
    assert!(q.namespace.is_none());
    assert!(q.utilization);
    assert_eq!(q.group_by, vec![Dimension::Storageclass, Dimension::Pv]);
    let q = parse(&["-n", "other", "--storage-class", "", "-g", "node,pvc"]).unwrap();
    assert_eq!(q.filters.storage_class.as_deref(), Some(""));
    assert_eq!(q.group_by[0], Dimension::Node);
}
#[test]
fn invalid_invocations_fail_without_a_client() {
    for args in [
        vec!["--kind", "pv"],
        vec!["-A", "-n", "a"],
        vec!["-A", "--kind", "pv", "--pvc", "a"],
        vec!["--warn-used", "90", "--critical-used", "80"],
        vec!["--warn-used", "1", "--critical-used", "0.9"],
        vec!["-g", "node,node"],
        vec!["-g", ""],
        vec!["-g", "nope"],
        vec!["--sort", "name:up"],
        vec!["--sort", "bad"],
        vec!["--concurrency", "0"],
        vec!["--concurrency", "65"],
        vec!["--pvc", "a", "--pvc", "b"],
        vec!["--timeout", "0s"],
        vec!["--fail-on", "info"],
        vec!["--metrics-source", "prometheus"],
        vec!["--node", ""],
    ] {
        assert!(parse(&args).is_err(), "{args:?}");
    }
}
#[test]
fn help_version_and_duration() {
    assert_eq!(
        Cli::try_parse_from(["cmd", "--help"]).unwrap_err().kind(),
        clap::error::ErrorKind::DisplayHelp
    );
    assert_eq!(
        Cli::try_parse_from(["cmd", "--version"])
            .unwrap_err()
            .kind(),
        clap::error::ErrorKind::DisplayVersion
    );
    for (input, millis) in [("1ms", 1), ("2s", 2000), ("3m", 180000), ("1h", 3600000)] {
        assert_eq!(duration(input).unwrap().as_millis(), millis);
    }
    for input in [
        "1",
        "-1s",
        "1.5s",
        "0ms",
        "1d",
        "s",
        "18446744073709551615h",
    ] {
        assert!(duration(input).is_err());
    }
    for input in [
        "name",
        "requested:desc",
        "provisioned",
        "used",
        "available",
        "used-percent",
        "inode-percent",
        "severity",
    ] {
        assert_eq!(
            input
                .parse::<Sort>()
                .unwrap()
                .to_string()
                .parse::<Sort>()
                .unwrap(),
            input.parse::<Sort>().unwrap()
        );
    }
}
