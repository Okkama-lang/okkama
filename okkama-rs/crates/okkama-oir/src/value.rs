use crate::{Arena, BindingDecl, BindingId, BlockId, DebugLoc, FuncId, InstId, Module, Operation, ScopeId, ServiceType,
    Type, TypeId, TypeTable, ValueId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Body {
    pub cleanup: crate::CleanupMetadata,
    pub high_results: Vec<HighResultExit>,
    pub high_effects: Vec<HighEffectRecord>,
    pub descriptor_bindings: Vec<crate::DescriptorBinding>,
    pub service_schedules: Vec<crate::ServiceSchedule>,
    /// Required in coro/low for an async body and forbidden elsewhere (async ABI D21).
    pub frame_layout: Option<FrameLayout>,
    pub entry: BlockId,
    pub values: Arena<ValueId, ValueData>,
    pub blocks: Arena<BlockId, Block>,
    pub insts: Arena<InstId, Instruction>,
    pub bindings: Arena<BindingId, BindingDecl>,
    pub boundaries: Arena<crate::BoundaryId, crate::Boundary>,
    pub deps: Arena<crate::DepExprId, crate::DepExpr>,
    pub keys: Arena<crate::KeyExprId, crate::KeyExpr>,
    pub selections: Arena<crate::SelectionId, crate::SelectionExpr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueData {
    pub ty: TypeId,
    pub def: Definition,
    pub facts: crate::ValueFacts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Definition {
    BlockParam { block: BlockId, index: u32 },
    InstResult { inst: InstId, index: u16 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    pub results: Vec<ValueId>,
    pub op: Operation,
    pub facts: crate::InstructionFacts,
    pub loc: Option<DebugLoc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub lexical_scope: ScopeId,
    pub params: Vec<ValueId>,
    pub unwind: Option<BlockUnwind>,
    pub insts: Vec<InstId>,
    pub term: Terminator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub block: BlockId,
    pub args: Vec<ValueId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockUnwind {
    /// The implicit failure is the target's last parameter, after these arguments.
    pub target: Target,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Normal, Unwind, Cancel, DeferContinue, DeferReplace(u16),
    ExitSuccess, ExitPending, ExitFailure, FailureSome, CleanupSelected, CleanupDone,
}

/// An invoke result belongs only to its edge, never to the source block's SSA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdgeResult {
    pub outcome: Outcome,
    pub slot: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeArg {
    Value(ValueId),
    Produced(EdgeResult),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvokeEdge {
    pub block: BlockId,
    pub args: Vec<EdgeArg>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Terminator {
    pub kind: TermKind,
    pub loc: Option<DebugLoc>,
}

/// Edge results are interpreted with the invoked operation contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TermKind {
    /// Runtime serial selector. Selection never executes destroy.
    CleanupNext { cleanup: ValueId, selected: InvokeEdge, done: InvokeEdge },
    ExitDispatch {
        close: crate::CleanupCloseId,
        closed: ValueId,
        failure: ValueId,
        exit: ValueId,
        success: Option<InvokeEdge>,
        pending: Option<InvokeEdge>,
        failed: Option<InvokeEdge>,
    },
    FailureDispatch { failure: ValueId, none: Target, some: InvokeEdge },
    Br(Target),
    Cond { condition: ValueId, yes: Target, no: Target },
    Switch { value: ValueId, cases: Vec<SwitchCase>, default: Target },
    InvokeOwn { op: Box<Operation>, normal: InvokeEdge, unwind: InvokeEdge },
    InvokeDefer {
        function: crate::FuncId,
        call: crate::CallSite,
        continue_edge: InvokeEdge,
        replacements: Vec<DeferReplaceEdge>,
        unwind: Option<InvokeEdge>,
    },
    RetDefer { tag: DeferReturnTag, values: Vec<ValueId> },
    Ret { values: Vec<ValueId> },
    Throw(ValueId),
    Raise(ValueId),
    Panic(ValueId),
    Trap { code: crate::TrapCode, message: String },
    Catch { failure: ValueId, ty: TypeId, handler: InvokeEdge, next: Target },
    Lookup { kind: LookupKind, collection: ValueId, key: ValueId, found: InvokeEdge, missing: Target },
    Unreachable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchCase {
    pub value: crate::Literal,
    pub target: Target,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookupKind { Array, Map }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrapCode { Confirm, Range, Empty, Unbound, Unreachable, Pending }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeferReplaceEdge {
    pub label: String,
    pub edge: InvokeEdge,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeferReturnTag { Continue, Replace(String) }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighEdgeSelector {
    Normal(u16),
    Unwind,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighResultExit {
    pub block: BlockId,
    pub edge: HighEdgeSelector,
    pub value: ValueId,
    pub demand: crate::ResultDemand,
    pub prepared: Option<InstId>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighOperationSite { Instruction(InstId), Terminator(BlockId) }
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HighEffectEnvelope {
    pub failures: crate::FailureKindSet,
    pub may_suspend: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighEffectRecord {
    pub site: HighOperationSite,
    pub effects: HighEffectEnvelope,
}

/// Typed frame slots and continuation metadata of an async body (async ABI D21).
/// Slots are abstract: the loader derives machine words from [`FrameLayout::slot_words`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameLayout {
    pub slots: Vec<FrameSlotDecl>,
    /// Continuation 0 is the start.
    pub continuations: Vec<Continuation>,
}
/// One type per slot for the whole function; a slot may be reused for values of that type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSlotDecl {
    pub ty: TypeId,
    pub linear: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Continuation {
    pub site: SuspendSite,
    pub saved: Vec<u32>,
    /// Operand slots of the suspending operation, in operand order.
    pub operands: Vec<u32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuspendSite {
    Start,
    /// The terminator of this block is the suspending operation.
    Terminator(BlockId),
    /// This instruction is the suspending operation.
    Instruction(InstId),
}
/// Machine shape of one frame slot. Offsets are bytes from the slot start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotWords {
    pub words: u32,
    /// Traced references of a service slot; `None` defers to the register repr of a value.
    pub references: Option<&'static [u32]>,
}
impl FrameLayout {
    /// Raw words per lexical scope of the function: holder id and owner id (D21, review B2).
    pub const SCOPE_AREA_WORDS: u32 = 2;

    /// Mirrors `spb17::layout::service_layout` for service kinds; values take one register word.
    /// The 5-word `destroy-borrow` kind has no IR slot type.
    pub fn slot_words(types: &TypeTable, ty: TypeId) -> Option<SlotWords> {
        let words = match types.get(ty).ok()? {
            Type::Frame(_) => SlotWords { words: 0, references: Some(&[]) },
            Type::Service(ServiceType::Deps | ServiceType::Closed | ServiceType::UniqueCache) => {
                SlotWords { words: 0, references: Some(&[]) }
            }
            Type::Service(ServiceType::StableKey) => SlotWords { words: 2, references: Some(&[0]) },
            Type::Service(_) => SlotWords { words: 1, references: Some(&[0]) },
            _ => SlotWords { words: 1, references: None },
        };
        Some(words)
    }
    /// Words of the typed slot area, before the scope area.
    pub fn slot_area_words(&self, types: &TypeTable) -> Option<u32> {
        let mut total = 0_u32;
        for slot in &self.slots {
            total = total.checked_add(Self::slot_words(types, slot.ty)?.words)?;
        }
        Some(total)
    }
    /// Two Raw words for every lexical scope of `function`, after the slots.
    pub fn scope_area_words(module: &Module, function: FuncId) -> Option<u32> {
        let scopes = module.scopes.iter().filter(|(_, scope)| scope.function == function).count();
        u32::try_from(scopes).ok()?.checked_mul(Self::SCOPE_AREA_WORDS)
    }
    /// The frame's machine area: typed slots followed by the scope area.
    pub fn machine_words(&self, module: &Module, function: FuncId) -> Option<u32> {
        self.slot_area_words(&module.types)?.checked_add(Self::scope_area_words(module, function)?)
    }
}
