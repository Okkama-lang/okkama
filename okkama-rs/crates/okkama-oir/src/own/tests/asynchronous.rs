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
    "async-never-awaited",
    "async-plain-await",
    "async-repeated-await",
    "async-void",
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
