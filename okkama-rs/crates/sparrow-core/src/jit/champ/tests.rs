use super::*;
use c::{CallOwn, CallPost, PacketSlot};
use crate::runtime::FailureKind;
use std::cell::RefCell;

#[derive(Default)]
struct Scenario {
    fail_destroy: Vec<u64>,
    risky: bool,
    defer: bool,
    defer_calls: usize,
    stop: bool,
    replacement: u64,
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
    Runtime::new_champ(program, external).unwrap()
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
        let (value, input) = two_inputs(&mut runtime, &mut ctx);
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
        SCENARIO.with(|state| assert_eq!(state.borrow().defer_calls, 0));
    }
}

#[test]
fn champ_j1_d1_cleanup_limit_is_fatal_for_direct_and_executor() {
    use c::cleanup::*;
    let text = include_str!("../../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/local-pair.spb17");
    for direct in [false, true] {
        let mut runtime = load(text);
        let mut ctx = context(&mut runtime);
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
    compile(&mut runtime, 0).unwrap();
    for ordinal in 0..4 {
        SCENARIO.with(|state| *state.borrow_mut() = Scenario { replacement: ordinal, ..Scenario::default() });
        let mut ctx = context(&mut runtime);
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
