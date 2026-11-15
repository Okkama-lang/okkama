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

const SUSPENDING: &str = include_str!("../../tests/fixtures/async/low-input/low-suspending-close.oir");

/// The reconciled coro goldens (V2's own output through coro) and the V3-form input whose closes suspend, with
/// their Low goldens. Every async body ends in a D18 close with `cancel=true`; the fixed-root mains close without
/// a reserve.
const GOLDENS: &[(&str, &str, &str)] = &[
    golden!("async-plain-await"),
    golden!("async-await-failure"),
    golden!("async-caught-throw"),
    golden!("async-void"),
    golden!("async-repeated-await"),
    golden!("async-never-awaited"),
    golden!("coro-start-continuation"),
    golden!("coro-start-descriptors"),
    golden!("coro-nested-cold-await"),
    golden!("coro-schedule-duty-live-across-await"),
    ("low-suspending-close", SUSPENDING, include_str!("../../tests/fixtures/async/low/low-suspending-close.oir")),
    // R2: a resource held across an await; a temporary take with its rechecked proof.
    golden!("r2-resource-across-await"),
    golden!("r2-take-proof-recheck"),
    // R2, C-6: async defers whose entry awaits, after a body failure and across a pending exit.
    golden!("r2-defer-await-body-failure"),
    golden!("r2-defer-await-reserve"),
    // R2: closes that suspend while the body owns a resource.
    golden!("r2-resource-suspending-close"),
    // K1: cancellation before start, the elaboration §13.4 fixture, a nested close before the next await.
    golden!("k1-cancel-unstarted"),
    golden!("k1-failure-view-escapes"),
    golden!("k1-nested-close-next-await"),
    // #684: a nested close while a Future of an enclosing scope is live.
    golden!("nested-close-outer-future"),
    // #691: a void fixed root (an async test); the deps of a Future argument in continuation 0.
    golden!("void-fixed-root"),
    golden!("start-deps"),
    // #733: an async successor action publishes its successor (T5 and B8.2 read the language signature).
    golden!("async-successor"),
];
const PLAIN: usize = 0;
const NEVER: usize = 5;

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
    let own = include_str!("../own/tests/async-goldens/async-plain-await.oir");
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
    // An async defer entry runs only from an async body (C-6; R2 lowers it there, r2-defer-await-* goldens).
    let mut module = parsed(include_str!("../../tests/fixtures/async/coro/r2-defer-await-body-failure.oir"));
    let risky = function_id(&module, "risky");
    let closer = function_id(&module, "closer");
    let body = function_mut(&mut module, "risky");
    let block = body.entry;
    let term = std::mem::replace(&mut body.blocks.get_mut(block).unwrap().term.kind, TermKind::Unreachable);
    let next = body.blocks.alloc(Block {
        lexical_scope: body.blocks.get(block).unwrap().lexical_scope, params: vec![], unwind: None, insts: vec![],
        term: Terminator { kind: term, loc: None },
    }).unwrap();
    let call = CallSite { cleanup: CallCleanupMode::Ordinary, args: vec![], packet: None, deps: vec![],
        descriptors: vec![], type_args: vec![], summary: module.functions.get(closer).unwrap().summary };
    function_mut(&mut module, "risky").blocks.get_mut(block).unwrap().term.kind = TermKind::InvokeDefer {
        function: closer, call, continue_edge: InvokeEdge { block: next, args: vec![] }, replacements: vec![],
        unwind: None,
    };
    let error = lower_cleanup(module, &LowOptions::default()).unwrap_err();
    assert_eq!(error, LowError::Unsupported { function: risky, message: eligibility::ASYNC_DEFER.into() });
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
    let (name, coro, _) = GOLDENS[PLAIN];
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
/// failure. Dropping the commit of `work` on the not-delivered path leaves its reserve unconsumed.
#[test]
fn the_pending_exit_after_a_conditional_delivery_is_checked() {
    let (_, coro, _) = GOLDENS[PLAIN];
    let commit = "    (%15: int, %16: own, %17: deps) = exit.commit %13, destination caller, result int\n    \
        fut.publish %1, success(value=%15, packet=%16, deps=%17)\n";
    let dropped = edited(coro, commit, "    %16: own = own.empty\n    %17: deps = deps.of %14\n    \
        fut.publish %1, success(value=%14, packet=%16, deps=%17)\n");
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
    let (name, coro, _) = GOLDENS[PLAIN];
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

/// D20 §2.6, D24.2: a close that holds a plain Future neither suspends nor fails in Low when nothing else does,
/// so the exact envelopes that own emits (`failures=[]`, `suspends=false`) verify, for a sync and an async owner.
#[test]
fn a_plain_future_needs_no_suspension_or_failure_at_its_close() {
    for index in [NEVER, PLAIN] {
        let (name, coro, _) = GOLDENS[index];
        let output = lowered(name, coro);
        let main = function_id(&output, "main");
        let body = output.functions.get(main).unwrap().body.as_ref().unwrap();
        let settled: Vec<CleanupCloseId> = body.cleanup.schedules.iter()
            .filter(|(_, schedule)| !schedule.holders.is_empty()).map(|(_, schedule)| schedule.close).collect();
        assert!(!settled.is_empty(), "{name}");
        for close in settled {
            let effects = &body.cleanup.closes.get(close).unwrap().effects;
            assert!(effects.failures.is_empty() && !effects.suspends, "{name}: {effects:?}");
        }
    }
}

/// B10.6, D16: a static duty inventory names its Services protocol; the runtime dictionary is optional.
#[test]
fn a_duty_schedule_names_its_protocol() {
    let (name, coro, _) = GOLDENS[NEVER];
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
    let (name, coro, _) = GOLDENS[NEVER];
    let output = lowered(name, coro);
    let main = function_id(&output, "main");
    let body = output.functions.get(main).unwrap().body.as_ref().unwrap();
    let (_, settle) = body.insts.iter().find(|(_, inst)| matches!(inst.op,
        Operation::Cleanup(CleanupOp::Settle { .. }))).unwrap();
    let contract = ContractRegistry::new(&output).lookup(&settle.op).unwrap();
    assert!(!contract.effects.suspends && contract.effects.failures.is_empty() && contract.unwind.is_none());
}

/// D20 §2.6 narrows only the settle of a Future: its outcome identities keep the cleanup effects of the result
/// type, so a Future of a resource whose destroy throws needs those failures in the exact envelope of its close.
#[test]
fn a_future_keeps_the_cleanup_effects_of_its_outcome() {
    let (_, coro, _) = GOLDENS[NEVER];
    let source = edited(coro, "scope S0 action in @explode\n", "object Res {\n    destroy @destroy\n}\n\
        summary Md internal parameters [parameter_effect(convention=borrow, destination=none)] contract \
        call_contract(failures=[thrown], publication=atomic)\n\
        func @destroy(%self: Res) -> void [extern \"fixture.destroy\", destroy Res] summary(Md);\n\
        scope S0 action in @explode\n");
    let source = edited(&source, "(%7: future<int> facts(managed(deps=d0))",
        "(%7: future<Res> facts(managed(deps=d0))");
    let source = edited(&source, "environment deferred(function=@explode), result int, owner S1",
        "environment deferred(function=@explode), result Res, owner S1");
    let output = lower_cleanup(parsed(&source), &LowOptions::default()).unwrap();
    assert!(rules(&output).contains(&RuleId::B104), "{:?}", rules(&output));
}

/// Champ draft 3 §5: the selector of the second plan decides an open delivery; a finish reached without it would
/// dispatch only one of the two outcomes.
#[test]
fn an_open_delivery_is_decided_before_finish() {
    let (name, coro, _) = GOLDENS[PLAIN];
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

/// C-3 (#695): a settle step never takes the task or the receipt of an await unwind edge; those reach only
/// `await.unwind`. Here the unwind close of main settles them instead of the adapter consuming them.
#[test]
fn a_settle_step_never_takes_an_await_task() {
    let (name, coro, _) = GOLDENS[PLAIN];
    let mut output = lowered(name, coro);
    let main = function_id(&output, "main");
    let mut body = output.functions.get_mut(main).unwrap().body.take().unwrap();
    let (block, position) = body.blocks.iter().find_map(|(id, data)| data.insts.iter().position(|inst| {
        matches!(body.insts.get(*inst).unwrap().op, Operation::Async(AsyncOp::AwaitUnwind { .. }))
    }).map(|position| (id, position))).unwrap();
    let insts = body.blocks.get(block).unwrap().insts.clone();
    let save = body.insts.get(insts[position - 1]).unwrap().clone();
    let unwind = body.insts.get(insts[position]).unwrap().clone();
    let resume = body.insts.get(insts[position + 1]).unwrap().clone();
    let Operation::Async(AsyncOp::FrameSave { frame, slots, .. }) = &save.op else { panic!("a save brackets it") };
    let Operation::Async(AsyncOp::AwaitUnwind { state, failure, task, receipt, .. }) = unwind.op else {
        unreachable!();
    };
    let cache = slots[0].value;
    let mut editor = FunctionEditor::new(&mut output, &mut body);
    for (old, new) in [(resume.results[0], *frame), (resume.results[1], cache), (unwind.results[0], state),
        (unwind.results[1], failure)] {
        editor.replace_uses(old, new).unwrap();
    }
    body.blocks.get_mut(block).unwrap().insts.drain(position - 1..position + 2);
    // The bracket of await.unwind is the last continuation of main; it goes with the adapter.
    let site = SuspendSite::Instruction(insts[position]);
    body.frame_layout.as_mut().unwrap().continuations.retain(|continuation| continuation.site != site);
    let settle = body.blocks.get(block).unwrap().insts[position];
    let Operation::Cleanup(CleanupOp::Settle { cleanup: begun, protocol, .. }) = body.insts.get(settle).unwrap().op
    else {
        panic!("the begin of the unwind close is followed by its settle");
    };
    let ty = body.values.get(begun).unwrap().ty;
    let mut previous = begun;
    let mut steps = Vec::new();
    for holder in [task, receipt] {
        let inst = body.insts.alloc(Instruction {
            results: vec![], op: Operation::Cleanup(CleanupOp::Settle { cleanup: previous, holder, protocol }),
            facts: InstructionFacts::default(), loc: None,
        }).unwrap();
        previous = body.values.alloc(ValueData {
            ty, def: Definition::InstResult { inst, index: 0 }, facts: ValueFacts::Service,
        }).unwrap();
        body.insts.get_mut(inst).unwrap().results = vec![previous];
        steps.push(inst);
    }
    let Operation::Cleanup(CleanupOp::Settle { cleanup, .. }) = &mut body.insts.get_mut(settle).unwrap().op else {
        unreachable!();
    };
    *cleanup = previous;
    let data = body.blocks.get_mut(block).unwrap();
    data.insts.splice(position..position, steps);
    for (_, schedule) in body.cleanup.schedules.iter_mut() {
        if schedule.holders == [cache] {
            schedule.holders = vec![task, receipt, cache];
        }
    }
    body.cleanup.roots = Arena::new();
    compact::compact(&mut body).unwrap();
    output.functions.get_mut(main).unwrap().body = Some(body);
    rebuild_roots(&mut output, "main");
    let body = output.functions.get(main).unwrap().body.as_ref().unwrap();
    let task_state = Ok(&Type::Service(ServiceType::TaskState));
    let first = body.insts.iter().find(|(_, inst)| matches!(inst.op,
        Operation::Cleanup(CleanupOp::Settle { holder, .. })
            if body.values.get(holder).is_ok_and(|value| output.types.get(value.ty) == task_state)))
        .map(|(id, _)| id).unwrap();
    let found = diagnostics(&output).into_iter().any(|diagnostic| diagnostic.rule == RuleId::B91
        && diagnostic.primary.entity == EntityLocation::Instruction { function: main, instruction: first });
    assert!(found, "{:?}", diagnostics(&output));
}

const K1_CANCEL: usize = 16;
const K1_ESCAPES: usize = 17;
const K1_NESTED: usize = 18;

/// The scope closes of a body: each close, its scope, whether it runs on a failure and whether it delivers.
fn closes_of(body: &Body) -> Vec<(CleanupCloseId, ScopeId, bool, bool)> {
    body.cleanup.closes.iter().filter_map(|(id, close)| match close.source {
        CleanupSource::Scope(scope) => Some((id, scope, close.active_failure, close.effects.cancel)),
        CleanupSource::Packet(_) => None,
    }).collect()
}

/// The close an instruction of a cleanup chain belongs to, through its cleanup-state operand back to the begin.
fn close_of(body: &Body, mut cleanup: ValueId) -> CleanupCloseId {
    loop {
        match body.values.get(cleanup).unwrap().def {
            Definition::InstResult { inst, .. } => match &body.insts.get(inst).unwrap().op {
                Operation::Cleanup(CleanupOp::Begin { close, .. }) => return *close,
                Operation::Cleanup(CleanupOp::Settle { cleanup: previous, .. }
                    | CleanupOp::SettleEmpty { cleanup: previous, .. } | CleanupOp::Plan { cleanup: previous, .. }
                    | CleanupOp::DeliverCancel { cleanup: previous }) => cleanup = *previous,
                other => panic!("{other:?}"),
            },
            Definition::BlockParam { block, index } => {
                let (_, source) = body.blocks.iter().find_map(|(id, data)| {
                    let arguments: Vec<_> = match &data.term.kind {
                        TermKind::Br(target) if target.block == block => {
                            target.args.iter().copied().map(EdgeArg::Value).collect()
                        }
                        kind => dispatch_edges(kind).into_iter().find(|(_, edge)| edge.block == block)
                            .map(|(_, edge)| edge.args.clone()).unwrap_or_default(),
                    };
                    match arguments.get(index as usize) {
                        Some(EdgeArg::Value(value)) => Some((id, *value)),
                        Some(EdgeArg::Produced(_)) => match &data.term.kind {
                            TermKind::CleanupNext { cleanup, .. } | TermKind::FailureDispatch { failure: cleanup, .. }
                                => Some((id, *cleanup)),
                            _ => None,
                        },
                        None => None,
                    }
                }).unwrap();
                cleanup = source;
            }
        }
    }
}

/// K1 (D18, elaboration §13.4): a nested close keeps a pending cancellation for the next await. The successful
/// close of the branch block S2 settles the duty of g, declares no cancellation and delivers none; no failing
/// close delivers either. The final close keeps the D18 form of the IR, which the K2 ruling leaves for #727
/// (at run time it delivers nothing), so this test does not state it.
#[test]
fn k1_a_nested_close_keeps_the_cancellation_for_the_next_await() {
    let (name, coro, _) = GOLDENS[K1_NESTED];
    assert_eq!(name, "k1-nested-close-next-await");
    let output = lowered(name, coro);
    let main = function_id(&output, "main");
    let body = output.functions.get(main).unwrap().body.as_ref().unwrap();
    let closes = closes_of(body);
    let nested: Vec<_> = closes.iter().filter(|(_, scope, failing, _)| {
        output.scopes.get(*scope).unwrap().parent.is_some() && !failing
    }).collect();
    assert_eq!(nested.len(), 1, "{closes:?}");
    assert!(!nested[0].3, "a nested close declares no cancellation");
    let delivering: std::collections::BTreeSet<_> = body.insts.iter().filter_map(|(_, inst)| match inst.op {
        Operation::Cleanup(CleanupOp::DeliverCancel { cleanup }) => Some(close_of(body, cleanup)),
        _ => None,
    }).collect();
    let kept: std::collections::BTreeSet<_> = closes.iter().filter(|(_, scope, failing, _)| {
        output.scopes.get(*scope).unwrap().parent.is_some() || *failing
    }).map(|(close, ..)| *close).collect();
    assert!(!kept.is_empty());
    assert!(delivering.is_disjoint(&kept), "{closes:?}");
    // The duty of g is settled by the nested close, before the await of f.
    let schedule = body.cleanup.schedules.iter().find(|(_, schedule)| schedule.close == nested[0].0).unwrap().1;
    assert_eq!(schedule.holders.len(), 1);
}

/// K1 (D18): delivering in the nested close of S2 is rejected; the pending cancellation belongs to the next await.
#[test]
fn k1_a_nested_close_that_delivers_is_rejected() {
    let (name, coro, _) = GOLDENS[K1_NESTED];
    let mut output = lowered(name, coro);
    let scopes = output.scopes.clone();
    let body = function_mut(&mut output, "main");
    let nested = body.cleanup.closes.iter().find(|(_, close)| !close.active_failure && match close.source {
        CleanupSource::Scope(scope) => scopes.get(scope).unwrap().parent.is_some(),
        CleanupSource::Packet(_) => false,
    }).map(|(id, _)| id).unwrap();
    let (plan, settled) = body.insts.iter().find_map(|(id, inst)| match inst.op {
        Operation::Cleanup(CleanupOp::Plan { cleanup, .. }) if close_of(body, cleanup) == nested => {
            Some((id, cleanup))
        }
        _ => None,
    }).unwrap();
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
    rebuild_roots(&mut output, "main");
    let main = function_id(&output, "main");
    let found = diagnostics(&output).into_iter().any(|diagnostic| diagnostic.rule == RuleId::LowCancelOutsideFinalExit
        && diagnostic.primary.entity == EntityLocation::Instruction { function: main, instruction: deliver });
    assert!(found, "{:?}", diagnostics(&output));
}

/// K1, elaboration §13.4: the failure of `await f` escapes the Future-owning action main. The failing close
/// after `await.unwind` begins with the adapter's failure, settles the cache duty of f and finishes; main
/// publishes the failure the close returns, which closing the duty did not invalidate.
#[test]
fn k1_the_failure_view_outlives_the_cache_duty_it_came_from() {
    let (name, coro, _) = GOLDENS[K1_ESCAPES];
    assert_eq!(name, "k1-failure-view-escapes");
    let output = lowered(name, coro);
    let main = function_id(&output, "main");
    let body = output.functions.get(main).unwrap().body.as_ref().unwrap();
    let adapted: Vec<ValueId> = body.insts.iter()
        .filter(|(_, inst)| matches!(inst.op, Operation::Async(AsyncOp::AwaitUnwind { .. })))
        .map(|(_, inst)| inst.results[1]).collect();
    assert_eq!(adapted.len(), 1);
    let close = body.insts.iter().find_map(|(_, inst)| match inst.op {
        Operation::Cleanup(CleanupOp::Begin { failure: Some(failure), close, .. }) if failure == adapted[0] => {
            Some(close)
        }
        _ => None,
    }).expect("the failing close begins with the failure that await.unwind returns");
    assert!(!body.cleanup.closes.get(close).unwrap().effects.cancel, "a failing close never delivers");
    let settles = body.insts.iter().filter(|(_, inst)| matches!(inst.op,
        Operation::Cleanup(CleanupOp::Settle { cleanup, .. }) if close_of(body, cleanup) == close)).count();
    assert_eq!(settles, 1, "the close settles the cache duty of f");
    assert!(body.blocks.iter().any(|(_, block)| block.insts.iter().any(|inst| matches!(
        body.insts.get(*inst).unwrap().op,
        Operation::Async(AsyncOp::FuturePublish { outcome: PublishOutcome::Failure(_), .. })))));
}

/// K1 (elaboration §13.1, Champ draft 3 §5): cancelling a cold Future twice before it starts leaves its cache
/// duty with its owner. Every close of main settles that duty once.
#[test]
fn k1_a_cancelled_cold_future_keeps_its_duty_until_the_close() {
    let (name, coro, _) = GOLDENS[K1_CANCEL];
    assert_eq!(name, "k1-cancel-unstarted");
    let output = lowered(name, coro);
    let main = function_id(&output, "main");
    let body = output.functions.get(main).unwrap().body.as_ref().unwrap();
    let cancels = body.insts.iter().filter(|(_, inst)| matches!(inst.op,
        Operation::Async(AsyncOp::FutureCancel { .. }))).count();
    assert_eq!(cancels, 2);
    let cache_states = body.insts.iter().filter_map(|(_, inst)| match inst.op {
        Operation::Async(AsyncOp::FutureNew { .. }) => Some(inst.results[1]),
        _ => None,
    }).count();
    assert_eq!(cache_states, 1);
    for (close, ..) in closes_of(body) {
        let schedule = body.cleanup.schedules.iter().find(|(_, schedule)| schedule.close == close).unwrap().1;
        let settles = body.insts.iter().filter(|(_, inst)| matches!(inst.op,
            Operation::Cleanup(CleanupOp::Settle { cleanup, .. }) if close_of(body, cleanup) == close)).count();
        assert_eq!(settles, schedule.holders.len());
    }
}

/// K1 (elaboration §13.3, C-3): the cancel edge of an await that skips `await.unwind` drops its task and
/// receipt; the abstract close that would take them is rejected in Low as in Coro.
#[test]
fn k1_a_cancel_edge_without_its_adapter_is_rejected_in_low() {
    for source in [include_str!("../../tests/fixtures/async/coro-invalid/k1-cancel-edge-drops-task.oir"),
        include_str!("../../tests/fixtures/async/coro-invalid/k1-stale-await-failure.oir")] {
        let output = lower_cleanup(parsed(source), &LowOptions::default()).unwrap();
        assert!(rules(&output).contains(&RuleId::B91), "{:?}", diagnostics(&output));
    }
}

/// #690, plan §3.4, Champ draft 3 §5: the shield is a property of every close, not an operation, so Low has no
/// `cleanup.shield` step. The close of `work` verifies without one, and the text form no longer has the opcode.
#[test]
fn the_shield_is_a_property_of_every_close() {
    let (_, _, golden) = GOLDENS[PLAIN];
    let mut lines: Vec<String> = golden.lines().map(str::to_owned).collect();
    let begin = lines.iter().position(|line| line.contains("= cleanup.begin ")).unwrap();
    let (name, _) = lines[begin].trim_start().split_once(':').unwrap();
    let name = name.rsplit(' ').next().unwrap().to_owned();
    // The step goes after the whole begin, whose continuation lines are indented further.
    let after = (begin + 1..lines.len()).find(|index| !lines[*index].starts_with("        ")).unwrap();
    lines.insert(after, format!("    %shielded: cleanup-state = cleanup.shield {name}"));
    let shielded = lines.join("\n");
    let error = text::parse(&shielded).unwrap_err();
    assert_eq!(error.kind, text::DiagnosticKind::Syntax, "{error}");
    assert!(error.to_string().contains("unknown opcode"), "{error}");
    assert!(diagnostics(&parsed(golden)).is_empty());
}

const OUTER_FUTURE: usize = 19;

/// The cache-state result of the `fut.new` whose owner is `owner`, in a Low body.
fn cache_state_of(body: &Body, owner: ScopeId) -> Vec<ValueId> {
    body.insts.iter().filter_map(|(_, inst)| match inst.op {
        Operation::Async(AsyncOp::FutureNew { owner: scope, .. }) if scope == owner => Some(inst.results[1]),
        _ => None,
    }).collect()
}

/// #684, D16: a close settles exactly the duties of the scope it closes. In nested-close-outer-future the
/// successful close of the branch block S2 settles the duty of g while the duty of f, owned by main's root
/// scope, passes through it; the final close of main settles f.
#[test]
fn a_close_settles_only_the_duties_of_its_own_scope() {
    let (name, coro, _) = GOLDENS[OUTER_FUTURE];
    assert_eq!(name, "nested-close-outer-future");
    let output = lowered(name, coro);
    let main = function_id(&output, "main");
    let body = output.functions.get(main).unwrap().body.as_ref().unwrap();
    let closes = closes_of(body);
    assert!(closes.iter().any(|(_, scope, failing, _)| {
        !failing && output.scopes.get(*scope).unwrap().parent.is_some()
    }));
    for (close, scope, failing, _) in closes {
        // S2 created g and S1 created f: a successful close schedules the one duty of its own scope and never the
        // other; a failing close schedules at most that duty (a failure before the creation has none).
        let schedule = body.cleanup.schedules.iter().find(|(_, schedule)| schedule.close == close).unwrap().1;
        let created = cache_state_of(body, scope).len();
        if failing {
            assert!(schedule.holders.len() <= created, "{scope:?}");
        } else {
            assert_eq!(schedule.holders.len(), created, "{scope:?}");
        }
        let settles = body.insts.iter().filter(|(_, inst)| matches!(inst.op,
            Operation::Cleanup(CleanupOp::Settle { cleanup, .. }) if close_of(body, cleanup) == close)).count();
        assert_eq!(settles, schedule.holders.len(), "{scope:?}");
    }
}

/// #684: the nested close of S2 that names the outer duty of f in its schedule is rejected at its begin: the
/// begin takes only the duties of S2. Before #684 the begin took every live duty and accepted this schedule.
#[test]
fn a_nested_close_that_schedules_an_outer_duty_is_rejected() {
    let (_, coro, _) = GOLDENS[OUTER_FUTURE];
    let mut output = lower_cleanup(parsed(coro), &LowOptions::default()).unwrap();
    let scopes = output.scopes.clone();
    let main = function_id(&output, "main");
    let body = function_mut(&mut output, "main");
    let (nested, branch) = body.cleanup.closes.iter().find_map(|(id, close)| match close.source {
        CleanupSource::Scope(scope) if !close.active_failure && scopes.get(scope).unwrap().parent.is_some() => {
            Some((id, scope))
        }
        _ => None,
    }).unwrap();
    let root = scopes.get(branch).unwrap().parent.unwrap();
    let begin = body.insts.iter().find_map(|(id, inst)| match inst.op {
        Operation::Cleanup(CleanupOp::Begin { close, .. }) if close == nested => Some(id),
        _ => None,
    }).unwrap();
    // The outer duty as the nested close sees it: g's holder and f's cache-state come back from the same resume
    // of the await of g, which saved both.
    let cache_type = body.values.get(cache_state_of(body, root)[0]).unwrap().ty;
    let schedule = body.cleanup.schedules.iter().find(|(_, schedule)| schedule.close == nested).unwrap().0;
    let own = body.cleanup.schedules.get(schedule).unwrap().holders.clone();
    assert_eq!(own.len(), 1, "the nested close settles the duty of g");
    let Definition::InstResult { inst: resume, .. } = body.values.get(own[0]).unwrap().def else {
        panic!("g's duty reaches the nested close from the resume of its await");
    };
    let holder = body.insts.get(resume).unwrap().results.iter().copied().find(|value| {
        *value != own[0] && body.values.get(*value).unwrap().ty == cache_type
    }).expect("the resume of the await of g restores the duty of f");
    body.cleanup.schedules.get_mut(schedule).unwrap().holders.push(holder);
    let found = diagnostics(&output).into_iter().any(|diagnostic| diagnostic.rule == RuleId::B106
        && diagnostic.primary.entity == EntityLocation::Instruction { function: main, instruction: begin });
    assert!(found, "{:?}", diagnostics(&output));
}
