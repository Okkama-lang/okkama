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

// --- #601 D25: returned and rehomed Futures (R1's own output through coro) ------------------------------------

/// R1's own goldens whose Futures leave their owner through `cache.rehome` and reach a scope that admits them
/// (async ABI C-4 and its ruling, D25.1, D25.3).
const ESCAPES: &[(&str, &str)] = &[
    ("r1-make-escape", include_str!("../own/tests/async-goldens/r1-make-escape.oir")),
    ("r1-cold-escape", include_str!("../own/tests/async-goldens/r1-cold-escape.oir")),
    ("r1-view-escape", include_str!("../own/tests/async-goldens/r1-view-escape.oir")),
    ("r1-loop-escape", include_str!("../own/tests/async-goldens/r1-loop-escape.oir")),
];

/// own → coro → verify(coro), the input of `lowered`.
fn coro_of(name: &str, source: &str) -> String {
    let coro = crate::coro::lower_coro(parsed(source), &Default::default())
        .unwrap_or_else(|error| panic!("{name}: coro {error}"));
    text::print(&coro).unwrap()
}

fn ops<'b>(body: &'b Body, block: BlockId) -> impl Iterator<Item = (InstId, &'b Operation)> + 'b {
    body.blocks.get(block).unwrap().insts.iter().map(move |inst| (*inst, &body.insts.get(*inst).unwrap().op))
}

/// The cleanup steps of each close, in order, from its `cleanup.begin` to its first plan.
fn close_steps(body: &Body) -> Vec<(CleanupCloseId, Vec<&Operation>)> {
    let mut closes = Vec::new();
    for (block, _) in body.blocks.iter() {
        let mut current: Option<(CleanupCloseId, Vec<&Operation>)> = None;
        for (_, op) in ops(body, block) {
            match op {
                Operation::Cleanup(CleanupOp::Begin { close, .. }) => current = Some((*close, vec![op])),
                Operation::Cleanup(CleanupOp::Plan { .. }) => {
                    if let Some((close, mut steps)) = current.take() {
                        steps.push(op);
                        closes.push((close, steps));
                    }
                }
                Operation::Cleanup(_) => {
                    if let Some((_, steps)) = current.as_mut() {
                        steps.push(op);
                    }
                }
                _ => {}
            }
        }
    }
    closes
}

/// D25.3: every close of a scope that admits a returned or rehomed Future settles the static duties, then its
/// entries in one dynamic step, then plans through the executor; the other closes have no dynamic step. The
/// rehome of a duty created here takes its token (C-1, C-4); the rehome of an admitted entry keeps the state form.
#[test]
fn escaping_futures_settle_where_they_are_admitted() {
    for (name, own) in ESCAPES {
        let low = lowered(name, &coro_of(name, own));
        let mut dynamic = 0;
        for (_, function) in low.functions.iter() {
            let Some(body) = &function.body else { continue };
            for (close, steps) in close_steps(body) {
                let schedule = body.cleanup.schedules.iter().find(|(_, schedule)| schedule.close == close).unwrap().1;
                let settles: Vec<_> = steps.iter().filter(|op| matches!(op,
                    Operation::Cleanup(CleanupOp::SettleDynamic { .. }))).collect();
                assert_eq!(settles.len(), usize::from(schedule.dynamic), "{name}: close {close:?}");
                if schedule.dynamic {
                    dynamic += 1;
                    let position = steps.iter().position(|op| matches!(op,
                        Operation::Cleanup(CleanupOp::SettleDynamic { .. }))).unwrap();
                    assert_eq!(position, steps.len() - 2, "{name}: the dynamic step comes right before the plan");
                    assert!(matches!(steps[position - 1], Operation::Cleanup(CleanupOp::Settle { .. }
                        | CleanupOp::SettleEmpty { .. })), "{name}: after the static duties");
                    let Operation::Cleanup(CleanupOp::Plan { plan, .. }) = steps[steps.len() - 1] else {
                        unreachable!()
                    };
                    assert!(body.cleanup.plans.get(*plan).unwrap().direct.is_none(), "{name}: the executor plans");
                }
            }
        }
        assert!(dynamic > 0, "{name}: a scope admits the escaped Future");
    }
    let make = lowered("r1-make-escape", &coro_of("r1-make-escape", ESCAPES[0].1));
    let forms: Vec<_> = make.functions.iter().filter_map(|(_, function)| function.body.as_ref()).flat_map(|body| {
        body.insts.iter().filter_map(|(_, inst)| match &inst.op {
            Operation::Async(AsyncOp::CacheRehome { form, target, .. }) => Some((form.clone(), *target)),
            _ => None,
        }).collect::<Vec<_>>()
    }).collect();
    // @make returns its Future with its token; main's block passes the admitted Future on in the own form.
    assert!(forms.iter().any(|(form, target)| matches!(form, RehomeForm::Token { task: None, .. })
        && *target == RehomeTarget::Caller), "{forms:?}");
    assert!(forms.iter().any(|(form, target)| matches!(form, RehomeForm::Scope { .. })
        && matches!(target, RehomeTarget::Scope(_))), "{forms:?}");
}

/// The Low form of r1-cold-escape and the body of `main`, for the negatives of the dynamic step.
fn cold_escape() -> (Module, FuncId) {
    let low = lowered("r1-cold-escape", &coro_of("r1-cold-escape", ESCAPES[1].1));
    let main = function_id(&low, "main");
    (low, main)
}

/// The first close of `main` whose schedule has the dynamic step, its dynamic settle and the step before it.
fn dynamic_close(module: &Module, main: FuncId) -> (CleanupCloseId, InstId, InstId) {
    let body = module.functions.get(main).unwrap().body.as_ref().unwrap();
    for (block, _) in body.blocks.iter() {
        let insts: Vec<_> = ops(body, block).collect();
        for (index, (inst, op)) in insts.iter().enumerate() {
            if let Operation::Cleanup(CleanupOp::SettleDynamic { cleanup, .. }) = op {
                let begin = insts.iter().find_map(|(_, op)| match op {
                    Operation::Cleanup(CleanupOp::Begin { close, .. }) => Some(*close),
                    _ => None,
                }).unwrap();
                let _ = cleanup;
                return (begin, insts[index - 1].0, *inst);
            }
        }
    }
    panic!("no dynamic close");
}

fn schedule_of(body: &mut Body, close: CleanupCloseId) -> &mut CleanupSchedule {
    body.cleanup.schedules.iter_mut().find(|(_, schedule)| schedule.close == close).unwrap().1
}

fn has_rule(module: &Module, message: &str) -> bool {
    diagnostics(module).iter().any(|diagnostic| diagnostic.rule == RuleId::B106 && diagnostic.message.contains(message))
}

#[test]
fn a_dynamic_close_settles_its_entries_before_its_plan() {
    let (mut module, main) = cold_escape();
    assert!(diagnostics(&module).is_empty());
    // A close whose schedule has no dynamic step gets one: its plan comes without the step.
    let protocol = module.protocols.iter().next().unwrap().0;
    let body = function_mut(&mut module, "main");
    let close = body.cleanup.schedules.iter().find(|(_, schedule)| !schedule.dynamic).unwrap().1.close;
    schedule_of(body, close).dynamic = true;
    schedule_of(body, close).protocol = Some(protocol);
    let _ = main;
    assert!(has_rule(&module, "settles them before its plan"), "{:?}", diagnostics(&module));
}

#[test]
fn a_dynamic_settle_belongs_to_a_dynamic_schedule() {
    let (mut module, main) = cold_escape();
    let (close, _, _) = dynamic_close(&module, main);
    schedule_of(function_mut(&mut module, "main"), close).dynamic = false;
    assert!(has_rule(&module, "belongs to a close"), "{:?}", diagnostics(&module));
}

#[test]
fn a_dynamic_settle_follows_the_static_duties() {
    let (mut module, main) = cold_escape();
    let (close, before, settle) = dynamic_close(&module, main);
    let body = function_mut(&mut module, "main");
    let Operation::Cleanup(CleanupOp::SettleEmpty { cleanup: first, schedule }) = body.insts.get(before).unwrap().op
    else {
        panic!("the dynamic close of main has no static duty");
    };
    let Operation::Cleanup(CleanupOp::SettleDynamic { cleanup: second, protocol }) =
        body.insts.get(settle).unwrap().op else { unreachable!() };
    body.insts.get_mut(before).unwrap().op = Operation::Cleanup(CleanupOp::SettleDynamic { cleanup: first, protocol });
    body.insts.get_mut(settle).unwrap().op = Operation::Cleanup(CleanupOp::SettleEmpty { cleanup: second, schedule });
    let _ = close;
    assert!(has_rule(&module, "follows the static duties"), "{:?}", diagnostics(&module));
}

/// Insert a second dynamic settle right after `settle`, which the plan then takes.
fn settle_again(body: &mut Body, settle: InstId) -> InstId {
    let Operation::Cleanup(CleanupOp::SettleDynamic { protocol, .. }) = body.insts.get(settle).unwrap().op
    else { unreachable!() };
    let first = body.insts.get(settle).unwrap().results[0];
    let ty = body.values.get(first).unwrap().ty;
    let again = body.insts.alloc(Instruction { results: vec![], facts: InstructionFacts::default(), loc: None,
        op: Operation::Cleanup(CleanupOp::SettleDynamic { cleanup: first, protocol }) }).unwrap();
    let second = body.values.alloc(ValueData { ty, def: Definition::InstResult { inst: again, index: 0 },
        facts: ValueFacts::Service }).unwrap();
    body.insts.get_mut(again).unwrap().results = vec![second];
    let (block, position) = body.blocks.iter().find_map(|(block, data)| {
        data.insts.iter().position(|inst| *inst == settle).map(|position| (block, position))
    }).unwrap();
    let next = body.blocks.get(block).unwrap().insts[position + 1];
    let Operation::Cleanup(CleanupOp::Plan { cleanup, .. }) = &mut body.insts.get_mut(next).unwrap().op else {
        panic!("the plan follows the dynamic settle");
    };
    *cleanup = second;
    body.blocks.get_mut(block).unwrap().insts.insert(position + 1, again);
    again
}

#[test]
fn a_close_settles_its_dynamic_entries_once() {
    let (mut module, main) = cold_escape();
    let (_, _, settle) = dynamic_close(&module, main);
    settle_again(function_mut(&mut module, "main"), settle);
    rebuild_roots(&mut module, "main");
    assert!(has_rule(&module, "dynamic entries once"), "{:?}", diagnostics(&module));
}

#[test]
fn dynamically_settled_entries_need_the_executor() {
    let (mut module, main) = cold_escape();
    let (close, _, _) = dynamic_close(&module, main);
    let body = function_mut(&mut module, "main");
    let state = body.values.iter().next().unwrap().0;
    let proof = body.cleanup.proofs.alloc(CleanupOrderProof { state, slots: vec![], preserves: vec![], joins: vec![],
        suffixes: vec![], reserve_empty: true }).unwrap();
    for (_, plan) in body.cleanup.plans.iter_mut().filter(|(_, plan)| plan.close == close) {
        plan.direct = Some(proof);
    }
    assert!(has_rule(&module, "executor plan"), "{:?}", diagnostics(&module));
}
