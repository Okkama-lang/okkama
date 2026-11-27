//! #1112 (stage 1): direct calls of small leaf callees that cannot fail are replaced by the callee's body, so that
//! the scalar replacement after it sees a constructor, its successor and their accessors in one function.
//!
//! A callee qualifies when it is one block ending in `ret`, without an unwind target, of at most
//! [`INLINE_INSTRUCTIONS`] instructions, each of which can neither fail nor suspend nor touch a region or a
//! resource: constants, `int` addition, subtraction, multiplication and negation, comparisons, `bool` negation,
//! `obj.new` / `obj.next` of plain objects (no destroy, no `once` field, not builtin, nothing in a region),
//! `obj.get` and `succ.take`. Only FAILED records an activation in a failure's trace, so a callee that cannot fail
//! leaves every report as it was. Its entry step is not counted any more (owner, 2026-11-27: `--max-steps` is a
//! guard against runaway programs, not an exact count).
//!
//! The caller is marked [`Attr::Inlined`] with each callee, which grants the verifier the callee's own rights:
//! `obj.new` of a constructor, `obj.next` of a successor, `obj.get` of the object's functions. Source
//! encapsulation was checked by the analyser before lowering.
use super::{Attr, FuncId, Function, InstKind, IntBinaryOp, IntUnaryOp, Module, ResultType, TermKind, Type, ValueId};
use super::regions::Plain;

/// The largest callee inlined, in instructions.
pub const INLINE_INSTRUCTIONS: usize = 24;

/// Inline every qualifying direct call of the module (one round: a qualifying callee calls nothing).
pub fn inline_leaves(module: &mut Module) {
    let plain = Plain::of(module);
    let leaves: Vec<bool> = module.functions.iter().map(|function| is_leaf(module, &plain, function)).collect();
    for index in 0..module.functions.len() {
        if module.functions[index].is_async() || module.functions[index].blocks.is_empty() {
            continue;
        }
        let mut function = std::mem::replace(&mut module.functions[index], empty_function());
        inline_into(module, &leaves, FuncId(index), &mut function);
        module.functions[index] = function;
    }
}

fn empty_function() -> Function {
    Function {
        symbol: String::new(),
        params: Vec::new(),
        result: ResultType::Void,
        attrs: Vec::new(),
        values: Vec::new(),
        blocks: Vec::new(),
        span: Default::default(),
    }
}

/// Can `function` be inlined at a direct call?
fn is_leaf(module: &Module, plain: &Plain, function: &Function) -> bool {
    if function.is_async() || function.blocks.len() != 1 {
        return false;
    }
    let qualified = function.attrs.iter().all(|attr| {
        matches!(attr, Attr::Method(_) | Attr::Ctor(_) | Attr::Successor(_) | Attr::Hidden | Attr::Inlined(_))
    });
    if !qualified {
        return false;
    }
    let block = &function.blocks[0];
    if block.unwind.is_some() || !block.params.is_empty() || block.insts.len() > INLINE_INSTRUCTIONS {
        return false;
    }
    if !matches!(block.term.kind, TermKind::Ret(_)) {
        return false;
    }
    let plain_object = |object: super::ObjectId| {
        module.object(object).is_some_and(|declared| {
            !declared.builtin
                && declared.destroy.is_none()
                && declared.fields.iter().all(|field| !field.once && field.name != "$pending")
                && plain.ty(&Type::Object(object))
        })
    };
    block.insts.iter().all(|inst| match &inst.kind {
        InstKind::Const(_) | InstKind::IntCompare { .. } | InstKind::BoolNot(_) | InstKind::BoolCompare { .. } => true,
        InstKind::IntBinary { op, .. } => matches!(op, IntBinaryOp::Add | IntBinaryOp::Sub | IntBinaryOp::Mul),
        InstKind::IntUnary { op, .. } => matches!(op, IntUnaryOp::Neg),
        InstKind::ObjNew { object, .. } => plain_object(*object),
        InstKind::ObjNext { object, .. } => match function.values.get(object.0) {
            Some(Type::Object(owner)) => plain_object(*owner),
            _ => false,
        },
        InstKind::ObjGet { .. } | InstKind::SuccTake(_) => true,
        _ => false,
    })
}

/// Replace the qualifying calls of `function` (the module's function `id`) by their callees' bodies.
fn inline_into(module: &Module, leaves: &[bool], id: FuncId, function: &mut Function) {
    let mut inlined: Vec<FuncId> = Vec::new();
    for block in 0..function.blocks.len() {
        let mut at = 0;
        while at < function.blocks[block].insts.len() {
            let InstKind::Call { func, args } = &function.blocks[block].insts[at].kind else {
                at += 1;
                continue;
            };
            let (callee_id, args) = (*func, args.clone());
            if callee_id == id || !leaves.get(callee_id.0).copied().unwrap_or(false) {
                at += 1;
                continue;
            }
            let callee = &module.functions[callee_id.0];
            let call_result = function.blocks[block].insts[at].result;
            // A successor's result `^C` is taken (`succ.take`) by the caller: the callee's `C` stands for the
            // taken value, so the call is inlined only when its one use is that `succ.take`.
            let successor = matches!(callee.result, ResultType::Value(Type::Successor(_)));
            let mut take = None;
            if successor {
                let Some(result) = call_result else {
                    at += 1;
                    continue;
                };
                match single_succ_take(function, result) {
                    Some(found) => take = Some(found),
                    None => {
                        at += 1;
                        continue;
                    }
                }
            }
            // Fresh values for the callee's, its parameters mapped to the arguments.
            let mut map: Vec<Option<ValueId>> = vec![None; callee.values.len()];
            for (param, arg) in callee.params.iter().zip(&args) {
                map[param.0] = Some(*arg);
            }
            let mut body = Vec::with_capacity(callee.blocks[0].insts.len());
            for inst in &callee.blocks[0].insts {
                let mut copy = inst.clone();
                if let Some(result) = inst.result {
                    let fresh = ValueId(function.values.len());
                    function.values.push(callee.values[result.0].clone());
                    map[result.0] = Some(fresh);
                    copy.result = Some(fresh);
                }
                for operand in copy.kind.operands_mut() {
                    *operand = map[operand.0].expect("an inlined operand is defined before its use");
                }
                body.push(copy);
            }
            let returned = match &callee.blocks[0].term.kind {
                TermKind::Ret(value) => value.map(|value| map[value.0].expect("the returned value is defined")),
                _ => unreachable!("a leaf ends in ret"),
            };
            let count = body.len();
            function.blocks[block].insts.splice(at..=at, body);
            match (take, call_result, returned) {
                (Some((take_block, take_at, taken)), _, Some(returned)) => {
                    // The `succ.take` follows the call in its block or later; it moved by the spliced body.
                    let take_at = if take_block == block && take_at > at { take_at + count - 1 } else { take_at };
                    function.blocks[take_block].insts.remove(take_at);
                    replace_uses(function, taken, returned);
                }
                (None, Some(result), Some(returned)) => replace_uses(function, result, returned),
                _ => {}
            }
            if !inlined.contains(&callee_id) {
                inlined.push(callee_id);
            }
            at += count;
        }
    }
    if !inlined.is_empty() {
        // The calls' results (and successors' `^C` values) are defined no more: renumber the values.
        super::cfg::renumber_values(function);
    }
    for callee in inlined {
        let attr = Attr::Inlined(callee);
        if !function.attrs.contains(&attr) {
            function.attrs.push(attr);
        }
    }
}

/// The one use of `value` when it is a `succ.take` of it: (block, index, the taken value).
fn single_succ_take(function: &Function, value: ValueId) -> Option<(usize, usize, ValueId)> {
    let mut found = None;
    let mut uses = 0;
    for (b, block) in function.blocks.iter().enumerate() {
        for (i, inst) in block.insts.iter().enumerate() {
            let reads = inst.kind.operands().iter().filter(|operand| **operand == value).count();
            if reads == 0 {
                continue;
            }
            uses += reads;
            if let (InstKind::SuccTake(taken), Some(result)) = (&inst.kind, inst.result) {
                if *taken == value {
                    found = Some((b, i, result));
                }
            }
        }
        uses += block.term.kind.operands().iter().filter(|operand| **operand == value).count();
        for target in block.term.kind.targets() {
            uses += target.args.iter().filter(|arg| **arg == value).count();
        }
        if let Some(unwind) = &block.unwind {
            uses += unwind.args.iter().filter(|arg| **arg == value).count();
        }
    }
    if uses == 1 { found } else { None }
}

/// Every read of `from` in `function` reads `to` instead.
fn replace_uses(function: &mut Function, from: ValueId, to: ValueId) {
    for block in &mut function.blocks {
        for inst in &mut block.insts {
            for operand in inst.kind.operands_mut() {
                if *operand == from {
                    *operand = to;
                }
            }
        }
        for operand in block.term.kind.operands_mut() {
            if *operand == from {
                *operand = to;
            }
        }
        for target in block.term.kind.targets_mut() {
            for arg in &mut target.args {
                if *arg == from {
                    *arg = to;
                }
            }
        }
        if let Some(unwind) = &mut block.unwind {
            for arg in &mut unwind.args {
                if *arg == from {
                    *arg = to;
                }
            }
        }
    }
}
