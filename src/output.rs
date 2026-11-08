//! Render a complete report without fetching data or doing storage accounting.
use crate::{
    cli::{Color, Severity, View},
    pipeline::Report,
    work::{Checkpoint, Unbounded},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    io::{self, Write},
};
use tokio_util::sync::CancellationToken;

fn check_cancel(cancel: &CancellationToken) -> io::Result<()> {
    if cancel.is_cancelled() {
        // Write::write_all retries ErrorKind::Interrupted. A cancellation error
        // must terminate that loop instead of causing an endless retry.
        Err(io::Error::other("run interrupted"))
    } else {
        Ok(())
    }
}
struct WriterControl<'a> {
    cancel: &'a CancellationToken,
    remaining: usize,
}
impl Checkpoint for WriterControl<'_> {
    type Error = io::Error;
    fn step(&mut self) -> io::Result<()> {
        self.remaining -= 1;
        if self.remaining == 0 {
            self.remaining = crate::work::CHECK_INTERVAL;
            check_cancel(self.cancel)?;
            std::thread::yield_now();
        }
        Ok(())
    }
}

/// Render on the caller's owned blocking worker. The writer must also honor
/// cancellation while waiting for its destination to accept bytes.
pub fn write_text(
    mut writer: impl Write,
    report: &Report,
    view: View,
    wide: bool,
    color: bool,
    cancel: &CancellationToken,
) -> io::Result<()> {
    check_cancel(cancel)?;
    let rendered = text_checked(
        report,
        view,
        wide,
        color,
        &mut WriterControl {
            cancel,
            remaining: crate::work::CHECK_INTERVAL,
        },
    )?;
    check_cancel(cancel)?;
    writer.write_all(rendered.as_bytes())?;
    writer.flush()
}

/// Visit one flat record at a time without allocating the entire CSV export.
/// The callback supplies the maintained CSV encoder and its fallible writer.
pub fn for_each_csv_row(
    report: &Value,
    cancel: &CancellationToken,
    visit: impl FnMut(Vec<String>) -> io::Result<()>,
) -> io::Result<()> {
    check_cancel(cancel)?;
    visit_csv_rows(
        report,
        &mut WriterControl {
            cancel,
            remaining: crate::work::CHECK_INTERVAL,
        },
        visit,
    )?;
    check_cancel(cancel)
}

pub const CSV_HEADER: [&str; 27] = [
    "kind",
    "namespace",
    "name",
    "uid",
    "phase",
    "storage_class",
    "driver",
    "volume_mode",
    "pv_name",
    "requested_bytes",
    "bound_bytes",
    "provisioned_bytes",
    "filesystem_capacity_bytes",
    "used_bytes",
    "available_bytes",
    "used_percent",
    "inodes_total",
    "inodes_used",
    "inodes_free",
    "inode_percent",
    "metrics_status",
    "metrics_reason",
    "metrics_source",
    "observed_at",
    "active_consumer_count",
    "terminating_consumer_count",
    "severity",
];

pub fn write_json(mut writer: impl Write, report: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut writer, report).map_err(|error| {
        io::Error::new(
            error.io_error_kind().unwrap_or(io::ErrorKind::InvalidData),
            error,
        )
    })?;
    writer.write_all(b"\n")?;
    writer.flush()
}

pub fn use_color(when: Color, is_terminal: bool, no_color: bool) -> bool {
    match when {
        Color::Always => true,
        Color::Never => false,
        Color::Auto => is_terminal && !no_color,
    }
}

pub fn escape(input: &str) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control()
                || matches!(ch,'\u{2028}'|'\u{2029}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}') =>
            {
                let _ = write!(out, "\\u{{{:x}}}", ch as u32);
            }
            _ => out.push(ch),
        }
    }
    out
}
fn cell(value: &Value) -> String {
    if value.is_null() {
        "--".into()
    } else if let Some(s) = value.as_str() {
        escape(s)
    } else {
        escape(&value.to_string())
    }
}
fn items(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}
fn iec(value: &Value) -> String {
    let Some(bytes) = value.as_str().and_then(|s| s.parse::<u128>().ok()) else {
        return "--".into();
    };
    const UNITS: [&str; 9] = ["", "Ki", "Mi", "Gi", "Ti", "Pi", "Ei", "Zi", "Yi"];
    let mut unit = 1u128;
    let mut index = 0;
    while index + 1 < UNITS.len() && bytes / unit >= 1024 {
        unit *= 1024;
        index += 1;
    }
    if index == 0 {
        return bytes.to_string();
    }
    let scaled = (bytes / unit) * 100 + ((bytes % unit) * 100 + unit / 2) / unit;
    let fraction = scaled % 100;
    if fraction == 0 {
        format!("{}{}", scaled / 100, UNITS[index])
    } else {
        format!("{}.{:02}{}", scaled / 100, fraction, UNITS[index])
            .replace(&format!("0{}", UNITS[index]), UNITS[index])
    }
}
fn percent(value: &Value) -> String {
    let Some(value) = value.as_f64() else {
        return "--".into();
    };
    let mut value = format!("{value:.6}");
    while value.ends_with('0') && !value.ends_with(".0") {
        value.pop();
    }
    format!("{value}%")
}
fn quantity(value: &Value) -> String {
    let formatted =
        if value["known_count"] == 0 && value["total_count"].as_u64().is_some_and(|n| n > 0) {
            "--".into()
        } else {
            iec(&value["known_sum_bytes"])
        };
    if value["complete"] == false {
        format!("{formatted} (partial)")
    } else {
        formatted
    }
}
fn metric_cells(metric: &Value) -> String {
    if metric["status"] == "not_applicable" {
        return "n/a n/a n/a n/a".into();
    }
    format!(
        "{} {} {} {}",
        iec(&metric["capacity_bytes"]),
        iec(&metric["used_bytes"]),
        iec(&metric["available_bytes"]),
        percent(&metric["used_percent"])
    )
}
fn inventory_cells(totals: &Value, metric: &Value, utilization: bool) -> String {
    // For an empty, incomplete selection, mark the inventory subtotal row once
    // on its last sum. The selection footer explains that every sum is a bound.
    let requested = if totals["record_count"] == 0 && totals["selection_complete"] == false {
        iec(&totals["requested"]["known_sum_bytes"])
    } else {
        quantity(&totals["requested"])
    };
    let mut out = format!(
        "{} {} {}",
        cell(&totals["record_count"]),
        requested,
        quantity(&totals["provisioned"])
    );
    if utilization {
        let _ = write!(out, " {}", metric_cells(metric));
    }
    out
}

pub fn text(report: &Report, view: View, wide: bool, color: bool) -> String {
    text_checked(report, view, wide, color, &mut Unbounded).unwrap_or_else(|never| match never {})
}
fn text_checked<C: Checkpoint>(
    report: &Report,
    view: View,
    wide: bool,
    color: bool,
    control: &mut C,
) -> Result<String, C::Error> {
    let value = &report.document;
    let utilization = value["query"]["utilization"] == true;
    let mut out = format!(
        "CONTEXT {}   SCOPE {}   KIND {}\n",
        cell(&value["collection"]["context"]),
        if value["collection"]["scope"]["all_namespaces"] == true {
            "all namespaces".into()
        } else {
            cell(&value["collection"]["scope"]["namespace"])
        },
        cell(&value["query"]["kind"])
    );
    let mut records = BTreeMap::new();
    for record in items(&value["records"]) {
        control.step()?;
        if let Some(id) = record["id"].as_str() {
            records.insert(id, record);
        }
    }
    match view {
        View::Tree => {
            out.push_str(if utilization {
                "STORAGE RECORDS REQUESTED PROVISIONED FS-CAP USED AVAIL USE%\n"
            } else {
                "STORAGE RECORDS REQUESTED PROVISIONED\n"
            });
            for group in items(&value["groups"]) {
                tree(&mut out, group, 0, &records, utilization, control)?;
            }
            if records.is_empty() {
                empty(&mut out, value);
            }
        }
        View::Table => {
            let _ = writeln!(
                out,
                "{} REQUESTED PROVISIONED{} STATE",
                cell(&value["query"]["kind"]).to_uppercase(),
                if utilization {
                    " FS-CAP USED AVAIL USE% INODE%"
                } else {
                    ""
                }
            );
            for record in items(&value["records"]) {
                control.step()?;
                let _ = write!(
                    out,
                    "{} {} {}",
                    cell(&record["name"]),
                    iec(&record["requested_bytes"]),
                    iec(&record["provisioned_bytes"])
                );
                if utilization {
                    let _ = write!(
                        out,
                        " {} {}",
                        metric_cells(&record["filesystem"]),
                        if record["inodes"]["status"] == "not_applicable" {
                            "n/a".into()
                        } else {
                            percent(&record["inodes"]["used_percent"])
                        }
                    );
                }
                let _ = write!(out, " {}", cell(&record["phase"]));
                if utilization {
                    let _ = write!(out, " [metrics:{}]", cell(&record["filesystem"]["status"]));
                }
                out.push('\n');
            }
            if records.is_empty() {
                empty(&mut out, value);
            }
        }
        View::Summary => {
            out.push_str(if utilization {
                "GROUP RECORDS REQUESTED PROVISIONED FS-CAP USED AVAIL USE% METRICS\n"
            } else {
                "GROUP RECORDS REQUESTED PROVISIONED\n"
            });
            summary_row(&mut out, "ROOT", &value["summary"], utilization, control)?;
            for group in items(&value["groups"]) {
                control.step()?;
                summary_row(
                    &mut out,
                    &group_label(group),
                    &group["totals"],
                    utilization,
                    control,
                )?;
            }
        }
        View::Issues => {
            out.push_str("SEVERITY OBJECT CODE EVIDENCE\n");
            for finding in items(&value["findings"]) {
                control.step()?;
                let record = finding["subject_id"]
                    .as_str()
                    .and_then(|id| records.get(id))
                    .copied()
                    .unwrap_or(&Value::Null);
                let severity = cell(&finding["severity"]);
                let severity = if color {
                    format!(
                        "\u{1b}[{}m{severity}\u{1b}[0m",
                        if severity == "critical" {
                            "31"
                        } else if severity == "warning" {
                            "33"
                        } else {
                            "36"
                        }
                    )
                } else {
                    severity
                };
                let mut evidence = String::new();
                for item in items(&finding["evidence"]) {
                    control.step()?;
                    if !evidence.is_empty() {
                        evidence.push(' ');
                    }
                    let _ = write!(
                        evidence,
                        "{}={}",
                        cell(&item["field"]),
                        cell(&item["value"])
                    );
                }
                let _ = writeln!(
                    out,
                    "{severity} {} {} {evidence}",
                    record_identity(record),
                    cell(&finding["code"])
                );
                let _ = writeln!(out, "  Investigate: {}", cell(&finding["suggested_action"]));
            }
            if items(&value["findings"]).next().is_none() {
                out.push_str(if value["selection"]["complete"] == true {
                    "No observed findings in selected records.\n"
                } else {
                    "No observed findings among confirmed matches; selection is incomplete.\n"
                });
            }
        }
    }
    if wide {
        declared(&mut out, value, control)?;
    }
    footer(&mut out, report, utilization, control)?;
    Ok(out)
}
fn record_identity(record: &Value) -> String {
    if record["namespace"].is_null() {
        cell(&record["name"])
    } else {
        format!("{}/{}", cell(&record["namespace"]), cell(&record["name"]))
    }
}
fn empty(out: &mut String, report: &Value) {
    out.push_str(if report["selection"]["complete"] == true {
        "No matching resources.\n"
    } else {
        "No confirmed matches; query membership is unresolved.\n"
    });
}
fn group_label(group: &Value) -> String {
    let label = if group["value_state"] == "known" {
        cell(&group["value"])
    } else if group["value_state"] == "unknown" {
        format!("(unknown {})", cell(&group["dimension"]))
    } else if group["value_state"] == "unassigned" {
        "(unassigned storage class)".into()
    } else if group["dimension"] == "storageclass" {
        "(classless)".into()
    } else if group["dimension"] == "pod" || group["dimension"] == "node" {
        "(no live consumer)".into()
    } else {
        format!("(no {})", cell(&group["dimension"]))
    };
    format!("{}={label}", cell(&group["dimension"]))
}
fn tree<C: Checkpoint>(
    out: &mut String,
    group: &Value,
    depth: usize,
    records: &BTreeMap<&str, &Value>,
    utilization: bool,
    control: &mut C,
) -> Result<(), C::Error> {
    control.step()?;
    let leaf = items(&group["children"]).next().is_none();
    let mut members = vec![];
    for id in items(&group["member_ids"]) {
        control.step()?;
        if let Some(record) = id.as_str().and_then(|id| records.get(id)).copied() {
            members.push(record);
        }
    }
    let identifies =
        leaf && members.len() == 1 && (group["dimension"] == "pvc" || group["dimension"] == "pv");
    let metric = if identifies {
        &members[0]["filesystem"]
    } else {
        &group["totals"]["filesystem"]
    };
    let _ = write!(
        out,
        "{}{} {}",
        "  ".repeat(depth),
        group_label(group),
        inventory_cells(&group["totals"], metric, utilization)
    );
    if utilization {
        let _ = write!(out, " [metrics:{}]", cell(&metric["status"]));
    }
    out.push('\n');
    if leaf && !identifies {
        for record in members {
            control.step()?;
            let _ = write!(
                out,
                "{}{} {} {}",
                "  ".repeat(depth + 1),
                record_identity(record),
                iec(&record["requested_bytes"]),
                iec(&record["provisioned_bytes"])
            );
            if utilization {
                let _ = write!(
                    out,
                    " {} [metrics:{}]",
                    metric_cells(&record["filesystem"]),
                    cell(&record["filesystem"]["status"])
                );
            }
            out.push('\n');
        }
    }
    for child in items(&group["children"]) {
        tree(out, child, depth + 1, records, utilization, control)?;
    }
    Ok(())
}
fn summary_row<C: Checkpoint>(
    out: &mut String,
    label: &str,
    totals: &Value,
    utilization: bool,
    control: &mut C,
) -> Result<(), C::Error> {
    let _ = write!(
        out,
        "{label} {}",
        inventory_cells(totals, &totals["filesystem"], utilization)
    );
    if utilization {
        let _ = write!(
            out,
            " {}/{}",
            cell(&totals["filesystem"]["coverage"]["fresh_complete"]),
            cell(&totals["filesystem"]["coverage"]["eligible"])
        );
    }
    let mut phases = String::from("{");
    if let Some(counts) = totals["phase_counts"].as_object() {
        for (index, (phase, count)) in counts.iter().enumerate() {
            control.step()?;
            if index > 0 {
                phases.push(',');
            }
            let _ = write!(phases, "{}:{}", Value::String(phase.clone()), count);
        }
    }
    phases.push('}');
    let _ = writeln!(out, " phases={}", escape(&phases));
    Ok(())
}
fn joined_cells<C: Checkpoint>(
    values: &Value,
    separator: &str,
    control: &mut C,
) -> Result<String, C::Error> {
    let mut result = String::new();
    for (index, item) in items(values).enumerate() {
        control.step()?;
        if index > 0 {
            result.push_str(separator);
        }
        result.push_str(&cell(item));
    }
    Ok(result)
}
fn declared<C: Checkpoint>(
    out: &mut String,
    report: &Value,
    control: &mut C,
) -> Result<(), C::Error> {
    out.push_str("DECLARED CONSUMERS (runtime unverified)\n");
    let mut consumers = BTreeMap::new();
    for consumer in items(&report["consumers"]) {
        control.step()?;
        if let Some(id) = consumer["id"].as_str() {
            consumers.insert(id, consumer);
        }
    }
    let mut references: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for edge in items(&report["relationships"]) {
        control.step()?;
        if let Some(id) = edge["record_id"].as_str() {
            references.entry(id).or_default().push(edge);
        }
    }
    for record in items(&report["records"]) {
        control.step()?;
        let edges = record["id"]
            .as_str()
            .and_then(|id| references.get(id))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let access = joined_cells(&record["access_modes"], ",", control)?;
        for edge in edges
            .iter()
            .copied()
            .map(Some)
            .chain(edges.is_empty().then_some(None))
        {
            control.step()?;
            let _ = writeln!(
                out,
                "  {} pv={} driver={} mode={} access={} reclaim={}",
                record_identity(record),
                cell(&record["pv_name"]),
                cell(&record["driver"]),
                cell(&record["volume_mode"]),
                if access.is_empty() { "--" } else { &access },
                cell(&record["reclaim_policy"])
            );
            let Some(edge) = edge else {
                continue;
            };
            let consumer = edge["consumer_id"]
                .as_str()
                .and_then(|id| consumers.get(id))
                .copied()
                .unwrap_or(&Value::Null);
            if items(&edge["mounts"]).next().is_none() {
                let _ = writeln!(
                    out,
                    "    pod={} state={} node={} volumes={} (no declared container mapping)",
                    cell(&consumer["name"]),
                    cell(&consumer["reference_state"]),
                    cell(&consumer["node"]),
                    joined_cells(&edge["volume_names"], ",", control)?
                );
            }
            for mount in items(&edge["mounts"]) {
                control.step()?;
                let container = match mount["container_kind"].as_str() {
                    Some("init") => "init-container",
                    Some("ephemeral") => "ephemeral-container",
                    _ => "container",
                };
                let path = if mount["kind"] == "block" {
                    "device"
                } else {
                    "mount"
                };
                let _ = writeln!(
                    out,
                    "    pod={} state={} node={} {container}={} volume={} {path}={} readOnly={} subPath={} subPathExpr={}",
                    cell(&consumer["name"]),
                    cell(&consumer["reference_state"]),
                    cell(&consumer["node"]),
                    cell(&mount["container"]),
                    cell(&mount["volume_name"]),
                    cell(&mount["path"]),
                    cell(&mount["read_only"]),
                    cell(&mount["sub_path"]),
                    cell(&mount["sub_path_expr"])
                );
            }
        }
    }
    Ok(())
}
fn footer<C: Checkpoint>(
    out: &mut String,
    report: &Report,
    utilization: bool,
    control: &mut C,
) -> Result<(), C::Error> {
    let value = &report.document;
    let _ = writeln!(
        out,
        "SELECTION {}; matched={} unknown={}",
        if value["selection"]["complete"] == true {
            "complete"
        } else {
            "incomplete"
        },
        cell(&value["selection"]["matched_count"]),
        cell(&value["selection"]["unknown_count"])
    );
    if utilization {
        let _ = writeln!(
            out,
            "METRICS {}/{} known-match eligible claims fresh and complete",
            cell(&value["summary"]["filesystem"]["coverage"]["fresh_complete"]),
            cell(&value["summary"]["filesystem"]["coverage"]["eligible"])
        );
    } else {
        out.push_str("metrics: not requested\n");
    }
    if value["selection"]["complete"] == false {
        out.push_str(
            if value["query"]["require_metrics"] == true || !value["query"]["fail_on"].is_null() {
                "Counts are lower bounds; strict check inconclusive.\n"
            } else {
                "Counts are lower bounds.\n"
            },
        );
    }
    if let Some(level) = report.strict_conflict {
        let level = match level {
            Severity::Critical => "critical",
            Severity::Warning => "warning",
            Severity::Info => "info",
        };
        let _ = writeln!(
            out,
            "Strict {level} check inconclusive: conflicting fresh candidates report {level} pressure."
        );
    }
    if value["summary"]["overlapping_children"] == true || has_overlap(&value["groups"], control)? {
        out.push_str("Shared references: sibling group totals cannot be added; root totals use distinct records.\n");
    }
    for diagnostic in items(&value["diagnostics"]) {
        control.step()?;
        let _ = writeln!(
            out,
            "DIAGNOSTIC {}: {}",
            cell(&diagnostic["reason"]),
            cell(&diagnostic["message"])
        );
    }
    if utilization {
        let mut overlapping = false;
        for record in items(&value["records"]) {
            control.step()?;
            if record["overlapping_filesystem"] == true {
                overlapping = true;
                break;
            }
        }
        if overlapping {
            out.push_str("Known shared filesystems are excluded from additive metrics coverage and totals; individual observations remain visible.\n");
        }
        out.push_str(
            "FILESYSTEM TOTALS per-claim reported filesystem totals; physical_total=false\n",
        );
    }
    Ok(())
}
fn has_overlap<C: Checkpoint>(groups: &Value, control: &mut C) -> Result<bool, C::Error> {
    for group in items(groups) {
        control.step()?;
        if group["overlapping_children"] == true || has_overlap(&group["children"], control)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn csv_rows(report: &Value) -> Vec<Vec<String>> {
    let mut rows = vec![];
    visit_csv_rows(report, &mut Unbounded, |row| {
        rows.push(row);
        Ok(())
    })
    .unwrap_or_else(|never| match never {});
    rows
}
fn visit_csv_rows<C: Checkpoint>(
    report: &Value,
    control: &mut C,
    mut visit: impl FnMut(Vec<String>) -> Result<(), C::Error>,
) -> Result<(), C::Error> {
    let mut severity = BTreeMap::new();
    for finding in items(&report["findings"]) {
        control.step()?;
        let score = match finding["severity"].as_str() {
            Some("critical") => 3,
            Some("warning") => 2,
            Some("info") => 1,
            _ => 0,
        };
        if let Some(id) = finding["subject_id"].as_str() {
            severity
                .entry(id)
                .and_modify(|value: &mut u8| *value = (*value).max(score))
                .or_insert(score);
        }
    }
    for record in items(&report["records"]) {
        control.step()?;
        let mut row = [
            "kind",
            "namespace",
            "name",
            "uid",
            "phase",
            "storage_class",
            "driver",
            "volume_mode",
            "pv_name",
            "requested_bytes",
            "bound_bytes",
            "provisioned_bytes",
        ]
        .into_iter()
        .map(|key| csv_cell(&record[key]))
        .collect::<Vec<_>>();
        for (family, key) in [
            ("filesystem", "capacity_bytes"),
            ("filesystem", "used_bytes"),
            ("filesystem", "available_bytes"),
            ("filesystem", "used_percent"),
            ("inodes", "inodes_total"),
            ("inodes", "inodes_used"),
            ("inodes", "inodes_free"),
            ("inodes", "used_percent"),
            ("filesystem", "status"),
            ("filesystem", "reason"),
        ] {
            row.push(csv_cell(&record[family][key]));
        }
        row.push(csv_cell(&record["filesystem"]["source"]["provider"]));
        row.push(csv_cell(&record["filesystem"]["observed_at"]));
        row.push(csv_cell(&record["active_consumer_count"]));
        row.push(csv_cell(&record["terminating_consumer_count"]));
        row.push(
            match record["id"].as_str().and_then(|id| severity.get(id)) {
                Some(3) => "critical",
                Some(2) => "warning",
                Some(1) => "info",
                _ => "",
            }
            .into(),
        );
        visit(row)?;
    }
    Ok(())
}
fn csv_cell(value: &Value) -> String {
    if value.is_null() {
        String::new()
    } else if let Some(s) = value.as_str() {
        s.to_owned()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
#[path = "../tests/unit/output.rs"]
mod tests;
