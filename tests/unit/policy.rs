#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use clap::Parser;
use std::collections::BTreeMap;
fn fixture(input: &str, args: &[&str]) -> (Graph, Query) {
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
    (graph, query)
}
fn run(input: &str, args: &[&str], complete: bool) -> Exit {
    let (graph, query) = fixture(input, args);
    let input: serde_json::Value = serde_json::from_str(input).unwrap();
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
fn public_conflict_check_respects_the_requested_finding_policy() {
    let input = include_str!("../../examples/contract/ambiguous-critical/input.json");
    let (graph, query) = fixture(input, &["-u", "--fail-on", "critical"]);
    assert!(conflicts_reach_policy(&graph, &query));
    let (graph, query) = fixture(input, &["--require-metrics"]);
    assert!(!conflicts_reach_policy(&graph, &query));
    let (graph, query) = fixture(input, &["--fail-on", "critical"]);
    assert!(!conflicts_reach_policy(&graph, &query));
}

#[test]
fn only_unknown_consumers_linked_to_selected_records_make_a_finding_check_inconclusive() {
    let (mut graph, query) = fixture(
        include_str!("../../examples/contract/healthy/input.json"),
        &["-u", "--fail-on", "warning"],
    );
    assert_eq!(decide(true, &graph, &[], &query), Exit::Success);
    graph.consumers[0].state = ReferenceState::Unknown;
    assert_eq!(decide(true, &graph, &[], &query), Exit::Inconclusive);
    graph.records[0].consumers.clear();
    assert_eq!(decide(true, &graph, &[], &query), Exit::Success);
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
