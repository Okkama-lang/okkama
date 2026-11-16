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
    "r1-loop-escape",
    "r1-make-escape",
    "r1-never-started-close",
    "r1-plain-temporary",
    "r1-temporary-take",
    "r1-view-escape",
    // K1: a cold Future cancelled before it starts; the elaboration §13.4 fixture; a nested close before the
    // next await.
    "k1-cancel-unstarted",
    "k1-failure-view-escapes",
    "k1-nested-close-next-await",
    // #684: a nested close while a Future of an enclosing scope is live.
    "nested-close-outer-future",
    // #691: a void fixed root (an async test); the deps of a Future argument in continuation 0.
    "void-fixed-root",
    "start-deps",
    // #733: an async successor action publishes its successor.
    "async-successor",
    // #730, C-6: async defers whose entry awaits, after a body failure and across a pending exit.
    "r2-defer-await-body-failure",
    "r2-defer-await-reserve",
    // #601 P1, D24.1: spawn of a fresh Future forwards its owner state; awaited, never awaited, in a sync owner.
    "p1-spawn-awaited",
    "p1-spawn-detached",
    "p1-sync-owner-spawn",
    "p1-spawn-returned",
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
        // C-6 (#730): an async defer entry runs inside its owner's cleanup; it creates no Future, has only its
        // entry summary and never delivers cancellation at its close.
        let defer_entry = data.attrs.iter().any(|attr| matches!(attr, FunctionAttr::DeferEntry { .. }));
        let asynchronous = data.attrs.contains(&FunctionAttr::Async) && !defer_entry;
        if asynchronous {
            async_summaries(name, module, id, data);
        }
        if defer_entry && data.attrs.contains(&FunctionAttr::Async) {
            assert_eq!((data.async_creation_summary, data.async_body_summary), (None, None), "{name}");
            assert!(module.summaries.get(data.summary.unwrap()).unwrap().contract.suspends, "{name}");
            assert!(matches!(data.signature.ownership_abi, OwnershipAbi::Explicit { .. }), "{name}");
        }
        // D12: own holds no cache-state value.
        for (_, value) in body.values.iter() {
            assert_ne!(module.types.get(value.ty), Ok(&Type::Service(ServiceType::CacheState)), "{name}");
        }
        let operations = operations(body);
        // Own projects the failures of a scope's defers into its close envelope, as for sync defers; the
        // close runs no defer, so a defer never makes it suspend (#730).
        let deferred: Vec<FailureKind> = body.blocks.iter().filter_map(|(_, block)| match &block.term.kind {
            TermKind::InvokeDefer { call, .. } => call.summary,
            _ => None,
        }).flat_map(|summary| module.summaries.get(summary).unwrap().contract.failures.clone()).collect();
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
                    assert!(effects.failures.iter().all(|kind| deferred.contains(kind)) && !effects.suspends,
                        "{name}");
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
    for name in ["r1-cached-outcome", "async-void", "r1-plain-temporary"] {
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



/// C-4 through a loop exit: the commit names the loop boundary, which has no scope, so the rehome names the scope
/// where the exit lands (the block SO around the loop); the duty then leaves SO for S as its value.
#[test]
fn r1_a_loop_exit_rehomes_into_the_scope_where_it_lands() {
    let output = rewrite_async(&input("r1-loop-escape")).unwrap();
    let module = &output.module;
    let (main_id, main) = function(module, "main");
    let body = main.body.as_ref().unwrap();
    let scope = |kind: ScopeKind| module.scopes.iter().find(|(_, data)| data.function == main_id && data.kind == kind)
        .map(|(id, _)| id).unwrap();
    let (root, block, iteration) = (scope(ScopeKind::Action), scope(ScopeKind::Block), scope(ScopeKind::Iteration));
    let mut found = rehomes(module, body);
    found.sort_by_key(|(scope, _)| scope.as_u32());
    let mut expected = vec![(iteration, RehomeTarget::Scope(block)), (block, RehomeTarget::Scope(root))];
    expected.sort_by_key(|(scope, _)| scope.as_u32());
    assert_eq!(found, expected);
    let commits: Vec<_> = body.insts.iter().filter_map(|(_, instruction)| match &instruction.op {
        Operation::Exit(ExitOp::Commit { destination, .. }) => Some(*destination),
        _ => None,
    }).collect();
    assert!(commits.iter().any(|destination| matches!(destination, ExitDestination::Loop(_))), "{commits:?}");
    assert!(commits.contains(&ExitDestination::Scope(root)), "{commits:?}");
    // A loop exit's rehome that names a scope other than the landing one is refused where the packet lands.
    let golden = golden("r1-loop-escape");
    let rehome = "future %78, target S4, protocol P0";
    let elsewhere = edit(&golden, rehome, &rehome.replace("target S4", "target S3"));
    let message = "committed packet must be admitted into its declared destination";
    assert!(rejected_by(&elsewhere, RuleId::B101, message), "{:?}", rejections(&elsewhere));
}

/// OIR-2 §10.2, D12 (`future.passed-and-awaited-elsewhere`): a Future passed to an async entry stays in the cache
/// of its owner. The creation of the outer Future offers nothing for it, so no selection names the inner Future:
/// its duty never travels through an offer, and a run-time selection walk never meets a Future.
#[test]
fn r1_a_future_input_of_a_cold_future_is_never_offered() {
    let source = input("async-plain-await")
        .replace("scope S action in @main\n", "scope SO action in @outer\nscope S action in @main\n")
        .replace("func @main()", "summary MouterC call-boundary parameters [parameter_effect(convention=borrow, \
            destination=none)] results [result_relation(provenance=unbound_result)] contract \
            call_contract(results=[future<int>], publication=atomic)
summary MouterB call-boundary parameters [parameter_effect(convention=borrow, destination=none)] results \
            [result_relation(provenance=unbound_result)] contract call_contract(results=[int], failures=[thrown, \
            program, panic, native, cancelled], suspends=true, publication=atomic)
func @outer(%g: future<int>) -> int [async] async-creation(MouterC) async-body(MouterB) {
    boundary BO lexical in SO
    ^entry in SO unwind ^failed:
    %n: int = await %g, result int
    want %n
    [iprepo] exit.prepare return %n at BO
    result.exit edge normal(0), value %n, demand want, prepare iprepo
    ret %n
    ^failed(%failure: failure) in SO:
    raise %failure
}
func @main()")
        .replace("    %v: int = await %f, result int\n", "    %h: future<int> = call @outer(%f)\n    temp %h, S\n    \
            %v: int = await %h, result int\n");
    let output = rewrite_async(&source).unwrap();
    verify_own(&output.module).unwrap();
    let (_, main) = function(&output.module, "main");
    let body = main.body.as_ref().unwrap();
    let mut creations = 0;
    for op in operations(body) {
        let selection = match op {
            Operation::Ownership(OwnershipOp::ScopeSplit { selection, .. } | OwnershipOp::Split { selection, .. }) => {
                *selection
            }
            Operation::Async(AsyncOp::FutureNew { .. }) => {
                creations += 1;
                continue;
            }
            _ => continue,
        };
        let mut pending = vec![selection];
        while let Some(selection) = pending.pop() {
            match body.selections.get(selection).unwrap() {
                SelectionExpr::Union(items) => pending.extend(items),
                SelectionExpr::Admissible(inner) => pending.push(*inner),
                SelectionExpr::Value(value) => {
                    let ty = output.module.types.get(body.values.get(*value).unwrap().ty).unwrap();
                    assert!(!matches!(ty, Type::Future(_)), "a selection names a Future: {op:?}");
                }
                _ => {}
            }
        }
    }
    assert_eq!(creations, 2);
}

/// K1 (elaboration §13.1, Champ draft 3 §5): a cancellation request and the Future queries only observe the
/// Future. Own keeps them as they are, on the renamed Future, and the cancelled cold Future keeps its cache duty
/// in the schedule of its owner, which settles it at the close.
#[test]
fn k1_cancellation_requests_observe_the_future_and_keep_its_duty() {
    let output = rewrite_async(&input("k1-cancel-unstarted")).unwrap();
    let module = &output.module;
    let (_, main) = function(module, "main");
    let body = main.body.as_ref().unwrap();
    let created: Vec<ValueId> = body.insts.iter()
        .filter(|(_, inst)| matches!(inst.op, Operation::Async(AsyncOp::FutureNew { .. })))
        .map(|(_, inst)| inst.results[0]).collect();
    assert_eq!(created.len(), 1);
    let observed: Vec<ValueId> = body.insts.iter().filter_map(|(_, inst)| match inst.op {
        Operation::Async(AsyncOp::FutureCancel { future } | AsyncOp::FutureDone { future }
            | AsyncOp::FutureCancelled { future }) => Some(future),
        _ => None,
    }).collect();
    assert_eq!(observed.len(), 4, "two requests, done and cancelled");
    // The two requests name the Future that fut.new created; the queries read it after the try.
    assert_eq!(observed.iter().filter(|future| **future == created[0]).count(), 2);
    let root = module.scopes.iter().find(|(_, scope)| scope.function == function(module, "main").0
        && scope.parent.is_none()).unwrap().0;
    let schedule = body.service_schedules.iter().find(|schedule| schedule.owner == root).unwrap();
    assert_eq!(schedule.duties, [ScheduleDuty::Value(created[0])]);
}

/// The H6 diagnostics of `verify_high` on a High input.
fn high_rules(text: &str) -> Vec<RuleId> {
    let module = crate::text::parse(text).unwrap();
    match crate::verify_high(&module) {
        Ok(_) => vec![],
        Err(report) => report.diagnostics.into_iter().map(|diagnostic| diagnostic.rule).collect(),
    }
}

const CLOSER: &str = "func @closer() -> void [defer(), async] async-body(Mcl) {";

#[test]
fn c6_an_async_defer_entry_has_a_body_summary_and_no_creation_summary() {
    // #730, async ABI C-6: a defer entry creates no Future. H6 asks it for its body summary only; every other
    // async action keeps both summaries.
    let source = input("r2-defer-await-body-failure");
    assert_eq!(high_rules(&source), []);
    let creation = edit(&source, "[defer(), async] async-body(Mcl)",
        "[defer(), async] async-creation(MtC) async-body(Mcl)");
    assert!(high_rules(&creation).contains(&RuleId::H6), "a defer entry with a creation summary");
    let bodiless = edit(&source, CLOSER, "func @closer() -> void [defer(), async] {");
    assert!(high_rules(&bodiless).contains(&RuleId::H6), "a defer entry without a body summary");
    let task = edit(&source, "func @task() -> void [async] async-creation(MtC) async-body(MtB) {",
        "func @task() -> void [async] async-body(MtB) {");
    assert!(high_rules(&task).contains(&RuleId::H6), "an async action without a creation summary");
}

#[test]
fn c6_own_runs_an_async_defer_entry_with_its_input_packet() {
    // #730, C-6, plan §3.2: own accepts the suspending defer of an async owner. The entry keeps no creation
    // summary and takes the explicit packet ABI; each exit of f runs it through `invoke.defer` with an input
    // packet, and f's closes do not suspend for it (the close runs no defer).
    let output = rewrite_async(&input("r2-defer-await-body-failure")).unwrap();
    let module = &output.module;
    let (closer, data) = function(module, "closer");
    assert_eq!((data.async_creation_summary, data.async_body_summary), (None, None));
    assert_eq!(data.signature.ownership_abi, OwnershipAbi::Explicit { input_packet: 0, output_packet: 0 });
    let body = function(module, "f").1.body.as_ref().unwrap();
    let packets: Vec<_> = body.blocks.iter().filter_map(|(_, block)| match &block.term.kind {
        TermKind::InvokeDefer { function, call, .. } if *function == closer => Some(call.packet.is_some()),
        _ => None,
    }).collect();
    assert_eq!(packets, [true, true], "the normal and the failure exit of f");
    for op in operations(body) {
        if let Operation::Ownership(OwnershipOp::ScopeEnd { effects, .. } | OwnershipOp::ScopeUnwind { effects, .. })
            = op {
            assert!(!effects.suspends, "{op:?}");
        }
    }
    // A sync owner cannot run a suspending defer.
    let sync = edit(&input("r2-defer-await-body-failure"),
        "func @f() -> void [async] async-creation(MfC) async-body(MfB) {", "func @f() -> void {");
    let error = rewrite_async(&sync).unwrap_err();
    let OwnError::Diagnostics { diagnostics, .. } = error else { panic!("{error:?}") };
    assert_eq!(diagnostics[0].kind, OwnDiagnosticKind::UnsupportedAsync);
    assert!(diagnostics[0].message.contains("may own suspending cleanup"), "{diagnostics:?}");
}

/// #601 Q1, D22: async-plain-await with `@work` a bodyless async intrinsic (`external`) called through
/// `call.intrinsic`, or an async native when `native`. The catalogue gives the external its body summary.
fn driver_source(native: bool) -> String {
    let work = input("async-plain-await");
    let start = work.find("func @work(").unwrap();
    let end = work.find("func @main(").unwrap();
    let (attr, call) = if native {
        ("[async, extern \"q1.work\"]", "call.native @work(%seven)")
    } else {
        ("[async, intrinsic \"q1.work\"]", "call.intrinsic @work(%seven)")
    };
    let declaration = format!("func @work(%x: int) -> int {attr} async-creation(MworkC) async-body(MworkB);\n");
    let source = format!("{}{declaration}{}", &work[..start], &work[end..]);
    edit(&source, "call @work(%seven)", call)
}

/// The trusted catalogue of `@work` at one profile: its declared signature and its body summary.
struct Driver(Signature, Summary);
impl crate::verify::ExternalContracts for Driver {
    fn native(&self, name: &str) -> Option<&Signature> {
        (name == "q1.work").then_some(&self.0)
    }
    fn intrinsic(&self, name: &str) -> Option<&Signature> {
        (name == "q1.work").then_some(&self.0)
    }
    fn native_summary(&self, name: &str) -> Option<&Summary> {
        (name == "q1.work").then_some(&self.1)
    }
    fn intrinsic_summary(&self, name: &str) -> Option<&Summary> {
        (name == "q1.work").then_some(&self.1)
    }
}

fn driver_contracts(module: &Module) -> Driver {
    let (_, work) = function(module, "work");
    let body = work.summary.or(work.async_body_summary).unwrap();
    Driver(work.signature.clone(), module.summaries.get(body).unwrap().clone())
}

/// Own's driver declaration: the call-boundary entry ABI, `(int, own) -> (int, own)`.
fn own_driver(module: &mut Module, body: Summary) -> Driver {
    let int = module.types.intern(Type::Int).unwrap();
    let own = module.types.intern(Type::Service(ServiceType::Own)).unwrap();
    let signature = Signature {
        params: vec![int, own], results: vec![int, own],
        ownership_abi: OwnershipAbi::Explicit { input_packet: 1, output_packet: 1 },
    };
    Driver(signature, body)
}

fn rewrite_driver(source: &str) -> Result<OwnOutput, OwnError> {
    let mut module = crate::text::parse(source).unwrap();
    let contracts = driver_contracts(&module);
    let context = crate::verify::VerifyContext { external_contracts: &contracts, ..Default::default() };
    crate::verify::verify_high_with(&module, &Default::default(), context)
        .unwrap_or_else(|report| panic!("High input: {report:?}"));
    let mut natives = catalog(&module);
    natives.named.insert("q1.work".into(), contracts.1.clone());
    let own = own_driver(&mut module, contracts.1.clone());
    let output = elaborate_ownership(module, &natives, &OwnOptions::default())?;
    let context = crate::verify::VerifyContext { external_contracts: &own, ..Default::default() };
    rewrite_synchronous_with(output, &OwnOptions::default(), context)
}

#[test]
fn q1_an_async_intrinsic_call_creates_an_external_future() {
    // Async ABI D22 §4.2: High types the call as the Future of the driver's result (verified in rewrite_driver);
    // own creates it with `fut.new … environment external`, gives its owner the cache duty and leaves the
    // driver without a body.
    let output = rewrite_driver(&driver_source(false)).unwrap();
    let (work, data) = function(&output.module, "work");
    assert!(data.body.is_none() && data.async_creation_summary.is_some() && data.async_body_summary.is_some());
    let (_, main) = function(&output.module, "main");
    let body = main.body.as_ref().unwrap();
    let environments: Vec<_> = operations(body).into_iter().filter_map(|op| match op {
        Operation::Async(AsyncOp::FutureNew { environment, .. }) => Some(environment.clone()),
        _ => None,
    }).collect();
    assert!(matches!(environments.as_slice(), [FutureEnvironment::External { function, .. }] if *function == work),
        "{environments:?}");
    assert_eq!(body.service_schedules.len(), 1, "the owner settles the driver Future's cache duty");
}

#[test]
fn q1_an_async_native_is_still_refused() {
    // Async natives are slice N1: own refuses them by name before any rewrite.
    let error = rewrite_driver(&driver_source(true)).unwrap_err();
    let OwnError::Diagnostics { diagnostics, .. } = error else { panic!("{error:?}") };
    assert_eq!(diagnostics[0].kind, OwnDiagnosticKind::UnsupportedAsync);
    assert!(diagnostics[0].message.contains("slice N1"), "{diagnostics:?}");
}

#[test]
fn a_future_input_beside_a_managed_input_is_never_offered() {
    // OIR-2 §10.2, D12: a Future input's cache is borrowed, so no selection of the creation names it, also when
    // a managed input (a fresh Res) is offered with it.
    let source = edit(&input("r1-cold-escape"), "func @read(%arg: Res) -> int",
        "func @read(%arg: Res, %g: future<int>) -> int");
    let borrow = "parameter_effect(convention=borrow, destination=none)";
    let source = edit(&source, &format!("summary MreadC call-boundary parameters [{borrow}]"),
        &format!("summary MreadC call-boundary parameters [{borrow}, {borrow}]"));
    let source = edit(&source, &format!("summary MreadB call-boundary parameters [{borrow}]"),
        &format!("summary MreadB call-boundary parameters [{borrow}, {borrow}]"));
    let source = edit(&source, "scope S action in @main\n", "scope SS action in @seed\nscope S action in @main\n");
    let source = edit(&source, "func @main()", "summary MseedC call-boundary results \
        [result_relation(provenance=unbound_result)] contract call_contract(results=[future<int>], publication=atomic)
summary MseedB call-boundary results [result_relation(provenance=unbound_result)] contract \
        call_contract(results=[int], failures=[cancelled], suspends=true, publication=atomic)
func @seed() -> int [async] async-creation(MseedC) async-body(MseedB) {
    boundary BS lexical in SS
    ^entry in SS:
    %n: int = const 2
    want %n
    [iprepS] exit.prepare return %n at BS
    result.exit edge normal(0), value %n, demand want, prepare iprepS
    ret %n
}
func @main()");
    let source = edit(&source, "    %r: Res = call @res()\n    %f: future<int> = call @read(%r)\n",
        "    %r: Res = call @res()\n    %e: future<int> = call @seed()\n    %f: future<int> = call @read(%r, %e)\n");
    let output = rewrite_async(&source).unwrap();
    verify_own(&output.module).unwrap();
    let (_, main) = function(&output.module, "main");
    let body = main.body.as_ref().unwrap();
    // PR-B1 (#784, approved 2026-11-16), item 4: `f` leaves the block SI and is bound in S, so it is an admitted
    // Future whose binding is `scope.split admissible` + `own.bind`. That split names `f` for its binding; no other
    // selection, the creation's offer included, names a Future.
    let binding_split = |block: &Block| match &block.term.kind {
        TermKind::InvokeOwn { op, normal, .. }
            if matches!(**op, Operation::Ownership(OwnershipOp::ScopeSplit { .. })) => {
            body.blocks.get(normal.block).unwrap().insts.first().is_some_and(|inst| matches!(
                body.insts.get(*inst).unwrap().op, Operation::Ownership(OwnershipOp::Bind { .. })))
        }
        _ => false,
    };
    let sites: Vec<(&Operation, bool)> = body.blocks.iter().flat_map(|(_, block)| {
        let binding = binding_split(block);
        let term = match &block.term.kind {
            TermKind::InvokeOwn { op, .. } => Some((op.as_ref(), binding)),
            _ => None,
        };
        block.insts.iter().map(|inst| (&body.insts.get(*inst).unwrap().op, false)).chain(term)
    }).collect();
    let mut offers = 0;
    let mut bound = 0;
    for (op, binding) in sites {
        let Operation::Ownership(OwnershipOp::ScopeSplit { selection, .. } | OwnershipOp::Split { selection, .. })
            = op else { continue };
        let mut pending = vec![*selection];
        while let Some(selection) = pending.pop() {
            match body.selections.get(selection).unwrap() {
                SelectionExpr::Union(items) => pending.extend(items),
                SelectionExpr::Admissible(inner) => pending.push(*inner),
                SelectionExpr::Value(value) => {
                    offers += 1;
                    let ty = output.module.types.get(body.values.get(*value).unwrap().ty).unwrap();
                    if matches!(ty, Type::Future(_)) {
                        assert!(binding, "a selection names a Future: {op:?}");
                        bound += 1;
                    }
                }
                _ => {}
            }
        }
    }
    assert!(offers > bound, "the managed inputs are offered");
    assert_eq!(bound, 1, "the binding of the admitted f names it once");
}

// --- PR-B1 (#784, OIR-2-async-remaining §2.2a): what a binding does with a Future ---------------------------

const PR_B1: &str = include_str!("../../../tests/fixtures/async/high/pr-b1-bindings.oir");

/// Every value a selection of `body` names directly, through unions and admissible restrictions, with whether the
/// selection is admissible.
fn named_values(body: &Body) -> Vec<(bool, ValueId)> {
    let mut named = Vec::new();
    for op in operations(body) {
        let Operation::Ownership(OwnershipOp::ScopeSplit { selection, .. } | OwnershipOp::Split { selection, .. }
            | OwnershipOp::ScopeTransfer { selection, .. }) = op else { continue };
        let admissible = matches!(body.selections.get(*selection).unwrap(), SelectionExpr::Admissible(_));
        let mut pending = vec![*selection];
        while let Some(selection) = pending.pop() {
            match body.selections.get(selection).unwrap() {
                SelectionExpr::Union(items) => pending.extend(items),
                SelectionExpr::Admissible(inner) => pending.push(*inner),
                SelectionExpr::Value(value) => named.push((admissible, *value)),
                _ => {}
            }
        }
    }
    named
}

fn named_futures(module: &Module, body: &Body) -> Vec<(bool, ValueId)> {
    named_values(body).into_iter().filter(|(_, value)| {
        matches!(module.types.get(body.values.get(*value).unwrap().ty).unwrap(), Type::Future(_))
    }).collect()
}

/// The `binding` attribute of every `fut.new` of a body, in block order.
fn creation_bindings(body: &Body) -> Vec<Option<BindingId>> {
    body.blocks.iter().flat_map(|(_, block)| block.insts.iter()).filter_map(|inst| {
        match body.insts.get(*inst).unwrap().op {
            Operation::Async(AsyncOp::FutureNew { binding, .. }) => Some(binding),
            _ => None,
        }
    }).collect()
}

fn pr_b1_output() -> OwnOutput {
    let output = rewrite_async(PR_B1).unwrap();
    verify_own(&output.module).unwrap_or_else(|report| panic!("{report:?}"));
    output
}

/// `let c = Child(n: n).run(); Holder(f: c);`: the binding holds `c` from its creation, so `fut.new` carries it and
/// the constructor's selection names nothing; `c` is borrowed by the Holder and leaves by `cache.rehome`.
#[test]
fn a_bound_created_future_is_not_named_by_a_store() {
    let output = pr_b1_output();
    let (_, bound) = function(&output.module, "app.Maker.bound#1");
    let body = bound.body.as_ref().unwrap();
    let c = body.bindings.iter().find(|(_, binding)| binding.name == "c").unwrap().0;
    assert_eq!(creation_bindings(body), [Some(c)]);
    assert!(named_futures(&output.module, body).is_empty(), "{:?}", named_futures(&output.module, body));
    assert_eq!(rehomes(&output.module, body).len(), 1, "the returned Holder takes c's duty along by its rehome");
}

/// `Holder(f: Child(n: n).run());`: a fresh temporary has no binding, and the store's admissible selection names it,
/// so its duty moves into the constructor's packet (D25.2) and no rehome remains.
#[test]
fn a_fresh_future_is_named_by_its_store() {
    let output = pr_b1_output();
    let (_, fresh) = function(&output.module, "app.Maker.fresh#2");
    let body = fresh.body.as_ref().unwrap();
    assert_eq!(creation_bindings(body), [None]);
    assert_eq!(named_futures(&output.module, body).into_iter().filter(|(admissible, _)| *admissible).count(), 1);
    assert!(rehomes(&output.module, body).is_empty());
}

/// `let g = Maker().make(9);`: an admitted Future (a call result) is bound like any managed call result: an
/// admissible split that names it, then `own.bind`, which moves its entry to the bound journal at run time. The
/// creation of a Future the function only awaits carries no binding.
#[test]
fn an_admitted_future_binding_is_a_split_and_a_bind() {
    let output = pr_b1_output();
    let (_, main) = function(&output.module, "main");
    let body = main.body.as_ref().unwrap();
    let g = body.bindings.iter().find(|(_, binding)| binding.name == "g").unwrap().0;
    let split = body.blocks.iter().find_map(|(_, block)| {
        let TermKind::InvokeOwn { op, normal, .. } = &block.term.kind else { return None };
        let Operation::Ownership(OwnershipOp::ScopeSplit { selection, .. }) = op.as_ref() else { return None };
        let first = *body.blocks.get(normal.block).unwrap().insts.first()?;
        matches!(&body.insts.get(first).unwrap().op,
            Operation::Ownership(OwnershipOp::Bind { binding }) if binding.binding == g).then_some(*selection)
    }).expect("the binding of g follows a split");
    let SelectionExpr::Admissible(inner) = body.selections.get(split).unwrap() else { panic!("admissible") };
    let SelectionExpr::Value(value) = body.selections.get(*inner).unwrap() else { panic!("names g") };
    assert!(matches!(output.module.types.get(body.values.get(*value).unwrap().ty).unwrap(), Type::Future(_)));
    assert!(creation_bindings(body).iter().all(Option::is_none), "{:?}", creation_bindings(body));
}

/// Whether a body's Services schedules end with the dynamic step (D25.3).
fn settles_dynamically(body: &Body) -> bool {
    body.service_schedules.iter().any(|schedule| schedule.duties.contains(&ScheduleDuty::Dynamic))
}

/// PR-B2 (#782), ABI D25.3: an entry packet whose inputs can bring service entries makes the root scope one that
/// may admit them: a synchronous constructor with a Future parameter (a fresh Future stored through it arrives with
/// its entry) and an async method whose receiver stores a Future. An input that reaches no holder does not.
#[test]
fn an_entry_packet_that_can_carry_entries_settles_dynamically() {
    let output = pr_b1_output();
    for (symbol, dynamic) in [("app.Holder.new#0", true), ("app.Holder.get#0", true), ("app.Child.run#0", false)] {
        let (_, function) = function(&output.module, symbol);
        assert_eq!(settles_dynamically(function.body.as_ref().unwrap()), dynamic, "{symbol}");
    }
}

/// PR-B2: a bare Future input of an async body is borrowed (own never offers it to `fut.new`), so its start packet
/// brings no entry and the body's schedule has no dynamic step (golden start-deps: `Adder.plus(f: Future<int>)`).
#[test]
fn a_bare_future_input_of_an_async_body_brings_no_entry() {
    let output = rewrite_async(&input("start-deps")).unwrap();
    let (_, plus) = output.module.functions.iter().find(|(_, function)| function.symbol.contains("plus")).unwrap();
    let body = plus.body.as_ref().unwrap();
    assert!(!settles_dynamically(body));
}

/// PR-B2 (#782): `first() is Future<int> { f; }` returns a Future it borrows from its receiver. No cache duty of
/// its scope reaches it, so no rehome takes it; a carrier entry packet may still have brought its entry, so the
/// exit's selection names it (`SPB_EXIT_SELECTION`). A Future whose duty a call gave the function keeps leaving by
/// `cache.rehome` only.
#[test]
fn a_returned_borrowed_future_is_named_by_its_exit_selection() {
    let output = pr_b1_output();
    let (_, first) = function(&output.module, "app.Box.first#0");
    let body = first.body.as_ref().unwrap();
    let named = named_futures(&output.module, body);
    assert_eq!(named.len(), 1, "{named:?}");
    assert!(!named[0].0, "the exit selection is not admissible");
    assert!(rehomes(&output.module, body).is_empty());
    let (_, make) = function(&output.module, "app.Maker.make#0");
    let make = make.body.as_ref().unwrap();
    assert!(named_futures(&output.module, make).is_empty(), "{:?}", named_futures(&output.module, make));
    assert_eq!(rehomes(&output.module, make).len(), 1);
}

// --- PR-B1 across nested scopes (review of D25, F1 and F3) --------------------------------------------------------

const PR_B1_SCOPES: &str = include_str!("../../../tests/fixtures/async/high/pr-b1-scopes.oir");

fn pr_b1_scopes() -> OwnOutput {
    let output = rewrite_async(PR_B1_SCOPES).unwrap_or_else(|error| panic!("{error}"));
    verify_own(&output.module).unwrap_or_else(|report| panic!("{report:?}"));
    output
}

fn binding_named(body: &Body, name: &str) -> BindingId {
    body.bindings.iter().find(|(_, binding)| binding.name == name).unwrap().0
}

/// The packet that `own.bind` of `binding` takes, and whether `own.empty` made it.
fn bound_packet(body: &Body, binding: BindingId) -> (ValueId, bool) {
    let packet = operations(body).into_iter().find_map(|op| match op {
        Operation::Ownership(OwnershipOp::Bind { binding: elaborated }) if elaborated.binding == binding => {
            Some(elaborated.resource.packet)
        }
        _ => None,
    }).expect("an own.bind of the binding");
    let empty = body.insts.iter().any(|(_, instruction)| instruction.results.contains(&packet)
        && matches!(instruction.op, Operation::Ownership(OwnershipOp::Empty {})));
    (packet, empty)
}

/// The Futures that admissible selections of `body` name, each once (a split per open scope repeats a selection).
fn admissible_futures(module: &Module, body: &Body) -> Vec<ValueId> {
    let named: BTreeSet<_> = named_futures(module, body).into_iter().filter(|(admissible, _)| *admissible)
        .map(|(_, value)| value).collect();
    named.into_iter().collect()
}

/// F1: `let f = Child(n: 4).run(); let e = f; if true { let r = Res(); let g = f; } Holder(f: f)`. High passes `f`
/// into the branch through a block parameter and binds that parameter as `g` in a retained scope; `f` is bound in
/// its owner before, inside and after the branch. `fut.new` carries `f`'s binding (the first one in the owner), the
/// binding of `g` emits nothing (the duty never left its owner), and the store after the branch names no Future:
/// the Holder borrows `f`, whose token stays scheduled.
#[test]
fn an_alias_in_a_nested_scope_keeps_a_created_future_bound() {
    let output = pr_b1_scopes();
    let (_, alias) = function(&output.module, "app.Cases.alias#0");
    let body = alias.body.as_ref().unwrap();
    assert_eq!(creation_bindings(body), [Some(binding_named(body, "f"))]);
    assert!(admissible_futures(&output.module, body).is_empty(), "{:?}", named_futures(&output.module, body));
}

/// F1, admitted: `let g = Maker().make(9); if true { let r = Res(); let k = g; Holder(f: k); } Holder(f: g)`. Only
/// `g`'s own binding is a split that names it. `k` aliases a Future already bound in the enclosing scope, so its
/// `own.bind` takes an empty packet (the entry stays in the bound journal of `g`'s scope), and neither store names a
/// Future.
#[test]
fn an_alias_of_a_bound_admitted_future_takes_nothing() {
    let output = pr_b1_scopes();
    let (_, retained) = function(&output.module, "app.Cases.retained#1");
    let body = retained.body.as_ref().unwrap();
    assert_eq!(admissible_futures(&output.module, body).len(), 1, "{:?}", named_futures(&output.module, body));
    assert!(!bound_packet(body, binding_named(body, "g")).1, "g's binding takes its split's packet");
    assert!(bound_packet(body, binding_named(body, "k")).1, "k's binding takes nothing");
}

/// F3 (PR-B1 item 4 with several open scopes): `if true { let r = Res(); let g = Maker().make(7); Holder(f: g); }`.
/// The binding of an admitted Future in a nested retained scope is an admissible split that names it and an
/// `own.bind` of the split's packet, as with one open scope; the Full verifier accepts it (no B51 at the store).
#[test]
fn an_admitted_future_first_bound_in_a_nested_scope_is_a_split_and_a_bind() {
    let output = pr_b1_scopes();
    let (_, nested) = function(&output.module, "app.Cases.nested#2");
    let body = nested.body.as_ref().unwrap();
    assert_eq!(admissible_futures(&output.module, body).len(), 1, "{:?}", named_futures(&output.module, body));
    assert!(!bound_packet(body, binding_named(body, "g")).1, "g's binding takes its split's packet");
}

/// `let f = Child(n: 1).run(); Holder(f: Chain().after(f))`: the fresh Future depends on the bound `f`, and is still a
/// fresh temporary: its store names it (D25.2), and only it.
#[test]
fn a_fresh_future_made_from_a_bound_one_is_named_by_its_store() {
    let output = pr_b1_scopes();
    let (_, chained) = function(&output.module, "app.Cases.chained#3");
    let body = chained.body.as_ref().unwrap();
    assert_eq!(creation_bindings(body), [Some(binding_named(body, "f")), None]);
    let fresh = body.insts.iter().find_map(|(_, instruction)| match instruction.op {
        Operation::Async(AsyncOp::FutureNew { binding: None, .. }) => instruction.results.first().copied(),
        _ => None,
    }).unwrap();
    assert_eq!(admissible_futures(&output.module, body), [fresh]);
}

/// The instruction of the one `fut.new` of `body` that carries `binding` (`Some(true)`), carries none
/// (`Some(false)`), or either (`None`).
fn creation(body: &Body, bound: Option<bool>) -> InstId {
    let found: Vec<_> = body.insts.iter().filter_map(|(inst, instruction)| match instruction.op {
        Operation::Async(AsyncOp::FutureNew { binding, .. }) if bound.is_none_or(|bound| binding.is_some() == bound) => {
            Some(inst)
        }
        _ => None,
    }).collect();
    let [inst] = found.as_slice() else { panic!("one fut.new: {found:?}") };
    *inst
}

/// Review of D25, 4b: the Full verifier's partition of an admissible split that reaches a bound Future through an
/// aggregate (`Holder(f: f)` offered to `take`) leaves the Future's holder graph in the rest, as the runtime's
/// admission filter does; no partition of `alias` moves it into a selected part.
#[test]
fn a_bound_future_reached_through_an_aggregate_stays_in_the_rest() {
    let output = rewrite_async(PR_B1_SCOPES).unwrap();
    let (id, alias) = function(&output.module, "app.Cases.alias#0");
    let body = alias.body.as_ref().unwrap();
    let created = creation(body, Some(true));
    crate::verify::take_partitions();
    verify_own(&output.module).unwrap();
    let records: Vec<_> = crate::verify::take_partitions().into_iter().filter(|record| record.function == id).collect();
    assert!(records.iter().all(|record| !record.selected.contains(&created)), "{records:?}");
    let holder = |value: &ValueId| matches!(output.module.types.get(body.values.get(*value).unwrap().ty),
        Ok(Type::Object { decl, .. }) if output.module.declarations.get(*decl).unwrap().name == "app.Holder");
    let offers: Vec<_> = records.iter().filter(|record| {
        let Ok(SelectionExpr::Admissible(inner)) = body.selections.get(record.selection) else { return false };
        matches!(body.selections.get(*inner), Ok(SelectionExpr::Union(items)) if items.iter().any(|item| {
            matches!(body.selections.get(*item), Ok(SelectionExpr::Value(value)) if holder(value))
        }))
    }).collect();
    assert!(!offers.is_empty(), "the offer of the Holder is partitioned");
    assert!(offers.iter().all(|record| record.rest.contains(&created)), "{offers:?}");
}

const STEALS: &str = include_str!("../../../tests/fixtures/verify/invalid/B9-own-split-steals-shared-cache.oir");

/// Review of D25, F4: no partition takes the holder graph of a bound Future it names directly, a plain one included
/// (`scope.split %s, selection(%f)` with `fut.new … binding b0`). The structural rule `b9-bound-future-named`
/// refuses the selection; the Full model keeps the duty with its owner as well.
#[test]
fn no_partition_takes_a_bound_future_it_names() {
    let module = crate::text::parse(STEALS).unwrap();
    let (id, main) = function(&module, "main");
    let body = main.body.as_ref().unwrap();
    let created = creation(body, Some(true));
    crate::verify::take_partitions();
    let report = verify_own(&module).unwrap_err();
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == RuleId::B9BoundFutureNamed), "{report:?}");
    let named: Vec<_> = crate::verify::take_partitions().into_iter().filter(|record| record.function == id
        && matches!(body.selections.get(record.selection), Ok(SelectionExpr::Value(value))
            if matches!(module.types.get(body.values.get(*value).unwrap().ty), Ok(Type::Future(_)))))
        .collect();
    assert!(!named.is_empty(), "the split that names f is partitioned");
    assert!(named.iter().all(|record| record.rest.contains(&created) && !record.selected.contains(&created)),
        "{named:?}");
}
