//! Champ G1 identity, packet/journal ABI and common cleanup executor.
//! Identity primitives below do not collect or call application code; X helpers declare those effects.
//! Callers keep refs in stack maps/Runtime.roots; neither a Rust local nor a serial is a GC root.
#![allow(unsafe_code)]

mod layout;
mod p1;
mod records;
mod abi;
pub use abi::*;
pub(crate) mod machine;
pub(crate) mod collections;
pub(crate) mod ordering;
pub use machine::{ExternalEntry, PlainEntry};
mod helpers;
pub(crate) mod testing;
pub mod cleanup;
pub use cleanup::{rt_cleanup_deliver_cancel, rt_cleanup_settle_dynamic, rt_exit_abort, rt_service_settle};
mod futures;
pub use futures::*;
mod groups;
mod bridge;
pub use records::*;
pub use helpers::*;
pub use okkama_sparrow::spb17::layout::{LayoutFacts, ObjectRuntimeLayout, RuntimeLayoutRegistry};

use crate::heap::{self, Heap};
use crate::runtime::Runtime;
use crate::word::Word;
use crate::Error;

pub use okkama_sparrow::spb17::layout::HAS_IDENTITY;
use okkama_sparrow::spb17::layout::{LIFE_MASK, LIFE_SHIFT};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum Life {
    Uncommitted = 0,
    Live = 1,
    Destroying = 2,
    Dead = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct HolderId(u64);

impl HolderId {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct OwnerId(u64);

impl OwnerId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Materialized key. Its object word MUST remain rooted while this key is used.
/// S2 packets/services trace object(), never serial(); no unrooted address lookup.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct KeyRef {
    object: Word,
    serial: u64,
}

const _: () = assert!(size_of::<KeyRef>() == 16);

impl KeyRef {
    pub fn object(self) -> Word {
        self.object
    }

    pub fn serial(self) -> u64 {
        self.serial
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub serial: u64,
    pub holder_id: u64,
    pub owner_id: u64,
    pub epoch: u64,
    pub life: Life,
}

/// State belongs to a Runtime, persists across invocations and never wraps.
#[derive(Debug, Default)]
pub(crate) struct State {
    pub(crate) machine: Option<machine::Program>,
    #[cfg(test)]
    pub(crate) destroy_events: Vec<u64>,
    cleanup_floor: Option<u64>,
    serial: u64,
    holder: u64,
    owner: u64,
    trace: Option<std::fs::File>,
    pub(crate) captured_trace: Option<Vec<u8>>,
    selections: Vec<Vec<usize>>,
    entries: Vec<EntryContract>,
    cleanup_plans: Vec<CleanupPlan>,
    rejection_floor: Option<u64>,
    rejection_plan: Option<PlanId>,
    value_reprs: std::collections::BTreeMap<(u64, u64, bool), crate::word::Repr>,
    /// Relocated D16 schedules: the service kind of each duty in order (`SERVICE_CACHE`, or `SERVICE_TASK` for a
    /// spawned Future's task duty, D24.2), by runtime id minus one.
    schedules: Vec<Vec<u64>>,
    /// The owner of caches detached to the root (draft 3 §6.2).
    root_owner: Option<OwnerId>,
    #[cfg(test)]
    pub(crate) future_events: Vec<futures::FutureEvent>,
    /// The receipts `await.unwind` consumed, with the function of the frame that consumed each.
    #[cfg(test)]
    pub(crate) receipts: Vec<(usize, u64)>,
}

impl State {
    pub(crate) fn new() -> Self {
        Self {
            trace: std::env::var_os("SPARROW_TRACE_DESTROY")
                .and_then(|path| std::fs::OpenOptions::new().create(true).append(true).open(path).ok()),
            ..Self::default()
        }
    }
}

fn next(value: u64) -> Result<u64, Error> {
    value.checked_add(1).ok_or_else(|| Error("Champ identity counter exhausted".into()))
}

impl Heap {
    fn identity_address(&self, object: Word) -> Option<Word> {
        let header = self.header_of(object);
        // HAS_IDENTITY overlaps array/string flags: kind must be checked first.
        if heap::kind(header) != heap::OBJECT || header & HAS_IDENTITY == 0 {
            return None;
        }
        let layout = self.object_runtime_layout(heap::payload(header));
        layout.identity_offset().map(|offset| object + offset as Word)
    }

    pub fn champ_identity(&self, object: Word) -> Option<Identity> {
        let address = self.identity_address(object)?;
        let header = self.header_of(object);
        let life = match (header & LIFE_MASK) >> LIFE_SHIFT {
            0 => Life::Uncommitted,
            1 => Life::Live,
            2 => Life::Destroying,
            _ => Life::Dead,
        };
        // SAFETY: the final descriptor reserves four raw words at identity_offset.
        let tail = unsafe { std::slice::from_raw_parts(address as *const u64, 4) };
        Some(Identity { serial: tail[0], holder_id: tail[1], owner_id: tail[2], epoch: tail[3], life })
    }

    fn identity_write(&mut self, object: Word, identity: Identity) {
        let address = self.identity_address(object).expect("identity descriptor");
        // SAFETY: valid descriptor-backed OBJECT with its own four-word tail.
        unsafe {
            let tail = std::slice::from_raw_parts_mut(address as *mut u64, 4);
            tail.copy_from_slice(&[identity.serial, identity.holder_id, identity.owner_id, identity.epoch]);
        }
        self.identity_life(object, identity.life);
    }

    fn identity_life(&mut self, object: Word, life: Life) {
        // SAFETY: only checked transitions of a live allocation call this method.
        unsafe {
            let header = &mut *(object as *mut u64);
            *header = (*header & !LIFE_MASK) | ((life as u64) << LIFE_SHIFT);
        }
    }

    /// Storage fields only. Identity is diagnostic raw metadata, never a value field.
    pub fn champ_dump_object(&self, object: Word) -> String {
        let header = self.header_of(object);
        assert_eq!(heap::kind(header), heap::OBJECT);
        let descriptor = heap::payload(header);
        let layout = self.object_runtime_layout(descriptor);
        let fields: Vec<_> = (0..layout.storage_field_count()).map(|index| self.field(object, index)).collect();
        let mut dump = format!("object {descriptor} fields={fields:?}");
        if let Some(identity) = self.champ_identity(object) {
            dump.push_str(&format!(
                " identity serial={} holder={} owner={} epoch={} life={:?}",
                identity.serial, identity.holder_id, identity.owner_id, identity.epoch, identity.life,
            ));
        }
        dump
    }
}

impl Runtime {
    pub fn champ_new_holder(&mut self) -> Result<HolderId, Error> {
        self.champ.holder = next(self.champ.holder)?;
        Ok(HolderId(self.champ.holder))
    }

    /// A function-local D16 schedule relocated to a nonzero runtime id: its duties, by service kind, settle in order.
    pub fn champ_register_schedule(&mut self, duties: Vec<u64>) -> u64 {
        self.champ.schedules.push(duties);
        self.champ.schedules.len() as u64
    }

    pub fn champ_new_owner(&mut self) -> Result<OwnerId, Error> {
        self.champ.owner = next(self.champ.owner)?;
        Ok(OwnerId(self.champ.owner))
    }

    /// Completion primitive: preflight before all stores, then publish the key without
    /// a safepoint/callback/fallible action. S2 installs this key in the output packet.
    pub fn champ_create_commit(
        &mut self,
        object: Word,
        holder: HolderId,
        owner: OwnerId,
    ) -> Result<Option<KeyRef>, Error> {
        let Some(identity) = self.heap.champ_identity(object) else {
            return Ok(None);
        };
        if identity.life != Life::Uncommitted || identity.serial != 0 || identity.epoch != 0
            || identity.holder_id != 0 || identity.owner_id != 0
        {
            return Err(Error("Champ creation already committed".into()));
        }
        let serial = next(self.champ.serial)?;
        self.heap.identity_write(object, Identity {
            serial,
            holder_id: holder.0,
            owner_id: owner.0,
            epoch: 1,
            life: Life::Live,
        });
        self.champ.serial = serial;
        Ok(Some(KeyRef { object, serial }))
    }

    fn champ_check_key(&self, key: KeyRef, holder: HolderId, owner: OwnerId, life: Life)
        -> Result<Identity, Error>
    {
        let identity = self.heap.champ_identity(key.object)
            .ok_or_else(|| Error("Champ object has no identity".into()))?;
        if identity.serial != key.serial || identity.holder_id != holder.0
            || identity.owner_id != owner.0 || identity.life != life
        {
            return Err(Error("Champ key, holder, owner or life mismatch".into()));
        }
        Ok(identity)
    }

    /// Identity-level transition only; S2 additionally checks consumed cleanup-state authority.
    pub fn champ_begin_destroy(&mut self, key: KeyRef, holder: HolderId, owner: OwnerId) -> Result<(), Error> {
        self.champ_check_key(key, holder, owner, Life::Live)?;
        self.heap.identity_life(key.object, Life::Destroying);
        #[cfg(test)]
        self.champ.destroy_events.push(key.serial);
        if let Some(mut trace) = self.champ.trace.take() {
            let _ = self.champ_trace_destroy(key, &mut trace);
            self.champ.trace = Some(trace);
        }
        if let Some(mut trace) = self.champ.captured_trace.take() {
            let _ = self.champ_trace_destroy(key, &mut trace);
            self.champ.captured_trace = Some(trace);
        }
        Ok(())
    }

    /// Call only after destroy's body AND its local cleanup, including failure capture.
    pub fn champ_finish_destroy(&mut self, key: KeyRef, holder: HolderId, owner: OwnerId) -> Result<(), Error> {
        self.champ_check_key(key, holder, owner, Life::Destroying)?;
        self.heap.identity_life(key.object, Life::Dead);
        Ok(())
    }

    pub fn champ_trace_destroy(&self, key: KeyRef, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        let descriptor = heap::payload(self.heap.header_of(key.object)) as usize;
        writeln!(out, "{} {}", self.loaded.program.objects[descriptor].name, key.serial)
    }
}

/// Sweep is not a finalizer. Losing a committed obligation indicates missing roots/cleanup.
pub(crate) fn check_sweep(header: u64, stress: bool) {
    if (cfg!(debug_assertions) || stress) && heap::kind(header) == heap::OBJECT && header & HAS_IDENTITY != 0 {
        let life = (header & LIFE_MASK) >> LIFE_SHIFT;
        assert!(life == Life::Uncommitted as u64 || life == Life::Dead as u64, "lost Champ live identity");
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod runtime_tests;

#[cfg(test)]
mod executor_tests;

#[cfg(test)]
mod scalar_tests;

#[cfg(test)]
mod async_tests;
