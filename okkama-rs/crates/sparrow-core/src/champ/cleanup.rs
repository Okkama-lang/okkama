//! One serial-ordered cleanup executor for static, journal, settled and rejection packets.
#![allow(clippy::not_unsafe_ptr_arg_deref)]
use super::helpers::{consume, data, error, packet, pending, replace, scope, stop, store, validate};
use super::records::*;
use super::{HolderId, KeyRef, OwnerId};
use crate::runtime::{self, frame_pointer, Ctx, Runtime, EXHAUSTED, FAILED, FATAL, OK};
use crate::word::{Repr, Word};
use crate::Error;

impl Runtime {
    pub fn champ_cleanup_from_packet(&mut self, input: &mut PacketSlot) -> Result<CleanupSlot, Error> {
        let remainder = packet(self, input.record)?;
        validate(self, &remainder)?;
        let record = store(self, Data::Cleanup(Cleanup { remainder, settled: true, ..Cleanup::default() }));
        consume(self, input.record);
        input.record = 0;
        Ok(CleanupSlot { record })
    }
    pub fn champ_cleanup_from_scope(&mut self, input: &mut ScopeSlot) -> Result<CleanupSlot, Error> {
        let closing = scope(self, input.record)?;
        let remainder = closing.all();
        validate(self, &remainder)?;
        let record = store(self, Data::Cleanup(Cleanup {
            owner: Some(closing.owner), remainder, settled: true, ..Cleanup::default()
        }));
        consume(self, input.record);
        input.record = 0;
        Ok(CleanupSlot { record })
    }
}

fn read_cleanup(runtime: &Runtime, record: Word) -> Result<&Cleanup, Error> {
    match data(runtime, record)? {
        Data::Cleanup(cleanup) => Ok(cleanup),
        _ => Err(error("expected cleanup")),
    }
}
/// L-only transaction: move service storage out and back without cloning its live suffix.
/// No safepoint, callback or application failure may run inside body.
fn edit<T>(runtime: &mut Runtime, record: Word,
    body: impl FnOnce(&mut Runtime, &mut Cleanup) -> Result<T, Error>) -> Result<T, Error>
{
    read_cleanup(runtime, record)?;
    let data = &mut runtime.heap.payload_mut::<Record>(record).expect("cleanup record").0;
    let Data::Cleanup(mut cleanup) = std::mem::replace(data, Data::Consumed) else { unreachable!() };
    let result = body(runtime, &mut cleanup);
    replace(runtime, record, Data::Cleanup(cleanup));
    result
}
fn ready(cleanup: &Cleanup) -> Result<(), Error> {
    if cleanup.active.is_some() || cleanup.selected.is_some() || cleanup.captured != 0 || cleanup.done {
        return Err(error("cleanup step before current/failure was consumed"));
    }
    Ok(())
}
fn stream_append(runtime: &Runtime, stream: &mut Stream, packet: Packet) {
    for run in packet.journals.into_iter().flatten() {
        if let Some(object) = run.front() {
            let serial = runtime.heap.champ_identity(object).expect("validated identity").serial;
            stream.heads.push((serial, stream.runs.len()));
            stream.runs.push(run);
        }
    }
}
fn abort(runtime: &Runtime, cleanup: &mut Cleanup) {
    if let Some(pending) = cleanup.pending.take() {
        if cleanup.planned {
            stream_append(runtime, &mut cleanup.stream, pending.reserve);
        } else {
            cleanup.remainder.append(pending.reserve);
        }
        cleanup.aborted = true;
    }
}
fn record_failure(runtime: &mut Runtime, cleanup: &mut Cleanup, failure: Word) -> Result<(), Error> {
    ready(cleanup)?;
    if runtime.failure(failure).is_none() {
        return Err(error("invalid failure carrier"));
    }
    if cleanup.failure == 0 {
        cleanup.failure = failure;
    } else {
        if cleanup.failure == failure {
            return Err(error("duplicate cleanup failure"));
        }
        runtime.failure_link(cleanup.failure, failure, true);
    }
    abort(runtime, cleanup);
    Ok(())
}
impl Runtime {
    pub fn champ_register_cleanup_plan(&mut self, plan: CleanupPlan) -> PlanId {
        self.champ.cleanup_plans.push(plan);
        self.champ.cleanup_plans.len() as u64
    }
}
fn leaf(ctx: *mut Ctx, body: impl FnOnce(&mut Runtime) -> Result<(), Error>) -> Outcome {
    if unsafe { (*ctx).status } != OK || unsafe { (*ctx).failure } != 0 {
        return stop(ctx, error("cleanup helper entered with active bridge failure"));
    }
    match body(unsafe { &mut *(*ctx).runtime }) {
        Ok(()) => NORMAL,
        Err(failure) => stop(ctx, failure),
    }
}
pub extern "C" fn rt_cleanup_begin(ctx: *mut Ctx, inputs: *const CloseInputs,
    post: *mut CleanupSlot) -> Outcome
{
    leaf(ctx, |runtime| {
        let inputs = unsafe { &*inputs };
        if inputs.scope.is_null() == inputs.packet.is_null() {
            return Err(error("close requires exactly one inventory"));
        }
        let source = if inputs.scope.is_null() {
            unsafe { (*inputs.packet).record }
        } else {
            unsafe { (*inputs.scope).record }
        };
        let (owner, remainder) = if inputs.scope.is_null() {
            (None, packet(runtime, source)?)
        } else {
            let closing = scope(runtime, source)?;
            (Some(closing.owner), closing.all())
        };
        let pending_record = if inputs.pending.is_null() { 0 } else { unsafe { (*inputs.pending).record } };
        let pending = if inputs.pending.is_null() {
            None
        } else {
            Some(pending(runtime, pending_record)?)
        };
        let mut inventory = remainder.clone();
        if let Some(pending) = &pending {
            inventory.append(pending.reserve.clone());
        }
        validate(runtime, &inventory)?;
        let mut cleanup = Cleanup {
            owner, remainder, pending, settlements: inputs.settlements, ..Cleanup::default()
        };
        if inputs.failure != 0 {
            record_failure(runtime, &mut cleanup, inputs.failure)?;
        }
        let record = store(runtime, Data::Cleanup(cleanup));
        consume(runtime, source);
        unsafe {
            if inputs.scope.is_null() {
                (*inputs.packet).record = 0;
            } else {
                (*inputs.scope).record = 0;
            }
            if !inputs.pending.is_null() {
                if pending_record != 0 {
                    consume(runtime, pending_record);
                }
                (*inputs.pending).record = 0;
            }
            post.write(CleanupSlot { record });
        }
        Ok(())
    })
}
pub extern "C" fn rt_cleanup_settle_empty(ctx: *mut Ctx, input: *mut CleanupSlot,
    post: *mut CleanupSlot) -> Outcome
{
    leaf(ctx, |runtime| {
        let record = unsafe { (*input).record };
        edit(runtime, record, |_, cleanup| {
            ready(cleanup)?;
            if cleanup.settled || cleanup.settlements != 0 || cleanup.planned {
                return Err(error("nonempty or repeated empty settlement"));
            }
            cleanup.settled = true;
            Ok(())
        })?;
        unsafe {
            (*input).record = 0;
            post.write(CleanupSlot { record });
        }
        Ok(())
    })
}
/// Protocol adapters settle their duties before selection and hand off the actual packet, even on failure.
/// No runtime defer runner or asynchronous protocol is hidden here.
pub extern "C" fn rt_cleanup_settle_packet(ctx: *mut Ctx, input: *mut CleanupSlot,
    settled: *mut PacketSlot, failure: Word, post: *mut CleanupSlot) -> Outcome
{
    leaf(ctx, |runtime| {
        let record = unsafe { (*input).record };
        let packet_record = unsafe { (*settled).record };
        let packet = packet(runtime, packet_record)?;
        edit(runtime, record, |runtime, cleanup| {
            ready(cleanup)?;
            if cleanup.settlements == 0 || cleanup.settled || cleanup.planned {
                return Err(error("unexpected protocol settlement"));
            }
            let mut all = cleanup.remainder.clone();
            all.append(packet.clone());
            if let Some(pending) = &cleanup.pending {
                all.append(pending.reserve.clone());
            }
            validate(runtime, &all)?;
            if failure != 0 && runtime.failure(failure).is_none() {
                return Err(error("invalid settlement failure"));
            }
            cleanup.remainder.append(packet);
            cleanup.settlements -= 1;
            cleanup.settled = cleanup.settlements == 0;
            if failure != 0 {
                runtime.heap.payload_mut::<runtime::Failure>(failure).expect("failure").cleanup_origin = true;
                record_failure(runtime, cleanup, failure)?;
            }
            Ok(())
        })?;
        consume(runtime, packet_record);
        unsafe {
            (*settled).record = 0;
            (*input).record = 0;
            post.write(CleanupSlot { record });
        }
        Ok(())
    })
}
/// The nonempty `cleanup.settle` (SPB 21) for one scheduled duty, class L (draft 3 §6.2, ABI D24.2): no settle
/// waits in 0.6.0. `schedule` is the runtime id of the close's schedule and `index` the duty's position in it;
/// the first duty opens the settlement and the last completes it. `this` is the settling body's frame, or 0 in a
/// synchronous owner. A cache duty releases a never-started Future's inputs and a success's outcome packet into
/// the settled run, detaches a running Future to the root, and settles a cold one as Cancelled; a cache of
/// another owner settles to nothing.
pub extern "C" fn rt_service_settle(ctx: *mut Ctx, this: Word, input: *mut CleanupSlot, schedule: u64, index: u64,
    duty: Word, post: *mut CleanupSlot) -> Outcome
{
    let _ = this;
    leaf(ctx, |runtime| {
        let count = *runtime.champ.schedules.get(schedule.wrapping_sub(1) as usize)
            .ok_or_else(|| error("unknown service schedule"))?;
        if index >= count {
            return Err(error("duty outside its schedule"));
        }
        let record = unsafe { (*input).record };
        edit(runtime, record, |runtime, cleanup| {
            ready(cleanup)?;
            if cleanup.settled || cleanup.planned {
                return Err(error("duty settle after the settlement"));
            }
            if index == 0 {
                if cleanup.settlements != 0 {
                    return Err(error("settlement opened twice"));
                }
                cleanup.settlements = count;
            }
            if cleanup.settlements != count - index {
                return Err(error("duty settled out of schedule order"));
            }
            let settle = runtime.champ_cache_settlement(duty, cleanup.owner)?;
            let mut released = Packet::default();
            let records = match settle {
                super::futures::CacheSettle::Release { inputs, outcome, .. } => [inputs, outcome],
                _ => [0, 0],
            };
            for record in records.into_iter().filter(|&record| record != 0) {
                released.append(packet(runtime, record)?);
            }
            let mut all = cleanup.remainder.clone();
            all.append(released.clone());
            if let Some(pending) = &cleanup.pending {
                all.append(pending.reserve.clone());
            }
            validate(runtime, &all)?;
            runtime.champ_cache_settle(duty, settle)?;
            for record in records.into_iter().filter(|&record| record != 0) {
                consume(runtime, record);
            }
            cleanup.remainder.append(released);
            cleanup.settlements -= 1;
            cleanup.settled = cleanup.settlements == 0;
            Ok(())
        })?;
        unsafe {
            (*input).record = 0;
            post.write(CleanupSlot { record });
        }
        Ok(())
    })
}
/// `cleanup.deliver_cancel` (SPB 77), class L (draft 3 §5): at the final exit of an async body whose close
/// declares D18 `cancel=true`. A cancellation requested of the body's task and not yet delivered becomes the
/// close's primary failure when it has none, and activates abort; the failure surfaces through `cleanup.finish`.
/// With a primary already recorded the request stays for later.
pub extern "C" fn rt_cleanup_deliver_cancel(ctx: *mut Ctx, this: Word, input: *mut CleanupSlot,
    post: *mut CleanupSlot) -> Outcome
{
    use crate::tasks::{champ_flags as flags, get, set, FLAGS};
    leaf(ctx, |runtime| {
        let task = runtime.champ_task_of(this)?;
        let record = unsafe { (*input).record };
        edit(runtime, record, |runtime, cleanup| {
            ready(cleanup)?;
            let current = get(task, FLAGS);
            let requested = current & flags::CANCEL_REQUESTED != 0 && current & flags::CANCEL_DELIVERED == 0;
            if requested && cleanup.failure == 0 {
                let failure = runtime.champ_take_emergency(this)?;
                record_failure(runtime, cleanup, failure)?;
                set(task, FLAGS, current | flags::CANCEL_DELIVERED);
            }
            Ok(())
        })?;
        unsafe {
            (*input).record = 0;
            post.write(CleanupSlot { record });
        }
        Ok(())
    })
}
/// The one explicit abort operation, for pending not consumed by begin. No phase flag.
pub extern "C" fn rt_exit_abort(ctx: *mut Ctx, input: *mut PendingSlot, state: *mut CleanupSlot,
    post: *mut CleanupSlot) -> Outcome
{
    leaf(ctx, |runtime| {
        let record = unsafe { (*state).record };
        let pending_record = unsafe { (*input).record };
        let pending = pending(runtime, pending_record)?;
        edit(runtime, record, |runtime, cleanup| {
            ready(cleanup)?;
            if cleanup.aborted || cleanup.pending.is_some() {
                return Err(error("reserve already attached or aborted"));
            }
            let mut all = cleanup.remainder.clone();
            for run in &cleanup.stream.runs {
                all.append(Packet::from_run(run.clone()));
            }
            all.append(pending.reserve.clone());
            validate(runtime, &all)?;
            cleanup.pending = Some(pending);
            abort(runtime, cleanup);
            Ok(())
        })?;
        consume(runtime, pending_record);
        unsafe {
            (*input).record = 0;
            (*state).record = 0;
            post.write(CleanupSlot { record });
        }
        Ok(())
    })
}
pub extern "C" fn rt_cleanup_plan(ctx: *mut Ctx, input: *mut CleanupSlot, plan: PlanId,
    post: *mut CleanupSlot) -> Outcome
{
    leaf(ctx, |runtime| {
        let plan = *runtime.champ.cleanup_plans.get(plan.wrapping_sub(1) as usize)
            .ok_or_else(|| error("unknown cleanup plan"))?;
        let record = unsafe { (*input).record };
        edit(runtime, record, |runtime, cleanup| {
            ready(cleanup)?;
            if !cleanup.settled || cleanup.planned || plan.reserve != (cleanup.pending.is_some() || cleanup.aborted) {
                return Err(error("cleanup plan inventory/settlement mismatch"));
            }
            let reserve_empty = cleanup.pending.as_ref().is_none_or(|pending| pending.reserve.words().next().is_none());
            if plan.direct && plan.may_fail && !reserve_empty {
                return Err(error("B2: fallible direct plan with nonempty reserve"));
            }
            stream_append(runtime, &mut cleanup.stream, std::mem::take(&mut cleanup.remainder));
            cleanup.planned = true;
            Ok(())
        })?;
        unsafe {
            (*input).record = 0;
            post.write(CleanupSlot { record });
        }
        Ok(())
    })
}
fn select(runtime: &Runtime, cleanup: &mut Cleanup) -> Option<(Word, u64, HolderId, OwnerId)> {
    let (serial, index) = cleanup.stream.heads.pop()?;
    let run = &mut cleanup.stream.runs[index];
    let object = run.pop_front().expect("active stream head");
    if let Some(next) = run.front() {
        let serial = runtime.heap.champ_identity(next).expect("validated identity").serial;
        cleanup.stream.heads.push((serial, index));
    }
    Some((object, serial, run.holder, run.owner))
}
/// Ordinals follow Low selected/done order. The key has a two-word machine representation.
pub extern "C" fn rt_cleanup_next(ctx: *mut Ctx, input: *mut CleanupSlot, post: *mut NextPost) -> Outcome {
    let mut ordinal = 0;
    let outcome = leaf(ctx, |runtime| {
        let record = unsafe { (*input).record };
        let key = edit(runtime, record, |runtime, cleanup| {
            ready(cleanup)?;
            if !cleanup.planned {
                return Err(error("next before plan"));
            }
            cleanup.selected = select(runtime, cleanup);
            if let Some((object, serial, _, _)) = cleanup.selected {
                Ok(KeyRef { object, serial })
            } else {
                cleanup.done = true;
                ordinal = 1;
                Ok(KeyRef::default())
            }
        })?;
        unsafe {
            (*input).record = 0;
            post.write(NextPost { state: CleanupSlot { record }, key });
        }
        Ok(())
    });
    if outcome == OUTCOME_STOP { outcome } else { ordinal }
}
pub extern "C" fn rt_cleanup_finish(ctx: *mut Ctx, input: *mut CleanupSlot,
    post: *mut FinishPost) -> Outcome
{
    leaf(ctx, |runtime| {
        let record = unsafe { (*input).record };
        let output = edit(runtime, record, |runtime, cleanup| {
            if !cleanup.planned || !cleanup.settled || cleanup.active.is_some()
                || cleanup.selected.is_some() || cleanup.captured != 0
                || !cleanup.stream.heads.is_empty() || cleanup.remainder.words().next().is_some()
                || (cleanup.failure != 0 && cleanup.pending.is_some())
            {
                return Err(error("finish before complete cleanup"));
            }
            let pending = cleanup.pending.take().map_or(0, |pending| store(runtime, Data::Pending(pending)));
            Ok(FinishPost { closed: 1, failure: cleanup.failure, pending: PendingSlot { record: pending } })
        })?;
        consume(runtime, record);
        unsafe {
            (*input).record = 0;
            post.write(output);
        }
        Ok(())
    })
}

/// Capture is one noncollecting publication batch. The rooted destination is written before bridge reset.
/// Catch/defer landings use the same primitive as destroy capture; trace must be recorded before landing.
pub extern "C" fn rt_failure_capture(ctx: *mut Ctx, destination: *mut Word) -> Outcome {
    unsafe {
        if (*ctx).status != FAILED || (*ctx).failure == 0 {
            return stop(ctx, error("capture without pending failure"));
        }
        destination.write((*ctx).failure);
        (*ctx).failure = crate::word::UNBOUND;
        (*ctx).status = OK;
    }
    NORMAL
}

pub extern "C" fn rt_destroy_claim(ctx: *mut Ctx, state: *mut CleanupSlot, key: KeyRef,
    expected_holder: u64, expected_owner: u64, post: *mut DestroyBorrow) -> Outcome
{
    if unsafe { (*ctx).status } != OK || unsafe { (*ctx).failure } != 0 {
        return stop(ctx, error("destroy step entered with active bridge failure"));
    }
    let runtime = unsafe { &mut *(*ctx).runtime };
    let record = unsafe { (*state).record };
    let result = (|| {
        edit(runtime, record, |runtime, cleanup| {
            if !cleanup.settled || cleanup.active.is_some() || cleanup.captured != 0 || cleanup.done {
                return Err(error("destroy already active"));
            }
            let holder = HolderId(expected_holder);
            let owner = OwnerId(expected_owner);
            runtime.champ_check_key(key, holder, owner, super::Life::Live)?;
            if let Some(selected) = cleanup.selected {
                if selected != (key.object(), key.serial(), holder, owner) {
                    return Err(error("claim differs from selected identity"));
                }
                cleanup.selected = None;
            } else if cleanup.planned {
                let (serial, index) = *cleanup.stream.heads.peek().ok_or_else(|| error("empty stream"))?;
                let run = &cleanup.stream.runs[index];
                if serial != key.serial() || run.front() != Some(key.object())
                    || run.holder != holder || run.owner != owner
                {
                    return Err(error("direct claim violates serial order"));
                }
                select(runtime, cleanup);
            } else {
                let run = cleanup.remainder.runs_mut().find(|run| run.words().any(|object| object == key.object()))
                    .ok_or_else(|| error("claim not in consumed cleanup state"))?;
                if run.holder != holder || run.owner != owner {
                    return Err(error("claim authority mismatch"));
                }
                if run.front() == Some(key.object()) {
                    run.pop_front();
                } else if run.back() == Some(key.object()) {
                    run.pop_back();
                } else {
                    for chunk in &mut run.chunks {
                        chunk.retain(|object| *object != key.object());
                    }
                    run.chunks.retain(|chunk| !chunk.is_empty());
                    run.length -= 1;
                }
            }
            runtime.champ_begin_destroy(key, holder, owner)?;
            cleanup.active = Some((key.object(), key.serial(), holder, owner));
            cleanup.invoked = false;
            Ok(())
        })?;
        Ok(DestroyBorrow {
            object: key.object(), serial: key.serial(), holder: expected_holder,
            owner: expected_owner, activation: record,
        })
    })();
    match result {
        Ok(borrow) => {
            unsafe { post.write(borrow) };
            NORMAL
        }
        Err(error) => stop(ctx, error),
    }
}
fn check_borrow<'a>(runtime: &'a Runtime, record: Word, borrow: &DestroyBorrow) -> Result<&'a Cleanup, Error> {
    let cleanup = read_cleanup(runtime, record)?;
    let expected = Some((borrow.object, borrow.serial, HolderId(borrow.holder), OwnerId(borrow.owner)));
    if borrow.activation != record || cleanup.active != expected {
        return Err(error("destroy borrow activation mismatch"));
    }
    Ok(cleanup)
}

pub extern "C" fn rt_destroy_invoke(ctx: *mut Ctx, state: *mut CleanupSlot, borrow: *const DestroyBorrow,
    entry: EntryId, post: *mut DestroyPost) -> Outcome
{
    let frame = frame_pointer!();
    let runtime = unsafe { &mut *(*ctx).runtime };
    let record = unsafe { (*state).record };
    let borrow = unsafe { &*borrow };
    let result = (|| {
        let cleanup = check_borrow(runtime, record, borrow)?;
        if cleanup.invoked || unsafe { (*ctx).status } != OK || unsafe { (*ctx).failure } != 0 {
            return Err(error("destroy invoke repeated or bridge not reset"));
        }
        let descriptor = crate::heap::payload(runtime.heap.header_of(borrow.object)) as usize;
        let target = runtime.loaded.program.objects[descriptor].destroy.ok_or_else(|| error("no destroy"))?;
        let function = &runtime.loaded.program.functions[target.0 as usize];
        if target.0 as u64 != entry || function.is_async || function.params.len() != 1 {
            return Err(error("destroy entry contract"));
        }
        let code = crate::jit::compile(runtime, target.0 as usize)?;
        edit(runtime, record, |_, cleanup| {
            cleanup.invoked = true;
            Ok(())
        })?;
        let roots = runtime.roots.len();
        runtime.roots.push((record, Repr::Ref));
        runtime.safepoint(frame, unsafe { &*ctx }, &[]);
        // The active self and remainder live in the rooted cleanup record during all callbacks.
        let saved_limit = super::machine::enter_cleanup(ctx);
        unsafe { crate::jit::invoke(code, ctx, &[borrow.object]) };
        super::machine::leave_cleanup(ctx, saved_limit);
        let runtime = unsafe { &mut *(*ctx).runtime };
        if unsafe { (*ctx).status } == FAILED {
            let failure = unsafe { (*ctx).failure };
            runtime.heap.payload_mut::<runtime::Failure>(failure).expect("bridge failure").cleanup_origin = true;
            edit(runtime, record, |_, cleanup| {
                cleanup.captured = failure;
                Ok(())
            })?;
            // The rooted record already owns the exact carrier before clearing the bridge.
            unsafe {
                (*ctx).failure = crate::word::UNBOUND;
                (*ctx).status = OK;
            }
        }
        runtime.roots.truncate(roots);
        Ok(read_cleanup(runtime, record)?.captured)
    })();
    match result {
        Ok(failure) => {
            if unsafe { (*ctx).status } != OK {
                return OUTCOME_STOP;
            }
            unsafe { post.write(DestroyPost { state: CleanupSlot::default(), failure }) };
            NORMAL
        }
        Err(error) => stop(ctx, error),
    }
}
pub extern "C" fn rt_destroy_finish(ctx: *mut Ctx, state: *mut CleanupSlot,
    borrow: *mut DestroyBorrow) -> Outcome
{
    if unsafe { (*ctx).status } != OK || unsafe { (*ctx).failure } != 0 {
        return stop(ctx, error("destroy step entered with active bridge failure"));
    }
    let runtime = unsafe { &mut *(*ctx).runtime };
    let record = unsafe { (*state).record };
    let borrow_ref = unsafe { &*borrow };
    let result = (|| {
        let cleanup = check_borrow(runtime, record, borrow_ref)?;
        if !cleanup.invoked {
            return Err(error("finish before invoke"));
        }
        runtime.champ_finish_destroy(KeyRef { object: borrow_ref.object, serial: borrow_ref.serial },
            HolderId(borrow_ref.holder), OwnerId(borrow_ref.owner))?;
        edit(runtime, record, |_, cleanup| {
            cleanup.active = None;
            cleanup.invoked = false;
            Ok(())
        })?;
        Ok::<_, Error>(())
    })();
    match result {
        Ok(()) => {
            unsafe { borrow.write(DestroyBorrow::default()) };
            NORMAL
        }
        Err(error) => stop(ctx, error),
    }
}

/// Attach the exact captured step failure once, retaining it in the rooted cleanup accumulator.
pub extern "C" fn rt_cleanup_failure(ctx: *mut Ctx, input: *mut CleanupSlot, failure: Word,
    post: *mut CleanupSlot) -> Outcome
{
    leaf(ctx, |runtime| {
        let record = unsafe { (*input).record };
        edit(runtime, record, |runtime, cleanup| {
            if failure == 0 || cleanup.active.is_some() || cleanup.selected.is_some()
                || (cleanup.captured != failure && (cleanup.planned || cleanup.captured != 0))
            {
                return Err(error("cleanup failure must consume captured carrier after Dead"));
            }
            if runtime.failure(failure).is_none() {
                return Err(error("invalid cleanup failure"));
            }
            runtime.heap.payload_mut::<runtime::Failure>(failure).expect("failure").cleanup_origin = true;
            cleanup.captured = 0;
            record_failure(runtime, cleanup, failure)
        })?;
        unsafe {
            (*input).record = 0;
            post.write(CleanupSlot { record });
        }
        Ok(())
    })
}

/// Descriptor-selected and direct calls share claim/invoke/finish; failure is data until cleanup.failure.
pub extern "C" fn rt_cleanup_destroy(ctx: *mut Ctx, state: *mut CleanupSlot, key: KeyRef,
    post: *mut DestroyPost) -> Outcome
{
    let runtime = unsafe { &mut *(*ctx).runtime };
    let result = (|| {
        let cleanup = read_cleanup(runtime, unsafe { (*state).record })?;
        let Some((object, serial, holder, owner)) = cleanup.selected else {
            return Err(error("destroy without selected key"));
        };
        if key != (KeyRef { object, serial }) {
            return Err(error("destroy selected key mismatch"));
        }
        let descriptor = crate::heap::payload(runtime.heap.header_of(object)) as usize;
        let entry = runtime.loaded.program.objects[descriptor].destroy.ok_or_else(|| error("no destroy"))?.0;
        Ok((holder, owner, entry))
    })();
    let (holder, owner, entry) = match result {
        Ok(selected) => selected,
        Err(error) => return stop(ctx, error),
    };
    let mut borrow = DestroyBorrow::default();
    let mut output = DestroyPost::default();
    if rt_destroy_claim(ctx, state, key, holder.0, owner.0, &mut borrow) == OUTCOME_STOP
        || rt_destroy_invoke(ctx, state, &borrow, entry as u64, &mut output) == OUTCOME_STOP
        || rt_destroy_finish(ctx, state, &mut borrow) == OUTCOME_STOP
    {
        return OUTCOME_STOP;
    }
    let record = unsafe { (*state).record };
    output.state = CleanupSlot { record };
    unsafe {
        (*state).record = 0;
        post.write(output);
    }
    NORMAL
}

pub extern "C" fn rt_destroy_direct(ctx: *mut Ctx, state: *mut CleanupSlot, key: KeyRef,
    entry: EntryId, post: *mut DestroyPost) -> Outcome
{
    let runtime = unsafe { &mut *(*ctx).runtime };
    if !matches!(read_cleanup(runtime, unsafe { (*state).record }), Ok(cleanup) if cleanup.planned) {
        return stop(ctx, error("direct destroy before plan"));
    }
    let Some(identity) = runtime.heap.champ_identity(key.object()) else {
        return stop(ctx, error("direct identity"));
    };
    let mut borrow = DestroyBorrow::default();
    let mut output = DestroyPost::default();
    if rt_destroy_claim(ctx, state, key, identity.holder_id, identity.owner_id, &mut borrow) == OUTCOME_STOP
        || rt_destroy_invoke(ctx, state, &borrow, entry, &mut output) == OUTCOME_STOP
        || rt_destroy_finish(ctx, state, &mut borrow) == OUTCOME_STOP
    {
        return OUTCOME_STOP;
    }
    let record = unsafe { (*state).record };
    output.state = CleanupSlot { record };
    unsafe {
        (*state).record = 0;
        post.write(output);
    }
    NORMAL
}

/// Internal X runner, also used by entry rejection. Compiled Low may drive the same steps explicitly.
fn close_packet(ctx: *mut Ctx, input: &mut PacketSlot, primary: Word) -> Result<Word, Error> {
    let inputs = CloseInputs {
        scope: std::ptr::null_mut(), packet: input, pending: std::ptr::null_mut(),
        failure: primary, settlements: 0,
    };
    let mut state = CleanupSlot::default();
    if rt_cleanup_begin(ctx, &inputs, &mut state) == OUTCOME_STOP
        || rt_cleanup_settle_empty(ctx, &mut state, &mut state) == OUTCOME_STOP
    {
        return Err(error("rejection cleanup begin stopped"));
    }
    let runtime = unsafe { &mut *(*ctx).runtime };
    let plan = if let Some(plan) = runtime.champ.rejection_plan {
        plan
    } else {
        let plan = runtime.champ_register_cleanup_plan(CleanupPlan { reserve: false, direct: false, may_fail: true });
        runtime.champ.rejection_plan = Some(plan);
        plan
    };
    if rt_cleanup_plan(ctx, &mut state, plan, &mut state) == OUTCOME_STOP {
        return Err(error("rejection cleanup plan stopped"));
    }
    let roots = runtime.roots.len();
    runtime.roots.push((state.record, Repr::Ref));
    let result = (|| {
        loop {
            let mut next = NextPost::default();
            match rt_cleanup_next(ctx, &mut state, &mut next) {
                NORMAL => {}
                1 => {
                    state = next.state;
                    break;
                }
                _ => return Err(error("rejection selector stopped")),
            }
            state = next.state;
            let mut output = DestroyPost::default();
            if rt_cleanup_destroy(ctx, &mut state, next.key, &mut output) == OUTCOME_STOP {
                return Err(error("rejection destroy stopped"));
            }
            state = output.state;
            if output.failure != 0 && rt_cleanup_failure(ctx, &mut state, output.failure, &mut state) == OUTCOME_STOP {
                return Err(error("rejection failure capture stopped"));
            }
        }
        let mut output = FinishPost::default();
        if rt_cleanup_finish(ctx, &mut state, &mut output) == OUTCOME_STOP {
            return Err(error("rejection finish stopped"));
        }
        Ok(output.failure)
    })();
    unsafe { &mut *(*ctx).runtime }.roots.truncate(roots);
    result
}

pub extern "C" fn rt_entry_reject(ctx: *mut Ctx, io: *mut CallOwn) -> Outcome {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let io = unsafe { &mut *io };
    let Some(entry) = runtime.champ_entry_contract(io.entry_abi) else {
        return stop(ctx, error("entry rejection ABI"));
    };
    if entry.replacements >= OUTCOME_STOP - 1 {
        return stop(ctx, error("invalid owning outcome count"));
    }
    match reject_inputs(ctx, &mut io.input) {
        Ok(failure) => {
            unsafe {
                (*ctx).failure = failure;
                (*ctx).status = FAILED;
                io.output.write(CallPost { value: 0, packet: PacketSlot::default(), failure });
            }
            entry.replacements + 1
        }
        Err(outcome) => outcome,
    }
}

/// Close `input` with a stack overflow as the primary failure, in the reserved headroom: the combined
/// failure, or the terminal outcome when the close stopped.
fn reject_inputs(ctx: *mut Ctx, input: &mut PacketSlot) -> Result<Word, Outcome> {
    let frame = frame_pointer!();
    let runtime = unsafe { &mut *(*ctx).runtime };
    // Validate before constructing or consuming anything, including a repeated rejection.
    if let Err(error) = packet(runtime, input.record).and_then(|packet| validate(runtime, &packet)) {
        return Err(stop(ctx, error));
    }
    let roots = runtime.roots.len();
    runtime.roots.push((input.record, Repr::Ref));
    runtime::rt_stack_overflow(ctx);
    if unsafe { (*ctx).status } != FAILED {
        unsafe { &mut *(*ctx).runtime }.roots.truncate(roots);
        return Err(OUTCOME_STOP);
    }
    let primary = unsafe { (*ctx).failure };
    unsafe { &mut *(*ctx).runtime }.roots.push((primary, Repr::Ref));
    unsafe {
        (*ctx).failure = crate::word::UNBOUND;
        (*ctx).status = OK;
    }
    // Rejection may use the reserved Rust/machine headroom, never below the hard stack boundary.
    let old_limit = unsafe { (*ctx).stack_limit };
    let runtime = unsafe { &mut *(*ctx).runtime };
    let previous_floor = runtime.champ.rejection_floor;
    let hard_limit = previous_floor.unwrap_or_else(|| old_limit.saturating_sub(runtime::STACK_HEADROOM));
    runtime.champ.rejection_floor = Some(hard_limit);
    let usable_limit = hard_limit.saturating_add(crate::jit::MAX_FRAME);
    unsafe { (*ctx).stack_limit = old_limit.min(usable_limit) };
    unsafe { &mut *(*ctx).runtime }.safepoint(frame, unsafe { &*ctx }, &[]);
    let result = close_packet(ctx, input, primary);
    unsafe { (*ctx).stack_limit = old_limit };
    let runtime = unsafe { &mut *(*ctx).runtime };
    runtime.champ.rejection_floor = previous_floor;
    runtime.roots.truncate(roots);
    result.map_err(|error| {
        if matches!(unsafe { (*ctx).status }, FATAL | EXHAUSTED) {
            OUTCOME_STOP
        } else {
            stop(ctx, error)
        }
    })
}

/// A Champ async body that cannot start for want of stack (draft 3 §3), class X. Its inputs in continuation 0
/// close with the overflow as the primary failure, as `rt_entry_reject` closes an owning entry's, and the Future
/// publishes the combined failure. A resumed body holds state that only its own code can close: running out of
/// stack there stops the engine.
pub extern "C" fn rt_body_reject(ctx: *mut Ctx, frame: Word) -> Outcome {
    use crate::tasks::{get, set, RESUME};
    let runtime = unsafe { &mut *(*ctx).runtime };
    let Some(map) = runtime.champ_frame_map(runtime.function_of(frame)) else {
        return stop(ctx, error("body rejection of a frame without a map"));
    };
    if get(frame, RESUME) != 0 {
        return stop(ctx, error("stack exhausted while resuming an async body"));
    }
    let start = map.start.iter().map(|&slot| map.slots[slot as usize].offset).collect::<Vec<_>>();
    let mut input = PacketSlot { record: get(frame, map.packet()) };
    for offset in start {
        set(frame, offset, 0);
    }
    let failure = match reject_inputs(ctx, &mut input) {
        Ok(failure) => failure,
        Err(outcome) => return outcome,
    };
    let published = PublishIn { tag: 1, value: 0, packet: std::ptr::null_mut(), failure };
    super::rt_future_publish(ctx, frame, &published)
}

fn invoke_owned(ctx: *mut Ctx, entry_id: EntryId, arguments: (*const Word, u64),
    io: *mut CallOwn, replacements: u64, defer: bool, post: *mut CallPost) -> Outcome
{
    let frame = frame_pointer!();
    let (args, argc) = arguments;
    let runtime = unsafe { &mut *(*ctx).runtime };
    let contract = runtime.champ_entry_contract(entry_id);
    let Some(contract) = contract else { return stop(ctx, error("owned entry ID")) };
    if contract.replacements >= OUTCOME_STOP - 1 {
        return stop(ctx, error("invalid owning outcome count"));
    }
    if unsafe { (*io).entry_abi } != entry_id || contract.arguments.len() as u64 != argc
        || contract.replacements != replacements || unsafe { (*io).context } != contract.summary
        || unsafe { (*ctx).status } != OK || unsafe { (*ctx).failure } != 0
        || unsafe { (*io).cleanup_mode } != contract.cleanup_mode
        || contract.cleanup_mode > CLEANUP_SHIELDED || (defer && contract.cleanup_mode != CLEANUP_SHIELDED)
    {
        return stop(ctx, error("owned call contract"));
    }
    if let Err(error) = packet(runtime, unsafe { (*io).input.record }).and_then(|packet| validate(runtime, &packet)) {
        return stop(ctx, error);
    }
    let roots = runtime.roots.len();
    runtime.roots.push((unsafe { (*io).input.record }, Repr::Ref));
    if argc != 0 {
        let words = unsafe { std::slice::from_raw_parts(args, argc as usize) };
        for (&word, &repr) in words.iter().zip(&contract.arguments) {
            runtime.roots.push((word, repr));
        }
    }
    // Entry output is temporary until after the last safepoint; outer Post is never a safepoint root.
    let mut output = CallPost::default();
    let saved_output = unsafe { (*io).output };
    unsafe { (*io).output = &mut output };
    runtime.safepoint(frame, unsafe { &*ctx }, &[]);
    let outcome = (contract.invoke)(ctx, io, args, argc);
    unsafe { (*io).output = saved_output };
    let runtime = unsafe { &mut *(*ctx).runtime };
    runtime.roots.truncate(roots);
    if outcome == OUTCOME_STOP {
        if !matches!(unsafe { (*ctx).status }, FATAL | EXHAUSTED) {
            return stop(ctx, error("STOP without terminal bridge status"));
        }
        return outcome;
    }
    if outcome > replacements + 1 || unsafe { (*io).input.record } != 0 {
        return stop(ctx, error("owning entry did not consume input or returned invalid ordinal"));
    }
    if outcome == replacements + 1 {
        if unsafe { (*ctx).status } != FAILED || unsafe { (*ctx).failure } != output.failure
            || output.packet.record != 0
        {
            return stop(ctx, error("owned unwind bridge/post mismatch"));
        }
    } else {
        if unsafe { (*ctx).status } != OK || unsafe { (*ctx).failure } != 0 || output.failure != 0 {
            return stop(ctx, error("owned normal bridge/post mismatch"));
        }
        if defer && outcome == 0 {
            if output.packet.record != 0 || output.value != 0 {
                return stop(ctx, error("Continue cannot publish a replacement"));
            }
        } else if let Err(error) = packet(runtime, output.packet.record).and_then(|packet| validate(runtime, &packet)) {
            return stop(ctx, error);
        }
    }
    unsafe { post.write(output) };
    outcome
}
pub extern "C" fn rt_call_owned(ctx: *mut Ctx, entry: EntryId, args: *const Word, argc: u64,
    io: *mut CallOwn, post: *mut CallPost) -> Outcome
{
    invoke_owned(ctx, entry, (args, argc), io, 0, false, post)
}
pub extern "C" fn rt_invoke_defer(ctx: *mut Ctx, entry: EntryId, args: *const Word, argc: u64,
    io: *mut CallOwn, registration: *const RegistrationMap, post: *mut DeferPost) -> Outcome
{
    invoke_owned(ctx, entry, (args, argc), io, unsafe { (*registration).replacements }, true, post)
}

impl Runtime {
    /// Verify before mutation, then relocate function-local plan indices to runtime-local nonzero IDs.
    /// No constructor, native entry or machine code is entered during this operation.
    pub fn champ_load_cleanup_plans(&mut self, program: &okkama_sparrow::spb17::Program,
        contracts: &dyn okkama_sparrow::spb17::ExternalContracts) -> Result<Vec<Vec<PlanId>>, Error>
    {
        okkama_sparrow::spb17::verify_load(program, contracts).map_err(|failure| error(&failure.to_string()))?;
        let mut relocated = Vec::new();
        for function in &program.functions {
            let mut plans = vec![0; function.proofs.plans.len()];
            for evidence in &function.proofs.cleanup_plans {
                let close = &function.proofs.closes[evidence.close as usize];
                let envelope = &function.proofs.envelopes[close.envelope as usize];
                let plan = CleanupPlan {
                    reserve: evidence.reserve,
                    direct: !function.proofs.plans[evidence.plan as usize].merge,
                    may_fail: envelope.failures != 0,
                };
                plans[evidence.plan as usize] = self.champ_register_cleanup_plan(plan);
            }
            relocated.push(plans);
        }
        Ok(relocated)
    }
}
