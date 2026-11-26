//! Runtime tests of async tasks: scheduling, wakes and cancellation.

/// Run the program's `tests` block; true when every test passed.
fn passes(source: &str) -> bool {
    let program = okkama_sparrow::assemble(source).unwrap();
    program.verify().unwrap();
    let bytes = okkama_sparrow::encode(&program).unwrap();
    crate::run_tests(bytes, Box::new(std::io::sink())).unwrap()
}

/// #663: W awaits child C; C settles and queues main, then W; main cancels W
/// before W runs. W keeps C's outcome (stores it, sets `done`), and the
/// cancellation is delivered at its next await (`never.wait()`).
const CANCEL_AFTER_SETTLE: &str = r#"sparrow 18
module "cancel-after-settle" "1.0.0"
type t0 int
type t1 bool
type t2 object obj0
type t3 future t0
type t4 object obj1
type t5 future t2

object obj0 "Event" {}
object obj1 "Atomic<int>" {}
object obj2 "Closed" {}

function f0 "Event.new" intrinsic "okkama.event.Event.new_ready" [obj0] [] {
    registers [t1]
    params [r0]
    result t2
}
function f1 "Event.set" intrinsic "okkama.event.Event.set" [] [] {
    registers [t2]
    params [r0]
    result void
}
function f2 "Event.wait" async intrinsic "okkama.event.Event.wait" [] [] {
    registers [t2]
    params [r0]
    result t2
}
function f3 "Atomic.new" intrinsic "okkama.atomic.Atomic.new_value" [obj1] [] {
    registers [t0]
    params [r0]
    result t4
}
function f4 "Atomic.value" intrinsic "okkama.atomic.Atomic.value" [obj2] [] {
    registers [t4]
    params [r0]
    result t0
}
function f5 "Atomic.replace" intrinsic "okkama.atomic.Atomic.replace" [obj2] [] {
    registers [t4, t0]
    params [r0, r1]
    result void
}
function f6 "test.is_int" intrinsic "okkama.test.is" [] [] {
    registers [t0, t0]
    params [r0, r1]
    result void
}
function f7 "test.is_bool" intrinsic "okkama.test.is" [] [] {
    registers [t1, t1]
    params [r0, r1]
    result void
}
function f8 "child" async {
    registers [t2, t5, t2, t0]
    params [r0]
    result t0
    0: call r1, f2, [r0]
    1: await r2, r1
    2: const r3, int 42
    3: ret r3
}
function f9 "worker" async {
    registers [t2, t2, t2, t3, t4, t2, t5, t2, t0]
    params [r0, r1, r2, r3, r4, r5]
    result t0
    0: call r6, f2, [r0]
    1: await r7, r6
    2: call _, f1, [r1]
    3: await r8, r3
    4: call _, f5, [r4, r8]
    5: call _, f1, [r5]
    6: call r6, f2, [r2]
    7: await r7, r6
    8: ret r8
}
function f10 "cancel after await settled" async {
    registers [t1, t2, t2, t2, t2, t0, t4, t3, t3, t0, t5, t2, t1, t0]
    params []
    result void
    0: const r0, bool false
    1: call r1, f0, [r0]
    2: call r2, f0, [r0]
    3: call r3, f0, [r0]
    4: call r4, f0, [r0]
    5: const r5, int 0
    6: call r6, f3, [r5]
    7: call r7, f8, [r2]
    8: fut.spawn r7, r7
    9: call r8, f9, [r1, r2, r3, r7, r6, r4]
    10: fut.spawn r8, r8
    11: call _, f1, [r1]
    12: await r9, r7
    13: const r5, int 42
    14: call _, f6, [r9, r5]
    15: fut cancel, _, r8
    16: call r10, f2, [r4]
    17: await r11, r10
    18: call r13, f4, [r6]
    19: call _, f6, [r13, r5]
    20: fut cancelled, r12, r8
    21: const r0, bool true
    22: call _, f7, [r12, r0]
    23: ret _
}
tests { test "a task cancelled after its await settled keeps the outcome" f10 }
"#;

#[test]
fn cancel_after_settle_keeps_outcome_and_waits_for_next_await() {
    assert!(passes(CANCEL_AFTER_SETTLE));
}

/// The other order: `gate.set()` wakes the worker's wait driver, and main
/// cancels the worker before that driver runs. The awaited Future settles
/// after the request, so the re-entered await raises Cancelled: the worker
/// never stores 1. The helper, woken by the same gate, then sets `done`.
fn cancel_before_settle() -> String {
    let functions = CANCEL_AFTER_SETTLE.split("function f8 ").next().unwrap();
    let rest = r#"function f8 "worker" async {
    registers [t2, t4, t5, t2, t0]
    params [r0, r1]
    result void
    0: call r2, f2, [r0]
    1: await r3, r2
    2: const r4, int 1
    3: call _, f5, [r1, r4]
    4: ret _
}
function f9 "helper" async {
    registers [t2, t2, t5, t2]
    params [r0, r1]
    result void
    0: call r2, f2, [r0]
    1: await r3, r2
    2: call _, f1, [r1]
    3: ret _
}
function f10 "cancel before await settled" async {
    registers [t1, t2, t2, t0, t4, t6, t6, t5, t2, t0, t1]
    params []
    result void
    0: const r0, bool false
    1: call r1, f0, [r0]
    2: call r2, f0, [r0]
    3: const r3, int 0
    4: call r4, f3, [r3]
    5: call r5, f8, [r1, r4]
    6: fut.spawn r5, r5
    7: call r6, f9, [r1, r2]
    8: fut.spawn r6, r6
    9: call _, f1, [r1]
    10: fut cancel, _, r5
    11: call r7, f2, [r2]
    12: await r8, r7
    13: call r9, f4, [r4]
    14: call _, f6, [r9, r3]
    15: fut cancelled, r10, r5
    16: const r0, bool true
    17: call _, f7, [r10, r0]
    18: ret _
}
tests { test "a task cancelled before its await settled raises there" f10 }
"#;
    let functions = functions.replace("type t5 future t2\n", "type t5 future t2\ntype t6 future void\n");
    format!("{functions}{rest}")
}

#[test]
fn cancel_before_settle_raises_at_the_reentered_await() {
    assert!(passes(&cancel_before_settle()));
}

/// #670: CANCEL_AFTER_SETTLE with the worker's body moved into `inner`, an
/// async call the worker awaits: a nested frame of the worker's task, not a
/// task of its own. The child's settlement queues main, then `inner`; main
/// cancels the worker while `inner` is queued. `inner` keeps the child's
/// outcome, and the cancellation is delivered at its next await.
fn nested_cancel_after_settle() -> String {
    let (head, rest) = CANCEL_AFTER_SETTLE.split_once("function f9 \"worker\" async {").unwrap();
    let (inner_body, rest) = rest.split_once("function f10 ").unwrap();
    let (main, tests) = rest.split_once("tests {").unwrap();
    let worker = r#"function f9 "worker" async {
    registers [t2, t2, t2, t3, t4, t2, t3, t0]
    params [r0, r1, r2, r3, r4, r5]
    result t0
    0: call r6, f11, [r0, r1, r2, r3, r4, r5]
    1: await r7, r6
    2: ret r7
}
"#;
    let inner = format!("function f11 \"inner\" async {{{inner_body}");
    let main = main.replace("cancel after await settled", "nested cancel after await settled");
    format!("{head}{worker}function f10 {main}{inner}tests {{{tests}")
}

#[test]
fn nested_frame_cancel_after_settle_is_delivered_at_its_next_await() {
    assert!(passes(&nested_cancel_after_settle()));
}

/// #670 with a structured child: only `inner` awaits the spawned child. main
/// awaits `sib`, which waits on the same event as the child and registered
/// first, so its settlement queues main before the child's queues `inner`.
fn structured_cancel_after_settle(take: bool) -> String {
    let nested = nested_cancel_after_settle();
    let (head, rest) = nested.split_once("function f10 ").unwrap();
    let (_, rest) = rest.split_once("function f11 ").unwrap();
    let main = r#"function f10 "structured child settled before cancel" async {
    registers [t1, t2, t2, t2, t2, t0, t4, t3, t3, t0, t5, t2, t1, t0, t3]
    params []
    result void
    0: const r0, bool false
    1: call r1, f0, [r0]
    2: call r2, f0, [r0]
    3: call r3, f0, [r0]
    4: call r4, f0, [r0]
    5: const r5, int 0
    6: call r6, f3, [r5]
    7: call r14, f8, [r2]
    8: fut.spawn r14, r14
    9: call r7, f8, [r2]
    10: fut.spawn r7, r7
    11: call r8, f9, [r1, r2, r3, r7, r6, r4]
    12: fut.spawn r8, r8
    13: call _, f1, [r1]
    14: await r9, r14
    15: const r5, int 42
    16: call _, f6, [r9, r5]
    17: fut cancel, _, r8
    18: call r10, f2, [r4]
    19: await r11, r10
    20: call r13, f4, [r6]
    21: call _, f6, [r13, r5]
    22: fut cancelled, r12, r8
    23: const r0, bool true
    24: call _, f7, [r12, r0]
    25: ret _
}
"#;
    let source = format!("{head}{main}function f11 {rest}");
    if !take {
        return source;
    }
    // The worker's await of `inner` and `inner`'s await of the child take
    // their outcomes (`rt_await_take`).
    let source = source.replace("    1: await r7, r6\n    2: ret r7\n", "    1: await.take r7, r6\n    2: ret r7\n");
    source.replace("    3: await r8, r3\n", "    3: await.take r8, r3\n")
}

#[test]
fn structured_child_settled_before_cancel_is_delivered_at_the_next_await() {
    assert!(passes(&structured_cancel_after_settle(false)));
}

#[test]
fn await_take_in_a_nested_frame_delivers_a_later_cancellation_at_the_next_await() {
    let source = structured_cancel_after_settle(true);
    assert_eq!(source.matches("await.take").count(), 2);
    assert!(passes(&source));
}

/// The #670 walk over the frames awaiting a frame, on hand-built frames.
mod requested_walk {
    use crate::heap;
    use crate::runtime::Runtime;
    use crate::tasks::*;
    use crate::word::{Repr, UNBOUND, Word};

    fn runtime() -> Box<Runtime> {
        let program = okkama_sparrow::Program::default();
        Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap())
    }

    /// A bare async frame: header and fixed words, nothing awaited, no waiter.
    fn frame(runtime: &mut Runtime) -> Word {
        let frame = runtime.heap.allocate(1 + FIXED_WORDS);
        // SAFETY: fresh storage of 1 + FIXED_WORDS words.
        unsafe {
            *(frame as *mut u64) = heap::header(heap::FRAME, Repr::Raw, 0);
            std::ptr::write_bytes((frame + 8) as *mut u64, 0, FIXED_WORDS);
        }
        set(frame, WAITER, UNBOUND);
        set(frame, AWAITING, UNBOUND);
        frame
    }

    /// `waiter` awaits `future`.
    fn awaits(runtime: &mut Runtime, waiter: Word, future: Word) {
        runtime.listen(future, waiter);
        set(waiter, AWAITING, future);
    }

    fn request(frame: Word) {
        set(frame, FLAGS, get(frame, FLAGS) | CANCEL_REQUESTED);
    }

    #[test]
    fn a_deep_chain_of_nested_frames_is_walked_without_recursion() {
        let mut runtime = runtime();
        let bottom = frame(&mut runtime);
        let mut top = bottom;
        for _ in 0..200_000 {
            let next = frame(&mut runtime);
            awaits(&mut runtime, next, top);
            top = next;
        }
        assert_eq!(runtime.requested_above(bottom), None);
        request(top);
        assert_eq!(runtime.requested_above(bottom), Some(top));
    }

    #[test]
    fn the_first_request_depth_first_in_waiter_order_is_found() {
        let mut runtime = runtime();
        let future = frame(&mut runtime);
        let (first, second, above_first) = (frame(&mut runtime), frame(&mut runtime), frame(&mut runtime));
        awaits(&mut runtime, first, future);
        awaits(&mut runtime, second, future);
        awaits(&mut runtime, above_first, first);
        request(second);
        request(above_first);
        assert_eq!(runtime.requested_above(future), Some(above_first));
        // A delivered request no longer counts.
        set(above_first, FLAGS, get(above_first, FLAGS) | CANCEL_DELIVERED);
        assert_eq!(runtime.requested_above(future), Some(second));
    }

    #[test]
    fn listeners_spawned_and_shielded_frames_end_the_walk() {
        let mut runtime = runtime();
        let future = frame(&mut runtime);
        // A frame that only listens to the future does not await it.
        let listener = frame(&mut runtime);
        runtime.listen(future, listener);
        request(listener);
        assert_eq!(runtime.requested_above(future), None);
        // A spawned or shielded waiter is a task boundary: requests above it do not count.
        for boundary in [SPAWNED, SHIELDED] {
            let future = frame(&mut runtime);
            let task = frame(&mut runtime);
            let above = frame(&mut runtime);
            awaits(&mut runtime, task, future);
            awaits(&mut runtime, above, task);
            set(task, FLAGS, boundary);
            request(above);
            assert_eq!(runtime.requested_above(future), None);
            request(task);
            assert_eq!(runtime.requested_above(future), Some(task));
        }
    }
}

/// #932: an action without inputs publishes its outcome as it completes; one with inputs waits for
/// `finish_inputs` (#587).
mod complete_action {
    use crate::runtime::Runtime;
    use crate::tasks::*;
    use crate::word::{Repr, UNBOUND, Word};

    fn runtime() -> Box<Runtime> {
        let program = okkama_sparrow::assemble(
            "sparrow 18\nmodule \"complete\" \"1.0.0\"\ntype t0 int\ntype t1 object obj0\n\
             object obj0 \"Lease\" {\n destroy f2\n}\n\
             function f0 \"main\" {\n registers [t0]\n params []\n result t0\n 0: const r0, int 0\n 1: ret r0\n}\n\
             function f1 \"work\" async {\n registers [t0]\n params []\n result t0\n 0: const r0, int 0\n 1: ret r0\n}\n\
             function f2 \"Lease.destroy\" {\n registers [t1]\n params [r0]\n result void\n 0: ret _\n}\n\
             entry f0\n",
        ).unwrap();
        Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap())
    }

    fn running_frame(runtime: &mut Runtime) -> Word {
        let frame = runtime.future_new(1, &[]);
        set(frame, STATE, RUNNING);
        frame
    }

    #[test]
    fn an_action_without_inputs_is_published_at_once_and_one_with_inputs_is_not() {
        let mut runtime = runtime();
        let future = running_frame(&mut runtime);
        let waiter = running_frame(&mut runtime);
        runtime.listen(future, waiter);
        set(waiter, AWAITING, future);
        runtime.complete_action(future, DONE, 7);
        assert_eq!(get(future, STATE), DONE);
        assert_eq!(get(future, RESULT), 7);
        assert_eq!(get(future, FLAGS) & COMPLETING, 0);
        assert_eq!(get(waiter, AWAITING), UNBOUND, "the waiter is woken");
        let held = running_frame(&mut runtime);
        let inputs = runtime.heap.allocate_array(Repr::Ref, &[UNBOUND; 5]);
        runtime.set_held(held, Held::Inputs, inputs);
        runtime.complete_action(held, DONE, 9);
        assert_eq!(get(held, STATE), RUNNING, "published after finish_inputs");
        assert_ne!(get(held, FLAGS) & COMPLETING, 0);
    }

    /// #932: the ownership regions live behind the registers, in programs with resources only; a program without
    /// them keeps 0.5.8's frame and reads them as unbound.
    #[test]
    fn ownership_regions_live_behind_the_registers_only_with_resources() {
        let mut runtime = runtime();
        let frame = runtime.future_new(1, &[]);
        let info = &runtime.loaded.functions[1];
        let at = info.held_at.expect("a program with resources");
        assert_eq!(at, REGISTERS + info.kinds.len() as u64 * 8);
        assert_eq!(info.identity_at, Some(at + 16), "no parameters: the identity follows the regions");
        let region = runtime.heap.allocate_array(Repr::Ref, &[UNBOUND; 5]);
        runtime.set_held(frame, Held::Owned, region);
        assert_eq!(get(frame, at), region);
        assert_eq!(runtime.held(frame, Held::Owned), region);
        assert_eq!(runtime.held(frame, Held::Inputs), UNBOUND);
        runtime.heap.collect(|visit| visit(frame, Repr::Ref));
        assert_eq!(runtime.heap.array_length(runtime.held(frame, Held::Owned)), 5, "the collector traces the region");
        let program = okkama_sparrow::assemble(
            "sparrow 18\nmodule \"plain\" \"1.0.0\"\ntype t0 int\n\
             function f0 \"work\" async {\n registers [t0]\n params []\n result t0\n 0: const r0, int 0\n 1: ret r0\n}\n",
        ).unwrap();
        let mut plain = Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap());
        assert_eq!(plain.loaded.functions[0].held_at, None);
        assert_eq!(plain.loaded.layouts().frames[0].len(), FIXED_WORDS + 1, "fixed words and one register");
        let frame = plain.future_new(0, &[]);
        assert_eq!(plain.held(frame, Held::Inputs), UNBOUND);
    }
}
