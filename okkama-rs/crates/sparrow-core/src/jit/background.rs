//! #1147 (step 2): Cranelift's codegen of emitted units on worker threads.
//!
//! The VM thread emits a unit (its CLIF, from the runtime's state) and either compiles it itself or submits it here;
//! a worker runs only `Context::compile` with the JIT's own target, which touches nothing of the runtime. A unit
//! comes back only to the VM thread, which installs it when the function is first called: definitions keep the
//! order a synchronous run gives them. A demand for a unit still queued takes it back and compiles it on the VM
//! thread (no wait for a worker that has not started); one a worker runs is waited for.

use super::Unit;
use crate::Error;
use cranelift_codegen::isa::OwnedTargetIsa;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};

/// Where a submitted unit is when its function is demanded.
pub(super) enum Taken {
    /// Not started: compile it here.
    Queued(Unit),
    /// Compiled by a worker (or its error).
    Done(Result<Unit, Error>),
}

#[derive(Default)]
struct State {
    queue: VecDeque<(usize, Unit)>,
    running: BTreeSet<usize>,
    done: BTreeMap<usize, Result<Unit, Error>>,
    stop: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

pub(super) struct Pool {
    shared: Arc<Shared>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pool").field("workers", &self.workers.len()).finish()
    }
}

impl Pool {
    /// `threads` workers compiling with `isa` (`unoptimized` for units that ask for it).
    pub(super) fn new(threads: usize, isa: OwnedTargetIsa, unoptimized: Option<OwnedTargetIsa>) -> Self {
        let shared = Arc::new(Shared { state: Mutex::new(State::default()), changed: Condvar::new() });
        let workers = (0..threads.max(1))
            .map(|index| {
                let (shared, isa, unoptimized) = (shared.clone(), isa.clone(), unoptimized.clone());
                std::thread::Builder::new()
                    .name(format!("sparrow-compile-{index}"))
                    .spawn(move || work(&shared, &isa, unoptimized.as_ref()))
                    .expect("a compile worker thread")
            })
            .collect();
        Self { shared, workers }
    }

    pub(super) fn submit(&self, function: usize, unit: Unit) {
        let mut state = self.shared.state.lock().expect("compile pool");
        state.queue.push_back((function, unit));
        self.shared.changed.notify_one();
    }

    /// The unit of `function`, back from the pool: still queued (taken out), or done (waited for while a worker
    /// runs it). None when the pool does not hold it.
    pub(super) fn take(&self, function: usize) -> Option<Taken> {
        let mut state = self.shared.state.lock().expect("compile pool");
        if let Some(position) = state.queue.iter().position(|(queued, _)| *queued == function) {
            let (_, unit) = state.queue.remove(position).expect("a queued unit");
            return Some(Taken::Queued(unit));
        }
        loop {
            if let Some(result) = state.done.remove(&function) {
                return Some(Taken::Done(result));
            }
            if !state.running.contains(&function) {
                return None;
            }
            state = self.shared.changed.wait(state).expect("compile pool");
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        {
            let mut state = self.shared.state.lock().expect("compile pool");
            state.stop = true;
            state.queue.clear();
            self.shared.changed.notify_all();
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn work(shared: &Shared, isa: &OwnedTargetIsa, unoptimized: Option<&OwnedTargetIsa>) {
    loop {
        let (function, mut unit) = {
            let mut state = shared.state.lock().expect("compile pool");
            loop {
                if state.stop {
                    return;
                }
                if let Some(job) = state.queue.pop_front() {
                    state.running.insert(job.0);
                    break job;
                }
                state = shared.changed.wait(state).expect("compile pool");
            }
        };
        let target = match unoptimized {
            Some(unoptimized) if unit.unoptimized => &**unoptimized,
            _ => &**isa,
        };
        let symbol = unit.symbol.clone();
        let result = super::codegen_with(target, &mut unit, &symbol).map(|()| unit);
        let mut state = shared.state.lock().expect("compile pool");
        state.running.remove(&function);
        state.done.insert(function, result);
        shared.changed.notify_all();
    }
}
