use super::*;
use super::tests::empty_function;

#[derive(Clone)]
struct Catalog(Vec<Function>);
impl ExternalContracts for Catalog {
    fn entry(&self, name: &str) -> Option<&Function> {
        self.0.iter().find(|f| f.proofs.entry.as_ref().and_then(|entry| entry.external.as_deref()) == Some(name))
    }
}
fn fixture(name: &str) -> (Program, Catalog) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../okkama-compiler/src/codegen/sparrow_oir2/fixtures/{name}.spb17"));
    let program = assemble(&std::fs::read_to_string(path).unwrap()).unwrap();
    let catalog = Catalog(program.functions.iter().filter(|f| f.proofs.entry.as_ref()
        .is_some_and(|entry| entry.external.is_some())).cloned().collect());
    verify_load(&program, &catalog).unwrap();
    (program, catalog)
}
fn main(p: &mut Program) -> &mut Function {
    p.functions.iter_mut().find(|f| f.symbol == "main").unwrap()
}
fn reject(p: &Program, catalog: &dyn ExternalContracts, code: &'static str) {
    let failure = verify_load(p, catalog).unwrap_err();
    assert_eq!(failure.rule, code, "{failure}");
}
fn small(f: Function) -> Program {
    Program { functions: vec![f], ..Program::default() }
}
fn failure_param(f: &mut Function) -> Location {
    let location = Location::Register(f.registers.len() as u32);
    f.registers.push(Type::Failure);
    f.params.push(location);
    f.blocks[0].params.push(location);
    f.proofs.entry.as_mut().unwrap().failures = 15;
    location
}
/// Repair only ancillary evidence in a counterexample, so the targeted semantic rule is reached.
fn refresh(f: &mut Function) {
    f.proofs.states = service_states(f).unwrap();
    f.proofs.roots.clear();
    f.proofs.post_roots.clear();
    let live = live_locations(f).unwrap();
    f.proofs.roots = live.iter().enumerate().map(|(pc, locations)| RootRequirement {
        pc: pc as u32, locations: locations.iter().copied().collect(),
    }).collect();
    for pc in 0..f.code.len().saturating_sub(1) {
        if matches!(f.code[pc], Instruction::Champ { flow: Flow::Next(_), .. }) {
            f.proofs.post_roots.push(RootRequirement {
                pc: pc as u32, locations: live[pc + 1].iter().copied().collect(),
            });
        }
    }
}

#[test]
fn pending_return_cannot_reinterpret_int_as_object() {
    let (mut p, catalog) = fixture("valued");
    main(&mut p).registers[0] = Type::Int;
    reject(&p, &catalog, "PENDING_RETURN_TYPE");
}

#[test]
fn stage_checks_physical_field_type() {
    let (mut p, catalog) = fixture("valued");
    let f = main(&mut p);
    // Replace accept with a Same staging operation into Res.label: str.
    let Instruction::Champ { arguments, flow, .. } = &f.code[1] else { panic!("accept") };
    let args = vec![Argument::Flag(false), arguments[0].clone(), Argument::Object(0), Argument::Field(0),
        Argument::Location(Location::Register(0)), arguments[1].clone()];
    f.code[1] = Instruction::Champ { op: Opcode::OwnStage, arguments: args, flow: flow.clone() };
    reject(&p, &catalog, "STAGE_FIELD_TYPE");
}

#[test]
fn bind_cannot_hide_a_committed_packet_in_a_value_operand() {
    let (mut p, catalog) = fixture("valued");
    let f = main(&mut p);
    let Instruction::Champ { arguments, flow, .. } = &f.code[1] else { panic!("accept") };
    let args = vec![arguments[0].clone(), Argument::Binding(0), Argument::Scope(0),
        arguments[1].clone(), arguments[1].clone()];
    f.code[1] = Instruction::Champ { op: Opcode::OwnBind, arguments: args, flow: flow.clone() };
    reject(&p, &catalog, "OPERAND_VALUE");
}

#[test]
fn defer_capture_cannot_consume_cleanup() {
    let mut f = empty_function();
    f.services = vec![ServiceKind::ScopeState, ServiceKind::Cleanup, ServiceKind::ScopeState];
    f.proofs.registrations.push(RegistrationMap { entry: 1, labels: vec![] });
    f.code.insert(0, Instruction::Champ { op: Opcode::DeferRegister,
        arguments: vec![Argument::Location(Location::Service(0)), Argument::Registration(0),
            Argument::Location(Location::Service(1))], flow: Flow::Next(vec![Location::Service(2)]) });
    let mut defer = empty_function();
    defer.entry_abi = EntryAbi::Defer { labels: vec![] };
    let mut p = small(f);
    p.functions.push(defer);
    reject(&p, &NoExternalContracts, "OPERAND_VALUE");
}

#[test]
fn scope_split_selection_operand_cannot_consume_pending() {
    let (mut p, catalog) = fixture("valued");
    let f = main(&mut p);
    let pending = f.services.iter().position(|kind| *kind == ServiceKind::Pending).unwrap() as u32;
    let Instruction::Champ { arguments, .. } = &mut f.code[2] else { panic!("split") };
    arguments.push(Argument::Location(Location::Service(pending)));
    reject(&p, &catalog, "OPERAND_VALUE");
}

#[test]
fn every_selection_table_requires_values() {
    for name in ["valued", "exit-updates"] {
        let (mut p, catalog) = fixture(name);
        let f = main(&mut p);
        f.proofs.selections.last_mut().unwrap().locations = vec![Location::Service(0)];
        reject(&p, &catalog, "OPERAND_VALUE");
    }
}

#[test]
fn close_tail_cannot_substitute_pending_for_active_failure() {
    let (mut p, catalog) = fixture("valued");
    let f = main(&mut p);
    f.proofs.closes[0].reserve = false;
    f.proofs.closes[0].active_failure = true;
    reject(&p, &catalog, "CLOSE_TAIL");
}

#[test]
fn declared_failure_mask_covers_raise_and_scalar_failure() {
    let mut raise = empty_function();
    raise.registers = vec![Type::Failure];
    raise.params = vec![Location::Register(0)];
    raise.blocks[0].params = raise.params.clone();
    raise.code = vec![Instruction::Raise(0)];
    reject(&small(raise), &NoExternalContracts, "ENTRY_FAILURE_MASK");
    let mut modulo = empty_function();
    modulo.registers = vec![Type::Int; 3];
    modulo.params = vec![Location::Register(0), Location::Register(1)];
    modulo.blocks[0].params = modulo.params.clone();
    modulo.code.insert(0, Instruction::Data { op: DataOp::IntMod, inputs: vec![0, 1], output: Some(2),
        immediate: Immediate::None });
    reject(&small(modulo), &NoExternalContracts, "ENTRY_FAILURE_MASK");
}

#[test]
fn declared_failure_mask_covers_unhandled_call_unwind() {
    let (mut p, catalog) = fixture("fresh-call");
    main(&mut p).proofs.entry.as_mut().unwrap().failures = 0;
    reject(&p, &catalog, "ENTRY_FAILURE_MASK");
}

#[test]
fn begun_failure_requires_envelope_or_active_failure() {
    let (mut p, catalog) = fixture("valued");
    let f = main(&mut p);
    let failure = failure_param(f);
    f.proofs.envelopes[0].failures = 0;
    let Instruction::Champ { arguments, flow, .. } = &f.code[5] else { panic!("settle") };
    f.code[5] = Instruction::Champ { op: Opcode::CleanupFailure,
        arguments: vec![arguments[0].clone(), Argument::Location(failure)], flow: flow.clone() };
    refresh(f);
    reject(&p, &catalog, "CLEANUP_FAILURE_BEGIN");
}

#[test]
fn cleanup_failure_must_use_its_dispatch_slot() {
    let (mut p, catalog) = fixture("valued");
    let f = main(&mut p);
    let other = failure_param(f);
    let arguments = f.code.iter_mut().find_map(|inst| match inst {
        Instruction::Champ { op: Opcode::CleanupFailure, arguments, .. } => Some(arguments), _ => None,
    }).unwrap();
    arguments[1] = Argument::Location(other);
    refresh(f);
    reject(&p, &catalog, "CLEANUP_FAILURE_SLOT");
}

#[test]
fn merge_envelope_covers_visible_descriptor_destroys() {
    let (mut p, mut catalog) = fixture("valued");
    p.functions[0].proofs.entry.as_mut().unwrap().failures = 8;
    catalog.0[0].proofs.entry.as_mut().unwrap().failures = 8;
    main(&mut p).proofs.entry.as_mut().unwrap().failures = 15;
    reject(&p, &catalog, "D18_EXECUTOR");
}

#[test]
fn owning_entry_cannot_use_plain_ret() {
    let (mut p, catalog) = fixture("valued");
    let f = main(&mut p);
    let inst = f.code.iter_mut().find(|inst| matches!(inst,
        Instruction::Champ { op: Opcode::RetOwn, .. })).unwrap();
    *inst = Instruction::Ret(vec![1]);
    reject(&p, &catalog, "ENTRY_RETURN_ABI");
}

#[test]
fn fresh_summary_requires_a_reachable_return() {
    let mut f = empty_function();
    f.registers = vec![Type::Failure];
    f.params = vec![Location::Register(0)];
    f.blocks[0].params = f.params.clone();
    f.blocks.push(Block { pc: 1, params: vec![] });
    f.proofs.entry = Some(EntryContract { failures: 15, fresh_singleton: true, ..EntryContract::default() });
    f.code = vec![Instruction::Raise(0), Instruction::Ret(vec![])];
    f.proofs.roots.push(RootRequirement { pc: 0, locations: f.params.clone() });
    reject(&small(f), &NoExternalContracts, "FRESH_REACHABLE_RETURN");
}

#[test]
fn proof_maps_are_compared_instead_of_repaired() {
    let (p, catalog) = fixture("valued");
    let mut states = p.clone();
    main(&mut states).proofs.states.clear();
    reject(&states, &catalog, "EVIDENCE_STATES");
    let mut sources = p.clone();
    main(&mut sources).proofs.cleanup_plans[0].sources.pop();
    reject(&sources, &catalog, "EVIDENCE_SOURCES");
    let mut reserve = p.clone();
    let evidence = &mut main(&mut reserve).proofs.cleanup_plans[0];
    evidence.reserve = false;
    evidence.sources.pop();
    reject(&reserve, &catalog, "EVIDENCE_RESERVE");
    let mut before = p.clone();
    main(&mut before).proofs.roots.clear();
    reject(&before, &catalog, "ROOTS_BEFORE");
    let mut after = p;
    main(&mut after).proofs.post_roots.clear();
    reject(&after, &catalog, "ROOTS_AFTER");
}

#[test]
fn failure_state_is_not_an_entry_parameter() {
    let mut f = empty_function();
    f.services.push(ServiceKind::FailureState);
    f.params.push(Location::Service(0));
    f.blocks[0].params = f.params.clone();
    reject(&small(f), &NoExternalContracts, "ENTRY_PRIVATE_SERVICE");
}

#[test]
fn unused_merge_plans_are_checked_too() {
    let (mut p, catalog) = fixture("valued");
    let f = main(&mut p);
    let id = f.proofs.plans.len() as u32;
    f.proofs.plans.push(f.proofs.plans[0].clone());
    let mut unused = f.proofs.cleanup_plans[0].clone();
    unused.plan = id;
    unused.reserve_empty = true;
    f.proofs.cleanup_plans.push(unused);
    reject(&p, &catalog, "MERGE_EVIDENCE");
}

#[test]
fn hundred_thousand_moves_are_bounded_without_recursion() {
    let mut f = empty_function();
    f.registers = vec![Type::Int; 100_001];
    f.params = vec![Location::Register(0)];
    f.blocks[0].params = f.params.clone();
    f.code = (1..=100_000).map(|id| Instruction::Data { op: DataOp::Move, inputs: vec![id - 1],
        output: Some(id), immediate: Immediate::None }).collect();
    f.code.push(Instruction::Ret(vec![]));
    let p = small(f);
    let mut evidence = evidence::Evidence::new(&p, &p.functions[0]);
    assert_eq!(evidence.identity(Location::Register(100_000)), None);
    assert_eq!(evidence.check_budget().unwrap_err().rule, "LOAD_WORK_BUDGET");
    reject(&p, &NoExternalContracts, "LOAD_WORK_BUDGET");
}

#[test]
fn replaced_pending_keeps_its_new_value_type() {
    let (mut p, catalog) = fixture("exit-updates");
    let f = main(&mut p);
    for instruction in &mut f.code {
        if let Instruction::Champ { op: Opcode::ExitReplace, arguments, .. } = instruction {
            let locations = arguments.iter_mut().filter(|arg| matches!(arg, Argument::Location(_)))
                .collect::<Vec<_>>();
            *locations.into_iter().nth(1).unwrap() = Argument::Location(Location::Register(0));
            break;
        }
    }
    refresh(f);
    reject(&p, &catalog, "PENDING_RETURN_TYPE");
}

#[test]
fn abort_before_plan_still_requires_merge() {
    let (mut p, catalog) = fixture("explicit-abort");
    main(&mut p).proofs.plans[0].merge = false;
    reject(&p, &catalog, "ABORT_MERGE");
}

#[test]
fn packet_is_consumed_only_once() {
    let mut f = empty_function();
    f.services = vec![ServiceKind::Packet; 2];
    f.params = vec![Location::Service(0)];
    f.blocks[0].params = f.params.clone();
    f.code.insert(0, Instruction::Champ { op: Opcode::OwnJoin,
        arguments: vec![Argument::Location(Location::Service(0)); 2],
        flow: Flow::Next(vec![Location::Service(1)]) });
    reject(&small(f), &NoExternalContracts, "LINEAR_CONSUME");
}

#[test]
fn static_selection_evidence_is_compared() {
    let (mut p, catalog) = fixture("exit-updates");
    main(&mut p).proofs.static_selections[0].indices.push(17);
    reject(&p, &catalog, "EVIDENCE_SELECTIONS");
}

#[test]
fn destroy_cannot_understate_its_body_failures() {
    let (mut p, catalog) = fixture("valued");
    let destroy = &mut p.functions[0];
    destroy.proofs.entry.as_mut().unwrap().external = None;
    destroy.registers.extend([Type::Int; 3]);
    destroy.blocks.push(Block { pc: 0, params: destroy.params.clone() });
    destroy.code = vec![
        Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(1),
            immediate: Immediate::Constant(Constant::Int("1".into())) },
        Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(2),
            immediate: Immediate::Constant(Constant::Int("0".into())) },
        Instruction::Data { op: DataOp::IntMod, inputs: vec![1, 2], output: Some(3), immediate: Immediate::None },
        Instruction::Ret(vec![]),
    ];
    reject(&p, &catalog, "ENTRY_FAILURE_MASK");
}

#[test]
fn unreachable_ret_own_does_not_authorize_a_reachable_plain_ret() {
    let (mut p, catalog) = fixture("valued");
    let f = main(&mut p);
    f.proofs.entry.as_mut().unwrap().fresh_singleton = true;
    let returned = f.code.iter().position(|inst| matches!(inst,
        Instruction::Champ { op: Opcode::RetOwn, .. })).unwrap();
    let unreachable = f.code[returned].clone();
    f.code[returned] = Instruction::Ret(vec![1]);
    f.blocks.push(Block { pc: f.code.len() as u32, params: vec![] });
    f.code.push(unreachable);
    reject(&p, &catalog, "ENTRY_RETURN_ABI");
}

#[test]
fn reachable_parameter_return_cannot_be_fresh() {
    let (mut p, catalog) = fixture("valued");
    main(&mut p).proofs.entry.as_mut().unwrap().fresh_singleton = true;
    reject(&p, &catalog, "FRESH_ORIGIN");
}

#[test]
fn long_move_origin_chain_uses_explicit_frames() {
    let mut f = empty_function();
    f.registers = vec![Type::Object(0); 20_001];
    f.services = vec![ServiceKind::Packet];
    f.code = vec![
        Instruction::Data { op: DataOp::ObjNew, inputs: vec![], output: Some(0), immediate: Immediate::Object(0) },
        Instruction::Champ { op: Opcode::OwnCreateCommit,
            arguments: vec![Argument::Location(Location::Register(0))],
            flow: Flow::Next(vec![Location::Service(0)]) },
    ];
    f.code.extend((1..=20_000).map(|id| Instruction::Data { op: DataOp::Move, inputs: vec![id - 1],
        output: Some(id), immediate: Immediate::None }));
    let p = small(f);
    let mut evidence = evidence::Evidence::new(&p, &p.functions[0]);
    assert_eq!(evidence.identity(Location::Register(20_000)), Some((1, None)));
}

#[test]
fn non_return_commit_cannot_reinterpret_pending_value() {
    let (mut p, catalog) = fixture("valued");
    let f = main(&mut p);
    f.registers[0] = Type::Int;
    for instruction in &mut f.code {
        if let Instruction::Champ { op: Opcode::ExitPrepare, arguments, .. } = instruction {
            arguments[0] = Argument::Kind(ExitKind::Next);
            break;
        }
    }
    reject(&p, &catalog, "PENDING_COMMIT_TYPE");
}

/// fresh-call's callee rebuilt to bind its fresh object: create_commit, own.accept into the bound journal,
/// a scope.split without the admissible flag, and a close of the emptied scope. With `commit` the selected
/// packet returns through exit.prepare and exit.commit; without it ret.own returns that bound packet itself.
fn bound_fresh_return(commit: bool) -> (Program, Catalog) {
    let (mut p, catalog) = fixture("fresh-call");
    let f = p.functions.iter_mut().find(|f| f.symbol == "fresh").unwrap();
    assert!(matches!(f.code[11], Instruction::Champ { op: Opcode::OwnSplit, .. }), "fixture changed");
    assert!(matches!(f.code[14], Instruction::Champ { op: Opcode::CleanupBegin, .. }), "fixture changed");
    assert!(matches!(f.code[32], Instruction::Champ { op: Opcode::RetOwn, .. }), "fixture changed");
    let service = Location::Service;
    let produced = |destination, slot| Assignment { destination, source: Source::ProducedSlot(slot) };
    let emptied = service(f.services.len() as u32);
    f.services.push(ServiceKind::ScopeState);
    f.code[11] = Instruction::Champ { op: Opcode::ScopeBegin,
        arguments: vec![Argument::Scope(3), Argument::Boundary(3)], flow: Flow::Next(vec![service(12)]) };
    f.code[12] = Instruction::Champ { op: Opcode::OwnAccept,
        arguments: vec![Argument::Location(service(12)), Argument::Location(service(8))],
        flow: Flow::Next(vec![service(13)]) };
    let normal = Edge { target_pc: 14,
        assignments: vec![produced(emptied, 0), produced(service(10), 1), produced(service(11), 2)] };
    let unwind = Edge { target_pc: 20,
        assignments: vec![produced(service(19), 0), produced(Location::Register(1), 1)] };
    f.code[13] = Instruction::Champ { op: Opcode::ScopeSplit,
        arguments: vec![Argument::Location(service(13)), Argument::Selection(3)],
        flow: Flow::Outcomes(vec![Outcome { tag: Tag::Normal, edge: normal },
            Outcome { tag: Tag::Unwind, edge: unwind }]) };
    let Instruction::Champ { arguments, .. } = &mut f.code[14] else { unreachable!() };
    arguments[0] = Argument::Location(emptied);
    // The old unwind landing (own.admit of a rest packet) is dead; the split unwinds into the close.
    f.code[18] = Instruction::Unreachable;
    f.code[19] = Instruction::Unreachable;
    f.blocks.iter_mut().find(|block| block.pc == 18).unwrap().params.clear();
    f.blocks.push(Block { pc: 14, params: vec![emptied, service(10), service(11)] });
    f.blocks.push(Block { pc: 19, params: vec![] });
    f.blocks.push(Block { pc: 20, params: vec![service(19), Location::Register(1)] });
    f.proofs.static_selections.clear();
    if commit {
        let end = f.code.len() as u32;
        let base = f.services.len() as u32;
        let (pending, packet, deps) = (service(base), service(base + 1), service(base + 2));
        f.services.extend([ServiceKind::Pending, ServiceKind::Packet, ServiceKind::Deps]);
        let value = Location::Register(f.registers.len() as u32);
        f.registers.push(Type::Object(0));
        f.code[32] = Instruction::Br(Edge { target_pc: end, assignments: vec![] });
        f.blocks.push(Block { pc: end, params: vec![] });
        f.code.push(Instruction::Champ { op: Opcode::ExitPrepare,
            arguments: vec![Argument::Kind(ExitKind::Return), Argument::Boundary(2),
                Argument::Location(Location::Register(0)), Argument::Location(service(10)), Argument::Selection(3)],
            flow: Flow::Next(vec![pending]) });
        f.code.push(Instruction::Champ { op: Opcode::ExitCommit,
            arguments: vec![Argument::Location(pending), Argument::Destination(ExitDestination::Caller)],
            flow: Flow::Next(vec![value, packet, deps]) });
        f.code.push(Instruction::Champ { op: Opcode::RetOwn,
            arguments: vec![Argument::Location(value), Argument::Location(packet), Argument::Location(deps)],
            flow: Flow::Next(vec![]) });
    }
    f.blocks.sort_by_key(|block| block.pc);
    refresh(f);
    (p, catalog)
}

#[test]
fn bound_return_packet_cannot_claim_all_admission_at_its_caller() {
    // A caller's admission-only split would select nothing and its merge would destroy the result.
    let (p, catalog) = bound_fresh_return(false);
    reject(&p, &catalog, "RETURN_PACKET_NOT_FLAT");
}

#[test]
fn bound_return_packet_loads_after_exit_commit() {
    let (p, catalog) = bound_fresh_return(true);
    verify_load(&p, &catalog).unwrap();
}

fn modulo(services: Vec<ServiceKind>) -> Function {
    let mut f = empty_function();
    f.registers = vec![Type::Int; 3];
    f.params = vec![Location::Register(0), Location::Register(1)];
    f.params.extend((0..services.len() as u32).map(Location::Service));
    f.services = services;
    f.blocks[0].params = f.params.clone();
    f.proofs.entry = Some(EntryContract { failures: 2, ..EntryContract::default() });
    f.code.insert(0, Instruction::Data { op: DataOp::IntMod, inputs: vec![0, 1], output: Some(2),
        immediate: Immediate::None });
    refresh(&mut f);
    f
}

#[test]
fn fallible_data_needs_outcomes_where_services_live() {
    verify_load(&small(modulo(vec![])), &NoExternalContracts).unwrap();
    reject(&small(modulo(vec![ServiceKind::Deps])), &NoExternalContracts, "FALLIBLE_DATA_OUTCOMES");
}

#[test]
fn fallible_evaluate_loads_where_services_live() {
    let mut f = modulo(vec![ServiceKind::Deps]);
    let failure = Location::Register(3);
    f.registers.push(Type::Failure);
    let produced = |destination| Assignment { destination, source: Source::ProducedSlot(0) };
    let evaluation = Evaluation { op: DataOp::IntMod, result: Type::Int, immediate: Immediate::None };
    f.code = vec![
        Instruction::Champ { op: Opcode::Evaluate,
            arguments: vec![Argument::Evaluation(evaluation), Argument::Location(Location::Register(0)),
                Argument::Location(Location::Register(1))],
            flow: Flow::Outcomes(vec![
                Outcome { tag: Tag::Normal, edge: Edge { target_pc: 1,
                    assignments: vec![produced(Location::Register(2))] } },
                Outcome { tag: Tag::Unwind, edge: Edge { target_pc: 2, assignments: vec![produced(failure)] } },
            ]) },
        Instruction::Ret(vec![]),
        Instruction::Raise(3),
    ];
    f.blocks.push(Block { pc: 1, params: vec![Location::Register(2)] });
    f.blocks.push(Block { pc: 2, params: vec![failure] });
    refresh(&mut f);
    verify_load(&small(f), &NoExternalContracts).unwrap();
}

/// An observation function beside fixture valued's destroy-bearing `Res { label: str }`.
/// Its first `params` registers are its parameters.
fn creating(params: u32, registers: Vec<Type>, results: Vec<Type>, code: Vec<Instruction>) -> (Program, Catalog) {
    let (mut p, catalog) = fixture("valued");
    let mut f = empty_function();
    f.symbol = "create".into();
    f.params = (0..params).map(Location::Register).collect();
    f.registers = registers;
    f.results = results;
    f.blocks[0].params = f.params.clone();
    f.code = code;
    f.proofs.entry = Some(EntryContract::default());
    refresh(&mut f);
    p.functions.push(f);
    (p, catalog)
}

#[test]
fn packet_less_construction_cannot_return_a_destroy_bearing_object() {
    let (p, catalog) = creating(1, vec![Type::Str, Type::Object(0)], vec![Type::Object(0)], vec![
        Instruction::Data { op: DataOp::ObjNew, inputs: vec![0], output: Some(1), immediate: Immediate::Object(0) },
        Instruction::Ret(vec![1]),
    ]);
    reject(&p, &catalog, "CREATION_UNCOMMITTED");
}

#[test]
fn successor_cannot_leave_its_scope_without_its_commit() {
    let registers = vec![Type::Object(0), Type::Str, Type::Successor(0), Type::Object(0)];
    let (p, catalog) = creating(2, registers, vec![], vec![
        Instruction::Data { op: DataOp::ObjNext, inputs: vec![0, 1], output: Some(2),
            immediate: Immediate::Updates(vec![0]) },
        Instruction::Data { op: DataOp::Move, inputs: vec![2], output: Some(3), immediate: Immediate::None },
        Instruction::Ret(vec![]),
    ]);
    reject(&p, &catalog, "CREATION_UNCOMMITTED");
}

#[test]
fn incomplete_construction_cannot_move_a_packet_instead_of_committing() {
    // cleanup.rs's Incomplete path: the fresh object travels to ret.own beside a packet that never holds it.
    let (mut p, catalog) = fixture("fresh-call");
    let f = p.functions.iter_mut().find(|f| f.symbol == "fresh").unwrap();
    let move_packet = |from, to| Instruction::Champ { op: Opcode::OwnMove,
        arguments: vec![Argument::Location(Location::Service(from))], flow: Flow::Next(vec![Location::Service(to)]) };
    assert!(matches!(f.code[8], Instruction::Champ { op: Opcode::OwnCreateCommit, .. }), "fixture changed");
    f.code[8] = move_packet(7, 58);
    f.code[9] = move_packet(58, 59);
    refresh(f);
    reject(&p, &catalog, "CREATION_UNCOMMITTED");
}

#[test]
fn creation_may_reach_its_commit_past_unrelated_instructions() {
    let (mut p, catalog) = fixture("fresh-call");
    let f = p.functions.iter_mut().find(|f| f.symbol == "fresh").unwrap();
    assert!(matches!(f.code[7], Instruction::Data { op: DataOp::ObjNew, .. }), "fixture changed");
    f.code.swap(6, 7);
    refresh(f);
    verify_load(&p, &catalog).unwrap();
}
