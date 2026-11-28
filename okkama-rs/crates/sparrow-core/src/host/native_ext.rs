//! Synchronous extension calls: scalar bits and call-scoped typed tokens.
#![allow(unsafe_code)]

use super::{Heap, Shape, Slot};
use crate::{Error, natives::Refusal, runtime::{FailureKind, Runtime}, word::{Repr, Word}};
use okkama_runtime_abi::{NativeFailure, Signature, Type, ext::*};
use std::{borrow::Cow, ffi::c_void, sync::atomic::{AtomicU64, Ordering}};

#[derive(Debug)]
pub(crate) struct Binding {
    pub symbol: &'static str,
    sync: okk_sync,
    params: Box<[(Shape, u64)]>,
    result: (Shape, u64),
    bytes_tag: u64,
}

impl Binding {
    pub(crate) fn new(host: &mut Heap, symbol: &'static str, signature: &Signature, sync: Option<okk_sync>) -> Result<Self, Error> {
        let Some(sync) = sync.filter(|_| !signature.is_async) else {
            return Err(Error(format!("native {symbol}: async extensions are not in this step")));
        };
        let mut shape = |ty: &Type| {
            let kind = match ty {
                Type::Void => OKK_NONE, Type::Int => OKK_INT, Type::Float => OKK_FLOAT,
                Type::Bool => OKK_BOOL, Type::Byte => OKK_BYTE, Type::Char => OKK_CHAR,
                Type::Str => OKK_STRING,
                Type::Array(item) if **item == Type::Byte => OKK_ARRAY,
                _ => return Err(Error(format!("native {symbol}: type {ty:?} is not in this step"))),
            };
            Ok((host.shape(ty), kind))
        };
        let params = signature.params.iter().map(&mut shape).collect::<Result<_, _>>()?;
        let result = shape(&signature.result)?;
        let Shape::Array(byte) = host.shape(&Type::bytes()) else { unreachable!() };
        Ok(Self { symbol, sync, params, result, bytes_tag: super::ARRAY | ((byte as u64) << 8) })
    }
}

// Process-wide serials reject tokens from earlier calls, even another runtime.
static SERIAL: AtomicU64 = AtomicU64::new(1);

#[repr(C)]
struct Call<'a> {
    public: okk_call,
    host: &'a mut Heap,
    binding: &'a Binding,
    serial: u64,
    slots: [Slot; 8],
    overflow: Vec<Slot>,
    count: usize,
    views: Vec<Vec<u8>>,
    failure: Option<Word>,
    fault: bool,
}

impl Call<'_> {
    fn token(&mut self, slot: Slot, kind: u64) -> Result<okk_slot, u32> {
        let index = u32::try_from(self.count).map_err(|_| OKK_RANGE)?;
        if self.count < self.slots.len() { self.slots[self.count] = slot; } else { self.overflow.push(slot); }
        self.count += 1;
        Ok(okk_slot { bits: (self.serial << 32) | u64::from(index), kind })
    }

    fn get(&self, token: okk_slot, kind: u64, tag: u64) -> Result<Slot, u32> {
        if token.kind != kind { return Err(OKK_BAD_KIND); }
        let index = token.bits as u32 as usize;
        if token.bits >> 32 != self.serial || index >= self.count { return Err(OKK_BAD_TOKEN); }
        let slot = if index < self.slots.len() { self.slots[index] } else { self.overflow[index - self.slots.len()] };
        if slot.tag != tag { return Err(OKK_BAD_SHAPE); }
        Ok(slot)
    }

    fn failure(&mut self, code: &str, message: &str) -> Result<(), Error> {
        let payload = self.host.native_failure_new(NativeFailure {
            domain: self.binding.symbol.split('@').next().unwrap_or("").into(),
            operation: self.binding.symbol.into(), code: code.into(), message: message.into(),
        })?;
        self.failure = Some(self.host.failure_new(super::FailureKind::Native(payload))?.bits);
        Ok(())
    }
}

pub(crate) fn call(runtime: &mut Runtime, binding: &Binding, arguments: &[Word]) -> Result<(Word, Repr), Refusal> {
    let result = super::with_host(runtime, false, |host| invoke(host, binding, arguments));
    if matches!(result, Err(Refusal::Fatal(_))) { runtime.poisoned = true; return result; }
    if runtime.limits.interrupted() {
        return Err(Refusal::Exhausted(crate::budget::Exhausted::Interrupted));
    }
    result
}

#[cold]
fn fault(binding: &Binding, reason: impl std::fmt::Display) -> Refusal {
    Refusal::Fatal(format!("internal error in native {}: {reason}", binding.symbol))
}

fn invoke(host: &mut Heap, binding: &Binding, arguments: &[Word]) -> Result<(Word, Repr), Refusal> {
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
    if serial > u64::from(u32::MAX) { return Err(fault(binding, "call token serial exhausted")); }
    let mut call = Call {
        public: okk_call { host: &HOST }, host, binding, serial,
        slots: [Slot::default(); 8], overflow: Vec::new(), count: 0,
        views: Vec::new(), failure: None, fault: false,
    };
    let mut stack = [okk_slot::default(); 8];
    let mut overflow = Vec::new();
    let args = if arguments.len() <= stack.len() { &mut stack[..arguments.len()] } else {
        overflow.resize(arguments.len(), okk_slot::default());
        &mut overflow
    };
    for ((out, &word), (shape, kind)) in args.iter_mut().zip(arguments).zip(binding.params.iter()) {
        *out = match *kind {
            OKK_INT => {
                let bits = call.host.int64(call.host.slot(word, shape))
                    .map_err(|e| fault(binding, e.0))?.ok_or_else(|| {
                        Refusal::Failure(FailureKind::Program(format!("native {}: bigint arguments are not in this step", binding.symbol)))
                    })? as u64;
                okk_slot { bits, kind: *kind }
            }
            OKK_STRING | OKK_ARRAY => call.token(call.host.slot(word, shape), *kind).map_err(|s| fault(binding, s))?,
            _ => okk_slot { bits: word, kind: *kind },
        };
    }
    #[cfg(debug_assertions)]
    let collections = call.host.core().stats().collections;
    let mut result = okk_slot::default();
    // No collection or VM re-entry until the callback returns; the result
    // becomes rt_native's extra safepoint root immediately afterwards.
    // Derive the pointer from the whole context, not just its public prefix.
    let context = (&mut call as *mut Call<'_>).cast::<okk_call>();
    let status = unsafe { (binding.sync)(context, args.as_ptr(), &mut result) };
    #[cfg(debug_assertions)]
    debug_assert_eq!(call.host.core().stats().collections, collections);
    if call.fault { return Err(fault(binding, "host callback panicked")); }
    if status == OKK_FAILED {
        return call.failure.map(|f| Err(Refusal::Raise(f))).unwrap_or_else(|| Err(fault(binding, "OKK_FAILED without fail")));
    }
    if status == OKK_STOPPED && unsafe { (*call.host.rt).limits.interrupted() } {
        return Err(Refusal::Exhausted(crate::budget::Exhausted::Interrupted));
    }
    if status != OKK_OK { return Err(fault(binding, format_args!("status {status}"))); }
    if call.failure.is_some() { return Err(fault(binding, "pending failure with OKK_OK")); }
    if result.kind != binding.result.1 { return Err(fault(binding, "wrong result kind")); }
    let raw = |bits| Ok((bits, Repr::Raw));
    match result.kind {
        OKK_NONE => raw(crate::word::UNBOUND),
        OKK_INT => Ok((call.host.core_mut().int(result.bits as i64), Repr::Int)),
        OKK_FLOAT if !f64::from_bits(result.bits).is_finite() => {
            call.failure("not_finite", "Native float result is not finite").map_err(|e| fault(binding, e.0))?;
            Err(Refusal::Raise(call.failure.expect("native failure")))
        }
        OKK_FLOAT => raw(result.bits),
        OKK_BOOL if result.bits <= 1 => raw(result.bits),
        OKK_BYTE if result.bits <= 255 => raw(result.bits),
        OKK_CHAR if u32::try_from(result.bits).ok().and_then(char::from_u32).is_some() => raw(result.bits),
        OKK_STRING | OKK_ARRAY => {
            let tag = if result.kind == OKK_STRING { super::STR } else { binding.bytes_tag };
            let slot = call.get(result, result.kind, tag).map_err(|s| fault(binding, s))?;
            Ok((slot.bits, Repr::Ref))
        }
        _ => Err(fault(binding, "result is out of range")),
    }
}

// A host panic cannot unwind through foreign code. Keep it sticky even if
// the callback ignores the status; rt_native supplies the fatal boundary.
unsafe fn guarded(call: *mut okk_call, body: impl FnOnce(&mut Call<'_>) -> u32) -> u32 {
    let call = unsafe { &mut *call.cast::<Call<'_>>() };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(call))) {
        Ok(status) => status,
        Err(_) => { call.fault = true; OKK_PANIC }
    }
}

unsafe fn input<'a>(data: *const u8, len: usize) -> Result<&'a [u8], u32> {
    if len == 0 { return Ok(&[]); }
    if data.is_null() || len > isize::MAX as usize { return Err(OKK_RANGE); }
    Ok(unsafe { std::slice::from_raw_parts(data, len) })
}

unsafe extern "C" fn text(call: *mut okk_call, value: okk_slot, data: *mut *const u8, len: *mut usize) -> u32 {
    unsafe { guarded(call, |call| {
        let slot = match call.get(value, OKK_STRING, super::STR) { Ok(s) => s, Err(s) => return s };
        let view = call.host.core().text(slot.bits);
        *data = view.as_ptr(); *len = view.len();
        OKK_OK
    }) }
}

unsafe extern "C" fn bytes(call: *mut okk_call, value: okk_slot, data: *mut *const u8, len: *mut usize) -> u32 {
    unsafe { guarded(call, |call| {
        let slot = match call.get(value, OKK_ARRAY, call.binding.bytes_tag) { Ok(s) => s, Err(s) => return s };
        let view = call.host.core().byte_view(slot.bits);
        *len = view.len();
        match view {
            Cow::Borrowed(view) => *data = view.as_ptr(),
            Cow::Owned(view) => {
                *data = view.as_ptr();
                call.views.push(view);
            }
        }
        OKK_OK
    }) }
}

unsafe extern "C" fn text_new(call: *mut okk_call, data: *const u8, len: usize, result: *mut okk_slot) -> u32 {
    unsafe { guarded(call, |call| {
        let data = match input(data, len) { Ok(d) => d, Err(s) => return s };
        let text = match std::str::from_utf8(data) { Ok(t) => t, Err(_) => return OKK_BAD_TEXT };
        let slot = Slot { bits: call.host.core_mut().allocate_text(text), tag: super::STR };
        match call.token(slot, OKK_STRING) { Ok(s) => { *result = s; OKK_OK }, Err(s) => s }
    }) }
}

unsafe extern "C" fn bytes_new(call: *mut okk_call, data: *const u8, len: usize, result: *mut okk_slot) -> u32 {
    unsafe { guarded(call, |call| {
        let data = match input(data, len) { Ok(d) => d, Err(s) => return s };
        let slot = Slot { bits: call.host.core_mut().allocate_bytes(data), tag: call.binding.bytes_tag };
        match call.token(slot, OKK_ARRAY) { Ok(s) => { *result = s; OKK_OK }, Err(s) => s }
    }) }
}

unsafe extern "C" fn fail(call: *mut okk_call, code: *const u8, code_len: usize, message: *const u8, message_len: usize) -> u32 {
    unsafe { guarded(call, |call| {
        let (code, message) = match (input(code, code_len), input(message, message_len)) {
            (Ok(c), Ok(m)) => (c, m), _ => return OKK_RANGE,
        };
        let (Ok(code), Ok(message)) = (std::str::from_utf8(code), std::str::from_utf8(message)) else { return OKK_BAD_TEXT; };
        match call.failure(code, message) { Ok(()) => OKK_OK, Err(_) => { call.fault = true; OKK_PANIC } }
    }) }
}

unsafe extern "C" fn interrupted(call: *mut okk_call, result: *mut u32) -> u32 {
    unsafe { guarded(call, |call| {
        *result = u32::from((*call.host.rt).limits.interrupted());
        OKK_OK
    }) }
}

macro_rules! deferred {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => { $(
        unsafe extern "C" fn $name($($arg: $ty),*) -> u32 {
            $(let _ = $arg;)*
            OKK_UNSUPPORTED
        }
    )* };
}

deferred! {
    bytes_reserve(c: *mut okk_call, n: usize, p: *mut *mut u8, r: *mut okk_slot);
    bigint_read(c: *mut okk_call, v: okk_slot, s: *mut i32, p: *mut *const u8, n: *mut usize);
    bigint_new(c: *mut okk_call, s: i32, p: *const u8, n: usize, r: *mut okk_slot);
    rat_parts(c: *mut okk_call, v: okk_slot, n: *mut okk_slot, d: *mut okk_slot);
    rat_new(c: *mut okk_call, n: okk_slot, d: okk_slot, r: *mut okk_slot);
    array_len(c: *mut okk_call, v: okk_slot, n: *mut usize);
    array_get(c: *mut okk_call, v: okk_slot, i: usize, r: *mut okk_slot);
    array_read(c: *mut okk_call, v: okk_slot, i: usize, r: *mut okk_slot, n: usize);
    array_new(c: *mut okk_call, v: *const okk_slot, n: usize, r: *mut okk_slot);
    map_len(c: *mut okk_call, v: okk_slot, n: *mut usize);
    map_entries(c: *mut okk_call, v: okk_slot, k: *mut okk_slot, r: *mut okk_slot, n: usize);
    map_new(c: *mut okk_call, k: *const okk_slot, v: *const okk_slot, n: usize, r: *mut okk_slot);
    state_new(c: *mut okk_call, t: *const okk_state_type, p: *mut c_void, n: usize, r: *mut okk_slot);
    state_get(c: *mut okk_call, v: okk_slot, t: *const okk_state_type, p: *mut *mut c_void);
    state_release(c: *mut okk_call, v: okk_slot);
}

static HOST: okk_host = okk_host {
    text, bytes, text_new, bytes_new, bytes_reserve, bigint_read, bigint_new, rat_parts, rat_new,
    array_len, array_get, array_read, array_new, map_len, map_entries, map_new,
    state_new, state_get, state_release, fail, interrupted,
};
