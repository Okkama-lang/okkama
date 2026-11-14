use super::*;

/// Keep the verifier verdict intact and name the binding involved in #592.
pub(super) fn label_borrowed_rebind(module: &Module, report: &mut VerifyReport) {
    for diagnostic in &mut report.diagnostics {
        if !matches!(diagnostic.rule, RuleId::B31 | RuleId::B32) {
            continue;
        }
        let function = match diagnostic.primary.entity {
            EntityLocation::Instruction { function, .. } | EntityLocation::Terminator { function, .. }
                | EntityLocation::Block { function, .. } | EntityLocation::Edge { function, .. }
                | EntityLocation::Value { function, .. } => function,
            _ => continue,
        };
        let Some(body) = module.functions.get(function).ok().and_then(|function| function.body.as_ref()) else {
            continue;
        };
        let mut found = None;
        for (_, block) in body.blocks.iter() {
            for instruction in &block.insts {
                let data = body.insts.get(*instruction).unwrap();
                if let Operation::Ownership(OwnershipOp::Rebind { binding }) = &data.op
                    && binding.original_scope != block.lexical_scope {
                    found = Some(DiagnosticLabel {
                        location: IrLocation {
                            entity: EntityLocation::Instruction { function, instruction: *instruction },
                            text_span: None, source_loc: data.loc,
                        },
                        message: format!("binding {:?}: rebind preserves its scope; #592 does not extend a borrow",
                            body.bindings.get(binding.binding).unwrap().name),
                    });
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        if let Some(label) = found {
            diagnostic.related.push(label);
        }
    }
}

#[derive(Default)]
struct Reachable {
    blocks: BTreeSet<BlockId>,
    insts: BTreeSet<InstId>,
    values: BTreeSet<ValueId>,
    deps: BTreeSet<DepExprId>,
    keys: BTreeSet<KeyExprId>,
    selections: BTreeSet<SelectionId>,
}
impl VisitorMut for Reachable {
    fn value(&mut self, value: &mut ValueId) {
        self.values.insert(*value);
    }
    fn dependency(&mut self, value: &mut DepExprId) {
        self.deps.insert(*value);
    }
    fn key(&mut self, value: &mut KeyExprId) {
        self.keys.insert(*value);
    }
    fn selection(&mut self, value: &mut SelectionId) {
        self.selections.insert(*value);
    }
}

#[derive(Default)]
struct Compact {
    blocks: BTreeMap<BlockId, BlockId>,
    insts: BTreeMap<InstId, InstId>,
    values: BTreeMap<ValueId, ValueId>,
    deps: BTreeMap<DepExprId, DepExprId>,
    keys: BTreeMap<KeyExprId, KeyExprId>,
    selections: BTreeMap<SelectionId, SelectionId>,
}
impl VisitorMut for Compact {
    fn block(&mut self, value: &mut BlockId) {
        *value = self.blocks[value];
    }
    fn instruction(&mut self, value: &mut InstId) {
        *value = self.insts[value];
    }
    fn value(&mut self, value: &mut ValueId) {
        *value = self.values[value];
    }
    fn dependency(&mut self, value: &mut DepExprId) {
        *value = self.deps[value];
    }
    fn key(&mut self, value: &mut KeyExprId) {
        *value = self.keys[value];
    }
    fn selection(&mut self, value: &mut SelectionId) {
        *value = self.selections[value];
    }
}

fn targets(data: &Block) -> Vec<BlockId> {
    let mut result: Vec<_> = data.unwind.iter().map(|unwind| unwind.target.block).collect();
    match &data.term.kind {
        TermKind::Br(target) => result.push(target.block),
        TermKind::Cond { yes, no, .. } => result.extend([yes.block, no.block]),
        TermKind::Switch { cases, default, .. } => {
            result.extend(cases.iter().map(|case| case.target.block));
            result.push(default.block);
        }
        TermKind::InvokeOwn { normal, unwind, .. } => result.extend([normal.block, unwind.block]),
        TermKind::InvokeDefer { continue_edge, replacements, unwind, .. } => {
            result.push(continue_edge.block);
            result.extend(replacements.iter().map(|replacement| replacement.edge.block));
            result.extend(unwind.iter().map(|edge| edge.block));
        }
        TermKind::Catch { handler, next, .. } => result.extend([handler.block, next.block]),
        _ => {}
    }
    result
}

impl Driver<'_, '_> {
    pub(super) fn compact(&mut self) -> Result<(), OwnError> {
        let mut live = Reachable::default();
        let mut pending = vec![self.body.entry];
        while let Some(block) = pending.pop() {
            self.emitter.charge(1, 32)?;
            if !live.blocks.insert(block) {
                continue;
            }
            let data = self.body.blocks.get(block).map_err(arena)?;
            pending.extend(targets(data));
            live.insts.extend(&data.insts);
            data.clone().rewrite(&mut live);
        }
        for inst in live.insts.clone() {
            self.body.insts.get(inst).map_err(arena)?.clone().rewrite(&mut live);
        }
        let mut visited = Reachable::default();
        loop {
            let values: Vec<_> = live.values.difference(&visited.values).copied().collect();
            let deps: Vec<_> = live.deps.difference(&visited.deps).copied().collect();
            let keys: Vec<_> = live.keys.difference(&visited.keys).copied().collect();
            let selections: Vec<_> = live.selections.difference(&visited.selections).copied().collect();
            let count = values.len() + deps.len() + keys.len() + selections.len();
            self.emitter.charge(count, count * 64)?;
            if count == 0 {
                break;
            }
            visited.values.extend(&values);
            visited.deps.extend(&deps);
            visited.keys.extend(&keys);
            visited.selections.extend(&selections);
            for value in values {
                self.body.values.get(value).map_err(arena)?.clone().rewrite(&mut live);
            }
            for deps in deps {
                self.body.deps.get(deps).map_err(arena)?.clone().rewrite(&mut live);
            }
            for key in keys {
                self.body.keys.get(key).map_err(arena)?.clone().rewrite(&mut live);
            }
            for selection in selections {
                self.body.selections.get(selection).map_err(arena)?.clone().rewrite(&mut live);
            }
        }
        let mut map = Compact::default();
        macro_rules! compact {
            ($arena:ident) => {
                let mut next = Arena::new();
                for id in &live.$arena {
                    map.$arena.insert(*id, next.alloc(self.body.$arena.get(*id).map_err(arena)?.clone())
                        .map_err(arena)?);
                }
                self.body.$arena = next;
            };
        }
        let mut blocks = Arena::new();
        for id in std::iter::once(self.body.entry).chain(live.blocks.iter().copied()
            .filter(|id| *id != self.body.entry)) {
            map.blocks.insert(id, blocks.alloc(self.body.blocks.get(id).map_err(arena)?.clone()).map_err(arena)?);
        }
        self.body.blocks = blocks;
        compact!(insts);
        compact!(values);
        compact!(deps);
        compact!(keys);
        compact!(selections);
        self.body.rewrite(&mut map);
        Ok(())
    }
}
