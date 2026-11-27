//! Process workers emulated in the same instance (#1127; owner's decision 8 of the design
//! `okkama-docs/runtime/sparrow-interpreter-1079.md`): a build without the host (wasm32, the playground) has no
//! processes, so `worker.spawn` runs the worker's entry on a runtime of its own in this instance, at the spawn, as a
//! process worker starts at its spawn. What the language promises of a worker holds:
//!
//! - **The same program, a fresh run.** The worker's runtime loads the same program with fresh globals, `once` slots
//!   and heap; it shares no object with the parent. Its standard input is empty, as a process worker's.
//! - **Only values cross.** The receiver's fields and the arguments are encoded in the parent and decoded in the
//!   worker's runtime by the same codec as a process worker's (`worker_wire`), and the result comes back the same
//!   way: an object, closure, Future or handle is refused at the spawn with `spawn cannot transfer …`.
//! - **A failure comes back as text**: the worker's report, a runtime failure of the awaiting parent.
//! - **Output is passed on whole**, when the parent takes the result (the worker Future's driver runs), before it.
//! - **Cancellation**: a worker cancelled before the parent took its result reports nothing; its output is dropped.
//!
//! Where process workers are nondeterministic, the emulation is one valid run of them: each worker finishes at its
//! spawn, so the workers' outputs reach the parent in the order their Futures' drivers run (spawn order, in the
//! scheduler's queue). The worker's runtime runs on the parent's engine under the parent's budgets (each worker
//! its own steps, memory and output; the same interrupt flag). A bound action keeps its receiver fields
//! (`worker.bind`); a pool reuses nothing, since every spawn is a fresh runtime anyway.
// The spawning instruction's context is a raw pointer: its stack limit is read once (`spawn`).
#![allow(unsafe_code)]
use crate::drivers::Step;
use crate::heap::Payload;
use crate::primitives::State;
use crate::runtime::{Ctx, FailureKind, Runtime};
use crate::word::{Repr, UNBOUND, Word};
use crate::worker_wire::{Reader, encode};
use okkama_sparrow::TypeRef;

/// The receiver fields a bound action (`worker.bind`) passes to its worker entry ahead of the call's arguments.
#[cfg_attr(feature = "host", allow(dead_code))]
pub fn bind(runtime: &mut Runtime, action: Word, entry: Word, fields: Vec<(Word, Repr)>) -> Word {
    let bound = runtime.closure_copy(action);
    runtime.state_new(bound, State::WorkerBinding { entry, fields, pool: None });
    bound
}

/// A worker pool reuses nothing here: every spawn is a fresh runtime. The receiver's own action.
#[cfg_attr(feature = "host", allow(dead_code))]
pub(crate) fn pool_bind(runtime: &mut Runtime, receiver: Word) -> Result<Word, String> {
    Ok(runtime.heap.field(receiver, 0))
}

/// No pool holds anything to close.
#[cfg_attr(feature = "host", allow(dead_code))]
pub(crate) fn pool_close(_runtime: &mut Runtime, _frame: Word, _receiver: Word) -> Step {
    Step::Done(UNBOUND)
}

/// What an emulated worker reported, kept in its Future's driver slot until the parent takes it.
#[derive(Debug)]
struct Report {
    /// The encoded result, or the failure text.
    outcome: Result<Vec<u8>, String>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    result: Option<TypeRef>,
    /// Has the driver run once? A spawn runs its Future's driver at once; the report waits for the scheduler's next
    /// round, as a process worker's reply comes after the parent goes on.
    started: bool,
}

impl Payload for Report {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// `worker.spawn`: encode the call, run the worker now on a runtime of its own, and return the Future that hands
/// its report over. `direct` calls `entry` itself; otherwise `entry` is a bound action whose receiver fields
/// precede `arguments`.
pub fn spawn(runtime: &mut Runtime, ctx: *mut Ctx, entry: Word, arguments: &[Word], direct: bool) -> Result<Word, String> {
    let (entry, arguments) = if direct {
        (entry, arguments.to_vec())
    } else {
        let Some(State::WorkerBinding { entry, fields, .. }) = runtime.states.get(&entry) else {
            return Err("spawn requires a synchronous bound object action whose receiver fields reconstruct it".into());
        };
        let mut values: Vec<Word> = fields.iter().map(|&(w, _)| w).collect();
        values.extend_from_slice(arguments);
        (*entry, values)
    };
    let function = crate::heap::payload(runtime.heap.header_of(entry)) as usize;
    let source = &runtime.loaded.program.functions[function];
    if source.params.len() != arguments.len() {
        return Err("worker entry cannot capture VM values".into());
    }
    let types: Vec<TypeRef> = source.params.iter().map(|r| source.registers[r.0 as usize]).collect();
    let result = source.result;
    let mut request = Vec::new();
    for (&value, &ty) in arguments.iter().zip(&types) {
        encode(runtime, &mut request, value, ty)?;
    }
    let driver = runtime.loaded.worker(result).ok_or_else(|| "worker.spawn without a worker Future type".to_owned())?;
    // SAFETY: the instruction's context, live for the call.
    let limit = unsafe { (*ctx).stack_limit };
    let report = work(runtime, limit, function, &types, result, &request);
    let future = runtime.future_new(driver, &[]);
    let object = runtime.heap.allocate_payload(Box::new(report));
    let slot = runtime.driver_slot(future);
    crate::tasks::set(future, slot, object);
    runtime.spawn(ctx, future);
    Ok(future)
}

/// The worker's run: a fresh runtime of the same program on the parent's engine and budgets, the arguments decoded
/// into it, its output captured. `limit` is the parent's stack limit, which the worker's run keeps.
fn work(parent: &Runtime, limit: u64, function: usize, types: &[TypeRef], result: Option<TypeRef>, request: &[u8]) -> Report {
    let report = |outcome, stdout, stderr| Report { outcome, stdout, stderr, result, started: false };
    let mut runtime = match Runtime::new(parent.loaded.clone()) {
        Ok(runtime) => Box::new(runtime),
        Err(error) => return report(Err(error.0), Vec::new(), Vec::new()),
    };
    if parent.interp.is_some() {
        runtime.use_engine(crate::Engine::Interpreter);
    }
    runtime.limits = parent.limits.clone();
    if let Some(host) = runtime.host.as_mut() {
        host.set_stdin(Vec::new());
    }
    let mut reader = Reader::new(request);
    let mut arguments = Vec::with_capacity(types.len());
    for &ty in types {
        match reader.value(&mut runtime, ty) {
            Ok(value) => arguments.push(value),
            Err(message) => return report(Err(message), Vec::new(), Vec::new()),
        }
    }
    if let Err(message) = reader.finish() {
        return report(Err(message), Vec::new(), Vec::new());
    }
    let marker = 0u8;
    let stack = (&marker as *const u8 as u64).saturating_sub(limit).saturating_add(crate::runtime::STACK_HEADROOM);
    let (value, stdout, stderr) =
        okkama_runtime_core::stdout::capture_streams(|| runtime.run_function_with(function, &arguments, stack));
    let outcome = value.map_err(|error| error.0).and_then(|value| {
        let mut bytes = Vec::new();
        match result {
            Some(ty) => encode(&runtime, &mut bytes, value, ty)?,
            None => bytes.push(0),
        }
        Ok(bytes)
    });
    report(outcome, stdout, stderr)
}

/// The driver of an emulated worker's Future: at its first run (the spawn) wait for the scheduler's next round; then
/// hand the report over (its output first, then the result or the failure). `deliver` cancels it, and a cancelled
/// worker reports nothing.
pub fn drive(runtime: &mut Runtime, frame: Word, deliver: bool) -> Step {
    if deliver {
        return Step::Failed(runtime.cancelled_failure());
    }
    let object = crate::tasks::get(frame, runtime.driver_slot(frame));
    let report = runtime.heap.payload_mut::<Report>(object).expect("worker report");
    if !std::mem::replace(&mut report.started, true) {
        // The scheduler skips the frame if a cancellation settles it first.
        runtime.ready.push_back(frame);
        return Step::Pending;
    }
    let outcome = std::mem::replace(&mut report.outcome, Ok(Vec::new()));
    let (stdout, stderr, result) = (std::mem::take(&mut report.stdout), std::mem::take(&mut report.stderr), report.result);
    let _ = okkama_runtime_core::stdout::write(&stdout);
    let _ = okkama_runtime_core::stdout::write_error(&stderr);
    let decoded = outcome.and_then(|bytes| {
        let mut reader = Reader::new(&bytes);
        let value = match result {
            Some(ty) => reader.value(runtime, ty)?,
            None => {
                if reader.take(1)?[0] != 0 {
                    return Err("invalid worker response value".into());
                }
                UNBOUND
            }
        };
        reader.finish()?;
        Ok(value)
    });
    match decoded {
        Ok(value) => Step::Done(value),
        Err(message) => Step::Failed(runtime.failure_new(FailureKind::Program(message))),
    }
}
