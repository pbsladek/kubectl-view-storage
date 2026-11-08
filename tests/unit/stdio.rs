#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use std::{
    io::Read,
    sync::{Arc, mpsc},
    time::Instant,
};

fn assert_restored(fd: impl AsFd, original: OFlags) {
    // Darwin also exposes the kernel's FWASWRITTEN history bit in F_GETFL.
    // It is not settable status and legitimately changes after a successful write.
    let status = OFlags::O_NONBLOCK
        | OFlags::O_APPEND
        | OFlags::O_ASYNC
        | OFlags::O_SYNC
        | OFlags::O_DSYNC
        | OFlags::O_ACCMODE;
    assert_eq!(fcntl_getfl(fd).unwrap() & status, original & status);
}

#[test]
fn successful_and_failed_writes_restore_descriptor_flags() {
    let (mut reader, writer) = io::pipe().unwrap();
    let original = fcntl_getfl(&writer).unwrap();
    with_writer(&writer, CancellationToken::new(), |out| {
        out.write_all(b"report\n")
    })
    .unwrap();
    assert_restored(&writer, original);
    let mut bytes = [0; 7];
    reader.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"report\n");
    with_writer(&writer, CancellationToken::new(), |out| {
        out.write_all(b"discard")?;
        Err(io::Error::other("controlled failure"))
    })
    .unwrap_err();
    assert_restored(&writer, original);
    // The failed operation's buffered bytes must not be flushed during Drop.
    fcntl_setfl(&reader, fcntl_getfl(&reader).unwrap() | OFlags::O_NONBLOCK).unwrap();
    assert_eq!(
        reader.read(&mut bytes).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
}

#[test]
fn full_pipe_cancellation_joins_and_leaves_no_late_bytes() {
    let (reader, writer) = io::pipe().unwrap();
    let (done, wait) = mpsc::channel();
    let rescue = std::thread::spawn(move || {
        // A regression that ignores cancellation must fail instead of leaving
        // a permanently full pipe at test teardown. Closing its reader releases
        // an OS write if the normal cancellation path fails.
        wait.recv_timeout(Duration::from_secs(3))
            .ok()
            .map(|()| reader)
    });
    let writer = Arc::new(writer);
    let original = fcntl_getfl(&writer).unwrap();
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let worker_writer = writer.clone();
    let worker = std::thread::spawn(move || {
        with_writer(worker_writer, worker_cancel, |out| {
            out.write_all(&vec![b'x'; 8 * 1024 * 1024])
        })
    });
    // The reader consumes nothing. A large report must fill the real OS pipe.
    std::thread::sleep(Duration::from_millis(75));
    let stopped = Instant::now();
    cancel.cancel();
    let error = worker.join().unwrap().unwrap_err();
    let _ = done.send(());
    let mut reader = rescue
        .join()
        .unwrap()
        .expect("output worker exceeded watchdog");
    assert_ne!(
        error.kind(),
        io::ErrorKind::Interrupted,
        "write_all would retry cancellation"
    );
    assert!(stopped.elapsed() < Duration::from_secs(1));
    assert_restored(&writer, original);
    fcntl_setfl(&reader, fcntl_getfl(&reader).unwrap() | OFlags::O_NONBLOCK).unwrap();
    let mut bytes = vec![0; 64 * 1024];
    let mut received = 0;
    loop {
        match reader.read(&mut bytes) {
            Ok(0) => break,
            Ok(count) => received += count,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("pipe drain failed: {error}"),
        }
    }
    assert!(received > 0 && received < 8 * 1024 * 1024);
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(
        reader.read(&mut bytes).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
}

#[test]
fn precancelled_closed_and_panicking_destinations_do_not_claim_success() {
    let (reader, writer) = io::pipe().unwrap();
    let original = fcntl_getfl(&writer).unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        with_writer(&writer, cancel, |_| panic!(
            "must not start cancelled operation"
        ))
        .is_err()
    );
    assert_restored(&writer, original);
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _ = with_writer(&writer, CancellationToken::new(), |out| {
            out.write_all(b"discard on panic").unwrap();
            panic!("controlled writer panic");
        });
    }));
    assert!(panic.is_err());
    assert_restored(&writer, original);
    drop(reader);
    assert_eq!(
        with_writer(&writer, CancellationToken::new(), |out| out
            .write_all(b"closed"))
        .unwrap_err()
        .kind(),
        io::ErrorKind::BrokenPipe
    );
    assert_restored(&writer, original);
}

#[test]
fn an_already_nonblocking_descriptor_keeps_its_original_mode() {
    let (mut reader, writer) = io::pipe().unwrap();
    let original = fcntl_getfl(&writer).unwrap() | OFlags::O_NONBLOCK;
    fcntl_setfl(&writer, original).unwrap();
    with_writer(&writer, CancellationToken::new(), |out| {
        out.write_all(b"ok")
    })
    .unwrap();
    assert_restored(&writer, original);
    let mut bytes = [0; 2];
    reader.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"ok");
}
