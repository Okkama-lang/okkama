use super::*;
use c::{CallOwn, CallPost, PacketSlot};
use crate::runtime::FailureKind;
use std::cell::RefCell;

#[derive(Default)]
struct Scenario {
    fail_destroy: Vec<u64>,
    destroy_calls: Vec<u64>,
    risky: bool,
    defer: bool,
    defer_calls: usize,
    stop: bool,
    replacement: u64,
    collect_post: bool,
}
thread_local! {
    static SCENARIO: RefCell<Scenario> = RefCell::new(Scenario::default());
}

fn failure(ctx: *mut Ctx, message: &str) -> u64 {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let payload = runtime.heap.allocate_text(message);
    let failure = runtime.failure_new(FailureKind::Thrown(payload));
    unsafe {
        (*ctx).failure = failure;
        (*ctx).status = FAILED;
    }
    failure
}

extern "C" fn destroy(ctx: *mut Ctx, args: *const Word, count: u64) -> Word {
    assert_eq!(count, 1);
    assert_eq!(unsafe { (*ctx).status }, OK, "capture must reset before the next destroy");
    let runtime = unsafe { &mut *(*ctx).runtime };
    let object = unsafe { *args };
    let identity = runtime.heap.champ_identity(object).unwrap();
    assert_eq!(identity.life, c::Life::Destroying);
    SCENARIO.with(|state| state.borrow_mut().destroy_calls.push(identity.serial));
    if SCENARIO.with(|state| state.borrow().fail_destroy.contains(&identity.serial)) {
        failure(ctx, &format!("destroy {}", identity.serial));
    }
    0
}

extern "C" fn external(ctx: *mut Ctx, io: *mut CallOwn, args: *const Word, count: u64) -> Word {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let machine = runtime.champ.machine.as_ref().unwrap();
    let index = machine.function_for_entry(unsafe { (*io).entry_abi }).unwrap();
    let function = machine.source.functions[index].clone();
    let symbol = function.symbol.as_str();
    let words = if count == 0 { &[] } else { unsafe { std::slice::from_raw_parts(args, count as usize) } };
    let mut post = CallPost::default();
    assert!(runtime.champ_packet_keys(unsafe { &(*io).input }).unwrap().is_empty());
    assert_eq!(c::rt_own_move(ctx, unsafe { &mut (*io).input }, &mut post.packet), 0);
    if symbol == "risky" && SCENARIO.with(|state| state.borrow().stop) {
        return machine::rt_machine_invalid(ctx);
    }
    if symbol == "replacements" {
        let ordinal = SCENARIO.with(|state| state.borrow().replacement);
        if ordinal == 3 {
            post.failure = failure(ctx, "replacement unwind");
            post.packet.record = 0;
        } else if ordinal == 0 {
            post.packet.record = 0;
        } else {
            post.value = if ordinal == 1 { word::small(37).unwrap() } else { 0 };
        }
        unsafe { (*io).output.write(post) };
        return ordinal;
    }
    if symbol == "risky" && SCENARIO.with(|state| state.borrow().risky) {
        post.failure = failure(ctx, "body");
        // Its offered packet is empty, but is still consumed exactly once.
    }
    if symbol == "log" {
        SCENARIO.with(|state| state.borrow_mut().defer_calls += 1);
        if SCENARIO.with(|state| state.borrow().defer) {
            post.failure = failure(ctx, "defer");
        }
    }
    if post.failure != 0 {
        post.packet.record = 0;
        unsafe { (*io).output.write(post) };
        return 1;
    }
    if symbol == "log" {
        post.packet.record = 0;
        unsafe { (*io).output.write(post) };
        return 0;
    }
    if matches!(function.results.first(), Some(s::Type::Object(0))) {
        let object = runtime.heap.object_new(0, words);
        let owner = runtime.champ_new_owner().unwrap();
        assert_eq!(c::rt_own_create_commit(ctx, object, 0, owner.get(), &mut post.packet), 0);
        post.value = object;
    } else if function.results.first() == Some(&s::Type::Object(1)) {
        post.value = runtime.heap.object_new(1, words);
        assert_eq!(c::rt_own_empty(ctx, &mut post.packet), 0);
    } else {
        post.value = runtime.heap.int(7);
        assert_eq!(c::rt_own_empty(ctx, &mut post.packet), 0);
    }
    if SCENARIO.with(|state| state.borrow().collect_post) {
        let base = runtime.roots.len();
        let repr = function.results.first().copied().map_or(Repr::Raw, machine::repr);
        runtime.roots.push((post.value, repr));
        runtime.roots.push((post.packet.record, Repr::Ref));
        let frame = runtime::frame_pointer!();
        runtime.safepoint(frame, unsafe { &*ctx }, &[]);
        runtime.roots.truncate(base);
    }
    // The future result stays in Rust roots through the last safepoint, then Post is published.
    unsafe { (*io).output.write(post) };
    0
}

fn load(text: &str) -> Box<Runtime> {
    let program = s::assemble(text).unwrap();
    let external = program.functions.iter().filter(|function| function.code.is_empty()).map(|function| {
        c::ExternalEntry {
            declaration: function.clone(),
            owned: (function.entry_abi != s::EntryAbi::Observation).then_some(external as c::OwnedEntry),
            plain: (function.entry_abi == s::EntryAbi::Observation).then_some(destroy as c::PlainEntry),
        }
    }).collect();
    let mut runtime = Runtime::new_champ(program, external).unwrap();
    runtime.heap.set_stress(true);
    runtime
}

fn context(runtime: &mut Runtime) -> Ctx {
    Ctx { status: OK, stack_limit: 0, failure: 0, runtime, entry_frame: 0, scratch: 0 }
}

fn run(text: &str, scenario: Scenario) -> (Box<Runtime>, Ctx, CallPost, Word) {
    let expected_stop = scenario.stop;
    SCENARIO.with(|state| *state.borrow_mut() = scenario);
    let mut runtime = load(text);
    let source = runtime.champ.machine.as_ref().unwrap().source.clone();
    let index = source.functions.iter().position(|function| function.symbol == "main").unwrap();
    for index in 0..source.functions.len() {
        compile(&mut runtime, index).unwrap();
    }
    let function = &source.functions[index];
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = runtime::frame_pointer!();
    let mut input = PacketSlot::default();
    c::rt_own_empty(&mut ctx, &mut input);
    let mut args = Vec::new();
    for &location in &function.params {
        let L::Register(id) = location else { continue };
        let value = match function.registers[id as usize] {
            s::Type::Object(descriptor) => {
                let object = runtime.heap.object_new(descriptor, &[]);
                if function.entry_abi != s::EntryAbi::Observation {
                    let owner = runtime.champ_new_owner().unwrap();
                    c::rt_own_create_commit(&mut ctx, object, descriptor as u64, owner.get(), &mut input);
                }
                object
            }
            s::Type::Str => runtime.heap.allocate_text("A"),
            s::Type::Failure => runtime.failure_new(FailureKind::Program("body".into())),
            s::Type::Bool => 0,
            _ => word::small(0).unwrap(),
        };
        args.push(value);
    }
    let mut post = CallPost::default();
    let result = if function.entry_abi == s::EntryAbi::Observation {
        unsafe { invoke(runtime.invocations[index].code, &mut ctx, &args) }
    } else {
        let entry = runtime.champ.machine.as_ref().unwrap().entries[index];
        let mut io = CallOwn { input, entry_abi: entry, output: &mut post, context: 0, cleanup_mode: 0 };
        let result = c::cleanup::rt_call_owned(&mut ctx, entry, args.as_ptr(), args.len() as u64, &mut io, &mut post);
        assert_eq!(io.input.record, 0);
        result
    };
    assert!((1..512).contains(&runtime.heap.stats().collections));
    SCENARIO.with(|state| assert_eq!(state.borrow().destroy_calls, runtime.champ.destroy_events));
    assert_eq!(runtime.fatal.is_some(), expected_stop, "{:?}", runtime.fatal);
    (runtime, ctx, post, result)
}

macro_rules! golden {
    ($test:ident, $name:literal, $expected:expr) => {
        #[test]
        fn $test() {
            let text = include_str!(concat!(
                "../../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/", $name, ".spb17"
            ));
            let (runtime, ctx, _, _) = run(text, Scenario::default());
            assert_eq!(ctx.status, OK, "{:?}", runtime.failure(ctx.failure));
            assert_eq!(runtime.champ.destroy_events, $expected);
        }
    };
}
golden!(champ_j1_local_pair, "local-pair", vec![2, 1]);
golden!(champ_j1_return_old, "return-old", vec![2, 1]);
golden!(champ_j1_fresh_call, "fresh-call", vec![2, 1]);
golden!(champ_j1_mixed_journal, "mixed-journal", vec![2, 1]);
golden!(champ_j1_explicit_abort, "explicit-abort", vec![2, 1]);
golden!(champ_j1_unwind_reserve, "unwind-reserve", vec![2, 1]);
golden!(champ_j1_reserve_executor, "reserve-executor", vec![1]);
golden!(champ_j1_exit_updates, "exit-updates", vec![1]);
golden!(champ_j1_valued, "valued", Vec::<u64>::new());
golden!(champ_j1_spec16_1, "spec16-1", vec![1]);
golden!(champ_j1_spec16_4, "spec16-4", vec![1]);

const RESERVE: &str = include_str!(
    "../../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/reserve-executor.spb17"
);

fn settle(ctx: &mut Ctx, packet: &mut PacketSlot) -> u64 {
    ctx.entry_frame = runtime::frame_pointer!();
    use c::cleanup::*;
    let mut state = c::CleanupSlot::default();
    let inputs = c::CloseInputs {
        scope: std::ptr::null_mut(), packet, pending: std::ptr::null_mut(), failure: 0, settlements: 0,
    };
    assert_eq!(rt_cleanup_begin(ctx, &inputs, &mut state), 0);
    assert_eq!(rt_cleanup_settle_empty(ctx, &mut state, &mut state), 0);
    let plan = unsafe { &mut *ctx.runtime }.champ_register_cleanup_plan(c::CleanupPlan {
        reserve: false, direct: false, may_fail: true,
    });
    assert_eq!(rt_cleanup_plan(ctx, &mut state, plan, &mut state), 0);
    // All fixtures have at most three identities. A broken loop fails without a long run.
    for _ in 0..4 {
        let mut next = c::NextPost::default();
        let ordinal = rt_cleanup_next(ctx, &mut state, &mut next);
        state = next.state;
        if ordinal == 1 {
            let mut finish = c::FinishPost::default();
            assert_eq!(rt_cleanup_finish(ctx, &mut state, &mut finish), 0);
            return finish.failure;
        }
        assert_eq!(ordinal, 0);
        let mut output = c::DestroyPost::default();
        assert_eq!(rt_cleanup_destroy(ctx, &mut state, next.key, &mut output), 0);
        state = output.state;
        if output.failure != 0 {
            assert_eq!(rt_cleanup_failure(ctx, &mut state, output.failure, &mut state), 0);
        }
    }
    panic!("unbounded cleanup");
}

#[test]
fn champ_j1_model_a_r_and_r_a_with_reserve_failure() {
    let (runtime, mut ctx, mut post, result) = run(RESERVE, Scenario::default());
    assert_eq!(result, 0);
    assert_eq!(ctx.status, OK);
    assert_eq!(runtime.champ.destroy_events, vec![1]);
    assert_ne!(post.packet.record, 0);
    assert_eq!(settle(&mut ctx, &mut post.packet), 0);
    assert_eq!(runtime.champ.destroy_events, vec![1, 2]);
    assert_eq!(post.packet.record, 0);

    let (runtime, ctx, post, result) = run(RESERVE, Scenario {
        fail_destroy: vec![1, 2], ..Scenario::default()
    });
    assert_eq!(result, 1);
    assert_eq!(ctx.status, FAILED);
    assert_eq!(runtime.champ.destroy_events, vec![1, 2]);
    assert_eq!(post.packet.record, 0);
    let failure = runtime.failure(post.failure).unwrap();
    assert_eq!(failure.secondary.len(), 1);
    assert!(failure.cleanup_origin);
}

#[test]
fn champ_j1_direct_plan_has_no_resource_graph_walk() {
    let before = machine::graph_walks();
    let text = include_str!("../../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/local-pair.spb17");
    let (runtime, ctx, _, _) = run(text, Scenario { fail_destroy: vec![2, 1], ..Scenario::default() });
    assert_eq!(runtime.champ.destroy_events, vec![2, 1]);
    assert_eq!(ctx.status, FAILED);
    assert_eq!(runtime.failure(ctx.failure).unwrap().secondary.len(), 1);
    assert_eq!(machine::graph_walks(), before);
}

#[test]
fn champ_j1_defer_capture_resets_status_and_preserves_body_primary() {
    let text = include_str!("../../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/spec16-4.spb17");
    let (runtime, ctx, _, _) = run(text, Scenario {
        risky: true, defer: true, fail_destroy: vec![1], ..Scenario::default()
    });
    assert_eq!(ctx.status, FAILED);
    assert_eq!(runtime.champ.destroy_events, vec![1]);
    SCENARIO.with(|state| assert_eq!(state.borrow().defer_calls, 1));
    let primary = runtime.failure(ctx.failure).unwrap();
    let FailureKind::Thrown(payload) = primary.kind else { panic!("body failure kind") };
    assert_eq!(runtime.heap.text(payload), "body");
    assert_eq!(primary.secondary.len(), 2);
    assert!(!primary.trace.is_empty());
    let secondary = runtime.failure(primary.secondary[0]).unwrap();
    let FailureKind::Thrown(payload) = secondary.kind else { panic!("defer failure kind") };
    assert_eq!(runtime.heap.text(payload), "defer");
    assert!(runtime.failure(primary.secondary[1]).unwrap().cleanup_origin);
}

fn two_inputs(runtime: &mut Runtime, ctx: &mut Ctx) -> (Word, PacketSlot) {
    let owner = runtime.champ_new_owner().unwrap();
    let mut packets = [PacketSlot::default(), PacketSlot::default()];
    let mut value = 0;
    for packet in &mut packets {
        value = runtime.heap.object_new(0, &[]);
        assert_eq!(c::rt_own_create_commit(ctx, value, 0, owner.get(), packet), 0);
    }
    let mut joined = PacketSlot::default();
    assert_eq!(c::rt_own_join(ctx, packets.as_mut_ptr(), 2, &mut joined), 0);
    (value, joined)
}

#[test]
fn champ_j1_owning_entry_overflow_settles_once_before_accept_including_defer() {
    for defer in [false, true] {
        let text = if defer {
            include_str!("../../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/spec16-4.spb17")
        } else {
            include_str!("../../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/valued.spb17")
        };
        let mut runtime = load(text);
        let source = runtime.champ.machine.as_ref().unwrap().source.clone();
        let symbol = if defer { "log" } else { "main" };
        let index = source.functions.iter().position(|function| function.symbol == symbol).unwrap();
        compile(&mut runtime, index).unwrap();
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = runtime::frame_pointer!();
        let (value, input) = two_inputs(&mut runtime, &mut ctx);
        runtime.roots.push((value, Repr::Ref));
        let entry = runtime.champ.machine.as_ref().unwrap().entries[index];
        let mut post = CallPost::default();
        let mut io = CallOwn {
            input, entry_abi: entry, output: &mut post, context: 0, cleanup_mode: u64::from(defer),
        };
        SCENARIO.with(|state| *state.borrow_mut() = Scenario {
            fail_destroy: vec![2, 1], ..Scenario::default()
        });
        let marker = 0u8;
        ctx.stack_limit = &marker as *const u8 as u64 + 32768;
        let ordinal = if defer {
            let registration = c::RegistrationMap { replacements: 0 };
            c::cleanup::rt_invoke_defer(&mut ctx, entry, std::ptr::null(), 0,
                &mut io, &registration, &mut post)
        } else {
            c::cleanup::rt_call_owned(&mut ctx, entry, &value, 1, &mut io, &mut post)
        };
        assert_eq!(ordinal, 1, "{:?}", runtime.fatal);
        assert_eq!(io.input.record, 0);
        assert_eq!(ctx.status, FAILED);
        assert_eq!(post.packet.record, 0);
        assert_eq!(runtime.champ.destroy_events, vec![2, 1]);
        let primary = runtime.failure(post.failure).unwrap();
        assert!(matches!(&primary.kind, FailureKind::Program(message) if message == "Stack overflow"));
        assert_eq!(primary.secondary.len(), 2);
        for (&secondary, serial) in primary.secondary.iter().zip([2, 1]) {
            let failure = runtime.failure(secondary).unwrap();
            assert!(failure.cleanup_origin);
            let FailureKind::Thrown(payload) = failure.kind else { panic!("destroy failure kind") };
            assert_eq!(runtime.heap.text(payload), format!("destroy {serial}"));
        }
        assert_eq!(runtime.heap.champ_identity(value).unwrap().life, c::Life::Dead);
        SCENARIO.with(|state| assert_eq!(state.borrow().destroy_calls, vec![2, 1]));
        SCENARIO.with(|state| assert_eq!(state.borrow().defer_calls, 0));
    }
}

#[test]
fn champ_j1_d1_cleanup_limit_is_fatal_for_direct_and_executor() {
    use c::cleanup::*;
    let text = include_str!("../../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/local-pair.spb17");
    for direct in [false, true] {
        SCENARIO.with(|state| *state.borrow_mut() = Scenario::default());
        let mut runtime = load(text);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = runtime::frame_pointer!();
        let (_, mut packet) = two_inputs(&mut runtime, &mut ctx);
        let key = runtime.champ_packet_keys(&packet).unwrap().into_iter().max_by_key(|key| key.serial()).unwrap();
        let mut state = c::CleanupSlot::default();
        let inputs = c::CloseInputs {
            scope: std::ptr::null_mut(), packet: &mut packet, pending: std::ptr::null_mut(),
            failure: 0, settlements: 0,
        };
        assert_eq!(rt_cleanup_begin(&mut ctx, &inputs, &mut state), 0);
        assert_eq!(rt_cleanup_settle_empty(&mut ctx, &mut state, &mut state), 0);
        let plan = runtime.champ_register_cleanup_plan(c::CleanupPlan {
            reserve: false, direct, may_fail: true,
        });
        assert_eq!(rt_cleanup_plan(&mut ctx, &mut state, plan, &mut state), 0);
        let mut next = c::NextPost::default();
        if !direct {
            assert_eq!(rt_cleanup_next(&mut ctx, &mut state, &mut next), 0);
            state = next.state;
            assert_eq!(next.key, key);
        }
        let marker = 0u8;
        ctx.stack_limit = &marker as *const u8 as u64 + runtime::STACK_HEADROOM + 65536;
        let mut post = c::DestroyPost::default();
        let ordinal = if direct {
            rt_destroy_direct(&mut ctx, &mut state, key, 0, &mut post)
        } else {
            rt_cleanup_destroy(&mut ctx, &mut state, key, &mut post)
        };
        assert_eq!(ordinal, c::OUTCOME_STOP);
        assert_eq!(ctx.status, runtime::FATAL);
        assert_eq!(ctx.failure, 0, "D1 must not manufacture a catchable stack-overflow failure");
        assert_eq!(post.failure, 0);
        assert!(runtime.fatal.as_ref().unwrap().contains("headroom"));
        assert_eq!(runtime.champ.destroy_events, vec![2], "the newest identity was claimed");
        SCENARIO.with(|state| assert!(state.borrow().destroy_calls.is_empty()));
        assert!((1..512).contains(&runtime.heap.stats().collections));
    }
}

fn insert_third_creation(text: &str) -> String {
    let mut program = s::assemble(text).unwrap();
    let function = program.functions.iter_mut().find(|function| function.symbol == "main").unwrap();
    let pc = 8;
    let base = function.services.len() as u32;
    let value = function.registers.len() as u32;
    function.registers.push(s::Type::Object(0));
    function.services.extend([s::ServiceKind::Packet, s::ServiceKind::Packet,
        s::ServiceKind::Deps, s::ServiceKind::ScopeState]);
    let service = |id| L::Service(base + id);
    let additions = vec![
        s::Instruction::Champ {
            op: O::OwnEmpty, arguments: vec![], flow: s::Flow::Next(vec![service(0)]),
        },
        s::Instruction::Champ {
            op: O::InvokeCallOwn,
            arguments: vec![A::Entry(1), A::Summary(0), A::Flag(false), A::Location(service(0))],
            flow: s::Flow::Next(vec![L::Register(value), service(1), service(2)]),
        },
        s::Instruction::Champ {
            op: O::OwnAdmit,
            arguments: vec![A::Location(L::Service(10)), A::Location(service(1))],
            flow: s::Flow::Next(vec![service(3)]),
        },
    ];
    let shift = |target: &mut u32| {
        if *target >= pc { *target += 3; }
    };
    for instruction in &mut function.code {
        match instruction {
            s::Instruction::Br(edge) => shift(&mut edge.target_pc),
            s::Instruction::Cond { yes, no, .. } => {
                shift(&mut yes.target_pc);
                shift(&mut no.target_pc);
            }
            s::Instruction::Champ { flow: s::Flow::Outcomes(outcomes), .. } => {
                for outcome in outcomes {
                    shift(&mut outcome.edge.target_pc);
                }
            }
            _ => {}
        }
    }
    for block in &mut function.blocks {
        shift(&mut block.pc);
    }
    for close in &mut function.proofs.closes {
        shift(&mut close.begin_pc);
    }
    for root in function.proofs.roots.iter_mut().chain(&mut function.proofs.post_roots) {
        shift(&mut root.pc);
        if root.pc == pc + 3 {
            for location in &mut root.locations {
                if *location == L::Service(10) { *location = service(3); }
            }
        }
    }
    let s::Instruction::Champ { arguments, .. } = &mut function.code[pc as usize] else { unreachable!() };
    arguments[0] = A::Location(service(3));
    function.code.splice(pc as usize..pc as usize, additions);
    function.proofs.states = s::service_states(function).unwrap();
    let roots = [
        vec![L::Register(2), L::Service(10)],
        vec![L::Register(2), L::Service(10), service(0)],
        vec![L::Register(2), L::Service(10), L::Register(value), service(1)],
    ];
    for (offset, locations) in roots.into_iter().enumerate() {
        function.proofs.roots.push(s::RootRequirement { pc: pc + offset as u32, locations });
    }
    function.proofs.post_roots.extend([
        s::RootRequirement { pc, locations: vec![L::Register(2), L::Service(10), service(0)] },
        s::RootRequirement {
            pc: pc + 1, locations: vec![L::Register(2), L::Service(10), L::Register(value), service(1)],
        },
        s::RootRequirement { pc: pc + 2, locations: vec![L::Register(2), service(3)] },
    ]);
    s::disassemble(&program).unwrap()
}

#[test]
fn champ_j1_model_c_r_a_after_first_destroy_failure() {
    // Same verified executor CFG; one extra successful creation before the reserve is extracted.
    let text = insert_third_creation(RESERVE);
    let (runtime, ctx, post, ordinal) = run(&text, Scenario {
        fail_destroy: vec![3, 2], ..Scenario::default()
    });
    assert_eq!(ordinal, 1);
    assert_eq!(ctx.status, FAILED);
    assert_eq!(runtime.champ.destroy_events, vec![3, 2, 1]);
    assert_eq!(post.packet.record, 0);
    assert_eq!(runtime.failure(post.failure).unwrap().secondary.len(), 1);
}

#[test]
fn champ_j1_stop_is_terminal_and_does_not_dispatch_as_application_unwind() {
    let text = include_str!("../../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/spec16-4.spb17");
    let (runtime, ctx, _, result) = run(text, Scenario { stop: true, ..Scenario::default() });
    assert_eq!(result, c::OUTCOME_STOP);
    assert_eq!(ctx.status, runtime::FATAL);
    assert_eq!(ctx.failure, 0);
    assert!(runtime.champ.destroy_events.is_empty());
    SCENARIO.with(|state| assert_eq!(state.borrow().defer_calls, 0));
}

#[test]
fn champ_j1_owning_budget_overflow_preserves_stop_and_does_not_publish_post() {
    let text = include_str!("../../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/valued.spb17");
    let mut runtime = load(text);
    let index = runtime.champ.machine.as_ref().unwrap().source.functions.iter()
        .position(|function| function.symbol == "main").unwrap();
    compile(&mut runtime, index).unwrap();
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = runtime::frame_pointer!();
    let (value, packet) = two_inputs(&mut runtime, &mut ctx);
    let original = packet.record;
    let entry = runtime.champ_entry_id(index).unwrap();
    let mut post = CallPost { value: 0xfeed, ..CallPost::default() };
    let mut io = CallOwn { input: packet, entry_abi: entry, output: &mut post, context: 0, cleanup_mode: 0 };
    let marker = 0u8;
    ctx.stack_limit = &marker as *const u8 as u64 + 32768;
    runtime.budget.stack_capped = true;
    let ordinal = c::cleanup::rt_call_owned(&mut ctx, entry, &value, 1, &mut io, &mut post);
    assert_eq!(ordinal, c::OUTCOME_STOP);
    assert_eq!(ctx.status, runtime::EXHAUSTED);
    assert_eq!(ctx.failure, 0);
    assert_eq!(io.input.record, original);
    assert_eq!(post.value, 0xfeed);
    assert_eq!(post.failure, 0);
    assert!(runtime.champ.destroy_events.is_empty());
}

#[test]
fn champ_j1_compiled_defer_preserves_declaration_ordinals_and_payload_schemas() {
    let function = s::Function {
        symbol: "replacements".into(),
        entry_abi: s::EntryAbi::Defer { labels: vec![
            s::DeferLabel { label: 90, result: Some(s::Type::Int) },
            s::DeferLabel { label: 3, result: None },
        ] },
        registers: vec![], services: vec![s::ServiceKind::Packet],
        params: vec![L::Service(0)], results: vec![], blocks: vec![],
        proofs: s::FunctionProofs {
            entry: Some(s::EntryContract {
                external: Some("fixture.replacements".into()), failures: 1,
                ..s::EntryContract::default()
            }),
            ..s::FunctionProofs::default()
        },
        code: vec![],
    };
    let program = s::Program { functions: vec![function.clone()], ..s::Program::default() };
    let binding = c::ExternalEntry { declaration: function, owned: Some(external), plain: None };
    let mut runtime = Runtime::new_champ(program, vec![binding]).unwrap();
    runtime.heap.set_stress(true);
    compile(&mut runtime, 0).unwrap();
    for ordinal in 0..4 {
        SCENARIO.with(|state| *state.borrow_mut() = Scenario { replacement: ordinal, ..Scenario::default() });
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = runtime::frame_pointer!();
        let mut input = PacketSlot::default();
        assert_eq!(c::rt_own_empty(&mut ctx, &mut input), 0);
        let entry = runtime.champ_entry_id(0).unwrap();
        let mut post = CallPost::default();
        let mut io = CallOwn { input, entry_abi: entry, output: &mut post, context: 0, cleanup_mode: 1 };
        let registration = c::RegistrationMap { replacements: 2 };
        let result = c::cleanup::rt_invoke_defer(&mut ctx, entry, std::ptr::null(), 0,
            &mut io, &registration, &mut post);
        assert_eq!(result, ordinal);
        assert_eq!(io.input.record, 0);
        assert_eq!(ctx.status, if ordinal == 3 { FAILED } else { OK });
        if ordinal == 1 {
            assert_eq!(word::small_value(post.value), Some(37));
        } else {
            assert_eq!(post.value, 0);
        }
        assert_eq!(post.packet.record == 0, ordinal == 0 || ordinal == 3);
        assert_eq!(post.failure != 0, ordinal == 3);
    }
}

#[test]
fn champ_j1_owned_span_adapter_accepts_shielded_call_contract() {
    let text = include_str!("../../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/valued.spb17");
    let mut runtime = load(text);
    let index = runtime.champ.machine.as_ref().unwrap().source.functions.iter()
        .position(|function| function.symbol == "main").unwrap();
    compile(&mut runtime, index).unwrap();
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = runtime::frame_pointer!();
    let (value, input) = two_inputs(&mut runtime, &mut ctx);
    let entry = runtime.champ.machine.as_ref().unwrap().shielded_entries[index];
    assert_ne!(Some(entry), runtime.champ_entry_id(index));
    let mut post = CallPost::default();
    let mut io = CallOwn { input, entry_abi: entry, output: &mut post, context: 0, cleanup_mode: 1 };
    SCENARIO.with(|state| *state.borrow_mut() = Scenario::default());
    let ordinal = c::cleanup::rt_call_owned(&mut ctx, entry, &value, 1, &mut io, &mut post);
    assert_eq!(ordinal, 0, "{:?}", runtime.fatal);
    assert_eq!(ctx.status, OK);
    assert_eq!(post.value, value);
    assert_eq!(runtime.champ.destroy_events, vec![1]);
    assert_eq!(runtime.champ_packet_keys(&post.packet).unwrap().len(), 1);
    assert_eq!(settle(&mut ctx, &mut post.packet), 0);
    assert_eq!(runtime.champ.destroy_events, vec![1, 2]);
}

fn merge_pair() -> String {
    let text = include_str!("../../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/local-pair.spb17");
    let mut program = s::assemble(text).unwrap();
    let function = &mut program.functions[1];
    // Use the same source inventory and return value, routing both closes through the executor CFG.
    function.proofs.plans[0].merge = true;
    function.proofs.selections[0].locations.clear();
    function.proofs.order_witnesses[0].creation_pcs.clear();
    let evidence = &mut function.proofs.cleanup_plans[0];
    evidence.slots.clear();
    evidence.suffixes.clear();
    evidence.preserves.clear();
    evidence.reserve_empty = false;
    function.code[17] = s::Instruction::Br(s::Edge {
        target_pc: 38,
        assignments: vec![s::Assignment { destination: L::Service(36), source: s::Source::Location(L::Service(11)) }],
    });
    let success = s::Outcome {
        tag: s::Tag::Success,
        edge: s::Edge {
            target_pc: 23,
            assignments: vec![s::Assignment { destination: L::Service(18), source: s::Source::ProducedSlot(0) }],
        },
    };
    let failure = s::Outcome {
        tag: s::Tag::Failure,
        edge: s::Edge {
            target_pc: 24,
            assignments: vec![
                s::Assignment { destination: L::Service(19), source: s::Source::ProducedSlot(0) },
                s::Assignment { destination: L::Register(5), source: s::Source::ProducedSlot(1) },
            ],
        },
    };
    let s::Instruction::Champ { arguments, flow, .. } = &mut function.code[44] else { unreachable!() };
    arguments[4] = A::Flag(false);
    *flow = s::Flow::Outcomes(vec![success, failure]);
    function.proofs.closes.truncate(1);
    function.proofs.cleanup_plans.truncate(1);
    function.proofs.plans.truncate(1);
    // Drop the now unreachable direct plan, retaining the original instruction order for relocation.
    let retained = (0..function.code.len()).filter(|pc| !matches!(pc, 18..=22 | 25..=37)).collect::<Vec<_>>();
    let relocate = |pc: u32| retained.iter().position(|&old| old == pc as usize).map(|pc| pc as u32);
    function.code = retained.iter().map(|&pc| function.code[pc].clone()).collect();
    for instruction in &mut function.code {
        let edges = match instruction {
            s::Instruction::Br(edge) => vec![edge],
            s::Instruction::Cond { yes, no, .. } => vec![yes, no],
            s::Instruction::Champ { flow: s::Flow::Outcomes(outcomes), .. } => {
                outcomes.iter_mut().map(|outcome| &mut outcome.edge).collect()
            }
            _ => vec![],
        };
        for edge in edges {
            edge.target_pc = relocate(edge.target_pc).unwrap();
        }
    }
    function.blocks.retain_mut(|block| {
        let Some(pc) = relocate(block.pc) else { return false };
        block.pc = pc;
        true
    });
    for close in &mut function.proofs.closes {
        close.begin_pc = relocate(close.begin_pc).unwrap();
    }
    for roots in [&mut function.proofs.roots, &mut function.proofs.post_roots] {
        roots.retain_mut(|root| {
            let Some(pc) = relocate(root.pc) else { return false };
            let executor = root.pc >= 38;
            root.pc = pc;
            if executor && !root.locations.contains(&L::Register(0)) {
                root.locations.push(L::Register(0));
            }
            true
        });
    }
    function.proofs.states = s::service_states(function).unwrap();
    s::disassemble(&program).unwrap()
}

#[test]
fn champ_j2_same_close_direct_and_merge_under_forced_gc() {
    let direct = include_str!("../../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/local-pair.spb17");
    let merge = merge_pair();
    for fail_destroy in [vec![], vec![2, 1]] {
        let mut traces = Vec::new();
        for text in [direct, &merge] {
            let scenario = Scenario { fail_destroy: fail_destroy.clone(), ..Scenario::default() };
            let (runtime, ctx, _, _) = run(text, scenario);
            assert!(runtime.heap.stats().collections > 0);
            traces.push((runtime.champ.destroy_events.clone(), ctx.status,
                (ctx.failure != 0).then(|| runtime.failure(ctx.failure).unwrap().secondary.len())));
        }
        assert_eq!(traces[0], traces[1]);
        assert_eq!(traces[0].0, vec![2, 1]);
    }
}

#[test]
fn champ_j2_gc_precedes_late_scope_on_both_branches() {
    for branch in [false, true] {
        let mut program = s::assemble(&merge_pair()).unwrap();
        let mut plain = program.objects[0].clone();
        plain.name = "Plain".into();
        plain.layout.has_destroy = false;
        plain.layout.may_reach_resource = false;
        plain.layout.identity_offset = None;
        plain.layout.allocation_words = 2;
        program.objects.push(plain);
        let function = &mut program.functions[1];
        let condition = function.registers.len() as u32;
        function.registers.extend([s::Type::Bool, s::Type::Object(1), s::Type::Object(1)]);
        for instruction in &mut function.code {
            let edges = match instruction {
                s::Instruction::Br(edge) => vec![edge],
                s::Instruction::Champ { flow: s::Flow::Outcomes(outcomes), .. } => {
                    outcomes.iter_mut().map(|outcome| &mut outcome.edge).collect()
                }
                _ => vec![],
            };
            for edge in edges {
                edge.target_pc += 6;
            }
        }
        function.blocks[0].params.clear();
        for block in &mut function.blocks {
            block.pc += 6;
        }
        let mut blocks = vec![
            s::Block { pc: 0, params: function.params.clone() },
            s::Block { pc: 2, params: vec![] },
            s::Block { pc: 4, params: vec![] },
        ];
        blocks.append(&mut function.blocks);
        function.blocks = blocks;
        for close in &mut function.proofs.closes {
            close.begin_pc += 6;
        }
        for root in function.proofs.roots.iter_mut().chain(&mut function.proofs.post_roots) {
            root.pc += 6;
        }
        for pc in 0..6 {
            function.proofs.roots.push(s::RootRequirement { pc, locations: vec![L::Register(0)] });
        }
        let allocate = |output| s::Instruction::Data {
            op: s::DataOp::ObjNew, inputs: vec![], output: Some(output), immediate: s::Immediate::Object(1),
        };
        let prefix = vec![
            s::Instruction::Data {
                op: s::DataOp::Const, inputs: vec![], output: Some(condition),
                immediate: s::Immediate::Constant(s::Constant::Bool(branch)),
            },
            s::Instruction::Cond {
                condition,
                yes: s::Edge { target_pc: 2, assignments: vec![] },
                no: s::Edge { target_pc: 4, assignments: vec![] },
            },
            allocate(condition + 1),
            s::Instruction::Br(s::Edge { target_pc: 6, assignments: vec![] }),
            allocate(condition + 2),
            s::Instruction::Br(s::Edge { target_pc: 6, assignments: vec![] }),
        ];
        function.code.splice(0..0, prefix);
        function.proofs.states = s::service_states(function).unwrap();
        let text = s::disassemble(&program).unwrap();
        let (runtime, ctx, _, _) = run(&text, Scenario::default());
        assert_eq!(ctx.status, OK);
        assert_eq!(runtime.champ.destroy_events, vec![2, 1]);
    }
}

#[test]
fn champ_j2_future_result_survives_last_gc_inside_reentrant_helper() {
    let text = include_str!("../../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/fresh-call.spb17");
    let (runtime, ctx, _, _) = run(text, Scenario { collect_post: true, ..Scenario::default() });
    assert_eq!(ctx.status, OK);
    assert_eq!(runtime.champ.destroy_events, vec![2, 1]);
    assert!(runtime.roots.is_empty(), "nested Rust temporary roots must be retired");
}

#[test]
fn champ_j2_external_parameters_have_distinct_live_slots() {
    let function = s::Function {
        symbol: "two_strings".into(),
        entry_abi: s::EntryAbi::Observation,
        registers: vec![s::Type::Str, s::Type::Str], services: vec![],
        params: vec![L::Register(0), L::Register(1)], results: vec![], blocks: vec![],
        proofs: s::FunctionProofs {
            entry: Some(s::EntryContract {
                external: Some("fixture.two_strings".into()), ..s::EntryContract::default()
            }),
            ..s::FunctionProofs::default()
        },
        code: vec![],
    };
    let frame = Frame::new(&function).unwrap();
    assert_ne!(frame.allocation.locations[&L::Register(0)], frame.allocation.locations[&L::Register(1)]);
    assert_eq!(frame.before(&function, None).len(), 2);
}

#[test]
fn champ_j2_ir_maps_exclude_post_and_metadata_words() {
    use cranelift_codegen::ir::StackSlot;
    let text = include_str!("../../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/local-pair.spb17");
    let (runtime, _, _, _) = run(text, Scenario::default());
    for (&index, function) in &runtime.jit.emitted {
        let source = &runtime.champ.machine.as_ref().unwrap().source.functions[index];
        let frame = Frame::new(source).unwrap();
        let mut maps = 0;
        for block in function.layout.blocks() {
            for instruction in function.layout.block_insts(block) {
                for entry in function.dfg.user_stack_map_entries(instruction).into_iter().flatten() {
                    assert_eq!(entry.slot, StackSlot::from_u32(0), "Post and argument records are never roots");
                    assert!(frame.traced.contains(&entry.offset));
                    maps += 1;
                }
            }
        }
        assert!(maps > 0);
    }
    assert!(!runtime.jit.maps.is_empty(), "machine maps must be published to the existing frame walker");
}

fn p1_runtime() -> Box<Runtime> {
    let mut program = s::Program::default();
    for (name, fields, error_cases) in [
        ("async.atomic.Atomic_int", vec![s::Type::Int, s::Type::Bool], vec![]),
        ("async.atomic.Atomic_string", vec![s::Type::Str, s::Type::Bool], vec![]),
        ("async.atomic.AtomicClosed", vec![s::Type::Int], vec!["AtomicClosed.closed".to_string()]),
        ("app.Problem.note", vec![s::Type::Str], vec!["Problem.note".to_string()]),
        ("runtime.buffer.BufferState", vec![], vec![]),
    ] {
        let trace_layout = fields.iter().enumerate().filter_map(|(index, ty)| {
            (*ty == s::Type::Str).then_some(index as u32)
        }).collect();
        program.objects.push(s::Object {
            name: name.into(), error_cases, enum_cases: 0, variant_cases: vec![],
            handle: name == "runtime.buffer.BufferState", role: None, implements: vec![],
            fields: if name == "app.Problem.note" {
                vec![s::Field { name: "message".into(), storage: 0, once: false }]
            } else {
                vec![]
            },
            layout: s::ObjectRuntimeLayout {
                allocation_words: (1 + fields.len() as u32).max(2),
                identity_offset: None, has_destroy: false, may_reach_resource: false,
                may_async_destroy: false, storage_fields: fields, trace_layout, selection_layout: vec![],
            },
        });
    }
    let contracts = s::p1::P1Contracts::new(&program.objects).unwrap();
    // Test intrinsics belong to a test profile (`check_tests`); these programs have no tests table.
    program.functions = contracts.entries().iter().filter(|entry| !entry.symbol.starts_with(s::TEST_INTRINSIC_PREFIX))
        .cloned().collect();
    for symbol in ["runtime/utf@1::utf8_encode/1", "runtime/stdio@1::stdout_write/1",
        "runtime/buffer@1::buffer_state_create/1", "runtime/buffer@1::buffer_state_size/1",
        "runtime/buffer@1::buffer_state_closed/1", "runtime/buffer@1::buffer_state_close/1"]
    {
        program.functions.push(contracts.native(symbol).unwrap().clone());
    }
    // An unresolved provider: the checked adapter with handle transport of its own module.
    program.objects.push(s::Object {
        name: "app.Port".into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle: true,
        role: None, implements: vec![], fields: vec![],
        layout: s::ObjectRuntimeLayout {
            allocation_words: 2, identity_offset: None, has_destroy: false, may_reach_resource: false,
            may_async_destroy: false, storage_fields: vec![], trace_layout: vec![], selection_layout: vec![],
        },
    });
    let mut port = contracts.native("runtime/buffer@1::buffer_state_size/1").unwrap().clone();
    port.symbol = "app@1::port_open/1".into();
    port.proofs.entry.as_mut().unwrap().external = Some("app@1::port_open/1".into());
    port.registers = vec![s::Type::Str];
    port.results = vec![s::Type::Handle(program.objects.len() as u32 - 1)];
    program.functions.push(port);
    let mut runtime = Runtime::new_champ(program, vec![]).unwrap();
    runtime.heap.set_stress(true);
    runtime
}

fn p1_call(runtime: &mut Runtime, ctx: &mut Ctx, name: &str, args: &[Word]) -> (Word, Option<CallPost>) {
    let source = runtime.champ.machine.as_ref().unwrap().source.clone();
    let function = source.functions.iter().position(|function| {
        function.proofs.entry.as_ref().and_then(|entry| entry.external.as_deref()) == Some(name)
    }).unwrap();
    let code = compile(runtime, function).unwrap();
    if source.functions[function].entry_abi == s::EntryAbi::Observation {
        return (unsafe { invoke(code, ctx, args) }, None);
    }
    let mut post = CallPost::default();
    let mut input = PacketSlot::default();
    assert_eq!(c::rt_own_empty(ctx, &mut input), 0);
    let mut io = CallOwn {
        input, entry_abi: runtime.champ_entry_id(function).unwrap(), output: &mut post, context: 0, cleanup_mode: 0,
    };
    let entry: extern "C" fn(*mut Ctx, *mut CallOwn, *const Word, u64) -> Word =
        unsafe { std::mem::transmute(code) };
    let outcome = entry(ctx, &mut io, args.as_ptr(), args.len() as u64);
    assert_eq!(io.input.record, 0);
    (outcome, Some(post))
}

#[test]
fn champ_p1_atomic_specializations_closed_failure_and_gc() {
    for suffix in ["int", "string"] {
        let mut runtime = p1_runtime();
        let mut ctx = context(&mut runtime);
        let (first, second, repr) = if suffix == "int" {
            (runtime.heap.int(2), runtime.heap.int(7), Repr::Int)
        } else {
            (runtime.heap.allocate_text("old"), runtime.heap.allocate_text("новое"), Repr::Ref)
        };
        runtime.roots.extend([(first, repr), (second, repr)]);
        let (_, post) = p1_call(&mut runtime, &mut ctx, &format!("okkama.atomic.Atomic.new_value${suffix}"), &[first]);
        let post = post.unwrap();
        let object = post.value;
        runtime.roots.push((object, Repr::Ref));
        assert!(runtime.champ_packet_keys(&post.packet).unwrap().is_empty());
        let (outcome, post) = p1_call(&mut runtime, &mut ctx,
            &format!("okkama.atomic.Atomic.replace${suffix}"), &[object, second]);
        assert_eq!(outcome, c::NORMAL);
        assert!(runtime.champ_packet_keys(&post.unwrap().packet).unwrap().is_empty());
        let (value, post) = p1_call(&mut runtime, &mut ctx,
            &format!("okkama.atomic.Atomic.value${suffix}"), &[object]);
        assert!(post.is_none(), "value remains a borrowed observation");
        assert_eq!(value, second);
        runtime.roots.clear();
        runtime.roots.push((object, Repr::Ref));
        runtime.heap.collect(|visit| visit(object, Repr::Ref));
        if suffix == "string" {
            assert_eq!(runtime.heap.text(runtime.heap.field(object, 0)), "новое");
        }
        for _ in 0..2 {
            let (_, post) = p1_call(&mut runtime, &mut ctx,
                &format!("okkama.atomic.Atomic.close${suffix}"), &[object]);
            assert!(post.is_none(), "close does not accept ownership of the borrowed value");
            assert_eq!(ctx.status, OK);
        }
        let replacement = if suffix == "int" {
            runtime.heap.int(11)
        } else {
            runtime.heap.allocate_text("ignored")
        };
        runtime.roots.push((replacement, repr));
        for op in ["value", "replace"] {
            let args = [object, replacement];
            let count = 1 + usize::from(op == "replace");
            let (outcome, post) = p1_call(&mut runtime, &mut ctx,
                &format!("okkama.atomic.Atomic.{op}${suffix}"), &args[..count]);
            assert_eq!(ctx.status, FAILED);
            if op == "replace" {
                assert_eq!(outcome, c::UNWIND);
                let post = post.unwrap();
                assert_eq!(post.failure, ctx.failure);
                assert_eq!(post.packet.record, 0, "accepted input is settled before unwind");
            } else {
                assert!(post.is_none());
            }
            let FailureKind::Thrown(error) = runtime.failure(ctx.failure).unwrap().kind else { panic!("typed error") };
            runtime.roots.push((error, Repr::Ref));
            ctx.status = OK;
            ctx.failure = 0;
            let (text, _) = p1_call(&mut runtime, &mut ctx, "okkama.Error.text", &[error]);
            assert_eq!(runtime.heap.text(text), "AtomicClosed.closed");
            assert_eq!(runtime.heap.field(object, 0), word::UNBOUND);
        }
    }
}

#[test]
fn champ_p1_atomic_replace_preserves_old_string_borrow() {
    let mut runtime = p1_runtime();
    let mut ctx = context(&mut runtime);
    let old = runtime.heap.allocate_text("old");
    runtime.roots.push((old, Repr::Ref));
    let (_, post) = p1_call(&mut runtime, &mut ctx, "okkama.atomic.Atomic.new_value$string", &[old]);
    let object = post.unwrap().value;
    runtime.roots.push((object, Repr::Ref));
    let (borrow, _) = p1_call(&mut runtime, &mut ctx, "okkama.atomic.Atomic.value$string", &[object]);
    assert_eq!(borrow, old);
    let fresh = runtime.heap.allocate_text("new");
    runtime.roots.push((fresh, Repr::Ref));
    let (outcome, post) = p1_call(&mut runtime, &mut ctx,
        "okkama.atomic.Atomic.replace$string", &[object, fresh]);
    assert_eq!(outcome, c::NORMAL);
    assert!(runtime.champ_packet_keys(&post.unwrap().packet).unwrap().is_empty());
    runtime.heap.collect(|visit| {
        visit(object, Repr::Ref);
        visit(borrow, Repr::Ref);
    });
    assert_eq!(runtime.heap.text(borrow), "old");
    assert_eq!(runtime.heap.text(runtime.heap.field(object, 0)), "new");
    p1_call(&mut runtime, &mut ctx, "okkama.atomic.Atomic.close$string", &[object]);
    runtime.heap.collect(|visit| visit(borrow, Repr::Ref));
    assert_eq!(runtime.heap.text(borrow), "old");
}

#[test]
fn champ_p1_utf8_stdout_and_output_budget() {
    let mut runtime = p1_runtime();
    let mut ctx = context(&mut runtime);
    let text = runtime.heap.allocate_text("Привет 🌍\n");
    runtime.roots.push((text, Repr::Ref));
    let (outcome, post) = p1_call(&mut runtime, &mut ctx, "runtime/utf@1::utf8_encode/1", &[text]);
    assert_eq!(outcome, 0);
    let bytes = post.unwrap().value;
    runtime.roots.push((bytes, Repr::Ref));
    let ((outcome, post), output) = okkama_runtime_core::stdout::capture(|| {
        p1_call(&mut runtime, &mut ctx, "runtime/stdio@1::stdout_write/1", &[bytes])
    });
    assert_eq!(outcome, 0);
    assert_eq!(output, "Привет 🌍\n".as_bytes());
    assert_eq!(runtime.small_or_big(post.unwrap().value), Some(output.len() as i64));
    runtime.limits.output = Some(0);
    let (_, output) = okkama_runtime_core::stdout::capture(|| {
        p1_call(&mut runtime, &mut ctx, "runtime/stdio@1::stdout_write/1", &[bytes])
    });
    assert!(output.is_empty());
    assert_ne!(ctx.status, OK);
}

#[test]
fn champ_p1_error_text_payload_is_plain_and_rooted() {
    let mut runtime = p1_runtime();
    let mut ctx = context(&mut runtime);
    let text = runtime.heap.allocate_text("line\nquote\"");
    let object = runtime.heap.object_new(3, &[text]);
    runtime.roots.push((object, Repr::Ref));
    let (text, _) = p1_call(&mut runtime, &mut ctx, "okkama.Error.text", &[object]);
    assert_eq!(ctx.status, OK);
    assert_eq!(runtime.heap.text(text), "Problem.note(\"line\\nquote\\\"\")");
    assert!(runtime.heap.champ_identity(object).is_none());
    let fields = ["io", "failed", "stdout_write", "io failed"].map(|text| runtime.heap.allocate_text(text));
    let descriptor = runtime.champ.machine.as_ref().unwrap().native_failure;
    let object = runtime.heap.object_new(descriptor, &fields);
    runtime.roots.push((object, Repr::Ref));
    let (text, _) = p1_call(&mut runtime, &mut ctx, "okkama.Error.text", &[object]);
    assert_eq!(runtime.heap.text(text), "io failed");
}

#[test]
fn champ_p1_observation_outcomes_and_gc() {
    let template = p1_runtime();
    let mut source = (*template.champ.machine.as_ref().unwrap().source).clone();
    let target = source.functions.iter().position(|function| {
        function.proofs.entry.as_ref().unwrap().external.as_deref() == Some("okkama.atomic.Atomic.value$int")
    }).unwrap() as u32;
    let reg = s::Location::Register;
    let deps = s::Location::Service(0);
    let summary = source.tables.summaries.len() as u32;
    source.tables.summaries.push(s::Summary { entry: target, retained_params: vec![], may_fail: true });
    let caller = source.functions.len();
    source.functions.push(s::Function {
        symbol: "observe".into(), entry_abi: s::EntryAbi::Observation,
        registers: vec![s::Type::Object(0), s::Type::Int, s::Type::Failure],
        services: vec![s::ServiceKind::Deps], params: vec![reg(0), deps], results: vec![s::Type::Int],
        blocks: vec![
            s::Block { pc: 0, params: vec![reg(0), deps] },
            s::Block { pc: 1, params: vec![reg(1)] },
            s::Block { pc: 2, params: vec![reg(2)] },
        ],
        proofs: s::FunctionProofs {
            entry: Some(s::EntryContract { failures: 1, ..s::EntryContract::default() }),
            roots: (0..3).map(|pc| s::RootRequirement { pc, locations: vec![reg(pc)] }).collect(),
            ..s::FunctionProofs::default()
        },
        code: vec![
            s::Instruction::Champ {
                op: s::Opcode::InvokeObserve,
                arguments: vec![s::Argument::Entry(target), s::Argument::Summary(summary),
                    s::Argument::Location(reg(0)), s::Argument::Location(deps)],
                flow: s::Flow::Outcomes(vec![
                    s::Outcome { tag: s::Tag::Normal, edge: s::Edge {
                        target_pc: 1, assignments: vec![s::Assignment {
                            destination: reg(1), source: s::Source::ProducedSlot(0),
                        }],
                    } },
                    s::Outcome { tag: s::Tag::Unwind, edge: s::Edge {
                        target_pc: 2, assignments: vec![s::Assignment {
                            destination: reg(2), source: s::Source::ProducedSlot(0),
                        }],
                    } },
                ]),
            },
            s::Instruction::Ret(vec![1]),
            s::Instruction::Raise(2),
        ],
    });
    let mut runtime = Runtime::new_champ(source, vec![]).unwrap();
    runtime.heap.set_stress(true);
    let mut ctx = context(&mut runtime);
    let value = runtime.heap.int(7);
    let (_, post) = p1_call(&mut runtime, &mut ctx, "okkama.atomic.Atomic.new_value$int", &[value]);
    let object = post.unwrap().value;
    runtime.roots.push((object, Repr::Ref));
    let code = compile(&mut runtime, caller).unwrap();
    let value = unsafe { invoke(code, &mut ctx, &[object]) };
    assert_eq!(runtime.small_or_big(value), Some(7));
    assert_eq!(ctx.status, OK);
    p1_call(&mut runtime, &mut ctx, "okkama.atomic.Atomic.close$int", &[object]);
    unsafe { invoke(code, &mut ctx, &[object]) };
    assert_eq!(ctx.status, FAILED);
    assert!(matches!(runtime.failure(ctx.failure).unwrap().kind, FailureKind::Thrown(_)));
    assert_eq!(runtime.failure(ctx.failure).unwrap().trace.len(), 1);
}

#[test]
fn champ_plain_array_traces_elements_and_checks_all_index_forms() {
    let array = s::Type::Array(s::ArrayType { leaf: s::ArrayLeaf::Str, depth: 1 });
    let reg = L::Register;
    let mut function = s::Function {
        symbol: "array".into(), entry_abi: s::EntryAbi::Observation,
        registers: vec![s::Type::Str, s::Type::Int, array, array, s::Type::Str, s::Type::Failure],
        services: vec![], params: vec![reg(0), reg(1)], results: vec![s::Type::Str],
        blocks: vec![
            s::Block { pc: 0, params: vec![reg(0), reg(1)] },
            s::Block { pc: 3, params: vec![reg(4)] },
            s::Block { pc: 4, params: vec![reg(5)] },
        ],
        proofs: s::FunctionProofs {
            entry: Some(s::EntryContract { failures: 2, ..s::EntryContract::default() }),
            ..s::FunctionProofs::default()
        },
        code: vec![
            s::Instruction::Data { op: s::DataOp::ArrayNew, inputs: vec![0], output: Some(2),
                immediate: s::Immediate::None },
            s::Instruction::Data { op: s::DataOp::ArrayNew, inputs: vec![], output: Some(3),
                immediate: s::Immediate::None },
            s::Instruction::Champ { op: O::ArrayGet,
                arguments: vec![A::Location(reg(2)), A::Location(reg(1))],
                flow: s::Flow::Outcomes(vec![
                    s::Outcome { tag: s::Tag::Normal, edge: s::Edge {
                        target_pc: 3, assignments: vec![s::Assignment {
                            destination: reg(4), source: s::Source::ProducedSlot(0),
                        }],
                    } },
                    s::Outcome { tag: s::Tag::Unwind, edge: s::Edge {
                        target_pc: 4, assignments: vec![s::Assignment {
                            destination: reg(5), source: s::Source::ProducedSlot(0),
                        }],
                    } },
                ]) },
            s::Instruction::Ret(vec![4]),
            s::Instruction::Raise(5),
        ],
    };
    function.proofs.roots = s::live_locations(&function).unwrap().into_iter().enumerate()
        .map(|(pc, locations)| s::RootRequirement { pc: pc as u32, locations: locations.into_iter().collect() })
        .collect();
    let source = s::Program { functions: vec![function], ..s::Program::default() };
    let mut runtime = Runtime::new_champ(source, vec![]).unwrap();
    runtime.heap.set_stress(true);
    let mut ctx = context(&mut runtime);
    let code = compile(&mut runtime, 0).unwrap();
    for index in ["0", "-1", "1", "999999999999999999999999999999999999999999"] {
        ctx.status = OK;
        ctx.failure = 0;
        let text = runtime.heap.allocate_text("живой элемент");
        let index_word = runtime.heap.big(index.parse().unwrap());
        let value = unsafe { invoke(code, &mut ctx, &[text, index_word]) };
        if index == "0" {
            assert_eq!(ctx.status, OK);
            assert_eq!(runtime.heap.text(value), "живой элемент");
        } else {
            assert_eq!(ctx.status, FAILED);
            let failure = runtime.failure(ctx.failure).unwrap();
            assert!(matches!(&failure.kind, FailureKind::Program(message) if message == "Array index out of bounds"));
            assert_eq!(failure.trace.len(), 1);
        }
    }
    assert!(runtime.heap.stats().collections > 0);
}

/// S1 data form: `left op right`, then a use of the result that would misread a failed helper's zero.
fn s1_fallible(op: s::DataOp, result: s::Type, immediate: s::Immediate, use_op: s::DataOp) -> s::Function {
    let reg = L::Register;
    let mut function = s::Function {
        symbol: "fallible".into(), entry_abi: s::EntryAbi::Observation,
        registers: vec![s::Type::Int, s::Type::Int, result, s::Type::Int],
        services: vec![], params: vec![reg(0), reg(1)], results: vec![s::Type::Int],
        blocks: vec![s::Block { pc: 0, params: vec![reg(0), reg(1)] }],
        proofs: s::FunctionProofs {
            entry: Some(s::EntryContract { failures: 2, ..s::EntryContract::default() }),
            ..s::FunctionProofs::default()
        },
        code: vec![
            s::Instruction::Data { op, inputs: vec![0, 1], output: Some(2), immediate },
            s::Instruction::Data { op: use_op, inputs: vec![2], output: Some(3), immediate: s::Immediate::None },
            s::Instruction::Ret(vec![3]),
        ],
    };
    function.proofs.roots = s::live_locations(&function).unwrap().into_iter().enumerate()
        .map(|(pc, locations)| s::RootRequirement { pc: pc as u32, locations: locations.into_iter().collect() })
        .collect();
    function
}

#[test]
fn champ_s1_fallible_data_leaves_like_a_failing_call() {
    let cases = [
        (s1_fallible(s::DataOp::RangeNew, s::Type::Range, s::Immediate::Inclusive(false), s::DataOp::RangeStart),
            "Range start must be less than or equal to range end"),
        (s1_fallible(s::DataOp::IntMod, s::Type::Int, s::Immediate::None, s::DataOp::IntNeg), "Modulo by zero"),
    ];
    for (function, message) in cases {
        let source = s::Program { functions: vec![function], ..s::Program::default() };
        let mut runtime = Runtime::new_champ(source, vec![]).unwrap();
        runtime.heap.set_stress(true);
        let mut ctx = context(&mut runtime);
        let code = compile(&mut runtime, 0).unwrap();
        let (one, three) = (runtime.heap.int(1), runtime.heap.int(3));
        let value = unsafe { invoke(code, &mut ctx, &[one, three]) };
        assert_eq!(ctx.status, OK);
        assert!(runtime.small_or_big(value).is_some());
        let zero = runtime.heap.int(0);
        let right = if message == "Modulo by zero" { zero } else { one };
        unsafe { invoke(code, &mut ctx, &[three, right]) };
        assert_eq!(ctx.status, FAILED);
        assert!(runtime.fatal.is_none(), "{:?}", runtime.fatal);
        let failure = runtime.failure(ctx.failure).unwrap();
        assert!(matches!(&failure.kind, FailureKind::Program(text) if text == message), "{:?}", failure.kind);
    }
}

#[test]
fn champ_native_handles_are_registry_values_under_collection() {
    let mut runtime = p1_runtime();
    let mut ctx = context(&mut runtime);
    let initial = runtime.heap.allocate_bytes(b"ab");
    runtime.roots.push((initial, Repr::Ref));
    let (outcome, post) = p1_call(&mut runtime, &mut ctx, "runtime/buffer@1::buffer_state_create/1", &[initial]);
    assert_eq!(outcome, c::NORMAL);
    let post = post.unwrap();
    assert!(runtime.champ_packet_keys(&post.packet).unwrap().is_empty(), "a handle carries no destroy token");
    let handle = post.value;
    runtime.roots.clear();
    runtime.roots.push((handle, Repr::Ref));
    runtime.heap.collect(|visit| visit(handle, Repr::Ref));
    let (_, post) = p1_call(&mut runtime, &mut ctx, "runtime/buffer@1::buffer_state_size/1", &[handle]);
    assert_eq!(runtime.small_or_big(post.unwrap().value), Some(2));
    // Tomato parity: close is idempotent and reports whether this call closed the state.
    for expected in [1, 0] {
        let (_, post) = p1_call(&mut runtime, &mut ctx, "runtime/buffer@1::buffer_state_close/1", &[handle]);
        assert_eq!(post.unwrap().value, expected);
    }
    let (_, post) = p1_call(&mut runtime, &mut ctx, "runtime/buffer@1::buffer_state_closed/1", &[handle]);
    assert_eq!(post.unwrap().value, 1);
    let (outcome, post) = p1_call(&mut runtime, &mut ctx, "runtime/buffer@1::buffer_state_size/1", &[handle]);
    assert_eq!((outcome, ctx.status), (c::UNWIND, FAILED));
    let post = post.unwrap();
    assert_eq!(post.packet.record, 0, "accepted input is settled before unwind");
    assert!(matches!(runtime.failure(post.failure).unwrap().kind, FailureKind::Native(_)));
}

#[test]
fn champ_unresolved_native_fails_as_a_missing_provider() {
    let mut runtime = p1_runtime();
    let mut ctx = context(&mut runtime);
    let name = runtime.heap.allocate_text("x");
    runtime.roots.push((name, Repr::Ref));
    let (outcome, post) = p1_call(&mut runtime, &mut ctx, "app@1::port_open/1", &[name]);
    assert_eq!((outcome, ctx.status), (c::UNWIND, FAILED));
    let post = post.unwrap();
    assert_eq!(post.packet.record, 0);
    let FailureKind::Native(payload) = runtime.failure(post.failure).unwrap().kind else { panic!("native failure") };
    runtime.roots.push((payload, Repr::Ref));
    ctx.status = OK;
    ctx.failure = 0;
    let (text, _) = p1_call(&mut runtime, &mut ctx, "okkama.Error.text", &[payload]);
    assert_eq!(runtime.heap.text(text), "Native symbol \"app@1::port_open/1\" is not registered");
}
