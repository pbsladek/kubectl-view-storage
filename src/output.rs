//! Render a complete report without fetching data or doing storage accounting.
use crate::{
    calculations::Unit,
    cli::{Color, Severity, View},
    pipeline::Report,
    work::{Checkpoint, Unbounded},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    io::{self, Write},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

mod details;
mod statistics;
mod views;
use statistics::statistics_text;
use views::{issues_view, summary_view, table_view, tree_view};

/// Presentation choices stay outside the machine-readable report.
#[derive(Debug, Clone, Copy)]
pub struct TextOptions {
    pub view: View,
    pub wide: bool,
    pub color: bool,
    pub units: Unit,
    pub clear: bool,
    pub collection_elapsed: Option<Duration>,
    pub watch_interval: Option<Duration>,
}
impl TextOptions {
    pub fn new(view: View, wide: bool, color: bool) -> Self {
        Self {
            view,
            wide,
            color,
            units: Unit::Auto,
            clear: false,
            collection_elapsed: None,
            watch_interval: None,
        }
    }
}

struct TextContext<'a> {
    records: BTreeMap<&'a str, &'a Value>,
    utilization: bool,
    units: Unit,
    color: bool,
}

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
    writer: impl Write,
    report: &Report,
    view: View,
    wide: bool,
    color: bool,
    cancel: &CancellationToken,
) -> io::Result<()> {
    write_text_refresh(writer, report, view, wide, color, cancel, false)
}

pub fn write_text_refresh(
    writer: impl Write,
    report: &Report,
    view: View,
    wide: bool,
    color: bool,
    cancel: &CancellationToken,
    clear: bool,
) -> io::Result<()> {
    let mut options = TextOptions::new(view, wide, color);
    options.clear = clear;
    write_text_options(writer, report, options, cancel)
}

pub fn write_text_options(
    mut writer: impl Write,
    report: &Report,
    options: TextOptions,
    cancel: &CancellationToken,
) -> io::Result<()> {
    check_cancel(cancel)?;
    let rendered = render_checked(
        report,
        options,
        &mut WriterControl {
            cancel,
            remaining: crate::work::CHECK_INTERVAL,
        },
    )?;
    check_cancel(cancel)?;
    if options.clear {
        writer.write_all(b"\x1b[2J\x1b[H")?;
    }
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

pub fn metric_guidance(report: &Value, cancel: &CancellationToken) -> io::Result<String> {
    check_cancel(cancel)?;
    let mut hints = String::new();
    details::metric_hints(
        &mut hints,
        report,
        &mut WriterControl {
            cancel,
            remaining: crate::work::CHECK_INTERVAL,
        },
    )?;
    check_cancel(cancel)?;
    Ok(hints)
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
    const UNITS: [&str; 9] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB", "ZiB", "YiB"];
    let (mut unit, mut index) = (1u128, 0usize);
    while index + 1 < UNITS.len() && bytes / unit >= 1024 {
        unit *= 1024;
        index += 1;
    }
    if index == 0 {
        return format!("{bytes} B");
    }
    let mut scaled = (bytes / unit) * 10 + ((bytes % unit) * 10 + unit / 2) / unit;
    if scaled == 10240 && index + 1 < UNITS.len() {
        scaled = 10;
        index += 1;
    }
    if scaled % 10 == 0 {
        format!("{} {}", scaled / 10, UNITS[index])
    } else {
        format!("{}.{:01} {}", scaled / 10, scaled % 10, UNITS[index])
    }
}
fn amount(value: &Value, unit: Unit) -> String {
    if unit == Unit::Auto {
        iec(value)
    } else {
        calculated_amount(value, unit)
    }
}

fn percent(value: &Value) -> String {
    value
        .as_f64()
        .map(|value| format!("{value:.1}%"))
        .unwrap_or_else(|| "--".into())
}
fn quantity(value: &Value, units: Unit) -> String {
    let formatted =
        if value["known_count"] == 0 && value["total_count"].as_u64().is_some_and(|n| n > 0) {
            "--".into()
        } else {
            amount(&value["known_sum_bytes"], units)
        };
    if value["complete"] == false {
        format!("{formatted}*")
    } else {
        formatted
    }
}

/// Two passes keep numeric columns aligned without introducing a table crate.
/// Both passes cooperate with the same cancellation owner as rendering.
struct TextTable {
    columns: Vec<(&'static str, bool)>,
    rows: Vec<Vec<String>>,
}
impl TextTable {
    fn new(columns: Vec<(&'static str, bool)>) -> Self {
        Self {
            columns,
            rows: vec![],
        }
    }
    fn draw<C: Checkpoint>(&self, out: &mut String, control: &mut C) -> Result<(), C::Error> {
        let mut widths: Vec<_> = self.columns.iter().map(|(label, _)| label.len()).collect();
        for row in &self.rows {
            for (index, value) in row.iter().enumerate() {
                control.step()?;
                widths[index] = widths[index].max(value.chars().count());
            }
        }
        for row in std::iter::once(
            self.columns
                .iter()
                .map(|(label, _)| label.to_string())
                .collect::<Vec<_>>(),
        )
        .chain(std::iter::once(
            widths.iter().map(|width| "-".repeat(*width)).collect(),
        ))
        .chain(self.rows.iter().cloned())
        {
            let mut line = String::new();
            for (index, value) in row.iter().enumerate() {
                control.step()?;
                if index > 0 {
                    line.push_str("  ");
                }
                let padding = widths[index].saturating_sub(value.chars().count());
                if self.columns[index].1 {
                    line.push_str(&" ".repeat(padding));
                }
                line.push_str(value);
                if !self.columns[index].1 && index + 1 < row.len() {
                    line.push_str(&" ".repeat(padding));
                }
            }
            let _ = writeln!(out, "{}", line.trim_end());
        }
        Ok(())
    }
}
fn pod_count(record: &Value) -> String {
    match (
        record["active_consumer_count"].as_u64(),
        record["terminating_consumer_count"].as_u64(),
    ) {
        (Some(active), Some(0)) => active.to_string(),
        (Some(active), Some(deleting)) => format!("{active}+{deleting}d"),
        _ => "--".into(),
    }
}
fn inode_percent(metric: &Value) -> String {
    if metric["status"] == "not_applicable" {
        return "n/a".into();
    }
    let value = percent(&metric["used_percent"]);
    if metric["status"] == "ok" || value == "--" {
        value
    } else {
        format!("{value} ({})", metric_status(metric).to_lowercase())
    }
}
fn metric_cells(metric: &Value, units: Unit) -> Vec<String> {
    if metric["status"] == "not_applicable" {
        return vec!["n/a".into(); 4];
    }
    vec![
        amount(&metric["capacity_bytes"], units),
        amount(&metric["used_bytes"], units),
        amount(&metric["available_bytes"], units),
        percent(&metric["used_percent"]),
    ]
}
fn metric_status(metric: &Value) -> String {
    let label = match metric["status"].as_str() {
        Some("ok") => "Fresh",
        Some("partial") => "Partial",
        Some("not_applicable") => "n/a",
        Some("not_requested") => "Not collected",
        Some("forbidden") => "Access denied",
        Some("missing") if metric["reason"] == "no_live_consumer" => "No consumer",
        Some("missing") => "No sample",
        Some("uncertain") => "Conflicting",
        Some("identity_uncertain") => "Unverified",
        Some("timestamp_unknown") => "No timestamp",
        Some("stale") => "Stale",
        Some("timeout") => "Timed out",
        Some("invalid") => "Invalid",
        Some("unavailable") if metric["reason"] == "time_skew" => "Time skew",
        Some("unavailable") if metric["reason"] == "overlapping_filesystem" => "Shared storage",
        _ => "Unavailable",
    };
    label.into()
}
fn inventory_cells(totals: &Value, metric: &Value, context: &TextContext<'_>) -> Vec<String> {
    let mut row = vec![
        cell(&totals["record_count"]),
        quantity(&totals["requested"], context.units),
        quantity(&totals["provisioned"], context.units),
    ];
    if context.utilization {
        row.extend(metric_cells(metric, context.units));
        row.push(metric_status(metric));
    }
    row
}
fn group_table(first: &'static str, utilization: bool) -> TextTable {
    let mut columns = vec![
        (first, false),
        ("COUNT", true),
        ("REQUESTED", true),
        ("PROVISIONED", true),
    ];
    if utilization {
        columns.extend([
            ("FS SIZE", true),
            ("USED", true),
            ("AVAILABLE", true),
            ("USE%", true),
            ("METRICS", false),
        ]);
    }
    TextTable::new(columns)
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
    render_checked(report, TextOptions::new(view, wide, color), control)
}

pub fn text_with_options(report: &Report, options: TextOptions) -> String {
    render_checked(report, options, &mut Unbounded).unwrap_or_else(|never| match never {})
}

fn render_checked<C: Checkpoint>(
    report: &Report,
    options: TextOptions,
    control: &mut C,
) -> Result<String, C::Error> {
    let TextOptions {
        view,
        wide,
        color,
        units,
        ..
    } = options;
    let value = &report.document;
    let utilization = value["query"]["utilization"] == true;
    let mut out = format!(
        "{}\nContext: {}  |  {}\n\n",
        if value["query"]["kind"] == "pv" {
            "Persistent volumes"
        } else {
            "Persistent volume claims"
        },
        cell(&value["collection"]["context"]),
        if value["collection"]["scope"]["all_namespaces"] == true {
            "Namespaces: all".into()
        } else {
            format!(
                "Namespace: {}",
                cell(&value["collection"]["scope"]["namespace"])
            )
        }
    );
    let mut records = BTreeMap::new();
    if let Some(cycle) = value.get("watch") {
        let _ = writeln!(
            out,
            "Collection cycle {}: {} to {} (refresh every {}s)\n",
            cell(&cycle["cycle"]),
            cell(&value["collection"]["started_at"]),
            cell(&value["collection"]["finished_at"]),
            cell(&cycle["interval_seconds"])
        );
    }
    for record in items(&value["records"]) {
        control.step()?;
        if let Some(id) = record["id"].as_str() {
            records.insert(id, record);
        }
    }
    let context = TextContext {
        records,
        utilization,
        units,
        color,
    };
    render_view(&mut out, report, view, wide, &context, control)?;
    if wide {
        declared(&mut out, value, control)?;
    }
    footer(&mut out, report, utilization, control)?;
    if wide {
        details::collection_cost(&mut out, value, control)?;
    }
    details::collection_timing(&mut out, options);
    details::metric_hints(&mut out, value, control)?;
    Ok(out)
}
fn render_view<C: Checkpoint>(
    out: &mut String,
    report: &Report,
    view: View,
    wide: bool,
    context: &TextContext<'_>,
    control: &mut C,
) -> Result<(), C::Error> {
    let value = &report.document;
    match view {
        View::Stats => statistics_text(out, value, context.utilization, wide, control),
        View::Tree => tree_view(out, value, context, control),
        View::Table => table_view(out, value, context, control),
        View::Summary => summary_view(out, value, context, control),
        View::Issues => issues_view(out, value, context, control),
    }
}

fn human_label(value: &Value) -> String {
    let mut label = cell(value).replace(['_', '.'], " ");
    if let Some(first) = label.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    label
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
        let raw = cell(&group["value"]);
        if matches!(
            group["dimension"].as_str(),
            Some("pod" | "statefulset" | "workload")
        ) {
            raw.rsplit_once('@')
                .map(|(name, _)| name.to_owned())
                .unwrap_or(raw)
        } else {
            raw
        }
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
    let dimension = match group["dimension"].as_str() {
        Some("storageclass") => "Class".into(),
        Some("pvc") => "PVC".into(),
        Some("pv") => "PV".into(),
        _ => human_label(&group["dimension"]),
    };
    format!("{dimension}: {label}")
}
fn tree<C: Checkpoint>(
    table: &mut TextTable,
    group: &Value,
    prefix: &str,
    branch: &str,
    context: &TextContext<'_>,
    control: &mut C,
) -> Result<(), C::Error> {
    control.step()?;
    let children: Vec<_> = items(&group["children"]).collect();
    let mut members = vec![];
    for id in items(&group["member_ids"]) {
        control.step()?;
        if let Some(record) = id.as_str().and_then(|id| context.records.get(id)).copied() {
            members.push(record);
        }
    }
    let identifies = children.is_empty()
        && members.len() == 1
        && (group["dimension"] == "pvc" || group["dimension"] == "pv");
    let metric = if identifies {
        &members[0]["filesystem"]
    } else {
        &group["totals"]["filesystem"]
    };
    let mut row = vec![format!("{prefix}{branch}{}", group_label(group))];
    row.extend(inventory_cells(&group["totals"], metric, context));
    table.rows.push(row);
    let below = format!(
        "{prefix}{}",
        if branch == "|- " {
            "|  "
        } else if branch.is_empty() {
            ""
        } else {
            "   "
        }
    );
    if children.is_empty() && !identifies {
        for (index, record) in members.iter().enumerate() {
            control.step()?;
            let mut row = vec![
                format!(
                    "{below}{}{}",
                    if index + 1 == members.len() {
                        "`- "
                    } else {
                        "|- "
                    },
                    record_identity(record)
                ),
                "1".into(),
                amount(&record["requested_bytes"], context.units),
                amount(&record["provisioned_bytes"], context.units),
            ];
            if context.utilization {
                row.extend(metric_cells(&record["filesystem"], context.units));
                row.push(metric_status(&record["filesystem"]));
            }
            table.rows.push(row);
        }
    }
    for (index, child) in children.iter().enumerate() {
        tree(
            table,
            child,
            &below,
            if index + 1 == children.len() {
                "`- "
            } else {
                "|- "
            },
            context,
            control,
        )?;
    }
    Ok(())
}
fn summary_row<C: Checkpoint>(
    table: &mut TextTable,
    label: &str,
    totals: &Value,
    context: &TextContext<'_>,
    control: &mut C,
) -> Result<(), C::Error> {
    let mut row = vec![label.into()];
    row.extend(inventory_cells(totals, &totals["filesystem"], context));
    if context.utilization {
        let metric = &totals["filesystem"];
        if let Some(status) = row.last_mut() {
            *status = format!(
                "{}/{} {}",
                cell(&metric["coverage"]["fresh_complete"]),
                cell(&metric["coverage"]["eligible"]),
                metric_status(metric)
            );
        }
    }
    let mut phases = vec![];
    if let Some(counts) = totals["phase_counts"].as_object() {
        for (phase, count) in counts {
            control.step()?;
            phases.push(format!("{} {}", cell(count), escape(phase)));
        }
    }
    row.push(phases.join(", "));
    table.rows.push(row);
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
    out.push_str("\nDeclared consumers (runtime unverified)\n");
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
        let _ = writeln!(
            out,
            "  {}\n    PV: {}  |  Driver: {}\n    Mode: {}  |  Access: {}  |  Reclaim: {}",
            record_identity(record),
            cell(&record["pv_name"]),
            cell(&record["driver"]),
            cell(&record["volume_mode"]),
            joined_cells(&record["access_modes"], ", ", control)?,
            cell(&record["reclaim_policy"])
        );
        let edges = record["id"]
            .as_str()
            .and_then(|id| references.get(id))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if edges.is_empty() {
            out.push_str("    No visible consumers.\n");
        }
        for edge in edges {
            control.step()?;
            let consumer = edge["consumer_id"]
                .as_str()
                .and_then(|id| consumers.get(id))
                .copied()
                .unwrap_or(&Value::Null);
            let _ = writeln!(
                out,
                "    Pod: {}  |  Node: {}  |  State: {}",
                record_identity(consumer),
                cell(&consumer["node"]),
                cell(&consumer["reference_state"])
            );
            if items(&edge["mounts"]).next().is_none() {
                let _ = writeln!(
                    out,
                    "      Volumes: {} (no declared container mapping)",
                    joined_cells(&edge["volume_names"], ", ", control)?
                );
            }
            declared_mounts(out, edge, control)?;
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
    out.push('\n');
    if value["selection"]["complete"] == true {
        let _ = writeln!(
            out,
            "Selection: complete ({} matched)",
            cell(&value["selection"]["matched_count"])
        );
    } else {
        let _ = writeln!(
            out,
            "Selection: incomplete ({} matched, {} unresolved)",
            cell(&value["selection"]["matched_count"]),
            cell(&value["selection"]["unknown_count"])
        );
    }
    metrics_footer(out, value, utilization, control)?;
    if value["selection"]["complete"] == false {
        out.push_str(
            if value["query"]["require_metrics"] == true || !value["query"]["fail_on"].is_null() {
                "Counts are lower bounds; strict check inconclusive.\n"
            } else {
                "Counts are lower bounds.\n"
            },
        );
    }
    if value["summary"]["requested"]["complete"] == false
        || value["summary"]["provisioned"]["complete"] == false
    {
        out.push_str("* Partial capacity: sum of known values; unknown values are excluded.\n");
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
    let mut finding_counts = BTreeMap::new();
    for finding in items(&value["findings"]) {
        control.step()?;
        *finding_counts
            .entry(cell(&finding["severity"]))
            .or_insert(0usize) += 1;
    }
    if !finding_counts.is_empty() {
        let counts: Vec<_> = ["critical", "warning", "info"]
            .iter()
            .filter_map(|severity| {
                finding_counts
                    .get(*severity)
                    .map(|n| format!("{n} {severity}"))
            })
            .collect();
        let _ = writeln!(
            out,
            "Findings: {} (details: --view issues)",
            counts.join(", ")
        );
    }
    if value["summary"]["overlapping_children"] == true || has_overlap(&value["groups"], control)? {
        out.push_str(
            "Shared references: do not add sibling totals; the root counts each volume once.\n",
        );
    }
    for diagnostic in items(&value["diagnostics"]) {
        control.step()?;
        let _ = writeln!(
            out,
            "Collection note [{}]: {}",
            cell(&diagnostic["reason"]),
            cell(&diagnostic["message"])
        );
    }
    filesystem_caveats(out, value, utilization, control)?;
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
    if report.get("calculations").is_some() {
        return calculation_csv_rows(report, control, visit);
    }
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

fn calculated_amount(value: &Value, unit: crate::calculations::Unit) -> String {
    value
        .as_str()
        .and_then(|v| v.parse::<u128>().ok())
        .map(|v| unit.format(v, 1))
        .unwrap_or_else(|| "--".into())
}

pub const CALCULATION_CSV_HEADER: [&str; 26] = [
    "row_type",
    "group",
    "measure",
    "unit",
    "known_count",
    "total_count",
    "complete",
    "sum_bytes",
    "sum",
    "min_bytes",
    "min",
    "mean_numerator_bytes",
    "mean_denominator",
    "mean",
    "median_numerator_bytes",
    "median_denominator",
    "median",
    "p95_bytes",
    "p95",
    "max_bytes",
    "max",
    "growth_numerator_bytes_per_second",
    "growth_denominator",
    "first_full_numerator_seconds",
    "first_full_denominator",
    "group_id",
];
fn statistic_csv_row(
    row_type: &str,
    label: &str,
    id: &str,
    measure: &str,
    stats: &Value,
    unit: crate::calculations::Unit,
) -> Vec<String> {
    let mut row = vec![String::new(); CALCULATION_CSV_HEADER.len()];
    row[25] = id.into();
    for (index, value) in [
        (0, row_type.to_owned()),
        (1, label.to_owned()),
        (2, measure.to_owned()),
        (3, unit.to_string()),
        (4, csv_cell(&stats["known_count"])),
        (5, csv_cell(&stats["total_count"])),
        (6, csv_cell(&stats["complete"])),
    ] {
        row[index] = value;
    }
    for (raw, display, key) in [
        (7, 8, "sum_bytes"),
        (9, 10, "min_bytes"),
        (17, 18, "p95_bytes"),
        (19, 20, "max_bytes"),
    ] {
        row[raw] = csv_cell(&stats[key]);
        if !stats[key].is_null() {
            row[display] = calculated_amount(&stats[key], unit);
        }
    }
    for (raw, display, key) in [(11, 13, "mean_bytes"), (14, 16, "median_bytes")] {
        row[raw] = csv_cell(&stats[key]["numerator"]);
        row[raw + 1] = csv_cell(&stats[key]["denominator"]);
        if !stats[key].is_null() {
            row[display] = crate::calculations::format_fraction(&stats[key], unit);
        }
    }
    row
}
fn calculation_csv_rows<C: Checkpoint>(
    report: &Value,
    control: &mut C,
    mut visit: impl FnMut(Vec<String>) -> Result<(), C::Error>,
) -> Result<(), C::Error> {
    let calculations = &report["calculations"];
    let unit = calculations["display_unit"]
        .as_str()
        .and_then(|v| v.parse().ok())
        .unwrap_or(crate::calculations::Unit::Auto);
    for row in items(&calculations["rows"]) {
        control.step()?;
        let label = row["label"].as_str().unwrap_or("");
        let id = row["id"].as_str().unwrap_or("");
        for measure in ["requested", "bound", "provisioned"] {
            visit(statistic_csv_row(
                "group",
                label,
                id,
                measure,
                &row["statistics"][measure],
                unit,
            ))?;
        }
        if report["query"]["utilization"] == true {
            let fs = &row["totals"]["filesystem"];
            for (measure, key) in [
                ("filesystem_capacity", "capacity_bytes"),
                ("used", "used_bytes"),
                ("available", "available_bytes"),
            ] {
                let stats = serde_json::json!({"known_count":fs["coverage"]["fresh_complete"],"total_count":fs["coverage"]["eligible"],"complete":fs["status"]=="ok" && row["totals"]["selection_complete"]==true,"sum_bytes":fs[key]});
                visit(statistic_csv_row(
                    "filesystem",
                    label,
                    id,
                    measure,
                    &stats,
                    unit,
                ))?;
            }
        }
        if let Some(growth) = row.get("growth") {
            let mut entry = statistic_csv_row(
                "growth",
                label,
                id,
                "used",
                &serde_json::json!({"known_count":growth["matched_count"],"total_count":growth["eligible_count"],"complete":growth["complete"]}),
                unit,
            );
            for (index, key) in [(21, "growth_bytes_per_second"), (23, "first_full_seconds")] {
                entry[index] = csv_cell(&growth[key]["numerator"]);
                entry[index + 1] = csv_cell(&growth[key]["denominator"]);
            }
            visit(entry)?;
        }
        let gaps = &row["capacity_gaps"];
        for measure in ["requested_above_bound", "provisioned_above_requested"] {
            let stats = serde_json::json!({"known_count":gaps["known_count"],"total_count":gaps["total_count"],"complete":gaps["complete"],"sum_bytes":gaps[format!("{measure}_bytes")]});
            visit(statistic_csv_row(
                "capacity_gap",
                label,
                id,
                measure,
                &stats,
                unit,
            ))?;
        }
    }
    for measure in ["requested", "bound", "provisioned"] {
        control.step()?;
        visit(statistic_csv_row(
            "storage_classes",
            "CLASS TOTALS",
            "storage_classes",
            measure,
            &calculations["storage_classes"]["statistics"][measure],
            unit,
        ))?;
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

fn declared_mounts<C: Checkpoint>(
    out: &mut String,
    edge: &Value,
    control: &mut C,
) -> Result<(), C::Error> {
    for mount in items(&edge["mounts"]) {
        control.step()?;
        let container = match mount["container_kind"].as_str() {
            Some("init") => "Init container",
            Some("ephemeral") => "Ephemeral container",
            _ => "Container",
        };
        let path = if mount["kind"] == "block" {
            "Device"
        } else {
            "Mount"
        };
        let _ = writeln!(
            out,
            "      {container}: {}  |  Volume: {}\n      {path}: {}  |  Read-only: {}",
            cell(&mount["container"]),
            cell(&mount["volume_name"]),
            cell(&mount["path"]),
            match mount["read_only"].as_bool() {
                Some(true) => "yes",
                Some(false) => "no",
                None => "--",
            }
        );
        for (field, label) in [
            ("sub_path", "Subpath"),
            ("sub_path_expr", "Subpath expression"),
        ] {
            if !mount[field].is_null() {
                let _ = writeln!(out, "      {label}: {}", cell(&mount[field]));
            }
        }
    }
    Ok(())
}

fn metrics_footer<C: Checkpoint>(
    out: &mut String,
    value: &Value,
    utilization: bool,
    control: &mut C,
) -> Result<(), C::Error> {
    if utilization {
        let fs = &value["summary"]["filesystem"];
        let inodes = &value["summary"]["inodes"];
        let _ = writeln!(
            out,
            "Metrics: {}/{} {} fresh and complete; inodes: {}/{}{}",
            cell(&fs["coverage"]["fresh_complete"]),
            cell(&fs["coverage"]["eligible"]),
            if value["query"]["kind"] == "pv" {
                "volumes"
            } else {
                "claims"
            },
            cell(&inodes["coverage"]["fresh_complete"]),
            cell(&inodes["coverage"]["eligible"]),
            if value["selection"]["complete"] == false {
                " (confirmed matches only)"
            } else {
                ""
            }
        );
        if !fs["newest_observed_at"].is_null() {
            let _ = writeln!(out, "Latest sample: {}", cell(&fs["newest_observed_at"]));
        }
        for (family, label) in [(fs, "Bytes"), (inodes, "Inodes")] {
            if let Some(reasons) = family["coverage"]["by_reason"].as_object() {
                let mut unavailable = vec![];
                for (reason, count) in reasons {
                    control.step()?;
                    unavailable.push(format!("{} {}", cell(count), reason.replace('_', " ")));
                }
                if !unavailable.is_empty() {
                    let _ = writeln!(out, "{label} unavailable: {}", unavailable.join(", "));
                }
            }
        }
    } else {
        out.push_str("Metrics: not requested (use -u to collect usage).\n");
    }
    Ok(())
}

fn filesystem_caveats<C: Checkpoint>(
    out: &mut String,
    value: &Value,
    utilization: bool,
    control: &mut C,
) -> Result<(), C::Error> {
    if utilization {
        let mut overlapping = false;
        let mut exceeds = 0;
        for record in items(&value["records"]) {
            control.step()?;
            overlapping |= record["overlapping_filesystem"] == true;
            let capacity = record["filesystem"]["capacity_bytes"]
                .as_str()
                .and_then(|s| s.parse::<u128>().ok());
            let provisioned = record["provisioned_bytes"]
                .as_str()
                .and_then(|s| s.parse::<u128>().ok());
            if let (Some(capacity), Some(provisioned)) = (capacity, provisioned)
                && capacity > provisioned
            {
                exceeds += 1;
            }
        }
        if overlapping {
            out.push_str("Known shared filesystems are excluded from additive metrics coverage and totals; individual observations remain visible.\n");
        }
        if exceeds > 0 {
            let _ = writeln!(
                out,
                "Note: filesystem size exceeds provisioned size on {exceeds} volume(s); the driver may report backing storage."
            );
        }
        out.push_str("Filesystem totals are per claim, not physical storage totals.\n");
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/output.rs"]
mod tests;
