//! Точное рациональное `rat` (OIR-1 §16.2): несократимая дробь с
//! положительным знаменателем. Текст — `n/d`, у целого — `n`. Ошибки и
//! преобразования с едиными правилами округления.

use crate::IntValue;
use crate::integer::{add, compare, divide, gcd, multiply, subtract};
use std::cmp::Ordering;
use std::fmt;

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Rat {
    num: IntValue,
    den: IntValue,
}

fn one() -> IntValue {
    IntValue::from_i64(1)
}

fn pow10(exponent: u32) -> IntValue {
    IntValue::from_big(crate::bignum::Big::from_u64(10).power(exponent))
}

/// Both parts of a fraction as machine words, when they fit (#1180): the
/// arithmetic below then runs in `i128` and reduces with `u64` gcds, as Knuth
/// 4.5.1 does, instead of going through `Big`. A time is such a fraction
/// (nanoseconds over 10⁹), and one multiplication by 10⁹ took 0.5 µs.
fn small_parts(value: &Rat) -> Option<(i64, i64)> {
    Some((value.num.to_i64()?, value.den.to_i64()?))
}

fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// A reduced fraction from parts already in lowest terms, `den > 0`.
fn from_reduced(num: i128, den: i128) -> Rat {
    if num == 0 {
        return Rat {
            num: IntValue::from_i64(0),
            den: one(),
        };
    }
    Rat {
        num: IntValue::from_i128(num),
        den: IntValue::from_i128(den),
    }
}

/// `a/b * c/d` for reduced fractions with `b, d > 0`: cancel across first,
/// then the products are already in lowest terms.
fn small_mul(a: i64, b: i64, c: i64, d: i64) -> Rat {
    let g1 = gcd_u64(a.unsigned_abs(), d as u64).max(1) as i128;
    let g2 = gcd_u64(c.unsigned_abs(), b as u64).max(1) as i128;
    let num = (i128::from(a) / g1) * (i128::from(c) / g2);
    let den = (i128::from(b) / g2) * (i128::from(d) / g1);
    from_reduced(num, den)
}

/// `a/b ± c/d` for reduced fractions with `b, d > 0` (Knuth 4.5.1).
fn small_add(a: i64, b: i64, c: i64, d: i64, subtract: bool) -> Rat {
    let c = if subtract { -i128::from(c) } else { i128::from(c) };
    let d1 = gcd_u64(b as u64, d as u64);
    if d1 == 1 {
        // |a·d| and |c·b| are below 2¹²⁶, so their sum fits.
        let num = i128::from(a) * i128::from(d) + c * i128::from(b);
        return from_reduced(num, i128::from(b) * i128::from(d));
    }
    let d1 = i128::from(d1);
    let t = i128::from(a) * (i128::from(d) / d1) + c * (i128::from(b) / d1);
    // gcd(t, d1) through t mod d1; it is d1 itself when t is a multiple.
    let d2 = i128::from(gcd_u64((t.unsigned_abs() % d1 as u128) as u64, d1 as u64));
    from_reduced(t / d2, (i128::from(b) / d1) * (i128::from(d) / d2))
}

impl Rat {
    fn normalized(mut num: IntValue, mut den: IntValue) -> Self {
        if den.is_negative() {
            num = num.negated();
            den = den.negated();
        }
        let common = gcd(&num, &den);
        if !common.is_zero() && common != one() {
            num = divide(&num, &common).expect("non-zero gcd").0;
            den = divide(&den, &common).expect("non-zero gcd").0;
        }
        if num.is_zero() {
            den = one();
        }
        Self { num, den }
    }

    /// `rat(n, d)` и `n / d` над целыми.
    pub fn new(num: &IntValue, den: &IntValue) -> Result<Self> {
        if den.is_zero() {
            return Err("Division by zero".into());
        }
        if let (Some(n), Some(d)) = (num.to_i64(), den.to_i64()) {
            // Words: one u64 gcd, no IntValue arithmetic (a clock reading is rat(ns, 10⁹)).
            let g = i128::from(gcd_u64(n.unsigned_abs(), d.unsigned_abs()));
            let (n, d) = if d < 0 { (-i128::from(n), -i128::from(d)) } else { (i128::from(n), i128::from(d)) };
            return Ok(from_reduced(n / g, d / g));
        }
        Ok(Self::normalized(num.clone(), den.clone()))
    }

    pub fn from_int(value: &IntValue) -> Self {
        Self {
            num: value.clone(),
            den: one(),
        }
    }

    /// `float.rat()`: точная дробь канонической десятичной записи float.
    pub fn from_float(value: f64) -> Self {
        let text = crate::float::text(value);
        let (mantissa, exponent) = match text.split_once('e') {
            Some((mantissa, exponent)) => (
                mantissa.to_string(),
                exponent.parse::<i32>().expect("float text exponent"),
            ),
            None => (text.clone(), 0),
        };
        let (negative, mantissa) = match mantissa.strip_prefix('-') {
            Some(rest) => (true, rest.to_string()),
            None => (false, mantissa),
        };
        let (whole, fraction) = mantissa.split_once('.').unwrap_or((&mantissa, ""));
        let digits = IntValue::from_decimal_digits(&format!("{whole}{fraction}"))
            .expect("float text digits");
        let digits = if negative { digits.negated() } else { digits };
        let scale = fraction.len() as i32 - exponent;
        if scale <= 0 {
            Self {
                num: multiply(&digits, &pow10(scale.unsigned_abs())),
                den: one(),
            }
        } else {
            Self::normalized(digits, pow10(scale as u32))
        }
    }

    pub fn add(&self, other: &Self) -> Self {
        if let (Some((a, b)), Some((c, d))) = (small_parts(self), small_parts(other)) {
            return small_add(a, b, c, d, false);
        }
        Self::normalized(
            add(
                &multiply(&self.num, &other.den),
                &multiply(&other.num, &self.den),
            ),
            multiply(&self.den, &other.den),
        )
    }

    pub fn sub(&self, other: &Self) -> Self {
        if let (Some((a, b)), Some((c, d))) = (small_parts(self), small_parts(other)) {
            return small_add(a, b, c, d, true);
        }
        Self::normalized(
            subtract(
                &multiply(&self.num, &other.den),
                &multiply(&other.num, &self.den),
            ),
            multiply(&self.den, &other.den),
        )
    }

    pub fn mul(&self, other: &Self) -> Self {
        if let (Some((a, b)), Some((c, d))) = (small_parts(self), small_parts(other)) {
            return small_mul(a, b, c, d);
        }
        Self::normalized(
            multiply(&self.num, &other.num),
            multiply(&self.den, &other.den),
        )
    }

    pub fn div(&self, other: &Self) -> Result<Self> {
        if other.num.is_zero() {
            return Err("Division by zero".into());
        }
        if let (Some((a, b)), Some((c, d))) = (small_parts(self), small_parts(other)) {
            // The reciprocal d/c with a positive denominator; -i64::MIN does not fit.
            if c != i64::MIN {
                return Ok(if c < 0 { small_mul(a, b, -d, -c) } else { small_mul(a, b, d, c) });
            }
        }
        Ok(Self::normalized(
            multiply(&self.num, &other.den),
            multiply(&self.den, &other.num),
        ))
    }

    /// Остаток со знаком делимого: `a - b * trunc(a / b)` (D-031).
    pub fn modulo(&self, other: &Self) -> Result<Self> {
        if other.num.is_zero() {
            return Err("Modulo by zero".into());
        }
        let quotient = divide(
            &multiply(&self.num, &other.den),
            &multiply(&self.den, &other.num),
        )
        .expect("non-zero divisor")
        .0;
        Ok(self.sub(&other.mul(&Self::from_int(&quotient))))
    }

    pub fn neg(&self) -> Self {
        Self {
            num: self.num.negated(),
            den: self.den.clone(),
        }
    }

    pub fn abs(&self) -> Self {
        if self.num.is_negative() {
            self.neg()
        } else {
            self.clone()
        }
    }

    pub fn compare(&self, other: &Self) -> Ordering {
        if let (Some((a, b)), Some((c, d))) = (small_parts(self), small_parts(other)) {
            return (i128::from(a) * i128::from(d)).cmp(&(i128::from(c) * i128::from(b)));
        }
        compare(
            &multiply(&self.num, &other.den),
            &multiply(&other.num, &self.den),
        )
    }

    pub fn sign(&self) -> i64 {
        self.num.signum()
    }

    pub fn numerator(&self) -> &IntValue {
        &self.num
    }

    pub fn denominator(&self) -> &IntValue {
        &self.den
    }

    /// Усечение к нулю.
    pub fn to_int(&self) -> IntValue {
        divide(&self.num, &self.den).expect("positive denominator").0
    }

    /// Как `BigDecimal.divide(MathContext.DECIMAL128).doubleValue()`: частное,
    /// округлённое до 34 значащих цифр (half even), затем ближайшее binary64.
    pub fn to_float(&self) -> Result<f64> {
        let result = if self.num.is_zero() {
            0.0
        } else {
            let magnitude = if self.num.is_negative() {
                self.num.negated()
            } else {
                self.num.clone()
            };
            let digits = |value: &IntValue| value.to_string().len() as i64;
            // Ищем k: 10^33 <= |n| * 10^k / d < 10^34.
            let mut k = 33 - (digits(&magnitude) - digits(&self.den));
            let low = pow10(33);
            let (quotient, rest, divisor) = loop {
                let (scaled, divisor) = if k >= 0 {
                    (multiply(&magnitude, &pow10(k as u32)), self.den.clone())
                } else {
                    (magnitude.clone(), multiply(&self.den, &pow10((-k) as u32)))
                };
                let (quotient, rest) = divide(&scaled, &divisor).expect("positive divisor");
                if compare(&quotient, &low).is_lt() {
                    k += 1;
                } else if quotient.to_string().len() > 34 {
                    k -= 1;
                } else {
                    break (quotient, rest, divisor);
                }
            };
            let twice = multiply(&rest, &IntValue::from_i64(2));
            let odd = || !divide(&quotient, &IntValue::from_i64(2)).expect("two").1.is_zero();
            let quotient = match compare(&twice, &divisor) {
                Ordering::Greater => add(&quotient, &one()),
                Ordering::Equal if odd() => add(&quotient, &one()),
                _ => quotient,
            };
            let sign = if self.num.is_negative() { "-" } else { "" };
            format!("{sign}{quotient}e{}", -k)
                .parse::<f64>()
                .expect("decimal text is a float")
        };
        if !result.is_finite() {
            return Err("Cannot convert rat to float: expected a finite binary64 value".into());
        }
        Ok(result)
    }

    /// Усечение к нулю, затем 0..255; `context` — номер операнда `@byte()`.
    pub fn to_byte(&self, context: Option<&str>) -> Result<u8> {
        let truncated = self.to_int();
        truncated
            .to_i64()
            .and_then(|value| u8::try_from(value).ok())
            .ok_or_else(|| {
                let message = format!("Cannot convert rat value {self} to byte: expected 0..255");
                match context {
                    Some(context) => format!("{context}: {message}"),
                    None => message,
                }
            })
    }
}

impl fmt::Display for Rat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == one() {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rat(n: i64, d: i64) -> Rat {
        Rat::new(&IntValue::from_i64(n), &IntValue::from_i64(d)).unwrap()
    }

    #[test]
    fn fractions_are_reduced_with_positive_denominators() {
        assert_eq!(rat(6, -4).to_string(), "-3/2");
        assert_eq!(rat(0, -5).to_string(), "0");
        assert_eq!(rat(1, 2).add(&rat(1, 3)).to_string(), "5/6");
        assert_eq!(rat(-7, 2).modulo(&rat(1, 1)).unwrap().to_string(), "-1/2");
        assert_eq!(rat(-7, 2).to_int(), IntValue::from_i64(-3));
        assert!(rat(1, 3).div(&rat(0, 1)).is_err());
    }

    #[test]
    fn float_conversions_use_canonical_decimal_text() {
        assert_eq!(Rat::from_float(0.1).to_string(), "1/10");
        assert_eq!(Rat::from_float(1e21).to_string(), "1000000000000000000000");
        assert_eq!(Rat::from_float(-2.5e-7).to_string(), "-1/4000000");
        assert_eq!(rat(1, 3).to_float().unwrap(), 1.0 / 3.0);
        assert_eq!(rat(-2, 3).to_float().unwrap(), -2.0 / 3.0);
        assert_eq!(rat(1, 10).to_float().unwrap(), 0.1);
        assert_eq!(
            rat(300, 1).to_byte(None).unwrap_err(),
            "Cannot convert rat value 300 to byte: expected 0..255"
        );
    }
    /// The word fast paths (#1180) agree with the general arithmetic over
    /// `Big` on edge values and on a deterministic spread of fractions.
    #[test]
    fn word_fast_paths_agree_with_big_arithmetic() {
        fn reference(op: usize, x: &Rat, y: &Rat) -> Option<Rat> {
            let (a, b, c, d) = (&x.num, &x.den, &y.num, &y.den);
            match op {
                0 => Some(Rat::normalized(add(&multiply(a, d), &multiply(c, b)), multiply(b, d))),
                1 => Some(Rat::normalized(subtract(&multiply(a, d), &multiply(c, b)), multiply(b, d))),
                2 => Some(Rat::normalized(multiply(a, c), multiply(b, d))),
                _ if c.is_zero() => None,
                _ => Some(Rat::normalized(multiply(a, d), multiply(b, c))),
            }
        }
        let edges = [
            0,
            1,
            -1,
            2,
            -2,
            7,
            1_000_000_000,
            -1_000_000_000,
            1_700_000_000_123_456_789,
            i64::MAX,
            i64::MIN,
            i64::MIN + 1,
            i64::MAX - 1,
            3_037_000_499,
            4_611_686_018_427_387_904,
        ];
        let mut values: Vec<Rat> = Vec::new();
        for &n in &edges {
            for &d in &[1, 2, 3, 10, 1_000_000_000, i64::MAX, 4_611_686_018_427_387_904, 999_999_937] {
                values.push(rat_parts(n, d));
            }
        }
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = || {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            state
        };
        for _ in 0..150 {
            let n = (next() as i64) >> (next() % 63);
            let d = ((next() >> 1) as i64 >> (next() % 62)).max(1);
            values.push(rat_parts(n, d));
        }
        for &n in &edges {
            for &d in &edges {
                if d != 0 {
                    let made = Rat::new(&IntValue::from_i64(n), &IntValue::from_i64(d)).unwrap();
                    assert_eq!(made, Rat::normalized(IntValue::from_i64(n), IntValue::from_i64(d)), "rat({n}, {d})");
                }
            }
        }
        for x in &values {
            for y in &values {
                assert_eq!(x.add(y), reference(0, x, y).unwrap(), "{x} + {y}");
                assert_eq!(x.sub(y), reference(1, x, y).unwrap(), "{x} - {y}");
                assert_eq!(x.mul(y), reference(2, x, y).unwrap(), "{x} * {y}");
                match reference(3, x, y) {
                    Some(quotient) => assert_eq!(x.div(y).unwrap(), quotient, "{x} / {y}"),
                    None => assert!(x.div(y).is_err()),
                }
                let expected = compare(&multiply(&x.num, &y.den), &multiply(&y.num, &x.den));
                assert_eq!(x.compare(y), expected, "{x} <=> {y}");
            }
        }
    }

    fn rat_parts(n: i64, d: i64) -> Rat {
        Rat::new(&IntValue::from_i64(n), &IntValue::from_i64(d)).unwrap()
    }
}
