//! Structural rules of the async ABI assigned to slice V1 (D20, D21, D22, C-2, C-6).
//!
//! Only an async body of coro/low declares a D21 frame layout
//! (`coro-layout-misplaced`). The suspension, layout and body rules apply to the
//! async coro/low bodies that declare one; an async body without a layout
//! predates the async ABI, and its layout and continuation graph stay an
//! inventoried structural gap (`B95FrameLayoutAndContinuations`) until the coro
//! producer exists. Ownership of saved tokens, saved-set completeness and the
//! one-to-one site correspondence belong to the B9.5 linearity slice.
use std::collections::BTreeMap;
use super::{EntityLocation as L, RuleId as R, State};
use crate::*;

pub(super) fn check<'a>(module: &'a Module, function: FuncId, data: &'a Function, state: &mut State<'a>) {
    let replaces = data.attrs.iter().any(|attr| {
        matches!(attr, FunctionAttr::DeferEntry { replacements } if !replacements.is_empty())
    });
    if data.attrs.contains(&FunctionAttr::Async) && replaces {
        state.at(L::Function(function), data.loc);
        state.error(R::AsyncDeferReplace, "an async defer entry cannot declare a replacement label (C-6)");
    }
    let Some(body) = &data.body else {
        return;
    };
    let token_profile = matches!(module.profile, Profile::Coro | Profile::Low);
    let async_body = token_profile && data.attrs.contains(&FunctionAttr::Async);
    if !async_body && body.frame_layout.is_some() {
        state.at(L::Function(function), data.loc);
        state.error(R::CoroLayoutMisplaced, "only an async body of coro/low declares a frame layout (D21)");
    }
    for (block_id, block) in body.blocks.iter() {
        if !state.step(1) {
            return;
        }
        for &instruction in &block.insts {
            let Ok(inst) = body.insts.get(instruction) else {
                continue;
            };
            state.at(L::Instruction { function, instruction }, inst.loc);
            operation(module, &inst.op, token_profile, state);
        }
        if let TermKind::InvokeOwn { op, .. } = &block.term.kind {
            state.at(L::Terminator { function, block: block_id }, block.term.loc);
            operation(module, op, token_profile, state);
        }
    }
    if async_body && let Some(layout) = &body.frame_layout {
        Layout { module, function, data, body, layout, state, sites: BTreeMap::new(), started: false }.check();
    }
}

fn operation(module: &Module, op: &Operation, token_profile: bool, state: &mut State<'_>) {
    if token_profile && matches!(op, Operation::Async(AsyncOp::AwaitShielded { .. })) {
        state.error(R::CoroShieldedAwait, "await.shielded is not produced in coro/low (D20, review B10)");
    }
    let Operation::Async(AsyncOp::FutureNew { environment: FutureEnvironment::External { function, .. }, .. }) = op
    else {
        return;
    };
    let valid = module.functions.get(*function).is_ok_and(|target| {
        let external = target.attrs.iter().any(|attr| {
            matches!(attr, FunctionAttr::Extern(_) | FunctionAttr::Intrinsic { .. })
        });
        target.body.is_none() && external && target.attrs.contains(&FunctionAttr::Async)
    });
    if !valid {
        state.error(R::D22ExternalWithBody,
            "an external Future environment names an [async] extern or intrinsic without a body (D22)");
    }
}

/// The full verifier's D24.1 freshness of every `fut.spawn` (async ABI §7: a rule of the ownership verifier;
/// the SPB 17 loader checks it again as `SPB_SPAWN`).
pub(super) fn spawn_freshness<'a>(module: &'a Module, selected: Option<FuncId>, state: &mut State<'a>) {
    for (function, data) in module.functions.iter() {
        if selected.is_some_and(|id| id != function) {
            continue;
        }
        if let Some(body) = &data.body {
            spawn_fresh(module, function, body, state);
        }
    }
}

/// D24.1 (`d24-spawn-not-fresh`): `fut.spawn` starts a Future created earlier in the same block, by a call of an
/// async entry in High and by `fut.new` after it. Between the creation and the spawn the Future is only observed
/// (`deps.of`, `fut.done`, `fut.cancelled`, `fut.cancel`), so it is not awaited, does not escape and joins no
/// group first. Outside High the spawn forwards the creation's second result, the owner state (own) or the cache
/// duty (coro/low), so the duty it starts is the one that creation gave.
fn spawn_fresh(module: &Module, function: FuncId, body: &Body, state: &mut State<'_>) {
    for (_, block) in body.blocks.iter() {
        if !state.step(block.insts.len()) {
            return;
        }
        for (position, &instruction) in block.insts.iter().enumerate() {
            let Ok(inst) = body.insts.get(instruction) else {
                continue;
            };
            let Operation::Async(AsyncOp::FutureSpawn { future, state: forwarded }) = &inst.op else {
                continue;
            };
            state.at(L::Instruction { function, instruction }, inst.loc);
            let creation = block.insts[..position].iter().position(|earlier| {
                body.insts.get(*earlier).is_ok_and(|earlier| earlier.results.first() == Some(future))
            });
            let fresh = creation.is_some_and(|creation| {
                let Ok(created) = body.insts.get(block.insts[creation]) else {
                    return false;
                };
                let origin = match &created.op {
                    Operation::Async(AsyncOp::FutureNew { .. }) => {
                        forwarded.is_some() && created.results.get(1) == forwarded.as_ref()
                    }
                    Operation::Call(CallOp::Direct { function, .. }) => forwarded.is_none()
                        && module.functions.get(*function).is_ok_and(|callee| {
                            callee.attrs.contains(&FunctionAttr::Async)
                        }),
                    _ => false,
                };
                origin && block.insts[creation + 1..position].iter().all(|between| {
                    body.insts.get(*between).is_ok_and(|between| {
                        matches!(between.op, Operation::Ownership(OwnershipOp::Dependencies { .. })
                            | Operation::Async(AsyncOp::FutureDone { .. } | AsyncOp::FutureCancelled { .. }
                                | AsyncOp::FutureCancel { .. }))
                            || between.op.operand_refs().iter().all(|operand| {
                                operand.reference != OperandRef::Value(*future)
                            })
                    })
                })
            });
            if !fresh {
                state.error(R::D24SpawnNotFresh,
                    "fut.spawn starts a Future just created in its block and only observed since (D24.1)");
            }
        }
    }
}

/// The frame layout of an async body of coro/low in the D21 form, if `function` is one.
pub(super) fn frame_layout<'a>(module: &Module, function: &'a Function) -> Option<&'a FrameLayout> {
    let token_profile = matches!(module.profile, Profile::Coro | Profile::Low);
    if !token_profile || !function.attrs.contains(&FunctionAttr::Async) {
        return None;
    }
    function.body.as_ref()?.frame_layout.as_ref()
}

/// Whether `value` is an entry value of `function`: an entry parameter, or for a D21 async body a value
/// that its start resume restores from continuation 0 (C-2).
pub(super) fn entry_value(module: &Module, function: &Function, body: &Body, value: ValueId) -> bool {
    let Ok(data) = body.values.get(value) else {
        return false;
    };
    if frame_layout(module, function).is_none() {
        return matches!(data.def, Definition::BlockParam { block, .. } if block == body.entry);
    }
    let Definition::InstResult { inst, index } = data.def else {
        return false;
    };
    let start = body.blocks.get(body.entry).ok().and_then(|entry| entry.insts.first()) == Some(&inst);
    let resume = body.insts.get(inst).is_ok_and(|inst| {
        matches!(inst.op, Operation::Async(AsyncOp::FrameResume { continuation: 0, .. }))
    });
    start && resume && index > 0
}

/// The language signature of a function (C-2). An async body of coro/low in the D21 form has only its
/// frame as a parameter: its receiver, arguments, descriptors and input packet are the slots of
/// continuation 0, and its results are the values it publishes on success. A body that never publishes
/// a success keeps the results of its summary. The ownership ABI stays the body's own: a fixed root
/// (`async main`, an async test) keeps its observation ABI. `None` means that the body disagrees with
/// itself.
pub(super) fn language_signature(module: &Module, function: &Function) -> Option<Signature> {
    let Some(layout) = frame_layout(module, function) else {
        return Some(function.signature.clone());
    };
    let start = layout.continuations.first()?;
    let params = start.saved.iter()
        .map(|slot| layout.slots.get(*slot as usize).map(|decl| decl.ty))
        .collect::<Option<Vec<_>>>()?;
    let body = function.body.as_ref()?;
    let mut published: Option<Vec<TypeId>> = None;
    for (_, inst) in body.insts.iter() {
        let Operation::Async(AsyncOp::FuturePublish { outcome: PublishOutcome::Success { value, .. }, .. }) = &inst.op
        else {
            continue;
        };
        let types = value.iter().map(|value| body.values.get(*value).ok().map(|data| data.ty))
            .collect::<Option<Vec<_>>>()?;
        if published.as_ref().is_some_and(|previous| *previous != types) {
            return None;
        }
        published = Some(types);
    }
    let results = match published {
        Some(results) => results,
        None => function.summary.and_then(|id| module.summaries.get(id).ok())
            .map(|summary| summary.contract.results.clone())?,
    };
    Some(Signature { params, results, ownership_abi: function.signature.ownership_abi })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Suspends { No, May, Yes }

struct Layout<'s, 'a> {
    module: &'a Module,
    function: FuncId,
    data: &'a Function,
    body: &'a Body,
    layout: &'a FrameLayout,
    state: &'s mut State<'a>,
    /// The first site recorded for each continuation.
    sites: BTreeMap<u32, SuspendSite>,
    /// Whether the body starts with the resume of continuation 0.
    started: bool,
}

impl Layout<'_, '_> {
    fn check(&mut self) {
        self.declarations();
        let predecessors = self.predecessors();
        for (block_id, block) in self.body.blocks.iter() {
            if !self.state.step(1) {
                return;
            }
            for (position, &instruction) in block.insts.iter().enumerate() {
                let Ok(inst) = self.body.insts.get(instruction) else {
                    continue;
                };
                self.state.at(L::Instruction { function: self.function, instruction }, inst.loc);
                match &inst.op {
                    Operation::Async(AsyncOp::FrameAlloc { .. } | AsyncOp::FrameLoad { .. }
                        | AsyncOp::FrameStore { .. } | AsyncOp::FrameCancel { .. } | AsyncOp::FutureSettle { .. }) => {
                        self.state.error(R::CoroRawFrameAccess,
                            "raw frame access is not produced in an async coro/low body (D20)");
                    }
                    Operation::Async(AsyncOp::FrameSave { slots, continuation, .. }) => {
                        self.saved_slots(slots);
                        self.save(block_id, block, position, inst, *continuation, &predecessors);
                    }
                    Operation::Async(AsyncOp::FrameResume { frame, continuation, slots }) => {
                        self.resumed_slots(slots);
                        self.resume(block_id, block, position, *frame, *continuation);
                    }
                    op => {
                        let saved = position.checked_sub(1).and_then(|index| block.insts.get(index))
                            .and_then(|id| self.body.insts.get(*id).ok())
                            .is_some_and(|inst| matches!(inst.op, Operation::Async(AsyncOp::FrameSave { .. })));
                        if self.instruction_suspends(op) == Suspends::Yes && !saved {
                            self.state.error(R::CoroMissingResume,
                                "a suspending instruction needs frame.save before and frame.resume after it (D20)");
                        }
                    }
                }
            }
            self.state.at(L::Terminator { function: self.function, block: block_id }, block.term.loc);
            // C-2: only a failure that leaves the body must be published; a block unwind catches it inside.
            if block.unwind.is_none() && matches!(block.term.kind,
                TermKind::Throw(_) | TermKind::Raise(_) | TermKind::Panic(_) | TermKind::Trap { .. }) {
                self.state.error(R::CoroBodyRaises, "an async body publishes its failure instead of raising (C-2)");
            }
            let last_saves = block.insts.last().and_then(|id| self.body.insts.get(*id).ok())
                .is_some_and(|inst| matches!(inst.op, Operation::Async(AsyncOp::FrameSave { .. })));
            if self.terminator_suspends(&block.term.kind) == Suspends::Yes && !last_saves {
                self.state.error(R::CoroMissingResume,
                    "a suspending terminator needs frame.save as the last instruction of its block (D20)");
            }
        }
        self.continuations_used();
    }

    /// The body starts with the resume of continuation 0 (C-2), and every other declared continuation
    /// is the continuation of a suspension site (D21).
    fn continuations_used(&mut self) {
        self.at_function();
        if !self.started {
            self.state.error(R::CoroMissingResume,
                "an async body starts with frame.resume of continuation 0 on its frame parameter (C-2)");
        }
        let unused = (1..self.layout.continuations.len() as u32).any(|index| !self.sites.contains_key(&index));
        if unused {
            self.state.error(R::CoroContinuationMismatch, "a declared continuation has no suspension site (D21)");
        }
    }

    fn at_function(&mut self) {
        self.state.at(L::Function(self.function), self.data.loc);
    }

    /// Slot types and linearity, continuation numbering and slot indices (D21).
    fn declarations(&mut self) {
        self.at_function();
        for slot in &self.layout.slots {
            if !self.state.step(1) {
                return;
            }
            if self.module.types.get(slot.ty).is_ok_and(|ty| linear(ty) != slot.linear) {
                self.state.error(R::CoroLayoutTypeMismatch, "frame slot linearity disagrees with its type (D21)");
            }
        }
        let start = self.layout.continuations.first().is_some_and(|first| first.site == SuspendSite::Start);
        let other_start = self.layout.continuations.iter().skip(1).any(|item| item.site == SuspendSite::Start);
        if !start || other_start {
            self.state.error(R::CoroContinuationMismatch, "continuation 0, and only it, is the start (D21)");
        }
        let count = self.layout.slots.len();
        for continuation in &self.layout.continuations {
            if !self.state.step(1) {
                return;
            }
            let indices = continuation.saved.iter().chain(&continuation.operands);
            if indices.into_iter().any(|slot| *slot as usize >= count) {
                self.state.error(R::CoroLayoutTypeMismatch, "continuation names an undeclared frame slot (D21)");
            }
            // D20 §2.2: at PENDING the words of S_k ∪ O_k are stored in the frame, each in its own slot.
            let mut seen = std::collections::BTreeSet::new();
            if !continuation.saved.iter().chain(&continuation.operands).all(|slot| seen.insert(*slot)) {
                self.state.error(R::CoroSlotOverlap,
                    "the saved and operand slots of one continuation are pairwise distinct (D20 §2.2)");
            }
        }
    }

    fn predecessors(&mut self) -> BTreeMap<BlockId, usize> {
        let mut counts = BTreeMap::new();
        for (_, block) in self.body.blocks.iter() {
            for target in super::structure::targets(block) {
                if !self.state.step(1) || !self.state.memory(32) {
                    return counts;
                }
                *counts.entry(target).or_insert(0) += 1;
            }
        }
        counts
    }

    fn slot(&self, index: u32) -> Option<FrameSlotDecl> {
        self.layout.slots.get(index as usize).copied()
    }

    fn saved_slots(&mut self, slots: &[FrameSlot]) {
        for slot in slots {
            if !self.state.step(1) {
                return;
            }
            let ty = self.body.values.get(slot.value).ok().map(|value| value.ty);
            let valid = self.slot(slot.slot).is_some_and(|decl| {
                Some(decl.ty) == ty && decl.linear == (slot.mode == OperandMode::Consume)
            });
            if !valid {
                self.state.error(R::CoroLayoutTypeMismatch, "saved value disagrees with its frame slot type (D21)");
            }
        }
    }

    fn resumed_slots(&mut self, slots: &[SavedSlot]) {
        for slot in slots {
            if !self.state.step(1) {
                return;
            }
            let valid = self.slot(slot.slot).is_some_and(|decl| decl.ty == slot.ty && decl.linear == slot.linear);
            if !valid {
                self.state.error(R::CoroLayoutTypeMismatch, "resumed slot disagrees with the frame layout (D21)");
            }
        }
    }

    fn instruction_suspends(&self, op: &Operation) -> Suspends {
        match op {
            Operation::Async(AsyncOp::Await { .. } | AsyncOp::AwaitTake { .. } | AsyncOp::AwaitShielded { .. }
                | AsyncOp::AwaitUnwind { .. }) => Suspends::Yes,
            Operation::Ownership(OwnershipOp::ScopeEnd { effects, .. } | OwnershipOp::ScopeUnwind { effects, .. }) => {
                if effects.suspends { Suspends::Yes } else { Suspends::No }
            }
            Operation::Cleanup(CleanupOp::DestroyDirect { descriptor, .. }) => {
                let suspends = self.body.descriptor_bindings.iter().any(|binding| {
                    binding.dictionary == *descriptor && binding.contract.async_destroy
                });
                if suspends { Suspends::Yes } else { Suspends::May }
            }
            // The descriptor of a runtime-selected identity decides at run time.
            Operation::Cleanup(CleanupOp::Destroy { .. }) => Suspends::May,
            _ => Suspends::No,
        }
    }

    fn terminator_suspends(&self, term: &TermKind) -> Suspends {
        match term {
            TermKind::InvokeOwn { op, .. } => self.instruction_suspends(op),
            TermKind::InvokeDefer { function, .. } => {
                let async_defer = self.module.functions.get(*function).is_ok_and(|target| {
                    target.attrs.contains(&FunctionAttr::Async)
                        && target.attrs.iter().any(|attr| matches!(attr, FunctionAttr::DeferEntry { .. }))
                });
                if async_defer { Suspends::Yes } else { Suspends::No }
            }
            _ => Suspends::No,
        }
    }

    /// The saved continuation must be declared non-start, have one site and match the layout.
    fn record_site(&mut self, continuation: u32, site: SuspendSite) {
        let declared = self.layout.continuations.get(continuation as usize).map(|item| item.site);
        let previous = *self.sites.entry(continuation).or_insert(site);
        if continuation == 0 || declared != Some(site) || previous != site {
            self.state.error(R::CoroContinuationMismatch,
                "frame.save names a continuation whose declared site differs, or that has another site (D21)");
        }
    }

    /// Value operands of a suspending operation agree with the operand slot types (D21).
    fn operand_slots(&mut self, op: &Operation, continuation: u32) {
        let Some(declared) = self.layout.continuations.get(continuation as usize) else {
            return;
        };
        let values: Vec<_> = op.operand_refs().into_iter().filter_map(|operand| match operand.reference {
            OperandRef::Value(value) => Some(value),
            _ => None,
        }).collect();
        // Operand-list agreement itself is B9.5; only the slot types are checked here.
        if values.len() != declared.operands.len() {
            return;
        }
        for (value, slot) in values.into_iter().zip(&declared.operands) {
            let ty = self.body.values.get(value).ok().map(|value| value.ty);
            if self.slot(*slot).is_some_and(|decl| Some(decl.ty) != ty) {
                self.state.error(R::CoroLayoutTypeMismatch, "operand disagrees with its operand slot type (D21)");
            }
        }
    }

    fn resume_of(&self, block: BlockId, frame: Option<ValueId>) -> Option<u32> {
        let first = self.body.blocks.get(block).ok()?.insts.first()?;
        match &self.body.insts.get(*first).ok()?.op {
            Operation::Async(AsyncOp::FrameResume { frame: resumed, continuation, .. })
                if Some(*resumed) == frame => Some(*continuation),
            _ => None,
        }
    }

    fn save(
        &mut self, block_id: BlockId, block: &Block, position: usize, inst: &Instruction, continuation: u32,
        predecessors: &BTreeMap<BlockId, usize>,
    ) {
        let frame = inst.results.first().copied();
        let location = self.state.location.clone();
        if position + 1 == block.insts.len() {
            // Form (a): every successor starts with the matching resume and has no other predecessor.
            if self.terminator_suspends(&block.term.kind) == Suspends::No {
                self.unbracketed();
                return;
            }
            self.record_site(continuation, SuspendSite::Terminator(block_id));
            if let TermKind::InvokeOwn { op, .. } = &block.term.kind {
                self.operand_slots(op, continuation);
            }
            for target in super::structure::targets(block) {
                if !self.state.step(1) {
                    return;
                }
                match self.resume_of(target, frame) {
                    Some(resumed) if resumed != continuation => {
                        self.state.at(L::Block { function: self.function, block: target }, None);
                        self.state.error(R::CoroContinuationMismatch,
                            "frame.save and frame.resume name different continuations (D20)");
                    }
                    Some(_) if predecessors.get(&target) == Some(&1) => {}
                    _ => {
                        self.state.at(L::Terminator { function: self.function, block: block_id }, block.term.loc);
                        self.state.error(R::CoroMissingResume,
                            "every successor of a suspending terminator starts with its resume, alone (D20)");
                    }
                }
                self.state.location = location.clone();
            }
            return;
        }
        // Form (b): save, the suspending instruction and resume are consecutive.
        let Some(&suspending) = block.insts.get(position + 1) else {
            return;
        };
        let Ok(op) = self.body.insts.get(suspending).map(|inst| &inst.op) else {
            return;
        };
        if self.instruction_suspends(op) == Suspends::No {
            self.unbracketed();
            return;
        }
        self.record_site(continuation, SuspendSite::Instruction(suspending));
        self.operand_slots(op, continuation);
        let resume = block.insts.get(position + 2).and_then(|id| self.body.insts.get(*id).ok());
        match resume.map(|inst| &inst.op) {
            Some(Operation::Async(AsyncOp::FrameResume { frame: resumed, continuation: other, .. }))
                if Some(*resumed) == frame => {
                if *other != continuation {
                    self.state.at(L::Instruction { function: self.function, instruction: block.insts[position + 2] },
                        None);
                    self.state.error(R::CoroContinuationMismatch,
                        "frame.save and frame.resume name different continuations (D20)");
                }
            }
            _ => {
                self.state.at(L::Instruction { function: self.function, instruction: suspending }, None);
                self.state.error(R::CoroMissingResume,
                    "a suspending instruction needs frame.resume right after it (D20)");
            }
        }
        self.state.location = location;
    }

    /// `coro-unbracketed-suspend` (D20 §2.5): a `frame.save` brackets exactly one suspending operation; around any
    /// other operation it would move a token into the frame with no resume to bring it back.
    fn unbracketed(&mut self) {
        self.state.error(R::CoroUnbracketedSuspend, "frame.save brackets an operation that does not suspend (D20)");
    }

    fn resume(&mut self, block_id: BlockId, block: &Block, position: usize, frame: ValueId, continuation: u32) {
        if continuation == 0 {
            // C-2: the start resumes the sole frame parameter at the top of the entry block.
            let start = block_id == self.body.entry && position == 0 && block.params == [frame];
            self.started |= start;
            if !start {
                self.state.error(R::CoroContinuationMismatch,
                    "continuation 0 resumes the frame parameter at the start of the body (C-2)");
            }
            return;
        }
        let save = match self.body.values.get(frame).map(|value| value.def) {
            Ok(Definition::InstResult { inst, .. }) => self.body.insts.get(inst).ok()
                .filter(|data| matches!(data.op, Operation::Async(AsyncOp::FrameSave { .. })))
                .map(|_| inst),
            _ => None,
        };
        let bracketed = save.is_some_and(|save| {
            if position == 0 {
                self.body.blocks.iter().any(|(_, source)| {
                    source.insts.last() == Some(&save) && super::structure::targets(source).contains(&block_id)
                })
            } else {
                position >= 2 && block.insts[position - 2] == save
            }
        });
        if !bracketed {
            self.state.error(R::CoroMissingResume,
                "frame.resume must follow its frame.save around one suspending operation (D20)");
        }
    }
}

/// D21: `linear` equals the linearity of the slot type, as for saved slots (T1).
fn linear(ty: &Type) -> bool {
    matches!(ty, Type::Service(service) if service.is_linear())
        || matches!(ty, Type::Builder(_) | Type::MapBuilder(..) | Type::Frame(_))
}
