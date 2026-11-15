//! Verified SPB 17 loading and small machine-ABI adapters. Control flow stays in Cranelift.
//! Machine frames use precise stack maps; Rust executor temporaries use Runtime.roots.
#![allow(clippy::not_unsafe_ptr_arg_deref)]
use super::*;
use super::helpers::{data, error, stop};
use crate::runtime::{self, Ctx};
use crate::word::Repr;
use okkama_sparrow::spb17 as s;
use std::sync::Arc;

pub type PlainEntry = extern "C" fn(*mut Ctx, *const Word, u64) -> Word;

pub(crate) fn error_text(runtime: &Runtime, object: Word) -> Option<String> {
    let machine = runtime.champ.machine.as_ref()?;
    let descriptor = heap::payload(runtime.heap.header_of(object)) as usize;
    let declaration = machine.source.objects.get(descriptor)?;
    (!declaration.error_cases.is_empty()).then(|| super::p1::error_text(runtime, &machine.source, object))
}

pub(crate) fn error_name(runtime: &Runtime, object: Word) -> Option<String> {
    let machine = runtime.champ.machine.as_ref()?;
    let descriptor = heap::payload(runtime.heap.header_of(object)) as usize;
    let case = machine.source.objects.get(descriptor)?.error_cases.first()?;
    Some(case.split('.').next()?.to_owned())
}

/// Trusted host implementation and its independently supplied load-time declaration.
#[derive(Clone, Debug)]
pub struct ExternalEntry {
    pub declaration: s::Function,
    pub owned: Option<OwnedEntry>,
    pub plain: Option<PlainEntry>,
}

#[derive(Debug)]
pub(crate) struct Program {
    pub source: Arc<s::Program>,
    pub plans: Vec<Vec<PlanId>>,
    pub selections: Vec<Vec<(u32, PlanId)>>,
    pub entries: Vec<EntryId>,
    pub shielded_entries: Vec<EntryId>,
    pub external: Vec<Option<ExternalEntry>>,
    pub p1: Vec<bool>,
    pub native_failure: u32,
    /// The frame of each async body (draft 3 §2), index-aligned with the functions.
    pub frames: Vec<Option<super::FrameMap>>,
    /// Runtime ids of each function's D16 schedules, by local schedule index.
    pub schedules: Vec<Vec<u64>>,
}

impl Program {
    pub(crate) fn function_for_entry(&self, entry: EntryId) -> Option<usize> {
        self.entries.iter().zip(&self.shielded_entries)
            .position(|(&ordinary, &shielded)| ordinary == entry || shielded == entry)
    }
}

struct Catalog<'a>(&'a [ExternalEntry], &'a s::p1::P1Contracts);
impl s::ExternalContracts for Catalog<'_> {
    fn entry(&self, name: &str) -> Option<&s::Function> {
        self.1.entry(name).or_else(|| self.0.iter().map(|entry| &entry.declaration).find(|entry| {
            entry.proofs.entry.as_ref().and_then(|contract| contract.external.as_deref()) == Some(name)
        }))
    }

    /// Host implementations bind by name first; only an unbound canonical symbol uses the adapter.
    fn adapter(&self, declared: &s::Function) -> Option<s::Function> {
        self.1.adapter(declared)
    }
}

/// An SPB 17 program accepted by `verify_load` against the P1 catalogue and its host entries. Only `new`
/// makes one, so a runtime built from it never runs unverified code.
#[derive(Debug)]
pub(crate) struct Verified {
    source: Arc<s::Program>,
    external: Vec<ExternalEntry>,
    p1: s::p1::P1Contracts,
}

impl Verified {
    pub(crate) fn new(source: s::Program, external: Vec<ExternalEntry>) -> Result<Self, Error> {
        for entry in &external {
            let owns = entry.declaration.entry_abi != s::EntryAbi::Observation;
            if owns != entry.owned.is_some() || owns == entry.plain.is_some() {
                return Err(error("external implementation ABI mismatch"));
            }
        }
        let p1 = s::p1::P1Contracts::new(&source.objects).map_err(|error| Error(error.to_string()))?;
        s::verify_load(&source, &Catalog(&external, &p1)).map_err(|error| Error(error.to_string()))?;
        Ok(Self { source: Arc::new(source), external, p1 })
    }

    #[cfg(test)]
    pub(crate) fn source(&self) -> &s::Program {
        &self.source
    }
}

/// A native symbol (`module@major::name/arity`), as opposed to a runtime intrinsic name.
pub(super) fn native_symbol(name: &str) -> bool {
    okkama_sparrow::abi::catalogue::Symbol::parse(name).is_some()
}

pub(crate) fn repr(ty: s::Type) -> Repr {
    match ty {
        s::Type::Int => Repr::Int,
        s::Type::Bool | s::Type::Descriptor | s::Type::Never | s::Type::Byte | s::Type::Float
            | s::Type::Char => Repr::Raw,
        _ => Repr::Ref,
    }
}

impl Runtime {
    /// A source main offers an empty packet. Resource parameters need an explicit host transport.
    pub(crate) fn champ_run_owned(&mut self, ctx: &mut Ctx, function: usize, arguments: &[Word])
        -> Result<Word, Error>
    {
        if !arguments.is_empty() {
            return Err(error("parameterized owning entry requires CallOwn transport"));
        }
        let entry = self.champ_entry_id(function).ok_or_else(|| error("missing Champ entry"))?;
        let mut input = PacketSlot::default();
        if rt_own_empty(ctx, &mut input) != 0 {
            return Err(error("cannot initialize entry packet"));
        }
        let mut post = CallPost::default();
        let mut io = CallOwn { input, entry_abi: entry, output: &mut post, context: 0, cleanup_mode: 0 };
        let outcome = cleanup::rt_call_owned(ctx, entry, arguments.as_ptr(), 0, &mut io, &mut post);
        if ctx.status != runtime::OK {
            return Ok(0);
        }
        match outcome {
            0 => {
                if !self.champ_packet_keys(&post.packet)?.is_empty() {
                    return Err(error("main returned an ownership packet"));
                }
                post.packet.record = 0;
                Ok(post.value)
            }
            1 => {
                ctx.failure = post.failure;
                ctx.status = runtime::FAILED;
                Ok(0)
            }
            _ => Err(error("invalid main outcome")),
        }
    }

    /// Runtime-local CallOwn entry ID for a verified SPB function index.
    pub fn champ_entry_id(&self, function: usize) -> Option<EntryId> {
        self.champ.machine.as_ref()?.entries.get(function).copied()
    }

    /// The runtime id of a function's D16 schedule, the `schedule` operand of `champ_cleanup_settle`.
    pub fn champ_schedule_id(&self, function: usize, schedule: u32) -> Option<u64> {
        self.champ.machine.as_ref()?.schedules.get(function)?.get(schedule as usize).copied()
    }

    /// There is no SPB 16 fallback. External contracts are checked before any machine code is published.
    pub fn new_champ(source: s::Program, external: Vec<ExternalEntry>) -> Result<Box<Self>, Error> {
        Self::new_champ_verified(&Verified::new(source, external)?)
    }

    /// A runtime for a program the loader has already checked: no second `verify_load`, and the source is
    /// shared with every other runtime of the same verified program.
    pub(crate) fn new_champ_verified(verified: &Verified) -> Result<Box<Self>, Error> {
        let source = &*verified.source;
        let external = &verified.external;
        let catalog = Catalog(external, &verified.p1);
        let mut legacy = okkama_sparrow::Program {
            entry: source.entry.map(okkama_sparrow::FuncRef),
            ..okkama_sparrow::Program::default()
        };
        legacy.types.push(okkama_sparrow::Type::Byte);
        legacy.handles = source.objects.iter().map(|object| object.name.clone()).collect();
        // A Future's outcome type, as Tomato's declarations name it (draft 3 §2); a void Future has none.
        let mut futures = Vec::new();
        for result in &source.tables.future_results {
            let outcome = result.map(|ty| {
                legacy.types.push(legacy_type(ty, &futures));
                okkama_sparrow::TypeRef(legacy.types.len() as u32 - 1)
            });
            futures.push(outcome);
        }
        // This declaration view serves existing descriptors, diagnostics and heap layout consumers only.
        // No old bytecode is generated or executed.
        for object in &source.objects {
            let fields = object.layout.storage_fields.iter().enumerate().map(|(index, &ty)| {
                let id = okkama_sparrow::TypeRef(legacy.types.len() as u32);
                legacy.types.push(legacy_type(ty, &futures));
                (format!("storage{index}"), id)
            }).collect();
            let descriptor = legacy.objects.len() as u32;
            let destroy = source.functions.iter().position(|function| {
                function.proofs.entry.as_ref().is_some_and(|entry| entry.destroy == Some(descriptor))
            }).map(|index| okkama_sparrow::FuncRef(index as u32));
            legacy.objects.push(okkama_sparrow::ObjectDecl {
                name: object.name.clone(), fields, does: vec![], destroy, builtin: false,
            });
        }
        for function in &source.functions {
            let registers = function.registers.iter().map(|&ty| {
                let id = okkama_sparrow::TypeRef(legacy.types.len() as u32);
                legacy.types.push(legacy_type(ty, &futures));
                id
            }).collect();
            let params = function.params.iter().filter_map(|location| match location {
                s::Location::Register(id) => Some(okkama_sparrow::Register(*id)),
                _ => None,
            }).collect();
            legacy.functions.push(okkama_sparrow::Function {
                symbol: function.symbol.clone(), is_async: false,
                // Tomato resolves the provider by symbol; an unresolved adapter finds none and fails at the call.
                native: function.proofs.entry.as_ref().and_then(|entry| entry.external.clone())
                    .filter(|name| native_symbol(name)),
                intrinsic: None,
                registers, params, result: None, code: vec![], handlers: vec![],
            });
        }
        // Native diagnostics use Sparrow's existing four-string NativeFailure carrier.
        // This private runtime descriptor is never a source object or a resource identity.
        let native_failure = legacy.objects.len() as u32;
        let text = okkama_sparrow::TypeRef(legacy.types.len() as u32);
        legacy.types.push(okkama_sparrow::Type::Str);
        legacy.objects.push(okkama_sparrow::ObjectDecl {
            name: "NativeFailure".into(),
            fields: ["domain", "code", "operation", "message"].into_iter()
                .map(|name| (name.into(), text)).collect(),
            does: vec![], destroy: None, builtin: true,
        });
        let mut runtime = Self::new(crate::program::Loaded::new(legacy))?;
        let mut layouts = source.objects.iter().map(|object| {
            let layout = &object.layout;
            ObjectRuntimeLayout::new(layout.storage_fields.iter().copied().map(repr).collect(), LayoutFacts {
                has_destroy: layout.has_destroy,
                may_reach_resource: layout.may_reach_resource,
                may_async_destroy: layout.may_async_destroy,
            }, layout.selection_layout.iter().map(|&index| index as usize).collect())
        }).collect::<Result<Vec<_>, _>>().map_err(|error| Error(error.to_string()))?;
        layouts.push(ObjectRuntimeLayout::new(vec![Repr::Ref; 4], LayoutFacts::default(), vec![])
            .map_err(|error| Error(error.to_string()))?);
        // An async body's frame is traced by its frame map, not by Tomato's register layout.
        let frames = (0..source.functions.len()).map(|index| super::FrameMap::new(source, index)).collect::<Vec<_>>();
        let mut heap_layouts = runtime.loaded.layouts();
        for (index, map) in frames.iter().enumerate() {
            if let Some(map) = map {
                heap_layouts.frames[index] = map.trace.clone();
            }
        }
        runtime.heap = Heap::new_champ(heap_layouts, layouts.into_boxed_slice());
        let plans = runtime.champ_load_cleanup_plans(source, &catalog)?;
        let schedules = source.functions.iter().map(|function| {
            function.proofs.schedules.iter()
                .map(|schedule| runtime.champ_register_schedule(schedule.services.len() as u64))
                .collect()
        }).collect();
        let mut selections = Vec::new();
        let mut entries = Vec::new();
        let mut shielded_entries = Vec::new();
        let mut implementations = Vec::new();
        for function in &source.functions {
            let mut relocated = Vec::new();
            for selection in s::static_selections(source, function).map_err(|error| Error(error.to_string()))? {
                let indices = selection.indices.iter().map(|&index| index as usize).collect();
                relocated.push((selection.pc, runtime.champ_register_selection(indices)));
            }
            selections.push(relocated);
            let replacements = match &function.entry_abi {
                s::EntryAbi::Defer { labels } => labels.len() as u64,
                _ => 0,
            };
            let arguments = function.params.iter().filter_map(|location| match location {
                s::Location::Register(id) => Some(repr(function.registers[*id as usize])),
                _ => None,
            }).collect();
            let mut contract = EntryContract {
                invoke: owned_adapter, arguments, replacements, summary: 0,
                cleanup_mode: if matches!(function.entry_abi, s::EntryAbi::Defer { .. }) { 1 } else { 0 },
            };
            let id = runtime.champ_register_entry(contract.clone());
            entries.push(id);
            let shielded = if function.entry_abi == s::EntryAbi::Owned {
                contract.cleanup_mode = CLEANUP_SHIELDED;
                runtime.champ_register_entry(contract)
            } else {
                id
            };
            shielded_entries.push(shielded);
            implementations.push(function.proofs.entry.as_ref().and_then(|entry| entry.external.as_deref())
                .and_then(|name| external.iter().find(|entry| {
                    entry.declaration.proofs.entry.as_ref().and_then(|entry| entry.external.as_deref()) == Some(name)
                })).cloned());
        }
        // Trusted P1/native entries run through the finite adapters of `p1`, as do checked
        // unresolved-provider adapters that no embedder implementation binds by name.
        let p1 = source.functions.iter().zip(&implementations).map(|(function, implementation)| {
            function.proofs.entry.as_ref().and_then(|entry| entry.external.as_deref())
                .is_some_and(|name| s::ExternalContracts::entry(catalog.1, name).is_some()
                    || (implementation.is_none() && catalog.1.adapter(function).is_some()))
        }).collect();
        runtime.champ.machine = Some(Program {
            source: verified.source.clone(), plans, selections, entries, shielded_entries,
            external: implementations, p1, native_failure, frames, schedules,
        });
        Ok(Box::new(runtime))
    }
}

fn legacy_type(ty: s::Type, futures: &[Option<okkama_sparrow::TypeRef>]) -> okkama_sparrow::Type {
    use okkama_sparrow::Type as T;
    match ty {
        s::Type::Error => T::Failure,
        s::Type::Bytes | s::Type::Array(_) | s::Type::Map(_) | s::Type::MapBuilder(_) | s::Type::Builder(_) => {
            T::Array(okkama_sparrow::TypeRef(0))
        }
        s::Type::Int => T::Int,
        s::Type::Bool | s::Type::Descriptor | s::Type::Never => T::Bool,
        s::Type::Str => T::Str,
        s::Type::Object(id) | s::Type::Successor(id) | s::Type::Variant(id) | s::Type::Role(id)
            | s::Type::RoleSuccessor(id) => T::Object(okkama_sparrow::ObjectRef(id)),
        s::Type::Range => T::Range,
        // Handle indices name SPB objects; the declaration view lists every object name as a handle name.
        s::Type::Handle(id) => T::Handle(id),
        s::Type::Failure => T::Failure,
        s::Type::Byte => T::Byte,
        s::Type::Float => T::Float,
        s::Type::Rat => T::Rat,
        s::Type::Char => T::Char,
        // A Future is a reference to its frame (Champ draft 3 §2), with its outcome type when it has one.
        s::Type::Future(index) => T::Future(futures.get(index as usize).copied().flatten()),
        s::Type::Any => T::Any,
    }
}

extern "C" fn owned_adapter(ctx: *mut Ctx, io: *mut CallOwn, args: *const Word, count: u64) -> Outcome {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let machine = runtime.champ.machine.as_ref().expect("Champ program");
    let Some(function) = machine.function_for_entry(unsafe { (*io).entry_abi }) else {
        return stop(ctx, error("machine entry ID"));
    };
    let code = match crate::jit::compile(runtime, function) {
        Ok(code) => code,
        Err(error) => return stop(ctx, error),
    };
    // Fixed span ABI: language arity never competes with a hidden ownership argument.
    let entry: extern "C" fn(*mut Ctx, *mut CallOwn, *const Word, u64) -> Outcome =
        unsafe { std::mem::transmute(code) };
    entry(ctx, io, args, count)
}

pub extern "C" fn rt_machine_external(ctx: *mut Ctx, function: u64, io: *mut CallOwn,
    args: *const Word, count: u64) -> Word
{
    // The direct call of a compiled function is on every level of a recursion: its frame stays
    // small, and the provider paths with their large temporaries live in their own frame.
    let runtime = unsafe { &mut *(*ctx).runtime };
    let machine = runtime.champ.machine.as_ref().expect("Champ program");
    if !io.is_null() || machine.source.functions[function as usize].code.is_empty() {
        return machine_provider(ctx, function, io, args, count);
    }
    let code = match crate::jit::compile(runtime, function as usize) {
        Ok(code) => code,
        Err(error) => return stop(ctx, error),
    };
    let arguments = if count == 0 { &[] } else {
        unsafe { std::slice::from_raw_parts(args, count as usize) }
    };
    unsafe { crate::jit::invoke(code, ctx, arguments) }
}

#[inline(never)]
fn machine_provider(ctx: *mut Ctx, function: u64, io: *mut CallOwn, args: *const Word, count: u64) -> Word {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let machine = runtime.champ.machine.as_ref().expect("Champ program");
    if machine.p1[function as usize] {
        let arguments = if count == 0 { &[] } else {
            unsafe { std::slice::from_raw_parts(args, count as usize) }
        };
        return super::p1::invoke(ctx, function as usize, io, arguments);
    }
    let external = machine.external[function as usize].clone();
    let Some(external) = external else { return stop(ctx, error("unbound external entry")) };
    if io.is_null() {
        match external.plain {
            Some(entry) => entry(ctx, args, count),
            None => stop(ctx, error("external observation ABI")),
        }
    } else {
        match external.owned {
            Some(entry) => entry(ctx, io, args, count),
            None => stop(ctx, error("external owning ABI")),
        }
    }
}

/// Test-only collection at each collecting/reentrant machine call, before Post publication.
#[cfg(test)]
pub extern "C" fn rt_machine_safepoint(ctx: *mut Ctx) {
    let frame = runtime::frame_pointer!();
    unsafe { &mut *(*ctx).runtime }.safepoint(frame, unsafe { &*ctx }, &[]);
}

pub extern "C" fn rt_machine_scope(ctx: *mut Ctx, function: u64, scope_id: u64,
    parent: Word, post: *mut ScopeSlot) -> Outcome
{
    let runtime = unsafe { &mut *(*ctx).runtime };
    let result = (|| {
        let owner = runtime.champ_new_owner()?;
        let holder = runtime.champ_new_holder()?;
        let parent_owner = parent;
        // Binding representations belong to this activation's owner, including staging fields.
        let source = runtime.champ.machine.as_ref().expect("Champ program").source.clone();
        let function = &source.functions[function as usize];
        for instruction in &function.code {
            let s::Instruction::Champ { op, arguments, .. } = instruction else { continue };
            if !matches!(op, s::Opcode::OwnBind | s::Opcode::OwnRebind | s::Opcode::OwnStage) {
                continue;
            }
            let slot = arguments.iter().find_map(|arg| match arg {
                s::Argument::Binding(id) | s::Argument::Field(id) => Some(*id),
                _ => None,
            }).expect("verified binding");
            let ty = arguments.iter().find_map(|arg| match arg {
                s::Argument::Location(s::Location::Register(id)) => Some(function.registers[*id as usize]),
                _ => None,
            }).expect("verified binding value");
            runtime.champ_register_value_repr(owner, slot as u64, *op == s::Opcode::OwnStage, repr(ty));
        }
        let _ = scope_id;
        Ok(ScopeInstance { holder, owner, parent_owner })
    })();
    match result {
        Ok(instance) => rt_scope_begin(ctx, &instance, post),
        Err(error) => stop(ctx, error),
    }
}

pub extern "C" fn rt_machine_invalid(ctx: *mut Ctx) -> Outcome {
    stop(ctx, error("invalid machine outcome or unreachable SPB block"))
}

pub extern "C" fn rt_machine_destination(ctx: *mut Ctx, state: Word, post: *mut ExitDestination) -> Outcome {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let result = if state == 0 {
        runtime.champ_new_holder().and_then(|holder| {
            runtime.champ_new_owner().map(|owner| ExitDestination { holder, owner })
        })
    } else {
        match data(runtime, state) {
            Ok(Data::Scope(scope)) => Ok(ExitDestination { holder: scope.holder, owner: scope.owner }),
            _ => Err(error("exit destination")),
        }
    };
    match result {
        Ok(destination) => {
            unsafe { post.write(destination) };
            NORMAL
        }
        Err(error) => stop(ctx, error),
    }
}

pub extern "C" fn rt_machine_serial(ctx: *mut Ctx, object: Word) -> Word {
    unsafe { &*(*ctx).runtime }.heap.champ_identity(object).expect("verified identity").serial
}

pub extern "C" fn rt_machine_fatal_stack(ctx: *mut Ctx) -> Outcome {
    stop(ctx, error("cleanup stack headroom exhausted"))
}

/// D1: lower the entry guard only for the dynamic extent of cleanup, and never lower an existing floor.
pub(super) fn enter_cleanup(ctx: *mut Ctx) -> (u64, Option<u64>) {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let old = unsafe { (*ctx).stack_limit };
    let previous = runtime.champ.cleanup_floor;
    let floor = previous.or(runtime.champ.rejection_floor)
        .unwrap_or_else(|| old.saturating_sub(runtime::STACK_HEADROOM));
    runtime.champ.cleanup_floor = Some(floor);
    unsafe { (*ctx).stack_limit = old.min(floor.saturating_add(crate::jit::MAX_FRAME)) };
    (old, previous)
}

pub(super) fn leave_cleanup(ctx: *mut Ctx, saved: (u64, Option<u64>)) {
    unsafe {
        (*ctx).stack_limit = saved.0;
        (*(*ctx).runtime).champ.cleanup_floor = saved.1;
    }
}

pub extern "C" fn rt_machine_cleanup_mode(ctx: *mut Ctx) -> Word {
    u64::from(unsafe { &*(*ctx).runtime }.champ.cleanup_floor.is_some())
}

pub extern "C" fn rt_machine_secondary(ctx: *mut Ctx, primary: Word, secondary: Word) -> Word {
    unsafe { &mut *(*ctx).runtime }.failure_link(primary, secondary, true)
}

pub extern "C" fn rt_machine_caused(ctx: *mut Ctx, fresh: Word, handled: Word) -> Word {
    unsafe { &mut *(*ctx).runtime }.failure_link(fresh, handled, false)
}

pub extern "C" fn rt_machine_fail(ctx: *mut Ctx, kind: Word, payload: Word) -> Word {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let failure = match kind {
        0 => runtime::FailureKind::Thrown(payload),
        1 => runtime::FailureKind::Program(runtime.heap.text(payload).to_owned()),
        2 => runtime::FailureKind::Panic(runtime.heap.text(payload).to_owned()),
        _ => return stop(ctx, error("invalid verified failure kind")),
    };
    runtime.failure_new(failure)
}

pub extern "C" fn rt_machine_range(ctx: *mut Ctx, kind: Word, left: Word, right: Word, inclusive: Word) -> Word {
    let frame = runtime::frame_pointer!();
    let runtime = unsafe { &mut *(*ctx).runtime };
    let integer = |word| match runtime.heap.integer(word) {
        heap::Integer::Small(value) => num_bigint::BigInt::from(value),
        heap::Integer::Big(value) => value.clone(),
    };
    let value = if kind == 0 {
        if integer(left) > integer(right) {
            let failure = runtime.failure_new(runtime::FailureKind::Program(
                "Range start must be less than or equal to range end".into()));
            unsafe {
                (*ctx).failure = failure;
                (*ctx).status = runtime::FAILED;
            }
            return 0;
        }
        runtime.heap.allocate_payload(Box::new(crate::ops::RangeValue { start: left, end: right,
            inclusive: inclusive != 0 }))
    } else {
        let range = runtime.heap.payload::<crate::ops::RangeValue>(left).expect("verified range");
        let value = match kind {
            1 => integer(range.start),
            2 => integer(range.end) + u8::from(range.inclusive),
            3 => integer(range.end) - integer(range.start) + u8::from(range.inclusive),
            _ => return stop(ctx, error("invalid verified range operation")),
        };
        runtime.heap.big(value)
    };
    let repr = if kind == 0 { Repr::Ref } else { Repr::Int };
    runtime.safepoint(frame, unsafe { &*ctx }, &[(value, repr)]);
    value
}

/// The operand code of a `Convert` helper call for an SPB 17 scalar type.
pub(crate) fn scalar_code(ty: s::Type) -> Option<u64> {
    Some(match ty {
        s::Type::Int => 0,
        s::Type::Bool => 1,
        s::Type::Str => 2,
        s::Type::Byte => 3,
        s::Type::Float => 4,
        s::Type::Rat => 5,
        s::Type::Char => 6,
        _ => return None,
    })
}

/// Tomato's type and representation for a `Convert` operand code.
fn scalar_type(code: Word) -> Option<(okkama_sparrow::Type, Repr)> {
    Some(match code {
        0 => (okkama_sparrow::Type::Int, Repr::Int),
        1 => (okkama_sparrow::Type::Bool, Repr::Raw),
        2 => (okkama_sparrow::Type::Str, Repr::Ref),
        3 => (okkama_sparrow::Type::Byte, Repr::Raw),
        4 => (okkama_sparrow::Type::Float, Repr::Raw),
        5 => (okkama_sparrow::Type::Rat, Repr::Ref),
        6 => (okkama_sparrow::Type::Char, Repr::Raw),
        _ => return None,
    })
}

/// SPB 17 `Convert` on Tomato's scalar representations. A failing conversion is a program failure.
/// Conversions to byte are Tomato's `byte.from`; every other pair is Tomato's `cvt`.
pub extern "C" fn rt_machine_convert(ctx: *mut Ctx, from: Word, to: Word, value: Word) -> Word {
    use okkama_sparrow::Type as T;
    let frame = runtime::frame_pointer!();
    let runtime = unsafe { &mut *(*ctx).runtime };
    let (Some((source, _)), Some((target, repr))) = (scalar_type(from), scalar_type(to)) else {
        return stop(ctx, error("invalid verified conversion"));
    };
    let outcome = match (&source, &target) {
        (T::Int | T::Float | T::Rat, T::Byte) => {
            let kind = match source {
                T::Int => crate::program::Kind::Int,
                T::Float => crate::program::Kind::Float,
                _ => crate::program::Kind::Ref,
            };
            crate::ops::byte_from(runtime, kind, value, None)
        }
        _ => crate::ops::convert_scalar(runtime, Some(source), Some(target), value),
    };
    match outcome {
        Ok(word) => {
            runtime.safepoint(frame, unsafe { &*ctx }, &[(word, repr)]);
            word
        }
        Err(message) => {
            // A program failure with Tomato's message, as the program sees it.
            super::helpers::fail(ctx, crate::Error(message));
            0
        }
    }
}

/// An SPB 17 int, byte, float, rat or str family operation (`DataOp::scalar_code`) on Tomato's
/// representations and with Tomato's semantics (`crate::ops`); unused operands are zero. A failure is a
/// program failure with Tomato's message; a result larger than the memory budget stops.
pub extern "C" fn rt_machine_scalar(ctx: *mut Ctx, op: Word, first: Word, second: Word, third: Word) -> Word {
    let frame = runtime::frame_pointer!();
    let runtime = unsafe { &mut *(*ctx).runtime };
    let Some((op, result)) = s::DataOp::from_scalar_code(op)
        .and_then(|op| op.scalar_signature().map(|(_, result)| (op, result)))
    else {
        return stop(ctx, error("invalid verified scalar operation"));
    };
    if runtime.limits.memory.is_some() {
        let bytes = scalar_result_bytes(runtime, op, first, second, third);
        if runtime.would_exceed(bytes) {
            return runtime.exhaust(unsafe { &mut *ctx }, crate::budget::Exhausted::Memory);
        }
    }
    match scalar(runtime, op, first, second, third) {
        Ok(word) => {
            runtime.safepoint(frame, unsafe { &*ctx }, &[(word, repr(result))]);
            word
        }
        Err(message) => {
            // A program failure with Tomato's message, as the program sees it.
            super::helpers::fail(ctx, crate::Error(message));
            0
        }
    }
}

/// The size of a result that can be much larger than its operands, before it is built (#72).
fn scalar_result_bytes(runtime: &Runtime, op: s::DataOp, first: Word, second: Word, third: Word) -> u64 {
    use s::DataOp as D;
    match op {
        D::StrReplace => {
            let text = |word| runtime.heap.text(word);
            crate::ops::replace_bytes(text(first), text(second), text(third))
        }
        D::StrRepeat => {
            let count = runtime.small_or_big(second).and_then(|count| u64::try_from(count).ok()).unwrap_or(0);
            (runtime.heap.text(first).len() as u64).saturating_mul(count)
        }
        _ => 0,
    }
}

/// The value of a scalar family operation, as Tomato's instruction of the same name computes it.
pub(crate) fn scalar(runtime: &mut Runtime, op: s::DataOp, first: Word, second: Word, third: Word)
    -> Result<Word, String>
{
    use crate::ops::{self, Bitwise as B, RatArith as R};
    use crate::program::Kind;
    use okkama_sparrow::{BitOp, StrFn};
    use s::DataOp as D;
    let bits = |op| match op {
        D::IntAnd | D::ByteAnd => B::And,
        D::IntOr | D::ByteOr => B::Or,
        D::IntXor | D::ByteXor => B::Xor,
        D::IntShl | D::ByteShl => B::Shl,
        D::IntShr | D::ByteShr => B::Shr,
        _ => B::Ushr,
    };
    let action = |op| match op {
        D::IntBitCount | D::ByteBitCount => BitOp::BitCount,
        D::IntLeadingZeros | D::ByteLeadingZeros => BitOp::LeadingZeros,
        D::IntTrailingZeros | D::ByteTrailingZeros => BitOp::TrailingZeros,
        D::IntReverseBits | D::ByteReverseBits => BitOp::ReverseBits,
        D::IntSwapBytes | D::ByteSwapBytes => BitOp::SwapBytes,
        D::IntRotl | D::ByteRotl => BitOp::RotateLeft,
        _ => BitOp::RotateRight,
    };
    let float = |code, left: Word, right: Word| {
        crate::scalar::float_binary(code, f64::from_bits(left), f64::from_bits(right))
            .map(f64::to_bits)
            .map_err(String::from)
    };
    match op {
        D::IntAnd | D::IntOr | D::IntXor | D::IntShl | D::IntShr | D::IntUshr => {
            ops::int_bitwise(runtime, bits(op), first, second)
        }
        D::IntNot => ops::int_not(runtime, first),
        D::IntAbs => ops::int_abs(runtime, first),
        D::IntSign => ops::sign(runtime, Kind::Int, first),
        D::IntBitCount | D::IntLeadingZeros | D::IntTrailingZeros | D::IntReverseBits | D::IntSwapBytes => {
            ops::bit_action(runtime, action(op), false, first, None)
        }
        D::IntRotl | D::IntRotr => ops::bit_action(runtime, action(op), false, first, Some(second)),
        D::ByteAnd | D::ByteOr | D::ByteXor | D::ByteShl | D::ByteShr => {
            ops::byte_bitwise(runtime, bits(op), first, second)
        }
        // OIR-2's byte.ushr counts in byte; Tomato's count is an int word.
        D::ByteUshr => {
            let count = runtime.heap.int(i64::from(second as u8));
            ops::byte_bitwise(runtime, B::Ushr, first, count)
        }
        D::ByteNot => Ok(Word::from(!(first as u8))),
        D::ByteBitCount | D::ByteLeadingZeros | D::ByteTrailingZeros | D::ByteReverseBits | D::ByteSwapBytes => {
            ops::bit_action(runtime, action(op), true, first, None)
        }
        D::ByteRotl | D::ByteRotr => ops::bit_action(runtime, action(op), true, first, Some(second)),
        D::FloatAdd => float(crate::scalar::FLOAT_ADD, first, second),
        D::FloatSub => float(crate::scalar::FLOAT_SUB, first, second),
        D::FloatMul => float(crate::scalar::FLOAT_MUL, first, second),
        D::FloatDiv => float(crate::scalar::FLOAT_DIV, first, second),
        D::FloatMod => float(crate::scalar::FLOAT_MOD, first, second),
        D::FloatPow => float(crate::scalar::FLOAT_POW, first, second),
        D::FloatNeg => Ok((-f64::from_bits(first)).to_bits()),
        D::FloatAbs => Ok(f64::from_bits(first).abs().to_bits()),
        D::FloatSign => ops::sign(runtime, Kind::Float, first),
        D::FloatSqrt => ops::float_sqrt(first),
        D::RatAdd => ops::rat_arith(runtime, R::Add, first, second),
        D::RatSub => ops::rat_arith(runtime, R::Sub, first, second),
        D::RatMul => ops::rat_arith(runtime, R::Mul, first, second),
        D::RatDiv => ops::rat_arith(runtime, R::Div, first, second),
        D::RatMod => ops::rat_arith(runtime, R::Mod, first, second),
        D::RatNeg | D::RatAbs => ops::rat_unary(runtime, op == D::RatNeg, first),
        D::RatSign => ops::sign(runtime, Kind::Ref, first),
        D::RatNumerator | D::RatDenominator => Ok(ops::rat_part(runtime, first, op == D::RatDenominator)),
        D::StrSlice => ops::str_slice(runtime, first, second, third),
        D::StrSplit => Ok(ops::str_split(runtime, first, second)),
        D::StrReplace | D::StrReplaceFirst => ops::str_replace(runtime, first, second, third, op == D::StrReplaceFirst),
        D::StrTrim => Ok(ops::str_trim(runtime, first)),
        D::StrChomp => Ok(ops::str_chomp(runtime, first)),
        D::StrUtf8 => {
            let bytes = runtime.heap.text(first).as_bytes().to_vec();
            Ok(runtime.heap.allocate_bytes(&bytes))
        }
        D::StrCharacters => ops::str_fn(runtime, StrFn::Characters, vec![first]),
        D::StrTrimStart | D::StrTrimEnd | D::StrLower | D::StrUpper => {
            let op = match op {
                D::StrTrimStart => StrFn::TrimStart,
                D::StrTrimEnd => StrFn::TrimEnd,
                D::StrLower => StrFn::Lower,
                _ => StrFn::Upper,
            };
            ops::str_fn(runtime, op, vec![first])
        }
        D::StrRepeat | D::StrFind | D::StrFindLast | D::StrStartsWith | D::StrEndsWith => {
            let op = match op {
                D::StrRepeat => StrFn::Repeat,
                D::StrFind => StrFn::Find,
                D::StrFindLast => StrFn::FindLast,
                D::StrStartsWith => StrFn::StartsWith,
                _ => StrFn::EndsWith,
            };
            ops::str_fn(runtime, op, vec![first, second])
        }
        _ => Err("invalid verified scalar operation".into()),
    }
}

/// A payload field of a variant value whose active case must be the object `case`.
pub extern "C" fn rt_machine_variant_get(ctx: *mut Ctx, case: Word, storage: Word, value: Word) -> Word {
    let runtime = unsafe { &*(*ctx).runtime };
    if u64::from(heap::payload(runtime.heap.header_of(value))) != case {
        super::helpers::fail(ctx, crate::Error("Variant payload of an inactive case".into()));
        return 0;
    }
    runtime.heap.field(value, storage as usize)
}

/// A role value narrowed to `role`: the value itself when its object implements the role, otherwise
/// Tomato's program failure for a missing witness (`I::RoleNarrow` without a source message).
pub extern "C" fn rt_machine_role_narrow(ctx: *mut Ctx, role: Word, value: Word) -> Word {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let Some(machine) = runtime.champ.machine.as_ref() else {
        return stop(ctx, error("role narrowing outside a Champ program"));
    };
    let header = runtime.heap.header_of(value);
    let implements = heap::kind(header) == heap::OBJECT && machine.source.objects
        .get(heap::payload(header) as usize)
        .is_some_and(|object| object.implements.iter().any(|table| u64::from(table.role) == role));
    if implements {
        return value;
    }
    let failure = runtime.failure_new(runtime::FailureKind::Program("role witness is missing".into()));
    unsafe {
        (*ctx).failure = failure;
        (*ctx).status = runtime::FAILED;
    }
    0
}

/// The order of two exact rationals: -1, 0 or 1.
pub extern "C" fn rt_machine_rat_order(ctx: *mut Ctx, left: Word, right: Word) -> i64 {
    let runtime = unsafe { &*(*ctx).runtime };
    match runtime.rational(left).compare(runtime.rational(right)) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// Deep value equality (§16.3) of two objects of descriptor `descriptor`, or of two values of the role
/// `descriptor` (the comparison reads the actual objects): 1 or 0. Comparing an unbound field is a program
/// failure, as in Tomato.
pub extern "C" fn rt_machine_value_eq(ctx: *mut Ctx, descriptor: Word, left: Word, right: Word) -> Word {
    let runtime = unsafe { &*(*ctx).runtime };
    let Some(machine) = runtime.champ.machine.as_ref() else {
        return stop(ctx, error("value equality outside a Champ program"));
    };
    let program = machine.source.clone();
    match value_equal(runtime, &program, left, right, s::Type::Object(descriptor as u32)) {
        Ok(equal) => Word::from(equal),
        Err(message) => {
            // A program failure with Tomato's message, as the program sees it.
            super::helpers::fail(ctx, crate::Error(message));
            0
        }
    }
}

pub(crate) fn value_equal(runtime: &Runtime, program: &s::Program, left: Word, right: Word, ty: s::Type)
    -> Result<bool, String>
{
    let mut pending = vec![(left, right, ty)];
    let mut seen = std::collections::HashSet::new();
    while let Some((a, b, ty)) = pending.pop() {
        let equal = match ty {
            s::Type::Int => runtime.big_value(a) == runtime.big_value(b),
            s::Type::Float => f64::from_bits(a) == f64::from_bits(b),
            s::Type::Bool | s::Type::Byte | s::Type::Char | s::Type::Descriptor | s::Type::Never => a == b,
            _ if a == b => true,
            _ if a == 0 || b == 0 => return Err("cannot compare unbound object fields".into()),
            s::Type::Str => runtime.heap.text(a) == runtime.heap.text(b),
            s::Type::Rat => runtime.rational(a) == runtime.rational(b),
            s::Type::Handle(_) => return Err("cannot compare native handles".into()),
            s::Type::Future(_) => return Err("cannot compare Futures".into()),
            s::Type::Bytes => runtime.heap.bytes(a) == runtime.heap.bytes(b),
            s::Type::Range => {
                let x = runtime.heap.payload::<crate::ops::RangeValue>(a).expect("verified range");
                let y = runtime.heap.payload::<crate::ops::RangeValue>(b).expect("verified range");
                x.inclusive == y.inclusive && runtime.big_value(x.start) == runtime.big_value(y.start)
                    && runtime.big_value(x.end) == runtime.big_value(y.end)
            }
            s::Type::Array(shape) => {
                let (x, y) = (runtime.heap.elements(a), runtime.heap.elements(b));
                let element = shape.element().expect("verified array shape");
                if x.len() == y.len() && seen.insert((a, b)) {
                    pending.extend(x.iter().zip(y.iter()).map(|(&x, &y)| (x, y, element)));
                }
                x.len() == y.len()
            }
            // An `any` value compares as its box: one boxed type, equal values.
            s::Type::Object(_) | s::Type::Successor(_) | s::Type::Variant(_) | s::Type::Role(_)
                | s::Type::RoleSuccessor(_) | s::Type::Any => {
                let descriptor = heap::payload(runtime.heap.header_of(a));
                if descriptor != heap::payload(runtime.heap.header_of(b)) {
                    return Ok(false);
                }
                let object = &program.objects[descriptor as usize];
                if seen.insert((a, b)) {
                    let storage = object.fields.iter().filter(|field| !field.once).map(|field| field.storage)
                        .collect::<Vec<_>>();
                    // An enum has no source fields: its tag is its value.
                    let storage = if object.fields.is_empty() {
                        (0..object.layout.storage_fields.len() as u32).collect()
                    } else {
                        storage
                    };
                    for index in storage {
                        let field = object.layout.storage_fields[index as usize];
                        let index = index as usize;
                        let (x, y) = (runtime.heap.field(a, index), runtime.heap.field(b, index));
                        // As in Tomato, meeting an unbound field fails before any field is compared. An unbound
                        // reference or int is the zero word; a raw scalar field has no unbound marker.
                        if (field.traced() || field == s::Type::Int) && (x == 0 || y == 0) {
                            return Err("cannot compare unbound object fields".into());
                        }
                        pending.push((x, y, field));
                    }
                }
                true
            }
            s::Type::Map(id) => {
                let Some(&s::Composite::Map { value, .. }) = program.composites.get(id as usize) else {
                    return Err("map equality on a non-map composite".into());
                };
                let x = &runtime.heap.payload::<crate::ops::MapValue>(a).expect("verified map").entries;
                let y = &runtime.heap.payload::<crate::ops::MapValue>(b).expect("verified map").entries;
                if x.len() != y.len() {
                    return Ok(false);
                }
                for ((left_key, left), (right_key, right)) in x.iter().zip(y.iter()) {
                    if left_key != right_key {
                        return Ok(false);
                    }
                    pending.push((left.1, right.1, value));
                }
                true
            }
            s::Type::Error | s::Type::Failure | s::Type::Builder(_) | s::Type::MapBuilder(_) => false,
        };
        if !equal {
            return Ok(false);
        }
    }
    Ok(true)
}

pub extern "C" fn rt_machine_catch(ctx: *mut Ctx, failure: Word, descriptor: Word) -> Word {
    let runtime = unsafe { &*(*ctx).runtime };
    match runtime.failure(failure).map(|failure| &failure.kind) {
        Some(runtime::FailureKind::Thrown(payload) | runtime::FailureKind::Native(payload))
            if descriptor == u64::MAX || u64::from(heap::payload(runtime.heap.header_of(*payload))) == descriptor => {
            *payload
        }
        _ => 0,
    }
}

pub extern "C" fn rt_machine_plain_packet(ctx: *mut Ctx, packet: *const PacketSlot) -> Word {
    let runtime = unsafe { &*(*ctx).runtime };
    if runtime.champ_packet_keys(unsafe { &*packet }).is_ok_and(|keys| keys.is_empty()) {
        NORMAL
    } else {
        stop(ctx, error("plain entry received resource obligations"))
    }
}

pub extern "C" fn rt_machine_text_order(ctx: *mut Ctx, left: Word, right: Word) -> i64 {
    let runtime = unsafe { &*(*ctx).runtime };
    match runtime.heap.text(left).cmp(runtime.heap.text(right)) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

pub extern "C" fn rt_machine_text_len(ctx: *mut Ctx, text: Word) -> Word {
    let runtime = unsafe { &*(*ctx).runtime };
    crate::word::small(runtime.heap.text(text).chars().count() as i64).expect("addressable text length")
}

#[cfg(test)]
pub(crate) fn graph_walks() -> usize {
    super::helpers::GRAPH_WALKS.with(|count| count.get())
}
