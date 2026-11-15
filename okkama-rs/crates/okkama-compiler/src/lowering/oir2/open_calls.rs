//! #569 C3b: open action invocation. An invocation is open exactly where own
//! treats it as open: its callee has no finite known origin in the invoking body
//! (`own::open_callees`: entry parameters, produced results, field reads, calls
//! and the block parameters they reach). Each such invocation receives the
//! registered open sync callable ABI of the `okkama-oir` runtime catalogue: the
//! Closure protocol (once per module), the cleanup intrinsic of its `fn` type (one
//! per type, the catalogue's typed key) and, in the invoking function, the
//! `type.of` runtime descriptor of that type. The call itself stays unsummarized
//! in High; own takes its contract from the catalogue. Known invocations get
//! nothing here: own adapts their finite targets precisely.
use std::collections::{BTreeSet, HashMap};

use super::{LowerError, Result};
use crate::oir2 as ir;
use crate::source::Span;
use ir::catalogue;

/// Source spans of action-value invocations, for diagnostics after the CFG is built.
pub(super) type CallSpans = HashMap<(ir::FuncId, ir::InstId), Span>;

/// One open invocation: the invoking function, the callable type and its span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OpenSite {
    pub function: ir::FuncId,
    pub callable: ir::TypeId,
    pub span: Span,
}

/// The invocations own treats as open, in function and block order.
pub(super) fn open_sites(module: &ir::Module, spans: &CallSpans) -> Result<Vec<OpenSite>> {
    let options = ir::own::OwnOptions::default();
    let mut sites = vec![];
    for (function, data) in module.functions.iter() {
        let Some(body) = &data.body else { continue };
        let open = ir::own::open_callees(module, body, &options).map_err(|error| {
            LowerError::unsupported(Span::default(), format!("open callable origins: {error:?}"))
        })?;
        for (_, block) in body.blocks.iter() {
            let mut calls = vec![];
            for &inst in &block.insts {
                if let ir::Operation::Call(ir::CallOp::Closure { callee, .. }) = &body.insts.get(inst)?.op {
                    calls.push((callee.value, spans.get(&(function, inst)).copied()));
                }
            }
            if let ir::TermKind::InvokeOwn { op, .. } = &block.term.kind
                && let ir::Operation::Call(ir::CallOp::Closure { callee, .. }) = op.as_ref() {
                calls.push((callee.value, None));
            }
            for (callee, span) in calls {
                if !open.contains(&callee) {
                    continue;
                }
                let callable = body.values.get(callee)?.ty;
                sites.push(OpenSite { function, callable, span: span.unwrap_or_default() });
            }
        }
    }
    Ok(sites)
}

pub(super) fn attach(module: &mut ir::Module, spans: &CallSpans) -> Result<()> {
    let sites = open_sites(module, spans)?;
    if sites.is_empty() {
        return Ok(());
    }
    let protocol = module.protocols.alloc(catalogue::open_callable_protocol())?;
    let mut callables = vec![];
    for site in &sites {
        if !matches!(module.types.get(site.callable)?, ir::Type::Fn(_)) {
            return Err(LowerError::invalid(site.span, "open invocation of a non-action value"));
        }
        if catalogue::open_callable_summary(module, site.callable, protocol).is_none() {
            return Err(LowerError::unsupported(site.span, "open asynchronous callable (#565 commit 5)"));
        }
        if !callables.contains(&site.callable) {
            callables.push(site.callable);
        }
    }
    let summary = module.summaries.alloc(catalogue::sync_closure_cleanup_summary())?;
    let mut cleanups = HashMap::new();
    for (index, &callable) in callables.iter().enumerate() {
        let signature = catalogue::sync_closure_cleanup_signature(module, callable, ir::Profile::High)
            .ok_or_else(|| LowerError::unsupported(Span::default(), "open callable cleanup ABI"))?;
        let symbol = if index == 0 {
            catalogue::SYNC_CLOSURE_CLEANUP.to_owned()
        } else {
            format!("{}#{index}", catalogue::SYNC_CLOSURE_CLEANUP)
        };
        let cleanup = module.functions.alloc(ir::Function {
            symbol,
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
        cleanups.insert(callable, cleanup);
    }
    let descriptor = module.types.intern(ir::Type::Descriptor)?;
    let wanted: BTreeSet<_> = sites.iter().map(|site| (site.function, site.callable)).collect();
    for (function, callable) in wanted {
        let Some(mut body) = module.functions.get_mut(function)?.body.take() else { continue };
        let entry = body.entry;
        let op = ir::Operation::Core(ir::CoreOp::TypeOf {
            ty: callable,
            descriptor: ir::DescriptorContract {
                managed: ir::DescriptorManaged::Runtime,
                retention: ir::SummarySelection::OwnerGraph(0),
                cleanup: cleanups[&callable],
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
