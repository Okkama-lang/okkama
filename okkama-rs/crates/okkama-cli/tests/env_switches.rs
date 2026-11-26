//! The CLI reads its switches through the one policy of every crate (`okkama_devtrace::env_switch`, #1095; its
//! value rules are tested there): what each switch does through the binary, default-on switches included, and the
//! std terminal capability.
mod tree;
use std::path::Path;

/// `OKKAMA_TRACE` is on until set (9d7749f4e; one switch rule since #1095): unset, an uncaught failure prints its
/// stack under `okkama run` and under `okkama exec` of a built program; a set value follows the boolean rule.
#[test]
fn trace_is_on_until_set_and_then_follows_the_switch_rule() {
    let program = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/traces/cause.ok");
    let built = std::env::temp_dir().join(format!("okkama-env-switches-{}.spb", std::process::id()));
    let build = tree::okkama().arg("build").arg(&program).arg("-o").arg(&built)
        .output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    for (setting, traced) in [(None, true), (Some(""), false), (Some("0"), false), (Some("Off"), false),
                              (Some("no"), false), (Some("1"), true), (Some("yes"), true)] {
        for command in ["run", "exec"] {
            let mut okkama = tree::okkama();
            okkama.arg(command).arg(if command == "run" { &program } else { &built }).env_remove("OKKAMA_TRACE");
            if let Some(value) = setting { okkama.env("OKKAMA_TRACE", value); }
            let output = okkama.output().unwrap();
            assert_eq!(output.status.code(), Some(1), "{command} {setting:?}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.starts_with("OrderError: OrderError.failed(7)"), "{command} {setting:?}: {stderr}");
            assert_eq!(stderr.contains("Stack trace ("), traced, "{command} {setting:?}: {stderr}");
        }
    }
    let _ = std::fs::remove_file(&built);
}

/// A malformed byte-count switch (`SPARROW_CORE_CODE_SHIFT`, #933) stops the program before it runs, as a program
/// that does not load: the refusal names the switch and carries the `okkama:` prefix of other load errors, under
/// `okkama run` and `okkama exec` alike (#1097).
#[test]
fn a_malformed_code_shift_is_a_load_error_with_the_prefix() {
    let dir = std::env::temp_dir().join(format!("okkama-code-shift-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (source, built) = (dir.join("zero.ok"), dir.join("zero.spb"));
    std::fs::write(&source, "module app; main() is int { 0; }").unwrap();
    let build = tree::okkama().arg("build").arg(&source).arg("-o").arg(&built)
        .env_remove("SPARROW_CORE_CODE_SHIFT").output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    for (command, file) in [("run", &source), ("exec", &built)] {
        let output = tree::okkama().arg(command).arg(file)
            .env("SPARROW_CORE_CODE_SHIFT", "64k").output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{command}");
        assert_eq!(String::from_utf8_lossy(&output.stderr),
                   "okkama: SPARROW_CORE_CODE_SHIFT: expected a byte count from 0 to 65536, got \"64k\"\n", "{command}");
        assert!(output.stdout.is_empty(), "{command}");
        // A well-formed shift runs the program.
        let shifted = tree::okkama().arg(command).arg(file)
            .env("SPARROW_CORE_CODE_SHIFT", "64").output().unwrap();
        assert_eq!(shifted.status.code(), Some(0), "{command}: {}", String::from_utf8_lossy(&shifted.stderr));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn std_terminal_capability_inline_tests() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let output = tree::okkama()
        .args(["run", "-t"])
        .arg(root.join("std/terminal/capability.ok"))
        .output().unwrap();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}


#[test]
fn stats_switch_controls_runtime_report() {
    use std::io::Write;
    use std::process::Stdio;
    for (setting, expected) in [(None, false), (Some(""), false), (Some("0"), false),
                                (Some("FaLsE"), false), (Some("OFF"), false), (Some("No"), false),
                                (Some("1"), true), (Some("yes"), true)] {
        let mut command = tree::okkama();
        command.args(["run", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .env_remove("SPARROW_CORE_STATS").env("OKKAMA_DEV", "off")
            .env("SPARROW_CORE_PROFILE_COMPILE", "off").env("SPARROW_CORE_GC_STRESS", "off")
            .env("SPARROW_CORE_GC_AGES", "off");
        if let Some(value) = setting { command.env("SPARROW_CORE_STATS", value); }
        let mut child = command.spawn().unwrap();
        child.stdin.take().unwrap().write_all(b"module app; main() is int { 0; }").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(stderr.contains("heap bytes allocated"), expected, "{setting:?}: {stderr}");
        if !expected { assert!(stderr.is_empty(), "{setting:?}: {stderr}"); }
    }
}
