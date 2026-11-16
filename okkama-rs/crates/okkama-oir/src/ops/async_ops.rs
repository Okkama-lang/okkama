use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TakeBoundary { Existing { owner: ScopeId }, Temporary { awaiting: ScopeId } }
/// Static evidence, never a runtime probe or a use-count heuristic. #564 must
/// check every assertion against the whole function, and repeat after coro.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniqueCacheProof {
    pub future: ValueId,
    pub scope: ScopeId,
    pub region: Vec<BlockId>,
    pub evidence: UniqueCacheEvidence,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreationOrigin {
    Instruction(InstId),
    Invoke { block: BlockId, outcome: Outcome, slot: u16 },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniqueCacheEvidence {
    pub creation: CreationOrigin,
    pub aliases: Vec<ValueId>,
    pub frame_slots: Vec<u32>,
    pub no_external_storage: bool,
    pub no_other_waiters: bool,
    pub no_live_views: bool,
    pub no_collector_retention: bool,
    pub scheduler_cannot_publish: bool,
}
impl OperandSource for UniqueCacheProof {
    fn append(&self, _: OperandMode, out: &mut Vec<ReferencedOperand>) {
        self.future.append(OperandMode::Observe, out);
        self.evidence.aliases.append(OperandMode::Observe, out);
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameSlot { pub slot: u32, pub value: ValueId, pub mode: OperandMode }
impl OperandSource for FrameSlot {
    fn append(&self, _: OperandMode, out: &mut Vec<ReferencedOperand>) { self.value.append(self.mode, out); }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedSlot { pub slot: u32, pub ty: TypeId, pub linear: bool }

/// Coro keeps `Deferred` and `External` unchanged (async ABI C-1, D22): the
/// runtime materializes the frame inside `fut.new`. `Frame` stays in the data
/// model but is not produced in 0.6.0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FutureEnvironment {
    Deferred { function: FuncId, arguments: Vec<ValueId>, descriptors: Vec<ValueId> },
    /// An async native or intrinsic run by a runtime driver (D22).
    External { function: FuncId, arguments: Vec<ValueId>, descriptors: Vec<ValueId> },
    Frame(ValueId),
}
impl FutureEnvironment {
    /// The function whose body or driver runs the Future, if it is named.
    pub fn function(&self) -> Option<FuncId> {
        match self {
            Self::Deferred { function, .. } | Self::External { function, .. } => Some(*function),
            Self::Frame(_) => None,
        }
    }
}
impl OperandSource for FutureEnvironment {
    fn append(&self, _: OperandMode, out: &mut Vec<ReferencedOperand>) {
        match self {
            Self::Deferred { arguments, descriptors, .. } | Self::External { arguments, descriptors, .. } => {
                arguments.append(OperandMode::Borrow, out);
                descriptors.append(OperandMode::Observe, out);
            }
            Self::Frame(frame) => frame.append(OperandMode::Consume, out),
        }
    }
}

/// Outcome of `fut.publish` (async ABI C-2). A void success has no value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishOutcome {
    Success { value: Option<ValueId>, packet: ValueId, deps: ValueId },
    Failure(ValueId),
}
impl PublishOutcome {
    pub fn packet(&self) -> Option<ValueId> {
        match self {
            Self::Success { packet, .. } => Some(*packet),
            Self::Failure(_) => None,
        }
    }
}
impl OperandSource for PublishOutcome {
    fn append(&self, _: OperandMode, out: &mut Vec<ReferencedOperand>) {
        match self {
            Self::Success { value, packet, deps } => {
                value.append(OperandMode::Borrow, out);
                packet.append(OperandMode::Consume, out);
                deps.append(OperandMode::Observe, out);
            }
            Self::Failure(failure) => failure.append(OperandMode::Observe, out),
        }
    }
}

/// Operand forms of `cache.rehome` (async ABI C-4, OQ-1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RehomeForm {
    /// The pre-C-4 form over one cache-state token; kept unchanged until R1 retires it.
    State(ValueId),
    /// own: moves the Future's duties from the scope state into the pending reserve.
    Scope { state: ValueId, pending: ValueId, future: ValueId },
    /// coro/low: consumes the duty tokens into the pending reserve.
    Token { pending: ValueId, cache: ValueId, task: Option<ValueId> },
}
impl OperandSource for RehomeForm {
    fn append(&self, _: OperandMode, out: &mut Vec<ReferencedOperand>) {
        match self {
            Self::State(state) => state.append(OperandMode::Consume, out),
            Self::Scope { state, pending, future } => {
                state.append(OperandMode::Consume, out);
                pending.append(OperandMode::Consume, out);
                future.append(OperandMode::Observe, out);
            }
            Self::Token { pending, cache, task } => {
                pending.append(OperandMode::Consume, out);
                cache.append(OperandMode::Consume, out);
                task.append(OperandMode::Consume, out);
            }
        }
    }
}

/// The destination of a `cache.rehome` (async ABI C-4, ruling of #601 R1): the
/// destination of the exit whose pending reserve receives the duties, the same
/// one its `exit.commit` names. A nested scope's exit lands in a scope of the
/// same function; a returned Future reaches the caller (D25.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RehomeTarget { Scope(ScopeId), Caller }

/// A service entry `ServiceOf{holder, kind}` of a packet (async ABI D25.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceEntry { pub holder: ValueId, pub kind: ServiceKeyKind }
impl OperandSource for ServiceEntry {
    fn append(&self, _: OperandMode, out: &mut Vec<ReferencedOperand>) {
        self.holder.append(OperandMode::Observe, out);
    }
}

/// Registry name of the `await.unwind` adapter operation (async ABI C-3).
pub const AWAIT_UNWIND_ENTRY: &str = "@oir.await.unwind";

operation_group! { AsyncOp {
    Await { future: ValueId, result: Option<TypeId> }
        => ("await", [future: Borrow], Rule::Await { future: *future, result: *result, take: None, shielded: false });
    AwaitShielded { future: ValueId, result: Option<TypeId> }
        => ("await.shielded", [future: Borrow], Rule::Await { future: *future, result: *result, take: None, shielded: true });
    AwaitTake { future: ValueId, result: Option<TypeId>, unique: ValueId, boundary: TakeBoundary }
        => ("await.take", [future: Borrow, unique: Consume], Rule::Await { future: *future, result: *result, take: Some((*unique, *boundary)), shielded: false });
    UniqueCacheProof { proof: UniqueCacheProof }
        => ("unique_cache.proof", [proof: Observe], Rule::UniqueProof(proof.clone()));
    CacheRehome { form: RehomeForm, target: RehomeTarget, protocol: ProtocolId }
        => ("cache.rehome", [form: Consume], match form {
            RehomeForm::State(state) => Rule::Protocol { state: *state, owner: None, packet: None,
                protocol: *protocol, transition: ProtocolTransition::Rehome }
                .requires(Postcondition::CacheRehome(*target)),
            _ => Rule::Rehome { form: form.clone(), target: *target, protocol: *protocol },
        });
    FutureSpawn { future: ValueId, state: Option<ValueId> }
        => ("fut.spawn", [future: Borrow, state: Consume], Rule::Spawn { state: *state });
    FutureCancel { future: ValueId }
        => ("fut.cancel", [future: Observe], Rule::Intent);
    FutureDone { future: ValueId }
        => ("fut.done", [future: Observe], Rule::Data { ty: ResultType::Scalar(ScalarType::Bool), trap: false });
    FutureCancelled { future: ValueId }
        => ("fut.cancelled", [future: Observe], Rule::Data { ty: ResultType::Scalar(ScalarType::Bool), trap: false });
    FutureNew { packet: ValueId, deps: ValueId, owner_state: ValueId, environment: FutureEnvironment,
        result: Option<TypeId>, owner: ScopeId }
        => ("fut.new", [packet: Consume, deps: Observe, owner_state: Consume, environment: Borrow],
            Rule::FutureNew { environment: environment.clone(), packet: *packet, owner: *owner_state,
            result: *result });
    FuturePublish { frame: ValueId, outcome: PublishOutcome }
        => ("fut.publish", [frame: Consume, outcome: Consume], Rule::Publish { frame: *frame,
            packet: outcome.packet() });
    AwaitUnwind { state: ValueId, failure: ValueId, task: ValueId, receipt: ValueId, protocol: ProtocolId }
        => ("await.unwind", [state: Consume, failure: Observe, task: Consume, receipt: Consume],
            Rule::AwaitUnwind { state: *state, task: *task, receipt: *receipt, protocol: *protocol });
    ServiceMaterialize { packet: ValueId, entry: ServiceEntry }
        => ("service.materialize", [packet: Consume, entry: Observe],
            Rule::Materialize { packet: *packet, kind: entry.kind });
    ServiceAbsorb { packet: ValueId, token: ValueId }
        => ("service.absorb", [packet: Consume, token: Consume], Rule::Absorb { packet: *packet, token: *token });
    FutureSettle { task: ValueId, protocol: ProtocolId }
        => ("fut.settle", [task: Consume], Rule::TaskSettle { task: *task, protocol: *protocol });
    FrameAlloc { function: FuncId }
        => ("frame.alloc", [], Rule::Data { ty: ResultType::Frame(*function), trap: false });
    FrameLoad { frame: ValueId, slot: u32, ty: TypeId }
        => ("frame.load", [frame: Borrow], Rule::Data { ty: ResultType::Exact(*ty), trap: false }.requires(Postcondition::NonLinearSlotOnly));
    FrameStore { frame: ValueId, slot: u32, value: ValueId }
        => ("frame.store", [frame: Borrow, value: Borrow], Rule::Intent.requires(Postcondition::NonLinearSlotOnly));
    FrameSave { frame: ValueId, slots: Vec<FrameSlot>, continuation: u32 }
        => ("frame.save", [frame: Consume, slots: Observe], Rule::FrameSave { frame: *frame, slots: slots.clone() });
    FrameResume { frame: ValueId, continuation: u32, slots: Vec<SavedSlot> }
        => ("frame.resume", [frame: Consume], Rule::FrameResume { frame: *frame, slots: slots.clone() });
    FrameCancel { frame: ValueId, cleanup: ValueId, protocol: ProtocolId }
        => ("frame.cancel", [frame: Consume, cleanup: Consume], Rule::FrameCancel { frame: *frame, cleanup: *cleanup, protocol: *protocol });
    Pin { value: ValueId, deps: ValueId, owner: ValueId, protocol: ProtocolId }
        => ("pin", [value: Borrow, deps: Observe, owner: Consume], Rule::Pin { owner: *owner, protocol: *protocol });
    PinTransfer { pin: ValueId, owner: ValueId, protocol: ProtocolId }
        => ("pin.transfer", [pin: Consume, owner: Consume], Rule::Protocol { state: *pin, owner: Some(*owner), packet: None, protocol: *protocol, transition: ProtocolTransition::Transfer });
    Unpin { pin: ValueId, owner: ValueId, protocol: ProtocolId }
        => ("unpin", [pin: Consume, owner: Consume], Rule::Protocol { state: *pin, owner: Some(*owner), packet: None, protocol: *protocol, transition: ProtocolTransition::Unpin });
    WorkerBind { action: ValueId, entry: ValueId, fields: Vec<ValueId>, ty: TypeId }
        => ("worker.bind", [action: Borrow, entry: Borrow, fields: Borrow], Rule::Data { ty: ResultType::Exact(*ty), trap: true });
    WorkerSpawn { entry: ValueId, args: Vec<ValueId>, direct: bool, result: Option<TypeId> }
        => ("worker.spawn", [entry: Borrow, args: Borrow], Rule::Worker(*result));
    ProcessSpawn { constructor: FuncId, constructor_args: Vec<ValueId>, action: FuncId, args: Vec<ValueId>, result: Option<TypeId> }
        => ("proc.spawn", [constructor_args: Borrow, args: Borrow], Rule::Worker(*result));
} }
