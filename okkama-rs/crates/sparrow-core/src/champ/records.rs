//! Fixed machine records. A service handle is a traced PAYLOAD reference, never an address of Rust storage.
use super::{HolderId, OwnerId};
use crate::heap::Payload;
use crate::word::{Repr, Word};
use std::any::Any;
use std::collections::{BinaryHeap, VecDeque};

pub type Outcome = u64;
pub type PlanId = u64;
pub type EntryId = u64;
pub const OUTCOME_STOP: Outcome = u64::MAX;
/// Class S transport (Champ draft 3 §1.1): never an outcome ordinal, and it carries no Post.
pub const OUTCOME_PENDING: Outcome = u64::MAX - 1;
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
    pub state: CleanupSlot,
    pub failure: Word,
}

/// `fut.new` (SPB 64): `entry` is the SPB index of an async body; `args` are the values of its continuation 0
/// in order (receiver, language arguments, descriptor parameters). Both slots are consumed on Normal only.
#[derive(Debug)]
#[repr(C)]
pub struct FutureNewIn {
    pub entry: u64,
    pub args: *const Word,
    pub argc: u64,
    pub inputs: *mut PacketSlot,
    pub owner: *mut ScopeSlot,
}
#[derive(Debug, Default)]
#[repr(C)]
pub struct FutureNewPost {
    pub future: Word,
    /// The owner's cache duty: one word, the Future frame (draft 3 §14.2).
    pub cache: Word,
    pub state: ScopeSlot,
}
/// `await` (SPB 66). Normal writes the outcome value (zero for a void Future); Unwind writes the failure, the
/// awaited Future as the task and the receipt.
#[derive(Debug, Default)]
#[repr(C)]
pub struct AwaitPost {
    pub value: Word,
    pub task: Word,
    pub receipt: u64,
}
#[derive(Debug, Default)]
#[repr(C)]
pub struct AwaitUnwindPost {
    pub state: ScopeSlot,
    pub failure: Word,
}
/// `fut.publish` (SPB 70): tag 0 publishes success (`value`, `packet`), tag 1 publishes `failure`.
#[derive(Debug)]
#[repr(C)]
pub struct PublishIn {
    pub tag: u64,
    pub value: Word,
    pub packet: *mut PacketSlot,
    pub failure: Word,
}
/// A one-word boolean result (`fut.cancelled`).
#[derive(Debug, Default)]
#[repr(C)]
pub struct FlagPost {
    pub value: u64,
}

/// Exactly one of scope/packet is non-null. Optional pending is consumed by begin.
/// Settlements names the checked synchronous protocol inventory, never a defer stack.
#[derive(Debug)]
#[repr(C)]
pub struct CloseInputs {
    pub scope: *mut ScopeSlot,
    pub packet: *mut PacketSlot,
    pub pending: *mut PendingSlot,
    pub failure: Word,
    pub settlements: u64,
}
#[derive(Debug, Default)]
#[repr(C)]
pub struct NextPost {
    pub state: CleanupSlot,
    pub key: super::KeyRef,
}
#[derive(Debug, Default)]
#[repr(C)]
pub struct FinishPost {
    /// Transport completion marker, not storage for the zero-word abstract Closed proof.
    /// J1 erases Closed and never copies this word into a service frame slot.
    pub closed: u64,
    pub failure: Word,
    /// Zero means no pending, including every failed close.
    pub pending: PendingSlot,
}
/// Relocated verified Low plan. Runtime IDs do not reuse function-local proof/plan IDs.
#[derive(Debug, Clone, Copy)]
pub struct CleanupPlan {
    pub reserve: bool,
    pub direct: bool,
    pub may_fail: bool,
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
#[derive(Debug)]
pub(super) struct Run {
    pub holder: HolderId,
    pub owner: OwnerId,
    pub chunks: VecDeque<VecDeque<Word>>,
    pub length: usize,
}
#[cfg(test)]
thread_local! {
    pub(super) static RUN_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
impl Clone for Run {
    fn clone(&self) -> Self {
        #[cfg(test)]
        RUN_WORK.with(|work| work.set(work.get() + self.length));
        Self { holder: self.holder, owner: self.owner, chunks: self.chunks.clone(), length: self.length }
    }
}
impl Run {
    pub fn front(&self) -> Option<Word> {
        self.chunks.front().and_then(|chunk| chunk.front()).copied()
    }
    pub fn back(&self) -> Option<Word> {
        self.chunks.back().and_then(|chunk| chunk.back()).copied()
    }
    /// Active storage only: popped slots disappear immediately, without tombstones or pruning.
    pub fn pop_front(&mut self) -> Option<Word> {
        #[cfg(test)]
        RUN_WORK.with(|work| work.set(work.get() + 1));
        let chunk = self.chunks.front_mut()?;
        let object = chunk.front_mut().map(std::mem::take);
        chunk.pop_front();
        if object.is_some() {
            self.length -= 1;
        }
        if chunk.is_empty() {
            self.chunks.pop_front();
        }
        object
    }
    pub fn pop_back(&mut self) -> Option<Word> {
        #[cfg(test)]
        RUN_WORK.with(|work| work.set(work.get() + 1));
        let chunk = self.chunks.back_mut()?;
        let object = chunk.back_mut().map(std::mem::take);
        chunk.pop_back();
        if object.is_some() {
            self.length -= 1;
        }
        if chunk.is_empty() {
            self.chunks.pop_back();
        }
        object
    }
    pub fn words(&self) -> impl Iterator<Item = Word> + '_ {
        self.chunks.iter().flatten().copied().inspect(|_| {
            #[cfg(test)]
            RUN_WORK.with(|work| work.set(work.get() + 1));
        })
    }
}
#[derive(Debug, Clone, Default)]
pub(super) struct Packet {
    /// Positional provenance: admission, bound history, staging. Never encoded in identity bits.
    pub journals: [VecDeque<Run>; 3],
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
        if run.length == 0 {
            return Self::default();
        }
        Self { journals: [VecDeque::from([run]), VecDeque::new(), VecDeque::new()] }
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
/// One heap head per ordered run. Pop/update is O(log r); no suffix copies or rescans.
#[derive(Debug, Clone, Default)]
pub(super) struct Stream {
    pub runs: Vec<Run>,
    pub heads: BinaryHeap<(u64, usize)>,
}
impl Stream {
    pub fn trace(&self, visit: &mut dyn FnMut(Word, Repr)) {
        for object in self.runs.iter().flat_map(Run::words) {
            visit(object, Repr::Ref);
        }
    }
}
#[derive(Debug, Clone, Default)]
pub(super) struct Cleanup {
    /// The closing scope's owner when the close began from a scope state: cache duties of other owners
    /// settle to nothing (draft 3 §6.2).
    pub owner: Option<OwnerId>,
    pub remainder: Packet,
    pub pending: Option<Pending>,
    pub stream: Stream,
    pub selected: Option<(Word, u64, HolderId, OwnerId)>,
    pub settlements: u64,
    pub settled: bool,
    pub planned: bool,
    pub aborted: bool,
    pub done: bool,
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
                cleanup.stream.trace(visit);
                if let Some(pending) = &cleanup.pending {
                    pending.reserve.trace(visit);
                    visit(pending.value, pending.value_repr);
                }
                if let Some((object, _, _, _)) = cleanup.selected {
                    visit(object, Repr::Ref);
                }
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
                run.chunks.capacity() * size_of::<VecDeque<Word>>()
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
            Data::Cleanup(cleanup) => {
                packet_bytes(&cleanup.remainder)
                    + cleanup.pending.as_ref().map_or(0, |pending| packet_bytes(&pending.reserve))
                    + cleanup.stream.runs.capacity() * size_of::<Run>()
                    + cleanup.stream.heads.capacity() * size_of::<(u64, usize)>()
                    + cleanup.stream.runs.iter().map(|run| {
                        run.chunks.capacity() * size_of::<VecDeque<Word>>()
                            + run.chunks.iter().map(|chunk| chunk.capacity() * 8).sum::<usize>()
                    }).sum::<usize>()
            }
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
