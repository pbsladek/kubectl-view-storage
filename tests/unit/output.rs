#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::policy::Exit;
macro_rules! scenario {
    ($name:literal) => {
        (
            $name,
            include_str!(concat!("../../examples/contract/", $name, "/report.json")),
            [
                include_str!(concat!("../../examples/contract/", $name, "/tree.txt")),
                include_str!(concat!("../../examples/contract/", $name, "/table.txt")),
                include_str!(concat!("../../examples/contract/", $name, "/summary.txt")),
                include_str!(concat!("../../examples/contract/", $name, "/issues.txt")),
            ],
        )
    };
}

#[test]
fn all_contract_views_render_from_report_data() {
    for (name, document, expected) in [
        scenario!("healthy"),
        scenario!("shared-rwx"),
        scenario!("partial-permissions"),
        scenario!("missing-stale"),
        scenario!("empty"),
        scenario!("unknown-selection"),
        scenario!("field-pressure"),
        scenario!("ambiguous-critical"),
        scenario!("terminating-consumer"),
    ] {
        let report = Report {
            document: serde_json::from_str(document).unwrap(),
            exit: Exit::Success,
            strict_conflict: (name == "ambiguous-critical").then_some(Severity::Critical),
        };
        for (index, view) in [View::Tree, View::Table, View::Summary, View::Issues]
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                text(&report, view, view == View::Table, false),
                expected[index],
                "{name}: {view:?}"
            );
        }
    }
}

#[test]
fn csv_schema_preserves_exact_values_nulls_and_zero() {
    let mut report: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/report.json")).unwrap();
    report["records"][0]["requested_bytes"] = "18446744073709551615".into();
    report["records"][0]["filesystem"]["used_bytes"] = "0".into();
    report["records"][0]["inodes"]["inodes_free"] = Value::Null;
    let rows = csv_rows(&report);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), CSV_HEADER.len());
    assert_eq!(rows[0][9], "18446744073709551615");
    assert_eq!(rows[0][13], "0");
    assert_eq!(rows[0][18], "");
    assert_eq!(rows[0][22], "kubelet");
    report["records"] = serde_json::json!([]);
    assert!(csv_rows(&report).is_empty());
}

#[test]
fn terminal_controls_and_color_policy_are_explicit() {
    assert_eq!(
        escape("a\u{1b}[31m\n\r\tb\u{202e}"),
        "a\\u{1b}[31m\\n\\r\\tb\\u{202e}"
    );
    assert!(!use_color(Color::Auto, false, false));
    assert!(!use_color(Color::Auto, true, true));
    assert!(use_color(Color::Auto, true, false));
    assert!(use_color(Color::Always, false, true));
    assert!(!use_color(Color::Never, true, false));
    let mut report = Report {
        document: serde_json::from_str(include_str!(
            "../../examples/contract/field-pressure/report.json"
        ))
        .unwrap(),
        exit: Exit::Findings,
        strict_conflict: None,
    };
    report.document["records"][0]["name"] = "bad\u{1b}[31m\nname".into();
    let plain = text(&report, View::Issues, true, false);
    assert!(!plain.contains('\u{1b}'));
    assert!(plain.contains("\\u{1b}"));
    let colored = text(&report, View::Issues, false, true);
    assert!(colored.contains("\u{1b}[31mcritical\u{1b}[0m"));
    let mut json = Vec::new();
    write_json(&mut json, &report.document).unwrap();
    assert_eq!(json.last(), Some(&b'\n'));
    assert!(!json.contains(&27));
    assert_eq!(
        serde_json::from_slice::<Value>(&json).unwrap(),
        report.document
    );
}

#[test]
fn output_failures_are_returned_including_closed_pipes() {
    struct Closed;
    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    assert!(write_json(Closed, &serde_json::json!({})).is_err());
}

#[test]
fn no_usage_columns_and_declared_block_init_details() {
    let mut report = Report {
        document: serde_json::from_str(include_str!("../../examples/contract/healthy/report.json"))
            .unwrap(),
        exit: Exit::Success,
        strict_conflict: None,
    };
    report.document["query"]["utilization"] = false.into();
    report.document["relationships"][0]["mounts"][0]["container_kind"] = "init".into();
    report.document["relationships"][0]["mounts"][0]["kind"] = "block".into();
    let rendered = text(&report, View::Table, true, false);
    assert!(!rendered.contains("FS-CAP"));
    assert!(rendered.contains("Metrics: not requested"));
    assert!(rendered.contains("Init container: app"));
    assert!(rendered.contains("Device: /var/lib/data"));
    assert_eq!(iec(&Value::from("1536")), "1.5 KiB");
    assert_eq!(iec(&Value::from("18446744073709551615")), "16 EiB");
}

struct Budget {
    remaining: usize,
    steps: usize,
}
impl Checkpoint for Budget {
    type Error = ();
    fn step(&mut self) -> Result<(), ()> {
        self.remaining = self.remaining.checked_sub(1).ok_or(())?;
        self.steps += 1;
        Ok(())
    }
}

fn large_report(count: usize) -> Report {
    let mut document: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/report.json")).unwrap();
    let record = document["records"][0].clone();
    let edge = document["relationships"][0].clone();
    let mut records = vec![];
    let mut edges = vec![];
    for index in 0..count {
        let mut record = record.clone();
        record["id"] = format!("pvc:claim-{index}").into();
        record["uid"] = format!("claim-{index}").into();
        record["name"] = format!("claim-{index}").into();
        let mut edge = edge.clone();
        edge["record_id"] = record["id"].clone();
        records.push(record);
        edges.push(edge);
    }
    document["records"] = Value::Array(records);
    document["relationships"] = Value::Array(edges);
    Report {
        document,
        exit: Exit::Success,
        strict_conflict: None,
    }
}

#[test]
fn large_and_nested_rendering_work_is_interruptible() {
    let report = large_report(1000);
    for view in [View::Tree, View::Table, View::Summary, View::Issues] {
        assert!(
            text_checked(
                &report,
                view,
                true,
                false,
                &mut Budget {
                    remaining: 64,
                    steps: 0
                }
            )
            .is_err()
        );
    }
    let mut out = String::new();
    assert!(
        declared(
            &mut out,
            &report.document,
            &mut Budget {
                remaining: 64,
                steps: 0
            }
        )
        .is_err()
    );
    let mut group = report.document["groups"][0].clone();
    group["children"] = serde_json::json!([]);
    group["member_ids"] = serde_json::json!(
        (0..1000)
            .map(|index| format!("pvc:claim-{index}"))
            .collect::<Vec<_>>()
    );
    assert!(
        tree(
            &mut group_table("STORAGE", true),
            &group,
            "",
            "",
            &BTreeMap::new(),
            true,
            &mut Budget {
                remaining: 64,
                steps: 0
            }
        )
        .is_err()
    );
    let mut totals = report.document["summary"].clone();
    totals["phase_counts"] = Value::Object(
        (0..1000)
            .map(|index| (format!("phase-{index}"), Value::from(1)))
            .collect(),
    );
    assert!(
        summary_row(
            &mut group_table("GROUP", true),
            "ROOT",
            &totals,
            true,
            &mut Budget {
                remaining: 64,
                steps: 0
            }
        )
        .is_err()
    );
    assert!(
        visit_csv_rows(
            &report.document,
            &mut Budget {
                remaining: 64,
                steps: 0
            },
            |_| Ok(())
        )
        .is_err()
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    let mut bytes = vec![];
    assert!(write_text(&mut bytes, &report, View::Tree, true, false, &cancel).is_err());
    assert!(bytes.is_empty());
}

#[test]
fn wide_rendering_indexes_relationships_and_scales_with_records_and_edges() {
    let report = large_report(1000);
    let mut control = Budget {
        remaining: usize::MAX,
        steps: 0,
    };
    let mut rendered = String::new();
    declared(&mut rendered, &report.document, &mut control).unwrap();
    assert!(
        control.steps <= 8000,
        "relationship scanning grew beyond a linear pass: {}",
        control.steps
    );
    assert_eq!(rendered.matches("PV:").count(), 1000);
    assert_eq!(rendered.matches("Pod:").count(), 1000); // two declared mounts per edge
    assert!(rendered.contains("demo/claim-999"));
}

#[test]
fn csv_rows_stream_and_stop_on_callback_failure_or_cancellation() {
    let report = large_report(1000);
    let cancel = CancellationToken::new();
    let mut received = 0;
    let error = for_each_csv_row(&report.document, &cancel, |_| {
        received += 1;
        Err(io::ErrorKind::BrokenPipe.into())
    })
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(received, 1);
    received = 0;
    assert!(
        for_each_csv_row(&report.document, &cancel, |row| {
            assert_eq!(row.len(), CSV_HEADER.len());
            received += 1;
            if received == 20 {
                cancel.cancel();
            }
            Ok(())
        })
        .is_err()
    );
    assert!(
        (20..=84).contains(&received),
        "cancellation escaped the checkpoint allowance: {received}"
    );
    let mut after = 0;
    assert!(
        for_each_csv_row(&report.document, &cancel, |_| {
            after += 1;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(after, 0);
}

#[test]
fn human_tables_align_numbers_keep_namespace_identity_and_round_only_display() {
    let mut report = Report {
        document: serde_json::from_str(include_str!("../../examples/contract/healthy/report.json"))
            .unwrap(),
        exit: Exit::Success,
        strict_conflict: None,
    };
    report.document["collection"]["scope"]["all_namespaces"] = true.into();
    let mut second = report.document["records"][0].clone();
    second["namespace"] = "production".into();
    second["provisioned_bytes"] = "1073741824".into();
    report.document["records"]
        .as_array_mut()
        .unwrap()
        .push(second);
    let rendered = text(&report, View::Table, false, false);
    assert!(rendered.contains("demo/data") && rendered.contains("production/data"));
    let header = rendered.lines().find(|l| l.starts_with("PVC")).unwrap();
    let first = rendered
        .lines()
        .find(|l| l.starts_with("demo/data "))
        .unwrap();
    let second = rendered
        .lines()
        .find(|l| l.starts_with("production/data "))
        .unwrap();
    let start = header.find("PROVISIONED").unwrap();
    let end = start + "PROVISIONED".len();
    assert_eq!(
        first[..end].trim_end().rsplit("  ").next().unwrap().trim(),
        "100 GiB"
    );
    assert_eq!(
        second[..end].trim_end().rsplit("  ").next().unwrap().trim(),
        "1 GiB"
    );
    assert!(rendered.contains("Fresh"));
    assert!(!rendered.contains("physical_total=") && !rendered.contains("[metrics:"));
    assert_eq!(percent(&serde_json::json!(60.132122)), "60.1%");
    assert_eq!(iec(&Value::from("62671097856")), "58.4 GiB");
    assert_eq!(iec(&Value::from("0")), "0 B");
    assert_eq!(iec(&Value::Null), "--");
    assert_eq!(
        report.document["records"][0]["filesystem"]["used_percent"],
        40.0
    );
}

#[test]
fn terminal_metric_labels_preserve_partial_stale_block_and_conflicting_states() {
    let base: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/report.json")).unwrap();
    let mut metric = base["records"][0]["inodes"].clone();
    for (state, expected) in [
        ("stale", "10.0% (stale)"),
        ("uncertain", "10.0% (conflicting)"),
        ("ok", "10.0%"),
        ("not_applicable", "n/a"),
    ] {
        metric["status"] = state.into();
        assert_eq!(inode_percent(&metric), expected);
    }
    metric["used_percent"] = Value::Null;
    metric["status"] = "invalid".into();
    assert_eq!(inode_percent(&metric), "--");
    for (state, reason, expected) in [
        ("partial", "missing_field", "Partial"),
        ("forbidden", "forbidden", "Access denied"),
        ("timeout", "timeout", "Timed out"),
        ("missing", "no_live_consumer", "No consumer"),
        ("unavailable", "time_skew", "Time skew"),
        ("unavailable", "overlapping_filesystem", "Shared storage"),
        ("timestamp_unknown", "no_timestamp", "No timestamp"),
        ("identity_uncertain", "identity_uncertain", "Unverified"),
    ] {
        metric["status"] = state.into();
        metric["reason"] = reason.into();
        assert_eq!(metric_status(&metric), expected);
    }
}
