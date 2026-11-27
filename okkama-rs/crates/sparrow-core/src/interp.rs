//! Sparrow's interpreter (#1120; design `okkama-docs/runtime/sparrow-interpreter-1079.md`): the same runtime as
//! the JIT's — heap, collector, `ops`, failures, regions, natives, intrinsics, budgets — with another executor of
//! function bodies. `SPARROW_CORE_INTERPRET=1` chooses it when a runtime is made (`Runtime::interp`); then no
//! function is compiled and every entry from Rust goes through the seam (`Runtime::invoke`, `Runtime::resume`).
//!
//! - **Semantics are the JIT's `SPARROW_CORE_PLAIN` path without Cranelift.** An instruction the JIT runs through
//!   a shared helper (`program::generic`, `program::failure_or_region`, calls of natives, intrinsics, closures,
//!   Futures) calls the same `rt_*` function here; the rest are branches of their own, each the meaning of the
//!   emitter's code for it. The interpreter decides no ownership rule of its own.
//! - **Frames are a register stack, not the Rust stack.** A call of an Okkama body pushes an activation (its
//!   registers in a chunk of the register stack, which never moves) and goes on in the same loop; only the
//!   runtime's own calls into Okkama code (a destroy, an intrinsic's callback, `Error.text`, a constant's init)
//!   enter [`invoke`] again on the Rust stack.
//! - **Roots are every register by its declared kind** (`Kind::traced`), with neither liveness nor the
//!   pointer-free proofs: the oracle must not depend on the analyses it checks. A dead register keeps an old value
//!   alive a little longer; nothing else differs.
//! - **Dispatch is a `match` over the decoded `Instruction`**, with a plan per function made at its first call:
//!   the registers' initial words, the traced registers, and per instruction what its execution needs that the
//!   program says statically (a constant's word, the uses of a helper's instruction, a dispatch table).
//!
//! - **A failure follows the JIT's status protocol** (#1121): each activation it reaches is recorded once, and the
//!   first handler covering the instruction takes it with the activations above it left; `catch` asks
//!   `rt_catch`. Cleanups are the program's own code there (`region.unwind`, `resource.unwind`, `fail.*`), run
//!   through `rt_failure_op` as any instruction.
//!
//! - **An async body's registers are its Future's heap frame** (#1122): a suspension writes the resume point and
//!   leaves, a resumption executes the suspending instruction again, as the JIT's body does; such an activation
//!   is always the base of its entry, and an await-like instruction calls the same helper as the JIT.
//!
//! Rule for this file: a heap word is not held in a Rust local across a call that may collect, unless it is still
//! in a register (all registers are roots; the heap does not move objects). Operations follow the JIT's order: an
//! allocation first, then the reads of the registers that fill it.
#![allow(unsafe_code)]

use crate::heap::{self, Heap};
use crate::program::Kind;
use crate::runtime::{self, Ctx, FAILED, FailureKind, OK, Runtime};
use crate::word::{self, Repr, Word};
use okkama_sparrow::{Cmp, Constant, Instruction as I, Register, Target, Type};

/// Words in one chunk of the register stack; a bigger frame gets a chunk of its own.
const CHUNK: usize = 1 << 16;

/// The interpreter's state in a runtime: plans, the activations of every entry now running, the register stack.
pub(crate) struct Interp {
    plans: Vec<Option<Box<Plan>>>,
    /// Innermost last; every entry from Rust ([`invoke`]) adds its own after the ones running below it.
    activations: Vec<Activation>,
    stack: Stack,
    /// Role dispatch tables (`program::dispatch_table`), shared by the call sites of one (role, slot).
    tables: Vec<Box<[u64]>>,
    table_of: std::collections::HashMap<(u32, u32), u32>,
    /// Entries from Rust ([`invoke`], [`resume`]) now nested, and how many may nest (#1122, [`REENTRIES`]).
    depth: u32,
    reentries: u32,
}

impl std::fmt::Debug for Interp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Interp")
            .field("planned", &self.plans.iter().filter(|plan| plan.is_some()).count())
            .field("activations", &self.activations.len())
            .field("stack words", &self.stack.live)
            .finish_non_exhaustive()
    }
}

/// One active call of an Okkama body.
#[derive(Debug, Clone, Copy)]
struct Activation {
    function: u32,
    /// The instruction it is at while a callee runs (the call), or where it failed.
    pc: u32,
    regs: *mut Word,
    count: u32,
    /// The register stack's top before this frame.
    mark: Mark,
    /// #1122: the heap frame (the Future) of an async body, whose registers `regs` are; 0 for a synchronous call.
    frame: Word,
}

/// What a function's execution needs that its code says statically, made at its first call.
#[derive(Debug)]
struct Plan {
    /// The registers' words when a frame starts: unbound, a float as `UNBOUND_RAW` (the JIT's initial values).
    init: Box<[Word]>,
    /// The registers the collector reads, with their representation.
    traced: Box<[(u32, Repr)]>,
    /// Per instruction.
    prep: Box<[Prep]>,
}

/// The static part of one instruction's execution.
#[derive(Debug)]
enum Prep {
    None,
    /// `const`: its word (strings and big integers are runtime constants, roots).
    Word(Word),
    /// An instruction a shared helper runs: its uses in visit order and the register it defines.
    /// `region` when it is `rt_failure_op`'s (`program::failure_or_region`), else `rt_exec`'s.
    Uses { uses: Box<[Register]>, def: Option<Register>, region: bool },
    /// `obj.get` of a field a stub of `undef` may reach (#1076): the stub's word, and whether it is a `bool`.
    Stub { stub: Word, boolean: bool },
    /// `variant.get`: the case's object, and the stub check of its field (#1076).
    Case { object: u32, stub: Option<(Word, bool)> },
    /// `variant.new`: the case's object; `obj.next`: the declared object; `variant.tag`: the first case's object.
    Object(u32),
    /// `arr.new`: the representation argument of `rt_array`.
    Array(u64),
    /// `arr.get`, `arr.lookup` of a byte array (packed).
    Bytes,
    /// `call.role`: the dispatch table, and whether the slot makes a Future.
    Role { table: u32, asynchronous: bool },
    /// `call`: what the static callee is.
    Call(Callee),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Callee {
    /// An async function: the call makes its cold Future.
    Future,
    Native,
    Intrinsic,
    Body,
}

/// A position of the register stack's top.
#[derive(Debug, Clone, Copy)]
struct Mark {
    chunk: usize,
    top: usize,
}

/// Chunks of registers; a chunk is never reallocated, so a frame's registers stay where they are while the
/// runtime re-enters the interpreter above it.
#[derive(Debug, Default)]
struct Stack {
    chunks: Vec<Box<[Word]>>,
    chunk: usize,
    top: usize,
    /// Words of the live frames, and the most bytes they may take (the machine stack's size, #1120).
    live: u64,
    limit: u64,
}

impl Stack {
    /// Room for a frame of `words` registers, or `None` past the limit (a stack overflow).
    fn push(&mut self, words: usize) -> Option<(*mut Word, Mark)> {
        if (self.live + words as u64).saturating_mul(8) > self.limit {
            return None;
        }
        let mark = Mark { chunk: self.chunk, top: self.top };
        if let Some(chunk) = self.chunks.get_mut(self.chunk)
            && chunk.len() - self.top >= words
        {
            // SAFETY: `top + words` is within the chunk.
            let regs = unsafe { chunk.as_mut_ptr().add(self.top) };
            self.top += words;
            self.live += words as u64;
            return Some((regs, mark));
        }
        // Chunks above the current one hold no live frame: a too small one is replaced.
        let next = if self.chunks.is_empty() { 0 } else { self.chunk + 1 };
        if self.chunks.get(next).is_none_or(|chunk| chunk.len() < words) {
            self.chunks.truncate(next);
            self.chunks.push(vec![0; words.max(CHUNK)].into_boxed_slice());
        }
        self.chunk = next;
        self.top = words;
        self.live += words as u64;
        Some((self.chunks[next].as_mut_ptr(), mark))
    }

    fn pop(&mut self, mark: Mark, words: u32) {
        self.chunk = mark.chunk;
        self.top = mark.top;
        self.live -= u64::from(words);
    }
}

impl Interp {
    pub(crate) fn new(functions: usize) -> Self {
        Self {
            plans: (0..functions).map(|_| None).collect(),
            activations: Vec::new(),
            stack: Stack { limit: crate::STACK_BYTES, ..Stack::default() },
            tables: Vec::new(),
            table_of: std::collections::HashMap::new(),
            depth: 0,
            reentries: REENTRIES,
        }
    }

    /// The register stack takes at most `bytes` (a run's stack size; under a memory budget, capped by it).
    pub(crate) fn limit(&mut self, bytes: u64) {
        self.stack.limit = bytes;
    }

    /// #588: a session's extension added functions; the role tables are made again for its objects.
    pub(crate) fn extend(&mut self, functions: usize) {
        self.plans.resize_with(functions, || None);
        self.tables.clear();
        self.table_of.clear();
        for plan in self.plans.iter_mut() {
            // A plan's role sites name tables by index.
            if plan.as_ref().is_some_and(|plan| plan.prep.iter().any(|prep| matches!(prep, Prep::Role { .. }))) {
                *plan = None;
            }
        }
    }

    /// The roots of every active frame: each traced register, whatever its liveness.
    pub(crate) fn roots(&self, visit: &mut dyn FnMut(Word, Repr)) {
        for activation in &self.activations {
            let Some(plan) = &self.plans[activation.function as usize] else { continue };
            for &(register, repr) in plan.traced.iter() {
                // SAFETY: a live frame of `count` registers; traced indices are below it.
                visit(unsafe { *activation.regs.add(register as usize) }, repr);
            }
        }
    }

    /// Leave every activation above `base`.
    fn pop_to(&mut self, base: usize) {
        while self.activations.len() > base {
            let activation = self.activations.pop().expect("an activation");
            self.stack.pop(activation.mark, activation.count);
        }
    }
}

/// The runtime of `ctx` and its interpreter.
///
/// SAFETY: `ctx` is a live context whose runtime has an interpreter; the references are used only between calls
/// that reach the runtime through `ctx` themselves (helpers, natives), as machine code uses it.
unsafe fn parts<'a>(ctx: *mut Ctx) -> (&'a mut Runtime, *mut Interp) {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let interp: *mut Interp = &mut **runtime.interp.as_mut().expect("the interpreter");
    (runtime, interp)
}

/// Run synchronous `function` over `words` (the seam's interpreter branch): its result, with a failure or a stop
/// in `ctx.status` and the trace of the activations it left recorded.
pub(crate) fn invoke(ctx: *mut Ctx, function: usize, words: &[Word]) -> Word {
    okkama_devtrace::count!("interp", "entries from Rust");
    if deep(ctx) {
        return runtime::rt_stack_overflow(ctx);
    }
    let (_, interp) = unsafe { parts(ctx) };
    unsafe { (*interp).depth += 1 };
    let result = invoke_entered(ctx, function, words);
    unsafe { (*interp).depth -= 1 };
    result
}

/// The most entries from Rust that nest at once (#1122): each takes Rust stack (the loop, the helpers between),
/// whose overflow on wasm32 is a trap, not a failure; natively the stack limit decides (`deep`).
const REENTRIES: u32 = if cfg!(target_arch = "wasm32") { 200 } else { u32::MAX };

/// Is the Rust stack too deep for one more entry: below the context's stack limit, as the JIT's prologue checks
/// its SP, or past `REENTRIES` nested entries?
fn deep(ctx: *mut Ctx) -> bool {
    let marker = 0u8;
    if (&marker as *const u8 as u64) < unsafe { (*ctx).stack_limit } {
        return true;
    }
    let (_, interp) = unsafe { parts(ctx) };
    unsafe { (*interp).depth >= (*interp).reentries }
}

fn invoke_entered(ctx: *mut Ctx, function: usize, words: &[Word]) -> Word {
    let (runtime, interp) = unsafe { parts(ctx) };
    let source = &runtime.loaded.program.functions[function];
    if source.native.is_some() {
        return runtime::rt_native(ctx, function as u64, words.as_ptr(), words.len() as u64);
    }
    if source.intrinsic.is_some() {
        return crate::intrinsics::rt_intrinsic(ctx, function as u64, words.as_ptr(), words.len() as u64);
    }
    if source.is_async {
        // Callers make an async function's Future (`Runtime::future_new`) and run it through `resume`.
        let message = format!("{}: an async function is run as a Future, not called", source.symbol);
        return runtime::fatal(unsafe { &mut *ctx }, runtime, message);
    }
    let base = unsafe { (*interp).activations.len() };
    match enter(ctx, interp, function, words) {
        Entered::Frame => {}
        Entered::Stopped => {
            unsafe { (*interp).pop_to(base) };
            return 0;
        }
    }
    run(ctx, interp, base, 0)
}

/// Start or resume the async body of `frame` (#1122): its registers are the frame's (`tasks::REGISTERS`), so a
/// suspension writes the resume point and returns, and a resumption executes again the instruction that
/// suspended, as the JIT's body does. Every entry is a step. True while the body is pending.
pub(crate) fn resume(ctx: *mut Ctx, function: usize, frame: Word) -> bool {
    okkama_devtrace::count!("interp", "async bodies resumed");
    if deep(ctx) {
        runtime::rt_stack_overflow(ctx);
        return false;
    }
    let (runtime, interp) = unsafe { parts(ctx) };
    if let Err(message) = plan(runtime, interp, function) {
        runtime::fatal(unsafe { &mut *ctx }, runtime, message);
        return false;
    }
    let interp_ref = unsafe { &mut *interp };
    let base = interp_ref.activations.len();
    let mark = Mark { chunk: interp_ref.stack.chunk, top: interp_ref.stack.top };
    let regs = (frame + crate::tasks::REGISTERS) as *mut Word;
    interp_ref.activations.push(Activation { function: function as u32, pc: 0, regs, count: 0, mark, frame });
    if step(ctx, runtime) {
        interp_ref.pop_to(base);
        return false;
    }
    let start = match crate::tasks::get(frame, crate::tasks::RESUME) {
        0 => 0,
        resume => resume as usize - 1,
    };
    interp_ref.depth += 1;
    let pending = run(ctx, interp, base, start) != 0;
    unsafe { (*interp).depth -= 1 };
    pending
}

enum Entered {
    /// A new activation is on top; the loop goes on in it.
    Frame,
    /// Its prologue stopped the run or failed (status set); nothing was pushed, or the push is undone by the
    /// caller's unwinding.
    Stopped,
}

/// Push an activation of body `function` with `words` as its parameters: the stack check, then the entry's step
/// (the JIT's prologue order). A stack overflow is a failure of the caller's instruction, so the callee is not in
/// the trace.
fn enter(ctx: *mut Ctx, interp: *mut Interp, function: usize, words: &[Word]) -> Entered {
    let (runtime, _) = unsafe { parts(ctx) };
    let plan = match plan(runtime, interp, function) {
        Ok(plan) => plan,
        Err(message) => {
            runtime::fatal(unsafe { &mut *ctx }, runtime, message);
            return Entered::Stopped;
        }
    };
    let interp = unsafe { &mut *interp };
    let count = plan.init.len();
    let Some((regs, mark)) = interp.stack.push(count) else {
        runtime::rt_stack_overflow(ctx);
        return Entered::Stopped;
    };
    // SAFETY: `count` fresh registers; the verifier gives every parameter a register below `count`.
    unsafe {
        std::ptr::copy_nonoverlapping(plan.init.as_ptr(), regs, count);
        let params = &runtime.loaded.program.functions[function].params;
        for (param, &word) in params.iter().zip(words) {
            *regs.add(param.0 as usize) = word;
        }
    }
    interp.activations.push(Activation { function: function as u32, pc: 0, regs, count: count as u32, mark, frame: 0 });
    if step(ctx, runtime) {
        return Entered::Stopped;
    }
    Entered::Frame
}

/// One budget step, when a budget is set: true when the run must stop.
#[inline(always)]
fn step(ctx: *mut Ctx, runtime: &Runtime) -> bool {
    if !runtime.polled() || !runtime.budget.fuel.step() {
        return false;
    }
    runtime::rt_poll(ctx);
    unsafe { (*ctx).status != OK }
}

/// The plan of `function`, made at its first call.
fn plan<'a>(runtime: &mut Runtime, interp: *mut Interp, function: usize) -> Result<&'a Plan, String> {
    // SAFETY: plans are boxed and kept while the runtime lives (an extension replaces only role plans, between
    // runs), so the reference outlives the loop that uses it.
    let interp = unsafe { &mut *interp };
    if let Some(plan) = &interp.plans[function] {
        return Ok(unsafe { &*(&**plan as *const Plan) });
    }
    let made = make_plan(runtime, interp, function)?;
    okkama_devtrace::count!("interp", "functions planned");
    let slot = &mut interp.plans[function];
    let plan: &Plan = slot.insert(Box::new(made));
    Ok(unsafe { &*(plan as *const Plan) })
}

fn make_plan(runtime: &mut Runtime, interp: &mut Interp, function: usize) -> Result<Plan, String> {
    let Runtime { loaded, heap, constants, .. } = runtime;
    let program = &loaded.program;
    let source = &program.functions[function];
    let kinds = &loaded.functions[function].kinds;
    let init = kinds.iter().map(|kind| if *kind == Kind::Float { word::UNBOUND_RAW } else { word::UNBOUND }).collect();
    let traced = kinds.iter().enumerate().filter(|(_, kind)| kind.traced()).map(|(register, kind)| (register as u32, kind.repr())).collect();
    let ty = |register: &Register| program.ty(source.registers[register.0 as usize]);
    let stub_of = |object: usize, field: u32| -> Option<(Word, bool)> {
        if !loaded.may_hold_stub(object, field as usize) {
            return None;
        }
        let declared = program.objects[object].fields[field as usize].1;
        Some((word::unbound(Kind::of(program, declared).repr()), matches!(program.ty(declared), Some(Type::Bool))))
    };
    let bytes = |register: &Register| matches!(ty(register), Some(Type::Array(element)) if Kind::of(program, *element) == Kind::Byte);
    let unsupported = |pc: usize, instruction: &I| format!("{}:{pc}: {} is not supported by sparrow-core yet", source.symbol, instruction.mnemonic());
    let mut prep = Vec::with_capacity(source.code.len());
    for (pc, instruction) in source.code.iter().enumerate() {
        prep.push(match instruction {
            I::Const { value, .. } => Prep::Word(match value {
                Constant::Int(text) => {
                    let n: num_bigint::BigInt = text.parse().map_err(|_| "invalid integer constant".to_string())?;
                    let word = heap.big(n);
                    if word & 1 == 0 {
                        constants.push((word, Repr::Int));
                    }
                    word
                }
                Constant::Bool(b) => Word::from(*b),
                Constant::Byte(b) => Word::from(*b),
                Constant::Char(c) => Word::from(u32::from(*c)),
                Constant::Float(bits) => *bits,
                Constant::Str(text) => {
                    let word = heap.allocate_text(text);
                    constants.push((word, Repr::Ref));
                    word
                }
            }),
            other if crate::program::generic(other) || crate::program::failure_or_region(other) => {
                let (mut uses, mut def) = (Vec::new(), None);
                other.visit(&mut |access| match access {
                    okkama_sparrow::Access::Use(register) => uses.push(register),
                    okkama_sparrow::Access::Def(register) => def = Some(register),
                    _ => {}
                });
                Prep::Uses { uses: uses.into_boxed_slice(), def, region: crate::program::failure_or_region(other) }
            }
            I::ObjGet { object, field, .. } => match ty(object) {
                Some(Type::Object(declared)) => match stub_of(declared.0 as usize, *field) {
                    Some((stub, boolean)) => Prep::Stub { stub, boolean },
                    None => Prep::None,
                },
                _ => Prep::None,
            },
            I::VariantGet { value, case, field, .. } => match ty(value) {
                Some(Type::Variant(v)) => {
                    let object = program.variants[v.0 as usize].cases[*case as usize].1.0;
                    Prep::Case { object, stub: stub_of(object as usize, *field) }
                }
                _ => return Err(unsupported(pc, instruction)),
            },
            I::VariantNew { variant, case, .. } => Prep::Object(program.variants[variant.0 as usize].cases[*case as usize].1.0),
            I::VariantTag { value, .. } => match ty(value) {
                Some(Type::Variant(v)) => Prep::Object(program.variants[v.0 as usize].cases[0].1.0),
                _ => return Err(unsupported(pc, instruction)),
            },
            I::ObjNext { object, .. } => match ty(object) {
                Some(Type::Object(o) | Type::Successor(o)) => Prep::Object(o.0),
                _ => return Err(unsupported(pc, instruction)),
            },
            I::ArrayNew { dst, .. } => match ty(&dst.0) {
                Some(Type::Array(element)) if Kind::of(program, *element) == Kind::Byte => Prep::Array(runtime::ARRAY_BYTES),
                Some(Type::Array(element)) => Prep::Array(Kind::of(program, *element).repr() as u64),
                _ => return Err(unsupported(pc, instruction)),
            },
            I::ArrayGet { array, .. } | I::ArrayLookup { array, .. } if bytes(array) => Prep::Bytes,
            I::CallRole { role, slot, .. } => {
                let key = (role.0, *slot);
                let table = match interp.table_of.get(&key) {
                    Some(&table) => table,
                    None => {
                        interp.tables.push(crate::program::dispatch_table(program, role.0, *slot, program.objects.len()));
                        let table = (interp.tables.len() - 1) as u32;
                        interp.table_of.insert(key, table);
                        table
                    }
                };
                Prep::Role { table, asynchronous: crate::program::role_slot_is_async(program, *role, *slot) }
            }
            I::Call { function, .. } => {
                let callee = &program.functions[function.0 as usize];
                // FutureCompletions.completion reads its origins at the call (as the JIT does).
                let eager = callee.intrinsic.as_ref().is_some_and(|i| i.name == "okkama.collect.FutureCompletions.completion");
                Prep::Call(if callee.is_async && !eager {
                    Callee::Future
                } else if callee.native.is_some() {
                    Callee::Native
                } else if callee.intrinsic.is_some() {
                    Callee::Intrinsic
                } else {
                    Callee::Body
                })
            }
            _ => Prep::None,
        });
    }
    Ok(Plan { init, traced, prep: prep.into_boxed_slice() })
}

/// The words of `registers` in `short`, or in `long` past its length.
#[inline(always)]
fn gather<'a>(regs: *const Word, registers: &[Register], short: &'a mut [Word; 16], long: &'a mut Vec<Word>) -> &'a [Word] {
    // SAFETY: verified register indices of the current frame.
    let read = |register: &Register| unsafe { *regs.add(register.0 as usize) };
    if registers.len() <= short.len() {
        for (slot, register) in short.iter_mut().zip(registers) {
            *slot = read(register);
        }
        &short[..registers.len()]
    } else {
        long.clear();
        long.extend(registers.iter().map(read));
        long
    }
}

/// The comparison `op` of an order.
#[inline(always)]
fn holds(op: Cmp, order: std::cmp::Ordering) -> bool {
    use std::cmp::Ordering::{Equal, Greater, Less};
    match op {
        Cmp::Eq => order == Equal,
        Cmp::Ne => order != Equal,
        Cmp::Lt => order == Less,
        Cmp::Le => order != Greater,
        Cmp::Gt => order == Greater,
        Cmp::Ge => order != Less,
    }
}

/// Two inline integers' exact operation, when its result is inline too; `None` takes the helper (`rt_int`), as
/// the JIT's overflow and divisor checks do.
#[inline(always)]
fn small_arithmetic(op: u64, left: Word, right: Word) -> Option<Word> {
    if left & right & 1 == 0 {
        return None;
    }
    let (a, b) = ((left as i64) >> 1, (right as i64) >> 1);
    match op {
        runtime::INT_ADD => word::small(a + b),
        runtime::INT_SUB => word::small(a - b),
        runtime::INT_MUL => a.checked_mul(b).and_then(word::small),
        // Divisors 0 and -1 go to the exact path.
        _ if b == 0 || b == -1 => None,
        _ => word::small(a % b),
    }
}

/// Element `position` (in bounds) of `array`: a byte of a packed array, a flat or shared word, or a trie's.
#[inline(always)]
fn element(ctx: *mut Ctx, array: Word, header: u64, position: u64, bytes: bool) -> Word {
    // SAFETY: a live array with `position` inside it; the layouts are the heap's (see `jit::Emitter::element`).
    unsafe {
        if bytes {
            return Word::from(*((array + 8 + position) as *const u8));
        }
        if header & (heap::SHARED | heap::TRIE) == 0 {
            *((array + 8 + position * 8) as *const Word)
        } else if header & heap::SHARED != 0 {
            *((*((array + 8) as *const Word) + position * 8) as *const Word)
        } else {
            crate::ops::rt_trie_get(ctx, array, position)
        }
    }
}

/// A new cell of `words` words: from its class's free list, else through `rt_allocate` (a collection when due,
/// the memory budget's refusal as 0), as the JIT's inline allocation.
#[inline(always)]
fn allocate(ctx: *mut Ctx, runtime: &mut Runtime, words: usize) -> Word {
    if let Some(class) = Heap::class_of(words) {
        let head = runtime.heap.free_head(class);
        // SAFETY: the free list's head and its cells' second words are the heap's (see `Heap::allocate_class`).
        unsafe {
            let cell = *head;
            if cell != 0 {
                *head = *((cell + 8) as *const Word);
                return cell;
            }
        }
    }
    runtime::rt_allocate(ctx, words as u64)
}

/// A new object's resource identity, when its type can reach a destroy (the JIT's plain path).
#[inline(always)]
fn created(ctx: *mut Ctx, runtime: &Runtime, descriptor: u32, object: Word) {
    if runtime.resources.enabled && runtime.resources.flags[descriptor as usize] {
        crate::failures::rt_created(ctx, u64::from(descriptor), object);
    }
}

/// A program failure with `message` (what `rt_trap` makes).
#[cold]
#[inline(never)]
fn trap(ctx: *mut Ctx, message: &str) {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let failure = runtime.failure_new(FailureKind::Program(message.to_owned()));
    unsafe {
        (*ctx).failure = failure;
        (*ctx).status = FAILED;
    }
}

/// The failure or stop of the innermost activation at `pc`, as the JIT's `check` after each instruction that may
/// fail: each activation the failure reaches is recorded (once, at the instruction it is at), then the first
/// handler covering that instruction takes it — the activations above it are left, the status is OK again, the
/// handler's edge is taken (a step when it goes back) and its register gets the failure. True when a handler
/// caught it: its activation is innermost, at the handler's target. Otherwise (no handler, or a stop: FATAL and
/// EXHAUSTED pass every handler) every activation above `base` is left.
#[cold]
#[inline(never)]
fn unwind(ctx: *mut Ctx, interp: *mut Interp, base: usize, pc: usize) -> bool {
    let (runtime, _) = unsafe { parts(ctx) };
    let interp = unsafe { &mut *interp };
    if unsafe { (*ctx).status } == FAILED {
        let top = interp.activations.len() - 1;
        interp.activations[top].pc = pc as u32;
        for index in (base..=top).rev() {
            let Activation { function, pc, regs, .. } = interp.activations[index];
            runtime::rt_trace(ctx, u64::from(function), u64::from(pc), regs as u64);
            let source: *const okkama_sparrow::Function = &runtime.loaded.program.functions[function as usize];
            // SAFETY: the code and handlers are never mutated while they run (as for `rt_exec`).
            let source = unsafe { &*source };
            let Some(handler) = source.handlers.iter().find(|h| (h.start..h.end).contains(&pc)) else { continue };
            okkama_devtrace::count!("interp", "failures caught");
            interp.pop_to(index + 1);
            unsafe { (*ctx).status = OK };
            let target = &handler.target;
            if target.pc <= pc && step(ctx, runtime) {
                break;
            }
            // The edge's moves read the old registers; the failure is written last.
            let read = |register: &Register| unsafe { *regs.add(register.0 as usize) };
            let moved: Vec<Word> = target.moves.iter().map(|(_, source)| read(source)).collect();
            for ((register, _), value) in target.moves.iter().zip(moved) {
                unsafe { *regs.add(register.0 as usize) = value };
            }
            unsafe { *regs.add(handler.failure.0 as usize) = (*ctx).failure };
            interp.activations[index].pc = target.pc;
            return true;
        }
    }
    interp.pop_to(base);
    false
}

/// The loop: run the activation on top until the one at `base` returns (its result) or the run fails or stops
/// (0, the status set, every activation above `base` left).
///
/// `ctx` and `interp` as for [`parts`]; the activation at `base` was just entered.
fn run(ctx: *mut Ctx, interp: *mut Interp, base: usize, start: usize) -> Word {
    let runtime: *mut Runtime = unsafe { (*ctx).runtime };
    macro_rules! rt {
        () => {
            unsafe { &mut *runtime }
        };
    }
    macro_rules! it {
        () => {
            unsafe { &mut *interp }
        };
    }
    let mut short: [Word; 16] = [0; 16];
    let mut long = Vec::new();
    let mut moved = Vec::new();
    // The current activation, held in locals.
    let mut function: usize;
    let mut code: &[I];
    let mut regs: *mut Word;
    let mut prep: &[Prep];
    let mut pc: usize = start;
    // The heap frame of the activation when it is an async body's (#1122).
    let mut this: Word;
    macro_rules! load {
        () => {{
            let activation = *it!().activations.last().expect("an activation");
            function = activation.function as usize;
            // SAFETY: the code is never mutated while it runs (as for `rt_exec`), and plans outlive the loop.
            let source: *const [I] = rt!().loaded.program.functions[function].code.as_slice();
            code = unsafe { &*source };
            let planned: *const [Prep] = it!().plans[function].as_ref().expect("a plan").prep.as_ref();
            prep = unsafe { &*planned };
            regs = activation.regs;
            this = activation.frame;
        }};
    }
    load!();
    macro_rules! get {
        ($register:expr) => {
            unsafe { *regs.add($register.0 as usize) }
        };
    }
    macro_rules! set {
        ($register:expr, $value:expr) => {{
            let value = $value;
            unsafe { *regs.add($register.0 as usize) = value };
        }};
    }
    // The failure or stop of this instruction: on at a handler that caught it, or out of this entry.
    macro_rules! fail {
        () => {{
            if unwind(ctx, interp, base, pc) {
                load!();
                pc = it!().activations.last().expect("the handler's activation").pc as usize;
                continue;
            }
            return 0;
        }};
    }
    // After anything that may fail or stop.
    macro_rules! check {
        () => {
            if unsafe { (*ctx).status } != OK {
                fail!();
            }
        };
    }
    // After an await-like helper: pending leaves the async body (the base) to resume at this instruction; ready
    // defines `$dst` from `scratch`.
    macro_rules! suspend_unless_ready {
        ($outcome:expr, $dst:expr) => {{
            if $outcome == crate::tasks::AWAIT_PENDING {
                crate::tasks::set(this, crate::tasks::RESUME, pc as u64 + 1);
                it!().pop_to(base);
                return 1;
            }
            if let Some(dst) = $dst {
                set!(dst.0, unsafe { (*ctx).scratch });
            }
        }};
    }
    // Take an edge, then define `$register` as `$word` (a found element, a caught payload).
    macro_rules! jump_then {
        ($target:expr, $register:expr, $word:expr) => {{
            let target: &Target = $target;
            let word: Word = $word;
            if (target.pc as usize) <= pc && step(ctx, rt!()) {
                fail!();
            }
            moved.clear();
            moved.extend(target.moves.iter().map(|(_, source)| get!(source)));
            for ((register, _), &value) in target.moves.iter().zip(&moved) {
                set!(register, value);
            }
            set!($register, word);
            pc = target.pc as usize;
            continue;
        }};
    }
    // Take an edge: a back-edge is a step; the moves read every source before they write.
    macro_rules! jump {
        ($target:expr) => {{
            let target: &Target = $target;
            if (target.pc as usize) <= pc && step(ctx, rt!()) {
                fail!();
            }
            match target.moves.as_slice() {
                [] => {}
                [(dst, source)] => set!(dst, get!(source)),
                moves => {
                    moved.clear();
                    moved.extend(moves.iter().map(|(_, source)| get!(source)));
                    for ((dst, _), &value) in moves.iter().zip(&moved) {
                        set!(dst, value);
                    }
                }
            }
            pc = target.pc as usize;
            continue;
        }};
    }
    // Call body `callee` with `$words` (gathered): a new activation, or the failure of this instruction.
    macro_rules! call_body {
        ($callee:expr, $words:expr) => {{
            it!().activations.last_mut().expect("an activation").pc = pc as u32;
            match enter(ctx, interp, $callee, $words) {
                Entered::Frame => {
                    load!();
                    pc = 0;
                    continue;
                }
                // A failed prologue (a stack overflow) is this instruction's failure: the callee is not in the
                // trace. An entry's step that stopped the run leaves every activation, the callee's too.
                Entered::Stopped => fail!(),
            }
        }};
    }
    // A dynamic call of `callee` (role, closure, cap): a native or an intrinsic through its helper, a body in a new
    // activation; the result, when there is one at once, goes to `$dst`.
    macro_rules! call_dynamic {
        ($callee:expr, $words:expr, $dst:expr) => {{
            let callee: usize = $callee;
            let source = &rt!().loaded.program.functions[callee];
            if source.native.is_some() || source.intrinsic.is_some() {
                let words: &[Word] = $words;
                let result = if source.native.is_some() {
                    runtime::rt_native(ctx, callee as u64, words.as_ptr(), words.len() as u64)
                } else {
                    crate::intrinsics::rt_intrinsic(ctx, callee as u64, words.as_ptr(), words.len() as u64)
                };
                check!();
                if let Some(dst) = $dst {
                    set!(dst.0, result);
                }
                pc += 1;
                continue;
            }
            if source.is_async {
                // The JIT cannot compile an async native or intrinsic either.
                let message = format!("{}: async natives and intrinsics are driven, not compiled", source.symbol);
                runtime::fatal(unsafe { &mut *ctx }, rt!(), message);
                fail!();
            }
            call_body!(callee, $words)
        }};
    }
    loop {
        let instruction = &code[pc];
        #[cfg(test)]
        tests::executed(instruction.mnemonic());
        match instruction {
            I::Const { dst, .. } => {
                let Prep::Word(word) = prep[pc] else { unreachable!("a const has its word") };
                set!(dst.0, word);
            }
            I::Move { dst, source } => set!(dst.0, get!(source)),
            I::IntAdd { dst, left, right }
            | I::IntSub { dst, left, right }
            | I::IntMul { dst, left, right }
            | I::IntMod { dst, left, right } => {
                let op = match instruction {
                    I::IntAdd { .. } => runtime::INT_ADD,
                    I::IntSub { .. } => runtime::INT_SUB,
                    I::IntMul { .. } => runtime::INT_MUL,
                    _ => runtime::INT_MOD,
                };
                let (l, r) = (get!(left), get!(right));
                let value = match small_arithmetic(op, l, r) {
                    Some(value) => value,
                    None => {
                        let value = runtime::rt_int(ctx, op, l, r);
                        check!();
                        value
                    }
                };
                set!(dst.0, value);
            }
            I::IntCmp { op, dst, left, right } => {
                let (l, r) = (get!(left), get!(right));
                let order = if l & r & 1 == 1 {
                    // Tagging keeps the order of inline integers.
                    (l as i64).cmp(&(r as i64))
                } else {
                    runtime::rt_int_order(ctx, l, r).cmp(&0)
                };
                set!(dst.0, Word::from(holds(*op, order)));
            }
            I::BoolNot { dst, operand } => set!(dst.0, get!(operand) ^ 1),
            I::BoolCmp { op, dst, left, right } | I::ByteCmp { op, dst, left, right } | I::CharCmp { op, dst, left, right } => {
                let order = (get!(left) as i64).cmp(&(get!(right) as i64));
                set!(dst.0, Word::from(holds(*op, order)));
            }
            I::FloatAdd { dst, left, right }
            | I::FloatSub { dst, left, right }
            | I::FloatMul { dst, left, right }
            | I::FloatDiv { dst, left, right } => {
                let (lw, rw) = (get!(left), get!(right));
                let (l, r) = (f64::from_bits(lw), f64::from_bits(rw));
                let (value, op) = match instruction {
                    I::FloatAdd { .. } => (l + r, crate::scalar::FLOAT_ADD),
                    I::FloatSub { .. } => (l - r, crate::scalar::FLOAT_SUB),
                    I::FloatMul { .. } => (l * r, crate::scalar::FLOAT_MUL),
                    _ => (l / r, crate::scalar::FLOAT_DIV),
                };
                let bits = if value.abs() <= f64::MAX {
                    value.to_bits()
                } else {
                    let bits = runtime::rt_float(ctx, op, lw, rw);
                    check!();
                    bits
                };
                set!(dst.0, bits);
            }
            I::FloatNeg { dst, operand } => set!(dst.0, (-f64::from_bits(get!(operand))).to_bits()),
            I::FloatAbs { dst, operand } => set!(dst.0, f64::from_bits(get!(operand)).abs().to_bits()),
            I::FloatCmp { op, dst, left, right } => {
                let (l, r) = (f64::from_bits(get!(left)), f64::from_bits(get!(right)));
                let flag = match op {
                    Cmp::Eq => l == r,
                    Cmp::Ne => l != r,
                    Cmp::Lt => l < r,
                    Cmp::Le => l <= r,
                    Cmp::Gt => l > r,
                    Cmp::Ge => l >= r,
                };
                set!(dst.0, Word::from(flag));
            }
            I::StrConcat { dst, left, right } => {
                let text = runtime::rt_concat(ctx, get!(left), get!(right));
                check!();
                set!(dst.0, text);
            }
            I::ArrayNew { dst, elements } => {
                let Prep::Array(repr) = prep[pc] else { unreachable!("arr.new has its representation") };
                let words = gather(regs, elements, &mut short, &mut long);
                let array = runtime::rt_array(ctx, repr, words.as_ptr(), words.len() as u64);
                check!();
                set!(dst.0, array);
            }
            I::ArrayLen { dst, array } => {
                let header = rt!().heap.header_of(get!(array));
                set!(dst.0, ((header >> 32) << 1) | 1);
            }
            I::ArrayGet { dst, array, index } | I::ArrayLookup { array, index, value: dst, .. } => {
                let (array, index) = (get!(array), get!(index));
                let header = rt!().heap.header_of(array);
                let position = (index as i64 >> 1) as u64;
                // Unsigned: a negative position is outside too.
                let inside = index & 1 == 1 && position < header >> 32;
                if let I::ArrayLookup { found, missing, .. } = instruction {
                    if !inside {
                        jump!(missing);
                    }
                    // The edge's moves read the old registers; the element is written last.
                    jump_then!(found, dst.0, element(ctx, array, header, position, matches!(prep[pc], Prep::Bytes)));
                }
                if !inside {
                    trap(ctx, crate::texts::ARRAY_INDEX);
                    fail!();
                }
                set!(dst.0, element(ctx, array, header, position, matches!(prep[pc], Prep::Bytes)));
            }
            I::ObjNew { dst, object, fields } => {
                let words = rt!().heap.object_words(object.0, fields.len());
                let pointer = allocate(ctx, rt!(), words);
                if pointer == 0 {
                    fail!();
                }
                // A fresh cell of `words` words; the fields are read after the allocation.
                store(pointer, heap::header(heap::OBJECT, Repr::Raw, object.0));
                for (i, field) in fields.iter().enumerate() {
                    store(pointer + 8 + i as u64 * 8, get!(field));
                }
                set!(dst.0, pointer);
                created(ctx, rt!(), object.0, pointer);
            }
            I::VariantNew { dst, fields, .. } => {
                let Prep::Object(object) = prep[pc] else { unreachable!("variant.new has its case") };
                let words = rt!().heap.object_words(object, fields.len());
                let pointer = allocate(ctx, rt!(), words);
                if pointer == 0 {
                    fail!();
                }
                store(pointer, heap::header(heap::OBJECT, Repr::Raw, object));
                for (i, field) in fields.iter().enumerate() {
                    store(pointer + 8 + i as u64 * 8, get!(field));
                }
                set!(dst.0, pointer);
                created(ctx, rt!(), object, pointer);
            }
            I::ObjGet { dst, object, field } => {
                let value = load(get!(object) + 8 + u64::from(*field) * 8);
                let value = match prep[pc] {
                    Prep::Stub { stub, boolean } if value == stub => {
                        if !boolean {
                            stub_read(ctx, function, pc);
                            fail!();
                        }
                        1
                    }
                    _ => value,
                };
                set!(dst.0, value);
            }
            I::ObjInit { object, field, value } => {
                store(get!(object) + 8 + u64::from(*field) * 8, get!(value));
            }
            I::ObjNext { dst, object, updates } => {
                let Prep::Object(declared) = prep[pc] else { unreachable!("obj.next has its object") };
                let count = rt!().loaded.program.objects[declared as usize].fields.len();
                let words = rt!().heap.object_words(declared, count);
                let pointer = allocate(ctx, rt!(), words);
                if pointer == 0 {
                    fail!();
                }
                let source = get!(object);
                // The header and fields of the source, into a fresh cell of the same layout.
                for word_index in 0..=count {
                    let value = match updates.iter().find(|(f, _)| *f as usize + 1 == word_index) {
                        Some((_, register)) => get!(register),
                        None => load(source + word_index as u64 * 8),
                    };
                    store(pointer + word_index as u64 * 8, value);
                }
                set!(dst.0, pointer);
                created(ctx, rt!(), declared, pointer);
            }
            I::RoleWrap { dst, source, .. } | I::VariantOf { dst, source, .. } => set!(dst.0, get!(source)),
            I::VariantTag { dst, value } => {
                let Prep::Object(first) = prep[pc] else { unreachable!("variant.tag has its first case") };
                let descriptor = u64::from(heap::payload(rt!().heap.header_of(get!(value))));
                set!(dst.0, (descriptor.wrapping_sub(u64::from(first)) << 1) | 1);
            }
            I::VariantGet { dst, value, field, .. } => {
                let Prep::Case { object, stub } = prep[pc] else { unreachable!("variant.get has its case") };
                let pointer = get!(value);
                if heap::payload(rt!().heap.header_of(pointer)) != object {
                    trap(ctx, crate::texts::INACTIVE_CASE);
                    fail!();
                }
                let read = load(pointer + 8 + u64::from(*field) * 8);
                let read = match stub {
                    Some((stub, boolean)) if read == stub => {
                        if !boolean {
                            stub_read(ctx, function, pc);
                            fail!();
                        }
                        1
                    }
                    _ => read,
                };
                set!(dst.0, read);
            }
            I::Call { dst, function: callee, args } => {
                let Prep::Call(kind) = prep[pc] else { unreachable!("a call has its callee") };
                let callee = callee.0 as usize;
                let words = gather(regs, args, &mut short, &mut long);
                let result = match kind {
                    Callee::Body => call_body!(callee, words),
                    Callee::Future => crate::tasks::rt_future_new(ctx, callee as u64, words.as_ptr(), words.len() as u64),
                    Callee::Native => runtime::rt_native(ctx, callee as u64, words.as_ptr(), words.len() as u64),
                    Callee::Intrinsic => crate::intrinsics::rt_intrinsic(ctx, callee as u64, words.as_ptr(), words.len() as u64),
                };
                check!();
                if let Some(dst) = dst {
                    set!(dst.0, result);
                }
            }
            I::CallRole { dst, receiver, args, .. } => {
                let Prep::Role { table, asynchronous } = prep[pc] else { unreachable!("call.role has its table") };
                let object = get!(receiver);
                let descriptor = heap::payload(rt!().heap.header_of(object)) as usize;
                let callee = it!().tables[table as usize][descriptor];
                if callee == u64::MAX {
                    runtime::fatal(unsafe { &mut *ctx }, rt!(), format!("call.role: no implementation for descriptor {descriptor}"));
                    fail!();
                }
                // The receiver, then the arguments.
                let words: &[Word] = if args.len() < short.len() {
                    short[0] = object;
                    for (slot, arg) in short[1..].iter_mut().zip(args) {
                        *slot = get!(arg);
                    }
                    &short[..=args.len()]
                } else {
                    long.clear();
                    long.push(object);
                    long.extend(args.iter().map(|arg| get!(arg)));
                    &long
                };
                if asynchronous {
                    // Async witnesses: the call makes a cold Future of the method.
                    let future = crate::tasks::rt_future_new(ctx, callee, words.as_ptr(), words.len() as u64);
                    check!();
                    if let Some(dst) = dst {
                        set!(dst.0, future);
                    }
                } else {
                    call_dynamic!(callee as usize, words, *dst);
                }
            }
            I::CallCap { dst, cap, ty, field } => {
                let Some(&(_, callee)) = rt!().loaded.program.caps[ty.0 as usize].fields.get(*field as usize) else {
                    runtime::fatal(unsafe { &mut *ctx }, rt!(), format!("call.cap: no field {field}"));
                    fail!();
                };
                let receiver = [get!(cap)];
                if rt!().loaded.program.functions[callee.0 as usize].is_async {
                    let future = crate::tasks::rt_future_new(ctx, u64::from(callee.0), receiver.as_ptr(), 1);
                    check!();
                    set!(dst.0, future);
                } else {
                    call_dynamic!(callee.0 as usize, &receiver, Some(*dst));
                }
            }
            I::Closure { dst, function: callee, captures } => {
                let words = gather(regs, captures, &mut short, &mut long);
                let closure = runtime::rt_closure(ctx, u64::from(callee.0), words.as_ptr(), words.len() as u64);
                check!();
                set!(dst.0, closure);
            }
            I::CallFn { dst, callee, args } => {
                // `rt_call_closure`'s rule: the captures fill the first parameters; an async function makes its
                // cold Future.
                let closure = get!(callee);
                let target = heap::payload(rt!().heap.header_of(closure)) as usize;
                let Some(mut entry) = rt!().invocations.get(target).copied() else {
                    runtime::fatal(unsafe { &mut *ctx }, rt!(), "invalid closure function".into());
                    fail!();
                };
                if rt!().closure_bridge_reference {
                    let source = &rt!().loaded.program.functions[target];
                    entry.params = source.params.len() as u32;
                    entry.is_async = source.is_async;
                }
                let Some(captured) = (entry.params as usize).checked_sub(args.len()) else {
                    runtime::fatal(unsafe { &mut *ctx }, rt!(), "closure argument count exceeds parameter count".into());
                    fail!();
                };
                long.clear();
                long.extend((0..captured).map(|i| rt!().heap.field(closure, i)));
                long.extend(args.iter().map(|arg| get!(arg)));
                if entry.is_async {
                    // Calling an async action makes its cold Future.
                    let future = rt!().future_new(target, &long);
                    if let Some(dst) = dst {
                        set!(dst.0, future);
                    }
                } else {
                    call_dynamic!(target, &long, *dst);
                }
            }
            I::Return { value } => {
                let value = value.map_or(0, |register| get!(register));
                if this != 0 {
                    // An async body's outcome is reserved; `run_body` settles its Future (#587). It is the base.
                    crate::tasks::rt_complete(ctx, this, value);
                    it!().pop_to(base);
                    return 0;
                }
                let activation = it!().activations.pop().expect("an activation");
                it!().stack.pop(activation.mark, activation.count);
                if it!().activations.len() == base {
                    return value;
                }
                load!();
                pc = it!().activations.last().expect("an activation").pc as usize;
                let dst = match &code[pc] {
                    I::Call { dst, .. } | I::CallRole { dst, .. } | I::CallFn { dst, .. } => *dst,
                    I::CallCap { dst, .. } => Some(*dst),
                    other => unreachable!("a return into {}", other.mnemonic()),
                };
                if let Some(dst) = dst {
                    set!(dst.0, value);
                }
            }
            I::Jump { target } => jump!(target),
            I::Branch { condition, then_target, else_target } => {
                if get!(condition) != 0 {
                    jump!(then_target)
                } else {
                    jump!(else_target)
                }
            }
            I::MapLookup { map, key, value, found, missing } => {
                let present = runtime::rt_map_find(ctx, function as u64, pc as u64, get!(map), get!(key));
                if present == 0 {
                    jump!(missing);
                }
                jump_then!(found, value.0, unsafe { (*ctx).scratch });
            }
            I::Throw { value } => {
                runtime::rt_throw(ctx, get!(value));
                fail!();
            }
            I::Trap { message, .. } => {
                trap(ctx, message);
                fail!();
            }
            I::Unreachable {} => {
                trap(ctx, crate::texts::UNREACHABLE);
                fail!();
            }
            I::FutSpawn { dst, future } => {
                let spawned = crate::tasks::rt_spawn(ctx, get!(future));
                check!();
                set!(dst.0, spawned);
            }
            I::Fut { op, dst, future } => {
                let result = crate::tasks::rt_fut(ctx, *op as u64, get!(future));
                check!();
                if let Some(dst) = dst {
                    set!(dst.0, result);
                }
            }
            I::ResourcePin { value } => crate::failures::rt_resource_pin(ctx, get!(value)),
            I::ResourceLeave { value } => {
                crate::failures::rt_resource_leave(ctx, get!(value));
                check!();
            }
            // The failure in a handler's register against the caught type: its payload, or the next test.
            I::Catch { failure, value, handler, next, .. } => {
                let matched = crate::failures::rt_catch(ctx, function as u64, pc as u64, get!(failure));
                if matched != 0 {
                    jump_then!(handler, value.0, unsafe { (*ctx).scratch });
                }
                jump!(next);
            }
            // #1122: an await-like helper of the async body `this`: ready (its value, if any, in `scratch`), failed
            // (the status), or pending: the body records where to resume, this very instruction, and leaves.
            I::Await { dst, future } | I::AwaitTake { dst, future } => {
                let outcome = if matches!(instruction, I::AwaitTake { .. }) {
                    crate::tasks::rt_await_take(ctx, this, get!(future))
                } else {
                    crate::tasks::rt_await(ctx, this, get!(future))
                };
                check!();
                suspend_unless_ready!(outcome, *dst);
            }
            I::RegionSettle { region, keep } => {
                let keep = keep.map_or(word::UNBOUND, |keep| get!(keep));
                let outcome = crate::tasks::rt_region_settle(ctx, this, get!(region), keep, word::UNBOUND);
                check!();
                suspend_unless_ready!(outcome, None::<okkama_sparrow::Dst>);
            }
            I::RegionSettleUnwind { dst, region, failure } => {
                let outcome = crate::tasks::rt_region_settle(ctx, this, get!(region), word::UNBOUND, get!(failure));
                check!();
                suspend_unless_ready!(outcome, Some(*dst));
            }
            I::ResourceSettle { value } => {
                let outcome = crate::tasks::rt_resource_settle(ctx, this, get!(value), word::UNBOUND);
                check!();
                suspend_unless_ready!(outcome, None::<okkama_sparrow::Dst>);
            }
            I::ResourceSettleUnwind { dst, value, failure } => {
                let outcome = crate::tasks::rt_resource_settle(ctx, this, get!(value), get!(failure));
                check!();
                suspend_unless_ready!(outcome, Some(*dst));
            }
            other => {
                let Prep::Uses { uses, def, region } = &prep[pc] else {
                    let message = format!("{}:{pc}: {} is not supported by sparrow-core yet", rt!().loaded.program.functions[function].symbol, other.mnemonic());
                    runtime::fatal(unsafe { &mut *ctx }, rt!(), message);
                    fail!();
                };
                let words = gather(regs, uses, &mut short, &mut long);
                let result = if *region {
                    crate::failures::rt_failure_op(ctx, function as u64, pc as u64, words.as_ptr())
                } else {
                    crate::ops::rt_exec(ctx, function as u64, pc as u64, words.as_ptr(), uses.as_ptr(), uses.len() as u64)
                };
                check!();
                if let Some(def) = def {
                    set!(def, result);
                }
            }
        }
        pc += 1;
    }
}

/// A word of the heap at `address` (a verified field or header of a live cell).
#[inline(always)]
fn load(address: Word) -> Word {
    // SAFETY: the verifier and the heap's layouts put `address` inside a live cell.
    unsafe { *(address as *const Word) }
}

/// Write a word of a cell being built or initialized (see [`load`]).
#[inline(always)]
fn store(address: Word, value: Word) {
    // SAFETY: as for `load`.
    unsafe { *(address as *mut Word) = value }
}

/// #1076: a read of a field that still holds the stub of `undef` (not a `bool`): the program fails.
#[cold]
#[inline(never)]
fn stub_read(ctx: *mut Ctx, function: usize, pc: usize) {
    let runtime = unsafe { &*(*ctx).runtime };
    let program = &runtime.loaded.program;
    let source = &program.functions[function];
    let ty = |register: &Register| program.ty(source.registers[register.0 as usize]);
    let (object, field) = match &source.code[pc] {
        I::ObjGet { object, field, .. } => match ty(object) {
            Some(Type::Object(declared)) => (declared.0 as usize, *field),
            _ => unreachable!("a checked obj.get reads an object"),
        },
        I::VariantGet { value, case, field, .. } => match ty(value) {
            Some(Type::Variant(v)) => (program.variants[v.0 as usize].cases[*case as usize].1.0 as usize, *field),
            _ => unreachable!("a checked variant.get reads a variant"),
        },
        _ => unreachable!("a stub check of a field read"),
    };
    let declared = &program.objects[object];
    let message = crate::texts::unbound_field(&declared.fields[field as usize].0, &declared.name);
    trap(ctx, &message);
}

#[cfg(test)]
#[path = "interp_tests.rs"]
mod tests;
