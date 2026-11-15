//! Commit 5a, plain async (#601 V2): reviewed own goldens and the contracts of
//! the async rewrite. Expectations come from the P2.5 plan §3.2, elaboration
//! §13 and D12/D16/D18, and async ABI C-2, C-3 and D24.2.
use super::*;
use super::synchronous::{fixtures, roundtrip};

fn catalog(module: &Module) -> TestNatives {
    let mut catalog = TestNatives::default();
    for (protocol, data) in module.protocols.iter() {
        if data.kind == ProtocolKind::Services {
            catalog.protocols.insert(protocol, crate::own::futures::settle_summary());
        }
    }
    catalog
}

fn rewrite_async(source: &str) -> Result<OwnOutput, OwnError> {
    let module = crate::text::parse(source).unwrap();
    crate::verify_high(&module).unwrap_or_else(|report| panic!("High input: {report:?}"));
    let natives = catalog(&module);
    let output = elaborate_ownership(module, &natives, &OwnOptions::default())?;
    rewrite_synchronous(output, &OwnOptions::default())
}

fn input(name: &str) -> String {
    let path = fixtures("async-high").into_iter().find(|path| path.file_stem().unwrap() == name).unwrap();
    std::fs::read_to_string(path).unwrap()
}

fn function<'m>(module: &'m Module, symbol: &str) -> (FuncId, &'m Function) {
    module.functions.iter().find(|(_, function)| function.symbol == symbol).unwrap()
}

fn operations(body: &Body) -> Vec<&Operation> {
    body.insts.iter().map(|(_, instruction)| &instruction.op)
        .chain(body.blocks.iter().filter_map(|(_, block)| match &block.term.kind {
            TermKind::InvokeOwn { op, .. } => Some(op.as_ref()),
            _ => None,
        })).collect()
}

// Reviewed expectations; changes require an explanation of the semantic difference.
const WITNESSES: &[&str] = &[
    "async-await-failure",
    "async-caught-throw",
    "async-never-awaited",
    "async-plain-await",
    "async-repeated-await",
    "async-void",
    // The extra shapes of the coro pass (V3): its inputs are these own goldens.
    "coro-nested-cold-await",
    "coro-schedule-duty-live-across-await",
    "coro-start-continuation",
    // Commit 5b (R1): resources across the Future boundary.
    "r1-cached-outcome",
    "r1-cold-escape",
    "r1-make-escape",
    "r1-never-started-close",
    "r1-temporary-take",
    "r1-view-escape",
];

#[test]
fn reviewed_async_goldens_verify() {
    assert_eq!(fixtures("async-high").len(), WITNESSES.len());
    assert_eq!(fixtures("async-goldens").len(), WITNESSES.len());
    let expected = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/own/tests/async-goldens");
    for name in WITNESSES {
        let output = rewrite_async(&input(name)).unwrap_or_else(|error| panic!("{name}: {error}"));
        let printed = crate::text::print(&output.module).unwrap();
        let golden = std::fs::read_to_string(expected.join(format!("{name}.oir"))).unwrap();
        assert_eq!(printed, golden, "review the {name} difference; never auto-bless it");
        let parsed = crate::text::parse(&golden).unwrap();
        for module in [&output.module, &roundtrip(&output.module), &parsed] {
            verify_own(module).unwrap_or_else(|report| panic!("{name}: {report:?}"));
            async_invariants(name, module);
        }
    }
}

#[test]
fn every_async_high_input_declares_the_registered_services_protocol() {
    // D16: the producer declares the registered Services protocol (okkama_oir::catalogue), and so do these
    // High inputs, so own sees the form that real producer output has.
    for path in fixtures("async-high") {
        let module = crate::text::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let services: Vec<_> = module.protocols.iter().filter(|(_, protocol)| protocol.kind == ProtocolKind::Services)
            .map(|(_, protocol)| protocol.clone()).collect();
        assert_eq!(services, [crate::catalogue::services_protocol()], "{}", path.display());
    }
}

/// The own-level contract of plain async, independent of SSA numbering.
fn async_invariants(name: &str, module: &Module) {
    let services: Vec<_> = module.protocols.iter().filter(|(_, protocol)| protocol.kind == ProtocolKind::Services)
        .map(|(id, _)| id).collect();
    assert_eq!(services.len(), 1, "{name}");
    for (id, data) in module.functions.iter() {
        let Some(body) = &data.body else { continue };
        let asynchronous = data.attrs.contains(&FunctionAttr::Async);
        if asynchronous {
            async_summaries(name, module, id, data);
        }
        // D12: own holds no cache-state value.
        for (_, value) in body.values.iter() {
            assert_ne!(module.types.get(value.ty), Ok(&Type::Service(ServiceType::CacheState)), "{name}");
        }
        let operations = operations(body);
        cache_duties(name, body, &operations, &services);
        await_adapters(name, body, &services);
        // D18: cancel=true only on the final successful close of an async action.
        for op in &operations {
            match op {
                Operation::Ownership(OwnershipOp::ScopeEnd { state, effects, .. }) => {
                    if effects.cancel {
                        assert!(asynchronous, "{name}: cancel in a sync owner");
                        let ty = module.types.get(body.values.get(*state).unwrap().ty).unwrap();
                        let Type::Service(ServiceType::Scope(scope)) = ty else { panic!("{name}: {ty:?}") };
                        assert!(module.scopes.get(*scope).unwrap().parent.is_none(), "{name}: nested cancel");
                    }
                    // D24.2: a plain cache settle neither fails nor waits.
                    assert!(effects.failures.is_empty() && !effects.suspends, "{name}");
                }
                Operation::Ownership(OwnershipOp::ScopeUnwind { effects, .. }) => {
                    assert!(!effects.cancel, "{name}");
                }
                _ => {}
            }
        }
    }
}

/// H6 and plan §3.2: two summaries; the body summary is the entry summary.
fn async_summaries(name: &str, module: &Module, id: FuncId, data: &Function) {
    assert_eq!(data.async_body_summary, data.summary, "{name} {}", data.symbol);
    let body = module.summaries.get(data.summary.unwrap()).unwrap();
    assert!(body.contract.suspends, "{name} {}", data.symbol);
    assert!(body.contract.failures.contains(&FailureKind::Cancelled), "{name} {}", data.symbol);
    let creation = module.summaries.get(data.async_creation_summary.unwrap()).unwrap();
    assert!(creation.contract.failures.is_empty() && !creation.contract.suspends, "{name}");
    assert!(matches!(creation.contract.results.as_slice(),
        [ty] if matches!(module.types.get(*ty), Ok(Type::Future(_)))), "{name}");
    // C-2: the runtime starts a non-root body with its accepted input packet.
    if !fixed_root(module, id) {
        assert!(matches!(data.signature.ownership_abi, OwnershipAbi::Explicit { .. }), "{name}");
    }
}

/// D12, D16: each duty of `fut.new ... owner S` is listed once in a Services
/// schedule of S, and binding its Future transfers nothing.
fn cache_duties(name: &str, body: &Body, operations: &[&Operation], services: &[ProtocolId]) {
    for (_, instruction) in body.insts.iter() {
        let Operation::Async(AsyncOp::FutureNew { owner, environment, .. }) = &instruction.op else { continue };
        assert!(matches!(environment, FutureEnvironment::Deferred { .. }), "{name}");
        let future = instruction.results[0];
        let entries = body.service_schedules.iter()
            .filter(|schedule| schedule.owner == *owner && services.contains(&schedule.protocol))
            .flat_map(|schedule| &schedule.duties).filter(|duty| **duty == ScheduleDuty::Value(future)).count();
        assert_eq!(entries, 1, "{name}: cache duty of {future:?}");
        assert!(!operations.iter().any(|op| matches!(op, Operation::Ownership(OwnershipOp::Bind { binding })
            if binding.resource.value == future)), "{name}");
    }
}

/// Elaboration §13.3, C-3: an await's unwind landing first consumes its task and receipt.
fn await_adapters(name: &str, body: &Body, services: &[ProtocolId]) {
    for (_, block) in body.blocks.iter() {
        let TermKind::InvokeOwn { op, unwind, .. } = &block.term.kind else { continue };
        if !matches!(op.as_ref(), Operation::Async(AsyncOp::Await { .. })) {
            continue;
        }
        let landing = body.blocks.get(unwind.block).unwrap();
        let first = &body.insts.get(landing.insts[0]).unwrap().op;
        let Operation::Async(AsyncOp::AwaitUnwind { task, receipt, protocol, .. }) = first else {
            panic!("{name}: await unwind starts with {first:?}");
        };
        assert_eq!([*task, *receipt], [landing.params[1], landing.params[2]], "{name}");
        assert!(services.contains(protocol), "{name}");
    }
}

/// Counts of creations, awaits, adapters, final cancel closes, catches and
/// schedule sizes of main.
type Shape = (usize, usize, usize, usize, usize, Vec<usize>);

fn shape(name: &str) -> Shape {
    let output = rewrite_async(&input(name)).unwrap();
    let (_, main) = function(&output.module, "main");
    let body = main.body.as_ref().unwrap();
    let operations = operations(body);
    let count = |test: &dyn Fn(&Operation) -> bool| operations.iter().filter(|op| test(op)).count();
    let creations = count(&|op| matches!(op, Operation::Async(AsyncOp::FutureNew { .. })));
    let awaits = count(&|op| matches!(op, Operation::Async(AsyncOp::Await { .. })));
    let adapters = count(&|op| matches!(op, Operation::Async(AsyncOp::AwaitUnwind { .. })));
    let finals = count(&|op| matches!(op, Operation::Ownership(OwnershipOp::ScopeEnd { effects, .. })
        if effects.cancel));
    let catches = body.blocks.iter().filter(|(_, block)| matches!(block.term.kind, TermKind::Catch { .. })).count();
    let schedules = body.service_schedules.iter().map(|schedule| schedule.duties.len()).collect();
    (creations, awaits, adapters, finals, catches, schedules)
}

#[test]
fn every_async_golden_has_its_spec_shape() {
    // future.lazy-and-cached: one creation, two awaits of the same cache.
    assert_eq!(shape("async-repeated-await"), (1, 2, 2, 1, 0, vec![1]));
    assert_eq!(shape("async-plain-await"), (1, 1, 1, 1, 0, vec![1]));
    // failure.rethrown-at-every-await: both awaits rethrow; the first is caught.
    assert_eq!(shape("async-await-failure"), (1, 2, 2, 1, 1, vec![1]));
    // void.await-for-effect: two cache duties of one scope, three awaits.
    assert_eq!(shape("async-void"), (2, 3, 3, 1, 0, vec![2]));
    // future.never-awaited: a sync owner settles a never-started Future, no cancellation.
    assert_eq!(shape("async-never-awaited"), (1, 0, 0, 0, 0, vec![1]));
}

#[test]
fn creating_a_future_never_runs_the_body() {
    // Elaboration §3.3: creation and body equations stay separate. The body may
    // fail and suspend; creating its Future does neither, so the creating main
    // of async-never-awaited stays synchronous and failure-free.
    let output = rewrite_async(&input("async-never-awaited")).unwrap();
    let (_, explode) = function(&output.module, "explode");
    let body = output.module.summaries.get(explode.async_body_summary.unwrap()).unwrap();
    assert!(body.contract.failures.contains(&FailureKind::Panic) && body.contract.suspends);
    let (_, main) = function(&output.module, "main");
    let summary = output.module.summaries.get(main.summary.unwrap()).unwrap();
    assert!(summary.contract.failures.is_empty() && !summary.contract.suspends);
}

#[test]
fn a_high_async_call_creates_a_future() {
    // Plan §3.1: a High call of an async entry has the result future<T>.
    let source = input("async-plain-await");
    crate::verify_high(&crate::text::parse(&source).unwrap()).unwrap();
    let wrong = source.replace("%f: future<int> = call @work(%seven)", "%f: int = call @work(%seven)")
        .replace("binding b0 \"pending\": future<int> in S", "binding b0 \"pending\": int in S")
        .replace("%v: int = await %f, result int", "%v: int = int.add %f, %f");
    let report = crate::verify_high(&crate::text::parse(&wrong).unwrap()).unwrap_err();
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == RuleId::T2), "{report:?}");
}

fn with_schedule(duties: &str) -> String {
    let binding = "    binding b0 \"pending\": future<void> in S\n";
    let schedule = format!("    service.schedule Order owner S, protocol Services{duties}\n{binding}");
    input("async-void").replace(binding, &schedule)
}

#[test]
fn a_high_schedule_of_the_owner_receives_its_cache_duties() {
    // D16: own fills an empty High schedule of the owner instead of declaring a second one.
    let output = rewrite_async(&with_schedule("")).unwrap();
    let (_, main) = function(&output.module, "main");
    let schedules = &main.body.as_ref().unwrap().service_schedules;
    assert_eq!(schedules.len(), 1);
    assert_eq!(schedules[0].name, "Order");
    assert_eq!(schedules[0].duties.len(), 2);
}

#[test]
fn a_high_schedule_order_is_kept_when_it_names_the_owners_duties() {
    // D16: the concrete service order is an admissible schedule input.
    let output = rewrite_async(&with_schedule(", duties [%f, %temporary]")).unwrap();
    let (_, main) = function(&output.module, "main");
    let body = main.body.as_ref().unwrap();
    let futures: Vec<_> = body.insts.iter().filter_map(|(_, instruction)| {
        matches!(instruction.op, Operation::Async(AsyncOp::FutureNew { .. })).then_some(instruction.results[0])
    }).collect();
    assert_eq!(body.service_schedules[0].duties, [ScheduleDuty::Value(futures[1]), ScheduleDuty::Value(futures[0])]);
}

#[test]
fn a_high_schedule_that_misses_a_duty_of_its_owner_is_an_equation_error() {
    let error = rewrite_async(&with_schedule(", duties [%f]")).unwrap_err();
    let OwnError::Diagnostics { diagnostics, .. } = error else { panic!("{error:?}") };
    assert_eq!(diagnostics[0].kind, OwnDiagnosticKind::InvalidEquation);
}

#[test]
fn await_unwind_requires_the_services_protocol() {
    let source = input("async-plain-await");
    let start = source.find("protocol Services").unwrap();
    let end = source.find("scope SW").unwrap();
    let source = format!("{}{}", &source[..start], &source[end..]);
    let module = crate::text::parse(&source).unwrap();
    let output = elaborate_ownership(module, &TestNatives::default(), &OwnOptions::default()).unwrap();
    let error = rewrite_subset(output, &OwnOptions::default()).unwrap_err();
    let OwnError::Diagnostics { diagnostics, .. } = error else { panic!("{error:?}") };
    assert_eq!(diagnostics[0].kind, OwnDiagnosticKind::MissingBoundary);
}

/// OIR-2 §10.1, #587: the creation of a cold Future retains the fresh part of its managed input. The offered packet
/// comes out of the creating scope, the creation summary takes the parameter into the Future's cache, and the
/// environment names the argument as the offer left it.
#[test]
fn r1_a_cold_future_retains_its_fresh_input() {
    let output = rewrite_async(&input("r1-never-started-close")).unwrap();
    let module = &output.module;
    let (_, consume) = function(module, "consume");
    let creation = module.summaries.get(consume.async_creation_summary.unwrap()).unwrap();
    assert_eq!(creation.parameters[0].convention, FreshConvention::Take);
    assert_eq!(creation.parameters[0].destination, TransferDestination::Cache);
    assert_eq!(creation.parameters[0].retention, Some(SummarySelection::Parameter(0)));
    let (_, main) = function(module, "main");
    let body = main.body.as_ref().unwrap();
    let (packet, environment) = body.insts.iter().find_map(|(_, instruction)| match &instruction.op {
        Operation::Async(AsyncOp::FutureNew { packet, environment, .. }) => Some((*packet, environment.clone())),
        _ => None,
    }).unwrap();
    // The input packet is not an empty packet: it joins what the scope split offered.
    let Definition::InstResult { inst, .. } = body.values.get(packet).unwrap().def else { panic!("packet") };
    assert!(matches!(body.insts.get(inst).unwrap().op, Operation::Ownership(OwnershipOp::Join { .. })));
    let offers = body.blocks.iter().filter(|(_, block)| matches!(&block.term.kind,
        TermKind::InvokeOwn { op, .. } if matches!(op.as_ref(), Operation::Ownership(OwnershipOp::ScopeSplit { .. }))))
        .count();
    assert_eq!(offers, 1, "the fresh Res is offered once, from main's scope");
    let FutureEnvironment::Deferred { arguments, .. } = environment else { panic!("deferred environment") };
    let argument = body.values.get(arguments[0]).unwrap();
    assert!(matches!(argument.def, Definition::BlockParam { .. }), "the argument after the offer's edge");
}

/// OIR-2 §10.2, elaboration §13.2: a shared await of a resource outcome borrows the cached outcome. It yields the
/// value and its deps and no packet; the body publishes a fresh result.
#[test]
fn r1_a_shared_await_borrows_the_cached_outcome() {
    let output = rewrite_async(&input("r1-cached-outcome")).unwrap();
    let module = &output.module;
    let (_, work) = function(module, "work");
    let body = module.summaries.get(work.async_body_summary.unwrap()).unwrap();
    assert!(!body.results[0].fresh.is_empty(), "work's outcome is fresh");
    let (_, main) = function(module, "main");
    let main_body = main.body.as_ref().unwrap();
    let mut awaits = 0;
    for (_, block) in main_body.blocks.iter() {
        let TermKind::InvokeOwn { op, normal, .. } = &block.term.kind else { continue };
        if !matches!(op.as_ref(), Operation::Async(AsyncOp::Await { .. })) {
            continue;
        }
        awaits += 1;
        let produced: Vec<_> = normal.args.iter().filter_map(|arg| match arg {
            EdgeArg::Produced(result) => Some(result.slot),
            EdgeArg::Value(_) => None,
        }).collect();
        assert_eq!(produced, [0, 1], "value and deps, no packet");
        let view = main_body.blocks.get(normal.block).unwrap().params[0];
        assert!(matches!(main_body.values.get(view).unwrap().facts, ValueFacts::Managed { .. }));
    }
    assert_eq!(awaits, 2);
}

/// The `cache.rehome`s of a function: (owner scope of the state, target).
fn rehomes(module: &Module, body: &Body) -> Vec<(ScopeId, RehomeTarget)> {
    body.insts.iter().filter_map(|(_, instruction)| match &instruction.op {
        Operation::Async(AsyncOp::CacheRehome { form: RehomeForm::Scope { state, .. }, target, .. }) => {
            let Ok(Type::Service(ServiceType::Scope(scope))) = module.types.get(body.values.get(*state).unwrap().ty)
            else { panic!("rehome state") };
            Some((*scope, *target))
        }
        _ => None,
    }).collect()
}

fn scope_named(module: &Module, function: FuncId, parent: bool) -> ScopeId {
    module.scopes.iter().find(|(_, scope)| scope.function == function && scope.parent.is_none() != parent)
        .map(|(id, _)| id).unwrap()
}

/// OIR-2 §10.1 make(), async ABI C-4 and D25.1: a returned cold Future takes its duty and its fresh inputs to the
/// caller. make rehomes the duty into the pending reserve of its return (target caller), its summary marks the
/// Future's fresh family and returns the inputs with it, and main's scope that admits the packet settles it by the
/// dynamic step (D25.3). When the Future leaves main's inner block, its duty is rehomed again (target S).
#[test]
fn r1_a_returned_future_takes_its_cache_duty_to_the_caller() {
    let output = rewrite_async(&input("r1-make-escape")).unwrap();
    let module = &output.module;
    let (make_id, make) = function(module, "make");
    let body = make.body.as_ref().unwrap();
    assert_eq!(rehomes(module, body), [(scope_named(module, make_id, false), RehomeTarget::Caller)]);
    // No split selects the Future: its duty leaves only through the rehome.
    for (_, block) in body.blocks.iter() {
        if let TermKind::InvokeOwn { op, .. } = &block.term.kind
            && let Operation::Ownership(OwnershipOp::ScopeSplit { selection, .. }) = op.as_ref()
            && let Ok(SelectionExpr::Value(value)) = body.selections.get(*selection) {
            assert!(!matches!(module.types.get(body.values.get(*value).unwrap().ty), Ok(Type::Future(_))),
                "a Future's duty never leaves by a split");
        }
    }
    let summary = module.summaries.get(make.summary.unwrap()).unwrap();
    assert_eq!(summary.results[0].fresh.len(), 3, "the Future's duty and the fresh R and A");
    assert_eq!(summary.parameters[0].destination, TransferDestination::Result);
    let (main_id, main) = function(module, "main");
    let main_body = main.body.as_ref().unwrap();
    let outer = scope_named(module, main_id, false);
    let inner = scope_named(module, main_id, true);
    assert_eq!(rehomes(module, main_body), [(inner, RehomeTarget::Scope(outer))]);
    for scope in [inner, outer] {
        let dynamic = main_body.service_schedules.iter().filter(|schedule| schedule.owner == scope)
            .flat_map(|schedule| &schedule.duties).filter(|duty| **duty == ScheduleDuty::Dynamic).count();
        assert_eq!(dynamic, 1, "{scope:?} admits a service entry");
    }
}

/// OIR-2 §10.2, C-4: a cold Future created in an inner block and a view of a cached outcome escaping it both
/// rehome the cache duty to the block's destination; the commit names the same scope.
#[test]
fn r1_an_escape_from_a_block_rehomes_into_the_destination() {
    for name in ["r1-cold-escape", "r1-view-escape"] {
        let output = rewrite_async(&input(name)).unwrap();
        let module = &output.module;
        let (main_id, main) = function(module, "main");
        let body = main.body.as_ref().unwrap();
        let outer = scope_named(module, main_id, false);
        let inner = scope_named(module, main_id, true);
        assert_eq!(rehomes(module, body), [(inner, RehomeTarget::Scope(outer))], "{name}");
        let commits: Vec<_> = body.insts.iter().filter_map(|(_, instruction)| match &instruction.op {
            Operation::Exit(ExitOp::Commit { destination, .. }) => Some(*destination),
            _ => None,
        }).collect();
        assert_eq!(commits, [ExitDestination::Scope(outer)], "{name}");
        // The inner block keeps its static duty; the outer scope admits the entry.
        let duties = |scope: ScopeId| body.service_schedules.iter().filter(|schedule| schedule.owner == scope)
            .flat_map(|schedule| schedule.duties.clone()).collect::<Vec<_>>();
        assert!(matches!(duties(inner).as_slice(), [ScheduleDuty::Value(_)]), "{name}");
        assert_eq!(duties(outer), [ScheduleDuty::Dynamic], "{name}");
    }
}

fn golden(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("src/own/tests/async-goldens/{name}.oir"));
    std::fs::read_to_string(path).unwrap()
}

/// The diagnostics of the full own verifier, with their locations.
fn rejections(text: &str) -> Vec<(RuleId, EntityLocation, String)> {
    let module = crate::text::parse(text).unwrap();
    match verify_own(&module) {
        Ok(_) => vec![],
        Err(report) => report.diagnostics.into_iter()
            .map(|diagnostic| (diagnostic.rule, diagnostic.primary.entity, diagnostic.message)).collect(),
    }
}

fn rejected_by(text: &str, rule: RuleId, message: &str) -> bool {
    rejections(text).iter().any(|(actual, _, text)| *actual == rule && text == message)
}

fn edit(text: &str, from: &str, to: &str) -> String {
    assert!(text.contains(from), "{from}");
    text.replacen(from, to, 1)
}

const COLD_REHOME: &str = "(%46: scope<S4>, %47: pending) = cache.rehome %42, %45, future %41, target S3, protocol P0";

/// C-4, ruling of #601 R1: the target of a rehome is a live scope that encloses the duty's owner, or the caller.
#[test]
fn r1_a_rehome_target_encloses_the_owner_scope() {
    let valid = golden("r1-cold-escape");
    assert_eq!(rejections(&valid), []);
    let message = "cache rehome requires a live sufficient target boundary";
    let inner = edit(&valid, COLD_REHOME, &COLD_REHOME.replace("target S3", "target S4"));
    assert!(rejected_by(&inner, RuleId::B94, message), "{:?}", rejections(&inner));
}

/// C-4 step 2: the rehomed duties reach the destination their rehome named; the commit checks it.
#[test]
fn r1_a_rehome_names_the_destination_of_its_exit() {
    let message = "rehomed cache duties reach a destination other than their rehome target";
    let valid = golden("r1-cold-escape");
    let caller = edit(&valid, COLD_REHOME, &COLD_REHOME.replace("target S3", "target caller"));
    assert!(rejected_by(&caller, RuleId::B94, message), "{:?}", rejections(&caller));
    let make = golden("r1-make-escape");
    let returned = "cache.rehome %47, %50, future %46, target caller, protocol P0";
    assert!(make.contains(returned));
    // make's own scope is no destination of its return.
    let own = edit(&make, returned, &returned.replace("target caller", "target S3"));
    let boundary = "cache rehome requires a live sufficient target boundary";
    assert!(rejected_by(&own, RuleId::B94, boundary), "{:?}", rejections(&own));
}

/// C-4: a rehome moves the cache duty its scope state holds; a second rehome of the same duty finds none, and
/// the duties of one pending exit name one destination.
#[test]
fn r1_a_rehome_moves_a_duty_its_state_holds() {
    let valid = golden("r1-cold-escape");
    let again = |target: &str| {
        let second = format!("{COLD_REHOME}\n    (%90: scope<S4>, %91: pending) = cache.rehome %46, %47, future %41, \
            target {target}, protocol P0");
        let text = edit(&valid, COLD_REHOME, &second);
        let text = edit(&text, "%49: pending = exit.extend %47, %48", "%49: pending = exit.extend %91, %48");
        edit(&text, "scope.end %46, reserve %49", "scope.end %90, reserve %49")
    };
    let twice = again("S3");
    let absent = "cache.rehome names a Future whose cache duty its scope state does not hold";
    assert!(rejected_by(&twice, RuleId::B94, absent), "{:?}", rejections(&twice));
    let conflict = again("caller");
    let message = "the duties rehomed into one pending exit name different destinations";
    assert!(rejected_by(&conflict, RuleId::B94, message), "{:?}", rejections(&conflict));
}

/// The pre-C-4 form over one cache-state token keeps naming a scope.
#[test]
fn r1_the_token_state_rehome_names_a_scope() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/spec/10-cache-rehome.oir");
    let valid = std::fs::read_to_string(path).unwrap();
    let message = "the pre-C-4 cache.rehome form needs a target scope";
    assert!(!rejected_by(&valid, RuleId::B94, message));
    let caller = edit(&valid, "target Souter", "target caller");
    assert!(rejected_by(&caller, RuleId::B94, message), "{:?}", rejections(&caller));
}

/// OIR-2 §10.2, §10.3, elaboration §13.2: the awaiting block takes a temporary Future of a managed outcome.
/// The proof carries the full evidence (its creation, no alias, frame slot, storage, waiter, view or collector, the block of
/// the take) and the take's boundary is the awaiting scope, which admits the actual outcome packet. A bound or a
/// plain temporary Future stays a shared await.
#[test]
fn r1_a_temporary_future_is_taken_by_its_awaiting_scope() {
    let output = rewrite_async(&input("r1-temporary-take")).unwrap();
    let module = &output.module;
    let (main_id, main) = function(module, "main");
    let body = main.body.as_ref().unwrap();
    let inner = scope_named(module, main_id, true);
    let (proof_block, proof) = body.blocks.iter().find_map(|(id, block)| block.insts.iter().find_map(|inst| {
        match &body.insts.get(*inst).unwrap().op {
            Operation::Async(AsyncOp::UniqueCacheProof { proof }) => Some((id, proof.clone())),
            _ => None,
        }
    })).unwrap();
    let Definition::InstResult { inst, .. } = body.values.get(proof.future).unwrap().def else { panic!("future") };
    assert!(matches!(body.insts.get(inst).unwrap().op, Operation::Async(AsyncOp::FutureNew { owner, .. })
        if owner == inner));
    assert_eq!(proof.scope, inner);
    assert_eq!(proof.region, [proof_block]);
    assert_eq!(proof.evidence, UniqueCacheEvidence {
        creation: CreationOrigin::Instruction(inst), aliases: vec![], frame_slots: vec![], no_external_storage: true,
        no_other_waiters: true, no_live_views: true, no_collector_retention: true, scheduler_cannot_publish: true,
    });
    let TermKind::InvokeOwn { op, normal, .. } = &body.blocks.get(proof_block).unwrap().term.kind else {
        panic!("take")
    };
    let Operation::Async(AsyncOp::AwaitTake { future, boundary, .. }) = op.as_ref() else { panic!("{op:?}") };
    assert_eq!((*future, *boundary), (proof.future, TakeBoundary::Temporary { awaiting: inner }));
    // The landing admits the taken packet into the awaiting scope, and no edge carries the Future on.
    let landing = body.blocks.get(normal.block).unwrap();
    let packet = landing.params[1];
    assert!(landing.insts.iter().any(|inst| matches!(&body.insts.get(*inst).unwrap().op,
        Operation::Ownership(OwnershipOp::Admit { packet: admitted, .. }) if *admitted == packet)));
    assert!(!normal.args.contains(&EdgeArg::Value(proof.future)));
    for name in ["r1-cached-outcome", "async-void"] {
        let output = rewrite_async(&input(name)).unwrap();
        let (_, main) = function(&output.module, "main");
        assert!(!operations(main.body.as_ref().unwrap()).iter().any(|op| matches!(op,
            Operation::Async(AsyncOp::AwaitTake { .. } | AsyncOp::UniqueCacheProof { .. }))), "{name}");
    }
}

/// The main of async-repeated-await with its creation and await in a loop of the root scope, or in an
/// iteration scope that the loop opens on every round.
fn creation_in_loop(iteration_scope: bool) -> String {
    let source = input("async-repeated-await");
    let start = source.find("    binding b0 \"pending\"").unwrap();
    let end = source.find("    ^failed(").unwrap();
    let mut lines = vec!["    boundary Bauto lexical in S"];
    if iteration_scope {
        lines.push("    boundary BI lexical in SI");
    }
    lines.extend(["    ^entry in S unwind ^failed:", "    %zero: int = const 0"]);
    if iteration_scope {
        lines.extend(["    br ^head(%zero)", "    ^head(%h: int) in S unwind ^failed:", "    br ^loop(%h)",
            "    ^loop(%i: int) in SI unwind ^failed:"]);
    } else {
        lines.extend(["    br ^loop(%zero)", "    ^loop(%i: int) in S unwind ^failed:"]);
    }
    lines.extend(["    %f: future<int> = call @work(%i)", "    %x: int = await %f, result int",
        "    %lim: int = const 3", "    %c: bool = int.lt %x, %lim"]);
    if iteration_scope {
        lines.extend(["    want %x", "    [ip0] exit.prepare fallthrough %x at BI",
            "    result.exit edge normal(0), value %x, demand want, prepare ip0",
            "    result.exit edge normal(1), value %x, demand want, prepare ip0",
            "    cond %c, ^head(%x), ^done(%x)"]);
    } else {
        lines.push("    cond %c, ^loop(%x), ^done(%x)");
    }
    lines.extend(["    ^done(%r: int) in S unwind ^failed:", "    want %r",
        "    [iprep1] exit.prepare return %r at Bauto",
        "    result.exit edge normal(0), value %r, demand want, prepare iprep1", "    ret %r", ""]);
    let source = format!("{}{}{}", &source[..start], lines.join("\n"), &source[end..]);
    if iteration_scope {
        let root = "scope S action in @main\n";
        return source.replace(root, &format!("{root}scope SI iteration in @main parent S\n"));
    }
    source
}

#[test]
fn a_future_created_in_a_loop_of_its_owner_is_unsupported() {
    // D25 (OIR-2 B10.6): one fut.new site names one static duty of its owner. A loop of the owner reaches the
    // site again while the owner holds the earlier duty; such duties are dynamic and arrive with R1.
    let error = rewrite_async(&creation_in_loop(false)).unwrap_err();
    let OwnError::Diagnostics { diagnostics, .. } = error else { panic!("{error:?}") };
    assert_eq!(diagnostics[0].kind, OwnDiagnosticKind::UnsupportedAsync);
    assert!(diagnostics[0].message.contains("fut.new inside a loop of its owner"), "{diagnostics:?}");
    // An iteration scope owns the Future of each round and closes it before the next one.
    let output = rewrite_async(&creation_in_loop(true)).unwrap();
    crate::verify_own(&output.module).unwrap();
}
