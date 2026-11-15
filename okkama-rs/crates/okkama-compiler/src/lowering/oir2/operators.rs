//! Numeric, bit and text operators and scalar builtins, as the OIR-2 core ops of the same
//! semantics as OIR-1 (§13 keeps ordinary arithmetic). Failing core ops get their H5 unwind
//! from `BodyLowerer::core`; byte arithmetic widens to int (§16.2 of the language spec).
use super::body::BodyLowerer;
use super::scalar::compare;
use super::{LowerError, Result};
use crate::analysis::thir::{ArithOp, BinaryOp, BitOp, Builtin, CompareKind, CompareOp, NumericKind, UnaryOp};
use crate::oir2 as ir;
use crate::source::Span;

impl BodyLowerer<'_, '_> {
    pub(super) fn unary(&mut self, op: UnaryOp, value: ir::ValueId) -> Result<ir::ValueId> {
        let op = match op {
            UnaryOp::Not => ir::CoreOp::BoolNot { value },
            UnaryOp::Negate(NumericKind::Int) => ir::CoreOp::IntNeg { value },
            UnaryOp::Negate(NumericKind::Float) => ir::CoreOp::FloatNeg { value },
            UnaryOp::Negate(NumericKind::Rat) => ir::CoreOp::RatNeg { value },
            UnaryOp::Negate(NumericKind::Byte) => {
                let value = self.widen(value)?;
                ir::CoreOp::IntNeg { value }
            }
            UnaryOp::BitNot { byte: false } => ir::CoreOp::IntNot { value },
            UnaryOp::BitNot { byte: true } => ir::CoreOp::ByteNot { value },
        };
        self.core(op)
    }

    pub(super) fn binary(&mut self, op: BinaryOp, left: ir::ValueId, right: ir::ValueId, span: Span)
        -> Result<ir::ValueId> {
        use ir::CoreOp as C;
        let operation = match op {
            BinaryOp::Arith(NumericKind::Int, op) => int_arith(op, left, right),
            BinaryOp::Arith(NumericKind::Byte, op) => {
                let left = self.widen(left)?;
                let right = self.widen(right)?;
                int_arith(op, left, right)
            }
            BinaryOp::Arith(NumericKind::Float, op) => match op {
                ArithOp::Add => C::FloatAdd { left, right },
                ArithOp::Sub => C::FloatSub { left, right },
                ArithOp::Mul => C::FloatMul { left, right },
                ArithOp::Div => C::FloatDiv { left, right },
                ArithOp::Mod => C::FloatMod { left, right },
            },
            BinaryOp::Arith(NumericKind::Rat, op) => match op {
                ArithOp::Add => C::RatAdd { left, right },
                ArithOp::Sub => C::RatSub { left, right },
                ArithOp::Mul => C::RatMul { left, right },
                ArithOp::Div => C::RatDiv { left, right },
                ArithOp::Mod => C::RatMod { left, right },
            },
            BinaryOp::Bits { byte: false, op } => match op {
                BitOp::And => C::IntAnd { left, right },
                BitOp::Or => C::IntOr { left, right },
                BitOp::Xor => C::IntXor { left, right },
                BitOp::Shl => C::IntShl { left, right },
                BitOp::Shr => C::IntShr { left, right },
                BitOp::Ushr => C::IntUshr { left, right },
            },
            BinaryOp::Bits { byte: true, op } => match op {
                BitOp::And => C::ByteAnd { left, right },
                BitOp::Or => C::ByteOr { left, right },
                BitOp::Xor => C::ByteXor { left, right },
                BitOp::Shl => C::ByteShl { left, right },
                BitOp::Shr => C::ByteShr { left, right },
                // The language counts in int, as for << and >>, while OIR-2's byte.ushr takes a byte
                // count: narrowing it first would change the out-of-range failures. A byte has no sign,
                // so `>>>` is `>>`: byte.shr takes the int count and refuses one outside 0..7 with
                // Tomato's message, as Tomato's byte.ushr does.
                BitOp::Ushr => C::ByteShr { left, right },
            },
            BinaryOp::Concat => C::StrConcat { left, right },
            BinaryOp::Compare(kind, op) => compare(kind, op, left, right, span)?,
            _ => return Err(LowerError::unsupported(span, format!("binary {op:?}"))),
        };
        self.core(operation)
    }

    /// Scalar builtins whose receiver and arguments are already evaluated, left to right.
    pub(super) fn scalar_builtin(&mut self, op: &Builtin, value: ir::ValueId, args: &[ir::ValueId], span: Span)
        -> Result<Option<ir::ValueId>> {
        use ir::CoreOp as C;
        let operation = match (op, args) {
            (Builtin::StrEmpty, []) => {
                let length = self.core(C::StrLen { value })?;
                let zero = self.constant(ir::Literal::Int("0".into()))?;
                C::IntEq { left: length, right: zero }
            }
            (Builtin::StrSlice, &[start, end]) => C::StrSlice { value, start, end },
            (Builtin::StrSplit, &[separator]) => C::StrSplit { value, separator },
            (Builtin::StrReplace, &[target, replacement]) => C::StrReplace { value, target, replacement },
            (Builtin::StrReplaceFirst, &[target, replacement]) => {
                C::StrReplaceFirst { value, target, replacement }
            }
            (Builtin::StrTrim, []) => C::StrTrim { value },
            (Builtin::StrChomp, []) => C::StrChomp { value },
            (Builtin::StrTrimStart, []) => C::StrTrimStart { value },
            (Builtin::StrTrimEnd, []) => C::StrTrimEnd { value },
            (Builtin::StrLower, []) => C::StrLower { value },
            (Builtin::StrUpper, []) => C::StrUpper { value },
            (Builtin::StrRepeat, &[count]) => C::StrRepeat { value, count },
            (Builtin::StrStartsWith, &[part]) => C::StrStartsWith { value, part },
            (Builtin::StrEndsWith, &[part]) => C::StrEndsWith { value, part },
            (Builtin::StrCharacters, []) => C::StrCharacters { value },
            // `c.code()`: the code point of a char (#679).
            (Builtin::CharCode, []) => C::Convert { value, from: ir::NumericType::Char, to: ir::NumericType::Int },
            // Searches give a code-point index or -1 (#210); `contains` is a found index.
            (Builtin::StrContains, &[part]) => {
                let index = self.core(C::StrFind { value, part })?;
                let zero = self.constant(ir::Literal::Int("0".into()))?;
                C::IntGe { left: index, right: zero }
            }
            (Builtin::Numerator, []) => C::RatNumerator { value },
            (Builtin::Denominator, []) => C::RatDenominator { value },
            (Builtin::Abs, []) => match self.scalar_kind(value)? {
                Some(NumericKind::Int) => C::IntAbs { value },
                Some(NumericKind::Float) => C::FloatAbs { value },
                Some(NumericKind::Rat) => C::RatAbs { value },
                _ => return Ok(None),
            },
            (Builtin::Sign, []) => match self.scalar_kind(value)? {
                Some(NumericKind::Int) => C::IntSign { value },
                Some(NumericKind::Float) => C::FloatSign { value },
                Some(NumericKind::Rat) => C::RatSign { value },
                _ => return Ok(None),
            },
            (Builtin::Sqrt, []) => {
                let value = match self.scalar_kind(value)? {
                    Some(NumericKind::Int) => self.core(C::Convert {
                        value, from: ir::NumericType::Int, to: ir::NumericType::Float,
                    })?,
                    _ => value,
                };
                C::FloatSqrt { value }
            }
            (Builtin::Pow, &[exponent]) => match self.scalar_kind(value)? {
                Some(NumericKind::Float) => C::FloatPow { left: value, right: exponent },
                Some(kind @ (NumericKind::Int | NumericKind::Rat)) => {
                    return self.exact_power(value, exponent, kind == NumericKind::Rat).map(Some);
                }
                _ => return Ok(None),
            },
            (Builtin::Bits { name, byte }, args) => bits(name, *byte, value, args)
                .ok_or_else(|| LowerError::invalid(span, format!("bit action {name}")))?,
            _ => return Ok(None),
        };
        self.core(operation).map(Some)
    }

    fn widen(&mut self, value: ir::ValueId) -> Result<ir::ValueId> {
        self.core(ir::CoreOp::Convert { value, from: ir::NumericType::Byte, to: ir::NumericType::Int })
    }

    fn scalar_kind(&self, value: ir::ValueId) -> Result<Option<NumericKind>> {
        let ty = self.b.body().values.get(value)?.ty;
        Ok(match self.b.module().types.get(ty)? {
            ir::Type::Int => Some(NumericKind::Int),
            ir::Type::Byte => Some(NumericKind::Byte),
            ir::Type::Float => Some(NumericKind::Float),
            ir::Type::Rat => Some(NumericKind::Rat),
            _ => None,
        })
    }

    /// Exact `int.pow`/`rat.pow` (#228) by binary exponentiation, as OIR-1 does. An int
    /// base refuses a negative exponent; a rat base inverts the power (0 fails in rat.div).
    fn exact_power(&mut self, base: ir::ValueId, exponent: ir::ValueId, rat: bool) -> Result<ir::ValueId> {
        use ir::CoreOp as C;
        let int = self.b.intern_type(ir::Type::Int)?;
        let ty = self.b.body().values.get(base)?.ty;
        let zero = self.constant(ir::Literal::Int("0".into()))?;
        let one_int = self.constant(ir::Literal::Int("1".into()))?;
        let negative = self.core(C::IntLt { left: exponent, right: zero })?;
        let locals = self.locals();
        let (start, flip, keep) = (self.carry(&[int])?, self.carry(&[])?, self.carry(&[])?);
        self.cond(negative, flip, keep, &locals)?;
        self.enter(flip, &locals, 0)?;
        if rat {
            let positive = self.core(C::IntNeg { value: exponent })?;
            self.jump(start, &locals, &[positive])?;
        } else {
            self.terminate(ir::TermKind::Trap {
                code: ir::TrapCode::Range,
                message: "int.pow needs a non-negative exponent; a rat base takes a negative one".into(),
            })?;
        }
        self.enter(keep, &locals, 0)?;
        self.jump(start, &locals, &[exponent])?;
        self.enter(start, &locals, 1)?;
        let count = self.b.block_params(start)?[0];
        let one = if rat {
            self.constant(ir::Literal::Rat { numerator: "1".into(), denominator: "1".into() })?
        } else {
            one_int
        };
        let multiply = |left, right| if rat { C::RatMul { left, right } } else { C::IntMul { left, right } };
        // head(result, square, count): while count > 0, multiply in the square of each set bit.
        let head = self.carry(&[ty, ty, int])?;
        let done = self.carry(&[ty])?;
        let step = self.carry(&[])?;
        let times = self.carry(&[])?;
        let join = self.carry(&[ty])?;
        self.jump(head, &locals, &[one, base, count])?;
        self.enter(head, &locals, 3)?;
        let params = self.b.block_params(head)?.to_vec();
        let (result, square, left) = (params[0], params[1], params[2]);
        let more = self.core(C::IntGt { left, right: zero })?;
        let args = self.args(&locals);
        let mut finished = vec![result];
        finished.extend(args.iter().copied());
        self.terminate(ir::TermKind::Cond {
            condition: more,
            yes: ir::Target { block: step, args: args.clone() },
            no: ir::Target { block: done, args: finished },
        })?;
        self.enter(step, &locals, 0)?;
        let bit = self.core(C::IntAnd { left, right: one_int })?;
        let odd = self.core(C::IntNe { left: bit, right: zero })?;
        let args = self.args(&locals);
        let mut skipped = vec![result];
        skipped.extend(args.iter().copied());
        self.terminate(ir::TermKind::Cond {
            condition: odd,
            yes: ir::Target { block: times, args },
            no: ir::Target { block: join, args: skipped },
        })?;
        self.enter(times, &locals, 0)?;
        let multiplied = self.core(multiply(result, square))?;
        self.jump(join, &locals, &[multiplied])?;
        self.enter(join, &locals, 1)?;
        let accumulated = self.b.block_params(join)?[0];
        let squared = self.core(multiply(square, square))?;
        let halved = self.core(C::IntShr { left, right: one_int })?;
        self.jump(head, &locals, &[accumulated, squared, halved])?;
        self.enter(done, &locals, 1)?;
        let power = self.b.block_params(done)?[0];
        if !rat {
            return Ok(power);
        }
        let out = self.carry(&[ty])?;
        let invert = self.carry(&[])?;
        let args = self.args(&locals);
        let mut kept = vec![power];
        kept.extend(args.iter().copied());
        self.terminate(ir::TermKind::Cond {
            condition: negative,
            yes: ir::Target { block: invert, args },
            no: ir::Target { block: out, args: kept },
        })?;
        self.enter(invert, &locals, 0)?;
        let inverted = self.core(C::RatDiv { left: one, right: power })?;
        self.jump(out, &locals, &[inverted])?;
        self.enter(out, &locals, 1)?;
        Ok(self.b.block_params(out)?[0])
    }

    /// A block of the current scope taking `extra` values first, then the live locals.
    fn carry(&mut self, extra: &[ir::TypeId]) -> Result<ir::BlockId> {
        let mut types = extra.to_vec();
        let locals = self.locals();
        types.extend(self.local_types(&locals));
        let scope = self.scope();
        self.new_block(scope, &types)
    }

    fn jump(&mut self, block: ir::BlockId, locals: &[usize], extra: &[ir::ValueId]) -> Result<()> {
        let mut args = extra.to_vec();
        args.extend(self.args(locals));
        self.terminate(ir::TermKind::Br(ir::Target { block, args }))
    }

    fn cond(&mut self, condition: ir::ValueId, yes: ir::BlockId, no: ir::BlockId, locals: &[usize]) -> Result<()> {
        let args = self.args(locals);
        self.terminate(ir::TermKind::Cond {
            condition,
            yes: ir::Target { block: yes, args: args.clone() },
            no: ir::Target { block: no, args },
        })
    }
}

fn int_arith(op: ArithOp, left: ir::ValueId, right: ir::ValueId) -> ir::CoreOp {
    match op {
        ArithOp::Add => ir::CoreOp::IntAdd { left, right },
        ArithOp::Sub => ir::CoreOp::IntSub { left, right },
        ArithOp::Mul => ir::CoreOp::IntMul { left, right },
        // `int / int` is an exact rat; a zero divisor traps.
        ArithOp::Div => ir::CoreOp::IntDiv { left, right },
        ArithOp::Mod => ir::CoreOp::IntMod { left, right },
    }
}

fn bits(name: &str, byte: bool, value: ir::ValueId, args: &[ir::ValueId]) -> Option<ir::CoreOp> {
    use ir::CoreOp as C;
    Some(match (name, byte, args) {
        ("bit_count", false, []) => C::IntBitCount { value },
        ("leading_zeros", false, []) => C::IntLeadingZeros { value },
        ("trailing_zeros", false, []) => C::IntTrailingZeros { value },
        ("reverse_bits", false, []) => C::IntReverseBits { value },
        ("swap_bytes", false, []) => C::IntSwapBytes { value },
        ("rotate_left", false, &[count]) => C::IntRotateLeft { value, count },
        ("rotate_right", false, &[count]) => C::IntRotateRight { value, count },
        ("bit_count", true, []) => C::ByteBitCount { value },
        ("leading_zeros", true, []) => C::ByteLeadingZeros { value },
        ("trailing_zeros", true, []) => C::ByteTrailingZeros { value },
        ("reverse_bits", true, []) => C::ByteReverseBits { value },
        ("swap_bytes", true, []) => C::ByteSwapBytes { value },
        ("rotate_left", true, &[count]) => C::ByteRotateLeft { value, count },
        ("rotate_right", true, &[count]) => C::ByteRotateRight { value, count },
        _ => return None,
    })
}

/// Equality kinds of a match subject: the scalar categories with a core equality op.
pub(super) fn match_kind(ty: &ir::Type) -> Option<CompareKind> {
    Some(match ty {
        ir::Type::Int => CompareKind::Int,
        ir::Type::Bool => CompareKind::Bool,
        ir::Type::Str => CompareKind::Str,
        ir::Type::Byte => CompareKind::Byte,
        ir::Type::Float => CompareKind::Float,
        ir::Type::Rat => CompareKind::Rat,
        ir::Type::Char => CompareKind::Char,
        ir::Type::Enum(_) => CompareKind::Enum,
        _ => return None,
    })
}

pub(super) fn equality(kind: CompareKind, left: ir::ValueId, right: ir::ValueId, span: Span) -> Result<ir::CoreOp> {
    compare(kind, CompareOp::Eq, left, right, span)
}
