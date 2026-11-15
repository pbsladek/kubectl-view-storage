//! Sequential executable refresh, automatic growth history and strict stops.
use super::http_reports::{bundle, mount_bundle};
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watch_emits_complete_json_lines_and_automatically_pairs_new_samples() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    let input = bundle("healthy");
    let original = input["summary_responses"]["worker-a"].clone();
    mount_bundle(&server, input).await;
    let sequence = AtomicUsize::new(0);
    Mock::given(method("GET"))
        .and(path("/api/v1/nodes/worker-a/proxy/stats/summary"))
        .respond_with(move |_: &wiremock::Request| {
            let n = sequence.fetch_add(1, Ordering::SeqCst) as u64;
            let mut summary = original.clone();
            let volume = &mut summary["pods"][0]["volume"][0];
            volume["time"] = jiff::Timestamp::now().to_string().into();
            volume["usedBytes"] = json!(40 * 1024u64.pow(3) + n * 1024);
            volume["availableBytes"] = json!(55 * 1024u64.pow(3) - n * 1024);
            ResponseTemplate::new(200).set_body_json(summary)
        })
        .with_priority(1)
        .mount(&server)
        .await;
    let output = config.run(&[
        "--watch",
        "10ms",
        "--watch-count",
        "3",
        "--view",
        "stats",
        "-o",
        "json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reports: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(reports.len(), 3);
    for (index, report) in reports.iter().enumerate() {
        save_integration(&format!("watch-{}", index + 1), report);
    }
    assert!(reports[0]["calculations"]["comparison"].is_null());
    for index in 1..3 {
        assert_eq!(reports[index]["watch"]["cycle"], index + 1);
        assert_eq!(
            reports[index]["calculations"]["comparison"]["baseline_finished_at"],
            reports[index - 1]["collection"]["finished_at"]
        );
        assert_eq!(
            reports[index]["calculations"]["comparison"]["records"][0]["status"],
            "growing"
        );
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.url.path().ends_with("stats/summary"))
            .count(),
        3
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watch_redirected_text_appends_and_strict_failure_stops_after_its_report() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    mount_bundle(&server, bundle("healthy")).await;
    let output = config.run(&["--watch", "1ms", "--watch-count", "2"]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Collection cycle 1:") && text.contains("Collection cycle 2:"));
    assert!(!text.contains('\x1b'));
    server.reset().await;
    mount_bundle(&server, bundle("missing-stale")).await;
    let output = config.run(&[
        "--watch",
        "1ms",
        "--watch-count",
        "3",
        "--require-metrics",
        "-o",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(String::from_utf8(output.stdout).unwrap().lines().count(), 1);
    server.reset().await;
    mount_bundle(&server, bundle("field-pressure")).await;
    let output = config.run(&[
        "--watch",
        "1ms",
        "--watch-count",
        "3",
        "-u",
        "--fail-on",
        "critical",
        "-o",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(String::from_utf8(output.stdout).unwrap().lines().count(), 1);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watch_interrupt_during_the_interval_joins_and_keeps_the_completed_json_line() {
    use std::io::BufRead;
    use std::process::{Child, Stdio};
    use std::time::Duration;
    struct Owned(Child);
    impl Drop for Owned {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    let mut configuration: Value =
        serde_json::from_slice(&std::fs::read(&config.path).unwrap()).unwrap();
    configuration["users"][0]["user"] = json!({"exec":{"apiVersion":"client.authentication.k8s.io/v1","interactiveMode":"Never","command":"/bin/sh","args":["-c","printf '%s' '{\"apiVersion\":\"client.authentication.k8s.io/v1\",\"kind\":\"ExecCredential\",\"status\":{\"token\":\"watch-fixture-token\"}}'"]}});
    std::fs::write(&config.path, serde_json::to_vec(&configuration).unwrap()).unwrap();
    mount_bundle(&server, bundle("healthy")).await;
    let mut child = Owned(
        Command::new(env!("CARGO_BIN_EXE_kubectl-view_storage"))
            .arg("--kubeconfig")
            .arg(&config.path)
            .args(["--watch", "1h", "-o", "json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let mut first = tokio::task::spawn_blocking(move || {
        let mut reader = std::io::BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        (reader, line)
    });
    let first = match tokio::time::timeout(Duration::from_secs(5), &mut first).await {
        Ok(result) => result.unwrap(),
        Err(_) => {
            child.0.kill().unwrap();
            let _ = first.await;
            panic!("watch never emitted its first complete cycle");
        }
    };
    let (_reader, line) = first;
    let report: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(report["watch"]["cycle"], 1);
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(child.0.id() as i32),
        nix::sys::signal::Signal::SIGINT,
    )
    .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("watch did not stop during its interval");
    assert_eq!(status.code(), Some(130));
    assert_eq!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path().ends_with("persistentvolumeclaims"))
            .count(),
        1
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watch_replaces_terminal_text_only_after_a_complete_cycle() {
    use std::io::Read;
    use std::process::{Child, Stdio};
    use std::time::Duration;
    struct Owned(Child);
    impl Drop for Owned {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    mount_bundle(&server, bundle("healthy")).await;
    let terminal = nix::pty::openpty(None, None).unwrap();
    let mut child = Owned(
        Command::new(env!("CARGO_BIN_EXE_kubectl-view_storage"))
            .arg("--kubeconfig")
            .arg(&config.path)
            .args(["--watch", "1ms", "--watch-count", "2"])
            .stdin(Stdio::null())
            .stdout(Stdio::from(terminal.slave))
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let reader = tokio::task::spawn_blocking(move || {
        let mut master = std::fs::File::from(terminal.master);
        let mut bytes = vec![];
        let mut buffer = [0; 4096];
        loop {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => bytes.extend_from_slice(&buffer[..n]),
                Err(error) if error.raw_os_error() == Some(nix::libc::EIO) => break,
                Err(error) => panic!("PTY read: {error}"),
            }
        }
        String::from_utf8(bytes).unwrap()
    });
    let status = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("terminal watch did not finish");
    assert!(status.success());
    let text = reader.await.unwrap();
    assert_eq!(text.matches("\x1b[2J\x1b[H").count(), 2);
    assert!(text.contains("Collection cycle 1:") && text.contains("Collection cycle 2:"));
}
