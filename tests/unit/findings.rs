#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use clap::Parser;
use std::collections::BTreeMap;
fn now() -> jiff::Timestamp {
    "2026-11-06T20:00:00Z".parse().unwrap()
}
pub fn fixture(input: &str) -> (Graph, Query) {
    let input: serde_json::Value = serde_json::from_str(input).unwrap();
    let query = crate::cli::Cli::try_parse_from(["cmd", "-u"])
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
#[test]
fn independent_fields_survive_and_exhaustion_is_critical() {
    let (graph, query) = fixture(include_str!(
        "../../examples/contract/field-pressure/input.json"
    ));
    let findings = evaluate(&graph, &query, now());
    for code in [
        "filesystem_usage_high",
        "filesystem_space_exhausted",
        "filesystem_inodes_exhausted",
    ] {
        assert!(findings.iter().any(|f| f.code == code), "{code}");
    }
    assert!(
        findings
            .iter()
            .filter(|f| f.code.ends_with("exhausted"))
            .all(|f| f.severity == Severity::Critical)
    );
}
#[test]
fn conflict_suppresses_pressure_and_healthy_has_no_findings() {
    let (graph, query) = fixture(include_str!(
        "../../examples/contract/ambiguous-critical/input.json"
    ));
    let findings = evaluate(&graph, &query, now());
    assert!(findings.iter().any(|f| f.code == "metrics_disagreement"));
    assert!(!findings.iter().any(|f| f.code == "filesystem_usage_high"));
    let (graph, query) = fixture(include_str!("../../examples/contract/healthy/input.json"));
    assert!(evaluate(&graph, &query, now()).is_empty());
}

#[test]
fn inventory_findings_require_observed_states_and_complete_consumers() {
    let (mut graph, query) = fixture(include_str!("../../examples/contract/healthy/input.json"));
    for (phase, code, severity) in [
        (Phase::Lost, "claim_lost", Severity::Critical),
        (Phase::Failed, "volume_failed", Severity::Warning),
        (Phase::Pending, "claim_pending", Severity::Info),
        (Phase::Released, "volume_released", Severity::Info),
    ] {
        graph.records[0].phase = phase;
        let findings = evaluate(&graph, &query, now());
        assert!(
            findings
                .iter()
                .any(|f| f.code == code && f.severity == severity)
        );
    }
    graph.records[0].binding_conflict = true;
    graph.records[0].active_count = Some(0);
    graph.records[0].terminating_count = Some(0);
    assert!(
        evaluate(&graph, &query, now())
            .iter()
            .any(|f| f.code == "binding_conflict")
    );
    assert!(
        evaluate(&graph, &query, now())
            .iter()
            .any(|f| f.code == "no_active_consumer")
    );
    graph.records[0].active_count = None;
    assert!(
        !evaluate(&graph, &query, now())
            .iter()
            .any(|f| f.code == "no_active_consumer")
    );
}

#[test]
fn unavailable_stale_and_invalid_are_distinct_from_pressure() {
    let (mut graph, query) = fixture(include_str!("../../examples/contract/healthy/input.json"));
    for (status, reason, code) in [
        ("missing", "no_sample", "metrics_missing"),
        ("forbidden", "forbidden", "metrics_unavailable"),
        ("timeout", "timeout", "metrics_unavailable"),
        ("missing", "consumers_unknown", "metrics_unavailable"),
    ] {
        graph.records[0].filesystem = MetricFamily::Unavailable {
            status: status.into(),
            reason: reason.into(),
        };
        assert!(
            evaluate(&graph, &query, now())
                .iter()
                .any(|f| f.code == code)
        );
    }
    let (mut graph, query) = fixture(include_str!("../../examples/contract/healthy/input.json"));
    if let MetricFamily::Observed { selected, .. } = &mut graph.records[0].inodes {
        selected.used = Field::Valid(crate::qty::Inodes::new(900));
        selected.available = Field::Missing;
    }
    assert!(
        evaluate(&graph, &query, now())
            .iter()
            .any(|f| f.code == "inode_usage_high" && f.severity == Severity::Critical)
    );
    if let MetricFamily::Observed { selected, .. } = &mut graph.records[0].filesystem {
        selected.eligibility = Eligibility::Stale;
    }
    assert!(
        evaluate(&graph, &query, now())
            .iter()
            .any(|f| f.code == "metrics_stale")
    );
    if let MetricFamily::Observed { selected, .. } = &mut graph.records[0].filesystem {
        selected.eligibility = Eligibility::ClockSkew;
    }
    assert!(
        evaluate(&graph, &query, now())
            .iter()
            .any(|f| f.code == "invalid_metrics")
    );
}

#[test]
fn inode_only_conflict_and_staleness_keep_correct_family_and_provenance() {
    let (mut graph, query) = fixture(include_str!("../../examples/contract/healthy/input.json"));
    let original = graph.records[0].inodes.selected().unwrap().clone();
    let mut second = original.clone();
    second.used = Field::Valid(crate::qty::Inodes::new(990));
    second.available = Field::Valid(crate::qty::Inodes::new(10));
    second.source.node = "worker-b".into();
    graph.records[0].inodes = crate::metrics::select(vec![original.clone(), second], "no_sample");
    let findings = evaluate(&graph, &query, now());
    let finding = findings
        .iter()
        .find(|finding| finding.code == "metrics_disagreement")
        .unwrap();
    assert_eq!(finding.evidence.len(), 1);
    assert_eq!(finding.evidence[0].field, "inodes.observed_range_percent");
    assert_eq!(finding.evidence[0].value, json!("10..99"));
    assert!(finding.evidence[0].source.is_some());
    assert!(
        !findings
            .iter()
            .any(|finding| finding.code == "inode_usage_high")
    );
    assert_eq!(graph.records[0].filesystem.status(), "ok");
    let mut stale = original;
    stale.eligibility = Eligibility::Stale;
    graph.records[0].inodes = crate::metrics::select(vec![stale.clone()], "no_sample");
    let findings = evaluate(&graph, &query, now());
    let finding = findings
        .iter()
        .find(|finding| finding.code == "metrics_stale")
        .unwrap();
    assert_eq!(finding.evidence[0].field, "inodes.status");
    assert_eq!(finding.evidence[0].value, json!("stale"));
    assert_eq!(finding.evidence[0].observed_at, stale.observed_at);
    assert_eq!(finding.evidence[0].source.as_ref(), Some(&stale.source));
}

#[test]
fn distinct_family_failures_and_partial_pressure_have_actionable_evidence() {
    let (mut graph, query) = fixture(include_str!(
        "../../examples/contract/field-pressure/input.json"
    ));
    let findings = evaluate(&graph, &query, now());
    let high = findings
        .iter()
        .find(|finding| {
            finding.code == "filesystem_usage_high" && finding.severity == Severity::Critical
        })
        .unwrap();
    assert_eq!(high.evidence[0].field, "used_percent");
    assert_eq!(high.evidence[0].value.to_string(), "95");
    assert_eq!(high.evidence[0].unit.as_deref(), Some("percent"));
    assert!(
        high.evidence
            .iter()
            .any(|evidence| evidence.field == "used_bytes")
    );
    assert!(
        high.evidence
            .iter()
            .all(|evidence| evidence.source.is_some() && evidence.observed_at.is_some())
    );
    graph.records[0].filesystem = MetricFamily::Unavailable {
        status: "missing".into(),
        reason: "request_budget_exhausted".into(),
    };
    let mut stale = graph.records[0].inodes.selected().unwrap().clone();
    stale.eligibility = Eligibility::Stale;
    graph.records[0].inodes = crate::metrics::select(vec![stale], "no_sample");
    let findings = evaluate(&graph, &query, now());
    for code in ["metrics_unavailable", "metrics_stale"] {
        assert!(
            findings
                .iter()
                .any(|finding| finding.code == code && finding.subject_id == graph.records[0].id)
        );
    }
    if let MetricFamily::Observed { selected, .. } = &mut graph.records[1].filesystem {
        selected.available = Field::Invalid;
    }
    let findings = evaluate(&graph, &query, now());
    let invalid = findings
        .iter()
        .find(|finding| finding.code == "invalid_metrics")
        .unwrap();
    assert!(invalid.evidence.iter().any(|evidence| evidence.field
        == "filesystem.available_bytes.status"
        && evidence.value == "invalid"));
}

#[test]
fn disagreement_hint_agrees_with_the_actual_strict_risk() {
    let (mut graph, mut query) =
        fixture(include_str!("../../examples/contract/healthy/input.json"));
    query.fail_on = Some(Severity::Critical);
    let template = graph.records[0].filesystem.selected().unwrap().clone();
    for (used, expected_exit, inconclusive) in [
        ([400, 450], crate::policy::Exit::Success, false),
        ([900, 990], crate::policy::Exit::Inconclusive, true),
    ] {
        let candidates = used
            .into_iter()
            .map(|used| {
                let mut candidate = template.clone();
                candidate.capacity = Field::Valid(crate::qty::Bytes::new(1000));
                candidate.used = Field::Valid(crate::qty::Bytes::new(used));
                candidate.available = Field::Valid(crate::qty::Bytes::new(1000 - used));
                candidate
            })
            .collect();
        graph.records[0].filesystem = crate::metrics::select(candidates, "no_sample");
        let findings = evaluate(&graph, &query, now());
        let disagreement = findings
            .iter()
            .find(|finding| finding.code == "metrics_disagreement")
            .unwrap();
        assert_eq!(
            disagreement.suggested_action.contains("inconclusive"),
            inconclusive
        );
        assert_eq!(
            crate::policy::decide(true, &graph, &findings, &query),
            expected_exit
        );
    }
}

#[test]
fn pending_evidence_uses_collection_clock_and_only_known_binding_mode_and_age() {
    let mut input: serde_json::Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    input["kubernetes"]["pvcs"][0]["status"]["phase"] = "Pending".into();
    input["kubernetes"]["pvcs"][0]["metadata"]["creationTimestamp"] =
        "2026-11-06T19:58:59.5Z".into();
    input["kubernetes"]["storage_classes"][0]["volumeBindingMode"] = "WaitForFirstConsumer".into();
    let (graph, query) = fixture(&input.to_string());
    assert_eq!(
        graph.records[0].volume_binding_mode.as_deref(),
        Some("WaitForFirstConsumer")
    );
    let findings = evaluate(&graph, &query, now());
    let pending = findings
        .iter()
        .find(|finding| finding.code == "claim_pending")
        .unwrap();
    assert_eq!(pending.severity, Severity::Info);
    assert!(
        pending
            .evidence
            .iter()
            .any(|evidence| evidence.field == "age_seconds"
                && evidence.value == "60"
                && evidence.observed_at == Some(now()))
    );
    assert!(
        pending
            .evidence
            .iter()
            .any(|evidence| evidence.field == "volume_binding_mode"
                && evidence.value == "WaitForFirstConsumer")
    );
    input["kubernetes"]["storage_classes"] = json!([]);
    for created in ["not-a-timestamp", "2026-11-06T20:00:01Z"] {
        input["kubernetes"]["pvcs"][0]["metadata"]["creationTimestamp"] = created.into();
        let (graph, query) = fixture(&input.to_string());
        let findings = evaluate(&graph, &query, now());
        let pending = findings
            .iter()
            .find(|finding| finding.code == "claim_pending")
            .unwrap();
        assert_eq!(pending.evidence.len(), 1);
        assert_eq!(pending.evidence[0].field, "phase");
    }
}
