//! Callback tests use a call-local fake host, without linking Sparrow.
use super::*;
use std::{ffi::{c_void, CStr}, ptr, sync::atomic::{AtomicUsize, Ordering}};

#[repr(C)]
struct Fake {
    call: okk_call,
    failure: Option<(String, String)>,
    fail_status: u32,
    state_status: u32,
    states: Vec<(*const okk_state_type, *mut c_void)>,
    text: String,
}
impl Fake {
    fn new() -> Self { Self { call: okk_call { host: &HOST }, failure: None, fail_status: OKK_OK,
        state_status: OKK_OK, states: Vec::new(), text: "owned input".into() } }
    fn raw(&mut self) -> *mut okk_call { (self as *mut Self).cast() }
    fn invoke(&mut self, index: usize, args: &[okk_slot]) -> (u32, okk_slot) {
        let mut out = okk_slot::default();
        let status = unsafe { __OKK_EXPORTS[index].sync.unwrap()(self.raw(), args.as_ptr(), &mut out) };
        (status, out)
    }
}
impl Drop for Fake {
    fn drop(&mut self) {
        for (ty, payload) in self.states.drain(..) {
            if !payload.is_null() { unsafe { ((*ty).drop)(payload); } }
        }
    }
}
unsafe fn fake<'a>(c: *mut okk_call) -> &'a mut Fake { unsafe { &mut *c.cast::<Fake>() } }
unsafe extern "C" fn fail(c: *mut okk_call, code: *const u8, cn: usize, msg: *const u8, mn: usize) -> u32 {
    let c = unsafe { fake(c) };
    c.failure = Some((unsafe { std::str::from_utf8(std::slice::from_raw_parts(code, cn)).unwrap().into() },
        unsafe { std::str::from_utf8(std::slice::from_raw_parts(msg, mn)).unwrap().into() }));
    c.fail_status
}
unsafe extern "C" fn state_new(c: *mut okk_call, ty: *const okk_state_type, payload: *mut c_void, _: usize, out: *mut okk_slot) -> u32 {
    let c = unsafe { fake(c) };
    if c.state_status != OKK_OK { return c.state_status; }
    let slot = okk_slot { kind: OKK_STATE, bits: c.states.len() as u64 };
    c.states.push((ty, payload));
    unsafe { *out = slot; }
    OKK_OK
}
unsafe extern "C" fn state_get(c: *mut okk_call, s: okk_slot, ty: *const okk_state_type, out: *mut *mut c_void) -> u32 {
    let c = unsafe { fake(c) };
    let Some((actual, payload)) = c.states.get(s.bits as usize) else { return OKK_BAD_TOKEN; };
    if *actual != ty { return OKK_BAD_SHAPE; }
    if payload.is_null() { c.failure = Some(("already_closed".into(), "closed".into())); return OKK_FAILED; }
    unsafe { *out = *payload; }
    OKK_OK
}
unsafe extern "C" fn state_release(c: *mut okk_call, s: okk_slot, ty: *const okk_state_type) -> u32 {
    let mut payload = ptr::null_mut();
    let status = unsafe { state_get(c, s, ty, &mut payload) };
    if status != OKK_OK { return status; }
    unsafe { fake(c).states[s.bits as usize].1 = ptr::null_mut(); ((*ty).drop)(payload); }
    OKK_OK
}
unsafe extern "C" fn text(c: *mut okk_call, _: okk_slot, out: *mut *const u8, len: *mut usize) -> u32 {
    let c = unsafe { fake(c) };
    unsafe { *out = c.text.as_ptr(); *len = c.text.len(); }
    OKK_OK
}
unsafe extern "C" fn text_new(c: *mut okk_call, data: *const u8, len: usize, out: *mut okk_slot) -> u32 {
    let value = if len == 0 { String::new() } else { unsafe { std::str::from_utf8(std::slice::from_raw_parts(data, len)).unwrap().to_owned() } };
    unsafe { fake(c).text = value; *out = okk_slot { kind: OKK_STRING, bits: 0 }; }
    OKK_OK
}
unsafe extern "C" fn bytes(_: *mut okk_call, _: okk_slot, _: *mut *const u8, _: *mut usize) -> u32 { OKK_UNSUPPORTED }
unsafe extern "C" fn bytes_new(_: *mut okk_call, _: *const u8, _: usize, _: *mut okk_slot) -> u32 { OKK_UNSUPPORTED }
unsafe extern "C" fn bytes_reserve(_: *mut okk_call, _: usize, _: *mut *mut u8, _: *mut okk_slot) -> u32 { OKK_UNSUPPORTED }
unsafe extern "C" fn bigint_read(_: *mut okk_call, _: okk_slot, _: *mut i32, _: *mut *const u8, _: *mut usize) -> u32 { OKK_UNSUPPORTED }
unsafe extern "C" fn bigint_new(_: *mut okk_call, _: i32, _: *const u8, _: usize, _: *mut okk_slot) -> u32 { OKK_UNSUPPORTED }
unsafe extern "C" fn rat_parts(_: *mut okk_call, _: okk_slot, _: *mut okk_slot, _: *mut okk_slot) -> u32 { OKK_UNSUPPORTED }
unsafe extern "C" fn rat_new(_: *mut okk_call, _: okk_slot, _: okk_slot, _: *mut okk_slot) -> u32 { OKK_UNSUPPORTED }
unsafe extern "C" fn array_len(_: *mut okk_call, _: okk_slot, _: *mut usize) -> u32 { OKK_UNSUPPORTED }
unsafe extern "C" fn array_get(_: *mut okk_call, _: okk_slot, _: usize, _: *mut okk_slot) -> u32 { OKK_UNSUPPORTED }
unsafe extern "C" fn array_read(_: *mut okk_call, _: okk_slot, _: usize, _: *mut okk_slot, _: usize) -> u32 { OKK_UNSUPPORTED }
unsafe extern "C" fn array_new(_: *mut okk_call, _: *const okk_slot, _: usize, _: *mut okk_slot) -> u32 { OKK_UNSUPPORTED }
unsafe extern "C" fn map_len(_: *mut okk_call, _: okk_slot, _: *mut usize) -> u32 { OKK_UNSUPPORTED }
unsafe extern "C" fn map_entries(_: *mut okk_call, _: okk_slot, _: *mut okk_slot, _: *mut okk_slot, _: usize) -> u32 { OKK_UNSUPPORTED }
unsafe extern "C" fn map_new(_: *mut okk_call, _: *const okk_slot, _: *const okk_slot, _: usize, _: *mut okk_slot) -> u32 { OKK_UNSUPPORTED }
unsafe extern "C" fn interrupted(_: *mut okk_call, _: *mut u32) -> u32 { OKK_UNSUPPORTED }
static HOST: okk_host = okk_host { text, bytes, text_new, bytes_new, bytes_reserve, bigint_read, bigint_new, rat_parts, rat_new, array_len, array_get, array_read, array_new, map_len, map_entries, map_new, state_new, state_get, state_release, fail, interrupted };

static IMAGE_DROPS: AtomicUsize = AtomicUsize::new(0);
static DROPS: AtomicUsize = AtomicUsize::new(0);
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[state] struct Counter(i64);
impl Drop for Counter { fn drop(&mut self) { DROPS.fetch_add(1, Ordering::SeqCst); } }
#[state(shared)] struct Image(String);
impl Drop for Image { fn drop(&mut self) { IMAGE_DROPS.fetch_add(1, Ordering::SeqCst); } }
#[native] fn sum(a: i64, b: i64) -> i64 { a + b }
#[native] fn refusal() -> Result<i64, Failure> { Err(Failure::new("bad_input", "test refusal")) }
#[native] fn panicking() -> i64 { panic!("user panic"); }
#[native] fn create() -> Counter { Counter(42) }
#[native] fn release(value: Counter) { assert_eq!(value.0, 42); }
#[native] fn alias(a: &mut Counter, b: &Counter) -> i64 { a.0 += b.0; a.0 }
#[native] fn read(a: &Counter) -> i64 { a.0 }
#[native] fn image() -> Image { Image("shared".into()) }
#[native(blocking)] fn blocking(image: Shared<Image>, text: &str) -> String { format!("{} {text}", image.clone().0) }
#[native(blocking)] fn async_panic() -> i64 { panic!("pool panic"); }
#[native] fn types(a: Vec<Vec<i64>>, b: BTreeMap<String, Option<i64>>, c: Option<String>, d: BigInt, e: f64, f: bool, g: u8, h: char, i: &[u8]) -> Result<(), Failure> {
    let _ = (a, b, c, d, e, f, g, h, i); Ok(())
}
#[native(release)] fn close_image(value: Shared<Image>) { drop(value); }
#[native] fn mutate_image(value: &mut Image) { value.0.clear(); }
#[native(blocking)] fn make_counter() -> Counter { Counter(42) }
#[native] fn nonfinite() -> f64 { f64::NAN }
extension!("sdk/test", [sum, refusal, panicking, create, release, alias, read, image, blocking, async_panic, types, nonfinite, close_image, mutate_image, make_counter], states: [Counter, Image]);

#[test]
fn sdk_signatures_and_parameter_names_are_static() {
    let text = |p| unsafe { CStr::from_ptr(p) }.to_str().unwrap();
    assert_eq!(text(__OKK_EXPORTS[0].symbol), "sdk/test@0::sum/2");
    assert_eq!(text(__OKK_EXPORTS[0].params), "a, b");
    assert_eq!(text(__OKK_EXPORTS[8].signature), "async (Image, string) -> string");
    assert_eq!(text(__OKK_EXPORTS[10].signature), "(@@int, %string:?int, ?string, int, float, bool, byte, char, @byte) -> void");
    assert_eq!(text(__OKK_STATES[0].name), "Counter");
}

#[test]
fn sdk_sync_shims_contain_panics_errors_and_conversion_failures() {
    let mut c = Fake::new();
    let int = |bits| okk_slot { bits, kind: OKK_INT };
    assert_eq!(c.invoke(0, &[int(17), int(29)]), (OKK_OK, int(46)));
    assert_eq!(c.invoke(0, &[okk_slot { kind: OKK_BIGINT, bits: 0 }, int(0)]).0, OKK_FAILED);
    assert_eq!(c.failure.as_ref().unwrap().0, "out_of_range");
    assert_eq!(c.invoke(1, &[]).0, OKK_FAILED);
    assert_eq!(c.failure, Some(("bad_input".into(), "test refusal".into())));
    c.fail_status = OKK_STOPPED;
    assert_eq!(c.invoke(1, &[]).0, OKK_STOPPED);
    assert_eq!(c.invoke(2, &[]).0, OKK_PANIC);
    c.fail_status = OKK_OK;
    assert_eq!(c.invoke(11, &[]).0, OKK_FAILED);
    assert_eq!(c.failure.as_ref().unwrap().0, "not_finite");
    assert_eq!(c.invoke(0, &[okk_slot::default(), int(0)]).0, OKK_BAD_KIND);
}

#[test]
fn sdk_states_release_once_and_reject_aliasing_before_user_code() {
    let _lock = LOCK.lock().unwrap();
    DROPS.store(0, Ordering::SeqCst);
    let mut c = Fake::new();
    let (status, value) = c.invoke(3, &[]);
    assert_eq!(status, OKK_OK);
    assert_eq!(c.invoke(5, &[value, value]).0, OKK_FAILED);
    assert_eq!(c.failure.as_ref().unwrap().0, "state_borrowed");
    assert_eq!(c.invoke(6, &[value]).1.bits, 42);
    assert_eq!(c.invoke(4, &[value]).0, OKK_OK);
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);
    assert_eq!(c.invoke(4, &[value]).0, OKK_FAILED);
    c.state_status = OKK_STOPPED;
    assert_eq!(c.invoke(3, &[]).0, OKK_STOPPED);
    assert_eq!(DROPS.load(Ordering::SeqCst), 2);
    drop(c);
    assert_eq!(DROPS.load(Ordering::SeqCst), 2);
    let mut c = Fake::new(); c.invoke(3, &[]); drop(c);
    assert_eq!(DROPS.load(Ordering::SeqCst), 3);
}

#[test]
fn sdk_blocking_jobs_copy_views_and_retain_released_shared_states() {
    let _lock = LOCK.lock().unwrap();
    IMAGE_DROPS.store(0, Ordering::SeqCst);
    let mut c = Fake::new();
    let (_, state) = c.invoke(7, &[]);
    let callbacks = unsafe { &*__OKK_EXPORTS[8].r#async };
    let mut job = ptr::null_mut();
    assert_eq!(unsafe { (callbacks.prepare)(c.raw(), [state, okk_slot::default()].as_ptr(), &mut job) }, OKK_OK);
    assert_eq!(c.invoke(13, &[state]).0, OKK_FAILED);
    c.text.clear();
    assert_eq!(c.invoke(12, &[state]).0, OKK_OK);
    assert_eq!(IMAGE_DROPS.load(Ordering::SeqCst), 0);
    let address = job as usize;
    std::thread::spawn(move || unsafe { (callbacks.execute)(address as *mut c_void, ptr::null()); }).join().unwrap();
    assert_eq!(IMAGE_DROPS.load(Ordering::SeqCst), 1);
    let mut out = okk_slot::default();
    assert_eq!(unsafe { (callbacks.complete)(c.raw(), job, &mut out) }, OKK_OK);
    assert_eq!(c.text, "shared owned input");
    unsafe { (callbacks.drop)(job); }
    let (_, state) = c.invoke(7, &[]);
    assert_eq!(unsafe { (callbacks.prepare)(c.raw(), [state, okk_slot::default()].as_ptr(), &mut job) }, OKK_OK);
    assert_eq!(c.invoke(12, &[state]).0, OKK_OK);
    unsafe { (callbacks.drop)(job); }
    assert_eq!(IMAGE_DROPS.load(Ordering::SeqCst), 2);
    job = ptr::dangling_mut();
    assert_eq!(unsafe { (callbacks.prepare)(c.raw(), [state, okk_slot::default()].as_ptr(), &mut job) }, OKK_FAILED);
    assert!(job.is_null());
    let callbacks = unsafe { &*__OKK_EXPORTS[9].r#async };
    assert_eq!(unsafe { (callbacks.prepare)(c.raw(), ptr::null(), &mut job) }, OKK_OK);
    unsafe { (callbacks.execute)(job, ptr::null()); }
    assert_eq!(unsafe { (callbacks.complete)(c.raw(), job, &mut out) }, OKK_PANIC);
    unsafe { (callbacks.drop)(job); }
    assert_eq!(unsafe { (callbacks.prepare)(c.raw(), ptr::null(), &mut job) }, OKK_OK);
    // Cancellation can drop a prepared job before execute or complete.
    unsafe { (callbacks.drop)(job); }
}

#[test]
fn sdk_cancelled_results_and_panicking_destructors_stay_inside_callbacks() {
    let _lock = LOCK.lock().unwrap();
    DROPS.store(0, Ordering::SeqCst);
    let callbacks = unsafe { &*__OKK_EXPORTS[14].r#async };
    let mut c = Fake::new();
    let mut job = ptr::null_mut();
    assert_eq!(unsafe { (callbacks.prepare)(c.raw(), ptr::null(), &mut job) }, OKK_OK);
    unsafe { (callbacks.execute)(job, ptr::null()); (callbacks.drop)(job); }
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);
    struct Bomb;
    impl Drop for Bomb { fn drop(&mut self) { panic!("destructor panic"); } }
    let payload = Box::new(__private::Payload::Plain(std::cell::RefCell::new(Some(Bomb))));
    unsafe { __private::drop_state::<Bomb>(Box::into_raw(payload).cast()); }
}

#[native(release)] fn close_ref(value: &Counter, mode: i64) -> Result<(), Failure> {
    assert_eq!(value.0, 42);
    match mode { 1 => Err(Failure::new("close_failed", "close failure")), 2 => panic!("close panic"), _ => Ok(()) }
}
#[native(release)] fn close_mut(value: &mut Counter) { value.0 += 1; }
#[native(release)] fn close_shared(value: Shared<Image>, mode: i64) -> Result<(), Failure> {
    assert_eq!(value.0, "shared");
    match mode { 1 => Err(Failure::new("close_failed", "close failure")), 2 => panic!("close panic"), _ => Ok(()) }
}
#[test]
fn sdk_release_runs_after_success_failure_panic_and_invalid_arguments() {
    let _lock = LOCK.lock().unwrap();
    DROPS.store(0, Ordering::SeqCst);
    IMAGE_DROPS.store(0, Ordering::SeqCst);
    let mut c = Fake::new();
    type Close = unsafe extern "C" fn(*mut okk_call, *const okk_slot, *mut okk_slot) -> u32;
    for (mode, expected) in [(0, OKK_OK), (1, OKK_FAILED), (2, OKK_PANIC)] {
        for (create, close) in [(3, __okk_close_ref::sync as Close), (7, __okk_close_shared::sync as Close)] {
            let (_, state) = c.invoke(create, &[]);
            let retained: Option<Shared<Image>> = if create == 7 {
                Some(unsafe { __private::Call::new(c.raw()) }.read(state).unwrap())
            } else { None };
            let before = IMAGE_DROPS.load(Ordering::SeqCst);
            let mut out = okk_slot::default();
            assert_eq!(unsafe { close(c.raw(), [state, okk_slot { kind: OKK_INT, bits: mode }].as_ptr(), &mut out) }, expected);
            assert!(c.states[state.bits as usize].1.is_null());
            if mode == 1 { assert_eq!(c.failure.as_ref().unwrap().0, "close_failed"); }
            assert_eq!(IMAGE_DROPS.load(Ordering::SeqCst), before);
            drop(retained);
            assert_eq!(IMAGE_DROPS.load(Ordering::SeqCst), before + usize::from(create == 7));
        }
    }
    let (_, state) = c.invoke(3, &[]);
    let mut out = okk_slot::default();
    assert_eq!(unsafe { __okk_close_mut::sync(c.raw(), [state].as_ptr(), &mut out) }, OKK_OK);
    let (_, state) = c.invoke(3, &[]);
    assert_eq!(unsafe { __okk_close_ref::sync(c.raw(), [state, okk_slot::default()].as_ptr(), &mut out) }, OKK_BAD_KIND);
    assert!(c.states[state.bits as usize].1.is_null());
    drop(c);
    assert_eq!(DROPS.load(Ordering::SeqCst), 5);
    assert_eq!(IMAGE_DROPS.load(Ordering::SeqCst), 3);
}
