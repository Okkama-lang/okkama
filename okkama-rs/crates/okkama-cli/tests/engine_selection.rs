//! One engine (Tweed, owner 2026-11-20): Sparrow. There is no engine to select: `--engine` is an unknown
//! option, and the environment variables that used to select one are ignored.
mod tree;
use std::process::Command;

fn command() -> Command {
    let mut command = tree::okkama();
    command.env_remove("OKKAMA_ENGINE").env_remove("OCS_RUST_ENGINE");
    command
}

#[test]
fn an_engine_option_is_an_unknown_option_for_programs_and_tests() {
    for flags in [
        &["run", "--engine", "sparrow", "missing.ok"][..],
        &["run", "-t", "--engine", "sparrow", "missing.ok"],
        &["run", "--engine", "ir", "missing.ok"],
        &["run", "--engine", "jvm", "missing.ok"],
    ] {
        let output = command().args(flags).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{flags:?}");
        assert!(output.stdout.is_empty());
        let message = String::from_utf8_lossy(&output.stderr);
        assert!(message.contains("unknown option `--engine`"), "{flags:?}: {message}");
    }
}

#[test]
fn ocs_reports_sparrow_whatever_the_environment() {
    for value in [None, Some("ir"), Some("jvm"), Some("typo")] {
        let mut ocs = command();
        if let Some(value) = value {
            ocs.env("OCS_RUST_ENGINE", value);
        }
        let output = ocs.arg("--ocs-capabilities").output().unwrap();
        assert!(output.status.success(), "{value:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("\"engine\": \"sparrow\""));
    }
}

#[test]
fn a_build_target_is_an_unknown_option() {
    for flags in [
        &["build", "--target", "jvm", "missing.ok", "-o", "out.spb"][..],
        &["build", "--target=sparrow", "missing.ok", "-o", "out.spb"],
        &["build", "--backend", "sparrow", "missing.ok", "-o", "out.spb"],
    ] {
        let output = command().args(flags).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{flags:?}");
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("unknown option"), "{flags:?}");
    }
}

/// One engine, no `--engine` option; the REPL is back on Sparrow (#588, owner 2026-11-14).
#[test]
fn help_lists_the_sparrow_repl_and_no_engine_option() {
    let help = command().arg("--help").output().unwrap();
    assert!(String::from_utf8_lossy(&help.stdout).contains("okkama repl"));
    assert!(!String::from_utf8_lossy(&help.stdout).contains("--engine"));
}
