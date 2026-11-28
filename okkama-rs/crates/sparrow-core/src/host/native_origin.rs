//! Synchronous natives that no core provider implements yet, run by the
//! origin runtime's implementations (`okkama-runtime-core`). Arguments and
//! results cross as its values, converted by the ABI types of the native; an
//! origin `native object` stays an origin value inside a core handle.
//!
//! `any` arguments convert through their program types (`native_debug`).
//! Async natives of the terminal (`runtime/application`) are bridged too: the
//! origin returns a promise settled by its scheduler's deliveries (worker
//! threads), which the core pumps when they wake it. TLS runs over core
//! sockets (`native_tls`); other async natives have core providers.
use crate::host::*;
use okkama_runtime_abi::{Native, Type};
use okkama_runtime_core::heap::{ArrayHeap, Map, MapKey};
use okkama_runtime_abi::NativeFailure;
use okkama_runtime_core::{IntValue, Value, sched};
use std::rc::Rc;

pub(crate) fn handles(native: Native) -> bool {
    (!native.signature().is_async || native.module() == "runtime/application" || native.module() == "runtime/signal")
        // #1124: without the host runtime-core has only its pure providers; a native none of them provides is
        // unsupported here, not routed to the bridge's refusal.
        && (cfg!(feature = "host") || okkama_runtime_core::native::provides(native))
}

/// Core wakers of pending origin promises; origin deliveries wake them all.
static WAITING: std::sync::Mutex<Vec<std::task::Waker>> = std::sync::Mutex::new(Vec::new());

fn hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        sched::set_wake(|| {
            for waker in std::mem::take(&mut *WAITING.lock().expect("origin wakers")) {
                waker.wake();
            }
        })
    });
}

/// A pending origin promise; dropping it unsettled cancels it.
struct Pending(sched::Future);

impl Drop for Pending {
    fn drop(&mut self) {
        if self.0.running() {
            sched::cancel(&self.0);
        }
    }
}

/// An origin result, converted by its ABI type when the frame resumes.
struct Settled(Value, Type);

impl Output for Settled {
    fn publish(self: Box<Self>, heap: &mut Heap) -> Result<Slot, crate::natives::Refusal> {
        heap.from_origin(self.0, &self.1).map_err(Into::into)
    }
}

fn native_failure(failure: &okkama_runtime_core::failure::Failure, native: Native) -> NativeFailure {
    match &failure.kind {
        okkama_runtime_core::failure::Kind::Native(failure) => (**failure).clone(),
        _ => NativeFailure {
            domain: "core.runtime".into(),
            code: "unexpected".into(),
            operation: native.symbol().into(),
            message: failure.message().to_string(),
        },
    }
}

impl Heap {
    pub(crate) fn native_origin(&mut self, native: Native, args: &[Slot]) -> Result<Outcome, Error> {
        if native.module() == "runtime/debug" {
            if self.origin.objects.is_empty() {
                self.origin_names();
            }
            // Parts stay core values (see `native_debug`).
            if matches!(native, Native::DebugDataKeys | Native::DebugDataItems) {
                return self.debug_parts(native == Native::DebugDataKeys, args[0]).map(Outcome::Value);
            }
        }
        let values = args
            .iter()
            .zip(native.parameters())
            .map(|(&slot, ty)| {
                if native == Native::DebugDataDump {
                    self.dump_to_origin(slot)
                } else {
                    self.to_origin(slot, ty)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut context = std::mem::take(&mut self.origin);
        let result = okkama_runtime_core::native::execute(native, &values, &mut ArrayHeap, &mut context);
        self.origin = context;
        use okkama_runtime_core::native::Error as Origin;
        match result {
            Ok(Value::Future(promise)) if native.signature().is_async => self.origin_promise(native, promise),
            Ok(value) => self.from_origin(value, native.result()).map(Outcome::Value),
            Err(Origin::Failure(failure)) => {
                let payload = self.native_failure_new(failure)?;
                self.failure_new(FailureKind::Native(payload)).map(Outcome::Failure)
            }
            Err(Origin::InvalidState(message) | Origin::Trap(message) | Origin::Unsupported(message)) => {
                Err(Error(message))
            }
        }
    }

    /// A core future of an origin promise: settled already, or pumped from
    /// the origin scheduler whenever one of its deliveries wakes the core.
    fn origin_promise(&mut self, native: Native, promise: sched::Future) -> Result<Outcome, Error> {
        let ty = native.result().clone();
        if !promise.running() {
            let state = match promise.outcome() {
                Ok(value) => State::Ready(Ok(self.from_origin(value, &ty)?)),
                Err(failure) => {
                    let payload = self.native_failure_new(native_failure(&failure, native))?;
                    State::Ready(Err(self.failure_new(FailureKind::Native(payload))?))
                }
            };
            return self.future_with_state(state).map(Outcome::Value);
        }
        hook();
        let pending = Pending(promise);
        self.native_io(std::future::poll_fn(move |cx| {
            // Register before pumping: a delivery after the pump wakes us.
            WAITING.lock().expect("origin wakers").push(cx.waker().clone());
            sched::pump();
            if pending.0.running() {
                return std::task::Poll::Pending;
            }
            std::task::Poll::Ready(match pending.0.outcome() {
                Ok(value) => Ok(Settled(value, ty.clone())),
                Err(failure) => Err(native_failure(&failure, native)),
            })
        }))
        .map(Outcome::Value)
    }

    pub(crate) fn to_origin(&self, slot: Slot, ty: &Type) -> Result<Value, Error> {
        Ok(match ty {
            Type::Int => Value::Int(match self.integer(slot)? {
                Integer::Small(n) => IntValue::from_i64(n),
                Integer::Big(n) => IntValue::parse(&n.to_string()).ok_or_else(|| Error("integer".into()))?,
            }),
            Type::Float => Value::Float(f64::from_bits(slot.bits)),
            Type::Bool => Value::Bool(slot.bits != 0),
            Type::Byte => Value::Byte(slot.bits as u8),
            Type::Char => Value::Char(char::from_u32(slot.bits as u32).ok_or_else(|| Error("char".into()))?),
            Type::Str => Value::Str(Rc::from(self.text(slot)?)),
            Type::Array(element) if **element == Type::Byte => {
                ArrayHeap.allocate(self.byte_slice(slot)?.iter().map(|&b| Value::Byte(b)).collect())
            }
            Type::Array(element) => {
                let items = self.elements(slot)?;
                ArrayHeap.allocate(items.into_iter().map(|s| self.to_origin(s, element)).collect::<Result<_, _>>()?)
            }
            Type::Map(key, item) => {
                let mut entries = std::collections::BTreeMap::new();
                for (k, v) in self.map_entries(slot)? {
                    let k = self.to_origin(k, key)?;
                    let key = MapKey::from_value(&k).map_err(|e| Error(e.message))?;
                    entries.insert(key, self.to_origin(v, item)?);
                }
                Value::Map(Rc::new(Map::new(entries)))
            }
            Type::Handle(_) if slot.kind() != HANDLE => return Err(Error("expected a native object".into())),
            Type::Handle(_) => match self.handles.get(slot.bits)? {
                Handle::Origin(value) => value.clone(),
                _ => return Err(Error("expected an origin native object".into())),
            },
            Type::Any => self.any_to_origin(slot)?,
            // A Tomato rat is a payload around the origin's own Rat (#489).
            Type::Rat => match self.core().payload::<crate::ops::Rational>(slot.bits) {
                Some(rational) => Value::Rat(Rc::new(rational.0.clone())),
                None => return Err(Error("expected a rat".into())),
            },
            // #1083: `?T` comes with extensions (#1083 step 2); no std native takes it.
            Type::Void | Type::Maybe(_) => return Err(Error("native argument type is not bridged".into())),
        })
    }

    #[allow(clippy::wrong_self_convention)] // converts into this heap
    fn from_origin(&mut self, value: Value, ty: &Type) -> Result<Slot, Error> {
        Ok(match (ty, value) {
            (Type::Void, _) => Slot::boolean(true),
            (Type::Int, Value::Int(n)) => match n.to_i64() {
                Some(n) => Slot::small(n),
                None => self.allocate(n.to_string().parse().map_err(|_| Error("integer".into()))?)?,
            },
            (Type::Float, Value::Float(f)) => Slot { tag: FLOAT, bits: f.to_bits() },
            (Type::Bool, Value::Bool(b)) => Slot::boolean(b),
            (Type::Byte, Value::Byte(b)) => Slot { tag: BYTE, bits: u64::from(b) },
            (Type::Char, Value::Char(c)) => Slot { tag: CHAR, bits: u64::from(u32::from(c)) },
            (Type::Str, Value::Str(s)) => self.allocate_text(s.to_string())?,
            (Type::Array(element), value @ Value::Array(_)) if **element == Type::Byte => {
                let bytes: Vec<u8> = ArrayHeap
                    .elements(&value)
                    .map_err(|e| Error(e.message))?
                    .iter()
                    .map(|v| match v {
                        Value::Byte(b) => Ok(*b),
                        _ => Err(Error("expected byte".into())),
                    })
                    .collect::<Result<_, _>>()?;
                self.allocate_bytes(&bytes)?
            }
            (Type::Array(element), value @ Value::Array(_)) => {
                let items = ArrayHeap.elements(&value).map_err(|e| Error(e.message))?.to_vec();
                let slots = items.into_iter().map(|v| self.from_origin(v, element)).collect::<Result<Vec<_>, _>>()?;
                self.allocate_array(slots)?
            }
            (Type::Map(key, item), Value::Map(map)) => {
                let mut entries = Vec::with_capacity(map.len());
                for (k, v) in map.entries().iter() {
                    entries.push((self.from_origin(k.to_value(), key)?, self.from_origin(v.clone(), item)?));
                }
                self.map_new(entries.into_iter())?
            }
            (Type::Rat, Value::Rat(rat)) => {
                Slot { tag: OBJECT, bits: self.core_mut().allocate_payload(Box::new(crate::ops::Rational((*rat).clone()))) }
            }
            (Type::Handle(_), value) => Slot { tag: HANDLE, bits: self.handles.allocate(Handle::Origin(value))? },
            (ty, value) => return Err(Error(format!("native result {value:?} does not match {ty:?}"))),
        })
    }
}

#[cfg(test)]
mod tests {
    use okkama_runtime_abi::Native;

    /// Natives `natives.rs` answers itself without falling back to the host,
    /// although `classify` would send them to the bridge.
    fn answered_by_the_core(native: Native) -> bool {
        matches!(
            native,
            Native::Utf8Encode
                | Native::StdoutWrite
                | Native::StderrWrite
                | Native::StdoutFlush
                | Native::BytesBytesFind
                | Native::BytesBytesUtf8Invalid
                | Native::BytesBytesText
                | Native::BytesBytesDecimalInvalid
                | Native::BytesBytesDecimal
                | Native::BytesBytesEqual
                | Native::BytesBytesIntegerFits
                | Native::BytesBytesInteger
                | Native::BytesBytesJoin
        )
    }

    /// #1016: runtime-core provides exactly the natives the host routes to
    /// the bridge. A native routed here without a provider would fail at run
    /// time; a provider for a native the host serves itself is a twin.
    #[cfg(unix)]
    #[test]
    fn origin_routes_match_the_runtime_core_providers() {
        for &native in Native::ALL {
            let routed = crate::host::provider(native) == Some(crate::host::Provider::Origin)
                && !answered_by_the_core(native);
            let provided = okkama_runtime_core::native::provides(native);
            assert_eq!(routed, provided, "{}: routed to the bridge {routed}, provided {provided}", native.symbol());
        }
    }
}
