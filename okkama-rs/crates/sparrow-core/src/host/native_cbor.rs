//! `runtime/cbor` (#1082): CBOR values as leaf handles over `crate::cbor`'s immutable trees. A handle holds an
//! `Rc` of a value (or of a decode), so building a container shares its children and the collector sees no edges.
use crate::cbor::{self, DecodeError, Limits, Map, Value};
use crate::host::*;
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use okkama_runtime_abi::{Native, NativeFailure};
use std::rc::Rc;

/// A decode: its value and the offset past it, or its failure.
#[derive(Debug)]
pub(crate) struct Decoded {
    outcome: Result<(Rc<Value>, usize), DecodeError>,
}

#[derive(Debug, Clone)]
pub(crate) enum CborHandle {
    Node(Rc<Value>),
    Decode(Rc<Decoded>),
}

pub(crate) fn handles(native: Native) -> bool {
    native.symbol().starts_with("runtime/cbor@1::")
}

fn failure(native: Native, code: &str, message: &str) -> ProviderError {
    ProviderError::Native(NativeFailure {
        domain: "core.cbor".into(),
        code: code.into(),
        operation: native.symbol().into(),
        message: message.into(),
    })
}

/// The kind code `runtime/cbor` documents.
fn kind(value: &Value) -> i64 {
    match value {
        Value::Integer(_) | Value::Big(_) => 0,
        Value::Float(_) => 1,
        Value::Rational(..) => 2,
        Value::Text(_) => 3,
        Value::Bytes(_) => 4,
        Value::Bool(_) => 5,
        Value::Null => 6,
        Value::Undefined => 7,
        Value::Simple(_) => 8,
        Value::Array(_) => 9,
        Value::Map(_) => 10,
        Value::Tag(..) => 11,
    }
}

/// Rust memory of a value made in code (its own cell and payload, not its shared children), charged to the
/// collector: the handles are small cells, the trees are not.
fn footprint(value: &Value) -> usize {
    std::mem::size_of::<Value>()
        + match value {
            Value::Text(text) => text.len(),
            Value::Bytes(bytes) => bytes.len(),
            Value::Array(items) => items.len() * 8,
            Value::Map(map) => map.len() * 16,
            Value::Big(n) | Value::Rational(n, _) => (n.bits() / 8) as usize,
            _ => 0,
        }
}

impl Heap {
    pub(crate) fn native_cbor(&mut self, native: Native, args: &[Slot]) -> Result<Outcome, Error> {
        match self.cbor_invoke(native, args) {
            Ok(value) => Ok(Outcome::Value(value)),
            Err(ProviderError::Engine(error)) => Err(error),
            Err(ProviderError::Native(failure)) => {
                let payload = self.native_failure_new(failure)?;
                self.failure_new(crate::host::FailureKind::Native(payload)).map(Outcome::Failure)
            }
        }
    }

    fn cbor_handle(&self, value: Slot) -> Result<CborHandle, Error> {
        if value.kind() != HANDLE {
            return Err(Error("expected native CBOR handle".into()));
        }
        match self.handles.get(value.bits)? {
            Handle::Cbor(state) => Ok(state.clone()),
            _ => Err(Error("expected native CBOR handle".into())),
        }
    }
    fn cbor_node(&self, value: Slot) -> Result<Rc<Value>, Error> {
        match self.cbor_handle(value)? {
            CborHandle::Node(node) => Ok(node),
            CborHandle::Decode(_) => Err(Error("expected a CborNodeState".into())),
        }
    }
    fn cbor_decoded(&self, value: Slot) -> Result<Rc<Decoded>, Error> {
        match self.cbor_handle(value)? {
            CborHandle::Decode(decoded) => Ok(decoded),
            CborHandle::Node(_) => Err(Error("expected a CborParseState".into())),
        }
    }
    fn cbor_nodes(&self, value: Slot) -> Result<Vec<Rc<Value>>, Error> {
        self.elements(value)?.into_iter().map(|slot| self.cbor_node(slot)).collect()
    }
    fn cbor_new(&mut self, value: Value) -> Result<Slot, Error> {
        self.core_mut().charge_external(footprint(&value));
        self.cbor_value(Rc::new(value))
    }
    fn cbor_value(&mut self, value: Rc<Value>) -> Result<Slot, Error> {
        Ok(Slot { tag: HANDLE, bits: self.handles.allocate(Handle::Cbor(CborHandle::Node(value)))? })
    }
    fn cbor_array(&mut self, values: &[Rc<Value>]) -> Result<Slot, Error> {
        let slots = values.iter().map(|value| self.cbor_value(value.clone())).collect::<Result<Vec<_>, _>>()?;
        self.allocate_array(slots)
    }

    fn cbor_invoke(&mut self, native: Native, args: &[Slot]) -> Result<Slot, ProviderError> {
        use Native::*;
        let invalid = |message| failure(native, "invalid_input", message);
        let index = |heap: &Heap, value: Slot, len: usize, message| -> Result<usize, ProviderError> {
            heap.int64(value)?.and_then(|n| usize::try_from(n).ok()).filter(|n| *n < len).ok_or_else(|| invalid(message))
        };
        let limit = |heap: &Heap, value: Slot| -> Result<usize, ProviderError> {
            let n = heap.bigint(value)?;
            if n.sign() != num_bigint::Sign::Plus {
                return Err(invalid("a CBOR decode limit must be positive"));
            }
            Ok(n.to_usize().unwrap_or(usize::MAX))
        };
        let value = match native {
            CborCborDecode => {
                let limits = Limits { depth: limit(self, args[3])?, items: limit(self, args[4])?, size: limit(self, args[5])? };
                let bytes = self.byte_slice(args[0])?;
                let start = index(self, args[1], bytes.len() + 1, "a CBOR decode starts inside its bytes")?;
                let rest = &bytes[start..];
                let outcome = if args[2].bits != 0 {
                    cbor::decode(rest, limits).map(|value| (value, rest.len()))
                } else {
                    cbor::decode_first(rest, limits)
                };
                // The tree outlives the call: about three times its encoding.
                let charge = outcome.as_ref().map_or(0, |(_, used)| used * 3 + std::mem::size_of::<Value>());
                let outcome = outcome
                    .map(|(value, used)| (value, start + used))
                    .map_err(|error| DecodeError { offset: start + error.offset, ..error });
                self.core_mut().charge_external(charge);
                let decoded = Rc::new(Decoded { outcome });
                Slot { tag: HANDLE, bits: self.handles.allocate(Handle::Cbor(CborHandle::Decode(decoded)))? }
            }
            CborCborDecodeFailure | CborCborDecodeOffset | CborCborDecodeReason | CborCborDecodeNumber
            | CborCborDecodeEnd | CborCborDecodeRoot => {
                let decoded = self.cbor_decoded(args[0])?;
                match (native, &decoded.outcome) {
                    (CborCborDecodeFailure, Ok(_)) => self.allocate_text(String::new())?,
                    (CborCborDecodeFailure, Err(error)) => self.allocate_text(error.failure.code().into())?,
                    (CborCborDecodeOffset, Ok(_)) => Slot::small(0),
                    (CborCborDecodeOffset, Err(error)) => Slot::small(error.offset as i64),
                    (CborCborDecodeReason, Ok(_)) => self.allocate_text(String::new())?,
                    (CborCborDecodeReason, Err(error)) => self.allocate_text(error.failure.reason().into())?,
                    (CborCborDecodeNumber, Ok(_)) => Slot::small(0),
                    (CborCborDecodeNumber, Err(error)) => self.allocate(BigInt::from(error.failure.number()))?,
                    (CborCborDecodeEnd, Ok((_, end))) => Slot::small(*end as i64),
                    (CborCborDecodeEnd, Err(_)) => Slot::small(0),
                    (CborCborDecodeRoot, Ok((root, _))) => self.cbor_value(root.clone())?,
                    _ => return Err(failure(native, "invalid_data", "a failed CBOR decode has no value")),
                }
            }
            CborCborKind => Slot::small(kind(&*self.cbor_node(args[0])?)),
            CborCborInt => match cbor::as_bigint(&*self.cbor_node(args[0])?) {
                Some(n) => self.allocate(n)?,
                None => return Err(invalid("CBOR value is not an integer")),
            },
            CborCborFloat => match *self.cbor_node(args[0])? {
                Value::Float(f) if f.is_finite() => Slot { tag: FLOAT, bits: f.to_bits() },
                Value::Float(_) => return Err(invalid("CBOR float is not finite")),
                _ => return Err(invalid("CBOR value is not a float")),
            },
            CborCborNumerator | CborCborDenominator => match &*self.cbor_node(args[0])? {
                Value::Rational(n, d) => self.allocate(if native == CborCborNumerator { n.clone() } else { d.clone() })?,
                _ => return Err(invalid("CBOR value is not a rational")),
            },
            CborCborText => match &*self.cbor_node(args[0])? {
                Value::Text(text) => self.allocate_text(text.clone())?,
                _ => return Err(invalid("CBOR value is not a text string")),
            },
            CborCborBytes => match &*self.cbor_node(args[0])? {
                Value::Bytes(bytes) => self.allocate_bytes(bytes)?,
                _ => return Err(invalid("CBOR value is not a byte string")),
            },
            CborCborBoolean => match *self.cbor_node(args[0])? {
                Value::Bool(b) => Slot::boolean(b),
                _ => return Err(invalid("CBOR value is not a boolean")),
            },
            CborCborNumber => match *self.cbor_node(args[0])? {
                Value::Simple(n) => Slot::small(i64::from(n)),
                Value::Tag(n, _) => self.allocate(BigInt::from(n))?,
                _ => return Err(invalid("CBOR value is not a tag or a simple value")),
            },
            CborCborContent => match &*self.cbor_node(args[0])? {
                Value::Tag(_, content) => self.cbor_value(content.clone())?,
                _ => return Err(invalid("CBOR value is not a tag")),
            },
            CborCborSize => match &*self.cbor_node(args[0])? {
                Value::Array(items) => Slot::small(items.len() as i64),
                Value::Map(map) => Slot::small(map.len() as i64),
                _ => return Err(invalid("CBOR value is not an array or a map")),
            },
            CborCborItem => {
                let node = self.cbor_node(args[0])?;
                let Value::Array(items) = &*node else { return Err(invalid("CBOR value is not an array")) };
                let at = index(self, args[1], items.len(), "CBOR array index is outside the array")?;
                self.cbor_value(items[at].clone())?
            }
            CborCborItems => {
                let node = self.cbor_node(args[0])?;
                let Value::Array(items) = &*node else { return Err(invalid("CBOR value is not an array")) };
                self.cbor_array(items)?
            }
            CborCborKeys | CborCborValues => {
                let node = self.cbor_node(args[0])?;
                let Value::Map(map) = &*node else { return Err(invalid("CBOR value is not a map")) };
                self.cbor_array(if native == CborCborKeys { map.keys() } else { map.values() })?
            }
            CborCborTextPosition | CborCborIntPosition | CborCborPosition => {
                let node = self.cbor_node(args[0])?;
                let Value::Map(map) = &*node else { return Err(invalid("CBOR value is not a map")) };
                let position = match native {
                    CborCborTextPosition => map.text_position(self.text(args[1])?),
                    CborCborIntPosition => map.position(&cbor::integer(self.bigint(args[1])?.into_owned())),
                    _ => map.position(&*self.cbor_node(args[1])?),
                };
                Slot::small(position.map_or(-1, |at| at as i64))
            }
            CborCborValue => {
                let node = self.cbor_node(args[0])?;
                let Value::Map(map) = &*node else { return Err(invalid("CBOR value is not a map")) };
                let at = index(self, args[1], map.len(), "CBOR map position is outside the map")?;
                self.cbor_value(map.values()[at].clone())?
            }
            CborCborIntNode => {
                let n = self.bigint(args[0])?.into_owned();
                self.cbor_new(cbor::integer(n))?
            }
            CborCborFloatNode => self.cbor_new(Value::Float(f64::from_bits(args[0].bits)))?,
            CborCborRationalNode => {
                let (n, d) = (self.bigint(args[0])?.into_owned(), self.bigint(args[1])?.into_owned());
                let value = cbor::rational(n, d).ok_or_else(|| invalid("a rational's denominator is zero"))?;
                self.cbor_new(value)?
            }
            CborCborTextNode => {
                let text = self.text(args[0])?.to_string();
                self.cbor_new(Value::Text(text))?
            }
            CborCborBytesNode => {
                let bytes = self.byte_slice(args[0])?.into_owned();
                self.cbor_new(Value::Bytes(bytes))?
            }
            CborCborBooleanNode => self.cbor_new(Value::Bool(args[0].bits != 0))?,
            CborCborNullNode => self.cbor_new(Value::Null)?,
            CborCborTagNode => {
                let number = self.bigint(args[0])?.to_u64().ok_or_else(|| invalid("a tag number is 0..2^64-1"))?;
                let content = self.cbor_node(args[1])?;
                let value = cbor::tag(number, content).map_err(|failure| invalid(failure.reason()))?;
                self.cbor_new(value)?
            }
            CborCborArrayNode => {
                let items = self.cbor_nodes(args[0])?;
                self.cbor_new(Value::Array(items))?
            }
            CborCborRepeatedKey => {
                let keys = self.cbor_nodes(args[0])?;
                Slot::small(cbor::first_repeated(&keys).unwrap_or(keys.len()) as i64)
            }
            CborCborMapNode => {
                let (keys, values) = (self.cbor_nodes(args[0])?, self.cbor_nodes(args[1])?);
                if keys.len() != values.len() {
                    return Err(invalid("CBOR map keys and values differ in number"));
                }
                let map = Map::new(keys, values).map_err(|_| invalid("repeated CBOR map key"))?;
                self.cbor_new(Value::Map(map))?
            }
            CborCborEncode => {
                let bytes = cbor::encode(&*self.cbor_node(args[0])?, args[1].bits != 0);
                self.allocate_bytes(&bytes)?
            }
            CborCborDiagnostic => {
                let text = cbor::diagnostic(&*self.cbor_node(args[0])?);
                self.allocate_text(text)?
            }
            _ => return Err(ProviderError::Engine(Error("unsupported CBOR native".into()))),
        };
        Ok(value)
    }
}
