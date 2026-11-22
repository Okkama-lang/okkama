//! One-shot and run-local reusable workers share Tokio process ownership and pipe I/O.
//! Cancellation kills and
//! reaps the child before the request becomes ready; dropping a VM waiter does
//! not abandon a running OS process.
use std::{
    cell::{Cell, RefCell},
    ffi::OsString,
    future::Future,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

const DEFAULT_OUTPUT_LIMIT: usize = 64 * 1024 * 1024;

fn worker_command(mut command: Vec<OsString>) -> Vec<OsString> {
    if !command.is_empty() {
        // CLI switches must precede FILE.spb; everything after it belongs to
        // the language program. Appending this flag would run main recursively.
        command.insert(1, "--worker".into());
    }
    command
}

// Workers own their ordinary descendants. Drop also covers an abandoned host
// future; no detached pipe-reading tasks outlive this guard.
#[derive(Debug)]
struct ProcessGroup {
    #[cfg(unix)]
    pid: Option<rustix::process::Pid>,
}
impl ProcessGroup {
    fn new(child: &tokio::process::Child) -> Self {
        Self {
            #[cfg(unix)]
            pid: child
                .id()
                .and_then(|id| rustix::process::Pid::from_raw(id as i32)),
        }
    }
    fn terminate(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid.take() {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.terminate();
    }
}

async fn read_limited(
    mut stream: impl AsyncRead + Unpin,
    remaining: &Cell<usize>,
) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Ok(bytes);
        }
        let Some(left) = remaining.get().checked_sub(count) else {
            return Err(std::io::Error::other("worker output exceeds byte limit"));
        };
        remaining.set(left);
        bytes.extend_from_slice(&buffer[..count]);
    }
}

#[derive(Debug)]
pub(crate) struct Output {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub success: bool,
    pub code: Option<i32>,
}
#[derive(Debug)]
pub(crate) enum Failure {
    Cancelled,
    Process(String),
}
type Outcome = Result<Rc<Output>, Rc<Failure>>;
type Running = Pin<Box<dyn Future<Output = Outcome>>>;
pub(crate) struct Request {
    body: RefCell<Option<Running>>,
    result: RefCell<Option<Outcome>>,
    cancelled: tokio::sync::watch::Sender<bool>,
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerRequest")
            .field("ready", &self.result.borrow().is_some())
            .finish()
    }
}
impl Request {
    pub fn new(
        command: Vec<OsString>,
        input: Vec<u8>,
        environment: Option<std::collections::BTreeMap<String, String>>,
    ) -> Rc<Self> {
        Self::from_command(worker_command(command), input, environment)
    }
    fn from_command(
        command: Vec<OsString>,
        input: Vec<u8>,
        environment: Option<std::collections::BTreeMap<String, String>>,
    ) -> Rc<Self> {
        let (cancelled, wait) = tokio::sync::watch::channel(false);
        Rc::new(Self {
            body: RefCell::new(Some(Box::pin(run(command, input, environment, wait)))),
            result: RefCell::new(None),
            cancelled,
        })
    }
    pub fn cancel(&self) {
        self.cancelled.send_replace(true);
    }
    pub fn poll(&self, cx: &mut Context<'_>) -> Poll<Outcome> {
        if let Some(result) = self.result.borrow().as_ref() {
            return Poll::Ready(result.clone());
        }
        let mut body = self.body.borrow_mut();
        let result = body.as_mut().expect("running worker").as_mut().poll(cx);
        if let Poll::Ready(value) = &result {
            *self.result.borrow_mut() = Some(value.clone());
            body.take();
        }
        result
    }
}
impl Drop for Request {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl Request {
    pub(crate) fn pooled(pool: Rc<Pool>, command: Vec<OsString>, input: Vec<u8>) -> Rc<Self> {
        #[cfg(not(unix))]
        {
            let _ = pool;
            Self::new(command, input, None)
        }
        #[cfg(unix)]
        {
            let (cancelled, wait) = tokio::sync::watch::channel(false);
            Rc::new(Self {
                body: RefCell::new(Some(Box::pin(pool.run(worker_command(command), input, wait)))),
                result: RefCell::new(None),
                cancelled,
            })
        }
    }
    pub(crate) fn closing(pool: Rc<Pool>) -> Rc<Self> {
        let (cancelled, _wait) = tokio::sync::watch::channel(false);
        Rc::new(Self {
            // Cleanup is shielded: cancellation must not interrupt reaping.
            body: RefCell::new(Some(Box::pin(async move { pool.close().await }))),
            result: RefCell::new(None),
            cancelled,
        })
    }
}

fn output_limit() -> Result<usize, Rc<Failure>> {
    match std::env::var("SPARROW_WORKER_OUTPUT_LIMIT_BYTES") {
        Ok(value) => value
            .parse::<usize>()
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(|| failed("SPARROW_WORKER_OUTPUT_LIMIT_BYTES must be a positive integer")),
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_OUTPUT_LIMIT),
        Err(error) => Err(failed(error)),
    }
}
fn failed(message: impl ToString) -> Rc<Failure> {
    Rc::new(Failure::Process(message.to_string()))
}
async fn run(
    command: Vec<OsString>,
    input: Vec<u8>,
    environment: Option<std::collections::BTreeMap<String, String>>,
    cancelled: tokio::sync::watch::Receiver<bool>,
) -> Outcome {
    let output_limit = output_limit()?;
    run_with_limit(command, input, environment, cancelled, output_limit).await
}

async fn run_with_limit(
    command: Vec<OsString>,
    input: Vec<u8>,
    environment: Option<std::collections::BTreeMap<String, String>>,
    mut cancelled: tokio::sync::watch::Receiver<bool>,
    output_limit: usize,
) -> Outcome {
    if *cancelled.borrow() {
        return Err(Rc::new(Failure::Cancelled));
    }
    let (executable, args) = command
        .split_first()
        .ok_or_else(|| failed("empty worker command"))?;
    let mut command = tokio::process::Command::new(executable);
    if let Some(environment) = environment {
        command.env_clear().envs(environment);
    }
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .args(args)
        // Independent of argument parsing: a marked child may never fall back
        // to main if the worker flag is lost or misplaced in a future change.
        .env("SPARROW_INTERNAL_WORKER", "1")
        .env_remove("SPARROW_INTERNAL_WORKER_POOL")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(failed)?;
    let mut group = ProcessGroup::new(&child);
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let remaining = Cell::new(output_limit);
    let result = {
        let operation = async {
            let (_, stdout, stderr, status) = tokio::try_join!(
                async move { stdin.write_all(&input).await },
                read_limited(stdout, &remaining),
                read_limited(stderr, &remaining),
                async {
                    let status = child.wait().await;
                    // A descendant must not keep pipes open after the worker exits.
                    group.terminate();
                    status
                },
            )
            .map_err(failed)?;
            Ok(Rc::new(Output {
                stdout,
                stderr,
                success: status.success(),
                code: status.code(),
            }))
        };
        tokio::select! {
            biased;
            _ = cancelled.changed() => Err(Rc::new(Failure::Cancelled)),
            result = operation => result,
        }
    };
    if result.is_err() {
        group.terminate();
        let _ = child.start_kill();
        child.wait().await.map_err(failed)?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worker_flag_precedes_program_path_and_language_arguments() {
        for args in [
            vec!["sparrow-next", "program.spb"],
            vec!["sparrow-next", "--cold", "program.spb", "--test"],
            vec!["sparrow-next", "--test", "program.spb"],
        ] {
            let original: Vec<OsString> = args.into_iter().map(Into::into).collect();
            let command = worker_command(original.clone());
            assert_eq!(command[0], original[0]);
            assert_eq!(command[1], "--worker");
            assert_eq!(&command[2..], &original[1..]);
        }
        assert!(worker_command(vec![]).is_empty());
    }

    #[test]
    fn output_overflow_stops_worker_without_waiting_for_exit() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (_cancel, wait) = tokio::sync::watch::channel(false);
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(3),
                run_with_limit(
                    vec![
                        "/bin/sh".into(),
                        "-c".into(),
                        "head -c 65536 /dev/zero; exec sleep 5".into(),
                    ],
                    vec![],
                    None,
                    wait,
                    4096,
                ),
            )
            .await
            .unwrap();
            assert!(matches!(result, Err(error) if matches!(&*error,
                Failure::Process(message) if message.contains("output exceeds byte limit"))));
        });
    }

    #[test]
    fn output_budget_is_shared_by_both_streams() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let remaining = Cell::new(5);
            assert_eq!(read_limited(&b"abc"[..], &remaining).await.unwrap(), b"abc");
            assert!(read_limited(&b"def"[..], &remaining).await.is_err());
            assert_eq!(remaining.get(), 2);
        });
    }

    #[cfg(unix)]
    #[test]
    fn worker_exit_closes_pipes_inherited_by_descendants() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let request = Request::from_command(
                vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "sleep 5 & printf done".into(),
                ],
                vec![],
                None,
            );
            let output = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                std::future::poll_fn(|cx| request.poll(cx)),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(output.stdout, b"done");
            assert!(output.success);
        });
    }

    #[test]
    fn cancelling_a_process_reaps_it_and_large_pipe_outputs_do_not_deadlock() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let request = Request::from_command(
                vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "printf ready; printf error >&2; exec sleep 60".into(),
                ],
                vec![],
                None,
            );
            let wait = std::future::poll_fn(|cx| request.poll(cx));
            tokio::pin!(wait);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(20), wait.as_mut())
                    .await
                    .is_err()
            );
            request.cancel();
            let result = tokio::time::timeout(std::time::Duration::from_secs(3), wait)
                .await
                .unwrap();
            assert!(matches!(result,Err(failure) if matches!(&*failure,Failure::Cancelled)));
            let request = Request::from_command(
                vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "cat; head -c 200000 /dev/zero >&2".into(),
                ],
                vec![b'x'; 200000],
                None,
            );
            let output = tokio::time::timeout(
                std::time::Duration::from_secs(3),
                std::future::poll_fn(|cx| request.poll(cx)),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(output.success);
            assert_eq!(output.stdout.len(), 200000);
            assert_eq!(output.stderr.len(), 200000);
            assert_eq!(output.code, Some(0));
        });
    }
}

/// One results() lifetime. Only idle, successfully answered sessions are reused.
#[derive(Debug)]
pub struct Pool {
    bound: usize,
    active: Cell<usize>,
    closed: Cell<bool>,
    #[cfg(unix)]
    idle: RefCell<Vec<Session>>,
}
impl Pool {
    pub(crate) fn new(bound: usize) -> Rc<Self> {
        Rc::new(Self {
            bound,
            active: Cell::new(0),
            closed: Cell::new(false),
            #[cfg(unix)]
            idle: RefCell::new(Vec::new()),
        })
    }

    async fn close(&self) -> Outcome {
        self.closed.set(true);
        #[cfg(unix)]
        {
            let sessions = std::mem::take(&mut *self.idle.borrow_mut());
            let mut error = None;
            for mut session in sessions {
                if let Err(failure) = session.stop().await {
                    error.get_or_insert(failure);
                }
            }
            if let Some(error) = error {
                return Err(error);
            }
        }
        if self.active.get() != 0 {
            return Err(failed("worker pool closed with active requests"));
        }
        Ok(Rc::new(Output {
            stdout: vec![],
            stderr: vec![],
            success: true,
            code: Some(0),
        }))
    }

    #[cfg(unix)]
    async fn run(
        self: Rc<Self>,
        command: Vec<OsString>,
        input: Vec<u8>,
        mut cancelled: tokio::sync::watch::Receiver<bool>,
    ) -> Outcome {
        if *cancelled.borrow() {
            return Err(Rc::new(Failure::Cancelled));
        }
        if self.closed.get() {
            return Err(failed("worker pool is closed"));
        }
        if self.active.get() >= self.bound {
            return Err(failed("worker pool bound exceeded"));
        }
        let limit = output_limit()?;
        self.active.set(self.active.get() + 1);
        struct Active<'a>(&'a Cell<usize>);
        impl Drop for Active<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() - 1);
            }
        }
        let _active = Active(&self.active);
        let idle = self.idle.borrow_mut().pop();
        let mut session = match idle {
            Some(session) => session,
            None => Session::start(command)?,
        };
        let result = tokio::select! {
            biased;
            _ = cancelled.changed() => Err(Rc::new(Failure::Cancelled)),
            result = session.exchange(&input, limit) => result,
        };
        if result.as_ref().map_or(true, |output| !output.success) || self.closed.get() {
            session.stop().await?;
        } else {
            self.idle.borrow_mut().push(session);
        }
        result
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct Session {
    child: tokio::process::Child,
    group: ProcessGroup,
    control: tokio::net::UnixStream,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
}

#[cfg(unix)]
impl Session {
    fn start(command: Vec<OsString>) -> Result<Self, Rc<Failure>> {
        let (executable, args) = command.split_first().ok_or_else(|| failed("empty worker command"))?;
        let (parent, child_socket) = std::os::unix::net::UnixStream::pair().map_err(failed)?;
        parent.set_nonblocking(true).map_err(failed)?;
        let control = tokio::net::UnixStream::from_std(parent).map_err(failed)?;
        let child_socket: std::os::fd::OwnedFd = child_socket.into();
        let mut child = tokio::process::Command::new(executable)
            .args(args)
            .process_group(0)
            .env("SPARROW_INTERNAL_WORKER", "1")
            .env("SPARROW_INTERNAL_WORKER_POOL", "1")
            .stdin(std::process::Stdio::from(child_socket))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(failed)?;
        let group = ProcessGroup::new(&child);
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        Ok(Self {
            child,
            group,
            control,
            stdout,
            stderr,
        })
    }

    async fn stop(&mut self) -> Result<(), Rc<Failure>> {
        self.group.terminate();
        let _ = self.child.start_kill();
        self.child.wait().await.map_err(failed)?;
        Ok(())
    }

    async fn exchange(&mut self, input: &[u8], limit: usize) -> Outcome {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let remaining = Cell::new(limit);
        let Self {
            child,
            group,
            control,
            stdout: out,
            stderr: err,
        } = self;
        let response = async {
            control.write_u64(input.len() as u64).await.map_err(ExchangeError::io)?;
            control.write_all(input).await.map_err(ExchangeError::io)?;
            let length = control.read_u64().await.map_err(ExchangeError::io)?;
            let length = usize::try_from(length).map_err(|e| ExchangeError::Invalid(failed(e)))?;
            // Reserve response bytes before allocation; stdout/stderr share the budget.
            spend(&remaining, length).map_err(ExchangeError::Invalid)?;
            let mut response = vec![0; length];
            control.read_exact(&mut response).await.map_err(ExchangeError::io)?;
            Ok::<_, ExchangeError>(response)
        };
        let result = tokio::select! {
            result = response => result,
            result = collect(out, &mut stdout, &remaining) => Err(ExchangeError::Invalid(result.unwrap_err())),
            result = collect(err, &mut stderr, &remaining) => Err(ExchangeError::Invalid(result.unwrap_err())),
            status = child.wait() => {
                group.terminate();
                let status = status.map_err(failed)?;
                drain(out, &mut stdout, &remaining)?;
                drain(err, &mut stderr, &remaining)?;
                return Ok(Rc::new(Output { stdout, stderr, success: false, code: status.code() }));
            }
        };
        let status = if matches!(result, Err(ExchangeError::Closed)) {
            // EOF can precede the SIGCHLD notification. Wait/reap rather than
            // replacing spawn's stderr/exit diagnostic with a pipe error.
            let status = tokio::select! {
                status = child.wait() => status.map_err(failed)?,
                result = collect(out, &mut stdout, &remaining) => return Err(result.unwrap_err()),
                result = collect(err, &mut stderr, &remaining) => return Err(result.unwrap_err()),
            };
            group.terminate();
            Some(status)
        } else {
            None
        };
        // The child flushes both pipes before acknowledging on the independent
        // control socket. Read the remaining kernel bytes, without a readiness
        // race or a delimiter that user output could impersonate.
        drain(out, &mut stdout, &remaining)?;
        drain(err, &mut stderr, &remaining)?;
        match result {
            Ok(response) => {
                stdout.extend(response);
                Ok(Rc::new(Output {
                    stdout,
                    stderr,
                    success: true,
                    code: Some(0),
                }))
            }
            Err(ExchangeError::Closed) => Ok(Rc::new(Output {
                stdout,
                stderr,
                success: false,
                code: status.and_then(|s| s.code()),
            })),
            Err(ExchangeError::Invalid(error)) => Err(error),
        }
    }
}

#[cfg(unix)]
enum ExchangeError {
    Closed,
    Invalid(Rc<Failure>),
}
#[cfg(unix)]
impl ExchangeError {
    fn io(error: std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset => Self::Closed,
            _ => Self::Invalid(failed(error)),
        }
    }
}

#[cfg(unix)]
fn spend(remaining: &Cell<usize>, count: usize) -> Result<(), Rc<Failure>> {
    let left = remaining
        .get()
        .checked_sub(count)
        .ok_or_else(|| failed("worker output exceeds byte limit"))?;
    remaining.set(left);
    Ok(())
}

#[cfg(unix)]
async fn collect(
    stream: &mut (impl AsyncRead + Unpin),
    output: &mut Vec<u8>,
    remaining: &Cell<usize>,
) -> Result<(), Rc<Failure>> {
    let mut buffer = [0; 8192];
    loop {
        let count = stream.read(&mut buffer).await.map_err(failed)?;
        // An action may close a stream before answering on the control socket.
        if count == 0 {
            return std::future::pending().await;
        }
        spend(remaining, count)?;
        output.extend_from_slice(&buffer[..count]);
    }
}

#[cfg(unix)]
fn drain(stream: &impl std::os::fd::AsFd, output: &mut Vec<u8>, remaining: &Cell<usize>) -> Result<(), Rc<Failure>> {
    let mut buffer = [0; 8192];
    loop {
        match rustix::io::read(stream, &mut buffer) {
            Ok(0) | Err(rustix::io::Errno::AGAIN) => return Ok(()),
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(failed(error)),
            Ok(count) => {
                spend(remaining, count)?;
                output.extend_from_slice(&buffer[..count]);
            }
        }
    }
}

#[cfg(all(test, unix))]
mod pool_tests {
    use super::*;
    const CHILD: &str = r#"
import os, socket, struct, time
s = socket.socket(fileno=os.dup(0))
def exact(n):
    b = b''
    while len(b) < n:
        chunk = s.recv(n-len(b))
        if not chunk: raise EOFError()
        b += chunk
    return b
while True:
    try: n = struct.unpack('>Q', exact(8))[0]
    except EOFError: break
    data = exact(n)
    if data == b'sleep': time.sleep(60)
    if data == b'flood':
        os.write(1, b'x' * 65536)
        time.sleep(60)
    if data == b'oversize':
        s.sendall(struct.pack('>Q', 2**63))
        time.sleep(60)
    if data == b'exit':
        os.write(2, b'worker-crash-diagnostic\n')
        os._exit(7)
    os.write(1, b'ordinary OKW2 OKL1 output\n')
    os.write(2, b'item stderr\n')
    result = str(os.getpid()).encode() + b':' + data
    s.sendall(struct.pack('>Q', len(result)) + result)
"#;
    fn command() -> Vec<OsString> {
        vec!["python3".into(), "-u".into(), "-c".into(), CHILD.into()]
    }
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }
    async fn request(pool: Rc<Pool>, input: &[u8]) -> Outcome {
        let (_cancel, wait) = tokio::sync::watch::channel(false);
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            pool.run(command(), input.to_vec(), wait),
        )
        .await
        .unwrap()
    }
    fn gone(pid: u32) {
        let pid = rustix::process::Pid::from_raw(pid as i32).unwrap();
        assert_eq!(
            rustix::process::kill_process(pid, rustix::process::Signal::CONT),
            Err(rustix::io::Errno::SRCH),
            "worker must already be reaped"
        );
    }

    #[test]
    fn reuses_process_keeps_output_separate_and_reaps_at_close() {
        runtime().block_on(async {
            let pool = Pool::new(1);
            let first = request(pool.clone(), b"1").await.unwrap();
            let pid = pool.idle.borrow()[0].child.id().unwrap();
            for item in [b"2", b"3"] {
                let result = request(pool.clone(), item).await.unwrap();
                assert_eq!(pool.idle.borrow()[0].child.id(), Some(pid));
                assert_eq!(result.stderr, b"item stderr\n");
                assert_eq!(
                    result.stdout,
                    format!("ordinary OKW2 OKL1 output\n{pid}:{}", String::from_utf8_lossy(item)).as_bytes()
                );
            }
            assert!(first.success);
            assert_eq!(pool.active.get(), 0);
            pool.close().await.unwrap();
            gone(pid);
            assert!(request(pool, b"4").await.is_err(), "closed pools never restart");
        });
    }

    #[test]
    fn cancellation_reaps_active_worker_and_bound_refuses_more_work() {
        runtime().block_on(async {
            let pool = Pool::new(1);
            request(pool.clone(), b"ready").await.unwrap();
            let pid = pool.idle.borrow()[0].child.id().unwrap();
            let (cancel, wait) = tokio::sync::watch::channel(false);
            let running = pool.clone().run(command(), b"sleep".to_vec(), wait);
            tokio::pin!(running);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(20), &mut running)
                    .await
                    .is_err()
            );
            assert_eq!(pool.active.get(), 1);
            let refused = request(pool.clone(), b"later").await.unwrap_err();
            assert!(matches!(&*refused, Failure::Process(message) if message == "worker pool bound exceeded"));
            cancel.send_replace(true);
            let result = tokio::time::timeout(std::time::Duration::from_secs(3), running)
                .await
                .unwrap();
            assert!(matches!(result, Err(error) if matches!(&*error, Failure::Cancelled)));
            gone(pid);
            assert_eq!(pool.active.get(), 0);
            assert!(pool.idle.borrow().is_empty());
            pool.close().await.unwrap();
        });
    }

    #[test]
    fn oversized_response_is_rejected_before_allocating_and_session_is_reaped() {
        runtime().block_on(async {
            let pool = Pool::new(1);
            request(pool.clone(), b"ready").await.unwrap();
            let pid = pool.idle.borrow()[0].child.id().unwrap();
            let result = request(pool.clone(), b"oversize").await;
            assert!(matches!(result, Err(error) if matches!(&*error, Failure::Process(message) if message.contains("byte limit"))));
            gone(pid);
            assert!(pool.idle.borrow().is_empty());
        });
    }

    #[test]
    fn pooled_output_limit_is_enforced_while_the_item_is_running() {
        runtime().block_on(async {
            let mut session = Session::start(command()).unwrap();
            let pid = session.child.id().unwrap();
            let result = tokio::time::timeout(std::time::Duration::from_secs(3), session.exchange(b"flood", 1024)).await.unwrap();
            assert!(matches!(result, Err(error) if matches!(&*error, Failure::Process(message) if message.contains("byte limit"))));
            session.stop().await.unwrap();
            gone(pid);
        });
    }

    #[test]
    fn a_crashed_reused_worker_keeps_its_exit_status_and_stderr() {
        runtime().block_on(async {
            let pool = Pool::new(1);
            request(pool.clone(), b"ready").await.unwrap();
            let pid = pool.idle.borrow()[0].child.id().unwrap();
            let output = request(pool.clone(), b"exit").await.unwrap();
            assert!(!output.success);
            assert_eq!(output.code, Some(7));
            assert_eq!(output.stderr, b"worker-crash-diagnostic\n");
            gone(pid);
            assert!(pool.idle.borrow().is_empty());
        });
    }

    #[test]
    fn empty_pool_and_pre_cancelled_request_never_launch_a_worker() {
        runtime().block_on(async {
            let pool = Pool::new(2);
            let (_cancel, wait) = tokio::sync::watch::channel(true);
            let result = pool.clone().run(vec!["/no/such/worker".into()], vec![], wait).await;
            assert!(matches!(result, Err(error) if matches!(&*error, Failure::Cancelled)));
            assert_eq!(pool.active.get(), 0);
            pool.close().await.unwrap();
            assert!(pool.idle.borrow().is_empty());
        });
    }
}
