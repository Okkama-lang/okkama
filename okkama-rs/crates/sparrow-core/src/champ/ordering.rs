//! `ordering()` on Champ (#678): Tomato's `ordering` intrinsic as runtime operations, not as a loop in OIR.
//! `array.order` sorts scalars and text with Tomato's comparators and budget polling (`sort_natural`);
//! `array.order.dispatch` runs Tomato's bottom-up merge (`merge_order`) and calls the elements' `order`
//! slot through the site's role dispatch once per comparison, so the comparisons, and any effect inside
//! them, come in Tomato's order. The first failing comparison stops the sort and its failure propagates.
#![allow(clippy::not_unsafe_ptr_arg_deref)]
use super::helpers::{error, stop};
use super::machine::repr;
use super::{CallOwn, CallPost, PacketSlot};
use crate::heap;
use crate::intrinsics::{NaturalOrder, merge_order, sort_natural};
use crate::runtime::{self, Ctx, OK};
use crate::word::{Repr, Word};
use okkama_sparrow::spb17 as s;

/// The natural order of the elements of a sortable array type (`ARRAY_ORDER_TYPE`).
fn natural(array: s::Type) -> Option<NaturalOrder> {
    Some(match s::element_type(array)? {
        s::Type::Int => NaturalOrder::Int,
        s::Type::Float => NaturalOrder::Float,
        s::Type::Rat => NaturalOrder::Rat,
        s::Type::Str => NaturalOrder::Str,
        s::Type::Byte | s::Type::Char => NaturalOrder::Raw,
        _ => return None,
    })
}

fn elements(runtime: &runtime::Runtime, array: Word, ty: s::Type) -> Vec<Word> {
    if ty == s::Type::Bytes {
        runtime.heap.byte_view(array).iter().map(|&byte| Word::from(byte)).collect()
    } else {
        runtime.heap.elements(array).into_owned()
    }
}

fn allocate(runtime: &mut runtime::Runtime, ty: s::Type, items: &[Word]) -> Word {
    if ty == s::Type::Bytes {
        let bytes: Vec<u8> = items.iter().map(|&word| word as u8).collect();
        return runtime.heap.allocate_bytes(&bytes);
    }
    let element = s::element_type(ty).expect("verified array type");
    runtime.heap.allocate_array(repr(element), items)
}

/// `array.order` of `array`, an array of type `ty`: a new array of its elements in ascending order (a byte
/// array stays bytes). An exhausted budget or an interrupt stops the machine, as in Tomato; nothing fails.
pub(crate) fn order_natural(ctx: *mut Ctx, array: Word, ty: s::Type) -> Word {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let Some(order) = natural(ty) else {
        return stop(ctx, error("array.order of an unsortable array"));
    };
    let mut items = elements(runtime, array, ty);
    if let Err(what) = sort_natural(runtime, &mut items, order) {
        return runtime.exhaust(unsafe { &mut *ctx }, what);
    }
    allocate(runtime, ty, &items)
}

/// `array.order.dispatch` at (function, pc) on `array`: the elements merged by their `order` slot, called
/// through the site's dispatch. Returns the new array, or 0 with the failure of the first failing comparison
/// (or a stop) in the context. Re-entrant: each comparison runs compiled code, which may collect, so the
/// source array, which holds every element, stays a runtime root while the comparisons run.
pub extern "C" fn rt_machine_order(ctx: *mut Ctx, function: u64, pc: u64, array: Word) -> Word {
    okkama_devtrace::count!("runtime", "rt_machine_order");
    let frame = runtime::frame_pointer!();
    let runtime = unsafe { &mut *(*ctx).runtime };
    let Some(program) = runtime.champ.machine.as_ref().map(|machine| machine.source.clone()) else {
        return stop(ctx, error("array.order.dispatch outside a Champ program"));
    };
    let source = &program.functions[function as usize];
    let Some(s::Instruction::Champ { op: s::Opcode::OrderDispatch, arguments, .. }) = source.code.get(pc as usize)
    else {
        return stop(ctx, error("invalid verified array.order.dispatch"));
    };
    let (Some(s::Argument::Dispatch(dispatch)), Some(s::Argument::Location(s::Location::Register(register)))) =
        (arguments.first(), arguments.get(1))
    else {
        return stop(ctx, error("invalid verified array.order.dispatch"));
    };
    let ty = source.registers[*register as usize];
    let owned = program.objects[dispatch.role as usize].role.as_ref()
        .is_some_and(|role| role.slots[dispatch.slot as usize].entry_abi == s::EntryAbi::Owned);
    // Each listed object descriptor and its implementation of the slot (`DISPATCH_TARGET`): the function, or
    // its owning entry for an owned slot.
    let targets = dispatch.targets.iter().map(|&target| {
        let function = s::implementation(&program, target, dispatch.role, dispatch.slot)?;
        let entry = if owned { runtime.champ_entry_id(function as usize)? } else { u64::from(function) };
        Some((target, entry))
    }).collect::<Option<Vec<_>>>();
    let Some(targets) = targets else {
        return stop(ctx, error("array.order.dispatch target without an implementation"));
    };
    let items = elements(runtime, array, ty);
    let roots = runtime.roots.len();
    runtime.roots.push((array, Repr::Ref));
    let mut invalid = false;
    let sorted = merge_order(items, |left, right| {
        let runtime = unsafe { &*(*ctx).runtime };
        let descriptor = heap::payload(runtime.heap.header_of(left));
        let Some(&(_, entry)) = targets.iter().find(|(target, _)| *target == descriptor) else {
            invalid = true;
            return None;
        };
        let result = compare(ctx, entry, owned, [left, right])?;
        // `compare.Order` is a plain enum: its one storage word is the tagged case, and `greater` is case 2.
        let runtime = unsafe { &*(*ctx).runtime };
        Some(crate::word::small_value(runtime.heap.field(result, 0)) == Some(2))
    });
    let runtime = unsafe { &mut *(*ctx).runtime };
    runtime.roots.truncate(roots);
    if invalid {
        // A receiver outside the dispatch's objects stops the machine, as a dispatch does.
        return stop(ctx, error("array.order.dispatch receiver outside its dispatch"));
    }
    let Some(items) = sorted else { return 0 };
    let word = allocate(runtime, ty, &items);
    runtime.safepoint(frame, unsafe { &*ctx }, &[(word, Repr::Ref)]);
    word
}

/// One comparison `left.order(right)` through the selected entry: a plain call of an observation, or an owned
/// call with an empty input packet that must return an empty packet (the elements are plain, the result is a
/// plain `compare.Order`). None when it failed or stopped; the context then holds the failure or the stop.
fn compare(ctx: *mut Ctx, entry: u64, owned: bool, arguments: [Word; 2]) -> Option<Word> {
    if !owned {
        let result = super::machine::rt_machine_external(ctx, entry, std::ptr::null_mut(), arguments.as_ptr(), 2);
        return (unsafe { (*ctx).status } == OK).then_some(result);
    }
    let mut input = PacketSlot::default();
    if super::rt_own_empty(ctx, &mut input) != 0 {
        return None;
    }
    let mut post = CallPost::default();
    let mut io = CallOwn { input, entry_abi: entry, output: &mut post, context: 0, cleanup_mode: 0 };
    let outcome = super::cleanup::rt_call_owned(ctx, entry, arguments.as_ptr(), 2, &mut io, &mut post);
    if unsafe { (*ctx).status } != OK {
        return None;
    }
    let runtime = unsafe { &*(*ctx).runtime };
    if outcome != 0 || !runtime.champ_packet_keys(&post.packet).is_ok_and(|keys| keys.is_empty()) {
        stop(ctx, error("a comparison of array.order.dispatch returned an ownership packet"));
        return None;
    }
    Some(post.value)
}

#[cfg(test)]
#[path = "ordering_tests.rs"]
mod tests;
