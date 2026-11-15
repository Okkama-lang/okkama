//! Plain once slots use the ordinary concrete Maybe layout and failure CFG.
use super::body::BodyLowerer;
use super::{LowerError, Result};
use crate::analysis::thir;
use crate::oir2 as ir;

impl BodyLowerer<'_, '_> {
    pub(super) fn once_let(&mut self, local: thir::LocalId, value: &thir::Expr) -> Result<()> {
        let thir::BodyOwner::Method { method, .. } = self.source.owner else {
            return Err(LowerError::invalid(value.span, "once let outside an object's action"));
        };
        let name = format!("$once{method}_{}", local.0);
        let layout = self.layout()?;
        let index = layout.iter().position(|field| field.once && field.name == name)
            .ok_or_else(|| LowerError::invalid(value.span, "once let without a concrete slot"))?;
        let field = self.field(index)?;
        let object = self.receiver.expect("once receiver");
        let slot = self.core(ir::CoreOp::OnceLoad { object, field })?;
        let tag = self.core(ir::CoreOp::VariantTag { value: slot })?;
        let zero = self.int(0)?;
        let condition = self.core(ir::CoreOp::IntEq { left: tag, right: zero })?;
        let scope = self.scope();
        let found = self.new_block(scope, &[])?;
        let compute = self.new_block(scope, &[])?;
        let ty = self.ty(&self.source.locals[local.0].ty, value.span)?;
        let join = self.join(scope, Some(ty))?;
        let env = self.env.clone();
        self.terminate(ir::TermKind::Cond {
            condition,
            yes: ir::Target { block: found, args: vec![] },
            no: ir::Target { block: compute, args: vec![] },
        })?;
        self.switch(found)?;
        let ir::Type::Variant(declaration) = *self.b.module().types.get(layout[index].ty)? else {
            return Err(LowerError::invalid(value.span, "once slot is not Maybe"));
        };
        let ir::DeclarationKind::Variant { cases, .. } = &self.b.module().declarations.get(declaration)?.kind else {
            unreachable!("Maybe layout");
        };
        let payload = ir::FieldRef { declaration: cases[0].layout, index: 0 };
        let kept = self.core(ir::CoreOp::VariantGet { value: slot, case: 0, field: payload })?;
        self.branch_join(&join, Some(kept), ir::ExitKind::Fallthrough)?;
        self.env = env;
        self.switch(compute)?;
        if let Some(computed) = self.value(value)? {
            let wrapped = self.core(ir::CoreOp::VariantNew {
                ty: layout[index].ty, case: 0, fields: vec![computed], packet: None,
            })?;
            self.b.emit_schema(ir::Operation::Core(ir::CoreOp::OnceStore { object, field, value: wrapped }))?;
            self.branch_join(&join, Some(computed), ir::ExitKind::Fallthrough)?;
        }
        let result = self.finish_join(&join)?.expect("once value");
        self.bind(local, result)
    }
}

/// Reject known resource graphs here as well as in the public frontend check.
/// This is a producer gate; it does not issue the #565 ownership certificate.
///
/// Bible §13.13 (#491, #520): a once value must not retain a resource. A function or role value is allowed
/// only when it cannot retain one in this program: every closure of that function type captures plain
/// values, and every implementation of that role is plain. A native handle has no Okkama destroy (P2.4
/// §4.7), so it is plain; an object with `destroy`, a builtin capability, a cap, `any` or a Future is not.
pub(super) fn check(module: &ir::Module) -> Result<()> {
    let functions = Functions::settle(module)?;
    for (_, declaration) in module.declarations.iter() {
        if let ir::DeclarationKind::Object { fields, .. } = &declaration.kind {
            for field in fields.iter().filter(|field| field.once) {
                let mut seen = std::collections::HashSet::new();
                if !plain(module, &functions, field.ty, &mut seen)? {
                    return Err(LowerError::invalid(Default::default(),
                        "once let cannot keep a resource: its value would outlive the scope that releases it"));
                }
            }
        }
    }
    Ok(())
}

/// The function types whose closures cannot retain a resource in this module. A closure retains what it
/// captures: a bound action its receiver, a lambda its captures. A function type is plain until one of its
/// closures captures a value that is not plain; that can only turn types from plain to not plain, so the
/// table settles after at most one pass per function type.
struct Functions {
    not_plain: std::collections::HashSet<ir::TypeId>,
}

impl Functions {
    fn settle(module: &ir::Module) -> Result<Self> {
        let mut origins: Vec<(ir::TypeId, Vec<ir::TypeId>)> = vec![];
        for (_, function) in module.functions.iter() {
            let Some(body) = &function.body else {
                continue;
            };
            for (_, instruction) in body.insts.iter() {
                let (ty, captured) = match &instruction.op {
                    ir::Operation::Core(ir::CoreOp::FnClosure { captures, ty, .. }) => (*ty, captures.clone()),
                    ir::Operation::Core(ir::CoreOp::FnBind { receiver, ty, .. }
                        | ir::CoreOp::FnBindRole { receiver, ty, .. }) => (*ty, vec![*receiver]),
                    _ => continue,
                };
                let types = captured.iter().map(|value| Ok(body.values.get(*value)?.ty)).collect::<Result<_>>()?;
                origins.push((ty, types));
            }
        }
        let mut functions = Self { not_plain: std::collections::HashSet::new() };
        loop {
            let mut changed = false;
            for (ty, captures) in &origins {
                if functions.not_plain.contains(ty) {
                    continue;
                }
                let mut plain_captures = true;
                for capture in captures {
                    let mut seen = std::collections::HashSet::new();
                    plain_captures &= plain(module, &functions, *capture, &mut seen)?;
                }
                if !plain_captures {
                    functions.not_plain.insert(*ty);
                    changed = true;
                }
            }
            if !changed {
                return Ok(functions);
            }
        }
    }
}

fn plain(module: &ir::Module, functions: &Functions, ty: ir::TypeId, seen: &mut std::collections::HashSet<ir::TypeId>)
    -> Result<bool>
{
    if !seen.insert(ty) {
        return Ok(true);
    }
    Ok(match *module.types.get(ty)? {
        ir::Type::Int | ir::Type::Bool | ir::Type::Byte | ir::Type::Float | ir::Type::Rat
        | ir::Type::Char | ir::Type::Str | ir::Type::Range | ir::Type::Never | ir::Type::Enum(_)
        | ir::Type::Handle(_) => true,
        ir::Type::Fn(_) => !functions.not_plain.contains(&ty),
        ir::Type::Array(inner) | ir::Type::Successor(inner) => plain(module, functions, inner, seen)?,
        ir::Type::Map(key, value) => plain(module, functions, key, seen)? && plain(module, functions, value, seen)?,
        ir::Type::Object { decl, .. } => {
            let declaration = module.declarations.get(decl)?;
            let ir::DeclarationKind::Object { fields, destroy, builtin, .. } = &declaration.kind else {
                return Err(LowerError::invalid(Default::default(), "once object has no layout"));
            };
            let mut result = destroy.is_none() && !*builtin;
            for field in fields {
                result &= plain(module, functions, field.ty, seen)?;
            }
            result
        }
        ir::Type::Variant(decl) => {
            let ir::DeclarationKind::Variant { cases, .. } = &module.declarations.get(decl)?.kind else {
                return Err(LowerError::invalid(Default::default(), "once variant has no layout"));
            };
            let mut result = true;
            for case in cases {
                let ir::DeclarationKind::Object { fields, .. } = &module.declarations.get(case.layout)?.kind else {
                    return Err(LowerError::invalid(Default::default(), "once case has no layout"));
                };
                for field in fields {
                    result &= plain(module, functions, field.ty, seen)?;
                }
            }
            result
        }
        ir::Type::Role { .. } => {
            let mut result = true;
            for (_, declaration) in module.declarations.iter() {
                if let ir::DeclarationKind::Object { witnesses, .. } = &declaration.kind
                    && witnesses.iter().any(|witness| witness.role == ty) {
                    let ir::DeclarationKind::Object { fields, destroy, builtin, .. } = &declaration.kind else {
                        unreachable!("witness owner");
                    };
                    result &= destroy.is_none() && !*builtin;
                    for field in fields {
                        result &= plain(module, functions, field.ty, seen)?;
                    }
                }
            }
            result
        }
        _ => false,
    })
}
