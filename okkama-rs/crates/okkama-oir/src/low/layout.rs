//! D20 §2.6, review D: Low recomputes the D21 frame layout after the cleanup expansion.
//!
//! The continuations of narrowed closes are gone; the others are renumbered in order, so continuation `k`
//! stays the `k`-th entry of the layout. Sites are read again from the brackets, because the expansion moves
//! a terminator into the continuation block of a split. Slots that no continuation uses any more are
//! dropped and the rest keep their order.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn output(message: &str) -> LowError {
    LowError::InvalidOutput(message.into())
}

/// The site of each `frame.save`, by continuation.
fn sites(body: &Body, budget: &mut Budget<'_>) -> Result<BTreeMap<u32, SuspendSite>, LowError> {
    let mut sites = BTreeMap::new();
    for (block, data) in body.blocks.iter() {
        for (position, &inst) in data.insts.iter().enumerate() {
            budget.step(1)?;
            let Operation::Async(AsyncOp::FrameSave { continuation, .. }) = body.insts.get(inst)?.op else {
                continue;
            };
            let site = match data.insts.get(position + 1) {
                Some(next) => SuspendSite::Instruction(*next),
                None => SuspendSite::Terminator(block),
            };
            if sites.insert(continuation, site).is_some() {
                return Err(output("a continuation has one frame.save (D21)"));
            }
        }
    }
    Ok(sites)
}

/// Renumber the remaining continuations, recompute their sites and drop unused slots.
pub(super) fn relayout(body: &mut Body, removed: &BTreeSet<u32>, budget: &mut Budget<'_>) -> Result<(), LowError> {
    let Some(layout) = body.frame_layout.take() else {
        return Ok(());
    };
    let sites = sites(body, budget)?;
    let mut continuations = BTreeMap::new();
    let mut kept = Vec::new();
    for (old, continuation) in layout.continuations.into_iter().enumerate() {
        budget.step(1)?;
        let old = u32::try_from(old).map_err(|_| LowError::LimitExceeded("continuations"))?;
        if removed.contains(&old) {
            continue;
        }
        let new = u32::try_from(kept.len()).map_err(|_| LowError::LimitExceeded("continuations"))?;
        let site = if old == 0 {
            SuspendSite::Start
        } else {
            *sites.get(&old).ok_or_else(|| output("a remaining continuation keeps its frame.save (D20)"))?
        };
        continuations.insert(old, new);
        kept.push(Continuation { site, ..continuation });
    }
    if sites.keys().any(|old| !continuations.contains_key(old)) {
        return Err(output("a frame.save names a removed continuation"));
    }
    let used: BTreeSet<u32> = kept.iter().flat_map(|item| item.saved.iter().chain(&item.operands)).copied().collect();
    let slots: BTreeMap<u32, u32> = used.iter().enumerate()
        .map(|(new, old)| Ok((*old, u32::try_from(new).map_err(|_| LowError::LimitExceeded("slots"))?)))
        .collect::<Result<_, LowError>>()?;
    let mut decls = Vec::new();
    for old in &used {
        decls.push(*layout.slots.get(*old as usize).ok_or_else(|| output("a continuation names a declared slot"))?);
    }
    let renumber = |slot: &mut u32| -> Result<(), LowError> {
        *slot = *slots.get(slot).ok_or_else(|| output("a bracket names a slot that its continuation lists"))?;
        Ok(())
    };
    for continuation in &mut kept {
        for slot in continuation.saved.iter_mut().chain(continuation.operands.iter_mut()) {
            renumber(slot)?;
        }
    }
    for (_, inst) in body.insts.iter_mut() {
        budget.step(1)?;
        match &mut inst.op {
            Operation::Async(AsyncOp::FrameSave { continuation, slots: saved, .. }) => {
                *continuation = *continuations.get(continuation).ok_or_else(|| output("a saved continuation is kept"))?;
                for slot in saved {
                    renumber(&mut slot.slot)?;
                }
            }
            Operation::Async(AsyncOp::FrameResume { continuation, slots: resumed, .. }) => {
                *continuation = *continuations.get(continuation)
                    .ok_or_else(|| output("a resumed continuation is kept"))?;
                for slot in resumed {
                    renumber(&mut slot.slot)?;
                }
            }
            _ => {}
        }
    }
    body.frame_layout = Some(FrameLayout { slots: decls, continuations: kept });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layout lines of `@main`.
    fn layout_text(module: &Module) -> String {
        let printed = text::print(module).unwrap();
        let main = &printed[printed.find("func @main").unwrap()..];
        main.lines().filter(|line| line.trim_start().starts_with("continuation ")
            || line.trim_start().starts_with("slot ")).map(str::trim).collect::<Vec<_>>().join("\n")
    }

    /// Continuations 2 and 4 of plain await go; 3 becomes 2; the slot of the pending exit, used only by the
    /// close, goes and the later slots move down.
    #[test]
    fn remaining_continuations_and_slots_are_renumbered() {
        let source = include_str!("../../tests/fixtures/async/low-input/low-suspending-close.oir");
        let mut module = text::parse(source).unwrap();
        let (function, _) = module.functions.iter().find(|(_, function)| function.symbol == "main").unwrap();
        let options = LowOptions::default();
        let mut budget = Budget { options: &options, steps: 0, blocks: 0, roots: 0 };
        let body = module.functions.get_mut(function).unwrap().body.as_mut().unwrap();
        let removed = narrow::closes(body, &mut budget).unwrap();
        compact::compact(body).unwrap();
        relayout(body, &removed, &mut budget).unwrap();
        let expected = "slot 0: own linear\nslot 1: scope<S1> linear\nslot 2: cache-state linear\n\
            slot 3: future<int>\nslot 4: failure\nslot 5: task-state linear\nslot 6: receipt linear\n\
            continuation 0 start saves [0]\ncontinuation 1 at ^entry saves [1, 2] operands [3]\n\
            continuation 2 at #90 saves [2] operands [1, 4, 5, 6]";
        // Site labels depend on the printer's numbering; the continuation shape does not.
        let without_site = |line: &str| match (line.find(" at "), line.find(" saves")) {
            (Some(at), Some(saves)) => format!("{}{}", &line[..at], &line[saves..]),
            _ => line.to_string(),
        };
        let actual: Vec<_> = layout_text(&module).lines().map(without_site).collect();
        let expected: Vec<_> = expected.lines().map(without_site).collect();
        assert_eq!(actual, expected);
    }

    /// A frame.save of a continuation that the layout lost is an invalid output.
    #[test]
    fn a_save_of_a_removed_continuation_is_rejected() {
        let source = include_str!("../../tests/fixtures/async/low-input/low-suspending-close.oir");
        let mut module = text::parse(source).unwrap();
        let (function, _) = module.functions.iter().find(|(_, function)| function.symbol == "main").unwrap();
        let options = LowOptions::default();
        let mut budget = Budget { options: &options, steps: 0, blocks: 0, roots: 0 };
        let body = module.functions.get_mut(function).unwrap().body.as_mut().unwrap();
        let removed = BTreeSet::from([1]);
        let error = relayout(body, &removed, &mut budget).unwrap_err();
        assert!(matches!(error, LowError::InvalidOutput(_)), "{error:?}");
    }
}
