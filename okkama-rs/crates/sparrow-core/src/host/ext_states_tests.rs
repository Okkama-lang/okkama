//! State lifetime through the ordinary watched handle and wrapper paths.
#![allow(unsafe_code)]
use super::*;
use crate::{budget::Limits, ext::Extension, program::Loaded, runtime::Runtime};
use okkama_runtime_abi::ext::*;
use std::{ffi::c_void, ptr, sync::{Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}}};

static LOCK: Mutex<()> = Mutex::new(());
static DROPS: AtomicUsize = AtomicUsize::new(0);
static WRONG_THREAD: AtomicBool = AtomicBool::new(false);
unsafe extern "C" fn drop_state(payload: *mut c_void) {
    if !payload.is_null() {
        let creator = unsafe { Box::from_raw(payload.cast::<std::thread::ThreadId>()) };
        if *creator != std::thread::current().id() { WRONG_THREAD.store(true, Ordering::SeqCst); }
    }
    DROPS.fetch_add(1, Ordering::SeqCst);
}
unsafe fn new_state(c: *mut okk_call, ty: *const okk_state_type, bytes: usize, out: *mut okk_slot) -> u32 {
    let payload = Box::into_raw(Box::new(std::thread::current().id())).cast();
    let status = unsafe { ((*(*c).host).state_new)(c, ty, payload, bytes, out) };
    if status != OKK_OK { unsafe { drop(Box::from_raw(payload.cast::<std::thread::ThreadId>())); } }
    status
}
static TYPES: [okk_state_type; 2] = [
    okk_state_type { name: c"State".as_ptr(), drop: drop_state },
    okk_state_type { name: c"Other".as_ptr(), drop: drop_state },
];
unsafe extern "C" fn create(c: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe { new_state(c, &TYPES[0], 4096, out) }
}
macro_rules! export {
    ($symbol:literal, $signature:literal, $callback:ident) => {
        okk_export { symbol: $symbol.as_ptr(), signature: $signature.as_ptr(), params: c"".as_ptr(),
            sync: Some($callback), r#async: ptr::null() }
    };
}
unsafe extern "C" fn read(c: *mut okk_call, args: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe {
        let mut payload = ptr::null_mut();
        let status = ((*(*c).host).state_get)(c, *args, &TYPES[0], &mut payload);
        if status != OKK_OK { return status; }
        if payload.is_null() || *payload.cast::<std::thread::ThreadId>() != std::thread::current().id() { return OKK_PANIC; }
        *out = okk_slot { bits: 42, kind: OKK_INT };
        OKK_OK
    }
}
unsafe extern "C" fn release(c: *mut okk_call, args: *const okk_slot, _: *mut okk_slot) -> u32 {
    unsafe { ((*(*c).host).state_release)(c, *args, &TYPES[0]) }
}
unsafe extern "C" fn count(_: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe { *out = okk_slot { bits: DROPS.load(Ordering::SeqCst) as u64, kind: OKK_INT }; }
    OKK_OK
}
unsafe extern "C" fn probe(c: *mut okk_call, args: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe {
        let mode = (*args).bits;
        let h = &*(*c).host;
        let ty = if mode == 3 { &TYPES[1] } else { &TYPES[0] };
        let status = new_state(c, ty, if mode == 5 { usize::MAX } else { 4096 }, out);
        if status != OKK_OK { return status; }
        let mut payload = ptr::null_mut();
        match mode {
            0 => { (h.fail)(c, b"after_create".as_ptr(), 12, b"failed".as_ptr(), 6); OKK_FAILED }
            1 => { (h.state_get)(c, *out, &TYPES[1], &mut payload); OKK_OK }
            2 => { (h.state_release)(c, *out, &TYPES[1]); OKK_OK }
            3 => OKK_OK,
            4 => { (h.state_get)(c, *out, &FOREIGN, &mut payload); OKK_OK }
            _ => OKK_OK,
        }
    }
}
unsafe extern "C" fn foreign(c: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe { new_state(c, &FOREIGN, 64, out) }
}
static FOREIGN_EXPORT: okk_export = export!(c"states@0::foreign/0", c"() -> State", foreign);
static FOREIGN_DESCRIPTOR: okk_extension = okk_extension {
    exports: &FOREIGN_EXPORT, export_count: 1, states: &FOREIGN, state_count: 1, ..DESCRIPTOR
};
static FOREIGN: okk_state_type = okk_state_type { name: c"State".as_ptr(), drop: drop_state };
static EXPORTS: &[okk_export] = &[okk_export {
    symbol: c"states@0::create/0".as_ptr(), signature: c"() -> State".as_ptr(), params: c"".as_ptr(),
    sync: Some(create), r#async: ptr::null(),
},
    export!(c"states@0::read/1", c"(State) -> int", read),
    export!(c"states@0::release/1", c"(State) -> void", release),
    export!(c"states@0::count/0", c"() -> int", count),
    export!(c"states@0::probe/1", c"(int) -> State", probe),
];
static DESCRIPTOR: okk_extension = okk_extension {
    abi: OKKAMA_EXT_ABI, size: std::mem::size_of::<okk_extension>() as u32,
    name: c"states".as_ptr(), exports: EXPORTS.as_ptr(), export_count: EXPORTS.len() as u32,
    states: TYPES.as_ptr(), state_count: TYPES.len() as u32,
};
fn runtime() -> Box<Runtime> {
    let p = okkama_sparrow::assemble(r#"sparrow 19
module "states" "1"
handle "states::State"
type t0 handle 0
function f0 "create" native "states@0::create/0" {
 registers []
 params []
 result t0
}
function f1 "main" {
 registers [t0]
 params []
 result t0
 0: call r0, f0, []
 1: ret r0
}
"#).unwrap();
    let limits = Limits { extensions: vec![unsafe { Extension::from_static(&DESCRIPTOR).unwrap() }, unsafe { Extension::from_static(&FOREIGN_DESCRIPTOR).unwrap() }], ..Default::default() };
    Box::new(Runtime::with_limits(Loaded::new(p), limits).unwrap())
}
fn collect(r: &mut Runtime, roots: &[Word]) {
    r.heap.collect(|visit| { for &word in roots { visit(word, Repr::Ref); } });
    let dead = r.heap.take_released();
    r.host.as_mut().unwrap().released(&dead);
}
#[test]
fn ext_states_characterization_watched_arena_cell() {
    let p = okkama_sparrow::Program::default();
    let mut r = Box::new(Runtime::new(Loaded::new(p)).unwrap());
    let (id, cell) = super::with_host(&mut r, false, |h| {
        let id = h.handles.allocate(Handle::TextBuffer("owned".into())).unwrap();
        (id, h.handle_object(id))
    });
    collect(&mut r, &[cell]);
    assert!(r.host.as_ref().unwrap().handles.get(id).is_ok());
    collect(&mut r, &[]);
    assert!(r.host.as_ref().unwrap().handles.get(id).is_err());
}
#[test]
fn ext_states_live_state_drops_at_teardown() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    DROPS.store(0, Ordering::SeqCst);
    let mut r = runtime();
    let cell = r.run_function_with(1, &[], 1 << 24).unwrap();
    collect(&mut r, &[cell]);
    assert_eq!(DROPS.load(Ordering::SeqCst), 0);
    drop(r);
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);
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
fn source_runtime(body: &str) -> Box<Runtime> {
    use okkama_compiler::{SourceFile, syntax, lowering, codegen};
    let imports = if body.contains("Event") { "import \"async/event\" take Event;" } else { "" };
    let text = format!(r#"module states;
        {imports}
        native object State;
        native object Other;
        native create() is State;
        native foreign() is State;
        native read(s is State) is int;
        native release(s is State) is void;
        native count() is int;
        native probe(mode is int) is State;
        object Wrapper {{
            has state is State;
            Wrapper(:state);
            read() is int {{ read(state); }}
            destroy {{ release(state); }}
        }}
        {body}
    "#);
    let source = SourceFile::new("ext-states.ok", text.clone()).unwrap();
    let syntax = syntax::parse(&source, &syntax::ParseOptions::default()).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
    let ir = lowering::lower_linked(&syntax, source.name(), &mut Stdlib).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
    let p = codegen::sparrow::generate(&ir).unwrap();
    p.verify().unwrap();
    let limits = Limits { extensions: vec![unsafe { Extension::from_static(&DESCRIPTOR).unwrap() }, unsafe { Extension::from_static(&FOREIGN_DESCRIPTOR).unwrap() }], ..Default::default() };
    Box::new(Runtime::with_limits(Loaded::new(p), limits).unwrap())
}

#[test]
fn ext_states_wrapper_scope_failure_and_move() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for body in [
        "main() is int { { let w = Wrapper(state: create()); confirm w.read() == 42; } count(); }",
        "enum Stop is Error = (now); main() is int { try { let w = Wrapper(state: create()); confirm w.read() == 42; throw Stop.now; } catch Stop.now {} count(); }",
        "object Owner { has w is Wrapper; Owner(:w); read() is int { w.read(); } } main() is int { { let owner = if true { let w = Wrapper(state: create()); Owner(w: w); } else { Owner(w: Wrapper(state: create())); }; confirm count() == 0; confirm owner.read() == 42; } count(); }",
    ] {
        DROPS.store(0, Ordering::SeqCst);
        let mut r = source_runtime(body);
        assert_eq!(r.run_entry(1 << 24).unwrap(), 1, "{body}");
        drop(r);
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
        assert!(!WRONG_THREAD.load(Ordering::SeqCst));
    }
}

#[test]
fn ext_states_wrapper_task_cancellation() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    DROPS.store(0, Ordering::SeqCst);
    let mut r = source_runtime(r#"
        object Work {
            Work();
            async hold(started is Event, gate is Event) is void {
                let w = Wrapper(state: create());
                confirm w.read() == 42;
                started.set();
                await gate.wait();
            }
        }
        async main() is int {
            let started = Event();
            let gate = Event();
            let task = spawn Work().hold(started, gate);
            await started.wait();
            confirm count() == 0;
            task.cancel();
            try { await task; } catch e is Error {}
            count();
        }
    "#);
    assert_eq!(r.run_entry(1 << 24).unwrap(), 1);
    drop(r);
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);
}

#[test]
fn ext_states_double_close_and_read_are_native_failures() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for operation in ["release(s)", "read(s)"] {
        DROPS.store(0, Ordering::SeqCst);
        let mut r = source_runtime(&format!(r#"main() is int {{
            let s = create(); release(s);
            let closed = try {{ {operation}; false; }} catch e is NativeFailure {{
                confirm e.code() == "already_closed";
                confirm e.domain() == "states";
                true;
            }};
            confirm closed;
            count();
        }}"#));
        assert_eq!(r.run_entry(1 << 24).unwrap(), 1);
        assert!(!r.poisoned);
        drop(r);
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
        assert!(!WRONG_THREAD.load(Ordering::SeqCst));
    }
}

#[test]
fn ext_states_orphans_and_failed_creation_calls_are_swept() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for body in ["main() is int { create(); 0; }", "main() is int { try { probe(0); } catch e is Error {} 0; }"] {
        DROPS.store(0, Ordering::SeqCst);
        let mut r = source_runtime(body);
        r.run_entry(1 << 24).unwrap();
        collect(&mut r, &[]);
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
        drop(r);
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
        assert!(!WRONG_THREAD.load(Ordering::SeqCst));
    }
}

#[test]
fn ext_states_wrong_types_are_sticky_contract_faults() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for mode in 1..=5 {
        DROPS.store(0, Ordering::SeqCst);
        let mut r = source_runtime(&format!("main() is int {{ probe({mode}); 0; }}"));
        assert!(r.run_entry(1 << 24).unwrap_err().0.contains("internal error in native"));
        assert!(r.poisoned);
        drop(r);
        assert_eq!(DROPS.load(Ordering::SeqCst), usize::from(mode != 5));
        assert!(!WRONG_THREAD.load(Ordering::SeqCst));
    }
}

#[test]
fn ext_states_foreign_extension_with_same_handle_name_faults_on_argument() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    DROPS.store(0, Ordering::SeqCst);
    let mut r = source_runtime("main() is int { let s = foreign(); read(s); }");
    assert!(r.run_entry(1 << 24).unwrap_err().0.contains("internal error in native"));
    assert!(r.poisoned);
    drop(r);
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);
}

#[test]
fn ext_states_creation_charges_gc_pressure_and_budget_refusal_keeps_payload() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    DROPS.store(0, Ordering::SeqCst);
    let mut r = runtime();
    let before = r.heap.stats().external_allocated_bytes;
    crate::natives::call(&mut r, 0, &[]).unwrap();
    assert_eq!(r.heap.stats().external_allocated_bytes - before, 4096);
    r.limits.memory_scope = crate::budget::MemoryScope::Runtime;
    r.limits.memory = Some(r.heap.held_bytes() as u64 + 128);
    assert!(matches!(crate::natives::call(&mut r, 0, &[]), Err(crate::natives::Refusal::Exhausted(crate::budget::Exhausted::Memory))));
    assert!(!r.poisoned);
    drop(r);
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);
    assert!(!WRONG_THREAD.load(Ordering::SeqCst));
}

#[test]
fn ext_states_descriptor_names_are_unique_short_names() {
    static DUPLICATE: [okk_state_type; 2] = [
        okk_state_type { name: c"State".as_ptr(), drop: drop_state },
        okk_state_type { name: c"State".as_ptr(), drop: drop_state },
    ];
    static BAD: [okk_state_type; 1] = [okk_state_type { name: c"foreign::State".as_ptr(), drop: drop_state }];
    static DUPLICATE_DESCRIPTOR: okk_extension = okk_extension { states: DUPLICATE.as_ptr(), state_count: 2, ..DESCRIPTOR };
    static BAD_DESCRIPTOR: okk_extension = okk_extension { states: BAD.as_ptr(), state_count: 1, ..DESCRIPTOR };
    assert!(unsafe { Extension::from_static(&DUPLICATE_DESCRIPTOR) }.unwrap_err().0.contains("duplicate extension state"));
    assert!(unsafe { Extension::from_static(&BAD_DESCRIPTOR) }.unwrap_err().0.contains("invalid extension state name"));
}
