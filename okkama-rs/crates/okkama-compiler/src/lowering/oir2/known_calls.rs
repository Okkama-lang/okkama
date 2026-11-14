//! Conservative producer gate only: finite origins through SSA, snapshots and closed entries.
//! Unknown boundaries poison joins; unseeded cycles never establish a known callable.
use std::collections::{BTreeSet, HashMap, HashSet};

use super::{LowerError, Result};
use crate::oir2 as ir;
use crate::source::Span;

type Value = (ir::FuncId, ir::ValueId);
pub(super) type CallSpans = HashMap<(ir::FuncId, ir::InstId), Span>;

#[derive(Clone, Default)]
struct Origins {
    targets: BTreeSet<ir::FuncId>,
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

pub(super) fn check(module: &ir::Module, spans: &CallSpans) -> Result<()> {
    if spans.is_empty() {
        return Ok(());
    }
    let mut facts = HashMap::<Value, Origins>::new();
    let mut copies = vec![];
    let mut supplied = HashSet::new();
    let mut invocations = vec![];
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
                        origins.targets.insert(*target);
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
                    ir::Operation::Call(ir::CallOp::Closure { .. }) => {}
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
    // Uncalled ordinary declarations still require checking; their inputs have no known caller.
    for (function, data) in module.functions.iter() {
        let lambda = data.attrs.iter().any(|attr| matches!(attr, ir::FunctionAttr::Lambda { .. }));
        if !supplied.contains(&function) && !lambda {
            for param in entry(module, function)? {
                if let Some(origins) = facts.get_mut(&(function, param)) {
                    origins.open = true;
                }
            }
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
            for target in origins.targets {
                let captures = module.functions.get(target)?.attrs.iter().find_map(|attr| {
                    if let ir::FunctionAttr::Lambda { captures, .. } = attr { Some(*captures as usize) } else { None }
                }).unwrap_or(0);
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
