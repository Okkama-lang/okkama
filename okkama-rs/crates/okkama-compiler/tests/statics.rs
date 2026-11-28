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
    borrowing(&mut untracked, "Res.read");
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
    borrowing(&mut untracked, "Res.read");
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

/// The parameters of `symbol` borrow: its adopts of them go, as the parameter-adopt elision drops them (#555). A
/// caller that passes such an action a fresh resource must own it itself (#1093); the #1073 tests above make
/// `Res.read` borrow, because an action that adopts its argument would own it.
fn borrowing(module: &mut ir::Module, symbol_name: &str) {
    let index = symbol(module, symbol_name);
    let params = module.functions[index].params.clone();
    for block in &mut module.functions[index].blocks {
        block.insts.retain(|inst| !matches!(inst.kind, ir::InstKind::RegionAdopt { value, .. } if params.contains(&value)));
    }
}

/// #1093: lowering's output passes the check with and without the IR passes when a call hands back an identity
/// someone else owns (a cap field reading its owner's field, a closure returning its argument, sync or async), when
/// a fresh resource goes straight to an action that adopts it (a cap field or a closure that makes one, a worker
/// entry's rebuilt receiver), and when an async action returns its argument.
#[test]
fn the_linearity_check_accepts_borrowed_call_results_and_adopted_arguments() {
    let programs = [
        format!("{RES} object Box {{ has item is Res; Box(:item); cap View {{ item is Res {{ item; }} \
            fresh is Res {{ Res(n: 7); }} }} }} main() is int {{ let b = Box(item: Res(n: 1)); let v = b::View; \
            v::item.read() + v::fresh.read(); }}"),
        format!("{RES} object Use {{ Use(); alias() is Res {{ let f = (v is Res) is Res {{ v; }}; f(Res(n: 2)); }} \
            made() is int {{ let f = () is Res {{ Res(n: 5); }}; f().read(); }} }} main() is int {{ Use().alias().read(); }}"),
        format!("{RES} object Job {{ Job(); async alias(r is Res) is Res {{ r; }} async make() is Res {{ Res(n: 2); }} \
            async both() is int {{ let a = await self.alias(Res(n: 3)); let b = await self.make(); a.read() + b.read(); }} }} \
            async main() is int {{ await Job().both(); }}"),
        format!("{RES} object W {{ has n is int; W(:n); work(i is int) is int {{ i + n; }} destroy {{}} }} \
            async main() is int {{ let a = await spawn W(n: 1).work(2); let b = await spawn Res(n: 3).read(); a + b; }}"),
    ];
    for program in &programs {
        for passes in ["none", "all", "statics"] {
            let module = lowered(program, passes);
            ir::linearity::check(&module).unwrap_or_else(|error| panic!("{passes}: {error}\n{program}"));
        }
    }
}

/// #1093: whether a call makes a resource is the census's answer. A closure that makes one hands it out fresh, so
/// without the scope's track of it (#1099) and with an action that only borrows it, nothing owns it; a closure that
/// returns its argument hands back the caller's own (tracked) resource, so the same borrowing action leaves nothing
/// unowned.
#[test]
fn the_linearity_check_takes_fresh_call_results_from_the_census() {
    let module = lowered(&format!("{RES} object Use {{ Use(); \
        made() is int {{ let f = () is Res {{ Res(n: 5); }}; f().read(); }} }} main() is int {{ 0; }}"), "none");
    let mut borrowed = module.clone();
    borrowing(&mut borrowed, "Res.read");
    let index = symbol(&borrowed, "Use.made");
    let made = borrowed.functions[index].blocks.iter().flat_map(|block| &block.insts)
        .find_map(|inst| matches!(inst.kind, ir::InstKind::CallFn { .. }).then_some(inst.result).flatten())
        .expect("Use.made calls its closure");
    for block in &mut borrowed.functions[index].blocks {
        block.insts.retain(|inst| !matches!(inst.kind, ir::InstKind::RegionTrack { value, .. } if value == made));
    }
    let error = ir::linearity::check(&borrowed).expect_err("the closure's resource is in no region");
    assert!(error.contains(&format!("in Use.made: {made} may be in no region")), "{error}");
    let mut alias = lowered(&format!("{RES} object Use {{ Use(); \
        alias() is int {{ let f = (v is Res) is Res {{ v; }}; f(Res(n: 2)).read(); }} }} main() is int {{ 0; }}"), "none");
    borrowing(&mut alias, "Res.read");
    ir::linearity::check(&alias).expect("a closure returning its argument makes nothing");
}

/// #1093: a worker entry rebuilds the receiver and hands it to the action, which adopts it; an action that only
/// borrowed it would leave the receiver with no owner.
#[test]
fn the_linearity_check_lets_an_adopting_action_own_a_worker_receiver() {
    let module = lowered(&format!("{RES} object W {{ has n is int; W(:n); work(i is int) is int {{ i + n; }} \
        destroy {{}} }} async main() is int {{ await spawn W(n: 1).work(2); }}"), "none");
    ir::linearity::check(&module).expect("the action adopts the rebuilt receiver");
    let entry = module.functions.iter().find(|function| function.symbol.starts_with("W.work$worker#")).expect("a worker entry");
    let entry = entry.symbol.clone();
    let mut borrowed = module.clone();
    borrowing(&mut borrowed, "W.work");
    let error = ir::linearity::check(&borrowed).expect_err("nobody owns the receiver");
    assert!(error.contains(&format!("in {entry}: ")) && error.contains("may be in no region"), "{error}");
}

/// #1093: a resource passed to a block parameter moves there: returning the parameter hands it out, and dropping
/// it leaves the resource with no owner. Lowering does not build this shape yet; the module is rewritten by hand.
#[test]
fn the_linearity_check_moves_a_resource_into_a_block_parameter() {
    let module = lowered(&format!("{RES} object P {{ P(); make() is Res {{ Res(n: 2); }} }} \
        main() is int {{ P().make().read(); }}"), "none");
    let index = symbol(&module, "P.make");
    let through = |returned_param: bool| {
        let mut module = module.clone();
        let function = &mut module.functions[index];
        let (at, value) = function.blocks.iter().enumerate()
            .find_map(|(at, block)| match block.term.kind { ir::TermKind::Ret(Some(value)) => Some((at, value)), _ => None })
            .expect("P.make returns");
        let ty = function.values[value.0].clone();
        function.values.push(ty.clone());
        let param = ir::ValueId(function.values.len() - 1);
        let span = function.blocks[at].term.span;
        let next = ir::BlockId(function.blocks.len());
        function.blocks[at].term.kind = ir::TermKind::Br(ir::Target { block: next, args: vec![value] });
        let mut insts = Vec::new();
        let returned = if returned_param {
            param
        } else {
            // The parameter is dropped: the block returns another resource it makes.
            let func = function.blocks.iter().flat_map(|block| &block.insts).find_map(|inst| match &inst.kind {
                ir::InstKind::Call { func, .. } if inst.result == Some(value) => Some(*func),
                _ => None,
            }).expect("P.make calls the constructor");
            function.values.push(ir::Type::Int);
            let n = ir::ValueId(function.values.len() - 1);
            function.values.push(ty);
            let other = ir::ValueId(function.values.len() - 1);
            insts.push(ir::Inst { result: Some(n), kind: ir::InstKind::Const(ir::Constant::Int(ir::IntValue::from_i64(3))), span });
            insts.push(ir::Inst { result: Some(other), kind: ir::InstKind::Call { func, args: vec![n] }, span });
            other
        };
        function.blocks.push(ir::Block { params: vec![param], unwind: None, insts,
            term: ir::Terminator { kind: ir::TermKind::Ret(Some(returned)), span } });
        ir::verify(&module).expect("valid IR");
        (module, param)
    };
    let (returned, _) = through(true);
    ir::linearity::check(&returned).expect("the block parameter hands the resource out");
    let (dropped, param) = through(false);
    let error = ir::linearity::check(&dropped).expect_err("the block parameter's resource has no owner");
    assert!(error.contains(&format!("in P.make: {param} may be in no region")), "{error}");
}

/// #1138: a loop that passes each round's fresh resource to its header's block parameter loses the one the
/// parameter held unless a region owns it first: the branch refuses it, as the check did before #1093 moved
/// resources into block parameters. With a region tracking the parameter before the branch, the loop is linear.
/// Lowering does not build this shape; the module is rewritten by hand.
#[test]
fn the_linearity_check_refuses_a_block_parameter_overwritten_while_free() {
    let module = lowered(&format!("{RES} object P {{ P(); make() is Res {{ Res(n: 2); }} }} \
        main() is int {{ P().make().read(); }}"), "none");
    let index = symbol(&module, "P.make");
    let looping = |owned: bool| {
        let mut module = module.clone();
        let function = &mut module.functions[index];
        let (value, func, span) = function.blocks.iter().flat_map(|block| &block.insts).find_map(|inst| match &inst.kind {
            ir::InstKind::Call { func, .. } if inst.result.is_some_and(|value| matches!(function.values[value.0], ir::Type::Object(_))) => {
                Some((inst.result?, *func, inst.span))
            }
            _ => None,
        }).expect("P.make calls the constructor");
        let ty = function.values[value.0].clone();
        // The body is replaced whole: only the parameters' values stay.
        assert!(function.params.iter().enumerate().all(|(at, param)| param.0 == at), "{:?}", function.params);
        function.values.truncate(function.params.len());
        let mut new_value = |ty: ir::Type| {
            function.values.push(ty);
            ir::ValueId(function.values.len() - 1)
        };
        let (one, first, param, flag, two, next) =
            (new_value(ir::Type::Int), new_value(ty.clone()), new_value(ty.clone()), new_value(ir::Type::Bool), new_value(ir::Type::Int), new_value(ty));
        // The owned loop's region; the leaking one has none.
        let region = owned.then(|| new_value(ir::Type::Region));
        let inst = |result: ir::ValueId, kind: ir::InstKind| ir::Inst { result: Some(result), kind, span };
        let effect = |kind: ir::InstKind| ir::Inst { result: None, kind, span };
        let term = |kind: ir::TermKind| ir::Terminator { kind, span };
        let target = |block: usize, args: Vec<ir::ValueId>| ir::Target { block: ir::BlockId(block), args };
        let mut round = vec![
            inst(two, ir::InstKind::Const(ir::Constant::Int(ir::IntValue::from_i64(2)))),
            inst(next, ir::InstKind::Call { func, args: vec![two] }),
        ];
        let mut entry = vec![
            inst(one, ir::InstKind::Const(ir::Constant::Int(ir::IntValue::from_i64(1)))),
            inst(first, ir::InstKind::Call { func, args: vec![one] }),
        ];
        let mut exit = Vec::new();
        if let Some(region) = region {
            entry.insert(0, inst(region, ir::InstKind::RegionEnter));
            round.push(effect(ir::InstKind::RegionTrack { region, value: param }));
            exit.push(effect(ir::InstKind::RegionLeave { region, keep: Some(param) }));
        }
        function.blocks = vec![
            ir::Block { params: Vec::new(), unwind: None, insts: entry, term: term(ir::TermKind::Br(target(1, vec![first]))) },
            ir::Block { params: vec![param], unwind: None,
                insts: vec![inst(flag, ir::InstKind::Const(ir::Constant::Bool(true)))],
                term: term(ir::TermKind::Cond { condition: flag, then_target: target(2, Vec::new()), else_target: target(3, Vec::new()) }) },
            ir::Block { params: Vec::new(), unwind: None, insts: round, term: term(ir::TermKind::Br(target(1, vec![next]))) },
            ir::Block { params: Vec::new(), unwind: None, insts: exit, term: term(ir::TermKind::Ret(Some(param))) },
        ];
        ir::verify(&module).expect("valid IR");
        (module, param)
    };
    let (leaking, param) = looping(false);
    let error = ir::linearity::check(&leaking).expect_err("the previous round's resource is lost");
    assert!(error.contains(&format!("in P.make: {param} takes a new value at the branch from block 2 to block 1")) &&
        error.contains("while the one it holds may be in no region"), "{error}");
    let (owned, _) = looping(true);
    ir::linearity::check(&owned).expect("the region owns each round's resource before the next replaces it");
}

/// #1093: the `statics` pass alone leaves no region open. It removes a region it emptied, so a failing
/// `resource.leave` before that region's `region.leave` does not leave it open on the failure path.
#[test]
fn the_statics_pass_alone_leaves_no_emptied_region() {
    let text = "module app v1.0.0; object Guard { has n is int; Guard(:n); destroy { confirm n > 0; } } \
        object Work { Work(); fail(code is int) is int { confirm code < 0; code; } \
        guarded(code is int) is int { let outer = Guard(n: code); let inner = Guard(n: code); self.fail(code) + 1; } \
        single(code is int) is int { let g = Guard(n: code); self.fail(code); } } \
        main() is int { Work().guarded(1) + Work().single(1); }";
    let module = lowered(text, "statics");
    for symbol_name in ["Work.guarded", "Work.single"] {
        let function = &module.functions[symbol(&module, symbol_name)];
        let pins = function.blocks.iter().flat_map(|block| &block.insts)
            .filter(|inst| matches!(inst.kind, ir::InstKind::ResourcePin { .. })).count();
        assert!(pins >= 1, "{symbol_name} has a static resource: {module}");
    }
    ir::linearity::check(&module).expect("no region stays open");
}

/// #1099: lowering tracks a closure's or a cap field's result in its scope when the census finds it is a new
/// identity, and leaves a borrowed one (the owner's field, the closure's argument) untracked, as before.
#[test]
fn closure_and_cap_results_are_tracked_only_when_they_are_new() {
    let module = lowered(&format!("{RES} object Box {{ has item is Res; Box(:item); cap View {{ item is Res {{ item; }} \
        fresh is Res {{ Res(n: 7); }} }} }} object Use {{ Use(); \
        run(b is Box) is int {{ let v = b::View; v::item; v::fresh; let f = () is Res {{ Res(n: 5); }}; f(); \
        let same = (r is Res) is Res {{ r; }}; same(Res(n: 6)); 0; }} }} main() is int {{ 0; }}"), "none");
    let function = &module.functions[symbol(&module, "Use.run")];
    let insts: Vec<&ir::Inst> = function.blocks.iter().flat_map(|block| &block.insts).collect();
    let results: Vec<ir::ValueId> = insts.iter()
        .filter(|inst| matches!(inst.kind, ir::InstKind::CallCap { .. } | ir::InstKind::CallFn { .. }))
        .filter_map(|inst| inst.result)
        .collect();
    let tracked = |value: ir::ValueId| insts.iter().any(|inst| matches!(inst.kind, ir::InstKind::RegionTrack { value: v, .. } if v == value));
    // In order: the owner's field, the fresh cap field, the closure that makes one, the closure returning its argument.
    assert_eq!(results.len(), 4, "{module}");
    assert_eq!(results.iter().map(|&value| tracked(value)).collect::<Vec<_>>(), [false, true, true, false], "{module}");
    ir::linearity::check(&module).expect("every new resource has an owner");
}

/// #1102: the Future of a bound async action's call is tracked in its scope like a direct call's: discarded, right
/// after the call; awaited at once (#932), only on the await's failure path, in the landing block before the
/// handler.
#[test]
fn a_bound_async_call_future_is_tracked_like_a_direct_calls() {
    let module = lowered(&format!("{RES} object Job {{ Job(); async take(r is Res) is int {{ r.read(); }} }} \
        async main() is int {{ let t is async (Res) is int = Job().take; t(Res(n: 1)); await t(Res(n: 2)); }}"), "none");
    let function = &module.functions[symbol(&module, "main")];
    let futures: Vec<ir::ValueId> = function.blocks.iter().flat_map(|block| &block.insts)
        .filter(|inst| matches!(inst.kind, ir::InstKind::CallFn { .. }))
        .filter_map(|inst| inst.result)
        .collect();
    assert_eq!(futures.len(), 2, "{module}");
    let after_call = |future: ir::ValueId| function.blocks.iter().any(|block| block.insts.windows(2).any(|pair| {
        pair[0].result == Some(future) && matches!(pair[1].kind, ir::InstKind::RegionTrack { value, .. } if value == future)
    }));
    let landing = |future: ir::ValueId| function.blocks.iter().any(|block| block.params.len() == 1
        && matches!(block.insts.as_slice(), [ir::Inst { kind: ir::InstKind::RegionTrack { value, .. }, .. }] if *value == future));
    assert!(after_call(futures[0]) && !landing(futures[0]), "the discarded Future: {module}");
    assert!(!after_call(futures[1]) && landing(futures[1]), "the awaited Future: {module}");
    // `Job.take` is no runtime driver: the normal path tracks the taken Future nowhere.
    let tracks = function.blocks.iter().flat_map(|block| &block.insts)
        .filter(|inst| matches!(inst.kind, ir::InstKind::RegionTrack { value, .. } if value == futures[1])).count();
    assert_eq!(tracks, 1, "{module}");
    ir::linearity::check(&module).expect("every resource has an owner");
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
