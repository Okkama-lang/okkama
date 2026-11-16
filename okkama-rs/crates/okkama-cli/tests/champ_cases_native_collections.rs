//! Collections that cross or hold native state on Champ: arrays, builders and maps of native handles (#751,
//! json's parsed nodes) and natives with map parameters or results (#752, regex and environment).
//!
//! Pure language plus natives, one case per test so the slow debug runs go in parallel. The process environment
//! case runs `okkama` as a child with the environment its case.json gives.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{case_dir, check, root};
use serde_json::Value;
use std::process::Command;

#[test]
fn json_typed_reading() {
    check(&["L09.json.typed-reading"]);
}

#[test]
fn json_member_readers() {
    check(&["L09.json.member-readers"]);
}

#[test]
fn json_parsed_reading() {
    check(&["L09.json.parsed-reading"]);
}

#[test]
fn json_rejections() {
    check(&["L09.json.rejections"]);
}

/// A builder of handles (`Builder(Handle(_))`, #751).
#[test]
fn json_codec() {
    check(&["L09.json.codec"]);
}

#[test]
fn json_large_objects() {
    check(&["L09.json.large-objects"]);
}

/// `regex_state_prepare` returns `%string:int` (#752).
#[test]
fn regex_invalid_patterns() {
    check(&["L09.regex.invalid-patterns"]);
}

#[test]
fn regex_search_replace_split() {
    check(&["L09.regex.search-replace-split"]);
}

/// `environment_normalize` takes and returns `%string:string` (#752); an isolated snapshot never reads the
/// process environment.
#[test]
fn env_isolated_snapshot() {
    check(&["L10.env.isolated-snapshot"]);
}

/// The process environment is the case's (`env` of case.json, and `TZ=UTC` as the runner sets in every case).
#[test]
fn env_process_snapshot() {
    let id = "L10.env.process-snapshot";
    let dir = case_dir(id);
    let case: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("case.json")).unwrap()).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_okkama"));
    command.current_dir(&dir).env_clear().env("TZ", "UTC").env("OKKAMA_PIPELINE", "oir2");
    for (name, value) in case["env"].as_object().unwrap() {
        command.env(name, value.as_str().unwrap());
    }
    command.arg("run").arg("--stdlib").arg(root().join("std"));
    for source in case["sources"].as_array().unwrap() {
        command.arg(source.as_str().unwrap());
    }
    let output = command.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let expect = &case["expect"];
    assert_eq!(expect["outcome"], "success", "{id}: only successful cases");
    let status = expect["exit_status"].as_i64().unwrap_or(0) as i32;
    assert_eq!(output.status.code(), Some(status), "{id}: stderr={stderr:?}");
    assert_eq!(stdout, expect["stdout"]["text"].as_str().unwrap_or(""), "{id}: stderr={stderr:?}");
}
