//! #601 D25: service duties inside packets in SPB 17. The dynamic settle (ID 84), service materialize and absorb
//! (IDs 85, 86), and their load rules: `SPB_SCHEDULE` requires exactly one dynamic step, after the static duties
//! and before the plan, in every close of a scope that may have admitted a service entry (ABI D25.3), and
//! `SPB_MATERIALIZE` takes only an entry its packet surely holds (D25.2, `d25-materialize-absent`). The programs
//! follow OIR-2 §10.1 `make()`: @make returns a cold Future through `cache.rehome ... target caller` (C-4 and its
//! ruling), and @caller admits the returned packet, whose entry its close settles. Each negative is valid up to
//! the rule it names.
use super::*;
use super::async_tests::{Body, Dynamic, FUTURE_INT, MAIN, CONSUME, l, p, program, reject_with, work};

use Argument as A;
use Opcode as O;
use ServiceKind as K;

/// @make's scope (the third function's).
const MAKER: u32 = CONSUME;
/// Function indices.
const CALLER: u32 = 1;
const MAKE: u32 = 2;

/// How @make returns its Future.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Make {
    /// The cache duty joins the reserve and leaves with the Future (C-4, target caller).
    Rehomed,
    /// The close settles the duty: the returned packet holds no entry.
    Settled,
}

/// What @caller does with the returned Future, and how its close settles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Caller {
    /// `let f = make();`: the returned packet joins the scope.
    Admit,
    /// A Future of @caller absorbed into an empty packet that joins the scope, with a direct plan instead of the
    /// executor.
    AbsorbDirect,
    /// The entry is materialized as a token and absorbed again before the packet joins the scope.
    Materialized,
    /// The materialize names a Future of @caller, whose entry the returned packet does not hold.
    MaterializeOther,
    /// No call: a Future of @caller is absorbed into an empty packet that joins the scope.
    AbsorbOwn,
    /// The same, and the absorbed token is also a duty of the close's schedule.
    AbsorbAndSettle,
    /// A Future of @caller is absorbed into a packet, and that packet is the inputs packet of a second Future of
    /// @work, whose only start value is an int (#782, `SPB_FUTURE_INPUT`); `feed: false` gives that Future an empty
    /// packet and admits the absorbed one instead.
    Feed { feed: bool },
}

/// The int argument of @work and its external deps.
fn seven(b: &mut Body) -> (Location, Location) {
    let seven = b.constant(Constant::Int("7".into()), Type::Int);
    let external = b.emit(O::DepsOf, vec![l(seven)], K::Deps);
    b.f.proofs.dependencies.push(Dependency { value: external, obligations: vec![seven] });
    (seven, external)
}

/// A cold Future of @work owned by `scope`: the Future and its cache duty.
fn create(b: &mut Body, state: &mut Location, scope: u32) -> (Location, Location) {
    let (seven, external) = seven(b);
    let offered = b.emit(O::OwnEmpty, vec![], K::Packet);
    let [future, cache, next] = [b.register(FUTURE_INT), b.service(K::CacheState), b.service(K::ScopeState)];
    b.next(O::FutureNew, vec![A::Entry(0), A::Scope(scope), A::Result(Some(FUTURE_INT)), l(seven), l(offered),
        l(external), l(*state)], vec![future, cache, next]);
    *state = next;
    (future, cache)
}

/// `make() is Future<int> { return work(7); }` (OIR-2 §10.1).
fn maker(variant: Make) -> Function {
    let mut b = Body::new("make", EntryAbi::Owned, vec![FUTURE_INT]);
    let input = b.service(K::Packet);
    b.f.params = vec![input];
    b.f.proofs.entry = Some(EntryContract::default());
    b.block("entry", &[input]);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(MAKER), A::Boundary(MAKER)], K::ScopeState);
    let mut state = b.emit(O::OwnAccept, vec![l(q0), l(input)], K::ScopeState);
    let (future, cache) = create(&mut b, &mut state, MAKER);
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let mut pending = b.emit(O::ExitPrepare, vec![A::Kind(ExitKind::Return), A::Boundary(MAKER), l(future),
        l(empty), A::Selection(selection)], K::Pending);
    let duties = match variant {
        Make::Rehomed => {
            pending = b.emit(O::CacheRehome, vec![A::Destination(ExitDestination::Caller), l(pending), l(cache)],
                K::Pending);
            vec![]
        }
        Make::Settled => vec![cache],
    };
    let finished = b.close(state, Some(pending), None, MAKER, &duties, 0, true);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, "done", vec![p(0), p(1)])]);
    b.block("done", &[closed, reserved]);
    let [result, packet, deps] = [b.register(FUTURE_INT), b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(ExitDestination::Caller)], vec![result, packet, deps]);
    b.f.code.push(Instruction::Champ { op: O::RetOwn, arguments: vec![l(result), l(packet), l(deps)],
        flow: Flow::Next(vec![]) });
    b.finish()
}

/// `action caller() { let f = make(); }` and its variants; `dynamic` places the close's dynamic step, if any.
fn caller(variant: Caller, dynamic: Option<Dynamic>) -> Function {
    let mut b = Body::new("caller", EntryAbi::Observation, vec![]);
    b.f.proofs.entry = Some(EntryContract::default());
    if let Some(dynamic) = dynamic {
        b.dynamic.insert(MAIN, dynamic);
    }
    b.block("entry", &[]);
    let mut state = b.emit(O::ScopeBegin, vec![A::Scope(MAIN), A::Boundary(MAIN)], K::ScopeState);
    let mut duties = Vec::new();
    let admitted = if let Caller::Feed { feed } = variant {
        let (_, cache) = create(&mut b, &mut state, MAIN);
        let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
        let absorbed = b.emit(O::ServiceAbsorb, vec![l(empty), l(cache)], K::Packet);
        let (seven, external) = seven(&mut b);
        let inputs = if feed { absorbed } else { b.emit(O::OwnEmpty, vec![], K::Packet) };
        let [future, cache, next] = [b.register(FUTURE_INT), b.service(K::CacheState), b.service(K::ScopeState)];
        b.next(O::FutureNew, vec![A::Entry(0), A::Scope(MAIN), A::Result(Some(FUTURE_INT)), l(seven), l(inputs),
            l(external), l(state)], vec![future, cache, next]);
        state = next;
        duties.push(cache);
        if feed { b.emit(O::OwnEmpty, vec![], K::Packet) } else { absorbed }
    } else if matches!(variant, Caller::AbsorbOwn | Caller::AbsorbAndSettle | Caller::AbsorbDirect) {
        let (_, cache) = create(&mut b, &mut state, MAIN);
        if variant == Caller::AbsorbAndSettle {
            duties.push(cache);
        }
        let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
        b.emit(O::ServiceAbsorb, vec![l(empty), l(cache)], K::Packet)
    } else {
        let offered = b.emit(O::OwnEmpty, vec![], K::Packet);
        b.outcomes(O::InvokeCallOwn, vec![A::Entry(MAKE), A::Summary(0), l(offered), A::Flag(false)],
            vec![(Tag::Normal, "made", vec![p(0), p(1), p(2)])]);
        let [future, packet, deps] = [b.register(FUTURE_INT), b.service(K::Packet), b.service(K::Deps)];
        b.block("made", &[future, packet, deps]);
        match variant {
            Caller::Materialized | Caller::MaterializeOther => {
                let holder = if variant == Caller::MaterializeOther {
                    let (other, cache) = create(&mut b, &mut state, MAIN);
                    duties.push(cache);
                    other
                } else {
                    future
                };
                let [rest, token] = [b.service(K::Packet), b.service(K::CacheState)];
                b.next(O::ServiceMaterialize, vec![l(packet), l(holder)], vec![rest, token]);
                b.emit(O::ServiceAbsorb, vec![l(rest), l(token)], K::Packet)
            }
            _ => packet,
        }
    };
    let state = b.emit(O::OwnAdmit, vec![l(state), l(admitted)], K::ScopeState);
    let finished = b.close(state, None, None, MAIN, &duties, 0, variant != Caller::AbsorbDirect);
    let closed = b.service(K::Closed);
    b.dispatch(finished, [false, false, false], vec![(Tag::Success, "done", vec![p(0)])]);
    b.block("done", &[closed]);
    b.ret();
    if variant == Caller::AbsorbAndSettle {
        // The counterexample has no consistent service lifetimes to record.
        return b.finish_unchecked();
    }
    b.finish()
}

fn returned(make: Make, variant: Caller, dynamic: Option<Dynamic>) -> Program {
    let (work, work_layout) = work();
    let mut p = program(vec![work, caller(variant, dynamic), maker(make)], vec![work_layout], None);
    p.tables.summaries.push(Summary { entry: MAKE, may_fail: false, retained_params: vec![] });
    p
}

fn loads(p: &Program) {
    verify_load(p, &NoExternalContracts).unwrap();
}

// --- Registry and codec ---------------------------------------------------------------------------------------

#[test]
fn d25_forms_have_their_schemas_and_classes() {
    let context = ResultContext::default();
    let cleanup = ProducedType::Service(K::Cleanup);
    assert_eq!(O::CleanupSettleDynamic.results(Tag::Normal, &context).unwrap(), vec![cleanup]);
    assert_eq!(O::ServiceMaterialize.results(Tag::Normal, &context).unwrap(),
        vec![ProducedType::Service(K::Packet), ProducedType::Service(K::CacheState)]);
    assert_eq!(O::ServiceAbsorb.results(Tag::Normal, &context).unwrap(), vec![ProducedType::Service(K::Packet)]);
    for op in [O::CleanupSettleDynamic, O::ServiceMaterialize, O::ServiceAbsorb] {
        assert_eq!(op.helper_class(), Some(HelperClass::L), "{}", op.name());
        assert!(!op.suspends() && !op.frame_bound() && !op.is_terminator(), "{}", op.name());
        assert_eq!(op.tags(&context), vec![Tag::Normal]);
    }
    assert_eq!(O::CleanupSettleDynamic.helper().unwrap().name, "champ_cleanup_settle_dynamic");
    assert_eq!(O::ServiceMaterialize.helper().unwrap().name, "champ_service_materialize");
    assert_eq!(O::ServiceAbsorb.helper().unwrap().name, "champ_service_absorb");
    // The async defer runs as a shielded child the frame waits for (draft 3 §§11, 14.4).
    let defer = helper("champ_invoke_defer_async").unwrap();
    assert_eq!(defer.opcode, Some(O::InvokeDefer));
    assert_ne!(defer.effects & HELPER_SUSPENDS, 0);
    assert_ne!(defer.effects & HELPER_UNWINDS, 0);
}

#[test]
fn d25_codec_round_trips_its_forms() {
    let programs = [returned(Make::Rehomed, Caller::Materialized, Some(Dynamic::AfterStatic)),
        returned(Make::Rehomed, Caller::AbsorbOwn, Some(Dynamic::AfterStatic)), passing(true)];
    let mut used = std::collections::BTreeSet::new();
    for p in &programs {
        let bytes = encode(p).unwrap();
        assert_eq!(&decode(&bytes).unwrap(), p);
        let text = disassemble(p).unwrap();
        assert_eq!(&assemble(&text).unwrap(), p);
        assert_eq!(disassemble(&assemble(&text).unwrap()).unwrap(), text);
        assert!(text.contains("champ cleanup.settle.dynamic"));
        used.extend(p.functions.iter().flat_map(|f| &f.code).filter_map(|instruction| match instruction {
            Instruction::Champ { op, .. } => Some(op.id()),
            _ => None,
        }));
    }
    for id in [69, 84, 85, 86] {
        assert!(used.contains(&id), "opcode {id} is not round-tripped");
    }
    // Both forms of cache.rehome.
    let forms: std::collections::BTreeSet<_> = programs.iter().flat_map(|p| &p.functions).flat_map(|f| &f.code)
        .filter_map(|instruction| match instruction {
            Instruction::Champ { op: O::CacheRehome, arguments, .. } => Some(arguments.len()),
            _ => None,
        }).collect();
    assert_eq!(forms, [3, 4].into());
}

#[test]
fn d25_operands_are_typed() {
    // A materialize names a Future holder; an absorb takes a duty token.
    let mut p = returned(Make::Rehomed, Caller::Materialized, Some(Dynamic::AfterStatic));
    let f = &mut p.functions[CALLER as usize];
    let pc = super::async_tests::find(f, O::ServiceMaterialize);
    let packet = super::async_tests::arguments(f, pc)[0].clone();
    super::async_tests::arguments(f, pc)[1] = packet;
    assert!(verify_load(&p, &NoExternalContracts).is_err());
    let mut p = returned(Make::Rehomed, Caller::AbsorbOwn, Some(Dynamic::AfterStatic));
    let f = &mut p.functions[CALLER as usize];
    let pc = super::async_tests::find(f, O::ServiceAbsorb);
    let packet = super::async_tests::arguments(f, pc)[0].clone();
    super::async_tests::arguments(f, pc)[1] = packet;
    let failure = verify_load(&p, &NoExternalContracts).unwrap_err();
    assert!(failure.message.contains("cache-state or task-state"), "{failure}");
}

// --- SPB_SCHEDULE: the dynamic step (D25.3) -------------------------------------------------------------------

#[test]
fn a_returned_future_leaves_through_its_reserve_and_settles_at_the_admitting_close() {
    loads(&returned(Make::Rehomed, Caller::Admit, Some(Dynamic::AfterStatic)));
}

#[test]
fn a_close_that_may_admit_an_entry_settles_it_dynamically() {
    reject_with(&returned(Make::Rehomed, Caller::Admit, None), suspension::SCHEDULE, "settles them dynamically");
}

#[test]
fn the_dynamic_step_follows_the_static_duties() {
    reject_with(&returned(Make::Rehomed, Caller::Admit, Some(Dynamic::BeforeStatic)), suspension::SCHEDULE,
        "follows the static duties");
}

#[test]
fn a_close_settles_its_dynamic_entries_once() {
    reject_with(&returned(Make::Rehomed, Caller::Admit, Some(Dynamic::Twice)), suspension::SCHEDULE, "once");
}

#[test]
fn an_absorbed_duty_makes_its_packet_carry_an_entry() {
    loads(&returned(Make::Rehomed, Caller::AbsorbOwn, Some(Dynamic::AfterStatic)));
    reject_with(&returned(Make::Rehomed, Caller::AbsorbOwn, None), suspension::SCHEDULE,
        "settles them dynamically");
}

#[test]
fn a_dynamic_step_releases_into_the_executor() {
    // The settled entries may release an outcome packet, which only the executor plan can take (draft 3 §6.3).
    reject_with(&returned(Make::Rehomed, Caller::AbsorbDirect, Some(Dynamic::AfterStatic)), suspension::SCHEDULE,
        "needs the executor");
}

#[test]
fn an_absorbed_duty_is_not_also_settled_statically() {
    // `coro-token-in-two-places` (D25.2): the token is linear, so the static settle finds it consumed.
    let failure = verify_load(&returned(Make::Rehomed, Caller::AbsorbAndSettle, Some(Dynamic::AfterStatic)),
        &NoExternalContracts).unwrap_err();
    assert_eq!(failure.rule, "LINEAR_CONSUME", "{failure}");
}

// --- SPB_MATERIALIZE (D25.2) ----------------------------------------------------------------------------------

#[test]
fn a_returned_entry_can_be_materialized() {
    loads(&returned(Make::Rehomed, Caller::Materialized, Some(Dynamic::AfterStatic)));
}

#[test]
fn a_materialize_names_an_entry_of_its_packet() {
    reject_with(&returned(Make::Rehomed, Caller::MaterializeOther, Some(Dynamic::AfterStatic)),
        services::MATERIALIZE, "does not surely hold");
}

#[test]
fn a_future_returned_without_its_duty_has_no_entry_to_materialize() {
    // @make settles the duty at its own close: the returned Future's packet holds nothing (D25.1).
    reject_with(&returned(Make::Settled, Caller::Materialized, Some(Dynamic::AfterStatic)),
        services::MATERIALIZE, "does not surely hold");
}

#[test]
fn only_a_rehomed_return_proves_its_entry() {
    let rehomed = returned(Make::Rehomed, Caller::Admit, Some(Dynamic::AfterStatic));
    assert_eq!(services::fresh_futures(&rehomed).unwrap(), vec![false, false, true]);
    let settled = returned(Make::Settled, Caller::Admit, Some(Dynamic::AfterStatic));
    assert_eq!(services::fresh_futures(&settled).unwrap(), vec![false, false, false]);
}

#[test]
fn holders_are_futures_and_what_reaches_them() {
    let p = returned(Make::Rehomed, Caller::Admit, None);
    assert!(services::reaches_holder(&p, FUTURE_INT));
    assert!(!services::reaches_holder(&p, Type::Int));
    let mut p = p;
    p.objects.push(super::async_tests::res());
    assert!(!services::reaches_holder(&p, Type::Object(0)));
    p.objects[0].layout.storage_fields = vec![Type::Str, FUTURE_INT];
    assert!(services::reaches_holder(&p, Type::Object(0)));
    assert!(services::reaches_holder(&p, Type::Successor(0)));
    // An unknown declaration may reach one.
    assert!(services::reaches_holder(&p, Type::Object(9)));
}

/// #810: a FutureGroup holds its own entry, so it is a holder whatever its storage (its runtime cell); an object of
/// another name with the same storage is not.
#[test]
fn a_future_group_is_a_holder() {
    let mut p = returned(Make::Rehomed, Caller::Admit, None);
    let mut group = super::async_tests::res();
    group.name = "async.scope.FutureGroup_int".into();
    group.layout.storage_fields = vec![Type::RuntimeCell];
    p.objects.push(group);
    assert!(services::reaches_holder(&p, Type::Object(0)));
    p.objects[0].name = "async.scope.FutureGroupish_int".into();
    assert!(!services::reaches_holder(&p, Type::Object(0)));
}

// --- SPB_REHOME, state form (ABI C-4 with D25.3) --------------------------------------------------------------

/// @pass's scope (the fourth function's).
const PASSER: u32 = super::async_tests::MAKE;
const PASS: u32 = 3;

/// `pass() is Future<int> { return make(); }`: the returned Future joins @pass's scope, and leaves it again in the
/// state form of `cache.rehome`, target caller. When `admitted` is false, the returned packet goes into the exit
/// instead, so the state does not hold the entry the rehome names.
fn passer(admitted: bool) -> Function {
    let mut b = Body::new("pass", EntryAbi::Owned, vec![FUTURE_INT]);
    let input = b.service(K::Packet);
    b.f.params = vec![input];
    b.f.proofs.entry = Some(EntryContract::default());
    b.dynamic.insert(PASSER, Dynamic::AfterStatic);
    b.block("entry", &[input]);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(PASSER), A::Boundary(PASSER)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(input)], K::ScopeState);
    let offered = b.emit(O::OwnEmpty, vec![], K::Packet);
    b.outcomes(O::InvokeCallOwn, vec![A::Entry(MAKE), A::Summary(0), l(offered), A::Flag(false)],
        vec![(Tag::Normal, "made", vec![p(0), p(1), p(2)])]);
    let [future, packet, deps] = [b.register(FUTURE_INT), b.service(K::Packet), b.service(K::Deps)];
    b.block("made", &[future, packet, deps]);
    let (state, exit_packet) = if admitted {
        (b.emit(O::OwnAdmit, vec![l(q1), l(packet)], K::ScopeState), b.emit(O::OwnEmpty, vec![], K::Packet))
    } else {
        (q1, packet)
    };
    let selection = b.selection();
    let pending = b.emit(O::ExitPrepare, vec![A::Kind(ExitKind::Return), A::Boundary(PASSER), l(future),
        l(exit_packet), A::Selection(selection)], K::Pending);
    let [state, pending] = [b.service(K::ScopeState), {
        let reserve = b.service(K::Pending);
        b.next(O::CacheRehome, vec![A::Destination(ExitDestination::Caller), l(pending), l(state), l(future)],
            vec![]);
        reserve
    }];
    // The rehome's outputs: the state, then the reserve.
    let last = b.f.code.len() - 1;
    let Instruction::Champ { flow: Flow::Next(outputs), .. } = &mut b.f.code[last] else { unreachable!() };
    *outputs = vec![state, pending];
    let finished = b.close(state, Some(pending), None, PASSER, &[], 0, true);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, "done", vec![p(0), p(1)])]);
    b.block("done", &[closed, reserved]);
    let [result, packet, deps] = [b.register(FUTURE_INT), b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(ExitDestination::Caller)], vec![result, packet, deps]);
    b.f.code.push(Instruction::Champ { op: O::RetOwn, arguments: vec![l(result), l(packet), l(deps)],
        flow: Flow::Next(vec![]) });
    b.finish()
}

fn passing(admitted: bool) -> Program {
    let mut p = returned(Make::Rehomed, Caller::Admit, Some(Dynamic::AfterStatic));
    p.functions.push(passer(admitted));
    p.tables.scopes.push(Scope { function: PASS, parent: None, boundary: PASSER });
    p.tables.boundaries.push(Boundary { scope: PASSER });
    assert_eq!(p.tables.scopes.len() as u32, PASSER + 1);
    p
}

#[test]
fn an_admitted_entry_leaves_its_scope_in_the_state_form() {
    loads(&passing(true));
    // @pass returns the entry it took from its state: its callers may materialize it (D25.1).
    assert!(services::fresh_futures(&passing(true)).unwrap()[PASS as usize]);
}

#[test]
fn the_state_form_takes_an_entry_its_state_holds() {
    reject_with(&passing(false), automaton::REHOME, "does not surely hold");
}

#[test]
fn the_state_form_names_the_scope_of_one_state() {
    // Two states of different scopes reach the rehome's state operand through one block parameter.
    let mut b = Body::new("both", EntryAbi::Observation, vec![]);
    let flag = b.constant(Constant::Bool(true), Type::Bool);
    let [first, second, joined] = [b.service(K::ScopeState), b.service(K::ScopeState), b.service(K::ScopeState)];
    b.next(O::ScopeBegin, vec![A::Scope(MAIN), A::Boundary(MAIN)], vec![first]);
    b.next(O::ScopeBegin, vec![A::Scope(PASSER), A::Boundary(PASSER)], vec![second]);
    b.cond(flag, "one", "two");
    b.block("one", &[]);
    b.br("join", vec![super::async_tests::s(first)]);
    b.block("two", &[]);
    b.br("join", vec![super::async_tests::s(second)]);
    b.block("join", &[joined]);
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let future = b.register(FUTURE_INT);
    let pending = b.emit(O::ExitPrepare, vec![A::Kind(ExitKind::Return), A::Boundary(MAIN), l(empty),
        A::Selection(0)], K::Pending);
    let [state, reserve] = [b.service(K::ScopeState), b.service(K::Pending)];
    b.next(O::CacheRehome, vec![A::Destination(ExitDestination::Caller), l(pending), l(joined), l(future)],
        vec![state, reserve]);
    b.ret();
    let f = b.finish_unchecked();
    let mut p = passing(true);
    p.functions.push(f.clone());
    let pc = f.code.iter().position(|instruction| matches!(instruction,
        Instruction::Champ { op: O::CacheRehome, .. })).unwrap();
    assert_eq!(services::rehome_scopes(&p, &f).unwrap().get(&pc), Some(&None));
    let pass = &p.functions[PASS as usize];
    let pc = pass.code.iter().position(|instruction| matches!(instruction,
        Instruction::Champ { op: O::CacheRehome, .. })).unwrap();
    assert_eq!(services::rehome_scopes(&p, pass).unwrap().get(&pc), Some(&Some(PASSER)));
}

/// #750: a function without a state-form rehome reports nothing and skips the scope analysis; the analysis of a
/// function with one still names the rehome's scope (above). Without the early exit the loader spent 50 s of a
/// 53 s debug load of L10.stdin.lines cloning dense scope maps of sync functions.
#[test]
fn rehome_scopes_of_a_function_without_a_state_form_rehome_are_empty() {
    let p = passing(true);
    for f in &p.functions {
        let rehomes = f.code.iter().any(|instruction| matches!(instruction,
            Instruction::Champ { op: O::CacheRehome, arguments, .. } if arguments.iter()
                .filter(|argument| matches!(argument, A::Location(_))).count() == 3));
        if !rehomes {
            assert!(services::rehome_scopes(&p, f).unwrap().is_empty(), "{}", f.symbol);
        }
    }
}


// --- #771: a dynamic selection moves the entries it reaches; SPB_EXIT_SELECTION -------------------------------

/// How @pass's exit takes the Future it returns, which its state admitted from @make (ruling "How a stored Future
/// leaves", #771).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExitBy {
    /// The exit's scope split selects the Future and the reserve takes the selected packet.
    Selection,
    /// The exit takes an empty packet: the entry stays in the closing state.
    Nothing,
    /// The exit selection is taken, then a state-form rehome names the Future its state no longer surely holds.
    SelectionThenRehome,
}

/// `pass() is Future<int> { return make(); }` with the admitted Future leaving by `by`.
fn exiting(by: ExitBy) -> Function {
    let mut b = Body::new("pass", EntryAbi::Owned, vec![FUTURE_INT]);
    let input = b.service(K::Packet);
    b.f.params = vec![input];
    b.f.proofs.entry = Some(EntryContract::default());
    b.dynamic.insert(PASSER, Dynamic::AfterStatic);
    b.block("entry", &[input]);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(PASSER), A::Boundary(PASSER)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(input)], K::ScopeState);
    let offered = b.emit(O::OwnEmpty, vec![], K::Packet);
    b.outcomes(O::InvokeCallOwn, vec![A::Entry(MAKE), A::Summary(0), l(offered), A::Flag(false)],
        vec![(Tag::Normal, "made", vec![p(0), p(1), p(2)])]);
    let [future, packet, deps] = [b.register(FUTURE_INT), b.service(K::Packet), b.service(K::Deps)];
    b.block("made", &[future, packet, deps]);
    let state = b.emit(O::OwnAdmit, vec![l(q1), l(packet)], K::ScopeState);
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let exit_selection = b.selection();
    let mut pending = b.emit(O::ExitPrepare, vec![A::Kind(ExitKind::Return), A::Boundary(PASSER), l(future),
        l(empty), A::Selection(exit_selection)], K::Pending);
    let mut state = state;
    if by != ExitBy::Nothing {
        let selection = b.selection();
        b.f.proofs.selections[selection as usize].locations = vec![future];
        b.outcomes(O::ScopeSplit, vec![l(state), A::Selection(selection), A::Flag(false)],
            vec![(Tag::Normal, "split", vec![p(0), p(1), p(2)]), (Tag::Unwind, "refused", vec![p(0), p(1)])]);
        let [rest, selected, external] = [b.service(K::ScopeState), b.service(K::Packet), b.service(K::Deps)];
        b.block("split", &[rest, selected, external]);
        pending = b.emit(O::ExitExtend, vec![l(pending), l(selected)], K::Pending);
        state = rest;
        if by == ExitBy::SelectionThenRehome {
            let [moved, reserve] = [b.service(K::ScopeState), b.service(K::Pending)];
            b.next(O::CacheRehome, vec![A::Destination(ExitDestination::Caller), l(pending), l(state), l(future)],
                vec![moved, reserve]);
            state = moved;
            pending = reserve;
        }
    }
    let finished = b.close(state, Some(pending), None, PASSER, &[], 0, true);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, "done", vec![p(0), p(1)])]);
    b.block("done", &[closed, reserved]);
    let [result, packet, deps] = [b.register(FUTURE_INT), b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(ExitDestination::Caller)], vec![result, packet, deps]);
    b.f.code.push(Instruction::Champ { op: O::RetOwn, arguments: vec![l(result), l(packet), l(deps)],
        flow: Flow::Next(vec![]) });
    if by != ExitBy::Nothing {
        let [state, failure] = [b.service(K::ScopeState), b.register(Type::Failure)];
        b.block("refused", &[state, failure]);
        b.f.code.push(Instruction::Unreachable);
    }
    b.finish_unchecked()
}

/// The program of `passing(true)` with @pass's exit by `by`, and @pass itself.
fn exit_program(by: ExitBy) -> (Program, Function) {
    let mut p = passing(true);
    let f = exiting(by);
    p.functions[PASS as usize] = f.clone();
    (p, f)
}

#[test]
fn an_exit_selection_that_takes_the_returned_future_meets_its_entry() {
    let (p, f) = exit_program(ExitBy::Selection);
    let fresh = services::fresh_futures(&p).unwrap();
    services::check_materialize(&p, &f, &fresh).unwrap();
}

#[test]
fn an_exit_without_its_selection_leaves_the_entry_behind() {
    let (p, f) = exit_program(ExitBy::Nothing);
    let fresh = services::fresh_futures(&p).unwrap();
    let failure = services::check_materialize(&p, &f, &fresh).unwrap_err();
    assert_eq!(failure.rule, services::EXIT_SELECTION, "{failure}");
    assert!(failure.message.contains("does not surely take"), "{failure}");
}

#[test]
fn a_selection_takes_the_entry_its_state_held() {
    // After the split the state no longer surely holds the Future's entry, so a state-form rehome of it is refused.
    let (p, f) = exit_program(ExitBy::SelectionThenRehome);
    let fresh = services::fresh_futures(&p).unwrap();
    let failure = services::check_materialize(&p, &f, &fresh).unwrap_err();
    assert_eq!(failure.rule, automaton::REHOME, "{failure}");
}

/// PR-B2 (#782), `SPB_FUTURE_INPUT`: `fut.new` gives @work an inputs packet that carries an entry, but @work's
/// only start value is an int, so its body could never hold it (`entry_carries`). The same program that admits the
/// absorbed packet in @caller instead loads.
#[test]
fn an_inputs_packet_carries_entries_only_into_a_body_that_can_take_them() {
    loads(&returned(Make::Rehomed, Caller::Feed { feed: false }, Some(Dynamic::AfterStatic)));
    reject_with(&returned(Make::Rehomed, Caller::Feed { feed: true }, Some(Dynamic::AfterStatic)),
        services::FUTURE_INPUT, "inputs packet that may carry service entries");
}

/// #848: an entry packet's entries accepted into a scope (`admitted` false: `own.accept`, the bound journal) or
/// admitted into it (`own.admit`), then a split of that scope whose selection reaches a Future, admissible when
/// `filtered`, and the selected packet admitted into a nested scope. Returns the program and the nested close's pc.
fn split_into_block(admitted: bool, filtered: bool) -> (Program, u32) {
    let mut b = Body::new("split", EntryAbi::Owned, vec![]);
    let input = b.service(K::Packet);
    let future = b.register(FUTURE_INT);
    b.f.params = vec![input, future];
    b.f.proofs.entry = Some(EntryContract::default());
    b.block("entry", &[input, future]);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(0), A::Boundary(0)], K::ScopeState);
    let q1 = b.emit(if admitted { O::OwnAdmit } else { O::OwnAccept }, vec![l(q0), l(input)], K::ScopeState);
    let selection = b.selection();
    b.f.proofs.selections[selection as usize].locations = vec![future];
    let mut arguments = vec![l(q1), A::Selection(selection)];
    if filtered {
        arguments.push(A::Flag(true));
    }
    b.outcomes(O::ScopeSplit, arguments, vec![(Tag::Normal, "split", vec![p(0), p(1), p(2)]),
        (Tag::Unwind, "failed", vec![p(0), p(1)])]);
    let [state, selected, deps] = [b.service(K::ScopeState), b.service(K::Packet), b.service(K::Deps)];
    b.block("split", &[state, selected, deps]);
    let n0 = b.emit(O::ScopeBegin, vec![A::Scope(1), A::Boundary(1)], K::ScopeState);
    let n1 = b.emit(O::OwnAdmit, vec![l(n0), l(selected)], K::ScopeState);
    let close = b.pc();
    b.emit(O::CleanupBegin, vec![l(n1), A::Boundary(1), A::Envelope(0)], K::Cleanup);
    b.f.code.push(Instruction::Unreachable);
    let [unwound, failure] = [b.service(K::ScopeState), b.register(Type::Failure)];
    b.block("failed", &[unwound, failure]);
    b.f.code.push(Instruction::Unreachable);
    let mut p = program(vec![b.finish_unchecked()], vec![], None);
    p.tables.scopes = vec![Scope { function: 0, parent: None, boundary: 0 },
        Scope { function: 0, parent: Some(0), boundary: 1 }];
    p.tables.boundaries = vec![Boundary { scope: 0 }, Boundary { scope: 1 }];
    (p, close)
}

/// #848 (Fable's ruling): an admissible split never selects a bound entry, so a selection from a scope whose entries
/// were only accepted carries none, and the nested scope that admits it need not settle entries dynamically. A
/// scope that admitted the entries, or a split without the admission filter, still passes them on.
#[test]
fn an_admissible_selection_of_accepted_entries_carries_none() {
    let admits = |admitted, filtered| {
        let (p, close) = split_into_block(admitted, filtered);
        services::admitting_closes(&p, &p.functions[0]).unwrap().contains(&close)
    };
    assert!(!admits(false, true), "accepted entries, admissible selection");
    assert!(admits(true, true), "admitted entries, admissible selection");
    assert!(admits(false, false), "accepted entries, unfiltered selection");
}
