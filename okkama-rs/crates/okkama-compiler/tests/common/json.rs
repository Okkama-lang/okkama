//! A minimal JSON reader for test metadata (`case.json` of the corpus).

#![allow(dead_code)]

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(map) => map.get(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_array(&self) -> &[Json] {
        match self {
            Json::Array(items) => items,
            _ => &[],
        }
    }
}

pub fn parse(text: &str) -> Result<Json, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut pos = 0;
    let value = value(&chars, &mut pos, 0)?;
    skip_ws(&chars, &mut pos);
    if pos != chars.len() {
        return Err(format!("trailing data at {pos}"));
    }
    Ok(value)
}

fn skip_ws(c: &[char], pos: &mut usize) {
    while *pos < c.len() && c[*pos].is_whitespace() {
        *pos += 1;
    }
}

/// Nesting deeper than this is refused rather than recursed into.
const DEPTH: usize = 256;

fn value(c: &[char], pos: &mut usize, depth: usize) -> Result<Json, String> {
    if depth > DEPTH {
        return Err(format!("nesting deeper than {DEPTH} at {pos}"));
    }
    skip_ws(c, pos);
    match c.get(*pos) {
        Some('{') => {
            *pos += 1;
            let mut map = BTreeMap::new();
            skip_ws(c, pos);
            if c.get(*pos) == Some(&'}') {
                *pos += 1;
                return Ok(Json::Object(map));
            }
            loop {
                skip_ws(c, pos);
                let key = string(c, pos)?;
                skip_ws(c, pos);
                if c.get(*pos) != Some(&':') {
                    return Err(format!("expected ':' at {pos}"));
                }
                *pos += 1;
                map.insert(key, value(c, pos, depth + 1)?);
                skip_ws(c, pos);
                match c.get(*pos) {
                    Some(',') => *pos += 1,
                    Some('}') => {
                        *pos += 1;
                        return Ok(Json::Object(map));
                    }
                    _ => return Err(format!("bad object at {pos}")),
                }
            }
        }
        Some('[') => {
            *pos += 1;
            let mut items = Vec::new();
            skip_ws(c, pos);
            if c.get(*pos) == Some(&']') {
                *pos += 1;
                return Ok(Json::Array(items));
            }
            loop {
                items.push(value(c, pos, depth + 1)?);
                skip_ws(c, pos);
                match c.get(*pos) {
                    Some(',') => *pos += 1,
                    Some(']') => {
                        *pos += 1;
                        return Ok(Json::Array(items));
                    }
                    _ => return Err(format!("bad array at {pos}")),
                }
            }
        }
        Some('"') => Ok(Json::String(string(c, pos)?)),
        Some('t') => word(c, pos, "true", Json::Bool(true)),
        Some('f') => word(c, pos, "false", Json::Bool(false)),
        Some('n') => word(c, pos, "null", Json::Null),
        Some(ch) if ch.is_ascii_digit() || *ch == '-' => {
            let start = *pos;
            *pos += 1;
            while *pos < c.len() && (c[*pos].is_ascii_digit() || ".eE+-".contains(c[*pos])) {
                *pos += 1;
            }
            let text: String = c[start..*pos].iter().collect();
            text.parse()
                .map(Json::Number)
                .map_err(|_| format!("bad number {text}"))
        }
        _ => Err(format!("unexpected input at {pos}")),
    }
}

fn word(c: &[char], pos: &mut usize, text: &str, value: Json) -> Result<Json, String> {
    let end = *pos + text.chars().count();
    if end <= c.len() && c[*pos..end].iter().collect::<String>() == text {
        *pos = end;
        Ok(value)
    } else {
        Err(format!("bad literal at {pos}"))
    }
}

fn string(c: &[char], pos: &mut usize) -> Result<String, String> {
    if c.get(*pos) != Some(&'"') {
        return Err(format!("expected string at {pos}"));
    }
    *pos += 1;
    let mut out = String::new();
    while let Some(&ch) = c.get(*pos) {
        *pos += 1;
        match ch {
            '"' => return Ok(out),
            '\\' => {
                let escape = *c.get(*pos).ok_or("unterminated escape")?;
                *pos += 1;
                match escape {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    '/' => out.push('/'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'u' => {
                        let hex: String =
                            c.get(*pos..*pos + 4).ok_or("short \\u")?.iter().collect();
                        *pos += 4;
                        let mut code = u32::from_str_radix(&hex, 16).map_err(|e| e.to_string())?;
                        // A low surrogate follows only as `\uDC00`–`\uDFFF`; anything
                        // else leaves the high one unpaired.
                        if (0xD800..0xDC00).contains(&code)
                            && c.get(*pos) == Some(&'\\')
                            && c.get(*pos + 1) == Some(&'u')
                            && let Some(low) = c
                                .get(*pos + 2..*pos + 6)
                                .and_then(|low| u32::from_str_radix(&low.iter().collect::<String>(), 16).ok())
                                .filter(|low| (0xDC00..0xE000).contains(low))
                        {
                            *pos += 6;
                            code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                        }
                        out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                    }
                    other => return Err(format!("bad escape \\{other}")),
                }
            }
            other => out.push(other),
        }
    }
    Err("unterminated string".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surrogates_pair_only_with_a_low_escape_and_nesting_is_bounded() {
        assert_eq!(parse(r#""\uD83D\uDE00""#), Ok(Json::String("\u{1F600}".into())));
        assert_eq!(parse(r#""\uD83D\n0041""#), Ok(Json::String("\u{fffd}\n0041".into())));
        assert_eq!(parse(r#""\uD83D\u0041""#), Ok(Json::String("\u{fffd}A".into())));
        assert!(parse(&"[".repeat(DEPTH + 2)).is_err());
        assert!(parse(&format!("{}{}", "[".repeat(8), "]".repeat(8))).is_ok());
    }
}

