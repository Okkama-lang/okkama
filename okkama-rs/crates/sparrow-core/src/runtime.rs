//! Runtime state shared with machine code, runtime helpers called from it, and
//! root enumeration for the collector.
//!
//! Generated code receives a [`Ctx`] pointer. A failure sets `status` (and
//! `failure`), and every caller checks it after each call or helper, so
//! failures unwind through machine frames without Rust unwinding. Helpers that
//! may allocate collect at their end when the heap asks for it; roots are the
//! machine frames above the helper (found through frame records and Cranelift
//! stack maps), runtime constants and the context.
#![allow(unsafe_code)]

use crate::Error;
use crate::heap::{self, Heap, Integer, Payload};
use crate::program::{Kind, Loaded};
use crate::budget::Exhausted;
use crate::word::{Repr, UNBOUND, Word};
use num_bigint::BigInt;
use num_traits::{ToPrimitive, Zero};

pub const OK: u64 = 0;
/// A language failure is pending in `Ctx::failure`.
pub const FAILED: u64 = 1;
/// The engine stopped (unsupported construct, internal invariant).
pub const FATAL: u64 = 2;
/// A budget ran out or the host interrupted the run (`budget`); like
/// `FATAL`, it unwinds every activation without handlers.
pub const EXHAUSTED: u64 = 3;
/// Stack kept below the limit machine code checks: room for one frame
/// (`jit::MAX_FRAME`) and the Rust helpers it calls.
pub const STACK_HEADROOM: u64 = 1 << 20;

/// Shared with machine code; offsets come from `offset_of!`.
#[repr(C)]
#[derive(Debug)]
pub struct Ctx {
    pub status: u64,
    pub stack_limit: u64,
    pub failure: Word,
    pub runtime: *mut Runtime,
    /// Frame pointer of the Rust function that entered machine code; the
    /// collector's frame walk ends there.
    pub entry_frame: u64,
    /// Second result of helpers that return a flag (a found map value).
    pub scratch: Word,
}

/// A language failure value: what failed, where it passed, what caused it.
#[derive(Debug)]
pub struct Failure {
    pub kind: FailureKind,
    /// (function, pc) of each activation it unwound through, innermost first;
    /// at most `TRACE_LIMIT` of them, the rest only counted (#1009). Past the
    /// limit the last `TRACE_TAIL` slots are a ring of the outermost
    /// activations seen so far (#1034): read it through [`Failure::sites`].
    pub trace: Vec<(u32, u32)>,
    /// Activations between the head and the tail of the trace, not recorded.
    pub trace_omitted: u64,
    pub cause: Option<Word>,
    pub secondary: Vec<Word>,
    last_frame: Option<(u32, u64)>,
}
#[derive(Debug)]
pub enum FailureKind {
    Cancelled,
    /// A `NativeFailure` object from a provider.
    Native(Word),
    Thrown(Word),
    Program(String),
    Panic(String),
}
impl Payload for Failure {
    fn trace(&self, visit: &mut dyn FnMut(Word, Repr)) {
        if let FailureKind::Thrown(value) | FailureKind::Native(value) = self.kind {
            visit(value, Repr::Ref);
        }
        for &word in self.cause.iter().chain(&self.secondary) {
            visit(word, Repr::Ref);
        }
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// #588: the capacities of a session runtime: functions (the invocation table) and object descriptors (the role
/// dispatch tables).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capacities {
    pub functions: usize,
    pub objects: usize,
}

/// One stable invocation record per function, shared with generated static,
/// role and async calls. Only `code` changes, after code/maps publication.
/// Captures are a per-closure prefix, not a per-function count: the same
/// function may be closed over different prefixes with different fn types.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Invocation {
    pub code: u64,
    pub params: u32,
    pub is_async: bool,
}

impl Invocation {
    fn capture_count(self, count: u64) -> Result<usize, &'static str> {
        if count > u64::from(self.params) {
            return Err("closure argument count exceeds parameter count");
        }
        // count <= params < 2^32, before conversion/subtraction.
        Ok(self.params as usize - count as usize)
    }
}

#[derive(Debug)]
pub struct Runtime {
    pub heap: Heap,
    pub loaded: Loaded,
    #[cfg(feature = "jit")]
    pub jit: crate::jit::Jit,
    /// Stable allocation embedded in machine code; code is 0 until compiled.
    pub invocations: Box<[Invocation]>,
    /// #588: the capacities of a session runtime (`Runtime::session`), which `extend` grows within.
    capacities: Option<Capacities>,
    /// #588: the session region (what the REPL's variables own) and the values the last cell kept; GC roots.
    session: (Option<Word>, Option<Word>),
    /// Same-binary diagnostic control, read once at runtime construction.
    pub(crate) closure_bridge_reference: bool,
    /// Constants embedded in machine code; always roots.
    pub constants: Vec<(Word, Repr)>,
    /// Values held by Rust while machine code runs (entry arguments, results).
    pub roots: Vec<(Word, Repr)>,
    /// `const` arrays and maps (#153), built at their first read; each is
    /// then also in `constants`, so it stays a root.
    pub globals: Vec<Option<Word>>,
    pub fatal: Option<String>,
    /// The last failure `describe` reported, as the OCS protocol sees it.
    pub uncaught: Option<crate::Uncaught>,
    pub resources: crate::failures::Resources,
    /// Async frames ready to resume; spawned tasks; frames running on the
    /// machine stack (innermost last). All are roots.
    pub ready: std::collections::VecDeque<Word>,
    pub spawned: Vec<Word>,
    pub running: Vec<Word>,
    /// Sleeping drivers by deadline; roots.
    pub timers: Vec<(crate::clock::Instant, Word)>,
    /// Async primitive state by owning object (see `primitives`).
    pub states: crate::word::FastMap<Word, crate::primitives::State>,
    /// Native provider of each function, resolved once.
    pub natives: Vec<Option<okkama_runtime_abi::Native>>,
    /// Intrinsic of each function, resolved once.
    pub intrinsics: Vec<Option<crate::intrinsics::Intrinsic>>,
    /// Objects whose state the runtime holds, by descriptor (#956): `==` compares them by identity.
    pub(crate) held: Vec<bool>,
    /// Native provider state; taken out while a provider runs.
    pub(crate) host: Option<Box<crate::host::Heap>>,
    pub io: crate::io::Io,
    /// Event counts by name when `SPARROW_CORE_STATS` is set (diagnostics).
    pub stats: Option<Box<std::collections::HashMap<String, u64>>>,
    /// When this runtime was made: `--stats` reports the run's wall time, so
    /// machine code ≈ run − compile − collection (#55).
    created: crate::clock::Instant,
    /// The command (`sparrow`, FILE.spb) process workers run; empty
    /// when the embedder configured none.
    pub worker_command: Vec<std::ffi::OsString>,
    /// Budgets of the next run (`budget::Limits`); set before it starts.
    pub limits: crate::budget::Limits,
    /// A native panicked: the runtime's state is unknown and it runs nothing more.
    pub poisoned: bool,
    pub(crate) budget: Budget,
    /// #1120: the interpreter, when `SPARROW_CORE_INTERPRET` chose it at construction; then no function is ever
    /// compiled, every code address stays 0, and the collector finds the roots of active frames in its register
    /// stack instead of machine frames. One engine per runtime: stacks are never mixed.
    pub(crate) interp: Option<Box<crate::interp::Interp>>,
}

/// Budget state of the current run.
#[derive(Debug, Default)]
pub(crate) struct Budget {
    pub exhausted: Option<Exhausted>,
    steps_left: Option<u64>,
    /// Size of the slice given at the last refill.
    slice: u64,
    meter: Option<crate::budget::Meter>,
    pub written: u64,
    /// Steps left in the current slice; machine code polls below zero.
    pub fuel: crate::budget::Fuel,
    /// The memory budget caps the machine stack: overflowing it is running
    /// out of memory (#241).
    pub stack_capped: bool,
    /// #1129: whether code polls the fuel cell (entries and back-edges), fixed at this runtime's first run. The
    /// JIT compiles it into each function and the interpreter reads it at each step, so both take it from here.
    pub polled: Option<bool>,
}

/// Frame pointer of the calling function's own frame record. The JIT's root walk
/// follows frame records from a helper; without the JIT nothing reads it (0, #1124:
/// the `asm!` exists for aarch64 and x86_64 only).
#[cfg(feature = "jit")]
macro_rules! frame_pointer {
    () => {{
        let frame: u64;
        // SAFETY: reads the frame pointer register of the current function.
        #[cfg(target_arch = "aarch64")]
        unsafe {
            std::arch::asm!("mov {}, x29", out(reg) frame)
        };
        #[cfg(target_arch = "x86_64")]
        unsafe {
            std::arch::asm!("mov {}, rbp", out(reg) frame)
        };
        frame
    }};
}
#[cfg(not(feature = "jit"))]
macro_rules! frame_pointer {
    () => {
        0u64
    };
}

impl Runtime {
    /// A runtime for a loaded program. A `Loaded` is verified by construction
    /// outside the crate (`Loaded::verified`, #1009).
    pub fn new(loaded: Loaded) -> Result<Self, Error> {
        crate::contracts::check(&loaded)?;
        Self::assemble(loaded)
    }

    /// #588: a runtime that later programs may extend (`extend`), within `capacities`. Machine code embeds the
    /// invocation table's base and each role dispatch table's base, so both are allocated at their capacity now
    /// and never relocated.
    pub fn session(mut loaded: Loaded, capacities: Capacities) -> Result<Self, Error> {
        crate::contracts::check(&loaded)?;
        loaded.check_every_field();
        let functions = loaded.program.functions.len();
        if functions > capacities.functions || loaded.program.objects.len() > capacities.objects {
            return Err(Error("the program exceeds the session's capacities".into()));
        }
        let mut runtime = Self::assemble(loaded)?;
        let empty = Invocation { code: 0, params: 0, is_async: false };
        let mut invocations = vec![empty; capacities.functions];
        invocations[..functions].copy_from_slice(&runtime.invocations);
        runtime.invocations = invocations.into_boxed_slice();
        #[cfg(feature = "jit")]
        {
            runtime.jit.object_capacity = capacities.objects;
        }
        runtime.capacities = Some(capacities);
        Ok(runtime)
    }

    /// #1121: run on `engine` from now on, whatever `SPARROW_CORE_INTERPRET` said; before anything ran (a runtime
    /// never mixes the two engines' frames).
    pub fn use_engine(&mut self, engine: crate::Engine) {
        debug_assert!(self.invocations.iter().all(|entry| entry.code == 0), "an engine is chosen before code runs");
        self.interp = match engine {
            #[cfg(feature = "jit")]
            crate::Engine::Jit => None,
            crate::Engine::Interpreter => {
                // #1122: its frames trace every register; nothing has been allocated with the old layouts' frames.
                self.loaded.without_pointer_free();
                self.heap.relayout(self.loaded.layouts());
                Some(Box::new(crate::interp::Interp::new(self.loaded.program.functions.len())))
            }
        };
    }

    /// A new closure of `closure`'s function over the same captures (`worker.bind`).
    pub(crate) fn closure_copy(&mut self, closure: Word) -> Word {
        let function = heap::payload(self.heap.header_of(closure));
        let words = self.heap.layouts().closures[function as usize].len();
        let captures: Vec<Word> = (0..words).map(|i| self.heap.field(closure, i)).collect();
        self.heap.closure_new(function, &captures)
    }

    /// The root Future of an async entry leaves the GC roots once its run is over.
    fn drop_root(&mut self, root: Word) {
        if let Some(index) = self.roots.iter().rposition(|&(word, _)| word == root) {
            self.roots.remove(index);
        }
    }

    /// #588: a cell's kept values become the session's: what the previous kept values own and `kept` still
    /// reaches moves to a new session region, what the cell made and `kept` reaches is adopted, and the old
    /// region closes with what no variable keeps any more (dropped and rebound variables), destroys in their
    /// usual order. `kept` stays rooted until the next call. A destroy's failure is returned after all of them ran.
    pub fn session_keep(self: &mut Box<Self>, kept: Word) -> Result<(), Error> {
        self.roots.push((kept, Repr::Ref));
        let region = self.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + crate::failures::REGION_SLOTS]);
        self.roots.pop();
        let old = self.session.0.replace(region);
        self.session.1 = Some(kept);
        if let Some(old) = old {
            self.region_transfer(old, region, kept);
        }
        self.region_add(region, kept, true);
        match old {
            Some(old) => self.close_session_region(old),
            None => Ok(()),
        }
    }

    /// #588: `:reset` and exit: the session region closes; every value it owns is destroyed once.
    pub fn session_close(self: &mut Box<Self>) -> Result<(), Error> {
        self.session.1 = None;
        match self.session.0.take() {
            Some(region) => self.close_session_region(region),
            None => Ok(()),
        }
    }

    fn close_session_region(self: &mut Box<Self>, region: Word) -> Result<(), Error> {
        let marker = 0u8;
        let mut ctx = Ctx {
            status: OK,
            stack_limit: (&marker as *const u8 as u64).saturating_sub(crate::STACK_BYTES.saturating_sub(STACK_HEADROOM)),
            failure: UNBOUND,
            runtime: &mut **self,
            entry_frame: frame_pointer!(),
            scratch: UNBOUND,
        };
        self.roots.push((region, Repr::Ref));
        let failure = self.region_close(&mut ctx, region, &[], None);
        self.roots.pop();
        match failure {
            Some(failure) => Err(Error(self.describe(&mut ctx, failure))),
            None if ctx.status == OK => Ok(()),
            None => Err(Error(self.fatal.clone().unwrap_or_else(|| "engine failure".into()))),
        }
    }

    /// #588: extend a session runtime with `program`, which must begin with the loaded program: every published
    /// function, object, type, role, variant, cap and global, its function metadata, its trace layouts and its
    /// ownership metadata unchanged; only new entries after them. Prepared on copies, then committed by steps that
    /// cannot fail, so a rejected extension leaves the runtime exactly as it was. Compiled code keeps working:
    /// it refers only to published indices, through tables that are never relocated.
    pub fn extend(&mut self, program: okkama_sparrow::Program) -> Result<(), Error> {
        let Some(capacities) = self.capacities else {
            return Err(Error("only a session runtime can be extended".into()));
        };
        if self.poisoned {
            return Err(Error("internal error: the runtime is unusable after a panic in a native".into()));
        }
        // ---- prepare: nothing live changes ----
        program.verify().map_err(|error| Error(error.to_string()))?;
        let mut loaded = Loaded::new(program);
        // #1122: the interpreter's runtime loaded without the pointer-free proofs, and so does its extension, or
        // its function metadata and frame layouts would differ from the published ones in every function.
        if self.interp.is_some() {
            loaded.without_pointer_free();
        }
        loaded.check_every_field();
        crate::contracts::check(&loaded)?;
        let (old, new) = (&self.loaded.program, &loaded.program);
        let prefix = |what: &str, same: bool| if same { Ok(()) } else {
            Err(Error(format!("the extension changes published {what}")))
        };
        prefix("types", new.types.starts_with(&old.types))?;
        prefix("enums", new.enums.starts_with(&old.enums))?;
        prefix("roles", new.roles.starts_with(&old.roles))?;
        prefix("objects", new.objects.starts_with(&old.objects))?;
        prefix("variants", new.variants.starts_with(&old.variants))?;
        prefix("handles", new.handles.starts_with(&old.handles))?;
        prefix("caps", new.caps.starts_with(&old.caps))?;
        prefix("globals", new.globals.starts_with(&old.globals))?;
        prefix("functions", new.functions.starts_with(&old.functions))?;
        // Ownership first: turning resources on also changes every frame's layout, and this is the reason to say.
        let resources = crate::failures::Resources::new(new);
        prefix("ownership metadata", self.resources.metadata_prefix_of(&resources))?;
        prefix("function metadata", loaded.functions.starts_with(&self.loaded.functions))?;
        let layouts = loaded.layouts();
        let published = self.heap.layouts();
        prefix("object layouts", layouts.objects.starts_with(&published.objects))?;
        prefix("closure layouts", layouts.closures.starts_with(&published.closures))?;
        prefix("frame layouts", layouts.frames.starts_with(&published.frames))?;
        let (functions, objects) = (new.functions.len(), new.objects.len());
        if functions > capacities.functions {
            return Err(Error(format!("the session holds at most {} functions", capacities.functions)));
        }
        if objects > capacities.objects {
            return Err(Error(format!("the session holds at most {} object types", capacities.objects)));
        }
        let from = old.functions.len();
        let records = new.functions[from..].iter().map(|f| {
            Ok(Invocation {
                code: 0,
                params: u32::try_from(f.params.len()).map_err(|_| Error("too many function parameters".into()))?,
                is_async: f.is_async,
            })
        }).collect::<Result<Vec<_>, Error>>()?;
        let natives: Vec<_> = new.functions[from..].iter()
            .map(|f| f.native.as_deref().and_then(okkama_runtime_abi::Native::resolve)).collect();
        let intrinsics: Vec<_> = new.functions[from..].iter()
            .map(|f| f.intrinsic.as_ref().and_then(|i| crate::intrinsics::Intrinsic::resolve(&i.name))).collect();
        let objects_before = old.objects.len();
        let globals = new.globals.len();
        // ---- commit: infallible steps; new functions are callable only once their records are written ----
        self.heap.set_identity_words(resources.inline.iter().map(Option::is_some).collect());
        self.heap.extend_layouts(layouts);
        self.resources.extend_metadata(resources);
        self.invocations[from..functions].copy_from_slice(&records);
        self.natives.extend(natives);
        self.intrinsics.extend(intrinsics);
        self.held = held_objects(&loaded.program);
        self.globals.resize(globals, None);
        #[cfg(feature = "jit")]
        self.jit.extend_dispatch(&loaded.program, objects_before);
        #[cfg(not(feature = "jit"))]
        let _ = objects_before;
        if let Some(interp) = &mut self.interp {
            interp.extend(functions);
        }
        if let Some(host) = &mut self.host {
            host.extend(&loaded.program);
        }
        self.loaded = loaded;
        Ok(())
    }


    fn assemble(mut loaded: Loaded) -> Result<Self, Error> {
        // #1124: a build without the JIT has only the interpreter.
        let interpret = !cfg!(feature = "jit") || crate::env_switch::enabled("SPARROW_CORE_INTERPRET");
        if interpret {
            loaded.without_pointer_free();
        }
        let mut heap = Heap::new(loaded.layouts());
        heap.set_stress(crate::env_switch::enabled("SPARROW_CORE_GC_STRESS"));
        if crate::env_switch::enabled("SPARROW_CORE_STATS") {
            heap.allocations = Some(Box::default());
        }
        let invocations = loaded.program.functions.iter().map(|f| {
            Ok(Invocation {
                code: 0,
                params: u32::try_from(f.params.len()).map_err(|_| Error("too many function parameters".into()))?,
                is_async: f.is_async,
            })
        }).collect::<Result<Box<[_]>, Error>>()?;
        let resources = crate::failures::Resources::new(&loaded.program);
        heap.set_identity_words(resources.inline.iter().map(Option::is_some).collect());
        let intrinsics = loaded
            .program
            .functions
            .iter()
            .map(|f| f.intrinsic.as_ref().and_then(|i| crate::intrinsics::Intrinsic::resolve(&i.name)))
            .collect();
        Ok(Self {
            resources,
            ready: std::collections::VecDeque::new(),
            spawned: Vec::new(),
            running: Vec::new(),
            timers: Vec::new(),
            states: crate::word::FastMap::default(),
            natives: loaded
                .program
                .functions
                .iter()
                .map(|f| f.native.as_deref().and_then(okkama_runtime_abi::Native::resolve))
                .collect(),
            intrinsics,
            held: held_objects(&loaded.program),
            host: Some(Box::new(crate::host::Heap::new(&loaded.program))),
            io: crate::io::Io::default(),
            stats: crate::env_switch::enabled("SPARROW_CORE_STATS").then(Box::default),
            created: crate::clock::Instant::now(),
            worker_command: Vec::new(),
            limits: crate::budget::Limits::default(),
            poisoned: false,
            budget: Budget::default(),
            interp: interpret.then(|| Box::new(crate::interp::Interp::new(loaded.program.functions.len()))),
            heap,
            #[cfg(feature = "jit")]
            jit: crate::jit::Jit::new()?,
            invocations,
            capacities: None,
            session: (None, None),
            closure_bridge_reference: std::env::var_os("SPARROW_CORE_CLOSURE_BRIDGE").is_some_and(|value| value == "reference"),
            globals: vec![None; loaded.program.globals.len()],
            loaded,
            constants: Vec::new(),
            roots: Vec::new(),
            fatal: None,
            uncaught: None,
        })
    }

    /// Collect when the heap asks for it. `frame` is the frame pointer of a
    /// runtime helper called directly from machine code; `extra` are words
    /// the helper still holds.
    pub fn safepoint(&mut self, frame: u64, ctx: &Ctx, extra: &[(Word, Repr)]) {
        if !self.heap.needs_collection() {
            return;
        }
        self.count(|_| "gc collection".into());
        let started = self.heap.allocations.is_some().then(crate::clock::Instant::now);
        self.prune_spawned();
        #[cfg(feature = "jit")]
        let jit = &self.jit;
        let interp = self.interp.as_deref();
        let constants = &self.constants;
        let roots = &self.roots;
        // The scheduler's roots are visited where they live, without a
        // vector of them per collection (#434).
        let (ready, spawned, running) = (&self.ready, &self.spawned, &self.running);
        let (timers, requests) = (&self.timers, &self.io.requests);
        let states = &self.states;
        // #817: the root region of detached Futures, until the root closer closes it.
        let mut kept: Vec<Word> = self.resources.root.into_iter().collect();
        // #588: the session region and the values the REPL keeps.
        kept.extend(self.session.0.into_iter().chain(self.session.1));
        // #915: the pinned identities, their holders and pins waiting for a taker.
        kept.extend(self.pin_roots());
        self.heap.collect_with(|visit| {
            // A group's tasks are kept only while the group is reachable some
            // other way (#503); running and parked tasks are roots of their own.
            for state in states.values() {
                if !matches!(state, crate::primitives::State::Group(_)) {
                    state.trace(visit);
                }
            }
            match interp {
                // #1120: the interpreter's active frames are its register stack; `frame` is a Rust frame.
                Some(interp) => interp.roots(visit),
                // SAFETY: frame records above a helper called from machine code
                // lead through JIT frames to the Rust entry.
                #[cfg(feature = "jit")]
                None => unsafe { jit.frame_roots(frame, ctx.entry_frame, visit) },
                // #1124: without the JIT every runtime interprets (`assemble`).
                #[cfg(not(feature = "jit"))]
                None => {
                    let _ = (frame, ctx.entry_frame);
                    unreachable!("a runtime without the JIT always has its interpreter")
                }
            }
            for &(word, repr) in constants.iter().chain(roots).chain(extra) {
                visit(word, repr);
            }
            visit(ctx.failure, Repr::Ref);
            for &record in &kept {
                visit(record, Repr::Ref);
            }
            let tasks = ready.iter().chain(spawned).chain(running);
            for &task in tasks.chain(timers.iter().map(|(_, frame)| frame)).chain(requests.keys()) {
                visit(task, Repr::Ref);
            }
        }, |marked, visit| {
            for (&object, state) in states.iter() {
                if matches!(state, crate::primitives::State::Group(_)) && marked(object) {
                    state.trace(visit);
                }
            }
        });
        let released = self.heap.take_released();
        self.resources.forget(&released);
        for object in &released {
            self.states.remove(object);
        }
        if let Some(host) = &mut self.host {
            host.released(&released);
        }
        if let (Some(allocations), Some(started)) = (&mut self.heap.allocations, started) {
            allocations.around_nanos += started.elapsed().as_nanos() as u64;
        }
    }

    /// Count an event (only when statistics are on).
    #[inline]
    pub fn count(&mut self, event: impl FnOnce(&Runtime) -> String) {
        if self.stats.is_some() {
            let key = event(self);
            if let Some(stats) = &mut self.stats {
                *stats.entry(key).or_default() += 1;
            }
        }
    }

    /// The statistics report, most frequent first.
    pub fn stats_report(&self) -> Option<String> {
        let stats = self.stats.as_ref()?;
        let heap = self.heap.stats();
        let extra = [
            ("invocation metadata, bytes".to_string(), std::mem::size_of_val(&*self.invocations) as u64),
            ("heap bytes allocated".to_string(), heap.allocated_bytes as u64),
            ("heap live bytes, mean after collection".to_string(), (heap.live_sum / heap.collections.max(1)) as u64),
            ("collections".to_string(), heap.collections as u64),
            // Rust memory owned by heap objects, estimated (#331).
            ("owned Rust bytes charged, estimate".to_string(), heap.external_allocated_bytes as u64),
            ("owned Rust bytes live after the last collection, estimate".to_string(), heap.external_live_bytes as u64),
            ("collections triggered by owned Rust memory".to_string(), heap.external_collections as u64),
        ];
        #[cfg(feature = "jit")]
        let helpers = self.jit.helper_calls();
        #[cfg(not(feature = "jit"))]
        let helpers: Vec<(String, u64)> = Vec::new();
        // Runtime allocations by kind (objects by type) and collection time.
        let program = &self.loaded.program;
        let allocations: Vec<(String, u64)> = self
            .heap
            .allocations
            .iter()
            .flat_map(|allocations| {
                allocations
                    .kinds
                    .iter()
                    .flat_map(|(&(kind, descriptor), &(count, bytes))| {
                        let what = match kind {
                            "object" => format!("object {}", program.objects.get(descriptor as usize).map_or("?", |object| object.name.as_str())),
                            other => other.to_string(),
                        };
                        [(format!("alloc runtime {what}, count"), count), (format!("alloc runtime {what}, bytes"), bytes)]
                    })
                    .chain(allocations.live.iter().flat_map(|(&(kind, descriptor), &(count, bytes))| {
                        let what = match kind {
                            "object" => format!("object {}", program.objects.get(descriptor as usize).map_or("?", |object| object.name.as_str())),
                            other => other.to_string(),
                        };
                        [(format!("live after the last collection {what}, count"), count), (format!("live after the last collection {what}, bytes"), bytes)]
                    }))
                    .chain([
                        ("collection, microseconds".to_string(), allocations.collection_nanos / 1000),
                        ("collection marking, microseconds".to_string(), allocations.mark_nanos / 1000),
                        ("collection sweep, microseconds".to_string(), allocations.sweep_nanos / 1000),
                        (
                            "collection around it in the runtime, microseconds".to_string(),
                            allocations.around_nanos.saturating_sub(allocations.collection_nanos) / 1000,
                        ),
                        ("run, microseconds".to_string(), self.created.elapsed().as_micros() as u64),
                    ])
            })
            .collect();
        let mut entries: Vec<_> = stats
            .iter()
            .chain(extra.iter().map(|(k, v)| (k, v)))
            .chain(helpers.iter().map(|(k, v)| (k, v)))
            .chain(allocations.iter().map(|(k, v)| (k, v)))
            .collect();
        entries.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        Some(entries.iter().map(|(k, v)| format!("{v:>12} {k}\n")).collect())
    }

    pub fn failure_new(&mut self, kind: FailureKind) -> Word {
        self.heap.allocate_payload(Box::new(Failure {
            kind,
            trace: Vec::new(),
            trace_omitted: 0,
            cause: None,
            secondary: Vec::new(),
            last_frame: None,
        }))
    }

    pub fn failure(&self, word: Word) -> Option<&Failure> {
        self.heap.payload::<Failure>(word)
    }

    fn integer_value(&self, word: Word) -> BigInt {
        match self.heap.integer(word) {
            Integer::Small(n) => BigInt::from(n),
            Integer::Big(n) => n.clone(),
        }
    }
}

// ---- helpers called from machine code ----
//
// SAFETY (all helpers): `ctx` is the context the generated code was entered
// with; its runtime pointer is valid and not otherwise borrowed while machine
// code runs.

unsafe fn runtime<'a>(ctx: *mut Ctx) -> (&'a mut Ctx, &'a mut Runtime) {
    let ctx = unsafe { &mut *ctx };
    let runtime = unsafe { &mut *ctx.runtime };
    (ctx, runtime)
}

fn fail(ctx: &mut Ctx, runtime: &mut Runtime, kind: FailureKind) -> Word {
    ctx.failure = runtime.failure_new(kind);
    ctx.status = FAILED;
    0
}

pub(crate) fn fatal(ctx: &mut Ctx, runtime: &mut Runtime, message: String) -> Word {
    if runtime.fatal.is_none() {
        runtime.fatal = Some(message);
    }
    ctx.status = FATAL;
    0
}

/// Slow path of inline allocation: collect if due, then a cell of `words`.
/// Under a memory budget, 0 when the allocation would cross it (status
/// `EXHAUSTED`); machine code then returns.
#[inline(never)]
pub extern "C" fn rt_allocate(ctx: *mut Ctx, words: u64) -> Word {
    let frame = frame_pointer!();
    let (ctx, runtime) = unsafe { runtime(ctx) };
    runtime.count(|_| "allocation slow path".into());
    if let (Some(limit), Some(used)) = (runtime.limits.memory, runtime.memory_used()) {
        // A small cell comes from a new page when its free list is empty.
        let bytes = match Heap::class_of(words as usize) {
            Some(_) => crate::heap::PAGE_BYTES,
            None => words as usize * 8,
        };
        if used.saturating_add(bytes as i64) > limit as i64 {
            return runtime.exhaust(ctx, Exhausted::Memory);
        }
    }
    runtime.safepoint(frame, ctx, &[]);
    runtime.heap.allocate(words as usize)
}

/// A step slice is used up (or the run must stop): charge it, check the
/// other budgets and the interrupt flag, and give the next slice.
#[inline(never)]
pub extern "C" fn rt_poll(ctx: *mut Ctx) {
    let (ctx, runtime) = unsafe { runtime(ctx) };
    if let Some(what) = runtime.over_budget() {
        runtime.exhaust(ctx, what);
        return;
    }
    match runtime.refill() {
        // The step that polled is the first of the new slice.
        Some(fuel) => runtime.budget.fuel.set(fuel - 1),
        None => {
            runtime.exhaust(ctx, Exhausted::Steps);
        }
    }
}

impl Runtime {
    /// Stop the run because `what` ran out; the first reason is kept.
    pub(crate) fn exhaust(&mut self, ctx: &mut Ctx, what: Exhausted) -> Word {
        self.budget.exhausted.get_or_insert(what);
        ctx.status = EXHAUSTED;
        self.budget.fuel.set(0);
        0
    }

    /// What the memory budget measures now (#269): the process allocator's
    /// bytes beyond the start of the run, or this runtime's heap. In `i64` on
    /// every target (#1124: `isize` is 32 bits on wasm32).
    pub(crate) fn memory_used(&self) -> Option<i64> {
        self.limits.memory?;
        match self.limits.memory_scope {
            crate::budget::MemoryScope::Process => self.budget.meter.as_ref().map(crate::budget::Meter::used),
            crate::budget::MemoryScope::Runtime => Some(self.heap.held_bytes() as i64),
        }
    }

    /// Would `bytes` more cross the memory budget?
    pub(crate) fn would_exceed(&self, bytes: u64) -> bool {
        match (self.limits.memory, self.memory_used()) {
            (Some(limit), Some(used)) => used.saturating_add(bytes.min(i64::MAX as u64) as i64) > limit as i64,
            _ => false,
        }
    }

    /// The reason to stop at a poll, other than steps.
    pub(crate) fn over_budget(&self) -> Option<Exhausted> {
        if let Some(what) = self.budget.exhausted {
            return Some(what);
        }
        if self.limits.interrupted() {
            return Some(Exhausted::Interrupted);
        }
        match (self.limits.memory, self.memory_used()) {
            (Some(limit), Some(used)) if used > limit as i64 => Some(Exhausted::Memory),
            _ => None,
        }
    }

    /// Charge the slice just used and size the next; None when the step
    /// budget is spent.
    fn refill(&mut self) -> Option<i64> {
        let Some(left) = &mut self.budget.steps_left else {
            return Some(crate::budget::SLICE as i64);
        };
        *left = left.saturating_sub(self.budget.slice);
        if *left == 0 {
            return None;
        }
        self.budget.slice = (*left).min(crate::budget::SLICE);
        Some(self.budget.slice as i64)
    }

    /// Start the budgets of a run: the first slice of steps.
    fn start_budget(&mut self) -> Result<i64, Error> {
        // #1129: a run whose budgets would poll where earlier runs compiled code that does not (or the other way)
        // is refused: the code would keep the first runs' choice, and the engines must agree on it.
        let polled = self.limits.polled();
        match self.budget.polled {
            Some(fixed) if fixed != polled => return Err(Error(polling_fixed(fixed))),
            _ => self.budget.polled = Some(polled),
        }
        if let Some(limit) = self.limits.memory {
            // A budget of the runtime reads its heap and needs no probe.
            let meter = match self.limits.memory_scope {
                crate::budget::MemoryScope::Process => {
                    let probe = crate::budget::memory_probe().ok_or_else(|| {
                        Error("a memory budget needs a memory probe (sparrow_core::budget::set_memory_probe)".into())
                    })?;
                    Some(*self.budget.meter.get_or_insert_with(|| crate::budget::Meter::start(probe)))
                }
                crate::budget::MemoryScope::Runtime => None,
            };
            self.heap.watch = Some(crate::budget::Watch::new(meter, limit, &self.budget.fuel));
        }
        Ok(match self.limits.steps {
            None => crate::budget::SLICE as i64,
            Some(steps) => {
                self.budget.steps_left = Some(steps);
                self.budget.slice = steps.min(crate::budget::SLICE);
                self.budget.slice as i64
            }
        })
    }

    /// #1129: do this runtime's code and its interpreter poll the fuel cell? Fixed at the first run; before it,
    /// what the budgets say now.
    pub(crate) fn polled(&self) -> bool {
        self.budget.polled.unwrap_or_else(|| self.limits.polled())
    }

    /// The report of an exhausted run.
    pub(crate) fn exhausted_report(&mut self) -> Error {
        let message = self.budget.exhausted.unwrap_or(Exhausted::Interrupted).message();
        self.uncaught = Some(crate::Uncaught { category: "exhausted", error_type: None, message: message.into(), trace: Vec::new(), trace_omitted: 0, trace_offset: 0, cause_traces: Vec::new() });
        Error(message.into())
    }
}

/// #1129: the refusal of a run whose step, memory or interrupt budgets differ, in being set at all, from the first
/// run of the runtime.
#[cold]
fn polling_fixed(fixed: bool) -> String {
    let (first, now) = if fixed { ("with", "without") } else { ("without", "with") };
    format!(
        "this runtime first ran {first} a step, memory or interrupt budget and now runs {now} one: \
         its code keeps polling as it was compiled; set the budgets before the first run"
    )
}

/// Compile a function on its first call; returns its code address.
#[cfg(feature = "jit")]
#[inline(never)]
pub extern "C" fn rt_compile(ctx: *mut Ctx, function: u64) -> u64 {
    let (ctx, runtime) = unsafe { runtime(ctx) };
    match crate::jit::compile(runtime, function as usize) {
        Ok(code) => code,
        Err(error) => fatal(ctx, runtime, error.0),
    }
}

pub const INT_ADD: u64 = 0;
pub const INT_SUB: u64 = 1;
pub const INT_MUL: u64 = 2;
pub const INT_MOD: u64 = 3;

/// Exact integer arithmetic beyond the inline fast path.
#[inline(never)]
pub extern "C" fn rt_int(ctx: *mut Ctx, op: u64, left: Word, right: Word) -> Word {
    let frame = frame_pointer!();
    let (ctx, runtime) = unsafe { runtime(ctx) };
    // Borrow BigInt operands instead of cloning them; inline ones are widened.
    fn widen(n: Integer<'_>) -> std::borrow::Cow<'_, BigInt> {
        match n {
            Integer::Small(n) => std::borrow::Cow::Owned(BigInt::from(n)),
            Integer::Big(n) => std::borrow::Cow::Borrowed(n),
        }
    }
    let (l, r) = (widen(runtime.heap.integer(left)), widen(runtime.heap.integer(right)));
    let result = match op {
        INT_ADD => &*l + &*r,
        INT_SUB => &*l - &*r,
        INT_MUL => {
            // The product's size is known before the (long) multiplication.
            if runtime.would_exceed((l.bits() + r.bits()) / 8) {
                return runtime.exhaust(ctx, Exhausted::Memory);
            }
            &*l * &*r
        }
        _ => {
            if r.is_zero() {
                return fail(ctx, runtime, FailureKind::Program("Modulo by zero".into()));
            }
            &*l % &*r
        }
    };
    let word = runtime.heap.big(result);
    if runtime.heap.needs_collection() {
        runtime.safepoint(frame, ctx, &[(word, Repr::Int)]);
    }
    word
}

/// Integer order beyond two inline integers: -1, 0 or 1.
#[inline(never)]
pub extern "C" fn rt_int_order(ctx: *mut Ctx, left: Word, right: Word) -> i64 {
    let (_, runtime) = unsafe { runtime(ctx) };
    match runtime.integer_value(left).cmp(&runtime.integer_value(right)) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

#[inline(never)]
pub extern "C" fn rt_concat(ctx: *mut Ctx, left: Word, right: Word) -> Word {
    let frame = frame_pointer!();
    let (ctx, runtime) = unsafe { runtime(ctx) };
    // A string holds at most 4 GiB (#985), as `join` already checked.
    if let Some(message) = crate::ops::concat_too_long(runtime, left, right) {
        return fail(ctx, runtime, FailureKind::Program(message));
    }
    // The size of what is allocated is known before the copy (#72): a
    // memory budget stops it here.
    let Some(word) = crate::ops::concat_text(runtime, left, right, |runtime, bytes| !runtime.would_exceed(bytes as u64)) else {
        return runtime.exhaust(ctx, Exhausted::Memory);
    };
    runtime.safepoint(frame, ctx, &[(word, Repr::Ref)]);
    word
}

/// `rt_array` representation argument for a packed byte array.
pub const ARRAY_BYTES: u64 = 3;

/// Array from `count` words at `elements` (a stack slot of the caller).
#[inline(never)]
pub extern "C" fn rt_array(ctx: *mut Ctx, repr: u64, elements: *const Word, count: u64) -> Word {
    let frame = frame_pointer!();
    let (ctx, runtime) = unsafe { runtime(ctx) };
    let elements = unsafe { std::slice::from_raw_parts(elements, count as usize) };
    let word = if repr == ARRAY_BYTES {
        let bytes: Vec<u8> = elements.iter().map(|&w| w as u8).collect();
        runtime.heap.allocate_bytes(&bytes)
    } else {
        runtime.heap.allocate_array(Repr::from_bits(repr), elements)
    };
    runtime.safepoint(frame, ctx, &[(word, Repr::Ref)]);
    word
}

/// A program failure with a constant message string.
#[inline(never)]
pub extern "C" fn rt_trap(ctx: *mut Ctx, message: Word) -> Word {
    let (ctx, runtime) = unsafe { runtime(ctx) };
    let message = runtime.heap.text(message).to_owned();
    fail(ctx, runtime, FailureKind::Program(message))
}

/// The machine stack ran out at a function entry: the memory budget when it
/// caps the stack, otherwise the failure `Stack overflow`.
#[inline(never)]
pub extern "C" fn rt_stack_overflow(ctx: *mut Ctx) -> Word {
    let (ctx, runtime) = unsafe { runtime(ctx) };
    if runtime.budget.stack_capped {
        return runtime.exhaust(ctx, Exhausted::Memory);
    }
    fail(ctx, runtime, FailureKind::Program("Stack overflow".into()))
}

/// `throw value`.
#[inline(never)]
pub extern "C" fn rt_throw(ctx: *mut Ctx, value: Word) -> Word {
    let (ctx, runtime) = unsafe { runtime(ctx) };
    let header = runtime.heap.header_of(value);
    let native = heap::kind(header) == heap::OBJECT
        && runtime.loaded.program.objects[heap::payload(header) as usize].is_native_failure();
    fail(ctx, runtime, if native { FailureKind::Native(value) } else { FailureKind::Thrown(value) })
}

/// The most activations a failure's trace records (8 MiB of sites).
pub const TRACE_LIMIT: usize = 1 << 20;
/// Of those, the outermost ones kept once the trace is full (#1034): a stack
/// overflow still shows how the recursion was entered (`main` and its callers).
pub const TRACE_TAIL: usize = 1 << 12;

impl Failure {
    /// Record one more activation, the next one outwards.
    pub(crate) fn record(&mut self, site: (u32, u32)) {
        record_site(&mut self.trace, &mut self.trace_omitted, site, TRACE_LIMIT, TRACE_TAIL);
    }

    /// The recorded activations innermost first: the head, then the tail in
    /// order; `trace_omitted` of them are missing between the two.
    pub fn sites(&self) -> Vec<crate::Site> {
        ordered_sites(&self.trace, self.trace_omitted, TRACE_TAIL)
    }
}

/// Up to `limit` sites are pushed; after that each new one overwrites the
/// oldest of the last `tail` slots and counts one omitted activation (#1034).
fn record_site(trace: &mut Vec<(u32, u32)>, omitted: &mut u64, site: (u32, u32), limit: usize, tail: usize) {
    if trace.len() < limit {
        trace.push(site);
    } else {
        let at = limit - tail + (*omitted % tail as u64) as usize;
        trace[at] = site;
        *omitted += 1;
    }
}

/// The trace in unwinding order: a full trace's ring starts at its oldest slot.
fn ordered_sites(trace: &[(u32, u32)], omitted: u64, tail: usize) -> Vec<crate::Site> {
    let site = |&(function, pc): &(u32, u32)| crate::Site { function, pc };
    if omitted == 0 {
        return trace.iter().map(site).collect();
    }
    let (head, ring) = trace.split_at(trace.len() - tail);
    let oldest = (omitted % tail as u64) as usize;
    head.iter().chain(&ring[oldest..]).chain(&ring[..oldest]).map(site).collect()
}

/// Record that the pending failure reaches `function` at `pc` in the
/// activation with frame pointer `frame`; each activation is recorded once, at
/// the pc where the failure first reached it (a rethrow does not add it again).
#[inline(never)]
pub extern "C" fn rt_trace(ctx: *mut Ctx, function: u64, pc: u64, frame: u64) {
    let (ctx, runtime) = unsafe { runtime(ctx) };
    let failure = ctx.failure;
    if let Some(failure) = runtime.heap.payload_mut::<Failure>(failure)
        && failure.last_frame != Some((function as u32, frame)) {
            // A stack overflow unwinds through tens of millions of
            // activations: about a million innermost and the outermost few
            // thousand are kept, the ones between counted (#1009, #1034).
            failure.record((function as u32, pc as u32));
            failure.last_frame = Some((function as u32, frame));
        }
}

/// Call a native provider with `count` argument words at `arguments`.
#[inline(never)]
pub extern "C" fn rt_native(ctx: *mut Ctx, function: u64, arguments: *const Word, count: u64) -> Word {
    let frame = frame_pointer!();
    let (ctx, runtime) = unsafe { runtime(ctx) };
    // SAFETY: the caller's argument slot outlives the call; natives do not collect.
    let arguments = unsafe { std::slice::from_raw_parts(arguments, count as usize) };
    runtime.count(|r| format!("native {}", r.loaded.program.functions[function as usize].symbol));
    // A panicking provider must not unwind through machine code: it stops the
    // run with an internal error and poisons the runtime (H-14).
    let called = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::natives::call(runtime, function as usize, arguments)
    }));
    let called = match called {
        Ok(called) => called,
        Err(payload) => {
            let symbol = runtime.loaded.program.functions[function as usize].symbol.clone();
            runtime.poisoned = true;
            return fatal(ctx, runtime, format!("internal error in native {symbol}: {}", panic_text(&*payload)));
        }
    };
    match called {
        Ok((word, repr)) => {
            runtime.safepoint(frame, ctx, &[(word, repr)]);
            word
        }
        Err(crate::natives::Refusal::Failure(kind)) => fail(ctx, runtime, kind),
        Err(crate::natives::Refusal::Raise(failure)) => {
            ctx.failure = failure;
            ctx.status = FAILED;
            0
        }
        Err(crate::natives::Refusal::Fatal(message)) => fatal(ctx, runtime, message),
        Err(crate::natives::Refusal::Exhausted(what)) => runtime.exhaust(ctx, what),
    }
}

impl Runtime {
    /// Run the synchronous entry function; returns its integer result.
    pub fn run_entry(self: &mut Box<Self>, stack: u64) -> Result<i64, Error> {
        let entry = self
            .loaded
            .program
            .entry
            // A program without `main` fails when execution starts, with an ordinary runtime report.
            .ok_or_else(|| Error("RuntimeError: Program requires main() declaration".into()))?
            .0 as usize;
        let result = self.run_function(entry, stack)?;
        match self.heap.integer(result) {
            Integer::Small(n) => Ok(n),
            // Past i64 the result is out of range as surely as within it: the
            // same message the CLIs give for any result outside 0..255 (#1009).
            Integer::Big(n) => n.to_i64().ok_or_else(|| Error(format!("main() must return an int in 0..255, got {n}"))),
        }
    }

    /// Run a parameterless function (sync or async) to completion; its
    /// result word, or the report of its failure.
    pub fn run_function(self: &mut Box<Self>, entry: usize, stack: u64) -> Result<Word, Error> {
        if !self.loaded.program.functions[entry].params.is_empty() {
            return Err(Error("parameterized entry is not supported".into()));
        }
        self.run_function_with(entry, &[], stack)
    }

    /// Run a function (sync or async) over `arguments` to completion: its
    /// result word, or the report of its failure (process workers).
    pub fn run_function_with(self: &mut Box<Self>, entry: usize, arguments: &[Word], stack: u64) -> Result<Word, Error> {
        if self.poisoned {
            return Err(Error("internal error: the runtime is unusable after a panic in a native".into()));
        }
        let function = &self.loaded.program.functions[entry];
        if function.params.len() != arguments.len() {
            return Err(Error("argument count mismatch".into()));
        }
        let asynchronous = function.is_async;
        let fuel = self.start_budget()?;
        self.budget.fuel.set(fuel);
        // A memory budget also bounds the machine stack.
        self.budget.stack_capped = self.limits.memory.is_some_and(|memory| memory.max(2 << 20) < stack);
        let stack = self.limits.memory.map_or(stack, |memory| stack.min(memory.max(2 << 20)));
        let marker = 0u8;
        let mut ctx = Ctx {
            status: OK,
            // R2: the VM thread has a large stack; keep a headroom below the limit.
            stack_limit: (&marker as *const u8 as u64).saturating_sub(stack.saturating_sub(STACK_HEADROOM)),
            failure: UNBOUND,
            runtime: &mut **self,
            entry_frame: frame_pointer!(),
            scratch: UNBOUND,
        };
        // An async entry runs its own root Future through the same scheduler (draft 3 §3).
        let result = if asynchronous {
            // The root Future, its task tree, then shutdown of spawned tasks.
            let root = self.future_new(entry, arguments);
            self.roots.push((root, Repr::Ref));
            unsafe { *((root + crate::tasks::STATE) as *mut u64) = crate::tasks::RUNNING };
            self.run_body(&mut ctx, root);
            if ctx.status == OK {
                self.schedule(&mut ctx, root);
            }
            if ctx.status == OK
                && let Some(failure) = self.shutdown(&mut ctx) {
                    let message = self.describe(&mut ctx, failure);
                    self.drop_root(root);
                    return Err(Error(message));
                }
            // #817: the root closer, after the spawned tasks settled: outcomes of Futures that outlived their owners.
            if ctx.status == OK
                && let Some(failure) = self.close_root(&mut ctx) {
                    let message = self.describe(&mut ctx, failure);
                    self.drop_root(root);
                    return Err(Error(message));
                }
            if ctx.status == OK {
                let state = unsafe { *((root + crate::tasks::STATE) as *const u64) };
                if state == crate::tasks::FAILURE_STATE {
                    ctx.failure = unsafe { *((root + crate::tasks::FAILURE) as *const u64) };
                    ctx.status = FAILED;
                }
            }
            let result = unsafe { *((root + crate::tasks::RESULT) as *const u64) };
            // #588: a session's cell hands its result on, as `await.take` does: the root Future's cache gives up
            // what it owns, so the session adopts the result exactly as after a synchronous cell.
            if self.capacities.is_some() && ctx.status == OK {
                self.take_outcome(root);
            }
            // #588: a session runs many entries; the root Future is not kept past its run. The caller roots the
            // result before it allocates.
            self.drop_root(root);
            result
        } else {
            if let Some(interp) = &mut self.interp {
                // #1120: the register stack is bounded as the machine stack is.
                interp.limit(stack);
            }
            match self.invoke(&mut ctx, entry, arguments) {
                Ok(result) => result,
                Err(error) => {
                    fatal(&mut ctx, self, error.0);
                    return Err(Error(self.fatal.clone().unwrap_or_default()));
                }
            }
        };
        match ctx.status {
            OK => Ok(result),
            FAILED => {
                ctx.status = OK;
                let failure = ctx.failure;
                Err(Error(self.describe(&mut ctx, failure)))
            }
            EXHAUSTED => Err(self.exhausted_report()),
            _ => Err(Error(self.fatal.clone().unwrap_or_else(|| "engine failure".into()))),
        }
    }
}

pub(crate) use frame_pointer;

// ---- the engine seam (#1120) ----
//
// Rust enters Okkama code in six places: a synchronous entry, a closure called by its helper, an async body
// started or resumed, a function an intrinsic calls, a destroy, and `Error.text` of a failure report. All of them
// go through these two methods, and the collector's one root walk (`safepoint`) asks the same engine for the
// roots of its frames. The JIT's published code is read first, as before; only a function without code takes the
// cold branch, where the interpreter (every function has no code there) or the JIT's compiler runs.

impl Runtime {
    /// Run synchronous `function` over `words` (its parameters) on this runtime's engine; its result word, with
    /// a failure or a stop in `ctx.status`. `Err` only when the JIT could not compile the function: each caller
    /// reports that as it did before the seam.
    #[cfg(feature = "jit")]
    #[inline]
    pub(crate) fn invoke(&mut self, ctx: *mut Ctx, function: usize, words: &[Word]) -> Result<Word, Error> {
        let code = match self.invocations[function].code {
            0 => return self.invoke_cold(ctx, function, words),
            code => code,
        };
        // SAFETY: compiled functions take the context and their parameters.
        Ok(unsafe { crate::jit::invoke(code, ctx, words) })
    }

    /// Start or resume the async body of `frame` (a RUNNING frame of `function`): true while it is pending.
    #[cfg(feature = "jit")]
    #[inline]
    pub(crate) fn resume(&mut self, ctx: *mut Ctx, function: usize, frame: Word) -> Result<bool, Error> {
        let code = match self.invocations[function].code {
            0 => return self.resume_cold(ctx, function, frame),
            code => code,
        };
        // SAFETY: async bodies take their frame.
        Ok(unsafe { crate::jit::invoke(code, ctx, &[frame]) } != 0)
    }

    /// A function without code: the interpreter runs it, or the JIT compiles it first.
    #[cfg(feature = "jit")]
    #[cold]
    #[inline(never)]
    fn invoke_cold(&mut self, ctx: *mut Ctx, function: usize, words: &[Word]) -> Result<Word, Error> {
        if self.interp.is_some() {
            return Ok(crate::interp::invoke(ctx, function, words));
        }
        let code = crate::jit::compile(self, function)?;
        // SAFETY: compiled functions take the context and their parameters.
        Ok(unsafe { crate::jit::invoke(code, ctx, words) })
    }

    #[cfg(feature = "jit")]
    #[cold]
    #[inline(never)]
    fn resume_cold(&mut self, ctx: *mut Ctx, function: usize, frame: Word) -> Result<bool, Error> {
        if self.interp.is_some() {
            return Ok(crate::interp::resume(ctx, function, frame));
        }
        let code = crate::jit::compile(self, function)?;
        // SAFETY: async bodies take their frame.
        Ok(unsafe { crate::jit::invoke(code, ctx, &[frame]) } != 0)
    }

    /// Without the JIT (#1124) the interpreter runs every function: no code is ever published.
    #[cfg(not(feature = "jit"))]
    #[inline]
    pub(crate) fn invoke(&mut self, ctx: *mut Ctx, function: usize, words: &[Word]) -> Result<Word, Error> {
        Ok(crate::interp::invoke(ctx, function, words))
    }

    #[cfg(not(feature = "jit"))]
    #[inline]
    pub(crate) fn resume(&mut self, ctx: *mut Ctx, function: usize, frame: Word) -> Result<bool, Error> {
        Ok(crate::interp::resume(ctx, function, frame))
    }
}

/// Representation of an argument or result kind for root reporting.
pub fn repr_of(kind: Option<Kind>) -> Repr {
    kind.map_or(Repr::Raw, Kind::repr)
}


/// Float operation whose inline result was not finite (or a zero divisor).
#[inline(never)]
pub extern "C" fn rt_float(ctx: *mut Ctx, op: u64, left: u64, right: u64) -> u64 {
    let (ctx, runtime) = unsafe { runtime(ctx) };
    match crate::scalar::float_binary(op, f64::from_bits(left), f64::from_bits(right)) {
        Ok(value) => value.to_bits(),
        Err(message) => fail(ctx, runtime, FailureKind::Program(message.into())),
    }
}

/// A closure of `function` capturing `count` words at `captures`.
#[inline(never)]
pub extern "C" fn rt_closure(ctx: *mut Ctx, function: u64, captures: *const Word, count: u64) -> Word {
    let frame = frame_pointer!();
    let (ctx, runtime) = unsafe { runtime(ctx) };
    let Some(entry) = usize::try_from(function).ok().and_then(|index| runtime.invocations.get(index)) else {
        return fatal(ctx, runtime, "invalid closure function".into());
    };
    if count > u64::from(entry.params) {
        return fatal(ctx, runtime, "closure capture count exceeds parameter count".into());
    }
    // The verified caller supplies `count` initialized words (a non-null
    // spill address even for zero captures); count fits the function layout.
    let captures = unsafe { std::slice::from_raw_parts(captures, count as usize) };
    let closure = runtime.heap.closure_new(function as u32, captures);
    runtime.safepoint(frame, ctx, &[(closure, Repr::Ref)]);
    closure
}

/// Call a closure with `count` argument words: its captures fill the leading
/// parameters of its function.
#[inline(never)]
pub extern "C" fn rt_call_closure(ctx: *mut Ctx, closure: Word, arguments: *const Word, count: u64) -> Word {
    let (ctx_ref, runtime) = unsafe { runtime(ctx) };
    runtime.count(|_| "closure call".into());
    let function = heap::payload(runtime.heap.header_of(closure)) as usize;
    let Some(mut entry) = runtime.invocations.get(function).copied() else {
        return fatal(ctx_ref, runtime, "invalid closure function".into());
    };
    if runtime.closure_bridge_reference {
        let source = &runtime.loaded.program.functions[function];
        entry.params = source.params.len() as u32;
        entry.is_async = source.is_async;
    }
    let captured = match entry.capture_count(count) {
        Ok(captured) => captured,
        Err(message) => return fatal(ctx_ref, runtime, message.into()),
    };
    let params = entry.params as usize;
    // Keep initialized storage until removing it has a measured benefit.
    // No uninitialized Word or partially initialized slice reaches any path.
    // More than 8 parameters (#1074) take a vector, called through `jit::spread`.
    let mut short = [UNBOUND; 8];
    let mut long = Vec::new();
    let words: &mut [Word] = if params <= 8 {
        &mut short
    } else {
        long.resize(params, UNBOUND);
        &mut long
    };
    for (i, word) in words.iter_mut().enumerate().take(captured) {
        *word = runtime.heap.field(closure, i);
    }
    if runtime.closure_bridge_reference || params > 8 {
        words[captured..params].copy_from_slice(unsafe { std::slice::from_raw_parts(arguments, count as usize) });
    } else {
        // Bound the short copy explicitly. Inspect optimized target code:
        // a Rust loop alone does not promise absence of memcpy/memmove.
        for i in 0..8 {
            if i >= count as usize {
                break;
            }
            // SAFETY: i < count <= params <= 8; captured + count = params.
            // The verified caller's spill contains count initialized words.
            words[captured + i] = unsafe { *arguments.add(i) };
        }
    }
    if entry.is_async {
        // Calling an async action makes its cold Future.
        return runtime.future_new(function, &words[..params]);
    }
    match runtime.invoke(ctx, function, &words[..params]) {
        Ok(result) => result,
        Err(error) => fatal(ctx_ref, runtime, error.0),
    }
}

/// Look a key up: 1 and the value in `scratch` when present, else 0.
#[inline(never)]
pub extern "C" fn rt_map_find(ctx: *mut Ctx, function: u64, pc: u64, map: Word, key: Word) -> u64 {
    let (ctx, runtime) = unsafe { runtime(ctx) };
    match crate::ops::map_find(runtime, function as usize, pc as usize, map, key) {
        Some(value) => {
            ctx.scratch = value;
            1
        }
        None => 0,
    }
}

/// The message of a panic payload.
pub(crate) fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| text.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".into())
}

/// Objects whose state the runtime holds rather than their fields (#956): an
/// `Atomic`, a channel and its ends, an event, a lock. They are the receivers of
/// the runtime's object intrinsics (`okkama.<family>.<Object>.<action>`, the
/// compiler's table of runtime-backed objects): the result of a constructor
/// (`new…`) and the first parameter of an action. A constructor's parameters are
/// values, not the object. Two of them are equal only when they are the same
/// object.
pub(crate) fn held_objects(program: &okkama_sparrow::Program) -> Vec<bool> {
    let mut held = vec![false; program.objects.len()];
    for function in &program.functions {
        let Some(intrinsic) = &function.intrinsic else { continue };
        let parts: Vec<&str> = intrinsic.name.split('.').collect();
        let [_, _, object, action] = parts[..] else { continue };
        if !object.starts_with(|c: char| c.is_ascii_uppercase()) {
            continue;
        }
        // A constructor makes the object (its result); an action receives it (its first parameter).
        let ty = if action.starts_with("new") {
            match function.result {
                Some(result) => result,
                None => continue,
            }
        } else {
            let Some(receiver) = function.params.first() else { continue };
            function.registers[receiver.0 as usize]
        };
        if let Some(okkama_sparrow::Type::Object(id) | okkama_sparrow::Type::Successor(id)) = program.ty(ty)
            && let Some(slot) = held.get_mut(id.0 as usize)
        {
            *slot = true;
        }
    }
    held
}

#[cfg(test)]
mod embedding_tests {
    use super::*;
    use crate::budget::{Limits, NativeGroup};
    use okkama_sparrow::{Dst, FuncRef, Function, Instruction as I, Program, Register, Type, TypeRef};

    #[test]
    fn closure_bridge_counts_are_checked_before_access() {
        // #1074: any number of parameters, more than 8 as well.
        for params in [0, 1, 2, 8, 9, 20] {
            let entry = Invocation { code: 0, params, is_async: false };
            for count in 0..=u64::from(params) {
                assert_eq!(entry.capture_count(count), Ok(params as usize - count as usize));
            }
            assert_eq!(entry.capture_count(u64::from(params) + 1), Err("closure argument count exceeds parameter count"));
            assert_eq!(entry.capture_count(u64::MAX), Err("closure argument count exceeds parameter count"));
        }
        let entry = Invocation { code: 0, params: u32::MAX, is_async: true };
        assert_eq!(entry.capture_count(u64::from(u32::MAX)), Ok(0));
        assert_eq!(entry.capture_count(0), Ok(u32::MAX as usize));
    }

    /// #1009: outside the crate a `Loaded` is made only from a verified
    /// program: a hand-made one with a parameter register out of range is
    /// refused before any analysis indexes it.
    #[test]
    fn public_runtimes_verify_their_program() {
        let source = "sparrow 18\nmodule \"bad\" \"1.0.0\"\ntype t0 int\n\
            function f0 \"f\" {\n registers [t0]\n params [r0]\n result t0\n 0: ret r0\n}\n";
        let good = okkama_sparrow::assemble(source).unwrap();
        assert!(Runtime::new(Loaded::verified(good.clone()).unwrap()).is_ok());
        let mut bad = good;
        bad.functions[0].params = vec![okkama_sparrow::Register(7)];
        assert!(Loaded::verified(bad).is_err());
    }

    fn bridge_runtime() -> Box<Runtime> {
        let source = "sparrow 18\nmodule \"bridge\" \"1.0.0\"\ntype t0 int\n\
            function f0 \"sync\" {\n registers [t0, t0]\n params [r0, r1]\n result t0\n 0: ret r0\n}\n\
            function f1 \"async\" async {\n registers [t0]\n params [r0]\n result t0\n 0: ret r0\n}\n";
        Box::new(Runtime::new(Loaded::new(okkama_sparrow::assemble(source).unwrap())).unwrap())
    }

    fn bridge_context(runtime: &mut Runtime) -> Ctx {
        Ctx { status: OK, stack_limit: 0, failure: UNBOUND, runtime, entry_frame: 0, scratch: UNBOUND }
    }

    #[test]
    fn closure_bridge_invalid_helpers_fail_before_touching_input() {
        for reference in [false, true] {
            for count in [3, u64::MAX] {
                let mut runtime = bridge_runtime();
                runtime.closure_bridge_reference = reference;
                let closure = runtime.heap.closure_new(0, &[]);
                let mut ctx = bridge_context(&mut runtime);
                // No readable arguments: invalid counts must fail before a read.
                assert_eq!(rt_call_closure(&mut ctx, closure, std::ptr::null(), count), 0);
                assert_eq!(ctx.status, FATAL);
                assert_eq!(runtime.fatal.as_deref(), Some("closure argument count exceeds parameter count"));
            }
        }
        for (function, count, expected) in [
            (0, 3, "closure capture count exceeds parameter count"),
            (0, u64::MAX, "closure capture count exceeds parameter count"),
            (u64::MAX, 0, "invalid closure function"),
        ] {
            let mut runtime = bridge_runtime();
            let mut ctx = bridge_context(&mut runtime);
            assert_eq!(rt_closure(&mut ctx, function, std::ptr::null(), count), 0);
            assert_eq!(ctx.status, FATAL);
            assert_eq!(runtime.fatal.as_deref(), Some(expected));
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn closure_bridge_invocation_publication_is_stable() {
        let mut runtime = bridge_runtime();
        let base = runtime.invocations.as_ptr();
        assert_eq!(std::mem::size_of::<Invocation>(), 16);
        assert_eq!(std::mem::offset_of!(Invocation, code), 0);
        assert!(runtime.invocations.iter().all(|entry| entry.code == 0));
        for function in [0, 1] {
            let entry = runtime.invocations[function];
            let address = crate::jit::compile(&mut runtime, function).unwrap();
            assert_ne!(address, 0);
            assert_eq!(runtime.invocations.as_ptr(), base);
            assert_eq!(runtime.invocations[function].code, address);
            assert_eq!(runtime.invocations[function].params, entry.params);
            assert_eq!(runtime.invocations[function].is_async, entry.is_async);
            assert_eq!(crate::jit::compile(&mut runtime, function).unwrap(), address);
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn closure_bridge_compilation_failure_does_not_publish_code() {
        let mut runtime = bridge_runtime();
        // A tiny compile-cost budget refuses even this one-instruction body.
        // No large fixture or machine-code invocation is needed.
        runtime.limits.memory = Some(1);
        let closure = runtime.heap.closure_new(0, &[crate::word::small(1).unwrap(), crate::word::small(2).unwrap()]);
        let mut ctx = bridge_context(&mut runtime);
        assert_eq!(rt_call_closure(&mut ctx, closure, [].as_ptr(), 0), 0);
        assert_eq!(ctx.status, FATAL);
        assert!(runtime.fatal.as_deref().unwrap().contains("compiling it would take more than"));
        assert_eq!(runtime.invocations[0].code, 0);
        assert_eq!(runtime.invocations[0].params, 2);
        // A failed compilation left no callable address; the compiler can
        // still finalize the same declaration after its budget is restored.
        runtime.limits = Limits::default();
        assert_ne!(crate::jit::compile(&mut runtime, 0).unwrap(), 0);
    }

    /// `main` returns the wall clock: one native of the Clock group.
    fn clock() -> Box<Runtime> {
        let native = Function {
            symbol: "runtime/time@1::wall_clock/0".into(),
            is_async: false,
            native: Some("runtime/time@1::wall_clock/0".into()),
            intrinsic: None,
            registers: Vec::new(),
            params: Vec::new(),
            result: Some(TypeRef(0)),
            code: Vec::new(),
            handlers: Vec::new(),
        };
        let main = Function {
            symbol: "main".into(),
            is_async: false,
            native: None,
            intrinsic: None,
            registers: vec![TypeRef(0)],
            params: Vec::new(),
            result: Some(TypeRef(0)),
            code: vec![
                I::Call { dst: Some(Dst(Register(0))), function: FuncRef(0), args: Vec::new() },
                I::Return { value: Some(Register(0)) },
            ],
            handlers: Vec::new(),
        };
        let program = Program {
            module: "app".into(),
            version: "1.0.0".into(),
            types: vec![Type::Int],
            functions: vec![native, main],
            entry: Some(FuncRef(1)),
            ..Program::default()
        };
        program.verify().unwrap();
        Box::new(Runtime::new(Loaded::new(program)).unwrap())
    }

    #[test]
    fn a_native_outside_the_permitted_groups_fails_the_program() {
        let mut runtime = clock();
        runtime.limits = Limits { natives: Some(vec![NativeGroup::Fs]), ..Limits::default() };
        let error = runtime.run_function(1, 1 << 24).unwrap_err();
        assert!(error.0.contains("Native runtime/time@1::wall_clock/0 needs the Clock permission"), "{}", error.0);
        let mut runtime = clock();
        runtime.limits = Limits { natives: Some(vec![NativeGroup::Clock]), ..Limits::default() };
        assert!(runtime.run_function(1, 1 << 24).is_ok());
    }

    #[test]
    fn a_panicking_native_is_an_internal_error_and_poisons_the_runtime() {
        let mut runtime = clock();
        crate::natives::PANIC_NEXT.with(|next| next.set(true));
        let error = runtime.run_function(1, 1 << 24).unwrap_err();
        assert!(error.0.contains("internal error in native runtime/time@1::wall_clock/0: provider failure"), "{}", error.0);
        assert!(runtime.poisoned);
        let again = runtime.run_function(1, 1 << 24).unwrap_err();
        assert!(again.0.contains("unusable"), "{}", again.0);
        // A fresh runtime in the same process works.
        assert!(clock().run_function(1, 1 << 24).is_ok());
    }
}

#[cfg(test)]
mod trace_tests {
    use super::{ordered_sites, record_site};
    use crate::Site;

    fn recorded(count: u32, limit: usize, tail: usize) -> (Vec<Site>, u64) {
        let (mut trace, mut omitted) = (Vec::new(), 0);
        for activation in 0..count {
            record_site(&mut trace, &mut omitted, (activation, 0), limit, tail);
        }
        (ordered_sites(&trace, omitted, tail), omitted)
    }

    fn functions(sites: &[Site]) -> Vec<u32> {
        sites.iter().map(|site| site.function).collect()
    }

    /// #1034: a trace cut at its limit keeps the innermost activations and the
    /// outermost ones (the entry frames), in unwinding order, and counts the
    /// ones between.
    #[test]
    fn a_full_trace_keeps_its_head_and_its_tail() {
        let (sites, omitted) = recorded(8, 8, 3);
        assert_eq!((functions(&sites), omitted), ((0..8).collect(), 0));
        let (sites, omitted) = recorded(9, 8, 3);
        assert_eq!((functions(&sites), omitted), (vec![0, 1, 2, 3, 4, 6, 7, 8], 1));
        for count in [10, 11, 12, 100, 101] {
            let (sites, omitted) = recorded(count, 8, 3);
            assert_eq!(functions(&sites), vec![0, 1, 2, 3, 4, count - 3, count - 2, count - 1], "{count}");
            assert_eq!(omitted, u64::from(count) - 8, "{count}");
        }
    }
}
