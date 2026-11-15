//! Concrete builtin evidence uses C3 objects/witnesses, never an external body.
use super::{LowerError, Result, declarations::Declarations};
use crate::analysis::{Analysis, thir};
use crate::oir2 as ir;

pub(super) fn declare(declarations: &mut Declarations, analysis: &Analysis<'_>) -> Result<()> {
    let mut pairs = vec![];
    for body in &analysis.bodies {
        super::walk::block(&body.block, &mut |expr| {
            if let thir::ExprKind::Coerce { coercion: thir::Coercion::RoleWrap, value } = &expr.kind
                && let (Some(source), Some(target)) = (&value.ty, &expr.ty)
                && let (Some(&source), Some(&target))
                    = (declarations.types.get(source), declarations.types.get(target)) {
                pairs.push((source, target));
            }
        });
    }
    let mut cursors = vec![];
    for body in &analysis.bodies {
        super::walk::block(&body.block, &mut |expr| {
            let cursor = thir::Builtin::ArrayCursor;
            if let thir::ExprKind::Call { callee: thir::Callee::Builtin { receiver, op }, .. } = &expr.kind
                && *op == cursor
                && let (Some(source), Some(target)) = (&receiver.ty, &expr.ty)
                && let (Some(&source), Some(&target))
                    = (declarations.types.get(source), declarations.types.get(target)) {
                cursors.push((source, target));
            }
        });
    }
    pairs.sort();
    pairs.dedup();
    cursors.sort();
    cursors.dedup();
    for (input, role) in pairs {
        let ir::Type::Role { decl, .. } = *declarations.builder.module().types.get(role)? else {
            continue;
        };
        let ir::DeclarationKind::Role { methods, .. } = &declarations.builder.module().declarations.get(decl)?.kind
            else {
                continue;
            };
        let methods = methods.clone();
        if matches!(declarations.builder.module().types.get(input)?, ir::Type::Array(_))
            && methods.len() == 1 && methods[0].name == "cursor" {
            array(declarations, input, role, methods[0].signature.results[0])?;
        } else if let Some(kind) = super::sorting::scalar_kind(declarations.builder.module().types.get(input)?)
            && methods.len() == 1 && methods[0].name == "order" {
            scalar(declarations, input, role, kind, methods[0].signature.results[0])?;
        }
    }
    // `xs.cursor()`: the same array cursor evidence, reached without the Iterating adapter.
    for (input, cursor_role) in cursors {
        if matches!(declarations.builder.module().types.get(input)?, ir::Type::Array(_)) {
            cursor(declarations, input, cursor_role)?;
        }
    }
    Ok(())
}

fn object(declarations: &mut Declarations, name: String, fields: Vec<ir::TypeId>)
    -> Result<(ir::DeclId, ir::TypeId)> {
    let fields = fields.into_iter().enumerate().map(|(index, ty)| ir::Field {
        name: format!("value{index}"), ty, default: None, once: false,
    }).collect();
    let decl = declarations.builder.declare(ir::Declaration {
        name, type_params: vec![], loc: None,
        kind: ir::DeclarationKind::Object { fields, witnesses: vec![], destroy: None, builtin: false },
    })?;
    let ty = declarations.builder.intern_type(ir::Type::Object { decl, args: vec![] })?;
    Ok((decl, ty))
}

fn function(declarations: &mut Declarations, owner: ir::DeclId, name: &str,
    params: Vec<ir::TypeId>, result: ir::TypeId, constructor: bool,
    lower: impl FnOnce(&mut ir::FunctionBuilder<'_>, ir::ScopeId) -> Result<ir::ValueId>) -> Result<ir::FuncId> {
    let symbol = format!("{}.{}", declarations.builder.module().declarations.get(owner)?.name, name);
    let attribute = if constructor { ir::FunctionAttr::Ctor(owner) } else { ir::FunctionAttr::Method(owner) };
    let id = declarations.builder.declare_function(ir::Function {
        symbol,
        signature: ir::Signature { params, results: vec![result], ownership_abi: ir::OwnershipAbi::Unelaborated },
        attrs: vec![attribute], type_params: vec![], summary: None,
        async_creation_summary: None, async_body_summary: None, body: None, loc: None,
    })?;
    let root = declarations.builder.declare_scope(ir::ScopeDecl {
        function: id, parent: None, kind: ir::ScopeKind::Action, loc: None,
    })?;
    let mut builder = declarations.builder.function_builder(id, root)?;
    let entry = builder.entry();
    let failure = builder.intern_type(ir::Type::Service(ir::ServiceType::Failure))?;
    let unwind = builder.create_block(root, &[ir::ParamSpec::new(failure)])?;
    builder.switch_to(unwind)?;
    builder.terminate(ir::TermKind::Raise(builder.block_params(unwind)?[0]))?;
    builder.switch_to(entry)?;
    builder.set_unwind(Some(ir::BlockUnwind { target: ir::Target { block: unwind, args: vec![] } }))?;
    let value = lower(&mut builder, root)?;
    let block = builder.current_block().expect("open adapter result");
    super::roles::finish_adapter(&mut builder, root, block, Some(value))?;
    let body = builder.finish()?;
    declarations.builder.define_function(id, body)?;
    Ok(id)
}

fn switch(builder: &mut ir::FunctionBuilder<'_>, block: ir::BlockId) -> Result<()> {
    let unwind = builder.body().blocks.get(builder.entry())?.unwind.clone();
    builder.switch_to(block)?;
    builder.set_unwind(unwind)?;
    Ok(())
}

fn core(builder: &mut ir::FunctionBuilder<'_>, op: ir::CoreOp) -> Result<ir::ValueId> {
    Ok(builder.emit_schema(ir::Operation::Core(op))?[0])
}

fn get(builder: &mut ir::FunctionBuilder<'_>, object: ir::ValueId, owner: ir::DeclId, index: u32)
    -> Result<ir::ValueId> {
    core(builder, ir::CoreOp::ObjGet { object, field: ir::FieldRef { declaration: owner, index } })
}

fn construct(builder: &mut ir::FunctionBuilder<'_>, owner: ir::DeclId, fields: Vec<ir::ValueId>)
    -> Result<ir::ValueId> {
    core(builder, ir::CoreOp::ObjNew {
        declaration: owner, type_args: vec![], fields, packet: None, construction: ir::ConstructionStatus::Complete,
    })
}

fn call(builder: &mut ir::FunctionBuilder<'_>, target: ir::FuncId, args: Vec<ir::ValueId>)
    -> Result<ir::ValueId> {
    let args = args.into_iter().map(|value| ir::CallArgument {
        value, convention: ir::OperandConvention::Borrow,
    }).collect();
    let value = builder.emit_schema(ir::Operation::Call(ir::CallOp::Direct {
        function: target, call: super::roles::call_site(args),
    }))?[0];
    let block = builder.current_block().expect("open adapter call");
    super::roles::record_call_effect(builder, block)?;
    Ok(value)
}

fn witness(declarations: &mut Declarations, owner: ir::DeclId, role: ir::TypeId, slots: Vec<ir::FuncId>)
    -> Result<()> {
    let ir::DeclarationKind::Object { witnesses, .. } = &mut declarations.builder.raw_mut()
        .declarations.get_mut(owner)?.kind else {
            unreachable!("builtin object");
        };
    witnesses.push(ir::Witness { role, slots });
    Ok(())
}

fn array(declarations: &mut Declarations, input: ir::TypeId, role: ir::TypeId, cursor_role: ir::TypeId)
    -> Result<()> {
    let ctor = cursor(declarations, input, cursor_role)?;
    let sequence_name = format!("builtin.ArraySequence.t{}", input.as_u32());
    let (sequence, sequence_ty) = object(declarations, sequence_name, vec![input])?;
    let ctor_sequence = function(declarations, sequence, "new", vec![input], sequence_ty, true, |builder, _| {
        construct(builder, sequence, builder.block_params(builder.entry())?.to_vec())
    })?;
    let open = function(declarations, sequence, "cursor", vec![sequence_ty], cursor_role, false, |builder, _| {
        let receiver = builder.block_params(builder.entry())?[0];
        let array = get(builder, receiver, sequence, 0)?;
        let zero = builder.emit_schema(ir::Operation::Const(ir::Literal::Int("0".into())))?[0];
        let value = call(builder, ctor, vec![array, zero])?;
        core(builder, ir::CoreOp::RoleWrap { value, ty: cursor_role })
    })?;
    witness(declarations, sequence, role, vec![open])?;
    declarations.builtin_adapters.insert((input, role), ctor_sequence);
    Ok(())
}

/// The array cursor object (`items`, `at`) for `cursor_role`, once per array type; its
/// constructor `(items, at)` is the builtin adapter of `(array, cursor role)`.
fn cursor(declarations: &mut Declarations, input: ir::TypeId, cursor_role: ir::TypeId) -> Result<ir::FuncId> {
    if let Some(&ctor) = declarations.builtin_adapters.get(&(input, cursor_role)) {
        return Ok(ctor);
    }
    let int = declarations.builder.intern_type(ir::Type::Int)?;
    let bool_ty = declarations.builder.intern_type(ir::Type::Bool)?;
    let ir::Type::Array(element) = *declarations.builder.module().types.get(input)? else {
        unreachable!("array evidence");
    };
    let cursor_name = format!("builtin.ArrayCursor.t{}", input.as_u32());
    let (cursor, cursor_ty) = object(declarations, cursor_name, vec![input, int])?;
    let ctor = function(declarations, cursor, "new", vec![input, int], cursor_ty, true, |builder, _| {
        construct(builder, cursor, builder.block_params(builder.entry())?.to_vec())
    })?;
    let available = function(declarations, cursor, "available", vec![cursor_ty], bool_ty, false, |builder, _| {
        let receiver = builder.block_params(builder.entry())?[0];
        let array = get(builder, receiver, cursor, 0)?;
        let at = get(builder, receiver, cursor, 1)?;
        let size = builder.emit_schema(ir::Operation::Collection(ir::CollectionOp::ArrayLen { array }))?[0];
        core(builder, ir::CoreOp::IntLt { left: at, right: size })
    })?;
    let ended = declarations.builder.module().declarations.iter().find_map(|(id, decl)| {
        (decl.name == "collections.iteration.IterationEnded").then_some(id)
    }).ok_or_else(|| LowerError::unsupported(crate::source::Span::default(), "missing IterationEnded declaration"))?;
    let mut slots = vec![available];
    for (name, result) in [("item", element), ("successor", cursor_role)] {
        let target = function(declarations, cursor, name, vec![cursor_ty], result, false, |builder, root| {
            let receiver = builder.block_params(builder.entry())?[0];
            let condition = call(builder, available, vec![receiver])?;
            let go = builder.create_block(root, &[])?;
            let stop = builder.create_block(root, &[])?;
            builder.terminate(ir::TermKind::Cond {
                condition,
                yes: ir::Target { block: go, args: vec![] },
                no: ir::Target { block: stop, args: vec![] },
            })?;
            switch(builder, stop)?;
            let error = core(builder, ir::CoreOp::EnumCase { declaration: ended, case: 0 })?;
            builder.terminate(ir::TermKind::Throw(error))?;
            switch(builder, go)?;
            let array = get(builder, receiver, cursor, 0)?;
            let at = get(builder, receiver, cursor, 1)?;
            if name == "item" {
                Ok(builder.emit_schema(ir::Operation::Collection(ir::CollectionOp::ArrayGet { array, index: at }))?[0])
            } else {
                let one = builder.emit_schema(ir::Operation::Const(ir::Literal::Int("1".into())))?[0];
                let at = core(builder, ir::CoreOp::IntAdd { left: at, right: one })?;
                let value = call(builder, ctor, vec![array, at])?;
                core(builder, ir::CoreOp::RoleWrap { value, ty: cursor_role })
            }
        })?;
        slots.push(target);
    }
    witness(declarations, cursor, cursor_role, slots)?;
    declarations.builtin_adapters.insert((input, cursor_role), ctor);
    Ok(ctor)
}

fn scalar(declarations: &mut Declarations, input: ir::TypeId, role: ir::TypeId,
    kind: thir::CompareKind, order: ir::TypeId) -> Result<()> {
    let (owner, ty) = object(declarations, format!("builtin.ScalarOrder.t{}", input.as_u32()), vec![input])?;
    let ctor = function(declarations, owner, "new", vec![input], ty, true, |builder, _| {
        construct(builder, owner, builder.block_params(builder.entry())?.to_vec())
    })?;
    let target = function(declarations, owner, "order", vec![ty, input], order, false, |builder, root| {
        let params = builder.block_params(builder.entry())?.to_vec();
        let left = get(builder, params[0], owner, 0)?;
        let ir::Type::Enum(declaration) = *builder.module().types.get(order)? else {
            unreachable!("Order enum");
        };
        let join = builder.create_block(root, &[ir::ParamSpec::new(order)])?;
        for (case, op) in [(0, thir::CompareOp::Lt), (1, thir::CompareOp::Eq)] {
            let condition = core(builder, super::scalar::compare(kind, op, left, params[1], Default::default())?)?;
            let hit = builder.create_block(root, &[])?;
            let rest = builder.create_block(root, &[])?;
            builder.terminate(ir::TermKind::Cond {
                condition,
                yes: ir::Target { block: hit, args: vec![] },
                no: ir::Target { block: rest, args: vec![] },
            })?;
            switch(builder, hit)?;
            let value = core(builder, ir::CoreOp::EnumCase { declaration, case })?;
            builder.terminate(ir::TermKind::Br(ir::Target { block: join, args: vec![value] }))?;
            switch(builder, rest)?;
        }
        let value = core(builder, ir::CoreOp::EnumCase { declaration, case: 2 })?;
        builder.terminate(ir::TermKind::Br(ir::Target { block: join, args: vec![value] }))?;
        switch(builder, join)?;
        Ok(builder.block_params(join)?[0])
    })?;
    witness(declarations, owner, role, vec![target])?;
    declarations.builtin_adapters.insert((input, role), ctor);
    Ok(())
}
