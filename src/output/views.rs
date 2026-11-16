//! Text views over the same finalized report and record index.
use super::{
    TextContext, TextTable, amount, cell, empty, group_label, group_table, human_label,
    inode_percent, items, metric_cells, metric_status, percent, pod_count, record_identity,
    summary_row, tree,
};
use crate::work::Checkpoint;
use serde_json::Value;
use std::{collections::BTreeMap, fmt::Write as _};

pub(super) fn tree_view<C: Checkpoint>(
    out: &mut String,
    value: &Value,
    context: &TextContext<'_>,
    control: &mut C,
) -> Result<(), C::Error> {
    let mut table = group_table("STORAGE", context.utilization);
    let groups: Vec<_> = items(&value["groups"]).collect();
    for group in groups {
        tree(&mut table, group, "", "", context, control)?;
    }
    table.draw(out, control)?;
    if context.records.is_empty() {
        empty(out, value);
    }
    Ok(())
}

pub(super) fn table_view<C: Checkpoint>(
    out: &mut String,
    value: &Value,
    context: &TextContext<'_>,
    control: &mut C,
) -> Result<(), C::Error> {
    let mut columns = vec![
        (
            if value["query"]["kind"] == "pv" {
                "PV"
            } else {
                "PVC"
            },
            false,
        ),
        ("PHASE", false),
        ("PODS", true),
        ("REQUESTED", true),
        ("PROVISIONED", true),
    ];
    if context.utilization {
        columns.extend([
            ("FS SIZE", true),
            ("USED", true),
            ("AVAILABLE", true),
            ("USE%", true),
            ("INODE%", true),
            ("FS SAMPLE", false),
        ]);
    }
    let mut table = TextTable::new(columns);
    for record in items(&value["records"]) {
        control.step()?;
        let mut row = vec![
            if value["collection"]["scope"]["all_namespaces"] == true {
                record_identity(record)
            } else {
                cell(&record["name"])
            },
            cell(&record["phase"]),
            pod_count(record),
            amount(&record["requested_bytes"], context.units),
            amount(&record["provisioned_bytes"], context.units),
        ];
        if context.utilization {
            row.extend(metric_cells(&record["filesystem"], context.units));
            row.push(inode_percent(&record["inodes"]));
            row.push(metric_status(&record["filesystem"]));
        }
        table.rows.push(row);
    }
    table.draw(out, control)?;
    if items(&value["records"]).any(|r| {
        r["terminating_consumer_count"]
            .as_u64()
            .is_some_and(|n| n > 0)
    }) {
        out.push_str("Pods: active + deleting (d); references do not multiply volume capacity.\n");
    }
    if context.records.is_empty() {
        empty(out, value);
    }
    Ok(())
}

pub(super) fn summary_view<C: Checkpoint>(
    out: &mut String,
    value: &Value,
    context: &TextContext<'_>,
    control: &mut C,
) -> Result<(), C::Error> {
    let mut table = group_table("GROUP", context.utilization);
    table.columns.push(("PHASES", false));
    summary_row(&mut table, "TOTAL", &value["summary"], context, control)?;
    for group in items(&value["groups"]) {
        control.step()?;
        summary_row(
            &mut table,
            &group_label(group),
            &group["totals"],
            context,
            control,
        )?;
    }
    table.draw(out, control)?;
    Ok(())
}

pub(super) fn issues_view<C: Checkpoint>(
    out: &mut String,
    value: &Value,
    context: &TextContext<'_>,
    control: &mut C,
) -> Result<(), C::Error> {
    for finding in items(&value["findings"]) {
        control.step()?;
        let record = finding["subject_id"]
            .as_str()
            .and_then(|id| context.records.get(id))
            .copied()
            .unwrap_or(&Value::Null);
        let severity = cell(&finding["severity"]);
        let severity = if context.color {
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
        let _ = writeln!(
            out,
            "{severity}  {}: {} [{}]",
            record_identity(record),
            human_label(&finding["code"]),
            cell(&finding["code"])
        );
        let mut sources = BTreeMap::new();
        for evidence in items(&finding["evidence"]) {
            control.step()?;
            let displayed = if evidence["unit"] == "bytes"
                || evidence["field"]
                    .as_str()
                    .is_some_and(|s| s.ends_with("_bytes"))
            {
                amount(&evidence["value"], context.units)
            } else if evidence["unit"] == "percent" || evidence["field"] == "used_percent" {
                if evidence["value"].is_string() {
                    format!("{}%", cell(&evidence["value"]))
                } else {
                    percent(&evidence["value"])
                }
            } else {
                cell(&evidence["value"])
            };
            let _ = writeln!(out, "  {}: {}", human_label(&evidence["field"]), displayed);
            if !evidence["observed_at"].is_null() {
                sources.insert(
                    (
                        cell(&evidence["observed_at"]),
                        cell(&evidence["source"]["provider"]),
                        cell(&evidence["source"]["node"]),
                    ),
                    (),
                );
            }
        }
        for ((timestamp, provider, node), ()) in sources {
            control.step()?;
            let _ = writeln!(out, "  Observed: {timestamp} via {provider} on {node}");
        }
        let _ = writeln!(out, "  Next: {}\n", cell(&finding["suggested_action"]));
    }
    if items(&value["findings"]).next().is_none() {
        out.push_str(if value["selection"]["complete"] == true {
            "No observed findings in selected records.\n"
        } else {
            "No observed findings among confirmed matches; selection is incomplete.\n"
        });
    }
    Ok(())
}
