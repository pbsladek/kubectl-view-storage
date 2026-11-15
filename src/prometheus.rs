//! Explicit Prometheus scope, raw-sample timestamps, and per-claim tuples.
use crate::{
    cli::{Cli, MetricsSource, Query},
    join::Graph,
    metrics,
    model::*,
    qty::{Bytes, Inodes},
    work::{self, Checkpoint},
};
use jiff::Timestamp;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub fn endpoint(input: &str) -> Result<http::Uri, String> {
    let uri: http::Uri = input.parse().map_err(|_| "invalid Prometheus URL")?;
    if !matches!(uri.scheme_str(), Some("http" | "https"))
        || uri.host().is_none()
        || uri.authority().is_some_and(|a| a.as_str().contains('@'))
        || uri.query().is_some()
        || input.contains('#')
    {
        return Err(
            "Prometheus URL requires http(s), a host, and no credentials, query or fragment".into(),
        );
    }
    Ok(uri)
}
fn cluster(input: &str) -> Result<(&str, &str), String> {
    let (label, value) = input
        .split_once('=')
        .ok_or("Prometheus cluster requires LABEL=VALUE")?;
    if label.is_empty()
        || !label
            .bytes()
            .enumerate()
            .all(|(i, b)| b.is_ascii_alphabetic() || b == b'_' || i > 0 && b.is_ascii_digit())
        || matches!(
            label,
            "__name__"
                | "namespace"
                | "persistentvolumeclaim"
                | "pvc_uid"
                | "persistentvolumeclaim_uid"
        )
        || value.is_empty()
        || value.len() > 1024
        || value.chars().any(char::is_control)
    {
        return Err("Prometheus cluster requires an ordinary label and a nonempty value".into());
    }
    Ok((label, value))
}
pub fn validate_options(cli: &Cli) -> Result<(), String> {
    if cli.metrics_source != MetricsSource::Prometheus {
        if cli.prometheus_url.is_some()
            || cli.prometheus_token_file.is_some()
            || cli.prometheus_ca_file.is_some()
            || cli.prometheus_cluster.is_some()
            || cli.prometheus_single_cluster
        {
            return Err("Prometheus options require --metrics-source prometheus".into());
        }
        return Ok(());
    }
    let uri = endpoint(
        cli.prometheus_url
            .as_deref()
            .ok_or("--metrics-source prometheus requires --prometheus-url")?,
    )?;
    if cli.prometheus_cluster.is_none() && !cli.prometheus_single_cluster {
        return Err("select --prometheus-cluster LABEL=VALUE or explicitly assert --prometheus-single-cluster".into());
    }
    if let Some(value) = &cli.prometheus_cluster {
        cluster(value)?;
    }
    if cli.prometheus_token_file.is_some()
        && uri.scheme_str() != Some("https")
        && !matches!(
            uri.host(),
            Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
        )
    {
        return Err(
            "Prometheus bearer tokens require HTTPS (except loopback test endpoints)".into(),
        );
    }
    Ok(())
}

pub(crate) fn query_path<C: Checkpoint>(
    query: &Query,
    control: &mut C,
) -> Result<String, C::Error> {
    let mut selector = String::from(
        "{__name__=~\"kubelet_volume_stats_(capacity_bytes|used_bytes|available_bytes|inodes|inodes_used|inodes_free)\"",
    );
    if let Some(namespace) = &query.namespace {
        selector.push_str(&format!(",namespace={}", json!(namespace)));
    }
    if let Some(value) = &query.prometheus_cluster
        && let Ok((label, value)) = cluster(value)
    {
        selector.push_str(&format!(",{label}={}", json!(value)));
    }
    selector.push_str(&format!(
        "}}[{}ms]",
        query.max_metrics_age.as_millis().saturating_add(30_000)
    ));
    let mut path = String::from("/api/v1/query?query=");
    for byte in selector.bytes() {
        control.step()?;
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            path.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(path, "%{byte:02X}");
        }
    }
    Ok(path)
}

fn sample_time(value: &Value) -> Option<Timestamp> {
    let input = value.as_number()?.to_string();
    if input.len() > 64 {
        return None;
    }
    let (negative, input) = input
        .strip_prefix('-')
        .map(|v| (true, v))
        .unwrap_or((false, input.as_str()));
    let (coefficient, exponent) = input.split_once(['e', 'E']).unwrap_or((input, "0"));
    let exponent: i32 = exponent.parse().ok()?;
    let (whole, fraction) = coefficient.split_once('.').unwrap_or((coefficient, ""));
    let mut amount: u128 = format!("{whole}{fraction}").parse().ok()?;
    let power = 9i32
        .checked_add(exponent)?
        .checked_sub(fraction.len().try_into().ok()?)?;
    if power >= 0 {
        amount = amount.checked_mul(10u128.checked_pow(power.try_into().ok()?)?)?;
    } else {
        amount /= 10u128.checked_pow(power.checked_neg()?.try_into().ok()?)?;
    }
    let amount: i128 = amount.try_into().ok()?;
    Timestamp::from_nanosecond(if negative { -amount } else { amount }).ok()
}
// Prometheus float samples are strings, including integral decimal/scientific
// forms. Require exact u64 values without floating rounding or huge powers.
fn counter_value(raw: &str) -> Option<u64> {
    if raw.len() > 256 {
        return None;
    }
    if let Ok(value) = raw.parse::<u64>() {
        return Some(value);
    }
    let (negative, raw) = raw
        .strip_prefix('-')
        .map(|value| (true, value))
        .unwrap_or((false, raw));
    let (coefficient, exponent) = raw.split_once(['e', 'E']).unwrap_or((raw, "0"));
    let exponent: i32 = exponent.parse().ok()?;
    let (whole, fraction) = coefficient.split_once('.').unwrap_or((coefficient, ""));
    if whole.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let mut value: u128 = format!("{whole}{fraction}").parse().ok()?;
    if negative && value != 0 {
        return None;
    }
    if value == 0 {
        return Some(0);
    }
    let power = exponent.checked_sub(fraction.len().try_into().ok()?)?;
    if power >= 0 {
        value = value.checked_mul(10u128.checked_pow(power.try_into().ok()?)?)?;
    } else {
        let divisor = 10u128.checked_pow(power.checked_neg()?.try_into().ok()?)?;
        if !value.is_multiple_of(divisor) {
            return None;
        }
        value /= divisor;
    }
    value.try_into().ok()
}

const FIELDS: [&str; 6] = [
    "capacity_bytes",
    "used_bytes",
    "available_bytes",
    "inodes",
    "inodes_used",
    "inodes_free",
];

/// Normalize an already bounded Prometheus response without performing I/O.
/// Returns false for an invalid provider envelope while retaining inventory.
pub fn apply(graph: &mut Graph, response: &Value, query: &Query, now: Timestamp) -> bool {
    apply_checked(
        graph,
        Some(response),
        "no_sample",
        query,
        now,
        &mut crate::work::Unbounded,
    )
    .unwrap_or_else(|never| match never {})
}
type TupleKey = (String, String, BTreeMap<String, String>, Timestamp);
type Tuples = BTreeMap<TupleKey, [Value; 6]>;
fn tuples<C: Checkpoint>(response: &Value, control: &mut C) -> Result<Option<Tuples>, C::Error> {
    if response["status"] != "success"
        || response["data"]["resultType"] != "matrix"
        || response["warnings"]
            .as_array()
            .is_some_and(|v| !v.is_empty())
    {
        return Ok(None);
    }
    let Some(series) = response["data"]["result"].as_array() else {
        return Ok(None);
    };
    let mut tuples = BTreeMap::new();
    for series in series {
        control.step()?;
        let Some(metric) = series["metric"].as_object() else {
            return Ok(None);
        };
        let mut labels: BTreeMap<String, String> = BTreeMap::new();
        for (key, value) in metric {
            control.step()?;
            let Some(value) = value.as_str() else {
                return Ok(None);
            };
            if key != "__name__" {
                labels.insert(key.clone(), value.into());
            }
        }
        let (Some(namespace), Some(claim)) =
            (labels.get("namespace"), labels.get("persistentvolumeclaim"))
        else {
            return Ok(None);
        };
        let Some(field) = metric
            .get("__name__")
            .and_then(Value::as_str)
            .and_then(|name| name.strip_prefix("kubelet_volume_stats_"))
            .and_then(|field| FIELDS.iter().position(|name| *name == field))
        else {
            continue;
        };
        let Some(values) = series["values"].as_array() else {
            return Ok(None);
        };
        let mut newest = None;
        for value in values {
            control.step()?;
            let Some(pair) = value.as_array().filter(|pair| pair.len() == 2) else {
                return Ok(None);
            };
            let (Some(time), Some(raw)) = (sample_time(&pair[0]), pair[1].as_str()) else {
                return Ok(None);
            };
            let number = counter_value(raw)
                .map(Value::from)
                .unwrap_or(json!("invalid"));
            match &mut newest {
                Some((old, previous, conflicting)) if time == *old => {
                    *conflicting |= *previous != number
                }
                Some((old, _, _)) if time < *old => {}
                _ => newest = Some((time, number, false)),
            }
        }
        if let Some((time, value, conflicting)) = newest {
            if conflicting {
                return Ok(None);
            }
            let entry = tuples
                .entry((namespace.clone(), claim.clone(), labels, time))
                .or_insert_with(|| std::array::from_fn(|_| Value::Null));
            if entry[field] != Value::Null && entry[field] != value {
                return Ok(None);
            }
            entry[field] = value;
        }
    }
    Ok(Some(tuples))
}

pub(crate) fn apply_checked<C: Checkpoint>(
    graph: &mut Graph,
    response: Option<&Value>,
    failure: &str,
    query: &Query,
    now: Timestamp,
    control: &mut C,
) -> Result<bool, C::Error> {
    let tuples = match response {
        Some(value) => tuples(value, control)?,
        None => None,
    };
    let reason = if response.is_some() {
        "invalid_response"
    } else {
        failure
    };
    let mut index: BTreeMap<(&str, &str), Vec<_>> = BTreeMap::new();
    if let Some(tuples) = &tuples {
        for ((ns, claim, labels, time), values) in tuples {
            control.step()?;
            if let Some(expected) = &query.prometheus_cluster
                && let Ok((label, value)) = cluster(expected)
                && labels.get(label).map(String::as_str) != Some(value)
            {
                continue;
            }
            index
                .entry((ns, claim))
                .or_default()
                .push((labels, *time, values));
        }
    } else if graph
        .records
        .iter()
        .any(|record| record.volume_mode == VolumeMode::Filesystem)
    {
        graph.diagnostics.push(Diagnostic {code:"statistics_unavailable".into(),subject_id:None,source:"prometheus".into(),reason:reason.into(),affected_fields:vec!["filesystem".into(),"inodes".into()],message:"Prometheus samples could not be collected or validated; check the explicit endpoint, credentials, cluster selector and timeouts. No kubelet fallback was attempted.".into()});
    }
    for record in &mut graph.records {
        control.step()?;
        if record.volume_mode == VolumeMode::Block {
            record.filesystem = MetricFamily::NotApplicable;
            record.inodes = MetricFamily::NotApplicable;
            continue;
        }
        if record.volume_mode != VolumeMode::Filesystem {
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
        let mut seen = BTreeSet::new();
        if let (Some(ns), Some(claim)) = (record.namespace.known(), record.pvc_name.known()) {
            let mut samples = index
                .get(&(ns.as_str(), claim.as_str()))
                .cloned()
                .unwrap_or_default();
            work::sort_by(
                &mut samples,
                |a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)),
                control,
            )?;
            let mut clusters = BTreeSet::new();
            for (labels, _, _) in &samples {
                control.step()?;
                if let Some(cluster) = labels.get("cluster") {
                    clusters.insert(cluster.as_str());
                }
            }
            for (labels, time, values) in samples {
                control.step()?;
                let expected_uid = if record.kind == crate::cli::Kind::Pvc {
                    Some(record.uid.as_str())
                } else {
                    record.pvc_uid.as_deref()
                };
                let mismatched_uid = ["persistentvolumeclaim_uid", "pvc_uid"]
                    .iter()
                    .filter_map(|label| labels.get(*label))
                    .any(|uid| Some(uid.as_str()) != expected_uid);
                let eligibility = if record.binding_conflict
                    || record.claim_created_at.is_none()
                    || record
                        .claim_created_at
                        .is_some_and(|created| time < created)
                    || mismatched_uid
                    || clusters.len() > 1
                {
                    Eligibility::IdentityUncertain
                } else if time.as_nanosecond() - now.as_nanosecond() > 30_000_000_000 {
                    Eligibility::ClockSkew
                } else if now.as_nanosecond() - time.as_nanosecond()
                    > query.max_metrics_age.as_nanos() as i128
                {
                    Eligibility::Stale
                } else {
                    Eligibility::Fresh
                };
                let node = labels.get("node").cloned().unwrap_or_default();
                let signature = (
                    node.clone(),
                    values.iter().map(Value::to_string).collect::<Vec<_>>(),
                    eligibility as u8,
                );
                if !seen.insert(signature) {
                    continue;
                }
                let source = Source {
                    provider: MetricsSource::Prometheus,
                    node,
                    pod_id: None,
                    volume_name: String::new(),
                    claim: Some(Box::new(ClaimIdentity {
                        namespace: ns.clone(),
                        name: claim.clone(),
                    })),
                };
                let sample = json!({"capacityBytes":values[0],"usedBytes":values[1],"availableBytes":values[2],"inodes":values[3],"inodesUsed":values[4],"inodesFree":values[5]});
                bytes.push(metrics::normalize_candidate::<Bytes>(
                    &sample,
                    ["capacityBytes", "usedBytes", "availableBytes"],
                    source.clone(),
                    Some(time),
                    eligibility,
                    &record.id,
                    &mut graph.diagnostics,
                ));
                inodes.push(metrics::normalize_candidate::<Inodes>(
                    &sample,
                    ["inodes", "inodesUsed", "inodesFree"],
                    source,
                    Some(time),
                    eligibility,
                    &record.id,
                    &mut graph.diagnostics,
                ));
            }
        }
        let reason = if tuples.is_some() {
            "no_sample"
        } else {
            reason
        };
        record.filesystem = metrics::select_checked(bytes, reason, control)?;
        record.inodes = metrics::select_checked(inodes, reason, control)?;
    }
    Ok(tuples.is_some())
}

pub fn read_configuration_file(path: &std::path::Path, maximum: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| "cannot open Prometheus credential or CA file")?;
    let metadata = file
        .metadata()
        .map_err(|_| "cannot inspect Prometheus credential or CA file")?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err("Prometheus credential and CA inputs must be bounded regular files".into());
    }
    let mut bytes = vec![];
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read Prometheus credential or CA file")?;
    if bytes.len() as u64 > maximum {
        return Err("Prometheus configuration file exceeds its size limit".into());
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "../tests/unit/prometheus.rs"]
mod tests;
