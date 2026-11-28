//! The payload data model: one value type for JSON and CBOR messages, so a helper reads a payload the same way
//! whichever codec the call came in.

use std::fmt;

/// A payload value. JSON gives null, booleans, numbers, text, arrays and maps with text keys; CBOR adds byte
/// strings, integers beyond 64 bits (as tags 2 and 3), maps with keys of any kind and tags.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// JSON null; CBOR null or undefined.
    Null,
    /// true or false.
    Bool(bool),
    /// An integer: CBOR major types 0 and 1 cover −2⁶⁴ … 2⁶⁴−1.
    Int(i128),
    /// A float. JSON numbers with a fraction or an exponent arrive as floats.
    Float(f64),
    /// Text.
    Text(String),
    /// A CBOR byte string. In JSON it is written as an array of numbers.
    Bytes(Vec<u8>),
    /// An array.
    Array(Vec<Value>),
    /// A map, in the order it was written. JSON keys are text.
    Map(Vec<(Value, Value)>),
    /// A CBOR tag with its content (bignums are tags 2 and 3 over bytes; rat is tag 30).
    Tag(u64, Box<Value>),
}

impl Value {
    /// The value under a text key of a map, if there is one.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(entries) => entries.iter().find(|(k, _)| matches!(k, Value::Text(text) if text == key)).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The text, if this is text.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(text) => Some(text),
            _ => None,
        }
    }

    /// The integer, if this is an integer that fits an i64.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(n) => i64::try_from(*n).ok(),
            _ => None,
        }
    }

    /// The bytes, if this is a byte string.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// A map of text keys, in the order given.
    pub fn map<K: Into<String>>(entries: impl IntoIterator<Item = (K, Value)>) -> Value {
        Value::Map(entries.into_iter().map(|(k, v)| (Value::Text(k.into()), v)).collect())
    }

    /// The value as serde_json sees it, for typed reading. Bytes become an array of numbers, integer keys their
    /// decimal text and tags their content; an integer outside i64 and u64 or a float that is not finite has no
    /// JSON form.
    pub fn to_json(&self) -> Result<serde_json::Value, ValueError> {
        Ok(match self {
            Value::Null => serde_json::Value::Null,
            Value::Bool(b) => serde_json::Value::Bool(*b),
            Value::Int(n) => {
                if let Ok(small) = i64::try_from(*n) {
                    serde_json::Value::from(small)
                } else if let Ok(large) = u64::try_from(*n) {
                    serde_json::Value::from(large)
                } else {
                    return Err(ValueError(format!("integer {n} has no JSON form")));
                }
            }
            Value::Float(f) => serde_json::Number::from_f64(*f)
                .map(serde_json::Value::Number)
                .ok_or_else(|| ValueError(format!("float {f} has no JSON form")))?,
            Value::Text(text) => serde_json::Value::String(text.clone()),
            Value::Bytes(bytes) => serde_json::Value::Array(bytes.iter().map(|b| serde_json::Value::from(*b)).collect()),
            Value::Array(items) => serde_json::Value::Array(items.iter().map(Value::to_json).collect::<Result<_, _>>()?),
            Value::Map(entries) => {
                let mut object = serde_json::Map::new();
                for (key, value) in entries {
                    let name = match key {
                        Value::Text(text) => text.clone(),
                        Value::Int(n) => n.to_string(),
                        other => return Err(ValueError(format!("map key {other:?} has no JSON form"))),
                    };
                    if object.insert(name.clone(), value.to_json()?).is_some() {
                        return Err(ValueError(format!("map key {name:?} repeated")));
                    }
                }
                serde_json::Value::Object(object)
            }
            Value::Tag(_, content) => content.to_json()?,
        })
    }

    /// A value from serde_json: integers stay integers, other numbers are floats.
    pub fn from_json(json: &serde_json::Value) -> Value {
        match json {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(b) => Value::Bool(*b),
            serde_json::Value::Number(number) => {
                if let Some(n) = number.as_i64() {
                    Value::Int(n.into())
                } else if let Some(n) = number.as_u64() {
                    Value::Int(n.into())
                } else {
                    Value::Float(number.as_f64().unwrap_or(f64::NAN))
                }
            }
            serde_json::Value::String(text) => Value::Text(text.clone()),
            serde_json::Value::Array(items) => Value::Array(items.iter().map(Value::from_json).collect()),
            serde_json::Value::Object(object) => {
                Value::Map(object.iter().map(|(k, v)| (Value::Text(k.clone()), Value::from_json(v))).collect())
            }
        }
    }

    /// The value as JSON text (UTF-8), maps in their order.
    pub(crate) fn json_text(&self) -> Result<Vec<u8>, ValueError> {
        self.to_json()?;
        serde_json::to_vec(self).map_err(|error| ValueError(error.to_string()))
    }

    /// A value from JSON text, maps in the order written.
    pub(crate) fn parse_json(text: &[u8]) -> Result<Value, serde_json::Error> {
        serde_json::from_slice(text)
    }
}

/// JSON writing in map order; [`Value::json_text`] checks first that the value has a JSON form.
impl serde::Serialize for Value {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{Error, SerializeMap, SerializeSeq};
        match self {
            Value::Null => serializer.serialize_unit(),
            Value::Bool(b) => serializer.serialize_bool(*b),
            Value::Int(n) => match (i64::try_from(*n), u64::try_from(*n)) {
                (Ok(small), _) => serializer.serialize_i64(small),
                (_, Ok(large)) => serializer.serialize_u64(large),
                _ => Err(S::Error::custom(format!("integer {n} has no JSON form"))),
            },
            Value::Float(f) => serializer.serialize_f64(*f),
            Value::Text(text) => serializer.serialize_str(text),
            Value::Bytes(bytes) => {
                let mut seq = serializer.serialize_seq(Some(bytes.len()))?;
                for b in bytes {
                    seq.serialize_element(b)?;
                }
                seq.end()
            }
            Value::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Value::Map(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (key, item) in entries {
                    match key {
                        Value::Text(text) => map.serialize_entry(text, item)?,
                        Value::Int(n) => map.serialize_entry(&n.to_string(), item)?,
                        other => return Err(S::Error::custom(format!("map key {other:?} has no JSON form"))),
                    }
                }
                map.end()
            }
            Value::Tag(_, content) => content.serialize(serializer),
        }
    }
}

/// JSON reading that keeps the order of a map's members and integers exact up to 64 bits.
impl<'de> serde::Deserialize<'de> for Value {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Value, D::Error> {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Value;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON value")
            }

            fn visit_unit<E>(self) -> Result<Value, E> {
                Ok(Value::Null)
            }

            fn visit_bool<E>(self, b: bool) -> Result<Value, E> {
                Ok(Value::Bool(b))
            }

            fn visit_i64<E>(self, n: i64) -> Result<Value, E> {
                Ok(Value::Int(n.into()))
            }

            fn visit_u64<E>(self, n: u64) -> Result<Value, E> {
                Ok(Value::Int(n.into()))
            }

            fn visit_f64<E>(self, f: f64) -> Result<Value, E> {
                Ok(Value::Float(f))
            }

            fn visit_str<E>(self, text: &str) -> Result<Value, E> {
                Ok(Value::Text(text.to_string()))
            }

            fn visit_string<E>(self, text: String) -> Result<Value, E> {
                Ok(Value::Text(text))
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Value::Array(items))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
                let mut entries: Vec<(Value, Value)> = Vec::new();
                while let Some((key, item)) = map.next_entry::<String, Value>()? {
                    if entries.iter().any(|(k, _)| matches!(k, Value::Text(text) if *text == key)) {
                        return Err(serde::de::Error::custom(format!("member {key:?} repeated")));
                    }
                    entries.push((Value::Text(key), item));
                }
                Ok(Value::Map(entries))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

impl From<serde_json::Value> for Value {
    fn from(json: serde_json::Value) -> Value {
        Value::from_json(&json)
    }
}

impl From<&str> for Value {
    fn from(text: &str) -> Value {
        Value::Text(text.to_string())
    }
}

impl From<String> for Value {
    fn from(text: String) -> Value {
        Value::Text(text)
    }
}

impl From<i64> for Value {
    fn from(n: i64) -> Value {
        Value::Int(n.into())
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::Bool(b)
    }
}

impl From<f64> for Value {
    fn from(f: f64) -> Value {
        Value::Float(f)
    }
}

impl From<Vec<u8>> for Value {
    fn from(bytes: Vec<u8>) -> Value {
        Value::Bytes(bytes)
    }
}

/// A value that has no form in the codec asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueError(pub String);

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ValueError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip_keeps_integers_and_order() {
        let json: serde_json::Value = serde_json::from_str(r#"{"b":1,"a":[2.5,"x",null,true,18446744073709551615]}"#).unwrap();
        let value = Value::from_json(&json);
        assert_eq!(value.get("b"), Some(&Value::Int(1)));
        assert_eq!(value.get("a").unwrap(), &Value::Array(vec![
            Value::Float(2.5), Value::Text("x".into()), Value::Null, Value::Bool(true), Value::Int(u64::MAX.into()),
        ]));
        assert_eq!(value.to_json().unwrap(), json);
        let ordered = Value::parse_json(br#"{"b":1,"a":2}"#).unwrap();
        assert_eq!(ordered, Value::map([("b", Value::from(1)), ("a", Value::from(2))]));
        assert_eq!(ordered.json_text().unwrap(), br#"{"b":1,"a":2}"#);
        assert!(Value::parse_json(br#"{"a":1,"a":2}"#).is_err());
    }

    #[test]
    fn values_without_a_json_form_are_refused() {
        assert!(Value::Int(1 << 70).to_json().is_err());
        assert!(Value::Float(f64::NAN).to_json().is_err());
        assert!(Value::Map(vec![(Value::Bool(true), Value::Null)]).to_json().is_err());
        assert_eq!(Value::Bytes(vec![1, 255]).to_json().unwrap(), serde_json::json!([1, 255]));
        assert_eq!(Value::Map(vec![(Value::Int(7), Value::Null)]).to_json().unwrap(), serde_json::json!({"7": null}));
    }
}
