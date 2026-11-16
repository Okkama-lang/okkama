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

/// Failure mask bits of an entry (`EntryContract::failures`): a thrown error, a program failure, a panic and a
/// native provider's failure. A cancellation has no bit.
pub const THROWN: u32 = 1;
pub const PROGRAM: u32 = 2;
pub const PANIC: u32 = 4;
pub const NATIVE: u32 = 8;

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

/// The std declaration whose member a row is: `okkama.<family>.<Object>.<member>` is a member of
/// `async.<family>.<Object>`, the declaration of `async/<family>`'s object.
pub fn member_of(row: &Row) -> Option<String> {
    let mut parts = row.name.strip_prefix("okkama.")?.split('.');
    let (family, object) = (parts.next()?, parts.next()?);
    Some(format!("async.{family}.{object}"))
}

/// Whether some row is a member of the declaration `name` (#799: the producer's objects of the catalogue).
pub fn has_members(name: &str) -> bool {
    CATALOGUE.iter().any(|row| member_of(row).as_deref() == Some(name))
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

/// Ruling PR-N (#601 N1): a native driver's inputs packet is statically empty. Native parameters are plain data and
/// handles, which carry no ownership graph, so nothing may enter the driver's INPUTS (`SPB_EXTERNAL_ASYNC`).
///
/// What is checked: for each `fut.new` whose entry is an `AsyncExternal` with a canonical symbol (after the load
/// rules, an async native of the runtime table or its map adapter), the packet operand's evidence inventory
/// (`Evidence::inventory`) resolves and is empty. The form is free: `own.empty`, which own gives a native, and any
/// packet of no resource key (splits and joins of it, a packet of a resource-free program) pass alike. An
/// inventory that does not resolve is refused too, under `LOAD_WORK_BUDGET` when the evidence ran out of budget.
///
/// What is not: the inventory counts resource keys, not service entries (ABI D25). A cache duty absorbed into the
/// packet leaves it empty here; the D25 rule `SPB_FUTURE_INPUT` refuses such a packet, and Champ's `fut.new` stops
/// on a native driver's packet that holds a key or a service entry. The std async primitives' drivers are not
/// natives; Q2 lifts the rule for `send` (PR-I).
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

// --- D23 holders: the catalogue rows of Q2 (channels) and G1 (groups, collectors, timeouts) ---------------------
//
// Async ABI D23, memo `OIR-2-async-remaining.md` §§5.2, 6.2 (rulings PR-C, PR-G … PR-J). Each row is a template,
// instantiated once per specialization of its objects, as the Atomic entries are: the declaration
// `async.<family>.<Object>_<suffix>` and the entry `okkama.<family>.<Object>.<member>$<suffix>` (two suffixes for
// FutureScope<T, R>). A suffix is the Atomic one: a scalar or array suffix, `role_<declaration>`, or `t_<encoded>`.
//
// The rows exist before the runtime does (lane A, slices A3 and A4). A row is `ready` once the runtime runs it
// (#810, G1 in steps: FutureGroup first); the producer keeps its named refusal of the other members, and the loader
// trusts only ready rows, so no other program changes behaviour.

/// The hidden runtime word of a holder (ruling PR-C): its one storage word, a traced `RuntimeCell` that refers to the
/// runtime payload (`ChannelHolder`, `GroupHolder`). Channel, its endpoints and FutureGroup have no source field.
pub const HOLDER_STORAGE: [Type; 1] = [Type::RuntimeCell];

pub const CHANNEL: &str = "async.channel.Channel";
pub const SENDING: &str = "async.channel.Sending";
pub const RECEIVING: &str = "async.channel.Receiving";
pub const FUTURE_GROUP: &str = "async.scope.FutureGroup";
pub const FUTURE_SCOPE: &str = "async.scope.FutureScope";
pub const ALL_FUTURES: &str = "async.collect.AllFutures";
pub const RACED: &str = "async.collect.Raced";
pub const FIRST_SUCCESSFUL: &str = "async.collect.FirstSuccessful";
pub const FUTURE_COMPLETIONS: &str = "async.collect.FutureCompletions";
pub const WITHIN: &str = "async.timeout.Within";
pub const BEFORE: &str = "async.timeout.Before";
const INVALID_CAPACITY: &str = "async.channel.InvalidChannelCapacity";
const CHANNEL_CLOSED: &str = "async.channel.ChannelClosed";
const FINISHED_GROUP: &str = "async.scope.FinishedFutureGroup";
const NO_FUTURES: &str = "async.collect.NoFutures";
const TIMED_OUT: &str = "async.timeout.TimedOut";

/// The slice that brings a row's runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slice {
    Q2,
    G1,
}

/// A parameter or result of a holder row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Int,
    /// A specialization of this declaration, by the row's suffix.
    Holder(&'static str),
    /// The value type T: a channel's message, a group's or collector's outcome.
    Value,
    /// FutureScope's result type R.
    Outcome,
    /// `Future<T>`.
    Future,
    /// `Sequencing<T>`, AllFutures' result.
    Sequence,
}

/// What a row's call does to ownership (the summary own builds for it in A3 and A4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Nothing moves: plain data or a view.
    Plain,
    /// A plain result that depends on parameter 0 (`external_deps = [0]`): an endpoint, AllFutures' wrapper.
    DependsOnReceiver,
    /// A fresh result whose packet holds `ServiceOf{result, Channel}` or `{result, Group}`.
    FreshHolder,
    /// Parameter 1 is offered to the holder's protocol (`destination = Protocol`): `send` accepts it, `start`
    /// registers a fresh Future or retains a bound one (PR-B); the result, if any, aliases parameter 0.
    Offer,
    /// The result aliases parameter 0 (`aliases = [param 0]`, #774 A6).
    AliasesReceiver,
    /// The result is delivered from the holder's protocol, provenance cache-owned (`Deliver`).
    Delivered,
    /// The result is a view of the receiver's graph (`aliases`, `opaque`): a race's winner, a timeout's outcome.
    View,
    /// The result is a borrowed origin Future, no fresh family (`completion`).
    BorrowedFuture,
}

/// A program function a driver calls (#810, G1 step 2): `<entry>.helper<index>`, a synchronous function the producer
/// synthesizes from the std declarations. The loader checks its shape (`HOLDER_HELPER`); the driver reads its result
/// as that shape says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Helper {
    /// `Duration -> rat`: `limit::Span::seconds`.
    Seconds,
    /// `(Timing, Duration) -> Future<void>`: `clock.wait(limit)`, the cold timer (an async role slot, PR-D).
    Wait,
    /// `Timing -> MonotonicInstant`: `clock.monotonic_instant()`.
    Now,
    /// `(MonotonicInstant, MonotonicInstant) -> Duration`: `deadline.minus(now)`.
    Minus,
}

/// `Within.outcome`'s helpers, as Tomato's timeout driver indexes them.
pub const WITHIN_HELPERS: &[Helper] = &[Helper::Seconds, Helper::Wait];
/// `Before.outcome`'s helpers: the remaining time from the deadline first.
pub const BEFORE_HELPERS: &[Helper] = &[Helper::Now, Helper::Minus, Helper::Seconds, Helper::Wait];
/// The std declarations a helper's shape names.
pub const DURATION: &str = "time.duration.Duration";
pub const MONOTONIC_INSTANT: &str = "time.clock.MonotonicInstant";
pub const TIMING: &str = "time.clock.Timing";

/// The symbol of the helper `index` of the entry `entry` (`okkama.timeout.Within.outcome$int.helper1`).
pub fn helper_symbol(entry: &str, index: usize) -> String {
    format!("{entry}.helper{index}")
}

/// #810: the helpers of a ready row's driver are the program's functions of the shapes it reads, and the timeout
/// object it reads by storage index has the layout it expects: `origin`, the limit (`Duration` for Within, a
/// `MonotonicInstant` deadline for Before) and the `Timing` clock.
pub const HOLDER_HELPER: &str = "HOLDER_HELPER";

pub(super) fn check_helpers(p: &Program, f: &Function, name: &str) -> Result<(), Error> {
    let Some(row) = ready_holder(name).filter(|row| !row.helpers.is_empty()) else { return Ok(()) };
    let named = |ty: Type, wanted: &str| matches!(ty, Type::Object(id) | Type::Role(id)
        if p.objects.get(id as usize).is_some_and(|object| object.name == wanted));
    let limit = if row.name.starts_with("okkama.timeout.Within.") { DURATION } else { MONOTONIC_INSTANT };
    let layout = match f.registers.first() {
        Some(&Type::Object(id)) => p.objects.get(id as usize).map(|object| object.layout.storage_fields.as_slice()),
        _ => None,
    };
    enforce(matches!(layout, Some([Type::Future(_), limit_field, clock]) if named(*limit_field, limit)
        && named(*clock, TIMING)), HOLDER_HELPER,
        &format!("{name}: the receiver is not origin, {limit} and Timing"))?;
    // The driver returns the origin's result word as the outcome: the origin's outcome type is the entry's result
    // (#810 review T2).
    let origin = match layout {
        Some([Type::Future(id), ..]) => p.tables.future_results.get(*id as usize).copied(),
        _ => None,
    };
    enforce(origin == Some(f.results.first().copied()), HOLDER_HELPER,
        &format!("{name}: the origin's outcome is not the entry's result"))?;
    for (index, kind) in row.helpers.iter().enumerate() {
        let symbol = helper_symbol(name, index);
        let helper = p.functions.iter().find(|helper| helper.symbol == symbol)
            .ok_or_else(|| violation(HOLDER_HELPER, format!("{name} has no {symbol}")))?;
        let params: Vec<Type> = helper.params.iter().filter_map(|&location| match verify::loc_type(helper, location) {
            Ok(ProducedType::Value(ty)) => Some(ty),
            _ => None,
        }).collect();
        let results = helper.results.as_slice();
        let shape = match kind {
            Helper::Seconds => matches!(params.as_slice(), [d] if named(*d, DURATION)) && results == [Type::Rat],
            Helper::Wait => matches!(params.as_slice(), [c, d] if named(*c, TIMING) && named(*d, DURATION))
                && matches!(results, [Type::Future(id)] if p.tables.future_results.get(*id as usize) == Some(&None)),
            Helper::Now => matches!(params.as_slice(), [c] if named(*c, TIMING))
                && matches!(results, [i] if named(*i, MONOTONIC_INSTANT)),
            Helper::Minus => matches!(params.as_slice(), [a, b] if named(*a, MONOTONIC_INSTANT)
                && named(*b, MONOTONIC_INSTANT)) && matches!(results, [d] if named(*d, DURATION)),
        };
        let compiled = !helper.code.is_empty() && helper.proofs.entry.as_ref().is_none_or(|entry| entry.external.is_none())
            && matches!(helper.entry_abi, EntryAbi::Observation | EntryAbi::Owned);
        enforce(shape && compiled, HOLDER_HELPER, &format!("{symbol} is not a compiled {kind:?} helper"))?;
    }
    Ok(())
}

/// One row of a holder family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Template {
    /// The intrinsic's name, as Tomato names it; the entry adds `$<suffix>`.
    pub name: &'static str,
    pub params: &'static [Slot],
    pub results: &'static [Slot],
    pub abi: Abi,
    /// The failure mask: [`THROWN`], [`PROGRAM`]; a driver can also be cancelled.
    pub failures: u32,
    pub effect: Effect,
    /// The declarations the runtime creates or throws: a holder by its family (specialized), an error as named.
    pub related: &'static [&'static str],
    /// The program's helpers the driver calls, in the order it indexes them (`Within`, `Before`: the clock, #810).
    pub helpers: &'static [Helper],
    pub slice: Slice,
    /// Whether the runtime runs this row: the producer declares it as an intrinsic and the loader trusts it (#810).
    pub ready: bool,
}

#[allow(clippy::too_many_arguments)]
const fn holder(name: &'static str, params: &'static [Slot], results: &'static [Slot], abi: Abi, failures: u32,
    effect: Effect, related: &'static [&'static str], slice: Slice) -> Template
{
    Template { name, params, results, abi, failures, effect, related, helpers: &[], slice, ready: false }
}

/// A row the runtime runs (#810).
const fn ready(template: Template) -> Template {
    Template { ready: true, ..template }
}

use Slot::{Future as FUT, Holder as H, Int as INT, Outcome as OUT, Sequence as SEQ, Value as VAL};

/// The rows of memo §5.2 (Q2) and §6.2 (G1), in their order. `Receiving.next` is an ordinary std body.
pub const HOLDERS: &[Template] = &[
    holder("okkama.channel.Channel.new_capacity", &[INT], &[H(CHANNEL)], Abi::Owned, THROWN | PROGRAM,
        Effect::FreshHolder, &[CHANNEL, INVALID_CAPACITY], Slice::Q2),
    holder("okkama.channel.Channel.sending", &[H(CHANNEL)], &[H(SENDING)], Abi::Owned, PROGRAM,
        Effect::DependsOnReceiver, &[SENDING], Slice::Q2),
    holder("okkama.channel.Channel.receiving", &[H(CHANNEL)], &[H(RECEIVING)], Abi::Owned, PROGRAM,
        Effect::DependsOnReceiver, &[RECEIVING], Slice::Q2),
    holder("okkama.channel.Sending.new", &[], &[H(SENDING)], Abi::Owned, 0, Effect::Plain, &[SENDING], Slice::Q2),
    holder("okkama.channel.Receiving.new", &[], &[H(RECEIVING)], Abi::Owned, 0, Effect::Plain, &[RECEIVING],
        Slice::Q2),
    holder("okkama.channel.Sending.send", &[H(SENDING), VAL], &[H(SENDING)], Abi::Driver, THROWN | PROGRAM,
        Effect::Offer, &[CHANNEL_CLOSED], Slice::Q2),
    holder("okkama.channel.Receiving.item", &[H(RECEIVING)], &[VAL], Abi::Driver, THROWN | PROGRAM,
        Effect::Delivered, &[CHANNEL_CLOSED], Slice::Q2),
    holder("okkama.channel.Sending.close", &[H(SENDING)], &[], Abi::Observation, PROGRAM, Effect::Plain, &[],
        Slice::Q2),
    holder("okkama.channel.Receiving.close", &[H(RECEIVING)], &[], Abi::Observation, PROGRAM, Effect::Plain, &[],
        Slice::Q2),
    ready(holder("okkama.scope.FutureGroup.new", &[], &[H(FUTURE_GROUP)], Abi::Owned, 0, Effect::FreshHolder,
        &[FUTURE_GROUP], Slice::G1)),
    ready(holder("okkama.scope.FutureGroup.start", &[H(FUTURE_GROUP), FUT], &[], Abi::Owned, THROWN | PROGRAM,
        Effect::Offer, &[FINISHED_GROUP], Slice::G1)),
    ready(holder("okkama.scope.FutureGroup.active", &[H(FUTURE_GROUP)], &[INT], Abi::Observation, PROGRAM, Effect::Plain,
        &[], Slice::G1)),
    ready(holder("okkama.scope.FutureGroup.cancel", &[H(FUTURE_GROUP)], &[], Abi::Observation, PROGRAM, Effect::Plain,
        &[], Slice::G1)),
    ready(holder("okkama.scope.FutureGroup.wait", &[H(FUTURE_GROUP)], &[H(FUTURE_GROUP)], Abi::Driver, THROWN | PROGRAM,
        Effect::AliasesReceiver, &[], Slice::G1)),
    holder("okkama.scope.FutureScope.outcome", &[H(FUTURE_SCOPE)], &[OUT], Abi::Driver, THROWN | PROGRAM,
        Effect::Delivered, &[FUTURE_GROUP], Slice::G1),
    holder("okkama.collect.AllFutures.results", &[H(ALL_FUTURES)], &[SEQ], Abi::Driver, THROWN | PROGRAM,
        Effect::DependsOnReceiver, &[], Slice::G1),
    holder("okkama.collect.Raced.winner", &[H(RACED)], &[VAL], Abi::Driver, THROWN | PROGRAM, Effect::View,
        &[NO_FUTURES], Slice::G1),
    holder("okkama.collect.FirstSuccessful.outcome", &[H(FIRST_SUCCESSFUL)], &[VAL], Abi::Driver, THROWN | PROGRAM,
        Effect::View, &[NO_FUTURES], Slice::G1),
    holder("okkama.collect.FutureCompletions.remaining_count", &[H(FUTURE_COMPLETIONS)], &[INT], Abi::Observation,
        PROGRAM, Effect::Plain, &[], Slice::G1),
    holder("okkama.collect.FutureCompletions.completion", &[H(FUTURE_COMPLETIONS)], &[FUT], Abi::Driver,
        THROWN | PROGRAM, Effect::BorrowedFuture, &[NO_FUTURES], Slice::G1),
    ready(Template { helpers: WITHIN_HELPERS, ..holder("okkama.timeout.Within.outcome", &[H(WITHIN)], &[VAL], Abi::Driver,
        THROWN | PROGRAM, Effect::View, &[TIMED_OUT], Slice::G1) }),
    ready(Template { helpers: BEFORE_HELPERS, ..holder("okkama.timeout.Before.outcome", &[H(BEFORE)], &[VAL], Abi::Driver,
        THROWN | PROGRAM, Effect::View, &[TIMED_OUT], Slice::G1) }),
];

/// The holder row named `name` (without its suffix).
pub fn holder_named(name: &str) -> Option<&'static Template> {
    HOLDERS.iter().find(|template| template.name == name)
}

/// #810: the row of a ready holder entry `name$suffix…`, which the runtime runs; its `name` is Tomato's.
pub fn ready_holder(name: &str) -> Option<&'static Template> {
    let (base, _) = name.split_once('$')?;
    holder_named(base).filter(|template| template.ready)
}

/// #810: whether a declaration is a specialization of FutureGroup (`async.scope.FutureGroup_<suffix>`): its runtime
/// word refers to the runtime's group record, and its packet entry is the group's own duty.
pub fn group_specialization(name: &str) -> bool {
    name.strip_prefix(FUTURE_GROUP).is_some_and(|rest| rest.starts_with('_'))
}

/// The std declaration whose member a holder row is: `okkama.<family>.<Object>.<member>` of
/// `async.<family>.<Object>` (before its specialization).
pub fn member_of_holder(template: &Template) -> String {
    let mut parts = template.name.split('.').skip(1);
    format!("async.{}.{}", parts.next().unwrap_or(""), parts.next().unwrap_or(""))
}

/// The entry of `template` for a specialization: `name$suffix`, and `name$T$R` for FutureScope.
pub fn specialized(template: &Template, suffixes: &[&str]) -> String {
    std::iter::once(template.name).chain(suffixes.iter().copied()).collect::<Vec<_>>().join("$")
}

/// #810: whether a declaration is the specialization of a holder family whose runtime word is its only storage
/// (`holder_layout`): a Channel, its endpoints, a FutureGroup.
pub fn holder_specialization(name: &str) -> bool {
    [CHANNEL, SENDING, RECEIVING, FUTURE_GROUP].iter()
        .any(|family| name.strip_prefix(family).is_some_and(|rest| rest.starts_with('_')))
}

/// The declaration of a holder family's specialization: `async.channel.Channel_<suffix>`.
pub fn specialization(family: &str, suffixes: &[&str]) -> String {
    format!("{family}_{}", suffixes.join("_"))
}

/// The types a specialization binds: T (the suffix's value), R (FutureScope), `Future<T>`, `Sequencing<T>`. The
/// caller resolves them: a scalar or role suffix by [`suffix_value`], a `t_` one by the loader rule
/// `CHANNEL_MESSAGE` (slice A3), the Future and Sequencing types from the program's tables.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bindings {
    pub value: Option<Type>,
    pub outcome: Option<Type>,
    pub future: Option<Type>,
    pub sequence: Option<Type>,
}

/// The value type a scalar, array or role suffix names, as for the Atomic entries; None for a `t_` suffix.
pub fn suffix_value(objects: &[Object], suffix: &str) -> Option<Type> {
    super::p1::atomic_value(suffix).or_else(|| super::p1::atomic_role(objects, suffix))
}

/// A holder declaration with the runtime word of ruling PR-C: no source field, the one storage word a
/// `RuntimeCell`, no destroy or identity. A Channel and a FutureGroup hold duties (`may_reach_resource`, PR-C item
/// 5); their endpoints are plain.
fn holder_layout(object: &Object, duties: bool) -> bool {
    let layout = &object.layout;
    object.fields.is_empty() && layout.storage_fields == HOLDER_STORAGE && !layout.has_destroy
        && layout.identity_offset.is_none() && layout.may_reach_resource == duties && !object.handle
        && object.role.is_none() && object.error_cases.is_empty() && object.enum_cases == 0
}

/// The specialization of `family` in `objects`, with its runtime layout, or None.
fn holder_declaration(objects: &[Object], family: &str, suffixes: &[&str]) -> Option<u32> {
    let name = specialization(family, suffixes);
    let id = objects.iter().position(|object| object.name == name)?;
    let object = &objects[id];
    let valid = match family {
        CHANNEL | FUTURE_GROUP => holder_layout(object, true),
        SENDING | RECEIVING => holder_layout(object, false),
        // The collectors, the scope and the timeouts keep their source fields; FutureCompletions adds its stream's
        // runtime word after them (PR-C item 1).
        FUTURE_COMPLETIONS => object.layout.storage_fields.last() == Some(&Type::RuntimeCell),
        _ => !object.layout.storage_fields.contains(&Type::RuntimeCell),
    };
    valid.then_some(id as u32)
}

/// A value type a holder can carry (part of `CHANNEL_MESSAGE`): never a builder, `never` or a runtime word.
fn carried(ty: Type) -> bool {
    !matches!(ty, Type::Builder(_) | Type::MapBuilder(_) | Type::Never | Type::RuntimeCell)
}

/// #810: the related declarations of a holder row for one specialization: its holder families by their
/// specialization, its errors by name. None when one is missing.
pub fn holder_related(template: &Template, objects: &[Object], suffix: &str) -> Option<Vec<u32>> {
    template.related.iter().map(|name| {
        if [CHANNEL, SENDING, RECEIVING, FUTURE_GROUP].contains(name) {
            holder_declaration(objects, name, &[suffix])
        } else {
            objects.iter().position(|object| object.name == *name && !object.error_cases.is_empty()).map(|id| id as u32)
        }
    }).collect()
}

/// #810: the trusted entry of a ready holder row named `name$suffix`, for a declaration whose registers and results
/// give the bindings a suffix cannot (a `t_` element, the Future and Sequencing types of the program's tables).
pub fn holder_entry(name: &str, objects: &[Object], declared: &Function) -> Option<Function> {
    let (base, suffix) = name.split_once('$')?;
    let template = holder_named(base).filter(|template| template.ready)?;
    let mut bindings = Bindings::default();
    for (slot, &ty) in template.params.iter().zip(&declared.registers).chain(template.results.iter().zip(&declared.results)) {
        let binding = match slot {
            Slot::Future if matches!(ty, Type::Future(_)) => &mut bindings.future,
            Slot::Value => &mut bindings.value,
            Slot::Outcome => &mut bindings.outcome,
            Slot::Sequence => &mut bindings.sequence,
            _ => continue,
        };
        if binding.is_some_and(|bound| bound != ty) {
            return None;
        }
        *binding = Some(ty);
    }
    instantiate(template, objects, &[suffix], bindings).map(|(function, _)| function)
}

/// The trusted entry of `template` for the specialization `suffixes` (one, or T and R for FutureScope), or None when
/// a declaration it names lacks its runtime layout or a binding is missing or not carried.
pub fn instantiate(template: &Template, objects: &[Object], suffixes: &[&str], bindings: Bindings)
    -> Option<(Function, Vec<u32>)>
{
    let family_suffixes = |family: &str| if family == FUTURE_SCOPE { suffixes.to_vec() } else { vec![suffixes[0]] };
    let ty = |slot: &Slot| -> Option<Type> {
        let ty = match *slot {
            Slot::Int => Type::Int,
            Slot::Holder(family) => Type::Object(holder_declaration(objects, family, &family_suffixes(family))?),
            Slot::Value => bindings.value?,
            Slot::Outcome => bindings.outcome?,
            Slot::Future => bindings.future?,
            Slot::Sequence => bindings.sequence?,
        };
        carried(ty).then_some(ty)
    };
    let registers = template.params.iter().map(ty).collect::<Option<Vec<_>>>()?;
    let results = template.results.iter().map(ty).collect::<Option<Vec<_>>>()?;
    let related = template.related.iter().map(|name| {
        let holder = [CHANNEL, SENDING, RECEIVING, FUTURE_GROUP].contains(name);
        if holder {
            holder_declaration(objects, name, &[suffixes[0]])
        } else {
            objects.iter().position(|object| object.name == *name && !object.error_cases.is_empty()).map(|id| id as u32)
        }
    }).collect::<Option<Vec<_>>>()?;
    let name = specialized(template, suffixes);
    // Own gives deps to every managed parameter: the holder and a Future (#810).
    let deps = registers.iter().filter(|ty| super::p1::carries_deps(**ty)).count();
    let function = match template.abi {
        Abi::Observation => entry(&name, registers, deps, results, false, template.failures),
        Abi::Owned => entry(&name, registers, deps, results, true, template.failures),
        Abi::Driver => driver(&name, registers, results, template.failures),
    };
    // A result that aliases the receiver retains it (#774 A6, as the producer's summary says).
    let mut function = function;
    if matches!(template.effect, Effect::AliasesReceiver | Effect::View)
        && let Some(entry) = function.proofs.entry.as_mut() {
        entry.retained_params = vec![0];
    }
    Some((function, related))
}
