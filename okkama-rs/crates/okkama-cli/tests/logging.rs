//! #1180: std destinations through real stderr, without sockets or workers.
mod tree;

use std::io::Write;
use std::process::{Output, Stdio};

fn inline(file: &str, filter: &str) -> Output {
    let output = tree::okkama()
        .args(["run", "-t", "--filter", filter])
        .arg(tree::std_dir().join(file))
        .output()
        .expect("run inline logging test");
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("--- PASS:"));
    output
}

#[test]
fn stderr_lines_match_synchronous_streams_byte_for_byte() {
    for (filter, groups) in [("stderr text matches", 2), ("stderr JSON matches", 1)] {
        let output = inline("logging/stream.ok", filter);
        let lines: Vec<_> = output.stderr.split_inclusive(|byte| *byte == b'\n').collect();
        assert_eq!(lines.len(), groups * 10, "{filter}");
        for group in lines.chunks_exact(10) {
            assert_eq!(&group[..5], &group[5..], "{filter}");
        }
    }
    let output = inline("logging/logger.ok", "default logger writes");
    let text = String::from_utf8(output.stderr).expect("UTF-8");
    assert_eq!(text.lines().count(), 1);
    assert!(text.contains(" level=INFO name=\"default\" process="));
    assert!(text.ends_with(" message=\"through StderrLogs\" fields={}\n"));
}

#[test]
fn default_logger_uses_the_host_and_limits_are_applied_once() {
    let source = br#"module logging_host;
import "logging/logger" take Logger;
import "logging/stream" take StderrLogs;
import "runtime/stdio" take log_buffer_limit;
async main() is int {
    let logs = StderrLogs(limit: 0);
    let log = Logger(name: "probe", destination: logs);
    let before = logs.dropped();
    log.info("dropped first");
    confirm logs.dropped() == before + 1 else panic "limit was not applied";
    let default = Logger("default");
    log_buffer_limit(0);
    default.info("dropped default");
    confirm logs.dropped() == before + 2 else panic "default bypassed host";
    await default.drain();
    log_buffer_limit(1048576);
    log.info("kept after limit change");
    await log.drain();
    confirm logs.dropped() == before + 2 else panic "limit was applied again";
    0;
}
"#;
    let mut child = tree::okkama().args(["run", "-"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("start logging program");
    child.stdin.take().unwrap().write_all(source).unwrap();
    let output = child.wait_with_output().unwrap();
    let text = String::from_utf8(output.stderr).expect("UTF-8");
    assert!(output.status.success(), "{text}");
    assert!(output.stdout.is_empty());
    assert_eq!(text.lines().count(), 2, "{text}");
    assert!(text.starts_with("okkama: 2 log lines dropped\n"), "{text}");
    assert!(text.ends_with(" message=\"kept after limit change\" fields={}\n"), "{text}");
}
