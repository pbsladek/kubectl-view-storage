//! Keep SDK-owned credential subprocesses inside an owned process lifetime.
use kubectl_view_storage::policy::Exit;
#[cfg(target_os = "macos")]
use nix::sys::event::{EvFlags, EventFilter, FilterFlag, KEvent, Kqueue};
#[cfg(target_os = "linux")]
use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid, waitpid};
use nix::{
    errno::Errno,
    fcntl::{FcntlArg, OFlag, fcntl},
    sys::signal::{SigSet, SigmaskHow, Signal, kill, killpg, pthread_sigmask},
    time::{ClockId, clock_gettime},
    unistd::{Pid, getpgrp, getppid, tcgetpgrp, tcsetpgrp},
};
use std::{
    fs::{DirBuilder, OpenOptions},
    io::{self, IsTerminal},
    os::{
        fd::{AsFd, OwnedFd},
        unix::{
            fs::{DirBuilderExt, OpenOptionsExt},
            process::CommandExt,
        },
    },
    path::PathBuf,
    process::{Child, Command, ExitStatus},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

const PARENT: &str = "VIEW_STORAGE_INTERNAL_EXEC_PARENT";
const READY: &str = "VIEW_STORAGE_INTERNAL_EXEC_READY";
const DEADLINE: &str = "VIEW_STORAGE_INTERNAL_EXEC_DEADLINE";
const POLL: Duration = Duration::from_millis(5);

fn monotonic_nanos() -> io::Result<u128> {
    let value = clock_gettime(ClockId::CLOCK_MONOTONIC)?;
    let seconds =
        u128::try_from(value.tv_sec()).map_err(|_| io::Error::other("Invalid monotonic clock"))?;
    let nanos =
        u128::try_from(value.tv_nsec()).map_err(|_| io::Error::other("Invalid monotonic clock"))?;
    Ok(seconds * 1_000_000_000 + nanos)
}

pub fn remaining_budget() -> io::Result<Duration> {
    let deadline = std::env::var(DEADLINE)
        .ok()
        .and_then(|value| value.parse::<u128>().ok())
        .ok_or_else(|| io::Error::other("Invalid worker deadline"))?;
    let remaining = deadline.saturating_sub(monotonic_nanos()?);
    let seconds = u64::try_from(remaining / 1_000_000_000)
        .map_err(|_| io::Error::other("Invalid worker deadline"))?;
    Ok(Duration::new(seconds, (remaining % 1_000_000_000) as u32))
}

pub fn worker_parent() -> Option<Pid> {
    let pid = std::env::var(PARENT).ok()?.parse::<i32>().ok()?;
    if pid <= 0 || getppid().as_raw() != pid || std::env::var_os(READY).is_none() {
        return None;
    }
    Some(Pid::from_raw(pid))
}

pub fn interactive_stdin() -> bool {
    io::stdin().is_terminal() && tcgetpgrp(io::stdin()).is_ok_and(|owner| owner == getpgrp())
}

pub async fn wait_ready(cancel: &CancellationToken) -> Result<(), &'static str> {
    let path = PathBuf::from(std::env::var_os(READY).ok_or("Missing worker startup coordination")?);
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if path.is_file() {
            return Ok(());
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err("Credential-helper startup interrupted"),
            _ = tokio::time::sleep_until(deadline) => return Err("Credential-helper startup deadline exceeded"),
            _ = tokio::time::sleep(POLL) => {}
        }
    }
}

pub fn configuration_complete() -> io::Result<()> {
    let ready = PathBuf::from(
        std::env::var_os(READY)
            .ok_or_else(|| io::Error::other("Missing worker startup coordination"))?,
    );
    let directory = ready
        .parent()
        .ok_or_else(|| io::Error::other("Invalid worker startup coordination"))?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(directory.join("configured"))?;
    Ok(())
}

pub fn relay_interrupt(parent: Pid) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let stopping = std::env::var_os(READY)
                .map(PathBuf::from)
                .and_then(|path| path.parent().map(|parent| parent.join("stopping")))
                .is_some_and(|path| path.is_file());
            if !stopping && getppid() == parent {
                let _ = kill(parent, Signal::SIGINT);
            }
        }
    })
}

struct StartupDirectory(PathBuf);
impl StartupDirectory {
    fn new() -> io::Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for sequence in 0..64 {
            let path = std::env::temp_dir().join(format!(
                "view-storage-auth-{}-{stamp}-{sequence}",
                std::process::id()
            ));
            match DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::other(
            "Cannot allocate owned startup coordination",
        ))
    }
    fn ready(&self) -> PathBuf {
        self.0.join("ready")
    }
    fn release(&self) -> io::Result<()> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.ready())?;
        Ok(())
    }
}
impl StartupDirectory {
    fn stopping(&self) -> io::Result<()> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.0.join("stopping"))?;
        Ok(())
    }
    fn configured(&self) -> bool {
        self.0.join("configured").is_file()
    }
}
impl Drop for StartupDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.ready());
        let _ = std::fs::remove_file(self.0.join("stopping"));
        let _ = std::fs::remove_file(self.0.join("configured"));
        let _ = std::fs::remove_dir(&self.0);
    }
}

// A killed output worker cannot restore shared file-description flags itself.
struct OriginalFlags(Vec<(OwnedFd, OFlag)>);
impl OriginalFlags {
    fn capture() -> io::Result<Self> {
        let (input, output, error) = (io::stdin(), io::stdout(), io::stderr());
        let mut flags = Vec::new();
        for fd in [input.as_fd(), output.as_fd(), error.as_fd()] {
            let original = OFlag::from_bits_retain(fcntl(fd, FcntlArg::F_GETFL)?);
            flags.push((fd.try_clone_to_owned()?, original));
        }
        Ok(Self(flags))
    }
}
impl Drop for OriginalFlags {
    fn drop(&mut self) {
        for (fd, flags) in &self.0 {
            let _ = fcntl(fd, FcntlArg::F_SETFL(*flags));
        }
    }
}

// Mask SIGTTOU only during each synchronous handoff. A mask must never remain
// installed across await, where this future could move to another runtime thread.
fn foreground(fd: &OwnedFd, group: Pid) -> nix::Result<()> {
    let mut blocked = SigSet::empty();
    blocked.add(Signal::SIGTTOU);
    let mut old = SigSet::empty();
    pthread_sigmask(SigmaskHow::SIG_BLOCK, Some(&blocked), Some(&mut old))?;
    let result = tcsetpgrp(fd, group);
    pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&old), None).and(result)
}

struct Terminal(Option<(OwnedFd, Pid)>);
impl Terminal {
    fn capture() -> io::Result<Self> {
        let (input, output, error) = (io::stdin(), io::stdout(), io::stderr());
        for fd in [input.as_fd(), output.as_fd(), error.as_fd()] {
            if let Ok(owner) = tcgetpgrp(fd)
                && owner == getpgrp()
            {
                return Ok(Self(Some((fd.try_clone_to_owned()?, owner))));
            }
        }
        Ok(Self(None))
    }
    fn give_to(&self, group: Pid) -> nix::Result<()> {
        if let Some((fd, _)) = &self.0 {
            foreground(fd, group)?;
        }
        Ok(())
    }
    fn restore(&mut self) -> nix::Result<()> {
        if let Some((fd, owner)) = &self.0 {
            foreground(fd, *owner)?;
        }
        self.0 = None;
        Ok(())
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

struct Reaper {
    #[cfg(target_os = "linux")]
    previous: bool,
}
impl Reaper {
    fn new() -> nix::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            let previous = nix::sys::prctl::get_child_subreaper()?;
            nix::sys::prctl::set_child_subreaper(true)?;
            Ok(Self { previous })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(Self {})
        }
    }
}
impl Drop for Reaper {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        {
            let _ = nix::sys::prctl::set_child_subreaper(self.previous);
        }
    }
}

struct Worker {
    child: Child,
    group: Pid,
    joined: bool,
    #[cfg(target_os = "macos")]
    observer: Kqueue,
    #[cfg(target_os = "macos")]
    observed_exit: bool,
}
impl Worker {
    fn exited(&mut self) -> nix::Result<bool> {
        // Keep the leader waitable until its group is terminated. Its PID cannot
        // be reused between an exit observation and the final group signal.
        #[cfg(target_os = "linux")]
        {
            let result = waitid(
                Id::Pid(self.group),
                WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT,
            );
            match result {
                Ok(status) => Ok(!matches!(status, WaitStatus::StillAlive)),
                Err(Errno::EINTR) => Ok(false),
                Err(error) => Err(error),
            }
        }
        #[cfg(target_os = "macos")]
        {
            if !self.observed_exit {
                let mut events = [Self::exit_registration(self.group)];
                let count = self.observer.kevent(
                    &[],
                    &mut events,
                    Some(nix::libc::timespec {
                        tv_sec: 0,
                        tv_nsec: 0,
                    }),
                );
                let count = match count {
                    Ok(count) => count,
                    Err(Errno::EINTR) => return Ok(false),
                    Err(error) => return Err(error),
                };
                if count > 0 {
                    self.observe_event(events[0])?;
                }
            }
            Ok(self.observed_exit)
        }
    }
    #[cfg(target_os = "macos")]
    fn exit_registration(group: Pid) -> KEvent {
        KEvent::new(
            group.as_raw() as usize,
            EventFilter::EVFILT_PROC,
            EvFlags::EV_ADD | EvFlags::EV_ONESHOT,
            FilterFlag::NOTE_EXIT,
            0,
            0,
        )
    }
    #[cfg(target_os = "macos")]
    fn observe_event(&mut self, event: KEvent) -> nix::Result<()> {
        if event.flags().contains(EvFlags::EV_ERROR) {
            let error = Errno::from_raw(event.data() as i32);
            if error != Errno::ESRCH {
                return Err(error);
            }
            self.observed_exit = true;
        } else if event.fflags().contains(FilterFlag::NOTE_EXIT) {
            self.observed_exit = true;
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    fn register_exit(&mut self) -> nix::Result<()> {
        let registration = Self::exit_registration(self.group);
        let mut events = [registration];
        match self.observer.kevent(
            &[registration],
            &mut events,
            Some(nix::libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            }),
        ) {
            Ok(0) => Ok(()),
            Ok(_) => self.observe_event(events[0]),
            Err(Errno::ESRCH) => {
                self.observed_exit = true;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
    fn finish(&mut self) -> io::Result<ExitStatus> {
        match killpg(self.group, Signal::SIGKILL) {
            Ok(()) | Err(Errno::ESRCH) => {}
            // XNU's POSIX killpg skips zombies and returns EPERM when there
            // are no signalable live members. Accept that only after observing
            // our unreaped leader's exit; never mask EPERM for a live worker.
            #[cfg(target_os = "macos")]
            Err(Errno::EPERM) if self.exited()? => {}
            Err(error) => {
                let _ = self.child.kill();
                return Err(error.into());
            }
        }
        let status = self.child.wait()?;
        self.joined = true;
        #[cfg(target_os = "linux")]
        {
            let deadline = std::time::Instant::now() + Duration::from_millis(200);
            loop {
                match waitpid(
                    Pid::from_raw(-self.group.as_raw()),
                    Some(WaitPidFlag::WNOHANG),
                ) {
                    Err(Errno::ECHILD) => break,
                    Err(Errno::EINTR) => continue,
                    Err(error) => return Err(error.into()),
                    Ok(WaitStatus::StillAlive) if std::time::Instant::now() >= deadline => {
                        return Err(io::Error::other(
                            "Cannot reap terminated helper descendants",
                        ));
                    }
                    Ok(WaitStatus::StillAlive) => std::thread::sleep(POLL),
                    Ok(_) => {}
                }
            }
        }
        Ok(status)
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        if !self.joined {
            let _ = self.finish();
        }
    }
}

pub async fn supervise(timeout: Duration, cancel: CancellationToken) -> Result<Exit, String> {
    if cancel.is_cancelled() {
        return Ok(Exit::Interrupted);
    }
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or("Unsupported configuration deadline")?;
    let shared_deadline = monotonic_nanos()
        .map_err(|_| "Cannot read shared monotonic deadline")?
        .checked_add(timeout.as_nanos())
        .ok_or("Unsupported configuration deadline")?;
    let _flags =
        OriginalFlags::capture().map_err(|_| "Cannot preserve command descriptor flags")?;
    let _reaper =
        Reaper::new().map_err(|_| "Cannot establish credential-helper process ownership")?;
    let startup =
        StartupDirectory::new().map_err(|_| "Cannot create private worker coordination")?;
    let mut terminal = Terminal::capture().map_err(|_| "Cannot preserve foreground terminal")?;
    #[cfg(target_os = "macos")]
    let observer = Kqueue::new().map_err(|_| "Cannot create non-reaping worker observer")?;
    let child =
        Command::new(std::env::current_exe().map_err(|_| "Cannot locate command executable")?)
            .args(std::env::args_os().skip(1))
            .env(PARENT, std::process::id().to_string())
            .env(READY, startup.ready())
            .env(DEADLINE, shared_deadline.to_string())
            .process_group(0)
            .spawn()
            .map_err(|_| "Cannot start credential-helper worker")?;
    let raw_pid =
        i32::try_from(child.id()).map_err(|_| "Invalid credential-helper worker identity")?;
    let mut worker = Worker {
        child,
        group: Pid::from_raw(raw_pid),
        joined: false,
        #[cfg(target_os = "macos")]
        observer,
        #[cfg(target_os = "macos")]
        observed_exit: false,
    };
    #[cfg(target_os = "macos")]
    worker
        .register_exit()
        .map_err(|_| "Cannot register non-reaping worker observation")?;
    terminal
        .give_to(worker.group)
        .map_err(|_| "Cannot hand off foreground terminal")?;
    startup
        .release()
        .map_err(|_| "Cannot release credential-helper worker")?;
    let _ = killpg(worker.group, Signal::SIGCONT);
    let mut stopped = None;
    loop {
        if cancel.is_cancelled() {
            stopped = Some(Exit::Interrupted);
            break;
        }
        // Guard synchronous SDK construction. Once configured, the collector
        // owns request/stage deadlines and may emit retained partial inventory.
        // Report finalization/output have the same semantics as the direct path.
        let configured = startup.configured();
        if !configured && Instant::now() >= deadline {
            stopped = Some(Exit::Fatal);
            break;
        }
        if worker
            .exited()
            .map_err(|_| "Cannot observe credential-helper worker")?
        {
            break;
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {},
            _ = tokio::time::sleep_until(deadline), if !configured => {},
            _ = tokio::time::sleep(POLL) => {},
        }
    }
    if stopped.is_some() && startup.stopping().is_ok() {
        let _ = killpg(worker.group, Signal::SIGINT);
        let drain = Instant::now() + Duration::from_secs(1);
        while Instant::now() < drain
            && !worker
                .exited()
                .map_err(|_| "Cannot drain credential-helper worker")?
        {
            tokio::time::sleep(POLL).await;
        }
    }
    let status = worker.finish().map_err(|error| {
        format!(
            "Cannot join credential-helper worker and descendants (OS status {:?})",
            error.raw_os_error()
        )
    })?;
    terminal
        .restore()
        .map_err(|_| "Cannot restore foreground terminal")?;
    match stopped {
        Some(Exit::Fatal) => {
            Err("Credential-helper execution exceeded the command deadline".into())
        }
        Some(exit) => Ok(exit),
        None => Ok(match status.code() {
            Some(0) => Exit::Success,
            Some(2) => Exit::Invalid,
            Some(3) => Exit::Findings,
            Some(4) => Exit::Inconclusive,
            Some(130) => Exit::Interrupted,
            _ => Exit::Fatal,
        }),
    }
}
