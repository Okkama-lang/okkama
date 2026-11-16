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
pub const THROWN: u32 = 1;
pub const PROGRAM: u32 = 2;

/// How a catalogue member is called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Abi {
    /// A synchronous observation: no packet.
    Observation,
    /// A synchronous owning entry.
    Owned,
    /// An async member: its call creates a Future that a driver runs.
    Driver,
}

/// One member of the trusted catalogue (#601 Q1). The rows are the one source of both its users (#774 D3): the
/// loader's trusted entries ([`entries`]) and the producer's declarations and summaries (`okkama-compiler`,
/// `lowering/oir2/drivers.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    /// The intrinsic's name, as Tomato names it.
    pub name: &'static str,
    /// Parameter and result types: `bool`, `int` or a declaration's name.
    pub params: &'static [&'static str],
    pub results: &'static [&'static str],
    pub abi: Abi,
    /// The failure mask: [`THROWN`], [`PROGRAM`].
    pub failures: u32,
    /// Whether the call raises Cancelled: every driver, and the check of a requested cancellation.
    pub cancels: bool,
    /// The declarations the runtime creates or throws, in Tomato's order.
    pub related: &'static [&'static str],
}

const fn row(name: &'static str, params: &'static [&'static str], results: &'static [&'static str], abi: Abi,
    failures: u32, related: &'static [&'static str]) -> Row
{
    Row { name, params, results, abi, failures, cancels: matches!(abi, Abi::Driver), related }
}

/// The catalogue (Tomato `intrinsics.rs` and `drivers.rs`). Program failures are the runtime's refusal of an object
/// it did not create (Tomato's "Invalid async event"); they pass every catch.
pub const CATALOGUE: &[Row] = &[
    row("okkama.event.Event.new", &[], &[EVENT], Abi::Owned, 0, &[EVENT]),
    row("okkama.event.Event.new_ready", &["bool"], &[EVENT], Abi::Owned, 0, &[EVENT]),
    row("okkama.event.Event.set", &[EVENT], &[], Abi::Observation, PROGRAM, &[]),
    row("okkama.event.Event.ready", &[EVENT], &["bool"], Abi::Observation, PROGRAM, &[]),
    row("okkama.event.Event.wait", &[EVENT], &[EVENT], Abi::Driver, PROGRAM, &[]),
    row("okkama.lock.Lock.new", &[], &[LOCK], Abi::Owned, 0, &[LOCK]),
    row("okkama.lock.Lock.locked", &[LOCK], &["bool"], Abi::Observation, PROGRAM, &[]),
    row("okkama.lock.Lock.guard", &[LOCK], &[GUARD], Abi::Driver, PROGRAM, &[GUARD]),
    row("okkama.lock.LockGuard.closed", &[GUARD], &["bool"], Abi::Observation, PROGRAM, &[]),
    row("okkama.lock.LockGuard.close", &[GUARD], &[], Abi::Observation, PROGRAM, &[]),
    row("okkama.semaphore.Semaphore.permits", &[SEMAPHORE], &["int"], Abi::Observation, PROGRAM, &[]),
    row("okkama.semaphore.Semaphore.permit", &[SEMAPHORE], &[PERMIT], Abi::Driver, PROGRAM, &[PERMIT]),
    row("okkama.semaphore.Permit.closed", &[PERMIT], &["bool"], Abi::Observation, PROGRAM, &[]),
    row("okkama.semaphore.Permit.close", &[PERMIT], &[], Abi::Observation, PROGRAM, &[]),
    row("okkama.barrier.Barrier.wait", &[BARRIER], &["int"], Abi::Driver, THROWN | PROGRAM, &[BROKEN_BARRIER]),
    row("okkama.barrier.Barrier.state", &[BARRIER], &[BARRIER_STATE], Abi::Owned, PROGRAM, &[BARRIER_STATE]),
    row("okkama.barrier.Barrier.abort", &[BARRIER], &[], Abi::Observation, PROGRAM, &[]),
    row("okkama.barrier.Barrier.reset", &[BARRIER], &[], Abi::Observation, PROGRAM, &[]),
    row("okkama.barrier.BarrierState.parties", &[BARRIER_STATE], &["int"], Abi::Observation, PROGRAM, &[]),
    row("okkama.barrier.BarrierState.waiters", &[BARRIER_STATE], &["int"], Abi::Observation, PROGRAM, &[]),
    row("okkama.barrier.BarrierState.generation", &[BARRIER_STATE], &["int"], Abi::Observation, PROGRAM, &[]),
    row("okkama.barrier.BarrierState.broken", &[BARRIER_STATE], &["bool"], Abi::Observation, PROGRAM, &[]),
    row("okkama.cancellation.Cancellation.new", &[], &[CANCELLATION], Abi::Owned, 0, &[CANCELLATION]),
    row("okkama.cancellation.Cancellation.cancelling", &[CANCELLATION], &[CANCELLING], Abi::Owned, PROGRAM, &[TOKEN]),
    row("okkama.cancellation.Cancellation.cancel", &[CANCELLATION], &[], Abi::Observation, PROGRAM, &[]),
    row("okkama.cancellation.CancellationToken.new", &[], &[TOKEN], Abi::Owned, 0, &[TOKEN]),
    row("okkama.cancellation.CancellationToken.requested", &[TOKEN], &["bool"], Abi::Observation, PROGRAM, &[]),
    // A requested cancellation raises Cancelled at the check, as Tomato's `TokenCheck`.
    Row { cancels: true, ..row("okkama.cancellation.CancellationToken.check", &[TOKEN], &[],
        Abi::Observation, PROGRAM, &[]) },
    row("okkama.cancellation.CancellationToken.wait", &[TOKEN], &[CANCELLING], Abi::Driver, PROGRAM, &[]),
];

/// The catalogue row named `name`.
pub fn row_named(name: &str) -> Option<&'static Row> {
    CATALOGUE.iter().find(|row| row.name == name)
}

/// The deps proofs of a member, as own elaborates its external signature: one for each Event cell parameter (a
/// builtin capability: a reference with deps). The other declarations of the catalogue are plain values without
/// references. A member that is a role slot of its object (LockGuard's `close` for `Closing`) is never named by a
/// witness table: the slot reaches it through an adapter of the slot's ABI (ruling PR-D, #773), so its receiver is
/// an ordinary plain argument. The producer reads this count (#774 D3).
pub fn deps(row: &Row) -> usize {
    row.params.iter().filter(|&&param| param == EVENT).count()
}

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
    for row in CATALOGUE {
        let registers = row.params.iter().map(|name| ty(objects, name)).collect::<Option<Vec<_>>>();
        let results = row.results.iter().map(|name| ty(objects, name)).collect::<Option<Vec<_>>>();
        let related = row.related.iter().map(|name| declaration(objects, name)).collect::<Option<Vec<_>>>();
        let (Some(registers), Some(results), Some(related)) = (registers, results, related) else {
            continue;
        };
        let function = match row.abi {
            Abi::Observation => entry(row.name, registers, deps(row), results, false, row.failures),
            Abi::Owned => entry(row.name, registers, deps(row), results, true, row.failures),
            Abi::Driver => driver(row.name, registers, results, row.failures),
        };
        entries.push((function, related));
    }
    entries
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

/// A driver's trusted entry: its arguments are its registers, in order, with no service. The std async primitives'
/// drivers and the async natives of the runtime table (#601 N1, `p1`) share it.
pub(super) fn driver(name: &str, registers: Vec<Type>, results: Vec<Type>, failures: u32) -> Function {
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

/// Ruling PR-N (#601 N1): a native driver's inputs packet is statically empty. The packet operand of a `fut.new`
/// whose entry is an async native of the runtime table (a canonical native symbol) has an empty finite inventory:
/// `own.empty`, or only splits and joins of it, as own gives a native without arguments. Native parameters are
/// plain data and handles, which carry no ownership graph, so nothing may enter the driver's INPUTS
/// (`SPB_EXTERNAL_ASYNC`). The std async primitives' drivers are not natives; Q2 lifts the rule for `send` (PR-I).
pub(super) fn check_native_inputs(p: &Program, f: &Function) -> Result<(), Error> {
    let mut evidence = None;
    for instruction in &f.code {
        let Instruction::Champ { op: Opcode::FutureNew, arguments, .. } = instruction else {
            continue;
        };
        let native = arguments.iter().find_map(|arg| match *arg {
            Argument::Entry(id) => p.functions.get(id as usize),
            _ => None,
        }).filter(|callee| callee.entry_abi == EntryAbi::AsyncExternal)
            .and_then(|callee| callee.proofs.entry.as_ref()?.external.as_deref())
            .is_some_and(|name| crate::abi::catalogue::Symbol::parse(name).is_some());
        if !native {
            continue;
        }
        let packet = arguments.iter().find_map(|arg| match arg {
            Argument::Location(location) if verify::loc_type(f, *location)
                == Ok(ProducedType::Service(ServiceKind::Packet)) => Some(*location),
            _ => None,
        });
        let evidence = evidence.get_or_insert_with(|| super::evidence::Evidence::new(p, f));
        let empty = packet.is_some_and(|packet| evidence.inventory(packet).is_some_and(|keys| keys.is_empty()));
        if !empty {
            evidence.check_budget()?;
        }
        enforce(empty, EXTERNAL_ASYNC, &format!("{}: a native driver's inputs packet is statically empty (PR-N)",
            f.symbol))?;
    }
    Ok(())
}

/// The language argument types `fut.new` passes to the async external `callee`: its registers.
pub(super) fn arguments(callee: &Function) -> Vec<ProducedType> {
    callee.registers.iter().map(|&ty| ProducedType::Value(ty)).collect()
}
