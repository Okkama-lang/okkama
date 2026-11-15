use super::*;

impl Driver<'_, '_> {
    pub(super) fn stage(
        &mut self,
        mut point: Landing,
        action: ScopeId,
        field: FieldRef,
        original: ValueId,
    ) -> Result<Landing, OwnError> {
        let ty = self.high.values.get(original).map_err(arena)?.ty;
        // A value that only reaches plain targets through references carries
        // deps but can hold no obligation, so there is nothing to stage.
        if !self.managed(ty) || !self.analysis.types.get(&ty).is_some_and(TypeClassification::may_own) {
            return Ok(point);
        }
        let value = point.env.value(original)?.value;
        let (next, packet) = self.local_values(point, vec![value])?;
        point = next;
        let value = point.env.value(original)?.value;
        let state = point.env.scopes.get(&action).copied()
            .ok_or_else(|| flow_error("staging action scope is not retained"))?;
        self.emit(point, Operation::Ownership(OwnershipOp::Stage { stage: ElaboratedStage {
            action, field, resource: ResourceValue { value, packet }, states: TransferStates::Same(state),
        } }))
    }

    pub(super) fn construct(
        &mut self,
        mut point: Landing,
        instruction: &Instruction,
    ) -> Result<Landing, OwnError> {
        let mut op = instruction.op.clone();
        let inputs = match &op {
            Operation::Core(CoreOp::ObjNew { fields, .. } | CoreOp::VariantNew { fields, .. }) => fields.clone(),
            Operation::Core(CoreOp::ObjNext { updates, .. }) => updates.iter().map(|field| field.value).collect(),
            _ => vec![],
        };
        let creates = matches!(op, Operation::Core(CoreOp::ObjNew {
            construction: ConstructionStatus::Complete, .. } | CoreOp::ObjNext { .. } | CoreOp::VariantNew { .. }));
        // Only a result that may own takes a creation packet; one that merely
        // reaches references carries deps but no obligation.
        let managed = instruction.results.iter().any(|value| self.managed(self.high.values.get(*value).unwrap().ty));
        let owning = instruction.results.iter().any(|value| {
            let ty = self.high.values.get(*value).unwrap().ty;
            self.managed(ty) && self.analysis.types.get(&ty).is_some_and(TypeClassification::may_own)
        });
        let owners = self.current_instruction.and_then(|inst| self.constructor_inputs.get(&inst));
        if creates && owning && owners.is_some_and(|owners| owners.len() > 1) {
            return Err(flow_error("pre-accept construction spans admission owners; stage fields at the action first"));
        }
        let restore_to = owners.and_then(|owners| owners.first().copied());
        let packet = if creates && owning {
            let (next, packet) = self.offered(point, &inputs)?;
            point = next;
            Some(packet)
        } else {
            None
        };
        let mut missing = false;
        op.rewrite(&mut Rename { env: &point.env, missing: &mut missing });
        if missing {
            return Err(flow_error("construction operand has no reaching definition"));
        }
        match &mut op {
            Operation::Core(CoreOp::ObjNew { packet: input, .. } | CoreOp::ObjNext { packet: input, .. }
                | CoreOp::VariantNew { packet: input, .. }) => {
                *input = packet;
            }
            Operation::Core(CoreOp::ObjInit { field, completion, .. }) => {
                let declaration = self.module.declarations.get(field.declaration).map_err(arena)?;
                if matches!(&declaration.kind, DeclarationKind::Object { fields, .. }
                    if fields[field.index as usize].name == "$pending") {
                    let scope = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
                    let root = stack(self.module, scope)?[0];
                    if let Some(state) = point.env.scopes.get(&root) {
                        *completion = Some(ConstructionCompletion { state: *state });
                    }
                }
            }
            _ => {}
        }
        let recipe = match &op {
            Operation::Core(CoreOp::SuccTake { value, .. }) => {
                let ValueFacts::Managed { deps } = self.body.values.get(*value).map_err(arena)?.facts else {
                    return self.finish_construction(point, op, instruction, &[], None);
                };
                Some(self.body.deps.get(deps).map_err(arena)?.clone())
            }
            _ => None,
        };
        let recipes = if managed {
            recipe.map(|recipe| self.body.deps.alloc(recipe).map(|deps| vec![(SlotName::Value, deps)]))
                .transpose().map_err(arena)?.unwrap_or_default()
        } else {
            vec![]
        };
        self.finish_construction(point, op, instruction, &recipes, restore_to)
    }

    pub(super) fn finish_construction(
        &mut self,
        point: Landing,
        op: Operation,
        instruction: &Instruction,
        recipes: &[(SlotName, DepExprId)],
        restore_to: Option<ScopeId>,
    ) -> Result<Landing, OwnError> {
        let initialized = match (&instruction.op, &op) {
            (Operation::Core(CoreOp::ObjInit { object: original, .. }),
                Operation::Core(CoreOp::ObjInit { object, .. })) => Some((*original, *object)),
            _ => None,
        };
        let emitted = self.emitter.emit_with_result_deps(self.module, &mut self.body, point.block,
            point.env, op, recipes)?;
        if let Some(mut unwind) = emitted.unwind {
            if let Some(owner) = restore_to
                && let Some((_, packet)) = unwind.results.iter().find(|(slot, _)| *slot == SlotName::Packet) {
                unwind.env.restore_to.insert(*packet, owner);
            }
            self.record_failure(unwind, self.current_unwind.clone())?;
        }
        let mut point = emitted.normal;
        let results = point.results.clone();
        let packet = results.iter().find(|(slot, _)| *slot == SlotName::Packet).map(|(_, value)| *value);
        for (original, (_, value)) in instruction.results.iter().zip(&results) {
            let mut latest = LatestValue { value: *value, deps: None };
            if self.managed(self.body.values.get(*value).map_err(arena)?.ty) {
                point = self.emit(point, Operation::Ownership(OwnershipOp::Dependencies { value: *value }))?;
                latest.deps = Some(point.result(SlotName::Value)?);
            }
            point.env.values.insert(*original, latest);
        }
        if let Some(packet) = packet {
            let scope = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
            let state = point.env.scopes.get(&scope).copied()
                .ok_or_else(|| flow_error("construction packet has no retained lexical owner"))?;
            point = self.emit(point, Operation::Ownership(OwnershipOp::Admit { state, packet }))?;
        }
        if let Some((original, object)) = initialized
            && self.managed(self.body.values.get(object).map_err(arena)?.ty) {
            // Initialization changes the reachable field graph without creating
            // a new self reference or a second obligation. Refresh full deps for
            // every definite alias, including lazy binding captures.
            point = self.emit(point, Operation::Ownership(OwnershipOp::Dependencies { value: object }))?;
            let deps = point.result(SlotName::Value)?;
            let roots = self.incomplete.get(&original);
            let mut physical = BTreeSet::from([object]);
            for (source, latest) in &mut point.env.values {
                let same = *source == original || roots.is_some_and(|roots| roots.len() == 1
                    && self.incomplete.get(source) == Some(roots));
                if same {
                    physical.insert(latest.value);
                    latest.deps = Some(deps);
                }
            }
            for latest in point.env.bindings.values_mut() {
                if physical.contains(&latest.value) {
                    latest.deps = Some(deps);
                }
            }
        }
        Ok(point)
    }
}

