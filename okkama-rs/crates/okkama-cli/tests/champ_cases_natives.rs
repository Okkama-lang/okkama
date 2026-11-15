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
    // Its std catches the provider's NativeFailure (#673).
    check(&["L10.random.secure-invariants"]);
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

/// `catch failure is NativeFailure` binds the provider's failure, whose getters read its fields (#673,
/// language spec 9.5). The expected stdout is Tomato's for this program.
#[test]
fn champ_natives_native_failure_is_caught() {
    expect_output(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "import \"runtime/json\" take json_parse_state, json_parse_root, json_node_text;\n",
        "object Probe {\n",
        "    Probe();\n",
        "    text(source is string) is string {\n",
        "        let root = json_parse_root(json_parse_state(source, 64));\n",
        "        try {\n",
        "            json_node_text(root);\n",
        "        }\n",
        "        catch failure is NativeFailure {\n",
        "            Stdout().writeln(failure.domain() + \"/\" + failure.code() + \"/\" + failure.operation());\n",
        "            Stdout().writeln((failure.text() == failure.message()).string());\n",
        "            \"not text\";\n",
        "        }\n",
        "    }\n",
        "}\n",
        "main() is int {\n",
        "    let out = Stdout();\n",
        "    out.writeln(Probe().text(\"\\\"ok\\\"\"));\n",
        "    out.writeln(Probe().text(\"1\"));\n",
        "    0;\n",
        "}\n",
    ), "ok\ncore.json/invalid_input/runtime/json@1::json_node_text/1\ntrue\nnot text\n");
}

/// The provider's NativeFailure is an Error too: `catch e is Error` takes it and `e.text()` is its message, as on
/// Tomato, where this program prints the same line (#738, language spec 9.5).
#[test]
fn champ_natives_native_failure_is_caught_as_error() {
    expect_output(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "import \"runtime/json\" take json_parse_state, json_parse_root, json_node_text;\n",
        "main() is int {\n",
        "    let root = json_parse_root(json_parse_state(\"1\", 64));\n",
        "    let text = try {\n",
        "        json_node_text(root);\n",
        "    } catch e is Error {\n",
        "        \"error: \" + e.text();\n",
        "    };\n",
        "    Stdout().writeln(text);\n",
        "    0;\n",
        "}\n",
    ), "error: JSON node has the wrong kind\n");
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

/// A missing provider is a program failure of the call, not the native's own failure: `catch e is NativeFailure`
/// does not take it, and the run ends with Tomato's report (#719, language spec 9.5, L06.native.missing-provider).
/// Tomato (`okkama run`) prints `RuntimeError: Native symbol "app@1::conformance_unlinked_symbol/1" is not
/// registered` and exits 1.
#[test]
fn champ_natives_missing_provider_passes_a_native_failure_catch() {
    let run = run_source(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "native conformance_unlinked_symbol(x is int) is int;\n",
        "main() is int {\n",
        "    let text = try {\n",
        "        conformance_unlinked_symbol(1).string();\n",
        "    } catch e is NativeFailure {\n",
        "        \"caught: \" + e.message();\n",
        "    };\n",
        "    Stdout().writeln(text);\n",
        "    0;\n",
        "}\n",
    )).unwrap();
    assert_eq!(run.stdout, "");
    let error = run.result.unwrap_err();
    assert!(error.starts_with("RuntimeError: Native symbol \"app@1::conformance_unlinked_symbol/1\" is not registered"),
        "{error}");
}

/// A native the embedder's groups refuse fails with a program failure of the call that no catch takes, as on Tomato
/// (`natives::call` is shared): `catch e is NativeFailure` and `catch e is Error` let it pass (#719).
#[test]
fn champ_natives_permission_refusal_passes_every_catch() {
    let source = concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "import \"runtime/time\" take monotonic_clock;\n",
        "main() is int {\n",
        "    Stdout().writeln(\"before\");\n",
        "    let text = try {\n",
        "        monotonic_clock().string();\n",
        "    } catch e is NativeFailure {\n",
        "        \"native: \" + e.message();\n",
        "    } catch e is Error {\n",
        "        \"error: \" + e.text();\n",
        "    };\n",
        "    Stdout().writeln(text);\n",
        "    0;\n",
        "}\n",
    );
    let name = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/champ_support/inline.ok");
    let file = okkama_compiler::SourceFile::new(name.to_string_lossy(), source.to_string()).unwrap();
    let bytes = champ_support::compile(&[file]).unwrap();
    // Without restriction the clock is read; with every group refused the call fails uncaught.
    let open = sparrow_core::run_captured(bytes.clone(), sparrow_core::budget::Limits::default()).unwrap();
    assert_eq!(open.result.map_err(|error| error.0), Ok(0));
    let limits = sparrow_core::budget::Limits { natives: Some(vec![]), ..sparrow_core::budget::Limits::default() };
    let refused = sparrow_core::run_captured(bytes, limits).unwrap();
    assert_eq!(String::from_utf8_lossy(&refused.stdout), "before\n");
    let error = refused.result.unwrap_err().0;
    assert!(error.starts_with("RuntimeError: Native runtime/time@1::monotonic_clock/0 needs the Clock permission"),
        "{error}");
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

/// An object with an Atomic field is constructed, used and destroyed (P2.4 section 4.8, #686): the field is
/// a capability without an obligation of its own, so construction creates only the object's identity.
#[test]
fn champ_object_with_atomic_field_lifecycle() {
    expect_output(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "import \"async/atomic\" take Atomic;\n",
        "object Served {\n",
        "    has count is Atomic<int>;\n",
        "    Served() {\n",
        "        count = Atomic(0);\n",
        "    }\n",
        "    next() is int {\n",
        "        let at = count.value();\n",
        "        count.replace(at + 1);\n",
        "        at;\n",
        "    }\n",
        "    destroy {\n",
        "        Stdout().writeln(\"destroy \" + count.value().string());\n",
        "        count.close();\n",
        "    }\n",
        "}\n",
        "object Recorded {\n",
        "    has served is Atomic<int>;\n",
        "    has label is string;\n",
        "    Recorded(label is string) {\n",
        "        self.label = label;\n",
        "        served = Atomic(0);\n",
        "    }\n",
        "    batch() is string {\n",
        "        let at = served.value();\n",
        "        served.replace(at + 1);\n",
        "        label + at.string();\n",
        "    }\n",
        "}\n",
        "object Probe {\n",
        "    Probe();\n",
        "    run() is int {\n",
        "        let served = Served();\n",
        "        served.next();\n",
        "        served.next();\n",
        "        served.next();\n",
        "    }\n",
        "    record() is string {\n",
        "        let recorded = Recorded(\"batch \");\n",
        "        recorded.batch();\n",
        "        recorded.batch();\n",
        "    }\n",
        "}\n",
        "main() is int {\n",
        "    let out = Stdout();\n",
        "    out.writeln(Probe().run().string());\n",
        "    out.writeln(Probe().record());\n",
        "    0;\n",
        "}\n",
    ), "destroy 3\n2\nbatch 1\n");
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
