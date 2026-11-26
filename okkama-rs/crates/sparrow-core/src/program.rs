//! Static facts about a loaded SPB program: the machine representation of every
//! register and field and the heap layouts. The lazily filled code table lives
//! with the runtime (`runtime.rs`).
use crate::heap::Layouts;
use crate::word::Repr;
use okkama_sparrow::{Program, Type, TypeRef};

/// Machine kind of a register: how the JIT holds it and how the collector sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Tagged small integer or BigInt pointer.
    Int,
    /// F64 value; stored as raw bits.
    Float,
    /// bool, char, payload-less enums.
    Raw,
    /// byte: raw bits in registers; arrays of bytes are packed (see `heap`).
    Byte,
    /// Heap object pointer.
    Ref,
}

impl Kind {
    pub fn of(program: &Program, ty: TypeRef) -> Self {
        match program.ty(ty) {
            Some(Type::Int) => Self::Int,
            Some(Type::Float) => Self::Float,
            // Payload-less enums are `(enum << 32) | case`.
            Some(Type::Byte) => Self::Byte,
            Some(Type::Bool | Type::Char | Type::Enum(_)) => Self::Raw,
            _ => Self::Ref,
        }
    }
    pub fn repr(self) -> Repr {
        match self {
            Self::Int => Repr::Int,
            Self::Float | Self::Raw | Self::Byte => Repr::Raw,
            Self::Ref => Repr::Ref,
        }
    }
    /// Values the collector must see in stack maps.
    pub fn traced(self) -> bool {
        matches!(self, Self::Int | Self::Ref)
    }
}

#[derive(Debug, PartialEq)]
pub struct FunctionInfo {
    pub kinds: Vec<Kind>,
    /// Proven pointer-free for every definition of this SPB register (#535).
    pub(crate) pointer_free: Vec<bool>,
    pub result: Option<Kind>,
    /// See [`is_driver`].
    pub driver: bool,
    /// Byte offset of the Future's inline ownership identity (serial, owner) in this function's frame, after the
    /// retained arguments (#931). Only programs with resources (a type with `destroy`) have it; others keep 0.5.8's
    /// frame size.
    pub identity_at: Option<u64>,
    /// Byte offset of the Future's ownership regions (OWNED, then INPUTS) in this function's frame, behind the
    /// registers and the driver slot (#932). Only programs with resources have them.
    pub held_at: Option<u64>,
}

#[derive(Debug)]
pub struct Loaded {
    pub program: Program,
    pub functions: Vec<FunctionInfo>,
    /// Per descriptor: is it an Event cell (created by `Event.new`)?
    events: Vec<bool>,
    /// Worker Future functions by result type (see [`WORKER`]).
    workers: Vec<(Option<TypeRef>, usize)>,
    /// Native process launches can leave descendants outside VM ownership.
    /// Keep spawn's per-item process-group teardown for such programs.
    pub(crate) worker_reuse_safe: bool,
}

/// Intrinsic name of the synthetic async functions whose frames are the
/// Futures of `worker.spawn`, one per result type (`worker` drives them).
pub const WORKER: &str = "sparrow.worker.process";

/// Append a worker Future function for every result type `worker.spawn`
/// produces in `program`.
fn add_workers(program: &mut Program) -> Vec<(Option<TypeRef>, usize)> {
    let mut results = Vec::new();
    for function in &program.functions {
        for instruction in &function.code {
            if let okkama_sparrow::Instruction::WorkerSpawn { dst, .. } = instruction
                && let Some(Type::Future(result)) = program.ty(function.registers[dst.0.0 as usize])
                    && !results.contains(result) {
                        results.push(*result);
                    }
        }
    }
    // #588: a session's published program already holds the workers of earlier extensions; they keep their index.
    let existing: Vec<(Option<TypeRef>, usize)> = program.functions.iter().enumerate()
        .filter(|(_, f)| f.intrinsic.as_ref().is_some_and(|i| i.name == WORKER) && f.code.is_empty())
        .map(|(index, f)| (f.result, index))
        .collect();
    results
        .into_iter()
        .map(|result| {
            if let Some(&(_, index)) = existing.iter().find(|(r, _)| *r == result) {
                return (result, index);
            }
            program.functions.push(okkama_sparrow::Function {
                symbol: format!("$worker#{}", program.functions.len()),
                is_async: true,
                native: None,
                intrinsic: Some(okkama_sparrow::Intrinsic { name: WORKER.into(), objects: Vec::new(), helpers: Vec::new() }),
                registers: Vec::new(),
                params: Vec::new(),
                result,
                code: Vec::new(),
                handlers: Vec::new(),
            });
            (result, program.functions.len() - 1)
        })
        .collect()
}

/// An async native or intrinsic function: its Future is a frame run by a Rust
/// driver (`drivers`), which keeps its state in one extra traced word.
pub fn is_driver(function: &okkama_sparrow::Function) -> bool {
    function.is_async && (function.native.is_some() || function.intrinsic.is_some())
}

impl Loaded {
    /// A loaded program, verified first (#1009): the analyses here and the JIT
    /// index registers and pick machine representations by the program's
    /// types, so an unverified program could crash or corrupt memory.
    pub fn verified(program: Program) -> Result<Self, crate::Error> {
        program.verify().map_err(|error| crate::Error(error.to_string()))?;
        Ok(Self::new(program))
    }

    /// For a program the crate has verified (`decode` verifies).
    pub(crate) fn new(mut program: Program) -> Self {
        let workers = add_workers(&mut program);
        // One switch for both JIT roots and suspended-frame layouts. Read it
        // once at load time; never change a live runtime's tracing contract.
        let pointer_free = crate::env_switch::enabled_or("SPARROW_CORE_POINTER_FREE", true);
        let int_ranges = crate::env_switch::enabled_or("SPARROW_CORE_INT_RANGE", true);
        let profile = crate::env_switch::enabled("SPARROW_CORE_PROFILE_COMPILE");
        let resources = program.objects.iter().any(|o| o.destroy.is_some());
        let functions = program
            .functions
            .iter()
            .map(|f| {
                let kinds: Vec<Kind> = f.registers.iter().map(|t| Kind::of(&program, *t)).collect();
                // This work happens at load time, outside jit::compile's timer.
                let started = profile.then(std::time::Instant::now);
                let pointer_free = if !pointer_free {
                    vec![false; kinds.len()]
                } else if int_ranges {
                    crate::pointer_free::analyze(f, &kinds, &program)
                } else {
                    crate::pointer_free::analyze_with_ranges(f, &kinds, &program, false)
                };
                if let Some(started) = started {
                    eprintln!("; pointer-free analysis\t{}\t{} microseconds", f.symbol, started.elapsed().as_micros());
                }
                let driver = is_driver(f);
                // header, fixed words, registers, the driver slot; then, with resources, the ownership regions,
                // the retained arguments and the identity
                let held_at = resources.then(|| (1 + crate::tasks::FIXED_WORDS + kinds.len() + usize::from(driver)) as u64 * 8);
                let identity_at = held_at.map(|at| at + 16 + f.params.len() as u64 * 8);
                FunctionInfo { kinds, pointer_free, result: f.result.map(|t| Kind::of(&program, t)), driver, identity_at, held_at }
            })
            .collect();
        // A cell is what its constructor intrinsic makes, whatever its name.
        let mut events = vec![false; program.objects.len()];
        for f in &program.functions {
            if let Some(intrinsic) = &f.intrinsic
                && matches!(intrinsic.name.as_str(), "okkama.event.Event.new" | "okkama.event.Event.new_ready")
                    && let Some(object) = intrinsic.objects.first() {
                        events[object.0 as usize] = true;
                    }
        }
        let worker_reuse_safe = !program.functions.iter().any(|function| matches!(
            function.native.as_deref().and_then(okkama_runtime_abi::Native::resolve),
            Some(okkama_runtime_abi::Native::ProcessProcessStateStart | okkama_runtime_abi::Native::ProcessPipelineStateCapture)
        ));
        Self { program, functions, events, workers, worker_reuse_safe }
    }

    /// The worker Future function for results of type `result`.
    pub fn worker(&self, result: Option<TypeRef>) -> Option<usize> {
        self.workers.iter().find(|(r, _)| *r == result).map(|&(_, function)| function)
    }

    /// Is `descriptor` an Event cell (the object `Event.new` creates)?
    pub fn is_event(&self, descriptor: usize) -> bool {
        self.events.get(descriptor).copied().unwrap_or(false)
    }

    /// The value kind of an `Atomic<T>` descriptor (from its constructor).
    pub fn atomic_value(&self, descriptor: usize) -> Option<Kind> {
        let p = &self.program;
        p.functions.iter().zip(&self.functions).find_map(|(f, info)| {
            let intrinsic = f.intrinsic.as_ref()?;
            (intrinsic.name == "okkama.atomic.Atomic.new_value"
                && intrinsic.objects.first().is_some_and(|o| o.0 as usize == descriptor))
            .then(|| f.params.first().map(|p| info.kinds[p.0 as usize]))
            .flatten()
        })
    }

    /// Field representations for the collector. Async frames keep every
    /// register after the fixed frame words, an optional driver slot, and
    /// retained arguments independent of reused registers (see `tasks`).
    pub fn layouts(&self) -> Layouts {
        let p = &self.program;
        Layouts {
            objects: p
                .objects
                .iter()
                .enumerate()
                .map(|(descriptor, o)| match self.atomic_value(descriptor) {
                    // An Atomic cell: its value, then the closed flag (see `intrinsics`).
                    Some(kind) => vec![kind.repr(), Repr::Raw],
                    // An Event: `initial`, then the set flag and its waiters (`primitives`).
                    None if self.is_event(descriptor) => vec![Repr::Raw, Repr::Raw, Repr::Ref],
                    None => o.fields.iter().map(|(_, t)| Kind::of(p, *t).repr()).collect(),
                })
                .collect(),
            // A closure object has one word per parameter of its function:
            // captures fill a prefix, the rest stay unbound.
            closures: p
                .functions
                .iter()
                .zip(&self.functions)
                .map(|(f, info)| {
                    f.params
                        .iter()
                        .map(|r| info.kinds[r.0 as usize].repr())
                        .collect()
                })
                .collect(),
            frames: self
                .functions
                .iter()
                .zip(&p.functions)
                .map(|(f, source)| {
                    // state, result, failure, waiter, awaiting, resume, flags
                    let mut layout = vec![
                        Repr::Raw,
                        f.result.map_or(Repr::Raw, Kind::repr),
                        Repr::Ref,
                        Repr::Ref,
                        Repr::Ref,
                        Repr::Raw,
                        Repr::Raw,
                    ];
                    layout.extend(f.kinds.iter().zip(&f.pointer_free).map(|(k, free)| if *free { Repr::Raw } else { k.repr() }));
                    if f.driver {
                        layout.push(Repr::Ref);
                    }
                    // The ownership regions, the retained arguments and the identity serve resources only: a
                    // program without resources has none of them (#932).
                    if f.identity_at.is_some() {
                        layout.extend([Repr::Ref, Repr::Ref]); // owned, inputs (#932)
                        layout.extend(source.params.iter().map(|param| f.kinds[param.0 as usize].repr()));
                        layout.extend([Repr::Raw, Repr::Raw]); // identity serial and owner (#931)
                    }
                    layout
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod worker_tests {
    use super::*;

    /// #588: a program that already holds the worker of a result type (a session's published program) keeps it at
    /// its index; loading adds workers only for result types without one.
    #[test]
    fn loading_reuses_the_workers_a_program_already_has() {
        let program = okkama_sparrow::assemble(
            "sparrow 18\nmodule \"workers\" \"1.0.0\"\ntype t0 int\ntype t1 fn [] -> t0\ntype t2 future t0\n\
             function f0 \"work\" {\n registers [t0]\n params []\n result t0\n 0: const r0, int 1\n 1: ret r0\n}\n\
             function f1 \"spawner\" async {\n registers [t1, t2]\n params []\n result t2\n 0: fn.closure r0, f0, []\n 1: worker.spawn r1, r0, [], false\n 2: ret r1\n}\n",
        ).unwrap();
        let first = Loaded::new(program);
        assert_eq!(first.program.functions.len(), 3, "one worker added");
        assert_eq!(first.workers.len(), 1);
        let again = Loaded::new(first.program.clone());
        assert_eq!(again.program.functions, first.program.functions, "the existing worker is reused");
        assert_eq!(again.workers, first.workers);
    }
    #[test]
    fn worker_reuse_excludes_programs_that_can_launch_descendants() {
        use okkama_runtime_abi::Native;
        let program = okkama_sparrow::assemble(
            "sparrow 18\nmodule \"reuse\" \"1.0.0\"\ntype t0 int\nfunction f0 \"main\" {\nregisters [t0]\nparams []\nresult t0\n0: const r0, int 0\n1: ret r0\n}\n"
        ).unwrap();
        assert!(Loaded::new(program.clone()).worker_reuse_safe);
        for (native, safe) in [
            (Native::ProcessProcessCurrentId, true),
            (Native::ProcessProcessArguments, true),
            (Native::ProcessProcessStateStart, false),
            (Native::ProcessPipelineStateCapture, false),
        ] {
            // Classification alone; this synthetic native is never executed.
            let mut program = program.clone();
            program.functions[0].native = Some(native.symbol().into());
            assert_eq!(Loaded::new(program).worker_reuse_safe, safe);
        }
    }

}
