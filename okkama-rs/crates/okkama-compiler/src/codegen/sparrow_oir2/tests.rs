use super::*;

const LOOP: &str = include_str!("fixtures/scalar-loop.oir");
const OBJECT: &str = include_str!("fixtures/object-read.oir");
const BRANCH: &str = include_str!("fixtures/branching.oir");

fn parse(text: &str) -> o::Module {
    o::text::parse(text).unwrap_or_else(|error| panic!("{error}"))
}

fn layouts() -> BTreeMap<u32, ObjectLayout> {
    BTreeMap::from([(0, ObjectLayout {
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
    registry.get_mut(&0).unwrap().source_fields = vec![1, 0];
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
    // OIR-2 cvt defines only the conversions to string among int/bool/string (#622).
    for (input, from) in [(a, o::NumericType::Int), (flag, o::NumericType::Bool)] {
        append(&mut m, o::Operation::Core(C::Convert { value: input, from, to: o::NumericType::Str }),
            Some(o::Type::Str));
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
        o::Operation::Cleanup(o::CleanupOp::Plan {
            cleanup: o::ValueId::from_u32(0), plan: o::CleanupPlanId::from_u32(0),
        }),
        o::Operation::Lexical(o::LexicalOp::DeferRegister {
            scope: o::ScopeId::from_u32(0),
            action: o::DeferAction {
                function: o::FuncId::from_u32(0),
                captures: vec![],
                bindings: vec![],
                replacements: vec![],
            },
        }),
    ] {
        let name = op.opcode();
        let mut m = parse(LOOP);
        let body = m.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
        let inst = body.blocks.get(body.entry).unwrap().insts[0];
        body.insts.get_mut(inst).unwrap().op = op;
        let error = generate(&m, &layouts()).unwrap_err();
        let stage = if name.starts_with("cleanup.") { "S2" } else { "S1" };
        assert!(error.0.contains(&format!("{name}: not yet in {stage}")), "{error}");
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
    // OIR-2 §8: a successor is used at least once and its value may be read again, so a second succ.take is
    // legal OIR (ruling of 2026-11-15, #676). Codegen reads the first take's object instead of spending the
    // permission twice; the loader still rejects a duplicate take inside SPB 17 (spb17::roles_tests).
    append(&mut m, o::Operation::Core(o::CoreOp::SuccTake { value: successor, ty }), Some(object_type));
    assert!(generate(&m, &layouts()).is_ok());
}

/// The OIR of `read` with a successor of its receiver taken twice on one path.
fn taken_twice() -> o::Module {
    let mut m = parse(OBJECT);
    let decl = o::DeclId::from_u32(0);
    let object_type = o::Type::Object { decl, args: vec![] };
    let ty = m_type(&m, object_type.clone());
    let successor = append(&mut m, o::Operation::Core(o::CoreOp::ObjNext {
        object: o::ValueId::from_u32(0), updates: vec![], packet: None,
    }), Some(o::Type::Successor(ty))).unwrap();
    for _ in 0..2 {
        append(&mut m, o::Operation::Core(o::CoreOp::SuccTake { value: successor, ty }), Some(object_type.clone()));
    }
    m
}

#[test]
fn a_successor_taken_twice_spends_one_permission_and_loads() {
    let p = generate(&taken_twice(), &layouts()).unwrap();
    let f = &p.functions[0];
    let moves = f.code.iter().filter_map(|instruction| match instruction {
        s::Instruction::Data { op: s::DataOp::Move, inputs, output: Some(output), .. } => Some((inputs[0], *output)),
        _ => None,
    }).collect::<Vec<_>>();
    let [(first, object), (second, _)] = moves.as_slice() else { panic!("two takes: {moves:?}") };
    assert!(matches!(f.registers[*first as usize], s::Type::Successor(0)));
    // The second take reads the object the first produced: one permission is spent.
    assert_eq!(second, object);
    s::verify_load(&p, &s::NoExternalContracts).unwrap();
}

#[test]
fn champ_registry_reemits_571_object_read_golden() {
    use s::layout::{LayoutFacts, ObjectRuntimeLayout, RuntimeLayoutRegistry};
    use okkama_sparrow::spb17 as s;
    let module = okkama_oir::text::parse(include_str!(
        "fixtures/object-read.oir"
    )).unwrap();
    let declaration = 0;
    let mut registry = RuntimeLayoutRegistry::default();
    registry.register(declaration, vec![s::Type::Bool, s::Type::Int], vec![1], LayoutFacts::default(), vec![]).unwrap();
    let layout = registry.object_layout(declaration).unwrap();
    assert_eq!(layout.runtime.allocation_words, 3);
    assert_eq!(layout.source_fields, vec![1]);
    let frozen = ObjectRuntimeLayout::from_spb17(&layout.runtime).unwrap();
    assert_eq!(frozen.storage_fields(), &[s::layout::Repr::Raw, s::layout::Repr::Int]);
    assert_eq!(frozen.field_offset(1), 16);
    let program = generate(&module, &registry).unwrap();
    let mut snapshot = String::new();
    for (pc, instruction) in program.functions[0].code.iter().enumerate() {
        snapshot.push_str(&format!("{pc}: {instruction:?}\n"));
    }
    assert_eq!(snapshot, include_str!(
        "fixtures/object-read.golden"
    ));
    let bytes = s::encode(&program).unwrap();
    assert_eq!(s::decode(&bytes).unwrap(), program);
    assert_eq!(s::assemble(&s::disassemble(&program).unwrap()).unwrap(), program);
    assert!(registry.object_layout(1).is_err());
    assert!(registry.register(declaration, vec![], vec![], LayoutFacts::default(), vec![]).is_err());
}

fn s2_golden(source: &str, expected: &str) -> s::Program {
    let module = parse(source);
    let generated = generate_with_allocation(&module, &layouts()).unwrap();
    assert_eq!(generated.allocations.len(), generated.program.functions.len());
    let program = generated.program;
    s::verify(&program).unwrap();
    assert_eq!(s::decode(&s::encode(&program).unwrap()).unwrap(), program);
    assert_eq!(s::assemble(&s::disassemble(&program).unwrap()).unwrap(), program);
    let mut actual = String::new();
    for function in &program.functions {
        actual.push_str(&format!("{} {:?}\n", function.symbol, function.entry_abi));
        actual.push_str(&format!("registers {:?}\nservices {:?}\n", function.registers, function.services));
        for (pc, instruction) in function.code.iter().enumerate() {
            actual.push_str(&format!("{pc}: {instruction:#?}\n"));
        }
        let allocation = s::allocate(function).unwrap();
        s::check_allocation(function, &allocation).unwrap();
    }
    assert_eq!(actual, expected);
    program
}

#[test]
fn s2_owned_call_outcomes_and_unwind_only_liveness() {
    // The fixture entry supplies the fresh offered packet. Its contents are opaque to codegen.
    let program = s2_golden(include_str!("fixtures/owned-call.oir"), include_str!("fixtures/owned-call.golden"));
    let function = &program.functions[0];
    let mut without_metadata = function.clone();
    without_metadata.proofs.roots.clear();
    let live = s::live_locations(&without_metadata).unwrap();
    assert!(live[0].contains(&s::Location::Register(2)));
    assert!(!live[0].contains(&s::Location::Register(3)), "Post is not a call input root");
    let s::Instruction::Champ { op: s::Opcode::InvokeCallOwn, flow: s::Flow::Outcomes(outcomes), .. }
        = &function.code[0] else { panic!("owned call") };
    assert_eq!(outcomes.iter().map(|outcome| outcome.tag).collect::<Vec<_>>(), [s::Tag::Normal, s::Tag::Unwind]);
    assert_eq!(outcomes[0].edge.assignments[2].source, s::Source::ProducedSlot(2));
}

#[test]
fn s2_empty_input_packet_and_void_results() {
    s2_golden(include_str!("fixtures/empty-call.oir"), include_str!("fixtures/empty-call.golden"));
}

#[test]
fn s2_defer_continue_replace_and_unwind() {
    let program = s2_golden(include_str!("fixtures/defer-call.oir"), include_str!("fixtures/defer-call.golden"));
    let s::Instruction::Champ { flow: s::Flow::Outcomes(outcomes), .. } = &program.functions[0].code[0]
        else { panic!("defer call") };
    assert_eq!(outcomes.iter().map(|outcome| outcome.tag).collect::<Vec<_>>(),
        [s::Tag::Continue, s::Tag::Replace(0), s::Tag::Replace(1), s::Tag::Unwind]);
    assert_eq!(program.functions[0].proofs.registrations[0].labels.len(), 2);
}

#[test]
fn s2_shielded_preserves_application_unwind_without_cancel() {
    let program = s2_golden(include_str!("fixtures/shielded-call.oir"),
        include_str!("fixtures/shielded-call.golden"));
    let s::Instruction::Champ { arguments, flow: s::Flow::Outcomes(outcomes), .. } = &program.functions[0].code[0]
        else { panic!("shielded call") };
    assert!(arguments.contains(&s::Argument::Flag(true)));
    assert_eq!(outcomes.len(), 2);
}

#[test]
fn s2_checker_rejects_a_clobbered_parallel_cycle() {
    let mut module = parse(LOOP);
    let body = module.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
    let loop_id = o::BlockId::from_u32(1);
    let params = body.blocks.get(loop_id).unwrap().params.clone();
    body.blocks.get_mut(o::BlockId::from_u32(2)).unwrap().term.kind = o::TermKind::Br(o::Target {
        block: loop_id, args: vec![params[1], params[0]],
    });
    let program = generate(&module, &layouts()).unwrap();
    let function = &program.functions[0];
    let allocation = s::allocate(function).unwrap();
    s::check_allocation(function, &allocation).unwrap();
    let mut clobbered = allocation.clone();
    let copies = clobbered.edges.get_mut(&(7, 0)).unwrap();
    assert_eq!(copies.len(), 3, "cycle needs scratch before the two assignments");
    copies.remove(0);
    assert!(s::check_allocation(function, &clobbered).unwrap_err().message.contains("clobbered"));
}

#[test]
fn s2_rejects_foreign_slots_missing_packet_and_shield_outside_cleanup() {
    let source = include_str!("fixtures/owned-call.oir");
    let mut module = parse(source);
    let body = module.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
    let o::TermKind::InvokeOwn { normal, .. } = &mut body.blocks.get_mut(body.entry).unwrap().term.kind
        else { panic!("call fixture") };
    normal.args[0] = o::EdgeArg::Produced(o::EdgeResult { outcome: o::Outcome::Unwind, slot: 0 });
    assert!(generate(&module, &layouts()).unwrap_err().0.contains("another outcome"));
    let mut module = parse(source);
    let body = module.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
    let o::TermKind::InvokeOwn { op, .. } = &mut body.blocks.get_mut(body.entry).unwrap().term.kind
        else { panic!("call fixture") };
    let o::Operation::Call(o::CallOp::Direct { call, .. }) = op.as_mut() else { panic!("direct call") };
    call.packet = None;
    assert!(generate(&module, &layouts()).unwrap_err().0.contains("input packet"));
    let source = include_str!("fixtures/shielded-call.oir").replace(" [cleanup]", "");
    assert!(generate(&parse(&source), &layouts()).unwrap_err().0.contains("outside cleanup"));
}

#[test]
fn s2_metadata_only_dependencies_survive_allocation_and_roundtrip() {
    let mut module = parse(include_str!("fixtures/owned-call.oir"));
    let body = module.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
    let owner = body.blocks.get(body.entry).unwrap().params[0];
    let dependency = body.blocks.get(body.entry).unwrap().params[4];
    let deps = body.deps.alloc(o::DepExpr::FromSsa(dependency)).unwrap();
    body.values.get_mut(owner).unwrap().facts = o::ValueFacts::Managed { deps };
    let program = generate(&module, &layouts()).unwrap();
    let function = &program.functions[0];
    assert!(function.proofs.dependencies.contains(&s::Dependency {
        value: s::Location::Register(0), obligations: vec![s::Location::Register(2)],
    }));
    assert!(s::live_locations(function).unwrap()[0].contains(&s::Location::Register(2)));
    assert_eq!(s::decode(&s::encode(&program).unwrap()).unwrap(), program);
    s::verify(&program).unwrap();
}

#[test]
fn s3_malformed_cleanup_and_wrong_pending_operands_are_rejected() {
    let mut module = parse(include_str!("fixtures/owned-call.oir"));
    let body = module.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
    let id = body.insts.alloc(o::Instruction {
        results: vec![],
        op: o::Operation::Cleanup(o::CleanupOp::Plan {
            cleanup: o::ValueId::from_u32(0), plan: o::CleanupPlanId::from_u32(0),
        }),
        facts: o::InstructionFacts::default(), loc: None,
    }).unwrap();
    body.blocks.get_mut(body.entry).unwrap().insts.push(id);
    assert!(generate(&module, &layouts()).is_err());
    let pending = include_str!("fixtures/owned-call.oir").replace("%full: deps", "%full: pending");
    assert!(generate(&parse(&pending), &layouts()).is_err());
}

#[test]
fn s2_registration_order_does_not_reorder_outcomes() {
    let mut module = parse(include_str!("fixtures/defer-call.oir"));
    let expected = generate(&module, &layouts()).unwrap();
    let body = module.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
    let o::TermKind::InvokeDefer { replacements, .. } = &mut body.blocks.get_mut(body.entry).unwrap().term.kind
        else { panic!("defer fixture") };
    replacements.reverse();
    assert_eq!(generate(&module, &layouts()).unwrap(), expected);
}

#[test]
fn s2_packet_lifetime_is_explicit_and_leaked_packet_is_rejected() {
    let source = include_str!("fixtures/empty-call.oir");
    let program = generate(&parse(source), &layouts()).unwrap();
    let caller = &program.functions[0];
    // The original input is not a language argument of the nested call, but remains an active holder.
    assert!(caller.proofs.states.iter().any(|state| state.pc == 1
        && state.locations.contains(&s::Location::Service(0))));
    let leaked = source.replace("%joined: own = own.join [%original, %packet]", "%joined: own = own.move %packet");
    assert!(generate(&parse(&leaked), &layouts()).unwrap_err().0.contains("loses live service"));
}

#[test]
fn s2_checker_rejects_wrong_produced_slot_copy() {
    let module = parse(include_str!("fixtures/owned-call.oir"));
    let generated = generate_with_allocation(&module, &layouts()).unwrap();
    let mut allocation = generated.allocations[0].clone();
    let copies = allocation.edges.get_mut(&(0, 0)).unwrap();
    let copy = copies.iter_mut().find(|copy| copy.source == s::CopySource::Produced(1)).unwrap();
    copy.source = s::CopySource::Produced(0);
    assert!(s::check_allocation(&generated.program.functions[0], &allocation).is_err());
}

#[test]
fn s2_multi_result_outcome_with_parallel_cycle_golden() {
    let program = s2_golden(include_str!("fixtures/multi-result-cycle.oir"),
        include_str!("fixtures/multi-result-cycle.golden"));
    let caller = &program.functions[0];
    let mut allocation = s::allocate(caller).unwrap();
    let copies = allocation.edges.get_mut(&(1, 0)).unwrap();
    let scratch = copies.iter().position(|copy| {
        !allocation.locations.values().any(|&slot| slot == copy.destination)
    }).expect("multi-result normal edge must save the cycle source");
    copies.remove(scratch);
    assert!(s::check_allocation(caller, &allocation).is_err());
}

#[test]
fn s2_instruction_metadata_only_value_is_live_across_call() {
    let source = include_str!("fixtures/owned-call.oir")
        .replace("%unwind_only: str)", "%unwind_only: str, %metadata_only: str)");
    let mut module = parse(&source);
    let body = module.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
    let value = body.blocks.get(body.entry).unwrap().params[5];
    let selection = body.selections.alloc(o::SelectionExpr::Value(value)).unwrap();
    let inst = body.blocks.get(o::BlockId::from_u32(3)).unwrap().insts[0];
    body.insts.get_mut(inst).unwrap().facts.retention.push(selection);
    let program = generate(&module, &layouts()).unwrap();
    let caller = &program.functions[0];
    assert!(s::live_locations(caller).unwrap()[0].contains(&s::Location::Register(3)));
    s::verify(&program).unwrap();
    assert_eq!(s::decode(&s::encode(&program).unwrap()).unwrap(), program);
}

#[test]
fn s2_summary_retention_keeps_language_parameter_positions() {
    let mut module = parse(include_str!("fixtures/owned-call.oir"));
    let callee = module.functions.get_mut(o::FuncId::from_u32(1)).unwrap();
    callee.signature.params[..3].rotate_right(1);
    callee.signature.ownership_abi = o::OwnershipAbi::Explicit { input_packet: 0, output_packet: 1 };
    let body = callee.body.as_mut().unwrap();
    body.blocks.get_mut(body.entry).unwrap().params[..3].rotate_right(1);
    let params = body.blocks.get(body.entry).unwrap().params.clone();
    for (index, value) in params.into_iter().enumerate() {
        body.values.get_mut(value).unwrap().def = o::Definition::BlockParam { block: body.entry, index: index as u32 };
    }
    let summary = module.summaries.get_mut(o::SummaryId::from_u32(0)).unwrap();
    summary.parameters[0].retention = Some(o::SummarySelection::Parameter(1));
    let program = generate(&module, &layouts()).unwrap();
    assert_eq!(program.tables.summaries[0].retained_params, [2]);
    s::verify(&program).unwrap();
    assert_eq!(s::decode(&s::encode(&program).unwrap()).unwrap(), program);
}

const FALLIBLE: &str = "oir 2 low\nmodule fallible\nscope S action in @f\n\
    summary M0 internal parameters [parameter_effect(convention=borrow, destination=none),\n\
    parameter_effect(convention=borrow, destination=none)] contract call_contract(failures=[program],\n\
    publication=atomic)\n\
    func @f(%a: int, %b: int) -> int summary(M0) {\n\
    ^entry in S UNWIND:\n    %m: int = int.mod %a, %b\n    %n: int = int.add %m, %a\n    ret %n\nFAILED}\n";

fn fallible_program(unwind: bool) -> s::Program {
    let (header, failed) = if unwind {
        (" unwind ^failed", "^failed(%f: failure) in S:\n    raise %f\n")
    } else {
        ("", "")
    };
    let module = parse(&FALLIBLE.replace(" UNWIND", header).replace("FAILED", failed));
    let program = generate(&module, &layouts()).unwrap();
    s::verify_load(&program, &s::NoExternalContracts).unwrap();
    program
}

#[test]
fn s2_fallible_scalar_without_block_unwind_leaves_through_raise() {
    let program = fallible_program(false);
    let code = &program.functions[0].code;
    let s::Instruction::Champ { op: s::Opcode::Evaluate, arguments, flow: s::Flow::Outcomes(outcomes) } = &code[0]
        else { panic!("evaluate: {:?}", code[0]) };
    assert!(matches!(&arguments[0], s::Argument::Evaluation(evaluation) if evaluation.op == s::DataOp::IntMod));
    assert_eq!(outcomes.iter().map(|outcome| (outcome.tag, outcome.edge.target_pc)).collect::<Vec<_>>(),
        [(s::Tag::Normal, 2), (s::Tag::Unwind, 1)]);
    assert!(matches!(code[1], s::Instruction::Raise(_)));
    assert!(matches!(code[2], s::Instruction::Data { op: s::DataOp::IntAdd, .. }));
    let function = &program.functions[0];
    assert!(!code.iter().any(|instruction| function.data_fallible(instruction)));
}

#[test]
fn s2_fallible_scalar_unwinds_into_the_block_unwind() {
    let program = fallible_program(true);
    let function = &program.functions[0];
    let s::Instruction::Champ { op: s::Opcode::Evaluate, flow: s::Flow::Outcomes(outcomes), .. } = &function.code[0]
        else { panic!("evaluate: {:?}", function.code[0]) };
    let unwind = &outcomes[1].edge;
    assert_eq!((outcomes[0].edge.target_pc, outcomes[1].tag), (1, s::Tag::Unwind));
    let target = function.blocks.iter().position(|block| block.pc == unwind.target_pc).unwrap();
    assert!(matches!(function.code[unwind.target_pc as usize], s::Instruction::Raise(_)));
    assert_eq!(function.blocks[target].params.len(), 1);
}

/// OIR-2 may take a successor alias twice (§8); SPB 17 forwards the first take's object instead (#622).
#[test]
fn s2_taken_successor_alias_forwards_its_object() {
    let take = |input, output| s::Instruction::Data {
        op: s::DataOp::Move, inputs: vec![input], output: Some(output), immediate: s::Immediate::None,
    };
    let mut function = s::Function {
        symbol: "main".into(),
        entry_abi: s::EntryAbi::Observation,
        registers: vec![s::Type::Successor(0), s::Type::Object(0), s::Type::Successor(0), s::Type::Object(0)],
        services: vec![],
        params: vec![s::Location::Register(0)],
        results: vec![],
        blocks: vec![
            s::Block { pc: 0, params: vec![s::Location::Register(0)] },
            s::Block { pc: 2, params: vec![s::Location::Register(2)] },
        ],
        proofs: s::FunctionProofs::default(),
        code: vec![
            take(0, 1),
            s::Instruction::Br(s::Edge { target_pc: 2, assignments: vec![s::Assignment {
                destination: s::Location::Register(2),
                source: s::Source::Location(s::Location::Register(0)),
            }] }),
            take(2, 3),
            s::Instruction::Ret(vec![]),
        ],
    };
    super::successors::forward_taken(&mut function).unwrap();
    assert_eq!(function.registers[2], s::Type::Object(0));
    let s::Instruction::Br(edge) = &function.code[1] else { panic!("branch") };
    assert_eq!(edge.assignments[0].source, s::Source::Location(s::Location::Register(1)));
    assert!(function.proofs.roots.iter().any(|root| root.pc == 1 && root.locations == [s::Location::Register(1)]));
    // A permission that is still held is never retyped.
    let mut held = function.clone();
    held.registers[2] = s::Type::Successor(0);
    held.code[0] = s::Instruction::Data { op: s::DataOp::Const, inputs: vec![], output: Some(1),
        immediate: s::Immediate::None };
    let s::Instruction::Br(edge) = &mut held.code[1] else { panic!("branch") };
    edge.assignments[0].source = s::Source::Location(s::Location::Register(0));
    super::successors::forward_taken(&mut held).unwrap();
    assert_eq!(held.registers[2], s::Type::Successor(0));
}

/// main(r0: ^Plain) prepares r0 as the exit value (pc 0) and then fails (pc 1) into an unwind block (pc 2)
/// that raises: the pending exit, not r0, holds r0's must-use permission on the unwind edge (#680).
fn prepared_successor() -> s::Function {
    let location = |argument| s::Argument::Location(argument);
    s::Function {
        symbol: "main".into(),
        entry_abi: s::EntryAbi::Owned,
        registers: vec![s::Type::Successor(0), s::Type::Failure, s::Type::Failure],
        services: vec![s::ServiceKind::Packet, s::ServiceKind::Pending, s::ServiceKind::Pending],
        params: vec![s::Location::Register(0), s::Location::Register(1), s::Location::Service(0)],
        results: vec![s::Type::Successor(0)],
        blocks: vec![
            s::Block { pc: 0, params: vec![s::Location::Register(0), s::Location::Register(1),
                s::Location::Service(0)] },
            s::Block { pc: 2, params: vec![s::Location::Register(2), s::Location::Service(2)] },
        ],
        proofs: s::FunctionProofs {
            selections: vec![s::Selection { locations: vec![s::Location::Register(0)] }],
            ..s::FunctionProofs::default()
        },
        code: vec![
            s::Instruction::Champ { op: s::Opcode::ExitPrepare, arguments: vec![s::Argument::Kind(s::ExitKind::Return),
                s::Argument::Boundary(0), location(s::Location::Register(0)), location(s::Location::Service(0)),
                s::Argument::Selection(0)], flow: s::Flow::Next(vec![s::Location::Service(1)]) },
            s::Instruction::Champ { op: s::Opcode::Fail, arguments: vec![s::Argument::Label(0),
                location(s::Location::Register(1))], flow: s::Flow::Outcomes(vec![s::Outcome { tag: s::Tag::Unwind,
                edge: s::Edge { target_pc: 2, assignments: vec![
                    s::Assignment { destination: s::Location::Register(2), source: s::Source::ProducedSlot(0) },
                    s::Assignment { destination: s::Location::Service(2),
                        source: s::Source::Location(s::Location::Service(1)) },
                ] } }]) },
            s::Instruction::Raise(2),
        ],
    }
}

#[test]
fn s2_permission_effects_follow_the_loader() {
    let function = prepared_successor();
    let effects = |instruction: &s::Instruction| super::successors::permission_effects(&function, instruction);
    assert_eq!(effects(&function.code[0]), (vec![s::Location::Register(0)], vec![]));
    assert_eq!(effects(&function.code[1]), (vec![], vec![]));
    let ret = s::Instruction::Champ { op: s::Opcode::RetOwn, arguments: vec![
        s::Argument::Location(s::Location::Register(0)), s::Argument::Location(s::Location::Service(0))],
        flow: s::Flow::Next(vec![]) };
    assert_eq!(effects(&ret), (vec![s::Location::Register(0)], vec![]));
    let observe = s::Instruction::Champ { op: s::Opcode::InvokeObserve, arguments: vec![s::Argument::Entry(0),
        s::Argument::Summary(0), s::Argument::Location(s::Location::Register(1))],
        flow: s::Flow::Next(vec![s::Location::Register(0)]) };
    assert_eq!(effects(&observe), (vec![], vec![s::Location::Register(0)]));
    let take = s::Instruction::Data { op: s::DataOp::Move, inputs: vec![0], output: Some(1),
        immediate: s::Immediate::None };
    assert_eq!(effects(&take), (vec![s::Location::Register(0)], vec![]));
}

#[test]
fn s2_unwind_does_not_discharge_a_successor_moved_into_its_exit() {
    let mut function = prepared_successor();
    let before = function.clone();
    super::successors::close_unwind_permissions(&mut function).unwrap();
    assert_eq!(function, before);
    // Without exit.prepare the parameter keeps the permission, and the unwind edge discharges it with a take.
    // (Only proof-position services remain, so the raise leaves no linear service live.)
    let mut held = prepared_successor();
    held.services = vec![s::ServiceKind::Deps; 3];
    held.code[0] = s::Instruction::Champ { op: s::Opcode::DepsEmpty, arguments: vec![],
        flow: s::Flow::Next(vec![s::Location::Service(1)]) };
    super::successors::close_unwind_permissions(&mut held).unwrap();
    assert!(held.code.iter().any(|instruction| matches!(instruction,
        s::Instruction::Data { op: s::DataOp::Move, inputs, .. } if inputs == &[0])), "{:?}", held.code);
}

/// Every int, byte, float, rat and str family operation of OIR-2 on parameters of its operand types.
fn family_module() -> (o::Module, Vec<(o::CoreOp, o::Type)>) {
    let mut m = parse("oir 2 low\nmodule all\nscope S action in @f\n\
        func @f(%i: int, %j: int, %b: byte, %c: byte, %x: float, %y: float, %r: rat, %q: rat, %s: str, %t: str, \
        %u: str) -> void abi observation {\n^entry in S:\nret\n}\n");
    let value = o::ValueId::from_u32;
    let (i, j, b, c, x, y) = (value(0), value(1), value(2), value(3), value(4), value(5));
    let (r, q, s, t, u) = (value(6), value(7), value(8), value(9), value(10));
    use o::CoreOp as C;
    use o::Type as T;
    let str_array = m.types.iter().find_map(|(id, ty)| (*ty == T::Str).then_some(id));
    let str_array = T::Array(str_array.expect("str type of the fixture"));
    let byte_array = m.types.iter().find_map(|(id, ty)| (*ty == T::Byte).then_some(id));
    let byte_array = T::Array(byte_array.expect("byte type of the fixture"));
    let char_array = T::Array(m.types.intern(T::Char).unwrap());
    let ops = vec![
        (C::IntAnd { left: i, right: j }, T::Int), (C::IntOr { left: i, right: j }, T::Int),
        (C::IntXor { left: i, right: j }, T::Int), (C::IntShl { left: i, right: j }, T::Int),
        (C::IntShr { left: i, right: j }, T::Int), (C::IntUshr { left: i, right: j }, T::Int),
        (C::IntNot { value: i }, T::Int), (C::IntAbs { value: i }, T::Int), (C::IntSign { value: i }, T::Int),
        (C::IntBitCount { value: i }, T::Int), (C::IntLeadingZeros { value: i }, T::Int),
        (C::IntTrailingZeros { value: i }, T::Int), (C::IntReverseBits { value: i }, T::Int),
        (C::IntSwapBytes { value: i }, T::Int), (C::IntRotateLeft { value: i, count: j }, T::Int),
        (C::IntRotateRight { value: i, count: j }, T::Int),
        (C::ByteAnd { left: b, right: c }, T::Byte), (C::ByteOr { left: b, right: c }, T::Byte),
        (C::ByteXor { left: b, right: c }, T::Byte), (C::ByteShl { left: b, right: i }, T::Byte),
        (C::ByteShr { left: b, right: i }, T::Byte), (C::ByteUshr { left: b, right: c }, T::Byte),
        (C::ByteNot { value: b }, T::Byte), (C::ByteBitCount { value: b }, T::Int),
        (C::ByteLeadingZeros { value: b }, T::Int), (C::ByteTrailingZeros { value: b }, T::Int),
        (C::ByteReverseBits { value: b }, T::Byte), (C::ByteSwapBytes { value: b }, T::Byte),
        (C::ByteRotateLeft { value: b, count: i }, T::Byte), (C::ByteRotateRight { value: b, count: i }, T::Byte),
        (C::FloatAdd { left: x, right: y }, T::Float), (C::FloatSub { left: x, right: y }, T::Float),
        (C::FloatMul { left: x, right: y }, T::Float), (C::FloatDiv { left: x, right: y }, T::Float),
        (C::FloatMod { left: x, right: y }, T::Float), (C::FloatPow { left: x, right: y }, T::Float),
        (C::FloatNeg { value: x }, T::Float), (C::FloatAbs { value: x }, T::Float),
        (C::FloatSign { value: x }, T::Int), (C::FloatSqrt { value: x }, T::Float),
        (C::RatAdd { left: r, right: q }, T::Rat), (C::RatSub { left: r, right: q }, T::Rat),
        (C::RatMul { left: r, right: q }, T::Rat), (C::RatDiv { left: r, right: q }, T::Rat),
        (C::RatMod { left: r, right: q }, T::Rat), (C::RatNeg { value: r }, T::Rat), (C::RatAbs { value: r }, T::Rat),
        (C::RatSign { value: r }, T::Int), (C::RatNumerator { value: r }, T::Int),
        (C::RatDenominator { value: r }, T::Int),
        (C::StrSlice { value: s, start: i, end: j }, T::Str), (C::StrSplit { value: s, separator: t }, str_array),
        (C::StrReplace { value: s, target: t, replacement: u }, T::Str),
        (C::StrReplaceFirst { value: s, target: t, replacement: u }, T::Str),
        (C::StrRepeat { value: s, count: i }, T::Str), (C::StrTrim { value: s }, T::Str),
        (C::StrChomp { value: s }, T::Str), (C::StrTrimStart { value: s }, T::Str),
        (C::StrTrimEnd { value: s }, T::Str), (C::StrLower { value: s }, T::Str), (C::StrUpper { value: s }, T::Str),
        (C::StrFind { value: s, part: t }, T::Int), (C::StrFindLast { value: s, part: t }, T::Int),
        (C::StrStartsWith { value: s, part: t }, T::Bool), (C::StrEndsWith { value: s, part: t }, T::Bool),
        (C::StrUtf8 { value: s }, byte_array), (C::StrCharacters { value: s }, char_array),
    ];
    (m, ops)
}

#[test]
fn s1_scalar_families_translate_every_operation_with_its_signature() {
    let (mut m, ops) = family_module();
    let count = ops.len();
    for (op, ty) in &ops {
        let (data, inputs) = family_operation(op).unwrap_or_else(|| panic!("{op:?} has no SPB 17 operation"));
        let (operands, result) = data.scalar_signature().unwrap();
        assert_eq!(inputs.len(), operands.len(), "{op:?}");
        let id = m.types.intern(ty.clone()).unwrap();
        assert_eq!(translate_type(&m, id).unwrap(), result, "{op:?}: OIR-2 result {ty:?}");
        let value = append(&mut m, o::Operation::Core(op.clone()), Some(ty.clone())).unwrap();
        if matches!(result, s::Type::Array(_)) {
            // Own certifies a plain array result; the generator requires the certificate.
            let body = m.functions.get_mut(o::FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
            body.values.get_mut(value).unwrap().facts = o::ValueFacts::Plain {
                certificate: o::PlainCertificate::ClosedGraph { ty: id },
            };
        }
    }
    append(&mut m, o::Operation::Const(o::Literal::Rat { numerator: "-3".into(), denominator: "4".into() }),
        Some(o::Type::Rat));
    append(&mut m, o::Operation::Const(o::Literal::Int("1267650600228229401496703205376".into())),
        Some(o::Type::Int));
    let p = generate(&m, &layouts()).unwrap();
    assert_eq!(s::decode(&s::encode(&p).unwrap()).unwrap(), p);
    assert_eq!(s::assemble(&s::disassemble(&p).unwrap()).unwrap(), p);
    let code = &p.functions[0].code;
    for ((op, _), instruction) in ops.iter().zip(code) {
        let s::Instruction::Data { op: data, .. } = instruction else { panic!("{op:?}: {instruction:?}") };
        assert_eq!(Some(*data), family_operation(op).map(|(data, _)| data));
    }
    assert!(matches!(&code[count], s::Instruction::Data { immediate: s::Immediate::Constant(s::Constant::Rat(
        s::RatValue { numerator, denominator })), .. } if numerator == "-3" && denominator == "4"));
    assert!(matches!(&code[count + 1], s::Instruction::Data { immediate: s::Immediate::Constant(s::Constant::Int(
        value)), .. } if value == "1267650600228229401496703205376"));
}

#[test]
fn scalar_family_failures_match_their_spb_operations() {
    let (_, ops) = family_module();
    for (op, _) in ops {
        let operation = o::Operation::Core(op.clone());
        let (data, _) = family_operation(&op).unwrap();
        assert_eq!(calls::fallible_scalar(&operation), data.fallible(&[], None), "{op:?}");
        assert_eq!(total_family_operation(&operation), !data.fallible(&[], None), "{op:?}");
    }
    let value = o::ValueId::from_u32(0);
    let add = o::Operation::Core(o::CoreOp::IntAdd { left: value, right: value });
    assert!(!total_family_operation(&add) && family_operation(&o::CoreOp::IntAdd { left: value, right: value })
        .is_none());
    // Operations that OIR-2 lets trap but that never fail in Tomato are total in SPB 17.
    for op in [o::CoreOp::ByteAnd { left: value, right: value }, o::CoreOp::FloatNeg { value },
        o::CoreOp::FloatAbs { value }, o::CoreOp::FloatSign { value }]
    {
        assert!(total_family_operation(&o::Operation::Core(op)));
    }
}

/// A constructor's unbound field travels as undef through a branch before it is bound (#677).
const UNDEF_EDGES: &str = "oir 2 low
module undef_edges
object Box {
    field value: int
}
scope S action in @make
func @make(%flag: bool, %v: int) -> Box abi observation {
^entry in S:
    %u: int = undef ty int
    cond %flag, ^yes(%u), ^no(%u)
^yes(%a: int) in S:
    br ^join(%a, %v)
^no(%b: int) in S:
    br ^join(%b, %v)
^join(%p: int, %w: int) in S:
    %box: Box facts(plain(certificate=closed_graph(ty=Box))) = obj.new Box { value: %p } construction incomplete
    obj.init %box, %w, field field_ref(declaration=Box, index=0)
    ret %box
}
";

#[test]
fn s1_undef_only_parameters_become_block_local_undef() {
    let m = parse(UNDEF_EDGES);
    let p = generate(&m, &layouts()).unwrap_or_else(|error| panic!("{error}"));
    let f = &p.functions[0];
    let undef: BTreeSet<_> = f.code.iter().filter_map(|instruction| match instruction {
        s::Instruction::Data { op: s::DataOp::Undef, output: Some(id), .. } => Some(s::Location::Register(*id)),
        _ => None,
    }).collect();
    // Only the real value crosses an edge; the join defines its own undef for the construction.
    for instruction in &f.code {
        if let s::Instruction::Br(edge) | s::Instruction::Cond { yes: edge, .. } = instruction {
            assert!(edge.assignments.iter().all(|assignment| {
                !matches!(assignment.source, s::Source::Location(location) if undef.contains(&location))
            }), "{instruction:?}");
        }
    }
    assert_eq!(undef.len(), 4, "entry, both branches and the join keep their own undef");
    assert!(f.blocks.iter().all(|block| block.params.len() <= 2));
}

#[test]
fn s1_mixed_parameters_keep_their_edges() {
    // A parameter that receives a real value on one edge is an ordinary parameter.
    let text = UNDEF_EDGES.replace("br ^join(%a, %v)", "br ^join(%v, %v)");
    let p = generate(&parse(&text), &layouts()).expect_err("an undef edge source stays rejected");
    assert!(p.0.contains("undef"), "{}", p.0);
}

/// SPB 17 represents a one-case Error variant by its case object (`translate_type`), so `variant.tag` and
/// `variant.get` of such a value are a constant and a field read (#703). A multi-case Error variant, a plain
/// variant and other types are not.
#[test]
fn single_error_case_names_only_one_case_error_variants() {
    let module = parse(concat!(
        "oir 2 low\nmodule app v1.0.0\nrequires core\n\n",
        "variant app.One is Error = (at: app.One.at)\n\n",
        "object app.One.at {\n    field offset: int\n}\n\n",
        "variant app.Plain = (only: app.Plain.only)\n\n",
        "object app.Plain.only {\n    field value: int\n}\n\n",
        "variant app.Two is Error = (a: app.Two.a, b: app.Two.b)\n\n",
        "object app.Two.a {\n}\n\nobject app.Two.b {\n}\n\n",
        "scope S action in @probe\n",
        "func @probe(%a: app.One, %b: app.Plain, %c: app.Two) -> int abi observation {\n^entry in S:\n",
        "    %zero: int = const 0\n    ret %zero\n}\n",
    ));
    let named = |name: &str| module.types.iter().find(|(_, ty)| matches!(ty, o::Type::Variant(decl)
        if module.declarations.get(*decl).unwrap().name == name)).map(|(id, _)| id).unwrap();
    let layout = |name: &str| module.declarations.iter().find(|(_, d)| d.name == name).map(|(id, _)| id).unwrap();
    assert_eq!(single_error_case(&module, named("app.One")).unwrap(), Some(layout("app.One.at")));
    assert_eq!(translate_type(&module, named("app.One")).unwrap(), s::Type::Object(layout("app.One.at").as_u32()));
    assert_eq!(single_error_case(&module, named("app.Plain")).unwrap(), None);
    assert_eq!(single_error_case(&module, named("app.Two")).unwrap(), None);
    let int = module.types.iter().find(|(_, ty)| **ty == o::Type::Int).map(|(id, _)| id).unwrap();
    assert_eq!(single_error_case(&module, int).unwrap(), None);
}

/// A module constant's SPB 17 layout (#709): its init body as data, children first and each value once;
/// the global read becomes `global.get` of the constant's index. An init that computes anything else is
/// not a constant SPB 17 lays out.
#[test]
fn globals_lay_out_init_data_children_first() {
    let text = |extra: &str| format!(concat!(
        "oir 2 low\nmodule app v1.0.0\nrequires core\n\n",
        "global app.const.EMPTY: array<int> = @app.const.EMPTY.init\n",
        "global app.const.NESTED: array<array<int>> = @app.const.NESTED.init\n\n",
        "scope S0 action in @app.const.EMPTY.init\nscope S1 action in @app.const.NESTED.init\n",
        "func @app.const.EMPTY.init() -> array<int> abi observation {{\n^entry in S0:\n",
        "    %e: array<int> = array.new [], ty array<int>\n    ret %e\n}}\n",
        "func @app.const.NESTED.init() -> array<array<int>> abi observation {{\n^entry in S1:\n",
        "    %two: int = const 2\n    %inner: array<int> = array.new [%two, %two], ty array<int>\n{}",
        "    %outer: array<array<int>> = array.new [%inner, %inner], ty array<array<int>>\n    ret %outer\n}}\n"),
        extra);
    let module = parse(&text(""));
    let ints = s::Type::Array(s::ArrayType { leaf: s::ArrayLeaf::Int, depth: 1 });
    let nested = s::Type::Array(s::ArrayType { leaf: s::ArrayLeaf::Int, depth: 2 });
    assert_eq!(globals(&module).unwrap(), vec![
        s::Global { name: "app.const.EMPTY".into(), ty: ints, data: vec![
            s::Datum { ty: ints, value: s::DatumValue::Array(vec![]) },
        ] },
        s::Global { name: "app.const.NESTED".into(), ty: nested, data: vec![
            s::Datum { ty: s::Type::Int, value: s::DatumValue::Constant(s::Constant::Int("2".into())) },
            s::Datum { ty: ints, value: s::DatumValue::Array(vec![0, 0]) },
            s::Datum { ty: nested, value: s::DatumValue::Array(vec![1, 1]) },
        ] },
    ]);
    let nested_decl = module.declarations.iter().find(|(_, d)| d.name == "app.const.NESTED").unwrap().0;
    assert_eq!(global_index(&module, nested_decl).unwrap(), 1);
    let computed = "    %sum: int = int.add %two, %two\n    %more: array<int> = array.new [%sum], ty array<int>\n";
    let module = parse(&text(computed)
        .replace("array.new [%inner, %inner]", "array.new [%inner, %more]"));
    assert!(globals(&module).unwrap_err().0.contains("int.add"));
}
