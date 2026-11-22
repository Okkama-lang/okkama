//! Stackless async: the heap frame of an async call is also its Future.
//!
//! Frame words after the header: state, result, failure, waiter, awaiting,
//! resume point, flags, then every register of the function and an optional
//! driver slot; in programs with resources, then the outcome and input
//! ownership regions (OWNED, INPUTS), retained copies of the arguments
//! (registers can be reused) and the two words of the inline identity. An async body is
//! compiled as `(ctx, frame) -> 0 | 1` (completed or pending): it starts at
//! resume point 0 with its parameters in the frame, and a suspended `await` at
//! `pc` re-enters at `pc + 1` with every register reloaded. Whoever runs a body
//! (an `await` of a cold Future, `spawn`, the scheduler) turns a failure that
//! escaped it into the Future's failure. Semantics follow the previous engine
//! (`future.rs`, `await_future`, `take_cancellation`, `shutdown_tasks`).
//!
//! Async natives and intrinsics are frames too, run by a Rust driver
//! (`drivers`) instead of machine code. A driver that waits is PARKED: on a
//! resource (event, lock, timer…) or on other Futures; whatever completes the
//! wait wakes it through [`Runtime::wake`]. A cancellation reaching a parked
//! driver resumes it with DELIVER_NOW, and the driver withdraws its request.
#![allow(unsafe_code)]

use crate::heap::{self, Payload};
use crate::runtime::{Ctx, FAILED, FailureKind, OK, Runtime};
use crate::word::{self, Repr, UNBOUND, Word};

pub const COLD: u64 = 0;
pub const RUNNING: u64 = 1;
pub const DONE: u64 = 2;
pub const FAILURE_STATE: u64 = 3;
/// A unique await has consumed the cache; it cannot publish another view.
pub const CONSUMED: u64 = 4;

pub const STATE: u64 = 8;
pub const RESULT: u64 = 16;
pub const FAILURE: u64 = 24;
pub const WAITER: u64 = 32;
pub const AWAITING: u64 = 40;
pub const RESUME: u64 = 48;
pub const FLAGS: u64 = 56;
pub const REGISTERS: u64 = 64;
pub const FIXED_WORDS: usize = 7;

/// The ownership regions of a Future, behind its registers (and driver slot) in programs with resources only
/// (#932); a program without resources has neither and reads them as unbound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// The Future's outcome cache (#581).
    Owned,
    /// #587 (OIR-2 §10.1): the region of the fresh call inputs the Future owns. An async body releases what its
    /// outcome does not retain when the action completes; a driver keeps it until the Future itself closes.
    Inputs,
}

const SPAWNED: u64 = 1;
const OBSERVED: u64 = 2;
const CANCEL_REQUESTED: u64 = 4;
const CANCEL_DELIVERED: u64 = 8;
/// Resumed to receive its task's cancellation at the pending `await`.
pub const DELIVER_NOW: u64 = 16;
/// A driver waiting for a wake (see the module comment).
pub const PARKED: u64 = 32;
/// An async destroy awaited by a region close (#154): a cancellation neither
/// reaches it nor resumes its awaiter; it stays requested for the next await.
const SHIELDED: u64 = 64;
/// A driver told which of the Futures it listens to settled, in settlement
/// order: a collection (#443) or a stream's listening request (#444).
pub const INBOX: u64 = 128;
/// Suspended at an `await` of machine code with no cancellation requested
/// (#663). Cleared when the awaited Future settles after a request: then the
/// re-entered `await` raises the pending cancellation, as before. A request
/// that comes after the settlement stays pending for the next `await`.
const AWAIT_SUSPENDED: u64 = 256;
/// #587: the action of an async body has returned or failed; its outcome is reserved (in RESULT or FAILURE,
/// owned by OWNED) and the Future settles once its unretained inputs are released. A re-entry only resumes that
/// release, which may wait for async destroys.
const COMPLETING: u64 = 512;
/// With COMPLETING: the action returned a result (RESULT), not a failure.
const RESERVED: u64 = 1024;
/// One atomic channel-selection request, possibly in several receiver queues.
pub const SELECTING: u64 = 2048;

/// Several frames awaiting one Future.
#[derive(Debug, Default)]
pub struct Waiters(pub Vec<Word>);
impl Payload for Waiters {
    fn trace(&self, visit: &mut dyn FnMut(Word, Repr)) {
        for &waiter in &self.0 {
            visit(waiter, Repr::Ref);
        }
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[inline]
pub(crate) fn get(frame: Word, offset: u64) -> u64 {
    // SAFETY: `frame` is a live async frame; offsets are within its fixed words.
    unsafe { *((frame + offset) as *const u64) }
}
#[inline]
pub(crate) fn set(frame: Word, offset: u64, value: u64) {
    // SAFETY: as in `get`.
    unsafe { *((frame + offset) as *mut u64) = value }
}

impl Runtime {
    #[inline]
    fn held_at(&self, frame: Word, held: Held) -> Option<u64> {
        // Programs without resources have no regions: no lookup of the frame's function.
        if !self.resources.enabled {
            return None;
        }
        let at = self.loaded.functions[self.function_of(frame)].held_at?;
        Some(at + if held == Held::Inputs { 8 } else { 0 })
    }

    /// A Future's ownership region (`UNBOUND`: none, or a program without resources).
    #[inline]
    pub(crate) fn held(&self, frame: Word, held: Held) -> Word {
        self.held_at(frame, held).map_or(UNBOUND, |at| get(frame, at))
    }

    /// Set a Future's ownership region; only a program with resources has one.
    pub(crate) fn set_held(&self, frame: Word, held: Held, region: Word) {
        let Some(at) = self.held_at(frame, held) else {
            debug_assert_eq!(region, UNBOUND, "an ownership region in a program without resources");
            return;
        };
        set(frame, at, region);
    }

    /// The header of a frame names its function (its layout in `Layouts::frames`).
    pub(crate) fn function_of(&self, frame: Word) -> usize {
        heap::payload(self.heap.header_of(frame)) as usize
    }

    fn is_driver(&self, frame: Word) -> bool {
        self.loaded.functions[self.function_of(frame)].driver
    }

    /// Offset of the state word of a driver frame (after its registers).
    pub(crate) fn driver_slot(&self, frame: Word) -> u64 {
        REGISTERS + self.loaded.functions[self.function_of(frame)].kinds.len() as u64 * 8
    }

    /// Queue a frame whose wait is over. A driver is queued once per park.
    pub(crate) fn wake(&mut self, frame: Word) {
        if self.is_driver(frame) {
            let flags = get(frame, FLAGS);
            if flags & PARKED == 0 {
                return;
            }
            set(frame, FLAGS, flags & !PARKED);
        }
        self.ready.push_back(frame);
    }

    /// True while `frame` is a parked driver (not woken, not being cancelled).
    pub(crate) fn parked(frame: Word) -> bool {
        get(frame, FLAGS) & PARKED != 0
    }

    /// A cold Future of async `function` with its parameters.
    pub fn future_new(&mut self, function: usize, arguments: &[Word]) -> Word {
        let _cost = okkama_devtrace::span!("cleanup-time", "future_new");
        okkama_devtrace::count!("cleanup-count", "future_new");
        self.count(|r| format!("future {}", r.loaded.program.functions[function].symbol));
        let source = &self.loaded.program.functions[function];
        let registers = source.registers.len();
        // Programs with resources add the ownership regions (#932), the retained arguments (#578) and the inline
        // identity's two words (#931).
        let resources = self.loaded.functions[function].identity_at.is_some();
        let retained = FIXED_WORDS + registers + usize::from(self.loaded.functions[function].driver) + 2;
        let words = if resources { 1 + retained + source.params.len() + 2 } else { 1 + retained - 2 };
        self.heap.counted("async frame", 0, words);
        let frame = self.heap.allocate(words);
        // SAFETY: fresh storage of `words` words.
        unsafe {
            *(frame as *mut u64) = heap::header(heap::FRAME, Repr::Raw, function as u32);
            std::ptr::write_bytes((frame + 8) as *mut u64, 0, words - 1);
        }
        let kinds = &self.loaded.functions[function].kinds;
        for (register, kind) in kinds.iter().enumerate() {
            if matches!(kind, crate::program::Kind::Float | crate::program::Kind::Raw | crate::program::Kind::Byte) {
                set(frame, REGISTERS + register as u64 * 8, word::UNBOUND_RAW);
            }
        }
        for (index, (param, &argument)) in source.params.iter().zip(arguments).enumerate() {
            set(frame, REGISTERS + u64::from(param.0) * 8, argument);
            if resources {
                set(frame, (1 + retained + index) as u64 * 8, argument);
            }
        }
        self.future_created(frame);
        self.own_inputs(frame, arguments);
        frame
    }

    fn add_waiter(&mut self, future: Word, waiter: Word) {
        self.listen(future, waiter);
        set(waiter, AWAITING, future);
    }

    /// Add `waiter` to the frames woken when `future` settles.
    pub(crate) fn listen(&mut self, future: Word, waiter: Word) {
        let current = get(future, WAITER);
        if current == UNBOUND {
            set(future, WAITER, waiter);
        } else if heap::kind(self.heap.header_of(current)) == heap::FRAME {
            let list = self.heap.allocate_payload(Box::new(Waiters(vec![current, waiter])));
            set(future, WAITER, list);
        } else {
            self.heap.payload_mut::<Waiters>(current).expect("waiters").0.push(waiter);
        }
    }

    pub(crate) fn remove_waiter(&mut self, future: Word, waiter: Word) {
        let current = get(future, WAITER);
        if current == waiter {
            set(future, WAITER, UNBOUND);
        } else if current != UNBOUND && heap::kind(self.heap.header_of(current)) == heap::PAYLOAD {
            self.heap.payload_mut::<Waiters>(current).expect("waiters").0.retain(|&w| w != waiter);
        }
    }

    /// Settle a Future and queue the frames awaiting it.
    pub(crate) fn settle(&mut self, frame: Word, state: u64, value: Word) {
        self.own_outcome(frame, state == DONE, value);
        self.publish(frame, state, value);
    }

    /// Settle a Future whose outcome is already owned: its waiters see it now.
    fn publish(&mut self, frame: Word, state: u64, value: Word) {
        set(frame, STATE, state);
        if state == DONE {
            set(frame, RESULT, value);
        } else {
            set(frame, FAILURE, value);
        }
        let waiters = get(frame, WAITER);
        set(frame, WAITER, UNBOUND);
        if waiters == UNBOUND {
            return;
        }
        // One waiter, the common `await`, is the frame itself: woken without
        // a vector (#440). Several are a `Waiters` list.
        if heap::kind(self.heap.header_of(waiters)) == heap::FRAME {
            self.wake_waiter(frame, waiters);
            return;
        }
        let list = std::mem::take(&mut self.heap.payload_mut::<Waiters>(waiters).expect("waiters").0);
        for waiter in list {
            self.wake_waiter(frame, waiter);
        }
    }

    /// Wake `waiter`, which waited for the settled `frame`.
    fn wake_waiter(&mut self, frame: Word, waiter: Word) {
        if get(waiter, AWAITING) == frame {
            set(waiter, AWAITING, UNBOUND);
            let flags = get(waiter, FLAGS);
            if flags & CANCEL_REQUESTED != 0 {
                set(waiter, FLAGS, flags & !AWAIT_SUSPENDED);
            }
        } else if get(waiter, FLAGS) & INBOX != 0 && !crate::drivers::settled(self, waiter, frame) {
            // A collection or a stream only listens, so a plain `await`
            // skips this; a stream serves its requests itself.
            return;
        }
        self.wake(waiter);
    }

    /// A Future that is already settled (for drivers' internal races).
    pub(crate) fn settled_new(&mut self, function: usize, state: u64, value: Word) -> Word {
        let future = self.future_new(function, &[]);
        self.settle(future, state, value);
        future
    }

    pub(crate) fn cancelled_failure(&mut self) -> Word {
        self.failure_new(FailureKind::Cancelled)
    }

    /// Run (start or resume) the body of a RUNNING frame on this stack.
    /// Returns false while it is pending; a failure that escaped the body
    /// settles the Future. A fatal engine stop is left in the context.
    pub fn run_body(&mut self, ctx: *mut Ctx, frame: Word) -> bool {
        let _cost = okkama_devtrace::span!("cleanup-time", "run_body");
        okkama_devtrace::count!("cleanup-count", "run_body");
        let function = self.function_of(frame);
        if self.loaded.functions[function].driver {
            self.running.push(frame);
            let pending = crate::drivers::drive(self, ctx, frame);
            self.running.pop();
            return !pending;
        }
        self.running.push(frame);
        if get(frame, FLAGS) & COMPLETING == 0 {
            // A compiled body's address is in the invocation table: `compile`'s frame is not worth a call (#932).
            let compiled = self.invocations[function].code;
            let code = match if compiled != 0 { Ok(compiled) } else { crate::jit::compile(self, function) } {
                Ok(code) => code,
                Err(error) => {
                    self.running.pop();
                    crate::runtime::fatal(unsafe { &mut *ctx }, self, error.0);
                    return true;
                }
            };
            // SAFETY: async bodies take their frame.
            let pending = unsafe { crate::jit::invoke(code, ctx, &[frame]) } != 0;
            let status = unsafe { (*ctx).status };
            if status == FAILED {
                let failure = unsafe {
                    (*ctx).status = OK;
                    std::mem::replace(&mut (*ctx).failure, UNBOUND)
                };
                self.complete_action(frame, FAILURE_STATE, failure);
            } else if status != OK || pending {
                self.running.pop();
                return status != OK;
            }
        }
        // The action is over: release its unretained inputs, then publish (#587). Without inputs it is
        // published already.
        if get(frame, STATE) != RUNNING {
            self.running.pop();
            return true;
        }
        self.finish_inputs(ctx, frame);
        self.running.pop();
        (unsafe { (*ctx).status } != OK) || get(frame, STATE) != RUNNING
    }

    /// #587: the action of an async body is over with `value` (a result or a failure). Its outcome is owned now,
    /// with the inputs it retains; the Future publishes it after `finish_inputs`.
    pub(crate) fn complete_action(&mut self, frame: Word, state: u64, value: Word) {
        // No inputs to release: "publish after the unretained inputs are released" holds at once (#932).
        if self.held(frame, Held::Inputs) == UNBOUND {
            self.settle(frame, state, value);
            return;
        }
        set(frame, if state == DONE { RESULT } else { FAILURE }, value);
        set(frame, FLAGS, get(frame, FLAGS) | COMPLETING | if state == DONE { RESERVED } else { 0 });
        self.own_outcome(frame, state == DONE, value);
    }

    /// #587: release the inputs a completed action does not retain, in reverse creation order, then publish. An
    /// async destroy suspends the frame; its re-entry resumes here. A destroy failure aborts a reserved result:
    /// that result's own resources are released too and the failure is published (a failed action keeps its
    /// failure, with the destroy failures as secondaries).
    fn finish_inputs(&mut self, ctx: *mut Ctx, frame: Word) {
        let status = |ctx: *mut Ctx| unsafe { (*ctx).status };
        let inputs = self.held(frame, Held::Inputs);
        if inputs != UNBOUND {
            let failure = get(frame, FAILURE);
            if rt_region_settle(ctx, frame, inputs, UNBOUND, failure) == AWAIT_PENDING {
                return;
            }
            match status(ctx) {
                OK if failure != UNBOUND => set(frame, FAILURE, unsafe { (*ctx).scratch }),
                OK => {}
                FAILED => {
                    let cleanup = unsafe {
                        (*ctx).status = OK;
                        std::mem::replace(&mut (*ctx).failure, UNBOUND)
                    };
                    set(frame, FAILURE, cleanup);
                }
                _ => return,
            }
            self.set_held(frame, Held::Inputs, UNBOUND);
        }
        let failure = get(frame, FAILURE);
        let reserved = get(frame, FLAGS) & RESERVED != 0;
        if failure == UNBOUND {
            set(frame, FLAGS, get(frame, FLAGS) & !(COMPLETING | RESERVED));
            self.publish(frame, DONE, get(frame, RESULT));
            return;
        }
        if reserved {
            // A reserved result that a destroy failure aborted: its resources go first.
            let owned = self.held(frame, Held::Owned);
            if owned != UNBOUND {
                if rt_region_settle(ctx, frame, owned, UNBOUND, failure) == AWAIT_PENDING {
                    return;
                }
                if status(ctx) != OK {
                    return;
                }
                set(frame, FAILURE, unsafe { (*ctx).scratch });
                self.set_held(frame, Held::Owned, UNBOUND);
            }
            set(frame, RESULT, UNBOUND);
            set(frame, FLAGS, get(frame, FLAGS) & !RESERVED);
            let failure = get(frame, FAILURE);
            self.own_outcome(frame, false, failure);
        }
        set(frame, FLAGS, get(frame, FLAGS) & !COMPLETING);
        let failure = get(frame, FAILURE);
        self.publish(frame, FAILURE_STATE, failure);
    }

    /// Mark a cancellation of a running task delivered and return its failure.
    fn deliverable_cancel(&mut self) -> Option<Word> {
        // The frames running on this stack, innermost first, up to the task.
        for index in (0..self.running.len()).rev() {
            let frame = self.running[index];
            let flags = get(frame, FLAGS);
            if flags & DELIVER_NOW != 0 {
                set(frame, FLAGS, flags & !DELIVER_NOW);
                return Some(self.cancelled_failure());
            }
            if flags & CANCEL_REQUESTED != 0 && flags & CANCEL_DELIVERED == 0 {
                set(frame, FLAGS, flags | CANCEL_DELIVERED);
                return Some(self.cancelled_failure());
            }
            if flags & (SPAWNED | SHIELDED) != 0 {
                return None;
            }
        }
        // The stack starts at a nested frame the scheduler resumed (#670):
        // its task is suspended above it, awaiting it, and a cancellation
        // requested there is delivered here.
        let &outermost = self.running.first()?;
        let task = self.requested_above(outermost)?;
        set(task, FLAGS, get(task, FLAGS) | CANCEL_DELIVERED);
        Some(self.cancelled_failure())
    }

    /// The frame with an undelivered cancellation request among the frames
    /// awaiting `frame`, and the frames awaiting those, up to their task
    /// (#670). Frames that only listen to `frame` do not count. The walk is
    /// depth first in waiter order with an explicit stack, so a deep chain of
    /// nested frames never grows the process stack.
    fn requested_above(&self, frame: Word) -> Option<Word> {
        let mut pending = Vec::new();
        self.push_waiters(frame, &mut pending);
        while let Some((awaited, waiter)) = pending.pop() {
            // A frame that only listens to `awaited` does not await it.
            if get(waiter, AWAITING) != awaited {
                continue;
            }
            let flags = get(waiter, FLAGS);
            if flags & CANCEL_REQUESTED != 0 && flags & CANCEL_DELIVERED == 0 {
                return Some(waiter);
            }
            if flags & (SPAWNED | SHIELDED) != 0 {
                continue;
            }
            self.push_waiters(waiter, &mut pending);
        }
        None
    }

    /// Push the waiters of `frame`, each paired with `frame`, so that the
    /// first waiter is popped first.
    fn push_waiters(&self, frame: Word, pending: &mut Vec<(Word, Word)>) {
        let waiters = get(frame, WAITER);
        if waiters == UNBOUND {
            return;
        }
        if heap::kind(self.heap.header_of(waiters)) == heap::FRAME {
            pending.push((frame, waiters));
            return;
        }
        let list = &self.heap.payload::<Waiters>(waiters).expect("waiters").0;
        pending.extend(list.iter().rev().map(|&waiter| (frame, waiter)));
    }

    /// Deliver a cancellation to a suspended task: its innermost waiting
    /// frame resumes and its pending `await` raises Cancelled.
    fn deliver_cancel(&mut self, task: Word) {
        let mut frame = task;
        loop {
            let awaited = get(frame, AWAITING);
            if awaited == UNBOUND {
                break;
            }
            let header = self.heap.header_of(awaited);
            if heap::kind(header) != heap::FRAME || get(awaited, STATE) != RUNNING {
                break;
            }
            if get(awaited, FLAGS) & SHIELDED != 0 {
                return;
            }
            if get(awaited, FLAGS) & SPAWNED == 0 {
                frame = awaited;
                continue;
            }
            // Structured await (#147): a spawned task that no other frame
            // awaits is cancelled with its awaiter, which keeps waiting and
            // raises Cancelled once that task has settled.
            if !self.is_driver(frame) && self.sole_awaiter(awaited, frame) {
                set(task, FLAGS, get(task, FLAGS) | CANCEL_DELIVERED);
                set(frame, FLAGS, get(frame, FLAGS) | DELIVER_NOW);
                // Champ's await receipt (draft 3 §4.1): this waiter stays to see its own child settle.
                self.cancel_running(awaited);
                return;
            }
            break;
        }
        let awaited = get(frame, AWAITING);
        if awaited != UNBOUND {
            self.remove_waiter(awaited, frame);
            set(frame, AWAITING, UNBOUND);
        } else if !Self::parked(frame) {
            // Not waiting (or its wait already completed): the cancellation
            // stays requested for the next `await`.
            return;
        }
        set(frame, FLAGS, get(frame, FLAGS) & !PARKED);
        set(task, FLAGS, get(task, FLAGS) | CANCEL_DELIVERED);
        set(frame, FLAGS, get(frame, FLAGS) | DELIVER_NOW);
        self.ready.push_back(frame);
    }

    /// True when no frame but `frame` awaits `future` (frames that only
    /// listen to it, such as a group's, do not count).
    fn sole_awaiter(&self, future: Word, frame: Word) -> bool {
        let waiters = get(future, WAITER);
        if waiters == UNBOUND || heap::kind(self.heap.header_of(waiters)) == heap::FRAME {
            return true;
        }
        self.heap
            .payload::<Waiters>(waiters)
            .expect("waiters")
            .0
            .iter()
            .all(|&waiter| waiter == frame || get(waiter, AWAITING) != future)
    }

    /// Resume ready frames until `root` settles (or nothing can run).
    pub fn schedule(&mut self, ctx: *mut Ctx, root: Word) {
        while get(root, STATE) == RUNNING || get(root, STATE) == COLD {
            if (self.limits.interrupt.is_some() || self.limits.memory.is_some())
                && let Some(what) = self.over_budget() {
                    self.exhaust(unsafe { &mut *ctx }, what);
                    return;
                }
            self.poll_io();
            let Some(frame) = self.ready.pop_front() else {
                if self.wait_external() {
                    continue;
                }
                // A runtime failure of the run, worded as the IR engine and
                // the shared diagnostic contract requires (#342).
                let ctx = unsafe { &mut *ctx };
                ctx.failure = self.failure_new(FailureKind::Program("async main cannot complete: no task can make progress".into()));
                ctx.status = FAILED;
                return;
            };
            if get(frame, STATE) != RUNNING {
                continue;
            }
            self.run_body(ctx, frame);
            if unsafe { (*ctx).status } != OK {
                return;
            }
        }
    }

    /// Await `future` from driver `frame`: its result, its failure, or None
    /// when `frame` now waits for it (parked; woken when it settles).
    pub(crate) fn driver_await(&mut self, ctx: *mut Ctx, frame: Word, future: Word) -> Option<Result<Word, Word>> {
        set(future, FLAGS, get(future, FLAGS) | OBSERVED);
        if get(future, STATE) == COLD {
            set(future, STATE, RUNNING);
            self.run_body(ctx, future);
            if unsafe { (*ctx).status } != OK {
                return None;
            }
        }
        match get(future, STATE) {
            DONE => Some(Ok(get(future, RESULT))),
            FAILURE_STATE => Some(Err(get(future, FAILURE))),
            _ => {
                self.add_waiter(future, frame);
                None
            }
        }
    }

    /// Start `future` as a spawned task (no effect unless it is cold).
    pub(crate) fn spawn(&mut self, ctx: *mut Ctx, future: Word) {
        if get(future, STATE) == COLD {
            set(future, FLAGS, get(future, FLAGS) | SPAWNED);
            set(future, STATE, RUNNING);
            self.spawned.push(future);
            self.run_body(ctx, future);
        }
    }

    /// Drop finished spawned tasks that shutdown no longer needs (#503): it
    /// cancels running ones and reports unobserved failures. A long-running
    /// program (a server) otherwise kept every task it ever spawned, with
    /// everything its frame refers to, as a root of every collection.
    pub(crate) fn prune_spawned(&mut self) {
        let mut kept = Vec::with_capacity(self.spawned.len());
        for &task in &self.spawned {
            let state = get(task, STATE);
            let needed = match state {
                COLD | RUNNING => true,
                FAILURE_STATE => {
                    let reported = get(task, FLAGS) & OBSERVED == 0
                        && !matches!(self.failure(get(task, FAILURE)).map(|f| &f.kind), Some(FailureKind::Cancelled));
                    // A detached Champ driver that failed keeps its record while its INPUTS wait for the root closer.
                    reported
                }
                // Champ KL-1 (draft 3 §7.2, #699): a detached Future keeps its record while its outcome packet
                // waits for the root closer at shutdown.
                _ => false,
            };
            if needed {
                kept.push(task);
            }
        }
        self.spawned = kept;
    }

    pub(crate) fn observe(future: Word) {
        set(future, FLAGS, get(future, FLAGS) | OBSERVED);
    }

    /// After the root: cancel running spawned tasks and wait for them; an
    /// unobserved failure of a spawned task (not a cancellation) is reported.
    pub fn shutdown(&mut self, ctx: *mut Ctx) -> Option<Word> {
        if !self.shutdown_drain(ctx) {
            return None;
        }
        let failure = self.shutdown_report();
        self.spawned.clear();
        failure
    }

    /// Shutdown's first step: cancel every running spawned task and run the
    /// scheduler until all have settled. False when the run stopped.
    pub(crate) fn shutdown_drain(&mut self, ctx: *mut Ctx) -> bool {
        loop {
            let mut pending = None;
            for index in 0..self.spawned.len() {
                let task = self.spawned[index];
                self.cancel(ctx, task);
                if pending.is_none() && get(task, STATE) == RUNNING {
                    pending = Some(task);
                }
            }
            match pending {
                Some(task) => {
                    self.schedule(ctx, task);
                    if unsafe { (*ctx).status } != OK {
                        return false;
                    }
                }
                None => return true,
            }
        }
    }

    /// The first unobserved failure of a spawned task that is not a
    /// cancellation, in spawn order.
    pub(crate) fn shutdown_report(&self) -> Option<Word> {
        self.spawned.iter().copied().find_map(|task| {
            let failed = get(task, STATE) == FAILURE_STATE && get(task, FLAGS) & OBSERVED == 0;
            let failure = get(task, FAILURE);
            (failed && !matches!(self.failure(failure).map(|f| &f.kind), Some(FailureKind::Cancelled)))
                .then_some(failure)
        })
    }

    pub(crate) fn cancel(&mut self, ctx: *mut Ctx, future: Word) {
        let _ = ctx;
        // A Champ Future settles by its own rows: a cold one with its emergency failure (draft 3 §4.3).
        match get(future, STATE) {
            COLD => {
                let failure = self.cancelled_failure();
                self.settle(future, FAILURE_STATE, failure);
            }
            RUNNING => self.cancel_running(future),
            _ => {}
        }
    }

    fn cancel_running(&mut self, future: Word) {
        let flags = get(future, FLAGS);
        if flags & CANCEL_REQUESTED == 0 {
            set(future, FLAGS, flags | CANCEL_REQUESTED);
            if get(future, AWAITING) != UNBOUND || Self::parked(future) {
                self.deliver_cancel(future);
            }
        }
    }
}



// ---- helpers called from machine code ----

unsafe fn parts<'a>(ctx: *mut Ctx) -> (&'a mut Ctx, &'a mut Runtime) {
    let ctx = unsafe { &mut *ctx };
    let runtime = unsafe { &mut *ctx.runtime };
    (ctx, runtime)
}

/// A cold Future of async `function` with `count` argument words.
#[inline(never)]
pub extern "C" fn rt_future_new(ctx: *mut Ctx, function: u64, arguments: *const Word, count: u64) -> Word {
    okkama_devtrace::count!("runtime", "rt_future_new");
    let frame_pointer = crate::runtime::frame_pointer!();
    let (ctx, runtime) = unsafe { parts(ctx) };
    let arguments = unsafe { std::slice::from_raw_parts(arguments, count as usize) };
    let future = runtime.future_new(function as usize, arguments);
    runtime.safepoint(frame_pointer, ctx, &[(future, Repr::Ref)]);
    future
}

pub const AWAIT_READY: u64 = 0;
pub const AWAIT_PENDING: u64 = 2;

/// `await future` from frame `this`: ready (value in `scratch`), failed
/// (status set), or pending (`this` now waits for it).
#[inline(never)]
pub extern "C" fn rt_await(ctx_ptr: *mut Ctx, this: Word, future: Word) -> u64 {
    okkama_devtrace::count!("runtime", "rt_await");
    await_future(ctx_ptr, this, future, false)
}

/// The await protocol; `take` consumes a delivered result (`rt_await_take`) in the same call (#932).
#[inline(never)]
fn await_future(ctx_ptr: *mut Ctx, this: Word, future: Word, take: bool) -> u64 {
    let (ctx, runtime) = unsafe { parts(ctx_ptr) };
    runtime.count(|_| "await".into());
    // Re-entry after the awaited Future settled first (#663): only a
    // delivered cancellation raises. A cancellation requested after the
    // settlement stays pending for the next `await`; this one takes the
    // settled outcome.
    let flags = get(this, FLAGS);
    let reentry = flags & AWAIT_SUSPENDED != 0;
    let cancel = if reentry {
        set(this, FLAGS, flags & !(AWAIT_SUSPENDED | DELIVER_NOW));
        (flags & DELIVER_NOW != 0).then(|| runtime.cancelled_failure())
    } else {
        runtime.deliverable_cancel()
    };
    if let Some(failure) = cancel {
        ctx.failure = failure;
        ctx.status = FAILED;
        return AWAIT_READY;
    }
    set(future, FLAGS, get(future, FLAGS) | OBSERVED);
    if get(future, STATE) == COLD {
        set(future, STATE, RUNNING);
        runtime.run_body(ctx_ptr, future);
        let ctx = unsafe { &mut *ctx_ptr };
        if ctx.status != OK {
            return AWAIT_READY;
        }
    }
    let ctx = unsafe { &mut *ctx_ptr };
    match get(future, STATE) {
        DONE => {
            ctx.scratch = get(future, RESULT);
            if take {
                runtime.take_outcome(future);
                set(future, RESULT, UNBOUND);
                set(future, FAILURE, UNBOUND);
                set(future, STATE, CONSUMED);
            }
            AWAIT_READY
        }
        CONSUMED => {
            ctx.failure = runtime.failure_new(FailureKind::Panic("await of a consumed Future".into()));
            ctx.status = FAILED;
            AWAIT_READY
        }
        FAILURE_STATE => {
            ctx.failure = get(future, FAILURE);
            ctx.status = FAILED;
            AWAIT_READY
        }
        _ => {
            runtime.add_waiter(future, this);
            set(this, FLAGS, get(this, FLAGS) | AWAIT_SUSPENDED);
            AWAIT_PENDING
        }
    }
}

/// Same suspension/cancellation protocol as borrow-await. Consume only after
/// settlement was actually delivered; cancellation before delivery leaves the
/// cache for the Future's normal cleanup. Scratch carries the delivered value
/// until the JIT stores it; no safepoint intervenes after extraction. Failed
/// awaits leave their carrier in the cache for normal unwind cleanup.
#[inline(never)]
pub extern "C" fn rt_await_take(ctx_ptr: *mut Ctx, this: Word, future: Word) -> u64 {
    okkama_devtrace::count!("runtime", "rt_await_take");
    await_future(ctx_ptr, this, future, true)
}

/// `region.settle` / `region.settle_unwind` from frame `this` (#154): close
/// the region from its last identity, awaiting each async destroy shielded
/// from cancellation. Ready (the unwind failure in `scratch`; a leave's
/// destroy failure raised) or pending: `this` waits for the destroy, keeps it
/// in the region, and re-enters here when it settles.
#[inline(never)]
pub extern "C" fn rt_region_settle(ctx_ptr: *mut Ctx, this: Word, region: Word, keep: Word, failure: Word) -> u64 {
    let _cost = okkama_devtrace::span!("cleanup-time", "rt_region_settle");
    okkama_devtrace::count!("cleanup-count", "rt_region_settle");
    okkama_devtrace::count!("runtime", "rt_region_settle");
    let (_, runtime) = unsafe { parts(ctx_ptr) };
    let unwind = failure != UNBOUND;
    // #530: a region machine code never made holds nothing to settle.
    if region == UNBOUND {
        if unwind {
            unsafe { (*ctx_ptr).scratch = failure };
        }
        return AWAIT_READY;
    }
    let link = |runtime: &mut Runtime, primary: Option<Word>, cleanup: Word| match primary {
        Some(primary) => runtime.failure_link(primary, cleanup, true),
        None => cleanup,
    };
    let (settling, settled) = runtime.region_settling(region);
    let mut primary = settled.or(unwind.then_some(failure));
    if let Some(future) = settling {
        match get(future, STATE) {
            DONE => {}
            FAILURE_STATE => primary = Some(link(runtime, primary, get(future, FAILURE))),
            _ => {
                runtime.add_waiter(future, this);
                return AWAIT_PENDING;
            }
        }
        runtime.set_region_settling(region, None, primary);
    }
    // Normal exits keep at most one value: borrow its stack slot instead of allocating a Vec (#329).
    let payloads;
    let kept = if unwind {
        payloads = runtime.failure_payloads(failure);
        payloads.as_slice()
    } else if keep == UNBOUND {
        &[]
    } else {
        std::slice::from_ref(&keep)
    };
    loop {
        let (object, so_far) = match runtime.region_close_until(ctx_ptr, region, kept, primary, true) {
            crate::failures::Closed::Done(done) => {
                primary = done;
                break;
            }
            crate::failures::Closed::Async(object, so_far) => (object, so_far),
        };
        primary = so_far;
        let descriptor = heap::payload(runtime.heap.header_of(object)) as usize;
        let destroy = runtime.loaded.program.objects[descriptor].destroy.expect("an async destroy").0 as usize;
        let rooted = [Some(region), Some(this), primary, (keep != UNBOUND).then_some(keep), unwind.then_some(failure)];
        okkama_devtrace::count!("cleanup-count", "destroy root pushes", rooted.iter().flatten().count() + 1);
        let depth = runtime.roots.len();
        runtime.roots.extend(rooted.iter().flatten().map(|&word| (word, Repr::Ref)));
        let future = runtime.future_new(destroy, &[object]);
        set(future, FLAGS, get(future, FLAGS) | SHIELDED | OBSERVED);
        set(future, STATE, RUNNING);
        runtime.roots.push((future, Repr::Ref));
        runtime.run_body(ctx_ptr, future);
        runtime.roots.truncate(depth);
        if unsafe { (*ctx_ptr).status } != OK {
            return AWAIT_READY;
        }
        match get(future, STATE) {
            DONE => { okkama_devtrace::count!("cleanup-count", "destroy completed inline"); }
            FAILURE_STATE => primary = Some(link(runtime, primary, get(future, FAILURE))),
            _ => {
                okkama_devtrace::count!("cleanup-count", "destroy suspended");
                runtime.set_region_settling(region, Some(future), primary);
                runtime.add_waiter(future, this);
                return AWAIT_PENDING;
            }
        }
    }
    runtime.set_region_settling(region, None, None);
    let ctx = unsafe { &mut *ctx_ptr };
    if unwind {
        ctx.scratch = primary.unwrap_or(failure);
    } else if let Some(primary) = primary {
        ctx.failure = primary;
        ctx.status = FAILED;
    }
    AWAIT_READY
}

/// #329 `resource.settle` / `resource.settle_unwind`: a static resource with an async destroy leaves its scope
/// in the async frame `this`. The destroy runs shielded and observed, as in [`rt_region_settle`]; the frame
/// suspends while it runs and re-enters this call when it settles. On the failure path (`failure` bound) the
/// result failure is left in `scratch`; on the normal path a destroy failure fails the instruction.
#[inline(never)]
pub extern "C" fn rt_resource_settle(ctx_ptr: *mut Ctx, this: Word, object: Word, failure: Word) -> u64 {
    okkama_devtrace::count!("runtime", "rt_resource_settle");
    let (_, runtime) = unsafe { parts(ctx_ptr) };
    let unwind = failure != UNBOUND;
    let link = |runtime: &mut Runtime, primary: Option<Word>, cleanup: Word| match primary {
        Some(primary) => runtime.failure_link(primary, cleanup, true),
        None => cleanup,
    };
    let primary = match runtime.static_settling_take(this) {
        // Re-entered: the destroy this frame awaited has settled (or a wake came early).
        Some((future, primary)) => match get(future, STATE) {
            DONE => primary,
            FAILURE_STATE => Some(link(runtime, primary, get(future, FAILURE))),
            _ => {
                runtime.static_settling_put(this, future, primary);
                runtime.add_waiter(future, this);
                return AWAIT_PENDING;
            }
        },
        None => match runtime.resource_settle_start(ctx_ptr, object, unwind.then_some(failure)) {
            Err(primary) => primary,
            Ok((destroy, primary)) => {
                let depth = runtime.roots.len();
                let rooted = [Some(this), Some(object), primary, unwind.then_some(failure)];
                runtime.roots.extend(rooted.iter().flatten().map(|&word| (word, Repr::Ref)));
                let future = runtime.future_new(destroy, &[object]);
                set(future, FLAGS, get(future, FLAGS) | SHIELDED | OBSERVED);
                set(future, STATE, RUNNING);
                runtime.roots.push((future, Repr::Ref));
                runtime.run_body(ctx_ptr, future);
                runtime.roots.truncate(depth);
                if unsafe { (*ctx_ptr).status } != OK {
                    return AWAIT_READY;
                }
                match get(future, STATE) {
                    DONE => primary,
                    FAILURE_STATE => Some(link(runtime, primary, get(future, FAILURE))),
                    _ => {
                        runtime.static_settling_put(this, future, primary);
                        runtime.add_waiter(future, this);
                        return AWAIT_PENDING;
                    }
                }
            }
        },
    };
    let ctx = unsafe { &mut *ctx_ptr };
    if unwind {
        ctx.scratch = primary.unwrap_or(failure);
    } else if let Some(primary) = primary {
        ctx.failure = primary;
        ctx.status = FAILED;
    }
    AWAIT_READY
}

/// `return value` of an async body: its outcome is reserved; `run_body` settles the Future after the inputs
/// it does not retain are released (#587).
#[inline(never)]
pub extern "C" fn rt_complete(ctx: *mut Ctx, frame: Word, value: Word) {
    okkama_devtrace::count!("runtime", "rt_complete");
    let (_, runtime) = unsafe { parts(ctx) };
    runtime.complete_action(frame, DONE, value);
}

/// `spawn future`: start a cold Future now, on this stack.
#[inline(never)]
pub extern "C" fn rt_spawn(ctx_ptr: *mut Ctx, future: Word) -> Word {
    okkama_devtrace::count!("runtime", "rt_spawn");
    let (_, runtime) = unsafe { parts(ctx_ptr) };
    runtime.spawn(ctx_ptr, future);
    future
}

/// `fut` operations: done, cancelled (flags) and cancel.
#[inline(never)]
pub extern "C" fn rt_fut(ctx_ptr: *mut Ctx, op: u64, future: Word) -> Word {
    okkama_devtrace::count!("runtime", "rt_fut");
    let (_, runtime) = unsafe { parts(ctx_ptr) };
    let state = get(future, STATE);
    match op {
        0 => {
            runtime.cancel(ctx_ptr, future);
            UNBOUND
        }
        1 => Word::from(state == DONE || state == FAILURE_STATE),
        _ => Word::from(
            state == FAILURE_STATE
                && matches!(runtime.failure(get(future, FAILURE)).map(|f| &f.kind), Some(FailureKind::Cancelled)),
        ),
    }
}

#[cfg(test)]
#[path = "tasks_tests.rs"]
mod tests;
