//! Capability snapshots retain the complete concrete owner, including unread resource fields.
use super::{LowerError, Result};
use crate::analysis::{Analysis, thir, ty};
use crate::oir2 as ir;

impl super::declarations::Declarations {
    pub fn reserve_caps(&mut self, analysis: &Analysis<'_>) -> Result<()> {
        for (index, object) in analysis.registry.objects.iter().enumerate() {
            let owner = ty::ObjectId(index);
            let Some(&declaration) = self.objects.get(&owner) else {
                continue;
            };
            let name = self.builder.module().declarations.get(declaration)?.name.clone();
            let owner_type = self.ty(&ty::Ty::object(owner), object.decl.expect("source object").span)?;
            for (cap, info) in object.caps.iter().enumerate() {
                let declaration = self.builder.declare(ir::Declaration {
                    name: format!("{name}.{}", info.name),
                    type_params: vec![],
                    kind: ir::DeclarationKind::Cap { owner: owner_type, fields: vec![] },
                    loc: None,
                })?;
                let lowered = self.builder.intern_type(ir::Type::Cap { decl: declaration, args: vec![] })?;
                self.types.insert(ty::Ty::Cap(owner, vec![], cap), lowered);
            }
        }
        Ok(())
    }

    pub fn define_caps(&mut self, analysis: &Analysis<'_>) -> Result<()> {
        for (index, object) in analysis.registry.objects.iter().enumerate() {
            let owner = ty::ObjectId(index);
            if !self.objects.contains_key(&owner) {
                continue;
            }
            for (cap, info) in object.caps.iter().enumerate() {
                let lowered = self.types[&ty::Ty::Cap(owner, vec![], cap)];
                let ir::Type::Cap { decl, .. } = *self.builder.module().types.get(lowered)? else {
                    unreachable!("reserved cap type");
                };
                let mut fields = vec![];
                for (field, (name, _, _)) in info.fields.iter().enumerate() {
                    let key = thir::BodyOwner::CapField { object: owner, cap, field };
                    let function = self.functions.get(&key).copied().ok_or_else(|| {
                        LowerError::invalid(info.span, "missing discovered computed-field function")
                    })?;
                    fields.push(ir::ComputedField { name: name.clone(), function });
                }
                let ir::DeclarationKind::Cap { fields: target, .. } =
                    &mut self.builder.raw_mut().declarations.get_mut(decl)?.kind else {
                    unreachable!("reserved cap declaration");
                };
                *target = fields;
            }
        }
        Ok(())
    }
}

impl super::body::BodyLowerer<'_, '_> {
    pub fn cap_expr(&mut self, expr: &thir::Expr) -> Result<Option<ir::ValueId>> {
        match &expr.kind {
            thir::ExprKind::CapSelect { owner, .. } => {
                let Some(owner) = self.value(owner)? else {
                    return Ok(None);
                };
                let owner = self.take(owner)?;
                let ty = self.ty(expr.ty.as_ref().expect("typed cap selection"), expr.span)?;
                // Keep the owner SSA value itself. Projecting the fields used by the view
                // would lose hidden ownership and change snapshot semantics after a rebind.
                self.core(ir::CoreOp::CapMake { owner, ty, packet: None }).map(Some)
            }
            thir::ExprKind::CapRead { cap, field, .. } => {
                let Some(value) = self.value(cap)? else {
                    return Ok(None);
                };
                let ty = self.b.body().values.get(value)?.ty;
                let ir::Type::Cap { decl, .. } = *self.b.module().types.get(ty)? else {
                    return Err(LowerError::invalid(expr.span, "computed read without concrete cap"));
                };
                let field = ir::FieldRef { declaration: decl, index: *field as u32 };
                let call = ir::CallSite {
                    args: vec![], packet: None, deps: vec![], descriptors: vec![], type_args: vec![],
                    summary: None, cleanup: ir::CallCleanupMode::Ordinary,
                };
                // Each read is a call; acquisition never executes or caches this body.
                let cap = ir::CallArgument { value, convention: ir::OperandConvention::Borrow };
                self.emit_high_call(ir::CallOp::Cap { cap, field, call }, None)
            }
            _ => unreachable!("cap expression dispatch"),
        }
    }
}
