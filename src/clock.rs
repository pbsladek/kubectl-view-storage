//! Independent clocks: wall time never controls request or shutdown budgets.
use jiff::Timestamp;
use tokio::time::Instant;

pub trait Clock: Send + Sync {
    fn monotonic(&self) -> Instant;
    fn utc(&self) -> Timestamp;
}
pub struct SystemClock;
impl Clock for SystemClock {
    fn monotonic(&self) -> Instant {
        Instant::now()
    }
    fn utc(&self) -> Timestamp {
        Timestamp::now()
    }
}

#[cfg(test)]
#[path = "../tests/unit/clock.rs"]
pub mod tests;
