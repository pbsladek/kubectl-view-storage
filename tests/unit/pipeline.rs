#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::{
    cli::Cli,
    clock::tests::TestClock,
    collect::{self, Response, ResponseFuture, Transport},
};
use clap::Parser;
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct FixtureApi {
    bundle: Value,
}
impl Transport for FixtureApi {
    fn get(&self, url: String) -> ResponseFuture<'_> {
        Box::pin(async move {
            let path = url.split('?').next().unwrap();
            for failure in self.bundle["failures"].as_array().unwrap() {
                if failure["path"] == path {
                    return Ok(Response {
                        status: failure["status"].as_u64().unwrap() as u16,
                        body: vec![],
                        retry_after: None,
                    });
                }
            }
            let parts: Vec<_> = path.split('/').collect();
            let (resource, name, scope) = if path.starts_with("/apis/") {
                ("storage_classes", parts.get(5).copied(), None)
            } else if path.contains("/stats/summary") {
                let node = parts.get(4).copied().unwrap_or("");
                return Ok(Response {
                    status: 200,
                    body: serde_json::to_vec(&self.bundle["summary_responses"][node]).unwrap(),
                    retry_after: None,
                });
            } else if parts.get(3) == Some(&"namespaces") {
                (
                    match parts.get(5).copied() {
                        Some("persistentvolumeclaims") => "pvcs",
                        Some("pods") => "pods",
                        _ => "unknown",
                    },
                    parts.get(6).copied(),
                    parts.get(4).copied(),
                )
            } else {
                (
                    match parts.get(3).copied() {
                        Some("persistentvolumes") => "pvs",
                        Some("persistentvolumeclaims") => "pvcs",
                        Some("pods") => "pods",
                        _ => "unknown",
                    },
                    parts.get(4).copied(),
                    None,
                )
            };
            let objects = self.bundle["kubernetes"][resource].as_array().unwrap();
            let object = if let Some(name) = name {
                objects
                    .iter()
                    .find(|o| {
                        o["metadata"]["name"] == name
                            && scope.is_none_or(|scope| o["metadata"]["namespace"] == scope)
                    })
                    .cloned()
            } else {
                let rv_key = format!(
                    "{}/{}",
                    match resource {
                        "pvcs" => "persistentvolumeclaims",
                        "pvs" => "persistentvolumes",
                        other => other,
                    },
                    scope.unwrap_or("cluster"),
                );
                Some(
                    json!({"metadata":{"resourceVersion":self.bundle["resource_versions"][rv_key]},"items":objects.iter().filter(|o|scope.is_none_or(|scope|o["metadata"]["namespace"]==scope)).collect::<Vec<_>>()}),
                )
            };
            Ok(Response {
                status: if object.is_some() { 200 } else { 404 },
                body: object
                    .map(|o| serde_json::to_vec(&o).unwrap())
                    .unwrap_or_default(),
                retry_after: None,
            })
        })
    }
}

fn fixture_cli(bundle: &Value) -> Cli {
    let request = &bundle["request"];
    let mut args = vec!["cmd".to_owned()];
    if let Some(namespace) = request["namespace"].as_str() {
        args.extend(["-n".into(), namespace.into()]);
    } else {
        args.push("-A".into());
    }
    args.extend([
        "--kind".into(),
        request["kind"].as_str().unwrap().into(),
        "-g".into(),
        request["group_by"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d.as_str().unwrap())
            .collect::<Vec<_>>()
            .join(","),
    ]);
    if request["utilization"] == true {
        args.push("-u".into());
    }
    if request["require_metrics"] == true {
        args.push("--require-metrics".into());
    }
    for (field, flag) in [
        ("warn_used_percent", "--warn-used"),
        ("critical_used_percent", "--critical-used"),
        ("fail_on", "--fail-on"),
        ("sort", "--sort"),
    ] {
        if let Some(value) = request[field].as_str() {
            args.extend([flag.into(), value.into()]);
        }
    }
    for (field, flag) in [
        ("pvc", "--pvc"),
        ("pv", "--pv"),
        ("storage_class", "--storage-class"),
        ("driver", "--driver"),
        ("node", "--node"),
        ("selector", "--selector"),
    ] {
        if let Some(value) = request["filters"][field].as_str() {
            args.extend([flag.into(), value.into()]);
        }
    }
    args.extend([
        "--max-metrics-age".into(),
        format!("{}s", request["max_metrics_age_seconds"].as_u64().unwrap()),
    ]);
    Cli::try_parse_from(args).unwrap()
}
macro_rules! scenario {
    ($name:literal,$exit:literal) => {
        (
            $name,
            include_str!(concat!("../../examples/contract/", $name, "/input.json")),
            include_str!(concat!("../../examples/contract/", $name, "/report.json")),
            $exit,
        )
    };
}

fn export_case(name: &str, input: &Value, report: &Report) {
    if let Some(directory) = std::env::var_os("VIEW_STORAGE_TEST_REPORTS") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        for (name, value) in [
            (format!("{name}.json"), &report.document),
            (format!("{name}-input.json"), input),
        ] {
            std::fs::write(
                directory.join(name),
                serde_json::to_vec_pretty(value).unwrap(),
            )
            .unwrap();
        }
    }
}

#[tokio::test(start_paused = true)]
async fn requested_bound_provisioned_and_filesystem_capacities_remain_distinct() {
    for kind in ["pvc", "pv"] {
        let mut input: Value =
            serde_json::from_str(include_str!("../../examples/contract/healthy/input.json"))
                .unwrap();
        input["request"]["kind"] = kind.into();
        input["request"]["require_metrics"] = true.into();
        if kind == "pv" {
            input["request"]["namespace"] = Value::Null;
        }
        input["resource_versions"]["persistentvolumes/cluster"] = "100".into();
        input["resource_versions"]["pods/cluster"] = "101".into();
        input["kubernetes"]["pvcs"][0]["spec"]["resources"]["requests"]["storage"] = "100Gi".into();
        input["kubernetes"]["pvcs"][0]["status"]["capacity"]["storage"] = "103Gi".into();
        input["kubernetes"]["pvs"][0]["spec"]["capacity"]["storage"] = "120Gi".into();
        let sample = &mut input["summary_responses"]["worker-a"]["pods"][0]["volume"][0];
        sample["capacityBytes"] = json!(118 * 1024u64.pow(3));
        sample["usedBytes"] = json!(47 * 1024u64.pow(3));
        sample["availableBytes"] = json!(60 * 1024u64.pow(3));
        let cli = fixture_cli(&input);
        let query = cli.query("ignored").unwrap();
        let clock = Arc::new(TestClock::new(
            input["clock"].as_str().unwrap().parse().unwrap(),
        ));
        let collected = collect::collect(
            Arc::new(FixtureApi {
                bundle: input.clone(),
            }),
            clock,
            CancellationToken::new(),
            &cli,
            &query,
            "example".into(),
        )
        .await
        .unwrap();
        let report = finish_cancellable(collected, &query, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(report.exit, Exit::Success);
        let record = &report.document["records"][0];
        let summary = &report.document["summary"];
        for (field, size) in [("requested", 100), ("bound", 103), ("provisioned", 120)] {
            let bytes = (size * 1024u64.pow(3)).to_string();
            assert_eq!(record[format!("{field}_bytes")], bytes);
            assert_eq!(summary[field]["known_sum_bytes"], bytes);
        }
        assert_eq!(
            record["filesystem"]["capacity_bytes"],
            (118 * 1024u64.pow(3)).to_string()
        );
        assert_eq!(
            record["filesystem"]["available_bytes"],
            (60 * 1024u64.pow(3)).to_string()
        );
        assert_eq!(
            summary["filesystem"]["used_bytes"],
            (47 * 1024u64.pow(3)).to_string()
        );
        assert_eq!(
            summary["filesystem"]["capacity_bytes"],
            record["filesystem"]["capacity_bytes"]
        );
        assert_eq!(summary["filesystem"]["physical_total"], false);
        let rows = crate::output::csv_rows(&report.document);
        for (field, size) in [
            ("requested_bytes", 100),
            ("bound_bytes", 103),
            ("provisioned_bytes", 120),
            ("filesystem_capacity_bytes", 118),
            ("available_bytes", 60),
        ] {
            let index = crate::output::CSV_HEADER
                .iter()
                .position(|candidate| *candidate == field)
                .unwrap();
            assert_eq!(rows[0][index], (size * 1024u64.pow(3)).to_string());
        }
        for view in [
            crate::cli::View::Tree,
            crate::cli::View::Table,
            crate::cli::View::Summary,
            crate::cli::View::Issues,
        ] {
            assert!(crate::output::text(&report, view, true, false).contains("CONTEXT example"));
        }
        export_case(&format!("distinct-capacity-{kind}"), &input, &report);
    }
}

#[tokio::test(start_paused = true)]
async fn raw_block_inventory_and_required_metrics_never_infer_filesystem_observations() {
    for kind in ["pvc", "pv"] {
        for mode in ["inventory", "usage", "required"] {
            let mut input: Value =
                serde_json::from_str(include_str!("../../examples/contract/healthy/input.json"))
                    .unwrap();
            input["request"]["kind"] = kind.into();
            input["request"]["utilization"] = (mode != "inventory").into();
            input["request"]["require_metrics"] = (mode == "required").into();
            if kind == "pv" {
                input["request"]["namespace"] = Value::Null;
            }
            input["resource_versions"]["persistentvolumes/cluster"] = "100".into();
            input["resource_versions"]["pods/cluster"] = "101".into();
            input["kubernetes"]["pvcs"][0]["spec"]["volumeMode"] = "Block".into();
            input["kubernetes"]["pvs"][0]["spec"]["volumeMode"] = "Block".into();
            input["kubernetes"]["pods"][0]["spec"]["containers"] = json!([{"name":"block","image":"example.invalid/probe:test","volumeDevices":[{"name":"data","devicePath":"/dev/test-data"}]}]);
            input["kubernetes"]["pods"][0]["spec"]["initContainers"] = json!([]);
            input["kubernetes"]["pods"][0]["spec"]["ephemeralContainers"] = json!([]);
            let cli = fixture_cli(&input);
            let query = cli.query("ignored").unwrap();
            let clock = Arc::new(TestClock::new(
                input["clock"].as_str().unwrap().parse().unwrap(),
            ));
            let collected = collect::collect(
                Arc::new(FixtureApi {
                    bundle: input.clone(),
                }),
                clock,
                CancellationToken::new(),
                &cli,
                &query,
                "example".into(),
            )
            .await
            .unwrap();
            let report = finish_cancellable(collected, &query, CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(report.exit, Exit::Success, "{kind}/{mode}");
            let record = &report.document["records"][0];
            assert_eq!(record["volume_mode"], "Block");
            assert_eq!(record["provisioned_bytes"], "107374182400");
            assert_eq!(record["active_consumer_count"], 1);
            for family in ["filesystem", "inodes"] {
                assert_eq!(
                    record[family]["status"],
                    if mode == "inventory" {
                        "not_requested"
                    } else {
                        "not_applicable"
                    }
                );
                assert_eq!(record[family]["candidate_count"], 0);
                assert!(record[family]["source"].is_null());
                assert_eq!(
                    report.document["summary"][family]["coverage"]["eligible"],
                    0
                );
                assert_eq!(
                    report.document["summary"][family]["coverage"]["not_applicable"],
                    1
                );
            }
            assert_eq!(report.document["collection"]["cost"]["node_targets"], 0);
            assert_eq!(report.document["collection"]["cost"]["node_requests"], 0);
            let mounts = report.document["relationships"][0]["mounts"]
                .as_array()
                .unwrap();
            assert_eq!(mounts.len(), 1);
            assert_eq!(mounts[0]["kind"], "block");
            assert_eq!(mounts[0]["path"], "/dev/test-data");
            let rows = crate::output::csv_rows(&report.document);
            for field in [
                "filesystem_capacity_bytes",
                "used_bytes",
                "available_bytes",
                "used_percent",
                "inodes_total",
                "inodes_used",
                "inodes_free",
                "inode_percent",
            ] {
                let index = crate::output::CSV_HEADER
                    .iter()
                    .position(|candidate| *candidate == field)
                    .unwrap();
                assert!(rows[0][index].is_empty(), "{kind}/{mode}/{field}");
            }
            assert!(
                !report.document["findings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|finding| finding["code"].as_str().is_some_and(|code| code
                        .starts_with("filesystem_")
                        || code.starts_with("inode_")))
            );
            for view in [
                crate::cli::View::Tree,
                crate::cli::View::Table,
                crate::cli::View::Summary,
                crate::cli::View::Issues,
            ] {
                assert!(
                    crate::output::text(&report, view, true, false).contains("CONTEXT example")
                );
            }
            if mode == "required" {
                export_case(&format!("raw-block-{kind}"), &input, &report);
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn unfamiliar_api_classifications_preserve_inventory_and_known_siblings() {
    let mut original: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let mut claim = original["kubernetes"]["pvcs"][0].clone();
    claim["metadata"]["name"] = "other".into();
    claim["metadata"]["uid"] = "other".into();
    claim["spec"]["volumeName"] = "pv-other".into();
    original["kubernetes"]["pvcs"]
        .as_array_mut()
        .unwrap()
        .push(claim);
    let mut volume = original["kubernetes"]["pvs"][0].clone();
    volume["metadata"]["name"] = "pv-other".into();
    volume["metadata"]["uid"] = "pv-other".into();
    volume["spec"]["claimRef"]["name"] = "other".into();
    volume["spec"]["claimRef"]["uid"] = "other".into();
    volume["spec"]["csi"]["volumeHandle"] = "other-handle".into();
    original["kubernetes"]["pvs"]
        .as_array_mut()
        .unwrap()
        .push(volume);
    let mut pod = original["kubernetes"]["pods"][0].clone();
    pod["metadata"]["name"] = "other-pod".into();
    pod["metadata"]["uid"] = "other-pod".into();
    pod["spec"]["volumes"][0]["persistentVolumeClaim"]["claimName"] = "other".into();
    original["kubernetes"]["pods"]
        .as_array_mut()
        .unwrap()
        .push(pod);
    let mut observation = original["summary_responses"]["worker-a"]["pods"][0].clone();
    observation["podRef"]["name"] = "other-pod".into();
    observation["podRef"]["uid"] = "other-pod".into();
    observation["volume"][0]["pvcRef"]["name"] = "other".into();
    original["summary_responses"]["worker-a"]["pods"]
        .as_array_mut()
        .unwrap()
        .push(observation);
    for classification in ["volume-mode", "invalid-volume-mode", "pod-phase"] {
        for mode in [
            "inventory",
            "health",
            "usage",
            "strict-usage",
            "required",
            "node",
        ] {
            let mut input = original.clone();
            match classification {
                "volume-mode" => {
                    input["kubernetes"]["pvcs"][0]["spec"]["volumeMode"] = "FutureMode".into()
                }
                "invalid-volume-mode" => {
                    input["kubernetes"]["pvcs"][0]["spec"]["volumeMode"] = json!(7)
                }
                _ => input["kubernetes"]["pods"][0]["status"]["phase"] = "FuturePhase".into(),
            }
            let utilization = !matches!(mode, "inventory" | "health");
            input["request"]["utilization"] = utilization.into();
            input["request"]["require_metrics"] = matches!(mode, "required" | "node").into();
            input["request"]["fail_on"] = if matches!(mode, "health" | "strict-usage") {
                json!("critical")
            } else {
                Value::Null
            };
            if mode == "node" {
                input["request"]["filters"]["node"] = "worker-a".into();
            }
            let cli = fixture_cli(&input);
            let query = cli.query("default").unwrap();
            let clock = Arc::new(TestClock::new(
                input["clock"].as_str().unwrap().parse().unwrap(),
            ));
            let collected = collect::collect(
                Arc::new(FixtureApi {
                    bundle: input.clone(),
                }),
                clock,
                CancellationToken::new(),
                &cli,
                &query,
                "example".into(),
            )
            .await
            .unwrap();
            let report = finish_cancellable(collected, &query, CancellationToken::new())
                .await
                .unwrap_or_else(|error| panic!("{classification}/{mode}: {error}"));
            let expected_exit = if matches!(mode, "strict-usage" | "required" | "node")
                || mode == "health" && classification == "pod-phase"
            {
                Exit::Inconclusive
            } else {
                Exit::Success
            };
            assert_eq!(report.exit, expected_exit, "{classification}/{mode}");
            let records = report.document["records"].as_array().unwrap();
            let filtered_unknown = mode == "node" && classification == "pod-phase";
            assert_eq!(records.len(), if filtered_unknown { 1 } else { 2 });
            let known = records
                .iter()
                .find(|record| record["name"] == "other")
                .unwrap();
            assert_eq!(known["active_consumer_count"], 1);
            assert_eq!(known["requested_bytes"], "107374182400");
            if utilization {
                assert_eq!(known["filesystem"]["status"], "ok");
                assert_eq!(
                    report.document["summary"]["filesystem"]["used_bytes"],
                    "42949672960"
                );
                assert_eq!(report.document["collection"]["cost"]["node_requests"], 1);
            }
            if filtered_unknown {
                assert_eq!(report.document["selection"]["unknown_count"], 1);
                assert_eq!(report.document["selection"]["complete"], false);
            } else {
                let affected = records
                    .iter()
                    .find(|record| record["name"] == "data")
                    .unwrap();
                assert_eq!(affected["requested_bytes"], "107374182400");
                assert_eq!(
                    affected["volume_mode"],
                    match classification {
                        "volume-mode" => json!("FutureMode"),
                        "invalid-volume-mode" => Value::Null,
                        _ => json!("Filesystem"),
                    }
                );
                if utilization {
                    assert_eq!(
                        affected["filesystem"]["status"],
                        if classification == "pod-phase" {
                            "missing"
                        } else {
                            "invalid"
                        }
                    );
                    assert!(affected["filesystem"]["used_bytes"].is_null());
                    assert_eq!(affected["filesystem"]["candidate_count"], 0);
                }
                if classification == "pod-phase" {
                    assert_eq!(affected["consumer_visibility"], "unknown");
                    assert!(affected["active_consumer_count"].is_null());
                    assert!(affected["terminating_consumer_count"].is_null());
                    let consumer = report.document["consumers"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|consumer| consumer["name"] == "app")
                        .unwrap();
                    assert_eq!(consumer["phase"], "FuturePhase");
                    assert_eq!(consumer["reference_state"], "unknown");
                    assert!(consumer["active"].is_null());
                    assert!(
                        !report.document["findings"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|finding| finding["subject_id"] == "pvc:data"
                                && finding["code"] == "no_active_consumer")
                    );
                }
            }
            for view in [
                crate::cli::View::Tree,
                crate::cli::View::Table,
                crate::cli::View::Summary,
                crate::cli::View::Issues,
            ] {
                assert!(
                    crate::output::text(&report, view, true, false).contains("CONTEXT example")
                );
            }
            assert_eq!(
                crate::output::csv_rows(&report.document).len(),
                records.len()
            );
            if mode == "strict-usage"
                && let Some(directory) = std::env::var_os("VIEW_STORAGE_TEST_REPORTS")
            {
                let directory = std::path::PathBuf::from(directory);
                std::fs::create_dir_all(&directory).unwrap();
                for (name, value) in [
                    (
                        format!("unsupported-{classification}.json"),
                        &report.document,
                    ),
                    (format!("unsupported-{classification}-input.json"), &input),
                ] {
                    std::fs::write(
                        directory.join(name),
                        serde_json::to_vec_pretty(value).unwrap(),
                    )
                    .unwrap();
                }
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn unknown_pod_lifecycle_keeps_independent_known_live_byte_metrics() {
    let mut input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let mut known = input["kubernetes"]["pods"][0].clone();
    known["metadata"]["name"] = "known-pod".into();
    known["metadata"]["uid"] = "known-pod".into();
    input["kubernetes"]["pods"][0]["status"]["phase"] = "FuturePhase".into();
    input["kubernetes"]["pods"]
        .as_array_mut()
        .unwrap()
        .push(known);
    input["summary_responses"]["worker-a"]["pods"][0]["podRef"]["name"] = "known-pod".into();
    input["summary_responses"]["worker-a"]["pods"][0]["podRef"]["uid"] = "known-pod".into();
    input["request"]["require_metrics"] = true.into();
    for health in [false, true] {
        input["request"]["fail_on"] = if health {
            json!("critical")
        } else {
            Value::Null
        };
        let cli = fixture_cli(&input);
        let query = cli.query("default").unwrap();
        let clock = Arc::new(TestClock::new(
            input["clock"].as_str().unwrap().parse().unwrap(),
        ));
        let collected = collect::collect(
            Arc::new(FixtureApi {
                bundle: input.clone(),
            }),
            clock,
            CancellationToken::new(),
            &cli,
            &query,
            "example".into(),
        )
        .await
        .unwrap();
        let report = finish_cancellable(collected, &query, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            report.exit,
            if health {
                Exit::Inconclusive
            } else {
                Exit::Success
            }
        );
        let record = &report.document["records"][0];
        assert_eq!(record["consumer_visibility"], "unknown");
        assert!(record["active_consumer_count"].is_null());
        assert_eq!(record["filesystem"]["status"], "ok");
        assert_eq!(record["filesystem"]["source"]["pod_id"], "pod:known-pod");
        assert_eq!(
            report.document["summary"]["filesystem"]["coverage"]["fresh_complete"],
            1
        );
        if !health && let Some(directory) = std::env::var_os("VIEW_STORAGE_TEST_REPORTS") {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            for (name, value) in [
                ("unsupported-shared-lifecycle.json", &report.document),
                ("unsupported-shared-lifecycle-input.json", &input),
            ] {
                std::fs::write(
                    directory.join(name),
                    serde_json::to_vec_pretty(value).unwrap(),
                )
                .unwrap();
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn malformed_inventory_and_sample_permutations_keep_diagnostics_and_reports_identical() {
    let mut original: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    original["kubernetes"]["pvcs"][0]["status"]["phase"] = "FuturePhase".into();
    original["kubernetes"]["pvcs"][0]["spec"]["resources"]["requests"]["storage"] =
        "invalid".into();
    let mut second = original["kubernetes"]["pvcs"][0].clone();
    second["metadata"]["name"] = "second".into();
    second["metadata"]["uid"] = "second".into();
    second["spec"].as_object_mut().unwrap().remove("volumeName");
    second["spec"]["storageClassName"] = json!(7);
    original["kubernetes"]["pvcs"]
        .as_array_mut()
        .unwrap()
        .push(second);
    let mut a = original["summary_responses"]["worker-a"]["pods"][0]["volume"][0].clone();
    let mut b = a.clone();
    a["usedBytes"] = json!(-1);
    b["availableBytes"] = json!(-1);
    original["summary_responses"]["worker-a"]["pods"][0]["volume"] = json!([a, b]);
    let mut expected: Option<Report> = None;
    for reverse_inventory in [false, true] {
        for reverse_samples in [false, true] {
            let mut input = original.clone();
            if reverse_inventory {
                input["kubernetes"]["pvcs"]
                    .as_array_mut()
                    .unwrap()
                    .reverse();
            }
            if reverse_samples {
                input["summary_responses"]["worker-a"]["pods"][0]["volume"]
                    .as_array_mut()
                    .unwrap()
                    .reverse();
            }
            let cli = fixture_cli(&input);
            let query = cli.query("default").unwrap();
            let clock = Arc::new(TestClock::new(
                input["clock"].as_str().unwrap().parse().unwrap(),
            ));
            let collected = collect::collect(
                Arc::new(FixtureApi { bundle: input }),
                clock,
                CancellationToken::new(),
                &cli,
                &query,
                "example".into(),
            )
            .await
            .unwrap();
            let report = finish_cancellable(collected, &query, CancellationToken::new())
                .await
                .unwrap();
            assert!(report.document["diagnostics"].as_array().unwrap().len() >= 6);
            if let Some(expected) = &expected {
                if let Some(difference) = difference(&report.document, &expected.document, "report")
                {
                    panic!("permutation changed report: {difference}");
                }
                assert_eq!(report.exit, expected.exit);
                for view in [
                    crate::cli::View::Tree,
                    crate::cli::View::Table,
                    crate::cli::View::Summary,
                    crate::cli::View::Issues,
                ] {
                    assert_eq!(
                        crate::output::text(&report, view, true, false),
                        crate::output::text(expected, view, true, false)
                    );
                }
                assert_eq!(
                    crate::output::csv_rows(&report.document),
                    crate::output::csv_rows(&expected.document)
                );
            }
            expected = Some(report);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn equal_time_and_source_candidates_produce_the_same_report_and_exit_in_any_order() {
    let original: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    for (values, expected_used, expected_available, expected_exit) in [
        ([(81, 19), (79, 21)], "79", "21", policy::Exit::Success),
        ([(40, 1), (40, 0)], "40", "0", policy::Exit::Findings),
    ] {
        let mut expected = None;
        for order in [[0, 1], [1, 0]] {
            let mut input = original.clone();
            input["request"]["fail_on"] = "warning".into();
            let sample = input["summary_responses"]["worker-a"]["pods"][0]["volume"][0].clone();
            input["summary_responses"]["worker-a"]["pods"][0]["volume"] = Value::Array(
                order
                    .map(|index| {
                        let mut value = sample.clone();
                        value["capacityBytes"] = 100.into();
                        value["usedBytes"] = values[index].0.into();
                        value["availableBytes"] = values[index].1.into();
                        value
                    })
                    .to_vec(),
            );
            let cli = fixture_cli(&input);
            let query = cli.query("default").unwrap();
            let clock = Arc::new(TestClock::new(
                input["clock"].as_str().unwrap().parse().unwrap(),
            ));
            let collected = collect::collect(
                Arc::new(FixtureApi { bundle: input }),
                clock,
                CancellationToken::new(),
                &cli,
                &query,
                "example".into(),
            )
            .await
            .unwrap();
            let report = finish(collected, &query).unwrap();
            if let Some((document, exit)) = &expected {
                if let Some(difference) = difference(&report.document, document, "report") {
                    panic!("equal-key input order changed report: {difference}");
                }
                assert_eq!(
                    &report.exit, exit,
                    "equal-key input order changed strict policy"
                );
            }
            expected = Some((report.document, report.exit));
        }
        let (document, exit) = expected.unwrap();
        assert_eq!(exit, expected_exit);
        assert_eq!(
            document["records"][0]["filesystem"]["used_bytes"],
            expected_used
        );
        assert_eq!(
            document["records"][0]["filesystem"]["available_bytes"],
            expected_available
        );
        assert_eq!(document["records"][0]["filesystem"]["candidate_count"], 2);
    }
}

#[tokio::test(start_paused = true)]
async fn all_api_scenarios_run_through_collector_and_shared_finalization() {
    for (name, input, expected, exit) in [
        scenario!("healthy", 0),
        scenario!("shared-rwx", 0),
        scenario!("partial-permissions", 0),
        scenario!("missing-stale", 0),
        scenario!("empty", 0),
        scenario!("unknown-selection", 4),
        scenario!("field-pressure", 4),
        scenario!("ambiguous-critical", 4),
        scenario!("terminating-consumer", 0),
    ] {
        let input: Value = serde_json::from_str(input).unwrap();
        let expected: Value = serde_json::from_str(expected).unwrap();
        let cli = fixture_cli(&input);
        let query = cli.query("default").unwrap();
        let clock = Arc::new(TestClock::new(
            input["clock"].as_str().unwrap().parse().unwrap(),
        ));
        let result = collect::collect(
            Arc::new(FixtureApi { bundle: input }),
            clock,
            CancellationToken::new(),
            &cli,
            &query,
            "example".into(),
        )
        .await
        .unwrap();
        let sync = finish(
            crate::collect::Collected {
                graph: result.graph.clone(),
                selection: result.selection.clone(),
                collection: result.collection.clone(),
            },
            &query,
        )
        .unwrap();
        let cancel = CancellationToken::new();
        let report = finish_cancellable(result, &query, cancel.clone())
            .await
            .unwrap();
        assert_eq!(
            sync.document, report.document,
            "{name}: checked finalization differs"
        );
        assert_eq!(sync.exit, report.exit);
        assert!(
            !cancel.is_cancelled(),
            "completed report cancelled its parent"
        );
        assert_eq!(report.exit as u8, exit, "{name}");
        for key in [
            "query",
            "selection",
            "records",
            "consumers",
            "relationships",
            "groups",
            "summary",
        ] {
            if let Some(difference) = difference(&report.document[key], &expected[key], key) {
                panic!("{name}: {difference}");
            }
        }
        let actual_codes: Vec<_> = report.document["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (&f["code"], &f["severity"], &f["subject_id"]))
            .collect();
        let expected_codes: Vec<_> = expected["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (&f["code"], &f["severity"], &f["subject_id"]))
            .collect();
        assert_eq!(actual_codes, expected_codes, "{name}: findings");
        if let Some(directory) = std::env::var_os("VIEW_STORAGE_TEST_REPORTS") {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join(format!("{name}.json")),
                serde_json::to_vec_pretty(&report.document).unwrap(),
            )
            .unwrap();
        }
    }
}

fn difference(actual: &Value, expected: &Value, path: &str) -> Option<String> {
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => {
            if a.len() != e.len() {
                return Some(format!(
                    "{path}: object keys {:?} != {:?}",
                    a.keys().collect::<Vec<_>>(),
                    e.keys().collect::<Vec<_>>()
                ));
            }
            for (key, value) in e {
                if let Some(difference) = difference(&a[key], value, &format!("{path}/{key}")) {
                    return Some(difference);
                }
            }
            None
        }
        (Value::Array(a), Value::Array(e)) => {
            if a.len() != e.len() {
                return Some(format!("{path}: length {} != {}", a.len(), e.len()));
            }
            for (index, (a, e)) in a.iter().zip(e).enumerate() {
                if let Some(difference) = difference(a, e, &format!("{path}/{index}")) {
                    return Some(difference);
                }
            }
            None
        }
        _ if actual == expected => None,
        _ => Some(format!("{path}: {actual} != {expected}")),
    }
}

#[tokio::test(start_paused = true)]
async fn explicitly_classless_and_unassigned_claims_have_distinct_filters_groups_and_wire_states() {
    let mut bundle: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let template = bundle["kubernetes"]["pvcs"][0].clone();
    let mut claims = vec![];
    for name in ["classless", "unassigned"] {
        let mut claim = template.clone();
        claim["metadata"]["uid"] = name.into();
        claim["metadata"]["name"] = name.into();
        claim["spec"].as_object_mut().unwrap().remove("volumeName");
        claim["spec"]
            .as_object_mut()
            .unwrap()
            .remove("storageClassName");
        if name == "classless" {
            claim["spec"]["storageClassName"] = "".into();
        }
        claim["status"] = json!({"phase":"Pending"});
        claims.push(claim);
    }
    bundle["kubernetes"]["pvcs"] = json!(claims);
    for resource in ["pvs", "pods", "storage_classes"] {
        bundle["kubernetes"][resource] = json!([]);
    }
    bundle["summary_responses"] = json!({});
    bundle["request"]["utilization"] = json!(false);
    bundle["request"]["metrics_source"] = Value::Null;
    for filter in [None, Some("")] {
        let mut input = bundle.clone();
        input["request"]["filters"]["storage_class"] = json!(filter);
        let cli = fixture_cli(&input);
        let query = cli.query("demo").unwrap();
        let collected = collect::collect(
            Arc::new(FixtureApi {
                bundle: input.clone(),
            }),
            Arc::new(TestClock::new(
                input["clock"].as_str().unwrap().parse().unwrap(),
            )),
            CancellationToken::new(),
            &cli,
            &query,
            "example".into(),
        )
        .await
        .unwrap();
        let report = finish(collected, &query).unwrap();
        assert_eq!(report.exit, Exit::Success);
        let records = report.document["records"].as_array().unwrap();
        assert_eq!(records.len(), if filter.is_none() { 2 } else { 1 });
        assert_eq!(records[0]["storage_class_state"], "classless");
        assert!(records[0]["storage_class"].is_null());
        if filter.is_none() {
            assert_eq!(records[1]["storage_class_state"], "unassigned");
            assert_eq!(report.document["groups"].as_array().unwrap().len(), 2);
            assert!(
                report.document["groups"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|group| group["value_state"] == "unassigned")
            );
            let text = crate::output::text(&report, crate::cli::View::Tree, false, false);
            assert!(text.contains("(classless)"));
            assert!(text.contains("(unassigned storage class)"));
            if let Some(directory) = std::env::var_os("VIEW_STORAGE_TEST_REPORTS") {
                let directory = std::path::PathBuf::from(directory);
                std::fs::create_dir_all(&directory).unwrap();
                for (name, value) in [
                    ("classless-unassigned.json", &report.document),
                    ("classless-unassigned-input.json", &input),
                ] {
                    std::fs::write(
                        directory.join(name),
                        serde_json::to_vec_pretty(value).unwrap(),
                    )
                    .unwrap();
                }
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn known_shared_volume_identity_retains_per_claim_metrics_and_suppresses_additive_totals() {
    let mut bundle: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    bundle["request"]["require_metrics"] = json!(true);
    bundle["summary_responses"]["worker-a"]["pods"][0]["volume"][0]["inodes"] = json!(1000);
    bundle["summary_responses"]["worker-a"]["pods"][0]["volume"][0]["inodesUsed"] = json!(950);
    bundle["summary_responses"]["worker-a"]["pods"][0]["volume"][0]["inodesFree"] = json!(50);
    bundle["kubernetes"]["storage_classes"][0]["volumeBindingMode"] = "WaitForFirstConsumer".into();
    let mut claim = bundle["kubernetes"]["pvcs"][0].clone();
    claim["metadata"]["uid"] = "second".into();
    claim["metadata"]["name"] = "second".into();
    claim["spec"]["volumeName"] = "pv-second".into();
    bundle["kubernetes"]["pvcs"]
        .as_array_mut()
        .unwrap()
        .push(claim);
    let mut pv = bundle["kubernetes"]["pvs"][0].clone();
    pv["metadata"]["uid"] = "pv-second".into();
    pv["metadata"]["name"] = "pv-second".into();
    pv["spec"]["claimRef"]["name"] = "second".into();
    pv["spec"]["claimRef"]["uid"] = "second".into();
    bundle["kubernetes"]["pvs"].as_array_mut().unwrap().push(pv);
    let mut pod = bundle["kubernetes"]["pods"][0].clone();
    pod["metadata"]["uid"] = "second-pod".into();
    pod["metadata"]["name"] = "second-pod".into();
    pod["spec"]["volumes"][0]["persistentVolumeClaim"]["claimName"] = "second".into();
    bundle["kubernetes"]["pods"]
        .as_array_mut()
        .unwrap()
        .push(pod);
    let mut sample = bundle["summary_responses"]["worker-a"]["pods"][0].clone();
    sample["podRef"]["uid"] = "second-pod".into();
    sample["podRef"]["name"] = "second-pod".into();
    sample["volume"][0]["pvcRef"]["name"] = "second".into();
    bundle["summary_responses"]["worker-a"]["pods"]
        .as_array_mut()
        .unwrap()
        .push(sample);
    for same_identity in [true, false] {
        let mut input = bundle.clone();
        if !same_identity {
            input["kubernetes"]["pvs"][1]["spec"]["csi"]["volumeHandle"] =
                "different-volume".into();
        }
        let cli = fixture_cli(&input);
        let query = cli.query("default").unwrap();
        let collected = collect::collect(
            Arc::new(FixtureApi {
                bundle: input.clone(),
            }),
            Arc::new(TestClock::new(
                input["clock"].as_str().unwrap().parse().unwrap(),
            )),
            CancellationToken::new(),
            &cli,
            &query,
            "example".into(),
        )
        .await
        .unwrap();
        let report = finish(collected, &query).unwrap();
        // Required metrics tests individual attributable tuples, even when
        // aggregating those complete tuples would count one filesystem twice.
        assert_eq!(report.exit, Exit::Success);
        let findings = report.document["findings"].as_array().unwrap();
        assert_eq!(findings.len(), 2);
        assert!(
            findings
                .iter()
                .all(|finding| finding["code"] == "inode_usage_high"
                    && finding["severity"] == "critical")
        );
        assert_eq!(report.document["summary"]["record_count"], 2);
        assert_eq!(
            report.document["summary"]["provisioned"]["known_sum_bytes"],
            "214748364800"
        );
        for record in report.document["records"].as_array().unwrap() {
            assert_eq!(record["filesystem"]["status"], "ok");
            assert_eq!(record["filesystem"]["used_bytes"], "42949672960");
            assert_eq!(record["overlapping_filesystem"] == true, same_identity);
            assert_eq!(record["volume_binding_mode"], "WaitForFirstConsumer");
        }
        for family in ["filesystem", "inodes"] {
            let total = &report.document["summary"][family];
            if same_identity {
                assert_eq!(total["reason"], "overlapping_filesystem");
                assert_eq!(total["coverage"]["by_reason"]["overlapping_filesystem"], 2);
                assert!(total["used_percent"].is_null());
            } else {
                assert_eq!(total["status"], "ok");
                assert_eq!(total["coverage"]["fresh_complete"], 2);
            }
        }
        let text = crate::output::text(&report, crate::cli::View::Summary, false, false);
        assert_eq!(text.contains("Known shared filesystems"), same_identity);
        if same_identity && let Some(directory) = std::env::var_os("VIEW_STORAGE_TEST_REPORTS") {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join("known-overlap.json"),
                serde_json::to_vec_pretty(&report.document).unwrap(),
            )
            .unwrap();
            std::fs::write(
                directory.join("known-overlap-input.json"),
                serde_json::to_vec_pretty(&input).unwrap(),
            )
            .unwrap();
        }
    }
}
