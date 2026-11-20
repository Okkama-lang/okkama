//! Static resources (#909 step 2b): a resource created, used and ending in one scope of one function is destroyed
//! directly at that scope's exits, with no region operations.
//!
//! The escape question has one answer in the compiler: the ownership census (`ir::ownership`, #553). A creation
//! it places in the scope (`Owner::Scope`) never reaches a result, a field that leaves, a Future, a closure, a
//! worker, an atomic or a failure. On top of that this pass takes a value only when everything else is plain to
//! see:
//!
//! - it is the result of a call to a constructor of an object with a synchronous destroy of its own, not
//!   builtin, whose fields are all plain (no other identity can hang off it);
//! - its region operations are `region.track`/`region.adopt` into one region R, and R holds nothing else: no
//!   other member, no transfer, no keep that could reach a resource, R not passed around;
//! - every block reachable from its creation before R's exits is dominated by the creation (no drop flags yet).
//!
//! The rewrite: the creating block is split after the constructor call; `track`/`adopt` become `resource.pin`;
//! every `region.leave R` the value reaches gets a `resource.leave` before it; every edge from the value's code
//! into a handler that runs `region.unwind R` goes through a new block that runs `resource.unwind` first. R is
//! then empty and the `regions` pass removes it. The runtime destroys a pinned resource exactly once (a second
//! close of it does nothing), so a redirected edge that also passes a `resource.leave` cannot destroy twice.
//! Anything unsure stays dynamic.
use std::collections::HashSet;

use super::ownership::{self, Owner};
use super::regions::Plain;
use super::{Attr, Block, BlockId, Function, Inst, InstKind, Module, Target, TermKind, Terminator, Type, ValueId, cfg};

/// Make the module's provably local resources static; the number made static.
pub fn make_static(module: &mut Module) -> usize {
    if module.objects.iter().all(|object| object.destroy.is_none()) {
        return 0;
    }
    let report = ownership::analyze(module);
    let plain = Plain::of(module);
    let mut count = 0;
    for found in &report.functions {
        let candidates: Vec<ValueId> = found
            .creations
            .iter()
            .filter(|creation| creation.owner == Owner::Scope && creation.kind == "call.result")
            .map(|creation| ValueId(creation.value))
            .collect();
        for value in candidates {
            if !eligible(module, &plain, &module.functions[found.function], value) {
                continue;
            }
            split_after_definition(&mut module.functions[found.function], value);
            let Some(plan) = plan(&module.functions[found.function], value) else {
                // The split alone changes nothing: the value stays dynamic.
                continue;
            };
            apply(&mut module.functions[found.function], value, plan);
            count += 1;
        }
    }
    okkama_devtrace::count!("lower", "statics: resources made static", count as u64);
    count
}

/// Where the value is defined: block and instruction index.
fn definition(function: &Function, value: ValueId) -> Option<(usize, usize)> {
    function.blocks.iter().enumerate().find_map(|(block, body)| {
        body.insts.iter().position(|inst| inst.result == Some(value)).map(|index| (block, index))
    })
}

/// The region a value is tracked or adopted into, when that is its only region use.
fn region_of(function: &Function, value: ValueId) -> Option<ValueId> {
    let mut region = None;
    for inst in function.blocks.iter().flat_map(|block| &block.insts) {
        match &inst.kind {
            InstKind::RegionTrack { region: r, value: v } | InstKind::RegionAdopt { region: r, value: v } if *v == value => {
                if region.is_some_and(|known| known != *r) {
                    return None;
                }
                region = Some(*r);
            }
            InstKind::RegionTransfer { value: v, .. } if *v == value => return None,
            _ => {}
        }
    }
    region
}

/// The type and region conditions (no control flow yet).
fn eligible(module: &Module, plain: &Plain, function: &Function, value: ValueId) -> bool {
    if function.is_async() {
        return false;
    }
    let Some((block, index)) = definition(function, value) else { return false };
    let InstKind::Call { func, .. } = &function.blocks[block].insts[index].kind else { return false };
    let Some(object) = module.functions[func.0].attrs.iter().find_map(|attr| match attr {
        Attr::Ctor(object) => Some(*object),
        _ => None,
    }) else {
        return false;
    };
    if function.values[value.0] != Type::Object(object) {
        return false;
    }
    let declaration = &module.objects[object.0];
    let Some(destroy) = declaration.destroy else { return false };
    if declaration.builtin
        || module.functions[destroy.0].is_async()
        || !declaration.fields.iter().all(|field| plain.ty(&field.ty))
    {
        return false;
    }
    let Some(region) = region_of(function, value) else { return false };
    // R holds only this value, closes without keeping a resource, and is used by nothing but its own operations.
    for block in &function.blocks {
        for inst in &block.insts {
            match &inst.kind {
                InstKind::RegionEnter => {}
                InstKind::RegionTrack { region: r, value: v } | InstKind::RegionAdopt { region: r, value: v } => {
                    if *r == region && *v != value {
                        return false;
                    }
                }
                InstKind::RegionTransfer { from, to, .. } => {
                    if *from == region || *to == region {
                        return false;
                    }
                }
                InstKind::RegionLeave { region: r, keep } => {
                    if *r == region && keep.is_some_and(|keep| !plain.ty(&function.values[keep.0])) {
                        return false;
                    }
                }
                InstKind::RegionUnwind { .. } => {}
                InstKind::RegionSettle { region: r, .. } | InstKind::RegionSettleUnwind { region: r, .. } => {
                    if *r == region {
                        return false;
                    }
                }
                other => {
                    if other.operands().contains(&region) {
                        return false;
                    }
                }
            }
        }
        let targets = block.term.kind.targets().into_iter().chain(block.unwind.as_ref());
        if targets.flat_map(|target| &target.args).any(|arg| *arg == region)
            || block.term.kind.operands().contains(&region)
        {
            return false;
        }
    }
    true
}

/// Split the creating block right after the creation, so the creation's own failure keeps the old handler.
fn split_after_definition(function: &mut Function, value: ValueId) {
    let Some((block, index)) = definition(function, value) else { return };
    let rest = function.blocks[block].insts.split_off(index + 1);
    let span = function.blocks[block].term.span;
    let next = BlockId(function.blocks.len());
    let term = std::mem::replace(
        &mut function.blocks[block].term,
        Terminator { kind: TermKind::Br(Target::new(next)), span },
    );
    let unwind = function.blocks[block].unwind.clone();
    function.blocks.push(Block { params: Vec::new(), unwind, insts: rest, term });
}

struct Plan {
    region: ValueId,
    /// Blocks whose `region.leave R` the value reaches.
    leaves: Vec<usize>,
    /// Blocks whose unwind edge enters a handler of R.
    unwinds: Vec<usize>,
    /// Blocks whose terminator branches into a handler of R.
    branches: Vec<usize>,
}

fn has(block: &Block, region: ValueId, leave: bool) -> bool {
    block.insts.iter().any(|inst| match &inst.kind {
        InstKind::RegionLeave { region: r, .. } => leave && *r == region,
        InstKind::RegionUnwind { region: r, .. } => !leave && *r == region,
        _ => false,
    })
}

/// The control-flow conditions, after the split: the blocks to rewrite, or `None`.
fn plan(function: &Function, value: ValueId) -> Option<Plan> {
    let region = region_of(function, value)?;
    let (start, _) = definition(function, value)?;
    let edges: Vec<Vec<usize>> = (0..function.blocks.len())
        .map(|block| cfg::all_successors(function, BlockId(block)).into_iter().map(|target| target.0).collect())
        .collect();
    let (idom, _) = cfg::dominators(&edges);
    let dominates = |by: usize, mut block: usize| loop {
        if block == by {
            return true;
        }
        match idom[block] {
            Some(parent) if parent != block => block = parent,
            _ => return false,
        }
    };
    let handler = |block: usize| has(&function.blocks[block], region, false);
    // A handler closes R first: whatever it closed before would have to die before the value.
    let closes_first = |block: usize| matches!(function.blocks[block].insts.first().map(|inst| &inst.kind),
        Some(InstKind::RegionUnwind { region: r, .. }) if *r == region);
    // The creating block's own successor is the split-off rest; the walk starts there.
    let TermKind::Br(first) = &function.blocks[start].term.kind else { return None };
    let mut plan = Plan { region, leaves: Vec::new(), unwinds: Vec::new(), branches: Vec::new() };
    let mut seen = HashSet::new();
    let mut stack = vec![first.block.0];
    while let Some(block) = stack.pop() {
        if !seen.insert(block) {
            continue;
        }
        if !dominates(start, block) || handler(block) {
            return None;
        }
        let body = &function.blocks[block];
        if has(body, region, true) {
            // Nothing before `region.leave R` may fail into a handler that does not close R.
            let first_leave = body.insts.iter().position(|inst| matches!(&inst.kind,
                InstKind::RegionLeave { region: r, .. } if *r == region)).expect("leave");
            if first_leave > 0 && !body.unwind.as_ref().is_some_and(|unwind| handler(unwind.block.0)) {
                return None;
            }
            // The value is dead after `resource.leave`: what fails later in the block is not its concern.
            if first_leave > 0 {
                if !closes_first(body.unwind.as_ref().expect("checked").block.0) {
                    return None;
                }
                plan.unwinds.push(block);
            }
            plan.leaves.push(block);
            continue;
        }
        if let Some(unwind) = &body.unwind {
            if handler(unwind.block.0) {
                if !closes_first(unwind.block.0) {
                    return None;
                }
                plan.unwinds.push(block);
            } else {
                stack.push(unwind.block.0);
            }
        }
        let targets = body.term.kind.targets();
        if targets.iter().any(|target| handler(target.block.0)) {
            if targets.iter().any(|target| handler(target.block.0) && !closes_first(target.block.0)) {
                return None;
            }
            plan.branches.push(block);
        }
        stack.extend(targets.iter().filter(|target| !handler(target.block.0)).map(|target| target.block.0));
    }
    Some(plan)
}

fn apply(function: &mut Function, value: ValueId, plan: Plan) {
    let region = plan.region;
    // `track`/`adopt` into R become one pin where the first of them was.
    let mut pinned = false;
    for block in &mut function.blocks {
        let mut kept = Vec::with_capacity(block.insts.len());
        for inst in block.insts.drain(..) {
            match &inst.kind {
                InstKind::RegionTrack { region: r, value: v } | InstKind::RegionAdopt { region: r, value: v }
                    if *r == region && *v == value =>
                {
                    if !pinned {
                        pinned = true;
                        kept.push(Inst { result: None, kind: InstKind::ResourcePin { value }, span: inst.span });
                    }
                }
                _ => kept.push(inst),
            }
        }
        block.insts = kept;
    }
    for &block in &plan.leaves {
        let insts = &mut function.blocks[block].insts;
        let at = insts.iter().position(|inst| matches!(&inst.kind,
            InstKind::RegionLeave { region: r, .. } if *r == region)).expect("leave");
        let span = insts[at].span;
        insts.insert(at, Inst { result: None, kind: InstKind::ResourceLeave { value }, span });
    }
    // One forwarding block per handler: close the value, then the handler as before.
    let mut forwarders: Vec<(usize, usize)> = Vec::new();
    let mut forwarder = |function: &mut Function, handler: usize| -> usize {
        if let Some(&(_, made)) = forwarders.iter().find(|(of, _)| *of == handler) {
            return made;
        }
        let params: Vec<ValueId> = function.blocks[handler].params.iter().map(|param| {
            let ty = function.values[param.0].clone();
            function.values.push(ty);
            ValueId(function.values.len() - 1)
        }).collect();
        let (failure, rest) = params.split_last().expect("a handler takes its failure");
        function.values.push(Type::Failure);
        let closed = ValueId(function.values.len() - 1);
        let span = function.blocks[handler].term.span;
        let mut args = rest.to_vec();
        args.push(closed);
        function.blocks.push(Block {
            params: params.clone(),
            unwind: None,
            insts: vec![Inst { result: Some(closed), kind: InstKind::ResourceUnwind { value, failure: *failure }, span }],
            term: Terminator { kind: TermKind::Br(Target { block: BlockId(handler), args }), span },
        });
        let made = function.blocks.len() - 1;
        forwarders.push((handler, made));
        made
    };
    for &block in &plan.unwinds {
        let handler = function.blocks[block].unwind.as_ref().expect("unwind").block.0;
        let made = forwarder(function, handler);
        function.blocks[block].unwind.as_mut().expect("unwind").block = BlockId(made);
    }
    for &block in &plan.branches {
        let handlers: Vec<usize> = function.blocks[block].term.kind.targets().iter()
            .map(|target| target.block.0)
            .filter(|&target| has(&function.blocks[target], region, false))
            .collect();
        for handler in handlers {
            let made = forwarder(function, handler);
            for target in function.blocks[block].term.kind.targets_mut() {
                if target.block.0 == handler {
                    target.block = BlockId(made);
                }
            }
        }
    }
}
