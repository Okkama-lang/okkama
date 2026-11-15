//! #601 R4: async with resources in SPB 17. The temporary take with its proof (IDs 67, 68), the rehome of an
//! escaping Future's cache duty (ID 69), the take capability as a frame slot kind, and their load rules:
//! `SPB_TAKE_CAPABILITY` (Champ draft 3 §§4.2, 15, with the `box.f` case of draft 2 §7.3) and `SPB_REHOME` (async
//! ABI C-4 and its ruling). Positive programs follow the low golden `r2-take-proof-recheck` of okkama-oir; each
//! negative is valid up to the rule it names.
use super::*;
use super::async_tests::{Body, FUTURE_INT, FUTURE_VOID, INNER, MAIN, WORK, CONSUME, arguments, find, l, p,
    owner_program, program, reject_with, s, service, slot_types, value, work};

use Argument as A;
use Opcode as O;
use ServiceKind as K;

/// What a @take variant does. @take creates a Future of @work and takes its outcome:
/// `async action take() { let x = await work(7); }` with a proved temporary take (OIR-2 §10.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Take {
    Plain,
    /// A second Future is created and proved; its capability is used for the first Future's take.
    OtherFuture,
    /// `box.f`: the Future is handed to the frame of another Future, @hold, which never awaits it. Another holder
    /// can observe it, so it has no take capability, whether or not anyone waits on it (draft 2 §7.3).
    Boxed,
    /// The same through a copy: the Future is moved to another register, and the copy is handed to @hold.
    CopyBoxed,
    /// A shared await of the Future comes before its proof: a second waiter with an outcome view.
    AwaitedFirst,
    /// The take's boundary is a scope of another function.
    ForeignBoundary,
    /// The Future is proved and taken again after its first take.
    TakenTwice,
    /// The Future is an input of the body, not a temporary this body created.
    Foreign,
    /// The Future is proved before an await of another Future, then taken: its capability lives across that
    /// await.
    ProvedBeforeAwait,
    /// Each branch of a condition proves the Future; the take holds the capability of either.
    ProvedOnBothBranches,
    /// The same, with one branch proving a second Future.
    ProvedOtherOnOneBranch,
    /// An empty packet waits in the frame across the take, in its own slot, and joins the scope afterwards.
    PacketAcross,
    /// The same packet lives across the take outside the frame.
    PacketUnsaved,
    /// The same packet is saved in two slots.
    PacketTwice,
}

/// @take's slots: 0 inputs, 1 owner state, 2 the cache duty, 3 the Future, 4–6 the unwind operands, 7 the take
/// capability, 8 a second cache duty, 9 a Future input, 10 and 11 a packet across the take.
fn take_slots() -> Vec<FrameSlot> {
    vec![service(K::Packet), service(K::ScopeState), service(K::CacheState), value(FUTURE_INT),
        value(Type::Failure), service(K::TaskState), service(K::Receipt), service(K::UniqueCache),
        service(K::CacheState), value(FUTURE_INT), service(K::Packet), service(K::Packet)]
}

/// A leaked block label, one per bracket.
fn label(name: &str, k: u32) -> &'static str {
    Box::leak(format!("{name}{k}").into_boxed_str())
}

/// The await.unwind adapter of continuation `k`, keeping `kept` (slots `kept_slots`), then the failure close of
/// @take's scope: packets join the scope, cache duties are settled, values are dropped, and the failure is
/// published.
#[allow(clippy::too_many_arguments)]
fn unwind_close(b: &mut Body, slots: &[FrameSlot], frame: Location, k: u32, state: Location, unwind: [Location; 3],
    kept: &[Location], kept_slots: &[u32])
{
    let saved = b.save(frame, k, kept);
    let [failure, task, receipt] = unwind;
    let [mut after, adapted] = [b.service(K::ScopeState), b.register(Type::Failure)];
    b.next(O::AwaitUnwind, vec![l(state), l(failure), l(task), l(receipt)], vec![after, adapted]);
    let (body, resumed) = b.resume(saved, k, &slot_types(slots, kept_slots));
    let mut duties = Vec::new();
    for (&location, &slot) in resumed.iter().zip(kept_slots) {
        match slots[slot as usize].ty {
            ProducedType::Service(K::Packet) => {
                after = b.emit(O::OwnAccept, vec![l(after), l(location)], K::ScopeState);
            }
            ProducedType::Service(K::CacheState) => duties.push(location),
            _ => {}
        }
    }
    let finished = b.close(after, None, Some(adapted), MAIN, &duties, 0, false);
    let [closed, combined] = [b.service(K::Closed), b.register(Type::Failure)];
    let failed = label("failed", k);
    b.dispatch(finished, [false, true, false], vec![(Tag::Failure, failed, vec![p(0), p(1)])]);
    b.block(failed, &[closed, combined]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(false), l(combined)], vec![]);
    b.ret();
}

/// What one suspension leaves: the frame and the values it reloaded on the normal path, and the normal
/// outcome's values.
struct Resumed {
    body: Location,
    values: Vec<Location>,
    outcome: Vec<Location>,
}

/// A bracketed `op` (await or take) with `operands` at the next continuation, saving `values` in `saved`. The
/// unwind path resumes, takes `carried` into the scope, adapts and closes; the normal path resumes and continues
/// after the unwind path's code.
#[allow(clippy::too_many_arguments)]
fn suspend(b: &mut Body, slots: &[FrameSlot], continuations: &mut Vec<Continuation>, body: Location, op: Opcode,
    operands: Vec<Argument>, operand_slots: &[u32], values: &[Location], saved: &[u32], normal: &[ProducedType],
    carried: Option<Location>) -> Resumed
{
    let k = continuations.len() as u32;
    continuations.push(Continuation { site: None, saved: saved.to_vec(), operands: operand_slots.to_vec() });
    continuations.push(Continuation { site: None, saved: saved.iter().copied().filter(|&slot| slot != 1).collect(),
        operands: vec![1, 4, 5, 6] });
    let frame = b.save(body, k, values);
    let [ok, bad] = [label("ok", k), label("bad", k)];
    let normal_slots = (0..normal.len() as u32).map(p).collect();
    b.outcomes(op, operands, vec![(Tag::Normal, ok, normal_slots), (Tag::Unwind, bad, vec![p(0), p(1), p(2)])]);
    let outcome = normal.iter().map(|ty| match *ty {
        ProducedType::Value(ty) => b.register(ty),
        ProducedType::Service(kind) => b.service(kind),
    }).collect::<Vec<_>>();
    b.block(ok, &outcome);
    let (resumed, reloaded) = b.resume(frame, k, &slot_types(slots, saved));
    let next = label("next", k);
    b.br(next, vec![]);
    let unwind = [b.register(Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block(bad, &unwind);
    let (failed_body, failed) = b.resume(frame, k, &slot_types(slots, saved));
    // The scope state is slot 1; every other saved value is a duty or a packet the failure close takes.
    let mut state = failed[saved.iter().position(|&slot| slot == 1).unwrap()];
    if let Some(packet) = carried {
        state = b.emit(O::OwnAccept, vec![l(state), l(packet)], K::ScopeState);
    }
    let kept = saved.iter().zip(&failed).filter(|(slot, _)| **slot != 1).collect::<Vec<_>>();
    let values = kept.iter().map(|(_, location)| **location).collect::<Vec<_>>();
    let kept_slots = kept.iter().map(|(slot, _)| **slot).collect::<Vec<_>>();
    unwind_close(b, slots, failed_body, k + 1, state, unwind, &values, &kept_slots);
    b.block(next, &[]);
    Resumed { body: resumed, values: reloaded, outcome }
}

/// The normal outcome of a take of @work's Future: its int, the outcome packet and its deps.
fn take_normal() -> Vec<ProducedType> {
    vec![ProducedType::Value(Type::Int), ProducedType::Service(K::Packet), ProducedType::Service(K::Deps)]
}

/// @hold: an async body that receives a Future and never awaits it (`box.f`'s box).
fn hold() -> (Function, FrameLayout) {
    let (mut b, frame) = Body::asynchronous("hold", 2, vec![]);
    b.block("entry", &[frame]);
    let (body, start) = b.resume(frame, 0, &[ProducedType::Value(FUTURE_INT), ProducedType::Service(K::Packet)]);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(CONSUME), A::Boundary(CONSUME)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(start[1])], K::ScopeState);
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let pending = b.emit(O::ExitPrepare, vec![A::Kind(ExitKind::Return), A::Boundary(CONSUME), l(empty),
        A::Selection(selection)], K::Pending);
    let finished = b.close(q1, Some(pending), None, CONSUME, &[], 0, true);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, "done", vec![p(0), p(1)])]);
    b.block("done", &[closed, reserved]);
    let [packet, deps] = [b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(ExitDestination::Caller)], vec![packet, deps]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(packet), l(deps)], vec![]);
    b.ret();
    let layout = FrameLayout {
        slots: vec![value(FUTURE_INT), service(K::Packet)],
        continuations: vec![Continuation { site: None, saved: vec![0, 1], operands: vec![] }],
    };
    (b.finish(), layout)
}

fn taker(variant: Take) -> (Function, FrameLayout) {
    let slots = take_slots();
    let (mut b, frame) = Body::asynchronous("take", 1, vec![]);
    let foreign = variant == Take::Foreign;
    let start_saved = if foreign { vec![9, 0] } else { vec![0] };
    let mut continuations = vec![Continuation { site: None, saved: start_saved.clone(), operands: vec![] }];
    b.block("entry", &[frame]);
    let (body, start) = b.resume(frame, 0, &slot_types(&slots, &start_saved));
    let inputs = *start.last().unwrap();
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(MAIN), A::Boundary(MAIN)], K::ScopeState);
    let mut state = b.emit(O::OwnAccept, vec![l(q0), l(inputs)], K::ScopeState);
    let seven = b.constant(Constant::Int("7".into()), Type::Int);
    let external = b.emit(O::DepsOf, vec![l(seven)], K::Deps);
    b.f.proofs.dependencies.push(Dependency { value: external, obligations: vec![seven] });
    let create = |b: &mut Body, state: &mut Location, entry: u32, argument: Location, ty: Type| {
        let offered = b.emit(O::OwnEmpty, vec![], K::Packet);
        let [future, cache, next] = [b.register(ty), b.service(K::CacheState), b.service(K::ScopeState)];
        let mut operands = vec![A::Entry(entry), A::Scope(MAIN), A::Result(Some(ty)), l(argument), l(offered)];
        if entry == 0 {
            operands.push(l(external));
        }
        operands.push(l(*state));
        b.next(O::FutureNew, operands, vec![future, cache, next]);
        *state = next;
        (future, cache)
    };
    // The awaited Future and its cache duty; a foreign Future has no duty here.
    let (mut future, mut duties, mut duty_slots) = if foreign {
        (start[0], vec![], vec![])
    } else {
        let (future, cache) = create(&mut b, &mut state, 0, seven, FUTURE_INT);
        (future, vec![cache], vec![2])
    };
    let mut proved = future;
    let mut other = None;
    match variant {
        Take::OtherFuture | Take::ProvedBeforeAwait | Take::ProvedOtherOnOneBranch => {
            let (second, cache) = create(&mut b, &mut state, 0, seven, FUTURE_INT);
            if variant == Take::OtherFuture {
                proved = second;
            }
            other = Some(second);
            duties.push(cache);
            duty_slots.push(8);
        }
        Take::Boxed | Take::CopyBoxed => {
            let held = if variant == Take::CopyBoxed {
                let copy = b.register(FUTURE_INT);
                let (Location::Register(from), Location::Register(to)) = (future, copy) else { unreachable!() };
                b.f.code.push(Instruction::Data { op: DataOp::Move, inputs: vec![from], output: Some(to),
                    immediate: Immediate::None });
                copy
            } else {
                future
            };
            let (_, cache) = create(&mut b, &mut state, 2, held, FUTURE_VOID);
            duties.push(cache);
            duty_slots.push(8);
        }
        _ => {}
    }
    // Observations of the Future publish nothing.
    let done = b.register(Type::Bool);
    b.next(O::FutureDone, vec![l(future)], vec![done]);
    let mut body = body;
    let early = (variant == Take::ProvedBeforeAwait)
        .then(|| b.emit(O::UniqueCacheProof, vec![l(proved)], K::UniqueCache));
    if matches!(variant, Take::AwaitedFirst | Take::ProvedBeforeAwait) {
        let awaited = other.unwrap_or(future);
        let mut values = vec![state];
        values.extend(&duties);
        let mut saved = vec![1];
        saved.extend(&duty_slots);
        let kept = awaited != future;
        if kept {
            // The Future to take waits in the frame across the other await.
            values.push(future);
            saved.push(9);
        }
        let seen = suspend(&mut b, &slots, &mut continuations, body, O::Await, vec![l(awaited)], &[3], &values,
            &saved, &[ProducedType::Value(Type::Int), ProducedType::Service(K::Deps)], None);
        body = seen.body;
        state = seen.values[0];
        duties = seen.values[1..1 + duties.len()].to_vec();
        if kept {
            future = *seen.values.last().unwrap();
        }
    }
    let packet_across = matches!(variant, Take::PacketAcross | Take::PacketUnsaved | Take::PacketTwice);
    let extra = packet_across.then(|| b.emit(O::OwnEmpty, vec![], K::Packet));
    let boundary = if variant == Take::ForeignBoundary { WORK } else { MAIN };
    let rounds = if variant == Take::TakenTwice { 2 } else { 1 };
    for round in 0..rounds {
        let capability = match early {
            Some(capability) if round == 0 => capability,
            _ if matches!(variant, Take::ProvedOnBothBranches | Take::ProvedOtherOnOneBranch) => {
                let joined = b.service(K::UniqueCache);
                b.cond(done, "proof_yes", "proof_no");
                b.block("proof_yes", &[]);
                let yes = b.emit(O::UniqueCacheProof, vec![l(proved)], K::UniqueCache);
                b.br("proved", vec![s(yes)]);
                b.block("proof_no", &[]);
                let second = other.unwrap_or(proved);
                let no = b.emit(O::UniqueCacheProof, vec![l(second)], K::UniqueCache);
                b.br("proved", vec![s(no)]);
                b.block("proved", &[joined]);
                joined
            }
            _ => b.emit(O::UniqueCacheProof, vec![l(proved)], K::UniqueCache),
        };
        let mut values = vec![state];
        values.extend(&duties);
        let mut saved = vec![1];
        saved.extend(&duty_slots);
        match variant {
            Take::PacketAcross => {
                values.push(extra.unwrap());
                saved.push(10);
            }
            Take::PacketTwice => {
                values.extend([extra.unwrap(), extra.unwrap()]);
                saved.extend([10, 11]);
            }
            _ => {}
        }
        let carried = extra.filter(|_| variant == Take::PacketUnsaved);
        let taken = suspend(&mut b, &slots, &mut continuations, body, O::AwaitTake,
            vec![l(future), l(capability), A::Scope(boundary)], &[3, 7], &values, &saved, &take_normal(), carried);
        body = taken.body;
        state = b.emit(O::OwnAccept, vec![l(taken.values[0]), l(taken.outcome[1])], K::ScopeState);
        duties = taken.values[1..1 + duties.len()].to_vec();
        if variant == Take::PacketAcross {
            state = b.emit(O::OwnAccept, vec![l(state), l(*taken.values.last().unwrap())], K::ScopeState);
        }
    }
    if variant == Take::PacketUnsaved {
        state = b.emit(O::OwnAccept, vec![l(state), l(extra.unwrap())], K::ScopeState);
    }
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let pending = b.emit(O::ExitPrepare, vec![A::Kind(ExitKind::Return), A::Boundary(MAIN), l(empty),
        A::Selection(selection)], K::Pending);
    // The taken packet joined the scope: its inventory is the outcome's, so the close runs the executor.
    let finished = b.close(state, Some(pending), None, MAIN, &duties, 0, true);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, "done", vec![p(0), p(1)])]);
    b.block("done", &[closed, reserved]);
    let [packet, deps] = [b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(ExitDestination::Caller)], vec![packet, deps]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(packet), l(deps)], vec![]);
    b.ret();
    for (k, continuation) in continuations.iter_mut().enumerate().skip(1) {
        continuation.site = Some(b.sites[&(k as u32)]);
    }
    // Two negatives leave a linear token behind on some path; their proof maps stay empty, since the rule they
    // break is checked before the maps.
    let unchecked = matches!(variant, Take::PacketTwice | Take::ProvedBeforeAwait);
    let f = if unchecked { b.finish_unchecked() } else { b.finish() };
    (f, FrameLayout { slots, continuations })
}

fn taking(variant: Take) -> Program {
    let (work, work_layout) = work();
    let (take, take_layout) = taker(variant);
    let (hold, hold_layout) = hold();
    program(vec![work, take, hold], vec![work_layout, take_layout, hold_layout], None)
}

fn take_of(p: &mut Program) -> &mut Function {
    &mut p.functions[1]
}

// --- Registry, schemas and codec ------------------------------------------------------------------------------

#[test]
fn take_proof_and_rehome_schemas_follow_draft3() {
    use ProducedType::{Service as Sv, Value};
    let valued = ResultContext { value: Some(Type::Int), ..ResultContext::default() };
    let void = ResultContext::default();
    assert_eq!(O::AwaitTake.tags(&void), [Tag::Normal, Tag::Unwind]);
    assert!(O::AwaitTake.is_terminator() && O::AwaitTake.suspends() && O::AwaitTake.frame_bound());
    // Normal: the value, the outcome packet and its deps; unwind as for a shared await (draft 3 §14.4, ID 67).
    assert_eq!(O::AwaitTake.results(Tag::Normal, &valued).unwrap(),
        [Value(Type::Int), Sv(K::Packet), Sv(K::Deps)]);
    assert_eq!(O::AwaitTake.results(Tag::Normal, &void).unwrap(), [Sv(K::Packet), Sv(K::Deps)]);
    assert_eq!(O::AwaitTake.results(Tag::Unwind, &void).unwrap(),
        [Value(Type::Failure), Sv(K::TaskState), Sv(K::Receipt)]);
    // The proof produces the capability only; the rehome continues the pending exit (class L, no unwind).
    assert_eq!(O::UniqueCacheProof.results(Tag::Normal, &void).unwrap(), [Sv(K::UniqueCache)]);
    assert_eq!(O::CacheRehome.results(Tag::Normal, &void).unwrap(), [Sv(K::Pending)]);
    for op in [O::UniqueCacheProof, O::CacheRehome] {
        assert_eq!(op.tags(&void), [Tag::Normal], "{}", op.name());
        assert!(op.results(Tag::Unwind, &void).is_err(), "{}", op.name());
        assert!(!op.is_terminator() && !op.suspends() && !op.frame_bound(), "{}", op.name());
    }
    // The capability is a linear proof position of zero words (draft 3 §14.2).
    let words = layout::service_layout(K::UniqueCache);
    assert_eq!((words.words, words.references), (0, &[][..]));
    assert!(K::UniqueCache.linear());
    // Their helpers came with the runtime (R5): the take is class S, the rehome class L; the proof has none.
    let take = O::AwaitTake.helper().unwrap();
    assert_eq!((take.name, take.effects & HELPER_SUSPENDS != 0), ("champ_await_take", true));
    let rehome = O::CacheRehome.helper().unwrap();
    assert_eq!((rehome.name, rehome.effects & (HELPER_SUSPENDS | HELPER_UNWINDS)), ("champ_cache_rehome", 0));
    assert_eq!(O::UniqueCacheProof.helper(), None);
}

#[test]
fn take_proof_and_rehome_round_trip() {
    let programs = [taking(Take::Plain), escaping(Escape::Plain)];
    let mut used = std::collections::BTreeSet::new();
    for p in &programs {
        let bytes = encode(p).unwrap();
        assert_eq!(&decode(&bytes).unwrap(), p);
        let text = disassemble(p).unwrap();
        assert_eq!(&assemble(&text).unwrap(), p);
        assert_eq!(disassemble(&assemble(&text).unwrap()).unwrap(), text);
        used.extend(p.functions.iter().flat_map(|f| &f.code).filter_map(|instruction| match instruction {
            Instruction::Champ { op, .. } => Some(op.id()),
            _ => None,
        }));
    }
    for id in [67, 68, 69] {
        assert!(used.contains(&id), "opcode {id} is not round-tripped");
    }
    let text = disassemble(&programs[0]).unwrap();
    for form in ["champ await.take", "champ unique_cache.proof"] {
        assert!(text.contains(form), "{form}");
    }
    assert!(disassemble(&programs[1]).unwrap().contains("champ cache.rehome"));
}

// --- SPB_TAKE_CAPABILITY --------------------------------------------------------------------------------------

#[test]
fn a_proved_temporary_take_loads() {
    verify_load(&taking(Take::Plain), &NoExternalContracts).unwrap();
}

#[test]
fn a_take_needs_the_capability_of_its_own_future() {
    reject_with(&taking(Take::OtherFuture), take::TAKE_CAPABILITY, "takes the Future its proof names");
    reject_with(&taking(Take::ProvedOtherOnOneBranch), take::TAKE_CAPABILITY, "takes the Future its proof names");
}

/// One proof on each path, both of the taken Future: every capability the take can hold is its own.
#[test]
fn proofs_of_the_future_on_each_path_reach_its_take() {
    verify_load(&taking(Take::ProvedOnBothBranches), &NoExternalContracts).unwrap();
}

/// Champ draft 2 §7.3: F is kept in an accessible field `box.f` (here the frame of @hold); one waiter, no
/// outcome views. F has no capability, and that does not change without the waiter: the alias remains.
#[test]
fn box_f_has_no_take_capability() {
    reject_with(&taking(Take::Boxed), take::TAKE_CAPABILITY, "another holder can observe");
    reject_with(&taking(Take::CopyBoxed), take::TAKE_CAPABILITY, "another holder can observe");
}

#[test]
fn a_shared_await_before_the_proof_forbids_the_take() {
    reject_with(&taking(Take::AwaitedFirst), take::TAKE_CAPABILITY, "another holder can observe");
}

#[test]
fn a_take_extracts_into_a_scope_of_its_own_body() {
    reject_with(&taking(Take::ForeignBoundary), take::TAKE_CAPABILITY, "scope of the awaiting body");
}

#[test]
fn a_taken_cache_is_not_taken_again() {
    reject_with(&taking(Take::TakenTwice), take::TAKE_CAPABILITY, "proved or taken again");
}

#[test]
fn only_a_temporary_of_the_body_is_taken() {
    reject_with(&taking(Take::Foreign), take::TAKE_CAPABILITY, "a temporary that this body created");
}

/// No suspension separates a proof from its take: the capability is linear and waits in the frame only as its
/// take's operand, so across another await it lives outside the frame.
#[test]
fn a_suspension_between_proof_and_take_forbids_the_take() {
    reject_with(&taking(Take::ProvedBeforeAwait), suspension::FRAME_LINEAR, "outside the frame");
}

#[test]
fn the_take_capability_waits_only_as_its_take_operand() {
    let mut p = taking(Take::Plain);
    p.tables.frames[1].continuations[0].saved.push(7);
    reject_with(&p, take::TAKE_CAPABILITY, "crosses a suspension other than its take");
}

#[test]
fn a_take_capability_is_not_an_entry_argument() {
    // @make, a synchronous owner; an async body takes exactly its frame (SPB_ASYNC_ENTRY).
    let mut p = owner_program(true);
    let f = &mut p.functions[3];
    let token = Location::Service(f.services.len() as u32);
    f.services.push(K::UniqueCache);
    f.params.push(token);
    f.blocks[0].params.push(token);
    reject_with(&p, "ENTRY_PRIVATE_SERVICE", "private service");
}

#[test]
fn a_take_reads_a_future_and_consumes_a_capability() {
    let mut p = taking(Take::Plain);
    let f = take_of(&mut p);
    let take = find(f, O::AwaitTake);
    arguments(f, take).swap(0, 1);
    assert!(verify_load(&p, &NoExternalContracts).is_err());
    let mut p = taking(Take::Plain);
    let f = take_of(&mut p);
    let take = find(f, O::AwaitTake);
    arguments(f, take).pop();
    assert!(verify_load(&p, &NoExternalContracts).unwrap_err().message.contains("invalid operands for await.take"));
}

// --- Packets across a suspension --------------------------------------------------------------------------------

#[test]
fn a_packet_crosses_the_take_in_one_frame_slot() {
    verify_load(&taking(Take::PacketAcross), &NoExternalContracts).unwrap();
    reject_with(&taking(Take::PacketUnsaved), suspension::FRAME_LINEAR, "outside the frame");
    reject_with(&taking(Take::PacketTwice), suspension::FRAME_LINEAR, "saved twice");
}

// --- SPB_REHOME --------------------------------------------------------------------------------------------------

/// What an @escape variant does. `async action escape() { let f = { work(7) }; }`: the Future is created in the
/// inner block (scope INNER) and leaves it as the block's value, so its cache duty is rehomed into INNER's exit,
/// for MAIN (C-4 and its ruling).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Escape {
    Plain,
    /// The rehome names the caller while the exit commits into MAIN.
    WrongTarget,
    /// The Future belongs to MAIN, yet its duty is rehomed into INNER's exit.
    OuterOwner,
    /// The duty is rehomed after INNER's close, right before the commit.
    AfterClose,
    /// Duties of INNER and of MAIN go into one reserve.
    TwoOwners,
    /// The exit is replaced after the rehome.
    Replaced,
}

fn escaper(variant: Escape) -> (Function, FrameLayout) {
    let (mut b, frame) = Body::asynchronous("escape", 1, vec![]);
    b.block("entry", &[frame]);
    let (body, start) = b.resume(frame, 0, &[ProducedType::Service(K::Packet)]);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(MAIN), A::Boundary(MAIN)], K::ScopeState);
    let mut outer = b.emit(O::OwnAccept, vec![l(q0), l(start[0])], K::ScopeState);
    let mut inner = b.emit(O::ScopeBegin, vec![A::Scope(INNER), A::Boundary(INNER)], K::ScopeState);
    let seven = b.constant(Constant::Int("7".into()), Type::Int);
    let external = b.emit(O::DepsOf, vec![l(seven)], K::Deps);
    b.f.proofs.dependencies.push(Dependency { value: external, obligations: vec![seven] });
    let create = |b: &mut Body, state: &mut Location, scope: u32| {
        let offered = b.emit(O::OwnEmpty, vec![], K::Packet);
        let [future, cache, next] = [b.register(FUTURE_INT), b.service(K::CacheState), b.service(K::ScopeState)];
        b.next(O::FutureNew, vec![A::Entry(0), A::Scope(scope), A::Result(Some(FUTURE_INT)), l(seven), l(offered),
            l(external), l(*state)], vec![future, cache, next]);
        *state = next;
        (future, cache)
    };
    let (future, cache) = if variant == Escape::OuterOwner {
        create(&mut b, &mut outer, MAIN)
    } else {
        create(&mut b, &mut inner, INNER)
    };
    let second = (variant == Escape::TwoOwners).then(|| create(&mut b, &mut outer, MAIN).1);
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let mut pending = b.emit(O::ExitPrepare, vec![A::Kind(ExitKind::Fallthrough), A::Boundary(INNER), l(future),
        l(empty), A::Selection(selection)], K::Pending);
    let target = if variant == Escape::WrongTarget { ExitDestination::Caller } else { ExitDestination::Scope(MAIN) };
    let rehome = |b: &mut Body, pending: Location, duty: Location| {
        b.emit(O::CacheRehome, vec![A::Destination(target), l(pending), l(duty)], K::Pending)
    };
    if variant != Escape::AfterClose {
        pending = rehome(&mut b, pending, cache);
    }
    if let Some(duty) = second {
        pending = rehome(&mut b, pending, duty);
    }
    if variant == Escape::Replaced {
        let replacement = b.emit(O::OwnEmpty, vec![], K::Packet);
        let selection = b.selection();
        let [replaced, old] = [b.service(K::Pending), b.service(K::Packet)];
        b.next(O::ExitReplace, vec![l(pending), A::Kind(ExitKind::Fallthrough), A::Boundary(INNER), l(future),
            l(replacement), A::Selection(selection)], vec![replaced, old]);
        pending = replaced;
        outer = b.emit(O::OwnAccept, vec![l(outer), l(old)], K::ScopeState);
    }
    // The reserve holds a duty, so INNER's close runs the executor (the reserve is never proved empty).
    let finished = b.close(inner, Some(pending), None, INNER, &[], 0, true);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, "joined", vec![p(0), p(1)])]);
    b.block("joined", &[closed, reserved]);
    let reserved = if variant == Escape::AfterClose { rehome(&mut b, reserved, cache) } else { reserved };
    let [escaped, packet, deps] = [b.register(FUTURE_INT), b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(ExitDestination::Scope(MAIN)),
        A::Result(Some(FUTURE_INT))], vec![escaped, packet, deps]);
    let joined = b.emit(O::OwnAccept, vec![l(outer), l(packet)], K::ScopeState);
    let observed = b.register(Type::Bool);
    b.next(O::FutureDone, vec![l(escaped)], vec![observed]);
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let pending = b.emit(O::ExitPrepare, vec![A::Kind(ExitKind::Return), A::Boundary(MAIN), l(empty),
        A::Selection(selection)], K::Pending);
    let finished = b.close(joined, Some(pending), None, MAIN, &[], 0, true);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, "done", vec![p(0), p(1)])]);
    b.block("done", &[closed, reserved]);
    let [packet, deps] = [b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(ExitDestination::Caller)], vec![packet, deps]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(packet), l(deps)], vec![]);
    b.ret();
    let layout = FrameLayout {
        slots: vec![service(K::Packet)],
        continuations: vec![Continuation { site: None, saved: vec![0], operands: vec![] }],
    };
    (b.finish(), layout)
}

fn escaping(variant: Escape) -> Program {
    let (work, work_layout) = work();
    let (escape, escape_layout) = escaper(variant);
    let mut p = program(vec![work, escape], vec![work_layout, escape_layout], None);
    // exit.replace carries static selection evidence.
    p.functions[1].proofs.static_selections = static_selections(&p, &p.functions[1]).unwrap();
    p
}

#[test]
fn a_rehomed_future_leaves_its_owner_for_the_exit_destination() {
    verify_load(&escaping(Escape::Plain), &NoExternalContracts).unwrap();
}

#[test]
fn a_rehome_names_the_destination_of_its_exit() {
    reject_with(&escaping(Escape::WrongTarget), automaton::REHOME, "destination of the exit");
}

#[test]
fn a_rehome_moves_a_duty_of_the_closing_scope() {
    reject_with(&escaping(Escape::OuterOwner), automaton::REHOME, "scope that owns them");
}

#[test]
fn a_rehome_comes_before_its_owner_closes() {
    reject_with(&escaping(Escape::AfterClose), automaton::REHOME, "without their owner's close");
}

#[test]
fn one_reserve_takes_the_duties_of_one_owner() {
    reject_with(&escaping(Escape::TwoOwners), automaton::REHOME, "one owner");
}

#[test]
fn an_exit_with_rehomed_duties_is_not_replaced() {
    reject_with(&escaping(Escape::Replaced), automaton::REHOME, "exit.replace");
}

#[test]
fn a_rehome_takes_a_pending_exit_and_a_cache_duty() {
    let mut p = escaping(Escape::Plain);
    let f = &mut p.functions[1];
    let rehome = find(f, O::CacheRehome);
    arguments(f, rehome).swap(1, 2);
    assert!(verify_load(&p, &NoExternalContracts).unwrap_err().message.contains("wrong service operand type"));
}
