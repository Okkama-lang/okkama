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

/// `driven()` with the driver an async native of the runtime table (#601 N1): a canonical symbol.
const NATIVE: &str = "runtime/time@1::sleep_nanoseconds/1";

fn driven_native() -> Program {
    let mut p = driven();
    p.functions[2] = driver(NATIVE);
    p
}

#[test]
fn fut_new_creates_the_future_of_a_native_driver_with_an_empty_packet() {
    // Ruling PR-N: own offers a native nothing, so `fut.new`'s inputs packet is `own.empty`.
    verify_load(&driven_native(), &Catalogue(vec![driver(NATIVE)])).unwrap();
}

#[test]
fn a_native_drivers_inputs_packet_may_be_any_packet_of_empty_inventory() {
    // Ruling PR-N states the inventory, not the form: the parts of an `own.split` of `own.empty`, joined with
    // another `own.empty`, hold no resource key either, so the native's Future is created from them.
    let mut p = super::async_tests::plain_joined();
    p.functions.push(driver(NATIVE));
    let main = &mut p.functions[1];
    for op in [O::OwnSplit, O::OwnJoin] {
        assert!(main.code.iter().any(|instruction| matches!(instruction,
            Instruction::Champ { op: found, .. } if *found == op)), "fixture changed");
    }
    let new = find(main, O::FutureNew);
    arguments(main, new)[0] = A::Entry(2);
    verify_load(&p, &Catalogue(vec![driver(NATIVE)])).unwrap();
}

/// `async_tests::leaking`'s body, which commits a `Res` into a packet and offers it to `fut.new`, with the driver
/// `name` (function 3) as the created Future's entry: an inputs packet that is not empty.
fn leaking(name: &str) -> Program {
    let mut p = super::async_tests::leaking(true);
    p.functions.push(driver(name));
    let body = &mut p.functions[1];
    let new = find(body, O::FutureNew);
    arguments(body, new)[0] = A::Entry(3);
    p
}

#[test]
fn a_native_drivers_inputs_packet_is_statically_empty() {
    // Ruling PR-N, checked before the general rule of plain inputs (which a handle's inputs would also meet).
    rejects(&leaking(NATIVE), &Catalogue(vec![driver(NATIVE)]), EXTERNAL_ASYNC, "statically empty (PR-N)");
    // The std async primitives' drivers are no natives: PR-N leaves them to CALL_PLAIN_PACKET (and Q2's PR-I).
    rejects(&leaking("q1.work"), &trusted(), "CALL_PLAIN_PACKET", "plain Future inputs");
}

#[test]
fn the_program_entry_is_no_driver() {
    // #774: only `fut.new` starts a driver; a program cannot begin in one. Nothing else names the driver here.
    let mut p = plain();
    p.functions.push(driver(NATIVE));
    p.entry = Some(2);
    rejects(&p, &Catalogue(vec![driver(NATIVE)]), EXTERNAL_ASYNC, "program entry cannot be an async external");
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
            trace_layout: storage.iter().enumerate().filter_map(|(i, ty)| ty.traced().then_some(i as u32)).collect(),
            selection_layout: vec![], storage_fields: storage },
    }
}

fn entry<'c>(contracts: &'c p1::P1Contracts, name: &str) -> Option<&'c Function> {
    ExternalContracts::entry(contracts, name)
}

#[test]
fn the_event_cell_admits_its_entries_and_driver() {
    let objects = vec![object(EVENT, EVENT_STORAGE.to_vec(), &["initial"])];
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
        let contracts = p1::P1Contracts::new(&[object(EVENT, storage, &["initial"])]).unwrap();
        assert!(entry(&contracts, "okkama.event.Event.wait").is_none());
    }
    // Each fact alone takes the cell's entries away (#601 Q1 audit A2: a cell that reaches resources is no cell).
    for fact in 0..3 {
        let mut resource = object(EVENT, EVENT_STORAGE.to_vec(), &["initial"]);
        match fact {
            0 => resource.layout.may_reach_resource = true,
            1 => resource.layout.has_destroy = true,
            _ => resource.layout.may_async_destroy = true,
        }
        let contracts = p1::P1Contracts::new(&[resource]).unwrap();
        assert!(entry(&contracts, "okkama.event.Event.set").is_none(), "{fact}");
    }
    // Its one source field is `initial`, at storage 0.
    for fields in [&[][..], &["ready"][..]] {
        let contracts = p1::P1Contracts::new(&[object(EVENT, EVENT_STORAGE.to_vec(), fields)]).unwrap();
        assert!(entry(&contracts, "okkama.event.Event.set").is_none(), "{fields:?}");
    }
    // A native handle of the same name is no cell.
    let mut handle = object(EVENT, EVENT_STORAGE.to_vec(), &["initial"]);
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
fn a_role_slot_receiver_takes_no_deps() {
    // LockGuard does Closing: its close is a slot of the role, reached through an adapter of the slot's ABI and never
    // named by the witness table (ruling PR-D, #773), so its receiver is a plain argument.
    let mut closing = object("io.stream.Closing", vec![], &[]);
    closing.role = Some(RoleDecl { slots: vec![RoleSlot { name: "close".into(), entry_abi: EntryAbi::Observation,
        params: vec![], results: vec![] }] });
    let mut guard = object("async.lock.LockGuard", vec![], &[]);
    guard.implements = vec![RoleImpl { role: 0, slots: vec![0] }];
    let contracts = p1::P1Contracts::new(&[closing, guard]).unwrap();
    assert!(entry(&contracts, "okkama.lock.LockGuard.close").unwrap().services.is_empty());
    assert!(entry(&contracts, "okkama.lock.LockGuard.closed").unwrap().services.is_empty());
}

// --- The runtime cell (#601 Q1 audit A1) ------------------------------------------------------------------------

/// `plain()` with an Event cell declared, valid as is.
fn with_cell(edit: impl FnOnce(&mut Object)) -> Program {
    let mut p = plain();
    let mut event = object(EVENT, EVENT_STORAGE.to_vec(), &["initial"]);
    edit(&mut event);
    p.objects.push(event);
    p
}

#[test]
fn a_runtime_cell_is_a_hidden_word_of_an_object_with_source_fields() {
    verify_load(&with_cell(|_| {}), &NoExternalContracts).unwrap();
    let cell = 2;
    let named = with_cell(|event| event.fields.push(Field { name: "waiters".into(), storage: cell, once: false }));
    rejects(&named, &Catalogue(vec![]), verify::RUNTIME_CELL, "unselected hidden word");
    let selected = with_cell(|event| {
        event.layout.selection_layout.push(cell);
        event.layout.may_reach_resource = true;
    });
    rejects(&selected, &Catalogue(vec![]), verify::RUNTIME_CELL, "unselected hidden word");
    let bare = with_cell(|event| event.fields.clear());
    rejects(&bare, &Catalogue(vec![]), verify::RUNTIME_CELL, "unselected hidden word");
    let boxed = with_cell(|event| event.any_box = true);
    rejects(&boxed, &Catalogue(vec![]), verify::RUNTIME_CELL, "unselected hidden word");
    let untraced = with_cell(|event| event.layout.trace_layout.clear());
    assert!(verify_load(&untraced, &NoExternalContracts).unwrap_err().message.contains("trace layout"));
}

#[test]
fn a_runtime_cell_is_no_value_type() {
    // A register, a result or a Future outcome of the type would make it an operand of a comparison.
    let mut p = with_cell(|_| {});
    p.functions[1].registers.push(Type::RuntimeCell);
    rejects(&p, &Catalogue(vec![]), verify::RUNTIME_CELL, "never a value type");
    let mut p = with_cell(|_| {});
    p.tables.future_results.push(Some(Type::RuntimeCell));
    rejects(&p, &Catalogue(vec![]), verify::RUNTIME_CELL, "never a value type");
}

#[test]
fn a_runtime_cell_round_trips_the_codec() {
    let p = with_cell(|_| {});
    assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
}

/// #799: the rows name the std declarations whose members they are, the one source of the producer's objects.
#[test]
fn the_rows_name_the_declarations_of_their_members() {
    let row = super::drivers::row_named("okkama.lock.LockGuard.close").unwrap();
    assert_eq!(super::drivers::member_of(row).as_deref(), Some("async.lock.LockGuard"));
    for name in [EVENT, "async.lock.Lock", "async.semaphore.Permit", "async.barrier.BarrierState",
        "async.cancellation.Cancellation", "async.cancellation.CancellationToken"]
    {
        assert!(super::drivers::has_members(name), "{name}");
    }
    // A declaration the runtime only creates or throws, a role, another module's object, a channel (Q2).
    for name in ["async.barrier.BrokenBarrier", "async.cancellation.Cancelling", "async.atomic.Atomic",
        "async.channel.Channel", "app.Event"]
    {
        assert!(!super::drivers::has_members(name), "{name}");
    }
}

// --- D23 holders: the rows of Q2 and G1 (memo §§5.2, 6.2) ------------------------------------------------------

use super::drivers::{self as d, Bindings, Slice, Template};

/// A declaration named `name` with `storage`, plain unless `duties`.
fn declared(name: &str, storage: Vec<Type>, duties: bool) -> Object {
    Object {
        name: name.into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle: false, role: None,
        implements: vec![], any_box: false, enum_names: vec![], fields: vec![],
        layout: ObjectRuntimeLayout { allocation_words: 1 + storage.len().max(1) as u32, identity_offset: None,
            has_destroy: false, may_reach_resource: duties, may_async_destroy: false,
            trace_layout: storage.iter().enumerate().filter_map(|(i, ty)| ty.traced().then_some(i as u32)).collect(),
            selection_layout: vec![], storage_fields: storage },
    }
}

/// A one-case Error enum the runtime throws.
fn thrown(name: &str, case: &str) -> Object {
    Object { error_cases: vec![format!("{}.{case}", name.rsplit('.').next().unwrap())],
        ..declared(name, vec![Type::Int], false) }
}

/// The int specializations of every holder family with their runtime layouts (PR-C), and the errors they throw.
fn holders() -> Vec<Object> {
    let cell = || d::HOLDER_STORAGE.to_vec();
    vec![
        declared("async.channel.Channel_int", cell(), true),
        declared("async.channel.Sending_int", cell(), false),
        declared("async.channel.Receiving_int", cell(), false),
        declared("async.scope.FutureGroup_int", cell(), true),
        declared("async.scope.FutureScope_int_string", vec![Type::Int], false),
        declared("async.collect.AllFutures_int", vec![Type::Int], false),
        declared("async.collect.Raced_int", vec![Type::Int], false),
        declared("async.collect.FirstSuccessful_int", vec![Type::Int], false),
        declared("async.collect.FutureCompletions_int", vec![Type::Int, Type::RuntimeCell], false),
        declared("async.timeout.Within_int", vec![Type::Int], false),
        declared("async.timeout.Before_int", vec![Type::Int], false),
        thrown("async.channel.InvalidChannelCapacity", "capacity"),
        thrown("async.channel.ChannelClosed", "closed"),
        thrown("async.scope.FinishedFutureGroup", "finished"),
        thrown("async.collect.NoFutures", "none"),
        thrown("async.timeout.TimedOut", "elapsed"),
    ]
}

fn bindings() -> Bindings {
    Bindings { value: Some(Type::Int), outcome: Some(Type::Str), future: Some(Type::Future(0)),
        sequence: Some(Type::Role(0)) }
}

fn suffixes(template: &Template) -> Vec<&'static str> {
    if template.name.starts_with("okkama.scope.FutureScope.") { vec!["int", "string"] } else { vec!["int"] }
}

/// The rows follow memo §§5.2 and 6.2: one per member Tomato runs as an intrinsic, a driver for each async member
/// (thrown, program and cancelled), none of them in the Q1 catalogue, and only those with a runtime ready (#810:
/// FutureGroup's five members, the two timeouts, then the race and first-success collectors).
#[test]
fn holder_rows_are_the_q2_and_g1_members() {
    let ready: Vec<_> = d::HOLDERS.iter().filter(|template| template.ready).map(|template| template.name).collect();
    assert_eq!(ready, ["okkama.scope.FutureGroup.new", "okkama.scope.FutureGroup.start",
        "okkama.scope.FutureGroup.active", "okkama.scope.FutureGroup.cancel", "okkama.scope.FutureGroup.wait",
        "okkama.collect.Raced.winner", "okkama.collect.FirstSuccessful.outcome",
        "okkama.timeout.Within.outcome", "okkama.timeout.Before.outcome"]);
    let q2 = d::HOLDERS.iter().filter(|template| template.slice == Slice::Q2).count();
    assert_eq!((q2, d::HOLDERS.len() - q2), (9, 13));
    let mut names = std::collections::BTreeSet::new();
    for template in d::HOLDERS {
        assert!(names.insert(template.name), "{} twice", template.name);
        assert!(d::row_named(template.name).is_none(), "{}", template.name);
        let family = template.name.split('.').nth(1).unwrap();
        let expected = if template.slice == Slice::Q2 { family == "channel" } else { family != "channel" };
        assert!(expected, "{}", template.name);
        if template.abi == d::Abi::Driver {
            assert_eq!(template.failures, d::THROWN | d::PROGRAM, "{}", template.name);
        }
        assert_eq!(!template.helpers.is_empty(), template.name.starts_with("okkama.timeout."), "{}", template.name);
    }
    for family in [d::CHANNEL, d::SENDING, d::RECEIVING, d::FUTURE_GROUP, d::FUTURE_COMPLETIONS] {
        assert!(!d::has_members(family), "{family}: the producer keeps its refusal until the runtime");
    }
}

/// Every row instantiates for an int specialization (FutureScope<int, string>): its name carries the suffixes, its
/// registers and results are the specialization's, and a driver is a well-formed async external.
#[test]
fn every_holder_row_instantiates_for_a_specialization() {
    let objects = holders();
    let id = |name: &str| Type::Object(objects.iter().position(|object| object.name == name).unwrap() as u32);
    for template in d::HOLDERS {
        let suffixes = suffixes(template);
        let (function, related) = d::instantiate(template, &objects, &suffixes, bindings())
            .unwrap_or_else(|| panic!("{}", template.name));
        assert_eq!(function.symbol, format!("{}${}", template.name, suffixes.join("$")));
        assert_eq!(related.len(), template.related.len(), "{}", template.name);
        super::drivers::check_declaration(&function).unwrap_or_else(|error| panic!("{}: {error}", template.name));
        let abi = match template.abi {
            d::Abi::Observation => EntryAbi::Observation,
            d::Abi::Owned => EntryAbi::Owned,
            d::Abi::Driver => EntryAbi::AsyncExternal,
        };
        assert_eq!(function.entry_abi, abi, "{}", template.name);
    }
    let send = d::holder_named("okkama.channel.Sending.send").unwrap();
    let (send, related) = d::instantiate(send, &objects, &["int"], bindings()).unwrap();
    let sending = id("async.channel.Sending_int");
    assert_eq!((send.registers.as_slice(), send.results.as_slice()), (&[sending, Type::Int][..], &[sending][..]));
    assert_eq!(related, [match id("async.channel.ChannelClosed") { Type::Object(id) => id, _ => unreachable!() }]);
    let start = d::holder_named("okkama.scope.FutureGroup.start").unwrap();
    let (start, _) = d::instantiate(start, &objects, &["int"], bindings()).unwrap();
    assert_eq!(start.registers, [id("async.scope.FutureGroup_int"), Type::Future(0)]);
}

/// Each declaration a row names needs its runtime layout (PR-C), and the value a holder carries is a value type.
#[test]
fn a_holder_row_needs_its_runtime_layout_and_a_carried_value() {
    let new = d::holder_named("okkama.channel.Channel.new_capacity").unwrap();
    let send = d::holder_named("okkama.channel.Sending.send").unwrap();
    assert!(d::instantiate(new, &holders(), &["int"], bindings()).is_some());
    // A Channel whose word is not a runtime cell, or that is declared plain (it holds duties, PR-C item 5).
    for broken in [declared("async.channel.Channel_int", vec![Type::Int], true),
        declared("async.channel.Channel_int", d::HOLDER_STORAGE.to_vec(), false)]
    {
        let mut objects = holders();
        objects[0] = broken;
        assert!(d::instantiate(new, &objects, &["int"], bindings()).is_none());
    }
    // An endpoint with the duties flag, or without its cell.
    let mut objects = holders();
    objects[1] = declared("async.channel.Sending_int", d::HOLDER_STORAGE.to_vec(), true);
    assert!(d::instantiate(send, &objects, &["int"], bindings()).is_none());
    // Another specialization's declarations, a builder message, a missing error declaration.
    assert!(d::instantiate(send, &holders(), &["string"], bindings()).is_none());
    let builder = Bindings { value: Some(Type::Builder(0)), ..bindings() };
    assert!(d::instantiate(send, &holders(), &["int"], builder).is_none());
    let mut objects = holders();
    objects.retain(|object| object.name != "async.channel.ChannelClosed");
    assert!(d::instantiate(send, &objects, &["int"], bindings()).is_none());
    // A scalar or role suffix names its value; a `t_` one waits for CHANNEL_MESSAGE.
    assert_eq!(d::suffix_value(&holders(), "int"), Some(Type::Int));
    assert_eq!(d::suffix_value(&holders(), "t_object(app::Res)"), None);
}

/// #810: an entry of a ready row, by any specialization, runs Tomato's intrinsic of the row's name; a row without its
/// runtime, a name without a specialization and an Atomic entry are no ready holder entries.
#[test]
fn a_ready_holder_entry_is_a_specialized_ready_row() {
    let row = |name: &str| d::ready_holder(name).map(|row| row.name);
    assert_eq!(row("okkama.scope.FutureGroup.start$int"), Some("okkama.scope.FutureGroup.start"));
    assert_eq!(row("okkama.scope.FutureGroup.wait$t_object(app::Res)"), Some("okkama.scope.FutureGroup.wait"));
    assert_eq!(row("okkama.scope.FutureGroup.start"), None);
    assert_eq!(row("okkama.channel.Sending.send$int"), None);
    assert_eq!(row("okkama.scope.FutureScope.outcome$int$string"), None);
    assert_eq!(row("okkama.atomic.Atomic.value$int"), None);
}

/// #810: a FutureGroup specialization is named by its family and a suffix; other families and prefixes are not.
#[test]
fn a_group_specialization_is_named_by_its_family() {
    assert!(d::group_specialization("async.scope.FutureGroup_int"));
    assert!(d::group_specialization("async.scope.FutureGroup_t_object(app::Res)"));
    for name in ["async.scope.FutureGroup", "async.scope.FutureGroups_int", "async.scope.FutureScope_int_string",
        "async.channel.Channel_int", "app.FutureGroup_int"]
    {
        assert!(!d::group_specialization(name), "{name}");
    }
}

// --- HOLDER_HELPER (#810, G1 step 2) -----------------------------------------------------------------------------

/// A function of `params` returning `results`, compiled (one instruction), or the external `name` of a timeout row.
fn shaped(symbol: &str, entry_abi: EntryAbi, params: Vec<Type>, results: Vec<Type>, external: Option<&str>) -> Function {
    Function {
        symbol: symbol.into(), entry_abi, registers: params.clone(), services: vec![],
        params: (0..params.len() as u32).map(Location::Register).collect(), results, blocks: vec![],
        code: if external.is_some() { vec![] } else { vec![Instruction::Ret(vec![])] },
        proofs: FunctionProofs {
            entry: external.map(|name| EntryContract { external: Some(name.into()), ..EntryContract::default() }),
            ..FunctionProofs::default()
        },
    }
}

/// `Within_int`'s outcome entry with its two helpers, over the declarations their shapes name; the origin's outcome
/// is `origin`, the entry's result `result`.
fn within(origin: Option<Type>, result: Type) -> (Program, Function) {
    let mut p = Program::default();
    let mut timing = object(d::TIMING, vec![], &[]);
    timing.role = Some(RoleDecl::default());
    p.objects = vec![object(d::DURATION, vec![Type::Rat], &["seconds"]), timing,
        object("async.timeout.Within_int", vec![Type::Future(0), Type::Object(0), Type::Role(1)],
            &["origin", "limit", "clock"])];
    p.tables.future_results = vec![origin, None];
    let name = "okkama.timeout.Within.outcome$int";
    let entry = shaped(name, EntryAbi::AsyncExternal, vec![Type::Object(2)], vec![result], Some(name));
    p.functions = vec![
        shaped(&d::helper_symbol(name, 0), EntryAbi::Observation, vec![Type::Object(0)], vec![Type::Rat], None),
        shaped(&d::helper_symbol(name, 1), EntryAbi::Owned, vec![Type::Role(1), Type::Object(0)],
            vec![Type::Future(1)], None),
    ];
    (p, entry)
}

/// The loader checks what Tomato's timeout driver reads: the receiver's layout, each helper's shape, and that the
/// origin's outcome is the entry's result (review T2: the driver returns the origin's result word as the outcome).
#[test]
fn the_timeout_helpers_and_the_origin_match_the_entry() {
    let name = "okkama.timeout.Within.outcome$int";
    let (p, entry) = within(Some(Type::Int), Type::Int);
    d::check_helpers(&p, &entry, name).unwrap();
    let (p, entry) = within(Some(Type::Str), Type::Int);
    let refused = d::check_helpers(&p, &entry, name).unwrap_err();
    assert_eq!(refused.rule, d::HOLDER_HELPER, "{refused:?}");
    let (mut p, entry) = within(Some(Type::Int), Type::Int);
    let symbols = (p.functions[0].symbol.clone(), p.functions[1].symbol.clone());
    (p.functions[0].symbol, p.functions[1].symbol) = (symbols.1, symbols.0);
    assert_eq!(d::check_helpers(&p, &entry, name).unwrap_err().rule, d::HOLDER_HELPER);
}

/// `Raced_int`'s winner entry over a receiver whose first storage field is a role with the slots `slots`: `size()`
/// returns int and `item(int)` returns `Future(0)`, whose outcome is `outcome`; the entry's result is `result`.
fn raced(outcome: Option<Type>, result: Type, slots: &[&str]) -> (Program, Function) {
    let mut p = Program::default();
    let mut sequence = object("collections.sequence.Sequencing_future_int", vec![], &[]);
    sequence.role = Some(RoleDecl { slots: slots.iter().map(|&name| RoleSlot {
        name: name.into(), entry_abi: EntryAbi::Observation,
        params: if name == "item" { vec![Type::Int] } else { vec![] },
        results: if name == "item" { vec![Type::Future(0)] } else { vec![Type::Int] },
    }).collect() });
    p.objects = vec![sequence, object("async.collect.Raced_int", vec![Type::Role(0)], &["origins"])];
    p.tables.future_results = vec![outcome];
    let name = "okkama.collect.Raced.winner$int";
    let entry = shaped(name, EntryAbi::AsyncExternal, vec![Type::Object(1)], vec![result], Some(name));
    (p, entry)
}

/// #810 G1 step 3, `HOLDER_ORIGINS`: the loader checks what Tomato's collector driver reads, a receiver whose first
/// field is a sequence of Futures with `size` and `item`, and that those Futures' outcome is the entry's result (the
/// driver hands a member's result word on as the winner).
#[test]
fn a_collectors_origins_match_the_entry() {
    let name = "okkama.collect.Raced.winner$int";
    let (p, entry) = raced(Some(Type::Int), Type::Int, &["size", "item"]);
    d::check_origins(&p, &entry, name).unwrap();
    let (p, entry) = raced(Some(Type::Str), Type::Int, &["size", "item"]);
    assert_eq!(d::check_origins(&p, &entry, name).unwrap_err().rule, d::HOLDER_ORIGINS);
    let (p, entry) = raced(Some(Type::Int), Type::Int, &["item"]);
    assert_eq!(d::check_origins(&p, &entry, name).unwrap_err().rule, d::HOLDER_ORIGINS);
    // A row whose driver reads no origins is not checked.
    let (p, entry) = raced(Some(Type::Str), Type::Int, &["size", "item"]);
    d::check_origins(&p, &entry, "okkama.timeout.Within.outcome$int").unwrap();
}
