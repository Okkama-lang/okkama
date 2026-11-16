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
    // Every edge, unwind and catch edges included: a field bound in a `try` is undef on its handler's path.
    loop {
        let count = undefined.len();
        for (_, block) in body.blocks.iter() {
            for (target, args) in edges(block) {
                for (value, &param) in args.iter().zip(&body.blocks.get(target)?.params) {
                    if value.is_some_and(|value| undefined.contains(&value)) {
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
        // path to the allocation passes a Cond on that field's flag (#770), whose other edge traps as Tomato does.
        let guards = if *construction == ir::ConstructionStatus::Complete {
            let flags = field_flags(module, body, *declaration, layout.len(), &HashSet::new())?;
            guarded(body, allocation, &flags)?
        } else {
            HashSet::new()
        };
        let unguarded = |index: usize, value: &ir::ValueId| undefined.contains(value)
            && (direct_undef(body, *value) || !guards.contains(&index));
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
        let flags = field_flags(module, body, *declaration, layout.len(), &aliases)?;
        let identity = Identity { allocation, fields, construction: *construction, aliases: &aliases, pending };
        check_identity(body, &identity, &undefined, &flags)?;
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

pub(super) fn direct_undef(body: &ir::Body, value: ir::ValueId) -> bool {
    body.values.get(value).is_ok_and(|data| matches!(data.def, ir::Definition::InstResult { inst, .. }
        if body.insts.get(inst).is_ok_and(|inst| matches!(inst.op, ir::Operation::Core(ir::CoreOp::Undef { .. })))))
}

/// The fields whose bound-flag checks every path to `allocation` passes (#708): from its block up a chain of
/// blocks, each the `yes` target of its only predecessor's `Cond` on a field's flag (`flags`, #770), whose `no`
/// target traps with `TrapCode::Unbound`.
fn guarded(body: &ir::Body, allocation: ir::InstId, flags: &[HashSet<ir::ValueId>]) -> Result<HashSet<usize>> {
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
    let mut fields = HashSet::new();
    let mut seen = HashSet::new();
    while let Some(block) = current.take().filter(|block| seen.insert(*block)) {
        let Some([source]) = predecessors.get(&block).map(Vec::as_slice) else { break };
        let checked = bound_behind(body, &body.blocks.get(*source)?.term.kind, block, flags)?;
        if checked.is_empty() {
            break;
        }
        fields.extend(checked);
        current = Some(*source);
    }
    Ok(fields)
}

/// The bound flags of each field of `declaration` (#708, #770): the bool values whose every definition is
/// `const false`, a `const true` that follows a write of that field in its block (a `stage` of the field, or
/// an `obj.init` of it on `self` through `aliases`), or a block parameter whose every incoming value is a flag
/// of the field. A `true` flag therefore proves, on every path that carries it, that the field was written:
/// the check of the flag, not its trap's message, links the check to the field.
pub(super) fn field_flags(module: &ir::Module, body: &ir::Body, declaration: ir::DeclId, count: usize,
    aliases: &HashSet<ir::ValueId>) -> Result<Vec<HashSet<ir::ValueId>>> {
    let mut flags = vec![HashSet::new(); count];
    for (_, block) in body.blocks.iter() {
        let mut written = HashSet::new();
        for &id in &block.insts {
            let inst = body.insts.get(id)?;
            match &inst.op {
                ir::Operation::Lexical(ir::LexicalOp::Stage { field, .. }) if field.declaration == declaration => {
                    written.insert(field.index as usize);
                }
                ir::Operation::Core(ir::CoreOp::ObjInit { object, field, .. }) if aliases.contains(object) => {
                    written.insert(field.index as usize);
                }
                ir::Operation::Const(ir::Literal::Bool(value)) => {
                    for (index, flag) in flags.iter_mut().enumerate() {
                        if !*value || written.contains(&index) {
                            flag.extend(inst.results.iter().copied());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // Every edge into a block with its arguments (a produced result is never a flag).
    let mut incoming: HashMap<ir::BlockId, Vec<Vec<Option<ir::ValueId>>>> = HashMap::new();
    for (_, block) in body.blocks.iter() {
        for (target, mut args) in edges(block) {
            // An unwind edge passes the leading parameters; the failure follows them.
            if block.unwind.as_ref().is_some_and(|unwind| unwind.target.block == target) {
                let count = body.blocks.get(target)?.params.len();
                if args.len() < count {
                    args.resize(count, None);
                }
            }
            incoming.entry(target).or_default().push(args);
        }
    }
    let bool_ty = module.types.iter().find(|(_, ty)| **ty == ir::Type::Bool).map(|(id, _)| id);
    let mut candidates = vec![];
    for (id, block) in body.blocks.iter() {
        let Some(into) = incoming.get(&id) else { continue };
        if id == body.entry || into.iter().any(|args| args.len() != block.params.len()) {
            continue;
        }
        for (index, &param) in block.params.iter().enumerate() {
            if bool_ty.is_some_and(|ty| body.values.get(param).is_ok_and(|value| value.ty == ty)) {
                candidates.push((id, index, param));
            }
        }
    }
    for flag in &mut flags {
        flag.extend(candidates.iter().map(|&(_, _, param)| param));
        loop {
            let before = flag.len();
            for &(block, index, param) in &candidates {
                let passes = |args: &Vec<Option<ir::ValueId>>| args[index].is_some_and(|value| flag.contains(&value));
                if flag.contains(&param) && !incoming[&block].iter().all(passes) {
                    flag.remove(&param);
                }
            }
            if flag.len() == before {
                break;
            }
        }
    }
    Ok(flags)
}

/// The edges out of `block`, ordinary and unwind, with their arguments; `None` for a produced result.
fn edges(block: &ir::Block) -> Vec<(ir::BlockId, Vec<Option<ir::ValueId>>)> {
    let plain = |target: &ir::Target| (target.block, target.args.iter().copied().map(Some).collect());
    let produced = |edge: &ir::InvokeEdge| (edge.block, edge.args.iter().map(|arg| match arg {
        ir::EdgeArg::Value(value) => Some(*value),
        ir::EdgeArg::Produced(_) => None,
    }).collect());
    let mut edges: Vec<_> = block.unwind.iter().map(|unwind| plain(&unwind.target)).collect();
    match &block.term.kind {
        ir::TermKind::Br(target) => edges.push(plain(target)),
        ir::TermKind::Cond { yes, no, .. } => edges.extend([plain(yes), plain(no)]),
        ir::TermKind::Switch { cases, default, .. } => {
            edges.extend(cases.iter().map(|case| plain(&case.target)));
            edges.push(plain(default));
        }
        ir::TermKind::Catch { handler, next, .. } => edges.extend([produced(handler), plain(next)]),
        ir::TermKind::Lookup { found, missing, .. } => edges.extend([produced(found), plain(missing)]),
        ir::TermKind::InvokeOwn { normal, unwind, .. } => edges.extend([produced(normal), produced(unwind)]),
        ir::TermKind::InvokeDefer { continue_edge, replacements, unwind, .. } => {
            edges.push(produced(continue_edge));
            edges.extend(replacements.iter().map(|replacement| produced(&replacement.edge)));
            edges.extend(unwind.iter().map(produced));
        }
        ir::TermKind::CleanupNext { selected, done, .. } => edges.extend([produced(selected), produced(done)]),
        ir::TermKind::ExitDispatch { success, pending, failed, .. } => {
            edges.extend([success, pending, failed].into_iter().flatten().map(produced));
        }
        ir::TermKind::FailureDispatch { none, some, .. } => edges.extend([plain(none), produced(some)]),
        _ => {}
    }
    edges
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

/// The fields a `Cond` edge into `target` proves initialized (#746, #770): the `yes` edge of a check of a field's
/// bound flag (`flags`, in layout order), whose `no` target traps with `TrapCode::Unbound`. An early self reaches
/// its completion after the producer's flag checks of the fields a path may have left unbound; behind the check,
/// such a field is initialized on every path that goes on.
pub(super) fn bound_behind(body: &ir::Body, term: &ir::TermKind, target: ir::BlockId,
    flags: &[HashSet<ir::ValueId>]) -> Result<Vec<usize>> {
    let ir::TermKind::Cond { condition, yes, no } = term else {
        return Ok(vec![]);
    };
    if yes.block != target || no.block == target {
        return Ok(vec![]);
    }
    let ir::TermKind::Trap { code: ir::TrapCode::Unbound, .. } = &body.blocks.get(no.block)?.term.kind else {
        return Ok(vec![]);
    };
    Ok(flags.iter().enumerate().filter(|(_, flag)| flag.contains(condition)).map(|(index, _)| index).collect())
}

fn check_identity(
    body: &ir::Body,
    identity: &Identity<'_>,
    undefined: &HashSet<ir::ValueId>,
    flags: &[HashSet<ir::ValueId>],
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
                for index in bound_behind(body, &block.term.kind, edge.block, flags)? {
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
