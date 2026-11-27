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
