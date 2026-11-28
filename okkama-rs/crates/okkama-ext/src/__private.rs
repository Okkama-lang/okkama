//! Macro support, not an independent extension API.
use super::*;
use std::{cell::{RefCell, Ref, RefMut}, ffi::c_void, panic::{catch_unwind, AssertUnwindSafe}};

pub fn guard(body: impl FnOnce() -> Result<(), u32>) -> u32 {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(())) => OKK_OK,
        Ok(Err(status)) => status,
        Err(payload) => { std::mem::forget(payload); OKK_PANIC }
    }
}
pub fn status(status: u32) -> Result<(), u32> { if status == OKK_OK { Ok(()) } else { Err(status) } }

pub struct Call { raw: *mut okk_call }
impl Call {
    /// # Safety
    /// The host owns a valid ABI-1 call scope for this entire value's lifetime.
    pub unsafe fn new(raw: *mut okk_call) -> Self { Self { raw } }
    fn host(&self) -> &okk_host { unsafe { &*(*self.raw).host } }
    pub fn fail(&self, failure: Failure) -> u32 {
        let s = unsafe { (self.host().fail)(self.raw, failure.code.as_ptr(), failure.code.len(), failure.message.as_ptr(), failure.message.len()) };
        if s == OKK_OK { OKK_FAILED } else { s }
    }
    pub fn read<T: Read>(&self, slot: okk_slot) -> Result<T, u32> { T::read(self, slot) }
    pub fn write<T: Write>(&self, value: T) -> Result<okk_slot, u32> { value.write(self) }
    pub fn text(&self, slot: okk_slot) -> Result<&str, u32> {
        let mut ptr = std::ptr::null(); let mut len = 0;
        status(unsafe { (self.host().text)(self.raw, slot, &mut ptr, &mut len) })?;
        std::str::from_utf8(unsafe { view(ptr, len) }).map_err(|_| OKK_BAD_TEXT)
    }
    pub fn bytes(&self, slot: okk_slot) -> Result<&[u8], u32> {
        let mut ptr = std::ptr::null(); let mut len = 0;
        status(unsafe { (self.host().bytes)(self.raw, slot, &mut ptr, &mut len) })?;
        Ok(unsafe { view(ptr, len) })
    }
}
unsafe fn view<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(ptr, len) } }
}
pub trait Read: Sized {
    const BY_VALUE: () = ();
    fn read(call: &Call, slot: okk_slot) -> Result<Self, u32>;
    fn read_vec(c: &Call, s: okk_slot) -> Result<Vec<Self>, u32> {
        let mut len = 0;
        status(unsafe { (c.host().array_len)(c.raw, s, &mut len) })?;
        let mut slots = vec![okk_slot::default(); len];
        status(unsafe { (c.host().array_read)(c.raw, s, 0, slots.as_mut_ptr(), len) })?;
        slots.into_iter().map(|s| Self::read(c, s)).collect()
    }
}
pub trait Write: Sized {
    fn write(self, call: &Call) -> Result<okk_slot, u32>;
    fn write_vec(values: Vec<Self>, c: &Call) -> Result<okk_slot, u32> {
        let slots = values.into_iter().map(|v| v.write(c)).collect::<Result<Vec<_>, _>>()?;
        let mut out = okk_slot::default();
        status(unsafe { (c.host().array_new)(c.raw, slots.as_ptr(), slots.len(), &mut out) })?;
        Ok(out)
    }
}

macro_rules! scalar {
    ($ty:ty, $kind:ident, $decode:expr, $encode:expr) => {
        impl Read for $ty {
            fn read(_: &Call, slot: okk_slot) -> Result<Self, u32> {
                if slot.kind != $kind { return Err(OKK_BAD_KIND); }
                ($decode)(slot.bits)
            }
        }
        impl Write for $ty {
            fn write(self, _: &Call) -> Result<okk_slot, u32> { Ok(okk_slot { kind: $kind, bits: ($encode)(self) }) }
        }
    };
}
scalar!(bool, OKK_BOOL, |b| match b { 0 => Ok(false), 1 => Ok(true), _ => Err(OKK_RANGE) }, u64::from);
impl Read for u8 {
    fn read(_: &Call, slot: okk_slot) -> Result<Self, u32> {
        if slot.kind != OKK_BYTE { return Err(OKK_BAD_KIND); }
        u8::try_from(slot.bits).map_err(|_| OKK_RANGE)
    }
    fn read_vec(c: &Call, s: okk_slot) -> Result<Vec<Self>, u32> { c.bytes(s).map(<[u8]>::to_vec) }
}
impl Write for u8 {
    fn write(self, _: &Call) -> Result<okk_slot, u32> { Ok(okk_slot { kind: OKK_BYTE, bits: self.into() }) }
    fn write_vec(values: Vec<Self>, c: &Call) -> Result<okk_slot, u32> { values.as_slice().write(c) }
}
scalar!(char, OKK_CHAR, |b| u32::try_from(b).ok().and_then(char::from_u32).ok_or(OKK_RANGE), |c: char| c as u64);
impl Read for i64 {
    fn read(c: &Call, slot: okk_slot) -> Result<Self, u32> {
        if slot.kind == OKK_INT { return Ok(slot.bits as i64); }
        if slot.kind != OKK_BIGINT { return Err(OKK_BAD_KIND); }
        Err(c.fail(Failure::new("out_of_range", "integer does not fit i64")))
    }
}
impl Write for i64 {
    fn write(self, _: &Call) -> Result<okk_slot, u32> { Ok(okk_slot { bits: self as u64, kind: OKK_INT }) }
}
impl Read for f64 {
    fn read(_: &Call, slot: okk_slot) -> Result<Self, u32> {
        if slot.kind != OKK_FLOAT { return Err(OKK_BAD_KIND); }
        Ok(f64::from_bits(slot.bits))
    }
}
impl Write for f64 {
    fn write(self, c: &Call) -> Result<okk_slot, u32> {
        if !self.is_finite() { return Err(c.fail(Failure::new("not_finite", "float result is not finite"))); }
        Ok(okk_slot { bits: self.to_bits(), kind: OKK_FLOAT })
    }
}
impl Read for BigInt {
    fn read(c: &Call, slot: okk_slot) -> Result<Self, u32> {
        let mut sign = 0; let mut ptr = std::ptr::null(); let mut len = 0;
        status(unsafe { (c.host().bigint_read)(c.raw, slot, &mut sign, &mut ptr, &mut len) })?;
        let sign = match sign { -1 => num_bigint::Sign::Minus, 0 => num_bigint::Sign::NoSign, 1 => num_bigint::Sign::Plus, _ => return Err(OKK_RANGE) };
        Ok(BigInt::from_bytes_be(sign, unsafe { view(ptr, len) }))
    }
}
impl Write for BigInt {
    fn write(self, c: &Call) -> Result<okk_slot, u32> {
        let (sign, bytes) = self.to_bytes_be();
        let sign = match sign { num_bigint::Sign::Minus => -1, num_bigint::Sign::NoSign => 0, num_bigint::Sign::Plus => 1 };
        let mut out = okk_slot::default();
        status(unsafe { (c.host().bigint_new)(c.raw, sign, bytes.as_ptr(), bytes.len(), &mut out) })?;
        Ok(out)
    }
}
impl Read for String { fn read(c: &Call, s: okk_slot) -> Result<Self, u32> { c.text(s).map(str::to_owned) } }
impl Write for &str {
    fn write(self, c: &Call) -> Result<okk_slot, u32> {
        let mut out = okk_slot::default();
        status(unsafe { (c.host().text_new)(c.raw, self.as_ptr(), self.len(), &mut out) })?;
        Ok(out)
    }
}
impl Write for String { fn write(self, c: &Call) -> Result<okk_slot, u32> { self.as_str().write(c) } }
impl Write for &[u8] {
    fn write(self, c: &Call) -> Result<okk_slot, u32> {
        let mut out = okk_slot::default();
        status(unsafe { (c.host().bytes_new)(c.raw, self.as_ptr(), self.len(), &mut out) })?;
        Ok(out)
    }
}
impl<T: Read> Read for Vec<T> {
    fn read(c: &Call, s: okk_slot) -> Result<Self, u32> { T::read_vec(c, s) }
}
impl<T: Write> Write for Vec<T> {
    fn write(self, c: &Call) -> Result<okk_slot, u32> { T::write_vec(self, c) }
}
impl<K: Read + Ord, V: Read> Read for BTreeMap<K, V> {
    fn read(c: &Call, s: okk_slot) -> Result<Self, u32> {
        let mut len = 0;
        status(unsafe { (c.host().map_len)(c.raw, s, &mut len) })?;
        let mut keys = vec![okk_slot::default(); len]; let mut values = keys.clone();
        status(unsafe { (c.host().map_entries)(c.raw, s, keys.as_mut_ptr(), values.as_mut_ptr(), len) })?;
        keys.into_iter().zip(values).map(|(k, v)| Ok((K::read(c, k)?, V::read(c, v)?))).collect()
    }
}
impl<K: Write, V: Write> Write for BTreeMap<K, V> {
    fn write(self, c: &Call) -> Result<okk_slot, u32> {
        let (mut keys, mut values) = (Vec::new(), Vec::new());
        for (k, v) in self { keys.push(k.write(c)?); values.push(v.write(c)?); }
        let mut out = okk_slot::default();
        status(unsafe { (c.host().map_new)(c.raw, keys.as_ptr(), values.as_ptr(), keys.len(), &mut out) })?;
        Ok(out)
    }
}
impl<T: Read> Read for Option<T> {
    fn read(c: &Call, s: okk_slot) -> Result<Self, u32> {
        if s.kind == OKK_NONE { Ok(None) } else { T::read(c, s).map(Some) }
    }
}
impl<T: Write> Write for Option<T> {
    fn write(self, c: &Call) -> Result<okk_slot, u32> { match self { Some(v) => v.write(c), None => Ok(okk_slot::default()) } }
}
impl Write for () { fn write(self, _: &Call) -> Result<okk_slot, u32> { Ok(okk_slot::default()) } }
impl<T: Write> Write for Result<T, Failure> {
    fn write(self, c: &Call) -> Result<okk_slot, u32> { self.map_err(|e| c.fail(e))?.write(c) }
}

/// # Safety
/// Only `#[state]` and `extension!` implement this: the descriptor's drop must
/// destroy exactly `Payload<Self>`, and its identity must be in the state table.
pub unsafe trait State: Sized + 'static {
    const SHARED: bool;
    fn descriptor() -> &'static okk_state_type;
}
/// # Safety
/// The state macro sets `State::SHARED` and stores an Arc of a Send + Sync value.
pub unsafe trait SharedState: State + Send + Sync {}
pub enum Payload<T> { Plain(RefCell<Option<T>>), Shared(RefCell<Option<Arc<T>>>) }
impl Call {
    fn payload<T: State>(&self, slot: okk_slot) -> Result<&Payload<T>, u32> {
        let mut ptr = std::ptr::null_mut();
        status(unsafe { (self.host().state_get)(self.raw, slot, T::descriptor(), &mut ptr) })?;
        Ok(unsafe { &*ptr.cast::<Payload<T>>() })
    }
    fn busy(&self) -> u32 { self.fail(Failure::new("state_borrowed", "state has an outstanding borrow or shared job")) }
    pub fn borrow<T: State>(&self, s: okk_slot) -> Result<Ref<'_, T>, u32> {
        match self.payload::<T>(s)? {
            Payload::Plain(p) => Ref::filter_map(p.try_borrow().map_err(|_| self.busy())?, |v| v.as_ref()).map_err(|_| self.busy()),
            Payload::Shared(p) => Ref::filter_map(p.try_borrow().map_err(|_| self.busy())?, |v| v.as_deref()).map_err(|_| self.busy()),
        }
    }
    pub fn borrow_mut<T: State>(&self, s: okk_slot) -> Result<RefMut<'_, T>, u32> {
        match self.payload::<T>(s)? {
            Payload::Plain(p) => RefMut::filter_map(p.try_borrow_mut().map_err(|_| self.busy())?, |v| v.as_mut()).map_err(|_| self.busy()),
            Payload::Shared(p) => RefMut::filter_map(p.try_borrow_mut().map_err(|_| self.busy())?, |v| v.as_mut().and_then(Arc::get_mut)).map_err(|_| self.busy()),
        }
    }
    pub fn release<T: State>(&self, s: okk_slot) -> Result<(), u32> {
        status(unsafe { (self.host().state_release)(self.raw, s, T::descriptor()) })
    }
    pub fn take<T: State>(&self, s: okk_slot) -> Result<T, u32> {
        let value = match self.payload::<T>(s)? {
            Payload::Plain(p) => p.try_borrow_mut().map_err(|_| self.busy())?.take().ok_or_else(|| self.busy())?,
            Payload::Shared(p) => {
                let mut p = p.try_borrow_mut().map_err(|_| self.busy())?;
                let arc = p.take().ok_or_else(|| self.busy())?;
                match Arc::try_unwrap(arc) { Ok(v) => v, Err(arc) => { *p = Some(arc); return Err(self.busy()); } }
            }
        };
        status(unsafe { (self.host().state_release)(self.raw, s, T::descriptor()) })?;
        Ok(value)
    }
    pub fn state<T: State>(&self, value: T) -> Result<okk_slot, u32> {
        let payload = Box::new(if T::SHARED { Payload::Shared(RefCell::new(Some(Arc::new(value)))) }
            else { Payload::Plain(RefCell::new(Some(value))) });
        let ptr = Box::into_raw(payload);
        let mut out = okk_slot::default();
        let s = unsafe { (self.host().state_new)(self.raw, T::descriptor(), ptr.cast(), std::mem::size_of::<Payload<T>>(), &mut out) };
        if s != OKK_OK { unsafe { drop(Box::from_raw(ptr)); } return Err(s); }
        Ok(out)
    }
}
impl<T: SharedState> Read for Shared<T> {
    fn read(c: &Call, s: okk_slot) -> Result<Self, u32> {
        let Payload::Shared(p) = c.payload::<T>(s)? else { return Err(OKK_BAD_KIND); };
        Ok(Shared { inner: p.try_borrow().map_err(|_| c.busy())?.as_ref().ok_or_else(|| c.busy())?.clone() })
    }
}
/// # Safety
/// `ptr` is a unique payload allocated by `Call::state::<T>`.
pub unsafe extern "C" fn drop_state<T>(ptr: *mut c_void) {
    guard(|| { unsafe { drop(Box::from_raw(ptr.cast::<Payload<T>>())); } Ok(()) });
}

pub struct Job<A, R> { pub args: Option<A>, pub result: Option<Result<R, u32>> }
pub fn job<A: Send + 'static, R: Send + 'static>(args: A) -> *mut c_void {
    Box::into_raw(Box::new(Job::<A, R> { args: Some(args), result: None })).cast()
}
pub fn execute<R>(body: impl FnOnce() -> R) -> Result<R, u32> {
    match catch_unwind(AssertUnwindSafe(body)) { Ok(v) => Ok(v), Err(p) => { std::mem::forget(p); Err(OKK_PANIC) } }
}
