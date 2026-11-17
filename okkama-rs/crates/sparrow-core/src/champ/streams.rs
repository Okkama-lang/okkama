//! FutureCompletions on Champ (#810 G1 step 4, ruling Q4 of the steps 3–4 review). The stream Tomato keeps in
//! `Runtime.states` lives in the object's runtime word, the storage field after its source fields (ruling PR-C,
//! item 1). It is made when the stream is first read: by `remaining_count` or by the first step of the first
//! `completion` request. It holds references only: no duty of the origins is the stream's.
//!
//! Reading the origins calls the sequence's `size` and `item` with no driver frame to keep what they return, so a
//! callee that returns obligations (a user sequence whose `item` makes a fresh Future) is refused: its packet is
//! closed at once and the read fails with a program failure (ruling Q2).
use super::helpers::stop;
use super::*;
use crate::heap::{self, Payload};
use crate::primitives::Stream;
use crate::runtime::{self, Ctx, FailureKind};
use crate::word::Repr;
use okkama_sparrow::spb17 as s;

/// The program failure of a stream read whose `item` returned a Future with a duty of its own (ruling Q2, #850).
pub(crate) const REFUSED_ORIGIN: &str = "FutureCompletions: an origin was returned with a duty of its own; only \
    origins whose duties stay with their owners can be streamed";

/// The stream of a Champ FutureCompletions, as a heap payload the object's runtime word refers to.
#[derive(Debug)]
pub(crate) struct StreamCell(pub Stream);

impl Payload for StreamCell {
    fn trace(&self, visit: &mut dyn FnMut(Word, Repr)) {
        let stream = &self.0;
        stream.entries.iter().chain(&stream.inbox).chain(&stream.waiters).chain(&stream.aborted)
            .for_each(|&word| visit(word, Repr::Ref));
    }
    fn external_bytes(&self) -> usize {
        let stream = &self.0;
        (stream.entries.capacity() + stream.inbox.capacity() + stream.waiters.capacity()) * size_of::<Word>()
            + stream.positions.capacity() * size_of::<(Word, usize)>() + stream.queue.capacity() * size_of::<usize>()
            + stream.emitted.capacity() + stream.cancelled.capacity()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl Runtime {
    /// The storage index of the runtime word of `object`, a FutureCompletions specialization of the running program:
    /// its last storage field. None for any other object.
    fn champ_stream_word(&self, object: Word) -> Option<usize> {
        if object == 0 || object & 1 != 0 || heap::kind(self.heap.header_of(object)) != heap::OBJECT {
            return None;
        }
        let descriptor = heap::payload(self.heap.header_of(object)) as usize;
        let machine = self.champ.machine.as_ref()?;
        if !machine.streams.get(descriptor).copied().unwrap_or(false) {
            return None;
        }
        machine.source.objects.get(descriptor)?.layout.storage_fields.len().checked_sub(1)
    }

    /// Tomato's stream of a Champ FutureCompletions, for the drivers both engines share (`primitives::stream_mut`);
    /// None before it is made.
    pub(crate) fn champ_stream(&mut self, object: Word) -> Option<&mut Stream> {
        let index = self.champ_stream_word(object)?;
        let cell = self.heap.field(object, index);
        if cell == 0 {
            return None;
        }
        self.heap.payload_mut::<StreamCell>(cell).map(|cell| &mut cell.0)
    }

    /// Make the empty stream of `object` before its origins are read, so the origins read so far are roots.
    pub(crate) fn champ_stream_new(&mut self, object: Word) -> bool {
        let Some(index) = self.champ_stream_word(object) else { return false };
        let cell = self.heap.allocate_payload(Box::new(StreamCell(Stream::default())));
        self.heap.set_field(object, index, cell);
        true
    }

    /// Forget the stream of `object` after a failed read, as Tomato removes its state: the next read starts over.
    pub(crate) fn champ_stream_drop(&mut self, object: Word) {
        if let Some(index) = self.champ_stream_word(object) {
            self.heap.set_field(object, index, 0);
        }
    }

    /// The receiver register of `frame` when it is a `completion` request that listens to its stream's unsettled
    /// origins (#444): its offset and its value. The stream listens through the request that started it, so a
    /// released request keeps that one register for as long as an origin may name it as a listener, as Tomato's
    /// frame keeps its registers; it is a reference only (ruling Q4), the stream's object.
    pub(crate) fn champ_stream_listener(&self, frame: Word) -> Option<(u64, Word)> {
        if crate::tasks::get(frame, crate::tasks::FLAGS) & crate::tasks::INBOX == 0 {
            return None;
        }
        let function = self.function_of(frame);
        if self.intrinsics.get(function).copied().flatten() != Some(crate::intrinsics::Intrinsic::StreamCompletion) {
            return None;
        }
        let register = self.loaded.program.functions[function].params.first()?.0;
        let offset = crate::tasks::REGISTERS + u64::from(register) * 8;
        let receiver = crate::tasks::get(frame, offset);
        // Review of steps 3–4: the word kept is the stream's own object, a FutureCompletions of this program.
        debug_assert!(self.champ_stream_word(receiver).is_some(), "a listening request's receiver is a stream");
        Some((offset, receiver))
    }

    /// A call with no driver frame to keep what it returns (#810 G1 step 4, ruling Q2): an observation plainly; an
    /// owned callee with an empty packet, and a packet it returns is closed at once and the call fails. None when the
    /// call failed or stopped (the context says which).
    pub(crate) fn champ_plain_call(&mut self, ctx: *mut Ctx, function: usize, args: &[Word]) -> Option<Word> {
        let observation = self.champ.machine.as_ref().is_some_and(|machine| {
            machine.source.functions.get(function).is_some_and(|f| f.entry_abi == s::EntryAbi::Observation)
        });
        if observation {
            let value = super::machine::rt_machine_external(ctx, function as u64, std::ptr::null_mut(),
                args.as_ptr(), args.len() as u64);
            return (unsafe { (*ctx).status } == runtime::OK).then_some(value);
        }
        let Some(entry) = self.champ_entry_id(function) else {
            stop(ctx, error("a stream call without its entry"));
            return None;
        };
        let mut input = PacketSlot::default();
        if rt_own_empty(ctx, &mut input) != NORMAL {
            return None;
        }
        let mut post = CallPost::default();
        let mut io = CallOwn { input, entry_abi: entry, output: &mut post, context: 0, cleanup_mode: 0 };
        let outcome = super::cleanup::rt_call_owned(ctx, entry, args.as_ptr(), args.len() as u64, &mut io,
            &mut post);
        let runtime = unsafe { &mut *(*ctx).runtime };
        match outcome {
            NORMAL => {
                let returned = super::helpers::packet(runtime, post.packet.record).map(|packet| packet.is_empty());
                match returned {
                    Ok(true) => {
                        super::helpers::consume(runtime, post.packet.record);
                        Some(post.value)
                    }
                    Ok(false) => {
                        refuse_origin(ctx, &mut post.packet);
                        None
                    }
                    Err(failure) => {
                        stop(ctx, failure);
                        None
                    }
                }
            }
            UNWIND => {
                unsafe {
                    (*ctx).failure = post.failure;
                    (*ctx).status = runtime::FAILED;
                }
                None
            }
            _ => None,
        }
    }
}

/// Refuse a stream read whose callee returned `packet` with a duty in it (ruling Q2): nothing could hold the
/// obligation, so the packet closes now under the refusal, as `groups::start` closes a refused packet. A failing
/// destroy links to the refusal as a secondary, and the read fails with the combined failure; a stop during the
/// close (FATAL, EXHAUSTED) stays a stop, with no catchable failure.
pub(super) fn refuse_origin(ctx: *mut Ctx, packet: &mut PacketSlot) {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let failure = runtime.failure_new(FailureKind::Program(REFUSED_ORIGIN.into()));
    let roots = runtime.roots.len();
    runtime.roots.push((failure, Repr::Ref));
    let closed = super::cleanup::close_packet(ctx, packet, failure);
    unsafe { &mut *(*ctx).runtime }.roots.truncate(roots);
    match closed {
        Ok(combined) => unsafe {
            (*ctx).failure = combined;
            (*ctx).status = runtime::FAILED;
        },
        Err(_) if matches!(unsafe { (*ctx).status }, runtime::FATAL | runtime::EXHAUSTED) => {}
        Err(failure) => {
            stop(ctx, failure);
        }
    }
}
