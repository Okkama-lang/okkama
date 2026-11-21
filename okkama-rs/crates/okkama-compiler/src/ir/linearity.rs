//! The ownership linearity check (#910): in every function after the IR passes, each cleanup region and each
//! static resource is opened once, used only while open, and closed exactly once on every path that leaves the
//! function, the failure paths included.
//!
//! It runs in debug builds (tests, CI) and with `OKKAMA_VERIFY_OWNERSHIP=1`, never in a release build and never in
//! the loader. It checks the shape of the operations only; the runtime's destroy trace (#562) stays the arbiter of
//! what is destroyed when.
//!
//! The analysis is a forward data flow over the blocks. A tracked value (a `region.enter` result or a value given
//! to `resource.pin`) is in a set of states: not yet opened, open, closed. A failing instruction leaves through the
//! block's unwind target with the state from before it, so a close that fails is closed again by its handler (the
//! runtime makes that second close a no-op). The bookkeeping operations themselves (enter, track, adopt, transfer,
//! pin) cannot fail.
use std::collections::HashMap;

use super::{Function, InstKind, Module, TermKind, Type, ValueId};

const UNOPENED: u8 = 1;
const OPEN: u8 = 2;
const CLOSED: u8 = 4;

type State = HashMap<ValueId, u8>;

/// Check every function; the first violation, named by function.
pub fn check(module: &Module) -> Result<(), String> {
    for function in &module.functions {
        check_function(function).map_err(|error| format!("ownership linearity in {}: {error}", function.symbol))?;
    }
    Ok(())
}

fn tracked(function: &Function) -> Vec<ValueId> {
    let mut values = Vec::new();
    for inst in function.blocks.iter().flat_map(|block| &block.insts) {
        match &inst.kind {
            InstKind::RegionEnter => values.extend(inst.result),
            InstKind::ResourcePin { value } => values.push(*value),
            _ => {}
        }
    }
    values.sort_by_key(|value| value.0);
    values.dedup();
    values
}

pub(crate) fn can_fail(kind: &InstKind) -> bool {
    !matches!(
        kind,
        InstKind::RegionEnter
            | InstKind::RegionAdopt { .. }
            | InstKind::RegionTrack { .. }
            | InstKind::RegionTransfer { .. }
            | InstKind::ResourcePin { .. }
            | InstKind::RegionUnwind { .. }
            | InstKind::RegionSettleUnwind { .. }
            | InstKind::ResourceUnwind { .. }
            | InstKind::FailSecondary { .. }
            | InstKind::FailCaused { .. }
            | InstKind::EnumCase { .. }
            | InstKind::Const(_)
            | InstKind::Undef
    )
}

/// A close that can fail closes before its failure leaves: the handler sees the value closed.
fn closes(kind: &InstKind) -> Option<ValueId> {
    match kind {
        InstKind::RegionLeave { region, .. } | InstKind::RegionSettle { region, .. } => Some(*region),
        InstKind::ResourceLeave { value } => Some(*value),
        _ => None,
    }
}

fn join(into: &mut State, from: &State) -> bool {
    let mut changed = false;
    for (value, bits) in from {
        let entry = into.entry(*value).or_insert(0);
        if *entry | bits != *entry {
            *entry |= bits;
            changed = true;
        }
    }
    changed
}

fn check_function(function: &Function) -> Result<(), String> {
    if function.blocks.is_empty() {
        return Ok(());
    }
    let values = tracked(function);
    if values.is_empty() {
        return Ok(());
    }
    // A region passed between blocks as a parameter is out of this check's reach.
    let regions_in_params = function.blocks.iter().flat_map(|block| &block.params)
        .any(|param| function.values[param.0] == Type::Region);
    if regions_in_params {
        return Ok(());
    }
    let mut entry: Vec<Option<State>> = vec![None; function.blocks.len()];
    entry[0] = Some(values.iter().map(|&value| (value, UNOPENED)).collect());
    let mut pending = vec![0usize];
    let mut errors = Vec::new();
    let mut visits = 0usize;
    while let Some(block_index) = pending.pop() {
        visits += 1;
        if visits > 64 * function.blocks.len() + 64 {
            return Err("the data flow does not settle".into());
        }
        let block = &function.blocks[block_index];
        let mut state = entry[block_index].clone().expect("reached");
        let flow = |target: usize, state: &State, entry: &mut Vec<Option<State>>, pending: &mut Vec<usize>| {
            let changed = match &mut entry[target] {
                Some(existing) => join(existing, state),
                none => {
                    *none = Some(state.clone());
                    true
                }
            };
            if changed {
                pending.push(target);
            }
        };
        for (index, inst) in block.insts.iter().enumerate() {
            if can_fail(&inst.kind)
                && let Some(unwind) = &block.unwind
            {
                match closes(&inst.kind) {
                    Some(value) => {
                        let mut after = state.clone();
                        after.insert(value, CLOSED);
                        flow(unwind.block.0, &after, &mut entry, &mut pending);
                    }
                    None => flow(unwind.block.0, &state, &mut entry, &mut pending),
                }
            }
            let at = || format!("block {block_index}, instruction {index}");
            let open = |state: &State, value: &ValueId, what: &str| -> Result<(), String> {
                match state.get(value) {
                    Some(&OPEN) | None => Ok(()),
                    Some(bits) => Err(format!("{what} of {value} at {} while it is {}", at(), describe(*bits))),
                }
            };
            // A definition starts a tracked value afresh (a loop makes a new one each round).
            if let Some(defined) = inst.result
                && !matches!(inst.kind, InstKind::RegionEnter)
                && state.contains_key(&defined)
            {
                state.insert(defined, UNOPENED);
            }
            let result = match &inst.kind {
                InstKind::RegionEnter => {
                    if let Some(region) = inst.result {
                        state.insert(region, OPEN);
                    }
                    Ok(())
                }
                InstKind::RegionAdopt { region, .. } | InstKind::RegionTrack { region, .. } => open(&state, region, "an add"),
                InstKind::RegionTransfer { from, to, .. } => {
                    open(&state, from, "a transfer").and_then(|()| open(&state, to, "a transfer"))
                }
                InstKind::RegionLeave { region, .. } | InstKind::RegionSettle { region, .. } => {
                    let checked = open(&state, region, "a close");
                    state.insert(*region, CLOSED);
                    checked
                }
                // A close on the failure path may meet a value its failing leave already closed: it does nothing
                // then. It must not meet one that was never opened.
                InstKind::RegionUnwind { region: value, .. }
                | InstKind::RegionSettleUnwind { region: value, .. }
                | InstKind::ResourceUnwind { value, .. } => {
                    let checked = match state.get(value) {
                        Some(bits) if bits & UNOPENED != 0 => {
                            Err(format!("an unwind of {value} at {} while it is {}", at(), describe(*bits)))
                        }
                        _ => Ok(()),
                    };
                    state.insert(*value, CLOSED);
                    checked
                }
                InstKind::ResourcePin { value } => {
                    let checked = match state.get(value) {
                        Some(&UNOPENED) | None => Ok(()),
                        Some(bits) => Err(format!("a pin of {value} at {} while it is {}", at(), describe(*bits))),
                    };
                    state.insert(*value, OPEN);
                    checked
                }
                InstKind::ResourceLeave { value } => {
                    let checked = open(&state, value, "a close");
                    state.insert(*value, CLOSED);
                    checked
                }
                _ => Ok(()),
            };
            if let Err(error) = result {
                errors.push(error);
            }
        }
        match &block.term.kind {
            // A throw or raise goes to the block's unwind target when it has one; it leaves the function only
            // when it has none.
            TermKind::Raise(_) | TermKind::Throw(_) if block.unwind.is_some() => {
                let unwind = block.unwind.as_ref().expect("checked");
                flow(unwind.block.0, &state, &mut entry, &mut pending);
            }
            TermKind::Ret(_) | TermKind::Raise(_) | TermKind::Throw(_) => {
                for (value, bits) in &state {
                    if bits & OPEN != 0 {
                        errors.push(format!("{value} may still be open when block {block_index} leaves the function"));
                    }
                }
            }
            kind => {
                for target in kind.targets() {
                    flow(target.block.0, &state, &mut entry, &mut pending);
                }
            }
        }
    }
    errors.sort();
    errors.dedup();
    match errors.first() {
        Some(error) => Err(error.clone()),
        None => Ok(()),
    }
}

fn describe(bits: u8) -> &'static str {
    match bits {
        UNOPENED => "not yet opened",
        CLOSED => "already closed",
        _ if bits & CLOSED != 0 && bits & OPEN != 0 => "closed on some paths",
        _ => "not opened on some paths",
    }
}
