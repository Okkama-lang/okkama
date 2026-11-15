//! #601 K2: cancellation in the Champ runtime (Champ draft 3 §§4.1, 5; ABI C-3; OIR-2 §10.4; Bible §20.5).
//!
//! Each test states one rule of Bible §20.5 "Structured await and cancellation" with Tomato's observable behaviour
//! as the reference (the conformance cases `future.cancel-unstarted`, `cancel.awaited-cleanup-settles`,
//! `cancel.awaited-spawned`, `cancel.shared-awaited-survives`, `cancel.after-await-settled` and
//! `cancel.nested-frame-next-await` and the Bible's `20-cancel` example). The bodies are SPB 17 programs run, as in
//! the parent module, by the JIT and by the reference body, with the collector idle and with a collection at every
//! safepoint. Futures that SPB 17 cannot produce yet (a parked driver, a spawned task) are played by the test:
//! a gate is a running Future whose publication the test performs, and SPAWNED is set as `fut.spawn` (P1) sets it.
//! An async defer is driven through its helper, as in the R5 tests, because the JIT calls it only from D25 on.
use super::*;

const FUTURE_VOID: s::Type = s::Type::Future(1);
/// Function indices; each async function's frame layout, lexical scope and boundary have its index. `VALUE` is
/// the parent's `work`, so the parent's gates are Futures of it.
const VALUE: u32 = WORK;
const CONSUME_K: u32 = 1;
const DELIVERY: u32 = 2;
const ORDER: u32 = 3;
const AFTER: u32 = 4;
const OUTER: u32 = 5;
const UNSTARTED: u32 = 6;
const DIRECT: u32 = 7;
const INNER: u32 = 8;
const WORKER: u32 = 9;
const ASYNC_K: u32 = 10;

// --- SPB 17 programs ------------------------------------------------------------------------------------------

/// `let x = Res();` committed to the scope of `boundary`: the object and its packet.
fn create(b: &mut Body, boundary: u32) -> (Location, Location) {
    let object = b.register(s::Type::Object(0));
    let Location::Register(id) = object else { unreachable!() };
    b.f.code.push(s::Instruction::Data { op: s::DataOp::ObjNew, inputs: vec![], output: Some(id),
        immediate: s::Immediate::Object(0) });
    let packet = b.emit(O::OwnCreateCommit, vec![l(object), A::Object(0), A::Boundary(boundary)], K::Packet);
    (object, packet)
}

/// Return `value` (or nothing) through a merging close of `state` that settles `duties`, then publish it.
fn return_merging(b: &mut Body, body: Location, state: Location, scope: u32, value: Option<Location>,
    duties: &[Location], label: &'static str)
{
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let mut arguments = vec![A::Kind(s::ExitKind::Return), A::Boundary(scope)];
    arguments.extend(value.map(l));
    arguments.extend([l(empty), A::Selection(selection)]);
    let pending = b.emit(O::ExitPrepare, arguments, K::Pending);
    let finished = b.close(state, Some(pending), None, scope, duties, true);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, label, vec![p(0), p(1)])]);
    b.block(label, &[closed, reserved]);
    let ty = value.map(|value| match value {
        Location::Register(id) => b.f.registers[id as usize],
        Location::Service(_) => unreachable!(),
    });
    let mut outputs = ty.map(|ty| vec![b.register(ty)]).unwrap_or_default();
    outputs.extend([b.service(K::Packet), b.service(K::Deps)]);
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(s::ExitDestination::Caller)], outputs.clone());
    let mut published = vec![l(body), A::Flag(true)];
    published.extend(outputs.into_iter().map(l));
    b.next(O::FuturePublish, published, vec![]);
    b.ret();
}

/// Close `state` with the active `failure` through a merging plan that settles `duties`, and publish the result.
fn fail_merging(b: &mut Body, body: Location, state: Location, failure: Location, scope: u32, duties: &[Location],
    label: &'static str)
{
    let finished = b.close(state, None, Some(failure), scope, duties, true);
    let [closed, combined] = [b.service(K::Closed), b.register(s::Type::Failure)];
    b.dispatch(finished, [false, true, false], vec![(Tag::Failure, label, vec![p(0), p(1)])]);
    b.block(label, &[closed, combined]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(false), l(combined)], vec![]);
    b.ret();
}

/// One merging plan of close `close` over the scope and the reserve, run by its executor.
fn final_plan(b: &mut Body, cleanup: Location, close: u32) -> Location {
    let selection = b.selection();
    let proofs = &mut b.f.proofs;
    proofs.order_witnesses.push(s::OrderWitness { creation_pcs: vec![] });
    let plan = proofs.plans.len() as u32;
    proofs.plans.push(s::Plan { merge: true, selection, order_witness: proofs.order_witnesses.len() as u32 - 1 });
    proofs.cleanup_plans.push(s::CleanupPlan { plan, reserve: true, close, reserve_empty: false,
        sources: vec![s::MergeSource::Scope, s::MergeSource::Reserve], slots: vec![], preserves: vec![], joins: vec![],
        suffixes: vec![] });
    let planned = b.emit(O::CleanupPlan, vec![l(cleanup), A::Plan(plan)], K::Cleanup);
    b.executor(planned)
}

/// Return `value` through the final close of an async body as the compiler emits it (D18, `cancel=true`): settle
/// `duties`, the first plan, `cleanup.deliver_cancel`, the aborted reserve's plan, then publish the success or the
/// close's failure.
fn return_final(b: &mut Body, body: Location, state: Location, scope: u32, value: Location, duties: &[Location],
    labels: (&'static str, &'static str))
{
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let arguments = vec![A::Kind(s::ExitKind::Return), A::Boundary(scope), l(value), l(empty),
        A::Selection(selection)];
    let pending = b.emit(O::ExitPrepare, arguments, K::Pending);
    let proofs = &mut b.f.proofs;
    let close = proofs.closes.len() as u32;
    let envelope = proofs.envelopes.len() as u32;
    proofs.envelopes.push(s::Envelope { failures: s::ENVELOPE_CANCEL });
    proofs.closes.push(s::CloseContract { begin_pc: b.f.code.len() as u32, boundary: scope, reserve: true,
        active_failure: false, envelope });
    let schedule = proofs.schedules.len() as u32;
    proofs.schedules.push(s::Schedule { services: duties.to_vec() });
    let arguments = vec![l(state), l(pending), A::Boundary(scope), A::Envelope(envelope)];
    let mut cleanup = b.emit(O::CleanupBegin, arguments, K::Cleanup);
    if duties.is_empty() {
        cleanup = b.emit(O::CleanupSettle, vec![l(cleanup), A::Schedule(schedule)], K::Cleanup);
    }
    for &duty in duties {
        cleanup = b.emit(O::CleanupSettle, vec![l(cleanup), A::Schedule(schedule), l(duty)], K::Cleanup);
    }
    cleanup = final_plan(b, cleanup, close);
    cleanup = b.emit(O::CleanupDeliverCancel, vec![l(cleanup)], K::Cleanup);
    cleanup = final_plan(b, cleanup, close);
    let finished = [b.service(K::Closed), b.service(K::FailureState), b.service(K::ExitState)];
    b.next(O::CleanupFinish, vec![l(cleanup)], finished.to_vec());
    let (done, failed) = labels;
    b.dispatch((finished[0], finished[1], finished[2]), [true, false, true],
        vec![(Tag::Pending, done, vec![p(0), p(1)]), (Tag::Failure, failed, vec![p(0), p(1)])]);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.block(done, &[closed, reserved]);
    let outputs = [b.register(s::Type::Int), b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(s::ExitDestination::Caller)], outputs.to_vec());
    b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(outputs[0]), l(outputs[1]), l(outputs[2])], vec![]);
    b.ret();
    let [closed, combined] = [b.service(K::Closed), b.register(s::Type::Failure)];
    b.block(failed, &[closed, combined]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(false), l(combined)], vec![]);
    b.ret();
}

/// `async consume(r: Res) { }`: its inputs hold `r`'s obligation; its own close merges.
fn consumer() -> (s::Function, s::FrameLayout) {
    let (mut b, frame) = Body::asynchronous("consume", CONSUME_K, vec![]);
    let start = [s::ProducedType::Value(s::Type::Object(0)), s::ProducedType::Service(K::Packet)];
    let (body, inputs) = b.resume(frame, 0, &start);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(CONSUME_K), A::Boundary(CONSUME_K)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[1])], K::ScopeState);
    return_merging(&mut b, body, q1, CONSUME_K, None, &[], "done");
    let layout = b.layout(vec![value(s::Type::Object(0)), service(K::Packet)], vec![(vec![0, 1], vec![])]);
    (b.finish(), layout)
}

/// With `inner` None, `async delivery(f: Future<int>) is int { let van = Res(); return await f; }`; with an inner
/// entry, `async order(f…) is int { let van = Res(); return await inner(f…); }`, whose await starts the inner
/// Future (a frame of its task) and whose close settles its cache duty. The vans show the cleanup order.
fn forward(symbol: &str, index: u32, inner: Option<u32>, params: u32, final_exit: bool)
    -> (s::Function, s::FrameLayout)
{
    let mut slots = (0..params).map(|_| value(FUTURE_INT)).collect::<Vec<_>>();
    let state_slot = params + 1;
    slots.extend([service(K::Packet), service(K::ScopeState)]);
    let (cache_slots, awaited_slot) = match inner {
        Some(_) => {
            slots.extend([service(K::CacheState), value(FUTURE_INT)]);
            (vec![params + 2], params + 3)
        }
        None => (vec![], 0),
    };
    let unwind_slots = (slots.len() as u32..slots.len() as u32 + 3).collect::<Vec<_>>();
    slots.extend([value(s::Type::Failure), service(K::TaskState), service(K::Receipt)]);
    let (mut b, frame) = Body::asynchronous(symbol, index, vec![s::Type::Int]);
    let start = (0..=params).collect::<Vec<_>>();
    let (body, inputs) = b.resume(frame, 0, &types(&slots, &start));
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(index), A::Boundary(index)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[params as usize])], K::ScopeState);
    let (_, van) = create(&mut b, index);
    let q2 = b.emit(O::OwnAdmit, vec![l(q1), l(van)], K::ScopeState);
    let (awaited, state, cache) = match inner {
        Some(entry) => {
            let (future, cache, q3) = future_new(&mut b, index, entry, &inputs[..params as usize], q2);
            (future, q3, vec![cache])
        }
        None => (inputs[0], q2, vec![]),
    };
    let saved_slots = [vec![state_slot], cache_slots.clone()].concat();
    let first = b.save(body, 1, &[vec![state], cache].concat());
    b.outcomes(O::Await, vec![l(awaited)], vec![(Tag::Normal, "ok", vec![p(0), p(1)]),
        (Tag::Unwind, "bad", vec![p(0), p(1), p(2)])]);
    let [x, deps] = [b.register(s::Type::Int), b.service(K::Deps)];
    b.block("ok", &[x, deps]);
    let (body_ok, resumed) = b.resume(first, 1, &types(&slots, &saved_slots));
    if final_exit {
        return_final(&mut b, body_ok, resumed[0], index, x, &resumed[1..], ("done", "done_failed"));
    } else {
        return_merging(&mut b, body_ok, resumed[0], index, Some(x), &resumed[1..], "done");
    }
    let unwind = [b.register(s::Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block("bad", &unwind);
    let (body_bad, resumed) = b.resume(first, 1, &types(&slots, &saved_slots));
    let (body_adapted, after, adapted, kept) =
        b.adapter(body_bad, 2, resumed[0], unwind, &resumed[1..], &types(&slots, &cache_slots));
    fail_merging(&mut b, body_adapted, after, adapted, index, &kept, "failed");
    let layout = b.layout(slots, vec![(start, vec![]), (saved_slots, vec![awaited_slot]),
        (cache_slots, [vec![state_slot], unwind_slots].concat())]);
    (b.finish(), layout)
}

/// `async after(g: Future<int>, h: Future<int>) is int { let a = await g; let b = await h; return a + b; }`.
fn after() -> (s::Function, s::FrameLayout) {
    // 0 g, 1 h, 2 inputs, 3 owner state, 4 a, 5–7 the unwind.
    let slots = vec![value(FUTURE_INT), value(FUTURE_INT), service(K::Packet), service(K::ScopeState),
        value(s::Type::Int), value(s::Type::Failure), service(K::TaskState), service(K::Receipt)];
    let (mut b, frame) = Body::asynchronous("after", AFTER, vec![s::Type::Int]);
    let (body, inputs) = b.resume(frame, 0, &types(&slots, &[0, 1, 2]));
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(AFTER), A::Boundary(AFTER)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[2])], K::ScopeState);
    let first = b.save(body, 1, &[q1, inputs[1]]);
    b.outcomes(O::Await, vec![l(inputs[0])], vec![(Tag::Normal, "ok1", vec![p(0), p(1)]),
        (Tag::Unwind, "bad1", vec![p(0), p(1), p(2)])]);
    let [a, a_deps] = [b.register(s::Type::Int), b.service(K::Deps)];
    b.block("ok1", &[a, a_deps]);
    let (body_first, resumed) = b.resume(first, 1, &types(&slots, &[3, 1]));
    let second = b.save(body_first, 2, &[a, resumed[0]]);
    b.outcomes(O::Await, vec![l(resumed[1])], vec![(Tag::Normal, "ok2", vec![p(0), p(1)]),
        (Tag::Unwind, "bad2", vec![p(0), p(1), p(2)])]);
    let [c, c_deps] = [b.register(s::Type::Int), b.service(K::Deps)];
    b.block("ok2", &[c, c_deps]);
    let (body_second, resumed) = b.resume(second, 2, &types(&slots, &[4, 3]));
    let sum = b.register(s::Type::Int);
    let (Location::Register(left), Location::Register(right), Location::Register(out)) = (resumed[0], c, sum) else {
        unreachable!()
    };
    b.f.code.push(s::Instruction::Data { op: s::DataOp::IntAdd, inputs: vec![left, right], output: Some(out),
        immediate: s::Immediate::None });
    return_merging(&mut b, body_second, resumed[1], AFTER, Some(sum), &[], "done");
    for (k, label, source, saved) in [(3, "bad1", first, vec![3, 1]), (4, "bad2", second, vec![4, 3])] {
        let unwind = [b.register(s::Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
        b.block(label, &unwind);
        let (body_bad, resumed) = b.resume(source, k - 2, &types(&slots, &saved));
        let state = if k == 3 { resumed[0] } else { resumed[1] };
        let (body_adapted, closing, adapted, _) = b.adapter(body_bad, k, state, unwind, &[], &[]);
        fail_merging(&mut b, body_adapted, closing, adapted, AFTER, &[], if k == 3 { "failed1" } else { "failed2" });
    }
    let layout = b.layout(slots, vec![(vec![0, 1, 2], vec![]), (vec![3, 1], vec![0]), (vec![4, 3], vec![1]),
        (vec![], vec![3, 5, 6, 7]), (vec![], vec![3, 5, 6, 7])]);
    (b.finish(), layout)
}

/// `async unstarted(f: Future<void>) is bool { f.cancel(); f.cancel(); try { await f; false } catch e is Cancelled
/// { f.cancelled() } }`, the body of conformance `L08.future.cancel-unstarted` with `f` passed in.
fn unstarted() -> (s::Function, s::FrameLayout) {
    // 0 f, 1 inputs, 2 owner state, 3–5 the unwind.
    let slots = vec![value(FUTURE_VOID), service(K::Packet), service(K::ScopeState), value(s::Type::Failure),
        service(K::TaskState), service(K::Receipt)];
    let (mut b, frame) = Body::asynchronous("unstarted", UNSTARTED, vec![s::Type::Bool]);
    let (body, inputs) = b.resume(frame, 0, &types(&slots, &[0, 1]));
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(UNSTARTED), A::Boundary(UNSTARTED)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[1])], K::ScopeState);
    b.next(O::FutureCancel, vec![l(inputs[0])], vec![]);
    b.next(O::FutureCancel, vec![l(inputs[0])], vec![]);
    let first = b.save(body, 1, &[q1]);
    b.outcomes(O::Await, vec![l(inputs[0])], vec![(Tag::Normal, "ok", vec![p(0)]),
        (Tag::Unwind, "bad", vec![p(0), p(1), p(2)])]);
    let deps = b.service(K::Deps);
    b.block("ok", &[deps]);
    let (body_ok, resumed) = b.resume(first, 1, &types(&slots, &[2]));
    let no = b.constant(s::Constant::Bool(false), s::Type::Bool);
    return_merging(&mut b, body_ok, resumed[0], UNSTARTED, Some(no), &[], "done");
    let unwind = [b.register(s::Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block("bad", &unwind);
    let (body_bad, resumed) = b.resume(first, 1, &types(&slots, &[2]));
    let (body_caught, closing, _, kept) =
        b.adapter(body_bad, 2, resumed[0], unwind, &[inputs[0]], &types(&slots, &[0]));
    let cancelled = b.register(s::Type::Bool);
    b.next(O::FutureCancelled, vec![l(kept[0])], vec![cancelled]);
    return_merging(&mut b, body_caught, closing, UNSTARTED, Some(cancelled), &[], "caught");
    let layout = b.layout(slots, vec![(vec![0, 1], vec![]), (vec![2], vec![0]), (vec![0], vec![2, 3, 4, 5])]);
    (b.finish(), layout)
}

/// `async worker(g: Future<int>) is int { let f = inner(g); let v = try { await f } catch e is Cancelled { -2 };
/// return v; }`, the worker of the `window` probe, with the final exit of the compiler (D18).
fn worker() -> (s::Function, s::FrameLayout) {
    // 0 g, 1 inputs, 2 owner state, 3 the cache duty, 4 f, 5–7 the unwind.
    let slots = vec![value(FUTURE_INT), service(K::Packet), service(K::ScopeState), service(K::CacheState),
        value(FUTURE_INT), value(s::Type::Failure), service(K::TaskState), service(K::Receipt)];
    let (mut b, frame) = Body::asynchronous("worker", WORKER, vec![s::Type::Int]);
    let (body, inputs) = b.resume(frame, 0, &types(&slots, &[0, 1]));
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(WORKER), A::Boundary(WORKER)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[1])], K::ScopeState);
    let (future, cache, q2) = future_new(&mut b, WORKER, INNER, &[inputs[0]], q1);
    let first = b.save(body, 1, &[q2, cache]);
    b.outcomes(O::Await, vec![l(future)], vec![(Tag::Normal, "ok", vec![p(0), p(1)]),
        (Tag::Unwind, "bad", vec![p(0), p(1), p(2)])]);
    let [x, deps] = [b.register(s::Type::Int), b.service(K::Deps)];
    b.block("ok", &[x, deps]);
    let (body_ok, resumed) = b.resume(first, 1, &types(&slots, &[2, 3]));
    return_final(&mut b, body_ok, resumed[0], WORKER, x, &[resumed[1]], ("done", "done_failed"));
    let unwind = [b.register(s::Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block("bad", &unwind);
    let (body_bad, resumed) = b.resume(first, 1, &types(&slots, &[2, 3]));
    let (body_caught, closing, _, kept) =
        b.adapter(body_bad, 2, resumed[0], unwind, &[resumed[1]], &types(&slots, &[3]));
    let caught = b.constant(s::Constant::Int("-2".into()), s::Type::Int);
    return_final(&mut b, body_caught, closing, WORKER, caught, &[kept[0]], ("caught", "caught_failed"));
    let layout = b.layout(slots, vec![(vec![0, 1], vec![]), (vec![2, 3], vec![4]), (vec![3], vec![2, 5, 6, 7])]);
    (b.finish(), layout)
}

fn k2_program() -> s::Program {
    let mut functions = Vec::new();
    let mut frames = Vec::new();
    for (function, layout) in [work(), consumer(), forward("delivery", DELIVERY, None, 1, false),
        forward("order", ORDER, Some(DELIVERY), 1, false), after(), forward("outer", OUTER, Some(AFTER), 2, false),
        unstarted(), forward("direct", DIRECT, None, 1, true), forward("inner", INNER, None, 1, true), worker()]
    {
        functions.push(function);
        frames.push(layout);
    }
    functions.push(destroy());
    s::Program {
        module: "cancellation".into(),
        objects: vec![res()],
        functions,
        tables: s::ChampTables {
            scopes: (0..ASYNC_K).map(|index| s::Scope { function: index, parent: None, boundary: index }).collect(),
            boundaries: (0..ASYNC_K).map(|scope| s::Boundary { scope }).collect(),
            frames,
            future_results: vec![Some(s::Type::Int), None],
            ..s::ChampTables::default()
        },
        ..s::Program::default()
    }
}

/// [`k2_program`], through its text form, on a Champ runtime whose async bodies `engine` runs; the destroys are
/// recorded as `Object serial` lines.
fn load_k2(stress: bool, engine: Engine) -> Box<Runtime> {
    let built = k2_program();
    s::verify_load(&built, &s::NoExternalContracts).unwrap();
    let assembled = s::assemble(&s::disassemble(&built).unwrap()).unwrap();
    assert_eq!(assembled, built);
    let mut runtime = Runtime::new_champ(assembled, Vec::new()).unwrap();
    runtime.heap.set_stress(stress);
    runtime.champ.captured_trace = Some(Vec::new());
    if engine == Engine::Reference {
        for function in 0..ASYNC_K as usize {
            runtime.invocations[function].code = reference as *const () as u64;
        }
    }
    runtime
}

// --- Harness ----------------------------------------------------------------------------------------------------

use tasks::champ_flags::{CANCEL_DELIVERED, CANCEL_REQUESTED, OBSERVED, SHIELDED, SPAWNED};

/// The destroys so far, `Object serial` per line.
fn destroyed(runtime: &Runtime) -> String {
    String::from_utf8(runtime.champ.captured_trace.clone().unwrap()).unwrap()
}

fn flags(frame: Word) -> u64 {
    get(frame, tasks::FLAGS)
}

fn mark(frame: Word, flag: u64) {
    set(frame, tasks::FLAGS, flags(frame) | flag);
}

/// A new Future of `entry` with no input packet, started as its own task (a root or a spawned task).
fn task(ctx: &mut Ctx, entry: u32, args: &[Word], state: &mut ScopeSlot, spawned: bool) -> Word {
    let inputs = empty(ctx);
    let future = new_future(ctx, entry, args, state, inputs);
    if spawned {
        mark(future, SPAWNED);
    }
    let runtime = unsafe { &mut *ctx.runtime };
    runtime.champ_future_start(ctx, 0, future);
    assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
    future
}

/// A cold Future of `value(n)`.
fn cold(ctx: &mut Ctx, state: &mut ScopeSlot, n: i64) -> Word {
    let inputs = empty(ctx);
    new_future(ctx, VALUE, &[word::small(n).unwrap()], state, inputs)
}

/// The gate's publication of a Cancelled failure, as its body would publish the delivered request.
fn close_gate_cancelled(ctx: &mut Ctx, gate: Word) {
    let runtime = unsafe { &mut *ctx.runtime };
    let failure = runtime.cancelled_failure();
    runtime.roots.push((failure, Repr::Ref));
    let mut outcome = empty(ctx);
    let published = PublishIn { tag: 1, value: 0, packet: &mut outcome, failure };
    assert_eq!(rt_future_publish(ctx, gate, &published), NORMAL);
}

fn cancelled(ctx: &mut Ctx, future: Word) -> bool {
    let mut flag = FlagPost::default();
    assert_eq!(rt_future_cancelled(ctx, future, &mut flag), NORMAL);
    flag.value == 1
}

/// The pc of the `nth` await of `function`.
fn await_pc(runtime: &Runtime, function: u32, nth: usize) -> u32 {
    let program = &runtime.champ.machine.as_ref().unwrap().source;
    program.functions[function as usize].code.iter().enumerate()
        .filter(|(_, instruction)| matches!(instruction, s::Instruction::Champ { op: O::Await, .. }))
        .nth(nth).unwrap().0 as u32
}

/// The innermost activation a failure unwound through: (function, pc).
fn raised_at(runtime: &Runtime, failure: Word) -> (u32, u32) {
    runtime.failure(failure).unwrap().trace[0]
}

fn failed_cancelled(runtime: &Runtime, future: Word) -> bool {
    get(future, tasks::STATE) == tasks::FAILURE_STATE && kind(runtime, get(future, tasks::FAILURE)) == "Cancelled"
}

// --- Bible §20.5 -------------------------------------------------------------------------------------------------

/// "A future that never started is simply marked cancelled; its body never runs" (conformance
/// `L08.future.cancel-unstarted`, plan K2 "cancel before start"): a repeated request is safe, the await throws
/// Cancelled, `cancelled()` is then true, and the Future's inputs stay in it until its owner's close settles the
/// duty (draft 3 §5: cancel never moves a packet), which destroys them once.
#[test]
fn champ_k2_a_future_cancelled_before_start_never_runs_and_its_await_throws_cancelled() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_k2(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let scope_owner = super::helpers::scope(&runtime, state.record).unwrap().owner;
        let (r, inputs) = resource(&mut ctx, scope_owner);
        let f = new_future(&mut ctx, CONSUME_K, &[r], &mut state, inputs);
        let main = task(&mut ctx, UNSTARTED, &[f], &mut state, false);
        assert_eq!((get(main, tasks::STATE), get(main, tasks::RESULT)), (tasks::DONE, 1), "-1, then cancelled");
        assert!(steps(&runtime, CONSUME_K).is_empty(), "the body never runs");
        assert_eq!(runtime.champ.receipts, [(UNSTARTED as usize, RECEIPT_SETTLED)]);
        assert!(failed_cancelled(&runtime, f));
        assert_ne!(flags(f) & OBSERVED, 0);
        assert!(cancelled(&mut ctx, f));
        // The inputs wait in the Future for its owner.
        assert!(!cleared(&runtime, f));
        assert_eq!(runtime.heap.champ_identity(r).unwrap().life, Life::Live);
        assert_eq!(destroyed(&runtime), "");
        let before = (get(f, tasks::STATE), get(f, tasks::FAILURE), flags(f));
        assert_eq!(rt_cancel_request(&mut ctx, f), NORMAL);
        assert_eq!((get(f, tasks::STATE), get(f, tasks::FAILURE), flags(f)), before);
        assert_eq!(close(&mut ctx, &mut state, &[f, main]), 0);
        assert_eq!(destroyed(&runtime), "Res 1\n");
        assert!(cleared(&runtime, f));
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

/// "Cancelling a task cancels the future it awaits, if that future is its own … And the cancelled task does not
/// finish until that future has settled, cleanup included. Only then does its own await throw Cancelled and its
/// own cleanup run" (Bible `20-cancel`, conformance `cancel.awaited-spawned` and `cancel.awaited-cleanup-settles`).
/// The order awaits the delivery its await started; the delivery is the sole awaiter of a spawned child. `cancel()`
/// returns at once and runs nothing; the child is cancelled; the delivery keeps waiting for it and, once it has
/// settled (with a failure or, ignoring the request, a value), throws Cancelled; its van is destroyed before the
/// order's.
#[test]
fn champ_k2_cancelling_a_task_cancels_its_own_child_and_waits_for_it() {
    for child_fails in [true, false] {
        let mut traces = Traces::default();
        for configuration in configurations() {
            let (stress, engine) = configuration;
            let mut runtime = load_k2(stress, engine);
            let mut ctx = context(&mut runtime);
            ctx.entry_frame = frame_pointer!();
            let mut state = owner(&mut ctx);
            let child = gate(&mut ctx, &mut state);
            mark(child, SPAWNED);
            let order = task(&mut ctx, ORDER, &[child], &mut state, true);
            let delivery = get(order, tasks::AWAITING);
            runtime.roots.push((delivery, Repr::Ref));
            assert_eq!(runtime.function_of(delivery), DELIVERY as usize);
            assert_eq!(runtime.champ_task_of(delivery).unwrap(), order, "a frame its await started");
            assert_eq!(get(delivery, tasks::AWAITING), child);
            // `cancel()` returns at once: nothing runs, nothing is queued, no cleanup happens.
            let events = runtime.champ.future_events.len();
            assert_eq!(rt_cancel_request(&mut ctx, order), NORMAL);
            assert_eq!(runtime.champ.future_events.len(), events);
            assert!(runtime.ready.is_empty());
            assert_eq!(destroyed(&runtime), "");
            assert_eq!(flags(order) & (CANCEL_REQUESTED | CANCEL_DELIVERED), CANCEL_REQUESTED | CANCEL_DELIVERED);
            assert_ne!(flags(child) & CANCEL_REQUESTED, 0, "the child is cancelled with its sole awaiter");
            // The delivery keeps waiting for the child.
            assert_ne!(flags(delivery) & tasks::DELIVER_NOW, 0);
            assert_eq!(get(delivery, tasks::AWAITING), child);
            assert_eq!(get(child, tasks::WAITER), delivery);
            // A repeated request changes nothing.
            let before = [order, delivery, child].map(flags);
            assert_eq!(rt_cancel_request(&mut ctx, order), NORMAL);
            assert_eq!([order, delivery, child].map(flags), before);
            assert!(runtime.ready.is_empty());
            if child_fails {
                close_gate_cancelled(&mut ctx, child);
            } else {
                open_gate(&mut ctx, child, 5);
            }
            runtime.schedule(&mut ctx, order);
            assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
            assert!(failed_cancelled(&runtime, delivery));
            assert!(failed_cancelled(&runtime, order));
            assert!(cancelled(&mut ctx, order));
            assert_eq!(destroyed(&runtime), "Res 2\nRes 1\n", "the delivery's van, then the order's");
            // The structured receipt of the sole awaiter, then the order's view of the settled delivery.
            assert_eq!(runtime.champ.receipts,
                [(DELIVERY as usize, RECEIPT_CHILD_CANCELLED), (ORDER as usize, RECEIPT_SETTLED)]);
            use FutureStep::*;
            for function in [ORDER, DELIVERY] {
                assert_eq!(steps(&runtime, function), [Start, Pending, Queued, Resume, Published]);
            }
            traces.record(configuration, &runtime);
        }
        traces.agree();
    }
}

/// "A future that another task also awaits is not cancelled with one of its waiters: the cancelled task only stops
/// waiting for it, and the others still get its outcome" (conformance `cancel.shared-awaited-survives`). The
/// cancelled waiter is withdrawn (receipt Withdrawn, draft 3 §4.1), also when the child settles before the waiter's
/// turn comes.
#[test]
fn champ_k2_a_shared_child_survives_a_cancelled_waiter_and_settles_for_the_other() {
    for child_settles_first in [false, true] {
        let mut traces = Traces::default();
        for configuration in configurations() {
            let (stress, engine) = configuration;
            let mut runtime = load_k2(stress, engine);
            let mut ctx = context(&mut runtime);
            ctx.entry_frame = frame_pointer!();
            let mut state = owner(&mut ctx);
            let child = gate(&mut ctx, &mut state);
            mark(child, SPAWNED);
            let first = task(&mut ctx, DELIVERY, &[child], &mut state, true);
            let second = task(&mut ctx, DELIVERY, &[child], &mut state, true);
            assert_eq!(rt_cancel_request(&mut ctx, first), NORMAL);
            // Withdrawn: the waiter is taken off the child and queued to raise; the child is not asked to stop.
            assert_eq!(get(first, tasks::AWAITING), word::UNBOUND);
            assert_eq!(flags(child) & CANCEL_REQUESTED, 0);
            assert_eq!(get(child, tasks::STATE), tasks::RUNNING);
            assert_eq!(get(second, tasks::AWAITING), child);
            assert!(runtime.champ_listens(child, second) && !runtime.champ_listens(child, first));
            assert_eq!(runtime.ready.iter().copied().collect::<Vec<_>>(), [first]);
            if child_settles_first {
                open_gate(&mut ctx, child, 5);
                assert_eq!(runtime.ready.iter().copied().collect::<Vec<_>>(), [first, second]);
            }
            runtime.schedule(&mut ctx, first);
            assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
            assert!(failed_cancelled(&runtime, first));
            assert_eq!(runtime.champ.receipts, [(DELIVERY as usize, RECEIPT_WITHDRAWN)]);
            assert_eq!(destroyed(&runtime), "Res 1\n");
            if !child_settles_first {
                open_gate(&mut ctx, child, 5);
            }
            runtime.schedule(&mut ctx, second);
            assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
            let value = word::small(5).unwrap();
            assert_eq!((get(second, tasks::STATE), get(second, tasks::RESULT)), (tasks::DONE, value));
            assert!(!cancelled(&mut ctx, second));
            assert!(!cancelled(&mut ctx, child));
            assert_eq!(destroyed(&runtime), "Res 1\nRes 2\n");
            traces.record(configuration, &runtime);
        }
        traces.agree();
    }
}

/// Draft 3 §4.1 C1, test `k2-cancel-after-await-settled` (#546, Bible "When the await has already finished",
/// conformance `cancel.after-await-settled`): the awaited Future settles and queues the task; the task is cancelled
/// before its turn. Its await is finished: it runs on with the value, the request stays, and its next await raises
/// Cancelled before it starts anything.
#[test]
fn champ_k2_cancel_after_await_settled_keeps_the_value_and_raises_at_the_next_await() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_k2(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let first = gate(&mut ctx, &mut state);
        let next = cold(&mut ctx, &mut state, 7);
        let worker = task(&mut ctx, AFTER, &[first, next], &mut state, true);
        open_gate(&mut ctx, first, 5);
        assert_eq!(runtime.ready.iter().copied().collect::<Vec<_>>(), [worker]);
        assert_eq!(rt_cancel_request(&mut ctx, worker), NORMAL);
        // Not waiting any more: the request is only recorded.
        assert_eq!(flags(worker) & (CANCEL_REQUESTED | CANCEL_DELIVERED | tasks::DELIVER_NOW), CANCEL_REQUESTED);
        assert_eq!(runtime.ready.iter().copied().collect::<Vec<_>>(), [worker]);
        runtime.schedule(&mut ctx, worker);
        assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
        assert!(failed_cancelled(&runtime, worker));
        assert_ne!(flags(worker) & CANCEL_DELIVERED, 0);
        // Raised by the second await, before it started the Future it awaits.
        let failure = get(worker, tasks::FAILURE);
        assert_eq!(raised_at(&runtime, failure), (AFTER, await_pc(&runtime, AFTER, 1)));
        assert_eq!(get(next, tasks::STATE), tasks::COLD);
        assert_eq!(flags(next) & OBSERVED, 0);
        assert_eq!(count(&runtime, VALUE, FutureStep::Start), 0);
        assert_eq!(runtime.champ.receipts, [(AFTER as usize, RECEIPT_NOT_STARTED)]);
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

/// The same rule for a frame of the task that the scheduler resumes alone (#670, conformance
/// `cancel.nested-frame-next-await`): the outer task awaits a frame its await started; that frame's awaited Future
/// settles and queues it; the task is cancelled. The frame takes the value, raises the task's cancellation at its
/// next await before starting anything, and the task then throws it.
#[test]
fn champ_k2_a_frame_of_a_cancelled_task_raises_at_its_next_await() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_k2(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let first = gate(&mut ctx, &mut state);
        let next = cold(&mut ctx, &mut state, 7);
        let outer = task(&mut ctx, OUTER, &[first, next], &mut state, true);
        let inner = get(outer, tasks::AWAITING);
        runtime.roots.push((inner, Repr::Ref));
        assert_eq!(runtime.function_of(inner), AFTER as usize);
        open_gate(&mut ctx, first, 5);
        assert_eq!(runtime.ready.iter().copied().collect::<Vec<_>>(), [inner]);
        assert_eq!(rt_cancel_request(&mut ctx, outer), NORMAL);
        assert_eq!(flags(outer) & (CANCEL_REQUESTED | CANCEL_DELIVERED), CANCEL_REQUESTED);
        assert_eq!(flags(inner) & tasks::DELIVER_NOW, 0);
        assert_eq!(runtime.ready.iter().copied().collect::<Vec<_>>(), [inner]);
        runtime.schedule(&mut ctx, outer);
        assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
        assert!(failed_cancelled(&runtime, inner));
        assert!(failed_cancelled(&runtime, outer));
        assert_ne!(flags(outer) & CANCEL_DELIVERED, 0);
        assert_eq!(raised_at(&runtime, get(inner, tasks::FAILURE)), (AFTER, await_pc(&runtime, AFTER, 1)));
        assert_eq!(get(next, tasks::STATE), tasks::COLD);
        assert_eq!(destroyed(&runtime), "Res 1\n", "the outer task's cleanup ran");
        assert_eq!(runtime.champ.receipts, [(AFTER as usize, RECEIPT_NOT_STARTED), (OUTER as usize, RECEIPT_SETTLED)]);
        use FutureStep::*;
        assert_eq!(steps(&runtime, AFTER), [Start, Pending, Queued, Resume, Published]);
        assert_eq!(steps(&runtime, OUTER), [Start, Pending, Queued, Resume, Published]);
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

// --- The final exit (ruling #601 K2 in OIR-2 async ABI: Tomato parity) -----------------------------------------

/// Probe `window`, direct form: the awaited Future settles and queues the task, the task is cancelled before its
/// turn, and no await follows. Tomato: the task completes with its value and `cancelled()` is false. The final
/// exit's D18 step delivers nothing; the request stays pending.
#[test]
fn champ_k2_a_request_pending_at_the_final_exit_leaves_the_result() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_k2(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let first = gate(&mut ctx, &mut state);
        let direct = task(&mut ctx, DIRECT, &[first], &mut state, true);
        open_gate(&mut ctx, first, 42);
        assert_eq!(rt_cancel_request(&mut ctx, direct), NORMAL);
        runtime.schedule(&mut ctx, direct);
        assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
        assert_eq!((get(direct, tasks::STATE), get(direct, tasks::RESULT)), (tasks::DONE, word::small(42).unwrap()));
        assert!(!cancelled(&mut ctx, direct));
        assert_eq!(flags(direct) & (CANCEL_REQUESTED | CANCEL_DELIVERED), CANCEL_REQUESTED);
        assert_eq!(destroyed(&runtime), "Res 1\n", "its cleanup ran once, on the success path");
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

/// Probe `window`, nested form (Tomato: `inner observed 42`, `inner cleanup`, `inner cancelled false`, `worker got
/// -2`, `-2`, `cancelled false`): the worker awaits the inner frame its await started; the inner frame's awaited
/// Future settles and queues it; the worker is cancelled. The inner frame completes with its value at its final
/// exit; the worker's await of it throws Cancelled, which the worker catches, and the worker completes.
#[test]
fn champ_k2_a_frame_completes_and_its_cancelled_task_throws_at_its_await() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_k2(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let first = gate(&mut ctx, &mut state);
        let worker = task(&mut ctx, WORKER, &[first], &mut state, true);
        let inner = get(worker, tasks::AWAITING);
        runtime.roots.push((inner, Repr::Ref));
        assert_eq!(runtime.function_of(inner), INNER as usize);
        open_gate(&mut ctx, first, 42);
        assert_eq!(rt_cancel_request(&mut ctx, worker), NORMAL);
        assert_eq!(runtime.ready.iter().copied().collect::<Vec<_>>(), [inner]);
        runtime.schedule(&mut ctx, worker);
        assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
        let value = word::small(42).unwrap();
        assert_eq!((get(inner, tasks::STATE), get(inner, tasks::RESULT)), (tasks::DONE, value));
        assert!(!cancelled(&mut ctx, inner));
        assert_eq!((get(worker, tasks::STATE), get(worker, tasks::RESULT)), (tasks::DONE, word::small(-2).unwrap()));
        assert!(!cancelled(&mut ctx, worker));
        assert_ne!(flags(worker) & CANCEL_DELIVERED, 0, "delivered at the worker's await");
        assert_eq!(destroyed(&runtime), "Res 1\n", "the inner frame's cleanup");
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

// --- `defer await` (ruling #601 K2: not shielded, Tomato parity) -------------------------------------------------

/// A task whose body the test plays (it is on the stack while it steps) runs `defer await delivery(gate)`.
struct Deferring {
    this: Word,
    io: CallOwn,
    args: [Word; 1],
    registration: RegistrationMap,
    post: DeferPost,
}

impl Deferring {
    fn new(ctx: &mut Ctx, state: &mut ScopeSlot, awaited: Word) -> Self {
        let this = gate(ctx, state);
        let input = empty(ctx);
        unsafe { (*ctx.runtime).roots.push((input.record, Repr::Ref)) };
        let io = CallOwn { input, entry_abi: u64::from(DELIVERY), output: std::ptr::null_mut(), context: 0,
            cleanup_mode: CLEANUP_SHIELDED };
        let registration = RegistrationMap { replacements: 0 };
        Self { this, io, args: [awaited], registration, post: DeferPost::default() }
    }

    fn step(&mut self, ctx: &mut Ctx) -> Outcome {
        let runtime = unsafe { &mut *ctx.runtime };
        runtime.running.push(self.this);
        let outcome = rt_invoke_defer_async(ctx, self.this, u64::from(DELIVERY), self.args.as_ptr(), 1, &mut self.io,
            &self.registration, &mut self.post);
        runtime.running.pop();
        outcome
    }

    fn child(&self, runtime: &Runtime) -> Word {
        get(self.this, tail(runtime, self.this) + STEP)
    }

    /// The step's Unwind, taken off the bridge as the caller's capture does; its failure.
    fn unwound(&mut self, ctx: &mut Ctx, outcome: Outcome) -> Word {
        assert_eq!(outcome, UNWIND);
        assert_eq!((ctx.status, ctx.failure), (FAILED, self.post.failure));
        (ctx.status, ctx.failure) = (OK, 0);
        assert_eq!((self.io.input.record, self.child(unsafe { &*ctx.runtime })), (0, 0), "the step is over");
        self.post.failure
    }
}

/// Probe `finaldefer` (Tomato: `body done`, `cancel requested`, `task -1 cancelled true`, without `defer
/// finished`): a cancellation that arrives while the defer waits is delivered at its await. Here the deferred call
/// waits as the sole awaiter of a spawned child, which the delivery cancels; the deferred call throws Cancelled
/// before its end, and the defer step unwinds with it.
#[test]
fn champ_k2_a_cancellation_interrupts_a_waiting_defer_await() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_k2(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let release = gate(&mut ctx, &mut state);
        mark(release, SPAWNED);
        let mut defer = Deferring::new(&mut ctx, &mut state, release);
        let this = defer.this;
        assert_eq!(defer.step(&mut ctx), OUTCOME_PENDING, "{:?}", runtime.fatal);
        let child = defer.child(&runtime);
        assert_eq!(flags(child) & (SHIELDED | OBSERVED), OBSERVED, "not shielded");
        assert_eq!(runtime.champ_task_of(child).unwrap(), this, "a frame of the task");
        assert_eq!(get(this, tasks::AWAITING), child);
        assert_eq!(rt_cancel_request(&mut ctx, this), NORMAL);
        // The request reaches the defer's own wait.
        assert_ne!(flags(child) & tasks::DELIVER_NOW, 0);
        assert_ne!(flags(release) & CANCEL_REQUESTED, 0);
        assert_ne!(flags(this) & CANCEL_DELIVERED, 0);
        close_gate_cancelled(&mut ctx, release);
        runtime.schedule(&mut ctx, child);
        assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
        assert!(failed_cancelled(&runtime, child), "the deferred call did not reach its end");
        assert_eq!(runtime.ready.pop_front(), Some(this));
        let outcome = defer.step(&mut ctx);
        let failure = defer.unwound(&mut ctx, outcome);
        assert_eq!(kind(&runtime, failure), "Cancelled");
        assert_eq!(destroyed(&runtime), "Res 1\n");
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

/// The deferred call settles while the task's request is still pending (the delivery found no wait to reach):
/// Tomato's await of it then raises the request, as an await whose task was cancelled while it waited does.
#[test]
fn champ_k2_a_defer_await_raises_a_request_pending_when_its_call_settles() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_k2(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let release = gate(&mut ctx, &mut state);
        let mut defer = Deferring::new(&mut ctx, &mut state, release);
        let this = defer.this;
        assert_eq!(defer.step(&mut ctx), OUTCOME_PENDING, "{:?}", runtime.fatal);
        let child = defer.child(&runtime);
        open_gate(&mut ctx, release, 5);
        assert_eq!(runtime.ready.iter().copied().collect::<Vec<_>>(), [child]);
        assert_eq!(rt_cancel_request(&mut ctx, this), NORMAL);
        assert_eq!(flags(this) & (CANCEL_REQUESTED | CANCEL_DELIVERED), CANCEL_REQUESTED);
        runtime.schedule(&mut ctx, child);
        assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
        assert_eq!((get(child, tasks::STATE), get(child, tasks::RESULT)), (tasks::DONE, word::small(5).unwrap()));
        assert_eq!(runtime.ready.pop_front(), Some(this));
        let outcome = defer.step(&mut ctx);
        let failure = defer.unwound(&mut ctx, outcome);
        assert_eq!(kind(&runtime, failure), "Cancelled");
        assert_ne!(flags(this) & CANCEL_DELIVERED, 0);
        assert_eq!(destroyed(&runtime), "Res 1\n");
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

/// Probe `windowdefer` (Tomato: `worker observed 42`, `worker -1 cancelled true`, without `defer started`): a
/// request pending when the defer runs is raised by its await before the deferred call starts.
#[test]
fn champ_k2_a_request_pending_before_a_defer_await_never_starts_the_call() {
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_k2(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let release = gate(&mut ctx, &mut state);
        let mut defer = Deferring::new(&mut ctx, &mut state, release);
        assert_eq!(rt_cancel_request(&mut ctx, defer.this), NORMAL);
        let outcome = defer.step(&mut ctx);
        let failure = defer.unwound(&mut ctx, outcome);
        assert_eq!(kind(&runtime, failure), "Cancelled");
        assert_eq!(count(&runtime, DELIVERY, FutureStep::Start), 0);
        assert_ne!(flags(defer.this) & CANCEL_DELIVERED, 0);
        assert_eq!((get(release, tasks::WAITER), destroyed(&runtime)), (word::UNBOUND, String::new()));
        // No body ran in any configuration: there is no trace to compare.
        assert!(runtime.champ.future_events.iter().all(|event| event.step != FutureStep::Start), "{configuration:?}");
    }
}
