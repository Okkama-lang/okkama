//! Natives on Champ, in process (#569 C7): the runtime table's catalogue, handles of the native
//! registry, the unresolved-provider adapter and Atomic/Cell storage.
//!
//! Only pure language plus natives: no processes, workers, network, outside files or test mode.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{check, run_source};

#[test]
#[ignore = "a measurement over a case list: CHAMP_CASES=<file>"]
fn champ_natives_list() {
    champ_support::measure();
}

/// Conformance cases that are pure language plus natives.
#[test]
fn champ_cases_natives() {
    check(&["L06.native.missing-provider", "L09.unicode.normalization"]);
}

fn expect_output(source: &str, stdout: &str) {
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.result, Ok(0), "stdout={:?}", run.stdout);
    assert_eq!(run.stdout, stdout);
}

/// Data-only natives of the table bind to Tomato's providers.
#[test]
fn champ_natives_data_transport() {
    expect_output(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "import \"runtime/utf\" take utf8_encode, utf8_decode;\n",
        "import \"runtime/hex\" take hex_encode, hex_valid_prefix, hex_decode;\n",
        "main() is int {\n",
        "    let out = Stdout();\n",
        "    out.writeln(hex_encode(utf8_encode(\"Ok\")));\n",
        "    out.writeln(hex_valid_prefix(\"0x12\").string());\n",
        "    out.writeln(utf8_decode(hex_decode(\"4F6b\")));\n",
        "    out.writeln(hex_decode(\"00ABff\").size().string());\n",
        "    0;\n",
        "}\n",
    ), "4f6b\n1\nOk\n3\n");
}

/// A handle is published by the provider and used, closed and observed through the registry.
#[test]
fn champ_natives_handles() {
    expect_output(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "import \"runtime/utf\" take utf8_encode;\n",
        "import \"runtime/buffer\" take BufferState, buffer_state_create, buffer_state_append, ",
        "buffer_state_size, buffer_state_read, buffer_state_closed, buffer_state_close;\n",
        "main() is int {\n",
        "    let out = Stdout();\n",
        "    let state = buffer_state_create(utf8_encode(\"ab\"));\n",
        "    let appended = buffer_state_append(state, utf8_encode(\"c\"));\n",
        "    out.writeln(buffer_state_size(state).string());\n",
        "    out.writeln(buffer_state_read(state, 2).size().string());\n",
        "    out.writeln(buffer_state_size(state).string());\n",
        "    out.writeln(buffer_state_closed(state).string());\n",
        "    out.writeln(buffer_state_close(state).string());\n",
        "    out.writeln(buffer_state_closed(state).string());\n",
        "    0;\n",
        "}\n",
    ), "3\n2\n1\nfalse\ntrue\ntrue\n");
}

/// A language wrapper with `destroy` is an ordinary object whose destroy calls the native close.
#[test]
fn champ_natives_wrapper_destroy_closes_the_handle() {
    expect_output(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "import \"runtime/utf\" take utf8_encode;\n",
        "import \"runtime/buffer\" take BufferState, buffer_state_create, buffer_state_size, ",
        "buffer_state_close;\n",
        "object Held {\n",
        "    has state is BufferState;\n",
        "    Held(:state);\n",
        "    size() is int { buffer_state_size(state); }\n",
        "    destroy {\n",
        "        Stdout().writeln(\"close \" + buffer_state_close(state).string());\n",
        "    }\n",
        "}\n",
        "object Probe {\n",
        "    Probe();\n",
        "    size() is int {\n",
        "        let held = Held(state: buffer_state_create(utf8_encode(\"xyz\")));\n",
        "        held.size();\n",
        "    }\n",
        "}\n",
        "main() is int {\n",
        "    Stdout().writeln(Probe().size().string());\n",
        "    0;\n",
        "}\n",
    ), "close true\n3\n");
}

/// A provider refusal is a NativeFailure; uncaught, it ends the run after earlier effects.
#[test]
fn champ_natives_provider_failure() {
    let run = run_source(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "import \"runtime/hex\" take hex_decode;\n",
        "main() is int {\n",
        "    Stdout().writeln(\"before\");\n",
        "    Stdout().writeln(hex_decode(\"0x\").size().string());\n",
        "    0;\n",
        "}\n",
    )).unwrap();
    assert_eq!(run.stdout, "before\n");
    assert!(run.result.is_err());
}

/// A declared native without a provider fails at the call, after earlier effects, also with handles.
#[test]
fn champ_natives_missing_provider_with_handles() {
    let run = run_source(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "native object Port;\n",
        "native port_open(name is string) is Port;\n",
        "main() is int {\n",
        "    Stdout().writeln(\"before\");\n",
        "    let port = port_open(\"x\");\n",
        "    Stdout().writeln(\"after\");\n",
        "    0;\n",
        "}\n",
    )).unwrap();
    assert_eq!(run.stdout, "before\n");
    let error = run.result.unwrap_err();
    assert!(error.contains("Native symbol \"app@1::port_open/1\" is not registered"), "{error}");
}

/// Registered Atomic operations for every plain scalar specialization (P2.4 §4.8), as Tomato does them.
#[test]
fn champ_atomic_plain_specializations() {
    expect_output(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "import \"async/atomic\" take Atomic, AtomicClosed;\n",
        "main() is int {\n",
        "    let out = Stdout();\n",
        "    let flag = Atomic(false);\n",
        "    out.writeln(flag.compare_and_replace(false, true).string());\n",
        "    out.writeln(flag.compare_and_replace(false, true).string());\n",
        "    out.writeln(flag.value().string());\n",
        "    let ratio = Atomic(0.5);\n",
        "    ratio.replace(1.5);\n",
        "    out.writeln((ratio.value() == 1.5).string());\n",
        "    let letter = Atomic('a');\n",
        "    out.writeln(letter.compare_and_replace('b', 'c').string());\n",
        "    out.writeln((letter.value() == 'a').string());\n",
        "    let small = Atomic(byte(7));\n",
        "    out.writeln((small.value() == byte(7)).string());\n",
        "    let count = Atomic(1);\n",
        "    out.writeln(count.compare_and_replace(1, 2).string());\n",
        "    out.writeln(count.closed().string());\n",
        "    count.close();\n",
        "    count.close();\n",
        "    out.writeln(count.closed().string());\n",
        "    let probe = try { count.compare_and_replace(2, 3).string(); } catch e is AtomicClosed { \"closed\"; };\n",
        "    out.writeln(probe);\n",
        "    0;\n",
        "}\n",
    ), "true\nfalse\ntrue\ntrue\nfalse\ntrue\ntrue\ntrue\nfalse\ntrue\nclosed\n");
}

/// Cell is an ordinary std object over the registered Atomic of its specialization.
#[test]
#[ignore = "Cell construction fails the SPB 17 FRESH_PACKET load rule for every specialization, int included \
    (#671)"]
fn champ_cell_plain_specializations() {
    expect_output(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "import \"cell\" take Cell;\n",
        "main() is int {\n",
        "    let seen = Cell(false);\n",
        "    seen.replace(true);\n",
        "    let total = Cell(0.25);\n",
        "    total.replace(0.75);\n",
        "    Stdout().writeln(seen.value().string() + \" \" + (total.value() == 0.75).string());\n",
        "    0;\n",
        "}\n",
    ), "true true\n");
}

/// A plain array specialization compares by value; a failed comparison publishes nothing.
#[test]
fn champ_atomic_plain_array_specialization() {
    expect_output(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "import \"async/atomic\" take Atomic;\n",
        "main() is int {\n",
        "    let out = Stdout();\n",
        "    let ledger = Atomic([\"a\"]);\n",
        "    out.writeln(ledger.compare_and_replace([\"b\"], [\"x\"]).string());\n",
        "    out.writeln(ledger.value().size().string() + ledger.value()[0]);\n",
        "    out.writeln(ledger.compare_and_replace([\"a\"], [\"a\", \"b\"]).string());\n",
        "    out.writeln(ledger.value().size().string() + ledger.value()[1]);\n",
        "    0;\n",
        "}\n",
    ), "false\n1a\ntrue\n2b\n");
}
