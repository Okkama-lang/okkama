//! The rewrite of one function body.
use super::graph;
use super::layout::{self, Slots};
use super::liveness::{Liveness, Site};
use super::repair::{self, Variable};
use super::{Budget, CoroError};
use crate::*;
use std::collections::{BTreeMap, BTreeSet};

/// D26 R-2 (#857): a consumer (a store's absorb, a rehome) moves a duty only out of a slot that is full on every path.
const R2_MAYBE: &str = "a duty leaves a slot that is full on some paths only (D26 R-2)";

/// The owner's cache duty of one Future (C-1) and, for a spawned Future, its task duty (D24.1). `token` is the
/// cache duty as the spawn forwards it; the task duty settles first (D24.2, §6.3).
struct Duty {
    future: ValueId,
    token: ValueId,
    scope: ScopeId,
    task: Option<ValueId>,
    /// PR-B1 (#784): `fut.new … binding` — a binding holds the Future from its creation, so no store takes its
    /// duty: both tokens stay scheduled or leave by `cache.rehome`.
    bound: bool,
}

impl Duty {
    /// The duty tokens in schedule order: the task before the cache (§6.3).
    fn tokens(&self) -> impl Iterator<Item = ValueId> + '_ {
        self.task.into_iter().chain([self.token])
    }
}

/// The Services schedule that own declared for one owner (D16). Own names each duty by its Future;
/// several schedules of one owner are merged in declaration order. `dynamic` keeps own's step for the entries
/// the owner admits at run time (D25.3).
struct OwnSchedule {
    name: String,
    owner: ScopeId,
    protocol: ProtocolId,
    dictionary: Option<ValueId>,
    futures: Vec<ValueId>,
    dynamic: bool,
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
        // Rehomes first: a token that an exit rehomed is gone at the exit's split, which absorbs nothing. A stored
        // Future's duty leaves with its packet (D25.2 revision, #771), so own emits no rehome after a store, and
        // `absorbs` refuses a rehome that a store would precede.
        let mut rehomed = self.rehomes(&duties)?;
        let dynamic: BTreeSet<ScopeId> = schedules.iter().filter(|schedule| schedule.dynamic)
            .map(|schedule| schedule.owner).collect();
        self.absorbs(&duties, &dynamic, &mut rehomed)?;
        self.occupancy(&duties, &rehomed)?;
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
        let (closes, slotted) = self.closes(&duties, &rehomed)?;
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
            self.schedules(&schedules, &duties, &closes, &repaired, &rehomed, &slotted)?;
            self.body.frame_layout = Some(FrameLayout { slots: slots.decls, continuations });
        } else {
            self.schedules(&schedules, &duties, &closes, &BTreeMap::new(), &rehomed, &slotted)?;
        }
        let spawned = duties.iter().any(|duty| duty.task.is_some());
        if !rehomed.is_empty() || spawned {
            // The state results of the rewritten rehomes and spawns have no definition left.
            crate::low::compact::compact_body(&mut self.body).map_err(|message| self.invalid(&message))?;
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
    /// its Future and ends a schedule with `dynamic` when the owner admits entries at run time (D25.3); the
    /// D23 families belong to later slices.
    fn own_schedules(&mut self) -> Result<Vec<OwnSchedule>, CoroError> {
        let mut merged: Vec<OwnSchedule> = Vec::new();
        for schedule in std::mem::take(&mut self.body.service_schedules) {
            self.budget.step(schedule.duties.len() + 1)?;
            let services = self.module.protocols.get(schedule.protocol)?.kind == ProtocolKind::Services;
            if !services {
                return Err(self.unsupported("a service schedule of another protocol family (D23)"));
            }
            let mut futures = Vec::new();
            let mut dynamic = false;
            for duty in &schedule.duties {
                match duty {
                    ScheduleDuty::Value(future) => futures.push(*future),
                    ScheduleDuty::Dynamic if !dynamic => dynamic = true,
                    ScheduleDuty::Dynamic => return Err(self.invalid("a schedule names its dynamic step twice")),
                }
            }
            match merged.iter_mut().find(|item| item.owner == schedule.owner) {
                Some(item) if item.protocol != schedule.protocol || item.dictionary != schedule.dictionary => {
                    return Err(self.invalid("the Services schedules of one owner disagree (D16)"));
                }
                Some(item) => {
                    item.futures.extend(futures);
                    item.dynamic |= dynamic;
                }
                None => merged.push(OwnSchedule {
                    name: schedule.name, owner: schedule.owner, protocol: schedule.protocol,
                    dictionary: schedule.dictionary, futures, dynamic,
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
            let Operation::Async(AsyncOp::FutureNew { owner, binding, .. }) = self.body.insts.get(inst)?.op else {
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
            duties.push(Duty { future: results[0], token, scope: owner, task: None, bound: binding.is_some() });
        }
        self.spawns(&mut duties)?;
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

    /// D24.1: own's `%q1 = fut.spawn %f, state %q0` forwards the owner state; coro's form takes the cache duty of the
    /// Future's `fut.new` and gives it back with a new task-state, `(%c1, %task) = fut.spawn %f, state %c0`. The
    /// state passes through, so its later uses read the state the spawn no longer consumes.
    fn spawns(&mut self, duties: &mut [Duty]) -> Result<(), CoroError> {
        let insts: Vec<_> = self.body.blocks.iter().flat_map(|(_, block)| block.insts.clone()).collect();
        for inst in insts {
            self.budget.step(1)?;
            let Operation::Async(AsyncOp::FutureSpawn { future, state: Some(state), task_slot: None }) =
                self.body.insts.get(inst)?.op
            else {
                continue;
            };
            let duty = duties.iter_mut().find(|duty| duty.future == future)
                .ok_or_else(|| self.invalid("fut.spawn of a Future without a fut.new here (d24-spawn-not-fresh)"))?;
            if duty.task.is_some() {
                return Err(self.invalid("a Future is spawned twice"));
            }
            let results = self.body.insts.get(inst)?.results.clone();
            let [moved] = results[..] else {
                return Err(self.invalid("the own form of fut.spawn gives the owner state"));
            };
            let cache_ty = self.intern(Type::Service(ServiceType::CacheState))?;
            let task_ty = self.intern(Type::Service(ServiceType::TaskState))?;
            let cache = self.value(cache_ty, Definition::InstResult { inst, index: 0 }, ValueFacts::Service)?;
            let task = self.value(task_ty, Definition::InstResult { inst, index: 1 }, ValueFacts::Service)?;
            let instruction = self.body.insts.get_mut(inst)?;
            let spawn = AsyncOp::FutureSpawn { future, state: Some(duty.token), task_slot: None };
            instruction.op = Operation::Async(spawn);
            instruction.results = vec![cache, task];
            visit::rewrite_uses(&mut self.body, &mut repair::Replace::single(moved, state));
            duty.token = cache;
            duty.task = Some(task);
        }
        Ok(())
    }

    /// C-1, C-4: own's rehome of a Future created here moves its cache duty from the owner state; in coro the duty
    /// is the token of its `fut.new`, so the rehome takes the token form and the state passes through. A Future
    /// that reached the owner in an admitted packet has no token: its entry is dynamic (D25.3), and the own form,
    /// valid in coro, takes it from the state. Returns the rehomes that consume each token.
    fn rehomes(&mut self, duties: &[Duty]) -> Result<BTreeMap<ValueId, Vec<InstId>>, CoroError> {
        let predecessors = graph::predecessors(&self.body);
        let mut rehomed: BTreeMap<ValueId, Vec<InstId>> = BTreeMap::new();
        let insts: Vec<_> = self.body.blocks.iter().flat_map(|(_, block)| block.insts.clone()).collect();
        for inst in insts {
            self.budget.step(1)?;
            let Operation::Async(AsyncOp::CacheRehome { form: RehomeForm::Scope { state, pending, future }, target,
                protocol }) = self.body.insts.get(inst)?.op.clone()
            else {
                continue;
            };
            // The token form takes the duty from its owner; a duty that an earlier rehome moved on is an entry of
            // the scope that admitted it.
            let origin = self.origin(future, &predecessors)?;
            let owner = match self.module.types.get(self.ty(state)?)? {
                Type::Service(ServiceType::Scope(scope)) => *scope,
                _ => return Err(self.invalid("cache.rehome takes a scope state")),
            };
            let Some(duty) = duties.iter().find(|duty| Some(duty.future) == origin && duty.scope == owner) else {
                continue;
            };
            let results = self.body.insts.get(inst)?.results.clone();
            let [moved, reserve] = results[..] else {
                return Err(self.invalid("the own form of cache.rehome gives the state and the reserve"));
            };
            let instruction = self.body.insts.get_mut(inst)?;
            instruction.op = Operation::Async(AsyncOp::CacheRehome {
                form: RehomeForm::Token { pending, cache: duty.token, task: duty.task }, target, protocol,
            });
            instruction.results = vec![reserve];
            self.body.values.get_mut(reserve)?.def = Definition::InstResult { inst, index: 0 };
            // The state's other uses read the state the rehome no longer consumes.
            visit::rewrite_uses(&mut self.body, &mut repair::Replace::single(moved, state));
            for token in duty.tokens() {
                rehomed.entry(token).or_default().push(inst);
            }
        }
        Ok(rehomed)
    }

    /// D25.2, revision of 2026-11-16 (#771, #758): own moves the entry of a Future that is stored in a field or an
    /// array with a `scope.split` whose selection names the Future (key `service_of(%f, cache)`). Under PR-B1
    /// (#784) that is a fresh temporary only: a `fut.new` with `binding` is never absorbed. In coro the duty of
    /// a Future created here is its token, so the token moves with the selected packet: `service.absorb` of the
    /// task duty of a spawned Future, then of its cache duty, at the start of the split's normal block. The
    /// selection keeps naming the Future, whose entry is not in the state. Each absorbed token leaves the schedule
    /// (`consumed` gains its absorbs), so the owner never settles it a second time.
    ///
    /// A store split can fail; its failure leaves the entry in the owner's state, and its handler may be shared with
    /// paths where the token does not exist yet or was absorbed (a builder's, #802). When the owner settles entries at
    /// run time (its schedule ends with `dynamic`), the token moves there on the failure edge: `own.empty`,
    /// `service.absorb` of each token, `own.admit` into the restored state. After a store split the token exists on
    /// neither outcome, so every close reached after the `fut.new` settles it exactly once. Without `dynamic` the
    /// token stays static, and a close it reaches on some paths only is refused.
    fn absorbs(&mut self, duties: &[Duty], dynamic: &BTreeSet<ScopeId>, consumed: &mut BTreeMap<ValueId, Vec<InstId>>)
        -> Result<(), CoroError> {
        let predecessors = graph::predecessors(&self.body);
        // Splits in reverse postorder: a Future stored twice gives its entry to the first holder, and a later split
        // that names it finds the token consumed on every path (a token held on some paths only is refused).
        for block in self.reverse_postorder()? {
            self.budget.step(1)?;
            let TermKind::InvokeOwn { op, normal, unwind } = &self.body.blocks.get(block)?.term.kind else {
                continue;
            };
            let Operation::Ownership(OwnershipOp::ScopeSplit { state, selection }) = **op else {
                continue;
            };
            let normal = normal.clone();
            let unwind = unwind.clone();
            let owner = match self.module.types.get(self.ty(state)?)? {
                Type::Service(ServiceType::Scope(scope)) => *scope,
                _ => return Err(self.invalid("scope.split takes a scope state")),
            };
            let mut named = Vec::new();
            let mut pending = vec![selection];
            while let Some(selection) = pending.pop() {
                self.budget.step(1)?;
                match self.body.selections.get(selection)? {
                    SelectionExpr::Union(items) => pending.extend(items.iter().rev()),
                    SelectionExpr::Admissible(inner) => pending.push(*inner),
                    SelectionExpr::Value(value) => named.push(*value),
                    _ => {}
                }
            }
            let mut tokens = Vec::new();
            for value in named {
                let origin = self.origin(value, &predecessors)?;
                let Some(duty) = duties.iter().find(|duty| Some(duty.future) == origin && duty.scope == owner) else {
                    continue;
                };
                // PR-B1 (#784): only a fresh temporary is stored; a bound Future's store is an alias. The attribute
                // is tested before either token, so a spawned and bound Future keeps its task token too.
                if duty.bound {
                    continue;
                }
                for token in duty.tokens() {
                    let kills = consumed.get(&token).cloned().unwrap_or_default();
                    let (must, may) = self.available(token, &kills, duty.scope, &predecessors)?;
                    let index = block.as_u32() as usize;
                    let mut held = (must[index], may[index]);
                    for inst in &self.body.blocks.get(block)?.insts {
                        if self.body.insts.get(*inst)?.results.contains(&token) {
                            held = (true, true);
                        } else if self.consumes(*inst, &kills, duty.scope)? {
                            held = (false, false);
                        }
                    }
                    match held {
                        (true, _) if !tokens.contains(&token) => tokens.push(token),
                        (false, true) => return Err(self.unsupported(R2_MAYBE)),
                        _ => {}
                    }
                }
            }
            if tokens.is_empty() {
                continue;
            }
            let selected = normal.args.iter().position(|arg| {
                *arg == EdgeArg::Produced(EdgeResult { outcome: Outcome::Normal, slot: 1 })
            }).and_then(|index| self.body.blocks.get(normal.block).ok()?.params.get(index).copied())
                .ok_or_else(|| self.invalid("a scope.split passes its selected packet to its normal block"))?;
            if predecessors[normal.block.as_u32() as usize].len() != 1 {
                return Err(self.invalid("a scope.split's normal block has one predecessor"));
            }
            let own = self.ty(selected)?;
            if dynamic.contains(&owner) {
                self.absorb_on_failure(&unwind, own, &tokens, &predecessors, consumed)?;
            }
            let mut packet = selected;
            for (position, token) in tokens.into_iter().enumerate() {
                let absorbed = self.emit(normal.block, position, own,
                    Operation::Async(AsyncOp::ServiceAbsorb { packet, token }))?;
                let inst = match self.body.values.get(absorbed)?.def {
                    Definition::InstResult { inst, .. } => inst,
                    Definition::BlockParam { .. } => return Err(self.invalid("an absorb result is an instruction's")),
                };
                consumed.entry(token).or_default().push(inst);
                // The packet's later uses read the packet with the entry.
                let mut replace = repair::Replace::single(packet, absorbed);
                visit::rewrite_uses(&mut self.body, &mut replace);
                self.body.insts.get_mut(inst)?.op = Operation::Async(AsyncOp::ServiceAbsorb { packet, token });
                packet = absorbed;
            }
        }
        // A rehome takes a token its absorbs left on every path: own emits none after a store (ruling "How a
        // stored Future leaves"), so a token absorbed before its rehome is invalid input.
        let predecessors = graph::predecessors(&self.body);
        for (token, insts) in consumed.clone() {
            let Some(scope) = duties.iter().find(|duty| duty.tokens().any(|held| held == token)).map(|duty| duty.scope)
            else {
                return Err(self.invalid("a consumed token belongs to no duty"));
            };
            for &inst in &insts {
                if !matches!(self.body.insts.get(inst)?.op, Operation::Async(AsyncOp::CacheRehome { .. })) {
                    continue;
                }
                let others: Vec<_> = insts.iter().copied().filter(|other| *other != inst).collect();
                match self.held_at(token, &others, scope, inst, &predecessors)? {
                    (true, _) => {}
                    // D26 R-2: a duty leaves only a full slot.
                    (false, true) => return Err(self.unsupported(R2_MAYBE)),
                    (false, false) => return Err(self.invalid("a Future rehomed after its duty was stored (D25.2)")),
                }
            }
        }
        Ok(())
    }

    /// #802: a failed store split admits its Future's tokens into the owner's dynamic inventory, at the start of its
    /// unwind block (see `absorbs`).
    fn absorb_on_failure(&mut self, unwind: &InvokeEdge, own: TypeId, tokens: &[ValueId], predecessors: &[Vec<BlockId>],
        consumed: &mut BTreeMap<ValueId, Vec<InstId>>) -> Result<(), CoroError> {
        let restored = unwind.args.iter().position(|arg| {
            *arg == EdgeArg::Produced(EdgeResult { outcome: Outcome::Unwind, slot: 0 })
        }).and_then(|index| self.body.blocks.get(unwind.block).ok()?.params.get(index).copied())
            .ok_or_else(|| self.invalid("a scope.split passes its restored state to its unwind block"))?;
        if predecessors[unwind.block.as_u32() as usize].len() != 1 {
            return Err(self.invalid("a store split's unwind block has one predecessor"));
        }
        let mut packet = self.emit(unwind.block, 0, own, Operation::Ownership(OwnershipOp::Empty {}))?;
        for (index, &token) in tokens.iter().enumerate() {
            let absorbed = self.emit(unwind.block, 1 + index, own,
                Operation::Async(AsyncOp::ServiceAbsorb { packet, token }))?;
            let Definition::InstResult { inst, .. } = self.body.values.get(absorbed)?.def else {
                return Err(self.invalid("an absorb result is an instruction's"));
            };
            consumed.entry(token).or_default().push(inst);
            packet = absorbed;
        }
        let state = self.ty(restored)?;
        let admitted = self.emit(unwind.block, 1 + tokens.len(), state,
            Operation::Ownership(OwnershipOp::Admit { state: restored, packet }))?;
        let Definition::InstResult { inst: admit, .. } = self.body.values.get(admitted)?.def else {
            return Err(self.invalid("an admit result is an instruction's"));
        };
        // The block's later uses read the state with the admitted entries.
        let mut replace = repair::Replace::single(restored, admitted);
        visit::rewrite_uses(&mut self.body, &mut replace);
        self.body.insts.get_mut(admit)?.op = Operation::Ownership(OwnershipOp::Admit { state: restored, packet });
        Ok(())
    }

    /// Whether `token` is held just before the instruction `at` (on every path, on some path), with `kills` the
    /// instructions that consume it.
    fn held_at(&mut self, token: ValueId, kills: &[InstId], scope: ScopeId, at: InstId,
        predecessors: &[Vec<BlockId>]) -> Result<(bool, bool), CoroError> {
        let block = self.body.blocks.iter().find(|(_, block)| block.insts.contains(&at)).map(|(id, _)| id)
            .ok_or_else(|| self.invalid("an instruction outside every block"))?;
        let (must, may) = self.available(token, kills, scope, predecessors)?;
        let index = block.as_u32() as usize;
        let mut held = (must[index], may[index]);
        for inst in &self.body.blocks.get(block)?.insts {
            if *inst == at {
                break;
            }
            if self.body.insts.get(*inst)?.results.contains(&token) {
                held = (true, true);
            } else if self.consumes(*inst, kills, scope)? {
                held = (false, false);
            }
        }
        Ok(held)
    }

    /// The blocks in reverse postorder from the entry; unreachable blocks are left out.
    fn reverse_postorder(&mut self) -> Result<Vec<BlockId>, CoroError> {
        let mut order = Vec::new();
        let mut visited = BTreeSet::new();
        let mut stack = vec![(self.body.entry, false)];
        while let Some((block, done)) = stack.pop() {
            self.budget.step(1)?;
            if done {
                order.push(block);
                continue;
            }
            if !visited.insert(block) {
                continue;
            }
            stack.push((block, true));
            for successor in graph::successors(&self.body.blocks.get(block)?.term.kind).into_iter().rev() {
                if !visited.contains(&successor) {
                    stack.push((successor, false));
                }
            }
        }
        order.reverse();
        Ok(order)
    }

    /// The value that `value` stands for through block parameters, if every edge passes the same one.
    fn origin(&mut self, value: ValueId, predecessors: &[Vec<BlockId>]) -> Result<Option<ValueId>, CoroError> {
        let mut pending = vec![value];
        let mut seen = BTreeSet::new();
        let mut found = None;
        while let Some(value) = pending.pop() {
            self.budget.step(1)?;
            if !seen.insert(value) {
                continue;
            }
            match self.body.values.get(value)?.def {
                Definition::BlockParam { block, index } => {
                    for &source in &predecessors[block.as_u32() as usize] {
                        let term = &self.body.blocks.get(source)?.term.kind;
                        match graph::edge_argument(term, block, index as usize) {
                            Some(EdgeArg::Value(argument)) => pending.push(argument),
                            _ => return Ok(None),
                        }
                    }
                }
                Definition::InstResult { .. } => {
                    if found.is_some_and(|previous| previous != value) {
                        return Ok(None);
                    }
                    found = Some(value);
                }
            }
        }
        Ok(found)
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
            SelectionExpr::Held(inner) => SelectionExpr::Held(self.substituted_selection(*inner, substitution)?),
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
                    self.root_success(block, &values, &mut continue_deps)?
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
    /// A void fixed root (an async test, `async main` without a value) publishes the empty deps of a scalar
    /// taken at the entry, as the Continue of an async defer does (async ABI C-2 ruling of #691, C-6).
    fn root_success(&mut self, block: BlockId, values: &[ValueId], continue_deps: &mut Option<ValueId>)
        -> Result<PublishOutcome, CoroError> {
        let (value, deps) = match values {
            [] => {
                let deps = match continue_deps {
                    Some(deps) => *deps,
                    None => *continue_deps.insert(self.continue_deps()?),
                };
                (None, deps)
            }
            [value] => {
                if matches!(self.module.types.get(self.ty(*value)?)?, Type::Service(_)) {
                    return Err(self.invalid("a fixed root returns no service value"));
                }
                (Some(*value), self.root_deps(block, *value)?)
            }
            _ => return Err(self.invalid("a fixed root returns at most one language value")),
        };
        let own = self.intern(Type::Service(ServiceType::Own))?;
        let end = self.body.blocks.get(block)?.insts.len();
        let packet = self.emit(block, end, own, Operation::Ownership(OwnershipOp::Empty {}))?;
        Ok(PublishOutcome::Success { value, packet, deps })
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
    ///
    /// D26 (#857, duty-slot memo §13.9): a scope with a duty that reaches one of its closes on some paths only is
    /// slotted. Each of its duties becomes a slot (`slot_scope`), and each of its closes lists the reaching version
    /// of every slot: full, empty, or a merge of both. Other scopes keep the form before slots. Returns the closes
    /// and the duty token of each slot version they list.
    fn closes(&mut self, duties: &[Duty], rehomed: &BTreeMap<ValueId, Vec<InstId>>)
        -> Result<(BTreeMap<Site, Vec<ValueId>>, BTreeMap<ValueId, ValueId>), CoroError> {
        let tokens: Vec<(ValueId, ScopeId)> = duties.iter()
            .flat_map(|duty| duty.tokens().map(|token| (token, duty.scope))).collect();
        let mut slotted = BTreeSet::new();
        for &(token, scope) in &tokens {
            if self.held_at_closes(token, scope, rehomed)?.iter().any(|(_, here)| *here == (false, true)) {
                slotted.insert(scope);
            }
        }
        let mut versions: BTreeMap<ValueId, BTreeMap<Site, ValueId>> = BTreeMap::new();
        let mut token_of = BTreeMap::new();
        for &scope in &slotted {
            for (token, sites) in self.slot_scope(scope, duties, rehomed)? {
                token_of.extend(sites.values().map(|version| (*version, token)));
                versions.insert(token, sites);
            }
        }
        let mut closes: BTreeMap<Site, Vec<ValueId>> = BTreeMap::new();
        for &(token, scope) in &tokens {
            if let Some(sites) = versions.get(&token) {
                for (site, ..) in self.close_sites()?.into_iter().filter(|(.., closed)| *closed == scope) {
                    let version = sites.get(&site).copied()
                        .ok_or_else(|| self.invalid("a slot has no version at a close of its scope (D26)"))?;
                    closes.entry(site).or_default().push(version);
                }
                continue;
            }
            for (site, here) in self.held_at_closes(token, scope, rehomed)? {
                match here {
                    (true, _) => closes.entry(site).or_default().push(token),
                    (false, true) => return Err(self.invalid("an unslotted duty reaches a close on some paths only")),
                    (false, false) => {}
                }
            }
        }
        Ok((closes, token_of))
    }

    /// Every close of a scope: the site, its block, its position in the block, the scope it closes.
    fn close_sites(&self) -> Result<Vec<(Site, BlockId, usize, ScopeId)>, CoroError> {
        let mut sites = Vec::new();
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
        Ok(sites)
    }

    /// Whether `token`, a duty of `scope`, is held at each close of `scope` (on every path, on some path).
    fn held_at_closes(&mut self, token: ValueId, scope: ScopeId, consumed: &BTreeMap<ValueId, Vec<InstId>>)
        -> Result<Vec<(Site, (bool, bool))>, CoroError> {
        let predecessors = graph::predecessors(&self.body);
        let kills = consumed.get(&token).cloned().unwrap_or_default();
        let (must, may) = self.available(token, &kills, scope, &predecessors)?;
        let mut held = Vec::new();
        for (site, block, position, closed) in self.close_sites()? {
            if closed != scope {
                continue;
            }
            let index = block.as_u32() as usize;
            // The duty's last event in the block before the close, else its state at the block entry.
            let mut here = (must[index], may[index]);
            for &inst in &self.body.blocks.get(block)?.insts[..position] {
                if self.body.insts.get(inst)?.results.contains(&token) {
                    here = (true, true);
                } else if self.consumes(inst, &kills, scope)? {
                    here = (false, false);
                }
            }
            held.push((site, here));
        }
        Ok(held)
    }

    /// D26 (#857, memo §13.9): every duty of the slotted `scope` becomes a slot. An empty version follows each
    /// `scope.begin` of the scope and each consumer of the duty; the SSA repair merges the versions where paths meet,
    /// and the creation takes the version that reaches it as its slot: `fut.new … slot` for the cache duty, the
    /// spawn's `task_slot` for a spawned Future's task duty. Returns each duty token's version at each close.
    fn slot_scope(&mut self, scope: ScopeId, duties: &[Duty], consumed: &BTreeMap<ValueId, Vec<InstId>>)
        -> Result<BTreeMap<ValueId, BTreeMap<Site, ValueId>>, CoroError> {
        let begins: Vec<InstId> = self.body.insts.iter().filter(|(_, inst)| {
            matches!(inst.op, Operation::Ownership(OwnershipOp::ScopeBegin { scope: begun }) if begun == scope)
        }).map(|(inst, _)| inst).collect();
        let mut slots = Vec::new();
        for duty in duties.iter().filter(|duty| duty.scope == scope) {
            let created = self.definer(duty.future)?;
            for (token, task) in duty.task.map(|task| (task, true)).into_iter().chain([(duty.token, false)]) {
                let mut versions = Vec::new();
                for &after in begins.iter().chain(consumed.get(&token).into_iter().flatten()) {
                    versions.push(self.empty_after(after, scope, task)?);
                }
                // The cache slot fills at `fut.new`; a spawn then forwards the duty under a new name.
                let site = if task { self.definer(token)? } else { created };
                if !task && duty.task.is_some() {
                    versions.push(self.body.insts.get(created)?.results[1]);
                }
                slots.push((token, task, site, versions));
            }
        }
        let closes: Vec<Site> = self.close_sites()?.into_iter().filter(|(.., closed)| *closed == scope)
            .map(|(site, ..)| site).collect();
        let mut found = BTreeMap::new();
        for (token, task, site, versions) in slots {
            let mut implicit = closes.clone();
            implicit.push(Site::Instruction(site));
            let variable = Variable { original: token, versions, implicit };
            let mut reached = repair::repair(self.module, self.function, &mut self.body, &variable, self.budget)?;
            let slot = reached.remove(&Site::Instruction(site))
                .ok_or_else(|| self.invalid("a duty's creation has no slot version (D26)"))?;
            match &mut self.body.insts.get_mut(site)?.op {
                Operation::Async(AsyncOp::FutureSpawn { task_slot, .. }) if task => *task_slot = Some(slot),
                Operation::Async(AsyncOp::FutureNew { slot: operand, .. }) if !task => *operand = Some(slot),
                _ => return Err(self.invalid("a slot is filled only by its duty's creation (D26)")),
            }
            found.insert(token, reached);
        }
        Ok(found)
    }

    /// The instruction that defines `value`.
    fn definer(&self, value: ValueId) -> Result<InstId, CoroError> {
        match self.body.values.get(value)?.def {
            Definition::InstResult { inst, .. } => Ok(inst),
            Definition::BlockParam { .. } => Err(self.invalid("a duty is defined by an instruction")),
        }
    }

    /// D26: `duty.empty owner scope [task]` right after the instruction `after`, in its block.
    fn empty_after(&mut self, after: InstId, scope: ScopeId, task: bool) -> Result<ValueId, CoroError> {
        let (block, position) = self.body.blocks.iter().find_map(|(block, data)| {
            data.insts.iter().position(|inst| *inst == after).map(|position| (block, position))
        }).ok_or_else(|| self.invalid("an instruction outside every block"))?;
        let ty = self.intern(Type::Service(if task { ServiceType::TaskState } else { ServiceType::CacheState }))?;
        self.emit(block, position + 1, ty, Operation::Async(AsyncOp::DutyEmpty { owner: scope, task }))
    }

    /// D26 R-1, early: a duty's creation fills an empty slot, so its token is held on no path that reaches its own
    /// definition (a loop that creates the duty again before its owner settled the earlier one).
    fn occupancy(&mut self, duties: &[Duty], consumed: &BTreeMap<ValueId, Vec<InstId>>) -> Result<(), CoroError> {
        let predecessors = graph::predecessors(&self.body);
        for duty in duties {
            for token in duty.tokens() {
                let kills = consumed.get(&token).cloned().unwrap_or_default();
                let at = self.definer(token)?;
                if self.held_at(token, &kills, duty.scope, at, &predecessors)?.1 {
                    return Err(self.invalid("fut.new may fill a slot that still holds its earlier duty (D26 R-1)"));
                }
            }
        }
        Ok(())
    }

    /// Whether the instruction `inst` consumes a duty token of `scope`: one of `kills`, or a close of `scope`.
    fn consumes(&self, inst: InstId, kills: &[InstId], scope: ScopeId) -> Result<bool, CoroError> {
        Ok(kills.contains(&inst) || self.closed_scope(&self.body.insts.get(inst)?.op)? == Some(scope))
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

    /// Whether the token `value` is held at each block entry on every path, and on some path: defined and not
    /// yet consumed by one of `kills` or by a close of its owner `scope`.
    ///
    /// #759: the close of the owner settles the duty, so the token does not survive it. A duty of a loop body's
    /// scope is created anew in each iteration; without this kill the token of one iteration would flow along the
    /// back edge into the next, and the next iteration's close before the creation (an unwind) would see a duty
    /// held on some paths only.
    fn available(&mut self, value: ValueId, kills: &[InstId], scope: ScopeId, predecessors: &[Vec<BlockId>])
        -> Result<(Vec<bool>, Vec<bool>), CoroError> {
        let count = self.body.blocks.len();
        // A block's effect on the token: `Some(true)` when its last event defines it, `Some(false)` when that
        // event consumes it, `None` when it passes the entry state through.
        let mut effects: Vec<Option<bool>> = Vec::with_capacity(count);
        for (_, block) in self.body.blocks.iter() {
            let mut effect = block.params.contains(&value).then_some(true);
            for inst in &block.insts {
                if self.body.insts.get(*inst)?.results.contains(&value) {
                    effect = Some(true);
                } else if self.consumes(*inst, kills, scope)? {
                    effect = Some(false);
                }
            }
            if let TermKind::InvokeOwn { op, .. } = &block.term.kind
                && self.closed_scope(op)? == Some(scope) {
                effect = Some(false);
            }
            effects.push(effect);
        }
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
                    effects[source].unwrap_or(must[source])
                });
                let any = inputs.iter().any(|source| {
                    let source = source.as_u32() as usize;
                    effects[source].unwrap_or(may[source])
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
    /// owner that never closes after its Futures keeps own's schedule over the tokens of `fut.new`. A close of a
    /// slotted scope (D26) names slot versions, each of which `slotted` maps to its duty token.
    fn schedules(
        &mut self, own: &[OwnSchedule], duties: &[Duty], closes: &BTreeMap<Site, Vec<ValueId>>,
        repaired: &BTreeMap<ValueId, BTreeMap<Site, ValueId>>, rehomed: &BTreeMap<ValueId, Vec<InstId>>,
        slotted: &BTreeMap<ValueId, ValueId>,
    ) -> Result<(), CoroError> {
        for schedule in own {
            let tokens: Vec<ValueId> = duties.iter().filter(|duty| duty.scope == schedule.owner)
                .flat_map(Duty::tokens).collect();
            let mut versions: Vec<Vec<ScheduleDuty>> = Vec::new();
            for (site, settled) in closes {
                self.budget.step(settled.len() + 1)?;
                if !settled.iter().any(|token| tokens.contains(slotted.get(token).unwrap_or(token))) {
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
                // A rehomed duty leaves its owner with the exit (C-4): the owner never settles it.
                versions.push(tokens.iter().filter(|token| !rehomed.contains_key(token))
                    .map(|token| ScheduleDuty::Value(*token)).collect());
            }
            // D25.3: every close of the owner settles its admitted entries after the static duties.
            if schedule.dynamic {
                for version in &mut versions {
                    version.push(ScheduleDuty::Dynamic);
                }
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
