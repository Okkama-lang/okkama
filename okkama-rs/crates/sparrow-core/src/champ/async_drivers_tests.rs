//! #601 Q1: the D22 driver bridge in the Champ runtime (Champ draft 3 §§2.1, 2.4, 9; plan §3.6 "Drivers"). A
//! driver Future is a Tomato driver frame with the Champ tail; Tomato's drivers run it and its scheduler wakes it,
//! and the synchronous members of the std async primitives run as Tomato's intrinsics. The program is the trusted
//! catalogue's own entries over the declarations it admits; expectations come from Tomato's drivers
//! (`drivers.rs`, `primitives.rs`) and draft 3 §9.
use super::*;
use crate::champ::futures::{EMERGENCY, INPUTS, TASK};
use crate::intrinsics::Intrinsic;

const EVENT: u32 = 0;
const BARRIER: u32 = 1;
const BROKEN: u32 = 2;

fn object(name: &str, storage: Vec<s::Type>, fields: &[&str]) -> s::Object {
    s::Object {
        name: name.into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle: false, role: None,
        implements: vec![], any_box: false, enum_names: vec![],
        fields: fields.iter().enumerate().map(|(index, name)| s::Field {
            name: (*name).into(), storage: index as u32, once: false,
        }).collect(),
        layout: s::ObjectRuntimeLayout { allocation_words: 1 + storage.len().max(1) as u32, identity_offset: None,
            has_destroy: false, may_reach_resource: false, may_async_destroy: false,
            trace_layout: storage.iter().enumerate().filter_map(|(i, ty)| ty.traced().then_some(i as u32)).collect(),
            selection_layout: vec![], storage_fields: storage },
    }
}

/// The Event cell, Barrier and BrokenBarrier, with every catalogue entry they admit as the program's functions.
fn program() -> s::Program {
    let mut broken = object("async.barrier.BrokenBarrier", vec![s::Type::Int], &[]);
    broken.error_cases = vec!["BrokenBarrier.broken".into()];
    let objects = vec![object(s::drivers::EVENT, s::drivers::EVENT_STORAGE.to_vec(), &[]),
        object("async.barrier.Barrier", vec![s::Type::Int], &["amount"]), broken];
    let contracts = s::p1::P1Contracts::new(&objects).unwrap();
    let functions = contracts.entries().iter().filter(|entry| {
        entry.symbol.starts_with("okkama.event.") || entry.symbol.starts_with("okkama.barrier.Barrier.")
    }).cloned().collect::<Vec<_>>();
    let mut future_results = Vec::new();
    for function in functions.iter().filter(|f| f.entry_abi == s::EntryAbi::AsyncExternal) {
        let outcome = function.results.first().copied();
        if !future_results.contains(&outcome) {
            future_results.push(outcome);
        }
    }
    s::Program {
        module: "drivers".into(), objects, functions,
        tables: s::ChampTables { future_results, ..s::ChampTables::default() },
        ..s::Program::default()
    }
}

fn index(runtime: &Runtime, name: &str) -> usize {
    runtime.champ.machine.as_ref().unwrap().source.functions.iter().position(|f| f.symbol == name).unwrap()
}

fn load(stress: bool) -> Box<Runtime> {
    let mut runtime = Runtime::new_champ(program(), Vec::new()).unwrap();
    runtime.heap.set_stress(stress);
    runtime
}

/// A collection with `kept` rooted, as a caller's stack map would keep them.
fn collect(ctx: &mut Ctx, kept: &[Word]) {
    let runtime = unsafe { &mut *ctx.runtime };
    let base = runtime.roots.len();
    runtime.roots.extend(kept.iter().map(|&word| (word, Repr::Ref)));
    runtime.safepoint(frame_pointer!(), ctx, &[]);
    runtime.roots.truncate(base);
}

/// A synchronous member called as the machine calls an observation entry: no packet.
fn observe(ctx: &mut Ctx, function: usize, args: &[Word]) -> Word {
    super::super::p1::invoke(ctx, function, std::ptr::null_mut(), args)
}

#[test]
fn champ_q1_the_declaration_view_links_drivers_and_intrinsics() {
    let runtime = load(false);
    let wait = index(&runtime, "okkama.event.Event.wait");
    let set = index(&runtime, "okkama.event.Event.set");
    let legacy = &runtime.loaded.program.functions;
    // A driver is async with its intrinsic; a synchronous member has its intrinsic; both resolve in Tomato.
    assert!(legacy[wait].is_async && crate::program::is_driver(&legacy[wait]));
    assert!(!legacy[set].is_async && legacy[set].intrinsic.as_ref().unwrap().name == "okkama.event.Event.set");
    assert_eq!(runtime.intrinsics[wait], Some(Intrinsic::EventWait));
    assert_eq!(runtime.intrinsics[set], Some(Intrinsic::EventSet));
    assert!(runtime.loaded.is_event(EVENT as usize), "Event.new names the cell");
    let new = index(&runtime, "okkama.event.Event.new");
    assert_eq!(legacy[new].intrinsic.as_ref().unwrap().objects, [okkama_sparrow::ObjectRef(EVENT)]);
    let barrier = index(&runtime, "okkama.barrier.Barrier.wait");
    assert_eq!(legacy[barrier].intrinsic.as_ref().unwrap().objects, [okkama_sparrow::ObjectRef(BROKEN)]);
    // The driver's frame: Tomato's layout (fixed words, one register, the driver slot, the retained argument),
    // then the Champ tail (draft 3 §2.3). No async body takes a driver's map.
    let map = runtime.champ_frame_map(wait).unwrap();
    assert_eq!(map.driver, Some(1));
    assert_eq!((map.tail, map.words, map.inputs(), map.packet()), (8 * 12, 1 + 15, 1, Some(8 * 12 + INPUTS)));
    assert_eq!(runtime.heap.layouts().frames[wait], map.trace);
    assert_eq!(map.trace[11..], [Repr::Ref; 4]);
    assert!(!runtime.champ_async_body(wait));
    assert!(runtime.champ_frame_map(set).is_none());
}

#[test]
fn champ_q1_a_driver_future_waits_for_its_event() {
    for stress in [false, true] {
        let mut runtime = load(stress);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let ctx = &mut ctx;
        let runtime = runtime_of(ctx);
        let [new, set, ready, wait] = ["new", "set", "ready", "wait"]
            .map(|member| index(runtime, &format!("okkama.event.Event.{member}")));
        let event = crate::intrinsics::champ_call(ctx, new, &[]);
        runtime.roots.push((event, Repr::Ref));
        assert_eq!(observe(ctx, ready, &[event]), 0);
        let mut state = owner(ctx);
        let inputs = empty(ctx);
        let future = new_future(ctx, wait as u32, &[event], &mut state, inputs);
        // Creation (draft 3 §2.4): a cold Champ driver frame, its argument in its register and its retained word,
        // its inputs packet in INPUTS, its emergency failure ready.
        let tail = runtime.champ_frame_map(wait).unwrap().tail;
        assert_eq!((get(future, tasks::STATE), get(future, tasks::FLAGS) & CHAMP), (tasks::COLD, CHAMP));
        assert_eq!((get(future, tasks::REGISTERS), get(future, tasks::REGISTERS + 16)), (event, event));
        assert!(get(future, tail + INPUTS) != 0 && get(future, tail + EMERGENCY) != 0);
        collect(ctx, &[future, event]);
        // Started, it parks on the event with its own task (draft 3 §4.3).
        runtime.champ_future_start(ctx, 0, future);
        assert_eq!(ctx.status, OK);
        assert_eq!(get(future, tasks::STATE), tasks::RUNNING);
        assert_eq!(get(future, tail + TASK), future);
        assert!(runtime.ready.is_empty());
        collect(ctx, &[future, event]);
        // Event.set wakes it through the scheduler; it does not run inside the set (#340).
        assert_eq!(observe(ctx, set, &[event]), 0);
        assert_eq!((ctx.status, observe(ctx, ready, &[event])), (OK, 1));
        assert_eq!(get(future, tasks::STATE), tasks::RUNNING);
        assert_eq!(runtime.ready.iter().copied().collect::<Vec<_>>(), [future]);
        let woken = runtime.ready.pop_front().unwrap();
        assert!(runtime.run_body(ctx, woken));
        // Done: the receiver, with an empty outcome packet in the cache before the waiters woke (draft 3 §9).
        assert_eq!((get(future, tasks::STATE), get(future, tasks::RESULT)), (tasks::DONE, event));
        let outcome = runtime.champ_cache(future).unwrap().packet;
        assert!(outcome != 0 && runtime.champ_packet_keys(&PacketSlot { record: outcome }).unwrap().is_empty());
        collect(ctx, &[future, event]);
        // A Future of a set event settles at its start.
        let inputs = empty(ctx);
        let again = new_future(ctx, wait as u32, &[event], &mut state, inputs);
        runtime.champ_future_start(ctx, 0, again);
        assert_eq!(get(again, tasks::STATE), tasks::DONE);
    }
}

#[test]
fn champ_q1_a_cancelled_driver_leaves_its_event() {
    for stress in [false, true] {
        let mut runtime = load(stress);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let ctx = &mut ctx;
        let runtime = runtime_of(ctx);
        let (new, set, wait) = (index(runtime, "okkama.event.Event.new"), index(runtime, "okkama.event.Event.set"),
            index(runtime, "okkama.event.Event.wait"));
        let event = crate::intrinsics::champ_call(ctx, new, &[]);
        runtime.roots.push((event, Repr::Ref));
        let mut state = owner(ctx);
        let inputs = empty(ctx);
        let future = new_future(ctx, wait as u32, &[event], &mut state, inputs);
        runtime.champ_future_start(ctx, 0, future);
        // A request reaches the parked driver as Tomato's: it is queued with DELIVER_NOW and withdraws its wait.
        assert_eq!(rt_cancel_request(ctx, future), NORMAL);
        assert_eq!(runtime.ready.iter().copied().collect::<Vec<_>>(), [future]);
        collect(ctx, &[future, event]);
        let woken = runtime.ready.pop_front().unwrap();
        assert!(runtime.run_body(ctx, woken));
        assert_eq!(get(future, tasks::STATE), tasks::FAILURE_STATE);
        assert_eq!(kind(runtime, get(future, tasks::FAILURE)), "Cancelled");
        assert!(runtime.champ_cache(future).unwrap().packet == 0, "a failure publishes no outcome packet");
        // The event no longer holds it: a later set wakes nothing.
        observe(ctx, set, &[event]);
        assert!(runtime.ready.is_empty());
        // A cold driver Future cancels without running (draft 3 §5).
        let inputs = empty(ctx);
        let cold = new_future(ctx, wait as u32, &[event], &mut state, inputs);
        assert_eq!(rt_cancel_request(ctx, cold), NORMAL);
        assert_eq!(get(cold, tasks::STATE), tasks::FAILURE_STATE);
    }
}

#[test]
fn champ_q1_a_synchronous_member_refuses_an_object_it_did_not_create() {
    let mut runtime = load(false);
    let mut ctx = context(&mut runtime);
    let ctx = &mut ctx;
    let runtime = runtime_of(ctx);
    let set = index(runtime, "okkama.event.Event.set");
    // A Barrier object is no Event cell: Tomato's program failure, left in the context.
    let one = runtime.heap.int(1);
    let barrier = runtime.heap.object_new(BARRIER, &[one]);
    assert_eq!(observe(ctx, set, &[barrier]), 0);
    assert_eq!(ctx.status, FAILED);
    assert_eq!(kind(runtime, ctx.failure), "Program(\"Invalid async event\")");
}

#[test]
fn champ_q1_a_thrown_error_case_carries_its_case_number() {
    let mut runtime = load(false);
    let wait = index(&runtime, "okkama.barrier.Barrier.wait");
    // Champ's Error enum is the object of its first case with its case number (Tomato's has `$tag` arrays).
    let FailureKind::Thrown(object) = crate::intrinsics::thrown(&mut runtime, wait, 0, &[]) else {
        panic!("a thrown error");
    };
    assert_eq!(heap::payload(runtime.heap.header_of(object)), BROKEN);
    assert_eq!(runtime.heap.field(object, 0), word::small(0).unwrap());
}
