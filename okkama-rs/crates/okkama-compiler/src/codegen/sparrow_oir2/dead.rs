//! Code the emitter itself disconnects (#702).
//!
//! A Low invocation that SPB 17 computes totally (a total scalar family operation, a plain construction,
//! the payload read of an Error variant's only case) becomes data plus a branch on its normal edge: its
//! unwind edge is dropped, so no edge from the entry reaches the cleanup chain behind it any more. Low may
//! still name values across that chain by dominance (a local carried from its first block to a catch
//! handler, the roots that keep it alive, the failure a parameterless block reads), and the loader rightly
//! admits a use in an unreachable block only when that block defines it. Such code never runs, so the
//! emitter removes it: its blocks, their closes and cleanup evidence, and every record at its pcs. Reachable
//! code is kept instruction for instruction; only its pcs move.
//!
//! An async body is pruned the same way (#741, ruling PR-M), and the loader stays strict. Its D21 frame
//! layout names each suspension by pc (`Continuation::site`), so the sites move with the code. Every
//! successor of a suspension resumes it (`RESUME_TABLE`), so the edges from the entry reach each resume
//! block whose site is reachable. A continuation whose site was unreachable goes with its code, and the
//! continuations after it are renumbered in the `frame.save` and `frame.resume` operands that remain.

use super::*;

/// Branch targets of an instruction (none for a non-terminator).
fn targets(instruction: &s::Instruction) -> Vec<&s::Edge> {
    match instruction {
        s::Instruction::Br(edge) => vec![edge],
        s::Instruction::Cond { yes, no, .. } => vec![yes, no],
        s::Instruction::Champ { flow: s::Flow::Outcomes(outcomes), .. } => {
            outcomes.iter().map(|outcome| &outcome.edge).collect()
        }
        _ => vec![],
    }
}

/// Indices of the blocks no edge from the entry block reaches, in block order.
pub(super) fn unreachable_blocks(function: &s::Function) -> Result<Vec<usize>, GenerateError> {
    let count = function.blocks.len();
    let index = function.blocks.iter().enumerate().map(|(index, block)| (block.pc, index))
        .collect::<BTreeMap<_, _>>();
    let mut reached = vec![false; count];
    let mut work = if count == 0 { vec![] } else { vec![0] };
    while let Some(block) = work.pop() {
        if std::mem::replace(&mut reached[block], true) {
            continue;
        }
        let start = function.blocks[block].pc as usize;
        let end = function.blocks.get(block + 1).map_or(function.code.len(), |next| next.pc as usize);
        for instruction in function.code.get(start..end).ok_or_else(|| GenerateError("block outside code".into()))? {
            for edge in targets(instruction) {
                let target = *index.get(&edge.target_pc)
                    .ok_or_else(|| GenerateError("edge target is not a block".into()))?;
                if !reached[target] {
                    work.push(target);
                }
            }
        }
    }
    Ok((0..count).filter(|&block| !reached[block]).collect())
}

/// Remove the unreachable blocks of a function (module docs). A close whose `cleanup.begin` is removed goes
/// with its cleanup evidence; other tables keep their indices. `frames` are the program's D21 frame layouts:
/// an async body's own layout follows the removal.
pub(super) fn prune_unreachable(function: &mut s::Function, frames: &mut [s::FrameLayout])
    -> Result<(), GenerateError>
{
    let layout = match function.entry_abi {
        s::EntryAbi::AsyncBody { layout } | s::EntryAbi::AsyncDefer { layout } => Some(layout),
        _ => None,
    };
    let dead = unreachable_blocks(function)?;
    if dead.is_empty() {
        return Ok(());
    }
    let mut live = vec![true; function.code.len()];
    for &block in &dead {
        let start = function.blocks[block].pc as usize;
        let end = function.blocks.get(block + 1).map_or(function.code.len(), |next| next.pc as usize);
        live[start..end].fill(false);
    }
    let mut moved = vec![None; function.code.len()];
    let mut next = 0u32;
    for (pc, &kept) in live.iter().enumerate() {
        if kept {
            moved[pc] = Some(next);
            next += 1;
        }
    }
    let at = |pc: u32| moved.get(pc as usize).copied().flatten();
    let kept = |pc: u32| at(pc).ok_or_else(|| GenerateError(format!("live record at removed pc {pc}")));
    let mut code = Vec::with_capacity(next as usize);
    for (pc, instruction) in std::mem::take(&mut function.code).into_iter().enumerate() {
        if live[pc] {
            code.push(instruction);
        }
    }
    for instruction in &mut code {
        let edges: Vec<&mut s::Edge> = match instruction {
            s::Instruction::Br(edge) => vec![edge],
            s::Instruction::Cond { yes, no, .. } => vec![yes, no],
            s::Instruction::Champ { flow: s::Flow::Outcomes(outcomes), .. } => {
                outcomes.iter_mut().map(|outcome| &mut outcome.edge).collect()
            }
            _ => vec![],
        };
        for edge in edges {
            edge.target_pc = kept(edge.target_pc)?;
        }
    }
    function.code = code;
    let blocks = std::mem::take(&mut function.blocks);
    for (index, mut block) in blocks.into_iter().enumerate() {
        if dead.binary_search(&index).is_err() {
            block.pc = kept(block.pc)?;
            function.blocks.push(block);
        }
    }
    // Plans and envelopes keep only what kept code names; the loader wants one evidence per plan.
    let mut plans = BTreeSet::new();
    let mut envelopes = BTreeSet::new();
    for instruction in &function.code {
        if let s::Instruction::Champ { arguments, .. } = instruction {
            for argument in arguments {
                match *argument {
                    s::Argument::Plan(id) => { plans.insert(id); }
                    s::Argument::Envelope(id) => { envelopes.insert(id); }
                    _ => {}
                }
            }
        }
    }
    let proofs = &mut function.proofs;
    // Closes are found by their begin pc and named by position in the cleanup evidence.
    let closes = retain_indexed(&mut proofs.closes, |_, close| at(close.begin_pc).is_some());
    for close in &mut proofs.closes {
        close.begin_pc = kept(close.begin_pc)?;
        envelopes.insert(close.envelope);
    }
    let plans = retain_indexed(&mut proofs.plans, |index, _| plans.contains(&index));
    let envelopes = retain_indexed(&mut proofs.envelopes, |index, _| envelopes.contains(&index));
    let mut evidence = Vec::new();
    for mut plan in std::mem::take(&mut proofs.cleanup_plans) {
        let (Some(&close), Some(&id)) = (closes.get(&plan.close), plans.get(&plan.plan)) else { continue };
        plan.close = close;
        plan.plan = id;
        for slot in &mut plan.slots {
            slot.active_pc = kept(slot.active_pc)?;
            match &mut slot.origin {
                s::CreationOrigin::Singleton => {}
                s::CreationOrigin::Create(pc) | s::CreationOrigin::FreshResult { pc, .. } => *pc = kept(*pc)?,
            }
        }
        evidence.push(plan);
    }
    proofs.cleanup_plans = evidence;
    for close in &mut proofs.closes {
        close.envelope = envelopes[&close.envelope];
    }
    for instruction in &mut function.code {
        if let s::Instruction::Champ { arguments, .. } = instruction {
            for argument in arguments {
                match argument {
                    s::Argument::Plan(id) => *id = plans[id],
                    s::Argument::Envelope(id) => *id = envelopes[id],
                    _ => {}
                }
            }
        }
    }
    let proofs = &mut function.proofs;
    for records in [&mut proofs.roots, &mut proofs.post_roots] {
        records.retain_mut(|root| at(root.pc).map(|pc| root.pc = pc).is_some());
    }
    proofs.states.retain_mut(|state| at(state.pc).map(|pc| state.pc = pc).is_some());
    proofs.static_selections.retain_mut(|selection| at(selection.pc).map(|pc| selection.pc = pc).is_some());
    // A witness of removed creations orders nothing that runs.
    for witness in &mut proofs.order_witnesses {
        witness.creation_pcs = witness.creation_pcs.iter().filter_map(|&pc| at(pc)).collect();
    }
    if let Some(layout) = layout {
        let layout = frames.get_mut(layout as usize).ok_or_else(|| GenerateError("unknown frame layout".into()))?;
        prune_continuations(layout, &mut function.code, at)?;
    }
    Ok(())
}

/// Moves each continuation's site to its new pc (#741). A continuation whose site was removed is dropped, and the
/// continuation operands of the kept `frame.save`/`frame.resume` follow; the start (continuation 0) has no site.
fn prune_continuations(layout: &mut s::FrameLayout, code: &mut [s::Instruction], at: impl Fn(u32) -> Option<u32>)
    -> Result<(), GenerateError>
{
    let mut renamed = BTreeMap::new();
    let mut kept = Vec::new();
    for (index, mut continuation) in std::mem::take(&mut layout.continuations).into_iter().enumerate() {
        match continuation.site {
            Some(site) => match at(site) {
                Some(moved) => continuation.site = Some(moved),
                None => continue,
            },
            None if index == 0 => {}
            None => return Err(GenerateError(format!("continuation {index} without its site"))),
        }
        renamed.insert(index as u32, kept.len() as u32);
        kept.push(continuation);
    }
    layout.continuations = kept;
    for instruction in code {
        let s::Instruction::Champ { op: s::Opcode::FrameSave | s::Opcode::FrameResume, arguments, .. } = instruction
        else {
            continue;
        };
        for argument in arguments {
            if let s::Argument::Continuation(id) = argument {
                *id = *renamed.get(id)
                    .ok_or_else(|| GenerateError(format!("live frame operation of removed continuation {id}")))?;
            }
        }
    }
    Ok(())
}

/// Remove the entries `keep` rejects (it sees each old index); returns every kept old index with its new one.
fn retain_indexed<T>(entries: &mut Vec<T>, mut keep: impl FnMut(u32, &T) -> bool) -> BTreeMap<u32, u32> {
    let mut renamed = BTreeMap::new();
    let mut kept = Vec::new();
    for (index, entry) in std::mem::take(entries).into_iter().enumerate() {
        if keep(index as u32, &entry) {
            renamed.insert(index as u32, kept.len() as u32);
            kept.push(entry);
        }
    }
    *entries = kept;
    renamed
}
