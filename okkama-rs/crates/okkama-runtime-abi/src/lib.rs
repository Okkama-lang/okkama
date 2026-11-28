//! Native ABI metadata only: no compiler, VM, runtime values, or host I/O.

pub mod catalogue;

/// Тип ABI: тип Okkama параметра или результата provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    Int,
    Float,
    Rat,
    Bool,
    Byte,
    Char,
    Str,
    Any,
    Void,
    /// Непрозрачное значение provider (`native object`), названное полностью: `runtime.buffer::BufferState`, как
    /// SPB называет handle (#1083: короткие имена разных модулей сталкивались, правило #976 снято).
    Handle(&'static str),
    Array(Box<Type>),
    Map(Box<Type>, Box<Type>),
    /// `?T`: `Maybe<T>` (#1083), один уровень.
    Maybe(Box<Type>),
}

impl Type {
    /// `@byte`.
    pub fn bytes() -> Self {
        Self::Array(Box::new(Self::Byte))
    }

    /// Тип в начале `text` и остаток; ошибка, если типа нет.
    fn parse(text: &'static str) -> Result<(Self, &'static str), String> {
        let text = text.trim_start();
        if let Some(rest) = text.strip_prefix('@') {
            let (item, rest) = Self::parse(rest)?;
            return Ok((Self::Array(Box::new(item)), rest));
        }
        if let Some(rest) = text.strip_prefix('?') {
            let (item, rest) = Self::parse(rest)?;
            if matches!(item, Self::Maybe(_) | Self::Void) {
                return Err(format!("`?` applies to a value type once, not to `{}`", text.trim()));
            }
            return Ok((Self::Maybe(Box::new(item)), rest));
        }
        if let Some(rest) = text.strip_prefix('%') {
            let (key, rest) = Self::parse(rest)?;
            let rest = rest.strip_prefix(':').ok_or_else(|| format!("a map type needs `:` in `%{}`", rest.trim()))?;
            let (value, rest) = Self::parse(rest)?;
            return Ok((Self::Map(Box::new(key), Box::new(value)), rest));
        }
        // A name: letters, digits and `_`, joined by `.` or `::` (a handle's module path).
        let bytes = text.as_bytes();
        let mut end = 0;
        while end < bytes.len() {
            let byte = bytes[end];
            if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.' {
                end += 1;
            } else if bytes[end..].starts_with(b"::") && end > 0 {
                end += 2;
            } else {
                break;
            }
        }
        let (name, rest) = text.split_at(end);
        let ty = match name {
            "" => return Err(format!("expected a type at `{}`", text.trim())),
            "int" => Self::Int,
            "float" => Self::Float,
            "rat" => Self::Rat,
            "bool" => Self::Bool,
            "byte" => Self::Byte,
            "char" => Self::Char,
            "string" => Self::Str,
            "any" => Self::Any,
            "void" => Self::Void,
            handle if handle.ends_with("::") || handle.starts_with('.') || handle.ends_with('.') => {
                return Err(format!("`{handle}` is not a type name"));
            }
            handle => Self::Handle(handle),
        };
        Ok((ty, rest))
    }
}

/// Сигнатура provider: параметры, результат; `async` — результат Future.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub is_async: bool,
    pub params: Vec<Type>,
    pub result: Type,
}

impl Signature {
    /// Сигнатура таблицы std: её строки проверены генератором, ошибка разбора — ошибка сборки.
    fn parse(text: &'static str) -> Self {
        Self::try_parse(text).unwrap_or_else(|error| panic!("native signature `{text}`: {error}"))
    }

    /// #1083: `[async ](T, …) -> R` из внешнего источника (расширение): ошибка вместо паники. `void` — только
    /// результат; после результата ничего.
    pub fn try_parse(text: &'static str) -> Result<Self, String> {
        let (is_async, text) = match text.strip_prefix("async ") {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (params, result) = text
            .trim_start()
            .strip_prefix('(')
            .and_then(|rest| rest.split_once(") -> "))
            .ok_or_else(|| "a signature is `(params) -> result`".to_string())?;
        let mut list = Vec::new();
        if !params.trim().is_empty() {
            let mut rest = params;
            loop {
                let (ty, tail) = Type::parse(rest)?;
                if ty == Type::Void {
                    return Err("`void` is no parameter type".into());
                }
                list.push(ty);
                let tail = tail.trim_start();
                match tail.strip_prefix(',') {
                    Some(tail) => rest = tail,
                    None if tail.is_empty() => break,
                    None => return Err(format!("unexpected `{tail}` in the parameters")),
                }
            }
        }
        let (result, rest) = Type::parse(result)?;
        if !rest.trim().is_empty() {
            return Err(format!("unexpected `{}` after the result", rest.trim()));
        }
        Ok(Self { is_async, params: list, result })
    }
}

/// Таблица native: имя, стабильный wire ID, символ, сигнатура Okkama.
macro_rules! natives {
    ($($name:ident = $id:literal, $symbol:literal, $signature:literal;)*) => {
        /// Stable wire IDs. Never renumber an existing provider operation.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[repr(u32)]
        pub enum Native {
            $($name = $id,)*
        }

        impl Native {
            pub const ALL: &'static [Self] = &[$(Self::$name,)*];

            pub fn symbol(self) -> &'static str {
                match self {
                    $(Self::$name => $symbol,)*
                }
            }

            /// The signature as the table writes it: `(string, int) -> int`.
            pub fn signature_text(self) -> &'static str {
                match self {
                    $(Self::$name => $signature,)*
                }
            }
        }
    };
}

include!("table.rs");

impl Native {
    /// Сигнатура provider (разбирается один раз).
    pub fn signature(self) -> &'static Signature {
        static SIGNATURES: std::sync::OnceLock<Vec<Option<Signature>>> = std::sync::OnceLock::new();
        SIGNATURES
            .get_or_init(|| {
                let mut signatures: Vec<Option<Signature>> = Vec::new();
                for native in Self::ALL {
                    let at = *native as usize;
                    if signatures.len() <= at {
                        signatures.resize_with(at + 1, || None);
                    }
                    signatures[at] = Some(Signature::parse(native.signature_text()));
                }
                signatures
            })[self as usize]
            .as_ref()
            .expect("every native has a signature")
    }

    /// Модуль символа без версии: `runtime/utf@1::utf8_encode/1` → `runtime/utf`.
    pub fn module(self) -> &'static str {
        Self::parts()[self as usize].0
    }

    /// Имя функции без модуля и арности: `runtime/utf@1::utf8_encode/1` → `utf8_encode`.
    pub fn name(self) -> &'static str {
        Self::parts()[self as usize].1
    }

    /// Модуль и имя каждого символа по номеру natives — вычисляются один раз,
    /// а не разбором строки при каждом вызове.
    fn parts() -> &'static [(&'static str, &'static str)] {
        static PARTS: std::sync::OnceLock<Vec<(&'static str, &'static str)>> = std::sync::OnceLock::new();
        PARTS.get_or_init(|| {
            let mut parts = Vec::new();
            for native in Self::ALL {
                let at = *native as usize;
                if parts.len() <= at {
                    parts.resize(at + 1, ("", ""));
                }
                let symbol = native.symbol();
                let module = symbol.split('@').next().unwrap_or("");
                let name = symbol
                    .split("::")
                    .nth(1)
                    .and_then(|rest| rest.split('/').next())
                    .unwrap_or("");
                parts[at] = (module, name);
            }
            parts
        })
    }

    pub fn parameters(self) -> &'static [Type] {
        &self.signature().params
    }

    pub fn result(self) -> &'static Type {
        &self.signature().result
    }

    pub fn resolve(symbol: &str) -> Option<Self> {
        static SYMBOLS: std::sync::OnceLock<std::collections::HashMap<&'static str, Native>> =
            std::sync::OnceLock::new();
        SYMBOLS
            .get_or_init(|| Self::ALL.iter().map(|n| (n.symbol(), *n)).collect())
            .get(symbol)
            .copied()
    }
}

/// Provider failure: fields of the language's NativeFailure object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeFailure {
    pub domain: String,
    pub code: String,
    pub operation: String,
    pub message: String,
}
impl std::fmt::Display for NativeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NativeFailure: {}", self.message)
    }
}
impl std::error::Error for NativeFailure {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_ids_and_symbols_are_unique() {
        let mut ids: Vec<u32> = Native::ALL.iter().map(|native| *native as u32).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Native::ALL.len());
        let mut symbols: Vec<&str> = Native::ALL.iter().map(|native| native.symbol()).collect();
        symbols.sort_unstable();
        symbols.dedup();
        assert_eq!(symbols.len(), Native::ALL.len());
        for native in Native::ALL {
            let arity: usize = native.symbol().rsplit('/').next().unwrap().parse().unwrap();
            assert_eq!(arity, native.parameters().len(), "{}", native.symbol());
            assert_eq!(Native::resolve(native.symbol()), Some(*native));
        }
    }

    /// #1083: the signature language, as an extension's signatures will use it: full handle names, `?T`, and an error
    /// instead of a panic for anything malformed.
    #[test]
    fn signatures_parse_or_say_why_not() {
        let parsed = Signature::try_parse("async (runtime.buffer::BufferState, ?@byte, %string:?int) -> ?Image").unwrap();
        assert!(parsed.is_async);
        assert_eq!(
            parsed.params,
            vec![
                Type::Handle("runtime.buffer::BufferState"),
                Type::Maybe(Box::new(Type::bytes())),
                Type::Map(Box::new(Type::Str), Box::new(Type::Maybe(Box::new(Type::Int)))),
            ]
        );
        assert_eq!(parsed.result, Type::Maybe(Box::new(Type::Handle("Image"))));
        assert_eq!(Signature::try_parse("() -> void").unwrap().params, vec![]);
        for bad in [
            "(int",
            "int -> int",
            "(int,) -> int",
            "(void) -> int",
            "(int) -> ??int",
            "(int) -> ?void",
            "(%int) -> int",
            "(int) -> int extra",
            "(::Name) -> int",
            "(runtime.buffer::) -> int",
            "() -> ",
        ] {
            assert!(Signature::try_parse(bad).is_err(), "{bad}");
        }
        // Every signature of the std table goes through the same parser.
        for native in Native::ALL {
            assert!(Signature::try_parse(native.signature_text()).is_ok(), "{}", native.symbol());
        }
    }
}
