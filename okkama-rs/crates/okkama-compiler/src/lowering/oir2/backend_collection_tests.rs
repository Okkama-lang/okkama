//! Builders, maps and collection operations through SPB 17 generation and the load gate (#622).
use super::compile_p1_source;
use okkama_sparrow::spb17 as s;

fn program(text: &str) -> s::Program {
    let bytes = compile_p1_source(text).unwrap_or_else(|error| panic!("{error}\n{text}"));
    s::decode(&bytes).unwrap()
}

fn load(program: &s::Program) -> Result<(), s::Error> {
    let contracts = s::p1::P1Contracts::new(&program.objects).unwrap();
    s::verify_load(program, &contracts)
}

/// (function, pc) of every Champ operation `op`.
fn sites(program: &s::Program, op: s::Opcode) -> Vec<(usize, usize)> {
    program.functions.iter().enumerate().flat_map(|(index, function)| {
        function.code.iter().enumerate().filter_map(move |(pc, instruction)| {
            matches!(instruction, s::Instruction::Champ { op: found, .. } if *found == op).then_some((index, pc))
        })
    }).collect()
}

fn locations(instruction: &s::Instruction) -> Vec<s::Location> {
    let s::Instruction::Champ { arguments, .. } = instruction else { return vec![] };
    arguments.iter().filter_map(|argument| match argument {
        s::Argument::Location(location) => Some(*location),
        _ => None,
    }).collect()
}

const BUILDER: &str = concat!(
    "module app; main() is int { let a is @int = [0, 2..=4, 9]; ",
    "let b = a.map() -> v { v * 2; }; a.size() + b.size(); }",
);

#[test]
fn oir2_backend_builder_threads_its_journal_through_every_transition() {
    let program = program(BUILDER);
    load(&program).unwrap();
    assert!(program.composites.iter().any(|composite| matches!(composite, s::Composite::Builder { .. })));
    for op in [s::Opcode::BuilderPush, s::Opcode::BuilderRange, s::Opcode::BuilderFinish] {
        let found = sites(&program, op);
        assert!(!found.is_empty(), "{op:?}");
        for (function, pc) in found {
            let function = &program.functions[function];
            let last = *locations(&function.code[pc]).last().unwrap();
            let s::Location::Service(service) = last else { panic!("{op:?} journal is not a service") };
            assert_eq!(function.services[service as usize], s::ServiceKind::Packet);
        }
    }
}

#[test]
fn oir2_backend_builder_cannot_be_used_after_its_transition() {
    let mut program = program(BUILDER);
    let (function, push) = sites(&program, s::Opcode::BuilderPush)[0];
    let consumed = locations(&program.functions[function].code[push])[0];
    let (_, finish) = sites(&program, s::Opcode::BuilderFinish).into_iter()
        .find(|&(index, pc)| index == function && pc > push).unwrap();
    let s::Instruction::Champ { arguments, .. } = &mut program.functions[function].code[finish] else {
        unreachable!()
    };
    arguments[0] = s::Argument::Location(consumed);
    assert_eq!(load(&program).unwrap_err().rule, "BUILDER_LINEAR");
}

#[test]
fn oir2_backend_builder_operands_follow_the_buffer_type() {
    let mut program = program(BUILDER);
    let (function, push) = sites(&program, s::Opcode::BuilderPush)[0];
    let s::Instruction::Champ { arguments, .. } = &mut program.functions[function].code[push] else {
        unreachable!()
    };
    // The journal packet must close the operand list.
    arguments.swap(1, 2);
    assert_eq!(load(&program).unwrap_err().rule, "BUILDER_OPERANDS");
}

#[test]
fn oir2_backend_maps_and_lookups_load() {
    let program = program(concat!(
        "module app; main() is int { let m is %string:int = [\"a\": 1, \"b\": 2]; ",
        "let z = m + [\"c\": 3]; let r = m[\"q\"] else 5; let xs = [1, 2]; let past = xs[7] else 3; ",
        "if m.has(\"a\") { m[\"a\"] + z.size() + r + past + m.keys().size(); } else { 0; } }",
    ));
    load(&program).unwrap();
    assert_eq!(sites(&program, s::Opcode::Lookup).len(), 2);
    assert!(program.composites.contains(&s::Composite::Map { key: s::Type::Str, value: s::Type::Int }));
}

#[test]
fn oir2_backend_checked_array_operations_have_outcomes() {
    let program = program(concat!(
        "module app; main() is int { let a = [1, 2, 3]; let c = a.removed_at(0); ",
        "let d = c.replaced(0, 7); d.slice(1, 2).size() + ([d.size().string(), \"x\"].join(\",\")).length(); }",
    ));
    load(&program).unwrap();
    let evaluated = program.functions.iter().flat_map(|function| &function.code).filter_map(|instruction| {
        match instruction {
            s::Instruction::Champ { op: s::Opcode::Evaluate, arguments, .. } => match arguments.first() {
                Some(s::Argument::Evaluation(evaluation)) => Some(evaluation.op),
                _ => None,
            },
            _ => None,
        }
    }).collect::<Vec<_>>();
    for op in [s::DataOp::ArrayRemove, s::DataOp::ArrayReplace, s::DataOp::ArraySlice] {
        assert!(evaluated.contains(&op), "{op:?}: {evaluated:?}");
    }
}

/// A function that owns a resource is not resource-free: the journal's inventory comes from its packet.
#[test]
fn oir2_backend_builder_beside_a_resource_loads() {
    let program = program(concat!(
        "module app; object Res { has id is int; Res(:id); destroy { } } ",
        "main() is int { let r = Res(id: 1); let a = (0..3).map() -> i { i + 1; }; a.size(); }",
    ));
    load(&program).unwrap();
    assert!(!sites(&program, s::Opcode::BuilderFinish).is_empty());
}
