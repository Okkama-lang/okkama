//! Materialize memoized known entry specializations before freezing equations.
use super::*;
use crate::visit::{Rewrite, VisitorMut};

struct ScopeMap<'a>(&'a BTreeMap<ScopeId, ScopeId>);
impl VisitorMut for ScopeMap<'_> {
    fn scope(&mut self, scope: &mut ScopeId) {
        if let Some(mapped) = self.0.get(scope) {
            *scope = *mapped;
        }
    }
}

pub(super) fn materialize(module: &mut Module, budget: &mut Budget<'_>) -> Result<(), OwnError> {
    let mut requested = BTreeMap::new();
    for (_, function) in module.functions.iter() {
        let Some(body) = &function.body else { continue };
        for (_, instruction) in body.insts.iter() {
            budget.work(1)?;
            let request = match &instruction.op {
                Operation::Core(CoreOp::FnClosure { function, captures, ty, .. }) => {
                    Some((*function, captures.len(), *ty))
                }
                Operation::Core(CoreOp::FnBind { function, ty, .. }) => Some((*function, 1, *ty)),
                _ => None,
            };
            if let Some(key) = request {
                requested.insert(key, None);
            }
        }
    }
    for ((target, captures, ty), entry) in &mut requested {
        let mut function = module.functions.get(*target).map_err(arena)?.clone();
        if matches!(module.types.get(*ty), Ok(Type::Fn(FnType { is_async: true, .. }))) {
            return Err(budget.fail(OwnDiagnosticKind::UnsupportedAsync, "asynchronous closure creation"));
        }
        if function.body.is_none() {
            // External entries already supply a checked ABI. Do not fabricate
            // an implementation from an unavailable body.
            *entry = Some(*target);
            continue;
        }
        if !function.type_params.is_empty() {
            return Err(budget.fail(OwnDiagnosticKind::MissingDescriptor,
                "known entry specialization requires mono's concrete callable substitution"));
        }
        let id = FuncId::from_u32(module.functions.len() as u32);
        function.symbol = fresh_symbol(module, &format!("{}.__own_entry_{}", function.symbol, captures), budget)?;
        let mut scopes = BTreeMap::new();
        let originals: Vec<_> = module.scopes.iter().filter(|(_, scope)| scope.function == *target)
            .map(|(id, scope)| (id, scope.clone())).collect();
        budget.memory(originals.len() * 128)?;
        for (old, mut scope) in originals {
            scope.function = id;
            let new = module.scopes.alloc(scope).map_err(arena)?;
            scopes.insert(old, new);
        }
        for new in scopes.values() {
            let scope = module.scopes.get_mut(*new).map_err(arena)?;
            scope.parent = scope.parent.map(|parent| scopes[&parent]);
        }
        let body = function.body.as_mut().unwrap();
        budget.work(body.insts.len() + body.values.len())?;
        budget.memory(body.insts.len() * 256 + body.values.len() * 128 + body.blocks.len() * 128)?;
        body.rewrite(&mut ScopeMap(&scopes));
        *entry = Some(module.functions.alloc(function).map_err(arena)?);
        budget.stats.materialized_adapters += 1;
    }
    for (_, function) in module.functions.iter_mut() {
        let Some(body) = &mut function.body else { continue };
        for (_, instruction) in body.insts.iter_mut() {
            match &mut instruction.op {
                Operation::Core(CoreOp::FnClosure { function, captures, ty, .. }) => {
                    *function = requested[&(*function, captures.len(), *ty)].unwrap();
                }
                Operation::Core(CoreOp::FnBind { function, ty, .. }) => {
                    *function = requested[&(*function, 1, *ty)].unwrap();
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Witness implementations have one public transport shape, including a deps
/// slot for a plain concrete receiver implementing an opaque role.
pub(super) fn witness_entries(module: &mut Module, budget: &mut Budget<'_>) -> Result<(), OwnError> {
    let requests: Vec<_> = module.declarations.iter().flat_map(|(object, declaration)| {
        let DeclarationKind::Object { witnesses, .. } = &declaration.kind else { return vec![] };
        witnesses.iter().enumerate().flat_map(|(witness, data)| {
            data.slots.iter().enumerate().map(move |(slot, target)| (object, witness, slot, *target))
        }).collect::<Vec<_>>()
    }).collect();
    let mut adapters = BTreeMap::new();
    for (object, witness, slot, target) in requests {
        let entry = if let Some(entry) = adapters.get(&target) {
            *entry
        } else {
            let mut function = module.functions.get(target).map_err(arena)?.clone();
            if function.body.is_none() {
                // Registered external entries already bind their public ABI.
                // Keep their original identity for catalog and join checks.
                continue;
            }
            let entry = FuncId::from_u32(module.functions.len() as u32);
            if !function.type_params.is_empty() {
                return Err(budget.fail(OwnDiagnosticKind::MissingDescriptor,
                    "witness entry requires mono's concrete implementation substitution"));
            }
            function.symbol = fresh_symbol(module, &format!("own_witness_entry_{}", entry.as_u32()), budget)?;
            let originals: Vec<_> = module.scopes.iter().filter(|(_, scope)| scope.function == target)
                .map(|(id, scope)| (id, scope.clone())).collect();
            let mut scopes = BTreeMap::new();
            for (old, mut scope) in originals {
                scope.function = entry;
                scopes.insert(old, module.scopes.alloc(scope).map_err(arena)?);
            }
            for new in scopes.values() {
                let scope = module.scopes.get_mut(*new).map_err(arena)?;
                scope.parent = scope.parent.map(|parent| scopes[&parent]);
            }
            let body = function.body.as_mut().unwrap();
            budget.work(body.values.len() + body.insts.len())?;
            budget.memory(body.values.len() * 128 + body.insts.len() * 256)?;
            body.rewrite(&mut ScopeMap(&scopes));
            let entry = module.functions.alloc(function).map_err(arena)?;
            adapters.insert(target, entry);
            budget.stats.materialized_adapters += 1;
            entry
        };
        let DeclarationKind::Object { witnesses, .. } = &mut module.declarations.get_mut(object).map_err(arena)?.kind
            else { unreachable!() };
        witnesses[witness].slots[slot] = entry;
    }
    Ok(())
}

pub(super) fn role_signatures(module: &mut Module) -> Result<(), OwnError> {
    let mut signatures = BTreeMap::new();
    for (_, declaration) in module.declarations.iter() {
        let DeclarationKind::Object { witnesses, .. } = &declaration.kind else { continue };
        for witness in witnesses {
            let Type::Role { decl, .. } = module.types.get(witness.role).map_err(arena)? else { continue };
            for (slot, function) in witness.slots.iter().enumerate() {
                let mut signature = module.functions.get(*function).map_err(arena)?.signature.clone();
                signature.params.remove(0);
                if let OwnershipAbi::Explicit { input_packet, .. } = &mut signature.ownership_abi {
                    *input_packet = input_packet.checked_sub(1)
                        .ok_or_else(|| flow_error("witness receiver overlaps its packet"))?;
                }
                signatures.entry((*decl, slot)).or_insert(signature);
            }
        }
    }
    for ((decl, slot), signature) in signatures {
        let DeclarationKind::Role { methods, .. } = &mut module.declarations.get_mut(decl).map_err(arena)?.kind
            else { unreachable!() };
        methods[slot].signature = signature;
    }
    Ok(())
}

fn fresh_symbol(module: &Module, base: &str, budget: &mut Budget<'_>) -> Result<String, OwnError> {
    let mut name = base.to_owned();
    let mut suffix = 0usize;
    loop {
        budget.work(module.functions.len())?;
        if module.functions.iter().all(|(_, function)| function.symbol != name) {
            budget.memory(name.len())?;
            return Ok(name);
        }
        suffix += 1;
        name = format!("{base}_{suffix}");
    }
}

pub(super) fn dictionary_summaries(module: &mut Module, budget: &mut Budget<'_>) -> Result<(), OwnError> {
    budget.work(module.functions.len())?;
    budget.memory(module.functions.len() * 32)?;
    let summaries: BTreeMap<_, _> = module.functions.iter().map(|(id, function)| (id, function.summary)).collect();
    for (_, function) in module.functions.iter_mut() {
        let Some(body) = &mut function.body else { continue };
        budget.work(body.insts.len() + body.descriptor_bindings.len())?;
        for binding in &mut body.descriptor_bindings {
            binding.contract.summary = summaries[&binding.contract.cleanup]
                .ok_or_else(|| flow_error("dictionary cleanup has no final entry summary"))?;
        }
        for (_, instruction) in body.insts.iter_mut() {
            if let Operation::Core(CoreOp::TypeOf { descriptor, .. }) = &mut instruction.op {
                descriptor.summary = summaries[&descriptor.cleanup]
                    .ok_or_else(|| flow_error("dictionary cleanup has no final entry summary"))?;
            }
        }
    }
    Ok(())
}

#[derive(Default)]
pub(super) struct EntrySets {
    pub callables: std::collections::BTreeSet<FuncId>,
    pub witnesses: std::collections::BTreeSet<FuncId>,
}

pub(super) fn entry_sets(module: &Module, budget: &mut Budget<'_>) -> Result<EntrySets, OwnError> {
    let mut entries = EntrySets::default();
    for (_, function) in module.functions.iter() {
        budget.work(1)?;
        let Some(body) = &function.body else { continue };
        for (_, instruction) in body.insts.iter() {
            budget.work(1)?;
            if let Operation::Core(CoreOp::FnClosure { function, .. } | CoreOp::FnBind { function, .. }) =
                instruction.op
                && entries.callables.insert(function) {
                budget.memory(64)?;
            }
        }
    }
    for (_, declaration) in module.declarations.iter() {
        budget.work(1)?;
        let DeclarationKind::Object { witnesses, .. } = &declaration.kind else { continue };
        for witness in witnesses {
            for function in &witness.slots {
                budget.work(1)?;
                if entries.witnesses.insert(*function) {
                    budget.memory(64)?;
                }
            }
        }
    }
    Ok(entries)
}
