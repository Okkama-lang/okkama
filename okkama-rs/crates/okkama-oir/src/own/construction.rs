use super::*;
use super::edges::Graph;
use std::collections::BTreeSet;

pub(super) fn incomplete_aliases(
    high: &Body,
    graph: &Graph,
    budget: &mut Budget<'_>,
) -> Result<BTreeMap<ValueId, BTreeSet<ValueId>>, OwnError> {
    let mut aliases = BTreeMap::new();
    for (_, instruction) in high.insts.iter() {
        if matches!(instruction.op, Operation::Core(CoreOp::ObjNew {
            construction: ConstructionStatus::Incomplete, .. })) {
            for value in &instruction.results {
                aliases.insert(*value, BTreeSet::from([*value]));
            }
        }
    }
    loop {
        let mut changed = false;
        for edge in &graph.edges {
            let parameters = &high.blocks.get(edge.target).map_err(arena)?.params;
            for (value, parameter) in edge.args.iter().zip(parameters) {
                if let Some(roots) = aliases.get(value).cloned() {
                    budget.work(1 + roots.len())?;
                    let target = aliases.entry(*parameter).or_default();
                    let before = target.len();
                    target.extend(roots);
                    changed |= before != target.len();
                }
            }
        }
        if !changed {
            break;
        }
    }
    budget.memory(aliases.values().map(|values| 64 + values.len() * 32).sum())?;
    Ok(aliases)
}

type Fields = BTreeMap<(ScopeId, DeclId, u32), BTreeSet<ValueId>>;

fn fields_at_block(
    module: &Module,
    body: &Body,
    block: BlockId,
    mut fields: Fields,
    mut completions: Option<&mut BTreeMap<InstId, Vec<ValueId>>>,
    budget: &mut Budget<'_>,
) -> Result<(Fields, BTreeMap<InstId, Fields>), OwnError> {
    let data = body.blocks.get(block).map_err(arena)?;
    let root = super::edges::stack(module, data.lexical_scope)?[0];
    let mut before = BTreeMap::new();
    for inst in &data.insts {
        budget.work(1 + fields.values().map(BTreeSet::len).sum::<usize>())?;
        before.insert(*inst, fields.clone());
        match &body.insts.get(*inst).map_err(arena)?.op {
            Operation::Lexical(LexicalOp::Stage { action, field, value }) => {
                fields.insert((*action, field.declaration, field.index), BTreeSet::from([*value]));
            }
            Operation::Core(CoreOp::ObjInit { field, value, .. }) => {
                let declaration = module.declarations.get(field.declaration).map_err(arena)?;
                let DeclarationKind::Object { fields: layout, .. } = &declaration.kind else {
                    return Err(flow_error("initialization requires an object layout"));
                };
                if layout[field.index as usize].name == "$pending" {
                    if let Some(completions) = &mut completions {
                        let values = fields.iter().filter(|((owner, declaration, _), _)| {
                            *owner == root && *declaration == field.declaration
                        }).flat_map(|(_, values)| values.iter().copied()).collect();
                        completions.insert(*inst, values);
                    }
                } else {
                    fields.insert((root, field.declaration, field.index), BTreeSet::from([*value]));
                }
            }
            Operation::Core(CoreOp::ObjNew { declaration, fields: values,
                construction: ConstructionStatus::Incomplete, .. }) => {
                for (index, value) in values.iter().enumerate() {
                    fields.insert((root, *declaration, index as u32), BTreeSet::from([*value]));
                }
            }
            _ => {}
        }
    }
    budget.memory((1 + before.len()) * (64 + fields.values().map(BTreeSet::len).sum::<usize>() * 32))?;
    Ok((fields, before))
}

/// Solve final field slots before inferring relations. Publishing provisional
/// slots into a monotone summary would retain an overwritten field forever.
pub(super) fn completion_fields(
    module: &Module,
    body: &Body,
    graph: &Graph,
    budget: &mut Budget<'_>,
) -> Result<BTreeMap<InstId, Vec<ValueId>>, OwnError> {
    let ranks: BTreeMap<_, _> = graph.rpo.iter().enumerate().map(|(rank, block)| (*block, rank)).collect();
    let mut entries = BTreeMap::from([(body.entry, Fields::new())]);
    let mut queue = BTreeSet::from([(ranks[&body.entry], body.entry)]);
    while let Some((_, block)) = queue.pop_first() {
        let (after, before) = fields_at_block(module, body, block, entries[&block].clone(), None, budget)?;
        for index in &graph.outgoing[block.as_u32() as usize] {
            let edge = &graph.edges[*index];
            let incoming = edge.failure_point.and_then(|inst| before.get(&inst)).unwrap_or(&after);
            let unseen = !entries.contains_key(&edge.target);
            let target = entries.entry(edge.target).or_default();
            let mut changed = unseen;
            for (field, values) in incoming {
                budget.work(1 + values.len())?;
                let slot = target.entry(*field).or_default();
                let old = slot.len();
                slot.extend(values);
                changed |= old != slot.len();
            }
            if changed {
                queue.insert((ranks[&edge.target], edge.target));
            }
        }
    }
    let mut result = BTreeMap::new();
    for block in &graph.rpo {
        fields_at_block(module, body, *block, entries[block].clone(), Some(&mut result), budget)?;
    }
    Ok(result)
}
