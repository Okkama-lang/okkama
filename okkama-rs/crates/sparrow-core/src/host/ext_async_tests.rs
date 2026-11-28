//! Async extension ownership and scheduler boundaries.
#![allow(unsafe_code)]
use super::*;
use crate::{budget::Limits, ext::Extension, program::Loaded, runtime::Runtime};
use okkama_runtime_abi::ext::*;
use std::{ffi::c_void, ptr, sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}}, task::Waker};

#[test]
fn ext_async_characterization_cleanup_keeps_request_pending() {
    let mut r = Box::new(Runtime::new(Loaded::new(okkama_sparrow::Program::default())).unwrap());
    let released = Arc::new(AtomicBool::new(false));
    let gate = released.clone();
    let future = with_host(&mut r, true, |h| h.native_io_with_cleanup(
        std::future::pending::<Result<bool, okkama_runtime_abi::NativeFailure>>(),
        Some(Box::new(move || Box::pin(std::future::poll_fn(move |_| {
            if gate.load(Ordering::SeqCst) { std::task::Poll::Ready(Ok(())) } else { std::task::Poll::Pending }
        })))),
    ).unwrap());
    let native = okkama_runtime_abi::Native::TimeSleepNanoseconds;
    assert!(cancel(&mut r, future.bits));
    assert!(poll(&mut r, native, future.bits, Waker::noop()).is_none());
    released.store(true, Ordering::SeqCst);
    let Some(Start::Raise(failure)) = poll(&mut r, native, future.bits, Waker::noop()) else { panic!("cancel outcome"); };
    assert!(matches!(r.failure(failure).unwrap().kind, crate::runtime::FailureKind::Cancelled));
}

#[cfg(feature = "jit")]
#[test]
fn ext_async_callback_stack_matches_runtime_headroom() {
    assert_eq!(OKKAMA_EXT_STACK as u64, crate::runtime::STACK_HEADROOM - crate::jit::MAX_FRAME);
}

#[derive(Default)]
struct Probe {
    prepared: AtomicUsize,
    executed: AtomicUsize,
    completed: AtomicUsize,
    dropped: AtomicUsize,
    state_dropped: AtomicUsize,
    cancelled: AtomicBool,
    leave: AtomicBool,
    returned: AtomicBool,
    wrong_thread: AtomicBool,
    early_drop: AtomicBool,
    prepare_status: u32,
    complete_status: u32,
    fault: u32,
}
thread_local! { static PROBE: std::cell::RefCell<Arc<Probe>> = std::cell::RefCell::new(Arc::new(Probe::default())); }
fn probe() -> Arc<Probe> { PROBE.with(|p| p.borrow().clone()) }
fn install(probe: Probe) -> Arc<Probe> {
    let probe = Arc::new(probe);
    PROBE.with(|p| *p.borrow_mut() = probe.clone());
    probe
}
fn ordinary() -> Arc<Probe> { install(Probe { leave: AtomicBool::new(true), ..Default::default() }) }
fn wait_for(test: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !test() {
        assert!(std::time::Instant::now() < deadline, "blocking callback did not progress");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

// Owned copies deliberately share no VM words or tokens with complete.
enum Value {
    Scalar(okk_slot),
    Big(i32, Vec<u8>),
    Text(Vec<u8>),
    Ratio(Box<(Value, Value)>),
    Array(Vec<Value>),
    Map(Vec<Value>, Vec<Value>),
    State(Arc<i64>),
}
struct StatePayload(Arc<i64>, Arc<Probe>, std::thread::ThreadId);
unsafe extern "C" fn drop_state(p: *mut c_void) {
    let state = unsafe { Box::from_raw(p.cast::<StatePayload>()) };
    state.1.wrong_thread.fetch_or(state.2 != std::thread::current().id(), Ordering::SeqCst);
    state.1.state_dropped.fetch_add(1, Ordering::SeqCst);
}
static STATE: okk_state_type = okk_state_type { name: c"State".as_ptr(), drop: drop_state };
fn checked(status: u32) -> Result<(), u32> { if status == OKK_OK { Ok(()) } else { Err(status) } }
unsafe fn read_value(c: *mut okk_call, value: okk_slot) -> Result<Value, u32> {
    unsafe {
        let h = &*(*c).host;
        Ok(match value.kind {
            OKK_BIGINT => {
                let mut sign = 0; let mut data = ptr::null(); let mut len = 0;
                checked((h.bigint_read)(c, value, &mut sign, &mut data, &mut len))?;
                Value::Big(sign, std::slice::from_raw_parts(data, len).to_vec())
            }
            OKK_STRING => {
                let mut data = ptr::null(); let mut len = 0;
                checked((h.text)(c, value, &mut data, &mut len))?;
                Value::Text(std::slice::from_raw_parts(data, len).to_vec())
            }
            OKK_RAT => {
                let mut n = okk_slot::default(); let mut d = n;
                checked((h.rat_parts)(c, value, &mut n, &mut d))?;
                Value::Ratio(Box::new((read_value(c, n)?, read_value(c, d)?)))
            }
            OKK_ARRAY => {
                let mut len = 0; checked((h.array_len)(c, value, &mut len))?;
                let mut items = vec![okk_slot::default(); len];
                checked((h.array_read)(c, value, 0, items.as_mut_ptr(), len))?;
                Value::Array(items.into_iter().map(|v| read_value(c, v)).collect::<Result<_, _>>()?)
            }
            OKK_MAP => {
                let mut len = 0; checked((h.map_len)(c, value, &mut len))?;
                let mut keys = vec![okk_slot::default(); len]; let mut values = keys.clone();
                checked((h.map_entries)(c, value, keys.as_mut_ptr(), values.as_mut_ptr(), len))?;
                Value::Map(keys.into_iter().map(|v| read_value(c, v)).collect::<Result<_, _>>()?,
                    values.into_iter().map(|v| read_value(c, v)).collect::<Result<_, _>>()?)
            }
            OKK_STATE => {
                let mut payload = ptr::null_mut();
                checked((h.state_get)(c, value, &STATE, &mut payload))?;
                Value::State((*payload.cast::<StatePayload>()).0.clone())
            }
            _ => Value::Scalar(value),
        })
    }
}
unsafe fn write_value(c: *mut okk_call, value: &Value) -> Result<okk_slot, u32> {
    unsafe {
        let h = &*(*c).host;
        let mut out = okk_slot::default();
        let status = match value {
            Value::Scalar(value) => { out = *value; OKK_OK }
            Value::Big(sign, bytes) => (h.bigint_new)(c, *sign, bytes.as_ptr(), bytes.len(), &mut out),
            Value::Text(bytes) => (h.text_new)(c, bytes.as_ptr(), bytes.len(), &mut out),
            Value::Ratio(parts) => (h.rat_new)(c, write_value(c, &parts.0)?, write_value(c, &parts.1)?, &mut out),
            Value::Array(items) => {
                let items = items.iter().map(|v| write_value(c, v)).collect::<Result<Vec<_>, _>>()?;
                (h.array_new)(c, items.as_ptr(), items.len(), &mut out)
            }
            Value::Map(keys, values) => {
                let keys = keys.iter().map(|v| write_value(c, v)).collect::<Result<Vec<_>, _>>()?;
                let values = values.iter().map(|v| write_value(c, v)).collect::<Result<Vec<_>, _>>()?;
                (h.map_new)(c, keys.as_ptr(), values.as_ptr(), keys.len(), &mut out)
            }
            Value::State(shared) => {
                let payload = Box::into_raw(Box::new(StatePayload(shared.clone(), probe(), std::thread::current().id()))).cast();
                let status = (h.state_new)(c, &STATE, payload, 64, &mut out);
                if status != OKK_OK { drop(Box::from_raw(payload.cast::<StatePayload>())); }
                status
            }
        };
        checked(status)?;
        Ok(out)
    }
}
struct Job {
    value: Value,
    probe: Arc<Probe>,
    vm: std::thread::ThreadId,
    executed: bool,
    returned: bool,
    stale: okk_slot,
}
unsafe fn fail(c: *mut okk_call) -> u32 {
    let status = unsafe { ((*(*c).host).fail)(c, b"refused".as_ptr(), 7, b"extension failed".as_ptr(), 16) };
    if status == OKK_OK { OKK_FAILED } else { status }
}
unsafe extern "C" fn prepare(c: *mut okk_call, a: *const okk_slot, out: *mut *mut c_void) -> u32 {
    unsafe {
        let probe = probe();
        let value = match read_value(c, *a) { Ok(v) => v, Err(s) => return s };
        let status = probe.prepare_status;
        probe.prepared.fetch_add(1, Ordering::SeqCst);
        let job = Box::into_raw(Box::new(Job { value, probe, vm: std::thread::current().id(), executed: false, returned: false, stale: *a })).cast();
        if status != OKK_OK {
            drop_job(job);
            return if status == OKK_FAILED { fail(c) } else { status };
        }
        *out = job;
        if (&*job.cast::<Job>()).probe.fault == 5 {
            ((*(*c).host).text)(c, okk_slot::default(), &mut ptr::null(), &mut 0);
        }
        OKK_OK
    }
}
unsafe extern "C" fn execute(p: *mut c_void, cancel: *const okk_cancel) {
    let job = unsafe { &mut *p.cast::<Job>() };
    job.executed = true;
    job.probe.wrong_thread.fetch_or(job.vm == std::thread::current().id(), Ordering::SeqCst);
    job.probe.executed.fetch_add(1, Ordering::SeqCst);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !job.probe.leave.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
        if unsafe { ((*cancel).cancelled)(cancel) } == 1 { job.probe.cancelled.store(true, Ordering::SeqCst); }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    job.returned = true;
    job.probe.returned.store(true, Ordering::SeqCst);
}
unsafe extern "C" fn complete(c: *mut okk_call, p: *mut c_void, out: *mut okk_slot) -> u32 {
    unsafe {
        let job = &*p.cast::<Job>();
        job.probe.wrong_thread.fetch_or(job.vm != std::thread::current().id(), Ordering::SeqCst);
        job.probe.completed.fetch_add(1, Ordering::SeqCst);
        match job.probe.fault {
            1 => { *out = job.stale; return OKK_OK; }
            2 => { *out = okk_slot { bits: 1, kind: OKK_BOOL }; return OKK_OK; }
            3 => { ((*(*c).host).text)(c, okk_slot::default(), &mut ptr::null(), &mut 0); }
            4 => {
                let mut buffer = ptr::null_mut();
                let status = ((*(*c).host).bytes_reserve)(c, 1 << 24, &mut buffer, out);
                if status != OKK_STOPPED { return OKK_PANIC; }
                if ((*(*c).host).text_new)(c, b"x".as_ptr(), 1, out) != OKK_STOPPED { return OKK_PANIC; }
                return OKK_OK;
            }
            _ => {}
        }
        if job.probe.complete_status != OKK_OK {
            return if job.probe.complete_status == OKK_FAILED { fail(c) } else { job.probe.complete_status };
        }
        match write_value(c, &job.value) { Ok(value) => { *out = value; OKK_OK }, Err(s) => s }
    }
}
unsafe extern "C" fn drop_job(p: *mut c_void) {
    let job = unsafe { Box::from_raw(p.cast::<Job>()) };
    if job.executed && !job.returned { job.probe.early_drop.store(true, Ordering::SeqCst); }
    job.probe.dropped.fetch_add(1, Ordering::SeqCst);
}
static CALLBACKS: okk_async = okk_async { prepare, execute, complete, drop: drop_job };
unsafe extern "C" fn control(_: *mut okk_call, a: *const okk_slot, out: *mut okk_slot) -> u32 {
    let p = probe();
    let value = match unsafe { (*a).bits } {
        0 => p.prepared.load(Ordering::SeqCst),
        1 => { p.leave.store(true, Ordering::SeqCst); 0 }
        2 => p.completed.load(Ordering::SeqCst),
        _ => p.dropped.load(Ordering::SeqCst),
    };
    unsafe { *out = okk_slot { bits: value as u64, kind: OKK_INT }; }
    OKK_OK
}
unsafe extern "C" fn create(c: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    match unsafe { write_value(c, &Value::State(Arc::new(42))) } { Ok(v) => { unsafe { *out = v; } OKK_OK }, Err(s) => s }
}
unsafe extern "C" fn read(c: *mut okk_call, a: *const okk_slot, out: *mut okk_slot) -> u32 {
    match unsafe { read_value(c, *a) } {
        Ok(Value::State(v)) => { unsafe { *out = okk_slot { bits: *v as u64, kind: OKK_INT }; } OKK_OK }
        Err(s) => s, _ => OKK_PANIC,
    }
}
unsafe extern "C" fn release(c: *mut okk_call, a: *const okk_slot, _: *mut okk_slot) -> u32 {
    unsafe { ((*(*c).host).state_release)(c, *a, &STATE) }
}
static EXTRA: &[okk_export] = &[
    okk_export { symbol: c"async_ext@0::control/1".as_ptr(), signature: c"(int) -> int".as_ptr(), params: c"".as_ptr(), sync: Some(control), r#async: ptr::null() },
    okk_export { symbol: c"async_ext@0::create/0".as_ptr(), signature: c"() -> State".as_ptr(), params: c"".as_ptr(), sync: Some(create), r#async: ptr::null() },
    okk_export { symbol: c"async_ext@0::read/1".as_ptr(), signature: c"(State) -> int".as_ptr(), params: c"".as_ptr(), sync: Some(read), r#async: ptr::null() },
    okk_export { symbol: c"async_ext@0::release/1".as_ptr(), signature: c"(State) -> void".as_ptr(), params: c"".as_ptr(), sync: Some(release), r#async: ptr::null() },
];
fn limits(ty: &str) -> Limits {
    let signature = Box::leak(std::ffi::CString::new(format!("async ({ty}) -> {ty}")).unwrap().into_boxed_c_str());
    let mut exports: Vec<_> = EXTRA.iter().map(|e| okk_export { symbol: e.symbol, signature: e.signature, params: e.params, sync: e.sync, r#async: e.r#async }).collect();
    exports.push(okk_export { symbol: c"async_ext@0::echo/1".as_ptr(), signature: signature.as_ptr(), params: c"".as_ptr(), sync: None, r#async: &CALLBACKS });
    let exports = exports.leak();
    let descriptor = Box::leak(Box::new(okk_extension {
        abi: OKKAMA_EXT_ABI, size: std::mem::size_of::<okk_extension>() as u32,
        name: c"async_ext".as_ptr(), exports: exports.as_ptr(), export_count: exports.len() as u32, states: &STATE, state_count: 1,
    }));
    Limits { extensions: vec![unsafe { Extension::from_static(descriptor).unwrap() }], ..Default::default() }
}
struct Stdlib;
impl okkama_compiler::lowering::ModuleLoader for Stdlib {
    fn load(&mut self, path: &str, _: &str) -> Result<Option<okkama_compiler::lowering::LoadedModule>, String> {
        use okkama_compiler::{SourceFile, syntax, lowering::LoadedModule};
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../std").join(format!("{path}.ok"));
        let source = SourceFile::new(file.to_string_lossy(), std::fs::read_to_string(&file).map_err(|e| e.to_string())?).unwrap();
        let program = syntax::parse(&source, &syntax::ParseOptions::default()).unwrap();
        Ok(Some(LoadedModule { id: source.name().into(), source, program: syntax::prune::strip_test_items(program) }))
    }
}
fn source_runtime(ty: &str, body: &str) -> Box<Runtime> {
    use okkama_compiler::{SourceFile, syntax, lowering, codegen};
    let language_ty = if ty == "?int" { "Maybe<int>" } else { ty };
    let body = body.replace("?int", "Maybe<int>");
    let text = format!(r#"module async_ext;
        native object State;
        native async echo(value is {language_ty}) is {language_ty};
        native control(mode is int) is int;
        native create() is State;
        native read(s is State) is int;
        native release(s is State) is void;
        object Work {{ Work(); async run(value is {language_ty}) is {language_ty} {{ await echo(value); }} }}
        {body}
    "#);
    let source = SourceFile::new("ext-async.ok", text.clone()).unwrap();
    let syntax = syntax::parse(&source, &syntax::ParseOptions::default()).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
    let ir = lowering::lower_linked(&syntax, source.name(), &mut Stdlib).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
    let p = codegen::sparrow::generate(&ir).unwrap();
    p.verify().unwrap();
    Box::new(Runtime::with_limits(Loaded::new(p), limits(ty)).unwrap())
}
fn runtime() -> Box<Runtime> {
    let p = okkama_sparrow::assemble(r#"sparrow 19
module "async_ext" "1"
type t0 int
function f0 "echo" async native "async_ext@0::echo/1" {
 registers [t0]
 params [r0]
 result t0
}
"#).unwrap();
    Box::new(Runtime::with_limits(Loaded::new(p), limits("int")).unwrap())
}
fn assert_clean(p: &Probe, jobs: usize, completed: usize) {
    assert_eq!(p.prepared.load(Ordering::SeqCst), jobs);
    assert_eq!(p.dropped.load(Ordering::SeqCst), jobs);
    assert_eq!(p.completed.load(Ordering::SeqCst), completed);
    assert!(!p.wrong_thread.load(Ordering::SeqCst));
    assert!(!p.early_drop.load(Ordering::SeqCst));
}
#[test]
fn ext_async_integer_echo() {
    let p = ordinary();
    let mut r = runtime();
    let value = r.heap.int(42);
    let result = r.run_function_with(0, &[value], 1 << 24).unwrap();
    assert_eq!(r.small_or_big(result), Some(42));
    drop(r);
    assert_clean(&p, 1, 1);
}
#[test]
fn ext_async_echo_value_kinds_and_nested_values() {
    for (ty, value) in [
        ("int", "-9223372036854775808"), ("int", "184467440737095516170"),
        ("float", "-123.5"), ("bool", "true"), ("byte", "byte(255)"), ("char", "'🐦'"),
        ("rat", "3/7"), ("string", "\"echo 🐦\""), ("@byte", "[byte(0), byte(128), byte(255)]"),
        ("@int", "[1, 184467440737095516170]"), ("%string:@int", "[\"key\": [1, 2]]"),
        ("@@int", "[[1, 2], [3, 4]]"), ("?int", "Maybe.some(42)"), ("?int", "Maybe<int>.none"),
    ] {
        let p = ordinary();
        let mut r = source_runtime(ty, &format!("async main() is int {{ let value is {ty} = {value}; confirm (await echo(value)) == value; 0; }}"));
        assert_eq!(r.run_entry(1 << 24).unwrap(), 0, "{ty}");
        drop(r);
        assert_clean(&p, 1, 1);
    }
}
#[test]
fn ext_async_is_cold_and_cancellation_before_start_creates_no_job() {
    let p = ordinary();
    let mut r = source_runtime("int", r#"async main() is int {
        let f = echo(42);
        confirm control(0) == 0;
        f.cancel();
        let cancelled = try { await f; false; } catch e is Cancelled { true; };
        confirm cancelled;
        control(0);
    }"#);
    assert_eq!(r.run_entry(1 << 24).unwrap(), 0);
    drop(r);
    assert_clean(&p, 0, 0);
}
#[test]
fn ext_async_two_tasks_progress_while_execute_sleeps() {
    let p = install(Probe::default());
    let mut r = source_runtime("int", r#"async main() is int {
        let a = spawn Work().run(17);
        let b = spawn Work().run(29);
        confirm control(0) == 2;
        confirm control(2) == 0;
        control(1);
        (await a) + (await b);
    }"#);
    assert_eq!(r.run_entry(1 << 24).unwrap(), 46);
    drop(r);
    assert_clean(&p, 2, 2);
}
fn start_job(r: &mut Runtime) -> (NativeKey, u64) {
    let key = NativeKey::Extension(r.external[0].clone().unwrap());
    let value = r.heap.int(42);
    let Start::Pending(id) = start(r, key.clone(), &[value]) else { panic!("pending job"); };
    (key, id)
}
fn finish(r: &mut Runtime, key: NativeKey, id: u64) -> Start {
    let mut result = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while result.is_none() {
        result = poll(r, key.clone(), id, Waker::noop());
        assert!(std::time::Instant::now() < deadline);
        if result.is_none() { std::thread::sleep(std::time::Duration::from_millis(1)); }
    }
    result.unwrap()
}
#[test]
fn ext_async_cancellation_waits_for_execute_and_drops_without_complete() {
    let p = install(Probe::default());
    let mut r = runtime();
    let (key, id) = start_job(&mut r);
    wait_for(|| p.executed.load(Ordering::SeqCst) == 1);
    assert!(cancel(&mut r, id));
    assert!(cancel(&mut r, id));
    wait_for(|| p.cancelled.load(Ordering::SeqCst));
    assert!(poll(&mut r, key.clone(), id, Waker::noop()).is_none());
    assert_eq!(p.dropped.load(Ordering::SeqCst), 0);
    p.leave.store(true, Ordering::SeqCst);
    let Start::Raise(failure) = finish(&mut r, key, id) else { panic!("cancel outcome"); };
    assert!(matches!(r.failure(failure).unwrap().kind, crate::runtime::FailureKind::Cancelled));
    drop(r);
    assert_clean(&p, 1, 0);
}
#[test]
fn ext_async_cancel_after_completion_keeps_the_result() {
    let p = ordinary();
    let mut r = source_runtime("int", r#"async main() is int {
        let f = echo(42); confirm (await f) == 42;
        f.cancel(); await f;
    }"#);
    assert_eq!(r.run_entry(1 << 24).unwrap(), 42);
    drop(r);
    assert_clean(&p, 1, 1);
}
#[test]
fn ext_async_prepare_and_complete_failures_are_native_failures() {
    for preparing in [true, false] {
        let p = install(Probe { leave: AtomicBool::new(true), prepare_status: if preparing { OKK_FAILED } else { OKK_OK },
            complete_status: if preparing { OKK_OK } else { OKK_FAILED }, ..Default::default() });
        let mut r = source_runtime("int", r#"async main() is int {
            try { await echo(42); 1; } catch e is NativeFailure {
                confirm e.domain() == "async_ext";
                confirm e.operation() == "async_ext@0::echo/1";
                confirm e.code() == "refused";
                confirm e.message() == "extension failed";
                0;
            }
        }"#);
        assert_eq!(r.run_entry(1 << 24).unwrap(), 0);
        assert!(!r.poisoned);
        drop(r);
        assert_clean(&p, 1, usize::from(!preparing));
    }
}
#[test]
fn ext_async_contract_faults_poison_and_still_drop_once() {
    for fault in [1, 2, 3, 5] {
        let p = install(Probe { leave: AtomicBool::new(true), fault, ..Default::default() });
        let ty = if fault == 1 { "string" } else { "int" };
        let value = if fault == 1 { "\"stale\"" } else { "42" };
        let mut r = source_runtime(ty, &format!("async main() is int {{ try {{ await echo({value}); }} catch e is Error {{}} 0; }}"));
        assert!(r.run_entry(1 << 24).unwrap_err().0.contains("internal error in native"));
        assert!(r.poisoned);
        assert!(r.run_entry(1 << 24).unwrap_err().0.contains("unusable"));
        drop(r);
        assert_clean(&p, 1, usize::from(fault != 5));
    }
    for status in [OKK_PANIC, OKK_STOPPED, OKK_BAD_TOKEN, OKK_BAD_KIND, OKK_BAD_SHAPE, OKK_BAD_TEXT, OKK_RANGE, OKK_UNSUPPORTED, u32::MAX] {
        for preparing in [true, false] {
            let p = install(Probe { leave: AtomicBool::new(true), prepare_status: if preparing { status } else { OKK_OK },
                complete_status: if preparing { OKK_OK } else { status }, ..Default::default() });
            let mut r = runtime();
            let value = r.heap.int(42);
            assert!(r.run_function_with(0, &[value], 1 << 24).unwrap_err().0.contains("internal error in native"));
            assert!(r.poisoned);
            drop(r);
            assert_clean(&p, 1, usize::from(!preparing));
        }
    }
}
#[test]
fn ext_async_release_during_execute_keeps_the_jobs_arc_alive() {
    let p = install(Probe::default());
    let mut r = source_runtime("State", r#"async main() is int {
        let state = create();
        let task = spawn Work().run(state);
        release(state);
        control(1);
        let copied = await task;
        let value = read(copied);
        release(copied);
        value;
    }"#);
    assert_eq!(r.run_entry(1 << 24).unwrap(), 42);
    drop(r);
    assert_eq!(p.state_dropped.load(Ordering::SeqCst), 2);
    assert_clean(&p, 1, 1);
}
#[test]
fn ext_async_teardown_signals_and_waits_for_the_owned_job() {
    let p = install(Probe::default());
    let mut r = runtime();
    start_job(&mut r);
    wait_for(|| p.executed.load(Ordering::SeqCst) == 1);
    let worker = p.clone();
    let release = std::thread::spawn(move || {
        wait_for(|| worker.cancelled.load(Ordering::SeqCst));
        assert_eq!(worker.dropped.load(Ordering::SeqCst), 0);
        worker.leave.store(true, Ordering::SeqCst);
    });
    drop(r);
    release.join().unwrap();
    wait_for(|| p.dropped.load(Ordering::SeqCst) == 1);
    assert_clean(&p, 1, 0);
}
#[test]
fn ext_async_interrupt_and_timeout_reach_execute() {
    for timeout in [false, true] {
        let p = install(Probe::default());
        let mut r = runtime();
        r.limits.interrupt = Some(Arc::new(AtomicBool::new(false)));
        let (key, id) = start_job(&mut r);
        wait_for(|| p.executed.load(Ordering::SeqCst) == 1);
        if timeout { r.limits.option("--timeout", "0.01").unwrap(); }
        else { r.limits.interrupt.as_ref().unwrap().store(true, Ordering::SeqCst); }
        wait_for(|| p.cancelled.load(Ordering::SeqCst));
        assert!(cancel(&mut r, id));
        assert!(poll(&mut r, key.clone(), id, Waker::noop()).is_none());
        p.leave.store(true, Ordering::SeqCst);
        assert!(matches!(finish(&mut r, key, id), Start::Raise(_)));
        drop(r);
        assert_clean(&p, 1, 0);
    }
}
#[test]
fn ext_async_complete_memory_stop_is_sticky_and_not_catchable() {
    let p = install(Probe { leave: AtomicBool::new(true), fault: 4, ..Default::default() });
    let mut r = source_runtime("int", "async main() is int { try { await echo(42); } catch e is Error {} 0; }");
    r.limits.memory_scope = crate::budget::MemoryScope::Runtime;
    r.limits.memory = Some(2 << 20);
    let error = r.run_entry(1 << 24).unwrap_err();
    assert_eq!(error.0, crate::budget::Exhausted::Memory.message());
    assert!(!r.poisoned);
    drop(r);
    assert_clean(&p, 1, 1);
}

#[test]
fn ext_async_language_cancellation_drains_the_request() {
    let p = install(Probe::default());
    let mut r = source_runtime("int", r#"async main() is int {
        let task = spawn Work().run(42);
        confirm control(0) == 1;
        task.cancel();
        confirm control(2) == 0;
        control(1);
        let cancelled = try { await task; false; } catch e is Cancelled { true; };
        confirm cancelled;
        control(3);
    }"#);
    assert_eq!(r.run_entry(1 << 24).unwrap(), 1);
    drop(r);
    assert_clean(&p, 1, 0);
}
#[test]
fn ext_async_cancel_completed_execute_before_publication() {
    let p = ordinary();
    let mut r = runtime();
    let (key, id) = start_job(&mut r);
    let request = r.host.as_ref().unwrap().futures.get(id).unwrap().io.clone().unwrap();
    wait_for(|| request.poll(&mut std::task::Context::from_waker(Waker::noop())).is_ready());
    assert_eq!(p.completed.load(Ordering::SeqCst), 0);
    assert!(cancel(&mut r, id));
    assert!(matches!(finish(&mut r, key, id), Start::Raise(_)));
    drop(request);
    drop(r);
    assert_clean(&p, 1, 0);
}
#[test]
fn ext_async_work_is_not_charged_steps() {
    let p = install(Probe::default());
    let mut r = runtime();
    r.limits.steps = Some(1);
    let worker = p.clone();
    let release = std::thread::spawn(move || {
        wait_for(|| worker.executed.load(Ordering::SeqCst) == 1);
        std::thread::sleep(std::time::Duration::from_millis(10));
        worker.leave.store(true, Ordering::SeqCst);
    });
    let value = r.heap.int(42);
    let result = r.run_function_with(0, &[value], 1 << 24).unwrap();
    assert_eq!(r.small_or_big(result), Some(42));
    release.join().unwrap();
    drop(r);
    assert_clean(&p, 1, 1);
}
#[test]
fn ext_async_run_interrupt_stops_without_publishing() {
    for timeout in [false, true] {
        let p = install(Probe::default());
        let mut r = runtime();
        let flag = Arc::new(AtomicBool::new(false));
        r.limits.interrupt = Some(flag.clone());
        let worker = p.clone();
        let interrupt = std::thread::spawn(move || {
            wait_for(|| worker.executed.load(Ordering::SeqCst) == 1);
            if timeout {
                let mut timer = Limits { interrupt: Some(flag), ..Default::default() };
                timer.option("--timeout", "0.01").unwrap();
            } else { flag.store(true, Ordering::SeqCst); }
            wait_for(|| worker.cancelled.load(Ordering::SeqCst));
            worker.leave.store(true, Ordering::SeqCst);
        });
        let value = r.heap.int(42);
        assert_eq!(r.run_function_with(0, &[value], 1 << 24).unwrap_err().0, crate::budget::Exhausted::Interrupted.message());
        interrupt.join().unwrap();
        drop(r);
        assert_clean(&p, 1, 0);
    }
}

#[test]
fn ext_async_restrictions_refuse_before_prepare() {
    for playground in [false, true] {
        let p = ordinary();
        let mut r = runtime();
        if playground { r.limits.profile = Limits::playground().profile; }
        else { r.limits.natives = Some(vec![]); }
        let value = r.heap.int(42);
        let error = r.run_function_with(0, &[value], 1 << 24).unwrap_err();
        assert!(error.0.contains(if playground { "not available in the playground" } else { "native groups are restricted" }));
        assert!(!r.poisoned);
        drop(r);
        assert_clean(&p, 0, 0);
    }
}

static NULL_DROPS: AtomicUsize = AtomicUsize::new(0);
unsafe extern "C" fn prepare_null(_: *mut okk_call, _: *const okk_slot, out: *mut *mut c_void) -> u32 {
    unsafe { *out = ptr::null_mut(); }
    OKK_OK
}
unsafe extern "C" fn execute_null(job: *mut c_void, _: *const okk_cancel) { assert!(job.is_null()); }
unsafe extern "C" fn complete_void(_: *mut okk_call, job: *mut c_void, out: *mut okk_slot) -> u32 {
    assert!(job.is_null());
    unsafe { *out = okk_slot::default(); }
    OKK_OK
}
unsafe extern "C" fn drop_null(job: *mut c_void) {
    assert!(job.is_null());
    NULL_DROPS.fetch_add(1, Ordering::SeqCst);
}
#[test]
fn ext_async_void_result_and_successful_null_job() {
    static CALLBACKS: okk_async = okk_async { prepare: prepare_null, execute: execute_null, complete: complete_void, drop: drop_null };
    static EXPORT: okk_export = okk_export { symbol: c"async_ext@0::nothing/0".as_ptr(), signature: c"async () -> void".as_ptr(),
        params: c"".as_ptr(), sync: None, r#async: &CALLBACKS };
    static DESCRIPTOR: okk_extension = okk_extension { abi: OKKAMA_EXT_ABI, size: std::mem::size_of::<okk_extension>() as u32,
        name: c"async_ext".as_ptr(), exports: &EXPORT, export_count: 1, states: ptr::null(), state_count: 0 };
    let p = okkama_sparrow::assemble(r#"sparrow 19
module "async_ext" "1"
function f0 "nothing" async native "async_ext@0::nothing/0" {
 registers []
 params []
 result void
}
"#).unwrap();
    let limits = Limits { extensions: vec![unsafe { Extension::from_static(&DESCRIPTOR).unwrap() }], ..Default::default() };
    NULL_DROPS.store(0, Ordering::SeqCst);
    let mut r = Box::new(Runtime::with_limits(Loaded::new(p), limits).unwrap());
    assert_eq!(r.run_function_with(0, &[], 1 << 24).unwrap(), crate::word::UNBOUND);
    drop(r);
    assert_eq!(NULL_DROPS.load(Ordering::SeqCst), 1);
}
