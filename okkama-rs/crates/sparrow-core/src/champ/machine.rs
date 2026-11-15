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
}

impl Program {
    pub(crate) fn function_for_entry(&self, entry: EntryId) -> Option<usize> {
        self.entries.iter().zip(&self.shielded_entries)
            .position(|(&ordinary, &shielded)| ordinary == entry || shielded == entry)
    }
}

struct Catalog<'a>(&'a [ExternalEntry], s::p1::P1Contracts);
impl s::ExternalContracts for Catalog<'_> {
    fn entry(&self, name: &str) -> Option<&s::Function> {
        self.1.entry(name).or_else(|| self.0.iter().map(|entry| &entry.declaration).find(|entry| {
            entry.proofs.entry.as_ref().and_then(|contract| contract.external.as_deref()) == Some(name)
        }))
    }
}

pub(crate) fn repr(ty: s::Type) -> Repr {
    match ty {
        s::Type::Int => Repr::Int,
        s::Type::Bool | s::Type::Descriptor | s::Type::Never => Repr::Raw,
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

    /// There is no SPB 16 fallback. External contracts are checked before any machine code is published.
    pub fn new_champ(source: s::Program, external: Vec<ExternalEntry>) -> Result<Box<Self>, Error> {
        for entry in &external {
            let owns = entry.declaration.entry_abi != s::EntryAbi::Observation;
            if owns != entry.owned.is_some() || owns == entry.plain.is_some() {
                return Err(error("external implementation ABI mismatch"));
            }
        }
        let p1 = s::p1::P1Contracts::new(&source.objects).map_err(|error| Error(error.to_string()))?;
        let catalog = Catalog(&external, p1);
        s::verify_load(&source, &catalog).map_err(|error| Error(error.to_string()))?;
        let mut legacy = okkama_sparrow::Program {
            entry: source.entry.map(okkama_sparrow::FuncRef),
            ..okkama_sparrow::Program::default()
        };
        legacy.types.push(okkama_sparrow::Type::Byte);
        // This declaration view serves existing descriptors, diagnostics and heap layout consumers only.
        // No old bytecode is generated or executed.
        for object in &source.objects {
            let fields = object.layout.storage_fields.iter().enumerate().map(|(index, &ty)| {
                let id = okkama_sparrow::TypeRef(legacy.types.len() as u32);
                legacy.types.push(legacy_type(ty));
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
                legacy.types.push(legacy_type(ty));
                id
            }).collect();
            let params = function.params.iter().filter_map(|location| match location {
                s::Location::Register(id) => Some(okkama_sparrow::Register(*id)),
                _ => None,
            }).collect();
            legacy.functions.push(okkama_sparrow::Function {
                symbol: function.symbol.clone(), is_async: false,
                native: function.proofs.entry.as_ref().and_then(|entry| entry.external.clone())
                    .filter(|name| name.starts_with("runtime/")),
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
        runtime.heap = Heap::new_champ(runtime.loaded.layouts(), layouts.into_boxed_slice());
        let plans = runtime.champ_load_cleanup_plans(&source, &catalog)?;
        let mut selections = Vec::new();
        let mut entries = Vec::new();
        let mut shielded_entries = Vec::new();
        let mut implementations = Vec::new();
        for function in &source.functions {
            let mut relocated = Vec::new();
            for selection in s::static_selections(&source, function).map_err(|error| Error(error.to_string()))? {
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
        let p1 = source.functions.iter().map(|function| {
            function.proofs.entry.as_ref().and_then(|entry| entry.external.as_deref())
                .is_some_and(|name| s::ExternalContracts::entry(&catalog.1, name).is_some())
        }).collect();
        runtime.champ.machine = Some(Program {
            source: Arc::new(source), plans, selections, entries, shielded_entries,
            external: implementations, p1, native_failure,
        });
        Ok(Box::new(runtime))
    }
}

fn legacy_type(ty: s::Type) -> okkama_sparrow::Type {
    use okkama_sparrow::Type as T;
    match ty {
        s::Type::Error => T::Failure,
        s::Type::Bytes => T::Array(okkama_sparrow::TypeRef(0)),
        s::Type::Int => T::Int,
        s::Type::Bool | s::Type::Descriptor | s::Type::Never => T::Bool,
        s::Type::Str => T::Str,
        s::Type::Object(id) | s::Type::Successor(id) => T::Object(okkama_sparrow::ObjectRef(id)),
        s::Type::Range => T::Range,
        s::Type::Failure => T::Failure,
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
    let runtime = unsafe { &mut *(*ctx).runtime };
    let machine = runtime.champ.machine.as_ref().expect("Champ program");
    if io.is_null() && !machine.source.functions[function as usize].code.is_empty() {
        let code = match crate::jit::compile(runtime, function as usize) {
            Ok(code) => code,
            Err(error) => return stop(ctx, error),
        };
        let arguments = if count == 0 { &[] } else {
            unsafe { std::slice::from_raw_parts(args, count as usize) }
        };
        return unsafe { crate::jit::invoke(code, ctx, arguments) };
    }
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

#[cfg(test)]
pub(crate) fn graph_walks() -> usize {
    super::helpers::GRAPH_WALKS.with(|count| count.get())
}
