//! Finite P1 adapters. Plain Atomic values carry no resource obligations.
use super::*;
use crate::runtime::{self, Ctx, FailureKind};
use okkama_sparrow::spb17 as s;

pub(super) fn invoke(ctx: *mut Ctx, function: usize, io: *mut CallOwn, args: &[Word]) -> Word {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let source = runtime.champ.machine.as_ref().expect("Champ program").source.clone();
    let name = source.functions[function].proofs.entry.as_ref().unwrap().external.as_deref().unwrap();
    if name == s::p1::SYNC_CLOSURE_CLEANUP || name == s::p1::ANY_CLEANUP {
        return opaque_cleanup(ctx, io);
    }
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
    let value = if super::machine::native_symbol(name) {
        let value = runtime::rt_native(ctx, function as u64, args.as_ptr(), args.len() as u64);
        native_refusal(ctx, name);
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
            "closed" => u64::from(runtime.heap.field(args[0], 1) != 0),
            "value" | "replace" | "compare_and_replace" => {
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
                } else if operation == "compare_and_replace" {
                    // A failed comparison publishes nothing; plain values compare without failure.
                    let ty = source.functions[function].registers[1];
                    let current = runtime.heap.field(args[0], 0);
                    match super::machine::value_equal(runtime, &source, current, args[1], ty) {
                        Ok(true) => {
                            runtime.heap.set_field(args[0], 0, args[2]);
                            1
                        }
                        Ok(false) => 0,
                        Err(message) => return super::helpers::stop(ctx, Error(message)),
                    }
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

/// A synchronous native's entry fails only as its mask (`NATIVE_FAILURES`) admits, and the failure keeps its kind
/// (#719, language spec 9.5): the provider's own `NativeFailure` (Native), which a catch may take, and a program
/// failure of the call (a missing provider, a refused permission), which passes every catch with its message, as
/// on Tomato. A cancellation passes unchanged too (sync masks carry no cancellation bit). A thrown value or a
/// `Panic` failure is outside the trusted contract: it stops the machine, naming the native. A Rust panic in a
/// provider never gets here: `rt_native` stops the run with an internal error on both engines.
pub(super) fn native_refusal(ctx: *mut Ctx, name: &str) {
    let runtime = unsafe { &mut *(*ctx).runtime };
    if unsafe { (*ctx).status } != runtime::FAILED {
        return;
    }
    match runtime.failure(unsafe { (*ctx).failure }).map(|failure| &failure.kind) {
        Some(FailureKind::Native(_) | FailureKind::Program(_) | FailureKind::Cancelled) => {}
        Some(FailureKind::Thrown(_) | FailureKind::Panic(_)) | None => {
            super::helpers::stop(ctx, Error(format!("native {name} failed outside its entry contract")));
        }
    }
}

/// An opaque cleanup intrinsic releases the complete owner graph of an action value or `any` box: every
/// identity of the packet it is given, once, through the common cleanup executor, then returns an empty
/// packet. Its contract has no failure, so a destroy failure stops the machine.
pub(super) fn opaque_cleanup(ctx: *mut Ctx, io: *mut CallOwn) -> Word {
    if io.is_null() {
        return super::helpers::stop(ctx, Error("opaque cleanup without its packet".into()));
    }
    let input = unsafe { &mut (*io).input };
    match super::cleanup::close_packet(ctx, input, 0) {
        Ok(0) => {}
        Ok(_) => return super::helpers::stop(ctx, Error("opaque cleanup: a destroy failed".into())),
        Err(error) => return super::helpers::stop(ctx, error),
    }
    let mut post = CallPost::default();
    if rt_own_empty(ctx, &mut post.packet) != NORMAL {
        return OUTCOME_STOP;
    }
    unsafe { (*io).output.write(post) };
    NORMAL
}

/// Tomato's text of a cancellation (`Cancelled.text()`, OIR `CANCELLED_TEXT`).
pub(super) const CANCELLED_TEXT: &str = "Operation was cancelled";

/// The text of a plain Error value, as Tomato's compiled `text()` writes it (#306): the case, then its
/// payload in parentheses, strings as Okkama literals, plain enums and Error values by their own texts.
pub(super) fn error_text(runtime: &Runtime, source: &s::Program, object: Word) -> String {
    let descriptor = heap::payload(runtime.heap.header_of(object)) as usize;
    if descriptor == runtime.champ.machine.as_ref().unwrap().native_failure as usize {
        return runtime.heap.text(runtime.heap.field(object, 3)).to_string();
    }
    // A cancellation caught as Error (#738) reads as Tomato's text of a cancellation.
    if s::cancelled(source) == Some(descriptor as u32) {
        return CANCELLED_TEXT.to_owned();
    }
    let declaration = &source.objects[descriptor];
    if declaration.fields.is_empty() {
        if declaration.layout.storage_fields.is_empty() {
            return declaration.error_cases[0].clone();
        }
        let tag = runtime.small_or_big(runtime.heap.field(object, 0)).expect("enum tag") as usize;
        return declaration.error_cases[tag].clone();
    }
    case_text(runtime, source, object, declaration.error_cases[0].clone())
}

/// The text of a case object of a plain variant (`variant`), by the variant's case names (#763): `Shape.dot`,
/// or `Shape.rect(2, 3)` with its payload shown as an Error payload is.
fn variant_text(runtime: &Runtime, source: &s::Program, variant: u32, object: Word) -> String {
    let descriptor = heap::payload(runtime.heap.header_of(object));
    if !source.objects[descriptor as usize].error_cases.is_empty() {
        return error_text(runtime, source, object);
    }
    let cases = &source.objects[variant as usize];
    let case = cases.variant_cases.iter().position(|&case| case == descriptor).expect("a case of the variant");
    let name = cases.enum_names[case].clone();
    if source.objects[descriptor as usize].fields.is_empty() {
        return name;
    }
    case_text(runtime, source, object, name)
}

/// `name(payload, …)`: the payload fields of a case object in declaration order.
fn case_text(runtime: &Runtime, source: &s::Program, object: Word, name: String) -> String {
    let declaration = &source.objects[heap::payload(runtime.heap.header_of(object)) as usize];
    let mut text = name;
    text.push('(');
    for (index, field) in declaration.fields.iter().enumerate() {
        if index != 0 {
            text.push_str(", ");
        }
        let word = runtime.heap.field(object, field.storage as usize);
        match declaration.layout.storage_fields[field.storage as usize] {
            s::Type::Str => text.push_str(&literal(runtime.heap.text(word))),
            s::Type::Int => match runtime.heap.integer(word) {
                heap::Integer::Small(value) => text.push_str(&value.to_string()),
                heap::Integer::Big(value) => text.push_str(&value.to_string()),
            },
            s::Type::Bool => text.push_str(if word == 0 { "false" } else { "true" }),
            s::Type::Byte => text.push_str(&word.to_string()),
            s::Type::Float => text.push_str(&crate::scalar::float_text(f64::from_bits(word))),
            s::Type::Rat => text.push_str(&runtime.rational(word).to_string()),
            s::Type::Variant(variant) => text.push_str(&variant_text(runtime, source, variant, word)),
            s::Type::Object(id) if !source.objects[id as usize].error_cases.is_empty() => {
                text.push_str(&error_text(runtime, source, word));
            }
            s::Type::Object(id) => {
                let tag = runtime.small_or_big(runtime.heap.field(word, 0)).expect("enum tag") as usize;
                text.push_str(&source.objects[id as usize].enum_names[tag]);
            }
            _ => unreachable!("P1 error payload"),
        }
    }
    text.push(')');
    text
}

/// A string in Okkama literal syntax: backslash, quote, newline, tab and carriage return escaped, other
/// control characters as `\u{hex}`, everything else unchanged (#306).
fn literal(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
#[path = "p1_tests.rs"]
mod tests;
