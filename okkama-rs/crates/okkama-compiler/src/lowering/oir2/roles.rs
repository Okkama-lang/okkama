//! Concrete role signatures, ordered witness targets and result adapters.
use super::{LowerError, Result, declarations::Declarations};
use crate::analysis::{Analysis, thir, ty};
use crate::oir2 as ir;

pub(super) fn declare_witnesses(declarations: &mut Declarations, analysis: &Analysis<'_>) -> Result<()> {
    let mut objects: Vec<_> = declarations.objects.iter().map(|(&id, &decl)| (id, decl)).collect();
    objects.sort_by_key(|(id, _)| *id);
    for (object, declaration) in objects {
        let info = &analysis.registry.objects[object.0];
        let span = info.decl.expect("concrete object").span;
        let mut witnesses = vec![];
        for (role, _) in &info.does {
            let role_type = declarations.ty(&ty::Ty::role(*role), span)?;
            let ir::Type::Role { decl: role_decl, .. } = *declarations.builder.module().types.get(role_type)? else {
                unreachable!("concrete role");
            };
            let ir::DeclarationKind::Role { methods, .. } = &declarations.builder.module()
                .declarations.get(role_decl)?.kind else {
                unreachable!("role declaration");
            };
            let methods = methods.clone();
            let mut slots = vec![];
            for (slot, method) in methods.iter().enumerate() {
                let source_name = &analysis.registry.roles[role.0].methods[slot].name;
                let index = info.methods.iter().position(|candidate| {
                    candidate.name == *source_name && candidate.params.len() == method.signature.params.len()
                        && candidate.params.iter().zip(&method.signature.params)
                            .all(|((_, ty), lowered)| declarations.types.get(ty) == Some(lowered))
                }).ok_or_else(|| LowerError::invalid(span, format!("unknown role slot {}", method.name)))?;
                let target = declarations.functions.get(&thir::BodyOwner::Method { object, method: index })
                    .copied().ok_or_else(|| {
                        LowerError::unsupported(span, "role slot has no reachable implementation")
                    })?;
                let function = declarations.builder.module().functions.get(target)?;
                // A slot names an implementation of its own ABI. An async action never has it: it is reached through
                // its creation thunk, an adapter with the slot's signature that returns its cold Future (ruling PR-D).
                let asynchronous = function.attrs.contains(&ir::FunctionAttr::Async);
                if !asynchronous && function.signature.results == method.signature.results
                    && function.attrs.contains(&ir::FunctionAttr::Method(declaration)) {
                    slots.push(target);
                    continue;
                }
                let mut signature = method.signature.clone();
                signature.params.insert(0, declarations.types[&ty::Ty::object(object)]);
                let symbol = format!("{}.witness.{}.slot{slot}",
                    declarations.builder.module().declarations.get(declaration)?.name,
                    declarations.builder.module().declarations.get(role_decl)?.name);
                let adapter = adapter(declarations, declaration, target, signature, symbol, span)?;
                slots.push(adapter);
            }
            witnesses.push(ir::Witness { role: role_type, slots });
        }
        if let ir::DeclarationKind::Object { witnesses: destination, .. } = &mut declarations.builder.raw_mut()
            .declarations.get_mut(declaration)?.kind {
            *destination = witnesses;
        }
    }
    Ok(())
}

/// A synchronous function of `signature` (receiver first) that calls `target` with its parameters and returns its
/// result, taking a successor and wrapping into a role as the signature asks. It is a witness slot's adapter, and
/// the creation thunk of an async action (ruling PR-D): the call of an async `target` creates its cold Future,
/// which own rewrites to `fut.new` and returns through `cache.rehome … target caller`. An external `target` is
/// called through its own call form and summary.
pub(super) fn adapter(declarations: &mut Declarations, declaration: ir::DeclId, target: ir::FuncId,
    signature: ir::Signature, symbol: String, span: crate::source::Span) -> Result<ir::FuncId> {
    let function = declarations.builder.module().functions.get(target)?;
    let native = function.attrs.iter().any(|attr| matches!(attr, ir::FunctionAttr::Extern(_)));
    let intrinsic = function.attrs.iter().any(|attr| matches!(attr, ir::FunctionAttr::Intrinsic { .. }));
    let summary = function.summary;
    let result = signature.results.first().copied();
    let adapter = declarations.builder.declare_function(ir::Function {
        symbol, signature, attrs: vec![ir::FunctionAttr::Method(declaration)], type_params: vec![],
        summary: None, async_creation_summary: None, async_body_summary: None, body: None, loc: None,
    })?;
    let root = declarations.builder.declare_scope(ir::ScopeDecl {
        function: adapter, parent: None, kind: ir::ScopeKind::Action, loc: None,
    })?;
    let mut builder = declarations.builder.function_builder(adapter, root)?;
    let entry = builder.entry();
    let failure_ty = builder.intern_type(ir::Type::Service(ir::ServiceType::Failure))?;
    let failure = builder.create_block(root, &[ir::ParamSpec::new(failure_ty)])?;
    builder.switch_to(failure)?;
    builder.terminate(ir::TermKind::Raise(builder.block_params(failure)?[0]))?;
    builder.switch_to(entry)?;
    builder.set_unwind(Some(ir::BlockUnwind {
        target: ir::Target { block: failure, args: vec![] },
    }))?;
    let args = builder.block_params(entry)?.iter().map(|&value| ir::CallArgument {
        value, convention: ir::OperandConvention::Borrow,
    }).collect();
    // An external is called with its catalogue summary, as `BodyLowerer::call` does; a body's call has none in High.
    let call = ir::CallSite { summary: summary.filter(|_| native || intrinsic), ..call_site(args) };
    let call = if native {
        ir::CallOp::Native { function: target, call }
    } else if intrinsic {
        ir::CallOp::Intrinsic { function: target, call }
    } else {
        ir::CallOp::Direct { function: target, call }
    };
    let values = builder.emit_schema(ir::Operation::Call(call))?;
    record_call_effect(&mut builder, entry)?;
    let value = match (values.first().copied(), result) {
        (Some(value), Some(ty)) => {
            let input = builder.body().values.get(value)?.ty;
            let value = if matches!(builder.module().types.get(input)?, ir::Type::Successor(_))
                && !matches!(builder.module().types.get(ty)?, ir::Type::Successor(_)) {
                let ir::Type::Successor(inner) = *builder.module().types.get(input)? else {
                    unreachable!("successor result");
                };
                builder.emit_schema(ir::Operation::Core(ir::CoreOp::SuccTake { value, ty: inner }))?[0]
            } else {
                value
            };
            if builder.body().values.get(value)?.ty == ty {
                Some(value)
            } else {
                Some(builder.emit_schema(ir::Operation::Core(ir::CoreOp::RoleWrap { value, ty }))?[0])
            }
        }
        (None, None) => None,
        _ => return Err(LowerError::invalid(span, "role adapter result arity")),
    };
    finish_adapter(&mut builder, root, entry, value)?;
    let body = builder.finish()?;
    declarations.builder.define_function(adapter, body)?;
    Ok(adapter)
}

/// The creation thunks of the async actions a program binds as values (ruling PR-D item 3): a bound async action is
/// an `Fn` returning the Future, so `fn.bind` names a synchronous function of the receiver and the parameters that
/// creates the cold Future. One thunk per bound async method, keyed by the method's function.
pub(super) fn declare_thunks(declarations: &mut Declarations, inventory: &super::callables::Inventory)
    -> Result<std::collections::HashMap<ir::FuncId, ir::FuncId>> {
    let mut thunks = std::collections::HashMap::new();
    for bound in &inventory.bound {
        let super::callables::BoundTarget::Method { object, method } = bound.target else { continue };
        let Some(&target) = declarations.functions.get(&thir::BodyOwner::Method { object, method }) else {
            continue;
        };
        let function = declarations.builder.module().functions.get(target)?;
        if !function.attrs.contains(&ir::FunctionAttr::Async) || thunks.contains_key(&target) {
            continue;
        }
        let mut signature = function.signature.clone();
        let symbol = format!("{}.thunk", function.symbol);
        let outcome = signature.results.first().copied();
        signature.results = vec![declarations.builder.intern_type(ir::Type::Future(outcome))?];
        let declaration = declarations.objects[&object];
        let thunk = adapter(declarations, declaration, target, signature, symbol, bound.span)?;
        thunks.insert(target, thunk);
    }
    Ok(thunks)
}

pub(super) fn call_site(args: Vec<ir::CallArgument>) -> ir::CallSite {
    ir::CallSite {
        args, packet: None, deps: vec![], descriptors: vec![], type_args: vec![], summary: None,
        cleanup: ir::CallCleanupMode::Ordinary,
    }
}

pub(super) fn record_call_effect(builder: &mut ir::FunctionBuilder<'_>, block: ir::BlockId) -> Result<()> {
    let instruction = *builder.body().blocks.get(block)?.insts.last().expect("emitted call");
    builder.add_high_effect(ir::HighEffectRecord {
        site: ir::HighOperationSite::Instruction(instruction),
        effects: ir::HighEffectEnvelope {
            failures: vec![ir::FailureKind::Thrown, ir::FailureKind::Program, ir::FailureKind::Panic,
                ir::FailureKind::Native, ir::FailureKind::Cancelled],
            may_suspend: true,
        },
    })?;
    Ok(())
}

pub(super) fn finish_adapter(builder: &mut ir::FunctionBuilder<'_>, root: ir::ScopeId,
    block: ir::BlockId, value: Option<ir::ValueId>) -> Result<()> {
    let boundary = builder.declare_boundary(ir::Boundary { scope: root, kind: ir::BoundaryKind::Lexical })?;
    let selection = builder.declare_selection(match value {
        Some(value) => ir::SelectionExpr::Value(value),
        None => ir::SelectionExpr::Union(vec![]),
    })?;
    if let Some(value) = value {
        builder.emit(ir::Operation::Lexical(ir::LexicalOp::Want { value }), &[])?;
    }
    builder.emit(ir::Operation::Exit(ir::ExitOp::Prepare {
        exit: ir::PreparedExit {
            kind: ir::ExitKind::Return, value, packet: None, deps: None, selection, boundary,
        },
    }), &[])?;
    if let Some(value) = value {
        let prepared = builder.body().blocks.get(block)?.insts.last().copied();
        builder.add_high_result(ir::HighResultExit {
            block, edge: ir::HighEdgeSelector::Normal(0), value, demand: ir::ResultDemand::Want, prepared,
        })?;
    }
    builder.terminate(ir::TermKind::Ret { values: value.into_iter().collect() })?;
    Ok(())
}

/// The ByteSource bridges: a string (its UTF-8 bytes, the C3 builtin golden) and a byte
/// array (the bytes themselves, for `out <- @byte(...)`). Other adapters keep a diagnostic.
/// The adapters are ordinary lowered objects, like the C3 builtin evidence: neither own nor
/// the backend may treat them as runtime services.
pub(super) fn declare_builtins(declarations: &mut Declarations, analysis: &Analysis<'_>) -> Result<()> {
    super::builtin_roles::declare(declarations, analysis)?;
    let Some(role) = analysis.registry.standard.byte_source else {
        return Ok(());
    };
    for (input, name) in [(ty::Ty::Str, "core.StringSource"), (ty::Ty::array(ty::Ty::Byte), "core.BytesSource")] {
        let mut requested = false;
        for body in &analysis.bodies {
            super::walk::block(&body.block, &mut |expr| {
                if let thir::ExprKind::Coerce { coercion: thir::Coercion::RoleWrap, value } = &expr.kind {
                    requested |= value.ty.as_ref() == Some(&input) && expr.ty == Some(ty::Ty::role(role));
                }
            });
        }
        if requested {
            byte_source(declarations, role, &input, name)?;
        }
    }
    Ok(())
}

fn byte_source(declarations: &mut Declarations, role: ty::RoleId, input: &ty::Ty, name: &str) -> Result<()> {
    let span = crate::source::Span::default();
    let role_type = declarations.ty(&ty::Ty::role(role), span)?;
    let value_type = declarations.ty(input, span)?;
    let bytes = declarations.ty(&ty::Ty::array(ty::Ty::Byte), span)?;
    let declaration = declarations.builder.declare(ir::Declaration {
        name: name.into(), type_params: vec![], loc: None,
        kind: ir::DeclarationKind::Object {
            fields: vec![ir::Field { name: "value".into(), ty: value_type, default: None, once: false }],
            witnesses: vec![], destroy: None, builtin: false,
        },
    })?;
    let object = declarations.builder.intern_type(ir::Type::Object { decl: declaration, args: vec![] })?;
    let function = declarations.builder.declare_function(ir::Function {
        symbol: format!("{name}.bytes"),
        signature: ir::Signature {
            params: vec![object], results: vec![bytes], ownership_abi: ir::OwnershipAbi::Unelaborated,
        },
        attrs: vec![ir::FunctionAttr::Method(declaration)], type_params: vec![], summary: None,
        async_creation_summary: None, async_body_summary: None, body: None, loc: None,
    })?;
    let root = declarations.builder.declare_scope(ir::ScopeDecl {
        function, parent: None, kind: ir::ScopeKind::Action, loc: None,
    })?;
    let mut builder = declarations.builder.function_builder(function, root)?;
    let entry = builder.entry();
    let receiver = builder.block_params(entry)?[0];
    let value = builder.emit_schema(ir::Operation::Core(ir::CoreOp::ObjGet {
        object: receiver, field: ir::FieldRef { declaration, index: 0 },
    }))?[0];
    let bytes = if *input == ty::Ty::Str {
        builder.emit_schema(ir::Operation::Core(ir::CoreOp::StrUtf8 { value }))?[0]
    } else {
        value
    };
    finish_adapter(&mut builder, root, entry, Some(bytes))?;
    let body = builder.finish()?;
    declarations.builder.define_function(function, body)?;
    if let ir::DeclarationKind::Object { witnesses, .. } = &mut declarations.builder.raw_mut()
        .declarations.get_mut(declaration)?.kind {
        witnesses.push(ir::Witness { role: role_type, slots: vec![function] });
    }
    let constructor = declarations.builder.declare_function(ir::Function {
        symbol: format!("{name}.new"),
        signature: ir::Signature {
            params: vec![value_type], results: vec![object], ownership_abi: ir::OwnershipAbi::Unelaborated,
        },
        attrs: vec![ir::FunctionAttr::Ctor(declaration)], type_params: vec![], summary: None,
        async_creation_summary: None, async_body_summary: None, body: None, loc: None,
    })?;
    let root = declarations.builder.declare_scope(ir::ScopeDecl {
        function: constructor, parent: None, kind: ir::ScopeKind::Action, loc: None,
    })?;
    let mut builder = declarations.builder.function_builder(constructor, root)?;
    let entry = builder.entry();
    let failure_ty = builder.intern_type(ir::Type::Service(ir::ServiceType::Failure))?;
    let failure = builder.create_block(root, &[ir::ParamSpec::new(failure_ty)])?;
    builder.switch_to(failure)?;
    builder.terminate(ir::TermKind::Raise(builder.block_params(failure)?[0]))?;
    builder.switch_to(entry)?;
    builder.set_unwind(Some(ir::BlockUnwind { target: ir::Target { block: failure, args: vec![] } }))?;
    let value = builder.block_params(entry)?[0];
    let value = builder.emit_schema(ir::Operation::Core(ir::CoreOp::ObjNew {
        declaration, type_args: vec![], fields: vec![value], packet: None,
        construction: ir::ConstructionStatus::Complete,
    }))?[0];
    finish_adapter(&mut builder, root, entry, Some(value))?;
    let body = builder.finish()?;
    declarations.builder.define_function(constructor, body)?;
    declarations.builtin_adapters.insert((value_type, role_type), constructor);
    Ok(())
}
