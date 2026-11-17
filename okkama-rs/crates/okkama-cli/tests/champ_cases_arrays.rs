//! #826 on Champ, in process: arrays take Tomato's persistent forms. A chain of appends or prepends shares a buffer
//! (#246, #247), a large slice is a view (#249), a chain of replacements becomes a trie (#250), and machine code's
//! `item` reads every form. Every version keeps its own elements. Each program runs under Structural and Full
//! verification; the outputs are those of Tomato built from the rebuild tree.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::run_source_with;
use okkama_compiler::oir2::VerifyMode;

fn run(body: &str) -> Vec<(VerifyMode, champ_support::Run)> {
    let source = format!("module app v1.0.0;\n\nimport \"io/stdout\" take Stdout;\n\n{body}");
    [VerifyMode::Structural, VerifyMode::Full].into_iter()
        .map(|mode| (mode, run_source_with(&source, mode).unwrap_or_else(|error| panic!("{mode:?}: {error}"))))
        .collect()
}

fn expect(body: &str, stdout: &str) {
    for (mode, run) in run(body) {
        assert_eq!((run.stdout.as_str(), run.result), (stdout, Ok(0)), "{mode:?}");
    }
}

/// An array built by a thousand appends in a `reduce` reads back whole through `item`.
#[test]
fn a_long_append_chain_reads_back_whole() {
    expect(r#"main() is int {
    let out = Stdout();
    let built = (0..1000).reduce(@int()) -> acc, n { acc + [n]; };
    let total = (0..built.size()).reduce(0) -> sum, at { sum + built.item(at); };
    out.writeln(built.size().string() + " " + total.string());
    out.writeln(built.item(0).string() + " " + built.item(500).string() + " " + built.item(999).string());
    0;
}
"#, "1000 499500\n0 500 999\n");
}

/// Two appends to the same version give two arrays, and the version itself does not change: the second append,
/// which no longer ends at the buffer's frontier, copies.
#[test]
fn appends_to_one_version_branch() {
    expect(r#"main() is int {
    let out = Stdout();
    let base = (0..40).reduce(@int()) -> acc, n { acc + [n]; };
    let left = base + [100];
    let right = base + [200];
    let longer = left + [101];
    out.writeln(base.size().string() + " " + left.item(40).string() + " " + right.item(40).string());
    out.writeln(longer.item(40).string() + " " + longer.item(41).string() + " " + right.size().string());
    0;
}
"#, "40 100 200\n100 101 41\n");
}

/// A chain of prepends and an append to its result.
#[test]
fn a_prepend_chain_reads_back_in_order() {
    expect(r#"main() is int {
    let out = Stdout();
    let front = (0..50).reduce(@int()) -> acc, n { [n] + acc; };
    let both = front + [-1];
    out.writeln(front.size().string() + " " + front.item(0).string() + " " + front.item(49).string());
    out.writeln(both.item(50).string() + " " + both.size().string());
    0;
}
"#, "50 49 0\n-1 51\n");
}

/// A chain of replacements of a hundred elements (a trie) keeps its source.
#[test]
fn replacements_keep_their_source() {
    expect(r#"main() is int {
    let out = Stdout();
    let source = (0..100).reduce(@int()) -> acc, n { acc + [n]; };
    let changed = (0..100).reduce(source) -> acc, at { acc.replaced(at, at * 2); };
    let total = (0..changed.size()).reduce(0) -> sum, at { sum + changed.item(at); };
    out.writeln(total.string() + " " + source.item(99).string() + " " + changed.item(99).string());
    out.writeln(changed.removed_at(0).item(0).string() + " " + changed.removed_at(0).size().string());
    0;
}
"#, "9900 99 198\n2 99\n");
}

/// A large slice and a chain of `drop` read the elements of the array they come from.
#[test]
fn slices_read_their_source() {
    expect(r#"main() is int {
    let out = Stdout();
    let source = (0..100).reduce(@int()) -> acc, n { acc + [n]; };
    let middle = source.slice(10, 90);
    let fewer = source.drop(1).drop(1).take(50);
    out.writeln(middle.size().string() + " " + middle.item(0).string() + " " + middle.item(79).string());
    out.writeln(fewer.item(0).string() + " " + fewer.item(49).string() + " " + fewer.size().string());
    0;
}
"#, "80 10 89\n2 51 50\n");
}

/// `item` past the end of a view fails, though the buffer behind it holds more.
#[test]
fn an_index_past_a_view_fails() {
    for (mode, run) in run(r#"main() is int {
    let out = Stdout();
    let base = (0..40).reduce(@int()) -> acc, n { acc + [n]; };
    let shorter = base + [40];
    let longer = shorter + [41];
    out.writeln(shorter.size().string() + " " + longer.size().string());
    out.writeln(shorter.item(41).string());
    0;
}
"#) {
        assert_eq!(run.stdout, "41 42\n", "{mode:?}");
        let report = run.result.expect_err("a failure");
        assert!(report.contains("Array index out of bounds"), "{mode:?}: {report}");
    }
}
