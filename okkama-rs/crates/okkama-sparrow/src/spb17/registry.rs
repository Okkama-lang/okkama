use super::{DeferLabel, Error, ServiceKind, Type, error};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tag {
    Normal,
    Unwind,
    Selected,
    Done,
    Continue,
    Replace(u32),
    Success,
    Pending,
    Failure,
    None,
    Some,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProducedType {
    Value(Type),
    Service(ServiceKind),
}

/// Derived from verified entry/close/transfer contracts, never a wire schema.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResultContext {
    pub value: Option<Type>,
    pub between: bool,
    pub may_fail: bool,
    pub labels: Vec<DeferLabel>,
    pub reserve: bool,
    pub active_failure: bool,
}

macro_rules! registry {
    ($( $id:literal => $variant:ident, $name:literal; )*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Opcode {
            $( $variant, )*
        }
        impl Opcode {
            pub const ALL: &'static [Self] = &[$( Self::$variant, )*];
            pub fn id(self) -> u32 {
                match self { $( Self::$variant => $id, )* }
            }
            pub fn name(self) -> &'static str {
                match self { $( Self::$variant => $name, )* }
            }
            pub fn from_id(id: u32) -> Result<Self, Error> {
                match id {
                    $( $id => Ok(Self::$variant), )*
                    0 | 19 | 25 | 34..=36 => Err(error(format!("reserved Champ opcode {id}"))),
                    _ => Err(error(format!("unknown Champ opcode {id}"))),
                }
            }
        }
    };
}

registry! {
    1 => ScopeBegin, "scope.begin";
    2 => OwnEmpty, "own.empty";
    3 => OwnMove, "own.move";
    4 => OwnJoin, "own.join";
    5 => OwnAdmit, "own.admit";
    6 => OwnAccept, "own.accept";
    7 => OwnBind, "own.bind";
    8 => OwnRebind, "own.rebind";
    9 => OwnStage, "own.stage";
    10 => OwnSplit, "own.split";
    11 => OwnSplitChecked, "own.split.checked";
    12 => ScopeSplit, "scope.split";
    13 => ScopeTransfer, "scope.transfer";
    14 => DeferRegister, "defer.register";
    15 => ExitPrepare, "exit.prepare";
    16 => ExitExtend, "exit.extend";
    17 => ExitReplace, "exit.replace";
    18 => ExitCommit, "exit.commit";
    20 => CleanupBegin, "cleanup.begin";
    21 => CleanupSettle, "cleanup.settle";
    22 => CleanupPlan, "cleanup.plan";
    23 => CleanupNext, "cleanup.next";
    24 => CleanupFailure, "cleanup.failure";
    26 => CleanupFinish, "cleanup.finish";
    27 => DestroyDirect, "destroy.direct";
    28 => InvokeCallOwn, "invoke.call.own";
    29 => InvokeDefer, "invoke.defer";
    30 => RetOwn, "ret.own";
    31 => RetDefer, "ret.defer";
    32 => ExitDispatch, "exit.dispatch";
    33 => OwnCreateCommit, "own.create_commit";
    37 => ExitAbort, "exit.abort";
    38 => CleanupDestroy, "cleanup.destroy";
    39 => FailureDispatch, "failure.dispatch";
    40 => DepsOf, "deps.of";
    41 => InvokeObserve, "invoke.observe";
    42 => ArrayGet, "array.get";
    43 => Fail, "fail";
    44 => Evaluate, "evaluate";
    45 => Catch, "catch";
    46 => DepsEmpty, "deps.empty";
    96 => InvokeDispatch, "invoke.dispatch";
    97 => InvokeDispatchOwn, "invoke.dispatch.own";
}

impl Opcode {
    /// Ordered abstract slots, including the zero-word deps position (§4.5).
    pub fn results(self, tag: Tag, context: &ResultContext) -> Result<Vec<ProducedType>, Error> {
        use Opcode::*;
        use ProducedType::{Service as S, Value};
        use ServiceKind::*;
        let valued = || {
            let mut slots = Vec::new();
            if let Some(ty) = context.value {
                slots.push(Value(ty));
            }
            slots.extend([S(Packet), S(Deps)]);
            slots
        };
        let slots = match (self, tag) {
            (ScopeBegin | OwnAdmit | OwnAccept | OwnBind | OwnRebind | DeferRegister, Tag::Normal) => {
                vec![S(ScopeState)]
            }
            (DepsOf | DepsEmpty, Tag::Normal) => vec![S(Deps)],
            (OwnEmpty | OwnMove | OwnJoin | OwnCreateCommit, Tag::Normal) => vec![S(Packet)],
            (OwnStage, Tag::Normal) => {
                if context.between { vec![S(ScopeState), S(ScopeState)] } else { vec![S(ScopeState)] }
            }
            (OwnSplit | OwnSplitChecked, Tag::Normal) => vec![S(Packet), S(Packet), S(Deps)],
            (OwnSplitChecked, Tag::Unwind) => vec![S(Packet), Value(Type::Failure)],
            (ScopeSplit, Tag::Normal) => vec![S(ScopeState), S(Packet), S(Deps)],
            (ScopeSplit, Tag::Unwind) => vec![S(ScopeState), Value(Type::Failure)],
            (ScopeTransfer, Tag::Normal | Tag::Unwind) => {
                let mut slots = vec![S(ScopeState)];
                if context.between {
                    slots.push(S(ScopeState));
                }
                if tag == Tag::Unwind {
                    slots.push(Value(Type::Failure));
                }
                slots
            }
            (ExitPrepare | ExitExtend, Tag::Normal) => vec![S(Pending)],
            (ExitReplace, Tag::Normal) => vec![S(Pending), S(Packet)],
            (ExitCommit, Tag::Normal) => valued(),
            (ExitAbort | CleanupBegin | CleanupSettle | CleanupPlan | CleanupFailure, Tag::Normal) => vec![S(Cleanup)],
            (CleanupNext, Tag::Selected) => vec![S(Cleanup), S(StableKey)],
            (CleanupNext, Tag::Done) => vec![S(Cleanup)],
            (DestroyDirect | CleanupDestroy, Tag::Normal) => vec![S(Cleanup), S(FailureState)],
            (CleanupFinish, Tag::Normal) => vec![S(Closed), S(FailureState), S(ExitState)],
            (InvokeObserve | InvokeDispatch, Tag::Normal) => context.value.into_iter().map(Value).collect(),
            (InvokeObserve | InvokeDispatch, Tag::Unwind) if context.may_fail => vec![Value(Type::Failure)],
            (ArrayGet, Tag::Normal) => context.value.into_iter().map(Value).collect(),
            (ArrayGet, Tag::Unwind) => vec![Value(Type::Failure)],
            (Fail, Tag::Unwind) => vec![Value(Type::Failure)],
            (Evaluate, Tag::Normal) => context.value.into_iter().map(Value).collect(),
            (Evaluate, Tag::Unwind) => vec![Value(Type::Failure)],
            (Catch, Tag::Some) => context.value.into_iter().map(Value).collect(),
            (Catch, Tag::None) => vec![],
            (InvokeCallOwn | InvokeDispatchOwn, Tag::Normal) => valued(),
            (InvokeCallOwn | InvokeDispatchOwn | InvokeDefer, Tag::Unwind) if context.may_fail => {
                vec![Value(Type::Failure)]
            }
            (InvokeDefer, Tag::Continue) => vec![],
            (InvokeDefer, Tag::Replace(label)) => {
                let declared = context.labels.iter().find(|declared| declared.label == label)
                    .ok_or_else(|| error("undeclared defer replacement label"))?;
                let mut slots = Vec::new();
                if let Some(ty) = declared.result {
                    slots.push(Value(ty));
                }
                slots.extend([S(Packet), S(Deps)]);
                slots
            }
            (RetOwn | RetDefer, Tag::Normal) => vec![],
            (ExitDispatch, Tag::Success) if !context.reserve && !context.active_failure => vec![S(Closed)],
            (ExitDispatch, Tag::Pending) if context.reserve && !context.active_failure => vec![S(Closed), S(Pending)],
            (ExitDispatch, Tag::Failure) if context.may_fail || context.active_failure => {
                vec![S(Closed), Value(Type::Failure)]
            }
            (FailureDispatch, Tag::None) => vec![],
            (FailureDispatch, Tag::Some) => vec![Value(Type::Failure)],
            _ => return Err(error(format!("invalid outcome {tag:?} for {}", self.name()))),
        };
        Ok(slots)
    }

    pub fn tags(self, context: &ResultContext) -> Vec<Tag> {
        use Opcode::*;
        match self {
            Catch => vec![Tag::Some, Tag::None],
            Fail => vec![Tag::Unwind],
            OwnSplitChecked | ScopeSplit | ScopeTransfer | ArrayGet | Evaluate => vec![Tag::Normal, Tag::Unwind],
            InvokeObserve | InvokeDispatch if context.may_fail => vec![Tag::Normal, Tag::Unwind],
            InvokeCallOwn | InvokeDispatchOwn => {
                if context.may_fail { vec![Tag::Normal, Tag::Unwind] } else { vec![Tag::Normal] }
            }
            InvokeDefer => {
                let mut tags = vec![Tag::Continue];
                tags.extend(context.labels.iter().map(|label| Tag::Replace(label.label)));
                if context.may_fail {
                    tags.push(Tag::Unwind);
                }
                tags
            }
            CleanupNext => vec![Tag::Selected, Tag::Done],
            ExitDispatch => {
                let mut tags = Vec::new();
                if !context.active_failure {
                    tags.push(if context.reserve { Tag::Pending } else { Tag::Success });
                }
                if context.may_fail || context.active_failure {
                    tags.push(Tag::Failure);
                }
                tags
            }
            FailureDispatch => vec![Tag::None, Tag::Some],
            _ => vec![Tag::Normal],
        }
    }

    pub fn is_terminator(self) -> bool {
        matches!(self, Self::OwnSplitChecked | Self::ScopeSplit | Self::ScopeTransfer |
            Self::InvokeCallOwn | Self::InvokeObserve | Self::InvokeDefer | Self::CleanupNext | Self::ExitDispatch |
            Self::InvokeDispatch | Self::InvokeDispatchOwn |
            Self::FailureDispatch | Self::RetOwn | Self::RetDefer | Self::ArrayGet | Self::Fail | Self::Evaluate
            | Self::Catch)
    }
}

/// Shared machine transport for #572 S2 CallOwn/CallPost. No runtime deps and no second outcome tag.
/// Runtime repr(C) records assert these offsets; codegen/JIT consume this registry, not a local ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallAbi {
    pub helper: &'static str,
    pub input_offset: usize,
    pub entry_offset: usize,
    pub output_offset: usize,
    pub context_offset: usize,
    pub cleanup_offset: usize,
    pub call_own_size: usize,
    pub post_size: usize,
    pub post_value_offset: usize,
    pub post_packet_offset: usize,
    pub post_failure_offset: usize,
}

impl Opcode {
    pub const fn call_abi(self) -> Option<CallAbi> {
        let helper = match self {
            // A dispatch selects the implementation's entry, then transports it as an owned call.
            Self::InvokeCallOwn | Self::InvokeDispatchOwn => "champ_call_owned",
            Self::InvokeDefer => "champ_invoke_defer",
            _ => return None,
        };
        Some(CallAbi {
            helper, input_offset: 0, entry_offset: 8, output_offset: 16,
            context_offset: 24, cleanup_offset: 32, call_own_size: 40, post_size: 24,
            post_value_offset: 0, post_packet_offset: 8, post_failure_offset: 16,
        })
    }

    /// Abstract slot → CallPost byte offset. None is erased deps; Post is never a helper-call root.
    pub fn call_post_offsets(self, tag: Tag, context: &ResultContext) -> Result<Vec<Option<usize>>, Error> {
        let abi = self.call_abi().ok_or_else(|| error("operation does not use CallOwn"))?;
        self.results(tag, context)?.into_iter().map(|slot| Ok(match slot {
            ProducedType::Service(ServiceKind::Deps) => None,
            ProducedType::Service(ServiceKind::Packet) => Some(abi.post_packet_offset),
            ProducedType::Value(Type::Failure) if tag == Tag::Unwind => Some(abi.post_failure_offset),
            ProducedType::Value(_) => Some(abi.post_value_offset),
            _ => return Err(error("invalid CallPost slot")),
        })).collect()
    }
}

/// Fixed #572 machine ABI. KeyRef is passed as two words, never a pointer-sized surrogate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineArgument { Context, Pointer, Word, KeyRef }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CleanupAbi {
    pub helper: &'static str,
    pub arguments: &'static [MachineArgument],
    pub post_size: usize,
    pub post_offsets: &'static [usize],
}

impl Opcode {
    pub const fn cleanup_abi(self) -> Option<CleanupAbi> {
        use MachineArgument::{Context as C, KeyRef as K, Pointer as P, Word as W};
        let (helper, arguments, post_size, post_offsets): (_, &[_], _, &[_]) = match self {
            Self::CleanupBegin => ("champ_cleanup_begin", &[C, P, P], 8, &[0]),
            Self::CleanupSettle => ("champ_cleanup_settle_empty", &[C, P, P], 8, &[0]),
            Self::CleanupPlan => ("champ_cleanup_plan", &[C, P, W, P], 8, &[0]),
            Self::CleanupNext => ("champ_cleanup_next", &[C, P, P], 24, &[0, 8]),
            Self::CleanupDestroy => ("champ_cleanup_destroy", &[C, P, K, P], 16, &[0, 8]),
            Self::DestroyDirect => ("champ_destroy_direct", &[C, P, K, W, P], 16, &[0, 8]),
            Self::CleanupFailure => ("champ_cleanup_failure", &[C, P, W, P], 8, &[0]),
            Self::CleanupFinish => ("champ_cleanup_finish", &[C, P, P], 24, &[0, 8, 16]),
            Self::ExitPrepare => ("champ_exit_prepare", &[C, P, P], 8, &[0]),
            Self::ExitCommit => ("champ_exit_commit", &[C, P, P, P], 16, &[0, 8]),
            Self::ExitExtend => ("champ_exit_extend", &[C, P, P, P], 8, &[0]),
            Self::ExitReplace => ("champ_exit_replace", &[C, P, P, P], 16, &[0, 8]),
            Self::ExitAbort => ("champ_exit_abort", &[C, P, P, P], 8, &[0]),
            _ => return None,
        };
        Some(CleanupAbi { helper, arguments, post_size, post_offsets })
    }
}

impl Opcode {
    /// Operation-specific Post payloads. Proof-only deps retain their abstract ordinal without a word.
    pub fn cleanup_post_offsets(self, tag: Tag, context: &ResultContext) -> Result<Vec<Option<usize>>, Error> {
        let abi = self.cleanup_abi().ok_or_else(|| error("operation has no cleanup ABI"))?;
        let schema = self.results(tag, context)?;
        if self == Self::ExitCommit {
            return Ok(if context.value.is_some() { vec![Some(0), Some(8), None] } else { vec![Some(8), None] });
        }
        schema.iter().enumerate().map(|(index, ty)| {
            if matches!(ty, ProducedType::Service(ServiceKind::Deps | ServiceKind::Closed)) {
                Ok(None)
            } else {
                abi.post_offsets.get(index).copied().map(Some).ok_or_else(|| error("missing Post offset"))
            }
        }).collect()
    }
}

/// The executable helper contract. Rust checks its extern signature against this shared transport;
/// the JIT derives effects from it. Neither an SPB artifact nor a host entry can redefine this table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Helper {
    pub name: &'static str,
    pub arguments: &'static [MachineArgument],
    pub effects: u8,
    pub opcode: Option<Opcode>,
}

pub const HELPER_ALLOCATES: u8 = 1;
pub const HELPER_COLLECTS: u8 = 2;
pub const HELPER_REENTERS: u8 = 4;
pub const HELPER_UNWINDS: u8 = 8;
pub const HELPER_STOPS: u8 = 16;

impl Opcode {
    pub const fn helper(self) -> Option<Helper> {
        let name = if let Some(abi) = self.cleanup_abi() {
            abi.helper
        } else if let Some(abi) = self.call_abi() {
            abi.helper
        } else {
            match self {
                Self::ScopeBegin => "champ_scope_begin",
                Self::OwnEmpty => "champ_own_empty",
                Self::OwnMove => "champ_own_move",
                Self::OwnJoin => "champ_own_join",
                Self::OwnAdmit => "champ_own_admit",
                Self::OwnAccept => "champ_own_accept",
                Self::OwnBind => "champ_own_bind",
                Self::OwnRebind => "champ_own_rebind",
                Self::OwnStage => "champ_own_stage",
                Self::OwnSplit => "champ_own_split_static",
                Self::OwnSplitChecked => "champ_own_split_checked",
                Self::ScopeSplit => "champ_scope_split",
                Self::ScopeTransfer => "champ_scope_transfer",
                Self::OwnCreateCommit => "champ_own_create_commit",
                // These forms lower to CFG, return transport, or parallel slot moves.
                _ => return None,
            }
        };
        helper(name)
    }
}

use MachineArgument::{Context as C, Pointer as P, Word as W, KeyRef as K};
const L: u8 = HELPER_ALLOCATES | HELPER_STOPS;
const G: u8 = L | HELPER_UNWINDS;
const X: u8 = L | HELPER_COLLECTS | HELPER_REENTERS;

macro_rules! helper_registry {
    ($( $name:literal => $args:expr, $effects:expr, $op:expr; )*) => {
        pub const HELPER_TABLE: &[Helper] = &[$(Helper {
            name: $name, arguments: $args, effects: $effects, opcode: $op,
        }),*];
    };
}
use Opcode as O;
helper_registry! {
        "champ_scope_begin" => &[C, P, P], L, Some(O::ScopeBegin);
        "champ_own_empty" => &[C, P], L, Some(O::OwnEmpty);
        "champ_own_move" => &[C, P, P], L, Some(O::OwnMove);
        "champ_own_join" => &[C, P, W, P], L, Some(O::OwnJoin);
        "champ_own_create_commit" => &[C, W, W, W, P], L, Some(O::OwnCreateCommit);
        "champ_own_admit" => &[C, P, P, P], L, Some(O::OwnAdmit);
        "champ_own_accept" => &[C, P, P, P], L, Some(O::OwnAccept);
        "champ_own_bind" => &[C, P, W, W, P, P], L, Some(O::OwnBind);
        "champ_own_rebind" => &[C, P, W, W, P, P], L, Some(O::OwnRebind);
        "champ_own_stage" => &[C, P, P], L, Some(O::OwnStage);
        "champ_own_split_static" => &[C, P, W, P], L, Some(O::OwnSplit);
        "champ_own_split_checked" => &[C, P, P, P], G, Some(O::OwnSplitChecked);
        "champ_scope_split" => &[C, P, P, P], G, Some(O::ScopeSplit);
        "champ_scope_split_admissible" => &[C, P, P, P], G, Some(O::ScopeSplit);
        "champ_scope_transfer" => &[C, P, P, P], G, Some(O::ScopeTransfer);
        "champ_exit_prepare" => &[C, P, P], L, Some(O::ExitPrepare);
        "champ_exit_extend" => &[C, P, P, P], L, Some(O::ExitExtend);
        "champ_exit_replace" => &[C, P, P, P], L, Some(O::ExitReplace);
        "champ_exit_commit" => &[C, P, P, P], L, Some(O::ExitCommit);
        "champ_exit_abort" => &[C, P, P, P], L, Some(O::ExitAbort);
        "champ_cleanup_begin" => &[C, P, P], L, Some(O::CleanupBegin);
        "champ_cleanup_settle_empty" => &[C, P, P], HELPER_STOPS, Some(O::CleanupSettle);
        "champ_cleanup_settle_packet" => &[C, P, P, W, P], L, Some(O::CleanupSettle);
        "champ_cleanup_plan" => &[C, P, W, P], L, Some(O::CleanupPlan);
        "champ_cleanup_next" => &[C, P, P], HELPER_STOPS, Some(O::CleanupNext);
        "champ_cleanup_destroy" => &[C, P, K, P], X, Some(O::CleanupDestroy);
        "champ_destroy_direct" => &[C, P, K, W, P], X, Some(O::DestroyDirect);
        "champ_cleanup_failure" => &[C, P, W, P], L, Some(O::CleanupFailure);
        "champ_cleanup_finish" => &[C, P, P], L, Some(O::CleanupFinish);
        "champ_call_owned" => &[C, W, P, W, P, P], X | HELPER_UNWINDS, Some(O::InvokeCallOwn);
        "champ_invoke_defer" => &[C, W, P, W, P, P, P], X | HELPER_UNWINDS, Some(O::InvokeDefer);
        "champ_entry_reject" => &[C, P], X | HELPER_UNWINDS, None;
        "champ_destroy_claim" => &[C, P, K, W, W, P], L, None;
        "champ_destroy_invoke" => &[C, P, P, W, P], X, None;
        "champ_destroy_finish" => &[C, P, P], HELPER_STOPS, None;
        "champ_failure_capture" => &[C, P], HELPER_STOPS, None;
        "champ_machine_secondary" => &[C, W, W], L, None;
        "champ_machine_caused" => &[C, W, W], L, None;
        "champ_machine_fail" => &[C, W, W], L, None;
        "champ_machine_range" => &[C, W, W, W, W], G | HELPER_COLLECTS, None;
        "champ_machine_convert" => &[C, W, W, W], G | HELPER_COLLECTS, None;
        "champ_machine_rat_order" => &[C, W, W], 0, None;
        "champ_machine_value_eq" => &[C, W, W, W], G, None;
        "champ_machine_variant_get" => &[C, W, W, W], G, None;
        "champ_machine_role_narrow" => &[C, W, W], G, None;
        "champ_machine_catch" => &[C, W, W], 0, None;
        "champ_machine_plain_packet" => &[C, P], HELPER_STOPS, None;
        "champ_machine_text_order" => &[C, W, W], 0, None;
        "champ_machine_text_len" => &[C, W], 0, None;
        "champ_machine_external" => &[C, W, P, P, W], X | HELPER_UNWINDS, None;
        "champ_machine_scope" => &[C, W, W, W, P], L, None;
        "champ_machine_invalid" => &[C], HELPER_STOPS, None;
        "champ_machine_destination" => &[C, W, P], L, None;
        "champ_machine_serial" => &[C, W], 0, None;
        "champ_machine_fatal_stack" => &[C], HELPER_STOPS, None;
        "champ_machine_cleanup_mode" => &[C], 0, None;
}

pub const fn helper(name: &str) -> Option<Helper> {
    let mut index = 0;
    while index < HELPER_TABLE.len() {
        let entry = HELPER_TABLE[index];
        if equal(name.as_bytes(), entry.name.as_bytes()) {
            return Some(entry);
        }
        index += 1;
    }
    None
}

const fn equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}
