//! Record layouts used by the helper registry and future service-slot stack-map lowering.
//! Pointer arguments designate caller storage; they are never themselves heap references.
use super::*;
use crate::jit::{Effect, HELPERS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperClass {
    L,
    G,
    X,
    /// Draft 3 §1.1: like X, and it may return `OUTCOME_PENDING`.
    S,
}
/// Classes derive from the same effects table that checks Rust and CLIF signatures.
pub fn helper_class(name: &str) -> Option<HelperClass> {
    let (_, effects) = HELPERS.iter().find(|(entry, _)| *entry == name && name.starts_with("champ_"))?;
    Some(if effects.contains(&Effect::Suspends) {
        HelperClass::S
    } else if effects.contains(&Effect::Reenters) {
        HelperClass::X
    } else if effects.contains(&Effect::Collects) || effects.contains(&Effect::Fails) {
        HelperClass::G
    } else {
        HelperClass::L
    })
}
#[derive(Debug)]
pub struct RecordLayout {
    pub name: &'static str,
    pub size: usize,
    pub align: usize,
    /// Unconditional Ref words in an initialized/live record.
    pub traced_offsets: &'static [usize],
    /// Words traced according to the verified language result schema, not unconditionally as Ref.
    pub schema_offsets: &'static [usize],
    /// Indirect caller-rooted word span (pointer offset, count offset), e.g. selection roots.
    pub span: Option<(usize, usize)>,
}
macro_rules! record {
    ($ty:ty; [$($root:ident),*]; [$($value:ident),*]) => {
        RecordLayout {
            name: stringify!($ty), size: size_of::<$ty>(), align: align_of::<$ty>(),
            traced_offsets: &[$(std::mem::offset_of!($ty, $root)),*],
            schema_offsets: &[$(std::mem::offset_of!($ty, $value)),*], span: None,
        }
    };
}
pub const RECORD_LAYOUTS: &[RecordLayout] = &[
    record!(KeyRef; [object]; []),
    record!(StaticEntry; [object]; []),
    record!(PacketSlot; [record]; []),
    record!(ScopeSlot; [record]; []),
    record!(PendingSlot; [record]; []),
    record!(CleanupSlot; [record]; []),
    record!(ScopeInstance; []; []),
    RecordLayout {
        name: "Selection", size: size_of::<Selection>(), align: align_of::<Selection>(),
        traced_offsets: &[], schema_offsets: &[],
        span: Some((std::mem::offset_of!(Selection, roots), std::mem::offset_of!(Selection, count))),
    },
    // Nested one-word service records have their Ref at offset zero.
    record!(SplitPost; [rest, selected, failure]; []),
    record!(ScopeSplitPost; [state, selected, failure]; []),
    record!(TransferInputs; []; []),
    record!(TransferPost; [from, to, failure]; []),
    record!(StageInputs; []; [value]),
    record!(StagePost; [state, source]; []),
    record!(ExitInputs; []; [value]),
    record!(StaticReplacement; []; [value]),
    record!(ReplacePost; [pending, rest]; []),
    record!(ExitDestination; []; []),
    record!(CommitPost; [packet]; [value]),
    record!(CallPost; [packet, failure]; [value]),
    record!(CallOwn; [input]; []),
    record!(RegistrationMap; []; []),
    record!(DestroyBorrow; [object, activation]; []),
    record!(DestroyPost; [state, failure]; []),
    record!(CloseInputs; [failure]; []),
    record!(NextPost; [state, key]; []),
    record!(FinishPost; [failure, pending]; []),
    record!(FutureNewIn; []; []),
    record!(FutureNewPost; [future, cache, state]; []),
    record!(AwaitPost; [task]; [value]),
    record!(AwaitUnwindPost; [state, failure]; []),
    record!(PublishIn; [failure]; [value]),
    record!(FlagPost; []; []),
];

/// One view of signatures/effects (JIT), machine records (above), and abstract outcome schemas (SPB 17).
/// Internal destroy/capture steps have no SPB opcode; their fixed output record is still registered.
#[derive(Debug)]
pub struct HelperAbi {
    pub name: &'static str,
    pub class: HelperClass,
    pub effects: &'static [Effect],
    pub output: Option<&'static RecordLayout>,
    pub opcode: Option<okkama_sparrow::spb17::Opcode>,
}
impl HelperAbi {
    pub fn schemas(&self, context: &okkama_sparrow::spb17::ResultContext)
        -> Result<Vec<(okkama_sparrow::spb17::Tag, Vec<okkama_sparrow::spb17::ProducedType>)>,
            okkama_sparrow::spb17::Error>
    {
        let Some(opcode) = self.opcode else { return Ok(Vec::new()) };
        opcode.tags(context).into_iter().map(|tag| Ok((tag, opcode.results(tag, context)?))).collect()
    }
    /// ProducedSlot to Post byte offsets. None is the proof-only, zero-machine-word deps slot.
    /// These offsets are read only after return; Post never belongs to the helper call's stack map.
    pub fn post_offsets(&self, tag: okkama_sparrow::spb17::Tag,
        context: &okkama_sparrow::spb17::ResultContext)
        -> Result<Vec<Option<usize>>, okkama_sparrow::spb17::Error>
    {
        use okkama_sparrow::spb17::{Opcode as O, ProducedType, ServiceKind, Tag};
        let invalid = || okkama_sparrow::spb17::Error {
            offset: None, message: format!("{}: no payload schema for {tag:?}", self.name), rule: "ABI_SCHEMA",
        };
        let opcode = self.opcode.ok_or_else(invalid)?;
        if opcode.call_abi().is_some() {
            return opcode.call_post_offsets(tag, context);
        }
        if opcode.cleanup_abi().is_some() {
            return runtime_cleanup_offsets(opcode, tag, context);
        }
        let schema = opcode.results(tag, context)?;
        let erased = |slot: &ProducedType| matches!(slot, ProducedType::Service(ServiceKind::Deps));
        let count = schema.iter().filter(|slot| !erased(slot)).count();
        let offsets = match (opcode, tag) {
            (O::OwnSplitChecked | O::ScopeSplit, Tag::Unwind) => vec![0, 16],
            // ScopeSplitPost: the state, the unaccepted packet and the failure.
            (O::OwnDynamic, Tag::Unwind) => vec![0, 8, 16],
            (O::ScopeTransfer, Tag::Unwind) => {
                if context.between { vec![0, 8, 16] } else { vec![0, 16] }
            }
            (O::ExitCommit, Tag::Normal) => {
                if count == 1 { vec![8] } else { vec![0, 8] }
            }
            _ => (0..count).map(|index| index * 8).collect(),
        };
        let mut offsets = offsets.into_iter();
        let mut result = Vec::new();
        for slot in schema {
            result.push(if erased(&slot) { None } else { Some(offsets.next().ok_or_else(invalid)?) });
        }
        if offsets.next().is_some() {
            return Err(invalid());
        }
        Ok(result)
    }

}
pub fn helper_abi(name: &str) -> Option<HelperAbi> {
    use okkama_sparrow::spb17::Opcode as O;
    let (name, effects) = HELPERS.iter().find(|(entry, _)| *entry == name && name.starts_with("champ_"))?;
    let (record, opcode) = match *name {
        "champ_scope_begin" => ("ScopeSlot", Some(O::ScopeBegin)),
        "champ_own_empty" => ("PacketSlot", Some(O::OwnEmpty)),
        "champ_own_move" => ("PacketSlot", Some(O::OwnMove)),
        "champ_own_join" => ("PacketSlot", Some(O::OwnJoin)),
        "champ_own_create_commit" => ("PacketSlot", Some(O::OwnCreateCommit)),
        "champ_own_admit" => ("ScopeSlot", Some(O::OwnAdmit)),
        "champ_own_accept" => ("ScopeSlot", Some(O::OwnAccept)),
        "champ_own_bind" => ("ScopeSlot", Some(O::OwnBind)),
        "champ_own_rebind" => ("ScopeSlot", Some(O::OwnRebind)),
        "champ_own_stage" => ("StagePost", Some(O::OwnStage)),
        "champ_own_split_static" => ("SplitPost", Some(O::OwnSplit)),
        "champ_own_split_checked" => ("SplitPost", Some(O::OwnSplitChecked)),
        "champ_scope_split" | "champ_scope_split_admissible" => ("ScopeSplitPost", Some(O::ScopeSplit)),
        "champ_scope_transfer" | "champ_scope_transfer_admissible" => ("TransferPost", Some(O::ScopeTransfer)),
        "champ_own_dynamic" => ("ScopeSplitPost", Some(O::OwnDynamic)),
        "champ_exit_prepare" => ("PendingSlot", Some(O::ExitPrepare)),
        "champ_exit_extend" => ("PendingSlot", Some(O::ExitExtend)),
        "champ_exit_replace" => ("ReplacePost", Some(O::ExitReplace)),
        "champ_exit_commit" => ("CommitPost", Some(O::ExitCommit)),
        "champ_exit_abort" => ("CleanupSlot", Some(O::ExitAbort)),
        "champ_cleanup_begin" => ("CleanupSlot", Some(O::CleanupBegin)),
        "champ_cleanup_settle_empty" | "champ_cleanup_settle_packet" => ("CleanupSlot", Some(O::CleanupSettle)),
        "champ_cleanup_plan" => ("CleanupSlot", Some(O::CleanupPlan)),
        "champ_cleanup_next" => ("NextPost", Some(O::CleanupNext)),
        "champ_cleanup_destroy" => ("DestroyPost", Some(O::CleanupDestroy)),
        "champ_destroy_direct" => ("DestroyPost", Some(O::DestroyDirect)),
        "champ_cleanup_finish" => ("FinishPost", Some(O::CleanupFinish)),
        "champ_cleanup_failure" => ("CleanupSlot", Some(O::CleanupFailure)),
        "champ_call_owned" => ("CallPost", Some(O::InvokeCallOwn)),
        "champ_invoke_defer" => ("CallPost", Some(O::InvokeDefer)),
        "champ_entry_reject" => ("CallPost", None),
        "champ_destroy_claim" => ("DestroyBorrow", None),
        "champ_destroy_invoke" => ("DestroyPost", None),
        "champ_destroy_finish" | "champ_failure_capture" => ("", None),
        "champ_future_new" => ("FutureNewPost", Some(O::FutureNew)),
        "champ_await_view" => ("AwaitPost", Some(O::Await)),
        "champ_await_unwind" => ("AwaitUnwindPost", Some(O::AwaitUnwind)),
        "champ_future_publish" => ("", Some(O::FuturePublish)),
        "champ_future_cancel" => ("", Some(O::FutureCancel)),
        "champ_future_cancelled" => ("FlagPost", Some(O::FutureCancelled)),
        "champ_cleanup_deliver_cancel" => ("CleanupSlot", Some(O::CleanupDeliverCancel)),
        "champ_cleanup_settle" => ("CleanupSlot", Some(O::CleanupSettle)),
        _ if okkama_sparrow::spb17::helper(name).is_some() => ("", None),
        _ => return None,
    };
    let shared = okkama_sparrow::spb17::helper(name)?;
    assert_eq!(opcode, shared.opcode);
    Some(HelperAbi {
        name,
        class: helper_class(name)?,
        effects,
        output: RECORD_LAYOUTS.iter().find(|layout| layout.name == record),
        opcode,
    })
}

/// Independent runtime record view; never delegates to SPB's offset implementation.
fn runtime_cleanup_layout(op: okkama_sparrow::spb17::Opcode) -> Option<(usize, Vec<usize>)> {
    use okkama_sparrow::spb17::Opcode as O;
    macro_rules! layout {
        ($ty:ty; $($field:ident),+) => {
            (size_of::<$ty>(), vec![$(std::mem::offset_of!($ty, $field)),+])
        };
    }
    Some(match op {
        O::CleanupBegin | O::CleanupSettle | O::CleanupPlan | O::CleanupFailure | O::ExitAbort => {
            layout!(CleanupSlot; record)
        }
        O::ExitPrepare | O::ExitExtend => layout!(PendingSlot; record),
        O::ExitReplace => layout!(ReplacePost; pending, rest),
        O::ExitCommit => layout!(CommitPost; value, packet),
        O::CleanupNext => layout!(NextPost; state, key),
        O::CleanupDestroy | O::DestroyDirect => layout!(DestroyPost; state, failure),
        O::CleanupFinish => layout!(FinishPost; closed, failure, pending),
        _ => return None,
    })
}

fn runtime_cleanup_offsets(op: okkama_sparrow::spb17::Opcode, tag: okkama_sparrow::spb17::Tag,
    context: &okkama_sparrow::spb17::ResultContext) -> Result<Vec<Option<usize>>, okkama_sparrow::spb17::Error>
{
    use okkama_sparrow::spb17::{Opcode as O, ProducedType as P, ServiceKind as S};
    let (_, offsets) = runtime_cleanup_layout(op).expect("registered cleanup record");
    let schema = op.results(tag, context)?;
    if op == O::ExitCommit {
        let mut result = Vec::new();
        if context.value.is_some() { result.push(Some(std::mem::offset_of!(CommitPost, value))); }
        result.extend([Some(std::mem::offset_of!(CommitPost, packet)), None]);
        return Ok(result);
    }
    Ok(schema.iter().enumerate().map(|(index, slot)| {
        if matches!(slot, P::Service(S::Deps | S::Closed)) { None } else { Some(offsets[index]) }
    }).collect())
}

// Keep the always-built SPB transport registry mechanically tied to the actual #572 records.
const _: () = {
    use okkama_sparrow::spb17::Opcode;
    assert!(size_of::<NextPost>() == 24);
    assert!(std::mem::offset_of!(NextPost, state) == 0);
    assert!(std::mem::offset_of!(NextPost, key) == 8);
    assert!(size_of::<DestroyPost>() == 16);
    assert!(std::mem::offset_of!(DestroyPost, state) == 0);
    assert!(std::mem::offset_of!(DestroyPost, failure) == 8);
    assert!(size_of::<FinishPost>() == 24);
    assert!(std::mem::offset_of!(FinishPost, closed) == 0);
    assert!(std::mem::offset_of!(FinishPost, failure) == 8);
    assert!(std::mem::offset_of!(FinishPost, pending) == 16);
    // call_abi is also consumed by codegen; offsets below are checked against repr(C), never inferred.
    let Some(abi) = Opcode::InvokeCallOwn.call_abi() else { panic!("missing call ABI") };
    assert!(size_of::<CallOwn>() == abi.call_own_size);
    assert!(std::mem::offset_of!(CallOwn, input) == abi.input_offset);
    assert!(std::mem::offset_of!(CallOwn, entry_abi) == abi.entry_offset);
    assert!(std::mem::offset_of!(CallOwn, output) == abi.output_offset);
    assert!(std::mem::offset_of!(CallOwn, context) == abi.context_offset);
    assert!(std::mem::offset_of!(CallOwn, cleanup_mode) == abi.cleanup_offset);
    assert!(size_of::<CallPost>() == abi.post_size);
    assert!(std::mem::offset_of!(CallPost, value) == abi.post_value_offset);
    assert!(std::mem::offset_of!(CallPost, packet) == abi.post_packet_offset);
    assert!(std::mem::offset_of!(CallPost, failure) == abi.post_failure_offset);
};

// Relocation transport is checked against the repr(C) records used by #572, including the two-word key.
const _: () = {
    use okkama_sparrow::spb17::Opcode;
    let Some(next) = Opcode::CleanupNext.cleanup_abi() else { panic!("missing next ABI") };
    assert!(next.post_size == size_of::<NextPost>());
    assert!(next.post_offsets[0] == std::mem::offset_of!(NextPost, state));
    assert!(next.post_offsets[1] == std::mem::offset_of!(NextPost, key));
    let Some(destroy) = Opcode::CleanupDestroy.cleanup_abi() else { panic!("missing destroy ABI") };
    assert!(destroy.post_size == size_of::<DestroyPost>());
    assert!(destroy.post_offsets[1] == std::mem::offset_of!(DestroyPost, failure));
    let Some(finish) = Opcode::CleanupFinish.cleanup_abi() else { panic!("missing finish ABI") };
    assert!(finish.post_size == size_of::<FinishPost>());
    assert!(finish.post_offsets[2] == std::mem::offset_of!(FinishPost, pending));
    let Some(commit) = Opcode::ExitCommit.cleanup_abi() else { panic!("missing commit ABI") };
    assert!(commit.post_size == size_of::<CommitPost>());
    assert!(commit.post_offsets[0] == std::mem::offset_of!(CommitPost, value));
    assert!(commit.post_offsets[1] == std::mem::offset_of!(CommitPost, packet));
};

#[cfg(test)]
mod emission_abi_tests {
    use super::*;

    fn validate(op: okkama_sparrow::spb17::Opcode, shared: okkama_sparrow::spb17::CleanupAbi)
        -> Result<(), &'static str>
    {
        use okkama_sparrow::spb17::MachineArgument as M;
        let (size, offsets) = runtime_cleanup_layout(op).unwrap();
        if shared.post_size != size || shared.post_offsets != offsets { return Err("ABI_RECORD"); }
        let (_, types) = crate::jit::HELPER_ARGUMENT_TYPES.iter()
            .find(|(name, _)| *name == shared.helper).unwrap();
        let mut arguments = vec![M::Context];
        arguments.extend(types.iter().map(|ty| {
            if ty.contains("KeyRef") { M::KeyRef }
            else if ty.starts_with('*') { M::Pointer }
            else { M::Word }
        }));
        if shared.arguments != arguments { return Err("ABI_ARGUMENTS"); }
        Ok(())
    }

    #[test]
    fn champ_cleanup_abi_rejects_record_and_argument_disagreement() {
        use okkama_sparrow::spb17::{MachineArgument as M, Opcode as O};
        for &op in O::ALL {
            let Some(shared) = op.cleanup_abi() else { continue };
            let mut wrong = shared;
            wrong.post_size += 8;
            assert_eq!(validate(op, wrong), Err("ABI_RECORD"));
            wrong = shared;
            wrong.post_offsets = &[usize::MAX];
            assert_eq!(validate(op, wrong), Err("ABI_RECORD"));
            wrong = shared;
            wrong.arguments = &[M::Context];
            assert_eq!(validate(op, wrong), Err("ABI_ARGUMENTS"));
        }
    }

    #[test]
    fn champ_cleanup_transport_matches_registered_records() {
        use okkama_sparrow::spb17::{Opcode, ResultContext, ServiceKind, Type};
        use okkama_sparrow::spb17::layout::service_layout;
        for &op in Opcode::ALL {
            let Some(shared) = op.cleanup_abi() else { continue };
            let runtime = helper_abi(shared.helper).expect("registered #572 helper");
            assert_eq!(runtime.opcode, Some(op));
            let (size, offsets) = runtime_cleanup_layout(op).unwrap();
            assert_eq!(shared.post_size, size, "{}", op.name());
            assert_eq!(shared.post_offsets, offsets, "{}", op.name());
            validate(op, shared).unwrap();
            for value in [None, Some(Type::Int), Some(Type::Object(0))] {
                let context = ResultContext { value, ..ResultContext::default() };
                for tag in op.tags(&context) {
                    assert_eq!(op.cleanup_post_offsets(tag, &context).unwrap(),
                        runtime_cleanup_offsets(op, tag, &context).unwrap(), "{} {tag:?}", op.name());
                }
            }
        }
        assert_eq!(service_layout(ServiceKind::Closed).words, 0);
        assert_eq!(Opcode::CleanupFinish.cleanup_post_offsets(okkama_sparrow::spb17::Tag::Normal,
            &ResultContext::default()).unwrap(), vec![None, Some(8), Some(16)], "ABI_CLOSED");
        assert_eq!(service_layout(ServiceKind::StableKey).words as usize * 8, size_of::<KeyRef>());
        assert_eq!(service_layout(ServiceKind::DestroyBorrow).words as usize * 8, size_of::<DestroyBorrow>());
    }
}

#[cfg(test)]
mod shared_registry_tests {
    use super::*;
    use okkama_sparrow::spb17::{self as s, MachineArgument as M};

    fn validate(shared: s::Helper) -> Result<(), &'static str> {
        let (_, types) = crate::jit::HELPER_ARGUMENT_TYPES.iter()
            .find(|(name, _)| *name == shared.name).ok_or("HELPER_MISSING")?;
        let mut arguments = vec![M::Context];
        arguments.extend(types.iter().map(|ty| {
            if ty.contains("KeyRef") { M::KeyRef }
            else if ty.starts_with('*') { M::Pointer }
            else { M::Word }
        }));
        if shared.arguments != arguments { return Err("HELPER_SIGNATURE"); }
        let (_, effects) = HELPERS.iter().find(|(name, _)| *name == shared.name).unwrap();
        let flags = effects.iter().fold(0, |flags, effect| flags | match effect {
            Effect::Allocates => s::HELPER_ALLOCATES,
            Effect::Collects => s::HELPER_COLLECTS,
            Effect::Reenters => s::HELPER_REENTERS,
            Effect::Fails => s::HELPER_UNWINDS,
            Effect::Stops => s::HELPER_STOPS,
            Effect::Suspends => s::HELPER_SUSPENDS,
        });
        if flags != shared.effects { return Err("HELPER_EFFECTS"); }
        Ok(())
    }

    #[test]
    fn champ_j1_shared_helper_signatures_effects_schemas_and_mismatch_rejection() {
        for &shared in s::HELPER_TABLE {
            validate(shared).unwrap();
        }
        for &(name, _) in HELPERS.iter().filter(|(name, _)| name.starts_with("champ_")) {
            let shared = s::helper(name).expect("every Champ helper has a shared contract");
            validate(shared).unwrap();
            let mut wrong = shared;
            wrong.arguments = &[];
            assert_eq!(validate(wrong), Err("HELPER_SIGNATURE"));
            wrong = shared;
            wrong.effects ^= s::HELPER_COLLECTS;
            assert_eq!(validate(wrong), Err("HELPER_EFFECTS"));
            let runtime = helper_abi(name).unwrap();
            assert_eq!(runtime.opcode, shared.opcode);
            if let Some(opcode) = shared.opcode {
                let context = crate::champ::runtime_tests::schema_context(Some(opcode));
                for tag in opcode.tags(&context) {
                    assert_eq!(runtime.schemas(&context).unwrap().iter().find(|(kind, _)| *kind == tag)
                        .unwrap().1, opcode.results(tag, &context).unwrap());
                }
                if let Some(cleanup) = opcode.cleanup_abi() {
                    // settle.packet is an internal S3 extension, not P1's empty settle opcode.
                    if name == cleanup.helper {
                        assert_eq!(shared.arguments, cleanup.arguments);
                    }
                }
            }
        }
    }
}
