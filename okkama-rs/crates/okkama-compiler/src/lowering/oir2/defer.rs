//! Lazy defer entries. Binding captures are supplied at execution, after snapshots. A deferred expression
//! that awaits (`defer await x.close();`) is an async defer entry (async ABI C-6).
use std::collections::HashSet;

use super::{LowerError, Result};
use crate::analysis::{thir, ty::Ty};
use crate::oir2 as ir;

pub(super) struct Deferred {
    pub function: ir::FuncId,
    pub source: thir::Body,
    pub receiver: bool,
    pub key: super::callables::BodyKey,
}

impl super::declarations::Declarations {
    pub fn declare_defers(&mut self, source: &thir::Body, owner: ir::FuncId,
        key: &super::callables::BodyKey) -> Result<()> {
        let mut expressions = vec![];
        scan_block(&source.block, &mut |stmt| {
            if let thir::StmtKind::Defer(expr) = &stmt.kind {
                expressions.push(expr.clone());
            }
        });
        let symbol = self.builder.module().functions.get(owner)?.symbol.clone();
        for (number, expr) in expressions.into_iter().enumerate() {
            let mut defined = HashSet::new();
            let mut returns = false;
            scan_expr(&expr, &mut |stmt| {
                if let thir::StmtKind::Let { local, .. } = stmt.kind {
                    defined.insert(local);
                }
                // Include nested defers: their replacement must leave this helper too.
                returns |= matches!(stmt.kind, thir::StmtKind::Return(_));
            });
            let block = thir::Block {
                stmts: vec![thir::Stmt { span: expr.span, kind: thir::StmtKind::Expr(expr.clone()) }],
                span: expr.span,
            };
            super::walk::block(&block, &mut |expr| {
                if let thir::ExprKind::For { binding, .. } = expr.kind {
                    defined.insert(binding);
                }
                if let thir::ExprKind::Functional(functional) = &expr.kind {
                    defined.extend(functional.bindings.iter().copied());
                }
                if let thir::ExprKind::Try(found) = &expr.kind {
                    for catch in &found.catches {
                        defined.insert(catch.local);
                    }
                }
            });
            let params: Vec<_> = source.locals.iter().enumerate().filter_map(|(index, _)| {
                let local = thir::LocalId(index);
                (!defined.contains(&local) && thir::expr_mentions(&expr, local) != 0).then_some(local)
            }).collect();
            let mut body = source.clone();
            body.block = block;
            body.params = params;
            body.returns_value = false;
            body.span = expr.span;
            let receiver = thir::uses_receiver(&body);
            let mut types = vec![];
            let replacements = if returns {
                vec![ir::DeferLabel {
                    label: "return".into(),
                    result: self.builder.module().functions.get(owner)?.signature.results.first().copied(),
                }]
            } else {
                vec![]
            };
            // An expression that awaits makes an async defer entry (async ABI C-6), which has no replacement.
            let awaits = super::asynchronous::awaits(&expr);
            if awaits && returns {
                return Err(super::asynchronous::async_defer_replace(expr.span));
            }
            let mut attrs = vec![ir::FunctionAttr::DeferEntry { replacements }];
            if awaits {
                attrs.push(ir::FunctionAttr::Async);
            }
            if receiver {
                let object = match key.root {
                    thir::BodyOwner::Method { object, .. } | thir::BodyOwner::Ctor { object, .. }
                    | thir::BodyOwner::Default { object, .. } | thir::BodyOwner::Destroy { object }
                    | thir::BodyOwner::CapField { object, .. } => object,
                    _ => return Err(LowerError::unsupported(expr.span, "defer receiver without object owner")),
                };
                types.push(self.types[&Ty::object(object)]);
                // The helper retains field-read authority, without constructor/successor authority.
                attrs.push(ir::FunctionAttr::Method(self.objects[&object]));
            }
            for param in &body.params {
                let local = &body.locals[param.0];
                types.push(*self.types.get(&local.ty).ok_or_else(|| {
                    LowerError::unsupported(local.span, "defer capture type")
                })?);
            }
            let function = self.builder.declare_function(ir::Function {
                symbol: format!("{symbol}.defer#{number}"),
                signature: ir::Signature {
                    params: types, results: vec![], ownership_abi: ir::OwnershipAbi::Unelaborated,
                },
                attrs, type_params: vec![], summary: None,
                async_creation_summary: None, async_body_summary: None, body: None, loc: None,
            })?;
            if awaits {
                super::asynchronous::declare(&mut self.builder, function)?;
            }
            self.defers.push(Deferred { function, source: body, receiver, key: key.clone() });
        }
        Ok(())
    }
}

impl super::body::BodyLowerer<'_, '_> {
    pub fn register_defer(&mut self, expr: &thir::Expr) -> Result<()> {
        let deferred = self.context.defers.iter().find(|entry| {
            entry.key == self.context.key && entry.source.span == expr.span
        }).ok_or_else(|| LowerError::invalid(expr.span, "missing defer declaration"))?;
        let function = deferred.function;
        let locals = deferred.source.params.clone();
        let captures = if deferred.receiver { vec![self.self_value()?] } else { vec![] };
        let mut bindings = vec![];
        for local in locals {
            bindings.push(self.bindings[local.0].ok_or_else(|| {
                LowerError::invalid(expr.span, "defer capture without dominating initial bind")
            })?);
        }
        let scope = self.scope();
        let replacements = self.b.module().functions.get(function)?.attrs.iter().find_map(|attr| {
            if let ir::FunctionAttr::DeferEntry { replacements } = attr {
                Some(replacements.iter().map(|label| ir::DeferRegistration {
                    label: label.label.clone(), kind: ir::ExitKind::Return, boundary: self.return_boundary,
                }).collect())
            } else {
                None
            }
        }).ok_or_else(|| LowerError::invalid(expr.span, "defer helper without tagged entry"))?;
        self.b.emit(ir::Operation::Lexical(ir::LexicalOp::DeferRegister {
            scope, action: ir::DeferAction { function, captures, bindings, replacements },
        }), &[])?;
        self.registrations.entry(scope).or_default().push(function);
        // Before/after registration have distinct failure points and entry-visible environments.
        let locals = self.locals();
        let types = self.local_types(&locals);
        let next = self.new_block(scope, &types)?;
        let args = self.args(&locals);
        self.terminate(ir::TermKind::Br(ir::Target { block: next, args }))?;
        self.enter(next, &locals, 0)
    }
}

fn scan_block(block: &thir::Block, visit: &mut impl FnMut(&thir::Stmt)) {
    for stmt in &block.stmts {
        scan_stmt(stmt, visit);
    }
}

fn scan_stmt(stmt: &thir::Stmt, visit: &mut impl FnMut(&thir::Stmt)) {
    visit(stmt);
    if let thir::StmtKind::Guarded { condition, stmt } = &stmt.kind {
        scan_expr(condition, visit);
        scan_stmt(stmt, visit);
        return;
    }
    use thir::StmtKind as S;
    match &stmt.kind {
        S::Let { value, .. } | S::Expr(value) | S::Throw(value) | S::Defer(value)
        | S::BindField { value, .. } | S::StageField { value, .. } | S::Rebind { value, .. } => {
            scan_expr(value, visit);
        }
        S::Return(value) | S::Last(value) => {
            if let Some(value) = value {
                scan_expr(value, visit);
            }
        }
        S::Confirm { condition, fallback } => {
            scan_expr(condition, visit);
            if let Some(thir::Fallback::Throw(value) | thir::Fallback::Panic(value)) = fallback {
                scan_expr(value, visit);
            }
        }
        S::Guarded { .. } => unreachable!("handled above"),
        S::Rethrow | S::Next => {}
    }
}

fn scan_expr(expr: &thir::Expr, visit: &mut impl FnMut(&thir::Stmt)) {
    use thir::ExprKind as E;
    match &expr.kind {
        E::Block(block) | E::Loop(block) => scan_block(block, visit),
        E::If { condition, then_block, else_block } => {
            scan_expr(condition, visit);
            scan_block(then_block, visit);
            if let Some(block) = else_block {
                scan_block(block, visit);
            }
        }
        E::Match(found) => {
            scan_expr(&found.subject, visit);
            for (patterns, block) in &found.cases {
                for pattern in patterns {
                    scan_expr(pattern, visit);
                }
                scan_block(block, visit);
            }
            if let Some(block) = &found.else_block {
                scan_block(block, visit);
            }
        }
        E::Try(found) => {
            scan_block(&found.body, visit);
            for catch in &found.catches {
                scan_block(&catch.body, visit);
            }
        }
        E::Functional(functional) => {
            match &functional.source {
                thir::Source::Range { start, end, .. } => {
                    scan_expr(start, visit);
                    scan_expr(end, visit);
                }
                thir::Source::Array(expr) | thir::Source::Map(expr) | thir::Source::Iterating(expr)
                | thir::Source::Batching(expr) => scan_expr(expr, visit),
                thir::Source::Varargs(_) => {}
            }
            if let Some(initial) = &functional.initial {
                scan_expr(initial, visit);
            }
            scan_block(&functional.body, visit);
            if let Some(found) = &functional.found {
                scan_expr(found, visit);
            }
        }
        E::For { source, body, .. } => {
            match source {
                thir::Source::Range { start, end, .. } => {
                    scan_expr(start, visit);
                    scan_expr(end, visit);
                }
                thir::Source::Array(expr) | thir::Source::Map(expr) | thir::Source::Iterating(expr)
                | thir::Source::Batching(expr) => scan_expr(expr, visit),
                thir::Source::Varargs(_) => {}
            }
            scan_block(body, visit);
        }
        _ => thir::each_child(expr, &mut |expr| scan_expr(expr, visit)),
    }
}


/// A deferred constructor-field read needs a stable early-self reference too.
pub(super) fn uses_receiver(stmt: &thir::Stmt) -> bool {
    let mut receiver = false;
    scan_stmt(stmt, &mut |stmt| {
        if let thir::StmtKind::Defer(expr) = &stmt.kind {
            receiver |= thir::first_in(expr, &|expr| matches!(expr.kind,
                thir::ExprKind::SelfValue | thir::ExprKind::Field(_)
                | thir::ExprKind::PendingField(_) | thir::ExprKind::OwnerField(_))).is_some();
        }
    });
    receiver
}
