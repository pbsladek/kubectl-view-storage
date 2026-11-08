#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use clap::Parser;
#[test]
fn healthy_report_matches_shared_contract_semantics() {
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let expected: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/report.json")).unwrap();
    let query = crate::cli::Cli::try_parse_from(["cmd", "-u"])
        .unwrap()
        .query("demo")
        .unwrap();
    let graph = join::normalize(
        &serde_json::from_value(input["kubernetes"].clone()).unwrap(),
        &query,
        true,
    )
    .unwrap();
    let (mut graph, selection) = select(graph, &query);
    let now = input["clock"].as_str().unwrap().parse().unwrap();
    crate::metrics::apply(
        &mut graph,
        &serde_json::from_value(input["summary_responses"].clone()).unwrap(),
        &BTreeMap::new(),
        &query,
        now,
    );
    let findings = crate::findings::evaluate(&graph, &query, now);
    let groups = group::build(&graph, &query, &findings, selection.complete);
    let collection = Collection {
        started_at: now,
        finished_at: now,
        context: "example".into(),
        sources: vec![],
        cost: Cost::initial(),
    };
    let report = build(&graph, &query, &selection, &collection, &groups, &findings).unwrap();
    for key in [
        "records",
        "consumers",
        "relationships",
        "groups",
        "summary",
        "selection",
        "query",
    ] {
        assert_eq!(report[key], expected[key], "{key}");
    }
}

struct Budget(usize);
impl Checkpoint for Budget {
    type Error = ();
    fn step(&mut self) -> Result<(), ()> {
        self.0 = self.0.checked_sub(1).ok_or(())?;
        Ok(())
    }
}

#[test]
fn nested_metadata_and_evidence_serialization_observe_checkpoints() {
    let now = "2026-11-07T00:00:00Z".parse().unwrap();
    let query = crate::cli::Cli::try_parse_from(["cmd"])
        .unwrap()
        .query("demo")
        .unwrap();
    let mut collection = Collection {
        started_at: now,
        finished_at: now,
        context: "nested-fixture".into(),
        sources: vec![],
        cost: Cost::initial(),
    };
    for index in 0..1000 {
        collection.sources.push(SourceCollection {
            resource: "pv".into(),
            scope: format!("pv-{index}"),
            verb: "get".into(),
            status: "ok".into(),
            resource_version: None,
        });
    }
    assert!(collection_checked(&collection, &query, &mut Budget(64)).is_err());
    collection.sources.clear();
    collection.cost.unresolved_prerequisites = (0..1000).map(|n| format!("pv-{n}")).collect();
    assert!(collection_checked(&collection, &query, &mut Budget(64)).is_err());
    let fields: BTreeSet<_> = (0..1000).map(|n| format!("field-{n}")).collect();
    let unknown = UnknownRecord {
        id: "pvc:unknown".into(),
        namespace: Some("demo".into()),
        name: "unknown".into(),
        unresolved_fields: fields.clone(),
    };
    let selection = Selection {
        unknown_records: vec![unknown],
        ..Selection::default()
    };
    assert!(selection_checked(&selection, &mut Budget(64)).is_err());
    let diagnostic = Diagnostic {
        code: "lookup_failed".into(),
        subject_id: None,
        source: "kubernetes".into(),
        reason: "forbidden".into(),
        message: "lookup failed".into(),
        affected_fields: fields.into_iter().collect(),
    };
    assert!(diagnostic_checked(&diagnostic, &mut Budget(64)).is_err());
    let array = json!((0..1000).collect::<Vec<_>>());
    let nested = json!({"samples":array});
    assert!(work::clone_json(&nested, &mut Budget(64)).is_err());
    assert_eq!(work::clone_json(&nested, &mut Unbounded).unwrap(), nested);
}

#[test]
fn candidate_and_group_member_arrays_observe_checkpoints() {
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let query = crate::cli::Cli::try_parse_from(["cmd", "-u"])
        .unwrap()
        .query("demo")
        .unwrap();
    let mut graph = join::normalize(
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
    if let MetricFamily::Observed {
        selected,
        candidates,
        ..
    } = &mut graph.records[0].filesystem
    {
        *candidates = vec![selected.clone(); 1000];
    } else {
        panic!("fixture must have a filesystem observation");
    }
    assert!(record_checked(&graph.records[0], &mut Budget(64)).is_err());
    let mut groups = group::build(&graph, &query, &[], true);
    groups[0].member_ids = (0..1000)
        .map(|index| RecordId::new(crate::cli::Kind::Pvc, &format!("claim-{index}")).unwrap())
        .collect();
    assert!(group_checked(&groups[0], &mut Budget(64)).is_err());
}
