use super::*;

fn parsed(source: &str) -> Module {
    text::parse(source).unwrap_or_else(|error| panic!("{error}"))
}

fn options() -> VerifyOptions {
    VerifyOptions { execution: ExecutionMode::Tests, ..VerifyOptions::default() }
}

macro_rules! golden {
    ($name:literal) => {
        ($name, include_str!(concat!("../../tests/fixtures/async/own/", $name, ".oir")),
            include_str!(concat!("../../tests/fixtures/async/coro/", $name, ".oir")))
    };
}

/// The inputs of V2's own goldens (plan V2), hand-written until own commit 5a produces them.
const GOLDENS: &[(&str, &str, &str)] = &[
    golden!("async-plain-await"),
    golden!("async-await-failure"),
    golden!("async-void"),
    golden!("async-repeated-await"),
    golden!("async-never-awaited"),
    golden!("coro-start-continuation"),
    golden!("coro-nested-cold-await"),
    golden!("coro-schedule-duty-live-across-await"),
];

/// Own → verify(own) → coro → verify(coro) (OIR-2 §12) on the hand-written V2 inputs.
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

const PLAIN: &str = include_str!("../../tests/fixtures/async/own/async-plain-await.oir");

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
    let source = edited(PLAIN, "func @work(%n: int; %inputs: own) -> (int, own) [async] abi explicit(input_packet=1,",
        "func @work(%inputs: own, %n: int) -> (int, own) [async] abi explicit(input_packet=0,");
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
    let source = edited(PLAIN, "unwind ^bad($failure, $task, $receipt, %q2)",
        "unwind ^bad($failure, $receipt, $task, %q2)");
    assert!(matches!(lower(&source), Err(CoroError::InvalidInput { message, .. }) if message.contains("C-3")));
}

#[test]
fn a_success_exit_publishes_the_outcome_of_its_commit() {
    let commit = "    (%r: int, %p: own, %rd: deps) = exit.commit %reserved, destination caller, result int\n";
    let source = edited(PLAIN, &format!("{commit}    ret %r, %p"),
        &format!("{commit}    %moved: own = own.move %p\n    ret %r, %moved"));
    assert!(matches!(lower(&source), Err(CoroError::InvalidInput { message, .. }) if message.contains("C-2")));
}

#[test]
fn later_slices_keep_their_forms_unsupported() {
    // A block unwind leaves from any point of its block, across a suspension.
    let bad = "^bad(%failure: failure, %task: task-state, %receipt: receipt, %q4: scope<SM>) in SM";
    let source = edited(PLAIN, &format!("{bad}:"), &format!("{bad} unwind ^close_failed(%q4):"));
    assert!(matches!(lower(&source), Err(CoroError::Unsupported { .. })));
    // Async defer entries are slice D1 (C-6).
    let mut module = parsed(PLAIN);
    let work = module.functions.iter().find(|(_, function)| function.symbol == "work").unwrap().0;
    module.functions.get_mut(work).unwrap().attrs.push(FunctionAttr::DeferEntry { replacements: Vec::new() });
    assert!(matches!(lower_coro(module, &CoroOptions::default()), Err(CoroError::Unsupported { .. })));
}

#[test]
fn an_edge_without_saved_values_resumes_in_its_own_successor() {
    // D20 form (a): the successor starts with the resume; no extra block is needed.
    let source = edited(PLAIN, "normal ^ok($value, $deps, %q2)", "normal ^ok($value, $deps)");
    let source = edited(&source, "unwind ^bad($failure, $task, $receipt, %q2)",
        "unwind ^bad($failure, $task, $receipt)");
    let source = edited(&source, "^ok(%x: int, %xd: deps, %q3: scope<SM>) in SM:", "^ok(%x: int, %xd: deps) in SM:");
    let source = edited(&source, "invoke.own scope.end %q3,", "invoke.own scope.end %q2,");
    let source = edited(&source, "^bad(%failure: failure, %task: task-state, %receipt: receipt, %q4: scope<SM>) in SM:",
        "^bad(%failure: failure, %task: task-state, %receipt: receipt) in SM:");
    let source = edited(&source, "await.unwind %q4, %failure", "await.unwind %q2, %failure");
    let input = parsed(&source);
    let blocks = function(&input, "main").body.as_ref().unwrap().blocks.len();
    let output = lowered("dominance", &source);
    let body = function(&output, "main").body.as_ref().unwrap();
    assert_eq!(body.blocks.len(), blocks);
    let TermKind::InvokeOwn { normal, unwind, .. } = &body.blocks.get(body.entry).unwrap().term.kind else {
        panic!("the entry block ends with the await");
    };
    for target in [normal.block, unwind.block] {
        let block = body.blocks.get(target).unwrap();
        let first = body.insts.get(block.insts[0]).unwrap();
        assert!(matches!(first.op, Operation::Async(AsyncOp::FrameResume { continuation: 1, .. })));
    }
}

#[test]
fn a_value_returned_after_a_suspension_reads_its_resumed_version() {
    // The exit selection names the value saved across the await; the pass names its resumed version.
    let source = edited(PLAIN, "func @main(%inputs: own) -> own [async] abi explicit(input_packet=0, output_packet=0)",
        "func @main(%inputs: own) -> (int, own) [async] abi explicit(input_packet=0, output_packet=1)");
    let source = edited(&source, "summary Mmb call-boundary contract call_contract(failures=",
        concat!("summary Mmb call-boundary results [result_relation(provenance=unbound_result)] contract ",
            "call_contract(results=[int],\n    failures="));
    let close = "    invoke.own scope.end %q3";
    let void = "    %pending: pending = exit.prepare return packet %e select s0 at B0\n";
    let valued = concat!("    %sd: deps = deps.of %seven\n",
        "    %pending: pending = exit.prepare return %seven, %e, deps(%sd) at B0\n");
    let source = edited(&source, &format!("{void}{close}"), &format!("{valued}{close}"));
    let source = edited(&source, "    (%p: own, %pd: deps) = exit.commit %reserved, destination caller\n    ret %p\n",
        "    (%v: int, %p: own, %pd: deps) = exit.commit %reserved, destination caller, result int\n    ret %v, %p\n");
    let output = lowered("returned", &source);
    let body = function(&output, "main").body.as_ref().unwrap();
    let saved: Vec<_> = body.insts.iter().filter_map(|(_, inst)| match &inst.op {
        Operation::Async(AsyncOp::FrameSave { slots, continuation: 1, .. }) => Some(slots.len()),
        _ => None,
    }).collect();
    // The scope state, the cache duty and the returned integer.
    assert_eq!(saved, [3]);
}

#[test]
fn a_sync_function_gains_cache_duties_and_schedules_only() {
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
    let new = body.insts.iter().find(|(_, inst)| matches!(inst.op, Operation::Async(AsyncOp::FutureNew { .. })))
        .unwrap().1;
    let cache = new.results[1];
    assert_eq!(output.types.get(body.values.get(cache).unwrap().ty), Ok(&Type::Service(ServiceType::CacheState)));
    assert_eq!(body.values.get(new.results[2]).unwrap().def, Definition::InstResult {
        inst: body.insts.iter().find(|(_, inst)| std::ptr::eq(*inst, new)).unwrap().0, index: 2,
    });
    assert_eq!(body.service_schedules.len(), 1);
    assert_eq!(body.service_schedules[0].duties, [ScheduleDuty::Value(cache)]);
}
