//! #329: single-object cleanup uses the ordinary ownership and async protocols.
use super::*;
use crate::tasks::{self, *};

fn runtime(body: &str) -> Box<Runtime> {
    let source = format!(r#"sparrow 18
module "single-async-close" "1"
type t0 int
type t1 object obj0
type t2 object obj1
type t3 future t2
object obj0 "Guard" {{ field "gate" t2
 destroy f1
}}
object obj1 "Event" {{}}
function f0 "owner" async {{
 registers [t0]
 params []
 result t0
 0: const r0, int 7
 1: ret r0
}}
function f1 "Guard.destroy" async {{
 registers [t1, t2, t3, t2]
 params [r0]
 result void
{body}
}}
function f2 "Event.new" intrinsic "okkama.event.Event.new" [obj1] [] {{
 registers []
 params []
 result t2
}}
function f3 "Event.wait" async intrinsic "okkama.event.Event.wait" [] [] {{
 registers [t2]
 params [r0]
 result t2
}}
entry f0
"#);
    Box::new(Runtime::new(crate::program::Loaded::new(okkama_sparrow::assemble(&source).unwrap())).unwrap())
}

#[inline(always)]
fn context(runtime: &mut Runtime) -> Ctx {
    Ctx { status: OK, stack_limit: 0, failure: UNBOUND, runtime,
        entry_frame: crate::runtime::frame_pointer!(), scratch: UNBOUND }
}

fn region(runtime: &mut Runtime) -> Word {
    runtime.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS])
}

fn guard(runtime: &mut Runtime, home: Word, gate: Word) -> Word {
    let object = runtime.heap.object_new(0, &[gate]);
    runtime.created(0, object);
    runtime.region_add(home, object, true);
    object
}

fn next(runtime: &mut Runtime, home: Word, keep: &[Word]) -> Closed {
    // Selection never calls machine code: ctx is unused for an async destroy.
    runtime.region_close_until(std::ptr::null_mut(), home, keep, None, true)
}

#[test]
fn single_async_close_keeps_reverse_creation_order_and_destroys_once() {
    let mut r = runtime(" 0: ret _");
    let home = region(&mut r);
    let older = guard(&mut r, home, UNBOUND);
    let newer = guard(&mut r, home, UNBOUND);
    // Deliberately track in the opposite order to creation.
    r.heap.set_field(home, 1, newer);
    r.heap.set_field(home, 2, older);
    for expected in [newer, older] {
        assert!(matches!(next(&mut r, home, &[]), Closed::Async(object, None) if object == expected));
        assert_eq!(r.owner_of(expected), Some(Owner::Dead));
    }
    assert!(matches!(next(&mut r, home, &[]), Closed::Done(None)));
    assert_eq!(r.region_len(home), 0);
    assert!(matches!(next(&mut r, home, &[]), Closed::Done(None)));
    // Also exercise the single-object path itself, including the scalar keep.
    let object = guard(&mut r, home, UNBOUND);
    assert!(matches!(next(&mut r, home, &[3]), Closed::Async(found, None) if found == object));
    assert!(matches!(next(&mut r, home, &[]), Closed::Done(None)));
}

#[test]
fn single_async_close_preserves_a_kept_graph_and_a_foreign_owner() {
    let mut r = runtime(" 0: ret _");
    let home = region(&mut r);
    let object = guard(&mut r, home, UNBOUND);
    let kept = r.heap.allocate_array(Repr::Ref, &[object]);
    assert!(matches!(next(&mut r, home, &[kept]), Closed::Done(None)));
    assert_eq!(r.owner_of(object), Some(Owner::Free));
    assert_eq!(r.region_len(home), 0);
    r.region_add(home, object, true);
    let other = region(&mut r);
    r.set_owner(object, Owner::Region(other));
    assert!(matches!(next(&mut r, home, &[]), Closed::Done(None)));
    assert_eq!(r.owner_of(object), Some(Owner::Region(other)));
}

#[test]
fn single_async_close_hands_a_pinned_object_to_its_holder() {
    let mut r = runtime(" 0: ret _");
    let home = region(&mut r);
    let object = guard(&mut r, home, UNBOUND);
    let holder = region(&mut r);
    r.pin(object, holder);
    assert!(matches!(next(&mut r, home, &[]), Closed::Done(None)));
    assert_eq!(r.owner_of(object), Some(Owner::Region(holder)));
    assert!(matches!(next(&mut r, holder, &[]), Closed::Async(found, None) if found == object));
}
#[test]
fn single_async_close_links_destroy_failure_and_keeps_unwind_primary() {
    crate::on_vm_thread(|| {
        for unwind in [false, true] {
            let mut r = runtime(" 0: trap confirm, \"destroy failed\"");
            let home = region(&mut r);
            guard(&mut r, home, UNBOUND);
            let this = r.future_new(0, &[]);
            let mut ctx = context(&mut r);
            let primary = if unwind { r.failure_new(FailureKind::Panic("body failed".into())) } else { UNBOUND };
            assert_eq!(rt_region_settle(&mut ctx, this, home, UNBOUND, primary), AWAIT_READY);
            let failure = if unwind {
                assert_eq!(ctx.status, OK);
                assert_eq!(ctx.scratch, primary);
                let secondary = &r.failure(primary).unwrap().secondary;
                assert_eq!(secondary.len(), 1);
                secondary[0]
            } else {
                assert_eq!(ctx.status, FAILED);
                ctx.failure
            };
            assert!(matches!(&r.failure(failure).unwrap().kind, FailureKind::Program(text) if text.contains("destroy failed")));
            assert_eq!(r.region_len(home), 0);
        }
        Ok(())
    }).unwrap();
}
#[test]
fn single_async_close_shields_a_suspended_destroy_and_roots_it_through_gc() {
    crate::on_vm_thread(|| {
        let mut r = runtime(" 0: obj.get r1, r0, 0\n 1: call r2, f3, [r1]\n 2: await r3, r2\n 3: ret _");
        let gate = r.run_function(2, crate::STACK_BYTES).unwrap();
        let home = region(&mut r);
        let object = guard(&mut r, home, gate);
        let this = r.future_new(0, &[]);
        tasks::set(this, STATE, RUNNING);
        let mut ctx = context(&mut r);
        assert_eq!(rt_region_settle(&mut ctx, this, home, UNBOUND, UNBOUND), AWAIT_PENDING);
        let destroy = r.region_settling(home).0.unwrap();
        r.cancel(&mut ctx, this);
        assert_eq!(tasks::get(this, AWAITING), destroy, "cancellation must not detach the cleanup awaiter");
        assert!(r.ready.is_empty(), "cancellation must not resume the shielded destroy");
        // An ordinary explicit collection, not a GC-stress build: the region traces both receiver and future.
        r.heap.collect(|visit| { visit(home, Repr::Ref); visit(this, Repr::Ref); });
        assert_eq!(r.heap.field(object, 0), gate);
        assert_eq!(r.region_settling(home).0, Some(destroy));
        let event = r.event_of(gate).unwrap();
        r.event_set(&event);
        while let Some(frame) = r.ready.pop_front() {
            if frame != this { r.run_body(&mut ctx, frame); }
        }
        assert_eq!(tasks::get(destroy, STATE), DONE);
        assert_eq!(rt_region_settle(&mut ctx, this, home, UNBOUND, UNBOUND), AWAIT_READY);
        assert_eq!(ctx.status, OK);
        assert_eq!(r.region_len(home), 0);
        // The request survived shielding and is delivered at the next ordinary await.
        let ready = r.future_new(0, &[]);
        r.running.push(this);
        assert_eq!(rt_await(&mut ctx, this, ready), AWAIT_READY);
        r.running.pop();
        assert_eq!(ctx.status, FAILED);
        assert!(matches!(r.failure(ctx.failure).unwrap().kind, FailureKind::Cancelled));
        Ok(())
    }).unwrap();
}

#[test]
fn single_async_close_region_insertion_is_idempotent_inline_and_overflow() {
    let mut r = runtime(" 0: ret _");
    let home = region(&mut r);
    let mut objects = Vec::new();
    for _ in 0..REGION_SLOTS + 2 {
        let object = guard(&mut r, home, UNBOUND);
        r.region_push(home, object);
        objects.push(object);
    }
    for &object in &objects { r.region_push(home, object); }
    assert_eq!(r.region_len(home), objects.len());
    for &object in &objects { assert!(r.region_contains(home, object)); }
    for &expected in objects.iter().rev() {
        assert!(matches!(next(&mut r, home, &[]), Closed::Async(found, None) if found == expected));
    }
    assert!(matches!(next(&mut r, home, &[]), Closed::Done(None)));
}

#[test]
fn single_async_close_roots_the_receiver_before_its_future_exists() {
    let mut r = runtime(" 0: ret _");
    let home = region(&mut r);
    let object = guard(&mut r, home, UNBOUND);
    r.heap.watch(object);
    assert!(matches!(next(&mut r, home, &[]), Closed::Async(found, None) if found == object));
    r.heap.collect(|visit| visit(home, Repr::Ref));
    assert!(!r.heap.take_released().contains(&object), "the selected receiver must survive until future_new retains it");
    assert_eq!(r.owner_of(object), Some(Owner::Dead));
}
