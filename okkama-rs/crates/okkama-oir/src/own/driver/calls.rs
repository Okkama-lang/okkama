use super::*;
use crate::own::emit::{AdmissionComponent, ScopeExtraction};

impl Driver<'_, '_> {
    /// Extract one selection from each accessible admission component. Aliases
    /// occur once in the selection; bound journals never become call inputs.
    pub(super) fn may_offer(&self, point: &Landing, original: ValueId) -> bool {
        if !self.managed(self.high.values.get(original).unwrap().ty) {
            return false;
        }
        let flow = self.current_instruction.and_then(|inst| self.flow.before_instructions.get(&inst));
        flow.is_none_or(|flow| {
            let deps = &flow.values[original].deps;
            flow.journals.iter().any(|((owner, atom), quantity)| {
                point.env.scopes.contains_key(owner) && deps.contains(atom)
                    && !matches!(quantity.presence, domain::Presence::Bottom | domain::Presence::Absent)
                    && flow.provenance.get(&(*owner, *atom)).is_some_and(|mask| mask & 1 != 0)
            })
        })
    }

    pub(super) fn offered(
        &mut self,
        point: Landing,
        originals: &[ValueId],
    ) -> Result<(Landing, ValueId), OwnError> {
        let mut values = BTreeSet::new();
        for original in originals {
            if self.elide_offers && !self.may_offer(&point, *original) {
                continue;
            }
            if self.managed(self.high.values.get(*original).map_err(arena)?.ty) {
                values.insert(point.env.value(*original)?.value);
            }
        }
        if values.is_empty() && originals.iter().any(|original| {
            self.managed(self.high.values.get(*original).unwrap().ty)
        }) {
            self.emitter.budget_ref().stats.splits_elided += point.env.scopes.len();
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
        point: Landing,
        instruction: &Instruction,
        function: FuncId,
    ) -> Result<Landing, OwnError> {
        let mut op = instruction.op.clone();
        let call = match &mut op {
            Operation::Call(CallOp::Direct { call, .. } | CallOp::Native { call, .. }
                | CallOp::Intrinsic { call, .. }) => call,
            _ => return Err(flow_error("expected direct call")),
        };
        call.summary = self.module.functions.get(function).map_err(arena)?.summary;
        self.emit_call(point, instruction, op)
    }

    pub(super) fn emit_call(
        &mut self,
        mut point: Landing,
        instruction: &Instruction,
        mut op: Operation,
    ) -> Result<Landing, OwnError> {
        let (call, receiver) = summaries::call(&op).ok_or_else(|| flow_error("expected call"))?;
        let originals: Vec<_> = receiver.iter().chain(&call.args).map(|arg| arg.value).collect();
        let summary = call.summary.ok_or_else(|| flow_error("call has no inferred summary"))?;
        let observation = self.module.summaries.get(summary).map_err(arena)?.entry_kind
            == EntryKind::ObservationAdmissionStaysCaller;
        let open_protocol = if let Operation::Call(CallOp::Closure { callee, .. }) = &op {
            let origin = &self.origins[&callee.value];
            if origin.open || origin.entries.is_empty() {
                self.module.summaries.get(summary).map_err(arena)?.contract.protocol
            } else {
                None
            }
        } else {
            None
        };
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
        let witness = match &op {
            Operation::Call(CallOp::Direct { function, .. } | CallOp::Native { function, .. }
                | CallOp::Intrinsic { function, .. }) => self.witnesses.contains(function),
            _ => false,
        };
        let mut deps = vec![];
        for (index, original) in originals.iter().enumerate() {
            let latest = point.env.value(*original)?;
            if self.managed(self.high.values.get(*original).map_err(arena)?.ty) {
                deps.push(latest.deps.ok_or_else(|| flow_error("call argument omits full deps"))?);
            } else if witness && index == 0 {
                point = self.emit(point, Operation::Ownership(OwnershipOp::Dependencies { value: latest.value }))?;
                deps.push(point.result(SlotName::Value)?);
            }
        }
        let call = match &mut op {
            Operation::Call(CallOp::Direct { call, .. } | CallOp::Native { call, .. }
                | CallOp::Intrinsic { call, .. } | CallOp::Closure { call, .. }
                | CallOp::Cap { call, .. } | CallOp::Role { call, .. }) => call,
            _ => unreachable!(),
        };
        call.packet = packet;
        call.deps = deps;
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
            point = if let Some(protocol) = open_protocol {
                self.emit(point, Operation::Ownership(OwnershipOp::Dynamic {
                    state, packet, target: scope, reason: DynamicReason::Closure, protocol,
                }))?
            } else {
                self.emit(point, Operation::Ownership(OwnershipOp::Admit { state, packet }))?
            };
        }
        Ok(point)
    }
}
