//! #601 Q1, D22: the std async primitives in the High producer and through the build pipeline to SPB 17.
use super::super::case_tests::case_high;
use super::super::pipeline::compile_high_with;
use super::super::tests::{lower_std, verify_high};
use crate::oir2 as ir;
use okkama_sparrow::spb17 as s;

/// Every catalogue member that a program can call directly (the token's members need its role, slice F1).
const PROGRAM: &str = concat!(
    "module app; import \"async/event\" take Event; import \"async/lock\" take Lock, LockGuard; ",
    "import \"async/semaphore\" take Semaphore, Permit; import \"async/barrier\" take Barrier, BarrierState; ",
    "import \"async/cancellation\" take Cancellation; ",
    "async main() is int { let e = Event(); let r = Event(ready: true); e.set(); let w = await r.wait(); ",
    "let lock = Lock(); let guard = await lock.guard(); let held = lock.locked(); guard.close(); ",
    "let shut = guard.closed(); let semaphore = Semaphore(permits: 1); let permit = await semaphore.permit(); ",
    "permit.close(); let left = semaphore.permits() + (if permit.closed() { 1; } else { 0; }); ",
    "let barrier = Barrier(parties: 1); let arrival = await barrier.wait(); let state = barrier.state(); ",
    "barrier.abort(); barrier.reset(); let c = Cancellation(); c.cancel(); ",
    "if e.ready() && w.ready() && held && shut && !state.broken() { arrival + left + state.parties() ",
    "+ state.waiters() + state.generation(); } else { 0; } }",
);

const DRIVERS: [&str; 3] = ["okkama.event.Event.wait", "okkama.lock.Lock.guard", "okkama.barrier.Barrier.wait"];

fn external<'m>(module: &'m ir::Module, name: &str) -> &'m ir::Function {
    module.functions.iter().map(|(_, function)| function).find(|function| function.attrs.iter().any(|attr| {
        matches!(attr, ir::FunctionAttr::Intrinsic { name: found, .. } if found == name)
    })).unwrap_or_else(|| panic!("{name} is declared"))
}

#[test]
fn q1_members_are_bodyless_catalogue_externals() {
    let module = lower_std(PROGRAM).unwrap_or_else(|error| panic!("{error}"));
    verify_high(&module, PROGRAM);
    for name in DRIVERS.iter().chain(&["okkama.semaphore.Semaphore.permit"]) {
        // An async member is an async external with the High creation envelope and the catalogue body (D22).
        let function = external(&module, name);
        assert!(function.body.is_none() && function.attrs.contains(&ir::FunctionAttr::Async), "{name}");
        assert_eq!(function.summary, None, "{name}");
        let body = module.summaries.get(function.async_body_summary.unwrap()).unwrap();
        assert!(body.contract.suspends && body.contract.failures.contains(&ir::FailureKind::Cancelled), "{name}");
        let creation = module.summaries.get(function.async_creation_summary.unwrap()).unwrap();
        assert!(!creation.contract.suspends, "{name}: creating a driver's Future runs nothing");
    }
    for name in ["okkama.event.Event.new", "okkama.event.Event.new_ready", "okkama.event.Event.set",
        "okkama.lock.LockGuard.close", "okkama.barrier.BarrierState.generation",
        "okkama.cancellation.Cancellation.cancel"]
    {
        let function = external(&module, name);
        assert!(function.body.is_none() && !function.attrs.contains(&ir::FunctionAttr::Async), "{name}");
        let summary = module.summaries.get(function.summary.unwrap()).unwrap();
        assert!(!summary.contract.suspends, "{name}");
    }
    // The constructors name the object they create; Barrier.wait the error it throws.
    let objects = |name| match external(&module, name).attrs.iter().find_map(|attr| match attr {
        ir::FunctionAttr::Intrinsic { objects, .. } => Some(objects.clone()),
        _ => None,
    }) {
        Some(objects) => objects.iter().map(|&id| module.declarations.get(id).unwrap().name.clone()).collect(),
        None => vec![],
    };
    assert_eq!(objects("okkama.event.Event.new"), ["async.event.Event"]);
    assert_eq!(objects("okkama.lock.Lock.guard"), ["async.lock.LockGuard"]);
    assert_eq!(objects("okkama.barrier.Barrier.wait"), ["async.barrier.BrokenBarrier"]);
    // The Event cell is builtin: its runtime layout replaces its source field.
    let event = module.declarations.iter().find(|(_, declaration)| declaration.name == "async.event.Event").unwrap();
    assert!(matches!(&event.1.kind, ir::DeclarationKind::Object { builtin: true, fields, .. } if fields.is_empty()));
}

#[test]
fn q1_members_build_to_spb17_drivers() {
    // The whole build pipeline (its structural gates: the full verifier needs more steps than its default limit
    // for this one large body), then the SPB 17 load gate against the trusted catalogue.
    let high = lower_std(PROGRAM).unwrap_or_else(|error| panic!("{error}"));
    let bytes = compile_high_with(high, ir::verify::VerifyMode::Structural).unwrap_or_else(|error| panic!("{error}"));
    let program = s::decode(&bytes).unwrap();
    let contracts = s::p1::P1Contracts::new(&program.objects).unwrap();
    s::verify_load(&program, &contracts).unwrap_or_else(|error| panic!("{error}"));
    for name in DRIVERS {
        let (index, function) = program.functions.iter().enumerate().find(|(_, function)| {
            function.proofs.entry.as_ref().and_then(|entry| entry.external.as_deref()) == Some(name)
        }).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(function.entry_abi, s::EntryAbi::AsyncExternal, "{name}");
        let created = program.functions.iter().flat_map(|function| &function.code).any(|instruction| matches!(
            instruction, s::Instruction::Champ { op: s::Opcode::FutureNew, arguments, .. }
                if arguments.contains(&s::Argument::Entry(index as u32))));
        assert!(created, "{name}: fut.new names the driver");
    }
    let event = program.objects.iter().find(|object| object.name == s::drivers::EVENT).unwrap();
    assert_eq!(event.layout.storage_fields, s::drivers::EVENT_STORAGE);
}

#[test]
fn q1_wake_event_set_builds_under_both_gates() {
    for mode in [ir::verify::VerifyMode::Structural, ir::verify::VerifyMode::Full] {
        let high = case_high("L08.wake.event-set").unwrap_or_else(|error| panic!("{error}"));
        let bytes = compile_high_with(high, mode).unwrap_or_else(|error| panic!("{error}"));
        let program = s::decode(&bytes).unwrap();
        let contracts = s::p1::P1Contracts::new(&program.objects).unwrap();
        s::verify_load(&program, &contracts).unwrap_or_else(|error| panic!("{error}"));
    }
}

#[test]
fn q1_a_catalogue_signature_is_exact() {
    // The catalogue entry of Event.set takes the Event cell; a declaration over another type is refused.
    let module = lower_std(PROGRAM).unwrap();
    let int = module.types.iter().find_map(|(id, ty)| (*ty == ir::Type::Int).then_some(id)).unwrap();
    let wrong = ir::Signature { params: vec![int], results: vec![], ownership_abi: ir::OwnershipAbi::Unelaborated };
    let error = super::contract(&module, "okkama.event.Event.set", &wrong).unwrap_err();
    assert!(error.message.contains("invalid Q1 signature"), "{}", error.message);
    assert!(super::contract(&module, "okkama.event.Event.unknown", &wrong).unwrap().is_none());
}

#[test]
fn q1_deps_follow_own_for_role_slots() {
    // LockGuard does Closing: used through the role, its close and closed are witness slots whose receiver
    // takes deps; the Event cell always does, and plain objects never.
    let source = concat!(
        "module app; import \"async/lock\" take Lock, LockGuard; import \"io/stream\" take Closing; ",
        "import \"async/event\" take Event; ",
        "async main() is int { let guard = await Lock().guard(); let c is Closing = guard; c.close(); ",
        "Event().set(); if Lock().locked() { 1; } else { 0; } }",
    );
    let module = lower_std(source).unwrap_or_else(|error| panic!("{error}"));
    let deps = |name| {
        let (id, function) = module.functions.iter().find(|(_, function)| function.attrs.iter().any(|attr| {
            matches!(attr, ir::FunctionAttr::Intrinsic { name: found, .. } if found == name)
        })).unwrap();
        super::deps(&module, id, &function.signature.params)
    };
    assert_eq!(deps("okkama.lock.LockGuard.close"), 1);
    assert_eq!(deps("okkama.event.Event.set"), 1);
    assert_eq!(deps("okkama.lock.Lock.locked"), 0);
}
