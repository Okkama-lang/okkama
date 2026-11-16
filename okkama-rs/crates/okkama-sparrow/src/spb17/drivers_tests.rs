//! #601 Q1, async ABI D22: `EntryAbi::AsyncExternal` and its loader rule `SPB_EXTERNAL_ASYNC`, and the trusted
//! catalogue of the std async primitives (`drivers`). The positive program is the plain await of `async_tests`
//! with its Future created from a driver; each negative is valid up to the rule it names.
use super::*;
use super::async_tests::{arguments, find, plain};
use super::drivers::{EVENT, EVENT_STORAGE, EXTERNAL_ASYNC};

use Argument as A;
use Opcode as O;

/// A trusted catalogue of the given entries, as the runtime's.
struct Catalogue(Vec<Function>);
impl ExternalContracts for Catalogue {
    fn entry(&self, name: &str) -> Option<&Function> {
        self.0.iter().find(|entry| {
            entry.proofs.entry.as_ref().and_then(|entry| entry.external.as_deref()) == Some(name)
        })
    }
}

/// A driver declaration of `name` taking one int and producing one, as the catalogue's.
fn driver(name: &str) -> Function {
    Function {
        symbol: name.into(), entry_abi: EntryAbi::AsyncExternal, registers: vec![Type::Int], services: vec![],
        params: vec![Location::Register(0)], results: vec![Type::Int], blocks: vec![], code: vec![],
        proofs: FunctionProofs {
            entry: Some(EntryContract { external: Some(name.into()), failures: 2, ..EntryContract::default() }),
            ..FunctionProofs::default()
        },
    }
}

/// `async main() { let x = await work(7); }` with `work` the driver `q1.work` (function 2) instead of an async body.
fn driven() -> Program {
    let mut p = plain();
    p.functions.push(driver("q1.work"));
    let main = &mut p.functions[1];
    let new = find(main, O::FutureNew);
    arguments(main, new)[0] = A::Entry(2);
    p
}

fn trusted() -> Catalogue {
    Catalogue(vec![driver("q1.work")])
}

fn rejects(p: &Program, catalogue: &Catalogue, code: &'static str, message: &str) {
    let failure = verify_load(p, catalogue).unwrap_err();
    assert_eq!(failure.rule, code, "{failure}");
    assert!(failure.message.contains(message), "{failure}");
}

#[test]
fn fut_new_creates_the_future_of_a_catalogue_driver() {
    verify_load(&driven(), &trusted()).unwrap();
}

#[test]
fn an_async_external_round_trips_the_codec() {
    let p = driven();
    assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
}

#[test]
fn an_async_external_has_no_body() {
    let mut p = driven();
    p.functions[2].code.push(Instruction::Ret(vec![]));
    p.functions[2].blocks.push(Block { pc: 0, params: vec![Location::Register(0)] });
    rejects(&p, &trusted(), EXTERNAL_ASYNC, "bodyless external");
}

#[test]
fn an_async_external_is_an_external() {
    let mut p = driven();
    p.functions[2].proofs.entry = None;
    rejects(&p, &trusted(), EXTERNAL_ASYNC, "bodyless external");
}

#[test]
fn an_async_external_takes_its_registers_and_no_service() {
    let mut p = driven();
    p.functions[2].services.push(ServiceKind::Deps);
    p.functions[2].params.push(Location::Service(0));
    rejects(&p, &Catalogue(vec![p.functions[2].clone()]), EXTERNAL_ASYNC, "takes its registers in order");
    let mut p = driven();
    p.functions[2].results.push(Type::Int);
    rejects(&p, &Catalogue(vec![p.functions[2].clone()]), EXTERNAL_ASYNC, "at most one result");
}

#[test]
fn an_async_external_resolves_to_a_catalogue_driver() {
    // No trusted entry: the unresolved-provider adapter never makes a driver.
    rejects(&driven(), &Catalogue(vec![]), EXTERNAL_ASYNC, "resolves to a catalogue driver");
    // The catalogue's entry of the name is synchronous.
    let mut sync = driver("q1.work");
    sync.entry_abi = EntryAbi::Observation;
    rejects(&driven(), &Catalogue(vec![sync]), EXTERNAL_ASYNC, "resolves to a catalogue driver");
}

#[test]
fn a_catalogue_driver_is_declared_async() {
    // A declaration of the driver's name as a synchronous entry, which nothing calls.
    let mut p = plain();
    let mut sync = driver("q1.work");
    sync.entry_abi = EntryAbi::Observation;
    p.functions.push(sync);
    rejects(&p, &trusted(), EXTERNAL_ASYNC, "a driver is async");
}

#[test]
fn fut_new_passes_a_driver_its_argument_types() {
    let mut p = driven();
    p.functions[2].registers = vec![Type::Bool];
    let catalogue = Catalogue(vec![p.functions[2].clone()]);
    rejects(&p, &catalogue, EXTERNAL_ASYNC, "argument of another type");
}

#[test]
fn fut_new_of_a_driver_produces_its_result() {
    let mut p = driven();
    p.functions[2].results = vec![Type::Bool];
    let catalogue = Catalogue(vec![p.functions[2].clone()]);
    rejects(&p, &catalogue, verify::FUTURE_TYPES, "differs from the async body's result");
}

#[test]
fn only_fut_new_names_a_driver() {
    // `invoke.call_own` of @main's await replaced by a direct call of the driver: no synchronous ABI admits it.
    let mut p = driven();
    let main = &mut p.functions[1];
    let new = find(main, O::FutureNew);
    let Instruction::Champ { op, .. } = &mut main.code[new] else { unreachable!() };
    *op = O::InvokeCallOwn;
    assert!(verify_load(&p, &trusted()).is_err());
}

// --- The catalogue of the std async primitives -----------------------------------------------------------------

fn object(name: &str, storage: Vec<Type>, fields: &[&str]) -> Object {
    Object {
        name: name.into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle: false, role: None,
        implements: vec![], any_box: false, enum_names: vec![],
        fields: fields.iter().enumerate().map(|(index, name)| Field {
            name: (*name).into(), storage: index as u32, once: false,
        }).collect(),
        layout: ObjectRuntimeLayout { allocation_words: 1 + storage.len().max(1) as u32, identity_offset: None,
            has_destroy: false, may_reach_resource: false, may_async_destroy: false,
            trace_layout: vec![], selection_layout: vec![], storage_fields: storage },
    }
}

fn entry<'c>(contracts: &'c p1::P1Contracts, name: &str) -> Option<&'c Function> {
    ExternalContracts::entry(contracts, name)
}

#[test]
fn the_event_cell_admits_its_entries_and_driver() {
    let objects = vec![object(EVENT, EVENT_STORAGE.to_vec(), &[])];
    let contracts = p1::P1Contracts::new(&objects).unwrap();
    let wait = entry(&contracts, "okkama.event.Event.wait").unwrap();
    assert_eq!((wait.entry_abi.clone(), wait.registers.clone(), wait.results.clone()),
        (EntryAbi::AsyncExternal, vec![Type::Object(0)], vec![Type::Object(0)]));
    assert!(wait.services.is_empty());
    // A synchronous member takes the deps of the cell, a reference with deps; the constructors own their result.
    let set = entry(&contracts, "okkama.event.Event.set").unwrap();
    assert_eq!((set.entry_abi.clone(), set.services.clone()), (EntryAbi::Observation, vec![ServiceKind::Deps]));
    assert_eq!(entry(&contracts, "okkama.event.Event.new").unwrap().entry_abi, EntryAbi::Owned);
    assert_eq!(contracts.related("okkama.event.Event.new"), Some(&[0][..]));
    assert_eq!(contracts.related("okkama.event.Event.wait"), Some(&[][..]));
}

#[test]
fn an_event_with_another_layout_has_no_entry() {
    for storage in [vec![Type::Bool], vec![Type::Bool, Type::Bool, Type::Str]] {
        let contracts = p1::P1Contracts::new(&[object(EVENT, storage, &[])]).unwrap();
        assert!(entry(&contracts, "okkama.event.Event.wait").is_none());
    }
    let mut resource = object(EVENT, EVENT_STORAGE.to_vec(), &[]);
    resource.layout.has_destroy = true;
    resource.layout.may_reach_resource = true;
    let contracts = p1::P1Contracts::new(&[resource]).unwrap();
    assert!(entry(&contracts, "okkama.event.Event.set").is_none());
    // A native handle of the same name is no cell.
    let mut handle = object(EVENT, EVENT_STORAGE.to_vec(), &[]);
    handle.handle = true;
    assert!(entry(&p1::P1Contracts::new(&[handle]).unwrap(), "okkama.event.Event.set").is_none());
}

#[test]
fn a_counting_primitive_reads_its_amount() {
    let objects = vec![object("async.semaphore.Semaphore", vec![Type::Int], &["amount"]),
        object("async.semaphore.Permit", vec![], &[])];
    let contracts = p1::P1Contracts::new(&objects).unwrap();
    let permit = entry(&contracts, "okkama.semaphore.Semaphore.permit").unwrap();
    assert_eq!((permit.entry_abi.clone(), permit.results.clone()), (EntryAbi::AsyncExternal, vec![Type::Object(1)]));
    assert_eq!(contracts.related("okkama.semaphore.Semaphore.permit"), Some(&[1][..]));
    // Plain objects carry no deps.
    assert!(entry(&contracts, "okkama.semaphore.Semaphore.permits").unwrap().services.is_empty());
    let renamed = vec![object("async.semaphore.Semaphore", vec![Type::Int], &["count"]),
        object("async.semaphore.Permit", vec![], &[])];
    assert!(entry(&p1::P1Contracts::new(&renamed).unwrap(), "okkama.semaphore.Semaphore.permit").is_none());
}

#[test]
fn a_role_slot_receiver_takes_its_deps() {
    // LockGuard does Closing: its close is a slot of the role, so its receiver is a witness with deps.
    let mut closing = object("io.stream.Closing", vec![], &[]);
    closing.role = Some(RoleDecl { slots: vec![RoleSlot { name: "close".into(), entry_abi: EntryAbi::Observation,
        params: vec![], results: vec![] }] });
    let mut guard = object("async.lock.LockGuard", vec![], &[]);
    guard.implements = vec![RoleImpl { role: 0, slots: vec![0] }];
    let contracts = p1::P1Contracts::new(&[closing, guard]).unwrap();
    assert_eq!(entry(&contracts, "okkama.lock.LockGuard.close").unwrap().services, vec![ServiceKind::Deps]);
    assert!(entry(&contracts, "okkama.lock.LockGuard.closed").unwrap().services.is_empty());
}
