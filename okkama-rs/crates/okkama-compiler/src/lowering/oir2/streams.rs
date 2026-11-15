//! `sink <- source` (#569) as the ordinary THIR it means, before specialization walks it:
//! `Writing.write(source as ByteSource)` on the sink, with the sink as the value, like OIR-1.
//! `sink <- xs...` writes every element in order. A chain `out <- a <- b` writes left to
//! right to the same sink. The sink is a local binding, so naming it again evaluates nothing.
use super::{LowerError, Result};
use crate::analysis::{Registry, thir::{self, LocalId}, ty::{RoleId, Ty}};
use crate::source::Span;

pub(super) struct StreamRoles {
    writing: RoleId,
    slot: usize,
    byte_source: RoleId,
}

impl StreamRoles {
    /// The io/stream roles and the `write(source is ByteSource)` slot of Writing.
    pub fn find(registry: &Registry<'_>, span: Span) -> Result<Self> {
        let missing = || LowerError::invalid(span, "`<-` without the io/stream roles");
        let writing = registry.standard.writing.ok_or_else(missing)?;
        let byte_source = registry.standard.byte_source.ok_or_else(missing)?;
        let slot = registry.roles[writing.0].methods.iter().position(|method| {
            method.name == "write" && method.params.iter().map(|(_, ty)| ty).eq([&Ty::role(byte_source)])
        }).ok_or_else(missing)?;
        Ok(Self { writing, slot, byte_source })
    }
}

/// Rewrites one `StreamWrite` chain in place into a block; `fresh` declares element bindings.
pub(super) fn desugar(expr: &mut thir::Expr, roles: &StreamRoles,
    fresh: &mut dyn FnMut(Ty, Span) -> Result<LocalId>) -> Result<()> {
    let mut writes = vec![];
    let mut sink = expr.clone();
    while let thir::ExprKind::StreamWrite { sink: inner, source, unpack } = sink.kind {
        writes.push((*source, unpack));
        sink = *inner;
    }
    writes.reverse();
    if !matches!(sink.kind, thir::ExprKind::Local(_)) {
        return Err(LowerError::unsupported(sink.span, "stream write to a computed sink"));
    }
    let span = expr.span;
    let mut stmts = vec![];
    for (source, unpack) in writes {
        if !unpack {
            stmts.push(statement(write(&sink, source, roles)));
            continue;
        }
        let element = match source.ty.as_ref().map(Ty::taken) {
            Some(Ty::Array(element) | Ty::Varargs(element)) => *element,
            _ => return Err(LowerError::invalid(source.span, "`<- xs...` of a non-array")),
        };
        let binding = fresh(element.clone(), source.span)?;
        let item = thir::Expr {
            kind: thir::ExprKind::Local(binding), ty: Some(element), diverges: false, span: source.span,
        };
        let body = thir::Block { stmts: vec![statement(write(&sink, item, roles))], span: source.span };
        let source_span = source.span;
        let each = thir::ExprKind::For { source: thir::Source::Array(Box::new(source)), binding, body };
        stmts.push(statement(thir::Expr { kind: each, ty: None, diverges: false, span: source_span }));
    }
    stmts.push(statement(sink.clone()));
    *expr = thir::Expr {
        kind: thir::ExprKind::Block(thir::Block { stmts, span }), ty: sink.ty.clone(), diverges: false, span,
    };
    Ok(())
}

fn write(sink: &thir::Expr, source: thir::Expr, roles: &StreamRoles) -> thir::Expr {
    let target = Ty::role(roles.byte_source);
    let span = source.span;
    let argument = if source.ty.as_ref() == Some(&target) {
        source
    } else {
        thir::Expr {
            kind: thir::ExprKind::Coerce { coercion: thir::Coercion::RoleWrap, value: Box::new(source) },
            ty: Some(target), diverges: false, span,
        }
    };
    let callee = thir::Callee::Role { receiver: Box::new(sink.clone()), role: roles.writing, slot: roles.slot };
    thir::Expr { kind: thir::ExprKind::Call { callee, args: vec![argument] }, ty: None, diverges: false, span }
}

fn statement(expr: thir::Expr) -> thir::Stmt {
    thir::Stmt { span: expr.span, kind: thir::StmtKind::Expr(expr) }
}
