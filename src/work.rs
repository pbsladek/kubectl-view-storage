//! Cooperative CPU work. Pure callers use an infallible control; a command
//! owns and joins every blocking worker instead of trying to abort its CPU.
use crate::clock::Clock;
use std::{cmp::Ordering, convert::Infallible, fmt, sync::Arc};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub(crate) const CHECK_INTERVAL: usize = 64;
pub(crate) const BYTE_CHUNK: usize = 8 * 1024;

pub(crate) fn clone_string<C: Checkpoint>(
    value: &str,
    control: &mut C,
) -> Result<String, C::Error> {
    if !C::ACTIVE {
        return Ok(value.to_owned());
    }
    let mut copy = String::with_capacity(value.len());
    let mut start = 0;
    while start < value.len() {
        control.boundary()?;
        let mut end = (start + BYTE_CHUNK).min(value.len());
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        copy.push_str(&value[start..end]);
        start = end;
    }
    Ok(copy)
}

pub(crate) fn clone_json<C: Checkpoint>(
    value: &serde_json::Value,
    control: &mut C,
) -> Result<serde_json::Value, C::Error> {
    use serde_json::Value;
    control.step()?;
    match value {
        Value::String(value) => clone_string(value, control).map(Value::String),
        Value::Array(values) => {
            let mut copy = vec![];
            for value in values {
                copy.push(clone_json(value, control)?);
            }
            Ok(Value::Array(copy))
        }
        Value::Object(values) => {
            let mut copy = serde_json::Map::new();
            for (key, value) in values {
                control.step()?;
                copy.insert(clone_string(key, control)?, clone_json(value, control)?);
            }
            Ok(Value::Object(copy))
        }
        _ => Ok(value.clone()),
    }
}

#[derive(Debug)]
pub(crate) enum CheckedError<E> {
    Invalid(String),
    Interrupted(E),
}
impl<E> From<String> for CheckedError<E> {
    fn from(value: String) -> Self {
        Self::Invalid(value)
    }
}
impl<E> From<&str> for CheckedError<E> {
    fn from(value: &str) -> Self {
        Self::Invalid(value.into())
    }
}
impl CheckedError<Infallible> {
    pub(crate) fn into_message(self) -> String {
        match self {
            Self::Invalid(message) => message,
            Self::Interrupted(never) => match never {},
        }
    }
}

pub(crate) trait Checkpoint {
    type Error;
    const ACTIVE: bool = true;
    fn step(&mut self) -> Result<(), Self::Error>;
    /// A bounded I/O/decode boundary checks immediately instead of consuming
    /// an object-loop allowance first.
    fn boundary(&mut self) -> Result<(), Self::Error> {
        self.step()
    }
    fn checked(&mut self) -> Result<(), CheckedError<Self::Error>> {
        self.step().map_err(CheckedError::Interrupted)
    }
}
pub(crate) struct Unbounded;
impl Checkpoint for Unbounded {
    type Error = Infallible;
    const ACTIVE: bool = false;
    fn step(&mut self) -> Result<(), Infallible> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    Cancelled,
    Deadline,
}
impl fmt::Display for Stop {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Cancelled => "interrupted",
            Self::Deadline => "deadline exceeded",
        })
    }
}

pub(crate) struct RunControl {
    cancel: CancellationToken,
    clock: Arc<dyn Clock>,
    deadline: Instant,
    stop_at_deadline: bool,
    deadline_observed: bool,
    remaining: usize,
}
impl RunControl {
    /// Network response processing must finish within its request allowance.
    pub(crate) fn bounded(
        cancel: CancellationToken,
        clock: Arc<dyn Clock>,
        deadline: Instant,
    ) -> Self {
        let mut control = Self::retained_inventory(cancel, clock, deadline);
        control.stop_at_deadline = true;
        control
    }
    /// Completed base inventory still needs a finite projection after a
    /// collection deadline. Observe that deadline, retain all base records,
    /// and keep user cancellation effective throughout this draining work.
    pub(crate) fn retained_inventory(
        cancel: CancellationToken,
        clock: Arc<dyn Clock>,
        deadline: Instant,
    ) -> Self {
        Self {
            cancel: cancel.child_token(),
            clock,
            deadline,
            stop_at_deadline: false,
            deadline_observed: false,
            remaining: CHECK_INTERVAL,
        }
    }
    pub(crate) fn check_now(&mut self) -> Result<(), Stop> {
        if self.cancel.is_cancelled() {
            return Err(Stop::Cancelled);
        }
        self.deadline_observed |= self.clock.monotonic() >= self.deadline;
        if self.deadline_observed && self.stop_at_deadline {
            return Err(Stop::Deadline);
        }
        Ok(())
    }
}
impl Checkpoint for RunControl {
    type Error = Stop;
    fn step(&mut self) -> Result<(), Stop> {
        self.remaining -= 1;
        if self.remaining == 0 {
            self.remaining = CHECK_INTERVAL;
            self.boundary()?;
        }
        Ok(())
    }
    fn boundary(&mut self) -> Result<(), Stop> {
        self.check_now()?;
        // This runs on the blocking pool, never on a Tokio executor worker.
        std::thread::yield_now();
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) enum RunError {
    Invalid(String),
    Stopped(Stop),
    Worker,
}
impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(formatter, "invalid normalized data: {message}"),
            Self::Stopped(reason) => reason.fmt(formatter),
            Self::Worker => formatter.write_str("CPU worker failed"),
        }
    }
}

pub(crate) async fn run<T: Send + 'static>(
    mut control: RunControl,
    operation: impl FnOnce(&mut RunControl) -> Result<T, CheckedError<Stop>> + Send + 'static,
) -> Result<T, RunError> {
    // Dropping a caller requests cooperation too. The command supervisor must
    // await this future; a blocking JoinHandle cannot be aborted as a substitute
    // for joining it. There is no detached writer or background request here.
    let _cancel_on_drop = control.cancel.clone().drop_guard();
    let result = tokio::task::spawn_blocking(move || {
        control.check_now().map_err(CheckedError::Interrupted)?;
        let value = operation(&mut control)?;
        control.check_now().map_err(CheckedError::Interrupted)?;
        Ok(value)
    })
    .await
    .map_err(|_| RunError::Worker)?;
    result.map_err(|error| match error {
        CheckedError::Invalid(message) => RunError::Invalid(message),
        CheckedError::Interrupted(reason) => RunError::Stopped(reason),
    })
}

/// Stable merge sort with a bounded initial run size and checked merge steps.
/// A cancelled owned result is discarded, so no partially sorted graph escapes.
pub(crate) fn sort_by<T, C: Checkpoint>(
    values: &mut Vec<T>,
    mut compare: impl FnMut(&T, &T) -> Ordering,
    control: &mut C,
) -> Result<(), C::Error> {
    if !C::ACTIVE {
        values.sort_by(compare);
        return Ok(());
    }
    if values.len() <= CHECK_INTERVAL {
        for _ in values.iter() {
            control.step()?;
        }
        values.sort_by(compare);
        return Ok(());
    }
    let mut input = std::mem::take(values).into_iter();
    let mut runs = vec![];
    loop {
        let mut run = Vec::with_capacity(CHECK_INTERVAL);
        for value in input.by_ref().take(CHECK_INTERVAL) {
            control.step()?;
            run.push(value);
        }
        if run.is_empty() {
            break;
        }
        run.sort_by(&mut compare);
        runs.push(run);
    }
    while runs.len() > 1 {
        let mut previous = std::mem::take(&mut runs).into_iter();
        while let Some(left) = previous.next() {
            let Some(right) = previous.next() else {
                runs.push(left);
                break;
            };
            let mut merged = Vec::with_capacity(left.len() + right.len());
            let mut left = left.into_iter().peekable();
            let mut right = right.into_iter().peekable();
            while let (Some(a), Some(b)) = (left.peek(), right.peek()) {
                control.step()?;
                let next = if compare(a, b) != Ordering::Greater {
                    left.next()
                } else {
                    right.next()
                };
                if let Some(value) = next {
                    merged.push(value);
                }
            }
            for value in left.chain(right) {
                control.step()?;
                merged.push(value);
            }
            runs.push(merged);
        }
    }
    *values = runs.pop().unwrap_or_default();
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/work.rs"]
mod tests;
