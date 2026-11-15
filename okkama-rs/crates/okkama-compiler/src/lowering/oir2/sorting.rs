//! Stable sorting (`ordering()`) is one runtime operation, as in Tomato (#678); scalar order helpers.
use super::body::BodyLowerer;
use super::{LowerError, Result};
use crate::analysis::thir;
use crate::oir2 as ir;
use crate::source::Span;

pub(super) fn scalar_kind(ty: &ir::Type) -> Option<thir::CompareKind> {
    use thir::CompareKind as K;
    Some(match ty {
        ir::Type::Int => K::Int,
        ir::Type::Byte => K::Byte,
        ir::Type::Float => K::Float,
        ir::Type::Rat => K::Rat,
        ir::Type::Char => K::Char,
        ir::Type::Str => K::Str,
        _ => return None,
    })
}

impl BodyLowerer<'_, '_> {
    pub(super) fn scalar_order(&mut self, kind: thir::CompareKind, left: ir::ValueId,
        right: ir::ValueId, ty: ir::TypeId, span: Span) -> Result<ir::ValueId> {
        let ir::Type::Enum(declaration) = *self.b.module().types.get(ty)? else {
            return Err(LowerError::invalid(span, "order result is not compare.Order"));
        };
        let join = self.join(self.scope(), Some(ty))?;
        let env = self.env.clone();
        for (case, op) in [(0, thir::CompareOp::Lt), (1, thir::CompareOp::Eq)] {
            let condition = self.core(super::scalar::compare(kind, op, left, right, span)?)?;
            let hit = self.new_block(self.scope(), &[])?;
            let rest = self.new_block(self.scope(), &[])?;
            self.terminate(ir::TermKind::Cond {
                condition,
                yes: ir::Target { block: hit, args: vec![] },
                no: ir::Target { block: rest, args: vec![] },
            })?;
            self.switch(hit)?;
            let value = self.core(ir::CoreOp::EnumCase { declaration, case })?;
            self.branch_join(&join, Some(value), ir::ExitKind::Fallthrough)?;
            self.env = env.clone();
            self.switch(rest)?;
        }
        let value = self.core(ir::CoreOp::EnumCase { declaration, case: 2 })?;
        self.branch_join(&join, Some(value), ir::ExitKind::Fallthrough)?;
        Ok(self.finish_join(&join)?.expect("Order result"))
    }

    /// The role and slot of `order` for elements of type `element`: the `Comparing<T>` witness of an object,
    /// or the role itself for role values; the slot takes one more element and returns `compare.Order`.
    fn order_slot(&self, element: ir::TypeId, span: Span) -> Result<(ir::DeclId, u32)> {
        let module = self.b.module();
        let slot = |role: ir::DeclId| -> Option<u32> {
            let ir::DeclarationKind::Role { methods, .. } = &module.declarations.get(role).ok()?.kind else {
                return None;
            };
            let slot = methods.iter().position(|method| {
                method.name == "order" && method.signature.params == [element]
                    && matches!(method.signature.results.as_slice(),
                        [result] if matches!(module.types.get(*result), Ok(ir::Type::Enum(_))))
            })?;
            u32::try_from(slot).ok()
        };
        let found = match *module.types.get(element)? {
            ir::Type::Object { decl, .. } => {
                let ir::DeclarationKind::Object { witnesses, .. } = &module.declarations.get(decl)?.kind else {
                    return Err(LowerError::invalid(span, "ordering element object without a declaration"));
                };
                witnesses.iter().find_map(|witness| match module.types.get(witness.role).ok()? {
                    ir::Type::Role { decl: role, .. } => slot(*role).map(|slot| (*role, slot)),
                    _ => None,
                })
            }
            ir::Type::Role { decl, .. } => slot(decl).map(|slot| (decl, slot)),
            _ => None,
        };
        found.ok_or_else(|| LowerError::unsupported(span, "ordering element without a Comparing order"))
    }

    /// `array.ordering()`, as Tomato's `ordering` intrinsic (#678): one operation. Scalars and text sort
    /// in their natural order (`array.order`, total); other elements are merged by their `order` method,
    /// called through role dispatch once per comparison (`call.order`, failing with its failures).
    pub(super) fn array_ordering(&mut self, array: ir::ValueId, span: Span) -> Result<ir::ValueId> {
        let ty = self.b.body().values.get(array)?.ty;
        let ir::Type::Array(element) = *self.b.module().types.get(ty)? else {
            return Err(LowerError::invalid(span, "ordering receiver is not an array"));
        };
        if scalar_kind(self.b.module().types.get(element)?).is_some() {
            return self.collection(ir::CollectionOp::ArrayOrder { array });
        }
        let (role, slot) = self.order_slot(element, span)?;
        let borrowed = ir::CallArgument { value: array, convention: ir::OperandConvention::Borrow };
        self.sync_failure()?;
        let values = self.b.emit_schema(ir::Operation::Call(ir::CallOp::Order {
            array: borrowed, role, slot, call: super::roles::call_site(vec![borrowed]),
        }))?;
        super::roles::record_call_effect(&mut self.b, self.current.expect("open ordering"))?;
        values.first().copied().ok_or_else(|| LowerError::invalid(span, "ordering without a result"))
    }
}
