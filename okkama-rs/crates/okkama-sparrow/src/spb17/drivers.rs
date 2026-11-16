//! #601 Q1, async ABI D22: the trusted catalogue of the std async primitives (Event, Lock, Semaphore, Barrier and
//! the cancellation token), built from the program's object declarations, never from its functions.
//!
//! - A synchronous member is an ordinary external entry (`Observation` or `Owned`), as the P1 Atomic entries.
//! - An `async` member is an `AsyncExternal` entry: a driver. It takes its language arguments as words, has no
//!   service parameters (its creation packet goes to `fut.new`, its deps are proofs of the caller) and at most one
//!   result. Only `fut.new` names it (`SPB_EXTERNAL_ASYNC`); the runtime allocates a driver frame for it.
//! - Each entry also names the declarations the runtime creates or throws (Tomato's intrinsic objects).
//! - Every payload is plain. An entry exists only when every declaration it names has its runtime layout: the
//!   Event cell `[initial, set, waiters]` (its source field `initial`, the waiters a `RuntimeCell`), Semaphore
//!   and Barrier with their `amount`, the other objects without
//!   storage, `BrokenBarrier` a one-case Error enum, `Cancelling` a role.
use super::*;

/// An `AsyncExternal` entry is a bodyless external whose name the trusted catalogue resolves to a driver, and only
/// `fut.new` names it, with its arguments typed as its registers (async ABI D22, Champ draft 3 §15).
pub const EXTERNAL_ASYNC: &str = "SPB_EXTERNAL_ASYNC";

/// The Event cell's declaration and its runtime layout: `initial`, the set flag and the waiters (Tomato
/// `primitives`); the waiters word is a traced runtime list.
pub const EVENT: &str = "async.event.Event";
pub const EVENT_STORAGE: [Type; 3] = [Type::Bool, Type::Bool, Type::RuntimeCell];

const LOCK: &str = "async.lock.Lock";
const GUARD: &str = "async.lock.LockGuard";
const SEMAPHORE: &str = "async.semaphore.Semaphore";
const PERMIT: &str = "async.semaphore.Permit";
const BARRIER: &str = "async.barrier.Barrier";
const BARRIER_STATE: &str = "async.barrier.BarrierState";
const BROKEN_BARRIER: &str = "async.barrier.BrokenBarrier";
const CANCELLATION: &str = "async.cancellation.Cancellation";
const TOKEN: &str = "async.cancellation.CancellationToken";
const CANCELLING: &str = "async.cancellation.Cancelling";

/// Failure mask bits of an entry (`EntryContract::failures`): a thrown error, a program failure. A cancellation
/// has no bit.
const THROWN: u32 = 1;
const PROGRAM: u32 = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Abi {
    Observation,
    Owned,
    Driver,
}

/// (name, parameters, results, ABI, failures, related declarations). Types are `bool`, `int` or a declaration.
type Row = (&'static str, &'static [&'static str], &'static [&'static str], Abi, u32, &'static [&'static str]);

const CATALOGUE: &[Row] = &[
    ("okkama.event.Event.new", &[], &[EVENT], Abi::Owned, 0, &[EVENT]),
    ("okkama.event.Event.new_ready", &["bool"], &[EVENT], Abi::Owned, 0, &[EVENT]),
    ("okkama.event.Event.set", &[EVENT], &[], Abi::Observation, PROGRAM, &[]),
    ("okkama.event.Event.ready", &[EVENT], &["bool"], Abi::Observation, PROGRAM, &[]),
    ("okkama.event.Event.wait", &[EVENT], &[EVENT], Abi::Driver, PROGRAM, &[]),
    ("okkama.lock.Lock.new", &[], &[LOCK], Abi::Owned, 0, &[LOCK]),
    ("okkama.lock.Lock.locked", &[LOCK], &["bool"], Abi::Observation, PROGRAM, &[]),
    ("okkama.lock.Lock.guard", &[LOCK], &[GUARD], Abi::Driver, PROGRAM, &[GUARD]),
    ("okkama.lock.LockGuard.closed", &[GUARD], &["bool"], Abi::Observation, PROGRAM, &[]),
    ("okkama.lock.LockGuard.close", &[GUARD], &[], Abi::Observation, PROGRAM, &[]),
    ("okkama.semaphore.Semaphore.permits", &[SEMAPHORE], &["int"], Abi::Observation, PROGRAM, &[]),
    ("okkama.semaphore.Semaphore.permit", &[SEMAPHORE], &[PERMIT], Abi::Driver, PROGRAM, &[PERMIT]),
    ("okkama.semaphore.Permit.closed", &[PERMIT], &["bool"], Abi::Observation, PROGRAM, &[]),
    ("okkama.semaphore.Permit.close", &[PERMIT], &[], Abi::Observation, PROGRAM, &[]),
    ("okkama.barrier.Barrier.wait", &[BARRIER], &["int"], Abi::Driver, THROWN | PROGRAM, &[BROKEN_BARRIER]),
    ("okkama.barrier.Barrier.state", &[BARRIER], &[BARRIER_STATE], Abi::Owned, PROGRAM, &[BARRIER_STATE]),
    ("okkama.barrier.Barrier.abort", &[BARRIER], &[], Abi::Observation, PROGRAM, &[]),
    ("okkama.barrier.Barrier.reset", &[BARRIER], &[], Abi::Observation, PROGRAM, &[]),
    ("okkama.barrier.BarrierState.parties", &[BARRIER_STATE], &["int"], Abi::Observation, PROGRAM, &[]),
    ("okkama.barrier.BarrierState.waiters", &[BARRIER_STATE], &["int"], Abi::Observation, PROGRAM, &[]),
    ("okkama.barrier.BarrierState.generation", &[BARRIER_STATE], &["int"], Abi::Observation, PROGRAM, &[]),
    ("okkama.barrier.BarrierState.broken", &[BARRIER_STATE], &["bool"], Abi::Observation, PROGRAM, &[]),
    ("okkama.cancellation.Cancellation.new", &[], &[CANCELLATION], Abi::Owned, 0, &[CANCELLATION]),
    ("okkama.cancellation.Cancellation.cancelling", &[CANCELLATION], &[CANCELLING], Abi::Owned, PROGRAM, &[TOKEN]),
    ("okkama.cancellation.Cancellation.cancel", &[CANCELLATION], &[], Abi::Observation, PROGRAM, &[]),
    ("okkama.cancellation.CancellationToken.new", &[], &[TOKEN], Abi::Owned, 0, &[TOKEN]),
    ("okkama.cancellation.CancellationToken.requested", &[TOKEN], &["bool"], Abi::Observation, PROGRAM, &[]),
    ("okkama.cancellation.CancellationToken.check", &[TOKEN], &[], Abi::Observation, PROGRAM, &[]),
    ("okkama.cancellation.CancellationToken.wait", &[TOKEN], &[CANCELLING], Abi::Driver, PROGRAM, &[]),
];

/// A declaration of the catalogue with its runtime layout, or None.
fn declaration(objects: &[Object], name: &str) -> Option<u32> {
    let id = objects.iter().position(|object| object.name == name)?;
    let object = &objects[id];
    let layout = &object.layout;
    let plain = !layout.has_destroy && !layout.may_reach_resource && !layout.may_async_destroy
        && !object.handle && !object.any_box
        && object.enum_cases == 0 && object.variant_cases.is_empty();
    let valid = match name {
        CANCELLING => object.role.is_some(),
        // Its source field `initial` is storage 0, which an equality reads as Tomato's does (#601 Q1 audit A1).
        EVENT => plain && object.role.is_none() && layout.storage_fields == EVENT_STORAGE
            && object.error_cases.is_empty() && matches!(object.fields.as_slice(),
                [field] if field.name == "initial" && field.storage == 0 && !field.once),
        SEMAPHORE | BARRIER => plain && object.role.is_none() && object.error_cases.is_empty()
            && layout.storage_fields.first() == Some(&Type::Int)
            && object.fields.first().is_some_and(|field| field.name == "amount" && field.storage == 0 && !field.once),
        BROKEN_BARRIER => plain && object.error_cases == ["BrokenBarrier.broken"] && object.fields.is_empty()
            && layout.storage_fields == [Type::Int],
        _ => plain && object.role.is_none() && object.error_cases.is_empty() && object.fields.is_empty()
            && layout.storage_fields.is_empty(),
    };
    valid.then_some(id as u32)
}

fn ty(objects: &[Object], name: &str) -> Option<Type> {
    match name {
        "bool" => Some(Type::Bool),
        "int" => Some(Type::Int),
        CANCELLING => declaration(objects, name).map(Type::Role),
        _ => declaration(objects, name).map(Type::Object),
    }
}

/// The trusted catalogue entries the program's declarations admit, each with the declarations its runtime
/// creates or throws.
pub(super) fn entries(objects: &[Object]) -> Vec<(Function, Vec<u32>)> {
    let mut entries = Vec::new();
    for &(name, params, results, abi, failures, related) in CATALOGUE {
        let registers = params.iter().map(|name| ty(objects, name)).collect::<Option<Vec<_>>>();
        let results = results.iter().map(|name| ty(objects, name)).collect::<Option<Vec<_>>>();
        let related = related.iter().map(|name| declaration(objects, name)).collect::<Option<Vec<_>>>();
        let (Some(registers), Some(results), Some(related)) = (registers, results, related) else {
            continue;
        };
        let deps = deps(objects, &registers);
        let function = match abi {
            Abi::Observation => entry(name, registers, deps, results, false, failures),
            Abi::Owned => entry(name, registers, deps, results, true, failures),
            Abi::Driver => driver(name, registers, results, failures),
        };
        entries.push((function, related));
    }
    entries
}

/// The deps proofs of an entry, as the producer's elaboration gives them: one for each Event cell parameter (a
/// builtin capability: a reference with deps). The other declarations of the catalogue are plain values without
/// references. A member that is a role slot of its object (LockGuard's `close` for `Closing`) is never named by a
/// witness table: the slot reaches it through an adapter of the slot's ABI (ruling PR-D, #773), so its receiver is
/// an ordinary plain argument.
///
/// The producer gives its declarations the same deps (`okkama-compiler`, `lowering/oir2/drivers.rs`, `deps`), as own
/// elaborates external signatures: change both together.
fn deps(objects: &[Object], registers: &[Type]) -> usize {
    let event = declaration(objects, EVENT);
    registers.iter().filter(|&&ty| Some(ty) == event.map(Type::Object)).count()
}

/// A synchronous entry: its registers, then its deps proofs, then the input packet of an owning entry
/// (`p1::entry`, with the deps of [`deps`]).
fn entry(name: &str, registers: Vec<Type>, deps: usize, results: Vec<Type>, owned: bool, failures: u32) -> Function {
    let mut params = (0..registers.len() as u32).map(Location::Register).collect::<Vec<_>>();
    let mut services = vec![ServiceKind::Deps; deps];
    params.extend((0..deps as u32).map(Location::Service));
    if owned {
        params.push(Location::Service(services.len() as u32));
        services.push(ServiceKind::Packet);
    }
    Function {
        symbol: name.into(), entry_abi: if owned { EntryAbi::Owned } else { EntryAbi::Observation },
        registers, services, params, results, blocks: vec![], code: vec![],
        proofs: FunctionProofs {
            entry: Some(EntryContract { external: Some(name.into()), failures, ..EntryContract::default() }),
            ..FunctionProofs::default()
        },
    }
}

/// A driver's trusted entry: its arguments are its registers, in order, with no service.
fn driver(name: &str, registers: Vec<Type>, results: Vec<Type>, failures: u32) -> Function {
    Function {
        symbol: name.into(), entry_abi: EntryAbi::AsyncExternal,
        params: (0..registers.len() as u32).map(Location::Register).collect(),
        registers, services: vec![], results, blocks: vec![], code: vec![],
        proofs: FunctionProofs {
            entry: Some(EntryContract { external: Some(name.into()), failures, ..EntryContract::default() }),
            ..FunctionProofs::default()
        },
    }
}

/// verify(): an `AsyncExternal` is a bodyless external whose parameters are its registers in order, with no
/// service and at most one result (`SPB_EXTERNAL_ASYNC`).
pub(super) fn check_declaration(f: &Function) -> Result<(), Error> {
    if f.entry_abi != EntryAbi::AsyncExternal {
        return Ok(());
    }
    let external = f.proofs.entry.as_ref().is_some_and(|entry| entry.external.is_some() && entry.destroy.is_none());
    enforce(external && f.code.is_empty() && f.blocks.is_empty(), EXTERNAL_ASYNC,
        "an async external is a bodyless external entry")?;
    let registers = (0..f.registers.len() as u32).map(Location::Register).collect::<Vec<_>>();
    enforce(f.services.is_empty() && f.params == registers && f.results.len() <= 1, EXTERNAL_ASYNC,
        "an async external takes its registers in order, no service, and has at most one result")
}

/// The language argument types `fut.new` passes to the async external `callee`: its registers.
pub(super) fn arguments(callee: &Function) -> Vec<ProducedType> {
    callee.registers.iter().map(|&ty| ProducedType::Value(ty)).collect()
}
