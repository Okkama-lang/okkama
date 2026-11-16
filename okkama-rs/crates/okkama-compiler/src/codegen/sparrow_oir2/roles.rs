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

/// What a closure layout's trampoline calls: a function with the captures first (`fn.closure`, `fn.bind`),
/// or a role slot on the one captured receiver (`fn.bind.role`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClosureTarget {
    Function(o::FuncId),
    RoleSlot { role: o::DeclId, slot: u32 },
}

/// The object an action value of `target` is: its captures, viewed as the role of its `fn` type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClosureLayout {
    pub target: ClosureTarget,
    pub callable: o::TypeId,
    pub captures: Vec<o::TypeId>,
}

/// An instruction that builds an action value: its layout, its capture values and its packet.
pub(crate) struct ClosureSite {
    pub layout: ClosureLayout,
    pub values: Vec<o::ValueId>,
    pub packet: Option<o::ValueId>,
}

/// The action value `op` builds, if it builds one.
pub(crate) fn closure_site(body: &o::Body, op: &o::Operation) -> Result<Option<ClosureSite>, GenerateError> {
    let (target, values, callable, packet) = match op {
        o::Operation::Core(o::CoreOp::FnClosure { function, captures, ty, packet }) => {
            (ClosureTarget::Function(*function), captures.clone(), *ty, *packet)
        }
        o::Operation::Core(o::CoreOp::FnBind { receiver, function, ty, packet }) => {
            (ClosureTarget::Function(*function), vec![*receiver], *ty, *packet)
        }
        o::Operation::Core(o::CoreOp::FnBindRole { receiver, role, slot, ty, packet }) => {
            (ClosureTarget::RoleSlot { role: *role, slot: *slot }, vec![*receiver], *ty, *packet)
        }
        _ => return Ok(None),
    };
    let captures = values.iter().map(|&value| Ok(body.values.get(value)?.ty))
        .collect::<Result<Vec<_>, GenerateError>>()?;
    Ok(Some(ClosureSite { layout: ClosureLayout { target, callable, captures }, values, packet }))
}

/// Descriptors and functions the backend adds after the module's own: the `fn` roles (in type order), the
/// closure layouts (in function and instruction order) and one trampoline per closure layout.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Synthetic {
    pub callables: Vec<o::TypeId>,
    pub closures: Vec<ClosureLayout>,
    /// Whether the module has the `any` type: its descriptor follows the closure layouts.
    pub any: bool,
    /// The boxed value types of the `any.box` sites, one any box each, after the `any` descriptor.
    pub boxes: Vec<o::TypeId>,
}

/// An `any.box` site: the boxed value and its packet.
pub(crate) fn any_box_site(body: &o::Body, op: &o::Operation) -> Result<Option<(o::TypeId, o::ValueId,
    Option<o::ValueId>)>, GenerateError>
{
    let o::Operation::Core(o::CoreOp::AnyBox { value, packet, .. }) = op else { return Ok(None) };
    Ok(Some((body.values.get(*value)?.ty, *value, *packet)))
}

/// An array of role values, caps, action values or `any` values (at any nesting depth), or a map whose values
/// are such values or arrays of them. Own tracks the deps of their elements as references, so it never
/// certifies such a collection as a closed graph; the loader admits it only as closed plain data.
pub(crate) fn view_array(module: &o::Module, mut ty: o::TypeId) -> Result<bool, GenerateError> {
    let mut depth = 0;
    if let o::Type::Map(_, value) = module.types.get(ty)? {
        ty = *value;
        depth += 1;
    }
    while let o::Type::Array(element) = module.types.get(ty)? {
        ty = *element;
        depth += 1;
    }
    Ok(depth > 0 && matches!(module.types.get(ty)?,
        o::Type::Role { .. } | o::Type::Cap { .. } | o::Type::Fn(_) | o::Type::Any))
}

fn sync_fn(ty: &o::Type) -> bool {
    matches!(ty, o::Type::Fn(function) if !function.is_async)
}

/// A runtime descriptor of an opaque value (`type.of` of a `fn` type or `any`) names the registered opaque
/// cleanup intrinsic `name` of exactly that type: a bodiless declaration keyed by its first parameter.
pub(crate) fn opaque_descriptor(module: &o::Module, ty: o::TypeId, descriptor: &o::DescriptorContract, name: &str)
    -> Result<(), GenerateError>
{
    let cleanup = module.functions.get(descriptor.cleanup)?;
    let intrinsic = cleanup.attrs.iter().any(|attr| matches!(attr, o::FunctionAttr::Intrinsic { name: actual, .. }
        if actual == name));
    if descriptor.managed != o::DescriptorManaged::Runtime || descriptor.async_destroy || !intrinsic
        || cleanup.body.is_some() || cleanup.signature.params.first() != Some(&ty)
    {
        return Err(GenerateError(format!("runtime descriptor without the {name} intrinsic of its type")));
    }
    Ok(())
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
        let any = module.types.iter().any(|(_, ty)| *ty == o::Type::Any);
        let mut synthetic = Self { callables, closures: vec![], any, boxes: vec![] };
        for (_, function) in module.functions.iter() {
            let Some(body) = &function.body else { continue };
            for (_, instruction) in body.insts.iter() {
                if let Some((ty, _, _)) = any_box_site(body, &instruction.op)? {
                    if !synthetic.boxes.contains(&ty) {
                        synthetic.boxes.push(ty);
                    }
                    continue;
                }
                let Some(site) = closure_site(body, &instruction.op)? else { continue };
                // A target function has one capture layout; a bound role slot has one per receiver type.
                let same = synthetic.closures.iter().find(|closure| closure.target == site.layout.target
                    && (matches!(closure.target, ClosureTarget::Function(_))
                        || closure.captures == site.layout.captures));
                match same {
                    Some(existing) if *existing != site.layout => {
                        return Err(GenerateError("action value target with two capture layouts".into()));
                    }
                    Some(_) => {}
                    None => synthetic.closures.push(site.layout),
                }
            }
        }
        Ok(synthetic)
    }

    /// The index of `layout` among the closure layouts.
    pub fn closure_index(&self, layout: &ClosureLayout) -> Option<usize> {
        self.closures.iter().position(|closure| closure == layout)
    }

    /// The descriptor of closure layout `index`.
    pub fn closure_object(&self, module: &o::Module, index: usize) -> u32 {
        (module.declarations.len() + self.callables.len() + index) as u32
    }

    /// The descriptor of `any`, after the closure layouts.
    pub fn any_object(&self, module: &o::Module) -> u32 {
        (module.declarations.len() + self.callables.len() + self.closures.len()) as u32
    }

    /// The any box of the boxed value type `ty`, after the `any` descriptor.
    pub fn box_object(&self, module: &o::Module, ty: o::TypeId) -> Option<u32> {
        let index = self.boxes.iter().position(|boxed| *boxed == ty)?;
        Some(self.any_object(module) + 1 + index as u32)
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

pub(super) fn placeholder(name: String, role: Option<s::RoleDecl>, implements: Vec<s::RoleImpl>) -> s::Object {
    s::Object {
        error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle: false, name, fields: vec![],
        layout: s::ObjectRuntimeLayout {
            storage_fields: vec![], allocation_words: 2, identity_offset: None, has_destroy: false,
            may_reach_resource: false, may_async_destroy: false, trace_layout: vec![], selection_layout: vec![],
        },
        role, implements, any_box: false, enum_names: vec![],
    }
}

/// Whether a value of `ty` may reach a resource, given the descriptors built so far: a resource object or
/// successor, a role one of whose implementations reaches one, or a managed array or map of them.
fn reaches(module: &o::Module, objects: &[s::Object], ty: s::Type) -> bool {
    match ty {
        s::Type::Object(id) | s::Type::Successor(id) | s::Type::Variant(id) => objects.get(id as usize)
            .is_none_or(|object| object.layout.may_reach_resource),
        s::Type::Role(id) | s::Type::RoleSuccessor(id) => objects.iter()
            .filter(|object| object.implements.iter().any(|table| table.role == id))
            .any(|object| object.layout.may_reach_resource),
        s::Type::Error => objects.iter().filter(|object| !object.error_cases.is_empty())
            .any(|object| object.layout.may_reach_resource),
        s::Type::Any => objects.iter().any(|object| object.any_box && object.layout.may_reach_resource),
        s::Type::Array(shape) => s::ArrayType { depth: 1, ..shape }.element()
            .is_some_and(|leaf| reaches(module, objects, leaf)),
        s::Type::Map(id) => match composites(module).entries().get(id as usize) {
            Some(s::Composite::Map { value, .. }) => reaches(module, objects, *value),
            _ => true,
        },
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
        let name = match closure.target {
            ClosureTarget::Function(function) => format!("{}.closure", module.functions.get(function)?.symbol),
            ClosureTarget::RoleSlot { role, slot } => {
                let declaration = module.declarations.get(role)?;
                let o::DeclarationKind::Role { methods, .. } = &declaration.kind else {
                    return Err(GenerateError("bound role slot of a non-role".into()));
                };
                let method = methods.get(slot as usize).ok_or_else(|| GenerateError("bound missing slot".into()))?;
                format!("{}.{}#{index}.closure", declaration.name, method.name)
            }
        };
        let mut object = placeholder(name, None, vec![s::RoleImpl {
            role: callable_role(module, closure.callable)?,
            slots: vec![Synthetic::trampoline(module, index)],
        }]);
        object.fields = (0..storage_fields.len() as u32).map(|storage| s::Field {
            name: format!("capture{storage}"), storage, once: false,
        }).collect();
        object.layout.allocation_words = (1 + storage_fields.len() as u32).max(2);
        object.layout.trace_layout = storage_fields.iter().enumerate()
            .filter_map(|(index, &ty)| ty.traced().then_some(index as u32)).collect();
        // A captured resource is selected like a managed field: the closure reaches what it references.
        object.layout.selection_layout = storage_fields.iter().enumerate()
            .filter_map(|(index, &ty)| reaches(module, objects, ty).then_some(index as u32)).collect();
        object.layout.may_reach_resource = !object.layout.selection_layout.is_empty();
        object.layout.storage_fields = storage_fields;
        objects.push(object);
    }
    let first = objects.len() - synthetic.closures.len();
    // The `any` descriptor, then one any box per boxed value type: the value in its one field.
    if synthetic.any {
        objects.push(placeholder("any".into(), None, vec![]));
        for &ty in &synthetic.boxes {
            let value = translate_type(module, ty)?;
            let mut object = placeholder(format!("any#{}", ty.as_u32()), None, vec![]);
            object.any_box = true;
            object.fields = vec![s::Field { name: "value".into(), storage: 0, once: false }];
            object.layout.trace_layout = if value.traced() { vec![0] } else { vec![] };
            object.layout.storage_fields = vec![value];
            objects.push(object);
        }
    }
    // A closure capturing an action value reaches what that value's closures reach, wherever they are listed;
    // an any box reaches what its value reaches.
    loop {
        let mut changed = false;
        for index in first..objects.len() {
            let selection = objects[index].layout.storage_fields.iter().enumerate()
                .filter_map(|(field, &ty)| reaches(module, objects, ty).then_some(field as u32)).collect::<Vec<_>>();
            if selection != objects[index].layout.selection_layout {
                objects[index].layout.may_reach_resource = !selection.is_empty();
                objects[index].layout.selection_layout = selection;
                changed = true;
            }
        }
        if !changed {
            return Ok(());
        }
    }
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

/// The call a trampoline makes after reading its captures: its operation, its leading arguments (before
/// the captures and arguments), its result types, its failures and its retained parameters in the
/// trampoline's language positions (environment first).
struct TrampolineCall {
    op: s::Opcode,
    leading: Vec<A>,
    results: Vec<s::Type>,
    failures: u32,
    retained: Vec<u32>,
}

/// A `fn.closure`/`fn.bind` target: the captures are the target's leading parameters. Its contract is the
/// target's, with every capture position mapped to the environment and the arguments shifted after it.
fn function_call(program: &s::Program, function: o::FuncId, captures: usize, summaries: &mut Vec<s::Summary>)
    -> Result<(TrampolineCall, Vec<s::Type>), GenerateError>
{
    let target = function.as_u32();
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
    if language.len() < captures {
        return Err(GenerateError("closure captures exceed the target's parameters".into()));
    }
    let retained = contract.retained_params.iter().map(|&param| {
        let position = language.iter().position(|(index, _)| *index == param)
            .ok_or_else(|| GenerateError("closure target retains a service".into()))?;
        Ok(if position < captures { 0 } else { (position - captures + 1) as u32 })
    }).collect::<Result<BTreeSet<_>, GenerateError>>()?.into_iter().collect();
    let summary = summaries.len() as u32;
    summaries.push(s::Summary { entry: target, may_fail: contract.failures != 0,
        retained_params: contract.retained_params.clone() });
    let call = TrampolineCall {
        op: s::Opcode::InvokeCallOwn, leading: vec![A::Entry(target), A::Summary(summary), A::Flag(false)],
        results: callee.results.clone(), failures: contract.failures, retained,
    };
    Ok((call, language.iter().map(|(_, ty)| *ty).collect()))
}

/// A `fn.bind.role` target: a dispatch of the slot on the captured receiver, over every implementation (the
/// one object of a concrete receiver). Its contract joins theirs; the slot's positions are the trampoline's.
fn role_call(program: &s::Program, role: o::DeclId, slot: u32, receiver: s::Type)
    -> Result<(TrampolineCall, Vec<s::Type>), GenerateError>
{
    let role = role.as_u32();
    let signature = program.objects.get(role as usize).and_then(|object| object.role.as_ref())
        .and_then(|declaration| declaration.slots.get(slot as usize))
        .ok_or_else(|| GenerateError("bound role slot outside the program".into()))?;
    if signature.entry_abi != s::EntryAbi::Owned {
        return Err(unsupported("bound plain role slot as an action value"));
    }
    let targets = match receiver {
        s::Type::Object(object) => vec![object],
        _ => program.objects.iter().enumerate()
            .filter(|(_, object)| object.implements.iter().any(|table| table.role == role))
            .map(|(id, _)| id as u32).collect(),
    };
    let mut failures = 0;
    let mut retained = BTreeSet::new();
    for &target in &targets {
        let function = s::implementation(program, target, role, slot)
            .and_then(|id| program.functions.get(id as usize))
            .ok_or_else(|| GenerateError("bound role slot without implementation".into()))?;
        let contract = function.proofs.entry.as_ref()
            .ok_or_else(|| GenerateError("role implementation without contract".into()))?;
        failures |= contract.failures;
        let language = function.params.iter().enumerate()
            .filter(|(_, location)| matches!(location, L::Register(_))).map(|(index, _)| index as u32)
            .collect::<Vec<_>>();
        for param in &contract.retained_params {
            let position = language.iter().position(|index| index == param)
                .ok_or_else(|| GenerateError("role implementation retains a service".into()))?;
            retained.insert(position as u32);
        }
    }
    let retained = retained.into_iter().collect::<Vec<_>>();
    let dispatch = s::Dispatch { role, slot, failures, retained_params: retained.clone(), targets };
    let call = TrampolineCall {
        op: s::Opcode::InvokeDispatchOwn, leading: vec![A::Dispatch(dispatch), A::Flag(false)],
        results: signature.results.clone(), failures, retained,
    };
    let mut language = vec![receiver];
    language.extend(&signature.params);
    Ok((call, language))
}

/// The trampoline of a closure layout: reads the captures, makes its target's call with the captures first
/// and the arguments after them, and returns the outcome as it is.
pub(crate) fn trampoline(program: &s::Program, closure: &ClosureLayout, layout: u32,
    summaries: &mut Vec<s::Summary>) -> Result<s::Function, GenerateError>
{
    let object = &program.objects[layout as usize];
    let captures = object.layout.storage_fields.len();
    let (call, language) = match closure.target {
        ClosureTarget::Function(function) => function_call(program, function, captures, summaries)?,
        ClosureTarget::RoleSlot { role, slot } => {
            role_call(program, role, slot, *object.layout.storage_fields.first().ok_or_else(|| {
                GenerateError("bound role slot without receiver".into())
            })?)?
        }
    };
    if language[..captures] != object.layout.storage_fields[..] {
        return Err(GenerateError("closure captures differ from the target's leading parameters".into()));
    }
    let mut f = s::Function {
        symbol: format!("{}.call", object.name), entry_abi: s::EntryAbi::Owned, registers: vec![], services: vec![],
        params: vec![], results: call.results.clone(), blocks: vec![], proofs: s::FunctionProofs::default(),
        code: vec![],
    };
    let environment = fresh_location(&mut f, s::ProducedType::Value(s::Type::Object(layout)));
    let arguments = language[captures..].iter().map(|&ty| fresh_location(&mut f, s::ProducedType::Value(ty)))
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
    let normal = f.code.len() as u32 + 1;
    let unwind = normal + 1;
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
    let failure = (call.failures != 0).then(|| fresh_location(&mut f, s::ProducedType::Value(s::Type::Failure)));
    if let Some(failure) = failure {
        outcomes.push(s::Outcome { tag: s::Tag::Unwind, edge: s::Edge {
            target_pc: unwind, assignments: vec![s::Assignment { destination: failure,
                source: s::Source::ProducedSlot(0) }],
        } });
    }
    let mut call_arguments = call.leading;
    call_arguments.extend(values.iter().map(|&location| A::Location(location)));
    call_arguments.push(A::Location(packet));
    f.code.push(s::Instruction::Champ { op: call.op, arguments: call_arguments, flow: s::Flow::Outcomes(outcomes) });
    f.blocks.push(s::Block { pc: normal, params: normal_params.clone() });
    f.code.push(s::Instruction::Champ { op: s::Opcode::RetOwn,
        arguments: normal_params.into_iter().map(A::Location).collect(), flow: s::Flow::Next(vec![]) });
    if let Some(failure) = failure {
        let L::Register(id) = failure else { unreachable!("register") };
        f.blocks.push(s::Block { pc: unwind, params: vec![failure] });
        f.code.push(s::Instruction::Raise(id));
    }
    f.proofs.entry = Some(s::EntryContract {
        external: None, retained_params: call.retained, fresh_singleton: false, failures: call.failures,
        destroy: None,
    });
    f.proofs.states = s::service_states(&f)?;
    Ok(f)
}

/// Finite origins of an action value: the closure layouts it can hold, or `None` when it is open.
/// Mirrors own's origins: closures and bound actions, through retags and block parameters. Entry
/// parameters, produced edge results and every other definition are open.
pub(crate) fn callable_origins(module: &o::Module, body: &o::Body, synthetic: &Synthetic)
    -> Result<BTreeMap<o::ValueId, Option<BTreeSet<usize>>>, GenerateError>
{
    let mut origins = BTreeMap::new();
    for (value, data) in body.values.iter() {
        if !matches!(module.types.get(data.ty)?, o::Type::Fn(_)) {
            continue;
        }
        let origin = match data.def {
            o::Definition::BlockParam { block, .. } if block == body.entry => None,
            o::Definition::BlockParam { .. } => Some(BTreeSet::new()),
            o::Definition::InstResult { inst, .. } => {
                let op = &body.insts.get(inst)?.op;
                match closure_site(body, op)? {
                    Some(site) => synthetic.closure_index(&site.layout).map(|index| BTreeSet::from([index])),
                    None if matches!(op, o::Operation::Core(o::CoreOp::Retag { .. })) => Some(BTreeSet::new()),
                    None => None,
                }
            }
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
