//! A single-case Error variant is its case's object in SPB 17 (#704): its tag is the constant 0 and its
//! payload read is a field read that cannot fail; a real variant keeps variant.tag and variant.get.
use super::*;

const ONE_CASE: &str = "oir 2 high
module one_case
object Short {
    field reason: int
}
object Item {
    field value: int
}
variant Fail is Error = (because: Short)
variant Choice = (one: Item, two: Item)
scope S0 action in @f
func @f(%e: Fail, %c: Choice) -> int {
    ^entry in S0:
    %z: int = const 0
    ret %z
}
";

fn module() -> (o::Module, o::ValueId, o::ValueId) {
    let m = o::text::parse(ONE_CASE).unwrap_or_else(|error| panic!("{error}"));
    let body = m.functions.get(o::FuncId::from_u32(0)).unwrap().body.as_ref().unwrap();
    let params = body.blocks.get(body.entry).unwrap().params.clone();
    (m, params[0], params[1])
}

fn field(m: &o::Module, name: &str) -> o::FieldRef {
    let (declaration, _) = m.declarations.iter().find(|(_, declaration)| declaration.name.ends_with(name)).unwrap();
    o::FieldRef { declaration, index: 0 }
}

#[test]
fn only_an_error_variant_with_one_case_is_its_case_object() {
    let (m, error, choice) = module();
    let body = m.functions.get(o::FuncId::from_u32(0)).unwrap().body.as_ref().unwrap();
    let error_ty = body.values.get(error).unwrap().ty;
    let choice_ty = body.values.get(choice).unwrap().ty;
    assert!(single_error_case(&m, error_ty).unwrap().is_some());
    assert!(single_error_case(&m, choice_ty).unwrap().is_none());
    assert!(single_error_case(&m, m.types.iter().find(|(_, ty)| **ty == o::Type::Int).unwrap().0).unwrap().is_none());
    assert!(matches!(translate_type(&m, error_ty).unwrap(), s::Type::Object(_)));
    assert!(matches!(translate_type(&m, choice_ty).unwrap(), s::Type::Variant(_)));
}

#[test]
fn the_only_case_read_is_total_and_a_real_variant_read_stays_fallible() {
    let (m, error, choice) = module();
    let body = m.functions.get(o::FuncId::from_u32(0)).unwrap().body.as_ref().unwrap();
    let read = o::Operation::Core(o::CoreOp::VariantGet { value: error, case: 0, field: field(&m, "Short") });
    let other = o::Operation::Core(o::CoreOp::VariantGet { value: choice, case: 1, field: field(&m, "Item") });
    assert!(calls::only_case_read(&m, body, &read).unwrap());
    assert!(!calls::fallible_in(&m, body, &read).unwrap() && calls::total_invocation(&m, body, &read).unwrap());
    assert!(!calls::only_case_read(&m, body, &other).unwrap());
    assert!(calls::fallible_in(&m, body, &other).unwrap() && !calls::total_invocation(&m, body, &other).unwrap());
}

/// Every object of the fixture has one int field at storage 0; variants have no storage of their own.
fn layouts(m: &o::Module) -> BTreeMap<u32, ObjectLayout> {
    m.declarations.iter().map(|(id, declaration)| {
        let object = matches!(declaration.kind, o::DeclarationKind::Object { .. });
        (id.as_u32(), ObjectLayout {
            runtime: s::ObjectRuntimeLayout {
                storage_fields: if object { vec![s::Type::Int] } else { vec![] },
                allocation_words: if object { 2 } else { 1 }, identity_offset: None, has_destroy: false,
                may_reach_resource: false, may_async_destroy: false, trace_layout: vec![], selection_layout: vec![],
            },
            source_fields: if object { vec![0] } else { vec![] },
        })
    }).collect()
}

/// Translates `op` as an instruction of `@f` with a result of `ty`.
fn translated(op: impl Fn(o::ValueId, o::ValueId, &o::Module) -> o::CoreOp, ty: o::Type)
    -> Result<s::Instruction, GenerateError>
{
    let (mut m, error, choice) = module();
    let core = op(error, choice, &m);
    let ty = m.types.intern(ty).unwrap();
    let body = m.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
    let inst = o::InstId::from_u32(body.insts.len() as u32);
    let result = body.values.alloc(o::ValueData {
        ty, def: o::Definition::InstResult { inst, index: 0 }, facts: o::ValueFacts::Unelaborated,
    }).unwrap();
    body.insts.alloc(o::Instruction {
        results: vec![result], op: o::Operation::Core(core), facts: o::InstructionFacts::default(), loc: None,
    }).unwrap();
    let objects = with_composites(&m, || objects(&m, &layouts(&m)))?;
    let body = m.functions.get(o::FuncId::from_u32(0)).unwrap().body.as_ref().unwrap();
    translate_instruction(&m, body, body.insts.get(inst).unwrap(), &objects)
}

#[test]
fn variant_tag_of_the_only_error_case_is_the_constant_zero() {
    let tag = translated(|error, _, _| o::CoreOp::VariantTag { value: error }, o::Type::Int).unwrap();
    assert!(matches!(tag, s::Instruction::Data { op: s::DataOp::Const, ref inputs,
        immediate: s::Immediate::Constant(s::Constant::Int(ref zero)), .. } if inputs.is_empty() && zero == "0"));
    let tag = translated(|_, choice, _| o::CoreOp::VariantTag { value: choice }, o::Type::Int).unwrap();
    assert!(matches!(tag, s::Instruction::Data { op: s::DataOp::VariantTag, .. }));
}

#[test]
fn variant_get_of_the_only_error_case_reads_the_case_field() {
    let get = translated(|error, _, m| o::CoreOp::VariantGet { value: error, case: 0, field: field(m, "Short") },
        o::Type::Int).unwrap();
    assert!(matches!(get, s::Instruction::Data { op: s::DataOp::ObjGet, immediate: s::Immediate::Field(0), .. }));
    let get = translated(|_, choice, m| o::CoreOp::VariantGet { value: choice, case: 1, field: field(m, "Item") },
        o::Type::Int).unwrap();
    assert!(matches!(get, s::Instruction::Data { op: s::DataOp::VariantGet, .. }));
    // Another case or another layout is not the value's object.
    let wrong = translated(|error, _, m| o::CoreOp::VariantGet { value: error, case: 1, field: field(m, "Short") },
        o::Type::Int).unwrap_err();
    assert!(wrong.0.contains("only Error case"), "{wrong:?}");
    let wrong = translated(|error, _, m| o::CoreOp::VariantGet { value: error, case: 0, field: field(m, "Item") },
        o::Type::Int).unwrap_err();
    assert!(wrong.0.contains("only Error case"), "{wrong:?}");
}
