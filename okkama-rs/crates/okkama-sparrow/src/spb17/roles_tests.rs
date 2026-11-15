//! Role values and dynamic dispatch (#676): role declarations, implementation tables, role.wrap,
//! role.narrow, role successors and the dispatch contract.
use super::*;

fn reject(p: &Program, rule: &str) {
    let failure = verify_load(p, &NoExternalContracts).unwrap_err();
    assert_eq!(failure.rule, rule, "{failure}");
}

fn accept(p: &Program) {
    verify_load(p, &NoExternalContracts).unwrap();
}

fn register(id: u32) -> Location {
    Location::Register(id)
}

fn service(id: u32) -> Location {
    Location::Service(id)
}

fn produced(destination: Location, slot: u32) -> Assignment {
    Assignment { destination, source: Source::ProducedSlot(slot) }
}

fn layout(storage_fields: Vec<Type>) -> ObjectRuntimeLayout {
    let count = storage_fields.len() as u32;
    let trace_layout = storage_fields.iter().enumerate().filter_map(|(index, ty)| {
        matches!(ty, Type::Str | Type::Object(_) | Type::Role(_) | Type::RoleSuccessor(_) | Type::Array(_))
            .then_some(index as u32)
    }).collect();
    ObjectRuntimeLayout {
        storage_fields, allocation_words: (1 + count).max(2), identity_offset: None, has_destroy: false,
        may_reach_resource: false, may_async_destroy: false, trace_layout, selection_layout: vec![],
    }
}

fn object(name: &str, storage_fields: Vec<Type>, implements: Vec<RoleImpl>) -> Object {
    let count = storage_fields.len() as u32;
    Object {
        name: name.into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle: false, role: None,
        implements,
        any_box: false, enum_names: vec![],
        fields: (0..count).map(|storage| Field { name: format!("f{storage}"), storage, once: false }).collect(),
        layout: layout(storage_fields),
    }
}

fn role(name: &str, slots: Vec<RoleSlot>) -> Object {
    Object { role: Some(RoleDecl { slots }), ..object(name, vec![], vec![]) }
}

fn owned_slot(name: &str) -> RoleSlot {
    RoleSlot { name: name.into(), entry_abi: EntryAbi::Owned, params: vec![], results: vec![Type::Str] }
}

/// Live locations are roots before every instruction, and after every helper with a next pc.
fn seal(f: &mut Function) {
    f.proofs.states = service_states(f).unwrap();
    f.proofs.roots.clear();
    f.proofs.post_roots.clear();
    let live = live_locations(f).unwrap();
    f.proofs.roots = live.iter().enumerate().map(|(pc, locations)| RootRequirement {
        pc: pc as u32, locations: locations.iter().copied().collect(),
    }).collect();
    f.proofs.post_roots = f.code.iter().enumerate().filter(|(_, instruction)| {
        matches!(instruction, Instruction::Champ { flow: Flow::Next(_), op, .. }
            if !matches!(op, Opcode::RetOwn | Opcode::RetDefer))
    }).map(|(pc, _)| RootRequirement {
        pc: pc as u32, locations: live.get(pc + 1).into_iter().flatten().copied().collect(),
    }).collect();
}

/// An owning implementation `(receiver; packet) -> (str, packet)` that returns `text` or fails.
fn implementation(symbol: &str, receiver: Type, failures: u32) -> Function {
    let mut f = tests::empty_function();
    f.symbol = symbol.into();
    f.entry_abi = EntryAbi::Owned;
    f.registers = vec![receiver, Type::Str];
    f.services = vec![ServiceKind::Packet];
    f.params = vec![register(0), service(0)];
    f.results = vec![Type::Str];
    f.blocks = vec![Block { pc: 0, params: f.params.clone() }];
    f.code = vec![
        Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(1),
            immediate: Immediate::Constant(Constant::Str(symbol.into())) },
        Instruction::Champ { op: Opcode::RetOwn, arguments: vec![Argument::Location(register(1)),
            Argument::Location(service(0))], flow: Flow::Next(vec![]) },
    ];
    f.proofs.entry = Some(EntryContract { failures, ..EntryContract::default() });
    seal(&mut f);
    f
}

/// Objects: 0 role `Named` (`name`), 1 `Tag` and 2 `Box` implement it, 3 `Plain` does not, 4 role `Labeled`
/// (`label`) that only `Tag` implements. Functions 1 `Tag.name`, 2 `Box.name` and 3 `Tag.label`; function 0 is
/// the caller each test builds.
fn program(caller: Function) -> Program {
    let objects = vec![
        role("Named", vec![owned_slot("name")]),
        object("Tag", vec![], vec![RoleImpl { role: 0, slots: vec![1] }, RoleImpl { role: 4, slots: vec![3] }]),
        object("Box", vec![], vec![RoleImpl { role: 0, slots: vec![2] }]),
        object("Plain", vec![], vec![]),
        role("Labeled", vec![owned_slot("label")]),
    ];
    let functions = vec![
        caller,
        implementation("Tag.name", Type::Object(1), 0),
        implementation("Box.name", Type::Object(2), 0),
        implementation("Tag.label", Type::Object(1), 0),
    ];
    Program { objects, functions, ..Program::default() }
}

fn dispatch(targets: Vec<u32>, failures: u32) -> Dispatch {
    Dispatch { role: 0, slot: 0, failures, retained_params: vec![], targets }
}

/// `(receiver; packet) -> (str, packet)`: dispatches `name` and returns its outcome.
fn caller(receiver: Type, dispatch: Dispatch) -> Function {
    let mut f = tests::empty_function();
    f.symbol = "caller".into();
    f.entry_abi = EntryAbi::Owned;
    f.registers = vec![receiver, Type::Str, Type::Failure];
    f.services = vec![ServiceKind::Packet, ServiceKind::Packet];
    f.params = vec![register(0), service(0)];
    f.results = vec![Type::Str];
    f.blocks = vec![Block { pc: 0, params: f.params.clone() }, Block { pc: 1, params: vec![register(1), service(1)] }];
    let may_fail = dispatch.failures != 0;
    let mut outcomes = vec![Outcome { tag: Tag::Normal, edge: Edge { target_pc: 1,
        assignments: vec![produced(register(1), 0), produced(service(1), 1)] } }];
    if may_fail {
        outcomes.push(Outcome { tag: Tag::Unwind, edge: Edge { target_pc: 2,
            assignments: vec![produced(register(2), 0)] } });
        f.blocks.push(Block { pc: 2, params: vec![register(2)] });
    }
    f.proofs.entry = Some(EntryContract { failures: dispatch.failures, ..EntryContract::default() });
    f.code = vec![
        Instruction::Champ { op: Opcode::InvokeDispatchOwn, arguments: vec![Argument::Dispatch(dispatch),
            Argument::Flag(false), Argument::Location(register(0)), Argument::Location(service(0))],
            flow: Flow::Outcomes(outcomes) },
        Instruction::Champ { op: Opcode::RetOwn, arguments: vec![Argument::Location(register(1)),
            Argument::Location(service(1))], flow: Flow::Next(vec![]) },
    ];
    if may_fail {
        f.code.push(Instruction::Raise(2));
    }
    seal(&mut f);
    f
}

fn role_caller(targets: Vec<u32>, failures: u32) -> Program {
    program(caller(Type::Role(0), dispatch(targets, failures)))
}

#[test]
fn role_dispatch_loads_and_round_trips() {
    let p = role_caller(vec![1, 2], 0);
    accept(&p);
    assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
    assert_eq!(assemble(&disassemble(&p).unwrap()).unwrap(), p);
}

#[test]
fn role_types_name_role_declarations() {
    // ROLE_TYPE: a role type names a role declaration, an object type never does.
    let mut p = program(caller(Type::Role(1), dispatch(vec![1], 0)));
    reject(&p, "ROLE_TYPE");
    p = role_caller(vec![1, 2], 0);
    p.functions[0].registers.push(Type::Object(0));
    reject(&p, "ROLE_TYPE");
    p = role_caller(vec![1, 2], 0);
    p.functions[0].registers.push(Type::RoleSuccessor(3));
    reject(&p, "ROLE_TYPE");
}

#[test]
fn role_declarations_are_placeholders() {
    // ROLE_DECL: no storage, implementations or destroy; slots are plain or owning with one result at most.
    let mut p = role_caller(vec![1, 2], 0);
    p.objects[0].implements = vec![RoleImpl { role: 4, slots: vec![3] }];
    reject(&p, "ROLE_DECL");
    p = role_caller(vec![1, 2], 0);
    p.objects[4].layout = layout(vec![Type::Int]);
    p.objects[4].fields = vec![Field { name: "f".into(), storage: 0, once: false }];
    reject(&p, "ROLE_DECL");
    p = role_caller(vec![1, 2], 0);
    p.objects[4].role.as_mut().unwrap().slots[0].entry_abi = EntryAbi::Defer { labels: vec![] };
    reject(&p, "ROLE_DECL");
    p = role_caller(vec![1, 2], 0);
    p.objects[4].role.as_mut().unwrap().slots[0].results = vec![Type::Str, Type::Int];
    reject(&p, "ROLE_DECL");
}

#[test]
fn implementation_tables_match_their_slots() {
    // ROLE_IMPL: one table per role, one function per slot, and each has the slot's ABI and signature
    // after the implementing object.
    let mut p = role_caller(vec![1, 2], 0);
    p.objects[2].implements[0].slots.push(2);
    reject(&p, "ROLE_IMPL");
    p = role_caller(vec![1, 2], 0);
    p.objects[2].implements.push(RoleImpl { role: 0, slots: vec![2] });
    reject(&p, "ROLE_IMPL");
    p = role_caller(vec![1, 2], 0);
    p.objects[2].implements[0].slots = vec![1];
    reject(&p, "ROLE_IMPL");
    p = role_caller(vec![1, 2], 0);
    p.objects[0].role.as_mut().unwrap().slots[0].results = vec![Type::Int];
    reject(&p, "ROLE_IMPL");
    p = role_caller(vec![1, 2], 0);
    p.objects[0].role.as_mut().unwrap().slots[0].entry_abi = EntryAbi::Observation;
    reject(&p, "ROLE_IMPL");
    p = role_caller(vec![1, 2], 0);
    p.objects[2].implements[0].slots = vec![9];
    reject(&p, "ROLE_IMPL");
}

#[test]
fn dispatch_targets_implement_the_role() {
    // DISPATCH_SITE: distinct implementing targets, a receiver of the role and the slot's ABI.
    reject(&role_caller(vec![1, 3], 0), "DISPATCH_SITE");
    reject(&role_caller(vec![1, 1], 0), "DISPATCH_SITE");
    reject(&role_caller(vec![], 0), "DISPATCH_SITE");
    let mut wrong_role = dispatch(vec![1], 0);
    wrong_role.role = 4;
    reject(&program(caller(Type::Role(0), wrong_role)), "DISPATCH_SITE");
    let mut out_of_range = dispatch(vec![1, 2], 0);
    out_of_range.retained_params = vec![1];
    reject(&program(caller(Type::Role(0), out_of_range)), "DISPATCH_SITE");
    let mut observed = role_caller(vec![1, 2], 0);
    let Instruction::Champ { op, arguments, .. } = &mut observed.functions[0].code[0] else { unreachable!() };
    *op = Opcode::InvokeDispatch;
    arguments.retain(|argument| !matches!(argument, Argument::Flag(_) | Argument::Location(Location::Service(_))));
    reject(&observed, "DISPATCH_SITE");
}

#[test]
fn a_single_target_dispatch_may_take_its_object() {
    accept(&program(caller(Type::Object(1), dispatch(vec![1], 0))));
    reject(&program(caller(Type::Object(1), dispatch(vec![1, 2], 0))), "DISPATCH_SITE");
    reject(&program(caller(Type::Object(2), dispatch(vec![1], 0))), "DISPATCH_SITE");
}

#[test]
fn every_target_refines_the_dispatch_contract() {
    // DISPATCH_TARGET: no target fails or retains beyond the site contract.
    let mut p = role_caller(vec![1, 2], 0);
    p.functions[2].proofs.entry.as_mut().unwrap().failures = 2;
    reject(&p, "DISPATCH_TARGET");
    p = role_caller(vec![1, 2], 2);
    p.functions[2].proofs.entry.as_mut().unwrap().failures = 2;
    accept(&p);
    p = role_caller(vec![1, 2], 2);
    p.functions[2].proofs.entry.as_mut().unwrap().failures = 3;
    reject(&p, "DISPATCH_TARGET");
    p = role_caller(vec![1, 2], 0);
    p.functions[1].proofs.entry.as_mut().unwrap().retained_params = vec![0];
    reject(&p, "DISPATCH_TARGET");
    let mut retaining = dispatch(vec![1, 2], 0);
    retaining.retained_params = vec![0];
    p = program(caller(Type::Role(0), retaining));
    p.functions[1].proofs.entry.as_mut().unwrap().retained_params = vec![0];
    accept(&p);
}

#[test]
fn a_dispatch_unwind_carries_the_site_failures() {
    // A thrown-only site contract: the caller's mask must hold exactly the dispatch failures.
    let mut p = role_caller(vec![1, 2], 1);
    accept(&p);
    p.functions[0].proofs.entry.as_mut().unwrap().failures = 2;
    reject(&p, "ENTRY_FAILURE_MASK");
}

#[test]
fn an_owning_dispatch_consumes_its_packet() {
    let mut p = role_caller(vec![1, 2], 0);
    let Instruction::Champ { arguments, .. } = &mut p.functions[0].code[0] else { unreachable!() };
    arguments.pop();
    assert_eq!(verify_load(&p, &NoExternalContracts).unwrap_err().message, "caller: owning call requires input packet");
}

/// An object reaching a resource (`Res`, with destroy) and its destroy entry, added at the end.
fn with_resource(p: &mut Program) -> u32 {
    let id = p.objects.len() as u32;
    let mut resource = object("Res", vec![], vec![]);
    resource.layout.has_destroy = true;
    resource.layout.may_reach_resource = true;
    resource.layout.allocation_words = 5;
    resource.layout.identity_offset = Some(8);
    p.objects.push(resource);
    let mut destroy = tests::empty_function();
    destroy.symbol = "Res.destroy".into();
    destroy.registers = vec![Type::Object(id)];
    destroy.params = vec![register(0)];
    destroy.blocks = vec![Block { pc: 0, params: destroy.params.clone() }];
    destroy.proofs.entry = Some(EntryContract { destroy: Some(id), ..EntryContract::default() });
    seal(&mut destroy);
    p.functions.push(destroy);
    id
}

#[test]
fn plain_targets_need_an_empty_packet() {
    // CALL_PLAIN_PACKET: every target has plain inputs, so the dispatch cannot carry the caller's
    // opaque obligations (its resource parameter makes the caller's input packet opaque).
    let mut p = role_caller(vec![1, 2], 0);
    let resource = with_resource(&mut p);
    let f = &mut p.functions[0];
    f.registers.push(Type::Object(resource));
    f.params = vec![register(0), register(3), service(0)];
    f.blocks[0].params = f.params.clone();
    seal(f);
    reject(&p, "CALL_PLAIN_PACKET");
}

#[test]
fn a_plain_external_target_needs_an_empty_packet() {
    // A resource implementation makes `Named` managed, so the caller's packet is opaque and the dispatch has
    // mixed targets. A plain target with code checks its packet on entry; an external one cannot.
    let program = |external: bool| {
        let mut p = role_caller(vec![1, 2], 0);
        let resource = with_resource(&mut p);
        p.objects[resource as usize].implements = vec![RoleImpl { role: 0, slots: vec![p.functions.len() as u32] }];
        p.functions.push(implementation("Res.name", Type::Object(resource), 0));
        let Instruction::Champ { arguments, .. } = &mut p.functions[0].code[0] else { unreachable!() };
        arguments[0] = Argument::Dispatch(dispatch(vec![1, 2, resource], 0));
        seal(&mut p.functions[0]);
        if external {
            let f = &mut p.functions[2];
            f.code.clear();
            f.blocks.clear();
            f.proofs = FunctionProofs { entry: Some(EntryContract { external: Some("Box.name".into()),
                ..EntryContract::default() }), ..FunctionProofs::default() };
        }
        p
    };
    reject(&program(true), "CALL_PLAIN_PACKET");
    assert_ne!(verify_load(&program(false), &NoExternalContracts).unwrap_err().rule, "CALL_PLAIN_PACKET");
}

/// The caller returns only the packet of a dispatch whose implementations return `int`.
fn returned_packet() -> Program {
    let mut p = role_caller(vec![1, 2], 0);
    p.objects[0].role.as_mut().unwrap().slots[0].results = vec![Type::Int];
    for index in [1, 2] {
        let f = &mut p.functions[index];
        f.registers[1] = Type::Int;
        f.results = vec![Type::Int];
        f.code[0] = Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(1),
            immediate: Immediate::Constant(Constant::Int("1".into())) };
        seal(f);
    }
    let f = &mut p.functions[0];
    f.registers[1] = Type::Int;
    f.results = vec![];
    f.code[1] = Instruction::Champ { op: Opcode::RetOwn, arguments: vec![Argument::Location(service(1))],
        flow: Flow::Next(vec![]) };
    seal(f);
    p
}

#[test]
fn plain_dispatch_results_return_an_empty_packet() {
    // RETURN_PLAIN_PACKET holds: every implementation returns plain results, so the packet is empty.
    accept(&returned_packet());
}

#[test]
fn a_dispatch_packet_is_empty_only_when_every_target_returns_plain_results() {
    let p = returned_packet();
    let mut evidence = evidence::Evidence::new(&p, &p.functions[0]);
    assert_eq!(evidence.inventory(service(1)), Some(Default::default()));
    // One implementation returning a resource makes the dispatch's packet opaque.
    let mut q = returned_packet();
    let resource = with_resource(&mut q);
    q.functions[2].results = vec![Type::Object(resource)];
    let mut evidence = evidence::Evidence::new(&q, &q.functions[0]);
    assert_eq!(evidence.inventory(service(1)), None);
}

#[test]
fn a_plain_role_slot_dispatches_by_observation() {
    // A plain slot: `invoke.dispatch` passes the receiver and arguments and returns the value.
    let mut p = role_caller(vec![1, 2], 0);
    p.objects[0].role.as_mut().unwrap().slots[0].entry_abi = EntryAbi::Observation;
    for (index, receiver) in [(1, 1), (2, 2)] {
        let f = &mut p.functions[index];
        f.entry_abi = EntryAbi::Observation;
        f.services.clear();
        f.params = vec![register(0)];
        f.blocks[0].params = f.params.clone();
        f.registers[0] = Type::Object(receiver);
        f.code[1] = Instruction::Ret(vec![1]);
        seal(f);
    }
    p.objects[1].implements.pop();
    p.objects.pop();
    p.functions.truncate(3);
    let f = &mut p.functions[0];
    f.entry_abi = EntryAbi::Observation;
    f.services.clear();
    f.params = vec![register(0)];
    f.blocks = vec![Block { pc: 0, params: f.params.clone() }];
    f.code = vec![
        Instruction::Champ { op: Opcode::InvokeDispatch, arguments: vec![Argument::Dispatch(dispatch(vec![1, 2], 0)),
            Argument::Location(register(0))], flow: Flow::Next(vec![register(1)]) },
        Instruction::Ret(vec![1]),
    ];
    seal(f);
    accept(&p);
    let Instruction::Champ { op, .. } = &mut p.functions[0].code[0] else { unreachable!() };
    *op = Opcode::InvokeDispatchOwn;
    reject(&p, "DISPATCH_SITE");
}

/// `main` with `registers` and `code` before its `ret`, failing with `failures`.
fn plain_main(registers: Vec<Type>, code: Vec<Instruction>, failures: u32) -> Program {
    let mut f = tests::empty_function();
    f.registers = registers;
    f.code = code;
    f.code.push(Instruction::Ret(vec![]));
    f.proofs.entry = Some(EntryContract { failures, ..EntryContract::default() });
    seal(&mut f);
    let mut p = role_caller(vec![1, 2], 0);
    p.functions[0] = f;
    p
}

fn undef(output: u32) -> Instruction {
    Instruction::Data { op: DataOp::Undef, inputs: vec![], output: Some(output), immediate: Immediate::None }
}

fn new(object: u32, output: u32) -> Instruction {
    Instruction::Data { op: DataOp::ObjNew, inputs: vec![], output: Some(output), immediate: Immediate::Object(object) }
}

fn data(op: DataOp, input: u32, output: u32) -> Instruction {
    Instruction::Data { op, inputs: vec![input], output: Some(output), immediate: Immediate::None }
}

#[test]
fn role_wrap_needs_an_implementing_object() {
    // ROLE_WRAP: a move into a role starts from an object that implements it.
    let wrap = |object, role| plain_main(vec![Type::Object(object), Type::Role(role)],
        vec![new(object, 0), data(DataOp::Move, 0, 1)], 0);
    accept(&wrap(1, 0));
    accept(&wrap(1, 4));
    reject(&wrap(2, 4), "ROLE_WRAP");
    reject(&wrap(3, 0), "ROLE_WRAP");
}

#[test]
fn role_narrow_converts_between_roles_and_can_fail() {
    // ROLE_NARROW: a role value narrows to another role; the data form fails like a program failure.
    let narrow = |from, to, failures| plain_main(vec![Type::Object(1), from, to],
        vec![new(1, 0), data(DataOp::Move, 0, 1), data(DataOp::RoleNarrow, 1, 2)], failures);
    accept(&narrow(Type::Role(0), Type::Role(4), 2));
    reject(&narrow(Type::Role(0), Type::Role(4), 0), "ENTRY_FAILURE_MASK");
    reject(&narrow(Type::Object(1), Type::Role(4), 2), "ROLE_NARROW");
    reject(&narrow(Type::Role(0), Type::Object(1), 2), "ROLE_NARROW");
}

#[test]
fn a_role_successor_keeps_its_must_use_permission() {
    // obj.next makes ^Tag; role.wrap moves its permission into ^Named; succ.take discharges it.
    let successor = |code| plain_main(vec![Type::Object(1), Type::Successor(1), Type::RoleSuccessor(0),
        Type::Role(0), Type::Role(0)], code, 0);
    let next = Instruction::Data { op: DataOp::ObjNext, inputs: vec![0], output: Some(1),
        immediate: Immediate::Updates(vec![]) };
    accept(&successor(vec![new(1, 0), next.clone(), data(DataOp::Move, 1, 2), data(DataOp::Move, 2, 3)]));
    let failure = verify_load(&successor(vec![new(1, 0), next.clone(), data(DataOp::Move, 1, 2)]),
        &NoExternalContracts).unwrap_err();
    assert!(failure.message.contains("return loses successor must-use permission"), "{failure}");
    let failure = verify_load(&successor(vec![new(1, 0), next.clone(), data(DataOp::Move, 1, 2),
        data(DataOp::Move, 2, 3), data(DataOp::Move, 2, 4)]), &NoExternalContracts).unwrap_err();
    assert!(failure.message.contains("successor permission already taken"), "{failure}");
    reject(&successor(vec![new(1, 0), next, data(DataOp::Move, 1, 2), data(DataOp::Move, 2, 0)]), "SPB_STRUCTURE");
}

#[test]
fn an_object_successor_permission_is_spent_once_in_spb() {
    // The duplicate take stays a loader error: codegen maps a second OIR succ.take to the first one's object.
    let next = Instruction::Data { op: DataOp::ObjNext, inputs: vec![0], output: Some(1),
        immediate: Immediate::Updates(vec![]) };
    let takes = |count: u32| plain_main(vec![Type::Object(1), Type::Successor(1), Type::Object(1), Type::Object(1)],
        [new(1, 0), next.clone()].into_iter().chain((0..count).map(|index| data(DataOp::Move, 1, 2 + index)))
            .collect(), 0);
    accept(&takes(1));
    let failure = verify_load(&takes(2), &NoExternalContracts).unwrap_err();
    assert!(failure.message.contains("successor permission already taken"), "{failure}");
}

#[test]
fn role_values_compare_within_one_role() {
    let compare = |right| plain_main(vec![Type::Object(1), Type::Role(0), right, Type::Bool], vec![
        new(1, 0), data(DataOp::Move, 0, 1), data(DataOp::Move, 0, 2),
        Instruction::Data { op: DataOp::ValEq, inputs: vec![1, 2], output: Some(3), immediate: Immediate::None },
    ], 2);
    accept(&compare(Type::Role(0)));
    reject(&compare(Type::Role(4)), "VAL_EQ_TYPE");
}

#[test]
fn role_arrays_and_fields_are_closed_plain() {
    // ARRAY_PLAIN: a role array is plain only when every implementation is closed plain data.
    let array = Type::Array(ArrayType { leaf: ArrayLeaf::Role(0), depth: 1 });
    let mut p = plain_main(vec![array], vec![undef(0)], 0);
    p.functions[0].code = vec![Instruction::Data { op: DataOp::ArrayNew, inputs: vec![], output: Some(0),
        immediate: Immediate::None }, Instruction::Ret(vec![])];
    seal(&mut p.functions[0]);
    accept(&p);
    let resource = with_resource(&mut p);
    p.objects[resource as usize].implements = vec![RoleImpl { role: 0, slots: vec![2] }];
    // The implementation must take the resource object; reuse Box.name's body under a new receiver.
    let mut name = p.functions[2].clone();
    name.registers[0] = Type::Object(resource);
    seal(&mut name);
    p.objects[resource as usize].implements[0].slots = vec![p.functions.len() as u32];
    p.functions.push(name);
    reject(&p, "ARRAY_PLAIN");
}

#[test]
fn role_fields_are_traced_and_never_understate_resources() {
    let mut p = role_caller(vec![1, 2], 0);
    p.objects.push(object("Holder", vec![Type::Role(0)], vec![]));
    accept(&p);
    let holder = p.objects.len() - 1;
    p.objects[holder].layout.trace_layout.clear();
    assert!(verify_load(&p, &NoExternalContracts).unwrap_err().message.contains("incorrect object trace layout"));
    let mut q = role_caller(vec![1, 2], 0);
    q.objects.push(object("Holder", vec![Type::Role(0)], vec![]));
    let resource = with_resource(&mut q);
    let mut name = q.functions[2].clone();
    name.registers[0] = Type::Object(resource);
    seal(&mut name);
    q.objects[resource as usize].implements = vec![RoleImpl { role: 0, slots: vec![q.functions.len() as u32] }];
    q.functions.push(name);
    let failure = verify_load(&q, &NoExternalContracts).unwrap_err();
    assert!(failure.message.contains("understated object retention metadata"), "{failure}");
}
