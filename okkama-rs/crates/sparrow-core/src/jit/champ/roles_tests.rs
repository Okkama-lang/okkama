//! Role values and action values in machine code (#676): dispatch through implementation tables, checked
//! narrowing and closure objects, under forced collection at every collecting helper.
use super::*;
use c::{CallOwn, CallPost, PacketSlot};
use crate::runtime::FailureKind;

fn reg(id: u32) -> L {
    L::Register(id)
}

fn service(id: u32) -> L {
    L::Service(id)
}

fn produced(destination: L, slot: u32) -> s::Assignment {
    s::Assignment { destination, source: s::Source::ProducedSlot(slot) }
}

fn data(op: s::DataOp, inputs: Vec<u32>, output: u32, immediate: s::Immediate) -> s::Instruction {
    s::Instruction::Data { op, inputs, output: Some(output), immediate }
}

fn text(output: u32, value: &str) -> s::Instruction {
    data(s::DataOp::Const, vec![], output, s::Immediate::Constant(s::Constant::Str(value.into())))
}

fn ret_own(locations: &[L]) -> s::Instruction {
    s::Instruction::Champ { op: O::RetOwn, arguments: locations.iter().copied().map(A::Location).collect(),
        flow: s::Flow::Next(vec![]) }
}

fn object(name: &str, storage: Vec<s::Type>, role: Option<s::RoleDecl>, implements: Vec<s::RoleImpl>) -> s::Object {
    let count = storage.len() as u32;
    s::Object {
        name: name.into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![], role, implements,
        fields: (0..count).map(|storage| s::Field { name: format!("f{storage}"), storage, once: false }).collect(),
        layout: s::ObjectRuntimeLayout {
            trace_layout: (0..count).collect(), storage_fields: storage, allocation_words: (1 + count).max(2),
            identity_offset: None, has_destroy: false, may_reach_resource: false, may_async_destroy: false,
            selection_layout: vec![],
        },
    }
}

fn slot(name: &str) -> s::RoleSlot {
    s::RoleSlot { name: name.into(), entry_abi: s::EntryAbi::Owned, params: vec![], results: vec![s::Type::Str] }
}

/// Register, service and result types of a function.
type Types = (Vec<s::Type>, Vec<s::ServiceKind>, Vec<s::Type>);

/// The entry block's parameters are the function's; roots before every instruction and the reconstructed
/// service states complete its proofs.
fn function(symbol: &str, abi: s::EntryAbi, (registers, services, results): Types, blocks: Vec<s::Block>,
    code: Vec<s::Instruction>, failures: u32) -> s::Function
{
    let mut f = s::Function {
        symbol: symbol.into(), entry_abi: abi, registers, services, params: blocks[0].params.clone(), results,
        blocks,
        proofs: s::FunctionProofs {
            entry: Some(s::EntryContract { failures, ..s::EntryContract::default() }),
            ..s::FunctionProofs::default()
        },
        code,
    };
    f.proofs.states = s::service_states(&f).unwrap();
    let live = s::live_locations(&f).unwrap();
    f.proofs.roots = live.iter().enumerate()
        .map(|(pc, locations)| s::RootRequirement { pc: pc as u32, locations: locations.iter().copied().collect() })
        .collect();
    // A helper with a next pc also roots what is live after it.
    f.proofs.post_roots = f.code.iter().enumerate().filter(|(_, instruction)| {
        matches!(instruction, s::Instruction::Champ { flow: s::Flow::Next(_), op, .. } if *op != O::RetOwn)
    }).map(|(pc, _)| s::RootRequirement {
        pc: pc as u32, locations: live.get(pc + 1).into_iter().flatten().copied().collect(),
    }).collect();
    f
}

/// `(receiver; packet) -> (str, packet)`: the receiver's text field followed by `suffix`.
fn implementation(symbol: &str, receiver: u32, suffix: &str) -> s::Function {
    let types = (vec![s::Type::Object(receiver), s::Type::Str, s::Type::Str, s::Type::Str],
        vec![s::ServiceKind::Packet], vec![s::Type::Str]);
    function(symbol, s::EntryAbi::Owned, types, vec![s::Block { pc: 0, params: vec![reg(0), service(0)] }], vec![
            data(s::DataOp::ObjGet, vec![0], 1, s::Immediate::Field(0)),
            text(2, suffix),
            data(s::DataOp::StrConcat, vec![1, 2], 3, s::Immediate::None),
            ret_own(&[reg(3), service(0)]),
        ], 0)
}

/// `(receiver; packet) -> (str, packet)`: dispatches slot 0 of `role` to `targets`.
fn caller(symbol: &str, role: u32, receiver: s::Type, targets: Vec<u32>) -> s::Function {
    let dispatch = s::Dispatch { role, slot: 0, failures: 0, retained_params: vec![], targets };
    let types = (vec![receiver, s::Type::Str], vec![s::ServiceKind::Packet; 2], vec![s::Type::Str]);
    function(symbol, s::EntryAbi::Owned, types,
        vec![s::Block { pc: 0, params: vec![reg(0), service(0)] },
            s::Block { pc: 1, params: vec![reg(1), service(1)] }],
        vec![
            s::Instruction::Champ { op: O::InvokeDispatchOwn, arguments: vec![A::Dispatch(dispatch), A::Flag(false),
                A::Location(reg(0)), A::Location(service(0))],
                flow: s::Flow::Outcomes(vec![s::Outcome { tag: s::Tag::Normal, edge: s::Edge {
                    target_pc: 1, assignments: vec![produced(reg(1), 0), produced(service(1), 1)],
                } }]) },
            ret_own(&[reg(1), service(1)]),
        ], 0)
}

const DESCRIBE: usize = 0;
const NARROW: usize = 4;
const MAKE_CLOSURE: usize = 7;
const CALL_CLOSURE: usize = 8;
const SHOW: usize = 10;

/// Descriptors: 0 role `Named` (implemented by 1 `Tag` and 2 `Box`), 3 role `Labeled` (by `Tag` only),
/// 4 closure layout `greet.closure` capturing a string, 5 role `fn() -> str` (implemented by the layout).
fn program() -> s::Program {
    let objects = vec![
        object("Named", vec![], Some(s::RoleDecl { slots: vec![slot("name")] }), vec![]),
        object("Tag", vec![s::Type::Str], None,
            vec![s::RoleImpl { role: 0, slots: vec![1] }, s::RoleImpl { role: 3, slots: vec![3] },
                s::RoleImpl { role: 6, slots: vec![9] }]),
        object("Box", vec![s::Type::Str], None, vec![s::RoleImpl { role: 0, slots: vec![2] }]),
        object("Labeled", vec![], Some(s::RoleDecl { slots: vec![slot("label")] }), vec![]),
        object("greet.closure", vec![s::Type::Str], None, vec![s::RoleImpl { role: 5, slots: vec![5] }]),
        object("fn#0", vec![], Some(s::RoleDecl { slots: vec![slot("call")] }), vec![]),
        object("Texted", vec![], Some(s::RoleDecl { slots: vec![s::RoleSlot {
            name: "text".into(), entry_abi: s::EntryAbi::Observation, params: vec![], results: vec![s::Type::Str],
        }] }), vec![]),
    ];
    let types = (vec![s::Type::Role(0), s::Type::Role(3)], vec![], vec![s::Type::Role(3)]);
    let entry = vec![s::Block { pc: 0, params: vec![reg(0)] }];
    let narrow = function("narrow", s::EntryAbi::Observation, types, entry, vec![
            data(s::DataOp::RoleNarrow, vec![0], 1, s::Immediate::None),
            s::Instruction::Ret(vec![1]),
        ], 2);
    // The trampoline reads its capture and calls the lambda with it.
    let types = (vec![s::Type::Object(4), s::Type::Str, s::Type::Str], vec![s::ServiceKind::Packet; 2],
        vec![s::Type::Str]);
    let trampoline = function("greet.closure.call", s::EntryAbi::Owned, types,
        vec![s::Block { pc: 0, params: vec![reg(0), service(0)] },
            s::Block { pc: 2, params: vec![reg(2), service(1)] }],
        vec![
            data(s::DataOp::ObjGet, vec![0], 1, s::Immediate::Field(0)),
            s::Instruction::Champ { op: O::InvokeCallOwn, arguments: vec![A::Entry(6), A::Summary(0), A::Flag(false),
                A::Location(reg(1)), A::Location(service(0))],
                flow: s::Flow::Outcomes(vec![s::Outcome { tag: s::Tag::Normal, edge: s::Edge {
                    target_pc: 2, assignments: vec![produced(reg(2), 0), produced(service(1), 1)],
                } }]) },
            ret_own(&[reg(2), service(1)]),
        ], 0);
    let types = (vec![s::Type::Str; 3], vec![s::ServiceKind::Packet], vec![s::Type::Str]);
    let lambda = function("greet", s::EntryAbi::Owned, types,
        vec![s::Block { pc: 0, params: vec![reg(0), service(0)] }], vec![
            text(1, "!"),
            data(s::DataOp::StrConcat, vec![0, 1], 2, s::Immediate::None),
            ret_own(&[reg(2), service(0)]),
        ], 0);
    let types = (vec![s::Type::Str, s::Type::Object(4), s::Type::Role(5)], vec![], vec![s::Type::Role(5)]);
    let make = function("make", s::EntryAbi::Observation, types, vec![s::Block { pc: 0, params: vec![reg(0)] }], vec![
            data(s::DataOp::ObjNew, vec![0], 1, s::Immediate::Object(4)),
            data(s::DataOp::Move, vec![1], 2, s::Immediate::None),
            s::Instruction::Ret(vec![2]),
        ], 0);
    // A plain slot: the implementation and its dispatching caller are observations.
    let types = (vec![s::Type::Object(1), s::Type::Str, s::Type::Str, s::Type::Str], vec![], vec![s::Type::Str]);
    let text = function("Tag.text", s::EntryAbi::Observation, types, vec![s::Block { pc: 0, params: vec![reg(0)] }],
        vec![
            data(s::DataOp::ObjGet, vec![0], 1, s::Immediate::Field(0)),
            text(2, " text"),
            data(s::DataOp::StrConcat, vec![1, 2], 3, s::Immediate::None),
            s::Instruction::Ret(vec![3]),
        ], 0);
    let dispatch = s::Dispatch { role: 6, slot: 0, failures: 0, retained_params: vec![], targets: vec![1] };
    let types = (vec![s::Type::Role(6), s::Type::Str], vec![], vec![s::Type::Str]);
    let show = function("show", s::EntryAbi::Observation, types, vec![s::Block { pc: 0, params: vec![reg(0)] }],
        vec![
            s::Instruction::Champ { op: O::InvokeDispatch, arguments: vec![A::Dispatch(dispatch), A::Location(reg(0))],
                flow: s::Flow::Next(vec![reg(1)]) },
            s::Instruction::Ret(vec![1]),
        ], 0);
    let functions = vec![
        caller("describe", 0, s::Type::Role(0), vec![1, 2]),
        implementation("Tag.name", 1, " tag"),
        implementation("Box.name", 2, " box"),
        implementation("Tag.label", 1, " label"),
        narrow,
        trampoline,
        lambda,
        make,
        caller("call", 5, s::Type::Role(5), vec![4]),
        text,
        show,
    ];
    let mut program = s::Program { objects, functions, ..s::Program::default() };
    program.tables.summaries.push(s::Summary { entry: 6, may_fail: false, retained_params: vec![] });
    program
}

fn runtime() -> Box<Runtime> {
    let mut runtime = Runtime::new_champ(program(), vec![]).unwrap();
    runtime.heap.set_stress(true);
    runtime
}

fn context(runtime: &mut Runtime) -> Ctx {
    Ctx { status: OK, stack_limit: 0, failure: 0, runtime, entry_frame: 0, scratch: 0 }
}

/// Calls an owning function with an empty packet; returns its outcome ordinal and Post.
fn owned(runtime: &mut Runtime, ctx: &mut Ctx, function: usize, args: &[Word]) -> (Word, CallPost) {
    let code = compile(runtime, function).unwrap();
    let mut post = CallPost::default();
    let mut input = PacketSlot::default();
    assert_eq!(c::rt_own_empty(ctx, &mut input), 0);
    let mut io = CallOwn {
        input, entry_abi: runtime.champ_entry_id(function).unwrap(), output: &mut post, context: 0, cleanup_mode: 0,
    };
    let entry: extern "C" fn(*mut Ctx, *mut CallOwn, *const Word, u64) -> Word =
        unsafe { std::mem::transmute(code) };
    let outcome = entry(ctx, &mut io, args.as_ptr(), args.len() as u64);
    (outcome, post)
}

fn object_of(runtime: &mut Runtime, descriptor: u32, value: &str) -> Word {
    let value = runtime.heap.allocate_text(value);
    let object = runtime.heap.object_new(descriptor, &[value]);
    runtime.roots.push((object, Repr::Ref));
    object
}

#[test]
fn champ_dispatch_selects_each_implementation_under_forced_gc() {
    let mut runtime = runtime();
    let mut ctx = context(&mut runtime);
    for (descriptor, expected) in [(1, "first tag"), (2, "second box")] {
        let receiver = object_of(&mut runtime, descriptor, expected.split(' ').next().unwrap());
        let (outcome, post) = owned(&mut runtime, &mut ctx, DESCRIBE, &[receiver]);
        assert_eq!((outcome, ctx.status), (0, OK));
        assert_eq!(runtime.heap.text(post.value), expected);
        assert!(runtime.champ_packet_keys(&post.packet).unwrap().is_empty());
    }
    assert!(runtime.heap.stats().collections > 0);
}

#[test]
fn champ_dispatch_outside_its_targets_stops() {
    // A `Box` reaching a dispatch listed only for `Tag` stops the machine instead of entering `Box.name`.
    let mut p = program();
    let s::Instruction::Champ { arguments, .. } = &mut p.functions[DESCRIBE].code[0] else { unreachable!() };
    let A::Dispatch(dispatch) = &mut arguments[0] else { unreachable!() };
    dispatch.targets = vec![1];
    let mut runtime = Runtime::new_champ(p, vec![]).unwrap();
    let mut ctx = context(&mut runtime);
    let receiver = object_of(&mut runtime, 2, "second");
    let (outcome, _) = owned(&mut runtime, &mut ctx, DESCRIBE, &[receiver]);
    assert_eq!(outcome, c::OUTCOME_STOP);
    assert!(runtime.fatal.as_deref().is_some_and(|message| message.contains("invalid machine outcome")),
        "{:?}", runtime.fatal);
}

#[test]
fn champ_role_narrowing_checks_the_implementation_table() {
    let mut runtime = runtime();
    let mut ctx = context(&mut runtime);
    let code = compile(&mut runtime, NARROW).unwrap();
    let tag = object_of(&mut runtime, 1, "tag");
    assert_eq!(unsafe { invoke(code, &mut ctx, &[tag]) }, tag);
    assert_eq!(ctx.status, OK);
    let boxed = object_of(&mut runtime, 2, "box");
    unsafe { invoke(code, &mut ctx, &[boxed]) };
    assert_eq!(ctx.status, FAILED);
    let failure = runtime.failure(ctx.failure).unwrap();
    assert!(matches!(&failure.kind, FailureKind::Program(message) if message == "role witness is missing"));
}

#[test]
fn champ_closure_object_keeps_its_capture_across_collections() {
    let mut runtime = runtime();
    let mut ctx = context(&mut runtime);
    let code = compile(&mut runtime, MAKE_CLOSURE).unwrap();
    let capture = runtime.heap.allocate_text("captured");
    runtime.roots.push((capture, Repr::Ref));
    let closure = unsafe { invoke(code, &mut ctx, &[capture]) };
    assert_eq!(ctx.status, OK);
    runtime.roots.clear();
    runtime.roots.push((closure, Repr::Ref));
    for _ in 0..3 {
        let (outcome, post) = owned(&mut runtime, &mut ctx, CALL_CLOSURE, &[closure]);
        assert_eq!((outcome, ctx.status), (0, OK));
        assert_eq!(runtime.heap.text(post.value), "captured!");
    }
    assert!(runtime.heap.stats().collections > 0);
}

#[test]
fn champ_dispatch_reports_its_receiver_and_writes_results_after_the_call() {
    // The receiver is a precise root of the collecting call; the result lands only on the normal edge.
    let runtime = runtime();
    let source = runtime.champ.machine.as_ref().unwrap().source.clone();
    let frame = roots::Frame::new(&source.functions[DESCRIBE]).unwrap();
    let before = frame.before(&source.functions[DESCRIBE], Some(0));
    let receiver = frame.offsets[frame.allocation.locations[&reg(0)] as usize];
    assert!(before.contains(&receiver));
    let result = frame.offsets[frame.allocation.locations[&reg(1)] as usize];
    assert!(!before.contains(&result));
}

#[test]
fn champ_plain_dispatch_returns_the_implementation_value_under_forced_gc() {
    let mut runtime = runtime();
    let mut ctx = context(&mut runtime);
    let code = compile(&mut runtime, SHOW).unwrap();
    let tag = object_of(&mut runtime, 1, "plain");
    for _ in 0..3 {
        let value = unsafe { invoke(code, &mut ctx, &[tag]) };
        assert_eq!(ctx.status, OK);
        assert_eq!(runtime.heap.text(value), "plain text");
    }
    assert!(runtime.heap.stats().collections > 0);
}
