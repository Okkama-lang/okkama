//! The helpers of the timeout drivers (#810, G1 step 2): `Within.outcome` and `Before.outcome` read the clock and the
//! limit through program functions, which Tomato's driver calls (`timeout_start`). Each helper is a synchronous
//! function named `<entry>.helper<index>` (`spb17::drivers::helper_symbol`), synthesized here from the std
//! declarations the timeout object's fields name, in the order of the row's `helpers`:
//!
//! - `Seconds` (`Duration -> rat`) calls the computed field `Span::seconds` of `Duration`;
//! - `Wait` (`(Timing, Duration) -> Future<void>`) calls the async role slot `wait`, which returns the cold timer
//!   (ruling PR-D); the Future's duty leaves in the output packet, which the runtime bridge puts into the driver's
//!   inputs (memo §6.4, I-G5);
//! - `Now` (`Timing -> MonotonicInstant`) calls the role slot `monotonic_instant`;
//! - `Minus` (`(MonotonicInstant, MonotonicInstant) -> Duration`) calls `MonotonicInstant.minus`.
//!
//! - `Wrap` (`array of T -> Sequencing<T>`, AllFutures' result, G1 step 3) calls the constructor of the array
//!   adapter that witnesses the entry's result role and wraps the adapter as that role, as a program's own
//!   conversion of an array to `Sequencing<T>` does (ruling Q1 of the steps 3–4 review).
//!
//! The functions are listed in the intrinsic's `FunctionAttr::Intrinsic.helpers`; the SPB 17 loader checks their
//! shape by symbol (`HOLDER_HELPER`).
use super::declarations::Declarations;
use super::roles::{call_site, finish_adapter, record_call_effect};
use super::{LowerError, Result};
use crate::oir2 as ir;
use okkama_sparrow::spb17::drivers::{self as catalogue, Helper};

/// What a helper calls.
enum Target {
    Direct(ir::FuncId),
    Role { role: ir::DeclId, slot: u32 },
    /// An adapter's constructor, whose object the helper returns as the role `ty`.
    Wrap { constructor: ir::FuncId, ty: ir::TypeId },
}

pub(super) fn declare_helpers(declarations: &mut Declarations, analysis: &crate::analysis::Analysis<'_>) -> Result<()> {
    let span = crate::source::Span::default();
    let entries: Vec<_> = declarations.builder.module().functions.iter().filter_map(|(id, function)| {
        function.attrs.iter().find_map(|attr| match attr {
            ir::FunctionAttr::Intrinsic { name, .. } => catalogue::ready_holder(name)
                .filter(|row| !row.helpers.is_empty()).map(|row| (id, name.clone(), row.helpers)),
            _ => None,
        })
    }).collect();
    for (entry, name, kinds) in entries {
        let module = declarations.builder.module();
        let result = module.functions.get(entry)?.signature.results.first().copied();
        let receiver = *module.functions.get(entry)?.signature.params.first()
            .ok_or_else(|| LowerError::invalid(span, format!("{name} has no receiver")))?;
        let ir::Type::Object { decl, .. } = *module.types.get(receiver)? else {
            return Err(LowerError::invalid(span, format!("{name}'s receiver is not an object")));
        };
        let ir::DeclarationKind::Object { fields, .. } = &module.declarations.get(decl)?.kind else {
            return Err(LowerError::invalid(span, format!("{name}'s receiver is not an object")));
        };
        // Within: origin, limit, clock. Before: origin, deadline, clock. AllFutures: origins.
        let timeout = match fields.as_slice() {
            [_, limit, clock] => Some((limit.ty, clock.ty)),
            _ => None,
        };
        let timed = || timeout.ok_or_else(|| LowerError::invalid(span, format!("{name}'s receiver has not three fields")));
        let mut helpers = Vec::new();
        for (index, kind) in kinds.iter().enumerate() {
            let symbol = catalogue::helper_symbol(&name, index);
            let helper = match kind {
                Helper::Seconds => {
                    let (limit, _) = timed()?;
                    let duration = duration_type(declarations, &name, limit)?;
                    let target = seconds(declarations, duration, &name)?;
                    let rat = declarations.builder.intern_type(ir::Type::Rat)?;
                    helper(declarations, symbol, vec![duration], rat, Target::Direct(target))?
                }
                Helper::Wait => {
                    let (limit, clock) = timed()?;
                    let duration = duration_type(declarations, &name, limit)?;
                    let (role, slot, result) = role_slot(declarations, clock, "wait", &name)?;
                    helper(declarations, symbol, vec![clock, duration], result, Target::Role { role, slot })?
                }
                Helper::Now => {
                    let (_, clock) = timed()?;
                    let (role, slot, result) = role_slot(declarations, clock, "monotonic_instant", &name)?;
                    helper(declarations, symbol, vec![clock], result, Target::Role { role, slot })?
                }
                Helper::Minus => {
                    let (limit, _) = timed()?;
                    let (target, result) = minus(declarations, limit, &name)?;
                    helper(declarations, symbol, vec![limit, limit], result, Target::Direct(target))?
                }
                Helper::Wrap => {
                    // The result role `Sequencing<T>`; T is its `item`'s result.
                    let role = result.ok_or_else(|| LowerError::invalid(span, format!("{name} has no result")))?;
                    let (_, _, element) = role_slot(declarations, role, "item", &name)?;
                    let array = declarations.builder.intern_type(ir::Type::Array(element))?;
                    let constructor = super::builtin_roles::array_adapter(declarations, analysis, array, role)?
                        .ok_or_else(|| LowerError::invalid(span, format!("{name} has no array adapter")))?;
                    helper(declarations, symbol, vec![array], role, Target::Wrap { constructor, ty: role })?
                }
            };
            helpers.push(helper);
        }
        for attr in &mut declarations.builder.raw_mut().functions.get_mut(entry)?.attrs {
            if let ir::FunctionAttr::Intrinsic { helpers: destination, .. } = attr {
                *destination = helpers.clone();
            }
        }
    }
    Ok(())
}

/// The Duration of a Within's limit; a Before's limit is its deadline, whose `minus` gives the Duration.
fn duration_type(declarations: &Declarations, name: &str, limit: ir::TypeId) -> Result<ir::TypeId> {
    if declaration_name(declarations, limit).as_deref() == Some(catalogue::DURATION) {
        return Ok(limit);
    }
    let (_, result) = minus(declarations, limit, name)?;
    Ok(result)
}

fn declaration_name(declarations: &Declarations, ty: ir::TypeId) -> Option<String> {
    let module = declarations.builder.module();
    match module.types.get(ty).ok()? {
        ir::Type::Object { decl, .. } | ir::Type::Role { decl, .. } => {
            module.declarations.get(*decl).ok().map(|declaration| declaration.name.clone())
        }
        _ => None,
    }
}

/// The computed field `Span::seconds` of `Duration`: the function of the cap's field, which takes the Duration.
fn seconds(declarations: &Declarations, duration: ir::TypeId, name: &str) -> Result<ir::FuncId> {
    let module = declarations.builder.module();
    let wanted = format!("{}.Span", catalogue::DURATION);
    module.declarations.iter().find_map(|(_, declaration)| match &declaration.kind {
        ir::DeclarationKind::Cap { owner, fields } if *owner == duration && declaration.name == wanted => {
            fields.iter().find(|field| field.name == "seconds").map(|field| field.function)
        }
        _ => None,
    }).ok_or_else(|| LowerError::invalid(crate::source::Span::default(),
        format!("{name} needs Duration's Span::seconds")))
}

/// `MonotonicInstant.minus(MonotonicInstant) -> Duration`.
fn minus(declarations: &Declarations, instant: ir::TypeId, name: &str) -> Result<(ir::FuncId, ir::TypeId)> {
    let module = declarations.builder.module();
    let ir::Type::Object { decl, .. } = *module.types.get(instant)? else {
        return Err(LowerError::invalid(crate::source::Span::default(), format!("{name}'s deadline is no object")));
    };
    let prefix = format!("{}.minus", catalogue::MONOTONIC_INSTANT);
    module.functions.iter().find_map(|(id, function)| {
        let method = function.attrs.contains(&ir::FunctionAttr::Method(decl));
        let duration = function.signature.results.first().copied()
            .filter(|&result| declaration_name(declarations, result).as_deref() == Some(catalogue::DURATION));
        (method && function.symbol.starts_with(&prefix) && function.signature.params == [instant, instant])
            .then_some(()).and(duration).map(|duration| (id, duration))
    }).ok_or_else(|| LowerError::invalid(crate::source::Span::default(),
        format!("{name} needs MonotonicInstant.minus(MonotonicInstant)")))
}

/// The slot `method` of the role `clock` is: its declaration, index and result type.
fn role_slot(declarations: &Declarations, clock: ir::TypeId, method: &str, name: &str)
    -> Result<(ir::DeclId, u32, ir::TypeId)> {
    let module = declarations.builder.module();
    let invalid = || LowerError::invalid(crate::source::Span::default(), format!("{name} needs Timing.{method}"));
    let ir::Type::Role { decl, .. } = *module.types.get(clock)? else { return Err(invalid()) };
    let ir::DeclarationKind::Role { methods, .. } = &module.declarations.get(decl)?.kind else {
        return Err(invalid());
    };
    let slot = methods.iter().position(|candidate| candidate.name == method).ok_or_else(invalid)?;
    let result = *methods[slot].signature.results.first().ok_or_else(invalid)?;
    Ok((decl, slot as u32, result))
}

/// A synchronous function of `params` that calls `target` with them and returns its one result.
fn helper(declarations: &mut Declarations, symbol: String, params: Vec<ir::TypeId>, result: ir::TypeId,
    target: Target) -> Result<ir::FuncId> {
    // #824: a clock role no object witnesses has no value (`role.wrap` needs a witness), so the helper is never
    // called; its slot call joins an empty set of implementations, as an ordinary role call does (#757).
    let uninhabited = matches!(target, Target::Role { .. })
        && !declarations.builder.module().declarations.iter().any(|(_, declaration)| matches!(&declaration.kind,
            ir::DeclarationKind::Object { witnesses, .. } if witnesses.iter().any(|witness| witness.role == params[0])));
    let signature = ir::Signature { params, results: vec![result], ownership_abi: ir::OwnershipAbi::Unelaborated };
    let function = declarations.builder.declare_function(ir::Function {
        symbol, signature, attrs: vec![], type_params: vec![],
        summary: None, async_creation_summary: None, async_body_summary: None, body: None, loc: None,
    })?;
    let root = declarations.builder.declare_scope(ir::ScopeDecl {
        function, parent: None, kind: ir::ScopeKind::Action, loc: None,
    })?;
    let mut builder = declarations.builder.function_builder(function, root)?;
    let entry = builder.entry();
    if uninhabited {
        builder.terminate(ir::TermKind::Unreachable)?;
        let body = builder.finish()?;
        declarations.builder.define_function(function, body)?;
        return Ok(function);
    }
    let failure_ty = builder.intern_type(ir::Type::Service(ir::ServiceType::Failure))?;
    let failure = builder.create_block(root, &[ir::ParamSpec::new(failure_ty)])?;
    builder.switch_to(failure)?;
    builder.terminate(ir::TermKind::Raise(builder.block_params(failure)?[0]))?;
    builder.switch_to(entry)?;
    builder.set_unwind(Some(ir::BlockUnwind { target: ir::Target { block: failure, args: vec![] } }))?;
    let mut args: Vec<_> = builder.block_params(entry)?.iter().map(|&value| ir::CallArgument {
        value, convention: ir::OperandConvention::Borrow,
    }).collect();
    let mut wrap = None;
    let call = match target {
        Target::Direct(function) => ir::CallOp::Direct { function, call: call_site(args) },
        Target::Role { role, slot } => {
            let receiver = args.remove(0);
            ir::CallOp::Role { receiver, role, slot, call: call_site(args) }
        }
        Target::Wrap { constructor, ty } => {
            wrap = Some(ty);
            ir::CallOp::Direct { function: constructor, call: call_site(args) }
        }
    };
    let values = builder.emit_schema(ir::Operation::Call(call))?;
    record_call_effect(&mut builder, entry)?;
    let mut value = values.first().copied();
    if let (Some(ty), Some(adapter)) = (wrap, value) {
        value = builder.emit_schema(ir::Operation::Core(ir::CoreOp::RoleWrap { value: adapter, ty }))?.first().copied();
    }
    finish_adapter(&mut builder, root, entry, value)?;
    let body = builder.finish()?;
    declarations.builder.define_function(function, body)?;
    Ok(function)
}
