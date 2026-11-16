//! The contract of `dead::prune_unreachable` (#702): unreachable blocks go with their closes, plans and
//! records; reachable code keeps every instruction, and its pcs and indices follow the removal.
use super::*;
use s::{Argument as A, Location as L};

fn br(target_pc: u32) -> s::Instruction {
    s::Instruction::Br(s::Edge { target_pc, assignments: vec![] })
}

fn champ(op: s::Opcode, arguments: Vec<A>, output: u32) -> s::Instruction {
    s::Instruction::Champ { op, arguments, flow: s::Flow::Next(vec![L::Service(output)]) }
}

fn function(code: Vec<s::Instruction>, blocks: &[u32]) -> s::Function {
    s::Function {
        symbol: "fixture".into(), entry_abi: s::EntryAbi::Owned, registers: vec![s::Type::Int],
        services: vec![s::ServiceKind::ScopeState, s::ServiceKind::Cleanup, s::ServiceKind::Cleanup],
        params: vec![], results: vec![],
        blocks: blocks.iter().map(|&pc| s::Block { pc, params: vec![] }).collect(),
        proofs: s::FunctionProofs::default(), code,
    }
}

fn evidence(plan: u32, close: u32, slots: Vec<s::DirectSlot>) -> s::CleanupPlan {
    s::CleanupPlan { plan, reserve: false, close, reserve_empty: false, sources: vec![s::MergeSource::Scope], slots,
        preserves: vec![], joins: vec![], suffixes: vec![] }
}

fn close(begin_pc: u32, envelope: u32) -> s::CloseContract {
    s::CloseContract { begin_pc, boundary: 0, reserve: false, active_failure: false, envelope }
}

/// pc 0 jumps over the cleanup chain at 1..=3 (its first block, dead) to the live cleanup at 4..=6.
fn cleanup_pair() -> s::Function {
    let mut f = function(vec![
        br(4),
        champ(s::Opcode::CleanupBegin, vec![A::Location(L::Service(0)), A::Envelope(0)], 1),
        champ(s::Opcode::CleanupPlan, vec![A::Location(L::Service(1)), A::Plan(0)], 2),
        br(4),
        champ(s::Opcode::CleanupBegin, vec![A::Location(L::Service(0)), A::Envelope(1)], 1),
        champ(s::Opcode::CleanupPlan, vec![A::Location(L::Service(1)), A::Plan(1)], 2),
        s::Instruction::Ret(vec![]),
    ], &[0, 1, 4]);
    let slot = s::DirectSlot { identity: L::Register(0), descriptor: L::Register(0), destroy: 0,
        origin: s::CreationOrigin::Create(4), active_pc: 4 };
    f.proofs.closes = vec![close(1, 0), close(4, 1)];
    f.proofs.envelopes = vec![s::Envelope { failures: 1 }, s::Envelope { failures: 2 }];
    f.proofs.plans = vec![s::Plan { merge: true, selection: 0, order_witness: 0 },
        s::Plan { merge: false, selection: 1, order_witness: 1 }];
    f.proofs.cleanup_plans = vec![evidence(0, 0, vec![]), evidence(1, 1, vec![slot])];
    f.proofs.roots = vec![s::RootRequirement { pc: 2, locations: vec![L::Service(1)] },
        s::RootRequirement { pc: 5, locations: vec![L::Service(1)] }];
    f.proofs.post_roots = vec![s::RootRequirement { pc: 1, locations: vec![L::Service(1)] }];
    f.proofs.states = vec![s::StateRecord { pc: 3, locations: vec![] }, s::StateRecord { pc: 6, locations: vec![] }];
    f.proofs.order_witnesses = vec![s::OrderWitness { creation_pcs: vec![] },
        s::OrderWitness { creation_pcs: vec![1, 4] }];
    f
}

#[test]
fn unreachable_blocks_follow_every_edge_from_the_entry() {
    assert_eq!(dead::unreachable_blocks(&cleanup_pair()).unwrap(), vec![1]);
    // A dead cycle stays dead: 1 and 2 reach each other but nothing from the entry reaches them.
    let cycle = function(vec![s::Instruction::Ret(vec![]), br(2), br(1)], &[0, 1, 2]);
    assert_eq!(dead::unreachable_blocks(&cycle).unwrap(), vec![1, 2]);
    let joined = function(vec![br(2), br(2), s::Instruction::Ret(vec![])], &[0, 1, 2]);
    assert_eq!(dead::unreachable_blocks(&joined).unwrap(), vec![1]);
}

#[test]
fn prune_removes_the_unreachable_chain_with_its_close_plan_and_records() {
    let mut f = cleanup_pair();
    dead::prune_unreachable(&mut f, &mut []).unwrap();
    assert_eq!(f.code, vec![
        br(1),
        champ(s::Opcode::CleanupBegin, vec![A::Location(L::Service(0)), A::Envelope(0)], 1),
        champ(s::Opcode::CleanupPlan, vec![A::Location(L::Service(1)), A::Plan(0)], 2),
        s::Instruction::Ret(vec![]),
    ]);
    assert_eq!(f.blocks.iter().map(|block| block.pc).collect::<Vec<_>>(), vec![0, 1]);
    assert_eq!(f.proofs.closes, vec![close(1, 0)]);
    assert_eq!(f.proofs.envelopes, vec![s::Envelope { failures: 2 }]);
    assert_eq!(f.proofs.plans, vec![s::Plan { merge: false, selection: 1, order_witness: 1 }]);
    let slot = s::DirectSlot { identity: L::Register(0), descriptor: L::Register(0), destroy: 0,
        origin: s::CreationOrigin::Create(1), active_pc: 1 };
    assert_eq!(f.proofs.cleanup_plans, vec![evidence(0, 0, vec![slot])]);
    assert_eq!(f.proofs.roots, vec![s::RootRequirement { pc: 2, locations: vec![L::Service(1)] }]);
    assert!(f.proofs.post_roots.is_empty());
    assert_eq!(f.proofs.states, vec![s::StateRecord { pc: 3, locations: vec![] }]);
    assert_eq!(f.proofs.order_witnesses[1].creation_pcs, vec![1]);
    // Unreferenced tables keep their indices: selections and witnesses are named by the kept plan.
    assert_eq!(f.proofs.order_witnesses.len(), 2);
}

#[test]
fn prune_keeps_a_function_without_unreachable_code_as_it_is() {
    let mut f = cleanup_pair();
    f.code[0] = br(1);
    f.code[3] = br(4);
    let before = f.clone();
    dead::prune_unreachable(&mut f, &mut []).unwrap();
    assert_eq!(f, before);
}

fn frame(op: s::Opcode, continuation: u32) -> s::Instruction {
    champ(op, vec![A::Location(L::Service(0)), A::Continuation(continuation)], 0)
}

/// An async body (#741): the start at pc 0 jumps over a dead block with its own suspension (continuation 1, site 3)
/// to the live one (continuation 2, site 7).
fn async_pair() -> (s::Function, s::FrameLayout) {
    let site = |site| s::Continuation { site, saved: vec![], operands: vec![] };
    let mut f = function(vec![
        frame(s::Opcode::FrameResume, 0),
        br(6),
        frame(s::Opcode::FrameSave, 1),
        champ(s::Opcode::FutureDone, vec![A::Location(L::Service(0))], 0),
        frame(s::Opcode::FrameResume, 1),
        br(6),
        frame(s::Opcode::FrameSave, 2),
        champ(s::Opcode::FutureDone, vec![A::Location(L::Service(0))], 0),
        frame(s::Opcode::FrameResume, 2),
        s::Instruction::Ret(vec![]),
    ], &[0, 2, 6]);
    f.entry_abi = s::EntryAbi::AsyncBody { layout: 0 };
    let layout = s::FrameLayout { slots: vec![], continuations: vec![site(None), site(Some(3)), site(Some(7))] };
    (f, layout)
}

#[test]
fn prune_moves_an_async_body_and_its_continuation_sites() {
    let (mut f, layout) = async_pair();
    let mut frames = vec![layout];
    dead::prune_unreachable(&mut f, &mut frames).unwrap();
    assert_eq!(f.code, vec![
        frame(s::Opcode::FrameResume, 0),
        br(2),
        frame(s::Opcode::FrameSave, 1),
        champ(s::Opcode::FutureDone, vec![A::Location(L::Service(0))], 0),
        frame(s::Opcode::FrameResume, 1),
        s::Instruction::Ret(vec![]),
    ]);
    assert_eq!(f.blocks.iter().map(|block| block.pc).collect::<Vec<_>>(), vec![0, 2]);
    // The dead continuation goes; the live one is renumbered and its site follows its pc.
    let sites = frames[0].continuations.iter().map(|continuation| continuation.site).collect::<Vec<_>>();
    assert_eq!(sites, vec![None, Some(3)]);
}

#[test]
fn prune_moves_the_sites_of_an_async_body_whose_dead_code_does_not_suspend() {
    let (mut f, mut layout) = async_pair();
    // The dead block suspends nothing: only the pcs of the live sites move.
    f.code[2] = br(6);
    f.code[3] = br(6);
    f.code[4] = br(6);
    layout.continuations.remove(1);
    f.code[6] = frame(s::Opcode::FrameSave, 1);
    f.code[8] = frame(s::Opcode::FrameResume, 1);
    let mut frames = vec![layout];
    dead::prune_unreachable(&mut f, &mut frames).unwrap();
    assert_eq!(frames[0].continuations[1].site, Some(3));
    assert_eq!(f.code[4], frame(s::Opcode::FrameResume, 1));
}

#[test]
fn prune_refuses_a_live_resume_of_a_removed_continuation() {
    let (mut f, layout) = async_pair();
    f.code[8] = frame(s::Opcode::FrameResume, 1);
    let mut frames = vec![layout];
    let failure = dead::prune_unreachable(&mut f, &mut frames).unwrap_err().0;
    assert!(failure.contains("removed continuation 1"), "{failure}");
}

#[test]
fn prune_refuses_a_live_record_at_a_removed_pc() {
    let mut f = cleanup_pair();
    f.proofs.cleanup_plans[1].slots[0].active_pc = 2;
    assert!(dead::prune_unreachable(&mut f, &mut []).unwrap_err().0.contains("removed pc 2"));
}
