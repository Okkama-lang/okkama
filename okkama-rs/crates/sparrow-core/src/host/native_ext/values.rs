//! Checked value conversion; only validated call-table entries reach the heap.
use super::*;
use num_bigint::{BigInt, Sign};
use num_traits::ToPrimitive;
use crate::host as h;
use crate::ops::{Key, MapValue, Rational};
use okkama_runtime_core::{IntValue, bignum::Big, rat::Rat};
use std::rc::Rc;

type Result<T> = std::result::Result<T, u32>;

pub(super) fn repr(shape: &Shape) -> Repr {
    match shape {
        Shape::Int => Repr::Int,
        Shape::Unknown | Shape::Bool | Shape::Byte | Shape::Char | Shape::Float => Repr::Raw,
        _ => Repr::Ref,
    }
}

impl Call<'_> {
    pub(super) fn refusal(&self, status: u32) -> Refusal {
        if self.stopped { Refusal::Exhausted(crate::budget::Exhausted::Memory) }
        else if status == OKK_FAILED && self.failure.is_some() { Refusal::Raise(self.failure.unwrap()) }
        else { fault(self.binding, format_args!("value status {status}")) }
    }

    // Call-local storage counts too: it stays alive until the callback returns.
    pub(super) fn reserve(&mut self, bytes: usize) -> Result<()> {
        let total = self.scratch.checked_add(bytes).ok_or(OKK_RANGE)?;
        if self.stopped || unsafe { (*self.host.rt).would_exceed(total as u64) } {
            self.stopped = true;
            return Err(OKK_STOPPED);
        }
        self.scratch = total;
        Ok(())
    }

    pub(super) fn preflight(&mut self, len: usize, width: usize) -> Result<()> {
        if len > u32::MAX as usize { return Err(OKK_RANGE); }
        let bytes = len.checked_mul(width).and_then(|n| n.checked_add(64)).ok_or(OKK_RANGE)?;
        if bytes > isize::MAX as usize { return Err(OKK_RANGE); }
        if unsafe { (*self.host.rt).would_exceed(self.scratch.saturating_add(bytes) as u64) } {
            self.stopped = true;
            return Err(OKK_STOPPED);
        }
        Ok(())
    }

    pub(super) fn entry(&self, token: okk_slot) -> Result<Slot> {
        let index = token.bits as u32 as usize;
        if token.bits >> 32 != self.serial || index >= self.count { return Err(OKK_BAD_TOKEN); }
        let slot = if index < self.inline.len() { self.inline[index] } else { self.overflow[index - self.inline.len()] };
        let kind = match slot.kind() {
            h::BIG => OKK_BIGINT, h::RAT => OKK_RAT, h::STR => OKK_STRING,
            h::ARRAY => OKK_ARRAY, h::MAP => OKK_MAP, h::HANDLE => OKK_STATE, _ => return Err(OKK_BAD_KIND),
        };
        if token.kind != kind { return Err(OKK_BAD_KIND); }
        Ok(slot)
    }

    fn shape(&self, slot: Slot) -> Shape {
        match slot.kind() {
            h::SMALL | h::BIG => Shape::Int,
            h::RAT => Shape::Rat, h::BOOL => Shape::Bool, h::BYTE => Shape::Byte,
            h::CHAR => Shape::Char, h::FLOAT => Shape::Float, h::STR => Shape::Str,
            h::ARRAY => Shape::Array((slot.tag >> 8) as usize),
            h::MAP | h::MAYBE | h::HANDLE => self.host.shapes[(slot.tag >> 8) as usize].clone(),
            _ => Shape::Unknown,
        }
    }

    pub(super) fn export(&mut self, slot: Slot) -> Result<okk_slot> {
        let scalar = |bits, kind| Ok(okk_slot { bits, kind });
        match slot.kind() {
            h::SMALL | h::BIG => {
                if let Some(n) = self.host.int64(slot).map_err(|_| OKK_BAD_KIND)? { scalar(n as u64, OKK_INT) }
                else { self.token(slot, OKK_BIGINT) }
            }
            h::FLOAT => scalar(slot.bits, OKK_FLOAT), h::BOOL => scalar(slot.bits, OKK_BOOL),
            h::BYTE => scalar(slot.bits, OKK_BYTE), h::CHAR => scalar(slot.bits, OKK_CHAR),
            h::STR => self.token(slot, OKK_STRING), h::RAT => self.token(slot, OKK_RAT),
            h::ARRAY => self.token(slot, OKK_ARRAY), h::MAP => self.token(slot, OKK_MAP),
            h::HANDLE => {
                self.external(slot)?;
                self.token(slot, OKK_STATE)
            }
            h::MAYBE => {
                let Shape::Maybe(inner, some, none) = self.shape(slot) else { return Err(OKK_BAD_SHAPE); };
                let descriptor = crate::heap::payload(self.host.core().header_of(slot.bits));
                if descriptor == none { scalar(0, OKK_NONE) }
                else if descriptor == some {
                    let inner = self.host.slot(self.host.core().field(slot.bits, 0), &self.host.shapes[inner]);
                    self.export(inner)
                } else { Err(OKK_BAD_SHAPE) }
            }
            _ => scalar(0, OKK_NONE),
        }
    }

    fn import(&mut self, value: okk_slot) -> Result<Slot> {
        let tag = match value.kind {
            OKK_NONE if value.bits == 0 => return Ok(Slot { bits: crate::word::UNBOUND, tag: h::OBJECT }),
            OKK_INT => return Ok(Slot::small(value.bits as i64)),
            OKK_BOOL if value.bits <= 1 => h::BOOL,
            OKK_BYTE if value.bits <= 255 => h::BYTE,
            OKK_CHAR if u32::try_from(value.bits).ok().and_then(char::from_u32).is_some() => h::CHAR,
            OKK_FLOAT => {
                if !f64::from_bits(value.bits).is_finite() {
                    self.failure("not_finite", "Native float result is not finite")?;
                    return Err(OKK_FAILED);
                }
                h::FLOAT
            }
            OKK_BIGINT | OKK_RAT | OKK_STRING | OKK_ARRAY | OKK_MAP | OKK_STATE => return self.entry(value),
            _ => return Err(OKK_BAD_KIND),
        };
        Ok(Slot { bits: value.bits, tag })
    }

    pub(super) fn import_as(&mut self, value: okk_slot, expected: &Shape, exact: bool) -> Result<Slot> {
        if let Shape::Maybe(inner, some, none) = *expected {
            let (descriptor, fields) = if value.kind == OKK_NONE {
                if value.bits != 0 { return Err(OKK_BAD_KIND); }
                (none, Vec::new())
            } else {
                let shape = self.host.shapes[inner].clone();
                let slot = self.import_as(value, &shape, exact)?;
                (some, vec![self.host.word(slot)])
            };
            self.preflight(fields.len(), 8)?;
            let word = self.host.core_mut().object_new(descriptor, &fields);
            return Ok(self.host.slot(word, expected));
        }
        let kind_matches = matches!((expected, value.kind),
            (Shape::Unknown, OKK_NONE) | (Shape::Int, OKK_INT | OKK_BIGINT) | (Shape::Rat, OKK_RAT)
            | (Shape::Float, OKK_FLOAT) | (Shape::Bool, OKK_BOOL) | (Shape::Byte, OKK_BYTE)
            | (Shape::Char, OKK_CHAR) | (Shape::Str, OKK_STRING) | (Shape::Array(_), OKK_ARRAY)
            | (Shape::Map(..), OKK_MAP) | (Shape::External(..), OKK_STATE));
        if !kind_matches { return Err(OKK_BAD_KIND); }
        let slot = self.import(value)?;
        if exact {
            if &self.shape(slot) == expected { return Ok(slot); }
            if self.empty(slot) && matches!((slot.kind(), expected), (h::ARRAY, Shape::Array(_)) | (h::MAP, Shape::Map(..))) {
                return Ok(self.host.slot(slot.bits, expected));
            }
            return Err(OKK_BAD_SHAPE);
        }
        self.coerce(slot, expected, true)
    }

    fn coerce(&mut self, slot: Slot, expected: &Shape, lift: bool) -> Result<Slot> {
        if &self.shape(slot) == expected { return Ok(slot); }
        if lift && matches!(expected, Shape::Maybe(..)) {
            let token = self.export(slot)?;
            return self.import_as(token, expected, false);
        }
        // Empty containers have no element evidence. A declared shape supplies it.
        match (slot.kind(), expected) {
            (h::ARRAY, Shape::Array(_)) if self.host.core().array_length(slot.bits) == 0 => Ok(self.host.slot(slot.bits, expected)),
            (h::MAP, Shape::Map(..)) if self.map(slot)?.entries.is_empty() => Ok(self.host.slot(slot.bits, expected)),
            (h::ARRAY, Shape::Array(element)) => {
                self.preflight(self.host.core().array_length(slot.bits), 32)?;
                let source = self.host.elements(slot).map_err(|_| OKK_BAD_SHAPE)?;
                let target = self.host.shapes[*element].clone();
                let mut words = Vec::with_capacity(source.len());
                for item in source {
                    let item = self.coerce(item, &target, lift)?;
                    words.push(self.host.word(item));
                }
                let word = self.host.core_mut().allocate_array(repr(&target), &words);
                Ok(self.host.slot(word, expected))
            }
            (h::MAP, Shape::Map(k, v)) => {
                self.preflight(self.map(slot)?.entries.len(), 192)?;
                let source = self.host.map_entries(slot).map_err(|_| OKK_BAD_SHAPE)?;
                let key_shape = self.host.shapes[*k].clone();
                let value_shape = self.host.shapes[*v].clone();
                let mut map = MapValue { key: repr(&key_shape), value: repr(&value_shape), entries: Default::default(), owned: Default::default() };
                for (key, value) in source {
                    let key = self.coerce(key, &key_shape, lift)?;
                    let value = self.coerce(value, &value_shape, lift)?;
                    let order = self.key(key)?;
                    if map.entries.contains_key(&order) { return Err(OKK_RANGE); }
                    map.entries.insert(order, (self.host.word(key), self.host.word(value)));
                }
                let word = self.host.core_mut().allocate_payload(Box::new(map));
                Ok(self.host.slot(word, expected))
            }
            _ => Err(OKK_BAD_SHAPE),
        }
    }

    fn integer(&mut self, value: okk_slot) -> Result<BigInt> {
        if !matches!(value.kind, OKK_INT | OKK_BIGINT) { return Err(OKK_BAD_KIND); }
        let slot = self.import(value)?;
        self.host.bigint(slot).map(|n| n.into_owned()).map_err(|_| OKK_BAD_KIND)
    }

    fn new_integer(&mut self, value: BigInt) -> Result<okk_slot> {
        if let Some(n) = value.to_i64() { return Ok(okk_slot { bits: n as u64, kind: OKK_INT }); }
        self.preflight(value.bits().div_ceil(8) as usize, 1)?;
        let word = self.host.core_mut().big(value);
        self.token(Slot { bits: word, tag: h::BIG }, OKK_BIGINT)
    }

    fn map(&self, slot: Slot) -> Result<&MapValue> {
        if slot.kind() != h::MAP { return Err(OKK_BAD_KIND); }
        self.host.core().payload::<MapValue>(slot.bits).ok_or(OKK_BAD_KIND)
    }

    pub(super) fn byte_slot(&mut self, value: okk_slot) -> Result<Slot> {
        let slot = self.array(value)?;
        let shape = Shape::Array((self.binding.bytes_tag >> 8) as usize);
        self.coerce(slot, &shape, false)
    }

    fn array(&self, value: okk_slot) -> Result<Slot> {
        if value.kind != OKK_ARRAY { return Err(OKK_BAD_KIND); }
        self.entry(value)
    }

    fn array_item(&mut self, array: Slot, index: usize) -> Result<okk_slot> {
        if index >= self.host.core().array_length(array.bits) { return Err(OKK_RANGE); }
        let heap = self.host.core();
        let word = if heap.is_bytes(array.bits) { u64::from(heap.bytes(array.bits)[index]) }
        else if heap.header_of(array.bits) & crate::heap::TRIE != 0 { heap.trie_get(array.bits, index) }
        else { heap.elements(array.bits)[index] };
        let shape = &self.host.shapes[(array.tag >> 8) as usize];
        self.export(self.host.slot(word, shape))
    }

    fn constructed(&mut self, slot: Slot) -> Result<okk_slot> {
        let expected = match self.binding.result {
            Shape::Maybe(inner, ..) => self.host.shapes[inner].clone(),
            ref shape => shape.clone(),
        };
        let slot = if matches!((slot.kind(), &expected), (h::ARRAY, Shape::Array(_)) | (h::MAP, Shape::Map(..))) {
            match self.coerce(slot, &expected, true) {
                Ok(slot) => slot,
                // Intermediate collections need not have the callback's result type.
                Err(OKK_BAD_SHAPE) => slot,
                Err(status) => return Err(status),
            }
        } else { slot };
        self.export(slot)
    }

    fn intern(&mut self, shape: Shape) -> Result<usize> {
        if let Some(index) = self.host.shape_index.get(&shape) { return Ok(*index); }
        self.reserve(3 * std::mem::size_of::<Shape>())?;
        Ok(self.host.intern(shape))
    }

    fn collection_slots(&mut self, values: &[okk_slot]) -> Result<(Vec<Slot>, Shape)> {
        let mut slots = values.iter().map(|s| self.import(*s)).collect::<Result<Vec<_>>>()?;
        if values.iter().any(|s| s.kind == OKK_NONE) {
            // NONE has no type evidence; the program supplies the Maybe descriptor.
            let candidate = self.host.shapes.iter().find(|shape| {
                let Shape::Maybe(inner, ..) = shape else { return false; };
                slots.iter().all(|slot| self.shape(*slot) == Shape::Unknown || self.shape(*slot) == self.host.shapes[*inner])
            }).cloned().ok_or(OKK_BAD_SHAPE)?;
            slots = values.iter().map(|s| self.import_as(*s, &candidate, false)).collect::<Result<Vec<_>>>()?;
            return Ok((slots, candidate));
        }
        let mut shape = Shape::Unknown;
        for slot in &slots {
            if !self.empty(*slot) { shape = self.common_shape(shape, self.shape(*slot))?; }
        }
        if shape == Shape::Unknown { shape = slots.first().map(|s| self.shape(*s)).unwrap_or(shape); }
        for slot in &mut slots { *slot = self.coerce(*slot, &shape, true)?; }
        Ok((slots, shape))
    }

    // Some-only intermediate containers have no Maybe tag. Join their shapes
    // before coercing, so the order of Some and None containers is immaterial.
    fn common_shape(&mut self, a: Shape, b: Shape) -> Result<Shape> {
        if a == b || b == Shape::Unknown { return Ok(a); }
        if a == Shape::Unknown { return Ok(b); }
        match (&a, &b) {
            (Shape::Maybe(inner, ..), _) => {
                let inner = self.host.shapes[*inner].clone();
                let other = match b { Shape::Maybe(i, ..) => self.host.shapes[i].clone(), _ => b };
                if self.common_shape(inner.clone(), other)? == inner { Ok(a) } else { Err(OKK_BAD_SHAPE) }
            }
            (_, Shape::Maybe(..)) => self.common_shape(b, a),
            (Shape::Array(x), Shape::Array(y)) => {
                let inner = self.common_shape(self.host.shapes[*x].clone(), self.host.shapes[*y].clone())?;
                Ok(Shape::Array(self.intern(inner)?))
            }
            (Shape::Map(ak, av), Shape::Map(bk, bv)) => {
                let key = self.common_shape(self.host.shapes[*ak].clone(), self.host.shapes[*bk].clone())?;
                let value = self.common_shape(self.host.shapes[*av].clone(), self.host.shapes[*bv].clone())?;
                Ok(Shape::Map(self.intern(key)?, self.intern(value)?))
            }
            _ => Err(OKK_BAD_SHAPE),
        }
    }

    fn empty(&self, slot: Slot) -> bool {
        match slot.kind() {
            h::ARRAY => self.host.core().array_length(slot.bits) == 0,
            h::MAP => self.map(slot).is_ok_and(|m| m.entries.is_empty()),
            _ => false,
        }
    }

    fn key(&self, slot: Slot) -> Result<Key> {
        Ok(match slot.kind() {
            h::SMALL => Key::Small(slot.bits as i64),
            h::BIG => Key::int(self.host.core().integer(slot.bits)),
            h::STR => Key::text(self.host.core().text(slot.bits).into()),
            h::FLOAT => Key::Float(f64::from_bits(slot.bits)),
            h::RAT => Key::Rat(Rc::new(self.host.core().payload::<Rational>(slot.bits).ok_or(OKK_BAD_KIND)?.0.clone())),
            h::BOOL | h::BYTE | h::CHAR => Key::Raw(slot.bits),
            h::MAYBE => {
                let descriptor = crate::heap::payload(self.host.core().header_of(slot.bits));
                let ty = okkama_sparrow::Type::Object(okkama_sparrow::ObjectRef(descriptor));
                unsafe { (*self.host.rt).checked_key(slot.bits, crate::program::Kind::Ref, Some(&ty)).map_err(|_| OKK_BAD_SHAPE)? }
            }
            _ => return Err(OKK_BAD_SHAPE),
        })
    }
}

macro_rules! host_fn {
    ($name:ident($c:ident: *mut okk_call $(, $arg:ident: $ty:ty)*) $body:block) => {
        pub(super) unsafe extern "C" fn $name($c: *mut okk_call, $($arg: $ty),*) -> u32 {
            unsafe { guarded($c, |$c| {
                let result: Result<()> = (|| { $body })();
                match result { Ok(()) => OKK_OK, Err(s) => s }
            }) }
        }
    };
}

host_fn!(bytes_reserve(c: *mut okk_call, len: usize, data: *mut *mut u8, out: *mut okk_slot) {
    output(data)?; output(out)?;
    c.preflight(len, 2)?;
    let word = c.host.core_mut().allocate_bytes(&vec![0; len]);
    *out = c.token(Slot { bits: word, tag: c.binding.bytes_tag }, OKK_ARRAY)?;
    // A fresh packed array cannot alias any other value until publication.
    *data = (word + 8) as *mut u8;
    Ok(())
});

host_fn!(bigint_read(c: *mut okk_call, value: okk_slot, sign: *mut i32, data: *mut *const u8, len: *mut usize) {
    output(sign)?; output(data)?; output(len)?;
    let integer = c.integer(value)?;
    c.preflight(integer.bits().div_ceil(8) as usize, 1)?;
    let (s, bytes) = integer.to_bytes_be();
    c.reserve(bytes.len().saturating_add(2 * std::mem::size_of::<Vec<u8>>()))?;
    *sign = match s { Sign::Minus => -1, Sign::NoSign => 0, Sign::Plus => 1 };
    *len = bytes.len(); *data = bytes.as_ptr();
    c.views.push(bytes);
    Ok(())
});

host_fn!(bigint_new(c: *mut okk_call, sign: i32, data: *const u8, len: usize, out: *mut okk_slot) {
    output(out)?;
    c.preflight(len, 2)?;
    let bytes = input(data, len)?;
    let sign = match sign { -1 => Sign::Minus, 0 => Sign::NoSign, 1 => Sign::Plus, _ => return Err(OKK_RANGE) };
    if sign == Sign::NoSign && bytes.iter().any(|b| *b != 0) { return Err(OKK_RANGE); }
    *out = c.new_integer(BigInt::from_bytes_be(sign, bytes))?;
    Ok(())
});

host_fn!(rat_parts(c: *mut okk_call, value: okk_slot, numerator: *mut okk_slot, denominator: *mut okk_slot) {
    output(numerator)?; output(denominator)?;
    let slot = c.get(value, OKK_RAT, h::RAT)?;
    let ratio = &c.host.core().payload::<Rational>(slot.bits).ok_or(OKK_BAD_KIND)?.0;
    let n = ratio.numerator().to_big().as_bigint().clone();
    let d = ratio.denominator().to_big().as_bigint().clone();
    *numerator = c.new_integer(n)?; *denominator = c.new_integer(d)?;
    Ok(())
});

host_fn!(rat_new(c: *mut okk_call, numerator: okk_slot, denominator: okk_slot, out: *mut okk_slot) {
    output(out)?;
    let n = c.integer(numerator)?; let d = c.integer(denominator)?;
    c.preflight((n.bits().saturating_add(d.bits())).div_ceil(8) as usize, 4)?;
    let n = IntValue::from_big(Big::from_bigint(n)); let d = IntValue::from_big(Big::from_bigint(d));
    let rat = Rat::new(&n, &d).map_err(|_| OKK_RANGE)?;
    let word = c.host.core_mut().allocate_payload(Box::new(Rational(rat)));
    *out = c.token(Slot { bits: word, tag: h::RAT }, OKK_RAT)?;
    Ok(())
});

host_fn!(array_len(c: *mut okk_call, value: okk_slot, len: *mut usize) {
    output(len)?;
    *len = c.host.core().array_length(c.array(value)?.bits);
    Ok(())
});

host_fn!(array_get(c: *mut okk_call, value: okk_slot, index: usize, out: *mut okk_slot) {
    output(out)?;
    let array = c.array(value)?;
    *out = c.array_item(array, index)?;
    Ok(())
});

host_fn!(array_read(c: *mut okk_call, value: okk_slot, start: usize, out: *mut okk_slot, len: usize) {
    let array = c.array(value)?;
    let size = c.host.core().array_length(array.bits);
    if start > size || len > size - start { return Err(OKK_RANGE); }
    if len > 0 { output(out)?; }
    if len > isize::MAX as usize / std::mem::size_of::<okk_slot>() { return Err(OKK_RANGE); }
    for i in 0..len { *out.add(i) = c.array_item(array, start + i)?; }
    Ok(())
});

host_fn!(array_new(c: *mut okk_call, values: *const okk_slot, len: usize, out: *mut okk_slot) {
    output(out)?; c.preflight(len, 48)?;
    let values = input(values, len)?;
    let (slots, shape) = c.collection_slots(values)?;
    let words = slots.into_iter().map(|s| c.host.word(s)).collect::<Vec<_>>();
    let word = if shape == Shape::Byte {
        c.host.core_mut().allocate_bytes(&words.iter().map(|w| *w as u8).collect::<Vec<_>>())
    } else { c.host.core_mut().allocate_array(repr(&shape), &words) };
    let shape = Shape::Array(c.intern(shape)?);
    *out = c.constructed(c.host.slot(word, &shape))?;
    Ok(())
});

host_fn!(map_len(c: *mut okk_call, value: okk_slot, len: *mut usize) {
    output(len)?;
    *len = c.map(c.entry(value)?)?.entries.len();
    Ok(())
});

host_fn!(map_entries(c: *mut okk_call, value: okk_slot, keys: *mut okk_slot, values: *mut okk_slot, len: usize) {
    let slot = c.entry(value)?;
    if len > c.map(slot)?.entries.len() { return Err(OKK_RANGE); }
    if len > 0 { output(keys)?; output(values)?; }
    c.preflight(len, 32)?;
    let Shape::Map(k, v) = c.shape(slot) else { return Err(OKK_BAD_SHAPE); };
    let entries = c.map(slot)?.entries.values().take(len).copied().collect::<Vec<_>>();
    for (i, (kword, vword)) in entries.into_iter().enumerate() {
        let key = c.host.slot(kword, &c.host.shapes[k]);
        let value = c.host.slot(vword, &c.host.shapes[v]);
        *keys.add(i) = c.export(key)?; *values.add(i) = c.export(value)?;
    }
    Ok(())
});

host_fn!(map_new(c: *mut okk_call, keys: *const okk_slot, values: *const okk_slot, len: usize, out: *mut okk_slot) {
    output(out)?; c.preflight(len, 192)?;
    let (keys, key) = c.collection_slots(input(keys, len)?)?;
    let (values, value) = c.collection_slots(input(values, len)?)?;
    let mut map = MapValue { key: repr(&key), value: repr(&value), entries: Default::default(), owned: Default::default() };
    for (k, v) in keys.into_iter().zip(values) {
        let key = c.key(k)?;
        if map.entries.contains_key(&key) { return Err(OKK_RANGE); }
        let pair = (c.host.word(k), c.host.word(v));
        map.entries.insert(key, pair);
    }
    let word = c.host.core_mut().allocate_payload(Box::new(map));
    let shape = Shape::Map(c.intern(key)?, c.intern(value)?);
    c.intern(shape.clone())?;
    *out = c.constructed(c.host.slot(word, &shape))?;
    Ok(())
});
