//! Shared boolean environment-switch policy (#1067).
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

/// NO_COLOR is a presence convention, not a boolean switch.
pub(crate) fn no_color(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty())
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
