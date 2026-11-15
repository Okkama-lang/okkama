//! `any` on Champ, in process (#689, #569 C6): an `any` value is a traced reference to an any box, which holds
//! the value in its one field and the value's type in its descriptor. Conformance cases are compared with
//! their case.json; a case that stops on a gap outside `any` passes own and Low and is a `through_low` entry
//! until the backend accepts it.
//!
//! Only pure language cases: no processes, workers, network, outside files or test mode.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{case_dir, check, compile, run_case, run_source};
use okkama_compiler::SourceFile;

/// Phases after verify_low belong to the SPB 17 / Champ backend.
const BACKEND: [&str; 4] = ["layout", "compile_low", "verify_load", "encode"];

/// The case passes own and Low; it runs in Champ, compared with case.json, once the backend accepts it.
fn through_low(id: &str) {
    let file = case_dir(id).join("main.ok");
    let text = std::fs::read_to_string(&file).unwrap();
    let source = SourceFile::new(std::fs::canonicalize(&file).unwrap().to_string_lossy(), text).unwrap();
    match compile(&[source]) {
        Ok(_) => run_case(id).unwrap_or_else(|error| panic!("{id}: {error}")),
        Err(error) if BACKEND.iter().any(|phase| error.contains(&format!("OIR-2 {phase}:"))) => {
            println!("{id}: own and low passed; stops in the backend: {error}");
        }
        Err(error) => panic!("{id}: {error}"),
    }
}

/// `any` boundaries: boxed literals, `any` parameters and results, arrays and maps of `any`, overloads.
#[test]
fn champ_cases_any() {
    check(&[
        "L03.overloads.selection",
        "L03.variadic.overload-by-tail-type",
        "L04.ctor.typed-overload",
        "L05.any.transport",
        "L05.boundary.typed-values",
        "L05.generic.inference",
        "L05.literal.heterogeneous-boundaries",
        "L05.literal.heterogeneous-to-any",
        "L05.literal.homogeneous-inference",
    ]);
}

/// compile_low: `array.get` of a byte array (`bytes[0]`), which SPB 17 has only as `lookup`.
#[test]
fn any_array_replaced() {
    through_low("L05.array.replaced");
}

/// layout: a module constant (`const NESTED`) is a declaration the SPB 17 layout does not take.
#[test]
fn any_literal_empty_collections() {
    through_low("L05.literal.empty-collections");
}

#[test]
#[ignore = "#569: OIR-2 has no char-to-string conversion"]
fn any_variadic_string_tail_converts() {
    run_case("L03.variadic.string-tail-converts").unwrap();
}

fn expect_output(source: &str, stdout: &str) {
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.result, Ok(0), "stdout={:?}", run.stdout);
    assert_eq!(run.stdout, stdout);
}

/// A boxed resource belongs to the scope that holds the `any` value: it is destroyed once, when that scope
/// ends, also after a round trip through an `any` parameter and result. Tomato prints the same.
#[test]
fn a_boxed_resource_is_destroyed_once_by_its_scope() {
    expect_output(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    destroy { Stdout().writeln("destroy " + name); }
}
object Relay {
    Relay();
    echo(value is any) is any { value; }
}
main() is int {
    let out = Stdout();
    {
        let held is any = Res(name: "boxed");
        out.writeln("held");
    }
    let kept = Relay().echo(Res(name: "echoed"));
    out.writeln("end");
    0;
}"#, "held\ndestroy boxed\nend\ndestroy echoed\n");
}

/// Plain values of several types share `any` arrays and maps: they are counted, iterated, passed and spread.
/// Tomato prints the same.
#[test]
fn plain_any_values_travel_through_arrays_maps_and_calls() {
    expect_output(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Relay {
    Relay();
    count(values is @any) is int { values.size(); }
    echo(value is any) is any { value; }
}
main() is int {
    let out = Stdout();
    let one is any = 1;
    let text is any = "two";
    let values is @any = [one, text, 3.5, [4, 5]];
    let row is %string:any = ["a": one, "b": true];
    let back = Relay().echo(text);
    out.writeln(Relay().count(values).string() + " " + row.size().string());
    for values -> item {
        Relay().echo(item);
    }
    let copy is @any = values...;
    out.writeln(copy.size().string());
    0;
}"#, "4 2\n4\n");
}
