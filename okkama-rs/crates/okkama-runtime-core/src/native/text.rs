//! Text natives Sparrow routes here (`host/native_origin.rs`):
//! `runtime/hpack` (Huffman decoding), `runtime/collections` (star match, map
//! snapshot) and the float half of `runtime/number_text`. `runtime/binary` and
//! the integer half of `runtime/number_text` are Sparrow's own providers.
use super::Error;
use super::support::{
    byte_array, bytes, failed, failure, int, items, string, string_map,
};
use crate::{IntValue, Value, abi::Native};

#[path = "hpack_table.rs"]
mod hpack_table;

fn operation(native: Native) -> &'static str {
    native.name()
}

// ---- runtime/hpack ----

fn huffman_decode(native: Native, origin: &[u8], capacity: &IntValue) -> Result<Vec<u8>, Error> {
    let maximum = capacity
        .to_i64()
        .filter(|capacity| (0..=1_048_576).contains(capacity))
        .filter(|_| origin.len() <= 1_048_576)
        .ok_or_else(|| failure("core.hpack", "invalid_input", native, "invalid HPACK Huffman bounds"))?
        as usize;
    let nodes = &hpack_table::HUFFMAN;
    let mut node: usize = 0;
    let mut out = Vec::new();
    for octet in origin {
        for shift in (0..8).rev() {
            let bit = usize::from((octet >> shift) & 1);
            let next = nodes[node * 4 + bit];
            if next < 0 {
                return Err(failure("core.hpack", "invalid_data", native, "invalid HPACK Huffman code"));
            }
            node = next as usize;
            let decoded = nodes[node * 4 + 2];
            if decoded >= 0 {
                if decoded == 256 {
                    return Err(failure(
                        "core.hpack",
                        "invalid_data",
                        native,
                        "HPACK Huffman EOS is forbidden",
                    ));
                }
                if out.len() >= maximum {
                    return Err(failure(
                        "core.hpack",
                        "resource_exhausted",
                        native,
                        "decoded HPACK Huffman limit",
                    ));
                }
                out.push(decoded as u8);
                node = 0;
            }
        }
    }
    if nodes[node * 4 + 3] == 0 {
        return Err(failure("core.hpack", "invalid_data", native, "invalid HPACK Huffman padding"));
    }
    Ok(out)
}

// ---- runtime/collections ----

fn star_match(native: Native, pattern: &str, target: &str, anchored: bool, limit: &IntValue) -> Result<bool, Error> {
    let budget = limit
        .to_i64()
        .filter(|limit| (1..=262_144).contains(limit))
        .ok_or_else(|| {
            failure(
                "core.collections",
                "invalid_input",
                native,
                "Invalid string star-match step budget",
            )
        })?;
    let left: Vec<char> = pattern.chars().collect();
    let right: Vec<char> = target.chars().collect();
    let (mut position, mut at, mut star, mut retry, mut steps) = (0usize, 0usize, None, 0usize, 0i64);
    loop {
        steps += 1;
        if steps > budget {
            return Err(failure(
                "core.collections",
                "resource_exhausted",
                native,
                "String star-match step limit",
            ));
        }
        if position == left.len() && (!anchored || at == right.len()) {
            return Ok(true);
        }
        if position < left.len() && left[position] == '*' {
            star = Some(position);
            retry = at;
            position += 1;
            continue;
        }
        if at < right.len() && position < left.len() && left[position] == right[at] {
            at += 1;
            position += 1;
            continue;
        }
        if let Some(star) = star
            && retry < right.len() {
                retry += 1;
                at = retry;
                position = star + 1;
                continue;
            }
        return Ok(false);
    }
}

fn map_snapshot(native: Native, args: &[Value]) -> Result<Value, Error> {
    let invalid = |code: &str, message: &str| failure("core.collections", code, native, message);
    let entries = int(&args[2])?
        .to_i64()
        .filter(|entries| (0..=4096).contains(entries));
    let characters = int(&args[3])?
        .to_i64()
        .filter(|characters| (0..=1_048_576).contains(characters));
    let (Some(entries), Some(maximum)) = (entries, characters) else {
        return Err(invalid("invalid_input", "Invalid string map snapshot budgets"));
    };
    let (keys, values) = (items(&args[0])?, items(&args[1])?);
    if keys.len() != values.len() {
        return Err(invalid(
            "invalid_input",
            "String map coordinates have different lengths",
        ));
    }
    if keys.len() as i64 > entries {
        return Err(invalid("resource_exhausted", "String map entry limit"));
    }
    let mut seen = std::collections::HashSet::new();
    let mut used: i64 = 0;
    let mut out = Vec::new();
    for (key, value) in keys.iter().zip(values.iter()) {
        let (key, value) = (key.as_string()?, value.as_string()?);
        if !seen.insert(key) {
            return Err(invalid("invalid_input", "String map has duplicate keys"));
        }
        let width = (key.chars().count() + value.chars().count()) as i64;
        if width > maximum - used {
            return Err(invalid("resource_exhausted", "String map character limit"));
        }
        used += width;
        out.push((key.to_string(), value.to_string()));
    }
    Ok(string_map(out))
}

pub fn execute(native: Native, args: &[Value]) -> Result<Value, Error> {
    match operation(native) {
        "huffman_decode" => Ok(byte_array(huffman_decode(
            native,
            &bytes(&args[0])?,
            int(&args[1])?,
        )?)),
        "string_star_match" => Ok(Value::Bool(star_match(
            native,
            string(&args[0])?,
            string(&args[1])?,
            args[2].as_bool()?,
            int(&args[3])?,
        )?)),
        "string_map_snapshot" => map_snapshot(native, args),
        "float_parse_valid" => Ok(Value::Bool(float_parse(string(&args[0])?).is_some())),
        "float_parse" => float_parse(string(&args[0])?)
            .map(Value::Float)
            .ok_or_else(|| failed(native)),
        _ => Err(Error::Unsupported(format!(
            "native provider is not implemented: {}",
            native.symbol()
        ))),
    }
}

// ---- runtime/number_text ----

/// Дробное в записи `strings/parse::Number.float` (#170): необязательный `-`,
/// цифры, `.` и цифры; без экспоненты и `+`. Ближайшее `f64`.
fn float_parse(text: &str) -> Option<f64> {
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = unsigned.split_once('.')?;
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(whole) || !digits(fraction) {
        return None;
    }
    // Language floats are finite (#986): a literal past f64::MAX is the nearest
    // finite float, never infinity (std's Decimal.float relies on that).
    text.parse::<f64>().ok().map(|value| value.clamp(f64::MIN, f64::MAX))
}

#[cfg(test)]
mod number_text_tests {
    use super::*;

    /// #986: a float literal past f64::MAX saturates to the nearest finite float.
    #[test]
    fn float_literals_parse_only_to_finite_values() {
        assert_eq!(float_parse(&format!("{}.0", "9".repeat(400))), Some(f64::MAX));
        assert_eq!(float_parse(&format!("-{}.0", "9".repeat(400))), Some(f64::MIN));
        assert_eq!(float_parse("1.5"), Some(1.5));
        assert!(float_parse(&format!("{}.0", "9".repeat(300))).is_some_and(f64::is_finite));
    }
}
