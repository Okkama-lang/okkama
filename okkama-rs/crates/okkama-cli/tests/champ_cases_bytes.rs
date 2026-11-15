//! Byte arrays on Champ (#703): `array.get` of an `@byte` value (`bytes[i]`), with the cases that reached it
//! through std (encodings, PCG32, UUIDs, standard input). Each case is compiled through OIR-2, run in
//! Champ and compared with its case.json; the standard input cases run the `okkama` binary with the case's
//! stdin fixture.
//!
//! Only pure language cases and standard input: no processes, workers, network or test mode.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{case_dir, check, root, run_source};
use serde_json::Value;
use std::process::{Command, Stdio};

/// `bytes[i]` in the case itself (L05.array.replaced) and in std: hex, quoted-printable and UTF codecs,
/// PCG32 bytes, UUID bytes. The codec cases also catch one-case Error variants with a payload
/// (`catch InvalidHex.at(offset)`), and the UUID case offers a role value to an owning call.
#[test]
fn champ_cases_byte_array_get() {
    check(&[
        "L05.array.replaced",
        "L09.encoding.hex",
        "L09.encoding.quoted-printable",
        "L09.encoding.utf-codecs",
        "L09.random.pcg32-reproducible",
        "L09.uuid.values",
    ]);
}

/// Runs a run-mode case that reads standard input with the `okkama` binary on Champ, feeding its stdin
/// fixture, and compares stdout and the exit status with case.json.
fn run_stdin_case(id: &str) -> Result<(), String> {
    let dir = case_dir(id);
    let case: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("case.json")).unwrap()).unwrap();
    assert_eq!(case["mode"], "run", "{id}: only run-mode cases");
    let input = std::fs::File::open(dir.join(case["stdin"].as_str().unwrap())).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_okkama"));
    command.current_dir(&dir).env("OKKAMA_PIPELINE", "oir2").arg("run").arg("--stdlib").arg(root().join("std"));
    for source in case["sources"].as_array().unwrap() {
        command.arg(source.as_str().unwrap());
    }
    let output = command.arg("--").stdin(Stdio::from(input)).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let expect = &case["expect"];
    assert_eq!(expect["outcome"], "success", "{id}: only successful cases");
    let status = expect["exit_status"].as_i64().unwrap_or(0) as i32;
    if output.status.code() != Some(status) {
        return Err(format!("exit {:?}, expected {status}; stderr={stderr:?}", output.status.code()));
    }
    let expected = expect["stdout"]["text"].as_str().unwrap_or("");
    if stdout != expected {
        return Err(format!("stdout {stdout:?}, expected {expected:?}; stderr={stderr:?}"));
    }
    Ok(())
}

/// Standard input reads its buffer bytes by index (`io/stdin`).
#[test]
fn champ_cases_stdin_byte_buffers() {
    let failures = ["L10.stdin.chunks", "L10.stdin.for-lines", "L10.stdin.lines"].iter()
        .filter_map(|id| run_stdin_case(id).err().map(|error| format!("{id}: {error}")))
        .collect::<Vec<_>>();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Every element of a byte array reads back as its byte; an index outside it (past the end, negative,
/// beyond 64 bits) is a program failure with Tomato's text, which no `catch` of an Error intercepts.
#[test]
fn champ_byte_array_get_reads_every_element_and_checks_bounds() {
    let program = |index: &str| format!(r#"module app v1.0.0;

import "io/stdout" take Stdout;

object Probe {{
    has bytes is @byte;
    Probe(:bytes);
    at(index is int) is string {{
        try {{ bytes[index].int().string(); }} catch e is Error {{ e.text(); }};
    }}
}}

main() is int {{
    let probe = Probe(bytes: [byte(0), byte(128), byte(255)]);
    Stdout().writeln(probe.at(0) + " " + probe.at(1) + " " + probe.at(2));
    Stdout().writeln(probe.at({index}));
    0;
}}
"#);
    let run = run_source(&program("2")).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.result, Ok(0), "stdout={:?}", run.stdout);
    assert_eq!(run.stdout, "0 128 255\n255\n");
    for index in ["3", "-1", "99999999999999999999"] {
        let run = run_source(&program(index)).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(run.stdout, "0 128 255\n", "{index}");
        let report = run.result.expect_err(index);
        assert!(report.contains("Array index out of bounds"), "{index}: {report}");
    }
}

/// A one-case Error variant with a payload is its case object: a catch matches its case and binds the
/// payload.
#[test]
fn champ_one_case_error_variant_payload_is_caught() {
    let source = r#"module app v1.0.0;

import "io/stdout" take Stdout;

enum Bad is Error = (at(offset is int));

object Probe {
    Probe();
    check(n is int) is int {
        confirm n > 0 else throw Bad.at(n);
        n;
    }
}

main() is int {
    let probe = Probe();
    let caught = try { probe.check(-4).string(); } catch Bad.at(offset) { "offset " + offset.string(); };
    let passed = try { probe.check(5).string(); } catch Bad.at(offset) { "offset " + offset.string(); };
    Stdout().writeln(caught + " " + passed);
    0;
}
"#;
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.result, Ok(0), "stdout={:?}", run.stdout);
    assert_eq!(run.stdout, "offset -4 5\n");
}
