//! #1124: the scheduler's wait for a timer in this build's body (`io.rs`): Tokio with the host, a sleep on a
//! condition variable natively without it (`cargo test -p sparrow-core --no-default-features --features jit`).
//! An async `main` sleeping through `runtime/time@1::sleep_nanoseconds` is resumed when its timer is due, not
//! before; an interrupted run stops while it waits.
use sparrow_core::budget::Limits;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// `main` sleeps `NANOS` nanoseconds, then returns 7.
fn sleeping(nanos: u64) -> Vec<u8> {
    let source = format!(
        r#"sparrow 18
module "timer-wait" "1.0.0"
type t0 int
type t1 str
type t2 bool
type t3 future t2

function f0 "main" async {{
    registers [t1, t3, t2, t0]
    params []
    result t0
    0: const r0, str "{nanos}"
    1: call r1, f1, [r0]
    2: await.take r2, r1
    3: const r3, int 7
    4: ret r3
}}
function f1 "runtime.time::sleep_nanoseconds" async native "runtime/time@1::sleep_nanoseconds/1" {{
    registers [t1]
    params [r0]
    result t2
}}
entry f0
"#
    );
    okkama_sparrow::encode(&okkama_sparrow::assemble(&source).unwrap()).unwrap()
}
#[test]
fn a_task_sleeping_on_a_timer_resumes_when_it_is_due() {
    let started = Instant::now();
    assert_eq!(sparrow_core::run(sleeping(30_000_000)).unwrap(), 7);
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(30), "resumed before its timer: {elapsed:?}");
    assert!(elapsed < Duration::from_secs(10), "the wait outlived its timer: {elapsed:?}");
}
#[test]
fn an_interrupt_stops_a_run_waiting_for_a_timer() {
    let flag = Arc::new(AtomicBool::new(false));
    let setter = flag.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        setter.store(true, Ordering::Relaxed);
    });
    let started = Instant::now();
    let limits = Limits { interrupt: Some(flag), ..Limits::default() };
    let error = sparrow_core::run_limited(sleeping(60_000_000_000), Vec::new(), false, Vec::new(), limits).unwrap_err();
    assert_eq!(error.0, "Execution interrupted");
    assert!(started.elapsed() < Duration::from_secs(10), "the wait did not look at the interrupt flag");
}
