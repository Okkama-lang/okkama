use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Classification {
    Plain,
    Managed,
    Residual,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeClassification {
    pub class: Classification,
    pub certificate: Option<PlainCertificate>,
    pub reachable_resources: BTreeSet<TypeId>,
    pub destroy_entries: BTreeSet<FuncId>,
    pub service_types: BTreeSet<TypeId>,
    pub descriptors: Vec<DescriptorContract>,
    /// The function types that are parts of this graph and resolved by the closures this module creates
    /// (#723), with their dictionaries when the module binds one (#777). They do not make the type residual or
    /// owning, yet a scope that owns the type cleans their closures up through those dictionaries: own binds them
    /// in the owner's function, and the scope's envelope counts their cleanup effects.
    pub closures: BTreeMap<TypeId, Option<DescriptorContract>>,
    pub unresolved: BTreeSet<TypeId>,
    /// The value graph reaches an opaque reference (function, cap, role, any or
    /// future). The verifier requires full dependencies for such values even
    /// when every reachable target is plain.
    pub references: bool,
    /// The value graph reaches a builtin capability without destroy (Atomic, P2.4 section 4.8):
    /// a managed reference that elaboration partitions like an owned value, yet holds no obligation.
    pub capabilities: bool,
    /// A once let may keep a value of this type (Bible section 13.13, #491, #520). Computed only for the
    /// types of once fields; false elsewhere. See `once_graph`.
    pub once_plain: bool,
}

impl TypeClassification {
    pub fn may_own(&self) -> bool {
        !self.reachable_resources.is_empty() || !self.service_types.is_empty()
            || !self.unresolved.is_empty()
            || self.descriptors.iter().any(|descriptor| descriptor.managed != DescriptorManaged::Plain)
    }
}

// Terms pair an input type with bindings to already interned terms. In
// Box<Array<T>>, the argument's environment survives shadowing Box's T.
// No input type is added to the module, and recursive graphs share terms.
type ParameterKey = (u8, u32, u32);
type Environment = BTreeMap<ParameterKey, usize>;
fn parameter_key(parameter: TypeParameterRef) -> ParameterKey {
    let (kind, id) = match parameter.owner {
        GenericOwner::Declaration(id) => (0, id.as_u32()),
        GenericOwner::Function(id) => (1, id.as_u32()),
    };
    (kind, id, parameter.index)
}

fn free_parameters(module: &Module, budget: &mut Budget<'_>)
    -> Result<BTreeMap<TypeId, BTreeSet<ParameterKey>>, OwnError> {
    budget.memory(module.types.len() * 128)?;
    let mut free: BTreeMap<_, BTreeSet<_>> = module.types.iter().map(|(ty, _)| (ty, BTreeSet::new())).collect();
    loop {
        let mut changed = false;
        for (id, ty) in module.types.iter() {
            budget.work(1)?;
            let mut children = vec![];
            match ty {
                Type::Parameter(parameter) => {
                    changed |= free.get_mut(&id).unwrap().insert(parameter_key(*parameter));
                }
                Type::Object { args, .. } | Type::Role { args, .. } | Type::Cap { args, .. } => {
                    children.extend(args);
                }
                Type::Successor(ty) | Type::Array(ty) | Type::Builder(ty) | Type::LowCell(ty) => children.push(ty),
                Type::Map(k, v) | Type::MapBuilder(k, v) => children.extend([k, v]),
                Type::Fn(function) => {
                    children.extend(&function.params);
                    children.extend(&function.result);
                }
                Type::Future(result) => children.extend(result),
                _ => {}
            }
            let mut additions = BTreeSet::new();
            for child in children {
                additions.extend(free[child].iter().copied());
            }
            for parameter in additions {
                if free.get_mut(&id).unwrap().insert(parameter) {
                    budget.memory(64)?;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    Ok(free)
}

struct Terms<'a> {
    module: &'a Module,
    free: &'a BTreeMap<TypeId, BTreeSet<ParameterKey>>,
    ids: BTreeMap<(TypeId, Environment), usize>,
    nodes: Vec<(TypeId, Environment)>,
}
impl Terms<'_> {
    fn intern(&mut self, ty: TypeId, env: &Environment, budget: &mut Budget<'_>) -> Result<usize, OwnError> {
        budget.work(1)?;
        if let Type::Parameter(parameter) = self.module.types.get(ty).map_err(arena)?
            && let Some(term) = env.get(&parameter_key(*parameter)) {
            return Ok(*term);
        }
        let env: Environment = env.iter().filter(|(key, _)| self.free[&ty].contains(key))
            .map(|(key, term)| (*key, *term)).collect();
        let key = (ty, env);
        if let Some(id) = self.ids.get(&key) {
            return Ok(*id);
        }
        budget.memory(192 + key.1.len() * 128)?;
        let id = self.nodes.len();
        self.nodes.push(key.clone());
        self.ids.insert(key, id);
        Ok(id)
    }
}

pub(super) fn classify(
    module: &Module,
    options: &OwnOptions,
    budget: &mut Budget<'_>,
) -> Result<BTreeMap<TypeId, TypeClassification>, OwnError> {
    budget.memory(options.descriptors.len() * 512)?;
    let mut dictionaries = options.descriptors.clone();
    let mut captures: BTreeMap<TypeId, BTreeSet<TypeId>> = BTreeMap::new();
    let mut origins = BTreeSet::new();
    let mut open = BTreeSet::new();
    for (function_id, function) in module.functions.iter() {
        let Some(body) = &function.body else {
            continue;
        };
        for binding in &body.descriptor_bindings {
            let SummarySelection::Descriptor(index) = binding.contract.retention else { continue };
            for (ty, data) in module.types.iter() {
                budget.work(1)?;
                if !matches!(data, Type::Parameter(parameter)
                    if parameter.owner == GenericOwner::Function(function_id) && parameter.index == u32::from(index)) {
                    continue;
                }
                if dictionaries.get(&ty).is_some_and(|old| old != &binding.contract) {
                    return Err(budget.fail(OwnDiagnosticKind::InvalidDescriptor,
                        format!("conflicting bound dictionaries for {ty:?}")));
                }
                budget.memory(512)?;
                dictionaries.insert(ty, binding.contract.clone());
            }
        }
        for value in &body.blocks.get(body.entry).map_err(arena)?.params {
            let ty = body.values.get(*value).map_err(arena)?.ty;
            if matches!(module.types.get(ty), Ok(Type::Fn(_) | Type::Any)) {
                open.insert(ty);
            }
        }
        for (_, instruction) in body.insts.iter() {
            budget.work(1)?;
            let _handling = routing::handling(&instruction.op);
            match &instruction.op {
                Operation::Core(CoreOp::TypeOf { ty, descriptor }) => {
                    if dictionaries.get(ty).is_some_and(|old| old != descriptor) {
                        return Err(budget.fail(OwnDiagnosticKind::InvalidDescriptor, format!(
                            "conflicting dictionaries for {ty:?}",
                        )));
                    }
                    dictionaries.insert(*ty, descriptor.clone());
                }
                Operation::Core(CoreOp::FnClosure { captures: values, ty, .. }) => {
                    origins.insert(*ty);
                    for value in values {
                        captures.entry(*ty).or_default().insert(body.values.get(*value).map_err(arena)?.ty);
                    }
                }
                Operation::Core(CoreOp::FnBind { receiver, ty, .. }
                    | CoreOp::FnBindRole { receiver, ty, .. }) => {
                    origins.insert(*ty);
                    captures.entry(*ty).or_default().insert(body.values.get(*receiver).map_err(arena)?.ty);
                }
                Operation::Core(CoreOp::AnyBox { value, ty, .. }) => {
                    origins.insert(*ty);
                    captures.entry(*ty).or_default().insert(body.values.get(*value).map_err(arena)?.ty);
                }
                _ => {}
            }
        }
    }
    let free = free_parameters(module, budget)?;
    let foreign = foreign_functions(module, budget)?;
    let context = Context { module, dictionaries: &dictionaries, captures: &captures, origins: &origins, open: &open,
        free: &free, foreign: &foreign };
    let mut once_types = BTreeSet::new();
    for (_, declaration) in module.declarations.iter() {
        budget.work(1)?;
        if let DeclarationKind::Object { fields, .. } = &declaration.kind {
            once_types.extend(fields.iter().filter(|field| field.once).map(|field| field.ty));
        }
    }
    let mut result = BTreeMap::new();
    for (root, _) in module.types.iter() {
        let mut facts = graph(&context, root, false, budget)?.facts;
        facts.class = if !facts.descriptors.is_empty() || !facts.unresolved.is_empty() {
            Classification::Residual
        } else if facts.may_own() {
            Classification::Managed
        } else {
            Classification::Plain
        };
        if facts.class == Classification::Plain {
            facts.certificate = Some(PlainCertificate::ClosedGraph { ty: root });
        }
        if once_types.contains(&root) {
            facts.once_plain = once_graph(&context, root, budget)?;
        }
        result.insert(root, facts);
    }
    Ok(result)
}

/// What classification reads from the module's function bodies.
struct Context<'a> {
    module: &'a Module,
    dictionaries: &'a BTreeMap<TypeId, DescriptorContract>,
    captures: &'a BTreeMap<TypeId, BTreeSet<TypeId>>,
    origins: &'a BTreeSet<TypeId>,
    open: &'a BTreeSet<TypeId>,
    free: &'a BTreeMap<TypeId, BTreeSet<ParameterKey>>,
    /// Function types a value from outside the module may have (#723): reached from a bodyless function's
    /// results or the entry's parameters. The verifier's obligation formula counts the same set.
    foreign: &'a BTreeSet<TypeId>,
}

/// The function types a value from outside the module may have: those the results of bodyless functions and the
/// entry's parameters reach (#723, as the verifier's `closures`). Each function and each visited type is one unit of
/// work.
fn foreign_functions(module: &Module, budget: &mut Budget<'_>) -> Result<BTreeSet<TypeId>, OwnError> {
    let mut pending = vec![];
    for (_, function) in module.functions.iter() {
        budget.work(1)?;
        if function.body.is_none() {
            pending.extend(function.signature.results.iter().copied());
        }
        if function.attrs.contains(&FunctionAttr::Entry) {
            pending.extend(function.signature.params.iter().copied());
        }
    }
    let mut seen = BTreeSet::new();
    let mut foreign = BTreeSet::new();
    while let Some(ty) = pending.pop() {
        budget.work(1)?;
        if !seen.insert(ty) {
            continue;
        }
        budget.memory(64)?;
        let fields = |decl: &DeclId, pending: &mut Vec<TypeId>| -> Result<(), OwnError> {
            match &module.declarations.get(*decl).map_err(arena)?.kind {
                DeclarationKind::Object { fields, .. } => pending.extend(fields.iter().map(|field| field.ty)),
                DeclarationKind::Variant { cases, .. } => {
                    for case in cases {
                        if let DeclarationKind::Object { fields, .. } =
                            &module.declarations.get(case.layout).map_err(arena)?.kind {
                            pending.extend(fields.iter().map(|field| field.ty));
                        }
                    }
                }
                DeclarationKind::Cap { owner, .. } => pending.push(*owner),
                _ => {}
            }
            Ok(())
        };
        match module.types.get(ty).map_err(arena)? {
            Type::Fn(function) => {
                foreign.insert(ty);
                pending.extend(&function.params);
                pending.extend(&function.result);
            }
            Type::Object { decl, args } | Type::Role { decl, args } | Type::Cap { decl, args } => {
                pending.extend(args);
                fields(decl, &mut pending)?;
            }
            Type::Variant(decl) => fields(decl, &mut pending)?,
            Type::Successor(child) | Type::Array(child) | Type::Builder(child) | Type::LowCell(child) => {
                pending.push(*child);
            }
            Type::Map(key, value) | Type::MapBuilder(key, value) => pending.extend([*key, *value]),
            Type::Future(result) => pending.extend(result),
            _ => {}
        }
    }
    Ok(foreign)
}

/// The reachable graph of one type: its facts, whether it reaches a cap, `any` or Future, and the function
/// and `any` types in it.
struct Graph {
    facts: TypeClassification,
    opaque: bool,
    functions: BTreeSet<TypeId>,
}

/// Bible section 13.13 (#491, #520): a once value must not retain a resource; a closure or role value is
/// allowed only when it cannot retain one in this program. The graph of a once type reaches no resource,
/// service, builtin capability, cap, `any`, Future or unbound parameter. A function type in it counts by the
/// closures this module creates, whose captures the graph includes: a function parameter leaves a callee's
/// own analysis open, yet every closure that reaches it is one of those, and a function type without
/// closures has no value. A role counts by its implementations.
fn once_graph(context: &Context<'_>, root: TypeId, budget: &mut Budget<'_>) -> Result<bool, OwnError> {
    let Graph { facts, opaque, functions } = graph(context, root, true, budget)?;
    Ok(!opaque && !facts.capabilities && facts.reachable_resources.is_empty() && facts.service_types.is_empty()
        && facts.descriptors.is_empty() && facts.unresolved.is_subset(&functions))
}

fn graph(context: &Context<'_>, root: TypeId, once: bool, budget: &mut Budget<'_>) -> Result<Graph, OwnError> {
    let module = context.module;
    let mut facts = TypeClassification {
        class: Classification::Plain,
        certificate: None,
        reachable_resources: BTreeSet::new(),
        destroy_entries: BTreeSet::new(),
        service_types: BTreeSet::new(),
        descriptors: vec![],
        closures: BTreeMap::new(),
        unresolved: BTreeSet::new(),
        references: false,
        capabilities: false,
        once_plain: false,
    };
    let mut terms = Terms { module, free: context.free, ids: BTreeMap::new(), nodes: vec![] };
    let root_term = terms.intern(root, &Environment::new(), budget)?;
    let mut opaque = false;
    let mut functions = BTreeSet::new();
    let mut pending = vec![root_term];
    let mut visited = BTreeSet::new();
    while let Some(term) = pending.pop() {
        budget.work(1)?;
        if !visited.insert(term) {
            continue;
        }
        let (ty, mut env) = terms.nodes[term].clone();
        // A function type's dictionary is the cleanup contract of its open calls. The once graph resolves the
        // type by the closures this module creates instead (`once_graph`), and so does every graph in which the
        // function type is a part, not the root (#723): a holder of action values owns what their closures may
        // own, as a holder of role values owns what the role's implementations may own (P2.4 §4.1: a closure
        // owns a captured resource only when it captured a fresh one). The function type itself keeps its
        // dictionary for its own calls and cleanup.
        let function_type = matches!(module.types.get(ty).map_err(arena)?, Type::Fn(_));
        let by_closures = function_type && (once || term != root_term && !context.foreign.contains(&ty));
        if by_closures && !once {
            budget.memory(512)?;
            facts.closures.insert(ty, context.dictionaries.get(&ty).cloned());
        }
        if let Some(descriptor) = context.dictionaries.get(&ty).filter(|_| !by_closures) {
            module.functions.get(descriptor.cleanup).map_err(arena)?;
            module.summaries.get(descriptor.summary).map_err(arena)?;
            if !facts.descriptors.contains(descriptor) {
                facts.descriptors.push(descriptor.clone());
            }
            if descriptor.managed != DescriptorManaged::Plain {
                facts.reachable_resources.insert(ty);
                facts.destroy_entries.insert(descriptor.cleanup);
            }
            // A residual dictionary covers traversal, settlement and all
            // permitted substitutions. Visible concrete fields are still
            // traversed, so a dictionary cannot hide a known resource.
            if matches!(module.types.get(ty).map_err(arena)?, Type::Parameter(_) | Type::Any | Type::Fn(_)) {
                continue;
            }
        }
        let mut children = vec![];
        facts.references |= matches!(module.types.get(ty).map_err(arena)?,
            Type::Fn(_) | Type::Cap { .. } | Type::Role { .. } | Type::Any | Type::Future(_));
        opaque |= matches!(module.types.get(ty).map_err(arena)?, Type::Cap { .. } | Type::Any | Type::Future(_));
        match module.types.get(ty).map_err(arena)? {
            Type::Successor(child) | Type::Array(child) | Type::Builder(child) | Type::LowCell(child) => {
                children.push(*child);
            }
            Type::Map(key, value) | Type::MapBuilder(key, value) => {
                children.extend([*key, *value]);
            }
            Type::Object { decl, args } | Type::Role { decl, args } | Type::Cap { decl, args } => {
                let mut actuals = vec![];
                for ty in args {
                    actuals.push(terms.intern(*ty, &env, budget)?);
                }
                for (index, actual) in actuals.into_iter().enumerate() {
                    env.insert((0, decl.as_u32(), index as u32), actual);
                }
                declaration_edges(module, ty, *decl, &mut facts, &mut children, budget)?;
                if let Type::Role { .. } = module.types.get(ty).map_err(arena)? {
                    // Every enabled witness target participates, including
                    // targets visible only in a module's test profile.
                    for (candidate, candidate_type) in module.types.iter() {
                        budget.work(1)?;
                        let Type::Object { decl: object, .. } = candidate_type else {
                            continue;
                        };
                        let DeclarationKind::Object { witnesses, .. } =
                            &module.declarations.get(*object).map_err(arena)?.kind else {
                            continue;
                        };
                        if witnesses.iter().any(|witness| matches!(module.types.get(witness.role),
                            Ok(Type::Role { decl: role, .. }) if role == decl)) {
                            children.push(candidate);
                        }
                    }
                }
            }
            Type::Variant(decl) => {
                declaration_edges(module, ty, *decl, &mut facts, &mut children, budget)?;
            }
            Type::Fn(_) | Type::Any => {
                children.extend(context.captures.get(&ty).into_iter().flatten().copied());
                // A part resolved by its closures (#723): every value of the type is one of the closures this
                // module creates, whose captures are its children; a type without closures has no value.
                let closed = by_closures && !once;
                if !closed && (!context.origins.contains(&ty) || context.open.contains(&ty)) {
                    facts.unresolved.insert(ty);
                }
                functions.insert(ty);
            }
            Type::Parameter(_) => {
                facts.unresolved.insert(ty);
            }
            Type::Future(result) => {
                facts.service_types.insert(ty);
                children.extend(result);
            }
            Type::Frame(function) => {
                facts.service_types.insert(ty);
                let function = module.functions.get(*function).map_err(arena)?;
                children.extend(&function.signature.params);
                if let Some(body) = &function.body {
                    children.extend(body.values.iter().map(|(_, value)| value.ty));
                }
            }
            Type::Service(service) if service.is_linear() => {
                facts.service_types.insert(ty);
            }
            Type::Int | Type::Bool | Type::Byte | Type::Float | Type::Rat | Type::Char | Type::Str
            | Type::Range | Type::Never | Type::Enum(_) | Type::Handle(_) | Type::Descriptor
            | Type::Service(_) => {}
        }
        budget.memory(children.len() * (64 + env.len() * 64))?;
        for child in children.into_iter().rev() {
            pending.push(terms.intern(child, &env, budget)?);
        }
    }
    Ok(Graph { facts, opaque, functions })
}

fn declaration_edges(
    module: &Module,
    ty: TypeId,
    decl: DeclId,
    facts: &mut TypeClassification,
    children: &mut Vec<TypeId>,
    budget: &mut Budget<'_>,
) -> Result<(), OwnError> {
    budget.work(1)?;
    let declaration = module.declarations.get(decl).map_err(arena)?;
    match &declaration.kind {
        DeclarationKind::Object { fields, destroy, builtin, .. } => {
            children.extend(fields.iter().map(|field| field.ty));
            // A D23 holder (#810: a FutureGroup) holds its members' service duties: a service type of its own.
            if *builtin && crate::catalogue::service_holder(&declaration.name) {
                facts.service_types.insert(ty);
                // Its runtime word refers to the members' Futures, whose outcomes its settle may release.
                facts.references = true;
                children.extend(crate::catalogue::holder_members(module, decl));
            // A builtin capability without destroy (Atomic, P2.4 section 4.8) holds no obligation of
            // its own: close is an explicit operation, a read returns a reference with deps.
            } else if *builtin && destroy.is_none() {
                facts.references = true;
                facts.capabilities = true;
            } else if *builtin {
                facts.service_types.insert(ty);
            }
            if let Some(destroy) = destroy {
                facts.reachable_resources.insert(ty);
                facts.destroy_entries.insert(*destroy);
            }
        }
        DeclarationKind::Variant { cases, .. } => {
            for case in cases {
                if let DeclarationKind::Object { fields, destroy, .. } =
                    &module.declarations.get(case.layout).map_err(arena)?.kind {
                    children.extend(fields.iter().map(|field| field.ty));
                    if let Some(destroy) = destroy {
                        facts.reachable_resources.insert(ty);
                        facts.destroy_entries.insert(*destroy);
                    }
                }
            }
        }
        DeclarationKind::Role { builtin: true, .. } => {
            facts.service_types.insert(ty);
        }
        DeclarationKind::Cap { owner, .. } => children.push(*owner),
        _ => {}
    }
    Ok(())
}
