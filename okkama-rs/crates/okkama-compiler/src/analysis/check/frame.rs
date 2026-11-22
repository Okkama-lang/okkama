//! Local non-escaping Cell actions (#957). One lexical walk per body, with
//! capture status passed to nested bodies; no call graph or fixed point.
//! Action locals are immutable (only object successors can be rebound).
use crate::analysis::{
    registry::Registry,
    thir::*,
    ty::{ObjectId, Ty},
};

pub(super) fn check(reg: &mut Registry<'_>, body: &Body, receiver: Option<&Ty>) {
    let Some(cell) = reg.standard.cell else {
        return;
    };
    let mut frame = Frame {
        reg,
        cell,
        locals: vec![false; body.locals.len()],
        receiver_cell: receiver.is_some_and(|ty| ty.holds_object(cell)),
        uses_receiver: false,
    };
    frame.body(body);
}

struct Frame<'r, 'a> {
    reg: &'r mut Registry<'a>,
    cell: ObjectId,
    locals: Vec<bool>,
    receiver_cell: bool,
    uses_receiver: bool,
}

impl Frame<'_, '_> {
    fn body(&mut self, body: &Body) {
        self.reg.set_module(body.module);
        self.block(
            &body.block,
            Some(if body.returns_value {
                "returned"
            } else {
                "discarded"
            }),
        );
    }

    // None permits propagation to a local, capture or direct synchronous call.
    fn block(&mut self, block: &Block, usage: Option<&str>) -> bool {
        let mut bound = false;
        for (index, stmt) in block.stmts.iter().enumerate() {
            let tail = index + 1 == block.stmts.len();
            let value = self.stmt(stmt, if tail { usage } else { Some("discarded") }, tail);
            if tail {
                bound = value;
            }
        }
        bound
    }

    fn stmt(&mut self, stmt: &Stmt, usage: Option<&str>, tail: bool) -> bool {
        match &stmt.kind {
            StmtKind::Let { local, value, once } => {
                let bound = self.expr(
                    value,
                    if *once {
                        Some("stored by once let")
                    } else {
                        None
                    },
                );
                self.locals[local.0] |= bound;
                if tail {
                    if bound && let Some(usage) = usage.filter(|usage| *usage != "discarded") {
                        self.reject(value, usage);
                    }
                    return bound;
                }
            }
            StmtKind::Rebind { local, value } => {
                self.locals[local.0] |= self.expr(value, None);
            }
            StmtKind::Expr(value) => return self.expr(value, usage),
            StmtKind::Guarded { condition, stmt } => {
                self.expr(condition, Some("used as a condition"));
                return self.stmt(stmt, usage, tail);
            }
            StmtKind::Return(Some(value)) => {
                self.expr(value, Some("returned"));
            }
            StmtKind::Last(Some(value)) => {
                self.expr(value, Some("used as a loop result"));
            }
            StmtKind::BindField { value, .. } | StmtKind::StageField { value, .. } => {
                self.expr(value, Some("stored in a field"));
            }
            StmtKind::Defer(value) | StmtKind::Throw(value) => {
                self.expr(value, Some("used here"));
            }
            StmtKind::Confirm {
                condition,
                fallback,
            } => {
                self.expr(condition, Some("used as a condition"));
                if let Some(Fallback::Throw(value) | Fallback::Panic(value)) = fallback {
                    self.expr(value, Some("used as a failure"));
                }
            }
            StmtKind::Return(None) | StmtKind::Last(None) | StmtKind::Next | StmtKind::Rethrow => {}
        }
        false
    }

    fn source(&mut self, source: &Source) {
        match source {
            Source::Range { start, end, .. } => {
                self.expr(start, Some("used as a range"));
                self.expr(end, Some("used as a range"));
            }
            Source::Array(value)
            | Source::Map(value)
            | Source::Iterating(value)
            | Source::Batching(value) => {
                self.expr(value, Some("iterated"));
            }
            Source::Varargs(_) => {}
        }
    }

    fn expr(&mut self, expr: &Expr, usage: Option<&str>) -> bool {
        let bound = match &expr.kind {
            ExprKind::Local(local) => self.locals[local.0],
            ExprKind::Closure { body, captures } => {
                let mut locals = vec![false; body.locals.len()];
                let mut bound = false;
                for (outer, inner) in captures {
                    locals[inner.0] = self.locals[outer.0];
                    bound |= locals[inner.0] || body.locals[inner.0].ty.holds_object(self.cell);
                }
                let module = self.reg.current_module();
                let mut nested = Frame {
                    reg: self.reg,
                    cell: self.cell,
                    locals,
                    receiver_cell: self.receiver_cell,
                    uses_receiver: false,
                };
                nested.body(body);
                let uses_receiver = nested.uses_receiver;
                bound |= self.receiver_cell && uses_receiver;
                self.uses_receiver |= uses_receiver;
                self.reg.set_module(module);
                if bound && body.is_async {
                    self.reject(expr, "used as an async action");
                }
                bound
            }
            ExprKind::BoundAction { receiver, .. } | ExprKind::BoundRoleAction { receiver, .. } => {
                let bound = self.expr(receiver, Some("used as a receiver"));
                bound
                    || receiver
                        .ty
                        .as_ref()
                        .is_some_and(|ty| ty.holds_object(self.cell))
            }
            ExprKind::Block(block) => self.block(block, None),
            ExprKind::If {
                condition,
                then_block,
                else_block,
            } => {
                self.expr(condition, Some("used as a condition"));
                let left = self.block(then_block, None);
                let right = else_block
                    .as_ref()
                    .is_some_and(|block| self.block(block, None));
                left || right
            }
            ExprKind::Match(found) => {
                self.expr(&found.subject, Some("matched"));
                let mut bound = false;
                for (patterns, block) in &found.cases {
                    for pattern in patterns {
                        self.expr(pattern, Some("used as a pattern"));
                    }
                    bound |= self.block(block, None);
                }
                if let Some(block) = &found.else_block {
                    bound |= self.block(block, None);
                }
                bound
            }
            ExprKind::CallValue { callee, args } => {
                let asynchronous =
                    matches!(&callee.ty, Some(Ty::Action(action)) if action.is_async);
                self.expr(callee, asynchronous.then_some("called asynchronously"));
                for arg in args {
                    self.expr(arg, Some("passed to an action"));
                }
                false
            }
            ExprKind::Spawn(call) => {
                match &call.kind {
                    ExprKind::CallValue { callee, args } => {
                        self.expr(callee, Some("spawned"));
                        for arg in args {
                            self.expr(arg, Some("passed to a task"));
                        }
                    }
                    _ => {
                        self.expr(call, Some("spawned"));
                    }
                }
                false
            }
            ExprKind::Loop(block) => {
                self.block(block, Some("used as a loop result"));
                false
            }
            ExprKind::For { source, body, .. } => {
                self.source(source);
                self.block(body, Some("used as a loop result"));
                false
            }
            ExprKind::Try(found) => {
                self.block(&found.body, Some("used as a try result"));
                for catch in &found.catches {
                    self.block(&catch.body, Some("used as a try result"));
                }
                false
            }
            ExprKind::Functional(found) => {
                self.source(&found.source);
                if let Some(initial) = &found.initial {
                    self.expr(initial, Some("used as an accumulator"));
                }
                if let Some(value) = &found.found {
                    self.expr(value, Some("stored by a functional operation"));
                }
                self.block(&found.body, Some("stored by a functional operation"));
                false
            }
            ExprKind::SelfValue
            | ExprKind::Field(_)
            | ExprKind::PendingField(_)
            | ExprKind::OwnerField(_) => {
                // Reuse the receiver-capture rule on these leaf forms; do not
                // rescan subtrees or derive a different capture policy.
                self.uses_receiver |= expr_uses_receiver(expr);
                false
            }
            // All remaining forms consume their operands. Storage/calls cannot
            // produce frame-bound values: their input/return boundaries reject them.
            // each_child is exhaustive, including casts, variant payloads, packs,
            // interpolation's calls, indexing fallbacks, caps and await.
            _ => {
                let why = match &expr.kind {
                    ExprKind::Call { .. } => "passed to an action",
                    ExprKind::Coerce { .. } => "hidden by a coercion",
                    ExprKind::Array(_)
                    | ExprKind::Map(_)
                    | ExprKind::Pack { .. }
                    | ExprKind::Unpack(_) => "stored in a collection",
                    ExprKind::VariantNew { .. } => "stored in a case payload",
                    ExprKind::Await(_) => "awaited",
                    _ => "used here",
                };
                each_child(expr, &mut |child| {
                    self.expr(child, Some(why));
                });
                false
            }
        };
        if bound && let Some(usage) = usage {
            self.reject(expr, usage);
        }
        bound
    }

    fn reject(&mut self, expr: &Expr, usage: &str) {
        self.reg.error(expr.span, format!("An action that retains a Cell cannot be {usage}: a cell stays in its synchronous frame; share state between tasks with Atomic"));
    }
}
