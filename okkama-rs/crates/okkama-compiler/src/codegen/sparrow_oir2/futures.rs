//! Plain async in SPB 17 (#601 V7): async bodies with their D21 frame layout, Future creation, await and its
//! unwind adapter, publication, the suspension brackets and cancellation requests. The forms follow the async
//! ABI (C-1…C-3, D20, D21) and Champ draft 3 §14; the loader rules of §15 check what this emits.
use super::*;

/// What SPB 17 takes from an async body besides its code and layout. A function is an async body when it carries
/// `[async]`: it then needs both async summaries, its frame layout and its own frame as its only parameter.
pub(super) struct AsyncBody {
    /// The Future's outcome value; `None` for a void Future.
    pub outcome: Option<s::Type>,
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
        return Err(later(&format!("{}: async external (D22)", function.symbol)));
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
    Ok(Some(AsyncBody { outcome }))
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
            X::FutureNew { packet, deps, owner_state, environment, result, owner } => {
                let o::FutureEnvironment::Deferred { function, arguments, descriptors } = environment else {
                    return Err(later("fut.new of an async external (D22) or an allocated frame"));
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
            X::Await { .. } => return Err(error("await is an invoke.own terminator")),
            _ => return Err(later(o::Operation::Async(op.clone()).opcode())),
        };
        let outputs = results.iter().map(|&value| self.loc(value)).collect::<Result<_, _>>()?;
        Ok(s::Instruction::Champ { op: opcode, arguments, flow: s::Flow::Next(outputs) })
    }

    /// `invoke.own await` (form a of D20): the normal outcome is the value and its deps, the unwind outcome
    /// the failure with the waiter's task and receipt.
    pub(super) fn async_invocation(&mut self, op: &o::Operation, normal: &o::InvokeEdge, unwind: &o::InvokeEdge)
        -> Result<s::Instruction, GenerateError>
    {
        let o::Operation::Async(o::AsyncOp::Await { future, result }) = op else {
            return Err(later(op.opcode()));
        };
        let (_, outcome) = self.future(*future)?;
        if outcome != result.map(|ty| translate_type(self.module, ty)).transpose()? {
            return Err(error("await result type differs from its Future"));
        }
        let context = s::ResultContext { value: outcome, may_fail: true, ..s::ResultContext::default() };
        let tags = O::Await.tags(&context);
        let edges = [(o::Outcome::Normal, normal), (o::Outcome::Unwind, unwind)];
        if tags != [s::Tag::Normal, s::Tag::Unwind] {
            return Err(error("await outcome inventory mismatch"));
        }
        let outcomes = tags.into_iter().zip(edges).map(|(tag, (outcome, edge))| {
            let schema = O::Await.results(tag, &context)?;
            Ok(s::Outcome { tag, edge: self.edge(edge.block, &edge.args, outcome, &schema)? })
        }).collect::<Result<_, GenerateError>>()?;
        Ok(s::Instruction::Champ {
            op: O::Await,
            arguments: vec![A::Location(self.loc(*future)?)],
            flow: s::Flow::Outcomes(outcomes),
        })
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
