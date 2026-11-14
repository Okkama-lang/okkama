//! S1 instruction allowlist. Unsupported THIR variants have no OIR-1 fallback.
use super::body::BodyLowerer;
use super::{LowerError, Result};
use crate::analysis::thir::{self, ArithOp, BinaryOp, CastTarget, CompareKind, CompareOp, NumericKind, UnaryOp};
use crate::oir2 as ir;
use crate::source::Span;

macro_rules! value {
    ($this:expr, $expr:expr) => {
        match $this.value($expr)? {
            Some(value) => value,
            None => return Ok(None),
        }
    };
}

impl BodyLowerer<'_, '_> {
    pub(super) fn core(&mut self, op: ir::CoreOp) -> Result<ir::ValueId> {
        let operation = ir::Operation::Core(op);
        let contract = ir::ContractRegistry::new(self.b.module())
            .lookup(&operation).map_err(ir::BuildError::from)?;
        if !contract.effects.failures.is_empty() || contract.effects.suspends {
            self.sync_failure()?;
        }
        Ok(self.b.emit_schema(operation)?[0])
    }

    pub(super) fn collection(&mut self, op: ir::CollectionOp) -> Result<ir::ValueId> {
        let operation = ir::Operation::Collection(op);
        let contract = ir::ContractRegistry::new(self.b.module())
            .lookup(&operation).map_err(ir::BuildError::from)?;
        if !contract.effects.failures.is_empty() || contract.effects.suspends {
            self.sync_failure()?;
        }
        Ok(self.b.emit_schema(operation)?[0])
    }

    pub(super) fn constant(&mut self, value: ir::Literal) -> Result<ir::ValueId> {
        Ok(self.b.emit_schema(ir::Operation::Const(value))?[0])
    }

    pub(super) fn scalar(&mut self, expr: &thir::Expr) -> Result<Option<ir::ValueId>> {
        use thir::ExprKind as E;
        let result = match &expr.kind {
            E::Constant(constant) => self.constant(literal(constant))?,
            E::Global(_) => return Err(LowerError::unsupported(expr.span, "module constant reference")),
            E::Unary { op, operand } => {
                let value = value!(self, operand);
                let op = match op {
                    UnaryOp::Not => ir::CoreOp::BoolNot { value },
                    UnaryOp::Negate(NumericKind::Int) => ir::CoreOp::IntNeg { value },
                    UnaryOp::Negate(NumericKind::Float) => ir::CoreOp::FloatNeg { value },
                    UnaryOp::Negate(NumericKind::Rat) => ir::CoreOp::RatNeg { value },
                    _ => return Err(LowerError::unsupported(expr.span, format!("unary {op:?}"))),
                };
                self.core(op)?
            }
            E::Binary { op, left, right } => {
                let left = value!(self, left);
                let right = value!(self, right);
                let operation = match op {
                    BinaryOp::Arith(NumericKind::Int, ArithOp::Add) => ir::CoreOp::IntAdd { left, right },
                    BinaryOp::Arith(NumericKind::Int, ArithOp::Sub) => ir::CoreOp::IntSub { left, right },
                    BinaryOp::Arith(NumericKind::Int, ArithOp::Mul) => ir::CoreOp::IntMul { left, right },
                    BinaryOp::Arith(NumericKind::Int, ArithOp::Div) => ir::CoreOp::IntDiv { left, right },
                    BinaryOp::Arith(NumericKind::Int, ArithOp::Mod) => ir::CoreOp::IntMod { left, right },
                    BinaryOp::Concat => ir::CoreOp::StrConcat { left, right },
                    BinaryOp::Compare(kind, op) => compare(*kind, *op, left, right, expr.span)?,
                    _ => return Err(LowerError::unsupported(expr.span, format!("binary {op:?}"))),
                };
                self.core(operation)?
            }
            E::Cast { target, value, .. } => {
                let value = value!(self, value);
                self.cast_scalar(value, *target, expr.span)?
            }
            E::Call { callee: thir::Callee::Builtin { receiver, op }, args } => {
                let value = value!(self, receiver);
                match (op, args.as_slice()) {
                    (thir::Builtin::ArraySize, []) => {
                        self.collection(ir::CollectionOp::ArrayLen { array: value })?
                    }
                    (thir::Builtin::StrLength, []) => self.core(ir::CoreOp::StrLen { value })?,
                    (thir::Builtin::Convert(target), []) => {
                        self.cast_scalar(value, *target, expr.span)?
                    }
                    (thir::Builtin::Compare(kind, op), [other]) => {
                        let right = value!(self, other);
                        self.core(compare(*kind, *op, value, right, expr.span)?)?
                    }
                    (thir::Builtin::Order(kind), [other]) => {
                        let right = value!(self, other);
                        let ty = self.ty(expr.ty.as_ref().expect("Order type"), expr.span)?;
                        self.scalar_order(*kind, value, right, ty, expr.span)?
                    }
                    (thir::Builtin::StrCompare(op), [other]) => {
                        let right = value!(self, other);
                        self.core(compare(CompareKind::Str, *op, value, right, expr.span)?)?
                    }
                    _ => return Err(LowerError::unsupported(expr.span, format!("builtin {op:?}"))),
                }
            }
            E::RatNew { numerator, denominator } => {
                let numerator = value!(self, numerator);
                let denominator = value!(self, denominator);
                self.core(ir::CoreOp::RatNew { numerator, denominator })?
            }
            E::Range { start, end, inclusive } => {
                let start = value!(self, start);
                let end = value!(self, end);
                self.core(ir::CoreOp::RangeNew { start, end, inclusive: *inclusive })?
            }
            _ => return Err(LowerError::unsupported(expr.span, construct(&expr.kind))),
        };
        Ok(Some(result))
    }

    pub(super) fn cast_scalar(&mut self, value: ir::ValueId, target: CastTarget, span: Span)
        -> Result<ir::ValueId> {
        let to = match target {
            CastTarget::Int => ir::NumericType::Int,
            CastTarget::Byte => ir::NumericType::Byte,
            CastTarget::Float => ir::NumericType::Float,
            CastTarget::Rat => ir::NumericType::Rat,
            CastTarget::Str => ir::NumericType::Str,
            CastTarget::Char => {
                let ty = self.b.body().values.get(value)?.ty;
                if matches!(self.b.module().types.get(ty)?, ir::Type::Char) {
                    return Ok(value);
                }
                return Err(LowerError::unsupported(span, "char conversion"));
            }
        };
        self.convert(value, to, span)
    }

    fn convert(&mut self, value: ir::ValueId, to: ir::NumericType, span: Span) -> Result<ir::ValueId> {
        let ty = self.b.body().values.get(value)?.ty;
        let from = match self.b.module().types.get(ty)? {
            ir::Type::Int => ir::NumericType::Int,
            ir::Type::Bool => ir::NumericType::Bool,
            ir::Type::Byte => ir::NumericType::Byte,
            ir::Type::Float => ir::NumericType::Float,
            ir::Type::Rat => ir::NumericType::Rat,
            ir::Type::Str => ir::NumericType::Str,
            _ => return Err(LowerError::unsupported(span, "conversion from a non-S1 type")),
        };
        if from == to {
            return Ok(value);
        }
        if from == ir::NumericType::Str {
            return Err(LowerError::unsupported(span, "string-to-number conversion (#567)"));
        }
        self.core(ir::CoreOp::Convert { value, from, to })
    }

    pub(super) fn equal(&mut self, left: ir::ValueId, right: ir::ValueId, span: Span) -> Result<ir::ValueId> {
        let ty = self.b.body().values.get(left)?.ty;
        let kind = match self.b.module().types.get(ty)? {
            ir::Type::Int => CompareKind::Int,
            ir::Type::Bool => CompareKind::Bool,
            ir::Type::Str => CompareKind::Str,
            _ => return Err(LowerError::unsupported(span, "non-scalar match subject")),
        };
        self.core(compare(kind, CompareOp::Eq, left, right, span)?)
    }
}

fn literal(constant: &thir::Constant) -> ir::Literal {
    match constant {
        thir::Constant::Int(value) => ir::Literal::Int(value.clone()),
        thir::Constant::Bool(value) => ir::Literal::Bool(*value),
        thir::Constant::Str(value) => ir::Literal::Str(value.clone()),
        thir::Constant::Byte(value) => ir::Literal::Byte(*value),
        thir::Constant::Float(value) => ir::Literal::Float(*value),
        thir::Constant::Char(value) => ir::Literal::Char(*value),
    }
}

fn construct(expr: &thir::ExprKind) -> &'static str {
    use thir::ExprKind as E;
    match expr {
        E::Call { .. } => "direct, constructor or role call (S2)",
        E::SelfValue | E::Field(_) | E::PendingField(_) => "object/field access (S2)",
        E::Try(_) | E::Panic(_) => "try/panic (#567)",
        E::Coerce { .. } => "coercion/successor (S2)",
        E::Await(_) | E::Spawn(_) => "async operation",
        E::Closure { .. } | E::CallValue { .. } | E::BoundAction { .. } => "closure or action value",
        E::Array(_) | E::Map(_) | E::Index { .. } | E::Functional(_) => "collection operation",
        _ => "expression outside the S1 allowlist",
    }
}

pub(super) fn compare(
    kind: CompareKind,
    op: CompareOp,
    left: ir::ValueId,
    right: ir::ValueId,
    span: Span,
) -> Result<ir::CoreOp> {
    use ir::CoreOp as C;
    use CompareKind as K;
    use CompareOp as O;
    Ok(match (kind, op) {
        (K::Int, O::Eq) => C::IntEq { left, right },
        (K::Int, O::Ne) => C::IntNe { left, right },
        (K::Int, O::Lt) => C::IntLt { left, right },
        (K::Int, O::Le) => C::IntLe { left, right },
        (K::Int, O::Gt) => C::IntGt { left, right },
        (K::Int, O::Ge) => C::IntGe { left, right },
        (K::Bool, O::Eq) => C::BoolEq { left, right },
        (K::Bool, O::Ne) => C::BoolNe { left, right },
        (K::Str, O::Eq) => C::StrEq { left, right },
        (K::Str, O::Ne) => C::StrNe { left, right },
        (K::Str, O::Lt) => C::StrLt { left, right },
        (K::Str, O::Le) => C::StrLe { left, right },
        (K::Str, O::Gt) => C::StrGt { left, right },
        (K::Str, O::Ge) => C::StrGe { left, right },
        (K::Byte, O::Eq) => C::ByteEq { left, right },
        (K::Byte, O::Ne) => C::ByteNe { left, right },
        (K::Byte, O::Lt) => C::ByteLt { left, right },
        (K::Byte, O::Le) => C::ByteLe { left, right },
        (K::Byte, O::Gt) => C::ByteGt { left, right },
        (K::Byte, O::Ge) => C::ByteGe { left, right },
        (K::Float, O::Eq) => C::FloatEq { left, right },
        (K::Float, O::Ne) => C::FloatNe { left, right },
        (K::Float, O::Lt) => C::FloatLt { left, right },
        (K::Float, O::Le) => C::FloatLe { left, right },
        (K::Float, O::Gt) => C::FloatGt { left, right },
        (K::Float, O::Ge) => C::FloatGe { left, right },
        (K::Rat, O::Eq) => C::RatEq { left, right },
        (K::Rat, O::Ne) => C::RatNe { left, right },
        (K::Rat, O::Lt) => C::RatLt { left, right },
        (K::Rat, O::Le) => C::RatLe { left, right },
        (K::Rat, O::Gt) => C::RatGt { left, right },
        (K::Rat, O::Ge) => C::RatGe { left, right },
        (K::Char, O::Eq) => C::CharEq { left, right },
        (K::Char, O::Ne) => C::CharNe { left, right },
        (K::Char, O::Lt) => C::CharLt { left, right },
        (K::Char, O::Le) => C::CharLe { left, right },
        (K::Char, O::Gt) => C::CharGt { left, right },
        (K::Char, O::Ge) => C::CharGe { left, right },
        (K::Enum, O::Eq) => C::EnumEq { left, right },
        (K::Enum, O::Ne) => C::EnumNe { left, right },
        (K::Value, O::Eq) => C::ValEq { left, right },
        (K::Value, O::Ne) => C::ValNe { left, right },
        _ => return Err(LowerError::unsupported(span, format!("comparison {kind:?} {op:?}"))),
    })
}
