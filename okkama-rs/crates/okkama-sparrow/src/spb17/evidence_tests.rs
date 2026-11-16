//! The loader's emptiness proof of a location (#753): a packet carried around a loop through a builder journal
//! has no finite inventory, but no key can reach it unless a definition introduces one.
use super::*;

fn packet() -> Location {
    Location::Service(2)
}

/// A function whose packet `P1` starts at `start` and enters a loop as `P2`; each turn a builder push carries
/// `P2` on as its journal packet. Its parameter reaches a resource, so the function is not resource-free and
/// the proof cannot short-cut through the inputs.
fn packet_loop(start: Instruction) -> Program {
    let mut f = super::super::tests::empty_function();
    f.registers = vec![Type::Int, Type::Object(0)];
    f.params = vec![Location::Register(1)];
    f.blocks[0].params = f.params.clone();
    f.services = vec![ServiceKind::Packet; 3];
    f.code = vec![
        start,
        Instruction::Br(Edge { target_pc: 2, assignments: vec![Assignment {
            destination: packet(), source: Source::Location(Location::Service(1)),
        }] }),
        Instruction::Champ {
            op: Opcode::BuilderPush,
            arguments: vec![Argument::Location(Location::Register(0)), Argument::Location(packet())],
            flow: Flow::Outcomes(vec![Outcome { tag: Tag::Normal, edge: Edge { target_pc: 2, assignments: vec![
                Assignment { destination: packet(), source: Source::ProducedSlot(1) },
            ] } }]),
        },
    ];
    let resource = Object {
        name: "Res".into(), fields: vec![], error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle: false,
        role: None, implements: vec![], any_box: false, enum_names: vec![],
        layout: ObjectRuntimeLayout {
            storage_fields: vec![], allocation_words: 5, identity_offset: Some(8), has_destroy: true,
            may_reach_resource: true, may_async_destroy: false, trace_layout: vec![], selection_layout: vec![],
        },
    };
    Program { objects: vec![resource], functions: vec![f], ..Program::default() }
}

fn start(op: Opcode, arguments: Vec<Argument>) -> Instruction {
    Instruction::Champ { op, arguments, flow: Flow::Next(vec![Location::Service(1)]) }
}

/// An empty packet that loops through builder journals stays empty, though the cycle has no finite inventory.
#[test]
fn a_packet_carried_around_a_loop_from_empty_is_provably_empty() {
    let p = packet_loop(start(Opcode::OwnEmpty, vec![]));
    let mut evidence = Evidence::new(&p, &p.functions[0]);
    assert!(evidence.inventory(packet()).is_none(), "the cycle has no finite inventory");
    assert!(evidence.provably_empty(packet()));
    assert!(evidence.provably_empty(Location::Service(1)));
}

/// A committed resource that enters the loop reaches the packet on every turn.
#[test]
fn a_committed_resource_carried_around_a_loop_is_not_empty() {
    let commit = start(Opcode::OwnCreateCommit, vec![Argument::Location(Location::Register(0)), Argument::Object(0)]);
    let p = packet_loop(commit);
    let mut evidence = Evidence::new(&p, &p.functions[0]);
    assert!(!evidence.provably_empty(packet()));
    assert!(!evidence.provably_empty(Location::Service(1)));
}

/// A producer the loader does not recognise may introduce keys.
#[test]
fn an_unrecognised_producer_is_not_empty() {
    let p = packet_loop(start(Opcode::OwnDynamic, vec![]));
    let mut evidence = Evidence::new(&p, &p.functions[0]);
    assert!(!evidence.provably_empty(packet()));
}

/// A location nothing defines is not proved empty.
#[test]
fn an_undefined_location_is_not_empty() {
    let p = packet_loop(start(Opcode::OwnEmpty, vec![]));
    let mut evidence = Evidence::new(&p, &p.functions[0]);
    assert!(!evidence.provably_empty(Location::Service(0)));
}
