//! #1112 (stage 2): scalar replacement. A plain object made in a function (`obj.new`, or `obj.next` of another such
//! object) whose every use is a field read or the receiver of such an `obj.next` never needs to exist: its fields
//! are the values it was made from. Objects compare structurally and have no identity (Book §8.5), so dropping the
//! allocation is unobservable.
//!
//! A candidate is a plain object (no destroy, no `once` field, no `$pending`, not builtin, nothing in a region) whose
//! value is read only by `obj.get`, by `obj.next` whose result is a candidate too, and by branches to block
//! parameters that are candidates. A block parameter is one when every value branches pass it is: a loop-carried
//! object (`sum = sum.plus(v)`) then becomes one parameter per field. A value that reaches a call, an unwind
//! target, a terminator or anything else is materialised as before.
use super::regions::Plain;
use super::{Function, InstKind, Module, ObjectId, Type, ValueId};

/// Replace the candidates of every function of `module`.
pub fn replace_scalars(module: &mut Module) {
    let plain = Plain::of(module);
    // The field types of each eligible object.
    let eligible: Vec<Option<Vec<Type>>> = (0..module.objects.len())
        .map(|index| {
            let object = ObjectId(index);
            module.object(object).filter(|declared| {
                !declared.builtin
                    && declared.destroy.is_none()
                    && declared.fields.iter().all(|field| !field.once && field.name != "$pending")
                    && plain.ty(&Type::Object(object))
            }).map(|declared| declared.fields.iter().map(|field| field.ty.clone()).collect())
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

fn replace_in(function: &mut Function, eligible: &[Option<Vec<Type>>]) {
    let count = function.values.len();
    let eligible_type = |ty: Option<&Type>| match ty {
        Some(Type::Object(owner)) => eligible.get(owner.0).and_then(Option::as_ref),
        _ => None,
    };
    let mut made: Vec<Option<Made>> = vec![None; count];
    for block in &function.blocks {
        for inst in &block.insts {
            let Some(result) = inst.result else { continue };
            match &inst.kind {
                InstKind::ObjNew { object, fields } if eligible.get(object.0).is_some_and(Option::is_some) => {
                    made[result.0] = Some(Made::New(fields.clone()));
                }
                InstKind::ObjNext { object, updates } => {
                    if eligible_type(function.values.get(object.0)).is_some() {
                        made[result.0] = Some(Made::Next(*object, updates.clone()));
                    }
                }
                _ => {}
            }
        }
    }
    // Block parameters of an eligible object type: (block, position) of each.
    let mut phi: Vec<Option<(usize, usize)>> = vec![None; count];
    for (b, block) in function.blocks.iter().enumerate().skip(1) {
        for (position, param) in block.params.iter().enumerate() {
            if eligible_type(function.values.get(param.0)).is_some() {
                phi[param.0] = Some((b, position));
            }
        }
    }
    // A candidate stays one while every use is a field read, an `obj.next` receiver with a candidate result or a
    // branch argument of a candidate parameter; an `obj.next` stays one while its receiver does, and a parameter
    // while every value branches pass it is one.
    let mut candidate: Vec<bool> = (0..count).map(|value| made[value].is_some() || phi[value].is_some()).collect();
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
            for target in block.term.kind.targets() {
                let params = &function.blocks[target.block.0].params;
                for (arg, param) in target.args.iter().zip(params) {
                    if !(candidate[arg.0] && candidate[param.0]) {
                        // The parameter gets something that is not a candidate, or the argument goes to a
                        // parameter that is not one: neither can be split.
                        drop(*arg, &mut candidate);
                        drop(*param, &mut candidate);
                    }
                }
                // A parameter the edge fills itself (a lookup's element, a caught failure) is no candidate.
                for param in params.iter().skip(target.args.len()) {
                    drop(*param, &mut candidate);
                }
            }
            if let Some(unwind) = &block.unwind {
                // Unwind edges are not rewritten: neither their arguments nor any parameter of their target split.
                for arg in &unwind.args {
                    drop(*arg, &mut candidate);
                }
                for param in &function.blocks[unwind.block.0].params {
                    drop(*param, &mut candidate);
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
    // The fields of each candidate, from its construction (a receiver's fields with the updates); a candidate
    // parameter's are new parameters of its block, one per field, in its place.
    let mut fields: Vec<Option<Vec<ValueId>>> = vec![None; count];
    for value in 0..count {
        if !candidate[value] {
            continue;
        }
        let Some((block, _)) = phi[value] else { continue };
        let types = eligible_type(function.values.get(value)).expect("an eligible parameter").clone();
        let split: Vec<ValueId> = types
            .into_iter()
            .map(|ty| {
                function.values.push(ty);
                ValueId(function.values.len() - 1)
            })
            .collect();
        let _ = block;
        fields[value] = Some(split);
    }
    if fields.len() < function.values.len() {
        fields.resize(function.values.len(), None);
    }
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
    // Branches pass a candidate parameter its argument's fields; the block lists the fields in its place.
    let mut passed: Vec<(usize, usize, Vec<ValueId>)> = Vec::new();
    for (b, block) in function.blocks.iter().enumerate() {
        for (t, target) in block.term.kind.targets().into_iter().enumerate() {
            let params = &function.blocks[target.block.0].params;
            if !params.iter().any(|param| candidate[param.0]) {
                continue;
            }
            let mut args = Vec::new();
            for (arg, param) in target.args.iter().zip(params) {
                if candidate[param.0] {
                    args.extend(fields_of(*arg, &made, &mut fields));
                } else {
                    args.push(*arg);
                }
            }
            passed.push((b, t, args));
        }
    }
    for (b, t, args) in passed {
        function.blocks[b].term.kind.targets_mut()[t].args = args;
    }
    for block in &mut function.blocks {
        if !block.params.iter().any(|param| candidate[param.0]) {
            continue;
        }
        block.params = block
            .params
            .iter()
            .flat_map(|param| if candidate[param.0] { fields[param.0].clone().expect("split") } else { vec![*param] })
            .collect();
    }
    // Each field read of a candidate is its field's value.
    let count = function.values.len();
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
