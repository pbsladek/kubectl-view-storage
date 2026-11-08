#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use clap::Parser;
fn fixture() -> Graph {
    let value: serde_json::Value = serde_json::from_str(include_str!(
        "../../examples/contract/shared-rwx/input.json"
    ))
    .unwrap();
    let query = crate::cli::Cli::try_parse_from(["cmd"])
        .unwrap()
        .query("demo")
        .unwrap();
    crate::join::normalize(
        &serde_json::from_value(value["kubernetes"].clone()).unwrap(),
        &query,
        true,
    )
    .unwrap()
}
#[test]
fn repeated_consumers_and_dimension_order_preserve_union_totals() {
    let graph = fixture();
    for dims in ["node,pod,pvc", "pod,node,pvc", "pvc,node,pod"] {
        let query = crate::cli::Cli::try_parse_from(["cmd", "-g", dims])
            .unwrap()
            .query("demo")
            .unwrap();
        let groups = build(&graph, &query, &[], true);
        let union: BTreeSet<_> = groups.iter().flat_map(|g| g.member_ids.iter()).collect();
        assert_eq!(union.len(), 1);
        assert!(
            groups
                .iter()
                .all(|g| g.totals.provisioned.sum == Some(107374182400))
        );
        if dims.starts_with("node") {
            assert_eq!(groups.len(), 2);
            assert!(overlap(&groups));
            assert_eq!(groups.iter().map(|g| g.children.len()).sum::<usize>(), 3);
        }
    }
}
#[test]
fn missing_quantities_and_empty_results_are_distinct() {
    let mut graph = fixture();
    graph.records[0].provisioned = Field::Missing;
    let total = totals(&graph.records.iter().collect::<Vec<_>>(), &[], false, false);
    assert_eq!(total.provisioned.sum, None);
    assert!(!total.provisioned.complete);
    assert_eq!(total.provisioned.known_count, 0);
    assert!(!total.selection_complete);
    let empty = totals(&[], &[], true, true);
    assert_eq!(empty.requested.sum, Some(0));
    assert!(empty.requested.complete);
    assert_eq!(empty.filesystem.used, None);
    assert_eq!(empty.filesystem.coverage.eligible, 0);
}
#[test]
fn exact_weighted_metrics_and_time_skew() {
    let value: serde_json::Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let query = crate::cli::Cli::try_parse_from(["cmd", "-u"])
        .unwrap()
        .query("demo")
        .unwrap();
    let mut graph = crate::join::normalize(
        &serde_json::from_value(value["kubernetes"].clone()).unwrap(),
        &query,
        true,
    )
    .unwrap();
    crate::metrics::apply(
        &mut graph,
        &serde_json::from_value(value["summary_responses"].clone()).unwrap(),
        &BTreeMap::new(),
        &query,
        value["clock"].as_str().unwrap().parse().unwrap(),
    );
    let mut second = graph.records[0].clone();
    second.id = RecordId::new(crate::cli::Kind::Pvc, "second").unwrap();
    if let MetricFamily::Observed { selected, .. } = &mut second.filesystem {
        selected.capacity = Field::Valid(Bytes::new(100));
        selected.used = Field::Valid(Bytes::new(0));
        selected.available = Field::Valid(Bytes::new(100));
    }
    graph.records.push(second);
    let total = totals(&graph.records.iter().collect::<Vec<_>>(), &[], true, true);
    assert_eq!(total.filesystem.used, Some(42949672960));
    assert!(total.filesystem.ratio.unwrap() < ExactRatio::new(40, 100).unwrap());
    if let MetricFamily::Observed { selected, .. } = &mut graph.records[1].filesystem {
        selected.observed_at = Some("2026-11-06T19:58:49Z".parse().unwrap());
    }
    let total = totals(&graph.records.iter().collect::<Vec<_>>(), &[], true, true);
    assert_eq!(total.filesystem.reason.as_deref(), Some("time_skew"));
    assert!(total.filesystem.used.is_none());
}

#[test]
fn block_partial_overlap_and_findings_have_independent_coverage() {
    let mut graph = fixture();
    let id = graph.records[0].id.clone();
    let findings = vec![Finding {
        code: "volume_failed".into(),
        severity: crate::cli::Severity::Warning,
        subject_id: id,
        evidence: vec![],
        suggested_action: "inspect".into(),
    }];
    let total = totals(
        &graph.records.iter().collect::<Vec<_>>(),
        &findings,
        true,
        false,
    );
    assert_eq!(total.finding_counts["warning"], 1);
    assert_eq!(total.filesystem.status, "not_requested");
    graph.records[0].volume_mode = VolumeMode::Block;
    let total = totals(
        &graph.records.iter().collect::<Vec<_>>(),
        &findings,
        true,
        true,
    );
    assert_eq!(total.filesystem.status, "not_applicable");
    assert_eq!(total.filesystem.coverage.not_applicable, 1);
    graph.records[0].volume_mode = VolumeMode::Filesystem;
    graph.records[0].overlapping_filesystem = true;
    let total = totals(
        &graph.records.iter().collect::<Vec<_>>(),
        &findings,
        true,
        true,
    );
    assert_eq!(
        total.filesystem.reason.as_deref(),
        Some("overlapping_filesystem")
    );
    assert!(total.filesystem.used.is_none());
    graph.records[0].overlapping_filesystem = false;
    graph.records[0].filesystem = MetricFamily::Unavailable {
        status: "forbidden".into(),
        reason: "forbidden".into(),
    };
    let total = totals(
        &graph.records.iter().collect::<Vec<_>>(),
        &findings,
        true,
        true,
    );
    assert_eq!(total.filesystem.coverage.by_reason["forbidden"], 1);
    assert_eq!(
        total.filesystem.reason.as_deref(),
        Some("no_usable_samples")
    );
}

#[test]
fn unresolved_and_absent_reference_keys_do_not_merge_or_create_cross_products() {
    let mut graph = fixture();
    let query = crate::cli::Cli::try_parse_from([
        "cmd",
        "-g",
        "storageclass,driver,namespace,pv,node,pod,pvc",
    ])
    .unwrap()
    .query("demo")
    .unwrap();
    graph.records[0].storage_class = MetadataValue::Unknown;
    graph.records[0].driver = MetadataValue::Absent;
    graph.records[0].pv_name = None;
    graph.records[0].consumer_visibility = ConsumerVisibility::Unknown("forbidden".into());
    let groups = build(&graph, &query, &[], false);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].value, MetadataValue::Unknown);
    assert_eq!(groups[0].children[0].value, MetadataValue::Absent);
    assert!(!groups[0].totals.selection_complete);
    assert!(groups[0].id.contains("~unknown"));
    assert_eq!(
        percent_escape_checked("a/b%~", &mut Unbounded).unwrap(),
        "a%2Fb%25%7E"
    );
}
