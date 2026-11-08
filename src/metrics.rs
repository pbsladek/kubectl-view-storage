//! Attribution, independent scalar validation, freshness, and tuple selection.
use crate::{
    cli::Query,
    join::{Graph, diagnostic},
    model::*,
    qty::{Bytes, ExactRatio, Inodes},
    work::{self, Checkpoint, Unbounded},
};
use jiff::Timestamp;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub fn node_targets(graph: &Graph) -> BTreeSet<String> {
    node_targets_checked(graph, &mut Unbounded).unwrap_or_else(|never| match never {})
}
pub(crate) fn node_targets_checked<C: Checkpoint>(
    graph: &Graph,
    control: &mut C,
) -> Result<BTreeSet<String>, C::Error> {
    let mut relevant = BTreeSet::new();
    for record in &graph.records {
        control.step()?;
        if record.volume_mode == VolumeMode::Filesystem {
            for consumer in &record.consumers {
                control.step()?;
                relevant.insert(consumer);
            }
        }
    }
    let mut nodes = BTreeSet::new();
    for consumer in &graph.consumers {
        control.step()?;
        if relevant.contains(&consumer.id)
            && consumer.state.live()
            && let Some(node) = &consumer.node
            && !node.is_empty()
        {
            nodes.insert(node.clone());
        }
    }
    Ok(nodes)
}

pub fn apply(
    graph: &mut Graph,
    responses: &BTreeMap<String, Value>,
    failures: &BTreeMap<String, String>,
    query: &Query,
    now: Timestamp,
) {
    apply_checked(graph, responses, failures, query, now, &mut Unbounded)
        .unwrap_or_else(|never| match never {})
}
pub(crate) fn apply_checked<C: Checkpoint>(
    graph: &mut Graph,
    responses: &BTreeMap<String, Value>,
    failures: &BTreeMap<String, String>,
    query: &Query,
    now: Timestamp,
    control: &mut C,
) -> Result<(), C::Error> {
    if !query.utilization {
        return Ok(());
    }
    let samples = sample_index_checked(responses, control)?;
    let mut consumers = BTreeMap::new();
    for consumer in &graph.consumers {
        control.step()?;
        consumers.insert(&consumer.id, consumer);
    }
    let edges: BTreeMap<_, Vec<_>> = {
        let mut map: BTreeMap<&RecordId, Vec<&Relationship>> = BTreeMap::new();
        for edge in &graph.relationships {
            control.step()?;
            map.entry(&edge.record_id).or_default().push(edge);
        }
        map
    };
    for record in &mut graph.records {
        control.step()?;
        if record.volume_mode == VolumeMode::Block {
            record.filesystem = MetricFamily::NotApplicable;
            record.inodes = MetricFamily::NotApplicable;
            continue;
        }
        if matches!(
            record.volume_mode,
            VolumeMode::Unsupported(_) | VolumeMode::Invalid
        ) {
            let reason = if record.volume_mode == VolumeMode::Invalid {
                "invalid_volume_mode"
            } else {
                "unsupported_volume_mode"
            };
            record.filesystem = MetricFamily::Unavailable {
                status: "invalid".into(),
                reason: reason.into(),
            };
            record.inodes = MetricFamily::Unavailable {
                status: "invalid".into(),
                reason: reason.into(),
            };
            continue;
        }
        let mut bytes = vec![];
        let mut inodes = vec![];
        let mut live = false;
        let mut failure = None;
        for edge in edges.get(&record.id).into_iter().flatten() {
            control.step()?;
            let Some(consumer) = consumers.get(&edge.consumer_id) else {
                continue;
            };
            if !consumer.state.live() {
                continue;
            }
            live = true;
            let Some(node) = &consumer.node else {
                continue;
            };
            if let Some(reason) = failures.get(node) {
                failure = Some(reason.clone());
            }
            for volume in &edge.volume_names {
                control.step()?;
                let key = (
                    node.as_str(),
                    consumer.uid.as_str(),
                    consumer.namespace.as_str(),
                    consumer.name.as_str(),
                    volume.as_str(),
                );
                for sample in samples.get(&key).into_iter().flatten() {
                    control.step()?;
                    if sample.pointer("/pvcRef/name").and_then(Value::as_str)
                        != record.pvc_name.known().map(String::as_str)
                        || sample.pointer("/pvcRef/namespace").and_then(Value::as_str)
                            != record.namespace.known().map(String::as_str)
                    {
                        continue;
                    }
                    let source = Source {
                        node: node.clone(),
                        pod_id: consumer.id.clone(),
                        volume_name: volume.clone(),
                    };
                    let timestamp = sample.get("time").and_then(Value::as_str);
                    let observed_at = timestamp.and_then(|s| s.parse::<Timestamp>().ok());
                    let eligibility = if record.binding_conflict
                        || record.claim_created_at.is_none()
                        || observed_at
                            .zip(record.claim_created_at)
                            .is_some_and(|(sample, created)| sample < created)
                    {
                        Eligibility::IdentityUncertain
                    } else if sample.get("time").is_none()
                        || sample.get("time") == Some(&Value::Null)
                    {
                        Eligibility::TimestampUnknown
                    } else if observed_at.is_none() {
                        Eligibility::InvalidTimestamp
                    } else if observed_at
                        .is_some_and(|t| t.as_nanosecond() - now.as_nanosecond() > 30_000_000_000)
                    {
                        Eligibility::ClockSkew
                    } else if observed_at.is_some_and(|t| {
                        now.as_nanosecond() - t.as_nanosecond()
                            > query.max_metrics_age.as_nanos() as i128
                    }) {
                        Eligibility::Stale
                    } else {
                        Eligibility::Fresh
                    };
                    bytes.push(normalize_candidate::<Bytes>(
                        sample,
                        ["capacityBytes", "usedBytes", "availableBytes"],
                        source.clone(),
                        observed_at,
                        eligibility,
                        &record.id,
                        &mut graph.diagnostics,
                    ));
                    control.step()?;
                    inodes.push(normalize_candidate::<Inodes>(
                        sample,
                        ["inodes", "inodesUsed", "inodesFree"],
                        source,
                        observed_at,
                        eligibility,
                        &record.id,
                        &mut graph.diagnostics,
                    ));
                }
            }
        }
        let reason = if let Some(reason) = failure {
            reason
        } else if let ConsumerVisibility::Unknown(reason) = &record.consumer_visibility {
            reason.clone()
        } else if !live {
            "no_live_consumer".into()
        } else {
            "no_sample".into()
        };
        record.filesystem = select_checked(bytes, &reason, control)?;
        record.inodes = select_checked(inodes, &reason, control)?;
    }
    Ok(())
}

type SampleKey<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str);
fn sample_index_checked<'a, C: Checkpoint>(
    responses: &'a BTreeMap<String, Value>,
    control: &mut C,
) -> Result<BTreeMap<SampleKey<'a>, Vec<&'a Value>>, C::Error> {
    let mut index: BTreeMap<SampleKey<'_>, Vec<&Value>> = BTreeMap::new();
    for (node, summary) in responses {
        control.step()?;
        for pod in summary
            .get("pods")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            control.step()?;
            let Some((uid, namespace, name)) = pod
                .pointer("/podRef/uid")
                .and_then(Value::as_str)
                .zip(pod.pointer("/podRef/namespace").and_then(Value::as_str))
                .zip(pod.pointer("/podRef/name").and_then(Value::as_str))
                .map(|((uid, namespace), name)| (uid, namespace, name))
            else {
                continue;
            };
            for sample in pod
                .get("volume")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                control.step()?;
                if let Some(volume) = sample.get("name").and_then(Value::as_str) {
                    index
                        .entry((node.as_str(), uid, namespace, name, volume))
                        .or_default()
                        .push(sample);
                }
            }
        }
    }
    Ok(index)
}

fn counter<T: Counter>(value: Option<&Value>) -> Field<T> {
    match value {
        None | Some(Value::Null) => Field::Missing,
        Some(value) => match value.as_u64() {
            Some(v) => Field::Valid(T::from_raw(v)),
            None => Field::Invalid,
        },
    }
}
/// Validate one provider tuple independently, retaining valid sibling counters.
pub fn normalize_candidate<T: Counter>(
    sample: &Value,
    keys: [&str; 3],
    source: Source,
    observed_at: Option<Timestamp>,
    eligibility: Eligibility,
    id: &RecordId,
    diagnostics: &mut Vec<Diagnostic>,
) -> Candidate<T> {
    let capacity = counter::<T>(sample.get(keys[0]));
    let mut used = counter::<T>(sample.get(keys[1]));
    let mut available = counter::<T>(sample.get(keys[2]));
    if let Field::Valid(cap) = capacity {
        if cap.raw() > 0 {
            if used.value().is_some_and(|v| v.raw() > cap.raw()) {
                used = Field::Invalid;
            }
            if available.value().is_some_and(|v| v.raw() > cap.raw()) {
                available = Field::Invalid;
            }
        } else {
            diagnostics.push(diagnostic(
                "invalid_metrics",
                id,
                keys[0],
                "zero denominator; independent free/available fields retained",
            ));
        }
    }
    for (key, field) in keys.iter().zip([&capacity, &used, &available]) {
        if matches!(field, Field::Invalid) {
            diagnostics.push(diagnostic(
                "invalid_metrics",
                id,
                key,
                "invalid individual counter; valid sibling fields retained",
            ));
        }
    }
    Candidate {
        capacity,
        used,
        available,
        source,
        observed_at,
        eligibility,
    }
}

pub fn select<T: Counter>(candidates: Vec<Candidate<T>>, reason: &str) -> MetricFamily<T> {
    select_checked(candidates, reason, &mut Unbounded).unwrap_or_else(|never| match never {})
}

// Timestamp/source collisions can still contain different tuples. Canonicalize
// every remaining value so API array order cannot choose a finding or exit.
// This is a whole-observation tie-breaker, independent of policy thresholds.
fn candidate_tie_key<T: Counter>(candidate: &Candidate<T>) -> (u8, [(u8, u64); 3]) {
    let eligibility = match candidate.eligibility {
        Eligibility::Fresh => 0,
        Eligibility::Stale => 1,
        Eligibility::TimestampUnknown => 2,
        Eligibility::ClockSkew => 3,
        Eligibility::InvalidTimestamp => 4,
        Eligibility::IdentityUncertain => 5,
    };
    let fields =
        [&candidate.capacity, &candidate.used, &candidate.available].map(|field| match field {
            Field::Valid(value) => (0, value.raw()),
            Field::Missing => (1, 0),
            Field::Invalid => (2, 0),
        });
    (eligibility, fields)
}

pub(crate) fn select_checked<T: Counter, C: Checkpoint>(
    mut candidates: Vec<Candidate<T>>,
    reason: &str,
    control: &mut C,
) -> Result<MetricFamily<T>, C::Error> {
    work::sort_by(
        &mut candidates,
        |a, b| {
            b.observed_at
                .cmp(&a.observed_at)
                .then_with(|| a.source.cmp(&b.source))
                .then_with(|| candidate_tie_key(a).cmp(&candidate_tie_key(b)))
        },
        control,
    )?;
    let Some(first) = candidates.first() else {
        let status = match reason {
            "forbidden" => "forbidden",
            "timeout" | "deadline_exceeded" => "timeout",
            "invalid_response" | "response_too_large" => "invalid",
            _ => "missing",
        };
        return Ok(MetricFamily::Unavailable {
            status: status.into(),
            reason: reason.into(),
        });
    };
    let mut selected = first.clone();
    for candidate in &candidates {
        control.step()?;
        if candidate.eligibility == Eligibility::Fresh && candidate.usable() {
            selected = candidate.clone();
            break;
        }
    }
    let mut complete = vec![];
    for candidate in &candidates {
        control.step()?;
        if candidate.eligibility == Eligibility::Fresh
            && candidate.complete()
            && let Some(time) = candidate.observed_at
            && let Some(ratio) = candidate.ratio()
        {
            complete.push((time, ratio));
        }
    }
    let mut disagreement = None;
    // Complete candidates retain descending timestamp order. Keep a multiset
    // of ratios in the inclusive 60-second window, so repeated RWX observations
    // cost O(n log n), rather than comparing every sample pair. A qualifying
    // pair exists exactly when the window's minimum and maximum disagree.
    let mut window: BTreeMap<ExactRatio, usize> = BTreeMap::new();
    let mut first = 0;
    for (index, (time, ratio)) in complete.iter().enumerate() {
        control.step()?;
        *window.entry(*ratio).or_default() += 1;
        while first < index
            && complete[first].0.as_nanosecond() - time.as_nanosecond() > 60_000_000_000
        {
            control.step()?;
            let expired = complete[first].1;
            if let Some(count) = window.get_mut(&expired) {
                *count -= 1;
                if *count == 0 {
                    window.remove(&expired);
                }
            }
            first += 1;
        }
        if let Some((&min, &max)) = window
            .first_key_value()
            .zip(window.last_key_value())
            .map(|((min, _), (max, _))| (min, max))
        {
            // Ratios originate from u64 counters, including after reduction,
            // so each product fits u128. Compare the difference exactly.
            let difference =
                max.numerator() * min.denominator() - min.numerator() * max.denominator();
            let denominator = max.denominator() * min.denominator();
            if ExactRatio::new(difference, denominator)
                .is_ok_and(|delta| ExactRatio::new(1, 20).is_ok_and(|threshold| delta >= threshold))
            {
                disagreement = Some(match disagreement {
                    Some((lo, hi)) => (min.min(lo), max.max(hi)),
                    None => (min, max),
                });
            }
        }
    }
    Ok(MetricFamily::Observed {
        selected,
        candidates,
        disagreement,
    })
}

#[cfg(test)]
#[path = "../tests/unit/metrics.rs"]
mod tests;
