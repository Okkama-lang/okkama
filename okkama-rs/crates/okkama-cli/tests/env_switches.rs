//! CLI switches and std terminal capability follow the documented policies.
#[path = "../src/env_switch.rs"]
mod cli_switch;
#[path = "../../sparrow-cli/src/env_switch.rs"]
mod sparrow_switch;

#[test]
fn no_color_is_nonempty_presence_including_boolean_off_words() {
    use std::ffi::OsStr;
    assert!(!cli_switch::no_color(None));
    assert!(!cli_switch::no_color(Some(OsStr::new(""))));
    for text in ["0", "false", "off", "no", "1"] {
        assert!(cli_switch::no_color(Some(OsStr::new(text))));
    }
    assert!(!cli_switch::enabled("OKKAMA_TEST_NONEXISTENT_SWITCH_1067"));
    assert!(!sparrow_switch::enabled("OKKAMA_TEST_NONEXISTENT_SWITCH_1067"));
}

#[test]
fn std_terminal_capability_inline_tests() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_okkama"))
        .args(["run", "-t"])
        .arg(root.join("std/terminal/capability.ok"))
        .env("OKKAMA_STDLIB_DIR", root.join("std"))
        .output().unwrap();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}


#[test]
fn stats_switch_controls_runtime_report() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    for (setting, expected) in [(None, false), (Some(""), false), (Some("0"), false),
                                (Some("FaLsE"), false), (Some("OFF"), false), (Some("No"), false),
                                (Some("1"), true), (Some("yes"), true)] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_okkama"));
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
