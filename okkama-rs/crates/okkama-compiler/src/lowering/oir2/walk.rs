//! Reuse THIR traversal for finite call specialization discovery.
use crate::analysis::thir;

pub(super) fn block(block: &thir::Block, visit: &mut dyn FnMut(&thir::Expr)) {
    thir::each_block_expr(block, &mut |value| expr(value, visit));
}

fn expr(value: &thir::Expr, visit: &mut dyn FnMut(&thir::Expr)) {
    visit(value);
    thir::each_child(value, &mut |child| expr(child, visit));
}

/// Mutable traversal of one lexical body. Closure bodies are separate worklist entries.
pub(super) fn block_mut(block: &mut thir::Block, visit: &mut dyn FnMut(&mut thir::Expr)) {
    for stmt in &mut block.stmts {
        stmt_mut(stmt, visit);
    }
}

fn stmt_mut(stmt: &mut thir::Stmt, visit: &mut dyn FnMut(&mut thir::Expr)) {
    use thir::StmtKind as S;
    match &mut stmt.kind {
        S::Let { value, .. } | S::Expr(value) | S::Throw(value) | S::Defer(value)
        | S::BindField { value, .. } | S::StageField { value, .. } | S::Rebind { value, .. } => {
            expr_mut(value, visit);
        }
        S::Guarded { condition, stmt } => {
            expr_mut(condition, visit);
            stmt_mut(stmt, visit);
        }
        S::Return(value) | S::Last(value) => {
            if let Some(value) = value {
                expr_mut(value, visit);
            }
        }
        S::Confirm { condition, fallback } => {
            expr_mut(condition, visit);
            if let Some(thir::Fallback::Throw(value) | thir::Fallback::Panic(value)) = fallback {
                expr_mut(value, visit);
            }
        }
        S::Rethrow | S::Next => {}
    }
}

fn source_mut(source: &mut thir::Source, visit: &mut dyn FnMut(&mut thir::Expr)) {
    match source {
        thir::Source::Range { start, end, .. } => {
            expr_mut(start, visit);
            expr_mut(end, visit);
        }
        thir::Source::Array(value) | thir::Source::Map(value) | thir::Source::Iterating(value)
        | thir::Source::Batching(value) => expr_mut(value, visit),
        thir::Source::Varargs(_) => {}
    }
}

fn expr_mut(expr: &mut thir::Expr, visit: &mut dyn FnMut(&mut thir::Expr)) {
    use thir::ExprKind as E;
    visit(expr);
    match &mut expr.kind {
        E::Closure { .. } | E::Local(_) | E::Constant(_) | E::Global(_) | E::SelfValue | E::Field(_)
        | E::PendingField(_) | E::OwnerField(_) | E::EnumCase { .. } | E::TestCapability | E::Unreachable => {}
        E::VariantNew { values, .. } => {
            for value in values {
                expr_mut(value, visit);
            }
        }
        E::VariantTag(value) | E::VariantGet { value, .. } | E::Unary { operand: value, .. }
        | E::Coerce { value, .. } | E::Cast { value, .. } | E::Panic(value) | E::Unpack(value)
        | E::Await(value) | E::Spawn(value) | E::BoundAction { receiver: value, .. }
        | E::BoundRoleAction { receiver: value, .. } | E::CapSelect { owner: value, .. }
        | E::CapRead { cap: value, .. } => expr_mut(value, visit),
        E::Binary { left, right, .. } | E::Logical { left, right, .. } | E::Range { start: left, end: right, .. }
        | E::RatNew { numerator: left, denominator: right } | E::StreamWrite { sink: left, source: right, .. } => {
            expr_mut(left, visit);
            expr_mut(right, visit);
        }
        E::If { condition, then_block, else_block } => {
            expr_mut(condition, visit);
            block_mut(then_block, visit);
            if let Some(block) = else_block {
                block_mut(block, visit);
            }
        }
        E::Match(found) => {
            expr_mut(&mut found.subject, visit);
            for (patterns, block) in &mut found.cases {
                for pattern in patterns {
                    expr_mut(pattern, visit);
                }
                block_mut(block, visit);
            }
            if let Some(block) = &mut found.else_block {
                block_mut(block, visit);
            }
        }
        E::Loop(block) | E::Block(block) => block_mut(block, visit),
        E::For { source, body, .. } => {
            source_mut(source, visit);
            block_mut(body, visit);
        }
        E::Try(found) => {
            block_mut(&mut found.body, visit);
            for catch in &mut found.catches {
                block_mut(&mut catch.body, visit);
            }
        }
        E::Call { callee, args } => {
            match callee {
                thir::Callee::Method { receiver, .. } | thir::Callee::Role { receiver, .. }
                | thir::Callee::Builtin { receiver, .. } => expr_mut(receiver, visit),
                thir::Callee::Ctor { .. } | thir::Callee::Native(_) | thir::Callee::Test(_) => {}
            }
            for arg in args {
                expr_mut(arg, visit);
            }
        }
        E::CallValue { callee, args } => {
            expr_mut(callee, visit);
            for arg in args {
                expr_mut(arg, visit);
            }
        }
        E::Array(parts) => {
            for part in parts {
                match part {
                    thir::ArrayPart::Value(value) => expr_mut(value, visit),
                    thir::ArrayPart::Range { start, end, .. } => {
                        expr_mut(start, visit);
                        expr_mut(end, visit);
                    }
                }
            }
        }
        E::Map(entries) => {
            for (key, value) in entries {
                expr_mut(key, visit);
                expr_mut(value, visit);
            }
        }
        E::Index { object, index, fallback } => {
            expr_mut(object, visit);
            expr_mut(index, visit);
            if let Some(value) = fallback {
                expr_mut(value, visit);
            }
        }
        E::Functional(found) => {
            source_mut(&mut found.source, visit);
            if let Some(value) = &mut found.initial {
                expr_mut(value, visit);
            }
            if let Some(value) = &mut found.found {
                expr_mut(value, visit);
            }
            block_mut(&mut found.body, visit);
        }
        E::ArrayCast { parts, .. } => {
            for part in parts {
                let (thir::CastPart::One(value) | thir::CastPart::Spread(value)) = part;
                expr_mut(value, visit);
            }
        }
        E::Pack { items, spread } => {
            for value in items {
                expr_mut(value, visit);
            }
            if let Some(value) = spread {
                expr_mut(value, visit);
            }
        }
    }
}

/// Separate body traversal: each_child intentionally stops at anonymous action boundaries.
pub(super) fn bodies(body: &thir::Body, visit: &mut dyn FnMut(&thir::Body)) {
    visit(body);
    block(&body.block, &mut |expr| {
        if let thir::ExprKind::Closure { body, .. } = &expr.kind {
            bodies(body, visit);
        }
    });
}
