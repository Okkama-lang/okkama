//! Builtin values behind roles on Champ (#757): arrays as `Sequencing<T>`, `Iterating<T>` and `Sizing` through
//! adapter objects (Tomato's `ArraySequence<S, T>`), nested arrays as roles of roles, object elements wrapped into
//! a role element, and calls on roles that nothing witnesses. Conformance cases are compared with their
//! case.json; inline programs print what Tomato prints.
//!
//! Only synchronous cases: no processes, workers, network or outside files.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{check, run_source};

/// The nine cases of #757 that run synchronously; `L08.collect.race-empty` is async (an array of futures
/// behind `Sequencing`) and stays with the async lane.
#[test]
fn champ_cases_roles_builtin() {
    check(&[
        "L05.boundary.nested-array-as-role-of-roles",
        "L09.toml.document-visit",
        "L09.toml.rejections",
        "L09.url.percent-encoding",
        "L09.url.reference-resolution",
        "L09.url.query-parameters",
        "L10.args.utf8-arguments",
        "L12.x509-path-search-budget",
    ]);
}

fn expect_output(source: &str, stdout: &str) {
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.result, Ok(0), "stdout={:?}", run.stdout);
    assert_eq!(run.stdout, stdout);
}

/// An array behind each array role: object elements wrapped into a role element, `Sizing` of objects and of
/// ints, a nested walk, a Sequencing widened to Iterating, and `first()` of an empty array, which fails with
/// Tomato's message after the earlier lines.
#[test]
fn arrays_play_the_array_roles_as_on_tomato() {
    let run = run_source(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "collections/sequence" take Sequencing;
import "collections/iteration" take Iterating;
import "size" take Sizing;
import "cell" take Cell;
role Named { name() is string; }
object Box does Named {
    has n is string;
    Box(:n);
    name() is string { n; }
}
object Probe {
    Probe();
    names(items is Sequencing<Named>) is string {
        items.first().name() + items.item(1).name() + items.last().name() + items.size().string();
    }
    walk(items is Iterating<Named>) is string {
        let out = Cell("");
        for items -> item { out.replace(out.value() + item.name()); }
        out.value();
    }
    count(items is Sizing) is int { items.size(); }
    deep(rows is Sequencing<Iterating<int>>) is int {
        let total = Cell(0);
        for rows -> row { for row -> x { total.replace(total.value() + x); } }
        total.value();
    }
    widened(items is Sequencing<int>) is int {
        let walk is Iterating<int> = items;
        let total = Cell(0);
        for walk -> x { total.replace(total.value() + x); }
        total.value();
    }
    first(items is Sequencing<int>) is int { items.first(); }
}
main() is int {
    let out = Stdout();
    let p = Probe();
    let boxes is @Box = [Box(n: "a"), Box(n: "b"), Box(n: "c")];
    out.writeln(p.names(boxes));
    out.writeln(p.walk(boxes));
    out.writeln(p.count(boxes).string());
    out.writeln(p.count([1, 2]).string());
    out.writeln(p.deep([[1, 2], [3], []]).string());
    out.writeln(p.widened([4, 5, 6]).string());
    let empty is @int = [];
    out.writeln(p.first(empty).string());
    0;
}"#).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.stdout, "abc3\nabc\n3\n2\n6\n15\n");
    let failure = run.result.unwrap_err();
    assert!(failure.contains("Cannot read first item of an empty sequence"), "{failure}");
}

/// An array of objects that each hold a role value is closed plain data: it is stored, read and walked
/// behind Sequencing. Tomato prints the same.
#[test]
fn arrays_of_objects_holding_role_values_travel_behind_roles() {
    expect_output(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "collections/sequence" take Sequencing;
role Value { text() is string; }
object Plain does Value {
    has t is string;
    Plain(:t);
    text() is string { t; }
}
object Member {
    has value is Value;
    Member(:value);
    text() is string { value.text(); }
}
object Table {
    has origin is @Member;
    Table(:origin);
    members() is Sequencing<Member> { origin; }
}
main() is int {
    let t = Table(origin: [Member(value: Plain(t: "a")), Member(value: Plain(t: "b"))]);
    let ms = t.members();
    Stdout().writeln(ms.size().string() + ms.first().text() + ms.last().text() + ms.item(1).text());
    for ms -> m { Stdout().writeln(m.text()); }
    0;
}"#, "2abb\na\nb\n");
}

/// Actions that call or walk a role nothing witnesses compile and never run; the rest of the program runs.
/// Tomato prints the same.
#[test]
fn calls_on_roles_nothing_witnesses_do_not_stop_the_program() {
    expect_output(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "collections/iteration" take Iterating;
role Visitor { visit(value is int) is int; }
object Node {
    has value is int;
    Node(:value);
    accept(to is Visitor) is int { to.visit(value) + 1; }
    floats(items is Iterating<float>) is int {
        for items -> item { Stdout().writeln(item.string()); }
        value;
    }
    total(items is Iterating<string>) is int {
        for items -> item { Stdout().writeln(item); }
        value;
    }
}
main() is int {
    Stdout().writeln(Node(value: 7).total(["x", "y"]).string());
    0;
}"#, "x\ny\n7\n");
}

/// A Cell over a plain role (P2.4 §4.8, #764): the registered Atomic holds the role reference; `replace`
/// publishes another implementation and `value` reads it back. Tomato prints the same.
#[test]
fn a_cell_holds_a_plain_role_value() {
    expect_output(r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "cell" take Cell;

role Shape { area() is int; }

object Square does Shape {
    has side is int;
    Square(:side);
    area() is int { side * side; }
}

object Rect does Shape {
    has w is int;
    has h is int;
    Rect(:w, :h);
    area() is int { w * h; }
}

main() is int {
    let first is Shape = Square(side: 3);
    let held = Cell(first);
    Stdout().writeln(held.value().area().string());
    let second is Shape = Rect(w: 2, h: 5);
    held.replace(second);
    Stdout().writeln(held.value().area().string());
    let parts = Cell(0);
    for 1..4 -> n {
        let square is Shape = Square(side: n);
        held.replace(square);
        parts.replace(parts.value() + held.value().area());
    }
    Stdout().writeln(parts.value().string());
    0;
}"#, "9\n10\n14\n");
}

/// Cells over other closed plain data (#778): a map, a variant of a plain object and an array of plain role values.
/// Tomato prints the same.
#[test]
fn cells_hold_maps_variants_and_role_arrays() {
    expect_output(r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "cell" take Cell;

role Shape { area() is int; }

object Square does Shape {
    has side is int;
    Square(:side);
    area() is int { side * side; }
}

main() is int {
    let out = Stdout();
    let counts = Cell(["a": 1]);
    counts.replace(["a": 1, "b": 2]);
    out.writeln(counts.value().size().string());
    let found is Maybe<Square> = Maybe.none;
    let maybe = Cell(found);
    maybe.replace(Maybe.some(Square(side: 4)));
    match maybe.value() {
        case Maybe.some(square) { out.writeln(square.area().string()); }
        case Maybe.none { out.writeln("none"); }
    }
    let first is Shape = Square(side: 2);
    let shapes = Cell([first]);
    let second is Shape = Square(side: 3);
    shapes.replace([first, second]);
    out.writeln(shapes.value().size().string() + " " + shapes.value()[1].area().string());
    0;
}"#, "2\n16\n2 9\n");
}
