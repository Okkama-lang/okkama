//! Finite callable origins, propagated through simultaneous CFG parameters.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Origins {
    pub entries: BTreeSet<(FuncId, usize)>,
    pub open: bool,
}

pub(super) fn callable_origins(
    module: &Module,
    body: &Body,
    budget: &mut Budget<'_>,
) -> Result<BTreeMap<ValueId, Origins>, OwnError> {
    budget.work(body.values.len() + body.insts.len() + body.blocks.len())?;
    let mut origins: BTreeMap<_, _> = body.values.iter().filter_map(|(value, data)| {
        matches!(module.types.get(data.ty), Ok(Type::Fn(_))).then_some((value, Origins::default()))
    }).collect();
    budget.memory(origins.len() * 128)?;
    for (value, origin) in &mut origins {
        match body.values.get(*value).map_err(arena)?.def {
            Definition::BlockParam { block, .. } => origin.open = block == body.entry,
            Definition::InstResult { inst, .. } => {
                match &body.insts.get(inst).map_err(arena)?.op {
                    Operation::Core(CoreOp::FnClosure { function, captures, .. }) => {
                        origin.entries.insert((*function, captures.len()));
                    }
                    Operation::Core(CoreOp::FnBind { function, .. }) => {
                        origin.entries.insert((*function, 1));
                    }
                    Operation::Core(CoreOp::FnBindRole { role, slot, .. }) => {
                        for (_, declaration) in module.declarations.iter() {
                            budget.work(1)?;
                            let DeclarationKind::Object { witnesses, .. } = &declaration.kind else { continue };
                            for witness in witnesses {
                                if matches!(module.types.get(witness.role), Ok(Type::Role { decl, .. }) if decl == role)
                                    && let Some(function) = witness.slots.get(*slot as usize) {
                                    origin.entries.insert((*function, 1));
                                }
                            }
                        }
                        origin.open = origin.entries.is_empty();
                    }
                    Operation::Core(CoreOp::Retag { .. }) => {}
                    _ => origin.open = true,
                }
            }
        }
    }
    loop {
        budget.work(body.insts.len() + body.blocks.len())?;
        let mut changed = false;
        let mut pairs = vec![];
        for (_, instruction) in body.insts.iter() {
            if let Operation::Core(CoreOp::Retag { value, .. }) = instruction.op {
                pairs.extend(instruction.results.iter().map(|result| (*result, value)));
            }
        }
        for (_, block) in body.blocks.iter() {
            let mut edge = |target: BlockId, args: &[EdgeArg]| -> Result<(), OwnError> {
                for (parameter, argument) in body.blocks.get(target).map_err(arena)?.params.iter().zip(args) {
                    if let EdgeArg::Value(value) = argument {
                        pairs.push((*parameter, *value));
                    } else if let Some(origin) = origins.get_mut(parameter) {
                        changed |= !origin.open;
                        origin.open = true;
                    }
                }
                Ok(())
            };
            let values = |target: &Target| target.args.iter().copied().map(EdgeArg::Value).collect::<Vec<_>>();
            match &block.term.kind {
                TermKind::Br(target) => edge(target.block, &values(target))?,
                TermKind::Cond { yes, no, .. } => {
                    edge(yes.block, &values(yes))?;
                    edge(no.block, &values(no))?;
                }
                TermKind::Switch { cases, default, .. } => {
                    for case in cases {
                        edge(case.target.block, &values(&case.target))?;
                    }
                    edge(default.block, &values(default))?;
                }
                TermKind::InvokeOwn { normal, unwind, .. } => {
                    edge(normal.block, &normal.args)?;
                    edge(unwind.block, &unwind.args)?;
                }
                _ => {}
            }
        }
        for (parameter, argument) in pairs {
            budget.work(1)?;
            let Some(incoming) = origins.get(&argument).cloned() else { continue };
            if let Some(origin) = origins.get_mut(&parameter) {
                changed |= incoming.open && !origin.open;
                origin.open |= incoming.open;
                for entry in incoming.entries {
                    if origin.entries.insert(entry) {
                        budget.memory(32)?;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    Ok(origins)
}
