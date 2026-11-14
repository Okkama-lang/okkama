use super::*;

const LOOP: &str = include_str!("fixtures/scalar-loop.oir");
const OBJECT: &str = include_str!("fixtures/object-read.oir");
const BRANCH: &str = include_str!("fixtures/branching.oir");

fn parse(text: &str) -> o::Module {
    o::text::parse(text).unwrap_or_else(|error| panic!("{error}"))
}

fn layouts() -> BTreeMap<o::DeclId, ObjectLayout> {
    BTreeMap::from([(o::DeclId::from_u32(0), ObjectLayout {
        // Deliberate runtime override: source field 0 lives at physical slot 1.
        // This tests the registry seam, including a hidden construction slot.
        runtime: s::ObjectRuntimeLayout {
            storage_fields: vec![s::Type::Bool, s::Type::Int],
            allocation_words: 3,
            identity_offset: None,
            has_destroy: false,
            may_reach_resource: false,
            may_async_destroy: false,
            trace_layout: vec![],
            selection_layout: vec![],
        },
        source_fields: vec![1],
    })])
}

fn snapshot(p: &s::Program) -> String {
    let f = &p.functions[0];
    let mut text = String::new();
    for (pc, instruction) in f.code.iter().enumerate() {
        let edge = |edge: &s::Edge| {
            let moves = edge.assignments.iter().map(|assignment| {
                let s::Location::Register(dst) = assignment.destination else { panic!("S1 language location"); };
                let s::Source::Location(s::Location::Register(src)) = assignment.source else {
                    panic!("S1 language source");
                };
                format!("r{dst}<-r{src}")
            }).collect::<Vec<_>>().join(", ");
            format!("@{} [{moves}]", edge.target_pc)
        };
        match instruction {
            s::Instruction::Br(target) => text.push_str(&format!("{pc}: br {}\n", edge(target))),
            s::Instruction::Cond { condition, yes, no } => {
                text.push_str(&format!("{pc}: cond r{condition}\n  yes {}\n  no {}\n", edge(yes), edge(no)));
            }
            _ => text.push_str(&format!("{pc}: {instruction:?}\n")),
        }
    }
    text
}

fn golden(source: &str, expected: &str) -> s::Program {
    let m = parse(source);
    let p = generate(&m, &layouts()).unwrap();
    assert_eq!(snapshot(&p), expected);
    let bytes = s::encode(&p).unwrap();
    assert_eq!(s::decode(&bytes).unwrap(), p);
    let assembly = s::disassemble(&p).unwrap();
    assert_eq!(s::assemble(&assembly).unwrap(), p);
    p
}

#[test]
fn s1_scalar_loop_golden() {
    golden(LOOP, include_str!("fixtures/scalar-loop.golden"));
}

#[test]
fn s1_object_field_read_golden_uses_runtime_layout() {
    let p = golden(OBJECT, include_str!("fixtures/object-read.golden"));
    assert_eq!(p.objects[0].layout.storage_fields.len(), 2);
    assert_eq!(p.objects[0].fields, vec![s::Field { name: "value".into(), storage: 1, once: false }]);
}

#[test]
fn s1_branching_golden() {
    golden(BRANCH, include_str!("fixtures/branching.golden"));
}

#[test]
fn s1_rejects_wrong_profile_unsupported_and_invalid_input() {
    let mut m = parse(LOOP);
    m.profile = o::Profile::High;
    assert!(generate(&m, &layouts()).unwrap_err().0.contains("low profile"));
    let mut m = parse(LOOP);
    let body = m.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
    let id = body.blocks.get(body.entry).unwrap().insts[0];
    body.insts.get_mut(id).unwrap().op = o::Operation::Ownership(o::OwnershipOp::Empty {});
    assert!(generate(&m, &layouts()).unwrap_err().0.contains("own.empty: not yet in S1"));
    let mut m = parse(LOOP);
    let body = m.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
    let o::TermKind::Br(target) = &mut body.blocks.get_mut(body.entry).unwrap().term.kind else {
        panic!("fixture");
    };
    target.args.pop();
    assert!(generate(&m, &layouts()).unwrap_err().0.contains("edge arity"));
    let mut m = parse(LOOP);
    let body = m.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
    let value = body.blocks.get(body.entry).unwrap().params[0];
    body.values.get_mut(value).unwrap().def = o::Definition::InstResult { inst: o::InstId::from_u32(0), index: 0 };
    assert!(generate(&m, &layouts()).unwrap_err().0.contains("SSA definition"));
    let m = parse(OBJECT);
    assert!(generate(&m, &BTreeMap::new()).unwrap_err().0.contains("missing final runtime layout"));
}

#[test]
fn s1_rejects_type_errors_non_dominating_values_and_invented_registers() {
    let invalid = BRANCH.replace("int.add %value, %one", "int.add %flag, %one");
    assert!(generate(&parse(&invalid), &layouts()).unwrap_err().0.contains("type"));
    let invalid = BRANCH.replace("int.sub %value, %two", "int.sub %a, %two");
    assert!(generate(&parse(&invalid), &layouts()).unwrap_err().0.contains("dominate"));
    let mut m = parse(LOOP);
    let body = m.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
    let id = body.blocks.get(body.entry).unwrap().insts[0];
    body.insts.get_mut(id).unwrap().op = o::Operation::Core(o::CoreOp::IntNeg {
        value: o::ValueId::from_u32(u32::MAX),
    });
    assert!(generate(&m, &layouts()).is_err());
}

fn append(m: &mut o::Module, op: o::Operation, ty: Option<o::Type>) -> Option<o::ValueId> {
    let ty = ty.map(|ty| m.types.intern(ty).unwrap());
    let body = m.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
    let inst = o::InstId::from_u32(body.insts.len() as u32);
    let result = ty.map(|ty| body.values.alloc(o::ValueData {
        ty,
        def: o::Definition::InstResult { inst, index: 0 },
        facts: o::ValueFacts::Unelaborated,
    }).unwrap());
    let id = body.insts.alloc(o::Instruction {
        results: result.into_iter().collect(),
        op,
        facts: o::InstructionFacts::default(),
        loc: None,
    }).unwrap();
    body.blocks.get_mut(body.entry).unwrap().insts.push(id);
    result
}

#[test]
fn s1_scalar_allowlist_and_plain_object_operations() {
    let mut m = parse("oir 2 low\nmodule all\nobject Box {\nfield value: int\nfield pending: bool\n}\n\
        scope S action in @f\n\
        func @f(%a: int, %b: int, %flag: bool, %text: str, %r: range, %box: Box) -> void abi observation {\n\
        ^entry in S:\nret\n}\n");
    let o::DeclarationKind::Object { fields, .. } = &mut m.declarations
        .get_mut(o::DeclId::from_u32(0)).unwrap().kind else { panic!("fixture"); };
    fields[1].name = "$pending".into();
    let mut registry = layouts();
    registry.get_mut(&o::DeclId::from_u32(0)).unwrap().source_fields = vec![1, 0];
    let value = o::ValueId::from_u32;
    let (a, b, flag, text, range, object) = (value(0), value(1), value(2), value(3), value(4), value(5));
    use o::CoreOp as C;
    let int_binary = [
        C::IntAdd { left: a, right: b },
        C::IntSub { left: a, right: b },
        C::IntMul { left: a, right: b },
        C::IntMod { left: a, right: b },
    ];
    for op in int_binary {
        append(&mut m, o::Operation::Core(op), Some(o::Type::Int));
    }
    for op in [
        C::IntEq { left: a, right: b }, C::IntNe { left: a, right: b },
        C::IntLt { left: a, right: b }, C::IntLe { left: a, right: b },
        C::IntGt { left: a, right: b }, C::IntGe { left: a, right: b },
        C::BoolEq { left: flag, right: flag }, C::BoolNe { left: flag, right: flag },
        C::BoolNot { value: flag },
        C::StrEq { left: text, right: text }, C::StrNe { left: text, right: text },
        C::StrLt { left: text, right: text }, C::StrLe { left: text, right: text },
        C::StrGt { left: text, right: text }, C::StrGe { left: text, right: text },
    ] {
        append(&mut m, o::Operation::Core(op), Some(o::Type::Bool));
    }
    for op in [C::IntNeg { value: a }, C::StrLen { value: text }, C::RangeStart { value: range },
        C::RangeEnd { value: range }, C::RangeLen { value: range }]
    {
        append(&mut m, o::Operation::Core(op), Some(o::Type::Int));
    }
    append(&mut m, o::Operation::Core(C::StrConcat { left: text, right: text }), Some(o::Type::Str));
    append(&mut m, o::Operation::Core(C::RangeNew { start: a, end: b, inclusive: true }), Some(o::Type::Range));
    for (input, from) in [(a, o::NumericType::Int), (flag, o::NumericType::Bool), (text, o::NumericType::Str)] {
        for (ty, to) in [(o::Type::Int, o::NumericType::Int), (o::Type::Bool, o::NumericType::Bool),
            (o::Type::Str, o::NumericType::Str)]
        {
            append(&mut m, o::Operation::Core(C::Convert { value: input, from, to }), Some(ty));
        }
    }
    append(&mut m, o::Operation::Const(o::Literal::Bool(true)), Some(o::Type::Bool));
    append(&mut m, o::Operation::Const(o::Literal::Str("text".into())), Some(o::Type::Str));
    let decl = o::DeclId::from_u32(0);
    let object_type = o::Type::Object { decl, args: vec![] };
    append(&mut m, o::Operation::Core(C::ObjNew {
        declaration: decl,
        type_args: vec![],
        fields: vec![a, flag],
        packet: None,
        construction: o::ConstructionStatus::Complete,
    }), Some(object_type.clone()));
    let field = o::FieldRef { declaration: decl, index: 0 };
    append(&mut m, o::Operation::Core(C::ObjInit { object, field, value: a, completion: None }), None);
    let object_ty = m_type(&m, object_type.clone());
    let next = append(&mut m, o::Operation::Core(C::ObjNext {
        object,
        updates: vec![o::FieldValue { field, value: b }],
        packet: None,
    }), Some(o::Type::Successor(object_ty))).unwrap();
    let ty = m_type(&m, object_type.clone());
    append(&mut m, o::Operation::Core(C::SuccTake { value: next, ty }), Some(object_type));
    let int = m_type(&m, o::Type::Int);
    let unset = append(&mut m, o::Operation::Core(C::Undef { ty: int }), Some(o::Type::Int)).unwrap();
    append(&mut m, o::Operation::Core(C::ObjInit { object, field, value: unset, completion: None }), None);
    append(&mut m, o::Operation::Core(C::ObjPending {
        object,
        field: o::FieldRef { declaration: decl, index: 1 },
    }), Some(o::Type::Bool));
    let p = generate(&m, &registry).unwrap();
    assert_eq!(s::decode(&s::encode(&p).unwrap()).unwrap(), p);
    assert_eq!(s::assemble(&s::disassemble(&p).unwrap()).unwrap(), p);
    assert!(p.functions[0].registers.len() > m.functions.get(o::FuncId::from_u32(0)).unwrap()
        .body.as_ref().unwrap().values.len());
}

fn m_type(m: &o::Module, expected: o::Type) -> o::TypeId {
    m.types.iter().find_map(|(id, ty)| (*ty == expected).then_some(id)).unwrap()
}

#[test]
fn s1_switch_expansion_and_parallel_edge_cycle() {
    let mut m = parse(BRANCH);
    let body = m.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
    let first = body.entry;
    let o::TermKind::Cond { yes, no, .. } = body.blocks.get(first).unwrap().term.kind.clone() else {
        panic!("fixture");
    };
    let value = body.blocks.get(first).unwrap().params[1];
    body.blocks.get_mut(first).unwrap().term.kind = o::TermKind::Switch {
        value,
        cases: vec![
            o::SwitchCase { value: o::Literal::Int("1".into()), target: yes },
            o::SwitchCase { value: o::Literal::Int("2".into()), target: no.clone() },
        ],
        default: no,
    };
    let p = generate(&m, &layouts()).unwrap();
    assert_eq!(p.functions[0].code.iter().filter(|inst| matches!(inst, s::Instruction::Cond { .. })).count(), 2);
    assert_eq!(s::decode(&s::encode(&p).unwrap()).unwrap(), p);
    let mut m = parse(LOOP);
    let body = m.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
    let loop_id = o::BlockId::from_u32(1);
    let params = body.blocks.get(loop_id).unwrap().params.clone();
    let back = body.blocks.get_mut(o::BlockId::from_u32(2)).unwrap();
    back.term.kind = o::TermKind::Br(o::Target { block: loop_id, args: vec![params[1], params[0]] });
    let p = generate(&m, &layouts()).unwrap();
    let s::Instruction::Br(edge) = &p.functions[0].code[7] else { panic!("fixture"); };
    assert_eq!(edge.assignments, vec![
        s::Assignment { destination: s::Location::Register(3),
            source: s::Source::Location(s::Location::Register(4)) },
        s::Assignment { destination: s::Location::Register(4),
            source: s::Source::Location(s::Location::Register(3)) },
    ]);
    assert_eq!(s::decode(&s::encode(&p).unwrap()).unwrap(), p);
}

#[test]
fn s1_rejects_cleanup_defer_and_lost_or_duplicate_successor_permissions() {
    for op in [
        o::Operation::Cleanup(o::CleanupOp::Plan { cleanup: o::ValueId::from_u32(0) }),
        o::Operation::Lexical(o::LexicalOp::DeferRegister {
            scope: o::ScopeId::from_u32(0),
            action: o::DeferAction { function: o::FuncId::from_u32(0), captures: vec![], bindings: vec![] },
        }),
    ] {
        let name = op.opcode();
        let mut m = parse(LOOP);
        let body = m.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
        let inst = body.blocks.get(body.entry).unwrap().insts[0];
        body.insts.get_mut(inst).unwrap().op = op;
        let error = generate(&m, &layouts()).unwrap_err();
        assert!(error.0.contains(&format!("{name}: not yet in S1")), "{error}");
    }
    let mut m = parse(OBJECT);
    let decl = o::DeclId::from_u32(0);
    let object = o::ValueId::from_u32(0);
    let object_type = o::Type::Object { decl, args: vec![] };
    let ty = m_type(&m, object_type.clone());
    let successor = append(&mut m, o::Operation::Core(o::CoreOp::ObjNext {
        object, updates: vec![], packet: None,
    }), Some(o::Type::Successor(ty))).unwrap();
    assert!(generate(&m, &layouts()).unwrap_err().0.contains("must-use"));
    append(&mut m, o::Operation::Core(o::CoreOp::SuccTake { value: successor, ty }), Some(object_type.clone()));
    assert!(generate(&m, &layouts()).is_ok());
    append(&mut m, o::Operation::Core(o::CoreOp::SuccTake { value: successor, ty }), Some(object_type));
    assert!(generate(&m, &layouts()).unwrap_err().0.contains("already taken"));
}
