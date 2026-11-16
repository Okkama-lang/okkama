//! The test profile on Champ (§14, OIR-2 §7.4): assertions, the test intrinsics and the runner.
//!
//! Each test runs on a fresh runtime with a running case context, as on Tomato. A failed check is
//! recorded in the case and execution continues. `test.skip` records its reason and fails with the
//! skip marker: a program failure, which no `catch` takes, so it unwinds through every cleanup to the
//! runner. The case is skipped only when that marker alone ends it; a cleanup failure on the way, or a
//! check that failed before, makes it a failure.
use super::*;
use crate::runtime::{self, Ctx, FailureKind};
use okkama_sparrow::spb17 as s;

/// The message of the skip marker, and the whole report of a case that only skipped.
const SKIPPED: &str = "test skipped";
const OUTSIDE: &str = "The test capability is available only inside a running test";

fn program_failure(ctx: *mut Ctx, message: &str) -> Word {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let failure = runtime.failure_new(FailureKind::Program(message.into()));
    unsafe {
        (*ctx).failure = failure;
        (*ctx).status = runtime::FAILED;
    }
    0
}

fn in_case(runtime: &Runtime) -> bool {
    runtime.host.as_ref().is_some_and(|host| host.test_failures.is_some())
}

fn record(runtime: &mut Runtime, message: Option<String>) -> Word {
    let held = message.is_none();
    if let Some(failures) = runtime.host.as_mut().and_then(|host| host.test_failures.as_mut()) {
        failures.extend(message);
    }
    Word::from(held)
}

/// `test.ok`/`test.is`/`test.isnt`/`test.fail` (kinds 0–3): whether the check held. The operand type of
/// `is`/`isnt` is the type of register `register` of function `function`.
pub extern "C" fn rt_machine_test(ctx: *mut Ctx, kind: Word, function: Word, register: Word,
    left: Word, right: Word) -> Word
{
    let runtime = unsafe { &mut *(*ctx).runtime };
    if !in_case(runtime) {
        return program_failure(ctx, OUTSIDE);
    }
    let Some(machine) = runtime.champ.machine.as_ref() else {
        return super::helpers::stop(ctx, super::helpers::error("test check outside a Champ program"));
    };
    let program = machine.source.clone();
    let message = match kind {
        0 => (left == 0).then(|| "expected true, got false".to_owned()),
        3 => Some(runtime.heap.text(left).to_owned()),
        _ => {
            let ty = program.functions[function as usize].registers[register as usize];
            let equal = match super::machine::value_equal(runtime, &program, left, right, ty) {
                Ok(equal) => equal,
                // A comparison the loader should have refused fails the check's case, not the engine.
                Err(message) => {
                    super::helpers::fail(ctx, super::helpers::error(&message));
                    return 0;
                }
            };
            if kind == 1 && !equal {
                Some(format!("expected {}, got {}", show(runtime, &program, right, ty),
                    show(runtime, &program, left, ty)))
            } else if kind == 2 && equal {
                Some(format!("expected values to differ, both were {}", show(runtime, &program, left, ty)))
            } else {
                None
            }
        }
    };
    record(runtime, message)
}

/// The `okkama.test.*` intrinsics of the closed P1 catalogue: `skip`, `close`, `text` and `bytes`.
pub(super) fn intrinsic(ctx: *mut Ctx, name: &str, args: &[Word]) -> Word {
    let runtime = unsafe { &mut *(*ctx).runtime };
    if !in_case(runtime) {
        return program_failure(ctx, OUTSIDE);
    }
    let message = match name {
        "skip" => {
            let reason = runtime.heap.text(args[0]).to_owned();
            if let Some(host) = runtime.host.as_mut() {
                host.test_skipped.get_or_insert(reason);
            }
            return program_failure(ctx, SKIPPED);
        }
        "close" => okkama_runtime_core::testing::close_failure(
            f64::from_bits(args[0]),
            f64::from_bits(args[1]),
            f64::from_bits(args[2]),
            crate::scalar::float_text,
        ),
        "text" => {
            okkama_runtime_core::testing::text_failure(runtime.heap.text(args[0]), runtime.heap.text(args[1]))
        }
        "bytes" => {
            okkama_runtime_core::testing::bytes_failure(runtime.heap.bytes(args[0]), runtime.heap.bytes(args[1]))
        }
        _ => unreachable!("trusted finite P1 catalog"),
    };
    record(runtime, message);
    0
}

/// A value as test reports show it (strings quoted at the top level), as on Tomato: an object shows its type
/// and fields, a variant its case and payload (`Down.unavailable(3)`), as the source writes them (#756).
fn show(runtime: &Runtime, program: &s::Program, word: Word, ty: s::Type) -> String {
    let mut out = String::new();
    show_into(runtime, program, word, ty, true, 0, &mut out);
    out
}

fn show_into(runtime: &Runtime, program: &s::Program, word: Word, ty: s::Type, quoted: bool, depth: usize,
    out: &mut String)
{
    match ty {
        s::Type::Str if quoted => {
            out.push('"');
            out.push_str(runtime.heap.text(word));
            out.push('"');
        }
        s::Type::Str => out.push_str(runtime.heap.text(word)),
        s::Type::Bool => out.push_str(if word == 0 { "false" } else { "true" }),
        s::Type::Byte => out.push_str(&(word as u8 as i8).to_string()),
        s::Type::Char => out.push_str(&word.to_string()),
        s::Type::Int => out.push_str(&runtime.big_value(word).to_string()),
        s::Type::Float => out.push_str(&crate::scalar::float_text(f64::from_bits(word))),
        s::Type::Rat => out.push_str(&runtime.rational(word).to_string()),
        s::Type::Bytes => {
            let items = runtime.heap.bytes(word).iter().map(|&byte| (byte as i8).to_string()).collect::<Vec<_>>();
            out.push_str(&format!("[{}]", items.join(", ")));
        }
        s::Type::Array(shape) => {
            let element = shape.element().expect("verified array shape");
            out.push('[');
            for (index, &item) in runtime.heap.elements(word).iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                show_into(runtime, program, item, element, false, depth, out);
            }
            out.push(']');
        }
        _ if word == 0 || word == crate::word::UNBOUND => out.push_str("null"),
        _ => {
            let header = runtime.heap.header_of(word);
            let object = (heap::kind(header) == heap::OBJECT)
                .then(|| program.objects.get(heap::payload(header) as usize)).flatten();
            match object {
                // An `any` box shows the value it holds.
                Some(object) if object.any_box && object.layout.storage_fields.len() == 1 => {
                    let held = object.layout.storage_fields[0];
                    show_into(runtime, program, runtime.heap.field(word, 0), held, quoted, depth, out);
                }
                // An Error value shows as its text without `Error.text`: `Down.unavailable(3)`.
                Some(object) if !object.error_cases.is_empty() => {
                    out.push_str(&super::p1::error_text(runtime, program, word));
                }
                // A plain enum shows its case's name, as Tomato does.
                Some(object) if object.fields.is_empty() && object.layout.storage_fields == [s::Type::Int] => {
                    let tag = runtime.big_value(runtime.heap.field(word, 0)).to_string();
                    let case = tag.parse::<usize>().ok().and_then(|tag| object.enum_names.get(tag));
                    match case {
                        Some(case) => out.push_str(case.rsplit('.').next().unwrap_or(case)),
                        None => out.push_str(&format!("{}.{tag}", object.name)),
                    }
                }
                Some(object) if object.handle || object.role.is_some() => {
                    out.push_str(&format!("{}@{word:x}", object.name));
                }
                Some(_) => show_object(runtime, program, word, depth, out),
                None => out.push_str(&format!("value@{word:x}")),
            }
        }
    }
}

/// An object as its source writes it (#756), as on Tomato: a variant case as `Enum.case(payload, …)` or
/// `Enum.case`, any other object as `Type(field: value, …)`. `once let` slots are caches and stay out, as in `==`.
fn show_object(runtime: &Runtime, program: &s::Program, word: Word, depth: usize, out: &mut String) {
    use okkama_runtime_core::testing::{display_name, literal, variant_name, SHOWN_DEPTH};
    let descriptor = heap::payload(runtime.heap.header_of(word));
    let declaration = &program.objects[descriptor as usize];
    // The case from the variant's case text, the enum from its declaration name: a generic instance's case text
    // does not carry the whole name (`Point)>.some` for `app::Maybe<object(app::Point)>`).
    let case = program.objects.iter().find_map(|variant| {
        let case = variant.variant_cases.iter().position(|&case| case == descriptor)?;
        let text = variant.enum_names.get(case)?;
        Some(variant_name(&variant.name, text.rsplit('.').next().unwrap_or(text)))
    });
    let fields = declaration.fields.iter().filter(|field| !field.once).collect::<Vec<_>>();
    match &case {
        Some(case) if fields.is_empty() => {
            out.push_str(case);
            return;
        }
        Some(case) => out.push_str(case),
        None => out.push_str(&display_name(&declaration.name)),
    }
    out.push('(');
    if depth + 1 >= SHOWN_DEPTH && !fields.is_empty() {
        out.push_str("…)");
        return;
    }
    for (position, field) in fields.into_iter().enumerate() {
        if position > 0 {
            out.push_str(", ");
        }
        if case.is_none() {
            out.push_str(&field.name);
            out.push_str(": ");
        }
        let ty = declaration.layout.storage_fields[field.storage as usize];
        let value = runtime.heap.field(word, field.storage as usize);
        // As in `value_equal`: an unbound raw scalar is Tomato's marker, an unbound reference or int the zero word.
        let unbound = match ty {
            s::Type::Float | s::Type::Bool | s::Type::Byte | s::Type::Char => value == crate::word::UNBOUND_RAW,
            _ => (ty.traced() || ty == s::Type::Int) && value == 0,
        };
        if unbound {
            out.push_str("null");
        } else if ty == s::Type::Str {
            out.push_str(&literal(runtime.heap.text(value)));
        } else {
            show_into(runtime, program, value, ty, false, depth + 1, out);
        }
    }
    out.push(')');
}

/// Run the tests table of an SPB 17 program and write the standard report: true when no test failed.
/// Each test runs on a fresh runtime under `limits`.
#[cfg(test)]
pub(crate) fn run_suite(program: s::Program, out: Box<dyn std::io::Write + Send>, limits: crate::budget::Limits)
    -> Result<bool, Error>
{
    suite_tests(&program)?;
    run_verified_suite(&machine::Verified::new(program, Vec::new())?, out, limits)
}

/// [`run_suite`] for a program the loader has already checked: every case gets a fresh runtime over the
/// same verified program, and nothing verifies it again.
pub(crate) fn run_verified_suite(verified: &machine::Verified, out: Box<dyn std::io::Write + Send>,
    limits: crate::budget::Limits) -> Result<bool, Error>
{
    let tests = suite_tests(verified.source())?;
    let names = tests.iter().map(|test| test.name.clone()).collect::<Vec<_>>();
    crate::report_suite(&names, out, |index| {
        let mut runtime = Runtime::new_champ_verified(verified)?;
        runtime.limits = limits.clone();
        if let Some(host) = &mut runtime.host {
            host.test_failures = Some(Vec::new());
        }
        let outcome = runtime.run_function(tests[index].function as usize, crate::STACK_BYTES);
        if let Some(report) = runtime.stats_report() {
            eprint!("{report}");
        }
        let host = runtime.host.as_mut();
        let (mut failures, mut skipped) = host
            .map(|host| (host.test_failures.take().unwrap_or_default(), host.test_skipped.take()))
            .unwrap_or_default();
        if let Err(error) = outcome {
            // Only the skip marker alone ends a skipped case.
            if skipped.is_none() || error.0 != format!("RuntimeError: {SKIPPED}") {
                failures.push(error.0);
                skipped = None;
            }
        }
        Ok(crate::CaseOutcome { failures, skipped })
    })
}

fn suite_tests(program: &s::Program) -> Result<Vec<s::Test>, Error> {
    let tests = program.tests.clone().ok_or_else(|| Error("module has no tests".into()))?;
    if tests.is_empty() {
        return Err(Error("Test run requires at least one test".into()));
    }
    Ok(tests)
}

#[cfg(test)]
#[path = "testing_tests.rs"]
mod tests;
