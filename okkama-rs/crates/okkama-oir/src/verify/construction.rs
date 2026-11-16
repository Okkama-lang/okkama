use super::{EntityLocation as L, RuleId as R, State};
use crate::*;
use std::collections::{BTreeSet, VecDeque};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin { Instruction(InstId), Invoke(BlockId) }
struct Creation<'a> {
    origin: Origin,
    initial: &'a [ValueId],
    status: ConstructionStatus,
    layout: &'a [Field],
    pending_layout: bool,
    aliases: BTreeSet<ValueId>,
}
fn undef(body: &Body, value: ValueId) -> bool {
    body.values.get(value).is_ok_and(|v| match v.def {
        Definition::InstResult { inst, .. } => body.insts.get(inst)
            .is_ok_and(|i| matches!(i.op, Operation::Core(CoreOp::Undef { .. }))),
        _ => false,
    })
}
pub(super) fn check(module: &Module, function: FuncId, body: &Body, state: &mut State<'_>) {
    let edges = super::graph::edges(body, state);
    let instructions = body.insts.iter().filter_map(|(id, inst)| {
        inst.results.first().map(|value| (Origin::Instruction(id), &inst.op, vec![*value]))
    });
    let invokes = body.blocks.iter().filter_map(|(id, block)| {
        let TermKind::InvokeOwn { op, normal, .. } = &block.term.kind else { return None; };
        let target = body.blocks.get(normal.block).ok()?;
        let aliases = normal.args.iter().zip(&target.params).filter_map(|(arg, param)| {
            matches!(arg, EdgeArg::Produced(EdgeResult { outcome: Outcome::Normal, slot: 0 })).then_some(*param)
        }).collect();
        Some((Origin::Invoke(id), op.as_ref(), aliases))
    });
    let mut seeds = BTreeSet::new();
    for (origin, op, origin_values) in instructions.chain(invokes) {
        let Operation::Core(CoreOp::ObjNew { declaration, fields, construction, .. }) = op else { continue; };
        let Ok(decl) = module.declarations.get(*declaration) else { continue; };
        let DeclarationKind::Object { fields: layout, .. } = &decl.kind else { continue; };
        seeds.extend(&origin_values);
        let pending_layout = layout.last().is_some_and(|field| field.name == "$pending"
            && module.types.get(field.ty) == Ok(&Type::Bool));
        let mut creation = Creation {
            origin, initial: fields, status: *construction, layout, pending_layout,
            aliases: origin_values.into_iter().collect(),
        };
        let mut changed = true;
        while changed {
            changed = false;
            for edge in &edges {
                let Ok(target) = body.blocks.get(edge.target) else { continue; };
                for (arg, param) in edge.args.iter().zip(&target.params) {
                    if !state.step(1) || !state.memory(32) { return; }
                    if let EdgeArg::Value(value) = arg
                        && creation.aliases.contains(value) && creation.aliases.insert(*param) {
                        changed = true;
                    }
                }
            }
        }
        analyze(module, function, body, &edges, &creation, state);
    }
    let valid_self = definite_self(body, &edges, seeds, state);
    for (instruction, inst) in body.insts.iter() {
        state.at(L::Instruction { function, instruction }, inst.loc);
        valid_init(&inst.op, &valid_self, state);
    }
    for (block_id, block) in body.blocks.iter() {
        if let TermKind::InvokeOwn { op, .. } = &block.term.kind {
            state.at(L::Terminator { function, block: block_id }, block.term.loc);
            valid_init(op, &valid_self, state);
        }
    }
}
fn valid_init(op: &Operation, valid: &BTreeSet<ValueId>, state: &mut State<'_>) {
    if let Operation::Core(CoreOp::ObjInit { object, .. }) = op
        && !valid.contains(object) {
        state.error(R::A2, "obj.init requires an early self created by this constructor");
    }
}
fn analyze(
    module: &Module,
    function: FuncId,
    body: &Body,
    edges: &[super::graph::Edge],
    c: &Creation<'_>,
    state: &mut State<'_>,
) {
    let width = c.layout.len() + 1;
    if !state.array(body.blocks.len(), width.saturating_mul(4).saturating_add(64)) { return; }
    // 0 = this origin absent, 1 = uninitialized/incomplete, 2 = initialized/completed.
    // Union ignores absent alternatives; each SSA phi is also checked for definite local origins.
    // The final cell is the construction state. Creation resets each incarnation.
    let Some(flags) = field_flags(module, body, edges, c, state) else { return; };
    let mut inputs: Vec<Option<Vec<u8>>> = vec![None; body.blocks.len()];
    inputs[body.entry.as_u32() as usize] = Some(vec![0; width]);
    let mut queue = VecDeque::from([body.entry]);
    let mut queued = vec![false; body.blocks.len()];
    queued[body.entry.as_u32() as usize] = true;
    while let Some(block_id) = queue.pop_front() {
        if !state.step(1) { return; }
        queued[block_id.as_u32() as usize] = false;
        let Some(input) = inputs[block_id.as_u32() as usize].clone() else { continue; };
        let Ok(block) = body.blocks.get(block_id) else { continue; };
        let mut output = input.clone();
        let mut exceptional: Option<Vec<u8>> = None;
        for &id in &block.insts {
            if !state.step(width) { return; }
            if let Ok(inst) = body.insts.get(id) {
                if ContractRegistry::new(module).construction_schema(&inst.op, body)
                    .is_ok_and(|contract| !contract.effects.failures.is_empty() || contract.effects.suspends) {
                    join_failure(&mut exceptional, &output);
                }
                transfer(body, &inst.op, Origin::Instruction(id), c, &mut output, None);
            }
        }
        if matches!(block.term.kind, TermKind::Throw(_) | TermKind::Raise(_) | TermKind::Panic(_)
            | TermKind::Trap { .. }) {
            join_failure(&mut exceptional, &output);
        }
        for edge in edges.iter().filter(|edge| edge.source == block_id) {
            if !state.step(width) { return; }
            let mut next = if edge.entry {
                let Some(exceptional) = &exceptional else { continue; };
                exceptional.clone()
            } else {
                output.clone()
            };
            if let TermKind::InvokeOwn { op, normal, .. } = &block.term.kind
                && !edge.entry && edge.target == normal.block && edge.args == normal.args {
                transfer(body, op, Origin::Invoke(block_id), c, &mut next, None);
            }
            if !edge.entry && next[c.layout.len()] == 1 {
                for index in bound_behind(body, &block.term.kind, edge.target, &flags) {
                    next[index] = 2;
                }
            }
            let slot = &mut inputs[edge.target.as_u32() as usize];
            let changed = match slot {
                None => {
                    *slot = Some(next);
                    true
                }
                Some(old) => {
                    let mut changed = false;
                    for (old, new) in old.iter_mut().zip(next) {
                        let merged = *old | new;
                        changed |= merged != *old;
                        *old = merged;
                    }
                    changed
                }
            };
            let target = edge.target.as_u32() as usize;
            if changed && !queued[target] {
                queued[target] = true;
                queue.push_back(edge.target);
            }
        }
    }
    // Diagnose only after reaching the fixpoint.
    for (block_id, block) in body.blocks.iter() {
        let Some(mut values) = inputs[block_id.as_u32() as usize].clone() else { continue; };
        for &id in &block.insts {
            if !state.step(width) { return; }
            if let Ok(inst) = body.insts.get(id) {
                state.at(L::Instruction { function, instruction: id }, inst.loc);
                transfer(body, &inst.op, Origin::Instruction(id), c, &mut values, Some(state));
            }
        }
        state.at(L::Terminator { function, block: block_id }, block.term.loc);
        if let TermKind::InvokeOwn { op, .. } = &block.term.kind {
            transfer(body, op, Origin::Invoke(block_id), c, &mut values, Some(state));
        }
        if let TermKind::Ret { values: returned } = &block.term.kind
            && returned.iter().any(|v| c.aliases.contains(v))
            && values[c.layout.len()] != 0 && values[c.layout.len()] != 2 {
            state.error(R::A2, "constructor returns an identity before completion");
        }
    }
}
fn transfer(
    body: &Body,
    op: &Operation,
    origin: Origin,
    c: &Creation<'_>,
    values: &mut [u8],
    mut state: Option<&mut State<'_>>,
) {
    if origin == c.origin {
        let incomplete = c.initial.iter().any(|value| undef(body, *value));
        let pending_unbound = c.initial.last().is_some_and(|value| undef(body, *value));
        let pending_false = c.initial.last().is_some_and(|value| is_false(body, *value));
        let invalid = match c.status {
            ConstructionStatus::Complete => incomplete || (c.pending_layout && !pending_false),
            ConstructionStatus::Incomplete => !c.pending_layout || !pending_unbound,
        };
        if invalid && let Some(state) = state.as_mut() {
            state.error(R::A2, "construction status does not match complete fields or the early-self layout");
        }
        for (slot, value) in values.iter_mut().zip(c.initial) {
            *slot = if undef(body, *value) { 1 } else { 2 };
        }
        values[c.layout.len()] = if c.status == ConstructionStatus::Complete { 2 } else { 1 };
    }
    if values[c.layout.len()] == 0 { return; }
    if let Operation::Core(CoreOp::ObjGet { object, field } | CoreOp::OnceLoad { object, field }) = op
        && c.aliases.contains(object) && values.get(field.index as usize).is_some_and(|v| *v != 2)
        && let Some(state) = state.as_mut() {
        state.error(R::A2, "constructor reads an uninitialized field of its early self");
    }
    let Operation::Core(CoreOp::ObjInit { object, field, value, completion }) = op else { return; };
    if !c.aliases.contains(object) { return; }
    let index = field.index as usize;
    let Some(slot) = values.get(index).copied() else { return; };
    let is_pending = c.layout.get(index).is_some_and(|field| field.name == "$pending");
    let invalid = values[c.layout.len()] != 1 || slot != 1
        || (completion.is_some() && !is_pending)
        || (is_pending && (values[..c.layout.len()].iter().enumerate().any(|(i, v)| i != index && *v != 2)
            || !is_false(body, *value)));
    if invalid && let Some(state) = state.as_mut() {
        state.error(R::A2, "field already initialized, self complete, or completion precedes field initialization");
    }
    values[index] = 2;
    if is_pending { values[c.layout.len()] = 2; }
}
/// The fields of an incomplete self that the `yes` edge of a bound-flag check into `target` proves initialized
/// (A2, #746, #770): the `cond` reads the flag of the field (`flags`), and its `no` target traps with
/// `TrapCode::Unbound`, directly or through empty `br` blocks (own splits the edges). The producer keeps a flag per
/// field that a path may leave unbound and checks it before the completion, as Tomato checks its field flags
/// after the body.
fn bound_behind(body: &Body, term: &TermKind, target: BlockId, flags: &[BTreeSet<ValueId>]) -> Vec<usize> {
    let TermKind::Cond { condition, yes, no } = term else { return vec![]; };
    if yes.block != target || no.block == target || !unbound_trap(body, no.block) { return vec![]; }
    flags.iter().enumerate().filter(|(_, flag)| flag.contains(condition)).map(|(index, _)| index).collect()
}
/// The bound flags of each field of the incomplete self (A2, #770): the bool values whose every definition is
/// `const false`, a `const true` that follows an `obj.init` of that field on the self in its block, or a block
/// parameter whose every incoming edge passes a flag of the field. A `true` flag proves, on every path that
/// carries it, that the field was initialized. `None` when the work budget is exhausted.
fn field_flags(module: &Module, body: &Body, edges: &[super::graph::Edge], c: &Creation<'_>,
    state: &mut State<'_>)
    -> Option<Vec<BTreeSet<ValueId>>> {
    let mut flags = vec![BTreeSet::new(); c.layout.len()];
    for (_, block) in body.blocks.iter() {
        let mut written = BTreeSet::new();
        for &id in &block.insts {
            if !state.step(1) { return None; }
            let Ok(inst) = body.insts.get(id) else { continue; };
            match &inst.op {
                Operation::Core(CoreOp::ObjInit { object, field, .. }) if c.aliases.contains(object) => {
                    written.insert(field.index as usize);
                }
                Operation::Const(Literal::Bool(value)) => {
                    for (index, flag) in flags.iter_mut().enumerate() {
                        if !*value || written.contains(&index) { flag.extend(inst.results.iter().copied()); }
                    }
                }
                _ => {}
            }
        }
    }
    // A parameter is a candidate when every edge into its block passes an argument for each parameter.
    let mut incoming: Vec<Vec<&super::graph::Edge>> = vec![vec![]; body.blocks.len()];
    for edge in edges {
        if let Some(slot) = incoming.get_mut(edge.target.as_u32() as usize) { slot.push(edge); }
    }
    let mut candidates = vec![];
    for (id, block) in body.blocks.iter() {
        let into = &incoming[id.as_u32() as usize];
        // An unwind edge passes the leading parameters; the failure follows them.
        if id == body.entry || into.is_empty() || into.iter().any(|edge| edge.args.len() > block.params.len()
            || !edge.entry && edge.args.len() != block.params.len()) {
            continue;
        }
        for (index, &param) in block.params.iter().enumerate() {
            if body.values.get(param).is_ok_and(|value| module.types.get(value.ty) == Ok(&Type::Bool)) {
                candidates.push((id, index, param));
            }
        }
    }
    for flag in &mut flags {
        flag.extend(candidates.iter().map(|&(_, _, param)| param));
        let mut changed = true;
        while changed {
            changed = false;
            for &(block, index, param) in &candidates {
                if !state.step(1) { return None; }
                let passes = |edge: &&super::graph::Edge| matches!(edge.args.get(index), Some(EdgeArg::Value(value))
                    if flag.contains(value));
                if flag.contains(&param) && !incoming[block.as_u32() as usize].iter().all(passes) {
                    flag.remove(&param);
                    changed = true;
                }
            }
        }
    }
    Some(flags)
}
/// Whether `block` reaches a `TrapCode::Unbound` trap through empty `br` blocks.
fn unbound_trap(body: &Body, mut block: BlockId) -> bool {
    let mut seen = BTreeSet::new();
    while seen.insert(block) {
        let Ok(data) = body.blocks.get(block) else { return false; };
        match &data.term.kind {
            TermKind::Trap { code: TrapCode::Unbound, .. } => return true,
            TermKind::Br(next) if data.insts.is_empty() => block = next.block,
            _ => return false,
        }
    }
    false
}
pub(super) fn is_false(body: &Body, value: ValueId) -> bool {
    body.values.get(value).is_ok_and(|v| match v.def {
        Definition::InstResult { inst, .. } => body.insts.get(inst)
            .is_ok_and(|i| i.op == Operation::Const(Literal::Bool(false))),
        _ => false,
    })
}

fn definite_self(
    body: &Body,
    edges: &[super::graph::Edge],
    seeds: BTreeSet<ValueId>,
    state: &mut State<'_>,
) -> BTreeSet<ValueId> {
    if !state.array(body.values.len(), 64) { return seeds; }
    let mut valid = seeds.clone();
    // Greatest fixpoint keeps loop-carried self, but removes any phi with an
    // incoming value that is not a constructor-local identity.
    for (block, data) in body.blocks.iter() {
        if block != body.entry { valid.extend(&data.params); }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for edge in edges {
            let Ok(target) = body.blocks.get(edge.target) else { continue; };
            for (arg, param) in edge.args.iter().zip(&target.params) {
                if !state.step(1) { return valid; }
                let local = match arg {
                    EdgeArg::Value(value) => valid.contains(value),
                    EdgeArg::Produced(result) => result.outcome == Outcome::Normal && result.slot == 0
                        && body.blocks.get(edge.source).is_ok_and(|block| match &block.term.kind {
                            TermKind::InvokeOwn { op, normal, .. } => normal.block == edge.target
                                && normal.args == edge.args && matches!(**op, Operation::Core(CoreOp::ObjNew { .. })),
                            _ => false,
                        }),
                };
                if !local && valid.remove(param) { changed = true; }
            }
        }
    }
    valid
}

fn join_failure(target: &mut Option<Vec<u8>>, values: &[u8]) {
    match target {
        None => *target = Some(values.to_vec()),
        Some(old) => {
            for (old, new) in old.iter_mut().zip(values) { *old |= new; }
        }
    }
}
