//! Small, observable calls for the four extension micro cells.
use okkama_ext::{extension, native, state};

#[native]
fn scalar(value: i64) -> i64 { value + 1 }
#[native]
fn bytes(value: &[u8]) -> Vec<u8> { value.to_vec() }
#[state]
struct Counter(i64);
#[native]
fn create(value: i64) -> Counter { Counter(value) }
#[native]
fn read(counter: &Counter) -> i64 { counter.0 }
#[native(release)]
fn release(counter: &Counter) { let _ = counter; }
#[native(blocking)]
fn asynchronous(value: i64) -> i64 { value + 1 }

extension!("micro_ext", [scalar, bytes, create, read, release, asynchronous], states: [Counter]);
