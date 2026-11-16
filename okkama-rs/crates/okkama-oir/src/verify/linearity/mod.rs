//! Bounded relational worklist. Diagnostics are emitted only by replaying the
//! fixed point; solver visits never turn a temporary absence into a rejection.
// Ordered keys contain accounting Cells only. Eq/Ord read logical labels;
// changing labels copies Rc paths and cannot mutate another ordered key.
#![allow(clippy::mutable_key_type)]
mod domain;
mod per_value;
mod shared;
mod metadata;
mod packets;
mod scopes;
mod summaries;
mod cleanup;
mod async_rules;
mod defer;
mod storage;
mod descriptors;
mod history;

use super::{EntityLocation as L, RuleId as R, State};
use crate::*;
use domain::*;
use per_value::AllocationSize;
use shared::SharedSet;
use metadata::*;
use std::collections::{BTreeSet, VecDeque};

#[cfg(any(test, feature = "verifier-differential"))]
#[derive(Default)]
struct ScheduleWork {
    visits: usize,
    alternatives: usize,
    comparisons: usize,
}
#[cfg(test)]
thread_local! {
    static LAST_SCHEDULE: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}
#[cfg(any(test, feature = "verifier-differential"))]
impl Drop for ScheduleWork {
    fn drop(&mut self) {
        super::differential::record_schedule(self.visits, self.alternatives, self.comparisons);
        #[cfg(test)]
        LAST_SCHEDULE.set((self.visits, self.alternatives));
    }
}

struct Engine<'m, 's> {
    module: &'m Module,
    function: FuncId,
    declaration: &'m Function,
    body: &'m Body,
    state: &'s mut State<'m>,
    emit: bool,
    reported: BTreeSet<(L, R)>,
    managed: &'s [bool],
    /// Types whose values may hold an obligation (`obligation_types`).
    obligations: &'s [bool],
    flow_memory: usize,
    materialized_scopes: Option<BTreeSet<ScopeId>>,
    witnesses: std::rc::Rc<Vec<bool>>,
    diagnose: bool,
    collecting: bool,
    capture_returns: bool,
    contract_invalid: bool,
    defer_stack: Vec<FuncId>,
    defer_outputs: Vec<defer::ReturnWitness>,
    defer_cache: std::collections::BTreeMap<FuncId, Vec<defer::ReturnWitness>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Slot {
    token: Token,
    journal: Token,
    refs: BTreeSet<Origin>,
    identity: Option<Origin>,
    aliases: SharedSet<ValueId>,
    contained: SharedSet<ValueId>,
    bases: SharedSet<ValueId>,
    requirements: BTreeSet<Origin>,
    lifetime: u8,
    must_use: u8,
    must_use_aliases: SharedSet<ValueId>,
    failures: u8,
    redefine: bool,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            token: Token::default(), journal: Token::default(), refs: BTreeSet::new(),
            identity: None, aliases: SharedSet::new(),
            requirements: BTreeSet::new(), lifetime: 1, must_use: 0, failures: 0,
            contained: SharedSet::new(), bases: SharedSet::new(), redefine: true,
            must_use_aliases: SharedSet::new(),
        }
    }
}

impl Slot {
    fn rename(&mut self, old: Origin, new: Origin) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::work_profile::Operation::new("mod::rename");
        let rename = |key| if key == old { new } else { key };
        self.refs = self.refs.iter().copied().map(rename).collect();
        self.requirements = self.requirements.iter().copied().map(rename).collect();
        self.identity = self.identity.map(rename);
        for token in [&mut self.token, &mut self.journal] {
            token.pieces = token.pieces.iter().cloned().map(|mut piece| {
                piece.origin = rename(piece.origin);
                piece.predicates = piece.predicates.into_iter().map(|(mut filter, selected)| {
                    filter.keys = filter.keys.into_iter().map(rename).collect();
                    filter.possible = filter.possible.into_iter().map(rename).collect();
                    (filter, selected)
                }).collect();
                piece
            }).collect();
        }
    }
}

pub(super) fn check<'m>(module: &'m Module, selected: Option<FuncId>, state: &mut State<'m>) {
    let managed = managed_types(module, state);
    let obligations = obligation_types(module, state);
    let once = once_types(module, state);
    for (id, declaration) in module.declarations.iter() {
        if let DeclarationKind::Object { fields, .. } = &declaration.kind {
            for field in fields {
                if field.once && once.get(field.ty.as_u32() as usize).copied().unwrap_or(true) {
                    state.at(L::Declaration(id), declaration.loc);
                    state.error(R::B111, "once storage must not retain a resource over its complete target/type graph");
                }
            }
        }
    }
    for (function, declaration) in module.functions.iter() {
        if selected.is_some_and(|id| id != function) {
            continue;
        }
        let Some(body) = &declaration.body else {
            continue;
        };
        if !super::graph::eligible(function, body, state) {
            continue;
        }
        if module.profile != Profile::High {
            value_facts(module, function, body, &managed, state);
        }
        if !state.array(body.values.len(), 64) {
            return;
        }
        let mut engine = Engine {
            module, function, declaration, body, state, emit: false,
            reported: BTreeSet::new(),
            managed: &managed, obligations: &obligations, flow_memory: 0, materialized_scopes: None,
            witnesses: witnesses(module, body), diagnose: true, collecting: false,
            capture_returns: false, contract_invalid: false,
            defer_stack: vec![function], defer_outputs: vec![], defer_cache: Default::default(),
        };
        engine.solve();
    }
}

impl Engine<'_, '_> {
    fn scan_work(&mut self, work: usize) -> bool {
        self.state.step(work)
    }
    fn has_closing_token(&self, flow: &Flow) -> bool {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::work_profile::Operation::new("mod::has_closing_token");
        flow.tokens.indexed().any(|(index, token)| {
            flow.availability[index] == AVAILABLE && token.cleanup.is_some()
        })
    }
    fn materialized_scope(&mut self, ancestor: ScopeId) -> Option<bool> {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::work_profile::Operation::new("mod::materialized_scope");
        if self.materialized_scopes.is_none() {
            if !self.state.step(self.body.values.len()) || !self.state.array(self.body.values.len(), 64) {
                return None;
            }
            self.materialized_scopes = Some(self.body.values.iter().filter_map(|(value, _)| {
                metadata::scope(self.module, self.body, value)
            }).collect());
        }
        if !self.state.step(1) {
            return None;
        }
        Some(self.materialized_scopes.as_ref().unwrap().contains(&ancestor))
    }
    fn clear_finish_generation(&self, branch: &mut Flow, generation: Option<Origin>) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::work_profile::Operation::new("mod::clear_finish_generation");
        // Defaults have no failure evidence. Clearing them must not materialize
        // a token and a failure entry for every SSA value on every dispatch.
        for (index, failure) in branch.failures.indexed_mut() {
            if branch.tokens[index].finish_origin == generation {
                *failure = 0;
            }
        }
        for (_, token) in branch.tokens.indexed_mut() {
            if token.finish_origin == generation {
                token.failure_origin = None;
            }
        }
    }
    fn empty_ctor_packet(&self, value: ValueId) -> bool {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::work_profile::Operation::new("mod::empty_ctor_packet");
        self.declaration.attrs.iter().any(|attr| matches!(attr, FunctionAttr::Ctor(_)))
            && super::later::value_type(self.module, self.body, value)
                == Some(&Type::Service(ServiceType::Own))
            && self.body.blocks.get(self.body.entry).is_ok_and(|entry| {
                entry.params.iter().all(|value| !self.is_managed(*value))
            })
    }
    fn error(&mut self, rule: R, message: &str) {
        if self.collecting && !self.diagnose {
            self.contract_invalid = true;
        }
        if self.emit && self.reported.insert((self.state.location.clone(), rule)) {
            self.state.error(rule, message);
        }
    }
    fn retain_flow(&mut self, flow: &Flow) -> bool {
        #[cfg(any(test, feature = "verifier-differential"))]
        let measurement = std::env::var_os("OKKAMA_634_RETAINED").map(|_| {
            flow.allocation_bytes()
        });
        #[cfg(any(test, feature = "verifier-differential"))]
        shared::retention_counts();
        let bytes = flow.charge_retained();
        #[cfg(any(test, feature = "verifier-differential"))]
        if let Some(logical) = measurement {
            let (shared, nodes) = shared::retention_counts();
            super::differential::record_retained(
                self.module.profile, &self.declaration.symbol, bytes, shared, nodes, logical,
            );
        }
        self.state.memory(bytes.saturating_add(1))
    }
    fn flow_bytes(&self, flow: &Flow) -> usize {
        flow.scratch_bytes()
    }
    fn charge_flow(&mut self, flow: &Flow) -> bool {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::work_profile::Operation::new("mod::charge_flow");
        self.charge_flow_work(flow, flow.clone_work())
    }
    fn retire_origin(&mut self, flow: &mut Flow, origin: Origin, _charged: bool) -> bool {
        flow.retire_origin(origin, self.state);
        !self.state.incomplete
    }
    fn charge_flow_work(&mut self, flow: &Flow, work: usize) -> bool {
        // Scratch clones are released after each transfer. Charge their peak,
        // and account retained alternatives separately when inserting them.
        let bytes = self.flow_bytes(flow).saturating_mul(8);
        if bytes > self.flow_memory {
            if !self.state.memory(bytes - self.flow_memory) {
                return false;
            }
            self.flow_memory = bytes;
        }
        self.state.step(work)
    }

    fn initial(&mut self) -> Flow {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::work_profile::Operation::new("mod::initial");
        let mut flow = Flow::new(self.body.values.len(), self.witnesses.clone());
        let defer_entry = self.declaration.attrs.iter().any(|attr| matches!(attr, FunctionAttr::DeferEntry { .. }));
        let call_boundary = defer_entry || self.declaration.summary.and_then(|id| self.module.summaries.get(id).ok())
            .is_some_and(|s| s.entry_kind == EntryKind::CallBoundary);
        // C-2: a D21 async body takes its input packet from its start continuation.
        let parameters = self.entry_values();
        let input = if self.start_resume().is_some() {
            parameters.iter().any(|value| super::later::value_type(self.module, self.body, *value)
                == Some(&Type::Service(ServiceType::Own)))
        } else {
            self.declaration.signature.params.iter().any(|ty| {
                self.module.types.get(*ty) == Ok(&Type::Service(ServiceType::Own))
            })
        };
        flow.entry_accepted = !matches!(self.declaration.signature.ownership_abi, OwnershipAbi::Explicit { .. })
            && !(call_boundary && input);
        if self.body.blocks.get(self.body.entry).is_err() {
            return flow;
        }
        for &value in &parameters {
            let i = value.as_u32() as usize;
            flow.availability[i] = AVAILABLE;
            if self.body.descriptor_bindings.iter().any(|binding| binding.dictionary == value) {
                flow.tokens[i].dictionary = Some(DictionarySource::Parameter(value));
            }
            flow.aliases[i].insert(value);
            flow.contained[i].insert(value);
            if linear(self.module, self.body, value) {
                let mut token = Token { scope: scope(self.module, self.body, value), ..Token::default() };
                token.sources.insert(value);
                if matches!(super::later::value_type(self.module, self.body, value),
                    Some(Type::Frame(_) | Type::Service(ServiceType::TaskState | ServiceType::Receipt
                        | ServiceType::CacheState | ServiceType::Pin))) {
                    token.service = Some(Origin::Input(value));
                }
                let permission = matches!(super::later::value_type(self.module, self.body, value),
                    Some(Type::Service(ServiceType::UniqueCache | ServiceType::Pin | ServiceType::Receipt
                        | ServiceType::TaskState | ServiceType::CleanupState | ServiceType::ExitState)));
                if !builder(self.module, self.body, value) && !permission {
                    let mut piece = Piece::new(Origin::Input(value), AdmissionProvenance::UnboundResult);
                    if !call_boundary || token.scope.is_some() {
                        piece.provenance = 63;
                    }
                    piece.owner = token.scope;
                    piece.family = true;
                    let empty_ctor_inputs = self.empty_ctor_packet(value);
                    if !empty_ctor_inputs {
                        token.pieces.insert(piece);
                    }
                }
                if let Some(scope) = token.scope {
                    flow.scopes.insert(scope, 1);
                }
                flow.tokens[i] = token;
            } else if self.is_managed(value) || matches!(super::later::value_type(self.module, self.body, value),
                Some(Type::Service(ServiceType::StableKey))) {
                flow.refs[i].insert(Origin::External(value));
                flow.identities[i] = Some(Origin::External(value));
                flow.full_deps[i].insert(Origin::External(value));
                flow.provenance.insert(Origin::External(value), provenance_bit(AdmissionProvenance::Bound));
            }
            if matches!(super::later::value_type(self.module, self.body, value), Some(Type::Future(_))) {
                flow.cache_keys.insert(Origin::External(value));
            }
            if successor(self.module, self.body, value) {
                flow.must_use[i] = 1;
                flow.must_use_aliases[i].insert(value);
            }
            if self.body.values.get(value).ok().and_then(|v| self.module.types.get(v.ty).ok())
                == Some(&Type::Service(ServiceType::Failure)) {
                flow.failures[i] = 31;
                flow.tokens[i].failure_origin = Some(Origin::Input(value));
            }
        }
        // A paired deps parameter is an ABI precondition, checked at direct
        // calls below. Its symbolic entry graph is the argument's full graph.
        for &value in &parameters {
            let Ok(ValueData { facts: ValueFacts::Managed { deps }, .. }) = self.body.values.get(value) else {
                continue;
            };
            if let Ok(DepExpr::FromSsa(parameter)) = self.body.deps.get(*deps)
                && parameters.contains(parameter) {
                let required = flow.full_deps[value.as_u32() as usize].clone();
                flow.refs[parameter.as_u32() as usize].extend(&required);
                flow.full_deps[parameter.as_u32() as usize].extend(required);
            }
        }
        if call_boundary {
            let keys: BTreeSet<_> = parameters.iter().flat_map(|v| {
                flow.refs[v.as_u32() as usize].iter().copied()
            }).collect();
            if keys.is_empty() {
                flow.entry_accepted = true;
            }
            let input_labels: SharedSet<_> = parameters.iter().copied().filter(|value| {
                flow.witnesses[value.as_u32() as usize] && !flow.refs[value.as_u32() as usize].is_empty()
            }).collect();
            for &value in &parameters {
                let token = &mut flow.tokens[value.as_u32() as usize];
                if self.body.values.get(value).ok().and_then(|v| self.module.types.get(v.ty).ok())
                    != Some(&Type::Service(ServiceType::Own)) { continue; }
                if keys.is_empty() {
                    token.pieces.clear();
                }
                else {
                    token.pieces = token.pieces.iter().cloned().map(|mut piece| {
                        piece.predicates.insert(Filter {
                            keys: keys.clone(), fresh: false, possible: BTreeSet::new(),
                            labels: input_labels.clone(), stale: false, whole_future: false, futures: false, state: 0,
                        }, true);
                        piece
                    }).collect();
                }
            }
        }
        for &value in &parameters {
            self.state.at(L::Value { function: self.function, value }, None);
            let expected = flow.full_deps[value.as_u32() as usize].clone();
            self.check_deps(&mut flow, value, &expected);
        }
        self.start_frame(&mut flow);
        flow
    }
    fn is_managed(&self, value: ValueId) -> bool {
        self.body.values.get(value).is_ok_and(|v| !matches!(v.facts, ValueFacts::Plain {
            certificate: PlainCertificate::Descriptor(_),
        }) && (matches!(v.facts, ValueFacts::Managed { .. })
            || self.managed.get(v.ty.as_u32() as usize).copied().unwrap_or(true)))
    }
    fn solve(&mut self) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let before = (self.state.stats.memory_bytes, self.state.stats.steps);
        #[cfg(any(test, feature = "verifier-differential"))]
        super::work_profile::begin();
        self.solve_inner();
        #[cfg(any(test, feature = "verifier-differential"))]
        super::work_profile::finish(self.module.profile, &self.declaration.symbol);
        #[cfg(any(test, feature = "verifier-differential"))]
        super::differential::record_function(
            self.module.profile, &self.declaration.symbol, false, before,
            &self.state.stats, self.flow_memory, self.state.incomplete,
        );
    }
    fn solve_inner(&mut self) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::work_profile::Operation::new("mod::solve_inner");
        // ReturnWitness translation inspects the entire callee Flow. Keep that
        // history intact until a separate proof covers the cross-body observer.
        let history = if self.capture_returns { None }
            else { history::BoundaryHistory::new(self.module, self.body, self.state) };
        if self.state.incomplete {
            return;
        }
        if !self.state.array(self.body.blocks.len(), 64) {
            return;
        }
        let mut incoming: Vec<Vec<Flow>> = vec![Vec::new(); self.body.blocks.len()];
        if !self.state.array(self.body.values.len(), 320) {
            return;
        }
        // Flow locals live on the stack. Only vectors of Flow allocate their
        // inline headers on the heap; charge their capacities separately.
        let outgoing = self.body.blocks.iter().map(|(_, block)| targets(&block.term.kind).len()).max().unwrap_or(0);
        let outgoing_capacity = if outgoing == 0 { 0 } else { outgoing.next_power_of_two().max(4) };
        if !self.state.array(outgoing_capacity, size_of::<(BlockId, Flow)>()) {
            return;
        }
        #[cfg(any(test, feature = "verifier-differential"))]
        let mut schedule = ScheduleWork { alternatives: 1, ..ScheduleWork::default() };
        let mut initial = self.initial();
        if let Some(history) = &history {
            history.normalize(self.body.entry, &mut initial, self.state);
        }
        if !self.charge_flow(&initial) || !self.retain_flow(&initial) {
            return;
        }
        incoming[self.body.entry.as_u32() as usize].reserve_exact(1);
        incoming[self.body.entry.as_u32() as usize].push(initial);
        // State indices are stable: alternatives are appended and only masks
        // are joined in place. One dirty bit coalesces versions awaiting a visit.
        // Every new or changed mask is queued; a queued visit reads the latest
        // mask, and a later change queues another visit. Thus every state is
        // processed with its final mask before replay emits diagnostics.
        // Verdict equality with the old block schedule is checked by the differential.
        let mut dirty = vec![Vec::new(); self.body.blocks.len()];
        dirty[self.body.entry.as_u32() as usize].reserve_exact(1);
        dirty[self.body.entry.as_u32() as usize].push(true);
        let mut queue = VecDeque::from([(self.body.entry, 0)]);
        let mut queue_peak = 1;
        if !self.state.array(queue_peak, size_of::<(BlockId, usize)>()) {
            return;
        }
        while let Some((block, state_id)) = queue.pop_front() {
            let index = block.as_u32() as usize;
            dirty[index][state_id] = false;
            #[cfg(any(test, feature = "verifier-differential"))]
            { schedule.visits += 1; }
            if !self.state.step(1) || !self.charge_flow(&incoming[index][state_id]) {
                return;
            }
            let input = incoming[index][state_id].clone();
            for (target, mut flow) in self.block(block, input) {
                // The outgoing clone existed before pruning. Reserve its
                // unpruned scratch bound before deleting any slots.
                if !self.charge_flow(&flow) {
                    return;
                }
                if let Some(history) = &history {
                    history.normalize(target, &mut flow, self.state);
                }
                let index = target.as_u32() as usize;
                let Some(states) = incoming.get_mut(index) else {
                    continue;
                };
                let mut found = None;
                for (id, old) in states.iter().enumerate() {
                    #[cfg(any(test, feature = "verifier-differential"))]
                    { schedule.comparisons += 1; }
                    if old.same_relations(&flow, self.state) {
                        found = Some(id);
                        break;
                    }
                }
                if self.state.incomplete {
                    return;
                }
                let (id, changed) = if let Some(id) = found {
                    if !self.state.step(flow.mask_work()) {
                        return;
                    }
                    (id, states[id].join_masks(&flow))
                } else {
                    if !self.retain_flow(&flow) {
                        return;
                    }
                    #[cfg(any(test, feature = "verifier-differential"))]
                    { schedule.alternatives += 1; }
                    let id = states.len();
                    states.reserve_exact(1);
                    states.push(flow);
                    dirty[index].reserve_exact(1);
                    dirty[index].push(false);
                    (id, true)
                };
                if changed && !dirty[index][id] {
                    dirty[index][id] = true;
                    if queue.len() == queue_peak {
                        if !self.state.array(queue_peak, size_of::<(BlockId, usize)>()) {
                            return;
                        }
                        queue_peak = queue_peak.saturating_mul(2);
                    }
                    queue.push_back((target, id));
                }
            }
        }
        self.emit = self.diagnose;
        self.collecting = self.capture_returns;
        self.initial();
        for (block, _) in self.body.blocks.iter() {
            for flow in &incoming[block.as_u32() as usize] {
                if !self.charge_flow(flow) {
                    return;
                }
                self.block(block, flow.clone());
            }
        }
    }
    fn use_value(&mut self, flow: &mut Flow, value: ValueId, mode: OperandMode, rule: R) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::work_profile::Operation::new("mod::use_value");
        let index = value.as_u32() as usize;
        if flow.tokens[index].pin_deps.iter().any(|key| flow.dead.contains(key)) {
            self.error(R::B96, "retained view pin outlives its stable owner");
        }
        if flow.tokens[index].unique.is_some_and(|key| flow.dead.contains(&key)) {
            self.error(R::B92, "unique-cache capability outlives the cache owner");
        }
        let reservation = self.body.values.get(value).ok().and_then(|v| self.module.types.get(v.ty).ok())
            == Some(&Type::Service(ServiceType::Pending));
        if !reservation {
            self.live_use(flow, value);
        }
        if linear(self.module, self.body, value) {
            if flow.availability[index] != AVAILABLE {
                let pending = self.body.values.get(value).ok().and_then(|v| self.module.types.get(v.ty).ok())
                    == Some(&Type::Service(ServiceType::Pending));
                let rule = if pending { R::B101 }
                    else if builder(self.module, self.body, value) { R::B81 } else { rule };
                self.error(rule, "linear carrier is not available on every reaching path");
            }
            if matches!(mode, OperandMode::Consume | OperandMode::Forward) {
                flow.retire(value);
            }
        }
    }
    fn define(&mut self, flow: &mut Flow, value: ValueId, mut slot: Slot) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::work_profile::Operation::new("mod::define");
        if !self.state.step(32 * (2 + self.body.values.len().max(1).ilog2() as usize)) {
            return;
        }
        let i = value.as_u32() as usize;
        if slot.redefine && flow.availability[i] != ABSENT {
            if !self.state.step(flow.relation_work()) {
                return;
            }
            for key in flow.projection_keys() {
                let Origin::Projection(base, kind) = key else {
                    continue;
                };
                let changes = base == value || match kind {
                    ProjectionKind::Index(index) | ProjectionKind::Map(index) | ProjectionKind::Remove(index)
                    | ProjectionKind::MapIndex(index, _) => {
                        index == value
                    }
                    ProjectionKind::Slice(start, end) => start == value || end == value,
                    _ => false,
                };
                if changes {
                    flow.retire_origin(key, self.state);
                    slot.rename(key, Origin::RetiredProjection(base, kind));
                }
            }
        }
        let core = linear(self.module, self.body, value);
            if core && flow.availability[i] & AVAILABLE != 0 {
            self.error(if builder(self.module, self.body, value) { R::B81 } else { R::B22 },
                "new incarnation overwrites an available linear carrier");
        }
        if successor(self.module, self.body, value) && flow.must_use[i] == 1 {
            self.error(R::B82, "successor incarnation overwritten before a permitted use");
        }
        if slot.aliases != flow.aliases[i] || !slot.aliases.contains(&value) {
            let old = flow.aliases[i].clone();
            let mut retained = old.clone();
            retained.remove(&value);
            flow.replace_aliases(&old, &retained, self.state);
        }
        let source = slot.aliases.clone();
        let mut aliases = source.clone();
        aliases.insert(value);
        flow.replace_aliases(&source, &aliases, self.state);
        flow.aliases[i] = aliases.clone();
        if !self.state.step(slot.contained.comparison_work(&source)) {
            return;
        }
        if slot.contained == source {
            flow.contained.set(i, aliases.clone());
        } else {
            flow.contained[i] = slot.contained.clone();
            flow.contained[i].extend(&aliases);
        }
        for base in &slot.bases {
            let roots = flow.aliases[base.as_u32() as usize].clone();
            flow.add_contained(&roots, &aliases, self.state);
        }
        flow.availability[i] = AVAILABLE;
        flow.tokens[i] = flow.normalize_token(slot.token, self.state);
        flow.journals[i] = flow.normalize_token(slot.journal, self.state);
        let service_deps = self.body.values.get(value).ok().and_then(|v| self.module.types.get(v.ty).ok())
            .is_some_and(|ty| matches!(ty, Type::Service(ServiceType::Deps | ServiceType::Pending
                | ServiceType::StableKey
                | ServiceType::CleanupState | ServiceType::ExitState | ServiceType::OwnerSnapshot)));
        flow.refs[i] = if self.is_managed(value) || service_deps { slot.refs }
            else { BTreeSet::new() };
        flow.full_deps[i] = flow.refs[i].union(&slot.requirements).copied().collect();
        flow.lifetimes[i] = slot.lifetime;
        flow.failures[i] = slot.failures;
        flow.identities[i] = slot.identity;
        // Must-use aliases only cross SSA forwarding and role conversions. An
        // exit.commit creates a new obligation even when ownership identity aliases.
        let mut previous = flow.must_use_aliases[i].clone();
        previous.remove(&value);
        for &alias in &previous {
            flow.must_use_aliases.set(alias.as_u32() as usize, previous.clone());
        }
        let mut aliases = slot.must_use_aliases;
        if successor(self.module, self.body, value) {
            aliases.insert(value);
        }
        for &alias in &aliases {
            flow.must_use_aliases.set(alias.as_u32() as usize, aliases.clone());
        }
        flow.must_use_aliases.set(i, aliases);
        flow.must_use[i] = if successor(self.module, self.body, value) && slot.must_use == 0 { 1 }
            else { slot.must_use };
    }
    fn block(&mut self, id: BlockId, mut flow: Flow) -> Vec<(BlockId, Flow)> {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::work_profile::Operation::new("mod::block");
        let Ok(block) = self.body.blocks.get(id) else {
            return Vec::new();
        };
        let mut outgoing = Vec::new();
        for &instruction in &block.insts {
            let Ok(inst) = self.body.insts.get(instruction) else {
                continue;
            };
            self.state.at(L::Instruction { function: self.function, instruction }, inst.loc);
            let Ok(contract) = ContractRegistry::new(self.module).construction_schema(&inst.op, self.body) else {
                continue;
            };
            for outcome in [Outcome::Unwind, Outcome::Cancel] {
                if let Some(schema) = contract.outcome(outcome) {
                    let mut failed = flow.clone();
                    if self.module.profile != Profile::High && schema.results.iter().any(|slot| {
                        matches!(slot.ty, ResultType::Service(service) if service.is_linear())
                            || matches!(slot.ty, ResultType::SameAs(v) if linear(self.module, self.body, v))
                    }) {
                        self.error(R::B63, "exceptional linear results require an invoke.own edge");
                    }
                    self.operation(id, &inst.op, schema, outcome,
                        Origin::Instruction(instruction, 0), &mut failed);
                    if let Some(unwind) = &block.unwind {
                        outgoing.push(self.edge(&failed, unwind.target.block,
                            &unwind.target.args.iter().copied().map(EdgeArg::Value).collect::<Vec<_>>(), &[], R::B63));
                    } else {
                        self.finish(&mut failed, false, &[]);
                    }
                }
            }
            let slots = self.operation(id, &inst.op, &contract.normal, Outcome::Normal,
                Origin::Instruction(instruction, 0), &mut flow);
            for (value, slot) in inst.results.iter().copied().zip(slots) {
                self.define(&mut flow, value, slot);
                let expected = flow.full_deps[value.as_u32() as usize].clone();
                self.check_deps(&mut flow, value, &expected);
            }
            self.facts(&flow, &inst.op, &inst.facts, &inst.results);
        }
        self.state.at(L::Terminator { function: self.function, block: id }, block.term.loc);
        match &block.term.kind {
            TermKind::InvokeOwn { op, normal, unwind } => {
                if let Ok(contract) = ContractRegistry::new(self.module).construction_schema(op, self.body) {
                    for (outcome, edge) in [(Outcome::Normal, normal), (Outcome::Unwind, unwind),
                        (Outcome::Cancel, unwind)] {
                        if let Some(schema) = contract.outcome(outcome) {
                            let mut branch = flow.clone();
                            let slots = self.operation(id, op, schema, outcome, Origin::Invoke(id, 0), &mut branch);
                            let rule = if outcome != Outcome::Normal && matches!(**op, Operation::Call(_)) {
                                R::B54
                            } else { R::B21 };
                            outgoing.push(self.edge(&branch, edge.block, &edge.args, &slots, rule));
                        }
                    }
                }
            }
            TermKind::ExitDispatch { .. } | TermKind::FailureDispatch { .. } => {
                self.cleanup_dispatch(&mut flow, &block.term.kind, &mut outgoing);
            }
            TermKind::CleanupNext { cleanup, selected, done } => {
                self.cleanup_next(&mut flow, *cleanup, selected, done, &mut outgoing);
            }
            TermKind::InvokeDefer { function, call, continue_edge, replacements, unwind } => {
                self.defer_invoke(id, *function, call, continue_edge, replacements, unwind.as_ref(),
                    &flow, &mut outgoing);
            }
            TermKind::RetDefer { tag, values } => {
                if self.collecting {
                    if !self.state.memory(flow.charge_retained()) {
                        return outgoing;
                    }
                    self.defer_outputs.push(defer::ReturnWitness {
                        tag: tag.clone(), values: values.clone(), flow: flow.clone(),
                    });
                }
                self.defer_return(tag, values, &flow);
                let normal = !matches!(tag, DeferReturnTag::Continue) || values.is_empty();
                self.finish(&mut flow, normal, values);
            }
            TermKind::Ret { values } => {
                let normal = self.ret_normal(block);
                self.finish(&mut flow, normal, values);
            }
            TermKind::Throw(v) | TermKind::Raise(v) | TermKind::Panic(v) => {
                self.live_use(&flow, *v);
                if flow.active_failure != 0 && !matches!(block.term.kind, TermKind::Raise(_)) {
                    self.error(R::B104, "cleanup cannot reclassify an active failure as panic or typed throw");
                }
                if matches!(block.term.kind, TermKind::Raise(_))
                    && flow.active_failure_origin.is_some()
                    && flow.active_failure_origin != flow.tokens[v.as_u32() as usize].failure_origin {
                    self.error(R::B104, "cleanup cannot substitute a different primary Error/failure view");
                }
                flow.active_failure_origin = if matches!(block.term.kind, TermKind::Raise(_)) {
                    flow.tokens[v.as_u32() as usize].failure_origin
                } else { Some(Origin::External(*v)) };
                flow.active_failure = match &block.term.kind {
                    TermKind::Throw(_) => failure_bit(FailureKind::Thrown),
                    TermKind::Panic(_) => failure_bit(FailureKind::Panic),
                    _ => flow.failures[v.as_u32() as usize],
                };
                flow.latest_failure = flow.active_failure;
                flow.latest_failure_origin = flow.active_failure_origin;
                if let Some(unwind) = &block.unwind {
                    outgoing.push(self.edge(&flow, unwind.target.block,
                        &unwind.target.args.iter().copied().map(EdgeArg::Value).collect::<Vec<_>>(), &[], R::B63));
                } else { self.finish(&mut flow, false, &[]); }
            }
            TermKind::Trap { .. } => {
                // Spec §9.2: program failure follows the ordinary unwind continuation.
                flow.active_failure = failure_bit(FailureKind::Program);
                flow.active_failure_origin = Some(Origin::Invoke(id, u32::MAX));
                flow.latest_failure = flow.active_failure;
                flow.latest_failure_origin = flow.active_failure_origin;
                if let Some(unwind) = &block.unwind {
                    let args = unwind.target.args.iter().copied().map(EdgeArg::Value).collect::<Vec<_>>();
                    outgoing.push(self.edge(&flow, unwind.target.block, &args, &[], R::B63));
                } else {
                    self.finish(&mut flow, false, &[]);
                }
            }
            TermKind::Unreachable => {
                flow.active_failure = failure_bit(FailureKind::Program);
                self.finish(&mut flow, false, &[]);
            }
            TermKind::Lookup { kind, collection, key, found, missing } => {
                self.live_use(&flow, *collection);
                self.live_use(&flow, *key);
                let projection = match kind {
                    LookupKind::Array => ProjectionKind::Index(*key),
                    LookupKind::Map => ProjectionKind::Map(*key),
                };
                let view = Origin::Projection(*collection, projection);
                let mut found_flow = flow.clone();
                found_flow.views.insert(view, flow.refs[collection.as_u32() as usize].clone());
                let slot = Slot {
                    refs: BTreeSet::from([view]), bases: SharedSet::from([*collection]),
                    lifetime: flow.lifetimes[collection.as_u32() as usize], ..Slot::default()
                };
                outgoing.push(self.edge(&found_flow, found.block, &found.args, &[slot], R::B21));
                let args = missing.args.iter().copied().map(EdgeArg::Value).collect::<Vec<_>>();
                outgoing.push(self.edge(&flow, missing.block, &args, &[], R::B21));
            }
            TermKind::Catch { failure, handler, next, .. } => {
                self.live_use(&flow, *failure);
                let mut caught = flow.clone();
                if flow.tokens[failure.as_u32() as usize].failure_origin == flow.active_failure_origin {
                    caught.active_failure = 0;
                    caught.active_failure_origin = None;
                }
                outgoing.push(self.edge(&caught, handler.block, &handler.args, &[Slot::default()], R::B21));
                let args = next.args.iter().copied().map(EdgeArg::Value).collect::<Vec<_>>();
                outgoing.push(self.edge(&flow, next.block, &args, &[], R::B21));
            }
            _ => {
                match &block.term.kind {
                    TermKind::Cond { condition, .. } | TermKind::Switch { value: condition, .. } => {
                        self.live_use(&flow, *condition);
                    }
                    _ => {}
                }
                for (target, args) in targets(&block.term.kind) {
                    outgoing.push(self.edge(&flow, target, &args, &[], R::B21));
                }
            }
        }
        outgoing
    }
    fn edge(&mut self, flow: &Flow, target: BlockId, args: &[EdgeArg], slots: &[Slot], rule: R) -> (BlockId, Flow) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::work_profile::Operation::new("mod::edge");
        if !self.charge_flow(flow) {
            return (target, flow.clone());
        }
        let mut next = flow.clone();
        if !self.state.step(flow.tokens.stored_len()) {
            return (target, flow.clone());
        }
        for (index, token) in flow.tokens.indexed() {
            if flow.availability[index] == AVAILABLE && token.unique.is_some()
                && !token.unique_region.contains(&target) {
                self.error(R::B92, "unique capability leaves its proved CFG region");
            }
        }
        let mut saved = Vec::new();
        for arg in args {
            let slot = match arg {
                EdgeArg::Value(value) => {
                    let i = value.as_u32() as usize;
                    let slot = Slot {
                        token: flow.tokens[i].clone(), journal: flow.journals[i].clone(),
                        refs: flow.refs[i].clone(), must_use: flow.must_use[i],
                        must_use_aliases: flow.must_use_aliases[i].clone(),
                        failures: flow.failures[i], identity: flow.identities[i], aliases: flow.aliases[i].clone(),
                        requirements: flow.full_deps[i].clone(), lifetime: flow.lifetimes[i],
                        contained: flow.contained[i].clone(), bases: SharedSet::new(), redefine: true,
                    };
                    self.use_value(&mut next, *value, OperandMode::Forward, rule);
                    next.must_use[i] = 0;
                    slot
                }
                EdgeArg::Produced(result) => slots.get(usize::from(result.slot)).cloned().unwrap_or_default(),
            };
            saved.push(slot);
        }
        if let Ok(block) = self.body.blocks.get(target) {
            if block.params.len() == saved.len() + 1 && self.body.values.get(block.params[saved.len()]).ok()
                .and_then(|v| self.module.types.get(v.ty).ok()) == Some(&Type::Service(ServiceType::Failure)) {
                let token = Token { failure_origin: next.latest_failure_origin, ..Token::default() };
                saved.push(Slot { token, failures: next.latest_failure, ..Slot::default() });
            }
            for (index, slot) in saved.iter_mut().enumerate() {
                slot.redefine = args.get(index) != block.params.get(index).map(|value| EdgeArg::Value(*value)).as_ref();
            }
            let replaced: SharedSet<_> = block.params.iter().copied().collect();
            let originals: Vec<_> = saved.iter().map(|slot| slot.aliases.clone()).collect();
            let mut roots = BTreeSet::new();
            let mut classes = BTreeSet::new();
            for aliases in next.aliases.stored() {
                if !self.state.step(2 + next.aliases.stored_len().max(1).ilog2() as usize) {
                    break;
                }
                if !aliases.is_empty() && roots.insert(aliases.root_id()) {
                    if !self.state.step(aliases.len() * (2 + classes.len().max(1).ilog2() as usize)) {
                        break;
                    }
                    classes.insert(aliases.clone());
                }
            }
            if self.state.incomplete {
                return (target, next);
            }
            let mut remapped = std::collections::BTreeMap::new();
            for class in classes {
                let mut members = class.without(&replaced);
                for (index, aliases) in originals.iter().enumerate() {
                    if aliases == &class {
                        members.insert(block.params[index]);
                    }
                }
                remapped.insert(class, members);
            }
            next.remap_aliases(&remapped, self.state);
            for (index, slot) in saved.iter_mut().enumerate() {
                if let Some(members) = remapped.get(&originals[index]) {
                    slot.aliases = members.clone();
                }
                slot.contained = slot.contained.iter().flat_map(|label| {
                    remapped.get(&flow.aliases[label.as_u32() as usize]).into_iter().flatten().copied()
                }).collect();
                slot.token.pieces = slot.token.pieces.iter().cloned().map(|mut piece| {
                    piece.predicates = piece.predicates.into_iter().map(|(mut filter, selected)| {
                        let mut labels = SharedSet::new();
                        for label in &filter.labels {
                            let class = &flow.aliases[label.as_u32() as usize];
                            if let Some(members) = remapped.get(class) {
                                labels.extend(members);
                                filter.stale |= members.is_empty();
                            }
                        }
                        filter.labels = labels;
                        (filter, selected)
                    }).collect();
                    piece
                }).collect();
            }
            for (&param, slot) in block.params.iter().zip(saved) {
                self.define(&mut next, param, slot);
            }
            if !self.state.step(next.tokens.work()) {
                return (target, next);
            }
            let normalized: Vec<_> = next.tokens.indexed().map(|(index, token)| {
                (index, next.normalize_token(token.clone(), self.state))
            }).collect();
            for (index, token) in normalized {
                next.tokens.set(index, token);
            }
            for &param in &block.params {
                let expected = next.full_deps[param.as_u32() as usize].clone();
                self.check_deps(&mut next, param, &expected);
            }
        }
        (target, next)
    }
    fn discharge_must_use(&self, flow: &mut Flow, value: ValueId) {
        flow.must_use[value.as_u32() as usize] = 2;
        for alias in &flow.must_use_aliases[value.as_u32() as usize] {
            flow.must_use[alias.as_u32() as usize] = 2;
        }
    }
    fn finish(&mut self, flow: &mut Flow, normal: bool, values: &[ValueId]) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::work_profile::Operation::new("mod::finish");
        // A D21 async body checks its summary where it publishes (C-2); its `ret` carries no values.
        if !self.declaration.attrs.iter().any(|attr| matches!(attr, FunctionAttr::DeferEntry { .. }))
            && !self.frame_body() {
            self.body_summary(flow, normal, values);
        }
        if !flow.entry_accepted && self.module.profile != Profile::High {
            self.error(R::B54, "explicit input packet was never accepted into callee cleanup");
        }
        let exported: BTreeSet<_> = values.iter().flat_map(|v| {
            flow.tokens[v.as_u32() as usize].pieces.iter().map(|p| p.origin)
        }).collect();
        for &value in values {
            if super::later::value_type(self.module, self.body, value)
                == Some(&Type::Service(ServiceType::ExitState)) {
                self.error(R::B101, "ExitState must be eliminated by typed dispatch, not returned");
            }
            if flow.tokens[value.as_u32() as usize].unique.is_some() {
                self.error(R::B92, "unique-cache capability cannot escape its function/region");
            }
            if flow.refs[value.as_u32() as usize].iter().any(|origin| {
                matches!(origin, Origin::Instruction(..) | Origin::Invoke(..)
                    | Origin::RetiredInstruction(..) | Origin::RetiredInvoke(..)) && !exported.contains(origin)
            }) && self.module.profile != Profile::High {
                self.error(R::B32, "escaped value lacks ownership or a sufficient external lifetime boundary");
            }
            self.use_value(flow, value, OperandMode::Forward, R::B21);
            self.discharge_must_use(flow, value);
        }
        if !self.state.step(self.body.values.len()) {
            return;
        }
        for (value, _) in self.body.values.iter() {
            let i = value.as_u32() as usize;
            let core = linear(self.module, self.body, value);
            if core && flow.availability[i] & AVAILABLE != 0 {
                let rule = if scope(self.module, self.body, value).is_some() { R::B41 }
                    else if builder(self.module, self.body, value) { R::B81 }
                    else if self.body.values.get(value).ok().and_then(|v| self.module.types.get(v.ty).ok())
                        == Some(&Type::Service(ServiceType::Pending)) { R::B101 } else { R::B22 };
                self.error(rule, "function exit loses an available linear carrier");
            }
            if normal && flow.must_use[i] == 1 {
                self.error(R::B82, "normal exit leaves an unused successor");
            }
        }
        if flow.scopes.values().any(|status| *status == 1) {
            self.error(R::B41, "function exits with an open lexical scope");
        }
    }
}

fn targets(term: &TermKind) -> Vec<(BlockId, Vec<EdgeArg>)> {
    let target = |t: &Target| (t.block, t.args.iter().copied().map(EdgeArg::Value).collect());
    match term {
        TermKind::Br(t) => vec![target(t)],
        TermKind::Cond { yes, no, .. } => vec![target(yes), target(no)],
        TermKind::Switch { cases, default, .. } => cases.iter().map(|c| target(&c.target))
            .chain(std::iter::once(target(default))).collect(),
        TermKind::Catch { handler, next, .. } => vec![(handler.block, handler.args.clone()), target(next)],
        TermKind::Lookup { found, missing, .. } => vec![(found.block, found.args.clone()), target(missing)],
        _ => Vec::new(),
    }
}

fn witnesses(module: &Module, body: &Body) -> std::rc::Rc<Vec<bool>> {
    std::rc::Rc::new(body.values.iter().map(|(_, data)| {
        module.types.get(data.ty).is_ok_and(|ty| {
            !matches!(ty, Type::Service(_) | Type::Descriptor | Type::Frame(_)
                | Type::Builder(_) | Type::MapBuilder(..))
        })
    }).collect())
}

#[cfg(test)]
mod scheduling_tests {
    use super::*;

    #[test]
    fn delayed_alternative_does_not_replay_unchanged_block_inputs() {
        let module = text::parse(
            "oir 2 high
module schedule
requires core
scope S action in @f
func @f(%c: bool, %x: int, %y: int) -> void {
^entry in S:
    cond %c, ^join(%x), ^delay
^delay in S:
    br ^tail
^tail in S:
    br ^join(%y)
^join(%p: int) in S:
    br ^end(%p)
^end(%q: int) in S:
    ret
}",
        ).unwrap();
        let options = super::super::VerifyOptions::default();
        super::super::run_mode(
            &module, &options, super::super::VerifyContext::default(), None, None, false,
        ).unwrap();
        // Entry, delay and tail once; join and end once per distinct input.
        // The old block queue revisited the first join/end alternatives too.
        assert_eq!(LAST_SCHEDULE.get(), (7, 7));
    }
}
