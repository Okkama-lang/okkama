//! Хостовая криптография Okkama: проверенные реализации RustCrypto и aws-lc
//! (RSA), доступные Sparrow через Rust API (`ops`, `x509`). Алгоритмы не
//! реализуются здесь заново: модуль только выбирает их, проверяет аргументы и
//! переводит отказы в переносимые коды.

pub mod ops;
#[cfg(feature = "aws-lc")]
pub mod rsa_public;
pub mod secure;
#[cfg(feature = "aws-lc")]
pub mod x509;

pub use ops::{Error, Hasher, call, interruptible};
