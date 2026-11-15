use super::State;
use crate::*;
use std::collections::{BTreeMap, BTreeSet};

type FnShape = (bool, usize, bool);

#[derive(Default)]
pub(super) struct OriginIndex {
    values: BTreeMap<TypeId, Vec<(FuncId, TypeId)>>,
    dictionaries: BTreeMap<(FuncId, TypeId), Option<InstId>>,
    parameters: BTreeSet<TypeId>,
    declaration_parameters: BTreeSet<TypeId>,
    generic_functions: BTreeSet<FnShape>,
    external_results: BTreeSet<TypeId>,
}

impl OriginIndex {
    fn build(module: &Module, state: &mut State<'_>) -> Self {
        let mut index = Self::default();
        let mut parents = BTreeMap::<TypeId, Vec<TypeId>>::new();
        let mut pending = Vec::new();
        for (id, ty) in module.types.iter() {
            if !state.step(1) || !state.memory(64) {
                return index;
            }
            if let Type::Parameter(parameter) = ty {
                pending.push((id, matches!(parameter.owner, GenericOwner::Declaration(_))));
            }
            for child in super::types::children(ty) {
                if !state.step(1) || !state.memory(32) {
                    return index;
                }
                parents.entry(child).or_default().push(id);
            }
        }
        // Propagate both flags through the structural type graph once, without
        // recursion or assumptions about arena order.
        while let Some((ty, declaration)) = pending.pop() {
            if !state.step(1) || !state.memory(32) {
                return index;
            }
            let changed = if declaration {
                index.parameters.insert(ty);
                index.declaration_parameters.insert(ty)
            } else {
                index.parameters.insert(ty)
            };
            if changed && let Some(parents) = parents.get(&ty) {
                if !state.array(parents.len(), 16) {
                    return index;
                }
                pending.extend(parents.iter().map(|parent| (*parent, declaration)));
            }
        }
        let mut external = Vec::new();
        for (function, data) in module.functions.iter() {
            if !state.step(1) {
                return index;
            }
            let Some(body) = &data.body else {
                if !state.array(data.signature.results.len(), 16) {
                    return index;
                }
                external.extend(data.signature.results.iter().copied());
                continue;
            };
            for (instruction, data) in body.insts.iter() {
                if !index.operation(module, body, function, &data.op, state) {
                    return index;
                }
                if let Operation::Core(CoreOp::TypeOf { ty, descriptor }) = &data.op {
                    if !state.memory(64) {
                        return index;
                    }
                    let valid = descriptor.managed != DescriptorManaged::Plain
                        && descriptor.retention == SummarySelection::OwnerGraph(0)
                        && bound_opaque_dictionary(module, descriptor, *ty, state);
                    let selected = valid.then_some(instruction);
                    let old = index.dictionaries.entry((function, *ty)).or_insert(selected);
                    let same = old.and_then(|id| body.insts.get(id).ok()).is_some_and(|instruction| {
                        matches!(&instruction.op, Operation::Core(CoreOp::TypeOf { descriptor: previous, .. })
                            if previous == descriptor)
                    });
                    if !valid || !same {
                        *old = None;
                    }
                }
            }
            // Allocating origins may be checked operations at terminators.
            for (_, block) in body.blocks.iter() {
                if !state.step(1) {
                    return index;
                }
                if let TermKind::InvokeOwn { op, .. } = &block.term.kind
                    && !index.operation(module, body, function, op, state) {
                    return index;
                }
            }
        }
        while let Some(ty) = external.pop() {
            if !state.step(1) || !state.memory(32) {
                return index;
            }
            if !index.external_results.insert(ty) {
                continue;
            }
            let Ok(ty) = module.types.get(ty) else {
                continue;
            };
            for child in super::types::children(ty) {
                if !state.step(1) || !state.memory(16) {
                    return index;
                }
                external.push(child);
            }
            let declaration = match ty {
                Type::Object { decl, .. } | Type::Cap { decl, .. } | Type::Variant(decl) => Some(*decl),
                _ => None,
            };
            if let Some(declaration) = declaration.and_then(|id| module.declarations.get(id).ok()) {
                match &declaration.kind {
                    DeclarationKind::Object { fields, .. } => {
                        if !state.array(fields.len(), 16) {
                            return index;
                        }
                        external.extend(fields.iter().map(|field| field.ty));
                    }
                    DeclarationKind::Cap { owner, .. } => external.push(*owner),
                    DeclarationKind::Variant { cases, .. } => {
                        for case in cases {
                            if !state.step(1) {
                                return index;
                            }
                            if let Ok(Declaration { kind: DeclarationKind::Object { fields, .. }, .. }) =
                                module.declarations.get(case.layout) {
                                if !state.array(fields.len(), 16) {
                                    return index;
                                }
                                external.extend(fields.iter().map(|field| field.ty));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        index
    }

    fn operation(
        &mut self, module: &Module, body: &Body, function: FuncId, op: &Operation, state: &mut State<'_>,
    ) -> bool {
        if !state.step(1) {
            return false;
        }
        let (ty, inputs) = match op {
            Operation::Core(CoreOp::FnClosure { ty, captures, .. }) => (*ty, captures.as_slice()),
            Operation::Core(CoreOp::FnBind { ty, receiver, .. } | CoreOp::FnBindRole { ty, receiver, .. }) => {
                (*ty, std::slice::from_ref(receiver))
            }
            Operation::Core(CoreOp::AnyBox { ty, value, .. }) => (*ty, std::slice::from_ref(value)),
            _ => return true,
        };
        if !state.memory(64) || !state.array(inputs.len(), 32) {
            return false;
        }
        if self.parameters.contains(&ty) && let Ok(Type::Fn(signature)) = module.types.get(ty) {
            self.generic_functions.insert(fn_shape(signature));
        }
        let values = self.values.entry(ty).or_default();
        for input in inputs {
            if !state.step(1) {
                return false;
            }
            if let Ok(value) = body.values.get(*input) {
                values.push((function, value.ty));
            }
        }
        true
    }
}

fn fn_shape(signature: &FnType) -> FnShape {
    (signature.is_async, signature.params.len(), signature.result.is_some())
}

fn current_function(state: &State<'_>) -> Option<FuncId> {
    match state.location {
        super::EntityLocation::Instruction { function, .. }
        | super::EntityLocation::Terminator { function, .. }
        | super::EntityLocation::Value { function, .. } => Some(function),
        _ => None,
    }
}

pub(super) fn cleanup(module: &Module, body: &Body, ty: TypeId, state: &mut State<'_>) -> Effects {
    if state.cleanup_origins.is_none() {
        state.cleanup_origins = Some(OriginIndex::build(module, state));
    }
    let mut result = Effects::default();
    let mut pending = vec![ty];
    let mut seen = BTreeSet::new();
    let mut bindings = Vec::<(TypeParameterRef, TypeId)>::new();
    let mut used = Vec::<TypeParameterRef>::new();
    while let Some(ty) = pending.pop() {
        if !state.step(1) {
            return result;
        }
        if !seen.insert(ty) {
            continue;
        }
        match module.types.get(ty) {
            Ok(Type::Object { decl, args }) => {
                for (index, argument) in args.iter().enumerate() {
                    let parameter = TypeParameterRef { owner: GenericOwner::Declaration(*decl), index: index as u32 };
                    if !bindings.contains(&(parameter, *argument)) {
                        bindings.push((parameter, *argument));
                        if used.contains(&parameter) {
                            pending.push(*argument);
                        }
                    }
                }
                if let Ok(Declaration { kind: DeclarationKind::Object { fields, destroy, .. }, .. }) =
                    module.declarations.get(*decl) {
                    pending.extend(fields.iter().map(|field| field.ty));
                    if let Some(destroy) = destroy {
                        add_function(module, *destroy, &mut result);
                    }
                }
            }
            Ok(Type::Variant(decl)) => {
                if let Ok(Declaration { kind: DeclarationKind::Variant { cases, .. }, .. }) =
                    module.declarations.get(*decl) {
                    for case in cases {
                        if let Ok(Declaration { kind: DeclarationKind::Object { fields, .. }, .. }) =
                            module.declarations.get(case.layout) {
                            pending.extend(fields.iter().map(|field| field.ty));
                        }
                    }
                }
            }
            Ok(Type::Array(item) | Type::Builder(item) | Type::Successor(item) | Type::LowCell(item)) => {
                pending.push(*item);
            }
            Ok(Type::Map(key, value) | Type::MapBuilder(key, value)) => pending.extend([*key, *value]),
            Ok(Type::Parameter(parameter)) => {
                used.push(*parameter);
                let actual: Vec<_> = bindings.iter().filter(|(formal, argument)| {
                    formal == parameter && module.types.get(*argument) != Ok(&Type::Parameter(*parameter))
                }).map(|(_, argument)| *argument).collect();
                if !actual.is_empty() {
                    pending.extend(actual);
                    continue;
                }
                let owner = current_function(state).map(GenericOwner::Function);
                let dictionary = body.descriptor_bindings.iter().find(|binding| {
                    Some(parameter.owner) == owner &&
                    matches!(binding.contract.retention, SummarySelection::Descriptor(index)
                        if u32::from(index) == parameter.index)
                });
                if let Some(binding) = dictionary {
                    result.suspends |= binding.contract.async_destroy;
                    add_function(module, binding.contract.cleanup, &mut result);
                } else { unknown(&mut result); }
            }
            // OIR-2 §11: a cap retains the complete owner graph, including
            // hidden resources. Its public computed-field result is irrelevant.
            Ok(Type::Cap { decl, args }) => {
                for (index, argument) in args.iter().enumerate() {
                    let parameter = TypeParameterRef { owner: GenericOwner::Declaration(*decl), index: index as u32 };
                    if !bindings.contains(&(parameter, *argument)) {
                        bindings.push((parameter, *argument));
                        if used.contains(&parameter) {
                            pending.push(*argument);
                        }
                    }
                }
                if let Ok(Declaration { kind: DeclarationKind::Cap { owner, .. }, .. }) =
                    module.declarations.get(*decl) {
                    pending.push(*owner);
                } else {
                    unknown(&mut result);
                }
            }
            // OIR-2 §12 and P1.3 §3.1/D16: a concrete, full-owner dictionary
            // binds opaque cleanup. Partial selection and an unbound cleanup
            // declaration cannot establish synchronous cleanup.
            Ok(Type::Any | Type::Fn(_)) => {
                let index = state.cleanup_origins.as_ref().unwrap();
                let generic = index.parameters.contains(&ty);
                let generic_origin = match module.types.get(ty) {
                    Ok(Type::Fn(signature)) => index.generic_functions.contains(&fn_shape(signature)),
                    _ => false,
                };
                if generic || generic_origin || index.external_results.contains(&ty) {
                    unknown(&mut result);
                    continue;
                }
                if let Some(dictionary) = opaque_dictionary(body, ty, state) {
                    result.suspends |= dictionary.async_destroy;
                    add_function(module, dictionary.cleanup, &mut result);
                    // Foreign generic roots have no substitution in this body.
                    // Never resolve them using bindings from another origin.
                    let count = state.cleanup_origins.as_ref().unwrap().values.get(&ty).map_or(0, Vec::len);
                    for position in 0..count {
                        if !state.step(1) {
                            return result;
                        }
                        let index = state.cleanup_origins.as_ref().unwrap();
                        let (function, captured) = index.values[&ty][position];
                        if index.parameters.contains(&captured) && current_function(state) != Some(function)
                            || index.declaration_parameters.contains(&captured) {
                            unknown(&mut result);
                        } else {
                            pending.push(captured);
                        }
                    }
                } else {
                    unknown(&mut result);
                }
            }
            // P1.3 §3.1: declarations are authoritative even when their object
            // type has never been interned. Generic implementors remain open.
            Ok(Type::Role { decl: role, args }) if args.is_empty() => {
                let mut found = false;
                for (_, declaration) in module.declarations.iter() {
                    if !state.step(1) {
                        return result;
                    }
                    let DeclarationKind::Object { fields, witnesses, destroy, .. } = &declaration.kind else {
                        continue;
                    };
                    if !state.step(witnesses.len()) {
                        return result;
                    }
                    if witnesses.iter().any(|witness| matches!(module.types.get(witness.role),
                        Ok(Type::Role { decl, .. }) if decl == role)) {
                        found = true;
                        if declaration.type_params.is_empty() {
                            pending.extend(fields.iter().map(|field| field.ty));
                            if let Some(destroy) = destroy {
                                add_function(module, *destroy, &mut result);
                            }
                        } else {
                            unknown(&mut result);
                        }
                    }
                }
                if !found {
                    unknown(&mut result);
                }
            }
            // D20 §2.6, async ABI D24.2: in Low a Future duty is settled without waiting in 0.6.0, and its
            // outcome packet then joins the plan with the identities of the result graph.
            Ok(Type::Future(result_type)) if module.profile == Profile::Low => pending.extend(*result_type),
            Ok(Type::Future(_) | Type::Frame(_) | Type::Role { .. }) => unknown(&mut result),
            _ => {}
        }
    }
    result
}
fn add_function(module: &Module, function: FuncId, result: &mut Effects) {
    let Ok(function) = module.functions.get(function) else {
        unknown(result);
        return;
    };
    result.suspends |= function.attrs.contains(&FunctionAttr::Async);
    if let Some(summary) = function.summary.and_then(|id| module.summaries.get(id).ok()) {
        result.suspends |= summary.contract.suspends || summary.contract.async_destroy;
        for &kind in &summary.contract.failures {
            if kind != FailureKind::Cancelled && !result.failures.contains(&kind) {
                result.failures.push(kind);
            }
        }
    } else { unknown(result); }
}
fn unknown(result: &mut Effects) {
    result.suspends = true;
    for kind in [FailureKind::Thrown, FailureKind::Program, FailureKind::Panic, FailureKind::Native] {
        if !result.failures.contains(&kind) {
            result.failures.push(kind);
        }
    }
}

/// TypeOf binds a dictionary for this exact concrete type. A language callback
/// with no packet ABI is not a cleanup dictionary, even if its flags say sync.
fn opaque_dictionary<'a>(
    body: &'a Body, ty: TypeId, state: &mut State<'_>,
) -> Option<&'a DescriptorContract> {
    if !state.step(1) {
        return None;
    }
    let function = current_function(state)?;
    let instruction = (*state.cleanup_origins.as_ref()?.dictionaries.get(&(function, ty))?)?;
    let instruction = body.insts.get(instruction).ok()?;
    let Operation::Core(CoreOp::TypeOf { descriptor, .. }) = &instruction.op else {
        return None;
    };
    Some(descriptor)
}

fn bound_opaque_dictionary(module: &Module, descriptor: &DescriptorContract, ty: TypeId, state: &State<'_>) -> bool {
    let Some(cleanup) = module.functions.get(descriptor.cleanup).ok() else { return false };
    // A body closing the same opaque input cannot certify its own effects
    // circularly. Open graphs require a registry-bound runtime dictionary.
    if cleanup.body.is_some() {
        return false;
    }
    let trusted = cleanup.attrs.iter().find_map(|attr| match attr {
        FunctionAttr::Intrinsic { name, .. } => state.context.external_contracts.descriptor_cleanup(name)
            .filter(|summary| state.context.external_contracts.intrinsic_summary(name) == Some(*summary)),
        _ => None,
    });
    let Some(summary) = module.summaries.get(descriptor.summary).ok() else { return false };
    if trusted != Some(summary) || cleanup.summary != Some(descriptor.summary)
        || summary.entry_kind != EntryKind::CallBoundary
        || summary.parameters.first().is_none_or(|parameter| parameter.convention != FreshConvention::Take) {
        return false;
    }
    let OwnershipAbi::Explicit { input_packet, .. } = cleanup.signature.ownership_abi else { return false };
    let Some(packet) = cleanup.signature.params.get(usize::from(input_packet)) else { return false };
    let mut language = cleanup.signature.params.iter().filter(|ty| {
        !matches!(module.types.get(**ty), Ok(Type::Service(_) | Type::Descriptor))
    });
    module.types.get(*packet) == Ok(&Type::Service(ServiceType::Own))
        && language.next() == Some(&ty) && language.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify::{EntityLocation, VerifyContext, VerifyOptions, VerifyStats};

    #[test]
    fn origin_index_is_built_and_charged_once_per_verification() {
        let source = include_str!("../../tests/fixtures/verify/valid/B11-indexed-origins.oir");
        let module = crate::text::parse(source).unwrap();
        let options = VerifyOptions::default();
        let (function, data) = module.functions.iter().find(|(_, function)| function.symbol == "f").unwrap();
        let body = data.body.as_ref().unwrap();
        let ty = data.signature.params[0];
        let mut state = State {
            module: &module, options: &options, context: VerifyContext::default(), diagnostics: vec![],
            stats: VerifyStats::default(), incomplete: false, exhausted_limit: None, suppressed: 0,
            location: EntityLocation::Instruction { function, instruction: InstId::from_u32(2) },
            source_loc: None, poisoned: BTreeSet::new(), positions: BTreeMap::new(),
            edge_positions: BTreeMap::new(), cleanup_origins: None,
        };
        let first = cleanup(&module, body, ty, &mut state);
        let first_steps = state.stats.steps;
        let first_memory = state.stats.memory_bytes;
        let second = cleanup(&module, body, ty, &mut state);
        let query_steps = state.stats.steps - first_steps;
        assert_eq!(first, second);
        assert!(query_steps > 0);
        let instructions: usize = module.functions.iter().filter_map(|(_, function)| function.body.as_ref())
            .map(|body| body.insts.len()).sum();
        assert!(first_steps >= query_steps + instructions);
        assert_eq!(state.stats.memory_bytes, first_memory);
        assert!(!state.incomplete);
    }
}
