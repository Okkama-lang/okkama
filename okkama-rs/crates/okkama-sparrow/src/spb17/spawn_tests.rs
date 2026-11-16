//! #601 P1: `fut.spawn` (SPB 65) at the load gate (Champ draft 3 §15; OIR-2 async ABI D24). A spawner body creates
//! a Future of @work, spawns it, and either never awaits it or awaits it once; its close settles the spawned task's
//! duty, then the cache duty. Each negative is valid up to the rule it names.
use super::*;
use super::async_tests::{find, l, p, program, reject_with, service, slot_types, value, work, Body, FUTURE_INT,
    MAIN};

use Argument as A;
use Opcode as O;
use ServiceKind as K;

/// What a spawner variant does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Spawner {
    /// `spawn work(7);` and return: the close settles the task duty, then the cache duty.
    Detached,
    /// The close settles the cache duty before the task duty.
    CacheFirst,
    /// A branch separates the creation from the spawn.
    Branched,
    /// `let f = spawn work(7); await f;`: both duties cross the await in the frame.
    Awaited,
    /// As `Awaited`, but the await's adapter takes the spawned task's duty instead of the await's task.
    UnwindTakesDuty,
    /// `fut.spawn.bound %f` (ruling PR-E): no duty goes in or comes out; the close settles the cache duty alone.
    Bound,
    /// The bound form naming the cache duty as well.
    BoundWithService,
    /// The bound form of a register that is not a Future.
    BoundOfInt,
}

fn spawner(variant: Spawner) -> (Function, FrameLayout) {
    // Slots: 0 inputs, 1 owner state, 2 cache duty, 3 task duty, 4 the Future, 5–7 the unwind operands.
    let slots = vec![service(K::Packet), service(K::ScopeState), service(K::CacheState), service(K::TaskState),
        value(FUTURE_INT), value(Type::Failure), service(K::TaskState), service(K::Receipt)];
    let (mut b, frame) = Body::asynchronous("spawner", 1, vec![]);
    b.block("entry", &[frame]);
    let (body, start) = b.resume(frame, 0, &[ProducedType::Service(K::Packet)]);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(MAIN), A::Boundary(MAIN)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(start[0])], K::ScopeState);
    let seven = b.constant(Constant::Int("7".into()), Type::Int);
    let offered = b.emit(O::OwnEmpty, vec![], K::Packet);
    let [future, cache, q2] = [b.register(FUTURE_INT), b.service(K::CacheState), b.service(K::ScopeState)];
    b.next(O::FutureNew, vec![A::Entry(0), A::Scope(MAIN), A::Result(Some(FUTURE_INT)), l(seven), l(offered),
        l(q1)], vec![future, cache, q2]);
    // An observation of the Future may come between its creation and its spawn (D24.1).
    let deps = b.emit(O::DepsOf, vec![l(future)], K::Deps);
    b.f.proofs.dependencies.push(Dependency { value: deps, obligations: vec![future] });
    if variant == Spawner::Branched {
        b.br("spawn", vec![]);
        b.block("spawn", &[]);
    }
    let bound = match variant {
        Spawner::Bound => Some(vec![l(future)]),
        Spawner::BoundWithService => Some(vec![l(future), l(cache)]),
        Spawner::BoundOfInt => Some(vec![l(seven)]),
        _ => None,
    };
    let [kept, task] = match bound.clone() {
        Some(operands) => {
            b.next(O::FutureSpawnBound, operands, vec![]);
            [cache, cache]
        }
        None => {
            let [kept, task] = [b.service(K::CacheState), b.service(K::TaskState)];
            b.next(O::FutureSpawn, vec![l(future), l(cache)], vec![kept, task]);
            [kept, task]
        }
    };
    let _ = deps;
    let mut continuations = vec![Continuation { site: None, saved: vec![0], operands: vec![] }];
    let (body, state, duties) = if matches!(variant, Spawner::Awaited | Spawner::UnwindTakesDuty) {
        let first = b.save(body, 1, &[q2, kept, task]);
        b.outcomes(O::Await, vec![l(future)], vec![(Tag::Normal, "ok", vec![p(0), p(1)]),
            (Tag::Unwind, "bad", vec![p(0), p(1), p(2)])]);
        continuations.push(Continuation { site: None, saved: vec![1, 2, 3], operands: vec![4] });
        let unwind = [b.register(Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
        b.block("bad", &unwind);
        let (resumed_body, resumed) = b.resume(first, 1, &slot_types(&slots, &[1, 2, 3]));
        let [q3, c3, t3] = resumed[..] else { unreachable!() };
        // The adapter (C-3) with both duties kept in the frame across it; the variant hands it the spawned task's
        // duty instead of the await's task, which it leaves unused.
        let taken = variant == Spawner::UnwindTakesDuty;
        let (adapted_task, kept) = if taken { (t3, vec![2]) } else { (unwind[1], vec![2, 3]) };
        let values = if taken { vec![c3] } else { vec![c3, t3] };
        let saved = b.save(resumed_body, 2, &values);
        let [q4, failure] = [b.service(K::ScopeState), b.register(Type::Failure)];
        b.next(O::AwaitUnwind, vec![l(q3), l(unwind[0]), l(adapted_task), l(unwind[2])], vec![q4, failure]);
        continuations.push(Continuation { site: None, saved: kept.clone(), operands: vec![1, 5, 6, 7] });
        let (failed_body, resumed) = b.resume(saved, 2, &slot_types(&slots, &kept));
        let duties: Vec<_> = resumed.iter().rev().copied().collect();
        let finished = b.close(q4, None, Some(failure), MAIN, &duties, 0, false);
        let [closed, combined] = [b.service(K::Closed), b.register(Type::Failure)];
        b.dispatch(finished, [false, true, false], vec![(Tag::Failure, "failed", vec![p(0), p(1)])]);
        b.block("failed", &[closed, combined]);
        b.next(O::FuturePublish, vec![l(failed_body), A::Flag(false), l(combined)], vec![]);
        b.ret();
        let [x, xd] = [b.register(Type::Int), b.service(K::Deps)];
        b.block("ok", &[x, xd]);
        let (resumed_body, resumed) = b.resume(first, 1, &slot_types(&slots, &[1, 2, 3]));
        (resumed_body, resumed[0], vec![resumed[2], resumed[1]])
    } else if variant == Spawner::CacheFirst {
        (body, q2, vec![kept, task])
    } else if bound.is_some() {
        (body, q2, vec![kept])
    } else {
        (body, q2, vec![task, kept])
    };
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let pending = b.emit(O::ExitPrepare, vec![A::Kind(ExitKind::Return), A::Boundary(MAIN), l(empty),
        A::Selection(selection)], K::Pending);
    let finished = b.close(state, Some(pending), None, MAIN, &duties, 0, false);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, "done", vec![p(0), p(1)])]);
    b.block("done", &[closed, reserved]);
    let [packet, out] = [b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(ExitDestination::Caller)], vec![packet, out]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(packet), l(out)], vec![]);
    b.ret();
    for (k, continuation) in continuations.iter_mut().enumerate().skip(1) {
        continuation.site = Some(b.sites[&(k as u32)]);
    }
    // The adapter that takes the duty leaves the await's task live: only the edges resolve, no lifetimes exist.
    let function = if variant == Spawner::UnwindTakesDuty { b.finish_unchecked() } else { b.finish() };
    (function, FrameLayout { slots, continuations })
}

fn spawner_program(variant: Spawner) -> Program {
    let (work, work_layout) = work();
    let (spawner, spawner_layout) = spawner(variant);
    program(vec![work, spawner], vec![work_layout, spawner_layout], None)
}

#[test]
fn a_spawned_task_and_its_cache_settle_at_their_owner() {
    // D24.1, D24.2: the spawn takes the fresh cache duty; the close settles the task duty, then the cache duty.
    verify_load(&spawner_program(Spawner::Detached), &NoExternalContracts).unwrap();
}

#[test]
fn a_spawned_task_crosses_an_await_in_the_frame() {
    // Both duties are saved across the await and its adapter; the adapter takes the await's own task.
    verify_load(&spawner_program(Spawner::Awaited), &NoExternalContracts).unwrap();
}

// --- SPB_SPAWN ------------------------------------------------------------------------------------------------

#[test]
fn spawn_takes_the_duty_its_fut_new_just_gave() {
    reject_with(&spawner_program(Spawner::Branched), suspension::SPAWN, "just gave");
}

// --- SPB_SCHEDULE ---------------------------------------------------------------------------------------------

#[test]
fn a_spawned_task_settles_before_its_cache() {
    reject_with(&spawner_program(Spawner::CacheFirst), suspension::SCHEDULE, "task duty is settled before");
}

// --- SPB_TASK_RECEIPT -----------------------------------------------------------------------------------------

#[test]
fn a_spawned_task_never_reaches_an_await_adapter() {
    reject_with(&spawner_program(Spawner::UnwindTakesDuty), suspension::TASK_RECEIPT, "spawned task's duty reaches");
}

#[test]
fn spawn_forwards_the_cache_duty_and_adds_the_task_duty() {
    let p = spawner_program(Spawner::Detached);
    let f = &p.functions[1];
    let spawn = find(f, O::FutureSpawn);
    let Instruction::Champ { arguments, flow: Flow::Next(outputs), .. } = &f.code[spawn] else { unreachable!() };
    assert_eq!(O::FutureSpawn.id(), 65);
    assert_eq!(O::FutureSpawn.helper_class(), Some(HelperClass::X));
    assert!(!O::FutureSpawn.suspends() && !O::FutureSpawn.frame_bound());
    let kinds: Vec<_> = outputs.iter().map(|&location| verify::loc_type(f, location).unwrap()).collect();
    assert_eq!(kinds, [ProducedType::Service(K::CacheState), ProducedType::Service(K::TaskState)]);
    assert_eq!(load::consumed(f, O::FutureSpawn, arguments), vec![match arguments[1] {
        A::Location(location) => location,
        _ => unreachable!(),
    }]);
    // A round trip through the codec keeps the opcode.
    assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
}

// --- fut.spawn.bound (ruling PR-E, #601 F1) -------------------------------------------------------------------

#[test]
fn a_bound_spawn_moves_no_duty_and_the_cache_settles_at_its_owner() {
    // The Future stays the cache duty's only holder; the close settles that duty, and there is no task duty.
    let p = spawner_program(Spawner::Bound);
    verify_load(&p, &NoExternalContracts).unwrap();
    let f = &p.functions[1];
    let spawn = find(f, O::FutureSpawnBound);
    let Instruction::Champ { arguments, flow: Flow::Next(outputs), .. } = &f.code[spawn] else { unreachable!() };
    assert!(outputs.is_empty());
    assert!(load::consumed(f, O::FutureSpawnBound, arguments).is_empty());
    assert_eq!(O::FutureSpawnBound.id(), 87);
    assert_eq!(O::FutureSpawnBound.helper_class(), Some(HelperClass::X));
    assert!(!O::FutureSpawnBound.suspends() && !O::FutureSpawnBound.frame_bound());
    assert_eq!(O::FutureSpawnBound.helper().map(|helper| helper.name), Some("champ_future_spawn_bound"));
    // A round trip through the codec keeps the opcode.
    assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
}

// #791 N1: `verify` alone types opcode 87, with the shape `l` and the Future operand of `fut.cancel`; the rule of
// `SPB_SPAWN` for the bound form is then the operand check (ruling PR-E: the bound form names no service).

#[test]
fn a_bound_spawn_names_no_service() {
    let p = spawner_program(Spawner::BoundWithService);
    let failure = verify(&p).unwrap_err();
    assert_eq!(failure.rule, "SPB_STRUCTURE", "{failure}");
    assert!(failure.message.contains("invalid operands for fut.spawn.bound"), "{failure}");
    reject_with(&p, "SPB_STRUCTURE", "invalid operands for fut.spawn.bound");
}

#[test]
fn a_bound_spawn_takes_a_future() {
    let p = spawner_program(Spawner::BoundOfInt);
    let failure = verify(&p).unwrap_err();
    assert_eq!(failure.rule, verify::FUTURE_TYPES, "{failure}");
    assert!(failure.message.contains("fut.spawn.bound needs a Future"), "{failure}");
    reject_with(&p, verify::FUTURE_TYPES, "fut.spawn.bound needs a Future");
}

#[test]
fn verify_alone_admits_a_bound_spawn_of_a_future() {
    verify(&spawner_program(Spawner::Bound)).unwrap();
}
