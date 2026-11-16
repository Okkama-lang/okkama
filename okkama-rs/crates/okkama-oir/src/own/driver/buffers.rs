//! Builder buffers: the unique buffer and its element journal travel together.
//!
//! `builder.new` starts an empty journal. Every transition consumes the current
//! buffer with its journal and returns both; a checked failure restores both
//! under the High name of the pre-instruction buffer. Finish and abandon return
//! the journal, which is admitted at the lexical owner of the instruction.
//!
//! Inserted elements go to the journal of the builder's owner (spec §4.1, §8):
//! the parts of an inserted value that inner retained scopes hold, admitted or
//! bound (#697), move into the nearest retained scope enclosing the `builder.new`.
//! That scope encloses every transition and the finish, so it outlives the buffer,
//! and it closes where Tomato destroys the collection: a builder made in a loop body
//! releases its elements at the end of each iteration. Parts owned outside stay borrowed.
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
            // #802 (Fable's ruling in OIR-2-async-abi.md): a fresh Future inserted into a builder is stored. Its duty
            // leaves the owner's state in the builder's journal (a split that coro absorbs), so no rehome takes it.
            // A bound Future is an alias (PR-B1): no split names it, and it keeps its token and rehome. A push from a
            // retained scope nested in the builder's owner keeps the `held` transfer of every inserted value (#815).
            let nested = self.nested_push(&point, buffer)?;
            let (futures, moved): (Vec<_>, Vec<_>) = inserted.into_iter().partition(|original| !nested
                && self.high.values.get(*original).is_ok_and(|value| crate::own::futures::is_future(self.module, value.ty)));
            point = self.rehome(point, buffer, &moved)?;
            let mut stored = Vec::new();
            let fresh: Vec<_> = futures.iter().copied().filter(|original| !self.bound_future(*original)).collect();
            for original in &fresh {
                let future = point.env.value(*original)?.value;
                // A failed store split moves the token to its owner's dynamic step (the follow-up ruling), so the
                // owner of the Future's `fut.new` settles entries at run time.
                if let Some((owner, _)) = self.cache_duties.iter().rev().find(|(_, created)| *created == future) {
                    self.dynamic_owners.insert(*owner);
                }
                if !stored.contains(&future) {
                    stored.push(future);
                }
            }
            let mut selected = None;
            if !stored.is_empty() {
                // The store is not an offer that a snapshot may elide: the Future's duty must leave the state.
                let (next, packet) = self.offered_values(point, stored)?;
                point = next;
                selected = Some(packet);
            }
            // Checked transfers forward every live SSA name: rename again.
            op = instruction.op.clone();
            let mut missing = false;
            op.rewrite(&mut Rename { env: &point.env, missing: &mut missing });
            if missing {
                return Err(flow_error("builder operand has no reaching SSA definition"));
            }
            let mut journal = point.env.journals.remove(&buffer)
                .ok_or_else(|| flow_error("builder buffer has no element journal"))?;
            if let Some(selected) = selected {
                point = self.emit(point, Operation::Ownership(OwnershipOp::Join { packets: vec![journal, selected] }))?;
                journal = point.result(SlotName::Packet)?;
            }
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
                let services = consumed.is_some_and(|buffer| self.holds_services(buffer));
                let value = point.result(SlotName::Value)?;
                let journal = point.result(SlotName::Packet)?;
                let original = original.ok_or_else(|| flow_error("builder finish has no High result"))?;
                let mut latest = LatestValue { value, deps: None };
                if self.managed(self.body.values.get(value).map_err(arena)?.ty) {
                    point = self.emit(point, Operation::Ownership(OwnershipOp::Dependencies { value }))?;
                    latest.deps = Some(point.result(SlotName::Value)?);
                }
                point.env.values.insert(original, latest);
                point = self.admit_journal(point, journal, services)?;
            }
            Operation::Collection(CollectionOp::BuilderAbandon { .. } | CollectionOp::MapBuilderAbandon { .. }) => {
                let services = consumed.is_some_and(|buffer| self.holds_services(buffer));
                let journal = point.result(SlotName::Packet)?;
                point = self.admit_journal(point, journal, services)?;
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
            // A Future is stored, never offered: elision does not drop it (#802).
            let future = self.high.values.get(original).is_ok_and(|value| crate::own::futures::is_future(self.module, value.ty));
            if self.elide_offers && !future && !self.may_offer(point, original) {
                continue;
            }
            if !values.contains(&original) {
                values.push(original);
            }
        }
        Ok(values)
    }

    /// Whether a transition of `buffer` runs in a retained scope nested in the builder's owner (a loop body that pushes
    /// into a builder made outside it): its inserted values move by `held` transfers (#697).
    fn nested_push(&self, point: &Landing, buffer: ValueId) -> Result<bool, OwnError> {
        let owner = *self.builder_owners.get(&buffer)
            .ok_or_else(|| flow_error("builder transition has no builder.new owner"))?;
        let Some(outer) = stack(self.module, owner)?.into_iter().rev()
            .find(|scope| point.env.scopes.contains_key(scope)) else {
            return Ok(false);
        };
        let scope = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
        Ok(arrays::inner_scopes(self.module, scope, outer)?.into_iter().any(|scope| point.env.scopes.contains_key(&scope)))
    }

    /// Move what every retained scope inside the builder's owner holds of `originals` into the nearest retained
    /// scope enclosing its `builder.new`: `held(...)` takes the admitted and the bound parts (#697). An element may
    /// borrow an identity a nested scope binds, such as the field of a cursor step bound in the iteration scope;
    /// that identity lives as long as the collection, as in Tomato. An identity outside the source stays with its
    /// owner, and only the source's own parts move: the cursor step itself stays unless the element reaches it.
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
            let selection = self.body.selections.alloc(SelectionExpr::Held(selection)).map_err(arena)?;
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
            // #802: the journal joins the loose packets admitted before lexical unwinding, with its Futures' duties.
            if self.holds_services(buffer) {
                let owner = self.journal_owner(&point)?;
                self.dynamic_owners.insert(owner);
            }
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

    /// The returned journal belongs to the instruction's nearest retained owner. A journal of elements that can hold
    /// a Future carries their duties (#802), so they join that owner's dynamic inventory (D25.1, D25.3).
    fn admit_journal(&mut self, point: Landing, packet: ValueId, services: bool) -> Result<Landing, OwnError> {
        let owner = self.journal_owner(&point)?;
        let state = point.env.state(owner)?;
        if services {
            self.dynamic_owners.insert(owner);
        }
        self.emit(point, Operation::Ownership(OwnershipOp::Admit { state, packet }))
    }

    fn journal_owner(&self, point: &Landing) -> Result<ScopeId, OwnError> {
        let scope = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
        stack(self.module, scope)?.into_iter().rev().find(|scope| point.env.scopes.contains_key(scope))
            .ok_or_else(|| flow_error("builder journal has no retained lexical owner"))
    }

    /// #802: whether the elements of a High builder can hold a service entry (a Future), by the criterion of a call's
    /// output packet (`service_types`).
    fn holds_services(&self, buffer: ValueId) -> bool {
        let Ok(value) = self.high.values.get(buffer) else {
            return false;
        };
        let elements = match self.module.types.get(value.ty) {
            Ok(Type::Builder(element)) => vec![*element],
            Ok(Type::MapBuilder(key, element)) => vec![*key, *element],
            _ => vec![],
        };
        elements.iter().any(|element| self.analysis.types.get(element)
            .is_some_and(|classification| !classification.service_types.is_empty()))
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
