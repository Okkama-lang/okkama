//! Natives форматов данных: `runtime/io_decorators`,
//! `runtime/testing`, `runtime/pem`, `runtime/template` (`runtime/csv_state` became Okkama in #13)
//! (`runtime/ini` переписан на Okkama, #13). Смещения и длины — в символах (code points), тексты отказов
//! — как у Perl-provider. Строки Rust всегда скалярны, поэтому проверки
//! одиночных суррогатов здесь не срабатывают никогда.
use super::Error;
use super::support::{
    failed, failure, handle, int, int_value, new_handle, string, string_array, text,
};
use crate::{IntValue, Value, abi::Native};
use std::cell::{Cell, RefCell};

fn operation(native: Native) -> &'static str {
    native.name()
}

fn chars_of(text: &str) -> Vec<char> {
    text.chars().collect()
}

fn slice(points: &[char], from: usize, to: usize) -> String {
    points[from..to].iter().collect()
}

// ---- runtime/io_decorators ----

#[derive(Debug)]
struct Counter(RefCell<IntValue>);

// ---- runtime/testing ----

#[derive(Debug)]
struct TestState {
    limit: i64,
    size: Cell<i64>,
    closed: Cell<bool>,
}

fn require_test(native: Native) -> Result<(), Error> {
    if !crate::testing::active() {
        return Err(failure(
            "core.testing",
            "permission_denied",
            native,
            "Test state requires a running case",
        ));
    }
    Ok(())
}

fn test_state(native: Native, value: &Value) -> Result<&TestState, Error> {
    require_test(native)?;
    let state = handle::<TestState>(value)?;
    if state.closed.get() {
        return Err(failure(
            "core.testing",
            "already_closed",
            native,
            "Native handle is already closed",
        ));
    }
    Ok(state)
}

// ---- runtime/pem ----

/// The value of a standard Base64 digit (`A-Z a-z 0-9 + /`).
fn sextet(symbol: char) -> Option<u64> {
    let value = match symbol {
        'A'..='Z' => symbol as u32 - 'A' as u32,
        'a'..='z' => symbol as u32 - 'a' as u32 + 26,
        '0'..='9' => symbol as u32 - '0' as u32 + 52,
        '+' => 62,
        '/' => 63,
        _ => return None,
    };
    Some(u64::from(value))
}

fn kind_invalid_offset(kind: &str) -> i64 {
    let text = chars_of(kind);
    if text.is_empty() {
        return 0;
    }
    let mut separator = true;
    for (offset, symbol) in text.iter().enumerate() {
        if *symbol == ' ' || *symbol == '-' {
            if separator || offset + 1 == text.len() {
                return offset as i64;
            }
            separator = true;
            continue;
        }
        if !(symbol.is_ascii_uppercase() || symbol.is_ascii_digit()) {
            return offset as i64;
        }
        separator = false;
    }
    -1
}

fn base64_invalid(source: &[char]) -> i64 {
    let size = source.len();
    if size == 0 {
        return -1;
    }
    let mut data_end = size;
    let mut padding = 0;
    let mut inside = false;
    for (offset, symbol) in source.iter().enumerate() {
        if *symbol == '=' {
            if !inside {
                inside = true;
                data_end = offset;
            }
            padding += 1;
            continue;
        }
        if inside || sextet(*symbol).is_none() {
            return offset as i64;
        }
    }
    if !size.is_multiple_of(4) {
        return size as i64;
    }
    let expected = match data_end % 4 {
        0 => 0,
        2 => 2,
        3 => 1,
        _ => -1,
    };
    if expected < 0 || padding != expected {
        return data_end as i64;
    }
    let mut buffer: u64 = 0;
    let mut bits = 0;
    for symbol in &source[..data_end] {
        let digit = sextet(*symbol).expect("validated base64");
        buffer = ((buffer << 6) | digit) & 0xffff_ffff;
        bits += 6;
        while bits >= 8 {
            bits -= 8;
        }
    }
    if bits > 0 && (buffer & ((1 << bits) - 1)) != 0 {
        return data_end as i64 - 1;
    }
    -1
}

struct PemLine {
    text: String,
    start: usize,
    end: usize,
    length: usize,
}

struct PemBlock {
    kind: String,
    base64: String,
    source: String,
}

fn pem_kind(line: &str) -> Option<&str> {
    let kind = line.strip_prefix("-----BEGIN ")?.strip_suffix("-----")?;
    // [A-Z0-9]+(?:[ -][A-Z0-9]+)*
    let word = |w: &str| !w.is_empty() && w.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    kind.split([' ', '-']).all(word).then_some(kind)
}

/// Смещение первой ошибки (или -1) и блоки.
fn pem_parsed(origin: &str) -> (i64, Vec<PemBlock>) {
    let source = chars_of(origin);
    let size = source.len();
    let mut lines = Vec::new();
    let (mut start, mut offset) = (0, 0);
    let line = |from: usize, stop: usize, end: usize| PemLine {
        text: slice(&source, from, stop),
        start: from,
        end,
        length: stop - from,
    };
    while offset < size {
        let symbol = source[offset];
        if symbol == '\r' {
            if offset + 1 >= size || source[offset + 1] != '\n' {
                return (offset as i64, Vec::new());
            }
            lines.push(line(start, offset, offset + 2));
            offset += 2;
            start = offset;
            continue;
        }
        if symbol == '\n' {
            lines.push(line(start, offset, offset + 1));
            offset += 1;
            start = offset;
            continue;
        }
        offset += 1;
    }
    if start < size {
        lines.push(line(start, size, size));
    }
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if lines[index].text.is_empty() {
            index += 1;
            continue;
        }
        let Some(kind) = pem_kind(&lines[index].text).map(str::to_string) else {
            return (lines[index].start as i64, Vec::new());
        };
        let block_start = lines[index].start;
        let mut block_end = lines[index].end;
        let mut last_line_start = lines[index].start;
        index += 1;
        let mut body: Vec<char> = Vec::new();
        let mut mapping = Vec::new();
        let mut body_lines: Vec<(usize, usize)> = Vec::new();
        let mut closed = false;
        while index < lines.len() {
            let current = &lines[index];
            last_line_start = current.start;
            if let Some(end_kind) = current
                .text
                .strip_prefix("-----END ")
                .and_then(|rest| rest.strip_suffix("-----"))
            {
                if end_kind != kind {
                    return ((current.start + 9) as i64, Vec::new());
                }
                closed = true;
                block_end = current.end;
                index += 1;
                break;
            }
            if current.text.is_empty() || current.text.starts_with("-----BEGIN ") {
                return (current.start as i64, Vec::new());
            }
            if current.length > 64 {
                return ((current.start + 64) as i64, Vec::new());
            }
            body_lines.push((current.start, current.length));
            for (column, point) in current.text.chars().enumerate() {
                body.push(point);
                mapping.push(current.start + column);
            }
            index += 1;
        }
        if !closed {
            return (size as i64, Vec::new());
        }
        for (at, length) in body_lines.iter().take(body_lines.len().saturating_sub(1)) {
            if *length != 64 {
                return ((at + length) as i64, Vec::new());
            }
        }
        let error = base64_invalid(&body);
        if error != -1 {
            let at = if (error as usize) < mapping.len() {
                mapping[error as usize]
            } else if let Some((start, length)) = body_lines.last() {
                start + length
            } else {
                last_line_start
            };
            return (at as i64, Vec::new());
        }
        blocks.push(PemBlock {
            kind,
            base64: body.into_iter().collect(),
            source: slice(&source, block_start, block_end),
        });
    }
    (-1, blocks)
}

fn pem_block(native: Native, origin: &str, index: &IntValue) -> Result<PemBlock, Error> {
    let (offset, mut blocks) = pem_parsed(origin);
    if offset != -1 {
        return Err(failed(native));
    }
    let at = super::support::small(index, blocks.len()).ok_or_else(|| failed(native))?;
    Ok(blocks.swap_remove(at))
}

pub fn execute(native: Native, args: &[Value]) -> Result<Value, Error> {
    let name = operation(native);
    match name {
        // ---- io_decorators ----
        "limited_reading_state_create" => {
            let limit = int(&args[0])?;
            if limit.is_negative() {
                return Err(failure(
                    "core.io_decorators",
                    "invalid_state",
                    native,
                    "Limited reading state requires a non-negative limit",
                ));
            }
            Ok(new_handle(Counter(RefCell::new(limit.clone()))))
        }
        "limited_reading_state_remaining" | "chained_reading_state_position" => {
            Ok(Value::Int(handle::<Counter>(&args[0])?.0.borrow().clone()))
        }
        "limited_reading_state_consume" => {
            let counter = handle::<Counter>(&args[0])?;
            let amount = int(&args[1])?;
            let current = counter.0.borrow().clone();
            if amount.is_negative() || crate::integer::compare(amount, &current).is_gt() {
                return Err(failure(
                    "core.io_decorators",
                    "invalid_state",
                    native,
                    "Limited reading consumption exceeds remaining bytes",
                ));
            }
            let next = crate::integer::subtract(&current, amount);
            *counter.0.borrow_mut() = next.clone();
            Ok(Value::Int(next))
        }
        "chained_reading_state_create" => {
            Ok(new_handle(Counter(RefCell::new(IntValue::from_i64(0)))))
        }
        "chained_reading_state_advance" => {
            let counter = handle::<Counter>(&args[0])?;
            let next = crate::integer::add(&counter.0.borrow(), &IntValue::from_i64(1));
            *counter.0.borrow_mut() = next.clone();
            Ok(Value::Int(next))
        }
        // ---- testing ----
        "test_state_open" => {
            require_test(native)?;
            let limit = int(&args[0])?
                .to_i64()
                .filter(|limit| (0..=65_536).contains(limit))
                .ok_or_else(|| {
                    failure(
                        "core.testing",
                        "invalid_input",
                        native,
                        "Test state capacity must be 0..65536",
                    )
                })?;
            Ok(new_handle(TestState {
                limit,
                size: Cell::new(0),
                closed: Cell::new(false),
            }))
        }
        "test_state_size" => Ok(int_value(test_state(native, &args[0])?.size.get())),
        "test_state_next" => {
            let state = test_state(native, &args[0])?;
            if state.size.get() >= state.limit {
                return Err(failure(
                    "core.testing",
                    "resource_exhausted",
                    native,
                    "Test state capacity exhausted",
                ));
            }
            state.size.set(state.size.get() + 1);
            Ok(int_value(state.size.get() - 1))
        }
        "test_state_close" => {
            require_test(native)?;
            handle::<TestState>(&args[0])?.closed.set(true);
            Ok(Value::Bool(true))
        }
        "test_context_active" => Ok(Value::Bool(crate::testing::active())),
        // ---- pem ----
        "pem_kind_invalid_offset" => Ok(int_value(kind_invalid_offset(string(&args[0])?))),
        "pem_encode" => {
            let (kind, base64) = (string(&args[0])?, chars_of(string(&args[1])?));
            let mut out = format!("-----BEGIN {kind}-----\n");
            for chunk in base64.chunks(64) {
                out.extend(chunk);
                out.push('\n');
            }
            out.push_str(&format!("-----END {kind}-----\n"));
            Ok(text(out))
        }
        "pem_block_sources" => {
            let (offset, blocks) = pem_parsed(string(&args[0])?);
            if offset != -1 {
                return Err(failure("core.pem", "invalid_data", native, "invalid PEM input"));
            }
            Ok(string_array(blocks.into_iter().map(|block| block.source)))
        }
        "pem_invalid_offset" => Ok(int_value(pem_parsed(string(&args[0])?).0)),
        "pem_block_count" => {
            let (offset, blocks) = pem_parsed(string(&args[0])?);
            if offset != -1 {
                return Err(failed(native));
            }
            Ok(int_value(blocks.len() as i64))
        }
        "pem_block_source" => Ok(text(pem_block(native, string(&args[0])?, int(&args[1])?)?.source)),
        "pem_block_kind" => Ok(text(pem_block(native, string(&args[0])?, int(&args[1])?)?.kind)),
        "pem_block_base64" => Ok(text(pem_block(native, string(&args[0])?, int(&args[1])?)?.base64)),
        // ---- template ----
        "template_output_size" => {
            let maximum = int(&args[1])?;
            if maximum.is_negative() {
                return Err(failure(
                    "core.template",
                    "invalid_input",
                    native,
                    "negative template byte budget",
                ));
            }
            let value = string(&args[0])?;
            let limit = maximum.to_i64().unwrap_or(i64::MAX);
            if value.chars().count() as i64 > limit {
                return Ok(int_value(-1));
            }
            let size = value.len() as i64;
            Ok(int_value(if size > limit { -1 } else { size }))
        }
        _ => Err(Error::Unsupported(format!(
            "native provider is not implemented: {}",
            native.symbol()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{base64_invalid, chars_of, sextet};

    #[test]
    fn pem_sextet_is_the_standard_base64_alphabet() {
        let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        for (value, symbol) in alphabet.chars().enumerate() {
            assert_eq!(sextet(symbol), Some(value as u64));
        }
        for symbol in ['=', '-', '_', ' ', '\n', '\u{e9}', '\u{ff21}', '@', '[', '`', '{'] {
            assert_eq!(sextet(symbol), None);
        }
    }

    #[test]
    fn pem_base64_invalid_reports_the_first_offending_offset() {
        let offset = |text: &str| base64_invalid(&chars_of(text));
        assert_eq!(offset(""), -1);
        assert_eq!(offset("QUJD"), -1);
        assert_eq!(offset("QUI="), -1);
        assert_eq!(offset("QQ=="), -1);
        assert_eq!(offset("QU-D"), 2);
        assert_eq!(offset("QUJ"), 3);
        assert_eq!(offset("QUJ="), 2);
        assert_eq!(offset("QR=="), 1);
        assert_eq!(offset("Q==="), 1);
        assert_eq!(offset("QQ=A"), 3);
    }
}
