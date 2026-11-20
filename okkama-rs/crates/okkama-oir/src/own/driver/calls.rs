use super::*;
use crate::own::emit::{AdmissionComponent, ScopeExtraction};
use crate::own::futures::{is_future, CACHE_DUTY};

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
            // A fresh Future's duty is a cache duty, never an admission the snapshot knows: elision does not drop it,
            // or the callee would not get it (a group would retain the Future instead of registering it, #810 review
            // F2), as a builder push keeps it (#802).
            let future = self.high.values.get(*original).is_ok_and(|value| crate::own::futures::is_future(self.module, value.ty));
            if self.elide_offers && !future && !self.may_offer(&point, *original) {
                continue;
            }
            // PR-B1 (#784): own never names a bound Future in an admissible selection for a store or an offer.
            if self.bound_future(*original) {
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
        // An offered fresh Future is stored: coro absorbs its token right after its owner's split (D25.2). The splits
        // run from the innermost scope out, so when the owner is not the outermost one, a later split can fail and its
        // unwind returns the selected packet, with the entry, to the owner. That owner settles entries at run time
        // (D25.3), as the owner of a Future pushed into a builder does (#802).
        let outermost = point.env.scopes.keys().next().copied();
        for value in &values {
            if let Some((owner, _)) = self.cache_duties.iter().rev().find(|(_, created)| created == value)
                && Some(*owner) != outermost
            {
                self.dynamic_owners.insert(*owner);
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

    /// PR-B1 (#784): a High Future whose duty a binding of this function marks bound at this point. A binding
    /// marks its duty bound, so no admissible selection takes it: storing or offering it is an alias, and the duty
    /// stays with its owner. The fact is flow-sensitive and read from the analysis (`analyze::transfer_selected`): a
    /// binding adds the bound bit (2) to the cache-duty atoms of the bound value in its scope's journal. It follows
    /// the value through block parameters (a Future that High passes through a branch, `if c { let g = f; }`) and
    /// survives an alias's scope, since the bit sits on the owner's journal entry; a Future carried out of the scope
    /// that holds its duty lands unbound until bound again (`edge_state`). The Future is bound when every cache-duty
    /// atom it depends on is: a fresh Future made from a bound one (`chain(f)`) depends on `f`'s duty too, and stays
    /// fresh. A created Future is bound from its creation (`fut.new … binding`): High binds it right after the call.
    pub(super) fn bound_future(&self, original: ValueId) -> bool {
        if !self.high.values.get(original).is_ok_and(|value| crate::own::futures::is_future(self.module, value.ty)) {
            return false;
        }
        let flow = match self.current_instruction {
            Some(inst) => self.flow.before_instructions.get(&inst),
            None => self.flow.before_edges.get(&self.current_high),
        };
        let Some(state) = flow.and_then(|flow| flow.values.get(original)) else {
            return false;
        };
        let flow = flow.unwrap();
        let mut duties = flow.journals.iter().filter(|((_, atom), quantity)| {
            state.deps.contains(atom)
                && !matches!(quantity.presence, domain::Presence::Bottom | domain::Presence::Absent)
        }).filter_map(|(key, _)| flow.provenance.get(key)).filter(|mask| *mask & CACHE_DUTY != 0).peekable();
        duties.peek().is_some() && duties.all(|mask| mask & 2 != 0)
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
        let entries: Vec<_> = call.summary.and_then(|id| self.module.summaries.get(id).ok()).into_iter()
            .flat_map(|summary| summary.results.iter()).flat_map(|relation| relation.beside.iter().cloned()).collect();
        if !entries.is_empty() && let [result] = instruction.results.as_slice()
            && self.high.values.get(*result).is_ok_and(|value| is_future(self.module, value.ty))
            && let Some((site, receiver)) = crate::own::summaries::call(&instruction.op) {
            let here = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
            let arguments: Vec<_> = receiver.iter().chain(&site.args).map(|argument| argument.value).collect();
            let owners = entries.iter().map(|entry| match entry {
                BesideEntry::Parameter(parameter) => arguments.get(usize::from(*parameter))
                    .and_then(|argument| self.binding_scope(*argument)).unwrap_or(here),
                BesideEntry::Site { .. } => here,
            }).collect();
            self.futures_with_beside.insert(*result, owners);
        }
        self.emit_call(point, instruction, op)
    }

    /// #871: the scope that owns a bound High value: its binding's scope, or the function's root for a parameter.
    fn binding_scope(&self, value: ValueId) -> Option<ScopeId> {
        let bound = self.high.insts.iter().find_map(|(_, instruction)| match &instruction.op {
            Operation::Lexical(LexicalOp::Bind { binding, value: bound }
                | LexicalOp::Rebind { binding, value: bound, .. })
                if *bound == value => self.high.bindings.get(*binding).ok().map(|binding| binding.scope),
            _ => None,
        });
        let parameter = self.high.blocks.get(self.high.entry).ok().filter(|entry| entry.params.contains(&value))
            .map(|entry| entry.lexical_scope);
        bound.or(parameter)
    }

    /// Ruling PR-N2 (#860): whether a role, closure or capability call reaches an async callee, whose call creates a
    /// Future: an async role slot, or a callee of an async function type.
    fn async_callee(&self, op: &Operation) -> bool {
        let async_fn = |ty: TypeId| matches!(self.module.types.get(ty), Ok(Type::Fn(FnType { is_async: true, .. })));
        match op {
            Operation::Call(CallOp::Role { role, slot, .. }) => self.module.declarations.get(*role).is_ok_and(|role| {
                matches!(&role.kind, DeclarationKind::Role { methods, .. }
                    if methods.get(*slot as usize).is_some_and(|method| method.is_async))
            }),
            Operation::Call(CallOp::Closure { callee, .. }) => {
                self.high.values.get(callee.value).is_ok_and(|value| async_fn(value.ty))
            }
            Operation::Call(CallOp::Cap { field, .. }) => self.module.declarations.get(field.declaration)
                .is_ok_and(|declaration| match &declaration.kind {
                    DeclarationKind::Object { fields, .. } => {
                        fields.get(field.index as usize).is_some_and(|field| async_fn(field.ty))
                    }
                    DeclarationKind::Cap { fields, .. } => fields.get(field.index as usize).is_some_and(|field| {
                        self.module.functions.get(field.function)
                            .is_ok_and(|function| function.attrs.contains(&FunctionAttr::Async))
                    }),
                    _ => false,
                }),
            _ => false,
        }
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
        // A sort's comparisons only observe elements of its array: the sort offers no packet, and the
        // runtime gives each owning comparison an empty one (`call.order`).
        let observation = self.module.summaries.get(summary).map_err(arena)?.entry_kind
            == EntryKind::ObservationAdmissionStaysCaller || matches!(op, Operation::Call(CallOp::Order { .. }));
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
        } else if std::mem::take(&mut self.forward_entry) {
            // Ruling PR-M (#818): an adapter's one owning call takes the adapter's entry packet whole: the one linear
            // packet still live, under its current name.
            let own = self.module.types.intern(Type::Service(ServiceType::Own)).map_err(arena)?;
            let packets: Vec<_> = point.env.linear.iter().copied()
                .filter(|value| self.body.values.get(*value).is_ok_and(|data| data.ty == own)).collect();
            let [entry] = packets[..] else {
                return Err(flow_error("an adapter's call takes its one entry packet (PR-M)"));
            };
            Some(entry)
        } else {
            // Ruling PR-N2 (#860): a call of an async callee is a creation; like `fut.new`, it borrows a bare Future
            // argument, which stays with its owner, and offers the rest.
            let creation = self.async_callee(&op);
            // #870, #878 (Fable): an outcome view stays in its Future's cache, which owns the outcome; offering it
            // would extract the cache's holder graph. It is never offered, whatever the parameter's convention: a
            // callee that stores it aliases the view and depends on the cache, as a store of a bound Future does.
            let offered: Vec<_> = originals.iter().copied().filter(|original| {
                let future = self.high.values.get(*original).is_ok_and(|value| is_future(self.module, value.ty));
                !(creation && future) && !self.outcome_view(*original)
            }).collect();
            let (next, packet) = self.offered(point, &offered)?;
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
                | CallOp::Cap { call, .. } | CallOp::Role { call, .. } | CallOp::Order { call, .. }) => call,
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
            // A result that can hold a Future (or another service holder) brings its duties in the
            // packet; they join the scope's dynamic inventory (D25.1, D25.3).
            let services = instruction.results.iter().any(|original| self.high.values.get(*original).ok()
                .and_then(|value| self.analysis.types.get(&value.ty))
                .is_some_and(|classification| !classification.service_types.is_empty()));
            if services {
                self.dynamic_owners.insert(scope);
            }
        }
        Ok(point)
    }
}
