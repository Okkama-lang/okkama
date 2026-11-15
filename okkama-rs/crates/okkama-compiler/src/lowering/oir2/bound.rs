//! Bound receivers are SSA snapshots. Invocation guards live at the concrete method entry.
use std::collections::{HashMap, HashSet};

use super::{LowerError, Result, callables};
use crate::analysis::{thir, ty};
use crate::oir2 as ir;

pub(super) fn role_targets(module: &ir::Module, role: ir::DeclId, slot: u32) -> Result<Vec<ir::FuncId>> {
    let mut targets = vec![];
    for (_, declaration) in module.declarations.iter() {
        if let ir::DeclarationKind::Object { witnesses, .. } = &declaration.kind {
            for witness in witnesses {
                if matches!(module.types.get(witness.role)?, ir::Type::Role { decl, .. } if *decl == role)
                    && let Some(&target) = witness.slots.get(slot as usize) {
                    targets.push(target);
                }
            }
        }
    }
    targets.sort();
    targets.dedup();
    Ok(targets)
}

pub(super) fn entries(module: &ir::Module, inventory: &callables::Inventory,
    functions: &HashMap<thir::BodyOwner, ir::FuncId>, types: &HashMap<ty::Ty, ir::TypeId>)
    -> Result<HashSet<ir::FuncId>> {
    let mut entries = HashSet::new();
    for bound in &inventory.bound {
        match bound.target {
            callables::BoundTarget::Method { object, method } => {
                entries.insert(functions[&thir::BodyOwner::Method { object, method }]);
            }
            callables::BoundTarget::Role { role, slot } => {
                let ir::Type::Role { decl, .. } = *module.types.get(types[&ty::Ty::role(role)])? else {
                    unreachable!("specialized bound role");
                };
                for target in role_targets(module, decl, slot as u32)? {
                    entries.insert(target);
                    // A witness adapter can call the implementation; guard that concrete entry too.
                    if let Some(body) = &module.functions.get(target)?.body {
                        for (_, inst) in body.insts.iter() {
                            if let ir::Operation::Call(ir::CallOp::Direct { function, .. }) = inst.op {
                                entries.insert(function);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(entries)
}

impl super::body::BodyLowerer<'_, '_> {
    pub fn bound(&mut self, expr: &thir::Expr) -> Result<Option<ir::ValueId>> {
        let (thir::ExprKind::BoundAction { receiver, .. }
            | thir::ExprKind::BoundRoleAction { receiver, .. }) = &expr.kind else {
            unreachable!("bound action dispatch");
        };
        let Some(receiver) = self.value(receiver)? else {
            return Ok(None);
        };
        // Frontend must-use proof precedes this conversion, just as for anonymous captures.
        let receiver = self.take(receiver)?;
        let ty = self.ty(expr.ty.as_ref().expect("typed bound action"), expr.span)?;
        let op = match &expr.kind {
            thir::ExprKind::BoundAction { object, method, .. } => {
                let function = self.context.functions[&thir::BodyOwner::Method { object: *object, method: *method }];
                ir::CoreOp::FnBind { receiver, function, ty, packet: None }
            }
            thir::ExprKind::BoundRoleAction { role, slot, .. } => {
                let role_type = self.ty(&ty::Ty::role(*role), expr.span)?;
                let ir::Type::Role { decl: role, .. } = *self.b.module().types.get(role_type)? else {
                    return Err(LowerError::invalid(expr.span, "bound role without specialized declaration"));
                };
                let slot = u32::try_from(*slot)
                    .map_err(|_| LowerError::unsupported(expr.span, "bound role slot limit"))?;
                ir::CoreOp::FnBindRole { receiver, role, slot, ty, packet: None }
            }
            _ => unreachable!("bound action dispatch"),
        };
        self.core(op).map(Some)
    }
}
