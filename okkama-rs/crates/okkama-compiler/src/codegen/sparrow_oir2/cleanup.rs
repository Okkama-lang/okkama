//! Relocation of verified Low cleanup evidence. No runtime signatures are invented here.
use super::*;

fn mask(effects: &[o::FailureKind]) -> Result<u32, GenerateError> {
    effects.iter().try_fold(0, |bits, kind| Ok(bits | match kind {
        o::FailureKind::Thrown => 1,
        o::FailureKind::Program => 2,
        o::FailureKind::Panic => 4,
        o::FailureKind::Native => 8,
        _ => return Err(error("cancellation is outside P1")),
    }))
}

pub(super) fn entry_contract(module: &o::Module, f: &o::Function) -> Result<s::EntryContract, GenerateError> {
    let summary = f.summary.map(|id| module.summaries.get(id)).transpose()?;
    let mut retained = summary.into_iter().flat_map(|summary| &summary.results)
        .flat_map(|result| result.external_deps.iter().copied()).collect::<BTreeSet<_>>();
    let mut selections = summary.into_iter().flat_map(|summary| &summary.parameters)
        .filter_map(|parameter| parameter.retention.as_ref())
        .chain(summary.into_iter().flat_map(|summary| &summary.results).flat_map(|result| &result.aliases))
        .collect::<Vec<_>>();
    while let Some(selection) = selections.pop() {
        match selection {
            o::SummarySelection::Parameter(parameter) | o::SummarySelection::OwnerGraph(parameter)
                | o::SummarySelection::Field { parameter, .. } | o::SummarySelection::Path { parameter, .. } => {
                retained.insert(*parameter);
            }
            o::SummarySelection::Union(parts) => selections.extend(parts),
            o::SummarySelection::Descriptor(_) => return Err(error("descriptor summary retention is outside P1")),
        }
    }
    let positions = f.signature.params.iter().enumerate().filter_map(|(index, &ty)| {
        matches!(kind(module, ty), Ok(P::Value(_))).then_some(index as u32)
    }).collect::<Vec<_>>();
    let retained_params = retained.into_iter().map(|parameter| positions.get(usize::from(parameter)).copied()
        .ok_or_else(|| error("summary retention parameter outside signature"))).collect::<Result<_, _>>()?;
    Ok(s::EntryContract {
        retained_params,
        external: f.attrs.iter().find_map(|attr| match attr {
            o::FunctionAttr::Extern(name) | o::FunctionAttr::Intrinsic { name, .. } => Some(name.clone()),
            _ => None,
        }),
        fresh_singleton: summary.is_some_and(|summary| matches!(summary.results.as_slice(), [result]
            if result.fresh.len() == 1 && result.aliases.is_empty()
                && result.provenance == o::AdmissionProvenance::UnboundResult)),
        failures: mask(&summary.map_or(&[][..], |summary| summary.contract.failures.as_slice())
            .iter().copied().filter(|kind| *kind != o::FailureKind::Cancelled).collect::<Vec<_>>())?,
        destroy: f.attrs.iter().find_map(|attr| match attr {
            o::FunctionAttr::Destroy(object) => Some(object.as_u32()),
            _ => None,
        }),
    })
}

pub(super) fn external(module: &o::Module, f: &o::Function) -> Result<s::Function, GenerateError> {
    let contract = entry_contract(module, f)?;
    if contract.external.is_none() {
        return Err(error("bodyless entry requires a registered external contract"));
    }
    let mut function = s::Function {
        symbol: f.symbol.clone(), entry_abi: s::EntryAbi::Observation, registers: vec![], services: vec![],
        params: vec![], results: vec![], blocks: vec![], code: vec![], proofs: s::FunctionProofs::default(),
    };
    function.proofs.entry = Some(contract);
    function.entry_abi = if f.attrs.iter().any(|attr| matches!(attr, o::FunctionAttr::DeferEntry { .. })) {
        s::EntryAbi::Defer { labels: labels(module, f)? }
    } else if matches!(f.signature.ownership_abi, o::OwnershipAbi::Explicit { .. })
        || f.signature.results.iter().any(|&ty| kind(module, ty) == Ok(P::Service(K::Packet)))
    {
        s::EntryAbi::Owned
    } else {
        s::EntryAbi::Observation
    };
    for &ty in &f.signature.params {
        let location = match kind(module, ty)? {
            P::Value(ty) => {
                let location = L::Register(function.registers.len() as u32);
                function.registers.push(ty);
                location
            }
            P::Service(kind) => {
                let location = L::Service(function.services.len() as u32);
                function.services.push(kind);
                location
            }
        };
        function.params.push(location);
    }
    if matches!(function.entry_abi, s::EntryAbi::Owned | s::EntryAbi::Defer { .. })
        && !function.params.iter().any(|&loc| matches!(loc, L::Service(id)
            if function.services[id as usize] == K::Packet))
    {
        function.params.push(L::Service(function.services.len() as u32));
        function.services.push(K::Packet);
    }
    for &ty in &f.signature.results {
        if let P::Value(ty) = kind(module, ty)? {
            function.results.push(ty);
        }
    }
    Ok(function)
}

impl Emitter<'_> {
    fn selection(&mut self, id: o::SelectionId) -> Result<u32, GenerateError> {
        let op = o::Operation::Ownership(o::OwnershipOp::Split {
            packet: o::ValueId::from_u32(0), selection: id,
        });
        let locations = o::cfg::operation_uses(&op, self.body)
            .map_err(|failure| error(failure.to_string()))?.into_iter().skip(1)
            .map(|operand| self.loc(operand.value)).collect::<Result<BTreeSet<_>, _>>()?;
        let id = self.result.proofs.selections.len() as u32;
        self.result.proofs.selections.push(s::Selection { locations: locations.into_iter().collect() });
        Ok(id)
    }

    fn boundary(&self, id: o::BoundaryId) -> Result<u32, GenerateError> {
        Ok(self.body.boundaries.get(id)?.scope.as_u32())
    }

    pub(super) fn ownership(&mut self, op: &o::OwnershipOp) -> Result<(O, Vec<A>), GenerateError> {
        use o::OwnershipOp::*;
        let location = |value| self.loc(value).map(A::Location);
        Ok(match op {
            Bind { binding } | Rebind { binding } => {
                let op = if matches!(op, Bind { .. }) { O::OwnBind } else { O::OwnRebind };
                (op, vec![location(binding.state)?, A::Binding(binding.binding.as_u32()),
                    A::Scope(binding.original_scope.as_u32()), location(binding.resource.value)?,
                    location(binding.resource.packet)?])
            }
            Split { packet, selection } | SplitChecked { packet, selection }
                | ScopeSplit { state: packet, selection } => {
                let op = match op {
                    Split { .. } => O::OwnSplit,
                    SplitChecked { .. } => O::OwnSplitChecked,
                    _ => O::ScopeSplit,
                };
                let packet = location(*packet)?;
                let admissible = matches!(self.body.selections.get(*selection)?, o::SelectionExpr::Admissible(_));
                if admissible && op != O::ScopeSplit {
                    return Err(error("admissible packet selection requires provenance transport"));
                }
                let selection = self.selection(*selection)?;
                let mut arguments = vec![packet, A::Selection(selection)];
                if admissible { arguments.push(A::Flag(true)); }
                (op, arguments)
            }
            Stage { stage } => {
                let mut args = vec![A::Flag(matches!(stage.states, o::TransferStates::Between { .. }))];
                args.extend(stage.states.values().into_iter().map(location).collect::<Result<Vec<_>, _>>()?);
                let field = self.objects.get(stage.field.declaration.as_u32() as usize)
                    .and_then(|object| object.fields.get(stage.field.index as usize))
                    .ok_or_else(|| error("invalid staging field"))?;
                args.push(A::Object(stage.field.declaration.as_u32()));
                args.push(A::Field(field.storage));
                args.push(location(stage.resource.value)?);
                args.push(location(stage.resource.packet)?);
                (O::OwnStage, args)
            }
            ScopeTransfer { states, selection } => {
                let mut arguments = vec![A::Flag(matches!(states, o::TransferStates::Between { .. }))];
                arguments.extend(states.values().into_iter().map(location).collect::<Result<Vec<_>, _>>()?);
                arguments.push(A::Selection(self.selection(*selection)?));
                (O::ScopeTransfer, arguments)
            }
            _ => return Err(later(&format!("ownership operation {op:?} outside synchronous P1 transport"))),
        })
    }

    pub(super) fn cleanup_metadata(&mut self, instructions: &BTreeMap<o::InstId, u32>,
        terms: &BTreeMap<o::BlockId, u32>) -> Result<(), GenerateError>
    {
        for (id, close) in self.body.cleanup.closes.iter() {
            let begin = self.body.insts.iter().find_map(|(inst, data)| match data.op {
                o::Operation::Cleanup(o::CleanupOp::Begin { close, .. }) if close == id => Some(inst),
                _ => None,
            }).ok_or_else(|| error("close has no cleanup.begin"))?;
            let boundary = match close.source {
                o::CleanupSource::Scope(scope) => scope.as_u32(),
                o::CleanupSource::Packet(boundary) => self.boundary(boundary)?,
            };
            self.result.proofs.envelopes.push(s::Envelope { failures: mask(&close.effects.failures)? });
            self.result.proofs.closes.push(s::CloseContract {
                begin_pc: instructions[&begin], boundary, reserve: close.reserve,
                active_failure: close.active_failure, envelope: id.as_u32(),
            });
        }
        for (_, schedule) in self.body.cleanup.schedules.iter() {
            if !schedule.holders.is_empty() || schedule.dictionary.is_some() || schedule.protocol.is_some() {
                return Err(error("nonempty D16 is outside P1"));
            }
            self.result.proofs.schedules.push(s::Schedule { services: vec![] });
        }
        let site = |site: &o::HighOperationSite| match site {
            o::HighOperationSite::Instruction(id) => instructions[id],
            o::HighOperationSite::Terminator(id) => terms[id],
        };
        for (id, plan) in self.body.cleanup.plans.iter() {
            let mut slots = Vec::new();
            let mut preserves = Vec::new();
            let mut joins = Vec::new();
            let mut suffixes = Vec::new();
            let mut reserve_empty = false;
            if let Some(proof) = plan.direct {
                let proof = self.body.cleanup.proofs.get(proof)?;
                reserve_empty = proof.reserve_empty;
                for slot in &proof.slots {
                    let origin = match &slot.origin {
                        o::CreationWitness::Singleton => s::CreationOrigin::Singleton,
                        o::CreationWitness::Create { site: origin } => s::CreationOrigin::Create(site(origin) + 1),
                        o::CreationWitness::FreshResult { site: origin, callee, .. } => {
                            s::CreationOrigin::FreshResult { pc: site(origin), callee: callee.as_u32() }
                        }
                    };
                    slots.push(s::DirectSlot {
                        identity: self.loc(slot.identity)?, descriptor: self.loc(slot.descriptor)?,
                        destroy: slot.destroy.as_u32(), origin, active_pc: self.pcs[&slot.active_at],
                    });
                }
                for preserve in &proof.preserves {
                    preserves.push(s::Dependency {
                        value: self.loc(preserve.to)?, obligations: vec![self.loc(preserve.from)?],
                    });
                }
                for join in &proof.joins {
                    joins.push(s::Dependency { value: self.loc(join.state)?, obligations: join.predecessors.iter()
                        .map(|&value| self.loc(value)).collect::<Result<_, _>>()? });
                }
                for suffix in &proof.suffixes {
                    suffixes.push(s::CleanupSuffix {
                        completed: suffix.completed,
                        remaining: suffix.remaining.iter().map(|&v| self.loc(v)).collect::<Result<_, _>>()?,
                        roots: suffix.roots.iter().map(|&v| self.loc(v)).collect::<Result<_, _>>()?,
                    });
                }
            }
            let selection = self.result.proofs.selections.len() as u32;
            self.result.proofs.selections.push(s::Selection { locations: slots.iter().map(|s| s.identity).collect() });
            let witness = self.result.proofs.order_witnesses.len() as u32;
            self.result.proofs.order_witnesses.push(s::OrderWitness {
                creation_pcs: slots.iter().filter_map(|slot| match slot.origin {
                    s::CreationOrigin::Singleton => None,
                    s::CreationOrigin::Create(pc) | s::CreationOrigin::FreshResult { pc, .. } => Some(pc),
                }).collect(),
            });
            self.result.proofs.plans.push(s::Plan {
                merge: plan.direct.is_none(), selection, order_witness: witness,
            });
            self.result.proofs.cleanup_plans.push(s::CleanupPlan {
                plan: id.as_u32(), reserve: plan.sources.contains(&o::MergeSource::WholeReserveOnFailure),
                close: plan.close.as_u32(), reserve_empty,
                sources: plan.sources.iter().map(|source| match source {
                    o::MergeSource::ScopeInventory => s::MergeSource::Scope,
                    o::MergeSource::PacketInventory => s::MergeSource::Packet,
                    o::MergeSource::WholeReserveOnFailure => s::MergeSource::Reserve,
                }).collect(), slots, preserves, joins, suffixes,
            });
        }
        for (_, root) in self.body.cleanup.roots.iter() {
            if root.through_suspend {
                return Err(error("suspension roots are outside P1"));
            }
            let (pc, after) = match root.boundary {
                o::RootBoundary::BeforeInstruction(id) => (instructions[&id], false),
                o::RootBoundary::AfterInstruction(id) => {
                    match &self.body.insts.get(id)?.op {
                        o::Operation::Core(o::CoreOp::RangeLen { .. }) => (instructions[&id] + 2, true),
                        o::Operation::Lexical(o::LexicalOp::DeferRegister { .. }) => (instructions[&id], false),
                        // An evaluate has no post state: its result exists where the normal outcome lands.
                        op if super::fallible_scalar(op) => {
                            let raise = self.body.blocks.iter().any(|(_, block)| {
                                block.insts.contains(&id) && block.unwind.is_none()
                            });
                            (instructions[&id] + 1 + u32::from(raise), false)
                        }
                        _ => (instructions[&id], true),
                    }
                }
                o::RootBoundary::Terminator(id) => (terms[&id], false),
                o::RootBoundary::Entry(id) => (self.pcs[&id], false),
            };
            let root = s::RootRequirement { pc, locations: root.bindings.iter()
                .map(|binding| self.loc(binding.value)).collect::<Result<_, _>>()? };
            if after {
                self.result.proofs.post_roots.push(root);
            } else {
                self.result.proofs.roots.push(root);
            }
        }
        Ok(())
    }

    pub(super) fn cleanup_instruction(&mut self, op: &o::CleanupOp, results: &[o::ValueId])
        -> Result<s::Instruction, GenerateError>
    {
        use o::CleanupOp::*;
        let loc = |value| self.loc(value).map(A::Location);
        let (op, arguments) = match op {
            Begin { state, pending, failure, close, .. } => {
                let contract = &self.result.proofs.closes[close.as_u32() as usize];
                let mut args = vec![loc(*state)?];
                for &value in pending.iter().chain(failure) {
                    args.push(loc(value)?);
                }
                args.extend([A::Boundary(contract.boundary), A::Envelope(close.as_u32())]);
                (O::CleanupBegin, args)
            }
            SettleEmpty { cleanup, schedule } => {
                (O::CleanupSettle, vec![loc(*cleanup)?, A::Schedule(schedule.as_u32())])
            }
            Plan { cleanup, plan } => (O::CleanupPlan, vec![loc(*cleanup)?, A::Plan(plan.as_u32())]),
            Destroy { cleanup, key } => (O::CleanupDestroy, vec![loc(*cleanup)?, loc(*key)?]),
            DestroyDirect { cleanup, identity, descriptor, destroy } => {
                let ty = self.body.values.get(*identity)?.ty;
                let o::Type::Object { decl, .. } = self.module.types.get(ty)? else {
                    return Err(error("direct identity must be an object"));
                };
                let _descriptor = self.loc(*descriptor)?;
                (O::DestroyDirect, vec![loc(*cleanup)?, loc(*identity)?, A::Object(decl.as_u32()),
                    A::Entry(destroy.as_u32())])
            }
            RecordFailure { cleanup, failure } => (O::CleanupFailure, vec![loc(*cleanup)?, loc(*failure)?]),
            Finish { cleanup } => (O::CleanupFinish, vec![loc(*cleanup)?]),
            _ => return Err(error("unsupported P1 cleanup/service operation")),
        };
        op.cleanup_abi().ok_or_else(|| error("cleanup operation has no shared runtime ABI"))?;
        Ok(s::Instruction::Champ { op, arguments, flow: s::Flow::Next(results.iter()
            .map(|&value| self.loc(value)).collect::<Result<_, _>>()?) })
    }

    fn prepared(&mut self, exit: &o::PreparedExit) -> Result<Vec<A>, GenerateError> {
        let kind = match exit.kind {
            o::ExitKind::Return => s::ExitKind::Return,
            o::ExitKind::Last => s::ExitKind::Last,
            o::ExitKind::Next => s::ExitKind::Next,
            o::ExitKind::Fallthrough => s::ExitKind::Fallthrough,
        };
        let mut args = vec![A::Kind(kind), A::Boundary(self.boundary(exit.boundary)?)];
        if let Some(value) = exit.value {
            args.push(A::Location(self.loc(value)?));
        }
        let packet = exit.packet.ok_or_else(|| error("exit.prepare requires explicit packet"))?;
        args.push(A::Location(self.loc(packet)?));
        args.push(A::Selection(self.selection(exit.selection)?));
        Ok(args)
    }

    pub(super) fn exit_instruction(&mut self, op: &o::ExitOp, results: &[o::ValueId])
        -> Result<s::Instruction, GenerateError>
    {
        use o::ExitOp::*;
        let loc = |value| self.loc(value).map(A::Location);
        let (op, arguments) = match op {
            Prepare { exit } => (O::ExitPrepare, self.prepared(exit)?),
            Extend { pending, packet } => (O::ExitExtend, vec![loc(*pending)?, loc(*packet)?]),
            Abort { pending, cleanup } => (O::ExitAbort, vec![loc(*pending)?, loc(*cleanup)?]),
            Replace { pending, replacement } => {
                let pending = loc(*pending)?;
                let mut args = vec![pending];
                args.extend(self.prepared(replacement)?);
                (O::ExitReplace, args)
            }
            Commit { pending, destination, result } => {
                let pending = loc(*pending)?;
                let boundary = match destination {
                    o::ExitDestination::Caller => s::ExitDestination::Caller,
                    o::ExitDestination::Loop(boundary) => s::ExitDestination::Loop(self.boundary(*boundary)?),
                    o::ExitDestination::Scope(scope) => s::ExitDestination::Scope(scope.as_u32()),
                };
                let expected = result.map(|ty| translate_type(self.module, ty)).transpose()?;
                if boundary == s::ExitDestination::Caller && expected != self.result.results.first().copied() {
                    return Err(error("commit result differs from entry result"));
                }
                let mut arguments = vec![pending, A::Destination(boundary)];
                if boundary != s::ExitDestination::Caller {
                    arguments.push(A::Result(expected));
                }
                (O::ExitCommit, arguments)
            }
        };
        Ok(s::Instruction::Champ { op, arguments, flow: s::Flow::Next(results.iter()
            .map(|&value| self.loc(value)).collect::<Result<_, _>>()?) })
    }

    pub(super) fn cleanup_term(&self, term: &o::TermKind) -> Result<s::Instruction, GenerateError> {
        let loc = |value| self.loc(value).map(A::Location);
        let mut context = s::ResultContext::default();
        let (op, arguments, edges) = match term {
            o::TermKind::CleanupNext { cleanup, selected, done } => (O::CleanupNext, vec![loc(*cleanup)?],
                vec![(s::Tag::Selected, o::Outcome::CleanupSelected, selected),
                    (s::Tag::Done, o::Outcome::CleanupDone, done)]),
            o::TermKind::ExitDispatch { close, closed, failure, exit, success, pending, failed } => {
                let contract = self.body.cleanup.closes.get(*close)?;
                context.reserve = contract.reserve;
                context.active_failure = contract.active_failure;
                context.may_fail = !contract.effects.failures.is_empty();
                let args = vec![loc(*closed)?, loc(*failure)?, loc(*exit)?, A::Flag(context.reserve),
                    A::Flag(context.active_failure), A::Flag(context.may_fail)];
                let edges = [(s::Tag::Success, o::Outcome::ExitSuccess, success),
                    (s::Tag::Pending, o::Outcome::ExitPending, pending),
                    (s::Tag::Failure, o::Outcome::ExitFailure, failed)].into_iter()
                    .filter_map(|(tag, outcome, edge)| edge.as_ref().map(|edge| (tag, outcome, edge))).collect();
                (O::ExitDispatch, args, edges)
            }
            o::TermKind::FailureDispatch { failure, none, some } => {
                let outcomes = vec![
                    s::Outcome { tag: s::Tag::None, edge: self.branch(none)? },
                    s::Outcome { tag: s::Tag::Some, edge: self.edge(some.block, &some.args, o::Outcome::FailureSome,
                        &O::FailureDispatch.results(s::Tag::Some, &context)?)? },
                ];
                return Ok(s::Instruction::Champ { op: O::FailureDispatch, arguments: vec![loc(*failure)?],
                    flow: s::Flow::Outcomes(outcomes) });
            }
            _ => return Err(error("expected cleanup terminator")),
        };
        let outcomes = edges.into_iter().map(|(tag, outcome, edge)| Ok(s::Outcome {
            tag, edge: self.edge(edge.block, &edge.args, outcome, &op.results(tag, &context)?)?,
        })).collect::<Result<_, GenerateError>>()?;
        Ok(s::Instruction::Champ { op, arguments, flow: s::Flow::Outcomes(outcomes) })
    }

    pub(super) fn ownership_invoke(&mut self, op: &o::Operation, normal: &o::InvokeEdge, unwind: &o::InvokeEdge)
        -> Result<s::Instruction, GenerateError>
    {
        let o::Operation::Ownership(op) = op else { return Err(error("expected ownership invoke")) };
        let (op, arguments) = self.ownership(op)?;
        let context = s::ResultContext {
            between: arguments.first() == Some(&A::Flag(true)), ..s::ResultContext::default()
        };
        let outcomes = [(s::Tag::Normal, o::Outcome::Normal, normal),
            (s::Tag::Unwind, o::Outcome::Unwind, unwind)].into_iter().map(|(tag, outcome, edge)| {
                Ok(s::Outcome { tag, edge: self.edge(edge.block, &edge.args, outcome, &op.results(tag, &context)?)? })
            }).collect::<Result<_, GenerateError>>()?;
        Ok(s::Instruction::Champ { op, arguments, flow: s::Flow::Outcomes(outcomes) })
    }

    pub(super) fn construction(&mut self, op: &o::Operation, normal: &o::InvokeEdge)
        -> Result<s::Instruction, GenerateError>
    {
        let (declaration, inputs, packet, construction, op, immediate, ty) = match op {
            o::Operation::Core(o::CoreOp::VariantNew { ty, case: 0, fields, packet }) => {
                let s::Type::Object(id) = translate_type(self.module, *ty)? else {
                    return Err(error("variant construction requires a single plain Error case"));
                };
                (o::DeclId::from_u32(id),
                    fields.iter().map(|&value| self.reg(value)).collect::<Result<Vec<_>, _>>()?,
                    *packet, o::ConstructionStatus::Complete, s::DataOp::ObjNew, s::Immediate::Object(id),
                    s::Type::Object(id))
            }
            o::Operation::Core(o::CoreOp::ObjNew {
                declaration, fields, packet, construction, type_args,
            }) if type_args.is_empty() => (*declaration,
                fields.iter().map(|&value| self.reg(value)).collect::<Result<Vec<_>, _>>()?,
                *packet, *construction, s::DataOp::ObjNew, s::Immediate::Object(declaration.as_u32()),
                s::Type::Object(declaration.as_u32())),
            o::Operation::Core(o::CoreOp::ObjNext { object, updates, packet }) => {
                let o::Type::Object { decl, args } = self.module.types.get(self.body.values.get(*object)?.ty)? else {
                    return Err(error("successor base must be an object"));
                };
                if !args.is_empty() {
                    return Err(error("successor must be monomorphized"));
                }
                let fields = &self.objects[decl.as_u32() as usize].fields;
                let indices = updates.iter().map(|update| {
                    if update.field.declaration != *decl {
                        return Err(error("successor field declaration mismatch"));
                    }
                    fields.get(update.field.index as usize).map(|field| field.storage)
                        .ok_or_else(|| error("invalid successor field"))
                }).collect::<Result<_, _>>()?;
                let mut inputs = vec![self.reg(*object)?];
                inputs.extend(updates.iter().map(|update| self.reg(update.value)).collect::<Result<Vec<_>, _>>()?);
                (*decl, inputs, *packet, o::ConstructionStatus::Complete, s::DataOp::ObjNext,
                    s::Immediate::Updates(indices), s::Type::Successor(decl.as_u32()))
            }
            _ => return Err(error("expected monomorphized object construction")),
        };
        let object = fresh(&mut self.result, ty)?;
        if packet.is_none() {
            let layout = &self.objects[declaration.as_u32() as usize].layout;
            if layout.has_destroy || layout.may_reach_resource {
                return Err(error("resource construction requires an explicit packet"));
            }
            self.result.code.push(s::Instruction::Data { op, inputs, output: Some(object), immediate });
            let schema = [P::Value(ty)];
            let mut edge = self.edge(normal.block, &normal.args, o::Outcome::Normal, &schema)?;
            for assignment in &mut edge.assignments {
                if assignment.source == s::Source::ProducedSlot(0) {
                    assignment.source = s::Source::Location(L::Register(object));
                }
            }
            return Ok(s::Instruction::Br(edge));
        }
        let packet = packet.unwrap();
        let mut service = || {
            let location = L::Service(self.result.services.len() as u32);
            self.result.services.push(K::Packet);
            location
        };
        let committed = service();
        let joined = service();
        self.result.code.push(s::Instruction::Data { op, inputs, output: Some(object), immediate });
        if construction == o::ConstructionStatus::Complete {
            self.result.code.push(s::Instruction::Champ { op: O::OwnCreateCommit,
                arguments: vec![A::Location(L::Register(object)), A::Object(declaration.as_u32()),
                    A::Boundary(self.scope.as_u32())], flow: s::Flow::Next(vec![committed]) });
            self.result.code.push(s::Instruction::Champ { op: O::OwnJoin,
                arguments: vec![A::Location(self.loc(packet)?), A::Location(committed)],
                flow: s::Flow::Next(vec![joined]) });
        } else {
            // Allocation is not a successful identity completion. Only the supplied fields' packet moves.
            self.result.code.push(s::Instruction::Champ { op: O::OwnMove,
                arguments: vec![A::Location(self.loc(packet)?)], flow: s::Flow::Next(vec![committed]) });
            self.result.code.push(s::Instruction::Champ { op: O::OwnMove,
                arguments: vec![A::Location(committed)], flow: s::Flow::Next(vec![joined]) });
        }
        let params = &self.body.blocks.get(normal.block)?.params;
        let assignments = params.iter().zip(&normal.args).map(|(&dst, arg)| {
            let source = match *arg {
                o::EdgeArg::Value(value) => self.loc(value)?,
                o::EdgeArg::Produced(slot) if slot.outcome == o::Outcome::Normal => match slot.slot {
                    0 => L::Register(object),
                    1 => joined,
                    _ => return Err(error("unexpected construction payload")),
                },
                _ => return Err(error("construction outcome mismatch")),
            };
            Ok(s::Assignment { destination: self.loc(dst)?, source: s::Source::Location(source) })
        }).collect::<Result<_, GenerateError>>()?;
        Ok(s::Instruction::Br(s::Edge { target_pc: self.pcs[&normal.block], assignments }))
    }
}

impl Emitter<'_> {
    pub(super) fn finish_evidence(&mut self, instructions: &BTreeMap<o::InstId, u32>,
        terms: &BTreeMap<o::BlockId, u32>) -> Result<(), GenerateError>
    {
        let linear = |location| matches!(location, L::Service(id) if !matches!(
            self.result.services[id as usize], K::Closed | K::Deps | K::DestroyBorrow));
        let mut states = BTreeMap::<u32, BTreeSet<L>>::new();
        let mut incoming = BTreeMap::from([(self.body.entry,
            self.result.params.iter().copied().filter(|&loc| linear(loc)).collect::<BTreeSet<_>>())]);
        let mut work = vec![self.body.entry];
        while let Some(block_id) = work.pop() {
            let block = self.body.blocks.get(block_id)?;
            let mut active = incoming[&block_id].clone();
            for &id in &block.insts {
                let instruction = self.body.insts.get(id)?;
                let pc = instructions[&id];
                states.insert(pc, active.clone());
                let uses = o::cfg::operation_uses(&instruction.op, self.body)
                    .map_err(|e| error(e.to_string()))?;
                for operand in uses {
                    let loc = self.loc(operand.value)?;
                    if linear(loc) && !active.remove(&loc) {
                        return Err(error("Low consumes unavailable service"));
                    }
                }
                for &value in &instruction.results {
                    let loc = self.loc(value)?;
                    if linear(loc) { active.insert(loc); }
                }
                if matches!(instruction.op, o::Operation::Core(o::CoreOp::RangeLen { .. })) {
                    states.insert(pc + 1, active.clone());
                    states.insert(pc + 2, active.clone());
                }
                if super::fallible_scalar(&instruction.op) {
                    // The evaluate consumes no service: its raise block and normal landing see the same state.
                    states.insert(pc + 1, active.clone());
                    if block.unwind.is_none() {
                        states.insert(pc + 2, active.clone());
                    }
                }
            }
            let pc = terms[&block_id];
            states.insert(pc, active.clone());
            let mut uses = Vec::new();
            o::cfg::walk_terminator(block_id, &block.term, self.body, o::ContractRegistry::new(self.module),
                &mut |event| {
                    if let o::cfg::CfgEvent::Use { operand, .. } = event { uses.push(operand); }
                }).map_err(|e| error(e.to_string()))?;
            let returning = matches!(block.term.kind, o::TermKind::Ret { .. } | o::TermKind::RetDefer { .. });
            for operand in uses {
                if returning || operand.mode != o::OperandMode::Forward {
                    let loc = self.loc(operand.value)?;
                    if linear(loc) { active.remove(&loc); }
                }
            }
            let end = self.result.blocks.iter().find(|block| block.pc > pc)
                .map_or(self.result.code.len(), |block| block.pc as usize);
            let last = &self.result.code[end - 1];
            let edges = match last {
                s::Instruction::Br(edge) => vec![edge],
                s::Instruction::Cond { yes, no, .. } => vec![yes, no],
                s::Instruction::Champ { flow: s::Flow::Outcomes(outcomes), .. } => {
                    outcomes.iter().map(|outcome| &outcome.edge).collect()
                }
                _ => vec![],
            };
            if edges.is_empty() && !active.is_empty() {
                return Err(error("Low terminal loses live service state"));
            }
            for edge in edges {
                let mut next = active.clone();
                for assignment in &edge.assignments {
                    if let s::Source::Location(source) = assignment.source && linear(source) {
                        next.remove(&source);
                    }
                }
                for assignment in &edge.assignments {
                    if linear(assignment.destination) { next.insert(assignment.destination); }
                }
                let target = self.pcs.iter().find_map(|(&block, &pc)| (pc == edge.target_pc).then_some(block))
                    .ok_or_else(|| error("Low edge relocation has no block"))?;
                if let Some(previous) = incoming.get(&target) {
                    if *previous != next { return Err(error("Low service states disagree at join")); }
                } else {
                    incoming.insert(target, next);
                    work.push(target);
                }
            }
        }
        // One Low construction expands to allocate/commit/join/branch. The two temporary packets
        // have no Low SSA name, so their states are recorded at the expansion's own boundaries.
        for pc in 0..self.result.code.len().saturating_sub(3) {
            if matches!(self.result.code[pc], s::Instruction::Data { op: s::DataOp::ObjNew | s::DataOp::ObjNext, .. })
                && let s::Instruction::Champ { flow: s::Flow::Next(committed), .. } = &self.result.code[pc + 1]
                && let s::Instruction::Champ { op: O::OwnJoin, flow: s::Flow::Next(joined), .. }
                    = &self.result.code[pc + 2]
            {
                let before = states.get(&(pc as u32)).cloned().unwrap_or_default();
                states.insert(pc as u32 + 1, before.clone());
                let mut during = before;
                during.extend(committed);
                states.insert(pc as u32 + 2, during);
                let mut after = states[&(pc as u32)].clone();
                if let s::Instruction::Champ { arguments, .. } = &self.result.code[pc + 2] {
                    for argument in arguments {
                        if let A::Location(location) = argument { after.remove(location); }
                    }
                }
                after.extend(joined);
                states.insert(pc as u32 + 3, after.clone());
                let mut values = self.result.proofs.roots.iter().filter(|root| root.pc == pc as u32)
                    .flat_map(|root| root.locations.iter().copied()).filter(|loc| matches!(loc, L::Register(_)))
                    .collect::<BTreeSet<_>>();
                if let s::Instruction::Data { output: Some(object), .. } = self.result.code[pc] {
                    values.insert(L::Register(object));
                }
                for offset in 1..=3 {
                    let mut before = values.clone();
                    before.extend(&states[&(pc as u32 + offset)]);
                    self.result.proofs.roots.push(s::RootRequirement {
                        pc: pc as u32 + offset, locations: before.into_iter().collect(),
                    });
                }
                let mut committed_roots = values.clone();
                committed_roots.extend(&states[&(pc as u32 + 2)]);
                self.result.proofs.post_roots.push(s::RootRequirement {
                    pc: pc as u32 + 1, locations: committed_roots.into_iter().collect(),
                });
                values.extend(after);
                self.result.proofs.post_roots.push(s::RootRequirement {
                    pc: pc as u32 + 2, locations: values.into_iter().collect(),
                });
            }
        }
        for pc in 0..self.result.code.len().saturating_sub(1) {
            if let s::Instruction::Data { op: s::DataOp::ObjNew | s::DataOp::ObjNext, output: Some(object), .. }
                = self.result.code[pc]
                && matches!(self.result.code[pc + 1], s::Instruction::Br(_))
            {
                let before = states.get(&(pc as u32)).cloned().unwrap_or_default();
                states.insert(pc as u32 + 1, before.clone());
                let mut roots = self.result.proofs.roots.iter().filter(|root| root.pc == pc as u32)
                    .flat_map(|root| root.locations.iter().copied()).collect::<BTreeSet<_>>();
                roots.extend(before);
                roots.insert(L::Register(object));
                self.result.proofs.roots.push(s::RootRequirement {
                    pc: pc as u32 + 1, locations: roots.into_iter().collect(),
                });
            }
        }
        for (&block_id, &pc) in terms {
            if matches!(self.body.blocks.get(block_id)?.term.kind, o::TermKind::Trap { .. }) {
                states.insert(pc + 1, states.get(&pc).cloned().unwrap_or_default());
                let mut roots = self.result.proofs.roots.iter().filter(|root| root.pc == pc)
                    .flat_map(|root| root.locations.iter().copied()).collect::<BTreeSet<_>>();
                if let s::Instruction::Data { output: Some(payload), .. } = self.result.code[pc as usize] {
                    roots.insert(L::Register(payload));
                }
                self.result.proofs.roots.push(s::RootRequirement {
                    pc: pc + 1, locations: roots.into_iter().collect(),
                });
            }
        }
        for (pc, instruction) in self.result.code.iter().enumerate() {
            if let s::Instruction::Champ { op: O::OwnSplit | O::ExitReplace, arguments, .. } = instruction {
                let selection = arguments.iter().find_map(|arg| match arg {
                    A::Selection(id) => Some(*id), _ => None,
                }).ok_or_else(|| error("missing static Low selection"))?;
                let input = arguments.iter().find_map(|arg| match arg {
                    A::Location(location) => Some(*location), _ => None,
                }).ok_or_else(|| error("missing static input"))?;
                let locations = &self.result.proofs.selections[selection as usize].locations;
                let mut count = 0;
                for &location in locations {
                    if let L::Register(id) = location
                        && let s::Type::Object(object) = self.result.registers[id as usize]
                        && self.objects[object as usize].layout.may_reach_resource
                    {
                        let layout = &self.objects[object as usize].layout;
                        if !layout.has_destroy || layout.storage_fields.iter().any(|ty| {
                            matches!(ty, s::Type::Object(_) | s::Type::Successor(_))
                        }) {
                            return Err(error("static Low selection requires a finite singleton layout"));
                        }
                        count += 1;
                    }
                }
                self.result.proofs.static_selections.push(s::StaticSelection {
                    pc: pc as u32, selection, input, indices: (0..count).collect(),
                });
            }
        }
        self.result.proofs.states = states.into_iter().filter(|(_, locations)| !locations.is_empty())
            .map(|(pc, locations)| s::StateRecord { pc, locations: locations.into_iter().collect() }).collect();
        Ok(())
    }
}
