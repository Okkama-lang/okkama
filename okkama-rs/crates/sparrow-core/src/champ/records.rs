//! Fixed machine records. A service handle is a traced PAYLOAD reference, never an address of Rust storage.
use super::{HolderId, OwnerId};
use crate::heap::Payload;
use crate::word::{Repr, Word};
use std::any::Any;

pub type Outcome = u64;
pub type PlanId = u64;
pub type EntryId = u64;
pub const OUTCOME_STOP: Outcome = u64::MAX;
pub const NORMAL: Outcome = 0;
pub const UNWIND: Outcome = 1;
pub const CHUNK_ENTRIES: usize = 32;
pub const CLEANUP_ORDINARY: u64 = 0;
pub const CLEANUP_SHIELDED: u64 = 1;

macro_rules! slots {
    ($($name:ident),*) => {$ (
        #[derive(Debug, Default, PartialEq, Eq)]
        #[repr(C)]
        pub struct $name {
            pub record: Word,
        }
    )*};
}
slots!(PacketSlot, ScopeSlot, PendingSlot, CleanupSlot);

/// Fixed service storage used before packet materialization. Inactive guards always have a zero Ref.
#[derive(Debug, Default)]
#[repr(C)]
pub struct StaticEntry {
    pub object: Word,
    pub active: u64,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct ScopeInstance {
    pub holder: HolderId,
    pub owner: OwnerId,
    pub parent_owner: u64,
}

/// Checked selection roots. The iterative descriptor walk selects identities, never arbitrary Rust pointers.
#[derive(Debug)]
#[repr(C)]
pub struct Selection {
    pub roots: *const Word,
    pub count: u64,
}

#[derive(Debug, Default)]
#[repr(C)]
pub struct SplitPost {
    pub rest: PacketSlot,
    pub selected: PacketSlot,
    pub failure: Word,
}
#[derive(Debug, Default)]
#[repr(C)]
pub struct ScopeSplitPost {
    pub state: ScopeSlot,
    pub selected: PacketSlot,
    pub failure: Word,
}
#[derive(Debug)]
#[repr(C)]
pub struct TransferInputs {
    pub from: *mut ScopeSlot,
    /// Equal pointers mean Same. Both outputs then denote a single state, in `from` only.
    pub to: *mut ScopeSlot,
}
#[derive(Debug, Default)]
#[repr(C)]
pub struct TransferPost {
    pub from: ScopeSlot,
    pub to: ScopeSlot,
    pub failure: Word,
}
#[derive(Debug)]
#[repr(C)]
pub struct StageInputs {
    pub state: *mut ScopeSlot,
    /// Null for a packet-only/same-root stage; otherwise a distinct source scope, consumed with the root.
    pub source: *mut ScopeSlot,
    pub selection: PlanId,
    pub field: u64,
    pub value: Word,
    pub packet: *mut PacketSlot,
}
#[derive(Debug, Default)]
#[repr(C)]
pub struct StagePost {
    pub state: ScopeSlot,
    pub source: ScopeSlot,
}
#[derive(Debug)]
#[repr(C)]
pub struct ExitInputs {
    pub kind: u64,
    pub boundary: u64,
    pub value: Word,
    pub value_repr: u64,
    pub packet: *mut PacketSlot,
    pub selection: PlanId,
}
#[derive(Debug)]
#[repr(C)]
pub struct StaticReplacement {
    pub value: Word,
    pub value_repr: u64,
    pub packet: *mut PacketSlot,
    pub selection: PlanId,
    pub boundary: u64,
}
#[derive(Debug, Default)]
#[repr(C)]
pub struct ReplacePost {
    pub pending: PendingSlot,
    pub rest: PacketSlot,
}
#[derive(Debug)]
#[repr(C)]
pub struct ExitDestination {
    pub holder: HolderId,
    pub owner: OwnerId,
}
#[derive(Debug, Default)]
#[repr(C)]
pub struct CommitPost {
    pub value: Word,
    pub packet: PacketSlot,
}
#[derive(Debug, Default)]
#[repr(C)]
pub struct CallPost {
    pub value: Word,
    pub packet: PacketSlot,
    pub failure: Word,
}
pub type DeferPost = CallPost;
#[derive(Debug)]
#[repr(C)]
pub struct CallOwn {
    pub input: PacketSlot,
    pub entry_abi: EntryId,
    pub output: *mut CallPost,
    pub context: u64,
    pub cleanup_mode: u64,
}
#[derive(Debug)]
#[repr(C)]
pub struct RegistrationMap {
    pub replacements: u64,
}
#[derive(Debug, Default)]
#[repr(C)]
pub struct DestroyBorrow {
    pub object: Word,
    pub serial: u64,
    pub holder: u64,
    pub owner: u64,
    pub activation: Word,
}
#[derive(Debug, Default)]
#[repr(C)]
pub struct DestroyPost {
    pub failure: Word,
}

/// Generic owning-entry adapter: the checked span is independent of the legacy eight-argument ABI.
pub type OwnedEntry = extern "C" fn(*mut crate::runtime::Ctx, *mut CallOwn, *const Word, u64) -> Outcome;
#[derive(Debug, Clone)]
pub struct EntryContract {
    pub invoke: OwnedEntry,
    pub arguments: Vec<Repr>,
    pub replacements: u64,
    pub summary: u64,
    pub cleanup_mode: u64,
}

/// One known-holder/owner run. Each active journal entry is exactly one rooted Ref (8 bytes).
#[derive(Debug, Clone)]
pub(super) struct Run {
    pub holder: HolderId,
    pub owner: OwnerId,
    pub chunks: Vec<Vec<Word>>,
}
impl Run {
    pub fn words(&self) -> impl Iterator<Item = Word> + '_ {
        self.chunks.iter().flatten().copied()
    }
}
#[derive(Debug, Clone, Default)]
pub(super) struct Packet {
    /// Positional provenance: admission, bound history, staging. Never encoded in identity bits.
    pub journals: [Vec<Run>; 3],
}
impl Packet {
    pub fn runs(&self) -> impl Iterator<Item = &Run> {
        self.journals.iter().flatten()
    }
    pub fn runs_mut(&mut self) -> impl Iterator<Item = &mut Run> {
        self.journals.iter_mut().flatten()
    }
    pub fn words(&self) -> impl Iterator<Item = Word> + '_ {
        self.runs().flat_map(Run::words)
    }
    pub fn trace(&self, visit: &mut dyn FnMut(Word, Repr)) {
        for object in self.words() {
            visit(object, Repr::Ref);
        }
    }
    pub fn append(&mut self, other: Self) {
        for (destination, mut source) in self.journals.iter_mut().zip(other.journals) {
            destination.append(&mut source);
        }
    }
    pub fn from_run(run: Run) -> Self {
        Self { journals: [vec![run], Vec::new(), Vec::new()] }
    }
}
#[derive(Debug, Clone)]
pub(super) struct Scope {
    pub holder: HolderId,
    pub owner: OwnerId,
    pub parent_owner: u64,
    pub admission: Packet,
    pub bound: Packet,
    pub staging: Packet,
    pub bindings: Vec<(u64, Word, Repr)>,
    pub fields: Vec<(u64, Word, Repr)>,
}
impl Scope {
    pub fn all(&self) -> Packet {
        Packet { journals: [
            self.admission.runs().cloned().collect(),
            self.bound.runs().cloned().collect(),
            self.staging.runs().cloned().collect(),
        ] }
    }
}
#[derive(Debug, Clone)]
pub(super) struct Pending {
    pub kind: u64,
    pub boundary: u64,
    pub value: Word,
    pub selection: PlanId,
    pub value_repr: Repr,
    pub reserve: Packet,
}
#[derive(Debug, Clone, Default)]
pub(super) struct Cleanup {
    pub remainder: Packet,
    pub active: Option<(Word, u64, HolderId, OwnerId)>,
    pub invoked: bool,
    pub captured: Word,
    pub failure: Word,
}
#[derive(Debug, Clone)]
pub(super) enum Data {
    Packet(Packet),
    Scope(Scope),
    Pending(Pending),
    Cleanup(Cleanup),
    Consumed,
}
#[derive(Debug)]
pub(super) struct Record(pub Data);
impl Payload for Record {
    fn trace(&self, visit: &mut dyn FnMut(Word, Repr)) {
        match &self.0 {
            Data::Packet(packet) => packet.trace(visit),
            Data::Scope(scope) => {
                scope.admission.trace(visit);
                scope.bound.trace(visit);
                scope.staging.trace(visit);
                for &(_, value, repr) in scope.bindings.iter().chain(&scope.fields) {
                    visit(value, repr);
                }
            }
            Data::Pending(pending) => {
                pending.reserve.trace(visit);
                visit(pending.value, pending.value_repr);
            }
            Data::Cleanup(cleanup) => {
                cleanup.remainder.trace(visit);
                if let Some((object, _, _, _)) = cleanup.active {
                    visit(object, Repr::Ref);
                }
                visit(cleanup.failure, Repr::Ref);
                visit(cleanup.captured, Repr::Ref);
            }
            Data::Consumed => {}
        }
    }
    fn external_bytes(&self) -> usize {
        let packet_bytes = |packet: &Packet| {
            packet.journals.iter().map(|runs| runs.capacity() * size_of::<Run>()).sum::<usize>()
                + packet.runs().map(|run| {
                run.chunks.capacity() * size_of::<Vec<Word>>()
                    + run.chunks.iter().map(|chunk| chunk.capacity() * 8).sum::<usize>()
            }).sum::<usize>()
        };
        match &self.0 {
            Data::Packet(packet) => packet_bytes(packet),
            Data::Scope(scope) => {
                let values = (scope.bindings.capacity() + scope.fields.capacity()) * size_of::<(u64, Word, Repr)>();
                packet_bytes(&scope.admission) + packet_bytes(&scope.bound) + packet_bytes(&scope.staging) + values
            }
            Data::Pending(pending) => packet_bytes(&pending.reserve),
            Data::Cleanup(cleanup) => packet_bytes(&cleanup.remainder),
            Data::Consumed => 0,
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
