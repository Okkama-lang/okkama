use super::*;

fn parsed(source: &str) -> Module {
    text::parse(source).unwrap_or_else(|error| panic!("{error}"))
}

fn options() -> VerifyOptions {
    VerifyOptions { execution: ExecutionMode::Tests, ..VerifyOptions::default() }
}

/// A coro golden whose input is the own golden that V2's own pass produces from its High input (ruling 1).
macro_rules! golden {
    ($name:literal) => {
        ($name, include_str!(concat!("../own/tests/async-goldens/", $name, ".oir")),
            include_str!(concat!("../../tests/fixtures/async/coro/", $name, ".oir")))
    };
}

/// A coro golden whose input is a V2-shaped variant of an own golden: a form own does not produce yet.
macro_rules! variant {
    ($name:literal) => {
        ($name, include_str!(concat!("../../tests/fixtures/async/own/", $name, ".oir")),
            include_str!(concat!("../../tests/fixtures/async/coro/", $name, ".oir")))
    };
}

/// The inputs of the coro goldens: V2's own goldens and the V2-shaped variants.
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
    variant!("coro-start-descriptors"),
    // R2: a resource held across an await; a temporary take whose proof coro rechecks.
    variant!("r2-resource-across-await"),
    variant!("r2-take-proof-recheck"),
    // R2, C-6: async defers whose entry awaits, after a body failure and across a pending exit (own's output, #730).
    golden!("r2-defer-await-body-failure"),
    golden!("r2-defer-await-reserve"),
    // R2: closes that suspend while the body owns a resource.
    variant!("r2-resource-suspending-close"),
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
    // #601 P1 (D24.1): spawn forwards the cache duty and adds the task duty, which settles first (§6.3).
    golden!("p1-spawn-awaited"),
    golden!("p1-spawn-detached"),
    // D26 (#857): duty slots. A temporary created in one branch of its owner, a handler shared by a call before and
    // after a creation, a Future created and spawned in one branch.
    variant!("d857-branch-temporary"),
    variant!("d857-shared-handler"),
    variant!("d857-branch-spawn"),
    // A suspension between the begin and the creation, a rehome in a slotted scope, a slotted owner that admits
    // entries at run time (absorbs on both outcomes of a store).
    variant!("d857-slot-across-await"),
    variant!("d857-rehome-slotted"),
    variant!("d857-stored-dynamic-slotted"),
    golden!("p1-sync-owner-spawn"),
    // C-4: a returned spawned Future's rehome takes its task duty with its cache duty (token form, `task`).
    golden!("p1-spawn-returned"),
    // #771 (D25.2): a fresh Future stored in a field; its token is absorbed into the constructor's packet (PR-B1).
    variant!("d25-stored-cold"),
    // #759: a Future created in a `for` body, also spawned; the body's close settles each iteration's duties.
    variant!("d759-loop-body-future"),
    variant!("d759-loop-body-spawn"),
    // #849: a Future created in a `map` body (the #831 accumulator loop), awaited or abandoned; the builder's
    // cleanup is not shared across the creation.
    variant!("d849-map-body-await"),
    variant!("d849-map-body-abandon"),
];

/// Own → verify(own) → coro → verify(coro) (OIR-2 §12).
pub(super) fn lowered(name: &str, source: &str) -> Module {
    let module = parsed(source);
    verify_own_with(&module, &options(), VerifyContext::default())
        .unwrap_or_else(|report| panic!("{name}: own {report:?}"));
    let output = lower_coro(module, &CoroOptions::default()).unwrap_or_else(|error| panic!("{name}: {error}"));
    verify_coro_with(&output, &options(), VerifyContext::default())
        .unwrap_or_else(|report| panic!("{name}: coro {report:?}\n{}", text::print(&output).unwrap()));
    let printed = text::print(&output).unwrap();
    let reparsed = parsed(&printed);
    assert!(text::normalized_eq(&output, &reparsed).unwrap(), "{name}: unstable text");
    output
}

#[test]
fn coro_goldens_of_the_plain_async_inputs() {
    for (name, own, golden) in GOLDENS {
        let output = lowered(name, own);
        let expected = parsed(golden);
        verify_coro_with(&expected, &options(), VerifyContext::default())
            .unwrap_or_else(|report| panic!("{name}: golden {report:?}"));
        assert!(text::normalized_eq(&output, &expected).unwrap(), "{name}\n{}\nexpected:\n{}",
            text::print(&output).unwrap(), text::print(&expected).unwrap());
    }
}

const PLAIN: &str = include_str!("../own/tests/async-goldens/async-plain-await.oir");
const VOID: &str = include_str!("../own/tests/async-goldens/async-void.oir");
const NESTED: &str = include_str!("../own/tests/async-goldens/coro-nested-cold-await.oir");

fn edited(source: &str, from: &str, to: &str) -> String {
    assert_eq!(source.matches(from).count(), 1, "{from}");
    source.replace(from, to)
}

fn lower(source: &str) -> Result<Module, CoroError> {
    lower_coro(parsed(source), &CoroOptions::default())
}

fn function<'m>(module: &'m Module, symbol: &str) -> &'m Function {
    module.functions.iter().find(|(_, function)| function.symbol == symbol).unwrap().1
}

fn body<'m>(module: &'m Module, symbol: &str) -> &'m Body {
    function(module, symbol).body.as_ref().unwrap()
}

/// The instruction that defines `value` and the block that holds it.
fn definition(body: &Body, value: ValueId) -> (BlockId, usize, &Instruction) {
    let Definition::InstResult { inst, .. } = body.values.get(value).unwrap().def else {
        panic!("{value:?} is a block parameter");
    };
    let (block, position) = body.blocks.iter().find_map(|(block, data)| {
        data.insts.iter().position(|item| *item == inst).map(|position| (block, position))
    }).unwrap();
    (block, position, body.insts.get(inst).unwrap())
}

/// The cache-state token of every `fut.new` of a body, in instruction order.
fn cache_tokens(body: &Body) -> Vec<ValueId> {
    body.blocks.iter().flat_map(|(_, block)| block.insts.iter()).filter_map(|inst| {
        let instruction = body.insts.get(*inst).unwrap();
        matches!(instruction.op, Operation::Async(AsyncOp::FutureNew { .. })).then(|| instruction.results[1])
    }).collect()
}

#[test]
fn input_profile_and_budget() {
    let coro = lower(PLAIN).unwrap();
    assert!(matches!(lower_coro(coro, &CoroOptions::default()), Err(CoroError::InvalidInput { .. })));
    let options = CoroOptions { max_steps: 0 };
    assert_eq!(lower_coro(parsed(PLAIN), &options), Err(CoroError::LimitExceeded("steps")));
}

#[test]
fn start_continuation_restores_language_values_then_descriptors_then_the_packet() {
    // C-2 orders continuation 0 whatever the order of the Own entry parameters.
    let source = edited(PLAIN, "func @work(%0: int; %1: own) -> (int, own) [async] abi explicit(input_packet=1,",
        "func @work(%1: own, %0: int) -> (int, own) [async] abi explicit(input_packet=0,");
    let output = lower(&source).unwrap();
    let expected = lower(PLAIN).unwrap();
    assert!(text::normalized_eq(&output, &expected).unwrap());
    let work = function(&output, "work");
    let frame = output.types.iter().find(|(_, ty)| **ty == Type::Frame(FuncId::from_u32(0))).unwrap().0;
    let signature = Signature { params: vec![frame], results: vec![], ownership_abi: OwnershipAbi::Unelaborated };
    assert_eq!(work.signature, signature);
    let layout = work.body.as_ref().unwrap().frame_layout.as_ref().unwrap();
    let types: Vec<_> = layout.continuations[0].saved.iter()
        .map(|slot| output.types.get(layout.slots[*slot as usize].ty).unwrap().clone()).collect();
    assert_eq!(types, [Type::Int, Type::Service(ServiceType::Own)]);
}

#[test]
fn every_await_unwind_flows_into_await_unwind() {
    // C-3: the adapter consumes the task and the receipt of the edge.
    let source = edited(PLAIN, "unwind ^b3($failure, $task, $receipt, %6)",
        "unwind ^b3($failure, $receipt, $task, %6)");
    assert!(matches!(lower(&source), Err(CoroError::InvalidInput { message, .. }) if message.contains("C-3")));
}

#[test]
fn a_success_exit_publishes_the_outcome_of_its_commit() {
    let commit = "    (%12: int, %13: own, %14: deps) = exit.commit %11, destination caller, result int\n";
    let source = edited(PLAIN, &format!("{commit}    ret %12, %13"),
        &format!("{commit}    %moved: own = own.move %13\n    ret %12, %moved"));
    assert!(matches!(lower(&source), Err(CoroError::InvalidInput { message, .. }) if message.contains("C-2")));
}

#[test]
fn later_slices_keep_their_forms_unsupported() {
    // A block unwind leaves from any point of its block, across a suspension.
    let bad = "^b3(%13: failure, %14: task-state, %15: receipt, %16: scope<S1>) in S1";
    let source = edited(PLAIN, &format!("{bad}:"), &format!("{bad} unwind ^b5(%16):"));
    assert!(matches!(lower(&source), Err(CoroError::Unsupported { .. })));
}

#[test]
fn an_async_defer_entry_never_replaces() {
    // C-6 (R2): an async defer entry is lowered as an async body; a replacement inside it stays forbidden.
    let source = include_str!("../own/tests/async-goldens/r2-defer-await-body-failure.oir");
    let replacing = edited(source, "    ret defer continue\n", "    ret defer replace return\n");
    assert!(matches!(lower(&replacing), Err(CoroError::InvalidInput { message, .. }) if message.contains("C-6")));
}

#[test]
fn an_edge_without_saved_values_resumes_in_its_own_successor() {
    // D20 form (a): the successor starts with the resume; no extra block is needed.
    let source = edited(PLAIN, "normal ^b2($value, $deps, %5, %6, %7)", "normal ^b2($value, $deps)");
    let source = edited(&source, "unwind ^b3($failure, $task, $receipt, %6)", "unwind ^b3($failure, $task, $receipt)");
    let source = edited(&source, "    dependency d3: dependency = from_ssa(%12)\n", "");
    let source = edited(&source,
        "^b2(%8: int, %9: deps, %10: future<int> facts(managed(deps=d3)), %11: scope<S1>, %12: deps) in S1:",
        "^b2(%8: int, %9: deps) in S1:");
    let source = edited(&source, "invoke.own scope.end %11,", "invoke.own scope.end %6,");
    let source = edited(&source, "^b3(%13: failure, %14: task-state, %15: receipt, %16: scope<S1>) in S1:",
        "^b3(%13: failure, %14: task-state, %15: receipt) in S1:");
    let source = edited(&source, "await.unwind %16, %13", "await.unwind %6, %13");
    let input = parsed(&source);
    let blocks = body(&input, "main").blocks.len();
    let output = lowered("dominance", &source);
    let main = body(&output, "main");
    assert_eq!(main.blocks.len(), blocks);
    let awaiting = main.blocks.iter().find(|(_, block)| match &block.term.kind {
        TermKind::InvokeOwn { op, .. } => matches!(**op, Operation::Async(AsyncOp::Await { .. })),
        _ => false,
    }).unwrap().1;
    let TermKind::InvokeOwn { normal, unwind, .. } = &awaiting.term.kind else {
        unreachable!();
    };
    for target in [normal.block, unwind.block] {
        let block = main.blocks.get(target).unwrap();
        let first = main.insts.get(block.insts[0]).unwrap();
        assert!(matches!(first.op, Operation::Async(AsyncOp::FrameResume { continuation: 1, .. })));
    }
}

#[test]
fn a_value_returned_after_a_suspension_reads_its_resumed_version() {
    // The exit transaction names the argument of outer, live across its await: the pass saves the argument
    // and names its resumed version in the exit.
    let source = edited(NESTED,
        "    %17: deps = deps.of %11\n    %18: pending = exit.prepare return %11, %16, deps(%17) at B1",
        "    %17: deps = deps.of %4\n    %18: pending = exit.prepare return %4, %16, deps(%17) at B1");
    let output = lowered("returned", &source);
    let outer = body(&output, "outer");
    let saved: Vec<_> = outer.insts.iter().filter_map(|(_, inst)| match &inst.op {
        Operation::Async(AsyncOp::FrameSave { slots, continuation: 1, .. }) => Some(slots.len()),
        _ => None,
    }).collect();
    // The scope state, the deps of the Future, the cache duty, the returned argument, and the deps operand of the
    // Future's `fut.new`, which its fact names (#864).
    assert_eq!(saved, [5]);
}

#[test]
fn a_sync_function_gains_cache_duties_and_keeps_its_own_schedule() {
    // D16 (ruling 1): own declares the schedule; coro renames its Future to the cache-state token.
    let source = "oir 2 own
module sync_future
protocol Services kind services holders [scope, cache] rules [protocol_rule(transition=settle, from=cache, to=scope,
    checks=[owner, live_dependencies], normal=protocol_poststate(holder=scope, state=settled))]
scope ST action in @task
scope S action in @f
summary Mt internal contract call_contract(suspends=true, publication=atomic)
summary Mf internal contract call_contract(publication=atomic)
func @task() -> void [async] summary(Mt);
func @f(%in: own, %deps: deps) -> void summary(Mf) {
    service.schedule Order owner S, protocol Services, duties [%t]
^entry in S:
    %q0: scope<S> = scope.begin S
    (%t: future<void>, %q1: scope<S>) = fut.new %in, %deps, %q0, environment deferred(function=@task, arguments=[],
        descriptors=[]), owner S
    %closed: closed = scope.end %q1, effects scope_effects(failures=[], suspends=false, cancel=false)
    ret
}
";
    let output = lower(source).unwrap();
    assert_eq!(output.profile, Profile::Coro);
    let f = function(&output, "f");
    assert_eq!(f.signature, function(&parsed(source), "f").signature);
    let body = f.body.as_ref().unwrap();
    assert!(body.frame_layout.is_none());
    let cache = cache_tokens(body)[0];
    let (_, _, new) = definition(body, cache);
    assert_eq!(output.types.get(body.values.get(cache).unwrap().ty), Ok(&Type::Service(ServiceType::CacheState)));
    let index = new.results.iter().position(|value| *value == cache).unwrap();
    assert_eq!(index, 1);
    let inst = body.insts.iter().find(|(_, inst)| std::ptr::eq(*inst, new)).unwrap().0;
    assert_eq!(body.values.get(new.results[2]).unwrap().def, Definition::InstResult { inst, index: 2 });
    assert_eq!(body.service_schedules.len(), 1);
    assert_eq!(body.service_schedules[0].name, "Order");
    assert_eq!(body.service_schedules[0].duties, [ScheduleDuty::Value(cache)]);
}

#[test]
fn every_close_names_the_tokens_of_owns_schedule_in_owns_order() {
    // D16: own's schedule order is an admissible input, so coro keeps it rather than creation order.
    let schedule = "service.schedule Schedule0 owner S1, protocol P0, duties [%5, %15]";
    for (duties, first) in [("[%5, %15]", 0), ("[%15, %5]", 1)] {
        let renamed = format!("service.schedule Schedule0 owner S1, protocol P0, duties {duties}");
        let source = edited(VOID, schedule, &renamed);
        let output = lowered("order", &source);
        let main = body(&output, "main");
        let tokens = cache_tokens(main);
        assert_eq!(main.service_schedules.len(), 4);
        assert_eq!(main.service_schedules[0].name, "Schedule0");
        for schedule in &main.service_schedules[1..] {
            assert!(schedule.name.starts_with("Schedule0#"), "{}", schedule.name);
            assert_eq!(schedule.duties.len(), 2);
            // A duty is the token of its fut.new or a version that a resume of that token defines.
            let ScheduleDuty::Value(head) = schedule.duties[0] else { panic!("dynamic duty") };
            assert_eq!(origin(main, head), tokens[first], "{}", schedule.name);
        }
    }
}

#[test]
fn every_cache_duty_has_exactly_one_entry_in_a_schedule_of_its_owner() {
    // D16, B10.6: coro renames own's entries, so a duty without an entry, a repeated entry or an entry that
    // names no duty is invalid Own.
    let schedule = "service.schedule Schedule0 owner S1, protocol P0, duties [%5, %15]";
    for duties in ["[%5]", "[%5, %15, %15]", "[%5, %15, %2]"] {
        let renamed = format!("service.schedule Schedule0 owner S1, protocol P0, duties {duties}");
        let source = edited(VOID, schedule, &renamed);
        assert!(matches!(lower(&source), Err(CoroError::InvalidInput { message, .. }) if message.contains("D16")),
            "{duties}");
    }
}

/// The `fut.new` token that a resumed version of a cache-state token restores.
fn origin(body: &Body, value: ValueId) -> ValueId {
    let mut current = value;
    loop {
        if cache_tokens(body).contains(&current) {
            return current;
        }
        let (_, _, resume) = definition(body, current);
        let Operation::Async(AsyncOp::FrameResume { frame, slots, .. }) = &resume.op else {
            panic!("{current:?} is not a resumed version");
        };
        let index = resume.results.iter().position(|result| *result == current).unwrap();
        let slot = slots[index - 1].slot;
        let (_, _, save) = definition(body, *frame);
        let Operation::Async(AsyncOp::FrameSave { slots, .. }) = &save.op else {
            panic!("{frame:?} is not a saved frame");
        };
        current = slots.iter().find(|saved| saved.slot == slot).unwrap().value;
    }
}

#[test]
fn a_further_version_of_a_schedule_takes_the_first_unused_name() {
    let taken = |name: &str| ["Order#1", "Order#2"].contains(&name);
    assert_eq!(super::body::version_name("Order", 0, taken), "Order");
    assert_eq!(super::body::version_name("Order", 1, taken), "Order#3");
    assert_eq!(super::body::version_name("Order", 1, |_| false), "Order#1");
    assert_eq!(super::body::version_name("Order", 2, |_| false), "Order#2");
}

#[test]
fn a_fixed_root_publishes_its_value_with_an_empty_packet_and_deps_taken_before_its_close() {
    // Ruling 3, Champ draft 3 §3: async main keeps its observation ABI and publishes an empty packet. Its deps
    // are taken while its scope is open: after the close only exit forms run (B4.2).
    let output = lowered("root", PLAIN);
    let main = function(&output, "main");
    assert_eq!(main.signature.ownership_abi, OwnershipAbi::Observation);
    let body = main.body.as_ref().unwrap();
    let publish = body.insts.iter().find_map(|(_, inst)| match &inst.op {
        Operation::Async(AsyncOp::FuturePublish { outcome: PublishOutcome::Success { value, packet, deps }, .. }) => {
            Some((*value, *packet, *deps))
        }
        _ => None,
    }).unwrap();
    let (value, packet, deps) = publish;
    assert!(value.is_some());
    let (_, _, empty) = definition(body, packet);
    assert!(matches!(empty.op, Operation::Ownership(OwnershipOp::Empty {})));
    let (block, position, of) = definition(body, deps);
    assert!(matches!(of.op, Operation::Ownership(OwnershipOp::Dependencies { .. })));
    let data = body.blocks.get(block).unwrap();
    assert_eq!(position + 1, data.insts.len());
    let TermKind::InvokeOwn { op, .. } = &data.term.kind else { panic!("the deps block ends with the close") };
    assert!(matches!(**op, Operation::Ownership(OwnershipOp::ScopeEnd { .. })));
}

#[test]
fn a_fixed_root_whose_exit_follows_its_close_through_a_branch_takes_its_deps_before_the_close() {
    // D18 (#695): the successful close of an async root carries cancel=true, so it is an invoke with an unwind edge
    // and never a close instruction. Its normal edge may still reach the exit block through a branch.
    let source = edited(PLAIN, "        normal ^b4($closed, %8)\n", "        normal ^b6($closed, %8)\n");
    let source = edited(&source, "    ^b5(%22: closed, %23: failure) in S1:\n    raise %23\n}",
        "    ^b5(%22: closed, %23: failure) in S1:\n    raise %23\n    \
        ^b6(%24: closed, %25: int) in S1:\n    br ^b4(%24, %25)\n}");
    // The exit block follows the close through a branch: the deps are taken at the end of the closing block.
    let output = lowered("root-close", &source);
    let body = body(&output, "main");
    let (block, position, _) = body.insts.iter().find_map(|(_, inst)| match &inst.op {
        Operation::Async(AsyncOp::FuturePublish { outcome: PublishOutcome::Success { deps, .. }, .. }) => Some(*deps),
        _ => None,
    }).map(|deps| definition(body, deps)).unwrap();
    let data = body.blocks.get(block).unwrap();
    assert_eq!(position + 1, data.insts.len());
    let TermKind::InvokeOwn { op, .. } = &data.term.kind else { panic!("the deps block ends with the close") };
    assert!(matches!(**op, Operation::Ownership(OwnershipOp::ScopeEnd { .. })));
}

#[test]
fn a_landing_parameter_depends_on_values_that_its_landing_sees() {
    // #715: the normal edge of the await passes a managed view to a join block. The landing of that edge takes
    // the join parameter's place, so its dependency names the landing parameter, not the join parameter that the
    // landing does not see; the output verifies in full and is the reviewed fixture.
    let source = include_str!("../../tests/fixtures/async/own/b9-cache-view.oir");
    let output = lowered("b9-cache-view", source);
    let f = body(&output, "f");
    let mut checked = 0;
    for (block, data) in f.blocks.iter() {
        let resumes = data.insts.first().is_some_and(|inst| matches!(f.insts.get(*inst).unwrap().op,
            Operation::Async(AsyncOp::FrameResume { continuation: 1.., .. })));
        for &param in data.params.iter().filter(|_| resumes) {
            let ValueFacts::Managed { deps } = f.values.get(param).unwrap().facts else { continue };
            let DepExpr::Retention { value, selection } = f.deps.get(deps).unwrap() else { panic!("{block:?}") };
            assert_eq!(*value, param);
            assert_eq!(f.selections.get(*selection).unwrap(), &SelectionExpr::Value(param));
            checked += 1;
        }
    }
    assert_eq!(checked, 1, "the landing of the await's normal edge takes the managed view");
    let expected = parsed(include_str!("../../tests/fixtures/verify/valid/B9-cache-view-escapes-without-rehome.oir"));
    assert!(text::normalized_eq(&output, &expected).unwrap(), "{}", text::print(&output).unwrap());
}

const START_DEPS: &str = include_str!("../own/tests/async-goldens/start-deps.oir");
const VOID_ROOT: &str = include_str!("../own/tests/async-goldens/void-fixed-root.oir");

/// The types of the start slots of `symbol`, in continuation 0 order.
fn start_types(module: &Module, symbol: &str) -> Vec<Type> {
    let layout = body(module, symbol).frame_layout.as_ref().unwrap();
    layout.continuations[0].saved.iter()
        .map(|slot| module.types.get(layout.slots[*slot as usize].ty).unwrap().clone()).collect()
}

/// #691 (2), the C-2 ruling: a Future argument brings its deps into the start, after the language arguments and
/// before the one input packet, the order that the loader's SPB_ASYNC_ENTRY reads. Moving the deps parameter
/// after the packet in Own's signature does not move it in continuation 0.
#[test]
fn the_deps_of_the_start_follow_the_arguments_and_precede_the_packet() {
    let output = lower(START_DEPS).unwrap();
    let types = start_types(&output, "app.Adder.plus#0");
    let [Type::Object { .. }, Type::Future(_), Type::Int, Type::Service(ServiceType::Deps),
        Type::Service(ServiceType::Own)] = types.as_slice() else {
        panic!("{types:?}");
    };
    // A fixed root starts empty (L1).
    assert!(start_types(&output, "main").is_empty());
}

/// The value a resumed value was saved from, through the saves and resumes of the frame (D20).
fn saved_origin(body: &Body, mut value: ValueId) -> ValueId {
    loop {
        let Definition::InstResult { inst, index } = body.values.get(value).unwrap().def else {
            return value;
        };
        let Operation::Async(AsyncOp::FrameResume { frame, slots, .. }) = &body.insts.get(inst).unwrap().op else {
            return value;
        };
        let Some(slot) = (index as usize).checked_sub(1).map(|index| slots[index].slot) else {
            return value;
        };
        let (_, _, save) = definition(body, *frame);
        let Operation::Async(AsyncOp::FrameSave { slots, .. }) = &save.op else { panic!("{save:?}") };
        value = slots.iter().find(|saved| saved.slot == slot).unwrap().value;
    }
}

/// #691 (1), the C-2 ruling: a void fixed root publishes an empty packet and the empty deps of a scalar taken at
/// the entry, before its root scope begins and so before any close (B4.2).
#[test]
fn a_void_fixed_root_publishes_the_empty_deps_of_a_scalar() {
    let output = lower(VOID_ROOT).unwrap();
    assert!(start_types(&output, "check").is_empty());
    let check = body(&output, "check");
    let mut published = 0;
    for (_, inst) in check.insts.iter() {
        let Operation::Async(AsyncOp::FuturePublish { outcome: PublishOutcome::Success { value, packet, deps }, .. })
            = &inst.op else {
            continue;
        };
        published += 1;
        assert!(value.is_none());
        assert!(matches!(definition(check, *packet).2.op, Operation::Ownership(OwnershipOp::Empty {})));
        let (block, position, made) = definition(check, saved_origin(check, *deps));
        let Operation::Ownership(OwnershipOp::Dependencies { value: unit }) = made.op else { panic!("{made:?}") };
        assert!(matches!(definition(check, unit).2.op, Operation::Const(Literal::Int(_))));
        assert_eq!(block, check.entry);
        let begins = check.blocks.get(block).unwrap().insts.iter().position(|inst| {
            matches!(check.insts.get(*inst).unwrap().op, Operation::Ownership(OwnershipOp::ScopeBegin { .. }))
        }).unwrap();
        assert!(position < begins, "the deps are taken before the root scope opens");
    }
    assert_eq!(published, 1);
}

/// The rehomes of a coro body: the form and the scope whose state or duty they take.
fn rehomes(module: &Module, body: &Body) -> Vec<(bool, Option<ScopeId>)> {
    body.blocks.iter().flat_map(|(_, block)| block.insts.iter()).filter_map(|inst| {
        match &body.insts.get(*inst).unwrap().op {
            Operation::Async(AsyncOp::CacheRehome { form: RehomeForm::Token { .. }, .. }) => Some((true, None)),
            Operation::Async(AsyncOp::CacheRehome { form: RehomeForm::Scope { state, .. }, .. }) => {
                let ty = body.values.get(*state).unwrap().ty;
                let Ok(Type::Service(ServiceType::Scope(scope))) = module.types.get(ty) else { unreachable!() };
                Some((false, Some(*scope)))
            }
            _ => None,
        }
    }).collect()
}

/// #601 D25 (C-1, C-4, D25.3): a Future created here leaves its owner with its token: the rehome takes the token
/// form (verify_coro then checks that no close settles it again). A Future that reached a scope through an admitted
/// packet has no token: it leaves in the own form, which takes it from the state. Own's dynamic step stays the
/// last entry of every schedule version of its owner.
#[test]
fn a_rehome_takes_the_token_of_a_duty_created_here_and_the_state_of_an_admitted_one() {
    let source = include_str!("../own/tests/async-goldens/r1-loop-escape.oir");
    let module = lowered("r1-loop-escape", source);
    let main = body(&module, "main");
    // S5's Future leaves S5 by its token; S4 admitted it and passes it on from its state.
    let forms = rehomes(&module, main);
    assert_eq!(forms.len(), 2, "{forms:?}");
    assert!(forms.contains(&(true, None)), "{forms:?}");
    assert!(forms.iter().any(|(token, scope)| !token && scope.is_some()), "{forms:?}");
    let own = parsed(source);
    let own_main = own.functions.iter().find(|(_, function)| function.symbol == "main").unwrap().1;
    let dynamic: Vec<_> = own_main.body.as_ref().unwrap().service_schedules.iter()
        .filter(|schedule| schedule.duties.contains(&ScheduleDuty::Dynamic)).map(|schedule| schedule.owner).collect();
    assert!(!dynamic.is_empty());
    for schedule in main.service_schedules.iter().filter(|schedule| dynamic.contains(&schedule.owner)) {
        assert_eq!(schedule.duties.last(), Some(&ScheduleDuty::Dynamic), "{}", schedule.name);
        assert_eq!(schedule.duties.iter().filter(|duty| **duty == ScheduleDuty::Dynamic).count(), 1);
    }
    for schedule in main.service_schedules.iter().filter(|schedule| !dynamic.contains(&schedule.owner)) {
        assert!(!schedule.duties.contains(&ScheduleDuty::Dynamic), "{}", schedule.name);
    }
}

/// The returned Future of OIR-2 §10.1 `make()`: @make's rehome takes its token to the caller, and main's scopes
/// that admit it settle it dynamically.
#[test]
fn a_returned_future_leaves_by_its_token() {
    let module = lowered("r1-make-escape", include_str!("../own/tests/async-goldens/r1-make-escape.oir"));
    let make = body(&module, "make");
    assert_eq!(rehomes(&module, make), [(true, None)]);
    // The duty left with the exit: no schedule of @make names its token.
    let token = cache_tokens(make)[0];
    assert!(make.service_schedules.iter().all(|schedule| !schedule.duties.contains(&ScheduleDuty::Value(token))));
    let main = body(&module, "main");
    assert!(main.service_schedules.iter().all(|schedule| schedule.duties.last() == Some(&ScheduleDuty::Dynamic)));
}

/// A schedule names its dynamic step once.
#[test]
fn a_schedule_names_its_dynamic_step_once() {
    let source = include_str!("../own/tests/async-goldens/r1-make-escape.oir");
    let twice = source.replace("duties [dynamic]", "duties [dynamic, dynamic]");
    assert!(matches!(lower(&twice), Err(CoroError::InvalidInput { .. })));
}

#[test]
fn a_nested_selection_of_a_saved_value_names_its_resumed_version() {
    // #734, D20: a value used after a suspension is saved and redefined by the resume. A selection names it
    // through nested expressions too (the admissible part of a union of single values, as own selects the
    // borrowed inputs of a cold Future); the repair copies the expression to name the resumed version.
    let source = include_str!("../../tests/fixtures/async/own/r2-resource-across-await.oir");
    let nested = edited(source, "selection s2: selection = union([selection(%31)])",
        "selection s2: selection = union([selection(%16)])");
    let output = lowered("nested selection", &nested);
    let f = body(&output, "f");
    let split = f.blocks.iter().find_map(|(_, block)| match &block.term.kind {
        TermKind::InvokeOwn { op, .. } => match **op {
            Operation::Ownership(OwnershipOp::ScopeSplit { selection, .. }) => match f.selections.get(selection) {
                Ok(SelectionExpr::Admissible(inner))
                    if matches!(f.selections.get(*inner), Ok(SelectionExpr::Union(items)) if !items.is_empty()) => {
                    Some(selection)
                }
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }).expect("the split after the await");
    let SelectionExpr::Admissible(union) = f.selections.get(split).unwrap() else { unreachable!() };
    let SelectionExpr::Union(items) = f.selections.get(*union).unwrap() else { panic!("{union:?}") };
    let [single] = items.as_slice() else { panic!("{items:?}") };
    let SelectionExpr::Value(named) = f.selections.get(*single).unwrap() else { panic!("{single:?}") };
    let (_, _, definition) = definition(f, *named);
    assert!(matches!(definition.op, Operation::Async(AsyncOp::FrameResume { continuation: 1, .. })),
        "{named:?} is the version the await's resume restores");
}

const D25_COLD: &str = include_str!("../../tests/fixtures/async/own/d25-stored-cold.oir");
const D25_SPAWNED: &str = include_str!("../../tests/fixtures/async/own/d25-stored-spawned.oir");

/// The own exit of `make` with the rehome that the ruling "How a stored Future leaves" forbids after a store.
pub(crate) fn d25_rehomed_after_store() -> String {
    let rehomed = edited(D25_COLD, "    %33: pending = exit.prepare return %27, %32, deps(%29) at B1\n",
        "    %33: pending = exit.prepare return %27, %32, deps(%29) at B1\n    (%900: scope<S2>, %901: pending) = \
cache.rehome %31, %33, future %12, target caller, protocol P0\n");
    let split = "    invoke.own scope.split %31, selection(%27)\n        normal ^b8($state, $selected, $external_deps, \
%27, %29, %33)\n        unwind ^b9($state, $failure, %33)";
    let rehomed_split = "    invoke.own scope.split %900, selection(%27)\n        normal ^b8($state, $selected, \
$external_deps, %27, %29, %901)\n        unwind ^b9($state, $failure, %901)";
    edited(&rehomed, split, rehomed_split)
}

/// The `service.absorb`s of a body that start from a split's selected packet (`normal`, a block parameter) and from a
/// failure edge's `own.empty` (#802 follow-up: a failed store moves the token to the owner's dynamic inventory).
fn absorbs_by_edge(body: &Body) -> (Vec<ValueId>, Vec<ValueId>) {
    let (mut normal, mut failure) = (vec![], vec![]);
    for (mut packet, token) in absorbs(body) {
        while let Definition::InstResult { inst, .. } = body.values.get(packet).unwrap().def
            && let Operation::Async(AsyncOp::ServiceAbsorb { packet: earlier, .. }) = body.insts.get(inst).unwrap().op {
            packet = earlier;
        }
        match body.values.get(packet).unwrap().def {
            Definition::BlockParam { .. } => normal.push(token),
            _ => failure.push(token),
        }
    }
    (normal, failure)
}

/// The `service.absorb`s of a body, in block order: (packet, token).
fn absorbs(body: &Body) -> Vec<(ValueId, ValueId)> {
    body.blocks.iter().flat_map(|(_, block)| block.insts.iter()).filter_map(|inst| {
        match body.insts.get(*inst).unwrap().op {
            Operation::Async(AsyncOp::ServiceAbsorb { packet, token }) => Some((packet, token)),
            _ => None,
        }
    }).collect()
}

/// The `cleanup.settle`s of `token` in a low body: those no absorb of it precedes, and those it does.
fn settles_after_absorb(body: &Body, token: ValueId) -> (usize, usize) {
    let mut after = std::collections::BTreeSet::new();
    let mut pending: Vec<_> = body.blocks.iter().filter(|(_, block)| block.insts.iter().any(|inst| {
        matches!(body.insts.get(*inst).unwrap().op, Operation::Async(AsyncOp::ServiceAbsorb { token: t, .. })
            if t == token)
    })).map(|(id, _)| id).collect();
    while let Some(block) = pending.pop() {
        if after.insert(block) {
            pending.extend(graph::successors(&body.blocks.get(block).unwrap().term.kind));
        }
    }
    let mut counts = (0, 0);
    for (id, block) in body.blocks.iter() {
        for inst in &block.insts {
            if let Operation::Cleanup(CleanupOp::Settle { holder, .. }) = body.insts.get(*inst).unwrap().op
                && holder == token
            {
                if after.contains(&id) { counts.1 += 1 } else { counts.0 += 1 }
            }
        }
    }
    counts
}

/// #771 (async ABI D25.2 revision): the split before `Holder.new` names the Future created here, so its cache
/// token moves into the selected packet, once. `make`'s scope settles entries at run time (`dynamic`: its result can
/// hold a Future), so on the split's failure edge the token moves into that dynamic inventory (#802 follow-up
/// ruling): the schedule no longer names it, and low settles it statically nowhere. The duty is never settled twice
/// (the first of the three properties). The full verifiers accept the result.
#[test]
fn a_stored_future_is_absorbed_once_and_leaves_its_schedules() {
    let module = lowered("d25-stored-cold", D25_COLD);
    let make = body(&module, "app.Maker.make#0");
    let token = cache_tokens(make)[0];
    assert_eq!(absorbs_by_edge(make), (vec![token], vec![token]));
    assert!(make.service_schedules.iter().all(|schedule| !schedule.duties.contains(&ScheduleDuty::Value(token))));
    let low = crate::low::lower_cleanup(module.clone(), &crate::low::LowOptions::default()).unwrap();
    let lowered_make = body(&low, "app.Maker.make#0");
    assert_eq!(settles_after_absorb(lowered_make, token), (0, 0));
    // The absorb opens the split's normal block and the packet it gives is the one the call takes.
    assert!(absorbs(make).iter().any(|(packet, _)| {
        matches!(make.values.get(*packet).unwrap().def, Definition::BlockParam { .. })
    }), "the selected packet");
    // No rehome: the entry leaves with the Holder's packet.
    assert!(rehomes(&module, make).is_empty());
}

/// #771, D24.1: a spawned Future stored in a field moves its task duty and its cache duty together, task first
/// (the third property).
#[test]
fn a_stored_spawned_future_absorbs_its_task_then_its_cache() {
    let module = lowered("d25-stored-spawned", D25_SPAWNED);
    let make = body(&module, "app.Maker.make#0");
    // On the split's normal edge into the selected packet, on its failure edge into the dynamic inventory (#802
    // follow-up ruling): task first, then cache, on each.
    let (normal, failure) = absorbs_by_edge(make);
    assert_eq!(normal, failure);
    let kinds: Vec<_> = normal.iter().map(|token| {
        let ty = make.values.get(*token).unwrap().ty;
        module.types.get(ty).unwrap().clone()
    }).collect();
    assert_eq!(kinds, [Type::Service(ServiceType::TaskState), Type::Service(ServiceType::CacheState)]);
    // Each edge's second absorb takes the packet of its first.
    for (packet, _) in absorbs(make).iter().filter(|(_, token)| *token == normal[1]) {
        let Definition::InstResult { inst, .. } = make.values.get(*packet).unwrap().def else {
            panic!("the cache absorb takes the task absorb's packet");
        };
        assert!(matches!(make.insts.get(inst).unwrap().op, Operation::Async(AsyncOp::ServiceAbsorb { .. })));
    }
    let low = crate::low::lower_cleanup(module.clone(), &crate::low::LowOptions::default()).unwrap();
    let lowered_make = body(&low, "app.Maker.make#0");
    for token in &normal {
        assert_eq!(settles_after_absorb(lowered_make, *token), (0, 0), "{token:?}");
    }
}

/// The ruling "How a stored Future leaves": own emits no rehome after a store, so a rehome that a store precedes
/// is invalid input.
#[test]
fn a_rehome_after_a_store_is_refused() {
    let error = lower(&d25_rehomed_after_store()).unwrap_err();
    assert!(error.to_string().contains("rehomed after its duty was stored"), "{error}");
}

/// The token forms of the rehomes of a coro body: the cache token and the task token, if any.
fn token_rehomes(body: &Body) -> Vec<(ValueId, Option<ValueId>)> {
    body.blocks.iter().flat_map(|(_, block)| block.insts.iter()).filter_map(|inst| {
        match &body.insts.get(*inst).unwrap().op {
            Operation::Async(AsyncOp::CacheRehome { form: RehomeForm::Token { cache, task, .. }, .. }) => {
                Some((*cache, *task))
            }
            _ => None,
        }
    }).collect()
}

/// PR-B1 (OIR-2-async-remaining §2.2a), #784: `c` is bound (`let c = Child(n).run();`), so `fut.new` carries
/// `binding` and an aggregate only borrows it: neither store takes its duty, which stays with its lexical owner
/// and leaves through `cache.rehome` when `make` returns the second Holder. The verifiers accept the result.
#[test]
fn a_bound_future_is_not_absorbed_by_a_store() {
    let module = lowered("d25-stored-twice", include_str!("../../tests/fixtures/async/own/d25-stored-twice.oir"));
    let make = body(&module, "app.Maker.make#0");
    assert!(absorbs(make).is_empty(), "{:?}", absorbs(make));
    let token = cache_tokens(make)[0];
    assert_eq!(token_rehomes(make), [(token, None)], "the bound Future leaves make with its token");
    let low = crate::low::lower_cleanup(module.clone(), &crate::low::LowOptions::default()).unwrap();
    crate::verify_low_with(&low, &options(), VerifyContext::default()).unwrap_or_else(|report| panic!("{report:?}"));
}

/// PR-B1, §2.2a danger 1 (D24.1, #765): `let t = spawn Child(n).run(); Holder(f: t);`. The attribute marks the
/// Future, so both duties are bound: neither the task token nor the cache token is absorbed, and both leave `make`
/// together through the token rehome.
#[test]
fn a_bound_spawned_future_keeps_both_tokens_when_stored() {
    let module = lowered("d25-stored-spawned-bound",
        include_str!("../../tests/fixtures/async/own/d25-stored-spawned-bound.oir"));
    let make = body(&module, "app.Maker.make#0");
    assert!(absorbs(make).is_empty(), "{:?}", absorbs(make));
    let rehomes = token_rehomes(make);
    assert_eq!(rehomes.len(), 1, "{rehomes:?}");
    assert!(rehomes[0].1.is_some(), "the task token leaves with the cache token: {rehomes:?}");
}

/// PR-B1: coro tests the attribute itself. Even where a split names a bound Future (input that own never produces
/// and that verify_own refuses as `b9-bound-future-named`), the split takes no token: the absorb is only for a fresh
/// temporary.
#[test]
fn a_split_never_absorbs_a_bound_token() {
    let source = include_str!("../../tests/fixtures/async/own/d25-stored-twice.oir").replacen(
        "    selection s0: selection = admissible(selection(%19))\n",
        "    selection s9: selection = union([selection(%19), selection(%22)])\n    selection s0: selection = \
admissible(s9)\n", 1);
    let module = lower_coro(parsed(&source), &CoroOptions::default()).unwrap();
    let make = body(&module, "app.Maker.make#0");
    assert!(absorbs(make).is_empty(), "{:?}", absorbs(make));
}

const BUILDER_FUTURES: &str = include_str!("../../tests/fixtures/async/own/d802-builder-futures.oir");

/// #802 (Fable's ruling and its follow-up, OIR-2-async-abi.md): a fresh Future pushed into a builder is stored. Its
/// token is absorbed into the split's selected packet on the normal edge, and on the split's unwind edge it moves
/// into the owner's dynamic inventory (`own.empty`, `service.absorb`, `own.admit`), so the builder's shared handler
/// sees it on no path. Without `dynamic` in the owner's schedule the token stays static on the unwind edge, and the
/// shared handler's close is reached with the duty on some paths only: under duty slots (D26, #857, which changed
/// this test's intended behaviour from a refusal) the owner's duties become slots and the close settles the merged
/// slot, full or empty.
#[test]
fn a_future_stored_in_a_builder_leaves_its_token_on_both_outcomes() {
    let output = lowered("d802-builder-futures", BUILDER_FUTURES);
    let main = body(&output, "main");
    let absorbs = main.insts.iter().filter(|(_, instruction)| {
        matches!(instruction.op, Operation::Async(AsyncOp::ServiceAbsorb { .. }))
    }).count();
    assert_eq!(absorbs, 16, "eight normal edges and eight failure edges");
    let admitted_on_failure = main.blocks.iter().filter(|(_, block)| block.insts.iter().any(|inst| {
        matches!(main.insts.get(*inst).unwrap().op, Operation::Ownership(OwnershipOp::Admit { .. }))
    }) && block.insts.iter().any(|inst| {
        matches!(main.insts.get(*inst).unwrap().op, Operation::Async(AsyncOp::ServiceAbsorb { .. }))
    })).count();
    assert!(admitted_on_failure >= 8, "{admitted_on_failure}");
    let schedule = "%331, %367, dynamic]";
    assert!(BUILDER_FUTURES.contains(schedule));
    let output = lower(&edited(BUILDER_FUTURES, schedule, "%331, %367]")).unwrap_or_else(|error| panic!("{error:?}"));
    let main = body(&output, "main");
    let (empties, filled) = slots(main);
    assert!(empties > 0 && filled, "the owner's duties are slots: {empties} empties");
}

/// D26 (#857): the `duty.empty` instructions of a body, and whether every `fut.new` fills a slot.
fn slots(body: &Body) -> (usize, bool) {
    let empties = body.insts.iter().filter(|(_, inst)| matches!(inst.op, Operation::Async(AsyncOp::DutyEmpty { .. })))
        .count();
    let filled = body.insts.iter().all(|(_, inst)| match &inst.op {
        Operation::Async(AsyncOp::FutureNew { slot, .. }) => slot.is_some(),
        _ => true,
    });
    (empties, filled)
}

const LOOP_BODY: &str = include_str!("../../tests/fixtures/async/own/d759-loop-body-future.oir");

/// #759: the close of a duty's owner settles it, so a loop body's duty does not flow along the back edge into the
/// next iteration (the golden `d759-loop-body-future`). Here the body's close is shared by the path that created the
/// Future and the unwind of `Job(n: i)` before it. Under duty slots (D26, #857, which changed this test's intended
/// behaviour from a refusal) the body scope's duty is a slot: empty after the body's `scope.begin`, filled by
/// `fut.new`, and the shared close settles the version that the two paths merge.
#[test]
fn a_loop_body_duty_ends_at_each_close_of_its_owner() {
    let output = lowered("d759-loop-body-future", LOOP_BODY);
    let main = body(&output, "main");
    let [schedule] = main.service_schedules.as_slice() else { panic!("one schedule") };
    assert_eq!(schedule.duties.len(), 1);
    let effects = "effects scope_effects(failures=[], suspends=false, cancel=false)";
    let close = format!("    %42: closed = scope.end %40, {effects}\n    br ^b15\n");
    let shared = format!("    br ^bx(%40, %33, %34, %35)\n    \
        ^bx(%60: scope<S2>, %61: int, %62: int, %63: scope<S1>) in S2:\n    \
        %42: closed = scope.end %60, {effects}\n    br ^b6(%61, %62, %63)\n");
    let source = edited(LOOP_BODY, &close, &shared);
    // The unwind of `Job(n: i)` joins the shared close with the loop's counters of ^b5; its failure is dropped, as
    // a catch would. ^b16, left without a predecessor, goes.
    let unwind = format!("    %46: failure = scope.unwind %45, %43, {effects}\n    br ^b16\n");
    let source = edited(&source, &unwind, "    br ^bx(%45, %11, %12, %44)\n");
    let dead = format!("    ^b16 in S1:\n    %47: failure = scope.unwind %44, %46, {effects}\n    raise %47\n");
    let source = edited(&source, &dead, "");
    let output = lower(&source).unwrap_or_else(|error| panic!("{error:?}"));
    let main = body(&output, "main");
    assert_eq!(slots(main), (1, true), "one empty after the body's begin; fut.new fills it");
    let [schedule] = main.service_schedules.as_slice() else { panic!("one schedule: {:?}", main.service_schedules) };
    let [ScheduleDuty::Value(merged)] = schedule.duties.as_slice() else { panic!("one duty") };
    assert!(matches!(main.values.get(*merged).unwrap().def, Definition::BlockParam { .. }),
        "the shared close settles the merged slot");
}

/// Duty slots, S0 check 1 (#854): in every own input of the coro goldens, each close of a scope is dominated by a
/// begin of that scope, so a slot defined empty at the begin has a version at every close.
#[test]
fn every_close_is_dominated_by_its_scope_begin() {
    for (name, own, _) in GOLDENS {
        let module = parsed(own);
        for (_, function) in module.functions.iter() {
            let Some(body) = &function.body else { continue };
            let undominated = graph::undominated_closes(&module, body);
            assert!(undominated.is_empty(), "{name}: {}: {undominated:?}", function.symbol);
        }
    }
}

/// D26 R-1 (#857): a `fut.new` in a loop of its owner, with no close of the owner in the loop, would fill the slot
/// while it still holds the duty of the iteration before. Coro refuses it before it emits slots.
#[test]
fn a_creation_in_a_loop_of_its_owner_is_refused_by_occupancy() {
    let source = include_str!("../../tests/fixtures/async/own/d857-branch-temporary.oir");
    let looped = edited(source, "    br ^b4(%8)\n", "    br ^b1(%8)\n");
    let error = lower(&looped).unwrap_err();
    assert!(matches!(&error, CoroError::InvalidInput { message, .. }
        if message == "fut.new may fill a slot that still holds its earlier duty (D26 R-1)"), "{error:?}");
}

/// #864 (Fable's review): a Future's fact names the deps operand of its `fut.new`, so a suspension that holds the
/// Future saves that deps value. The coro verifier derives this on its own: a `frame.save` without it is refused as an
/// incomplete saved set.
#[test]
fn a_suspension_saves_what_a_live_futures_fact_names() {
    let golden = include_str!("../../tests/fixtures/async/coro/async-plain-await.oir");
    verify_coro_with(&parsed(golden), &options(), VerifyContext::default())
        .unwrap_or_else(|report| panic!("{report:?}"));
    let source = edited(golden, "frame_slot(slot=0, value=%6, mode=borrow), ", "");
    let source = edited(&source, "continuation 1 at ^b1 saves [0, 1, 2, 3] operands [4]",
        "continuation 1 at ^b1 saves [1, 2, 3] operands [4]");
    let source = edited(&source, "(%35: frame<@main>, %36: deps, %37: scope<S1>", "(%35: frame<@main>, %37: scope<S1>");
    let source = edited(&source, "(%43: frame<@main>, %44: deps, %45: scope<S1>", "(%43: frame<@main>, %45: scope<S1>");
    let source = source.replace("continuation 1, slots [saved_slot(slot=0, ty=deps), saved_slot(slot=1,",
        "continuation 1, slots [saved_slot(slot=1,");
    let report = verify_coro_with(&parsed(&source), &options(), VerifyContext::default())
        .expect_err("the deps the Future's fact names is not saved");
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == RuleId::CoroSavedSetIncomplete),
        "{report:?}");
}

/// #806: own's output of three loops that each suspend (a Future awaited after `next`/`last`, a spawn, a temporary
/// awaited). The reaching-version fixed point of the frame token oscillated and spent coro's whole budget
/// (10 000 000 steps); with merges that stay merged until the trivial ones are removed it repairs in a small one.
#[test]
fn three_loops_that_each_suspend_repair_within_a_small_budget() {
    let source = include_str!("fixtures/d806-three-suspending-loops.oir");
    // The natives of the fixture's `Stdout` need the compiler's external contracts to verify; the Champ case
    // `three_loops_that_each_suspend_run` verifies this output on the real pipeline.
    lower_coro(parsed(source), &CoroOptions { max_steps: 100_000 }).unwrap_or_else(|error| panic!("{error}"));
}
