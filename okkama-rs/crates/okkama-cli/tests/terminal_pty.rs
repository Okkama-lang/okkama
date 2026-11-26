//! A terminal read interrupted by a timeout or a cancelled task gives its
//! late bytes back to the session, and a resize reports the new geometry
//! (#12). Runs `tests/terminal/cancel.ok` under a pseudo-terminal driven by
//! `tests/terminal/pty_driver.py`.

use std::path::Path;
use std::process::Command;

#[test]
#[cfg(unix)]
fn terminal_reads_survive_cancellation_and_resizes_report_geometry() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skipped: no python3");
        return;
    }
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let std = here.join("../../../std");
    let driver = here.join("tests/terminal/pty_driver.py");
    let program = here.join("tests/terminal/cancel.ok");
    let output = Command::new("python3")
        .arg(&driver)
        .arg(env!("CARGO_BIN_EXE_okkama"))
        .arg(&program)
        .env("OKKAMA_STDLIB_DIR", &std)
        .output()
        .expect("run the pty driver");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// #1017: on a terminal without `COLUMNS`, `okkama man` wraps its page to the
/// window's width (it took 80 in a window of any width), and its pager gets
/// `LESS=R` for the bold headings.
#[test]
#[cfg(unix)]
fn man_wraps_to_the_window_width() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skipped: no python3");
        return;
    }
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    // The child gets a pseudo-terminal of 50 columns; its output comes back
    // as it was written to the terminal.
    let driver = r#"
import fcntl, os, pty, struct, sys, termios
pid, fd = pty.fork()
if pid == 0:
    os.environ.pop("COLUMNS", None)
    os.environ.pop("LESS", None)
    if sys.argv[2] == "env":
        os.environ["PAGER"] = "env"
    else:
        os.environ.pop("PAGER", None)
    fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 50, 0, 0))
    os.execv(sys.argv[1], [sys.argv[1], "man"] + (["--no-pager"] if sys.argv[2] != "env" else []) + ["io/stdout"])
out = b""
while True:
    try:
        chunk = os.read(fd, 65536)
    except OSError:
        break
    if not chunk:
        break
    out += chunk
os.waitpid(pid, 0)
sys.stdout.write(out.decode("utf-8", "replace").replace("\r", ""))
"#;
    let run = |pager: &str| {
        let output = Command::new("python3")
            .args(["-c", driver, env!("CARGO_BIN_EXE_okkama"), pager])
            .env("OKKAMA_STDLIB_DIR", here.join("../../../std"))
            .env("NO_COLOR", "1")
            .output()
            .expect("run the pty driver");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    // The pager (here `env`, which prints its environment) gets LESS=R when
    // LESS is not set, so `less` shows the bold headings (#1017).
    let environment = run("env");
    assert!(environment.lines().any(|line| line == "LESS=R"), "{environment}");
    let page = run("none");
    // The summary under NAME is 62 columns on one line; at 50 it wraps.
    // (Code examples keep their lines whatever the width.)
    let name = page.lines().skip_while(|line| *line != "NAME").nth(1).unwrap_or_default();
    assert!(name.starts_with("    io/stdout - ") && name.chars().count() <= 50, "{page}");
}
