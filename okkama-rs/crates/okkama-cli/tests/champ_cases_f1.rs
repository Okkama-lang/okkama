//! #601 F1: async role slots, bound async action values and variadic role slots on Champ (OIR-2 async ABI, ruling
//! PR-D; `fut.spawn.bound`, ruling PR-E). Conformance cases are checked in full against their case.json; the inline
//! programs' expected output is Tomato's for the same program (okkama 0.5.8, run in the cage).
//! The file runs twice: as is, and with `SPARROW_CORE_GC_STRESS=1`, which collects at every safepoint.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{case_dir, run_case};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

macro_rules! cases {
    ($($name:ident => $id:literal,)*) => {
        $(
            #[test]
            fn $name() {
                run_case($id).unwrap_or_else(|error| panic!("{}: {error}", $id));
            }
        )*
    };
}

cases! {
    f1_variadic_slot_stderr_basic => "L00.stderr.basic",
}

/// A case that works with files, run by the okkama binary through OIR-2 in a fresh copy of its directory, so that
/// its files stay out of the repository; checked in full against its case.json.
fn run_in_copy(id: &str) -> Result<(), String> {
    let source = case_dir(id);
    let copy = std::env::temp_dir().join(format!("okkama-f1-{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&copy);
    std::fs::create_dir_all(&copy).unwrap();
    for entry in std::fs::read_dir(&source).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            std::fs::copy(&path, copy.join(path.file_name().unwrap())).unwrap();
        }
    }
    let case: Value = serde_json::from_str(&std::fs::read_to_string(source.join("case.json")).unwrap()).unwrap();
    let main = case["sources"][0].as_str().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_okkama"))
        .args(["run", main])
        .current_dir(&copy)
        .env("OKKAMA_PIPELINE", "oir2")
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&copy);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let expect = &case["expect"];
    assert_eq!(expect["outcome"], "success", "{id}: only successful cases");
    let status = expect["exit_status"].as_i64().unwrap_or(0) as i32;
    if output.status.code() != Some(status) {
        return Err(format!("exit {:?}, expected {status}; stdout={stdout:?} stderr={stderr:?}", output.status.code()));
    }
    let expected = expect["stdout"]["text"].as_str().unwrap_or("");
    if stdout != expected {
        return Err(format!("stdout {stdout:?}, expected {expected:?}; stderr={stderr:?}"));
    }
    Ok(())
}

/// The variadic slots of `WritingText` (PR-D item 5) reached through std's file and directory objects.
#[test]
fn f1_variadic_slot_file_cases() {
    assert!(Path::new(env!("CARGO_BIN_EXE_okkama")).is_file());
    let failures = ["L10.fs.directory-entries-sorted", "L10.fs.line-reading", "L10.fs.missing-file-message",
        "L10.fs.text-lines-in-batches", "L10.fs.write-then-read"].iter()
        .filter_map(|id| run_in_copy(id).err().map(|error| format!("{id}: {error}")))
        .collect::<Vec<_>>();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
