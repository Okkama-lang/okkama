//! Exact BigInt native operations use the same Rust arithmetic as VM integers.
use crate::host::*;
use num_bigint::BigInt;
use num_traits::{ToPrimitive, Zero};
use okkama_runtime_abi::Native;
pub(crate) fn handles(native: Native) -> bool {
    use Native::*;
    matches!(
        native,
        DecimalValid
            | DecimalCanonical
            | DecimalCoefficient
            | DecimalExponent
            | DecimalOrder
            | BigintValid
            | BigintBits
    )
}
fn parse(source: &str) -> Option<BigInt> {
    let digits = source.strip_prefix('-').unwrap_or(source);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    BigInt::parse_bytes(source.as_bytes(), 10)
}
impl Heap {
    pub(crate) fn native_numbers(
        &mut self,
        native: Native,
        args: &[Slot],
    ) -> Result<Outcome, Error> {
        use Native::*;
        if matches!(
            native,
            DecimalValid | DecimalCanonical | DecimalCoefficient | DecimalExponent | DecimalOrder
        ) {
            let parts = components(self.text(args[0])?);
            if native == DecimalValid {
                return Ok(Outcome::Value(Slot::boolean(parts.is_some())));
            }
            let invalid = || Error(format!("Native invocation \"{}\" failed", native.symbol()));
            let left = parts.ok_or_else(invalid)?;
            let value = match native {
                DecimalCanonical => self.allocate_text(canonical(&left))?,
                DecimalCoefficient => self.allocate_text(left.coefficient)?,
                DecimalExponent => Slot::small(left.exponent),
                DecimalOrder => {
                    let right = components(self.text(args[1])?).ok_or_else(invalid)?;
                    let mut order = left.sign.cmp(&right.sign);
                    if order.is_eq() && left.sign != 0 {
                        order = (left.digits.len() as i128 + left.exponent as i128)
                            .cmp(&(right.digits.len() as i128 + right.exponent as i128));
                        if order.is_eq() {
                            let width = left.digits.len().max(right.digits.len());
                            order = format!("{:0<width$}", left.digits)
                                .cmp(&format!("{:0<width$}", right.digits));
                        }
                        if left.sign < 0 {
                            order = order.reverse();
                        }
                    }
                    Slot::small(match order {
                        std::cmp::Ordering::Less => -1,
                        std::cmp::Ordering::Equal => 0,
                        std::cmp::Ordering::Greater => 1,
                    })
                }
                _ => unreachable!(),
            };
            return Ok(Outcome::Value(value));
        }
        let number = |slot| {
            parse(self.text(slot)?)
                .ok_or_else(|| Error(format!("Native invocation \"{}\" failed", native.symbol())))
        };
        if native == BigintValid {
            return Ok(Outcome::Value(Slot::boolean(
                parse(self.text(args[0])?).is_some(),
            )));
        }
        // BigInt arithmetic is the builtin's (#511); the bit length is all
        // that is left here (#13).
        let left = number(args[0])?;
        self.allocate(BigInt::from(left.bits())).map(Outcome::Value)
    }
}

/// Canonical decimal components shared with JSON; exponents remain exact during
/// parsing, then checked against the language's i64 exponent contract.
pub(crate) struct Decimal {
    pub(crate) coefficient: String,
    pub(crate) exponent: i64,
    pub(crate) digits: String,
    pub(crate) sign: i64,
}
pub(crate) fn components(source: &str) -> Option<Decimal> {
    let (negative, rest) = source
        .strip_prefix('-')
        .map_or((false, source), |s| (true, s));
    let integer_end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if integer_end == 0 {
        return None;
    }
    let integer = &rest[..integer_end];
    let mut tail = &rest[integer_end..];
    let mut fraction = "";
    if let Some(after) = tail.strip_prefix('.') {
        let end = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        if end == 0 {
            return None;
        }
        fraction = &after[..end];
        tail = &after[end..];
    }
    let mut written = BigInt::zero();
    if let Some(after) = tail.strip_prefix(['e', 'E']) {
        let (negative, digits) = match after.as_bytes().first() {
            Some(b'+') => (false, &after[1..]),
            Some(b'-') => (true, &after[1..]),
            _ => (false, after),
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        written = BigInt::parse_bytes(digits.as_bytes(), 10)?;
        if negative {
            written = -written;
        }
        tail = "";
    }
    if !tail.is_empty() {
        return None;
    }
    let all = format!("{integer}{fraction}");
    let digits = all.trim_start_matches('0');
    if digits.is_empty() {
        return Some(Decimal {
            coefficient: "0".into(),
            digits: "0".into(),
            exponent: 0,
            sign: 0,
        });
    }
    let trimmed = digits.trim_end_matches('0');
    let exponent = (written - BigInt::from(fraction.len())
        + BigInt::from(digits.len() - trimmed.len()))
    .to_i64()?;
    Some(Decimal {
        coefficient: format!("{}{trimmed}", if negative { "-" } else { "" }),
        exponent,
        digits: trimmed.into(),
        sign: if negative { -1 } else { 1 },
    })
}
pub(crate) fn canonical(parts: &Decimal) -> String {
    if parts.sign == 0 {
        return "0".into();
    }
    let mut out = String::new();
    if parts.sign < 0 {
        out.push('-');
    }
    out.push_str(&parts.digits[..1]);
    if parts.digits.len() > 1 {
        out.push('.');
        out.push_str(&parts.digits[1..]);
    }
    let exponent = parts.exponent as i128 + parts.digits.len() as i128 - 1;
    if exponent != 0 {
        out.push_str(&format!("e{exponent}"));
    }
    out
}
