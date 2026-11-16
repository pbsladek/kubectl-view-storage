//! Statistics tables and growth presentation; accounting stays in calculations.
use super::{TextTable, calculated_amount, cell, escape, items, percent};
use crate::work::Checkpoint;
use serde_json::Value;
use std::{collections::BTreeMap, fmt::Write as _};

pub(super) fn statistics_text<C: Checkpoint>(
    out: &mut String,
    report: &Value,
    utilization: bool,
    wide: bool,
    control: &mut C,
) -> Result<(), C::Error> {
    use crate::calculations::Unit;
    let calculations = &report["calculations"];
    let unit = calculations["display_unit"]
        .as_str()
        .and_then(|v| v.parse().ok())
        .unwrap_or(Unit::Auto);
    let _ = writeln!(
        out,
        "Storage calculations (units: {unit}; logical capacity)\n"
    );
    let mut sums = TextTable::new(vec![
        ("GROUP", false),
        ("COUNT", true),
        ("REQUESTED", true),
        ("BOUND", true),
        ("PROVISIONED", true),
    ]);
    for row in items(&calculations["rows"]) {
        control.step()?;
        let mut cells = vec![cell(&row["label"]), cell(&row["totals"]["record_count"])];
        for measure in ["requested", "bound", "provisioned"] {
            let stats = &row["statistics"][measure];
            let amount = calculated_amount(&stats["sum_bytes"], unit);
            cells.push(format!(
                "{amount}{}",
                if stats["complete"] == false { "*" } else { "" }
            ));
        }
        sums.rows.push(cells);
    }
    sums.draw(out, control)?;
    statistics_filesystems(out, calculations, unit, utilization, control)?;
    statistics_distributions(out, calculations, unit, wide, control)?;
    statistics_growth(out, calculations, unit, control)?;
    for row in items(&calculations["rows"]) {
        control.step()?;
        let gaps = &row["capacity_gaps"];
        if gaps["requested_above_bound_bytes"]
            .as_str()
            .is_some_and(|v| v != "0")
            || gaps["provisioned_above_requested_bytes"]
                .as_str()
                .is_some_and(|v| v != "0")
        {
            let _ = writeln!(
                out,
                "\n{}: requested above bound {}; provisioned above requested {} (paired {}/{}, complete={}).",
                cell(&row["label"]),
                calculated_amount(&gaps["requested_above_bound_bytes"], unit),
                calculated_amount(&gaps["provisioned_above_requested_bytes"], unit),
                cell(&gaps["known_count"]),
                cell(&gaps["total_count"]),
                cell(&gaps["complete"])
            );
        }
    }
    out.push_str("\n* Incomplete selection or quantities; statistics use known values.\nRequested, bound and provisioned are separate capacities; do not add them together.\n");
    Ok(())
}

fn statistics_row(
    table: &mut TextTable,
    label: &str,
    measure: &str,
    stats: &Value,
    unit: crate::calculations::Unit,
) {
    table.rows.push(vec![
        label.into(),
        measure.into(),
        format!(
            "{}/{}{}",
            cell(&stats["known_count"]),
            cell(&stats["total_count"]),
            if stats["complete"] == false { "*" } else { "" }
        ),
        calculated_amount(&stats["min_bytes"], unit),
        crate::calculations::format_fraction(&stats["mean_bytes"], unit),
        crate::calculations::format_fraction(&stats["median_bytes"], unit),
        calculated_amount(&stats["p95_bytes"], unit),
        calculated_amount(&stats["max_bytes"], unit),
    ]);
}

fn statistics_filesystems<C: Checkpoint>(
    out: &mut String,
    calculations: &Value,
    unit: crate::calculations::Unit,
    utilization: bool,
    control: &mut C,
) -> Result<(), C::Error> {
    if utilization {
        out.push_str("\nReported filesystem usage (per claim; not physical backend totals)\n\n");
        let mut filesystems = TextTable::new(vec![
            ("GROUP", false),
            ("FS SIZE", true),
            ("USED", true),
            ("AVAILABLE", true),
            ("USE%", true),
            ("SAMPLES", true),
        ]);
        for row in items(&calculations["rows"]) {
            control.step()?;
            let mut cells = vec![cell(&row["label"])];
            let fs = &row["totals"]["filesystem"];
            for field in ["capacity_bytes", "used_bytes", "available_bytes"] {
                cells.push(if fs["status"] == "not_applicable" {
                    "n/a".into()
                } else {
                    calculated_amount(&fs[field], unit)
                });
            }
            cells.push(percent(&fs["used_percent"]));
            cells.push(format!(
                "{}/{}",
                cell(&fs["coverage"]["fresh_complete"]),
                cell(&fs["coverage"]["eligible"])
            ));
            filesystems.rows.push(cells);
        }
        filesystems.draw(out, control)?;
    }
    Ok(())
}

fn statistics_growth<C: Checkpoint>(
    out: &mut String,
    calculations: &Value,
    unit: crate::calculations::Unit,
    control: &mut C,
) -> Result<(), C::Error> {
    use crate::calculations::format_seconds;
    if !calculations["comparison"].is_null() {
        out.push_str("\nObserved growth (paired samples; first full means earliest claim)\n\n");
        let mut table = TextTable::new(vec![
            ("GROUP", false),
            ("GROWTH / HOUR", true),
            ("FIRST FULL IN", true),
            ("PAIRS", true),
            ("STATUS", false),
        ]);
        for row in items(&calculations["rows"]) {
            control.step()?;
            let growth = &row["growth"];
            table.rows.push(vec![
                cell(&row["label"]),
                crate::calculations::format_hourly_rate(&growth["growth_bytes_per_second"], unit),
                format_seconds(&growth["first_full_seconds"]),
                format!(
                    "{}/{}",
                    cell(&growth["matched_count"]),
                    cell(&growth["eligible_count"])
                ),
                if growth["complete"] == true {
                    "Complete".into()
                } else {
                    "Partial".into()
                },
            ]);
        }
        table.draw(out, control)?;
        let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
        for rate in items(&calculations["comparison"]["records"]) {
            control.step()?;
            if rate["status"] == "not_applicable" {
                continue;
            }
            if let Some(reason) = rate["reason"].as_str() {
                *reasons.entry(reason).or_default() += 1;
            }
        }
        for (reason, count) in reasons {
            let _ = writeln!(
                out,
                "Growth/projection note: {} ({count} record(s))",
                escape(reason)
            );
        }
        for row in items(&calculations["rows"]) {
            control.step()?;
            if row["growth"]["reason"] == "arithmetic_overflow" {
                let _ = writeln!(
                    out,
                    "{}: growth/projection exceeds the supported arithmetic range.",
                    cell(&row["label"])
                );
            }
        }
        out.push_str("Two-sample linear estimates assume unchanged growth; they are not forecasts of backend capacity.\n");
    }
    Ok(())
}

fn statistics_distributions<C: Checkpoint>(
    out: &mut String,
    calculations: &Value,
    unit: crate::calculations::Unit,
    wide: bool,
    control: &mut C,
) -> Result<(), C::Error> {
    out.push_str(if wide {
        "\nSize distribution (known records; P95 uses nearest rank)\n\n"
    } else {
        "\nProvisioned volume sizes (known records; P95 uses nearest rank)\n\n"
    });
    let mut stats_table = TextTable::new(vec![
        ("GROUP", false),
        ("MEASURE", false),
        ("KNOWN", true),
        ("MIN", true),
        ("MEAN", true),
        ("MEDIAN", true),
        ("P95", true),
        ("MAX", true),
    ]);
    for row in items(&calculations["rows"]) {
        control.step()?;
        for &measure in if wide {
            &["requested", "bound", "provisioned"][..]
        } else {
            &["provisioned"][..]
        } {
            statistics_row(
                &mut stats_table,
                &cell(&row["label"]),
                measure,
                &row["statistics"][measure],
                unit,
            );
        }
    }
    stats_table.draw(out, control)?;
    let _ = writeln!(
        out,
        "\nDistribution of named storage-class totals ({} classes)\n",
        cell(&calculations["storage_classes"]["named_count"])
    );
    let mut classes = TextTable::new(stats_table.columns.clone());
    for &measure in if wide {
        &["requested", "bound", "provisioned"][..]
    } else {
        &["requested", "provisioned"][..]
    } {
        statistics_row(
            &mut classes,
            "CLASS TOTALS",
            measure,
            &calculations["storage_classes"]["statistics"][measure],
            unit,
        );
    }
    classes.draw(out, control)?;
    let states = &calculations["storage_classes"]["record_counts_by_assignment"];
    if states.get("classless").is_some()
        || states.get("unassigned").is_some()
        || states.get("unknown").is_some()
    {
        let _ = writeln!(
            out,
            "Class distributions exclude classless={}, unassigned={}, unknown={} record(s); TOTAL includes them.",
            states["classless"].as_u64().unwrap_or(0),
            states["unassigned"].as_u64().unwrap_or(0),
            states["unknown"].as_u64().unwrap_or(0)
        );
    }
    Ok(())
}
