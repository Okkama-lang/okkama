//! Shared environment-switch policy: booleans (#1067) and byte counts (#933).
use std::ffi::OsStr;

#[allow(dead_code)]
pub fn enabled(name: &str) -> bool {
    value_enabled(std::env::var_os(name).as_deref())
}

/// A switch that is on (or off) until set: unset keeps `default`; a set value
/// follows the one rule of [`value_enabled`] (#1067).
#[allow(dead_code)]
pub fn enabled_or(name: &str, default: bool) -> bool {
    value_enabled_or(std::env::var_os(name).as_deref(), default)
}

/// A byte-count switch (#933): unset or empty is `None`; a value that is not
/// a decimal count up to `max` is an error naming the switch, never ignored.
pub fn bytes(name: &str, max: usize) -> Result<Option<usize>, String> {
    value_bytes(name, std::env::var_os(name).as_deref(), max)
}

pub(crate) fn value_bytes(name: &str, value: Option<&OsStr>, max: usize) -> Result<Option<usize>, String> {
    let Some(value) = value.filter(|value| !value.is_empty()) else { return Ok(None) };
    let text = value.to_string_lossy();
    match text.parse::<usize>() {
        Ok(count) if count <= max && text.bytes().all(|b| b.is_ascii_digit()) => Ok(Some(count)),
        _ => Err(format!("{name}: expected a byte count from 0 to {max}, got {text:?}")),
    }
}

pub(crate) fn value_enabled_or(value: Option<&OsStr>, default: bool) -> bool {
    match value {
        None => default,
        value => value_enabled(value),
    }
}

pub(crate) fn value_enabled(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| {
        let bytes = value.as_encoded_bytes();
        !bytes.is_empty() && ![b"0".as_slice(), b"false", b"off", b"no"]
            .iter().any(|off| bytes.eq_ignore_ascii_case(off))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_switch_keeps_its_default() {
        assert!(value_enabled_or(None, true));
        assert!(!value_enabled_or(None, false));
        assert!(!value_enabled_or(Some(OsStr::new("off")), true));
        assert!(value_enabled_or(Some(OsStr::new("1")), false));
    }

    #[test]
    fn byte_count_switch_values() {
        let read = |value: Option<&str>| value_bytes("S", value.map(OsStr::new), 4096);
        assert_eq!(read(None), Ok(None));
        assert_eq!(read(Some("")), Ok(None));
        assert_eq!(read(Some("0")), Ok(Some(0)));
        assert_eq!(read(Some("192")), Ok(Some(192)));
        assert_eq!(read(Some("4096")), Ok(Some(4096)));
        for bad in ["4097", "-1", "+64", " 64", "64 ", "0x40", "64k", "on"] {
            let error = read(Some(bad)).unwrap_err();
            assert!(error.starts_with("S: expected a byte count from 0 to 4096"), "{bad:?}: {error}");
        }
    }

    #[test]
    fn boolean_switch_values() {
        assert!(!value_enabled(None));
        for value in ["", "0", "false", "FALSE", "False", "off", "OFF", "no", "No"] {
            assert!(!value_enabled(Some(OsStr::new(value))), "{value:?}");
        }
        for value in ["1", "true", "TRUE", "on", "yes", "2", "anything", " ", " false "] {
            assert!(value_enabled(Some(OsStr::new(value))), "{value:?}");
        }
    }
}
