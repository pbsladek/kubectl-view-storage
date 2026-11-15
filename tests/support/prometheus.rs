//! Separate credentials, raw timestamps and conservative Prometheus accounting.
use super::http_reports::{bundle, mount_bundle};
use super::*;

fn matrix(age: i64, conflicting: bool, wrong_uid: bool) -> (Value, String) {
    let time = jiff::Timestamp::from_second(jiff::Timestamp::now().as_second() - age).unwrap();
    let mut result = vec![];
    for replica in ["one", "two"] {
        for (field, value) in [
            ("capacity_bytes", 10000),
            (
                "used_bytes",
                if conflicting && replica == "two" {
                    9500
                } else {
                    4000
                },
            ),
            (
                "available_bytes",
                if conflicting && replica == "two" {
                    500
                } else {
                    5500
                },
            ),
            ("inodes", 1000),
            ("inodes_used", 100),
            ("inodes_free", 850),
        ] {
            let mut labels = json!({"__name__":format!("kubelet_volume_stats_{field}"),"namespace":"demo","persistentvolumeclaim":"data","node":"worker-a","replica":replica,"cluster":"production"});
            if wrong_uid {
                labels["pvc_uid"] = "reused-claim-uid".into();
            }
            result
                .push(json!({"metric":labels,"values":[[time.as_second(),format!("{value}e0")]]}));
        }
    }
    (
        json!({"status":"success","data":{"resultType":"matrix","result":result}}),
        time.to_string(),
    )
}
fn args(url: &str) -> Vec<&str> {
    vec![
        "--metrics-source",
        "prometheus",
        "--prometheus-url",
        url,
        "--prometheus-cluster",
        "cluster=production",
        "-o",
        "json",
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prometheus_uses_separate_credentials_raw_timestamps_and_deduplicated_tuples() {
    let kube = MockServer::start().await;
    let prom = MockServer::start().await;
    let config = Kubeconfig::new(&kube);
    let mut kubeconfig: Value =
        serde_json::from_slice(&std::fs::read(&config.path).unwrap()).unwrap();
    kubeconfig["users"][0]["user"]["token"] = "PRIVATE_KUBERNETES_TOKEN".into();
    std::fs::write(&config.path, serde_json::to_vec(&kubeconfig).unwrap()).unwrap();
    let token = config.directory.join("prometheus-token");
    std::fs::write(&token, b"PRIVATE_PROMETHEUS_TOKEN\n").unwrap();
    mount_bundle(&kube, bundle("healthy")).await;
    let (response, time) = matrix(5, false, false);
    save_integration("prometheus-input", &response);
    Mock::given(method("GET"))
        .and(path("/api/v1/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .expect(1)
        .mount(&prom)
        .await;
    let url = prom.uri();
    let mut arguments = args(&url);
    arguments.extend([
        "--prometheus-token-file",
        token.to_str().unwrap(),
        "--require-metrics",
    ]);
    let output = config.run(&arguments);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    save_integration("prometheus", &report);
    let fs = &report["records"][0]["filesystem"];
    assert_eq!(fs["status"], "ok");
    assert_eq!(fs["observed_at"], time);
    assert_eq!(fs["source"]["provider"], "prometheus");
    assert!(fs["source"]["pod_id"].is_null());
    assert_eq!(fs["candidate_count"], 1);
    assert_eq!(fs["used_bytes"], "4000");
    assert_eq!(report["summary"]["filesystem"]["used_bytes"], "4000");
    assert_eq!(report["collection"]["cost"]["node_requests"], 0);
    assert_eq!(report["collection"]["cost"]["prometheus_requests"], 1);
    for request in kube.received_requests().await.unwrap() {
        assert!(!request.url.path().contains("/proxy/"));
        assert_eq!(
            request.headers.get("authorization").unwrap(),
            "Bearer PRIVATE_KUBERNETES_TOKEN"
        );
    }
    let requests = prom.received_requests().await.unwrap();
    assert_eq!(
        requests[0].headers.get("authorization").unwrap(),
        "Bearer PRIVATE_PROMETHEUS_TOKEN"
    );
    let query = requests[0]
        .url
        .query_pairs()
        .find(|(name, _)| name == "query")
        .unwrap()
        .1
        .into_owned();
    assert!(query.contains("cluster=\"production\"") && query.ends_with("[330000ms]"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE_"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_conflicting_and_wrong_identity_series_cannot_pass_required_metrics() {
    for (age, conflict, wrong_uid, status) in [
        (400, false, false, "stale"),
        (5, true, false, "uncertain"),
        (5, false, true, "identity_uncertain"),
    ] {
        let kube = MockServer::start().await;
        let prom = MockServer::start().await;
        let config = Kubeconfig::new(&kube);
        mount_bundle(&kube, bundle("healthy")).await;
        let (response, _) = matrix(age, conflict, wrong_uid);
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&prom)
            .await;
        let url = prom.uri();
        let mut arguments = args(&url);
        arguments.push("--require-metrics");
        let output = config.run(&arguments);
        assert_eq!(
            output.status.code(),
            Some(4),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["records"][0]["filesystem"]["status"], status);
        assert!(report["summary"]["filesystem"]["used_bytes"].is_null());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prometheus_failure_preserves_inventory_without_fallback_or_authenticated_redirects() {
    for status in [301, 401, 403, 503] {
        let kube = MockServer::start().await;
        let prom = MockServer::start().await;
        let redirect = MockServer::start().await;
        let config = Kubeconfig::new(&kube);
        mount_bundle(&kube, bundle("healthy")).await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("location", redirect.uri())
                    .set_body_string("PRIVATE_PROVIDER_ERROR"),
            )
            .mount(&prom)
            .await;
        let url = prom.uri();
        let mut arguments = args(&url);
        arguments.push("--require-metrics");
        let output = config.run(&arguments);
        assert_eq!(
            output.status.code(),
            Some(4),
            "status {status}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["records"].as_array().unwrap().len(), 1);
        assert!(redirect.received_requests().await.unwrap().is_empty());
        assert!(
            kube.received_requests()
                .await
                .unwrap()
                .iter()
                .all(|r| !r.url.path().contains("/proxy/"))
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE_PROVIDER_ERROR"));
        assert_eq!(
            prom.received_requests().await.unwrap().len(),
            if status == 503 { 3 } else { 1 }
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn block_volumes_and_empty_selections_do_not_contact_prometheus() {
    for block in [true, false] {
        let kube = MockServer::start().await;
        let prom = MockServer::start().await;
        let config = Kubeconfig::new(&kube);
        let mut input = bundle("healthy");
        if block {
            input["kubernetes"]["pvcs"][0]["spec"]["volumeMode"] = "Block".into();
            input["kubernetes"]["pvs"][0]["spec"]["volumeMode"] = "Block".into();
        }
        mount_bundle(&kube, input).await;
        let url = prom.uri();
        let mut arguments = args(&url);
        arguments.push("--require-metrics");
        if !block {
            arguments.extend(["--pvc", "absent"]);
        }
        let output = config.run(&arguments);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(prom.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_or_truncated_prometheus_results_keep_inventory_and_fail_strict_metrics() {
    for response in [
        json!({"status":"success","data":{"resultType":"vector","result":[]}}),
        json!({"status":"success","warnings":["result truncated"],"data":{"resultType":"matrix","result":[]}}),
    ] {
        let kube = MockServer::start().await;
        let prom = MockServer::start().await;
        let config = Kubeconfig::new(&kube);
        mount_bundle(&kube, bundle("healthy")).await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&prom)
            .await;
        let url = prom.uri();
        let mut arguments = args(&url);
        arguments.push("--require-metrics");
        let output = config.run(&arguments);
        assert_eq!(output.status.code(), Some(4));
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["records"].as_array().unwrap().len(), 1);
        assert_eq!(
            report["collection"]["sources"]
                .as_array()
                .unwrap()
                .iter()
                .find(|source| source["resource"] == "prometheus/query")
                .unwrap()["status"],
            "unavailable"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_prometheus_url_path_prefix_is_preserved() {
    let kube = MockServer::start().await;
    let prom = MockServer::start().await;
    let config = Kubeconfig::new(&kube);
    mount_bundle(&kube, bundle("healthy")).await;
    let (response, _) = matrix(5, false, false);
    Mock::given(method("GET"))
        .and(path("/monitoring/api/v1/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .expect(1)
        .mount(&prom)
        .await;
    let url = format!("{}/monitoring/", prom.uri());
    let mut arguments = args(&url);
    arguments.push("--require-metrics");
    let output = config.run(&arguments);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
