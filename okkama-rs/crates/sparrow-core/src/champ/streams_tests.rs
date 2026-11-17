//! The refusal of a stream read whose callee returned a duty (#810 G1 step 4, ruling Q2; review of steps 3–4, C1).
use super::runtime_tests::Harness;
use super::streams::{refuse_origin, REFUSED_ORIGIN};
use super::*;
use crate::runtime::{frame_pointer, FailureKind, FAILED, FATAL};
use crate::word;

fn refusal(h: &Harness) -> &crate::runtime::Failure {
    h.runtime.failure(h.ctx.failure).expect("the refusal")
}

/// The returned packet closes at once, its destroy runs, and the read fails with the refusal.
#[test]
fn a_refused_origin_closes_its_packet_and_fails_with_the_refusal() {
    let mut h = Harness::new();
    h.ctx.entry_frame = frame_pointer!();
    let owner = h.runtime.champ_new_owner().unwrap();
    let (good, mut packet) = h.fresh(0, owner);
    h.runtime.roots.push((good, crate::word::Repr::Ref));
    refuse_origin(&mut h.ctx, &mut packet);
    assert_eq!(h.ctx.status, FAILED);
    assert!(matches!(&refusal(&h).kind, FailureKind::Program(text) if text == REFUSED_ORIGIN));
    assert!(refusal(&h).secondary.is_empty());
    assert_eq!(h.identity(good).life, Life::Dead);
    assert_eq!(h.runtime.heap.field(good, 0), word::small(9).unwrap());
}

/// A destroy that fails during the close links to the refusal as a secondary; it is not dropped.
#[test]
fn a_failing_destroy_during_the_refusal_is_its_secondary() {
    let mut h = Harness::new();
    h.ctx.entry_frame = frame_pointer!();
    let owner = h.runtime.champ_new_owner().unwrap();
    let (bad, mut packet) = h.fresh(1, owner);
    h.runtime.roots.push((bad, crate::word::Repr::Ref));
    refuse_origin(&mut h.ctx, &mut packet);
    assert_eq!(h.ctx.status, FAILED);
    let failure = refusal(&h);
    assert!(matches!(&failure.kind, FailureKind::Program(text) if text == REFUSED_ORIGIN));
    assert_eq!(failure.secondary.len(), 1);
    let secondary = h.runtime.failure(failure.secondary[0]).unwrap();
    assert!(secondary.cleanup_origin && matches!(&secondary.kind, FailureKind::Panic(_)));
    assert_eq!(h.identity(bad).life, Life::Dead);
}

/// A stop during the close stays a stop: here the close finds no stack headroom left (FATAL), and no catchable
/// failure replaces that (before the review's C1 the refusal overwrote it with FAILED).
#[test]
fn a_stop_during_the_refusal_stays_a_stop() {
    let mut h = Harness::new();
    h.ctx.entry_frame = frame_pointer!();
    let owner = h.runtime.champ_new_owner().unwrap();
    let (good, mut packet) = h.fresh(0, owner);
    h.runtime.roots.push((good, crate::word::Repr::Ref));
    h.ctx.stack_limit = u64::MAX;
    refuse_origin(&mut h.ctx, &mut packet);
    assert_eq!(h.ctx.status, FATAL);
    assert_eq!(h.ctx.failure, 0);
    assert_eq!(h.runtime.fatal.as_deref(), Some("Champ: cleanup stack headroom exhausted"));
}
