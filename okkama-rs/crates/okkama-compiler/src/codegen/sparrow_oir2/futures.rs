//! Async in SPB 17 (#601 V7, R4): async bodies with their D21 frame layout, Future creation, await and its
//! unwind adapter, publication, the suspension brackets and cancellation requests; with resources, the temporary
//! take with its proof and the rehome of an escaping Future's cache duty. The forms follow the async ABI
//! (C-1…C-4, D20, D21) and Champ draft 3 §14; the loader rules of §15 check what this emits.
use super::*;

/// What SPB 17 takes from an async body besides its code and layout. A function is an async body when it carries
/// `[async]`: it then needs both async summaries, its frame layout and its own frame as its only parameter.
pub(super) struct AsyncBody {
    /// The Future's outcome value; `None` for a void Future.
    pub outcome: Option<s::Type>,
    /// An async defer entry (C-6): `invoke.defer` runs it as a shielded child, it has no Future of its own.
    pub defer: bool,
}

/// The async body shape of `function`, `None` for a synchronous function. A layout or an async summary outside
/// an async body, and an async body without its layout, are rejected.
pub(super) fn async_body(module: &o::Module, id: o::FuncId, function: &o::Function)
    -> Result<Option<AsyncBody>, GenerateError>
{
    let asynchronous = function.attrs.contains(&o::FunctionAttr::Async);
    let layout = function.body.as_ref().and_then(|body| body.frame_layout.as_ref());
    if !asynchronous {
        if layout.is_some() || function.async_creation_summary.is_some() || function.async_body_summary.is_some() {
            return Err(error(format!("{}: async metadata on a synchronous function", function.symbol)));
        }
        return Ok(None);
    }
    if function.body.is_none() {
        // An async intrinsic is a driver of the trusted catalogue (D22, #601 Q1): `cleanup::external` declares it.
        if function.attrs.iter().any(|attr| matches!(attr, o::FunctionAttr::Intrinsic { .. })) {
            return Ok(None);
        }
        return Err(later(&format!("{}: async native (D22, slice N1)", function.symbol)));
    }
    if layout.is_none() {
        return Err(error(format!("{}: async body without its D21 frame layout", function.symbol)));
    }
    let own_frame = match function.signature.params.as_slice() {
        [param] => *module.types.get(*param)? == o::Type::Frame(id),
        _ => false,
    };
    if !own_frame || !function.signature.results.is_empty() {
        return Err(error(format!("{}: an async body takes exactly its own frame and returns nothing",
            function.symbol)));
    }
    // C-6: an async defer entry publishes an empty success (Continue) or its failure, never a Future's outcome,
    // and never replaces the exit in 0.6.0.
    let replacements = function.attrs.iter().find_map(|attr| match attr {
        o::FunctionAttr::DeferEntry { replacements } => Some(replacements),
        _ => None,
    });
    if let Some(replacements) = replacements {
        if !replacements.is_empty() {
            return Err(error(format!("{}: an async defer entry never replaces (C-6)", function.symbol)));
        }
        if function.async_creation_summary.is_some() || function.async_body_summary.is_some() {
            return Err(error(format!("{}: an async defer entry creates no Future", function.symbol)));
        }
        return Ok(Some(AsyncBody { outcome: None, defer: true }));
    }
    let (Some(body), Some(creation)) = (function.async_body_summary, function.async_creation_summary) else {
        return Err(error(format!("{}: async body without its creation and body summaries", function.symbol)));
    };
    let outcome = match module.summaries.get(body)?.contract.results.as_slice() {
        [] => None,
        [result] => Some(translate_type(module, *result)?),
        _ => return Err(error(format!("{}: async body with more than one result", function.symbol))),
    };
    // The creation summary returns the Future of exactly this outcome.
    let agrees = match module.summaries.get(creation)?.contract.results.as_slice() {
        [future] => match translate_type(module, *future)? {
            s::Type::Future(index) => composites(module).futures().get(index as usize) == Some(&outcome),
            _ => false,
        },
        _ => false,
    };
    if !agrees {
        return Err(error(format!("{}: creation summary's Future differs from the body's outcome", function.symbol)));
    }
    Ok(Some(AsyncBody { outcome, defer: false }))
}

/// The D21 layout of an async body in SPB 17: slot types as produced types and each continuation's site as the
/// pc of its suspending operation (`inst_pcs`, `term_pcs`).
pub(super) fn frame_layout(module: &o::Module, layout: &o::FrameLayout, instructions: &BTreeMap<o::InstId, u32>,
    terms: &BTreeMap<o::BlockId, u32>) -> Result<s::FrameLayout, GenerateError>
{
    let mut slots = Vec::new();
    for slot in &layout.slots {
        let ty = kind(module, slot.ty)?;
        let linear = match ty {
            P::Value(_) => false,
            P::Service(K::Frame) => return Err(error("a frame is never stored in a frame slot")),
            P::Service(service) => service.linear(),
        };
        if slot.linear != linear {
            return Err(error("frame slot linearity differs from its type"));
        }
        slots.push(s::FrameSlot { ty, linear });
    }
    let mut continuations = Vec::new();
    for (index, continuation) in layout.continuations.iter().enumerate() {
        let site = match continuation.site {
            o::SuspendSite::Start if index == 0 => None,
            o::SuspendSite::Terminator(block) if index > 0 => {
                Some(*terms.get(&block).ok_or_else(|| error("continuation site names no block"))?)
            }
            o::SuspendSite::Instruction(inst) if index > 0 => {
                Some(*instructions.get(&inst).ok_or_else(|| error("continuation site names no instruction"))?)
            }
            _ => return Err(error("continuation 0 is the start, and only continuation 0")),
        };
        for &slot in continuation.saved.iter().chain(&continuation.operands) {
            if slot as usize >= slots.len() {
                return Err(error(format!("continuation {index} names frame slot {slot} outside the layout")));
            }
        }
        continuations.push(s::Continuation {
            site,
            saved: continuation.saved.clone(),
            operands: continuation.operands.clone(),
        });
    }
    if continuations.is_empty() {
        return Err(error("a frame layout needs its start continuation"));
    }
    Ok(s::FrameLayout { slots, continuations })
}

impl Emitter<'_> {
    /// The saved slots of continuation `k` in this body's layout.
    fn saved_slots(&self, continuation: u32) -> Result<&[u32], GenerateError> {
        let layout = self.body.frame_layout.as_ref().ok_or_else(|| error("frame operation outside an async body"))?;
        layout.continuations.get(continuation as usize).map(|continuation| continuation.saved.as_slice())
            .ok_or_else(|| error(format!("unknown continuation {continuation}")))
    }

    /// The name `obligation` has in the facts of `owner`, after the suspension that reloaded `owner`. Coro keeps
    /// the facts of a resumed value as they were (a deps key still names the value its continuation saved, for
    /// example the Future before an await), while the saved name is dead after its save (B9.5). When `owner` is a
    /// `frame.resume` result and `obligation` a value its continuation's `frame.save` stored, the obligation is the
    /// resume's result of the same slot, so the dependency never keeps the dead name alive across the suspension.
    pub(super) fn through_frame(&self, owner: o::ValueId, obligation: o::ValueId)
        -> Result<o::ValueId, GenerateError>
    {
        let o::Definition::InstResult { inst, .. } = self.body.values.get(owner)?.def else { return Ok(obligation) };
        let resume = self.body.insts.get(inst)?;
        // A committed value depends on the deps its exit carried, which the commit returns as its last result: the
        // value the prepare named has left with the reserve and is not kept alive for it (#601 D25).
        if let o::Operation::Exit(o::ExitOp::Commit { .. }) = &resume.op
            && let (Some(&value), Some(&deps)) = (resume.results.first(), resume.results.last())
            && value == owner && deps != owner && obligation != owner
        {
            return Ok(deps);
        }
        let o::Operation::Async(o::AsyncOp::FrameResume { continuation, slots: reloaded, .. }) = &resume.op else {
            return Ok(obligation);
        };
        let saved = self.body.insts.iter().find_map(|(_, candidate)| match &candidate.op {
            o::Operation::Async(o::AsyncOp::FrameSave { slots, continuation: saved, .. })
                if saved == continuation && *continuation != 0 => {
                slots.iter().find(|slot| slot.value == obligation).map(|slot| slot.slot)
            }
            _ => None,
        });
        let position = saved.and_then(|slot| reloaded.iter().position(|reloaded| reloaded.slot == slot));
        Ok(position.and_then(|position| resume.results.get(position + 1).copied()).unwrap_or(obligation))
    }

    /// The SPB 17 Future type of an OIR value and its outcome.
    fn future(&self, value: o::ValueId) -> Result<(s::Type, Option<s::Type>), GenerateError> {
        let ty = translate_type(self.module, self.body.values.get(value)?.ty)?;
        let s::Type::Future(index) = ty else { return Err(error("expected a Future value")) };
        let outcome = composites(self.module).futures().get(index as usize).copied()
            .ok_or_else(|| error("Future type without its outcome entry"))?;
        Ok((ty, outcome))
    }

    /// Whether `boundary` is just before a suspending operation of this body's layout: the terminator of a form
    /// (a) site or the instruction of a form (b) site.
    pub(super) fn suspension_site(&self, boundary: &o::RootBoundary) -> bool {
        let Some(layout) = &self.body.frame_layout else { return false };
        layout.continuations.iter().any(|continuation| match (continuation.site, boundary) {
            (o::SuspendSite::Terminator(site), o::RootBoundary::Terminator(block)) => site == *block,
            (o::SuspendSite::Instruction(site), o::RootBoundary::BeforeInstruction(inst)) => site == *inst,
            _ => false,
        })
    }

    /// The protocol of a settle or await.unwind step: the Services family (D16).
    pub(super) fn services(&self, protocol: o::ProtocolId) -> Result<(), GenerateError> {
        if self.module.protocols.get(protocol)?.kind != o::ProtocolKind::Services {
            return Err(later("a protocol family other than Services"));
        }
        Ok(())
    }

    /// An async instruction: Future creation, observation and cancellation, publication, the suspension
    /// brackets and the await.unwind adapter (form b of D20).
    pub(super) fn async_instruction(&mut self, op: &o::AsyncOp, results: &[o::ValueId])
        -> Result<s::Instruction, GenerateError>
    {
        use o::AsyncOp as X;
        let location = |value: o::ValueId| self.loc(value).map(A::Location);
        let (opcode, arguments) = match op {
            // PR-B1 (#784): SPB 17 does not carry `binding`. A created Future's duty is a token at run time, and no
            // selection can reach a token.
            X::FutureNew { packet, deps, owner_state, environment, result, owner, binding: _ } => {
                // An async external's Future is run by its driver (D22, #601 Q1); `fut.new` names it the same way.
                let (o::FutureEnvironment::Deferred { function, arguments, descriptors }
                    | o::FutureEnvironment::External { function, arguments, descriptors }) = environment else {
                    return Err(later("fut.new of an allocated frame"));
                };
                let callee = self.module.functions.get(*function)?;
                if !callee.attrs.contains(&o::FunctionAttr::Async) {
                    return Err(error(format!("fut.new names the synchronous function {}", callee.symbol)));
                }
                let created = results.first().ok_or_else(|| error("fut.new without its Future"))?;
                let (future, outcome) = self.future(*created)?;
                if outcome != result.map(|ty| translate_type(self.module, ty)).transpose()? {
                    return Err(error("fut.new result type differs from its Future"));
                }
                // C-2: the receiver and the language arguments, then the descriptors, then the inputs packet.
                let mut operands = vec![A::Entry(function.as_u32()), A::Scope(owner.as_u32()), A::Result(Some(future))];
                for &value in arguments.iter().chain(descriptors) {
                    operands.push(location(value)?);
                }
                operands.extend([location(*packet)?, location(*deps)?, location(*owner_state)?]);
                (O::FutureNew, operands)
            }
            // D24.1 (#601 P1): the Future and its cache duty; the duty comes back with the task duty.
            X::FutureSpawn { future, state: Some(cache) } => {
                (O::FutureSpawn, vec![location(*future)?, location(*cache)?])
            }
            X::FutureSpawn { state: None, .. } => return Err(error("High's fut.spawn reached codegen")),
            X::FutureCancel { future } => (O::FutureCancel, vec![location(*future)?]),
            X::FutureDone { future } => (O::FutureDone, vec![location(*future)?]),
            X::FutureCancelled { future } => (O::FutureCancelled, vec![location(*future)?]),
            X::FuturePublish { frame, outcome } => {
                let mut operands = vec![location(*frame)?];
                match outcome {
                    o::PublishOutcome::Success { value, packet, deps } => {
                        operands.push(A::Flag(true));
                        if let Some(value) = value {
                            operands.push(location(*value)?);
                        }
                        operands.extend([location(*packet)?, location(*deps)?]);
                    }
                    o::PublishOutcome::Failure(failure) => {
                        operands.extend([A::Flag(false), location(*failure)?]);
                    }
                }
                (O::FuturePublish, operands)
            }
            X::FrameSave { frame, slots, continuation } => {
                let order = slots.iter().map(|slot| slot.slot).collect::<Vec<_>>();
                if *continuation == 0 || order != self.saved_slots(*continuation)? {
                    return Err(error(format!("frame.save differs from continuation {continuation} of the layout")));
                }
                let mut operands = vec![location(*frame)?, A::Continuation(*continuation)];
                for slot in slots {
                    operands.push(location(slot.value)?);
                }
                (O::FrameSave, operands)
            }
            X::FrameResume { frame, continuation, slots } => {
                let order = slots.iter().map(|slot| slot.slot).collect::<Vec<_>>();
                if order != self.saved_slots(*continuation)? || results.len() != slots.len() + 1 {
                    return Err(error(format!("frame.resume differs from continuation {continuation} of the layout")));
                }
                (O::FrameResume, vec![location(*frame)?, A::Continuation(*continuation)])
            }
            X::AwaitUnwind { state, failure, task, receipt, protocol } => {
                self.services(*protocol)?;
                let operands = [*state, *failure, *task, *receipt].into_iter().map(location)
                    .collect::<Result<Vec<_>, _>>()?;
                (O::AwaitUnwind, operands)
            }
            // The capability is a proof position: its evidence was checked by verify_low, and the loader proves
            // it again from the code (SPB_TAKE_CAPABILITY), so only the proved Future travels.
            X::UniqueCacheProof { proof } => (O::UniqueCacheProof, vec![location(proof.future)?]),
            // C-4, token form: the cache duty, and a spawned Future's task duty with it (D24.1), join the pending
            // reserve of the exit whose destination it names.
            X::CacheRehome { form: o::RehomeForm::Token { pending, cache, task }, target, protocol } => {
                self.services(*protocol)?;
                // The target is the exit's destination (ruling after C-4): a scope, or the caller of a returned
                // Future (#601 R1).
                let destination = A::Destination(match target {
                    o::RehomeTarget::Scope(scope) => s::ExitDestination::Scope(scope.as_u32()),
                    o::RehomeTarget::Caller => s::ExitDestination::Caller,
                });
                let mut operands = vec![destination, location(*pending)?, location(*cache)?];
                if let Some(task) = task {
                    operands.push(location(*task)?);
                }
                (O::CacheRehome, operands)
            }
            // C-4, own form: the entry the owner admitted (D25.3) leaves the state for the reserve.
            X::CacheRehome { form: o::RehomeForm::Scope { state, pending, future }, target, protocol } => {
                self.services(*protocol)?;
                let destination = A::Destination(match target {
                    o::RehomeTarget::Scope(scope) => s::ExitDestination::Scope(scope.as_u32()),
                    o::RehomeTarget::Caller => s::ExitDestination::Caller,
                });
                (O::CacheRehome, vec![destination, location(*pending)?, location(*state)?, location(*future)?])
            }
            X::CacheRehome { form: o::RehomeForm::State(_), .. } => {
                return Err(error("the pre-C-4 cache.rehome form is not produced"));
            }
            // D25.2: a cache duty's entry out of a packet as its token, or a token into a packet.
            X::ServiceMaterialize { packet, entry } if entry.kind == o::ServiceKeyKind::Cache => {
                (O::ServiceMaterialize, vec![location(*packet)?, location(entry.holder)?])
            }
            X::ServiceMaterialize { .. } => return Err(later("service.materialize of a task duty (P1)")),
            X::ServiceAbsorb { packet, token } => (O::ServiceAbsorb, vec![location(*packet)?, location(*token)?]),
            X::Await { .. } | X::AwaitTake { .. } => {
                return Err(error("await and await.take are invoke.own terminators"));
            }
            _ => return Err(later(o::Operation::Async(op.clone()).opcode())),
        };
        let outputs = results.iter().map(|&value| self.loc(value)).collect::<Result<_, _>>()?;
        Ok(s::Instruction::Champ { op: opcode, arguments, flow: s::Flow::Next(outputs) })
    }

    /// `invoke.own await` and `invoke.own await.take` (form a of D20): the normal outcome is the value and its
    /// deps, for a take with the outcome packet between them; the unwind outcome is the failure with the waiter's
    /// task and receipt. Only the temporary take exists in 0.6.0 (C5): its boundary is the awaiting scope.
    pub(super) fn async_invocation(&mut self, op: &o::Operation, normal: &o::InvokeEdge, unwind: &o::InvokeEdge)
        -> Result<s::Instruction, GenerateError>
    {
        let (opcode, future, result, mut arguments) = match op {
            o::Operation::Async(o::AsyncOp::Await { future, result }) => (O::Await, future, result, vec![]),
            o::Operation::Async(o::AsyncOp::AwaitTake { future, result, unique,
                boundary: o::TakeBoundary::Temporary { awaiting } }) => {
                (O::AwaitTake, future, result, vec![A::Location(self.loc(*unique)?), A::Scope(awaiting.as_u32())])
            }
            o::Operation::Async(o::AsyncOp::AwaitTake { .. }) => return Err(later("bound await.take (C5)")),
            _ => return Err(later(op.opcode())),
        };
        let (_, outcome) = self.future(*future)?;
        if outcome != result.map(|ty| translate_type(self.module, ty)).transpose()? {
            return Err(error("await result type differs from its Future"));
        }
        arguments.insert(0, A::Location(self.loc(*future)?));
        let context = s::ResultContext { value: outcome, may_fail: true, ..s::ResultContext::default() };
        let tags = opcode.tags(&context);
        let edges = [(o::Outcome::Normal, normal), (o::Outcome::Unwind, unwind)];
        if tags != [s::Tag::Normal, s::Tag::Unwind] {
            return Err(error("await outcome inventory mismatch"));
        }
        let outcomes = tags.into_iter().zip(edges).map(|(tag, (outcome, edge))| {
            let schema = opcode.results(tag, &context)?;
            Ok(s::Outcome { tag, edge: self.edge(edge.block, &edge.args, outcome, &schema)? })
        }).collect::<Result<_, GenerateError>>()?;
        Ok(s::Instruction::Champ { op: opcode, arguments, flow: s::Flow::Outcomes(outcomes) })
    }

    /// The schedule a `cleanup.settle` step works through: the one of the close its cleanup state began, which
    /// must list the settled duty.
    pub(super) fn schedule_of(&self, cleanup: o::ValueId, duty: o::ValueId) -> Result<u32, GenerateError> {
        let close = self.close_of(cleanup)?;
        let mut owners = self.body.cleanup.schedules.iter().filter(|(_, schedule)| schedule.close == close);
        let (Some((id, schedule)), None) = (owners.next(), owners.next()) else {
            return Err(error("a close needs exactly one service schedule"));
        };
        if !schedule.holders.contains(&duty) {
            return Err(error("cleanup.settle settles a duty its close does not schedule"));
        }
        Ok(id.as_u32())
    }

    /// The close whose `cleanup.begin` produced `cleanup`, through the settle-phase steps that follow it.
    fn close_of(&self, mut cleanup: o::ValueId) -> Result<o::CleanupCloseId, GenerateError> {
        for _ in 0..self.body.insts.len() + 1 {
            let o::Definition::InstResult { inst, .. } = self.body.values.get(cleanup)?.def else {
                return Err(error("a settle step needs its cleanup.begin in the same block"));
            };
            match &self.body.insts.get(inst)?.op {
                o::Operation::Cleanup(o::CleanupOp::Begin { close, .. }) => return Ok(*close),
                o::Operation::Cleanup(o::CleanupOp::Settle { cleanup: previous, .. }
                    | o::CleanupOp::SettleEmpty { cleanup: previous, .. }) => cleanup = *previous,
                _ => return Err(error("a settle step follows cleanup.begin or another settle step")),
            }
        }
        Err(error("cyclic cleanup states"))
    }
}
