//! Native providers Sparrow does not implement itself. The only entry is
//! [`execute`], called by Sparrow's bridge (`sparrow-core/src/host/native_origin.rs`)
//! for the natives its host routes to `ORIGIN`; every other native has a
//! Sparrow provider and never reaches this crate (#1016). A native without a
//! provider here is an explicit [`Error::Unsupported`]. `fs` and `regex` also
//! hold the few items Sparrow's own providers call directly. Native identifiers
//! and signatures live in runtime-abi.
use crate::{
    Value, ValueError,
    abi::{Native, NativeFailure},
    heap::ArrayHeap,
};
pub(crate) mod application;
pub(crate) mod cldr;
pub(crate) mod collation;
pub(crate) mod compression;
pub(crate) mod formats;
pub mod fs;
pub(crate) mod host;
pub(crate) mod idna;
pub(crate) mod json;
pub(crate) mod json_split;
pub(crate) mod math;
pub(crate) mod misc;
pub(crate) mod net;
pub mod regex;
pub(crate) mod signal;
pub(crate) mod text;
pub(crate) mod time;
pub(crate) mod unicode;
pub(crate) mod value;
pub(crate) mod x509;

pub(crate) mod inspect;
pub(crate) mod support;

/// Provider state belongs to one execution, not a process-global VM singleton.
#[derive(Debug, Default)]
pub struct Context {
    /// Имена enum программы без модуля и их case (`debug.data_dump`).
    pub enums: Vec<(String, Vec<String>)>,
    /// Имена объектов программы и их полей по порядку (`debug/dumper`).
    pub objects: Vec<(String, Vec<String>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidState(String),
    /// No provider for this native here: Sparrow provides it itself.
    Unsupported(String),
    Trap(String),
    Failure(NativeFailure),
}
impl From<ValueError> for Error {
    fn from(error: ValueError) -> Self {
        Self::InvalidState(error.message)
    }
}

pub fn execute(
    native: Native,
    args: &[Value],
    arrays: &mut ArrayHeap,
    context: &mut Context,
) -> Result<Value, Error> {
    if args.len() != native.parameters().len() {
        return Err(Error::InvalidState(
            "native argument count does not match ABI".into(),
        ));
    }
    match provider(native) {
        Some(Provider::Misc) => misc::execute(native, args, context),
        Some(Provider::Json) => json::execute(native, args, arrays),
        Some(Provider::Math) => math::execute(native, args, context),
        Some(Provider::Host) => host::execute(native, args),
        Some(Provider::Value) => value::execute(native, args),
        Some(Provider::Unicode) => unicode::execute(native, args),
        Some(Provider::X509) => x509::execute(native, args),
        Some(Provider::Application) => application::execute(native, args),
        Some(Provider::Collation) => collation::execute(native, args),
        Some(Provider::Time) => time::execute(native, args),
        Some(Provider::Formats) => formats::execute(native, args),
        Some(Provider::Text) => text::execute(native, args),
        Some(Provider::Compression) => compression::execute(native, args, arrays),
        Some(Provider::Signal) => signal::execute(native, args),
        Some(Provider::Cldr) => cldr::execute(native, args),
        None => Err(not_provided(native)),
    }
}

/// Whether [`execute`] has a provider for `native`; the others are refused
/// with [`Error::Unsupported`]. Sparrow checks its `ORIGIN` routes against it.
pub fn provides(native: Native) -> bool {
    provider(native).is_some()
}

/// The refusal for a native Sparrow provides itself (#1016).
fn not_provided(native: Native) -> Error {
    Error::Unsupported(format!(
        "native is not provided by runtime-core: {}",
        native.symbol()
    ))
}

/// Provider, исполняющий native.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    Misc,
    Json,
    Math,
    Host,
    Value,
    Unicode,
    X509,
    Application,
    Collation,
    Time,
    Formats,
    Text,
    Compression,
    Signal,
    Cldr,
}

/// Provider native по таблице, заполненной один раз [`route`]: при вызове —
/// индекс, а не сравнение строк модуля.
fn provider(native: Native) -> Option<Provider> {
    static PROVIDERS: std::sync::OnceLock<Vec<Option<Provider>>> = std::sync::OnceLock::new();
    PROVIDERS.get_or_init(|| {
        let mut providers = Vec::new();
        for native in Native::ALL {
            let at = *native as usize;
            if providers.len() <= at {
                providers.resize(at + 1, None);
            }
            providers[at] = route(*native);
        }
        providers
    })[native as usize]
}

/// Providers по модулю символа (`runtime/x509@1::…` → `runtime/x509`): only
/// the natives Sparrow's host routes to `ORIGIN` (`sparrow-core/src/host/mod.rs`,
/// `classify`). The rest are `None`.
fn route(native: Native) -> Option<Provider> {
    let name = native.name();
    Some(match native.module() {
        "runtime/crypto" | "runtime/debug" | "runtime/diff" | "runtime/math_aggregate"
        | "runtime/layout" => Provider::Misc,
        "runtime/json" if name.starts_with("json_split_") => Provider::Json,
        "runtime/format" | "runtime/statistics" | "runtime/random_distribution" => Provider::Math,
        "runtime/random" if name.starts_with("pcg_") => Provider::Math,
        "runtime/random" => Provider::Host,
        "runtime/system"
            if !matches!(
                native,
                Native::SystemSystemPlatform
                    | Native::SystemSystemArchitecture
                    | Native::SystemSystemByteOrder
            ) =>
        {
            Provider::Host
        }
        "runtime/uuid" | "runtime/unicode_normalization" => Provider::Value,
        "runtime/unicode_character" => Provider::Unicode,
        "runtime/x509" => Provider::X509,
        "runtime/application" | "runtime/account" => Provider::Application,
        "runtime/collation" | "runtime/locale" => Provider::Collation,
        "runtime/time_zone" => Provider::Time,
        "runtime/io_decorators" | "runtime/pem" | "runtime/template" => Provider::Formats,
        "runtime/hpack" | "runtime/collections" => Provider::Text,
        "runtime/number_text" if name.starts_with("float_parse") => Provider::Text,
        _ if compression::handles(native) => Provider::Compression,
        _ if signal::handles(native) => Provider::Signal,
        _ if cldr::handles(native) => Provider::Cldr,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1016: a native Sparrow provides itself is refused by name, not run by
    /// a twin provider; the routed ones still reach theirs.
    #[test]
    fn natives_without_a_provider_here_are_refused_explicitly() {
        let mut context = Context::default();
        for native in [Native::Utf8Encode, Native::StdoutFlush, Native::SystemSystemPlatform] {
            let args = vec![Value::Str("x".into()); native.parameters().len()];
            assert_eq!(provider(native), None, "{}", native.symbol());
            let refused = execute(native, &args, &mut ArrayHeap, &mut context).err();
            assert_eq!(refused, Some(not_provided(native)), "{}", native.symbol());
        }
        let routed = execute(Native::SystemSystemCpuCount, &[], &mut ArrayHeap, &mut context);
        assert!(matches!(routed, Ok(Value::Int(_))), "{routed:?}");
        assert_eq!(provider(Native::X509SystemCertificates), Some(Provider::X509));
        assert_eq!(provider(Native::JsonJsonSplitState), Some(Provider::Json));
        assert_eq!(provider(Native::JsonJsonParseState), None);
    }
}
