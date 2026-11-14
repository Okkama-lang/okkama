//! Anonymous action snapshots. Ownership adapters and retention elaboration belong to `own`.
use super::callables::{BodyKey, DiscoveredBody, Inventory, Snapshot};
use super::{LowerError, Result};
use crate::analysis::{thir, ty};
use crate::oir2 as ir;

pub(super) struct Lambda {
    pub function: ir::FuncId,
    pub source: DiscoveredBody,
}

impl super::declarations::Declarations {
    pub fn declare_lambdas(&mut self, inventory: &Inventory) -> Result<()> {
        for entry in inventory.bodies.iter().filter(|entry| !entry.key.path.is_empty()) {
            let mut parent = entry.key.clone();
            let ordinal = parent.path.pop().expect("nested body");
            let enclosing = inventory.bodies.iter().find(|body| body.key == parent)
                .ok_or_else(|| LowerError::invalid(entry.body.span, "missing lexical parent"))?;
            let mut closures = vec![];
            super::walk::block(&enclosing.body.block, &mut |expr| {
                if matches!(expr.kind, thir::ExprKind::Closure { .. }) {
                    closures.push(expr.ty.clone());
                }
            });
            let Some(ty::Ty::Action(action)) = &closures[ordinal] else {
                return Err(LowerError::invalid(entry.body.span, "lambda without action signature"));
            };
            let mut params = entry.captures.iter().map(|capture| {
                // Frontend flow has already proved carried must-use obligations. High entries
                // transport the ordinary value; succ.take performs the required boundary conversion.
                let ty = match &capture.ty {
                    ty::Ty::Successor(inner) => inner.as_ref(),
                    ty => ty,
                };
                self.ty(ty, entry.body.span)
            }).collect::<Result<Vec<_>>>()?;
            params.extend(action.params.iter().map(|ty| self.ty(ty, entry.body.span)).collect::<Result<Vec<_>>>()?);
            let results = action.result.iter().map(|ty| self.ty(ty, entry.body.span)).collect::<Result<_>>()?;
            let root = self.functions[&entry.key.root];
            let path = entry.key.path.iter().map(usize::to_string).collect::<Vec<_>>().join(".");
            let symbol = format!("{}.lambda#{path}", self.builder.module().functions.get(root)?.symbol);
            let owner = super::specialize::owner_object(&entry.key.root).map(|object| self.objects[&object]);
            let captures = u32::try_from(entry.captures.len())
                .map_err(|_| LowerError::unsupported(entry.body.span, "lambda capture limit"))?;
            let function = self.builder.declare_function(ir::Function {
                symbol,
                signature: ir::Signature { params, results, ownership_abi: ir::OwnershipAbi::Unelaborated },
                attrs: vec![ir::FunctionAttr::Lambda { captures, owner }],
                type_params: vec![], summary: None, async_creation_summary: None, async_body_summary: None,
                body: None, loc: None,
            })?;
            let mut source = entry.clone();
            for capture in &entry.captures {
                if let (Some(local), ty::Ty::Successor(inner)) = (capture.target, &capture.ty) {
                    source.body.locals[local.0].ty = (**inner).clone();
                }
            }
            self.lambdas.push(Lambda { function, source });
        }
        // Reserve every lambda before scanning its lazy helpers, including mutually referenced bodies.
        for index in 0..self.lambdas.len() {
            let lambda = &self.lambdas[index];
            let source = lambda.source.clone();
            let function = lambda.function;
            self.declare_defers(&source.body, function, &source.key)?;
        }
        Ok(())
    }
}

fn child_of(child: &BodyKey, parent: &BodyKey) -> bool {
    child.root == parent.root && child.path.len() == parent.path.len() + 1
        && child.path.starts_with(&parent.path)
}

impl super::body::BodyLowerer<'_, '_> {
    pub fn anonymous(&mut self, expr: &thir::Expr) -> Result<Option<ir::ValueId>> {
        let thir::ExprKind::Closure { body, .. } = &expr.kind else {
            unreachable!("anonymous dispatch");
        };
        // The key identifies the lexical body; the span selects its creation expression even inside defer.
        let lambda = self.context.lambdas.iter().find(|lambda| {
            child_of(&lambda.source.key, &self.context.key) && lambda.source.body.span == body.span
        }).ok_or_else(|| LowerError::invalid(expr.span, "missing discovered lambda"))?;
        let function = lambda.function;
        let layout = lambda.source.captures.clone();
        let mut captures = vec![];
        for capture in layout {
            let value = match capture.source {
                Snapshot::Receiver => self.self_value()?,
                Snapshot::Parameter(local) | Snapshot::Local(local) => self.env[local.0]
                    .ok_or_else(|| LowerError::invalid(expr.span, "capture without current SSA value"))?,
            };
            captures.push(self.take(value)?);
        }
        let ty = self.ty(expr.ty.as_ref().expect("typed anonymous action"), expr.span)?;
        self.core(ir::CoreOp::FnClosure { function, captures, ty, packet: None }).map(Some)
    }

    pub fn invoke_anonymous(&mut self, expr: &thir::Expr) -> Result<Option<ir::ValueId>> {
        let thir::ExprKind::CallValue { callee, args } = &expr.kind else {
            unreachable!("anonymous invocation dispatch");
        };
        let Some(callee) = self.value(callee)? else {
            return Ok(None);
        };
        let mut values = vec![];
        for arg in args {
            let Some(value) = self.value(arg)? else {
                return Ok(None);
            };
            values.push(self.take(value)?);
        }
        let args = values.into_iter().map(|value| ir::CallArgument {
            value, convention: ir::OperandConvention::Borrow,
        }).collect();
        let call = ir::CallSite {
            args, packet: None, deps: vec![], descriptors: vec![], type_args: vec![], summary: None,
            cleanup: ir::CallCleanupMode::Ordinary,
        };
        self.sync_failure()?;
        let values = self.b.emit_schema(ir::Operation::Call(ir::CallOp::Closure {
            callee: ir::CallArgument { value: callee, convention: ir::OperandConvention::Borrow }, call,
        }))?;
        let block = self.current.expect("open invocation block");
        let instruction = *self.b.body().blocks.get(block)?.insts.last().expect("emitted invocation");
        // Match direct-call admission: the known target's body effects are inferred later.
        self.b.add_high_effect(ir::HighEffectRecord {
            site: ir::HighOperationSite::Instruction(instruction),
            effects: ir::HighEffectEnvelope {
                failures: vec![ir::FailureKind::Thrown, ir::FailureKind::Program, ir::FailureKind::Panic,
                    ir::FailureKind::Native, ir::FailureKind::Cancelled],
                may_suspend: true,
            },
        })?;
        // Preserve a source diagnostic for the post-CFG known-origin gate.
        let function = self.b.module().scopes.get(self.root)?.function;
        self.context.calls.borrow_mut().insert((function, instruction), expr.span);
        Ok(values.first().copied())
    }
}
