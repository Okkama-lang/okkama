use super::*;
use std::collections::{BTreeMap, VecDeque};

struct CloseInputs {
    state: ValueId,
    pending: Option<ValueId>,
    failure: Option<ValueId>,
    source: CleanupSource,
    effects: ScopeEffectEnvelope,
}
fn inputs(module: &Module, body: &Body, op: &Operation) -> Result<Option<CloseInputs>, LowError> {
    let (state, pending, failure, effects, boundary) = match op {
        Operation::Ownership(OwnershipOp::ScopeEnd { state, reserve, effects }) => {
            (*state, *reserve, None, effects.clone(), None)
        }
        Operation::Ownership(OwnershipOp::ScopeUnwind { state, failure, reserve, effects }) => {
            (*state, *reserve, Some(*failure), effects.clone(), None)
        }
        Operation::Ownership(OwnershipOp::End { packet, boundary }) => {
            let effects = ScopeEffectEnvelope {
                failures: vec![FailureKind::Thrown, FailureKind::Program, FailureKind::Panic, FailureKind::Native],
                suspends: false,
                cancel: false,
            };
            (*packet, None, None, effects, Some(*boundary))
        }
        _ => return Ok(None),
    };
    let source = if let Some(boundary) = boundary {
        body.boundaries.get(boundary)?;
        CleanupSource::Packet(boundary)
    } else if let Type::Service(ServiceType::Scope(scope)) = module.types.get(body.values.get(state)?.ty)? {
        CleanupSource::Scope(*scope)
    } else {
        return Err(LowError::InvalidInput { function: None, message: "close source is not a scope".into() });
    };
    Ok(Some(CloseInputs { state, pending, failure, source, effects }))
}
fn build(error: BuildError) -> LowError {
    LowError::InvalidOutput(error.to_string())
}
fn service(module: &mut Module, ty: ServiceType) -> Result<TypeId, LowError> {
    Ok(module.types.intern(Type::Service(ty))?)
}
fn block(
    body: &mut Body, scope: ScopeId, types: &[TypeId], budget: &mut Budget<'_>,
) -> Result<(BlockId, Vec<ValueId>), LowError> {
    budget.block()?;
    let id = body.blocks.alloc(Block {
        lexical_scope: scope,
        params: vec![],
        unwind: None,
        insts: vec![],
        term: Terminator { kind: TermKind::Unreachable, loc: None },
    })?;
    let mut values = vec![];
    for (index, ty) in types.iter().enumerate() {
        values.push(body.values.alloc(ValueData {
            ty: *ty,
            def: Definition::BlockParam { block: id, index: index as u32 },
            facts: ValueFacts::Service,
        })?);
    }
    body.blocks.get_mut(id)?.params.clone_from(&values);
    Ok((id, values))
}
fn emit(
    module: &mut Module, body: &mut Body, block: BlockId, op: CleanupOp, loc: Option<DebugLoc>,
) -> Result<Vec<ValueId>, LowError> {
    let op = Operation::Cleanup(op);
    let contract = ContractRegistry::new(module).construction_schema(&op, body)
        .map_err(|error| LowError::InvalidOutput(error.to_string()))?;
    let mut types = vec![];
    for slot in contract.normal.results {
        types.push(resolve_result_type(module, body, &slot.ty).map_err(build)?);
    }
    let id = body.insts.alloc(Instruction {
        results: vec![], op, facts: InstructionFacts::default(), loc,
    })?;
    let mut values = vec![];
    for (index, ty) in types.into_iter().enumerate() {
        let facts = if matches!(module.types.get(ty)?, Type::Service(_)) {
            ValueFacts::Service
        } else {
            ValueFacts::Plain { certificate: PlainCertificate::Scalar }
        };
        values.push(body.values.alloc(ValueData {
            ty, def: Definition::InstResult { inst: id, index: index as u16 }, facts,
        })?);
    }
    body.insts.get_mut(id)?.results.clone_from(&values);
    body.blocks.get_mut(block)?.insts.push(id);
    Ok(values)
}
fn terminate(body: &mut Body, block: BlockId, kind: TermKind, loc: Option<DebugLoc>) -> Result<(), LowError> {
    body.blocks.get_mut(block)?.term = Terminator { kind, loc };
    Ok(())
}
fn branch(body: &mut Body, source: BlockId, target: BlockId, args: Vec<ValueId>) -> Result<(), LowError> {
    terminate(body, source, TermKind::Br(Target { block: target, args }), None)
}
fn retag(mut edge: InvokeEdge, outcome: Outcome) -> InvokeEdge {
    for arg in &mut edge.args {
        if let EdgeArg::Produced(result) = arg {
            result.outcome = outcome;
        }
    }
    edge
}
fn produced(outcome: Outcome, slot: u16) -> EdgeArg {
    EdgeArg::Produced(EdgeResult { outcome, slot })
}

/// D20 §2.6: the envelope of a lowered close suspends only if one of its cleanup steps does. In 0.6.0 none
/// does: `cleanup.settle` never waits (async ABI D24.2, OQ-4) and eligibility rejects async destroy and async
/// defer. Failures and the final cancellation stay as own and coro declared them.
fn narrowed(effects: &ScopeEffectEnvelope) -> ScopeEffectEnvelope {
    ScopeEffectEnvelope { suspends: false, ..effects.clone() }
}

/// The blocks of the runtime selector loop over the remaining identities of a close: `cleanup.next`, a
/// destroy, and the record of its failure. Their parameters are allocated before any later block of the close,
/// so the synchronous closes keep their value numbering.
struct SelectorLoop {
    next: (BlockId, Vec<ValueId>),
    destroy: (BlockId, Vec<ValueId>),
    record: (BlockId, Vec<ValueId>),
}

fn selector_blocks(
    module: &mut Module, body: &mut Body, scope: ScopeId, budget: &mut Budget<'_>,
) -> Result<SelectorLoop, LowError> {
    let cleanup_type = service(module, ServiceType::CleanupState)?;
    let key_type = service(module, ServiceType::StableKey)?;
    let failure_type = service(module, ServiceType::Failure)?;
    let next = block(body, scope, &[cleanup_type], budget)?;
    let destroy = block(body, scope, &[cleanup_type, key_type], budget)?;
    let record = block(body, scope, &[cleanup_type, failure_type], budget)?;
    Ok(SelectorLoop { next, destroy, record })
}

/// `cleanup.next`, then a destroy whose failure is recorded, back to `cleanup.next`; the done edge enters
/// `done`. Returns the loop entry.
fn selector_fill(
    module: &mut Module, body: &mut Body, blocks: &SelectorLoop, done: BlockId, loc: Option<DebugLoc>,
) -> Result<BlockId, LowError> {
    let (next, next_params) = &blocks.next;
    let (destroy, destroy_params) = &blocks.destroy;
    let (record, record_params) = &blocks.record;
    terminate(body, *next, TermKind::CleanupNext {
        cleanup: next_params[0],
        selected: InvokeEdge { block: *destroy, args: vec![produced(Outcome::CleanupSelected, 0),
            produced(Outcome::CleanupSelected, 1)] },
        done: InvokeEdge { block: done, args: vec![produced(Outcome::CleanupDone, 0)] },
    }, loc)?;
    let step = emit(module, body, *destroy, CleanupOp::Destroy {
        cleanup: destroy_params[0], key: destroy_params[1],
    }, loc)?;
    terminate(body, *destroy, TermKind::FailureDispatch {
        failure: step[1],
        none: Target { block: *next, args: vec![step[0]] },
        some: InvokeEdge { block: *record, args: vec![EdgeArg::Value(step[0]),
            produced(Outcome::FailureSome, 0)] },
    }, loc)?;
    let recorded = emit(module, body, *record, CleanupOp::RecordFailure {
        cleanup: record_params[0], failure: record_params[1],
    }, loc)?[0];
    branch(body, *record, *next, vec![recorded])?;
    Ok(*next)
}

/// D18, OIR-2 §9.3 step 6: after the plan of the final successful close of an async action has run,
/// `cleanup.deliver_cancel` delivers a pending cancellation. A delivered cancellation becomes the primary
/// failure and aborts the reserve into a second plan, which the selector closes before `cleanup.finish`; the
/// already executed prefix stays executed (trace A R). Returns the block that receives the drained state.
#[allow(clippy::too_many_arguments)]
fn deliver_cancel(
    module: &mut Module, body: &mut Body, scope: ScopeId, close: CleanupCloseId, sources: Vec<MergeSource>,
    finish: BlockId, loc: Option<DebugLoc>, budget: &mut Budget<'_>,
) -> Result<BlockId, LowError> {
    let cleanup_type = service(module, ServiceType::CleanupState)?;
    let (deliver, deliver_params) = block(body, scope, &[cleanup_type], budget)?;
    let delivered = emit(module, body, deliver, CleanupOp::DeliverCancel { cleanup: deliver_params[0] }, loc)?[0];
    let plan = body.cleanup.plans.alloc(CleanupPlan { close, sources, direct: None })?;
    let planned = emit(module, body, deliver, CleanupOp::Plan { cleanup: delivered, plan }, loc)?[0];
    let blocks = selector_blocks(module, body, scope, budget)?;
    let next = selector_fill(module, body, &blocks, finish, loc)?;
    branch(body, deliver, next, vec![planned])?;
    Ok(deliver)
}

pub(super) fn expand(
    module: &mut Module, body: &mut Body, fresh: &plans::FreshSummaries,
    duties: &BTreeMap<services::CloseKey, services::CloseDuties>, budget: &mut Budget<'_>,
) -> Result<(), LowError> {
    let type_ofs = plans::type_ofs(body, budget)?;
    let mut queue: VecDeque<_> = body.blocks.iter().map(|(id, _)| id).collect();
    while let Some(source) = queue.pop_front() {
        budget.step(1)?;
        let mut site = None;
        for (position, id) in body.blocks.get(source)?.insts.iter().enumerate() {
            budget.step(1)?;
            if let Some(mut close) = inputs(module, body, &body.insts.get(*id)?.op)? {
                if matches!(close.source, CleanupSource::Packet(_))
                    && let TermKind::Raise(failure) = body.blocks.get(source)?.term.kind {
                    close.failure = Some(failure);
                }
                site = Some((position, *id, close));
                break;
            }
        }
        let scope = body.blocks.get(source)?.lexical_scope;
        let (close, normal, failed, loc, reuse, key) = if let Some((position, id, close)) = site {
            let old = body.insts.get(id)?.clone();
            let continuation = FunctionEditor::new(module, body).split_block(source, position + 1).map_err(build)?;
            budget.block()?;
            queue.push_back(continuation);
            body.blocks.get_mut(continuation)?.params.clone_from(&old.results);
            for (index, value) in old.results.iter().enumerate() {
                body.values.get_mut(*value)?.def = Definition::BlockParam {
                    block: continuation, index: index as u32,
                };
            }
            let outcome = if close.failure.is_some() {
                Outcome::ExitFailure
            } else if close.pending.is_some() {
                Outcome::ExitPending
            } else {
                Outcome::ExitSuccess
            };
            let args = if close.failure.is_some() {
                let slot = if matches!(close.source, CleanupSource::Packet(_)) { 0 } else { 1 };
                vec![produced(outcome, slot)]
            } else {
                (0..old.results.len()).map(|slot| produced(outcome, slot as u16)).collect()
            };
            let normal = InvokeEdge { block: continuation, args };
            let failed = if close.failure.is_none() && !close.effects.failures.is_empty() {
                if let Some(unwind) = &body.blocks.get(source)?.unwind {
                    let mut args: Vec<_> = unwind.target.args.iter().copied().map(EdgeArg::Value).collect();
                    args.push(produced(Outcome::ExitFailure, 1));
                    Some(InvokeEdge { block: unwind.target.block, args })
                } else {
                    // No local handler: the original close propagates to the caller after closing its source.
                    let closed = service(module, ServiceType::Closed)?;
                    let failure = service(module, ServiceType::Failure)?;
                    let (failed, values) = block(body, scope, &[closed, failure], budget)?;
                    terminate(body, failed, TermKind::Raise(values[1]), old.loc)?;
                    Some(InvokeEdge { block: failed, args: vec![produced(Outcome::ExitFailure, 0),
                        produced(Outcome::ExitFailure, 1)] })
                }
            } else {
                None
            };
            (close, normal, failed, old.loc, Some(id), services::CloseKey::Instruction(id))
        } else {
            let term = body.blocks.get(source)?.term.clone();
            let TermKind::InvokeOwn { op, normal, unwind } = term.kind else {
                continue;
            };
            let Some(close) = inputs(module, body, &op)? else {
                continue;
            };
            // D18: an invoke close has an outward Unwind for its failures or for the final cancellation.
            if close.failure.is_some() || close.effects.failures.is_empty() && !close.effects.cancel {
                return Err(LowError::InvalidInput {
                    function: None, message: "invoke close requires a D18 outward failure outcome".into(),
                });
            }
            let key = services::CloseKey::Edge(normal.block);
            (close, normal, Some(unwind), term.loc, None, key)
        };
        let effects = narrowed(&close.effects);
        let close_id = body.cleanup.closes.alloc(CleanupClose {
            source: close.source.clone(), after_defers: source, reserve: close.pending.is_some(),
            active_failure: close.failure.is_some(), effects: effects.clone(),
        })?;
        let assigned = duties.get(&key);
        let schedule = body.cleanup.schedules.alloc(CleanupSchedule {
            close: close_id,
            holders: assigned.map(|item| item.duties.clone()).unwrap_or_default(),
            dictionary: assigned.and_then(|item| item.dictionary),
            protocol: assigned.map(|item| item.protocol),
            dynamic: assigned.is_some_and(|item| item.dynamic),
        })?;
        let mut sources = vec![match close.source {
            CleanupSource::Scope(_) => MergeSource::ScopeInventory,
            CleanupSource::Packet(_) => MergeSource::PacketInventory,
        }];
        if close.pending.is_some() {
            sources.push(MergeSource::WholeReserveOnFailure);
        }
        // D25.3, draft 3 §6.3: entries settled dynamically may release outcomes, which only the executor takes.
        let direct = if assigned.is_some_and(|item| item.dynamic) {
            None
        } else {
            plans::prove(module, body, close.state, close.pending, body.cleanup.closes.get(close_id)?, fresh,
                &type_ofs, budget)?
        }.map(|proof| body.cleanup.proofs.alloc(proof)).transpose()?;
        let plan = body.cleanup.plans.alloc(CleanupPlan { close: close_id, sources: sources.clone(), direct })?;
        let begin = CleanupOp::Begin {
            state: close.state, pending: close.pending, failure: close.failure,
            close: close_id, effects: effects.clone(),
        };
        let cleanup_type = service(module, ServiceType::CleanupState)?;
        let failure_type = service(module, ServiceType::Failure)?;
        let c = if let Some(id) = reuse {
            let value = body.values.alloc(ValueData {
                ty: cleanup_type, def: Definition::InstResult { inst: id, index: 0 }, facts: ValueFacts::Service,
            })?;
            *body.insts.get_mut(id)? = Instruction {
                results: vec![value], op: Operation::Cleanup(begin), facts: InstructionFacts::default(), loc,
            };
            value
        } else {
            emit(module, body, source, begin, loc)?[0]
        };
        // OIR-2 §9.3 step 3: settle every scheduled duty, in schedule order, before the plan.
        let mut c = c;
        match assigned {
            Some(assigned) if !assigned.duties.is_empty() => {
                for &holder in &assigned.duties {
                    let settle = CleanupOp::Settle { cleanup: c, holder, protocol: assigned.protocol };
                    c = emit(module, body, source, settle, loc)?[0];
                }
            }
            _ => c = emit(module, body, source, CleanupOp::SettleEmpty { cleanup: c, schedule }, loc)?[0],
        }
        // D25.3: one step settles the entries the owner admitted at run time, after the static duties.
        if let Some(assigned) = assigned.filter(|assigned| assigned.dynamic) {
            let settle = CleanupOp::SettleDynamic { cleanup: c, protocol: assigned.protocol };
            c = emit(module, body, source, settle, loc)?[0];
        }
        let c = emit(module, body, source, CleanupOp::Plan { cleanup: c, plan }, loc)?[0];
        // The first plan drains into the final cancellation step, or into the finish itself.
        let (finish, finish_params) = if let Some(proof) = direct {
            let (finish, finish_params) = block(body, scope, &[cleanup_type], budget)?;
            let drained = if effects.cancel {
                deliver_cancel(module, body, scope, close_id, sources, finish, loc, budget)?
            } else {
                finish
            };
            let slots = body.cleanup.proofs.get(proof)?.slots.clone();
            let mut predecessor = source;
            let mut current = c;
            for slot in slots {
                let (destroy, destroy_params) = block(body, scope, &[cleanup_type], budget)?;
                let (record, record_params) = block(body, scope, &[cleanup_type, failure_type], budget)?;
                let (next, next_params) = block(body, scope, &[cleanup_type], budget)?;
                branch(body, predecessor, destroy, vec![current])?;
                let step = emit(module, body, destroy, CleanupOp::DestroyDirect {
                    cleanup: destroy_params[0], identity: slot.identity,
                    descriptor: slot.descriptor, destroy: slot.destroy,
                }, loc)?;
                terminate(body, destroy, TermKind::FailureDispatch {
                    failure: step[1], none: Target { block: next, args: vec![step[0]] },
                    some: InvokeEdge { block: record, args: vec![EdgeArg::Value(step[0]),
                        produced(Outcome::FailureSome, 0)] },
                }, loc)?;
                let recorded = emit(module, body, record, CleanupOp::RecordFailure {
                    cleanup: record_params[0], failure: record_params[1],
                }, loc)?[0];
                branch(body, record, next, vec![recorded])?;
                predecessor = next;
                current = next_params[0];
            }
            branch(body, predecessor, drained, vec![current])?;
            (finish, finish_params)
        } else {
            let blocks = selector_blocks(module, body, scope, budget)?;
            let (finish, finish_params) = block(body, scope, &[cleanup_type], budget)?;
            let drained = if effects.cancel {
                deliver_cancel(module, body, scope, close_id, sources, finish, loc, budget)?
            } else {
                finish
            };
            let next = selector_fill(module, body, &blocks, drained, loc)?;
            branch(body, source, next, vec![c])?;
            (finish, finish_params)
        };
        let finished = emit(module, body, finish, CleanupOp::Finish { cleanup: finish_params[0] }, loc)?;
        let mut success = None;
        let mut pending = None;
        let failed = if close.failure.is_some() {
            Some(retag(normal, Outcome::ExitFailure))
        } else {
            if close.pending.is_some() {
                pending = Some(retag(normal, Outcome::ExitPending));
            } else {
                success = Some(retag(normal, Outcome::ExitSuccess));
            }
            failed.map(|edge| retag(edge, Outcome::ExitFailure))
        };
        terminate(body, finish, TermKind::ExitDispatch {
            close: close_id, closed: finished[0], failure: finished[1], exit: finished[2],
            success, pending, failed,
        }, loc)?;
    }
    Ok(())
}
