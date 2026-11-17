//! #830 on Champ, in process: Atomic operations of plain values. `value`, `closed` and `close` run inline over the
//! cell, `replace` and `compare_and_replace` through the P1 boundary resolved at load; a closed cell's `value`
//! throws `AtomicClosed`. Each program runs under Structural and Full verification; the outputs are those of
//! Tomato built from the rebuild tree.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::run_source_with;
use okkama_compiler::oir2::VerifyMode;

fn expect(body: &str, stdout: &str) {
    let source = format!("module app v1.0.0;\n\nimport \"async/atomic\" take Atomic, AtomicClosed;\nimport \"io/stdout\" take Stdout;\n\n{body}");
    for mode in [VerifyMode::Structural, VerifyMode::Full] {
        let run = run_source_with(&source, mode).unwrap_or_else(|error| panic!("{mode:?}: {error}"));
        assert_eq!((run.stdout.as_str(), run.result), (stdout, Ok(0)), "{mode:?}");
    }
}

/// Reads, a replacement, both outcomes of a comparison, and the state after `close`.
#[test]
fn an_atomic_reads_replaces_and_closes() {
    expect(r#"main() is int {
    let out = Stdout();
    let counter = Atomic(1);
    counter.replace(counter.value() + 41);
    out.writeln(counter.value().string() + " " + counter.closed().string());
    let first = counter.compare_and_replace(expected: 42, revised: 7);
    let second = counter.compare_and_replace(expected: 42, revised: 8);
    out.writeln(first.string() + " " + second.string() + " " + counter.value().string());
    counter.close();
    counter.close();
    out.writeln(counter.closed().string());
    0;
}
"#, "42 false\ntrue false 7\ntrue\n");
}

/// `value` of a closed cell throws `AtomicClosed`, also of a cell of strings.
#[test]
fn a_closed_atomic_refuses_its_value() {
    expect(r#"main() is int {
    let out = Stdout();
    let name = Atomic("ada");
    out.writeln(name.value());
    name.close();
    out.writeln(try { name.value(); } catch e is AtomicClosed { "closed"; });
    0;
}
"#, "ada\nclosed\n");
}

/// A loop of reads and replacements counts exactly.
#[test]
fn a_loop_of_reads_and_replacements() {
    expect(r#"main() is int {
    let out = Stdout();
    let total = Atomic(0);
    for 0..1000 -> n { total.replace(total.value() + n); }
    out.writeln(total.value().string());
    0;
}
"#, "499500\n");
}
