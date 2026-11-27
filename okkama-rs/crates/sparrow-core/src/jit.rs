//! Whole-function compilation to machine code (Cranelift), lazily on first call.
//!
//! Every function takes the context and its parameters as words and returns
//! one word (floats travel as bits). Calls go through the code table: a zero
//! entry compiles the callee first. Known ordinary entries already published
//! in this runtime can embed their stable address (see docs/ready-callees.md).
//! Integer and reference registers are
//! declared for stack maps unless every definition is proven pointer-free.
//! Every collecting call remains a precise safepoint; the
//! collector walks frame records (frame pointers are preserved) and reads the
//! spilled slots of each frame from the map of its return address.
#![allow(unsafe_code)]

use crate::Error;
use crate::heap::{self, Heap};
use crate::intrinsics::Intrinsic;
use crate::program::{Kind, dispatch_table, failure_or_region, generic, suspends};
use crate::runtime::{self, Ctx, FAILED, OK, Runtime};
use crate::word::{self, Repr, Word};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{AbiParam, Block, InstBuilder, MemFlagsData, Signature, StackSlotData, StackSlotKind, TrapCode, Value, types};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, ModuleReloc, default_libcall_names};
use okkama_sparrow::{Cmp, Constant, Instruction as I, Target, Type};
use std::collections::BTreeMap;

/// What a runtime helper may do, beyond computing its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Allocates heap objects.
    Allocates,
    /// May collect (a safepoint: the caller's frame is walked for roots).
    Collects,
    /// Runs machine code (compiled functions, destroys, async bodies).
    Reenters,
    /// May set a failure status, which machine code reads after the call or
    /// returns to its caller.
    Fails,
}

/// One description per helper (H-06): its Rust signature, checked at compile
/// time (the function must coerce to exactly that `extern "C"` pointer type;
/// each scalar is 8 bytes), the Cranelift signature derived from it, and its
/// effects. A panic inside a helper aborts with its message at the `extern
/// "C"` boundary and never unwinds through machine code.
macro_rules! helpers {
    ($(
        $(#[$meta:meta])* $field:ident = $function:path
        as ($ctx:ident $(, $arg:ident: $ty:ty)*) $(-> $ret:ty)? ; [$($effect:ident),*];
    )*) => {
        #[derive(Debug)]
        struct Helpers { $($(#[$meta])* $field: FuncId,)* }

        /// Every helper and its effects, in declaration order.
        pub const HELPERS: &[(&str, &[Effect])] = &[$($(#[$meta])* (stringify!($field), &[$(Effect::$effect),*]),)*];


        $(
            $(#[$meta])*
            const _: () = {
                $(assert!(std::mem::size_of::<$ty>() == 8
                    || (std::mem::size_of::<$ty>() == 16 && std::mem::align_of::<$ty>() == 8));)*
                $(assert!(std::mem::size_of::<$ret>() == 8);)?
            };
            $(#[$meta])*
            const _: extern "C" fn(*mut Ctx $(, $ty)*) $(-> $ret)? = $function;
        )*

        fn declare_helpers(builder: &mut JITBuilder) {
            $($(#[$meta])* builder.symbol(concat!("sparrow_core_", stringify!($field)), $function as *const u8);)*
        }

        fn import_helpers(module: &mut JITModule) -> Result<Helpers, Error> {
            Ok(Helpers { $($(#[$meta])* $field: {
                let mut signature = module.make_signature();
                signature.params.push(AbiParam::new(types::I64));
                $(let _ = stringify!($arg);
                    for _ in 0..std::mem::size_of::<$ty>() / 8 {
                        signature.params.push(AbiParam::new(types::I64));
                    }
                )*
                $(let _ = stringify!($ret); signature.returns.push(AbiParam::new(types::I64));)?
                module
                    .declare_function(concat!("sparrow_core_", stringify!($field)), Linkage::Import, &signature)
                    .map_err(|e| Error(format!("JIT helper: {e}")))?
            },)* })
        }

        impl Helpers {
            /// The ids in `HELPERS` order.
            fn ids(&self) -> Vec<FuncId> {
                vec![$($(#[$meta])* self.$field),*]
            }
        }
    };
}

helpers! {
    allocate = runtime::rt_allocate as (ctx, words: u64) -> Word; [Allocates, Collects];
    poll = runtime::rt_poll as (ctx); [Fails];
    compile = runtime::rt_compile as (ctx, function: u64) -> u64; [Fails];
    int = runtime::rt_int as (ctx, op: u64, left: Word, right: Word) -> Word; [Allocates, Collects, Fails];
    int_order = runtime::rt_int_order as (ctx, left: Word, right: Word) -> i64; [];
    concat = runtime::rt_concat as (ctx, left: Word, right: Word) -> Word; [Allocates, Collects, Fails];
    array = runtime::rt_array as (ctx, repr: u64, elements: *const Word, count: u64) -> Word; [Allocates, Collects];
    trap = runtime::rt_trap as (ctx, message: Word) -> Word; [Allocates, Fails];
    stack_overflow = runtime::rt_stack_overflow as (ctx) -> Word; [Allocates, Fails];
    throw = runtime::rt_throw as (ctx, value: Word) -> Word; [Allocates, Fails];
    trace = runtime::rt_trace as (ctx, function: u64, pc: u64, frame: u64); [];
    native = runtime::rt_native as (ctx, function: u64, arguments: *const Word, count: u64) -> Word; [Allocates, Collects, Fails];
    float = runtime::rt_float as (ctx, op: u64, left: u64, right: u64) -> u64; [Allocates, Fails];
    exec = crate::ops::rt_exec as (ctx, function: u64, pc: u64, uses: *const Word, registers: *const okkama_sparrow::Register, count: u64) -> Word; [Allocates, Collects, Reenters, Fails];
    call_closure = runtime::rt_call_closure as (ctx, closure: Word, arguments: *const Word, count: u64) -> Word; [Allocates, Collects, Reenters, Fails];
    map_find = runtime::rt_map_find as (ctx, function: u64, pc: u64, map: Word, key: Word) -> u64; [];
    intrinsic = crate::intrinsics::rt_intrinsic as (ctx, function: u64, arguments: *const Word, count: u64) -> Word; [Allocates, Collects, Reenters, Fails];
    failure_op = crate::failures::rt_failure_op as (ctx, function: u64, pc: u64, uses: *const Word) -> Word; [Allocates, Collects, Reenters, Fails];
    catch = crate::failures::rt_catch as (ctx, function: u64, pc: u64, failure: Word) -> u64; [Allocates];
    created = crate::failures::rt_created as (ctx, descriptor: u64, object: Word); [];
    region_add = crate::failures::rt_region_add as (ctx, region: Word, value: Word, own: u64) -> Word; [Allocates];
    region_new = crate::failures::rt_region_new as (ctx) -> Word; [Allocates];
    region_leave = crate::failures::rt_region_leave as (ctx, region: Word, keep: *const Word, count: u64); [Allocates, Collects, Reenters, Fails];
    resource_pin = crate::failures::rt_resource_pin as (ctx, object: Word); [];
    dev_site = crate::failures::rt_dev_site as (ctx, kind: u64, function: u64, pc: u64); [];
    resource_leave = crate::failures::rt_resource_leave as (ctx, object: Word); [Allocates, Collects, Reenters, Fails];
    builder_push = crate::ops::rt_builder_push as (ctx, builder: Word, item: Word) -> Word; [];
    builder_new = crate::ops::rt_builder_new as (ctx, repr: u64) -> Word; [Allocates, Collects];
    builder_finish = crate::ops::rt_builder_finish as (ctx, builder: Word, packed: u64) -> Word; [Allocates, Collects, Fails];
    rat_new = crate::ops::rt_rat_new as (ctx, left: Word, right: Word) -> Word; [Allocates, Collects, Fails];
    map_len = crate::ops::rt_map_len as (ctx, map: Word) -> Word; [Allocates, Collects];
    trie_get = crate::ops::rt_trie_get as (ctx, array: Word, position: u64) -> Word; [Allocates];
    int_text = crate::ops::rt_int_text as (ctx, word: Word) -> Word; [Allocates, Collects, Fails];
    future_new = crate::tasks::rt_future_new as (ctx, function: u64, arguments: *const Word, count: u64) -> Word; [Allocates, Collects, Fails];
    await_take = crate::tasks::rt_await_take as (ctx, this: Word, future: Word) -> u64; [Collects, Reenters, Fails];
    await_ = crate::tasks::rt_await as (ctx, this: Word, future: Word) -> u64; [Collects, Reenters, Fails];
    region_settle = crate::tasks::rt_region_settle as (ctx, this: Word, region: Word, keep: Word, failure: Word) -> u64; [Allocates, Collects, Reenters, Fails];
    resource_settle = crate::tasks::rt_resource_settle as (ctx, this: Word, object: Word, failure: Word) -> u64; [Allocates, Collects, Reenters, Fails];
    complete = crate::tasks::rt_complete as (ctx, frame: Word, value: Word); [Collects, Reenters];
    spawn = crate::tasks::rt_spawn as (ctx, future: Word) -> Word; [Reenters, Fails];
    fut = crate::tasks::rt_fut as (ctx, op: u64, future: Word) -> Word; [Reenters, Fails];
}

const COMPILE_HEADER: &str = "; compile\tfunction\tinstructions\tregisters\tsuspensions\tclif blocks\tclif instructions\tsafepoints\tcode bytes\tframe bytes\tdeclare\tliveness\temit\tfinalize\tcodegen\tmaps\tdefinitions\ttables (microseconds)";

pub struct Jit {
    module: JITModule,
    helpers: Helpers,
    ids: BTreeMap<usize, FuncId>,
    /// Stack map of each call site: return address, the frame's FP-to-SP
    /// distance, and the SP offsets of its live words.
    maps: Vec<(u64, u32, Vec<u32>)>,
    /// Address ranges of compiled functions, sorted by start.
    ranges: Vec<(u64, u64)>,
    /// (role, slot) → function index by object descriptor (u64::MAX: none).
    dispatch: BTreeMap<(u32, u32), Box<[u64]>>,
    /// #588: the object capacity of a session runtime. Machine code embeds a dispatch table's address and indexes
    /// it by descriptor, so a session's tables are this long from the start and an extension fills new entries in
    /// place; 0 outside a session.
    pub(crate) object_capacity: usize,
    /// The registers each `rt_exec` call site passes, in visit order: the
    /// helper reads them instead of visiting the instruction on every call
    /// (#325). Machine code holds their addresses, so they live as long.
    operands: Vec<Box<[okkama_sparrow::Register]>>,
    /// Calls of each helper (`HELPERS` order) from code compiled under
    /// `--stats`; machine code increments them, other code does not count.
    helper_counts: Box<[u64]>,
    /// The largest machine frame compiled, bytes (`--stats`).
    largest_frame: u32,
    /// Functions whose CLIF and machine code go to stderr (`SPARROW_CORE_DUMP`).
    dump: Chosen,
    /// Functions compiled without Cranelift's optimizations, and the target
    /// that does it (`SPARROW_CORE_UNOPTIMIZED`).
    unoptimized: Chosen,
    unoptimized_isa: Option<std::sync::Arc<dyn cranelift_codegen::isa::TargetIsa>>,
    /// Functions compiled without the core's inline fast paths: each
    /// instruction that has one runs through the shared slow path instead
    /// (`SPARROW_CORE_PLAIN`).
    plain: Chosen,
    /// Keep lazy dispatch for selected callers (`SPARROW_CORE_OLD_DISPATCH`).
    old_dispatch: Chosen,
    /// Callers compiled without inlining (`SPARROW_CORE_NO_INLINE`, #927), for differential runs.
    no_inline: Chosen,
    #[cfg(test)]
    emitted: BTreeMap<usize, cranelift_codegen::ir::Function>,
    /// Async bodies whose stack maps Cranelift's frontend keeps, as before
    /// #354 (`SPARROW_CORE_FRONTEND_MAPS`); synchronous functions always do
    /// (#362).
    frontend_maps: Chosen,
    /// Allocations of each inline allocation site compiled under `--stats`:
    /// its label and the count and bytes machine code adds there (#55).
    alloc_sites: Vec<(String, Box<[u64; 2]>)>,
    /// Where compilation spends its time (`SPARROW_CORE_PROFILE_COMPILE=1`,
    /// #345): a line per function on stderr, and Cranelift's own passes,
    /// summed, when the JIT is dropped.
    profile: Option<cranelift_codegen::timing::PassTimes>,
    /// #1074: the adapter that calls compiled code of each arity over 8 from Rust (see [`Jit::spread`]).
    spreads: BTreeMap<usize, u64>,
}

impl Drop for Jit {
    fn drop(&mut self) {
        if let Some(passes) = &self.profile {
            eprintln!("; Cranelift passes, all compiled functions\n{passes}");
        }
    }
}

/// Functions chosen by symbol in an environment variable: a comma-separated
/// list, or `*` for all. Bisecting a wrong result over these lists narrows it
/// to one compiled function without rebuilding (#44).
#[derive(Debug, Default)]
struct Chosen {
    all: bool,
    symbols: std::collections::BTreeSet<String>,
}

impl Chosen {
    fn from_env(variable: &str) -> Self {
        let Some(text) = std::env::var_os(variable) else {
            return Self::default();
        };
        let text = text.to_string_lossy();
        let symbols: std::collections::BTreeSet<String> =
            text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
        Self { all: symbols.contains("*"), symbols }
    }

    fn any(&self) -> bool {
        self.all || !self.symbols.is_empty()
    }

    fn has(&self, symbol: &str) -> bool {
        self.all || self.symbols.contains(symbol)
    }
}

impl Jit {
    /// #588: fill the dispatch entries of the objects an extension appended, in place (the tables are as long as
    /// the session's object capacity).
    pub(crate) fn extend_dispatch(&mut self, program: &okkama_sparrow::Program, from: usize) {
        for (&(role, slot), table) in &mut self.dispatch {
            for (descriptor, object) in program.objects.iter().enumerate().skip(from) {
                if let Some(witness) = object.does.iter().find(|w| w.role.0 == role) {
                    table[descriptor] = u64::from(witness.slots[slot as usize].0);
                }
            }
        }
    }
}

/// The target for compiled code, at a Cranelift optimization level.
fn target(opt_level: &str) -> Result<std::sync::Arc<dyn cranelift_codegen::isa::TargetIsa>, Error> {
    let mut flags = settings::builder();
    let set = |flags: &mut settings::Builder, name: &str, value: &str| {
        flags.set(name, value).map_err(|e| Error(format!("JIT flag {name}: {e}")))
    };
    set(&mut flags, "opt_level", opt_level)?;
    set(&mut flags, "is_pic", "false")?;
    set(&mut flags, "use_colocated_libcalls", "false")?;
    // The collector walks frame records to find each frame's stack map.
    set(&mut flags, "preserve_frame_pointers", "true")?;
    // Cranelift's IR verifier checks the emitter, not the program: a quarter
    // of the compile time of a large async program (#345). Debug builds,
    // and so the gate's debug tests, keep it; `SPARROW_CORE_VERIFY=1` turns
    // it on in a release build.
    let verify = cfg!(debug_assertions)
        || crate::env_switch::enabled("SPARROW_CORE_VERIFY");
    set(&mut flags, "enable_verifier", if verify { "true" } else { "false" })?;
    cranelift_native::builder()
        .map_err(|e| Error(format!("JIT target: {e}")))?
        .finish(settings::Flags::new(flags))
        .map_err(|e| Error(format!("JIT target: {e}")))
}

/// Largest machine frame of a compiled function: a quarter of the stack
/// headroom (`STACK_HEADROOM`), leaving the rest to the Rust helpers below the
/// deepest machine frame.
pub const MAX_FRAME: u64 = crate::runtime::STACK_HEADROOM / 4;

/// Memory compiling one function may take (#73). Machine-code liveness and
/// register allocation keep a set per Cranelift block, so the cost is about
/// Σ registers live at an instruction × blocks it emits — quadratic for k
/// values live across k fallible instructions. A memory budget lowers it.
pub const MAX_COMPILE_BYTES: u64 = 256 << 20;
/// Compiler bytes per live register in one block (measured: about 58).
const BYTES_PER_LIVE_BLOCK: u64 = 64;
/// No instruction emits more blocks than this (inline allocation, checks).
const MAX_BLOCKS_PER_INSTRUCTION: u64 = 16;

/// The compile-cost limit for its refusal (#1066): whole MiB from 1 MiB, KiB
/// below (a budget under 1 MiB printed "more than 0 MiB"), bytes below 1 KiB.
fn compile_limit_text(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} bytes"),
        1024..0x10_0000 => format!("{} KiB", bytes >> 10),
        _ => format!("{} MiB", bytes >> 20),
    }
}

impl std::fmt::Debug for Jit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Jit")
            .field("functions", &self.ids.len())
            .field("safepoints", &self.maps.len())
            .finish_non_exhaustive()
    }
}

impl Jit {
    pub fn new() -> Result<Self, Error> {
        // The layout perturbation of the micro A/B (#933, `code_memory`).
        let shift = crate::env_switch::bytes("SPARROW_CORE_CODE_SHIFT", crate::code_memory::MAX_SHIFT).map_err(Error)?;
        Self::with_code_shift(shift.unwrap_or(0))
    }

    /// A JIT whose functions start `shift` bytes past their pages (#933).
    pub(crate) fn with_code_shift(shift: usize) -> Result<Self, Error> {
        let isa = target("speed")?;
        let unoptimized = Chosen::from_env("SPARROW_CORE_UNOPTIMIZED");
        let unoptimized_isa = if unoptimized.any() { Some(target("none")?) } else { None };
        let mut builder = JITBuilder::with_isa(isa, default_libcall_names());
        builder.memory_provider(Box::new(crate::code_memory::CodeMemory::new(shift)));
        declare_helpers(&mut builder);
        let mut module = JITModule::new(builder);
        let helpers = import_helpers(&mut module)?;
        Ok(Self {
            module,
            helpers,
            ids: BTreeMap::new(),
            maps: Vec::new(),
            ranges: Vec::new(),
            dispatch: BTreeMap::new(),
            object_capacity: 0,
            operands: Vec::new(),
            helper_counts: vec![0; HELPERS.len()].into_boxed_slice(),
            largest_frame: 0,
            dump: Chosen::from_env("SPARROW_CORE_DUMP"),
            unoptimized,
            unoptimized_isa,
            plain: Chosen::from_env("SPARROW_CORE_PLAIN"),
            old_dispatch: Chosen::from_env("SPARROW_CORE_OLD_DISPATCH"),
            no_inline: Chosen::from_env("SPARROW_CORE_NO_INLINE"),
            #[cfg(test)]
            emitted: BTreeMap::new(),
            frontend_maps: Chosen::from_env("SPARROW_CORE_FRONTEND_MAPS"),
            alloc_sites: Vec::new(),
            profile: crate::env_switch::enabled("SPARROW_CORE_PROFILE_COMPILE")
                .then(|| {
                    eprintln!("{COMPILE_HEADER}");
                    cranelift_codegen::timing::PassTimes::default()
                }),
            spreads: BTreeMap::new(),
        })
    }

    /// #1074: code `extern "C" fn(ctx, words: *const Word, code) -> Word` that calls `code`, a function
    /// compiled here taking `arity` words after the context, with the `arity` words at `words`. Rust calls
    /// compiled code through a signature it names, which [`invoke`] does for up to 8 words; a wider call
    /// (a closure over a function of many parameters) goes through this adapter, one per arity, compiled on
    /// its first use. It holds no value across its call, and the collector passes over its frame as over a
    /// Rust one: it is not in `ranges`.
    pub(crate) fn spread(&mut self, arity: usize) -> Result<u64, Error> {
        if let Some(&code) = self.spreads.get(&arity) {
            return Ok(code);
        }
        let mut signature = self.module.make_signature();
        for _ in 0..3 {
            signature.params.push(AbiParam::new(types::I64));
        }
        signature.returns.push(AbiParam::new(types::I64));
        let id = self
            .module
            .declare_function(&format!("spread{arity}"), Linkage::Local, &signature)
            .map_err(|e| Error(format!("JIT declare: {e}")))?;
        let mut context = self.module.make_context();
        context.func.signature = signature;
        let mut frontend = FunctionBuilderContext::new();
        {
            let config = self.module.target_config();
            let mut builder = FunctionBuilder::new(&mut context.func, &mut frontend);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            let (ctx, words, code) = (builder.block_params(entry)[0], builder.block_params(entry)[1], builder.block_params(entry)[2]);
            let mut values = Vec::with_capacity(1 + arity);
            values.push(ctx);
            for index in 0..arity {
                let offset = i32::try_from(index * 8).map_err(|_| Error(format!("a call of {arity} words is too wide")))?;
                values.push(builder.ins().load(types::I64, MEMORY, words, offset));
            }
            let callee = builder.import_signature(self.signature(arity));
            let call = builder.ins().call_indirect(callee, code, &values);
            let result = builder.inst_results(call)[0];
            builder.ins().return_(&[result]);
            builder.seal_all_blocks();
            builder.finalize(config);
        }
        self.module.define_function(id, &mut context).map_err(|e| Error(format!("spread{arity}: {e:?}")))?;
        self.module.finalize_definitions().map_err(|e| Error(format!("JIT finalize: {e}")))?;
        let code = self.module.get_finalized_function(id) as u64;
        self.spreads.insert(arity, code);
        Ok(code)
    }

    /// `--stats` lines of the helpers machine code called, and the largest
    /// machine frame.
    pub(crate) fn helper_calls(&self) -> Vec<(String, u64)> {
        HELPERS
            .iter()
            .zip(self.helper_counts.iter())
            .filter(|(_, count)| **count > 0)
            .map(|((name, _), &count)| (format!("helper {name}"), count))
            .chain([
                ("largest machine frame, bytes".to_string(), u64::from(self.largest_frame)),
                ("stack map sites".to_string(), self.maps.len() as u64),
                ("stack map root words, static sum".to_string(), self.maps.iter().map(|(_, _, slots)| slots.len() as u64).sum()),
            ])
            .chain(self.alloc_sites.iter().filter(|(_, counts)| counts[0] > 0).flat_map(|(site, counts)| {
                [(format!("alloc site {site}, count"), counts[0]), (format!("alloc site {site}, bytes"), counts[1])]
            }))
            .collect()
    }

    fn in_code(&self, address: u64) -> bool {
        let index = self.ranges.partition_point(|&(start, _)| start <= address);
        index > 0 && address < self.ranges[index - 1].1
    }

    /// Report the traced values of every machine frame above `frame`, the
    /// frame pointer of a runtime function called from machine code.
    ///
    /// SAFETY: frame records from `frame` up to `entry` are live frames; those
    /// returning into code compiled here have stack maps.
    pub(crate) unsafe fn frame_roots(&self, mut frame: u64, entry: u64, visit: &mut dyn FnMut(Word, Repr)) {
        // Rust frames between machine frames (a helper that calls a closure)
        // are passed over; the walk ends at the frame that entered machine code.
        // A deep stack repeats a few return addresses (recursion): a small direct-mapped cache of what each one
        // resolved to spares the two binary searches per frame (#917: deep, ~100 000 frames per collection).
        const NOT_CODE: u32 = u32::MAX;
        const NO_MAP: u32 = u32::MAX - 1;
        let mut cache = [(0u64, 0u32); 64];
        while frame != 0 && frame != entry {
            let (caller, ret) = unsafe { (*(frame as *const u64), *((frame + 8) as *const u64)) };
            let slot = &mut cache[((ret >> 2) & 63) as usize];
            let resolved = if slot.0 == ret && ret != 0 {
                slot.1
            } else {
                let resolved = if !self.in_code(ret) {
                    NOT_CODE
                } else {
                    match self.maps.binary_search_by_key(&ret, |(address, _, _)| *address) {
                        Ok(index) => index as u32,
                        Err(_) => NO_MAP,
                    }
                };
                *slot = (ret, resolved);
                resolved
            };
            if resolved == NOT_CODE {
                frame = caller;
                continue;
            }
            if resolved != NO_MAP {
                let index = resolved as usize;
                // The machine frame's SP at the call, from its own FP (`caller`):
                // the callee's frame record says nothing about where the caller's
                // SP was (Linux puts the record below the callee's saved registers).
                let sp = caller - u64::from(self.maps[index].1);
                for &offset in &self.maps[index].2 {
                    // Integer slots and pointers share one rule: a pointer is
                    // non-zero with a clear low bit.
                    visit(unsafe { *((sp + u64::from(offset)) as *const u64) }, Repr::Int);
                }
            }
            frame = caller;
        }
    }

    fn signature(&self, params: usize) -> Signature {
        let mut signature = self.module.make_signature();
        for _ in 0..=params {
            signature.params.push(AbiParam::new(types::I64));
        }
        signature.returns.push(AbiParam::new(types::I64));
        signature
    }
}

/// Call compiled code with the context and `words` as parameters: up to 8
/// words directly, more through [`Jit::spread`] (#1074).
///
/// SAFETY: `code` is a function compiled here taking exactly `words.len()`
/// parameters after the context.
pub(crate) unsafe fn invoke(code: u64, ctx: *mut Ctx, words: &[Word]) -> Word {
    type W = Word;
    let c = ctx;
    let w = words;
    unsafe {
        match w.len() {
            0 => std::mem::transmute::<u64, extern "C" fn(*mut Ctx) -> W>(code)(c),
            1 => std::mem::transmute::<u64, extern "C" fn(*mut Ctx, W) -> W>(code)(c, w[0]),
            2 => std::mem::transmute::<u64, extern "C" fn(*mut Ctx, W, W) -> W>(code)(c, w[0], w[1]),
            3 => std::mem::transmute::<u64, extern "C" fn(*mut Ctx, W, W, W) -> W>(code)(c, w[0], w[1], w[2]),
            4 => std::mem::transmute::<u64, extern "C" fn(*mut Ctx, W, W, W, W) -> W>(code)(c, w[0], w[1], w[2], w[3]),
            5 => std::mem::transmute::<u64, extern "C" fn(*mut Ctx, W, W, W, W, W) -> W>(code)(c, w[0], w[1], w[2], w[3], w[4]),
            6 => std::mem::transmute::<u64, extern "C" fn(*mut Ctx, W, W, W, W, W, W) -> W>(code)(c, w[0], w[1], w[2], w[3], w[4], w[5]),
            7 => std::mem::transmute::<u64, extern "C" fn(*mut Ctx, W, W, W, W, W, W, W) -> W>(code)(c, w[0], w[1], w[2], w[3], w[4], w[5], w[6]),
            8 => std::mem::transmute::<u64, extern "C" fn(*mut Ctx, W, W, W, W, W, W, W, W) -> W>(code)(c, w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]),
            // #1074: wider calls through the adapter of their arity.
            arity => {
                let runtime = &mut *(*c).runtime;
                match runtime.jit.spread(arity) {
                    Ok(spread) => std::mem::transmute::<u64, extern "C" fn(*mut Ctx, *const W, u64) -> W>(spread)(c, w.as_ptr(), code),
                    Err(error) => crate::runtime::fatal(&mut *c, runtime, error.0),
                }
            }
        }
    }
}

/// Compile `function` unless it is compiled; returns its code address.
/// Instructions where control enters other than by falling through: the
/// first, every jump/branch/lookup/catch target, every handler target and
/// every `await` (its resume point). Only these get their own Cranelift
/// block: a block per instruction made stack-map liveness and register
/// allocation quadratic in straight-line code with many live values (#73).
fn leaders(source: &okkama_sparrow::Function) -> Vec<bool> {
    let mut leaders = vec![false; source.code.len()];
    let mut mark = |target: &Target| {
        if let Some(leader) = leaders.get_mut(target.pc as usize) {
            *leader = true;
        }
    };
    for instruction in &source.code {
        match instruction {
            I::Jump { target } => mark(target),
            I::Branch { then_target, else_target, .. } => {
                mark(then_target);
                mark(else_target);
            }
            I::Catch { handler, next, .. } => {
                mark(handler);
                mark(next);
            }
            I::ArrayLookup { found, missing, .. } | I::MapLookup { found, missing, .. } => {
                mark(found);
                mark(missing);
            }
            _ => {}
        }
    }
    for handler in &source.handlers {
        mark(&handler.target);
    }
    for (pc, instruction) in source.code.iter().enumerate() {
        if suspends(instruction) {
            leaders[pc] = true;
        }
    }
    if let Some(first) = leaders.first_mut() {
        *first = true;
    }
    leaders
}


pub fn compile(runtime: &mut Runtime, function: usize) -> Result<u64, Error> {
    if runtime.invocations[function].code != 0 {
        return Ok(runtime.invocations[function].code);
    }
    let _span = okkama_devtrace::span!("jit", "compile");
    okkama_devtrace::count!("jit", "functions compiled");
    let closure_reference = runtime.closure_bridge_reference;
    // #1129: the polling of this runtime's code, as the interpreter reads it.
    let polled = runtime.polled();
    let Runtime { heap, loaded, jit, invocations, constants, limits, budget, stats, .. } = runtime;
    let resources = &runtime.resources;
    let started = stats.is_some().then(std::time::Instant::now);
    // Stage boundaries under `SPARROW_CORE_PROFILE_COMPILE` (#345).
    let profiling = jit.profile.is_some();
    let mut marks: Vec<std::time::Instant> = Vec::new();
    macro_rules! mark {
        () => {
            if profiling {
                marks.push(std::time::Instant::now());
            }
        };
    }
    mark!();
    let program = &loaded.program;
    let source = &program.functions[function];
    let symbol = source.symbol.as_str();
    if source.is_async && (source.native.is_some() || source.intrinsic.is_some()) {
        return Err(Error(format!("{}: async natives and intrinsics are driven, not compiled", source.symbol)));
    }
    // An async body takes its frame (see `tasks`).
    let params = if source.is_async { 1 } else { source.params.len() };
    let id = match jit.ids.get(&function) {
        Some(id) => *id,
        None => {
            let signature = jit.signature(params);
            let id = jit
                .module
                .declare_function(&format!("f{function}"), Linkage::Local, &signature)
                .map_err(|e| Error(format!("JIT declare: {e}")))?;
            jit.ids.insert(function, id);
            id
        }
    };
    mark!();
    // Live registers per instruction, for the cost of large functions only:
    // a function whose instructions × registers × blocks cannot reach the
    // limit skips the analysis.
    let cost_limit = limits.memory.map_or(MAX_COMPILE_BYTES, |memory| memory.min(MAX_COMPILE_BYTES)) / BYTES_PER_LIVE_BLOCK;
    let bound = (source.code.len() as u64)
        .saturating_mul(source.registers.len() as u64)
        .saturating_mul(MAX_BLOCKS_PER_INSTRUCTION);
    let live = if bound > cost_limit {
        Some(okkama_sparrow::live_counts(source).ok_or_else(|| {
            Error(format!("{}: too large to compile ({} instructions, {} registers)", source.symbol, source.code.len(), source.registers.len()))
        })?)
    } else {
        None
    };
    mark!();
    let plain = jit.plain.has(symbol);
    let late_bound = plain || jit.old_dispatch.has(symbol) || jit.unoptimized.has(symbol);
    let no_inline = late_bound || jit.no_inline.has(symbol);
    // Explicit roots for async bodies (#354), unless the analysis is too
    // large: there Cranelift's reloads after every call made more than half
    // of the code. A synchronous function keeps Cranelift's stack maps, which
    // cost a third less in call-heavy code such as recursion (#362).
    let explicit = source.is_async && !jit.frontend_maps.has(symbol);
    // #552: a big async body keeps every register in its heap frame (stored at definition, loaded at use), so a
    // suspension copies nothing; otherwise each await saves and reloads every register, O(awaits × registers).
    let suspensions = source.code.iter().filter(|instruction| suspends(instruction)).count();
    let resident = frame_resident(source.is_async, suspensions, source.registers.len(), plain);
    let frame_live = (source.is_async && !resident).then(|| okkama_sparrow::live_sets(source).map(std::sync::Arc::new)).flatten();
    let root_live = if explicit && !resident { frame_live.clone() } else { None };
    let config = jit.module.target_config();
    let mut context = jit.module.make_context();
    context.func.signature = jit.signature(params);
    let mut frontend = FunctionBuilderContext::new();
    let call_sites;
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut frontend);
        let mut emit = Emitter {
            builder: &mut builder,
            heap,
            loaded,
            jit,
            invocations,
            late_bound,
            constants,
            resources,
            function,
            kinds: &loaded.functions[function].kinds,
            vars: Vec::new(),
            blocks: Vec::new(),
            ctx: None,
            frame: None,
            polled,
            count_helpers: stats.is_some(),
            closure_reference,
            fuel: budget.fuel.address(),
            refusable: limits.memory.is_some(),
            pc: 0,
            spill_area: None,
            live,
            cost: 0,
            cost_limit,
            plain,
            root_live,
            frame_live,
            resident,
            roots: None,
            call_sites: [0; 3],
            inline: None,
            inlined: 0,
            no_inline,
            exits: BTreeMap::new(),
        };
        emit.body()?;
        call_sites = emit.call_sites;
        mark!();
        builder.seal_all_blocks();
        builder.finalize(config);
    }
    mark!();
    // Sample before codegen, only for the diagnostic report (#481).
    let clif = profiling.then(|| {
        #[cfg(test)]
        profile_tests::SAMPLES.with(|samples| samples.set(samples.get() + 1));
        (context.func.layout.blocks().count(), context.func.dfg.num_insts())
    });
    #[cfg(test)]
    jit.emitted.insert(function, context.func.clone());
    let dump = jit.dump.has(symbol);
    if dump {
        // Callees by their CLIF names: the runtime's helpers and the program's functions.
        let callees: String = context
            .func
            .params
            .user_named_funcs()
            .values()
            .map(|name| {
                let id = FuncId::from_u32(name.index);
                let linkage = jit.module.declarations().get_function_decl(id).linkage_name(id).into_owned();
                let callee = match linkage.strip_prefix('f').and_then(|n| n.parse::<usize>().ok()) {
                    Some(n) => program.functions[n].symbol.clone(),
                    None => linkage,
                };
                format!(";   u{}:{} = {callee}\n", name.namespace, name.index)
            })
            .collect();
        eprintln!("; CLIF of {symbol} (f{function}), before Cranelift's passes\n{callees}{}", context.func.display());
        context.set_disasm(true);
    }
    match &jit.unoptimized_isa {
        Some(isa) if jit.unoptimized.has(symbol) => {
            // What `define_function` does, with the unoptimizing target.
            context.compile(&**isa, &mut Default::default()).map_err(|e| Error(format!("{symbol}: {:?}", e.inner)))?;
            let compiled = context.compiled_code().expect("compiled function");
            let alignment = u64::from(compiled.buffer.min_alignment);
            let relocs: Vec<ModuleReloc> =
                compiled.buffer.relocs().iter().map(|reloc| ModuleReloc::from_mach_reloc(reloc, &context.func, id)).collect();
            jit.module
                .define_function_bytes(id, alignment, compiled.code_buffer(), &relocs)
                .map_err(|e| Error(format!("{symbol}: {e:?}")))?;
        }
        _ => jit.module.define_function(id, &mut context).map_err(|e| Error(format!("{symbol}: {e:?}")))?,
    }
    mark!();
    let compiled = context.compiled_code().expect("compiled function");
    let maps: Vec<(u32, Vec<u32>)> = compiled
        .buffer
        .user_stack_maps()
        .iter()
        .map(|(ret, _, map)| (*ret, map.entries().map(|(_, offset)| offset).collect()))
        .collect();
    // Distance from the frame's FP down to its SP inside the body.
    let fp_to_sp = compiled.buffer.frame_layout().map_or(0, |layout| layout.frame_to_fp_offset);
    // The prologue compares SP with the stack limit after reserving the
    // frame; a frame within the headroom below that limit cannot reach the
    // guard page before the comparison fails (#25).
    if u64::from(fp_to_sp) > MAX_FRAME {
        return Err(Error(format!(
            "{}: machine frame of {fp_to_sp} bytes exceeds the {} KiB limit",
            source.symbol,
            MAX_FRAME >> 10
        )));
    }
    jit.largest_frame = jit.largest_frame.max(fp_to_sp);
    let size = compiled.code_info().total_size;
    mark!();
    jit.module
        .finalize_definitions()
        .map_err(|e| Error(format!("JIT finalize: {e}")))?;
    mark!();
    let safepoints = maps.len();
    let (root_words, traced, omitted) = if dump || profiling || stats.is_some() {
        let root_words: usize = maps.iter().map(|(_, words)| words.len()).sum();
        let info = &loaded.functions[function];
        let omitted = info.kinds.iter().zip(&info.pointer_free).filter(|(kind, free)| kind.traced() && **free).count();
        let traced = info.kinds.iter().filter(|kind| kind.traced()).count() - omitted;
        (root_words, traced, omitted)
    } else {
        (0, 0, 0)
    };
    if dump || profiling {
        eprintln!("; roots\t{symbol}\ttraced registers {traced}\tomitted registers {omitted}\tstack-map words {root_words}");
    }
    let address = jit.module.get_finalized_function(id) as u64;
    okkama_devtrace::event!("jit", "code placed", fn = symbol, address = format!("{address:#x}"), bytes = size);
    register_code(&mut jit.maps, &mut jit.ranges, address, size, fp_to_sp, maps);
    // Publish only after finalization and registration of maps/ranges. All
    // static/role/async/closure users read this same stable record.
    invocations[function].code = address;
    mark!();
    if let Some(passes) = jit.profile.as_mut() {
        let clif = clif.expect("profiling sampled CLIF before codegen");
        passes.add(&cranelift_codegen::timing::take_current());
        let micros: Vec<String> = marks.windows(2).map(|pair| (pair[1] - pair[0]).as_micros().to_string()).collect();
        report_compile(format_args!(
            "; compile\t{symbol}\t{}\t{}\t{}\t{}\t{}\t{safepoints}\t{size}\t{fp_to_sp}\t{}",
            source.code.len(),
            source.registers.len(),
            source.code.iter().filter(|instruction| suspends(instruction)).count(),
            clif.0,
            clif.1,
            micros.join("\t")
        ));
    }
    if dump && let Some(listing) = &compiled.vcode {
        eprintln!("; machine code of {symbol} at {address:#x}, {size} bytes, frame {fp_to_sp} bytes\n{listing}");
    }
    if let (Some(stats), Some(started)) = (stats, started) {
        // Emitted sites, not executed calls; no machine-code instrumentation.
        for (kind, count) in ["ready static", "lazy static", "dynamic"].into_iter().zip(call_sites) {
            *stats.entry(format!("call dispatch {kind}, sites")).or_default() += count;
        }
        let micros = started.elapsed().as_micros() as u64;
        for (key, value) in [
            (format!("compile {symbol}, microseconds"), micros),
            (format!("machine code {symbol}, bytes"), u64::from(size)),
            (format!("roots {symbol}, traced registers"), traced as u64),
            (format!("roots {symbol}, omitted registers"), omitted as u64),
            (format!("roots {symbol}, stack-map words"), root_words as u64),
            ("compile, microseconds".to_string(), micros),
            ("machine code, bytes".to_string(), u64::from(size)),
        ] {
            *stats.entry(key).or_default() += value;
        }
    }
    Ok(address)
}

/// Register a compiled function's stack maps and code range (#1108). A function's code is one range that no other
/// function's overlaps, so its maps go in as one sorted batch at their place in the address order (a binary search
/// and a splice), without sorting every map of the runtime again; code addresses need not grow from one function
/// to the next.
fn register_code(
    maps: &mut Vec<(u64, u32, Vec<u32>)>,
    ranges: &mut Vec<(u64, u64)>,
    address: u64,
    size: u32,
    fp_to_sp: u32,
    batch: Vec<(u32, Vec<u32>)>,
) {
    let end = address + u64::from(size);
    let mut batch: Vec<(u64, u32, Vec<u32>)> =
        batch.into_iter().map(|(ret, slots)| (address + u64::from(ret), fp_to_sp, slots)).collect();
    batch.sort_unstable_by_key(|(at, _, _)| *at);
    let at = maps.partition_point(|(entry, _, _)| *entry < address);
    debug_assert!(maps.get(at).is_none_or(|(entry, _, _)| *entry >= end), "code ranges overlap");
    maps.splice(at..at, batch);
    let at = ranges.partition_point(|range| range.0 < address);
    ranges.insert(at, (address, end));
}

fn report_compile(report: std::fmt::Arguments<'_>) {
    #[cfg(test)]
    profile_tests::REPORTS.with(|reports| reports.borrow_mut().push(report.to_string()));
    eprintln!("{report}");
}

struct Emitter<'a, 'b> {
    builder: &'a mut FunctionBuilder<'b>,
    heap: &'a mut Heap,
    loaded: &'a crate::program::Loaded,
    jit: &'a mut Jit,
    invocations: &'a [runtime::Invocation],
    /// Reference dispatch for forced-old/plain/unoptimized callers.
    late_bound: bool,
    /// Emitted ready-static, lazy-static and dynamic sites (`--stats`).
    call_sites: [u64; 3],
    constants: &'a mut Vec<(Word, Repr)>,
    resources: &'a crate::failures::Resources,
    function: usize,
    kinds: &'a [Kind],
    vars: Vec<Variable>,
    /// The block of each instruction where control enters other than by
    /// falling through (`leaders`); straight-line instructions share one.
    blocks: Vec<Option<Block>>,
    /// Registers live before each instruction, when the function is large
    /// enough to need the compile-cost check (#73).
    live: Option<Vec<u32>>,
    /// Σ live registers × blocks emitted so far, and its limit.
    cost: u64,
    cost_limit: u64,
    /// Instructions with an inline fast path take the shared slow path
    /// (`SPARROW_CORE_PLAIN`).
    plain: bool,
    ctx: Option<Value>,
    /// The frame variable of an async body.
    frame: Option<Variable>,
    /// Budget polls at entry and on back-edges (`budget`).
    polled: bool,
    /// Count helper calls (`--stats`).
    count_helpers: bool,
    /// `SPARROW_CORE_CLOSURE_BRIDGE=reference`: every closure call goes through the helper.
    closure_reference: bool,
    /// Address of the runtime's fuel cell.
    fuel: i64,
    /// Allocation slow paths may refuse under a memory budget.
    refusable: bool,
    /// The instruction being emitted.
    pc: usize,
    /// Stack slot of `spill`, created on first use.
    spill_area: Option<cranelift_codegen::ir::StackSlot>,
    /// Registers live before each instruction, for explicit roots (#354);
    /// `None`: Cranelift's frontend keeps the stack maps.
    root_live: Option<std::sync::Arc<okkama_sparrow::LiveSets>>,
    /// Live registers at suspension, independently of the stack-map strategy.
    frame_live: Option<std::sync::Arc<okkama_sparrow::LiveSets>>,
    /// #552: every register lives in the heap frame (a big async body).
    resident: bool,
    roots: Option<Roots>,
    /// The callee being inlined, if any (#926).
    inline: Option<Inline>,
    /// Instructions inlined into this function so far (#927).
    inlined: usize,
    /// No inlining into this function (`SPARROW_CORE_NO_INLINE`, plain and late-bound callers).
    no_inline: bool,
    /// #1108: the shared failure exit of each (function, inlined activation, handler), entered with the pc.
    exits: BTreeMap<(usize, Option<Block>, Option<usize>), Block>,
}

/// An inlined callee's exits (#926): its `ret` jumps to `ok` with the result; a failure or a stop jumps to
/// `fail`, where the caller's own check after the call takes over, as after a real call.
#[derive(Clone, Copy)]
struct Inline {
    ok: Block,
    fail: Block,
    depth: u32,
}

/// Does `source` jump backwards (a loop)? Such a callee is not inlined: its loop amortizes the call, and moved into
/// a bigger caller it competes for the caller's registers (0.6.1 micro `float`: 9.4 -> 15.5 ns on Linux when
/// `Bench.run`'s loop was inlined into `main`).
fn has_loop(source: &okkama_sparrow::Function) -> bool {
    let mut backwards = false;
    for (pc, instruction) in source.code.iter().enumerate() {
        instruction.visit(&mut |access| {
            if let okkama_sparrow::Access::Target(target) = access
                && target.pc as usize <= pc
            {
                backwards = true;
            }
        });
    }
    backwards
}

/// The user trap after the float slow path's `check` (#1105): that helper always fails, so reaching the trap is a
/// broken invariant of `scalar::float_binary`, never a program's failure.
const FLOAT_SLOW_PATH_RETURNED: u8 = 1;

/// The user trap after a shared slow path that always fails on the inputs that reach it (#1106: a byte shift
/// count outside 0..7, sqrt of a negative float).
const GENERIC_SLOW_PATH_RETURNED: u8 = 2;

/// Array literals of at most this many elements are allocated inline (#1167); longer ones keep the helper.
const INLINE_ARRAY_LITERAL: usize = 16;

/// A shift or rotation count checked inline (#1106).
#[derive(Clone, Copy)]
struct Count {
    valid: Value,
    value: Value,
}

/// Largest callee inlined, in instructions, and the most instructions inlined into one function (#927).
const INLINE_CALLEE: usize = 24;
const INLINE_TOTAL: usize = 160;

/// GC roots kept by the core instead of Cranelift's frontend (#354). The
/// heap does not move, so a pointer never has to be reloaded after a
/// collection; what a collection needs is to find it. Every traced register
/// has a word in one frame slot, written at each definition of the
/// register, and a call reports the words of the registers live before its
/// instruction or written by it. The frontend's scheme stores a value at
/// its definition too, but reloads it at every use after any call, which
/// made more than half of the code of large async bodies.
///
/// No reported word may hold a stale pointer: a live register was written
/// on every path in this activation; a register the instruction may write
/// but that is not live before it is cleared when the instruction starts;
/// an async body clears every word at entry, as a resumption enters an
/// instruction in the middle.
struct Roots {
    slot: cranelift_codegen::ir::StackSlot,
    /// The word of each traced register.
    word: Vec<Option<u32>>,
    /// The word of an async body's frame.
    frame: Option<u32>,
    /// The words a call reports now.
    reported: Vec<u32>,
}



const MEMORY: MemFlagsData = MemFlagsData::trusted();

/// A constant table index is not enough: only the bytecode's static call
/// instructions establish the ordinary entry ABI. Role targets stay dynamic.
enum CallTarget {
    Static(usize),
    Dynamic(Value),
}

impl Emitter<'_, '_> {
    fn ctx(&self) -> Value {
        self.ctx.expect("context")
    }

    fn helper(&mut self, id: fn(&Helpers) -> FuncId, args: &[Value]) -> Option<Value> {
        let id = id(&self.jit.helpers);
        if self.count_helpers {
            let index = self.jit.helpers.ids().iter().position(|known| *known == id).expect("helper");
            let counter = self.iconst(&self.jit.helper_counts[index] as *const u64 as i64);
            let count = self.builder.ins().load(types::I64, MEMORY, counter, 0);
            let count = self.builder.ins().iadd_imm_s(count, 1);
            self.builder.ins().store(MEMORY, count, counter, 0);
        }
        let reference = self.jit.module.declare_func_in_func(id, self.builder.func);
        let call = self.builder.ins().call(reference, args);
        self.report_roots(call);
        self.builder.inst_results(call).first().copied()
    }

    /// The roots a collection in `call` finds: the words of [`Roots`].
    fn report_roots(&mut self, call: cranelift_codegen::ir::Inst) {
        if let Some(roots) = &self.roots {
            for &word in &roots.reported {
                self.builder.func.dfg.append_user_stack_map_entry(
                    call,
                    cranelift_codegen::ir::UserStackMapEntry { ty: types::I64, slot: roots.slot, offset: word * 8 },
                );
            }
        }
    }

    /// Instruction `pc` starts: what its calls report, and its written
    /// registers that are not live cleared, so that none holds a stale
    /// pointer.
    fn enter_roots(&mut self, pc: usize) {
        let (Some(roots), Some(live)) = (&self.roots, &self.root_live) else {
            return;
        };
        let source = &self.loaded.program.functions[self.function];
        let live_before = &live[pc];
        let is_live = |register: usize| live_before.get(register / 64).is_some_and(|word| word >> (register % 64) & 1 != 0);
        let mut reported: Vec<u32> = roots.frame.into_iter().collect();
        let mut cleared = Vec::new();
        for (register, word) in roots.word.iter().enumerate() {
            if let Some(word) = word
                && is_live(register)
            {
                reported.push(*word);
            }
        }
        for register in okkama_sparrow::written(source, pc) {
            if let Some(word) = roots.word[register.0 as usize]
                && !is_live(register.0 as usize)
            {
                reported.push(word);
                cleared.push(word);
            }
        }
        let slot = roots.slot;
        if !cleared.is_empty() {
            let zero = self.iconst(0);
            for word in cleared {
                self.builder.ins().stack_store(types::I64, zero, slot, (word * 8) as i32);
            }
        }
        if let Some(roots) = &mut self.roots {
            roots.reported = reported;
        }
    }

    fn iconst(&mut self, value: i64) -> Value {
        self.builder.ins().iconst(types::I64, value)
    }

    fn get(&mut self, register: &okkama_sparrow::Register) -> Value {
        if self.resident {
            let word = self.frame_register(register.0 as usize);
            return if self.kinds[register.0 as usize] == Kind::Float {
                self.builder.ins().bitcast(types::F64, MemFlagsData::new(), word)
            } else {
                word
            };
        }
        self.builder.use_var(self.vars[register.0 as usize])
    }

    fn set(&mut self, register: &okkama_sparrow::Register, value: Value) {
        if self.resident {
            // Stored when defined, the value is rooted by the frame from now on.
            let word = if self.kinds[register.0 as usize] == Kind::Float {
                self.builder.ins().bitcast(types::I64, MemFlagsData::new(), value)
            } else {
                value
            };
            let frame = self.builder.use_var(self.frame.expect("frame"));
            let offset = crate::tasks::REGISTERS as i32 + register.0 as i32 * 8;
            self.builder.ins().store(MEMORY, word, frame, offset);
            return;
        }
        self.builder.def_var(self.vars[register.0 as usize], value);
        // If liveness exceeds its analysis budget, keep the heap snapshot
        // current at every pointer definition instead. Saving an unrooted old
        // machine register at a later suspension could resurrect freed memory.
        if self.frame_live.is_none() && self.kinds[register.0 as usize].traced()
            && let Some(frame) = self.frame
        {
            let frame = self.builder.use_var(frame);
            let offset = crate::tasks::REGISTERS as i32 + register.0 as i32 * 8;
            self.builder.ins().store(MEMORY, value, frame, offset);
        }
        if let Some(roots) = &self.roots
            && let Some(word) = roots.word[register.0 as usize]
        {
            let slot = roots.slot;
            self.builder.ins().stack_store(types::I64, value, slot, (word * 8) as i32);
        }
    }

    /// A register as a word (floats as bits).
    fn word(&mut self, register: &okkama_sparrow::Register) -> Value {
        let value = self.get(register);
        if self.kinds[register.0 as usize] == Kind::Float {
            self.builder.ins().bitcast(types::I64, MemFlagsData::new(), value)
        } else {
            value
        }
    }

    /// Store a word into a register of its kind.
    fn set_word(&mut self, register: &okkama_sparrow::Register, value: Value) {
        let value = if self.kinds[register.0 as usize] == Kind::Float {
            self.builder.ins().bitcast(types::F64, MemFlagsData::new(), value)
        } else {
            value
        };
        self.set(register, value);
    }

    fn status_offset() -> i32 {
        std::mem::offset_of!(Ctx, status) as i32
    }

    /// After anything that can fail: on a pending failure go to the handler
    /// covering `pc`, otherwise record this activation and return.
    fn check(&mut self, pc: usize) {
        // #1108: one shared exit per function, inlined activation and handler, entered with the pc from a two-
        // instruction cold trampoline; the hot path is the same load and branch. An async body with explicit roots
        // reports different words at each call, so it keeps an exit per call.
        if self.roots.is_none() {
            let handler = self.loaded.program.functions[self.function]
                .handlers
                .iter()
                .position(|h| (h.start as usize..h.end as usize).contains(&pc));
            let ctx = self.ctx();
            let status = self.builder.ins().load(types::I64, MEMORY, ctx, Self::status_offset());
            let trampoline = self.builder.create_block();
            let next = self.builder.create_block();
            self.builder.set_cold_block(trampoline);
            self.builder.ins().brif(status, trampoline, &[], next, &[]);
            self.builder.switch_to_block(trampoline);
            let key = (self.function, self.inline.map(|inline| inline.fail), handler);
            let (exit, fresh) = match self.exits.get(&key) {
                Some(exit) => (*exit, false),
                None => {
                    let exit = self.builder.create_block();
                    self.builder.append_block_param(exit, types::I64);
                    self.builder.set_cold_block(exit);
                    self.exits.insert(key, exit);
                    (exit, true)
                }
            };
            let at = self.iconst(pc as i64);
            self.builder.ins().jump(exit, &[at.into()]);
            if fresh {
                self.builder.switch_to_block(exit);
                let at = self.builder.block_params(exit)[0];
                let status = self.builder.ins().load(types::I64, MEMORY, ctx, Self::status_offset());
                self.failure_exit(status, at, handler);
            }
            self.builder.switch_to_block(next);
            return;
        }
        let ctx = self.ctx();
        let status = self.builder.ins().load(types::I64, MEMORY, ctx, Self::status_offset());
        let failed = self.builder.create_block();
        let next = self.builder.create_block();
        self.builder.ins().brif(status, failed, &[], next, &[]);
        self.builder.switch_to_block(failed);
        let is_failure = self.builder.ins().icmp_imm_s(IntCC::Equal, status, FAILED as i64);
        let recorded = self.builder.create_block();
        let fatal = self.builder.create_block();
        // #1107: a failure is rare; its blocks leave the hot layout.
        for block in [failed, recorded, fatal] {
            self.builder.set_cold_block(block);
        }
        self.builder.ins().brif(is_failure, recorded, &[], fatal, &[]);
        self.builder.switch_to_block(fatal);
        self.exit_fail();
        self.builder.switch_to_block(recorded);
        let at = self.iconst(pc as i64);
        let handler = self.loaded.program.functions[self.function]
            .handlers
            .iter()
            .position(|h| (h.start as usize..h.end as usize).contains(&pc));
        self.record_and_handle(at, handler);
        self.builder.switch_to_block(next);
    }

    /// A pending status at `at` (#1108's shared exit): a failure is recorded and goes to `handler` or out of this
    /// activation; a fatal status or a stop leaves at once.
    fn failure_exit(&mut self, status: Value, at: Value, handler: Option<usize>) {
        let is_failure = self.builder.ins().icmp_imm_s(IntCC::Equal, status, FAILED as i64);
        let recorded = self.builder.create_block();
        let fatal = self.builder.create_block();
        self.builder.set_cold_block(recorded);
        self.builder.set_cold_block(fatal);
        self.builder.ins().brif(is_failure, recorded, &[], fatal, &[]);
        self.builder.switch_to_block(fatal);
        self.exit_fail();
        self.builder.switch_to_block(recorded);
        self.record_and_handle(at, handler);
    }

    /// Record this activation of the failure at `at`, then go to `handler` (its index in the function's handlers)
    /// with the failure, or out of this activation.
    fn record_and_handle(&mut self, at: Value, handler: Option<usize>) {
        let ctx = self.ctx();
        let function = self.iconst(self.function as i64);
        // #1122 (design #1079 §6.4): an async body is keyed by its heap frame, which stays where it is while the
        // body suspends and resumes on another stack depth; its machine frame pointer does not, and a failure that
        // unwound through a suspended settle recorded the body twice.
        let mut frame = match self.frame {
            Some(frame) if self.inline.is_none() => self.builder.use_var(frame),
            _ => self.builder.ins().get_frame_pointer(types::I64),
        };
        if let Some(inline) = &self.inline {
            // An inlined activation shares its caller's frame: frame pointers are 16-aligned, so `fp - depth` keys
            // it apart for the trace's once-per-activation rule (#926).
            let depth = inline.depth;
            frame = self.builder.ins().iadd_imm_s(frame, -i64::from(depth));
        }
        self.helper(|h| h.trace, &[ctx, function, at, frame]);
        let handler = handler.map(|index| self.loaded.program.functions[self.function].handlers[index].clone());
        match handler {
            Some(handler) => {
                let failure = self.builder.ins().load(
                    types::I64,
                    MEMORY,
                    ctx,
                    std::mem::offset_of!(Ctx, failure) as i32,
                );
                let ok = self.iconst(OK as i64);
                self.builder.ins().store(MEMORY, ok, ctx, Self::status_offset());
                // Edge moves read the old registers; the failure is written last.
                self.jump_then(&handler.target, handler.failure, failure);
            }
            None => {
                self.exit_fail();
            }
        }
    }

    /// One budget step (`budget`): below zero fuel calls `rt_poll`, and a
    /// stopped run returns from this activation like a fatal status.
    fn poll(&mut self) {
        let ctx = self.ctx();
        let cell = self.iconst(self.fuel);
        let fuel = self.builder.ins().load(types::I64, MEMORY, cell, 0);
        let left = self.builder.ins().iadd_imm_s(fuel, -1);
        self.builder.ins().store(MEMORY, left, cell, 0);
        let empty = self.builder.ins().icmp_imm_s(IntCC::SignedLessThan, left, 0);
        let slow = self.builder.create_block();
        let next = self.builder.create_block();
        self.builder.set_cold_block(slow);
        self.builder.ins().brif(empty, slow, &[], next, &[]);
        self.builder.switch_to_block(slow);
        self.helper(|h| h.poll, &[ctx]);
        self.return_unless_ok(next);
    }

    /// Continue at `next` when the status is OK, otherwise return 0.
    fn return_unless_ok(&mut self, next: Block) {
        let ctx = self.ctx();
        let status = self.builder.ins().load(types::I64, MEMORY, ctx, Self::status_offset());
        let stop = self.builder.create_block();
        self.builder.set_cold_block(stop);
        self.builder.ins().brif(status, stop, &[], next, &[]);
        self.builder.switch_to_block(stop);
        self.exit_fail();
        self.builder.switch_to_block(next);
    }

    /// A refused allocation (0 under a memory budget) returns.
    fn refused(&mut self, cell: Value) {
        if !self.refusable {
            return;
        }
        let next = self.builder.create_block();
        let stop = self.builder.create_block();
        self.builder.set_cold_block(stop);
        self.builder.ins().brif(cell, next, &[], stop, &[]);
        self.builder.switch_to_block(stop);
        self.exit_fail();
        self.builder.switch_to_block(next);
    }

    /// Leave this activation with `value`: return it, or, inlined, continue the caller with it (#926).
    fn exit_ok(&mut self, value: Value) {
        match self.inline {
            Some(inline) => {
                self.builder.ins().jump(inline.ok, &[value.into()]);
            }
            None => {
                self.builder.ins().return_(&[value]);
            }
        }
    }

    /// Leave this activation on a failure or a stop (the status says which): return 0, or, inlined, go to the
    /// caller's check after the call (#926).
    fn exit_fail(&mut self) {
        match self.inline {
            Some(inline) => {
                self.builder.ins().jump(inline.fail, &[]);
            }
            None => {
                let zero = self.iconst(0);
                self.builder.ins().return_(&[zero]);
            }
        }
    }

    /// Can the static call of `callee` with `args` arguments be inlined here (#927)? A small synchronous Okkama
    /// function without handlers, into a synchronous caller that keeps Cranelift's stack maps, one level deep.
    fn inlinable(&self, callee: usize, args: usize) -> bool {
        if self.no_inline || self.inline.is_some() || self.frame.is_some() || self.roots.is_some() {
            return false;
        }
        let source = &self.loaded.program.functions[callee];
        !source.is_async
            && source.native.is_none()
            && source.intrinsic.is_none()
            && source.handlers.is_empty()
            && source.params.len() == args
            && source.code.len() <= INLINE_CALLEE
            && self.inlined + source.code.len() <= INLINE_TOTAL
            && !source.code.iter().any(|instruction| matches!(instruction, I::Catch { .. }) || suspends(instruction))
            && !has_loop(source)
    }

    /// The body of `callee` in place of its call at `pc` (#926, #927). Its entry is a step; its registers are
    /// variables of this function (with stack maps where traced); a failure inside it records the callee's frame
    /// and then the caller's at `pc`, as the call would.
    fn inline_call(&mut self, pc: usize, callee: usize, args: &[okkama_sparrow::Register]) -> Result<Value, Error> {
        okkama_devtrace::count!("jit", "calls inlined");
        let values: Vec<Value> = args.iter().map(|arg| self.word(arg)).collect();
        let ok = self.builder.create_block();
        self.builder.append_block_param(ok, types::I64);
        let fail = self.builder.create_block();
        self.builder.set_cold_block(fail);
        let loaded = self.loaded;
        let source = &loaded.program.functions[callee];
        let info = &loaded.functions[callee];
        let saved_function = std::mem::replace(&mut self.function, callee);
        let saved_kinds = std::mem::replace(&mut self.kinds, &info.kinds);
        let saved_vars = std::mem::take(&mut self.vars);
        let saved_blocks = std::mem::take(&mut self.blocks);
        let saved_live = self.live.take();
        self.inline = Some(Inline { ok, fail, depth: 1 });
        self.inlined += source.code.len();
        for (register, kind) in info.kinds.iter().enumerate() {
            let var = self.builder.declare_var(if *kind == Kind::Float { types::F64 } else { types::I64 });
            if kind.traced() && !info.pointer_free[register] {
                self.builder.declare_var_needs_stack_map(var);
            }
            let initial = if *kind == Kind::Float {
                self.builder.ins().f64const(f64::from_bits(word::UNBOUND_RAW))
            } else {
                self.iconst(0)
            };
            self.builder.def_var(var, initial);
            self.vars.push(var);
        }
        for (register, value) in source.params.iter().zip(values) {
            self.set_word(register, value);
        }
        // A function entry is a step.
        if self.polled {
            self.poll();
        }
        self.blocks = leaders(source).into_iter().map(|leader| leader.then(|| self.builder.create_block())).collect();
        let first = self.block(0);
        self.builder.ins().jump(first, &[]);
        let emitted = self.emit_code();
        self.function = saved_function;
        self.kinds = saved_kinds;
        self.vars = saved_vars;
        self.blocks = saved_blocks;
        self.live = saved_live;
        self.inline = None;
        self.pc = pc;
        emitted?;
        // A failure or a stop in the callee: the caller's check, as after the call; it always leaves.
        self.builder.switch_to_block(fail);
        self.check(pc);
        let zero = self.iconst(0);
        self.builder.ins().jump(ok, &[zero.into()]);
        self.builder.switch_to_block(ok);
        Ok(self.builder.block_params(ok)[0])
    }

    /// The block of a leader instruction.
    fn block(&self, pc: usize) -> Block {
        self.blocks[pc].expect("a jump, handler or resume target starts a block")
    }

    fn jump(&mut self, target: &Target) {
        if self.polled && target.pc as usize <= self.pc {
            self.poll();
        }
        let values: Vec<_> = target.moves.iter().map(|(_, source)| self.get(source)).collect();
        for ((dst, _), value) in target.moves.iter().zip(values) {
            self.set(dst, value);
        }
        let block = self.block(target.pc as usize);
        self.builder.ins().jump(block, &[]);
    }

    /// Inline allocation of `words` words from the free list of their class.
    fn allocate(&mut self, words: usize) -> Value {
        if self.count_helpers {
            self.count_allocation(words);
        }
        let ctx = self.ctx();
        let Some(class) = Heap::class_of(words) else {
            let size = self.iconst(words as i64);
            let cell = self.helper(|h| h.allocate, &[ctx, size]).expect("result");
            self.refused(cell);
            return cell;
        };
        let head = self.heap.free_head(class) as i64;
        let head = self.iconst(head);
        let cell = self.builder.ins().load(types::I64, MEMORY, head, 0);
        let fast = self.builder.create_block();
        let slow = self.builder.create_block();
        self.builder.set_cold_block(slow);
        let done = self.builder.create_block();
        self.builder.append_block_param(done, types::I64);
        self.builder.ins().brif(cell, fast, &[], slow, &[]);
        self.builder.switch_to_block(fast);
        let next = self.builder.ins().load(types::I64, MEMORY, cell, 8);
        self.builder.ins().store(MEMORY, next, head, 0);
        self.builder.ins().jump(done, &[cell.into()]);
        self.builder.switch_to_block(slow);
        let size = self.iconst(words as i64);
        let fresh = self.helper(|h| h.allocate, &[ctx, size]).expect("result");
        self.refused(fresh);
        self.builder.ins().jump(done, &[fresh.into()]);
        self.builder.switch_to_block(done);
        self.builder.block_params(done)[0]
    }

    /// Count an allocation of `words` words at this instruction (`--stats`):
    /// the site is the function, the instruction and, for objects, the type.
    fn count_allocation(&mut self, words: usize) {
        let program = &self.loaded.program;
        let source = &program.functions[self.function];
        let instruction = &source.code[self.pc];
        let what = match instruction {
            I::ObjNew { object, .. } => format!("{} {}", instruction.mnemonic(), program.objects[object.0 as usize].name),
            _ => instruction.mnemonic().to_string(),
        };
        let label = format!("{}:{} {what}", source.symbol, self.pc);
        let counts = Box::new([0u64; 2]);
        let address = counts.as_ptr() as i64;
        self.jit.alloc_sites.push((label, counts));
        let address = self.iconst(address);
        for (offset, step) in [(0, 1), (8, words as i64 * 8)] {
            let value = self.builder.ins().load(types::I64, MEMORY, address, offset);
            let value = self.builder.ins().iadd_imm_s(value, step);
            self.builder.ins().store(MEMORY, value, address, offset);
        }
    }

    /// #1076: `value`, just read from field `field` of an object of `descriptor`, when a stub of `undef` may
    /// reach that field (`Loaded::may_hold_stub`): the stub of a `bool` field reads as `true`, the contract of
    /// the compiler's construction check, which reads its unbound `$pending` as "still pending" (#271); any
    /// other stub fails the program. A field no stub reaches stays a bare load. Returns the value read.
    fn bound_field(&mut self, pc: usize, descriptor: usize, field: u32, value: Value) -> Value {
        if !self.loaded.may_hold_stub(descriptor, field as usize) {
            return value;
        }
        okkama_devtrace::count!("jit", "stub-checked field reads");
        let program = &self.loaded.program;
        let declared = &program.objects[descriptor];
        let (name, ty) = &declared.fields[field as usize];
        let stub = crate::word::unbound(Kind::of(program, *ty).repr());
        let unbound = self.builder.ins().icmp_imm_s(IntCC::Equal, value, stub as i64);
        if matches!(program.ty(*ty), Some(Type::Bool)) {
            let yes = self.iconst(1);
            return self.builder.ins().select(unbound, yes, value);
        }
        let message = crate::texts::unbound_field(name, &declared.name);
        let bound = self.builder.create_block();
        let stubbed = self.builder.create_block();
        self.builder.set_cold_block(stubbed);
        self.builder.ins().brif(unbound, stubbed, &[], bound, &[]);
        self.builder.switch_to_block(stubbed);
        let ctx = self.ctx();
        let message = self.constant_text(&message);
        self.helper(|h| h.trap, &[ctx, message]);
        self.check(pc);
        self.builder.ins().jump(bound, &[]);
        self.builder.switch_to_block(bound);
        value
    }

    /// Is the field an accessor `callee` returns, of its parameter `param`, one a stub may reach (#1076)?
    fn stub_field(&self, callee: &okkama_sparrow::Function, param: usize, field: u32) -> bool {
        let ty = callee.registers[callee.params[param].0 as usize];
        matches!(self.loaded.program.ty(ty), Some(Type::Object(declared)) if self.loaded.may_hold_stub(declared.0 as usize, field as usize))
    }

    fn constant_text(&mut self, text: &str) -> Value {
        let word = self.heap.allocate_text(text);
        self.constants.push((word, Repr::Ref));
        self.iconst(word as i64)
    }

    /// Use a published ordinary entry only for a proven static target.
    /// Addresses are immutable until this runtime's JIT is dropped; publication
    /// follows finalization and stack-map installation in `compile`.
    fn ready_static(&self, function: usize, params: usize) -> Option<u64> {
        if self.late_bound || function == self.function {
            return None;
        }
        let source = self.loaded.program.functions.get(function)?;
        if source.is_async || source.native.is_some() || source.intrinsic.is_some() || source.params.len() != params {
            return None;
        }
        self.invocations.get(function).map(|entry| entry.code).filter(|&address| address != 0)
    }

    /// Resolve the code address; both paths use the same call, roots and check.
    fn call(&mut self, pc: usize, target: CallTarget, args: &[Value]) -> Value {
        let ctx = self.ctx();
        let address = match target {
            CallTarget::Static(function) => self.ready_static(function, args.len()),
            CallTarget::Dynamic(_) => None,
        };
        let kind = match target {
            CallTarget::Static(_) => usize::from(address.is_none()),
            CallTarget::Dynamic(_) => 2,
        };
        self.call_sites[kind] += 1;
        let code = if let Some(address) = address {
            self.iconst(address as i64)
        } else {
            let function = match target {
                CallTarget::Static(function) => self.iconst(function as i64),
                CallTarget::Dynamic(function) => function,
            };
            let base = self.iconst(self.invocations.as_ptr() as i64);
            let offset = self.builder.ins().imul_imm_u(function, std::mem::size_of::<runtime::Invocation>() as i64);
            let entry = self.builder.ins().iadd(base, offset);
            let code = self.builder.ins().load(types::I64, MEMORY, entry, std::mem::offset_of!(runtime::Invocation, code) as i32);
            let ready = self.builder.create_block();
            let compile = self.builder.create_block();
            self.builder.set_cold_block(compile);
            self.builder.append_block_param(ready, types::I64);
            self.builder.ins().brif(code, ready, &[code.into()], compile, &[]);
            self.builder.switch_to_block(compile);
            let compiled = self.helper(|h| h.compile, &[ctx, function]).expect("result");
            self.check(pc);
            self.builder.ins().jump(ready, &[compiled.into()]);
            self.builder.switch_to_block(ready);
            self.builder.block_params(ready)[0]
        };
        let signature = self.jit.signature(args.len());
        let signature = self.builder.import_signature(signature);
        let mut values = vec![ctx];
        values.extend_from_slice(args);
        let call = self.builder.ins().call_indirect(signature, code, &values);
        self.report_roots(call);
        let result = self.builder.inst_results(call)[0];
        self.check(pc);
        result
    }

    /// A closure call through the runtime's helper: it reads the captures, compiles the function if needed and
    /// makes an async one's cold Future.
    fn closure_call_slow(&mut self, pc: usize, callee: &okkama_sparrow::Register, args: &[okkama_sparrow::Register]) -> Value {
        let ctx = self.ctx();
        let closure = self.get(callee);
        let address = self.spill(args);
        let count = self.iconst(args.len() as i64);
        let result = self.helper(|h| h.call_closure, &[ctx, closure, address, count]).expect("result");
        self.check(pc);
        result
    }

    /// A closure call (#928): a synchronous function whose captures fill the first parameters is called
    /// directly, the captures read from the closure's fields, for up to `CLOSURE_DIRECT_CAPTURES` captures; the
    /// rest (async functions, more captures) goes through the helper. The function comes from the closure's
    /// header, and an uncompiled one is compiled on its first call, as any dynamic call.
    fn closure_call(&mut self, pc: usize, callee: &okkama_sparrow::Register, args: &[okkama_sparrow::Register]) -> Value {
        const CLOSURE_DIRECT_CAPTURES: usize = 2;
        let closure = self.get(callee);
        let header = self.builder.ins().load(types::I64, MEMORY, closure, 0);
        let function = self.builder.ins().ushr_imm_u(header, 32);
        let base = self.iconst(self.invocations.as_ptr() as i64);
        let offset = self.builder.ins().imul_imm_u(function, std::mem::size_of::<runtime::Invocation>() as i64);
        let entry = self.builder.ins().iadd(base, offset);
        // uload32/uload8 zero-extend into an i64 themselves.
        let params = self.builder.ins().uload32(MEMORY, entry, std::mem::offset_of!(runtime::Invocation, params) as i32);
        let asynchronous = self.builder.ins().uload8(types::I64, MEMORY, entry, std::mem::offset_of!(runtime::Invocation, is_async) as i32);
        let words: Vec<Value> = args.iter().map(|arg| self.word(arg)).collect();
        let merge = self.builder.create_block();
        self.builder.append_block_param(merge, types::I64);
        for captured in 0..=CLOSURE_DIRECT_CAPTURES {
            let arity = self.builder.ins().icmp_imm_u(IntCC::Equal, params, (args.len() + captured) as i64);
            let sync = self.builder.ins().icmp_imm_u(IntCC::Equal, asynchronous, 0);
            let direct = self.builder.ins().band(arity, sync);
            let call = self.builder.create_block();
            let next = self.builder.create_block();
            self.builder.ins().brif(direct, call, &[], next, &[]);
            self.builder.switch_to_block(call);
            let mut values = Vec::with_capacity(captured + words.len());
            for index in 0..captured {
                values.push(self.builder.ins().load(types::I64, MEMORY, closure, (8 + index * 8) as i32));
            }
            values.extend_from_slice(&words);
            let result = self.call(pc, CallTarget::Dynamic(function), &values);
            self.builder.ins().jump(merge, &[result.into()]);
            self.builder.switch_to_block(next);
        }
        let result = self.closure_call_slow(pc, callee, args);
        self.builder.ins().jump(merge, &[result.into()]);
        self.builder.switch_to_block(merge);
        self.builder.block_params(merge)[0]
    }

    /// Words of `registers` in the function's spill area; returns its address.
    /// One area serves every helper call (its contents are read before the
    /// call returns), sized for the widest: a slot per call site made the
    /// machine frame grow with the number of calls (#25).
    fn spill(&mut self, registers: &[okkama_sparrow::Register]) -> Value {
        let size = u32::try_from(registers.len().max(1).saturating_mul(8)).unwrap_or(u32::MAX);
        let slot = match self.spill_area {
            Some(slot) => {
                let data = &mut self.builder.func.sized_stack_slots[slot];
                data.size = data.size.max(size);
                slot
            }
            None => {
                let slot = self.builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, size, 3));
                self.spill_area = Some(slot);
                slot
            }
        };
        for (i, register) in registers.iter().enumerate() {
            let value = self.word(register);
            self.builder.ins().stack_store(types::I64, value, slot, (i * 8) as i32);
        }
        self.builder.ins().stack_addr(types::I64, slot, 0)
    }

    fn register_type(&self, register: &okkama_sparrow::Register) -> Option<&Type> {
        let source = &self.loaded.program.functions[self.function];
        self.loaded.program.ty(source.registers[register.0 as usize])
    }

    fn body(&mut self) -> Result<(), Error> {
        let program = &self.loaded.program;
        let source = &program.functions[self.function];
        let entry = self.builder.create_block();
        self.builder.append_block_params_for_function_params(entry);
        self.builder.switch_to_block(entry);
        self.ctx = Some(self.builder.block_params(entry)[0]);
        let explicit = self.root_live.is_some();
        let mut words = Vec::with_capacity(self.kinds.len());
        let mut next = 0u32;
        for (register, kind) in self.kinds.iter().enumerate() {
            let var = self.builder.declare_var(if *kind == Kind::Float { types::F64 } else { types::I64 });
            let traced = kind.traced() && !self.loaded.functions[self.function].pointer_free[register];
            if traced && explicit {
                words.push(Some(next));
                next += 1;
            } else {
                if traced {
                    self.builder.declare_var_needs_stack_map(var);
                }
                words.push(None);
            }
            self.vars.push(var);
        }
        if explicit {
            let frame = source.is_async.then_some(next);
            let count = next + u32::from(frame.is_some());
            let slot = self.builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                count.max(1) * 8,
                3,
            ));
            if source.is_async {
                // A resumption enters an instruction in the middle: no word
                // may keep what an earlier activation left.
                let zero = self.iconst(0);
                for word in 0..count {
                    self.builder.ins().stack_store(types::I64, zero, slot, (word * 8) as i32);
                }
            }
            self.roots = Some(Roots { slot, word: words, frame, reported: frame.into_iter().collect() });
        }
        // Registers start unbound; the verifier proves defined use.
        let resident = self.resident;
        for (register, kind) in self.kinds.iter().enumerate().filter(|_| !resident) {
            let value = if *kind == Kind::Float {
                self.builder.ins().f64const(f64::from_bits(word::UNBOUND_RAW))
            } else {
                self.iconst(0)
            };
            self.builder.def_var(self.vars[register], value);
        }
        if source.native.is_some() || source.intrinsic.is_some() {
            // A trampoline, for calls through the code table (roles, closures).
            let params = self.builder.block_params(entry)[1..].to_vec();
            let slot = self.builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                (params.len().max(1) * 8) as u32,
                3,
            ));
            for (i, value) in params.iter().enumerate() {
                self.builder.ins().stack_store(types::I64, *value, slot, (i * 8) as i32);
            }
            let address = self.builder.ins().stack_addr(types::I64, slot, 0);
            let ctx = self.ctx();
            let index = self.iconst(self.function as i64);
            let count = self.iconst(params.len() as i64);
            let result = if source.native.is_some() {
                self.helper(|h| h.native, &[ctx, index, address, count])
            } else {
                self.helper(|h| h.intrinsic, &[ctx, index, address, count])
            }
            .expect("result");
            self.builder.ins().return_(&[result]);
            return Ok(());
        }
        if source.is_async {
            let frame = self.builder.declare_var(types::I64);
            let value = self.builder.block_params(entry)[1];
            match &self.roots {
                Some(Roots { slot, frame: Some(word), .. }) => {
                    let (slot, word) = (*slot, *word);
                    self.builder.ins().stack_store(types::I64, value, slot, (word * 8) as i32);
                }
                _ => self.builder.declare_var_needs_stack_map(frame),
            }
            self.builder.def_var(frame, value);
            self.frame = Some(frame);
        } else {
            for (i, register) in source.params.iter().enumerate() {
                let value = self.builder.block_params(entry)[i + 1];
                self.set_word(register, value);
            }
            // The prologue's calls find the parameters.
            if let Some(roots) = &mut self.roots {
                roots.reported = source.params.iter().filter_map(|register| roots.word[register.0 as usize]).collect();
            }
        }
        self.blocks = leaders(source)
            .into_iter()
            .map(|leader| leader.then(|| self.builder.create_block()))
            .collect();
        // R2: stack exhaustion is a typed failure.
        let ctx = self.ctx();
        let sp = self.builder.ins().get_stack_pointer(types::I64);
        let limit = self.builder.ins().load(
            types::I64,
            MEMORY,
            ctx,
            std::mem::offset_of!(Ctx, stack_limit) as i32,
        );
        let low = self.builder.ins().icmp(IntCC::UnsignedLessThan, sp, limit);
        let overflow = self.builder.create_block();
        let start = if source.is_async { self.builder.create_block() } else { self.block(0) };
        let entered = if self.polled { self.builder.create_block() } else { start };
        self.builder.ins().brif(low, overflow, &[], entered, &[]);
        if self.polled {
            // A function entry is a step.
            self.builder.switch_to_block(entered);
            self.poll();
            self.builder.ins().jump(start, &[]);
        }
        if source.is_async {
            self.resume_dispatch(start);
        }
        self.builder.switch_to_block(overflow);
        self.helper(|h| h.stack_overflow, &[ctx]);
        self.exit_fail();
        self.emit_code()
    }

    /// Emit the instructions of the current function (the compiled one, or an inlined callee).
    fn emit_code(&mut self) -> Result<(), Error> {
        let source = &self.loaded.program.functions[self.function];
        // Open: the previous instruction falls through into this one.
        let mut open = false;
        for pc in 0..source.code.len() {
            match self.blocks[pc] {
                Some(block) => {
                    if open {
                        self.builder.ins().jump(block, &[]);
                    }
                    self.builder.switch_to_block(block);
                }
                // Unreachable: nothing jumps here and nothing falls through.
                None if !open => {
                    let block = self.builder.create_block();
                    self.builder.switch_to_block(block);
                }
                None => {}
            }
            self.pc = pc;
            self.enter_roots(pc);
            let before = self.builder.func.dfg.num_blocks();
            open = !self.instruction(pc)?;
            if let Some(live) = &self.live {
                let blocks = self.builder.func.dfg.num_blocks() - before + usize::from(self.blocks[pc].is_some());
                self.cost = self.cost.saturating_add(u64::from(live[pc]) * blocks as u64);
                if self.cost > self.cost_limit {
                    return Err(Error(format!(
                        "{}: compiling it would take more than {} (registers live across its blocks, at instruction {pc}); \
                         keep fewer values alive at once or split the function",
                        source.symbol,
                        compile_limit_text(self.cost_limit * BYTES_PER_LIVE_BLOCK)
                    )));
                }
            }
        }
        if open {
            return Err(Error(format!("{}: falls off the end", source.symbol)));
        }
        Ok(())
    }

    /// Entry of an async body: start with the parameters from the frame, or
    /// re-enter a suspended `await` with every register reloaded.
    fn resume_dispatch(&mut self, dispatch: Block) {
        let source = &self.loaded.program.functions[self.function];
        self.builder.switch_to_block(dispatch);
        let frame = self.builder.use_var(self.frame.expect("frame"));
        let resume = self.builder.ins().load(types::I64, MEMORY, frame, crate::tasks::RESUME as i32);
        let start = self.builder.create_block();
        let mut switch = cranelift_frontend::Switch::new();
        let mut reloads = Vec::new();
        for (pc, instruction) in source.code.iter().enumerate() {
            if suspends(instruction) {
                let block = self.builder.create_block();
                switch.set_entry(pc as u128 + 1, block);
                reloads.push((pc, block));
            }
        }
        switch.emit(self.builder, resume, start);
        self.builder.switch_to_block(start);
        let resident = self.resident;
        for register in source.params.iter().filter(|_| !resident) {
            let value = self.frame_register(register.0 as usize);
            self.set_word(register, value);
        }
        let first = self.block(0);
        self.builder.ins().jump(first, &[]);
        for (pc, block) in reloads {
            self.builder.switch_to_block(block);
            for register in (0..self.kinds.len()).filter(|_| !resident) {
                let value = self.frame_register(register);
                self.set_word(&okkama_sparrow::Register(register as u32), value);
            }
            let resumed = self.block(pc);
            self.builder.ins().jump(resumed, &[]);
        }
    }

    fn frame_register(&mut self, register: usize) -> Value {
        let frame = self.builder.use_var(self.frame.expect("frame"));
        let offset = crate::tasks::REGISTERS as i32 + register as i32 * 8;
        self.builder.ins().load(types::I64, MEMORY, frame, offset)
    }

    /// Emit one instruction; true when it ends its block.
    /// After an await-like helper: continue when ready (its value, if any, in
    /// `scratch`), or save every register and return pending; the resumed
    /// frame re-enters the instruction at `pc`.
    fn suspend_unless_ready(&mut self, pc: usize, outcome: Value, dst: Option<okkama_sparrow::Dst>) {
        let ctx = self.ctx();
        let suspend = self.builder.create_block();
        let ready = self.builder.create_block();
        let pending = self.builder.ins().icmp_imm_u(IntCC::Equal, outcome, crate::tasks::AWAIT_PENDING as i64);
        self.builder.ins().brif(pending, suspend, &[], ready, &[]);
        // Save the live continuation and re-enter this await when resumed.
        // Dead machine registers may already point to reclaimed cells. They
        // must never be published back into a precisely traced heap frame.
        self.builder.switch_to_block(suspend);
        let frame = self.builder.use_var(self.frame.expect("frame"));
        let resident = self.resident;
        for register in (0..self.kinds.len()).filter(|_| !resident) {
            let live = self.frame_live.as_ref().is_none_or(|sets|
                sets[pc].get(register / 64).is_some_and(|word| word >> (register % 64) & 1 != 0));
            let value = if live { self.word(&okkama_sparrow::Register(register as u32)) } else { self.iconst(0) };
            let offset = crate::tasks::REGISTERS as i32 + register as i32 * 8;
            self.builder.ins().store(MEMORY, value, frame, offset);
        }
        let resume = self.iconst(pc as i64 + 1);
        self.builder.ins().store(MEMORY, resume, frame, crate::tasks::RESUME as i32);
        let one = self.iconst(1);
        self.builder.ins().return_(&[one]);
        self.builder.switch_to_block(ready);
        if let Some(dst) = dst {
            let value = self.builder.ins().load(
                types::I64,
                MEMORY,
                ctx,
                std::mem::offset_of!(Ctx, scratch) as i32,
            );
            self.set_word(&dst.0, value);
        }
    }

    fn instruction(&mut self, pc: usize) -> Result<bool, Error> {
        let program = &self.loaded.program;
        let source = &program.functions[self.function];
        let instruction = &source.code[pc];
        let ctx = self.ctx();
        let unsupported = || {
            Error(format!(
                "{}:{pc}: {} is not supported by sparrow-core yet",
                source.symbol,
                instruction.mnemonic()
            ))
        };
        if self.plain && generic(instruction) {
            self.generic_instruction(pc, instruction);
            return Ok(false);
        }
        match instruction {
            I::Const { dst, value } => {
                let value = match value {
                    Constant::Int(text) => {
                        let n: num_bigint::BigInt =
                            text.parse().map_err(|_| Error("invalid integer constant".into()))?;
                        let word = self.heap.big(n);
                        if word & 1 == 0 {
                            self.constants.push((word, Repr::Int));
                        }
                        self.iconst(word as i64)
                    }
                    Constant::Bool(b) => self.iconst(i64::from(*b)),
                    Constant::Byte(b) => self.iconst(i64::from(*b)),
                    Constant::Char(c) => self.iconst(i64::from(u32::from(*c))),
                    Constant::Float(bits) => self.builder.ins().f64const(f64::from_bits(*bits)),
                    Constant::Str(text) => self.constant_text(text),
                };
                self.set(&dst.0, value);
            }
            I::Move { dst, source } => {
                let value = self.get(source);
                self.set(&dst.0, value);
            }
            I::IntAdd { dst, left, right }
            | I::IntSub { dst, left, right }
            | I::IntMul { dst, left, right }
            | I::IntMod { dst, left, right } => {
                let (l, r) = (self.get(left), self.get(right));
                let both = self.builder.ins().band(l, r);
                let small = self.builder.ins().band_imm_u(both, 1);
                let fast = self.builder.create_block();
                let slow = self.builder.create_block();
                self.builder.set_cold_block(slow);
                let done = self.builder.create_block();
                self.builder.append_block_param(done, types::I64);
                self.builder.ins().brif(small, fast, &[], slow, &[]);
                self.builder.switch_to_block(fast);
                let op = match instruction {
                    I::IntAdd { .. } => runtime::INT_ADD,
                    I::IntSub { .. } => runtime::INT_SUB,
                    I::IntMul { .. } => runtime::INT_MUL,
                    _ => runtime::INT_MOD,
                };
                if op == runtime::INT_MOD {
                    // Divisors 0 and -1 go to the exact path.
                    let divisor = self.builder.ins().sshr_imm_u(r, 1);
                    let shifted = self.builder.ins().iadd_imm_s(divisor, 1);
                    let special = self.builder.ins().icmp_imm_u(IntCC::UnsignedLessThanOrEqual, shifted, 1);
                    let commit = self.builder.create_block();
                    self.builder.ins().brif(special, slow, &[], commit, &[]);
                    self.builder.switch_to_block(commit);
                    let dividend = self.builder.ins().sshr_imm_u(l, 1);
                    let remainder = self.builder.ins().srem(dividend, divisor);
                    let doubled = self.builder.ins().ishl_imm_u(remainder, 1);
                    let tagged = self.builder.ins().bor_imm_u(doubled, 1);
                    self.builder.ins().jump(done, &[tagged.into()]);
                } else {
                    let even = self.builder.ins().iadd_imm_s(r, -1);
                    let (value, overflow) = match op {
                        runtime::INT_ADD => self.builder.ins().sadd_overflow(l, even),
                        runtime::INT_SUB => self.builder.ins().ssub_overflow(l, even),
                        _ => {
                            let half = self.builder.ins().sshr_imm_u(l, 1);
                            let (product, overflow) = self.builder.ins().smul_overflow(half, even);
                            (self.builder.ins().bor_imm_u(product, 1), overflow)
                        }
                    };
                    let commit = self.builder.create_block();
                    self.builder.ins().brif(overflow, slow, &[], commit, &[]);
                    self.builder.switch_to_block(commit);
                    self.builder.ins().jump(done, &[value.into()]);
                }
                self.builder.switch_to_block(slow);
                let op = self.iconst(op as i64);
                let result = self.helper(|h| h.int, &[ctx, op, l, r]).expect("result");
                self.check(pc);
                self.builder.ins().jump(done, &[result.into()]);
                self.builder.switch_to_block(done);
                let value = self.builder.block_params(done)[0];
                self.set(&dst.0, value);
            }
            I::IntCmp { op, dst, left, right } => {
                let (l, r) = (self.get(left), self.get(right));
                let both = self.builder.ins().band(l, r);
                let small = self.builder.ins().band_imm_u(both, 1);
                let fast = self.builder.create_block();
                let slow = self.builder.create_block();
                self.builder.set_cold_block(slow);
                let done = self.builder.create_block();
                self.builder.append_block_param(done, types::I64);
                self.builder.ins().brif(small, fast, &[], slow, &[]);
                let condition = int_condition(*op);
                self.builder.switch_to_block(fast);
                let flag = self.builder.ins().icmp(condition, l, r);
                let flag = self.builder.ins().uextend(types::I64, flag);
                self.builder.ins().jump(done, &[flag.into()]);
                self.builder.switch_to_block(slow);
                let order = self.helper(|h| h.int_order, &[ctx, l, r]).expect("result");
                let flag = self.builder.ins().icmp_imm_s(condition, order, 0);
                let flag = self.builder.ins().uextend(types::I64, flag);
                self.builder.ins().jump(done, &[flag.into()]);
                self.builder.switch_to_block(done);
                let value = self.builder.block_params(done)[0];
                self.set(&dst.0, value);
            }
            I::BoolNot { dst, operand } => {
                let value = self.get(operand);
                let value = self.builder.ins().bxor_imm_u(value, 1);
                self.set(&dst.0, value);
            }
            I::BoolCmp { op, dst, left, right }
            | I::ByteCmp { op, dst, left, right }
            | I::CharCmp { op, dst, left, right } => {
                let (l, r) = (self.get(left), self.get(right));
                let flag = self.builder.ins().icmp(int_condition(*op), l, r);
                let flag = self.builder.ins().uextend(types::I64, flag);
                self.set(&dst.0, flag);
            }
            I::FloatAdd { dst, left, right }
            | I::FloatSub { dst, left, right }
            | I::FloatMul { dst, left, right }
            | I::FloatDiv { dst, left, right } => {
                let (l, r) = (self.get(left), self.get(right));
                let (value, op) = match instruction {
                    I::FloatAdd { .. } => (self.builder.ins().fadd(l, r), crate::scalar::FLOAT_ADD),
                    I::FloatSub { .. } => (self.builder.ins().fsub(l, r), crate::scalar::FLOAT_SUB),
                    I::FloatMul { .. } => (self.builder.ins().fmul(l, r), crate::scalar::FLOAT_MUL),
                    _ => (self.builder.ins().fdiv(l, r), crate::scalar::FLOAT_DIV),
                };
                // #1105: `x - x` is 0 for a finite x and NaN for an infinity or NaN, so one subtraction and an
                // ordered self-comparison check finiteness (no `fabs`, no constant load). `Ordered` is one
                // condition on x86-64, where `Equal` is two (Z and NP).
                let difference = self.builder.ins().fsub(value, value);
                let finite = self.builder.ins().fcmp(FloatCC::Ordered, difference, difference);
                let slow = self.builder.create_block();
                let done = self.builder.create_block();
                self.builder.set_cold_block(slow);
                self.builder.ins().brif(finite, done, &[], slow, &[]);
                // The operands are finite floats, so a non-finite result of + - * / always fails
                // (`scalar::float_binary`): the slow path never comes back, and `done` has the fast path as its only
                // predecessor, so the result stays in a float register.
                self.builder.switch_to_block(slow);
                let op = self.iconst(op as i64);
                let lw = self.builder.ins().bitcast(types::I64, MemFlagsData::new(), l);
                let rw = self.builder.ins().bitcast(types::I64, MemFlagsData::new(), r);
                self.helper(|h| h.float, &[ctx, op, lw, rw]).expect("result");
                self.check(pc);
                self.builder.ins().trap(TrapCode::unwrap_user(FLOAT_SLOW_PATH_RETURNED));
                self.builder.switch_to_block(done);
                self.set(&dst.0, value);
            }
            I::FloatNeg { dst, operand } | I::FloatAbs { dst, operand } => {
                let value = self.get(operand);
                let value = if matches!(instruction, I::FloatNeg { .. }) {
                    self.builder.ins().fneg(value)
                } else {
                    self.builder.ins().fabs(value)
                };
                self.set(&dst.0, value);
            }
            I::FloatCmp { op, dst, left, right } => {
                let (l, r) = (self.get(left), self.get(right));
                let condition = match op {
                    Cmp::Eq => FloatCC::Equal,
                    Cmp::Ne => FloatCC::NotEqual,
                    Cmp::Lt => FloatCC::LessThan,
                    Cmp::Le => FloatCC::LessThanOrEqual,
                    Cmp::Gt => FloatCC::GreaterThan,
                    Cmp::Ge => FloatCC::GreaterThanOrEqual,
                };
                let flag = self.builder.ins().fcmp(condition, l, r);
                let flag = self.builder.ins().uextend(types::I64, flag);
                self.set(&dst.0, flag);
            }
            I::StrConcat { dst, left, right } => {
                let (l, r) = (self.get(left), self.get(right));
                let text = self.helper(|h| h.concat, &[ctx, l, r]).expect("result");
                self.check(pc);
                self.set(&dst.0, text);
            }
            I::ArrayNew { dst, elements } => {
                let repr = match self.register_type(&dst.0) {
                    Some(Type::Array(element)) if Kind::of(program, *element) == Kind::Byte => runtime::ARRAY_BYTES,
                    Some(Type::Array(element)) => Kind::of(program, *element).repr() as u64,
                    _ => return Err(unsupported()),
                };
                if elements.is_empty() {
                    // #1168: one shared empty array per representation, a constant of the code.
                    let shared = self.heap.empty_array(repr);
                    let shared = self.iconst(shared as i64);
                    self.set(&dst.0, shared);
                    return Ok(false);
                }
                if elements.len() <= INLINE_ARRAY_LITERAL {
                    let array = self.array_literal(repr, elements);
                    self.set(&dst.0, array);
                    return Ok(false);
                }
                let address = self.spill(elements);
                let repr = self.iconst(repr as i64);
                let count = self.iconst(elements.len() as i64);
                let array = self.helper(|h| h.array, &[ctx, repr, address, count]).expect("result");
                self.set(&dst.0, array);
            }
            I::MapNew { dst, entries } if entries.is_empty() && !self.plain => {
                // #1168: one shared empty map per key and value representation, a constant of the code.
                let (key, value) = match self.register_type(&dst.0) {
                    Some(Type::Map(key, value)) => (Kind::of(program, *key).repr(), Kind::of(program, *value).repr()),
                    _ => return Err(unsupported()),
                };
                let shared = self.heap.empty_map(key, value);
                let shared = self.iconst(shared as i64);
                self.set(&dst.0, shared);
            }
            I::ArrayLen { dst, array } => {
                let array = self.get(array);
                let header = self.builder.ins().load(types::I64, MEMORY, array, 0);
                let length = self.builder.ins().ushr_imm_u(header, 32);
                let doubled = self.builder.ins().ishl_imm_u(length, 1);
                let tagged = self.builder.ins().bor_imm_u(doubled, 1);
                self.set(&dst.0, tagged);
            }
            I::ArrayGet { dst, array, index } => {
                let array_register = *array;
                let (array, index) = (self.get(array), self.get(index));
                let header = self.builder.ins().load(types::I64, MEMORY, array, 0);
                let length = self.builder.ins().ushr_imm_u(header, 32);
                let small = self.builder.ins().band_imm_u(index, 1);
                let position = self.builder.ins().sshr_imm_u(index, 1);
                // Unsigned compare also rejects negative positions.
                let inside = self.builder.ins().icmp(IntCC::UnsignedLessThan, position, length);
                let inside = self.builder.ins().uextend(types::I64, inside);
                let valid = self.builder.ins().band(small, inside);
                let load = self.builder.create_block();
                let outside = self.builder.create_block();
                self.builder.set_cold_block(outside);
                self.builder.ins().brif(valid, load, &[], outside, &[]);
                self.builder.switch_to_block(outside);
                let message = self.constant_text(crate::texts::ARRAY_INDEX);
                self.helper(|h| h.trap, &[ctx, message]);
                self.check(pc);
                self.builder.ins().jump(load, &[]);
                self.builder.switch_to_block(load);
                let value = self.element(array, header, position, self.bytes_array(array_register));
                self.set_word(&dst.0, value);
            }
            // #1168: a fieldless value of a shareable type is its one shared instance, a constant of the code.
            I::ObjNew { dst, object, fields } if fields.is_empty() && self.heap.shared_object(object.0).is_some() => {
                let shared = self.heap.shared_object(object.0).expect("shareable");
                let shared = self.iconst(shared as i64);
                self.set(&dst.0, shared);
            }
            I::ObjNew { dst, object, fields } => {
                let pointer = {
                    let words = self.object_words(object.0 as usize, fields.len());
                    let pointer = self.allocate(words);
                    let header = self.iconst(heap::header(heap::OBJECT, Repr::Raw, object.0) as i64);
                    self.builder.ins().store(MEMORY, header, pointer, 0);
                    pointer
                };
                for (i, field) in fields.iter().enumerate() {
                    let value = self.word(field);
                    self.builder.ins().store(MEMORY, value, pointer, ((i + 1) * 8) as i32);
                }
                self.set(&dst.0, pointer);
                self.created(object.0 as usize, pointer);
            }
            I::ObjGet { dst, object, field } => {
                let pointer = self.get(object);
                let mut value = self.builder.ins().load(types::I64, MEMORY, pointer, ((*field + 1) * 8) as i32);
                if let Some(Type::Object(declared)) = self.register_type(object) {
                    let declared = declared.0 as usize;
                    value = self.bound_field(pc, declared, *field, value);
                }
                self.set_word(&dst.0, value);
            }
            I::ObjInit { object, field, value } => {
                let pointer = self.get(object);
                let value = self.word(value);
                self.builder.ins().store(MEMORY, value, pointer, ((*field + 1) * 8) as i32);
            }
            I::ObjNext { dst, object, updates } => {
                let declared = match self.register_type(object) {
                    Some(Type::Object(o) | Type::Successor(o)) => o.0 as usize,
                    _ => return Err(unsupported()),
                };
                let pointer = {
                    let count = program.objects[declared].fields.len();
                    let pointer = self.allocate(self.object_words(declared, count));
                    let source_object = self.get(object);
                    for word_index in 0..=count {
                        let value = match updates.iter().find(|(f, _)| *f as usize + 1 == word_index) {
                            Some((_, register)) => self.word(register),
                            None => self.builder.ins().load(types::I64, MEMORY, source_object, (word_index * 8) as i32),
                        };
                        self.builder.ins().store(MEMORY, value, pointer, (word_index * 8) as i32);
                    }
                    pointer
                };
                self.set(&dst.0, pointer);
                self.created(declared, pointer);
            }
            I::RoleWrap { dst, source: value, .. } => {
                let value = self.get(value);
                self.set(&dst.0, value);
            }
            // Вариант (#290): значение — объект своего случая, только с полями
            // payload. Случаи идут подряд (это проверяет загрузка), поэтому
            // номер случая — разность дескрипторов.
            I::VariantNew { dst, variant, case, fields } => {
                let object = program.variants[variant.0 as usize].cases[*case as usize].1;
                if fields.is_empty()
                    && let Some(shared) = self.heap.shared_object(object.0)
                {
                    let shared = self.iconst(shared as i64);
                    self.set(&dst.0, shared);
                    return Ok(false);
                }
                let pointer = {
                    let words = self.object_words(object.0 as usize, fields.len());
                    let pointer = self.allocate(words);
                    let header = self.iconst(heap::header(heap::OBJECT, Repr::Raw, object.0) as i64);
                    self.builder.ins().store(MEMORY, header, pointer, 0);
                    pointer
                };
                for (i, field) in fields.iter().enumerate() {
                    let value = self.word(field);
                    self.builder.ins().store(MEMORY, value, pointer, ((i + 1) * 8) as i32);
                }
                self.set(&dst.0, pointer);
                self.created(object.0 as usize, pointer);
            }
            I::VariantTag { dst, value } => {
                let first = match self.register_type(value) {
                    Some(Type::Variant(v)) => program.variants[v.0 as usize].cases[0].1.0,
                    _ => return Err(unsupported()),
                };
                let pointer = self.get(value);
                let header = self.builder.ins().load(types::I64, MEMORY, pointer, 0);
                let descriptor = self.builder.ins().ushr_imm_u(header, 32);
                let tag = self.builder.ins().iadd_imm_s(descriptor, -i64::from(first));
                let doubled = self.builder.ins().ishl_imm_u(tag, 1);
                let tagged = self.builder.ins().bor_imm_u(doubled, 1);
                self.set(&dst.0, tagged);
            }
            I::VariantGet { dst, value, case, field } => {
                let object = match self.register_type(value) {
                    Some(Type::Variant(v)) => program.variants[v.0 as usize].cases[*case as usize].1.0,
                    _ => return Err(unsupported()),
                };
                let pointer = self.get(value);
                let header = self.builder.ins().load(types::I64, MEMORY, pointer, 0);
                let descriptor = self.builder.ins().ushr_imm_u(header, 32);
                let active = self.builder.ins().icmp_imm_s(IntCC::Equal, descriptor, i64::from(object));
                let load = self.builder.create_block();
                let inactive = self.builder.create_block();
                self.builder.ins().brif(active, load, &[], inactive, &[]);
                self.builder.switch_to_block(inactive);
                let message = self.constant_text(crate::texts::INACTIVE_CASE);
                self.helper(|h| h.trap, &[ctx, message]);
                self.check(pc);
                self.builder.ins().jump(load, &[]);
                self.builder.switch_to_block(load);
                let value = self.builder.ins().load(types::I64, MEMORY, pointer, ((*field + 1) * 8) as i32);
                let value = self.bound_field(pc, object as usize, *field, value);
                self.set_word(&dst.0, value);
            }
            I::VariantOf { dst, source, .. } => {
                let value = self.get(source);
                self.set(&dst.0, value);
            }
            I::CallRole { dst, receiver, role, slot, args } => {
                let objects = program.objects.len().max(self.jit.object_capacity);
                let table = self.jit.dispatch.entry((role.0, *slot)).or_insert_with(|| dispatch_table(program, role.0, *slot, objects));
                debug_assert_eq!(table.len(), objects);
                let table = table.as_ptr() as i64;
                let object = self.get(receiver);
                let header = self.builder.ins().load(types::I64, MEMORY, object, 0);
                let descriptor = self.builder.ins().ushr_imm_u(header, 32);
                let offset = self.builder.ins().ishl_imm_u(descriptor, 3);
                let base = self.iconst(table);
                let entry = self.builder.ins().iadd(base, offset);
                let function = self.builder.ins().load(types::I64, MEMORY, entry, 0);
                // Async witnesses: the call makes a cold Future of the method.
                let asynchronous = crate::program::role_slot_is_async(program, *role, *slot);
                let result = if asynchronous {
                    let mut registers = vec![*receiver];
                    registers.extend(args.iter().copied());
                    let address = self.spill(&registers);
                    let count = self.iconst(registers.len() as i64);
                    let future = self.helper(|h| h.future_new, &[ctx, function, address, count]).expect("result");
                    self.check(pc);
                    future
                } else {
                    let mut values = vec![object];
                    for arg in args {
                        let value = self.word(arg);
                        values.push(value);
                    }
                    self.call(pc, CallTarget::Dynamic(function), &values)
                };
                if let Some(dst) = dst {
                    self.set_word(&dst.0, result);
                }
            }
            I::Call { dst, function, args } => {
                let callee = &program.functions[function.0 as usize];
                // FutureCompletions.completion reads its origins at the call.
                let eager = callee.intrinsic.as_ref().is_some_and(|i| i.name == "okkama.collect.FutureCompletions.completion");
                let result = if callee.is_async && !eager {
                    // Async functions (natives and intrinsics too) make a cold Future.
                    let address = self.spill(args);
                    let index = self.iconst(i64::from(function.0));
                    let count = self.iconst(args.len() as i64);
                    let future = self.helper(|h| h.future_new, &[ctx, index, address, count]).expect("result");
                    self.check(pc);
                    future
                } else if let Some(atomic) = atomic_access(callee) {
                    self.atomic(pc, function.0, atomic, args)
                } else if callee.native.is_some() || callee.intrinsic.is_some() {
                    let address = self.spill(args);
                    let index = self.iconst(i64::from(function.0));
                    let count = self.iconst(args.len() as i64);
                    let result = if callee.native.is_some() {
                        self.helper(|h| h.native, &[ctx, index, address, count])
                    } else {
                        self.helper(|h| h.intrinsic, &[ctx, index, address, count])
                    }
                    .expect("result");
                    self.check(pc);
                    result
                } else if let Some((param, field)) = accessor(callee).filter(|&(param, field)| !self.plain && !self.stub_field(callee, param, field)) {
                    // #541: a field accessor is its load. Its entry stays a
                    // step, and it cannot fail, allocate or reach a safepoint,
                    // so no failure, trace or root differs from the call. A
                    // field a stub may reach is checked, so it can fail: that
                    // accessor stays a call (#1076).
                    if self.polled {
                        self.poll();
                    }
                    let pointer = self.get(&args[param]);
                    self.builder.ins().load(types::I64, MEMORY, pointer, ((field + 1) * 8) as i32)
                } else if self.inlinable(function.0 as usize, args.len()) {
                    self.inline_call(pc, function.0 as usize, args)?
                } else {
                    let mut values = Vec::with_capacity(args.len());
                    for arg in args {
                        let value = self.word(arg);
                        values.push(value);
                    }
                    self.call(pc, CallTarget::Static(function.0 as usize), &values)
                };
                if let Some(dst) = dst {
                    self.set_word(&dst.0, result);
                }
            }
            I::CallCap { dst, cap, ty, field } => {
                let function = program.caps[ty.0 as usize]
                    .fields
                    .get(*field as usize)
                    .ok_or_else(unsupported)?
                    .1
                    .0;
                let result = if program.functions[function as usize].is_async {
                    let index = self.iconst(i64::from(function));
                    let address = self.spill(std::slice::from_ref(cap));
                    let count = self.iconst(1);
                    let future = self.helper(|h| h.future_new, &[ctx, index, address, count]).expect("result");
                    self.check(pc);
                    future
                } else {
                    let receiver = self.word(cap);
                    self.call(pc, CallTarget::Static(function as usize), &[receiver])
                };
                self.set_word(&dst.0, result);
            }
            I::Throw { value } => {
                let value = self.get(value);
                self.helper(|h| h.throw, &[ctx, value]);
                self.check(pc);
                self.exit_fail();
                return Ok(true);
            }
            I::Trap { message, .. } => {
                let message = self.constant_text(message);
                self.helper(|h| h.trap, &[ctx, message]);
                self.check(pc);
                self.exit_fail();
                return Ok(true);
            }
            // #1072: the producer claims control never gets here; if it does, the program fails as at
            // `trap unreachable`, so a verified function with it compiles and runs.
            I::Unreachable {} => {
                let message = self.constant_text(UNREACHABLE);
                self.helper(|h| h.trap, &[ctx, message]);
                self.check(pc);
                self.exit_fail();
                return Ok(true);
            }
            I::Jump { target } => {
                self.jump(target);
                return Ok(true);
            }
            I::Branch { condition, then_target, else_target } => {
                let condition = self.get(condition);
                let (yes, no) = (self.builder.create_block(), self.builder.create_block());
                self.builder.ins().brif(condition, yes, &[], no, &[]);
                self.builder.switch_to_block(yes);
                self.jump(then_target);
                self.builder.switch_to_block(no);
                self.jump(else_target);
                return Ok(true);
            }
            I::Return { value } => {
                let value = match value {
                    Some(register) => self.word(register),
                    None => self.iconst(0),
                };
                if let Some(frame) = self.frame {
                    let frame = self.builder.use_var(frame);
                    self.helper(|h| h.complete, &[ctx, frame, value]);
                    let done = self.iconst(0);
                    self.builder.ins().return_(&[done]);
                } else {
                    self.exit_ok(value);
                }
                return Ok(true);
            }
            I::RegionSettle { .. } | I::RegionSettleUnwind { .. } if self.frame.is_some() => {
                let this = self.builder.use_var(self.frame.expect("frame"));
                let unbound = self.iconst(word::UNBOUND as i64);
                let (region, keep, failure, dst) = match instruction {
                    I::RegionSettle { region, keep } => {
                        let keep = keep.map(|keep| self.word(&keep)).unwrap_or(unbound);
                        (self.get(region), keep, unbound, None)
                    }
                    I::RegionSettleUnwind { dst, region, failure } => (self.get(region), unbound, self.get(failure), Some(*dst)),
                    _ => unreachable!("matched above"),
                };
                let outcome = self.helper(|h| h.region_settle, &[ctx, this, region, keep, failure]).expect("result");
                self.check(pc);
                self.suspend_unless_ready(pc, outcome, dst);
            }
            I::ResourceSettle { .. } | I::ResourceSettleUnwind { .. } if self.frame.is_some() => {
                let this = self.builder.use_var(self.frame.expect("frame"));
                let unbound = self.iconst(word::UNBOUND as i64);
                let (value, failure, dst) = match instruction {
                    I::ResourceSettle { value } => (*value, unbound, None),
                    I::ResourceSettleUnwind { dst, value, failure } => (*value, self.get(failure), Some(*dst)),
                    _ => unreachable!("matched above"),
                };
                let object = self.word(&value);
                let outcome = self.helper(|h| h.resource_settle, &[ctx, this, object, failure]).expect("result");
                self.check(pc);
                self.suspend_unless_ready(pc, outcome, dst);
            }
            I::Await { dst, future } | I::AwaitTake { dst, future } if self.frame.is_some() => {
                let this = self.builder.use_var(self.frame.expect("frame"));
                let future = self.get(future);
                let helper: fn(&Helpers) -> FuncId = if matches!(instruction, I::AwaitTake { .. }) {
                    |h| h.await_take
                } else {
                    |h| h.await_
                };
                let outcome = self.helper(helper, &[ctx, this, future]).expect("result");
                self.check(pc);
                self.suspend_unless_ready(pc, outcome, *dst);
            }
            I::FutSpawn { dst, future } => {
                let future = self.get(future);
                let spawned = self.helper(|h| h.spawn, &[ctx, future]).expect("result");
                self.check(pc);
                self.set(&dst.0, spawned);
            }
            I::Fut { op, dst, future } => {
                let future = self.get(future);
                let op = self.iconst(*op as i64);
                let result = self.helper(|h| h.fut, &[ctx, op, future]).expect("result");
                self.check(pc);
                if let Some(dst) = dst {
                    self.set_word(&dst.0, result);
                }
            }
            I::ArrayLookup { array, index, value, found, missing } => {
                let array_register = *array;
                let (array, index) = (self.get(array), self.get(index));
                let header = self.builder.ins().load(types::I64, MEMORY, array, 0);
                let length = self.builder.ins().ushr_imm_u(header, 32);
                let small = self.builder.ins().band_imm_u(index, 1);
                let position = self.builder.ins().sshr_imm_u(index, 1);
                let inside = self.builder.ins().icmp(IntCC::UnsignedLessThan, position, length);
                let inside = self.builder.ins().uextend(types::I64, inside);
                let valid = self.builder.ins().band(small, inside);
                let hit = self.builder.create_block();
                let miss = self.builder.create_block();
                self.builder.ins().brif(valid, hit, &[], miss, &[]);
                self.builder.switch_to_block(hit);
                let item = self.element(array, header, position, self.bytes_array(array_register));
                // Edge moves read the old registers; the found value is written last.
                self.jump_then(found, value.0, item);
                self.builder.switch_to_block(miss);
                self.jump(missing);
                return Ok(true);
            }
            I::MapLookup { map, key, value, found, missing } => {
                let (map, key) = (self.get(map), self.word(key));
                let function = self.iconst(self.function as i64);
                let at = self.iconst(pc as i64);
                let present = self.helper(|h| h.map_find, &[ctx, function, at, map, key]).expect("result");
                let hit = self.builder.create_block();
                let miss = self.builder.create_block();
                self.builder.ins().brif(present, hit, &[], miss, &[]);
                self.builder.switch_to_block(hit);
                let item = self.builder.ins().load(
                    types::I64,
                    MEMORY,
                    ctx,
                    std::mem::offset_of!(Ctx, scratch) as i32,
                );
                self.jump_then(found, value.0, item);
                self.builder.switch_to_block(miss);
                self.jump(missing);
                return Ok(true);
            }
            I::Closure { dst, function, captures } => {
                // #1167: the layout `Heap::closure_new` writes, allocated inline: the header, the captures, then
                // unbound words up to the function's layout. rt_closure's checks are static facts of verified code,
                // so the JIT has no helper call for a closure (the interpreter keeps rt_closure).
                let words = self.heap.closure_words(function.0);
                if words < captures.len() || captures.len() > self.loaded.program.functions[function.0 as usize].params.len() {
                    return Err(Error(format!("{}:{pc}: closure capture count exceeds parameter count", source.symbol)));
                }
                let pointer = self.allocate(words + 1);
                let header = self.iconst(heap::header(heap::CLOSURE, Repr::Raw, function.0) as i64);
                self.builder.ins().store(MEMORY, header, pointer, 0);
                for (i, capture) in captures.iter().enumerate() {
                    let value = self.word(capture);
                    self.builder.ins().store(MEMORY, value, pointer, ((i + 1) * 8) as i32);
                }
                if words > captures.len() {
                    let unbound = self.iconst(0);
                    for i in captures.len()..words {
                        self.builder.ins().store(MEMORY, unbound, pointer, ((i + 1) * 8) as i32);
                    }
                }
                self.set(&dst.0, pointer);
            }
            I::CallFn { dst, callee, args } => {
                let result = if self.plain || self.count_helpers || self.closure_reference {
                    self.closure_call_slow(pc, callee, args)
                } else {
                    self.closure_call(pc, callee, args)
                };
                if let Some(dst) = dst {
                    self.set_word(&dst.0, result);
                }
            }
            I::Catch { failure, value, handler, next, .. } => {
                let failure = self.get(failure);
                let function = self.iconst(self.function as i64);
                let at = self.iconst(pc as i64);
                let matched = self.helper(|h| h.catch, &[ctx, function, at, failure]).expect("result");
                let hit = self.builder.create_block();
                let miss = self.builder.create_block();
                self.builder.ins().brif(matched, hit, &[], miss, &[]);
                self.builder.switch_to_block(hit);
                let payload = self.builder.ins().load(
                    types::I64,
                    MEMORY,
                    ctx,
                    std::mem::offset_of!(Ctx, scratch) as i32,
                );
                self.jump_then(handler, value.0, payload);
                self.builder.switch_to_block(miss);
                self.jump(next);
                return Ok(true);
            }
            I::RegionEnter { dst } if !self.plain => {
                // #530: a region is made at its first add that holds something (`rt_region_add`); most stay empty.
                let none = self.iconst(0);
                self.set(&dst.0, none);
            }
            I::RegionEnter { dst } => {
                // `[overflow, slots…]`, all unbound (see `failures::REGION_SLOTS`).
                let length = 1 + crate::failures::REGION_SLOTS;
                let cell = self.allocate(1 + length);
                let header = self.iconst(heap::header(heap::ARRAY, Repr::Ref, length as u32) as i64);
                self.builder.ins().store(MEMORY, header, cell, 0);
                let unbound = self.iconst(0);
                for index in 0..length {
                    self.builder.ins().store(MEMORY, unbound, cell, (8 + index * 8) as i32);
                }
                self.set(&dst.0, cell);
            }
            I::RegionAdopt { region: region_register, value: register } | I::RegionTrack { region: region_register, value: register } => {
                let region_register = *region_register;
                // A scalar never reaches an identity.
                if !self.kinds[register.0 as usize].traced() && !self.plain {
                    return Ok(false);
                }
                let (region, value) = (self.get(&region_register), self.word(register));
                let own = self.iconst(i64::from(matches!(instruction, I::RegionAdopt { .. })));
                self.dev_site(1);
                if self.plain {
                    let made = self.helper(|h| h.region_add, &[ctx, region, value, own]).expect("result");
                    self.set(&region_register, made);
                } else {
                    // #529: `region_add` returns at once unless the value may reach a free identity
                    // (`Runtime::may_reach_free`); most adds are objects without an identity, decided here.
                    let call = self.builder.create_block();
                    let done = self.builder.create_block();
                    self.may_reach_identity(value, call, done);
                    self.builder.switch_to_block(call);
                    let made = self.helper(|h| h.region_add, &[ctx, region, value, own]).expect("result");
                    self.set(&region_register, made);
                    self.builder.ins().jump(done, &[]);
                    self.builder.switch_to_block(done);
                }
            }
            I::ResourcePin { value: register } => {
                let offset = self.static_identity(register);
                let value = self.word(register);
                match offset {
                    // #909: pin in machine code: a free owner word becomes static (`Runtime::resource_pin`).
                    Some(offset) => {
                        let owner = self.builder.ins().load(types::I64, MEMORY, value, offset + 8);
                        let free = self.builder.ins().icmp_imm_u(IntCC::Equal, owner, 0);
                        let (pin, done) = (self.builder.create_block(), self.builder.create_block());
                        self.builder.ins().brif(free, pin, &[], done, &[]);
                        self.builder.switch_to_block(pin);
                        let pinned = self.iconst(2);
                        self.builder.ins().store(MEMORY, pinned, value, offset + 8);
                        self.builder.ins().jump(done, &[]);
                        self.builder.switch_to_block(done);
                    }
                    None => {
                        self.helper(|h| h.resource_pin, &[ctx, value]);
                    }
                }
            }
            I::ResourceLeave { value: register } => {
                let offset = self.static_identity(register);
                let destroy = match self.register_type(register) {
                    Some(Type::Object(object)) => program.objects[object.0 as usize].destroy.map(|f| f.0 as usize),
                    _ => None,
                };
                let value = self.word(register);
                match (offset, destroy) {
                    // #909: a static resource still static dies here: mark it dead, then call its destroy directly
                    // (`Runtime::resource_close`); a destroy failure fails this instruction like any call.
                    (Some(offset), Some(destroy)) if !program.functions[destroy].is_async => {
                        let owner = self.builder.ins().load(types::I64, MEMORY, value, offset + 8);
                        let pinned = self.builder.ins().icmp_imm_u(IntCC::Equal, owner, 2);
                        let (close, done) = (self.builder.create_block(), self.builder.create_block());
                        self.builder.ins().brif(pinned, close, &[], done, &[]);
                        self.builder.switch_to_block(close);
                        let dead = self.iconst(1);
                        self.builder.ins().store(MEMORY, dead, value, offset + 8);
                        self.call(pc, CallTarget::Static(destroy), &[value]);
                        self.builder.ins().jump(done, &[]);
                        self.builder.switch_to_block(done);
                    }
                    _ => {
                        self.helper(|h| h.resource_leave, &[ctx, value]);
                        self.check(pc);
                    }
                }
            }
            I::RegionLeave { region, .. } => {
                // A region never made (#530) or that tracked nothing closes without a call.
                let cell = self.get(region);
                let (made, close, done) = (self.builder.create_block(), self.builder.create_block(), self.builder.create_block());
                self.builder.ins().brif(cell, made, &[], done, &[]);
                self.builder.switch_to_block(made);
                let entries = self.builder.ins().load(types::I64, MEMORY, cell, 16);
                self.builder.ins().brif(entries, close, &[], done, &[]);
                self.builder.switch_to_block(close);
                let I::RegionLeave { keep, .. } = instruction else { unreachable!() };
                let keep: Vec<_> = keep.iter().cloned().collect();
                let count = self.iconst(keep.len() as i64);
                let address = self.spill(&keep);
                self.helper(|h| h.region_leave, &[ctx, cell, address, count]);
                self.check(pc);
                self.builder.ins().jump(done, &[]);
                self.builder.switch_to_block(done);
            }
            I::RegionTransfer { to, .. } if !self.plain => {
                // #530: a transfer into a region not made yet makes it first, then runs as before.
                let target = self.get(to);
                let (make, ready) = (self.builder.create_block(), self.builder.create_block());
                self.builder.ins().brif(target, ready, &[], make, &[]);
                self.builder.switch_to_block(make);
                let made = self.helper(|h| h.region_new, &[ctx]).expect("result");
                self.set(to, made);
                self.builder.ins().jump(ready, &[]);
                self.builder.switch_to_block(ready);
                let mut uses = Vec::new();
                instruction.visit(&mut |access| {
                    if let okkama_sparrow::Access::Use(register) = access {
                        uses.push(register);
                    }
                });
                let address = self.spill(&uses);
                let function = self.iconst(self.function as i64);
                let at = self.iconst(pc as i64);
                self.helper(|h| h.failure_op, &[ctx, function, at, address]);
                self.check(pc);
            }
            other if failure_or_region(other) => {
                let mut uses = Vec::new();
                let mut def = None;
                other.visit(&mut |access| match access {
                    okkama_sparrow::Access::Use(register) => uses.push(register),
                    okkama_sparrow::Access::Def(register) => def = Some(register),
                    _ => {}
                });
                let address = self.spill(&uses);
                let function = self.iconst(self.function as i64);
                let at = self.iconst(pc as i64);
                let result = self.helper(|h| h.failure_op, &[ctx, function, at, address]).expect("result");
                self.check(pc);
                if matches!(other, I::Raise { .. } | I::Panic { .. }) {
                    self.exit_fail();
                    return Ok(true);
                }
                if let Some(def) = def {
                    self.set_word(&def, result);
                }
            }
            I::IntAnd { dst, left, right }
            | I::IntOr { dst, left, right }
            | I::IntXor { dst, left, right }
            | I::IntShl { dst, left, right }
            | I::IntShr { dst, left, right } => {
                // Two inline integers stay inline; the rest goes the exact way.
                let (l, r) = (self.get(left), self.get(right));
                let both = self.builder.ins().band(l, r);
                let small = self.builder.ins().band_imm_u(both, 1);
                let (fast, slow, done) = (self.builder.create_block(), self.builder.create_block(), self.builder.create_block());
                self.builder.set_cold_block(slow);
                self.builder.append_block_param(done, types::I64);
                self.builder.ins().brif(small, fast, &[], slow, &[]);
                self.builder.switch_to_block(fast);
                match instruction {
                    I::IntAnd { .. } => {
                        let value = self.builder.ins().band(l, r);
                        self.builder.ins().jump(done, &[value.into()]);
                    }
                    I::IntOr { .. } => {
                        let value = self.builder.ins().bor(l, r);
                        self.builder.ins().jump(done, &[value.into()]);
                    }
                    I::IntXor { .. } => {
                        let x = self.builder.ins().bxor(l, r);
                        let value = self.builder.ins().bor_imm_u(x, 1);
                        self.builder.ins().jump(done, &[value.into()]);
                    }
                    _ => {
                        let left_shift = matches!(instruction, I::IntShl { .. });
                        let v = self.builder.ins().sshr_imm_u(l, 1);
                        let n = self.builder.ins().sshr_imm_u(r, 1);
                        let in_range = self.builder.ins().icmp_imm_u(IntCC::UnsignedLessThan, n, 62);
                        let commit = self.builder.create_block();
                        self.builder.ins().brif(in_range, commit, &[], slow, &[]);
                        self.builder.switch_to_block(commit);
                        if left_shift {
                            // No bit may be lost: shifting back gives the value,
                            // and the result still fits an inline integer.
                            let shifted = self.builder.ins().ishl(v, n);
                            let tagged = self.builder.ins().ishl_imm_u(shifted, 1);
                            let tagged = self.builder.ins().bor_imm_u(tagged, 1);
                            let back = self.builder.ins().sshr(shifted, n);
                            let same = self.builder.ins().icmp(IntCC::Equal, back, v);
                            let untagged = self.builder.ins().sshr_imm_u(tagged, 1);
                            let fits = self.builder.ins().icmp(IntCC::Equal, untagged, shifted);
                            let ok = self.builder.ins().band(same, fits);
                            let exact = self.builder.create_block();
                            self.builder.ins().brif(ok, exact, &[], slow, &[]);
                            self.builder.switch_to_block(exact);
                            self.builder.ins().jump(done, &[tagged.into()]);
                        } else {
                            let shifted = self.builder.ins().sshr(v, n);
                            let tagged = self.builder.ins().ishl_imm_u(shifted, 1);
                            let tagged = self.builder.ins().bor_imm_u(tagged, 1);
                            self.builder.ins().jump(done, &[tagged.into()]);
                        }
                    }
                }
                self.builder.switch_to_block(slow);
                let result = self.exec_generic(pc, instruction);
                self.builder.ins().jump(done, &[result.into()]);
                self.builder.switch_to_block(done);
                let value = self.builder.block_params(done)[0];
                self.set(&dst.0, value);
            }
            I::IntNot { dst, operand } | I::IntNeg { dst, operand } => {
                let v = self.get(operand);
                let small = self.builder.ins().band_imm_u(v, 1);
                let (fast, slow, done) = (self.builder.create_block(), self.builder.create_block(), self.builder.create_block());
                self.builder.set_cold_block(slow);
                self.builder.append_block_param(done, types::I64);
                self.builder.ins().brif(small, fast, &[], slow, &[]);
                self.builder.switch_to_block(fast);
                if matches!(instruction, I::IntNot { .. }) {
                    // !(2v+1) | 1 == 2(!v)+1
                    let x = self.builder.ins().bnot(v);
                    let value = self.builder.ins().bor_imm_u(x, 1);
                    self.builder.ins().jump(done, &[value.into()]);
                } else {
                    // 2(-v)+1 == 2 - (2v+1)
                    let two = self.iconst(2);
                    let (value, overflow) = self.builder.ins().ssub_overflow(two, v);
                    let commit = self.builder.create_block();
                    self.builder.ins().brif(overflow, slow, &[], commit, &[]);
                    self.builder.switch_to_block(commit);
                    self.builder.ins().jump(done, &[value.into()]);
                }
                self.builder.switch_to_block(slow);
                let result = self.exec_generic(pc, instruction);
                self.builder.ins().jump(done, &[result.into()]);
                self.builder.switch_to_block(done);
                let value = self.builder.block_params(done)[0];
                self.set(&dst.0, value);
            }
            // #1106: byte bit operations inline. A byte is a raw u8 word, so `& | ^ ~` need no check; a shift's
            // count is an int that must be in 0..7, and any other count fails on the shared path.
            I::ByteAnd { dst, left, right } | I::ByteOr { dst, left, right } | I::ByteXor { dst, left, right } => {
                let (l, r) = (self.get(left), self.get(right));
                let value = match instruction {
                    I::ByteAnd { .. } => self.builder.ins().band(l, r),
                    I::ByteOr { .. } => self.builder.ins().bor(l, r),
                    _ => self.builder.ins().bxor(l, r),
                };
                self.set(&dst.0, value);
            }
            I::ByteNot { dst, operand } => {
                let value = self.get(operand);
                let value = self.builder.ins().bxor_imm_u(value, 0xFF);
                self.set(&dst.0, value);
            }
            I::ByteShl { dst, left, right } | I::ByteShr { dst, left, right } | I::ByteUshr { dst, left, right } => {
                let (l, r) = (self.get(left), self.get(right));
                let count = self.small_count(r, 8);
                let fast = self.builder.create_block();
                let slow = self.builder.create_block();
                self.builder.set_cold_block(slow);
                self.builder.ins().brif(count.valid, fast, &[], slow, &[]);
                self.fail_generic(slow, pc, instruction);
                self.builder.switch_to_block(fast);
                let value = if matches!(instruction, I::ByteShl { .. }) {
                    let shifted = self.builder.ins().ishl(l, count.value);
                    self.builder.ins().band_imm_u(shifted, 0xFF)
                } else {
                    // A byte has no sign: `>>>` shifts as `>>` does.
                    self.builder.ins().ushr(l, count.value)
                };
                self.set(&dst.0, value);
            }
            I::IntUshr { dst, left, right } => {
                // Inline for tagged-small operands, a count in 0..63 and a result that is tagged-small again; the
                // rest (BigInt operands, large results, bad counts) goes the exact way.
                let (l, r) = (self.get(left), self.get(right));
                let both = self.builder.ins().band(l, r);
                let small = self.builder.ins().band_imm_u(both, 1);
                let (fast, commit, slow, done) = (
                    self.builder.create_block(),
                    self.builder.create_block(),
                    self.builder.create_block(),
                    self.builder.create_block(),
                );
                self.builder.append_block_param(done, types::I64);
                self.builder.set_cold_block(slow);
                self.builder.ins().brif(small, fast, &[], slow, &[]);
                self.builder.switch_to_block(fast);
                let v = self.builder.ins().sshr_imm_u(l, 1);
                let n = self.builder.ins().sshr_imm_u(r, 1);
                let in_range = self.builder.ins().icmp_imm_u(IntCC::UnsignedLessThan, n, 64);
                self.builder.ins().brif(in_range, commit, &[], slow, &[]);
                self.builder.switch_to_block(commit);
                let shifted = self.builder.ins().ushr(v, n);
                self.tag_if_small(shifted, done, slow);
                self.slow_generic(slow, done, pc, instruction);
                self.builder.switch_to_block(done);
                let value = self.builder.block_params(done)[0];
                self.set(&dst.0, value);
            }
            I::IntAbs { dst, operand } => {
                // |2v+1| for v < 0 is 2 - (2v+1); only SMALL_MIN overflows (its absolute value needs a BigInt).
                let v = self.get(operand);
                let small = self.builder.ins().band_imm_u(v, 1);
                let (fast, negative, slow, done) = (
                    self.builder.create_block(),
                    self.builder.create_block(),
                    self.builder.create_block(),
                    self.builder.create_block(),
                );
                self.builder.append_block_param(done, types::I64);
                self.builder.set_cold_block(slow);
                self.builder.ins().brif(small, fast, &[], slow, &[]);
                self.builder.switch_to_block(fast);
                let below = self.builder.ins().icmp_imm_s(IntCC::SignedLessThan, v, 0);
                self.builder.ins().brif(below, negative, &[], done, &[v.into()]);
                self.builder.switch_to_block(negative);
                let two = self.iconst(2);
                let (value, overflow) = self.builder.ins().ssub_overflow(two, v);
                let commit = self.builder.create_block();
                self.builder.ins().brif(overflow, slow, &[], commit, &[]);
                self.builder.switch_to_block(commit);
                self.builder.ins().jump(done, &[value.into()]);
                self.slow_generic(slow, done, pc, instruction);
                self.builder.switch_to_block(done);
                let value = self.builder.block_params(done)[0];
                self.set(&dst.0, value);
            }
            I::Sign { dst, operand } if matches!(self.register_type(operand), Some(Type::Int)) => {
                // A tagged-small w = 2v+1: v > 0 iff w > 1, v < 0 iff w < 0. BigInts go the exact way.
                let v = self.get(operand);
                let small = self.builder.ins().band_imm_u(v, 1);
                let (fast, slow, done) = (self.builder.create_block(), self.builder.create_block(), self.builder.create_block());
                self.builder.append_block_param(done, types::I64);
                self.builder.set_cold_block(slow);
                self.builder.ins().brif(small, fast, &[], slow, &[]);
                self.builder.switch_to_block(fast);
                let above = self.builder.ins().icmp_imm_s(IntCC::SignedGreaterThan, v, 1);
                let below = self.builder.ins().icmp_imm_s(IntCC::SignedLessThan, v, 0);
                let tagged = self.tagged_sign(above, below);
                self.builder.ins().jump(done, &[tagged.into()]);
                self.slow_generic(slow, done, pc, instruction);
                self.builder.switch_to_block(done);
                let value = self.builder.block_params(done)[0];
                self.set(&dst.0, value);
            }
            I::Sign { dst, operand } if matches!(self.register_type(operand), Some(Type::Float)) => {
                // Both zeros have sign 0; a float is never NaN.
                let x = self.get(operand);
                let zero = self.builder.ins().f64const(0.0);
                let above = self.builder.ins().fcmp(FloatCC::GreaterThan, x, zero);
                let below = self.builder.ins().fcmp(FloatCC::LessThan, x, zero);
                let tagged = self.tagged_sign(above, below);
                self.set(&dst.0, tagged);
            }
            I::FloatSqrt { dst, operand } => {
                // A negative operand fails (its slow path never returns); sqrt of a finite x >= 0, -0 included, is
                // finite and exact in hardware.
                let x = self.get(operand);
                let zero = self.builder.ins().f64const(0.0);
                let negative = self.builder.ins().fcmp(FloatCC::LessThan, x, zero);
                let (fast, slow) = (self.builder.create_block(), self.builder.create_block());
                self.builder.set_cold_block(slow);
                self.builder.ins().brif(negative, slow, &[], fast, &[]);
                self.fail_generic(slow, pc, instruction);
                self.builder.switch_to_block(fast);
                let value = self.builder.ins().sqrt(x);
                self.set(&dst.0, value);
            }
            I::Bits { op, byte, dst, value: source, count } => {
                self.bits(pc, instruction, *op, *byte, &dst.0, source, count.as_ref())?;
            }
            I::StrCmp { op: op @ (Cmp::Eq | Cmp::Ne), dst, left, right } => {
                // The same string, or strings of different byte lengths, decide
                // equality inline; the rest compares bytes in the slow path.
                let (l, r) = (self.get(left), self.get(right));
                let equal = u64::from(*op == Cmp::Eq) as i64;
                let (decided, compare, slow, done) = (
                    self.builder.create_block(),
                    self.builder.create_block(),
                    self.builder.create_block(),
                    self.builder.create_block(),
                );
                self.builder.append_block_param(decided, types::I64);
                self.builder.append_block_param(done, types::I64);
                let same = self.builder.ins().icmp(IntCC::Equal, l, r);
                let yes = self.iconst(equal);
                self.builder.ins().brif(same, decided, &[yes.into()], compare, &[]);
                self.builder.switch_to_block(compare);
                let (hl, hr) = (
                    self.builder.ins().load(types::I64, MEMORY, l, 0),
                    self.builder.ins().load(types::I64, MEMORY, r, 0),
                );
                let (nl, nr) = (self.builder.ins().ushr_imm_u(hl, 32), self.builder.ins().ushr_imm_u(hr, 32));
                let differ = self.builder.ins().icmp(IntCC::NotEqual, nl, nr);
                let no = self.iconst(1 - equal);
                self.builder.ins().brif(differ, decided, &[no.into()], slow, &[]);
                self.builder.switch_to_block(decided);
                let flag = self.builder.block_params(decided)[0];
                self.builder.ins().jump(done, &[flag.into()]);
                self.builder.switch_to_block(slow);
                let result = self.exec_generic(pc, instruction);
                self.builder.ins().jump(done, &[result.into()]);
                self.builder.switch_to_block(done);
                let value = self.builder.block_params(done)[0];
                self.set(&dst.0, value);
            }
            I::ValCmp { op: op @ (Cmp::Eq | Cmp::Ne), dst, left, right }
                if matches!(self.kinds[left.0 as usize], Kind::Int | Kind::Raw | Kind::Byte | Kind::Float) =>
            {
                // Scalars compare by word (exact ints: two inline integers),
                // floats numerically (#314); anything else takes the
                // structural slow path.
                let int = self.kinds[left.0 as usize] == Kind::Int;
                let (l, r) = (self.get(left), self.get(right));
                let flag = if self.kinds[left.0 as usize] == Kind::Float {
                    let condition = if *op == Cmp::Eq { FloatCC::Equal } else { FloatCC::NotEqual };
                    self.builder.ins().fcmp(condition, l, r)
                } else {
                    let condition = if *op == Cmp::Eq { IntCC::Equal } else { IntCC::NotEqual };
                    self.builder.ins().icmp(condition, l, r)
                };
                let flag = self.builder.ins().uextend(types::I64, flag);
                if int {
                    let both = self.builder.ins().band(l, r);
                    let small = self.builder.ins().band_imm_u(both, 1);
                    let (fast, slow, done) = (self.builder.create_block(), self.builder.create_block(), self.builder.create_block());
                    self.builder.append_block_param(done, types::I64);
                    self.builder.ins().brif(small, fast, &[], slow, &[]);
                    self.builder.switch_to_block(fast);
                    self.builder.ins().jump(done, &[flag.into()]);
                    self.builder.switch_to_block(slow);
                    let result = self.exec_generic(pc, instruction);
                    self.builder.ins().jump(done, &[result.into()]);
                    self.builder.switch_to_block(done);
                    let value = self.builder.block_params(done)[0];
                    self.set(&dst.0, value);
                } else {
                    self.set(&dst.0, flag);
                }
            }
            I::BuilderNew { dst } => {
                let Some(Type::Builder(element)) = self.register_type(&dst.0) else {
                    self.generic_instruction(pc, instruction);
                    return Ok(false);
                };
                let repr = Kind::of(program, *element).repr();
                let repr = self.iconst(repr as i64);
                let result = self.helper(|h| h.builder_new, &[ctx, repr]).expect("result");
                self.check(pc);
                self.set(&dst.0, result);
            }
            I::BuilderFinish { dst, builder } => {
                let Some(Type::Array(element)) = self.register_type(&dst.0) else {
                    self.generic_instruction(pc, instruction);
                    return Ok(false);
                };
                let packed = Kind::of(program, *element) == Kind::Byte;
                let packed = self.iconst(i64::from(packed));
                let builder = self.get(builder);
                let result = self.helper(|h| h.builder_finish, &[ctx, builder, packed]).expect("result");
                self.check(pc);
                self.set(&dst.0, result);
            }
            I::RatNew { dst, left, right } => {
                let (left, right) = (self.get(left), self.get(right));
                let result = self.helper(|h| h.rat_new, &[ctx, left, right]).expect("result");
                self.check(pc);
                self.set(&dst.0, result);
            }
            I::MapLen { dst, map } if matches!(self.register_type(map), Some(Type::Map(..))) => {
                let map = self.get(map);
                let result = self.helper(|h| h.map_len, &[ctx, map]).expect("result");
                self.check(pc);
                self.set(&dst.0, result);
            }
            I::BuilderPush { dst, builder, value } => {
                let (builder, value) = (self.get(builder), self.word(value));
                let result = self.helper(|h| h.builder_push, &[ctx, builder, value]).expect("result");
                self.set(&dst.0, result);
            }
            I::Convert { dst, source }
                if matches!(self.register_type(source), Some(Type::Int)) && matches!(self.register_type(&dst.0), Some(Type::Float)) =>
            {
                let value = self.get(source);
                let small = self.builder.ins().band_imm_u(value, 1);
                let (fast, slow, done) = (self.builder.create_block(), self.builder.create_block(), self.builder.create_block());
                self.builder.append_block_param(done, types::F64);
                // #1105: no safepoint on the fast conversions (#537 had kept rt_exec's opportunity). A conversion adds
                // nothing to the collection counter, so its safepoint only ever collected for its neighbours' work;
                // the same code without a conversion never had that safepoint either.
                self.builder.set_cold_block(slow);
                self.builder.ins().brif(small, fast, &[], slow, &[]);
                self.builder.switch_to_block(fast);
                let integer = self.builder.ins().sshr_imm_u(value, 1);
                let result = self.builder.ins().fcvt_from_sint(types::F64, integer);
                self.builder.ins().jump(done, &[result.into()]);
                self.builder.switch_to_block(slow);
                let bits = self.exec_generic(pc, instruction);
                let result = self.builder.ins().bitcast(types::F64, MemFlagsData::new(), bits);
                self.builder.ins().jump(done, &[result.into()]);
                self.builder.switch_to_block(done);
                let result = self.builder.block_params(done)[0];
                self.set(&dst.0, result);
            }
            I::Convert { dst, source }
                if matches!(self.register_type(source), Some(Type::Float)) && matches!(self.register_type(&dst.0), Some(Type::Int)) =>
            {
                let value = self.get(source);
                let truncated = self.builder.ins().trunc(value);
                // Ordered comparisons reject NaN and infinities. Both bounds
                // are exact binary64 powers of two; SMALL_MAX as f64 would
                // round UP and incorrectly allow 2^62, which needs a BigInt.
                let lower = self.builder.ins().f64const(word::SMALL_MIN as f64);
                let upper = self.builder.ins().f64const(-(word::SMALL_MIN as f64));
                let above = self.builder.ins().fcmp(FloatCC::GreaterThanOrEqual, truncated, lower);
                let below = self.builder.ins().fcmp(FloatCC::LessThan, truncated, upper);
                let small = self.builder.ins().band(above, below);
                let (fast, slow, done) = (self.builder.create_block(), self.builder.create_block(), self.builder.create_block());
                self.builder.append_block_param(done, types::I64);
                self.builder.set_cold_block(slow);
                self.builder.ins().brif(small, fast, &[], slow, &[]);
                self.builder.switch_to_block(fast);
                // The conversion is reached only after the range proof, so the
                // saturating form (a bare `fcvtzs` on aarch64, no trap checks)
                // converts exactly; tagging cannot overflow the tagged-small domain.
                let integer = self.builder.ins().fcvt_to_sint_sat(types::I64, truncated);
                let doubled = self.builder.ins().ishl_imm_u(integer, 1);
                let result = self.builder.ins().bor_imm_u(doubled, 1);
                self.builder.ins().jump(done, &[result.into()]);
                self.builder.switch_to_block(slow);
                let result = self.exec_generic(pc, instruction);
                self.builder.ins().jump(done, &[result.into()]);
                self.builder.switch_to_block(done);
                let result = self.builder.block_params(done)[0];
                self.set(&dst.0, result);
            }
            I::Convert { dst, source }
                if matches!(self.register_type(source), Some(Type::Int)) && matches!(self.register_type(&dst.0), Some(Type::Str)) =>
            {
                let value = self.get(source);
                let text = self.helper(|h| h.int_text, &[ctx, value]).expect("result");
                self.check(pc);
                self.set(&dst.0, text);
            }
            // A code point or a byte is always a small int: tag it inline
            // (`character.code()` ran through the generic helper, #503).
            I::Convert { dst, source }
                if matches!(self.register_type(source), Some(Type::Char) | Some(Type::Byte))
                    && matches!(self.register_type(&dst.0), Some(Type::Int)) =>
            {
                let value = self.get(source);
                let doubled = self.builder.ins().ishl_imm_u(value, 1);
                let tagged = self.builder.ins().bor_imm_u(doubled, 1);
                self.set(&dst.0, tagged);
            }
            I::StrLen { dst, text } => {
                // ASCII strings know their length in characters: the byte length.
                let string = self.get(text);
                let header = self.builder.ins().load(types::I64, MEMORY, string, 0);
                let ascii = self.builder.ins().band_imm_u(header, heap::ASCII as i64);
                let (fast, slow, done) = (self.builder.create_block(), self.builder.create_block(), self.builder.create_block());
                self.builder.append_block_param(done, types::I64);
                self.builder.ins().brif(ascii, fast, &[], slow, &[]);
                self.builder.switch_to_block(fast);
                let length = self.builder.ins().ushr_imm_u(header, 32);
                let doubled = self.builder.ins().ishl_imm_u(length, 1);
                let tagged = self.builder.ins().bor_imm_u(doubled, 1);
                self.builder.ins().jump(done, &[tagged.into()]);
                self.builder.switch_to_block(slow);
                let result = self.exec_generic(pc, instruction);
                self.builder.ins().jump(done, &[result.into()]);
                self.builder.switch_to_block(done);
                let value = self.builder.block_params(done)[0];
                self.set(&dst.0, value);
            }
            I::EnumCase { dst, ty, case } => {
                let value = self.iconst(((u64::from(ty.0) << 32) | u64::from(*case)) as i64);
                self.set_word(&dst.0, value);
            }
            I::EnumCmp { op, dst, left, right } => {
                // Enum words compare as unsigned integers (see `ops`).
                let (l, r) = (self.word(left), self.word(right));
                let condition = match op {
                    Cmp::Eq => IntCC::Equal,
                    Cmp::Ne => IntCC::NotEqual,
                    Cmp::Lt => IntCC::UnsignedLessThan,
                    Cmp::Le => IntCC::UnsignedLessThanOrEqual,
                    Cmp::Gt => IntCC::UnsignedGreaterThan,
                    Cmp::Ge => IntCC::UnsignedGreaterThanOrEqual,
                };
                let flag = self.builder.ins().icmp(condition, l, r);
                let flag = self.builder.ins().uextend(types::I64, flag);
                self.set(&dst.0, flag);
            }
            I::ByteFrom { dst, source, .. } if self.kinds[source.0 as usize] == Kind::Int => {
                let v = self.get(source);
                let n = self.builder.ins().sshr_imm_u(v, 1);
                let small = self.builder.ins().band_imm_u(v, 1);
                let in_range = self.builder.ins().icmp_imm_u(IntCC::UnsignedLessThanOrEqual, n, 255);
                let in_range = self.builder.ins().uextend(types::I64, in_range);
                let ok = self.builder.ins().band(small, in_range);
                let (fast, slow, done) = (self.builder.create_block(), self.builder.create_block(), self.builder.create_block());
                self.builder.append_block_param(done, types::I64);
                self.builder.ins().brif(ok, fast, &[], slow, &[]);
                self.builder.switch_to_block(fast);
                self.builder.ins().jump(done, &[n.into()]);
                self.builder.switch_to_block(slow);
                let result = self.exec_generic(pc, instruction);
                self.builder.ins().jump(done, &[result.into()]);
                self.builder.switch_to_block(done);
                let value = self.builder.block_params(done)[0];
                self.set(&dst.0, value);
            }
            other if generic(other) => self.generic_instruction(pc, other),
            _ => return Err(unsupported()),
        }
        Ok(false)
    }

    /// Is `register` statically a byte array (packed, one byte per element)?
    fn bytes_array(&self, register: okkama_sparrow::Register) -> bool {
        matches!(self.register_type(&register), Some(Type::Array(element)) if Kind::of(&self.loaded.program, *element) == Kind::Byte)
    }

    /// Element `position` (in bounds) of an array whose header is `header`: a
    /// word, or a byte of a packed array. One branch on two header bits keeps
    /// flat arrays on the plain load; a shared version (`heap::SHARED`, #246)
    /// reads from the address of its first element (one load more), a trie (`heap::TRIE`,
    /// #250) calls a helper. Byte arrays are never either.
    fn element(&mut self, array: Value, header: Value, position: Value, bytes: bool) -> Value {
        if bytes {
            let address = self.builder.ins().iadd(array, position);
            return self.builder.ins().uload8(types::I64, MEMORY, address, 8);
        }
        let special = self.builder.ins().band_imm_u(header, (heap::SHARED | heap::TRIE) as i64);
        let (flat, other, view, trie, done) = (
            self.builder.create_block(),
            self.builder.create_block(),
            self.builder.create_block(),
            self.builder.create_block(),
            self.builder.create_block(),
        );
        self.builder.append_block_param(done, types::I64);
        self.builder.ins().brif(special, other, &[], flat, &[]);
        self.builder.switch_to_block(flat);
        let offset = self.builder.ins().ishl_imm_u(position, 3);
        let address = self.builder.ins().iadd(array, offset);
        let value = self.builder.ins().load(types::I64, MEMORY, address, 8);
        self.builder.ins().jump(done, &[value.into()]);
        self.builder.switch_to_block(other);
        let shared = self.builder.ins().band_imm_u(header, heap::SHARED as i64);
        self.builder.ins().brif(shared, view, &[], trie, &[]);
        self.builder.switch_to_block(view);
        let first = self.builder.ins().load(types::I64, MEMORY, array, 8);
        let offset = self.builder.ins().ishl_imm_u(position, 3);
        let address = self.builder.ins().iadd(first, offset);
        let value = self.builder.ins().load(types::I64, MEMORY, address, 0);
        self.builder.ins().jump(done, &[value.into()]);
        self.builder.switch_to_block(trie);
        let ctx = self.ctx();
        let value = self.helper(|h| h.trie_get, &[ctx, array, position]).expect("result");
        self.builder.ins().jump(done, &[value.into()]);
        self.builder.switch_to_block(done);
        self.builder.block_params(done)[0]
    }

    /// Inline access to an Atomic cell (`[header, value, closed]`; a closed
    /// cell takes the intrinsic call, which throws AtomicClosed) and to the
    /// set flag of an Event cell (`[header, initial, set, waiters]`).
    fn atomic(&mut self, pc: usize, function: u32, access: Intrinsic, args: &[okkama_sparrow::Register]) -> Value {
        let cell = self.get(&args[0]);
        let ctx = self.ctx();
        match access {
            Intrinsic::AtomicClosed | Intrinsic::EventReady => self.builder.ins().load(types::I64, MEMORY, cell, 16),
            Intrinsic::AtomicClose => {
                let one = self.iconst(1);
                self.builder.ins().store(MEMORY, one, cell, 16);
                let unbound = self.iconst(0);
                self.builder.ins().store(MEMORY, unbound, cell, 8);
                self.iconst(0)
            }
            _ => {
                let closed = self.builder.ins().load(types::I64, MEMORY, cell, 16);
                let (open, slow, done) = (self.builder.create_block(), self.builder.create_block(), self.builder.create_block());
                self.builder.append_block_param(done, types::I64);
                self.builder.ins().brif(closed, slow, &[], open, &[]);
                self.builder.switch_to_block(open);
                let result = if access == Intrinsic::AtomicValue {
                    self.builder.ins().load(types::I64, MEMORY, cell, 8)
                } else {
                    let value = self.word(&args[1]);
                    self.builder.ins().store(MEMORY, value, cell, 8);
                    self.iconst(0)
                };
                self.builder.ins().jump(done, &[result.into()]);
                self.builder.switch_to_block(slow);
                let address = self.spill(args);
                let index = self.iconst(i64::from(function));
                let count = self.iconst(args.len() as i64);
                let result = self.helper(|h| h.intrinsic, &[ctx, index, address, count]).expect("result");
                self.check(pc);
                self.builder.ins().jump(done, &[result.into()]);
                self.builder.switch_to_block(done);
                self.builder.block_params(done)[0]
            }
        }
    }


    /// An array literal of at most [`INLINE_ARRAY_LITERAL`] elements, allocated inline in the layouts the
    /// `array` helper writes (#1167): a header and one word per element, or for a byte array the PACKED header and
    /// its bytes packed eight to a word. Every word is written before the array is published.
    fn array_literal(&mut self, repr: u64, elements: &[okkama_sparrow::Register]) -> Value {
        if repr == runtime::ARRAY_BYTES {
            let words = elements.len().div_ceil(8);
            let pointer = self.allocate(1 + words);
            let header = heap::header(heap::ARRAY, Repr::Raw, heap::length(elements.len())) | heap::PACKED;
            let header = self.iconst(header as i64);
            self.builder.ins().store(MEMORY, header, pointer, 0);
            for (index, chunk) in elements.chunks(8).enumerate() {
                let mut packed = self.iconst(0);
                for (position, element) in chunk.iter().enumerate() {
                    let byte = self.get(element);
                    let byte = self.builder.ins().band_imm_u(byte, 0xFF);
                    let byte = self.builder.ins().ishl_imm_u(byte, (position * 8) as i64);
                    packed = self.builder.ins().bor(packed, byte);
                }
                self.builder.ins().store(MEMORY, packed, pointer, ((index + 1) * 8) as i32);
            }
            return pointer;
        }
        let pointer = self.allocate(elements.len() + 1);
        let header = heap::header(heap::ARRAY, Repr::from_bits(repr), heap::length(elements.len()));
        let header = self.iconst(header as i64);
        self.builder.ins().store(MEMORY, header, pointer, 0);
        for (i, element) in elements.iter().enumerate() {
            let value = self.word(element);
            self.builder.ins().store(MEMORY, value, pointer, ((i + 1) * 8) as i32);
        }
        pointer
    }

    /// A shift or rotation count (#1106): `valid` when the int word is tagged-small and in 0..limit, `value` the
    /// untagged count (meaningful only when valid).
    fn small_count(&mut self, word: Value, limit: i64) -> Count {
        let small = self.builder.ins().band_imm_u(word, 1);
        let value = self.builder.ins().sshr_imm_u(word, 1);
        let inside = self.builder.ins().icmp_imm_u(IntCC::UnsignedLessThan, value, limit);
        let inside = self.builder.ins().uextend(types::I64, inside);
        let valid = self.builder.ins().band(small, inside);
        Count { valid, value }
    }

    /// Jump to `done` with `value` tagged when it is tagged-small, otherwise to `slow` (#1106).
    fn tag_if_small(&mut self, value: Value, done: Block, slow: Block) {
        let doubled = self.builder.ins().ishl_imm_u(value, 1);
        let back = self.builder.ins().sshr_imm_u(doubled, 1);
        let fits = self.builder.ins().icmp(IntCC::Equal, back, value);
        let tagged = self.builder.ins().bor_imm_u(doubled, 1);
        self.builder.ins().brif(fits, done, &[tagged.into()], slow, &[]);
    }

    /// The tagged int -1, 0 or 1 from the two comparison flags of a sign (#1106).
    fn tagged_sign(&mut self, above: Value, below: Value) -> Value {
        let above = self.builder.ins().uextend(types::I64, above);
        let below = self.builder.ins().uextend(types::I64, below);
        let sign = self.builder.ins().isub(above, below);
        let doubled = self.builder.ins().ishl_imm_u(sign, 1);
        self.builder.ins().bor_imm_u(doubled, 1)
    }

    /// In the cold block `slow`: the shared path of `instruction`, whose result joins `done` (#1106).
    fn slow_generic(&mut self, slow: Block, done: Block, pc: usize, instruction: &I) {
        self.builder.switch_to_block(slow);
        let result = self.exec_generic(pc, instruction);
        self.builder.ins().jump(done, &[result.into()]);
    }

    /// In the cold block `slow`: the shared path of `instruction` on inputs it always refuses, so it never returns
    /// (#1106; the trap is a broken invariant, as after the float slow path of #1105).
    fn fail_generic(&mut self, slow: Block, pc: usize, instruction: &I) {
        self.builder.switch_to_block(slow);
        self.exec_generic(pc, instruction);
        self.builder.ins().trap(TrapCode::unwrap_user(GENERIC_SLOW_PATH_RETURNED));
    }

    /// `bits` actions inline (#1106): counts, rotations, bit reversal and byte swap of a byte or a tagged-small
    /// int; a BigInt operand, a bad rotation count or an int result outside tagged-small goes the exact way.
    #[allow(clippy::too_many_arguments)]
    fn bits(
        &mut self,
        pc: usize,
        instruction: &I,
        op: okkama_sparrow::BitOp,
        byte: bool,
        dst: &okkama_sparrow::Register,
        source: &okkama_sparrow::Register,
        count: Option<&okkama_sparrow::Register>,
    ) -> Result<(), Error> {
        use okkama_sparrow::BitOp;
        let word = self.get(source);
        let rotates = matches!(op, BitOp::RotateLeft | BitOp::RotateRight);
        let width: i64 = if byte { 8 } else { 64 };
        let (fast, slow, done) = (self.builder.create_block(), self.builder.create_block(), self.builder.create_block());
        self.builder.append_block_param(done, types::I64);
        self.builder.set_cold_block(slow);
        let mut valid = if byte { self.iconst(1) } else { self.builder.ins().band_imm_u(word, 1) };
        let mut amount = None;
        if rotates {
            let Some(count) = count else { return Err(Error(format!("bits {op:?} without a count"))) };
            let count = self.get(count);
            let count = self.small_count(count, width);
            valid = self.builder.ins().band(valid, count.valid);
            amount = Some(count.value);
        }
        self.builder.ins().brif(valid, fast, &[], slow, &[]);
        self.builder.switch_to_block(fast);
        let tag = |emit: &mut Self, value: Value| {
            let doubled = emit.builder.ins().ishl_imm_u(value, 1);
            emit.builder.ins().bor_imm_u(doubled, 1)
        };
        if byte {
            let w = word;
            let value = match op {
                BitOp::BitCount => {
                    let n = self.builder.ins().popcnt(w);
                    tag(self, n)
                }
                BitOp::LeadingZeros => {
                    let n = self.builder.ins().clz(w);
                    let n = self.builder.ins().iadd_imm_s(n, -56);
                    tag(self, n)
                }
                BitOp::TrailingZeros => {
                    // A zero byte has eight trailing zeros: bit 8 stops the count there.
                    let guarded = self.builder.ins().bor_imm_u(w, 0x100);
                    let n = self.builder.ins().ctz(guarded);
                    tag(self, n)
                }
                BitOp::RotateLeft | BitOp::RotateRight => {
                    let n = amount.expect("count");
                    let eight = self.iconst(8);
                    let back = self.builder.ins().isub(eight, n);
                    let (first, second) = if op == BitOp::RotateLeft {
                        (self.builder.ins().ishl(w, n), self.builder.ins().ushr(w, back))
                    } else {
                        (self.builder.ins().ushr(w, n), self.builder.ins().ishl(w, back))
                    };
                    let both = self.builder.ins().bor(first, second);
                    self.builder.ins().band_imm_u(both, 0xFF)
                }
                BitOp::ReverseBits => {
                    let reversed = self.builder.ins().bitrev(w);
                    self.builder.ins().ushr_imm_u(reversed, 56)
                }
                BitOp::SwapBytes => w,
            };
            self.builder.ins().jump(done, &[value.into()]);
        } else {
            let v = self.builder.ins().sshr_imm_u(word, 1);
            match op {
                BitOp::BitCount | BitOp::LeadingZeros | BitOp::TrailingZeros => {
                    let n = match op {
                        BitOp::BitCount => self.builder.ins().popcnt(v),
                        BitOp::LeadingZeros => self.builder.ins().clz(v),
                        _ => self.builder.ins().ctz(v),
                    };
                    let tagged = tag(self, n);
                    self.builder.ins().jump(done, &[tagged.into()]);
                }
                _ => {
                    let value = match op {
                        BitOp::RotateLeft => self.builder.ins().rotl(v, amount.expect("count")),
                        BitOp::RotateRight => self.builder.ins().rotr(v, amount.expect("count")),
                        BitOp::ReverseBits => self.builder.ins().bitrev(v),
                        _ => self.builder.ins().bswap(v),
                    };
                    self.tag_if_small(value, done, slow);
                }
            }
        }
        self.slow_generic(slow, done, pc, instruction);
        self.builder.switch_to_block(done);
        let value = self.builder.block_params(done)[0];
        self.set(dst, value);
        Ok(())
    }

    /// Compile `instruction` as a call of the shared slow path, defining its result.
    fn generic_instruction(&mut self, pc: usize, instruction: &I) {
        let mut def = None;
        instruction.visit(&mut |access| {
            if let okkama_sparrow::Access::Def(register) = access {
                def = Some(register);
            }
        });
        let result = self.exec_generic(pc, instruction);
        if let Some(def) = def {
            self.set_word(&def, result);
        }
    }

    /// Run `instruction` through the shared slow path (`rt_exec`); its result word.
    fn exec_generic(&mut self, pc: usize, instruction: &I) -> Value {
        let mut uses = Vec::new();
        instruction.visit(&mut |access| {
            if let okkama_sparrow::Access::Use(register) = access {
                uses.push(register);
            }
        });
        let ctx = self.ctx();
        let address = self.spill(&uses);
        let function = self.iconst(self.function as i64);
        let at = self.iconst(pc as i64);
        let count = self.iconst(uses.len() as i64);
        let registers: Box<[okkama_sparrow::Register]> = uses.into_boxed_slice();
        let table = self.iconst(registers.as_ptr() as i64);
        self.jit.operands.push(registers);
        let result = self.helper(|h| h.exec, &[ctx, function, at, address, table, count]).expect("result");
        self.check(pc);
        result
    }



    /// #912: with `OKKAMA_DEV=runtime`, count this site's executions (0: an identity is requested, 1: a region
    /// add). Nothing is emitted otherwise, and never in a release build.
    fn dev_site(&mut self, kind: u64) {
        static GATE: okkama_devtrace::Gate = okkama_devtrace::Gate::new("runtime");
        if !GATE.on() {
            return;
        }
        let ctx = self.ctx();
        let (kind, function, pc) = (self.iconst(kind as i64), self.iconst(self.function as i64), self.iconst(self.pc as i64));
        self.helper(|h| h.dev_site, &[ctx, kind, function, pc]);
    }

    /// #909: the offset of the hidden identity words of `descriptor` when machine code may handle them itself: an
    /// inline identity (#528) and nothing that observes each step (no stats, no devtrace, no destroy trace).
    fn inline_identity(&self, descriptor: usize) -> Option<i32> {
        static GATE: okkama_devtrace::Gate = okkama_devtrace::Gate::new("runtime");
        if self.count_helpers || self.plain || GATE.on() || !self.resources.inline_ownership() {
            return None;
        }
        self.resources.inline[descriptor].map(|offset| offset as i32)
    }

    /// `inline_identity` of a static resource register's object type.
    fn static_identity(&self, register: &okkama_sparrow::Register) -> Option<i32> {
        match self.register_type(register) {
            Some(Type::Object(object)) => self.inline_identity(object.0 as usize),
            _ => None,
        }
    }

    /// The words of a new object (#528: the heap adds its identity words).
    fn object_words(&self, descriptor: usize, fields: usize) -> usize {
        self.heap.object_words(descriptor as u32, fields)
    }

    /// Branch to `maybe` when `value` may reach a free identity, else to `never`: the header test of
    /// `Runtime::may_reach_free` without the owner lookup (an object with an identity goes to `maybe`).
    fn may_reach_identity(&mut self, value: Value, maybe: Block, never: Block) {
        // UNBOUND (0) and tagged immediates hold no pointer.
        let pointer = self.builder.create_block();
        let tag = self.builder.ins().band_imm_u(value, 1);
        let low = self.builder.ins().icmp_imm_u(IntCC::NotEqual, tag, 0);
        let zero = self.builder.ins().icmp_imm_u(IntCC::Equal, value, 0);
        let none = self.builder.ins().bor(low, zero);
        self.builder.ins().brif(none, never, &[], pointer, &[]);
        self.builder.switch_to_block(pointer);
        self.pointer_may_reach_identity(value, maybe, never, true);
    }

    /// The header part of the test above, for a word known to be a heap pointer. With `summaries`, a summarized
    /// container goes to `never`: it reaches no free identity (a walk from no region stops at it).
    fn pointer_may_reach_identity(&mut self, value: Value, maybe: Block, never: Block, summaries: bool) {
        let header = self.builder.ins().load(types::I64, MEMORY, value, 0);
        let kind = self.builder.ins().band_imm_u(header, 0xff);
        let is_object = self.builder.ins().icmp_imm_u(IntCC::Equal, kind, heap::OBJECT as i64);
        let (object, rest) = (self.builder.create_block(), self.builder.create_block());
        self.builder.ins().brif(is_object, object, &[], rest, &[]);
        self.builder.switch_to_block(object);
        let identity = self.builder.ins().band_imm_u(header, heap::IDENTITY as i64);
        self.builder.ins().brif(identity, maybe, &[], never, &[]);
        self.builder.switch_to_block(rest);
        if summaries {
            let summarized = self.builder.ins().band_imm_u(header, heap::SUMMARY as i64);
            let open = self.builder.create_block();
            self.builder.ins().brif(summarized, never, &[], open, &[]);
            self.builder.switch_to_block(open);
        }
        // A reference array (not packed) or a closure, payload or frame may reach one; nothing else can.
        let is_array = self.builder.ins().icmp_imm_u(IntCC::Equal, kind, heap::ARRAY as i64);
        let (array, other) = (self.builder.create_block(), self.builder.create_block());
        self.builder.ins().brif(is_array, array, &[], other, &[]);
        self.builder.switch_to_block(array);
        // Repr bits 9–10: 2 and 3 are Ref (`Repr::from_bits`); a packed array holds bytes.
        let bits = self.builder.ins().band_imm_u(header, (heap::PACKED | (2 << 9)) as i64);
        let reference = self.builder.ins().icmp_imm_u(IntCC::Equal, bits, (2 << 9) as i64);
        self.builder.ins().brif(reference, maybe, &[], never, &[]);
        self.builder.switch_to_block(other);
        let closure = self.builder.ins().icmp_imm_u(IntCC::Equal, kind, heap::CLOSURE as i64);
        let payload = self.builder.ins().icmp_imm_u(IntCC::Equal, kind, heap::PAYLOAD as i64);
        let frame = self.builder.ins().icmp_imm_u(IntCC::Equal, kind, heap::FRAME as i64);
        let either = self.builder.ins().bor(closure, payload);
        let any = self.builder.ins().bor(either, frame);
        self.builder.ins().brif(any, maybe, &[], never, &[]);
    }

    /// Branch to `maybe` when field `word` of representation `repr` may reach an identity, as
    /// `Runtime::fields_may_reach_identity` decides it (an unbound reference field may: it is set later).
    fn field_may_reach_identity(&mut self, word: Value, repr: Repr, maybe: Block, never: Block) {
        let pointer = self.builder.create_block();
        match repr {
            Repr::Raw => {
                self.builder.ins().jump(never, &[]);
                return;
            }
            Repr::Int => {
                let tag = self.builder.ins().band_imm_u(word, 1);
                let low = self.builder.ins().icmp_imm_u(IntCC::NotEqual, tag, 0);
                let zero = self.builder.ins().icmp_imm_u(IntCC::Equal, word, 0);
                let none = self.builder.ins().bor(low, zero);
                self.builder.ins().brif(none, never, &[], pointer, &[]);
            }
            Repr::Ref => {
                let zero = self.builder.ins().icmp_imm_u(IntCC::Equal, word, 0);
                self.builder.ins().brif(zero, maybe, &[], pointer, &[]);
            }
        }
        self.builder.switch_to_block(pointer);
        // Here any identity counts, owned or not (`created_built`): summaries do not answer that.
        self.pointer_may_reach_identity(word, maybe, never, false);
    }

    /// Give a new object a resource identity when its type can reach a destroy.
    fn created(&mut self, descriptor: usize, object: Value) {
        let resources = &self.resources;
        if !resources.enabled || !resources.flags[descriptor] {
            return;
        }
        self.dev_site(0);
        if let Some(offset) = self.inline_identity(descriptor) {
            // #909: an object with a destroy of its own numbers its identity in machine code: serial + 1, the
            // serial and a free owner in its hidden words, the identity bit in its header (`Runtime::created`).
            let counter = self.iconst(self.resources.serial_address() as i64);
            let serial = self.builder.ins().load(types::I64, MEMORY, counter, 0);
            let serial = self.builder.ins().iadd_imm_u(serial, 1);
            self.builder.ins().store(MEMORY, serial, counter, 0);
            self.builder.ins().store(MEMORY, serial, object, offset);
            let free = self.iconst(0);
            self.builder.ins().store(MEMORY, free, object, offset + 8);
            let header = self.builder.ins().load(types::I64, MEMORY, object, 0);
            let header = self.builder.ins().bor_imm_u(header, heap::IDENTITY as i64);
            self.builder.ins().store(MEMORY, header, object, 0);
            return;
        }
        let resources = &self.resources;
        let layout = self.heap.layouts().objects[descriptor].clone();
        // #529: an object without its own destroy gets an identity only when a field reaches one
        // (`Runtime::created_built`); with few fields that is decided here, without the call.
        let inline = resources.leaf_capable[descriptor] && layout.len() <= 8 && !self.plain;
        let ctx = self.ctx();
        let descriptor_value = self.iconst(descriptor as i64);
        if !inline {
            self.helper(|h| h.created, &[ctx, descriptor_value, object]);
            return;
        }
        let (call, done) = (self.builder.create_block(), self.builder.create_block());
        for (index, repr) in layout.into_iter().enumerate() {
            let next = self.builder.create_block();
            let word = self.builder.ins().load(types::I64, MEMORY, object, ((index + 1) * 8) as i32);
            self.field_may_reach_identity(word, repr, call, next);
            self.builder.switch_to_block(next);
        }
        self.builder.ins().jump(done, &[]);
        self.builder.switch_to_block(call);
        self.helper(|h| h.created, &[ctx, descriptor_value, object]);
        self.builder.ins().jump(done, &[]);
        self.builder.switch_to_block(done);
    }

    /// Jump to `target` after its edge moves, then define `register` as `word`.
    fn jump_then(&mut self, target: &Target, register: okkama_sparrow::Register, word: Value) {
        if self.polled && target.pc as usize <= self.pc {
            self.poll();
        }
        let values: Vec<_> = target.moves.iter().map(|(_, source)| self.get(source)).collect();
        for ((dst, _), value) in target.moves.iter().zip(values) {
            self.set(dst, value);
        }
        self.set_word(&register, word);
        let block = self.block(target.pc as usize);
        self.builder.ins().jump(block, &[]);
    }
}

/// #552: above this many (suspensions × registers), an async body keeps its registers in its frame.
const RESIDENT_LIMIT: usize = 40_000;

/// #552: does an async body keep its registers in its frame? Only a big one: a small body is faster with machine
/// registers saved at its few suspensions. `SPARROW_CORE_PLAIN` keeps the classic path.
fn frame_resident(is_async: bool, suspensions: usize, registers: usize, plain: bool) -> bool {
    is_async && !plain && suspensions.saturating_mul(registers) > RESIDENT_LIMIT
}

pub(crate) use crate::texts::UNREACHABLE;

#[cfg(test)]
mod frame_resident_tests {
    /// The fixture of #552 (300 awaits, about 600 registers) goes to the frame; a usual async action does not,
    /// nor a synchronous one, nor a plain compile.
    #[test]
    fn only_a_big_async_body_keeps_its_registers_in_its_frame() {
        assert!(super::frame_resident(true, 300, 600, false));
        assert!(!super::frame_resident(true, 5, 60, false));
        assert!(!super::frame_resident(false, 300, 600, false));
        assert!(!super::frame_resident(true, 300, 600, true));
        assert!(!super::frame_resident(true, 200, 200, false), "exactly the limit stays classic");
    }
}

/// The parameter and field of a function whose whole body is `obj.get` of a
/// parameter and `ret` of it (#541): a synchronous Okkama function without
/// handlers, so inlining it changes nothing a program can observe.
fn accessor(function: &okkama_sparrow::Function) -> Option<(usize, u32)> {
    use okkama_sparrow::Instruction as I;
    if function.is_async || function.native.is_some() || function.intrinsic.is_some() || !function.handlers.is_empty() {
        return None;
    }
    match function.code.as_slice() {
        [I::ObjGet { dst, object, field }, I::Return { value: Some(returned) }] if dst.0 == *returned => {
            function.params.iter().position(|param| param == object).map(|param| (param, *field))
        }
        _ => None,
    }
}

/// Atomic operations compiled inline (see `Emitter::atomic`).
fn atomic_access(callee: &okkama_sparrow::Function) -> Option<Intrinsic> {
    let intrinsic = Intrinsic::resolve(&callee.intrinsic.as_ref()?.name)?;
    matches!(
        intrinsic,
        Intrinsic::AtomicValue
            | Intrinsic::AtomicReplace
            | Intrinsic::AtomicClosed
            | Intrinsic::AtomicClose
            | Intrinsic::EventReady
    )
    .then_some(intrinsic)
}

fn int_condition(op: Cmp) -> IntCC {
    match op {
        Cmp::Eq => IntCC::Equal,
        Cmp::Ne => IntCC::NotEqual,
        Cmp::Lt => IntCC::SignedLessThan,
        Cmp::Le => IntCC::SignedLessThanOrEqual,
        Cmp::Gt => IntCC::SignedGreaterThan,
        Cmp::Ge => IntCC::SignedGreaterThanOrEqual,
    }
}

#[cfg(test)]
mod compile_limit_tests {
    use super::compile_limit_text;

    /// #1066: the limit is never printed as 0 of its unit.
    #[test]
    fn the_compile_cost_limit_prints_in_a_unit_it_fills() {
        assert_eq!(compile_limit_text(256 << 20), "256 MiB");
        assert_eq!(compile_limit_text(4 << 20), "4 MiB");
        assert_eq!(compile_limit_text(1 << 20), "1 MiB");
        assert_eq!(compile_limit_text((1 << 20) - 64), "1023 KiB");
        assert_eq!(compile_limit_text(256 << 10), "256 KiB");
        assert_eq!(compile_limit_text(1024), "1 KiB");
        assert_eq!(compile_limit_text(960), "960 bytes");
        assert_eq!(compile_limit_text(0), "0 bytes");
    }
}

#[cfg(test)]
#[path = "jit_pointer_free_tests.rs"]
mod pointer_free_tests;

#[cfg(test)]
#[path = "dispatch_537_tests.rs"]
mod dispatch_537_tests;
#[cfg(test)]
#[path = "scalar_inline_tests.rs"]
mod scalar_inline_tests;

#[cfg(test)]
mod profile_tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    thread_local! {
        pub(super) static SAMPLES: Cell<usize> = const { Cell::new(0) };
        pub(super) static REPORTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    #[test]
    fn compile_profile_preserves_diagnostics_and_code_characteristics() {
        assert_eq!(COMPILE_HEADER.split('\t').collect::<Vec<_>>(), [
            "; compile", "function", "instructions", "registers", "suspensions",
            "clif blocks", "clif instructions", "safepoints", "code bytes", "frame bytes",
            "declare", "liveness", "emit", "finalize", "codegen", "maps", "definitions", "tables (microseconds)",
        ]);
        crate::on_vm_thread(|| {
            let program = okkama_sparrow::assemble(
                "sparrow 18\nmodule \"profile\" \"v1.0.0\"\ntype t0 int\n\
                 function f0 \"main\" {\n registers [t0]\n params []\n result t0\n\
                 0: const r0, int 7\n 1: ret r0\n}\nentry f0\n"
            ).unwrap();
            let mut characteristics = Vec::new();
            for enabled in [false, true] {
                SAMPLES.set(0);
                REPORTS.with(|reports| reports.borrow_mut().clear());
                let mut runtime = Box::new(Runtime::new(crate::program::Loaded::new(program.clone()))?);
                runtime.jit.profile = enabled.then(Default::default);
                assert_eq!(runtime.run_entry(crate::STACK_BYTES)?, 7);
                assert_eq!(SAMPLES.get(), usize::from(enabled));
                REPORTS.with(|reports| {
                    let reports = reports.borrow();
                    assert_eq!(reports.len(), usize::from(enabled));
                    if enabled {
                        let columns: Vec<_> = reports[0].split('\t').collect();
                        assert_eq!(columns.len(), 18);
                        // Pre-codegen CLIF includes entry/limit/failure blocks.
                        let expected = ["; compile", "main", "2", "1", "0", "3", "10", "0"];
                        assert_eq!(&columns[..8], &expected);
                        for column in &columns[8..] {
                            column.parse::<u64>().unwrap();
                        }
                    }
                });
                characteristics.push((runtime.jit.ranges[0].1 - runtime.jit.ranges[0].0,
                    runtime.jit.largest_frame, runtime.jit.maps.len()));
            }
            assert_eq!(characteristics[0], characteristics[1]);
            Ok(())
        }).unwrap();
    }
}

#[cfg(test)]
mod ready_callees_tests;
#[cfg(test)]
mod accessor_inline_tests;
#[cfg(test)]
mod frame_roots_tests;
#[cfg(test)]
mod inline_tests;
#[cfg(test)]
mod closure_call_tests;
#[cfg(test)]
mod unreachable_tests;
#[cfg(test)]
mod wide_closure_tests;
#[cfg(test)]
mod stub_field_tests;

#[cfg(test)]
mod placement_tests;

/// #1108: registering functions' maps by splicing gives what the old full sort gave, whatever the order in which
/// code addresses come, and the code ranges stay ordered for `in_code`.
#[cfg(test)]
mod register_code_tests {
    use super::register_code;

    #[test]
    fn spliced_maps_equal_a_full_sort_for_any_address_order() {
        // (address, size, return offsets in the order the compiler reports them)
        let functions: [(u64, u32, &[u32]); 6] = [
            (0x5000, 0x100, &[0x10, 0x40, 0x80]),
            (0x1000, 0x80, &[0x8, 0x20]),
            (0x9000, 0x40, &[]),
            (0x3000, 0x200, &[0x1f0, 0x30, 0x100]),
            (0x2000, 0x10, &[0x4]),
            (0x7000, 0x300, &[0x2, 0x2fe]),
        ];
        let (mut maps, mut ranges) = (Vec::new(), Vec::new());
        let (mut reference, mut reference_ranges) = (Vec::new(), Vec::new());
        for (index, (address, size, rets)) in functions.iter().enumerate() {
            let batch: Vec<(u32, Vec<u32>)> = rets.iter().map(|&ret| (ret, vec![index as u32, ret])).collect();
            for (ret, slots) in &batch {
                reference.push((address + u64::from(*ret), index as u32, slots.clone()));
            }
            reference.sort_by_key(|(at, _, _)| *at);
            reference_ranges.push((*address, address + u64::from(*size)));
            reference_ranges.sort_unstable();
            register_code(&mut maps, &mut ranges, *address, *size, index as u32, batch);
            assert_eq!(maps, reference, "after function {index}");
            assert_eq!(ranges, reference_ranges, "after function {index}");
        }
    }
}
