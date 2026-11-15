//! The rewrite of one function body.
use super::graph;
use super::layout::{self, Slots};
use super::liveness::{Liveness, Site};
use super::repair::{self, Variable};
use super::{Budget, CoroError};
use crate::*;
use std::collections::{BTreeMap, BTreeSet};

/// The owner's cache duty of one Future (C-1).
struct Duty {
    token: ValueId,
    scope: ScopeId,
}

/// One suspension site and its continuation.
struct Suspension {
    site: Site,
    block: BlockId,
    saved: Vec<ValueId>,
    operands: Vec<ValueId>,
    /// Schedule duties that the suspending close settles; they are operand slots of its continuation.
    duties: Vec<ValueId>,
}

pub(super) fn lower(module: &mut Module, function: FuncId, budget: &mut Budget<'_>) -> Result<(), CoroError> {
    let data = module.functions.get(function)?;
    let is_async = data.attrs.contains(&FunctionAttr::Async);
    let Some(body) = module.functions.get_mut(function)?.body.take() else {
        return Ok(());
    };
    let mut lowering = Lowering { module, function, body, budget };
    let result = lowering.run(is_async);
    let Lowering { module, body, .. } = lowering;
    module.functions.get_mut(function)?.body = Some(body);
    result?;
    if is_async {
        // C-2: the coro body takes only its frame; the language signature stays on its summaries.
        let frame = module.types.intern(Type::Frame(function))?;
        module.functions.get_mut(function)?.signature = Signature {
            params: vec![frame], results: vec![], ownership_abi: OwnershipAbi::Unelaborated,
        };
    }
    Ok(())
}

struct Lowering<'a, 'b> {
    module: &'a mut Module,
    function: FuncId,
    body: Body,
    budget: &'a mut Budget<'b>,
}

impl Lowering<'_, '_> {
    fn invalid(&self, message: &str) -> CoroError {
        CoroError::InvalidInput { function: Some(self.function), message: message.into() }
    }
    fn unsupported(&self, message: &str) -> CoroError {
        CoroError::Unsupported { function: self.function, message: message.into() }
    }

    fn run(&mut self, is_async: bool) -> Result<(), CoroError> {
        self.check(is_async)?;
        let duties = self.futures()?;
        let mut slots = Slots::default();
        let mut continuations = Vec::new();
        let mut frame = None;
        if is_async {
            self.awaits_unwind()?;
            self.landings()?;
            let (start, continuation) = self.entry(&mut slots)?;
            continuations.push(continuation);
            self.exits(start)?;
            frame = Some(start);
        }
        let closes = self.closes(&duties)?;
        if let Some(start) = frame {
            let suspensions = self.suspensions(&closes)?;
            let mut versions: BTreeMap<ValueId, Vec<ValueId>> = BTreeMap::new();
            let mut frames = Vec::new();
            for (index, suspension) in suspensions.iter().enumerate() {
                let continuation = u32::try_from(index + 1).map_err(|_| CoroError::LimitExceeded("continuations"))?;
                let entry = self.bracket(start, continuation, suspension, &mut slots, &mut versions, &mut frames)?;
                continuations.push(entry);
            }
            repair::repair(self.module, self.function, &mut self.body,
                &Variable { original: start, versions: frames, implicit: Vec::new() }, self.budget)?;
            let mut repaired = BTreeMap::new();
            for (value, versions) in versions {
                let implicit = self.implicit_sites(&closes, value);
                let found = repair::repair(self.module, self.function, &mut self.body,
                    &Variable { original: value, versions, implicit }, self.budget)?;
                repaired.insert(value, found);
            }
            self.schedules(&duties, &closes, &repaired)?;
            self.body.frame_layout = Some(FrameLayout { slots: slots.decls, continuations });
        } else {
            self.schedules(&duties, &closes, &BTreeMap::new())?;
        }
        Ok(())
    }

    fn check(&self, is_async: bool) -> Result<(), CoroError> {
        if self.body.frame_layout.is_some() {
            return Err(self.invalid("an Own body declares no frame layout"));
        }
        if !self.body.service_schedules.is_empty() {
            return Err(self.unsupported("service schedules of an Own body"));
        }
        let declaration = self.module.functions.get(self.function)?;
        if is_async && declaration.attrs.iter().any(|attr| matches!(attr, FunctionAttr::DeferEntry { .. })) {
            return Err(self.unsupported("async defer entries (C-6) are lowered by slice D1"));
        }
        for (_, block) in self.body.blocks.iter() {
            // A block unwind leaves from any point of its block, so no frame version may change there.
            if is_async && block.unwind.is_some() && self.has_suspension(block)? {
                return Err(self.unsupported("a suspension inside a block with an unwind edge"));
            }
            if let TermKind::InvokeOwn { op, .. } = &block.term.kind
                && matches!(**op, Operation::Async(AsyncOp::FutureNew { .. })) {
                return Err(self.invalid("fut.new is an instruction"));
            }
            for &inst in &block.insts {
                let op = &self.body.insts.get(inst)?.op;
                if matches!(op, Operation::Async(AsyncOp::AwaitShielded { .. })) {
                    return Err(self.invalid("await.shielded is not produced (D20, review B10)"));
                }
                if matches!(op, Operation::Async(AsyncOp::FrameAlloc { .. } | AsyncOp::FrameLoad { .. }
                    | AsyncOp::FrameStore { .. } | AsyncOp::FrameSave { .. } | AsyncOp::FrameResume { .. }
                    | AsyncOp::FrameCancel { .. } | AsyncOp::FuturePublish { .. } | AsyncOp::FutureSettle { .. })) {
                    return Err(self.invalid("frame and publication operations belong to coro and low"));
                }
            }
        }
        Ok(())
    }

    fn has_suspension(&self, block: &Block) -> Result<bool, CoroError> {
        for &inst in &block.insts {
            if self.suspends(&self.body.insts.get(inst)?.op)? {
                return Ok(true);
            }
        }
        self.terminator_suspends(&block.term.kind)
    }

    fn intern(&mut self, ty: Type) -> Result<TypeId, CoroError> {
        Ok(self.module.types.intern(ty)?)
    }

    fn value(&mut self, ty: TypeId, def: Definition, facts: ValueFacts) -> Result<ValueId, CoroError> {
        Ok(self.body.values.alloc(ValueData { ty, def, facts })?)
    }

    /// Facts that the text codec gives a value of this type outside High.
    fn default_facts(&self, ty: TypeId) -> ValueFacts {
        match self.module.types.get(ty) {
            Ok(Type::Service(_)) => ValueFacts::Service,
            Ok(Type::Int | Type::Bool | Type::Byte | Type::Float | Type::Rat | Type::Char | Type::Str
                | Type::Range | Type::Descriptor) => ValueFacts::Plain { certificate: PlainCertificate::Scalar },
            _ => ValueFacts::Unelaborated,
        }
    }

    fn ty(&self, value: ValueId) -> Result<TypeId, CoroError> {
        Ok(self.body.values.get(value)?.ty)
    }

    /// C-1: `fut.new` keeps its environment and returns the owner's cache duty after the Future.
    fn futures(&mut self) -> Result<Vec<Duty>, CoroError> {
        let mut duties = Vec::new();
        let blocks: Vec<_> = self.body.blocks.iter().map(|(_, block)| block.insts.clone()).collect();
        for inst in blocks.into_iter().flatten() {
            self.budget.step(1)?;
            let Operation::Async(AsyncOp::FutureNew { owner, .. }) = self.body.insts.get(inst)?.op else {
                continue;
            };
            let ty = self.intern(Type::Service(ServiceType::CacheState))?;
            let token = self.value(ty, Definition::InstResult { inst, index: 1 }, ValueFacts::Service)?;
            let results = &mut self.body.insts.get_mut(inst)?.results;
            results.insert(1, token);
            let results = results.clone();
            for (index, value) in results.iter().enumerate().skip(2) {
                let index = u16::try_from(index).map_err(|_| CoroError::LimitExceeded("results"))?;
                self.body.values.get_mut(*value)?.def = Definition::InstResult { inst, index };
            }
            duties.push(Duty { token, scope: owner });
        }
        Ok(duties)
    }

    /// C-3: every await unwind edge flows into `await.unwind`, which consumes its task and receipt.
    fn awaits_unwind(&self) -> Result<(), CoroError> {
        for (_, block) in self.body.blocks.iter() {
            let TermKind::InvokeOwn { op, unwind, .. } = &block.term.kind else {
                continue;
            };
            if !matches!(**op, Operation::Async(AsyncOp::Await { .. } | AsyncOp::AwaitTake { .. })) {
                continue;
            }
            let target = self.body.blocks.get(unwind.block)?;
            let param = |slot: u16| unwind.args.iter().position(|arg| {
                *arg == EdgeArg::Produced(EdgeResult { outcome: Outcome::Unwind, slot })
            }).and_then(|index| target.params.get(index).copied());
            let (Some(task), Some(receipt)) = (param(1), param(2)) else {
                return Err(self.invalid("an await unwind edge passes its task and receipt (C-3)"));
            };
            let consumed = target.insts.iter().any(|inst| self.body.insts.get(*inst).is_ok_and(|inst| {
                matches!(inst.op, Operation::Async(AsyncOp::AwaitUnwind { task: t, receipt: r, .. })
                    if t == task && r == receipt)
            }));
            if !consumed {
                return Err(self.invalid("every await unwind flows into await.unwind (C-3)"));
            }
        }
        Ok(())
    }

    /// D20: the suspending operations of coro.
    fn suspends(&self, op: &Operation) -> Result<bool, CoroError> {
        match op {
            Operation::Async(AsyncOp::Await { .. } | AsyncOp::AwaitTake { .. } | AsyncOp::AwaitUnwind { .. }) => {
                Ok(true)
            }
            Operation::Ownership(OwnershipOp::ScopeEnd { effects, .. } | OwnershipOp::ScopeUnwind { effects, .. }) => {
                Ok(effects.suspends)
            }
            op => {
                let suspends = ContractRegistry::new(self.module).lookup(op).is_ok_and(|c| c.effects.suspends);
                if suspends {
                    return Err(self.unsupported("a suspending operation outside the D20 list"));
                }
                Ok(false)
            }
        }
    }

    fn terminator_suspends(&self, term: &TermKind) -> Result<bool, CoroError> {
        match term {
            TermKind::InvokeOwn { op, .. } => self.suspends(op),
            TermKind::InvokeDefer { function, .. } => {
                let target = self.module.functions.get(*function)?;
                if target.attrs.contains(&FunctionAttr::Async) {
                    return Err(self.unsupported("async defer invocations (C-6) are lowered by slice D1"));
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    /// D20 form (a): a successor of a suspending terminator starts with the resume and has no other
    /// predecessor. An edge that enters a shared block, or passes values that the suspension saves,
    /// enters a new block that resumes and then branches to the original successor.
    fn landings(&mut self) -> Result<(), CoroError> {
        let predecessors = graph::predecessors(&self.body);
        let blocks: Vec<BlockId> = self.body.blocks.iter().map(|(block, _)| block).collect();
        for block in blocks {
            self.budget.step(1)?;
            let mut term = self.body.blocks.get(block)?.term.kind.clone();
            if !self.terminator_suspends(&term)? {
                continue;
            }
            for edge in graph::invoke_edges(&mut term) {
                let target = edge.block;
                let shared = predecessors.get(target.as_u32() as usize).is_some_and(|list| list.len() > 1);
                let values = edge.args.iter().any(|arg| matches!(arg, EdgeArg::Value(_)));
                // The resume starts its block; a block unwind would leave across it.
                let unwinds = self.body.blocks.get(target)?.unwind.is_some();
                if !shared && !values && !unwinds {
                    continue;
                }
                let original = self.body.blocks.get(target)?;
                let lexical_scope = original.lexical_scope;
                let target_params = original.params.clone();
                let landing = self.body.blocks.alloc(Block {
                    lexical_scope, params: Vec::new(), unwind: None, insts: Vec::new(),
                    term: Terminator { kind: TermKind::Unreachable, loc: None },
                })?;
                let mut params = Vec::new();
                let mut produced = Vec::new();
                let mut forwarded = Vec::new();
                for (index, arg) in edge.args.iter().enumerate() {
                    match arg {
                        EdgeArg::Produced(result) => {
                            let param = *target_params.get(index)
                                .ok_or_else(|| self.invalid("an edge passes more values than its target takes"))?;
                            let template = self.body.values.get(param)?.clone();
                            let def = Definition::BlockParam {
                                block: landing,
                                index: u32::try_from(params.len()).map_err(|_| CoroError::LimitExceeded("params"))?,
                            };
                            let value = self.value(template.ty, def, template.facts)?;
                            params.push(value);
                            produced.push(EdgeArg::Produced(*result));
                            forwarded.push(value);
                        }
                        EdgeArg::Value(value) => forwarded.push(*value),
                    }
                }
                let data = self.body.blocks.get_mut(landing)?;
                data.params = params;
                data.term.kind = TermKind::Br(Target { block: target, args: forwarded });
                edge.block = landing;
                edge.args = produced;
            }
            self.body.blocks.get_mut(block)?.term.kind = term;
        }
        Ok(())
    }

    /// C-2: the body takes its frame and restores the receiver, the arguments, the descriptors and the
    /// input packet from continuation 0. Returns the restored frame token.
    fn entry(&mut self, slots: &mut Slots) -> Result<(ValueId, Continuation), CoroError> {
        let entry = self.body.entry;
        if graph::predecessors(&self.body).get(entry.as_u32() as usize).is_some_and(|list| !list.is_empty()) {
            return Err(self.unsupported("an async body whose entry block has predecessors"));
        }
        let params = self.body.blocks.get(entry)?.params.clone();
        let mut language = Vec::new();
        let mut descriptors = Vec::new();
        let mut rest = Vec::new();
        for value in params {
            match self.module.types.get(self.ty(value)?)? {
                Type::Descriptor => descriptors.push(value),
                Type::Service(_) | Type::Frame(_) => rest.push(value),
                _ => language.push(value),
            }
        }
        let restored: Vec<ValueId> = language.into_iter().chain(descriptors).chain(rest).collect();
        let frame_ty = self.intern(Type::Frame(self.function))?;
        let facts = self.default_facts(frame_ty);
        let parameter = self.value(frame_ty, Definition::BlockParam { block: entry, index: 0 }, facts.clone())?;
        let resume = self.body.insts.alloc(Instruction {
            results: Vec::new(),
            op: Operation::Async(AsyncOp::FrameResume { frame: parameter, continuation: 0, slots: Vec::new() }),
            facts: InstructionFacts::default(), loc: None,
        })?;
        let start = self.value(frame_ty, Definition::InstResult { inst: resume, index: 0 }, facts)?;
        let typed: Vec<(ValueId, TypeId)> = restored.iter().map(|value| Ok((*value, self.ty(*value)?)))
            .collect::<Result<_, CoroError>>()?;
        let assigned = slots.assign(&self.module.types, &typed);
        let mut saved = Vec::new();
        for (index, (&(value, ty), &slot)) in typed.iter().zip(&assigned).enumerate() {
            let index = u16::try_from(index + 1).map_err(|_| CoroError::LimitExceeded("results"))?;
            self.body.values.get_mut(value)?.def = Definition::InstResult { inst: resume, index };
            saved.push(SavedSlot { slot, ty, linear: layout::linear(&self.module.types, ty) });
        }
        let instruction = self.body.insts.get_mut(resume)?;
        instruction.results = std::iter::once(start).chain(restored).collect();
        instruction.op = Operation::Async(AsyncOp::FrameResume { frame: parameter, continuation: 0, slots: saved });
        let block = self.body.blocks.get_mut(entry)?;
        block.params = vec![parameter];
        block.insts.insert(0, resume);
        Ok((start, Continuation { site: SuspendSite::Start, saved: assigned, operands: Vec::new() }))
    }

    /// C-2: every exit publishes the outcome through the frame and returns. A success exit publishes
    /// the outcome that its `exit.commit` moved to the Future's cache; a failure exit publishes it.
    fn exits(&mut self, frame: ValueId) -> Result<(), CoroError> {
        let blocks: Vec<BlockId> = self.body.blocks.iter().map(|(block, _)| block).collect();
        for block in blocks {
            self.budget.step(1)?;
            let data = self.body.blocks.get(block)?;
            let outcome = match &data.term.kind {
                TermKind::Ret { values } => self.success(block, values)?,
                TermKind::Raise(failure) if data.unwind.is_none() => PublishOutcome::Failure(*failure),
                TermKind::Throw(_) | TermKind::Panic(_) if data.unwind.is_some() => continue,
                TermKind::Raise(_) => continue,
                TermKind::Throw(_) | TermKind::Panic(_) | TermKind::RetDefer { .. } => {
                    return Err(self.unsupported("an async body leaves through raise after its lexical unwind"));
                }
                _ => continue,
            };
            let publish = self.body.insts.alloc(Instruction {
                results: Vec::new(), op: Operation::Async(AsyncOp::FuturePublish { frame, outcome }),
                facts: InstructionFacts::default(), loc: None,
            })?;
            let data = self.body.blocks.get_mut(block)?;
            data.insts.push(publish);
            data.term.kind = TermKind::Ret { values: Vec::new() };
        }
        Ok(())
    }

    fn success(&self, block: BlockId, values: &[ValueId]) -> Result<PublishOutcome, CoroError> {
        let mut packet = None;
        let mut value = None;
        for &returned in values {
            match self.module.types.get(self.ty(returned)?)? {
                Type::Service(ServiceType::Own) if packet.is_none() => packet = Some(returned),
                Type::Service(_) => return Err(self.unsupported("an async body returns service values")),
                _ if value.is_none() => value = Some(returned),
                _ => return Err(self.unsupported("an async body returns several language values")),
            }
        }
        let commit = packet.and_then(|packet| match self.body.values.get(packet).ok()?.def {
            Definition::InstResult { inst, .. } => Some(inst),
            Definition::BlockParam { .. } => None,
        }).filter(|inst| self.body.blocks.get(block).is_ok_and(|data| data.insts.contains(inst)));
        let deps = commit.and_then(|inst| self.body.insts.get(inst).ok())
            .filter(|inst| matches!(inst.op, Operation::Exit(ExitOp::Commit { .. })))
            .and_then(|inst| inst.results.last().copied());
        match (packet, deps) {
            (Some(packet), Some(deps)) => Ok(PublishOutcome::Success { value, packet, deps }),
            _ => Err(self.invalid("an async success exit returns the outcome of its exit.commit (C-2)")),
        }
    }

    /// Close sites of every scope that owns a duty, with the duties that reach them on every path.
    fn closes(&mut self, duties: &[Duty]) -> Result<BTreeMap<Site, Vec<ValueId>>, CoroError> {
        let mut sites: Vec<(Site, BlockId, usize, ScopeId)> = Vec::new();
        for (block, data) in self.body.blocks.iter() {
            for (position, &inst) in data.insts.iter().enumerate() {
                if let Some(scope) = self.closed_scope(&self.body.insts.get(inst)?.op)? {
                    sites.push((Site::Instruction(inst), block, position, scope));
                }
            }
            if let TermKind::InvokeOwn { op, .. } = &data.term.kind
                && let Some(scope) = self.closed_scope(op)? {
                sites.push((Site::Terminator(block), block, data.insts.len(), scope));
            }
        }
        let predecessors = graph::predecessors(&self.body);
        let mut closes: BTreeMap<Site, Vec<ValueId>> = BTreeMap::new();
        for duty in duties {
            let (must, may) = self.defined(duty.token, &predecessors)?;
            let Definition::InstResult { inst, .. } = self.body.values.get(duty.token)?.def else {
                return Err(self.invalid("a cache duty is an instruction result"));
            };
            let (def_block, def_at) = self.body.blocks.iter().find_map(|(block, data)| {
                data.insts.iter().position(|item| *item == inst).map(|at| (block, at))
            }).ok_or_else(|| self.invalid("a cache duty has no defining instruction"))?;
            for &(site, block, position, scope) in &sites {
                if scope != duty.scope {
                    continue;
                }
                let index = block.as_u32() as usize;
                let earlier = block == def_block && def_at < position;
                if must[index] || earlier {
                    closes.entry(site).or_default().push(duty.token);
                } else if may[index] {
                    return Err(self.unsupported("a Future created on some paths only before its owner closes (D25)"));
                }
            }
        }
        Ok(closes)
    }

    fn closed_scope(&self, op: &Operation) -> Result<Option<ScopeId>, CoroError> {
        let Operation::Ownership(OwnershipOp::ScopeEnd { state, .. } | OwnershipOp::ScopeUnwind { state, .. }) = op
        else {
            return Ok(None);
        };
        match self.module.types.get(self.ty(*state)?)? {
            Type::Service(ServiceType::Scope(scope)) => Ok(Some(*scope)),
            _ => Err(self.invalid("a close consumes a scope state")),
        }
    }

    /// Whether `value` is defined at each block entry on every path, and on some path.
    fn defined(&mut self, value: ValueId, predecessors: &[Vec<BlockId>]) -> Result<(Vec<bool>, Vec<bool>), CoroError> {
        let count = self.body.blocks.len();
        let defines: Vec<bool> = self.body.blocks.iter().map(|(_, block)| {
            block.params.contains(&value) || block.insts.iter().any(|inst| {
                self.body.insts.get(*inst).is_ok_and(|inst| inst.results.contains(&value))
            })
        }).collect();
        let entry = self.body.entry.as_u32() as usize;
        let mut must = vec![true; count];
        let mut may = vec![false; count];
        must[entry] = false;
        let mut changed = true;
        while changed {
            changed = false;
            for index in 0..count {
                self.budget.step(1)?;
                let inputs = &predecessors[index];
                let all = index != entry && inputs.iter().all(|source| {
                    let source = source.as_u32() as usize;
                    must[source] || defines[source]
                });
                let any = inputs.iter().any(|source| {
                    let source = source.as_u32() as usize;
                    may[source] || defines[source]
                });
                if all != must[index] || any != may[index] {
                    must[index] = all;
                    may[index] = any;
                    changed = true;
                }
            }
        }
        Ok((must, may))
    }

    /// D20: each suspending operation with the values it saves (live after it, not its operands) and
    /// its operands. Schedule duties count as used at the close sites of their owner (review B4).
    fn suspensions(&mut self, closes: &BTreeMap<Site, Vec<ValueId>>) -> Result<Vec<Suspension>, CoroError> {
        let frames: BTreeSet<ValueId> = self.body.values.iter().filter(|(_, data)| {
            matches!(self.module.types.get(data.ty), Ok(Type::Frame(_)))
        }).map(|(value, _)| value).collect();
        let liveness = Liveness::compute(self.module, &self.body, closes, |value| frames.contains(&value),
            self.budget)?;
        let mut suspensions = Vec::new();
        for (block, data) in self.body.blocks.iter() {
            for (position, &inst) in data.insts.iter().enumerate() {
                let instruction = self.body.insts.get(inst)?;
                if !self.suspends(&instruction.op)? {
                    continue;
                }
                let site = Site::Instruction(inst);
                let mut live = liveness.after(&self.body, block, position)?;
                for result in &instruction.results {
                    live.remove(result);
                }
                suspensions.push(self.suspension(site, block, &instruction.op, live, closes, &frames));
            }
            let TermKind::InvokeOwn { op, .. } = &data.term.kind else {
                continue;
            };
            if !self.suspends(op)? {
                continue;
            }
            let mut live = BTreeSet::new();
            for target in graph::successors(&data.term.kind) {
                live.extend(liveness.live_in[target.as_u32() as usize].iter().copied());
            }
            suspensions.push(self.suspension(Site::Terminator(block), block, op, live, closes, &frames));
        }
        Ok(suspensions)
    }

    fn suspension(
        &self, site: Site, block: BlockId, op: &Operation, mut live: BTreeSet<ValueId>,
        closes: &BTreeMap<Site, Vec<ValueId>>, frames: &BTreeSet<ValueId>,
    ) -> Suspension {
        let operands: Vec<ValueId> = op.operand_refs().into_iter().filter_map(|operand| match operand.reference {
            OperandRef::Value(value) => Some(value),
            _ => None,
        }).collect();
        let duties = closes.get(&site).cloned().unwrap_or_default();
        for value in operands.iter().chain(&duties).chain(frames) {
            live.remove(value);
        }
        Suspension { site, block, saved: live.into_iter().collect(), operands, duties }
    }

    /// Insert `frame.save k` before and `frame.resume k` after the suspending operation (D20).
    fn bracket(
        &mut self, frame: ValueId, continuation: u32, suspension: &Suspension, slots: &mut Slots,
        versions: &mut BTreeMap<ValueId, Vec<ValueId>>, frames: &mut Vec<ValueId>,
    ) -> Result<Continuation, CoroError> {
        let mut typed = Vec::new();
        for value in suspension.saved.iter().chain(&suspension.operands).chain(&suspension.duties) {
            typed.push((*value, self.ty(*value)?));
        }
        let assigned = slots.assign(&self.module.types, &typed);
        let (saved_slots, operand_slots) = assigned.split_at(suspension.saved.len());
        let mut frame_slots = Vec::new();
        let mut resumed = Vec::new();
        for (&value, &slot) in suspension.saved.iter().zip(saved_slots) {
            let ty = self.ty(value)?;
            let linear = layout::linear(&self.module.types, ty);
            let mode = if linear { OperandMode::Consume } else { OperandMode::Borrow };
            frame_slots.push(FrameSlot { slot, value, mode });
            resumed.push(SavedSlot { slot, ty, linear });
        }
        let frame_ty = self.ty(frame)?;
        let save = self.body.insts.alloc(Instruction {
            results: Vec::new(),
            op: Operation::Async(AsyncOp::FrameSave { frame, slots: frame_slots, continuation }),
            facts: InstructionFacts::default(), loc: None,
        })?;
        let facts = self.default_facts(frame_ty);
        let saved_frame = self.value(frame_ty, Definition::InstResult { inst: save, index: 0 }, facts)?;
        self.body.insts.get_mut(save)?.results = vec![saved_frame];
        frames.push(saved_frame);
        let block = suspension.block;
        let site = match suspension.site {
            Site::Instruction(inst) => {
                let insts = &self.body.blocks.get(block)?.insts;
                let position = insts.iter().position(|item| *item == inst)
                    .ok_or_else(|| self.invalid("a suspension site left its block"))?;
                let resume = self.resume(saved_frame, continuation, suspension, &resumed, versions, frames)?;
                let insts = &mut self.body.blocks.get_mut(block)?.insts;
                insts.insert(position + 1, resume);
                insts.insert(position, save);
                SuspendSite::Instruction(inst)
            }
            Site::Terminator(_) => {
                self.body.blocks.get_mut(block)?.insts.push(save);
                let mut targets = graph::successors(&self.body.blocks.get(block)?.term.kind);
                targets.dedup();
                for target in targets {
                    let resume = self.resume(saved_frame, continuation, suspension, &resumed, versions, frames)?;
                    self.body.blocks.get_mut(target)?.insts.insert(0, resume);
                }
                SuspendSite::Terminator(block)
            }
        };
        Ok(Continuation { site, saved: saved_slots.to_vec(), operands: operand_slots.to_vec() })
    }

    fn resume(
        &mut self, frame: ValueId, continuation: u32, suspension: &Suspension, slots: &[SavedSlot],
        versions: &mut BTreeMap<ValueId, Vec<ValueId>>, frames: &mut Vec<ValueId>,
    ) -> Result<InstId, CoroError> {
        let resume = self.body.insts.alloc(Instruction {
            results: Vec::new(),
            op: Operation::Async(AsyncOp::FrameResume { frame, continuation, slots: slots.to_vec() }),
            facts: InstructionFacts::default(), loc: None,
        })?;
        let frame_ty = self.ty(frame)?;
        let facts = self.default_facts(frame_ty);
        let resumed_frame = self.value(frame_ty, Definition::InstResult { inst: resume, index: 0 }, facts)?;
        frames.push(resumed_frame);
        let mut results = vec![resumed_frame];
        for (index, &value) in suspension.saved.iter().enumerate() {
            let template = self.body.values.get(value)?.clone();
            let index = u16::try_from(index + 1).map_err(|_| CoroError::LimitExceeded("results"))?;
            let version = self.value(template.ty, Definition::InstResult { inst: resume, index }, template.facts)?;
            versions.entry(value).or_default().push(version);
            results.push(version);
        }
        self.body.insts.get_mut(resume)?.results = results;
        Ok(resume)
    }

    fn implicit_sites(&self, closes: &BTreeMap<Site, Vec<ValueId>>, value: ValueId) -> Vec<Site> {
        closes.iter().filter(|(_, duties)| duties.contains(&value)).map(|(site, _)| *site).collect()
    }

    /// D16, review B4: every close of a scope that owns duties names their reaching tokens in a service
    /// schedule of that scope, in creation order.
    fn schedules(
        &mut self, duties: &[Duty], closes: &BTreeMap<Site, Vec<ValueId>>,
        repaired: &BTreeMap<ValueId, BTreeMap<Site, ValueId>>,
    ) -> Result<(), CoroError> {
        if closes.is_empty() {
            return Ok(());
        }
        let protocol = self.services_protocol()?;
        for (site, tokens) in closes {
            let Some(owner) = duties.iter().find(|duty| tokens.contains(&duty.token)).map(|duty| duty.scope) else {
                continue;
            };
            let mut reaching = Vec::new();
            for token in tokens {
                let version = repaired.get(token).and_then(|sites| sites.get(site)).copied().unwrap_or(*token);
                reaching.push(ScheduleDuty::Value(version));
            }
            let exists = self.body.service_schedules.iter().any(|schedule| {
                schedule.owner == owner && schedule.duties == reaching
            });
            if exists {
                continue;
            }
            let name = format!("Sched{}", self.body.service_schedules.len());
            self.body.service_schedules.push(ServiceSchedule {
                dictionary: None, duties: reaching, name, owner, protocol,
            });
        }
        Ok(())
    }

    /// The services protocol of the module: the one the body's `await.unwind` names, else the first.
    fn services_protocol(&self) -> Result<ProtocolId, CoroError> {
        for (_, inst) in self.body.insts.iter() {
            if let Operation::Async(AsyncOp::AwaitUnwind { protocol, .. }) = inst.op {
                return Ok(protocol);
            }
        }
        self.module.protocols.iter().find(|(_, protocol)| protocol.kind == ProtocolKind::Services)
            .map(|(id, _)| id).ok_or_else(|| self.invalid("cache duties need a services protocol for their schedule"))
    }
}
