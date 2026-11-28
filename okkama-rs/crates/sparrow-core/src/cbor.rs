//! CBOR (RFC 8949, #1082): the data model, a decoder bounded against hostile input, the preferred and the core
//! deterministic encoders, and diagnostic notation. `runtime/cbor`'s natives (`host/native_cbor.rs`) and the fuzz
//! target `cbor` use it.
//!
//! - **Decoding** is iterative (no recursion on the input), never allocates from a declared length before the
//!   bytes are there, and stops at `Limits`: nesting depth, data items, total bytes. It accepts indefinite lengths
//!   and refuses what is not well-formed (Appendix F), text that is not UTF-8, repeated map keys, and bignum
//!   (tags 2, 3) or rational (tag 30) tags with the wrong content. Every failure names its byte offset.
//! - **Values** keep what Okkama cannot hold (non-finite floats, simple values, `undefined`, tags the codec does
//!   not interpret), so a decoded value encodes back to the same data model value.
//! - **Encoding** writes preferred serialization (§4.1: shortest heads, floats in the shortest width that keeps
//!   the value, definite lengths); the deterministic form (§4.2.1) also sorts map keys by their encoded bytes.
use num_bigint::{BigInt, Sign};
use num_integer::Integer as _;
use num_traits::{One, Signed, Zero};
use std::{cell::OnceCell, collections::HashMap, collections::HashSet, rc::Rc};

/// A CBOR data item.
#[derive(Debug)]
pub enum Value {
    /// An integer of major type 0 or 1: −2⁶⁴ ≤ n < 2⁶⁴.
    Integer(i128),
    /// An integer outside that range: a bignum (tags 2 and 3). Never holds a value `Integer` can.
    Big(BigInt),
    /// A float of any width, decoded to binary64 (every half and single is exact in it).
    Float(f64),
    /// A rational number (tag 30): reduced, with a positive denominator.
    Rational(BigInt, BigInt),
    Text(String),
    Bytes(Vec<u8>),
    Bool(bool),
    Null,
    Undefined,
    /// A simple value other than false, true, null and undefined: 0..=19 or 32..=255.
    Simple(u8),
    Array(Vec<Rc<Value>>),
    Map(Map),
    /// A tag the codec does not interpret, with its content.
    Tag(u64, Rc<Value>),
}

/// The integer `n`, as `Integer` when major types 0 and 1 hold it, else as a bignum.
pub fn integer(n: BigInt) -> Value {
    match i128::try_from(&n) {
        Ok(small) if in_heads(small) => Value::Integer(small),
        _ => Value::Big(n),
    }
}

fn in_heads(n: i128) -> bool {
    (-(1i128 << 64)..(1i128 << 64)).contains(&n)
}

/// The rational `numerator / denominator`, reduced; none for a zero denominator.
pub fn rational(numerator: BigInt, denominator: BigInt) -> Option<Value> {
    if denominator.is_zero() {
        return None;
    }
    let divisor = numerator.gcd(&denominator);
    let (mut n, mut d) = (numerator / &divisor, denominator / &divisor);
    if d.is_negative() {
        n = -n;
        d = -d;
    }
    Some(Value::Rational(n, d))
}

/// The pairs of a map in their order, and an index of the keys built on the first lookup of a large map.
#[derive(Debug, Default)]
pub struct Map {
    keys: Vec<Rc<Value>>,
    values: Vec<Rc<Value>>,
    // Boxed: few maps are indexed, and every value pays for `Value`'s size.
    #[allow(clippy::box_collection)]
    index: OnceCell<Box<HashMap<Vec<u8>, usize>>>,
}

/// Below this size a map finds a key by a scan, which beats hashing.
const INDEXED_MAP: usize = 16;

impl Map {
    /// The map of these pairs; `Err(i)` when key `i` repeats an earlier one.
    pub fn new(keys: Vec<Rc<Value>>, values: Vec<Rc<Value>>) -> Result<Self, usize> {
        assert_eq!(keys.len(), values.len(), "a map has as many keys as values");
        if let Some(repeated) = first_repeated(&keys) {
            return Err(repeated);
        }
        Ok(Self { keys, values, index: OnceCell::new() })
    }
    pub fn len(&self) -> usize {
        self.keys.len()
    }
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
    pub fn keys(&self) -> &[Rc<Value>] {
        &self.keys
    }
    pub fn values(&self) -> &[Rc<Value>] {
        &self.values
    }
    /// The position of the key equal to `key` in the data model (the same deterministic encoding).
    pub fn position(&self, key: &Value) -> Option<usize> {
        if self.keys.len() < INDEXED_MAP {
            let sought = KeyId::of(key);
            return self.keys.iter().position(|k| KeyId::of(k) == sought);
        }
        self.indexed(&encode(key, true))
    }
    /// The position of the text key `key`.
    pub fn text_position(&self, key: &str) -> Option<usize> {
        if self.keys.len() < INDEXED_MAP {
            return self.keys.iter().position(|k| matches!(&**k, Value::Text(t) if t == key));
        }
        let mut sought = Vec::with_capacity(key.len() + 9);
        head(&mut sought, 3, key.len() as u64);
        sought.extend_from_slice(key.as_bytes());
        self.indexed(&sought)
    }
    fn indexed(&self, sought: &[u8]) -> Option<usize> {
        let index = self.index.get_or_init(|| {
            let mut index = HashMap::with_capacity(self.keys.len());
            for (at, key) in self.keys.iter().enumerate() {
                index.insert(encode(key, true), at);
            }
            Box::new(index)
        });
        index.get(sought).copied()
    }
}

/// What makes two keys the same data model value, cheap for the common text and integer keys: two keys are
/// equal exactly when their deterministic encodings are.
#[derive(PartialEq, Eq, Hash)]
enum KeyId<'a> {
    Text(&'a str),
    Integer(i128),
    Encoded(Vec<u8>),
}
impl<'a> KeyId<'a> {
    fn of(key: &'a Value) -> Self {
        match key {
            Value::Text(text) => Self::Text(text),
            Value::Integer(n) => Self::Integer(*n),
            other => Self::Encoded(encode(other, true)),
        }
    }
}

/// Whether two keys are the same value; text and integer keys compare without encoding.
fn same_key(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Text(a), Value::Text(b)) => a == b,
        (Value::Integer(a), Value::Integer(b)) => a == b,
        (Value::Text(_) | Value::Integer(_), _) | (_, Value::Text(_) | Value::Integer(_)) => false,
        _ => encode(a, true) == encode(b, true),
    }
}

/// The index of the first key that repeats an earlier one.
pub fn first_repeated(keys: &[Rc<Value>]) -> Option<usize> {
    if keys.len() < INDEXED_MAP {
        return (1..keys.len()).find(|&i| keys[..i].iter().any(|earlier| same_key(earlier, &keys[i])));
    }
    let ids: Vec<KeyId> = keys.iter().map(|k| KeyId::of(k)).collect();
    let mut seen = HashSet::with_capacity(ids.len());
    ids.iter().position(|id| !seen.insert(id))
}

/// A deep value is dropped with a stack of its own, not by recursion.
impl Drop for Value {
    fn drop(&mut self) {
        fn take(value: &mut Value, pending: &mut Vec<Rc<Value>>) {
            match value {
                Value::Array(items) => pending.append(items),
                Value::Map(map) => {
                    pending.append(&mut map.keys);
                    pending.append(&mut map.values);
                }
                Value::Tag(_, content) => pending.push(std::mem::replace(content, Rc::new(Value::Null))),
                _ => {}
            }
        }
        let mut pending = Vec::new();
        take(self, &mut pending);
        while let Some(child) = pending.pop() {
            if let Ok(mut value) = Rc::try_unwrap(child) {
                take(&mut value, &mut pending);
            }
        }
    }
}

// ---- decoding ----

/// The bounds of one decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Containers and tags nest at most this deep: 1 allows `[1, 2]` but not `[[1]]`.
    pub depth: usize,
    /// At most this many data items (every head but a break or a string chunk).
    pub items: usize,
    /// At most this many bytes.
    pub size: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { depth: 128, items: 1 << 20, size: 16 << 20 }
    }
}

/// Why a decode failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The input ends inside an item.
    Truncated,
    /// Not well-formed (RFC 8949 Appendix F), and why.
    Malformed(&'static str),
    /// A text string that is not UTF-8.
    InvalidUtf8,
    /// A map key equal to an earlier key of the same map.
    DuplicateKey,
    /// A bignum or rational tag whose content is not what the tag needs.
    InvalidTag(u64, &'static str),
    /// Bytes after the item.
    Trailing,
    /// Nested deeper than the limit.
    TooDeep(usize),
    /// More data items than the limit.
    TooManyItems(usize),
    /// More bytes than the limit.
    TooLarge(usize),
}
impl Failure {
    /// The code `runtime/cbor` reports.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Truncated => "truncated",
            Self::Malformed(_) => "malformed",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::DuplicateKey => "duplicate_key",
            Self::InvalidTag(..) => "invalid_tag",
            Self::Trailing => "trailing",
            Self::TooDeep(_) => "too_deep",
            Self::TooManyItems(_) => "too_many_items",
            Self::TooLarge(_) => "too_large",
        }
    }
    /// The reason in words, empty when the code says it all.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Malformed(reason) | Self::InvalidTag(_, reason) => reason,
            _ => "",
        }
    }
    /// The limit passed, or the tag refused; 0 otherwise.
    pub fn number(&self) -> u64 {
        match self {
            Self::InvalidTag(tag, _) => *tag,
            Self::TooDeep(limit) | Self::TooManyItems(limit) | Self::TooLarge(limit) => *limit as u64,
            _ => 0,
        }
    }
}

/// A failed decode: why, and the byte offset of the item at fault (the item that runs past the end, the head
/// that is malformed, the repeated key, the first trailing byte, the item past a limit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    pub failure: Failure,
    pub offset: usize,
}

/// The one item that is the whole of `bytes`.
pub fn decode(bytes: &[u8], limits: Limits) -> Result<Rc<Value>, DecodeError> {
    if bytes.len() > limits.size {
        return Err(DecodeError { failure: Failure::TooLarge(limits.size), offset: limits.size });
    }
    let (value, used) = decode_first(bytes, limits)?;
    if used < bytes.len() {
        return Err(DecodeError { failure: Failure::Trailing, offset: used });
    }
    Ok(value)
}

/// The first item of a CBOR sequence (RFC 8742) and its length in bytes. An item that is not whole within
/// `limits.size` bytes is `TooLarge`; one cut off by the end of `bytes` is `Truncated`.
pub fn decode_first(bytes: &[u8], limits: Limits) -> Result<(Rc<Value>, usize), DecodeError> {
    let window = &bytes[..bytes.len().min(limits.size)];
    match (Decoder { bytes: window, at: 0, items: 0, limits }).run() {
        Err(DecodeError { failure: Failure::Truncated, .. }) if bytes.len() > limits.size => {
            Err(DecodeError { failure: Failure::TooLarge(limits.size), offset: limits.size })
        }
        other => other,
    }
}

struct Decoder<'a> {
    bytes: &'a [u8],
    at: usize,
    items: usize,
    limits: Limits,
}

/// A container or tag being read.
enum Frame {
    /// `left`: items still to come, none when the length is indefinite.
    Array { items: Vec<Rc<Value>>, left: Option<u64>, start: usize },
    /// `left`: keys and values still to come; `offsets`: where each key starts.
    Map { keys: Vec<Rc<Value>>, values: Vec<Rc<Value>>, offsets: Vec<usize>, left: Option<u64>, start: usize },
    Tag { number: u64, start: usize },
}

/// A head's argument.
enum Argument {
    Value(u64),
    Indefinite,
}

impl Decoder<'_> {
    fn fail<T>(failure: Failure, offset: usize) -> Result<T, DecodeError> {
        Err(DecodeError { failure, offset })
    }
    fn left(&self) -> usize {
        self.bytes.len() - self.at
    }
    fn byte(&mut self, start: usize) -> Result<u8, DecodeError> {
        let Some(&byte) = self.bytes.get(self.at) else { return Self::fail(Failure::Truncated, start) };
        self.at += 1;
        Ok(byte)
    }
    fn take(&mut self, count: u64, start: usize) -> Result<&[u8], DecodeError> {
        if count > self.left() as u64 {
            return Self::fail(Failure::Truncated, start);
        }
        let taken = &self.bytes[self.at..self.at + count as usize];
        self.at += count as usize;
        Ok(taken)
    }
    fn argument(&mut self, info: u8, start: usize) -> Result<Argument, DecodeError> {
        let width = match info {
            0..=23 => return Ok(Argument::Value(u64::from(info))),
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            31 => return Ok(Argument::Indefinite),
            _ => return Self::fail(Failure::Malformed("reserved additional information (28-30)"), start),
        };
        let raw = self.take(width, start)?;
        Ok(Argument::Value(raw.iter().fold(0u64, |n, &b| (n << 8) | u64::from(b))))
    }

    /// The bytes of a definite string or the joined chunks of an indefinite one.
    fn string(&mut self, major: u8, argument: Argument, start: usize) -> Result<Vec<u8>, DecodeError> {
        let check = |chunk: &[u8], at: usize| {
            if major == 3 && std::str::from_utf8(chunk).is_err() {
                return Self::fail(Failure::InvalidUtf8, at);
            }
            Ok(())
        };
        match argument {
            // A definite text string is checked as it becomes a String.
            Argument::Value(length) => Ok(self.take(length, start)?.to_vec()),
            Argument::Indefinite => {
                let mut joined = Vec::new();
                loop {
                    let chunk_start = self.at;
                    let initial = self.byte(start)?;
                    if initial == 0xff {
                        return Ok(joined);
                    }
                    if initial >> 5 != major || initial & 31 == 31 {
                        return Self::fail(
                            Failure::Malformed("a chunk of an indefinite-length string is not a definite string of its type"),
                            chunk_start,
                        );
                    }
                    let Argument::Value(length) = self.argument(initial & 31, chunk_start)? else { unreachable!() };
                    let chunk = self.take(length, start)?;
                    check(chunk, chunk_start)?;
                    joined.extend_from_slice(chunk);
                }
            }
        }
    }

    fn run(mut self) -> Result<(Rc<Value>, usize), DecodeError> {
        let mut stack: Vec<Frame> = Vec::new();
        loop {
            let start = self.at;
            let initial = self.byte(stack_start(&stack, start))?;
            let mut value = if initial == 0xff {
                match stack.pop() {
                    Some(Frame::Array { items, left: None, .. }) => Value::Array(items),
                    Some(Frame::Map { keys, values, offsets, left: None, .. }) if keys.len() == values.len() => {
                        map(keys, values, &offsets)?
                    }
                    Some(Frame::Map { left: None, .. }) => {
                        return Self::fail(Failure::Malformed("a break after a map key, before its value"), start);
                    }
                    _ => return Self::fail(Failure::Malformed("a break outside an indefinite-length item"), start),
                }
            } else {
                self.items += 1;
                if self.items > self.limits.items {
                    return Self::fail(Failure::TooManyItems(self.limits.items), start);
                }
                let (major, info) = (initial >> 5, initial & 31);
                let argument = self.argument(info, start)?;
                match (major, argument) {
                    (0, Argument::Value(n)) => Value::Integer(i128::from(n)),
                    (1, Argument::Value(n)) => Value::Integer(-1 - i128::from(n)),
                    (2, argument) => Value::Bytes(self.string(2, argument, start)?),
                    (3, argument) => match String::from_utf8(self.string(3, argument, start)?) {
                        Ok(text) => Value::Text(text),
                        Err(_) => return Self::fail(Failure::InvalidUtf8, start),
                    },
                    (4 | 5 | 6, argument) => {
                        let left = match argument {
                            Argument::Value(n) if major == 5 => {
                                // A key and a value take at least a byte each.
                                if n > (self.left() / 2) as u64 {
                                    return Self::fail(Failure::Truncated, start);
                                }
                                Some(n * 2)
                            }
                            Argument::Value(n) if major == 4 => {
                                if n > self.left() as u64 {
                                    return Self::fail(Failure::Truncated, start);
                                }
                                Some(n)
                            }
                            Argument::Value(n) => {
                                if stack.len() >= self.limits.depth {
                                    return Self::fail(Failure::TooDeep(self.limits.depth), start);
                                }
                                stack.push(Frame::Tag { number: n, start });
                                continue;
                            }
                            Argument::Indefinite if major == 6 => {
                                return Self::fail(Failure::Malformed("an indefinite-length tag"), start);
                            }
                            Argument::Indefinite => None,
                        };
                        if left == Some(0) {
                            if major == 4 { Value::Array(Vec::new()) } else { Value::Map(Map::default()) }
                        } else {
                            if stack.len() >= self.limits.depth {
                                return Self::fail(Failure::TooDeep(self.limits.depth), start);
                            }
                            // Room for the declared items, never more than a few: a hostile count costs nothing.
                            let room = left.map_or(4, |n| n.min(64) as usize);
                            stack.push(if major == 4 {
                                Frame::Array { items: Vec::with_capacity(room), left, start }
                            } else {
                                let room = room / 2;
                                Frame::Map {
                                    keys: Vec::with_capacity(room),
                                    values: Vec::with_capacity(room),
                                    offsets: Vec::with_capacity(room),
                                    left,
                                    start,
                                }
                            });
                            continue;
                        }
                    }
                    (7, Argument::Value(n)) => match info {
                        20 => Value::Bool(false),
                        21 => Value::Bool(true),
                        22 => Value::Null,
                        23 => Value::Undefined,
                        0..=19 => Value::Simple(info),
                        24 if n < 32 => {
                            return Self::fail(Failure::Malformed("a simple value below 32 in an extra byte"), start);
                        }
                        24 => Value::Simple(n as u8),
                        25 => Value::Float(half(n as u16)),
                        26 => Value::Float(f64::from(f32::from_bits(n as u32))),
                        _ => Value::Float(f64::from_bits(n)),
                    },
                    (_, _) => return Self::fail(Failure::Malformed("an indefinite length on an integer"), start),
                }
            };
            // The value is whole: give it to the open frames, closing each one it completes.
            let mut value_start = start;
            loop {
                match stack.last_mut() {
                    None => return Ok((Rc::new(value), self.at)),
                    Some(Frame::Array { items, left, start }) => {
                        items.push(Rc::new(value));
                        if !count_down(left) {
                            break;
                        }
                        value_start = *start;
                        let Some(Frame::Array { items, .. }) = stack.pop() else { unreachable!() };
                        value = Value::Array(items);
                    }
                    Some(Frame::Map { keys, values, offsets, left, start }) => {
                        if keys.len() == values.len() {
                            keys.push(Rc::new(value));
                            offsets.push(value_start);
                        } else {
                            values.push(Rc::new(value));
                        }
                        if !count_down(left) {
                            break;
                        }
                        value_start = *start;
                        let Some(Frame::Map { keys, values, offsets, .. }) = stack.pop() else { unreachable!() };
                        value = map(keys, values, &offsets)?;
                    }
                    Some(&mut Frame::Tag { number, start }) => {
                        stack.pop();
                        value = tag(number, Rc::new(value)).map_err(|failure| DecodeError { failure, offset: start })?;
                        value_start = start;
                    }
                }
            }
        }
    }
}

/// The offset a truncation reports when the next head is missing: the innermost open item.
fn stack_start(stack: &[Frame], at: usize) -> usize {
    match stack.last() {
        Some(Frame::Array { start, .. } | Frame::Map { start, .. } | Frame::Tag { start, .. }) => *start,
        None => at,
    }
}

/// One item fewer to come; whether the frame is now whole.
fn count_down(left: &mut Option<u64>) -> bool {
    match left {
        Some(n) => {
            *n -= 1;
            *n == 0
        }
        None => false,
    }
}

fn map(keys: Vec<Rc<Value>>, values: Vec<Rc<Value>>, offsets: &[usize]) -> Result<Value, DecodeError> {
    Map::new(keys, values)
        .map(Value::Map)
        .map_err(|repeated| DecodeError { failure: Failure::DuplicateKey, offset: offsets[repeated] })
}

/// The value tag `number` makes of `content`: bignums (2, 3) and rationals (30) are read, other tags kept.
pub fn tag(number: u64, content: Rc<Value>) -> Result<Value, Failure> {
    match number {
        2 | 3 => {
            let Value::Bytes(magnitude) = &*content else {
                return Err(Failure::InvalidTag(number, "a bignum's content must be a byte string"));
            };
            let magnitude = BigInt::from_bytes_be(Sign::Plus, magnitude);
            Ok(integer(if number == 2 { magnitude } else { -BigInt::one() - magnitude }))
        }
        30 => {
            const NEEDS: &str = "a rational is an array of an integer and a positive integer";
            let Value::Array(parts) = &*content else { return Err(Failure::InvalidTag(30, NEEDS)) };
            let [numerator, denominator] = parts.as_slice() else { return Err(Failure::InvalidTag(30, NEEDS)) };
            let (Some(numerator), Some(denominator)) = (as_bigint(numerator), as_bigint(denominator)) else {
                return Err(Failure::InvalidTag(30, NEEDS));
            };
            if !denominator.is_positive() {
                return Err(Failure::InvalidTag(30, NEEDS));
            }
            Ok(rational(numerator, denominator).expect("a positive denominator"))
        }
        _ => Ok(Value::Tag(number, content)),
    }
}

/// The integer a value is, if it is one.
pub fn as_bigint(value: &Value) -> Option<BigInt> {
    match value {
        Value::Integer(n) => Some(BigInt::from(*n)),
        Value::Big(n) => Some(n.clone()),
        _ => None,
    }
}

/// A half-precision float (RFC 8949 Appendix D).
fn half(bits: u16) -> f64 {
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f64::from(bits & 0x3ff);
    let magnitude = match exponent {
        0 => mantissa * 2f64.powi(-24),
        31 if mantissa == 0.0 => f64::INFINITY,
        31 => f64::NAN,
        _ => (mantissa + 1024.0) * 2f64.powi(exponent - 25),
    };
    if bits & 0x8000 != 0 { -magnitude } else { magnitude }
}

// ---- encoding ----

/// The head of major type `major` with argument `n`, in its shortest form.
fn head(out: &mut Vec<u8>, major: u8, n: u64) {
    let major = major << 5;
    match n {
        0..=23 => out.push(major | n as u8),
        24..=0xff => out.extend_from_slice(&[major | 24, n as u8]),
        0x100..=0xffff => {
            out.push(major | 25);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(major | 26);
            out.extend_from_slice(&(n as u32).to_be_bytes());
        }
        _ => {
            out.push(major | 27);
            out.extend_from_slice(&n.to_be_bytes());
        }
    }
}

/// The half-precision bits of `value` when a half holds it exactly.
fn exact_half(value: f64) -> Option<u16> {
    let bits = value.to_bits();
    let sign = ((bits >> 48) & 0x8000) as u16;
    let exponent = ((bits >> 52) & 0x7ff) as i64;
    let fraction = bits & ((1 << 52) - 1);
    if exponent == 0x7ff {
        // Every NaN is the quiet NaN without a sign (RFC 8949 4.2.2).
        return Some(if fraction == 0 { sign | 0x7c00 } else { 0x7e00 });
    }
    if exponent == 0 {
        // Zero; any binary64 subnormal is far below the smallest half.
        return (fraction == 0).then_some(sign);
    }
    let power = exponent - 1023;
    if (-14..=15).contains(&power) {
        // A normal half: ten bits of fraction.
        return (fraction & ((1 << 42) - 1) == 0).then(|| sign | (((power + 15) as u16) << 10) | (fraction >> 42) as u16);
    }
    if (-24..-14).contains(&power) {
        // A subnormal half: the value is m × 2⁻²⁴, m < 1024.
        let significand = fraction | (1 << 52);
        let shift = 28 - power;
        return (significand & ((1 << shift) - 1) == 0).then(|| sign | (significand >> shift) as u16);
    }
    None
}

/// A float in the shortest width that keeps its value; NaN as the half-precision quiet NaN.
fn float(out: &mut Vec<u8>, value: f64) {
    if let Some(half) = exact_half(value) {
        out.push(0xf9);
        out.extend_from_slice(&half.to_be_bytes());
    } else if f64::from(value as f32) == value {
        out.push(0xfa);
        out.extend_from_slice(&(value as f32).to_bits().to_be_bytes());
    } else {
        out.push(0xfb);
        out.extend_from_slice(&value.to_bits().to_be_bytes());
    }
}

fn int(out: &mut Vec<u8>, n: i128) {
    if n >= 0 {
        head(out, 0, n as u64);
    } else {
        head(out, 1, (-1 - n) as u64);
    }
}

fn big(out: &mut Vec<u8>, n: &BigInt) {
    let (tag, magnitude) = if n.is_negative() { (3, -BigInt::one() - n) } else { (2, n.clone()) };
    let (_, bytes) = magnitude.to_bytes_be();
    head(out, 6, tag);
    head(out, 2, bytes.len() as u64);
    out.extend_from_slice(&bytes);
}

fn whole(out: &mut Vec<u8>, n: &BigInt) {
    match integer(n.clone()) {
        Value::Integer(small) => int(out, small),
        _ => big(out, n),
    }
}

/// What is left to write: a value, or bytes already encoded (a deterministic map's key).
enum Pending<'a> {
    Value(&'a Value),
    Raw(Vec<u8>),
}

/// The encoding of `value`: preferred serialization (RFC 8949 §4.1), or with `deterministic` the core
/// deterministic encoding (§4.2.1), whose map keys are sorted by their encoded bytes. Definite lengths only.
pub fn encode(value: &Value, deterministic: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pending = vec![Pending::Value(value)];
    while let Some(next) = pending.pop() {
        let value = match next {
            Pending::Raw(bytes) => {
                out.extend_from_slice(&bytes);
                continue;
            }
            Pending::Value(value) => value,
        };
        match value {
            Value::Integer(n) => int(&mut out, *n),
            Value::Big(n) => big(&mut out, n),
            Value::Float(f) => float(&mut out, *f),
            Value::Rational(numerator, denominator) => {
                head(&mut out, 6, 30);
                head(&mut out, 4, 2);
                whole(&mut out, numerator);
                whole(&mut out, denominator);
            }
            Value::Text(text) => {
                head(&mut out, 3, text.len() as u64);
                out.extend_from_slice(text.as_bytes());
            }
            Value::Bytes(bytes) => {
                head(&mut out, 2, bytes.len() as u64);
                out.extend_from_slice(bytes);
            }
            Value::Bool(b) => out.push(if *b { 0xf5 } else { 0xf4 }),
            Value::Null => out.push(0xf6),
            Value::Undefined => out.push(0xf7),
            Value::Simple(n) if *n < 24 => out.push(0xe0 | n),
            Value::Simple(n) => out.extend_from_slice(&[0xf8, *n]),
            Value::Array(items) => {
                head(&mut out, 4, items.len() as u64);
                pending.extend(items.iter().rev().map(|item| Pending::Value(item)));
            }
            Value::Map(map) => {
                head(&mut out, 5, map.len() as u64);
                if deterministic {
                    let mut entries: Vec<(Vec<u8>, &Value)> =
                        map.keys.iter().zip(&map.values).map(|(k, v)| (encode(k, true), &**v)).collect();
                    entries.sort_by(|a, b| a.0.cmp(&b.0));
                    for (key, value) in entries.into_iter().rev() {
                        pending.push(Pending::Value(value));
                        pending.push(Pending::Raw(key));
                    }
                } else {
                    for (key, value) in map.keys.iter().zip(&map.values).rev() {
                        pending.push(Pending::Value(value));
                        pending.push(Pending::Value(key));
                    }
                }
            }
            Value::Tag(number, content) => {
                head(&mut out, 6, *number);
                pending.push(Pending::Value(content));
            }
        }
    }
    out
}

// ---- diagnostic notation ----

/// Diagnostic notation (RFC 8949 §8) of a value, as Appendix A writes it: floats with a point or an exponent
/// (`1.0`, `1.0e+300`, `Infinity`), bytes as `h'…'`, text as a JSON string with `"`, `\` and controls escaped.
pub fn diagnostic(value: &Value) -> String {
    enum Part<'a> {
        Value(&'a Value),
        Text(&'static str),
    }
    let mut out = String::new();
    let mut pending = vec![Part::Value(value)];
    while let Some(next) = pending.pop() {
        let value = match next {
            Part::Text(text) => {
                out.push_str(text);
                continue;
            }
            Part::Value(value) => value,
        };
        let mut parts: Vec<Part> = Vec::new();
        match value {
            Value::Integer(n) => out.push_str(&n.to_string()),
            Value::Big(n) => out.push_str(&n.to_string()),
            Value::Float(f) => out.push_str(&float_text(*f)),
            Value::Rational(numerator, denominator) => out.push_str(&format!("30([{numerator}, {denominator}])")),
            Value::Text(text) => quote(&mut out, text),
            Value::Bytes(bytes) => {
                out.push_str("h'");
                for byte in bytes {
                    out.push_str(&format!("{byte:02x}"));
                }
                out.push('\'');
            }
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Null => out.push_str("null"),
            Value::Undefined => out.push_str("undefined"),
            Value::Simple(n) => out.push_str(&format!("simple({n})")),
            Value::Array(items) => {
                out.push('[');
                for (at, item) in items.iter().enumerate() {
                    if at > 0 {
                        parts.push(Part::Text(", "));
                    }
                    parts.push(Part::Value(item));
                }
                parts.push(Part::Text("]"));
            }
            Value::Map(map) => {
                out.push('{');
                for (at, (key, value)) in map.keys.iter().zip(&map.values).enumerate() {
                    if at > 0 {
                        parts.push(Part::Text(", "));
                    }
                    parts.push(Part::Value(key));
                    parts.push(Part::Text(": "));
                    parts.push(Part::Value(value));
                }
                parts.push(Part::Text("}"));
            }
            Value::Tag(number, content) => {
                out.push_str(&format!("{number}("));
                parts.push(Part::Value(content));
                parts.push(Part::Text(")"));
            }
        }
        pending.extend(parts.into_iter().rev());
    }
    out
}

/// A float's shortest round-trip digits, positional for exponents −6..=20 as JavaScript writes numbers, with
/// `.0` where it would be an integer and a signed exponent otherwise: `1.0`, `100000.0`, `1.0e+300`, `5.960464477539063e-8`.
pub fn float_text(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').expect("{:e} writes an exponent");
    let exponent: i32 = exponent.parse().expect("a decimal exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let sign = if value.is_sign_negative() { "-" } else { "" };
    if !(-6..=20).contains(&exponent) {
        let mantissa = if mantissa.contains('.') { mantissa.to_string() } else { format!("{mantissa}.0") };
        let exponent_sign = if exponent < 0 { "-" } else { "+" };
        return format!("{sign}{mantissa}e{exponent_sign}{}", exponent.abs());
    }
    let point = exponent + 1;
    let text = if point <= 0 {
        format!("0.{}{digits}", "0".repeat((-point) as usize))
    } else if point as usize >= digits.len() {
        format!("{digits}{}.0", "0".repeat(point as usize - digits.len()))
    } else {
        let (whole, fraction) = digits.split_at(point as usize);
        format!("{whole}.{fraction}")
    };
    format!("{sign}{text}")
}

fn quote(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}
