//! Collection cost, timing, and practical next steps without changing report data.
use super::{TextOptions, items};
use crate::work::Checkpoint;
use serde_json::Value;
use std::{collections::BTreeSet, fmt::Write as _};

pub(super) fn collection_cost<C: Checkpoint>(
    out: &mut String,
    report: &Value,
    control: &mut C,
) -> Result<(), C::Error> {
    let cost = &report["collection"]["cost"];
    let count = |field: &str| cost[field].as_u64().map(u128::from).unwrap_or(0);
    let requests = count("base_attempts")
        + count("enrichment_attempts")
        + count("node_requests")
        + count("prometheus_requests");
    let _ = writeln!(
        out,
        "Collection cost: {requests} requests (base {}, enrichment {}/{}, kubelet {}, Prometheus {}); {} bytes read.",
        count("base_attempts"),
        count("enrichment_attempts"),
        count("enrichment_budget"),
        count("node_requests"),
        count("prometheus_requests"),
        count("response_bytes")
    );
    for diagnostic in items(&report["diagnostics"]) {
        control.step()?;
        if diagnostic["reason"] == "budget_exhausted" {
            out.push_str("Enrichment budget exhausted; some optional fields remain unknown.\n");
            break;
        }
    }
    Ok(())
}

pub(super) fn collection_timing(out: &mut String, options: TextOptions) {
    let Some(elapsed) = options.collection_elapsed else {
        return;
    };
    if !options.wide && options.watch_interval.is_none() {
        return;
    }
    let _ = write!(out, "Collection duration: {:.3}s", elapsed.as_secs_f64());
    if let Some(interval) = options
        .watch_interval
        .filter(|interval| elapsed > *interval)
    {
        let _ = write!(
            out,
            " (longer than the {:.3}s refresh interval)",
            interval.as_secs_f64()
        );
    }
    out.push('\n');
}

/// Only confirmed unavailable reasons produce hints. Never infer permissions
/// from an empty result or suggest that absent usage is zero.
pub(crate) fn metric_hints<C: Checkpoint>(
    out: &mut String,
    report: &Value,
    control: &mut C,
) -> Result<(), C::Error> {
    if report["query"]["utilization"] != true {
        return Ok(());
    }
    let prometheus = report["query"]["metrics_source"] == "prometheus";
    let mut hints = BTreeSet::new();
    for family in ["filesystem", "inodes"] {
        if let Some(reasons) = report["summary"][family]["coverage"]["by_reason"].as_object() {
            for reason in reasons.keys() {
                control.step()?;
                if let Some(hint) = metric_hint(reason, prometheus) {
                    hints.insert(hint);
                }
            }
        }
    }
    for hint in hints {
        control.step()?;
        let _ = writeln!(out, "Next: {hint}");
    }
    Ok(())
}

fn metric_hint(reason: &str, prometheus: bool) -> Option<&'static str> {
    match reason {
        "forbidden" if prometheus => {
            Some("Check the Prometheus endpoint's read permissions and its separate credentials.")
        }
        "forbidden" => Some(
            "Check get access to nodes/proxy in the selected context; ask your cluster administrator if access is denied.",
        ),
        "consumers_unknown" => Some(
            "Check pod-list access in the selected namespaces so live consumer nodes can be discovered.",
        ),
        "no_live_consumer" => Some(
            "Usage needs a scheduled active or terminating pod referencing the claim; inspect its consumer and scheduling state.",
        ),
        "max_age_exceeded" => Some(
            "Check CSI statistics refresh and node clocks; increase --max-metrics-age only if older observations are acceptable.",
        ),
        "timeout" | "deadline_exceeded" => Some(
            "Check API/node reachability and retry with appropriate --request-timeout and --timeout values.",
        ),
        "budget_exhausted" => Some(
            "Narrow the namespace or filters to reduce optional lookups within the enrichment budget.",
        ),
        "no_sample" if prometheus => Some(
            "Check kubelet_volume_stats_* series, namespace/claim labels, and the configured Prometheus cluster selector.",
        ),
        "no_sample" => Some(
            "Confirm the claim is mounted and its CSI driver reports volume statistics, then retry after the kubelet statistics refresh.",
        ),
        _ => None,
    }
}
