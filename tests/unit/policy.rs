#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use clap::Parser;
use std::collections::BTreeMap;
fn run(input: &str, args: &[&str], complete: bool) -> Exit {
    let input: serde_json::Value = serde_json::from_str(input).unwrap();
    let query = crate::cli::Cli::try_parse_from(std::iter::once("cmd").chain(args.iter().copied()))
        .unwrap()
        .query("demo")
        .unwrap();
    let mut graph = crate::join::normalize(
        &serde_json::from_value(input["kubernetes"].clone()).unwrap(),
        &query,
        true,
    )
    .unwrap();
    crate::metrics::apply(
        &mut graph,
        &serde_json::from_value(input["summary_responses"].clone()).unwrap(),
        &BTreeMap::new(),
        &query,
        input["clock"].as_str().unwrap().parse().unwrap(),
    );
    decide(
        complete,
        &graph,
        &findings::evaluate(
            &graph,
            &query,
            input["clock"].as_str().unwrap().parse().unwrap(),
        ),
        &query,
    )
}
#[test]
fn incomplete_selection_precedes_findings_and_empty_can_succeed() {
    let healthy = include_str!("../../examples/contract/healthy/input.json");
    assert_eq!(run(healthy, &[], false), Exit::Success);
    assert_eq!(
        run(healthy, &["--fail-on", "critical"], false),
        Exit::Inconclusive
    );
    assert_eq!(run(healthy, &["--require-metrics"], true), Exit::Success);
    assert_eq!(
        run(
            include_str!("../../examples/contract/empty/input.json"),
            &["--require-metrics"],
            true
        ),
        Exit::Success
    );
}
#[test]
fn uncertain_critical_data_cannot_pass_and_missing_is_not_critical() {
    assert_eq!(
        run(
            include_str!("../../examples/contract/ambiguous-critical/input.json"),
            &["-u", "--fail-on", "critical"],
            true
        ),
        Exit::Inconclusive
    );
    assert_eq!(
        run(
            include_str!("../../examples/contract/missing-stale/input.json"),
            &["-u", "--fail-on", "critical"],
            true
        ),
        Exit::Success
    );
    assert_eq!(
        run(
            include_str!("../../examples/contract/missing-stale/input.json"),
            &["--require-metrics"],
            true
        ),
        Exit::Inconclusive
    );
    assert_eq!(
        run(
            include_str!("../../examples/contract/field-pressure/input.json"),
            &["-u", "--fail-on", "warning"],
            true
        ),
        Exit::Findings
    );
    assert_eq!(
        run(
            include_str!("../../examples/contract/field-pressure/input.json"),
            &["--require-metrics", "--fail-on", "warning"],
            true
        ),
        Exit::Inconclusive
    );
}

#[test]
fn unfamiliar_health_phase_does_not_invalidate_independently_known_byte_metrics() {
    let mut input: serde_json::Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    input["kubernetes"]["pvcs"][0]["status"]["phase"] = "FuturePhase".into();
    assert_eq!(run(&input.to_string(), &[], true), Exit::Success);
    assert_eq!(
        run(&input.to_string(), &["--require-metrics"], true),
        Exit::Success
    );
    assert_eq!(
        run(&input.to_string(), &["--fail-on", "critical"], true),
        Exit::Inconclusive
    );
}
