//! B9.5 rules of the suspension form in coro/low (async ABI D20, D21, C-1; slice V3).
//!
//! A token is in exactly one place: the running state or a frame slot. At every suspension the frame
//! holds every value that is live after the operation and is not its operand; schedule duties count as
//! used at every close site of their owner scope (review B4). A saved value is never read through its
//! pre-suspension name, a continuation is resumed at most once per save, and a sync function never
//! suspends. The rules read SSA and the D21 metadata only, so the compact and the precise solver share
//! them.
use super::{EntityLocation as L, RuleId as R, State};
use crate::*;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn check<'a>(module: &'a Module, selected: Option<FuncId>, state: &mut State<'a>) {
    if !matches!(module.profile, Profile::Coro | Profile::Low) {
        return;
    }
    for (function, data) in module.functions.iter() {
        if selected.is_some_and(|id| id != function) {
            continue;
        }
        let Some(body) = &data.body else {
            continue;
        };
        if !super::graph::eligible(function, body, state) {
            continue;
        }
        if !data.attrs.contains(&FunctionAttr::Async) {
            sync(module, function, body, state);
        } else if let Some(layout) = &body.frame_layout {
            Frames::new(module, function, body, layout, state).check();
        }
    }
}

/// D20: a suspending operation of coro (the low cleanup steps belong to the low verifier).
fn suspends(op: &Operation) -> bool {
    match op {
        Operation::Async(AsyncOp::Await { .. } | AsyncOp::AwaitTake { .. } | AsyncOp::AwaitShielded { .. }
            | AsyncOp::AwaitUnwind { .. }) => true,
        Operation::Ownership(OwnershipOp::ScopeEnd { effects, .. } | OwnershipOp::ScopeUnwind { effects, .. }) => {
            effects.suspends
        }
        _ => false,
    }
}

fn async_defer(module: &Module, function: FuncId) -> bool {
    module.functions.get(function).is_ok_and(|target| {
        target.attrs.contains(&FunctionAttr::Async)
            && target.attrs.iter().any(|attr| matches!(attr, FunctionAttr::DeferEntry { .. }))
    })
}

/// `coro-sync-suspends`: no suspending operation and no frame of its own in a sync function (D20).
fn sync(module: &Module, function: FuncId, body: &Body, state: &mut State<'_>) {
    for (block_id, block) in body.blocks.iter() {
        for &instruction in &block.insts {
            let Ok(inst) = body.insts.get(instruction) else {
                continue;
            };
            if !state.step(1) {
                return;
            }
            if suspends(&inst.op) {
                state.at(L::Instruction { function, instruction }, inst.loc);
                state.error(R::CoroSyncSuspends, "a sync function contains a suspending operation (D20)");
            }
        }
        let suspending = match &block.term.kind {
            TermKind::InvokeOwn { op, .. } => suspends(op),
            TermKind::InvokeDefer { function: target, .. } => async_defer(module, *target),
            _ => false,
        };
        if suspending {
            state.at(L::Terminator { function, block: block_id }, block.term.loc);
            state.error(R::CoroSyncSuspends, "a sync function contains a suspending operation (D20)");
        }
    }
    for (value, data) in body.values.iter() {
        if module.types.get(data.ty) == Ok(&Type::Frame(function)) {
            state.at(L::Value { function, value }, None);
            state.error(R::CoroSyncSuspends, "a sync function has no frame of its own (D20, C-1)");
        }
    }
}

/// A use or definition position in a block; the terminator follows every instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Site {
    Instruction(InstId),
    Terminator(BlockId),
}

/// One `frame.save` with the operation it brackets.
struct Save {
    block: BlockId,
    position: usize,
    instruction: InstId,
    continuation: u32,
    slots: Vec<FrameSlot>,
    site: Site,
}

struct Frames<'s, 'a> {
    module: &'a Module,
    function: FuncId,
    body: &'a Body,
    layout: &'a FrameLayout,
    state: &'s mut State<'a>,
    frames: BTreeSet<ValueId>,
    /// Schedule duties used implicitly at each close site of their owner (review B4).
    duties: BTreeMap<Site, Vec<ValueId>>,
}

impl<'s, 'a> Frames<'s, 'a> {
    fn new(
        module: &'a Module, function: FuncId, body: &'a Body, layout: &'a FrameLayout, state: &'s mut State<'a>,
    ) -> Self {
        let frames = body.values.iter().filter(|(_, data)| matches!(module.types.get(data.ty), Ok(Type::Frame(_))))
            .map(|(value, _)| value).collect();
        Self { module, function, body, layout, state, frames, duties: BTreeMap::new() }
    }

    fn linear(&self, value: ValueId) -> bool {
        self.body.values.get(value).ok().and_then(|data| self.module.types.get(data.ty).ok()).is_some_and(|ty| {
            matches!(ty, Type::Service(service) if service.is_linear())
                || matches!(ty, Type::Builder(_) | Type::MapBuilder(..) | Type::Frame(_))
        })
    }

    fn check(&mut self) {
        if !self.schedule_duties() {
            return;
        }
        let saves = self.saves();
        self.agreement(&saves);
        let Some(liveness) = self.liveness() else {
            return;
        };
        for save in &saves {
            self.two_places(save);
            self.complete(save, &liveness);
        }
        self.read_after_save(&saves);
        self.double_resume();
    }

    fn closed_scope(&self, op: &Operation) -> Option<ScopeId> {
        let Operation::Ownership(OwnershipOp::ScopeEnd { state, .. } | OwnershipOp::ScopeUnwind { state, .. }) = op
        else {
            return None;
        };
        match self.body.values.get(*state).ok().and_then(|data| self.module.types.get(data.ty).ok()) {
            Some(Type::Service(ServiceType::Scope(scope))) => Some(*scope),
            _ => None,
        }
    }

    /// A schedule duty is used at a close of its owner where it is available on every path: defined and
    /// not yet consumed by a save, by an operation, or by an earlier close (review B4).
    fn schedule_duties(&mut self) -> bool {
        let owners: BTreeMap<ValueId, ScopeId> = self.body.service_schedules.iter().flat_map(|schedule| {
            schedule.duties.iter().filter_map(|duty| duty.value()).map(|duty| (duty, schedule.owner))
        }).collect();
        if owners.is_empty() {
            return true;
        }
        let count = self.body.blocks.len();
        let mut input: Vec<Option<BTreeSet<ValueId>>> = vec![None; count];
        input[self.body.entry.as_u32() as usize] = Some(BTreeSet::new());
        for report in [false, true] {
            let mut changed = true;
            while changed {
                changed = false;
                for (block, data) in self.body.blocks.iter() {
                    if !self.state.step(1) {
                        return false;
                    }
                    let Some(mut available) = input[block.as_u32() as usize].clone() else {
                        continue;
                    };
                    available.extend(data.params.iter().filter(|value| owners.contains_key(value)));
                    for &inst in &data.insts {
                        let Ok(instruction) = self.body.insts.get(inst) else {
                            continue;
                        };
                        self.available_step(Site::Instruction(inst), &instruction.op, &owners, &mut available, report);
                        available.extend(instruction.results.iter().filter(|value| owners.contains_key(value)));
                    }
                    if let TermKind::InvokeOwn { op, .. } = &data.term.kind {
                        self.available_step(Site::Terminator(block), op, &owners, &mut available, report);
                    }
                    if report {
                        continue;
                    }
                    for target in super::structure::targets(data) {
                        let slot = &mut input[target.as_u32() as usize];
                        let merged = match slot {
                            None => available.clone(),
                            Some(previous) => previous.intersection(&available).copied().collect(),
                        };
                        if slot.as_ref() != Some(&merged) {
                            *slot = Some(merged);
                            changed = true;
                        }
                    }
                }
                if report {
                    break;
                }
            }
        }
        !self.state.incomplete
    }

    fn available_step(
        &mut self, site: Site, op: &Operation, owners: &BTreeMap<ValueId, ScopeId>, available: &mut BTreeSet<ValueId>,
        report: bool,
    ) {
        if let Some(scope) = self.closed_scope(op) {
            let settled: Vec<ValueId> = available.iter().copied().filter(|duty| owners.get(duty) == Some(&scope))
                .collect();
            for duty in &settled {
                available.remove(duty);
            }
            if report {
                self.duties.insert(site, settled);
            }
        }
        for operand in op.operand_refs() {
            if let (OperandRef::Value(value), OperandMode::Consume) = (operand.reference, operand.mode) {
                available.remove(&value);
            }
        }
    }

    fn saves(&mut self) -> Vec<Save> {
        let mut saves = Vec::new();
        for (block, data) in self.body.blocks.iter() {
            for (position, &instruction) in data.insts.iter().enumerate() {
                let Ok(inst) = self.body.insts.get(instruction) else {
                    continue;
                };
                let Operation::Async(AsyncOp::FrameSave { slots, continuation, .. }) = &inst.op else {
                    continue;
                };
                let site = match data.insts.get(position + 1) {
                    Some(next) => Site::Instruction(*next),
                    None => Site::Terminator(block),
                };
                saves.push(Save {
                    block, position, instruction, continuation: *continuation, slots: slots.clone(), site,
                });
            }
        }
        saves
    }

    fn site_op(&self, site: Site) -> Option<&'a Operation> {
        match site {
            Site::Instruction(inst) => self.body.insts.get(inst).ok().map(|inst| &inst.op),
            Site::Terminator(block) => match &self.body.blocks.get(block).ok()?.term.kind {
                TermKind::InvokeOwn { op, .. } => Some(&**op),
                _ => None,
            },
        }
    }

    fn operands(&self, site: Site) -> Vec<ValueId> {
        self.site_op(site).map(|op| op.operand_refs().into_iter().filter_map(|operand| match operand.reference {
            OperandRef::Value(value) => Some(value),
            _ => None,
        }).collect()).unwrap_or_default()
    }

    /// Values that the edges of a suspending terminator pass on.
    fn edge_values(&self, site: Site) -> Vec<ValueId> {
        let Site::Terminator(block) = site else {
            return Vec::new();
        };
        let Ok(data) = self.body.blocks.get(block) else {
            return Vec::new();
        };
        let TermKind::InvokeOwn { normal, unwind, .. } = &data.term.kind else {
            return Vec::new();
        };
        normal.args.iter().chain(&unwind.args).filter_map(|arg| match arg {
            EdgeArg::Value(value) => Some(*value),
            EdgeArg::Produced(_) => None,
        }).collect()
    }

    fn at_save(&mut self, save: &Save) {
        let loc = self.body.insts.get(save.instruction).ok().and_then(|inst| inst.loc);
        self.state.at(L::Instruction { function: self.function, instruction: save.instruction }, loc);
    }

    /// D21 §3.4: saves and resumes restore exactly the declared saved slots; operands match their slots.
    fn agreement(&mut self, saves: &[Save]) {
        for save in saves {
            let declared = self.layout.continuations.get(save.continuation as usize);
            let slots: Vec<u32> = save.slots.iter().map(|slot| slot.slot).collect();
            self.at_save(save);
            if declared.is_some_and(|declared| declared.saved != slots) {
                self.state.error(R::CoroLayoutTypeMismatch, "frame.save differs from the declared saved slots (D21)");
            }
            let mut operands = self.operands(save.site);
            operands.extend(self.duties.get(&save.site).into_iter().flatten().copied());
            let types: Option<Vec<TypeId>> = operands.iter()
                .map(|value| self.body.values.get(*value).ok().map(|data| data.ty)).collect();
            let declared_types: Option<Vec<TypeId>> = declared.and_then(|declared| declared.operands.iter()
                .map(|slot| self.layout.slots.get(*slot as usize).map(|decl| decl.ty)).collect());
            if declared.is_some() && types != declared_types {
                self.state.error(R::CoroLayoutTypeMismatch,
                    "operand slots differ from the operands and settled duties of the suspension (D21)");
            }
        }
        for (instruction, inst) in self.body.insts.iter() {
            let Operation::Async(AsyncOp::FrameResume { continuation, slots, .. }) = &inst.op else {
                continue;
            };
            let resumed: Vec<u32> = slots.iter().map(|slot| slot.slot).collect();
            let declared = self.layout.continuations.get(*continuation as usize);
            if declared.is_some_and(|declared| declared.saved != resumed) {
                self.state.at(L::Instruction { function: self.function, instruction }, inst.loc);
                self.state.error(R::CoroLayoutTypeMismatch, "frame.resume differs from the declared saved slots (D21)");
            }
        }
    }

    /// `coro-token-in-two-places`: a linear value is saved twice, or saved and consumed by the suspension.
    fn two_places(&mut self, save: &Save) {
        let mut seen = BTreeSet::new();
        let mut taken: BTreeSet<ValueId> = self.operands(save.site).into_iter().collect();
        taken.extend(self.duties.get(&save.site).into_iter().flatten().copied());
        taken.extend(self.edge_values(save.site));
        let mut twice = false;
        for slot in &save.slots {
            if !self.linear(slot.value) {
                continue;
            }
            twice |= !seen.insert(slot.value) || taken.contains(&slot.value);
        }
        if twice {
            self.at_save(save);
            self.state.error(R::CoroTokenInTwoPlaces,
                "a token is in the frame and in the running state at once (B9.5, D20)");
        }
    }

    fn liveness(&mut self) -> Option<Liveness> {
        Liveness::compute(self.module, self.body, &self.duties, &self.frames, self.state)
    }

    /// `coro-saved-set-incomplete`: a value live after the suspension is neither saved nor its operand.
    fn complete(&mut self, save: &Save, liveness: &Liveness) {
        let mut live = BTreeSet::new();
        match save.site {
            Site::Instruction(op) => {
                let resume = self.body.blocks.get(save.block).ok()
                    .and_then(|data| data.insts.get(save.position + 2).copied());
                if let Some(resume) = resume {
                    live = liveness.after(self.body, save.block, resume);
                    for result in self.body.insts.get(resume).map(|inst| inst.results.as_slice()).unwrap_or(&[]) {
                        live.remove(result);
                    }
                }
                for result in self.body.insts.get(op).map(|inst| inst.results.as_slice()).unwrap_or(&[]) {
                    live.remove(result);
                }
            }
            Site::Terminator(block) => {
                let Ok(data) = self.body.blocks.get(block) else {
                    return;
                };
                for target in super::structure::targets(data) {
                    live.extend(liveness.live_in.get(&target).into_iter().flatten().copied());
                }
                live.extend(self.edge_values(save.site));
            }
        }
        for value in self.operands(save.site).iter().chain(self.duties.get(&save.site).into_iter().flatten()) {
            live.remove(value);
        }
        for slot in &save.slots {
            live.remove(&slot.value);
        }
        live.retain(|value| !self.frames.contains(value));
        if !live.is_empty() {
            self.at_save(save);
            self.state.error(R::CoroSavedSetIncomplete,
                "a value live after the suspension is neither saved nor its operand (D20, review B4)");
        }
    }

    /// `coro-read-after-save`: after `frame.save`, a saved value is read only through its resumed version.
    fn read_after_save(&mut self, saves: &[Save]) {
        let count = self.body.blocks.len();
        let by_instruction: BTreeMap<InstId, &Save> = saves.iter().map(|save| (save.instruction, save)).collect();
        let mut input: Vec<Option<BTreeSet<ValueId>>> = vec![None; count];
        input[self.body.entry.as_u32() as usize] = Some(BTreeSet::new());
        let mut changed = true;
        while changed {
            changed = false;
            for (block, data) in self.body.blocks.iter() {
                if !self.state.step(1) {
                    return;
                }
                let Some(mut saved) = input[block.as_u32() as usize].clone() else {
                    continue;
                };
                self.transfer(block, data, &mut saved, &by_instruction, false);
                for target in super::structure::targets(data) {
                    let slot = &mut input[target.as_u32() as usize];
                    let merged: BTreeSet<ValueId> = slot.iter().flatten().chain(&saved).copied().collect();
                    if slot.as_ref() != Some(&merged) {
                        *slot = Some(merged);
                        changed = true;
                    }
                }
            }
        }
        for (block, data) in self.body.blocks.iter() {
            if let Some(mut saved) = input[block.as_u32() as usize].clone() {
                self.transfer(block, data, &mut saved, &by_instruction, true);
            }
        }
    }

    fn transfer(
        &mut self, block: BlockId, data: &'a Block, saved: &mut BTreeSet<ValueId>,
        by_instruction: &BTreeMap<InstId, &Save>, report: bool,
    ) {
        for param in &data.params {
            saved.remove(param);
        }
        let mut suspended: Option<&Save> = None;
        for &instruction in &data.insts {
            let Ok(inst) = self.body.insts.get(instruction) else {
                continue;
            };
            let exempt = suspended.filter(|save| save.site == Site::Instruction(instruction));
            let uses = cfg::operation_uses(&inst.op, self.body).map(|uses| {
                uses.into_iter().map(|operand| operand.value).collect::<Vec<_>>()
            }).unwrap_or_default();
            if report && uses.iter().any(|value| saved.contains(value) && !(exempt.is_some() && self.linear(*value))) {
                self.state.at(L::Instruction { function: self.function, instruction }, inst.loc);
                self.state.error(R::CoroReadAfterSave, "a saved value is read through its pre-suspension name (B9.5)");
            }
            if let Some(save) = by_instruction.get(&instruction) {
                saved.extend(save.slots.iter().map(|slot| slot.value));
                suspended = Some(save);
            }
            for result in &inst.results {
                saved.remove(result);
            }
        }
        let exempt = suspended.filter(|save| save.site == Site::Terminator(block));
        let mut uses = Vec::new();
        let _ = cfg::walk_terminator(block, &data.term, self.body, ContractRegistry::new(self.module), &mut |event| {
            if let cfg::CfgEvent::Use { operand, .. } = event {
                uses.push(operand.value);
            }
        });
        if let Some(unwind) = &data.unwind {
            uses.extend(unwind.target.args.iter().copied());
        }
        if report && uses.iter().any(|value| saved.contains(value) && !(exempt.is_some() && self.linear(*value))) {
            self.state.at(L::Terminator { function: self.function, block }, data.term.loc);
            self.state.error(R::CoroReadAfterSave, "a saved value is read through its pre-suspension name (B9.5)");
        }
    }

    /// `coro-double-resume`: each save of a continuation is resumed at most once on a path.
    fn double_resume(&mut self) {
        let count = self.body.blocks.len();
        let mut input: Vec<Option<BTreeSet<u32>>> = vec![None; count];
        input[self.body.entry.as_u32() as usize] = Some(BTreeSet::new());
        for report in [false, true] {
            let mut changed = true;
            while changed {
                changed = false;
                for (block, data) in self.body.blocks.iter() {
                    if !self.state.step(1) {
                        return;
                    }
                    let Some(mut resumed) = input[block.as_u32() as usize].clone() else {
                        continue;
                    };
                    for &instruction in &data.insts {
                        let Ok(inst) = self.body.insts.get(instruction) else {
                            continue;
                        };
                        match &inst.op {
                            Operation::Async(AsyncOp::FrameSave { continuation, .. }) => {
                                resumed.remove(continuation);
                            }
                            Operation::Async(AsyncOp::FrameResume { continuation, .. })
                                if !resumed.insert(*continuation) && report => {
                                self.state.at(L::Instruction { function: self.function, instruction }, inst.loc);
                                self.state.error(R::CoroDoubleResume,
                                    "a continuation is resumed twice after one save (B9.5, D20)");
                            }
                            _ => {}
                        }
                    }
                    if report {
                        continue;
                    }
                    for target in super::structure::targets(data) {
                        let slot = &mut input[target.as_u32() as usize];
                        let merged: BTreeSet<u32> = slot.iter().flatten().chain(&resumed).copied().collect();
                        if slot.as_ref() != Some(&merged) {
                            *slot = Some(merged);
                            changed = true;
                        }
                    }
                }
                if report {
                    break;
                }
            }
        }
    }
}

/// Backward liveness of SSA values with the implicit uses of schedule duties.
struct Liveness {
    live_in: BTreeMap<BlockId, BTreeSet<ValueId>>,
    live_out: BTreeMap<BlockId, BTreeSet<ValueId>>,
    instruction_uses: BTreeMap<InstId, Vec<ValueId>>,
    terminator_uses: BTreeMap<BlockId, Vec<ValueId>>,
}

impl Liveness {
    fn compute(
        module: &Module, body: &Body, implicit: &BTreeMap<Site, Vec<ValueId>>, frames: &BTreeSet<ValueId>,
        state: &mut State<'_>,
    ) -> Option<Self> {
        let mut instruction_uses = BTreeMap::new();
        let mut terminator_uses = BTreeMap::new();
        let mut generated = BTreeMap::new();
        let mut killed = BTreeMap::new();
        for (block, data) in body.blocks.iter() {
            if !state.step(data.insts.len() + 1) || !state.memory(64 * (data.insts.len() + 1)) {
                return None;
            }
            let mut uses = Vec::new();
            cfg::walk_terminator(block, &data.term, body, ContractRegistry::new(module), &mut |event| {
                if let cfg::CfgEvent::Use { operand, .. } = event {
                    uses.push(operand.value);
                }
            }).ok()?;
            if let Some(unwind) = &data.unwind {
                uses.extend(unwind.target.args.iter().copied());
            }
            uses.extend(implicit.get(&Site::Terminator(block)).into_iter().flatten().copied());
            uses.retain(|value| !frames.contains(value));
            let mut live: BTreeSet<ValueId> = uses.iter().copied().collect();
            let mut kill = BTreeSet::new();
            terminator_uses.insert(block, uses);
            for &inst in data.insts.iter().rev() {
                let instruction = body.insts.get(inst).ok()?;
                for result in &instruction.results {
                    live.remove(result);
                    kill.insert(*result);
                }
                let mut uses: Vec<ValueId> = cfg::operation_uses(&instruction.op, body).ok()?
                    .into_iter().map(|operand| operand.value).collect();
                uses.extend(implicit.get(&Site::Instruction(inst)).into_iter().flatten().copied());
                uses.retain(|value| !frames.contains(value));
                live.extend(uses.iter().copied());
                instruction_uses.insert(inst, uses);
            }
            for param in &data.params {
                live.remove(param);
                kill.insert(*param);
            }
            generated.insert(block, live);
            killed.insert(block, kill);
        }
        let mut live_in = generated.clone();
        let mut live_out: BTreeMap<BlockId, BTreeSet<ValueId>> = BTreeMap::new();
        let mut changed = true;
        while changed {
            changed = false;
            for (block, data) in body.blocks.iter() {
                if !state.step(1) {
                    return None;
                }
                let mut out = BTreeSet::new();
                for target in super::structure::targets(data) {
                    out.extend(live_in.get(&target).into_iter().flatten().copied());
                }
                let mut input = generated.get(&block).cloned().unwrap_or_default();
                let kill = killed.get(&block).cloned().unwrap_or_default();
                input.extend(out.difference(&kill).copied());
                if live_in.get(&block) != Some(&input) || live_out.get(&block) != Some(&out) {
                    live_in.insert(block, input);
                    live_out.insert(block, out);
                    changed = true;
                }
            }
        }
        Some(Self { live_in, live_out, instruction_uses, terminator_uses })
    }

    /// Values live right after `inst` of `block`.
    fn after(&self, body: &Body, block: BlockId, inst: InstId) -> BTreeSet<ValueId> {
        let mut live = self.live_out.get(&block).cloned().unwrap_or_default();
        live.extend(self.terminator_uses.get(&block).into_iter().flatten().copied());
        let Ok(data) = body.blocks.get(block) else {
            return live;
        };
        for &later in data.insts.iter().rev().take_while(|item| **item != inst) {
            for result in body.insts.get(later).map(|inst| inst.results.as_slice()).unwrap_or(&[]) {
                live.remove(result);
            }
            live.extend(self.instruction_uses.get(&later).into_iter().flatten().copied());
        }
        live
    }
}
