#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use std::{sync::Mutex, time::Duration};
pub struct TestClock {
    wall: Mutex<Timestamp>,
}
impl TestClock {
    pub fn new(wall: Timestamp) -> Self {
        Self {
            wall: Mutex::new(wall),
        }
    }
    pub fn set_utc(&self, wall: Timestamp) {
        *self.wall.lock().unwrap() = wall;
    }
}
impl Clock for TestClock {
    fn monotonic(&self) -> Instant {
        Instant::now()
    }
    fn utc(&self) -> Timestamp {
        *self.wall.lock().unwrap()
    }
}
#[tokio::test(start_paused = true)]
async fn wall_jumps_do_not_extend_monotonic_deadlines() {
    let clock = TestClock::new("2026-11-07T00:00:00Z".parse().unwrap());
    let before = clock.monotonic();
    let wall = clock.utc();
    clock.set_utc("2020-01-01T00:00:00Z".parse().unwrap());
    assert_eq!(before, clock.monotonic());
    tokio::time::advance(Duration::from_secs(7)).await;
    assert_eq!(clock.monotonic() - before, Duration::from_secs(7));
    assert!(clock.utc() < wall);
    clock.set_utc("2030-01-01T00:00:00Z".parse().unwrap());
    assert_eq!(clock.monotonic() - before, Duration::from_secs(7));
}
