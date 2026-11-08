#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::clock::tests::TestClock;
use std::{
    sync::atomic::{AtomicUsize, Ordering as AtomicOrdering},
    time::Duration,
};

struct Budget {
    remaining: usize,
}
impl Checkpoint for Budget {
    type Error = ();
    fn step(&mut self) -> Result<(), ()> {
        self.remaining = self.remaining.checked_sub(1).ok_or(())?;
        Ok(())
    }
}

#[test]
fn checked_sort_is_stable_and_interruptible_during_merges() {
    let input: Vec<_> = (0..1000).map(|index| (index % 7, index)).rev().collect();
    let mut expected = input.clone();
    expected.sort_by_key(|value| value.0);
    let mut sorted = input.clone();
    sort_by(
        &mut sorted,
        |a, b| a.0.cmp(&b.0),
        &mut Budget {
            remaining: usize::MAX,
        },
    )
    .unwrap();
    assert_eq!(sorted, expected);
    for remaining in [0, 63, 64, 1001, 2000] {
        let mut values = input.clone();
        assert!(sort_by(&mut values, |a, b| a.0.cmp(&b.0), &mut Budget { remaining }).is_err());
    }
    for input in [vec![], vec![1], vec![3, 2, 1]] {
        let mut actual = input.clone();
        sort_by(&mut actual, Ord::cmp, &mut Budget { remaining: 1000 }).unwrap();
        let mut expected = input;
        expected.sort();
        assert_eq!(actual, expected);
    }
}

#[tokio::test(start_paused = true)]
async fn deadline_drain_preserves_inventory_but_never_ignores_user_cancellation() {
    let clock = Arc::new(TestClock::new("2026-11-07T00:00:00Z".parse().unwrap()));
    let cancel = CancellationToken::new();
    let deadline = clock.monotonic();
    let mut control = RunControl::retained_inventory(cancel.clone(), clock.clone(), deadline);
    for _ in 0..CHECK_INTERVAL {
        control.step().unwrap();
    }
    assert!(control.deadline_observed);
    let mut bounded = RunControl::bounded(cancel.clone(), clock, deadline);
    assert_eq!(bounded.check_now(), Err(Stop::Deadline));
    assert!(!cancel.is_cancelled());
    cancel.cancel();
    assert_eq!(control.check_now(), Err(Stop::Cancelled));
    assert_eq!(bounded.check_now(), Err(Stop::Cancelled));
    assert_eq!(CHECK_INTERVAL, 64);
}

#[tokio::test(flavor = "current_thread")]
async fn cpu_worker_yields_to_async_cancellation_and_is_joined_before_return() {
    let cancel = CancellationToken::new();
    let clock = Arc::new(crate::clock::SystemClock);
    let control = RunControl::retained_inventory(
        cancel.clone(),
        clock,
        Instant::now() + Duration::from_secs(30),
    );
    let progress = Arc::new(AtomicUsize::new(0));
    let observed = progress.clone();
    let ready = Arc::new(tokio::sync::Notify::new());
    let worker_ready = ready.clone();
    let operation = tokio::spawn(run(control, move |control| {
        worker_ready.notify_one();
        // A broken cancellation check must fail this test without leaving an
        // infinite blocking task that hangs the runtime's test teardown.
        let watchdog = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < watchdog {
            control.checked()?;
            observed.fetch_add(1, AtomicOrdering::Relaxed);
        }
        Ok::<(), CheckedError<Stop>>(())
    }));
    ready.notified().await;
    // The current-thread async executor remains responsive while the CPU loops.
    cancel.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), operation)
            .await
            .unwrap()
            .unwrap(),
        Err(RunError::Stopped(Stop::Cancelled))
    ));
    let joined = progress.load(AtomicOrdering::Relaxed);
    tokio::task::yield_now().await;
    assert_eq!(progress.load(AtomicOrdering::Relaxed), joined);
}

#[tokio::test]
async fn successful_worker_does_not_cancel_its_parent_and_errors_are_observed() {
    let cancel = CancellationToken::new();
    let make_control = || {
        RunControl::retained_inventory(
            cancel.clone(),
            Arc::new(crate::clock::SystemClock),
            Instant::now() + Duration::from_secs(30),
        )
    };
    assert_eq!(run(make_control(), |_| Ok(7)).await.unwrap(), 7);
    assert!(!cancel.is_cancelled());
    assert!(
        matches!(run::<()>(make_control(), |_| Err("bad identity".into())).await, Err(RunError::Invalid(message)) if message == "bad identity")
    );
    assert!(matches!(
        run::<()>(make_control(), |_| panic!("controlled test worker panic")).await,
        Err(RunError::Worker)
    ));
}
