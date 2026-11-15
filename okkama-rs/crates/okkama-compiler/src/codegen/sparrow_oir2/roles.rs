//! Role values and callable values (#676).
//!
//! - A role declaration becomes a placeholder descriptor with its slot signatures; every object that
//!   implements a role carries an implementation table, one function per slot.
//! - A sync `fn` type is a role with one owning slot, `call`, numbered after the declarations in type order.
//! - `fn.closure` and `fn.bind` build a closure layout object (one per target function, holding its
//!   captures) that implements its `fn` role through a trampoline: the trampoline reads the captures and
//!   calls the target with the captures first, then the arguments.
//! - `call.role` and `fn.call` dispatch on the receiver's object descriptor to the listed targets.
use super::*;
use s::{Argument as A, Location as L};

/// The object a `fn.closure` or `fn.bind` of `function` builds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClosureLayout {
    pub function: o::FuncId,
    pub callable: o::TypeId,
    pub captures: Vec<o::TypeId>,
}

/// Descriptors and functions the backend adds after the module's own: the `fn` roles (in type order), the
/// closure layouts (in function and instruction order) and one trampoline per closure layout.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Synthetic {
    pub callables: Vec<o::TypeId>,
    pub closures: Vec<ClosureLayout>,
}

/// An array of role values, caps or action values (at any nesting depth). Own tracks the deps of its
/// elements as references, so it never certifies such an array as a closed graph.
pub(crate) fn view_array(module: &o::Module, mut ty: o::TypeId) -> Result<bool, GenerateError> {
    let mut depth = 0;
    while let o::Type::Array(element) = module.types.get(ty)? {
        ty = *element;
        depth += 1;
    }
    Ok(depth > 0 && matches!(module.types.get(ty)?, o::Type::Role { .. } | o::Type::Cap { .. } | o::Type::Fn(_)))
}

fn sync_fn(ty: &o::Type) -> bool {
    matches!(ty, o::Type::Fn(function) if !function.is_async)
}

/// The descriptor of the role of a sync `fn` type: after the declarations, in type order.
pub(crate) fn callable_role(module: &o::Module, ty: o::TypeId) -> Result<u32, GenerateError> {
    if !sync_fn(module.types.get(ty)?) {
        return Err(unsupported("asynchronous callable type"));
    }
    let ordinal = module.types.iter().take(ty.as_u32() as usize).filter(|(_, ty)| sync_fn(ty)).count();
    u32::try_from(module.declarations.len() + ordinal).map_err(|_| GenerateError("too many descriptors".into()))
}

impl Synthetic {
    pub fn of(module: &o::Module) -> Result<Self, GenerateError> {
        let callables = module.types.iter().filter(|(_, ty)| sync_fn(ty)).map(|(id, _)| id).collect();
        let mut closures: Vec<ClosureLayout> = vec![];
        for (_, function) in module.functions.iter() {
            let Some(body) = &function.body else { continue };
            for (_, instruction) in body.insts.iter() {
                let (target, captures, callable) = match &instruction.op {
                    o::Operation::Core(o::CoreOp::FnClosure { function, captures, ty, .. }) => {
                        (*function, captures.clone(), *ty)
                    }
                    o::Operation::Core(o::CoreOp::FnBind { receiver, function, ty, .. }) => {
                        (*function, vec![*receiver], *ty)
                    }
                    o::Operation::Core(o::CoreOp::FnBindRole { .. }) => {
                        return Err(unsupported("bound role slot as an action value"));
                    }
                    _ => continue,
                };
                let captures = captures.iter().map(|&value| Ok(body.values.get(value)?.ty))
                    .collect::<Result<Vec<_>, GenerateError>>()?;
                let layout = ClosureLayout { function: target, callable, captures };
                match closures.iter().find(|closure| closure.function == target) {
                    Some(existing) if *existing != layout => {
                        return Err(GenerateError("action value target with two capture layouts".into()));
                    }
                    Some(_) => {}
                    None => closures.push(layout),
                }
            }
        }
        Ok(Self { callables, closures })
    }

    /// The descriptor of the closure layout of `function`.
    pub fn closure_object(&self, module: &o::Module, function: o::FuncId) -> Option<u32> {
        let index = self.closures.iter().position(|closure| closure.function == function)?;
        u32::try_from(module.declarations.len() + self.callables.len() + index).ok()
    }

    /// The trampoline of closure layout `index`, after the module's functions.
    pub fn trampoline(module: &o::Module, index: usize) -> u32 {
        (module.functions.len() + index) as u32
    }
}

fn entry_abi(abi: o::OwnershipAbi) -> s::EntryAbi {
    match abi {
        o::OwnershipAbi::Explicit { .. } => s::EntryAbi::Owned,
        // An unelaborated slot has neither implementations nor dynamic calls.
        o::OwnershipAbi::Observation | o::OwnershipAbi::Unelaborated => s::EntryAbi::Observation,
    }
}

fn language(module: &o::Module, types: &[o::TypeId]) -> Result<Vec<s::Type>, GenerateError> {
    let mut result = vec![];
    for &ty in types {
        if matches!(module.types.get(ty)?, o::Type::Service(_) | o::Type::Descriptor) {
            continue;
        }
        result.push(translate_type(module, ty)?);
    }
    Ok(result)
}

/// The slots of a role declaration.
pub(crate) fn role_decl(module: &o::Module, methods: &[o::RoleMethod]) -> Result<s::RoleDecl, GenerateError> {
    let slots = methods.iter().map(|method| Ok(s::RoleSlot {
        name: method.name.clone(),
        entry_abi: entry_abi(method.signature.ownership_abi),
        params: language(module, &method.signature.params)?,
        results: language(module, &method.signature.results)?,
    })).collect::<Result<_, GenerateError>>()?;
    Ok(s::RoleDecl { slots })
}

/// An object's implementation tables, from its witnesses.
pub(crate) fn implementations(module: &o::Module, witnesses: &[o::Witness])
    -> Result<Vec<s::RoleImpl>, GenerateError>
{
    witnesses.iter().map(|witness| {
        let o::Type::Role { decl, args } = module.types.get(witness.role)? else {
            return Err(GenerateError("witness names a non-role type".into()));
        };
        if !args.is_empty() {
            return Err(unsupported("generic role witness"));
        }
        Ok(s::RoleImpl { role: decl.as_u32(), slots: witness.slots.iter().map(|function| function.as_u32()).collect() })
    }).collect()
}

fn placeholder(name: String, role: Option<s::RoleDecl>, implements: Vec<s::RoleImpl>) -> s::Object {
    s::Object {
        error_cases: vec![], enum_cases: 0, variant_cases: vec![], name, fields: vec![],
        layout: s::ObjectRuntimeLayout {
            storage_fields: vec![], allocation_words: 2, identity_offset: None, has_destroy: false,
            may_reach_resource: false, may_async_destroy: false, trace_layout: vec![], selection_layout: vec![],
        },
        role, implements,
    }
}

pub(crate) fn traced(ty: s::Type) -> bool {
    matches!(ty, s::Type::Error | s::Type::Bytes | s::Type::Array(_) | s::Type::Str | s::Type::Object(_)
        | s::Type::Successor(_) | s::Type::Failure | s::Type::Rat | s::Type::Variant(_) | s::Type::Role(_)
        | s::Type::RoleSuccessor(_))
}

/// Whether a value of `ty` may reach a resource, given the descriptors built so far.
fn reaches(objects: &[s::Object], ty: s::Type) -> bool {
    match ty {
        s::Type::Object(id) | s::Type::Successor(id) => objects.get(id as usize)
            .is_none_or(|object| object.layout.may_reach_resource),
        s::Type::Role(id) | s::Type::RoleSuccessor(id) => objects.iter()
            .filter(|object| object.implements.iter().any(|table| table.role == id))
            .any(|object| object.layout.may_reach_resource),
        s::Type::Error => objects.iter().filter(|object| !object.error_cases.is_empty())
            .any(|object| object.layout.may_reach_resource),
        _ => false,
    }
}

/// The `fn` roles and the closure layouts, appended after the declarations' descriptors.
pub(crate) fn synthetic_objects(module: &o::Module, objects: &mut Vec<s::Object>) -> Result<(), GenerateError> {
    let synthetic = Synthetic::of(module)?;
    for &ty in &synthetic.callables {
        let o::Type::Fn(function) = module.types.get(ty)? else { unreachable!("sync fn type") };
        let params = function.params.iter().map(|&ty| translate_type(module, ty)).collect::<Result<Vec<_>, _>>();
        let results = function.result.iter().map(|&ty| translate_type(module, ty)).collect::<Result<Vec<_>, _>>();
        // A `fn` type outside the representable types has no slot: no value of it can be called.
        let slots = match (params, results) {
            (Ok(params), Ok(results)) => vec![s::RoleSlot {
                name: "call".into(), entry_abi: s::EntryAbi::Owned, params, results,
            }],
            _ => vec![],
        };
        objects.push(placeholder(format!("fn#{}", ty.as_u32()), Some(s::RoleDecl { slots }), vec![]));
    }
    for (index, closure) in synthetic.closures.iter().enumerate() {
        let storage_fields = closure.captures.iter().map(|&ty| translate_type(module, ty))
            .collect::<Result<Vec<_>, _>>()?;
        if storage_fields.iter().any(|&ty| reaches(objects, ty)) {
            return Err(unsupported("action value capturing a resource"));
        }
        let symbol = &module.functions.get(closure.function)?.symbol;
        let mut object = placeholder(format!("{symbol}.closure"), None, vec![s::RoleImpl {
            role: callable_role(module, closure.callable)?,
            slots: vec![Synthetic::trampoline(module, index)],
        }]);
        object.fields = (0..storage_fields.len() as u32).map(|storage| s::Field {
            name: format!("capture{storage}"), storage, once: false,
        }).collect();
        object.layout.allocation_words = (1 + storage_fields.len() as u32).max(2);
        object.layout.trace_layout = storage_fields.iter().enumerate()
            .filter_map(|(index, &ty)| traced(ty).then_some(index as u32)).collect();
        object.layout.storage_fields = storage_fields;
        objects.push(object);
    }
    Ok(())
}

fn fresh_location(function: &mut s::Function, ty: s::ProducedType) -> L {
    match ty {
        s::ProducedType::Value(ty) => {
            function.registers.push(ty);
            L::Register(function.registers.len() as u32 - 1)
        }
        s::ProducedType::Service(kind) => {
            function.services.push(kind);
            L::Service(function.services.len() as u32 - 1)
        }
    }
}

/// The trampoline of a closure layout: reads the captures, calls the target with the captures first and
/// the arguments after them, and returns its outcome. Its contract is the target's, with every capture
/// position mapped to the environment (position 0) and the arguments shifted after it.
pub(crate) fn trampoline(program: &s::Program, closure: &ClosureLayout, layout: u32,
    summaries: &mut Vec<s::Summary>) -> Result<s::Function, GenerateError>
{
    let target = closure.function.as_u32();
    let callee = program.functions.get(target as usize)
        .ok_or_else(|| GenerateError("closure target outside the program".into()))?;
    let contract = callee.proofs.entry.clone().ok_or_else(|| GenerateError("closure target without contract".into()))?;
    if callee.entry_abi != s::EntryAbi::Owned {
        return Err(unsupported("action value of a non-owning entry"));
    }
    let language = callee.params.iter().enumerate().filter_map(|(index, &location)| match location {
        L::Register(id) => Some((index as u32, callee.registers[id as usize])),
        L::Service(_) => None,
    }).collect::<Vec<_>>();
    let captures = closure.captures.len();
    let object = &program.objects[layout as usize];
    if language.len() < captures || language[..captures].iter().map(|(_, ty)| *ty).ne(
        object.layout.storage_fields.iter().copied())
    {
        return Err(GenerateError("closure captures differ from the target's leading parameters".into()));
    }
    let mut f = s::Function {
        symbol: format!("{}.call", object.name), entry_abi: s::EntryAbi::Owned, registers: vec![], services: vec![],
        params: vec![], results: callee.results.clone(), blocks: vec![], proofs: s::FunctionProofs::default(),
        code: vec![],
    };
    let environment = fresh_location(&mut f, s::ProducedType::Value(s::Type::Object(layout)));
    let arguments = language[captures..].iter().map(|&(_, ty)| fresh_location(&mut f, s::ProducedType::Value(ty)))
        .collect::<Vec<_>>();
    let packet = fresh_location(&mut f, s::ProducedType::Service(s::ServiceKind::Packet));
    f.params = std::iter::once(environment).chain(arguments.iter().copied()).chain([packet]).collect();
    f.blocks.push(s::Block { pc: 0, params: f.params.clone() });
    let L::Register(environment_register) = environment else { unreachable!("register") };
    let mut values = vec![];
    for (storage, &ty) in object.layout.storage_fields.iter().enumerate() {
        let capture = fresh_location(&mut f, s::ProducedType::Value(ty));
        let L::Register(id) = capture else { unreachable!("register") };
        f.code.push(data(s::DataOp::ObjGet, vec![environment_register], id, s::Immediate::Field(storage as u32)));
        values.push(capture);
    }
    values.extend(&arguments);
    let summary = summaries.len() as u32;
    summaries.push(s::Summary { entry: target, may_fail: contract.failures != 0,
        retained_params: contract.retained_params.clone() });
    let call = f.code.len() as u32;
    let normal = call + 1;
    let unwind = call + 2;
    let mut normal_params = vec![];
    let mut normal_assignments = vec![];
    for (slot, ty) in f.results.clone().into_iter().map(s::ProducedType::Value)
        .chain([s::ProducedType::Service(s::ServiceKind::Packet)]).enumerate()
    {
        let destination = fresh_location(&mut f, ty);
        normal_params.push(destination);
        normal_assignments.push(s::Assignment { destination, source: s::Source::ProducedSlot(slot as u32) });
    }
    let mut outcomes = vec![s::Outcome { tag: s::Tag::Normal, edge: s::Edge {
        target_pc: normal, assignments: normal_assignments,
    } }];
    let failure = (contract.failures != 0).then(|| fresh_location(&mut f, s::ProducedType::Value(s::Type::Failure)));
    if let Some(failure) = failure {
        outcomes.push(s::Outcome { tag: s::Tag::Unwind, edge: s::Edge {
            target_pc: unwind, assignments: vec![s::Assignment { destination: failure,
                source: s::Source::ProducedSlot(0) }],
        } });
    }
    let mut call_arguments = vec![A::Entry(target), A::Summary(summary), A::Flag(false)];
    call_arguments.extend(values.iter().map(|&location| A::Location(location)));
    call_arguments.push(A::Location(packet));
    f.code.push(s::Instruction::Champ { op: s::Opcode::InvokeCallOwn, arguments: call_arguments,
        flow: s::Flow::Outcomes(outcomes) });
    f.blocks.push(s::Block { pc: normal, params: normal_params.clone() });
    f.code.push(s::Instruction::Champ { op: s::Opcode::RetOwn,
        arguments: normal_params.into_iter().map(A::Location).collect(), flow: s::Flow::Next(vec![]) });
    if let Some(failure) = failure {
        let L::Register(id) = failure else { unreachable!("register") };
        f.blocks.push(s::Block { pc: unwind, params: vec![failure] });
        f.code.push(s::Instruction::Raise(id));
    }
    let retained_params = contract.retained_params.iter().map(|&param| {
        let position = language.iter().position(|(index, _)| *index == param)
            .ok_or_else(|| GenerateError("closure target retains a service".into()))?;
        Ok(if position < captures { 0 } else { (position - captures + 1) as u32 })
    }).collect::<Result<BTreeSet<_>, GenerateError>>()?.into_iter().collect();
    f.proofs.entry = Some(s::EntryContract {
        external: None, retained_params, fresh_singleton: false, failures: contract.failures, destroy: None,
    });
    f.proofs.states = s::service_states(&f)?;
    Ok(f)
}

/// Finite origins of an action value: the closure targets it can hold, or `None` when it is open.
/// Mirrors own's origins: closures and bound actions, through retags and block parameters. Entry
/// parameters, produced edge results and every other definition are open.
pub(crate) fn callable_origins(module: &o::Module, body: &o::Body)
    -> Result<BTreeMap<o::ValueId, Option<BTreeSet<o::FuncId>>>, GenerateError>
{
    let mut origins = BTreeMap::new();
    for (value, data) in body.values.iter() {
        if !matches!(module.types.get(data.ty)?, o::Type::Fn(_)) {
            continue;
        }
        let origin = match data.def {
            o::Definition::BlockParam { block, .. } if block == body.entry => None,
            o::Definition::BlockParam { .. } => Some(BTreeSet::new()),
            o::Definition::InstResult { inst, .. } => match &body.insts.get(inst)?.op {
                o::Operation::Core(o::CoreOp::FnClosure { function, .. } | o::CoreOp::FnBind { function, .. }) => {
                    Some(BTreeSet::from([*function]))
                }
                o::Operation::Core(o::CoreOp::Retag { .. }) => Some(BTreeSet::new()),
                _ => None,
            },
        };
        origins.insert(value, origin);
    }
    let mut pairs = vec![];
    for (_, instruction) in body.insts.iter() {
        if let o::Operation::Core(o::CoreOp::Retag { value, .. }) = instruction.op {
            pairs.extend(instruction.results.iter().map(|result| (*result, Some(value))));
        }
    }
    for (_, block) in body.blocks.iter() {
        let mut edge = |target: o::BlockId, args: Vec<o::EdgeArg>| -> Result<(), GenerateError> {
            for (&parameter, argument) in body.blocks.get(target)?.params.iter().zip(args) {
                match argument {
                    o::EdgeArg::Value(value) => pairs.push((parameter, Some(value))),
                    o::EdgeArg::Produced(_) => pairs.push((parameter, None)),
                }
            }
            Ok(())
        };
        let values = |target: &o::Target| target.args.iter().copied().map(o::EdgeArg::Value).collect::<Vec<_>>();
        if let Some(unwind) = &block.unwind {
            edge(unwind.target.block, values(&unwind.target))?;
        }
        match &block.term.kind {
            o::TermKind::Br(target) => edge(target.block, values(target))?,
            o::TermKind::Cond { yes, no, .. } => {
                edge(yes.block, values(yes))?;
                edge(no.block, values(no))?;
            }
            o::TermKind::Switch { cases, default, .. } => {
                for case in cases {
                    edge(case.target.block, values(&case.target))?;
                }
                edge(default.block, values(default))?;
            }
            o::TermKind::InvokeOwn { normal, unwind, .. } => {
                edge(normal.block, normal.args.clone())?;
                edge(unwind.block, unwind.args.clone())?;
            }
            o::TermKind::CleanupNext { selected, done, .. } => {
                edge(selected.block, selected.args.clone())?;
                edge(done.block, done.args.clone())?;
            }
            o::TermKind::FailureDispatch { none, some, .. } => {
                edge(none.block, values(none))?;
                edge(some.block, some.args.clone())?;
            }
            o::TermKind::ExitDispatch { success, pending, failed, .. } => {
                for edge_target in [success, pending, failed].into_iter().flatten() {
                    edge(edge_target.block, edge_target.args.clone())?;
                }
            }
            o::TermKind::InvokeDefer { continue_edge, replacements, unwind, .. } => {
                edge(continue_edge.block, continue_edge.args.clone())?;
                for replacement in replacements {
                    edge(replacement.edge.block, replacement.edge.args.clone())?;
                }
                if let Some(unwind) = unwind {
                    edge(unwind.block, unwind.args.clone())?;
                }
            }
            o::TermKind::Catch { handler, next, .. } => {
                edge(handler.block, handler.args.clone())?;
                edge(next.block, values(next))?;
            }
            o::TermKind::Lookup { found, missing, .. } => {
                edge(found.block, found.args.clone())?;
                edge(missing.block, values(missing))?;
            }
            _ => {}
        }
    }
    loop {
        let mut changed = false;
        for &(parameter, argument) in &pairs {
            let incoming = match argument {
                Some(argument) => match origins.get(&argument) {
                    Some(origin) => origin.clone(),
                    None => continue,
                },
                None => None,
            };
            let Some(origin) = origins.get_mut(&parameter) else { continue };
            match (origin.as_mut(), incoming) {
                (None, _) => {}
                (Some(_), None) => {
                    *origin = None;
                    changed = true;
                }
                (Some(entries), Some(incoming)) => {
                    for entry in incoming {
                        changed |= entries.insert(entry);
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    Ok(origins)
}
