//! Snapshot wrappers preserve the full graph independently of their packet.
use super::*;

impl Driver<'_, '_> {
    pub(super) fn protocol_value(
        &mut self,
        mut point: Landing,
        instruction: &Instruction,
    ) -> Result<Landing, OwnError> {
        let mut op = instruction.op.clone();
        let inputs = match &op {
            Operation::Core(CoreOp::FnClosure { captures, .. }) => captures.clone(),
            Operation::Core(CoreOp::FnBind { receiver, .. } | CoreOp::FnBindRole { receiver, .. }) => vec![*receiver],
            Operation::Core(CoreOp::CapMake { owner, .. }) => vec![*owner],
            Operation::Core(CoreOp::AnyBox { value, .. }) => vec![*value],
            _ => vec![],
        };
        let may_own = instruction.results.iter().any(|value| {
            self.analysis.types[&self.high.values.get(*value).unwrap().ty].may_own()
        });
        let offered = inputs.iter().any(|original| self.may_offer(&point, *original));
        // An action value that captures a resource by reference keeps the capture's obligation with its owner
        // (P2.4 §4.1), yet it is a new object that reaches the resource: it takes an empty packet, so the
        // backend commits it like any construction that reaches a resource.
        let references = matches!(op, Operation::Core(CoreOp::FnClosure { .. } | CoreOp::FnBind { .. }
            | CoreOp::FnBindRole { .. }))
            && inputs.iter().any(|original| self.high.values.get(*original).is_ok_and(|value| {
                self.analysis.types.get(&value.ty).is_some_and(|ty| !ty.reachable_resources.is_empty())
            }));
        let packet = if inputs.is_empty() || !may_own || !offered || point.env.scopes.is_empty() {
            if !inputs.is_empty() && may_own {
                self.emitter.budget_ref().stats.splits_elided += point.env.scopes.len();
            }
            if references && !point.env.scopes.is_empty() {
                point = self.emit(point, Operation::Ownership(OwnershipOp::Empty {}))?;
                Some(point.result(SlotName::Packet)?)
            } else {
                None
            }
        } else {
            let (next, packet) = self.offered(point, &inputs)?;
            point = next;
            Some(packet)
        };
        let mut missing = false;
        op.rewrite(&mut Rename { env: &point.env, missing: &mut missing });
        if missing {
            return Err(flow_error("snapshot operand has no reaching definition"));
        }
        match &mut op {
            Operation::Core(CoreOp::FnClosure { packet: input, .. } | CoreOp::FnBind { packet: input, .. }
                | CoreOp::FnBindRole { packet: input, .. } | CoreOp::CapMake { packet: input, .. }
                | CoreOp::AnyBox { packet: input, .. }) => *input = packet,
            Operation::Core(CoreOp::OnceStore { value, .. }) => {
                let ty = self.body.values.get(*value).map_err(arena)?.ty;
                let certificate = self.analysis.types[&ty].certificate.clone()
                    .ok_or_else(|| flow_error("once.store requires a full-graph plain certificate"))?;
                self.body.values.get_mut(*value).map_err(arena)?.facts = ValueFacts::Plain { certificate };
            }
            _ => {}
        }
        self.finish_construction(point, op, instruction, &[], None)
    }

    pub(super) fn dispatch(
        &mut self,
        point: Landing,
        instruction: &Instruction,
    ) -> Result<Landing, OwnError> {
        let mut op = instruction.op.clone();
        if let Operation::Call(CallOp::Cap { field, call, .. }) = &mut op {
            let DeclarationKind::Cap { fields, .. } = &self.module.declarations.get(field.declaration)
                .map_err(arena)?.kind else { return Err(flow_error("cap call has no computed field")) };
            call.summary = self.module.functions.get(fields[field.index as usize].function).map_err(arena)?.summary;
        }
        self.emit_call(point, instruction, op)
    }
}

/// A reason is selected from a validated family, never from method spelling.
pub(super) fn reason(module: &Module, protocol: ProtocolId) -> Result<DynamicReason, OwnError> {
    match module.protocols.get(protocol).map_err(arena)?.kind {
        ProtocolKind::Atomic => Ok(DynamicReason::Atomic),
        ProtocolKind::Cell => Ok(DynamicReason::Cell),
        ProtocolKind::Channel => Ok(DynamicReason::Channel),
        ProtocolKind::Group => Ok(DynamicReason::Group),
        ProtocolKind::Any => Ok(DynamicReason::Any),
        ProtocolKind::Closure => Ok(DynamicReason::Closure),
        ProtocolKind::Storage | ProtocolKind::Callable | ProtocolKind::Services => {
            Err(flow_error("entry fallback requires a checked dynamic family; P1 never emits Merge"))
        }
    }
}
