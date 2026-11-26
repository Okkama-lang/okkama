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
    let module = okkama_compiler::lowering::with_passes("all", || lower_program(&prune::strip_test_items(program))).unwrap_or_else(|error| panic!("{text}: {error}"));
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

/// #909: a deferred action inside a try links its own failure with `fail.secondary`, which cannot fail; that block's
/// unwind edge into the catch is never taken, so it does not keep the scope's resource dynamic.
#[test]
fn an_infallible_block_does_not_keep_a_resource_dynamic() {
    let text = function_text(&format!("{RES} enum Stop is Error = (now); object Log {{ Log(); note() is void {{}} }} \
        main() is int {{ try {{ if true {{ let r = Res(n: 1); defer Log().note(); r.read(); }} 0; }} \
        catch e is Stop {{ 1; }} }}"), "main");
    assert!(text.contains("fail.secondary"), "the shape under test: {text}");
    assert!(text.contains("resource.pin"), "{text}");
    assert!(text.contains("resource.leave"), "{text}");
}

/// #909: a deferred action's handler is shared by the creation's own failure (the value was never made) and by
/// failures after it; the value's close must not be placed on that path (the IR stays valid either way).
#[test]
fn a_handler_shared_with_the_creation_stays_valid() {
    let text = function_text(&format!("{RES} object Log {{ Log(); note(text is string) is void {{}} }} \
        main() is int {{ {{ defer Log().note(\"1\"); let a = Res(n: 1); defer Log().note(\"a\"); \
        let b = Res(n: 2); defer Log().note(\"2\"); a.read() + b.read(); }} }}"), "main");
    assert!(text.contains("fail.secondary"), "the shape under test: {text}");
}

/// #909: two static resources of one scope, the older one also bound to an alias (a second `adopt` that, after
/// the splits, sits in a lower-numbered block). The older pin comes right after its creation, before the newer
/// creation can fail into a handler that closes it (Bible 11-resources).
#[test]
fn an_alias_does_not_move_a_pin_after_a_newer_creation() {
    let text = function_text(&format!("{RES} object Log {{ Log(); note(text is string) is void {{}} }} \
        main() is int {{ {{ let a = Res(n: 1); let b = Res(n: 2); defer Log().note(\"x\"); let alias = a; \
        alias.read() + b.read(); }} }}"), "main");
    assert_eq!(text.matches("resource.pin").count(), 2, "{text}");
}

/// #910: the linearity check accepts what the lowering makes and refuses a region left open or closed twice.
#[test]
fn the_linearity_check_refuses_a_missing_or_doubled_close() {
    let source = SourceFile::new("test.ok", format!("{RES} object P {{ P(); run(a is Res, b is Res) is int {{ \
        let r = Res(n: a.read() + b.read()); r.read(); }} }} main() is int {{ 0; }}")).expect("small source");
    let program = syntax::parse(&source, &ParseOptions::default()).expect("valid syntax");
    let module = okkama_compiler::lowering::with_passes("all", || lower_program(&prune::strip_test_items(program))).expect("lowers");
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

/// #1007: a region passed between blocks as a parameter is out of the linearity check's reach, so the check
/// refuses it instead of accepting the function unchecked. The lowering never makes one.
#[test]
fn the_linearity_check_refuses_a_region_block_parameter() {
    let source = SourceFile::new("test.ok", format!("{RES} object P {{ P(); run(flag is bool) is int {{ \
        let r = Res(n: 1); if flag {{ r.read(); }} else {{ 0; }} }} }} main() is int {{ 0; }}")).expect("small source");
    let program = syntax::parse(&source, &ParseOptions::default()).expect("valid syntax");
    let module = okkama_compiler::lowering::with_passes("none", || lower_program(&prune::strip_test_items(program))).expect("lowers");
    ir::linearity::check(&module).expect("the lowering's output is linear");
    let index = module.functions.iter().position(|function| function.symbol == "P.run").expect("P.run");
    let mut passed = module.clone();
    let function = &mut passed.functions[index];
    let region = function.blocks.iter().flat_map(|block| &block.insts)
        .find_map(|inst| matches!(inst.kind, ir::InstKind::RegionEnter).then_some(inst.result).flatten())
        .expect("a region");
    // Pass the region along every branch edge into a block other than the entry.
    let target = function.blocks.iter().flat_map(|block| block.term.kind.targets()).map(|target| target.block)
        .find(|block| block.0 != 0).expect("a branch target");
    let param = ir::ValueId(function.values.len());
    function.values.push(ir::Type::Region);
    function.blocks[target.0].params.push(param);
    for block in &mut function.blocks {
        for edge in block.term.kind.targets_mut().into_iter().filter(|edge| edge.block == target) {
            edge.args.push(region);
        }
    }
    let error = ir::linearity::check(&passed).expect_err("a region block parameter");
    assert!(error.contains("a region passed as a block parameter"), "{error}");
}

const GUARD: &str = "module app v1.0.0; object Guard { has n is int; Guard(:n); read() is int { n; } \
    async destroy { confirm n > 0; } }";

/// #329: an async-destroy guard alone in its scope of an async body settles directly: `resource.settle` at the
/// exit, `resource.settle_unwind` on the failure path, no region.
#[test]
fn an_async_destroy_guard_alone_in_its_scope_settles_without_a_region() {
    let text = function_text(&format!("{GUARD} enum Stop is Error = (now); object Work {{ Work(); \
        async run(stop is bool) is int {{ let g = Guard(n: 1); confirm !stop else throw Stop.now; g.read(); }} }} \
        async main() is int {{ await Work().run(false); }}"), "Work.run");
    assert!(text.contains("resource.pin"), "{text}");
    assert!(text.contains("resource.settle "), "{text}");
    assert!(text.contains("resource.settle_unwind"), "{text}");
    assert!(!text.contains("region."), "{text}");
    assert!(!text.contains("resource.leave"), "{text}");
}

/// #329: two async-destroy guards share their scope's region: both stay dynamic (the region settles them in
/// reverse creation order).
#[test]
fn async_destroy_guards_sharing_a_region_stay_dynamic() {
    let text = function_text(&format!("{GUARD} object Work {{ Work(); \
        async run() is int {{ let a = Guard(n: 1); let b = Guard(n: 2); a.read() + b.read(); }} }} \
        async main() is int {{ await Work().run(); }}"), "Work.run");
    assert!(!text.contains("resource."), "{text}");
    assert!(text.contains("region.settle"), "{text}");
}

/// #329: a guard handed out of its scope keeps its region.
#[test]
fn a_returned_async_destroy_guard_stays_with_its_region() {
    let text = function_text(&format!("{GUARD} object Work {{ Work(); async make() is Guard {{ Guard(n: 1); }} }} \
        async main() is int {{ let g = await Work().make(); g.read(); }}"), "Work.make");
    assert!(!text.contains("resource."), "{text}");
}

