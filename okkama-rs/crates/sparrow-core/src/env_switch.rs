//! Shared boolean environment-switch policy (#1067).
use std::ffi::OsStr;

pub fn enabled(name: &str) -> bool {
    value_enabled(std::env::var_os(name).as_deref())
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
