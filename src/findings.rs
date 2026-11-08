//! Health rules consume normalized fields; unrelated invalid fields stay local.
use crate::{
    cli::{Query, Severity},
    join::Graph,
    model::*,
    work::{self, Checkpoint, Unbounded},
};
use serde_json::json;
use std::collections::BTreeMap;

pub fn pressure<T: Counter>(candidate: &Candidate<T>, query: &Query) -> Option<Severity> {
    if candidate.eligibility != Eligibility::Fresh {
        return None;
    }
    if candidate.available.value().is_some_and(|v| v.raw() == 0) {
        return Some(Severity::Critical);
    }
    usage(candidate, query)
}
pub fn usage<T: Counter>(candidate: &Candidate<T>, query: &Query) -> Option<Severity> {
    if candidate.eligibility != Eligibility::Fresh {
        return None;
    }
    let used = candidate.used.value()?.raw() as u128;
    let capacity = candidate.capacity.value()?.raw() as u128;
    if query.critical_used.reached(used, capacity) {
        Some(Severity::Critical)
    } else if query.warn_used.reached(used, capacity) {
        Some(Severity::Warning)
    } else {
        None
    }
}

pub fn conflicting_pressure<T: Counter>(
    family: &MetricFamily<T>,
    query: &Query,
) -> Option<Severity> {
    conflicting_pressure_checked(family, query, &mut Unbounded)
        .unwrap_or_else(|never| match never {})
}
pub(crate) fn conflicting_pressure_checked<T: Counter, C: Checkpoint>(
    family: &MetricFamily<T>,
    query: &Query,
    control: &mut C,
) -> Result<Option<Severity>, C::Error> {
    let mut severity = None;
    if let MetricFamily::Observed {
        disagreement: Some(_),
        candidates,
        ..
    } = family
    {
        for candidate in candidates {
            control.step()?;
            severity = severity.max(pressure(candidate, query));
        }
    }
    Ok(severity)
}

pub fn evaluate(graph: &Graph, query: &Query, now: jiff::Timestamp) -> Vec<Finding> {
    evaluate_checked(graph, query, now, &mut Unbounded).unwrap_or_else(|never| match never {})
}
pub(crate) fn evaluate_checked<C: Checkpoint>(
    graph: &Graph,
    query: &Query,
    now: jiff::Timestamp,
    control: &mut C,
) -> Result<Vec<Finding>, C::Error> {
    let mut findings = vec![];
    for record in &graph.records {
        control.step()?;
        let mut add = |code: &str, severity: Severity, evidence: Vec<Evidence>, action: &str| {
            findings.push(Finding {
                code: code.into(),
                severity,
                subject_id: record.id.clone(),
                evidence,
                suggested_action: action.into(),
            })
        };
        match record.phase {
            Phase::Lost => add(
                "claim_lost",
                Severity::Critical,
                vec![plain("phase", json!("Lost"), None)],
                "Inspect the claim binding and volume state.",
            ),
            Phase::Failed => add(
                "volume_failed",
                Severity::Warning,
                vec![plain("phase", json!("Failed"), None)],
                "Inspect volume status and storage controller events.",
            ),
            Phase::Pending => {
                let mut evidence = vec![plain("phase", json!("Pending"), None)];
                if let Some(created) = record.created_at.filter(|created| *created <= now) {
                    evidence.push(Evidence {
                        field: "age_seconds".into(),
                        value: json!(
                            ((now.as_nanosecond() - created.as_nanosecond()) / 1_000_000_000)
                                .to_string()
                        ),
                        unit: Some("seconds".into()),
                        observed_at: Some(now),
                        source: None,
                    });
                }
                if let Some(mode) = &record.volume_binding_mode {
                    evidence.push(plain("volume_binding_mode", json!(mode), None));
                }
                add(
                    "claim_pending",
                    Severity::Info,
                    evidence,
                    "Inspect provisioning and delayed binding conditions.",
                );
            }
            Phase::Released => add(
                "volume_released",
                Severity::Info,
                vec![
                    plain("phase", json!("Released"), None),
                    plain("reclaim_policy", json!(record.reclaim_policy), None),
                ],
                "Review reclaim policy and ownership before taking action.",
            ),
            _ => {}
        }
        if record.binding_conflict {
            add(
                "binding_conflict",
                Severity::Warning,
                vec![plain("pv_name", json!(record.pv_name), None)],
                "Inspect claim and volume UIDs; attribution is unverified.",
            );
        }
        if record.active_count == Some(0) && record.terminating_count == Some(0) {
            add(
                "no_active_consumer",
                Severity::Info,
                vec![
                    plain("active_consumer_count", json!(0), None),
                    plain("terminating_consumer_count", json!(0), None),
                ],
                "Review visible pod references and ownership; this does not establish deletion safety.",
            );
        }
        family_rules(
            &record.filesystem,
            query,
            ["filesystem_usage_high", "filesystem_space_exhausted"],
            ["used_bytes", "available_bytes"],
            "bytes",
            &mut add,
        );
        family_rules(
            &record.inodes,
            query,
            ["inode_usage_high", "filesystem_inodes_exhausted"],
            ["inodes_used", "inodes_free"],
            "inodes",
            &mut add,
        );
        let mut notes = BTreeMap::new();
        family_notes(&record.filesystem, "filesystem", &mut notes);
        family_notes(&record.inodes, "inodes", &mut notes);
        for (code, (severity, evidence)) in notes {
            add(
                code,
                severity,
                evidence,
                match code {
                    "metrics_disagreement"
                        if query.fail_on == Some(Severity::Critical)
                            && (conflicting_pressure_checked(
                                &record.filesystem,
                                query,
                                control,
                            )? == Some(Severity::Critical)
                                || conflicting_pressure_checked(
                                    &record.inodes,
                                    query,
                                    control,
                                )? == Some(Severity::Critical)) =>
                    {
                        "Inspect the conflicting node samples; the critical usage check is inconclusive."
                    }
                    "metrics_disagreement" => {
                        "Inspect the conflicting samples and their provenance; the selected observation is uncertain."
                    }
                    "invalid_metrics" => {
                        "Inspect invalid field diagnostics; valid sibling measurements remain available."
                    }
                    _ => {
                        "Inspect observation provenance, freshness, visibility, and collector diagnostics."
                    }
                },
            );
        }
    }
    let mut identities = BTreeMap::new();
    for record in &graph.records {
        control.step()?;
        identities.insert(
            &record.id,
            (record.namespace.known(), &record.name, &record.uid),
        );
    }
    work::sort_by(
        &mut findings,
        |a, b| {
            b.severity
                .cmp(&a.severity)
                .then_with(|| {
                    identities
                        .get(&a.subject_id)
                        .cmp(&identities.get(&b.subject_id))
                })
                .then_with(|| a.code.cmp(&b.code))
        },
        control,
    )?;
    Ok(findings)
}

type MetricNotes = BTreeMap<&'static str, (Severity, Vec<Evidence>)>;

fn family_notes<T: Counter>(family: &MetricFamily<T>, name: &str, notes: &mut MetricNotes) {
    let code = match family.status() {
        "uncertain" => Some(("metrics_disagreement", Severity::Warning)),
        "stale" => Some(("metrics_stale", Severity::Info)),
        "forbidden" | "timeout" | "timestamp_unknown" | "identity_uncertain" => {
            Some(("metrics_unavailable", Severity::Info))
        }
        "missing" => Some((
            if matches!(family, MetricFamily::Unavailable { reason, .. }
                if !matches!(reason.as_str(), "no_sample" | "no_live_consumer"))
            {
                "metrics_unavailable"
            } else {
                "metrics_missing"
            },
            Severity::Info,
        )),
        _ => None,
    };
    if let Some((code, severity)) = code {
        let evidence = &mut notes.entry(code).or_insert((severity, vec![])).1;
        if code == "metrics_disagreement" {
            if let Some((min, max)) = family.observed_range() {
                evidence.push(family_evidence(
                    family,
                    if name == "filesystem" {
                        "observed_range_percent"
                    } else {
                        "inodes.observed_range_percent"
                    },
                    json!(format!("{}..{}", min.percent(), max.percent())),
                    Some("percent"),
                ));
            }
        } else {
            evidence.push(family_evidence(
                family,
                &format!("{name}.status"),
                json!(family.status()),
                None,
            ));
            if let Some(reason) = family.reason() {
                evidence.push(family_evidence(
                    family,
                    &format!("{name}.reason"),
                    json!(reason),
                    None,
                ));
            }
        }
    }
    if has_invalid(family) {
        let evidence = &mut notes
            .entry("invalid_metrics")
            .or_insert((Severity::Warning, vec![]))
            .1;
        evidence.push(family_evidence(
            family,
            &format!("{name}.status"),
            json!(family.status()),
            None,
        ));
        if let Some(candidate) = family.selected() {
            for (field, value) in T::FIELDS.into_iter().zip([
                &candidate.capacity,
                &candidate.used,
                &candidate.available,
            ]) {
                if matches!(value, Field::Invalid) {
                    evidence.push(family_evidence(
                        family,
                        &format!("{name}.{field}.status"),
                        json!("invalid"),
                        None,
                    ));
                }
            }
        }
        if let Some(reason) = family.reason() {
            evidence.push(family_evidence(
                family,
                &format!("{name}.reason"),
                json!(reason),
                None,
            ));
        }
    }
}

fn family_evidence<T: Counter>(
    family: &MetricFamily<T>,
    field: &str,
    value: serde_json::Value,
    unit: Option<&str>,
) -> Evidence {
    Evidence {
        field: field.into(),
        value,
        unit: unit.map(str::to_owned),
        observed_at: family
            .selected()
            .and_then(|candidate| candidate.observed_at),
        source: family.selected().map(|candidate| candidate.source.clone()),
    }
}

fn has_invalid<T: Counter>(family: &MetricFamily<T>) -> bool {
    if matches!(family,MetricFamily::Unavailable{status,..} if status=="invalid") {
        return true;
    }
    family.selected().is_some_and(|c| {
        matches!(
            c.eligibility,
            Eligibility::ClockSkew | Eligibility::InvalidTimestamp
        ) || [&c.capacity, &c.used, &c.available]
            .iter()
            .any(|f| matches!(f, Field::Invalid))
    })
}
fn family_rules<T: Counter>(
    family: &MetricFamily<T>,
    query: &Query,
    codes: [&str; 2],
    fields: [&str; 2],
    unit: &str,
    add: &mut impl FnMut(&str, Severity, Vec<Evidence>, &str),
) {
    let Some(candidate) = family.fresh_unambiguous() else {
        return;
    };
    if let Some(severity) = usage(candidate, query) {
        let mut evidence = vec![
            measurement(
                fields[0],
                candidate.used.value().map(Counter::raw),
                unit,
                candidate,
            ),
            measurement(
                if unit == "bytes" {
                    "capacity_bytes"
                } else {
                    "inodes_total"
                },
                candidate.capacity.value().map(Counter::raw),
                unit,
                candidate,
            ),
        ];
        if let Some(ratio) = candidate.ratio() {
            evidence.insert(
                0,
                Evidence {
                    field: "used_percent".into(),
                    value: ratio
                        .percent()
                        .parse::<serde_json::Number>()
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null),
                    unit: Some("percent".into()),
                    observed_at: candidate.observed_at,
                    source: Some(candidate.source.clone()),
                },
            );
        }
        add(
            codes[0],
            severity,
            evidence,
            "Investigate filesystem pressure and workload growth.",
        );
    }
    if candidate.available.value().is_some_and(|v| v.raw() == 0) {
        add(
            codes[1],
            Severity::Critical,
            vec![measurement(fields[1], Some(0), unit, candidate)],
            "Investigate reported exhaustion; confirm application and storage state.",
        );
    }
}
fn measurement<T: Counter>(
    field: &str,
    value: Option<u64>,
    unit: &str,
    candidate: &Candidate<T>,
) -> Evidence {
    Evidence {
        field: field.into(),
        value: value
            .map(|v| json!(v.to_string()))
            .unwrap_or(serde_json::Value::Null),
        unit: Some(unit.into()),
        observed_at: candidate.observed_at,
        source: Some(candidate.source.clone()),
    }
}
fn plain(field: &str, value: serde_json::Value, unit: Option<&str>) -> Evidence {
    Evidence {
        field: field.into(),
        value,
        unit: unit.map(str::to_owned),
        observed_at: None,
        source: None,
    }
}

#[cfg(test)]
#[path = "../tests/unit/findings.rs"]
mod tests;
