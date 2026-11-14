//! Stable sorting is scalar CFG with an ordinary, failing order call per comparison.
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

    fn greater(&mut self, left: ir::ValueId, right: ir::ValueId, span: Span) -> Result<ir::ValueId> {
        let ty = self.b.body().values.get(left)?.ty;
        if let Some(kind) = scalar_kind(self.b.module().types.get(ty)?) {
            return self.core(super::scalar::compare(kind, thir::CompareOp::Gt, left, right, span)?);
        }
        let ir::Type::Object { decl, .. } = *self.b.module().types.get(ty)? else {
            return Err(LowerError::unsupported(span, "ordering element without a closed Comparing target"));
        };
        let ir::DeclarationKind::Object { witnesses, .. } = &self.b.module().declarations.get(decl)?.kind else {
            unreachable!("ordering object");
        };
        let target = witnesses.iter().find_map(|witness| {
            let ir::Type::Role { decl, .. } = self.b.module().types.get(witness.role).ok()? else {
                return None;
            };
            let ir::DeclarationKind::Role { methods, .. } = &self.b.module().declarations.get(*decl).ok()?.kind else {
                return None;
            };
            let slot = methods.iter().position(|method| {
                method.name == "order" && method.signature.params == [ty]
            })?;
            witness.slots.get(slot).copied()
        }).ok_or_else(|| LowerError::unsupported(span, "ordering has no specialized Comparing witness"))?;
        let result = self.call(target, vec![left, right])?.expect("order result");
        let order = self.b.body().values.get(result)?.ty;
        let ir::Type::Enum(declaration) = *self.b.module().types.get(order)? else {
            return Err(LowerError::invalid(span, "order action has no Order result"));
        };
        let greater = self.core(ir::CoreOp::EnumCase { declaration, case: 2 })?;
        self.core(ir::CoreOp::EnumEq { left: result, right: greater })
    }

    pub(super) fn array_ordering(&mut self, array: ir::ValueId, span: Span) -> Result<ir::ValueId> {
        let base = self.env.len();
        let one = self.int(1)?;
        let zero = self.int(0)?;
        let size = self.collection(ir::CollectionOp::ArrayLen { array })?;
        let ty = self.b.body().values.get(array)?.ty;
        self.env.extend([Some(array), Some(one), Some(one)]);
        let header = self.join(self.scope(), None)?;
        let exit = self.join(self.scope(), Some(ty))?;
        self.branch_join(&header, None, ir::ExitKind::Fallthrough)?;
        self.finish_join(&header)?;
        let condition = self.core(ir::CoreOp::IntLt { left: self.env[base + 1].unwrap(), right: size })?;
        let iteration = self.b.declare_scope(self.scope(), ir::ScopeKind::Iteration)?;
        let start = self.new_block(iteration, &[])?;
        let exhausted = self.new_block(self.scope(), &[])?;
        self.terminate(ir::TermKind::Cond {
            condition,
            yes: ir::Target { block: start, args: vec![] },
            no: ir::Target { block: exhausted, args: vec![] },
        })?;
        let outer = self.env.clone();
        self.switch(exhausted)?;
        self.branch_join(&exit, self.env[base], ir::ExitKind::Fallthrough)?;
        self.env = outer;
        self.switch(start)?;
        self.env[base + 2] = self.env[base + 1];
        let inner = self.join(iteration, None)?;
        let advance = self.join(iteration, None)?;
        self.branch_join(&inner, None, ir::ExitKind::Fallthrough)?;
        self.finish_join(&inner)?;
        let array = self.env[base].unwrap();
        let j = self.env[base + 2].unwrap();
        let condition = self.core(ir::CoreOp::IntGt { left: j, right: zero })?;
        let compare = self.new_block(iteration, &[])?;
        let done = self.new_block(iteration, &[])?;
        self.terminate(ir::TermKind::Cond {
            condition,
            yes: ir::Target { block: compare, args: vec![] },
            no: ir::Target { block: done, args: vec![] },
        })?;
        let env = self.env.clone();
        self.switch(done)?;
        self.branch_join(&advance, None, ir::ExitKind::Fallthrough)?;
        self.env = env.clone();
        self.switch(compare)?;
        let previous = self.core(ir::CoreOp::IntSub { left: j, right: one })?;
        let left = self.collection(ir::CollectionOp::ArrayGet { array, index: previous })?;
        let right = self.collection(ir::CollectionOp::ArrayGet { array, index: j })?;
        let condition = self.greater(left, right, span)?;
        let swap = self.new_block(iteration, &[])?;
        let sorted = self.new_block(iteration, &[])?;
        self.terminate(ir::TermKind::Cond {
            condition,
            yes: ir::Target { block: swap, args: vec![] },
            no: ir::Target { block: sorted, args: vec![] },
        })?;
        self.switch(sorted)?;
        self.branch_join(&advance, None, ir::ExitKind::Fallthrough)?;
        self.env = env;
        self.switch(swap)?;
        let array = self.collection(ir::CollectionOp::ArrayReplace {
            array, index: previous, value: right, packet: None,
        })?;
        let array = self.collection(ir::CollectionOp::ArrayReplace { array, index: j, value: left, packet: None })?;
        self.env[base] = Some(array);
        self.env[base + 2] = Some(previous);
        self.branch_join(&inner, None, ir::ExitKind::Next)?;
        self.finish_join(&advance)?;
        let i = self.env[base + 1].unwrap();
        self.env[base + 1] = Some(self.core(ir::CoreOp::IntAdd { left: i, right: one })?);
        self.branch_join(&header, None, ir::ExitKind::Next)?;
        let result = self.finish_join(&exit)?.expect("sorted array");
        self.env.truncate(base);
        Ok(result)
    }
}
