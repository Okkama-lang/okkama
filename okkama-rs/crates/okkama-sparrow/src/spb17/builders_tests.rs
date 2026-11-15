//! Unique builders in the load gate (`BUILDER_LINEAR`): created by `builder.new` or a transition outcome,
//! consumed exactly once on every path, never read by anything but their own transition.
use super::*;
use super::load_tests::refresh;

const INTS: Type = Type::Array(ArrayType { leaf: ArrayLeaf::Int, depth: 1 });

fn reg(id: u32) -> Argument {
    Argument::Location(Location::Register(id))
}

fn svc(id: u32) -> Argument {
    Argument::Location(Location::Service(id))
}

fn produced(destination: Location, slot: u32) -> Assignment {
    Assignment { destination, source: Source::ProducedSlot(slot) }
}

fn abandon(builder: u32, packet: u32, next: u32) -> Instruction {
    Instruction::Champ { op: Opcode::BuilderAbandon, arguments: vec![reg(builder), svc(packet)],
        flow: Flow::Next(vec![Location::Service(next)]) }
}

fn ret(packet: u32) -> Instruction {
    Instruction::Champ { op: Opcode::RetOwn, arguments: vec![svc(packet)], flow: Flow::Next(vec![]) }
}

/// `b = builder.new; b.push(1)`: the normal outcome continues the builder in r2 and abandons it, the unwind
/// outcome continues it in r4 and abandons it too.
fn push_and_abandon() -> Program {
    use Location::{Register as R, Service as S};
    let mut f = tests::empty_function();
    f.symbol = "g".into();
    f.entry_abi = EntryAbi::Owned;
    f.proofs.entry = Some(EntryContract::default());
    f.registers = vec![Type::Builder(0), Type::Int, Type::Builder(0), Type::Builder(0), Type::Builder(0),
        Type::Failure];
    f.services = vec![ServiceKind::Packet; 6];
    f.params = vec![S(0)];
    f.code = vec![
        Instruction::Data { op: DataOp::BuilderNew, inputs: vec![], output: Some(0), immediate: Immediate::None },
        Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(1),
            immediate: Immediate::Constant(Constant::Int("1".into())) },
        Instruction::Champ { op: Opcode::BuilderPush, arguments: vec![reg(0), reg(1), svc(0)],
            flow: Flow::Outcomes(vec![
                Outcome { tag: Tag::Normal, edge: Edge { target_pc: 3,
                    assignments: vec![produced(R(2), 0), produced(S(1), 1)] } },
                Outcome { tag: Tag::Unwind, edge: Edge { target_pc: 6,
                    assignments: vec![produced(R(4), 0), produced(S(4), 1), produced(R(5), 2)] } },
            ]) },
        abandon(2, 1, 2),
        Instruction::Champ { op: Opcode::OwnMove, arguments: vec![svc(2)], flow: Flow::Next(vec![S(3)]) },
        ret(3),
        abandon(4, 4, 5),
        ret(5),
    ];
    f.blocks = vec![
        Block { pc: 0, params: vec![S(0)] },
        Block { pc: 3, params: vec![R(2), S(1)] },
        Block { pc: 6, params: vec![R(4), S(4), R(5)] },
    ];
    refresh(&mut f);
    Program { functions: vec![f], composites: vec![Composite::Builder { array: INTS }], ..Program::default() }
}

fn reject(p: &Program, message: &str) {
    let failure = verify_load(p, &NoExternalContracts).unwrap_err();
    assert_eq!(failure.rule, "BUILDER_LINEAR", "{failure}");
    assert!(failure.message.contains(message), "{failure}");
}

fn push_outcomes(p: &mut Program) -> &mut Vec<Outcome> {
    let Instruction::Champ { flow: Flow::Outcomes(outcomes), .. } = &mut p.functions[0].code[2] else {
        unreachable!("builder.push")
    };
    outcomes
}

#[test]
fn a_pushed_builder_continues_and_is_abandoned_on_both_outcomes() {
    verify_load(&push_and_abandon(), &NoExternalContracts).unwrap();
}

/// The audit's duplication: the normal outcome assigns its produced builder to two parameters, and each copy
/// is abandoned. The builder an outcome produces continues in exactly one location (#692).
#[test]
fn an_outcome_cannot_duplicate_its_produced_builder() {
    let mut p = push_and_abandon();
    push_outcomes(&mut p)[0].edge.assignments.insert(1, produced(Location::Register(3), 0));
    let f = &mut p.functions[0];
    f.blocks[1].params.insert(1, Location::Register(3));
    f.code[4] = abandon(3, 2, 3);
    refresh(f);
    reject(&p, "an outcome duplicates its produced builder");
}

/// The unwind outcome drops the builder it produces: nothing abandons it (#692).
#[test]
fn an_outcome_cannot_drop_its_produced_builder() {
    let mut p = push_and_abandon();
    push_outcomes(&mut p)[1].edge.assignments.remove(0);
    let f = &mut p.functions[0];
    f.blocks[2].params.remove(0);
    f.code[6] = Instruction::Champ { op: Opcode::OwnMove, arguments: vec![svc(4)],
        flow: Flow::Next(vec![Location::Service(5)]) };
    refresh(f);
    reject(&p, "an outcome drops its produced builder");
}

/// An observation entry that takes a builder as its parameter, or returns the one it creates.
#[test]
fn a_builder_cannot_cross_an_entry() {
    let mut f = tests::empty_function();
    f.registers = vec![Type::Builder(0)];
    f.params = vec![Location::Register(0)];
    f.blocks[0].params = f.params.clone();
    refresh(&mut f);
    let p = Program { functions: vec![f], composites: vec![Composite::Builder { array: INTS }],
        ..Program::default() };
    reject(&p, "a builder cannot cross an entry");
    let mut f = tests::empty_function();
    f.registers = vec![Type::Builder(0)];
    f.results = vec![Type::Builder(0)];
    f.code = vec![
        Instruction::Data { op: DataOp::BuilderNew, inputs: vec![], output: Some(0), immediate: Immediate::None },
        Instruction::Ret(vec![0]),
    ];
    refresh(&mut f);
    let p = Program { functions: vec![f], composites: vec![Composite::Builder { array: INTS }],
        ..Program::default() };
    reject(&p, "a builder cannot cross an entry");
}

#[test]
fn object_storage_cannot_hold_a_builder() {
    let mut p = push_and_abandon();
    p.objects.push(Object {
        name: "Holder".into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle: false,
        fields: vec![Field { name: "items".into(), storage: 0, once: false }],
        layout: ObjectRuntimeLayout {
            allocation_words: 2, storage_fields: vec![Type::Builder(0)], identity_offset: None,
            has_destroy: false, may_reach_resource: false, may_async_destroy: false, trace_layout: vec![0],
            selection_layout: vec![],
        },
    });
    reject(&p, "object storage cannot hold a unique builder");
}

/// The normal path returns without abandoning the builder it holds.
#[test]
fn a_builder_cannot_be_live_at_a_return() {
    let mut p = push_and_abandon();
    let f = &mut p.functions[0];
    f.code[3] = Instruction::Champ { op: Opcode::OwnMove, arguments: vec![svc(1)],
        flow: Flow::Next(vec![Location::Service(2)]) };
    refresh(f);
    reject(&p, "a live builder is neither finished nor abandoned");
}

#[test]
fn a_builder_is_read_only_by_its_own_transition() {
    let mut p = push_and_abandon();
    let f = &mut p.functions[0];
    f.registers.push(Type::Builder(0));
    f.code[1] = Instruction::Data { op: DataOp::Move, inputs: vec![0], output: Some(6), immediate: Immediate::None };
    f.code.insert(2, Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(1),
        immediate: Immediate::Constant(Constant::Int("1".into())) });
    for block in &mut f.blocks[1..] {
        block.pc += 1;
    }
    let Instruction::Champ { flow: Flow::Outcomes(outcomes), .. } = &mut f.code[3] else { unreachable!() };
    for outcome in outcomes {
        outcome.edge.target_pc += 1;
    }
    refresh(f);
    reject(&p, "a builder is read outside its own transition");
}

/// Two paths join with different live builders: one created a second builder the other did not.
#[test]
fn live_builders_agree_at_a_join() {
    use Location::Service as S;
    let join = |second: bool| {
        let mut f = tests::empty_function();
        f.symbol = "g".into();
        f.entry_abi = EntryAbi::Owned;
        f.proofs.entry = Some(EntryContract::default());
        f.registers = vec![Type::Builder(0), Type::Bool, Type::Builder(0), Type::Int];
        f.services = vec![ServiceKind::Packet; 2];
        f.params = vec![S(0)];
        let created = if second {
            Instruction::Data { op: DataOp::BuilderNew, inputs: vec![], output: Some(2), immediate: Immediate::None }
        } else {
            Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(3),
                immediate: Immediate::Constant(Constant::Int("0".into())) }
        };
        f.code = vec![
            Instruction::Data { op: DataOp::BuilderNew, inputs: vec![], output: Some(0), immediate: Immediate::None },
            Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(1),
                immediate: Immediate::Constant(Constant::Bool(true)) },
            Instruction::Cond { condition: 1, yes: Edge { target_pc: 3, assignments: vec![] },
                no: Edge { target_pc: 5, assignments: vec![] } },
            created,
            Instruction::Br(Edge { target_pc: 6, assignments: vec![] }),
            Instruction::Br(Edge { target_pc: 6, assignments: vec![] }),
            abandon(0, 0, 1),
            ret(1),
        ];
        f.blocks = vec![
            Block { pc: 0, params: vec![S(0)] },
            Block { pc: 3, params: vec![] },
            Block { pc: 5, params: vec![] },
            Block { pc: 6, params: vec![] },
        ];
        refresh(&mut f);
        Program { functions: vec![f], composites: vec![Composite::Builder { array: INTS }], ..Program::default() }
    };
    verify_load(&join(false), &NoExternalContracts).unwrap();
    reject(&join(true), "inconsistent live builders at a CFG join");
}
