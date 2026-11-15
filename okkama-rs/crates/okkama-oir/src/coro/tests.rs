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
    // R2, C-6: async defers whose entry awaits, after a body failure and across a pending exit.
    variant!("r2-defer-await-body-failure"),
    variant!("r2-defer-await-reserve"),
    // R2: closes that suspend while the body owns a resource.
    variant!("r2-resource-suspending-close"),
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
    let commit = "    (%13: int, %14: own, %15: deps) = exit.commit %11, destination caller, result int\n";
    let source = edited(PLAIN, &format!("{commit}    ret %13, %14"),
        &format!("{commit}    %moved: own = own.move %14\n    ret %13, %moved"));
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
    let source = include_str!("../../tests/fixtures/async/own/r2-defer-await-body-failure.oir");
    let replacing = edited(source, "    ret defer continue\n", "    ret defer replace return\n");
    assert!(matches!(lower(&replacing), Err(CoroError::InvalidInput { message, .. }) if message.contains("C-6")));
}

#[test]
fn an_edge_without_saved_values_resumes_in_its_own_successor() {
    // D20 form (a): the successor starts with the resume; no extra block is needed.
    let source = edited(PLAIN, "normal ^b2($value, $deps, %5, %6, %7)", "normal ^b2($value, $deps)");
    let source = edited(&source, "unwind ^b3($failure, $task, $receipt, %6)", "unwind ^b3($failure, $task, $receipt)");
    let source = edited(&source, "    dependency d1: dependency = from_ssa(%12)\n", "");
    let source = edited(&source,
        "^b2(%8: int, %9: deps, %10: future<int> facts(managed(deps=d1)), %11: scope<S1>, %12: deps) in S1:",
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
    // The scope state, the deps of the Future, the cache duty and the returned argument.
    assert_eq!(saved, [4]);
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
