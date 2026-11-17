//! The driver call bridge (memo OIR-2-async-remaining §6.4, #810 G1 step 2): a driver of Tomato's catalogue (a
//! timeout, later a collector or a scope) calls a program function of the Champ machine. The call offers an empty
//! packet. A function of the observation ABI is called plainly; an owned one returns an output packet, which joins
//! the driver's INPUTS before the driver does anything else (I-G5): a timer Future's duty, for instance, which the
//! driver's cache-duty settle then releases. The returned entries take the owner of the driver's cache, as the
//! entries `fut.new` accepted into INPUTS have it, so that settle finds them.
use super::helpers::{consume, owner_preflight, owner_transition, packet, replace, stop, store, validate};
use super::records::{Data, Packet};
use super::*;
use crate::runtime::{self, Ctx};
use crate::tasks::{get, set};
use okkama_sparrow::spb17 as s;

impl Runtime {
    /// Call `function` with `args` for the driver `frame`; None when it failed or stopped, as the context says.
    pub(crate) fn champ_driver_call(&mut self, ctx: *mut Ctx, frame: Word, function: usize, args: &[Word])
        -> Option<Word>
    {
        let observation = self.champ.machine.as_ref().is_some_and(|machine| {
            machine.source.functions.get(function).is_some_and(|f| f.entry_abi == s::EntryAbi::Observation)
        });
        if observation {
            let value = super::machine::rt_machine_external(ctx, function as u64, std::ptr::null_mut(),
                args.as_ptr(), args.len() as u64);
            return (unsafe { (*ctx).status } == runtime::OK).then_some(value);
        }
        let Some(entry) = self.champ_entry_id(function) else {
            stop(ctx, error("a driver helper without its entry"));
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
            NORMAL => match runtime.champ_merge_inputs(frame, post.packet.record) {
                Ok(()) => Some(post.value),
                Err(failure) => {
                    stop(ctx, failure);
                    None
                }
            },
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

    /// The origins of a collector `receiver` (#810 G1 step 3): its `Sequencing` field and the functions that
    /// implement that sequence's `size` and `item`, selected by its descriptor as a role dispatch would be. Tomato's
    /// `collect_start` calls them through the driver call bridge. None, with a stop, for a receiver without that
    /// shape (the loader checked it).
    pub(crate) fn champ_sequence(&mut self, ctx: *mut Ctx, receiver: Word) -> Option<(Word, usize, usize)> {
        let found = self.champ.machine.as_ref().and_then(|machine| {
            let program = &machine.source;
            let descriptor = crate::heap::payload(self.heap.header_of(receiver));
            let Some(&s::Type::Role(role)) = program.objects.get(descriptor as usize)?.layout.storage_fields.first()
            else {
                return None;
            };
            let slots = &program.objects.get(role as usize)?.role.as_ref()?.slots;
            let slot = |name: &str| slots.iter().position(|slot| slot.name == name).map(|slot| slot as u32);
            let sequence = self.heap.field(receiver, 0);
            let target = crate::heap::payload(self.heap.header_of(sequence));
            let size = s::implementation(program, target, role, slot("size")?)?;
            let item = s::implementation(program, target, role, slot("item")?)?;
            Some((sequence, size as usize, item as usize))
        });
        if found.is_none() {
            stop(ctx, error("a collector without a sequence of origins"));
        }
        found
    }

    /// AllFutures' result on Champ (#810 G1 step 3, ruling Q1): the members' outcomes in input order, as an array of
    /// the `Wrap` helper's parameter type, which the helper wraps as the entry's `Sequencing<T>`. The array stays a
    /// root across the call. None when the helper failed or stopped (the context says which).
    pub(crate) fn champ_all_results(&mut self, ctx: *mut Ctx, frame: Word, function: usize, values: &[Word])
        -> Option<Word>
    {
        let wrap = self.loaded.program.functions[function].intrinsic.as_ref()
            .and_then(|intrinsic| intrinsic.helpers.first()).map(|helper| helper.0 as usize);
        let parameter = wrap.and_then(|wrap| {
            let source = &self.champ.machine.as_ref()?.source.functions[wrap];
            match source.params.first()? {
                s::Location::Register(id) => Some(source.registers[*id as usize]),
                _ => None,
            }
        });
        let (Some(wrap), Some(parameter)) = (wrap, parameter) else {
            stop(ctx, error("AllFutures without its Wrap helper"));
            return None;
        };
        let array = if parameter == s::Type::Bytes {
            let bytes: Vec<u8> = values.iter().map(|&word| word as u8).collect();
            self.heap.allocate_bytes(&bytes)
        } else {
            let element = s::element_type(parameter).expect("HOLDER_HELPER: Wrap takes an array");
            self.heap.allocate_array(super::machine::repr(element), values)
        };
        self.roots.push((array, crate::word::Repr::Ref));
        let result = self.champ_driver_call(ctx, frame, wrap, &[array]);
        let runtime = unsafe { &mut *(*ctx).runtime };
        runtime.roots.pop();
        result
    }

    /// I-G5: the packet `record` a driver got back joins its INPUTS, checked before the first store, with the owner
    /// of the driver's cache.
    fn champ_merge_inputs(&mut self, frame: Word, record: Word) -> Result<(), Error> {
        let returned = packet(self, record)?;
        if returned.is_empty() {
            consume(self, record);
            return Ok(());
        }
        let owner = self.champ_cache(frame)?.owner;
        let offset = self.champ_body(frame)?.packet().ok_or_else(|| error("a driver without its inputs"))?;
        let current = get(frame, offset);
        let mut inputs = if current == 0 { Packet::default() } else { packet(self, current)? };
        let mut all = inputs.clone();
        all.append(returned.clone());
        validate(self, &all)?;
        owner_preflight(self, &returned, owner)?;
        let moved = owner_transition(self, returned, owner);
        inputs.append(moved);
        if current == 0 {
            let stored = store(self, Data::Packet(inputs));
            set(frame, offset, stored);
        } else {
            replace(self, current, Data::Packet(inputs));
        }
        consume(self, record);
        Ok(())
    }
}
