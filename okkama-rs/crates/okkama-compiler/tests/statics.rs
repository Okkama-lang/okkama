//! Static resources (#909 2b): which resources the pass makes static, and what it leaves dynamic.
//! These tests lower but never execute Okkama; behaviour is in okkama-cli/tests/tweed_ownership.rs.

use okkama_compiler::SourceFile;
use okkama_compiler::ir;
use okkama_compiler::lowering::lower_program;
use okkama_compiler::syntax::{self, ParseOptions, prune};

const RES: &str = "module app v1.0.0; object Res { has n is int; Res(:n); \
    read() is int { n; } destroy { confirm n > 0; } }";

fn function_text(text: &str, symbol: &str) -> String {
    let source = SourceFile::new("test.ok", text).expect("small source");
    let program = syntax::parse(&source, &ParseOptions::default()).expect("valid syntax");
    let module = lower_program(&prune::strip_test_items(program)).unwrap_or_else(|error| panic!("{text}: {error}"));
    ir::verify(&module).expect("the passes keep the IR valid");
    let function = module.functions.iter().find(|function| function.symbol == symbol).expect(symbol);
    ir::print::function_text(&module, function)
}

#[test]
fn a_local_resource_is_destroyed_directly_without_a_region() {
    let text = function_text(&format!("{RES} main() is int {{ let r = Res(n: 1); r.read(); }}"), "main");
    assert!(text.contains("resource.pin"), "{text}");
    assert!(text.contains("resource.leave"), "{text}");
    assert!(text.contains("resource.unwind"), "{text}");
    assert!(!text.contains("region."), "{text}");
}

#[test]
fn a_returned_resource_stays_with_its_region() {
    let text = function_text(&format!("{RES} object Factory {{ Factory(); one() is Res {{ Res(n: 1); }} }} \
        main() is int {{ Factory().one().read(); }}"), "Factory.one");
    assert!(!text.contains("resource."), "{text}");
}

#[test]
fn a_resource_a_scope_hands_out_in_a_holder_stays_with_its_region() {
    let text = function_text(&format!("{RES} object Holder {{ has r is Res; Holder(:r); read() is int {{ r.read(); }} }} \
        object Picker {{ Picker(); pick(flag is bool) is int {{ \
        let holder = if flag {{ let r = Res(n: 1); Holder(r: r); }} else {{ Holder(r: Res(n: 2)); }}; \
        holder.read(); }} }} main() is int {{ Picker().pick(true); }}"), "Picker.pick");
    assert!(!text.contains("resource."), "the inner scope keeps the holder, which reaches the resource: {text}");
}

#[test]
fn a_resource_a_branch_hands_out_itself_outlives_the_branch() {
    let text = function_text(&format!("{RES} object Picker {{ Picker(); pick(flag is bool) is int {{ \
        let chosen = if flag {{ let r = Res(n: 1); r; }} else {{ Res(n: 2); }}; \
        chosen.read(); }} }} main() is int {{ Picker().pick(true); }}"), "Picker.pick");
    assert!(!text.contains("resource.leave"), "the branch keeps the resource it hands out: {text}");
}

/// #910: the linearity check accepts what the lowering makes and refuses a region left open or closed twice.
#[test]
fn the_linearity_check_refuses_a_missing_or_doubled_close() {
    let source = SourceFile::new("test.ok", format!("{RES} object P {{ P(); run(a is Res, b is Res) is int {{ \
        let r = Res(n: a.read() + b.read()); r.read(); }} }} main() is int {{ 0; }}")).expect("small source");
    let program = syntax::parse(&source, &ParseOptions::default()).expect("valid syntax");
    let module = lower_program(&prune::strip_test_items(program)).expect("lowers");
    ir::linearity::check(&module).expect("the lowering's output is linear");
    let index = module.functions.iter().position(|function| function.symbol == "P.run").expect("P.run");
    let closes = |kind: &ir::InstKind| matches!(kind, ir::InstKind::RegionLeave { .. } | ir::InstKind::ResourceLeave { .. });
    let mut missing = module.clone();
    for block in &mut missing.functions[index].blocks {
        block.insts.retain(|inst| !closes(&inst.kind));
    }
    let error = ir::linearity::check(&missing).expect_err("a close is missing");
    assert!(error.contains("may still be open"), "{error}");
    let mut doubled = module.clone();
    for block in &mut doubled.functions[index].blocks {
        if let Some(at) = block.insts.iter().position(|inst| closes(&inst.kind)) {
            let again = block.insts[at].clone();
            block.insts.insert(at, again);
            break;
        }
    }
    let error = ir::linearity::check(&doubled).expect_err("a close is doubled");
    assert!(error.contains("already closed"), "{error}");
}
