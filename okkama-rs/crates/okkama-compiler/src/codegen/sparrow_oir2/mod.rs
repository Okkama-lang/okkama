//! Low OIR-2 → SPB 17: scalar/object/control, owning calls and the cleanup automaton.
//! Always compiled, independent of runtime features. compile_low is the executable semantic driver.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use okkama_oir as o;
use okkama_sparrow::spb17 as s;

mod calls;
mod dead;
mod roles;
mod successors;
mod undef;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerateError(pub String);

impl fmt::Display for GenerateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OIR-2 → SPB 17: {}", self.0)
    }
}
impl std::error::Error for GenerateError {}

impl From<o::ArenaError> for GenerateError {
    fn from(error: o::ArenaError) -> Self { Self(error.to_string()) }
}
impl From<s::Error> for GenerateError {
    fn from(error: s::Error) -> Self { Self(error.to_string()) }
}

fn unsupported(name: &str) -> GenerateError {
    let stage = if name.starts_with("cleanup.") || name.starts_with("exit.")
        || matches!(name, "destroy.direct" | "scope.end" | "scope.unwind" | "end")
    {
        "S2"
    } else {
        "S1"
    };
    GenerateError(format!("{name}: not yet in {stage}"))
}

pub use s::layout::{LayoutRegistry, ObjectLayout};

/// Data transformation with structural checks and independently checked allocation.
/// Executable inputs must use compile_low, which also requires semantic Low and loader checks.
pub fn generate(module: &o::Module, layouts: &dyn LayoutRegistry) -> Result<s::Program, GenerateError> {
    Ok(generate_with_allocation(module, layouts)?.program)
}

#[derive(Clone, Debug)]
pub struct AllocatedProgram {
    pub program: s::Program,
    pub allocations: Vec<s::Allocation>,
}

/// Keep the checked frame/copy plans available to the later JIT lowering without changing SPB's SSA form.
pub fn generate_with_allocation(module: &o::Module, layouts: &dyn LayoutRegistry)
    -> Result<AllocatedProgram, GenerateError>
{
    with_composites(module, || generate_program(module, layouts))
}

fn generate_program(module: &o::Module, layouts: &dyn LayoutRegistry) -> Result<AllocatedProgram, GenerateError> {
    let mut p = translate_program(module, layouts)?;
    s::verify(&p)?;
    let redundant = p.functions.iter().map(|function| s::redundant_admission_filters(&p, function))
        .collect::<Result<Vec<_>, _>>()?;
    for (function, pcs) in p.functions.iter_mut().zip(redundant) {
        for pc in pcs {
            if let s::Instruction::Champ { arguments, .. } = &mut function.code[pc] {
                arguments.retain(|argument| !matches!(argument, s::Argument::Flag(_)));
            }
        }
    }
    let mut allocations = Vec::new();
    for function in &p.functions {
        let allocation = s::allocate(function)?;
        s::check_allocation(function, &allocation)?;
        allocations.push(allocation);
    }
    Ok(AllocatedProgram { program: p, allocations })
}

/// The translated program with its relocated evidence, before SPB 17's own structural verification.
fn translate_program(module: &o::Module, layouts: &dyn LayoutRegistry) -> Result<s::Program, GenerateError> {
    if module.profile != o::Profile::Low {
        return Err(GenerateError("expected verified low profile".into()));
    }
    let mut sunk = module.clone();
    for (_, function) in sunk.functions.iter_mut() {
        if let Some(body) = &mut function.body {
            undef::sink_undef_params(body)?;
        }
    }
    let module = &sunk;
    // The test profile carries its tests table; a tests table needs `requires test` and nothing else.
    // Plain async (#601 V7) is part of SPB 17; process workers are not.
    let profile = |feature: &o::Feature| matches!(feature, o::Feature::Core | o::Feature::Async)
        || *feature == o::Feature::Test && module.tests.is_some();
    if !module.imports.is_empty() || !module.requires.iter().all(profile)
        || module.tests.is_some() != module.requires.contains(&o::Feature::Test)
    {
        return Err(unsupported("unresolved declarations or service metadata"));
    }
    // The Services family (D16) is the protocol of Future duties: its settle and await.unwind steps are opcodes,
    // so it needs no SPB 17 protocol. Every dynamic transfer protocol maps to one.
    for (id, declared) in module.protocols.iter() {
        if declared.kind != o::ProtocolKind::Services {
            protocol(module, id)?;
        }
    }
    let tests = module.tests.as_ref().map(|tests| tests.iter().map(|test| s::Test {
        name: test.name.clone(),
        function: test.function.as_u32(),
    }).collect());
    let mut p = s::Program {
        module: module.name.clone(),
        version: module.version.clone().unwrap_or_default(),
        entry: module.entry.map(|id| id.as_u32()),
        tests,
        ..s::Program::default()
    };
    p.objects = objects(module, layouts)?;
    let synthetic = roles::Synthetic::of(module)?;
    p.composites = composites(module).entries().to_vec();
    p.tables.future_results = composites(module).futures().to_vec();
    p.globals = globals(module)?;
    for (id, scope) in module.scopes.iter() {
        p.tables.scopes.push(s::Scope {
            function: scope.function.as_u32(),
            parent: scope.parent.map(|id| id.as_u32()),
            boundary: id.as_u32(),
        });
        p.tables.boundaries.push(s::Boundary { scope: id.as_u32() });
    }
    for (id, function) in module.functions.iter() {
        if let Some(body) = &function.body {
            for (_, value) in body.values.iter() {
                let ty = translate_type(module, value.ty);
                let collection = matches!(ty, Ok(s::Type::Array(_) | s::Type::Map(_)));
                let plain = value.facts == (o::ValueFacts::Plain {
                    certificate: o::PlainCertificate::ClosedGraph { ty: value.ty },
                });
                // A managed array or map refers to resource objects: own proves its deps on their owners.
                let leaf = match ty {
                    Ok(s::Type::Array(shape)) => s::ArrayType { depth: 1, ..shape }.element(),
                    Ok(s::Type::Map(id)) => match composites(module).entries().get(id as usize) {
                        Some(s::Composite::Map { value, .. }) => Some(*value),
                        _ => None,
                    },
                    _ => None,
                };
                let managed = matches!(value.facts, o::ValueFacts::Managed { .. })
                    && matches!(leaf, Some(s::Type::Object(id))
                        if p.objects.get(id as usize).is_some_and(|object| object.layout.may_reach_resource));
                // Own tracks role, cap and action-value elements as references; the loader admits such an
                // array only when every object it can reach is closed plain data (ARRAY_PLAIN).
                let view = roles::view_array(module, value.ty)? && matches!(value.facts, o::ValueFacts::Managed { .. });
                if collection && !plain && !managed && !view {
                    return Err(GenerateError("plain array or map requires own ClosedGraph certificate".into()));
                }
            }
        }
        let translated = if calls::needs_s2(module, function)? {
            calls::translate(module, id, function, &p.objects, &mut p.tables, &synthetic)?
        } else {
            translate_function(module, id, function, &p.objects)?
        };
        p.functions.push(translated);
    }
    let mut trampolines = vec![];
    let mut summaries = std::mem::take(&mut p.tables.summaries);
    for (index, closure) in synthetic.closures.iter().enumerate() {
        let layout = synthetic.closure_object(module, index);
        trampolines.push(roles::trampoline(&p, closure, layout, &mut summaries)?);
        debug_assert_eq!(roles::Synthetic::trampoline(module, index) as usize, p.functions.len() + index);
    }
    p.tables.summaries = summaries;
    p.functions.extend(trampolines);
    for function in &mut p.functions {
        dead::prune_unreachable(function)?;
        successors::forward_taken(function)?;
        successors::prune_dead_aliases(function);
        successors::close_unwind_permissions(function)?;
        complete_codegen_roots(function)?;
    }
    Ok(p)
}

/// The SPB 17 protocol of a declared dynamic transfer protocol: the registered open Closure protocol, or the
/// synchronous `any` protocol (one checked packet-to-scope transfer, as Low admits it). Services protocols are
/// carried by opcodes (see `program`); any other protocol (storage merges) has no SPB 17 form yet.
pub(crate) fn protocol(module: &o::Module, id: o::ProtocolId) -> Result<s::Protocol, GenerateError> {
    let declared = module.protocols.get(id)?;
    if *declared == o::catalogue::open_callable_protocol() {
        return Ok(s::Protocol::Closure);
    }
    let open = o::ProtocolPoststate { holder: o::HolderKind::Scope, state: o::HolderState::Open };
    let unaccepted = o::ProtocolPoststate { holder: o::HolderKind::Packet, state: o::HolderState::Unaccepted };
    let transfer = |rule: &o::ProtocolRule| rule.transition == o::ProtocolTransition::Dynamic
        && rule.from == o::HolderKind::Packet && rule.to == o::HolderKind::Scope
        && rule.normal == open && rule.unwind == Some(unaccepted) && rule.cancel.is_none()
        && [o::ProtocolCheck::Owner, o::ProtocolCheck::LiveDependencies, o::ProtocolCheck::Snapshot,
            o::ProtocolCheck::Transferable].iter().all(|check| rule.checks.contains(check));
    if declared.kind == o::ProtocolKind::Any && declared.holders == [o::HolderKind::Scope, o::HolderKind::Packet]
        && matches!(declared.rules.as_slice(), [rule] if transfer(rule)) && declared.roots == [o::RootKind::Operands]
    {
        return Ok(s::Protocol::Any);
    }
    Err(unsupported(&format!("{:?} protocol", declared.kind)))
}

pub(crate) fn objects(module: &o::Module, layouts: &dyn LayoutRegistry) -> Result<Vec<s::Object>, GenerateError> {
    let mut objects = Vec::new();
    for (id, declaration) in module.declarations.iter() {
        if !declaration.type_params.is_empty() {
            return Err(unsupported("generic object declaration"));
        }
        let (fields, witnesses, destroy) = match &declaration.kind {
            o::DeclarationKind::Object { fields, witnesses, destroy, .. } => {
                (fields.as_slice(), witnesses.as_slice(), *destroy)
            }
            o::DeclarationKind::Enum { .. } | o::DeclarationKind::Variant { .. } => (&[][..], &[][..], None),
            // A cap value is its owner's reference: the cap declaration is a placeholder descriptor.
            o::DeclarationKind::Role { .. } | o::DeclarationKind::Cap { .. } => (&[][..], &[][..], None),
            o::DeclarationKind::Handle => (&[][..], &[][..], None),
            // A module constant is laid out in `Program::globals`; its declaration keeps a placeholder
            // descriptor so that descriptor indices stay declaration indices.
            o::DeclarationKind::Global { .. } => (&[][..], &[][..], None),
        };
        let role = match &declaration.kind {
            o::DeclarationKind::Role { builtin: true, .. } if declaration.name == "Error" => None,
            o::DeclarationKind::Role { methods, .. } => Some(roles::role_decl(module, methods)?),
            _ => None,
        };
        let implements = roles::implementations(module, witnesses)?;
        let layout = layouts.object_layout(id.as_u32())?;
        if let o::DeclarationKind::Variant { cases, is_error: false } = &declaration.kind {
            for case in cases {
                let case = layouts.object_layout(case.layout.as_u32())?;
                // A case may refer to resources (Maybe<Resource>); it is never an identity itself.
                if case.runtime.has_destroy {
                    return Err(unsupported("variant case with destroy"));
                }
            }
        }
        if layout.runtime.has_destroy != destroy.is_some() {
            return Err(GenerateError("descriptor destroy/layout mismatch".into()));
        }
        if fields.len() != layout.source_fields.len() {
            return Err(GenerateError("layout registry field mapping arity mismatch".into()));
        }
        for (field, &physical) in fields.iter().zip(&layout.source_fields) {
            if layout.runtime.storage_fields.get(physical as usize) != Some(&translate_type(module, field.ty)?) {
                return Err(GenerateError("layout registry field type mismatch".into()));
            }
        }
        objects.push(s::Object {
            error_cases: match &declaration.kind {
                o::DeclarationKind::Enum { cases, is_error: true } => cases.iter().map(|case| {
                    format!("{}.{case}", declaration.name.rsplit('.').next().unwrap())
                }).collect(),
                _ => module.declarations.iter().find_map(|(_, parent)| {
                    let o::DeclarationKind::Variant { cases, is_error: true } = &parent.kind else { return None };
                    cases.iter().find(|case| case.layout == id).map(|case| {
                        vec![format!("{}.{}", parent.name.rsplit('.').next().unwrap(), case.name)]
                    })
                }).unwrap_or_default(),
            },
            variant_cases: match &declaration.kind {
                o::DeclarationKind::Variant { cases, .. } => {
                    cases.iter().map(|case| case.layout.as_u32()).collect()
                }
                _ => vec![],
            },
            enum_cases: match &declaration.kind {
                o::DeclarationKind::Enum { cases, is_error: false } => {
                    u32::try_from(cases.len()).map_err(|_| GenerateError("too many enum cases".into()))?
                }
                _ => 0,
            },
            handle: matches!(declaration.kind, o::DeclarationKind::Handle),
            name: declaration.name.clone(),
            fields: fields.iter().zip(&layout.source_fields).map(|(field, &storage)| s::Field {
                name: field.name.clone(),
                storage,
                once: field.once,
            }).collect(),
            layout: layout.runtime,
            role,
            implements,
            any_box: false,
            // A plain enum names its cases as Tomato's texts do (`Enum.case`): an Error payload shows them.
            enum_names: match &declaration.kind {
                o::DeclarationKind::Enum { cases, is_error: false } => cases.iter().map(|case| {
                    format!("{}.{case}", declaration.name.rsplit('.').next().unwrap())
                }).collect(),
                _ => vec![],
            },
        });
    }
    roles::synthetic_objects(module, &mut objects)?;
    Ok(objects)
}

/// SPB 17 collection shapes and Future outcomes of one module, in OIR type order. Equal shapes share one
/// entry, and so do Futures of one outcome type (`ChampTables::future_results` entries are distinct).
#[derive(Debug, Default)]
pub(crate) struct Composites {
    index: std::collections::HashMap<o::TypeId, u32>,
    entries: Vec<s::Composite>,
    future_index: std::collections::HashMap<o::TypeId, u32>,
    futures: Vec<Option<s::Type>>,
}

impl Composites {
    fn build(module: &o::Module) -> Self {
        let mut table = Self::default();
        for (id, ty) in module.types.iter() {
            // A type is interned after the types it names, so its outcome is already in the table.
            if let o::Type::Future(result) = ty {
                let outcome = match result {
                    Some(result) => match translate_with(module, *result, &table) {
                        Ok(outcome) => Some(outcome),
                        Err(_) => continue,
                    },
                    None => None,
                };
                let position = match table.futures.iter().position(|entry| *entry == outcome) {
                    Some(position) => position,
                    None => {
                        table.futures.push(outcome);
                        table.futures.len() - 1
                    }
                };
                table.future_index.insert(id, position as u32);
                continue;
            }
            let composite = match ty {
                o::Type::Map(key, value) | o::Type::MapBuilder(key, value) => {
                    match (translate_with(module, *key, &table), translate_with(module, *value, &table)) {
                        (Ok(key), Ok(value)) => s::Composite::Map { key, value },
                        _ => continue,
                    }
                }
                o::Type::Builder(element) => match translate_with(module, *element, &table) {
                    Ok(element) => match s::ArrayType::of(element) {
                        Some(array) => s::Composite::Builder { array },
                        None => continue,
                    },
                    Err(_) => continue,
                },
                _ => continue,
            };
            let position = match table.entries.iter().position(|entry| *entry == composite) {
                Some(position) => position,
                None => {
                    table.entries.push(composite);
                    table.entries.len() - 1
                }
            };
            table.index.insert(id, position as u32);
        }
        table
    }

    pub(crate) fn entries(&self) -> &[s::Composite] {
        &self.entries
    }

    /// `ChampTables::future_results`: the outcome of each `Type::Future` index; `None` is a void Future.
    pub(crate) fn futures(&self) -> &[Option<s::Type>] {
        &self.futures
    }
}

thread_local! {
    /// The table of the module being generated, keyed by its address; outside `with_composites` every
    /// lookup rebuilds the table, so a stale entry can never be read.
    static COMPOSITES: std::cell::RefCell<Option<(usize, std::rc::Rc<Composites>)>> =
        const { std::cell::RefCell::new(None) };
}

fn module_key(module: &o::Module) -> usize {
    module as *const o::Module as usize
}

/// Run `body` with the module's composite table built once.
pub(crate) fn with_composites<T>(module: &o::Module, body: impl FnOnce() -> T) -> T {
    struct Restore(Option<(usize, std::rc::Rc<Composites>)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            COMPOSITES.with(|slot| *slot.borrow_mut() = previous);
        }
    }
    let table = std::rc::Rc::new(Composites::build(module));
    let previous = COMPOSITES.with(|slot| slot.borrow_mut().replace((module_key(module), table)));
    let _restore = Restore(previous);
    body()
}

pub(crate) fn composites(module: &o::Module) -> std::rc::Rc<Composites> {
    let cached = COMPOSITES.with(|slot| slot.borrow().as_ref()
        .filter(|(key, _)| *key == module_key(module)).map(|(_, table)| table.clone()));
    cached.unwrap_or_else(|| std::rc::Rc::new(Composites::build(module)))
}

pub(crate) fn translate_type(module: &o::Module, id: o::TypeId) -> Result<s::Type, GenerateError> {
    translate_with(module, id, &composites(module))
}

fn translate_with(module: &o::Module, id: o::TypeId, table: &Composites) -> Result<s::Type, GenerateError> {
    Ok(match module.types.get(id)? {
        o::Type::Int => s::Type::Int,
        o::Type::Array(element) => {
            let element = translate_with(module, *element, table)?;
            s::ArrayType::of(element).ok_or_else(|| unsupported("array element type"))?
        }
        o::Type::Map(..) | o::Type::MapBuilder(..) | o::Type::Builder(_) => {
            let Some(&index) = table.index.get(&id) else {
                let parts = match module.types.get(id)? {
                    o::Type::Map(key, value) | o::Type::MapBuilder(key, value) => vec![*key, *value],
                    o::Type::Builder(element) => vec![*element],
                    _ => vec![],
                };
                for part in parts {
                    translate_with(module, part, table)?;
                }
                return Err(unsupported("collection shape outside SPB 17"));
            };
            match module.types.get(id)? {
                o::Type::Map(..) => s::Type::Map(index),
                o::Type::MapBuilder(..) => s::Type::MapBuilder(index),
                _ => s::Type::Builder(index),
            }
        }
        o::Type::Role { decl, args } if args.is_empty() && module.declarations.get(*decl)?.name == "Error" => {
            s::Type::Error
        }
        o::Type::Role { decl, args } if args.is_empty() => s::Type::Role(decl.as_u32()),
        o::Type::Fn(_) => s::Type::Role(roles::callable_role(module, id)?),
        // An `any` value is a reference to an any box: the boxed value and, in its descriptor, its type.
        o::Type::Any => s::Type::Any,
        // A cap holds its owner's complete graph and reads it on every call: it is the owner's reference.
        o::Type::Cap { decl, args } if args.is_empty() => {
            let o::DeclarationKind::Cap { owner, .. } = &module.declarations.get(*decl)?.kind else {
                return Err(GenerateError("cap type references a non-cap declaration".into()));
            };
            match translate_with(module, *owner, table)? {
                owner @ s::Type::Object(_) => owner,
                _ => return Err(unsupported("cap of a non-object owner")),
            }
        }
        o::Type::Enum(decl) => s::Type::Object(decl.as_u32()),
        o::Type::Variant(decl) => {
            let o::DeclarationKind::Variant { cases, is_error } = &module.declarations.get(*decl)?.kind else {
                return Err(GenerateError("variant type references non-variant declaration".into()));
            };
            // A one-case Error variant is its case object (#703); an Error variant with several cases is a
            // variant whose values are its case objects, which are Error values (#705).
            match cases.as_slice() {
                [case] if *is_error => s::Type::Object(case.layout.as_u32()),
                _ => s::Type::Variant(decl.as_u32()),
            }
        }
        o::Type::Descriptor => s::Type::Descriptor,
        o::Type::Handle(decl) => {
            if !matches!(module.declarations.get(*decl)?.kind, o::DeclarationKind::Handle) {
                return Err(GenerateError("handle type references a non-handle declaration".into()));
            }
            s::Type::Handle(decl.as_u32())
        }
        o::Type::Bool => s::Type::Bool,
        o::Type::Byte => s::Type::Byte,
        o::Type::Float => s::Type::Float,
        o::Type::Rat => s::Type::Rat,
        o::Type::Char => s::Type::Char,
        o::Type::Str => s::Type::Str,
        o::Type::Range => s::Type::Range,
        o::Type::Never => s::Type::Never,
        o::Type::Service(o::ServiceType::Failure) => s::Type::Failure,
        o::Type::Object { decl, args } if args.is_empty() => {
            if !matches!(module.declarations.get(*decl)?.kind, o::DeclarationKind::Object { .. }) {
                return Err(GenerateError("object type references non-object declaration".into()));
            }
            s::Type::Object(decl.as_u32())
        }
        o::Type::Successor(base) => match module.types.get(*base)? {
            o::Type::Object { decl, args } if args.is_empty() => s::Type::Successor(decl.as_u32()),
            o::Type::Role { decl, args } if args.is_empty() && module.declarations.get(*decl)?.name != "Error" => {
                s::Type::RoleSuccessor(decl.as_u32())
            }
            _ => return Err(unsupported("non-object successor type")),
        },
        o::Type::Future(result) => match table.future_index.get(&id) {
            Some(&index) => s::Type::Future(index),
            None => {
                // The outcome type decides the error: a Future of an SPB 17 type is always in the table.
                if let Some(result) = result {
                    translate_with(module, *result, table)?;
                }
                return Err(unsupported("Future outcome outside SPB 17"));
            }
        },
        ty => return Err(unsupported(&format!("type {ty:?}"))),
    })
}

/// The index in `Program::globals` of a module constant: its position among the global declarations.
fn global_index(module: &o::Module, declaration: o::DeclId) -> Result<u32, GenerateError> {
    let position = module.declarations.iter()
        .filter(|(_, declared)| matches!(declared.kind, o::DeclarationKind::Global { .. }))
        .position(|(id, _)| id == declaration)
        .ok_or_else(|| GenerateError("global read of a declaration that is not a global".into()))?;
    u32::try_from(position).map_err(|_| GenerateError("too many globals".into()))
}

fn global_init(module: &o::Module, declaration: o::DeclId) -> Result<o::FuncId, GenerateError> {
    match module.declarations.get(declaration)?.kind {
        o::DeclarationKind::Global { init, .. } => Ok(init),
        _ => Err(GenerateError("global read of a declaration that is not a global".into())),
    }
}

/// The SPB 17 layout of the module constants (#709): each global's value as data, read from its init
/// function. The producer builds that body from literals, `enum.case`, `array.new` and `map.new` only
/// (`lowering::oir2::globals`), so the data are the value the init would compute, children first.
fn globals(module: &o::Module) -> Result<Vec<s::Global>, GenerateError> {
    let mut globals = Vec::new();
    for (_, declaration) in module.declarations.iter() {
        let o::DeclarationKind::Global { ty, init } = declaration.kind else { continue };
        let function = module.functions.get(init)?;
        let body = function.body.as_ref().ok_or_else(|| unsupported("global without an init body"))?;
        let mut definitions = BTreeMap::new();
        for (_, block) in body.blocks.iter() {
            for &inst in &block.insts {
                let instruction = body.insts.get(inst)?;
                if let [result] = instruction.results.as_slice() {
                    definitions.insert(*result, &instruction.op);
                }
            }
        }
        let returned = body.blocks.iter().find_map(|(_, block)| match &block.term.kind {
            o::TermKind::Ret { values } => values.first().copied(),
            _ => None,
        }).ok_or_else(|| unsupported("global init without a returned value"))?;
        let mut data = Vec::new();
        let mut built = BTreeMap::new();
        datum(module, body, &definitions, returned, &mut data, &mut built)?;
        let ty = translate_type(module, ty)?;
        if data.last().map(|datum| datum.ty) != Some(ty) {
            return Err(GenerateError("global init returns another type".into()));
        }
        globals.push(s::Global { name: declaration.name.clone(), ty, data });
    }
    Ok(globals)
}

/// The datum of one init value, after the data of its parts; each value becomes one datum.
fn datum(module: &o::Module, body: &o::Body, definitions: &BTreeMap<o::ValueId, &o::Operation>,
    value: o::ValueId, data: &mut Vec<s::Datum>, built: &mut BTreeMap<o::ValueId, u32>) -> Result<u32, GenerateError>
{
    if let Some(&index) = built.get(&value) {
        return Ok(index);
    }
    let ty = translate_type(module, body.values.get(value)?.ty)?;
    let operation = definitions.get(&value).ok_or_else(|| unsupported("global init value outside its data"))?;
    let mut parts = |values: &[o::ValueId], data: &mut Vec<s::Datum>| values.iter()
        .map(|&part| datum(module, body, definitions, part, data, built)).collect::<Result<Vec<_>, _>>();
    let value_datum = match operation {
        o::Operation::Const(literal) => s::DatumValue::Constant(translate_constant(literal)?),
        o::Operation::Core(o::CoreOp::EnumCase { case, .. }) => s::DatumValue::Case(*case),
        o::Operation::Collection(o::CollectionOp::ArrayNew { values, packet: None, .. }) => {
            s::DatumValue::Array(parts(values, data)?)
        }
        o::Operation::Collection(o::CollectionOp::MapNew { entries, packet: None, .. }) => {
            let values = entries.iter().flat_map(|&(key, item)| [key, item]).collect::<Vec<_>>();
            s::DatumValue::Map(parts(&values, data)?)
        }
        _ => return Err(unsupported(&format!("global init operation {}", operation.opcode()))),
    };
    let index = u32::try_from(data.len()).map_err(|_| GenerateError("too many global data".into()))?;
    data.push(s::Datum { ty, value: value_datum });
    built.insert(value, index);
    Ok(index)
}

/// The case layout of a one-case Error variant type, which SPB 17 represents by that case object
/// (`translate_type`); `None` for every other type.
pub(crate) fn single_error_case(module: &o::Module, ty: o::TypeId) -> Result<Option<o::DeclId>, GenerateError> {
    let o::Type::Variant(decl) = module.types.get(ty)? else { return Ok(None) };
    Ok(match &module.declarations.get(*decl)?.kind {
        o::DeclarationKind::Variant { cases, is_error: true } if cases.len() == 1 => Some(cases[0].layout),
        _ => None,
    })
}

/// The storage index of a variant case payload field.
pub(crate) fn variant_storage(module: &o::Module, objects: &[s::Object], field: o::FieldRef)
    -> Result<u32, GenerateError>
{
    let declaration = module.declarations.get(field.declaration)?;
    if !matches!(declaration.kind, o::DeclarationKind::Object { .. }) {
        return Err(GenerateError("variant payload field outside a case object".into()));
    }
    objects.get(field.declaration.as_u32() as usize).and_then(|case| case.fields.get(field.index as usize))
        .map(|field| field.storage).ok_or_else(|| GenerateError("invalid variant payload field".into()))
}

/// The storage of a once slot: a source field marked `once` (#491).
fn once_field(module: &o::Module, objects: &[s::Object], field: o::FieldRef) -> Result<u32, GenerateError> {
    let o::DeclarationKind::Object { fields, .. } = &module.declarations.get(field.declaration)?.kind else {
        return Err(GenerateError("once slot outside an object".into()));
    };
    if !fields.get(field.index as usize).is_some_and(|source| source.once) {
        return Err(GenerateError("once.load/once.store needs a once slot".into()));
    }
    objects.get(field.declaration.as_u32() as usize).and_then(|object| object.fields.get(field.index as usize))
        .map(|field| field.storage).ok_or_else(|| GenerateError("invalid once slot".into()))
}

/// The SPB 17 type of an OIR-2 conversion operand.
pub(crate) fn numeric_type(ty: o::NumericType) -> Option<s::Type> {
    match ty {
        o::NumericType::Int => Some(s::Type::Int),
        o::NumericType::Bool => Some(s::Type::Bool),
        o::NumericType::Str => Some(s::Type::Str),
        o::NumericType::Byte => Some(s::Type::Byte),
        o::NumericType::Float => Some(s::Type::Float),
        o::NumericType::Rat => Some(s::Type::Rat),
        o::NumericType::Char => Some(s::Type::Char),
    }
}

/// The SPB 17 operation of an OIR-2 int, byte, float, rat or str operation of the scalar families
/// (wire codes 14..=18) and its operands in order. Both IRs give these operations the same operand and
/// result types; `int.div` and `rat.new` stay `RatNew`.
pub(crate) fn family_operation(core: &o::CoreOp) -> Option<(s::DataOp, Vec<o::ValueId>)> {
    use o::CoreOp as C;
    use s::DataOp as D;
    Some(match *core {
        C::IntAnd { left, right } => (D::IntAnd, vec![left, right]),
        C::IntOr { left, right } => (D::IntOr, vec![left, right]),
        C::IntXor { left, right } => (D::IntXor, vec![left, right]),
        C::IntShl { left, right } => (D::IntShl, vec![left, right]),
        C::IntShr { left, right } => (D::IntShr, vec![left, right]),
        C::IntUshr { left, right } => (D::IntUshr, vec![left, right]),
        C::IntNot { value } => (D::IntNot, vec![value]),
        C::IntAbs { value } => (D::IntAbs, vec![value]),
        C::IntSign { value } => (D::IntSign, vec![value]),
        C::IntBitCount { value } => (D::IntBitCount, vec![value]),
        C::IntLeadingZeros { value } => (D::IntLeadingZeros, vec![value]),
        C::IntTrailingZeros { value } => (D::IntTrailingZeros, vec![value]),
        C::IntReverseBits { value } => (D::IntReverseBits, vec![value]),
        C::IntSwapBytes { value } => (D::IntSwapBytes, vec![value]),
        C::IntRotateLeft { value, count } => (D::IntRotl, vec![value, count]),
        C::IntRotateRight { value, count } => (D::IntRotr, vec![value, count]),
        C::ByteAnd { left, right } => (D::ByteAnd, vec![left, right]),
        C::ByteOr { left, right } => (D::ByteOr, vec![left, right]),
        C::ByteXor { left, right } => (D::ByteXor, vec![left, right]),
        C::ByteShl { left, right } => (D::ByteShl, vec![left, right]),
        C::ByteShr { left, right } => (D::ByteShr, vec![left, right]),
        C::ByteUshr { left, right } => (D::ByteUshr, vec![left, right]),
        C::ByteNot { value } => (D::ByteNot, vec![value]),
        C::ByteBitCount { value } => (D::ByteBitCount, vec![value]),
        C::ByteLeadingZeros { value } => (D::ByteLeadingZeros, vec![value]),
        C::ByteTrailingZeros { value } => (D::ByteTrailingZeros, vec![value]),
        C::ByteReverseBits { value } => (D::ByteReverseBits, vec![value]),
        C::ByteSwapBytes { value } => (D::ByteSwapBytes, vec![value]),
        C::ByteRotateLeft { value, count } => (D::ByteRotl, vec![value, count]),
        C::ByteRotateRight { value, count } => (D::ByteRotr, vec![value, count]),
        C::FloatAdd { left, right } => (D::FloatAdd, vec![left, right]),
        C::FloatSub { left, right } => (D::FloatSub, vec![left, right]),
        C::FloatMul { left, right } => (D::FloatMul, vec![left, right]),
        C::FloatDiv { left, right } => (D::FloatDiv, vec![left, right]),
        C::FloatMod { left, right } => (D::FloatMod, vec![left, right]),
        C::FloatPow { left, right } => (D::FloatPow, vec![left, right]),
        C::FloatNeg { value } => (D::FloatNeg, vec![value]),
        C::FloatAbs { value } => (D::FloatAbs, vec![value]),
        C::FloatSign { value } => (D::FloatSign, vec![value]),
        C::FloatSqrt { value } => (D::FloatSqrt, vec![value]),
        C::RatAdd { left, right } => (D::RatAdd, vec![left, right]),
        C::RatSub { left, right } => (D::RatSub, vec![left, right]),
        C::RatMul { left, right } => (D::RatMul, vec![left, right]),
        C::RatDiv { left, right } => (D::RatDiv, vec![left, right]),
        C::RatMod { left, right } => (D::RatMod, vec![left, right]),
        C::RatNeg { value } => (D::RatNeg, vec![value]),
        C::RatAbs { value } => (D::RatAbs, vec![value]),
        C::RatSign { value } => (D::RatSign, vec![value]),
        C::RatNumerator { value } => (D::RatNumerator, vec![value]),
        C::RatDenominator { value } => (D::RatDenominator, vec![value]),
        C::StrSlice { value, start, end } => (D::StrSlice, vec![value, start, end]),
        C::StrSplit { value, separator } => (D::StrSplit, vec![value, separator]),
        C::StrReplace { value, target, replacement } => (D::StrReplace, vec![value, target, replacement]),
        C::StrReplaceFirst { value, target, replacement } => {
            (D::StrReplaceFirst, vec![value, target, replacement])
        }
        C::StrRepeat { value, count } => (D::StrRepeat, vec![value, count]),
        C::StrTrim { value } => (D::StrTrim, vec![value]),
        C::StrChomp { value } => (D::StrChomp, vec![value]),
        C::StrTrimStart { value } => (D::StrTrimStart, vec![value]),
        C::StrTrimEnd { value } => (D::StrTrimEnd, vec![value]),
        C::StrLower { value } => (D::StrLower, vec![value]),
        C::StrUpper { value } => (D::StrUpper, vec![value]),
        C::StrFind { value, part } => (D::StrFind, vec![value, part]),
        C::StrFindLast { value, part } => (D::StrFindLast, vec![value, part]),
        C::StrStartsWith { value, part } => (D::StrStartsWith, vec![value, part]),
        C::StrEndsWith { value, part } => (D::StrEndsWith, vec![value, part]),
        C::StrUtf8 { value } => (D::StrUtf8, vec![value]),
        C::StrCharacters { value } => (D::StrCharacters, vec![value]),
        _ => return None,
    })
}

/// A scalar family operation that OIR-2 lets trap but that cannot fail in SPB 17 or in Tomato
/// (`byte.and`, `float.neg`, …): it has no failure to deliver to an unwind continuation.
pub(crate) fn total_family_operation(op: &o::Operation) -> bool {
    matches!(op, o::Operation::Core(core) if family_operation(core).is_some_and(|(op, _)| !op.fallible(&[], None)))
}

fn translate_constant(literal: &o::Literal) -> Result<s::Constant, GenerateError> {
    Ok(match literal {
        o::Literal::Int(value) => s::Constant::Int(value.clone()),
        o::Literal::Bool(value) => s::Constant::Bool(*value),
        o::Literal::Str(value) => s::Constant::Str(value.clone()),
        o::Literal::Byte(value) => s::Constant::Byte(u32::from(*value)),
        o::Literal::Char(value) => s::Constant::Char(u32::from(*value)),
        o::Literal::Float(bits) => s::Constant::Float(bits.to_string()),
        // OIR-2 keeps a rat literal in lowest terms with a positive denominator, as CONST_RAT requires.
        o::Literal::Rat { numerator, denominator } => s::Constant::Rat(s::RatValue {
            numerator: numerator.clone(), denominator: denominator.clone(),
        }),
        _ => return Err(unsupported("constant outside int/bool/byte/char/float/rat/string")),
    })
}

/// Attributes that only say which declaration a function implements: an action, a successor, a field
/// default or the body of an action value. They carry no ABI of their own; the signature and the
/// ownership ABI do (an action value's captures are its leading parameters).
pub(super) fn declaration_attr(attr: &o::FunctionAttr) -> bool {
    matches!(attr, o::FunctionAttr::Method(_) | o::FunctionAttr::Successor(_) | o::FunctionAttr::Default { .. }
        | o::FunctionAttr::Lambda { .. })
}

fn translate_function(module: &o::Module, id: o::FuncId, function: &o::Function, objects: &[s::Object])
    -> Result<s::Function, GenerateError>
{
    if !function.type_params.is_empty() || function.summary.is_some() || function.async_creation_summary.is_some()
        || function.async_body_summary.is_some() || function.signature.ownership_abi != o::OwnershipAbi::Observation
        || function.attrs.iter().any(|attr| {
            !declaration_attr(attr) && !matches!(attr, o::FunctionAttr::Entry | o::FunctionAttr::Hidden)
        })
    {
        return Err(unsupported(&format!(
            "{}: non-observation entry ABI, async/generic/native or ownership entry", function.symbol,
        )));
    }
    let body = function.body.as_ref().ok_or_else(|| unsupported("declaration without body"))?;
    if !body.bindings.is_empty() || !body.boundaries.is_empty() || !body.deps.is_empty()
        || !body.keys.is_empty() || !body.selections.is_empty()
    {
        return Err(unsupported("ownership/proof metadata"));
    }
    let mut order = vec![body.entry];
    order.extend(body.blocks.iter().map(|(id, _)| id).filter(|&id| id != body.entry));
    let mut pcs = BTreeMap::new();
    let mut pc = 0u32;
    let mut definitions = BTreeSet::new();
    let mut instructions = BTreeSet::new();
    for &block_id in &order {
        let block = body.blocks.get(block_id)?;
        if module.scopes.get(block.lexical_scope)?.function != id {
            return Err(GenerateError("block scope belongs to another function".into()));
        }
        if block.unwind.is_some() {
            return Err(unsupported("application unwind edges"));
        }
        pcs.insert(block_id, pc);
        let mut length = match &block.term.kind {
            o::TermKind::Switch { cases, .. } => cases.len().checked_mul(3).and_then(|n| n.checked_add(1)),
            _ => Some(1),
        }.ok_or_else(|| GenerateError("too much code".into()))?;
        for &id in &block.insts {
            let size = if matches!(body.insts.get(id)?.op, o::Operation::Core(o::CoreOp::RangeLen { .. })) {
                3
            } else {
                1
            };
            length = length.checked_add(size).ok_or_else(|| GenerateError("too much code".into()))?;
        }
        pc = pc.checked_add(u32::try_from(length).map_err(|_| GenerateError("too much code".into()))?)
            .ok_or_else(|| GenerateError("too much code".into()))?;
        for (index, &value) in block.params.iter().enumerate() {
            check_definition(body, value, o::Definition::BlockParam { block: block_id, index: index as u32 },
                &mut definitions)?;
        }
        for &inst_id in &block.insts {
            if !instructions.insert(inst_id) {
                return Err(GenerateError("duplicate instruction in input".into()));
            }
            let inst = body.insts.get(inst_id)?;
            if inst.facts != o::InstructionFacts::default() {
                return Err(unsupported("instruction ownership facts"));
            }
            for (index, &value) in inst.results.iter().enumerate() {
                let index = u16::try_from(index).map_err(|_| GenerateError("too many results".into()))?;
                check_definition(body, value, o::Definition::InstResult { inst: inst_id, index }, &mut definitions)?;
            }
        }
    }
    if definitions.len() != body.values.len() || instructions.len() != body.insts.len() {
        return Err(GenerateError("orphan SSA value or instruction in input".into()));
    }
    let entry = body.blocks.get(body.entry)?;
    let param_types = entry.params.iter().map(|&value| Ok(body.values.get(value)?.ty))
        .collect::<Result<Vec<_>, GenerateError>>()?;
    if param_types != function.signature.params {
        return Err(GenerateError("entry parameter signature mismatch".into()));
    }
    let edge = |target: &o::Target| -> Result<s::Edge, GenerateError> {
        let params = &body.blocks.get(target.block)?.params;
        if params.len() != target.args.len() {
            return Err(GenerateError("bad input edge arity".into()));
        }
        let assignments = params.iter().zip(&target.args).map(|(&dst, &src)| s::Assignment {
            destination: s::Location::Register(dst.as_u32()),
            source: s::Source::Location(s::Location::Register(src.as_u32())),
        }).collect();
        Ok(s::Edge { target_pc: pcs[&target.block], assignments })
    };
    let registers = body.values.iter().map(|(_, value)| translate_type(module, value.ty))
        .collect::<Result<Vec<_>, _>>()?;
    for (value, data) in body.values.iter() {
        if let o::ValueFacts::Plain { certificate } = &data.facts {
            let valid = match certificate {
                o::PlainCertificate::Scalar => matches!(registers[value.as_u32() as usize],
                    s::Type::Int | s::Type::Bool | s::Type::Str | s::Type::Range | s::Type::Never
                    | s::Type::Byte | s::Type::Float | s::Type::Char | s::Type::Rat),
                o::PlainCertificate::ClosedGraph { ty } => *ty == data.ty,
                o::PlainCertificate::Descriptor(_) => return Err(unsupported("descriptor plainness evidence")),
            };
            if !valid {
                return Err(GenerateError("invalid S1 plainness certificate".into()));
            }
        }
        if let s::Type::Object(id) | s::Type::Successor(id) = registers[value.as_u32() as usize]
            && objects.get(id as usize).is_some_and(|object| object.layout.may_reach_resource)
        {
            return Err(unsupported("managed object value needs ownership/deps verification"));
        }
        if let s::Type::Role(id) | s::Type::RoleSuccessor(id) = registers[value.as_u32() as usize]
            && objects.iter().any(|object| object.layout.may_reach_resource
                && object.implements.iter().any(|table| table.role == id))
        {
            return Err(unsupported("managed role value needs ownership/deps verification"));
        }
    }
    let mut result = s::Function {
        symbol: function.symbol.clone(),
        entry_abi: s::EntryAbi::Observation,
        registers,
        services: vec![],
        params: entry.params.iter().map(|id| s::Location::Register(id.as_u32())).collect(),
        results: function.signature.results.iter().map(|&id| translate_type(module, id)).collect::<Result<_, _>>()?,
        blocks: vec![],
        proofs: s::FunctionProofs::default(),
        code: vec![],
    };
    for &block_id in &order {
        let block = body.blocks.get(block_id)?;
        result.blocks.push(s::Block {
            pc: pcs[&block_id],
            params: block.params.iter().map(|id| s::Location::Register(id.as_u32())).collect(),
        });
        for &inst_id in &block.insts {
            let inst = body.insts.get(inst_id)?;
            if let o::Operation::Core(o::CoreOp::RangeLen { value }) = inst.op {
                if inst.results.len() != 1 {
                    return Err(GenerateError("range.len requires one result".into()));
                }
                let start = fresh(&mut result, s::Type::Int)?;
                let end = fresh(&mut result, s::Type::Int)?;
                result.code.extend([
                    data(s::DataOp::RangeStart, vec![value.as_u32()], start, s::Immediate::None),
                    data(s::DataOp::RangeEnd, vec![value.as_u32()], end, s::Immediate::None),
                    data(s::DataOp::IntSub, vec![end, start], inst.results[0].as_u32(), s::Immediate::None),
                ]);
            } else {
                result.code.push(translate_instruction(module, body, inst, objects)?);
            }
        }
        let term = match &block.term.kind {
            o::TermKind::Br(target) => s::Instruction::Br(edge(target)?),
            o::TermKind::Cond { condition, yes, no } => s::Instruction::Cond {
                condition: condition.as_u32(), yes: edge(yes)?, no: edge(no)?,
            },
            o::TermKind::Switch { value, cases, default } => {
                let ty = translate_type(module, body.values.get(*value)?.ty)?;
                let comparison = match ty {
                    s::Type::Int => s::DataOp::IntEq,
                    s::Type::Bool => s::DataOp::BoolEq,
                    s::Type::Str => s::DataOp::StrEq,
                    _ => return Err(unsupported("switch type")),
                };
                let mut seen = BTreeSet::new();
                for case in cases {
                    let constant = translate_constant(&case.value)?;
                    if !seen.insert(format!("{constant:?}")) {
                        return Err(GenerateError("duplicate switch case".into()));
                    }
                    let literal = fresh(&mut result, ty)?;
                    let condition = fresh(&mut result, s::Type::Bool)?;
                    result.code.push(data(s::DataOp::Const, vec![], literal, s::Immediate::Constant(constant)));
                    result.code.push(data(comparison, vec![value.as_u32(), literal], condition, s::Immediate::None));
                    let next_pc = u32::try_from(result.code.len() + 1)
                        .map_err(|_| GenerateError("too much code".into()))?;
                    result.code.push(s::Instruction::Cond {
                        condition,
                        yes: edge(&case.target)?,
                        no: s::Edge { target_pc: next_pc, assignments: vec![] },
                    });
                    result.blocks.push(s::Block { pc: next_pc, params: vec![] });
                }
                s::Instruction::Br(edge(default)?)
            }
            o::TermKind::Ret { values } => s::Instruction::Ret(values.iter().map(|id| id.as_u32()).collect()),
            o::TermKind::Unreachable => s::Instruction::Unreachable,
            _ => return Err(unsupported("ownership/failure/defer terminator")),
        };
        result.code.push(term);
    }
    Ok(result)
}

fn check_definition(body: &o::Body, value: o::ValueId, expected: o::Definition, seen: &mut BTreeSet<o::ValueId>)
    -> Result<(), GenerateError>
{
    let data = body.values.get(value)?;
    if data.def != expected || !seen.insert(value) {
        return Err(GenerateError("inconsistent or duplicate SSA definition".into()));
    }
    if !matches!(data.facts, o::ValueFacts::Unelaborated | o::ValueFacts::Plain { .. }) {
        return Err(unsupported("managed/service value facts"));
    }
    Ok(())
}

/// The trailing `$pending` operand of an incomplete construction (§11.4): High leaves it unbound, and SPB 17
/// stores the pending state there, so that a guard on a receiver whose construction is not complete sees it.
fn pending_placeholder(module: &o::Module, body: &o::Body, value: o::ValueId) -> bool {
    let pending = |declaration: o::DeclId| match module.declarations.get(declaration).map(|declared| &declared.kind) {
        Ok(o::DeclarationKind::Object { fields, .. }) => fields.last().is_some_and(|field| {
            field.name == "$pending" && module.types.get(field.ty) == Ok(&o::Type::Bool)
        }),
        _ => false,
    };
    let incomplete = |op: &o::Operation| matches!(op, o::Operation::Core(o::CoreOp::ObjNew {
        declaration, fields, construction: o::ConstructionStatus::Incomplete, ..
    }) if fields.last() == Some(&value) && pending(*declaration));
    body.insts.iter().any(|(_, inst)| incomplete(&inst.op))
        || body.blocks.iter().any(|(_, block)| {
            matches!(&block.term.kind, o::TermKind::InvokeOwn { op, .. } if incomplete(op))
        })
}

fn translate_instruction(module: &o::Module, body: &o::Body, inst: &o::Instruction, objects: &[s::Object])
    -> Result<s::Instruction, GenerateError>
{
    use o::CoreOp as C;
    use s::DataOp as D;
    let reg = |value: o::ValueId| value.as_u32();
    let field = |value: o::ValueId, field: o::FieldRef| -> Result<u32, GenerateError> {
        let ty = translate_type(module, body.values.get(value)?.ty)?;
        let decl = field.declaration.as_u32();
        if !matches!(ty, s::Type::Object(id) | s::Type::Successor(id) if id == decl) {
            return Err(GenerateError("field declaration does not match receiver".into()));
        }
        objects.get(decl as usize).and_then(|object| object.fields.get(field.index as usize)).map(|field| field.storage)
            .ok_or_else(|| GenerateError("invalid source field index".into()))
    };
    let mut immediate = s::Immediate::None;
    let family = match &inst.op {
        o::Operation::Core(core) => family_operation(core),
        _ => None,
    };
    let (op, inputs): (D, Vec<u32>) = match &inst.op {
        _ if family.is_some() => {
            let (op, inputs) = family.expect("scalar family operation");
            (op, inputs.into_iter().map(reg).collect())
        }
        o::Operation::Const(value) => {
            immediate = s::Immediate::Constant(translate_constant(value)?);
            (D::Const, vec![])
        }
        o::Operation::Collection(collection) => {
            let result = inst.results.first().map(|&value| body.values.get(value).map(|value| value.ty))
                .transpose()?;
            let (op, values) = collection_data(module, body, collection, result)?
                .ok_or_else(|| unsupported(inst.op.opcode()))?;
            (op, values.into_iter().map(reg).collect())
        }
        o::Operation::Core(core) => match core {
            C::Global { declaration, ty } => {
                let global = global_index(module, *declaration)?;
                if module.declarations.get(*declaration)?.kind != (o::DeclarationKind::Global {
                    ty: *ty, init: global_init(module, *declaration)?,
                }) {
                    return Err(GenerateError("global read type differs from its declaration".into()));
                }
                immediate = s::Immediate::Global(global);
                (D::GlobalGet, vec![])
            }
            C::EnumCase { declaration, case } => {
                immediate = s::Immediate::Updates(vec![declaration.as_u32(), *case]);
                (D::EnumCase, vec![])
            }
            C::FailSecondary { primary, cleanup } => (D::FailSecondary, vec![reg(*primary), reg(*cleanup)]),
            C::FailCaused { new, handled } => (D::FailCaused, vec![reg(*new), reg(*handled)]),
            C::IntAdd { left, right } => (D::IntAdd, vec![reg(*left), reg(*right)]),
            C::IntSub { left, right } => (D::IntSub, vec![reg(*left), reg(*right)]),
            C::IntMul { left, right } => (D::IntMul, vec![reg(*left), reg(*right)]),
            C::IntMod { left, right } => (D::IntMod, vec![reg(*left), reg(*right)]),
            C::IntEq { left, right } => (D::IntEq, vec![reg(*left), reg(*right)]),
            C::IntNe { left, right } => (D::IntNe, vec![reg(*left), reg(*right)]),
            C::IntLt { left, right } => (D::IntLt, vec![reg(*left), reg(*right)]),
            C::IntLe { left, right } => (D::IntLe, vec![reg(*left), reg(*right)]),
            C::IntGt { left, right } => (D::IntGt, vec![reg(*left), reg(*right)]),
            C::IntGe { left, right } => (D::IntGe, vec![reg(*left), reg(*right)]),
            C::BoolEq { left, right } => (D::BoolEq, vec![reg(*left), reg(*right)]),
            C::BoolNe { left, right } => (D::BoolNe, vec![reg(*left), reg(*right)]),
            C::StrConcat { left, right } => (D::StrConcat, vec![reg(*left), reg(*right)]),
            C::StrEq { left, right } => (D::StrEq, vec![reg(*left), reg(*right)]),
            C::StrNe { left, right } => (D::StrNe, vec![reg(*left), reg(*right)]),
            C::ByteEq { left, right } => (D::ByteEq, vec![reg(*left), reg(*right)]),
            C::ByteNe { left, right } => (D::ByteNe, vec![reg(*left), reg(*right)]),
            C::ByteLt { left, right } => (D::ByteLt, vec![reg(*left), reg(*right)]),
            C::ByteLe { left, right } => (D::ByteLe, vec![reg(*left), reg(*right)]),
            C::ByteGt { left, right } => (D::ByteGt, vec![reg(*left), reg(*right)]),
            C::ByteGe { left, right } => (D::ByteGe, vec![reg(*left), reg(*right)]),
            C::CharEq { left, right } => (D::CharEq, vec![reg(*left), reg(*right)]),
            C::CharNe { left, right } => (D::CharNe, vec![reg(*left), reg(*right)]),
            C::CharLt { left, right } => (D::CharLt, vec![reg(*left), reg(*right)]),
            C::CharLe { left, right } => (D::CharLe, vec![reg(*left), reg(*right)]),
            C::CharGt { left, right } => (D::CharGt, vec![reg(*left), reg(*right)]),
            C::CharGe { left, right } => (D::CharGe, vec![reg(*left), reg(*right)]),
            C::FloatEq { left, right } => (D::FloatEq, vec![reg(*left), reg(*right)]),
            C::FloatNe { left, right } => (D::FloatNe, vec![reg(*left), reg(*right)]),
            C::FloatLt { left, right } => (D::FloatLt, vec![reg(*left), reg(*right)]),
            C::FloatLe { left, right } => (D::FloatLe, vec![reg(*left), reg(*right)]),
            C::FloatGt { left, right } => (D::FloatGt, vec![reg(*left), reg(*right)]),
            C::FloatGe { left, right } => (D::FloatGe, vec![reg(*left), reg(*right)]),
            C::RatEq { left, right } => (D::RatEq, vec![reg(*left), reg(*right)]),
            C::RatNe { left, right } => (D::RatNe, vec![reg(*left), reg(*right)]),
            C::RatLt { left, right } => (D::RatLt, vec![reg(*left), reg(*right)]),
            C::RatLe { left, right } => (D::RatLe, vec![reg(*left), reg(*right)]),
            C::RatGt { left, right } => (D::RatGt, vec![reg(*left), reg(*right)]),
            C::RatGe { left, right } => (D::RatGe, vec![reg(*left), reg(*right)]),
            C::EnumEq { left, right } => (D::EnumEq, vec![reg(*left), reg(*right)]),
            C::EnumNe { left, right } => (D::EnumNe, vec![reg(*left), reg(*right)]),
            C::IntDiv { left, right } => (D::RatNew, vec![reg(*left), reg(*right)]),
            C::RatNew { numerator, denominator } => (D::RatNew, vec![reg(*numerator), reg(*denominator)]),
            // A one-case Error variant is its case object (`translate_type`): its active case is always 0.
            C::VariantTag { value } if single_error_case(module, body.values.get(*value)?.ty)?.is_some() => {
                immediate = s::Immediate::Constant(s::Constant::Int("0".into()));
                (D::Const, vec![])
            }
            C::VariantTag { value } => (D::VariantTag, vec![reg(*value)]),
            C::VariantOf { value, ty } if matches!(translate_type(module, *ty)?, s::Type::Variant(_)) => {
                (D::Move, vec![reg(*value)])
            }
            C::VariantGet { value, case, field } => {
                let storage = variant_storage(module, objects, *field)?;
                match translate_type(module, body.values.get(*value)?.ty)? {
                    // The single case of an Error variant is the value's own object: read the field.
                    s::Type::Object(id) if single_error_case(module, body.values.get(*value)?.ty)?.is_some() => {
                        if *case != 0 || field.declaration.as_u32() != id {
                            return Err(GenerateError("variant.get outside the only Error case".into()));
                        }
                        immediate = s::Immediate::Field(storage);
                        (D::ObjGet, vec![reg(*value)])
                    }
                    _ => {
                        immediate = s::Immediate::Updates(vec![*case, storage]);
                        (D::VariantGet, vec![reg(*value)])
                    }
                }
            }
            C::ValEq { left, right } => (D::ValEq, vec![reg(*left), reg(*right)]),
            C::ValNe { left, right } => (D::ValNe, vec![reg(*left), reg(*right)]),
            C::StrLt { left, right } => (D::StrLt, vec![reg(*left), reg(*right)]),
            C::StrLe { left, right } => (D::StrLe, vec![reg(*left), reg(*right)]),
            C::StrGt { left, right } => (D::StrGt, vec![reg(*left), reg(*right)]),
            C::StrGe { left, right } => (D::StrGe, vec![reg(*left), reg(*right)]),
            C::IntNeg { value } => (D::IntNeg, vec![reg(*value)]),
            C::BoolNot { value } => (D::BoolNot, vec![reg(*value)]),
            C::StrLen { value } => (D::StrLen, vec![reg(*value)]),
            C::RangeStart { value } => (D::RangeStart, vec![reg(*value)]),
            C::RangeEnd { value } => (D::RangeEnd, vec![reg(*value)]),
            C::Convert { value, from, to } => {
                let (Some(source), Some(target)) = (numeric_type(*from), numeric_type(*to)) else {
                    return Err(unsupported("conversion type"));
                };
                if s::conversion(source, target).is_none() {
                    return Err(unsupported("conversion pair"));
                }
                let result = inst.results.first().ok_or_else(|| GenerateError("missing conversion result".into()))?;
                if translate_type(module, body.values.get(*value)?.ty)? != source
                    || translate_type(module, body.values.get(*result)?.ty)? != target
                {
                    return Err(GenerateError("conversion annotation disagrees with SSA types".into()));
                }
                (D::Convert, vec![reg(*value)])
            }
            C::RangeNew { start, end, inclusive } => {
                immediate = s::Immediate::Inclusive(*inclusive);
                (D::RangeNew, vec![reg(*start), reg(*end)])
            }
            C::ObjNew { declaration, type_args, fields, packet, construction: _ } => {
                let object = objects.get(declaration.as_u32() as usize)
                    .ok_or_else(|| GenerateError("unknown object declaration".into()))?;
                if packet.is_some() || !type_args.is_empty() || object.layout.has_destroy
                    || object.layout.may_reach_resource
                {
                    return Err(unsupported("resource construction"));
                }
                immediate = s::Immediate::Object(declaration.as_u32());
                (D::ObjNew, fields.iter().map(|id| reg(*id)).collect())
            }
            C::ObjGet { object, field: reference } | C::ObjPending { object, field: reference } => {
                immediate = s::Immediate::Field(field(*object, *reference)?);
                if matches!(core, C::ObjPending { .. }) {
                    let declaration = module.declarations.get(reference.declaration)?;
                    let o::DeclarationKind::Object { fields, .. } = &declaration.kind else {
                        return Err(GenerateError("invalid pending field declaration".into()));
                    };
                    let source = fields.get(reference.index as usize)
                        .ok_or_else(|| GenerateError("invalid pending field index".into()))?;
                    if source.name != "$pending" || module.types.get(source.ty)? != &o::Type::Bool {
                        return Err(GenerateError("obj.pending requires registered $pending bool field".into()));
                    }
                }
                (D::ObjGet, vec![reg(*object)])
            }
            C::OnceLoad { object, field: reference } => {
                immediate = s::Immediate::Field(once_field(module, objects, *reference)?);
                (D::ObjGet, vec![reg(*object)])
            }
            C::OnceStore { object, field: reference, value } => {
                immediate = s::Immediate::Field(once_field(module, objects, *reference)?);
                field(*object, *reference)?;
                (D::ObjInit, vec![reg(*object), reg(*value)])
            }
            C::ObjInit { object, field: reference, value, completion } => {
                if completion.is_some() {
                    return Err(unsupported("construction completion state"));
                }
                immediate = s::Immediate::Field(field(*object, *reference)?);
                (D::ObjInit, vec![reg(*object), reg(*value)])
            }
            C::ObjNext { object, updates, packet } => {
                if packet.is_some() {
                    return Err(unsupported("successor packet"));
                }
                let ty = translate_type(module, body.values.get(*object)?.ty)?;
                let s::Type::Object(decl) = ty else { return Err(GenerateError("invalid successor base".into())); };
                if objects[decl as usize].layout.may_reach_resource {
                    return Err(unsupported("resource successor completion"));
                }
                immediate = s::Immediate::Updates(updates.iter().map(|update| field(*object, update.field))
                    .collect::<Result<_, _>>()?);
                let inputs = std::iter::once(reg(*object))
                    .chain(updates.iter().map(|update| reg(update.value))).collect();
                (D::ObjNext, inputs)
            }
            C::SuccTake { value, ty } => {
                let Some(&result) = inst.results.first() else {
                    return Err(GenerateError("missing succ.take result".into()));
                };
                if body.values.get(result)?.ty != *ty {
                    return Err(GenerateError("succ.take result annotation mismatch".into()));
                }
                let source = translate_type(module, body.values.get(*value)?.ty)?;
                let target = translate_type(module, *ty)?;
                if !matches!((source, target), (s::Type::Successor(a), s::Type::Object(b))
                    | (s::Type::RoleSuccessor(a), s::Type::Role(b)) if a == b)
                {
                    return Err(GenerateError("succ.take requires matching successor/object types".into()));
                }
                (D::Move, vec![reg(*value)])
            }
            // role.wrap is a move; the loader admits it only from an implementing object (ROLE_WRAP).
            C::RoleWrap { value, .. } => (D::Move, vec![reg(*value)]),
            C::CapMake { owner, packet: None, .. } => (D::Move, vec![reg(*owner)]),
            C::RoleNarrow { value, ty } => {
                let source = translate_type(module, body.values.get(*value)?.ty)?;
                if !matches!((source, translate_type(module, *ty)?), (s::Type::Role(_), s::Type::Role(_))) {
                    return Err(unsupported("role narrowing outside role values"));
                }
                (D::RoleNarrow, vec![reg(*value)])
            }
            C::Undef { ty } => {
                let Some(&result) = inst.results.first() else {
                    return Err(GenerateError("missing undef result".into()));
                };
                if body.values.get(result)?.ty != *ty {
                    return Err(GenerateError("undef result annotation mismatch".into()));
                }
                if pending_placeholder(module, body, result) {
                    // `obj.pending` reads `$pending`: it holds true until the completing write stores false.
                    immediate = s::Immediate::Constant(s::Constant::Bool(true));
                    (D::Const, vec![])
                } else {
                    (D::Undef, vec![])
                }
            }
            _ => return Err(unsupported(inst.op.opcode())),
        },
        _ => return Err(unsupported(inst.op.opcode())),
    };
    let arity = usize::from(op != D::ObjInit);
    if inst.results.len() != arity {
        return Err(GenerateError(format!("{}: expected {arity} S1 results", inst.op.opcode())));
    }
    Ok(s::Instruction::Data {
        op,
        inputs,
        output: inst.results.first().map(|id| reg(*id)),
        immediate,
    })
}

/// The data form of a plain collection operation: its operation and OIR input values. `result` is the
/// OIR type of its result, for retags. `None` for builder transitions and operations with packets.
pub(crate) fn collection_data(module: &o::Module, body: &o::Body, op: &o::CollectionOp, result: Option<o::TypeId>)
    -> Result<Option<(s::DataOp, Vec<o::ValueId>)>, GenerateError>
{
    use o::CollectionOp as C;
    use s::DataOp as D;
    Ok(Some(match op {
        C::ArrayNew { values, packet: None, .. } => (D::ArrayNew, values.clone()),
        C::ArrayLen { array } => (D::ArrayLen, vec![*array]),
        C::ArrayConcat { arrays, packet: None, .. } => (D::ArrayConcat, arrays.clone()),
        C::ArrayJoin { array, separator } => (D::ArrayJoin, vec![*array, *separator]),
        C::ArraySlice { array, start, end, packet: None } => (D::ArraySlice, vec![*array, *start, *end]),
        C::ArrayRemove { array, index, packet: None } => (D::ArrayRemove, vec![*array, *index]),
        C::ArrayReplace { array, index, value, packet: None } => (D::ArrayReplace, vec![*array, *index, *value]),
        C::ArrayDifference { array, removed, packet: None } => {
            (D::ArrayDifference, std::iter::once(*array).chain(removed.iter().copied()).collect())
        }
        C::MapNew { entries, packet: None, .. } => {
            (D::MapNew, entries.iter().flat_map(|&(key, value)| [key, value]).collect())
        }
        C::MapMerge { maps, packet: None, .. } => (D::MapMerge, maps.clone()),
        C::MapDifference { map, removed, packet: None } => {
            (D::MapDifference, std::iter::once(*map).chain(removed.iter().copied()).collect())
        }
        C::MapLen { map } => (D::MapLen, vec![*map]),
        C::MapHas { map, key } => (D::MapHas, vec![*map, *key]),
        C::MapGet { map, key } => (D::MapGet, vec![*map, *key]),
        C::MapKeys { map } => (D::MapKeys, vec![*map]),
        C::MapValues { map } => (D::MapValues, vec![*map]),
        C::MapKeyAt { map, index } => (D::MapKeyAt, vec![*map, *index]),
        C::MapValueAt { map, index } => (D::MapValueAt, vec![*map, *index]),
        C::ArrayRetag { array: value, ty } | C::MapRetag { map: value, ty } => {
            let source = translate_type(module, body.values.get(*value)?.ty)?;
            let target = translate_type(module, *ty)?;
            let empty = s::Type::Array(s::ArrayType { leaf: s::ArrayLeaf::Never, depth: 1 });
            if result.is_some_and(|result| result != *ty) {
                return Err(GenerateError("collection retag result annotation mismatch".into()));
            }
            if (source, target) == (empty, s::Type::Bytes) {
                // An uninferred `[]` becomes an empty byte array, which has its own representation.
                (D::ArrayNew, vec![])
            } else if target == source || s::never_retag(composites(module).entries(), source, target) {
                (D::Move, vec![*value])
            } else {
                return Err(unsupported("collection retag that changes the SPB 17 type"));
            }
        }
        C::BuilderNew { .. } | C::MapBuilderNew { .. } => (D::BuilderNew, vec![]),
        _ => return Ok(None),
    }))
}

fn fresh(function: &mut s::Function, ty: s::Type) -> Result<u32, GenerateError> {
    let id = u32::try_from(function.registers.len()).map_err(|_| GenerateError("too many registers".into()))?;
    function.registers.push(ty);
    Ok(id)
}

fn data(op: s::DataOp, inputs: Vec<u32>, output: u32, immediate: s::Immediate) -> s::Instruction {
    s::Instruction::Data { op, inputs, output: Some(output), immediate }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod cleanup_tests;
#[cfg(test)]
mod futures_tests;
#[cfg(test)]
mod roles_tests;
#[cfg(test)]
mod dead_tests;
#[cfg(test)]
mod variant_tests;

/// Mandatory semantic gate for executable Low. External contracts come from the caller's backend catalog.
/// This path cannot be selected by a file flag; external artifacts always pass verify_load independently.
pub fn compile_low(module: &o::Module, layouts: &dyn LayoutRegistry,
    context: o::VerifyContext<'_>, contracts: &dyn s::ExternalContracts)
    -> Result<AllocatedProgram, GenerateError>
{
    compile_low_with(module, layouts, context, contracts, &o::VerifyOptions::default())
}

/// Build policy may select structural verification; loader checks remain mandatory in either mode.
pub fn compile_low_with(
    module: &o::Module,
    layouts: &dyn LayoutRegistry,
    context: o::VerifyContext<'_>,
    contracts: &dyn s::ExternalContracts,
    options: &o::VerifyOptions,
) -> Result<AllocatedProgram, GenerateError> {
    if module.profile != o::Profile::Low {
        return Err(GenerateError("expected Low profile".into()));
    }
    o::verify_low_with(module, options, context)
        .map_err(|report| GenerateError(format!("verify_low: {report:?}")))?;
    let output = generate_with_allocation(module, layouts)?;
    s::verify_load(&output.program, contracts)?;
    Ok(output)
}

/// Low traces values and inventories; a Future's duty tokens have no Low trace. In SPB 17 each of them is a word
/// that refers to the Future frame (cache-state, task-state; Champ draft 3 §14.2), so it is rooted wherever the
/// token is active. The state records list the active tokens: like the maps before each pc, which take the
/// state records whole, the map after an operation takes the duty tokens of the next pc's state.
fn root_duty_words(function: &mut s::Function) {
    let duty = |location: &s::Location| matches!(location, s::Location::Service(id)
        if matches!(function.services[*id as usize], s::ServiceKind::CacheState | s::ServiceKind::TaskState));
    let mut missing = Vec::new();
    for state in &function.proofs.states {
        let Some(pc) = state.pc.checked_sub(1) else { continue };
        let next = matches!(function.code.get(pc as usize),
            Some(s::Instruction::Champ { op, flow: s::Flow::Next(_), .. })
                if !matches!(op, s::Opcode::RetOwn | s::Opcode::RetDefer));
        let duties = state.locations.iter().filter(|&location| duty(location)).copied().collect::<Vec<_>>();
        if next && !duties.is_empty() {
            missing.push((pc, duties));
        }
    }
    for (pc, duties) in missing {
        let post = &mut function.proofs.post_roots;
        let index = match post.iter().position(|root| root.pc == pc) {
            Some(index) => index,
            None => {
                post.push(s::RootRequirement { pc, locations: vec![] });
                post.len() - 1
            }
        };
        let locations = &mut post[index].locations;
        for location in duties {
            if !locations.contains(&location) {
                locations.push(location);
            }
        }
    }
}

/// Low roots a value with its full dependencies (`RootTrace::FullDependencies`, managed facts). In SPB 17 a location
/// keeps the anchors its dependencies name alive (`proofs.dependencies`), so every traced anchor joins each map that
/// holds the location: for instance a fixed root's deps of its result, taken before its scope closes, keep the
/// result's register alive up to fut.publish.
fn root_dependency_anchors(function: &s::Function, locations: &mut BTreeSet<s::Location>) {
    let traced = |location: &s::Location| match *location {
        s::Location::Register(id) => !matches!(function.registers[id as usize], s::Type::Bool | s::Type::Never
            | s::Type::Descriptor | s::Type::Byte | s::Type::Float | s::Type::Char),
        s::Location::Service(id) => !s::layout::service_layout(function.services[id as usize]).references.is_empty(),
    };
    let mut reached = locations.clone();
    loop {
        let before = reached.len();
        for dependency in &function.proofs.dependencies {
            if reached.contains(&dependency.value) {
                reached.extend(&dependency.obligations);
            }
        }
        if reached.len() == before {
            break;
        }
    }
    locations.extend(reached.into_iter().filter(traced));
}

/// Relocated Low roots remain authoritative for services. Scalar register liveness fills the gaps
/// introduced by machine expansion (range.len, switch, construction); it never synthesizes Low proofs.
fn complete_codegen_roots(function: &mut s::Function) -> Result<(), GenerateError> {
    // Undef placeholders contain a safe zero, never a live reference. Preserve their restriction to
    // construction operands; do not turn them into ordinary reads through relocated root metadata.
    let unbound = function.code.iter().filter_map(|instruction| match instruction {
        s::Instruction::Data { op: s::DataOp::Undef, output: Some(id), .. } => Some(s::Location::Register(*id)),
        _ => None,
    }).collect::<BTreeSet<_>>();
    for root in function.proofs.roots.iter_mut().chain(&mut function.proofs.post_roots) {
        root.locations.retain(|location| !unbound.contains(location));
    }
    for dependency in &mut function.proofs.dependencies {
        dependency.obligations.retain(|location| !unbound.contains(location));
    }
    let mut roots = BTreeMap::<u32, BTreeSet<s::Location>>::new();
    for root in &function.proofs.roots {
        roots.entry(root.pc).or_default().extend(&root.locations);
    }
    for state in &function.proofs.states {
        roots.entry(state.pc).or_default().extend(&state.locations);
    }
    let low_roots = !function.proofs.closes.is_empty() || !function.proofs.post_roots.is_empty();
    root_duty_words(function);
    for locations in roots.values_mut() {
        root_dependency_anchors(function, locations);
    }
    let mut post = std::mem::take(&mut function.proofs.post_roots);
    for root in &mut post {
        let mut locations = root.locations.iter().copied().collect::<BTreeSet<_>>();
        root_dependency_anchors(function, &mut locations);
        for location in locations {
            if !root.locations.contains(&location) {
                root.locations.push(location);
            }
        }
    }
    function.proofs.post_roots = post;
    if low_roots {
        // Cleanup-bearing Low has complete #591 roots. Never repair a missing Low root here.
        // Only the explicitly recorded machine-expansion boundaries were added by the emitter.
        function.proofs.roots = roots.into_iter().map(|(pc, locations)| s::RootRequirement {
            pc, locations: locations.into_iter().collect(),
        }).collect();
        return Ok(());
    }
    let mut live = vec![BTreeSet::<u32>::new(); function.code.len()];
    loop {
        let mut changed = false;
        for (pc, instruction) in function.code.iter().enumerate().rev() {
            let mut next = BTreeSet::new();
            let edges = match instruction {
                s::Instruction::Br(edge) => vec![edge],
                s::Instruction::Cond { yes, no, .. } => vec![yes, no],
                s::Instruction::Champ { flow: s::Flow::Outcomes(outcomes), .. } => {
                    outcomes.iter().map(|outcome| &outcome.edge).collect()
                }
                _ => vec![],
            };
            for edge in edges {
                let mut branch = live[edge.target_pc as usize].clone();
                for assignment in &edge.assignments {
                    if let s::Location::Register(id) = assignment.destination { branch.remove(&id); }
                }
                for assignment in &edge.assignments {
                    if let s::Source::Location(s::Location::Register(id)) = assignment.source { branch.insert(id); }
                }
                next.extend(branch);
            }
            if matches!(instruction, s::Instruction::Data { .. }
                | s::Instruction::Champ { flow: s::Flow::Next(_), .. })
                && !matches!(instruction, s::Instruction::Champ { op: s::Opcode::RetOwn | s::Opcode::RetDefer, .. })
            {
                next.extend(live.get(pc + 1).into_iter().flatten());
            }
            match instruction {
                s::Instruction::Data { inputs, output, .. } => {
                    if let Some(output) = output { next.remove(output); }
                    next.extend(inputs);
                }
                s::Instruction::Ret(inputs) => next.extend(inputs),
                s::Instruction::Raise(id) | s::Instruction::Cond { condition: id, .. } => { next.insert(*id); }
                s::Instruction::Champ { arguments, flow, .. } => {
                    if let s::Flow::Next(outputs) = flow {
                        for output in outputs {
                            if let s::Location::Register(id) = output { next.remove(id); }
                        }
                    }
                    for argument in arguments {
                        match argument {
                            s::Argument::Location(s::Location::Register(id)) => { next.insert(*id); }
                            s::Argument::Selection(id) => {
                                next.extend(function.proofs.selections[*id as usize].locations.iter()
                                    .filter_map(|loc| match loc { s::Location::Register(id) => Some(*id), _ => None }));
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            for location in function.proofs.post_roots.iter().filter(|root| root.pc as usize == pc)
                .flat_map(|root| &root.locations)
            {
                if let s::Location::Register(id) = location { next.insert(*id); }
            }
            if let s::Instruction::Data { output: Some(id), .. } = instruction { next.remove(id); }
            if let s::Instruction::Champ { flow: s::Flow::Next(outputs), .. } = instruction {
                for output in outputs {
                    if let s::Location::Register(id) = output { next.remove(id); }
                }
            }
            for location in roots.get(&(pc as u32)).into_iter().flatten() {
                if let s::Location::Register(id) = location { next.insert(*id); }
            }
            if next != live[pc] {
                live[pc] = next;
                changed = true;
            }
        }
        if !changed { break; }
    }
    for (pc, live) in live.into_iter().enumerate() {
        roots.entry(pc as u32).or_default().extend(live.into_iter().map(s::Location::Register));
    }
    function.proofs.roots = roots.into_iter().map(|(pc, locations)| s::RootRequirement {
        pc, locations: locations.into_iter().collect(),
    }).collect();
    Ok(())
}
