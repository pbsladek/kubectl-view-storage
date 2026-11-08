//! Actual helper processes must terminate/reap even if SDK construction blocks.
use super::*;
use nix::{
    errno::Errno,
    sys::signal::{Signal, kill, killpg},
    unistd::Pid,
};
use std::{
    process::{Child, Stdio},
    time::{Duration, Instant},
};

struct Running {
    child: Child,
    marker: PathBuf,
}
impl Running {
    fn identities(&self) -> Option<Vec<i32>> {
        let text = std::fs::read_to_string(&self.marker).ok()?;
        let pids: Vec<_> = text
            .split_whitespace()
            .map(str::parse::<i32>)
            .collect::<Result<_, _>>()
            .ok()?;
        (pids.len() == 3 && pids[0] == self.child.id() as i32 && pids.iter().all(|pid| *pid > 0))
            .then_some(pids)
    }
    fn wait_output(&mut self, maximum: Duration) -> Output {
        let until = Instant::now() + maximum;
        while Instant::now() < until {
            if let Some(status) = self.child.try_wait().unwrap() {
                use std::io::Read;
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                self.child
                    .stdout
                    .take()
                    .unwrap()
                    .read_to_end(&mut stdout)
                    .unwrap();
                self.child
                    .stderr
                    .take()
                    .unwrap()
                    .read_to_end(&mut stderr)
                    .unwrap();
                return Output {
                    status,
                    stdout,
                    stderr,
                };
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("credential-helper command exceeded its executable watchdog");
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            // Only this fixture's recorded worker group, while its owning root
            // child remains waitable, is eligible for emergency test cleanup.
            if let Some(pids) = self.identities() {
                let _ = killpg(Pid::from_raw(pids[1]), Signal::SIGKILL);
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn start(config: &Kubeconfig, timeout: &str, ignore_interrupt: bool) -> Running {
    let marker = config.directory.join("helper-processes");
    let script = format!(
        "{}printf '%s %s %s' \"$VIEW_STORAGE_INTERNAL_EXEC_PARENT\" \"$PPID\" \"$$\" > \"$1\"; exec /bin/sleep 5",
        if ignore_interrupt {
            "trap '' INT TERM; "
        } else {
            ""
        }
    );
    let mut value: Value = serde_json::from_slice(&std::fs::read(&config.path).unwrap()).unwrap();
    value["users"][0]["user"] = json!({"exec":{
        "apiVersion":"client.authentication.k8s.io/v1beta1", "interactiveMode":"Never",
        "command":"/bin/sh", "args":["-c",script,"fixture",marker]
    }});
    std::fs::write(&config.path, serde_json::to_vec(&value).unwrap()).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_kubectl-view_storage"))
        .arg("--kubeconfig")
        .arg(&config.path)
        .args(["--timeout", timeout, "-o", "json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    Running { child, marker }
}

fn assert_joined(run: &Running) {
    let pids = run
        .identities()
        .expect("fixture helper never entered the owned SDK worker");
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until {
        if pids
            .iter()
            .all(|pid| matches!(kill(Pid::from_raw(*pid), None), Err(Errno::ESRCH)))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    for pid in pids {
        assert!(
            matches!(kill(Pid::from_raw(pid), None), Err(Errno::ESRCH)),
            "owned helper/root process remains: {pid}"
        );
    }
    let prefix = format!("view-storage-auth-{}-", run.child.id());
    assert!(
        std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(&prefix)),
        "owned private startup directory remains"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sdk_helper_deadlines_terminate_and_reap_cooperative_and_stubborn_processes() {
    for ignore_interrupt in [false, true] {
        let server = MockServer::start().await;
        let config = Kubeconfig::new(&server);
        let started = Instant::now();
        let mut run = start(&config, "500ms", ignore_interrupt);
        let output = run.wait_output(Duration::from_secs(3));
        assert!(started.elapsed() < Duration::from_millis(2500));
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("deadline"));
        assert_joined(&run);
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_blocked_sdk_helper_joins_the_entire_owned_process_group() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    let mut run = start(&config, "8s", true);
    let ready = Instant::now() + Duration::from_secs(2);
    while run.identities().is_none() {
        assert!(
            Instant::now() < ready,
            "helper did not start before interrupt"
        );
        assert!(run.child.try_wait().unwrap().is_none());
        std::thread::sleep(Duration::from_millis(5));
    }
    kill(Pid::from_raw(run.child.id() as i32), Signal::SIGINT).unwrap();
    let output = run.wait_output(Duration::from_millis(2500));
    assert_eq!(output.status.code(), Some(130));
    assert!(output.stdout.is_empty());
    assert_joined(&run);
    assert!(server.received_requests().await.unwrap().is_empty());
}

fn token_helper(config: &Kubeconfig, refresh_blocks: bool) -> PathBuf {
    let marker = config.directory.join("helper-processes");
    let expires =
        jiff::Timestamp::from_nanosecond(jiff::Timestamp::now().as_nanosecond() + 5_000_000_000)
            .unwrap()
            .to_string();
    let mut script =
        "printf '%s %s %s' \"$VIEW_STORAGE_INTERNAL_EXEC_PARENT\" \"$PPID\" \"$$\" > \"$1\"; "
            .to_owned();
    if refresh_blocks {
        // Initial SDK construction succeeds. A short-lived credential makes
        // the request auth layer refresh only after the configured handoff.
        script.push_str("if [ -f \"${VIEW_STORAGE_INTERNAL_EXEC_READY%/*}/configured\" ]; then trap '' INT TERM; exec /bin/sleep 5; fi; ");
        script.push_str("printf '{\"apiVersion\":\"client.authentication.k8s.io/v1beta1\",\"kind\":\"ExecCredential\",\"status\":{\"token\":\"fixture-token-not-a-real-credential\",\"expirationTimestamp\":\"%s\"}}' \"$2\"");
    } else {
        script.push_str("printf '%s' '{\"apiVersion\":\"client.authentication.k8s.io/v1beta1\",\"kind\":\"ExecCredential\",\"status\":{\"token\":\"fixture-token-not-a-real-credential\"}}'");
    }
    let mut value: Value = serde_json::from_slice(&std::fs::read(&config.path).unwrap()).unwrap();
    value["users"][0]["user"] = json!({"exec":{
        "apiVersion":"client.authentication.k8s.io/v1beta1", "interactiveMode":"Never",
        "command":"/bin/sh", "args":["-c",script,"fixture",marker,expires]
    }});
    std::fs::write(&config.path, serde_json::to_vec(&value).unwrap()).unwrap();
    marker
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_sdk_refresh_does_not_hold_runtime_shutdown_open() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    let marker = token_helper(&config, true);
    let started = Instant::now();
    let child = Command::new(env!("CARGO_BIN_EXE_kubectl-view_storage"))
        .arg("--kubeconfig")
        .arg(&config.path)
        .args([
            "--timeout",
            "3s",
            "--request-timeout",
            "200ms",
            "-o",
            "json",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut run = Running { child, marker };
    let output = run.wait_output(Duration::from_secs(2));
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
    assert_joined(&run);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supervised_credentials_preserve_partial_inventory_after_optional_collection_deadline() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    let marker = token_helper(&config, false);
    super::http_reports::mount_bundle(&server, super::http_reports::bundle("healthy")).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/nodes/worker-a/proxy/stats/summary"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
        .with_priority(1)
        .expect(2)
        .mount(&server)
        .await;
    for (strict, exit) in [(false, 0), (true, 4)] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kubectl-view_storage"));
        command.arg("--kubeconfig").arg(&config.path).args([
            "--timeout",
            "750ms",
            "--request-timeout",
            "2s",
            "-u",
            "-o",
            "json",
        ]);
        if strict {
            command.arg("--require-metrics");
        }
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut run = Running {
            child,
            marker: marker.clone(),
        };
        let output = run.wait_output(Duration::from_secs(2));
        assert_eq!(
            output.status.code(),
            Some(exit),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["selection"]["matched_count"], 1);
        assert_eq!(report["records"][0]["requested_bytes"], "107374182400");
        assert_ne!(report["records"][0]["filesystem"]["status"], "ok");
        assert_eq!(report["collection"]["cost"]["node_requests"], 1);
        assert_joined(&run);
    }
}
