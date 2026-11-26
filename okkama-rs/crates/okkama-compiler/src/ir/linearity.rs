//! The ownership linearity check (#910): in every function after the IR passes, each cleanup region and each
//! static resource is opened once, used only while open, and closed exactly once on every path that leaves the
//! function, the failure paths included; and every value that may hold a free resource identity is owned before
//! the function leaves (#1073).
//!
//! It runs in debug builds (tests, CI) and, in an optimized build too, with `OKKAMA_VERIFY_OWNERSHIP=1`; packaged
//! releases (`--cfg okkama_release`) compile it out, and the loader never runs it. It checks the shape of the
//! operations only; the runtime's destroy trace (#562) stays the arbiter of what is destroyed when.
//!
//! The analysis is a forward data flow over the blocks. A tracked value (a `region.enter` result or a value given
//! to `resource.pin`) is in a set of states: not yet opened, open, closed. A failing instruction leaves through the
//! block's unwind target with the state from before it, so a close that fails is closed again by its handler (the
//! runtime makes that second close a no-op). The bookkeeping operations themselves (enter, track, adopt, transfer,
//! pin) cannot fail. A region entered again while it may still be open is refused at that `region.enter` (#1073).
//!
//! Ownership (#1073). A *fresh* resource is a value of an object type with its own `destroy` that an instruction
//! makes: the result of `obj.new`, of a call (`call`, `call.fn`, `call.role`, `call.cap`) or of `await.take`
//! (an `await` leaves the outcome with its Future), whose identity nobody owns yet. It is free from its definition until `region.track`,
//! `region.adopt` or `region.transfer` of it, or `resource.pin`, owns it; `region.leave`/`region.settle` with `keep` of it frees it again, and
//! `await.take` of a Future consumes it. A fresh value may leave the function free only as the value it returns
//! or throws (the caller or the handler takes it); free at any other exit, or made again by a loop while the one
//! before is free, it has no owner that would destroy it, and the check refuses it.
use std::collections::HashMap;

use super::{Function, InstKind, Module, TermKind, Type, ValueId};

const UNOPENED: u8 = 1;
const OPEN: u8 = 2;
const CLOSED: u8 = 4;
/// Ownership states of a fresh value (#1073), in their own entries of a state.
const FREE: u8 = 8;
const OWNED: u8 = 16;

/// The states of the values a function's check can meet, one byte per value
/// (0: not met yet), indexed through [`Keys`] (#972: a map per state made the
/// check of a function with many resources take most of a debug check).
#[derive(Clone)]
struct State(Vec<u8>);

/// The values a state can hold, and their positions.
struct Keys {
    index: HashMap<ValueId, usize>,
    values: Vec<ValueId>,
}

impl Keys {
    fn new(values: impl IntoIterator<Item = ValueId>) -> Self {
        let mut keys = Keys { index: HashMap::new(), values: Vec::new() };
        for value in values {
            if let std::collections::hash_map::Entry::Vacant(slot) = keys.index.entry(value) {
                slot.insert(keys.values.len());
                keys.values.push(value);
            }
        }
        keys
    }
}

impl State {
    fn get(&self, keys: &Keys, value: &ValueId) -> Option<u8> {
        keys.index.get(value).map(|&at| self.0[at]).filter(|&bits| bits != 0)
    }
    fn insert(&mut self, keys: &Keys, value: ValueId, bits: u8) {
        let at = keys.index[&value];
        self.0[at] = bits;
    }
    fn contains(&self, keys: &Keys, value: &ValueId) -> bool {
        self.get(keys, value).is_some()
    }
}

/// Check every function; the first violation, named by function.
pub fn check(module: &Module) -> Result<(), String> {
    for function in &module.functions {
        check_function(module, function)
            .map_err(|error| format!("ownership linearity in {}: {error}", function.symbol))?;
    }
    Ok(())
}

/// The resources `function` makes (#1073): values of an object type with its own `destroy` (a resource, the
/// Bible's ch. 11), made by `obj.new`, returned by a call or taken from a Future by `await.take`. An `await`
/// leaves the outcome with its Future, whose owner closes it. Containers, roles and Futures that may reach a
/// resource are left to the regions they are tracked in.
fn fresh(module: &Module, function: &Function) -> Vec<ValueId> {
    let mut values = Vec::new();
    for inst in function.blocks.iter().flat_map(|block| &block.insts) {
        let makes = matches!(
            inst.kind,
            InstKind::ObjNew { .. }
                | InstKind::Call { .. }
                | InstKind::CallFn { .. }
                | InstKind::CallRole { .. }
                | InstKind::CallCap { .. }
                | InstKind::AwaitTake(_)
        );
        if makes
            && let Some(value) = inst.result
            && let Type::Object(object) = &function.values[value.0]
            && module.object(*object).is_some_and(|object| object.destroy.is_some())
        {
            values.push(value);
        }
    }
    values.sort_by_key(|value| value.0);
    values.dedup();
    values
}

/// What owns or frees a fresh value: `Some(true)` owns, `Some(false)` frees.
fn ownership(kind: &InstKind) -> Option<(ValueId, bool)> {
    match kind {
        InstKind::RegionAdopt { value, .. }
        | InstKind::RegionTrack { value, .. }
        | InstKind::RegionTransfer { value, .. }
        | InstKind::ResourcePin { value }
        | InstKind::AwaitTake(value) => Some((*value, true)),
        InstKind::RegionLeave { keep: Some(value), .. } | InstKind::RegionSettle { keep: Some(value), .. } => {
            Some((*value, false))
        }
        _ => None,
    }
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
            | InstKind::ResourceSettleUnwind { .. }
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
        InstKind::ResourceLeave { value } | InstKind::ResourceSettle { value } => Some(*value),
        _ => None,
    }
}

fn join(into: &mut State, from: &State) -> bool {
    let mut changed = false;
    for (entry, bits) in into.0.iter_mut().zip(&from.0) {
        if *entry | bits != *entry {
            *entry |= bits;
            changed = true;
        }
    }
    changed
}

/// The most block visits the data flow can need (#972). A block is visited
/// once per change of its entry state, and a join only adds bits: each of
/// `values` tracked values gains at most its three states, so one entry
/// changes at most `3 * values` times after it is first reached. More visits
/// than that is a defect of the check, not a slow function; a fixed bound
/// (64 per block) refused real std functions with many resources.
fn visit_limit(blocks: usize, values: usize) -> usize {
    blocks.saturating_mul(3usize.saturating_mul(values).saturating_add(1)).saturating_add(1)
}

fn check_function(module: &Module, function: &Function) -> Result<(), String> {
    if function.blocks.is_empty() {
        return Ok(());
    }
    let values = tracked(function);
    let made = fresh(module, function);
    if values.is_empty() && made.is_empty() {
        return Ok(());
    }
    // A region passed between blocks as a parameter is out of this check's
    // reach. Lowering never makes one; if it starts to, say so instead of
    // silently checking nothing (#1007).
    let regions_in_params = function.blocks.iter().flat_map(|block| &block.params)
        .any(|param| function.values[param.0] == Type::Region);
    if regions_in_params {
        return Err("a region passed as a block parameter is not supported by the linearity check".into());
    }
    // Every value an instruction below can put in a state: the tracked ones,
    // and the operands of closes (outside them only in invalid IR).
    let keys = Keys::new(values.iter().copied().chain(function.blocks.iter().flat_map(|block| &block.insts).filter_map(|inst| {
        match &inst.kind {
            InstKind::RegionUnwind { region: value, .. }
            | InstKind::RegionSettleUnwind { region: value, .. }
            | InstKind::ResourceUnwind { value, .. }
            | InstKind::ResourceSettleUnwind { value, .. }
            | InstKind::ResourcePin { value }
            | InstKind::ResourceLeave { value }
            | InstKind::ResourceSettle { value } => Some(*value),
            kind => closes(kind),
        }
    })));
    // The ownership entries of the fresh values follow the linearity ones.
    let owners = Keys::new(made.iter().copied());
    let base = keys.values.len();
    let mut start = State(vec![0; base + owners.values.len()]);
    for &value in &values {
        start.insert(&keys, value, UNOPENED);
    }
    let owner_of = |state: &State, value: &ValueId| owners.index.get(value).map(|&at| state.0[base + at]);
    let set_owner = |state: &mut State, value: &ValueId, bits: u8| {
        if let Some(&at) = owners.index.get(value) {
            state.0[base + at] = bits;
        }
    };
    let mut entry: Vec<Option<State>> = vec![None; function.blocks.len()];
    entry[0] = Some(start);
    let mut pending = vec![0usize];
    let mut errors = Vec::new();
    let mut visits = 0usize;
    let limit = visit_limit(function.blocks.len(), keys.values.len() + owners.values.len());
    while let Some(block_index) = pending.pop() {
        visits += 1;
        if visits > limit {
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
                        after.insert(&keys, value, CLOSED);
                        flow(unwind.block.0, &after, &mut entry, &mut pending);
                    }
                    None => flow(unwind.block.0, &state, &mut entry, &mut pending),
                }
            }
            let at = || format!("block {block_index}, instruction {index}");
            let open = |state: &State, value: &ValueId, what: &str| -> Result<(), String> {
                match state.get(&keys, value) {
                    Some(OPEN) | None => Ok(()),
                    Some(bits) => Err(format!("{what} of {value} at {} while it is {}", at(), describe(bits))),
                }
            };
            // A definition starts a tracked value afresh (a loop makes a new one each round).
            if let Some(defined) = inst.result
                && !matches!(inst.kind, InstKind::RegionEnter)
                && state.contains(&keys, &defined)
            {
                state.insert(&keys, defined, UNOPENED);
            }
            // A fresh value is free when made; the one a loop made before must have an owner by then.
            if let Some(defined) = inst.result
                && let Some(bits) = owner_of(&state, &defined)
            {
                if bits & FREE != 0 {
                    errors.push((
                        (block_index, index),
                        format!("{defined} is made again at {} while the one made before is in no region", at()),
                    ));
                }
                set_owner(&mut state, &defined, FREE);
            }
            if let Some((value, owns)) = ownership(&inst.kind)
                && owner_of(&state, &value).is_some()
            {
                set_owner(&mut state, &value, if owns { OWNED } else { FREE });
            }
            let result = match &inst.kind {
                InstKind::RegionEnter => {
                    let mut checked = Ok(());
                    if let Some(region) = inst.result {
                        // A loop that enters its region each round must close it first (#1073).
                        if state.get(&keys, &region).is_some_and(|bits| bits & OPEN != 0) {
                            checked = Err(format!("{region} is entered again at {} while it may still be open", at()));
                        }
                        state.insert(&keys, region, OPEN);
                    }
                    checked
                }
                InstKind::RegionAdopt { region, .. } | InstKind::RegionTrack { region, .. } => open(&state, region, "an add"),
                InstKind::RegionTransfer { from, to, .. } => {
                    open(&state, from, "a transfer").and_then(|()| open(&state, to, "a transfer"))
                }
                InstKind::RegionLeave { region, .. } | InstKind::RegionSettle { region, .. } => {
                    let checked = open(&state, region, "a close");
                    state.insert(&keys, *region, CLOSED);
                    checked
                }
                // A close on the failure path may meet a value its failing leave already closed: it does nothing
                // then. It must not meet one that was never opened.
                InstKind::RegionUnwind { region: value, .. }
                | InstKind::RegionSettleUnwind { region: value, .. }
                | InstKind::ResourceUnwind { value, .. }
                | InstKind::ResourceSettleUnwind { value, .. } => {
                    let checked = match state.get(&keys, value) {
                        Some(bits) if bits & UNOPENED != 0 => {
                            Err(format!("an unwind of {value} at {} while it is {}", at(), describe(bits)))
                        }
                        _ => Ok(()),
                    };
                    state.insert(&keys, *value, CLOSED);
                    checked
                }
                InstKind::ResourcePin { value } => {
                    let checked = match state.get(&keys, value) {
                        Some(UNOPENED) | None => Ok(()),
                        Some(bits) => Err(format!("a pin of {value} at {} while it is {}", at(), describe(bits))),
                    };
                    state.insert(&keys, *value, OPEN);
                    checked
                }
                InstKind::ResourceLeave { value } | InstKind::ResourceSettle { value } => {
                    let checked = open(&state, value, "a close");
                    state.insert(&keys, *value, CLOSED);
                    checked
                }
                _ => Ok(()),
            };
            if let Err(error) = result {
                errors.push(((block_index, index), error));
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
                for (value, &bits) in keys.values.iter().zip(&state.0) {
                    if bits & OPEN != 0 {
                        errors.push(((block_index, usize::MAX), format!("{value} may still be open when block {block_index} leaves the function")));
                    }
                }
                // The returned or thrown value goes to the caller or the handler; any other free one has no owner.
                let handed = match &block.term.kind {
                    TermKind::Ret(value) => *value,
                    TermKind::Throw(value) => Some(*value),
                    _ => None,
                };
                for (value, &bits) in owners.values.iter().zip(&state.0[base..]) {
                    if bits & FREE != 0 && Some(*value) != handed {
                        errors.push((
                            (block_index, usize::MAX),
                            format!("{value} may be in no region when block {block_index} leaves the function"),
                        ));
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
    // The first error in program order (block, then instruction, the exit
    // after the instructions), not the first by text (#1007).
    errors.sort();
    errors.dedup();
    match errors.first() {
        Some((_, error)) => Err(error.clone()),
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
