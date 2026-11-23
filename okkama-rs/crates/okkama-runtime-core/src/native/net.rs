//! Numeric IP address text (`inet_pton`/`inet_ntop`) for the name checks of
//! `x509`. The `runtime/net` natives are Sparrow's own provider
//! (`host/native_net.rs`).

pub fn parse_ipv4(text: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut out = [0u8; 4];
    for (index, part) in parts.iter().enumerate() {
        let canonical = *part == "0"
            || (!part.is_empty() && part.len() <= 3 && !part.starts_with('0') && part.bytes().all(|b| b.is_ascii_digit()));
        if !canonical {
            return None;
        }
        out[index] = u8::try_from(part.parse::<u32>().ok()?).ok()?;
    }
    Some(out)
}

/// Группы `h16` через `:`; последняя может быть IPv4 (две группы).
fn groups(text: &str, out: &mut Vec<u16>, last: bool) -> bool {
    if text.is_empty() {
        return true;
    }
    let parts: Vec<&str> = text.split(':').collect();
    for (index, part) in parts.iter().enumerate() {
        if last && index == parts.len() - 1 && part.contains('.') {
            let Some(four) = parse_ipv4(part) else {
                return false;
            };
            out.push(u16::from_be_bytes([four[0], four[1]]));
            out.push(u16::from_be_bytes([four[2], four[3]]));
            continue;
        }
        if part.is_empty() || part.len() > 4 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
        out.push(u16::from_str_radix(part, 16).expect("hex group"));
    }
    true
}

pub fn parse_ipv6(text: &str) -> Option<[u8; 16]> {
    let gap = text.find("::");
    if let Some(at) = gap
        && text[at + 1..].contains("::") {
            return None;
        }
    let (mut head, mut tail) = (Vec::new(), Vec::new());
    match gap {
        Some(at) => {
            if !groups(&text[..at], &mut head, false) || !groups(&text[at + 2..], &mut tail, true) {
                return None;
            }
            if head.len() + tail.len() > 7 {
                return None;
            }
        }
        None => {
            if !groups(text, &mut head, true) || head.len() != 8 {
                return None;
            }
        }
    }
    let mut out = [0u8; 16];
    for (index, word) in head.iter().enumerate() {
        out[index * 2..index * 2 + 2].copy_from_slice(&word.to_be_bytes());
    }
    let base = 8 - tail.len();
    for (index, word) in tail.iter().enumerate() {
        out[(base + index) * 2..(base + index) * 2 + 2].copy_from_slice(&word.to_be_bytes());
    }
    Some(out)
}

/// `inet_ntop` BSD: самый длинный ряд ≥ 2 нулевых групп — `::`.
pub fn format(raw: &[u8]) -> String {
    if raw.len() == 4 {
        return format!("{}.{}.{}.{}", raw[0], raw[1], raw[2], raw[3]);
    }
    let words: Vec<u16> = (0..8).map(|index| u16::from_be_bytes([raw[index * 2], raw[index * 2 + 1]])).collect();
    let (mut best, mut best_length): (i64, usize) = (-1, 0);
    let mut index = 0;
    while index < 8 {
        if words[index] != 0 {
            index += 1;
            continue;
        }
        let start = index;
        while index < 8 && words[index] == 0 {
            index += 1;
        }
        if index - start > best_length {
            best = start as i64;
            best_length = index - start;
        }
    }
    if best_length < 2 {
        best = -1;
    }
    let mut out = String::new();
    for index in 0..8 {
        if best >= 0 && (index as i64) >= best && (index as i64) < best + best_length as i64 {
            if index as i64 == best {
                out.push(':');
            }
            continue;
        }
        if index != 0 {
            out.push(':');
        }
        if index == 6
            && best == 0
            && (best_length == 6
                || (best_length == 7 && words[7] != 0x0001)
                || (best_length == 5 && words[5] == 0xffff))
        {
            out.push_str(&format!("{}.{}.{}.{}", raw[12], raw[13], raw[14], raw[15]));
            break;
        }
        out.push_str(&format!("{:x}", words[index]));
    }
    if best >= 0 && best as usize + best_length == 8 {
        out.push(':');
    }
    out
}
