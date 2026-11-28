//! Process-wide, best-effort stderr logging (#1180).
//!
//! Each nonempty write is one complete record (the caller supplies its newline).
//! The limit includes the record currently being written; overflow drops the
//! entire record. The next record the writer takes is preceded by `okkama: N log lines dropped\n` for drops since its last notice.
//! Counts are cumulative and saturating. Any sink error disables the writer and
//! counts the failed record, queued records and every later write as dropped.
//!
//! Drains snapshot accepted records and wait at most two seconds, including time
//! spent waiting for a host pool worker. They never join the writer. Standard
//! output flushes (including exit and failure reports) and direct stderr writes
//! use this same bound. Ordering with direct stderr is guaranteed when the drain
//! completes; after timeout it is best effort. Direct stderr itself stays
//! synchronous and fallible, and can still block on its own write.
//!
//! Captures and embedding sinks receive bytes synchronously on their VM thread.
//! Builds without the host (including wasm32) use direct, error-ignoring writes
//! without a writer thread. Their buffer-limit setter does nothing.
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub const DEFAULT_CAPACITY: usize = 1024 * 1024;
pub const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

fn add(counter: &AtomicU64, count: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| Some(n.saturating_add(count)));
}

/// Copy a whole line without waiting for the destination. Empty writes do nothing.
pub fn write(bytes: &[u8]) {
    if bytes.is_empty() || crate::stdout::redirect_error(bytes) {
        return;
    }
    #[cfg(all(feature = "host", not(target_arch = "wasm32")))]
    host::sink().write(bytes);
    #[cfg(not(all(feature = "host", not(target_arch = "wasm32"))))]
    {
        use std::io::Write;
        if FAILED.load(Ordering::Relaxed) || std::io::stderr().write_all(bytes).is_err() {
            FAILED.store(true, Ordering::Relaxed);
            add(&DROPPED, 1);
        }
    }
}

/// Change the process limit for subsequent admissions; existing records stay.
/// Zero drops every line. Shrinking does not evict accepted records.
pub fn set_capacity(bytes: usize) {
    if crate::stdout::error_redirected() { return; }
    #[cfg(all(feature = "host", not(target_arch = "wasm32")))]
    { host::sink().set_capacity(bytes); }
    #[cfg(not(all(feature = "host", not(target_arch = "wasm32"))))]
    let _ = bytes;
}

#[cfg(not(all(feature = "host", not(target_arch = "wasm32"))))]
static DROPPED: AtomicU64 = AtomicU64::new(0);
#[cfg(not(all(feature = "host", not(target_arch = "wasm32"))))]
static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn dropped() -> u64 {
    #[cfg(all(feature = "host", not(target_arch = "wasm32")))]
    { host::existing().map_or(0, |sink| sink.shared.dropped.load(Ordering::Relaxed)) }
    #[cfg(not(all(feature = "host", not(target_arch = "wasm32"))))]
    { DROPPED.load(Ordering::Relaxed) }
}

/// A deadline and watermark captured before an asynchronous wait is scheduled.
#[derive(Debug)]
pub struct Drain {
    #[cfg(all(feature = "host", not(target_arch = "wasm32")))]
    pending: Option<host::Pending>,
}

pub fn snapshot() -> Drain {
    Drain {
        #[cfg(all(feature = "host", not(target_arch = "wasm32")))]
        pending: if crate::stdout::error_redirected() { None } else { host::existing().map(|sink| sink.snapshot(DRAIN_TIMEOUT)) },
    }
}

impl Drain {
    /// True if the watermark completed (including a failed sink), false on timeout.
    pub fn wait(self) -> bool {
        #[cfg(all(feature = "host", not(target_arch = "wasm32")))]
        if let Some(pending) = self.pending {
            return pending.wait();
        }
        true
    }
}

pub fn drain() {
    let _ = snapshot().wait();
}

#[cfg(all(feature = "host", not(target_arch = "wasm32")))]
mod host {
    use super::*;
    use std::{
        collections::VecDeque,
        io::{self, Write},
        sync::{Arc, Condvar, Mutex, OnceLock, PoisonError},
        time::Instant,
    };

    #[derive(Debug)]
    struct State {
        lines: VecDeque<Vec<u8>>,
        bytes: usize,
        capacity: usize,
        accepted: u64,
        completed: u64,
        failed: bool,
        closed: bool,
    }

    #[derive(Debug)]
    pub(super) struct Shared {
        state: Mutex<State>,
        changed: Condvar,
        pub(super) dropped: AtomicU64,
        notice: AtomicU64,
    }

    #[derive(Debug)]
    pub(super) struct Sink {
        pub(super) shared: Arc<Shared>,
    }

    static SINK: OnceLock<Sink> = OnceLock::new();

    pub(super) fn existing() -> Option<&'static Sink> { SINK.get() }

    // A separate OS handle avoids holding Rust's global stderr lock across a
    // stalled write, so a timed-out drain never leaves that lock captive.
    fn stderr() -> io::Result<std::fs::File> {
        #[cfg(unix)]
        {
            use std::os::fd::AsFd;
            io::stderr().as_fd().try_clone_to_owned().map(Into::into)
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsHandle;
            io::stderr().as_handle().try_clone_to_owned().map(Into::into)
        }
        #[cfg(not(any(unix, windows)))]
        { Err(io::Error::from(io::ErrorKind::Unsupported)) }
    }

    pub(super) fn sink() -> &'static Sink {
        SINK.get_or_init(|| Sink::new(stderr(), DEFAULT_CAPACITY))
    }

    impl Sink {
        fn new<W: Write + Send + 'static>(writer: io::Result<W>, capacity: usize) -> Self {
            let shared = Arc::new(Shared {
                state: Mutex::new(State {
                    lines: VecDeque::new(), bytes: 0, capacity, accepted: 0,
                    completed: 0, failed: writer.is_err(), closed: false,
                }),
                changed: Condvar::new(), dropped: AtomicU64::new(0), notice: AtomicU64::new(0),
            });
            if let Ok(writer) = writer {
                let into = shared.clone();
                if std::thread::Builder::new().name("okkama-log".into()).spawn(move || into.run(writer)).is_err() {
                    shared.state.lock().unwrap_or_else(PoisonError::into_inner).failed = true;
                }
            }
            Self { shared }
        }

        pub(super) fn set_capacity(&self, bytes: usize) {
            self.shared.state.lock().unwrap_or_else(PoisonError::into_inner).capacity = bytes;
        }

        pub(super) fn write(&self, bytes: &[u8]) {
            if bytes.is_empty() { return; }
            // The writer never holds this lock while touching the destination.
            let mut state = self.shared.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.failed || bytes.len() > state.capacity.saturating_sub(state.bytes) {
                self.shared.drop_lines(1);
                return;
            }
            state.lines.push_back(bytes.to_vec());
            state.bytes += bytes.len();
            state.accepted += 1;
            self.shared.changed.notify_all();
        }

        pub(super) fn snapshot(&self, bound: Duration) -> Pending {
            let deadline = Instant::now() + bound;
            let target = self.shared.state.lock().unwrap_or_else(PoisonError::into_inner).accepted;
            Pending { shared: self.shared.clone(), target, deadline }
        }
    }

    impl Drop for Sink {
        fn drop(&mut self) {
            self.shared.state.lock().unwrap_or_else(PoisonError::into_inner).closed = true;
            self.shared.changed.notify_all();
        }
    }

    impl Shared {
        fn drop_lines(&self, count: u64) {
            add(&self.dropped, count);
            add(&self.notice, count);
        }

        fn run(&self, mut writer: impl Write) {
            loop {
                let (line, dropped) = {
                    let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                    while state.lines.is_empty() && !state.closed {
                        state = self.changed.wait(state).unwrap_or_else(PoisonError::into_inner);
                    }
                    let Some(line) = state.lines.pop_front() else { return; };
                    (line, self.notice.swap(0, Ordering::Relaxed))
                };
                let result = (|| {
                    if dropped != 0 {
                        writeln!(writer, "okkama: {dropped} log lines dropped")?;
                    }
                    writer.write_all(&line)?;
                    writer.flush()
                })();
                let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                if result.is_err() {
                    self.drop_lines(1 + state.lines.len() as u64);
                    state.lines.clear();
                    state.bytes = 0;
                    state.failed = true;
                    state.completed = state.accepted;
                    self.changed.notify_all();
                    return;
                }
                state.bytes -= line.len();
                state.completed += 1;
                self.changed.notify_all();
            }
        }
    }

    #[derive(Debug)]
    pub(super) struct Pending {
        shared: Arc<Shared>,
        target: u64,
        deadline: Instant,
    }

    impl Pending {
        pub(super) fn wait(self) -> bool {
            let mut state = self.shared.state.lock().unwrap_or_else(PoisonError::into_inner);
            while state.completed < self.target {
                let left = self.deadline.saturating_duration_since(Instant::now());
                if left.is_zero() { return false; }
                state = self.shared.changed.wait_timeout(state, left).unwrap_or_else(PoisonError::into_inner).0;
            }
            true
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::mpsc::{self, Receiver, Sender};

        #[derive(Clone)]
        struct Bytes(Arc<Mutex<Vec<u8>>>);
        impl Write for Bytes {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> { Ok(()) }
        }

        struct Gated<W> {
            writer: W,
            started: Sender<()>,
            release: Receiver<()>,
            every: bool,
            first: bool,
        }
        impl<W: Write> Write for Gated<W> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.first || self.every {
                    self.first = false;
                    let _ = self.started.send(());
                    self.release.recv_timeout(Duration::from_secs(5)).map_err(|_| io::ErrorKind::TimedOut)?;
                }
                self.writer.write(bytes)
            }
            fn flush(&mut self) -> io::Result<()> { self.writer.flush() }
        }

        fn gated<W: Write>(writer: W, every: bool) -> (Gated<W>, Receiver<()>, Sender<()>) {
            let (started, begin) = mpsc::channel();
            let (release, wait) = mpsc::channel();
            (Gated { writer, started, release: wait, every, first: true }, begin, release)
        }

        fn bytes() -> Bytes { Bytes(Arc::new(Mutex::new(Vec::new()))) }
        fn started(begin: &Receiver<()>) { begin.recv_timeout(Duration::from_secs(2)).unwrap(); }

        #[test]
        fn overflow_keeps_whole_lines_and_reports_exact_count() {
            let output = bytes();
            let (writer, begin, release) = gated(output.clone(), false);
            let sink = Sink::new(Ok(writer), 8);
            sink.write(b"one\n");
            started(&begin);
            sink.write(b"two\n");
            sink.write(b"oversized\n");
            sink.write(b"bad\n");
            assert_eq!(sink.shared.dropped.load(Ordering::Relaxed), 2);
            release.send(()).unwrap();
            assert!(sink.snapshot(DRAIN_TIMEOUT).wait());
            assert_eq!(*output.0.lock().unwrap(), b"one\nokkama: 2 log lines dropped\ntwo\n");
            assert_eq!(sink.shared.dropped.load(Ordering::Relaxed), 2);
        }

        #[test]
        fn drain_waits_for_its_watermark_not_later_lines() {
            let output = bytes();
            let (writer, begin, release) = gated(output.clone(), true);
            let sink = Sink::new(Ok(writer), 8);
            sink.write(b"one\n");
            started(&begin);
            let drain = sink.snapshot(DRAIN_TIMEOUT);
            sink.write(b"two\n");
            let (done, finished) = mpsc::channel();
            let waiter = std::thread::spawn(move || done.send(drain.wait()).unwrap());
            assert!(finished.recv_timeout(Duration::from_millis(30)).is_err());
            release.send(()).unwrap();
            started(&begin);
            assert!(finished.recv_timeout(Duration::from_secs(1)).unwrap());
            assert_eq!(*output.0.lock().unwrap(), b"one\n");
            release.send(()).unwrap();
            waiter.join().unwrap();
            assert!(sink.snapshot(DRAIN_TIMEOUT).wait());
        }

        #[test]
        fn direct_stderr_waits_for_accepted_lines_and_keeps_bytes() {
            let output = bytes();
            let (writer, begin, release) = gated(output.clone(), false);
            let sink = Sink::new(Ok(writer), 64);
            sink.write(b"log\n");
            started(&begin);
            let drain = super::super::Drain { pending: Some(sink.snapshot(DRAIN_TIMEOUT)) };
            let mut direct = output.clone();
            let (done, finished) = mpsc::channel();
            let waiter = std::thread::spawn(move || {
                let result = crate::stdout::direct_error(b"direct\0\xff\n", drain, &mut direct);
                done.send(result).unwrap();
            });
            assert!(finished.recv_timeout(Duration::from_millis(30)).is_err());
            assert!(output.0.lock().unwrap().is_empty());
            release.send(()).unwrap();
            finished.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
            waiter.join().unwrap();
            assert_eq!(*output.0.lock().unwrap(), b"log\ndirect\0\xff\n");
        }

        #[test]
        fn shrinking_limit_preserves_accepted_lines_and_zero_drops() {
            let output = bytes();
            let (writer, begin, release) = gated(output.clone(), false);
            let sink = Sink::new(Ok(writer), 8);
            sink.write(b"one\n");
            started(&begin);
            sink.set_capacity(0);
            sink.write(b"two\n");
            release.send(()).unwrap();
            assert!(sink.snapshot(DRAIN_TIMEOUT).wait());
            assert_eq!(*output.0.lock().unwrap(), b"one\n");
            assert_eq!(sink.shared.dropped.load(Ordering::Relaxed), 1);
        }

        #[test]
        fn concurrent_producers_do_not_drop_when_there_is_room() {
            let output = bytes();
            let (writer, begin, release) = gated(output.clone(), false);
            let sink = Sink::new(Ok(writer), 4096);
            sink.write(b"one\n");
            started(&begin);
            std::thread::scope(|scope| {
                for _ in 0..8 {
                    let sink = &sink;
                    scope.spawn(move || {
                        for _ in 0..100 { sink.write(b"x\n"); }
                    });
                }
            });
            assert_eq!(sink.shared.dropped.load(Ordering::Relaxed), 0);
            release.send(()).unwrap();
            assert!(sink.snapshot(DRAIN_TIMEOUT).wait());
            let mut expected = b"one\n".to_vec();
            expected.extend_from_slice(&b"x\n".repeat(800));
            assert_eq!(*output.0.lock().unwrap(), expected);
        }

        #[test]
        fn write_errors_disable_sink_and_count_all_unwritten_lines() {
            for kind in [io::ErrorKind::BrokenPipe, io::ErrorKind::Other] {
                struct Broken(io::ErrorKind);
                impl Write for Broken {
                    fn write(&mut self, _: &[u8]) -> io::Result<usize> { Err(self.0.into()) }
                    fn flush(&mut self) -> io::Result<()> { Ok(()) }
                }
                let (writer, begin, release) = gated(Broken(kind), false);
                let sink = Sink::new(Ok(writer), 64);
                sink.write(b"failed\n");
                started(&begin);
                sink.write(b"queued\n");
                release.send(()).unwrap();
                assert!(sink.snapshot(DRAIN_TIMEOUT).wait());
                sink.write(b"later\n");
                assert_eq!(sink.shared.dropped.load(Ordering::Relaxed), 3);
                assert!(sink.shared.state.lock().unwrap().failed);
            }
        }

        #[test]
        fn failed_open_counts_every_line() {
            let sink = Sink::new::<io::Sink>(Err(io::ErrorKind::PermissionDenied.into()), 64);
            sink.write(b"line\n");
            assert!(sink.snapshot(DRAIN_TIMEOUT).wait());
            assert_eq!(sink.shared.dropped.load(Ordering::Relaxed), 1);
        }

        #[cfg(unix)]
        #[test]
        fn stalled_pipe_keeps_writes_fast_and_exit_drain_bounded() {
            use std::io::Read;
            // A line larger than the kernel pipe capacity cannot finish until
            // this test starts reading. No sleeps or uncontrolled reader.
            let (mut reader, writer) = std::io::pipe().unwrap();
            let (writer, begin, release) = gated(writer, false);
            let line = vec![b'x'; 1024 * 1024];
            let sink = Sink::new(Ok(writer), line.len());
            sink.write(&line);
            started(&begin);
            release.send(()).unwrap();
            let start = Instant::now();
            for _ in 0..1000 { sink.write(b"dropped\n"); }
            assert!(start.elapsed() < Duration::from_millis(250));
            let start = Instant::now();
            assert!(!sink.snapshot(DRAIN_TIMEOUT).wait());
            assert!(start.elapsed() >= DRAIN_TIMEOUT);
            assert!(start.elapsed() < DRAIN_TIMEOUT + Duration::from_millis(500));
            assert_eq!(sink.shared.dropped.load(Ordering::Relaxed), 1000);
            let mut received = vec![0; line.len()];
            reader.read_exact(&mut received).unwrap();
            assert_eq!(received, line);
            assert!(sink.snapshot(DRAIN_TIMEOUT).wait());
        }

        #[cfg(unix)]
        #[test]
        fn closed_pipe_is_silent_and_counted() {
            let (reader, writer) = std::io::pipe().unwrap();
            drop(reader);
            let sink = Sink::new(Ok(writer), 64);
            sink.write(b"broken\n");
            assert!(sink.snapshot(DRAIN_TIMEOUT).wait());
            sink.write(b"later\n");
            assert_eq!(sink.shared.dropped.load(Ordering::Relaxed), 2);
        }
    }

}

#[cfg(test)]
mod tests {
    #[test]
    fn log_capture_keeps_bytes_and_direct_stderr_order() {
        let ((), out, err) = crate::stdout::capture_streams(|| {
            super::write(b"log\0\xff\n");
            crate::stdout::write_error(b"direct\r\n").unwrap();
            super::drain();
        });
        assert!(out.is_empty());
        assert_eq!(err, b"log\0\xff\ndirect\r\n");
    }
}
