//! Companion catalogue of the native table (#569 C7; OIR-2 §11, P2.4 §4.7).
//!
//! The generated table gives each provider operation its canonical symbol,
//! stable wire ID and Okkama signature. This catalogue adds what the table does
//! not carry and what both the compiler (own, verifier) and the loader need:
//! - the ABI kind: a synchronous call, or an asynchronous one (`Future` result);
//! - the transport: data only, data with native handles, or opaque `any`;
//! - the handle role: a handle belongs to the native registry. The provider
//!   registers a handle it returns before the result is published; no handle
//!   carries an Okkama destroy token. Closing is an explicit native operation;
//! - the effect: pure computation, the standard streams, or the host;
//! - the failures: every provider may refuse with a `NativeFailure`.
//!
//! Every operation of the table is classified: [`classify`] is total, and the
//! module lists below are exhaustive (a test fails for a module in no list).
//! A data or handle operation is admitted: a synchronous one to the general
//! synchronous native ABI, an asynchronous one to the async native ABI (#601 N1,
//! async ABI D22): its call creates a Future that a runtime driver runs, whose
//! creation packet is empty (ruling PR-N) because data and handles carry no
//! ownership graph. [`Contract::admission`] names the reason otherwise.
//! A source declaration is admitted only when its signature equals the table's.
use crate::{Native, Signature, Type};

/// How the provider is called.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AbiKind {
    Sync,
    /// The result is a Future run by a driver (async ABI D22, #601 N1).
    Async,
}

/// What crosses the boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Transport {
    /// Scalars, strings, arrays and maps of them: no language ownership graph.
    Data,
    /// Data with at least one native handle of the operation's module.
    Handle,
    /// `any` crosses: only a trusted intrinsic bridge may carry it (P2.4 §4.7).
    Opaque,
}

/// What the operation does with handles of the native registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HandleRole {
    None,
    /// Takes handles and returns no handle.
    Uses,
    /// Returns a handle, registered by the provider before publication.
    Publishes,
    /// Closes the registry state of its first argument; idempotent, reports a bool.
    Closes,
}

/// What the operation reaches beyond its arguments and handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Effect {
    Pure,
    /// Standard output and standard error.
    Stdio,
    /// Files, network, processes, environment, clocks, randomness, terminal or standard input.
    Host,
}

/// Failures the provider may report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failures {
    /// A provider refusal: a `NativeFailure` (also a missing provider or a refused permission).
    Native,
}

/// Why an operation is not admitted to the general native ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    Opaque,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Opaque => "a native over `any` needs a trusted intrinsic bridge, not the general native ABI",
        })
    }
}

/// The classification of one operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contract {
    pub native: Native,
    pub abi: AbiKind,
    pub transport: Transport,
    pub handle: HandleRole,
    pub effect: Effect,
    pub failures: Failures,
}

impl Contract {
    /// `Ok` for a data or handle operation, synchronous or asynchronous.
    pub fn admission(&self) -> Result<(), Rejection> {
        if self.transport == Transport::Opaque {
            return Err(Rejection::Opaque);
        }
        Ok(())
    }
}

/// Modules whose operations compute over their arguments and handles only.
const PURE: &[&str] = &[
    "runtime/base32", "runtime/base64", "runtime/base85", "runtime/bigint", "runtime/binary",
    "runtime/buffer", "runtime/bytes", "runtime/checksum", "runtime/cldr", "runtime/collation",
    "runtime/collections", "runtime/compression", "runtime/crypto", "runtime/csv_state", "runtime/db",
    "runtime/debug", "runtime/decimal", "runtime/diff", "runtime/format", "runtime/hex",
    "runtime/host_crypto", "runtime/hpack", "runtime/html", "runtime/idna", "runtime/io_decorators",
    "runtime/json", "runtime/layout", "runtime/math_aggregate", "runtime/number_text", "runtime/pem",
    "runtime/public_suffix", "runtime/quoted_printable", "runtime/random_distribution", "runtime/regex",
    "runtime/secret", "runtime/single_byte", "runtime/statistics", "runtime/template", "runtime/testing",
    "runtime/time_format", "runtime/time_zone", "runtime/toml", "runtime/unicode_character",
    "runtime/unicode_normalization", "runtime/utf", "runtime/uuid", "runtime/x509",
    "runtime/xml",
];

/// Modules whose operations reach the host.
const HOST: &[&str] = &[
    "runtime/account", "runtime/application", "runtime/database", "runtime/environment", "runtime/fs",
    "runtime/fs_root", "runtime/input_state", "runtime/locale", "runtime/net", "runtime/network",
    "runtime/postgres", "runtime/process", "runtime/random", "runtime/signal", "runtime/snapshot",
    "runtime/system", "runtime/time", "runtime/tls_transport",
];

fn effect(native: Native) -> Option<Effect> {
    let module = native.module();
    let name = native.name();
    Some(match module {
        "runtime/stdio" if name.starts_with("stdin") => Effect::Host,
        "runtime/stdio" => Effect::Stdio,
        "runtime/time_zone" if name.starts_with("timezone_local") => Effect::Host,
        "runtime/x509" if name.starts_with("system_certificates") => Effect::Host,
        _ if HOST.contains(&module) => Effect::Host,
        _ if PURE.contains(&module) => Effect::Pure,
        _ => return None,
    })
}

fn mentions(ty: &Type, test: &dyn Fn(&Type) -> bool) -> bool {
    test(ty) || match ty {
        Type::Array(item) => mentions(item, test),
        Type::Map(key, value) => mentions(key, test) || mentions(value, test),
        _ => false,
    }
}

fn types(signature: &Signature) -> impl Iterator<Item = &Type> {
    signature.params.iter().chain(std::iter::once(&signature.result))
}

/// The classification of `native`; `None` only for a module in no effect list.
pub fn classify(native: Native) -> Option<Contract> {
    let signature = native.signature();
    let abi = if signature.is_async { AbiKind::Async } else { AbiKind::Sync };
    let any = |ty: &Type| *ty == Type::Any;
    let handle = |ty: &Type| matches!(ty, Type::Handle(_));
    let transport = if types(signature).any(|ty| mentions(ty, &any)) {
        Transport::Opaque
    } else if types(signature).any(|ty| mentions(ty, &handle)) {
        Transport::Handle
    } else {
        Transport::Data
    };
    let closes = matches!(signature.params.first(), Some(Type::Handle(_)))
        && native.name().ends_with("_close")
        && signature.result == Type::Bool;
    let role = if mentions(&signature.result, &handle) {
        HandleRole::Publishes
    } else if closes {
        HandleRole::Closes
    } else if transport == Transport::Handle {
        HandleRole::Uses
    } else {
        HandleRole::None
    };
    Some(Contract {
        native,
        abi,
        transport,
        handle: role,
        effect: effect(native)?,
        failures: Failures::Native,
    })
}

/// The admitted contract of a canonical symbol of the table, or why there is none.
pub fn admitted(symbol: &str) -> Result<Contract, Admission> {
    let native = Native::resolve(symbol).ok_or(Admission::Unknown)?;
    let contract = classify(native).ok_or(Admission::Unclassified)?;
    contract.admission().map_err(Admission::Rejected)?;
    Ok(contract)
}

/// The outcome of looking a symbol up for the native ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Not a symbol of the table: only the checked unresolved-provider adapter may carry it.
    Unknown,
    /// A symbol of the table in no effect list.
    Unclassified,
    Rejected(Rejection),
}

impl std::fmt::Display for Admission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown => f.write_str("not a native of the runtime table"),
            Self::Unclassified => f.write_str("a native of the runtime table missing from the companion catalogue"),
            Self::Rejected(rejection) => rejection.fmt(f),
        }
    }
}

/// The parts of a canonical symbol `module@major::name/arity`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Symbol<'a> {
    pub module: &'a str,
    pub major: u32,
    pub name: &'a str,
    pub arity: usize,
}

impl<'a> Symbol<'a> {
    /// `None` unless `text` is exactly canonical: non-empty module path segments of
    /// `[a-z0-9_]`, a decimal major without leading zeros, an identifier name and a decimal arity.
    pub fn parse(text: &'a str) -> Option<Self> {
        let (module, rest) = text.split_once('@')?;
        let (major, rest) = rest.split_once("::")?;
        let (name, arity) = rest.split_once('/')?;
        let segment = |part: &str| !part.is_empty()
            && part.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
        let number = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
            && (part == "0" || !part.starts_with('0'));
        let identifier = !name.is_empty()
            && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            && !name.as_bytes()[0].is_ascii_digit();
        if !module.split('/').all(segment) || !number(major) || !number(arity) || !identifier {
            return None;
        }
        Some(Self { module, major: major.parse().ok()?, name, arity: arity.parse().ok()? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_native_is_classified() {
        let mut counts = std::collections::BTreeMap::new();
        for &native in Native::ALL {
            let contract = classify(native).unwrap_or_else(|| panic!("unclassified {}", native.symbol()));
            *counts.entry((contract.abi, contract.transport)).or_insert(0) += 1;
            let admitted = admitted(native.symbol());
            assert_eq!(admitted.is_ok(), contract.transport != Transport::Opaque);
        }
        let total: usize = counts.values().sum();
        assert_eq!(total, Native::ALL.len());
        // A change to the table changes these counts: review its classification with it.
        let expected = [((AbiKind::Sync, Transport::Data), 217), ((AbiKind::Sync, Transport::Handle), 345),
            ((AbiKind::Sync, Transport::Opaque), 9), ((AbiKind::Async, Transport::Data), 5),
            ((AbiKind::Async, Transport::Handle), 47)];
        assert_eq!(counts, expected.into_iter().collect());
        let mut roles = std::collections::BTreeMap::new();
        let mut effects = std::collections::BTreeMap::new();
        for contract in Native::ALL.iter().filter_map(|&native| classify(native)) {
            if contract.admission().is_ok() {
                *roles.entry(contract.handle).or_insert(0) += 1;
                *effects.entry(contract.effect).or_insert(0) += 1;
            }
        }
        // #601 N1: the 51 async natives are admitted too (4 data, 20 using and 27 publishing a handle, all host).
        let roles_expected = [(HandleRole::None, 222), (HandleRole::Uses, 269), (HandleRole::Publishes, 99),
            (HandleRole::Closes, 24)];
        assert_eq!(roles, roles_expected.into_iter().collect());
        let effects_expected = [(Effect::Pure, 302), (Effect::Stdio, 3), (Effect::Host, 309)];
        assert_eq!(effects, effects_expected.into_iter().collect());
    }

    /// #601 N1, audit item 1 of the slice: every asynchronous native of the table takes and publishes plain data
    /// or native handles only, so its driver's creation packet is statically empty (ruling PR-N). The 51 natives,
    /// by module, are the `native async` declarations of std `runtime/`.
    #[test]
    fn async_natives_carry_data_and_handles_only() {
        let mut modules = std::collections::BTreeMap::new();
        for contract in Native::ALL.iter().filter_map(|&native| classify(native)) {
            if contract.abi != AbiKind::Async {
                continue;
            }
            let signature = contract.native.signature();
            assert!(types(signature).all(|ty| !mentions(ty, &|ty| *ty == Type::Any)), "{}", contract.native.symbol());
            assert!(contract.admission().is_ok() && contract.effect == Effect::Host, "{}", contract.native.symbol());
            assert_ne!(contract.handle, HandleRole::Closes, "{}", contract.native.symbol());
            *modules.entry(contract.native.module()).or_insert(0) += 1;
        }
        let expected = [("runtime/application", 5), ("runtime/database", 19), ("runtime/fs", 1),
            ("runtime/fs_root", 1), ("runtime/network", 13), ("runtime/process", 6), ("runtime/signal", 2),
            ("runtime/time", 1), ("runtime/tls_transport", 4)];
        assert_eq!(modules, expected.into_iter().collect());
    }

    #[test]
    fn module_lists_are_disjoint_and_used() {
        for module in PURE {
            assert!(!HOST.contains(module), "{module}");
        }
        for module in PURE.iter().chain(HOST) {
            assert!(Native::ALL.iter().any(|native| native.module() == *module), "unused {module}");
        }
    }

    #[test]
    fn classification_follows_the_signature() {
        let contract = classify(Native::HexEncode).unwrap();
        assert_eq!((contract.transport, contract.handle, contract.effect),
            (Transport::Data, HandleRole::None, Effect::Pure));
        let contract = classify(Native::BufferStateCreate).unwrap();
        assert_eq!((contract.transport, contract.handle), (Transport::Handle, HandleRole::Publishes));
        assert_eq!(classify(Native::BufferStateClose).unwrap().handle, HandleRole::Closes);
        assert_eq!(classify(Native::BufferStateClosed).unwrap().handle, HandleRole::Uses);
        assert_eq!(classify(Native::StdoutWrite).unwrap().effect, Effect::Stdio);
        assert_eq!(classify(Native::StdinRead).unwrap().effect, Effect::Host);
        assert_eq!(admitted("runtime/debug@1::data_dump/1"), Err(Admission::Rejected(Rejection::Opaque)));
        // An async native is admitted to the async native ABI (#601 N1): a driver that publishes a handle.
        let terminal = admitted("runtime/application@1::terminal_standard/1").unwrap();
        assert_eq!((terminal.abi, terminal.transport, terminal.handle, terminal.effect),
            (AbiKind::Async, Transport::Handle, HandleRole::Publishes, Effect::Host));
        let sleep = admitted("runtime/time@1::sleep_nanoseconds/1").unwrap();
        assert_eq!((sleep.abi, sleep.transport, sleep.handle), (AbiKind::Async, Transport::Data, HandleRole::None));
        assert_eq!(admitted("app@1::conformance_unlinked_symbol/1"), Err(Admission::Unknown));
    }

    #[test]
    fn canonical_symbols() {
        let symbol = Symbol::parse("app/sub@12::name_2/3").unwrap();
        assert_eq!(symbol, Symbol { module: "app/sub", major: 12, name: "name_2", arity: 3 });
        for text in ["app@1::x", "app::x/1", "@1::x/1", "app@01::x/1", "app@1::x/01", "App@1::x/1",
            "app//b@1::x/1", "app@1::2x/1", "app@1::x-y/1", "app@1::/1", "app@x::y/1"] {
            assert_eq!(Symbol::parse(text), None, "{text}");
        }
    }
}
