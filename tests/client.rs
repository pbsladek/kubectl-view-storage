//! Exercise the real executable, kubeconfig loader, HTTP adapter, and policy together.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};
#[path = "support/budget.rs"]
mod budget_scenario;
#[path = "support/calculations.rs"]
mod calculations;
#[cfg(unix)]
#[path = "support/exec_lifecycle.rs"]
mod exec_lifecycle;
#[path = "support/http_reports.rs"]
mod http_reports;

struct Kubeconfig {
    directory: PathBuf,
    path: PathBuf,
}
impl Kubeconfig {
    fn new(server: &MockServer) -> Self {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "view-storage-client-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("config.json");
        let value = json!({"apiVersion":"v1","kind":"Config","clusters":[{"name":"cluster","cluster":{"server":server.uri()}}],"users":[{"name":"test","user":{}}],"contexts":[{"name":"fixture","context":{"cluster":"cluster","user":"test","namespace":"demo"}},{"name":"other","context":{"cluster":"cluster","user":"test","namespace":"other"}}],"current-context":"fixture"});
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        Self { directory, path }
    }
    fn run(&self, args: &[&str]) -> Output {
        self.run_with_timeout(args, "3s")
    }
    fn run_with_timeout(&self, args: &[&str], timeout: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kubectl-view_storage"))
            .arg("--kubeconfig")
            .arg(&self.path)
            .args(["--timeout", timeout])
            .args(args)
            .env("RUST_LOG", "trace")
            .output()
            .unwrap()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_denied_bulk_falls_back_once_and_stops_at_the_shared_budget() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    for (resource, body) in [
        ("persistentvolumeclaims", budget_scenario::claims()),
        ("pods", budget_scenario::pods()),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/{resource}")))
            .and(query_param("limit", "500"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/v1/persistentvolumes"))
        .and(query_param("limit", "500"))
        .respond_with(ResponseTemplate::new(403).set_body_json(budget_scenario::denied()))
        .expect(1)
        .mount(&server)
        .await;
    // These permissions exist for every object, including the undispatched
    // remainder. The shared attempt budget alone must stop further reads.
    for index in 0..budget_scenario::CLAIMS {
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/persistentvolumes/pv-{index:03}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(budget_scenario::volume(index)))
            .expect(if index < budget_scenario::TARGETED_GETS {
                1
            } else {
                0
            })
            .mount(&server)
            .await;
    }
    let output = config.run_with_timeout(&["-A", "-o", "json"], "60s");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    budget_scenario::assert_report(&report);
    let actual: Vec<_> = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .map(|request| {
            request.url.query().map_or_else(
                || request.url.path().to_owned(),
                |query| format!("{}?{query}", request.url.path()),
            )
        })
        .collect();
    assert_eq!(actual, budget_scenario::expected_paths());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("forbidden") && stderr.contains("budget"));
}
impl Drop for Kubeconfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn current_and_explicit_context_namespaces_use_real_http_without_usage() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    for namespace in ["demo", "other"] {
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/namespaces/{namespace}/persistentvolumeclaims"
            )))
            .and(query_param("limit", "500"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"metadata":{"resourceVersion":"1"},"items":[]})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    for args in [vec!["-o", "json"], vec!["--context", "other", "-o", "json"]] {
        let output = config.run(&args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["selection"]["matched_count"], 0);
        assert_eq!(report["query"]["utilization"], false);
        assert_eq!(output.stdout.last(), Some(&b'\n'));
        assert!(output.stderr.is_empty());
        if args.contains(&"other") {
            assert_eq!(report["collection"]["scope"]["namespace"], "other");
            assert_eq!(report["collection"]["context"], "other");
        } else {
            assert_eq!(report["collection"]["scope"]["namespace"], "demo");
        }
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[cfg(unix)]
fn install_failing_helper(config: &Kubeconfig, mode: Option<&str>) -> PathBuf {
    let marker = config.directory.join("helper-invoked");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&config.path).unwrap()).unwrap();
    let mut helper = json!({
        "apiVersion":"client.authentication.k8s.io/v1beta1",
        "command":"/bin/sh",
        "args":["-c", "printf '%s' invoked > \"$1\"; printf '%s' credential-helper-secret-marker >&2; printf '%s' credential-helper-stdout-marker; exit 7", "fixture", marker],
    });
    if let Some(mode) = mode {
        helper["interactiveMode"] = mode.into();
    }
    value["users"][0]["user"] = json!({"exec":helper});
    std::fs::write(&config.path, serde_json::to_vec(&value).unwrap()).unwrap();
    marker
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn credential_helper_failure_on_nonterminal_input_never_exposes_payloads() {
    for mode in [None, Some("IfAvailable"), Some("Never")] {
        let server = MockServer::start().await;
        let config = Kubeconfig::new(&server);
        let marker = install_failing_helper(&config, mode);
        let output = config.run(&["-o", "json"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("credential-helper-secret-marker"),
            "mode={mode:?}"
        );
        assert!(!stderr.contains("credential-helper-stdout-marker"));
        assert!(stderr.contains("credential-helper"));
        assert!(marker.exists());
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn credential_helper_requiring_a_terminal_is_rejected_before_execution() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    let marker = install_failing_helper(&config, Some("Always"));
    let output = config.run(&["-o", "json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        !marker.exists(),
        "interactive helper ran without terminal input"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("terminal"));
    assert!(!stderr.contains("credential-helper-secret-marker"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn credential_helper_receives_noninteractive_exec_info_and_authenticates_real_http() {
    for (api_version, mode) in [
        ("client.authentication.k8s.io/v1beta1", None),
        ("client.authentication.k8s.io/v1beta1", Some("IfAvailable")),
        ("client.authentication.k8s.io/v1beta1", Some("Never")),
        ("client.authentication.k8s.io/v1", Some("Never")),
        ("client.authentication.k8s.io/v1", Some("IfAvailable")),
    ] {
        let server = MockServer::start().await;
        let config = Kubeconfig::new(&server);
        let mut value: Value =
            serde_json::from_slice(&std::fs::read(&config.path).unwrap()).unwrap();
        let mut helper = json!({
            "apiVersion":api_version, "command":"/bin/sh",
            "args":["-c", "case \"$KUBERNETES_EXEC_INFO\" in *'\"interactive\":false'*) ;; *) exit 9 ;; esac; test \"$FIXTURE_ENV\" = fixture-value || exit 8; printf '{\"apiVersion\":\"%s\",\"kind\":\"ExecCredential\",\"status\":{\"token\":\"fixture-token-not-a-real-credential\"}}' \"$FIXTURE_API_VERSION\""],
            "env":[{"name":"FIXTURE_ENV","value":"fixture-value"},{"name":"FIXTURE_API_VERSION","value":api_version}]
        });
        if let Some(mode) = mode {
            helper["interactiveMode"] = mode.into();
        }
        value["users"][0]["user"] = json!({"exec":helper});
        std::fs::write(&config.path, serde_json::to_vec(&value).unwrap()).unwrap();
        Mock::given(method("GET"))
            .and(path("/api/v1/namespaces/demo/persistentvolumeclaims"))
            .and(wiremock::matchers::header(
                "authorization",
                "Bearer fixture-token-not-a-real-credential",
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"metadata":{"resourceVersion":"1"},"items":[]})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let output = config.run(&["-o", "json"]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["selection"]["complete"], true);
        assert!(output.stderr.is_empty());
        assert!(
            !String::from_utf8_lossy(&output.stdout)
                .contains("fixture-token-not-a-real-credential")
        );
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn credential_helper_v1_requires_an_explicit_input_contract() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    let marker = install_failing_helper(&config, None);
    let mut value: Value = serde_json::from_slice(&std::fs::read(&config.path).unwrap()).unwrap();
    value["users"][0]["user"]["exec"]["apiVersion"] = "client.authentication.k8s.io/v1".into();
    std::fs::write(&config.path, serde_json::to_vec(&value).unwrap()).unwrap();
    let output = config.run(&["-o", "json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!marker.exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactiveMode"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

async fn mount_healthy(
    server: &MockServer,
    proxy_status: u16,
    times: u64,
    selector: bool,
    node_override: Option<ResponseTemplate>,
) {
    let mut input: Value =
        serde_json::from_str(include_str!("../examples/contract/healthy/input.json")).unwrap();
    input["kubernetes"]["pvcs"][0]["metadata"]["labels"] = json!({"app":"postgres"});
    input["kubernetes"]["pods"][0]["metadata"]["labels"] =
        json!({"app":"different-consumer-label"});
    let now = jiff::Timestamp::now().to_string();
    for pod in input["summary_responses"]["worker-a"]["pods"]
        .as_array_mut()
        .unwrap()
    {
        for volume in pod["volume"].as_array_mut().unwrap() {
            volume["time"] = now.clone().into();
        }
    }
    input["summary_responses"]["worker-a"]["pods"].as_array_mut().unwrap().push(json!({"podRef":{"uid":"unrelated","name":"do-not-export","namespace":"unrelated"},"volume":[{"name":"secret-payload-marker","usedBytes":73}]}));
    let mut base = Mock::given(method("GET"))
        .and(path("/api/v1/namespaces/demo/persistentvolumeclaims"))
        .and(query_param("limit", "500"));
    if selector {
        base = base.and(query_param("labelSelector", "app=postgres"));
    }
    base.respond_with(ResponseTemplate::new(200).set_body_json(
        json!({"metadata":{"resourceVersion":"1"},"items":input["kubernetes"]["pvcs"]}),
    ))
    .expect(times)
    .mount(server)
    .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/namespaces/demo/pods"))
        .and(query_param("limit", "500"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"metadata":{"resourceVersion":"2"},"items":input["kubernetes"]["pods"]}),
        ))
        .expect(times)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/persistentvolumes/pv-data"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&input["kubernetes"]["pvs"][0]))
        .expect(times)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/apis/storage.k8s.io/v1/storageclasses/fast"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(&input["kubernetes"]["storage_classes"][0]),
        )
        .expect(times)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/nodes/worker-a/proxy/stats/summary"))
        .respond_with(node_override.unwrap_or_else(|| {
            ResponseTemplate::new(proxy_status)
                .set_body_json(&input["summary_responses"]["worker-a"])
        }))
        .expect(times)
        .mount(server)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_usage_export_preserves_provenance_and_never_logs_unrelated_node_payloads() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    mount_healthy(&server, 200, 1, true, None).await;
    let output = config.run(&[
        "-u",
        "--require-metrics",
        "-l",
        "app=postgres",
        "-o",
        "json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["records"][0]["filesystem"]["status"], "ok");
    assert_eq!(
        report["records"][0]["consumer_ids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(report["collection"]["cost"]["node_requests"], 1);
    assert!(
        report["collection"]["cost"]["response_bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("secret-payload-marker"));
    assert!(output.stderr.is_empty());
    let requests = server.received_requests().await.unwrap();
    let pods = requests
        .iter()
        .find(|r| r.url.path().ends_with("/pods"))
        .unwrap();
    assert!(!pods.url.query().unwrap_or("").contains("labelSelector"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_denied_proxy_retains_inventory_and_required_metrics_exit_four() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    mount_healthy(&server, 403, 2, false, None).await;
    for (args, exit) in [
        (vec!["-u", "-o", "json"], 0),
        (vec!["--require-metrics", "-o", "json"], 4),
    ] {
        let output = config.run(&args);
        assert_eq!(output.status.code(), Some(exit));
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["records"].as_array().unwrap().len(), 1);
        assert_eq!(report["records"][0]["filesystem"]["status"], "forbidden");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("forbidden"));
        assert!(!stderr.contains("secret-payload-marker"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn node_authentication_failure_is_fatal_even_with_an_oversized_error_body() {
    for size in [128, kubectl_view_storage::transport::MAX_RESPONSE_BYTES + 1] {
        let server = MockServer::start().await;
        let config = Kubeconfig::new(&server);
        let marker = b"credential-payload-must-stay-private";
        let mut body = vec![b'x'; size];
        body[..marker.len()].copy_from_slice(marker);
        mount_healthy(
            &server,
            401,
            1,
            false,
            Some(ResponseTemplate::new(401).set_body_bytes(body)),
        )
        .await;
        let output = config.run(&["-u", "-o", "json"]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "size={size}: {stderr}");
        assert!(output.stdout.is_empty());
        assert!(stderr.contains("401") || stderr.contains("authentication"));
        assert!(!stderr.contains(std::str::from_utf8(marker).unwrap()));
    }
}

#[cfg(unix)]
mod output_cancellation {
    use super::*;
    use nix::fcntl::{FcntlArg, OFlag as OFlags, fcntl};
    use std::{
        io::{self, Read},
        process::{Child, Stdio},
        time::{Duration, Instant},
    };
    use wiremock::matchers::query_param_is_missing;

    fn fcntl_getfl(fd: impl std::os::fd::AsFd) -> nix::Result<OFlags> {
        Ok(OFlags::from_bits_retain(fcntl(fd, FcntlArg::F_GETFL)?))
    }
    fn fcntl_setfl(fd: impl std::os::fd::AsFd, flags: OFlags) -> nix::Result<()> {
        fcntl(fd, FcntlArg::F_SETFL(flags)).map(|_| ())
    }

    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
    impl OwnedChild {
        fn wait_bounded(&mut self) -> std::process::ExitStatus {
            let until = Instant::now() + Duration::from_secs(3);
            while Instant::now() < until {
                if let Some(status) = self.0.try_wait().unwrap() {
                    return status;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("command did not join its output worker before the watchdog");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn full_output_pipes_cancel_all_formats_and_restore_inherited_flags() {
        let server = MockServer::start().await;
        let config = Kubeconfig::new(&server);
        let claims: Vec<_> = (0..1000).map(|index| json!({
            "apiVersion":"v1", "kind":"PersistentVolumeClaim",
            "metadata":{"name":format!("claim-{index:05}"),"uid":format!("claim-{index:05}"),"namespace":"demo"},
            "spec":{"storageClassName":"","resources":{"requests":{"storage":"1Gi"}}},
            "status":{"phase":"Pending"}
        })).collect();
        Mock::given(method("GET"))
            .and(path("/api/v1/namespaces/demo/persistentvolumeclaims"))
            .and(query_param("limit", "500"))
            .and(query_param_is_missing("continue"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"metadata":{"resourceVersion":"20","continue":"page-2"},"items":&claims[..500]})))
            .expect(6).mount(&server).await;
        Mock::given(method("GET"))
            .and(path("/api/v1/namespaces/demo/persistentvolumeclaims"))
            .and(query_param("continue", "page-2"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"metadata":{"resourceVersion":"20"},"items":&claims[500..]}),
                ),
            )
            .expect(6)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/namespaces/demo/pods"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"metadata":{"resourceVersion":"21"},"items":[]})),
            )
            .expect(6)
            .mount(&server)
            .await;
        for with_credentials in [false, true] {
            if with_credentials {
                let mut value: Value =
                    serde_json::from_slice(&std::fs::read(&config.path).unwrap()).unwrap();
                value["users"][0]["user"] = json!({"exec":{
                    "apiVersion":"client.authentication.k8s.io/v1", "interactiveMode":"Never",
                    "command":"/bin/sh", "args":["-c", "printf '%s' '{\"apiVersion\":\"client.authentication.k8s.io/v1\",\"kind\":\"ExecCredential\",\"status\":{\"token\":\"fixture-token-not-a-real-credential\"}}'"]
                }});
                std::fs::write(&config.path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            for args in [
                vec!["-o", "json"],
                vec!["-o", "csv"],
                vec!["--view", "table", "--wide"],
            ] {
                let (mut reader, writer) = io::pipe().unwrap();
                let before = fcntl_getfl(&writer).unwrap();
                fcntl_setfl(&reader, fcntl_getfl(&reader).unwrap() | OFlags::O_NONBLOCK).unwrap();
                let mut child = OwnedChild(
                    Command::new(env!("CARGO_BIN_EXE_kubectl-view_storage"))
                        .arg("--kubeconfig")
                        .arg(&config.path)
                        .args(["--timeout", "10s"])
                        .args(&args)
                        .env("NO_COLOR", "1")
                        .stdout(Stdio::from(writer.try_clone().unwrap()))
                        .stderr(Stdio::piped())
                        .spawn()
                        .unwrap(),
                );
                let until = Instant::now() + Duration::from_secs(10);
                let mut first = [0; 1];
                loop {
                    match reader.read(&mut first) {
                        Ok(1) => break,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                        other => panic!("unexpected output readiness result: {other:?}"),
                    }
                    assert!(
                        child.0.try_wait().unwrap().is_none(),
                        "producer exited before writing {args:?}"
                    );
                    assert!(
                        Instant::now() < until,
                        "producer never started writing {args:?}"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                // Consume no more data: the report is larger than the OS pipe.
                std::thread::sleep(Duration::from_millis(50));
                assert!(fcntl_getfl(&writer).unwrap().contains(OFlags::O_NONBLOCK));
                assert!(
                    Command::new("/bin/kill")
                        .args(["-INT", &child.0.id().to_string()])
                        .status()
                        .unwrap()
                        .success()
                );
                assert_eq!(child.wait_bounded().code(), Some(130), "{args:?}");
                assert_eq!(
                    fcntl_getfl(&writer).unwrap().contains(OFlags::O_NONBLOCK),
                    before.contains(OFlags::O_NONBLOCK)
                );
                let mut stderr = String::new();
                child
                    .0
                    .stderr
                    .take()
                    .unwrap()
                    .read_to_string(&mut stderr)
                    .unwrap();
                assert!(!stderr.contains("panicked"));
            }
        }
    }
}
