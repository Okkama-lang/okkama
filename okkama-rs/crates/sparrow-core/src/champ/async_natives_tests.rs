//! #601 N1: async natives in the Champ runtime (OIR-2 async ABI D22, ruling PR-N; Champ draft 3 §§2.1, 2.4, 9). An
//! async native of the runtime table is a driver Future: a Tomato driver frame with the Champ tail, run by Tomato's
//! `drivers::native` (the timer of `sleep_nanoseconds`, `native_io` for a host request). The program is the trusted
//! P1 entries of every async native over the handle declarations of their modules; expectations come from Tomato's
//! drivers (`drivers.rs`, `io.rs`, `host::cancel`) and draft 3 §9.
use super::*;
use crate::champ::futures::{INPUTS, TASK};
use okkama_runtime_abi::{Native, Type as AbiType};
use std::collections::BTreeSet;

const SLEEP: &str = "runtime/time@1::sleep_nanoseconds/1";
const PIECE: &str = "runtime/fs_root@1::rooted_entry_piece/3";
const ROOT_OPEN: &str = "runtime/fs_root@1::file_root_open/1";
const ROOT_ENTRY: &str = "runtime/fs_root@1::file_root_entry/2";
const WATCH: &str = "runtime/fs@1::file_watch_event/1";
const WATCH_CREATE: &str = "runtime/fs@1::file_watch_create/1";
/// The map composites of the two async natives with a map result (#752): `file_watch_event`, `process_state_exit`.
const MAPS: [(s::Type, s::Type); 2] = [(s::Type::Str, s::Type::Str), (s::Type::Str, s::Type::Int)];

fn handle(name: &str) -> s::Object {
    s::Object {
        name: name.into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle: true, role: None,
        implements: vec![], any_box: false, enum_names: vec![], fields: vec![],
        layout: s::ObjectRuntimeLayout { allocation_words: 2, identity_offset: None, has_destroy: false,
            may_reach_resource: false, may_async_destroy: false, trace_layout: vec![], selection_layout: vec![],
            storage_fields: vec![] },
    }
}

fn handles(ty: &AbiType, module: &str, names: &mut BTreeSet<String>) {
    match ty {
        AbiType::Handle(name) => {
            names.insert(format!("{}.{name}", module.replace('/', ".")));
        }
        AbiType::Array(item) => handles(item, module, names),
        AbiType::Map(key, value) => {
            handles(key, module, names);
            handles(value, module, names);
        }
        _ => {}
    }
}

fn asynchronous() -> impl Iterator<Item = Native> {
    Native::ALL.iter().copied().filter(|native| native.signature().is_async)
}

/// Every async native as the trusted catalogue builds it (a map result through its adapter), then the synchronous
/// natives that open a rooted entry and a file watch.
fn program() -> s::Program {
    let mut names = BTreeSet::new();
    let opening = [Native::FsRootFileRootOpen, Native::FsRootFileRootEntry, Native::FsFileWatchCreate];
    for native in asynchronous().chain(opening) {
        let signature = native.signature();
        for ty in signature.params.iter().chain([&signature.result]) {
            handles(ty, native.module(), &mut names);
        }
    }
    let objects = names.iter().map(|name| handle(name)).collect::<Vec<_>>();
    let composites = MAPS.iter().map(|&(key, value)| s::Composite::Map { key, value }).collect::<Vec<_>>();
    let contracts = s::p1::P1Contracts::new(&objects).unwrap();
    let mut functions = Vec::new();
    for native in asynchronous() {
        let function = contracts.native(native.symbol()).cloned().unwrap_or_else(|| {
            // A map result: the driver adapter over the program's composite (#752).
            let signature = native.signature();
            let (AbiType::Handle(name), AbiType::Map(_, value)) = (&signature.params[0], &signature.result) else {
                panic!("{}: a map native takes a handle", native.symbol());
            };
            let name = format!("{}.{name}", native.module().replace('/', "."));
            let handle = objects.iter().position(|object| object.name == name).unwrap() as u32;
            let map = if **value == AbiType::Int { 1 } else { 0 };
            let mut declared = contracts.native(SLEEP).unwrap().clone();
            declared.symbol = native.symbol().into();
            declared.proofs.entry.as_mut().unwrap().external = Some(native.symbol().into());
            declared.registers = vec![s::Type::Handle(handle)];
            declared.results = vec![s::Type::Map(map)];
            contracts.adapter(&declared, &composites).unwrap_or_else(|| panic!("{}", native.symbol()))
        });
        functions.push(function);
    }
    for symbol in [ROOT_OPEN, ROOT_ENTRY, WATCH_CREATE] {
        functions.push(contracts.native(symbol).unwrap().clone());
    }
    let mut future_results = Vec::new();
    for function in functions.iter().filter(|f| f.entry_abi == s::EntryAbi::AsyncExternal) {
        let outcome = function.results.first().copied();
        if !future_results.contains(&outcome) {
            future_results.push(outcome);
        }
    }
    s::Program {
        module: "natives".into(), objects, functions, composites,
        tables: s::ChampTables { future_results, ..s::ChampTables::default() },
        ..s::Program::default()
    }
}

fn index(runtime: &Runtime, name: &str) -> usize {
    runtime.champ.machine.as_ref().unwrap().source.functions.iter().position(|f| {
        f.proofs.entry.as_ref().and_then(|entry| entry.external.as_deref()) == Some(name)
    }).unwrap()
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

/// A cold driver Future of `function` over `args`, created with an empty inputs packet (ruling PR-N).
fn driver(ctx: &mut Ctx, function: usize, args: &[Word]) -> Word {
    let mut state = owner(ctx);
    let inputs = empty(ctx);
    new_future(ctx, function as u32, args, &mut state, inputs)
}

/// Run the woken frames until `future` settles, waiting for the host when nothing is ready.
fn settle(ctx: &mut Ctx, future: Word) {
    let runtime = unsafe { &mut *ctx.runtime };
    for _ in 0..10_000 {
        if matches!(get(future, tasks::STATE), tasks::DONE | tasks::FAILURE_STATE) {
            return;
        }
        match runtime.ready.pop_front() {
            Some(frame) => {
                runtime.run_body(ctx, frame);
            }
            None => assert!(runtime.wait_external(), "a pending driver has a timer or a request"),
        }
        collect(ctx, &[future]);
    }
    panic!("the driver did not settle");
}

#[test]
fn champ_n1_the_declaration_view_agrees_with_every_native_driver() {
    // Audit item 4 of the slice: Tomato's declaration view and the SPB 17 entry agree on the driver's register count
    // and kinds; a handle register is a Ref, as Tomato traces it. A native's legacy function resolves its provider.
    let runtime = load(false);
    // Tomato's resource tracking stays compiled out: a Champ runtime never tracks (Q1 audit A2).
    assert!(!runtime.resources.enabled);
    let source = runtime.champ.machine.as_ref().unwrap().source.clone();
    let legacy = &runtime.loaded.program.functions;
    let mut drivers = 0;
    for (function, entry) in source.functions.iter().enumerate() {
        if entry.entry_abi != s::EntryAbi::AsyncExternal {
            continue;
        }
        drivers += 1;
        let symbol = entry.proofs.entry.as_ref().unwrap().external.clone().unwrap();
        assert!(legacy[function].is_async && crate::program::is_driver(&legacy[function]), "{symbol}");
        assert_eq!(legacy[function].native.as_deref(), Some(symbol.as_str()));
        assert!(legacy[function].intrinsic.is_none() && runtime.intrinsics[function].is_none(), "{symbol}");
        let map = runtime.champ_frame_map(function).unwrap();
        assert_eq!((map.driver, map.inputs(), map.packet()), (Some(entry.params.len()), entry.params.len(),
            Some(map.tail + INPUTS)), "{symbol}");
        assert_eq!(runtime.heap.layouts().frames[function], map.trace, "{symbol}");
        // STATE, RESULT, FAILURE, WAITER, AWAITING, RESUME, FLAGS, OWNED; the registers; the driver slot; the
        // retained arguments; the Champ tail.
        let result = entry.results.first().map_or(Repr::Raw, |&ty| super::machine::repr(ty));
        assert_eq!(map.trace[1], result, "{symbol}");
        let registers = entry.registers.iter().map(|&ty| super::machine::repr(ty)).collect::<Vec<_>>();
        let count = registers.len();
        assert_eq!(map.trace[8..8 + count], registers[..], "{symbol}");
        assert_eq!(map.trace[8 + count], Repr::Ref, "{symbol}: the driver slot");
        assert_eq!(map.trace[9 + count..9 + 2 * count], registers[..], "{symbol}: the retained arguments");
        for (register, ty) in entry.registers.iter().enumerate() {
            if matches!(ty, s::Type::Handle(_) | s::Type::Bytes | s::Type::Str | s::Type::Array(_)) {
                assert_eq!(registers[register], Repr::Ref, "{symbol}: register {register}");
            }
        }
        assert_eq!(map.tail, 8 * (1 + 9 + 2 * count as u64), "{symbol}");
    }
    // #601 N1: the 51 async natives of std `runtime/`.
    assert_eq!(drivers, 51);
}

#[test]
fn champ_n1_a_sleep_completes_after_its_timer() {
    for stress in [false, true] {
        let mut runtime = load(stress);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let ctx = &mut ctx;
        let runtime = runtime_of(ctx);
        let sleep = index(runtime, SLEEP);
        let duration = runtime.heap.allocate_text("1000000");
        runtime.roots.push((duration, Repr::Ref));
        let future = driver(ctx, sleep, &[duration]);
        let tail = runtime.champ_frame_map(sleep).unwrap().tail;
        assert_eq!(get(future, tasks::STATE), tasks::COLD);
        // Creation (draft 3 §2.4): the argument in its register and its retained word, an empty INPUTS packet.
        assert_eq!((get(future, tasks::REGISTERS), get(future, tasks::REGISTERS + 16)), (duration, duration));
        assert!(runtime.champ_packet_keys(&PacketSlot { record: get(future, tail + INPUTS) }).unwrap().is_empty());
        runtime.champ_future_start(ctx, 0, future);
        // Started, it waits on its timer: a pending timer keeps the frame (`Runtime::timers` is a root).
        assert_eq!((ctx.status, get(future, tasks::STATE)), (OK, tasks::RUNNING));
        assert_eq!(get(future, tail + TASK), future);
        assert!(runtime.timers.iter().any(|&(_, frame)| frame == future));
        collect(ctx, &[]);
        assert_eq!(get(future, tasks::STATE), tasks::RUNNING);
        settle(ctx, future);
        // Done with Tomato's `true`, and an empty outcome packet in the cache (ABI §4.3: no fresh identity).
        assert_eq!((get(future, tasks::STATE), get(future, tasks::RESULT)), (tasks::DONE, 1));
        let outcome = runtime.champ_cache(future).unwrap().packet;
        assert!(outcome != 0 && runtime.champ_packet_keys(&PacketSlot { record: outcome }).unwrap().is_empty());
    }
}

#[test]
fn champ_n1_a_zero_sleep_completes_at_once_and_a_non_canonical_one_fails() {
    for stress in [false, true] {
        let mut runtime = load(stress);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let ctx = &mut ctx;
        let runtime = runtime_of(ctx);
        let sleep = index(runtime, SLEEP);
        // Tomato: `sleep(0)` completes at its start, without a timer.
        let zero = runtime.heap.allocate_text("0");
        runtime.roots.push((zero, Repr::Ref));
        let future = driver(ctx, sleep, &[zero]);
        runtime.champ_future_start(ctx, 0, future);
        assert_eq!((get(future, tasks::STATE), get(future, tasks::RESULT)), (tasks::DONE, 1));
        assert!(runtime.timers.is_empty());
        // A duration that is not a canonical non-negative integer is a program failure, which no catch takes.
        for text in ["01", "-1", "1.5", "", "1e3"] {
            let duration = runtime.heap.allocate_text(text);
            runtime.roots.push((duration, Repr::Ref));
            let future = driver(ctx, sleep, &[duration]);
            runtime.champ_future_start(ctx, 0, future);
            collect(ctx, &[future]);
            assert_eq!(get(future, tasks::STATE), tasks::FAILURE_STATE, "{text:?}");
            assert_eq!(kind(runtime, get(future, tasks::FAILURE)),
                "Program(\"Wait duration must be a canonical non-negative integer\")", "{text:?}");
            assert!(runtime.champ_cache(future).unwrap().packet == 0, "a failure publishes no outcome packet");
        }
    }
}

#[test]
fn champ_n1_a_sleep_cancelled_while_pending_withdraws_its_timer() {
    for stress in [false, true] {
        let mut runtime = load(stress);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let ctx = &mut ctx;
        let runtime = runtime_of(ctx);
        let sleep = index(runtime, SLEEP);
        let minute = runtime.heap.allocate_text("60000000000");
        runtime.roots.push((minute, Repr::Ref));
        let future = driver(ctx, sleep, &[minute]);
        runtime.champ_future_start(ctx, 0, future);
        assert!(runtime.timers.iter().any(|&(_, frame)| frame == future));
        // The request reaches the parked driver as Tomato's: queued with DELIVER_NOW; the driver removes its timer.
        assert_eq!(rt_cancel_request(ctx, future), NORMAL);
        assert_eq!(runtime.ready.iter().copied().collect::<Vec<_>>(), [future]);
        collect(ctx, &[future]);
        let woken = runtime.ready.pop_front().unwrap();
        assert!(runtime.run_body(ctx, woken));
        assert_eq!(get(future, tasks::STATE), tasks::FAILURE_STATE);
        assert_eq!(kind(runtime, get(future, tasks::FAILURE)), "Cancelled");
        assert!(runtime.timers.is_empty(), "nothing is left pending: no deadlock is hidden by a dead timer");
        assert!(!runtime.wait_external());
    }
}

/// A rooted entry of the crate's manifest, opened through the synchronous fs_root natives.
fn manifest(ctx: &mut Ctx) -> Word {
    let runtime = unsafe { &mut *ctx.runtime };
    let [open, entry] = [ROOT_OPEN, ROOT_ENTRY].map(|symbol| index(runtime, symbol));
    let path = runtime.heap.allocate_bytes(env!("CARGO_MANIFEST_DIR").as_bytes());
    runtime.roots.push((path, Repr::Ref));
    let root = crate::runtime::rt_native(ctx, open as u64, [path].as_ptr(), 1);
    assert_eq!(ctx.status, OK);
    runtime.roots.push((root, Repr::Ref));
    let name = runtime.heap.allocate_text("Cargo.toml");
    runtime.roots.push((name, Repr::Ref));
    let segments = runtime.heap.allocate_array(Repr::Ref, &[name]);
    runtime.roots.push((segments, Repr::Ref));
    let rooted = crate::runtime::rt_native(ctx, entry as u64, [root, segments].as_ptr(), 2);
    assert_eq!(ctx.status, OK);
    runtime.roots.push((rooted, Repr::Ref));
    rooted
}

#[test]
fn champ_n1_a_host_request_completes_with_its_reply() {
    for stress in [false, true] {
        let mut runtime = load(stress);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let ctx = &mut ctx;
        let runtime = runtime_of(ctx);
        let piece = index(runtime, PIECE);
        let rooted = manifest(ctx);
        let future = driver(ctx, piece, &[rooted, word::small(0).unwrap(), word::small(9).unwrap()]);
        runtime.champ_future_start(ctx, 0, future);
        settle(ctx, future);
        assert_eq!(get(future, tasks::STATE), tasks::DONE, "{:?}", runtime.fatal);
        assert_eq!(runtime.heap.bytes(get(future, tasks::RESULT)), b"[package]");
        assert!(runtime.io.requests.is_empty());
        // The reply is plain data: an empty outcome packet (ABI §4.3).
        let outcome = runtime.champ_cache(future).unwrap().packet;
        assert!(outcome != 0 && runtime.champ_packet_keys(&PacketSlot { record: outcome }).unwrap().is_empty());
    }
}

/// A watch of an empty directory of the test's own, whose `file_watch_event` request stays pending.
fn quiet_watch(ctx: &mut Ctx, directory: &std::path::Path) -> Word {
    let runtime = unsafe { &mut *ctx.runtime };
    std::fs::create_dir_all(directory).unwrap();
    let create = index(runtime, WATCH_CREATE);
    let path = runtime.heap.allocate_bytes(directory.to_str().unwrap().as_bytes());
    runtime.roots.push((path, Repr::Ref));
    let watch = crate::runtime::rt_native(ctx, create as u64, [path].as_ptr(), 1);
    assert_eq!(ctx.status, OK);
    runtime.roots.push((watch, Repr::Ref));
    watch
}

#[test]
fn champ_n1_a_pending_host_request_is_rooted_and_cancelled() {
    for stress in [false, true] {
        let directory = std::env::temp_dir().join(format!("okkama-n1-watch-{}-{stress}", std::process::id()));
        let mut runtime = load(stress);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let ctx = &mut ctx;
        let runtime = runtime_of(ctx);
        let event = index(runtime, WATCH);
        let watch = quiet_watch(ctx, &directory);
        let future = driver(ctx, event, &[watch]);
        runtime.champ_future_start(ctx, 0, future);
        // An external request always suspends its caller once, and is woken at once to poll: its frame is in
        // `io.requests`, a root of the collector while the request is in flight (audit item 2 of the slice).
        assert_eq!(get(future, tasks::STATE), tasks::RUNNING);
        assert!(runtime.io.requests.contains_key(&future));
        assert!(runtime.ready.is_empty());
        runtime.poll_io();
        assert_eq!(runtime.ready.iter().copied().collect::<Vec<_>>(), [future]);
        collect(ctx, &[]);
        let woken = runtime.ready.pop_front().unwrap();
        // `run_body` is false while the driver is pending: nothing changed in the directory.
        assert!(!runtime.run_body(ctx, woken));
        assert_eq!(get(future, tasks::STATE), tasks::RUNNING);
        assert!(runtime.io.requests.contains_key(&future));
        collect(ctx, &[]);
        // The request has no cleanup to wait for: `host::cancel` says so, and the driver settles Cancelled at once.
        assert_eq!(rt_cancel_request(ctx, future), NORMAL);
        collect(ctx, &[future]);
        let woken = runtime.ready.pop_front().unwrap();
        assert_eq!(woken, future);
        runtime.run_body(ctx, woken);
        assert_eq!(get(future, tasks::STATE), tasks::FAILURE_STATE);
        assert_eq!(kind(runtime, get(future, tasks::FAILURE)), "Cancelled");
        assert!(runtime.io.requests.is_empty());
        assert!(runtime.champ_cache(future).unwrap().packet == 0);
        let _ = std::fs::remove_dir(&directory);
    }
}

#[test]
fn champ_n1_a_host_refusal_is_the_drivers_failure() {
    for stress in [false, true] {
        let mut runtime = load(stress);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let ctx = &mut ctx;
        let runtime = runtime_of(ctx);
        let piece = index(runtime, PIECE);
        let rooted = manifest(ctx);
        // A negative offset: the provider refuses the request.
        let future = driver(ctx, piece, &[rooted, word::small(-1).unwrap(), word::small(9).unwrap()]);
        runtime.champ_future_start(ctx, 0, future);
        settle(ctx, future);
        assert_eq!(get(future, tasks::STATE), tasks::FAILURE_STATE);
        let failure = kind(runtime, get(future, tasks::FAILURE));
        assert!(failure.starts_with("Native") || failure.starts_with("Program"), "{failure}");
        assert!(runtime.champ_cache(future).unwrap().packet == 0);
    }
}
