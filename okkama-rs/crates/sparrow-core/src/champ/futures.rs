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
/// `CacheRecord::flags`: the cache duty has been settled.
pub const RELEASED: u64 = 1;
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
        Some(Self { slots, scopes, tail, start, words: 1 + trace.len(), trace })
    }

    /// The number of language inputs of continuation 0: the deps of borrowed inputs take no words.
    pub fn inputs(&self) -> usize {
        self.start.split_last().map_or(0, |(_, inputs)| {
            inputs.iter().filter(|&&slot| !self.slots[slot as usize].reprs.is_empty()).count()
        })
    }

    /// The offset of the input packet's slot.
    pub fn packet(&self) -> u64 {
        self.slots[*self.start.last().expect("verified start packet") as usize].offset
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

/// A test-only record of the Future protocol: starts, resumes and suspensions come from test bodies,
/// publications and the queue entries they make from the runtime.
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

    /// Whether `function` of the loaded Champ program is an async body.
    pub(crate) fn champ_async_body(&self, function: usize) -> bool {
        self.champ_frame_map(function).is_some()
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

    fn champ_cache_mut(&mut self, future: Word) -> Result<&mut CacheRecord, Error> {
        self.champ_body(future)?;
        self.heap.payload_mut::<CacheRecord>(get(future, CACHE)).ok_or_else(|| error("Future without a cache"))
    }

    /// The frame whose cancellation flags govern `frame` (draft 3 §5): its TASK word, or itself.
    pub(super) fn champ_task_of(&self, frame: Word) -> Result<Word, Error> {
        let task = get(frame, self.champ_tail(frame)? + TASK);
        Ok(if task == 0 { frame } else { task })
    }

    /// The owner of everything detached to the root (draft 3 §6.2, ABI D24.2).
    fn champ_root_owner(&mut self) -> Result<OwnerId, Error> {
        if let Some(owner) = self.champ.root_owner {
            return Ok(owner);
        }
        let owner = self.champ_new_owner()?;
        self.champ.root_owner = Some(owner);
        Ok(owner)
    }

    /// Creation step 1 (draft 3 §2.4): the frame, its cache record and its emergency failure. This allocates
    /// and touches no input; the caller collects afterwards with the result rooted.
    fn champ_future_alloc(&mut self, function: usize, owner: OwnerId) -> Result<Word, Error> {
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
    /// input packet.
    fn champ_write_start(&self, frame: Word, args: &[Word], packet: Word) {
        let map = self.champ_frame_map(self.function_of(frame)).expect("Champ frame map");
        let inputs = map.start.split_last().map_or(&[][..], |(_, inputs)| inputs);
        let words = inputs.iter().filter(|&&slot| !map.slots[slot as usize].reprs.is_empty());
        for (&slot, &word) in words.zip(args) {
            set(frame, map.slots[slot as usize].offset, word);
        }
        set(frame, map.packet(), packet);
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
        self.running.push(frame);
        // SAFETY: an async body takes the context and its frame.
        let pending = unsafe { crate::jit::invoke(code, ctx, &[frame]) } != 0;
        self.running.pop();
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
        if ctx.status == OK && let Some(failure) = self.shutdown(ctx) {
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

    /// Settle-time view of a cache duty, with no change (draft 3 §6.2; ABI D24.2).
    pub(super) fn champ_cache_settlement(&self, duty: Word, closing: Option<OwnerId>) -> Result<CacheSettle, Error> {
        let packet_slot = self.champ_body(duty)?.packet();
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
        Ok(CacheSettle::Release { inputs: get(duty, packet_slot), outcome, cold: state == tasks::COLD })
    }

    /// Apply a checked settlement: L, no collection, no callback.
    pub(super) fn champ_cache_settle(&mut self, duty: Word, settle: CacheSettle) -> Result<(), Error> {
        let map = self.champ_body(duty)?;
        let (packet_slot, tail) = (map.packet(), map.tail);
        match settle {
            CacheSettle::Nothing => {}
            CacheSettle::Detach => {
                let owner = self.champ_root_owner()?;
                self.champ_cache_mut(duty)?.owner = owner;
                set(duty, tasks::FLAGS, get(duty, tasks::FLAGS) | DETACHED);
                set(duty, tail + TASK, duty);
                // The root registry: shutdown cancels it and waits for it, as Tomato does with spawned tasks.
                self.spawned.push(duty);
            }
            CacheSettle::Release { inputs, outcome, cold } => {
                if inputs != 0 {
                    set(duty, packet_slot, 0);
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

    /// The Cancelled carrier preallocated with `frame`, or a new one.
    pub(super) fn champ_take_emergency(&mut self, frame: Word) -> Result<Word, Error> {
        let tail = self.champ_tail(frame)?;
        let failure = get(frame, tail + EMERGENCY);
        if failure == 0 {
            return Ok(self.failure_new(FailureKind::Cancelled));
        }
        set(frame, tail + EMERGENCY, 0);
        Ok(failure)
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
    let mut moved = Packet::default();
    for (index, runs) in offered.journals.into_iter().enumerate() {
        for run in runs {
            let owner = run.owner;
            let mut updated = transition(runtime, Packet::from_run(run), holder, owner);
            moved.journals[index].append(&mut updated.journals[0]);
        }
    }
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

/// `await` (SPB 66), class S (draft 3 §4.1): a deliverable cancellation first, exactly as Tomato's `rt_await`
/// (#546, #663); then a cold Future starts on this stack; a settled one gives its value (deps take no words) or
/// its cached failure; a running one registers `this` and returns PENDING. Re-entered after PENDING, the step
/// continues: a settle wake honours only a delivered cancellation.
pub extern "C" fn rt_await_view(ctx: *mut Ctx, this: Word, future: Word, post: *mut AwaitPost) -> Outcome {
    let frame_pointer = frame_pointer!();
    if !bridge_clear(ctx) {
        return stop(ctx, error("S helper entered with an active bridge failure"));
    }
    let runtime = unsafe { &mut *(*ctx).runtime };
    if let Err(failure) = runtime.champ_body(this).and_then(|_| runtime.champ_body(future)) {
        return stop(ctx, failure);
    }
    if get(this, tasks::STATE) != tasks::RUNNING {
        return stop(ctx, error("await from a frame that is not running"));
    }
    runtime.count(|_| "await".into());
    // A collection at every entry, the first and each re-entry: operands are rooted by the caller.
    runtime.safepoint(frame_pointer, unsafe { &*ctx }, &[]);
    let current = get(this, tasks::FLAGS);
    // A cancellation delivered to this waiting frame: Tomato's structured await keeps a sole awaiter of a
    // spawned child registered until the child settles; any other waiter was withdrawn.
    let structured = get(future, tasks::FLAGS) & flags::SPAWNED != 0
        && matches!(get(future, tasks::STATE), tasks::DONE | tasks::FAILURE_STATE);
    let delivered = if structured { RECEIPT_CHILD_CANCELLED } else { RECEIPT_WITHDRAWN };
    let cancel = if current & flags::AWAIT_SUSPENDED != 0 {
        set(this, tasks::FLAGS, current & !(flags::AWAIT_SUSPENDED | tasks::DELIVER_NOW));
        (current & tasks::DELIVER_NOW != 0).then(|| (runtime.cancelled_failure(), delivered))
    } else {
        let receipt = if current & tasks::DELIVER_NOW != 0 { delivered } else { RECEIPT_NOT_STARTED };
        runtime.champ_deliverable_cancel().map(|failure| (failure, receipt))
    };
    if let Some((failure, receipt)) = cancel {
        return unwind(ctx, post, failure, future, receipt);
    }
    set(future, tasks::FLAGS, get(future, tasks::FLAGS) | flags::OBSERVED);
    if get(future, tasks::STATE) == tasks::COLD {
        runtime.champ_future_start(ctx, this, future);
        if unsafe { (*ctx).status } != OK {
            return OUTCOME_STOP;
        }
    }
    match get(future, tasks::STATE) {
        tasks::DONE => {
            unsafe { post.write(AwaitPost { value: get(future, tasks::RESULT), task: 0, receipt: 0 }) };
            NORMAL
        }
        tasks::FAILURE_STATE => unwind(ctx, post, get(future, tasks::FAILURE), future, RECEIPT_SETTLED),
        tasks::RUNNING => {
            runtime.champ_add_waiter(future, this);
            set(this, tasks::FLAGS, get(this, tasks::FLAGS) | flags::AWAIT_SUSPENDED);
            runtime.safepoint(frame_pointer, unsafe { &*ctx }, &[]);
            OUTCOME_PENDING
        }
        // B91 excludes a shared await of a taken cache.
        _ => stop(ctx, error("await of a consumed Future")),
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
                if outcome.runs().any(|run| run.holder != cache.holder || run.owner != cache.owner) {
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
