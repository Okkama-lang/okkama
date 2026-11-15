//! Slice V4: Coro → Low for plain async (plan §3.4; async ABI D20 §2.6, D24.2; OIR-2 §9.3; D18).
use super::*;

fn parsed(source: &str) -> Module {
    text::parse(source).unwrap_or_else(|error| panic!("{error}"))
}

fn options() -> VerifyOptions {
    VerifyOptions { execution: ExecutionMode::Tests, ..VerifyOptions::default() }
}

macro_rules! golden {
    ($name:literal) => {
        ($name, include_str!(concat!("../../tests/fixtures/async/coro/", $name, ".oir")),
            include_str!(concat!("../../tests/fixtures/async/low/", $name, ".oir")))
    };
}

const CANCEL: &str = include_str!("../../tests/fixtures/async/low-input/low-final-deliver-cancel.oir");

/// The V3 coro goldens and the hand-written D18 input, with their Low goldens.
const GOLDENS: &[(&str, &str, &str)] = &[
    golden!("async-plain-await"),
    golden!("async-await-failure"),
    golden!("async-caught-throw"),
    golden!("async-void"),
    golden!("async-repeated-await"),
    golden!("async-never-awaited"),
    golden!("coro-start-continuation"),
    golden!("coro-nested-cold-await"),
    golden!("coro-schedule-duty-live-across-await"),
    ("low-final-deliver-cancel", CANCEL, include_str!("../../tests/fixtures/async/low/low-final-deliver-cancel.oir")),
];

/// verify(coro) → low → verify(low) (OIR-2 §12), with a stable text round trip and the producer plan check.
fn lowered(name: &str, source: &str) -> Module {
    let module = parsed(source);
    verify_coro_with(&module, &options(), VerifyContext::default())
        .unwrap_or_else(|report| panic!("{name}: coro {report:?}"));
    let output = lower_cleanup(module, &LowOptions::default()).unwrap_or_else(|error| panic!("{name}: {error}"));
    verify_low_with(&output, &options(), VerifyContext::default())
        .unwrap_or_else(|report| panic!("{name}: low {report:?}\n{}", text::print(&output).unwrap()));
    let reparsed = parsed(&text::print(&output).unwrap());
    assert!(text::normalized_eq(&output, &reparsed).unwrap(), "{name}: unstable text");
    check_cleanup_plans(&reparsed, &LowOptions::default()).unwrap_or_else(|error| panic!("{name}: {error}"));
    output
}

fn diagnostics(module: &Module) -> Vec<VerifyDiagnostic> {
    match verify_low_with(module, &options(), VerifyContext::default()) {
        Ok(_) => Vec::new(),
        Err(report) => report.diagnostics,
    }
}

fn rules(module: &Module) -> Vec<RuleId> {
    diagnostics(module).iter().map(|diagnostic| diagnostic.rule).collect()
}

fn function_id(module: &Module, symbol: &str) -> FuncId {
    module.functions.iter().find(|(_, function)| function.symbol == symbol).unwrap().0
}

fn function_mut<'m>(module: &'m mut Module, symbol: &str) -> &'m mut Body {
    module.functions.iter_mut().find(|(_, function)| function.symbol == symbol).unwrap().1.body.as_mut().unwrap()
}

fn edited(source: &str, from: &str, to: &str) -> String {
    assert_eq!(source.matches(from).count(), 1, "{from}");
    source.replace(from, to)
}

/// Recompute the root maps of `symbol` after an edit, so that only the edited semantics can be rejected.
fn rebuild_roots(module: &mut Module, symbol: &str) {
    let function = function_id(module, symbol);
    let mut body = module.functions.get_mut(function).unwrap().body.take().unwrap();
    body.cleanup.roots = Arena::new();
    let options = LowOptions::default();
    let mut budget = Budget { options: &options, steps: 0, blocks: 0, roots: 0 };
    roots::collect(module, &mut body, &mut budget).unwrap();
    module.functions.get_mut(function).unwrap().body = Some(body);
}

#[test]
fn low_goldens_of_the_plain_async_inputs() {
    for (name, coro, golden) in GOLDENS {
        let output = lowered(name, coro);
        let expected = parsed(golden);
        verify_low_with(&expected, &options(), VerifyContext::default())
            .unwrap_or_else(|report| panic!("{name}: golden {report:?}"));
        assert!(text::normalized_eq(&output, &expected).unwrap(), "{name}\n{}\nexpected:\n{}",
            text::print(&output).unwrap(), text::print(&expected).unwrap());
    }
}

/// Plan §3.4: Own input stays synchronous; its async rejections did not move.
#[test]
fn own_input_keeps_the_synchronous_rejections() {
    let own = include_str!("../../tests/fixtures/async/own/async-plain-await.oir");
    let error = lower_cleanup(parsed(own), &LowOptions::default()).unwrap_err();
    assert!(matches!(error, LowError::Unsupported { .. }), "{error:?}");
    let mut high = parsed(own);
    high.profile = Profile::High;
    assert!(matches!(lower_cleanup(high, &LowOptions::default()), Err(LowError::InvalidInput { .. })));
}

/// The async forms of later slices keep named rejections in Coro input.
#[test]
fn later_slices_keep_named_rejections() {
    let source = include_str!("../../tests/fixtures/async/coro/async-plain-await.oir");
    // Async destroy (slice D1).
    let mut module = parsed(source);
    let main = function_id(&module, "main");
    let summary = module.functions.get(main).unwrap().summary.unwrap();
    module.summaries.get_mut(summary).unwrap().contract.async_destroy = true;
    let error = lower_cleanup(module, &LowOptions::default()).unwrap_err();
    assert_eq!(error, LowError::Unsupported { function: main, message: eligibility::ASYNC_DESTROY.into() });
    // An async defer entry (slice R2).
    let mut module = parsed(source);
    let work = function_id(&module, "work");
    module.functions.get_mut(work).unwrap().attrs.push(FunctionAttr::DeferEntry { replacements: vec![] });
    let error = lower_cleanup(module, &LowOptions::default()).unwrap_err();
    assert_eq!(error, LowError::Unsupported { function: work, message: eligibility::ASYNC_DEFER.into() });
    // Spawn (slice P1).
    let mut module = parsed(source);
    let body = function_mut(&mut module, "main");
    let (await_block, _) = body.blocks.iter().find(|(_, block)| matches!(&block.term.kind,
        TermKind::InvokeOwn { op, .. } if matches!(**op, Operation::Async(AsyncOp::Await { .. })))).unwrap();
    let TermKind::InvokeOwn { op, .. } = &mut body.blocks.get_mut(await_block).unwrap().term.kind else {
        unreachable!();
    };
    let Operation::Async(AsyncOp::Await { future, .. }) = **op else {
        unreachable!();
    };
    let state = body.blocks.get(body.entry).unwrap().params[0];
    let inst = body.insts.alloc(Instruction {
        results: vec![], op: Operation::Async(AsyncOp::FutureSpawn { future, state }),
        facts: InstructionFacts::default(), loc: None,
    }).unwrap();
    let entry = body.entry;
    body.blocks.get_mut(entry).unwrap().insts.push(inst);
    let error = lower_cleanup(module, &LowOptions::default()).unwrap_err();
    assert!(matches!(&error, LowError::Unsupported { message, .. } if message.contains("P1")), "{error:?}");
}

/// B10.4, D21 §3.3: the root map of a suspension site survives the suspension.
#[test]
fn suspension_root_maps_survive_the_suspension() {
    let (name, coro, _) = GOLDENS[0];
    let mut output = lowered(name, coro);
    let body = function_mut(&mut output, "main");
    let layout = body.frame_layout.clone().unwrap();
    let SuspendSite::Terminator(block) = layout.continuations[1].site else {
        panic!("continuation 1 is the await terminator");
    };
    let map = body.cleanup.roots.iter_mut().find(|(_, map)| map.boundary == RootBoundary::Terminator(block)).unwrap().1;
    assert!(map.through_suspend);
    map.through_suspend = false;
    let main = function_id(&output, "main");
    let found = diagnostics(&output).into_iter().any(|diagnostic| diagnostic.rule == RuleId::B105
        && diagnostic.primary.entity == EntityLocation::Terminator { function: main, block });
    assert!(found, "{:?}", rules(&output));
}

/// Champ draft 3 §5: a delivery is conditional, so the pending exit after it is checked as well as the Cancelled
/// failure. Dropping the commit on the not-delivered path leaves the reserve unconsumed.
#[test]
fn the_pending_exit_after_a_conditional_delivery_is_checked() {
    let commit = "    (%r: int, %p: own, %rd: deps) = exit.commit %reserved, destination caller, result int\n    \
        fut.publish %fr0, success(value=%r, packet=%p, deps=%rd)\n";
    let dropped = edited(CANCEL, commit, "    %p: own = own.empty\n    %rd: deps = deps.of %n\n    \
        fut.publish %fr0, success(value=%n, packet=%p, deps=%rd)\n");
    let output = lower_cleanup(parsed(&dropped), &LowOptions::default()).unwrap();
    let work = function_id(&output, "work");
    let body = output.functions.get(work).unwrap().body.as_ref().unwrap();
    let pending_arm = body.blocks.iter().find_map(|(_, block)| match &block.term.kind {
        TermKind::ExitDispatch { pending: Some(edge), .. } => Some(edge.block),
        _ => None,
    }).unwrap();
    let rejected = diagnostics(&output).into_iter().any(|diagnostic| match diagnostic.primary.entity {
        EntityLocation::Terminator { function, block } | EntityLocation::Block { function, block } => {
            function == work && block == pending_arm
        }
        _ => false,
    });
    assert!(rejected, "{:?}", diagnostics(&output));
}

/// OIR-2 §9.3 step 6: in a D21 body the delivery runs once, after the plan has drained.
#[test]
fn cancellation_is_delivered_after_the_plan_drains() {
    let (name, coro, _) = GOLDENS[9];
    let mut output = lowered(name, coro);
    let body = function_mut(&mut output, "work");
    let (plan, _) = body.insts.iter().find(|(_, inst)| matches!(inst.op,
        Operation::Cleanup(CleanupOp::Plan { .. }))).unwrap();
    let Operation::Cleanup(CleanupOp::Plan { cleanup: settled, .. }) = body.insts.get(plan).unwrap().op else {
        unreachable!();
    };
    let ty = body.values.get(settled).unwrap().ty;
    let deliver = body.insts.alloc(Instruction {
        results: vec![], op: Operation::Cleanup(CleanupOp::DeliverCancel { cleanup: settled }),
        facts: InstructionFacts::default(), loc: None,
    }).unwrap();
    let delivered = body.values.alloc(ValueData {
        ty, def: Definition::InstResult { inst: deliver, index: 0 }, facts: ValueFacts::Service,
    }).unwrap();
    body.insts.get_mut(deliver).unwrap().results = vec![delivered];
    let Operation::Cleanup(CleanupOp::Plan { cleanup, .. }) = &mut body.insts.get_mut(plan).unwrap().op else {
        unreachable!();
    };
    *cleanup = delivered;
    let (block, position) = body.blocks.iter().find_map(|(id, block)| {
        block.insts.iter().position(|inst| *inst == plan).map(|position| (id, position))
    }).unwrap();
    body.blocks.get_mut(block).unwrap().insts.insert(position, deliver);
    rebuild_roots(&mut output, "work");
    let work = function_id(&output, "work");
    let found = diagnostics(&output).into_iter().any(|diagnostic| diagnostic.rule == RuleId::B105
        && diagnostic.primary.entity == EntityLocation::Instruction { function: work, instruction: deliver });
    assert!(found, "{:?}", diagnostics(&output));
}

/// D20 §2.6: a close that holds a plain Future neither suspends nor fails in Low when nothing else does
/// (async ABI D24.2), so the exact envelope that own emits for it (`failures=[]`, `suspends=false`) verifies.
#[test]
fn a_plain_future_needs_no_suspension_or_failure_at_its_close() {
    let source = include_str!("../../tests/fixtures/async/coro/async-never-awaited.oir");
    let source = edited(source, "        slot 1: scope<SM> linear\n        slot 2: pending linear\n        \
        slot 3: cache-state linear\n", "");
    let source = edited(&source, "    continuation 1 at ^entry saves [] operands [1, 2, 3]\n", "");
    let close = "    %fr1: frame<@main> = frame.save %fr0, continuation 1\n    \
        invoke.own scope.end %q2, reserve %pending, \
        effects scope_effects(failures=[thrown, program, panic, native],\n        suspends=true, cancel=false)\n\
        \x20       normal ^done($closed, $pending)\n        unwind ^close_failed($closed, $failure)\n\
        ^done(%closed: closed, %reserved: pending) in SM:\n    \
        %fr2: frame<@main> = frame.resume %fr1, continuation 1\n";
    let exact = "    (%closed: closed, %reserved: pending) = scope.end %q2, reserve %pending, \
        effects scope_effects(failures=[],\n        suspends=false, cancel=false)\n";
    let source = edited(&source, close, exact);
    let source = edited(&source, "    fut.publish %fr2, success(packet=%p, deps=%pd)\n    ret\n\
        ^close_failed(%closed2: closed, %close_failure: failure) in SM:\n    \
        %fr3: frame<@main> = frame.resume %fr1, continuation 1\n    fut.publish %fr3, failure(%close_failure)\n",
        "    fut.publish %fr0, success(packet=%p, deps=%pd)\n");
    let output = lower_cleanup(parsed(&source), &LowOptions::default()).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(rules(&output), Vec::<RuleId>::new(), "{}", text::print(&output).unwrap());
}

/// B10.6, D16: a static duty inventory names its Services protocol; the runtime dictionary is optional.
#[test]
fn a_duty_schedule_names_its_protocol() {
    let (name, coro, _) = GOLDENS[5];
    let output = lowered(name, coro);
    let main = function_id(&output, "main");
    let schedule = output.functions.get(main).unwrap().body.as_ref().unwrap().cleanup.schedules.iter()
        .find(|(_, schedule)| !schedule.holders.is_empty()).map(|(id, _)| id).unwrap();
    let body = output.functions.get(main).unwrap().body.as_ref().unwrap();
    let entry = body.cleanup.schedules.get(schedule).unwrap();
    assert!(entry.dictionary.is_none() && entry.protocol.is_some());
    let mut missing = output.clone();
    function_mut(&mut missing, "main").cleanup.schedules.get_mut(schedule).unwrap().protocol = None;
    assert!(rules(&missing).contains(&RuleId::B106), "{:?}", rules(&missing));
}

/// Async ABI D20, D24.2 and Champ draft 3 §14.4 (opcode 21): `cleanup.settle` neither suspends nor fails outward.
#[test]
fn settle_is_class_l() {
    let (name, coro, _) = GOLDENS[5];
    let output = lowered(name, coro);
    let main = function_id(&output, "main");
    let body = output.functions.get(main).unwrap().body.as_ref().unwrap();
    let (_, settle) = body.insts.iter().find(|(_, inst)| matches!(inst.op,
        Operation::Cleanup(CleanupOp::Settle { .. }))).unwrap();
    let contract = ContractRegistry::new(&output).lookup(&settle.op).unwrap();
    assert!(!contract.effects.suspends && contract.effects.failures.is_empty() && contract.unwind.is_none());
}

/// D20 §2.6 narrows only the settle of a Future: its outcome identities keep the cleanup effects of the result
/// type, so a Future of a resource whose destroy throws still needs its failures in the close envelope.
#[test]
fn a_future_keeps_the_cleanup_effects_of_its_outcome() {
    let source = include_str!("../../tests/fixtures/async/coro/async-never-awaited.oir");
    let source = edited(source, "scope SW action in @work\n", "object Res {\n    destroy @destroy\n}\n\
        summary Md internal parameters [parameter_effect(convention=borrow, destination=none)] contract\n    \
        call_contract(failures=[thrown], publication=atomic)\n\
        func @destroy(%self: Res) -> void [extern \"fixture.destroy\", destroy Res] summary(Md);\n\
        scope SW action in @work\n");
    let source = edited(&source, "    (%f: future<int> facts(managed(deps=d0)), %c: cache-state,",
        "    (%f: future<Res> facts(managed(deps=d0)), %c: cache-state,");
    let source = edited(&source, "        function=@work, arguments=[%seven], descriptors=[]), result int, owner SM",
        "        function=@work, arguments=[%seven], descriptors=[]), result Res, owner SM");
    let mut output = lower_cleanup(parsed(&source), &LowOptions::default()).unwrap();
    let body = function_mut(&mut output, "main");
    let exact = ScopeEffectEnvelope { failures: vec![], suspends: false, cancel: false };
    for (_, close) in body.cleanup.closes.iter_mut() {
        close.effects.failures = exact.failures.clone();
    }
    for (_, inst) in body.insts.iter_mut() {
        if let Operation::Cleanup(CleanupOp::Begin { effects, .. }) = &mut inst.op {
            effects.failures = exact.failures.clone();
        }
    }
    assert!(rules(&output).contains(&RuleId::B104), "{:?}", rules(&output));
}

/// Champ draft 3 §5: the selector of the second plan decides an open delivery; a finish reached without it would
/// dispatch only one of the two outcomes.
#[test]
fn an_open_delivery_is_decided_before_finish() {
    let (name, coro, _) = GOLDENS[9];
    let mut output = lowered(name, coro);
    let body = function_mut(&mut output, "work");
    let (deliver_block, planned) = body.blocks.iter().find_map(|(id, block)| {
        let delivers = block.insts.iter().any(|inst| matches!(body.insts.get(*inst).unwrap().op,
            Operation::Cleanup(CleanupOp::DeliverCancel { .. })));
        let planned = body.insts.get(*block.insts.last()?).unwrap().results.first().copied();
        delivers.then_some((id, planned?))
    }).unwrap();
    let finish_block = body.blocks.iter().find_map(|(id, block)| {
        let inst = *block.insts.first()?;
        matches!(body.insts.get(inst).unwrap().op, Operation::Cleanup(CleanupOp::Finish { .. })).then_some(id)
    }).unwrap();
    let TermKind::Br(Target { block: next, .. }) = body.blocks.get(deliver_block).unwrap().term.kind else {
        panic!("the delivery block enters the selector of the second plan");
    };
    body.blocks.get_mut(deliver_block).unwrap().term.kind = TermKind::Br(Target {
        block: finish_block, args: vec![planned],
    });
    // The selector of the second plan is unreachable now; S3 wants it as empty tombstones.
    let TermKind::CleanupNext { selected, .. } = body.blocks.get(next).unwrap().term.kind.clone() else {
        panic!("the second plan has a selector");
    };
    let TermKind::FailureDispatch { some, .. } = body.blocks.get(selected.block).unwrap().term.kind.clone() else {
        panic!("its destroy dispatches the failure");
    };
    for block in [next, selected.block, some.block] {
        let data = body.blocks.get_mut(block).unwrap();
        data.insts.clear();
        data.params.clear();
        data.term.kind = TermKind::Unreachable;
    }
    // The old root maps name the removed values; `rebuild_roots` computes them again.
    body.cleanup.roots = Arena::new();
    compact::compact(body).unwrap();
    let finish = body.blocks.get(finish_block).unwrap().insts[0];
    rebuild_roots(&mut output, "work");
    let work = function_id(&output, "work");
    let found = diagnostics(&output).into_iter().any(|diagnostic| diagnostic.rule == RuleId::B105
        && diagnostic.primary.entity == EntityLocation::Instruction { function: work, instruction: finish });
    assert!(found, "{:?}", diagnostics(&output));
}
