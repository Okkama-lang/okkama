//! Finite P1 adapters. Plain Atomic values carry no resource obligations.
use super::*;
use crate::runtime::{self, Ctx, FailureKind};
use okkama_sparrow::spb17 as s;

pub(super) fn invoke(ctx: *mut Ctx, function: usize, io: *mut CallOwn, args: &[Word]) -> Word {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let source = runtime.champ.machine.as_ref().expect("Champ program").source.clone();
    let name = source.functions[function].proofs.entry.as_ref().unwrap().external.as_deref().unwrap();
    // Acceptance follows the packet ABI for every call-boundary entry, including replace.
    let mut post = CallPost::default();
    if !io.is_null() {
        let input = unsafe { &mut (*io).input };
        if !runtime.champ_packet_keys(input).is_ok_and(|keys| keys.is_empty()) {
            return super::helpers::stop(ctx, Error("P1 plain boundary received obligations".into()));
        }
        if rt_own_move(ctx, input, &mut post.packet) != NORMAL {
            return OUTCOME_STOP;
        }
    }
    let value = if name.starts_with("runtime/") {
        let value = runtime::rt_native(ctx, function as u64, args.as_ptr(), args.len() as u64);
        if unsafe { (*ctx).status } == runtime::FAILED
            && let Some(FailureKind::Program(message)) = runtime.failure(unsafe { (*ctx).failure })
                .map(|failure| &failure.kind)
        {
            let message = message.clone();
            let descriptor = runtime.champ.machine.as_ref().unwrap().native_failure;
            let fields = [
                runtime.heap.allocate_text("runtime"),
                runtime.heap.allocate_text("native"),
                runtime.heap.allocate_text(name),
                runtime.heap.allocate_text(&message),
            ];
            let object = runtime.heap.object_new(descriptor, &fields);
            let failure = runtime.failure_new(FailureKind::Native(object));
            unsafe { (*ctx).failure = failure };
        }
        value
    } else if let Some(check) = name.strip_prefix(s::TEST_INTRINSIC_PREFIX) {
        super::testing::intrinsic(ctx, check, args)
    } else if name == "okkama.Error.text" {
        let text = error_text(runtime, &source, args[0]);
        runtime.heap.allocate_text(&text)
    } else {
        let (operation, _) = name.strip_prefix("okkama.atomic.Atomic.").unwrap().split_once('$').unwrap();
        match operation {
            "new_value" => {
                let s::Type::Object(descriptor) = source.functions[function].results[0] else { unreachable!() };
                runtime.heap.object_new(descriptor, &[args[0], 0])
            }
            "close" => {
                runtime.heap.set_field(args[0], 1, 1);
                runtime.heap.set_field(args[0], 0, crate::word::UNBOUND);
                0
            }
            "value" | "replace" => {
                if runtime.heap.field(args[0], 1) != 0 {
                    let descriptor = source.objects.iter().position(|object| {
                        object.name == "async.atomic.AtomicClosed"
                    }).expect("validated AtomicClosed descriptor") as u32;
                    let tag = runtime.heap.int(0);
                    let object = runtime.heap.object_new(descriptor, &[tag]);
                    let failure = runtime.failure_new(FailureKind::Thrown(object));
                    unsafe {
                        (*ctx).failure = failure;
                        (*ctx).status = runtime::FAILED;
                    }
                    0
                } else if operation == "value" {
                    runtime.heap.field(args[0], 0)
                } else {
                    runtime.heap.set_field(args[0], 0, args[1]);
                    0
                }
            }
            _ => unreachable!("trusted finite P1 catalog"),
        }
    };
    if io.is_null() {
        return value;
    }
    if unsafe { (*ctx).status } == runtime::FAILED {
        post.failure = unsafe { (*ctx).failure };
        post.packet.record = 0;
        unsafe { (*io).output.write(post) };
        UNWIND
    } else if unsafe { (*ctx).status } != runtime::OK {
        OUTCOME_STOP
    } else {
        post.value = value;
        unsafe { (*io).output.write(post) };
        NORMAL
    }
}

pub(super) fn error_text(runtime: &Runtime, source: &s::Program, object: Word) -> String {
    let descriptor = heap::payload(runtime.heap.header_of(object)) as usize;
    if descriptor == runtime.champ.machine.as_ref().unwrap().native_failure as usize {
        return runtime.heap.text(runtime.heap.field(object, 3)).to_string();
    }
    let declaration = &source.objects[descriptor];
    if declaration.fields.is_empty() {
        let tag = runtime.small_or_big(runtime.heap.field(object, 0)).expect("enum tag") as usize;
        return declaration.error_cases[tag].clone();
    }
    let mut text = declaration.error_cases[0].clone();
    text.push('(');
    for (index, field) in declaration.fields.iter().enumerate() {
        if index != 0 {
            text.push_str(", ");
        }
        let word = runtime.heap.field(object, field.storage as usize);
        match declaration.layout.storage_fields[field.storage as usize] {
            s::Type::Str => text.push_str(&format!("{:?}", runtime.heap.text(word))),
            s::Type::Int => match runtime.heap.integer(word) {
                heap::Integer::Small(value) => text.push_str(&value.to_string()),
                heap::Integer::Big(value) => text.push_str(&value.to_string()),
            },
            s::Type::Bool => text.push_str(if word == 0 { "false" } else { "true" }),
            _ => unreachable!("P1 error payload"),
        }
    }
    text.push(')');
    text
}
