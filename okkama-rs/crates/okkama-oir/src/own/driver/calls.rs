use super::*;
use crate::own::emit::{AdmissionComponent, ScopeExtraction};

impl Driver<'_, '_> {
    /// Extract one selection from each accessible admission component. Aliases
    /// occur once in the selection; bound journals never become call inputs.
    pub(super) fn offered(
        &mut self,
        point: Landing,
        originals: &[ValueId],
    ) -> Result<(Landing, ValueId), OwnError> {
        let mut values = BTreeSet::new();
        for original in originals {
            if self.managed(self.high.values.get(*original).map_err(arena)?.ty) {
                values.insert(point.env.value(*original)?.value);
            }
        }
        self.offered_values(point, values.into_iter().collect())
    }

    pub(super) fn offered_values(
        &mut self,
        point: Landing,
        values: Vec<ValueId>,
    ) -> Result<(Landing, ValueId), OwnError> {
        self.extract_values(point, values, true)
    }

    pub(super) fn local_values(
        &mut self,
        point: Landing,
        values: Vec<ValueId>,
    ) -> Result<(Landing, ValueId), OwnError> {
        self.extract_values(point, values, false)
    }

    fn extract_values(
        &mut self,
        mut point: Landing,
        values: Vec<ValueId>,
        admissible: bool,
    ) -> Result<(Landing, ValueId), OwnError> {
        if values.is_empty() || point.env.scopes.is_empty() {
            point = self.emit(point, Operation::Ownership(OwnershipOp::Empty {}))?;
            let packet = point.result(SlotName::Packet)?;
            return Ok((point, packet));
        }
        let mut selections = vec![];
        for value in values {
            selections.push(self.body.selections.alloc(SelectionExpr::Value(value)).map_err(arena)?);
        }
        let selection = self.body.selections.alloc(SelectionExpr::Union(selections)).map_err(arena)?;
        let selection = if admissible {
            self.body.selections.alloc(SelectionExpr::Admissible(selection)).map_err(arena)?
        } else {
            selection
        };
        let requests: Vec<_> = point.env.scopes.keys().rev().map(|owner| ScopeExtraction {
            owner: *owner, selection, component: if admissible {
                AdmissionComponent::Admission
            } else {
                AdmissionComponent::Preserve
            },
        }).collect();
        let batch = self.emitter.extract_scopes(self.module, &mut self.body, point, &requests)?;
        for failure in batch.failures {
            self.record_failure(failure.restored, self.current_unwind.clone())?;
        }
        point = batch.normal;
        let packets = batch.packets.into_iter().map(|part| part.packet).collect();
        point = self.emit(point, Operation::Ownership(OwnershipOp::Join { packets }))?;
        let packet = point.result(SlotName::Packet)?;
        Ok((point, packet))
    }

    pub(super) fn call(
        &mut self,
        mut point: Landing,
        instruction: &Instruction,
        function: FuncId,
    ) -> Result<Landing, OwnError> {
        let mut op = instruction.op.clone();
        let call = match &mut op {
            Operation::Call(CallOp::Direct { call, .. } | CallOp::Native { call, .. }
                | CallOp::Intrinsic { call, .. }) => call,
            _ => return Err(flow_error("expected direct call")),
        };
        let originals: Vec<_> = call.args.iter().map(|arg| arg.value).collect();
        let summary = self.module.functions.get(function).map_err(arena)?.summary
            .ok_or_else(|| flow_error("call has no inferred summary"))?;
        call.summary = Some(summary);
        let observation = self.module.summaries.get(summary).map_err(arena)?.entry_kind
            == EntryKind::ObservationAdmissionStaysCaller;
        let packet = if observation {
            None
        } else {
            let (next, packet) = self.offered(point, &originals)?;
            point = next;
            Some(packet)
        };
        let mut missing = false;
        op.rewrite(&mut Rename { env: &point.env, missing: &mut missing });
        if missing {
            return Err(flow_error("call operand has no reaching SSA definition"));
        }
        let call = match &mut op {
            Operation::Call(CallOp::Direct { call, .. } | CallOp::Native { call, .. }
                | CallOp::Intrinsic { call, .. }) => call,
            _ => unreachable!(),
        };
        call.packet = packet;
        call.deps = originals.iter().filter_map(|value| point.env.values[value].deps).collect();
        self.prune(&mut point);
        point = self.emit(point, op)?;
        let output_packet = point.results.iter().find(|(name, _)| *name == SlotName::Packet).map(|(_, value)| *value);
        let deps = point.results.iter().find(|(name, _)| *name == SlotName::Deps).map(|(_, value)| *value);
        let results = point.results.clone();
        for (original, (_, value)) in instruction.results.iter().zip(&results) {
            let managed = self.managed(self.body.values.get(*value).map_err(arena)?.ty);
            let full = if managed && deps.is_none() {
                point = self.emit(point, Operation::Ownership(OwnershipOp::Dependencies { value: *value }))?;
                Some(point.result(SlotName::Value)?)
            } else if managed {
                deps
            } else {
                None
            };
            point.env.values.insert(*original, LatestValue { value: *value, deps: full });
        }
        if let Some(packet) = output_packet {
            let scope = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
            let state = point.env.scopes.get(&scope).copied()
                .ok_or_else(|| flow_error("call output packet has no lexical owner"))?;
            point = self.emit(point, Operation::Ownership(OwnershipOp::Admit { state, packet }))?;
        }
        Ok(point)
    }
}
