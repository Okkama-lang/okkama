//! A small CBOR (RFC 8949) codec for payloads: what Okkama's `cbor` module writes and what other helpers may
//! send. It writes definite lengths, integers in their shortest head and floats as doubles; it reads definite and
//! indefinite lengths, half, single and double floats, and refuses what is not well-formed.

use crate::value::Value;

/// How deep a decoded value may nest.
const DEPTH: usize = 128;

/// The CBOR bytes of a value.
pub fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write(value, &mut out);
    out
}

fn head(major: u8, n: u64, out: &mut Vec<u8>) {
    let major = major << 5;
    if n < 24 {
        out.push(major | n as u8);
    } else if n <= u8::MAX as u64 {
        out.extend([major | 24, n as u8]);
    } else if n <= u16::MAX as u64 {
        out.push(major | 25);
        out.extend((n as u16).to_be_bytes());
    } else if n <= u32::MAX as u64 {
        out.push(major | 26);
        out.extend((n as u32).to_be_bytes());
    } else {
        out.push(major | 27);
        out.extend(n.to_be_bytes());
    }
}

fn magnitude(n: u128) -> Vec<u8> {
    let bytes = n.to_be_bytes();
    let first = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len());
    bytes[first..].to_vec()
}

fn write(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.push(0xf6),
        Value::Bool(false) => out.push(0xf4),
        Value::Bool(true) => out.push(0xf5),
        Value::Int(n) if *n >= 0 => match u64::try_from(*n) {
            Ok(small) => head(0, small, out),
            Err(_) => {
                head(6, 2, out);
                write(&Value::Bytes(magnitude(*n as u128)), out);
            }
        },
        Value::Int(n) => {
            // −1 − n, the magnitude CBOR stores for a negative integer.
            let stored = (-1 - *n) as u128;
            match u64::try_from(stored) {
                Ok(small) => head(1, small, out),
                Err(_) => {
                    head(6, 3, out);
                    write(&Value::Bytes(magnitude(stored)), out);
                }
            }
        }
        Value::Float(f) => {
            out.push(0xfb);
            out.extend(f.to_bits().to_be_bytes());
        }
        Value::Text(text) => {
            head(3, text.len() as u64, out);
            out.extend(text.as_bytes());
        }
        Value::Bytes(bytes) => {
            head(2, bytes.len() as u64, out);
            out.extend(bytes);
        }
        Value::Array(items) => {
            head(4, items.len() as u64, out);
            items.iter().for_each(|item| write(item, out));
        }
        Value::Map(entries) => {
            head(5, entries.len() as u64, out);
            for (key, item) in entries {
                write(key, out);
                write(item, out);
            }
        }
        Value::Tag(tag, content) => {
            head(6, *tag, out);
            write(content, out);
        }
    }
}

/// The one value the bytes hold; anything after it, or anything not well-formed, is refused.
pub fn decode(bytes: &[u8]) -> Result<Value, String> {
    let mut reader = Reader { bytes, at: 0 };
    let value = reader.item(0)?;
    if reader.at != bytes.len() {
        return Err(format!("{} bytes after the CBOR value", bytes.len() - reader.at));
    }
    Ok(value)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

/// What an item's initial byte says.
enum Head {
    Value(u64),
    Indefinite,
}

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], String> {
        let end = self.at.checked_add(count).filter(|end| *end <= self.bytes.len()).ok_or_else(|| format!("CBOR truncated at byte {}", self.at))?;
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }

    fn head(&mut self, info: u8) -> Result<Head, String> {
        Ok(Head::Value(match info {
            0..=23 => info as u64,
            24 => self.take(1)?[0] as u64,
            25 => u16::from_be_bytes(self.take(2)?.try_into().unwrap()) as u64,
            26 => u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as u64,
            27 => u64::from_be_bytes(self.take(8)?.try_into().unwrap()),
            31 => return Ok(Head::Indefinite),
            _ => return Err(format!("reserved CBOR additional information {info} at byte {}", self.at - 1)),
        }))
    }

    fn length(&mut self, n: u64) -> Result<usize, String> {
        // A declared length past the remaining bytes is refused before anything is allocated for it.
        usize::try_from(n).ok().filter(|n| *n <= self.bytes.len() - self.at).ok_or_else(|| format!("CBOR truncated at byte {}", self.at))
    }

    fn item(&mut self, depth: usize) -> Result<Value, String> {
        if depth > DEPTH {
            return Err(format!("CBOR nests deeper than {DEPTH}"));
        }
        let start = self.at;
        let initial = self.take(1)?[0];
        let (major, info) = (initial >> 5, initial & 31);
        match (major, self.head(info)?) {
            (0, Head::Value(n)) => Ok(Value::Int(n.into())),
            (1, Head::Value(n)) => Ok(Value::Int(-1 - i128::from(n))),
            (2 | 3, Head::Value(n)) => {
                let size = self.length(n)?;
                let chunk = self.take(size)?.to_vec();
                if major == 2 { Ok(Value::Bytes(chunk)) } else { text(chunk, start) }
            }
            (2 | 3, Head::Indefinite) => {
                let mut joined = Vec::new();
                loop {
                    let next = *self.bytes.get(self.at).ok_or_else(|| format!("CBOR truncated at byte {}", self.at))?;
                    if next == 0xff {
                        self.at += 1;
                        break;
                    }
                    match self.item(depth + 1)? {
                        Value::Bytes(chunk) if major == 2 => joined.extend(chunk),
                        Value::Text(chunk) if major == 3 => joined.extend(chunk.into_bytes()),
                        _ => return Err(format!("indefinite CBOR string at byte {start} holds a chunk of another kind")),
                    }
                }
                if major == 2 { Ok(Value::Bytes(joined)) } else { text(joined, start) }
            }
            (4, head) => {
                let mut items = Vec::new();
                match head {
                    Head::Value(n) => {
                        let count = self.length(n)?;
                        for _ in 0..count {
                            items.push(self.item(depth + 1)?);
                        }
                    }
                    Head::Indefinite => while !self.end_of_items()? {
                        items.push(self.item(depth + 1)?);
                    },
                }
                Ok(Value::Array(items))
            }
            (5, head) => {
                let mut entries = Vec::new();
                match head {
                    Head::Value(n) => {
                        let count = self.length(n)?;
                        for _ in 0..count {
                            entries.push((self.item(depth + 1)?, self.item(depth + 1)?));
                        }
                    }
                    Head::Indefinite => while !self.end_of_items()? {
                        entries.push((self.item(depth + 1)?, self.item(depth + 1)?));
                    },
                }
                Ok(Value::Map(entries))
            }
            (6, Head::Value(tag)) => {
                let content = self.item(depth + 1)?;
                match (tag, &content) {
                    (2, Value::Bytes(bytes)) => Ok(bignum(bytes).map_or_else(|| Value::Tag(tag, Box::new(content.clone())), Value::Int)),
                    (3, Value::Bytes(bytes)) => Ok(bignum(bytes).map_or_else(|| Value::Tag(tag, Box::new(content.clone())), |m| Value::Int(-1 - m))),
                    _ => Ok(Value::Tag(tag, Box::new(content))),
                }
            }
            (7, Head::Value(n)) => match info {
                20 => Ok(Value::Bool(false)),
                21 => Ok(Value::Bool(true)),
                22 | 23 => Ok(Value::Null),
                25 => Ok(Value::Float(half(n as u16))),
                26 => Ok(Value::Float(f32::from_bits(n as u32) as f64)),
                27 => Ok(Value::Float(f64::from_bits(n))),
                _ => Err(format!("unsupported CBOR simple value at byte {start}")),
            },
            _ => Err(format!("CBOR item at byte {start} is not well-formed")),
        }
    }

    fn end_of_items(&mut self) -> Result<bool, String> {
        match self.bytes.get(self.at) {
            Some(0xff) => {
                self.at += 1;
                Ok(true)
            }
            Some(_) => Ok(false),
            None => Err(format!("CBOR truncated at byte {}", self.at)),
        }
    }
}

/// A bignum's magnitude when it fits an i128; a larger one stays a tag.
fn bignum(bytes: &[u8]) -> Option<i128> {
    let first = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len());
    let digits = &bytes[first..];
    if digits.len() > 16 || (digits.len() == 16 && digits[0] & 0x80 != 0) {
        return None;
    }
    Some(digits.iter().fold(0i128, |acc, b| (acc << 8) | *b as i128))
}

fn text(bytes: Vec<u8>, start: usize) -> Result<Value, String> {
    String::from_utf8(bytes).map(Value::Text).map_err(|_| format!("CBOR text at byte {start} is not UTF-8"))
}

/// An IEEE 754 half-precision float.
fn half(bits: u16) -> f64 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = ((bits >> 10) & 0x1f) as i32;
    let fraction = (bits & 0x3ff) as f64;
    sign * match exponent {
        0 => fraction * 2f64.powi(-24),
        31 => if fraction == 0.0 { f64::INFINITY } else { f64::NAN },
        _ => (1.0 + fraction / 1024.0) * 2f64.powi(exponent - 15),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn rfc_8949_appendix_a_examples_decode() {
        assert_eq!(decode(&hex("00")).unwrap(), Value::Int(0));
        assert_eq!(decode(&hex("1903e8")).unwrap(), Value::Int(1000));
        assert_eq!(decode(&hex("1bffffffffffffffff")).unwrap(), Value::Int(u64::MAX.into()));
        assert_eq!(decode(&hex("c249010000000000000000")).unwrap(), Value::Int(1 << 64));
        assert_eq!(decode(&hex("3bffffffffffffffff")).unwrap(), Value::Int(-(1i128 << 64)));
        assert_eq!(decode(&hex("c349010000000000000000")).unwrap(), Value::Int(-(1i128 << 64) - 1));
        assert_eq!(decode(&hex("f93e00")).unwrap(), Value::Float(1.5));
        assert_eq!(decode(&hex("f90001")).unwrap(), Value::Float(5.960464477539063e-8));
        assert_eq!(decode(&hex("fa47c35000")).unwrap(), Value::Float(100000.0));
        assert_eq!(decode(&hex("fb3ff199999999999a")).unwrap(), Value::Float(1.1));
        assert_eq!(decode(&hex("6449455446")).unwrap(), Value::Text("IETF".into()));
        assert_eq!(decode(&hex("4401020304")).unwrap(), Value::Bytes(vec![1, 2, 3, 4]));
        assert_eq!(decode(&hex("5f42010243030405ff")).unwrap(), Value::Bytes(vec![1, 2, 3, 4, 5]));
        assert_eq!(decode(&hex("7f657374726561646d696e67ff")).unwrap(), Value::Text("streaming".into()));
        assert_eq!(decode(&hex("9f018202039f0405ffff")).unwrap(), Value::Array(vec![
            Value::Int(1), Value::Array(vec![Value::Int(2), Value::Int(3)]), Value::Array(vec![Value::Int(4), Value::Int(5)]),
        ]));
        assert_eq!(decode(&hex("bf61610161629f0203ffff")).unwrap(), Value::map([
            ("a", Value::Int(1)), ("b", Value::Array(vec![Value::Int(2), Value::Int(3)])),
        ]));
        assert_eq!(decode(&hex("c074323031332d30332d32315432303a30343a30305a")).unwrap(),
            Value::Tag(0, Box::new(Value::Text("2013-03-21T20:04:00Z".into()))));
        assert_eq!(decode(&hex("f7")).unwrap(), Value::Null);
    }

    #[test]
    fn encoding_round_trips_with_shortest_heads() {
        let value = Value::map([
            ("id", Value::Int(4711)),
            ("big", Value::Int(1 << 70)),
            ("small", Value::Int(-(1i128 << 70))),
            ("tags", Value::Array(vec![Value::Text("new".into()), Value::Bytes(vec![0, 255]), Value::Float(2.5)])),
            ("rat", Value::Tag(30, Box::new(Value::Array(vec![Value::Int(1), Value::Int(3)])))),
        ]);
        assert_eq!(decode(&encode(&value)).unwrap(), value);
        assert_eq!(encode(&Value::Int(23)), hex("17"));
        assert_eq!(encode(&Value::Int(24)), hex("1818"));
        assert_eq!(encode(&Value::Int(-1000)), hex("3903e7"));
        assert_eq!(encode(&Value::Int(u64::MAX.into())), hex("1bffffffffffffffff"));
    }

    #[test]
    fn what_is_not_well_formed_is_refused() {
        assert!(decode(&hex("1a0000")).unwrap_err().contains("truncated"));
        assert!(decode(&hex("5bffffffffffffffff")).unwrap_err().contains("truncated"));
        assert!(decode(&hex("62c328")).unwrap_err().contains("UTF-8"));
        assert!(decode(&hex("0000")).unwrap_err().contains("after"));
        assert!(decode(&hex("1c")).unwrap_err().contains("reserved"));
        assert!(decode(&hex("5f01ff")).unwrap_err().contains("another kind"));
        assert!(decode(&[0x81; 200]).unwrap_err().contains("deeper"));
    }
}
