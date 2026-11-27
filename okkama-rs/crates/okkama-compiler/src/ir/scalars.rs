//! #1112 (stage 2): scalar replacement. A plain object made in a function (`obj.new`, or `obj.next` of another such
//! object) whose every use is a field read or the receiver of such an `obj.next` never needs to exist: its fields
//! are the values it was made from. Objects compare structurally and have no identity (Book §8.5), so dropping the
//! allocation is unobservable.
//!
//! A candidate is a plain object (no destroy, no `once` field, no `$pending`, not builtin, nothing in a region) whose
//! value is read only by `obj.get` and by `obj.next` whose result is a candidate too; a value that reaches a call, a
//! branch argument, a terminator or anything else is materialised as before. Loop-carried objects (block
//! parameters) are a later step.
use super::regions::Plain;
use super::{Function, InstKind, Module, ObjectId, Type, ValueId};

/// Replace the candidates of every function of `module`.
pub fn replace_scalars(module: &mut Module) {
    let plain = Plain::of(module);
    let eligible: Vec<bool> = (0..module.objects.len())
        .map(|index| {
            let object = ObjectId(index);
            module.object(object).is_some_and(|declared| {
                !declared.builtin
                    && declared.destroy.is_none()
                    && declared.fields.iter().all(|field| !field.once && field.name != "$pending")
                    && plain.ty(&Type::Object(object))
            })
        })
        .collect();
    for function in &mut module.functions {
        replace_in(function, &eligible);
    }
}

/// How a value is made: its fields (for a candidate's own construction) or its receiver and updates.
#[derive(Clone)]
enum Made {
    New(Vec<ValueId>),
    Next(ValueId, Vec<(usize, ValueId)>),
}

fn replace_in(function: &mut Function, eligible: &[bool]) {
    let count = function.values.len();
    let mut made: Vec<Option<Made>> = vec![None; count];
    for block in &function.blocks {
        for inst in &block.insts {
            let Some(result) = inst.result else { continue };
            match &inst.kind {
                InstKind::ObjNew { object, fields } if eligible.get(object.0).copied().unwrap_or(false) => {
                    made[result.0] = Some(Made::New(fields.clone()));
                }
                InstKind::ObjNext { object, updates } => {
                    if let Some(Type::Object(owner)) = function.values.get(object.0)
                        && eligible.get(owner.0).copied().unwrap_or(false)
                    {
                        made[result.0] = Some(Made::Next(*object, updates.clone()));
                    }
                }
                _ => {}
            }
        }
    }
    // A candidate stays one while every use is a field read or an `obj.next` receiver with a candidate result, and
    // an `obj.next` stays one while its receiver does.
    let mut candidate: Vec<bool> = made.iter().map(Option::is_some).collect();
    loop {
        let mut changed = false;
        let mut drop = |value: ValueId, candidate: &mut Vec<bool>| {
            if candidate[value.0] {
                candidate[value.0] = false;
                changed = true;
            }
        };
        for value in 0..count {
            if let Some(Made::Next(receiver, _)) = &made[value]
                && candidate[value]
                && !candidate[receiver.0]
            {
                drop(ValueId(value), &mut candidate);
            }
        }
        for block in &function.blocks {
            for inst in &block.insts {
                match &inst.kind {
                    InstKind::ObjGet { .. } => {}
                    InstKind::ObjNext { object, updates } => {
                        let result_ok = inst.result.is_some_and(|result| candidate[result.0]);
                        if !result_ok {
                            drop(*object, &mut candidate);
                        }
                        for (_, value) in updates {
                            drop(*value, &mut candidate);
                        }
                    }
                    other => {
                        for operand in other.operands() {
                            drop(operand, &mut candidate);
                        }
                    }
                }
            }
            for operand in block.term.kind.operands() {
                drop(operand, &mut candidate);
            }
            for target in block.term.kind.targets().into_iter().chain(block.unwind.as_ref()) {
                for arg in &target.args {
                    drop(*arg, &mut candidate);
                }
            }
        }
        if !changed {
            break;
        }
    }
    if !candidate.iter().any(|&c| c) {
        return;
    }
    // The fields of each candidate, from its construction (a receiver's fields with the updates).
    let mut fields: Vec<Option<Vec<ValueId>>> = vec![None; count];
    fn fields_of(value: ValueId, made: &[Option<Made>], fields: &mut Vec<Option<Vec<ValueId>>>) -> Vec<ValueId> {
        if let Some(known) = &fields[value.0] {
            return known.clone();
        }
        let resolved = match made[value.0].as_ref().expect("a candidate is made here") {
            Made::New(values) => values.clone(),
            Made::Next(receiver, updates) => {
                let mut values = fields_of(*receiver, made, fields);
                for (field, update) in updates {
                    values[*field] = *update;
                }
                values
            }
        };
        fields[value.0] = Some(resolved.clone());
        resolved
    }
    // Each field read of a candidate is its field's value.
    let mut replaced: Vec<Option<ValueId>> = vec![None; count];
    for block in &function.blocks {
        for inst in &block.insts {
            if let (InstKind::ObjGet { object, field }, Some(result)) = (&inst.kind, inst.result)
                && candidate[object.0]
            {
                let values = fields_of(*object, &made, &mut fields);
                replaced[result.0] = Some(values[*field]);
            }
        }
    }
    // A read of a read resolves to the end of the chain.
    let resolve = |mut value: ValueId| {
        while let Some(next) = replaced[value.0] {
            value = next;
        }
        value
    };
    for block in &mut function.blocks {
        block.insts.retain(|inst| match (&inst.kind, inst.result) {
            (InstKind::ObjGet { object, .. }, _) => !candidate[object.0],
            (InstKind::ObjNew { .. } | InstKind::ObjNext { .. }, Some(result)) => !candidate[result.0],
            _ => true,
        });
        for inst in &mut block.insts {
            for operand in inst.kind.operands_mut() {
                *operand = resolve(*operand);
            }
        }
        for operand in block.term.kind.operands_mut() {
            *operand = resolve(*operand);
        }
        for target in block.term.kind.targets_mut() {
            for arg in &mut target.args {
                *arg = resolve(*arg);
            }
        }
        if let Some(unwind) = &mut block.unwind {
            for arg in &mut unwind.args {
                *arg = resolve(*arg);
            }
        }
    }
    super::cfg::renumber_values(function);
}
