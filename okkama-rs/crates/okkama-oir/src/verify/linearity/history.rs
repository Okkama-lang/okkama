//! A deliberately narrow quotient: isolated, unused scalar constant slots only.
//! All non-scalar slots and all global history are anchors. This is stronger
//! than ordinary liveness: future definitions also read the old incarnation.
use super::*;
use super::super::dominance::leaves;
use super::super::read::{Read, Reader};

pub(super) struct BoundaryHistory {
    future: Vec<Vec<u64>>,
    anchors: Vec<bool>,
    candidates: Vec<usize>,
}

struct ObservedValues {
    used: Vec<bool>,
    work: usize,
}
impl Reader for ObservedValues {
    fn sequence(&mut self, _: usize, _: usize) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("history::sequence");
        self.work = self.work.saturating_add(1);
    }
    fn value(&mut self, value: &ValueId) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("history::value");
        self.work = self.work.saturating_add(1);
        self.used[value.as_u32() as usize] = true;
    }
    fn dependency(&mut self, _: &DepExprId) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("history::dependency");
        self.work = self.work.saturating_add(1);
    }
    fn key(&mut self, _: &KeyExprId) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("history::key");
        self.work = self.work.saturating_add(1);
    }
    fn selection(&mut self, _: &SelectionId) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("history::selection");
        self.work = self.work.saturating_add(1);
    }
    fn ty(&mut self, _: &TypeId) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("history::ty");
        self.work = self.work.saturating_add(1);
    }
}

fn unused_constants(module: &Module, body: &Body, state: &mut State<'_>) -> Option<Vec<usize>> {
    let entities = body.values.len() + body.blocks.len() + body.insts.len()
        + body.deps.len() + body.keys.len() + body.selections.len()
        + body.cleanup.closes.len() + body.cleanup.schedules.len()
        + body.cleanup.plans.len() + body.cleanup.proofs.len();
    if !state.step(entities.saturating_mul(8)) || !state.array(body.values.len(), 17) {
        return None;
    }
    let mut observed = ObservedValues { used: vec![false; body.values.len()], work: 0 };
    // Traverse uses only. Reading Instruction/Block wholesale would count their
    // result/parameter definitions as uses. Visit every expression DAG node once
    // directly, rather than expanding shared DAG paths for every occurrence.
    for (_, block) in body.blocks.iter() {
        block.term.kind.read(&mut observed);
        block.unwind.read(&mut observed);
    }
    for (_, instruction) in body.insts.iter() {
        instruction.op.read(&mut observed);
        instruction.facts.read(&mut observed);
    }
    for (_, value) in body.values.iter() {
        value.facts.read(&mut observed);
    }
    body.deps.read(&mut observed);
    body.keys.read(&mut observed);
    body.selections.read(&mut observed);
    body.descriptor_bindings.read(&mut observed);
    body.service_schedules.read(&mut observed);
    body.cleanup.closes.read(&mut observed);
    body.cleanup.schedules.read(&mut observed);
    body.cleanup.plans.read(&mut observed);
    body.cleanup.proofs.read(&mut observed);
    if !state.step(observed.work) {
        return None;
    }
    let mut candidates = Vec::new();
    for (_, instruction) in body.insts.iter() {
        if !matches!(instruction.op, Operation::Const(_)) {
            continue;
        }
        for value in &instruction.results {
            let index = value.as_u32() as usize;
            if !observed.used[index] && body.values.get(*value).is_ok_and(|data| scalar_certificate(module, data)) {
                candidates.push(index);
            }
        }
    }
    Some(candidates)
}

fn scalar_certificate(module: &Module, data: &ValueData) -> bool {
    // Check the certificate here too: High does not run value_facts. In
    // particular !managed is insufficient for services, dictionaries or cells.
    matches!(data.facts, ValueFacts::Plain { certificate: PlainCertificate::Scalar })
        && module.types.get(data.ty).is_ok_and(|ty| matches!(ty,
            Type::Int | Type::Bool | Type::Byte | Type::Float | Type::Rat | Type::Char
            | Type::Str | Type::Range | Type::Enum(_) | Type::Never))
}

impl BoundaryHistory {
    pub fn new(module: &Module, body: &Body, state: &mut State<'_>) -> Option<Self> {
        if !state.step(body.values.len()) {
            return None;
        }
        if !body.values.iter().any(|(_, data)| scalar_certificate(module, data)) {
            return None;
        }
        let candidates = unused_constants(module, body, state)?;
        if candidates.is_empty() {
            return None;
        }
        let words = body.values.len().div_ceil(64);
        let blocks = body.blocks.len();
        let bytes = blocks.saturating_mul(words).saturating_mul(8)
            .saturating_add(blocks.saturating_mul(80))
            .saturating_add(body.values.len().saturating_mul(2));
        if !state.memory(bytes) {
            return None;
        }
        let mut anchors = vec![true; body.values.len()];
        for &index in &candidates {
            anchors[index] = false;
        }
        let mut anchor = |values: Vec<ValueId>| {
            for value in values {
                anchors[value.as_u32() as usize] = true;
            }
        };
        // Input summary anchors, dictionaries, schedules and all expression DAG
        // leaves are kept even outside their ordinary SSA live ranges. Root-map
        // bindings only add anchors: they never authorize dropping a value.
        anchor(leaves(&body.blocks.get(body.entry).ok()?.params, body, state));
        anchor(leaves(&body.deps, body, state));
        anchor(leaves(&body.keys, body, state));
        anchor(leaves(&body.selections, body, state));
        anchor(leaves(&body.descriptor_bindings, body, state));
        anchor(leaves(&body.service_schedules, body, state));
        anchor(leaves(&body.cleanup.closes, body, state));
        anchor(leaves(&body.cleanup.schedules, body, state));
        anchor(leaves(&body.cleanup.plans, body, state));
        anchor(leaves(&body.cleanup.proofs, body, state));
        anchor(leaves(&body.cleanup.roots, body, state));
        let mut future = vec![vec![0u64; words]; blocks];
        let mut predecessors = vec![Vec::new(); blocks];
        // graph::edges includes exceptional block edges, invoke unwind/cancel,
        // cleanup dispatch and every defer continuation, independently of roots.
        for edge in super::super::graph::edges(body, state) {
            if !state.step(1) || !state.memory(2 * size_of::<usize>()) {
                return None;
            }
            predecessors[edge.target.as_u32() as usize].push(edge.source.as_u32() as usize);
        }
        for (id, block) in body.blocks.iter() {
            let bits = &mut future[id.as_u32() as usize];
            let mut add = |values: Vec<ValueId>| {
                for value in values {
                    let index = value.as_u32() as usize;
                    bits[index / 64] |= 1 << (index % 64);
                }
            };
            // No definition kills: define reads availability, old aliases and
            // projections. Keeping reachable definitions covers loop resets.
            add(leaves(&block.params, body, state));
            add(leaves(&block.term.kind, body, state));
            add(leaves(&block.unwind, body, state));
            for instruction in &block.insts {
                add(leaves(body.insts.get(*instruction).ok()?, body, state));
            }
        }
        let mut queue: VecDeque<_> = (0..blocks).collect();
        let mut queued = vec![true; blocks];
        while let Some(target) = queue.pop_front() {
            queued[target] = false;
            for &source in &predecessors[target] {
                if !state.step(words.saturating_add(1)) {
                    return None;
                }
                if source == target {
                    continue;
                }
                let [source_bits, target_bits] = future.get_disjoint_mut([source, target]).unwrap();
                let mut changed = false;
                for (source, target) in source_bits.iter_mut().zip(target_bits.iter()) {
                    let next = *source | *target;
                    changed |= next != *source;
                    *source = next;
                }
                if changed && !queued[source] {
                    queued[source] = true;
                    queue.push_back(source);
                }
            }
        }
        (!state.incomplete).then_some(Self { future, anchors, candidates })
    }

    pub fn normalize(&self, block: BlockId, flow: &mut Flow, state: &mut State<'_>) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("history::normalize");
        // Start with a sparse shortlist. Most loop boundaries cannot erase any
        // old scalar: its definition is reachable again. Do not scan the entire
        // relational state at those boundaries just to discover this fact.
        let future = &self.future[block.as_u32() as usize];
        let mut candidates = Vec::new();
        for &index in &self.candidates {
            if !state.step(1) {
                return;
            }
            if self.anchors[index] || future[index / 64] & (1 << (index % 64)) != 0
                || flow.aliases[index].is_empty() {
                continue;
            }
            if !state.step(10) {
                return;
            }
            let neutral = flow.tokens[index] == Token::default() && flow.journals[index] == Token::default()
                && flow.refs[index].is_empty() && flow.full_deps[index].is_empty()
                && flow.identities[index].is_none() && flow.lifetimes[index] == 1
                && flow.must_use[index] == 0 && flow.must_use_aliases[index].is_empty()
                && flow.failures[index] == 0;
            if neutral {
                candidates.push(index);
            }
        }
        if candidates.is_empty() {
            return;
        }
        // The root vector and shortlist fit the solver's existing per-value
        // scratch reservation. Every slot outside the shortlist is an anchor.
        if !state.step(self.anchors.len()) {
            return;
        }
        let mut roots = Roots { keep: vec![true; self.anchors.len()] };
        for &index in &candidates {
            roots.keep[index] = false;
        }
        // Keep both endpoints of EVERY cross-slot label edge, even one stored
        // in dead or asymmetric history. The remaining candidates are isolated
        // singleton components, so closure under aliases/contained is immediate.
        // This avoids constructing an inverse graph or renaming any label.
        for (index, labels) in flow.aliases.indexed().chain(flow.contained.indexed())
            .chain(flow.must_use_aliases.indexed()) {
            if !state.step(1 + labels.len()) {
                return;
            }
            for label in labels {
                let other = label.as_u32() as usize;
                if other != index {
                    roots.keep[index] = true;
                    roots.keep[other] = true;
                }
            }
        }
        if candidates.iter().all(|index| roots.keep[*index]) {
            return;
        }
        if !state.step(flow.work()) {
            return;
        }
        // Non-neutral slots (including all potentially available linear,
        // must-use, scope/binding, input-summary, pending/cleanup/frame/cell/
        // cache/pin evidence) are anchors. Visit their observer references too.
        for token in flow.tokens.stored().chain(flow.journals.stored()) {
            roots.token(token);
        }
        for refs in flow.refs.stored().chain(flow.full_deps.stored()) {
            roots.origins(refs);
        }
        for identity in flow.identities.stored() {
            roots.optional(*identity);
        }
        // All global facts are retained. Their references are roots even when
        // their creation family is currently retired or absent from live SSA.
        roots.origins(&flow.dead);
        roots.origins(&flow.cache_keys);
        roots.origins(&flow.taken_caches);
        for origin in flow.provenance.keys() {
            roots.origin(*origin);
        }
        for piece in &flow.closed_inputs {
            roots.piece(piece);
        }
        for (origin, refs) in &flow.views {
            roots.origin(*origin);
            roots.origins(refs);
        }
        for ((origin, _, _), (refs, identity)) in &flow.fields {
            roots.origin(*origin);
            roots.origins(refs);
            roots.optional(*identity);
        }
        for (origin, layout) in &flow.frames {
            roots.origin(*origin);
            for slot in layout.values() {
                roots.slot(slot);
            }
        }
        for (origin, slot) in &flow.cells {
            roots.origin(*origin);
            roots.slot(slot);
        }
        roots.optional(flow.active_failure_origin);
        roots.optional(flow.latest_failure_origin);
        // normalize_token can discover witnesses through any nonempty refs.
        // Those slots were excluded from the neutral shortlist. All cross-slot
        // labels were rooted above, so global roots need no further label walk.
        // No future read/define can mention an erased slot. finish sees a plain
        // type and must_use=0; its other per-value metadata is already neutral.
        for index in candidates {
            if !roots.keep[index] {
                flow.availability[index] = ABSENT;
            }
        }
        flow.aliases.retain_indices(&roots.keep);
        flow.contained.retain_indices(&roots.keep);
    }
}

struct Roots {
    keep: Vec<bool>,
}
impl Roots {
    fn value(&mut self, value: ValueId) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("history::value");
        self.keep[value.as_u32() as usize] = true;
    }
    fn origins(&mut self, origins: &BTreeSet<Origin>) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("history::origins");
        for origin in origins {
            self.origin(*origin);
        }
    }
    fn optional(&mut self, origin: Option<Origin>) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("history::optional");
        if let Some(origin) = origin {
            self.origin(origin);
        }
    }
    fn origin(&mut self, origin: Origin) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("history::origin");
        match origin {
            Origin::Input(value) | Origin::External(value) => self.value(value),
            Origin::Projection(value, kind) | Origin::RetiredProjection(value, kind) => {
                self.value(value);
                match kind {
                    ProjectionKind::Index(value) | ProjectionKind::Map(value)
                    | ProjectionKind::MapIndex(value, _) | ProjectionKind::Remove(value) => self.value(value),
                    ProjectionKind::Slice(start, end) => {
                        self.value(start);
                        self.value(end);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    fn piece(&mut self, piece: &Piece) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("history::piece");
        self.origin(piece.origin);
        self.optional(piece.holder);
        for filter in piece.predicates.keys() {
            self.origins(&filter.keys);
            self.origins(&filter.possible);
            for value in &filter.labels {
                self.value(*value);
            }
        }
    }
    fn token(&mut self, token: &Token) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("history::token");
        for value in &token.sources {
            self.value(*value);
        }
        if let Some(DictionarySource::Parameter(value)) = token.dictionary {
            self.value(value);
        }
        for piece in &token.pieces {
            self.piece(piece);
        }
        for origin in [token.service, token.cell, token.unique, token.frame, token.cache,
            token.finish_origin, token.failure_origin] {
            self.optional(origin);
        }
        self.origins(&token.pin_deps);
        if let Some((origins, _)) = &token.snapshot {
            self.origins(origins);
        }
        if let Some(cleanup) = &token.cleanup {
            for piece in cleanup.reserve.iter().chain(&cleanup.inventory) {
                self.piece(piece);
            }
            for origin in [cleanup.failure_origin, cleanup.awaiting_origin, cleanup.selected] {
                self.optional(origin);
            }
            if let Some(value) = cleanup.awaiting_source {
                self.value(value);
            }
            self.origins(&cleanup.executed);
            self.origins(&cleanup.services);
        }
    }
    fn slot(&mut self, slot: &Slot) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("history::slot");
        self.token(&slot.token);
        self.token(&slot.journal);
        self.origins(&slot.refs);
        self.origins(&slot.requirements);
        self.optional(slot.identity);
        for value in slot.aliases.iter().chain(&slot.contained).chain(&slot.bases).chain(&slot.must_use_aliases) {
            self.value(*value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use super::super::super::{VerifyContext, VerifyOptions, VerifyStats};

    fn with_state(module: &Module, run: impl FnOnce(&mut State<'_>)) {
        let options = VerifyOptions::default();
        let mut state = State {
            module, options: &options, context: VerifyContext::default(),
            diagnostics: vec![], stats: VerifyStats::default(), incomplete: false,
            exhausted_limit: None, suppressed: 0, location: L::Module, source_loc: None,
            poisoned: BTreeSet::new(), positions: Default::default(),
            edge_positions: Default::default(), cleanup_origins: None,
        };
        run(&mut state);
        assert!(!state.incomplete);
    }

    /// #700: an obligation is the duty to finish an identity (OIR-2 §3.2). A builtin capability without destroy
    /// (Atomic) holds borrowed references and no obligation (§7.2; P2.4 §4.8), a role holds one only when an
    /// implementation can (§3.1), and a holder holds one only through such a field. Managedness, which also
    /// counts deps, is unchanged: the capability and every role stay managed.
    #[test]
    fn obligations_follow_destroy_role_implementations_and_fields() {
        let module = text::parse("oir 2 own
module classes
role Shape {
}
role Owned {
}
object Cap [builtin] {
}
object Res {
    destroy @destroy
    does Owned {}
}
object Square {
    field side: int
    does Shape {}
}
object CapHolder {
    field cap: Cap
}
object ShapeHolder {
    field shape: Shape
}
object OwnedHolder {
    field owned: Owned
}
scope S action in @destroy
func @destroy(%self: Res) -> void [destroy Res] {
^entry in S:
    ret
}
scope SU action in @uses
func @uses(%a: Square, %b: CapHolder, %c: ShapeHolder, %d: OwnedHolder) -> void {
^entry in SU:
    ret
}
").unwrap_or_else(|error| panic!("{error}"));
        with_state(&module, |state| {
            let module = state.module;
            let obligations = obligation_types(module, state);
            let managed = managed_types(module, state);
            let classified = |name: &str| {
                let (decl, _) = module.declarations.iter().find(|(_, declaration)| declaration.name == name).unwrap();
                let (ty, _) = module.types.iter().find(|(_, ty)| match ty {
                    Type::Object { decl: object, .. } | Type::Role { decl: object, .. } => *object == decl,
                    _ => false,
                }).unwrap();
                (obligations[ty.as_u32() as usize], managed[ty.as_u32() as usize])
            };
            assert_eq!(classified("Cap"), (false, true));
            assert_eq!(classified("Res"), (true, true));
            assert_eq!(classified("Square"), (false, false));
            assert_eq!(classified("Shape"), (false, true));
            assert_eq!(classified("Owned"), (true, true));
            assert_eq!(classified("CapHolder"), (false, true));
            assert_eq!(classified("ShapeHolder"), (false, true));
            assert_eq!(classified("OwnedHolder"), (true, true));
        });
    }

    /// #723: a function type holds an obligation only when one of the module's closures of it captures one, or a
    /// value of it may come from outside the module (an entry parameter); a holder holds one through such a field.
    /// Managedness is unchanged: every function type stays managed.
    #[test]
    fn obligations_of_function_types_follow_their_closures() {
        let module = text::parse("oir 2 own
module closures
object Res {
    destroy @destroy
}
object Plain {
    field n: int
}
object Kept {
    field f: fn() -> int
}
object Leased {
    field g: fn(int) -> int
}
object Foreign {
    field h: fn(bool) -> int
}
scope S action in @destroy
func @destroy(%self: Res) -> void [destroy Res] {
^entry in S:
    ret
}
scope SP action in @plain
func @plain(%captured: Plain) -> int [lambda captures 1] {
^entry in SP:
    %n: int = const 1
    ret %n
}
scope SR action in @leasing
func @leasing(%captured: Res, %x: int) -> int [lambda captures 1] {
^entry in SR:
    ret %x
}
scope SM action in @main
func @main(%h: fn(bool) -> int) -> int [entry] {
^entry in SM:
    %n: int = const 0
    ret %n
}
scope SU action in @uses
func @uses(%p: Plain, %r: Res, %a: Kept, %b: Leased, %c: Foreign) -> void {
^entry in SU:
    %kept: fn() -> int = fn.closure @plain, [%p], ty fn() -> int
    %leased: fn(int) -> int = fn.closure @leasing, [%r], ty fn(int) -> int
    ret
}
").unwrap_or_else(|error| panic!("{error}"));
        with_state(&module, |state| {
            let module = state.module;
            let obligations = obligation_types(module, state);
            let managed = managed_types(module, state);
            let classified = |name: &str| {
                let (decl, _) = module.declarations.iter().find(|(_, declaration)| declaration.name == name).unwrap();
                let (ty, _) = module.types.iter().find(|(_, ty)| matches!(ty, Type::Object { decl: object, .. }
                    if *object == decl)).unwrap();
                (obligations[ty.as_u32() as usize], managed[ty.as_u32() as usize])
            };
            assert_eq!(classified("Kept"), (false, true));
            assert_eq!(classified("Leased"), (true, true));
            assert_eq!(classified("Foreign"), (true, true));
        });
    }

    fn module() -> Module {
        text::parse("oir 2 own
module history
requires core
scope S action in @f
func @f() -> void {
^entry in S:
    %a: int = const 1
    %b: int = const 2
    %c: int = const 3
    %d: int = const 4
    br ^done
^done in S:
    ret
}").unwrap()
    }

    #[test]
    fn two_results_with_shared_stale_aliases_remap_containment_by_exact_union() {
        let mut module = module();
        let function = FuncId::from_u32(0);
        let value = ValueId::from_u32;
        let instruction = InstId::from_u32(1);
        let body = module.functions.get_mut(function).unwrap().body.as_mut().unwrap();
        body.insts.get_mut(instruction).unwrap().results = vec![value(0), value(2)];
        body.values.get_mut(value(0)).unwrap().def = Definition::InstResult { inst: instruction, index: 0 };
        body.values.get_mut(value(2)).unwrap().def = Definition::InstResult { inst: instruction, index: 1 };
        body.blocks.get_mut(body.entry).unwrap().insts = vec![instruction];
        with_state(&module, |state| {
            let module = state.module;
            let declaration = module.functions.get(function).unwrap();
            let body = declaration.body.as_ref().unwrap();
            let managed = managed_types(module, state);
            let obligations = obligation_types(module, state);
            let mut engine = Engine {
                module, function, declaration, body, state, emit: true,
                reported: BTreeSet::new(), managed: &managed, obligations: &obligations, flow_memory: 0,
                materialized_scopes: None, witnesses: witnesses(module, body),
                diagnose: true, collecting: false, capture_returns: false, contract_invalid: false,
                defer_stack: vec![function], defer_outputs: vec![], defer_cache: Default::default(),
            };
            let mut flow = Flow::new(4, witnesses(module, body));
            flow.availability[1] = AVAILABLE;
            flow.aliases[1].insert(value(1));
            flow.contained[1].insert(value(1));
            // Like operation(), capture both result slots before either define().
            let slot = Slot {
                aliases: flow.aliases[1].clone(), contained: flow.contained[1].clone(),
                ..Slot::default()
            };
            for result in &body.insts.get(instruction).unwrap().results {
                engine.define(&mut flow, *result, slot.clone());
            }
            assert_eq!(flow.aliases[0], SharedSet::from([value(0), value(1)]));
            assert_eq!(flow.aliases[1], SharedSet::from([value(1), value(2)]));
            // The first result's labels include the source, whose class was
            // changed by the second result defined from the captured slot.
            flow.contained[3] = flow.aliases[0].clone();
            let classes = BTreeMap::from([
                (flow.aliases[0].clone(), SharedSet::from([value(0)])),
                (flow.aliases[1].clone(), SharedSet::from([value(2)])),
            ]);
            let expected: SharedSet<_> = flow.contained[3].iter().flat_map(|label| {
                classes.get(&flow.aliases[label.as_u32() as usize]).into_iter().flatten().copied()
            }).collect();
            assert_eq!(expected, SharedSet::from([value(0), value(2)]));
            flow.remap_aliases(&classes, engine.state);
            assert_eq!(flow.contained[3], expected);
        });
    }

    #[test]
    fn absent_origin_retirement_still_materializes_projection_views() {
        let module = module();
        let body = module.functions.get(FuncId::from_u32(0)).unwrap().body.as_ref().unwrap();
        let base = ValueId::from_u32(1);
        let key = Origin::Projection(base, ProjectionKind::Keys);
        let mut flow = Flow::new(4, witnesses(&module, body));
        flow.refs[1].insert(Origin::External(base));
        flow.refs[2].insert(key);
        with_state(&module, |state| {
            flow.retire_origin(Origin::Instruction(InstId::from_u32(1000), 0), state);
        });
        assert_eq!(flow.views.get(&key), Some(&BTreeSet::from([Origin::External(base)])));
        assert_eq!(flow.refs[2], BTreeSet::from([key]));
    }

    #[test]
    fn isolated_dead_slots_disappear_but_observer_labels_are_not_renamed() {
        let module = module();
        let body = module.functions.get(FuncId::from_u32(0)).unwrap().body.as_ref().unwrap();
        let value = ValueId::from_u32;
        let mut flow = Flow::new(4, witnesses(&module, body));
        for index in 0..4 {
            flow.availability[index] = AVAILABLE;
            flow.aliases[index].insert(value(index as u32));
            flow.contained[index].insert(value(index as u32));
        }
        let mut piece = Piece::new(Origin::Input(value(3)), AdmissionProvenance::Bound);
        piece.predicates.insert(Filter {
            keys: BTreeSet::new(), fresh: false, possible: BTreeSet::new(),
            labels: SharedSet::from([value(2)]), stale: false, whole_future: false, state: 0,
        }, true);
        flow.closed_inputs.insert(piece);
        let globals = flow.closed_inputs.clone();
        with_state(&module, |state| {
            let history = BoundaryHistory::new(&module, body, state).unwrap();
            let mut asymmetric = flow.clone();
            asymmetric.aliases[1].insert(value(2));
            history.normalize(BlockId::from_u32(1), &mut asymmetric, state);
            assert_eq!(&asymmetric.availability[..2], &[ABSENT, AVAILABLE]);
            history.normalize(BlockId::from_u32(1), &mut flow, state);
            assert_eq!(&flow.availability[..2], &[ABSENT, ABSENT]);
            assert!(flow.aliases[0].is_empty());
            assert!(flow.aliases[1].is_empty());
            assert_eq!(flow.aliases[2], BTreeSet::from([value(2)]));
            assert_eq!(flow.closed_inputs, globals);
            let once = flow.clone();
            history.normalize(BlockId::from_u32(1), &mut flow, state);
            assert_eq!(flow, once);
        });
    }

    #[test]
    fn stale_projection_keys_and_repeated_creation_keep_the_old_incarnation() {
        let module = module();
        let body = module.functions.get(FuncId::from_u32(0)).unwrap().body.as_ref().unwrap();
        let index = ValueId::from_u32(0);
        let base = ValueId::from_u32(1);
        let borrowed = ValueId::from_u32(2);
        let created = Origin::Instruction(InstId::from_u32(1), 0);
        let projection = Origin::Projection(base, ProjectionKind::Index(index));
        let mut flow = Flow::new(4, witnesses(&module, body));
        flow.availability.fill(AVAILABLE);
        flow.identities[1] = Some(created);
        flow.refs[1].insert(created);
        flow.refs[2].insert(projection);
        flow.full_deps[2].insert(projection);
        flow.tokens[2].snapshot = Some((BTreeSet::from([created]), false));
        flow.aliases[0].insert(index);
        flow.contained[0].insert(index);
        flow.aliases[2].insert(borrowed);
        let mut precise = flow.clone();
        with_state(&module, |state| {
            let history = BoundaryHistory::new(&module, body, state).unwrap();
            history.normalize(BlockId::from_u32(1), &mut flow, state);
            assert_eq!(flow.availability[0], AVAILABLE);
            assert_eq!(flow.aliases[0], precise.aliases[0]);
            for branch in [&mut precise, &mut flow] {
                // These are the two history observers called by define and by
                // repeated execution of the same creation site, respectively.
                branch.retire_origin(projection, state);
                branch.retire_origin(created, state);
            }
            assert_eq!(flow.refs[2], precise.refs[2]);
            assert_eq!(flow.full_deps[2], precise.full_deps[2]);
            assert_eq!(flow.tokens[2], precise.tokens[2]);
            assert!(flow.refs[2].contains(&Origin::RetiredProjection(base, ProjectionKind::Index(index))));
            assert_eq!(flow.tokens[2].snapshot.as_ref().map(|snapshot| snapshot.1), Some(true));
        });
        let expected = borrow_rejection(&module, precise);
        assert_eq!(expected, vec![R::B31]);
        assert_eq!(borrow_rejection(&module, flow), expected);
    }

    fn borrow_rejection(module: &Module, mut flow: Flow) -> Vec<R> {
        let mut rules = Vec::new();
        with_state(module, |state| {
            let module = state.module;
            let function = FuncId::from_u32(0);
            let declaration = module.functions.get(function).unwrap();
            let body = declaration.body.as_ref().unwrap();
            let managed = managed_types(module, state);
            let obligations = obligation_types(module, state);
            let mut engine = Engine {
                module, function, declaration, body, state, emit: true,
                reported: BTreeSet::new(), managed: &managed, obligations: &obligations, flow_memory: 0,
                materialized_scopes: None, witnesses: witnesses(module, body),
                diagnose: true, collecting: false, capture_returns: false, contract_invalid: false,
                defer_stack: vec![function], defer_outputs: vec![], defer_cache: Default::default(),
            };
            let before = flow.clone();
            let retired = Origin::RetiredInstruction(InstId::from_u32(1), 0);
            engine.invalidate(&before, &mut flow, &[Piece::new(retired, AdmissionProvenance::Bound)]);
            engine.live_use(&flow, ValueId::from_u32(2));
            rules = engine.state.diagnostics.iter().map(|diagnostic| diagnostic.rule).collect();
        });
        rules
    }

    // #569 O1 audit: a retired opaque fresh family is inside a graph only when the
    // graph names the retired view and carries the result's own labels.
    #[test]
    fn retired_fresh_family_needs_the_result_labels() {
        let module = module();
        with_state(&module, |state| {
            let module = state.module;
            let function = FuncId::from_u32(0);
            let declaration = module.functions.get(function).unwrap();
            let body = declaration.body.as_ref().unwrap();
            let managed = managed_types(module, state);
            let obligations = obligation_types(module, state);
            let engine = Engine {
                module, function, declaration, body, state, emit: true,
                reported: BTreeSet::new(), managed: &managed, obligations: &obligations, flow_memory: 0,
                materialized_scopes: None, witnesses: witnesses(module, body),
                diagnose: true, collecting: false, capture_returns: false, contract_invalid: false,
                defer_stack: vec![function], defer_outputs: vec![], defer_cache: Default::default(),
            };
            let value = ValueId::from_u32;
            let labels = |values: &[u32]| -> SharedSet<ValueId> { values.iter().map(|v| value(*v)).collect() };
            let piece = |key: Origin, recorded: &[u32]| {
                let mut piece = Piece::new(Origin::RetiredInstruction(InstId::from_u32(1), 0),
                    AdmissionProvenance::UnboundResult);
                piece.family = true;
                piece.predicates.insert(Filter {
                    keys: BTreeSet::from([key]), fresh: false, possible: BTreeSet::new(),
                    labels: labels(recorded), stale: false, whole_future: false, state: 0,
                }, true);
                piece
            };
            let retired = Origin::RetiredViewInstruction(InstId::from_u32(1), 0);
            let graph = BTreeSet::from([retired, Origin::External(value(3))]);
            // The old result (label 1) and a graph that also contains it accept.
            assert!(engine.subset(&piece(retired, &[1]), &graph, &labels(&[1])));
            assert!(engine.subset(&piece(retired, &[1]), &graph, &labels(&[1, 2, 3])));
            // Another incarnation (label 2), or only the aliased inputs (label 3).
            assert!(!engine.subset(&piece(retired, &[1]), &graph, &labels(&[2])));
            assert!(!engine.subset(&piece(retired, &[1]), &graph, &labels(&[3])));
            // Labels lost on the way identify no incarnation at all.
            assert!(!engine.subset(&piece(retired, &[]), &graph, &labels(&[1, 2, 3])));
            assert!(!engine.subset(&piece(retired, &[1]), &BTreeSet::new(), &labels(&[1])));
            // A current key names exactly one incarnation: labels are irrelevant.
            let view = Origin::ViewInstruction(InstId::from_u32(1), 0);
            assert!(engine.subset(&piece(view, &[]), &BTreeSet::from([view]), &labels(&[])));
        });
    }

    #[test]
    fn boundary_future_includes_unwind_dag_leaves_and_redefinitions() {
        let mut module = module();
        let body = module.functions.get_mut(FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
        let entry = body.entry;
        let done = BlockId::from_u32(1);
        // A backedge re-executes all four scalar definitions. Ordinary SSA
        // liveness would kill them and lose define's projection-retirement read.
        body.blocks.get_mut(done).unwrap().term.kind = TermKind::Br(Target { block: entry, args: vec![] });
        with_state(&module, |state| {
            let body = module.functions.get(FuncId::from_u32(0)).unwrap().body.as_ref().unwrap();
            let history = BoundaryHistory::new(&module, body, state).unwrap();
            assert_eq!(history.future[done.as_u32() as usize][0] & 15, 15);
        });
        let body = module.functions.get_mut(FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
        let key = body.keys.alloc(KeyExpr::Runtime(ValueId::from_u32(3))).unwrap();
        body.deps.alloc(DepExpr::Key(key)).unwrap();
        body.blocks.get_mut(done).unwrap().term.kind = TermKind::Ret { values: vec![] };
        body.blocks.get_mut(done).unwrap().unwind = Some(BlockUnwind {
            target: Target { block: entry, args: vec![] },
        });
        with_state(&module, |state| {
            let body = module.functions.get(FuncId::from_u32(0)).unwrap().body.as_ref().unwrap();
            let history = BoundaryHistory::new(&module, body, state).unwrap();
            assert_eq!(history.future[done.as_u32() as usize][0] & 15, 15);
            assert!(history.anchors[3], "dependency/key DAG leaves are independent roots");
        });
    }
}
