//! Extension ABI 1. Keep the layout and prototypes identical to `include/okkama_ext.h`.
#![allow(non_camel_case_types, unsafe_code)]

use core::ffi::{c_char, c_void};

pub const OKKAMA_EXT_ABI: u32 = 1;
/// Minimum stack available to a synchronous callback, in bytes.
pub const OKKAMA_EXT_STACK: usize = 512 * 1024;

// Integers, not Rust enums: an untrusted discriminant must remain representable.
pub const OKK_NONE: u64 = 0;
pub const OKK_INT: u64 = 1;
pub const OKK_FLOAT: u64 = 2;
pub const OKK_BOOL: u64 = 3;
pub const OKK_BYTE: u64 = 4;
pub const OKK_CHAR: u64 = 5;
pub const OKK_BIGINT: u64 = 6;
pub const OKK_RAT: u64 = 7;
pub const OKK_STRING: u64 = 8;
pub const OKK_ARRAY: u64 = 9;
pub const OKK_MAP: u64 = 10;
pub const OKK_STATE: u64 = 11;

pub const OKK_OK: u32 = 0;
pub const OKK_FAILED: u32 = 1;
pub const OKK_PANIC: u32 = 2;
pub const OKK_STOPPED: u32 = 3;
pub const OKK_BAD_TOKEN: u32 = 4;
pub const OKK_BAD_KIND: u32 = 5;
pub const OKK_BAD_SHAPE: u32 = 6;
pub const OKK_BAD_TEXT: u32 = 7;
pub const OKK_RANGE: u32 = 8;
/// This host has not implemented the operation yet.
pub const OKK_UNSUPPORTED: u32 = 9;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct okk_slot { pub bits: u64, pub kind: u64 }

#[repr(C)]
#[derive(Debug)]
pub struct okk_call { pub host: *const okk_host }

#[repr(C)]
#[derive(Debug)]
pub struct okk_cancel { pub cancelled: unsafe extern "C" fn(*const okk_cancel) -> u32 }

pub type okk_sync = unsafe extern "C" fn(*mut okk_call, *const okk_slot, *mut okk_slot) -> u32;

#[repr(C)]
#[derive(Debug)]
pub struct okk_async {
    pub prepare: unsafe extern "C" fn(*mut okk_call, *const okk_slot, *mut *mut c_void) -> u32,
    pub execute: unsafe extern "C" fn(*mut c_void, *const okk_cancel),
    pub complete: unsafe extern "C" fn(*mut okk_call, *mut c_void, *mut okk_slot) -> u32,
    pub drop: unsafe extern "C" fn(*mut c_void),
}

#[repr(C)]
#[derive(Debug)]
pub struct okk_export {
    pub symbol: *const c_char,
    pub signature: *const c_char,
    pub params: *const c_char,
    pub sync: Option<okk_sync>,
    pub r#async: *const okk_async,
}

#[repr(C)]
#[derive(Debug)]
/// Short native object name in the export module; drop runs on its runtime's VM thread.
pub struct okk_state_type {
    pub name: *const c_char,
    pub drop: unsafe extern "C" fn(*mut c_void),
}

#[repr(C)]
#[derive(Debug)]
pub struct okk_extension {
    pub abi: u32,
    pub size: u32,
    pub name: *const c_char,
    pub exports: *const okk_export,
    pub export_count: u32,
    pub states: *const okk_state_type,
    pub state_count: u32,
}

// Descriptor data is immutable. Dereferencing its pointers requires the
// embedding's unsafe registration contract, including transitive lifetimes.
unsafe impl Sync for okk_export {}
unsafe impl Sync for okk_state_type {}
unsafe impl Sync for okk_extension {}

/// Views and output buffers last until the callback returns. Input buffers
/// are copied. All pointers must address the stated length; null is allowed
/// only for an empty buffer. A call and its tokens belong to its VM thread.
/// BAD_TOKEN/KIND/SHAPE poison the call even when ignored. BAD_TEXT and RANGE
/// are recoverable data errors; returning them from the callback is a fault.
/// Allocation past the run's budget sets sticky STOPPED, without poison.
#[repr(C)]
#[derive(Debug)]
pub struct okk_host {
    pub text: unsafe extern "C" fn(*mut okk_call, okk_slot, *mut *const u8, *mut usize) -> u32,
    pub bytes: unsafe extern "C" fn(*mut okk_call, okk_slot, *mut *const u8, *mut usize) -> u32,
    pub text_new: unsafe extern "C" fn(*mut okk_call, *const u8, usize, *mut okk_slot) -> u32,
    pub bytes_new: unsafe extern "C" fn(*mut okk_call, *const u8, usize, *mut okk_slot) -> u32,
    /// Zeroed writable bytes and their ARRAY token; writable until callback return.
    pub bytes_reserve: unsafe extern "C" fn(*mut okk_call, usize, *mut *mut u8, *mut okk_slot) -> u32,
    /// INT or BIGINT to sign (-1/0/1) and unsigned big-endian magnitude view.
    pub bigint_read: unsafe extern "C" fn(*mut okk_call, okk_slot, *mut i32, *mut *const u8, *mut usize) -> u32,
    /// Leading zeroes accepted; sign zero requires zero magnitude. Normalize to INT when possible.
    pub bigint_new: unsafe extern "C" fn(*mut okk_call, i32, *const u8, usize, *mut okk_slot) -> u32,
    /// Reduced numerator and positive denominator as INT/BIGINT slots.
    pub rat_parts: unsafe extern "C" fn(*mut okk_call, okk_slot, *mut okk_slot, *mut okk_slot) -> u32,
    /// Reduce two int slots; zero denominator returns RANGE.
    pub rat_new: unsafe extern "C" fn(*mut okk_call, okk_slot, okk_slot, *mut okk_slot) -> u32,
    /// Element count of any ARRAY token.
    pub array_len: unsafe extern "C" fn(*mut okk_call, okk_slot, *mut usize) -> u32,
    /// One element; out-of-bounds index returns RANGE.
    pub array_get: unsafe extern "C" fn(*mut okk_call, okk_slot, usize, *mut okk_slot) -> u32,
    /// Copy count slots starting at the index; check the whole range first.
    pub array_read: unsafe extern "C" fn(*mut okk_call, okk_slot, usize, *mut okk_slot, usize) -> u32,
    /// Copy elements of one shape; an empty array fits every element type.
    pub array_new: unsafe extern "C" fn(*mut okk_call, *const okk_slot, usize, *mut okk_slot) -> u32,
    /// Entry count of a MAP token.
    pub map_len: unsafe extern "C" fn(*mut okk_call, okk_slot, *mut usize) -> u32,
    /// Copy the first count key/value pairs in key order; excess count returns RANGE.
    pub map_entries: unsafe extern "C" fn(*mut okk_call, okk_slot, *mut okk_slot, *mut okk_slot, usize) -> u32,
    /// Copy keys and values of one shape each; repeated keys return RANGE.
    pub map_new: unsafe extern "C" fn(*mut okk_call, *const okk_slot, *const okk_slot, usize, *mut okk_slot) -> u32,
    /// Success transfers payload ownership until release, dead-cell sweep or teardown.
    /// Failure leaves ownership with the caller; bytes charges collector pressure.
    pub state_new: unsafe extern "C" fn(*mut okk_call, *const okk_state_type, *mut c_void, usize, *mut okk_slot) -> u32,
    /// Type pointers must identify this extension's registered state types.
    /// Closed states set NativeFailure already_closed and return FAILED.
    pub state_get: unsafe extern "C" fn(*mut okk_call, okk_slot, *const okk_state_type, *mut *mut c_void) -> u32,
    pub state_release: unsafe extern "C" fn(*mut okk_call, okk_slot, *const okk_state_type) -> u32,
    pub fail: unsafe extern "C" fn(*mut okk_call, *const u8, usize, *const u8, usize) -> u32,
    pub interrupted: unsafe extern "C" fn(*mut okk_call, *mut u32) -> u32,
}
