//! Predeclared block-parameter SSA. Parameters remain explicit (including
//! one-predecessor phis); sealing never recursively chases a cyclic definition.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SsaSlot {
    Scope(ScopeId),
    Packet(u32),
    Linear(u32),
    Binding(BindingId),
    Value(ValueId),
    ExitValue,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatestValue {
    pub value: ValueId,
    pub deps: Option<ValueId>,
}
#[derive(Clone, Debug, Default)]
pub struct SealedSsa {
    slots: BTreeMap<BlockId, Vec<(SsaSlot, LatestValue)>>,
    incoming: BTreeMap<BlockId, BTreeMap<(BlockId, usize), Vec<ValueId>>>,
    sealed: BTreeSet<BlockId>,
}
impl SealedSsa {
    pub fn declare(
        &mut self,
        module: &mut Module,
        body: &mut Body,
        block: BlockId,
        slot: SsaSlot,
        ty: TypeId,
        managed: bool,
    ) -> Result<LatestValue, OwnError> {
        if self.sealed.contains(&block) || self.slots.get(&block).is_some_and(|slots| {
            slots.iter().any(|(existing, _)| *existing == slot)
        }) {
            return Err(flow_error("SSA slot is already declared or block sealed"));
        }
        let value = parameter(body, block, ty, ValueFacts::Service)?;
        let deps = if managed {
            let deps_ty = module.types.intern(Type::Service(ServiceType::Deps)).map_err(arena)?;
            let deps = parameter(body, block, deps_ty, ValueFacts::Service)?;
            let expression = body.deps.alloc(DepExpr::FromSsa(deps)).map_err(arena)?;
            body.values.get_mut(value).map_err(arena)?.facts = ValueFacts::Managed { deps: expression };
            Some(deps)
        } else {
            let facts = plain_or_service(module, ty)?;
            body.values.get_mut(value).map_err(arena)?.facts = facts;
            None
        };
        let result = LatestValue { value, deps };
        self.slots.entry(block).or_default().push((slot, result));
        Ok(result)
    }
    pub fn read(&self, block: BlockId, slot: SsaSlot) -> Result<LatestValue, OwnError> {
        self.slots.get(&block).and_then(|slots| slots.iter().find(|(key, _)| *key == slot))
            .map(|(_, value)| *value).ok_or_else(|| flow_error("undeclared SSA slot"))
    }
    /// Edge identity includes source and ordinal, so equal targets remain separate.
    pub fn incoming(
        &mut self,
        body: &Body,
        target: BlockId,
        source: BlockId,
        ordinal: usize,
        values: &BTreeMap<SsaSlot, LatestValue>,
    ) -> Result<Vec<ValueId>, OwnError> {
        if self.sealed.contains(&target) {
            return Err(flow_error("cannot add a predecessor to sealed SSA"));
        }
        let mut args = vec![];
        for (slot, parameter) in self.slots.get(&target).into_iter().flatten() {
            let incoming = values.get(slot).ok_or_else(|| flow_error("missing incoming SSA slot"))?;
            if body.values.get(parameter.value).map_err(arena)?.ty
                != body.values.get(incoming.value).map_err(arena)?.ty {
                return Err(flow_error("incoming SSA slot type mismatch"));
            }
            args.push(incoming.value);
            if parameter.deps.is_some() {
                let deps = incoming.deps.ok_or_else(|| flow_error("managed SSA edge omits full deps"))?;
                args.push(deps);
            }
        }
        if self.incoming.entry(target).or_default().insert((source, ordinal), args.clone()).is_some() {
            return Err(flow_error("duplicate SSA predecessor identity"));
        }
        Ok(args)
    }
    pub fn seal(&mut self, block: BlockId, expected_predecessors: usize) -> Result<(), OwnError> {
        let actual = self.incoming.get(&block).map_or(0, BTreeMap::len);
        if actual != expected_predecessors {
            return Err(flow_error("SSA sealing precedes all predecessors (including back edges)"));
        }
        if !self.sealed.insert(block) {
            return Err(flow_error("SSA block already sealed"));
        }
        Ok(())
    }
}

pub(super) fn parameter(
    body: &mut Body,
    block: BlockId,
    ty: TypeId,
    facts: ValueFacts,
) -> Result<ValueId, OwnError> {
    let index = u32::try_from(body.blocks.get(block).map_err(arena)?.params.len())
        .map_err(|_| flow_error("too many SSA parameters"))?;
    let value = body.values.alloc(ValueData { ty, facts, def: Definition::BlockParam { block, index } })
        .map_err(arena)?;
    body.blocks.get_mut(block).map_err(arena)?.params.push(value);
    Ok(value)
}

pub(super) fn block(body: &mut Body, scope: ScopeId) -> Result<BlockId, OwnError> {
    body.blocks.alloc(Block {
        lexical_scope: scope, params: vec![], unwind: None, insts: vec![],
        term: Terminator { kind: TermKind::Unreachable, loc: None },
    }).map_err(arena)
}

pub(super) fn plain_or_service(module: &Module, ty: TypeId) -> Result<ValueFacts, OwnError> {
    let ty_data = module.types.get(ty).map_err(arena)?;
    if matches!(ty_data, Type::Service(_) | Type::Descriptor | Type::Builder(_) | Type::MapBuilder(..)
        | Type::Frame(_)) {
        return Ok(ValueFacts::Service);
    }
    let certificate = if matches!(ty_data, Type::Int | Type::Bool | Type::Byte | Type::Float | Type::Rat
        | Type::Char | Type::Str | Type::Range | Type::Enum(_) | Type::Never) {
        PlainCertificate::Scalar
    } else {
        PlainCertificate::ClosedGraph { ty }
    };
    Ok(ValueFacts::Plain { certificate })
}
