//! Cancellable output for the supported Linux/macOS command targets.
//!
//! O_NONBLOCK belongs to the open file description, including inherited/duped
//! handles. Restore its prior state on every normal, error, cancellation and
//! unwinding path. The command owns one writer and awaits its blocking worker.
use nix::{
    errno::Errno,
    fcntl::{FcntlArg, OFlag as OFlags, fcntl},
    unistd::write,
};
use std::{
    io::{self, BufWriter, Write},
    os::fd::AsFd,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const RETRY_PAUSE: Duration = Duration::from_millis(25);
const CHUNK: usize = 64 * 1024;

fn fcntl_getfl(fd: impl AsFd) -> io::Result<OFlags> {
    Ok(OFlags::from_bits_retain(fcntl(fd, FcntlArg::F_GETFL)?))
}
fn fcntl_setfl(fd: impl AsFd, flags: OFlags) -> io::Result<()> {
    fcntl(fd, FcntlArg::F_SETFL(flags))?;
    Ok(())
}

fn cancelled(cancel: &CancellationToken) -> io::Result<()> {
    if cancel.is_cancelled() {
        Err(io::Error::other("run interrupted"))
    } else {
        Ok(())
    }
}

struct Nonblocking<F: AsFd> {
    fd: F,
    original: OFlags,
    changed: bool,
    cancel: CancellationToken,
}
impl<F: AsFd> Nonblocking<F> {
    fn new(fd: F, cancel: CancellationToken) -> io::Result<Self> {
        cancelled(&cancel)?;
        let original = fcntl_getfl(&fd)?;
        let changed = !original.contains(OFlags::O_NONBLOCK);
        if changed {
            fcntl_setfl(&fd, original | OFlags::O_NONBLOCK)?;
        }
        Ok(Self {
            fd,
            original,
            changed,
            cancel,
        })
    }
    fn restore(&mut self) -> io::Result<()> {
        if self.changed {
            fcntl_setfl(&self.fd, self.original)?;
            self.changed = false;
        }
        Ok(())
    }
}
impl<F: AsFd> Drop for Nonblocking<F> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
impl<F: AsFd> Write for Nonblocking<F> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            cancelled(&self.cancel)?;
            match write(&self.fd, &bytes[..bytes.len().min(CHUNK)]) {
                Ok(written) => return Ok(written),
                Err(Errno::EINTR) => continue,
                Err(Errno::EAGAIN) => {
                    // A bounded pause works for pipes and terminals on both
                    // platforms, including macOS descriptors unsupported by poll.
                    std::thread::sleep(RETRY_PAUSE);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        cancelled(&self.cancel)
    }
}

pub fn with_writer<F: AsFd>(
    fd: F,
    cancel: CancellationToken,
    operation: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> io::Result<()> {
    let mut raw = Nonblocking::new(fd, cancel)?;
    let mut buffered = BufWriter::with_capacity(CHUNK, &mut raw);
    let result = catch_unwind(AssertUnwindSafe(|| {
        operation(&mut buffered).and_then(|()| buffered.flush())
    }));
    // BufWriter's Drop flushes. Explicitly discard pending bytes on failure or
    // panic, so neither unwinding nor shutdown can produce a late write.
    let _ = buffered.into_parts();
    let restored = raw.restore();
    match result {
        Ok(outcome) => restored.and(outcome),
        Err(panic) => resume_unwind(panic),
    }
}

#[cfg(test)]
#[path = "../tests/unit/stdio.rs"]
mod tests;
