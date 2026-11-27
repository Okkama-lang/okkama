//! Standard output of the process, buffered (#132). Every `Stdout` value, the
//! test report and the command line's own output go through here, so their
//! order holds. A terminal gets every line as it ends; a pipe or a file gets
//! the bytes in blocks of 64 KiB.
//!
//! Pending bytes go out on [`flush`]. The runtimes call it before standard
//! error is written, before a child process starts or the terminal is
//! written, and when the program ends; [`before_input`] flushes before
//! standard input is read from a terminal, so a prompt shows, as C stdio does.
//!
//! A process whose output must survive `SIGKILL` (a test worker that a
//! `--test-timeout` may kill, #1011) calls [`write_through`] first: every
//! write then goes out at once.
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

thread_local! {
    static CAPTURE: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
    /// Standard error of a run inside [`capture_streams`] (#1121).
    static CAPTURE_ERROR: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
    static TO_STDERR: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The receiver of a run inside [`with_sink`] (#1125).
    static SINK: std::cell::RefCell<Option<Sink>> = const { std::cell::RefCell::new(None) };
}

/// The stream a [`with_sink`] receiver gets bytes of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Output,
    Error,
}

/// What [`with_sink`] hands every write to, as it happens.
pub type Sink = Box<dyn FnMut(Stream, &[u8])>;

/// Hand what the run on the current VM thread writes to standard output and, through [`write_error`], to
/// standard error to `sink` while `body` runs, each write as it happens and nothing buffered (#1125): the
/// playground marks every piece with the time it was written. A [`capture`] inside it takes precedence; the
/// guard restores the previous receiver even if `body` panics. The sink must not write here itself.
pub fn with_sink<T>(sink: Sink, body: impl FnOnce() -> T) -> T {
    struct Restore(Option<Sink>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SINK.with(|current| *current.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(SINK.with(|current| current.replace(Some(sink))));
    body()
}

/// Hand `bytes` to the [`with_sink`] receiver in progress, if any.
fn sunk(stream: Stream, bytes: &[u8]) -> bool {
    SINK.with(|current| match current.borrow_mut().as_mut() {
        Some(sink) => {
            sink(stream, bytes);
            true
        }
        None => false,
    })
}

/// Send what the program on the current VM thread writes to standard output
/// to standard error instead, while `body` runs: a machine-readable test report
/// then owns standard output alone (#339). Nothing is buffered on the way; the
/// guard restores the previous destination even if `body` panics.
pub fn to_stderr<T>(body: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            TO_STDERR.with(|flag| flag.set(self.0));
        }
    }
    let _restore = Restore(TO_STDERR.with(|flag| flag.replace(true)));
    body()
}

/// Capture a bounded embedding run on the current VM thread, including explicit flushes.
/// The guard restores the previous destination even if the embedding panics.
pub fn capture<T>(body: impl FnOnce() -> T) -> (T, Vec<u8>) {
    struct Restore(Option<Vec<u8>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CAPTURE.with(|capture| *capture.borrow_mut() = self.0.take());
        }
    }
    let previous = CAPTURE.with(|capture| capture.replace(Some(Vec::new())));
    let restore = Restore(previous);
    let result = body();
    let output = CAPTURE.with(|capture| capture.borrow_mut().take().unwrap_or_default());
    drop(restore);
    (result, output)
}

/// [`capture`] of both streams (#1121): what the run on the current VM thread writes to standard output and, through
/// [`write_error`], to standard error, each apart. The differential test of Sparrow's two engines compares both.
pub fn capture_streams<T>(body: impl FnOnce() -> T) -> (T, Vec<u8>, Vec<u8>) {
    struct Restore(Option<Vec<u8>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CAPTURE_ERROR.with(|capture| *capture.borrow_mut() = self.0.take());
        }
    }
    let previous = CAPTURE_ERROR.with(|capture| capture.replace(Some(Vec::new())));
    let restore = Restore(previous);
    let (result, output) = capture(body);
    let error = CAPTURE_ERROR.with(|capture| capture.borrow_mut().take().unwrap_or_default());
    drop(restore);
    (result, output, error)
}

/// Write `bytes` to standard error, after the standard output pending before it (the order of the two holds),
/// or into a [`capture_streams`] in progress.
pub fn write_error(bytes: &[u8]) -> io::Result<()> {
    let captured = CAPTURE_ERROR.with(|capture| match capture.borrow_mut().as_mut() {
        Some(error) => {
            error.extend_from_slice(bytes);
            true
        }
        None => false,
    });
    if captured || sunk(Stream::Error, bytes) {
        return Ok(());
    }
    flush()?;
    io::stderr().lock().write_all(bytes)
}

/// The block a pipe or a file gets at once.
const BLOCK: usize = 64 * 1024;

/// Set by [`write_through`]: nothing waits in the buffer.
static WRITE_THROUGH: AtomicBool = AtomicBool::new(false);

/// From now on, for the rest of the process, every write goes out at once
/// (after anything already pending): a process that may be killed without
/// warning loses nothing it wrote, at the cost of one system call per write.
pub fn write_through() -> io::Result<()> {
    WRITE_THROUGH.store(true, Ordering::Relaxed);
    flush()
}

fn buffer() -> &'static Mutex<Vec<u8>> {
    static PENDING: OnceLock<Mutex<Vec<u8>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(Vec::new()))
}

fn pending() -> MutexGuard<'static, Vec<u8>> {
    buffer().lock().unwrap_or_else(PoisonError::into_inner)
}

/// Is standard output a terminal (line buffering)?
pub fn terminal() -> bool {
    static TERMINAL: OnceLock<bool> = OnceLock::new();
    *TERMINAL.get_or_init(|| io::stdout().is_terminal())
}

/// Append `bytes`: a terminal gets every complete line now, and a full block
/// goes out at once.
pub fn write(bytes: &[u8]) -> io::Result<()> {
    if CAPTURE.with(|capture| {
        let mut capture = capture.borrow_mut();
        if let Some(output) = capture.as_mut() {
            output.extend_from_slice(bytes);
            true
        } else {
            false
        }
    }) || sunk(Stream::Output, bytes)
    {
        return Ok(());
    }
    if TO_STDERR.with(std::cell::Cell::get) {
        let mut err = io::stderr().lock();
        err.write_all(bytes)?;
        return err.flush();
    }
    let mut pending = pending();
    pending.extend_from_slice(bytes);
    if WRITE_THROUGH.load(Ordering::Relaxed) {
        let all = std::mem::take(&mut *pending);
        return emit(&all);
    }
    if terminal() {
        if let Some(end) = pending.iter().rposition(|&byte| byte == b'\n') {
            let rest = pending.split_off(end + 1);
            let lines = std::mem::replace(&mut *pending, rest);
            return emit(&lines);
        }
    } else if pending.len() >= BLOCK {
        let block = std::mem::take(&mut *pending);
        return emit(&block);
    }
    Ok(())
}

/// Write out everything pending.
pub fn flush() -> io::Result<()> {
    if CAPTURE.with(|capture| capture.borrow().is_some()) || SINK.with(|current| current.borrow().is_some()) {
        return Ok(());
    }
    let mut pending = pending();
    let all = std::mem::take(&mut *pending);
    emit(&all)
}

/// Flush before standard input is read: on a terminal a prompt must show.
pub fn before_input() -> io::Result<()> {
    if terminal() { flush() } else { Ok(()) }
}

fn emit(bytes: &[u8]) -> io::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    let mut out = io::stdout().lock();
    out.write_all(bytes)?;
    out.flush()
}

/// A [`Write`] over the process buffer, for writers handed to a runtime or a
/// test report.
#[derive(Debug, Default, Clone, Copy)]
pub struct Buffered;

impl Write for Buffered {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        write(bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        flush()
    }
}

/// [`flush`] unless another thread holds the buffer: a watchdog that stops
/// the process must not wait for it.
pub fn try_flush() {
    if let Ok(mut pending) = buffer().try_lock() {
        let all = std::mem::take(&mut *pending);
        let _ = emit(&all);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #339: inside to_stderr nothing reaches the standard output buffer, and
    /// the buffer is used again afterwards.
    #[test]
    fn to_stderr_keeps_program_output_out_of_stdout() {
        let before = pending().len();
        to_stderr(|| write(b"diverted\n").unwrap());
        assert_eq!(pending().len(), before);
        assert!(!TO_STDERR.with(std::cell::Cell::get));
        let (_, captured) = capture(|| to_stderr(|| write(b"captured").unwrap()));
        assert_eq!(captured, b"captured");
    }

    /// #1121: both streams of a run are captured apart; standard error outside a capture is not.
    #[test]
    fn capture_streams_keeps_the_two_streams_apart() {
        let ((), output, error) = capture_streams(|| {
            write(b"out").unwrap();
            write_error(b"err").unwrap();
            write(b"!").unwrap();
        });
        assert_eq!((output.as_slice(), error.as_slice()), (&b"out!"[..], &b"err"[..]));
        assert!(CAPTURE_ERROR.with(|capture| capture.borrow().is_none()));
    }

    /// #1125: a sink gets both streams' writes in order, apart, as they happen; a capture inside it wins,
    /// and outside it nothing reaches the sink.
    #[test]
    fn a_sink_gets_both_streams_in_order() {
        let pieces = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let into = pieces.clone();
        let captured = with_sink(Box::new(move |stream, bytes| into.borrow_mut().push((stream, bytes.to_vec()))), || {
            write(b"a").unwrap();
            write_error(b"b").unwrap();
            flush().unwrap();
            write(b"c").unwrap();
            capture(|| write(b"hidden").unwrap()).1
        });
        assert_eq!(captured, b"hidden");
        let expected = vec![(Stream::Output, b"a".to_vec()), (Stream::Error, b"b".to_vec()), (Stream::Output, b"c".to_vec())];
        assert_eq!(*pieces.borrow(), expected);
        assert!(SINK.with(|current| current.borrow().is_none()));
    }

    /// #1011: after write_through nothing waits in the buffer, so a process
    /// killed with SIGKILL has already written everything.
    #[test]
    fn write_through_leaves_nothing_pending() {
        let _ = write_through();
        write(b"").unwrap();
        write(b" ").unwrap();
        assert!(pending().is_empty());
        WRITE_THROUGH.store(false, Ordering::Relaxed);
    }
}
