//! Equivalent runtime inputs for every bounded #591 low/executor_tests.rs scenario.
//! Expected event sequences are fixed by the independent model, never read from a Low plan.
use super::*;
use super::cleanup::*;
use super::helpers::{data, packet, scope};
use super::records::Data;
use super::runtime_tests::Harness;
use crate::runtime::{frame_pointer, Ctx, FailureKind, FAILED, OK};
use crate::word::{self, Repr, Word};
use okkama_sparrow::{Constant, Dst, Instruction as I, Register as R, TypeRef};

struct Fixture {
    h: Harness,
    log: Word,
    objects: Vec<(Word, u64)>,
    failures: Vec<(u64, Word)>,
}
impl Fixture {
    fn new() -> Self {
        let mut program = Harness::new().runtime.loaded.program.clone();
        for (index, function) in program.functions.iter_mut().enumerate() {
            function.registers = vec![TypeRef(2 + index as u32), TypeRef(0), TypeRef(0), TypeRef(0),
                TypeRef(1), TypeRef(2), TypeRef(0), TypeRef(0), TypeRef(0)];
            function.code = vec![
                I::ObjGet { dst: Dst(R(1)), object: R(0), field: 0 },
                I::ObjGet { dst: Dst(R(5)), object: R(0), field: 1 },
                I::ObjGet { dst: Dst(R(6)), object: R(5), field: 0 },
                I::Const { dst: Dst(R(7)), value: Constant::Int("10".into()) },
                I::IntMul { dst: Dst(R(8)), left: R(6), right: R(7) },
                I::IntAdd { dst: Dst(R(6)), left: R(8), right: R(1) },
                I::ObjInit { object: R(5), field: 0, value: R(6) },
                I::IntAdd { dst: Dst(R(3)), left: R(1), right: R(7) },
                I::ObjInit { object: R(0), field: 0, value: R(3) },
            ];
            if index == 0 {
                function.code.push(I::Return { value: None });
            } else {
                function.code.push(I::Const { dst: Dst(R(4)), value: Constant::Str("destroy failed".into()) });
                function.code.push(I::Panic { message: R(4) });
            }
        }
        let mut runtime = Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap());
        runtime.heap.set_stress(true);
        let ctx = Ctx {
            status: OK, stack_limit: 0, failure: 0, runtime: &mut *runtime, entry_frame: 0, scratch: 0,
        };
        let mut h = Harness { runtime, ctx };
        // An uncommitted object is a plain diagnostic ledger, never a cleanup obligation.
        let log = h.runtime.heap.object_new(0, &[word::small(0).unwrap()]);
        h.runtime.roots.push((log, Repr::Ref));
        Self { h, log, objects: Vec::new(), failures: Vec::new() }
    }
    fn fresh(&mut self, event: u64, failing: bool, owner: OwnerId) -> PacketSlot {
        let (object, packet) = self.h.fresh(u64::from(failing), owner);
        self.h.runtime.heap.set_field(object, 0, word::small(event as i64).unwrap());
        self.h.runtime.heap.set_field(object, 1, self.log);
        self.objects.push((object, event));
        packet
    }
    fn trace(&self) -> String {
        word::small_value(self.h.runtime.heap.field(self.log, 0)).unwrap().to_string()
    }
    fn assert_once(&self) {
        for &(object, event) in &self.objects {
            assert_eq!(self.h.identity(object).life, Life::Dead);
            assert_eq!(self.h.runtime.heap.field(object, 0), word::small(event as i64 + 10).unwrap());
        }
    }
    fn pin_dead_checks(&mut self) {
        // Diagnostic aliases let us inspect Dead tails after later safepoints, without hiding roots
        // for any live identity: each pin is installed only after that identity became Dead.
        for &(object, _) in &self.objects {
            if self.h.identity(object).life == Life::Dead {
                self.h.runtime.roots.push((object, Repr::Ref));
            }
        }
    }
    fn begin(&mut self, input: &mut PacketSlot, pending: Option<&mut PendingSlot>, failure: Word,
        settlements: u64) -> CleanupSlot
    {
        let inputs = CloseInputs {
            scope: std::ptr::null_mut(), packet: input,
            pending: pending.map_or(std::ptr::null_mut(), |pending| pending), failure, settlements,
        };
        let mut state = CleanupSlot::default();
        assert_eq!(rt_cleanup_begin(&mut self.h.ctx, &inputs, &mut state), NORMAL);
        if settlements == 0 {
            assert_eq!(rt_cleanup_settle_empty(&mut self.h.ctx, &mut state, &mut state), NORMAL);
        }
        state
    }
    fn plan(&mut self, state: &mut CleanupSlot, reserve: bool, direct: bool) {
        let plan = self.h.runtime.champ_register_cleanup_plan(CleanupPlan { reserve, direct, may_fail: true });
        assert_eq!(rt_cleanup_plan(&mut self.h.ctx, state, plan, state), NORMAL);
    }
    fn step(&mut self, state: &mut CleanupSlot) -> bool {
        self.h.gc(&[state.record]);
        let mut next = NextPost::default();
        let ordinal = rt_cleanup_next(&mut self.h.ctx, state, &mut next);
        assert!(ordinal <= 1, "{:?}", self.h.runtime.fatal);
        *state = next.state;
        if ordinal == 1 {
            return false;
        }
        self.destroy(state, next.key, false);
        true
    }
    fn destroy(&mut self, state: &mut CleanupSlot, key: KeyRef, direct: bool) {
        self.h.gc(&[state.record]);
        let mut post = DestroyPost::default();
        self.h.ctx.entry_frame = frame_pointer!();
        if direct {
            let entry = crate::heap::payload(self.h.runtime.heap.header_of(key.object())) as u64;
            assert_eq!(rt_destroy_direct(&mut self.h.ctx, state, key, entry, &mut post), NORMAL);
        } else {
            assert_eq!(rt_cleanup_destroy(&mut self.h.ctx, state, key, &mut post), NORMAL);
        }
        assert_eq!(state.record, 0);
        *state = post.state;
        assert_eq!(self.h.identity(key.object()).life, Life::Dead);
        self.pin_dead_checks();
        assert_eq!(self.h.ctx.status, OK);
        assert_eq!(self.h.ctx.failure, 0);
        self.h.gc(&[state.record]);
        if post.failure != 0 {
            let event = self.objects.iter().find(|(object, _)| *object == key.object()).unwrap().1;
            self.failures.push((event, post.failure));
            // A selected prefix is already Dead; next must wait for explicit failure recording.
            let mut forbidden = NextPost::default();
            assert_eq!(rt_cleanup_next(&mut self.h.ctx, state, &mut forbidden), OUTCOME_STOP);
            self.h.reset_fatal();
            assert_eq!(rt_cleanup_failure(&mut self.h.ctx, state, post.failure, state), NORMAL);
        }
    }
    fn finish(&mut self, mut state: CleanupSlot, direct: bool) -> FinishPost {
        if direct {
            // Compiled direct Low emits the verified chain with no cleanup.next terminator.
            let objects: Vec<_> = self.objects.iter().rev().map(|&(object, _)| object).collect();
            for object in objects {
                let serial = self.h.identity(object).serial;
                self.destroy(&mut state, KeyRef { object, serial }, true);
            }
        } else {
            while self.step(&mut state) {}
        }
        let mut post = FinishPost::default();
        assert_eq!(rt_cleanup_finish(&mut self.h.ctx, &mut state, &mut post), NORMAL);
        assert_eq!(state.record, 0);
        assert_eq!(post.closed, 1);
        self.h.gc(&[post.failure, post.pending.record]);
        post
    }
    fn reserve(&mut self, input: &mut PacketSlot) -> PendingSlot {
        let mut pending = PendingSlot::default();
        let inputs = ExitInputs {
            kind: 1, boundary: 1, value: 0, value_repr: Repr::Raw as u64, packet: input, selection: 0,
        };
        assert_eq!(rt_exit_prepare(&mut self.h.ctx, &inputs, &mut pending), NORMAL);
        pending
    }
    fn join(&mut self, mut inputs: Vec<PacketSlot>) -> PacketSlot {
        let mut output = PacketSlot::default();
        assert_eq!(rt_own_join(&mut self.h.ctx, inputs.as_mut_ptr(), inputs.len() as u64, &mut output), NORMAL);
        output
    }
}

#[test]
fn champ_s3_model_whole_reserve_and_closed_inner_prefix_traces() {
    // Model fixtures: unwind-reserve, end-reserve, two-scope-{unwind,end}-reserve, two-scope-suffix.
    for closed_inner in [false, true] {
        for (before, c, expected) in [(true, false, "21"), (false, false, "12"), (false, true, "321")] {
            let mut f = Fixture::new();
            let owner = f.h.runtime.champ_new_owner().unwrap();
            let a = f.fresh(1, !before && !c, owner);
            let mut r = f.fresh(2, c, owner);
            let mut pending = f.reserve(&mut r);
            if closed_inner {
                f.h.runtime.roots.push((a.record, Repr::Ref));
                let mut empty = f.h.empty();
                let mut inner = f.begin(&mut empty, Some(&mut pending), 0, 0);
                f.plan(&mut inner, true, false);
                let post = f.finish(inner, false);
                assert_eq!(post.failure, 0);
                pending = post.pending;
            }
            let mut entries = vec![a];
            if c {
                entries.push(f.fresh(3, true, owner));
            }
            let mut inventory = f.join(entries);
            let primary = if before { f.h.runtime.failure_new(FailureKind::Panic("defer".into())) } else { 0 };
            let mut state = f.begin(&mut inventory, Some(&mut pending), 0, 0);
            if before {
                assert_eq!(rt_cleanup_failure(&mut f.h.ctx, &mut state, primary, &mut state), NORMAL);
            }
            f.plan(&mut state, true, false);
            let post = f.finish(state, false);
            assert_eq!(f.trace(), expected);
            f.assert_once();
            assert_ne!(post.failure, 0);
            assert_eq!(post.pending.record, 0);
            let failure = f.h.runtime.failure(post.failure).unwrap();
            assert_eq!(failure.secondary.len(), usize::from(c));
            if before {
                assert_eq!(post.failure, primary);
                assert!(failure.cleanup_origin);
            } else {
                assert!(failure.cleanup_origin);
            }
        }
    }
}

#[test]
fn champ_s3_model_local_pair_return_old_and_triplet() {
    // local-pair direct, return-old merge, finite local triplet creation order.
    for (direct, failing, count, expected) in [(true, true, 2, "21"), (false, true, 2, "21"),
        (true, false, 3, "321")]
    {
        let mut f = Fixture::new();
        let owner = f.h.runtime.champ_new_owner().unwrap();
        let packets = (1..=count).map(|event| f.fresh(event, failing, owner)).collect();
        let mut packet = f.join(packets);
        let mut state = f.begin(&mut packet, None, 0, 0);
        f.plan(&mut state, false, direct);
        let post = f.finish(state, direct);
        assert_eq!(f.trace(), expected);
        f.assert_once();
        if failing {
            let failure = f.h.runtime.failure(post.failure).unwrap();
            assert!(failure.cleanup_origin);
            assert_eq!(failure.secondary.len(), 1);
            let secondary = f.h.runtime.failure(failure.secondary[0]).unwrap();
            assert!(secondary.cleanup_origin);
            assert_eq!(failure.trace.len(), 1);
            assert_eq!(secondary.trace.len(), 1);
        } else {
            assert_eq!(post.failure, 0);
        }
    }
}

#[test]
fn champ_s3_model_mixed_static_journal_dynamic_and_settled_packet() {
    // mixed-journal: J2 J1 S; mixed-protocol: J2 J1 P S, with both protocol outcomes.
    for (protocol, fails, settled) in [
        (false, false, false), (true, false, false), (true, true, false),
        (true, false, true), (true, true, true),
    ] {
        let mut f = Fixture::new();
        let mut scope_slot = f.h.scope();
        let info = scope(&f.h.runtime, scope_slot.record).unwrap();
        let object = f.h.runtime.heap.object_new(0, &[word::small(1).unwrap(), f.log]);
        f.h.runtime.champ_create_commit(object, info.holder, info.owner).unwrap();
        f.objects.push((object, 1));
        let mut entries = [StaticEntry::default(), StaticEntry { object, active: 1 }];
        let mut s = f.h.runtime.champ_packet_from_static(&mut entries, info.holder, info.owner).unwrap();
        assert!(entries.iter().all(|entry| entry.active == 0 && entry.object == 0));
        assert_eq!(rt_own_admit(&mut f.h.ctx, &mut scope_slot, &mut s, &mut scope_slot), NORMAL);
        let mut p = if protocol { f.fresh(2, false, info.owner) } else { f.h.empty() };
        for event in if protocol { 3..=4 } else { 2..=3 } {
            let mut packet = f.fresh(event, false, info.owner);
            let object = f.objects.last().unwrap().0;
            assert_eq!(rt_own_rebind(&mut f.h.ctx, &mut scope_slot, 0, object, &mut packet,
                &mut scope_slot), NORMAL);
        }
        let failure = if fails { f.h.runtime.failure_new(FailureKind::Program("protocol".into())) } else { 0 };
        if protocol && !settled {
            // The model's storage protocol returns either an admitted inventory or an unaccepted
            // packet that its unwind CFG accepts before close. Both preserve P's old serial.
            assert_eq!(rt_own_accept(&mut f.h.ctx, &mut scope_slot, &mut p, &mut scope_slot), NORMAL);
        }
        let mut state = CleanupSlot::default();
        let inputs = CloseInputs {
            scope: &mut scope_slot, packet: std::ptr::null_mut(), pending: std::ptr::null_mut(),
            failure: if settled { 0 } else { failure }, settlements: u64::from(settled),
        };
        assert_eq!(rt_cleanup_begin(&mut f.h.ctx, &inputs, &mut state), NORMAL);
        f.h.gc(&[state.record, p.record, failure]);
        if settled {
            assert_eq!(rt_cleanup_settle_packet(&mut f.h.ctx, &mut state, &mut p, failure, &mut state), NORMAL);
        } else {
            assert_eq!(rt_cleanup_settle_empty(&mut f.h.ctx, &mut state, &mut state), NORMAL);
        }
        f.plan(&mut state, false, false);
        let post = f.finish(state, false);
        assert_eq!(f.trace(), if protocol { "4321" } else { "321" });
        f.assert_once();
        assert_eq!(post.failure != 0, fails);
        if fails {
            assert!(matches!(&f.h.runtime.failure(post.failure).unwrap().kind,
                FailureKind::Program(message) if message == "protocol"));
            assert_eq!(f.h.runtime.failure(post.failure).unwrap().cleanup_origin, settled);
        }
    }
}

#[test]
fn champ_s3_model_replacement_rest_bound_at_outer_boundary_once() {
    // replacement-rest.own.oir: A stays outer; R leaves an already closed inner scope as reserve.
    let mut f = Fixture::new();
    let mut outer = f.h.scope();
    let info = scope(&f.h.runtime, outer.record).unwrap();
    let mut a = f.fresh(1, false, info.owner);
    assert_eq!(rt_own_admit(&mut f.h.ctx, &mut outer, &mut a, &mut outer), NORMAL);
    f.h.runtime.roots.push((outer.record, Repr::Ref));
    let mut r = f.fresh(2, false, info.owner);
    let mut pending = f.reserve(&mut r);
    let mut empty = f.h.empty();
    let mut state = f.begin(&mut empty, Some(&mut pending), 0, 0);
    f.plan(&mut state, true, false);
    let mut inner = f.finish(state, false);
    assert_eq!(inner.failure, 0);
    assert_eq!(f.trace(), "0");
    let selection = f.h.runtime.champ_register_selection(vec![]);
    let mut empty = f.h.empty();
    let replacement = StaticReplacement {
        value: word::small(17).unwrap(), value_repr: Repr::Int as u64,
        packet: &mut empty, selection, boundary: 1,
    };
    let mut replaced = ReplacePost::default();
    assert_eq!(rt_exit_replace(&mut f.h.ctx, &mut inner.pending, &replacement, &mut replaced), NORMAL);
    assert_eq!(rt_own_accept(&mut f.h.ctx, &mut outer, &mut replaced.rest, &mut outer), NORMAL);
    let inputs = CloseInputs {
        scope: &mut outer, packet: std::ptr::null_mut(), pending: &mut replaced.pending,
        failure: 0, settlements: 0,
    };
    let mut state = CleanupSlot::default();
    assert_eq!(rt_cleanup_begin(&mut f.h.ctx, &inputs, &mut state), NORMAL);
    assert_eq!(rt_cleanup_settle_empty(&mut f.h.ctx, &mut state, &mut state), NORMAL);
    f.plan(&mut state, true, false);
    let mut post = f.finish(state, false);
    assert_eq!(post.failure, 0);
    assert_eq!(f.trace(), "21");
    f.assert_once();
    let destination = ExitDestination { holder: info.holder, owner: info.owner };
    let mut committed = CommitPost::default();
    assert_eq!(rt_exit_commit(&mut f.h.ctx, &mut post.pending, &destination, &mut committed), NORMAL);
    assert_eq!(committed.value, word::small(17).unwrap());
    assert!(f.h.runtime.champ_packet_keys(&committed.packet).unwrap().is_empty());
}

#[test]
fn champ_s3_body_primary_then_defer_then_destroy_secondaries_with_gc() {
    let mut f = Fixture::new();
    let owner = f.h.runtime.champ_new_owner().unwrap();
    let a = f.fresh(1, true, owner);
    let b = f.fresh(2, true, owner);
    let mut packet = f.join(vec![a, b]);
    let mut r = f.fresh(3, false, owner);
    let mut reserve = f.reserve(&mut r);
    let payload = f.h.runtime.heap.allocate_text("typed Error");
    let cause = f.h.runtime.failure_new(FailureKind::Program("cause".into()));
    let primary = f.h.runtime.failure_new(FailureKind::Thrown(payload));
    f.h.runtime.heap.payload_mut::<crate::runtime::Failure>(primary).unwrap().cause = Some(cause);
    let mut state = f.begin(&mut packet, Some(&mut reserve), primary, 0);
    let defer = f.h.runtime.failure_new(FailureKind::Panic("defer".into()));
    f.h.ctx.status = FAILED;
    f.h.ctx.failure = defer;
    let mut captured = 0;
    assert_eq!(rt_failure_capture(&mut f.h.ctx, &mut captured), NORMAL);
    assert_eq!(rt_cleanup_failure(&mut f.h.ctx, &mut state, captured, &mut state), NORMAL);
    f.h.gc(&[state.record]);
    f.plan(&mut state, true, false);
    let post = f.finish(state, false);
    assert_eq!(post.failure, primary);
    let failure = f.h.runtime.failure(primary).unwrap();
    assert!(matches!(failure.kind, FailureKind::Thrown(value) if value == payload));
    assert!(!failure.cleanup_origin);
    assert_eq!(failure.cause, Some(cause));
    assert_eq!(failure.secondary.len(), 3);
    assert_eq!(failure.secondary, vec![defer, f.failures[0].1, f.failures[1].1]);
    assert_eq!(f.failures.iter().map(|&(event, _)| event).collect::<Vec<_>>(), vec![2, 1]);
    for &secondary in &failure.secondary {
        assert!(f.h.runtime.failure(secondary).unwrap().cleanup_origin);
    }
    assert_eq!(f.trace(), "321");
    f.assert_once();
}

#[test]
fn champ_s3_model_reached_defer_prefix_and_failure_priority() {
    // spec16-4: constructor failure reaches no defer; the other three paths reach d then destroy R.
    for (creation_failure, body_failure, defer_failure, expected) in [
        (false, false, false, "91"),
        (false, false, true, "91"),
        (false, true, false, "91"),
        (false, true, true, "91"),
        (true, false, false, "0"),
    ] {
        let mut f = Fixture::new();
        let owner = f.h.runtime.champ_new_owner().unwrap();
        let mut packet = if creation_failure { f.h.empty() } else { f.fresh(1, false, owner) };
        let primary = if body_failure || creation_failure {
            f.h.runtime.failure_new(FailureKind::Panic("body".into()))
        } else {
            0
        };
        let mut state = f.begin(&mut packet, None, primary, 0);
        if !creation_failure {
            // Reached defer action in the explicit caller CFG, before cleanup.plan.
            f.h.runtime.heap.set_field(f.log, 0, word::small(9).unwrap());
            if defer_failure {
                let failure = f.h.runtime.failure_new(FailureKind::Panic("defer".into()));
                assert_eq!(rt_cleanup_failure(&mut f.h.ctx, &mut state, failure, &mut state), NORMAL);
            }
        }
        f.plan(&mut state, false, false);
        let post = f.finish(state, false);
        assert_eq!(f.trace(), expected);
        f.assert_once();
        if body_failure || creation_failure {
            assert_eq!(post.failure, primary);
            let failure = f.h.runtime.failure(primary).unwrap();
            assert_eq!(failure.secondary.len(), usize::from(defer_failure));
        } else if defer_failure {
            let failure = f.h.runtime.failure(post.failure).unwrap();
            assert!(failure.cleanup_origin);
            assert!(matches!(&failure.kind, FailureKind::Panic(text) if text == "defer"));
        }
    }
}

#[test]
fn champ_s3_b2_and_incomplete_settlement_are_rejected_without_consuming_inputs() {
    let mut f = Fixture::new();
    let owner = f.h.runtime.champ_new_owner().unwrap();
    let mut a = f.fresh(1, false, owner);
    let mut r = f.fresh(2, false, owner);
    let mut pending = f.reserve(&mut r);
    let mut state = f.begin(&mut a, Some(&mut pending), 0, 1);
    let direct = f.h.runtime.champ_register_cleanup_plan(CleanupPlan { reserve: true, direct: true, may_fail: true });
    let record = state.record;
    assert_eq!(rt_cleanup_plan(&mut f.h.ctx, &mut state, direct, &mut state), OUTCOME_STOP);
    assert_eq!(state.record, record);
    f.h.reset_fatal();
    assert_eq!(rt_cleanup_settle_empty(&mut f.h.ctx, &mut state, &mut state), OUTCOME_STOP);
    f.h.reset_fatal();
    let mut empty = f.h.empty();
    assert_eq!(rt_cleanup_settle_packet(&mut f.h.ctx, &mut state, &mut empty, 0, &mut state), NORMAL);
    assert_eq!(rt_cleanup_plan(&mut f.h.ctx, &mut state, direct, &mut state), OUTCOME_STOP);
    assert_eq!(state.record, record);
    f.h.reset_fatal();
    f.plan(&mut state, true, false);
    let mut next = NextPost::default();
    assert_eq!(rt_cleanup_next(&mut f.h.ctx, &mut state, &mut next), NORMAL);
    state = next.state;
    let mut finish = FinishPost::default();
    assert_eq!(rt_cleanup_finish(&mut f.h.ctx, &mut state, &mut finish), OUTCOME_STOP);
    f.h.reset_fatal();
    let mut empty = f.h.empty();
    let mut extra_pending = f.reserve(&mut empty);
    // Neither the selected identity nor the active body may be interrupted by reserve insertion.
    assert_eq!(rt_exit_abort(&mut f.h.ctx, &mut extra_pending, &mut state, &mut state), OUTCOME_STOP);
    assert_ne!(extra_pending.record, 0);
    f.h.reset_fatal();
    let mut post = DestroyPost::default();
    f.h.ctx.entry_frame = frame_pointer!();
    assert_eq!(rt_cleanup_destroy(&mut f.h.ctx, &mut state, next.key, &mut post), NORMAL);
    state = post.state;
    f.pin_dead_checks();
    let output = f.finish(state, false);
    assert_eq!(output.failure, 0);
    assert_ne!(output.pending.record, 0);
}

extern "C" fn overflow_adapter(ctx: *mut Ctx, io: *mut CallOwn, _: *const Word, _: u64) -> Outcome {
    // This is the owning-entry pre-body overflow block that #573-P1 will emit in machine code.
    rt_entry_reject(ctx, io)
}
#[test]
fn champ_s3_fresh_argument_overflow_runs_destroy_bodies_once_and_preserves_overflow_primary() {
    let mut f = Fixture::new();
    let owner = f.h.runtime.champ_new_owner().unwrap();
    let a = f.fresh(1, true, owner);
    let b = f.fresh(2, false, owner);
    let packet = f.join(vec![a, b]);
    let entry = f.h.runtime.champ_register_entry(EntryContract {
        invoke: overflow_adapter, arguments: vec![Repr::Ref], replacements: 0, summary: 0,
        cleanup_mode: CLEANUP_ORDINARY,
    });
    // Diagnostic roots retain dead tails only for post-return checks. A separate executor test
    // exercises the live suffix/reserve/failure graph with no diagnostic live aliases.
    for &(object, _) in &f.objects {
        f.h.runtime.roots.push((object, Repr::Ref));
    }
    let args = [f.objects[0].0];
    let mut post = CallPost::default();
    let mut io = CallOwn { input: packet, entry_abi: entry, output: &mut post, context: 0, cleanup_mode: 0 };
    let original_limit = frame_pointer!();
    f.h.ctx.entry_frame = frame_pointer!();
    // The ordinary entry would reject here. Rejection lowers it only within reserved headroom.
    f.h.ctx.stack_limit = original_limit;
    assert_eq!(rt_call_owned(&mut f.h.ctx, entry, args.as_ptr(), 1, &mut io, &mut post), UNWIND);
    assert_eq!(f.h.ctx.stack_limit, original_limit);
    assert_eq!(f.h.ctx.status, FAILED);
    assert_eq!(f.h.ctx.failure, post.failure);
    assert_eq!(io.input.record, 0);
    assert_eq!(post.packet.record, 0);
    assert_eq!(f.trace(), "21");
    f.assert_once();
    let failure = f.h.runtime.failure(post.failure).unwrap();
    assert!(matches!(&failure.kind, FailureKind::Program(text) if text == "Stack overflow"));
    assert_eq!(failure.secondary.len(), 1);
    assert!(f.h.runtime.failure(failure.secondary[0]).unwrap().cleanup_origin);
}

extern "C" fn opaque_cleanup_adapter(ctx: *mut Ctx, io: *mut CallOwn, _: *const Word, _: u64) -> Outcome {
    super::p1::opaque_cleanup(ctx, io)
}
fn opaque_cleanup_entry(f: &mut Fixture) -> EntryId {
    f.h.runtime.champ_register_entry(EntryContract {
        invoke: opaque_cleanup_adapter, arguments: vec![Repr::Ref], replacements: 0, summary: 0,
        cleanup_mode: CLEANUP_ORDINARY,
    })
}
/// The opaque cleanup intrinsic releases the complete owner graph it is given: each identity of its packet is
/// destroyed once, in descending serial order, and the result packet is empty.
#[test]
fn champ_s3_opaque_cleanup_destroys_the_graph_once_and_returns_an_empty_packet() {
    let mut f = Fixture::new();
    let owner = f.h.runtime.champ_new_owner().unwrap();
    let a = f.fresh(1, false, owner);
    let b = f.fresh(2, false, owner);
    let packet = f.join(vec![a, b]);
    let entry = opaque_cleanup_entry(&mut f);
    for &(object, _) in &f.objects {
        f.h.runtime.roots.push((object, Repr::Ref));
    }
    let args = [f.log];
    let mut post = CallPost::default();
    let mut io = CallOwn { input: packet, entry_abi: entry, output: &mut post, context: 0, cleanup_mode: 0 };
    f.h.ctx.entry_frame = frame_pointer!();
    assert_eq!(rt_call_owned(&mut f.h.ctx, entry, args.as_ptr(), 1, &mut io, &mut post), NORMAL);
    assert_eq!(f.h.ctx.status, OK);
    assert_eq!(io.input.record, 0);
    assert!(f.h.runtime.champ_packet_keys(&post.packet).unwrap().is_empty());
    assert_eq!(f.trace(), "21");
    f.assert_once();
}

/// Its contract has no failure: a destroy that fails stops the machine instead of unwinding.
#[test]
fn champ_s3_opaque_cleanup_stops_on_a_failing_destroy() {
    let mut f = Fixture::new();
    let owner = f.h.runtime.champ_new_owner().unwrap();
    let packet = f.fresh(1, true, owner);
    let entry = opaque_cleanup_entry(&mut f);
    for &(object, _) in &f.objects {
        f.h.runtime.roots.push((object, Repr::Ref));
    }
    let args = [f.log];
    let mut post = CallPost::default();
    let mut io = CallOwn { input: packet, entry_abi: entry, output: &mut post, context: 0, cleanup_mode: 0 };
    f.h.ctx.entry_frame = frame_pointer!();
    assert_eq!(rt_call_owned(&mut f.h.ctx, entry, args.as_ptr(), 1, &mut io, &mut post), OUTCOME_STOP);
    assert_ne!(f.h.ctx.status, OK);
    assert_eq!(f.h.identity(f.objects[0].0).life, Life::Dead);
    f.h.reset_fatal();
}

#[test]
fn champ_s3_journal_head_tail_drains_have_linear_storage_work() {
    for tail in [false, true] {
        for count in [64, 256, 1024] {
            let mut h = Harness::new();
            h.runtime.heap.set_stress(false);
            let holder = h.runtime.champ_new_holder().unwrap();
            let owner = h.runtime.champ_new_owner().unwrap();
            let mut entries = Vec::new();
            for _ in 0..count {
                let object = h.runtime.heap.object_new(0, &[word::small(7).unwrap()]);
                h.runtime.champ_create_commit(object, holder, owner).unwrap();
                entries.push(StaticEntry { object, active: 1 });
            }
            let mut packet_slot = h.runtime.champ_packet_from_static(&mut entries, holder, owner).unwrap();
            let original = packet_slot.record;
            let plans: Vec<_> = (1..=count).rev().map(|remaining| {
                h.runtime.champ_register_selection(vec![if tail { remaining - 1 } else { 0 }])
            }).collect();
            super::records::RUN_WORK.with(|work| work.set(0));
            let mut selected = Vec::new();
            for (offset, plan) in plans.into_iter().enumerate() {
                let mut post = SplitPost::default();
                assert_eq!(rt_own_split_static(&mut h.ctx, &mut packet_slot, plan, &mut post), NORMAL);
                packet_slot = post.rest;
                selected.push(post.selected);
                assert_eq!(packet_slot.record, original, "suffix storage must stay in place");
                let Data::Packet(packet) = data(&h.runtime, packet_slot.record).unwrap() else { panic!("packet") };
                let remaining = count - offset - 1;
                if remaining == 0 {
                    assert!(packet.journals[0].is_empty());
                } else {
                    let run = &packet.journals[0][0];
                    assert_eq!(run.length, remaining);
                    assert!(run.chunks.iter().all(|chunk| !chunk.is_empty()));
                }
            }
            let work = super::records::RUN_WORK.with(|work| work.get());
            assert!(work <= 8 * count, "{tail}: {count} entries required {work} word visits/copies/pops");
            assert_eq!(packet(&h.runtime, packet_slot.record).unwrap().words().count(), 0);
            h.gc(&selected.iter().map(|slot| slot.record).collect::<Vec<_>>());
        }
    }
}

#[test]
fn champ_s3_ordered_executor_drain_never_copies_or_rescans_the_suffix() {
    let mut h = Harness::new();
    h.runtime.heap.set_stress(false);
    let holder = h.runtime.champ_new_holder().unwrap();
    let owner = h.runtime.champ_new_owner().unwrap();
    let mut entries = Vec::new();
    for _ in 0..512 {
        let object = h.runtime.heap.object_new(0, &[word::small(7).unwrap()]);
        h.runtime.champ_create_commit(object, holder, owner).unwrap();
        entries.push(StaticEntry { object, active: 1 });
    }
    let mut packet = h.runtime.champ_packet_from_static(&mut entries, holder, owner).unwrap();
    let mut state = h.runtime.champ_cleanup_from_packet(&mut packet).unwrap();
    let plan = h.runtime.champ_register_cleanup_plan(CleanupPlan { reserve: false, direct: false, may_fail: false });
    assert_eq!(rt_cleanup_plan(&mut h.ctx, &mut state, plan, &mut state), NORMAL);
    h.gc(&[state.record]);
    h.ctx.entry_frame = frame_pointer!();
    super::records::RUN_WORK.with(|work| work.set(0));
    let mut destroyed = 0;
    loop {
        let mut next = NextPost::default();
        let outcome = rt_cleanup_next(&mut h.ctx, &mut state, &mut next);
        state = next.state;
        if outcome == 1 {
            break;
        }
        assert_eq!(outcome, NORMAL);
        let mut post = DestroyPost::default();
        assert_eq!(rt_cleanup_destroy(&mut h.ctx, &mut state, next.key, &mut post), NORMAL);
        state = post.state;
        assert_eq!(post.failure, 0);
        destroyed += 1;
    }
    let work = super::records::RUN_WORK.with(|work| work.get());
    assert_eq!(destroyed, 512);
    assert!(work <= 2 * destroyed, "suffix work {work} for {destroyed} destroys");
    let mut post = FinishPost::default();
    assert_eq!(rt_cleanup_finish(&mut h.ctx, &mut state, &mut post), NORMAL);
}

#[test]
fn champ_s3_explicit_abort_before_selection_or_after_dead_prefix_uses_the_same_transition() {
    for after_prefix in [false, true] {
        let mut f = Fixture::new();
        let owner = f.h.runtime.champ_new_owner().unwrap();
        let a = f.fresh(1, false, owner);
        let mut r = f.fresh(2, false, owner);
        let mut pending = f.reserve(&mut r);
        let mut packets = vec![a];
        if after_prefix {
            packets.push(f.fresh(3, true, owner));
        }
        let mut packet = f.join(packets);
        let mut state = f.begin(&mut packet, None, 0, 0);
        // pending is still external to this activation until the explicit exit.abort.
        f.h.runtime.roots.push((pending.record, Repr::Ref));
        if after_prefix {
            f.plan(&mut state, false, false);
            assert!(f.step(&mut state));
            assert_eq!(f.trace(), "3");
        } else {
            let failure = f.h.runtime.failure_new(FailureKind::Panic("defer".into()));
            assert_eq!(rt_cleanup_failure(&mut f.h.ctx, &mut state, failure, &mut state), NORMAL);
        }
        assert_eq!(rt_exit_abort(&mut f.h.ctx, &mut pending, &mut state, &mut state), NORMAL);
        assert_eq!(pending.record, 0);
        assert_eq!(rt_exit_abort(&mut f.h.ctx, &mut pending, &mut state, &mut state), OUTCOME_STOP);
        f.h.reset_fatal();
        if !after_prefix {
            f.plan(&mut state, true, false);
        }
        let post = f.finish(state, false);
        assert_ne!(post.failure, 0);
        assert_eq!(post.pending.record, 0);
        assert_eq!(f.trace(), if after_prefix { "321" } else { "21" });
        f.assert_once();
    }
}

#[test]
fn champ_s3_scope_journal_checked_split_and_transfer_have_linear_endpoint_drains() {
    for (transfer, many_runs) in [(false, false), (true, false), (false, true), (true, true)] {
        for tail in [false, true] {
            let mut h = Harness::new();
            h.runtime.heap.set_stress(false);
            let mut source = h.scope();
            let mut target = h.scope();
            let info = scope(&h.runtime, source.record).unwrap();
            let mut objects = Vec::new();
            let mut entries = Vec::new();
            for _ in 0..256 {
                let object = h.runtime.heap.object_new(0, &[word::small(7).unwrap()]);
                h.runtime.champ_create_commit(object, info.holder, info.owner).unwrap();
                objects.push(object);
                entries.push(StaticEntry { object, active: 1 });
            }
            if many_runs {
                for entry in &mut entries {
                    let object = entry.object;
                    let mut packet = h.runtime.champ_packet_from_static(
                        std::slice::from_mut(entry), info.holder, info.owner,
                    ).unwrap();
                    assert_eq!(rt_own_rebind(&mut h.ctx, &mut source, 0, object, &mut packet, &mut source), NORMAL);
                }
            } else {
                let mut packet = h.runtime.champ_packet_from_static(&mut entries, info.holder, info.owner).unwrap();
                assert_eq!(rt_own_accept(&mut h.ctx, &mut source, &mut packet, &mut source), NORMAL);
            }
            let original = source.record;
            // Serial runs are descending; the older creation is the tail.
            if !tail {
                objects.reverse();
            }
            let mut outputs = Vec::new();
            super::records::RUN_WORK.with(|work| work.set(0));
            for object in objects {
                let selection = Selection { roots: &object, count: 1 };
                if transfer {
                    let inputs = TransferInputs { from: &mut source, to: &mut target };
                    let mut post = TransferPost::default();
                    assert_eq!(rt_scope_transfer(&mut h.ctx, &inputs, &selection, &mut post), NORMAL);
                    source = post.from;
                    target = post.to;
                } else {
                    let mut post = ScopeSplitPost::default();
                    assert_eq!(rt_scope_split(&mut h.ctx, &mut source, &selection, &mut post), NORMAL);
                    source = post.state;
                    outputs.push(post.selected);
                }
                assert_eq!(source.record, original);
            }
            let work = super::records::RUN_WORK.with(|work| work.get());
            assert!(work <= 8 * 256, "transfer={transfer}, tail={tail}: {work}");
            let mut roots: Vec<_> = outputs.iter().map(|packet| packet.record).collect();
            roots.extend([source.record, target.record]);
            h.gc(&roots);
            assert_eq!(scope(&h.runtime, source.record).unwrap().all().words().count(), 0);
            if transfer {
                assert_eq!(scope(&h.runtime, target.record).unwrap().bound.words().count(), 256);
            }
        }
    }
}

#[test]
fn champ_s3_nonthrowing_direct_forwards_reserve_until_the_callers_packet_close() {
    let mut f = Fixture::new();
    let owner = f.h.runtime.champ_new_owner().unwrap();
    let mut a = f.fresh(1, false, owner);
    let mut r = f.fresh(2, false, owner);
    let mut pending = f.reserve(&mut r);
    let mut state = f.begin(&mut a, Some(&mut pending), 0, 0);
    let plan = f.h.runtime.champ_register_cleanup_plan(CleanupPlan { reserve: true, direct: true, may_fail: false });
    assert_eq!(rt_cleanup_plan(&mut f.h.ctx, &mut state, plan, &mut state), NORMAL);
    let object = f.objects[0].0;
    let key = KeyRef { object, serial: f.h.identity(object).serial };
    f.destroy(&mut state, key, true);
    let mut post = FinishPost::default();
    assert_eq!(rt_cleanup_finish(&mut f.h.ctx, &mut state, &mut post), NORMAL);
    assert_eq!(post.failure, 0);
    assert_ne!(post.pending.record, 0);
    assert_eq!(f.trace(), "1");
    let resource = f.objects[1].0;
    let identity = f.h.identity(resource);
    assert_eq!(identity.life, Life::Live);
    let destination = ExitDestination { holder: HolderId(identity.holder_id), owner };
    let mut committed = CommitPost::default();
    assert_eq!(rt_exit_commit(&mut f.h.ctx, &mut post.pending, &destination, &mut committed), NORMAL);
    let mut caller_close = f.begin(&mut committed.packet, None, 0, 0);
    f.plan(&mut caller_close, false, false);
    let post = f.finish(caller_close, false);
    assert_eq!(post.failure, 0);
    assert_eq!(f.trace(), "12");
    f.assert_once();
}

#[test]
fn champ_s3_spb_plan_relocation_is_nonzero_runtime_local_and_checked_before_mutation() {
    use okkama_sparrow::spb17 as s;
    struct Catalog<'a>(&'a s::Program);
    impl s::ExternalContracts for Catalog<'_> {
        fn entry(&self, name: &str) -> Option<&s::Function> {
            self.0.functions.iter().find(|function| function.proofs.entry.as_ref()
                .and_then(|entry| entry.external.as_deref()) == Some(name))
        }
    }
    let program = s::assemble(include_str!(
        "../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/unwind-reserve.spb17"
    )).unwrap();
    let mut harness = Harness::new();
    let catalog = Catalog(&program);
    let first = harness.runtime.champ_load_cleanup_plans(&program, &catalog).unwrap();
    let second = harness.runtime.champ_load_cleanup_plans(&program, &catalog).unwrap();
    let first = first.into_iter().flatten().collect::<Vec<_>>();
    let second = second.into_iter().flatten().collect::<Vec<_>>();
    assert!(!first.is_empty());
    assert!(first.iter().all(|&id| id != 0));
    assert!(second.iter().all(|id| *id > *first.last().unwrap()));
    let mut invalid = program.clone();
    invalid.functions.iter_mut().find(|function| function.symbol == "main").unwrap().proofs.roots.clear();
    let before = harness.runtime.champ.cleanup_plans.len();
    assert!(harness.runtime.champ_load_cleanup_plans(&invalid, &catalog).is_err());
    assert_eq!(harness.runtime.champ.cleanup_plans.len(), before);
}
