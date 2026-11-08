#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use clap::Parser;
fn run(input: &str) -> Graph {
    let value: Value = serde_json::from_str(input).unwrap();
    let inventory = serde_json::from_value(value["kubernetes"].clone()).unwrap();
    let q = crate::cli::Cli::try_parse_from(["cmd", "-u"])
        .unwrap()
        .query("demo")
        .unwrap();
    let mut graph = crate::join::normalize(&inventory, &q, true).unwrap();
    apply(
        &mut graph,
        &serde_json::from_value(value["summary_responses"].clone()).unwrap(),
        &BTreeMap::new(),
        &q,
        value["clock"].as_str().unwrap().parse().unwrap(),
    );
    graph
}

struct InterruptAfter {
    remaining: usize,
}
impl Checkpoint for InterruptAfter {
    type Error = ();
    fn step(&mut self) -> Result<(), ()> {
        self.remaining = self.remaining.checked_sub(1).ok_or(())?;
        Ok(())
    }
}

#[test]
fn statistics_index_normalization_selection_and_target_discovery_are_interruptible() {
    let bundle: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let query = crate::cli::Cli::try_parse_from(["cmd", "-u"])
        .unwrap()
        .query("demo")
        .unwrap();
    let now = bundle["clock"].as_str().unwrap().parse().unwrap();
    for nested in ["pods", "volume"] {
        let mut summaries = bundle["summary_responses"].clone();
        let value = if nested == "pods" {
            summaries["worker-a"]["pods"][0].clone()
        } else {
            summaries["worker-a"]["pods"][0]["volume"][0].clone()
        };
        let repeated = Value::Array(vec![value; 1000]);
        if nested == "pods" {
            summaries["worker-a"]["pods"] = repeated;
        } else {
            summaries["worker-a"]["pods"][0]["volume"] = repeated;
        }
        let summaries = serde_json::from_value(summaries).unwrap();
        let mut graph = run(include_str!("../../examples/contract/healthy/input.json"));
        assert!(
            apply_checked(
                &mut graph,
                &summaries,
                &BTreeMap::new(),
                &query,
                now,
                &mut InterruptAfter { remaining: 64 }
            )
            .is_err(),
            "{nested}"
        );
    }
    let graph = run(include_str!("../../examples/contract/healthy/input.json"));
    let candidate = graph.records[0].filesystem.selected().unwrap().clone();
    for remaining in [64, 1001, 5000] {
        assert!(
            select_checked(
                vec![candidate.clone(); 1000],
                "no_sample",
                &mut InterruptAfter { remaining }
            )
            .is_err()
        );
    }
    let mut input: crate::transport::Inventory =
        serde_json::from_value(bundle["kubernetes"].clone()).unwrap();
    let pod = input.pods[0].clone();
    input.pods = (0..1000)
        .map(|index| {
            let mut pod = pod.clone();
            pod.metadata.uid = Some(format!("pod-{index}"));
            pod.metadata.name = format!("pod-{index}");
            pod
        })
        .collect();
    let graph = crate::join::normalize(&input, &query, true).unwrap();
    graph.validate().unwrap();
    assert_eq!(node_targets(&graph), BTreeSet::from(["worker-a".into()]));
    assert!(node_targets_checked(&graph, &mut InterruptAfter { remaining: 64 }).is_err());
}
#[test]
fn healthy_partial_and_stale_inputs() {
    let graph = run(include_str!("../../examples/contract/healthy/input.json"));
    assert_eq!(node_targets(&graph).len(), 1);
    assert_eq!(graph.records[0].filesystem.status(), "ok");
    assert_eq!(
        graph.records[0]
            .inodes
            .selected()
            .unwrap()
            .ratio()
            .unwrap()
            .percent(),
        "10"
    );
    let graph = run(include_str!(
        "../../examples/contract/missing-stale/input.json"
    ));
    assert!(
        graph
            .records
            .iter()
            .any(|r| r.filesystem.status() == "stale")
    );
    assert!(
        graph
            .records
            .iter()
            .any(|r| r.filesystem.status() == "missing")
    );
    let graph = run(include_str!(
        "../../examples/contract/field-pressure/input.json"
    ));
    assert!(
        graph
            .records
            .iter()
            .any(|r| r.filesystem.status() == "partial")
    );
}
#[test]
fn conflicting_samples_retain_candidates_without_merging() {
    let graph = run(include_str!(
        "../../examples/contract/ambiguous-critical/input.json"
    ));
    let record = &graph.records[0];
    assert_eq!(record.filesystem.status(), "uncertain");
    if let MetricFamily::Observed {
        candidates,
        disagreement,
        ..
    } = &record.filesystem
    {
        assert_eq!(candidates.len(), 2);
        assert!(disagreement.is_some());
    } else {
        panic!("expected candidates");
    }
    assert!(record.filesystem.fresh_unambiguous().is_none());
}
#[test]
fn reused_identity_unscheduled_and_independent_fields() {
    let mut value: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    value["kubernetes"]["pvcs"][0]["metadata"]["creationTimestamp"] = "2026-11-06T20:00:00Z".into();
    assert_eq!(
        run(&value.to_string()).records[0].filesystem.status(),
        "identity_uncertain"
    );
    value["kubernetes"]["pods"][0]["spec"]["nodeName"] = "".into();
    assert!(node_targets(&run(&value.to_string())).is_empty());
    let graph = run(include_str!(
        "../../examples/contract/terminating-consumer/input.json"
    ));
    assert_eq!(graph.records[0].filesystem.status(), "ok");
}

fn complete_candidate(template: &Candidate<Bytes>, used: u64, seconds: i64) -> Candidate<Bytes> {
    let mut candidate = template.clone();
    candidate.capacity = Field::Valid(Bytes::new(1000));
    candidate.used = Field::Valid(Bytes::new(used));
    candidate.available = Field::Valid(Bytes::new(1000 - used));
    candidate.observed_at = Some(Timestamp::from_second(1_800_000_000 + seconds).unwrap());
    candidate
}

// Independent pairwise oracle, intentionally used only for small test sets.
fn pairwise_range(candidates: &[Candidate<Bytes>]) -> Option<(ExactRatio, ExactRatio)> {
    let mut range = None;
    for (index, a) in candidates.iter().enumerate() {
        for b in candidates.iter().skip(index + 1) {
            if a.eligibility != Eligibility::Fresh
                || b.eligibility != Eligibility::Fresh
                || !a.complete()
                || !b.complete()
                || !a.observed_at.zip(b.observed_at).is_some_and(|(a, b)| {
                    (a.as_nanosecond() - b.as_nanosecond()).abs() <= 60_000_000_000
                })
            {
                continue;
            }
            let (Some(ar), Some(br)) = (a.ratio(), b.ratio()) else {
                continue;
            };
            let difference =
                (ar.numerator() * br.denominator()).abs_diff(br.numerator() * ar.denominator());
            if ExactRatio::new(difference, ar.denominator() * br.denominator()).unwrap()
                < ExactRatio::new(1, 20).unwrap()
            {
                continue;
            }
            let (min, max) = (ar.min(br), ar.max(br));
            range = Some(match range {
                Some((lo, hi)) => (min.min(lo), max.max(hi)),
                None => (min, max),
            });
        }
    }
    range
}

#[test]
fn sliding_conflicts_match_pairwise_oracle_across_windows_and_partial_samples() {
    let graph = run(include_str!("../../examples/contract/healthy/input.json"));
    let template = graph.records[0].filesystem.selected().unwrap();
    let mut seed = 17u64;
    for _ in 0..200 {
        let mut candidates = vec![];
        for index in 0..40 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let mut candidate =
                complete_candidate(template, seed % 1001, ((seed >> 20) % 181) as i64);
            if index % 11 == 0 {
                candidate.available = Field::Missing;
            }
            if index % 13 == 0 {
                candidate.eligibility = Eligibility::Stale;
            }
            if index % 17 == 0 {
                candidate.observed_at = None;
            }
            candidates.push(candidate);
        }
        let expected = pairwise_range(&candidates);
        let MetricFamily::Observed { disagreement, .. } = select(candidates, "no_sample") else {
            panic!("expected samples");
        };
        assert_eq!(disagreement, expected);
    }
    // The inclusive window and exact 5 percentage point threshold both matter.
    for (gap, used, expected) in [(60, 550, true), (61, 550, false), (60, 549, false)] {
        let candidates = vec![
            complete_candidate(template, 500, 0),
            complete_candidate(template, used, gap),
        ];
        let MetricFamily::Observed { disagreement, .. } = select(candidates, "no_sample") else {
            panic!("expected samples");
        };
        assert_eq!(disagreement.is_some(), expected);
    }
    // Both ratios display as 50%/55%. u64-sized counters on either side of
    // the exact boundary still produce different classifications, with
    // denominator cross-products near the u128 limit.
    for (left_used, right_used, expected) in [
        (u64::MAX / 2, 10_145_709_240_540_253_387, true),
        (u64::MAX / 2 + 1, 10_145_709_240_540_253_387, false),
        (u64::MAX / 2 + 1, 10_145_709_240_540_253_388, true),
    ] {
        let candidates = [(u64::MAX, left_used), (u64::MAX - 2, right_used)]
            .into_iter()
            .map(|(capacity, used)| {
                let mut candidate = template.clone();
                candidate.capacity = Field::Valid(Bytes::new(capacity));
                candidate.used = Field::Valid(Bytes::new(used));
                candidate.available = Field::Valid(Bytes::new(capacity - used));
                candidate
            })
            .collect();
        let MetricFamily::Observed { disagreement, .. } = select(candidates, "no_sample") else {
            panic!("expected samples");
        };
        assert_eq!(disagreement.is_some(), expected);
    }
}

#[test]
fn ten_thousand_repeated_samples_retain_tuples_and_exact_range() {
    let graph = run(include_str!("../../examples/contract/healthy/input.json"));
    let template = graph.records[0].filesystem.selected().unwrap();
    let candidates = (0..10_000)
        .map(|index| {
            complete_candidate(template, if index % 2 == 0 { 500 } else { 550 }, index % 61)
        })
        .collect();
    let MetricFamily::Observed {
        candidates,
        disagreement,
        selected,
    } = select(candidates, "no_sample")
    else {
        panic!("expected samples");
    };
    assert_eq!(candidates.len(), 10_000);
    assert_eq!(
        selected.observed_at,
        Some(Timestamp::from_second(1_800_000_060).unwrap())
    );
    assert_eq!(
        disagreement,
        Some((
            ExactRatio::new(1, 2).unwrap(),
            ExactRatio::new(11, 20).unwrap()
        ))
    );
}
