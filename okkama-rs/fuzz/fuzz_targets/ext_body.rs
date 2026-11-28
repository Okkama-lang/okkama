//! Shared by libFuzzer and sparrow-core's deterministic seed replay.
#![allow(unsafe_code)]
#[cfg(test)]
use crate as sparrow_core;
use okkama_runtime_abi::ext::*;
use std::{cell::{Cell, RefCell}, ptr, sync::{Arc, atomic::{AtomicUsize, Ordering}}};

thread_local! {
    static JOBS: RefCell<Arc<JobCounts>> = RefCell::new(Arc::new(JobCounts::default()));
    static PREPARE_STATUS: Cell<u32> = const { Cell::new(0) };
    static COMPLETE_STATUS: Cell<u32> = const { Cell::new(0) };
    static CREATED: Cell<usize> = const { Cell::new(0) };
    static DROPPED: Cell<usize> = const { Cell::new(0) };
    static INPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static STALE: RefCell<okk_slot> = const { RefCell::new(okk_slot { bits: 0, kind: OKK_STRING }) };
}

unsafe extern "C" fn drop_state(_: *mut std::ffi::c_void) {
    DROPPED.with(|d| { d.set(d.get() + 1); CREATED.with(|c| assert!(d.get() <= c.get())); });
}
static STATES: [okk_state_type; 2] = [
    okk_state_type { name: c"State".as_ptr(), drop: drop_state },
    okk_state_type { name: c"Other".as_ptr(), drop: drop_state },
];
unsafe fn new_state(c: *mut okk_call, ty: *const okk_state_type, bytes: usize, out: *mut okk_slot) -> u32 {
    let status = unsafe { ((*(*c).host).state_new)(c, ty, ptr::null_mut(), bytes, out) };
    if status == OKK_OK { CREATED.with(|n| n.set(n.get() + 1)); }
    status
}

unsafe extern "C" fn hostile(c: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    INPUT.with(|input| unsafe {
        let input = input.borrow();
        let h = &*(*c).host;
        let mut bank = [okk_slot { bits: 0, kind: OKK_INT }; 16];
        (h.text_new)(c, b"seed".as_ptr(), 4, &mut bank[1]);
        (h.bytes_new)(c, b"bytes".as_ptr(), 5, &mut bank[2]);
        (h.bigint_new)(c, 1, [255; 16].as_ptr(), 16, &mut bank[3]);
        (h.rat_new)(c, okk_slot { bits: 6, kind: OKK_INT }, okk_slot { bits: 8, kind: OKK_INT }, &mut bank[4]);
        (h.array_new)(c, bank.as_ptr(), 1, &mut bank[5]);
        (h.map_new)(c, bank.as_ptr(), bank.as_ptr(), 1, &mut bank[6]);
        bank[7] = STALE.with(|s| s.replace(bank[1]));
        new_state(c, &STATES[0], 64, &mut bank[8]);
        new_state(c, &STATES[1], 64, &mut bank[9]);
        let mut status = OKK_OK;
        // Bound work, not scalar/token bit patterns. Foreign pointers must still
        // address their advertised buffers; arbitrary addresses cannot be sandboxed.
        for chunk in input.get(1..).unwrap_or_default().chunks(24).take(64) {
            let mut packet = [0u8; 24]; packet[..chunk.len()].copy_from_slice(chunk);
            let bits = u64::from_le_bytes(packet[4..12].try_into().unwrap());
            let kind = u64::from_le_bytes(packet[12..20].try_into().unwrap());
            let arbitrary = okk_slot { bits, kind };
            let slot = if packet[1] & 1 == 0 { arbitrary } else { bank[packet[2] as usize % bank.len()] };
            let other = bank[packet[3] as usize % bank.len()];
            let mut result = okk_slot::default();
            let mut len = 0; let mut sign = 0; let mut view = ptr::null(); let mut buffer = ptr::null_mut();
            let mut keys = [okk_slot::default(); 16]; let mut values = keys;
            let mut scalar = 0;
            let count = packet[20] as usize % 17;
            let bytes = &packet[..count];
            let ty = match packet[21] % 4 {
                0 => &STATES[0] as *const _, 1 => &STATES[1] as *const _,
                2 => ptr::null(), _ => bits as usize as *const okk_state_type,
            };
            let mut payload = ptr::null_mut();
            status = match packet[0] % 27 {
                0 => { result = arbitrary; u32::from_le_bytes(packet[20..24].try_into().unwrap()) },
                1 => (h.text)(c, slot, &mut view, &mut len),
                2 => (h.bytes)(c, slot, &mut view, &mut len),
                3 => (h.text_new)(c, bytes.as_ptr(), bytes.len(), &mut result),
                4 => (h.bytes_new)(c, bytes.as_ptr(), bytes.len(), &mut result),
                5 => {
                    let n = if packet[22] == 255 { usize::MAX } else if packet[22] == 254 { 1 << 24 } else { count };
                    let s = (h.bytes_reserve)(c, n, &mut buffer, &mut result);
                    if s == OKK_OK && n > 0 { ptr::write_bytes(buffer, packet[23], n); }
                    s
                }
                6 => (h.bigint_read)(c, slot, &mut sign, &mut view, &mut len),
                7 => (h.bigint_new)(c, packet[22] as i8 as i32, bytes.as_ptr(), bytes.len(), &mut result),
                8 => (h.rat_parts)(c, slot, &mut result, &mut values[0]),
                9 => (h.rat_new)(c, slot, other, &mut result),
                10 => (h.array_len)(c, slot, &mut len),
                11 => (h.array_get)(c, slot, bits as usize, &mut result),
                12 => (h.array_read)(c, slot, bits as usize, keys.as_mut_ptr(), count),
                13 => { keys[0] = slot; keys[1] = other; (h.array_new)(c, keys.as_ptr(), count, &mut result) },
                14 => (h.map_len)(c, slot, &mut len),
                15 => (h.map_entries)(c, slot, keys.as_mut_ptr(), values.as_mut_ptr(), count),
                16 => { keys[0] = slot; keys[1] = other; (h.map_new)(c, keys.as_ptr(), keys.as_ptr(), count, &mut result) },
                17 => (h.fail)(c, bytes.as_ptr(), bytes.len(), bytes.as_ptr(), bytes.len()),
                18 => (h.interrupted)(c, &mut scalar),
                19 => (h.text_new)(c, ptr::null(), bits as usize, &mut result),
                20 => (h.array_new)(c, ptr::null(), bits as usize, &mut result),
                21 => (h.array_len)(c, slot, ptr::null_mut()),
                22 => { result = slot; OKK_OK },
                24 => new_state(c, ty, if packet[22] == 255 { usize::MAX } else { count }, &mut result),
                25 => (h.state_get)(c, slot, ty, &mut payload),
                26 => (h.state_release)(c, slot, ty),
                _ => (h.bigint_read)(c, slot, ptr::null_mut(), &mut view, &mut len),
            };
            bank[8 + packet[23] as usize % 8] = result;
            *out = result;
        }
        match input.first().copied().unwrap_or(0) % 4 {
            0 => status, 1 => OKK_OK, 2 => OKK_FAILED, _ => input.get(1).copied().unwrap_or(255) as u32,
        }
    })
}

static EXPORT: okk_export = okk_export { symbol: c"fuzz/ext@0::hostile/0".as_ptr(), signature: c"() -> int".as_ptr(),
    params: c"".as_ptr(), sync: Some(hostile), r#async: ptr::null() };
static EXTENSION: okk_extension = okk_extension { abi: OKKAMA_EXT_ABI, size: std::mem::size_of::<okk_extension>() as u32,
    name: c"hostile".as_ptr(), exports: &EXPORT, export_count: 1, states: STATES.as_ptr(), state_count: STATES.len() as u32 };

#[derive(Default)]
struct JobCounts { created: AtomicUsize, dropped: AtomicUsize }
struct Job { counts: Arc<JobCounts>, returned: bool }
unsafe extern "C" fn drop_job(pointer: *mut std::ffi::c_void) {
    let job = unsafe { Box::from_raw(pointer.cast::<Job>()) };
    let dropped = job.counts.dropped.fetch_add(1, Ordering::SeqCst) + 1;
    assert!(dropped <= job.counts.created.load(Ordering::SeqCst));
}
unsafe extern "C" fn prepare(c: *mut okk_call, _: *const okk_slot, out: *mut *mut std::ffi::c_void) -> u32 {
    let counts = JOBS.with(|j| j.borrow().clone());
    counts.created.fetch_add(1, Ordering::SeqCst);
    let job = Box::into_raw(Box::new(Job { counts, returned: false })).cast();
    let status = PREPARE_STATUS.with(Cell::get);
    if status != OKK_OK {
        unsafe { drop_job(job); }
        if status == OKK_FAILED { unsafe { ((*(*c).host).fail)(c, b"prepare".as_ptr(), 7, ptr::null(), 0); } }
    } else { unsafe { *out = job; } }
    status
}
unsafe extern "C" fn execute(pointer: *mut std::ffi::c_void, cancel: *const okk_cancel) {
    unsafe {
        assert!(((*cancel).cancelled)(cancel) <= 1);
        (*pointer.cast::<Job>()).returned = true;
    }
}
unsafe extern "C" fn complete(c: *mut okk_call, pointer: *mut std::ffi::c_void, out: *mut okk_slot) -> u32 {
    unsafe {
        assert!((*pointer.cast::<Job>()).returned);
        hostile(c, ptr::null(), out);
        let status = COMPLETE_STATUS.with(Cell::get);
        if status == OKK_FAILED { ((*(*c).host).fail)(c, b"complete".as_ptr(), 8, ptr::null(), 0); }
        status
    }
}
static ASYNC: okk_async = okk_async { prepare, execute, complete, drop: drop_job };
static ASYNC_EXPORT: okk_export = okk_export { symbol: c"fuzz/ext@0::async_hostile/0".as_ptr(), signature: c"async () -> int".as_ptr(),
    params: c"".as_ptr(), sync: None, r#async: &ASYNC };
static ASYNC_EXTENSION: okk_extension = okk_extension { exports: &ASYNC_EXPORT, ..EXTENSION };

pub fn exercise(data: &[u8]) {
    CREATED.with(|n| n.set(0));
    DROPPED.with(|n| n.set(0));
    let program = okkama_sparrow::assemble(r#"sparrow 19
module "ext_fuzz" "1.0.0"
handle "fuzz.ext::State"
handle "fuzz.ext::Other"
type t0 int
type t1 str
type t2 object obj0
object obj0 "NativeFailure" builtin {
 field "domain" t1
 field "code" t1
 field "operation" t1
 field "message" t1
}
function f0 "hostile" native "fuzz/ext@0::hostile/0" {
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
entry f1
"#).unwrap();
    INPUT.with(|input| { let mut input = input.borrow_mut(); input.clear(); input.extend_from_slice(&data[..data.len().min(1537)]); });
    let limits = || sparrow_core::budget::Limits {
        memory: Some(2 << 20), memory_scope: sparrow_core::budget::MemoryScope::Runtime,
        steps: Some(1000), extensions: vec![unsafe { sparrow_core::ext::Extension::from_static(&EXTENSION).unwrap() }],
        ..Default::default()
    };
    let _ = sparrow_core::run_limited(okkama_sparrow::encode(&program).unwrap(), vec![], false, vec![], limits());
    let counts = Arc::new(JobCounts::default());
    JOBS.with(|j| *j.borrow_mut() = counts.clone());
    let mut program = program;
    program.functions.truncate(1);
    program.functions[0].is_async = true;
    program.functions[0].native = Some("fuzz/ext@0::async_hostile/0".into());
    program.entry = Some(okkama_sparrow::FuncRef(0));
    let encoded = okkama_sparrow::encode(&program).unwrap();
    let arbitrary_status = |offset: usize| {
        let mut bytes = [0; 4];
        for (out, input) in bytes.iter_mut().zip(data.get(offset..).unwrap_or_default()) { *out = *input; }
        u32::from_le_bytes(bytes)
    };
    for preparing in [true, false] {
        PREPARE_STATUS.with(|s| s.set(if preparing { arbitrary_status(0) } else { OKK_OK }));
        COMPLETE_STATUS.with(|s| s.set(arbitrary_status(4)));
        let mut limits = limits();
        limits.extensions = vec![unsafe { sparrow_core::ext::Extension::from_static(&ASYNC_EXTENSION).unwrap() }];
        let _ = sparrow_core::run_limited(encoded.clone(), vec![], false, vec![], limits);
        assert_eq!(counts.created.load(Ordering::SeqCst), counts.dropped.load(Ordering::SeqCst), "every allocated job is dropped exactly once");
    }
    CREATED.with(|c| DROPPED.with(|d| assert_eq!(c.get(), d.get(), "teardown drops every state")));
}
