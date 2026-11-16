//! Independent producer self-check. This is not the semantic verifier from #564.
use std::collections::{HashMap, HashSet, VecDeque};

use super::{LowerError, Result};
use crate::oir2 as ir;

fn error(message: &str) -> LowerError {
    LowerError::invalid(crate::source::Span::default(), format!("OIR-2 construction: {message}"))
}

fn targets(term: &ir::TermKind) -> Vec<&ir::Target> {
    match term {
        ir::TermKind::Br(target) => vec![target],
        ir::TermKind::Cond { yes, no, .. } => vec![yes, no],
        ir::TermKind::Switch { cases, default, .. } => {
            cases.iter().map(|case| &case.target).chain([default]).collect()
        }
        _ => vec![],
    }
}

pub(super) fn undefined_values(body: &ir::Body) -> Result<HashSet<ir::ValueId>> {
    let mut undefined = HashSet::new();
    for (_, inst) in body.insts.iter() {
        if matches!(inst.op, ir::Operation::Core(ir::CoreOp::Undef { .. })) {
            undefined.extend(inst.results.iter().copied());
        }
    }
    loop {
        let count = undefined.len();
        for (_, block) in body.blocks.iter() {
            for edge in targets(&block.term.kind) {
                for (&value, &param) in edge.args.iter().zip(&body.blocks.get(edge.block)?.params) {
                    if undefined.contains(&value) {
                        undefined.insert(param);
                    }
                }
            }
        }
        if undefined.len() == count {
            break;
        }
    }
    Ok(undefined)
}

pub(super) fn check(module: &ir::Module, body: &ir::Body) -> Result<()> {
    let undefined = undefined_values(body)?;
    let mut local_objects = HashSet::new();
    for (allocation, inst) in body.insts.iter() {
        let ir::Operation::Core(ir::CoreOp::ObjNew { declaration, fields, construction, .. }) = &inst.op else {
            continue;
        };
        let ir::DeclarationKind::Object { fields: layout, .. } = &module.declarations.get(*declaration)?.kind else {
            return Err(error("obj.new has no object layout"));
        };
        if fields.len() != layout.len() {
            return Err(error("allocation fields do not match layout"));
        }
        let pending = layout.last().is_some_and(|field| field.name == "$pending"
            && module.types.get(field.ty) == Ok(&ir::Type::Bool));
        if *construction == ir::ConstructionStatus::Incomplete
            && (!pending || !fields.last().is_some_and(|value| undefined.contains(value))) {
            return Err(error("incomplete self requires trailing $pending: bool = undef"));
        }
        // A complete self takes a field that may be unbound only after the check of its bound flag (#708): every
        // path to the allocation passes the flag's Cond, whose other edge traps as Tomato does.
        let guards = if *construction == ir::ConstructionStatus::Complete {
            guarded(body, allocation)?
        } else {
            HashSet::new()
        };
        let object = short_name(&module.declarations.get(*declaration)?.name);
        let unguarded = |index: usize, value: &ir::ValueId| undefined.contains(value)
            && (direct_undef(body, *value)
                || !guards.contains(&super::objects::unbound_message(&object, &layout[index].name)));
        if *construction == ir::ConstructionStatus::Complete
            && (fields.iter().enumerate().any(|(index, value)| unguarded(index, value))
                || (pending && !fields.last().is_some_and(|value| is_false(body, *value)))) {
            return Err(error("complete self has uninitialized fields or non-false $pending"));
        }
        let mut undefined = undefined.clone();
        for (index, value) in fields.iter().enumerate() {
            if !unguarded(index, value) {
                undefined.remove(value);
            }
        }
        let mut aliases: HashSet<_> = inst.results.iter().copied().collect();
        loop {
            let count = aliases.len();
            for (_, block) in body.blocks.iter() {
                for edge in targets(&block.term.kind) {
                    for (&value, &param) in edge.args.iter().zip(&body.blocks.get(edge.block)?.params) {
                        if aliases.contains(&value) {
                            aliases.insert(param);
                        }
                    }
                }
            }
            if aliases.len() == count {
                break;
            }
        }
        local_objects.extend(aliases.iter().copied());
        let messages: Vec<_> = layout.iter().map(|field| super::objects::unbound_message(&object, &field.name))
            .collect();
        let identity = Identity { allocation, fields, construction: *construction, aliases: &aliases, pending };
        check_identity(body, &identity, &undefined, &messages)?;
    }
    for (_, inst) in body.insts.iter() {
        if let ir::Operation::Core(ir::CoreOp::ObjInit { object, .. }) = inst.op
            && !local_objects.contains(&object) {
            return Err(error("obj.init does not target constructor-local self"));
        }
    }
    Ok(())
}

/// The object's name as Tomato's messages give it: without its module, or its generic instance after `::`.
pub(super) fn short_name(name: &str) -> String {
    match name.rsplit_once("::") {
        Some((_, instance)) => instance.to_owned(),
        None => name.rsplit('.').next().unwrap_or(name).to_owned(),
    }
}

fn direct_undef(body: &ir::Body, value: ir::ValueId) -> bool {
    body.values.get(value).is_ok_and(|data| matches!(data.def, ir::Definition::InstResult { inst, .. }
        if body.insts.get(inst).is_ok_and(|inst| matches!(inst.op, ir::Operation::Core(ir::CoreOp::Undef { .. })))))
}

/// The messages of the bound-flag checks that every path to `allocation` passes (#708): from its block up a chain
/// of blocks, each the `yes` target of its only predecessor's `Cond`, whose `no` target traps with
/// `TrapCode::Unbound`.
fn guarded(body: &ir::Body, allocation: ir::InstId) -> Result<HashSet<String>> {
    let mut predecessors: HashMap<ir::BlockId, Vec<ir::BlockId>> = HashMap::new();
    let mut current = None;
    for (id, block) in body.blocks.iter() {
        if block.insts.contains(&allocation) {
            current = Some(id);
        }
        for edge in targets(&block.term.kind) {
            predecessors.entry(edge.block).or_default().push(id);
        }
        if let Some(unwind) = &block.unwind {
            predecessors.entry(unwind.target.block).or_default().push(id);
        }
    }
    let mut messages = HashSet::new();
    let mut seen = HashSet::new();
    while let Some(block) = current.take().filter(|block| seen.insert(*block)) {
        let Some([source]) = predecessors.get(&block).map(Vec::as_slice) else { break };
        let ir::TermKind::Cond { yes, no, .. } = &body.blocks.get(*source)?.term.kind else { break };
        if yes.block != block || no.block == block {
            break;
        }
        let ir::TermKind::Trap { code: ir::TrapCode::Unbound, message } = &body.blocks.get(no.block)?.term.kind
        else {
            break;
        };
        messages.insert(message.clone());
        current = Some(*source);
    }
    Ok(messages)
}

fn is_false(body: &ir::Body, value: ir::ValueId) -> bool {
    let Ok(data) = body.values.get(value) else {
        return false;
    };
    let ir::Definition::InstResult { inst, .. } = data.def else {
        return false;
    };
    body.insts.get(inst).is_ok_and(|inst| inst.op == ir::Operation::Const(ir::Literal::Bool(false)))
}

/// One self allocation of a constructor body and the values that alias it.
struct Identity<'a> {
    allocation: ir::InstId,
    fields: &'a [ir::ValueId],
    construction: ir::ConstructionStatus,
    aliases: &'a HashSet<ir::ValueId>,
    pending: bool,
}

/// The fields a `Cond` edge into `target` proves initialized (#746): the `yes` edge of a bound-flag check, whose
/// `no` target traps with `TrapCode::Unbound` and Tomato's message for the field (`messages`, in layout order).
/// An early self reaches its completion after the producer's flag checks of the fields a path may have left
/// unbound; behind the check, such a field is initialized on every path that goes on.
pub(super) fn bound_behind(body: &ir::Body, term: &ir::TermKind, target: ir::BlockId, messages: &[String])
    -> Result<Vec<usize>> {
    let ir::TermKind::Cond { yes, no, .. } = term else {
        return Ok(vec![]);
    };
    if yes.block != target || no.block == target {
        return Ok(vec![]);
    }
    let ir::TermKind::Trap { code: ir::TrapCode::Unbound, message } = &body.blocks.get(no.block)?.term.kind else {
        return Ok(vec![]);
    };
    Ok(messages.iter().enumerate().filter(|(_, expected)| *expected == message).map(|(index, _)| index).collect())
}

fn check_identity(
    body: &ir::Body,
    identity: &Identity<'_>,
    undefined: &HashSet<ir::ValueId>,
    messages: &[String],
) -> Result<()> {
    let Identity { allocation, fields, construction, aliases, pending } = *identity;
    // Each cell is a set: absent (0), uninitialized (1), initialized (2).
    // Union at joins makes reads and completion valid on every incoming path.
    let mut incoming = HashMap::from([(body.entry, vec![0u8; fields.len() + 1])]);
    let mut queue = VecDeque::from([body.entry]);
    while let Some(id) = queue.pop_front() {
        let block = body.blocks.get(id)?;
        let mut state = incoming[&id].clone();
        for &instruction in &block.insts {
            let inst = body.insts.get(instruction)?;
            if instruction == allocation {
                for (slot, field) in state.iter_mut().zip(fields) {
                    *slot = if undefined.contains(field) { 1 } else { 2 };
                }
                state[fields.len()] = if construction == ir::ConstructionStatus::Complete { 2 } else { 1 };
            }
            if state[fields.len()] == 0 {
                continue;
            }
            match &inst.op {
                ir::Operation::Core(ir::CoreOp::ObjGet { object, field }) if aliases.contains(object) => {
                    if state[field.index as usize] != 2 {
                        return Err(error("read of an uninitialized field"));
                    }
                }
                ir::Operation::Core(ir::CoreOp::ObjInit { object, field, value, completion })
                    if aliases.contains(object) => {
                    let index = field.index as usize;
                    if index >= fields.len() || completion.is_some() || state[fields.len()] != 1
                        || state[index] != 1 || undefined.contains(value) {
                        return Err(error("duplicate initialization or completion of self"));
                    }
                    if pending && index + 1 == fields.len() {
                        if state[..index].iter().any(|slot| *slot != 2) || !is_false(body, *value) {
                            return Err(error("completion before field initialization"));
                        }
                        state[fields.len()] = 2;
                    }
                    state[index] = 2;
                }
                _ => {}
            }
        }
        if let ir::TermKind::Ret { values } = &block.term.kind
            && values.iter().any(|value| aliases.contains(value)) && state[fields.len()] != 2 {
            return Err(error("return of incomplete self"));
        }
        for edge in targets(&block.term.kind) {
            let mut state = state.clone();
            if state[fields.len()] == 1 {
                for index in bound_behind(body, &block.term.kind, edge.block, messages)? {
                    state[index] = 2;
                }
            }
            let changed = if let Some(old) = incoming.get_mut(&edge.block) {
                let previous = old.clone();
                for (slot, next) in old.iter_mut().zip(&state) {
                    *slot |= next;
                }
                *old != previous
            } else {
                incoming.insert(edge.block, state.clone());
                true
            };
            if changed {
                queue.push_back(edge.block);
            }
        }
    }
    Ok(())
}
