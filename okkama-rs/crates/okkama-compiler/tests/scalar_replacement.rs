//! #1112: `inline` puts small leaf callees into their callers and `scalars` replaces the plain objects that never
//! leave a function by their fields. Checked on the IR, values through `fold`: a chain that reads back the fields
//! it built folds to the constant it computes.
use okkama_compiler::SourceFile;
use okkama_compiler::ir::{Constant, InstKind, IntValue, Module, TermKind};
use okkama_compiler::lowering::{lower_program, with_passes};
use okkama_compiler::syntax::{self, ParseOptions};

const POINT: &str = "object Point {
    has x is int;
    has y is int;
    Point(:x, :y);
    sum() is int { x + y; }
    moved(dx is int) is ^Point { ^x = x + dx; }
    ratio() is int { x % y; }
}
";

fn lower(body: &str, passes: &str) -> Module {
    let text = format!("module app v1.0.0;\n{POINT}{body}");
    let source = SourceFile::new("test.ok", text).unwrap();
    let ast = syntax::parse(&source, &ParseOptions::default()).unwrap();
    with_passes(passes, || lower_program(&ast)).unwrap()
}

fn function<'a>(module: &'a Module, symbol: &str) -> &'a okkama_compiler::ir::Function {
    module.functions.iter().find(|function| function.symbol == symbol).unwrap_or_else(|| panic!("no @{symbol}"))
}

fn kinds(module: &Module, symbol: &str) -> Vec<&'static str> {
    function(module, symbol).blocks.iter().flat_map(|block| &block.insts).map(|inst| match inst.kind {
        InstKind::Call { .. } => "call",
        InstKind::ObjNew { .. } => "obj.new",
        InstKind::ObjNext { .. } => "obj.next",
        InstKind::ObjGet { .. } => "obj.get",
        InstKind::SuccTake(_) => "succ.take",
        _ => "other",
    }).collect()
}

const WITHOUT: &str = "statics,regions,tail-calls,fold,hoist,dce,undef,jumps";

#[test]
fn a_constructor_successor_and_accessor_chain_leaves_no_object_and_folds() {
    let body = "main() is int { Point(x: 3, y: 4).moved(2).sum(); }\n";
    let before = kinds(&lower(body, WITHOUT), "main");
    assert!(before.contains(&"call") && before.contains(&"succ.take"), "{before:?}");
    let module = lower(body, "all");
    let after = kinds(&module, "main");
    assert!(!after.iter().any(|kind| matches!(*kind, "call" | "obj.new" | "obj.next" | "obj.get" | "succ.take")), "{after:?}");
    // (3 + 2) + 4: the successor's update and the untouched field both reach the sum.
    let main = function(&module, "main");
    let TermKind::Ret(Some(value)) = main.blocks.last().unwrap().term.kind else { panic!("main returns") };
    let defined = main.blocks.iter().flat_map(|block| &block.insts).find(|inst| inst.result == Some(value)).unwrap();
    assert_eq!(defined.kind, InstKind::Const(Constant::Int(IntValue::from_i64(9))));
}

#[test]
fn inline_alone_keeps_the_object_and_scalars_alone_find_no_constructor() {
    let body = "main() is int { Point(x: 3, y: 4).moved(2).sum(); }\n";
    let inlined = kinds(&lower(body, "statics,regions,tail-calls,inline"), "main");
    assert!(inlined.contains(&"obj.new") && inlined.contains(&"obj.next") && !inlined.contains(&"call"), "{inlined:?}");
    let scalars = kinds(&lower(body, "statics,regions,tail-calls,scalars"), "main");
    assert!(scalars.contains(&"call"), "without inlining main makes no object of its own: {scalars:?}");
}

#[test]
fn an_object_that_leaves_the_function_is_made_as_before() {
    // Returned: the caller gets a real Point.
    let module = lower(
        "object Maker {\n    Maker();\n    make(n is int) is Point { Point(x: n, y: 1).moved(1); }\n}\nmain() is int { Maker().make(2).sum(); }\n",
        "all",
    );
    let make = kinds(&module, "Maker.make");
    assert!(make.contains(&"obj.new") || make.contains(&"obj.next"), "{make:?}");
}

#[test]
fn a_callee_that_can_fail_stays_a_call() {
    // `%` fails on a zero divisor: inlining it would lose its activation in a failure's trace.
    let module = lower("main() is int { Point(x: 7, y: 2).ratio(); }\n", "all");
    assert!(kinds(&module, "main").contains(&"call"), "{:?}", kinds(&module, "main"));
}

const COUNTER: &str = "object Counter {
    has total is int;
    Counter(:total);
    plus(value is int) is ^Counter { ^total = total + value; }
    total() is int { total; }
}
";

/// A loop-carried object (`sum = sum.plus(v)`) becomes one block parameter per field: the loop carries ints.
#[test]
fn a_loop_carried_object_becomes_its_fields() {
    let body = format!("{COUNTER}main() is int {{ let sum = Counter(total: 1); for 0..8 -> value {{ sum = sum.plus(value); }} sum.total(); }}\n");
    let module = lower(&body, "all");
    let main = kinds(&module, "main");
    assert!(!main.iter().any(|kind| matches!(*kind, "call" | "obj.new" | "obj.next" | "obj.get")), "{main:?}");
    let function = function(&module, "main");
    for block in &function.blocks {
        for param in &block.params {
            assert_eq!(function.values[param.0], okkama_compiler::ir::Type::Int, "every loop parameter is an int");
        }
    }
}

/// A loop-carried object that a call inside the loop takes is made as before, at every step.
#[test]
fn a_loop_carried_object_that_escapes_stays_an_object() {
    let body = format!(
        "{COUNTER}object Sink {{ Sink(); keep(counter is Counter) is int {{ counter.total() % 3; }} }}\n\
         main() is int {{ let sum = Counter(total: 1); for 0..8 -> value {{ sum = sum.plus(value); Sink().keep(sum); }} sum.total(); }}\n"
    );
    let module = lower(&body, "all");
    assert!(kinds(&module, "main").contains(&"obj.next"), "{:?}", kinds(&module, "main"));
}

/// An array's element reaches a loop through the parameter its lookup fills itself: that parameter is no
/// candidate (the edge passes no argument for it), while the loop's own Counter still becomes its field.
#[test]
fn a_parameter_a_lookup_fills_is_not_split() {
    let body = format!(
        "{COUNTER}main() is int {{ let items = [Counter(total: 1), Counter(total: 2)]; let total = Counter(total: 0); \
         for 0..2 -> i {{ let item = items[i] else Counter(total: 0); total = total.plus(item.total()); }} total.total(); }}\n"
    );
    let module = lower(&body, "all");
    okkama_compiler::ir::verify(&module).unwrap();
}

/// A successor with branches (a particle's step with `if`s) is inlined as blocks between the call and a
/// continuation, and the loop it steps then carries the particle's fields.
#[test]
fn a_branched_successor_is_inlined_and_its_loop_carries_fields() {
    let body = "object Ball {
    has x is int;
    has v is int;
    Ball(:x, :v);
    stepped() is ^Ball {
        let ahead = x + v;
        let hit = ahead < 0 || ahead > 100;
        ^v = if hit { 0 - v; } else { v; };
        ^x = if hit { x; } else { ahead; };
    }
    x() is int { x; }
}
main() is int { let ball = Ball(x: 3, v: 7); for 0..50 -> step { ball = ball.stepped(); } ball.x(); }
";
    let module = lower(body, "all");
    okkama_compiler::ir::verify(&module).unwrap();
    let main = kinds(&module, "main");
    assert!(!main.iter().any(|kind| matches!(*kind, "call" | "obj.new" | "obj.next" | "obj.get")), "{main:?}");
}
