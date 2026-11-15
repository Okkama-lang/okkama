use super::*;

pub(super) fn linear(module: &Module, body: &Body, value: ValueId) -> bool {
    body.values.get(value).ok().and_then(|v| module.types.get(v.ty).ok()).is_some_and(|ty| {
        matches!(ty, Type::Builder(_) | Type::MapBuilder(..) | Type::Frame(_))
            || matches!(ty, Type::Service(service) if service.is_linear())
    })
}
pub(super) fn builder(module: &Module, body: &Body, value: ValueId) -> bool {
    body.values.get(value).ok().and_then(|v| module.types.get(v.ty).ok())
        .is_some_and(|ty| matches!(ty, Type::Builder(_) | Type::MapBuilder(..)))
}
pub(super) fn scope(module: &Module, body: &Body, value: ValueId) -> Option<ScopeId> {
    match module.types.get(body.values.get(value).ok()?.ty).ok()? {
        Type::Service(ServiceType::Scope(scope)) => Some(*scope),
        _ => None,
    }
}
pub(super) fn successor(module: &Module, body: &Body, value: ValueId) -> bool {
    body.values.get(value).ok().and_then(|v| module.types.get(v.ty).ok())
        .is_some_and(|ty| matches!(ty, Type::Successor(_)))
}

// Ownership is a positive formula over formal type parameters. Substitution is
// iterative, so recursive nominal graphs do not recurse on untrusted input.
#[derive(Clone, Default)]
struct ManagedFormula {
    owned: bool,
    parameters: BTreeSet<(u8, u32, u32)>,
}
impl ManagedFormula {
    fn include(&mut self, other: &Self) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("metadata::include");
        self.owned |= other.owned;
        self.parameters.extend(&other.parameters);
    }
    fn grow(&mut self, other: Self, state: &mut State<'_>) -> bool {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("metadata::grow");
        let changed = other.owned && !self.owned
            || other.parameters.iter().any(|parameter| !self.parameters.contains(parameter));
        for parameter in other.parameters {
            if !self.parameters.contains(&parameter) && state.memory(32) {
                self.parameters.insert(parameter);
            }
        }
        self.owned |= other.owned;
        changed
    }
}
pub(super) fn managed_types(module: &Module, state: &mut State<'_>) -> Vec<bool> {
    classify_types(module, state, false)
}

/// Whether a value of each type may hold an obligation, the duty to finish an identity once (OIR-2 §3.2). This
/// is narrower than managedness, which also counts deps on borrowed identities: a builtin capability without
/// destroy holds borrowed references with their deps and no obligation of its own (Atomic, §7.2; P2.4 §4.8),
/// and a role holds one only when one of its implementations can (§3.1 counts every implementation of a role).
/// Closures, caps, `any` and Futures stay opaque.
pub(super) fn obligation_types(module: &Module, state: &mut State<'_>) -> Vec<bool> {
    classify_types(module, state, true)
}

fn classify_types(module: &Module, state: &mut State<'_>, obligations: bool) -> Vec<bool> {
    if !state.array(module.types.len() + module.declarations.len(), 40) {
        return Vec::new();
    }
    // The implementations of each role type, from the witness tables.
    let mut implementers = std::collections::BTreeMap::<TypeId, Vec<DeclId>>::new();
    if obligations {
        for (id, declaration) in module.declarations.iter() {
            if let DeclarationKind::Object { witnesses, .. } = &declaration.kind {
                for witness in witnesses {
                    if !state.step(1) || !state.memory(8) {
                        return Vec::new();
                    }
                    implementers.entry(witness.role).or_default().push(id);
                }
            }
        }
    }
    let opaque = ManagedFormula { owned: true, ..ManagedFormula::default() };
    let mut types = vec![ManagedFormula::default(); module.types.len()];
    let mut declarations = vec![ManagedFormula::default(); module.declarations.len()];
    let mut changed = true;
    while changed {
        changed = false;
        for (id, declaration) in module.declarations.iter() {
            if !state.step(1) {
                return Vec::new();
            }
            let mut next = ManagedFormula::default();
            match &declaration.kind {
                DeclarationKind::Object { fields, destroy, builtin, .. } => {
                    next.owned = destroy.is_some() || *builtin && !obligations;
                    for field in fields {
                        next.include(types.get(field.ty.as_u32() as usize).unwrap_or(&opaque));
                    }
                }
                DeclarationKind::Variant { cases, .. } => {
                    for case in cases {
                        next.include(declarations.get(case.layout.as_u32() as usize).unwrap_or(&opaque));
                    }
                }
                _ => {}
            }
            changed |= declarations[id.as_u32() as usize].grow(next, state);
        }
        for (id, ty) in module.types.iter() {
            if !state.step(1) {
                return Vec::new();
            }
            let mut next = ManagedFormula::default();
            match ty {
                Type::Object { decl, args } => {
                    let nominal = declarations.get(decl.as_u32() as usize).unwrap_or(&opaque);
                    next.owned = nominal.owned;
                    for &(owner, declaration, index) in &nominal.parameters {
                        if owner == 0 && declaration == decl.as_u32() {
                            if let Some(argument) = args.get(index as usize) {
                                next.include(types.get(argument.as_u32() as usize).unwrap_or(&opaque));
                            } else {
                                next.owned = true;
                            }
                        } else {
                            next.parameters.insert((owner, declaration, index));
                        }
                    }
                }
                Type::Variant(decl) => next.include(declarations.get(decl.as_u32() as usize).unwrap_or(&opaque)),
                Type::Successor(ty) | Type::Array(ty) | Type::Builder(ty) | Type::LowCell(ty) => {
                    next.include(types.get(ty.as_u32() as usize).unwrap_or(&opaque));
                }
                Type::Map(key, value) | Type::MapBuilder(key, value) => {
                    next.include(types.get(key.as_u32() as usize).unwrap_or(&opaque));
                    next.include(types.get(value.as_u32() as usize).unwrap_or(&opaque));
                }
                Type::Parameter(parameter) => {
                    let (kind, owner) = match parameter.owner {
                        GenericOwner::Declaration(id) => (0, id.as_u32()),
                        GenericOwner::Function(id) => (1, id.as_u32()),
                    };
                    next.parameters.insert((kind, owner, parameter.index));
                }
                Type::Role { decl, .. } if obligations => {
                    let builtin = matches!(module.declarations.get(*decl).map(|declaration| &declaration.kind),
                        Ok(DeclarationKind::Role { builtin: true, .. }) | Err(_));
                    next.owned = builtin;
                    for implementer in implementers.get(&id).into_iter().flatten() {
                        next.include(declarations.get(implementer.as_u32() as usize).unwrap_or(&opaque));
                    }
                }
                Type::Any | Type::Role { .. } | Type::Cap { .. } | Type::Fn(_) | Type::Future(_) => {
                    next.owned = true;
                }
                _ => {}
            }
            changed |= types[id.as_u32() as usize].grow(next, state);
        }
    }
    types.into_iter().map(|formula| formula.owned || !formula.parameters.is_empty()).collect()
}

pub(super) fn value_facts(
    module: &Module, function: FuncId, body: &Body, managed: &[bool], state: &mut State<'_>,
) {
    for (value, data) in body.values.iter() {
        if !state.step(1) {
            return;
        }
        state.at(L::Value { function, value }, None);
        let Ok(ty) = module.types.get(data.ty) else {
            continue;
        };
        if matches!(ty, Type::Service(_) | Type::Descriptor | Type::Frame(_)
            | Type::Builder(_) | Type::MapBuilder(..)) { continue; }
        let needs = managed.get(data.ty.as_u32() as usize).copied().unwrap_or(true);
        let valid = match &data.facts {
            ValueFacts::Managed { .. } => true,
            ValueFacts::Plain { certificate: PlainCertificate::Scalar } => matches!(ty,
                Type::Int | Type::Bool | Type::Byte | Type::Float | Type::Rat | Type::Char
                | Type::Str | Type::Range | Type::Enum(_) | Type::Never),
            ValueFacts::Plain { certificate: PlainCertificate::ClosedGraph { ty } } => *ty == data.ty && !needs,
            ValueFacts::Plain { certificate: PlainCertificate::Descriptor(dictionary) } => {
                let parameter = if let Type::Parameter(parameter) = ty {
                    (parameter.owner == GenericOwner::Function(function)).then_some(parameter.index)
                } else {
                    None
                };
                let valid = body.descriptor_bindings.iter().any(|binding| {
                    binding.dictionary == *dictionary && binding.contract.managed == DescriptorManaged::Plain
                        && !binding.contract.async_destroy
                        && matches!(binding.contract.retention, SummarySelection::Descriptor(index)
                            if Some(u32::from(index)) == parameter)
                        && module.summaries.get(binding.contract.summary).is_ok_and(|summary| {
                            !summary.contract.async_destroy && !summary.contract.suspends
                        })
                });
                if !valid { state.error(R::B111, "plain dictionary certificate lacks its exact checked contract"); }
                valid
            },
            ValueFacts::Unelaborated => false,
            ValueFacts::Service => false,
        };
        if !valid {
            state.error(R::B12, "managed value needs full dependencies or a checked plain certificate");
        }
    }
}

impl Engine<'_, '_> {
    pub(super) fn identity(&self, flow: &Flow, value: ValueId) -> Origin {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("metadata::identity");
        flow.identities[value.as_u32() as usize].unwrap_or(Origin::External(value))
    }
    pub(super) fn field(&self, flow: &Flow, value: ValueId, field: FieldRef) -> BTreeSet<Origin> {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("metadata::field");
        let identity = self.identity(flow, value);
        if let Some((refs, _)) = flow.fields.get(&(identity, field.declaration, field.index)) {
            return refs.clone();
        }
        if self.module.declarations.get(field.declaration).is_ok_and(|declaration| match &declaration.kind {
            DeclarationKind::Object { fields, .. } => fields.get(field.index as usize)
                .is_some_and(|field| !self.managed.get(field.ty.as_u32() as usize).copied().unwrap_or(true)),
            _ => false,
        }) { return BTreeSet::new(); }
        let root = match identity { Origin::External(value) => value, _ => value };
        BTreeSet::from([Origin::Projection(root, ProjectionKind::Field(field.declaration, field.index))])
    }
    pub(super) fn possible(&mut self, flow: &Flow, keys: &BTreeSet<Origin>) -> BTreeSet<Origin> {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("metadata::possible");
        let mut possible = BTreeSet::new();
        let mut pending: Vec<_> = keys.iter().copied().collect();
        while let Some(key) = pending.pop() {
            if !self.state.step(1) {
                break;
            }
            let refs = if let Some(refs) = flow.views.get(&key) { Some(refs) }
                else if let Origin::Projection(value, _) | Origin::RetiredProjection(value, _) = key {
                    Some(&flow.refs[value.as_u32() as usize])
                } else { None };
            if let Some(refs) = refs {
                for &dependency in refs {
                    if possible.insert(dependency) {
                        pending.push(dependency);
                    }
                }
            }
        }
        possible
    }
    fn may_alias(&mut self, flow: &Flow, a: Origin, b: Origin) -> bool {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("metadata::may_alias");
        if !self.state.step(1) {
            return true;
        }
        if a == b {
            return true;
        }
        let plain_external = |key| matches!(key, Origin::External(_));
        if plain_external(a) && plain_external(b) {
            return true;
        }
        let left = self.possible(flow, &BTreeSet::from([a]));
        let right = self.possible(flow, &BTreeSet::from([b]));
        if left.contains(&b) || right.contains(&a) || !left.is_disjoint(&right) {
            return true;
        }
        let external = |keys: &BTreeSet<Origin>| keys.iter().any(|key| plain_external(*key));
        (plain_external(a) && external(&right)) || (plain_external(b) && external(&left))
            || (external(&left) && external(&right))
    }
    pub(super) fn dependencies(&mut self, flow: &Flow, root: DepExprId) -> BTreeSet<Origin> {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("metadata::dependencies");
        let mut result = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            if !self.state.step(1) {
                break;
            }
            if !seen.insert(id) {
                continue;
            }
            match self.body.deps.get(id) {
                Ok(DepExpr::Empty) | Err(_) => {}
                Ok(DepExpr::Key(key)) => {
                    match self.body.keys.get(*key) {
                        Ok(KeyExpr::Created { instance, .. }) => {
                            self.live_use(flow, *instance);
                            // deps name the stable key of an obligation (OIR-2 §3.2): a creation that holds none
                            // (a value with deps only, see `obligation_types`) has no key to depend on.
                            let obligated = self.body.values.get(*instance).is_ok_and(|value| {
                                self.obligations.get(value.ty.as_u32() as usize).copied().unwrap_or(true)
                            });
                            if obligated || flow.identities[instance.as_u32() as usize].is_some() {
                                result.insert(self.identity(flow, *instance));
                            }
                        }
                        Ok(KeyExpr::Runtime(value) | KeyExpr::ServiceOf { value, .. }) => {
                            self.live_use(flow, *value);
                            result.extend(flow.refs[value.as_u32() as usize].iter().copied());
                            if flow.refs[value.as_u32() as usize].is_empty() {
                                result.insert(Origin::External(*value));
                            }
                        }
                        Err(_) => {}
                    }
                }
                Ok(DepExpr::Union(items)) => pending.extend(items),
                Ok(DepExpr::FromSsa(value)) => {
                    self.live_use(flow, *value);
                    if linear(self.module, self.body, *value) {
                        self.error(R::B31, "a packet register cannot anchor a lifetime");
                    }
                    result.extend(flow.refs[value.as_u32() as usize].iter().copied());
                }
                Ok(DepExpr::Retention { value, selection }) => {
                    self.live_use(flow, *value);
                    result.extend(flow.full_deps[value.as_u32() as usize].iter().copied());
                    let (selected, _) = self.selection(flow, *selection);
                    result.extend(selected);
                }
            }
        }
        result
    }
    pub(super) fn selection_terms(
        &mut self, flow: &Flow, root: SelectionId,
    ) -> Vec<(BTreeSet<Origin>, bool, SharedSet<ValueId>)> {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("metadata::selection_terms");
        let mut terms = Vec::new();
        let mut seen = BTreeSet::new();
        let mut pending = vec![(root, false)];
        while let Some((id, fresh)) = pending.pop() {
            if !self.state.step(1) {
                break;
            }
            if !seen.insert((id, fresh)) {
                continue;
            }
            match self.body.selections.get(id) {
                Ok(SelectionExpr::Admissible(inner)) => pending.push((*inner, true)),
                Ok(SelectionExpr::Union(items)) => pending.extend(items.iter().map(|id| (*id, fresh))),
                Ok(selection) => {
                    let (keys, _) = self.selection(flow, id);
                    let labels = match selection {
                        SelectionExpr::Value(value) | SelectionExpr::OwnerGraph(value)
                        | SelectionExpr::ViewOwner(value) | SelectionExpr::Descriptor { value, .. } => {
                            flow.contained[value.as_u32() as usize].clone()
                        }
                        _ => SharedSet::new(),
                    };
                    terms.push((keys, fresh, labels));
                }
                Err(_) => {}
            }
        }
        terms
    }
    pub(super) fn selection(&mut self, flow: &Flow, root: SelectionId) -> (BTreeSet<Origin>, bool) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("metadata::selection");
        let mut result = BTreeSet::new();
        let mut fresh = false;
        let mut seen = BTreeSet::new();
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            if !self.state.step(1) {
                break;
            }
            if !seen.insert(id) {
                continue;
            }
            let value = match self.body.selections.get(id) {
                Ok(SelectionExpr::Value(v) | SelectionExpr::OwnerGraph(v) | SelectionExpr::ViewOwner(v)) => *v,
                Ok(SelectionExpr::Descriptor { value, .. }) => *value,
                Ok(SelectionExpr::Projection(projection)) => {
                    let key = match projection {
                        Projection::Field { value, field } => {
                            result.extend(self.field(flow, *value, *field));
                            continue;
                        }
                        Projection::Index { value, index } => Origin::Projection(*value, ProjectionKind::Index(*index)),
                        Projection::Slice { value, start, end } => {
                            Origin::Projection(*value, ProjectionKind::Slice(*start, *end))
                        }
                        Projection::Map { value, keys } => Origin::Projection(*value, ProjectionKind::Map(*keys)),
                    };
                    result.insert(key);
                    continue;
                }
                Ok(SelectionExpr::Admissible(inner)) => {
                    fresh = true;
                    pending.push(*inner);
                    continue;
                }
                Ok(SelectionExpr::Union(items)) => {
                    pending.extend(items);
                    continue;
                }
                Err(_) => continue,
            };
            result.extend(flow.refs[value.as_u32() as usize].iter().copied());
        }
        (result, fresh)
    }
    pub(super) fn live_use(&mut self, flow: &Flow, value: ValueId) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("metadata::live_use");
        if flow.lifetimes[value.as_u32() as usize] != 1 {
            if flow.tokens[value.as_u32() as usize].cell.is_some() {
                self.error(R::B73, "Cell publication preserves the stored borrow's owner lifetime");
            }
            if !flow.full_deps[value.as_u32() as usize].is_disjoint(&flow.cache_keys) {
                self.error(R::B94, "cached result view outlives its cache owner without rehome");
            }
            self.error(R::B31, "use of a value whose stable dependency has been closed");
        }
    }
    pub(super) fn invalidate(&mut self, before: &Flow, flow: &mut Flow, pieces: &[Piece]) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("metadata::invalidate");
        for value in before.dependency_values() {
            if !self.scan_work(1) {
                return;
            }
            if before.lifetimes[value.as_u32() as usize] == 1 && self.invalidated(before, value, pieces) {
                flow.lifetimes[value.as_u32() as usize] = 2;
            }
        }
        for (key, layout) in &before.frames {
            for (index, slot) in layout {
                if self.keys_invalidated(before, &slot.requirements, &slot.aliases, pieces)
                    && let Some(saved) = flow.frames.get_mut(key).and_then(|layout| layout.get_mut(index)) {
                    saved.lifetime = 2;
                }
            }
        }
        for (key, slot) in &before.cells {
            if self.keys_invalidated(before, &slot.requirements, &slot.aliases, pieces)
                && let Some(stored) = flow.cells.get_mut(key) {
                stored.lifetime = 2;
            }
        }
    }
    fn invalidated(&mut self, flow: &Flow, value: ValueId, pieces: &[Piece]) -> bool {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("metadata::invalidated");
        self.keys_invalidated(flow, &flow.full_deps[value.as_u32() as usize],
            &flow.aliases[value.as_u32() as usize], pieces)
    }
    pub(super) fn keys_invalidated(
        &mut self, flow: &Flow, refs: &BTreeSet<Origin>, aliases: &SharedSet<ValueId>, pieces: &[Piece],
    ) -> bool {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("metadata::keys_invalidated");
        refs.iter().any(|key| {
            pieces.iter().any(|piece| {
                if !self.state.step(piece.predicates.len().saturating_add(1)) {
                    return true;
                }
                let retired = matches!(key, Origin::RetiredInstruction(..) | Origin::RetiredInvoke(..)
                    | Origin::RetiredProjection(..) | Origin::RetiredViewInstruction(..)
                    | Origin::RetiredViewInvoke(..));
                let excluded = piece.predicates.iter().any(|(filter, selected)| {
                    !*selected && !filter.fresh && filter.keys.contains(key)
                        && (!retired || (!filter.stale
                            && !filter.labels.is_disjoint(aliases)))
                });
                if excluded {
                    return false;
                }
                if !matches!(piece.origin, Origin::Input(_)) {
                    return self.may_alias(flow, piece.origin, *key);
                }
                piece.predicates.iter().all(|(filter, included)| {
                    !*included || filter.keys.iter().any(|other| self.may_alias(flow, *other, *key))
                })
            })
        })
    }
    pub(super) fn check_deps(&mut self, flow: &mut Flow, value: ValueId, expected: &BTreeSet<Origin>) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("metadata::check_deps");
        if let Ok(ValueData { facts: ValueFacts::Managed { deps }, .. }) = self.body.values.get(value) {
            let stated = self.dependencies(flow, *deps);
            if !expected.is_subset(&stated) {
                self.error(R::B32, "value.deps omits part of its retention graph");
            }
            flow.full_deps[value.as_u32() as usize].extend(stated);
        }
    }
}
