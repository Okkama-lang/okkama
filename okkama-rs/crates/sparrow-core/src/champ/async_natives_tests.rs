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
const SQLITE_CREATE: &str = "runtime/database@1::sqlite_create/7";
const SQLITE_CONNECT: &str = "runtime/database@1::sqlite_connect/1";
const SQLITE_SCRIPT: &str = "runtime/database@1::sqlite_script/2";
const SQLITE_ROWS: &str = "runtime/database@1::sqlite_rows/4";
const SQLITE_CLOSE: &str = "runtime/database@1::sqlite_close/1";
const SQLITE_REPLY_OK: &str = "runtime/database@1::sqlite_reply_ok/1";
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
    let opening = [Native::FsRootFileRootOpen, Native::FsRootFileRootEntry, Native::FsFileWatchCreate,
        Native::DatabaseSqliteCreate, Native::DatabaseSqliteClose, Native::DatabaseSqliteReplyOk];
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
    for symbol in [ROOT_OPEN, ROOT_ENTRY, WATCH_CREATE, SQLITE_CREATE, SQLITE_CLOSE, SQLITE_REPLY_OK] {
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

/// An open in-memory SQLite connection: `sqlite_create` (`:memory:`), then its `sqlite_connect` driver settled.
fn sqlite(ctx: &mut Ctx) -> Word {
    let runtime = unsafe { &mut *ctx.runtime };
    let path = runtime.heap.allocate_bytes(b"");
    runtime.roots.push((path, Repr::Ref));
    let journal = runtime.heap.allocate_text("memory");
    runtime.roots.push((journal, Repr::Ref));
    let create = index(runtime, SQLITE_CREATE);
    // path, read_only, create, foreign_keys, busy_ms, journal, memory.
    let arguments = [path, 0, 1, 0, word::small(0).unwrap(), journal, 1];
    let state = crate::runtime::rt_native(ctx, create as u64, arguments.as_ptr(), arguments.len() as u64);
    assert_eq!(ctx.status, OK);
    runtime.roots.push((state, Repr::Ref));
    let connect = driver(ctx, index(runtime, SQLITE_CONNECT), &[state]);
    runtime.champ_future_start(ctx, 0, connect);
    settle(ctx, connect);
    assert_eq!(get(connect, tasks::STATE), tasks::DONE, "{:?}", runtime.fatal);
    state
}

/// A `sqlite_script` driver of `sql` on `state`, started.
fn script(ctx: &mut Ctx, state: Word, sql: &str) -> Word {
    let runtime = unsafe { &mut *ctx.runtime };
    let bytes = runtime.heap.allocate_bytes(sql.as_bytes());
    runtime.roots.push((bytes, Repr::Ref));
    let future = driver(ctx, index(runtime, SQLITE_SCRIPT), &[state, bytes]);
    runtime.champ_future_start(ctx, 0, future);
    future
}

/// A `sqlite_rows` driver of `sql` without parameters on `state`, started: on success it opens a cursor.
fn rows(ctx: &mut Ctx, state: Word, sql: &str) -> Word {
    let runtime = unsafe { &mut *ctx.runtime };
    let bytes = runtime.heap.allocate_bytes(sql.as_bytes());
    runtime.roots.push((bytes, Repr::Ref));
    let kinds = runtime.heap.allocate_array(Repr::Int, &[]);
    runtime.roots.push((kinds, Repr::Ref));
    let values = runtime.heap.allocate_array(Repr::Ref, &[]);
    runtime.roots.push((values, Repr::Ref));
    let future = driver(ctx, index(runtime, SQLITE_ROWS), &[state, bytes, kinds, values]);
    runtime.champ_future_start(ctx, 0, future);
    future
}

/// #799 (§7.1 test list, the `host::cancel == true` branch): an SQLite request has a cleanup, so `host::cancel` asks
/// the driver to keep polling. The cancel interrupts the connection's worker and waits for the request's reply;
/// meanwhile the frame stays in `io.requests`, a root of the collector. The driver then settles Cancelled. A
/// cancelled `rows` that still succeeded opened a cursor, which the cleanup releases before the driver settles, so
/// the connection serves the next statement (Tomato's `drivers::native_io`, `host::cancel`, `cleanup_cancelled`).
#[test]
fn champ_n1_a_cancelled_request_with_a_cleanup_is_polled_until_it_ends() {
    for stress in [false, true] {
        let mut runtime = load(stress);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let ctx = &mut ctx;
        let runtime = runtime_of(ctx);
        let state = sqlite(ctx);
        // More rows than one reply holds: a successful reply leaves its cursor open.
        let future = rows(ctx, state,
            "with recursive c(x) as (select 1 union all select x + 1 from c where x < 100000) select x from c");
        assert_eq!(get(future, tasks::STATE), tasks::RUNNING);
        assert!(runtime.io.requests.contains_key(&future));
        assert_eq!(rt_cancel_request(ctx, future), NORMAL);
        let mut steps = 0;
        while get(future, tasks::STATE) == tasks::RUNNING {
            // While the cleanup runs, the request keeps its frame rooted.
            assert!(runtime.io.requests.contains_key(&future), "a running request is in io.requests");
            match runtime.ready.pop_front() {
                Some(frame) => {
                    runtime.run_body(ctx, frame);
                }
                None => assert!(runtime.wait_external(), "the cleanup is a pending request"),
            }
            collect(ctx, &[future, state]);
            steps += 1;
            assert!(steps < 10_000, "the cleanup ends");
        }
        assert_eq!(get(future, tasks::STATE), tasks::FAILURE_STATE);
        assert_eq!(kind(runtime, get(future, tasks::FAILURE)), "Cancelled");
        assert!(runtime.io.requests.is_empty());
        let next = script(ctx, state, "create table t(x integer); insert into t values (1);");
        settle(ctx, next);
        assert_eq!(get(next, tasks::STATE), tasks::DONE, "{:?}", runtime.fatal);
        // The statement ran: no cursor of the cancelled request blocks the connection (SQLITE_READER_ACTIVE).
        let reply = get(next, tasks::RESULT);
        let ok = crate::runtime::rt_native(ctx, index(runtime, SQLITE_REPLY_OK) as u64, [reply].as_ptr(), 1);
        assert_eq!((ctx.status, ok), (OK, 1));
        let close = index(runtime, SQLITE_CLOSE);
        crate::runtime::rt_native(ctx, close as u64, [state].as_ptr(), 1);
        assert_eq!(ctx.status, OK);
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
        // Tomato's kind: a NativeFailure, which `catch e is NativeFailure` takes (language spec 9.5, #719, #797),
        // not a Program failure. Source: `host/native_root.rs` `rooted_coordinates` refuses with
        // `fail(n, "invalid_input", "Invalid rooted offset")`, `native_root` makes it `FailureKind::Native`, and
        // `drivers::native_io` fails the driver with it (`Start::Raise`). Observed on Tomato 0.5.8
        // (`okkama run --engine sparrow`, in the cage): std `RootedEntry.piece(-1, 9)` catches it as
        // NativeFailure and throws `FileError.invalid_operation("Invalid rooted offset")`.
        let record = runtime.failure(get(future, tasks::FAILURE)).unwrap();
        let FailureKind::Native(payload) = record.kind else {
            panic!("{:?}", record.kind);
        };
        // The payload is the four-string NativeFailure: domain, code, operation, message.
        let fields = (0..4).map(|field| runtime.heap.text(runtime.heap.field(payload, field)).to_owned());
        assert_eq!(fields.collect::<Vec<_>>(), ["core.fs.root", "invalid_input", PIECE, "Invalid rooted offset"]);
        assert!(runtime.champ_cache(future).unwrap().packet == 0);
    }
}

/// Function indices of [`smuggling`]: `work`, the body that smuggles its duty, and the `sleep_nanoseconds` driver.
const SMUGGLER: u32 = 1;
const SMUGGLED_SLEEP: u32 = 2;
/// The sleep driver's Future type: `future_results[1]` is its bool.
const FUTURE_BOOL: s::Type = s::Type::Future(1);

/// `async smuggler() { let f = work(7); let g = sleep_nanoseconds("1") with f's cache duty; }`: the cache duty
/// of `f` is absorbed into the packet that `fut.new` offers the native driver, and `g`'s duty settles at the
/// close. PR-N's loader check counts resource keys, not service entries; the D25 rule `SPB_FUTURE_INPUT` refuses
/// the packet statically, and the machine refuses it when `fut.new` runs, whichever loader the program met.
fn smuggler() -> (s::Function, s::FrameLayout) {
    let slots = vec![service(K::Packet)];
    let (mut b, frame) = Body::asynchronous("smuggler", SMUGGLER, vec![]);
    let (body, inputs) = b.resume(frame, 0, &types(&slots, &[0]));
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(SMUGGLER), A::Boundary(SMUGGLER)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[0])], K::ScopeState);
    let seven = b.constant(s::Constant::Int("7".into()), s::Type::Int);
    let (_, cache, q2) = future_new(&mut b, SMUGGLER, WORK, &[seven], q1);
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let offered = b.emit(O::ServiceAbsorb, vec![l(empty), l(cache)], K::Packet);
    let duration = b.constant(s::Constant::Str("1".into()), s::Type::Str);
    let [sleep, sleep_cache, q3] = [b.register(FUTURE_BOOL), b.service(K::CacheState), b.service(K::ScopeState)];
    b.next(O::FutureNew, vec![A::Entry(SMUGGLED_SLEEP), A::Scope(SMUGGLER), A::Result(Some(FUTURE_BOOL)),
        l(duration), l(offered), l(q2)], vec![sleep, sleep_cache, q3]);
    b.return_value(body, q3, SMUGGLER, None, &[sleep_cache], "done");
    let layout = b.layout(slots, vec![(vec![0], vec![])]);
    (b.finish(), layout)
}

fn smuggling() -> s::Program {
    let contracts = s::p1::P1Contracts::new(&[]).unwrap();
    let mut functions = Vec::new();
    let mut frames = Vec::new();
    for (function, layout) in [work(), smuggler()] {
        functions.push(function);
        frames.push(layout);
    }
    functions.push(contracts.native(SLEEP).unwrap().clone());
    s::Program {
        module: "smuggling".into(),
        functions,
        tables: s::ChampTables {
            scopes: (0..=SMUGGLER).map(|index| s::Scope { function: index, parent: None, boundary: index }).collect(),
            boundaries: (0..=SMUGGLER).map(|scope| s::Boundary { scope }).collect(),
            frames,
            future_results: vec![Some(s::Type::Int), Some(s::Type::Bool), None],
            ..s::ChampTables::default()
        },
        ..s::Program::default()
    }
}

#[test]
fn champ_n1_a_native_driver_refuses_an_absorbed_duty_in_its_inputs() {
    // Ruling PR-N at run time: a native driver's inputs packet is empty, service entries included. The machine
    // stops at the `fut.new`, as the synchronous P1 boundary does, on both engines and under collections.
    // The guard is the backstop of the loader rules, so the program reaches the machine without them.
    let built = smuggling();
    let assembled = s::assemble(&s::disassemble(&built).unwrap()).unwrap();
    assert_eq!(assembled, built);
    let verified = crate::champ::machine::Verified::unchecked(assembled);
    for (stress, engine) in configurations() {
        let mut runtime = Runtime::new_champ_verified(&verified).unwrap();
        runtime.heap.set_stress(stress);
        if engine == Engine::Reference {
            for function in 0..=SMUGGLER as usize {
                runtime.invocations[function].code = reference as *const () as u64;
            }
        }
        let report = runtime.run_function(SMUGGLER as usize, crate::STACK_BYTES).unwrap_err().0;
        assert!(report.contains("a native driver's inputs packet received obligations (PR-N)"),
            "{stress} {engine:?}: {report}");
        // Nothing was created: the smuggled Future never ran, and no sleep was started.
        assert_eq!(count(&runtime, WORK, FutureStep::Start), 0, "{stress} {engine:?}");
        assert!(runtime.timers.is_empty(), "{stress} {engine:?}");
    }
}
