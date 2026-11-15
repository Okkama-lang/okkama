//! `ordering()` on the OIR-2 → SPB 17 → Champ path, in process: Tomato's stable ascending order, an
//! O(n log n) sort (#678) and a failing `order()` that stops it.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{compile, run_case, Run};
use okkama_compiler::SourceFile;
use std::path::Path;
use std::time::{Duration, Instant};

/// SPB 17 of an inline module.
fn compile_inline(text: &str) -> Vec<u8> {
    let name = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/champ_support/inline.ok");
    let source = SourceFile::new(name.to_string_lossy(), text.to_string()).unwrap();
    compile(&[source]).unwrap_or_else(|error| panic!("{error}"))
}

/// Runs SPB 17 in Champ without a step limit; the caller bounds the run's wall time.
fn run_unlimited(bytes: Vec<u8>) -> Run {
    let run = sparrow_core::run_captured(bytes, sparrow_core::budget::Limits::default())
        .unwrap_or_else(|error| panic!("load: {error}"));
    Run {
        stdout: String::from_utf8_lossy(&run.stdout).into_owned(),
        result: run.result.map_err(|error| error.0),
    }
}

fn run_unbounded(text: &str) -> Run {
    run_unlimited(compile_inline(text))
}

/// 10⁵ elements sort within a small wall-time budget: an O(n²) sort makes about 5·10⁹ element moves here.
#[test]
fn sorts_a_hundred_thousand_ints_quickly() {
    let source = r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "compare" take Comparing;
main() is int {
    let values is @int = (0..100000).map() -> i { (i * 7919 + 13) % 1000003; };
    let sorted = values.ordering();
    let out = Stdout();
    out.writeln(sorted.size().string() + " " + sorted[0].string() + " " + sorted[99999].string());
    let ordered = (1..100000).all() -> i { sorted[i - 1] <= sorted[i]; };
    out.writeln(ordered.string());
    out.writeln(values[0].string() + " " + values[99999].string());
    0;
}
"#;
    let bytes = compile_inline(source);
    let started = Instant::now();
    let run = run_unlimited(bytes);
    let elapsed = started.elapsed();
    assert_eq!(run.result, Ok(0), "stdout={:?}", run.stdout);
    // The least and greatest of (i * 7919 + 13) % 1000003 for i < 100000 (L09.compare provenance);
    // the source array is unchanged.
    assert_eq!(run.stdout, "100000 1 999972\ntrue\n13 889721\n");
    // An O(n log n) sort runs this in about 30 s in an unoptimized build on 4 cores; the insertion sort
    // ran on Champ for more than 7 minutes (#678).
    let budget = Duration::from_secs(if cfg!(debug_assertions) { 120 } else { 15 });
    assert!(elapsed < budget, "the run took {elapsed:?}");
    println!("10^5 sort run: {elapsed:?}");
}

/// Edge sizes, duplicates and every scalar kind keep Tomato's ascending order.
#[test]
fn sorts_edge_sizes_and_scalar_kinds() {
    let ints = |list: &str| format!("{list}.ordering().map() -> v {{ v.string(); }}.join(\",\")");
    let cases = [
        (&*format!("let empty is @int = [];\n    out.writeln(\"[\" + {} + \"]\");", ints("empty")), "[]"),
        (&*format!("out.writeln({});", ints("[7]")), "7"),
        (&*format!("out.writeln({});", ints("[2, 1]")), "1,2"),
        (&*format!("out.writeln({});", ints("[1, 2, 3, 4, 5]")), "1,2,3,4,5"),
        (&*format!("out.writeln({});", ints("[5, 4, 3, 2, 1]")), "1,2,3,4,5"),
        (&*format!("out.writeln({});", ints("[3, 1, 3, 2, 1, 3, 0]")), "0,1,1,2,3,3,3"),
        (&*format!("out.writeln({});", ints("[9, -4, 7, 0, -4, 12, 5, 3, 8]")), "-4,-4,0,3,5,7,8,9,12"),
        ("out.writeln([\"pear\", \"apple\", \"fig\", \"apple\", \"banana\"].ordering().join(\",\"));",
            "apple,apple,banana,fig,pear"),
        ("out.writeln([2.5, -1.0, 0.5].ordering().map() -> f { f.string(); }.join(\",\"));", "-1,0.5,2.5"),
    ];
    for (statement, expected) in cases {
        let header = "module app v1.0.0;\nimport \"io/stdout\" take Stdout;\nimport \"compare\" take Comparing;\n";
        let source = format!("{header}main() is int {{\n    let out = Stdout();\n    {statement}\n    0;\n}}\n");
        let run = run_unbounded(&source);
        assert_eq!(run.result, Ok(0), "{statement}: stdout={:?}", run.stdout);
        assert_eq!(run.stdout, format!("{expected}\n"), "{statement}");
    }
}

/// A failing `order()` stops the sort with its error, and a later sort of 10⁵ ints still works within the
/// case's budget: checked in full against its case.json.
#[test]
fn failing_order_stops_the_sort() {
    let id = "L09.compare.failing-order-stops-the-sort";
    run_case(id).unwrap_or_else(|error| panic!("{id}: {error}"));
}

/// Elements ordered by `Comparing<T>.order` sort by Tomato's bottom-up merge: every comparison calls `order`
/// through role dispatch, so the effects inside `order` come in Tomato's order, equal ranks keep their order,
/// and the source array is unchanged (#678).
#[test]
fn role_elements_merge_with_tomatos_comparisons() {
    let source = r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "compare" take Comparing, Order;
object Card does Comparing<Card> {
    has rank is int;
    has tag is string;
    Card(:rank, :tag);
    rank() is int { rank; }
    tag() is string { tag; }
    order(other is Card) is Order {
        Stdout().writeln(tag + "?" + other.tag());
        rank.order(other.rank());
    }
}
main() is int {
    let cards is @Card = [Card(rank: 5, tag: "a"), Card(rank: 3, tag: "b"), Card(rank: 8, tag: "c"),
        Card(rank: 3, tag: "d"), Card(rank: 1, tag: "e"), Card(rank: 9, tag: "f"), Card(rank: 5, tag: "g")];
    let sorted = cards.ordering();
    Stdout().writeln(sorted.map() -> c { c.tag(); }.join(","));
    Stdout().writeln(cards.map() -> c { c.tag(); }.join(","));
    0;
}
"#;
    let run = run_unbounded(source);
    assert_eq!(run.result, Ok(0), "stdout={:?}", run.stdout);
    // Tomato's output for this program (`okkama run` on a default build), which the merge below derives.
    let tomato = "a?b\nc?d\ne?f\nb?d\na?d\na?c\ne?g\nf?g\nb?e\nb?g\nd?g\na?g\nc?g\nc?f\ne,b,d,a,g,c,f\na,b,c,d,e,f,g\n";
    assert_eq!(run.stdout, tomato);
    assert_eq!(merge_oracle(&[(5, "a"), (3, "b"), (8, "c"), (3, "d"), (1, "e"), (9, "f"), (5, "g")]), tomato);
}

/// The bottom-up merge of Tomato's `ordering`: the trace of `left?right` per comparison, then the sorted tags
/// and the source tags.
fn merge_oracle(cards: &[(i64, &str)]) -> String {
    let mut trace = String::new();
    let mut items = cards.to_vec();
    let mut width = 1;
    while width < items.len() {
        let mut merged = Vec::new();
        let mut left = 0;
        while left < items.len() {
            let mid = (left + width).min(items.len());
            let end = (mid + width).min(items.len());
            let (mut i, mut j) = (left, mid);
            while i < mid && j < end {
                trace.push_str(&format!("{}?{}\n", items[i].1, items[j].1));
                if items[i].0 > items[j].0 {
                    merged.push(items[j]);
                    j += 1;
                } else {
                    merged.push(items[i]);
                    i += 1;
                }
            }
            merged.extend_from_slice(&items[i..mid]);
            merged.extend_from_slice(&items[j..end]);
            left = end;
        }
        items = merged;
        width *= 2;
    }
    let tags = |list: &[(i64, &str)]| list.iter().map(|card| card.1).collect::<Vec<_>>().join(",");
    format!("{trace}{}\n{}\n", tags(&items), tags(cards))
}

/// Elements that own a resource sort too: each comparison borrows them, the sorted array shares them with the
/// source, and each is destroyed once when the scope ends, as in Tomato (#678).
#[test]
fn resource_elements_are_borrowed_by_the_comparisons() {
    let source = r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "compare" take Comparing, Order;
object Ranked does Comparing<Ranked> {
    has n is int;
    Ranked(:n);
    n() is int { n; }
    order(other is Ranked) is Order {
        Stdout().writeln(n.string() + "?" + other.n().string());
        n.order(other.n());
    }
    destroy { Stdout().writeln("destroy " + n.string()); }
}
main() is int {
    let ranked = [Ranked(n: 3), Ranked(n: 1), Ranked(n: 2)];
    let sorted = ranked.ordering();
    Stdout().writeln(sorted.map() -> r { r.n().string(); }.join(","));
    0;
}
"#;
    let run = run_unbounded(source);
    assert_eq!(run.result, Ok(0), "stdout={:?}", run.stdout);
    // Tomato's output for this program (`okkama run` on a default build).
    assert_eq!(run.stdout, "3?1\n1?2\n3?2\n1,2,3\ndestroy 2\ndestroy 1\ndestroy 3\n");
}
