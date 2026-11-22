//! `runtime/html` (поток событий HTML с именованными ссылками WHATWG). TOML
//! читает std (`toml/parse`, #13).
//! Смещения — в символах; тексты отказов — как у Perl-provider.
use super::Error;
use super::support::{failure, handle, int, int_value, new_handle, string, text};
use crate::{Value, abi::Native};
use std::collections::HashMap;

fn slice(points: &[char], from: usize, to: usize) -> String {
    if to <= from {
        return String::new();
    }
    points[from..to.min(points.len())].iter().collect()
}

// ===================== runtime/html =====================

fn entities() -> &'static HashMap<&'static str, String> {
    static ENTITIES: std::sync::OnceLock<HashMap<&'static str, String>> = std::sync::OnceLock::new();
    ENTITIES.get_or_init(|| {
        crate::data::html_entities()
            .lines()
            .filter_map(|line| {
                let (name, points) = line.split_once('\t')?;
                let value = points
                    .split(' ')
                    .filter_map(|hex| u32::from_str_radix(hex, 16).ok().and_then(char::from_u32))
                    .collect();
                Some((name, value))
            })
            .collect()
    })
}

const NUMERIC_REPLACEMENT: [(u32, u32); 27] = [
    (0x80, 0x20AC), (0x82, 0x201A), (0x83, 0x0192), (0x84, 0x201E), (0x85, 0x2026),
    (0x86, 0x2020), (0x87, 0x2021), (0x88, 0x02C6), (0x89, 0x2030), (0x8A, 0x0160),
    (0x8B, 0x2039), (0x8C, 0x0152), (0x8E, 0x017D), (0x91, 0x2018), (0x92, 0x2019),
    (0x93, 0x201C), (0x94, 0x201D), (0x95, 0x2022), (0x96, 0x2013), (0x97, 0x2014),
    (0x98, 0x02DC), (0x99, 0x2122), (0x9A, 0x0161), (0x9B, 0x203A), (0x9C, 0x0153),
    (0x9E, 0x017E), (0x9F, 0x0178),
];

fn numeric_character(digits: &str, base: u32) -> String {
    let digits = digits.trim_start_matches('0');
    let value = if digits.len() > 8 {
        None
    } else {
        u32::from_str_radix(if digits.is_empty() { "0" } else { digits }, base).ok()
    };
    let Some(mut point) = value.filter(|value| *value != 0 && *value <= 0x10FFFF && !(0xD800..=0xDFFF).contains(value)) else {
        return "\u{FFFD}".into();
    };
    for (from, to) in NUMERIC_REPLACEMENT {
        if from == point {
            point = to;
        }
    }
    char::from_u32(point).expect("scalar").to_string()
}

/// Ссылка `&…;` в позиции: значение и длина.
fn reference_at(source: &[char], offset: usize, attribute: bool) -> Option<(String, usize)> {
    let length = source.len();
    if offset >= length || source[offset] != '&' {
        return None;
    }
    let following = offset + 1;
    if following >= length {
        return None;
    }
    if source[following] == '#' {
        let mut position = following + 1;
        let mut base = 10;
        if position < length && (source[position] == 'x' || source[position] == 'X') {
            base = 16;
            position += 1;
        }
        let start = position;
        while position < length
            && if base == 16 {
                source[position].is_ascii_hexdigit()
            } else {
                source[position].is_ascii_digit()
            }
        {
            position += 1;
        }
        if position == start {
            return None;
        }
        let digits = slice(source, start, position);
        if position < length && source[position] == ';' {
            position += 1;
        }
        return Some((numeric_character(&digits, base), position - offset));
    }
    let maximum = (length - following).min(32);
    let table = entities();
    for amount in (1..=maximum).rev() {
        let name = slice(source, following, following + amount);
        let Some(value) = table.get(name.as_str()) else {
            continue;
        };
        if attribute && !name.ends_with(';') {
            let after = following + amount;
            if after < length && (source[after].is_ascii_alphanumeric() || source[after] == '=') {
                continue;
            }
        }
        return Some((value.clone(), amount + 1));
    }
    None
}

fn decode_references(value: &str, attribute: bool) -> String {
    let source: Vec<char> = value.chars().collect();
    let mut result = String::new();
    let mut offset = 0;
    while offset < source.len() {
        if source[offset] == '&'
            && let Some((decoded, length)) = reference_at(&source, offset, attribute) {
                result.push_str(&decoded);
                offset += length;
                continue;
            }
        result.push(source[offset]);
        offset += 1;
    }
    result
}

#[derive(Debug, Default)]
struct Event {
    kind: i64,
    name: Option<String>,
    value: Option<String>,
    target: Option<String>,
    data: Option<String>,
    self_closing: Option<bool>,
    attributes: Vec<(String, String)>,
    valued: Vec<bool>,
}

fn push_text(events: &mut Vec<Event>, value: &str, raw: bool) {
    if value.is_empty() {
        return;
    }
    let decoded = if raw {
        value.to_string()
    } else {
        decode_references(value, false)
    };
    if let Some(last) = events.last_mut().filter(|last| last.kind == 2) {
        last.value.get_or_insert_with(String::new).push_str(&decoded);
    } else {
        events.push(Event {
            kind: 2,
            value: Some(decoded),
            ..Event::default()
        });
    }
}

fn tag_end(source: &[char], offset: usize) -> Option<usize> {
    let mut quote = None;
    for (position, c) in source.iter().enumerate().skip(offset) {
        match quote {
            Some(open) if *c == open => quote = None,
            Some(_) => {}
            None if *c == '"' || *c == '\'' => quote = Some(*c),
            None if *c == '>' => return Some(position),
            None => {}
        }
    }
    None
}

fn lower(value: &str) -> String {
    value.chars().map(|c| c.to_ascii_lowercase()).collect()
}

fn attributes(body: &[char], mut position: usize, event: &mut Event) {
    let length = body.len();
    while position < length {
        while position < length && body[position].is_whitespace() {
            position += 1;
        }
        if position >= length {
            break;
        }
        let start = position;
        while position < length && !body[position].is_whitespace() && body[position] != '=' {
            position += 1;
        }
        let name = slice(body, start, position);
        if name.is_empty() {
            break;
        }
        let name = lower(&name);
        while position < length && body[position].is_whitespace() {
            position += 1;
        }
        let mut valued = false;
        let mut value = String::new();
        if position < length && body[position] == '=' {
            valued = true;
            position += 1;
            while position < length && body[position].is_whitespace() {
                position += 1;
            }
            if position < length && (body[position] == '"' || body[position] == '\'') {
                let quote = body[position];
                position += 1;
                let value_start = position;
                while position < length && body[position] != quote {
                    position += 1;
                }
                value = slice(body, value_start, position);
                if position < length {
                    position += 1;
                }
            } else {
                let value_start = position;
                while position < length && !body[position].is_whitespace() {
                    position += 1;
                }
                value = slice(body, value_start, position);
            }
            value = decode_references(&value, true);
        }
        event.attributes.push((name, value));
        event.valued.push(valued);
    }
}

/// `[A-Za-z][^\s/>]*` с начала.
fn tag_name(body: &[char], from: usize) -> Option<usize> {
    if !body.get(from).is_some_and(char::is_ascii_alphabetic) {
        return None;
    }
    let mut end = from + 1;
    while end < body.len() && !body[end].is_whitespace() && body[end] != '/' && body[end] != '>' {
        end += 1;
    }
    Some(end)
}

fn start_tag(source: &[char], offset: usize) -> Option<(Event, usize)> {
    if source[offset] != '<' {
        return None;
    }
    let end = tag_end(source, offset + 1)?;
    let mut body: Vec<char> = source[offset + 1..end].to_vec();
    let name_end = tag_name(&body, 0)?;
    let name = lower(&slice(&body, 0, name_end));
    // `/\s*\z`: последний `/`, за которым только пробелы.
    let trimmed = body.iter().rposition(|c| !c.is_whitespace());
    let self_closing = trimmed.is_some_and(|at| body[at] == '/');
    if self_closing {
        body.truncate(trimmed.expect("checked"));
    }
    let mut event = Event {
        kind: 0,
        name: Some(name),
        self_closing: Some(self_closing),
        ..Event::default()
    };
    attributes(&body, name_end, &mut event);
    Some((event, end + 1))
}

fn index_of(source: &[char], needle: &str, from: usize) -> Option<usize> {
    let needle: Vec<char> = needle.chars().collect();
    let mut at = from;
    while at + needle.len() <= source.len() {
        if source[at..at + needle.len()] == needle[..] {
            return Some(at);
        }
        at += 1;
    }
    None
}

fn end_tag(source: &[char], offset: usize) -> Option<(Event, usize)> {
    if slice(source, offset, source.len().min(offset + 2)) != "</" {
        return None;
    }
    let end = index_of(source, ">", offset + 2)?;
    let body = &source[offset + 2..end];
    let start = body.iter().position(|c| !c.is_whitespace()).unwrap_or(body.len());
    let name_end = tag_name(body, start)?;
    Some((
        Event {
            kind: 1,
            name: Some(lower(&slice(body, start, name_end))),
            ..Event::default()
        },
        end + 1,
    ))
}

fn same_letter(a: char, b: char) -> bool {
    a == b
        || a.to_uppercase().eq(b.to_uppercase())
        || a.to_lowercase().eq(b.to_lowercase())
}

/// `</\s*name(?=[\s/>])` без учёта регистра: смещение начала.
fn raw_close(tail: &[char], name: &str) -> Option<usize> {
    let name: Vec<char> = name.chars().collect();
    (0..tail.len()).find(|start| {
        if tail.get(*start) != Some(&'<') || tail.get(start + 1) != Some(&'/') {
            return false;
        }
        let mut at = start + 2;
        while at < tail.len() && tail[at].is_whitespace() {
            at += 1;
        }
        if at + name.len() > tail.len() || !tail[at..at + name.len()].iter().zip(&name).all(|(a, b)| same_letter(*a, *b)) {
            return false;
        }
        tail.get(at + name.len())
            .is_some_and(|c| c.is_whitespace() || *c == '/' || *c == '>')
    })
}

fn tokenize(origin: &str) -> Vec<Event> {
    let source: Vec<char> = origin.chars().collect();
    let mut events = Vec::new();
    let mut offset = 0;
    let mut raw_name: Option<String> = None;
    while offset < source.len() {
        if let Some(name) = raw_name.take() {
            let tail = &source[offset..];
            match raw_close(tail, &name) {
                Some(position) => {
                    push_text(&mut events, &slice(&source, offset, offset + position), true);
                    offset += position;
                    continue;
                }
                None => {
                    push_text(&mut events, &slice(&source, offset, source.len()), true);
                    break;
                }
            }
        }
        let four = slice(&source, offset, source.len().min(offset + 4));
        let two = slice(&source, offset, source.len().min(offset + 2));
        if four == "<!--" {
            match index_of(&source, "-->", offset + 4) {
                None => {
                    events.push(Event {
                        kind: 3,
                        value: Some(slice(&source, offset + 4, source.len())),
                        ..Event::default()
                    });
                    break;
                }
                Some(end) => {
                    events.push(Event {
                        kind: 3,
                        value: Some(slice(&source, offset + 4, end)),
                        ..Event::default()
                    });
                    offset = end + 3;
                    continue;
                }
            }
        }
        if two == "<?" {
            let (end, ending) = match index_of(&source, "?>", offset + 2) {
                Some(end) => (end, 2),
                None => match index_of(&source, ">", offset + 2) {
                    Some(end) => (end, 1),
                    None => (source.len(), 0),
                },
            };
            let body = slice(&source, offset + 2, end).trim().to_string();
            if body.is_empty() {
                push_text(&mut events, &slice(&source, offset, end + ending), false);
            } else {
                let split = body.find(char::is_whitespace);
                let (target, data) = match split {
                    Some(at) => (&body[..at], body[at..].trim_start()),
                    None => (body.as_str(), ""),
                };
                events.push(Event {
                    kind: 5,
                    target: Some(target.to_string()),
                    data: Some(data.to_string()),
                    ..Event::default()
                });
            }
            offset = end + ending;
            continue;
        }
        if two == "<!" {
            match index_of(&source, ">", offset + 2) {
                None => {
                    events.push(Event {
                        kind: 4,
                        value: Some(slice(&source, offset + 2, source.len()).trim().to_string()),
                        ..Event::default()
                    });
                    break;
                }
                Some(end) => {
                    events.push(Event {
                        kind: 4,
                        value: Some(slice(&source, offset + 2, end).trim().to_string()),
                        ..Event::default()
                    });
                    offset = end + 1;
                    continue;
                }
            }
        }
        if two == "</"
            && let Some((event, next)) = end_tag(&source, offset) {
                events.push(event);
                offset = next;
                continue;
            }
        if source[offset] == '<'
            && let Some((event, next)) = start_tag(&source, offset) {
                let name = event.name.clone().expect("start tag has a name");
                let raw = event.self_closing == Some(false) && (name == "script" || name == "style");
                events.push(event);
                offset = next;
                if raw {
                    raw_name = Some(name);
                }
                continue;
            }
        let following = index_of(&source, "<", offset + 1).unwrap_or(source.len());
        push_text(&mut events, &slice(&source, offset, following), false);
        offset = following;
    }
    events
}

#[derive(Debug)]
struct Events(Vec<Event>);

fn html_execute(native: Native, name: &str, args: &[Value]) -> Result<Value, Error> {
    if name == "html_events" {
        return Ok(new_handle(Events(tokenize(string(&args[0])?))));
    }
    let events = &handle::<Events>(&args[0])?.0;
    if name == "html_event_count" {
        return Ok(int_value(events.len() as i64));
    }
    let event = super::support::small(int(&args[1])?, events.len())
        .map(|at| &events[at])
        .ok_or_else(|| {
            Error::Failure(crate::abi::NativeFailure {
                domain: "core.html".into(),
                code: "invalid_input".into(),
                operation: "runtime/html@1::html_event/2".into(),
                message: "HTML event index is outside the stream".into(),
            })
        })?;
    let missing = |field: &str| {
        failure(
            "core.html",
            "invalid_input",
            native,
            format!("HTML event has no {field}"),
        )
    };
    let field = |value: &Option<String>, label: &str| value.clone().map(text).ok_or_else(|| missing(label));
    match name {
        "html_event_kind" => Ok(int_value(event.kind)),
        "html_event_name" => field(&event.name, "name"),
        "html_event_value" => field(&event.value, "value"),
        "html_event_target" => field(&event.target, "target"),
        "html_event_data" => field(&event.data, "data"),
        "html_event_self_closing" => event
            .self_closing
            .map(Value::Bool)
            .ok_or_else(|| missing("self_closing")),
        "html_attribute_count" | "html_attribute_name" | "html_attribute_valued"
        | "html_attribute_value" => {
            if event.kind != 0 {
                return Err(failure(
                    "core.html",
                    "invalid_input",
                    native,
                    "HTML event has no attributes",
                ));
            }
            if name == "html_attribute_count" {
                return Ok(int_value(event.attributes.len() as i64));
            }
            let at = super::support::small(int(&args[2])?, event.attributes.len()).ok_or_else(|| {
                failure(
                    "core.html",
                    "invalid_input",
                    native,
                    "HTML attribute index is outside the tag",
                )
            })?;
            Ok(match name {
                "html_attribute_name" => text(event.attributes[at].0.clone()),
                "html_attribute_valued" => Value::Bool(event.valued[at]),
                _ => text(event.attributes[at].1.clone()),
            })
        }
        _ => Err(Error::Unsupported(format!(
            "native provider is not implemented: {}",
            native.symbol()
        ))),
    }
}

pub fn execute(native: Native, args: &[Value]) -> Result<Value, Error> {
    html_execute(native, native.name(), args)
}
