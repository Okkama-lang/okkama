//! Low OIR-2 → SPB 17: scalar/object/control, owning calls and the cleanup automaton.
//! Always compiled, independent of runtime features. compile_low is the executable semantic driver.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use okkama_oir as o;
use okkama_sparrow::spb17 as s;

mod calls;
mod roles;
mod successors;

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
    if module.profile != o::Profile::Low {
        return Err(GenerateError("expected verified low profile".into()));
    }
    if !module.imports.is_empty() || !module.protocols.is_empty()
        || module.tests.is_some() || module.requires.iter().any(|feature| *feature != o::Feature::Core)
    {
        return Err(unsupported("unresolved declarations, protocols or service metadata"));
    }
    let mut p = s::Program {
        module: module.name.clone(),
        version: module.version.clone().unwrap_or_default(),
        entry: module.entry.map(|id| id.as_u32()),
        ..s::Program::default()
    };
    p.objects = objects(module, layouts)?;
    let synthetic = roles::Synthetic::of(module)?;
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
                let translated = translate_type(module, value.ty);
                // Own tracks the deps of role, cap and action-value elements; the loader admits such an array
                // only when every object it can reach is closed plain data (ARRAY_PLAIN).
                let role_array = roles::view_array(module, value.ty)?
                    && matches!(value.facts, o::ValueFacts::Managed { .. });
                if matches!(translated, Ok(s::Type::Array(_))) && !role_array
                    && value.facts != (o::ValueFacts::Plain {
                        certificate: o::PlainCertificate::ClosedGraph { ty: value.ty },
                    })
                {
                    return Err(GenerateError("plain array requires own ClosedGraph certificate".into()));
                }
            }
        }
        let translated = if calls::needs_s2(module, function)? {
            calls::translate(module, id, function, &p.objects, &mut p.tables.summaries, &synthetic)?
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
        successors::forward_taken(function)?;
        successors::prune_dead_aliases(function);
        successors::close_unwind_permissions(function)?;
        complete_codegen_roots(function)?;
    }
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
            _ => return Err(unsupported("non-object declaration")),
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
                if case.runtime.has_destroy || case.runtime.may_reach_resource {
                    return Err(unsupported("variant case that reaches resources"));
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
                o::DeclarationKind::Variant { cases, is_error: false } => {
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
            name: declaration.name.clone(),
            fields: fields.iter().zip(&layout.source_fields).map(|(field, &storage)| s::Field {
                name: field.name.clone(),
                storage,
                once: field.once,
            }).collect(),
            layout: layout.runtime,
            role,
            implements,
        });
    }
    roles::synthetic_objects(module, &mut objects)?;
    Ok(objects)
}

pub(crate) fn translate_type(module: &o::Module, id: o::TypeId) -> Result<s::Type, GenerateError> {
    Ok(match module.types.get(id)? {
        o::Type::Int => s::Type::Int,
        o::Type::Array(element) if module.types.get(*element)? == &o::Type::Byte => s::Type::Bytes,
        o::Type::Array(element) => {
            let leaf = match translate_type(module, *element)? {
                s::Type::Int => s::ArrayLeaf::Int,
                s::Type::Bool => s::ArrayLeaf::Bool,
                s::Type::Str => s::ArrayLeaf::Str,
                s::Type::Range => s::ArrayLeaf::Range,
                s::Type::Bytes => s::ArrayLeaf::Bytes,
                s::Type::Object(id) => s::ArrayLeaf::Object(id),
                s::Type::Role(id) => s::ArrayLeaf::Role(id),
                s::Type::Array(shape) => {
                    let depth = shape.depth.checked_add(1)
                        .ok_or_else(|| GenerateError("array nesting overflow".into()))?;
                    return Ok(s::Type::Array(s::ArrayType { depth, ..shape }));
                }
                _ => return Err(unsupported("array element type")),
            };
            s::Type::Array(s::ArrayType { leaf, depth: 1 })
        }
        o::Type::Role { decl, args } if args.is_empty() && module.declarations.get(*decl)?.name == "Error" => {
            s::Type::Error
        }
        o::Type::Role { decl, args } if args.is_empty() => s::Type::Role(decl.as_u32()),
        o::Type::Fn(_) => s::Type::Role(roles::callable_role(module, id)?),
        // A cap holds its owner's complete graph and reads it on every call: it is the owner's reference.
        o::Type::Cap { decl, args } if args.is_empty() => {
            let o::DeclarationKind::Cap { owner, .. } = &module.declarations.get(*decl)?.kind else {
                return Err(GenerateError("cap type references a non-cap declaration".into()));
            };
            match translate_type(module, *owner)? {
                owner @ s::Type::Object(_) => owner,
                _ => return Err(unsupported("cap of a non-object owner")),
            }
        }
        o::Type::Enum(decl) => s::Type::Object(decl.as_u32()),
        o::Type::Variant(decl) => {
            let o::DeclarationKind::Variant { cases, is_error } = &module.declarations.get(*decl)?.kind else {
                return Err(GenerateError("variant type references non-variant declaration".into()));
            };
            if !is_error {
                return Ok(s::Type::Variant(decl.as_u32()));
            }
            let [case] = cases.as_slice() else { return Err(unsupported("multi-case Error variant type")); };
            s::Type::Object(case.layout.as_u32())
        }
        o::Type::Descriptor => s::Type::Descriptor,
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
        ty => return Err(unsupported(&format!("type {ty:?}"))),
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

/// The SPB 17 type of an OIR-2 conversion operand.
pub(crate) fn numeric_type(ty: o::NumericType) -> Option<s::Type> {
    match ty {
        o::NumericType::Int => Some(s::Type::Int),
        o::NumericType::Bool => Some(s::Type::Bool),
        o::NumericType::Str => Some(s::Type::Str),
        o::NumericType::Byte => Some(s::Type::Byte),
        o::NumericType::Float => Some(s::Type::Float),
        o::NumericType::Rat => Some(s::Type::Rat),
    }
}

fn translate_constant(literal: &o::Literal) -> Result<s::Constant, GenerateError> {
    Ok(match literal {
        o::Literal::Int(value) => s::Constant::Int(value.clone()),
        o::Literal::Bool(value) => s::Constant::Bool(*value),
        o::Literal::Str(value) => s::Constant::Str(value.clone()),
        o::Literal::Byte(value) => s::Constant::Byte(u32::from(*value)),
        o::Literal::Char(value) => s::Constant::Char(u32::from(*value)),
        o::Literal::Float(bits) => s::Constant::Float(bits.to_string()),
        _ => return Err(unsupported("constant outside int/bool/byte/char/float/string")),
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
    let (op, inputs) = match &inst.op {
        o::Operation::Const(value) => {
            immediate = s::Immediate::Constant(translate_constant(value)?);
            (D::Const, vec![])
        }
        o::Operation::Collection(collection) => match collection {
            o::CollectionOp::ArrayNew { values, packet: None, .. } => {
                (D::ArrayNew, values.iter().map(|value| reg(*value)).collect())
            }
            o::CollectionOp::ArrayLen { array } => (D::ArrayLen, vec![reg(*array)]),
            _ => return Err(unsupported(inst.op.opcode())),
        },
        o::Operation::Core(core) => match core {
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
            C::VariantTag { value } => (D::VariantTag, vec![reg(*value)]),
            C::VariantOf { value, ty } if matches!(translate_type(module, *ty)?, s::Type::Variant(_)) => {
                (D::Move, vec![reg(*value)])
            }
            C::VariantGet { value, case, field } => {
                immediate = s::Immediate::Updates(vec![*case, variant_storage(module, objects, *field)?]);
                (D::VariantGet, vec![reg(*value)])
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
                (D::Undef, vec![])
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
mod roles_tests;

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
    if !function.proofs.closes.is_empty() || !function.proofs.post_roots.is_empty() {
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
