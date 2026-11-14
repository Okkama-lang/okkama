//! C4 collection construction and projections. Contracts name retention; high
//! never computes packets/deps. Derived operations use ordinary scalar CFG.
use super::body::BodyLowerer;
use super::{LowerError, Result};
use crate::analysis::{thir, ty::Ty};
use crate::oir2 as ir;

macro_rules! value {
    ($this:expr, $expr:expr) => {
        match $this.value($expr)? {
            Some(value) => $this.take(value)?,
            None => return Ok(None),
        }
    };
}

pub(super) fn is_builtin(op: &thir::Builtin) -> bool {
    use thir::Builtin as B;
    matches!(op, B::ArraySize | B::ArraySlice | B::ArrayJoin | B::ArrayItem
        | B::ArrayFirst | B::ArrayLast | B::ArrayContains(_) | B::ArrayIndexOf(_)
        | B::ArrayReversed | B::ArrayTake | B::ArrayDrop | B::ArrayReplaced
        | B::ArrayRemovedAt | B::MapSize | B::MapHas | B::MapKeys | B::MapValues)
}

impl BodyLowerer<'_, '_> {
    pub(super) fn int(&mut self, value: i64) -> Result<ir::ValueId> {
        self.constant(ir::Literal::Int(value.to_string()))
    }

    fn empty_array(&mut self, ty: ir::TypeId) -> Result<ir::ValueId> {
        self.collection(ir::CollectionOp::ArrayNew { values: vec![], ty, packet: None })
    }

    fn singleton(&mut self, value: ir::ValueId, ty: ir::TypeId) -> Result<ir::ValueId> {
        self.collection(ir::CollectionOp::ArrayNew { values: vec![value], ty, packet: None })
    }

    fn append_array(&mut self, left: ir::ValueId, right: ir::ValueId, ty: ir::TypeId) -> Result<ir::ValueId> {
        self.collection(ir::CollectionOp::ArrayConcat { arrays: vec![left, right], ty, packet: None })
    }

    pub(super) fn collection_expr(&mut self, expr: &thir::Expr) -> Result<Option<ir::ValueId>> {
        use thir::ExprKind as E;
        let ty = self.ty(expr.ty.as_ref().expect("typed collection expression"), expr.span)?;
        let result = match &expr.kind {
            E::Array(parts) if parts.len() >= super::builders::STREAM_LIMIT
                || parts.iter().any(|part| matches!(part, thir::ArrayPart::Range { .. })) => {
                return self.stream_array(parts, ty);
            }
            E::Map(entries) if entries.len() >= super::builders::STREAM_LIMIT => {
                return self.stream_map(entries, ty);
            }
            E::Array(parts) => {
                let mut values = vec![];
                for part in parts {
                    let thir::ArrayPart::Value(expr) = part else {
                        unreachable!("range-expanded literals use a builder");
                    };
                    values.push(value!(self, expr));
                }
                self.collection(ir::CollectionOp::ArrayNew { values, ty, packet: None })?
            }
            E::Map(entries) => {
                let mut values = vec![];
                for (key, value) in entries {
                    let key = value!(self, key);
                    let value = value!(self, value);
                    values.push((key, value));
                }
                self.collection(ir::CollectionOp::MapNew { entries: values, ty, packet: None })?
            }
            E::Coerce { coercion: thir::Coercion::Retag, value } => {
                let value = value!(self, value);
                let op = match self.b.module().types.get(ty)? {
                    ir::Type::Array(_) => ir::CollectionOp::ArrayRetag { array: value, ty },
                    ir::Type::Map(..) => ir::CollectionOp::MapRetag { map: value, ty },
                    _ => return Err(LowerError::invalid(expr.span, "retag without collection type")),
                };
                self.collection(op)?
            }
            E::Binary { op, left, right } => {
                let mut parts = vec![];
                if matches!(op, thir::BinaryOp::ArrayConcat | thir::BinaryOp::MapMerge) {
                    flatten(*op, left, &mut parts);
                    flatten(*op, right, &mut parts);
                } else {
                    // Only the left difference chain is associative.
                    let mut base = &**left;
                    let mut removed = vec![&**right];
                    while let E::Binary { op: inner, left, right } = &base.kind {
                        if inner != op {
                            break;
                        }
                        removed.push(&**right);
                        base = left;
                    }
                    removed.reverse();
                    parts.push(base);
                    parts.extend(removed);
                }
                if parts.len() >= super::builders::STREAM_LIMIT
                    && matches!(op, thir::BinaryOp::ArrayConcat | thir::BinaryOp::MapMerge) {
                    return self.stream_collections(&parts, ty, *op == thir::BinaryOp::MapMerge);
                }
                let mut values = vec![];
                for part in parts {
                    let value = value!(self, part);
                    values.push(value);
                }
                // The frontend permits untagged empties alongside concrete
                // operands. The OIR primitive contract requires one exact type.
                for value in &mut values {
                    if self.b.body().values.get(*value)?.ty != ty {
                        let op = match self.b.module().types.get(ty)? {
                            ir::Type::Array(_) => ir::CollectionOp::ArrayRetag { array: *value, ty },
                            ir::Type::Map(..) => ir::CollectionOp::MapRetag { map: *value, ty },
                            _ => unreachable!("collection operator type"),
                        };
                        *value = self.collection(op)?;
                    }
                }
                let operation = match op {
                    thir::BinaryOp::ArrayConcat => {
                        ir::CollectionOp::ArrayConcat { arrays: values, ty, packet: None }
                    }
                    thir::BinaryOp::MapMerge => {
                        ir::CollectionOp::MapMerge { maps: values, ty, packet: None }
                    }
                    thir::BinaryOp::ArrayDifference => {
                        ir::CollectionOp::ArrayDifference { array: values[0],
                            removed: values[1..].to_vec(), packet: None }
                    }
                    thir::BinaryOp::MapDifference => {
                        ir::CollectionOp::MapDifference { map: values[0],
                            removed: values[1..].to_vec(), packet: None }
                    }
                    _ => unreachable!("collection binary dispatch"),
                };
                self.collection(operation)?
            }
            E::Index { object, index, fallback } => {
                // Positional projection of a map side, without retaining the
                // other side. Receiver still precedes the index expression.
                if fallback.is_none()
                    && let E::Call { callee: thir::Callee::Builtin { receiver, op }, .. } = &object.kind
                    && matches!(op, thir::Builtin::MapKeys | thir::Builtin::MapValues) {
                    let map = value!(self, receiver);
                    let index = value!(self, index);
                    let op = if *op == thir::Builtin::MapKeys {
                        ir::CollectionOp::MapKeyAt { map, index }
                    } else {
                        ir::CollectionOp::MapValueAt { map, index }
                    };
                    return self.collection(op).map(Some);
                }
                let collection = value!(self, object);
                let key = value!(self, index);
                let map = matches!(object.ty, Some(Ty::Map(..)));
                if let Some(fallback) = fallback {
                    // Untyped empty collections cannot produce a typed found value.
                    let empty = matches!(&object.ty, Some(Ty::Array(t)) if **t == Ty::Never)
                        || matches!(&object.ty, Some(Ty::Map(k, _)) if **k == Ty::Never);
                    if empty {
                        return self.value(fallback)?.map(|value| self.take(value)).transpose();
                    }
                    return self.lookup_fallback(collection, key, map, ty, fallback);
                }
                self.collection(if map {
                    ir::CollectionOp::MapGet { map: collection, key }
                } else {
                    ir::CollectionOp::ArrayGet { array: collection, index: key }
                })?
            }
            E::ArrayCast { element, parts } => {
                // Explicit array casts evaluate operands first, then conversions.
                let mut operands = vec![];
                for part in parts {
                    let (expr, spread) = match part {
                        thir::CastPart::One(expr) => {
                            let inner = match &expr.kind {
                                E::Cast { value, .. } => &**value,
                                _ => expr,
                            };
                            (inner, false)
                        }
                        thir::CastPart::Spread(expr) => (expr, true),
                    };
                    operands.push((value!(self, expr), spread));
                }
                let mut array = self.empty_array(ty)?;
                for (value, spread) in operands {
                    let part = if spread {
                        let len = self.collection(ir::CollectionOp::ArrayLen { array: value })?;
                        let empty = self.empty_array(ty)?;
                        self.array_shape(empty, ty, len, Shape::Cast(value, *element, expr.span))?
                    } else {
                        let value = self.cast_scalar(value, *element, expr.span)?;
                        self.singleton(value, ty)?
                    };
                    array = self.append_array(array, part, ty)?;
                }
                array
            }
            E::Call { callee: thir::Callee::Builtin { receiver, op }, args } => {
                let array = value!(self, receiver);
                let mut values = vec![];
                for arg in args {
                    values.push(value!(self, arg));
                }
                self.collection_builtin(array, op, &values, ty, expr.span)?
            }
            _ => unreachable!("collection dispatch"),
        };
        Ok(Some(result))
    }

    fn lookup_edge(&mut self, collection: ir::ValueId, key: ir::ValueId, map: bool,
        found: ir::BlockId, missing: ir::BlockId, args: Vec<ir::ValueId>) -> Result<()> {
        let mut found_args = vec![ir::EdgeArg::Produced(ir::EdgeResult {
            outcome: ir::Outcome::Normal, slot: 0,
        })];
        found_args.extend(args.iter().copied().map(ir::EdgeArg::Value));
        self.terminate(ir::TermKind::Lookup {
            kind: if map { ir::LookupKind::Map } else { ir::LookupKind::Array }, collection, key,
            found: ir::InvokeEdge { block: found, args: found_args },
            missing: ir::Target { block: missing, args },
        })
    }

    fn lookup_fallback(&mut self, collection: ir::ValueId, key: ir::ValueId, map: bool,
        ty: ir::TypeId, fallback: &thir::Expr) -> Result<Option<ir::ValueId>> {
        let scope = self.scope();
        let join = self.join(scope, Some(ty))?;
        let locals = self.locals();
        let types = self.local_types(&locals);
        let found_scope = self.b.declare_scope(scope, ir::ScopeKind::Branch)?;
        let missing_scope = self.b.declare_scope(scope, ir::ScopeKind::Branch)?;
        let mut found_types = vec![ty];
        found_types.extend(&types);
        let found = self.new_block(found_scope, &found_types)?;
        let missing = self.new_block(missing_scope, &types)?;
        let args = self.args(&locals);
        self.lookup_edge(collection, key, map, found, missing, args)?;
        self.enter(found, &locals, 1)?;
        let value = self.b.block_params(found)?[0];
        self.branch_join(&join, Some(value), ir::ExitKind::Fallthrough)?;
        self.enter(missing, &locals, 0)?;
        if let Some(value) = self.value(fallback)? {
            let value = self.take(value)?;
            self.branch_join(&join, Some(value), ir::ExitKind::Fallthrough)?;
        }
        self.finish_join(&join)
    }

    fn collection_builtin(&mut self, array: ir::ValueId, op: &thir::Builtin, args: &[ir::ValueId],
        ty: ir::TypeId, span: crate::source::Span) -> Result<ir::ValueId> {
        use ir::CollectionOp as C;
        use thir::Builtin as B;
        let operation = match op {
            B::ArraySize => C::ArrayLen { array },
            B::MapSize => C::MapLen { map: array },
            B::ArrayItem => C::ArrayGet { array, index: args[0] },
            B::ArraySlice => C::ArraySlice { array, start: args[0], end: args[1], packet: None },
            B::ArrayJoin => C::ArrayJoin { array, separator: args[0] },
            B::ArrayReplaced => C::ArrayReplace { array, index: args[0], value: args[1], packet: None },
            B::ArrayRemovedAt => C::ArrayRemove { array, index: args[0], packet: None },
            B::MapHas => C::MapHas { map: array, key: args[0] },
            B::MapKeys => C::MapKeys { map: array },
            B::MapValues => C::MapValues { map: array },
            B::ArrayFirst | B::ArrayLast => {
                let index = if *op == B::ArrayFirst {
                    self.int(0)?
                } else {
                    let len = self.collection(C::ArrayLen { array })?;
                    let one = self.int(1)?;
                    self.core(ir::CoreOp::IntSub { left: len, right: one })?
                };
                let scope = self.scope();
                let found = self.new_block(scope, &[ty])?;
                let missing = self.new_block(scope, &[])?;
                self.lookup_edge(array, index, false, found, missing, vec![])?;
                let env = self.env.clone();
                self.switch(missing)?;
                let name = if *op == B::ArrayFirst { "first" } else { "last" };
                self.terminate(ir::TermKind::Trap { code: ir::TrapCode::Empty,
                    message: format!("Cannot read {name} item of an empty sequence") })?;
                self.env = env;
                self.switch(found)?;
                return Ok(self.b.block_params(found)?[0]);
            }
            B::ArrayTake | B::ArrayDrop => return self.array_cut(array, args[0], *op == B::ArrayDrop),
            B::ArrayReversed => {
                let len = self.collection(C::ArrayLen { array })?;
                let empty = self.empty_array(ty)?;
                return self.array_shape(empty, ty, len, Shape::Reverse(array));
            }
            B::ArrayContains(kind) | B::ArrayIndexOf(kind) => {
                let index = self.array_search(array, args[0], *kind, span)?;
                let zero = self.int(0)?;
                let found = self.core(ir::CoreOp::IntGe { left: index, right: zero })?;
                if matches!(op, B::ArrayContains(_)) {
                    return Ok(found);
                }
                let scope = self.scope();
                let yes = self.new_block(scope, &[])?;
                let no = self.new_block(scope, &[])?;
                let join = self.join(scope, Some(ty))?;
                self.terminate(ir::TermKind::Cond { condition: found,
                    yes: ir::Target { block: yes, args: vec![] }, no: ir::Target { block: no, args: vec![] } })?;
                let env = self.env.clone();
                for (block, case, fields) in [(yes, 0, vec![index]), (no, 1, vec![])] {
                    self.env = env.clone();
                    self.switch(block)?;
                    let value = self.core(ir::CoreOp::VariantNew { ty, case, fields, packet: None })?;
                    self.branch_join(&join, Some(value), ir::ExitKind::Fallthrough)?;
                }
                return Ok(self.finish_join(&join)?.expect("Maybe result"));
            }
            _ => unreachable!("collection builtin allowlist"),
        };
        self.collection(operation)
    }

    fn array_cut(&mut self, array: ir::ValueId, count: ir::ValueId, drop: bool) -> Result<ir::ValueId> {
        let scope = self.scope();
        let zero = self.int(0)?;
        let negative = self.core(ir::CoreOp::IntLt { left: count, right: zero })?;
        let fail = self.new_block(scope, &[])?;
        let check = self.new_block(scope, &[])?;
        self.terminate(ir::TermKind::Cond { condition: negative,
            yes: ir::Target { block: fail, args: vec![] }, no: ir::Target { block: check, args: vec![] } })?;
        let env = self.env.clone();
        self.switch(fail)?;
        let name = if drop { "drop" } else { "take" };
        self.terminate(ir::TermKind::Trap { code: ir::TrapCode::Range,
            message: format!("{name} needs a non-negative count") })?;
        self.env = env;
        self.switch(check)?;
        let len = self.collection(ir::CollectionOp::ArrayLen { array })?;
        let short = self.core(ir::CoreOp::IntLt { left: count, right: len })?;
        let int_ty = self.b.body().values.get(count)?.ty;
        let cut = self.new_block(scope, &[int_ty])?;
        self.terminate(ir::TermKind::Cond { condition: short,
            yes: ir::Target { block: cut, args: vec![count] }, no: ir::Target { block: cut, args: vec![len] } })?;
        self.switch(cut)?;
        let cut = self.b.block_params(cut)?[0];
        let (start, end) = if drop { (cut, len) } else { (zero, cut) };
        self.collection(ir::CollectionOp::ArraySlice { array, start, end, packet: None })
    }

    fn array_search(&mut self, array: ir::ValueId, needle: ir::ValueId, kind: thir::CompareKind,
        span: crate::source::Span) -> Result<ir::ValueId> {
        let missing = self.int(-1)?;
        let array_ty = self.b.body().values.get(array)?.ty;
        if let ir::Type::Array(element) = self.b.module().types.get(array_ty)?
            && matches!(self.b.module().types.get(*element)?, ir::Type::Never) {
            return Ok(missing);
        }
        let scope = self.scope();
        let zero = self.int(0)?;
        let int_ty = self.b.body().values.get(zero)?.ty;
        let head = self.new_block(scope, &[int_ty])?;
        let body = self.new_block(scope, &[])?;
        let next = self.new_block(scope, &[])?;
        let done = self.new_block(scope, &[int_ty])?;
        let len = self.collection(ir::CollectionOp::ArrayLen { array })?;
        self.terminate(ir::TermKind::Br(ir::Target { block: head, args: vec![zero] }))?;
        self.switch(head)?;
        let index = self.b.block_params(head)?[0];
        let more = self.core(ir::CoreOp::IntLt { left: index, right: len })?;
        self.terminate(ir::TermKind::Cond { condition: more,
            yes: ir::Target { block: body, args: vec![] }, no: ir::Target { block: done, args: vec![missing] } })?;
        let head_env = self.env.clone();
        self.switch(body)?;
        let item = self.collection(ir::CollectionOp::ArrayGet { array, index })?;
        let equal = self.core(super::scalar::compare(kind, thir::CompareOp::Eq, item, needle, span)?)?;
        self.terminate(ir::TermKind::Cond { condition: equal,
            yes: ir::Target { block: done, args: vec![index] }, no: ir::Target { block: next, args: vec![] } })?;
        self.switch(next)?;
        let one = self.int(1)?;
        let following = self.core(ir::CoreOp::IntAdd { left: index, right: one })?;
        self.terminate(ir::TermKind::Br(ir::Target { block: head, args: vec![following] }))?;
        self.env = head_env;
        self.switch(done)?;
        Ok(self.b.block_params(done)?[0])
    }

    // Persistent reversed/cast shapes retain the C4 scalar traversal.
    fn array_shape(&mut self, initial: ir::ValueId, ty: ir::TypeId, len: ir::ValueId,
        shape: Shape) -> Result<ir::ValueId> {
        let (Shape::Reverse(array) | Shape::Cast(array, _, _)) = shape;
        let array_ty = self.b.body().values.get(array)?.ty;
        if let ir::Type::Array(element) = self.b.module().types.get(array_ty)?
            && matches!(self.b.module().types.get(*element)?, ir::Type::Never) {
            return Ok(initial);
        }
        let scope = self.scope();
        let zero = self.int(0)?;
        let int_ty = self.b.body().values.get(zero)?.ty;
        let head = self.new_block(scope, &[int_ty, ty])?;
        let body = self.new_block(scope, &[])?;
        let done = self.new_block(scope, &[])?;
        self.terminate(ir::TermKind::Br(ir::Target { block: head, args: vec![zero, initial] }))?;
        self.switch(head)?;
        let index = self.b.block_params(head)?[0];
        let partial = self.b.block_params(head)?[1];
        let more = self.core(ir::CoreOp::IntLt { left: index, right: len })?;
        self.terminate(ir::TermKind::Cond { condition: more,
            yes: ir::Target { block: body, args: vec![] }, no: ir::Target { block: done, args: vec![] } })?;
        let head_env = self.env.clone();
        self.switch(body)?;
        let one = self.int(1)?;
        let item = match shape {
            Shape::Reverse(array) => {
                let remaining = self.core(ir::CoreOp::IntSub { left: len, right: index })?;
                let index = self.core(ir::CoreOp::IntSub { left: remaining, right: one })?;
                self.collection(ir::CollectionOp::ArrayGet { array, index })?
            }
            Shape::Cast(array, target, span) => {
                let value = self.collection(ir::CollectionOp::ArrayGet { array, index })?;
                self.cast_scalar(value, target, span)?
            }
        };
        let part = self.singleton(item, ty)?;
        let result = self.append_array(partial, part, ty)?;
        let next = self.core(ir::CoreOp::IntAdd { left: index, right: one })?;
        self.terminate(ir::TermKind::Br(ir::Target { block: head, args: vec![next, result] }))?;
        self.env = head_env;
        self.switch(done)?;
        Ok(partial)
    }
}

#[derive(Clone, Copy)]
enum Shape {
    Reverse(ir::ValueId),
    Cast(ir::ValueId, thir::CastTarget, crate::source::Span),
}

fn flatten<'e>(op: thir::BinaryOp, expr: &'e thir::Expr, parts: &mut Vec<&'e thir::Expr>) {
    match &expr.kind {
        thir::ExprKind::Binary { op: inner, left, right } if *inner == op => {
            flatten(op, left, parts);
            flatten(op, right, parts);
        }
        _ => parts.push(expr),
    }
}
