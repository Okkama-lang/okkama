//! `runtime/application` (POSIX terminal) and `runtime/account`, which Sparrow
//! routes here (`host/native_origin.rs`): sessions on copies of
//! stdin/stdout/stderr or on `/dev/tty`, termios modes (a stack of leases),
//! geometry from `TIOCGWINSZ` and `SIGWINCH`; the host's user and group
//! databases.
#![allow(unsafe_code)]

use super::support::{array, byte_array, bytes, handle, int, int_map, int_value, items, map, new_handle, string, string_array, text};
use super::Error;
use crate::failure::{Failure, Kind};
use crate::sched;
use crate::{Value, abi::Native};
use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::{CStr, CString, c_char};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

const DOMAIN: &str = "core.application";

fn symbol(name: &str, arity: usize) -> String {
    format!("runtime/application@1::{name}/{arity}")
}

fn fail(domain: &str, code: &str, operation: &str, message: &str) -> Rc<Failure> {
    Failure::new(Kind::Native(Rc::new(crate::abi::NativeFailure {
        domain: domain.into(),
        code: code.into(),
        operation: operation.into(),
        message: message.into(),
    })))
}

fn terminal_failure(code: &str, operation: &str, message: &str) -> Rc<Failure> {
    fail(DOMAIN, code, operation, message)
}

fn as_error(failure: Rc<Failure>) -> Error {
    match &failure.kind {
        Kind::Native(native) => Error::Failure((**native).clone()),
        _ => Error::Trap(failure.message().to_string()),
    }
}

fn resolved(outcome: sched::Outcome) -> Value {
    Value::Future(sched::resolved(outcome))
}

/// Переносимый код errno.
fn errno_code(errno: i32) -> &'static str {
    match errno {
        2 => "not_found",
        1 | 13 | 30 => "permission_denied",
        4 => "interrupted",
        9 | 20 | 21 | 22 => "invalid_input",
        32 => "broken_pipe",
        12 | 23 | 24 | 28 => "resource_exhausted",
        17 => "already_exists",
        _ if libc::EAGAIN == errno || libc::EWOULDBLOCK == errno => "would_block",
        _ if cfg!(target_os = "macos") && (errno == 36 || errno == 37) => "would_block",
        _ => "unexpected",
    }
}

fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn errno_failure(errno: i32, operation: &str, message: &str) -> Rc<Failure> {
    terminal_failure(errno_code(errno), operation, message)
}

// ---- сессии ----

static OWNERS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

struct Mode {
    id: u32,
    before: libc::termios,
    released: bool,
}

struct Session {
    input: i32,
    output: i32,
    controlling: bool,
    input_interactive: bool,
    output_interactive: bool,
    original: Option<libc::termios>,
    closed: bool,
    modes: Vec<Mode>,
    serial: u32,
    cleanup: Vec<u8>,
    claims: Vec<String>,
    resizes: Vec<Rc<RefCell<Resizes>>>,
    /// Однопоточный исполнитель чтения.
    reader: Arc<Mutex<()>>,
    /// The waiting read and its limit.
    reading: Option<(sched::Future, usize)>,
    /// Input read by a cancelled read that nobody has taken yet.
    pending: Option<Vec<u8>>,
    /// The reader thread in flight, if any (#12): a cancelled read leaves it
    /// waiting, and the next read takes it over instead of queueing another
    /// one behind it.
    in_flight: Option<sched::Op>,
    /// Whether a read is waiting. The reader thread reads only then: input
    /// that comes after a read was cancelled stays in the terminal (#1013).
    wanted: Arc<AtomicBool>,
    /// Set when the session closes; a reader thread that starts later
    /// leaves without touching the descriptors.
    stopped: Arc<AtomicBool>,
    /// The pipe that stops the reader thread when the session closes
    /// (`[read end, write end]`, `-1` until the first read).
    stop: [i32; 2],
}

type SessionRef = Rc<RefCell<Session>>;

struct Lease {
    session: SessionRef,
    id: u32,
    dropped: RefCell<bool>,
}

struct Resizes {
    session: Option<SessionRef>,
    closed: bool,
    notified: bool,
    seen: u64,
    pending: Option<sched::Future>,
}

/// A copy of `fd` that children do not inherit. `F_DUPFD_CLOEXEC` sets the
/// flag with the copy, so a `fork` on another thread cannot come between
/// them as it could between `dup` and `fcntl(FD_CLOEXEC)` (#1013).
fn dup(fd: i32, operation: &str) -> Result<i32, Rc<Failure>> {
    // SAFETY: fcntl(2) on a descriptor number; a bad one is refused by errno.
    let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if copy < 0 {
        return Err(errno_failure(last_errno(), operation, "Cannot duplicate terminal descriptor"));
    }
    Ok(copy)
}

fn tty(fd: i32) -> bool {
    // SAFETY: isatty(3).
    unsafe { libc::isatty(fd) == 1 }
}

fn snapshot(session: &Session, operation: &str) -> Result<libc::termios, Rc<Failure>> {
    // SAFETY: an all-zero termios is a valid value of this plain C struct.
    let mut term: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: tcgetattr(3) fills the termios it is given.
    if unsafe { libc::tcgetattr(session.input, &mut term) } < 0 {
        return Err(errno_failure(last_errno(), operation, "Cannot observe terminal mode"));
    }
    Ok(term)
}

fn apply(session: &Session, snapshot: &libc::termios, operation: &str) -> Result<(), Rc<Failure>> {
    // SAFETY: tcsetattr(3) reads the termios it is given.
    if unsafe { libc::tcsetattr(session.input, libc::TCSANOW, snapshot) } < 0 {
        return Err(errno_failure(last_errno(), operation, "Cannot apply terminal mode"));
    }
    Ok(())
}

fn close_fds(session: &Session) {
    for fd in [session.input, session.output] {
        if fd >= 0 {
            // SAFETY: собственный дескриптор сессии.
            unsafe { libc::close(fd) };
        }
    }
}

fn opened(mut session: Session, keys: &[String], operation: &str) -> Result<SessionRef, Rc<Failure>> {
    {
        let mut owners = OWNERS.lock().expect("owners lock");
        let owners = owners.get_or_insert_with(HashSet::new);
        if keys.iter().any(|key| owners.contains(key)) {
            close_fds(&session);
            return Err(terminal_failure(
                "resource_exhausted",
                operation,
                "Terminal descriptor already belongs to an open session",
            ));
        }
        for key in keys {
            owners.insert(key.clone());
            session.claims.push(key.clone());
        }
    }
    session.input_interactive = tty(session.input);
    session.output_interactive = tty(session.output);
    if session.input_interactive {
        match snapshot(&session, operation) {
            Ok(original) => session.original = Some(original),
            Err(failure) => {
                // The claims and descriptors go with the session that failed
                // to open; they stayed forever before (#1013).
                release_claims(&session.claims);
                close_fds(&session);
                return Err(failure);
            }
        }
    }
    Ok(Rc::new(RefCell::new(session)))
}

fn release_claims(claims: &[String]) {
    let mut owners = OWNERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(owners) = owners.as_mut() {
        for claim in claims {
            owners.remove(claim);
        }
    }
}

fn new_session(input: i32, output: i32, controlling: bool) -> Session {
    Session {
        input,
        output,
        controlling,
        input_interactive: false,
        output_interactive: false,
        original: None,
        closed: false,
        modes: Vec::new(),
        serial: 0,
        cleanup: Vec::new(),
        claims: Vec::new(),
        resizes: Vec::new(),
        reader: Arc::new(Mutex::new(())),
        reading: None,
        pending: None,
        in_flight: None,
        wanted: Arc::new(AtomicBool::new(false)),
        stopped: Arc::new(AtomicBool::new(false)),
        stop: [-1, -1],
    }
}

fn session(value: &Value, operation: &str) -> Result<SessionRef, Rc<Failure>> {
    let state = handle::<SessionRef>(value)
        .map_err(|_| terminal_failure("invalid_input", operation, "Invalid terminal session"))?
        .clone();
    if state.borrow().closed {
        return Err(terminal_failure("already_closed", operation, "Terminal session is closed"));
    }
    Ok(state)
}

/// [столбцы, строки] вывода; не терминал — нули.
fn size(session: &Session, operation: &str) -> Result<(i64, i64), Rc<Failure>> {
    if !session.output_interactive {
        return Ok((0, 0));
    }
    let mut winsize = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ пишет struct winsize.
    if unsafe { libc::ioctl(session.output, libc::TIOCGWINSZ, &mut winsize) } < 0 {
        return Err(errno_failure(last_errno(), operation, "Cannot observe terminal geometry"));
    }
    Ok((i64::from(winsize.ws_col), i64::from(winsize.ws_row)))
}

/// The columns of the terminal on `fd` (`TIOCGWINSZ`); `None` when `fd` is
/// not a terminal or reports no width. For `okkama man`, which wraps its page
/// to the window (#1017).
pub fn terminal_columns(fd: i32) -> Option<u16> {
    if !tty(fd) {
        return None;
    }
    let mut winsize = libc::winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: TIOCGWINSZ writes a struct winsize.
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut winsize) } < 0 {
        return None;
    }
    (winsize.ws_col > 0).then_some(winsize.ws_col)
}

fn write_fd(fd: i32, data: &[u8], operation: &str) -> Result<usize, Rc<Failure>> {
    loop {
        // SAFETY: буфер `data`.
        let count = unsafe { libc::write(fd, data.as_ptr() as *const libc::c_void, data.len()) };
        if count >= 0 {
            return Ok(count as usize);
        }
        let code = last_errno();
        if code != libc::EINTR {
            return Err(errno_failure(code, operation, "Cannot write terminal output"));
        }
    }
}

fn close(state: &SessionRef) -> Result<(), Rc<Failure>> {
    if state.borrow().closed {
        return Ok(());
    }
    state.borrow_mut().closed = true;
    let resizes = state.borrow().resizes.clone();
    for source in resizes {
        close_resizes(&source);
    }
    let operation = symbol("terminal_close", 1);
    let reading = state.borrow().reading.clone();
    if let Some((reading, _)) = reading {
        sched::settle(&reading, Err(terminal_failure("already_closed", &operation, "Terminal session is closed")));
    }
    stop_reader(state);
    let mut first = None;
    let mut session = state.borrow_mut();
    if !session.cleanup.is_empty() && session.output >= 0 {
        match write_fd(session.output, &session.cleanup, &operation) {
            Ok(count) if count != session.cleanup.len() => {
                first = Some(terminal_failure("unexpected", &operation, "Terminal screen restoration was incomplete"));
            }
            Ok(_) => {}
            Err(failure) => first = Some(failure),
        }
    }
    if let Some(original) = session.original
        && let Err(failure) = apply(&session, &original, &operation) {
            first = first.or(Some(failure));
        }
    session.modes.clear();
    close_fds(&session);
    for fd in std::mem::replace(&mut session.stop, [-1, -1]) {
        if fd >= 0 {
            // SAFETY: the session's own stop pipe; its reader thread has left.
            unsafe { libc::close(fd) };
        }
    }
    release_claims(&session.claims);
    session.claims.clear();
    match first {
        Some(failure) => Err(failure),
        None => Ok(()),
    }
}

fn terminal_read(value: &Value, limit: &crate::IntValue) -> Value {
    let operation = symbol("terminal_read", 2);
    let state = match session(value, &operation) {
        Ok(state) => state,
        Err(failure) => return resolved(Err(failure)),
    };
    let Some(limit) = limit.to_i64().filter(|limit| (1..=65536).contains(limit)) else {
        return resolved(Err(terminal_failure("invalid_input", &operation, "Terminal read limit must be 1..65536")));
    };
    let limit = limit as usize;
    if state.borrow().reading.is_some() {
        return resolved(Err(terminal_failure("resource_exhausted", &operation, "Terminal already has a pending read")));
    }
    if let Some(mut pending) = state.borrow_mut().pending.take() {
        let count = limit.min(pending.len());
        let rest = pending.split_off(count);
        if !rest.is_empty() {
            state.borrow_mut().pending = Some(rest);
        }
        return resolved(Ok(byte_array(pending)));
    }
    let promise = sched::promise();
    state.borrow_mut().reading = Some((promise.clone(), limit));
    state.borrow().wanted.store(true, Ordering::SeqCst);
    let in_flight = state.borrow().in_flight.clone();
    match in_flight {
        // A cancelled read's thread still waits for input: its bytes come to
        // this read (`deliver_input`), and its operation is awaited again.
        Some(op) => op.attach(),
        None => start_read(&state, operation),
    }
    let weak = Rc::downgrade(&state);
    promise.listen(move || {
        if let Some(state) = weak.upgrade() {
            let mut session = state.borrow_mut();
            session.wanted.store(false, Ordering::SeqCst);
            if let Some(op) = &session.in_flight {
                op.detach();
            }
            session.reading = None;
        }
    });
    Value::Future(promise)
}

/// How the reader thread ended.
enum ReadEnd {
    /// `read(2)` returned these bytes (empty at the end of input).
    Data(Vec<u8>),
    /// `read(2)` failed with this errno.
    Failed(i32),
    /// Input came, but no read was waiting any more: it stays unread.
    Idle,
    /// The session closed.
    Stopped,
}

/// The stop pipe of the session, made on the first read; `None` when it
/// cannot be made, and the reader then blocks in `read(2)` as before.
fn stop_pipe(session: &mut Session) -> Option<[i32; 2]> {
    if session.stop[0] < 0 {
        let mut ends = [-1i32; 2];
        // SAFETY: pipe(2) fills two descriptors.
        if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
            return None;
        }
        for fd in ends {
            // SAFETY: the pipe's own descriptors: not inherited by children.
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        session.stop = ends;
    }
    Some(session.stop)
}

/// Whether `select(2)` can watch both descriptors.
fn selectable(input: i32, stop: i32) -> bool {
    (0..libc::FD_SETSIZE as i32).contains(&input) && (0..libc::FD_SETSIZE as i32).contains(&stop)
}

/// Wait until `input` is readable or `stop` is: `Ok(true)` for input,
/// `Ok(false)` for stop. `select(2)`, not `poll(2)`: macOS `poll` answers
/// `POLLNVAL` for `/dev/tty`.
fn wait_input(input: i32, stop: i32) -> Result<bool, i32> {
    loop {
        // SAFETY: an all-zero fd_set is a valid, empty set.
        let mut set: libc::fd_set = unsafe { std::mem::zeroed() };
        // SAFETY: both descriptors are below FD_SETSIZE (`selectable`).
        unsafe {
            libc::FD_SET(input, &mut set);
            libc::FD_SET(stop, &mut set);
        }
        // SAFETY: `set` is a valid fd_set, no timeout.
        let count = unsafe {
            libc::select(input.max(stop) + 1, &mut set, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
        };
        if count < 0 {
            let code = last_errno();
            if code == libc::EINTR {
                continue;
            }
            return Err(code);
        }
        // SAFETY: `set` was filled by select(2).
        if unsafe { libc::FD_ISSET(stop, &set) } {
            return Ok(false);
        }
        // SAFETY: as above.
        if unsafe { libc::FD_ISSET(input, &set) } {
            return Ok(true);
        }
    }
}

/// Stop the reader thread of a closing session and wait until it has left,
/// so that neither `close(2)` of its descriptor nor `tcsetattr(3)` waits for
/// input, and no input is read for a session that is gone (#1013). A reader
/// that blocks in `read(2)` (no stop pipe) is not waited for.
fn stop_reader(state: &SessionRef) {
    let (stop, input, gate) = {
        let session = state.borrow();
        session.wanted.store(false, Ordering::SeqCst);
        session.stopped.store(true, Ordering::SeqCst);
        if session.in_flight.is_none() {
            return;
        }
        (session.stop, session.input, session.reader.clone())
    };
    if !selectable(input, stop[0]) || stop[1] < 0 {
        return;
    }
    let byte = 1u8;
    // SAFETY: the stop pipe's own write end and a one-byte buffer.
    unsafe { libc::write(stop[1], &byte as *const u8 as *const libc::c_void, 1) };
    // The reader holds the gate while it waits and reads; it leaves at once
    // on the stop byte. A `read(2)` whose input another reader of the same
    // terminal took first could still block, so the wait is bounded, and
    // then the session closes under it as it did before.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while matches!(gate.try_lock(), Err(std::sync::TryLockError::WouldBlock)) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// The reader thread: it waits until input comes or the session closes, and
/// then makes one `read(2)` if a read still waits. Its outcome goes to
/// whichever read is waiting when it completes, which is not necessarily the
/// one that started it: that one may have been cancelled meanwhile (#12).
fn start_read(state: &SessionRef, operation: String) {
    let limit = 65536;
    let (fd, gate, wanted, stopped, stop) = {
        let mut session = state.borrow_mut();
        let stop = stop_pipe(&mut session).map_or(-1, |ends| ends[0]);
        (session.input, session.reader.clone(), session.wanted.clone(), session.stopped.clone(), stop)
    };
    let delivered = state.clone();
    let op = sched::offload(
        move || {
            let _gate = gate.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if stopped.load(Ordering::SeqCst) {
                return ReadEnd::Stopped;
            }
            if selectable(fd, stop) {
                match wait_input(fd, stop) {
                    Ok(true) => {}
                    Ok(false) => return ReadEnd::Stopped,
                    Err(code) => return ReadEnd::Failed(code),
                }
                if !wanted.load(Ordering::SeqCst) {
                    return ReadEnd::Idle;
                }
            }
            let mut buffer = vec![0u8; limit];
            loop {
                // SAFETY: буфер длины `limit`.
                let count = unsafe { libc::read(fd, buffer.as_mut_ptr() as *mut libc::c_void, limit) };
                if count >= 0 {
                    buffer.truncate(count as usize);
                    return ReadEnd::Data(buffer);
                }
                let code = last_errno();
                if code != libc::EINTR {
                    return ReadEnd::Failed(code);
                }
            }
        },
        move |end: ReadEnd| {
            delivered.borrow_mut().in_flight = None;
            let outcome = match end {
                ReadEnd::Data(data) => Ok(data),
                ReadEnd::Failed(code) => Err(code),
                ReadEnd::Stopped => return,
                ReadEnd::Idle => {
                    // A read that came after the thread looked is still waiting.
                    let waiting = delivered.borrow().reading.as_ref().is_some_and(|(read, _)| read.running());
                    if waiting && !delivered.borrow().closed {
                        start_read(&delivered, operation);
                    }
                    return;
                }
            };
            deliver_input(&delivered, outcome, &operation);
        },
    );
    state.borrow_mut().in_flight = Some(op);
}

/// The outcome of a `read(2)`: to the waiting read, at most its limit, the
/// rest kept for the next one; with nobody waiting, kept whole.
fn deliver_input(state: &SessionRef, outcome: Result<Vec<u8>, i32>, operation: &str) {
    let waiting = state.borrow().reading.clone().filter(|(read, _)| read.state() == sched::State::Running);
    let Some((read, limit)) = waiting else {
        if let Ok(data) = outcome
            && !data.is_empty() {
                state.borrow_mut().pending.get_or_insert_with(Vec::new).extend(data);
            }
        return;
    };
    let result = match outcome {
        Ok(mut data) => {
            if data.len() > limit {
                let rest = data.split_off(limit);
                state.borrow_mut().pending.get_or_insert_with(Vec::new).extend(rest);
            }
            Ok(byte_array(data))
        }
        Err(_) if state.borrow().closed => Err(terminal_failure("already_closed", operation, "Terminal session is closed")),
        Err(code) => Err(errno_failure(code, operation, "Cannot read terminal input")),
    };
    sched::settle(&read, result);
}


// ---- режимы ----

/// The terminal settings of `mode` (0 cooked, 1 cbreak, 2 raw, 3 secret),
/// made from the current ones (`before`); cooked and secret start from the
/// settings the session found. The fields and flags are libc's, not byte
/// offsets of one platform's layout (#1013).
fn mode_termios(mode: i64, before: libc::termios, original: Option<libc::termios>) -> libc::termios {
    let mut term = if mode == 0 || mode == 3 { original.unwrap_or(before) } else { before };
    if mode == 0 || mode == 3 {
        term.c_lflag |= libc::ICANON | libc::ISIG;
        term.c_iflag = (term.c_iflag & !(libc::IGNCR | libc::INLCR)) | libc::ICRNL;
        term.c_oflag |= libc::OPOST;
        if mode == 0 {
            term.c_lflag |= libc::ECHO;
        } else {
            term.c_lflag &= !(libc::ECHO | libc::ECHONL);
        }
    } else {
        term.c_lflag &= !(libc::ICANON | libc::ECHO);
        if mode == 1 {
            term.c_lflag |= libc::ISIG;
        }
        term.c_cc[libc::VMIN] = 1;
        term.c_cc[libc::VTIME] = 0;
        if mode == 2 {
            term.c_lflag &= !(libc::ISIG | libc::IEXTEN);
            term.c_iflag &= !(libc::IGNBRK
                | libc::BRKINT
                | libc::PARMRK
                | libc::INLCR
                | libc::IGNCR
                | libc::ICRNL
                | libc::INPCK
                | libc::ISTRIP
                | libc::IXON);
            term.c_oflag &= !libc::OPOST;
            term.c_cflag = (term.c_cflag & !(libc::CSIZE | libc::PARENB)) | libc::CS8;
        }
    }
    term
}

fn mode_open(value: &Value, mode: &crate::IntValue) -> Result<Value, Error> {
    let operation = symbol("terminal_mode_open", 2);
    let state = session(value, &operation).map_err(as_error)?;
    let attempt = || -> Result<Value, Rc<Failure>> {
        let mut session = state.borrow_mut();
        if !session.input_interactive {
            return Err(terminal_failure("unsupported", &operation, "Terminal modes require interactive input"));
        }
        let Some(mode) = mode.to_i64().filter(|mode| (0..=3).contains(mode)) else {
            return Err(terminal_failure("invalid_input", &operation, "Unknown terminal mode"));
        };
        let before = snapshot(&session, &operation)?;
        let term = mode_termios(mode, before, session.original);
        apply(&session, &term, &operation)?;
        session.serial += 1;
        let id = session.serial;
        session.modes.push(Mode { id, before, released: false });
        Ok(new_handle(Lease {
            session: state.clone(),
            id,
            dropped: RefCell::new(false),
        }))
    };
    attempt().map_err(as_error)
}

fn mode_closed(session: &Session, id: u32) -> bool {
    !session.modes.iter().any(|mode| mode.id == id && !mode.released)
}

fn mode_close(state: &SessionRef, id: u32, dropping: bool, operation: &str) -> Result<(), Rc<Failure>> {
    let mut session = state.borrow_mut();
    if session.closed || mode_closed(&session, id) {
        return Ok(());
    }
    if !dropping && session.modes.last().is_some_and(|mode| mode.id != id) {
        return Err(terminal_failure("invalid_input", operation, "Terminal mode leases must close in reverse order"));
    }
    for mode in session.modes.iter_mut().filter(|mode| mode.id == id) {
        mode.released = true;
    }
    while session.modes.last().is_some_and(|mode| mode.released) {
        let before = session.modes.last().expect("mode").before;
        apply(&session, &before, operation)?;
        session.modes.pop();
    }
    Ok(())
}

// ---- SIGWINCH ----

static PIPE_WRITE: AtomicI32 = AtomicI32::new(-1);
static GENERATION: AtomicU64 = AtomicU64::new(0);

struct Signals {
    generation: Mutex<u64>,
    changed: Condvar,
}

/// Resize waits in progress (a thread each), for the test of their exit.
static RESIZE_WAITS: AtomicUsize = AtomicUsize::new(0);

/// Wait on a thread until the generation passes `seen` (true) or `stop` is
/// set (false). A cancelled observation sets `stop` and wakes the waiters
/// ([`wake_resize_waits`]), so its thread does not sleep until the next
/// `SIGWINCH` (#1013, as #940 did for `signal_arrival`).
fn wait_resize(seen: u64, stop: &AtomicBool) -> bool {
    RESIZE_WAITS.fetch_add(1, Ordering::SeqCst);
    let signals = signals();
    let mut generation = signals.generation.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    while *generation <= seen && !stop.load(Ordering::SeqCst) {
        generation = signals.changed.wait(generation).unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    let resized = *generation > seen;
    drop(generation);
    RESIZE_WAITS.fetch_sub(1, Ordering::SeqCst);
    resized
}

/// Wake the resize waits after a `stop` was set. Taking the lock first means
/// a wait between its check and its sleep cannot miss the wakeup.
fn wake_resize_waits() {
    let signals = signals();
    drop(signals.generation.lock());
    signals.changed.notify_all();
}

fn signals() -> &'static Signals {
    static SIGNALS: std::sync::OnceLock<Signals> = std::sync::OnceLock::new();
    SIGNALS.get_or_init(|| Signals {
        generation: Mutex::new(0),
        changed: Condvar::new(),
    })
}

static PRIOR: std::sync::OnceLock<libc::sigaction> = std::sync::OnceLock::new();

/// The `SIGWINCH` handler, installed with `SA_SIGINFO` so that it has the
/// arguments a three-argument prior handler needs (#1013: the prior handler
/// was always called with one).
extern "C" fn on_winch(signal: libc::c_int, info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    let fd = PIPE_WRITE.load(Ordering::SeqCst);
    if fd >= 0 {
        let byte = 1u8;
        // SAFETY: write(2) is async-signal-safe; the write end does not block
        // (a full pipe drops the byte, the generation still counts); errno is
        // the interrupted code's and is put back.
        unsafe {
            let saved = *super::signal::errno();
            libc::write(fd, &byte as *const u8 as *const libc::c_void, 1);
            *super::signal::errno() = saved;
        }
    }
    if let Some(prior) = PRIOR.get() {
        let handler = prior.sa_sigaction;
        if handler == libc::SIG_DFL || handler == libc::SIG_IGN || handler == on_winch as *const () as usize {
            return;
        }
        if prior.sa_flags & libc::SA_SIGINFO != 0 {
            // SAFETY: with SA_SIGINFO the prior handler has the sa_sigaction form.
            let chained: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) =
                unsafe { std::mem::transmute(handler) };
            chained(signal, info, context);
        } else {
            // SAFETY: without SA_SIGINFO the prior handler has the sa_handler form.
            let chained: extern "C" fn(libc::c_int) = unsafe { std::mem::transmute(handler) };
            chained(signal);
        }
    }
}

/// Install the `SIGWINCH` handler and its thread once; false when that
/// failed, and resize sources are refused instead of waiting forever (#1013:
/// a failed `pipe` used up the `Once`, and every wait hung).
fn install() -> bool {
    static INSTALLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *INSTALLED.get_or_init(|| {
        let mut fds = [0i32; 2];
        // SAFETY: pipe(2) fills two descriptors.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return false;
        }
        // SAFETY: the pipe's own descriptors: not inherited by children, and
        // the write end does not block the handler.
        unsafe {
            libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC);
            let flags = libc::fcntl(fds[1], libc::F_GETFL);
            libc::fcntl(fds[1], libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        let read = fds[0];
        let spawned = std::thread::Builder::new().name("okkama-winch".into()).spawn(move || {
            let mut byte = 0u8;
            loop {
                // SAFETY: the pipe's own read end and a one-byte buffer.
                let count = unsafe { libc::read(read, &mut byte as *mut u8 as *mut libc::c_void, 1) };
                if count == 0 || (count < 0 && last_errno() != libc::EINTR) {
                    return;
                }
                let signals = signals();
                if let Ok(mut generation) = signals.generation.lock() {
                    *generation = GENERATION.load(Ordering::SeqCst);
                }
                signals.changed.notify_all();
            }
        });
        if spawned.is_err() {
            // SAFETY: the pipe's own descriptors, not given to anybody.
            unsafe {
                libc::close(fds[0]);
                libc::close(fds[1]);
            }
            return false;
        }
        PIPE_WRITE.store(fds[1], Ordering::SeqCst);
        // SAFETY: sigaction(2) with a handler of the SA_SIGINFO form; the prior
        // action is kept to chain to.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_winch as *const () as usize;
            action.sa_flags = libc::SA_RESTART | libc::SA_SIGINFO;
            libc::sigemptyset(&mut action.sa_mask);
            let mut prior: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(libc::SIGWINCH, &action, &mut prior) != 0 {
                return false;
            }
            let _ = PRIOR.set(prior);
        }
        true
    })
}

fn current_generation() -> u64 {
    signals().generation.lock().map(|generation| *generation).unwrap_or(0)
}

fn deliver(source: &Rc<RefCell<Resizes>>) {
    let operation = symbol("terminal_resize_next", 1);
    let (promise, session) = {
        let mut borrowed = source.borrow_mut();
        borrowed.notified = false;
        borrowed.seen = current_generation();
        (borrowed.pending.clone(), borrowed.session.clone())
    };
    let Some(promise) = promise else {
        return;
    };
    let outcome = match session {
        Some(session) => size(&session.borrow(), &operation).map(|(columns, rows)| array(vec![int_value(columns), int_value(rows)])),
        None => Ok(array(vec![int_value(0), int_value(0)])),
    };
    sched::settle(&promise, outcome);
}

fn close_resizes(source: &Rc<RefCell<Resizes>>) {
    let (pending, session) = {
        let mut borrowed = source.borrow_mut();
        if borrowed.closed {
            return;
        }
        borrowed.closed = true;
        (borrowed.pending.clone(), borrowed.session.clone())
    };
    if let Some(session) = session
        && let Ok(mut session) = session.try_borrow_mut() {
            session.resizes.retain(|known| !Rc::ptr_eq(known, source));
        }
    if let Some(pending) = pending {
        sched::settle(
            &pending,
            Err(terminal_failure("already_closed", &symbol("terminal_resize_next", 1), "Terminal resize source is closed")),
        );
    }
}

fn resize_next(value: &Value) -> Result<Value, Error> {
    let operation = symbol("terminal_resize_next", 1);
    let source = handle::<Rc<RefCell<Resizes>>>(value)?.clone();
    if source.borrow().closed {
        return Ok(resolved(Err(terminal_failure("already_closed", &operation, "Terminal resize source is closed"))));
    }
    if source.borrow().pending.is_some() {
        return Ok(resolved(Err(terminal_failure(
            "resource_exhausted",
            &operation,
            "Resize source already has a pending observation",
        ))));
    }
    let promise = sched::promise();
    source.borrow_mut().pending = Some(promise.clone());
    let seen = source.borrow().seen;
    if source.borrow().notified || current_generation() > seen {
        let weak = Rc::downgrade(&source);
        promise.listen(move || {
            if let Some(source) = weak.upgrade() {
                source.borrow_mut().pending = None;
            }
        });
        deliver(&source);
        return Ok(Value::Future(promise));
    }
    // Ожидание сигнала держит программу до завершения операции.
    let waiting = source.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let op = sched::offload(
        move || wait_resize(seen, &stopping),
        move |resized: bool| {
            if !resized {
                return;
            }
            let pending = waiting.borrow().pending.clone();
            if pending.is_some_and(|pending| pending.state() == sched::State::Running) && !waiting.borrow().closed {
                deliver(&waiting);
            } else {
                waiting.borrow_mut().notified = true;
            }
        },
    );
    let weak = Rc::downgrade(&source);
    promise.listen(move || {
        op.detach();
        stop.store(true, Ordering::SeqCst);
        wake_resize_waits();
        if let Some(source) = weak.upgrade() {
            source.borrow_mut().pending = None;
        }
    });
    Ok(Value::Future(promise))
}

// ---- учётные записи ----

const ACCOUNT: &str = "core.account";

fn c_text(pointer: *const c_char) -> Option<String> {
    // SAFETY: строка с завершающим нулём или NULL.
    (!pointer.is_null()).then(|| unsafe { CStr::from_ptr(pointer) }.to_string_lossy().into_owned())
}

fn user(entry: *const libc::passwd, operation: &str) -> Result<Value, Error> {
    if entry.is_null() {
        return Err(as_error(fail(ACCOUNT, "not_found", operation, "User was not found")));
    }
    // SAFETY: запись getpw* действительна до следующего вызова.
    let entry = unsafe { &*entry };
    let Some(name) = c_text(entry.pw_name).filter(|name| !name.is_empty()) else {
        return Err(as_error(fail(ACCOUNT, "invalid_data", operation, "Account provider returned an invalid user")));
    };
    Ok(map([
        ("home".to_string(), text(c_text(entry.pw_dir).unwrap_or_default())),
        ("identity".to_string(), text(entry.pw_uid.to_string())),
        ("name".to_string(), text(name)),
    ]))
}

fn account_name(value: &str, kind: &str, operation: &str) -> Result<CString, Error> {
    if value.is_empty() {
        return Err(as_error(fail(ACCOUNT, "invalid_input", operation, &format!("{kind} name must not be empty"))));
    }
    CString::new(value).map_err(|_| as_error(fail(ACCOUNT, "invalid_input", operation, &format!("{kind} name contains a zero byte"))))
}

/// The non-empty logins of a NULL-terminated `gr_mem` list. macOS libc packs
/// the list right after the record's strings without aligning it (#1013), so
/// each pointer is read unaligned.
///
/// # Safety
///
/// `list` is NULL or points to a NULL-terminated sequence of pointers, each
/// to a NUL-terminated string, all valid for the duration of the call.
unsafe fn group_members(list: *const *mut c_char) -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    if list.is_null() {
        return names;
    }
    let mut member = list;
    loop {
        // SAFETY: by the contract, `member` is inside the list, up to its NULL
        // end; `read_unaligned` needs no alignment.
        let login = unsafe { member.read_unaligned() };
        if login.is_null() {
            return names;
        }
        if let Some(login) = c_text(login).filter(|login| !login.is_empty()) {
            names.insert(login);
        }
        // SAFETY: the list goes on at least until its NULL end.
        member = unsafe { member.add(1) };
    }
}

fn group(name: &CString, operation: &str) -> Result<*const libc::group, Error> {
    // SAFETY: getgrnam(3).
    let entry = unsafe { libc::getgrnam(name.as_ptr()) };
    if entry.is_null() {
        return Err(as_error(fail(ACCOUNT, "not_found", operation, "Group was not found")));
    }
    Ok(entry)
}

fn account(native: Native, name: &str, args: &[Value]) -> Result<Value, Error> {
    let operation = native.symbol();
    match name {
        // SAFETY: getpwuid(3) текущего пользователя.
        "account_current_user" => user(unsafe { libc::getpwuid(libc::geteuid()) }, operation),
        "account_user_named" => {
            let name = account_name(string(&args[0])?, "User", operation)?;
            // SAFETY: getpwnam(3).
            user(unsafe { libc::getpwnam(name.as_ptr()) }, operation)
        }
        "account_group_named" => {
            let name = account_name(string(&args[0])?, "Group", operation)?;
            let entry = group(&name, operation)?;
            // SAFETY: запись getgrnam действительна до следующего вызова.
            let entry = unsafe { &*entry };
            let Some(found) = c_text(entry.gr_name).filter(|found| !found.is_empty()) else {
                return Err(as_error(fail(ACCOUNT, "invalid_data", operation, "Account provider returned an invalid group")));
            };
            Ok(map([
                ("identity".to_string(), text(entry.gr_gid.to_string())),
                ("name".to_string(), text(found)),
            ]))
        }
        "account_group_users" => {
            let name = account_name(string(&args[0])?, "Group", operation)?;
            let entry = group(&name, operation)?;
            // SAFETY: the getgrnam record stays valid until the next call; its
            // member list ends with NULL.
            let (gid, mut names) = unsafe {
                let entry = &*entry;
                (entry.gr_gid, group_members(entry.gr_mem))
            };
            // SAFETY: перебор базы пользователей.
            unsafe {
                libc::setpwent();
                loop {
                    let user = libc::getpwent();
                    if user.is_null() {
                        break;
                    }
                    if let Some(login) = c_text((*user).pw_name)
                        && (*user).pw_gid == gid {
                            names.insert(login);
                        }
                }
                libc::endpwent();
            }
            Ok(string_array(std::mem::take(&mut names)))
        }
        _ => Err(Error::Unsupported(format!("native provider is not implemented: {}", native.symbol()))),
    }
}

pub fn execute(native: Native, args: &[Value]) -> Result<Value, Error> {
    let name = native.name();
    if native.symbol().starts_with("runtime/account@") {
        return account(native, name, args);
    }
    let operation = native.symbol();
    match name {
        "terminal_standard" => {
            let attempt = || -> Result<Value, Rc<Failure>> {
                let kind = int(&args[0]).ok().and_then(|kind| kind.to_i64()).filter(|kind| *kind == 0 || *kind == 1);
                let Some(kind) = kind else {
                    return Err(terminal_failure("invalid_input", operation, "Terminal output must be stdout (0) or stderr (1)"));
                };
                use std::io::Write;
                let _ = std::io::stdout().flush();
                let _ = std::io::stderr().flush();
                let input = dup(0, operation)?;
                let output = match dup(if kind == 0 { 1 } else { 2 }, operation) {
                    Ok(output) => output,
                    Err(failure) => {
                        // SAFETY: the copy made just above, owned by nobody else.
                        unsafe { libc::close(input) };
                        return Err(failure);
                    }
                };
                let session = new_session(input, output, false);
                Ok(new_handle(opened(session, &["fd:0".to_string(), format!("fd:{}", kind + 1)], operation)?))
            };
            Ok(resolved(attempt()))
        }
        "terminal_controlling" => {
            let attempt = || -> Result<Value, Rc<Failure>> {
                let path = CString::new("/dev/tty").expect("no NUL");
                // SAFETY: open(2).
                let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
                if fd < 0 {
                    return Err(terminal_failure("not_found", operation, "No controlling terminal is available"));
                }
                let output = match dup(fd, operation) {
                    Ok(output) => output,
                    Err(failure) => {
                        // SAFETY: the descriptor opened just above, owned by nobody else.
                        unsafe { libc::close(fd) };
                        return Err(failure);
                    }
                };
                let session = new_session(fd, output, true);
                Ok(new_handle(opened(session, &["tty".to_string()], operation)?))
            };
            Ok(resolved(attempt()))
        }
        "terminal_metadata" => {
            let state = session(&args[0], operation).map_err(as_error)?;
            let session = state.borrow();
            let (columns, rows) = size(&session, operation).map_err(as_error)?;
            Ok(int_map([
                ("colors", i64::from(session.output_interactive)),
                ("columns", columns),
                ("controlling", i64::from(session.controlling)),
                ("input", i64::from(session.input_interactive)),
                ("output", i64::from(session.output_interactive)),
                ("rows", rows),
            ]))
        }
        "terminal_write" => {
            let state = session(&args[0], operation).map_err(as_error)?;
            let data = bytes(&args[1])?;
            if data.len() > 65536 {
                return Err(as_error(terminal_failure("invalid_input", operation, "Terminal write exceeds 64 KiB")));
            }
            if data.is_empty() {
                return Ok(int_value(0));
            }
            let output = state.borrow().output;
            // Buffered standard output goes first (#132).
            let _ = crate::stdout::flush();
            Ok(int_value(write_fd(output, &data, operation).map_err(as_error)? as i64))
        }
        "terminal_read" => Ok(terminal_read(&args[0], int(&args[1])?)),
        "terminal_cleanup" => {
            let state = session(&args[0], operation).map_err(as_error)?;
            if items(&args[1])?.len() > 256 {
                return Err(as_error(terminal_failure("invalid_input", operation, "Terminal cleanup exceeds 256 bytes")));
            }
            state.borrow_mut().cleanup = bytes(&args[1])?;
            Ok(Value::Bool(true))
        }
        "terminal_closed" => Ok(Value::Bool(handle::<SessionRef>(&args[0])?.borrow().closed)),
        "terminal_close" => {
            close(handle::<SessionRef>(&args[0])?).map_err(as_error)?;
            Ok(Value::Bool(true))
        }
        "terminal_mode_open" => mode_open(&args[0], int(&args[1])?),
        "terminal_mode_closed" => {
            let lease = handle::<Lease>(&args[0])?;
            Ok(Value::Bool(*lease.dropped.borrow() || mode_closed(&lease.session.borrow(), lease.id)))
        }
        "terminal_mode_close" | "terminal_mode_drop" => {
            let lease = handle::<Lease>(&args[0])?;
            if *lease.dropped.borrow() {
                return Ok(Value::Bool(true));
            }
            let dropping = name == "terminal_mode_drop";
            if dropping {
                *lease.dropped.borrow_mut() = true;
            }
            mode_close(&lease.session, lease.id, dropping, operation).map_err(as_error)?;
            *lease.dropped.borrow_mut() = true;
            Ok(Value::Bool(true))
        }
        "terminal_resize_open" => {
            let attempt = || -> Result<Value, Rc<Failure>> {
                let state = session(&args[0], operation)?;
                if !state.borrow().output_interactive {
                    return Err(terminal_failure("unsupported", operation, "Resize notifications require interactive output"));
                }
                if !install() {
                    return Err(terminal_failure("unsupported", operation, "Resize notifications cannot be installed"));
                }
                let source = Rc::new(RefCell::new(Resizes {
                    session: Some(state.clone()),
                    closed: false,
                    notified: true,
                    seen: current_generation(),
                    pending: None,
                }));
                state.borrow_mut().resizes.push(source.clone());
                Ok(new_handle(source))
            };
            Ok(resolved(attempt()))
        }
        "terminal_resize_next" => resize_next(&args[0]),
        "terminal_resize_closed" => Ok(Value::Bool(handle::<Rc<RefCell<Resizes>>>(&args[0])?.borrow().closed)),
        "terminal_resize_close" => {
            close_resizes(handle::<Rc<RefCell<Resizes>>>(&args[0])?);
            Ok(Value::Bool(true))
        }
        _ => Err(Error::Unsupported(format!("native provider is not implemented: {}", native.symbol()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1013: macOS libc packs `gr_mem` without alignment; the members are
    /// read unaligned (a plain dereference panics in a debug build).
    #[test]
    fn group_members_read_an_unaligned_list() {
        let (first, second, empty) = (c"daemon", c"root", c"");
        let pointers = [first.as_ptr(), empty.as_ptr(), second.as_ptr(), first.as_ptr(), std::ptr::null()];
        let width = std::mem::size_of::<*const c_char>();
        let mut storage = vec![0u8; 1 + pointers.len() * width];
        for (index, pointer) in pointers.iter().enumerate() {
            let at = 1 + index * width;
            storage[at..at + width].copy_from_slice(&(*pointer as usize).to_ne_bytes());
        }
        // SAFETY: the list is in `storage`, ends with NULL and points to live strings.
        let names = unsafe { group_members(storage[1..].as_ptr().cast()) };
        assert_eq!(names.into_iter().collect::<Vec<_>>(), ["daemon", "root"]);
        // SAFETY: a NULL list is allowed.
        assert!(unsafe { group_members(std::ptr::null()) }.is_empty());
    }

    /// The tests that wait for or raise SIGWINCH run one at a time.
    static WINCH: Mutex<()> = Mutex::new(());

    /// A pseudo-terminal: (controller, terminal) descriptors.
    fn pty() -> (i32, i32) {
        // SAFETY: posix_openpt(3) family; the name is copied at once.
        unsafe {
            let controller = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(controller >= 0, "posix_openpt");
            assert_eq!(libc::grantpt(controller), 0);
            assert_eq!(libc::unlockpt(controller), 0);
            let name = CStr::from_ptr(libc::ptsname(controller)).to_owned();
            let terminal = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
            assert!(terminal >= 0, "open the terminal side");
            (controller, terminal)
        }
    }

    /// A session over its own copies of the terminal descriptor.
    fn pty_session(terminal: i32) -> SessionRef {
        let (input, output) = (dup(terminal, "test").expect("dup"), dup(terminal, "test").expect("dup"));
        opened(new_session(input, output, false), &[format!("test:pty:{terminal}")], "test").expect("a session")
    }

    fn put(fd: i32, data: &[u8]) {
        // SAFETY: a live descriptor and a buffer of that length.
        assert_eq!(unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) }, data.len() as isize);
    }

    /// What can be read from `fd` within a second (empty when nothing comes).
    fn take(fd: i32) -> Vec<u8> {
        let mut ready = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd.
        if unsafe { libc::poll(&mut ready, 1, 1000) } != 1 {
            return Vec::new();
        }
        let mut buffer = vec![0u8; 256];
        // SAFETY: a buffer of that length.
        let count = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        buffer.truncate(count.max(0) as usize);
        buffer
    }

    /// Run the scheduler's deliveries for a while.
    fn settle_for(milliseconds: u64) {
        let until = std::time::Instant::now() + std::time::Duration::from_millis(milliseconds);
        while std::time::Instant::now() < until {
            sched::pump();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn close_all(fds: &[i32]) {
        for fd in fds {
            // SAFETY: the test's own descriptors.
            unsafe { libc::close(*fd) };
        }
    }

    /// #1013: closing a session whose read was cancelled neither waits for
    /// input nor lets the cancelled read take the next line.
    #[test]
    fn close_after_a_cancelled_read_neither_waits_nor_eats_input() {
        let (controller, terminal) = pty();
        let state = pty_session(terminal);
        let Value::Future(read) = terminal_read(&new_handle(state.clone()), &crate::IntValue::from_i64(64)) else {
            panic!("a read future")
        };
        std::thread::sleep(std::time::Duration::from_millis(20));
        sched::cancel(&read);
        let writer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            put(controller, b"first\n");
        });
        let started = std::time::Instant::now();
        close(&state).expect("closed");
        let took = started.elapsed();
        // The reader has left before its descriptor was closed.
        assert!(state.borrow().reader.try_lock().is_ok(), "the reader still runs");
        writer.join().expect("the writer");
        settle_for(100);
        assert_eq!(String::from_utf8_lossy(&take(terminal)), "first\n", "the line stays in the terminal");
        assert!(took < std::time::Duration::from_secs(1), "close waited {took:?}");
        close_all(&[controller, terminal]);
    }

    /// #1013: input that comes after a read was cancelled stays in the
    /// terminal, so a close does not lose it.
    #[test]
    fn input_after_a_cancelled_read_stays_in_the_terminal() {
        let (controller, terminal) = pty();
        let state = pty_session(terminal);
        let Value::Future(read) = terminal_read(&new_handle(state.clone()), &crate::IntValue::from_i64(64)) else {
            panic!("a read future")
        };
        std::thread::sleep(std::time::Duration::from_millis(20));
        sched::cancel(&read);
        put(controller, b"early\n");
        settle_for(200);
        assert!(state.borrow().in_flight.is_none(), "the reader left without reading");
        close(&state).expect("closed");
        assert_eq!(String::from_utf8_lossy(&take(terminal)), "early\n");
        close_all(&[controller, terminal]);
    }

    /// #12 stays: the read after a cancelled one gets the input, whether it
    /// came while the reader still waited or after it left.
    #[test]
    fn a_read_after_a_cancelled_read_gets_the_input() {
        let (controller, terminal) = pty();
        let state = pty_session(terminal);
        let handle = new_handle(state.clone());
        let limit = crate::IntValue::from_i64(64);
        let read_now = |expected: &str| {
            let Value::Future(read) = terminal_read(&handle, &limit) else { panic!("a read future") };
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while read.running() && std::time::Instant::now() < until {
                sched::pump();
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let Ok(data) = bytes(&read.value().expect("a value")) else { panic!("bytes") };
            assert_eq!(String::from_utf8_lossy(&data), expected);
        };
        // The reader is still waiting when the next read comes.
        let Value::Future(first) = terminal_read(&handle, &limit) else { panic!("a read future") };
        std::thread::sleep(std::time::Duration::from_millis(20));
        sched::cancel(&first);
        let writer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            put(controller, b"one\n");
        });
        read_now("one\n");
        writer.join().expect("the writer");
        // The input came after the cancel, before the next read.
        let Value::Future(second) = terminal_read(&handle, &limit) else { panic!("a read future") };
        std::thread::sleep(std::time::Duration::from_millis(20));
        sched::cancel(&second);
        put(controller, b"two\n");
        settle_for(200);
        read_now("two\n");
        close(&state).expect("closed");
        close_all(&[controller, terminal]);
    }

    /// #1013: a cancelled resize observation's thread leaves at once instead
    /// of sleeping until the next SIGWINCH.
    #[test]
    fn a_cancelled_resize_wait_leaves() {
        let _winch = WINCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let source = Rc::new(RefCell::new(Resizes {
            session: None,
            closed: false,
            notified: false,
            seen: current_generation(),
            pending: None,
        }));
        let Ok(Value::Future(next)) = resize_next(&new_handle(source.clone())) else { panic!("a resize future") };
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while RESIZE_WAITS.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(RESIZE_WAITS.load(Ordering::SeqCst), 1, "the wait started");
        sched::cancel(&next);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while RESIZE_WAITS.load(Ordering::SeqCst) != 0 && std::time::Instant::now() < until {
            sched::pump();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(RESIZE_WAITS.load(Ordering::SeqCst), 0, "the wait left");
        settle_for(20);
        assert!(!source.borrow().notified, "a cancel is not a resize");
    }

    /// #1013: the modes are made from libc's termios fields and flags.
    #[test]
    fn modes_set_the_termios_flags() {
        // SAFETY: an all-zero termios is a valid value.
        let mut cooked: libc::termios = unsafe { std::mem::zeroed() };
        cooked.c_lflag = libc::ICANON | libc::ISIG | libc::ECHO | libc::IEXTEN;
        cooked.c_iflag = libc::ICRNL | libc::IXON | libc::BRKINT;
        cooked.c_oflag = libc::OPOST;
        cooked.c_cflag = libc::CS7 | libc::PARENB;
        let raw = mode_termios(2, cooked, None);
        assert_eq!(raw.c_lflag & (libc::ICANON | libc::ISIG | libc::ECHO | libc::IEXTEN), 0);
        assert_eq!(raw.c_iflag & (libc::ICRNL | libc::IXON | libc::BRKINT), 0);
        assert_eq!(raw.c_oflag & libc::OPOST, 0);
        assert_eq!(raw.c_cflag & (libc::CSIZE | libc::PARENB), libc::CS8);
        assert_eq!((raw.c_cc[libc::VMIN], raw.c_cc[libc::VTIME]), (1, 0));
        let cbreak = mode_termios(1, cooked, None);
        assert_eq!(cbreak.c_lflag & (libc::ICANON | libc::ECHO), 0);
        assert_eq!(cbreak.c_lflag & libc::ISIG, libc::ISIG);
        assert_eq!(cbreak.c_iflag, cooked.c_iflag);
        let secret = mode_termios(3, raw, Some(cooked));
        assert_eq!(secret.c_lflag & (libc::ICANON | libc::ISIG | libc::ECHO), libc::ICANON | libc::ISIG);
        assert_eq!(secret.c_cflag, cooked.c_cflag, "from the settings the session found");
        let back = mode_termios(0, raw, None);
        assert_eq!(back.c_lflag & (libc::ICANON | libc::ISIG | libc::ECHO), libc::ICANON | libc::ISIG | libc::ECHO);
        assert_eq!(back.c_iflag & libc::ICRNL, libc::ICRNL);
        assert_eq!(back.c_oflag & libc::OPOST, libc::OPOST);
    }

    /// A raw mode reaches the terminal and its lease puts the mode back.
    #[test]
    fn a_mode_lease_applies_and_restores_the_terminal() {
        let (controller, terminal) = pty();
        let state = pty_session(terminal);
        let local = |fd: i32| {
            // SAFETY: an all-zero termios is valid; tcgetattr fills it.
            let mut term: libc::termios = unsafe { std::mem::zeroed() };
            assert_eq!(unsafe { libc::tcgetattr(fd, &mut term) }, 0);
            term.c_lflag & (libc::ICANON | libc::ECHO)
        };
        assert_eq!(local(terminal), libc::ICANON | libc::ECHO);
        let lease = mode_open(&new_handle(state.clone()), &crate::IntValue::from_i64(2)).expect("a lease");
        assert_eq!(local(terminal), 0);
        let lease = handle::<Lease>(&lease).expect("a lease");
        mode_close(&lease.session, lease.id, false, "test").expect("restored");
        assert_eq!(local(terminal), libc::ICANON | libc::ECHO);
        close(&state).expect("closed");
        close_all(&[controller, terminal]);
    }

    static PRIOR_SAW: AtomicI32 = AtomicI32::new(0);

    extern "C" fn prior_winch(signal: libc::c_int, info: *mut libc::siginfo_t, _context: *mut libc::c_void) {
        // SAFETY: the kernel's siginfo for this delivery, when there is one.
        let number = if info.is_null() { -1 } else { unsafe { (*info).si_signo } };
        PRIOR_SAW.store(if number == signal { signal } else { -2 }, Ordering::SeqCst);
    }

    /// #1013: a prior SIGWINCH handler of the SA_SIGINFO form is chained with
    /// its three arguments; the count of resizes still goes up.
    #[test]
    fn the_winch_handler_chains_a_siginfo_handler() {
        let _winch = WINCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: sigaction(2) with a handler of the SA_SIGINFO form.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = prior_winch as *const () as usize;
            action.sa_flags = libc::SA_SIGINFO;
            libc::sigemptyset(&mut action.sa_mask);
            assert_eq!(libc::sigaction(libc::SIGWINCH, &action, std::ptr::null_mut()), 0);
        }
        assert!(install(), "installed");
        let before = GENERATION.load(Ordering::SeqCst);
        // SAFETY: raise(3) delivers the signal to this thread.
        assert_eq!(unsafe { libc::raise(libc::SIGWINCH) }, 0);
        assert!(GENERATION.load(Ordering::SeqCst) > before);
        assert_eq!(PRIOR_SAW.load(Ordering::SeqCst), libc::SIGWINCH);
        // The winch thread has passed the change on before the next test waits.
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while current_generation() < GENERATION.load(Ordering::SeqCst) && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}
