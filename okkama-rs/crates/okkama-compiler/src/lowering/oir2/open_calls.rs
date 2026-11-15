//! #569 O1 end-to-end evidence (test builds only). An invocation of a `fn`
//! parameter is open: it receives the registered open sync callable ABI of the
//! `okkama-oir` runtime catalogue, namely the Closure protocol, the cleanup intrinsic
//! and the runtime descriptor of the callable type. The call itself stays
//! unsummarized in High; own takes its contract from the catalogue. C3b, after the
//! audit, replaces this with the full producer integration.
use super::{LowerError, Result};
use crate::oir2 as ir;
use crate::source::Span;
use ir::catalogue;

pub(super) fn attach(module: &mut ir::Module) -> Result<()> {
    let mut sites = vec![];
    for (function, data) in module.functions.iter() {
        let Some(body) = &data.body else { continue };
        let mut callees = vec![];
        for (_, instruction) in body.insts.iter() {
            if let ir::Operation::Call(ir::CallOp::Closure { callee, .. }) = &instruction.op {
                callees.push(callee.value);
            }
        }
        for (_, block) in body.blocks.iter() {
            if let ir::TermKind::InvokeOwn { op, .. } = &block.term.kind
                && let ir::Operation::Call(ir::CallOp::Closure { callee, .. }) = op.as_ref() {
                callees.push(callee.value);
            }
        }
        for callee in callees {
            let value = body.values.get(callee)?;
            if matches!(module.types.get(value.ty)?, ir::Type::Fn(_)) && reaches_entry(body, callee)?
                && !sites.contains(&(function, value.ty)) {
                sites.push((function, value.ty));
            }
        }
    }
    let Some(&(_, callable)) = sites.first() else {
        return Ok(());
    };
    if sites.iter().any(|(_, ty)| *ty != callable) {
        return Err(LowerError::unsupported(Span::default(), "several open callable types (#569 O1 evidence)"));
    }
    let protocol = module.protocols.alloc(catalogue::open_callable_protocol())?;
    if catalogue::open_callable_summary(module, callable, protocol).is_none() {
        return Err(LowerError::unsupported(Span::default(), "open asynchronous callable (#565 commit 5)"));
    }
    let summary = module.summaries.alloc(catalogue::sync_closure_cleanup_summary())?;
    let signature = catalogue::sync_closure_cleanup_signature(module, callable, ir::Profile::High)
        .ok_or_else(|| LowerError::unsupported(Span::default(), "open callable cleanup ABI"))?;
    let cleanup = module.functions.alloc(ir::Function {
        symbol: "oir.sync_closure.cleanup".into(),
        signature,
        attrs: vec![ir::FunctionAttr::Intrinsic {
            name: catalogue::SYNC_CLOSURE_CLEANUP.into(), objects: vec![], helpers: vec![],
        }],
        type_params: vec![],
        summary: Some(summary),
        async_creation_summary: None,
        async_body_summary: None,
        body: None,
        loc: None,
    })?;
    let descriptor = module.types.intern(ir::Type::Descriptor)?;
    for (function, _) in sites {
        let Some(mut body) = module.functions.get_mut(function)?.body.take() else { continue };
        let entry = body.entry;
        let op = ir::Operation::Core(ir::CoreOp::TypeOf {
            ty: callable,
            descriptor: ir::DescriptorContract {
                managed: ir::DescriptorManaged::Runtime,
                retention: ir::SummarySelection::OwnerGraph(0),
                cleanup,
                async_destroy: false,
                summary,
            },
        });
        ir::FunctionEditor::new(module, &mut body).insert_instruction(entry, 0, op,
            &[ir::ResultSpec::new(descriptor)], ir::InstructionFacts::default(), None)?;
        module.functions.get_mut(function)?.body = Some(body);
    }
    Ok(())
}

/// The callee reaches an entry parameter through block parameters (a loop-carried
/// or merged `fn` parameter), the open case of own's origin analysis.
fn reaches_entry(body: &ir::Body, value: ir::ValueId) -> Result<bool> {
    let mut pending = vec![value];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(value) = pending.pop() {
        if !seen.insert(value) {
            continue;
        }
        let ir::Definition::BlockParam { block, index } = body.values.get(value)?.def else {
            continue;
        };
        if block == body.entry {
            return Ok(true);
        }
        for (_, source) in body.blocks.iter() {
            let mut targets = vec![];
            if let Some(unwind) = &source.unwind {
                targets.push(&unwind.target);
            }
            match &source.term.kind {
                ir::TermKind::Br(target) => targets.push(target),
                ir::TermKind::Cond { yes, no, .. } => targets.extend([yes, no]),
                ir::TermKind::Switch { cases, default, .. } => {
                    targets.extend(cases.iter().map(|case| &case.target));
                    targets.push(default);
                }
                _ => {}
            }
            for target in targets.into_iter().filter(|target| target.block == block) {
                if let Some(argument) = target.args.get(index as usize) {
                    pending.push(*argument);
                }
            }
        }
    }
    Ok(false)
}
