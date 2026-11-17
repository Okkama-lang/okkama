//! #831 on Champ, in process: a `reduce` whose accumulator the block names only as `acc` or the left end of
//! `acc + parts` keeps it in a builder (#243's rule on OIR-2). The results are those of the copying form: the
//! initial value stays as it was, a branch that adds nothing adds nothing, a failure in the block leaves no
//! partial result, and an accumulator the block reads otherwise is copied as before. Each program runs under
//! Structural and Full verification; the outputs are those of Tomato built from the rebuild tree.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::run_source_with;
use okkama_compiler::oir2::VerifyMode;

fn expect(body: &str, stdout: &str) {
    let source = format!("module app v1.0.0;\n\nimport \"io/stdout\" take Stdout;\n\n{body}");
    for mode in [VerifyMode::Structural, VerifyMode::Full] {
        let run = run_source_with(&source, mode).unwrap_or_else(|error| panic!("{mode:?}: {error}"));
        assert_eq!((run.stdout.as_str(), run.result), (stdout, Ok(0)), "{mode:?}");
    }
}

/// Ten thousand appends, from an initial array that stays as it was.
#[test]
fn a_builder_accumulator_keeps_its_initial_value() {
    expect(r#"main() is int {
    let out = Stdout();
    let start = [7, 8];
    let grown = (0..10000).reduce(start) -> acc, n { acc + [n]; };
    out.writeln(start.size().string() + " " + grown.size().string());
    out.writeln(grown.item(0).string() + " " + grown.item(2).string() + " " + grown.item(10001).string());
    0;
}
"#, "2 10002\n7 0 9999\n");
}

/// A block that adds two elements in one branch and nothing in the other.
#[test]
fn branches_add_their_parts() {
    expect(r#"main() is int {
    let out = Stdout();
    let evens is @int = (1..=6).reduce([0]) -> acc, n { if n % 2 == 0 { acc + [n, n * 10]; } else { acc; } };
    out.writeln(evens.size().string() + " " + evens.item(0).string() + " " + evens.item(1).string());
    out.writeln(evens.item(2).string() + " " + evens.item(5).string() + " " + evens.item(6).string());
    0;
}
"#, "7 0 2\n20 6 60\n");
}

/// A map accumulator from `[:]` grows by literal entries; a later entry for a key replaces its value.
#[test]
fn a_map_accumulator_grows_by_literals() {
    expect(r#"main() is int {
    let out = Stdout();
    let words = ["b", "a", "b", "cc"];
    let index is %string:int = words.reduce([:]) -> acc, w { acc + [w: w.length()]; };
    out.writeln(index.keys().join(",") + " " + index.size().string());
    0;
}
"#, "a,b,cc 3\n");
}

/// An accumulator the block also reads (`acc.size()`) is a value that `+` copies.
#[test]
fn a_read_accumulator_is_copied() {
    expect(r#"main() is int {
    let out = Stdout();
    let sizes = (1..=3).reduce([0]) -> acc, n { acc + [acc.size()]; };
    out.writeln(sizes.size().string() + " " + sizes.item(1).string() + " " + sizes.item(3).string());
    0;
}
"#, "4 1 3\n");
}

/// Nested reduces each keep their own accumulator.
#[test]
fn nested_reduces_keep_their_own_accumulators() {
    expect(r#"main() is int {
    let out = Stdout();
    let nested = (1..=3).reduce([0]) -> outer, n { outer + (1..=n).reduce(@int()) -> inner, m { inner + [m]; }; };
    out.writeln(nested.size().string() + " " + nested.item(3).string());
    0;
}
"#, "7 2\n");
}

/// A failure in the block ends the reduce: the catch sees no partial array, and the initial value is intact.
#[test]
fn a_failure_in_the_block_leaves_no_partial_result() {
    expect(r#"enum Stop is Error = (stop);

object Gather {
    Gather();

    upto(limit is int, start is @int) is int {
        let all = (0..10).reduce(start) -> acc, n {
            confirm n < limit else throw Stop.stop;
            acc + [n];
        };
        all.size();
    }
}

main() is int {
    let out = Stdout();
    let start = [1];
    out.writeln(Gather().upto(limit: 20, start: start).string());
    out.writeln((try { Gather().upto(limit: 5, start: start); } catch e is Stop { -1; }).string());
    out.writeln(start.size().string());
    0;
}
"#, "11\n-1\n1\n");
}
