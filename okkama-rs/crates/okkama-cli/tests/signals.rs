//! A web application stops on SIGTERM or SIGINT (#80): it stops accepting,
//! lets the request in flight finish, and `listen()` returns, so the program
//! goes on and ends with code 0; a second signal during the drain ends the
//! process at once; an HTTPS application stops the same way, also while TLS
//! handshakes are still in flight (#1000, #1021). Runs
//! `tests/signals/serve.ok` with `okkama run`.
#![cfg(unix)]
mod tree;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{JoinHandle, sleep, spawn};
use std::time::{Duration, Instant};

/// A running `serve.ok`; dropped while running, it is killed.
struct App {
    child: Child,
    port: u16,
    /// The program's stderr as it comes, read by `errors` (#556).
    stderr: Arc<Mutex<String>>,
    errors: Option<JoinHandle<()>>,
}

struct Ended {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl App {
    /// Start the program and wait until it accepts connections; `/slow`
    /// answers after `delay` seconds, and `tls` serves HTTPS.
    fn start(delay: u64, tls: bool) -> App {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let port = TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("a free port")
            .port();
        let mut command = tree::okkama();
        command
            .args(["run"])
            .arg(here.join("tests/signals/serve.ok"))
            .arg(port.to_string())
            .arg(delay.to_string());
        if tls {
            command.arg("tls");
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start okkama");
        let stderr = Arc::new(Mutex::new(String::new()));
        let errors = child.stderr.take().map(|mut err| {
            let stderr = Arc::clone(&stderr);
            spawn(move || {
                let mut chunk = [0u8; 4096];
                while let Ok(read) = err.read(&mut chunk) {
                    if read == 0 {
                        break;
                    }
                    stderr.lock().expect("stderr").push_str(&String::from_utf8_lossy(&chunk[..read]));
                }
            })
        });
        let mut app = App { child, port, stderr, errors };
        let deadline = Instant::now() + Duration::from_secs(180);
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            if let Some(status) = app.child.try_wait().expect("poll the program") {
                let ended = app.outputs(status);
                panic!("the program ended before listening ({status}):\n{}{}", ended.stdout, ended.stderr);
            }
            assert!(Instant::now() < deadline, "the program never listened");
            sleep(Duration::from_millis(100));
        }
        app
    }

    fn signal(&self, name: &str) {
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("kill -{name} {}", self.child.id()))
            .status()
            .expect("run kill");
        assert!(status.success(), "kill -{name} failed");
    }

    /// GET `path` in a thread: the whole answer, or the error that cut it.
    fn get(&self, path: &'static str) -> JoinHandle<Result<String, String>> {
        let port = self.port;
        spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|error| error.to_string())?;
            stream.set_read_timeout(Some(Duration::from_secs(60))).map_err(|error| error.to_string())?;
            write!(stream, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .map_err(|error| error.to_string())?;
            let mut answer = String::new();
            stream.read_to_string(&mut answer).map_err(|error| error.to_string())?;
            Ok(answer)
        })
    }

    /// Wait until the slow request is being handled, not just connected (#556):
    /// a fixed sleep let the signal win the race on a loaded machine.
    fn slow_started(&self, limit: Duration) {
        let deadline = Instant::now() + limit;
        while !self.stderr.lock().expect("stderr").contains("slow started") {
            assert!(Instant::now() < deadline, "/slow never started");
            sleep(Duration::from_millis(50));
        }
    }

    /// Whether the program stops accepting connections within `limit`. A
    /// debug build compiles the drain on first use, so the limit is generous;
    /// a passing run returns as soon as a connection is refused.
    fn refusing(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            if TcpStream::connect(("127.0.0.1", self.port)).is_err() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            sleep(Duration::from_millis(50));
        }
    }

    fn outputs(&mut self, status: ExitStatus) -> Ended {
        let mut stdout = String::new();
        let mut stderr = String::new();
        if let Some(mut out) = self.child.stdout.take() {
            let _ = out.read_to_string(&mut stdout);
        }
        if let Some(errors) = self.errors.take() {
            let _ = errors.join();
        }
        stderr.push_str(&self.stderr.lock().expect("stderr"));
        Ended { status, stdout, stderr }
    }

    /// Wait for the program to end within `limit`.
    fn ended(mut self, limit: Duration) -> Ended {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(status) = self.child.try_wait().expect("poll the program") {
                return self.outputs(status);
            }
            assert!(Instant::now() < deadline, "the program did not end within {limit:?}");
            sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[test]
fn sigterm_drains_the_request_in_flight_and_listen_returns() {
    let app = App::start(3, false);
    let hello = app.get("/").join().expect("hello thread").expect("hello answer");
    assert!(hello.starts_with("HTTP/1.1 200") && hello.ends_with("hello"), "{hello}");
    let slow = app.get("/slow");
    app.slow_started(Duration::from_secs(60));
    app.signal("TERM");
    assert!(app.refusing(Duration::from_secs(30)), "still accepting after SIGTERM");
    let answer = slow.join().expect("slow thread").expect("slow answer");
    assert!(answer.starts_with("HTTP/1.1 200") && answer.ends_with("slow and done"), "{answer}");
    let ended = app.ended(Duration::from_secs(60));
    assert_eq!(ended.status.code(), Some(0), "{}{}", ended.stdout, ended.stderr);
    assert_eq!(ended.stdout, "stopped\n", "{}", ended.stderr);
}

#[test]
fn a_second_signal_during_the_drain_ends_the_process_at_once() {
    let app = App::start(60, false);
    let slow = app.get("/slow");
    app.slow_started(Duration::from_secs(60));
    app.signal("INT");
    assert!(app.refusing(Duration::from_secs(30)), "still accepting after SIGINT");
    app.signal("INT");
    let ended = app.ended(Duration::from_secs(10));
    assert_ne!(ended.status.code(), Some(0), "{}{}", ended.stdout, ended.stderr);
    assert!(!ended.stdout.contains("stopped"), "{}", ended.stdout);
    let _ = slow.join();
}

#[test]
fn an_https_application_stops_on_sigterm() {
    let app = App::start(3, true);
    app.signal("TERM");
    let ended = app.ended(Duration::from_secs(60));
    assert_eq!(ended.status.code(), Some(0), "{}{}", ended.stdout, ended.stderr);
    assert_eq!(ended.stdout, "stopped\n", "{}", ended.stderr);
}

/// SIGTERM while TLS handshakes are in flight (#1000, #1021): one client sends
/// nothing, one the start of a ClientHello, and one hangs up in the middle of
/// one as the signal comes. The drain cancels those handshakes, which is not a
/// failure: `listen()` returns and the program ends with 0.
#[test]
fn an_https_application_stops_on_sigterm_during_tls_handshakes() {
    // A TLS 1.2 record header announcing a 512-byte ClientHello, then its
    // first bytes: the server waits for the rest.
    const PARTIAL: [u8; 9] = [0x16, 0x03, 0x01, 0x02, 0x00, 0x01, 0x00, 0x01, 0xfc];
    let app = App::start(3, true);
    let connect = || TcpStream::connect(("127.0.0.1", app.port)).expect("connect");
    let silent = connect();
    let mut partial = connect();
    partial.write_all(&PARTIAL).expect("send a partial ClientHello");
    let mut hanging_up = connect();
    hanging_up.write_all(&PARTIAL).expect("send a partial ClientHello");
    // The server accepts connections in order: once it has refused this
    // one's handshake, it has accepted the three before it, which are
    // still handshaking. Plain HTTP is no TLS record header, so the
    // refusal comes at once (#1024); before, "GET /" read as the header
    // of a record of 8239 bytes and waited out the 30-second headers
    // timeout, when the three handshakes had timed out too.
    let mut refused = connect();
    refused.set_read_timeout(Some(Duration::from_secs(60))).expect("read timeout");
    let started = Instant::now();
    refused.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").expect("send plain HTTP");
    let mut alert = Vec::new();
    let _ = refused.read_to_end(&mut alert);
    assert!(started.elapsed() < Duration::from_secs(10), "the refusal took {:?}", started.elapsed());
    // A peer that has already closed its side makes macOS answer ENOTCONN.
    let _ = hanging_up.shutdown(std::net::Shutdown::Both);
    app.signal("TERM");
    let ended = app.ended(Duration::from_secs(60));
    assert_eq!(ended.status.code(), Some(0), "{}{}", ended.stdout, ended.stderr);
    assert_eq!(ended.stdout, "stopped\n", "{}", ended.stderr);
    drop((silent, partial, hanging_up, refused));
}
