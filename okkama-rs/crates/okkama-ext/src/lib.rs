//! Safe Rust functions over ABI-1 call scopes. The macros own the foreign boundary.
//! Use `#[native(blocking)]` for work exceeding one millisecond. Blocking inputs
//! are copied; only `Shared<T>` may retain a state while the VM releases it.
//! A by-value Box state argument calls `state_release` and moves the payload.
//! By-value shared `T` is a compile error: use `#[native(release)]` with `Shared<T>`.
//! Release functions take the state first (`&T` or `&mut T` for Box states), call
//! the function, then always release it, including on Failure or panic. The original
//! failure/panic status is reported after release. Shared release drops the host's
//! Arc; jobs keep their clones and the payload drops once after the last clone.
//! Conflicting borrows fail with `state_borrowed`; shared `&mut T` requires a
//! unique Arc. A job's Arc survives host release or teardown.
//! Panics become `OKK_PANIC`; void drop callbacks contain panics but cannot report
//! a status. Compile extensions with unwinding enabled (`panic = "abort"` aborts).
//! `rat` awaits an available rational dependency; nested `Option` is not ABI 1.

pub use okkama_ext_macros::{extension, native, state};
pub use okkama_runtime_abi::ext::*;
pub use num_bigint::BigInt;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug, Clone)]
pub struct Failure { code: String, message: String }
impl Failure {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into() }
    }
}

/// An independently owned reference to a `#[state(shared)]` payload.
#[derive(Debug)]
pub struct Shared<T> { inner: Arc<T> }
impl<T> Clone for Shared<T> { fn clone(&self) -> Self { Self { inner: self.inner.clone() } } }
impl<T> std::ops::Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &T { &self.inner }
}

#[doc(hidden)]
pub mod __private;

#[cfg(test)]
extern crate self as okkama_ext;
#[cfg(test)]
mod tests;
