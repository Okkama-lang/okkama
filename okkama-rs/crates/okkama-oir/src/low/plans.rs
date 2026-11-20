//! Conservative finite inventories. Cyclic state equations and ambiguous joins stay with the executor.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

pub(super) type FreshSummaries = BTreeMap<FuncId, SummaryId>;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Seed {
    identity: ValueId,
    origin: CreationWitness,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Inventory {
    entries: Vec<Seed>,
    preserves: Vec<PreserveWitness>,
    joins: Vec<JoinWitness>,
}

/// The inventory of one value while its root is inferred. The witnesses of a value extend those of its inputs:
/// a chain of n scope states would copy 1 + 2 + ... + n witnesses, so a value shares its inputs' sequences and
/// only the root's are written out (#687).
#[derive(Clone, Default)]
struct Partial {
    entries: Vec<Seed>,
    preserves: Sequence<PreserveWitness>,
    joins: Sequence<JoinWitness>,
}

/// A sequence that shares the sequences it extends: the parts in order, each a shared sequence or one item.
struct Node<T> {
    parts: Vec<Part<T>>,
    len: usize,
}
enum Part<T> {
    Shared(Rc<Node<T>>),
    Item(T),
}
struct Sequence<T>(Option<Rc<Node<T>>>);

impl<T> Drop for Node<T> {
    /// A long chain would drop one node inside another: take the shared parts apart with a stack.
    fn drop(&mut self) {
        let shared = |parts: &mut Vec<Part<T>>| parts.drain(..).filter_map(|part| match part {
            Part::Shared(node) => Some(node),
            Part::Item(_) => None,
        }).collect::<Vec<_>>();
        let mut pending = shared(&mut self.parts);
        while let Some(node) = pending.pop() {
            if let Ok(mut node) = Rc::try_unwrap(node) {
                pending.extend(shared(&mut node.parts));
            }
        }
    }
}

impl<T> Clone for Sequence<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> Default for Sequence<T> {
    fn default() -> Self {
        Self(None)
    }
}
impl<T: Clone> Sequence<T> {
    fn len(&self) -> usize {
        self.0.as_ref().map_or(0, |node| node.len)
    }
    /// The items in order, written out with an explicit stack (a chain may be as long as the body).
    fn flatten(&self, budget: &mut Budget<'_>) -> Result<Vec<T>, LowError> {
        let mut items = Vec::with_capacity(self.len());
        let mut pending: Vec<&Part<T>> = self.0.iter().flat_map(|node| node.parts.iter().rev()).collect();
        while let Some(part) = pending.pop() {
            budget.step(1)?;
            match part {
                Part::Shared(node) => pending.extend(node.parts.iter().rev()),
                Part::Item(item) => items.push(item.clone()),
            }
        }
        Ok(items)
    }
}
/// A sequence under construction: shared prefixes and new items, in order.
struct Builder<T> {
    parts: Vec<Part<T>>,
    len: usize,
}
impl<T> Builder<T> {
    fn new() -> Self {
        Self { parts: vec![], len: 0 }
    }
    fn extend(&mut self, sequence: &Sequence<T>) {
        if let Some(node) = &sequence.0 {
            self.len += node.len;
            self.parts.push(Part::Shared(node.clone()));
        }
    }
    fn push(&mut self, item: T) {
        self.len += 1;
        self.parts.push(Part::Item(item));
    }
    fn finish(self) -> Sequence<T> {
        if self.parts.is_empty() {
            return Sequence(None);
        }
        Sequence(Some(Rc::new(Node { parts: self.parts, len: self.len })))
    }
}

/// The incoming edge arguments of the blocks an inventory asks for (#687), each found once per inventory. A block is
/// found by one scan of the terminators that clones only the edges into it (#889: a close's inventory asks for the
/// few blocks on its state's chain, and cleanup expansion proves one close after another on a changing body).
/// The edges into one block: each source with its argument list.
type IncomingEdges = Vec<(BlockId, Vec<EdgeArg>)>;
struct Edges {
    /// Per asked block: its incoming edges, or `None` when an unsupported terminator or a block unwind enters it
    /// (no finite inventory there).
    into: BTreeMap<BlockId, Option<IncomingEdges>>,
}
impl Edges {
    fn new(_body: &Body, _budget: &mut Budget<'_>) -> Result<Self, LowError> {
        Ok(Self { into: BTreeMap::new() })
    }
    fn scan(body: &Body, block: BlockId, budget: &mut Budget<'_>) -> Result<Option<IncomingEdges>, LowError>
    {
        let mut edges = vec![];
        for (id, data) in body.blocks.iter() {
            budget.step(1)?;
            if data.unwind.as_ref().is_some_and(|unwind| unwind.target.block == block) {
                return Ok(None);
            }
            let mut values = |target: &Target| {
                if target.block == block {
                    edges.push((id, target.args.iter().copied().map(EdgeArg::Value).collect()));
                }
            };
            match &data.term.kind {
                TermKind::Br(target) => values(target),
                TermKind::Cond { yes, no, .. } => {
                    values(yes);
                    values(no);
                }
                TermKind::Switch { cases, default, .. } => {
                    for target in cases.iter().map(|case| &case.target).chain([default]) {
                        values(target);
                    }
                }
                TermKind::InvokeOwn { normal, unwind, .. } => {
                    for edge in [normal, unwind] {
                        if edge.block == block {
                            edges.push((id, edge.args.clone()));
                        }
                    }
                }
                _ => {
                    // A non-supported predecessor invalidates finite inventory inference.
                    if graph::successors(body, id).is_ok_and(|targets| targets.contains(&block)) {
                        return Ok(None);
                    }
                }
            }
        }
        Ok(Some(edges))
    }
    /// The edges into `block` with their argument at `index`, charged by the scan and by the edges it examines.
    fn incoming(&mut self, body: &Body, block: BlockId, index: usize, budget: &mut Budget<'_>)
        -> Result<Vec<(BlockId, EdgeArg)>, LowError>
    {
        let into = match self.into.get(&block) {
            Some(into) => into,
            None => {
                let scanned = Self::scan(body, block, budget)?;
                self.into.entry(block).or_insert(scanned)
            }
        };
        let Some(edges) = into else {
            return Ok(vec![]);
        };
        budget.step(edges.len())?;
        Ok(edges.iter().filter_map(|(source, args)| args.get(index).map(|arg| (*source, *arg))).collect())
    }
}
struct Recipe {
    inputs: Vec<ValueId>,
    seed: Option<Seed>,
    alternatives: bool,
    partition: Option<(SelectionId, bool)>,
}
pub(super) fn incoming(body: &Body, block: BlockId, index: usize) -> Vec<(BlockId, EdgeArg)> {
    let mut args = vec![];
    for (id, data) in body.blocks.iter() {
        let mut edge = |target: BlockId, values: Vec<EdgeArg>| {
            if target == block && let Some(value) = values.get(index) {
                args.push((id, *value));
            }
        };
        match &data.term.kind {
            TermKind::Br(target) => edge(target.block, target.args.iter().copied().map(EdgeArg::Value).collect()),
            TermKind::Cond { yes, no, .. } => {
                for target in [yes, no] {
                    edge(target.block, target.args.iter().copied().map(EdgeArg::Value).collect());
                }
            }
            TermKind::Switch { cases, default, .. } => {
                for target in cases.iter().map(|case| &case.target).chain([default]) {
                    edge(target.block, target.args.iter().copied().map(EdgeArg::Value).collect());
                }
            }
            TermKind::InvokeOwn { normal, unwind, .. } => {
                edge(normal.block, normal.args.clone());
                edge(unwind.block, unwind.args.clone());
            }
            _ => {
                // A non-supported predecessor invalidates finite inventory inference.
                if graph::successors(body, id).is_ok_and(|targets| targets.contains(&block)) {
                    return vec![];
                }
            }
        }
        if data.unwind.as_ref().is_some_and(|unwind| unwind.target.block == block) {
            return vec![];
        }
    }
    args
}
fn outputs(body: &Body, site: &HighOperationSite) -> Option<Vec<ValueId>> {
    match site {
        HighOperationSite::Instruction(inst) => Some(body.insts.get(*inst).ok()?.results.clone()),
        HighOperationSite::Terminator(block) => {
            let TermKind::InvokeOwn { normal, .. } = &body.blocks.get(*block).ok()?.term.kind else {
                return None;
            };
            let params = &body.blocks.get(normal.block).ok()?.params;
            let mut results = BTreeMap::new();
            for (arg, param) in normal.args.iter().zip(params) {
                if let EdgeArg::Produced(EdgeResult { outcome: Outcome::Normal, slot }) = arg
                    && results.insert(*slot, *param).is_some() {
                    return None;
                }
            }
            let mut values = vec![];
            for slot in 0..results.len() {
                values.push(*results.get(&(slot as u16))?);
            }
            Some(values)
        }
    }
}
fn operation_recipe(
    body: &Body, op: &Operation, site: HighOperationSite, index: u16, fresh: &FreshSummaries,
) -> Option<Recipe> {
    let mut recipe = Recipe { inputs: vec![], seed: None, alternatives: false, partition: None };
    match op {
        Operation::Ownership(OwnershipOp::Empty {} | OwnershipOp::ScopeBegin { .. }) if index == 0 => {}
        Operation::Ownership(OwnershipOp::Move { packet }) if index == 0 => recipe.inputs.push(*packet),
        Operation::Ownership(OwnershipOp::Join { packets }) if index == 0 => recipe.inputs.clone_from(packets),
        Operation::Ownership(OwnershipOp::Admit { state, packet } | OwnershipOp::Accept { state, packet })
            if index == 0 => recipe.inputs.extend([*state, *packet]),
        Operation::Ownership(OwnershipOp::Bind { binding } | OwnershipOp::Rebind { binding }) if index == 0 => {
            recipe.inputs.extend([binding.state, binding.resource.packet]);
        }
        Operation::Ownership(OwnershipOp::Split { packet, selection }
            | OwnershipOp::SplitChecked { packet, selection }) if index < 2 => {
            recipe.inputs.push(*packet);
            recipe.partition = Some((*selection, index == 1));
        }
        Operation::Ownership(OwnershipOp::ScopeSplit { state, selection }) if index < 2 => {
            recipe.inputs.push(*state);
            recipe.partition = Some((*selection, index == 1));
        }
        Operation::Ownership(OwnershipOp::ScopeTransfer { states: TransferStates::Same(state), .. })
            if index == 0 => recipe.inputs.push(*state),
        Operation::Core(CoreOp::ObjNew { packet, construction: ConstructionStatus::Incomplete, .. })
            if index == 1 => recipe.inputs.extend(packet),
        Operation::Core(CoreOp::ObjNew { packet, construction: ConstructionStatus::Complete, .. })
            if index == 1 => {
            recipe.inputs.extend(packet);
            recipe.seed = Some(Seed {
                identity: *outputs(body, &site)?.first()?, origin: CreationWitness::Create { site },
            });
        }
        Operation::Core(CoreOp::ObjInit { object, completion: Some(completion), .. }) if index == 0 => {
            recipe.inputs.push(completion.state);
            recipe.seed = Some(Seed { identity: *object, origin: CreationWitness::Create { site } });
        }
        Operation::Call(CallOp::Direct { function, call }) if index == 1 => {
            let summary = *fresh.get(function)?;
            if call.summary != Some(summary) {
                return None;
            }
            recipe.seed = Some(Seed {
                identity: *outputs(body, &site)?.first()?,
                origin: CreationWitness::FreshResult { site, callee: *function, summary },
            });
        }
        _ => return None,
    }
    Some(recipe)
}
fn recipe(body: &Body, value: ValueId, fresh: &FreshSummaries, edges: &mut Option<Edges>, budget: &mut Budget<'_>)
    -> Result<Option<Recipe>, LowError>
{
    let Ok(data) = body.values.get(value) else {
        return Ok(None);
    };
    match data.def {
        Definition::InstResult { inst, index } => {
            let Ok(instruction) = body.insts.get(inst) else {
                return Ok(None);
            };
            Ok(operation_recipe(body, &instruction.op, HighOperationSite::Instruction(inst), index, fresh))
        }
        Definition::BlockParam { block, index } => {
            if block == body.entry {
                return Ok(None);
            }
            let edges = match edges {
                Some(edges) => edges,
                None => edges.insert(Edges::new(body, budget)?),
            };
            let args = edges.incoming(body, block, index as usize, budget)?;
            if args.is_empty() {
                return Ok(None);
            }
            let mut inputs = vec![];
            for &(source, arg) in &args {
                match arg {
                    EdgeArg::Value(value) => inputs.push(value),
                    EdgeArg::Produced(EdgeResult { outcome: Outcome::Normal, slot }) if inputs.is_empty() => {
                        let Ok(source) = body.blocks.get(source).map(|data| (source, data)) else {
                            return Ok(None);
                        };
                        let TermKind::InvokeOwn { op, normal, .. } = &source.1.term.kind else {
                            return Ok(None);
                        };
                        if normal.block != block || args.len() != 1 {
                            return Ok(None);
                        }
                        let site = HighOperationSite::Terminator(source.0);
                        return Ok(operation_recipe(body, op, site, slot, fresh));
                    }
                    _ => return Ok(None),
                }
            }
            Ok(Some(Recipe { inputs, seed: None, alternatives: true, partition: None }))
        }
    }
}
fn inventory(
    body: &Body, root: ValueId, fresh: &FreshSummaries, budget: &mut Budget<'_>,
) -> Result<Option<Inventory>, LowError> {
    let mut edges = None;
    let mut pending = vec![(root, None)];
    let mut visiting = BTreeSet::new();
    let mut known = BTreeMap::<ValueId, Partial>::new();
    while let Some((value, visited)) = pending.pop() {
        budget.step(1)?;
        if known.contains_key(&value) {
            continue;
        }
        // A value's recipe is read once: before its inputs, and kept for after them.
        let recipe = match visited {
            Some(recipe) => recipe,
            None => {
                let Some(recipe) = recipe(body, value, fresh, &mut edges, budget)? else {
                    return Ok(None);
                };
                if !visiting.insert(value) {
                    return Ok(None);
                }
                let inputs: Vec<_> = recipe.inputs.iter().rev().filter(|input| !known.contains_key(input))
                    .copied().collect();
                pending.push((value, Some(recipe)));
                pending.extend(inputs.into_iter().map(|input| (input, None)));
                continue;
            }
        };
        let mut entries = vec![];
        let mut preserves = Builder::new();
        let mut joins = Builder::new();
        for (index, input) in recipe.inputs.iter().enumerate() {
            let Some(source) = known.get(input) else {
                return Ok(None);
            };
            budget.step(source.entries.len() + 1)?;
            if recipe.alternatives && index != 0 {
                if entries != source.entries {
                    return Ok(None);
                }
            } else {
                entries.extend(source.entries.iter().cloned());
            }
            preserves.extend(&source.preserves);
            joins.extend(&source.joins);
            preserves.push(PreserveWitness { from: *input, to: value });
        }
        if let Some((selection, selected)) = recipe.partition {
            let SelectionExpr::Value(identity) = body.selections.get(selection)? else {
                return Ok(None);
            };
            if !entries.iter().any(|entry: &Seed| entry.identity == *identity) {
                return Ok(None);
            }
            entries.retain(|entry| (entry.identity == *identity) == selected);
        }

        if recipe.alternatives && recipe.inputs.len() > 1 {
            joins.push(JoinWitness { state: value, predecessors: recipe.inputs.clone() });
        }
        if let Some(seed) = recipe.seed {
            entries.push(seed);
        }
        let mut identities = BTreeSet::new();
        if entries.iter().any(|entry| !identities.insert(entry.identity)) {
            return Ok(None);
        }
        visiting.remove(&value);
        known.insert(value, Partial { entries, preserves: preserves.finish(), joins: joins.finish() });
    }
    let Some(partial) = known.remove(&root) else {
        return Ok(None);
    };
    Ok(Some(Inventory {
        entries: partial.entries,
        preserves: partial.preserves.flatten(budget)?,
        joins: partial.joins.flatten(budget)?,
    }))
}
/// Verify the actual output packet at every return, in addition to the final summary.
/// Native/opaque contracts need an explicit trusted singleton contract in a later integration.
pub(super) fn fresh_summaries(module: &Module, budget: &mut Budget<'_>) -> Result<FreshSummaries, LowError> {
    let mut fresh = BTreeMap::new();
    for (id, function) in module.functions.iter() {
        budget.step(1)?;
        let (Some(body), Some(summary_id)) = (&function.body, function.summary) else {
            continue;
        };
        let summary = module.summaries.get(summary_id)?;
        let [relation] = summary.results.as_slice() else {
            continue;
        };
        if relation.fresh.len() != 1 || !relation.aliases.is_empty() || relation.opaque
            || relation.provenance != AdmissionProvenance::UnboundResult {
            continue;
        }
        let mut returns = 0;
        let mut valid = true;
        for (_, block) in body.blocks.iter() {
            let TermKind::Ret { values } = &block.term.kind else {
                continue;
            };
            returns += 1;
            let [identity, packet, _deps] = values.as_slice() else {
                valid = false;
                break;
            };
            let output = inventory(body, *packet, &BTreeMap::new(), budget)?;
            let Some(output) = output else {
                valid = false;
                break;
            };
            if output.entries.len() != 1 || output.entries[0].identity != *identity {
                valid = false;
                break;
            }
        }
        if valid && returns != 0 {
            fresh.insert(id, summary_id);
        }
    }
    Ok(fresh)
}
fn location(body: &Body, site: &HighOperationSite) -> Option<(BlockId, usize)> {
    match site {
        HighOperationSite::Instruction(inst) => body.blocks.iter().find_map(|(block, data)| {
            data.insts.iter().position(|id| id == inst).map(|index| (block, index))
        }),
        HighOperationSite::Terminator(block) => Some((*block, body.blocks.get(*block).ok()?.insts.len())),
    }
}
fn before(
    body: &Body, first: (BlockId, usize), second: (BlockId, usize), budget: &mut Budget<'_>,
) -> Result<bool, LowError> {
    if first.0 == second.0 {
        return Ok(first.1 < second.1);
    }
    let mut pending = vec![body.entry];
    let mut seen = BTreeSet::new();
    let mut reaches_first = false;
    while let Some(block) = pending.pop() {
        budget.step(1)?;
        if !seen.insert(block) {
            continue;
        }
        if block == first.0 {
            reaches_first = true;
            continue;
        }
        if block == second.0 {
            return Ok(false);
        }
        pending.extend(graph::successors(body, block)?);
    }
    Ok(reaches_first)
}
fn origin_site(origin: &CreationWitness) -> &HighOperationSite {
    match origin {
        CreationWitness::Singleton => unreachable!("producer always records completions"),
        CreationWitness::Create { site } | CreationWitness::FreshResult { site, .. } => site,
    }
}
fn pending_empty(body: &Body, value: ValueId, budget: &mut Budget<'_>) -> Result<bool, LowError> {
    let Definition::InstResult { inst, .. } = body.values.get(value)?.def else {
        return Ok(false);
    };
    let Operation::Exit(ExitOp::Prepare { exit }) = &body.insts.get(inst)?.op else {
        return Ok(false);
    };
    let Some(packet) = exit.packet else {
        return Ok(true);
    };
    Ok(inventory(body, packet, &BTreeMap::new(), budget)?.is_some_and(|value| value.entries.is_empty()))
}
/// The `type.of` instructions of a body, in arena order (#889). Cleanup expansion splits blocks and adds cleanup
/// operations but never a `type.of`, so the list collected before it serves every close it proves.
pub(super) fn type_ofs(body: &Body, budget: &mut Budget<'_>) -> Result<Vec<InstId>, LowError> {
    budget.step(body.insts.len())?;
    Ok(body.insts.iter().filter(|(_, inst)| matches!(inst.op, Operation::Core(CoreOp::TypeOf { .. })))
        .map(|(id, _)| id).collect())
}

/// What every proof of one body reads: the fresh summaries and the body's `type.of` instructions (#889).
pub(super) struct ProofContext<'a> {
    pub fresh: &'a FreshSummaries,
    pub type_ofs: &'a [InstId],
}

pub(super) fn prove(
    module: &Module, body: &Body, state: ValueId, pending: Option<ValueId>, close: &CleanupClose,
    context: &ProofContext<'_>, budget: &mut Budget<'_>,
) -> Result<Option<CleanupOrderProof>, LowError> {
    let (fresh, type_ofs) = (context.fresh, context.type_ofs);
    let reserve_empty = match pending {
        None => true,
        Some(value) => pending_empty(body, value, budget)?,
    };
    if (!close.effects.failures.is_empty() || close.active_failure) && !reserve_empty {
        return Ok(None);
    }
    let Some(mut inventory) = inventory(body, state, fresh, budget)? else {
        return Ok(None);
    };
    if inventory.entries.is_empty() {
        return Ok(None);
    }
    // Insertion order is not serial evidence: establish every adjacent relation from the CFG.
    for index in 1..inventory.entries.len() {
        let mut current = index;
        while current > 0 {
            let Some(a) = location(body, origin_site(&inventory.entries[current - 1].origin)) else {
                return Ok(None);
            };
            let Some(b) = location(body, origin_site(&inventory.entries[current].origin)) else {
                return Ok(None);
            };
            if before(body, a, b, budget)? {
                break;
            }
            if !before(body, b, a, budget)? {
                return Ok(None);
            }
            inventory.entries.swap(current - 1, current);
            current -= 1;
        }
    }
    let at = (close.after_defers, body.blocks.get(close.after_defers)?.insts.len() + 1);
    let mut slots = vec![];
    for seed in inventory.entries.into_iter().rev() {
        let Some(created) = location(body, origin_site(&seed.origin)) else {
            return Ok(None);
        };
        if !before(body, created, at, budget)? {
            return Ok(None);
        }
        let mut target = None;
        for &id in type_ofs {
            budget.step(1)?;
            let inst = body.insts.get(id)?;
            let Operation::Core(CoreOp::TypeOf { ty, descriptor }) = &inst.op else {
                continue;
            };
            if *ty != body.values.get(seed.identity)?.ty || descriptor.managed != DescriptorManaged::Managed
                || descriptor.async_destroy {
                continue;
            }
            let Some(position) = location(body, &HighOperationSite::Instruction(id)) else {
                continue;
            };
            let contract = &module.summaries.get(descriptor.summary)?.contract;
            if contract.suspends || contract.async_destroy
                || contract.failures.iter().any(|kind| !close.effects.failures.contains(kind)) {
                continue;
            }
            if before(body, position, at, budget)? {
                target = inst.results.first().map(|value| (*value, descriptor.cleanup));
                break;
            }
        }
        let Some((descriptor, destroy)) = target else {
            return Ok(None);
        };
        slots.push(DirectSlot {
            identity: seed.identity, descriptor, destroy, origin: seed.origin, active_at: close.after_defers,
        });
    }
    let mut suffixes = vec![];
    for completed in 1..=slots.len() {
        budget.step(slots.len() - completed + 1)?;
        let remaining = slots[completed..].iter().map(|slot| slot.identity).collect();
        let roots = slots[completed..].iter().flat_map(|slot| [slot.identity, slot.descriptor]).collect();
        suffixes.push(CleanupSuffix { completed: completed as u32, remaining, roots });
    }
    Ok(Some(CleanupOrderProof {
        state, slots, preserves: inventory.preserves, joins: inventory.joins, suffixes, reserve_empty,
    }))
}

/// Producer consistency check. Does not replace the independent semantic verifier (#564).
/// Reconstructs evidence from operations and successful edges, without trusting the supplied witnesses.
pub fn check_cleanup_plans(module: &Module, options: &LowOptions) -> Result<(), LowError> {
    let mut budget = Budget { options, steps: 0, blocks: 0, roots: 0 };
    check(module, &mut budget)
}
pub(super) fn check(module: &Module, budget: &mut Budget<'_>) -> Result<(), LowError> {
    let fresh = fresh_summaries(module, budget)?;
    for (_, function) in module.functions.iter() {
        let Some(body) = &function.body else {
            continue;
        };
        let type_ofs = type_ofs(body, budget)?;
        for (_, plan) in body.cleanup.plans.iter() {
            budget.step(1)?;
            let close = body.cleanup.closes.get(plan.close)?;
            let mut sources = vec![match close.source {
                CleanupSource::Scope(_) => MergeSource::ScopeInventory,
                CleanupSource::Packet(_) => MergeSource::PacketInventory,
            }];
            if close.reserve {
                sources.push(MergeSource::WholeReserveOnFailure);
            }
            if plan.sources != sources {
                return Err(LowError::InvalidOutput("plan omits a close or reserve source".into()));
            }
            let Some(proof_id) = plan.direct else {
                continue;
            };
            let proof = body.cleanup.proofs.get(proof_id)?;
            budget.step(body.insts.len() + body.blocks.len())?;
            let begins: Vec<_> = body.insts.iter().filter_map(|(instruction, inst)| match &inst.op {
                Operation::Cleanup(CleanupOp::Begin { state, pending, close: id, effects, failure })
                    if *id == plan.close && *effects == close.effects
                        && failure.is_some() == close.active_failure && pending.is_some() == close.reserve
                        && location(body, &HighOperationSite::Instruction(instruction))
                            .is_some_and(|(block, _)| block == close.after_defers) => Some((*state, *pending)),
                _ => None,
            }).collect();
            let [(state, pending)] = begins.as_slice() else {
                return Err(LowError::InvalidOutput("direct plan has no unique matching begin".into()));
            };
            let source_matches = match close.source {
                CleanupSource::Scope(scope) => module.types.get(body.values.get(*state)?.ty)?
                    == &Type::Service(ServiceType::Scope(scope)),
                CleanupSource::Packet(_) => module.types.get(body.values.get(*state)?.ty)?
                    == &Type::Service(ServiceType::Own),
            };
            if !source_matches {
                return Err(LowError::InvalidOutput("plan has the wrong owner or packet source".into()));
            }
            let context = ProofContext { fresh: &fresh, type_ofs: &type_ofs };
            let reconstructed = prove(module, body, *state, *pending, close, &context, budget)?;
            if reconstructed.as_ref() != Some(proof) {
                return Err(LowError::InvalidOutput(
                    "direct order, active inventory, suffix or B2 evidence invalid".into(),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(options: &LowOptions) -> Budget<'_> {
        Budget { options, steps: 0, blocks: 0, roots: 0 }
    }

    /// Shared sequences write out in the order the copying inventory used: each input's sequence, then the
    /// item that extends it (#687).
    #[test]
    fn shared_sequences_write_out_in_extension_order() {
        let options = LowOptions::default();
        let mut first = Builder::new();
        first.push(1);
        first.push(2);
        let first = first.finish();
        let mut second = Builder::new();
        second.extend(&first);
        second.push(3);
        let second = second.finish();
        let mut joined = Builder::new();
        joined.extend(&second);
        joined.push(4);
        joined.extend(&first);
        joined.extend(&Sequence::default());
        joined.push(5);
        let joined = joined.finish();
        assert_eq!(joined.len(), 7);
        assert_eq!(joined.flatten(&mut budget(&options)).unwrap(), vec![1, 2, 3, 4, 1, 2, 5]);
        assert!(Builder::<u32>::new().finish().flatten(&mut budget(&options)).unwrap().is_empty());
    }

    /// A chain as long as a large body extends, writes out and drops without recursion, and writing it out
    /// is charged by its length.
    #[test]
    fn a_long_chain_is_linear_and_drops_iteratively() {
        let options = LowOptions { max_steps: usize::MAX, ..LowOptions::default() };
        let mut chain = Sequence::default();
        for item in 0..200_000_u32 {
            let mut next = Builder::new();
            next.extend(&chain);
            next.push(item);
            chain = next.finish();
        }
        let mut charged = budget(&options);
        let items = chain.flatten(&mut charged).unwrap();
        assert_eq!(items.len(), 200_000);
        assert!(items.iter().enumerate().all(|(index, item)| *item as usize == index));
        assert!(charged.steps < 3 * items.len(), "{}", charged.steps);
        drop(chain);
    }

    /// Two scope states meet at a join: equal inventories join with a witness, different ones have no finite
    /// inventory (#687 keeps the rule of the copying inventory).
    #[test]
    fn alternatives_at_a_join_must_agree() {
        let source = |second: &str| format!("oir 2 own
module joins
object Res {{
    destroy @destroy
}}
summary Md internal parameters [parameter_effect(convention=borrow, destination=none)]
    contract call_contract(failures=[thrown], publication=atomic)
func @destroy(%self: Res) -> void [extern \"fixture.destroy\", destroy Res] summary(Md);
scope S action in @main
summary M0 internal contract call_contract(publication=atomic)
func @main(%c: bool) -> void summary(M0) {{
^entry in S:
    %q0: scope<S> = scope.begin S
    %ea: own = own.empty
    invoke.own obj.new Res {{; %ea}} construction complete
        normal ^made($value, $packet, %q0)
        unwind ^failed($packet, $failure, %q0)
^made(%a: Res, %pa: own, %qa: scope<S>) in S:
    %q1: scope<S> = own.admit %qa, %pa
    cond %c, ^join(%q1), ^join({second})
^join(%joined: scope<S>) in S:
    %closed: closed = scope.end %joined, effects scope_effects()
    ret
^failed(%rest: own, %error: failure, %open: scope<S>) in S:
    raise %error
}}
");
        let options = LowOptions::default();
        let joined = |text: String| {
            let module = crate::text::parse(&text).unwrap();
            let (_, function) = module.functions.iter().find(|(_, function)| function.symbol == "main").unwrap();
            let body = function.body.as_ref().unwrap();
            let (block, _) = body.blocks.iter().find(|(_, block)| block.params.len() == 1
                && body.values.get(block.params[0]).is_ok_and(|value| {
                    matches!(module.types.get(value.ty), Ok(Type::Service(ServiceType::Scope(_))))
                }) && block.insts.iter().any(|inst| matches!(body.insts.get(*inst).map(|inst| &inst.op),
                    Ok(Operation::Ownership(OwnershipOp::ScopeEnd { .. }))))).unwrap();
            let value = body.blocks.get(block).unwrap().params[0];
            inventory(body, value, &BTreeMap::new(), &mut budget(&options)).unwrap()
        };
        let same = joined(source("%q1")).expect("equal alternatives join");
        assert_eq!(same.entries.len(), 1);
        assert_eq!(same.joins.len(), 1);
        assert_eq!(same.joins[0].predecessors.len(), 2);
        assert!(joined(source("%q0")).is_none(), "alternatives with different inventories");
    }

    /// The edge index answers every block parameter as `incoming` does, unsupported predecessors included.
    #[test]
    fn the_edge_index_agrees_with_incoming() {
        let options = LowOptions::default();
        let mut checked = 0;
        for source in [
            include_str!("fixtures/end.own.oir"), include_str!("fixtures/end.low.oir"),
            include_str!("fixtures/unwind-reserve.own.oir"), include_str!("fixtures/unwind-reserve.low.oir"),
            include_str!("fixtures/iteration.low.oir"), include_str!("fixtures/mixed-protocol.low.oir"),
            include_str!("fixtures/rebind-history.low.oir"), include_str!("fixtures/open-any.low.oir"),
            // Block unwinds enter their handler blocks, also one that a branch enters too.
            include_str!("../own/tests/protocol-high/closure-borrowed.oir"),
            "oir 2 high\nmodule unwinds\nscope S action in @f\nfunc @f(%x: int) -> int {\n^entry in S unwind ^h:\n    \
                br ^h(%x)\n^h(%v: int) in S:\n    ret %v\n}\n",
        ] {
            let module = crate::text::parse(source).unwrap();
            for (_, function) in module.functions.iter() {
                let Some(body) = &function.body else { continue };
                let mut edges = Edges::new(body, &mut budget(&options)).unwrap();
                for (block, data) in body.blocks.iter() {
                    for index in 0..data.params.len() + 1 {
                        let indexed = edges.incoming(body, block, index, &mut budget(&options)).unwrap();
                        assert_eq!(indexed, incoming(body, block, index), "{block:?} {index}");
                        checked += usize::from(!indexed.is_empty());
                    }
                }
            }
        }
        assert!(checked > 10, "{checked}");
    }
}
