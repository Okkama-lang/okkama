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
    future: ValueId,
    token: ValueId,
    scope: ScopeId,
}

/// The Services schedule that own declared for one owner (D16). Own names each duty by its Future;
/// several schedules of one owner are merged in declaration order.
struct OwnSchedule {
    name: String,
    owner: ScopeId,
    protocol: ProtocolId,
    dictionary: Option<ValueId>,
    futures: Vec<ValueId>,
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
    // `async main` and async tests are fixed roots: observation ABI, no packets, no exit transaction.
    let fixed_root = data.signature.ownership_abi == OwnershipAbi::Observation;
    let Some(body) = module.functions.get_mut(function)?.body.take() else {
        return Ok(());
    };
    let mut lowering = Lowering { module, function, body, budget, fixed_root };
    let result = lowering.run(is_async);
    let Lowering { module, body, .. } = lowering;
    module.functions.get_mut(function)?.body = Some(body);
    result?;
    if is_async {
        // C-2: the coro body takes only its frame; the language signature stays on its summaries. A fixed
        // root keeps its observation ABI, which its summary's entry kind states.
        let frame = module.types.intern(Type::Frame(function))?;
        let ownership_abi = if fixed_root { OwnershipAbi::Observation } else { OwnershipAbi::Unelaborated };
        let signature = Signature { params: vec![frame], results: vec![], ownership_abi };
        module.functions.get_mut(function)?.signature = signature;
    }
    Ok(())
}

struct Lowering<'a, 'b> {
    module: &'a mut Module,
    function: FuncId,
    body: Body,
    budget: &'a mut Budget<'b>,
    fixed_root: bool,
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
        let schedules = self.own_schedules()?;
        let duties = self.futures(&schedules)?;
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
            self.recheck_proofs()?;
            self.schedules(&schedules, &duties, &closes, &repaired)?;
            self.body.frame_layout = Some(FrameLayout { slots: slots.decls, continuations });
        } else {
            self.schedules(&schedules, &duties, &closes, &BTreeMap::new())?;
        }
        Ok(())
    }

    fn check(&self, is_async: bool) -> Result<(), CoroError> {
        if self.body.frame_layout.is_some() {
            return Err(self.invalid("an Own body declares no frame layout"));
        }
        let declaration = self.module.functions.get(self.function)?;
        // C-6: an async defer entry is an async body; a replacement (a `return` inside it) is forbidden.
        let defer_entry = declaration.attrs.iter().any(|attr| matches!(attr, FunctionAttr::DeferEntry { .. }));
        if is_async && defer_entry && self.body.blocks.iter().any(|(_, block)| matches!(block.term.kind,
            TermKind::RetDefer { tag: DeferReturnTag::Replace(_), .. })) {
            return Err(self.invalid("an async defer entry has no replacement (C-6, async-defer-replace)"));
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

    /// D16 (ruling 1): own declares the Services schedules; coro consumes them. Own names a cache duty by
    /// its Future; dynamic entries (D25) and the D23 families belong to later slices.
    fn own_schedules(&mut self) -> Result<Vec<OwnSchedule>, CoroError> {
        let mut merged: Vec<OwnSchedule> = Vec::new();
        for schedule in std::mem::take(&mut self.body.service_schedules) {
            self.budget.step(schedule.duties.len() + 1)?;
            let services = self.module.protocols.get(schedule.protocol)?.kind == ProtocolKind::Services;
            if !services {
                return Err(self.unsupported("a service schedule of another protocol family (D23)"));
            }
            let mut futures = Vec::new();
            for duty in &schedule.duties {
                let ScheduleDuty::Value(future) = duty else {
                    return Err(self.unsupported("a dynamic schedule entry (D25)"));
                };
                futures.push(*future);
            }
            match merged.iter_mut().find(|item| item.owner == schedule.owner) {
                Some(item) if item.protocol != schedule.protocol || item.dictionary != schedule.dictionary => {
                    return Err(self.invalid("the Services schedules of one owner disagree (D16)"));
                }
                Some(item) => item.futures.extend(futures),
                None => merged.push(OwnSchedule {
                    name: schedule.name, owner: schedule.owner, protocol: schedule.protocol,
                    dictionary: schedule.dictionary, futures,
                }),
            }
        }
        Ok(merged)
    }

    /// C-1: `fut.new` keeps its environment and returns the owner's cache duty after the Future. The duties
    /// follow their order in own's schedules: each Future has exactly one entry in a schedule of its owner
    /// (D16, B10.6).
    fn futures(&mut self, schedules: &[OwnSchedule]) -> Result<Vec<Duty>, CoroError> {
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
            duties.push(Duty { future: results[0], token, scope: owner });
        }
        let mut ordered = Vec::with_capacity(duties.len());
        for schedule in schedules {
            for future in &schedule.futures {
                let position = duties.iter().position(|duty| duty.future == *future && duty.scope == schedule.owner)
                    .ok_or_else(|| self.invalid("a schedule entry names no cache duty of its owner (D16)"))?;
                ordered.push(duties.swap_remove(position));
            }
        }
        if !duties.is_empty() {
            return Err(self.invalid("a cache duty has no entry in a Services schedule of its owner (D16)"));
        }
        Ok(ordered)
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
            // C-6: `invoke.defer` of an async defer entry runs it as a shielded child frame and suspends.
            TermKind::InvokeDefer { function, .. } => {
                let target = self.module.functions.get(*function)?;
                Ok(target.attrs.contains(&FunctionAttr::Async))
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
                // #715: a target parameter's dependency names the target's own parameters, which the landing
                // does not see; its landing parameter depends on the values that the landing forwards for them.
                let substitution: BTreeMap<_, _> = target_params.iter().copied().zip(forwarded.iter().copied())
                    .collect();
                for &param in &params {
                    if let ValueFacts::Managed { deps } = self.body.values.get(param)?.facts {
                        let deps = self.substituted_deps(deps, &substitution)?;
                        self.body.values.get_mut(param)?.facts = ValueFacts::Managed { deps };
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

    /// The dependency `deps` with every value of `substitution` replaced. An expression that names none of
    /// them is shared; a changed one is a new expression.
    fn substituted_deps(&mut self, deps: DepExprId, substitution: &BTreeMap<ValueId, ValueId>)
        -> Result<DepExprId, CoroError> {
        self.budget.step(1)?;
        let swap = |value: ValueId| substitution.get(&value).copied().unwrap_or(value);
        let expression = self.body.deps.get(deps)?.clone();
        let changed = match &expression {
            DepExpr::Empty => return Ok(deps),
            DepExpr::Key(key) => DepExpr::Key(self.substituted_key(*key, substitution)?),
            DepExpr::Union(parts) => {
                let mut substituted = Vec::with_capacity(parts.len());
                for part in parts {
                    substituted.push(self.substituted_deps(*part, substitution)?);
                }
                DepExpr::Union(substituted)
            }
            DepExpr::FromSsa(value) => DepExpr::FromSsa(swap(*value)),
            DepExpr::Retention { value, selection } => DepExpr::Retention {
                value: swap(*value),
                selection: self.substituted_selection(*selection, substitution)?,
            },
        };
        if changed == expression {
            return Ok(deps);
        }
        Ok(self.body.deps.alloc(changed)?)
    }

    fn substituted_selection(&mut self, selection: SelectionId, substitution: &BTreeMap<ValueId, ValueId>)
        -> Result<SelectionId, CoroError> {
        self.budget.step(1)?;
        let swap = |value: ValueId| substitution.get(&value).copied().unwrap_or(value);
        let expression = self.body.selections.get(selection)?.clone();
        let changed = match &expression {
            SelectionExpr::Value(value) => SelectionExpr::Value(swap(*value)),
            SelectionExpr::OwnerGraph(value) => SelectionExpr::OwnerGraph(swap(*value)),
            SelectionExpr::ViewOwner(value) => SelectionExpr::ViewOwner(swap(*value)),
            SelectionExpr::Descriptor { descriptor, value } => {
                SelectionExpr::Descriptor { descriptor: swap(*descriptor), value: swap(*value) }
            }
            SelectionExpr::Projection(projection) => SelectionExpr::Projection(match projection {
                Projection::Field { value, field } => Projection::Field { value: swap(*value), field: *field },
                Projection::Index { value, index } => Projection::Index { value: swap(*value), index: swap(*index) },
                Projection::Slice { value, start, end } => {
                    Projection::Slice { value: swap(*value), start: swap(*start), end: swap(*end) }
                }
                Projection::Map { value, keys } => Projection::Map { value: swap(*value), keys: swap(*keys) },
            }),
            SelectionExpr::Admissible(inner) => {
                SelectionExpr::Admissible(self.substituted_selection(*inner, substitution)?)
            }
            SelectionExpr::Union(parts) => {
                let mut substituted = Vec::with_capacity(parts.len());
                for part in parts {
                    substituted.push(self.substituted_selection(*part, substitution)?);
                }
                SelectionExpr::Union(substituted)
            }
        };
        if changed == expression {
            return Ok(selection);
        }
        Ok(self.body.selections.alloc(changed)?)
    }

    fn substituted_key(&mut self, key: KeyExprId, substitution: &BTreeMap<ValueId, ValueId>)
        -> Result<KeyExprId, CoroError> {
        let swap = |value: ValueId| substitution.get(&value).copied().unwrap_or(value);
        let expression = self.body.keys.get(key)?.clone();
        let changed = match &expression {
            KeyExpr::Created { site, instance } => KeyExpr::Created { site: *site, instance: swap(*instance) },
            KeyExpr::ServiceOf { value, kind } => KeyExpr::ServiceOf { value: swap(*value), kind: *kind },
            KeyExpr::Runtime(value) => KeyExpr::Runtime(swap(*value)),
        };
        if changed == expression {
            return Ok(key);
        }
        Ok(self.body.keys.alloc(changed)?)
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
        let mut continue_deps = None;
        for block in blocks {
            self.budget.step(1)?;
            let data = self.body.blocks.get(block)?;
            if let TermKind::RetDefer { tag: DeferReturnTag::Continue, values } = &data.term.kind {
                if !values.is_empty() {
                    return Err(self.invalid("an async defer entry continues without values (C-6)"));
                }
                let deps = match continue_deps {
                    Some(deps) => deps,
                    None => *continue_deps.insert(self.continue_deps()?),
                };
                let own = self.intern(Type::Service(ServiceType::Own))?;
                let end = self.body.blocks.get(block)?.insts.len();
                let packet = self.emit(block, end, own, Operation::Ownership(OwnershipOp::Empty {}))?;
                self.publish(block, frame, PublishOutcome::Success { value: None, packet, deps })?;
                continue;
            }
            let data = self.body.blocks.get(block)?;
            let outcome = match &data.term.kind {
                TermKind::Ret { values } if self.fixed_root => {
                    let values = values.clone();
                    self.root_success(block, &values)?
                }
                TermKind::Ret { values } => self.success(block, values)?,
                TermKind::Raise(failure) if data.unwind.is_none() => PublishOutcome::Failure(*failure),
                TermKind::Throw(_) | TermKind::Panic(_) if data.unwind.is_some() => continue,
                TermKind::Raise(_) => continue,
                TermKind::Throw(_) | TermKind::Panic(_) | TermKind::RetDefer { .. } => {
                    return Err(self.unsupported("an async body leaves through raise after its lexical unwind"));
                }
                _ => continue,
            };
            self.publish(block, frame, outcome)?;
        }
        Ok(())
    }

    /// End `block` with the publication of `outcome` and a return.
    fn publish(&mut self, block: BlockId, frame: ValueId, outcome: PublishOutcome) -> Result<(), CoroError> {
        let publish = self.body.insts.alloc(Instruction {
            results: Vec::new(), op: Operation::Async(AsyncOp::FuturePublish { frame, outcome }),
            facts: InstructionFacts::default(), loc: None,
        })?;
        let data = self.body.blocks.get_mut(block)?;
        data.insts.push(publish);
        data.term.kind = TermKind::Ret { values: Vec::new() };
        Ok(())
    }

    /// C-6: the Continue of an async defer entry publishes an empty packet with empty deps. Only exit forms
    /// run after a close (B4.2), so the deps are taken at the entry, right after the input packet's acceptance.
    fn continue_deps(&mut self) -> Result<ValueId, CoroError> {
        let entry = self.body.entry;
        let insts = self.body.blocks.get(entry)?.insts.clone();
        let mut position = 1;
        for (index, inst) in insts.iter().enumerate() {
            if matches!(self.body.insts.get(*inst)?.op, Operation::Ownership(OwnershipOp::Accept { .. })) {
                position = index + 1;
                break;
            }
        }
        let int = self.intern(Type::Int)?;
        let unit = self.emit(entry, position, int, Operation::Const(Literal::Int("0".into())))?;
        let deps = self.intern(Type::Service(ServiceType::Deps))?;
        self.emit(entry, position + 1, deps, Operation::Ownership(OwnershipOp::Dependencies { value: unit }))
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

    /// Ruling 3: a fixed root returns its value without an exit transaction. Its success publishes that value
    /// with an empty packet (Champ draft 3 §3: the root's success packet must be empty) and the value's deps.
    fn root_success(&mut self, block: BlockId, values: &[ValueId]) -> Result<PublishOutcome, CoroError> {
        let [value] = values else {
            return Err(self.unsupported("a fixed root without a value has no deps to publish (async tests, F1)"));
        };
        if matches!(self.module.types.get(self.ty(*value)?)?, Type::Service(_)) {
            return Err(self.invalid("a fixed root returns no service value"));
        }
        let deps = self.root_deps(block, *value)?;
        let own = self.intern(Type::Service(ServiceType::Own))?;
        let end = self.body.blocks.get(block)?.insts.len();
        let packet = self.emit(block, end, own, Operation::Ownership(OwnershipOp::Empty {}))?;
        Ok(PublishOutcome::Success { value: Some(*value), packet, deps })
    }

    /// `deps.of` the value of a fixed root, taken while its scope is still open: only exit forms run after
    /// the close (B4.2). The deps are taken right before the close in the exit block, or at the end of the
    /// block whose close terminator reaches the exit through single-predecessor edges.
    fn root_deps(&mut self, exit: BlockId, value: ValueId) -> Result<ValueId, CoroError> {
        let deps = self.intern(Type::Service(ServiceType::Deps))?;
        let predecessors = graph::predecessors(&self.body);
        let mut block = exit;
        let mut value = value;
        for _ in 0..self.body.blocks.len() {
            self.budget.step(1)?;
            let insts = self.body.blocks.get(block)?.insts.clone();
            for (position, inst) in insts.into_iter().enumerate() {
                if self.closed_scope(&self.body.insts.get(inst)?.op)?.is_some() {
                    return self.emit(block, position, deps, Operation::Ownership(OwnershipOp::Dependencies { value }));
                }
            }
            let [source] = predecessors.get(block.as_u32() as usize).map_or(&[][..], Vec::as_slice) else {
                break;
            };
            let source = *source;
            let data = self.body.blocks.get(block)?;
            if let Some(index) = data.params.iter().position(|param| *param == value) {
                value = match graph::edge_argument(&self.body.blocks.get(source)?.term.kind, block, index) {
                    Some(EdgeArg::Value(passed)) => passed,
                    _ => return Err(self.invalid("a fixed root's value is produced by its close")),
                };
            }
            let closes = match &self.body.blocks.get(source)?.term.kind {
                TermKind::InvokeOwn { op, .. } => self.closed_scope(op)?.is_some(),
                _ => false,
            };
            if closes {
                let end = self.body.blocks.get(source)?.insts.len();
                return self.emit(source, end, deps, Operation::Ownership(OwnershipOp::Dependencies { value }));
            }
            block = source;
        }
        Err(self.unsupported("a fixed root's value reaches its exit through a merge after its close"))
    }

    /// Insert an instruction with one result of type `ty` at `position` of `block`.
    fn emit(&mut self, block: BlockId, position: usize, ty: TypeId, op: Operation) -> Result<ValueId, CoroError> {
        let inst = self.body.insts.alloc(Instruction {
            results: Vec::new(), op, facts: InstructionFacts::default(), loc: None,
        })?;
        let facts = self.default_facts(ty);
        let value = self.value(ty, Definition::InstResult { inst, index: 0 }, facts)?;
        self.body.insts.get_mut(inst)?.results.push(value);
        self.body.blocks.get_mut(block)?.insts.insert(position, inst);
        Ok(value)
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
                let operands = instruction.op.operand_refs();
                suspensions.push(self.suspension(site, block, &operands, live, closes, &frames));
            }
            if !self.terminator_suspends(&data.term.kind)? {
                continue;
            }
            let mut operands = Vec::new();
            match &data.term.kind {
                TermKind::InvokeOwn { op, .. } => operands = op.operand_refs(),
                TermKind::InvokeDefer { call, .. } => call.append(OperandMode::Borrow, &mut operands),
                _ => {}
            }
            let mut live = BTreeSet::new();
            for target in graph::successors(&data.term.kind) {
                live.extend(liveness.live_in[target.as_u32() as usize].iter().copied());
            }
            suspensions.push(self.suspension(Site::Terminator(block), block, &operands, live, closes, &frames));
        }
        Ok(suspensions)
    }

    fn suspension(
        &self, site: Site, block: BlockId, operands: &[ReferencedOperand], mut live: BTreeSet<ValueId>,
        closes: &BTreeMap<Site, Vec<ValueId>>, frames: &BTreeSet<ValueId>,
    ) -> Suspension {
        let operands: Vec<ValueId> = operands.iter().filter_map(|operand| match operand.reference {
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

    /// Plan §3.3 step 6 (R2): a unique-cache proof is rechecked after the suspensions are bracketed. The Future
    /// it proves may be the resumed version of a value saved before; every frame slot that holds that value or
    /// one of its earlier versions is a retained slot of its evidence (B9.2). The earlier versions stay out of
    /// the evidence's aliases: their names are dead after the save (B9.5), and the slots account for them.
    fn recheck_proofs(&mut self) -> Result<(), CoroError> {
        // The saved value of each (continuation, slot) and the (continuation, slot) of each resumed version.
        let mut saved = BTreeMap::new();
        let mut resumed = BTreeMap::new();
        let mut proofs = Vec::new();
        for (inst, data) in self.body.insts.iter() {
            self.budget.step(1)?;
            match &data.op {
                Operation::Async(AsyncOp::FrameSave { slots, continuation, .. }) => {
                    for slot in slots {
                        saved.insert((*continuation, slot.slot), slot.value);
                    }
                }
                Operation::Async(AsyncOp::FrameResume { continuation, slots, .. }) => {
                    for (slot, value) in slots.iter().zip(data.results.iter().skip(1)) {
                        resumed.insert(*value, (*continuation, slot.slot));
                    }
                }
                Operation::Async(AsyncOp::UniqueCacheProof { .. }) => proofs.push(inst),
                _ => {}
            }
        }
        for inst in proofs {
            let Operation::Async(AsyncOp::UniqueCacheProof { mut proof }) = self.body.insts.get(inst)?.op.clone()
            else {
                continue;
            };
            let mut chain = Vec::new();
            let mut current = proof.future;
            while let Some(&(continuation, slot)) = resumed.get(&current) {
                self.budget.step(1)?;
                let previous = *saved.get(&(continuation, slot))
                    .ok_or_else(|| self.invalid("a resumed version has no saved value"))?;
                if chain.contains(&previous) || previous == proof.future {
                    break;
                }
                chain.push(previous);
                current = previous;
            }
            for (&(_, slot), value) in &saved {
                if (*value == proof.future || chain.contains(value)) && !proof.evidence.frame_slots.contains(&slot) {
                    proof.evidence.frame_slots.push(slot);
                }
            }
            self.body.insts.get_mut(inst)?.op = Operation::Async(AsyncOp::UniqueCacheProof { proof });
        }
        Ok(())
    }

    fn implicit_sites(&self, closes: &BTreeMap<Site, Vec<ValueId>>, value: ValueId) -> Vec<Site> {
        closes.iter().filter(|(_, duties)| duties.contains(&value)).map(|(site, _)| *site).collect()
    }

    /// D16 (ruling 1), review B4: own's Services schedules with each Future renamed to its cache-state token.
    /// Every close of an owner names the versions of the tokens that reach it, in own's order. Closes that see
    /// the same versions share one schedule; the first keeps own's name and each further one is `name#k`. An
    /// owner that never closes after its Futures keeps own's schedule over the tokens of `fut.new`.
    fn schedules(
        &mut self, own: &[OwnSchedule], duties: &[Duty], closes: &BTreeMap<Site, Vec<ValueId>>,
        repaired: &BTreeMap<ValueId, BTreeMap<Site, ValueId>>,
    ) -> Result<(), CoroError> {
        for schedule in own {
            let tokens: Vec<ValueId> = duties.iter().filter(|duty| duty.scope == schedule.owner)
                .map(|duty| duty.token).collect();
            let mut versions: Vec<Vec<ScheduleDuty>> = Vec::new();
            for (site, settled) in closes {
                self.budget.step(settled.len() + 1)?;
                if !settled.iter().any(|token| tokens.contains(token)) {
                    continue;
                }
                let reaching: Vec<ScheduleDuty> = settled.iter().map(|token| {
                    let version = repaired.get(token).and_then(|sites| sites.get(site)).copied();
                    ScheduleDuty::Value(version.unwrap_or(*token))
                }).collect();
                if !versions.contains(&reaching) {
                    versions.push(reaching);
                }
            }
            if versions.is_empty() {
                versions.push(tokens.iter().map(|token| ScheduleDuty::Value(*token)).collect());
            }
            for (index, duties) in versions.into_iter().enumerate() {
                let name = self.schedule_name(&schedule.name, index, own);
                self.body.service_schedules.push(ServiceSchedule {
                    dictionary: schedule.dictionary, duties, name, owner: schedule.owner, protocol: schedule.protocol,
                });
            }
        }
        Ok(())
    }

    /// `name` for the first version of an own schedule, else the first `name#k` that no schedule uses.
    fn schedule_name(&self, name: &str, index: usize, own: &[OwnSchedule]) -> String {
        version_name(name, index, |candidate| {
            own.iter().any(|schedule| schedule.name == candidate)
                || self.body.service_schedules.iter().any(|schedule| schedule.name == candidate)
        })
    }
}

/// The name of version `index` of the schedule `name`: `name` itself for the first version, else the first
/// `name#k` with `k >= index` that is not `taken`.
pub(super) fn version_name(name: &str, index: usize, taken: impl Fn(&str) -> bool) -> String {
    if index == 0 {
        return name.to_owned();
    }
    let mut suffix = index;
    loop {
        let candidate = format!("{name}#{suffix}");
        if !taken(&candidate) {
            return candidate;
        }
        suffix += 1;
    }
}
