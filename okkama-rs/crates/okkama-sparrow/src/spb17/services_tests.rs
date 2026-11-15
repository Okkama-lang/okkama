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
    /// The same, with a direct plan instead of the executor.
    AdmitDirect,
    /// The entry is materialized as a token and absorbed again before the packet joins the scope.
    Materialized,
    /// The materialize names a Future of @caller, whose entry the returned packet does not hold.
    MaterializeOther,
    /// No call: a Future of @caller is absorbed into an empty packet that joins the scope.
    AbsorbOwn,
    /// The same, and the absorbed token is also a duty of the close's schedule.
    AbsorbAndSettle,
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
    let admitted = if matches!(variant, Caller::AbsorbOwn | Caller::AbsorbAndSettle) {
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
    let finished = b.close(state, None, None, MAIN, &duties, 0, variant != Caller::AdmitDirect);
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
        returned(Make::Rehomed, Caller::AbsorbOwn, Some(Dynamic::AfterStatic))];
    let mut used = std::collections::BTreeSet::new();
    for p in &programs {
        let bytes = encode(p).unwrap();
        assert_eq!(&decode(&bytes).unwrap(), p);
        let text = disassemble(p).unwrap();
        assert_eq!(&assemble(&text).unwrap(), p);
        assert_eq!(disassemble(&assemble(&text).unwrap()).unwrap(), text);
        for form in ["champ cleanup.settle.dynamic", "champ service.absorb"] {
            assert!(text.contains(form), "{form}");
        }
        used.extend(p.functions.iter().flat_map(|f| &f.code).filter_map(|instruction| match instruction {
            Instruction::Champ { op, .. } => Some(op.id()),
            _ => None,
        }));
    }
    for id in [69, 84, 85, 86] {
        assert!(used.contains(&id), "opcode {id} is not round-tripped");
    }
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
    reject_with(&returned(Make::Rehomed, Caller::AdmitDirect, Some(Dynamic::AfterStatic)), suspension::SCHEDULE,
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
