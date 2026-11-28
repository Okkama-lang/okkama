//! Opaque payload ownership stays in the watched handle arena, on the VM thread.
use super::*;
use crate::host::Handle;

#[derive(Debug)]
pub(crate) struct External {
    ty: &'static okk_state_type,
    shape: usize,
    payload: Option<*mut c_void>,
}
impl External {
    fn close(&mut self) {
        if let Some(payload) = self.payload.take() {
            // Registration requires a non-unwinding callback; null is an opaque payload too.
            unsafe { (self.ty.drop)(payload); }
        }
    }
}
impl Drop for External {
    fn drop(&mut self) { self.close(); }
}

impl Call<'_> {
    fn state_type(&self, ty: *const okk_state_type) -> Result<(&'static okk_state_type, Shape), u32> {
        // Compare addresses before dereferencing: even a forged type pointer is a clean fault.
        self.binding.states.iter().find(|(t, _)| std::ptr::eq(*t, ty)).cloned().ok_or(OKK_BAD_SHAPE)
    }

    pub(super) fn external(&self, slot: Slot) -> Result<&External, u32> {
        let shape = (slot.tag >> 8) as usize;
        match self.host.handles.get(slot.bits) {
            Ok(Handle::External(state)) if state.shape == shape => Ok(state),
            _ => Err(OKK_BAD_SHAPE),
        }
    }

    fn state(&mut self, value: okk_slot, ty: *const okk_state_type) -> Result<Slot, u32> {
        let (_, shape) = self.state_type(ty)?;
        let slot = self.entry(value)?;
        if value.kind != OKK_STATE { return Err(OKK_BAD_KIND); }
        let state = self.external(slot)?;
        if self.host.shapes[state.shape] != shape { return Err(OKK_BAD_SHAPE); }
        if state.payload.is_none() {
            self.failure("already_closed", "Native handle is already closed")?;
            return Err(OKK_FAILED);
        }
        Ok(slot)
    }
}

pub(super) unsafe extern "C" fn state_new(c: *mut okk_call, ty: *const okk_state_type, payload: *mut c_void,
    bytes: usize, out: *mut okk_slot) -> u32
{
    unsafe { guarded(c, |c| {
        let result = (|| {
            output(out)?;
            let (ty, shape) = c.state_type(ty)?;
            c.preflight(bytes, 1)?;
            // Finish all fallible host allocations before taking the foreign payload.
            let id = c.host.handles.allocate(Handle::External(External { ty, shape: c.host.shape_index[&shape], payload: None }))
                .map_err(|_| OKK_PANIC)?;
            let word = c.host.handle_object(id);
            let token = c.token(c.host.slot(word, &shape), OKK_STATE)?;
            c.host.core_mut().charge_external(bytes);
            if let Err(status) = c.reserve(0) {
                c.host.core_mut().discharge_external(bytes);
                return Err(status);
            }
            let Handle::External(state) = c.host.handles.get_mut(id).map_err(|_| OKK_PANIC)? else { return Err(OKK_PANIC); };
            state.payload = Some(payload);
            *out = token;
            Ok(())
        })();
        match result { Ok(()) => OKK_OK, Err(s) => s }
    }) }
}

pub(super) unsafe extern "C" fn state_get(c: *mut okk_call, value: okk_slot, ty: *const okk_state_type,
    out: *mut *mut c_void) -> u32
{
    unsafe { guarded(c, |c| {
        let result = (|| {
            output(out)?;
            let slot = c.state(value, ty)?;
            *out = c.external(slot)?.payload.unwrap();
            Ok(())
        })();
        match result { Ok(()) => OKK_OK, Err(s) => s }
    }) }
}

pub(super) unsafe extern "C" fn state_release(c: *mut okk_call, value: okk_slot, ty: *const okk_state_type) -> u32 {
    unsafe { guarded(c, |c| {
        let result = (|| {
            let slot = c.state(value, ty)?;
            let Handle::External(state) = c.host.handles.get_mut(slot.bits).map_err(|_| OKK_BAD_SHAPE)? else { return Err(OKK_BAD_SHAPE); };
            // Keep the type tombstone until the watched cell dies.
            state.close();
            Ok(())
        })();
        match result { Ok(()) => OKK_OK, Err(s) => s }
    }) }
}
