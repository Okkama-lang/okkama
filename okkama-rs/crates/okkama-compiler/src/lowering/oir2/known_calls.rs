//! Conservative producer gate only: finite origins through SSA, snapshots and closed entries.
//! Unknown boundaries poison joins; unseeded cycles never establish a known callable.
//! Immutable fields use the union of all construction/update inputs in this closed module.
//! This proves admission only; ownership origins, contracts and adapters still belong to own.
use std::collections::{BTreeSet, HashMap, HashSet};

use super::{LowerError, Result};
use crate::oir2 as ir;
use crate::source::Span;

type Field = (ir::DeclId, u32);

fn field_key(field: ir::FieldRef) -> Field {
    (field.declaration, field.index)
}

type Value = (ir::FuncId, ir::ValueId);
pub(super) type CallSpans = HashMap<(ir::FuncId, ir::InstId), Span>;

#[derive(Clone, Default)]
struct Origins {
    targets: BTreeSet<(ir::FuncId, usize)>,
    open: bool,
}

impl Origins {
    fn merge(&mut self, other: &Self) -> bool {
        let before = self.targets.len();
        let open = self.open;
        self.targets.extend(&other.targets);
        self.open |= other.open;
        before != self.targets.len() || open != self.open
    }
}

fn entry(module: &ir::Module, function: ir::FuncId) -> Result<Vec<ir::ValueId>> {
    let Some(body) = &module.functions.get(function)?.body else {
        return Ok(vec![]);
    };
    Ok(body.blocks.get(body.entry)?.params.clone())
}

fn returns(module: &ir::Module, function: ir::FuncId) -> Result<Vec<ir::ValueId>> {
    let Some(body) = &module.functions.get(function)?.body else {
        return Ok(vec![]);
    };
    Ok(body.blocks.iter().filter_map(|(_, block)| {
        if let ir::TermKind::Ret { values } = &block.term.kind { values.first().copied() } else { None }
    }).collect())
}

fn cap_target(module: &ir::Module, field: ir::FieldRef) -> Result<ir::FuncId> {
    let ir::DeclarationKind::Cap { fields, .. } = &module.declarations.get(field.declaration)?.kind else {
        return Err(LowerError::invalid(Span::default(), "computed call without cap declaration"));
    };
    fields.get(field.index as usize).map(|field| field.function)
        .ok_or_else(|| LowerError::invalid(Span::default(), "computed call field index"))
}

pub(super) fn check(module: &ir::Module, spans: &CallSpans) -> Result<()> {
    if spans.is_empty() {
        return Ok(());
    }
    let mut facts = HashMap::<Value, Origins>::new();
    let mut copies = vec![];
    let mut supplied = HashSet::new();
    let mut invocations = vec![];
    let mut field_reads = vec![];
    let mut field_writes = HashMap::<Field, Vec<Value>>::new();
    let mut open_fields = HashSet::new();
    for (function, data) in module.functions.iter() {
        let Some(body) = &data.body else {
            continue;
        };
        for (value, data) in body.values.iter() {
            if !matches!(module.types.get(data.ty)?, ir::Type::Fn(_)) {
                continue;
            }
            let mut origins = Origins::default();
            if let ir::Definition::InstResult { inst, .. } = data.def {
                match &body.insts.get(inst)?.op {
                    ir::Operation::Core(ir::CoreOp::FnClosure { function: target, .. }) => {
                        let captures = module.functions.get(*target)?.attrs.iter().find_map(|attr| {
                            if let ir::FunctionAttr::Lambda { captures, .. } = attr {
                                Some(*captures as usize)
                            } else {
                                None
                            }
                        }).unwrap_or(0);
                        origins.targets.insert((*target, captures));
                    }
                    ir::Operation::Core(ir::CoreOp::FnBind { function: target, .. }) => {
                        origins.targets.insert((*target, 1));
                        supplied.insert(*target);
                    }
                    ir::Operation::Core(ir::CoreOp::FnBindRole { role, slot, .. }) => {
                        for target in super::bound::role_targets(module, *role, *slot)? {
                            origins.targets.insert((target, 1));
                            supplied.insert(target);
                        }
                        origins.open = origins.targets.is_empty();
                    }
                    ir::Operation::Core(ir::CoreOp::Retag { value: source, .. }) => {
                        copies.push(((function, *source), (function, value)));
                    }
                    ir::Operation::Call(ir::CallOp::Direct { function: target, .. })
                        if module.functions.get(*target)?.body.is_some() => {
                        for source in returns(module, *target)? {
                            copies.push(((*target, source), (function, value)));
                        }
                    }
                    ir::Operation::Core(ir::CoreOp::ObjGet { field, .. }) => {
                        field_reads.push((field_key(*field), (function, value)));
                    }
                    ir::Operation::Call(ir::CallOp::Role { role, slot, .. }) => {
                        for target in super::bound::role_targets(module, *role, *slot)? {
                            for source in returns(module, target)? {
                                copies.push(((target, source), (function, value)));
                            }
                        }
                    }
                    ir::Operation::Call(ir::CallOp::Closure { .. }) => {}
                    ir::Operation::Call(ir::CallOp::Cap { field, .. }) => {
                        let target = cap_target(module, *field)?;
                        for source in returns(module, target)? {
                            copies.push(((target, source), (function, value)));
                        }
                    }
                    _ => origins.open = true,
                }
            }
            facts.insert((function, value), origins);
        }
        for (_, block) in body.blocks.iter() {
            let mut targets = vec![];
            let mut produced = vec![];
            if let Some(unwind) = &block.unwind {
                targets.push(unwind.target.clone());
            }
            let mut invoke = |edge: &ir::InvokeEdge| {
                for (arg, &dest) in edge.args.iter().zip(&body.blocks.get(edge.block).unwrap().params) {
                    match arg {
                        ir::EdgeArg::Value(source) => copies.push(((function, *source), (function, dest))),
                        ir::EdgeArg::Produced(_) => produced.push(dest),
                    }
                }
            };
            match &block.term.kind {
                ir::TermKind::Br(target) => targets.push(target.clone()),
                ir::TermKind::Cond { yes, no, .. } => targets.extend([yes.clone(), no.clone()]),
                ir::TermKind::Switch { cases, default, .. } => {
                    targets.extend(cases.iter().map(|case| case.target.clone()));
                    targets.push(default.clone());
                }
                ir::TermKind::Catch { handler, next, .. } => {
                    invoke(handler);
                    targets.push(next.clone());
                }
                ir::TermKind::Lookup { found, missing, .. } => {
                    invoke(found);
                    targets.push(missing.clone());
                }
                _ => {}
            }
            for target in targets {
                for (&source, &dest) in target.args.iter().zip(&body.blocks.get(target.block)?.params) {
                    copies.push(((function, source), (function, dest)));
                }
            }
            for value in produced {
                if let Some(origin) = facts.get_mut(&(function, value)) {
                    origin.open = true;
                }
            }
        }
        let mut bindings = HashMap::<ir::BindingId, Vec<ir::ValueId>>::new();
        for (_, inst) in body.insts.iter() {
            if let ir::Operation::Lexical(ir::LexicalOp::Bind { binding, value }
                | ir::LexicalOp::Rebind { binding, value, .. }) = inst.op {
                bindings.entry(binding).or_default().push(value);
            }
        }
        for (inst, data) in body.insts.iter() {
            match &data.op {
                ir::Operation::Core(ir::CoreOp::ObjNew { declaration, fields, .. }) => {
                    for (index, &source) in fields.iter().enumerate() {
                        let field = ir::FieldRef { declaration: *declaration, index: index as u32 };
                        field_writes.entry(field_key(field)).or_default().push((function, source));
                    }
                }
                ir::Operation::Core(ir::CoreOp::ObjNext { updates, .. }) => {
                    for update in updates {
                        field_writes.entry(field_key(update.field)).or_default().push((function, update.value));
                    }
                }
                ir::Operation::Core(ir::CoreOp::ObjInit { field, value, .. }) => {
                    field_writes.entry(field_key(*field)).or_default().push((function, *value));
                }
                ir::Operation::Call(ir::CallOp::Role { role, slot, call, .. }) => {
                    for target in super::bound::role_targets(module, *role, *slot)? {
                        supplied.insert(target);
                        for (source, dest) in call.args.iter().zip(entry(module, target)?.into_iter().skip(1)) {
                            copies.push(((function, source.value), (target, dest)));
                        }
                    }
                }
                ir::Operation::Call(ir::CallOp::Cap { field, .. }) => {
                    supplied.insert(cap_target(module, *field)?);
                }
                ir::Operation::Core(ir::CoreOp::FnClosure { function: target, captures, .. }) => {
                    supplied.insert(*target);
                    for (&source, dest) in captures.iter().zip(entry(module, *target)?) {
                        copies.push(((function, source), (*target, dest)));
                    }
                }
                ir::Operation::Call(ir::CallOp::Direct { function: target, call }) => {
                    supplied.insert(*target);
                    for (source, dest) in call.args.iter().zip(entry(module, *target)?) {
                        copies.push(((function, source.value), (*target, dest)));
                    }
                }
                ir::Operation::Lexical(ir::LexicalOp::DeferRegister { action, .. }) => {
                    supplied.insert(action.function);
                    let params = entry(module, action.function)?;
                    for (&source, &dest) in action.captures.iter().zip(&params) {
                        copies.push(((function, source), (action.function, dest)));
                    }
                    // Deferred bindings are lazy. Union every possible bound/rebound value;
                    // this gate makes no claim about the execution-time binding selection.
                    for (binding, &dest) in action.bindings.iter().zip(&params[action.captures.len()..]) {
                        for &source in bindings.get(binding).into_iter().flatten() {
                            copies.push(((function, source), (action.function, dest)));
                        }
                    }
                }
                ir::Operation::Call(ir::CallOp::Closure { callee, call }) => {
                    invocations.push((function, inst, callee.value, call, data.results.first().copied()));
                }
                _ => {}
            }
        }
    }
    // Uncalled ordinary declarations still require checking; explicit inputs have no known caller.
    // An implicit receiver comes from this closed module's construction sites, rather than a
    // separate external input for every unused method in a concrete witness inventory.
    let computed: HashSet<_> = module.declarations.iter().flat_map(|(_, declaration)| {
        match &declaration.kind {
            ir::DeclarationKind::Cap { fields, .. } => fields.iter().map(|field| field.function).collect(),
            _ => vec![],
        }
    }).collect();
    for (function, data) in module.functions.iter() {
        let lambda = data.attrs.iter().any(|attr| matches!(attr, ir::FunctionAttr::Lambda { .. }));
        if !supplied.contains(&function) && !lambda {
            let receiver = usize::from(computed.contains(&function) || data.attrs.iter().any(|attr| matches!(attr,
                ir::FunctionAttr::Method(_) | ir::FunctionAttr::Successor(_) | ir::FunctionAttr::Destroy(_))));
            for param in entry(module, function)?.into_iter().skip(receiver) {
                if let Some(origins) = facts.get_mut(&(function, param)) {
                    origins.open = true;
                }
                let body = data.body.as_ref().expect("entry body");
                let ty = body.values.get(param)?.ty;
                unknown_fields(module, ty, &mut HashSet::new(), &mut open_fields)?;
            }
        }
    }
    for (field, dest) in field_reads {
        if let Some(sources) = field_writes.get(&field) {
            for &source in sources {
                copies.push((source, dest));
            }
        }
        if open_fields.contains(&field) && let Some(origins) = facts.get_mut(&dest) {
            origins.open = true;
        }
    }
    let mut completed = false;
    loop {
        let mut changed = false;
        for &(source, dest) in &copies {
            if let Some(input) = facts.get(&source).cloned()
                && let Some(output) = facts.get_mut(&dest) {
                changed |= output.merge(&input);
            }
        }
        for &(function, _, callee, call, result) in &invocations {
            let origins = facts.get(&(function, callee)).cloned().unwrap_or_default();
            if origins.open && let Some(result) = result && let Some(output) = facts.get_mut(&(function, result)) {
                changed |= output.merge(&Origins { open: true, ..Default::default() });
            }
            for (target, captures) in origins.targets {
                for (arg, dest) in call.args.iter().zip(entry(module, target)?.into_iter().skip(captures)) {
                    if let Some(input) = facts.get(&(function, arg.value)).cloned()
                        && let Some(output) = facts.get_mut(&(target, dest)) {
                        changed |= output.merge(&input);
                    }
                }
                if let Some(result) = result {
                    for source in returns(module, target)? {
                        if let Some(input) = facts.get(&(target, source)).cloned()
                            && let Some(output) = facts.get_mut(&(function, result)) {
                            changed |= output.merge(&input);
                        }
                    }
                }
            }
        }
        if !changed {
            if completed {
                break;
            }
            // An unresolved input must poison mixed known/unknown joins as well.
            for origin in facts.values_mut().filter(|origin| origin.targets.is_empty()) {
                origin.open = true;
            }
            completed = true;
        }
    }
    for (function, inst, callee, _, _) in invocations {
        let known = facts.get(&(function, callee)).is_some_and(|origin| !origin.open && !origin.targets.is_empty());
        if !known {
            let span = spans.get(&(function, inst)).copied().unwrap_or_default();
            let error = LowerError::unsupported(span, "open action invocation (pending audited O1 contract)");
            return Err(error.in_function(&module.functions.get(function)?.symbol));
        }
    }
    Ok(())
}

// A field union is closed only when every construction/update input is known and no
// unsupplied ordinary entry can introduce an external object containing that field.
fn unknown_fields(module: &ir::Module, ty: ir::TypeId, seen: &mut HashSet<ir::TypeId>,
    fields: &mut HashSet<Field>) -> Result<()> {
    if !seen.insert(ty) {
        return Ok(());
    }
    match module.types.get(ty)? {
        ir::Type::Cap { decl, .. } => {
            if let ir::DeclarationKind::Cap { owner, .. } = &module.declarations.get(*decl)?.kind {
                unknown_fields(module, *owner, seen, fields)?;
            }
        }
        ir::Type::Object { decl, .. } => {
            if let ir::DeclarationKind::Object { fields: layout, .. } = &module.declarations.get(*decl)?.kind {
                for (index, field) in layout.iter().enumerate() {
                    fields.insert((*decl, index as u32));
                    unknown_fields(module, field.ty, seen, fields)?;
                }
            }
        }
        ir::Type::Role { .. } => {
            for (decl, declaration) in module.declarations.iter() {
                if let ir::DeclarationKind::Object { witnesses, fields: layout, .. } = &declaration.kind
                    && witnesses.iter().any(|witness| witness.role == ty) {
                    for (index, field) in layout.iter().enumerate() {
                        fields.insert((decl, index as u32));
                        unknown_fields(module, field.ty, seen, fields)?;
                    }
                }
            }
        }
        ir::Type::Variant(decl) => {
            if let ir::DeclarationKind::Variant { cases, .. } = &module.declarations.get(*decl)?.kind {
                for case in cases {
                    if let ir::DeclarationKind::Object { fields: layout, .. } = &module.declarations
                        .get(case.layout)?.kind {
                        for (index, field) in layout.iter().enumerate() {
                            fields.insert((case.layout, index as u32));
                            unknown_fields(module, field.ty, seen, fields)?;
                        }
                    }
                }
            }
        }
        ir::Type::Successor(inner) | ir::Type::Array(inner) => unknown_fields(module, *inner, seen, fields)?,
        ir::Type::Map(key, value) => {
            unknown_fields(module, *key, seen, fields)?;
            unknown_fields(module, *value, seen, fields)?;
        }
        _ => {}
    }
    Ok(())
}
