//! Bounded Rust witnesses for the S2 ABI; no source compiler pipeline or external programs.
use super::*;
use super::cleanup::*;
use super::records::Data;
use super::helpers::{cleanup as read_cleanup, data, scope};
use crate::runtime::{frame_pointer, Ctx, FailureKind, FAILED, FATAL, OK};
use crate::word::{self, Repr, Word};
use okkama_sparrow::{Constant, Dst, FuncRef, Function, Instruction as I, ObjectDecl, ObjectRef};
use okkama_sparrow::{Program, Register as R, Type, TypeRef};
use std::collections::HashSet;

pub(super) struct Harness {
    pub(super) runtime: Box<Runtime>,
    pub(super) ctx: Ctx,
}
impl Harness {
    pub(super) fn new() -> Self {
        let mut program = Program {
            types: vec![Type::Int, Type::Str, Type::Object(ObjectRef(0)), Type::Object(ObjectRef(1)), Type::Byte],
            ..Program::default()
        };
        for (index, name) in ["Good", "Bad"].iter().enumerate() {
            program.objects.push(ObjectDecl {
                name: (*name).into(),
                fields: vec![("checkpoint".into(), TypeRef(0)), ("child".into(), TypeRef(2))],
                does: vec![],
                destroy: Some(FuncRef(index as u32)), builtin: false,
            });
            let code = if index == 0 {
                vec![
                    I::ObjGet { dst: Dst(R(1)), object: R(0), field: 0 },
                    I::ByteFrom { dst: Dst(R(2)), source: R(1), context: None },
                    I::Const { dst: Dst(R(3)), value: Constant::Int("9".into()) },
                    I::ObjInit { object: R(0), field: 0, value: R(3) },
                    I::Return { value: None },
                ]
            } else {
                vec![
                    I::Const { dst: Dst(R(4)), value: Constant::Str("destroy failed".into()) },
                    I::Panic { message: R(4) },
                ]
            };
            program.functions.push(Function {
                symbol: format!("{name}.destroy"), is_async: false, native: None, intrinsic: None,
                registers: vec![TypeRef(2 + index as u32), TypeRef(0), TypeRef(4), TypeRef(0), TypeRef(1)],
                params: vec![R(0)], result: None, code, handlers: vec![],
            });
        }
        let mut runtime = Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap());
        runtime.heap.set_stress(true);
        let ctx = Ctx {
            status: OK, stack_limit: 0, failure: 0, runtime: &mut *runtime, entry_frame: 0, scratch: 0,
        };
        Self { runtime, ctx }
    }
    pub(super) fn fresh(&mut self, descriptor: u64, owner: OwnerId) -> (Word, PacketSlot) {
        let object = self.runtime.heap.object_new(descriptor as u32, &[word::small(7).unwrap()]);
        let mut slot = PacketSlot::default();
        assert_eq!(rt_own_create_commit(&mut self.ctx, object, descriptor, owner.get(), &mut slot), NORMAL);
        (object, slot)
    }
    pub(super) fn scope(&mut self) -> ScopeSlot {
        let holder = self.runtime.champ_new_holder().unwrap();
        let owner = self.runtime.champ_new_owner().unwrap();
        self.runtime.champ_register_value_repr(owner, 0, false, Repr::Ref);
        self.runtime.champ_register_value_repr(owner, 0, true, Repr::Ref);
        let instance = ScopeInstance { holder, owner, parent_owner: 0 };
        let mut slot = ScopeSlot::default();
        assert_eq!(rt_scope_begin(&mut self.ctx, &instance, &mut slot), NORMAL);
        slot
    }
    pub(super) fn empty(&mut self) -> PacketSlot {
        let mut packet = PacketSlot::default();
        assert_eq!(rt_own_empty(&mut self.ctx, &mut packet), NORMAL);
        packet
    }
    pub(super) fn identity(&self, object: Word) -> Identity {
        self.runtime.heap.champ_identity(object).unwrap()
    }
    pub(super) fn gc(&mut self, records: &[Word]) {
        let roots = &self.runtime.roots;
        let constants = &self.runtime.constants;
        let failure = self.ctx.failure;
        self.runtime.heap.collect(|visit| {
            for &(word, repr) in roots.iter().chain(constants) {
                visit(word, repr);
            }
            visit(failure, Repr::Ref);
            for &record in records {
                visit(record, Repr::Ref);
            }
        });
    }
    pub(super) fn reset_fatal(&mut self) {
        self.ctx.status = OK;
        self.ctx.failure = 0;
        self.runtime.fatal = None;
    }
}
fn words(runtime: &Runtime, packet: &PacketSlot) -> HashSet<Word> {
    runtime.champ_packet_keys(packet).unwrap().into_iter().map(KeyRef::object).collect()
}
fn selection(roots: &[Word]) -> Selection {
    Selection { roots: roots.as_ptr(), count: roots.len() as u64 }
}
fn changed(before: Identity, after: Identity, holder: u64, owner: u64) {
    assert_eq!(after, Identity { holder_id: holder, owner_id: owner, epoch: before.epoch + 1, ..before });
}

/// The result context a registry check needs: `fut.new`'s schema names its Future type (#601 V8).
pub(super) fn schema_context(opcode: Option<okkama_sparrow::spb17::Opcode>) -> okkama_sparrow::spb17::ResultContext {
    use okkama_sparrow::spb17::{Opcode, ResultContext, Type};
    match opcode {
        Some(Opcode::FutureNew) => ResultContext { value: Some(Type::Future(0)), ..ResultContext::default() },
        _ => ResultContext::default(),
    }
}

#[test]
fn champ_s2_registry_layouts_effects_and_single_outcome_channel() {
    assert_eq!(size_of::<CallPost>(), 24);
    assert_eq!(size_of::<DestroyBorrow>(), 40);
    assert_eq!(size_of::<KeyRef>(), 16);
    assert_eq!(size_of::<PacketSlot>(), 8);
    let helpers: Vec<_> = crate::jit::HELPERS.iter().filter(|(name, _)| name.starts_with("champ_")).collect();
    // #622 adds champ_machine_collection, champ_machine_builder and champ_machine_lookup; #675 adds
    // champ_machine_scalar; #676 adds champ_machine_role_narrow and champ_own_dynamic; #601 V8 adds the eight
    // async helpers of Champ draft 3 §14.4 and champ_machine_future_destination; #601 V9 adds champ_body_reject.
    assert_eq!(helpers.len(), 72);
    for &(name, effects) in helpers {
        let abi = helper_abi(name).unwrap();
        let class = abi.class;
        assert_eq!(abi.effects, effects);
        let context = schema_context(abi.opcode);
        let schemas = abi.schemas(&context).unwrap();
        if let Some(opcode) = abi.opcode {
            for (tag, slots) in schemas {
                assert_eq!(slots, opcode.results(tag, &context).unwrap());
                let offsets = abi.post_offsets(tag, &context).unwrap();
                assert_eq!(offsets.len(), slots.len());
                for offset in offsets.into_iter().flatten() {
                    assert!(offset < abi.output.unwrap().size);
                }
            }
        }
        // Draft 3 §1.1: only class S may return PENDING, and an S helper is otherwise an X helper.
        assert_eq!(effects.contains(&crate::jit::Effect::Suspends), class == HelperClass::S, "{name}");
        if class == HelperClass::S {
            for effect in [crate::jit::Effect::Collects, crate::jit::Effect::Reenters] {
                assert!(effects.contains(&effect), "{name}: {effect:?}");
            }
        }
        if class == HelperClass::L {
            for effect in [crate::jit::Effect::Collects, crate::jit::Effect::Reenters, crate::jit::Effect::Fails] {
                assert!(!effects.contains(&effect), "{name}: {effect:?}");
            }
        }
    }
    for record in RECORD_LAYOUTS {
        assert_eq!(record.align, 8, "{}", record.name);
        for &offset in record.traced_offsets.iter().chain(record.schema_offsets) {
            assert!(offset < record.size && offset % 8 == 0, "{}", record.name);
        }
    }
    let split = helper_abi("champ_own_split_checked").unwrap();
    let context = okkama_sparrow::spb17::ResultContext::default();
    assert_eq!(split.post_offsets(okkama_sparrow::spb17::Tag::Normal, &context).unwrap(),
        vec![Some(0), Some(8), None]);
    assert_eq!(split.post_offsets(okkama_sparrow::spb17::Tag::Unwind, &context).unwrap(),
        vec![Some(0), Some(16)]);
    // Runtime construction imports every helper with its checked CLIF signature.
    let _ = Harness::new();
}

#[test]
fn champ_s2_write_table_empty_move_join_admit_bind_rebind_stage() {
    let mut h = Harness::new();
    let mut state = h.scope();
    let owner = scope(&h.runtime, state.record).unwrap().owner;
    let (a, mut a_packet) = h.fresh(0, owner);
    let before = h.identity(a);
    h.gc(&[state.record, a_packet.record]);
    let mut moved = PacketSlot::default();
    assert_eq!(rt_own_move(&mut h.ctx, &mut a_packet, &mut moved), NORMAL);
    assert_eq!(h.identity(a), before);
    let (b, b_packet) = h.fresh(0, owner);
    let before_b = h.identity(b);
    let mut inputs = [moved, b_packet, h.empty()];
    let mut joined = PacketSlot::default();
    assert_eq!(rt_own_join(&mut h.ctx, inputs.as_mut_ptr(), 3, &mut joined), NORMAL);
    assert_eq!(h.identity(a), before);
    assert_eq!(h.identity(b), before_b);
    assert!(inputs.iter().all(|input| input.record == 0));
    h.gc(&[state.record, joined.record]);
    assert_eq!(rt_own_admit(&mut h.ctx, &mut state, &mut joined, &mut state), NORMAL);
    let admitted = h.identity(a);
    let state_data = scope(&h.runtime, state.record).unwrap();
    changed(before, admitted, state_data.holder.get(), owner.get());
    let mut empty = h.empty();
    assert_eq!(rt_own_bind(&mut h.ctx, &mut state, 0, a, &mut empty, &mut state), NORMAL);
    let state_data = scope(&h.runtime, state.record).unwrap();
    assert_eq!(state_data.bound.words().collect::<Vec<_>>(), vec![a]);
    assert_eq!(state_data.admission.words().collect::<Vec<_>>(), vec![b]);
    assert_eq!(h.identity(a), admitted);
    let mut empty = h.empty();
    assert_eq!(rt_own_rebind(&mut h.ctx, &mut state, 0, b, &mut empty, &mut state), NORMAL);
    assert_eq!(h.identity(a), admitted);
    let state_data = scope(&h.runtime, state.record).unwrap();
    assert_eq!(state_data.bound.words().collect::<HashSet<_>>(), HashSet::from([a, b]));
    assert_eq!(state_data.bindings[0].1, b);
    let mut empty = h.empty();
    let inputs = StageInputs {
        state: &mut state, source: std::ptr::null_mut(), selection: 0, field: 0, value: b, packet: &mut empty };
    let before = h.identity(b);
    let mut post = StagePost::default();
    assert_eq!(rt_own_stage(&mut h.ctx, &inputs, &mut post), NORMAL);
    state = post.state;
    assert_eq!(h.identity(b), before);
    h.gc(&[state.record]);
    let (c, mut offered) = h.fresh(0, owner);
    let before = h.identity(c);
    assert_eq!(rt_own_rebind(&mut h.ctx, &mut state, 0, c, &mut offered, &mut state), NORMAL);
    changed(before, h.identity(c), state_data.holder.get(), owner.get());
    assert_eq!(scope(&h.runtime, state.record).unwrap().bound.words().count(), 3);
}

#[test]
fn champ_s2_splits_disjoint_rest_selected_and_failure_preserves_original() {
    let mut h = Harness::new();
    let owner = h.runtime.champ_new_owner().unwrap();
    let (a, p) = h.fresh(0, owner);
    let (b, q) = h.fresh(0, owner);
    let mut inputs = [p, q];
    let mut joined = PacketSlot::default();
    assert_eq!(rt_own_join(&mut h.ctx, inputs.as_mut_ptr(), 2, &mut joined), NORMAL);
    let old_a = h.identity(a);
    let old_b = h.identity(b);
    let plan = h.runtime.champ_register_selection(vec![0]);
    let mut split = SplitPost::default();
    assert_eq!(rt_own_split_static(&mut h.ctx, &mut joined, plan, &mut split), NORMAL);
    assert_eq!(words(&h.runtime, &split.rest), HashSet::from([b]));
    assert_eq!(words(&h.runtime, &split.selected), HashSet::from([a]));
    assert_eq!(h.identity(b), old_b);
    assert_eq!(h.identity(a).epoch, old_a.epoch + 1);
    h.gc(&[split.rest.record, split.selected.record]);
    let before = h.identity(a);
    let mut failed = SplitPost::default();
    let roots = [b];
    let original = split.selected.record;
    assert_eq!(rt_own_split_checked(&mut h.ctx, &mut split.selected, &selection(&roots), &mut failed), UNWIND);
    assert_eq!(failed.rest.record, original);
    assert_eq!(failed.selected.record, 0);
    assert_eq!(h.identity(a), before);
    assert_eq!(h.ctx.status, FAILED);
    assert_eq!(h.ctx.failure, failed.failure);
    let mut captured = 0;
    assert_eq!(rt_failure_capture(&mut h.ctx, &mut captured), NORMAL);
    assert_eq!(h.ctx.status, OK);
    assert_eq!(h.ctx.failure, 0);
    let mut split_again = SplitPost::default();
    let roots = [a, a];
    assert_eq!(rt_own_split_checked(&mut h.ctx, &mut failed.rest, &selection(&roots), &mut split_again), NORMAL);
    assert!(words(&h.runtime, &split_again.rest).is_empty());
    assert_eq!(words(&h.runtime, &split_again.selected), HashSet::from([a]));
    h.gc(&[split.rest.record, split_again.rest.record, split_again.selected.record]);
}

/// `own.dynamic`: a packet of unknown retention joins the scope inventory (its identities take the scope's
/// holder and owner); a packet that fails the checks (an identity already in the scope) comes back unaccepted
/// with the state, both unchanged, and a program failure.
#[test]
fn champ_s2_dynamic_transfer_joins_the_scope_or_returns_the_packet() {
    let mut h = Harness::new();
    let mut state = h.scope();
    let target = scope(&h.runtime, state.record).unwrap();
    let foreign = h.runtime.champ_new_owner().unwrap();
    let (a, mut packet) = h.fresh(0, foreign);
    let before = h.identity(a);
    let mut post = ScopeSplitPost::default();
    assert_eq!(rt_own_dynamic(&mut h.ctx, &mut state, &mut packet, &mut post), NORMAL);
    assert_eq!((state.record, packet.record, post.selected.record, post.failure), (0, 0, 0, 0));
    changed(before, h.identity(a), target.holder.get(), target.owner.get());
    let joined = scope(&h.runtime, post.state.record).unwrap();
    assert_eq!(joined.admission.words().collect::<Vec<_>>(), vec![a]);
    h.gc(&[post.state.record]);
    // The same identity again: it is already in the scope's inventory, so the check fails.
    let mut state = ScopeSlot { record: post.state.record };
    let mut again = PacketSlot::default();
    let run = super::helpers::run(&h.runtime, vec![a], target.holder, target.owner);
    again.record = super::helpers::store(&mut h.runtime, Data::Packet(run));
    let (original, unaccepted) = (state.record, again.record);
    let held = h.identity(a);
    let mut failed = ScopeSplitPost::default();
    assert_eq!(rt_own_dynamic(&mut h.ctx, &mut state, &mut again, &mut failed), UNWIND);
    assert_eq!((failed.state.record, failed.selected.record), (original, unaccepted));
    assert_eq!((state.record, again.record), (0, 0));
    assert_eq!(h.identity(a), held);
    assert_eq!(h.ctx.status, FAILED);
    assert_eq!(h.ctx.failure, failed.failure);
    assert!(matches!(h.runtime.failure(failed.failure).map(|failure| &failure.kind), Some(FailureKind::Program(_))));
    h.gc(&[failed.state.record, failed.selected.record]);
}

#[test]
fn champ_s2_transfer_roundtrip_same_compaction_and_precommit_overflow() {
    let mut h = Harness::new();
    let mut from = h.scope();
    let mut to = h.scope();
    let from_scope = scope(&h.runtime, from.record).unwrap();
    let to_scope = scope(&h.runtime, to.record).unwrap();
    let (a, mut packet) = h.fresh(0, from_scope.owner);
    assert_eq!(rt_own_accept(&mut h.ctx, &mut from, &mut packet, &mut from), NORMAL);
    let before = h.identity(a);
    let roots = [a];
    let mut post = TransferPost::default();
    let same = TransferInputs { from: &mut from, to: &mut from };
    assert_eq!(rt_scope_transfer(&mut h.ctx, &same, &selection(&roots), &mut post), NORMAL);
    from = std::mem::take(&mut post.from);
    assert_eq!(post.to.record, 0);
    assert_eq!(h.identity(a), before);
    let inputs = TransferInputs { from: &mut from, to: &mut to };
    assert_eq!(rt_scope_transfer(&mut h.ctx, &inputs, &selection(&roots), &mut post), NORMAL);
    from = std::mem::take(&mut post.from);
    to = std::mem::take(&mut post.to);
    changed(before, h.identity(a), to_scope.holder.get(), to_scope.owner.get());
    h.gc(&[from.record, to.record]);
    let inputs = TransferInputs { from: &mut to, to: &mut from };
    assert_eq!(rt_scope_transfer(&mut h.ctx, &inputs, &selection(&roots), &mut post), NORMAL);
    to = std::mem::take(&mut post.from);
    from = std::mem::take(&mut post.to);
    assert_eq!(scope(&h.runtime, from.record).unwrap().all().words().count(), 1);
    assert_eq!(scope(&h.runtime, to.record).unwrap().all().words().count(), 0);
    assert_eq!(h.identity(a), Identity { epoch: before.epoch + 2, ..before });
    let mut maxed = h.identity(a);
    maxed.epoch = u64::MAX;
    h.runtime.heap.identity_write(a, maxed);
    let old_from = from.record;
    let old_to = to.record;
    let inputs = TransferInputs { from: &mut from, to: &mut to };
    assert_eq!(rt_scope_transfer(&mut h.ctx, &inputs, &selection(&roots), &mut post), UNWIND);
    assert_eq!((post.from.record, post.to.record), (old_from, old_to));
    assert_eq!(h.identity(a), maxed);
    h.gc(&[post.from.record, post.to.record]);
}

#[test]
fn champ_s2_chunked_rebind_history_is_rooted_without_language_references() {
    let mut h = Harness::new();
    let mut state = h.scope();
    let info = scope(&h.runtime, state.record).unwrap();
    let mut objects = Vec::new();
    for _ in 0..70 {
        let (object, mut packet) = h.fresh(0, info.owner);
        assert_eq!(rt_own_rebind(&mut h.ctx, &mut state, 0, object, &mut packet, &mut state), NORMAL);
        objects.push(object);
        h.gc(&[state.record]);
    }
    let state_data = scope(&h.runtime, state.record).unwrap();
    assert_eq!(state_data.bound.words().count(), 70);
    for run in state_data.bound.runs() {
        assert!(run.chunks.iter().all(|chunk| chunk.len() <= CHUNK_ENTRIES));
    }
    assert_eq!(state_data.bindings.len(), 1);
    assert_eq!(state_data.bindings[0].1, objects[69]);
    for object in objects {
        assert_eq!(h.identity(object).life, Life::Live);
        assert_eq!(h.identity(object).epoch, 2);
        assert_eq!(h.runtime.heap.header_of(object) & (7 << 18), 0);
    }
    // Scalar words obey their declared trace representation, including pointer-looking raw bits.
    h.runtime.champ_register_value_repr(info.owner, 1, false, Repr::Raw);
    let mut empty = h.empty();
    assert_eq!(rt_own_bind(&mut h.ctx, &mut state, 1, 0x1000, &mut empty, &mut state), NORMAL);
    h.gc(&[state.record]);
}

#[test]
fn champ_s2_per_scope_reserve_extend_replace_restoration_commit_and_abort() {
    let mut h = Harness::new();
    let mut inner = h.scope();
    let mut outer = h.scope();
    let inner_info = scope(&h.runtime, inner.record).unwrap();
    let outer_info = scope(&h.runtime, outer.record).unwrap();
    let (a, mut p) = h.fresh(0, inner_info.owner);
    let (b, mut q) = h.fresh(0, outer_info.owner);
    assert_eq!(rt_own_accept(&mut h.ctx, &mut inner, &mut p, &mut inner), NORMAL);
    assert_eq!(rt_own_accept(&mut h.ctx, &mut outer, &mut q, &mut outer), NORMAL);
    let before_outer = h.identity(b);
    let mut pending = PendingSlot::default();
    let mut empty = h.empty();
    let input = ExitInputs {
        kind: 4, boundary: 99, value: a, value_repr: Repr::Ref as u64, packet: &mut empty, selection: 0,
    };
    assert_eq!(rt_exit_prepare(&mut h.ctx, &input, &mut pending), NORMAL);
    assert_eq!(h.runtime.champ_pending_contract(&pending).unwrap(), (4, 99));
    let mut split = ScopeSplitPost::default();
    let roots = [a];
    assert_eq!(rt_scope_split(&mut h.ctx, &mut inner, &selection(&roots), &mut split), NORMAL);
    inner = std::mem::take(&mut split.state);
    let before = h.identity(a);
    assert_eq!(rt_exit_extend(&mut h.ctx, &mut pending, &mut split.selected, &mut pending), NORMAL);
    assert_eq!(h.identity(a), before);
    assert_eq!(h.identity(b), before_outer);
    assert_eq!(scope(&h.runtime, outer.record).unwrap().all().words().count(), 1);
    h.gc(&[pending.record, outer.record, inner.record]);
    let plan = h.runtime.champ_register_selection(vec![]);
    let mut empty = h.empty();
    let replacement = StaticReplacement {
        value: b, value_repr: Repr::Ref as u64, packet: &mut empty, selection: plan, boundary: 99,
    };
    let mut replaced = ReplacePost::default();
    assert_eq!(rt_exit_replace(&mut h.ctx, &mut pending, &replacement, &mut replaced), NORMAL);
    pending = replaced.pending;
    assert_eq!(h.identity(a), before);
    assert_eq!(words(&h.runtime, &replaced.rest), HashSet::from([a]));
    assert_eq!(rt_own_accept(&mut h.ctx, &mut outer, &mut replaced.rest, &mut outer), NORMAL);
    changed(before, h.identity(a), outer_info.holder.get(), outer_info.owner.get());
    h.gc(&[pending.record, outer.record, inner.record]);
    // Extraction from the outer scope occurs only when its own defers have finished.
    let roots = [a, b];
    assert_eq!(rt_scope_split(&mut h.ctx, &mut outer, &selection(&roots), &mut split), NORMAL);
    outer = std::mem::take(&mut split.state);
    let a_before = h.identity(a);
    let b_before = h.identity(b);
    assert_eq!(rt_exit_extend(&mut h.ctx, &mut pending, &mut split.selected, &mut pending), NORMAL);
    let holder = h.runtime.champ_new_holder().unwrap();
    let owner = h.runtime.champ_new_owner().unwrap();
    let destination = ExitDestination { holder, owner };
    let mut committed = CommitPost::default();
    assert_eq!(rt_exit_commit(&mut h.ctx, &mut pending, &destination, &mut committed), NORMAL);
    changed(a_before, h.identity(a), holder.get(), owner.get());
    changed(b_before, h.identity(b), holder.get(), owner.get());
    let before = h.identity(a);
    let mut empty = h.empty();
    assert_eq!(rt_exit_prepare(&mut h.ctx, &ExitInputs {
        kind: 4, boundary: 99, value: b, value_repr: Repr::Ref as u64,
        packet: &mut committed.packet, selection: 0,
    }, &mut pending), NORMAL);
    let mut cleanup = h.runtime.champ_cleanup_from_packet(&mut empty).unwrap();
    assert_eq!(rt_exit_abort(&mut h.ctx, &mut pending, &mut cleanup, &mut cleanup), NORMAL);
    assert_eq!(h.identity(a), before);
    assert_eq!(read_cleanup(&h.runtime, cleanup.record).unwrap().remainder.words().count(), 2);
    assert_eq!(pending.record, 0);
    h.gc(&[cleanup.record, outer.record, inner.record]);
}

#[test]
fn champ_s2_destroy_claim_finish_exactly_once_failure_capture_and_m1_witness() {
    let mut h = Harness::new();
    h.ctx.entry_frame = frame_pointer!();
    let owner = h.runtime.champ_new_owner().unwrap();
    let (b, p) = h.fresh(0, owner);
    let (a, q) = h.fresh(1, owner);
    let mut inputs = [p, q];
    let mut packet = PacketSlot::default();
    assert_eq!(rt_own_join(&mut h.ctx, inputs.as_mut_ptr(), 2, &mut packet), NORMAL);
    let initial_a = h.identity(a);
    let initial_b = h.identity(b);
    let mut state = h.runtime.champ_cleanup_from_packet(&mut packet).unwrap();
    assert_eq!(h.identity(a), initial_a);
    assert_eq!(h.identity(b), initial_b);
    let mut first_failure = 0;
    for (object, entry) in [(a, 1), (b, 0)] {
        let before = h.identity(object);
        let key = KeyRef { object, serial: before.serial };
        let mut borrow = DestroyBorrow::default();
        h.gc(&[state.record]);
        assert_eq!(rt_destroy_claim(&mut h.ctx, &mut state, key, before.holder_id + 100,
            before.owner_id, &mut borrow), OUTCOME_STOP);
        assert_eq!(h.identity(object), before);
        h.reset_fatal();
        assert_eq!(rt_destroy_claim(&mut h.ctx, &mut state, key, before.holder_id,
            before.owner_id, &mut borrow), NORMAL);
        assert_eq!(h.identity(object), Identity { life: Life::Destroying, ..before });
        let mut duplicate = DestroyBorrow::default();
        assert_eq!(rt_destroy_claim(&mut h.ctx, &mut state, key, before.holder_id,
            before.owner_id, &mut duplicate), OUTCOME_STOP);
        h.reset_fatal();
        assert_eq!(rt_destroy_finish(&mut h.ctx, &mut state, &mut borrow), OUTCOME_STOP);
        h.reset_fatal();
        let mut post = DestroyPost::default();
        assert_eq!(rt_destroy_invoke(&mut h.ctx, &mut state, &borrow, entry, &mut post), NORMAL);
        assert_eq!(h.ctx.status, OK);
        assert_eq!(h.ctx.failure, 0);
        h.gc(&[state.record]);
        if object == a {
            first_failure = post.failure;
            assert_ne!(first_failure, 0);
            let failure = h.runtime.failure(first_failure).unwrap();
            assert!(matches!(&failure.kind, FailureKind::Panic(text) if text == "destroy failed"));
            assert_eq!(failure.trace.len(), 1);
            assert!(failure.cleanup_origin);
        } else {
            assert_eq!(post.failure, 0);
            // The initial fallible ByteFrom must pass its check before this checkpoint executes.
            assert_eq!(h.runtime.heap.field(b, 0), word::small(9).unwrap());
        }
        assert_eq!(rt_destroy_finish(&mut h.ctx, &mut state, &mut borrow), NORMAL);
        assert_eq!(h.identity(object), Identity { life: Life::Dead, ..before });
        assert_eq!(rt_destroy_finish(&mut h.ctx, &mut state, &mut borrow), OUTCOME_STOP);
        h.reset_fatal();
        if post.failure != 0 {
            assert_eq!(rt_cleanup_failure(&mut h.ctx, &mut state, post.failure, &mut state), NORMAL);
        }
    }
    assert_eq!(read_cleanup(&h.runtime, state.record).unwrap().failure, first_failure);
    assert_eq!(h.runtime.failure(first_failure).unwrap().secondary.len(), 0);
    assert_eq!(read_cleanup(&h.runtime, state.record).unwrap().remainder.words().count(), 0);
    h.gc(&[state.record]);
}

extern "C" fn rejected_entry(ctx: *mut Ctx, io: *mut CallOwn, _: *const Word, _: u64) -> Outcome {
    rt_entry_reject(ctx, io)
}
#[test]
fn champ_s2_entry_reject_closes_once_captures_secondaries_and_returns_unwind_ordinal() {
    let mut h = Harness::new();
    h.ctx.entry_frame = frame_pointer!();
    let owner = h.runtime.champ_new_owner().unwrap();
    let (good, p) = h.fresh(0, owner);
    let (bad, q) = h.fresh(1, owner);
    let mut inputs = [p, q];
    let mut packet = PacketSlot::default();
    assert_eq!(rt_own_join(&mut h.ctx, inputs.as_mut_ptr(), 2, &mut packet), NORMAL);
    let entry = h.runtime.champ_register_entry(EntryContract {
        invoke: rejected_entry, arguments: vec![], replacements: 2, summary: 0, cleanup_mode: CLEANUP_SHIELDED,
    });
    let mut post = CallPost::default();
    let mut io = CallOwn {
        input: packet, entry_abi: entry, output: &mut post, context: 0, cleanup_mode: CLEANUP_SHIELDED,
    };
    h.gc(&[io.input.record]);
    h.runtime.roots.push((good, Repr::Ref));
    h.runtime.roots.push((bad, Repr::Ref));
    assert_eq!(rt_entry_reject(&mut h.ctx, &mut io), 3);
    assert_eq!(io.input.record, 0);
    assert_eq!(post.packet.record, 0);
    assert_eq!(h.ctx.status, FAILED);
    assert_eq!(h.ctx.failure, post.failure);
    assert_eq!(h.identity(good).life, Life::Dead);
    assert_eq!(h.identity(bad).life, Life::Dead);
    assert_eq!(h.runtime.heap.field(good, 0), word::small(9).unwrap());
    let failure = h.runtime.failure(post.failure).unwrap();
    assert!(matches!(&failure.kind, FailureKind::Program(text) if text == "Stack overflow"));
    assert_eq!(failure.secondary.len(), 1);
    assert!(!failure.cleanup_origin);
    assert!(h.runtime.failure(failure.secondary[0]).unwrap().cleanup_origin);
    assert!(matches!(&h.runtime.failure(failure.secondary[0]).unwrap().kind, FailureKind::Panic(_)));
    assert_eq!(rt_entry_reject(&mut h.ctx, &mut io), OUTCOME_STOP);
    assert_eq!(h.ctx.status, FATAL);
    h.gc(&[post.failure, good, bad]);
}

#[test]
fn champ_s2_entry_rejection_budget_is_stop_without_catchable_failure() {
    let mut h = Harness::new();
    h.ctx.entry_frame = frame_pointer!();
    let owner = h.runtime.champ_new_owner().unwrap();
    let (_, packet) = h.fresh(0, owner);
    let entry = h.runtime.champ_register_entry(EntryContract {
        invoke: rejected_entry, arguments: vec![], replacements: 0, summary: 0, cleanup_mode: CLEANUP_ORDINARY,
    });
    let original = packet.record;
    let mut post = CallPost::default();
    let mut io = CallOwn {
        input: packet, entry_abi: entry, output: &mut post, context: 0, cleanup_mode: CLEANUP_ORDINARY,
    };
    h.runtime.budget.stack_capped = true;
    assert_eq!(rt_entry_reject(&mut h.ctx, &mut io), OUTCOME_STOP);
    assert_eq!(h.ctx.status, crate::runtime::EXHAUSTED);
    assert_eq!(io.input.record, original);
    assert_eq!(post.failure, 0);
}

extern "C" fn accepting_entry(ctx: *mut Ctx, io: *mut CallOwn, args: *const Word, argc: u64) -> Outcome {
    unsafe {
        let runtime = &mut *(*ctx).runtime;
        let holder = runtime.champ_new_holder().unwrap();
        let owner = runtime.champ_new_owner().unwrap();
        let mut state = ScopeSlot::default();
        let instance = ScopeInstance { holder, owner, parent_owner: 0 };
        assert_eq!(rt_scope_begin(ctx, &instance, &mut state), NORMAL);
        assert_eq!(rt_own_accept(ctx, &mut state, &mut (*io).input, &mut state), NORMAL);
        let root = *args;
        assert_eq!(argc, 12);
        assert_eq!(*args.add(11), word::small(11).unwrap());
        let mut split = ScopeSplitPost::default();
        assert_eq!(rt_scope_split(ctx, &mut state, &Selection { roots: &root, count: 1 }, &mut split), NORMAL);
        let runtime = &mut *(*ctx).runtime;
        assert_eq!(scope(runtime, split.state.record).unwrap().bound.words().count(), 0);
        let mut rest = std::mem::take(&mut split.state);
        runtime.champ_cleanup_from_scope(&mut rest).unwrap();
        (*io).output.write(CallPost { value: root, packet: split.selected, failure: 0 });
    }
    NORMAL
}
#[test]
fn champ_s2_callown_checked_span_acceptance_and_defer_ordinals() {
    let mut h = Harness::new();
    h.ctx.entry_frame = frame_pointer!();
    let owner = h.runtime.champ_new_owner().unwrap();
    let (object, packet) = h.fresh(0, owner);
    let before = h.identity(object);
    let mut reprs = vec![Repr::Int; 12];
    reprs[0] = Repr::Ref;
    let entry = h.runtime.champ_register_entry(EntryContract {
        invoke: accepting_entry, arguments: reprs, replacements: 0, summary: 42, cleanup_mode: CLEANUP_ORDINARY,
    });
    let mut args: Vec<_> = (0..12).map(|i| word::small(i).unwrap()).collect();
    args[0] = object;
    let mut post = CallPost::default();
    let mut io = CallOwn {
        input: packet, entry_abi: entry, output: &mut post, context: 42, cleanup_mode: CLEANUP_ORDINARY,
    };
    assert_eq!(rt_call_owned(&mut h.ctx, entry, args.as_ptr(), 12, &mut io, &mut post), NORMAL);
    assert_eq!(io.input.record, 0);
    assert_eq!(post.value, object);
    assert_eq!(h.identity(object).epoch, before.epoch + 2);
    assert_eq!(words(&h.runtime, &post.packet), HashSet::from([object]));
    h.gc(&[post.packet.record]);
    let entry = h.runtime.champ_register_entry(EntryContract {
        invoke: rejected_entry, arguments: vec![], replacements: 4, summary: 0, cleanup_mode: CLEANUP_SHIELDED,
    });
    let mut output = CallPost::default();
    let mut io = CallOwn {
        input: post.packet, entry_abi: entry, output: &mut output, context: 0, cleanup_mode: CLEANUP_SHIELDED,
    };
    assert_eq!(rt_invoke_defer(&mut h.ctx, entry, std::ptr::null(), 0, &mut io,
        &RegistrationMap { replacements: 4 }, &mut output), 5);
    assert_eq!(h.identity(object).life, Life::Dead);
    let mut captured = 0;
    assert_eq!(rt_failure_capture(&mut h.ctx, &mut captured), NORMAL);
    assert_eq!(captured, output.failure);
    assert_eq!(h.ctx.status, OK);
    h.gc(&[captured]);
    h.runtime.roots.push((captured, Repr::Ref));
    let older = h.runtime.champ_register_entry(EntryContract {
        invoke: defer_outcome, arguments: vec![], replacements: 0, summary: 0,
        cleanup_mode: CLEANUP_SHIELDED,
    });
    let mut older_post = CallPost::default();
    let mut older_io = CallOwn {
        input: h.empty(), entry_abi: older, output: &mut older_post, context: 0, cleanup_mode: CLEANUP_SHIELDED,
    };
    assert_eq!(rt_invoke_defer(&mut h.ctx, older, std::ptr::null(), 0, &mut older_io,
        &RegistrationMap { replacements: 0 }, &mut older_post), NORMAL);
    assert_eq!(h.ctx.status, OK);
    assert_eq!(older_post.failure, 0);
}

#[test]
fn champ_s2_duplicate_join_is_rejected_without_consumption() {
    let mut h = Harness::new();
    let owner = h.runtime.champ_new_owner().unwrap();
    let (object, packet) = h.fresh(0, owner);
    let before = h.identity(object);
    let record = packet.record;
    let mut inputs = [packet, PacketSlot { record }];
    let mut post = PacketSlot::default();
    assert_eq!(rt_own_join(&mut h.ctx, inputs.as_mut_ptr(), 2, &mut post), OUTCOME_STOP);
    assert_eq!(inputs[0].record, record);
    assert_eq!(inputs[1].record, record);
    assert_eq!(post.record, 0);
    assert_eq!(h.identity(object), before);
    assert!(matches!(data(&h.runtime, record), Ok(Data::Packet(_))));
    h.gc(&[record]);
}

#[test]
fn champ_s2_positional_bound_history_survives_split_admit_and_foreign_stage() {
    let mut h = Harness::new();
    let mut source = h.scope();
    let mut action = h.scope();
    let source_info = scope(&h.runtime, source.record).unwrap();
    let action_info = scope(&h.runtime, action.record).unwrap();
    let (object, mut offered) = h.fresh(0, source_info.owner);
    h.gc(&[source.record, action.record, offered.record]);
    assert_eq!(rt_own_bind(&mut h.ctx, &mut source, 0, object, &mut offered, &mut source), NORMAL);
    let mut split = ScopeSplitPost::default();
    let roots = [object];
    h.gc(&[source.record, action.record]);
    assert_eq!(rt_scope_split(&mut h.ctx, &mut source, &selection(&roots), &mut split), NORMAL);
    source = split.state;
    h.gc(&[source.record, action.record, split.selected.record]);
    let before = h.identity(object);
    assert_eq!(rt_own_admit(&mut h.ctx, &mut source, &mut split.selected, &mut source), NORMAL);
    changed(before, h.identity(object), source_info.holder.get(), source_info.owner.get());
    let info = scope(&h.runtime, source.record).unwrap();
    assert_eq!(info.admission.words().count(), 0);
    assert_eq!(info.bound.words().collect::<Vec<_>>(), vec![object]);
    h.gc(&[source.record, action.record]);
    let mut split = ScopeSplitPost::default();
    assert_eq!(rt_scope_split(&mut h.ctx, &mut source, &selection(&roots), &mut split), NORMAL);
    source = split.state;
    let before = h.identity(object);
    h.gc(&[source.record, action.record, split.selected.record]);
    let inputs = StageInputs {
        state: &mut action, source: std::ptr::null_mut(), selection: 0,
        field: 0, value: object, packet: &mut split.selected,
    };
    let mut post = StagePost::default();
    assert_eq!(rt_own_stage(&mut h.ctx, &inputs, &mut post), NORMAL);
    action = post.state;
    changed(before, h.identity(object), action_info.holder.get(), action_info.owner.get());
    h.gc(&[source.record, action.record]);
    assert_eq!(scope(&h.runtime, action.record).unwrap().staging.words().collect::<Vec<_>>(), vec![object]);
}

#[test]
fn champ_s2_static_guards_chunk_runs_and_whole_batch_overflow() {
    let mut h = Harness::new();
    let holder = h.runtime.champ_new_holder().unwrap();
    let owner = h.runtime.champ_new_owner().unwrap();
    let mut entries = Vec::new();
    for _ in 0..65 {
        let object = h.runtime.heap.object_new(0, &[word::small(7).unwrap()]);
        h.runtime.champ_create_commit(object, holder, owner).unwrap();
        entries.push(StaticEntry { object, active: 1 });
        entries.push(StaticEntry::default());
    }
    h.runtime.heap.collect(|visit| {
        for entry in &entries {
            visit(entry.object, Repr::Ref);
        }
    });
    let mut input = h.runtime.champ_packet_from_static(&mut entries, holder, owner).unwrap();
    assert!(entries.iter().all(|entry| entry.object == 0 && entry.active == 0));
    let packet = super::helpers::packet(&h.runtime, input.record).unwrap();
    let chunk_lengths: Vec<_> = packet.journals[0][0].chunks.iter().map(std::collections::VecDeque::len).collect();
    assert_eq!(chunk_lengths, vec![32, 32, 1]);
    let objects: Vec<_> = packet.words().collect();
    let selected = [objects[0], objects[64]];
    let before = h.identity(selected[0]);
    let mut maxed = h.identity(selected[1]);
    maxed.epoch = u64::MAX;
    h.runtime.heap.identity_write(selected[1], maxed);
    h.gc(&[input.record]);
    let original = input.record;
    let mut post = SplitPost::default();
    assert_eq!(rt_own_split_checked(&mut h.ctx, &mut input, &selection(&selected), &mut post), UNWIND);
    assert_eq!(post.rest.record, original);
    assert_eq!(h.identity(selected[0]), before);
    assert_eq!(h.identity(selected[1]), maxed);
    h.gc(&[post.rest.record]);
}

extern "C" fn defer_outcome(ctx: *mut Ctx, io: *mut CallOwn, _: *const Word, _: u64) -> Outcome {
    unsafe {
        let runtime = &mut *(*ctx).runtime;
        let ordinal = (*ctx).scratch;
        if ordinal == 0 {
            assert!(runtime.champ_packet_keys(&(*io).input).unwrap().is_empty());
            super::helpers::consume(runtime, (*io).input.record);
            (*io).input.record = 0;
            (*io).output.write(CallPost::default());
        } else {
            let mut packet = PacketSlot::default();
            assert_eq!(rt_own_move(ctx, &mut (*io).input, &mut packet), NORMAL);
            (*io).output.write(CallPost { value: word::small(17).unwrap(), packet, failure: 0 });
        }
        ordinal
    }
}
#[test]
fn champ_s2_defer_continue_and_declared_replace_ordinals_are_not_tags_in_post() {
    let mut h = Harness::new();
    h.ctx.entry_frame = frame_pointer!();
    let entry = h.runtime.champ_register_entry(EntryContract {
        invoke: defer_outcome, arguments: vec![], replacements: 3, summary: 77,
        cleanup_mode: CLEANUP_SHIELDED,
    });
    for ordinal in 0..=3 {
        h.ctx.scratch = ordinal;
        let mut post = CallPost { value: u64::MAX, packet: PacketSlot { record: u64::MAX }, failure: u64::MAX };
        let mut io = CallOwn {
            input: h.empty(), entry_abi: entry, output: &mut post, context: 77, cleanup_mode: CLEANUP_SHIELDED,
        };
        h.gc(&[io.input.record]);
        assert_eq!(rt_invoke_defer(&mut h.ctx, entry, std::ptr::null(), 0, &mut io,
            &RegistrationMap { replacements: 3 }, &mut post), ordinal);
        assert_eq!(h.ctx.status, OK);
        assert_eq!(post.failure, 0);
        assert_eq!(post.value, if ordinal == 0 { 0 } else { word::small(17).unwrap() });
        h.gc(&[post.packet.record]);
    }
}

#[test]
fn champ_s2_same_alias_replacement_keeps_reserve_without_tail_writes() {
    let mut h = Harness::new();
    let owner = h.runtime.champ_new_owner().unwrap();
    let (object, mut packet) = h.fresh(0, owner);
    let before = h.identity(object);
    let mut pending = PendingSlot::default();
    let inputs = ExitInputs {
        kind: 1, boundary: 7, value: object, value_repr: Repr::Ref as u64, packet: &mut packet, selection: 0,
    };
    h.gc(&[packet.record]);
    assert_eq!(rt_exit_prepare(&mut h.ctx, &inputs, &mut pending), NORMAL);
    h.gc(&[pending.record]);
    let plan = h.runtime.champ_register_selection(vec![0]);
    let mut empty = h.empty();
    let mut post = ReplacePost::default();
    let replacement = StaticReplacement {
        value: object, value_repr: Repr::Ref as u64, packet: &mut empty, selection: plan, boundary: 7,
    };
    h.gc(&[pending.record, empty.record]);
    assert_eq!(rt_exit_replace(&mut h.ctx, &mut pending, &replacement, &mut post), NORMAL);
    assert_eq!(h.identity(object), before);
    assert_eq!(words(&h.runtime, &post.rest).len(), 0);
    h.gc(&[post.pending.record, post.rest.record]);
    let destination = ExitDestination { holder: HolderId(before.holder_id), owner };
    let mut committed = CommitPost::default();
    assert_eq!(rt_exit_commit(&mut h.ctx, &mut post.pending, &destination, &mut committed), NORMAL);
    assert_eq!(h.identity(object), before);
    h.gc(&[committed.packet.record]);
}

#[test]
fn champ_s2_checked_selection_walks_a_cyclic_heap_iteratively() {
    let mut h = Harness::new();
    let holder = h.runtime.champ_new_holder().unwrap();
    let owner = h.runtime.champ_new_owner().unwrap();
    let mut entries = Vec::new();
    let mut previous = 0;
    for _ in 0..4096 {
        let object = h.runtime.heap.object_new(0, &[word::small(7).unwrap(), previous]);
        h.runtime.champ_create_commit(object, holder, owner).unwrap();
        entries.push(StaticEntry { object, active: 1 });
        previous = object;
    }
    h.runtime.heap.set_field(entries[0].object, 1, previous);
    let mut input = h.runtime.champ_packet_from_static(&mut entries, holder, owner).unwrap();
    h.gc(&[input.record]);
    let roots = [previous];
    let mut post = SplitPost::default();
    assert_eq!(rt_own_split_checked(&mut h.ctx, &mut input, &selection(&roots), &mut post), NORMAL);
    assert_eq!(words(&h.runtime, &post.rest).len(), 0);
    assert_eq!(words(&h.runtime, &post.selected).len(), 4096);
    h.gc(&[post.rest.record, post.selected.record]);
    for object in words(&h.runtime, &post.selected) {
        assert_eq!(h.identity(object).epoch, 2);
    }
}

#[test]
fn champ_s2_stage_between_scopes_is_one_transition_without_temporary_holder() {
    let mut h = Harness::new();
    let mut source = h.scope();
    let mut action = h.scope();
    let source_info = scope(&h.runtime, source.record).unwrap();
    let action_info = scope(&h.runtime, action.record).unwrap();
    let (object, mut packet) = h.fresh(0, source_info.owner);
    assert_eq!(rt_own_accept(&mut h.ctx, &mut source, &mut packet, &mut source), NORMAL);
    let before = h.identity(object);
    let selection = h.runtime.champ_register_selection(vec![0]);
    let mut empty = h.empty();
    let inputs = StageInputs {
        state: &mut action, source: &mut source, selection, field: 0, value: object, packet: &mut empty,
    };
    h.gc(&[source.record, action.record, empty.record]);
    let mut post = StagePost::default();
    assert_eq!(rt_own_stage(&mut h.ctx, &inputs, &mut post), NORMAL);
    changed(before, h.identity(object), action_info.holder.get(), action_info.owner.get());
    assert_eq!(scope(&h.runtime, post.source.record).unwrap().all().words().count(), 0);
    assert_eq!(scope(&h.runtime, post.state.record).unwrap().staging.words().count(), 1);
    h.gc(&[post.source.record, post.state.record]);
}

#[test]
fn champ_s2_owner_only_holder_only_and_combined_transitions_increment_once() {
    let mut h = Harness::new();
    let owner = h.runtime.champ_new_owner().unwrap();
    let (object, mut packet) = h.fresh(0, owner);
    let initial = h.identity(object);
    let next_owner = h.runtime.champ_new_owner().unwrap();
    let next_holder = h.runtime.champ_new_holder().unwrap();
    for (holder, owner, increment) in [
        (HolderId(initial.holder_id), next_owner, 1),
        (next_holder, next_owner, 1),
        (next_holder, next_owner, 0),
    ] {
        let before = h.identity(object);
        let mut pending = PendingSlot::default();
        let inputs = ExitInputs {
            kind: 1, boundary: owner.get(), value: object, value_repr: Repr::Ref as u64,
            packet: &mut packet, selection: 0,
        };
        h.gc(&[packet.record]);
        assert_eq!(rt_exit_prepare(&mut h.ctx, &inputs, &mut pending), NORMAL);
        h.gc(&[pending.record]);
        let mut post = CommitPost::default();
        assert_eq!(rt_exit_commit(&mut h.ctx, &mut pending, &ExitDestination { holder, owner }, &mut post), NORMAL);
        assert_eq!(h.identity(object), Identity {
            holder_id: holder.get(), owner_id: owner.get(), epoch: before.epoch + increment, ..before
        });
        packet = post.packet;
        h.gc(&[packet.record]);
    }
}

#[test]
fn champ_admissible_split_preserves_bound_history_and_escape_restores_admission() {
    let mut h = Harness::new();
    let mut state = h.scope();
    let owner = scope(&h.runtime, state.record).unwrap().owner;
    let (bound, mut packet) = h.fresh(0, owner);
    assert_eq!(rt_own_admit(&mut h.ctx, &mut state, &mut packet, &mut state), NORMAL);
    let mut empty = h.empty();
    assert_eq!(rt_own_bind(&mut h.ctx, &mut state, 0, bound, &mut empty, &mut state), NORMAL);
    let before = h.identity(bound);
    let (fresh, mut packet) = h.fresh(0, owner);
    assert_eq!(rt_own_admit(&mut h.ctx, &mut state, &mut packet, &mut state), NORMAL);
    let roots = [bound, fresh];
    let mut split = ScopeSplitPost::default();
    assert_eq!(rt_scope_split_admissible(&mut h.ctx, &mut state, &selection(&roots), &mut split), NORMAL);
    state = split.state;
    assert_eq!(words(&h.runtime, &split.selected), HashSet::from([fresh]));
    assert_eq!(scope(&h.runtime, state.record).unwrap().bound.words().collect::<Vec<_>>(), vec![bound]);
    assert_eq!(h.identity(bound), before);
    h.gc(&[state.record, split.selected.record]);
    let roots = [bound];
    let mut bound_split = ScopeSplitPost::default();
    assert_eq!(rt_scope_split(&mut h.ctx, &mut state, &selection(&roots), &mut bound_split), NORMAL);
    let mut target = h.scope();
    let destination = scope(&h.runtime, target.record).unwrap();
    let destination = ExitDestination { holder: destination.holder, owner: destination.owner };
    let mut pending = PendingSlot::default();
    let inputs = ExitInputs { kind: 1, boundary: owner.get(), value: bound, value_repr: Repr::Ref as u64,
        packet: &mut bound_split.selected, selection: 0 };
    assert_eq!(rt_exit_prepare(&mut h.ctx, &inputs, &mut pending), NORMAL);
    let mut committed = CommitPost::default();
    assert_eq!(rt_exit_commit(&mut h.ctx, &mut pending, &destination, &mut committed), NORMAL);
    assert_eq!(rt_own_admit(&mut h.ctx, &mut target, &mut committed.packet, &mut target), NORMAL);
    let mut escaped = ScopeSplitPost::default();
    assert_eq!(rt_scope_split_admissible(&mut h.ctx, &mut target, &selection(&roots), &mut escaped), NORMAL);
    assert_eq!(words(&h.runtime, &escaped.selected), HashSet::from([bound]));
}

/// A managed array refers to identities it does not own: selecting it selects its elements, also through
/// nested arrays, while an array of text selects nothing (#622).
#[test]
fn champ_selection_walk_descends_into_managed_arrays() {
    let mut h = Harness::new();
    let owner = h.runtime.champ_new_owner().unwrap();
    let (a, p) = h.fresh(0, owner);
    let (b, q) = h.fresh(0, owner);
    let mut inputs = [p, q];
    let mut joined = PacketSlot::default();
    assert_eq!(rt_own_join(&mut h.ctx, inputs.as_mut_ptr(), 2, &mut joined), NORMAL);
    let text = h.runtime.heap.allocate_text("plain");
    let texts = h.runtime.heap.allocate_array(Repr::Ref, &[text, text]);
    let mut post = SplitPost::default();
    let roots = [texts];
    assert_eq!(rt_own_split_checked(&mut h.ctx, &mut joined, &selection(&roots), &mut post), NORMAL);
    assert!(words(&h.runtime, &post.selected).is_empty());
    assert_eq!(words(&h.runtime, &post.rest), HashSet::from([a, b]));
    let inner = h.runtime.heap.allocate_array(Repr::Ref, &[b]);
    let managed = h.runtime.heap.allocate_array(Repr::Ref, &[a]);
    let nested = h.runtime.heap.allocate_array(Repr::Ref, &[inner]);
    let mut rest = post.rest;
    let mut post = SplitPost::default();
    let roots = [managed, nested];
    assert_eq!(rt_own_split_checked(&mut h.ctx, &mut rest, &selection(&roots), &mut post), NORMAL);
    assert_eq!(words(&h.runtime, &post.selected), HashSet::from([a, b]));
    assert!(words(&h.runtime, &post.rest).is_empty());
    h.gc(&[post.rest.record, post.selected.record]);
}

/// A managed array may refer to identities another packet owns (the result of mapping a managed array): a
/// checked split selects the elements in its source and leaves the others with their owner, while a direct
/// root outside the source is still refused (#692).
#[test]
fn champ_checked_selection_leaves_foreign_collection_elements_with_their_owner() {
    let mut h = Harness::new();
    let owner = h.runtime.champ_new_owner().unwrap();
    let (a, mut source) = h.fresh(0, owner);
    let (b, other) = h.fresh(0, owner);
    let managed = h.runtime.heap.allocate_array(Repr::Ref, &[a, b]);
    let nested = h.runtime.heap.allocate_array(Repr::Ref, &[managed]);
    let mut post = SplitPost::default();
    let roots = [nested];
    assert_eq!(rt_own_split_checked(&mut h.ctx, &mut source, &selection(&roots), &mut post), NORMAL);
    assert_eq!(words(&h.runtime, &post.selected), HashSet::from([a]));
    assert!(words(&h.runtime, &post.rest).is_empty());
    assert_eq!(words(&h.runtime, &other), HashSet::from([b]));
    h.gc(&[post.rest.record, post.selected.record, other.record]);
    let mut selected = post.selected;
    let roots = [b];
    let mut refused = SplitPost::default();
    assert_eq!(rt_own_split_checked(&mut h.ctx, &mut selected, &selection(&roots), &mut refused), UNWIND);
    assert_eq!(h.ctx.status, FAILED);
}

/// A transferred value may borrow an identity an enclosing scope owns (an element of its managed array pushed
/// into its builder): the transfer moves what its source holds and leaves the borrowed identity with its owner
/// (#692).
#[test]
fn champ_transfer_leaves_a_borrowed_identity_with_its_owner() {
    let mut h = Harness::new();
    let mut from = h.scope();
    let mut to = h.scope();
    let from_scope = scope(&h.runtime, from.record).unwrap();
    let to_scope = scope(&h.runtime, to.record).unwrap();
    let (moved, mut packet) = h.fresh(0, from_scope.owner);
    assert_eq!(rt_own_accept(&mut h.ctx, &mut from, &mut packet, &mut from), NORMAL);
    let (borrowed, mut owned) = h.fresh(0, to_scope.owner);
    assert_eq!(rt_own_accept(&mut h.ctx, &mut to, &mut owned, &mut to), NORMAL);
    let kept = h.identity(borrowed);
    let roots = [moved, borrowed];
    let mut post = TransferPost::default();
    let inputs = TransferInputs { from: &mut from, to: &mut to };
    assert_eq!(rt_scope_transfer(&mut h.ctx, &inputs, &selection(&roots), &mut post), NORMAL);
    let (from, to) = (post.from, post.to);
    assert_eq!(scope(&h.runtime, from.record).unwrap().all().words().count(), 0);
    assert_eq!(scope(&h.runtime, to.record).unwrap().all().words().collect::<HashSet<_>>(),
        HashSet::from([moved, borrowed]));
    assert_eq!(h.identity(borrowed), kept);
    h.gc(&[from.record, to.record]);
}
