//! Builder buffers: the unique buffer and its element journal travel together.
//!
//! `builder.new` starts an empty journal. Every transition consumes the current
//! buffer with its journal and returns both; a checked failure restores both
//! under the High name of the pre-instruction buffer. Finish and abandon return
//! the journal, which is admitted at the lexical owner of the instruction.
//!
//! Inserted elements go to the journal of the builder's owner (spec §4.1, §8):
//! the fresh parts of an inserted value move from inner retained scopes into the
//! nearest retained scope enclosing the `builder.new`. That scope encloses every
//! transition and the finish, so it outlives the buffer, and it closes where
//! Tomato destroys the collection: a builder made in a loop body releases its
//! elements at the end of each iteration. Already owned parts stay borrowed.
use super::*;

impl Driver<'_, '_> {
    pub(super) fn buffer(
        &mut self,
        mut point: Landing,
        instruction: &Instruction,
        mut op: Operation,
    ) -> Result<Landing, OwnError> {
        let consumed = arrays::consumed_buffer(&instruction.op);
        if let Some(buffer) = consumed {
            // Every transition, finish and abandon included, lies inside the builder's owner.
            let owner = *self.builder_owners.get(&buffer)
                .ok_or_else(|| flow_error("builder transition has no builder.new owner"))?;
            let scope = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
            arrays::inner_scopes(self.module, scope, owner)?;
            let inserted = self.inserted_values(&point, &instruction.op)?;
            point = self.rehome(point, buffer, &inserted)?;
            // Checked transfers forward every live SSA name: rename again.
            op = instruction.op.clone();
            let mut missing = false;
            op.rewrite(&mut Rename { env: &point.env, missing: &mut missing });
            if missing {
                return Err(flow_error("builder operand has no reaching SSA definition"));
            }
            let journal = point.env.journals.remove(&buffer)
                .ok_or_else(|| flow_error("builder buffer has no element journal"))?;
            point.env.values.remove(&buffer);
            set_packet(&mut op, journal)?;
        }
        point = self.emit_restoring(point, op.clone(), consumed)?;
        let original = instruction.results.first().copied();
        match op {
            Operation::Collection(CollectionOp::BuilderNew { .. } | CollectionOp::MapBuilderNew { .. }) => {
                let buffer = point.result(SlotName::Buffer)?;
                point = self.emit(point, Operation::Ownership(OwnershipOp::Empty {}))?;
                let journal = point.result(SlotName::Packet)?;
                let original = original.ok_or_else(|| flow_error("builder.new has no High result"))?;
                point.env.values.insert(original, LatestValue { value: buffer, deps: None });
                point.env.journals.insert(original, journal);
            }
            Operation::Collection(CollectionOp::BuilderPush { .. } | CollectionOp::BuilderAppend { .. }
                | CollectionOp::BuilderAppendRange { .. } | CollectionOp::MapBuilderPut { .. }
                | CollectionOp::MapBuilderAppend { .. }) => {
                let buffer = point.result(SlotName::Buffer)?;
                let journal = point.result(SlotName::Packet)?;
                let original = original.ok_or_else(|| flow_error("builder update has no High result"))?;
                point.env.values.insert(original, LatestValue { value: buffer, deps: None });
                point.env.journals.insert(original, journal);
            }
            Operation::Collection(CollectionOp::BuilderFinish { .. } | CollectionOp::MapBuilderFinish { .. }) => {
                let value = point.result(SlotName::Value)?;
                let journal = point.result(SlotName::Packet)?;
                let original = original.ok_or_else(|| flow_error("builder finish has no High result"))?;
                let mut latest = LatestValue { value, deps: None };
                if self.managed(self.body.values.get(value).map_err(arena)?.ty) {
                    point = self.emit(point, Operation::Ownership(OwnershipOp::Dependencies { value }))?;
                    latest.deps = Some(point.result(SlotName::Value)?);
                }
                point.env.values.insert(original, latest);
                point = self.admit_journal(point, journal)?;
            }
            Operation::Collection(CollectionOp::BuilderAbandon { .. } | CollectionOp::MapBuilderAbandon { .. }) => {
                let journal = point.result(SlotName::Packet)?;
                point = self.admit_journal(point, journal)?;
            }
            _ => return Err(flow_error("expected a builder operation")),
        }
        Ok(point)
    }

    /// High names of the managed values a transition inserts.
    fn inserted_values(&self, point: &Landing, operation: &Operation) -> Result<Vec<ValueId>, OwnError> {
        let Operation::Collection(collection) = operation else {
            return Ok(vec![]);
        };
        let originals = arrays::inserted(collection);
        let mut values = vec![];
        for original in originals {
            if !self.managed(self.high.values.get(original).map_err(arena)?.ty) {
                continue;
            }
            if self.elide_offers && !self.may_offer(point, original) {
                continue;
            }
            if !values.contains(&original) {
                values.push(original);
            }
        }
        Ok(values)
    }

    /// Move the admissible parts of `originals` from every retained scope inside
    /// the builder's owner into the nearest retained scope enclosing its `builder.new`.
    fn rehome(&mut self, mut point: Landing, buffer: ValueId, originals: &[ValueId]) -> Result<Landing, OwnError> {
        if originals.is_empty() {
            return Ok(point);
        }
        let owner = *self.builder_owners.get(&buffer)
            .ok_or_else(|| flow_error("builder transition has no builder.new owner"))?;
        let Some(outer) = stack(self.module, owner)?.into_iter().rev()
            .find(|scope| point.env.scopes.contains_key(scope)) else {
            return Ok(point);
        };
        let scope = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
        let inner: Vec<_> = arrays::inner_scopes(self.module, scope, outer)?.into_iter()
            .filter(|scope| point.env.scopes.contains_key(scope)).collect();
        for scope in inner.iter().rev() {
            let mut selections = vec![];
            for original in originals {
                let value = point.env.value(*original)?.value;
                selections.push(self.body.selections.alloc(SelectionExpr::Value(value)).map_err(arena)?);
            }
            let selection = self.body.selections.alloc(SelectionExpr::Union(selections)).map_err(arena)?;
            let selection = self.body.selections.alloc(SelectionExpr::Admissible(selection)).map_err(arena)?;
            let states = TransferStates::Between { from: point.env.state(*scope)?, to: point.env.state(outer)? };
            point = self.emit(point, Operation::Ownership(OwnershipOp::ScopeTransfer { states, selection }))?;
        }
        Ok(point)
    }

    /// A failure that Own introduced (a checked split or a failed close) may
    /// leave a block whose High handler does not receive a live buffer. Such a
    /// buffer is abandoned on the failure path; its journal is then admitted
    /// with the other loose packets before lexical unwinding.
    pub(super) fn abandon_unrouted(
        &mut self,
        mut point: Landing,
        target: Option<&Target>,
    ) -> Result<Landing, OwnError> {
        let buffers: Vec<_> = point.env.journals.keys().copied().filter(|buffer| {
            target.is_none_or(|target| !target.args.contains(buffer))
        }).collect();
        for buffer in buffers {
            let journal = point.env.journals.remove(&buffer).unwrap();
            let value = point.env.values.remove(&buffer)
                .ok_or_else(|| flow_error("live buffer lost its High name on a failure path"))?.value;
            let op = match self.module.types.get(self.body.values.get(value).map_err(arena)?.ty).map_err(arena)? {
                Type::MapBuilder(..) => CollectionOp::MapBuilderAbandon { buffer: value, packet: Some(journal) },
                _ => CollectionOp::BuilderAbandon { buffer: value, packet: Some(journal) },
            };
            point = self.emit(point, Operation::Collection(op))?;
        }
        Ok(point)
    }

    /// The returned journal belongs to the instruction's nearest retained owner.
    fn admit_journal(&mut self, point: Landing, packet: ValueId) -> Result<Landing, OwnError> {
        let scope = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
        let owner = stack(self.module, scope)?.into_iter().rev().find(|scope| point.env.scopes.contains_key(scope))
            .ok_or_else(|| flow_error("builder journal has no retained lexical owner"))?;
        let state = point.env.state(owner)?;
        self.emit(point, Operation::Ownership(OwnershipOp::Admit { state, packet }))
    }
}

fn set_packet(operation: &mut Operation, journal: ValueId) -> Result<(), OwnError> {
    match operation {
        Operation::Collection(CollectionOp::BuilderPush { packet, .. } | CollectionOp::BuilderAppend { packet, .. }
            | CollectionOp::BuilderAppendRange { packet, .. } | CollectionOp::BuilderFinish { packet, .. }
            | CollectionOp::BuilderAbandon { packet, .. } | CollectionOp::MapBuilderPut { packet, .. }
            | CollectionOp::MapBuilderAppend { packet, .. } | CollectionOp::MapBuilderFinish { packet, .. }
            | CollectionOp::MapBuilderAbandon { packet, .. }) => {
            if packet.is_some() {
                return Err(flow_error("High builder transition already names an element packet"));
            }
            *packet = Some(journal);
            Ok(())
        }
        _ => Err(flow_error("expected a builder transition")),
    }
}
