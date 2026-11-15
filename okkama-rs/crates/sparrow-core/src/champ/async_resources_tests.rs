//! #601 R5: the Champ runtime with resources across async bodies (Champ draft 3 §§2.2, 4.2, 6, 11–13; ABI C-4,
//! C-5, D25; plan §3.6).
//!
//! The bodies are SPB 17 programs run, as in the parent module, by the JIT and by the reference body, with the
//! collector idle and with a collection at every safepoint. Steps that SPB 17 cannot encode yet (`await.take`,
//! `cache.rehome`, `cleanup.settle.dynamic`, `service.*` and the async defer entry, R4) are driven by the test
//! through their helpers, with the operands a caller's stack map would root.
//!
//! Expectations come from the drafts: the E_pub trace of draft 2 §7.2 in its C-5 form (the failure comes from the
//! last destroy of the body after its return was reserved), and both rehome traces of draft 2 §7.6 as draft 3 §12
//! and ABI C-4 step 3 rewrite them: whether S's close fails before or after its settle phase, Q is destroyed once
//! by S and is dead when T catches the failure (destroy trace `C Q`).
use super::*;

const RES: s::Type = s::Type::Object(0);
const FUTURE_RES: s::Type = s::Type::Future(0);
/// Object 1, `Bad`: its destroy fails.
const BAD: u32 = 1;
/// Function indices; each async function's frame layout, lexical scope and boundary have its index.
const MAKE: u32 = 0;
const EPUB: u32 = 1;
const WATCH: u32 = 2;
const TAKER: u32 = 3;
const ESCAPE: u32 = 4;
const ASYNC: u32 = 5;
/// The destroy of `Res`, the first synchronous function.
const DESTROY_RES: u32 = ASYNC;
/// The inner block of `escape`, a scope and boundary after the async functions' own.
const INNER_E: u32 = ASYNC;
/// The failure mask of `fail` with label 1.
const FAIL_MASK: u32 = 2;

// --- SPB 17 programs ------------------------------------------------------------------------------------------

/// `let x = D();`: a new object of `descriptor` committed to the scope of `boundary`, and its packet.
fn create(b: &mut Body, descriptor: u32, boundary: u32) -> (Location, Location) {
    let object = b.register(s::Type::Object(descriptor));
    let Location::Register(id) = object else { unreachable!() };
    b.f.code.push(s::Instruction::Data { op: s::DataOp::ObjNew, inputs: vec![], output: Some(id),
        immediate: s::Immediate::Object(descriptor) });
    let arguments = vec![l(object), A::Object(descriptor), A::Boundary(boundary)];
    let packet = b.emit(O::OwnCreateCommit, arguments, K::Packet);
    (object, packet)
}

/// Return `value` with `packet` through a merging close of `state`, then publish the commit's outcome. A `failing`
/// close has a fallible executor (the failure mask of `fail`) and publishes its failure instead.
fn return_packet(b: &mut Body, body: Location, state: Location, scope: u32, value: Location, packet: Location,
    failing: bool)
{
    let selection = b.selection();
    let arguments = vec![A::Kind(s::ExitKind::Return), A::Boundary(scope), l(value), l(packet),
        A::Selection(selection)];
    let pending = b.emit(O::ExitPrepare, arguments, K::Pending);
    let envelope = b.f.proofs.envelopes.len();
    let finished = b.close(state, Some(pending), None, scope, &[], true);
    if failing {
        b.f.proofs.envelopes[envelope].failures = FAIL_MASK;
    }
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    let mut edges = vec![(Tag::Pending, "done", vec![p(0), p(1)])];
    if failing {
        edges.push((Tag::Failure, "failed", vec![p(0), p(1)]));
    }
    b.dispatch(finished, [true, false, failing], edges);
    b.block("done", &[closed, reserved]);
    let outputs = [b.register(RES), b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(s::ExitDestination::Caller)], outputs.to_vec());
    b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(outputs[0]), l(outputs[1]), l(outputs[2])], vec![]);
    b.ret();
    if failing {
        let [closed, combined] = [b.service(K::Closed), b.register(s::Type::Failure)];
        b.block("failed", &[closed, combined]);
        b.next(O::FuturePublish, vec![l(body), A::Flag(false), l(combined)], vec![]);
        b.ret();
    }
}

/// `async make() is Res { return Res(); }`: its outcome packet holds the new `Res`.
fn make() -> (s::Function, s::FrameLayout) {
    let (mut b, frame) = Body::asynchronous("make", MAKE, vec![RES]);
    let (body, inputs) = b.resume(frame, 0, &[s::ProducedType::Service(K::Packet)]);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(MAKE), A::Boundary(MAKE)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[0])], K::ScopeState);
    let (q, packet) = create(&mut b, 0, MAKE);
    return_packet(&mut b, body, q1, MAKE, q, packet, false);
    let layout = b.layout(vec![service(K::Packet)], vec![(vec![0], vec![])]);
    (b.finish(), layout)
}

/// Slots of a body that awaits its Future parameter: 0 the Future, 1 inputs, 2 owner state, 3–5 the unwind.
fn awaiting_slots() -> Vec<s::FrameSlot> {
    vec![value(FUTURE_RES), service(K::Packet), service(K::ScopeState), value(s::Type::Failure),
        service(K::TaskState), service(K::Receipt)]
}

/// `async epub(g: Future<Res>) is Res { await g; let q = Res(); let d = Bad(); return q; }` (the E_pub body,
/// draft 2 §7.2 in the form of ABI C-5): the return of `q` is reserved, then the close destroys `d`, whose destroy
/// fails. Without `epub`, `async watch(f: Future<Res>) { await f; }`; its closes merge, so the resources of its
/// inputs close with it.
fn awaiting(symbol: &str, index: u32, epub: bool) -> (s::Function, s::FrameLayout) {
    let slots = awaiting_slots();
    let results = if epub { vec![RES] } else { vec![] };
    let (mut b, frame) = Body::asynchronous(symbol, index, results);
    let (body, inputs) = b.resume(frame, 0, &types(&slots, &[0, 1]));
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(index), A::Boundary(index)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[1])], K::ScopeState);
    let first = b.save(body, 1, &[q1]);
    b.outcomes(O::Await, vec![l(inputs[0])], vec![(Tag::Normal, "ok", vec![p(0), p(1)]),
        (Tag::Unwind, "bad", vec![p(0), p(1), p(2)])]);
    let [x, deps] = [b.register(RES), b.service(K::Deps)];
    b.block("ok", &[x, deps]);
    let (body_ok, resumed) = b.resume(first, 1, &types(&slots, &[2]));
    if epub {
        let (q, q_packet) = create(&mut b, 0, index);
        let (_, d_packet) = create(&mut b, BAD, index);
        let q2 = b.emit(O::OwnAdmit, vec![l(resumed[0]), l(d_packet)], K::ScopeState);
        return_packet(&mut b, body_ok, q2, index, q, q_packet, true);
    } else {
        return_void(&mut b, body_ok, resumed[0], index, &[]);
    }
    let unwind = [b.register(s::Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block("bad", &unwind);
    let (body_bad, resumed) = b.resume(first, 1, &types(&slots, &[2]));
    let (body_adapted, after, adapted, _) = b.adapter(body_bad, 2, resumed[0], unwind, &[], &[]);
    // Its inputs may hold resources (an async defer's): the failing close merges them into its plan.
    fail_merging(&mut b, body_adapted, after, adapted, index, &[]);
    let layout = b.layout(slots, vec![(vec![0, 1], vec![]), (vec![2], vec![0]), (vec![], vec![2, 3, 4, 5])]);
    (b.finish(), layout)
}

/// Return nothing through a merging close of `state` that settles `duties`, then publish the success.
fn return_void(b: &mut Body, body: Location, state: Location, scope: u32, duties: &[Location]) {
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let arguments = vec![A::Kind(s::ExitKind::Return), A::Boundary(scope), l(empty), A::Selection(selection)];
    let pending = b.emit(O::ExitPrepare, arguments, K::Pending);
    let finished = b.close(state, Some(pending), None, scope, duties, true);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, "done", vec![p(0), p(1)])]);
    b.block("done", &[closed, reserved]);
    let [packet, deps] = [b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(s::ExitDestination::Caller)], vec![packet, deps]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(packet), l(deps)], vec![]);
    b.ret();
}

/// Close `state` with the active `failure` through a merging plan that settles `duties`, and publish the result.
fn fail_merging(b: &mut Body, body: Location, state: Location, failure: Location, scope: u32, duties: &[Location]) {
    let finished = b.close(state, None, Some(failure), scope, duties, true);
    let [closed, combined] = [b.service(K::Closed), b.register(s::Type::Failure)];
    b.dispatch(finished, [false, true, false], vec![(Tag::Failure, "unwound", vec![p(0), p(1)])]);
    b.block("unwound", &[closed, combined]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(false), l(combined)], vec![]);
    b.ret();
}

/// `fut.new` of `entry`, a `Future<Res>` with no arguments, owned by `scope`: (future, cache duty, state).
fn future_of(b: &mut Body, scope: u32, entry: u32, state: Location) -> (Location, Location, Location) {
    let offered = b.emit(O::OwnEmpty, vec![], K::Packet);
    let [future, cache, next] = [b.register(FUTURE_RES), b.service(K::CacheState), b.service(K::ScopeState)];
    let arguments = vec![A::Entry(entry), A::Scope(scope), A::Result(Some(FUTURE_RES)), l(offered), l(state)];
    b.next(O::FutureNew, arguments, vec![future, cache, next]);
    (future, cache, next)
}

/// `async taker() { let x = await make(); }` with a proved temporary take (OIR-2 §10.3, draft 3 §4.2): the
/// outcome packet joins the taker's scope, which destroys `x`; the consumed cache settles to nothing.
fn taker() -> (s::Function, s::FrameLayout) {
    // 0 inputs, 1 owner state, 2 cache duty, 3 the Future, 4–6 the unwind, 7 the take capability.
    let slots = vec![service(K::Packet), service(K::ScopeState), service(K::CacheState), value(FUTURE_RES),
        value(s::Type::Failure), service(K::TaskState), service(K::Receipt), service(K::UniqueCache)];
    let (mut b, frame) = Body::asynchronous("taker", TAKER, vec![]);
    let (body, inputs) = b.resume(frame, 0, &types(&slots, &[0]));
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(TAKER), A::Boundary(TAKER)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[0])], K::ScopeState);
    let (future, cache, q2) = future_of(&mut b, TAKER, MAKE, q1);
    let proof = b.emit(O::UniqueCacheProof, vec![l(future)], K::UniqueCache);
    let first = b.save(body, 1, &[q2, cache]);
    b.outcomes(O::AwaitTake, vec![l(future), l(proof), A::Scope(TAKER)], vec![
        (Tag::Normal, "ok", vec![p(0), p(1), p(2)]), (Tag::Unwind, "bad", vec![p(0), p(1), p(2)])]);
    let [x, packet, deps] = [b.register(RES), b.service(K::Packet), b.service(K::Deps)];
    b.block("ok", &[x, packet, deps]);
    let (body_ok, resumed) = b.resume(first, 1, &types(&slots, &[1, 2]));
    let q3 = b.emit(O::OwnAccept, vec![l(resumed[0]), l(packet)], K::ScopeState);
    return_void(&mut b, body_ok, q3, TAKER, &[resumed[1]]);
    let unwind = [b.register(s::Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block("bad", &unwind);
    let (body_bad, resumed) = b.resume(first, 1, &types(&slots, &[1, 2]));
    let (body_adapted, after, adapted, kept) =
        b.adapter(body_bad, 2, resumed[0], unwind, &[resumed[1]], &types(&slots, &[2]));
    fail_merging(&mut b, body_adapted, after, adapted, TAKER, &[kept[0]]);
    let layout = b.layout(slots, vec![(vec![0], vec![]), (vec![1, 2], vec![3, 7]), (vec![2], vec![1, 4, 5, 6])]);
    (b.finish(), layout)
}

/// `async escape() { let f = { let c = Res(); make() }; await f; }` (ABI C-4 and its ruling): F is created in
/// the inner block, which returns it to the body's scope, so its cache duty is rehomed into the block's exit for
/// that scope. The block's close destroys C; the commit gives F's duty to the body's scope, whose close settles
/// F after the await ran it and destroys its Q.
fn escaper() -> (s::Function, s::FrameLayout) {
    // 0 inputs, 1 owner state, 2 the Future, 3–5 the unwind.
    let slots = vec![service(K::Packet), service(K::ScopeState), value(FUTURE_RES), value(s::Type::Failure),
        service(K::TaskState), service(K::Receipt)];
    let (mut b, frame) = Body::asynchronous("escape", ESCAPE, vec![]);
    let (body, inputs) = b.resume(frame, 0, &types(&slots, &[0]));
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(ESCAPE), A::Boundary(ESCAPE)], K::ScopeState);
    let outer = b.emit(O::OwnAccept, vec![l(q0), l(inputs[0])], K::ScopeState);
    let i0 = b.emit(O::ScopeBegin, vec![A::Scope(INNER_E), A::Boundary(INNER_E)], K::ScopeState);
    let (_, c_packet) = create(&mut b, 0, INNER_E);
    let i1 = b.emit(O::OwnAdmit, vec![l(i0), l(c_packet)], K::ScopeState);
    let (future, cache, i2) = future_of(&mut b, INNER_E, MAKE, i1);
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let pending = b.emit(O::ExitPrepare, vec![A::Kind(s::ExitKind::Fallthrough), A::Boundary(INNER_E), l(future),
        l(empty), A::Selection(selection)], K::Pending);
    let destination = A::Destination(s::ExitDestination::Scope(ESCAPE));
    let pending = b.emit(O::CacheRehome, vec![destination.clone(), l(pending), l(cache)], K::Pending);
    let finished = b.close(i2, Some(pending), None, INNER_E, &[], true);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, "joined", vec![p(0), p(1)])]);
    b.block("joined", &[closed, reserved]);
    let [escaped, packet, deps] = [b.register(FUTURE_RES), b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), destination, A::Result(Some(FUTURE_RES))], vec![escaped, packet, deps]);
    let joined = b.emit(O::OwnAccept, vec![l(outer), l(packet)], K::ScopeState);
    let first = b.save(body, 1, &[joined]);
    b.outcomes(O::Await, vec![l(escaped)], vec![(Tag::Normal, "ok", vec![p(0), p(1)]),
        (Tag::Unwind, "bad", vec![p(0), p(1), p(2)])]);
    let [x, x_deps] = [b.register(RES), b.service(K::Deps)];
    b.block("ok", &[x, x_deps]);
    let (body_ok, resumed) = b.resume(first, 1, &types(&slots, &[1]));
    return_void(&mut b, body_ok, resumed[0], ESCAPE, &[]);
    let unwind = [b.register(s::Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block("bad", &unwind);
    let (body_bad, resumed) = b.resume(first, 1, &types(&slots, &[1]));
    let (body_adapted, after, adapted, _) = b.adapter(body_bad, 2, resumed[0], unwind, &[], &[]);
    fail_merging(&mut b, body_adapted, after, adapted, ESCAPE, &[]);
    let layout = b.layout(slots, vec![(vec![0], vec![]), (vec![1], vec![2]), (vec![], vec![1, 3, 4, 5])]);
    (b.finish(), layout)
}

/// The destroy entry of object `object`; `Bad`'s fails with "bad destroy".
fn destroy_of(symbol: &str, object: u32) -> s::Function {
    let fails = object == BAD;
    let mut b = Body::new(symbol, s::EntryAbi::Observation, vec![]);
    let target = b.register(s::Type::Object(object));
    b.f.params = vec![target];
    b.f.proofs.entry = Some(s::EntryContract {
        destroy: Some(object), failures: if fails { FAIL_MASK } else { 0 }, ..s::EntryContract::default()
    });
    b.block("entry", &[target]);
    if fails {
        let failure = b.fail("bad destroy", "raised");
        let Location::Register(id) = failure else { unreachable!() };
        b.f.code.push(s::Instruction::Raise(id));
    } else {
        b.ret();
    }
    b.finish()
}

fn r5_program() -> s::Program {
    let mut functions = Vec::new();
    let mut frames = Vec::new();
    for (function, layout) in [make(), awaiting("epub", EPUB, true), awaiting("watch", WATCH, false), taker(),
        escaper()]
    {
        functions.push(function);
        frames.push(layout);
    }
    functions.push(destroy_of("destroy_res", 0));
    functions.push(destroy_of("destroy_bad", BAD));
    let mut bad = res();
    bad.name = "Bad".into();
    s::Program {
        module: "resources".into(),
        objects: vec![res(), bad],
        functions,
        tables: s::ChampTables {
            scopes: (0..ASYNC).map(|index| s::Scope { function: index, parent: None, boundary: index })
                .chain([s::Scope { function: ESCAPE, parent: Some(ESCAPE), boundary: INNER_E }])
                .collect(),
            boundaries: (0..=INNER_E).map(|scope| s::Boundary { scope }).collect(),
            frames,
            future_results: vec![Some(RES)],
            ..s::ChampTables::default()
        },
        ..s::Program::default()
    }
}

/// [`r5_program`], through its text form, on a Champ runtime whose async bodies `engine` runs; the destroys are
/// recorded as `Object serial` lines.
fn load_r5(stress: bool, engine: Engine) -> Box<Runtime> {
    let built = r5_program();
    s::verify_load(&built, &s::NoExternalContracts).unwrap();
    let assembled = s::assemble(&s::disassemble(&built).unwrap()).unwrap();
    assert_eq!(assembled, built);
    let mut runtime = Runtime::new_champ(assembled, Vec::new()).unwrap();
    runtime.heap.set_stress(stress);
    runtime.champ.captured_trace = Some(Vec::new());
    if engine == Engine::Reference {
        for function in 0..ASYNC as usize {
            runtime.invocations[function].code = reference as *const () as u64;
        }
    }
    runtime
}

// --- Harness ----------------------------------------------------------------------------------------------------

/// The destroys so far, `Object serial` per line (draft 2's destroy trace).
fn destroyed(runtime: &Runtime) -> String {
    String::from_utf8(runtime.champ.captured_trace.clone().unwrap()).unwrap()
}

/// An object of `descriptor` committed to `owner`, rooted, in a packet of its own.
fn object_of(ctx: &mut Ctx, owner: OwnerId, descriptor: u32) -> (Word, PacketSlot) {
    let runtime = unsafe { &mut *ctx.runtime };
    let object = runtime.heap.object_new(descriptor, &[]);
    runtime.roots.push((object, Repr::Ref));
    let mut slot = PacketSlot::default();
    assert_eq!(rt_own_create_commit(ctx, object, descriptor as u64, owner.get(), &mut slot), NORMAL);
    (object, slot)
}

/// A Future of `make` started elsewhere and suspended there: the test plays its body.
fn gate(ctx: &mut Ctx, state: &mut ScopeSlot) -> Word {
    let inputs = empty(ctx);
    let gate = new_future(ctx, MAKE, &[], state, inputs);
    let runtime = unsafe { &mut *ctx.runtime };
    let tail = runtime.champ_frame_map(MAKE as usize).unwrap().tail;
    set(gate, tasks::STATE, tasks::RUNNING);
    set(gate, tail + TASK, gate);
    gate
}

/// Publish the gate's success (`failure` 0) or failure, with an empty outcome packet.
fn open(ctx: &mut Ctx, gate: Word, failure: Word) {
    let mut outcome = empty(ctx);
    let published = PublishIn { tag: u64::from(failure != 0), value: 0, packet: &mut outcome, failure };
    assert_eq!(rt_future_publish(ctx, gate, &published), NORMAL);
}

fn owner_of(runtime: &Runtime, state: &ScopeSlot) -> OwnerId {
    super::super::helpers::scope(runtime, state.record).unwrap().owner
}

/// `own.admit` of `packet` into `state`, rooted.
fn admit(ctx: &mut Ctx, state: &mut ScopeSlot, mut packet: PacketSlot) {
    let runtime = unsafe { &mut *ctx.runtime };
    let mut admitted = ScopeSlot::default();
    assert_eq!(rt_own_admit(ctx, state, &mut packet, &mut admitted), NORMAL, "{:?}", runtime.fatal);
    *state = admitted;
    runtime.roots.push((state.record, Repr::Ref));
}

/// A program failure with `text`, rooted.
fn failure(runtime: &mut Runtime, text: &str) -> Word {
    let failure = runtime.failure_new(FailureKind::Program(text.into()));
    runtime.roots.push((failure, Repr::Ref));
    failure
}

/// How a scope closes in [`close_with`].
struct Close<'a> {
    pending: Option<&'a mut PendingSlot>,
    failure: Word,
    /// Static duties in schedule order, or the dynamic step when None.
    duties: Option<&'a [Word]>,
}

/// Close `state` (D18 order: begin, settle, plan, the merge executor, finish) and return the finish Post.
fn close_with(ctx: &mut Ctx, state: &mut ScopeSlot, close: Close) -> FinishPost {
    let runtime = unsafe { &mut *ctx.runtime };
    let reserve = close.pending.is_some() || close.failure != 0;
    let plan = runtime.champ_register_cleanup_plan(CleanupPlan { reserve, direct: false, may_fail: true });
    let inputs = CloseInputs {
        scope: state, packet: std::ptr::null_mut(),
        pending: close.pending.map_or(std::ptr::null_mut(), |slot| slot as *mut PendingSlot),
        failure: close.failure, settlements: 0,
    };
    let mut cleanup = CleanupSlot::default();
    assert_eq!(rt_cleanup_begin(ctx, &inputs, &mut cleanup), NORMAL, "{:?}", runtime.fatal);
    let base = runtime.roots.len();
    runtime.roots.push((cleanup.record, Repr::Ref));
    match close.duties {
        Some(duties) => {
            let schedule = runtime.champ_register_schedule(duties.len() as u64);
            if duties.is_empty() {
                assert_eq!(rt_cleanup_settle_empty(ctx, &mut cleanup, &mut cleanup), NORMAL);
            }
            for (index, &duty) in duties.iter().enumerate() {
                let outcome = rt_service_settle(ctx, 0, &mut cleanup, schedule, index as u64, duty, &mut cleanup);
                assert_eq!(outcome, NORMAL, "{:?}", runtime.fatal);
            }
        }
        None => {
            let outcome = rt_cleanup_settle_dynamic(ctx, 0, &mut cleanup, &mut cleanup);
            assert_eq!(outcome, NORMAL, "{:?}", runtime.fatal);
        }
    }
    assert_eq!(rt_cleanup_plan(ctx, &mut cleanup, plan, &mut cleanup), NORMAL, "{:?}", runtime.fatal);
    loop {
        let mut next = NextPost::default();
        let ordinal = rt_cleanup_next(ctx, &mut cleanup, &mut next);
        cleanup = next.state;
        if ordinal == 1 {
            break;
        }
        assert_eq!(ordinal, NORMAL);
        let mut destroyed = DestroyPost::default();
        assert_eq!(rt_cleanup_destroy(ctx, &mut cleanup, next.key, &mut destroyed), NORMAL, "{:?}", runtime.fatal);
        cleanup = destroyed.state;
        if destroyed.failure != 0 {
            let outcome = rt_cleanup_failure(ctx, &mut cleanup, destroyed.failure, &mut cleanup);
            assert_eq!(outcome, NORMAL, "{:?}", runtime.fatal);
        }
    }
    let mut finished = FinishPost::default();
    assert_eq!(rt_cleanup_finish(ctx, &mut cleanup, &mut finished), NORMAL, "{:?}", runtime.fatal);
    runtime.roots.truncate(base);
    finished
}

// --- E_pub (draft 2 §7.2, ABI C-5) --------------------------------------------------------------------------------

#[test]
fn champ_r5_epub_trace_destroys_q_once_and_both_waiters_see_the_failure() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_r5(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        // F has started once and waits for G; W1 and W2 register with F in this order.
        let g = gate(&mut ctx, &mut state);
        let inputs = empty(&mut ctx);
        let f = new_future(&mut ctx, EPUB, &[g], &mut state, inputs);
        let mut watchers = Vec::new();
        for _ in 0..2 {
            let inputs = empty(&mut ctx);
            watchers.push(new_future(&mut ctx, WATCH, &[f], &mut state, inputs));
        }
        let [w1, w2] = watchers[..] else { unreachable!() };
        for waiter in [w1, w2] {
            runtime.champ_future_start(&mut ctx, 0, waiter);
            assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
            assert_eq!(get(waiter, tasks::AWAITING), f);
        }
        assert_eq!((get(f, tasks::STATE), get(f, tasks::AWAITING)), (tasks::RUNNING, g));
        let mark = runtime.champ.future_events.len();
        // F resumes: it reserves the return of Q, its close destroys D, whose destroy fails, the abort sends Q to
        // the plan, Q is destroyed, and F publishes the failure; then W1 and W2 are queued, in that order.
        open(&mut ctx, g, 0);
        runtime.schedule(&mut ctx, w2);
        assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
        let trace = runtime.champ.future_events[mark..].iter()
            .map(|event| (event.step, event.frame)).collect::<Vec<_>>();
        use FutureStep::*;
        assert_eq!(trace, [(Published, g), (Queued, f), (Resume, f), (Published, f), (Queued, w1), (Queued, w2),
            (Resume, w1), (Published, w1), (Resume, w2), (Published, w2)]);
        // destroy(Q) = 1: D (serial 2) first, then Q (serial 1), each once.
        assert_eq!(destroyed(&runtime), "Bad 2\nRes 1\n");
        // body_entries(F) = 1, terminal_transitions(F) = 1, wake(W1) = wake(W2) = 1.
        assert_eq!((count(&runtime, EPUB, Start), count(&runtime, EPUB, Published)), (1, 1));
        for waiter in [w1, w2] {
            let queued = trace.iter().filter(|&&event| event == (Queued, waiter)).count();
            assert_eq!(queued, 1);
        }
        // The success was never published: the cache holds no packet, and both waiters observe E_pub.
        let failure = get(f, tasks::FAILURE);
        assert_eq!(get(f, tasks::STATE), tasks::FAILURE_STATE);
        assert!(kind(&runtime, failure).contains("bad destroy"), "{}", kind(&runtime, failure));
        assert_eq!(runtime.champ_cache(f).unwrap().packet, 0);
        for frame in [f, w1, w2] {
            assert_eq!(get(frame, tasks::FAILURE), failure);
            assert!(cleared(&runtime, frame));
        }
        // A repeated await sees the same terminal outcome without entering F's body.
        let inputs = empty(&mut ctx);
        let w3 = new_future(&mut ctx, WATCH, &[f], &mut state, inputs);
        runtime.champ_future_start(&mut ctx, 0, w3);
        assert_eq!(get(w3, tasks::FAILURE), failure);
        assert_eq!(count(&runtime, EPUB, Start), 1);
        // Nothing else remains: every duty settles at the owner's close and nothing more is destroyed.
        let finished = close_with(&mut ctx, &mut state, Close {
            pending: None, failure: 0, duties: Some(&[g, f, w1, w2, w3]),
        });
        assert_eq!(finished.failure, 0);
        assert_eq!(destroyed(&runtime), "Bad 2\nRes 1\n");
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

// --- Rehome (draft 3 §12, ABI C-4 and its ruling) -----------------------------------------------------------------

/// A scope T and, inside it, S. F was created in S and has run on the engine: its cache holds Q(1). S holds C(2),
/// a `Res` or a `Bad`. The test returns F from S to T.
struct Escape {
    t: ScopeSlot,
    s: ScopeSlot,
    f: Word,
    q: Word,
}

fn escape(ctx: &mut Ctx, c: u32) -> Escape {
    let runtime = unsafe { &mut *ctx.runtime };
    let t = owner(ctx);
    let instance = ScopeInstance {
        holder: runtime.champ_new_holder().unwrap(), owner: runtime.champ_new_owner().unwrap(),
        parent_owner: owner_of(runtime, &t).get(),
    };
    let mut s = ScopeSlot::default();
    assert_eq!(rt_scope_begin(ctx, &instance, &mut s), NORMAL);
    runtime.roots.push((s.record, Repr::Ref));
    let inputs = empty(ctx);
    let f = new_future(ctx, MAKE, &[], &mut s, inputs);
    runtime.champ_future_start(ctx, 0, f);
    assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
    assert_eq!(get(f, tasks::STATE), tasks::DONE);
    let outcome = PacketSlot { record: runtime.champ_cache(f).unwrap().packet };
    let keys = runtime.champ_packet_keys(&outcome).unwrap();
    assert_eq!(keys.len(), 1);
    let q = keys[0].object();
    assert_eq!(q, get(f, tasks::RESULT));
    let (_, packet) = object_of(ctx, owner_of(runtime, &s), c);
    admit(ctx, &mut s, packet);
    Escape { t, s, f, q }
}

/// `exit.prepare` of S's return of `f` (the return carries no identity of S).
fn prepare_return(ctx: &mut Ctx, f: Word) -> PendingSlot {
    let mut packet = empty(ctx);
    let inputs = ExitInputs { kind: s::ExitKind::Return as u64, boundary: 0, value: f, value_repr: Repr::Ref as u64,
        packet: &mut packet, selection: 0 };
    let mut pending = PendingSlot::default();
    assert_eq!(rt_exit_prepare(ctx, &inputs, &mut pending), NORMAL);
    unsafe { &mut *ctx.runtime }.roots.push((pending.record, Repr::Ref));
    pending
}

/// The token form of `cache.rehome`: F's cache duty (the token is the Future frame) moves into the reserve.
fn rehome_token(ctx: &mut Ctx, pending: &mut PendingSlot, f: Word, target: u64) -> Outcome {
    let mut cache = CacheSlot { record: f };
    let mut post = RehomePost::default();
    let outcome = rt_cache_rehome(ctx, pending, std::ptr::null_mut(), &mut cache, std::ptr::null_mut(), f, target,
        &mut post);
    if outcome == NORMAL {
        assert_eq!((pending.record, cache.record, post.state.record), (0, 0, 0));
        *pending = post.pending;
        unsafe { &mut *ctx.runtime }.roots.push((pending.record, Repr::Ref));
    }
    outcome
}

/// F's cache token absorbed into a packet that `state` admits: the duty joins S's service list (ABI D25.2).
fn absorb_into(ctx: &mut Ctx, state: &mut ScopeSlot, f: Word) {
    let mut packet = empty(ctx);
    let mut absorbed = PacketSlot::default();
    assert_eq!(rt_service_absorb(ctx, &mut packet, f, SERVICE_CACHE, &mut absorbed), NORMAL);
    assert_eq!(packet.record, 0);
    admit(ctx, state, absorbed);
}

fn reserve_services(runtime: &Runtime, pending: &PendingSlot) -> Vec<Service> {
    super::super::helpers::pending(runtime, pending.record).unwrap().reserve.services
}

fn life(runtime: &Runtime, object: Word) -> Life {
    runtime.heap.champ_identity(object).unwrap().life
}

#[test]
fn champ_r5_rehome_then_failure_before_settle_destroys_q_in_s() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_r5(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let Escape { mut t, mut s, f, q } = escape(&mut ctx, 0);
        let t_owner = owner_of(&runtime, &t);
        let s_owner = owner_of(&runtime, &s);
        let mut pending = prepare_return(&mut ctx, f);
        assert_eq!(rehome_token(&mut ctx, &mut pending, f, t_owner.get()), NORMAL);
        // The duty is a receipt in the reserve; the owner is still S (draft 3 §12).
        let receipt = Service { holder: f, kind: SERVICE_CACHE, target: Some(t_owner.get()) };
        assert_eq!(reserve_services(&runtime, &pending), [receipt]);
        assert_eq!(runtime.champ_cache(f).unwrap().owner, s_owner);
        assert_eq!(runtime.champ_cache(f).unwrap().flags, REHOMED);
        // A failure before S's settle phase (a body failure while the return was pending) consumes the whole
        // reserve; S settles F's receipt before the plan, which destroys C, then Q.
        // There is no dynamic step: the receipt joins the remainder, which the plan settles before it merges.
        let e = failure(&mut runtime, "E");
        let finished = close_with(&mut ctx, &mut s, Close {
            pending: Some(&mut pending), failure: e, duties: Some(&[]),
        });
        assert_eq!((finished.failure, finished.pending.record), (e, 0));
        assert_eq!(destroyed(&runtime), "Res 2\nRes 1\n");
        // T catches E: Q is dead and F's duty is gone; T's close destroys nothing more.
        assert_eq!(life(&runtime, q), Life::Dead);
        let cache = runtime.champ_cache(f).unwrap();
        assert_eq!((cache.flags, cache.packet, cache.owner), (RELEASED, 0, s_owner));
        let finished = close_with(&mut ctx, &mut t, Close { pending: None, failure: 0, duties: None });
        assert_eq!(finished.failure, 0);
        assert_eq!(destroyed(&runtime), "Res 2\nRes 1\n");
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

#[test]
fn champ_r5_rehome_then_failed_destroy_after_settle_destroys_q_in_s() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_r5(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let Escape { mut t, mut s, f, q } = escape(&mut ctx, BAD);
        let t_owner = owner_of(&runtime, &t);
        // The state form: F's duty is an entry of S's service list, and the rehome takes it from there.
        absorb_into(&mut ctx, &mut s, f);
        let mut pending = prepare_return(&mut ctx, f);
        let mut post = RehomePost::default();
        let outcome = rt_cache_rehome(&mut ctx, &mut pending, &mut s, std::ptr::null_mut(), std::ptr::null_mut(),
            f, t_owner.get(), &mut post);
        assert_eq!(outcome, NORMAL, "{:?}", runtime.fatal);
        assert_eq!((pending.record, s.record), (0, 0));
        (pending, s) = (post.pending, post.state);
        runtime.roots.extend([(pending.record, Repr::Ref), (s.record, Repr::Ref)]);
        assert!(super::super::helpers::scope(&runtime, s.record).unwrap().all().services.is_empty());
        // Draft 3 §12: settle S (F is not there) → claim C → destroy C !E_C → abort → finish C → nested settle F
        // (DONE: Q joins the suffix) → destroy Q → closed S(E_C).
        let finished = close_with(&mut ctx, &mut s, Close { pending: Some(&mut pending), failure: 0, duties: None });
        assert_eq!(finished.pending.record, 0);
        assert!(kind(&runtime, finished.failure).contains("bad destroy"));
        assert_eq!(destroyed(&runtime), "Bad 2\nRes 1\n");
        // T catches E_C: Q is dead, destroy(Q) = 1, and T's close destroys nothing more.
        assert_eq!(life(&runtime, q), Life::Dead);
        assert_eq!(runtime.champ_cache(f).unwrap().flags, RELEASED);
        let finished = close_with(&mut ctx, &mut t, Close { pending: None, failure: 0, duties: None });
        assert_eq!(finished.failure, 0);
        assert_eq!(destroyed(&runtime), "Bad 2\nRes 1\n");
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

#[test]
fn champ_r5_rehome_commit_moves_custody_and_outcome_to_t() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_r5(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let Escape { mut t, mut s, f, q } = escape(&mut ctx, 0);
        let t_scope = super::super::helpers::scope(&runtime, t.record).unwrap();
        let mut pending = prepare_return(&mut ctx, f);
        assert_eq!(rehome_token(&mut ctx, &mut pending, f, t_scope.owner.get()), NORMAL);
        let mut finished = close_with(&mut ctx, &mut s, Close {
            pending: Some(&mut pending), failure: 0, duties: None,
        });
        assert_eq!(finished.failure, 0);
        assert_eq!(destroyed(&runtime), "Res 2\n");
        // exit.commit performs the single transition: F's cache and the identities of its outcome take T.
        let epoch = runtime.heap.champ_identity(q).unwrap().epoch;
        let destination = ExitDestination { holder: t_scope.holder, owner: t_scope.owner };
        let mut commit = CommitPost::default();
        assert_eq!(rt_exit_commit(&mut ctx, &mut finished.pending, &destination, &mut commit), NORMAL);
        assert_eq!(commit.value, f);
        let committed = super::super::helpers::packet(&runtime, commit.packet.record).unwrap();
        assert_eq!(committed.services, [Service { holder: f, kind: SERVICE_CACHE, target: None }]);
        let cache = runtime.champ_cache(f).unwrap();
        assert_eq!((cache.owner, cache.flags), (t_scope.owner, 0));
        let identity = runtime.heap.champ_identity(q).unwrap();
        assert_eq!((identity.owner_id, identity.holder_id, identity.life), (t_scope.owner.get(),
            cache.holder.get(), Life::Live));
        assert_eq!(identity.epoch, epoch + 1);
        // T admits the return and settles F's duty at its close: Q is destroyed there, once.
        admit(&mut ctx, &mut t, commit.packet);
        let finished = close_with(&mut ctx, &mut t, Close { pending: None, failure: 0, duties: None });
        assert_eq!(finished.failure, 0);
        assert_eq!(destroyed(&runtime), "Res 2\nRes 1\n");
        assert_eq!(runtime.champ_cache(f).unwrap().flags, RELEASED);
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

/// A refused operation stops the engine with `message`; the test then clears the stop.
fn stopped(ctx: &mut Ctx, outcome: Outcome, message: &str) {
    let runtime = unsafe { &mut *ctx.runtime };
    assert_eq!(outcome, OUTCOME_STOP);
    let fatal = runtime.fatal.take().unwrap();
    assert!(fatal.contains(message), "{fatal}");
    ctx.status = OK;
}

#[test]
fn champ_r5_rehome_refuses_a_second_custody_and_a_foreign_target() {
    let mut runtime = load_r5(true, Engine::Jit);
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = frame_pointer!();
    let Escape { t, mut s, f, .. } = escape(&mut ctx, 0);
    let t_scope = super::super::helpers::scope(&runtime, t.record).unwrap();
    let s_owner = owner_of(&runtime, &s);
    // The state form needs the duty in the state's list.
    let mut pending = prepare_return(&mut ctx, f);
    let mut post = RehomePost::default();
    let outcome = rt_cache_rehome(&mut ctx, &mut pending, &mut s, std::ptr::null_mut(), std::ptr::null_mut(), f,
        t_scope.owner.get(), &mut post);
    stopped(&mut ctx, outcome, "the state holds no cache duty");
    // A token names its own Future.
    let other = {
        let inputs = empty(&mut ctx);
        new_future(&mut ctx, MAKE, &[], &mut s, inputs)
    };
    let mut cache = CacheSlot { record: other };
    let outcome = rt_cache_rehome(&mut ctx, &mut pending, std::ptr::null_mut(), &mut cache, std::ptr::null_mut(), f,
        t_scope.owner.get(), &mut post);
    stopped(&mut ctx, outcome, "the cache token names another Future");
    // One place per duty: a receipt cannot be rehomed again, nor settled by S's static schedule.
    assert_eq!(rehome_token(&mut ctx, &mut pending, f, s_owner.get()), NORMAL);
    let outcome = rehome_token(&mut ctx, &mut pending, f, t_scope.owner.get());
    stopped(&mut ctx, outcome, "settled or rehomed");
    let begin = CloseInputs { scope: &mut s, packet: std::ptr::null_mut(), pending: std::ptr::null_mut(), failure: 0,
        settlements: 0 };
    let mut cleanup = CleanupSlot::default();
    assert_eq!(rt_cleanup_begin(&mut ctx, &begin, &mut cleanup), NORMAL);
    runtime.roots.push((cleanup.record, Repr::Ref));
    let schedule = runtime.champ_register_schedule(1);
    let outcome = rt_service_settle(&mut ctx, 0, &mut cleanup, schedule, 0, f, &mut cleanup);
    stopped(&mut ctx, outcome, "settles with the reserve");
    // The receipt names S as its target: committing the exit to T is refused, and nothing changes.
    let destination = ExitDestination { holder: t_scope.holder, owner: t_scope.owner };
    let mut commit = CommitPost::default();
    let outcome = rt_exit_commit(&mut ctx, &mut pending, &destination, &mut commit);
    stopped(&mut ctx, outcome, "another target");
    assert_eq!((runtime.champ_cache(f).unwrap().owner, runtime.champ_cache(f).unwrap().flags), (s_owner, REHOMED));
    // A receipt for the caller commits to whatever destination the exit has.
    let mut caller = prepare_return(&mut ctx, other);
    assert_eq!(rehome_token(&mut ctx, &mut caller, other, TARGET_CALLER), NORMAL);
    assert_eq!(rt_exit_commit(&mut ctx, &mut caller, &destination, &mut commit), NORMAL);
    assert_eq!(runtime.champ_cache(other).unwrap().owner, t_scope.owner);
}

// --- await.take (draft 3 §4.2) -------------------------------------------------------------------------------------

#[test]
fn champ_r5_await_take_consumes_the_cache_and_hands_its_packet_to_the_boundary() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_r5(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let boundary = owner_of(&runtime, &state);
        // `this` is a running body the test plays; F is cold and starts on its stack.
        let this = gate(&mut ctx, &mut state);
        let inputs = empty(&mut ctx);
        let f = new_future(&mut ctx, MAKE, &[], &mut state, inputs);
        let holder = runtime.champ_cache(f).unwrap().holder;
        let mut post = AwaitTakePost::default();
        assert_eq!(rt_await_take(&mut ctx, this, f, boundary.get(), &mut post), NORMAL, "{:?}", runtime.fatal);
        assert_eq!(steps(&runtime, MAKE), [FutureStep::Start, FutureStep::Published]);
        let taken = runtime.champ_packet_keys(&post.packet).unwrap();
        assert_eq!(taken.iter().map(|key| key.object()).collect::<Vec<_>>(), [post.value]);
        let q = post.value;
        // The packet left the cache in one transition to the boundary, under a new holder.
        let identity = runtime.heap.champ_identity(q).unwrap();
        assert_eq!(identity.owner_id, boundary.get());
        assert_ne!(identity.holder_id, holder.get());
        let cache = runtime.champ_cache(f).unwrap();
        assert_eq!((cache.packet, cache.flags), (0, CONSUMED_BY_TAKE));
        assert_eq!((get(f, tasks::STATE), get(f, tasks::RESULT)), (tasks::CONSUMED, 0));
        // The boundary admits it and destroys Q once; the consumed cache settles to nothing.
        admit(&mut ctx, &mut state, post.packet);
        open(&mut ctx, this, 0);
        let finished = close_with(&mut ctx, &mut state, Close { pending: None, failure: 0, duties: Some(&[this, f]) });
        assert_eq!(finished.failure, 0);
        assert_eq!(destroyed(&runtime), "Res 1\n");
        assert_eq!(runtime.champ_cache(f).unwrap().flags, CONSUMED_BY_TAKE | RELEASED);
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

#[test]
fn champ_r5_await_take_suspends_like_await_and_keeps_a_failed_cache() {
    let mut runtime = load_r5(true, Engine::Jit);
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = frame_pointer!();
    let mut state = owner(&mut ctx);
    let boundary = owner_of(&runtime, &state).get();
    let this = gate(&mut ctx, &mut state);
    // A running Future: `this` registers and suspends; the settle wake re-enters the same step.
    let g = gate(&mut ctx, &mut state);
    let inputs = empty(&mut ctx);
    let w = new_future(&mut ctx, WATCH, &[g], &mut state, inputs);
    let mut post = AwaitTakePost::default();
    assert_eq!(rt_await_take(&mut ctx, this, w, boundary, &mut post), OUTCOME_PENDING);
    assert_eq!(get(this, tasks::AWAITING), w);
    assert_ne!(get(this, tasks::FLAGS) & tasks::champ_flags::AWAIT_SUSPENDED, 0);
    open(&mut ctx, g, 0);
    runtime.schedule(&mut ctx, w);
    assert_eq!(runtime.ready.pop_front(), Some(this));
    assert_eq!(rt_await_take(&mut ctx, this, w, boundary, &mut post), NORMAL);
    assert_eq!(get(w, tasks::STATE), tasks::CONSUMED);
    assert!(runtime.champ_packet_keys(&post.packet).unwrap().is_empty());
    // A consumed cache is not taken twice.
    let outcome = rt_await_take(&mut ctx, this, w, boundary, &mut post);
    stopped(&mut ctx, outcome, "await.take of a consumed Future");
    // A failure is the await's Unwind with the Future as the task; the cache is not consumed.
    let g = gate(&mut ctx, &mut state);
    let inputs = empty(&mut ctx);
    let w = new_future(&mut ctx, WATCH, &[g], &mut state, inputs);
    runtime.champ_future_start(&mut ctx, 0, w);
    let e = failure(&mut runtime, "E");
    open(&mut ctx, g, e);
    runtime.schedule(&mut ctx, w);
    let mut post = AwaitTakePost::default();
    assert_eq!(rt_await_take(&mut ctx, this, w, boundary, &mut post), UNWIND);
    assert_eq!((ctx.status, ctx.failure), (FAILED, e));
    assert_eq!((post.value, post.task, post.receipt, post.packet.record), (e, w, RECEIPT_SETTLED, 0));
    (ctx.status, ctx.failure) = (OK, 0);
    assert_eq!(get(w, tasks::STATE), tasks::FAILURE_STATE);
    assert_eq!(runtime.champ_cache(w).unwrap().flags, 0);
    // A settled or rehomed cache has no outcome to take.
    let mut other = owner(&mut ctx);
    for rehomed in [false, true] {
        let inputs = empty(&mut ctx);
        let f = new_future(&mut ctx, MAKE, &[], &mut other, inputs);
        runtime.champ_future_start(&mut ctx, 0, f);
        if rehomed {
            let mut pending = prepare_return(&mut ctx, f);
            assert_eq!(rehome_token(&mut ctx, &mut pending, f, TARGET_CALLER), NORMAL);
        } else {
            let mut closing = owner(&mut ctx);
            runtime.champ_cache_mut(f).unwrap().owner = owner_of(&runtime, &closing);
            let finished = close_with(&mut ctx, &mut closing, Close { pending: None, failure: 0, duties: Some(&[f]) });
            assert_eq!(finished.failure, 0);
        }
        let outcome = rt_await_take(&mut ctx, this, f, boundary, &mut post);
        stopped(&mut ctx, outcome, "await.take of a settled or rehomed cache");
    }
}

// --- Service settle (draft 3 §§6.2–6.3, ABI D25) ---------------------------------------------------------------------

#[test]
fn champ_r5_released_outcomes_settle_their_nested_duties_first() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_r5(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        // G ran and caches Q(1); G's duty travels in the outcome packet that H publishes (a Future returned in an
        // object's field, ABI D25.4).
        let h = gate(&mut ctx, &mut state);
        let inputs = empty(&mut ctx);
        let g = new_future(&mut ctx, MAKE, &[], &mut state, inputs);
        runtime.champ_future_start(&mut ctx, 0, g);
        let mut outcome = empty(&mut ctx);
        let mut absorbed = PacketSlot::default();
        assert_eq!(rt_service_absorb(&mut ctx, &mut outcome, g, SERVICE_CACHE, &mut absorbed), NORMAL);
        let published = PublishIn { tag: 0, value: 0, packet: &mut absorbed, failure: 0 };
        assert_eq!(rt_future_publish(&mut ctx, h, &published), NORMAL);
        let cached = super::super::helpers::packet(&runtime, runtime.champ_cache(h).unwrap().packet).unwrap();
        assert_eq!(cached.services, [Service { holder: g, kind: SERVICE_CACHE, target: None }]);
        // The owner's close settles H; H's released outcome settles G first, so Q joins the plan.
        let finished = close_with(&mut ctx, &mut state, Close { pending: None, failure: 0, duties: Some(&[h]) });
        assert_eq!(finished.failure, 0);
        assert_eq!(destroyed(&runtime), "Res 1\n");
        for frame in [g, h] {
            assert_eq!(runtime.champ_cache(frame).unwrap().flags, RELEASED);
        }
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

#[test]
fn champ_r5_dynamic_settle_runs_once_between_the_static_duties_and_the_plan() {
    let mut runtime = load_r5(true, Engine::Jit);
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = frame_pointer!();
    let mut state = owner(&mut ctx);
    let inputs = empty(&mut ctx);
    let f = new_future(&mut ctx, MAKE, &[], &mut state, inputs);
    runtime.champ_future_start(&mut ctx, 0, f);
    // The list entry and its token are one duty: a second entry is refused.
    absorb_into(&mut ctx, &mut state, f);
    let mut packet = empty(&mut ctx);
    let mut absorbed = PacketSlot::default();
    assert_eq!(rt_service_absorb(&mut ctx, &mut packet, f, SERVICE_CACHE, &mut absorbed), NORMAL);
    let mut admitted = ScopeSlot::default();
    let outcome = rt_own_admit(&mut ctx, &mut state, &mut absorbed, &mut admitted);
    stopped(&mut ctx, outcome, "a service duty in two places");
    let begin = CloseInputs { scope: &mut state, packet: std::ptr::null_mut(), pending: std::ptr::null_mut(),
        failure: 0, settlements: 0 };
    let mut cleanup = CleanupSlot::default();
    assert_eq!(rt_cleanup_begin(&mut ctx, &begin, &mut cleanup), NORMAL);
    runtime.roots.push((cleanup.record, Repr::Ref));
    // Not inside an open static schedule.
    let schedule = runtime.champ_register_schedule(2);
    let other = {
        let mut owner = owner(&mut ctx);
        let inputs = empty(&mut ctx);
        new_future(&mut ctx, MAKE, &[], &mut owner, inputs)
    };
    assert_eq!(rt_service_settle(&mut ctx, 0, &mut cleanup, schedule, 0, other, &mut cleanup), NORMAL);
    let outcome = rt_cleanup_settle_dynamic(&mut ctx, 0, &mut cleanup, &mut cleanup);
    stopped(&mut ctx, outcome, "one dynamic settle per close");
    // A fresh close: static duties, then the list, once, then the plan.
    let mut state = owner(&mut ctx);
    let inputs = empty(&mut ctx);
    let f = new_future(&mut ctx, MAKE, &[], &mut state, inputs);
    runtime.champ_future_start(&mut ctx, 0, f);
    absorb_into(&mut ctx, &mut state, f);
    let begin = CloseInputs { scope: &mut state, packet: std::ptr::null_mut(), pending: std::ptr::null_mut(),
        failure: 0, settlements: 0 };
    assert_eq!(rt_cleanup_begin(&mut ctx, &begin, &mut cleanup), NORMAL);
    runtime.roots.push((cleanup.record, Repr::Ref));
    assert_eq!(rt_cleanup_settle_dynamic(&mut ctx, 0, &mut cleanup, &mut cleanup), NORMAL);
    let outcome = rt_cleanup_settle_dynamic(&mut ctx, 0, &mut cleanup, &mut cleanup);
    stopped(&mut ctx, outcome, "one dynamic settle per close");
    let plan = runtime.champ_register_cleanup_plan(CleanupPlan { reserve: false, direct: false, may_fail: false });
    assert_eq!(rt_cleanup_plan(&mut ctx, &mut cleanup, plan, &mut cleanup), NORMAL);
    let outcome = rt_cleanup_settle_dynamic(&mut ctx, 0, &mut cleanup, &mut cleanup);
    stopped(&mut ctx, outcome, "one dynamic settle per close");
}

#[test]
fn champ_r5_materialize_takes_back_exactly_a_held_entry() {
    let mut runtime = load_r5(false, Engine::Jit);
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = frame_pointer!();
    let mut state = owner(&mut ctx);
    let inputs = empty(&mut ctx);
    let f = new_future(&mut ctx, MAKE, &[], &mut state, inputs);
    let mut packet = empty(&mut ctx);
    let mut absorbed = PacketSlot::default();
    assert_eq!(rt_service_absorb(&mut ctx, &mut packet, f, SERVICE_CACHE, &mut absorbed), NORMAL);
    let mut post = MaterializePost::default();
    let outcome = rt_service_materialize(&mut ctx, &mut absorbed, f, SERVICE_TASK, &mut post);
    stopped(&mut ctx, outcome, "does not hold");
    assert_eq!(rt_service_materialize(&mut ctx, &mut absorbed, f, SERVICE_CACHE, &mut post), NORMAL);
    assert_eq!((absorbed.record, post.token), (0, f));
    assert!(super::super::helpers::packet(&runtime, post.packet.record).unwrap().is_empty());
    // A settled duty is not absorbed again.
    let finished = close_with(&mut ctx, &mut state, Close { pending: None, failure: 0, duties: Some(&[f]) });
    assert_eq!(finished.failure, 0);
    let outcome = rt_service_absorb(&mut ctx, &mut post.packet, f, SERVICE_CACHE, &mut absorbed);
    stopped(&mut ctx, outcome, "service.absorb of a settled or rehomed duty");
}

#[test]
fn champ_r5_publish_refuses_a_duty_of_another_owner_or_a_receipt() {
    let mut runtime = load_r5(false, Engine::Jit);
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = frame_pointer!();
    let mut state = owner(&mut ctx);
    let mut other = owner(&mut ctx);
    let h = gate(&mut ctx, &mut state);
    let inputs = empty(&mut ctx);
    let foreign = new_future(&mut ctx, MAKE, &[], &mut other, inputs);
    let mut packet = empty(&mut ctx);
    let mut absorbed = PacketSlot::default();
    assert_eq!(rt_service_absorb(&mut ctx, &mut packet, foreign, SERVICE_CACHE, &mut absorbed), NORMAL);
    let published = PublishIn { tag: 0, value: 0, packet: &mut absorbed, failure: 0 };
    let outcome = rt_future_publish(&mut ctx, h, &published);
    stopped(&mut ctx, outcome, "an outcome packet outside its cache");
    assert_eq!(get(h, tasks::STATE), tasks::RUNNING);
}

// --- GC roots (draft 3 §13) ------------------------------------------------------------------------------------------

#[test]
fn champ_r5_a_duty_in_a_list_or_a_reserve_keeps_its_future_alive() {
    let mut runtime = load_r5(true, Engine::Jit);
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = frame_pointer!();
    let mut state = owner(&mut ctx);
    let mut made = Vec::new();
    for _ in 0..2 {
        let inputs = empty(&mut ctx);
        let f = new_future(&mut ctx, MAKE, &[], &mut state, inputs);
        runtime.champ_future_start(&mut ctx, 0, f);
        made.push(f);
    }
    let [listed, reserved] = made[..] else { unreachable!() };
    absorb_into(&mut ctx, &mut state, listed);
    // `reserved` is in a reserve whose return value is not the Future.
    let mut packet = empty(&mut ctx);
    let inputs = ExitInputs { kind: s::ExitKind::Return as u64, boundary: 0, value: 0, value_repr: Repr::Raw as u64,
        packet: &mut packet, selection: 0 };
    let mut pending = PendingSlot::default();
    assert_eq!(rt_exit_prepare(&mut ctx, &inputs, &mut pending), NORMAL);
    runtime.roots.push((pending.record, Repr::Ref));
    assert_eq!(rehome_token(&mut ctx, &mut pending, reserved, TARGET_CALLER), NORMAL);
    // Only the list entry and the receipt refer to the two Futures now; a collection keeps them and their
    // cached outcomes (a lost live Q would fail the sweep check).
    runtime.roots.retain(|&(word, _)| word != listed && word != reserved);
    let collections = runtime.heap.stats().collections;
    runtime.safepoint(frame_pointer!(), &ctx, &[]);
    assert!(runtime.heap.stats().collections > collections);
    for f in [listed, reserved] {
        assert_eq!(runtime.champ_cache(f).unwrap().owner, owner_of(&runtime, &state));
    }
    let finished = close_with(&mut ctx, &mut state, Close { pending: Some(&mut pending), failure: 0, duties: None });
    assert_eq!(finished.failure, 0);
    assert_eq!(destroyed(&runtime), "Res 1\n");
    let state = super::super::helpers::scope(&runtime, owner(&mut ctx).record).unwrap();
    let destination = ExitDestination { holder: state.holder, owner: state.owner };
    let mut finished = finished;
    let mut commit = CommitPost::default();
    assert_eq!(rt_exit_commit(&mut ctx, &mut finished.pending, &destination, &mut commit), NORMAL);
    assert_eq!(runtime.champ_cache(reserved).unwrap().owner, state.owner);
}

// --- Async defer (draft 3 §11, ABI C-6) ------------------------------------------------------------------------------

/// `invoke.defer` of `watch(g)` as an async defer of the running `this`, with `r` in its input packet.
struct Defer {
    io: CallOwn,
    args: [Word; 1],
    registration: RegistrationMap,
    post: DeferPost,
}

impl Defer {
    fn new(input: PacketSlot, g: Word) -> Self {
        let io = CallOwn { input, entry_abi: u64::from(WATCH), output: std::ptr::null_mut(), context: 0,
            cleanup_mode: CLEANUP_SHIELDED };
        Self { io, args: [g], registration: RegistrationMap { replacements: 0 }, post: DeferPost::default() }
    }

    fn step(&mut self, ctx: &mut Ctx, this: Word) -> Outcome {
        rt_invoke_defer_async(ctx, this, u64::from(WATCH), self.args.as_ptr(), 1, &mut self.io, &self.registration,
            &mut self.post)
    }
}

#[test]
fn champ_r5_async_defer_suspends_shielded_and_continues_or_unwinds() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_r5(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let this = gate(&mut ctx, &mut state);
        let step = tail(&runtime, this) + STEP;
        for fails in [false, true] {
            let g = gate(&mut ctx, &mut state);
            let (r, input) = object_of(&mut ctx, owner_of(&runtime, &state), 0);
            let mut defer = Defer::new(input, g);
            runtime.roots.push((defer.io.input.record, Repr::Ref));
            // The child starts on this stack and waits for G: PENDING, with the child as the frame's STEP.
            assert_eq!(defer.step(&mut ctx, this), OUTCOME_PENDING, "{:?}", runtime.fatal);
            let child = get(this, step);
            assert_eq!(runtime.function_of(child), WATCH as usize);
            let shield = tasks::champ_flags::SHIELDED | tasks::champ_flags::OBSERVED;
            assert_eq!(get(child, tasks::FLAGS) & shield, shield);
            assert_eq!(runtime.champ_task_of(child).unwrap(), child);
            // `this` only listens (§5): no AWAITING, no WAITED; the input operand stays a packet, now empty.
            assert_eq!(get(child, tasks::WAITER), this);
            assert_eq!(get(this, tasks::AWAITING), word::UNBOUND);
            assert_eq!(get(this, tasks::FLAGS) & tasks::champ_flags::AWAIT_SUSPENDED, 0);
            assert!(super::super::helpers::packet(&runtime, defer.io.input.record).unwrap().is_empty());
            assert_eq!(life(&runtime, r), Life::Live);
            // A re-entry while the child runs continues the same step: no second child, no second listener.
            assert_eq!(defer.step(&mut ctx, this), OUTCOME_PENDING);
            assert_eq!((get(this, step), get(child, tasks::WAITER)), (child, this));
            // A cancellation of `this` stays requested: the shielded child is not cancelled.
            assert_eq!(rt_cancel_request(&mut ctx, this), NORMAL);
            let requested = tasks::champ_flags::CANCEL_REQUESTED;
            assert_eq!(get(child, tasks::FLAGS) & requested, 0);
            assert!(runtime.ready.is_empty());
            let e = if fails { failure(&mut runtime, "defer failed") } else { 0 };
            open(&mut ctx, g, e);
            runtime.schedule(&mut ctx, child);
            assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
            assert_eq!(runtime.ready.pop_front(), Some(this));
            assert!(runtime.ready.is_empty());
            // The child closed its inputs: r was destroyed by the defer, once.
            assert_eq!(life(&runtime, r), Life::Dead);
            let outcome = defer.step(&mut ctx, this);
            assert_eq!((get(this, step), defer.io.input.record), (0, 0));
            assert_eq!(runtime.champ_cache(child).unwrap().flags, RELEASED);
            if fails {
                // Unwind with the child's carrier, for the caller to capture.
                assert_eq!(outcome, UNWIND);
                assert_eq!((ctx.status, ctx.failure, defer.post.failure), (FAILED, e, e));
                (ctx.status, ctx.failure) = (OK, 0);
            } else {
                assert_eq!(outcome, NORMAL);
                assert_eq!((defer.post.failure, defer.post.value, defer.post.packet.record), (0, 0, 0));
            }
            let flags = get(this, tasks::FLAGS);
            assert_eq!(flags & (requested | tasks::champ_flags::CANCEL_DELIVERED), requested);
            set(this, tasks::FLAGS, flags & !requested);
        }
        assert_eq!(destroyed(&runtime), "Res 1\nRes 2\n");
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

#[test]
fn champ_r5_async_defer_refuses_labels_and_a_non_async_entry() {
    let mut runtime = load_r5(false, Engine::Jit);
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = frame_pointer!();
    let mut state = owner(&mut ctx);
    let this = gate(&mut ctx, &mut state);
    let g = gate(&mut ctx, &mut state);
    let input = empty(&mut ctx);
    let mut defer = Defer::new(input, g);
    defer.registration.replacements = 1;
    let outcome = defer.step(&mut ctx, this);
    stopped(&mut ctx, outcome, "no replacement labels");
    defer.registration.replacements = 0;
    let entry = u64::from(DESTROY_RES);
    let outcome = rt_invoke_defer_async(&mut ctx, this, entry, defer.args.as_ptr(), 1, &mut defer.io,
        &defer.registration, &mut defer.post);
    stopped(&mut ctx, outcome, "not an async body");
    assert_eq!(get(this, tail(&runtime, this) + STEP), 0);
    assert!(super::super::helpers::packet(&runtime, defer.io.input.record).is_ok());
}

// --- Service entries through packet transitions (ABI D25.1) ----------------------------------------------------------

/// A packet of `f`'s cache entry, made by the test as `service.absorb` would.
fn absorbed(ctx: &mut Ctx, f: Word) -> PacketSlot {
    let mut packet = empty(ctx);
    let mut absorbed = PacketSlot::default();
    assert_eq!(rt_service_absorb(ctx, &mut packet, f, SERVICE_CACHE, &mut absorbed), NORMAL);
    unsafe { &mut *ctx.runtime }.roots.push((absorbed.record, Repr::Ref));
    absorbed
}

#[test]
fn champ_r5_a_split_and_a_replacement_leave_the_entries_where_they_were() {
    let mut runtime = load_r5(false, Engine::Jit);
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = frame_pointer!();
    let mut state = owner(&mut ctx);
    let scope_owner = owner_of(&runtime, &state);
    let inputs = empty(&mut ctx);
    let f = new_future(&mut ctx, MAKE, &[], &mut state, inputs);
    // A packet of three identities and F's entry; a split of the middle identity selects only it.
    let mut parts = vec![absorbed(&mut ctx, f)];
    let mut objects = Vec::new();
    for _ in 0..3 {
        let (object, packet) = object_of(&mut ctx, scope_owner, 0);
        objects.push(object);
        parts.push(packet);
    }
    let mut joined = PacketSlot::default();
    assert_eq!(rt_own_join(&mut ctx, parts.as_mut_ptr(), parts.len() as u64, &mut joined), NORMAL);
    let plan = runtime.champ_register_selection(vec![1]);
    let mut split = SplitPost::default();
    assert_eq!(rt_own_split_static(&mut ctx, &mut joined, plan, &mut split), NORMAL, "{:?}", runtime.fatal);
    let entry = Service { holder: f, kind: SERVICE_CACHE, target: None };
    let rest = super::super::helpers::packet(&runtime, split.rest.record).unwrap();
    let selected = super::super::helpers::packet(&runtime, split.selected.record).unwrap();
    assert_eq!((&rest.services[..], &selected.services[..]), (&[entry][..], &[][..]));
    assert_eq!(selected.words().collect::<Vec<_>>(), [objects[1]]);
    // A replacement keeps the reserve's receipts; the replaced packet returns without them.
    let mut pending = prepare_return(&mut ctx, f);
    assert_eq!(rehome_token(&mut ctx, &mut pending, f, TARGET_CALLER), NORMAL);
    let mut extra = split.selected;
    let mut extended = PendingSlot::default();
    assert_eq!(rt_exit_extend(&mut ctx, &mut pending, &mut extra, &mut extended), NORMAL);
    let mut replacement_packet = empty(&mut ctx);
    let selection = runtime.champ_register_selection(vec![]);
    let replacement = StaticReplacement { value: 0, value_repr: Repr::Raw as u64, packet: &mut replacement_packet,
        selection, boundary: 0 };
    let mut replaced = ReplacePost::default();
    assert_eq!(rt_exit_replace(&mut ctx, &mut extended, &replacement, &mut replaced), NORMAL, "{:?}", runtime.fatal);
    let reserve = reserve_services(&runtime, &replaced.pending);
    assert_eq!(reserve, [Service { target: Some(TARGET_CALLER), ..entry }]);
    let returned = super::super::helpers::packet(&runtime, replaced.rest.record).unwrap();
    assert!(returned.services.is_empty());
    assert_eq!(returned.words().collect::<Vec<_>>(), [objects[1]]);
}

#[test]
fn champ_r5_a_settled_duty_or_an_unknown_kind_joins_no_packet() {
    let mut runtime = load_r5(false, Engine::Jit);
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = frame_pointer!();
    let mut state = owner(&mut ctx);
    let inputs = empty(&mut ctx);
    let f = new_future(&mut ctx, MAKE, &[], &mut state, inputs);
    let mut packet = empty(&mut ctx);
    let mut post = PacketSlot::default();
    let outcome = rt_service_absorb(&mut ctx, &mut packet, f, 7, &mut post);
    stopped(&mut ctx, outcome, "unknown service kind");
    // The entry was absorbed, then the duty settled anyway: admitting the stale entry is refused.
    let mut stale = absorbed(&mut ctx, f);
    let finished = close_with(&mut ctx, &mut state, Close { pending: None, failure: 0, duties: Some(&[f]) });
    assert_eq!(finished.failure, 0);
    let mut other = owner(&mut ctx);
    let mut admitted = ScopeSlot::default();
    let outcome = rt_own_admit(&mut ctx, &mut other, &mut stale, &mut admitted);
    stopped(&mut ctx, outcome, "a settled cache duty in a packet");
}

#[test]
fn champ_r5_publish_refuses_a_receipt_in_its_outcome() {
    let mut runtime = load_r5(false, Engine::Jit);
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = frame_pointer!();
    let mut state = owner(&mut ctx);
    let h = gate(&mut ctx, &mut state);
    let inputs = empty(&mut ctx);
    let f = new_future(&mut ctx, MAKE, &[], &mut state, inputs);
    // Only a reserve holds receipts; a forged one in an outcome is refused.
    let receipt = Service { holder: f, kind: SERVICE_CACHE, target: Some(TARGET_CALLER) };
    let forged = super::super::helpers::store(&mut runtime, Data::Packet(Packet {
        services: vec![receipt], ..Packet::default()
    }));
    runtime.roots.push((forged, Repr::Ref));
    let mut packet = PacketSlot { record: forged };
    let published = PublishIn { tag: 0, value: 0, packet: &mut packet, failure: 0 };
    let outcome = rt_future_publish(&mut ctx, h, &published);
    stopped(&mut ctx, outcome, "an outcome packet outside its cache");
}

#[test]
fn champ_r5_a_future_passed_with_its_duty_settles_from_the_inputs() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_r5(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        // F ran and caches Q(1); W, never started, takes F as its argument and F's duty in its inputs.
        let inputs = empty(&mut ctx);
        let f = new_future(&mut ctx, MAKE, &[], &mut state, inputs);
        runtime.champ_future_start(&mut ctx, 0, f);
        let inputs = absorbed(&mut ctx, f);
        let w = new_future(&mut ctx, WATCH, &[f], &mut state, inputs);
        let map = runtime.champ_frame_map(WATCH as usize).unwrap().clone();
        let accepted = super::super::helpers::packet(&runtime, get(w, map.packet().unwrap())).unwrap();
        assert_eq!(accepted.services, [Service { holder: f, kind: SERVICE_CACHE, target: None }]);
        // The close settles W (cold: Cancelled), whose released inputs settle F first: Q is destroyed once.
        let finished = close_with(&mut ctx, &mut state, Close { pending: None, failure: 0, duties: Some(&[w]) });
        assert_eq!(finished.failure, 0);
        assert_eq!(destroyed(&runtime), "Res 1\n");
        assert_eq!(runtime.champ_cache(f).unwrap().flags, RELEASED);
        assert_eq!(kind(&runtime, get(w, tasks::FAILURE)), "Cancelled");
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

#[test]
fn champ_r5_a_rehomed_cold_future_takes_its_inputs_to_t() {
    let mut runtime = load_r5(true, Engine::Reference);
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = frame_pointer!();
    let Escape { mut t, mut s, .. } = escape(&mut ctx, 0);
    let t_scope = super::super::helpers::scope(&runtime, t.record).unwrap();
    let g = gate(&mut ctx, &mut s);
    let (r, inputs) = object_of(&mut ctx, owner_of(&runtime, &s), 0);
    let w = new_future(&mut ctx, WATCH, &[g], &mut s, inputs);
    let mut pending = prepare_return(&mut ctx, w);
    assert_eq!(rehome_token(&mut ctx, &mut pending, w, t_scope.owner.get()), NORMAL);
    open(&mut ctx, g, 0);
    let mut finished = close_with(&mut ctx, &mut s, Close {
        pending: Some(&mut pending), failure: 0, duties: Some(&[g]),
    });
    assert_eq!(finished.failure, 0);
    let destination = ExitDestination { holder: t_scope.holder, owner: t_scope.owner };
    let mut commit = CommitPost::default();
    assert_eq!(rt_exit_commit(&mut ctx, &mut finished.pending, &destination, &mut commit), NORMAL);
    // The cold Future's inputs answer to T now, as its cache does.
    assert_eq!(runtime.heap.champ_identity(r).unwrap().owner_id, t_scope.owner.get());
    admit(&mut ctx, &mut t, commit.packet);
    let finished = close_with(&mut ctx, &mut t, Close { pending: None, failure: 0, duties: None });
    assert_eq!(finished.failure, 0);
    assert_eq!(life(&runtime, r), Life::Dead);
    assert!(steps(&runtime, WATCH).is_empty());
}

#[test]
fn champ_r5_a_running_spawned_receipt_detaches_with_its_task() {
    let mut runtime = load_r5(true, Engine::Jit);
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = frame_pointer!();
    let Escape { mut s, .. } = escape(&mut ctx, 0);
    // A running Future whose task duty travels with its cache (task first, D24.1).
    let g = gate(&mut ctx, &mut s);
    let mut pending = prepare_return(&mut ctx, g);
    let mut cache = CacheSlot { record: g };
    let mut task = TaskSlot { record: g };
    let mut post = RehomePost::default();
    let outcome = rt_cache_rehome(&mut ctx, &mut pending, std::ptr::null_mut(), &mut cache, &mut task, g,
        TARGET_CALLER, &mut post);
    assert_eq!(outcome, NORMAL, "{:?}", runtime.fatal);
    assert_eq!(task.record, 0);
    let kinds = reserve_services(&runtime, &post.pending).iter().map(|entry| entry.kind).collect::<Vec<_>>();
    assert_eq!(kinds, [SERVICE_TASK, SERVICE_CACHE]);
    runtime.roots.push((post.pending.record, Repr::Ref));
    // S's close fails: the task detaches to the root, not cancelled, and its cache goes with it.
    let e = failure(&mut runtime, "E");
    let finished = close_with(&mut ctx, &mut s, Close {
        pending: Some(&mut post.pending), failure: e, duties: Some(&[]),
    });
    assert_eq!(finished.failure, e);
    let flags = get(g, tasks::FLAGS);
    assert_ne!(flags & DETACHED, 0);
    assert_eq!(flags & tasks::champ_flags::CANCEL_REQUESTED, 0);
    assert_eq!(get(g, tasks::STATE), tasks::RUNNING);
    assert_eq!(runtime.champ_cache(g).unwrap().flags, 0);
    assert_eq!(runtime.spawned.iter().filter(|&&frame| frame == g).count(), 1);
    // S's own C is destroyed; the escape's F is not one of this close's duties.
    assert_eq!(destroyed(&runtime), "Res 2\n");
    // A task entry alone (its cache duty is elsewhere) detaches its running Future too.
    let mut state = owner(&mut ctx);
    let h = gate(&mut ctx, &mut state);
    let mut packet = empty(&mut ctx);
    let mut absorbed = PacketSlot::default();
    assert_eq!(rt_service_absorb(&mut ctx, &mut packet, h, SERVICE_TASK, &mut absorbed), NORMAL);
    admit(&mut ctx, &mut state, absorbed);
    let finished = close_with(&mut ctx, &mut state, Close { pending: None, failure: 0, duties: None });
    assert_eq!(finished.failure, 0);
    assert_ne!(get(h, tasks::FLAGS) & DETACHED, 0);
    assert_eq!(runtime.spawned.iter().filter(|&&frame| frame == h).count(), 1);
}

#[test]
fn champ_r5_async_defer_publishes_no_packet_and_keeps_its_operand() {
    let mut runtime = load_r5(false, Engine::Jit);
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = frame_pointer!();
    let mut state = owner(&mut ctx);
    let this = gate(&mut ctx, &mut state);
    // `make` publishes an outcome packet: an async defer has none to give.
    let input = empty(&mut ctx);
    let mut defer = Defer::new(input, 0);
    let outcome = rt_invoke_defer_async(&mut ctx, this, u64::from(MAKE), defer.args.as_ptr(), 0, &mut defer.io,
        &defer.registration, &mut defer.post);
    stopped(&mut ctx, outcome, "published an outcome packet");
    // The operand a PENDING step left must come back unchanged.
    let this = gate(&mut ctx, &mut state);
    let g = gate(&mut ctx, &mut state);
    let input = empty(&mut ctx);
    let mut defer = Defer::new(input, g);
    assert_eq!(defer.step(&mut ctx, this), OUTCOME_PENDING);
    let (r, packet) = object_of(&mut ctx, owner_of(&runtime, &state), 0);
    let changed = super::super::helpers::packet(&runtime, packet.record).unwrap();
    super::super::helpers::replace(&mut runtime, defer.io.input.record, Data::Packet(changed));
    open(&mut ctx, g, 0);
    let child = get(this, tail(&runtime, this) + STEP);
    runtime.schedule(&mut ctx, child);
    let outcome = defer.step(&mut ctx, this);
    stopped(&mut ctx, outcome, "changed while it was pending");
    assert_eq!(life(&runtime, r), Life::Live);
}

// --- await.take and cache.rehome in SPB 17 (R4 forms) ----------------------------------------------------------------

#[test]
fn champ_r5_a_proved_temporary_take_runs_on_both_engines() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_r5(stress, engine);
        runtime.run_function(TAKER as usize, crate::STACK_BYTES).unwrap();
        // make ran once, on the taker's stack; its outcome went to the taker's scope, which destroyed x once.
        assert_eq!(steps(&runtime, MAKE), [FutureStep::Start, FutureStep::Published]);
        assert_eq!(steps(&runtime, TAKER), [FutureStep::Start, FutureStep::Published]);
        assert_eq!(destroyed(&runtime), "Res 1\n");
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

#[test]
fn champ_r5_a_rehomed_future_settles_in_the_exit_destination_on_both_engines() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_r5(stress, engine);
        runtime.run_function(ESCAPE as usize, crate::STACK_BYTES).unwrap();
        // The block's close destroyed C(1); F ran after the commit, and the body's close destroyed its Q(2).
        assert_eq!(destroyed(&runtime), "Res 1\nRes 2\n");
        assert_eq!(steps(&runtime, MAKE), [FutureStep::Start, FutureStep::Published]);
        assert_eq!(steps(&runtime, ESCAPE), [FutureStep::Start, FutureStep::Published]);
        traces.record(configuration, &runtime);
    }
    traces.agree();
}
