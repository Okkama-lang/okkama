use super::*;

impl Engine<'_, '_> {
    pub(super) fn graph_filter(
        &mut self, flow: &Flow, keys: BTreeSet<Origin>, labels: SharedSet<ValueId>, fresh: bool,
    ) -> Filter {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("summaries::graph_filter");
        let possible = self.possible(flow, &keys);
        Filter { keys, possible, labels, fresh, state: 0, stale: false, whole_future: false, futures: false }
    }
    /// Ruling PR-N2 (#860): whether a role, closure or capability call reaches an async callee: an async role slot,
    /// a callee of an async function type, or a capability field implemented by an async function.
    fn async_callee(&self, op: &Operation, target: Option<FuncId>) -> bool {
        match op {
            Operation::Call(CallOp::Role { role, slot, .. }) => self.module.declarations.get(*role).is_ok_and(|role| {
                matches!(&role.kind, DeclarationKind::Role { methods, .. }
                    if methods.get(*slot as usize).is_some_and(|method| method.is_async))
            }),
            Operation::Call(CallOp::Closure { callee, .. }) => self.body.values.get(callee.value).is_ok_and(|value| {
                matches!(self.module.types.get(value.ty), Ok(Type::Fn(FnType { is_async: true, .. })))
            }),
            Operation::Call(CallOp::Cap { .. }) => target.and_then(|id| self.module.functions.get(id).ok())
                .is_some_and(|function| function.attrs.contains(&FunctionAttr::Async)),
            _ => false,
        }
    }

    /// Ruling PR-N2 (#860): whether this function is the creation thunk of an async target (an adapter).
    pub(super) fn creation_thunk(&self) -> bool {
        self.declaration.attrs.iter().any(|attr| matches!(attr, FunctionAttr::Adapter { target }
            if self.module.functions.get(*target).is_ok_and(|target| target.attrs.contains(&FunctionAttr::Async))))
    }

    /// #862 (OIR-2-async-remaining §6.3c, ruling (a)): the keys a returned bare Future carries beside its references,
    /// in the exit's selection and its result graph. They are in its full deps but not its references, and belong to a
    /// parameter the summary declares taken into the result. Empty for any other value.
    pub(super) fn future_carried_keys(&mut self, flow: &Flow, value: ValueId) -> BTreeSet<Origin> {
        if !matches!(super::super::later::value_type(self.module, self.body, value), Some(Type::Future(_))) {
            return BTreeSet::new();
        }
        let Some(summary) = self.declaration.summary.and_then(|id| self.module.summaries.get(id).ok()) else {
            return BTreeSet::new();
        };
        let arguments: Vec<_> = self.entry_values().into_iter().filter(|v| {
            self.body.values.get(*v).ok().and_then(|v| self.module.types.get(v.ty).ok())
                .is_some_and(|ty| !matches!(ty, Type::Service(_) | Type::Descriptor))
        }).collect();
        let mut taken = BTreeSet::new();
        for parameter in &summary.parameters {
            if parameter.convention == FreshConvention::Take && parameter.destination == TransferDestination::Result
                && let Some(selection) = &parameter.retention {
                taken.extend(self.summary_selection(flow, &arguments, selection));
            }
        }
        // #865 (OIR-2-async-remaining §6.3d): a creation of this body that the relation declares beside the Future
        // (a binding the Future borrows) leaves beside it too.
        let local = summary.results.first().is_some_and(|relation| {
            relation.beside.iter().any(|entry| matches!(entry, BesideEntry::Site { .. }))
        });
        let index = value.as_u32() as usize;
        flow.full_deps[index].difference(&flow.refs[index]).filter(|key| taken.contains(key) || (local && matches!(key,
            Origin::Instruction(..) | Origin::Invoke(..)))).copied().collect()
    }

    /// #872 (Fable, §6.3d): the `beside` marker of a returned bare Future is checked, not trusted. At the exit, what
    /// the packet exports and the Future does not hold is exactly what the relation declares beside it: creations as
    /// a multiset of types (two creations of one type are interchangeable for the caller), parameters by key; and a
    /// beside parameter is exported and not held by the Future.
    fn beside_checked(
        &mut self, flow: &Flow, value: ValueId, relation: &ResultRelation, values: &[ValueId], arguments: &[ValueId],
        summary: &Summary,
    ) {
        // A Future created here or returned by a call; a parameter returned as it is carries its own entry.
        let Some(future) = flow.identities[value.as_u32() as usize]
            .filter(|origin| !matches!(origin, Origin::External(_) | Origin::Input(_))) else { return };
        let pieces: Vec<_> = values.iter().filter(|value| self.body.values.get(**value).ok()
            .and_then(|data| self.module.types.get(data.ty).ok()) == Some(&Type::Service(ServiceType::Own)))
            .flat_map(|value| flow.tokens[value.as_u32() as usize].pieces.iter().cloned()).collect();
        let free = |piece: &Piece| piece.holder != Some(future) && piece.origin != future;
        let accepted = |piece: &Piece| matches!(piece.origin, Origin::Input(input) if self.body.values.get(input).ok()
            .and_then(|v| self.module.types.get(v.ty).ok()) == Some(&Type::Service(ServiceType::Own)));
        // Creations: the types of the free pieces against the declared sites.
        let mut exported: Vec<Option<TypeId>> = pieces.iter().filter(|piece| free(piece) && !accepted(piece))
            .map(|piece| self.body.values.iter().find(|(id, _)| flow.identities[id.as_u32() as usize]
                == Some(piece.origin)).map(|(_, data)| data.ty))
            .collect();
        let mut declared: Vec<Option<TypeId>> = relation.beside.iter().filter_map(|entry| match entry {
            BesideEntry::Site { ty, .. } => Some(Some(*ty)),
            BesideEntry::Parameter(_) => None,
        }).collect();
        exported.sort();
        declared.sort();
        if exported != declared {
            self.error(R::B51, "a returned Future's packet holds creations beside it other than its relation declares");
        }
        // Parameters: a beside parameter's accepted part is exported and not held; nothing else accepted is free.
        let marked: Vec<u16> = relation.beside.iter().filter_map(|entry| match entry {
            BesideEntry::Parameter(parameter) => Some(*parameter),
            BesideEntry::Site { .. } => None,
        }).collect();
        for parameter in &marked {
            let keys = summary.parameters.get(usize::from(*parameter)).and_then(|effect| effect.retention.as_ref())
                .map(|selection| self.summary_selection(flow, arguments, selection)).unwrap_or_default();
            let carried = pieces.iter().any(|piece| accepted(piece) && free(piece) && !keys.iter().all(|key| {
                piece.predicates.iter().any(|(filter, selected)| !*selected && filter.keys.contains(key))
            }));
            if keys.is_empty() || !carried {
                self.error(R::B51, "a parameter declared beside a returned Future is not exported beside it");
            }
        }
        if marked.is_empty() && pieces.iter().any(|piece| accepted(piece) && free(piece)) {
            self.error(R::B51, "a returned Future's packet holds an accepted input beside it that its relation does \
                not declare");
        }
        // #873 (Fable): the creations the Future holds, typed, against the types its relation declares held. Compared
        // as sets: one site of a caller brings every type a callee's cache holds, so pieces and entries need not pair.
        let mut held = BTreeSet::new();
        for piece in pieces.iter().filter(|piece| !free(piece) && !accepted(piece)) {
            match self.held_piece_types(flow, piece, future) {
                Some(types) => held.extend(types.into_iter().map(Some)),
                None => {
                    held.insert(None);
                }
            }
        }
        let declared: BTreeSet<_> = relation.held.iter().map(|entry| Some(entry.ty)).collect();
        if held != declared {
            self.error(R::B51, "a returned Future's cache holds creations other than its relation declares");
        }
    }

    /// #876 (Fable): a result that stores Futures declares, by type, what their caches hold: the creations of this
    /// body that a cache holds, and what the fresh families of calls bring at their sites (the callees' entries).
    /// Compared as sets, as for a bare Future (#873).
    fn stored_held_checked(&mut self, flow: &Flow, relation: &ResultRelation, values: &[ValueId]) {
        let pieces: Vec<_> = values.iter().filter(|value| self.body.values.get(**value).ok()
            .and_then(|data| self.module.types.get(data.ty).ok()) == Some(&Type::Service(ServiceType::Own)))
            .flat_map(|value| flow.tokens[value.as_u32() as usize].pieces.iter().cloned()).collect();
        let mut held = BTreeSet::new();
        for piece in &pieces {
            if matches!(piece.origin, Origin::Input(_)) {
                continue;
            }
            // As for a bare Future: a creation of this body that a cache holds brings its value's type; a piece of a
            // call's fresh family brings what the callee declares held at its site; anything else held is unknown.
            let cached = piece.holder.is_some_and(|holder| holder != piece.origin);
            let direct = cached.then(|| self.body.values.iter()
                .find(|(id, _)| flow.identities[id.as_u32() as usize] == Some(piece.origin))
                .map(|(_, data)| data.ty)).flatten();
            match (direct, self.call_held_types(piece.origin)) {
                (Some(ty), _) => {
                    held.insert(Some(ty));
                }
                (None, Some(types)) => held.extend(types.into_iter().map(Some)),
                (None, None) if cached => {
                    held.insert(None);
                }
                (None, None) => {}
            }
        }
        let declared: BTreeSet<_> = relation.held.iter().map(|entry| Some(entry.ty)).collect();
        if held != declared {
            self.error(R::B51, "a returned value's caches hold creations other than its relation declares");
        }
    }

    /// #876: what a call's relations declare held at the fresh site of `origin`, when a call created it.
    fn call_held_types(&self, origin: Origin) -> Option<Vec<TypeId>> {
        let (op, site) = match origin {
            Origin::Instruction(inst, site) => (&self.body.insts.get(inst).ok()?.op, site),
            Origin::Invoke(block, site) => match &self.body.blocks.get(block).ok()?.term.kind {
                TermKind::InvokeOwn { op, .. } => (&**op, site),
                _ => return None,
            },
            _ => return None,
        };
        let (call, _, _) = super::super::summaries::call(op)?;
        let summary = self.module.summaries.get(call.summary?).ok()?;
        Some(summary.results.iter().flat_map(|relation| &relation.held)
            .filter(|entry| entry.site.as_u32() == site).map(|entry| entry.ty).collect())
    }

    /// #873 (Fable): the types a piece that a returned Future holds brings into its cache. A creation of this body
    /// brings its value's type. A piece of a call's fresh family brings what the callee's relation declares held at
    /// its site, which the callee's exit checked. The Future's own identity brings only that, or nothing when this
    /// body created it: the inputs of `fut.new` are pieces of their own. `None` when nothing accounts for the piece.
    ///
    /// #876: a caller names a family's identity by its first fresh site, which need not be the Future's own (in a
    /// callee that creates a `Res` before the Future, it is the `Res`'s). The piece at the Future's own site then has
    /// neither a value nor a held entry, and brings no types: that is not a missing case.
    fn held_piece_types(&self, flow: &Flow, piece: &Piece, future: Origin) -> Option<Vec<TypeId>> {
        if piece.origin != future && let Some((_, data)) = self.body.values.iter()
            .find(|(id, _)| flow.identities[id.as_u32() as usize] == Some(piece.origin)) {
            return Some(vec![data.ty]);
        }
        let (op, site) = match piece.origin {
            Origin::Instruction(inst, site) => (&self.body.insts.get(inst).ok()?.op, site),
            Origin::Invoke(block, site) => match &self.body.blocks.get(block).ok()?.term.kind {
                TermKind::InvokeOwn { op, .. } => (&**op, site),
                _ => return None,
            },
            _ => return None,
        };
        let Some((call, _, _)) = super::super::summaries::call(op) else {
            return (piece.origin == future && matches!(op, Operation::Async(AsyncOp::FutureNew { .. })))
                .then(Vec::new);
        };
        let summary = self.module.summaries.get(call.summary?).ok()?;
        Some(summary.results.iter().flat_map(|relation| &relation.held)
            .filter(|entry| entry.site.as_u32() == site).map(|entry| entry.ty).collect())
    }

    pub(super) fn call_entry(&mut self, op: &Operation, before: &Flow, _: &mut Flow) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("summaries::call_entry");
        if self.module.profile == Profile::High {
            return;
        }
        let Some((call, receiver, target)) = super::super::summaries::call(op) else {
            return;
        };
        let Some(summary) = call.summary.and_then(|id| self.module.summaries.get(id).ok()) else {
            return;
        };
        let target = target.or_else(|| super::super::summaries::cap_target(self.module, op));
        // Ruling PR-N2 (#860): a call of an async callee is a creation, which borrows a bare Future argument (OIR-2
        // §10.2), as `fut.new` does: the Future and its cache duty stay with their owner. The callee declares it
        // borrowed, and the offered packet holds no piece of its duty (in coro and low a duty reaches a packet
        // through `service.absorb`).
        if self.async_callee(op, target) {
            let futures: Vec<_> = call.args.iter().filter(|argument| {
                matches!(super::super::later::value_type(self.module, self.body, argument.value),
                    Some(Type::Future(_)))
            }).collect();
            if futures.iter().any(|argument| argument.convention == OperandConvention::Consume) {
                self.error(R::B91, "a call of an async callee consumes no bare Future: it borrows it (PR-N2)");
            }
            let keys: Vec<Origin> = futures.iter()
                .filter_map(|argument| before.identities[argument.value.as_u32() as usize]).collect();
            if let Some(packet) = call.packet && before.tokens[packet.as_u32() as usize].pieces.iter()
                .any(|piece| piece.holder.is_some_and(|holder| keys.contains(&holder))) {
                self.error(R::B91, "a call of an async callee offers no bare Future's duty in its packet (PR-N2)");
            }
        }
        self.call_deps(call, receiver, target, before);
        self.descriptor_arguments(call, target, before);
        if let Some(function) = target.and_then(|id| self.module.functions.get(id).ok()) {
            let own = |ty: &&TypeId| self.module.types.get(**ty) == Ok(&Type::Service(ServiceType::Own));
            let inputs = function.signature.params.iter().filter(own).count();
            let outputs = function.signature.results.iter().filter(own).count();
            if inputs > 1 || outputs > 1 {
                self.error(R::B51, "ordinary call cannot transport multiple packet ABI slots");
            }
            if function.body.is_some() && inputs == 0 && call.packet.is_some_and(|packet| {
                !before.tokens[packet.as_u32() as usize].pieces.is_empty()
            }) {
                self.error(R::B54, "fresh inputs need an explicit callee packet and cleanup entry");
            }
        }
        if summary.entry_kind == EntryKind::ObservationAdmissionStaysCaller {
            if call.packet.is_some() {
                self.error(R::B52, "observation must keep admission in its caller");
            }
            return;
        }
        let defer_entry = target.and_then(|id| self.module.functions.get(id).ok()).is_some_and(|function| {
            function.attrs.iter().any(|attr| matches!(attr, FunctionAttr::DeferEntry { .. }))
        });
        if summary.entry_kind == EntryKind::Internal && !defer_entry {
            return;
        }
        let arguments: Vec<_> = receiver.iter().chain(&call.args).map(|arg| arg.value).collect();
        let graph: BTreeSet<_> = arguments.iter().flat_map(|v| before.refs[v.as_u32() as usize].iter().copied())
            .collect();
        let labels: SharedSet<_> = arguments.iter().flat_map(|v| before.contained[v.as_u32() as usize].iter().copied())
            .collect();
        if let Some(packet) = call.packet {
            for piece in &before.tokens[packet.as_u32() as usize].pieces {
                if piece.provenance & !FRESH != 0 {
                    self.error(R::B53, "call entry can accept only the offered fresh portion");
                }
                if !self.subset(piece, &graph, &labels) {
                    self.error(R::B51, "offered packet is not a subset of the call argument retention graph");
                }
            }
        }
        // Arguments that can hold a Future reach its holder graph whole (D25.2 revision, #771).
        let futures = arguments.iter().any(|value| {
            self.body.values.get(*value).is_ok_and(|data| self.holds_futures(data.ty))
        });
        for value in before.token_values() {
            if !self.scan_work(1) {
                return;
            }
            if Some(value) == call.packet || before.availability[value.as_u32() as usize] != AVAILABLE {
                continue;
            }
            let mut filter = self.graph_filter(before, graph.clone(), labels.clone(), true);
            filter.futures = futures;
            // #870 (Fable): when no argument can hold a Future, what a cache holds is no input of this call. An
            // argument that is a view of a cache's outcome reaches the cache's key, not its holder graph, which stays
            // with the Future; scanning it here would read as an extraction (B91).
            let mut token = before.tokens[value.as_u32() as usize].clone();
            if !futures {
                token.pieces.retain(|piece| piece.holder.is_none());
            }
            let (_, selected) = self.partition_filter(&token, filter);
            if !selected.pieces.is_empty() {
                self.error(R::B51, "fresh call inputs must enter callee cleanup, including borrow parameters");
            }
        }
    }
    fn call_deps(&mut self, call: &CallSite, receiver: Option<CallArgument>, target: Option<FuncId>, flow: &Flow) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("summaries::call_deps");
        let arguments: Vec<_> = receiver.iter().chain(&call.args).map(|argument| argument.value).collect();
        let formal = target.and_then(|id| self.module.functions.get(id).ok());
        let count = formal.map_or(0, |function| function.signature.params.iter().filter(|ty| {
            self.module.types.get(**ty) == Ok(&Type::Service(ServiceType::Deps))
        }).count());
        if count != 0 && call.deps.len() != count {
            self.error(R::B51, "call does not provide its declared deps ABI parameters");
        }
        if count == 0 && call.deps.is_empty() {
            return;
        }
        let supplied: BTreeSet<_> = call.deps.iter().flat_map(|value| {
            flow.refs[value.as_u32() as usize].iter().copied()
        }).collect();
        let required: BTreeSet<_> = arguments.iter().flat_map(|value| {
            flow.full_deps[value.as_u32() as usize].iter().copied()
        }).collect();
        if !required.is_subset(&supplied) {
            self.error(R::B32, "call deps ABI omits part of an argument's full dependencies");
        }
        // A native/indirect ABI without bodies uses the merged full graph. For
        // a checked body, each paired parameter additionally has its own check.
        let Some(body) = formal.and_then(|function| function.body.as_ref()) else {
            return;
        };
        let Ok(entry) = body.blocks.get(body.entry) else {
            return;
        };
        let parameters: Vec<_> = entry.params.iter().copied().filter(|value| {
            body.values.get(*value).ok().and_then(|value| self.module.types.get(value.ty).ok())
                == Some(&Type::Service(ServiceType::Deps))
        }).collect();
        let language: Vec<_> = entry.params.iter().copied().filter(|value| {
            body.values.get(*value).ok().and_then(|value| self.module.types.get(value.ty).ok())
                .is_some_and(|ty| !matches!(ty, Type::Service(_) | Type::Descriptor))
        }).collect();
        for (parameter, argument) in language.iter().zip(arguments) {
            let Ok(ValueData { facts: ValueFacts::Managed { deps }, .. }) = body.values.get(*parameter) else {
                continue;
            };
            let Ok(DepExpr::FromSsa(dep)) = body.deps.get(*deps) else {
                continue;
            };
            let supplied = parameters.iter().position(|parameter| parameter == dep)
                .and_then(|index| call.deps.get(index));
            if supplied.is_none_or(|value| {
                !flow.full_deps[argument.as_u32() as usize].is_subset(&flow.refs[value.as_u32() as usize])
            }) {
                self.error(R::B32, "paired call deps parameter does not carry its value's full graph");
            }
        }
    }
    // Spec §4.2 defines inclusion by retained identities, not container SSA names.
    // Current keys identify the same immutable graph across field projections.
    // Retired families can denote several incarnations, so they also need labels.
    pub(super) fn subset(&self, piece: &Piece, graph: &BTreeSet<Origin>, labels: &SharedSet<ValueId>) -> bool {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("summaries::subset");
        if !piece.family && graph.contains(&piece.origin) {
            return true;
        }
        // What a Future's cache holds (its inputs or outcome) is reachable from the Future (D25.1).
        if piece.holder.is_some_and(|holder| graph.contains(&holder)) {
            return true;
        }
        // A retired filter without labels names no particular incarnation.
        piece.predicates.iter().any(|(filter, selected)| {
            *selected && !filter.stale && filter.keys.is_subset(graph)
                && (filter.keys.iter().all(|key| !matches!(key,
                    Origin::RetiredInstruction(..) | Origin::RetiredInvoke(..) | Origin::RetiredProjection(..)
                    | Origin::RetiredViewInstruction(..) | Origin::RetiredViewInvoke(..)))
                    || (!filter.labels.is_empty() && filter.labels.is_subset(labels)))
        })
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn call_results(
        &mut self, call: &CallSite, receiver: Option<CallArgument>, origin: Origin, before: &Flow,
        flow: &mut Flow, schema: &OutcomeContract, slots: &mut [Slot],
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("summaries::call_results");
        let Some(summary) = call.summary.and_then(|id| self.module.summaries.get(id).ok()) else {
            return;
        };
        let arguments: Vec<_> = receiver.iter().chain(&call.args).map(|a| a.value).collect();
        let language: Vec<_> = schema.results.iter().enumerate().filter(|(_, slot)| {
            !matches!(slot.ty, ResultType::Service(_))
        }).map(|(i, _)| i).collect();
        let internal = summary.entry_kind == EntryKind::Internal;
        let fresh = summary.results.iter().any(|relation| !relation.fresh.is_empty());
        let state = summary.results.first().map_or(AdmissionProvenance::Bound, |relation| relation.provenance);
        let mixed = summary.results.iter().any(|relation| relation.provenance != state);
        let mut fresh_states = std::collections::BTreeMap::new();
        let mut all_refs = BTreeSet::new();
        let mut all_labels = SharedSet::new();
        let mut fresh_refs: std::collections::BTreeMap<CreationSiteId, BTreeSet<Origin>> =
            std::collections::BTreeMap::new();
        let opaque_sites: BTreeSet<_> = summary.results.iter().filter(|relation| relation.opaque)
            .flat_map(|relation| relation.fresh.iter().copied()).collect();
        // D25.1: a returned Future brings its duty ServiceOf{result, Cache} as a fresh family, keyed by
        // the result itself. Its fresh pieces are the Future's cache graph, held by that key.
        let mut held_sites = std::collections::BTreeMap::new();
        for (index, relation) in summary.results.iter().enumerate() {
            let Some(&slot) = language.get(index) else {
                continue;
            };
            let future = match schema.results[slot].ty {
                ResultType::Exact(ty) => matches!(self.module.types.get(ty), Ok(Type::Future(_))),
                ResultType::FutureOf(_) => true,
                _ => false,
            };
            // #865 (OIR-2-async-remaining §6.3d): what the packet holds beside the Future is not its cache graph.
            let beside = beside_sites(relation);
            if future && summary.results.len() == 1 && !relation.opaque
                && let Some(&first) = relation.fresh.iter().find(|site| !beside.contains(site)) {
                let key = creation(origin, first);
                held_sites.extend(relation.fresh.iter().filter(|site| !beside.contains(site)).map(|site| (*site, key)));
            }
        }
        for (index, relation) in summary.results.iter().enumerate() {
            let Some(&slot) = language.get(index) else {
                continue;
            };
            let mut aliases = BTreeSet::new();
            let mut labels = SharedSet::new();
            for alias in &relation.aliases {
                aliases.extend(self.summary_selection(before, &arguments, alias));
                labels.extend(self.summary_labels(before, &arguments, alias));
            }
            // A declared opaque relation (an open callable) never identifies its
            // result with an input alias or a single fresh site (#569 O1).
            let beside = beside_sites(relation);
            let held = relation.fresh.iter().find(|site| !beside.contains(site))
                .and_then(|site| held_sites.get(site)).copied();
            let opaque = relation.opaque || relation.aliases.len() > 1 || summary.results.len() > 1
                || relation.fresh.len() > 1;
            if let Some(key) = held {
                slots[slot].refs = aliases;
                slots[slot].refs.insert(key);
                slots[slot].contained = labels.clone();
                slots[slot].identity = Some(key);
                flow.cache_keys.insert(key);
                // #865 (§6.3d): the Future depends on what its packet holds beside it; a creation beside it is a
                // piece of the admitted packet that only this dependency names. The verifier's half of #871's
                // fail-closed rule (`own::tests::asynchronous::a_future_cannot_outlive_a_creation_beside_it`).
                slots[slot].requirements.extend(beside.iter().map(|site| creation(origin, *site)));
            } else if opaque {
                let view = match origin {
                    Origin::Instruction(inst, _) => Origin::ViewInstruction(inst, slot as u16),
                    Origin::Invoke(block, _) => Origin::ViewInvoke(block, slot as u16),
                    _ => origin,
                };
                let mut candidates = aliases.clone();
                candidates.extend(relation.fresh.iter().map(|site| creation(origin, *site)));
                flow.views.insert(view, candidates);
                slots[slot].refs.insert(view);
            } else {
                slots[slot].refs = aliases;
                slots[slot].contained = labels.clone();
                if relation.fresh.is_empty() && let [SummarySelection::Parameter(parameter)] = &relation.aliases[..]
                    && let Some(value) = arguments.get(usize::from(*parameter)) {
                    slots[slot].aliases = before.aliases[value.as_u32() as usize].clone();
                    slots[slot].identity = before.identities[value.as_u32() as usize];
                }
                if !relation.fresh.is_empty() {
                    let key = creation(origin, relation.fresh[0]);
                    slots[slot].refs.insert(key);
                    slots[slot].identity = Some(key);
                }
            }
            // The external dependency positions classify dependencies from the
            // selected aliases. They do not retain an entire input container.
            for parameter in &relation.external_deps {
                let mentioned = relation.aliases.iter().any(|selection| summary_mentions(selection, *parameter));
                if !mentioned && let Some(value) = arguments.get(usize::from(*parameter)) {
                    slots[slot].requirements.extend(&before.full_deps[value.as_u32() as usize]);
                }
            }
            for site in &relation.fresh {
                if let Some(old) = fresh_states.insert(*site, relation.provenance) && old != relation.provenance {
                    self.error(R::B51, "one fresh creation site cannot have two result provenance states");
                }
                fresh_refs.entry(*site).or_default().extend(&slots[slot].refs);
            }
            all_refs.extend(&slots[slot].refs);
            all_labels.extend(labels);
        }
        let mut output = if internal {
            call.packet.map(|p| before.tokens[p.as_u32() as usize].clone()).unwrap_or_default()
        } else { Token::default() };
        if !internal && let Some(packet) = call.packet {
            let mut permitted = BTreeSet::new();
            let mut labels = SharedSet::new();
            for parameter in &summary.parameters {
                if parameter.convention == FreshConvention::Take
                    && parameter.destination == TransferDestination::Result
                    && let Some(selection) = &parameter.retention {
                    permitted.extend(self.summary_selection(before, &arguments, selection));
                    labels.extend(self.summary_labels(before, &arguments, selection));
                }
            }
            // A Future argument, or one stored in an argument, takes its whole holder graph along (D25.2, #771).
            let futures = arguments.iter().any(|value| {
                self.body.values.get(*value).is_ok_and(|data| self.holds_futures(data.ty))
            });
            let mut filter = self.graph_filter(flow, permitted, labels, false);
            filter.futures = futures;
            let (denied, allowed) = self.partition_filter(&before.tokens[packet.as_u32() as usize], filter);
            // #866 (§6.3c-d): a parameter declared beside a returned Future comes back in the packet although no
            // result refers to it (the Future only depends on it).
            let mut returned = all_refs.clone();
            let mut returned_labels = all_labels.clone();
            for entry in summary.results.iter().flat_map(|relation| &relation.beside) {
                if let BesideEntry::Parameter(parameter) = entry
                    && let Some(selection) = summary.parameters.get(usize::from(*parameter))
                        .and_then(|effect| effect.retention.as_ref()) {
                    returned.extend(self.summary_selection(before, &arguments, selection));
                    returned_labels.extend(self.summary_labels(before, &arguments, selection));
                }
            }
            let mut filter = self.graph_filter(flow, returned, returned_labels, false);
            filter.futures = futures;
            let (unused, selected) = self.partition_filter(&allowed, filter);
            let settled: Vec<_> = denied.pieces.union(&unused.pieces).cloned().collect();
            let live = flow.clone();
            self.invalidate(&live, flow, &settled);
            for &slot in &language {
                let refs: BTreeSet<_> = slots[slot].refs.union(&slots[slot].requirements).copied().collect();
                let aliases = slots[slot].aliases.clone();
                if self.result_invalidated(flow, &refs, &aliases, &settled) {
                    slots[slot].lifetime = 2;
                    self.error(R::B51, "call summary returns a borrow of an input settled by callee cleanup");
                }
            }
            for piece in &settled {
                flow.dead.insert(piece.origin);
            }
            flow.closed_inputs.extend(settled);
            output = selected;
            output.pieces = output.pieces.into_iter().map(|mut piece| {
                let provenance = if mixed {
                    let mut states = 0u8;
                    let token = Token { pieces: BTreeSet::from([piece.clone()]), ..Token::default() };
                    for (index, relation) in summary.results.iter().enumerate() {
                        let Some(&slot) = language.get(index) else {
                            continue;
                        };
                        let filter = self.graph_filter(flow, slots[slot].refs.clone(),
                            slots[slot].contained.clone(), false);
                        let (_, selected) = self.partition_filter(&token, filter);
                        if !selected.pieces.is_empty() {
                            states |= provenance_bit(relation.provenance);
                        }
                    }
                    if states.count_ones() != 1 {
                        self.error(R::B51, "transferred input has ambiguous result provenance");
                    }
                    states
                } else { provenance_bit(state) };
                piece.provenance = provenance;
                if provenance & !FRESH == 0 {
                    // Publication is a new caller admission boundary. Moving,
                    // aliasing or rehoming inside a boundary cannot do this.
                    piece.owner = None;
                    piece.journal = false;
                }
                piece
            }).collect();
            // #872 (OIR-2-async-remaining §6.3d): a parameter taken into a returned bare Future that the relation does
            // not mark beside it went into the Future's inputs (a creation thunk forwards it): its cache holds it.
            if let Some(&key) = held_sites.values().next() {
                let beside: BTreeSet<u16> = summary.results.iter().flat_map(|relation| &relation.beside)
                    .filter_map(|entry| match entry {
                        BesideEntry::Parameter(parameter) => Some(*parameter),
                        BesideEntry::Site { .. } => None,
                    }).collect();
                let mut keys = BTreeSet::new();
                let mut labels = SharedSet::new();
                for (index, parameter) in summary.parameters.iter().enumerate() {
                    if parameter.convention == FreshConvention::Take
                        && parameter.destination == TransferDestination::Result
                        && !beside.contains(&(index as u16)) && let Some(selection) = &parameter.retention {
                        keys.extend(self.summary_selection(before, &arguments, selection));
                        labels.extend(self.summary_labels(before, &arguments, selection));
                    }
                }
                if !keys.is_empty() {
                    let filter = self.graph_filter(flow, keys, labels, false);
                    let (rest, held) = self.partition_filter(&output, filter);
                    output.pieces = rest.pieces;
                    output.pieces.extend(held.pieces.into_iter().map(|mut piece| {
                        piece.holder = Some(key);
                        piece.provenance = provenance_bit(AdmissionProvenance::CacheOwned);
                        piece
                    }));
                }
            }
        }
        for (site, state) in fresh_states {
            let key = creation(origin, site);
            let mut piece = Piece::new(key, state);
            piece.family = summary.results.len() > 1;
            if let Some(holder) = held_sites.get(&site) {
                // The returned Future's cache graph stays cache-owned in the caller (OIR-2 §10.2).
                piece.holder = Some(*holder);
                piece.provenance = provenance_bit(AdmissionProvenance::CacheOwned);
                output.pieces.insert(piece);
                flow.provenance.insert(key, provenance_bit(state));
                flow.dead.remove(&key);
                continue;
            }
            // A sync callee lets its fresh graph escape only through its results,
            // and its return check proves exactly that: the exported packet lies in
            // the union of the result graphs. So the fresh piece of an opaque (or
            // multi-result) call is the part reachable from every result naming
            // its site, never from the last one alone. The labels are the results'
            // own, added when they are defined, not the aliased inputs' (#569 O1).
            if piece.family || opaque_sites.contains(&site) {
                let filter = self.graph_filter(flow, fresh_refs.remove(&site).unwrap_or_default(),
                    SharedSet::new(), false);
                piece.predicates.insert(filter, true);
            }
            output.pieces.insert(piece);
            flow.provenance.insert(key, provenance_bit(state));
            flow.dead.remove(&key);
        }
        if let Some(slot) = schema.results.iter().position(|s| s.name == SlotName::Packet) {
            slots[slot].token = output;
        } else if fresh && self.module.profile != Profile::High {
            self.error(R::B23, "fresh call result has no explicit ownership packet");
        }
        if let Some(slot) = schema.results.iter().position(|s| s.name == SlotName::Deps) {
            slots[slot].refs = all_refs;
            let required: BTreeSet<_> = slots.iter().flat_map(|slot| slot.requirements.iter().copied()).collect();
            slots[slot].refs.extend(required);
        }
    }
    fn summary_labels(&mut self, flow: &Flow, args: &[ValueId], root: &SummarySelection) -> SharedSet<ValueId> {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("summaries::summary_labels");
        let mut labels = SharedSet::new();
        let mut pending = vec![root];
        while let Some(selection) = pending.pop() {
            if !self.state.step(1) {
                break;
            }
            match selection {
                SummarySelection::Union(items) => pending.extend(items),
                SummarySelection::Parameter(i) | SummarySelection::OwnerGraph(i) | SummarySelection::Descriptor(i) => {
                    if let Some(value) = args.get(usize::from(*i)) {
                        labels.extend(&flow.contained[value.as_u32() as usize]);
                    }
                }
                SummarySelection::Field { .. } | SummarySelection::Path { .. } => {}
            }
        }
        labels
    }
    fn summary_selection(&mut self, flow: &Flow, args: &[ValueId], root: &SummarySelection) -> BTreeSet<Origin> {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("summaries::summary_selection");
        let mut selected = BTreeSet::new();
        let mut pending = vec![root];
        while let Some(selection) = pending.pop() {
            if !self.state.step(1) {
                break;
            }
            let index = match selection {
                SummarySelection::Parameter(i) | SummarySelection::OwnerGraph(i) | SummarySelection::Descriptor(i)
                => *i,
                SummarySelection::Path { parameter, fields } => {
                    if let Some(value) = args.get(usize::from(*parameter)) {
                        if fields.is_empty() {
                            selected.extend(&flow.refs[value.as_u32() as usize]);
                        } else if fields.len() == 1 {
                            selected.extend(self.field(flow, *value, fields[0]));
                        } else if fields.len() <= 4 {
                            let mut keys = BTreeSet::from([self.identity(flow, *value)]);
                            let mut known = true;
                            for (index, field) in fields.iter().enumerate() {
                                let mut next = BTreeSet::new();
                                for key in keys {
                                    if let Some((refs, identity)) = flow.fields.get(&(
                                        key, field.declaration, field.index,
                                    )) {
                                        if index + 1 < fields.len() {
                                            next.extend(identity);
                                        } else {
                                            next.extend(refs);
                                        }
                                    } else {
                                        known = false;
                                    }
                                }
                                keys = next;
                            }
                            if known {
                                selected.extend(keys);
                            } else {
                                let mut path = [(DeclId::from_u32(0), 0); 4];
                                for (slot, field) in path.iter_mut().zip(fields) {
                                    *slot = (field.declaration, field.index);
                                }
                                selected.insert(Origin::Projection(*value, ProjectionKind::Path {
                                    fields: path, len: fields.len() as u8,
                                }));
                            }
                        }
                    }
                    continue;
                }
                SummarySelection::Field { parameter, field } => {
                    if let Some(value) = args.get(usize::from(*parameter)) {
                        selected.extend(self.field(flow, *value, *field));
                    }
                    continue;
                }
                SummarySelection::Union(items) => {
                    pending.extend(items);
                    continue;
                }
            };
            if let Some(value) = args.get(usize::from(index)) {
                selected.extend(&flow.refs[value.as_u32() as usize]);
            }
        }
        selected
    }
    pub(super) fn body_summary(&mut self, flow: &Flow, normal: bool, values: &[ValueId]) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("summaries::body_summary");
        if self.module.profile == Profile::High {
            return;
        }
        let Some(summary) = self.declaration.summary.and_then(|id| self.module.summaries.get(id).ok()) else {
            return;
        };
        if self.body.blocks.get(self.body.entry).is_err() {
            return;
        }
        // C-2: a D21 async body has its entry values in its start continuation.
        let parameters = self.entry_values();
        let arguments: Vec<_> = parameters.iter().copied().filter(|v| {
            self.body.values.get(*v).ok().and_then(|v| self.module.types.get(v.ty).ok())
                .is_some_and(|ty| !matches!(ty, Type::Service(_) | Type::Descriptor))
        }).collect();
        if !normal {
            let allowed = summary.contract.failures.iter().fold(0, |mask, kind| mask | failure_bit(*kind));
            if flow.active_failure & !allowed != 0 {
                self.error(R::B51, "body exposes a failure kind absent from its declared summary");
            }
            if parameters.iter().any(|v| linear(self.module, self.body, *v)
                && flow.availability[v.as_u32() as usize] == AVAILABLE) {
                self.error(R::B54, "failure/cancel exit has not settled accepted inputs");
            }
            return;
        }
        // A D21 async body publishes its output packet (C-2).
        if summary.entry_kind == EntryKind::Internal
            && (self.frame_body() || self.declaration.signature.results.iter().any(|ty| {
                self.module.types.get(*ty) == Ok(&Type::Service(ServiceType::Own))
            })) {
            for &parameter in &parameters {
                if self.body.values.get(parameter).ok().and_then(|v| self.module.types.get(v.ty).ok())
                    != Some(&Type::Service(ServiceType::Own)) { continue; }
                if self.empty_ctor_packet(parameter) {
                    continue;
                }
                let origin = Origin::Input(parameter);
                let exported = values.iter().any(|value| {
                    flow.tokens[value.as_u32() as usize].pieces.iter().any(|p| p.origin == origin)
                });
                if !exported || flow.dead.contains(&origin) {
                    self.error(R::B51, "internal packet ABI must preserve input contents on normal return");
                }
            }
        }
        let language: Vec<_> = values.iter().copied().filter(|v| self.body.values.get(*v).ok()
            .and_then(|v| self.module.types.get(v.ty).ok()).is_some_and(|ty| !matches!(ty, Type::Service(_))))
            .collect();
        if summary.entry_kind == EntryKind::CallBoundary {
            let graph = language.iter().flat_map(|value| flow.refs[value.as_u32() as usize].iter().copied())
                .collect();
            let output_labels = language.iter().flat_map(|value| {
                flow.contained[value.as_u32() as usize].iter().copied()
            }).collect();
            for value in values {
                if self.body.values.get(*value).ok().and_then(|value| self.module.types.get(value.ty).ok())
                    != Some(&Type::Service(ServiceType::Own)) { continue; }
                for piece in &flow.tokens[value.as_u32() as usize].pieces {
                    if !self.subset(piece, &graph, &output_labels) {
                        self.error(R::B51, "body exports obligations outside its result retention graph");
                    }
                }
            }
            let mut permitted = BTreeSet::new();
            let mut labels = SharedSet::new();
            for parameter in &summary.parameters {
                if parameter.convention == FreshConvention::Take
                    && parameter.destination == TransferDestination::Result
                    && let Some(selection) = &parameter.retention {
                    permitted.extend(self.summary_selection(flow, &arguments, selection));
                    labels.extend(self.summary_labels(flow, &arguments, selection));
                }
            }
            // #700, #731 (OIR-2 §7.2, §10.2; P2.4 §4.8): a borrowed reference that holds no obligation may reach
            // a result, stored in it or as it, when the result declares its external deps. Its graph is the
            // caller's: storage does not become ownership, so no take retention names it.
            for parameter in summary.results.iter().flat_map(|relation| &relation.external_deps) {
                let borrowed = summary.parameters.get(usize::from(*parameter))
                    .is_some_and(|effect| effect.convention == FreshConvention::Borrow);
                let obligation = arguments.get(usize::from(*parameter))
                    .and_then(|value| self.body.values.get(*value).ok())
                    .is_none_or(|value| self.obligations[value.ty.as_u32() as usize]);
                if borrowed && !obligation {
                    let selection = SummarySelection::OwnerGraph(*parameter);
                    permitted.extend(self.summary_selection(flow, &arguments, &selection));
                    labels.extend(self.summary_labels(flow, &arguments, &selection));
                }
            }
            // PR-P (#749): a result may keep a carrier-free field of a parameter (no obligation, no Future
            // reachable, D25.1) when its relation declares that field as an alias. The declaration admits the
            // export of the piece the field names, which carries nothing; the shape of the piece alone never does.
            // The caller gets the field's deps through the alias.
            for relation in &summary.results {
                let mut pending: Vec<&SummarySelection> = relation.aliases.iter().collect();
                while let Some(alias) = pending.pop() {
                    let last = match alias {
                        SummarySelection::Union(items) => {
                            pending.extend(items);
                            continue;
                        }
                        SummarySelection::Field { field, .. } => Some(*field),
                        SummarySelection::Path { fields, .. } => fields.last().copied(),
                        _ => None,
                    };
                    if last.is_some_and(|field| self.carrier_free_field(field)) {
                        permitted.extend(self.summary_selection(flow, &arguments, alias));
                        labels.extend(self.summary_labels(flow, &arguments, alias));
                    }
                }
            }
            // PR-P §5.2 (#749, #789): a fresh result's own keys. Its creation keys select nothing of the accepted
            // input, which never holds a later creation (`packets.rs`, partition_filter); what the fresh object
            // keeps of the input is named by its other keys, and each of those must still be permitted. A view key
            // joins only when every key it may stand for is permitted or is such a creation key.
            let mut creations = BTreeSet::new();
            let mut views = BTreeSet::new();
            for (value, relation) in language.iter().zip(&summary.results) {
                if relation.fresh.is_empty() {
                    continue;
                }
                for key in &flow.refs[value.as_u32() as usize] {
                    match key {
                        // A Future's key is the holder of the inputs its cache takes (D25.1): admitting it would admit
                        // those inputs without a declared retention.
                        Origin::Instruction(..) if self.future_origin(*key) => {}
                        Origin::Instruction(..) | Origin::Invoke(..) => {
                            creations.insert(*key);
                        }
                        Origin::RetiredInstruction(..) | Origin::RetiredInvoke(..) => {
                            creations.insert(*key);
                            labels.extend(&flow.contained[value.as_u32() as usize]);
                        }
                        Origin::ViewInstruction(..) | Origin::ViewInvoke(..) => {
                            views.insert(*key);
                        }
                        _ => {}
                    }
                }
            }
            permitted.extend(&creations);
            for view in views {
                let stands_for = view_candidates(flow, view);
                if stands_for.iter().all(|key| permitted.contains(key) || creation_key(*key)) {
                    permitted.insert(view);
                }
            }
            // Ruling PR-N2 (#860, OIR-2-async-remaining §6.3b): a creation thunk exports its entry packet whole inside
            // the Future it returns. The packet holds only the fresh part of the arguments that may own, and no bare
            // Future, which its callers borrow (B9.1 at `call_entry`), so it is permitted whole when each of those
            // arguments is declared taken whole into the result: a field retention does not cover the entry piece.
            let thunk = self.creation_thunk() && arguments.iter().enumerate().all(|(index, value)| {
                let ty = self.body.values.get(*value).map(|value| value.ty);
                ty.is_ok_and(|ty| !self.obligations[ty.as_u32() as usize]
                    || matches!(self.module.types.get(ty), Ok(Type::Future(_))))
                    || summary.parameters.get(index).is_some_and(|effect| {
                    effect.convention == FreshConvention::Take && effect.destination == TransferDestination::Result
                        && effect.retention.as_ref().is_some_and(|retention| retains_whole(retention, index))
                })
            });
            for &value in values {
                for piece in &flow.tokens[value.as_u32() as usize].pieces {
                    let accepted = match piece.origin {
                        Origin::Input(input) => self.body.values.get(input).ok()
                            .and_then(|v| self.module.types.get(v.ty).ok())
                            == Some(&Type::Service(ServiceType::Own)),
                        _ => false,
                    };
                    if accepted && !thunk && !self.subset(piece, &permitted, &labels) {
                        self.error(R::B51, "body exports accepted inputs beyond its declared take retention");
                    }
                }
            }
            // #862 (V2, OIR-2-async-remaining §6.3c, narrowed): the converse, for what a result depends on. When an
            // exported value's references or full deps reach the keys of a parameter declared taken into the result,
            // some exported piece of the accepted input still holds that parameter's part: a result does not depend on
            // an accepted input the callee closes. A parameter that only may leave (`pick`, `maybe`) is unaffected.
            let reached: BTreeSet<_> = language.iter().flat_map(|value| {
                flow.refs[value.as_u32() as usize].iter().chain(&flow.full_deps[value.as_u32() as usize]).copied()
            }).collect();
            for (index, parameter) in summary.parameters.iter().enumerate() {
                if parameter.convention != FreshConvention::Take
                    || parameter.destination != TransferDestination::Result {
                    continue;
                }
                // A bare Future parameter brings no entry of its own (PR-B2): its duty does not travel as a piece of
                // the accepted input. #866 (Fable): unless the relation declares it beside a returned Future, which
                // hands back whatever entry the caller offered, as for any other parameter.
                let beside = summary.results.iter().flat_map(|relation| &relation.beside)
                    .any(|entry| *entry == BesideEntry::Parameter(index as u16));
                if !beside && arguments.get(index).is_some_and(|value| {
                    matches!(super::super::later::value_type(self.module, self.body, *value), Some(Type::Future(_)))
                }) {
                    continue;
                }
                let Some(selection) = &parameter.retention else { continue };
                let keys = self.summary_selection(flow, &arguments, selection);
                if keys.is_empty() || keys.is_disjoint(&reached) {
                    continue;
                }
                let pieces = values.iter().flat_map(|value| flow.tokens[value.as_u32() as usize].pieces.iter());
                let exported = pieces.into_iter().any(|piece| {
                    let accepted = matches!(piece.origin, Origin::Input(input) if self.body.values.get(input).ok()
                        .and_then(|v| self.module.types.get(v.ty).ok()) == Some(&Type::Service(ServiceType::Own)));
                    // The piece lies in the intersection of its selected filters' graphs: it holds the parameter's
                    // part unless rejected filters cover those keys, each key by one of them (Fable's review: one
                    // filter covering them all is too generous), and when it is restricted, one of its selected
                    // filters names one of them.
                    let excluded = keys.iter().all(|key| piece.predicates.iter().any(|(filter, selected)| {
                        !*selected && filter.keys.contains(key)
                    }));
                    let mut chosen = piece.predicates.iter().filter(|(_, selected)| **selected).peekable();
                    let named = chosen.peek().is_none() || chosen.any(|(filter, _)| !filter.keys.is_disjoint(&keys));
                    accepted && !excluded && named
                });
                if !exported {
                    self.error(R::B51, "a parameter declared taken into the result stays in the callee");
                }
            }
        }
        for (value, relation) in language.iter().zip(&summary.results) {
            // #865 (OIR-2-async-remaining §6.3d): only a bare Future result has entries beside it.
            let future = matches!(super::super::later::value_type(self.module, self.body, *value),
                Some(Type::Future(_)));
            if !relation.beside.is_empty() && !future {
                self.error(R::B51, "only a bare Future result declares entries beside it");
            }
            let holds = self.body.values.get(*value).is_ok_and(|data| self.holds_futures(data.ty));
            if !relation.held.is_empty() && !holds {
                self.error(R::B51, "only a result that can hold a Future declares creations a cache holds");
            }
            if future && summary.entry_kind == EntryKind::CallBoundary {
                self.beside_checked(flow, *value, relation, values, &arguments, summary);
            } else if holds && summary.entry_kind == EntryKind::CallBoundary {
                self.stored_held_checked(flow, relation, values);
            }
            if summary.entry_kind == EntryKind::CallBoundary {
                let keys = flow.refs[value.as_u32() as usize].clone();
                let labels = flow.contained[value.as_u32() as usize].clone();
                for output in values {
                    let mut filter = self.graph_filter(flow, keys.clone(), labels.clone(), false);
                    // A returned Future carries its cache duty and the packets its cache holds (C-4):
                    // this check reads the whole graph and extracts nothing. They stay cache-owned, and
                    // the caller admits them as the Future's fresh family (D25.1).
                    filter.whole_future = true;
                    let (_, selected) = self.partition_filter(&flow.tokens[output.as_u32() as usize], filter);
                    if selected.pieces.iter().any(|piece| piece.holder.is_none()
                        && piece.provenance != provenance_bit(relation.provenance)) {
                        self.error(R::B51, "body result provenance differs from its declared summary");
                    }
                }
            }
            let mut aliases = BTreeSet::new();
            for alias in &relation.aliases {
                aliases.extend(self.summary_selection(flow, &arguments, alias));
            }
            let mut allowed = aliases.clone();
            for param in &relation.external_deps {
                if let Some(value) = arguments.get(usize::from(*param)) {
                    allowed.extend(&flow.full_deps[value.as_u32() as usize]);
                }
            }
            // An opaque callee result stays opaque in every relation that retains
            // it, else callers would give it one identity and direct references.
            let retained = flow.refs[value.as_u32() as usize].clone();
            let mut reachable = self.possible(flow, &retained);
            reachable.extend(retained);
            if !relation.opaque && reachable.into_iter().any(|key| self.opaque_call_view(key)) {
                self.error(R::B51, "body returns an opaque call result under a non-opaque summary relation");
            }
            let refs = &flow.full_deps[value.as_u32() as usize];
            // Fields do not retain their container (§4.1). Owner-dependent
            // views must expose their carried dependencies to summary checks (§4.2/§6.2).
            let mut keys = refs.clone();
            // A view exposes what it may stand for, through nested views; a projection among those is checked as
            // a projection below, not unfolded to its container (PR-P, #749: §4.1, a field does not retain its
            // container).
            for view in refs.iter().copied().filter(|key| matches!(key,
                Origin::ViewInstruction(..) | Origin::ViewInvoke(..)
                | Origin::RetiredViewInstruction(..) | Origin::RetiredViewInvoke(..))) {
                keys.extend(view_candidates(flow, view));
            }
            for key in keys {
                if allowed.contains(&key) {
                    continue;
                }
                match key {
                    Origin::External(_) => {
                        self.error(R::B51, "body returns an alias/dependency absent from its declared summary");
                    }
                    Origin::Instruction(..) | Origin::Invoke(..) | Origin::RetiredInstruction(..)
                    | Origin::RetiredInvoke(..) if relation.fresh.is_empty() => {
                        self.error(R::B51, "body returns a fresh identity absent from its declared summary");
                    }
                    Origin::Projection(..) | Origin::RetiredProjection(..) => {
                        let possible = self.possible(flow, &BTreeSet::from([key]));
                        if possible.is_empty() || !possible.is_subset(&allowed) {
                            self.error(R::B51, "body returns an undeclared projection of an input");
                        }
                    }
                    _ => {}
                }
            }
            if let Some(identity @ Origin::External(_)) = flow.identities[value.as_u32() as usize]
                && !aliases.contains(&identity) {
                self.error(R::B51, "body aliases an input without a declared result alias");
            }
        }
    }
}

impl Engine<'_, '_> {
    /// The view of a call result whose summary relation declares it opaque.
    /// PR-P (#749): whether a field's static type is carrier-free: no obligation (`obligation_types`) and no
    /// reachable Future (`holds_futures`, D25.1), so a packet can carry nothing through it.
    fn carrier_free_field(&self, field: FieldRef) -> bool {
        let Ok(DeclarationKind::Object { fields, .. }) = self.module.declarations.get(field.declaration)
            .map(|declaration| &declaration.kind) else {
            return false;
        };
        fields.get(field.index as usize).is_some_and(|field| {
            !self.obligations.get(field.ty.as_u32() as usize).copied().unwrap_or(true)
                && !self.holds_futures(field.ty)
        })
    }
    /// PR-P (#749): whether settling `settled` invalidates a call result with these keys. A view stands for one of
    /// its direct candidates (`view_candidates`): a piece invalidates it only when the piece does not exclude the
    /// view itself and invalidates one of the candidates. A projection of a carrier-free field needs nothing alive:
    /// no obligation is reachable through it, and a settled piece ends only obligations (P2.4 §4.8).
    fn result_invalidated(&mut self, flow: &Flow, keys: &BTreeSet<Origin>, aliases: &SharedSet<ValueId>,
        settled: &[Piece]) -> bool {
        let mut direct = BTreeSet::new();
        for &key in keys {
            if self.carrier_free_projection(key) {
                continue;
            }
            let retired = matches!(key, Origin::RetiredViewInstruction(..) | Origin::RetiredViewInvoke(..));
            if !retired && !matches!(key, Origin::ViewInstruction(..) | Origin::ViewInvoke(..)) {
                direct.insert(key);
                continue;
            }
            // A view with no recorded candidate is checked as itself.
            let recorded = view_candidates(flow, key);
            if recorded.is_empty() {
                direct.insert(key);
                continue;
            }
            let candidates: BTreeSet<_> = recorded.into_iter()
                .filter(|candidate| !self.carrier_free_projection(*candidate)).collect();
            for piece in settled {
                let excluded = piece.predicates.iter().any(|(filter, selected)| {
                    !*selected && !filter.fresh && filter.keys.contains(&key)
                        && (!retired || (!filter.stale && !filter.labels.is_disjoint(aliases)))
                });
                if !excluded && self.keys_invalidated(flow, &candidates, aliases, std::slice::from_ref(piece)) {
                    return true;
                }
            }
        }
        self.keys_invalidated(flow, &direct, aliases, settled)
    }
    /// PR-P (#749): whether a key is a projection whose last field is carrier-free.
    fn carrier_free_projection(&self, key: Origin) -> bool {
        let field = match key {
            Origin::Projection(_, ProjectionKind::Field(declaration, index)) => Some((declaration, index)),
            Origin::Projection(_, ProjectionKind::Path { fields, len }) => {
                usize::from(len).checked_sub(1).and_then(|last| fields.get(last).copied())
            }
            _ => None,
        };
        field.is_some_and(|(declaration, index)| self.carrier_free_field(FieldRef { declaration, index }))
    }
    fn opaque_call_view(&self, key: Origin) -> bool {
        let (op, slot) = match key {
            Origin::ViewInstruction(inst, slot) | Origin::RetiredViewInstruction(inst, slot) => {
                let Ok(inst) = self.body.insts.get(inst) else {
                    return false;
                };
                (&inst.op, slot)
            }
            Origin::ViewInvoke(block, slot) | Origin::RetiredViewInvoke(block, slot) => {
                let Ok(block) = self.body.blocks.get(block) else {
                    return false;
                };
                let TermKind::InvokeOwn { op, .. } = &block.term.kind else {
                    return false;
                };
                (op.as_ref(), slot)
            }
            _ => return false,
        };
        let Some((call, _, _)) = super::super::summaries::call(op) else {
            return false;
        };
        call.summary.and_then(|id| self.module.summaries.get(id).ok())
            .and_then(|summary| summary.results.get(usize::from(slot)))
            .is_some_and(|relation| relation.opaque)
    }
}

fn summary_mentions(root: &SummarySelection, parameter: u16) -> bool {
    let mut pending = vec![root];
    while let Some(selection) = pending.pop() {
        match selection {
            SummarySelection::Union(items) => pending.extend(items),
            SummarySelection::Parameter(i) | SummarySelection::OwnerGraph(i) | SummarySelection::Descriptor(i)
            | SummarySelection::Path { parameter: i, .. }
            | SummarySelection::Field { parameter: i, .. } if *i == parameter => return true,
            _ => {}
        }
    }
    false
}

/// PR-P §5.2: what a view key may stand for, through nested views only. A projection, a creation or an external
/// key is a candidate itself: unlike `possible`, a projection is not unfolded to its base.
fn view_candidates(flow: &Flow, view: Origin) -> BTreeSet<Origin> {
    let mut candidates = BTreeSet::new();
    let mut seen = BTreeSet::from([view]);
    let mut pending = vec![view];
    while let Some(key) = pending.pop() {
        for &candidate in flow.views.get(&key).into_iter().flatten() {
            if matches!(candidate, Origin::ViewInstruction(..) | Origin::ViewInvoke(..)
                | Origin::RetiredViewInstruction(..) | Origin::RetiredViewInvoke(..)) {
                if seen.insert(candidate) {
                    pending.push(candidate);
                }
            } else {
                candidates.insert(candidate);
            }
        }
    }
    candidates
}

/// A key of a creation in this body: no accepted input holds it.
fn creation_key(key: Origin) -> bool {
    matches!(key, Origin::Instruction(..) | Origin::Invoke(..) | Origin::RetiredInstruction(..)
        | Origin::RetiredInvoke(..))
}

/// Ruling PR-N2 (#860): whether `retention` keeps parameter `index` whole, alone or in a union.
fn retains_whole(retention: &SummarySelection, index: usize) -> bool {
    match retention {
        SummarySelection::Parameter(parameter) => *parameter as usize == index,
        SummarySelection::Union(items) => items.iter().any(|item| retains_whole(item, index)),
        _ => false,
    }
}

/// #865 (OIR-2-async-remaining §6.3d): the fresh sites a relation declares beside its returned Future.
fn beside_sites(relation: &ResultRelation) -> BTreeSet<CreationSiteId> {
    relation.beside.iter().filter_map(|entry| match entry {
        BesideEntry::Site { site, .. } => Some(*site),
        BesideEntry::Parameter(_) => None,
    }).collect()
}
