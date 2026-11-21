//! A synchronous program and process signals (#638), on real signals: without
//! an Interruption SIGINT ends it at once and its defer does not run; with one
//! the signal stops it at its check, its cleanup runs and it ends with 130 or
//! 143; a second signal during a cleanup ends it at once. Runs
//! `tests/interruption/stop.ok`.
#![cfg(unix)]

use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{JoinHandle, sleep, spawn};
use std::time::{Duration, Instant};

/// A running `stop.ok`; dropped while running, it is killed.
struct Program {
    child: Child,
    stderr: Arc<Mutex<String>>,
    reader: Option<JoinHandle<()>>,
}

impl Program {
    fn start(mode: &str) -> Program {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut child = Command::new(env!("CARGO_BIN_EXE_okkama"))
            .arg("run")
            .arg(here.join("tests/interruption/stop.ok"))
            .arg(mode)
            .env("OKKAMA_STDLIB_DIR", here.join("../../../std"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start okkama");
        let stderr = Arc::new(Mutex::new(String::new()));
        let reader = child.stderr.take().map(|mut err| {
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
        let program = Program { child, stderr, reader };
        program.said("ready");
        program
    }

    /// Wait until stderr has `line`.
    fn said(&self, line: &str) {
        let deadline = Instant::now() + Duration::from_secs(120);
        while !self.stderr.lock().expect("stderr").contains(line) {
            assert!(Instant::now() < deadline, "the program never said {line:?}: {}", self.stderr.lock().unwrap());
            sleep(Duration::from_millis(20));
        }
    }

    fn signal(&self, name: &str) {
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("kill -{name} {}", self.child.id()))
            .status()
            .expect("run kill");
        assert!(status.success(), "kill -{name} failed");
    }

    /// Wait for the end (at most a minute): the status and all of stderr.
    fn ended(mut self) -> (ExitStatus, String) {
        let deadline = Instant::now() + Duration::from_secs(60);
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("poll the program") {
                break status;
            }
            assert!(Instant::now() < deadline, "the program did not end: {}", self.stderr.lock().unwrap());
            sleep(Duration::from_millis(20));
        };
        if let Some(reader) = self.reader.take() {
            reader.join().expect("stderr reader");
        }
        let text = self.stderr.lock().expect("stderr").clone();
        (status, text)
    }
}

impl Drop for Program {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn without_an_interruption_sigint_ends_the_program_at_once() {
    let program = Program::start("plain");
    program.signal("INT");
    let (status, stderr) = program.ended();
    assert_eq!(status.signal(), Some(libc_sigint()), "{status}: {stderr}");
    assert!(!stderr.contains("cleaned"), "{stderr}");
}

#[test]
fn an_interruption_stops_the_program_at_its_check_and_its_cleanup_runs() {
    for (name, code) in [("INT", 130), ("TERM", 143)] {
        let program = Program::start("checked");
        program.signal(name);
        let (status, stderr) = program.ended();
        assert_eq!(status.code(), Some(code), "{name}: {status}: {stderr}");
        assert!(stderr.contains("cleaned") && stderr.contains("interrupted"), "{name}: {stderr}");
    }
}

#[test]
fn a_second_signal_during_the_cleanup_ends_the_program_at_once() {
    let program = Program::start("stubborn");
    program.signal("INT");
    program.said("cleaning");
    program.signal("INT");
    let (status, stderr) = program.ended();
    assert_eq!(status.signal(), Some(libc_sigint()), "{status}: {stderr}");
}

fn libc_sigint() -> i32 {
    2
}
