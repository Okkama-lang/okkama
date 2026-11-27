//! The values that cross into a worker (OKW2 transfer values, #586): encoded by the static types of the entry's
//! parameters and result, decoded into another runtime. Process workers send them through a pipe (`worker.rs`); a
//! build without the host emulates workers in the same instance (`worker_emulated.rs`, #1127) and sends them
//! through memory, so what crosses, and what is refused, is the same.
use crate::heap::Integer;
use crate::ops::MapValue;
use crate::program::Kind;
use crate::runtime::Runtime;
use crate::word::Word;
use okkama_sparrow::{Type, TypeRef};

pub(crate) const NOT_TRANSFERABLE: &str = "spawn cannot transfer objects, closures, Futures or native handles";

pub(crate) fn text(out: &mut Vec<u8>, value: &str) -> Result<(), String> {
    count(out, value.len())?;
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

pub(crate) fn count(out: &mut Vec<u8>, value: usize) -> Result<(), String> {
    let value = u32::try_from(value).map_err(|_| "spawn value exceeds wire length".to_owned())?;
    out.extend_from_slice(&value.to_be_bytes());
    Ok(())
}

/// Append `value` of static type `ty`.
pub(crate) fn encode(runtime: &Runtime, out: &mut Vec<u8>, value: Word, ty: TypeRef) -> Result<(), String> {
    // One frame per open container (#452): scratch is bounded by depth, and
    // a map is read through its iterator instead of a copy of its entries.
    enum Frame<'a> {
        Items(std::borrow::Cow<'a, [Word]>, usize, TypeRef),
        Entries(crate::maps::Iter<'a>, TypeRef, TypeRef, Option<Word>),
    }
    let program = &runtime.loaded.program;
    let mut frames: Vec<Frame> = Vec::new();
    let mut next = Some((value, ty));
    loop {
        let (value, ty) = match next.take() {
            Some(item) => item,
            None => match frames.last_mut() {
                None => break,
                Some(Frame::Items(elements, at, element)) => match elements.get(*at) {
                    Some(&value) => {
                        *at += 1;
                        (value, *element)
                    }
                    None => {
                        frames.pop();
                        continue;
                    }
                },
                Some(Frame::Entries(entries, key, item, pending)) => match pending.take() {
                    Some(value) => (value, *item),
                    None => match entries.next() {
                        Some((_, &(k, v))) => {
                            *pending = Some(v);
                            (k, *key)
                        }
                        None => {
                            frames.pop();
                            continue;
                        }
                    },
                },
            },
        };
        match program.ty(ty) {
            Some(Type::Int) => {
                out.push(1);
                let digits = match runtime.heap.integer(value) {
                    Integer::Small(n) => n.to_string(),
                    Integer::Big(n) => n.to_string(),
                };
                text(out, &digits)?;
            }
            Some(Type::Float) => {
                out.push(2);
                out.extend_from_slice(&value.to_be_bytes());
            }
            Some(Type::Str) => {
                out.push(3);
                text(out, runtime.heap.text(value))?;
            }
            Some(Type::Char) => {
                out.push(4);
                out.extend_from_slice(&(value as u32).to_be_bytes());
            }
            Some(Type::Byte) => {
                out.push(5);
                out.push(value as u8);
            }
            Some(Type::Bool) => {
                out.push(6);
                out.push(u8::from(value != 0));
            }
            Some(Type::Array(element)) => {
                let elements = runtime.heap.elements(value);
                out.push(7);
                count(out, elements.len())?;
                frames.push(Frame::Items(elements, 0, *element));
            }
            Some(Type::Map(key, item)) => {
                let map = runtime.heap.payload::<MapValue>(value).ok_or_else(|| NOT_TRANSFERABLE.to_owned())?;
                out.push(8);
                count(out, map.entries.len())?;
                frames.push(Frame::Entries(map.entries.iter(), *key, *item, None));
            }
            _ => return Err(NOT_TRANSFERABLE.into()),
        }
    }
    Ok(())
}

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }
    pub(crate) fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(count)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| "truncated spawn value".to_owned())?;
        let value = &self.bytes[self.at..end];
        self.at = end;
        Ok(value)
    }
    pub(crate) fn count(&mut self) -> Result<usize, String> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().expect("four bytes")) as usize)
    }
    pub(crate) fn text(&mut self) -> Result<&'a str, String> {
        let count = self.count()?;
        std::str::from_utf8(self.take(count)?).map_err(|_| "invalid spawn text".to_owned())
    }
    pub(crate) fn finish(&self) -> Result<(), String> {
        if self.at == self.bytes.len() { Ok(()) } else { Err("trailing spawn data".into()) }
    }

    /// One value of static type `ty`; its tags must match the type.
    pub(crate) fn value(&mut self, runtime: &mut Runtime, ty: TypeRef) -> Result<Word, String> {
        struct Container {
            ty: TypeRef,
            length: usize,
            values: Vec<Word>,
        }
        let mismatch = || "spawn value does not match the declared type".to_owned();
        let mut stack: Vec<Container> = Vec::new();
        let mut expected = ty;
        loop {
            let tag = self.take(1)?[0];
            let program = &runtime.loaded.program;
            let declared = program.ty(expected).cloned();
            let mut value = match (tag, declared) {
                (1, Some(Type::Int)) => {
                    let text = self.text()?;
                    let digits = text.strip_prefix('-').unwrap_or(text);
                    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                        return Err("invalid spawn integer".into());
                    }
                    let n = num_bigint::BigInt::parse_bytes(text.as_bytes(), 10)
                        .ok_or_else(|| "invalid spawn integer".to_owned())?;
                    runtime.heap.big(n)
                }
                (2, Some(Type::Float)) => {
                    let bits = u64::from_be_bytes(self.take(8)?.try_into().expect("eight bytes"));
                    if !f64::from_bits(bits).is_finite() {
                        return Err("invalid spawn float".into());
                    }
                    bits
                }
                (3, Some(Type::Str)) => {
                    let text = self.text()?;
                    runtime.heap.allocate_text(text)
                }
                (4, Some(Type::Char)) => {
                    let bits = self.count()? as u32;
                    char::from_u32(bits).ok_or_else(|| "invalid spawn character".to_owned())?;
                    u64::from(bits)
                }
                (5, Some(Type::Byte)) => u64::from(self.take(1)?[0]),
                (6, Some(Type::Bool)) => match self.take(1)?[0] {
                    flag @ (0 | 1) => u64::from(flag),
                    _ => return Err("invalid spawn boolean".into()),
                },
                (7, Some(Type::Array(_))) | (8, Some(Type::Map(..))) => {
                    let pairs = if tag == 8 { 2 } else { 1 };
                    // Every value takes at least one byte: reject lengths the
                    // remaining input cannot hold before reserving anything.
                    let length = self
                        .count()?
                        .checked_mul(pairs)
                        .filter(|&n| n <= self.bytes.len() - self.at)
                        .ok_or_else(|| "truncated spawn container".to_owned())?;
                    if length > 0 {
                        stack.push(Container { ty: expected, length, values: Vec::new() });
                        expected = element_type(runtime, expected, 0);
                        continue;
                    }
                    build(runtime, expected, &[])
                }
                (0..=8, _) => return Err(mismatch()),
                _ => return Err(format!("invalid spawn value tag {tag}")),
            };
            loop {
                let Some(container) = stack.last_mut() else {
                    return Ok(value);
                };
                container.values.push(value);
                if container.values.len() < container.length {
                    expected = element_type(runtime, container.ty, container.values.len());
                    break;
                }
                let container = stack.pop().expect("completed container");
                value = build(runtime, container.ty, &container.values);
            }
        }
    }
}

/// The type of the `index`th value inside a container of type `ty` (maps
/// alternate keys and values).
fn element_type(runtime: &Runtime, ty: TypeRef, index: usize) -> TypeRef {
    match runtime.loaded.program.ty(ty) {
        Some(Type::Array(element)) => *element,
        Some(Type::Map(key, item)) => {
            if index.is_multiple_of(2) {
                *key
            } else {
                *item
            }
        }
        _ => unreachable!("containers are arrays or maps"),
    }
}

fn build(runtime: &mut Runtime, ty: TypeRef, values: &[Word]) -> Word {
    let program = &runtime.loaded.program;
    match program.ty(ty).cloned() {
        Some(Type::Array(element)) => match Kind::of(program, element) {
            Kind::Byte => {
                let bytes: Vec<u8> = values.iter().map(|&b| b as u8).collect();
                runtime.heap.allocate_bytes(&bytes)
            }
            kind => runtime.heap.allocate_array(kind.repr(), values),
        },
        Some(Type::Map(key, item)) => {
            let key_kind = Kind::of(program, key);
            let key_type = program.ty(key).cloned();
            let mut map = MapValue {
                key: key_kind.repr(),
                value: Kind::of(program, item).repr(),
                entries: Default::default(),
                owned: Default::default(),
            };
            for pair in values.chunks_exact(2) {
                let k = runtime.key(pair[0], key_kind, key_type.as_ref());
                map.entries.insert(k, (pair[0], pair[1]));
            }
            runtime.heap.allocate_payload(Box::new(map))
        }
        _ => unreachable!("containers are arrays or maps"),
    }
}

