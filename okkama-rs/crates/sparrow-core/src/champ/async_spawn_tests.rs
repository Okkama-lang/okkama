//! #601 P1: `fut.spawn` in the Champ runtime and the JIT (Champ draft 3 §§6.2, 7; OIR-2 async ABI D24; #699).
//!
//! Each scenario runs, as in the parent module, compiled by the JIT and by the reference body, with the collector
//! idle and with a collection at every safepoint. A spawned task starts at once on the spawner's stack and runs
//! until its first suspension; its owner's close settles its task duty before its cache duty: a running task
//! detaches with its cache to the root registry, a settled one closes; shutdown cancels the running registered
//! tasks, waits for them, closes the outcome packets of the detached ones (#699) and reports the first unobserved
//! failure that is not a cancellation. Expectations come from draft 3 and Tomato's observable behaviour (Bible
//! §§20.4, 20.15; conformance `wake.spawn-and-cold-start`, `failure.spawned-task-observed`,
//! `shutdown.unobserved-failure`).
use super::*;
use tasks::champ_flags::{CANCEL_REQUESTED, OBSERVED, SPAWNED};

/// Function indices; each async function's frame layout, lexical scope and boundary have its index.
const SPAWNS_WORK: u32 = 2;
const SPAWNS_FAILS: u32 = 3;
const AWAITS_SPAWNED: u32 = 4;
const RETURNS_SPAWNED: u32 = 5;
const ASYNC_P1: u32 = 6;

/// Slots of a spawner: 0 inputs, 1 owner state, 2 cache duty, 3 task duty, 4 the Future, 5–7 the unwind.
fn spawner_slots() -> Vec<s::FrameSlot> {
    vec![service(K::Packet), service(K::ScopeState), service(K::CacheState), service(K::TaskState),
        value(FUTURE_INT), value(s::Type::Failure), service(K::TaskState), service(K::Receipt)]
}

/// `async spawner() is int { spawn <entry>(…); return 0; }`, or with `awaits`, `let t = spawn work(7); return await
/// t;`. Every close settles the task duty, then the cache duty (ABI §6.3).
fn spawner(symbol: &str, index: u32, entry: u32, awaits: bool) -> (s::Function, s::FrameLayout) {
    let slots = spawner_slots();
    let (mut b, frame) = Body::asynchronous(symbol, index, vec![s::Type::Int]);
    let (body, inputs) = b.resume(frame, 0, &types(&slots, &[0]));
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(index), A::Boundary(index)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[0])], K::ScopeState);
    let args = if entry == WORK { vec![b.constant(s::Constant::Int("7".into()), s::Type::Int)] } else { vec![] };
    let (future, cache, q2) = future_new(&mut b, index, entry, &args, q1);
    let [kept, task] = [b.service(K::CacheState), b.service(K::TaskState)];
    b.next(O::FutureSpawn, vec![l(future), l(cache)], vec![kept, task]);
    if !awaits {
        let zero = b.constant(s::Constant::Int("0".into()), s::Type::Int);
        b.return_value(body, q2, index, Some(zero), &[task, kept], "done");
        let layout = b.layout(slots, vec![(vec![0], vec![])]);
        return (b.finish(), layout);
    }
    let first = b.save(body, 1, &[q2, kept, task]);
    b.outcomes(O::Await, vec![l(future)], vec![(Tag::Normal, "ok", vec![p(0), p(1)]),
        (Tag::Unwind, "bad", vec![p(0), p(1), p(2)])]);
    let [x, deps] = [b.register(s::Type::Int), b.service(K::Deps)];
    b.block("ok", &[x, deps]);
    let (body_ok, resumed) = b.resume(first, 1, &types(&slots, &[1, 2, 3]));
    b.return_value(body_ok, resumed[0], index, Some(x), &[resumed[2], resumed[1]], "done");
    let unwind = [b.register(s::Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block("bad", &unwind);
    let (body_bad, resumed) = b.resume(first, 1, &types(&slots, &[1, 2, 3]));
    let (body_adapted, after, adapted, kept) =
        b.adapter(body_bad, 2, resumed[0], unwind, &[resumed[1], resumed[2]], &types(&slots, &[2, 3]));
    b.fail_close(body_adapted, after, adapted, index, &[kept[1], kept[0]], "failed");
    let layout = b.layout(slots, vec![(vec![0], vec![]), (vec![1, 2, 3], vec![4]), (vec![2, 3], vec![1, 5, 6, 7])]);
    (b.finish(), layout)
}

/// `async returns_spawned() is Future<int> { return spawn work(7); }`: the spawned Future leaves through the exit
/// with its task duty and its cache duty (token `cache.rehome` with the task, ABI C-4), so the close settles none.
fn returns_spawned() -> (s::Function, s::FrameLayout) {
    let slots = spawner_slots();
    let index = RETURNS_SPAWNED;
    let (mut b, frame) = Body::asynchronous("returns_spawned", index, vec![FUTURE_INT]);
    let (body, inputs) = b.resume(frame, 0, &types(&slots, &[0]));
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(index), A::Boundary(index)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[0])], K::ScopeState);
    let seven = b.constant(s::Constant::Int("7".into()), s::Type::Int);
    let (future, cache, q2) = future_new(&mut b, index, WORK, &[seven], q1);
    let [kept, task] = [b.service(K::CacheState), b.service(K::TaskState)];
    b.next(O::FutureSpawn, vec![l(future), l(cache)], vec![kept, task]);
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let prepared = b.emit(O::ExitPrepare, vec![A::Kind(s::ExitKind::Return), A::Boundary(index), l(future), l(empty),
        A::Selection(selection)], K::Pending);
    let pending = b.emit(O::CacheRehome, vec![A::Destination(s::ExitDestination::Caller), l(prepared), l(kept),
        l(task)], K::Pending);
    let finished = b.close(q2, Some(pending), None, index, &[], false);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, "done", vec![p(0), p(1)])]);
    b.block("done", &[closed, reserved]);
    let outputs = vec![b.register(FUTURE_INT), b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(s::ExitDestination::Caller)], outputs.clone());
    let mut published = vec![l(body), A::Flag(true)];
    published.extend(outputs.into_iter().map(l));
    b.next(O::FuturePublish, published, vec![]);
    b.ret();
    let layout = b.layout(slots, vec![(vec![0], vec![])]);
    (b.finish(), layout)
}

fn p1_program() -> s::Program {
    let mut functions = Vec::new();
    let mut frames = Vec::new();
    for (function, layout) in [work(), fails(), spawner("spawns_work", SPAWNS_WORK, WORK, false),
        spawner("spawns_fails", SPAWNS_FAILS, FAILS, false), spawner("awaits_spawned", AWAITS_SPAWNED, WORK, true),
        returns_spawned()]
    {
        functions.push(function);
        frames.push(layout);
    }
    functions.push(destroy());
    s::Program {
        module: "spawn".into(),
        objects: vec![res()],
        functions,
        tables: s::ChampTables {
            scopes: (0..ASYNC_P1).map(|index| s::Scope { function: index, parent: None, boundary: index }).collect(),
            boundaries: (0..ASYNC_P1).map(|scope| s::Boundary { scope }).collect(),
            frames,
            // Future 2 is `Future<Future<int>>`, the Future of `returns_spawned`.
            future_results: vec![Some(s::Type::Int), None, Some(FUTURE_INT)],
            ..s::ChampTables::default()
        },
        ..s::Program::default()
    }
}

fn load_p1(stress: bool, engine: Engine) -> Box<Runtime> {
    let built = p1_program();
    s::verify_load(&built, &s::NoExternalContracts).unwrap();
    let assembled = s::assemble(&s::disassemble(&built).unwrap()).unwrap();
    assert_eq!(assembled, built);
    let mut runtime = Runtime::new_champ(assembled, Vec::new()).unwrap();
    runtime.heap.set_stress(stress);
    if engine == Engine::Reference {
        for function in 0..ASYNC_P1 as usize {
            runtime.invocations[function].code = reference as *const () as u64;
        }
    }
    runtime
}

/// The destroys so far, `Object serial` per line.
fn destroyed(runtime: &Runtime) -> String {
    String::from_utf8(runtime.champ.captured_trace.clone().unwrap()).unwrap()
}

fn events(runtime: &Runtime) -> Vec<(FutureStep, usize)> {
    runtime.champ.future_events.iter().map(|event| (event.step, event.function)).collect()
}

/// Close `state` settling `duties` of the given kinds in schedule order with a merge plan; the close's failure.
fn close_duties(ctx: &mut Ctx, state: &mut ScopeSlot, duties: &[(Word, u64)]) -> Word {
    let runtime = unsafe { &mut *ctx.runtime };
    let schedule = runtime.champ_register_schedule(duties.iter().map(|&(_, kind)| kind).collect());
    let plan = runtime.champ_register_cleanup_plan(CleanupPlan { reserve: false, direct: false, may_fail: false });
    let inputs = CloseInputs {
        scope: state, packet: std::ptr::null_mut(), pending: std::ptr::null_mut(), failure: 0, settlements: 0,
    };
    let mut cleanup = CleanupSlot::default();
    assert_eq!(rt_cleanup_begin(ctx, &inputs, &mut cleanup), NORMAL);
    let base = runtime.roots.len();
    runtime.roots.push((cleanup.record, Repr::Ref));
    for (index, &(duty, _)) in duties.iter().enumerate() {
        let outcome = rt_service_settle(ctx, 0, &mut cleanup, schedule, index as u64, duty, &mut cleanup);
        assert_eq!(outcome, NORMAL, "{:?}", runtime.fatal);
    }
    assert_eq!(rt_cleanup_plan(ctx, &mut cleanup, plan, &mut cleanup), NORMAL);
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
    }
    let mut finished = FinishPost::default();
    assert_eq!(rt_cleanup_finish(ctx, &mut cleanup, &mut finished), NORMAL);
    runtime.roots.truncate(base);
    finished.failure
}

fn spawn(ctx: &mut Ctx, future: Word) {
    let mut cache = CacheSlot { record: future };
    let mut post = SpawnPost::default();
    let runtime = unsafe { &mut *ctx.runtime };
    assert_eq!(rt_future_spawn(ctx, future, &mut cache, &mut post), NORMAL, "{:?}", runtime.fatal);
    assert_eq!((cache.record, post.cache, post.task), (0, future, future));
}

#[test]
fn champ_p1_spawn_starts_the_task_at_once_and_its_owner_closes_it() {
    // `spawn work(7); return 0;`: work starts and publishes inside the spawn, before the spawner returns; the close
    // settles the settled task's duty (closed) and releases its cache's outcome.
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_p1(stress, engine);
        let result = runtime.run_function(SPAWNS_WORK as usize, crate::STACK_BYTES);
        assert_eq!(result.unwrap(), word::small(0).unwrap());
        let order = events(&runtime);
        let spawned = [(FutureStep::Start, WORK as usize), (FutureStep::Published, WORK as usize)];
        let start = order.iter().position(|event| *event == (FutureStep::Start, SPAWNS_WORK as usize)).unwrap();
        assert_eq!(order[start + 1..start + 3], spawned, "{order:?}");
        assert_eq!(count(&runtime, WORK, FutureStep::Start), 1);
        assert!(runtime.spawned.is_empty(), "shutdown empties the registry");
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

#[test]
fn champ_p1_the_spawned_task_is_awaited_through_its_future() {
    // `let t = spawn work(7); return await t;`: both duties cross the await in the frame and the await sees the
    // outcome the task already published.
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_p1(stress, engine);
        let result = runtime.run_function(AWAITS_SPAWNED as usize, crate::STACK_BYTES);
        assert_eq!(result.unwrap(), word::small(7).unwrap());
        assert_eq!(steps(&runtime, WORK), [FutureStep::Start, FutureStep::Published]);
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

#[test]
fn champ_p1_shutdown_reports_an_unobserved_spawned_failure() {
    // `spawn fails(); return 0;`: nobody awaits the task; the run fails with its failure (D-034,
    // `shutdown.unobserved-failure`).
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_p1(stress, engine);
        let report = runtime.run_function(SPAWNS_FAILS as usize, crate::STACK_BYTES).unwrap_err().0;
        assert!(report.starts_with("RuntimeError: boom"), "{report}");
        assert_eq!(count(&runtime, SPAWNS_FAILS, FutureStep::Published), 1, "the spawner itself succeeded");
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

thread_local! {
    /// The gate a `waits_then_holds` body waits for.
    static GATE: std::cell::Cell<Word> = const { std::cell::Cell::new(0) };
}

/// A body that waits for `GATE` at its first run and then publishes a success whose outcome holds a new Res,
/// committed into its cache as `exit.commit` does.
extern "C" fn waits_then_holds(ctx: *mut Ctx, frame: Word) -> Word {
    let runtime = runtime_of(ctx);
    if get(frame, tasks::RESUME) == 0 {
        let gate = GATE.with(|gate| gate.get());
        set(frame, tasks::RESUME, 1);
        set(frame, tasks::AWAITING, gate);
        runtime.champ_add_waiter(gate, frame);
        return 1;
    }
    returns_resource(ctx, frame)
}

/// The cache and registry facts of a Future: (detached, root-owned, released, registered).
fn facts(runtime: &Runtime, future: Word) -> (bool, bool, bool, usize) {
    let cache = runtime.champ_cache(future).unwrap();
    (get(future, tasks::FLAGS) & DETACHED != 0, Some(cache.owner) == runtime.champ.root_owner,
        cache.flags & RELEASED != 0, runtime.spawned.iter().filter(|&&task| task == future).count())
}

#[test]
fn champ_p1_a_running_task_detaches_at_its_close_and_shutdown_closes_its_outcome() {
    // D24.2: the owner closes while the spawned task waits: the task duty detaches the task with its cache to the
    // root registry (not cancelled, registered once), and the cache duty after it settles to nothing. KL-1: once the
    // task has published an outcome that holds a resource, the registry keeps it through every collection. #699:
    // shutdown cancels and drains the registry, then the root closer destroys the outcome exactly once.
    for stress in [false, true] {
        let mut runtime = load_p1(stress, Engine::Jit);
        runtime.champ.captured_trace = Some(Vec::new());
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let mut elsewhere = owner(&mut ctx);
        let gate = gate(&mut ctx, &mut elsewhere);
        GATE.with(|cell| cell.set(gate));
        let inputs = empty(&mut ctx);
        let task = new_future(&mut ctx, FAILS, &[], &mut state, inputs);
        runtime.invocations[FAILS as usize].code = waits_then_holds as *const () as u64;
        spawn(&mut ctx, task);
        assert_eq!(get(task, tasks::STATE), tasks::RUNNING);
        assert_ne!(get(task, tasks::FLAGS) & SPAWNED, 0);
        assert_eq!(runtime.champ_task_of(task).unwrap(), task);
        assert_eq!(facts(&runtime, task), (false, false, false, 1));
        // The task duty decides the detach; the cache duty after it then belongs to the root.
        let closing = super::super::helpers::scope(&runtime, state.record).unwrap().owner;
        assert_eq!(runtime.champ_task_settlement(task, Some(closing)).unwrap(), CacheSettle::Detach);
        assert_eq!(close_duties(&mut ctx, &mut state, &[(task, SERVICE_TASK), (task, SERVICE_CACHE)]), 0);
        assert_eq!(facts(&runtime, task), (true, true, false, 1));
        assert_eq!(runtime.champ_cache_settlement(task, Some(closing)).unwrap(), CacheSettle::Nothing);
        assert_eq!(get(task, tasks::FLAGS) & CANCEL_REQUESTED, 0, "a detached task is not cancelled");
        // The gate opens; the task publishes an outcome that holds a Res, which no scope owns.
        open_gate(&mut ctx, gate, 5);
        runtime.schedule(&mut ctx, task);
        assert_eq!((ctx.status, get(task, tasks::STATE)), (OK, tasks::DONE));
        assert!(runtime.champ_retained(task));
        runtime.prune_spawned();
        assert_eq!(facts(&runtime, task), (true, true, false, 1), "KL-1: kept until shutdown");
        assert_eq!(destroyed(&runtime), "");
        assert_eq!(runtime.champ_shutdown(&mut ctx).unwrap(), None);
        assert_eq!(ctx.status, OK);
        assert_eq!(destroyed(&runtime), "Res 1\n", "the root closer destroys the outcome once");
        assert!(runtime.spawned.is_empty());
        assert_eq!(runtime.champ_cache(task).unwrap().flags & RELEASED, RELEASED);
    }
}

/// A body that parks at its first run, as a driver waiting for the outside does (Tomato's PARKED), and publishes
/// Cancelled when a delivered cancellation wakes it, or 1 otherwise.
extern "C" fn parks(ctx: *mut Ctx, frame: Word) -> Word {
    let runtime = runtime_of(ctx);
    if get(frame, tasks::RESUME) == 0 {
        set(frame, tasks::RESUME, 1);
        set(frame, tasks::FLAGS, get(frame, tasks::FLAGS) | tasks::PARKED);
        return 1;
    }
    let delivered = get(frame, tasks::FLAGS) & tasks::DELIVER_NOW != 0;
    set(frame, tasks::FLAGS, get(frame, tasks::FLAGS) & !tasks::DELIVER_NOW);
    let mut outcome = PacketSlot::default();
    assert_eq!(rt_own_empty(ctx, &mut outcome), NORMAL);
    let failure = if delivered { runtime.cancelled_failure() } else { 0 };
    let published = PublishIn { tag: u64::from(delivered), value: word::small(1).unwrap(), packet: &mut outcome,
        failure };
    assert_eq!(rt_future_publish(ctx, frame, &published), NORMAL);
    0
}

#[test]
fn champ_p1_shutdown_cancels_and_drains_a_detached_task() {
    // D24.2 shutdown step 1: a detached task still waiting when the run ends is cancelled and scheduled until it
    // settles; its cancellation is no report and its empty outcome leaves nothing for the root closer.
    for stress in [false, true] {
        let mut runtime = load_p1(stress, Engine::Jit);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let inputs = empty(&mut ctx);
        let task = new_future(&mut ctx, FAILS, &[], &mut state, inputs);
        runtime.invocations[FAILS as usize].code = parks as *const () as u64;
        spawn(&mut ctx, task);
        assert_eq!(get(task, tasks::STATE), tasks::RUNNING);
        assert_eq!(close_duties(&mut ctx, &mut state, &[(task, SERVICE_TASK), (task, SERVICE_CACHE)]), 0);
        assert_eq!(facts(&runtime, task), (true, true, false, 1));
        assert_eq!(get(task, tasks::FLAGS) & CANCEL_REQUESTED, 0, "a detached task is not cancelled");
        assert_eq!(runtime.champ_shutdown(&mut ctx).unwrap(), None, "a cancellation is never reported");
        assert_eq!(ctx.status, OK);
        assert_ne!(get(task, tasks::FLAGS) & CANCEL_REQUESTED, 0, "shutdown cancelled it");
        assert!(failed_cancelled(&runtime, task), "and waited until it settled");
        assert!(runtime.spawned.is_empty());
    }
}

fn failed_cancelled(runtime: &Runtime, future: Word) -> bool {
    get(future, tasks::STATE) == tasks::FAILURE_STATE && kind(runtime, get(future, tasks::FAILURE)) == "Cancelled"
}

#[test]
fn champ_p1_task_duty_rows() {
    for engine in [Engine::Jit, Engine::Reference] {
        let mut runtime = load_p1(true, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        // A task that settled before its owner closed: its duty closes, its cache releases the outcome, and a
        // success leaves no report entry once pruned.
        let inputs = empty(&mut ctx);
        let done = new_future(&mut ctx, WORK, &[word::small(3).unwrap()], &mut state, inputs);
        spawn(&mut ctx, done);
        assert_eq!(get(done, tasks::STATE), tasks::DONE);
        assert_eq!(get(done, tasks::FLAGS) & OBSERVED, 0, "spawn does not observe");
        assert_eq!(close_duties(&mut ctx, &mut state, &[(done, SERVICE_TASK), (done, SERVICE_CACHE)]), 0);
        assert_eq!(facts(&runtime, done), (false, false, true, 1));
        runtime.prune_spawned();
        assert_eq!(facts(&runtime, done).3, 0, "a settled success leaves the registry");
        // A Future that is no longer cold is not started again; a second spawn changes nothing.
        let before = (get(done, tasks::STATE), get(done, tasks::FLAGS));
        let mut cache = CacheSlot { record: done };
        let mut post = SpawnPost::default();
        assert_eq!(rt_future_spawn(&mut ctx, done, &mut cache, &mut post), OUTCOME_STOP, "a released cache");
        assert!(runtime.fatal.as_deref().unwrap().contains("settled or rehomed"));
        assert_eq!((get(done, tasks::STATE), get(done, tasks::FLAGS)), before);
        ctx.status = OK;
        runtime.fatal = None;
        // The task duty of a Future that was never spawned is refused before anything changes.
        let mut other = owner(&mut ctx);
        let inputs = empty(&mut ctx);
        let cold = new_future(&mut ctx, WORK, &[word::small(4).unwrap()], &mut other, inputs);
        let schedule = runtime.champ_register_schedule(vec![SERVICE_TASK, SERVICE_CACHE]);
        let inputs = CloseInputs {
            scope: &mut other, packet: std::ptr::null_mut(), pending: std::ptr::null_mut(), failure: 0,
            settlements: 0,
        };
        let mut cleanup = CleanupSlot::default();
        assert_eq!(rt_cleanup_begin(&mut ctx, &inputs, &mut cleanup), NORMAL);
        runtime.roots.push((cleanup.record, Repr::Ref));
        assert_eq!(rt_service_settle(&mut ctx, 0, &mut cleanup, schedule, 0, cold, &mut cleanup), OUTCOME_STOP);
        assert!(runtime.fatal.as_deref().unwrap().contains("not spawned"));
        assert_eq!(get(cold, tasks::STATE), tasks::COLD);
        // A cache duty that names another Future is refused by the spawn.
        ctx.status = OK;
        runtime.fatal = None;
        let mut cache = CacheSlot { record: done };
        assert_eq!(rt_future_spawn(&mut ctx, cold, &mut cache, &mut post), OUTCOME_STOP);
        assert!(runtime.fatal.as_deref().unwrap().contains("another Future"));
        assert_eq!(get(cold, tasks::STATE), tasks::COLD);
    }
}

#[test]
fn champ_p1_a_returned_spawned_future_carries_both_duties_to_the_caller() {
    // C-4, D24.1: the rehome takes the cache duty and the task duty, task first, into the reserve; the commit hands
    // both to the returning Future's cache, whose outcome packet now holds them for the caller.
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load_p1(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let inputs = empty(&mut ctx);
        let outer = new_future(&mut ctx, RETURNS_SPAWNED, &[], &mut state, inputs);
        runtime.champ_future_start(&mut ctx, 0, outer);
        assert_eq!((ctx.status, get(outer, tasks::STATE)), (OK, tasks::DONE), "{:?}", runtime.fatal);
        let inner = get(outer, tasks::RESULT);
        assert_eq!(get(inner, tasks::STATE), tasks::DONE, "the spawned task ran at once");
        assert_ne!(get(inner, tasks::FLAGS) & SPAWNED, 0);
        let outcome = super::super::helpers::packet(&runtime, runtime.champ_cache(outer).unwrap().packet).unwrap();
        let entries: Vec<_> = outcome.services.iter().map(|entry| (entry.holder, entry.kind, entry.target)).collect();
        assert_eq!(entries, [(inner, SERVICE_TASK, None), (inner, SERVICE_CACHE, None)]);
        let cache = runtime.champ_cache(inner).unwrap();
        assert_eq!((cache.owner, cache.flags & (RELEASED | REHOMED)), (runtime.champ_cache(outer).unwrap().owner, 0));
        traces.record(configuration, &runtime);
    }
    traces.agree();
}
