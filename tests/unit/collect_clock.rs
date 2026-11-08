//! UTC jumps during real collection must not change Tokio request allowances.
use super::*;
use crate::{clock::tests::TestClock, pipeline, policy::Exit};
use std::sync::atomic::Ordering::SeqCst;
use tokio::sync::Semaphore;

struct GatedStatistics {
    inner: Arc<MockTransport>,
    starts: Mutex<Vec<Instant>>,
    active: AtomicUsize,
    replies: [Semaphore; 2],
}
impl GatedStatistics {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: fixture_transport(),
            starts: Mutex::new(vec![]),
            active: AtomicUsize::new(0),
            replies: [Semaphore::new(0), Semaphore::new(0)],
        })
    }
    async fn wait_for_attempts(&self, count: usize) {
        // Keep the test task runnable: paused Tokio must not automatically
        // advance to an overall deadline while a blocking inventory job joins.
        for _ in 0..100_000 {
            if self.starts.lock().unwrap().len() == count {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("statistics attempt {count} never started");
    }
    async fn retry_with_wall_jumps(&self, clock: &TestClock) -> Instant {
        self.wait_for_attempts(1).await;
        let started = self.starts.lock().unwrap()[0];
        clock.set_utc("2020-01-01T00:00:00Z".parse().unwrap());
        self.replies[0].add_permits(1);
        for _ in 0..100_000 {
            if self.active.load(SeqCst) == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(self.active.load(SeqCst), 0);
        clock.set_utc("2030-01-01T00:00:00Z".parse().unwrap());
        tokio::time::advance(Duration::from_millis(1999)).await;
        assert_eq!(self.starts.lock().unwrap().len(), 1);
        tokio::time::advance(Duration::from_millis(1)).await;
        self.wait_for_attempts(2).await;
        assert_eq!(
            self.starts.lock().unwrap()[1] - started,
            Duration::from_secs(2)
        );
        started
    }
}
impl Transport for GatedStatistics {
    fn get(&self, path: String) -> ResponseFuture<'_> {
        Box::pin(async move {
            if !path.contains("/stats/summary") {
                return self.inner.get(path).await;
            }
            let attempt = {
                let mut starts = self.starts.lock().unwrap();
                let attempt = starts.len();
                starts.push(Instant::now());
                attempt
            };
            assert!(attempt < 2, "unexpected late or extra statistics retry");
            self.active.fetch_add(1, SeqCst);
            let _active = Active(&self.active);
            self.replies[attempt].acquire().await.unwrap().forget();
            if attempt == 0 {
                Ok(Response {
                    status: 429,
                    body: vec![],
                    retry_after: Some(Duration::from_secs(2)),
                })
            } else {
                self.inner.get(path).await
            }
        })
    }
}

fn spawn_collection(
    transport: Arc<GatedStatistics>,
    clock: Arc<TestClock>,
    cancel: CancellationToken,
    timeout: &str,
    request_timeout: &str,
) -> tokio::task::JoinHandle<Result<(Collected, Query), Failure>> {
    let cli = Cli::try_parse_from([
        "cmd",
        "--require-metrics",
        "--timeout",
        timeout,
        "--request-timeout",
        request_timeout,
    ])
    .unwrap();
    let query = cli.query("demo").unwrap();
    tokio::spawn(async move {
        let collected = collect(
            transport,
            clock,
            cancel,
            &cli,
            &query,
            "clock-fixture".into(),
        )
        .await?;
        Ok((collected, query))
    })
}

#[tokio::test(start_paused = true)]
async fn wall_jumps_during_retry_preserve_backoff_and_final_freshness() {
    for (finished, status, reason, exit) in [
        ("2026-11-06T20:00:10Z", "ok", None, Exit::Success),
        (
            "2026-11-06T20:10:00Z",
            "stale",
            Some("max_age_exceeded"),
            Exit::Inconclusive,
        ),
        (
            "2026-11-06T19:59:00Z",
            "invalid",
            Some("clock_skew"),
            Exit::Inconclusive,
        ),
    ] {
        let transport = GatedStatistics::new();
        let clock = Arc::new(TestClock::new("2026-11-06T20:00:00Z".parse().unwrap()));
        let task = spawn_collection(
            transport.clone(),
            clock.clone(),
            CancellationToken::new(),
            "6s",
            "2s",
        );
        let started = transport.retry_with_wall_jumps(&clock).await;
        clock.set_utc(finished.parse().unwrap());
        transport.replies[1].add_permits(1);
        let (collected, query) = task.await.unwrap().unwrap();
        assert_eq!(Instant::now() - started, Duration::from_secs(2));
        assert_eq!(
            collected.collection.started_at.to_string(),
            "2026-11-06T20:00:00Z"
        );
        assert_eq!(collected.collection.finished_at.to_string(), finished);
        assert_eq!(collected.collection.cost.node_requests, 2);
        let report = pipeline::finish(collected, &query).unwrap();
        assert_eq!(report.exit, exit);
        for family in ["filesystem", "inodes"] {
            let metric = &report.document["records"][0][family];
            assert_eq!(metric["status"], status);
            assert_eq!(metric["candidate_count"], 1);
            assert_eq!(metric["observed_at"], "2026-11-06T19:59:50Z");
            if let Some(reason) = reason {
                assert_eq!(metric["candidates"][0]["reason"], reason);
            }
        }
        assert_eq!(transport.active.load(SeqCst), 0);
        tokio::time::advance(Duration::from_secs(20)).await;
        assert_eq!(transport.starts.lock().unwrap().len(), 2);
    }
}

#[tokio::test(start_paused = true)]
async fn wall_jumps_during_request_and_overall_deadlines_retain_inventory() {
    for (overall, request, elapsed) in [("6s", "2s", 4), ("3s", "10s", 3)] {
        let transport = GatedStatistics::new();
        let clock = Arc::new(TestClock::new("2026-11-06T20:00:00Z".parse().unwrap()));
        let cancel = CancellationToken::new();
        let task = spawn_collection(
            transport.clone(),
            clock.clone(),
            cancel.clone(),
            overall,
            request,
        );
        let started = transport.retry_with_wall_jumps(&clock).await;
        clock.set_utc("2020-01-01T00:00:00Z".parse().unwrap());
        let remaining = Duration::from_secs(elapsed - 2);
        tokio::time::advance(remaining - Duration::from_millis(1)).await;
        assert!(!task.is_finished());
        assert_eq!(transport.active.load(SeqCst), 1);
        clock.set_utc("2030-01-01T00:00:00Z".parse().unwrap());
        tokio::time::advance(Duration::from_millis(1)).await;
        let (collected, query) = task.await.unwrap().unwrap();
        assert_eq!(Instant::now() - started, Duration::from_secs(elapsed));
        assert!(
            !cancel.is_cancelled(),
            "internal expiry cancelled the user's token"
        );
        assert_eq!(collected.collection.cost.node_requests, 2);
        assert_eq!(collected.graph.records.len(), 1);
        assert_eq!(collected.graph.records[0].filesystem.status(), "timeout");
        assert_eq!(
            collected.collection.finished_at.to_string(),
            "2030-01-01T00:00:00Z"
        );
        let report = pipeline::finish(collected, &query).unwrap();
        assert_eq!(report.exit, Exit::Inconclusive);
        assert_eq!(
            report.document["records"][0]["requested_bytes"],
            "107374182400"
        );
        assert_eq!(transport.active.load(SeqCst), 0);
        tokio::time::advance(Duration::from_secs(20)).await;
        assert_eq!(transport.starts.lock().unwrap().len(), 2);
    }
}

#[tokio::test(start_paused = true)]
async fn wall_jumps_during_shutdown_preserve_the_one_second_abort_bound() {
    let clock = Arc::new(TestClock::new("2026-11-06T20:00:00Z".parse().unwrap()));
    let active = Arc::new(AtomicUsize::new(0));
    let worker_active = active.clone();
    let mut workers = JoinSet::new();
    workers.spawn(async move {
        worker_active.fetch_add(1, SeqCst);
        let _active = Active(&worker_active);
        tokio::time::sleep(Duration::from_secs(100)).await;
    });
    while active.load(SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    let started = clock.monotonic();
    let shutdown_clock = clock.clone();
    let task = tokio::spawn(async move {
        shutdown(&mut workers, shutdown_clock.as_ref()).await;
        assert!(workers.is_empty());
    });
    tokio::task::yield_now().await;
    clock.set_utc("2020-01-01T00:00:00Z".parse().unwrap());
    tokio::time::advance(Duration::from_millis(999)).await;
    assert!(!task.is_finished());
    assert_eq!(active.load(SeqCst), 1);
    clock.set_utc("2030-01-01T00:00:00Z".parse().unwrap());
    tokio::time::advance(Duration::from_millis(1)).await;
    task.await.unwrap();
    assert_eq!(clock.monotonic() - started, Duration::from_secs(1));
    assert_eq!(active.load(SeqCst), 0);
}
