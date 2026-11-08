//! Acceptance checks through the real SDK, executable and serialization boundary.
use super::*;
use std::{collections::BTreeMap, time::Duration};

pub(super) fn bundle(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/contract")
        .join(name)
        .join("input.json");
    let mut input: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    // Preserve relative freshness/conflict windows while the real process uses UTC.
    let original: jiff::Timestamp = input["clock"].as_str().unwrap().parse().unwrap();
    let now = jiff::Timestamp::now();
    let delta = now.as_nanosecond() - original.as_nanosecond();
    for summary in input["summary_responses"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        if let Some(pods) = summary["pods"].as_array_mut() {
            for pod in pods {
                if let Some(volumes) = pod["volume"].as_array_mut() {
                    for volume in volumes {
                        if let Some(value) = volume["time"].as_str() {
                            let old: jiff::Timestamp = value.parse().unwrap();
                            volume["time"] =
                                jiff::Timestamp::from_nanosecond(old.as_nanosecond() + delta)
                                    .unwrap()
                                    .to_string()
                                    .into();
                        }
                    }
                }
            }
        }
    }
    input["clock"] = now.to_string().into();
    input
}

pub(super) async fn mount_bundle(server: &MockServer, input: Value) {
    Mock::given(method("GET"))
        .respond_with(move |request: &wiremock::Request| {
            response_for_bundle(&input, request.url.path())
        })
        .mount(server)
        .await;
}

fn response_for_bundle(input: &Value, url: &str) -> ResponseTemplate {
    for failure in input["failures"].as_array().unwrap() {
        if failure["path"] == url {
            return ResponseTemplate::new(failure["status"].as_u64().unwrap() as u16)
                .set_body_string("untrusted-fixture-error-marker");
        }
    }
    let parts: Vec<_> = url.split('/').collect();
    if url.ends_with("/proxy/stats/summary") {
        return input["summary_responses"].get(parts[4]).map_or_else(
            || ResponseTemplate::new(404),
            |body| ResponseTemplate::new(200).set_body_json(body),
        );
    }
    let (resource, name, namespace) = if url.starts_with("/apis/storage.k8s.io/v1/") {
        ("storage_classes", parts.get(5).copied(), None)
    } else if parts.get(3) == Some(&"namespaces") {
        (
            parts.get(5).copied().unwrap_or(""),
            parts.get(6).copied(),
            parts.get(4).copied(),
        )
    } else {
        (
            parts.get(3).copied().unwrap_or(""),
            parts.get(4).copied(),
            None,
        )
    };
    let resource = match resource {
        "persistentvolumeclaims" => "pvcs",
        "persistentvolumes" => "pvs",
        "pods" => "pods",
        "storage_classes" => "storage_classes",
        _ => return ResponseTemplate::new(404),
    };
    let objects: Vec<_> = input["kubernetes"][resource]
        .as_array()
        .unwrap()
        .iter()
        .filter(|object| namespace.is_none_or(|ns| object["metadata"]["namespace"] == ns))
        .collect();
    if let Some(name) = name {
        objects
            .iter()
            .find(|object| object["metadata"]["name"] == name)
            .map_or_else(
                || ResponseTemplate::new(404),
                |object| ResponseTemplate::new(200).set_body_json(object),
            )
    } else {
        let wire_name = match resource {
            "pvcs" => "persistentvolumeclaims",
            "pvs" => "persistentvolumes",
            "storage_classes" => "storageclasses",
            other => other,
        };
        let key = format!("{wire_name}/{}", namespace.unwrap_or("cluster"));
        let version = input["resource_versions"]
            .get(&key)
            .cloned()
            .unwrap_or_else(|| "100".into());
        ResponseTemplate::new(200)
            .set_body_json(json!({"metadata":{"resourceVersion":version},"items":objects}))
    }
}

fn bundle_args(input: &Value) -> Vec<String> {
    let request = &input["request"];
    let mut args = if let Some(namespace) = request["namespace"].as_str() {
        vec!["-n".into(), namespace.into()]
    } else {
        vec!["-A".into()]
    };
    args.extend([
        "--kind".into(),
        request["kind"].as_str().unwrap().into(),
        "--group-by".into(),
        request["group_by"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>()
            .join(","),
    ]);
    if request["utilization"] == true {
        args.push("-u".into());
    }
    if request["require_metrics"] == true {
        args.push("--require-metrics".into());
    }
    for (key, flag) in [
        ("warn_used_percent", "--warn-used"),
        ("critical_used_percent", "--critical-used"),
        ("fail_on", "--fail-on"),
        ("sort", "--sort"),
    ] {
        if let Some(value) = request[key].as_str() {
            args.extend([flag.into(), value.into()]);
        }
    }
    for (key, flag) in [
        ("selector", "--selector"),
        ("pvc", "--pvc"),
        ("pv", "--pv"),
        ("storage_class", "--storage-class"),
        ("driver", "--driver"),
        ("node", "--node"),
    ] {
        if let Some(value) = request["filters"][key].as_str() {
            args.extend([flag.into(), value.into()]);
        }
    }
    args.extend([
        "--max-metrics-age".into(),
        format!("{}s", request["max_metrics_age_seconds"].as_u64().unwrap()),
    ]);
    args
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_contract_scenario_runs_through_real_http_and_all_six_outputs() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/contract");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(directory.join("manifest.json")).unwrap()).unwrap();
    for scenario in manifest["scenarios"].as_array().unwrap() {
        let name = scenario["id"].as_str().unwrap();
        let mut input = bundle(name);
        let server = MockServer::start().await;
        let config = Kubeconfig::new(&server);
        mount_bundle(&server, input.clone()).await;
        let args = bundle_args(&input);
        let run = |extra: &[&str]| {
            let mut all: Vec<_> = args.iter().map(String::as_str).collect();
            all.extend(extra);
            config.run(&all)
        };
        let exit = scenario["expected_exit"].as_i64().unwrap() as i32;
        let document = report(&run(&["-o", "json"]), exit);
        // The independent oracle needs the actual reference clock of the real
        // collection, alongside the exact rebased fixture bodies served above.
        input["clock"] = document["collection"]["finished_at"].clone();
        if let Some(destination) = std::env::var_os("VIEW_STORAGE_HTTP_REPORTS") {
            let destination = PathBuf::from(destination);
            std::fs::create_dir_all(&destination).unwrap();
            for (suffix, value) in [(".json", &document), ("-input.json", &input)] {
                std::fs::write(
                    destination.join(format!("{name}{suffix}")),
                    serde_json::to_vec_pretty(value).unwrap(),
                )
                .unwrap();
            }
        }
        let csv = csv_records(&run(&["-o", "csv"]), exit);
        let records = document["records"].as_array().unwrap();
        assert_eq!(csv.len(), records.len(), "{name}: CSV row count");
        for (row, record) in csv.iter().zip(records) {
            for field in [
                "kind",
                "name",
                "uid",
                "requested_bytes",
                "bound_bytes",
                "provisioned_bytes",
            ] {
                let expected = record[field].as_str().unwrap_or("");
                assert_eq!(row[field], expected, "{name}: CSV {field}");
            }
        }
        for (view, relative) in scenario["views"].as_object().unwrap() {
            let mut extra = vec!["--view", view.as_str()];
            extra.extend(
                scenario["view_options"][view]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap()),
            );
            let output = run(&extra);
            assert_eq!(
                output.status.code(),
                Some(exit),
                "{name}/{view}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let expected = std::fs::read_to_string(directory.join(relative.as_str().unwrap()))
                .unwrap()
                .replacen("Context: example", "Context: fixture", 1);
            let actual = String::from_utf8(output.stdout).unwrap();
            assert!(actual.ends_with('\n'));
            if let Some(timestamp) =
                document["summary"]["filesystem"]["newest_observed_at"].as_str()
            {
                assert!(
                    actual.contains(&format!("Latest sample: {timestamp}")),
                    "{name}/{view}: actual observation timestamp must be displayed"
                );
            }

            if view == "issues" {
                let displayed: Vec<_> = actual
                    .lines()
                    .filter_map(|line| {
                        line.strip_prefix("Collection note [")
                            .and_then(|text| text.split_once(']'))
                            .map(|(reason, _)| reason)
                    })
                    .collect();
                let exported: Vec<_> = document["diagnostics"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|diagnostic| diagnostic["reason"].as_str().unwrap())
                    .collect();
                assert_eq!(
                    displayed, exported,
                    "{name}: issue diagnostics must agree with JSON in canonical order"
                );
            }
            // The independent JSON oracle validates exact field evidence and
            // diagnostics. The text fixture fixes data rows, scope, coverage,
            // finding subjects/severities/codes and their order; its older
            // illustrative diagnostic/evidence prose is not an exact API.
            let normalize = |text: &str| {
                text.lines()
                    .filter(|line| !line.starts_with("Collection note ["))
                    .filter(|line| view != "issues" || !line.starts_with("  "))
                    .map(|line| {
                        if line.starts_with("Latest sample:") {
                            "Latest sample:".to_owned()
                        } else {
                            line.to_owned()
                        }
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(normalize(&actual), normalize(&expected), "{name}/{view}");
        }
    }
}

fn report(output: &Output, exit: i32) -> Value {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("untrusted-fixture-error-marker"));
    serde_json::from_slice(&output.stdout).unwrap()
}

fn csv_records(output: &Output, exit: i32) -> Vec<BTreeMap<String, String>> {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut reader = csv::Reader::from_reader(output.stdout.as_slice());
    let headers = reader.headers().unwrap().clone();
    assert_eq!(headers.len(), 27);
    reader
        .records()
        .map(|row| {
            headers
                .iter()
                .map(str::to_owned)
                .zip(row.unwrap().iter().map(str::to_owned))
                .collect()
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_cluster_namespace_and_all_namespaces_have_distinct_real_paths() {
    for args in [vec!["-n", "cluster"], vec!["-A"]] {
        let server = MockServer::start().await;
        let config = Kubeconfig::new(&server);
        mount_bundle(&server, bundle("empty")).await;
        let mut flags = args.clone();
        flags.extend(["--require-metrics", "--fail-on", "warning", "-o", "json"]);
        let result = report(&config.run(&flags), 0);
        assert_eq!(result["selection"]["complete"], true);
        assert_eq!(result["selection"]["base_count"], 0);
        assert_eq!(result["summary"]["filesystem"]["coverage"]["eligible"], 0);
        assert_eq!(
            result["summary"]["filesystem"]["coverage"]["fresh_complete"],
            0
        );
        assert_eq!(result["collection"]["cost"]["node_requests"], 0);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].url.path(),
            if args[0] == "-A" {
                "/api/v1/persistentvolumeclaims"
            } else {
                "/api/v1/namespaces/cluster/persistentvolumeclaims"
            }
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forbidden_base_or_second_page_never_emits_a_partial_success_document() {
    for page_two in [false, true] {
        let server = MockServer::start().await;
        let config = Kubeconfig::new(&server);
        let route = "/api/v1/namespaces/demo/persistentvolumeclaims";
        if page_two {
            Mock::given(method("GET"))
                .and(path(route))
                .and(wiremock::matchers::query_param_is_missing("continue"))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    json!({"metadata":{"resourceVersion":"1","continue":"second-page"},"items":[]}),
                ))
                .expect(1)
                .mount(&server)
                .await;
        }
        let mut denied = Mock::given(method("GET")).and(path(route));
        if page_two {
            denied = denied.and(query_param("continue", "second-page"));
        }
        denied
            .respond_with(ResponseTemplate::new(403).set_body_string("private-error-marker"))
            .expect(1)
            .mount(&server)
            .await;
        let output = config.run(&["-o", "json"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("403"), "{stderr}");
        assert!(!stderr.contains("private-error-marker"));
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            if page_two { 2 } else { 1 }
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn distinct_capacities_and_raw_block_survive_real_json_and_csv_exports() {
    for kind in ["pvc", "pv"] {
        for block in [false, true] {
            let server = MockServer::start().await;
            let config = Kubeconfig::new(&server);
            let mut input = bundle("healthy");
            input["kubernetes"]["pvcs"][0]["status"]["capacity"]["storage"] = "103Gi".into();
            input["kubernetes"]["pvs"][0]["spec"]["capacity"]["storage"] = "120Gi".into();
            let sample = &mut input["summary_responses"]["worker-a"]["pods"][0]["volume"][0];
            sample["capacityBytes"] = json!(118 * 1024u64.pow(3));
            sample["usedBytes"] = json!(47 * 1024u64.pow(3));
            sample["availableBytes"] = json!(60 * 1024u64.pow(3));
            if block {
                input["kubernetes"]["pvcs"][0]["spec"]["volumeMode"] = "Block".into();
                input["kubernetes"]["pvs"][0]["spec"]["volumeMode"] = "Block".into();
                input["kubernetes"]["pods"][0]["spec"]["containers"] = json!([{
                    "name":"block", "image":"example.invalid/probe:test",
                    "volumeDevices":[{"name":"data","devicePath":"/dev/test-data"}]
                }]);
            }
            mount_bundle(&server, input).await;
            let scope = if kind == "pv" {
                vec!["-A"]
            } else {
                vec!["-n", "demo"]
            };
            let mut args = scope;
            args.extend(["--kind", kind, "--require-metrics", "-o", "json"]);
            let document = report(&config.run(&args), 0);
            let record = &document["records"][0];
            assert_eq!(document["relationships"].as_array().unwrap().len(), 1);
            assert_eq!(document["summary"]["record_count"], 1);
            for (field, amount) in [
                ("requested_bytes", 100),
                ("bound_bytes", 103),
                ("provisioned_bytes", 120),
            ] {
                assert_eq!(record[field], (amount * 1024u64.pow(3)).to_string());
            }
            assert_eq!(document["summary"]["filesystem"]["physical_total"], false);
            assert_eq!(
                document["collection"]["cost"]["node_requests"],
                if block { 0 } else { 1 }
            );
            if block {
                assert_eq!(record["filesystem"]["status"], "not_applicable");
                assert_eq!(record["inodes"]["status"], "not_applicable");
                assert!(record["filesystem"]["used_bytes"].is_null());
                assert_eq!(document["relationships"][0]["mounts"][0]["kind"], "block");
            } else {
                assert_eq!(
                    record["filesystem"]["available_bytes"],
                    (60 * 1024u64.pow(3)).to_string()
                );
                let mounts = document["relationships"][0]["mounts"].as_array().unwrap();
                assert_eq!(mounts.len(), 2);
                assert_eq!(mounts[1]["sub_path_expr"], "$(POD_NAME)");
                assert_eq!(mounts[1]["read_only"], true);
            }
            *args.last_mut().unwrap() = "csv";
            let rows = csv_records(&config.run(&args), 0);
            assert_eq!(rows.len(), 1);
            for (field, amount) in [
                ("requested_bytes", 100),
                ("bound_bytes", 103),
                ("provisioned_bytes", 120),
            ] {
                assert_eq!(rows[0][field], (amount * 1024u64.pow(3)).to_string());
            }
            for (field, amount) in [
                ("filesystem_capacity_bytes", 118),
                ("used_bytes", 47),
                ("available_bytes", 60),
            ] {
                assert_eq!(
                    rows[0][field],
                    if block {
                        String::new()
                    } else {
                        (amount * 1024u64.pow(3)).to_string()
                    }
                );
            }
            let requests = server.received_requests().await.unwrap();
            assert_eq!(
                requests
                    .iter()
                    .filter(|request| request.url.path().ends_with("/proxy/stats/summary"))
                    .count(),
                if block { 0 } else { 2 }
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incomplete_warning_exports_preserve_zero_fields_and_raw_pressure_evidence() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    let mut input = bundle("field-pressure");
    input["failures"].as_array_mut().unwrap().push(json!({
        "path":"/apis/storage.k8s.io/v1/storageclasses/fast", "status":403
    }));
    mount_bundle(&server, input).await;
    let output = config.run(&["--require-metrics", "--fail-on", "critical", "-o", "json"]);
    let document = report(&output, 4);
    let findings = document["findings"].as_array().unwrap();
    for code in [
        "filesystem_space_exhausted",
        "filesystem_inodes_exhausted",
        "filesystem_usage_high",
    ] {
        assert!(findings.iter().any(|finding| finding["code"] == code));
    }
    for finding in findings {
        for evidence in finding["evidence"].as_array().unwrap() {
            assert_eq!(evidence["source"]["provider"], "kubelet");
            assert!(evidence["observed_at"].as_str().is_some());
        }
    }
    let records = document["records"].as_array().unwrap();
    let exhausted = records
        .iter()
        .find(|record| record["name"] == "exhausted")
        .unwrap();
    assert_eq!(exhausted["filesystem"]["available_bytes"], "0");
    assert_eq!(exhausted["inodes"]["inodes_free"], "0");
    let partial = records
        .iter()
        .find(|record| record["name"] == "partial-alert")
        .unwrap();
    assert!(partial["filesystem"]["available_bytes"].is_null());
    let output = config.run(&["--require-metrics", "--fail-on", "critical", "-o", "csv"]);
    let rows = csv_records(&output, 4);
    assert!(!output.stderr.is_empty());
    assert_eq!(
        rows.iter().find(|row| row["name"] == "exhausted").unwrap()["available_bytes"],
        "0"
    );
    assert_eq!(
        rows.iter()
            .find(|row| row["name"] == "partial-alert")
            .unwrap()["available_bytes"],
        ""
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn denied_pods_cannot_turn_unknown_node_selection_into_a_strict_pass() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    mount_bundle(&server, bundle("unknown-selection")).await;
    for strict in [vec!["--require-metrics"], vec!["--fail-on", "warning"]] {
        let mut args = vec!["--node", "worker-a", "-o", "json"];
        args.extend(strict);
        let document = report(&config.run(&args), 4);
        assert_eq!(document["selection"]["complete"], false);
        assert_eq!(document["selection"]["unknown_count"], 1);
        assert_eq!(document["selection"]["excluded_count"], 0);
        assert!(
            document["findings"]
                .as_array()
                .unwrap()
                .iter()
                .all(|finding| finding["code"] != "no_consumers")
        );
        assert_eq!(document["collection"]["cost"]["node_requests"], 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_conflicting_critical_samples_remain_inconclusive_and_retain_candidates() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    mount_bundle(&server, bundle("ambiguous-critical")).await;
    let document = report(
        &config.run(&["-u", "--fail-on", "critical", "-o", "json"]),
        4,
    );
    let family = &document["records"][0]["filesystem"];
    assert_eq!(family["candidate_count"], 2);
    assert_eq!(family["status"], "uncertain");
    assert_eq!(family["used_percent"], 99.0);
    assert_eq!(
        family["observed_range_percent"],
        json!({"min":90.0,"max":99.0})
    );
    assert!(document["summary"]["filesystem"]["used_percent"].is_null());
    assert_eq!(family["candidates"].as_array().unwrap().len(), 2);
    assert!(
        document["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["code"] == "metrics_disagreement")
    );
    assert!(
        !document["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["code"] == "filesystem_usage_high")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_timed_out_node_preserves_other_node_observations_and_partial_totals() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    let mut input = bundle("healthy");
    let mut claim = input["kubernetes"]["pvcs"][0].clone();
    claim["metadata"]["uid"] = "other".into();
    claim["metadata"]["name"] = "other".into();
    claim["spec"]["volumeName"] = "pv-other".into();
    input["kubernetes"]["pvcs"]
        .as_array_mut()
        .unwrap()
        .push(claim);
    let mut volume = input["kubernetes"]["pvs"][0].clone();
    volume["metadata"]["uid"] = "uid-pv-other".into();
    volume["metadata"]["name"] = "pv-other".into();
    volume["spec"]["claimRef"]["name"] = "other".into();
    volume["spec"]["claimRef"]["uid"] = "other".into();
    volume["spec"]["csi"]["volumeHandle"] = "handle-other".into();
    input["kubernetes"]["pvs"]
        .as_array_mut()
        .unwrap()
        .push(volume);
    let mut pod = input["kubernetes"]["pods"][0].clone();
    pod["metadata"]["uid"] = "other-app".into();
    pod["metadata"]["name"] = "other-app".into();
    pod["spec"]["nodeName"] = "worker-b".into();
    pod["spec"]["volumes"][0]["persistentVolumeClaim"]["claimName"] = "other".into();
    input["kubernetes"]["pods"]
        .as_array_mut()
        .unwrap()
        .push(pod);
    mount_bundle(&server, input).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/nodes/worker-b/proxy/stats/summary"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(500)))
        .with_priority(1)
        .expect(2..=6)
        .mount(&server)
        .await;
    for (strict, exit) in [(false, 0), (true, 4)] {
        let before = server.received_requests().await.unwrap().len();
        let mut args = vec!["-u", "--request-timeout", "100ms", "-o", "json"];
        if strict {
            args.push("--require-metrics");
        }
        let document = report(&config.run(&args), exit);
        assert_eq!(document["selection"]["matched_count"], 2);
        assert_eq!(
            document["summary"]["requested"]["known_sum_bytes"],
            (200 * 1024u64.pow(3)).to_string()
        );
        assert_eq!(document["summary"]["filesystem"]["status"], "partial");
        assert_eq!(
            document["summary"]["filesystem"]["capacity_bytes"],
            (100 * 1024u64.pow(3)).to_string()
        );
        assert_eq!(
            document["summary"]["filesystem"]["coverage"]["fresh_complete"],
            1
        );
        let requests = server.received_requests().await.unwrap();
        let node_requests: Vec<_> = requests[before..]
            .iter()
            .filter(|request| request.url.path().ends_with("/proxy/stats/summary"))
            .collect();
        assert_eq!(document["collection"]["cost"]["node_targets"], 2);
        assert_eq!(
            document["collection"]["cost"]["node_requests"],
            node_requests.len()
        );
        assert_eq!(
            node_requests
                .iter()
                .filter(|request| request.url.path().contains("/worker-a/"))
                .count(),
            1
        );
        // The SDK read timeout and outer deadline can win in either order.
        // A retryable SDK failure permits at most three attempts; an outer
        // timeout stops immediately. Cost must match the actual wire requests.
        let unavailable_attempts = node_requests
            .iter()
            .filter(|request| request.url.path().contains("/worker-b/"))
            .count();
        assert!((1..=3).contains(&unavailable_attempts));
        assert_eq!(document["records"][0]["filesystem"]["status"], "ok");
        assert_ne!(document["records"][1]["filesystem"]["status"], "ok");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn optional_oversized_success_and_error_bodies_keep_inventory_without_retrying() {
    for status in [200, 503] {
        let server = MockServer::start().await;
        let config = Kubeconfig::new(&server);
        mount_bundle(&server, bundle("healthy")).await;
        let body = vec![b' '; kubectl_view_storage::transport::MAX_RESPONSE_BYTES + 1];
        Mock::given(method("GET"))
            .and(path("/api/v1/nodes/worker-a/proxy/stats/summary"))
            .respond_with(ResponseTemplate::new(status).set_body_bytes(body))
            .with_priority(1)
            .expect(2)
            .mount(&server)
            .await;
        for (strict, exit) in [(false, 0), (true, 4)] {
            let mut args = vec!["-u", "-o", "json"];
            if strict {
                args.push("--require-metrics");
            }
            let document = report(&config.run_with_timeout(&args, "20s"), exit);
            assert_eq!(document["selection"]["matched_count"], 1);
            assert_eq!(document["records"][0]["requested_bytes"], "107374182400");
            assert_eq!(
                document["records"][0]["filesystem"]["reason"], "response_too_large",
                "status={status}, consumed cost={}",
                document["collection"]["cost"]
            );
            assert!(document["records"][0]["filesystem"]["used_bytes"].is_null());
            assert_eq!(document["collection"]["cost"]["node_requests"], 1);
            assert!(
                document["collection"]["cost"]["response_bytes"]
                    .as_u64()
                    .unwrap()
                    > kubectl_view_storage::transport::MAX_RESPONSE_BYTES as u64
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn out_of_range_counter_does_not_suppress_independent_zero_pressure_in_json_or_csv() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    let mut input = bundle("healthy");
    let sample = &mut input["summary_responses"]["worker-a"]["pods"][0]["volume"][0];
    sample["usedBytes"] = serde_json::from_str("18446744073709551616").unwrap();
    sample["availableBytes"] = 0.into();
    sample["inodesFree"] = 0.into();
    mount_bundle(&server, input).await;
    let document = report(
        &config.run(&["--require-metrics", "--fail-on", "critical", "-o", "json"]),
        4,
    );
    assert!(document["records"][0]["filesystem"]["used_bytes"].is_null());
    assert_eq!(
        document["records"][0]["filesystem"]["field_status"]["used_bytes"],
        "invalid"
    );
    assert_eq!(document["records"][0]["filesystem"]["available_bytes"], "0");
    assert!(!document["diagnostics"].as_array().unwrap().is_empty());
    for code in ["filesystem_space_exhausted", "filesystem_inodes_exhausted"] {
        assert!(
            document["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|finding| finding["code"] == code && finding["severity"] == "critical")
        );
    }
    let rows = csv_records(
        &config.run(&["--require-metrics", "--fail-on", "critical", "-o", "csv"]),
        4,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["used_bytes"], "");
    assert_eq!(rows[0]["available_bytes"], "0");
    assert_eq!(rows[0]["inodes_free"], "0");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_retries_are_owned_and_counted_by_the_collector_without_hidden_sdk_attempts() {
    for status in [429, 503, 504] {
        let server = MockServer::start().await;
        let config = Kubeconfig::new(&server);
        let input = bundle("healthy");
        let summary = input["summary_responses"]["worker-a"].clone();
        mount_bundle(&server, input).await;
        let attempts = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = attempts.clone();
        Mock::given(method("GET"))
            .and(path("/api/v1/nodes/worker-a/proxy/stats/summary"))
            .respond_with(move |_: &wiremock::Request| {
                if counter.fetch_add(1, Ordering::Relaxed) < 2 {
                    ResponseTemplate::new(status)
                        .insert_header("Retry-After", "0")
                        .set_body_string("retry-body-not-exported")
                } else {
                    ResponseTemplate::new(200).set_body_json(&summary)
                }
            })
            .with_priority(1)
            .expect(3)
            .mount(&server)
            .await;
        let output = config.run(&["--require-metrics", "-o", "json"]);
        let document = report(&output, 0);
        assert_eq!(attempts.load(Ordering::Relaxed), 3);
        assert_eq!(document["collection"]["cost"]["node_targets"], 1);
        assert_eq!(document["collection"]["cost"]["node_requests"], 3);
        assert_eq!(document["records"][0]["filesystem"]["status"], "ok");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("retry-body-not-exported"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("retry-body-not-exported"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unfamiliar_pv_phase_and_volume_mode_remain_open_values_under_strict_queries() {
    for mode_case in [false, true] {
        let server = MockServer::start().await;
        let config = Kubeconfig::new(&server);
        let mut input = bundle("healthy");
        input["kubernetes"]["pvs"][0]["future_extra"] = json!({"provider":"future"});
        if mode_case {
            input["kubernetes"]["pvs"][0]["spec"]["volumeMode"] = "FutureVolumeMode".into();
        } else {
            input["kubernetes"]["pvs"][0]["status"]["phase"] = "FuturePhase".into();
        }
        mount_bundle(&server, input).await;
        for strict in [false, true] {
            let mut args = vec!["-A", "--kind", "pv", "-o", "json"];
            if strict {
                args.extend(if mode_case {
                    vec!["--require-metrics"]
                } else {
                    vec!["--fail-on", "warning"]
                });
            }
            let document = report(&config.run(&args), if strict { 4 } else { 0 });
            assert_eq!(document["selection"]["matched_count"], 1);
            let record = &document["records"][0];
            assert_eq!(record["provisioned_bytes"], "107374182400");
            assert_eq!(record["driver"], "example.csi.test");
            if mode_case {
                assert_eq!(record["volume_mode"], "FutureVolumeMode");
                if strict {
                    assert_ne!(record["filesystem"]["status"], "ok");
                }
            } else {
                assert_eq!(record["phase"], "FuturePhase");
            }
            assert!(!document["diagnostics"].as_array().unwrap().is_empty());
        }
    }
}
