#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::{
    cli::Cli,
    clock::Clock,
    join,
    model::{ConsumerVisibility, MetadataValue, RecordId},
    report::{Collection, Cost, Selection},
};
use clap::Parser;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::time::Instant;

// This fixture tests normalized DTO processing, independently of transport.
fn collected(count: usize) -> (Collected, Query) {
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let query = Cli::try_parse_from(["cmd", "-g", "storageclass,pvc", "--fail-on", "critical"])
        .unwrap()
        .query("demo")
        .unwrap();
    let mut graph = join::normalize(
        &serde_json::from_value(input["kubernetes"].clone()).unwrap(),
        &query,
        true,
    )
    .unwrap();
    let template = graph.records[0].clone();
    graph.records.clear();
    graph.consumers.clear();
    graph.relationships.clear();
    graph.diagnostics.clear();
    for index in 0..count {
        let mut record = template.clone();
        record.uid = format!("claim-{index:05}");
        record.id = RecordId::new(record.kind, &record.uid).unwrap();
        record.name = format!("claim-{index:05}");
        record.pvc_name = MetadataValue::Known(record.name.clone());
        record.pv_name = Some(format!("pv-{index:05}"));
        record.consumers.clear();
        record.consumer_visibility = ConsumerVisibility::Complete;
        record.active_count = Some(0);
        record.terminating_count = Some(0);
        graph.records.push(record);
    }
    graph.validate().unwrap();
    let now = input["clock"].as_str().unwrap().parse().unwrap();
    (
        Collected {
            graph,
            selection: Selection {
                complete: true,
                base_count: count,
                matched_count: count,
                ..Selection::default()
            },
            collection: Collection {
                started_at: now,
                finished_at: now,
                context: "finalization-fixture".into(),
                sources: vec![],
                cost: Cost::initial(),
            },
        },
        query,
    )
}

struct Budget(usize);
impl Checkpoint for Budget {
    type Error = &'static str;
    fn step(&mut self) -> Result<(), Self::Error> {
        self.0 = self.0.checked_sub(1).ok_or("stopped")?;
        Ok(())
    }
}

#[test]
fn every_large_finalization_stage_observes_processing_checkpoints() {
    let (collected, query) = collected(1000);
    let graph = &collected.graph;
    let now = collected.collection.finished_at;
    let findings = findings::evaluate(graph, &query, now);
    let groups = group::build(graph, &query, &findings, true);
    macro_rules! stops {
        ($operation:expr) => {
            assert!(matches!($operation, Err("stopped")))
        };
    }
    assert!(matches!(
        graph.validate_checked(&mut Budget(64)),
        Err(CheckedError::Interrupted("stopped"))
    ));
    stops!(findings::evaluate_checked(
        graph,
        &query,
        now,
        &mut Budget(64)
    ));
    stops!(policy::decide_checked(
        true,
        graph,
        &findings,
        &query,
        &mut Budget(64)
    ));
    stops!(sort::records_checked(
        &mut graph.records.clone(),
        query.sort,
        &findings,
        &mut Budget(64)
    ));
    stops!(group::build_checked(
        graph,
        &query,
        &findings,
        true,
        &mut Budget(64)
    ));
    stops!(group::totals_checked(
        &graph.records.iter().collect::<Vec<_>>(),
        &findings,
        true,
        false,
        &mut Budget(64)
    ));
    stops!(sort::groups_checked(
        &mut groups.clone(),
        &query,
        &mut Budget(64)
    ));
    assert!(matches!(
        report::build_checked(
            graph,
            &query,
            &collected.selection,
            &collected.collection,
            &groups,
            &findings,
            &mut Budget(64)
        ),
        Err(CheckedError::Interrupted("stopped"))
    ));
    // Exercise later stages too: every checkpoint is a valid place to stop;
    // a partial sorted graph/report must never escape as success.
    for allowance in [0, 64, 4000, 8000, 12000, 16000, 24000, 32000] {
        let (input, query) = self::collected(1000);
        assert!(
            matches!(
                finish_checked(input, &query, &mut Budget(allowance)),
                Err(CheckedError::Interrupted("stopped"))
            ),
            "unexpected completed report with {allowance} checkpoints"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn high_cardinality_groups_count_each_members_findings_once() {
    let (input, query) = collected(1000);
    let capacity = input.graph.records[0].provisioned.value().unwrap().get() as u128;
    let cancel = CancellationToken::new();
    let report = finish_cancellable(input, &query, cancel.clone())
        .await
        .unwrap();
    assert_eq!(report.exit, Exit::Success);
    assert!(!cancel.is_cancelled());
    assert_eq!(report.document["summary"]["record_count"], 1000);
    assert_eq!(report.document["summary"]["finding_counts"]["info"], 1000);
    assert_eq!(
        report.document["summary"]["provisioned"]["known_sum_bytes"],
        (capacity * 1000).to_string()
    );
    let root = &report.document["groups"][0];
    assert_eq!(root["totals"]["finding_counts"]["info"], 1000);
    let leaves = root["children"].as_array().unwrap();
    assert_eq!(leaves.len(), 1000);
    for leaf in leaves {
        assert_eq!(leaf["member_ids"].as_array().unwrap().len(), 1);
        assert_eq!(leaf["totals"]["finding_counts"]["info"], 1);
        assert_eq!(
            leaf["totals"]["provisioned"]["known_sum_bytes"],
            capacity.to_string()
        );
    }
}

struct CancelDuringWork {
    reads: AtomicUsize,
    cancel: CancellationToken,
    instant: Instant,
}
impl Clock for CancelDuringWork {
    fn monotonic(&self) -> Instant {
        if self.reads.fetch_add(1, Ordering::SeqCst) == 1 {
            self.cancel.cancel();
        }
        self.instant
    }
    fn utc(&self) -> jiff::Timestamp {
        "2026-11-07T00:00:00Z".parse().unwrap()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_during_report_cpu_work_joins_before_return() {
    let (input, query) = collected(1000);
    let cancel = CancellationToken::new();
    let clock = Arc::new(CancelDuringWork {
        reads: AtomicUsize::new(0),
        cancel: cancel.clone(),
        instant: Instant::now(),
    });
    let control = RunControl::retained_inventory(cancel.clone(), clock.clone(), clock.instant);
    assert!(matches!(
        finish_controlled(input, query, control).await,
        Err(FinishError::Interrupted)
    ));
    assert!(cancel.is_cancelled());
    assert_eq!(clock.reads.load(Ordering::SeqCst), 2);
    tokio::task::yield_now().await;
    assert_eq!(
        clock.reads.load(Ordering::SeqCst),
        2,
        "worker survived its join"
    );
}

#[tokio::test]
async fn cancelled_and_invalid_reports_cannot_escape_as_success() {
    let (input, query) = collected(1);
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        finish_cancellable(input, &query, cancel).await,
        Err(FinishError::Interrupted)
    ));
    let (mut input, query) = collected(1);
    input.graph.records[0].uid = "changed".into();
    assert!(
        matches!(finish_cancellable(input, &query, CancellationToken::new()).await,
        Err(FinishError::Invalid(message)) if message.contains("record identity"))
    );
}
