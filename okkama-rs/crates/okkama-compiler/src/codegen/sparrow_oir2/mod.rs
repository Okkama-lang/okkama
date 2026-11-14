//! Verified S1 Low OIR-2 → SPB 17. Always compiled, independent of runtime features.
//! This verifier covers the plain S1 subset; #564's ownership gate is still a
//! prerequisite for S2/S3. Unsupported operations never fall back to OIR-1.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use okkama_oir as o;
use okkama_sparrow::spb17 as s;

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
    GenerateError(format!("{name}: not yet in S1"))
}

/// Final layout includes runtime overrides and hidden physical fields (§2.7).
/// The compiler never derives allocation size from source ObjectDecl fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectLayout {
    pub runtime: s::ObjectRuntimeLayout,
    pub source_fields: Vec<u32>,
}

pub trait LayoutRegistry {
    fn object_layout(&self, declaration: o::DeclId) -> Result<ObjectLayout, GenerateError>;
}

impl LayoutRegistry for BTreeMap<o::DeclId, ObjectLayout> {
    fn object_layout(&self, declaration: o::DeclId) -> Result<ObjectLayout, GenerateError> {
        self.get(&declaration).cloned().ok_or_else(|| GenerateError(format!(
            "missing final runtime layout for object {}", declaration.as_u32())))
    }
}

/// Mandatory input and output checks. Only plain scalar/object/control Low is
/// accepted here; services, ownership facts and failure/defer CFG wait for S2/S3.
pub fn generate(module: &o::Module, layouts: &dyn LayoutRegistry) -> Result<s::Program, GenerateError> {
    if module.profile != o::Profile::Low {
        return Err(GenerateError("expected verified low profile".into()));
    }
    if !module.imports.is_empty() || !module.protocols.is_empty() || !module.summaries.is_empty()
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
    for (id, declaration) in module.declarations.iter() {
        if !declaration.type_params.is_empty() {
            return Err(unsupported("generic object declaration"));
        }
        let o::DeclarationKind::Object { fields, witnesses, destroy, .. } = &declaration.kind else {
            return Err(unsupported("non-object declaration"));
        };
        if !witnesses.is_empty() || destroy.is_some() {
            return Err(unsupported("object witnesses/destroy entries"));
        }
        let layout = layouts.object_layout(id)?;
        if fields.len() != layout.source_fields.len() {
            return Err(GenerateError("layout registry field mapping arity mismatch".into()));
        }
        for (field, &physical) in fields.iter().zip(&layout.source_fields) {
            if layout.runtime.storage_fields.get(physical as usize) != Some(&translate_type(module, field.ty)?) {
                return Err(GenerateError("layout registry field type mismatch".into()));
            }
        }
        p.objects.push(s::Object {
            name: declaration.name.clone(),
            fields: fields.iter().zip(&layout.source_fields).map(|(field, &storage)| s::Field {
                name: field.name.clone(),
                storage,
                once: field.once,
            }).collect(),
            layout: layout.runtime,
        });
    }
    for (id, scope) in module.scopes.iter() {
        p.tables.scopes.push(s::Scope {
            function: scope.function.as_u32(),
            parent: scope.parent.map(|id| id.as_u32()),
            boundary: id.as_u32(),
        });
        p.tables.boundaries.push(s::Boundary { scope: id.as_u32() });
    }
    for (id, function) in module.functions.iter() {
        p.functions.push(translate_function(module, id, function, &p.objects)?);
    }
    s::verify(&p)?;
    Ok(p)
}

fn translate_type(module: &o::Module, id: o::TypeId) -> Result<s::Type, GenerateError> {
    Ok(match module.types.get(id)? {
        o::Type::Int => s::Type::Int,
        o::Type::Bool => s::Type::Bool,
        o::Type::Str => s::Type::Str,
        o::Type::Range => s::Type::Range,
        o::Type::Never => s::Type::Never,
        o::Type::Object { decl, args } if args.is_empty() => {
            if !matches!(module.declarations.get(*decl)?.kind, o::DeclarationKind::Object { .. }) {
                return Err(GenerateError("object type references non-object declaration".into()));
            }
            s::Type::Object(decl.as_u32())
        }
        o::Type::Successor(base) => match module.types.get(*base)? {
            o::Type::Object { decl, args } if args.is_empty() => s::Type::Successor(decl.as_u32()),
            _ => return Err(unsupported("non-object successor type")),
        },
        ty => return Err(unsupported(&format!("type {ty:?}"))),
    })
}

fn translate_constant(literal: &o::Literal) -> Result<s::Constant, GenerateError> {
    Ok(match literal {
        o::Literal::Int(value) => s::Constant::Int(value.clone()),
        o::Literal::Bool(value) => s::Constant::Bool(*value),
        o::Literal::Str(value) => s::Constant::Str(value.clone()),
        _ => return Err(unsupported("constant outside int/bool/string")),
    })
}

fn translate_function(module: &o::Module, id: o::FuncId, function: &o::Function, objects: &[s::Object])
    -> Result<s::Function, GenerateError>
{
    if !function.type_params.is_empty() || function.summary.is_some() || function.async_creation_summary.is_some()
        || function.async_body_summary.is_some() || function.signature.ownership_abi != o::OwnershipAbi::Observation
        || function.attrs.iter().any(|attr| !matches!(attr, o::FunctionAttr::Entry | o::FunctionAttr::Hidden))
    {
        return Err(unsupported("non-observation entry ABI, async/generic/native or ownership entry"));
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
                    s::Type::Int | s::Type::Bool | s::Type::Str | s::Type::Range | s::Type::Never),
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
        o::Operation::Core(core) => match core {
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
                let included = |ty: o::NumericType| matches!(ty, o::NumericType::Int | o::NumericType::Bool |
                    o::NumericType::Str);
                if !included(*from) || !included(*to) {
                    return Err(unsupported("conversion outside int/bool/string"));
                }
                let numeric = |ty: o::NumericType| match ty {
                    o::NumericType::Int => s::Type::Int,
                    o::NumericType::Bool => s::Type::Bool,
                    _ => s::Type::Str,
                };
                let result = inst.results.first().ok_or_else(|| GenerateError("missing conversion result".into()))?;
                if translate_type(module, body.values.get(*value)?.ty)? != numeric(*from)
                    || translate_type(module, body.values.get(*result)?.ty)? != numeric(*to)
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
                if !matches!((source, target), (s::Type::Successor(a), s::Type::Object(b)) if a == b) {
                    return Err(GenerateError("succ.take requires matching successor/object types".into()));
                }
                (D::Move, vec![reg(*value)])
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
