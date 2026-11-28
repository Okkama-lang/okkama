//! Synchronous extension calls: scalar bits and call-scoped typed tokens.
#![allow(unsafe_code)]

use super::{Heap, Shape, Slot};
use crate::{Error, natives::Refusal, runtime::Runtime, word::{Repr, Word}};
use okkama_runtime_abi::{NativeFailure, Signature, Type, ext::*};
use std::{borrow::Cow, ffi::c_void, sync::atomic::{AtomicU64, Ordering}};

#[derive(Debug)]
pub(crate) struct Binding {
    pub symbol: &'static str,
    sync: okk_sync,
    params: Box<[Shape]>,
    result: Shape,
    bytes_tag: u64,
}

impl Binding {
    pub(crate) fn new(host: &mut Heap, program: &okkama_sparrow::Program, function: &okkama_sparrow::Function,
        symbol: &'static str, signature: &Signature, sync: Option<okk_sync>) -> Result<Self, Error>
    {
        let Some(sync) = sync.filter(|_| !signature.is_async) else {
            return Err(Error(format!("native {symbol}: async extensions are not in this step")));
        };
        let params = signature.params.iter().zip(&function.params).map(|(ty, reg)| {
            value_shape(host, program, function.registers[reg.0 as usize], ty)
        }).collect::<Result<_, _>>()?;
        let result = match function.result {
            Some(ty) => value_shape(host, program, ty, &signature.result)?,
            None => Shape::Unknown,
        };
        let Shape::Array(byte) = host.shape(&Type::bytes()) else { unreachable!() };
        Ok(Self { symbol, sync, params, result, bytes_tag: super::ARRAY | ((byte as u64) << 8) })
    }
}

fn value_shape(host: &mut Heap, program: &okkama_sparrow::Program, ty: okkama_sparrow::TypeRef, abi: &Type) -> Result<Shape, Error> {
    use okkama_sparrow::Type as T;
    let shape = match (&program.types[ty.0 as usize], abi) {
        (T::Rat, Type::Rat) => Shape::Rat,
        (T::Array(t), Type::Array(a)) => {
            let element = value_shape(host, program, *t, a)?;
            Shape::Array(host.intern(element))
        }
        (T::Map(k, v), Type::Map(a, b)) => {
            let key = value_shape(host, program, *k, a)?;
            let value = value_shape(host, program, *v, b)?;
            Shape::Map(host.intern(key), host.intern(value))
        }
        (T::Variant(v), Type::Maybe(a)) => {
            let variant = &program.variants[v.0 as usize];
            let some = variant.cases.iter().find(|(n, _)| n == "some").unwrap().1;
            let none = variant.cases.iter().find(|(n, _)| n == "none").unwrap().1;
            let inner = value_shape(host, program, program.objects[some.0 as usize].fields[0].1, a)?;
            if matches!(inner, Shape::Maybe(..)) { return Err(Error("nested Maybe is not in ABI 1".into())); }
            Shape::Maybe(host.intern(inner), some.0, none.0)
        }
        (_, Type::Int | Type::Float | Type::Bool | Type::Byte | Type::Char | Type::Str) => host.shape(abi),
        _ => return Err(Error(format!("extension type {abi:?} is not in this step"))),
    };
    host.intern(shape.clone());
    Ok(shape)
}

mod values;
use values::*;

// Process-wide serials reject tokens from earlier calls, even another runtime.
static SERIAL: AtomicU64 = AtomicU64::new(1);

#[repr(C)]
struct Call<'a> {
    public: okk_call,
    host: &'a mut Heap,
    binding: &'a Binding,
    serial: u64,
    inline: [Slot; 8],
    overflow: Vec<Slot>,
    count: usize,
    views: Vec<Vec<u8>>,
    failure: Option<Word>,
    fault: bool,
    stopped: bool,
    scratch: usize,
    shape_start: usize,
}

impl Drop for Call<'_> {
    fn drop(&mut self) {
        // Dynamic shapes describe call-local tokens, never words stored in the VM.
        while self.host.shapes.len() > self.shape_start {
            let shape = self.host.shapes.pop().expect("call shape");
            self.host.shape_index.remove(&shape);
        }
    }
}

impl Call<'_> {
    fn token(&mut self, slot: Slot, kind: u64) -> Result<okk_slot, u32> {
        self.reserve(2 * std::mem::size_of::<Slot>())?;
        let index = u32::try_from(self.count).map_err(|_| OKK_RANGE)?;
        if self.count < self.inline.len() { self.inline[self.count] = slot; } else { self.overflow.push(slot); }
        self.count += 1;
        Ok(okk_slot { bits: (self.serial << 32) | u64::from(index), kind })
    }

    fn get(&self, token: okk_slot, kind: u64, tag: u64) -> Result<Slot, u32> {
        if token.kind != kind { return Err(OKK_BAD_KIND); }
        let index = token.bits as u32 as usize;
        if token.bits >> 32 != self.serial || index >= self.count { return Err(OKK_BAD_TOKEN); }
        let slot = if index < self.inline.len() { self.inline[index] } else { self.overflow[index - self.inline.len()] };
        if slot.tag != tag { return Err(OKK_BAD_SHAPE); }
        Ok(slot)
    }

    fn failure(&mut self, code: &str, message: &str) -> Result<(), u32> {
        self.preflight(code.len().saturating_add(message.len()).saturating_add(2 * self.binding.symbol.len()), 2)?;
        let payload = self.host.native_failure_new(NativeFailure {
            domain: self.binding.symbol.split('@').next().unwrap_or("").into(),
            operation: self.binding.symbol.into(), code: code.into(), message: message.into(),
        }).map_err(|_| OKK_PANIC)?;
        self.failure = Some(self.host.failure_new(super::FailureKind::Native(payload)).map_err(|_| OKK_PANIC)?.bits);
        self.reserve(0)
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
    let shape_start = host.shapes.len();
    let mut call = Call {
        public: okk_call { host: &HOST }, host, binding, serial,
        inline: [Slot::default(); 8], overflow: Vec::new(), count: 0,
        views: Vec::new(), failure: None, fault: false, stopped: false, scratch: 0, shape_start,
    };
    let mut stack = [okk_slot::default(); 8];
    let mut overflow = Vec::new();
    let args = if arguments.len() <= stack.len() { &mut stack[..arguments.len()] } else {
        overflow.resize(arguments.len(), okk_slot::default());
        &mut overflow
    };
    for ((out, &word), shape) in args.iter_mut().zip(arguments).zip(binding.params.iter()) {
        *out = call.export(call.host.slot(word, shape)).map_err(|s| call.refusal(s))?;
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
    if call.fault { return Err(fault(binding, "invalid host operation or panic")); }
    if call.stopped { return Err(Refusal::Exhausted(crate::budget::Exhausted::Memory)); }
    if status == OKK_FAILED {
        return call.failure.map(|f| Err(Refusal::Raise(f))).unwrap_or_else(|| Err(fault(binding, "OKK_FAILED without fail")));
    }
    if status == OKK_STOPPED && unsafe { (*call.host.rt).limits.interrupted() } {
        return Err(Refusal::Exhausted(crate::budget::Exhausted::Interrupted));
    }
    if status != OKK_OK { return Err(fault(binding, format_args!("status {status}"))); }
    if call.failure.is_some() { return Err(fault(binding, "pending failure with OKK_OK")); }
    let slot = call.import_as(result, &binding.result, true).map_err(|s| call.refusal(s))?;
    let word = call.host.word(slot);
    call.reserve(0).map_err(|s| call.refusal(s))?;
    Ok((word, repr(&binding.result)))
}

// A host panic cannot unwind through foreign code. Keep it sticky even if
// the callback ignores the status; rt_native supplies the fatal boundary.
unsafe fn guarded(call: *mut okk_call, body: impl FnOnce(&mut Call<'_>) -> u32) -> u32 {
    let call = unsafe { &mut *call.cast::<Call<'_>>() };
    if call.stopped { return OKK_STOPPED; }
    if call.fault { return OKK_PANIC; }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(call))) {
        Ok(status) => {
            if matches!(status, OKK_BAD_TOKEN | OKK_BAD_KIND | OKK_BAD_SHAPE | OKK_PANIC) { call.fault = true; }
            if call.reserve(0).is_err() { return OKK_STOPPED; }
            status
        },
        Err(_) => { call.fault = true; OKK_PANIC }
    }
}

unsafe fn input<'a, T>(data: *const T, len: usize) -> Result<&'a [T], u32> {
    if len == 0 { return Ok(&[]); }
    if data.is_null() || !data.is_aligned() || len > isize::MAX as usize / std::mem::size_of::<T>() { return Err(OKK_RANGE); }
    Ok(unsafe { std::slice::from_raw_parts(data, len) })
}

fn output<T>(data: *mut T) -> Result<(), u32> {
    if data.is_null() || !data.is_aligned() { Err(OKK_BAD_SHAPE) } else { Ok(()) }
}

unsafe extern "C" fn text(call: *mut okk_call, value: okk_slot, data: *mut *const u8, len: *mut usize) -> u32 {
    unsafe { guarded(call, |call| {
        if output(data).and(output(len)).is_err() { return OKK_BAD_SHAPE; }
        let slot = match call.get(value, OKK_STRING, super::STR) { Ok(s) => s, Err(s) => return s };
        let view = call.host.core().text(slot.bits);
        *data = view.as_ptr(); *len = view.len();
        OKK_OK
    }) }
}

unsafe extern "C" fn bytes(call: *mut okk_call, value: okk_slot, data: *mut *const u8, len: *mut usize) -> u32 {
    unsafe { guarded(call, |call| {
        if output(data).and(output(len)).is_err() { return OKK_BAD_SHAPE; }
        let slot = match call.byte_slot(value) { Ok(s) => s, Err(s) => return s };
        if !call.host.core().is_bytes(slot.bits)
            && let Err(s) = call.preflight(call.host.core().array_length(slot.bits), 1) { return s; }
        let view = call.host.core().byte_view(slot.bits);
        *len = view.len();
        match view {
            Cow::Borrowed(view) => *data = view.as_ptr(),
            Cow::Owned(view) => {
                *data = view.as_ptr();
                if let Err(s) = call.reserve(view.len().saturating_add(2 * std::mem::size_of::<Vec<u8>>())) { return s; }
                call.views.push(view);
            }
        }
        OKK_OK
    }) }
}

unsafe extern "C" fn text_new(call: *mut okk_call, data: *const u8, len: usize, result: *mut okk_slot) -> u32 {
    unsafe { guarded(call, |call| {
        if output(result).is_err() { return OKK_BAD_SHAPE; }
        if let Err(s) = call.preflight(len, 1) { return s; }
        let data = match input(data, len) { Ok(d) => d, Err(s) => return s };
        let text = match std::str::from_utf8(data) { Ok(t) => t, Err(_) => return OKK_BAD_TEXT };
        let slot = Slot { bits: call.host.core_mut().allocate_text(text), tag: super::STR };
        match call.token(slot, OKK_STRING) { Ok(s) => { *result = s; OKK_OK }, Err(s) => s }
    }) }
}

unsafe extern "C" fn bytes_new(call: *mut okk_call, data: *const u8, len: usize, result: *mut okk_slot) -> u32 {
    unsafe { guarded(call, |call| {
        if output(result).is_err() { return OKK_BAD_SHAPE; }
        if let Err(s) = call.preflight(len, 1) { return s; }
        let data = match input(data, len) { Ok(d) => d, Err(s) => return s };
        let slot = Slot { bits: call.host.core_mut().allocate_bytes(data), tag: call.binding.bytes_tag };
        match call.token(slot, OKK_ARRAY) { Ok(s) => { *result = s; OKK_OK }, Err(s) => s }
    }) }
}

unsafe extern "C" fn fail(call: *mut okk_call, code: *const u8, code_len: usize, message: *const u8, message_len: usize) -> u32 {
    unsafe { guarded(call, |call| {
        let Some(total) = code_len.checked_add(message_len) else { return OKK_RANGE; };
        if let Err(s) = call.preflight(total, 2) { return s; }
        let (code, message) = match (input(code, code_len), input(message, message_len)) {
            (Ok(c), Ok(m)) => (c, m), _ => return OKK_RANGE,
        };
        let (Ok(code), Ok(message)) = (std::str::from_utf8(code), std::str::from_utf8(message)) else { return OKK_BAD_TEXT; };
        match call.failure(code, message) { Ok(()) => OKK_OK, Err(status) => status }
    }) }
}

unsafe extern "C" fn interrupted(call: *mut okk_call, result: *mut u32) -> u32 {
    unsafe { guarded(call, |call| {
        if output(result).is_err() { return OKK_BAD_SHAPE; }
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
    state_new(c: *mut okk_call, t: *const okk_state_type, p: *mut c_void, n: usize, r: *mut okk_slot);
    state_get(c: *mut okk_call, v: okk_slot, t: *const okk_state_type, p: *mut *mut c_void);
    state_release(c: *mut okk_call, v: okk_slot);
}

static HOST: okk_host = okk_host {
    text, bytes, text_new, bytes_new, bytes_reserve, bigint_read, bigint_new, rat_parts, rat_new,
    array_len, array_get, array_read, array_new, map_len, map_entries, map_new,
    state_new, state_get, state_release, fail, interrupted,
};
