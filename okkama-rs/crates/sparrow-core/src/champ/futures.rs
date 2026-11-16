//! Champ Futures (draft 3 §§2–7, plan decision C3): a Future is a Tomato `FRAME` with Champ words appended.
//! Tomato's fixed words keep their offsets, so `Runtime::schedule`, `wake`, the deadlock check, cancellation
//! delivery and `shutdown` run Champ frames unchanged. Word OWNED, Tomato's outcome region (unused under
//! `champ-abi`), holds the cache record. The D21 slot area and the scope area follow the fixed words, then the
//! tail: INPUTS, EMERGENCY, STEP and TASK.
//!
//! A body runs as `(ctx, frame) -> 0 | 1` (done or pending) and publishes its own outcome; the JIT (V9) owns
//! resume dispatch and copy-on-suspend, and these helpers own everything that touches the Future protocol.
#![allow(clippy::not_unsafe_ptr_arg_deref)]
use super::helpers::{consume, error, packet, preflight, replace, scope, stop, store, transition};
use super::records::*;
use super::{HolderId, OwnerId};
use crate::heap::{self, Payload};
use crate::runtime::{self, frame_pointer, Ctx, FailureKind, Runtime, EXHAUSTED, FAILED, OK};
use crate::tasks::{self, champ_flags as flags, get, set};
use crate::word::{Repr, Word, UNBOUND};
use crate::Error;
use okkama_sparrow::spb17 as s;

/// The cache record (draft 3 §2.2) lives in Tomato's OWNED word.
pub const CACHE: u64 = tasks::OWNED;
/// The first word of the D21 slot area: Tomato's first register word.
pub const AREA: u64 = tasks::REGISTERS;
/// Tail words, as offsets from `FrameMap::tail`: a driver's unaccepted inputs (always 0 for a body), the
/// emergency failure, the in-flight step record and the cancellation mirror.
pub const INPUTS: u64 = 0;
pub const EMERGENCY: u64 = 8;
pub const STEP: u64 = 16;
pub const TASK: u64 = 24;
const TAIL_WORDS: usize = 4;
/// FLAGS bits after Tomato's (`tasks.rs`). Draft 3 §2.1 numbers them from 256, which Tomato already uses for
/// a suspended await (#663); that bit is draft 3's WAITED and keeps Tomato's meaning.
pub const CHAMP: u64 = 512;
pub const DETACHED: u64 = 1024;
/// A delivered cancellation reached this waiter as the sole awaiter of its spawned child (#147): the child was
/// cancelled and the waiter stays registered until it settles. Set with DELIVER_NOW by Tomato's `deliver_cancel`
/// and consumed with it by the await, whose receipt it decides: ChildCancelled with it, Withdrawn without.
pub const CHILD_CANCEL: u64 = 2048;
/// `CacheRecord::flags`: the cache duty has been settled.
pub const RELEASED: u64 = 1;
/// `CacheRecord::flags`: the cache duty is a receipt in an exit reserve (draft 3 §12): custody stays with the
/// reserve until `exit.commit` moves it to the target or an abort settles it in S.
pub const REHOMED: u64 = 2;
/// `CacheRecord::flags`: `await.take` consumed the outcome (draft 3 §4.2).
pub const CONSUMED_BY_TAKE: u64 = 4;
/// Await receipts (draft 3 §14.2).
pub const RECEIPT_NOT_STARTED: u64 = 0;
pub const RECEIPT_SETTLED: u64 = 1;
pub const RECEIPT_WITHDRAWN: u64 = 2;
pub const RECEIPT_CHILD_CANCELLED: u64 = 3;

/// One D21 slot: its byte offset from the frame word and the representation of each of its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotMap {
    pub offset: u64,
    pub reprs: Vec<Repr>,
}

/// The frame of one async body (draft 3 §§2.1, 2.3; ABI D21 §3.1), from its loaded layout. Offsets are bytes
/// from the frame word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameMap {
    pub slots: Vec<SlotMap>,
    /// The two Raw words (holder id, owner id) of each lexical scope of the function, by scope id.
    pub scopes: Vec<(u32, u64)>,
    /// The first tail word; INPUTS, EMERGENCY, STEP and TASK are offsets from it.
    pub tail: u64,
    /// Continuation 0: the slots of the language inputs in order and their deps, then the slot of the input
    /// packet. A fixed root's start is empty (#667).
    pub start: Vec<u32>,
    /// Allocation words, the header included.
    pub words: usize,
    /// The collector's layout after the header (`heap::Layouts::frames`).
    pub trace: Vec<Repr>,
    /// A driver frame (async ABI D22, draft 3 §§2.1, 9): the number of its language arguments. Its area is
    /// Tomato's driver layout (registers, the driver slot, the retained arguments), its INPUTS tail word holds the
    /// creation's inputs packet, and nothing compiles it.
    pub driver: Option<usize>,
}

impl FrameMap {
    /// The map of `function`, or None when it is not an async body.
    pub fn new(program: &s::Program, function: usize) -> Option<Self> {
        let source = &program.functions[function];
        let layout = program.frame_layout(source)?;
        let result = source.results.first().map_or(Repr::Raw, |&ty| super::machine::repr(ty));
        // STATE, RESULT, FAILURE, WAITER, AWAITING, RESUME, FLAGS, CACHE.
        let mut trace = vec![Repr::Raw, result, Repr::Ref, Repr::Ref, Repr::Ref, Repr::Raw, Repr::Raw, Repr::Ref];
        let mut offset = AREA;
        let mut slots = Vec::new();
        for slot in &layout.slots {
            let reprs = slot_reprs(slot.ty);
            slots.push(SlotMap { offset, reprs: reprs.clone() });
            offset += 8 * reprs.len() as u64;
            trace.extend(reprs);
        }
        let mut scopes = Vec::new();
        for (id, scope) in program.tables.scopes.iter().enumerate() {
            if scope.function as usize == function {
                scopes.push((id as u32, offset));
                offset += 16;
                trace.extend([Repr::Raw, Repr::Raw]);
            }
        }
        let tail = offset;
        trace.extend([Repr::Ref; TAIL_WORDS]);
        let start = layout.continuations.first()?.saved.clone();
        Some(Self { slots, scopes, tail, start, words: 1 + trace.len(), trace, driver: None })
    }

    /// The map of the async external `function` (D22, #601 Q1), from its legacy driver layout `legacy` (Tomato's
    /// fixed words, registers, driver slot and retained arguments): the tail follows it. None for any other
    /// function.
    pub fn driver(program: &s::Program, function: usize, legacy: &[Repr]) -> Option<Self> {
        let source = &program.functions[function];
        if source.entry_abi != s::EntryAbi::AsyncExternal {
            return None;
        }
        let tail = 8 * (1 + legacy.len() as u64);
        let mut trace = legacy.to_vec();
        trace.extend([Repr::Ref; TAIL_WORDS]);
        let driver = Some(source.params.len());
        Some(Self { slots: vec![], scopes: vec![], tail, start: vec![], words: 1 + trace.len(), trace, driver })
    }

    /// The number of language inputs of continuation 0: the deps of borrowed inputs take no words. A driver takes
    /// its arguments.
    pub fn inputs(&self) -> usize {
        if let Some(arguments) = self.driver {
            return arguments;
        }
        self.start.split_last().map_or(0, |(_, inputs)| {
            inputs.iter().filter(|&&slot| !self.slots[slot as usize].reprs.is_empty()).count()
        })
    }

    /// The offset of the input packet's slot; a fixed root, whose start is empty (#667), has none. A driver keeps
    /// its inputs in the INPUTS tail word (draft 3 §2.1).
    pub fn packet(&self) -> Option<u64> {
        if self.driver.is_some() {
            return Some(self.tail + INPUTS);
        }
        self.start.last().map(|&slot| self.slots[slot as usize].offset)
    }
}

/// Words of a slot: one per value, `service_layout` words per service kind (ABI D21 §3.1, review B3).
fn slot_reprs(ty: s::ProducedType) -> Vec<Repr> {
    match ty {
        s::ProducedType::Value(ty) => vec![super::machine::repr(ty)],
        s::ProducedType::Service(kind) => {
            let layout = s::layout::service_layout(kind);
            (0..layout.words).map(|word| {
                if layout.references.contains(&(word * 8)) { Repr::Ref } else { Repr::Raw }
            }).collect()
        }
    }
}

/// The outcome cache of one Future (draft 3 §2.2), allocated with its frame before any input is accepted.
#[derive(Debug)]
pub struct CacheRecord {
    /// The outcome packet of a success. Its runs have this cache's holder and owner.
    pub packet: Word,
    pub owner: OwnerId,
    pub holder: HolderId,
    pub flags: u64,
}

impl Payload for CacheRecord {
    fn trace(&self, visit: &mut dyn FnMut(Word, Repr)) {
        visit(self.packet, Repr::Ref);
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// A test-only record of the Future protocol, kept by the runtime for every body, compiled or not: starts,
/// resumes and suspensions as `run_body` sees them, publications and the queue entries they make.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FutureStep {
    Start,
    Resume,
    Pending,
    Published,
    Queued,
}

/// One step of one frame, with the frame's function read while the frame was alive.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FutureEvent {
    pub step: FutureStep,
    pub frame: Word,
    pub function: usize,
}

/// What a cache-duty settle does (draft 3 §6.2, ABI D24.2), decided before anything changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CacheSettle {
    /// The cache belongs to another owner.
    Nothing,
    /// A running Future detaches to the root: not cancelled, its inputs and waiters stay with it.
    Detach,
    /// The inputs packet (body never started, or cancelled before start) and a success's outcome packet join
    /// the settled run; a cold Future settles as Cancelled.
    Release { inputs: Word, outcome: Word, cold: bool },
}

fn bridge_clear(ctx: *mut Ctx) -> bool {
    unsafe { (*ctx).status == OK && (*ctx).failure == 0 }
}

impl Runtime {
    /// The frame map of an async body.
    pub fn champ_frame_map(&self, function: usize) -> Option<&FrameMap> {
        self.champ.machine.as_ref()?.frames.get(function)?.as_ref()
    }

    /// Whether `function` of the loaded Champ program is an async body (a driver is not).
    pub(crate) fn champ_async_body(&self, function: usize) -> bool {
        self.champ_frame_map(function).is_some_and(|map| map.driver.is_none())
    }

    /// A Champ body frame: a FRAME whose flags carry CHAMP and whose function has a frame map.
    fn champ_body(&self, frame: Word) -> Result<&FrameMap, Error> {
        if frame == 0 || frame & 7 != 0 || heap::kind(self.heap.header_of(frame)) != heap::FRAME
            || get(frame, tasks::FLAGS) & CHAMP == 0
        {
            return Err(error("expected a Champ Future"));
        }
        self.champ_frame_map(self.function_of(frame)).ok_or_else(|| error("Future of a function without a frame"))
    }

    fn champ_tail(&self, frame: Word) -> Result<u64, Error> {
        Ok(self.champ_body(frame)?.tail)
    }

    pub fn champ_cache(&self, future: Word) -> Result<&CacheRecord, Error> {
        self.champ_body(future)?;
        self.heap.payload::<CacheRecord>(get(future, CACHE)).ok_or_else(|| error("Future without a cache"))
    }

    pub(super) fn champ_cache_mut(&mut self, future: Word) -> Result<&mut CacheRecord, Error> {
        self.champ_body(future)?;
        self.heap.payload_mut::<CacheRecord>(get(future, CACHE)).ok_or_else(|| error("Future without a cache"))
    }

    /// The frame whose cancellation flags govern `frame` (draft 3 §5): its TASK word, or itself.
    pub(super) fn champ_task_of(&self, frame: Word) -> Result<Word, Error> {
        let task = get(frame, self.champ_tail(frame)? + TASK);
        Ok(if task == 0 { frame } else { task })
    }

    /// The owner of everything detached to the root (draft 3 §6.2, ABI D24.2).
    pub(super) fn champ_root_owner(&mut self) -> Result<OwnerId, Error> {
        if let Some(owner) = self.champ.root_owner {
            return Ok(owner);
        }
        let owner = self.champ_new_owner()?;
        self.champ.root_owner = Some(owner);
        Ok(owner)
    }

    /// Creation step 1 (draft 3 §2.4): the frame, its cache record and its emergency failure. This allocates
    /// and touches no input; the caller collects afterwards with the result rooted.
    pub(super) fn champ_future_alloc(&mut self, function: usize, owner: OwnerId) -> Result<Word, Error> {
        let map = self.champ_frame_map(function).ok_or_else(|| error("fut.new of a function that is not async"))?;
        let (words, tail) = (map.words, map.tail);
        let holder = self.champ_new_holder()?;
        self.count(|runtime| format!("future {}", runtime.loaded.program.functions[function].symbol));
        self.heap.counted("async frame", 0, words);
        let frame = self.heap.allocate(words);
        // SAFETY: fresh storage of `words` words; every word is written before the next collection.
        unsafe {
            *(frame as *mut u64) = heap::header(heap::FRAME, Repr::Raw, function as u32);
            std::ptr::write_bytes((frame + 8) as *mut u64, 0, words - 1);
        }
        let cache = self.heap.allocate_payload(Box::new(CacheRecord { packet: 0, owner, holder, flags: 0 }));
        let emergency = self.failure_new(FailureKind::Cancelled);
        set(frame, tasks::FLAGS, CHAMP);
        set(frame, CACHE, cache);
        set(frame, tail + EMERGENCY, emergency);
        Ok(frame)
    }

    /// Creation step 2: continuation 0 holds the language inputs (their deps take no words), then the accepted
    /// input packet. A driver frame takes Tomato's start (`future_new`): raw registers unbound, the arguments in
    /// their registers and in the retained words, and the packet in INPUTS (draft 3 §2.4).
    fn champ_write_start(&self, frame: Word, args: &[Word], packet: Word) {
        let function = self.function_of(frame);
        let map = self.champ_frame_map(function).expect("Champ frame map");
        if map.driver.is_some() {
            use crate::program::Kind;
            let kinds = &self.loaded.functions[function].kinds;
            for (register, kind) in kinds.iter().enumerate() {
                if matches!(kind, Kind::Float | Kind::Raw | Kind::Byte) {
                    set(frame, AREA + register as u64 * 8, crate::word::UNBOUND_RAW);
                }
            }
            let retained = AREA + 8 * (kinds.len() as u64 + 1);
            let params = &self.loaded.program.functions[function].params;
            for (index, (param, &argument)) in params.iter().zip(args).enumerate() {
                set(frame, AREA + u64::from(param.0) * 8, argument);
                set(frame, retained + index as u64 * 8, argument);
            }
            set(frame, map.tail + INPUTS, packet);
            return;
        }
        let inputs = map.start.split_last().map_or(&[][..], |(_, inputs)| inputs);
        let words = inputs.iter().filter(|&&slot| !map.slots[slot as usize].reprs.is_empty());
        for (&slot, &word) in words.zip(args) {
            set(frame, map.slots[slot as usize].offset, word);
        }
        if let Some(offset) = map.packet() {
            set(frame, offset, packet);
        }
    }

    /// Start a cold Future on this stack (draft 3 §4.3). A Future started by another inherits its task; a
    /// spawned, shielded or root Future is its own task. Its body reads continuation 0.
    pub(crate) fn champ_future_start(&mut self, ctx: *mut Ctx, starter: Word, future: Word) {
        set(future, tasks::STATE, tasks::RUNNING);
        let own = starter == 0 || get(future, tasks::FLAGS) & (flags::SPAWNED | flags::SHIELDED) != 0;
        let task = if own { Ok(future) } else { self.champ_task_of(starter) };
        match (task, self.champ_tail(future)) {
            (Ok(task), Ok(tail)) => set(future, tail + TASK, task),
            (Err(failure), _) | (_, Err(failure)) => {
                runtime::fatal(unsafe { &mut *ctx }, self, failure.0);
                return;
            }
        }
        self.run_body(ctx, future);
    }

    /// `run_body` of a Champ body (draft 3 §3): compiled as `(ctx, frame) -> 0 | 1`. A body publishes its own
    /// outcome, so a failure status coming back is an engine error. True when the body is no longer pending.
    pub(crate) fn champ_run_body(&mut self, ctx: *mut Ctx, frame: Word) -> bool {
        let function = self.function_of(frame);
        let code = match crate::jit::compile(self, function) {
            Ok(code) => code,
            Err(failure) => {
                runtime::fatal(unsafe { &mut *ctx }, self, failure.0);
                return true;
            }
        };
        #[cfg(test)]
        self.champ_event(if get(frame, tasks::RESUME) == 0 { FutureStep::Start } else { FutureStep::Resume }, frame);
        self.running.push(frame);
        // SAFETY: an async body takes the context and its frame.
        let pending = unsafe { crate::jit::invoke(code, ctx, &[frame]) } != 0;
        self.running.pop();
        #[cfg(test)]
        if pending {
            self.champ_event(FutureStep::Pending, frame);
        }
        let context = unsafe { &mut *ctx };
        match context.status {
            OK => {}
            FAILED => {
                let message = "Champ: an async body returned a failure instead of publishing it";
                runtime::fatal(context, self, message.into());
                return true;
            }
            _ => return true,
        }
        if pending != (get(frame, tasks::STATE) == tasks::RUNNING) {
            runtime::fatal(context, self, "Champ: an async body's result disagrees with its Future".into());
            return true;
        }
        !pending
    }

    /// The root driver of an async entry (draft 3 §3, plan §3.6 "Root"), as Tomato runs `async main`: the root
    /// Future on this stack, the scheduler until it settles, then shutdown with the report of the first
    /// unobserved spawned failure. The root is its own task, owned by the root. A fixed root starts empty; a root
    /// body with inputs (in tests) is offered them and an empty packet.
    pub(crate) fn champ_run_async_main(self: &mut Box<Self>, ctx: &mut Ctx, entry: usize, arguments: &[Word])
        -> Result<Word, Error>
    {
        let owner = self.champ_root_owner()?;
        let expected = self.champ_frame_map(entry).map(FrameMap::inputs);
        if expected != Some(arguments.len()) {
            return Err(error("async entry argument count"));
        }
        let root = self.champ_future_alloc(entry, owner)?;
        self.roots.push((root, Repr::Ref));
        // A fixed root has no caller (#667): its start is empty and nothing is written into continuation 0.
        if self.champ_frame_map(entry).is_some_and(|map| !map.start.is_empty()) {
            let inputs = store(self, Data::Packet(Packet::default()));
            self.champ_write_start(root, arguments, inputs);
        }
        self.champ_future_start(ctx, 0, root);
        if ctx.status == OK {
            self.schedule(ctx, root);
        }
        if ctx.status == OK && let Some(failure) = self.champ_shutdown(ctx)? {
            let message = self.describe(ctx, failure);
            return Err(Error(message));
        }
        if ctx.status == OK {
            match get(root, tasks::STATE) {
                tasks::FAILURE_STATE => {
                    ctx.failure = get(root, tasks::FAILURE);
                    ctx.status = FAILED;
                }
                tasks::DONE => {
                    let outcome = PacketSlot { record: self.champ_cache(root)?.packet };
                    if !self.champ_packet_keys(&outcome)?.is_empty() {
                        return Err(error("main returned an ownership packet"));
                    }
                    self.champ_cache_mut(root)?.packet = 0;
                }
                _ => return Err(error("the root Future did not settle")),
            }
        }
        let result = get(root, tasks::RESULT);
        match ctx.status {
            OK => Ok(result),
            FAILED => {
                ctx.status = OK;
                let failure = ctx.failure;
                Err(Error(self.describe(ctx, failure)))
            }
            EXHAUSTED => Err(self.exhausted_report()),
            _ => Err(Error(self.fatal.clone().unwrap_or_else(|| "engine failure".into()))),
        }
    }

    /// Shutdown after the root Future settled (draft 3 §7.3, ABI D24.2, #699): cancel every running registered task
    /// and schedule until all have settled; close the outcome packets of the detached Futures that succeeded in one
    /// serial plan at the root boundary, their packets' own service entries settled first (D25.4); report the first
    /// unobserved, non-Cancelled spawned failure in spawn order. A closed packet can detach more running Futures
    /// (an outcome that holds one), so the two steps repeat until nothing is left. Cleanup failures of the root
    /// close are the report's secondaries, or the report when there is none. Async destroys arrive with slice D1;
    /// until then the plan runs here rather than in a shielded root closer frame.
    pub(crate) fn champ_shutdown(&mut self, ctx: *mut Ctx) -> Result<Option<Word>, Error> {
        let mut cleanup = 0;
        loop {
            if !self.shutdown_drain(ctx) {
                return Ok(None);
            }
            let mut outcome = Packet::default();
            for task in self.spawned.clone() {
                if !self.champ_retained(task) {
                    continue;
                }
                let record = std::mem::take(&mut self.champ_cache_mut(task)?.packet);
                self.champ_cache_mut(task)?.flags |= RELEASED;
                outcome.append(packet(self, record)?);
                consume(self, record);
            }
            if outcome.is_empty() {
                break;
            }
            // The packets' own entries settle first, each cache for its own owner (draft 3 §6.3).
            let services = std::mem::take(&mut outcome.services);
            let mut plan = Vec::new();
            let mut released = Packet::default();
            self.champ_plan_services(&services, None, &mut plan, &mut released)?;
            self.champ_apply_services(plan)?;
            outcome.append(released);
            let roots = self.roots.len();
            let mut slot = PacketSlot { record: store(self, Data::Packet(outcome)) };
            self.roots.push((slot.record, Repr::Ref));
            if cleanup != 0 {
                self.roots.push((cleanup, Repr::Ref));
            }
            let closed = super::cleanup::close_packet(ctx, &mut slot, 0);
            self.roots.truncate(roots);
            match closed {
                Ok(0) => {}
                Ok(failure) if cleanup == 0 => cleanup = failure,
                Ok(failure) => {
                    self.heap.payload_mut::<runtime::Failure>(cleanup).expect("failure").secondary.push(failure);
                }
                Err(_) if unsafe { (*ctx).status } != OK => return Ok(None),
                Err(failure) => return Err(failure),
            }
        }
        let report = self.shutdown_report();
        self.spawned.clear();
        Ok(match (report, cleanup) {
            (Some(report), 0) => Some(report),
            (Some(report), cleanup) => {
                self.heap.payload_mut::<runtime::Failure>(report).expect("failure").secondary.push(cleanup);
                Some(report)
            }
            (None, 0) => None,
            (None, cleanup) => Some(cleanup),
        })
    }

    /// KL-1 (draft 3 §7.2, #699): a detached Future that succeeded keeps its registry record, and with it its
    /// outcome packet, until the root closer closes the packet at shutdown. An empty packet needs nothing.
    pub(crate) fn champ_retained(&self, task: Word) -> bool {
        if get(task, tasks::FLAGS) & (CHAMP | DETACHED) != CHAMP | DETACHED || get(task, tasks::STATE) != tasks::DONE {
            return false;
        }
        self.champ_cache(task).is_ok_and(|cache| cache.flags & RELEASED == 0 && cache.packet != 0
            && packet(self, cache.packet).is_ok_and(|outcome| !outcome.is_empty()))
    }

    /// Settle-time view of a cache duty, with no change (draft 3 §6.2; ABI D24.2).
    pub(super) fn champ_cache_settlement(&self, duty: Word, closing: Option<OwnerId>) -> Result<CacheSettle, Error> {
        let packet_slot = self.champ_body(duty)?.packet();
        let inputs = packet_slot.map_or(0, |offset| get(duty, offset));
        let cache = self.champ_cache(duty)?;
        if Some(cache.owner) != closing {
            return Ok(CacheSettle::Nothing);
        }
        if cache.flags & RELEASED != 0 {
            return Err(error("cache duty settled twice"));
        }
        let state = get(duty, tasks::STATE);
        if state == tasks::RUNNING {
            return Ok(CacheSettle::Detach);
        }
        let outcome = if state == tasks::DONE { cache.packet } else { 0 };
        Ok(CacheSettle::Release { inputs, outcome, cold: state == tasks::COLD })
    }

    /// Settle-time view of a spawned task's duty, with no change (draft 3 §6.2; ABI D24.2). A running task
    /// detaches, and its cache duty, settled next, goes with it to the root; a settled task closes, and the
    /// registry keeps its report entry while its failure is unobserved and not Cancelled. A task whose cache
    /// another owner holds settles to nothing.
    pub(super) fn champ_task_settlement(&self, duty: Word, closing: Option<OwnerId>) -> Result<CacheSettle, Error> {
        let cache = self.champ_cache(duty)?;
        if get(duty, tasks::FLAGS) & flags::SPAWNED == 0 {
            return Err(error("a task duty of a Future that was not spawned"));
        }
        if cache.flags & RELEASED != 0 {
            return Err(error("task duty settled after its cache duty"));
        }
        if Some(cache.owner) != closing || get(duty, tasks::STATE) != tasks::RUNNING {
            return Ok(CacheSettle::Nothing);
        }
        Ok(CacheSettle::Detach)
    }

    /// The packet records a cache duty answers for: its success's outcome and, until its body starts, its inputs
    /// in continuation 0 (a running body's slot area is its own).
    fn champ_duty_packets(&self, future: Word) -> Result<[Word; 2], Error> {
        let packet_slot = self.champ_body(future)?.packet();
        let inputs = match packet_slot {
            Some(offset) if get(future, tasks::STATE) != tasks::RUNNING => get(future, offset),
            _ => 0,
        };
        Ok([self.champ_cache(future)?.packet, inputs])
    }

    /// Preflight of a cache duty taking `owner` (G1 §3 through ABI D25.1): a live duty, and every identity of
    /// its outcome and inputs can take the transition.
    pub(super) fn champ_duty_preflight(&self, future: Word, owner: OwnerId) -> Result<(), Error> {
        let cache = self.champ_cache(future)?;
        if cache.flags & RELEASED != 0 {
            return Err(error("a settled cache duty in a packet"));
        }
        if cache.owner == owner {
            return Ok(());
        }
        for record in self.champ_duty_packets(future)?.into_iter().filter(|&record| record != 0) {
            super::helpers::owner_preflight(self, &packet(self, record)?, owner)?;
        }
        Ok(())
    }

    /// The owner transition of a preflighted cache duty: the cache, its outcome identities and its inputs take
    /// `owner` (draft 3 §12: "the duties and any outcome identities already published"), and a receipt ends.
    pub(super) fn champ_duty_transition(&mut self, future: Word, owner: OwnerId) {
        let records = self.champ_duty_packets(future).expect("preflighted duty");
        let cache = self.champ_cache_mut(future).expect("preflighted duty");
        cache.flags &= !REHOMED;
        if cache.owner == owner {
            return;
        }
        cache.owner = owner;
        for record in records.into_iter().filter(|&record| record != 0) {
            let moved = super::helpers::owner_transition(self, packet(self, record).expect("preflighted"), owner);
            replace(self, record, Data::Packet(moved));
        }
    }

    /// Plan the settle of `services` in list order, each released packet's own entries first (draft 3 §§6.2–6.3,
    /// ABI D24.2, D25.4), changing nothing. `closing` is the owner whose duties settle (a cache of another owner
    /// settles to nothing); a packet close (None) settles each cache for its own owner. A spawned task that
    /// detaches takes its cache with it, so the cache entry after it settles to nothing. The released packets'
    /// identities are appended to `released`.
    pub(super) fn champ_plan_services(&self, services: &[Service], closing: Option<OwnerId>,
        plan: &mut Vec<(Service, CacheSettle)>, released: &mut Packet) -> Result<(), Error>
    {
        for &service in services {
            let detached = plan.iter().any(|(entry, settle)| {
                entry.holder == service.holder && *settle == CacheSettle::Detach
            });
            let settle = match service.kind {
                SERVICE_TASK if !detached && get(service.holder, tasks::STATE) == tasks::RUNNING => {
                    self.champ_body(service.holder)?;
                    CacheSettle::Detach
                }
                SERVICE_TASK => CacheSettle::Nothing,
                SERVICE_CACHE if detached => CacheSettle::Nothing,
                SERVICE_CACHE => {
                    let owner = closing.unwrap_or(self.champ_cache(service.holder)?.owner);
                    self.champ_cache_settlement(service.holder, Some(owner))?
                }
                _ => return Err(error("unknown service kind")),
            };
            self.champ_plan_duty(service, settle, closing, plan, released)?;
        }
        Ok(())
    }

    /// Add one decided settle to `plan`, then the entries of the packets it releases, then their identities.
    pub(super) fn champ_plan_duty(&self, service: Service, settle: CacheSettle, closing: Option<OwnerId>,
        plan: &mut Vec<(Service, CacheSettle)>, released: &mut Packet) -> Result<(), Error>
    {
        plan.push((service, settle));
        if let CacheSettle::Release { inputs, outcome, .. } = settle {
            for record in [inputs, outcome].into_iter().filter(|&record| record != 0) {
                let mut packet = packet(self, record)?;
                let nested = std::mem::take(&mut packet.services);
                self.champ_plan_services(&nested, closing, plan, released)?;
                released.append(packet);
            }
        }
        Ok(())
    }

    /// Apply a checked plan of [`Runtime::champ_plan_services`]: L, no collection, no callback.
    pub(super) fn champ_apply_services(&mut self, plan: Vec<(Service, CacheSettle)>) -> Result<(), Error> {
        for (service, settle) in plan {
            self.champ_cache_settle(service.holder, settle)?;
            if let CacheSettle::Release { inputs, outcome, .. } = settle {
                for record in [inputs, outcome].into_iter().filter(|&record| record != 0) {
                    consume(self, record);
                }
            }
        }
        Ok(())
    }

    /// Apply a checked settlement: L, no collection, no callback.
    pub(super) fn champ_cache_settle(&mut self, duty: Word, settle: CacheSettle) -> Result<(), Error> {
        let map = self.champ_body(duty)?;
        let (packet_slot, tail) = (map.packet(), map.tail);
        if settle != CacheSettle::Nothing {
            self.champ_cache_mut(duty)?.flags &= !REHOMED;
        }
        match settle {
            CacheSettle::Nothing => {}
            CacheSettle::Detach => {
                let owner = self.champ_root_owner()?;
                self.champ_cache_mut(duty)?.owner = owner;
                set(duty, tasks::FLAGS, get(duty, tasks::FLAGS) | DETACHED);
                set(duty, tail + TASK, duty);
                // The root registry: shutdown cancels it and waits for it, as Tomato does with spawned tasks. A
                // spawned Future joined it when it started (draft 3 §7.1).
                if get(duty, tasks::FLAGS) & flags::SPAWNED == 0 {
                    self.spawned.push(duty);
                }
            }
            CacheSettle::Release { inputs, outcome, cold } => {
                if let (Some(offset), true) = (packet_slot, inputs != 0) {
                    set(duty, offset, 0);
                }
                if outcome != 0 {
                    self.champ_cache_mut(duty)?.packet = 0;
                }
                if cold {
                    let failure = get(duty, tail + EMERGENCY);
                    if failure == 0 {
                        return Err(error("cold Future without its emergency failure"));
                    }
                    set(duty, tail + EMERGENCY, 0);
                    set(duty, tasks::FAILURE, failure);
                    set(duty, tasks::STATE, tasks::FAILURE_STATE);
                    set(duty, tasks::FLAGS, get(duty, tasks::FLAGS) | flags::OBSERVED);
                }
                // A body that never ran leaves its inputs in continuation 0; nothing stays after the settle.
                clear(duty, AREA, tail);
                self.champ_cache_mut(duty)?.flags |= RELEASED;
            }
        }
        Ok(())
    }

    /// A driver of a Champ Future settles with a value (draft 3 §9): its outcome packet is empty, since every Q1
    /// driver result is plain (ABI §4.3: no fresh identity with a destroy), and is committed to the cache before
    /// the waiters wake. A Tomato frame is not touched.
    pub(crate) fn champ_driver_done(&mut self, frame: Word) {
        if get(frame, tasks::FLAGS) & CHAMP == 0 {
            return;
        }
        let record = store(self, Data::Packet(Packet::default()));
        if let Ok(cache) = self.champ_cache_mut(frame) {
            cache.packet = record;
        }
    }

    #[cfg(test)]
    pub(crate) fn champ_event(&mut self, step: FutureStep, frame: Word) {
        let function = self.function_of(frame);
        self.champ.future_events.push(FutureEvent { step, frame, function });
    }
}

fn clear(frame: Word, from: u64, to: u64) {
    for offset in (from..to).step_by(8) {
        set(frame, offset, 0);
    }
}

/// Acceptance of a preflighted inputs packet by a Future's inputs holder `holder` (draft 3 §2.4): its runs keep
/// their owners; the duties of Futures among the inputs go with them and keep their owners too.
fn accept_inputs(runtime: &mut Runtime, offered: Packet, holder: HolderId) -> Packet {
    let mut moved = Packet { services: offered.services, ..Packet::default() };
    for (index, runs) in offered.journals.into_iter().enumerate() {
        for run in runs {
            let owner = run.owner;
            let mut updated = transition(runtime, Packet::from_run(run), holder, owner);
            moved.journals[index].append(&mut updated.journals[0]);
        }
    }
    moved
}

/// `fut.new` (SPB 64), class G (draft 3 §§2.4, 14.4). The frame, its cache record and its emergency failure are
/// allocated before any input is accepted: until Normal the inputs packet and the owner state stay with the
/// caller. Acceptance moves the packet's runs to the Future's inputs holder and writes continuation 0, then
/// Post is published last.
pub extern "C" fn rt_future_new_owned(ctx: *mut Ctx, inputs: *const FutureNewIn, post: *mut FutureNewPost)
    -> Outcome
{
    let frame_pointer = frame_pointer!();
    if !bridge_clear(ctx) {
        return stop(ctx, error("G helper entered with an active bridge failure"));
    }
    let runtime = unsafe { &mut *(*ctx).runtime };
    let io = unsafe { &*inputs };
    let args = if io.argc == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(io.args, io.argc as usize) } };
    let offered_record = unsafe { (*io.inputs).record };
    // Preflight: nothing is allocated, accepted or changed.
    let prepared = (|| {
        let owner = scope(runtime, unsafe { (*io.owner).record })?.owner;
        let offered = packet(runtime, offered_record)?;
        super::helpers::validate(runtime, &offered)?;
        let map = runtime.champ_frame_map(io.entry as usize)
            .ok_or_else(|| error("fut.new of a function that is not async"))?;
        if map.inputs() != args.len() {
            return Err(error("fut.new argument count"));
        }
        let holder = runtime.champ_new_holder()?;
        for run in offered.runs() {
            preflight(runtime, &Packet::from_run(run.clone()), holder, run.owner)?;
        }
        Ok((owner, offered, holder))
    })();
    let (owner, offered, holder) = match prepared {
        Ok(prepared) => prepared,
        Err(failure) => return stop(ctx, failure),
    };
    let future = match runtime.champ_future_alloc(io.entry as usize, owner) {
        Ok(future) => future,
        Err(failure) => return stop(ctx, failure),
    };
    let accepted = store(runtime, Data::Packet(Packet::default()));
    // The only collection point: the caller keeps the operands rooted until return.
    runtime.safepoint(frame_pointer, unsafe { &*ctx }, &[(future, Repr::Ref), (accepted, Repr::Ref)]);
    // Acceptance: one batch without a safepoint.
    let moved = accept_inputs(runtime, offered, holder);
    replace(runtime, accepted, Data::Packet(moved));
    consume(runtime, offered_record);
    runtime.champ_write_start(future, args, accepted);
    unsafe {
        (*io.inputs).record = 0;
        let state = (*io.owner).record;
        (*io.owner).record = 0;
        post.write(FutureNewPost { future, cache: future, state: ScopeSlot { record: state } });
    }
    NORMAL
}

/// `fut.spawn` (SPB 65), class X (draft 3 §7.1, ABI D24.1): a cold Future becomes a task of its own: SPAWNED, its
/// own TASK, registered in the root registry, and started on this stack until its first suspension. The cache
/// duty comes back unchanged, with the task duty: both are the Future's frame. A Future that is no longer cold is
/// not started again. The caller keeps the Future rooted; the registry roots it while it runs.
pub extern "C" fn rt_future_spawn(ctx: *mut Ctx, future: Word, cache: *mut CacheSlot, post: *mut SpawnPost)
    -> Outcome
{
    if !bridge_clear(ctx) {
        return stop(ctx, error("X helper entered with an active bridge failure"));
    }
    let runtime = unsafe { &mut *(*ctx).runtime };
    let checked = runtime.champ_cache(future).and_then(|record| {
        if unsafe { (*cache).record } != future {
            return Err(error("fut.spawn: the cache duty names another Future"));
        }
        if record.flags & (RELEASED | REHOMED) != 0 {
            return Err(error("fut.spawn of a settled or rehomed cache duty"));
        }
        Ok(())
    });
    if let Err(failure) = checked {
        return stop(ctx, failure);
    }
    runtime.count(|_| "spawn".into());
    if get(future, tasks::STATE) == tasks::COLD {
        set(future, tasks::FLAGS, get(future, tasks::FLAGS) | flags::SPAWNED);
        runtime.spawned.push(future);
        runtime.champ_future_start(ctx, 0, future);
        if unsafe { (*ctx).status } != OK {
            return OUTCOME_STOP;
        }
    }
    unsafe {
        (*cache).record = 0;
        post.write(SpawnPost { cache: future, task: future });
    }
    NORMAL
}

/// What the common half of `await` and `await.take` found.
enum Awaited {
    /// A settled or running Future: the caller reads its state.
    Ready,
    /// A cancellation for this frame, with its receipt.
    Cancelled(Word, u64),
    Stop(Outcome),
}

/// `await` and `await.take` up to the Future's state (draft 3 §§4.1–4.2): a deliverable cancellation first,
/// exactly as Tomato's `rt_await` (#546, #663); then a cold Future starts on this stack. Re-entered after
/// PENDING, a settle wake honours only a delivered cancellation.
fn await_entry(ctx: *mut Ctx, frame_pointer: u64, this: Word, future: Word) -> Awaited {
    if !bridge_clear(ctx) {
        return Awaited::Stop(stop(ctx, error("S helper entered with an active bridge failure")));
    }
    let runtime = unsafe { &mut *(*ctx).runtime };
    if let Err(failure) = runtime.champ_body(this).and_then(|_| runtime.champ_body(future)) {
        return Awaited::Stop(stop(ctx, failure));
    }
    if get(this, tasks::STATE) != tasks::RUNNING {
        return Awaited::Stop(stop(ctx, error("await from a frame that is not running")));
    }
    runtime.count(|_| "await".into());
    // A collection at every entry, the first and each re-entry: operands are rooted by the caller.
    runtime.safepoint(frame_pointer, unsafe { &*ctx }, &[]);
    if let Some((failure, receipt)) = take_cancel(runtime, this) {
        return Awaited::Cancelled(failure, receipt);
    }
    set(future, tasks::FLAGS, get(future, tasks::FLAGS) | flags::OBSERVED);
    if get(future, tasks::STATE) == tasks::COLD {
        runtime.champ_future_start(ctx, this, future);
        if unsafe { (*ctx).status } != OK {
            return Awaited::Stop(OUTCOME_STOP);
        }
    }
    Awaited::Ready
}

/// The cancellation an await of the running frame `this` raises, with its receipt, exactly as Tomato's `rt_await`
/// (#546, #663): re-entered after a settle wake, only a delivered cancellation; otherwise a deliverable one of
/// the frames on this stack or of the task above it (#670).
fn take_cancel(runtime: &mut Runtime, this: Word) -> Option<(Word, u64)> {
    let current = get(this, tasks::FLAGS);
    // A cancellation delivered to this waiting frame: Tomato's structured await kept the sole awaiter of a spawned
    // child registered until the child settled (CHILD_CANCEL); any other waiter was withdrawn, even when the child
    // it left has settled since.
    let delivered = if current & CHILD_CANCEL != 0 { RECEIPT_CHILD_CANCELLED } else { RECEIPT_WITHDRAWN };
    if current & flags::AWAIT_SUSPENDED != 0 {
        set(this, tasks::FLAGS, current & !(flags::AWAIT_SUSPENDED | tasks::DELIVER_NOW | CHILD_CANCEL));
        (current & tasks::DELIVER_NOW != 0).then(|| (runtime.cancelled_failure(), delivered))
    } else {
        let receipt = if current & tasks::DELIVER_NOW != 0 { delivered } else { RECEIPT_NOT_STARTED };
        set(this, tasks::FLAGS, current & !CHILD_CANCEL);
        runtime.champ_deliverable_cancel().map(|failure| (failure, receipt))
    }
}

/// A running Future: register `this` as its waiter and suspend (draft 3 §4.1).
fn await_pending(ctx: *mut Ctx, frame_pointer: u64, this: Word, future: Word) -> Outcome {
    let runtime = unsafe { &mut *(*ctx).runtime };
    runtime.champ_add_waiter(future, this);
    set(this, tasks::FLAGS, get(this, tasks::FLAGS) | flags::AWAIT_SUSPENDED);
    runtime.safepoint(frame_pointer, unsafe { &*ctx }, &[]);
    OUTCOME_PENDING
}

/// `await` (SPB 66), class S (draft 3 §4.1): a settled Future gives its value (deps take no words) or its cached
/// failure; a running one registers `this` and returns PENDING. The outcome packet stays in the cache.
pub extern "C" fn rt_await_view(ctx: *mut Ctx, this: Word, future: Word, post: *mut AwaitPost) -> Outcome {
    let frame_pointer = frame_pointer!();
    match await_entry(ctx, frame_pointer, this, future) {
        Awaited::Ready => {}
        Awaited::Cancelled(failure, receipt) => return unwind(ctx, post, failure, future, receipt),
        Awaited::Stop(outcome) => return outcome,
    }
    match get(future, tasks::STATE) {
        tasks::DONE => {
            unsafe { post.write(AwaitPost { value: get(future, tasks::RESULT), task: 0, receipt: 0 }) };
            NORMAL
        }
        tasks::FAILURE_STATE => unwind(ctx, post, get(future, tasks::FAILURE), future, RECEIPT_SETTLED),
        tasks::RUNNING => await_pending(ctx, frame_pointer, this, future),
        // B91 excludes a shared await of a taken cache.
        _ => stop(ctx, error("await of a consumed Future")),
    }
}

/// `await.take` (SPB 67), class S (draft 3 §4.2): `await` that, on success, consumes the cache. The outcome
/// packet goes to Post in one transition to the boundary owner `boundary` under a new holder, as an unbound
/// admission (provenance `temporary-await-result`), its Futures' duties with it; the Future becomes CONSUMED with
/// RESULT, FAILURE and the cache's packet cleared. A failure or a cancellation leaves the cache unconsumed. The
/// zero-word `unique-cache` capability is the loader's proof (SPB_TAKE_CAPABILITY); the runtime does not probe it.
pub extern "C" fn rt_await_take(ctx: *mut Ctx, this: Word, future: Word, boundary: u64, post: *mut AwaitTakePost)
    -> Outcome
{
    let frame_pointer = frame_pointer!();
    let unwound = |ctx: *mut Ctx, failure: Word, receipt: u64| {
        unsafe {
            (*ctx).failure = failure;
            (*ctx).status = FAILED;
            post.write(AwaitTakePost { value: failure, packet: PacketSlot::default(), task: future, receipt });
        }
        UNWIND
    };
    match await_entry(ctx, frame_pointer, this, future) {
        Awaited::Ready => {}
        Awaited::Cancelled(failure, receipt) => return unwound(ctx, failure, receipt),
        Awaited::Stop(outcome) => return outcome,
    }
    match get(future, tasks::STATE) {
        tasks::DONE => {}
        tasks::FAILURE_STATE => return unwound(ctx, get(future, tasks::FAILURE), RECEIPT_SETTLED),
        tasks::RUNNING => return await_pending(ctx, frame_pointer, this, future),
        _ => return stop(ctx, error("await.take of a consumed Future")),
    }
    let runtime = unsafe { &mut *(*ctx).runtime };
    let taken = (|| {
        let cache = runtime.champ_cache(future)?;
        if cache.flags & (RELEASED | REHOMED) != 0 {
            return Err(error("await.take of a settled or rehomed cache"));
        }
        let record = cache.packet;
        let outcome = packet(runtime, record)?;
        let holder = runtime.champ_new_holder()?;
        preflight(runtime, &outcome, holder, OwnerId(boundary))?;
        let mut moved = transition(runtime, outcome, holder, OwnerId(boundary));
        let runs = moved.journals.iter_mut().flat_map(std::mem::take).collect();
        moved.journals = [runs, Default::default(), Default::default()];
        replace(runtime, record, Data::Packet(moved));
        let cache = runtime.champ_cache_mut(future)?;
        cache.packet = 0;
        cache.flags |= CONSUMED_BY_TAKE;
        Ok(record)
    })();
    match taken {
        Ok(record) => {
            let value = get(future, tasks::RESULT);
            set(future, tasks::RESULT, 0);
            set(future, tasks::FAILURE, 0);
            set(future, tasks::STATE, tasks::CONSUMED);
            unsafe { post.write(AwaitTakePost { value, packet: PacketSlot { record }, task: 0, receipt: 0 }) };
            NORMAL
        }
        Err(failure) => stop(ctx, failure),
    }
}

fn unwind(ctx: *mut Ctx, post: *mut AwaitPost, failure: Word, task: Word, receipt: u64) -> Outcome {
    unsafe {
        (*ctx).failure = failure;
        (*ctx).status = FAILED;
        post.write(AwaitPost { value: failure, task, receipt });
    }
    UNWIND
}

/// `await.unwind` (SPB 76), class S (draft 3 §5): consumes the task and the receipt of an await's unwind,
/// removes a registration that is still present and threads the owner state and the failure. It never returns
/// PENDING for the receipts `rt_await_view` produces (ABI OQ-3).
pub extern "C" fn rt_await_unwind(ctx: *mut Ctx, this: Word, state: *mut ScopeSlot, failure: Word,
    task: *mut Word, receipt: *mut u64, post: *mut AwaitUnwindPost) -> Outcome
{
    let frame_pointer = frame_pointer!();
    if !bridge_clear(ctx) {
        return stop(ctx, error("S helper entered with an active bridge failure"));
    }
    let runtime = unsafe { &mut *(*ctx).runtime };
    runtime.safepoint(frame_pointer, unsafe { &*ctx }, &[]);
    let child = unsafe { *task };
    let checked = (|| {
        runtime.champ_body(this)?;
        runtime.champ_body(child)?;
        scope(runtime, unsafe { (*state).record })?;
        if runtime.failure(failure).is_none() || unsafe { *receipt } > RECEIPT_CHILD_CANCELLED {
            return Err(error("await.unwind failure or receipt"));
        }
        Ok(())
    })();
    if let Err(failure) = checked {
        return stop(ctx, failure);
    }
    #[cfg(test)]
    {
        let function = runtime.function_of(this);
        runtime.champ.receipts.push((function, unsafe { *receipt }));
    }
    if get(this, tasks::AWAITING) == child {
        runtime.remove_waiter(child, this);
        set(this, tasks::AWAITING, UNBOUND);
    }
    unsafe {
        *task = 0;
        *receipt = 0;
        let record = (*state).record;
        (*state).record = 0;
        post.write(AwaitUnwindPost { state: ScopeSlot { record }, failure });
    }
    NORMAL
}

/// `fut.publish` (SPB 70), class L (draft 3 §4.4): the fields and the state first, then the wait list is detached
/// and its waiters queued in registration order. Nothing runs, nothing collects and nothing fails. The slot area,
/// the scope area and the step record are cleared.
pub extern "C" fn rt_future_publish(ctx: *mut Ctx, this: Word, inputs: *const PublishIn) -> Outcome {
    if !bridge_clear(ctx) {
        return stop(ctx, error("L helper entered with an active bridge failure"));
    }
    let runtime = unsafe { &mut *(*ctx).runtime };
    let io = unsafe { &*inputs };
    let result = (|| {
        let tail = runtime.champ_tail(this)?;
        if get(this, tasks::STATE) != tasks::RUNNING {
            return Err(error("fut.publish of a Future that is not running"));
        }
        match io.tag {
            0 => {
                let record = unsafe { (*io.packet).record };
                let outcome = packet(runtime, record)?;
                super::helpers::validate(runtime, &outcome)?;
                let cache = runtime.champ_cache(this)?;
                let foreign_duty = |service: &Service| service.kind == SERVICE_CACHE
                    && runtime.champ_cache(service.holder).is_ok_and(|duty| duty.owner != cache.owner);
                if outcome.runs().any(|run| run.holder != cache.holder || run.owner != cache.owner)
                    || outcome.services.iter().any(|service| service.target.is_some() || foreign_duty(service))
                {
                    return Err(error("an outcome packet outside its cache"));
                }
                runtime.champ_cache_mut(this)?.packet = record;
                unsafe { (*io.packet).record = 0 };
                set(this, tasks::RESULT, io.value);
                set(this, tasks::STATE, tasks::DONE);
            }
            1 => {
                if runtime.failure(io.failure).is_none() {
                    return Err(error("fut.publish of an invalid failure"));
                }
                set(this, tasks::FAILURE, io.failure);
                set(this, tasks::STATE, tasks::FAILURE_STATE);
            }
            _ => return Err(error("invalid fut.publish tag")),
        }
        #[cfg(test)]
        let queued = runtime.ready.len();
        runtime.champ_queue_waiters(this);
        #[cfg(test)]
        {
            runtime.champ_event(FutureStep::Published, this);
            let waiters = runtime.ready.iter().skip(queued).copied().collect::<Vec<_>>();
            for waiter in waiters {
                runtime.champ_event(FutureStep::Queued, waiter);
            }
        }
        clear(this, AREA, tail);
        set(this, tail + STEP, 0);
        Ok(())
    })();
    match result {
        Ok(()) => NORMAL,
        Err(failure) => stop(ctx, failure),
    }
}

/// `fut.cancel` (SPB 71), class L (draft 3 §5): a request only, and it never moves a packet. A cold Future
/// settles as Cancelled with its preallocated carrier, and its inputs stay in it until its cache duty settles;
/// a running one gets Tomato's request (`cancel_running`); a settled one is unchanged.
pub extern "C" fn rt_cancel_request(ctx: *mut Ctx, future: Word) -> Outcome {
    if !bridge_clear(ctx) {
        return stop(ctx, error("L helper entered with an active bridge failure"));
    }
    let runtime = unsafe { &mut *(*ctx).runtime };
    let tail = match runtime.champ_tail(future) {
        Ok(tail) => tail,
        Err(failure) => return stop(ctx, failure),
    };
    match get(future, tasks::STATE) {
        tasks::COLD => {
            let failure = get(future, tail + EMERGENCY);
            if failure == 0 {
                return stop(ctx, error("cold Future without its emergency failure"));
            }
            set(future, tail + EMERGENCY, 0);
            set(future, tasks::FAILURE, failure);
            set(future, tasks::STATE, tasks::FAILURE_STATE);
            set(future, tasks::FLAGS, get(future, tasks::FLAGS) | flags::OBSERVED);
        }
        tasks::RUNNING => runtime.champ_cancel_running(future),
        _ => {}
    }
    NORMAL
}

/// `fut.cancelled` (SPB 73), class L: whether the Future settled as Cancelled, as Tomato's `fut cancelled`.
pub extern "C" fn rt_future_cancelled(ctx: *mut Ctx, future: Word, post: *mut FlagPost) -> Outcome {
    let runtime = unsafe { &*(*ctx).runtime };
    if let Err(failure) = runtime.champ_body(future) {
        return stop(ctx, failure);
    }
    let cancelled = get(future, tasks::STATE) == tasks::FAILURE_STATE
        && matches!(runtime.failure(get(future, tasks::FAILURE)).map(|failure| &failure.kind),
            Some(FailureKind::Cancelled));
    unsafe { post.write(FlagPost { value: u64::from(cancelled) }) };
    NORMAL
}

/// The destination of `exit.commit ... caller` in an async body (ABI C-2): the Future's cache, whose holder and
/// owner the outcome packet takes in one transition.
pub extern "C" fn rt_machine_future_destination(ctx: *mut Ctx, frame: Word, post: *mut ExitDestination)
    -> Outcome
{
    let runtime = unsafe { &*(*ctx).runtime };
    match runtime.champ_cache(frame) {
        Ok(cache) => {
            unsafe { post.write(ExitDestination { holder: cache.holder, owner: cache.owner }) };
            NORMAL
        }
        Err(failure) => stop(ctx, failure),
    }
}

/// `invoke.defer` of an async defer entry in an async body `this` (draft 3 §11, ABI C-6), class S. A `defer await`
/// is not shielded (ruling #601 K2 in OIR-2 async ABI, Tomato parity): it is an await of a child Future started on
/// this stack as a frame of the caller's task, which `this` awaits, so a cancellation of the task reaches the
/// child's own wait, and the step raises a cancellation exactly where Tomato's await of the deferred call raises
/// it: before the child starts, or when the child has settled and `this` resumes with the request not yet
/// delivered. The child is the frame's STEP until it settles; re-entered with the same operands, the step
/// continues with it. A success is Continue (ordinal 0) and a failure or a cancellation is Unwind (ordinal 1) with
/// its carrier, captured by the caller (G1 §5.3). Replace does not exist in 0.6.0: the registration has no labels.
///
/// The first entry moves the defer's input packet into the child's continuation 0 and leaves the operand an empty
/// packet, so PENDING consumes no operand (class S, §1.1); the completion consumes it. `entry` is the function
/// index of the defer body; `args` are its continuation-0 values (arguments, then captures). A cancellation raised
/// before the start needs an empty input packet: a non-empty one has nobody to close it then (#728), so the child
/// starts and raises at its own first await.
pub extern "C" fn rt_invoke_defer_async(ctx: *mut Ctx, this: Word, entry: u64, args: *const Word, argc: u64,
    io: *mut super::CallOwn, registration: *const super::RegistrationMap, post: *mut super::DeferPost) -> Outcome
{
    let frame_pointer = frame_pointer!();
    if !bridge_clear(ctx) {
        return stop(ctx, error("S helper entered with an active bridge failure"));
    }
    let runtime = unsafe { &mut *(*ctx).runtime };
    let tail = match runtime.champ_tail(this) {
        Ok(tail) if get(this, tasks::STATE) == tasks::RUNNING => tail,
        Ok(_) => return stop(ctx, error("an async defer from a frame that is not running")),
        Err(failure) => return stop(ctx, failure),
    };
    runtime.safepoint(frame_pointer, unsafe { &*ctx }, &[]);
    let started = get(this, tail + STEP) != 0;
    let input = unsafe { (*io).input.record };
    let (empty, closable) = match packet(runtime, input) {
        Ok(offered) => (offered.is_empty(), closable_before_start(runtime, &offered)),
        Err(failure) => return stop(ctx, failure),
    };
    // The await of the deferred call checks for a cancellation first (`rt_await`); a child that still runs has
    // not woken `this`, so a re-entry finds one only after the child settled.
    let cancel = if started || closable { take_cancel(runtime, this).map(|(failure, _)| failure) } else { None };
    if let (Some(failure), false) = (cancel, started) {
        if empty {
            consume(runtime, input);
            unsafe { (*io).input.record = 0 };
            return defer_unwind(ctx, post, failure);
        }
        // The deferred call never starts (#728): its inputs close here, where its own body would have closed them,
        // with the cancellation as the primary failure, as a body that cannot start closes its continuation-0
        // inputs (`rt_body_reject`). The step unwinds with the combined failure.
        let roots = runtime.roots.len();
        runtime.roots.push((failure, Repr::Ref));
        let closed = super::cleanup::close_packet(ctx, unsafe { &mut (*io).input }, failure);
        unsafe { &mut *(*ctx).runtime }.roots.truncate(roots);
        return match closed {
            Ok(combined) => defer_unwind(ctx, post, combined),
            Err(_) if matches!(unsafe { (*ctx).status }, runtime::FATAL | EXHAUSTED) => OUTCOME_STOP,
            Err(failure) => stop(ctx, failure),
        };
    }
    if !started {
        let args = if argc == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(args, argc as usize) } };
        let prepared = (|| {
            if unsafe { (*registration).replacements } != 0 {
                return Err(error("an async defer has no replacement labels (ABI C-6)"));
            }
            let map = runtime.champ_frame_map(entry as usize)
                .ok_or_else(|| error("an async defer entry that is not an async body"))?;
            if map.inputs() != args.len() || map.packet().is_none() {
                return Err(error("async defer argument count"));
            }
            let offered = packet(runtime, input)?;
            let holder = runtime.champ_new_holder()?;
            for run in offered.runs() {
                preflight(runtime, &Packet::from_run(run.clone()), holder, run.owner)?;
            }
            super::helpers::validate(runtime, &offered)?;
            let owner = runtime.champ_root_owner()?;
            Ok((offered, holder, owner))
        })();
        let (offered, holder, owner) = match prepared {
            Ok(prepared) => prepared,
            Err(failure) => return stop(ctx, failure),
        };
        let child = match runtime.champ_future_alloc(entry as usize, owner) {
            Ok(child) => child,
            Err(failure) => return stop(ctx, failure),
        };
        let accepted = store(runtime, Data::Packet(Packet::default()));
        runtime.safepoint(frame_pointer, unsafe { &*ctx }, &[(child, Repr::Ref), (accepted, Repr::Ref)]);
        let moved = accept_inputs(runtime, offered, holder);
        replace(runtime, accepted, Data::Packet(moved));
        replace(runtime, input, Data::Packet(Packet::default()));
        runtime.champ_write_start(child, args, accepted);
        set(child, tasks::FLAGS, get(child, tasks::FLAGS) | flags::OBSERVED);
        set(this, tail + STEP, child);
        runtime.champ_future_start(ctx, this, child);
        if unsafe { (*ctx).status } != OK {
            return OUTCOME_STOP;
        }
    }
    let child = get(this, tail + STEP);
    let state = get(child, tasks::STATE);
    if state == tasks::RUNNING {
        // A delivery reaches the child's own wait (#147); `this` is woken only when the child settles.
        if cancel.is_some() {
            return stop(ctx, error("a cancellation reached an async defer step while its child runs"));
        }
        // `this` awaits the child, once, as `rt_await` registers a waiter.
        if !runtime.champ_listens(child, this) {
            runtime.champ_add_waiter(child, this);
        }
        set(this, tasks::FLAGS, get(this, tasks::FLAGS) | flags::AWAIT_SUSPENDED);
        runtime.safepoint(frame_pointer, unsafe { &*ctx }, &[]);
        return OUTCOME_PENDING;
    }
    // The child settled: its duty ends here, and the defer's input operand is consumed.
    let settled = (|| {
        let record = runtime.champ_cache(child)?.packet;
        if state == tasks::DONE && !packet(runtime, record)?.is_empty() {
            return Err(error("an async defer published an outcome packet"));
        }
        if state != tasks::DONE && state != tasks::FAILURE_STATE {
            return Err(error("an async defer child in an unexpected state"));
        }
        let input = unsafe { (*io).input.record };
        if !packet(runtime, input)?.is_empty() {
            return Err(error("an async defer's input operand changed while it was pending"));
        }
        if record != 0 {
            consume(runtime, record);
        }
        consume(runtime, input);
        let cache = runtime.champ_cache_mut(child)?;
        cache.packet = 0;
        cache.flags |= RELEASED;
        Ok(())
    })();
    if let Err(failure) = settled {
        return stop(ctx, failure);
    }
    set(this, tail + STEP, 0);
    unsafe { (*io).input.record = 0 };
    if let Some(failure) = cancel {
        return defer_unwind(ctx, post, failure);
    }
    if state == tasks::DONE {
        unsafe { post.write(super::DeferPost::default()) };
        return NORMAL;
    }
    defer_unwind(ctx, post, get(child, tasks::FAILURE))
}

/// Whether the inputs of an async defer can close in the step itself when a cancellation stops the deferred call
/// before it starts (#728): every identity has a synchronous destroy (or none) and the packet carries no service
/// entry. Otherwise the child starts and raises the cancellation at its first await, where its own code closes its
/// inputs (#745).
pub(super) fn closable_before_start(runtime: &Runtime, offered: &Packet) -> bool {
    let program = &runtime.loaded.program;
    offered.services.is_empty() && offered.words().all(|object| {
        let descriptor = heap::payload(runtime.heap.header_of(object)) as usize;
        program.objects[descriptor].destroy.is_none_or(|entry| !program.functions[entry.0 as usize].is_async)
    })
}

/// The Unwind of an async defer step with `failure`, for the caller to capture.
fn defer_unwind(ctx: *mut Ctx, post: *mut super::DeferPost, failure: Word) -> Outcome {
    unsafe {
        (*ctx).failure = failure;
        (*ctx).status = FAILED;
        post.write(super::DeferPost { value: 0, packet: PacketSlot::default(), failure });
    }
    UNWIND
}
