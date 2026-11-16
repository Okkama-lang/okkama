//! Concrete builtin evidence uses C3 objects/witnesses, never an external body.
use super::{LowerError, Result, declarations::Declarations};
use crate::analysis::{Analysis, thir, ty};
use crate::oir2 as ir;

pub(super) fn declare(declarations: &mut Declarations, analysis: &Analysis<'_>) -> Result<()> {
    let mut pairs = vec![];
    let mut cursors = vec![];
    // Closure bodies are lowered too, so their wraps and cursors need the same evidence.
    for root in &analysis.bodies {
        super::walk::bodies(root, &mut |body| {
            super::walk::block(&body.block, &mut |expr| {
                if let thir::ExprKind::Coerce { coercion: thir::Coercion::RoleWrap, value } = &expr.kind
                    && let (Some(source), Some(target)) = (&value.ty, &expr.ty)
                    && let (Some(&source), Some(&target))
                        = (declarations.types.get(source), declarations.types.get(target)) {
                    pairs.push((source, target));
                }
                if let thir::ExprKind::Call { callee: thir::Callee::Builtin { receiver, op }, .. } = &expr.kind
                    && *op == thir::Builtin::ArrayCursor
                    && let (Some(source), Some(target)) = (&receiver.ty, &expr.ty)
                    && let (Some(&source), Some(&target))
                        = (declarations.types.get(source), declarations.types.get(target)) {
                    cursors.push((source, target));
                }
            });
        });
    }
    pairs.sort();
    pairs.dedup();
    cursors.sort();
    cursors.dedup();
    let roles = ArrayRoles::new(declarations, analysis)?;
    for (input, role) in pairs {
        if matches!(declarations.builder.module().types.get(input)?, ir::Type::Array(_))
            && let Some(entry) = roles.find(role) {
            let ir::Type::Array(own) = *declarations.builder.module().types.get(input)? else {
                unreachable!("array input");
            };
            sequence(declarations, &roles, input, entry.element.unwrap_or(own))?;
            continue;
        }
        let ir::Type::Role { decl, .. } = *declarations.builder.module().types.get(role)? else {
            continue;
        };
        let ir::DeclarationKind::Role { methods, .. } = &declarations.builder.module().declarations.get(decl)?.kind
            else {
                continue;
            };
        let methods = methods.clone();
        if let Some(kind) = super::sorting::scalar_kind(declarations.builder.module().types.get(input)?)
            && methods.len() == 1 && methods[0].name == "order" {
            scalar(declarations, input, role, kind, methods[0].signature.results[0])?;
        }
    }
    // `xs.cursor()`: the same array cursor evidence, reached without the Iterating adapter.
    for (input, cursor_role) in cursors {
        if matches!(declarations.builder.module().types.get(input)?, ir::Type::Array(_)) {
            cursor(declarations, &roles, input, cursor_role)?;
        }
    }
    Ok(())
}

/// The standard roles an array plays (§12): `Sequencing<T>`, `Iterating<T>` and `Sizing`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArrayRole {
    Sequencing,
    Iterating,
    Sizing,
}

/// A specialization of an array role present in the module: its role type, the role element `T`
/// (`None` for `Sizing`) and, for the two walkable roles, its `Cursor<T>` type.
#[derive(Clone, Copy, Debug)]
struct ArrayRoleEntry {
    role: ir::TypeId,
    element: Option<ir::TypeId>,
    cursor: Option<ir::TypeId>,
}

/// Every specialization of the array roles in the module, in registry order: the roles an array
/// adapter object of element `T` witnesses, as Tomato's `ArraySequence<S, T>` does all three.
struct ArrayRoles {
    entries: Vec<ArrayRoleEntry>,
}

impl ArrayRoles {
    fn new(declarations: &Declarations, analysis: &Analysis<'_>) -> Result<Self> {
        let registry = &analysis.registry;
        let standard = registry.standard;
        let kinds = [(standard.sequencing, ArrayRole::Sequencing), (standard.iterating, ArrayRole::Iterating),
            (standard.sizing, ArrayRole::Sizing)];
        let mut entries = vec![];
        for (index, info) in registry.roles.iter().enumerate() {
            let kind = kinds.iter().find_map(|&(generic, kind)| {
                let generic = &registry.roles[generic?.0];
                let same = info.module == generic.module && (info.name == generic.name
                    || info.name.strip_prefix(generic.name.as_str()).is_some_and(|rest| rest.starts_with('<')));
                same.then_some(kind)
            });
            let (Some(kind), Some(&role)) = (kind, declarations.types.get(&ty::Ty::role(ty::RoleId(index)))) else {
                continue;
            };
            let slot = |name: &str| -> Result<Option<ir::TypeId>> {
                let ir::Type::Role { decl, .. } = *declarations.builder.module().types.get(role)? else {
                    return Ok(None);
                };
                let ir::DeclarationKind::Role { methods, .. } = &declarations.builder.module()
                    .declarations.get(decl)?.kind else {
                    return Ok(None);
                };
                Ok(methods.iter().find(|method| method.name == name)
                    .and_then(|method| method.signature.results.first().copied()))
            };
            let cursor = if kind == ArrayRole::Sizing { None } else { slot("cursor")? };
            let element = match kind {
                ArrayRole::Sequencing => slot("item")?,
                ArrayRole::Iterating => match cursor {
                    Some(cursor) => cursor_element(declarations, cursor)?,
                    None => None,
                },
                ArrayRole::Sizing => None,
            };
            if kind != ArrayRole::Sizing && (element.is_none() || cursor.is_none()) {
                continue;
            }
            entries.push(ArrayRoleEntry { role, element, cursor });
        }
        Ok(Self { entries })
    }

    fn find(&self, role: ir::TypeId) -> Option<ArrayRoleEntry> {
        self.entries.iter().find(|entry| entry.role == role).copied()
    }

    /// The roles an array adapter of role element `element` witnesses.
    fn of(&self, element: ir::TypeId) -> Vec<ArrayRoleEntry> {
        self.entries.iter().filter(|entry| entry.element.is_none_or(|own| own == element)).copied().collect()
    }

    fn cursor_of(&self, element: ir::TypeId) -> Option<ir::TypeId> {
        self.entries.iter().find_map(|entry| (entry.element == Some(element)).then_some(entry.cursor).flatten())
    }
}

/// The `item()` result of a `Cursor<T>` role type.
fn cursor_element(declarations: &Declarations, cursor: ir::TypeId) -> Result<Option<ir::TypeId>> {
    let ir::Type::Role { decl, .. } = *declarations.builder.module().types.get(cursor)? else {
        return Ok(None);
    };
    let ir::DeclarationKind::Role { methods, .. } = &declarations.builder.module().declarations.get(decl)?.kind
        else {
            return Ok(None);
        };
    Ok(methods.iter().find(|method| method.name == "item" && method.signature.params.is_empty())
        .and_then(|method| method.signature.results.first().copied()))
}

/// How an array element of type `S` becomes the role element `T` when an adapter reads it, as Tomato's
/// adapters cross it: unchanged, an object or variant wrapped into its role, or an inner array adapted to
/// an array role by its own adapter (#488, at any depth).
#[derive(Clone, Copy, Debug)]
enum Crossing {
    Same,
    Wrap(ir::TypeId),
    Nested { constructor: ir::FuncId, role: ir::TypeId },
}

fn crossing(declarations: &mut Declarations, roles: &ArrayRoles, source: ir::TypeId, target: ir::TypeId)
    -> Result<Crossing> {
    if source == target {
        return Ok(Crossing::Same);
    }
    let span = crate::source::Span::default();
    if !matches!(declarations.builder.module().types.get(target)?, ir::Type::Role { .. }) {
        return Err(LowerError::unsupported(span, "builtin role adapter element of another type"));
    }
    match declarations.builder.module().types.get(source)? {
        ir::Type::Object { .. } | ir::Type::Variant(_) => Ok(Crossing::Wrap(target)),
        &ir::Type::Array(inner) => {
            let entry = roles.find(target).ok_or_else(|| {
                LowerError::unsupported(span, "an inner array adapted to a role other than an array role")
            })?;
            let constructor = sequence(declarations, roles, source, entry.element.unwrap_or(inner))?;
            Ok(Crossing::Nested { constructor, role: target })
        }
        _ => Err(LowerError::unsupported(span, "builtin role adapter element crossing")),
    }
}

fn cross(builder: &mut ir::FunctionBuilder<'_>, value: ir::ValueId, crossing: Crossing) -> Result<ir::ValueId> {
    match crossing {
        Crossing::Same => Ok(value),
        Crossing::Wrap(ty) => core(builder, ir::CoreOp::RoleWrap { value, ty }),
        Crossing::Nested { constructor, role } => {
            let object = call(builder, constructor, vec![value])?;
            core(builder, ir::CoreOp::RoleWrap { value: object, ty: role })
        }
    }
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

pub(super) fn function(declarations: &mut Declarations, owner: ir::DeclId, name: &str,
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

pub(super) fn get(builder: &mut ir::FunctionBuilder<'_>, object: ir::ValueId, owner: ir::DeclId, index: u32)
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

/// The array adapter object of `input` with role element `element` (Tomato's `ArraySequence<S, T>`):
/// one object per pair, witnessing every array role of `element` in the module (`Sequencing<T>`,
/// `Iterating<T>`, `Sizing`), so that a narrowing between them succeeds as on Tomato. Its constructor
/// `(items)` is the builtin adapter of `(input, role)` for each of those roles.
fn sequence(declarations: &mut Declarations, roles: &ArrayRoles, input: ir::TypeId, element: ir::TypeId)
    -> Result<ir::FuncId> {
    if let Some(&ctor) = declarations.array_sequences.get(&(input, element)) {
        return Ok(ctor);
    }
    let ir::Type::Array(own) = *declarations.builder.module().types.get(input)? else {
        unreachable!("array evidence");
    };
    let entries = roles.of(element);
    let cursor_role = roles.cursor_of(element);
    let opener = cursor_role.map(|role| cursor(declarations, roles, input, role)).transpose()?;
    let crossing = crossing(declarations, roles, own, element)?;
    let name = if element == own {
        format!("builtin.ArraySequence.t{}", input.as_u32())
    } else {
        format!("builtin.ArraySequence.t{}.t{}", input.as_u32(), element.as_u32())
    };
    let (sequence, sequence_ty) = object(declarations, name, vec![input])?;
    let ctor = function(declarations, sequence, "new", vec![input], sequence_ty, true, |builder, _| {
        construct(builder, sequence, builder.block_params(builder.entry())?.to_vec())
    })?;
    declarations.array_sequences.insert((input, element), ctor);
    let int = declarations.builder.intern_type(ir::Type::Int)?;
    let mut actions: Vec<(String, ir::FuncId)> = vec![];
    for entry in &entries {
        let ir::Type::Role { decl, .. } = *declarations.builder.module().types.get(entry.role)? else {
            unreachable!("array role");
        };
        let ir::DeclarationKind::Role { methods, .. } = &declarations.builder.module().declarations.get(decl)?.kind
            else {
                unreachable!("array role declaration");
            };
        let methods = methods.clone();
        let mut slots = vec![];
        for method in &methods {
            if let Some((_, target)) = actions.iter().find(|(name, _)| *name == method.name) {
                slots.push(*target);
                continue;
            }
            let span = crate::source::Span::default();
            let unknown = || LowerError::invalid(span, format!("array adapter has no action {}", method.name));
            let target = match method.name.as_str() {
                "cursor" => {
                    let (Some(opener), Some(cursor_role)) = (opener, cursor_role) else {
                        return Err(unknown());
                    };
                    function(declarations, sequence, "cursor", vec![sequence_ty], cursor_role, false, |builder, _| {
                        let receiver = builder.block_params(builder.entry())?[0];
                        let array = get(builder, receiver, sequence, 0)?;
                        let zero = builder.emit_schema(ir::Operation::Const(ir::Literal::Int("0".into())))?[0];
                        let value = call(builder, opener, vec![array, zero])?;
                        core(builder, ir::CoreOp::RoleWrap { value, ty: cursor_role })
                    })?
                }
                "size" => function(declarations, sequence, "size", vec![sequence_ty], int, false, |builder, _| {
                    let receiver = builder.block_params(builder.entry())?[0];
                    let array = get(builder, receiver, sequence, 0)?;
                    Ok(builder.emit_schema(ir::Operation::Collection(ir::CollectionOp::ArrayLen { array }))?[0])
                })?,
                "item" => {
                    let params = vec![sequence_ty, int];
                    function(declarations, sequence, "item", params, element, false, |builder, _| {
                        let params = builder.block_params(builder.entry())?.to_vec();
                        let array = get(builder, params[0], sequence, 0)?;
                        let index = params[1];
                        let item = builder.emit_schema(ir::Operation::Collection(ir::CollectionOp::ArrayGet {
                            array, index,
                        }))?[0];
                        cross(builder, item, crossing)
                    })?
                }
                name @ ("first" | "last") => {
                    function(declarations, sequence, name, vec![sequence_ty], element, false, |builder, root| {
                        let receiver = builder.block_params(builder.entry())?[0];
                        let array = get(builder, receiver, sequence, 0)?;
                        let index = if name == "first" {
                            builder.emit_schema(ir::Operation::Const(ir::Literal::Int("0".into())))?[0]
                        } else {
                            let size = builder.emit_schema(ir::Operation::Collection(
                                ir::CollectionOp::ArrayLen { array }))?[0];
                            let one = builder.emit_schema(ir::Operation::Const(ir::Literal::Int("1".into())))?[0];
                            core(builder, ir::CoreOp::IntSub { left: size, right: one })?
                        };
                        let found = builder.create_block(root, &[ir::ParamSpec::new(own)])?;
                        let missing = builder.create_block(root, &[])?;
                        builder.terminate(ir::TermKind::Lookup {
                            kind: ir::LookupKind::Array, collection: array, key: index,
                            found: ir::InvokeEdge {
                                block: found,
                                args: vec![ir::EdgeArg::Produced(ir::EdgeResult {
                                    outcome: ir::Outcome::Normal, slot: 0,
                                })],
                            },
                            missing: ir::Target { block: missing, args: vec![] },
                        })?;
                        switch(builder, missing)?;
                        // Tomato's array adapter (and the array builtin): an empty-sequence program error.
                        builder.terminate(ir::TermKind::Trap {
                            code: ir::TrapCode::Empty,
                            message: format!("Cannot read {name} item of an empty sequence"),
                        })?;
                        switch(builder, found)?;
                        let item = builder.block_params(found)?[0];
                        cross(builder, item, crossing)
                    })?
                }
                _ => return Err(unknown()),
            };
            actions.push((method.name.clone(), target));
            slots.push(target);
        }
        witness(declarations, sequence, entry.role, slots)?;
        declarations.builtin_adapters.entry((input, entry.role)).or_insert(ctor);
    }
    Ok(ctor)
}

/// The array cursor object (`items`, `at`) for `cursor_role`, once per array type and cursor role; its
/// constructor `(items, at)` is the builtin adapter of `(array, cursor role)`. Its item crosses into the
/// cursor's element as the sequence's items do.
fn cursor(declarations: &mut Declarations, roles: &ArrayRoles, input: ir::TypeId, cursor_role: ir::TypeId)
    -> Result<ir::FuncId> {
    if let Some(&ctor) = declarations.builtin_adapters.get(&(input, cursor_role)) {
        return Ok(ctor);
    }
    let int = declarations.builder.intern_type(ir::Type::Int)?;
    let bool_ty = declarations.builder.intern_type(ir::Type::Bool)?;
    let ir::Type::Array(own) = *declarations.builder.module().types.get(input)? else {
        unreachable!("array evidence");
    };
    let element = cursor_element(declarations, cursor_role)?.ok_or_else(|| {
        LowerError::invalid(crate::source::Span::default(), "cursor role without item()")
    })?;
    let crossing = crossing(declarations, roles, own, element)?;
    let cursor_name = if element == own {
        format!("builtin.ArrayCursor.t{}", input.as_u32())
    } else {
        format!("builtin.ArrayCursor.t{}.t{}", input.as_u32(), cursor_role.as_u32())
    };
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
                let item = builder.emit_schema(ir::Operation::Collection(ir::CollectionOp::ArrayGet {
                    array, index: at,
                }))?[0];
                cross(builder, item, crossing)
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
