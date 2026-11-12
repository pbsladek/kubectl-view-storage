//! Calculation checks through the actual executable and Kubernetes HTTP adapter.
use super::http_reports::{bundle, mount_bundle};
use super::*;

fn input() -> Value {
    let mut input = bundle("healthy");
    input["kubernetes"]["pods"][0]["metadata"]["ownerReferences"] = json!([{"apiVersion":"apps/v1","kind":"StatefulSet","name":"database","uid":"db-uid","controller":true}]);
    input
}
fn report(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn save(name: &str, report: &Value) {
    if let Some(directory) = std::env::var_os("VIEW_STORAGE_HTTP_REPORTS") {
        let directory = std::path::PathBuf::from(directory).join("calculations");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join(format!("{name}.json")),
            serde_json::to_vec(report).unwrap(),
        )
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_stats_export_matches_class_type_and_statefulset_sums_in_every_format() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    let mut input = input();
    for (index, class, amount, mode, access) in [
        (1, "slow", 30, "Filesystem", "ReadWriteMany"),
        (2, "fast", 20, "Block", "ReadOnlyMany"),
    ] {
        let mut claim = input["kubernetes"]["pvcs"][0].clone();
        let name = format!("extra-{index}");
        claim["metadata"]["name"] = name.clone().into();
        claim["metadata"]["uid"] = name.clone().into();
        claim["spec"]["volumeName"] = name.clone().into();
        claim["spec"]["storageClassName"] = class.into();
        claim["spec"]["resources"]["requests"]["storage"] = format!("{amount}Gi").into();
        claim["spec"]["volumeMode"] = mode.into();
        claim["spec"]["accessModes"] = json!([access]);
        claim["status"]["capacity"]["storage"] = format!("{amount}Gi").into();
        let mut pv = input["kubernetes"]["pvs"][0].clone();
        pv["metadata"]["name"] = name.clone().into();
        pv["metadata"]["uid"] = name.clone().into();
        pv["spec"]["claimRef"]["name"] = name.clone().into();
        pv["spec"]["claimRef"]["uid"] = name.into();
        pv["spec"]["storageClassName"] = class.into();
        pv["spec"]["capacity"]["storage"] = format!("{amount}Gi").into();
        pv["spec"]["volumeMode"] = mode.into();
        input["kubernetes"]["pvcs"]
            .as_array_mut()
            .unwrap()
            .push(claim);
        input["kubernetes"]["pvs"].as_array_mut().unwrap().push(pv);
    }
    let mut class = input["kubernetes"]["storage_classes"][0].clone();
    class["metadata"]["name"] = "slow".into();
    input["kubernetes"]["storage_classes"]
        .as_array_mut()
        .unwrap()
        .push(class);
    mount_bundle(&server, input).await;
    let data = report(config.run(&["--view", "stats", "--units", "gb", "-o", "json"]));
    let calc = &data["calculations"];
    save("classes", &data);
    assert_eq!(
        calc["rows"][0]["statistics"]["requested"]["sum_bytes"],
        (150 * 1024u64.pow(3)).to_string()
    );
    assert_eq!(calc["storage_classes"]["named_count"], 2);
    assert_eq!(
        calc["storage_classes"]["statistics"]["requested"]["min_bytes"],
        (30 * 1024u64.pow(3)).to_string()
    );
    assert_eq!(
        calc["storage_classes"]["statistics"]["requested"]["max_bytes"],
        (120 * 1024u64.pow(3)).to_string()
    );
    assert_eq!(data["collection"]["cost"]["node_requests"], 0);
    assert_eq!(data["query"]["group_by"], json!(["storageclass"]));
    let types = report(config.run(&[
        "--view",
        "stats",
        "-g",
        "volumemode,accessmode",
        "-o",
        "json",
    ]));
    save("types", &types);
    assert_eq!(
        types["calculations"]["rows"][0]["statistics"]["requested"]["sum_bytes"],
        calc["rows"][0]["statistics"]["requested"]["sum_bytes"]
    );
    assert!(
        types["groups"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["value"] == "Block")
    );
    let scoped = report(config.run(&[
        "--view",
        "stats",
        "--statefulset",
        "database",
        "-g",
        "statefulset",
        "-o",
        "json",
    ]));
    save("statefulset", &scoped);
    assert_eq!(scoped["selection"]["matched_count"], 1);
    assert_eq!(
        scoped["calculations"]["rows"][0]["statistics"]["requested"]["sum_bytes"],
        (100 * 1024u64.pow(3)).to_string()
    );
    assert_eq!(scoped["consumers"][0]["statefulset"]["uid"], "db-uid");
    let text = config.run(&["--view", "stats", "--units", "GB"]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("161.06 GB"));
    assert!(text.contains("CLASS TOTALS") && text.contains("MEDIAN") && text.contains("P95"));
    let csv = config.run(&["--view", "stats", "--units", "GB", "-o", "csv"]);
    assert!(csv.status.success());
    let mut reader = csv::Reader::from_reader(csv.stdout.as_slice());
    assert_eq!(reader.headers().unwrap().get(0), Some("row_type"));
    let rows = reader
        .deserialize::<std::collections::BTreeMap<String, String>>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let requested = rows
        .iter()
        .find(|r| r["group"] == "TOTAL" && r["measure"] == "requested")
        .unwrap();
    assert_eq!(requested["sum_bytes"], (150 * 1024u64.pow(3)).to_string());
    assert_eq!(requested["sum"], "161.06 GB");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_actual_report_drives_fill_rate_without_double_sampling_or_external_storage() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    mount_bundle(&server, input()).await;
    let before = report(config.run(&[
        "--view",
        "stats",
        "--statefulset",
        "database",
        "-u",
        "-o",
        "json",
    ]));
    save("before", &before);
    let path = config.directory.join("before.json");
    std::fs::write(&path, serde_json::to_vec(&before).unwrap()).unwrap();
    server.reset().await;
    let mut after_input = input();
    let volume = &mut after_input["summary_responses"]["worker-a"]["pods"][0]["volume"][0];
    volume["time"] = jiff::Timestamp::now().to_string().into();
    let increase = 16 * 1024 * 1024u64;
    volume["usedBytes"] = json!(40 * 1024u64.pow(3) + increase);
    volume["availableBytes"] = json!(55 * 1024u64.pow(3) - increase);
    mount_bundle(&server, after_input).await;
    let after = report(config.run(&[
        "--view",
        "stats",
        "--statefulset",
        "database",
        "--compare",
        path.to_str().unwrap(),
        "-o",
        "json",
    ]));
    save("comparison", &after);
    assert_eq!(after["query"]["utilization"], true);
    assert_eq!(after["collection"]["cost"]["node_requests"], 1);
    let rate = &after["calculations"]["comparison"]["records"][0];
    assert_eq!(rate["status"], "growing");
    let numerator = rate["growth_bytes_per_second"]["numerator"]
        .as_str()
        .unwrap()
        .parse::<u128>()
        .unwrap();
    let denominator = rate["growth_bytes_per_second"]["denominator"]
        .as_str()
        .unwrap()
        .parse::<u128>()
        .unwrap();
    let elapsed = rate["interval_nanoseconds"]
        .as_str()
        .unwrap()
        .parse::<u128>()
        .unwrap();
    assert_eq!(
        numerator * elapsed,
        increase as u128 * 1_000_000_000 * denominator
    );
    assert_eq!(after["calculations"]["rows"][0]["growth"]["complete"], true);
    assert!(!rate["time_to_full_seconds"].is_null());
    for format in ["text", "csv"] {
        let output = config.run(&[
            "--view",
            "stats",
            "--statefulset",
            "database",
            "--compare",
            path.to_str().unwrap(),
            "-o",
            format,
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(if format == "text" {
            "GROWTH / HOUR"
        } else {
            "growth_numerator_bytes_per_second"
        }));
    }
    let invalid = config.run(&[
        "--view",
        "stats",
        "--compare",
        path.to_str().unwrap(),
        "-o",
        "json",
    ]);
    assert_eq!(invalid.status.code(), Some(1));
    assert!(invalid.stdout.is_empty());
    assert!(
        String::from_utf8(invalid.stderr)
            .unwrap()
            .contains("same selection filters")
    );
}
