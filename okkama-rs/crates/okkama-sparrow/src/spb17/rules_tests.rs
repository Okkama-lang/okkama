//! Positive and negative tests of the load gate's rules (#654). Each negative test reaches its rule with a
//! program that is valid up to that rule; the matching positive test shows the admissible form loads.
use super::*;
use super::load_tests::{Catalog, bound_fresh_return, fixture, main, refresh, reject, small};
use super::tests::empty_function;

/// Rules without a stable code of their own report SPB_STRUCTURE; the message names the rule.
fn reject_message(p: &Program, catalog: &dyn ExternalContracts, code: &'static str, message: &str) {
    let failure = verify_load(p, catalog).unwrap_err();
    assert_eq!(failure.rule, code, "{failure}");
    assert!(failure.message.contains(message), "{failure}");
}

fn function_named<'a>(p: &'a mut Program, symbol: &str) -> &'a mut Function {
    p.functions.iter_mut().find(|f| f.symbol == symbol).unwrap()
}

fn locations(instruction: &mut Instruction) -> Vec<&mut Argument> {
    let Instruction::Champ { arguments, .. } = instruction else { panic!("not a champ instruction") };
    arguments.iter_mut().filter(|argument| matches!(argument, Argument::Location(_))).collect()
}

pub(super) fn plain_object(error_cases: Vec<String>) -> Object {
    Object {
        name: "Plain".into(),
        fields: vec![],
        enum_cases: 0,
        variant_cases: vec![],
        handle: false,
        role: None,
        implements: vec![],
        error_cases,
        layout: ObjectRuntimeLayout {
            storage_fields: vec![Type::Int],
            allocation_words: 2,
            identity_offset: None,
            has_destroy: false,
            may_reach_resource: false,
            may_async_destroy: false,
            trace_layout: vec![],
            selection_layout: vec![],
        },
    }
}

/// A function whose first `params` registers are its parameters.
fn scalar(params: u32, registers: Vec<Type>, results: Vec<Type>, code: Vec<Instruction>) -> Function {
    let mut f = empty_function();
    f.registers = registers;
    f.params = (0..params).map(Location::Register).collect();
    f.blocks[0].params = f.params.clone();
    f.results = results;
    f.code = code;
    f.proofs.entry = Some(EntryContract::default());
    refresh(&mut f);
    f
}

fn data(op: DataOp, inputs: Vec<u32>, output: u32) -> Instruction {
    Instruction::Data { op, inputs, output: Some(output), immediate: Immediate::None }
}

// ---- CALL_PLAIN_PACKET ----

#[test]
fn plain_call_loads_with_an_empty_packet() {
    let (p, catalog) = fixture("fresh-call");
    let f = p.functions.iter().find(|f| f.symbol == "main").unwrap();
    assert!(matches!(f.code[8], Instruction::Champ { op: Opcode::OwnEmpty, .. }), "fixture changed");
    verify_load(&p, &catalog).unwrap();
}

#[test]
fn plain_call_cannot_pass_a_packet_with_obligations() {
    // main admits the empty packet into its scope and passes the packet holding its created object instead.
    let (mut p, catalog) = fixture("fresh-call");
    let f = main(&mut p);
    assert!(matches!(f.code[7], Instruction::Champ { op: Opcode::OwnAdmit, .. }), "fixture changed");
    assert!(matches!(f.code[9], Instruction::Champ { op: Opcode::InvokeCallOwn, .. }), "fixture changed");
    f.code.swap(7, 8);
    *locations(&mut f.code[8])[1] = Argument::Location(Location::Service(5));
    *locations(&mut f.code[9])[0] = Argument::Location(Location::Service(2));
    refresh(f);
    reject(&p, &catalog, "CALL_PLAIN_PACKET");
}

// ---- RETURN_PLAIN_PACKET ----

/// fresh-call's callee with no results: its packet split selects `selection` and ret.own returns the
/// selected packet. The caller is replaced by an empty function so the callee's schema can change.
fn plain_return(selection: u32) -> (Program, Catalog) {
    let (mut p, mut catalog) = fixture("fresh-call");
    *main(&mut p) = empty_function();
    // With nothing selected the object stays in the scope, whose close has an empty failure envelope:
    // its destroy must not fail.
    p.functions[0].proofs.entry.as_mut().unwrap().failures = 0;
    catalog.0[0].proofs.entry.as_mut().unwrap().failures = 0;
    let index = p.functions.iter().position(|f| f.symbol == "fresh").unwrap();
    let f = &mut p.functions[index];
    assert!(matches!(f.code[11], Instruction::Champ { op: Opcode::OwnSplit, .. }), "fixture changed");
    assert!(matches!(f.code[32], Instruction::Champ { op: Opcode::RetOwn, .. }), "fixture changed");
    f.results.clear();
    f.proofs.entry.as_mut().unwrap().fresh_singleton = false;
    let Instruction::Champ { arguments, .. } = &mut f.code[11] else { unreachable!() };
    arguments[1] = Argument::Selection(selection);
    let Instruction::Champ { arguments, .. } = &mut f.code[32] else { unreachable!() };
    arguments.remove(0);
    refresh(f);
    let selections = evidence::static_selections(&p, &p.functions[index]).unwrap();
    p.functions[index].proofs.static_selections = selections;
    (p, catalog)
}

#[test]
fn plain_return_loads_with_an_empty_packet() {
    let (p, catalog) = plain_return(0);
    verify_load(&p, &catalog).unwrap();
}

#[test]
fn plain_return_cannot_carry_an_obligation() {
    let (p, catalog) = plain_return(3);
    reject(&p, &catalog, "RETURN_PLAIN_PACKET");
}

// ---- PENDING_COMMIT_TYPE: exit.commit with a local result ----

/// valued's return exit rewritten as a local (Next) commit with an explicit result `result`. With `value`
/// the pending carries the parameter; ret.own returns the parameter itself.
fn local_commit(value: bool, result: Option<Type>) -> (Program, Catalog) {
    let (mut p, catalog) = fixture("valued");
    let f = main(&mut p);
    assert!(matches!(f.code[3], Instruction::Champ { op: Opcode::ExitPrepare, .. }), "fixture changed");
    assert!(matches!(f.code[8], Instruction::Champ { op: Opcode::ExitCommit, .. }), "fixture changed");
    let Instruction::Champ { arguments, .. } = &mut f.code[3] else { unreachable!() };
    arguments[0] = Argument::Kind(ExitKind::Next);
    if !value {
        arguments.remove(2);
    }
    let Instruction::Champ { arguments, flow, .. } = &mut f.code[8] else { unreachable!() };
    arguments.push(Argument::Result(result));
    let mut outputs = vec![Location::Service(12), Location::Service(13)];
    if result.is_some() {
        outputs.insert(0, Location::Register(1));
    }
    *flow = Flow::Next(outputs);
    let Instruction::Champ { arguments, .. } = &mut f.code[9] else { unreachable!() };
    arguments[0] = Argument::Location(Location::Register(0));
    refresh(f);
    (p, catalog)
}

#[test]
fn local_commit_loads_when_its_result_matches_the_pending() {
    let (p, catalog) = local_commit(false, None);
    verify_load(&p, &catalog).unwrap();
    let (p, catalog) = local_commit(true, Some(Type::Object(0)));
    verify_load(&p, &catalog).unwrap();
}

#[test]
fn local_commit_cannot_drop_the_pending_value() {
    let (p, catalog) = local_commit(true, None);
    reject(&p, &catalog, "PENDING_COMMIT_TYPE");
}

#[test]
fn local_commit_cannot_invent_a_pending_value() {
    let (p, catalog) = local_commit(false, Some(Type::Object(0)));
    reject(&p, &catalog, "PENDING_COMMIT_TYPE");
}

// ---- FALLIBLE_DATA_OUTCOMES ----

fn fallible(op: DataOp, services: Vec<ServiceKind>) -> Function {
    let (mut registers, inputs, immediate) = match op {
        DataOp::RangeNew => (vec![Type::Int, Type::Int, Type::Range], vec![0, 1], Immediate::Inclusive(false)),
        // int -> byte fails out of range (`conversion` is Some(true) for it).
        DataOp::Convert => (vec![Type::Int, Type::Byte], vec![0], Immediate::None),
        _ => panic!("not a fallible data operation"),
    };
    let output = registers.len() as u32 - 1;
    let mut f = empty_function();
    f.params = (0..inputs.len() as u32).map(Location::Register).collect();
    f.params.extend((0..services.len() as u32).map(Location::Service));
    f.registers.append(&mut registers);
    f.services = services;
    f.blocks[0].params = f.params.clone();
    f.proofs.entry = Some(EntryContract { failures: 2, ..EntryContract::default() });
    f.code.insert(0, Instruction::Data { op, inputs, output: Some(output), immediate });
    refresh(&mut f);
    f
}

#[test]
fn fallible_range_and_conversion_load_without_services() {
    for op in [DataOp::RangeNew, DataOp::Convert] {
        verify_load(&small(fallible(op, vec![])), &NoExternalContracts).unwrap();
    }
}

#[test]
fn fallible_range_and_conversion_need_outcomes_where_services_live() {
    for op in [DataOp::RangeNew, DataOp::Convert] {
        reject(&small(fallible(op, vec![ServiceKind::Deps])), &NoExternalContracts, "FALLIBLE_DATA_OUTCOMES");
    }
}

// ---- CATCH_TYPE ----

/// catch of a failure parameter into `object`: Some returns the caught error, None re-raises.
fn catching(error_cases: Vec<String>) -> Program {
    let mut f = scalar(1, vec![Type::Failure, Type::Object(0)], vec![Type::Object(0)], vec![
        Instruction::Champ { op: Opcode::Catch,
            arguments: vec![Argument::Location(Location::Register(0)), Argument::Object(0)],
            flow: Flow::Outcomes(vec![
                Outcome { tag: Tag::Some, edge: Edge { target_pc: 1, assignments: vec![Assignment {
                    destination: Location::Register(1), source: Source::ProducedSlot(0) }] } },
                Outcome { tag: Tag::None, edge: Edge { target_pc: 2, assignments: vec![] } },
            ]) },
        Instruction::Ret(vec![1]),
        Instruction::Raise(0),
    ]);
    f.blocks.push(Block { pc: 1, params: vec![Location::Register(1)] });
    f.blocks.push(Block { pc: 2, params: vec![] });
    f.proofs.entry.as_mut().unwrap().failures = 15;
    refresh(&mut f);
    Program { objects: vec![plain_object(error_cases)], ..small(f) }
}

#[test]
fn catch_loads_into_a_declared_error_object() {
    verify_load(&catching(vec!["E.one".into()]), &NoExternalContracts).unwrap();
}

#[test]
fn catch_target_object_must_declare_error_cases() {
    reject(&catching(vec![]), &NoExternalContracts, "CATCH_TYPE");
}

// ---- typing of deps.empty, range.len, fail.caused and Move Object -> Error ----

fn deps_empty(output: ServiceKind) -> Program {
    let mut f = empty_function();
    f.services = vec![output];
    f.code.insert(0, Instruction::Champ { op: Opcode::DepsEmpty, arguments: vec![],
        flow: Flow::Next(vec![Location::Service(0)]) });
    small(f)
}

#[test]
fn deps_empty_produces_deps() {
    verify_load(&deps_empty(ServiceKind::Deps), &NoExternalContracts).unwrap();
    reject_message(&deps_empty(ServiceKind::Packet), &NoExternalContracts, "SPB_STRUCTURE", "bad multi-result type");
}

#[test]
fn range_len_reads_a_range_into_an_int() {
    let range_len = |input| small(scalar(1, vec![input, Type::Int], vec![], vec![
        data(DataOp::RangeLen, vec![0], 1),
        Instruction::Ret(vec![]),
    ]));
    verify_load(&range_len(Type::Range), &NoExternalContracts).unwrap();
    reject_message(&range_len(Type::Int), &NoExternalContracts, "SPB_STRUCTURE", "scalar operand/result type");
}

#[test]
fn fail_caused_joins_two_failures() {
    let caused = |cause| small(scalar(2, vec![Type::Failure, cause, Type::Failure], vec![], vec![
        data(DataOp::FailCaused, vec![0, 1], 2),
        Instruction::Ret(vec![]),
    ]));
    verify_load(&caused(Type::Failure), &NoExternalContracts).unwrap();
    reject_message(&caused(Type::Str), &NoExternalContracts, "SPB_STRUCTURE", "scalar operand/result type");
}

#[test]
fn move_widens_only_a_declared_error_object_to_error() {
    let widen = |error_cases| Program {
        objects: vec![plain_object(error_cases)],
        ..small(scalar(1, vec![Type::Object(0), Type::Error], vec![], vec![
            data(DataOp::Move, vec![0], 1),
            Instruction::Ret(vec![]),
        ]))
    };
    verify_load(&widen(vec!["E.one".into()]), &NoExternalContracts).unwrap();
    reject_message(&widen(vec![]), &NoExternalContracts, "SPB_STRUCTURE", "invalid move/succ.take type");
}

// ---- own.create_commit on a Successor ----

/// An owning function beside fixture valued's `Res { label: str }`: it creates and commits an object,
/// derives its successor, commits the successor as `object`, takes it and returns it with the joined packet.
fn successor_commit(object: u32) -> (Program, Catalog) {
    let (mut p, catalog) = fixture("valued");
    if object != 0 {
        let mut other = p.objects[0].clone();
        other.name = "Other".into();
        p.objects.push(other);
        let mut destroy = p.functions[0].clone();
        destroy.symbol = "destroy_other".into();
        destroy.registers = vec![Type::Object(1)];
        let entry = destroy.proofs.entry.as_mut().unwrap();
        entry.external = None;
        entry.destroy = Some(1);
        destroy.blocks = vec![Block { pc: 0, params: destroy.params.clone() }];
        destroy.code = vec![Instruction::Ret(vec![])];
        refresh(&mut destroy);
        p.functions.push(destroy);
    }
    let mut f = empty_function();
    f.symbol = "advance".into();
    f.entry_abi = EntryAbi::Owned;
    f.registers = vec![Type::Str, Type::Object(0), Type::Successor(0), Type::Object(0)];
    f.services = vec![ServiceKind::Packet; 4];
    f.services.push(ServiceKind::Deps);
    f.params = vec![Location::Register(0), Location::Service(0)];
    f.blocks[0].params = f.params.clone();
    f.results = vec![Type::Object(0)];
    f.proofs.entry = Some(EntryContract::default());
    let register = |id| Argument::Location(Location::Register(id));
    let service = |id| Argument::Location(Location::Service(id));
    let commit = |id, object, packet| Instruction::Champ { op: Opcode::OwnCreateCommit,
        arguments: vec![register(id), Argument::Object(object), Argument::Boundary(0)],
        flow: Flow::Next(vec![Location::Service(packet)]) };
    f.code = vec![
        Instruction::Data { op: DataOp::ObjNew, inputs: vec![0], output: Some(1), immediate: Immediate::Object(0) },
        commit(1, 0, 1),
        Instruction::Data { op: DataOp::ObjNext, inputs: vec![1, 0], output: Some(2),
            immediate: Immediate::Updates(vec![0]) },
        commit(2, object, 2),
        Instruction::Champ { op: Opcode::OwnJoin, arguments: vec![service(0), service(1), service(2)],
            flow: Flow::Next(vec![Location::Service(3)]) },
        data(DataOp::Move, vec![2], 3),
        Instruction::Champ { op: Opcode::DepsEmpty, arguments: vec![], flow: Flow::Next(vec![Location::Service(4)]) },
        Instruction::Champ { op: Opcode::RetOwn, arguments: vec![register(3), service(3), service(4)],
            flow: Flow::Next(vec![]) },
    ];
    refresh(&mut f);
    p.functions.push(f);
    (p, catalog)
}

#[test]
fn successor_commits_under_its_own_descriptor() {
    let (p, catalog) = successor_commit(0);
    verify_load(&p, &catalog).unwrap();
}

#[test]
fn successor_commit_cannot_name_another_descriptor() {
    let (p, catalog) = successor_commit(1);
    reject_message(&p, &catalog, "SPB_STRUCTURE", "create_commit identity/descriptor mismatch");
}

// ---- CREATION_UNCOMMITTED ----

/// An owning function beside fixture valued's `Res { label: str }`: it creates an object, runs `between`,
/// commits the object into a packet, joins it with its input packet and returns it. `after` runs after the
/// join and may consume the joined packet s2 into s5. Registers: r0 str, r1 the object, r2 the scratch.
fn committing(between: Vec<Instruction>, after: Vec<Instruction>) -> (Program, Catalog) {
    let (mut p, catalog) = fixture("valued");
    let mut f = empty_function();
    f.symbol = "create".into();
    f.entry_abi = EntryAbi::Owned;
    f.registers = vec![Type::Str, Type::Object(0), Type::Object(0), Type::Int];
    f.services = vec![ServiceKind::Packet; 6];
    f.services.push(ServiceKind::Deps);
    f.params = vec![Location::Register(0), Location::Service(0)];
    f.blocks[0].params = f.params.clone();
    f.results = vec![Type::Object(0)];
    f.proofs.entry = Some(EntryContract::default());
    let service = |id| Argument::Location(Location::Service(id));
    let returned = if after.is_empty() { 2 } else { 5 };
    f.code = vec![Instruction::Data { op: DataOp::ObjNew, inputs: vec![0], output: Some(1),
        immediate: Immediate::Object(0) }];
    f.code.extend(between);
    f.code.push(commit_into(1, 1));
    f.code.push(Instruction::Champ { op: Opcode::OwnJoin, arguments: vec![service(0), service(1)],
        flow: Flow::Next(vec![Location::Service(2)]) });
    f.code.extend(after);
    f.code.push(Instruction::Champ { op: Opcode::DepsEmpty, arguments: vec![],
        flow: Flow::Next(vec![Location::Service(6)]) });
    f.code.push(Instruction::Champ { op: Opcode::RetOwn,
        arguments: vec![Argument::Location(Location::Register(1)), service(returned), service(6)],
        flow: Flow::Next(vec![]) });
    for (pc, instruction) in f.code.iter().enumerate() {
        if let Instruction::Br(edge) = instruction {
            assert_eq!(edge.target_pc as usize, pc + 1, "only a branch to the next instruction is supported");
            f.blocks.push(Block { pc: edge.target_pc, params: vec![] });
        }
    }
    refresh(&mut f);
    p.functions.push(f);
    (p, catalog)
}

fn commit_into(register: u32, packet: u32) -> Instruction {
    Instruction::Champ { op: Opcode::OwnCreateCommit,
        arguments: vec![Argument::Location(Location::Register(register)), Argument::Object(0), Argument::Boundary(0)],
        flow: Flow::Next(vec![Location::Service(packet)]) }
}

#[test]
fn creation_commits_past_an_unrelated_instruction() {
    let constant = Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(3),
        immediate: Immediate::Constant(Constant::Int("7".into())) };
    let (p, catalog) = committing(vec![constant], vec![]);
    verify_load(&p, &catalog).unwrap();
}

#[test]
fn creation_cannot_be_moved_before_its_commit() {
    let (p, catalog) = committing(vec![data(DataOp::Move, vec![1], 2)], vec![]);
    reject(&p, &catalog, "CREATION_UNCOMMITTED");
}

#[test]
fn creation_cannot_be_initialized_before_its_commit() {
    let init = Instruction::Data { op: DataOp::ObjInit, inputs: vec![1, 0], output: None,
        immediate: Immediate::Field(0) };
    let (p, catalog) = committing(vec![init], vec![]);
    reject(&p, &catalog, "CREATION_UNCOMMITTED");
}

#[test]
fn creation_cannot_be_selected_before_its_commit() {
    let (mut p, catalog) = committing(vec![], vec![]);
    let f = function_named(&mut p, "create");
    // own.split selects the uncommitted object from the input packet: s0 -> (rest s3, selected s4, deps s5).
    f.proofs.selections.push(Selection { locations: vec![Location::Register(1)] });
    f.code.insert(1, Instruction::Champ { op: Opcode::OwnSplit,
        arguments: vec![Argument::Location(Location::Service(0)), Argument::Selection(0)],
        flow: Flow::Next(vec![Location::Service(3), Location::Service(4), Location::Service(6)]) });
    f.services[5] = ServiceKind::Deps;
    let Instruction::Champ { arguments, .. } = &mut f.code[3] else { panic!("join") };
    *arguments = vec![Argument::Location(Location::Service(3)), Argument::Location(Location::Service(4)),
        Argument::Location(Location::Service(1))];
    let Instruction::Champ { flow, .. } = &mut f.code[4] else { panic!("deps.empty") };
    *flow = Flow::Next(vec![Location::Service(5)]);
    let Instruction::Champ { arguments, .. } = &mut f.code[5] else { panic!("ret.own") };
    arguments[2] = Argument::Location(Location::Service(5));
    refresh(f);
    reject(&p, &catalog, "CREATION_UNCOMMITTED");
}

#[test]
fn creation_cannot_reach_its_commit_through_a_branch() {
    let (p, catalog) = committing(vec![Instruction::Br(Edge { target_pc: 2, assignments: vec![] })], vec![]);
    reject(&p, &catalog, "CREATION_UNCOMMITTED");
}

#[test]
fn creation_cannot_be_committed_twice() {
    let join = Instruction::Champ { op: Opcode::OwnJoin,
        arguments: vec![Argument::Location(Location::Service(2)), Argument::Location(Location::Service(3))],
        flow: Flow::Next(vec![Location::Service(5)]) };
    let (p, catalog) = committing(vec![], vec![commit_into(1, 3), join]);
    reject(&p, &catalog, "CREATION_UNCOMMITTED");
}

// ---- RETURN_PACKET_NOT_FLAT ----

#[test]
fn opaque_parameter_packet_cannot_be_returned_directly() {
    // An owning function beside valued's main returns its input packet, whose inventory is the caller's
    // opaque argument run: the object parameter reaches resources.
    let (mut p, catalog) = fixture("valued");
    let mut f = empty_function();
    f.symbol = "identity".into();
    f.entry_abi = EntryAbi::Owned;
    f.registers = vec![Type::Object(0)];
    f.services = vec![ServiceKind::Packet, ServiceKind::Deps];
    f.params = vec![Location::Register(0), Location::Service(0)];
    f.blocks[0].params = f.params.clone();
    f.results = vec![Type::Object(0)];
    f.proofs.entry = Some(EntryContract::default());
    f.code = vec![
        Instruction::Champ { op: Opcode::DepsEmpty, arguments: vec![], flow: Flow::Next(vec![Location::Service(1)]) },
        ret_own((Location::Register(0), Location::Service(0), Location::Service(1))),
    ];
    refresh(&mut f);
    p.functions.push(f);
    reject(&p, &catalog, "RETURN_PACKET_NOT_FLAT");
}

/// The operands of a ret.own: value, packet and deps.
type Returned = (Location, Location, Location);

/// bound_fresh_return's callee with its final ret.own replaced by a branch to appended `tail` code, which
/// receives the replaced ret.own's operands and the first appended PC.
fn bound_tail(commit: bool, tail: impl FnOnce(&mut Function, Returned, u32) -> Vec<Instruction>)
    -> (Program, Catalog)
{
    let (mut p, catalog) = bound_fresh_return(commit);
    let f = function_named(&mut p, "fresh");
    let returned = f.code.iter().rposition(|instruction| {
        matches!(instruction, Instruction::Champ { op: Opcode::RetOwn, .. })
    }).unwrap();
    let Instruction::Champ { arguments, .. } = &f.code[returned] else { unreachable!() };
    let operands = arguments.iter().map(|argument| match argument {
        Argument::Location(location) => *location, _ => unreachable!(),
    }).collect::<Vec<_>>();
    let start = f.code.len() as u32;
    f.code[returned] = Instruction::Br(Edge { target_pc: start, assignments: vec![] });
    f.blocks.push(Block { pc: start, params: vec![] });
    let code = tail(f, (operands[0], operands[1], operands[2]), start);
    f.code.extend(code);
    f.blocks.sort_by_key(|block| block.pc);
    refresh(f);
    (p, catalog)
}

fn new_service(f: &mut Function, kind: ServiceKind) -> Location {
    f.services.push(kind);
    Location::Service(f.services.len() as u32 - 1)
}

fn ret_own((value, packet, deps): Returned) -> Instruction {
    Instruction::Champ { op: Opcode::RetOwn,
        arguments: vec![Argument::Location(value), Argument::Location(packet), Argument::Location(deps)],
        flow: Flow::Next(vec![]) }
}

/// Returns own.join(empty, packet) instead of the packet itself.
fn join_empty(f: &mut Function, (value, packet, deps): Returned, _: u32) -> Vec<Instruction> {
    let empty = new_service(f, ServiceKind::Packet);
    let joined = new_service(f, ServiceKind::Packet);
    vec![
        Instruction::Champ { op: Opcode::OwnEmpty, arguments: vec![], flow: Flow::Next(vec![empty]) },
        Instruction::Champ { op: Opcode::OwnJoin,
            arguments: vec![Argument::Location(empty), Argument::Location(packet)], flow: Flow::Next(vec![joined]) },
        ret_own((value, joined, deps)),
    ]
}

#[test]
fn joining_an_empty_packet_with_a_committed_one_stays_flat() {
    let (p, catalog) = bound_tail(true, join_empty);
    verify_load(&p, &catalog).unwrap();
}

#[test]
fn joining_a_flat_packet_with_a_bound_one_is_not_flat() {
    let (p, catalog) = bound_tail(false, join_empty);
    reject(&p, &catalog, "RETURN_PACKET_NOT_FLAT");
}

fn new_register(f: &mut Function, ty: Type) -> Location {
    f.registers.push(ty);
    Location::Register(f.registers.len() as u32 - 1)
}

/// Branches on a constant: the yes edge flattens the returned packet through exit.prepare and exit.commit,
/// the no edge carries it unchanged. Both reach one ret.own through block parameters.
fn join_flattened(f: &mut Function, (value, packet, deps): Returned, start: u32) -> Vec<Instruction> {
    let flag = new_register(f, Type::Bool);
    let pending = new_service(f, ServiceKind::Pending);
    let committed = (new_register(f, Type::Object(0)), new_service(f, ServiceKind::Packet),
        new_service(f, ServiceKind::Deps));
    let joined = (new_register(f, Type::Object(0)), new_service(f, ServiceKind::Packet),
        new_service(f, ServiceKind::Deps));
    let into_join = |(value, packet, deps): Returned| vec![
        Assignment { destination: joined.0, source: Source::Location(value) },
        Assignment { destination: joined.1, source: Source::Location(packet) },
        Assignment { destination: joined.2, source: Source::Location(deps) },
    ];
    let Location::Register(flag_id) = flag else { unreachable!() };
    let join = start + 5;
    f.blocks.push(Block { pc: start + 2, params: vec![] });
    f.blocks.push(Block { pc: join, params: vec![joined.0, joined.1, joined.2] });
    vec![
        Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(flag_id),
            immediate: Immediate::Constant(Constant::Bool(true)) },
        Instruction::Cond { condition: flag_id, yes: Edge { target_pc: start + 2, assignments: vec![] },
            no: Edge { target_pc: join, assignments: into_join((value, packet, deps)) } },
        Instruction::Champ { op: Opcode::ExitPrepare,
            arguments: vec![Argument::Kind(ExitKind::Return), Argument::Boundary(2), Argument::Location(value),
                Argument::Location(packet), Argument::Selection(3)],
            flow: Flow::Next(vec![pending]) },
        Instruction::Champ { op: Opcode::ExitCommit,
            arguments: vec![Argument::Location(pending), Argument::Destination(ExitDestination::Caller)],
            flow: Flow::Next(vec![committed.0, committed.1, committed.2]) },
        Instruction::Br(Edge { target_pc: join, assignments: into_join(committed) }),
        ret_own(joined),
    ]
}

#[test]
fn block_parameter_join_of_flat_packets_stays_flat() {
    let (p, catalog) = bound_tail(true, join_flattened);
    verify_load(&p, &catalog).unwrap();
}

#[test]
fn block_parameter_join_with_one_bound_edge_is_not_flat() {
    let (p, catalog) = bound_tail(false, join_flattened);
    reject(&p, &catalog, "RETURN_PACKET_NOT_FLAT");
}

/// An owning function beside fixture valued's `Res { label: str }` whose input packet circulates through a
/// loop before ret.own returns it. With `obligation` the packet first receives a created object.
fn loop_carried(obligation: bool) -> (Program, Catalog) {
    let (mut p, catalog) = fixture("valued");
    let mut f = empty_function();
    f.symbol = "circulate".into();
    f.entry_abi = EntryAbi::Owned;
    let item = if obligation { Type::Object(0) } else { Type::Int };
    f.registers = vec![Type::Str, item, Type::Bool];
    f.services = vec![ServiceKind::Packet; 5];
    f.services.push(ServiceKind::Deps);
    f.params = vec![Location::Register(0), Location::Service(0)];
    f.blocks = vec![
        Block { pc: 0, params: f.params.clone() },
        Block { pc: 5, params: vec![Location::Service(3)] },
        Block { pc: 6, params: vec![] },
        Block { pc: 8, params: vec![] },
    ];
    f.proofs.entry = Some(EntryContract::default());
    let service = |id| Argument::Location(Location::Service(id));
    let carry = |source| Edge { target_pc: 5,
        assignments: vec![Assignment { destination: Location::Service(3), source: Source::Location(source) }] };
    let (create, commit, returned) = if obligation {
        f.results = vec![Type::Object(0)];
        (Instruction::Data { op: DataOp::ObjNew, inputs: vec![0], output: Some(1), immediate: Immediate::Object(0) },
            commit_into(1, 1),
            vec![Argument::Location(Location::Register(1)), service(3), service(5)])
    } else {
        (Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(1),
            immediate: Immediate::Constant(Constant::Int("1".into())) },
            Instruction::Champ { op: Opcode::OwnEmpty, arguments: vec![],
                flow: Flow::Next(vec![Location::Service(1)]) },
            vec![service(3), service(5)])
    };
    f.code = vec![
        create,
        commit,
        Instruction::Champ { op: Opcode::OwnJoin, arguments: vec![service(0), service(1)],
            flow: Flow::Next(vec![Location::Service(2)]) },
        Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(2),
            immediate: Immediate::Constant(Constant::Bool(true)) },
        Instruction::Br(carry(Location::Service(2))),
        Instruction::Cond { condition: 2, yes: Edge { target_pc: 6, assignments: vec![] },
            no: Edge { target_pc: 8, assignments: vec![] } },
        Instruction::Champ { op: Opcode::OwnMove, arguments: vec![service(3)],
            flow: Flow::Next(vec![Location::Service(4)]) },
        Instruction::Br(carry(Location::Service(4))),
        Instruction::Champ { op: Opcode::DepsEmpty, arguments: vec![], flow: Flow::Next(vec![Location::Service(5)]) },
        Instruction::Champ { op: Opcode::RetOwn, arguments: returned, flow: Flow::Next(vec![]) },
    ];
    refresh(&mut f);
    p.functions.push(f);
    (p, catalog)
}

#[test]
fn loop_carried_empty_packet_is_flat() {
    let (p, catalog) = loop_carried(false);
    verify_load(&p, &catalog).unwrap();
}

#[test]
fn loop_carried_packet_with_an_obligation_is_not_flat() {
    // The same join without the loop loads: creation_commits_past_an_unrelated_instruction.
    let (p, catalog) = loop_carried(true);
    reject(&p, &catalog, "RETURN_PACKET_NOT_FLAT");
}

/// An owning function beside fixture valued's main that splits its opaque input packet with
/// own.split_checked. The normal edge returns through exit.prepare and exit.commit; the unwind edge
/// flattens its packet the same way, or with `direct` returns the unwind packet slot itself.
fn split_checked_unwind(direct: bool) -> (Program, Catalog) {
    let (mut p, catalog) = fixture("valued");
    let mut f = empty_function();
    f.symbol = "split".into();
    f.entry_abi = EntryAbi::Owned;
    f.registers = vec![Type::Object(0), Type::Failure, Type::Object(0), Type::Object(0)];
    use ServiceKind::{Deps, Packet, Pending};
    f.services = vec![Packet, Packet, Packet, Deps, Packet, Packet, Pending, Packet, Deps, Deps, Pending, Packet, Deps];
    f.params = vec![Location::Register(0), Location::Service(0)];
    f.results = vec![Type::Object(0)];
    f.blocks = vec![
        Block { pc: 0, params: f.params.clone() },
        Block { pc: 1, params: vec![Location::Service(1), Location::Service(2), Location::Service(3)] },
        Block { pc: 5, params: vec![Location::Service(4), Location::Register(1)] },
    ];
    f.proofs.entry = Some(EntryContract::default());
    f.proofs.selections.push(Selection { locations: vec![] });
    let register = Location::Register;
    let service = Location::Service;
    let produced = |destination, slot| Assignment { destination, source: Source::ProducedSlot(slot) };
    let commit = |value: u32, packet: u32, pending: u32, results: [u32; 3]| [
        Instruction::Champ { op: Opcode::ExitPrepare,
            arguments: vec![Argument::Kind(ExitKind::Return), Argument::Boundary(0),
                Argument::Location(register(value)), Argument::Location(service(packet)), Argument::Selection(0)],
            flow: Flow::Next(vec![service(pending)]) },
        Instruction::Champ { op: Opcode::ExitCommit,
            arguments: vec![Argument::Location(service(pending)), Argument::Destination(ExitDestination::Caller)],
            flow: Flow::Next(vec![register(results[0]), service(results[1]), service(results[2])]) },
        ret_own((register(results[0]), service(results[1]), service(results[2]))),
    ];
    f.code = vec![
        Instruction::Champ { op: Opcode::OwnSplitChecked,
            arguments: vec![Argument::Location(service(0)), Argument::Selection(0)],
            flow: Flow::Outcomes(vec![
                Outcome { tag: Tag::Normal, edge: Edge { target_pc: 1,
                    assignments: vec![produced(service(1), 0), produced(service(2), 1), produced(service(3), 2)] } },
                Outcome { tag: Tag::Unwind, edge: Edge { target_pc: 5,
                    assignments: vec![produced(service(4), 0), produced(register(1), 1)] } },
            ]) },
        Instruction::Champ { op: Opcode::OwnJoin,
            arguments: vec![Argument::Location(service(1)), Argument::Location(service(2))],
            flow: Flow::Next(vec![service(5)]) },
    ];
    f.code.extend(commit(0, 5, 6, [2, 7, 8]));
    if direct {
        f.code.push(Instruction::Champ { op: Opcode::DepsEmpty, arguments: vec![],
            flow: Flow::Next(vec![service(9)]) });
        f.code.push(ret_own((register(0), service(4), service(9))));
    } else {
        f.code.extend(commit(0, 4, 10, [3, 11, 12]));
    }
    refresh(&mut f);
    p.functions.push(f);
    (p, catalog)
}

#[test]
fn split_checked_unwind_packet_loads_after_exit_commit() {
    let (p, catalog) = split_checked_unwind(false);
    verify_load(&p, &catalog).unwrap();
}

#[test]
fn split_checked_unwind_slot_inherits_its_opaque_input() {
    let (p, catalog) = split_checked_unwind(true);
    reject(&p, &catalog, "RETURN_PACKET_NOT_FLAT");
}

// ---- D18: the narrowed descriptor closure of a merge envelope ----

fn resource_object(name: &str) -> Object {
    Object {
        name: name.into(),
        fields: vec![],
        enum_cases: 0,
        variant_cases: vec![],
        handle: false,
        role: None,
        implements: vec![],
        error_cases: vec![],
        layout: ObjectRuntimeLayout {
            storage_fields: vec![],
            allocation_words: 5,
            identity_offset: Some(8),
            has_destroy: true,
            may_reach_resource: true,
            may_async_destroy: false,
            trace_layout: vec![],
            selection_layout: vec![],
        },
    }
}

/// A destroy entry of `object` that declares `failures`.
fn destroy_entry(object: u32, failures: u32) -> Function {
    let mut f = scalar(1, vec![Type::Object(object)], vec![], vec![Instruction::Ret(vec![])]);
    f.symbol = format!("destroy{object}");
    f.proofs.entry = Some(EntryContract { failures, destroy: Some(object), ..EntryContract::default() });
    f
}

/// valued with `Res.label` retyped to a new resource object whose destroy fails with 8. Only the field
/// type makes that destroy visible to main's merge closes, whose envelopes declare `envelope`.
fn field_destroy(envelope: u32) -> (Program, Catalog) {
    let (mut p, catalog) = fixture("valued");
    p.objects.push(resource_object("Inner"));
    p.objects[0].layout.storage_fields[0] = Type::Object(1);
    p.functions.push(destroy_entry(1, 8));
    let f = main(&mut p);
    f.proofs.entry.as_mut().unwrap().failures = 15;
    for declared in &mut f.proofs.envelopes {
        declared.failures = envelope;
    }
    (p, catalog)
}

#[test]
fn merge_envelope_covers_a_destroy_reached_through_a_field() {
    let (p, catalog) = field_destroy(9);
    verify_load(&p, &catalog).unwrap();
}

#[test]
fn merge_envelope_cannot_omit_a_destroy_reached_through_a_field() {
    let (p, catalog) = field_destroy(1);
    reject_message(&p, &catalog, "D18_EXECUTOR", "main close 0: merge envelope 1 omits destroy failures 8");
}

/// fresh-call with a new resource object whose destroy fails with 8. With `callee_param` the callee takes
/// such an object, so it can reach the caller only through the callee's result packet. The merge closes
/// of both functions declare `envelope`.
fn callee_destroy(callee_param: bool, envelope: u32) -> (Program, Catalog) {
    let (mut p, catalog) = fixture("fresh-call");
    p.objects.push(resource_object("Inner"));
    p.functions.push(destroy_entry(1, 8));
    if callee_param {
        let fresh = function_named(&mut p, "fresh");
        let param = new_register(fresh, Type::Object(1));
        fresh.params.insert(0, param);
        fresh.blocks[0].params = fresh.params.clone();
        refresh(fresh);
        let f = main(&mut p);
        assert!(matches!(f.code[9], Instruction::Champ { op: Opcode::InvokeCallOwn, .. }), "fixture changed");
        let argument = new_register(f, Type::Object(1));
        f.params.push(argument);
        f.blocks[0].params = f.params.clone();
        let Instruction::Champ { arguments, .. } = &mut f.code[9] else { unreachable!() };
        arguments.insert(3, Argument::Location(argument));
        refresh(f);
    }
    for symbol in ["main", "fresh"] {
        let f = function_named(&mut p, symbol);
        f.proofs.envelopes[2].failures = envelope;
    }
    (p, catalog)
}

#[test]
fn merge_envelope_covers_a_destroy_reached_through_a_callee() {
    let (p, catalog) = callee_destroy(true, 9);
    verify_load(&p, &catalog).unwrap();
}

#[test]
fn merge_envelope_cannot_omit_a_destroy_reached_through_a_callee() {
    let (p, catalog) = callee_destroy(true, 1);
    reject_message(&p, &catalog, "D18_EXECUTOR", "main close 2: merge envelope 1 omits destroy failures 9");
}

#[test]
fn merge_envelope_does_not_require_an_unrelated_destroy() {
    let (p, catalog) = callee_destroy(false, 1);
    verify_load(&p, &catalog).unwrap();
}

// ---- scope.split with the admissible flag ----

#[test]
fn scope_split_loads_with_the_admissible_flag() {
    let (mut p, catalog) = fixture("valued");
    let Instruction::Champ { op: Opcode::ScopeSplit, arguments, .. } = &mut main(&mut p).code[2] else {
        panic!("fixture changed")
    };
    arguments.push(Argument::Flag(true));
    verify_load(&p, &catalog).unwrap();
}

#[test]
fn scope_split_takes_at_most_one_admissible_flag() {
    let (mut p, catalog) = fixture("valued");
    let Instruction::Champ { op: Opcode::ScopeSplit, arguments, .. } = &mut main(&mut p).code[2] else {
        panic!("fixture changed")
    };
    arguments.extend([Argument::Flag(true), Argument::Flag(true)]);
    reject_message(&p, &catalog, "SPB_STRUCTURE", "invalid operands for scope.split");
}

#[test]
fn ret_own_cannot_drop_a_successor_permission() {
    // successor_commit's function returns the original object instead of taking the successor.
    let (mut p, catalog) = successor_commit(0);
    let f = function_named(&mut p, "advance");
    assert!(matches!(f.code[5], Instruction::Data { op: DataOp::Move, .. }), "shape changed");
    f.code.remove(5);
    let Instruction::Champ { arguments, .. } = &mut f.code[6] else { panic!("ret.own") };
    arguments[0] = Argument::Location(Location::Register(1));
    refresh(f);
    reject_message(&p, &catalog, "SUCCESSOR_DROPPED", "successor must-use permission");
}

// ---- Successor must-use permissions (#680) ----
// A permission is taken (SUCCESSOR_TAKEN when it is already gone), never lost on a normal path
// (SUCCESSOR_DROPPED), never created from nothing or duplicated (SUCCESSOR_FORGED), and paths that join
// agree on it (SUCCESSOR_JOIN).

fn successor_program(functions: Vec<Function>, summaries: Vec<Summary>) -> Program {
    Program {
        objects: vec![plain_object(vec![])],
        functions,
        tables: ChampTables { summaries, ..ChampTables::default() },
        ..Program::default()
    }
}

fn obj_next(input: u32, output: u32) -> Instruction {
    Instruction::Data { op: DataOp::ObjNext, inputs: vec![input], output: Some(output),
        immediate: Immediate::Updates(vec![]) }
}

/// `next(r0: Plain) -> ^Plain`: derives and returns the successor of its argument.
fn derive_next() -> Function {
    let mut f = scalar(1, vec![Type::Object(0), Type::Successor(0)], vec![Type::Successor(0)], vec![
        obj_next(0, 1),
        Instruction::Ret(vec![1]),
    ]);
    f.symbol = "next".into();
    f
}

/// main(r0: Plain) observes `next(r0)`, which cannot fail, so the successor result r1 arrives through
/// `Flow::Next`; `tail` then uses it. r2 and r3 are Plain registers for takes.
fn observed_successor(tail: Vec<Instruction>) -> Program {
    let mut code = vec![Instruction::Champ { op: Opcode::InvokeObserve,
        arguments: vec![Argument::Entry(1), Argument::Summary(0), Argument::Location(Location::Register(0))],
        flow: Flow::Next(vec![Location::Register(1)]) }];
    code.extend(tail);
    let registers = vec![Type::Object(0), Type::Successor(0), Type::Object(0), Type::Object(0)];
    let main = scalar(1, registers, vec![], code);
    successor_program(vec![main, derive_next()], vec![Summary { entry: 1, may_fail: false, retained_params: vec![] }])
}

#[test]
fn observed_successor_result_is_taken_once() {
    let p = observed_successor(vec![data(DataOp::Move, vec![1], 2), Instruction::Ret(vec![])]);
    verify_load(&p, &NoExternalContracts).unwrap();
}

#[test]
fn observed_successor_result_cannot_be_dropped() {
    let p = observed_successor(vec![Instruction::Ret(vec![])]);
    reject_message(&p, &NoExternalContracts, "SUCCESSOR_DROPPED", "return loses successor must-use permission");
}

#[test]
fn observed_successor_result_cannot_be_taken_twice() {
    let p = observed_successor(vec![
        data(DataOp::Move, vec![1], 2),
        data(DataOp::Move, vec![1], 3),
        Instruction::Ret(vec![]),
    ]);
    reject_message(&p, &NoExternalContracts, "SUCCESSOR_TAKEN", "successor permission already taken");
}

/// main(r0: Plain) observes `next(r0)` that may fail: the normal outcome's produced successor is assigned
/// to the parameters `normal` lists of the block at pc 1 (each from produced slot 0), which takes the first
/// one and returns; the unwind outcome raises.
fn fallible_observed_successor(normal: usize) -> Program {
    let params = (0..normal as u32).map(|index| Location::Register(3 + index)).collect::<Vec<_>>();
    let assignments = params.iter().map(|&destination| Assignment { destination, source: Source::ProducedSlot(0) })
        .collect();
    let mut registers = vec![Type::Object(0), Type::Failure, Type::Object(0)];
    registers.extend(params.iter().map(|_| Type::Successor(0)));
    let mut tail = params.iter().take(1).map(|_| data(DataOp::Move, vec![3], 2)).collect::<Vec<_>>();
    tail.push(Instruction::Ret(vec![]));
    let mut code = vec![Instruction::Champ { op: Opcode::InvokeObserve,
        arguments: vec![Argument::Entry(1), Argument::Summary(0), Argument::Location(Location::Register(0))],
        flow: Flow::Outcomes(vec![
            Outcome { tag: Tag::Normal, edge: Edge { target_pc: 1, assignments } },
            Outcome { tag: Tag::Unwind, edge: Edge { target_pc: 1 + tail.len() as u32, assignments: vec![
                Assignment { destination: Location::Register(1), source: Source::ProducedSlot(0) }] } },
        ]) }];
    let raise_pc = 1 + tail.len() as u32;
    code.extend(tail);
    code.push(Instruction::Raise(1));
    let mut main = scalar(1, registers, vec![], code);
    main.blocks.push(Block { pc: 1, params });
    main.blocks.push(Block { pc: raise_pc, params: vec![Location::Register(1)] });
    main.proofs.entry.as_mut().unwrap().failures = 15;
    refresh(&mut main);
    let mut next = derive_next();
    next.proofs.entry.as_mut().unwrap().failures = 15;
    successor_program(vec![main, next], vec![Summary { entry: 1, may_fail: true, retained_params: vec![] }])
}

#[test]
fn outcome_successor_result_is_assigned_once() {
    verify_load(&fallible_observed_successor(1), &NoExternalContracts).unwrap();
}

#[test]
fn outcome_successor_result_cannot_be_dropped() {
    let p = fallible_observed_successor(0);
    reject_message(&p, &NoExternalContracts, "SUCCESSOR_DROPPED", "drops a produced successor permission");
}

#[test]
fn outcome_successor_result_cannot_be_duplicated() {
    let p = fallible_observed_successor(2);
    reject_message(&p, &NoExternalContracts, "SUCCESSOR_FORGED", "duplicates a produced successor permission");
}

/// fixture valued's owning main returns its parameter r0 through the exit protocol: exit.prepare (pc 3) moves
/// r0 into the pending exit, cleanup and exit.dispatch thread it, exit.commit (pc 8) produces r1 and ret.own
/// (pc 9) returns r1. `parameter` and `result` retype r0 and r1 (with the function result) to `^Res`.
fn successor_exit(parameter: bool, result: bool) -> (Program, Catalog) {
    let (mut p, catalog) = fixture("valued");
    let f = main(&mut p);
    assert!(matches!(f.code[3], Instruction::Champ { op: Opcode::ExitPrepare, .. }), "fixture changed");
    assert!(matches!(f.code[8], Instruction::Champ { op: Opcode::ExitCommit, .. }), "fixture changed");
    assert!(matches!(f.code[9], Instruction::Champ { op: Opcode::RetOwn, .. }), "fixture changed");
    assert_eq!((f.registers[0], f.registers[1], f.params[0]), (Type::Object(0), Type::Object(0),
        Location::Register(0)), "fixture changed");
    if parameter {
        f.registers[0] = Type::Successor(0);
    }
    if result {
        f.registers[1] = Type::Successor(0);
        f.results[0] = Type::Successor(0);
    }
    (p, catalog)
}

#[test]
fn exit_commit_returns_the_successor_its_exit_holds() {
    let (p, catalog) = successor_exit(true, true);
    verify_load(&p, &catalog).unwrap();
}

#[test]
fn exit_commit_cannot_produce_a_successor_its_exit_does_not_hold() {
    let (p, catalog) = successor_exit(false, true);
    reject_message(&p, &catalog, "SUCCESSOR_FORGED", "exit.commit produces a successor from an exit that holds none");
}

#[test]
fn exit_commit_cannot_store_a_held_successor_in_a_plain_register() {
    let (p, catalog) = successor_exit(true, false);
    reject_message(&p, &catalog, "SUCCESSOR_DROPPED", "non-successor register");
}

#[test]
fn exit_prepare_moves_the_successor_into_the_exit() {
    // ret.own returns the parameter r0 itself, whose permission exit.prepare moved into the exit at pc 3.
    let (mut p, catalog) = successor_exit(true, true);
    let Instruction::Champ { arguments, .. } = &mut main(&mut p).code[9] else { unreachable!() };
    arguments[0] = Argument::Location(Location::Register(0));
    reject_message(&p, &catalog, "SUCCESSOR_TAKEN", "ret.own uses a taken successor permission");
}

#[test]
fn exit_dispatch_cannot_succeed_without_the_held_successor() {
    // The normal path's exit.dispatch (pc 21) is declared without a reserve, so it succeeds with only the
    // closed state, into a new block at the end: nothing continues the exit that holds r0.
    let (mut p, catalog) = successor_exit(true, true);
    let f = main(&mut p);
    let closed = f.services.len() as u32;
    f.services.push(ServiceKind::Closed);
    let end = f.code.len() as u32;
    let Instruction::Champ { op: Opcode::ExitDispatch, arguments, flow: Flow::Outcomes(outcomes) } = &mut f.code[21]
    else {
        panic!("fixture changed")
    };
    assert_eq!((arguments[3].clone(), outcomes[0].tag), (Argument::Flag(true), Tag::Pending), "fixture changed");
    arguments[3] = Argument::Flag(false);
    outcomes[0] = Outcome { tag: Tag::Success, edge: Edge { target_pc: end, assignments: vec![Assignment {
        destination: Location::Service(closed), source: Source::ProducedSlot(0) }] } };
    f.code.push(Instruction::Unreachable);
    f.blocks.push(Block { pc: end, params: vec![Location::Service(closed)] });
    reject_message(&p, &catalog, "SUCCESSOR_DROPPED", "an outcome drops the successor held by its exit");
}

#[test]
fn ret_own_cannot_return_a_taken_successor() {
    // successor_commit's function returns its successor r2 after taking it into r3.
    let (mut p, catalog) = successor_commit(0);
    let f = function_named(&mut p, "advance");
    f.results[0] = Type::Successor(0);
    let Instruction::Champ { op: Opcode::RetOwn, arguments, .. } = &mut f.code[7] else { panic!("ret.own") };
    arguments[0] = Argument::Location(Location::Register(2));
    refresh(f);
    reject_message(&p, &catalog, "SUCCESSOR_TAKEN", "ret.own uses a taken successor permission");
}

#[test]
fn ret_own_returns_a_live_successor() {
    // successor_commit's function returns its successor r2 itself instead of taking it.
    let (mut p, catalog) = successor_commit(0);
    let f = function_named(&mut p, "advance");
    f.results[0] = Type::Successor(0);
    f.code[5] = Instruction::Champ { op: Opcode::DepsEmpty, arguments: vec![],
        flow: Flow::Next(vec![Location::Service(4)]) };
    f.code.remove(6);
    let Instruction::Champ { op: Opcode::RetOwn, arguments, .. } = &mut f.code[6] else { panic!("ret.own") };
    arguments[0] = Argument::Location(Location::Register(2));
    refresh(f);
    verify_load(&p, &catalog).unwrap();
}

/// `pass(r0: Plain)` derives r1 = ^r0 and runs `tail`; r2, r3: Plain, r4, r5: ^Plain, r6: Bool.
fn plain_successor(tail: Vec<Instruction>, blocks: Vec<Block>) -> Program {
    let registers = vec![Type::Object(0), Type::Successor(0), Type::Object(0), Type::Object(0), Type::Successor(0),
        Type::Successor(0), Type::Bool];
    let mut code = vec![obj_next(0, 1)];
    code.extend(tail);
    let mut f = scalar(1, registers, vec![], code);
    f.blocks.extend(blocks);
    refresh(&mut f);
    successor_program(vec![f], vec![])
}

fn jump(target_pc: u32, sources: &[(u32, u32)]) -> Instruction {
    Instruction::Br(Edge { target_pc, assignments: sources.iter().map(|&(destination, source)| Assignment {
        destination: Location::Register(destination), source: Source::Location(Location::Register(source)),
    }).collect() })
}

fn block(pc: u32, params: &[u32]) -> Block {
    Block { pc, params: params.iter().copied().map(Location::Register).collect() }
}

#[test]
fn successor_take_and_forward_load() {
    let p = plain_successor(vec![jump(2, &[(4, 1)]), data(DataOp::Move, vec![4], 2), Instruction::Ret(vec![])],
        vec![block(2, &[4])]);
    verify_load(&p, &NoExternalContracts).unwrap();
}

#[test]
fn successor_cannot_be_dropped_at_unreachable() {
    let p = plain_successor(vec![Instruction::Unreachable], vec![]);
    reject_message(&p, &NoExternalContracts, "SUCCESSOR_DROPPED", "unreachable loses successor must-use permission");
}

#[test]
fn successor_cannot_be_returned_after_its_take() {
    let mut p = plain_successor(vec![data(DataOp::Move, vec![1], 2), Instruction::Ret(vec![1])], vec![]);
    p.functions[0].results = vec![Type::Successor(0)];
    reject_message(&p, &NoExternalContracts, "SUCCESSOR_TAKEN", "successor returned twice or after take");
}

#[test]
fn successor_cannot_come_from_another_data_operation() {
    let p = plain_successor(vec![
        Instruction::Data { op: DataOp::Undef, inputs: vec![], output: Some(4), immediate: Immediate::None },
        data(DataOp::Move, vec![1], 2),
        Instruction::Ret(vec![]),
    ], vec![]);
    reject_message(&p, &NoExternalContracts, "SUCCESSOR_FORGED", "invalid successor producer");
}

#[test]
fn edge_cannot_forward_a_taken_successor() {
    let p = plain_successor(vec![data(DataOp::Move, vec![1], 2), jump(3, &[(4, 1)]), data(DataOp::Move, vec![4], 3),
        Instruction::Ret(vec![])], vec![block(3, &[4])]);
    reject_message(&p, &NoExternalContracts, "SUCCESSOR_TAKEN", "edge forwards a taken successor");
}

#[test]
fn edge_cannot_duplicate_a_successor() {
    let p = plain_successor(vec![jump(2, &[(4, 1), (5, 1)]), data(DataOp::Move, vec![4], 2),
        data(DataOp::Move, vec![5], 3), Instruction::Ret(vec![])], vec![block(2, &[4, 5])]);
    reject_message(&p, &NoExternalContracts, "SUCCESSOR_FORGED", "edge duplicates successor permission");
}

#[test]
fn successor_paths_cannot_disagree_at_a_join() {
    // One branch takes r1, the other does not; both join at pc 6, which returns.
    let p = plain_successor(vec![
        Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(6),
            immediate: Immediate::Constant(Constant::Bool(true)) },
        Instruction::Cond { condition: 6, yes: Edge { target_pc: 3, assignments: vec![] },
            no: Edge { target_pc: 5, assignments: vec![] } },
        data(DataOp::Move, vec![1], 2),
        jump(6, &[]),
        jump(6, &[]),
        data(DataOp::Move, vec![1], 3),
        Instruction::Ret(vec![]),
    ], vec![block(3, &[]), block(5, &[]), block(6, &[])]);
    reject_message(&p, &NoExternalContracts, "SUCCESSOR_JOIN", "inconsistent successor permissions at CFG join");
}

/// fixture exit-updates' owning main prepares its parameter r0 as the exit value (pc 3), replaces the
/// value with r1 (pc 6, defined at pc 4) and commits it into r2, which ret.own returns.
fn replaced_exit() -> (Program, Catalog) {
    let (mut p, catalog) = fixture("exit-updates");
    let f = main(&mut p);
    assert!(matches!(f.code[3], Instruction::Champ { op: Opcode::ExitPrepare, .. }), "fixture changed");
    assert!(matches!(f.code[4], Instruction::Data { output: Some(1), .. }), "fixture changed");
    assert!(matches!(f.code[6], Instruction::Champ { op: Opcode::ExitReplace, .. }), "fixture changed");
    assert_eq!((f.registers[0], f.registers[2], f.params[0]), (Type::Object(0), Type::Int, Location::Register(0)),
        "fixture changed");
    (p, catalog)
}

#[test]
fn exit_replace_moves_a_successor_value_into_the_exit() {
    // pc 4 derives r1 = ^r0 instead of a constant; the replaced exit then holds r1 and commits it into r2.
    let (mut p, _) = replaced_exit();
    let f = main(&mut p);
    f.code[4] = obj_next(0, 1);
    f.registers[1] = Type::Successor(0);
    f.registers[2] = Type::Successor(0);
    f.results[0] = Type::Successor(0);
    refresh(f);
    // The structural verifier runs every successor check; the load gate would further require r1's own
    // commit (CREATION_UNCOMMITTED), which this fixture has no packet for.
    verify(&p).unwrap();
}

#[test]
fn exit_replace_cannot_replace_a_held_successor() {
    let (mut p, catalog) = replaced_exit();
    main(&mut p).registers[0] = Type::Successor(0);
    reject_message(&p, &catalog, "SUCCESSOR_DROPPED", "exit.replace drops the successor held by its exit");
}

#[test]
fn exit_carry_follows_the_exit_service_of_each_schema() {
    use verify::{Carry, exit_carry};
    let contexts = [
        ResultContext::default(),
        ResultContext { reserve: true, ..ResultContext::default() },
        ResultContext { may_fail: true, ..ResultContext::default() },
        ResultContext { value: Some(Type::Successor(0)), ..ResultContext::default() },
    ];
    let mut slots = 0;
    for &op in Opcode::ALL {
        for context in &contexts {
            for tag in op.tags(context) {
                let Carry::Slot(slot) = exit_carry(op, tag) else { continue };
                let schema = op.results(tag, context).unwrap();
                assert!(matches!(schema.get(slot), Some(ProducedType::Service(
                    ServiceKind::Pending | ServiceKind::Cleanup | ServiceKind::ExitState))),
                    "{} {tag:?}: slot {slot} of {schema:?}", op.name());
                slots += 1;
            }
        }
    }
    assert!(slots > 0);
    // The exit ends only on failure paths; a successful dispatch without a reserve has no pending to carry on.
    assert_eq!(exit_carry(Opcode::ExitAbort, Tag::Normal), Carry::Lapses);
    assert_eq!(exit_carry(Opcode::ExitDispatch, Tag::Failure), Carry::Lapses);
    assert_eq!(exit_carry(Opcode::ExitDispatch, Tag::Success), Carry::Lost);
    assert_eq!(exit_carry(Opcode::ExitDispatch, Tag::Pending), Carry::Slot(1));
    assert_eq!(exit_carry(Opcode::ExitCommit, Tag::Normal), Carry::Commit);
    assert_eq!(exit_carry(Opcode::ExitReplace, Tag::Normal), Carry::Lost);
    assert_eq!(exit_carry(Opcode::InvokeCallOwn, Tag::Normal), Carry::Lost);
}

#[test]
fn edge_cannot_overwrite_a_live_successor() {
    // The loop at pc 2 derives r5 and binds it to its parameter r4 again, which still holds the previous one.
    let p = plain_successor(vec![jump(2, &[(4, 1)]), obj_next(0, 5), jump(2, &[(4, 5)])], vec![block(2, &[4])]);
    reject_message(&p, &NoExternalContracts, "SUCCESSOR_DROPPED", "edge overwrites live successor permission");
}
