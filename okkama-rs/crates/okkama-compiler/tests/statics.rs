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

fn lowered(text: &str, passes: &str) -> ir::Module {
    let source = SourceFile::new("test.ok", text).expect("small source");
    let program = syntax::parse(&source, &ParseOptions::default()).expect("valid syntax");
    okkama_compiler::lowering::with_passes(passes, || lower_program(&prune::strip_test_items(program))).expect("lowers")
}

fn symbol(module: &ir::Module, symbol: &str) -> usize {
    module.functions.iter().position(|function| function.symbol == symbol).expect(symbol)
}

/// #1073: a fresh resource must be owned by a region (or pinned) before the function leaves; the one it returns
/// goes to the caller. Without its `region.track` the resource made in `P.run` has no owner that would destroy it.
#[test]
fn the_linearity_check_refuses_a_resource_in_no_region() {
    let module = lowered(&format!("{RES} object P {{ P(); run(a is Res) is int {{ \
        let r = Res(n: a.read() + 1); r.read(); }} make() is Res {{ Res(n: 2); }} }} main() is int {{ 0; }}"), "none");
    // The constructor and `P.make` hand their resource out untracked or kept: the caller owns it.
    ir::linearity::check(&module).expect("the lowering's output owns every resource");
    let index = symbol(&module, "P.run");
    let made = module.functions[index].blocks.iter().flat_map(|block| &block.insts)
        .find_map(|inst| match &inst.kind {
            ir::InstKind::Call { .. } if module.functions[index].values[inst.result?.0] == ir::Type::Object(ir::ObjectId(0)) => inst.result,
            _ => None,
        })
        .expect("P.run makes a Res");
    let mut untracked = module.clone();
    for block in &mut untracked.functions[index].blocks {
        block.insts.retain(|inst| !matches!(inst.kind, ir::InstKind::RegionTrack { value, .. } | ir::InstKind::RegionAdopt { value, .. } if value == made));
    }
    ir::verify(&untracked).expect("only ownership is broken");
    let error = ir::linearity::check(&untracked).expect_err("the resource is in no region");
    assert!(error.contains(&format!("in P.run: {made} may be in no region when block")), "{error}");
}

/// #1073: a `return` from nested scopes keeps the resource at each close, so between two closes it is free; the
/// next region the exit closes owns it again, so a failing defer or close of an outer scope destroys it.
#[test]
fn a_resource_returned_from_nested_scopes_is_owned_between_their_closes() {
    let module = lowered(&format!("{RES} object P {{ P(); pick(flag is bool) is Res {{ \
        if flag {{ let r = Res(n: 1); return r; }} Res(n: 2); }} }} main() is int {{ P().pick(true).read(); }}"), "none");
    ir::linearity::check(&module).expect("the returned resource is owned until the last close");
    let function = &module.functions[symbol(&module, "P.pick")];
    let insts: Vec<&ir::InstKind> = function.blocks.iter().flat_map(|block| &block.insts).map(|inst| &inst.kind).collect();
    // Every close that keeps a value, but the last one before `ret`, is followed by a track of it in an outer region.
    let kept: Vec<(ir::ValueId, ir::ValueId)> = function.blocks.iter()
        .flat_map(|block| block.insts.iter().map(|inst| &inst.kind))
        .filter_map(|kind| match kind { ir::InstKind::RegionLeave { region, keep: Some(value) } => Some((*region, *value)), _ => None })
        .collect();
    assert!(kept.len() >= 3, "{kept:?}");
    for (region, value) in &kept {
        let retracked = insts.iter().any(|kind| matches!(kind, ir::InstKind::RegionTrack { region: r, value: v } if v == value && r != region));
        assert!(retracked, "{value} kept by {region} is tracked by an outer region");
    }
}

/// #1073: a loop that makes a resource each round must own each one before the next is made.
#[test]
fn the_linearity_check_refuses_a_loop_resource_in_no_region() {
    let module = lowered(&format!("{RES} object P {{ P(); run(n is int) is int {{ \
        for 1..=n -> i {{ Res(n: i).read(); }} 0; }} }} main() is int {{ 0; }}"), "none");
    ir::linearity::check(&module).expect("the lowering's output owns every resource");
    let index = symbol(&module, "P.run");
    let mut untracked = module.clone();
    for block in &mut untracked.functions[index].blocks {
        block.insts.retain(|inst| !matches!(inst.kind, ir::InstKind::RegionTrack { .. } | ir::InstKind::RegionAdopt { .. }));
    }
    let error = ir::linearity::check(&untracked).expect_err("a round's resource is in no region");
    assert!(error.contains("is made again at block") && error.contains("while the one made before is in no region"), "{error}");
}

/// #1073: a loop that enters its region every round without closing it is refused at the second `region.enter`,
/// which names the region, not at the function's exit.
#[test]
fn the_linearity_check_refuses_a_region_entered_while_open() {
    let module = lowered(&format!("{RES} object P {{ P(); run(n is int) is int {{ \
        for 1..=n -> i {{ Res(n: i).read(); }} 0; }} }} main() is int {{ 0; }}"), "none");
    let index = symbol(&module, "P.run");
    let function = &module.functions[index];
    // The loop body's region: the one entered in a block a back edge reaches again.
    let (block, region) = function.blocks.iter().enumerate()
        .flat_map(|(at, block)| block.insts.iter().filter(|inst| matches!(inst.kind, ir::InstKind::RegionEnter)).map(move |inst| (at, inst.result.expect("a region"))))
        .find(|(at, _)| *at > 1)
        .expect("a loop body region");
    let mut open = module.clone();
    for body in &mut open.functions[index].blocks {
        body.insts.retain(|inst| !matches!(inst.kind, ir::InstKind::RegionLeave { region: r, .. } if r == region));
    }
    let error = ir::linearity::check(&open).expect_err("the loop region is entered while open");
    assert!(error.contains(&format!("in P.run: {region} is entered again at block {block}")) && error.contains("while it may still be open"), "{error}");
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

/// #1070: a second `resource.pin` of a pinned resource names its state as open, not "not opened on some paths".
#[test]
fn the_linearity_check_names_an_open_resource_open() {
    let source = SourceFile::new("test.ok", format!("{RES} main() is int {{ let r = Res(n: 1); r.read(); }}")).expect("small source");
    let program = syntax::parse(&source, &ParseOptions::default()).expect("valid syntax");
    let mut module = okkama_compiler::lowering::with_passes("all", || lower_program(&prune::strip_test_items(program))).expect("lowers");
    let main = module.functions.iter().position(|function| function.symbol == "main").expect("main");
    let (block, at, pin) = module.functions[main].blocks.iter().enumerate()
        .find_map(|(block, body)| body.insts.iter().position(|inst| matches!(inst.kind, ir::InstKind::ResourcePin { .. }))
            .map(|at| (block, at, body.insts[at].clone())))
        .expect("the resource is pinned");
    module.functions[main].blocks[block].insts.insert(at + 1, pin);
    let error = ir::linearity::check(&module).expect_err("a second pin");
    assert!(error.contains(&format!("at block {block}, instruction {} while it is open", at + 1)), "{error}");
}
