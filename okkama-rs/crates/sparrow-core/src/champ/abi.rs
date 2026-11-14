//! Record layouts used by the helper registry and future service-slot stack-map lowering.
//! Pointer arguments designate caller storage; they are never themselves heap references.
use super::*;
use crate::jit::{Effect, HELPERS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperClass {
    L,
    G,
    X,
}
/// Classes derive from the same effects table that checks Rust and CLIF signatures.
pub fn helper_class(name: &str) -> Option<HelperClass> {
    let (_, effects) = HELPERS.iter().find(|(entry, _)| *entry == name && name.starts_with("champ_"))?;
    Some(if effects.contains(&Effect::Reenters) {
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
    record!(DestroyPost; [failure]; []),
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
            offset: None, message: format!("{}: no payload schema for {tag:?}", self.name),
        };
        let opcode = self.opcode.ok_or_else(invalid)?;
        if opcode.call_abi().is_some() {
            return opcode.call_post_offsets(tag, context);
        }
        let schema = opcode.results(tag, context)?;
        let erased = |slot: &ProducedType| matches!(slot, ProducedType::Service(ServiceKind::Deps));
        let count = schema.iter().filter(|slot| !erased(slot)).count();
        let offsets = match (opcode, tag) {
            (O::OwnSplitChecked | O::ScopeSplit, Tag::Unwind) => vec![0, 16],
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
        "champ_scope_split" => ("ScopeSplitPost", Some(O::ScopeSplit)),
        "champ_scope_transfer" => ("TransferPost", Some(O::ScopeTransfer)),
        "champ_exit_prepare" => ("PendingSlot", Some(O::ExitPrepare)),
        "champ_exit_extend" => ("PendingSlot", Some(O::ExitExtend)),
        "champ_exit_replace" => ("ReplacePost", Some(O::ExitReplace)),
        "champ_exit_commit" => ("CommitPost", Some(O::ExitCommit)),
        "champ_exit_abort" => ("CleanupSlot", Some(O::ExitAbort)),
        "champ_cleanup_failure" => ("CleanupSlot", Some(O::CleanupFailure)),
        "champ_call_owned" => ("CallPost", Some(O::InvokeCallOwn)),
        "champ_invoke_defer" => ("CallPost", Some(O::InvokeDefer)),
        "champ_entry_reject" => ("CallPost", None),
        "champ_destroy_claim" => ("DestroyBorrow", None),
        "champ_destroy_invoke" => ("DestroyPost", None),
        "champ_destroy_finish" | "champ_failure_capture" => ("", None),
        _ => return None,
    };
    Some(HelperAbi {
        name,
        class: helper_class(name)?,
        effects,
        output: RECORD_LAYOUTS.iter().find(|layout| layout.name == record),
        opcode,
    })
}

// Keep the always-built SPB transport registry mechanically tied to the actual #572 records.
const _: () = {
    use okkama_sparrow::spb17::Opcode;
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
