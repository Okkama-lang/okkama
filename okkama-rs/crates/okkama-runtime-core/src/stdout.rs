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
    static TO_STDERR: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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
    }) {
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
    if CAPTURE.with(|capture| capture.borrow().is_some()) {
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
