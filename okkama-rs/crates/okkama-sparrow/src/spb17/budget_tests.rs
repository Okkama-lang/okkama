//! Work and memory budgets of the load gate and the allocator (#654). Each budget test builds an input
//! just past one bound and checks that exactly that bound reports; positive controls load the same shape
//! below the bound.
use super::*;
use super::load_tests::{Catalog, fixture, main, refresh, small};
use super::rules_tests::plain_object;
use super::tests::empty_function;

const STORAGE: usize = 8 * 1024 * 1024;

fn reject_budget(p: &Program, catalog: &dyn ExternalContracts, code: &str, message: &str) {
    let failure = verify_load(p, catalog).unwrap_err();
    assert_eq!(failure.rule, code, "{failure}");
    assert!(failure.message.contains(message), "{failure}");
}

fn expect_budget(result: Result<(), Error>, code: &str, message: &str) {
    let failure = result.unwrap_err();
    assert_eq!(failure.rule, code, "{failure}");
    assert!(failure.message.contains(message), "{failure}");
}

fn constant(output: u32) -> Instruction {
    Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(output),
        immediate: Immediate::Constant(Constant::Int("0".into())) }
}

fn champ(op: Opcode, arguments: Vec<Argument>, outputs: Vec<Location>) -> Instruction {
    Instruction::Champ { op, arguments, flow: Flow::Next(outputs) }
}

fn service(id: u32) -> Argument {
    Argument::Location(Location::Service(id))
}

fn branch(target_pc: u32) -> Instruction {
    Instruction::Br(Edge { target_pc, assignments: vec![] })
}

// ---- LOAD_WORK_BUDGET: the function-size bound checked before verification ----

const FUNCTION_BUDGET: &str = "function exceeds verification work budget";

/// Straight-line constants into fresh registers, then ret.
fn constants(count: u32) -> Function {
    let mut f = empty_function();
    f.registers = vec![Type::Int; count as usize];
    f.code = (0..count).map(constant).collect();
    f.code.push(Instruction::Ret(vec![]));
    refresh(&mut f);
    f
}

#[test]
fn function_at_the_code_bound_loads() {
    verify_load(&small(constants(65_535)), &NoExternalContracts).unwrap();
}

#[test]
fn code_longer_than_the_bound_is_rejected() {
    reject_budget(&small(constants(65_536)), &NoExternalContracts, "LOAD_WORK_BUDGET", FUNCTION_BUDGET);
}

#[test]
fn locations_beyond_the_bound_are_rejected() {
    let mut f = empty_function();
    f.registers = vec![Type::Int; STORAGE / 2];
    f.services = vec![ServiceKind::Deps; STORAGE / 2 + 1];
    reject_budget(&small(f), &NoExternalContracts, "LOAD_WORK_BUDGET", FUNCTION_BUDGET);
}

#[test]
fn dominator_storage_beyond_the_bound_is_rejected() {
    // 23_171 blocks need 23_171 * 363 dominator words, just above 8 Mi.
    let mut f = empty_function();
    f.blocks = (0..23_171).map(|pc| Block { pc, params: vec![] }).collect();
    reject_budget(&small(f), &NoExternalContracts, "LOAD_WORK_BUDGET", FUNCTION_BUDGET);
}

#[test]
fn root_requirements_beyond_the_bound_are_rejected() {
    let mut f = empty_function();
    f.registers = vec![Type::Str];
    let half = vec![Location::Register(0); STORAGE / 2];
    f.proofs.roots.push(RootRequirement { pc: 0, locations: half.clone() });
    f.proofs.post_roots.push(RootRequirement { pc: 0, locations: [half, vec![Location::Register(0)]].concat() });
    reject_budget(&small(f), &NoExternalContracts, "LOAD_WORK_BUDGET", FUNCTION_BUDGET);
}

// ---- LOAD_WORK_BUDGET: service-state storage ----

/// An owning function that keeps `packets` packets live across `idle` constants, then joins and returns
/// them. Its input packet is the first of them; the rest start empty.
fn idle_packets(packets: u32, idle: u32) -> Function {
    let mut f = empty_function();
    f.entry_abi = EntryAbi::Owned;
    f.registers = vec![Type::Int; idle as usize];
    f.services = vec![ServiceKind::Packet; packets as usize + 1];
    f.services.push(ServiceKind::Deps);
    f.params = vec![Location::Service(0)];
    f.blocks[0].params = f.params.clone();
    f.proofs.entry = Some(EntryContract::default());
    f.code = (1..packets).map(|id| champ(Opcode::OwnEmpty, vec![], vec![Location::Service(id)])).collect();
    f.code.extend((0..idle).map(constant));
    f.code.push(champ(Opcode::OwnJoin, (0..packets).map(service).collect(), vec![Location::Service(packets)]));
    f.code.push(champ(Opcode::DepsEmpty, vec![], vec![Location::Service(packets + 1)]));
    f.code.push(champ(Opcode::RetOwn, vec![service(packets), service(packets + 1)], vec![]));
    f
}

#[test]
fn live_packets_below_the_state_bound_load() {
    let mut f = idle_packets(64, 16);
    refresh(&mut f);
    verify_load(&small(f), &NoExternalContracts).unwrap();
}

#[test]
fn service_state_storage_beyond_the_bound_is_rejected() {
    // 1024 packets live at 8200 constants store more than 8 Mi state entries. The state and root maps
    // stay empty: the reconstruction stops at its bound before they are compared.
    let f = idle_packets(1024, 8200);
    reject_budget(&small(f), &NoExternalContracts, "LOAD_WORK_BUDGET",
        "service-state storage exceeds verification work budget");
}

// ---- LOAD_WORK_BUDGET: successor permission storage ----

/// `successors` successor parameters of a plain object pass through a chain of `blocks` blocks, where
/// each permission is taken. The root and state maps are left to the caller.
fn successor_chain(successors: u32, blocks: u32) -> Program {
    let mut f = empty_function();
    f.registers = vec![Type::Successor(0); successors as usize];
    f.registers.extend(vec![Type::Object(0); successors as usize]);
    f.params = (0..successors).map(Location::Register).collect();
    f.blocks = vec![Block { pc: 0, params: f.params.clone() }];
    f.blocks.extend((1..blocks).map(|pc| Block { pc, params: vec![] }));
    f.code = (1..blocks).map(branch).collect();
    f.code.extend((0..successors).map(|id| Instruction::Data { op: DataOp::Move, inputs: vec![id],
        output: Some(successors + id), immediate: Immediate::None }));
    f.code.push(Instruction::Ret(vec![]));
    f.proofs.entry = Some(EntryContract::default());
    Program { objects: vec![plain_object(vec![])], ..small(f) }
}

#[test]
fn successor_permissions_below_the_bound_load() {
    let mut p = successor_chain(64, 16);
    refresh(&mut p.functions[0]);
    verify_load(&p, &NoExternalContracts).unwrap();
}

#[test]
fn successor_permission_storage_beyond_the_bound_is_rejected() {
    // 1024 permissions stored at each of 8200 blocks; verification stops before the root maps are read.
    reject_budget(&successor_chain(1024, 8200), &NoExternalContracts, "LOAD_WORK_BUDGET",
        "successor permission storage exceeds verification work budget");
}

// ---- LOAD_WORK_BUDGET: the destroy descriptor closure ----

/// valued's main takes one more parameter: a plain object with `fields` int fields. Main's merge closes
/// must reconstruct the descriptor closure of its parameters, field by field.
fn wide_parameter(fields: usize) -> (Program, Catalog) {
    let (mut p, catalog) = fixture("valued");
    let mut wide = plain_object(vec![]);
    wide.layout.storage_fields = vec![Type::Int; fields];
    wide.layout.allocation_words = 1 + fields as u32;
    p.objects.push(wide);
    let f = main(&mut p);
    let param = Location::Register(f.registers.len() as u32);
    f.registers.push(Type::Object(1));
    f.params.insert(1, param);
    f.blocks[0].params = f.params.clone();
    refresh(f);
    (p, catalog)
}

#[test]
fn descriptor_closure_below_the_bound_loads() {
    let (p, catalog) = wide_parameter(1000);
    verify_load(&p, &catalog).unwrap();
}

#[test]
fn descriptor_type_closure_beyond_the_bound_is_rejected() {
    let (p, catalog) = wide_parameter(1_000_000);
    reject_budget(&p, &catalog, "LOAD_WORK_BUDGET", "destroy type closure exceeds work budget");
}

// ---- LOAD_WORK_BUDGET: evidence resolution ----

/// An owning function beside fixture valued's `Res { label: str }`. It commits a created object into one
/// of `width` packets, renames all of them through `depth` blocks and returns their join. Resolving the
/// joined inventory visits width * depth renames, each a separate step, at a frame depth of about depth.
fn wide_renames(width: u32, depth: u32) -> (Program, Catalog) {
    let (mut p, catalog) = fixture("valued");
    let mut f = empty_function();
    f.symbol = "renames".into();
    f.entry_abi = EntryAbi::Owned;
    f.registers = vec![Type::Str, Type::Object(0)];
    f.results = vec![Type::Object(0)];
    let layer = |level: u32, index: u32| Location::Service(level * width + index);
    f.services = vec![ServiceKind::Packet; (width * (depth + 1) + 1) as usize];
    f.services.push(ServiceKind::Deps);
    let joined = width * (depth + 1);
    f.params = vec![Location::Register(0), layer(0, 0)];
    f.proofs.entry = Some(EntryContract::default());
    f.code = vec![
        Instruction::Data { op: DataOp::ObjNew, inputs: vec![0], output: Some(1), immediate: Immediate::Object(0) },
        champ(Opcode::OwnCreateCommit,
            vec![Argument::Location(Location::Register(1)), Argument::Object(0), Argument::Boundary(0)],
            vec![layer(0, 1)]),
    ];
    f.code.extend((2..width).map(|index| champ(Opcode::OwnEmpty, vec![], vec![layer(0, index)])));
    f.blocks = vec![Block { pc: 0, params: f.params.clone() }];
    for level in 1..=depth {
        let target_pc = f.code.len() as u32 + 1;
        let assignments = (0..width).map(|index| Assignment {
            destination: layer(level, index), source: Source::Location(layer(level - 1, index)),
        }).collect();
        f.code.push(Instruction::Br(Edge { target_pc, assignments }));
        f.blocks.push(Block { pc: target_pc, params: (0..width).map(|index| layer(level, index)).collect() });
    }
    let last = (0..width).map(|index| Argument::Location(layer(depth, index))).collect();
    f.code.push(champ(Opcode::OwnJoin, last, vec![Location::Service(joined)]));
    f.code.push(champ(Opcode::DepsEmpty, vec![], vec![Location::Service(joined + 1)]));
    f.code.push(champ(Opcode::RetOwn,
        vec![Argument::Location(Location::Register(1)), service(joined), service(joined + 1)], vec![]));
    refresh(&mut f);
    p.functions.push(f);
    (p, catalog)
}

#[test]
fn shallow_wide_renames_load() {
    let (p, catalog) = wide_renames(64, 4);
    verify_load(&p, &catalog).unwrap();
}

/// 64 packets renamed through 8000 blocks: about 1.02 M evidence steps, far below the 65_536-frame depth
/// bound and the code bound, so only the step budget can stop the resolution.
fn step_exhausting_renames() -> (Program, Catalog) {
    let (p, catalog) = wide_renames(64, 8000);
    assert!(p.functions.last().unwrap().code.len() <= 65_536);
    (p, catalog)
}

#[test]
fn hundred_thousand_moves_are_bounded_without_recursion() {
    let (p, _) = step_exhausting_renames();
    let f = p.functions.last().unwrap();
    let Instruction::Champ { flow: Flow::Next(outputs), .. } = &f.code[f.code.len() - 3] else { panic!("join") };
    let mut evidence = evidence::Evidence::new(&p, f);
    assert_eq!(evidence.inventory(outputs[0]), None);
    expect_budget(evidence.check_budget(), "LOAD_WORK_BUDGET", "evidence resolution exceeds step/depth budget");
}

#[test]
fn exhausted_evidence_budget_is_reported_as_the_budget() {
    let (p, catalog) = step_exhausting_renames();
    reject_budget(&p, &catalog, "LOAD_WORK_BUDGET", "evidence resolution exceeds step/depth budget");
}

#[test]
fn move_chain_deeper_than_the_frame_bound_is_unresolved() {
    let mut f = empty_function();
    f.registers = vec![Type::Object(0); 70_001];
    f.params = vec![Location::Register(0)];
    f.blocks[0].params = f.params.clone();
    f.code = (1..=70_000).map(|id| Instruction::Data { op: DataOp::Move, inputs: vec![id - 1],
        output: Some(id), immediate: Immediate::None }).collect();
    f.code.push(Instruction::Ret(vec![]));
    let p = Program { objects: vec![plain_object(vec![])], ..small(f) };
    let mut evidence = evidence::Evidence::new(&p, &p.functions[0]);
    assert_eq!(evidence.identity(Location::Register(70_000)), None);
    assert_eq!(evidence.check_budget().unwrap_err().rule, "LOAD_WORK_BUDGET");
}

// ---- LOAD_WORK_BUDGET: rules decided from evidence report the budget, not themselves ----

/// The fresh singleton `fresh` of fixture fresh-call and the location of its returned value and packet.
fn fresh_function() -> (Program, Location, Location) {
    let (p, _) = fixture("fresh-call");
    let f = p.functions.iter().find(|f| f.symbol == "fresh").unwrap();
    let returned = f.code.iter().find_map(|instruction| match instruction {
        Instruction::Champ { op: Opcode::RetOwn, arguments, .. } => Some(arguments.clone()),
        _ => None,
    }).unwrap();
    let [Argument::Location(value), Argument::Location(packet), ..] = returned[..] else { panic!("ret.own") };
    (p, value, packet)
}

fn fresh(p: &Program) -> &Function {
    p.functions.iter().find(|f| f.symbol == "fresh").unwrap()
}

#[test]
fn fresh_return_holds_with_its_evidence() {
    let (p, _, _) = fresh_function();
    let f = fresh(&p);
    load::check_fresh(f, &mut evidence::Evidence::new(&p, f)).unwrap();
}

#[test]
fn fresh_packet_must_hold_exactly_the_returned_identity() {
    let (mut p, _, packet) = fresh_function();
    let f = p.functions.iter_mut().find(|f| f.symbol == "fresh").unwrap();
    // Return the empty packet made before the creation instead of the one the commit joined into.
    let empty = f.code.iter().find_map(|instruction| match instruction {
        Instruction::Champ { op: Opcode::OwnEmpty, flow: Flow::Next(outputs), .. } => Some(outputs[0]),
        _ => None,
    }).unwrap();
    for instruction in &mut f.code {
        if let Instruction::Champ { op: Opcode::RetOwn, arguments, .. } = instruction {
            arguments[1] = Argument::Location(empty);
        }
    }
    assert_ne!(empty, packet);
    let f = fresh(&p);
    let failure = load::check_fresh(f, &mut evidence::Evidence::new(&p, f)).unwrap_err();
    assert_eq!(failure.rule, "FRESH_PACKET", "{failure}");
}

#[test]
fn fresh_origin_beyond_the_evidence_budget_reports_the_budget() {
    let (p, _, _) = fresh_function();
    let f = fresh(&p);
    let mut evidence = evidence::Evidence::new(&p, f);
    evidence.spend_budget();
    let failure = load::check_fresh(f, &mut evidence).unwrap_err();
    assert_eq!(failure.rule, "LOAD_WORK_BUDGET", "{failure}");
}

#[test]
fn fresh_packet_beyond_the_evidence_budget_reports_the_budget() {
    let (p, value, _) = fresh_function();
    let f = fresh(&p);
    let mut evidence = evidence::Evidence::new(&p, f);
    // The returned identity resolves within the budget; the packet's inventory does not.
    assert!(evidence.identity(value).is_some());
    evidence.spend_budget();
    let failure = load::check_fresh(f, &mut evidence).unwrap_err();
    assert_eq!(failure.rule, "LOAD_WORK_BUDGET", "{failure}");
}

#[test]
fn creation_commit_beyond_the_evidence_budget_reports_the_budget() {
    let (p, _, _) = fresh_function();
    let f = fresh(&p);
    load::check_creations_with(&p, f, &mut evidence::Evidence::new(&p, f)).unwrap();
    let mut evidence = evidence::Evidence::new(&p, f);
    evidence.spend_budget();
    let failure = load::check_creations_with(&p, f, &mut evidence).unwrap_err();
    assert_eq!(failure.rule, "LOAD_WORK_BUDGET", "{failure}");
}

/// `main` of fixture fresh-call, whose first close runs a direct plan of two identities, with the destroy
/// failure masks of its closes.
fn direct_plan() -> (Program, Vec<u32>) {
    let (p, _) = fixture("fresh-call");
    let f = p.functions.iter().find(|f| f.symbol == "main").unwrap();
    assert!(f.proofs.cleanup_plans.iter().any(|plan| plan.slots.len() == 2), "fixture changed");
    let mut evidence = evidence::Evidence::new(&p, f);
    let masks = (0..f.proofs.closes.len() as u32)
        .map(|close| load::visible_destroy_failures(&p, f, close, &mut evidence).unwrap()).collect();
    (p, masks)
}

#[test]
fn direct_plan_holds_with_its_evidence() {
    let (p, masks) = direct_plan();
    let f = main_of(&p);
    load::check_plans(&p, f, &mut evidence::Evidence::new(&p, f), &masks).unwrap();
}

fn main_of(p: &Program) -> &Function {
    p.functions.iter().find(|f| f.symbol == "main").unwrap()
}

#[test]
fn direct_identity_origin_beyond_the_evidence_budget_reports_the_budget() {
    let (p, masks) = direct_plan();
    let f = main_of(&p);
    let mut evidence = evidence::Evidence::new(&p, f);
    evidence.spend_budget();
    let failure = load::check_plans(&p, f, &mut evidence, &masks).unwrap_err();
    assert_eq!(failure.rule, "LOAD_WORK_BUDGET", "{failure}");
}

#[test]
fn direct_owner_inventory_beyond_the_evidence_budget_reports_the_budget() {
    let (p, masks) = direct_plan();
    let f = main_of(&p);
    let mut evidence = evidence::Evidence::new(&p, f);
    // Every slot identity resolves within the budget; the closed owner's inventory does not.
    for plan in &f.proofs.cleanup_plans {
        for slot in &plan.slots {
            assert!(evidence.identity(slot.identity).is_some());
        }
    }
    evidence.spend_budget();
    let failure = load::check_plans(&p, f, &mut evidence, &masks).unwrap_err();
    assert_eq!(failure.rule, "LOAD_WORK_BUDGET", "{failure}");
}

// ---- ALLOCATION_WORK and ALLOCATION_MEMORY ----

/// ret of `used` int registers at pc 0, reached by a chain of `chain` branches that each jump one
/// instruction back: each backward pass of the liveness fixed point moves the live set one branch further.
fn backward_chain(used: u32, chain: u32) -> Function {
    let mut f = empty_function();
    f.registers = vec![Type::Int; used as usize];
    f.code = vec![Instruction::Ret((0..used).collect())];
    f.code.extend((1..=chain).map(|pc| branch(pc - 1)));
    f
}

#[test]
fn liveness_converges_below_the_work_bound() {
    let live = live_locations(&backward_chain(100, 100)).unwrap();
    assert!(live.iter().all(|set| set.len() == 100));
}

#[test]
fn liveness_of_a_long_backward_chain_converges_below_the_work_bound() {
    // The sweeping solver before #755 moved the set one branch per pass: about 100 * 1100^2 / 2 = 60 M entries
    // over 1101 passes, beyond the bound. In postorder each PC is visited once: about 2 * 110_100 entries.
    let live = live_locations(&backward_chain(100, 1100)).unwrap();
    assert!(live.iter().all(|set| set.len() == 100));
}

#[test]
fn liveness_work_beyond_the_bound_is_rejected() {
    // A dispatch with 50 K outcomes to a ret of 1000 registers: its one visit reads the ret's 1000 live entries
    // for every outcome, 50 M in all; the stored sets stay at 2000 entries.
    let mut f = empty_function();
    f.registers = vec![Type::Int; 1000];
    f.code = vec![fan_out(50_000, 1), Instruction::Ret((0..1000).collect())];
    expect_budget(live_locations(&f).map(drop), "ALLOCATION_WORK", "allocation liveness work budget exceeded");
}

/// `count` unused parameters, then `used` constants that the ret reads.
fn parameters(count: u32, used: u32) -> Function {
    let mut f = empty_function();
    f.registers = vec![Type::Int; (count + used) as usize];
    f.params = (0..count).map(Location::Register).collect();
    f.blocks[0].params = f.params.clone();
    f.code = (count..count + used).map(constant).collect();
    f.code.push(Instruction::Ret((count..count + used).collect()));
    f
}

#[test]
fn interference_below_the_bound_is_allocated() {
    let f = parameters(300, 200);
    let allocation = allocate(&f).unwrap();
    check_allocation(&f, &allocation).unwrap();
}

#[test]
fn interference_clique_beyond_the_bound_is_rejected() {
    // 2900 parameters interfere pairwise: 2900 * 2899 directed edges in one clique.
    expect_budget(allocate(&parameters(2900, 0)).map(drop), "ALLOCATION_MEMORY",
        "interference clique exceeds storage budget");
}

#[test]
fn interference_graph_beyond_the_bound_is_rejected() {
    // Two cliques each within the clique bound: 2800 parameters, then 750 registers live at the ret.
    expect_budget(allocate(&parameters(2800, 750)).map(drop), "ALLOCATION_MEMORY",
        "interference graph exceeds storage budget");
}

/// A function without locations and an allocation with `slots` unused frame slots. `code` decides the
/// visited states; every outgoing edge of it gets an empty copy list.
fn wide_frame(code: Vec<Instruction>, slots: usize) -> (Function, Allocation) {
    let mut f = empty_function();
    f.code = code;
    let mut edges = std::collections::BTreeMap::new();
    for (pc, instruction) in f.code.iter().enumerate() {
        let count = match instruction {
            Instruction::Br(_) => 1,
            Instruction::Champ { flow: Flow::Outcomes(outcomes), .. } => outcomes.len(),
            _ => 0,
        };
        for ordinal in 0..count {
            edges.insert((pc as u32, ordinal as u32), vec![]);
        }
    }
    let allocation = Allocation {
        locations: std::collections::BTreeMap::new(),
        slots: vec![ProducedType::Value(Type::Int); slots],
        edges,
        roots: std::collections::BTreeMap::new(),
    };
    (f, allocation)
}

/// An instruction with `count` outcome edges to `target_pc`. The allocation checker reads only its edges.
fn fan_out(count: usize, target_pc: u32) -> Instruction {
    let outcome = Outcome { tag: Tag::Normal, edge: Edge { target_pc, assignments: vec![] } };
    Instruction::Champ { op: Opcode::FailureDispatch, arguments: vec![], flow: Flow::Outcomes(vec![outcome; count]) }
}

#[test]
fn allocation_check_below_the_bounds_passes() {
    let (f, allocation) = wide_frame(vec![fan_out(4, 1), Instruction::Ret(vec![])], 1_000_000);
    check_allocation(&f, &allocation).unwrap();
}

#[test]
fn allocation_check_state_visits_beyond_the_bound_are_rejected() {
    // 2.5 M slots: pc 0 and its 19 edges visit exactly 50 M entries; visiting pc 1 exceeds the bound.
    let (f, allocation) = wide_frame(vec![fan_out(19, 1), Instruction::Ret(vec![])], 2_500_000);
    expect_budget(check_allocation(&f, &allocation), "ALLOCATION_WORK", "allocation checker work budget exceeded");
}

#[test]
fn allocation_check_edge_visits_beyond_the_bound_are_rejected() {
    // The 20th edge of pc 0, a self loop, exceeds the bound while pc 0 is being visited.
    let (f, allocation) = wide_frame(vec![fan_out(21, 0)], 2_500_000);
    expect_budget(check_allocation(&f, &allocation), "ALLOCATION_WORK", "allocation edge work budget exceeded");
}

#[test]
fn allocation_check_storage_beyond_the_bound_is_rejected() {
    // Three stored states of 3 M slots each.
    let code = vec![branch(1), branch(2), branch(3), Instruction::Ret(vec![])];
    let (f, allocation) = wide_frame(code, 3_000_000);
    expect_budget(check_allocation(&f, &allocation), "ALLOCATION_MEMORY", "allocation checker storage exceeded");
}
