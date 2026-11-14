//! Shared S2 claim/invoke/finish and packet rejection close. S3 adds the full pending/suffix executor.
#![allow(clippy::not_unsafe_ptr_arg_deref)]
use super::helpers::{cleanup as read_cleanup, consume, error, packet, replace, scope, stop, store, validate};
use super::records::*;
use super::{HolderId, KeyRef, OwnerId};
use crate::runtime::{self, frame_pointer, Ctx, Runtime, EXHAUSTED, FAILED, FATAL, OK};
use crate::word::{Repr, Word};
use crate::Error;

impl Runtime {
    pub fn champ_cleanup_from_packet(&mut self, input: &mut PacketSlot) -> Result<CleanupSlot, Error> {
        let remainder = packet(self, input.record)?;
        validate(self, &remainder)?;
        let record = store(self, Data::Cleanup(Cleanup { remainder, ..Cleanup::default() }));
        consume(self, input.record);
        input.record = 0;
        Ok(CleanupSlot { record })
    }
    pub fn champ_cleanup_from_scope(&mut self, input: &mut ScopeSlot) -> Result<CleanupSlot, Error> {
        let remainder = scope(self, input.record)?.all();
        validate(self, &remainder)?;
        let record = store(self, Data::Cleanup(Cleanup { remainder, ..Cleanup::default() }));
        consume(self, input.record);
        input.record = 0;
        Ok(CleanupSlot { record })
    }
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
        let mut cleanup = read_cleanup(runtime, record)?;
        if cleanup.active.is_some() || cleanup.captured != 0 {
            return Err(error("destroy already active"));
        }
        let mut found = 0;
        for run in cleanup.remainder.runs_mut() {
            for chunk in &mut run.chunks {
                if chunk.contains(&key.object()) {
                    if run.holder.0 != expected_holder || run.owner.0 != expected_owner {
                        return Err(error("claim authority mismatch"));
                    }
                    found += 1;
                    chunk.retain(|object| *object != key.object());
                }
            }
            run.chunks.retain(|chunk| !chunk.is_empty());
        }
        if found != 1 {
            return Err(error("claim not in consumed cleanup state"));
        }
        let holder = HolderId(expected_holder);
        let owner = OwnerId(expected_owner);
        runtime.champ_begin_destroy(key, holder, owner)?;
        cleanup.active = Some((key.object(), key.serial(), holder, owner));
        cleanup.invoked = false;
        replace(runtime, record, Data::Cleanup(cleanup));
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
fn check_borrow(runtime: &Runtime, record: Word, borrow: &DestroyBorrow) -> Result<Cleanup, Error> {
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
        let mut cleanup = check_borrow(runtime, record, borrow)?;
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
        cleanup.invoked = true;
        replace(runtime, record, Data::Cleanup(cleanup));
        let roots = runtime.roots.len();
        runtime.roots.push((record, Repr::Ref));
        runtime.safepoint(frame, unsafe { &*ctx }, &[]);
        // The active self and remainder live in the rooted cleanup record during all callbacks.
        unsafe { crate::jit::invoke(code, ctx, &[borrow.object]) };
        if unsafe { (*ctx).status } == FAILED {
            let failure = unsafe { (*ctx).failure };
            let mut cleanup = read_cleanup(runtime, record)?;
            runtime.heap.payload_mut::<runtime::Failure>(failure).expect("bridge failure").cleanup_origin = true;
            cleanup.captured = failure;
            replace(runtime, record, Data::Cleanup(cleanup));
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
            unsafe { post.write(DestroyPost { failure }) };
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
        let mut cleanup = check_borrow(runtime, record, borrow_ref)?;
        if !cleanup.invoked {
            return Err(error("finish before invoke"));
        }
        runtime.champ_finish_destroy(KeyRef { object: borrow_ref.object, serial: borrow_ref.serial },
            HolderId(borrow_ref.holder), OwnerId(borrow_ref.owner))?;
        cleanup.active = None;
        cleanup.invoked = false;
        replace(runtime, record, Data::Cleanup(cleanup));
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
    let runtime = unsafe { &mut *(*ctx).runtime };
    let record = unsafe { (*input).record };
    let result = (|| {
        let mut cleanup = read_cleanup(runtime, record)?;
        if cleanup.captured != failure || failure == 0 || unsafe { (*ctx).status } != OK {
            return Err(error("cleanup failure must consume captured carrier once"));
        }
        cleanup.failure = if cleanup.failure == 0 {
            failure
        } else {
            runtime.failure_link(cleanup.failure, failure, true)
        };
        cleanup.captured = 0;
        replace(runtime, record, Data::Cleanup(cleanup));
        Ok::<_, Error>(())
    })();
    match result {
        Ok(()) => {
            unsafe {
                (*input).record = 0;
                post.write(CleanupSlot { record });
            }
            NORMAL
        }
        Err(error) => stop(ctx, error),
    }
}

/// S2 packet close only: no pending, reserve, defer runner, prefix or plan executor.
/// Entry rejection shares the exact claim/invoke/finish implementation used by Rust direct tests.
fn close_packet(ctx: *mut Ctx, input: &mut PacketSlot, primary: Word) -> Result<Word, Error> {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let mut state = runtime.champ_cleanup_from_packet(input)?;
    let mut initial = read_cleanup(runtime, state.record)?;
    initial.failure = primary;
    replace(runtime, state.record, Data::Cleanup(initial));
    let roots = runtime.roots.len();
    runtime.roots.push((state.record, Repr::Ref));
    runtime.roots.push((primary, Repr::Ref));
    let mut result = Ok(primary);
    loop {
        let runtime = unsafe { &mut *(*ctx).runtime };
        let cleanup = read_cleanup(runtime, state.record)?;
        let selected = cleanup.remainder.runs().flat_map(|run| {
            run.words().map(move |object| (object, run.holder, run.owner))
        }).max_by_key(|(object, _, _)| runtime.heap.champ_identity(*object).unwrap().serial);
        let Some((object, holder, owner)) = selected else { break };
        let key = KeyRef { object, serial: runtime.heap.champ_identity(object).unwrap().serial };
        let entry = runtime.loaded.program.objects[crate::heap::payload(runtime.heap.header_of(object)) as usize]
            .destroy.ok_or_else(|| error("no destroy"))?.0 as u64;
        let mut borrow = DestroyBorrow::default();
        let mut post = DestroyPost::default();
        if rt_destroy_claim(ctx, &mut state, key, holder.0, owner.0, &mut borrow) == OUTCOME_STOP
            || rt_destroy_invoke(ctx, &mut state, &borrow, entry, &mut post) == OUTCOME_STOP
        {
            result = Err(error("rejection cleanup stopped"));
            break;
        }
        if rt_destroy_finish(ctx, &mut state, &mut borrow) == OUTCOME_STOP {
            result = Err(error("rejection finish stopped"));
            break;
        }
        if post.failure != 0 && rt_cleanup_failure(ctx, &mut state, post.failure, &mut state) == OUTCOME_STOP {
            result = Err(error("rejection failure capture stopped"));
            break;
        }
    }
    unsafe { &mut *(*ctx).runtime }.roots.truncate(roots);
    result
}

pub extern "C" fn rt_entry_reject(ctx: *mut Ctx, io: *mut CallOwn) -> Outcome {
    let frame = frame_pointer!();
    let runtime = unsafe { &mut *(*ctx).runtime };
    let io = unsafe { &mut *io };
    let Some(entry) = runtime.champ_entry_contract(io.entry_abi) else {
        return stop(ctx, error("entry rejection ABI"));
    };
    if entry.replacements >= OUTCOME_STOP - 1 {
        return stop(ctx, error("invalid owning outcome count"));
    }
    // Validate before constructing or consuming anything, including a repeated rejection.
    if let Err(error) = packet(runtime, io.input.record).and_then(|packet| validate(runtime, &packet)) {
        return stop(ctx, error);
    }
    let roots = runtime.roots.len();
    runtime.roots.push((io.input.record, Repr::Ref));
    runtime::rt_stack_overflow(ctx);
    if unsafe { (*ctx).status } != FAILED {
        unsafe { &mut *(*ctx).runtime }.roots.truncate(roots);
        return OUTCOME_STOP;
    }
    let primary = unsafe { (*ctx).failure };
    unsafe { &mut *(*ctx).runtime }.roots.push((primary, Repr::Ref));
    unsafe {
        (*ctx).failure = crate::word::UNBOUND;
        (*ctx).status = OK;
    }
    // Rejection may use the reserved Rust/machine headroom, never below the hard stack boundary.
    let old_limit = unsafe { (*ctx).stack_limit };
    let hard_limit = old_limit.saturating_sub(runtime::STACK_HEADROOM);
    let usable_limit = hard_limit.saturating_add(crate::jit::MAX_FRAME);
    unsafe { (*ctx).stack_limit = old_limit.min(usable_limit) };
    unsafe { &mut *(*ctx).runtime }.safepoint(frame, unsafe { &*ctx }, &[]);
    let result = close_packet(ctx, &mut io.input, primary);
    unsafe { (*ctx).stack_limit = old_limit };
    let runtime = unsafe { &mut *(*ctx).runtime };
    runtime.roots.truncate(roots);
    match result {
        Ok(failure) => {
            unsafe {
                (*ctx).failure = failure;
                (*ctx).status = FAILED;
                io.output.write(CallPost { value: 0, packet: PacketSlot::default(), failure });
            }
            entry.replacements + 1
        }
        Err(error) => {
            if matches!(unsafe { (*ctx).status }, FATAL | EXHAUSTED) {
                OUTCOME_STOP
            } else {
                stop(ctx, error)
            }
        }
    }
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
