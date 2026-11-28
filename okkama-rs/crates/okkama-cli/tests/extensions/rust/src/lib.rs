//! A Rust extension with no Sparrow linkage or process-global runtime state.
use okkama_ext::{native, state, extension, Failure, Shared, BigInt};
use std::{collections::BTreeMap, sync::atomic::{AtomicI64, Ordering}};

#[native] fn add(a: i64, b: i64) -> i64 { a + b }
#[native] fn negate(value: bool) -> bool { !value }
#[native] fn floating(value: f64) -> f64 { value * 2.0 }
#[native] fn byte_copy(value: u8) -> u8 { value }
#[native] fn character(value: char) -> char { value }
#[native] fn text_copy(value: &str) -> &str { value }
#[native] fn bytes_copy(value: &[u8]) -> Vec<u8> { value.to_vec() }
#[native] fn integers(value: Vec<i64>) -> Vec<i64> { value }
#[native] fn bytes_owned(value: Vec<u8>) -> Vec<u8> { value }
#[native] fn dictionary(value: BTreeMap<String, i64>) -> BTreeMap<String, i64> { value }
#[native] fn nested(value: Vec<BTreeMap<String, Option<i64>>>) -> Vec<BTreeMap<String, Option<i64>>> { value }
#[native] fn maybe(value: Option<String>) -> Option<String> { value }
#[native] fn big(value: BigInt) -> BigInt { value + 1 }
#[native] fn failure() -> Result<i64, Failure> { Err(Failure::new("sample_failure", "Rust failure")) }
#[native] fn panic_sync() -> i64 { panic!("Rust panic") }
#[native] fn not_finite() -> f64 { f64::INFINITY }

static DROPPED: AtomicI64 = AtomicI64::new(0);
#[state] struct Counter(i64);
impl Drop for Counter { fn drop(&mut self) { DROPPED.fetch_add(1, Ordering::SeqCst); } }
#[native] fn create(value: i64) -> Counter { Counter(value) }
#[native] fn read(counter: &Counter) -> i64 { counter.0 }
#[native] fn increment(counter: &mut Counter) -> i64 { counter.0 += 1; counter.0 }
#[native] fn release(counter: Counter) { drop(counter); }
#[native] fn drops() -> i64 { DROPPED.load(Ordering::SeqCst) }
#[native] fn alias(a: &mut Counter, b: &Counter) -> i64 { a.0 += b.0; a.0 }

static IMAGE_DROPPED: AtomicI64 = AtomicI64::new(0);
static JOB_GATE: (std::sync::Mutex<bool>, std::sync::Condvar) = (std::sync::Mutex::new(false), std::sync::Condvar::new());
#[state(shared)] struct Image(String);
impl Drop for Image { fn drop(&mut self) { IMAGE_DROPPED.fetch_add(1, Ordering::SeqCst); } }
#[native(release)] fn close(image: Shared<Image>) { drop(image); }
#[native] fn image_drops() -> i64 { IMAGE_DROPPED.load(Ordering::SeqCst) }
#[native] fn finish_job() {
    *JOB_GATE.0.lock().unwrap() = true;
    JOB_GATE.1.notify_all();
}
#[native(blocking)] fn held_image(image: Shared<Image>) -> String {
    let (ready, _) = JOB_GATE.1.wait_timeout_while(JOB_GATE.0.lock().unwrap(),
        std::time::Duration::from_secs(5), |ready| !*ready).unwrap();
    assert!(*ready, "host must release the state before finishing the job");
    image.0.clone()
}
#[native] fn image(value: &str) -> Image { Image(value.into()) }
#[native(blocking)] fn borrowed(value: &str) -> &str { value }
#[native(blocking)] fn uppercase(value: &str) -> String { value.to_uppercase() }
#[native(blocking)] fn shared(image: Shared<Image>, suffix: &[u8]) -> Result<String, Failure> {
    let copy = image.clone();
    Ok(format!("{}:{}", copy.0.to_uppercase(), suffix.len()))
}
#[native(blocking)] fn panic_async() -> i64 { panic!("blocking panic") }
#[native(blocking)] fn fail_async() -> Result<String, Failure> { Err(Failure::new("async_failure", "blocking failure")) }

extension!("rust_sample", [add, negate, floating, byte_copy, character, text_copy, bytes_copy, integers, bytes_owned,
    dictionary, nested, maybe, big, failure, panic_sync, not_finite, create, read, increment, release, drops, alias,
    image, close, image_drops, finish_job, held_image, borrowed, uppercase, shared, panic_async, fail_async], states: [Counter, Image]);
