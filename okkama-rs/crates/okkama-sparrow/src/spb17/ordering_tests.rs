//! `ordering()` (#678): `array.order` sorts scalars and text into their own type; `array.order.dispatch` sorts
//! elements by a `compare.Order` slot through its dispatch, with the dispatch's contract.
use super::*;

fn reject(p: &Program, rule: &str) {
    let failure = verify_load(p, &NoExternalContracts).unwrap_err();
    assert_eq!(failure.rule, rule, "{failure}");
}

fn accept(p: &Program) {
    verify_load(p, &NoExternalContracts).unwrap_or_else(|failure| panic!("{failure}"));
}

fn round_trip(p: &Program) {
    assert_eq!(decode(&encode(p).unwrap()).unwrap(), *p);
    assert_eq!(assemble(&disassemble(p).unwrap()).unwrap(), *p);
}

fn register(id: u32) -> Location {
    Location::Register(id)
}

fn array(leaf: ArrayLeaf) -> Type {
    Type::Array(ArrayType { leaf, depth: 1 })
}

/// `(array) -> result`: `array.order` of its parameter, returned.
fn natural(input: Type, output: Type) -> Program {
    let mut f = tests::empty_function();
    f.registers = vec![input, output];
    f.params = vec![register(0)];
    f.results = vec![output];
    f.blocks = vec![Block { pc: 0, params: vec![register(0)] }];
    f.code = vec![
        Instruction::Data { op: DataOp::ArrayOrder, inputs: vec![0], output: Some(1), immediate: Immediate::None },
        Instruction::Ret(vec![1]),
    ];
    if let (Ok(states), Ok(live)) = (service_states(&f), live_locations(&f)) {
        f.proofs.states = states;
        f.proofs.roots = live.iter().enumerate().map(|(pc, locations)| RootRequirement {
            pc: pc as u32, locations: locations.iter().copied().collect(),
        }).collect();
    }
    Program { functions: vec![f], ..Program::default() }
}

#[test]
fn array_order_sorts_scalars_and_text_into_their_type() {
    for ty in [array(ArrayLeaf::Int), array(ArrayLeaf::Float), array(ArrayLeaf::Rat), array(ArrayLeaf::Char),
        array(ArrayLeaf::Str), Type::Bytes]
    {
        let p = natural(ty, ty);
        accept(&p);
        round_trip(&p);
        // A total operation: its data form needs no outcome even beside services.
        assert!(!DataOp::ArrayOrder.fallible(&[ty], Some(ty)));
    }
}

#[test]
fn array_order_needs_a_sortable_array_and_its_own_type() {
    // ARRAY_ORDER_TYPE: bools, objects, nested arrays and maps have no natural order; the result keeps the type.
    let nested = Type::Array(ArrayType { leaf: ArrayLeaf::Int, depth: 2 });
    for ty in [array(ArrayLeaf::Bool), nested, array(ArrayLeaf::Range), Type::Str, Type::Int] {
        reject(&natural(ty, ty), "ARRAY_ORDER_TYPE");
    }
    reject(&natural(array(ArrayLeaf::Int), array(ArrayLeaf::Float)), "ARRAY_ORDER_TYPE");
    reject(&natural(Type::Bytes, array(ArrayLeaf::Int)), "ARRAY_ORDER_TYPE");
    let mut two = natural(array(ArrayLeaf::Int), array(ArrayLeaf::Int));
    let Instruction::Data { inputs, .. } = &mut two.functions[0].code[0] else { unreachable!() };
    inputs.push(0);
    reject(&two, "ARRAY_ORDER_TYPE");
}

fn layout(storage_fields: Vec<Type>) -> ObjectRuntimeLayout {
    let count = storage_fields.len() as u32;
    ObjectRuntimeLayout {
        storage_fields, allocation_words: (1 + count).max(2), identity_offset: None, has_destroy: false,
        may_reach_resource: false, may_async_destroy: false, trace_layout: vec![], selection_layout: vec![],
    }
}

fn object(name: &str, storage_fields: Vec<Type>, implements: Vec<RoleImpl>) -> Object {
    let count = storage_fields.len() as u32;
    Object {
        name: name.into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle: false, role: None,
        implements, any_box: false,
        fields: (0..count).map(|storage| Field { name: format!("f{storage}"), storage, once: false }).collect(),
        layout: layout(storage_fields),
    }
}

const ROLE: u32 = 0;
const CARD: u32 = 1;
const ORDER: u32 = 2;

/// `Card.order(other) -> Order`, owning: returns `Order.greater`, or may fail with `failures`.
fn order_implementation(failures: u32) -> Function {
    let mut f = tests::empty_function();
    f.symbol = "Card.order".into();
    f.entry_abi = EntryAbi::Owned;
    f.registers = vec![Type::Object(CARD), Type::Object(CARD), Type::Object(ORDER)];
    f.services = vec![ServiceKind::Packet];
    f.params = vec![register(0), register(1), Location::Service(0)];
    f.results = vec![Type::Object(ORDER)];
    f.blocks = vec![Block { pc: 0, params: f.params.clone() }];
    f.code = vec![
        Instruction::Data { op: DataOp::EnumCase, inputs: vec![], output: Some(2),
            immediate: Immediate::Updates(vec![ORDER, 2]) },
        Instruction::Champ { op: Opcode::RetOwn, arguments: vec![Argument::Location(register(2)),
            Argument::Location(Location::Service(0))], flow: Flow::Next(vec![]) },
    ];
    f.proofs.entry = Some(EntryContract { failures, ..EntryContract::default() });
    seal(&mut f);
    f
}

/// Live locations are roots before every instruction, and after every helper with a next pc.
fn seal(f: &mut Function) {
    f.proofs.states = service_states(f).unwrap();
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

fn dispatch(failures: u32) -> Dispatch {
    Dispatch { role: ROLE, slot: 0, failures, retained_params: vec![], targets: vec![CARD] }
}

/// `(elements) -> elements`: sorts its array of `element` by the dispatch; a failing sort raises.
fn sorter(element: Type, dispatch: Dispatch) -> Function {
    let ty = ArrayType::of(element).unwrap();
    let mut f = tests::empty_function();
    f.symbol = "sort".into();
    f.registers = vec![ty, ty, Type::Failure];
    f.params = vec![register(0)];
    f.results = vec![ty];
    f.blocks = vec![Block { pc: 0, params: vec![register(0)] }];
    let may_fail = dispatch.failures != 0;
    let failures = dispatch.failures;
    let arguments = vec![Argument::Dispatch(dispatch), Argument::Location(register(0))];
    if may_fail {
        f.blocks.extend([Block { pc: 1, params: vec![register(1)] }, Block { pc: 2, params: vec![register(2)] }]);
        let outcome = |tag, target_pc, destination| Outcome { tag, edge: Edge { target_pc,
            assignments: vec![Assignment { destination: register(destination), source: Source::ProducedSlot(0) }] } };
        f.code = vec![
            Instruction::Champ { op: Opcode::OrderDispatch, arguments,
                flow: Flow::Outcomes(vec![outcome(Tag::Normal, 1, 1), outcome(Tag::Unwind, 2, 2)]) },
            Instruction::Ret(vec![1]),
            Instruction::Raise(2),
        ];
        f.proofs.entry = Some(EntryContract { failures, ..EntryContract::default() });
    } else {
        f.code = vec![
            Instruction::Champ { op: Opcode::OrderDispatch, arguments, flow: Flow::Next(vec![register(1)]) },
            Instruction::Ret(vec![1]),
        ];
    }
    seal(&mut f);
    f
}

/// Objects: 0 role `Comparing<Card>` (`order(Card) -> Order`, owning), 1 `Card` implementing it with
/// function 1, 2 the plain enum `Order` (less, equal, greater), 3 `Plain`. Function 0 is the sorter.
fn program(sorter: Function, failures: u32) -> Program {
    let slot = RoleSlot { name: "order".into(), entry_abi: EntryAbi::Owned, params: vec![Type::Object(CARD)],
        results: vec![Type::Object(ORDER)] };
    let objects = vec![
        Object { role: Some(RoleDecl { slots: vec![slot] }), ..object("compare::Comparing<Card>", vec![], vec![]) },
        object("Card", vec![Type::Int], vec![RoleImpl { role: ROLE, slots: vec![1] }]),
        Object { enum_cases: 3, fields: vec![], ..object("compare::Order", vec![Type::Int], vec![]) },
        object("Plain", vec![], vec![]),
    ];
    Program { objects, functions: vec![sorter, order_implementation(failures)], ..Program::default() }
}

fn sorting(failures: u32) -> Program {
    program(sorter(Type::Object(CARD), dispatch(failures)), failures)
}

#[test]
fn order_dispatch_loads_and_round_trips() {
    for failures in [0, 1, 3] {
        let p = sorting(failures);
        accept(&p);
        round_trip(&p);
    }
    // The elements may be role values: every implementation of the role is a receiver.
    let mut p = sorting(0);
    p.objects[ROLE as usize].role.as_mut().unwrap().slots[0].params = vec![Type::Role(ROLE)];
    p.functions[0] = sorter(Type::Role(ROLE), dispatch(0));
    p.functions[1].registers[1] = Type::Role(ROLE);
    seal(&mut p.functions[1]);
    accept(&p);
}

#[test]
fn order_dispatch_compares_its_elements_by_an_order_slot() {
    // ORDER_DISPATCH: the elements are the dispatch's receivers, the slot takes one element and returns a
    // three-case plain enum, and a comparison retains no element.
    reject(&program(sorter(Type::Object(3), dispatch(0)), 0), "ORDER_DISPATCH");
    reject(&program(sorter(Type::Role(ROLE), dispatch(0)), 0), "ORDER_DISPATCH");
    let mut p = sorting(0);
    p.objects[ORDER as usize].enum_cases = 4;
    reject(&p, "ORDER_DISPATCH");
    let mut retaining = dispatch(0);
    retaining.retained_params = vec![1];
    reject(&program(sorter(Type::Object(CARD), retaining), 0), "ORDER_DISPATCH");
    // The operand is an array, not one element.
    let mut p = sorting(0);
    p.functions[0].registers[0] = Type::Object(CARD);
    p.functions[0].params = vec![register(0)];
    reject(&p, "ORDER_DISPATCH");
    // The dispatch itself is checked as for invoke.dispatch.
    let mut wide = dispatch(0);
    wide.targets = vec![CARD, 3];
    reject(&program(sorter(Type::Object(CARD), wide), 0), "DISPATCH_SITE");
}

#[test]
fn order_dispatch_targets_refine_its_contract() {
    // DISPATCH_TARGET: no comparison fails beyond the dispatch's failures.
    let mut p = sorting(0);
    p.functions[1] = order_implementation(1);
    reject(&p, "DISPATCH_TARGET");
    let mut p = sorting(1);
    p.functions[1] = order_implementation(3);
    reject(&p, "DISPATCH_TARGET");
}

#[test]
fn a_failing_sort_fails_with_the_dispatch_failures() {
    // ENTRY_FAILURE_MASK: the unwind outcome carries the dispatch's failures (a thrown error here), which the
    // entry must declare, and nothing beyond them.
    let p = sorting(1);
    accept(&p);
    let mut p = sorting(1);
    p.functions[0].proofs.entry.as_mut().unwrap().failures = 0;
    reject(&p, "ENTRY_FAILURE_MASK");
    let mut p = sorting(1);
    p.functions[0].proofs.entry.as_mut().unwrap().failures = 2;
    reject(&p, "ENTRY_FAILURE_MASK");
}
