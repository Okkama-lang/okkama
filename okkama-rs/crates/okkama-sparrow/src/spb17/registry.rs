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
            (InvokeCallOwn, Tag::Normal) => valued(),
            (InvokeCallOwn | InvokeDefer, Tag::Unwind) if context.may_fail => vec![Value(Type::Failure)],
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
            OwnSplitChecked | ScopeSplit | ScopeTransfer => vec![Tag::Normal, Tag::Unwind],
            InvokeCallOwn => {
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
            Self::InvokeCallOwn | Self::InvokeDefer | Self::CleanupNext | Self::ExitDispatch |
            Self::FailureDispatch | Self::RetOwn | Self::RetDefer)
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
            Self::InvokeCallOwn => "champ_call_owned",
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
