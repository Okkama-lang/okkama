//! #601 V7: plain async from Low OIR-2 to SPB 17. The inputs are the async Low goldens of okkama-oir (V4); the
//! expected forms are the async ABI (C-1…C-3, D20, D21) and Champ draft 3 §14, as the V6 positives build them.
use super::*;
use s::{Argument as A, Location as L, Opcode as O, ProducedType as P, ServiceKind as K};

/// Plain layouts of the goldens' declarations: objects without destroy and Error enums.
fn layouts(module: &o::Module) -> BTreeMap<u32, ObjectLayout> {
    let mut layouts = BTreeMap::new();
    for (id, declaration) in module.declarations.iter() {
        let (storage_fields, fields) = match &declaration.kind {
            o::DeclarationKind::Object { fields, destroy: None, .. } => {
                let storage = fields.iter().map(|field| translate_type(module, field.ty).unwrap()).collect::<Vec<_>>();
                (storage, fields.len() as u32)
            }
            o::DeclarationKind::Enum { .. } => (vec![s::Type::Int], 0),
            kind => panic!("{}: no test layout for {kind:?}", declaration.name),
        };
        let trace_layout = storage_fields.iter().enumerate()
            .filter_map(|(index, ty): (usize, &s::Type)| ty.traced().then_some(index as u32)).collect();
        layouts.insert(id.as_u32(), ObjectLayout {
            runtime: s::ObjectRuntimeLayout {
                allocation_words: (1 + storage_fields.len() as u32).max(2),
                identity_offset: None,
                has_destroy: false,
                may_reach_resource: false,
                may_async_destroy: false,
                storage_fields,
                trace_layout,
                selection_layout: vec![],
            },
            source_fields: (0..fields).collect(),
        });
    }
    layouts
}

fn source(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../okkama-oir/tests/fixtures/async/low").join(format!("{name}.oir"));
    std::fs::read_to_string(path).unwrap()
}

fn golden(name: &str) -> o::Module {
    o::text::parse(&source(name)).unwrap_or_else(|error| panic!("{name}: {error:?}"))
}

/// The translated program, before SPB 17's structural verification.
fn translate(module: &o::Module) -> Result<s::Program, GenerateError> {
    with_composites(module, || translate_program(module, &layouts(module)))
}

fn translated(module: &o::Module) -> s::Program {
    translate(module).unwrap_or_else(|error| panic!("{}: {error}", module.name))
}

/// The executable gate: verify_low, generation and the load check.
fn compile(module: &o::Module) -> Result<AllocatedProgram, GenerateError> {
    let layouts = layouts(module);
    let objects = with_composites(module, || objects(module, &layouts))?;
    let contracts = s::p1::P1Contracts::new(&objects)?;
    compile_low(module, &layouts, o::VerifyContext::default(), &contracts)
}

fn function<'p>(program: &'p s::Program, symbol: &str) -> &'p s::Function {
    program.functions.iter().find(|function| function.symbol == symbol)
        .unwrap_or_else(|| panic!("no function {symbol}"))
}

fn index(program: &s::Program, symbol: &str) -> u32 {
    program.functions.iter().position(|function| function.symbol == symbol).unwrap() as u32
}

/// Every Champ instruction of `op` with its pc, arguments and flow.
fn sites(f: &s::Function, op: O) -> Vec<(usize, &[s::Argument], &s::Flow)> {
    f.code.iter().enumerate().filter_map(|(pc, instruction)| match instruction {
        s::Instruction::Champ { op: found, arguments, flow } if *found == op => Some((pc, arguments.as_slice(), flow)),
        _ => None,
    }).collect()
}

fn outputs(flow: &s::Flow) -> &[L] {
    let s::Flow::Next(outputs) = flow else { panic!("expected a straight-line operation") };
    outputs
}

fn ty(f: &s::Function, location: L) -> P {
    match location {
        L::Register(id) => P::Value(f.registers[id as usize]),
        L::Service(id) => P::Service(f.services[id as usize]),
    }
}

fn types(f: &s::Function, arguments: &[s::Argument]) -> Vec<P> {
    arguments.iter().filter_map(|argument| match argument {
        A::Location(location) => Some(ty(f, *location)),
        _ => None,
    }).collect()
}

fn value(ty: s::Type) -> s::FrameSlot {
    s::FrameSlot { ty: P::Value(ty), linear: false }
}

fn service(kind: K) -> s::FrameSlot {
    s::FrameSlot { ty: P::Service(kind), linear: kind.linear() }
}

fn continuation(site: Option<usize>, saved: &[u32], operands: &[u32]) -> s::Continuation {
    s::Continuation { site: site.map(|pc| pc as u32), saved: saved.to_vec(), operands: operands.to_vec() }
}

const FUTURE_INT: s::Type = s::Type::Future(0);

#[test]
fn v7_async_body_takes_its_frame_and_publishes_its_outcome() {
    let p = translated(&golden("async-plain-await"));
    let work = function(&p, "work");
    assert_eq!(work.entry_abi, s::EntryAbi::AsyncBody { layout: 0 });
    assert_eq!(work.params.len(), 1, "an async body takes exactly its frame (C-2)");
    assert_eq!(ty(work, work.params[0]), P::Service(K::Frame));
    assert_eq!(work.results, [s::Type::Int], "the results of an async body are its Future's outcome");
    // async main publishes its exit status.
    let main = function(&p, "main");
    assert_eq!(main.entry_abi, s::EntryAbi::AsyncBody { layout: 1 });
    assert_eq!(main.results, [s::Type::Int]);
    assert_eq!(p.entry, Some(index(&p, "main")));
    // A void Future has no outcome value.
    let p = translated(&golden("async-void"));
    assert_eq!(function(&p, "note").results, []);
}

#[test]
fn v7_future_results_hold_each_outcome_once() {
    let p = translated(&golden("async-plain-await"));
    assert_eq!(p.tables.future_results, [Some(s::Type::Int)], "future<int> of fut.new, await and the layout");
    let p = translated(&golden("async-void"));
    let mut outcomes = p.tables.future_results.clone();
    outcomes.sort_by_key(|outcome| outcome.is_some());
    assert_eq!(outcomes, [None, Some(s::Type::Int)], "future<void> and future<int>, each once");
}

#[test]
fn v7_frame_layout_is_the_d21_layout_with_site_pcs() {
    let p = translated(&golden("async-plain-await"));
    let work = &p.tables.frames[0];
    assert_eq!(work.slots, [value(s::Type::Int), service(K::Packet)]);
    assert_eq!(work.continuations, [continuation(None, &[0, 1], &[])]);
    let main = function(&p, "main");
    let layout = &p.tables.frames[1];
    assert_eq!(layout.slots, [service(K::ScopeState), service(K::Deps), service(K::CacheState), value(FUTURE_INT),
        value(s::Type::Failure), service(K::TaskState), service(K::Receipt)]);
    let [(awaited, ..)] = sites(main, O::Await)[..] else { panic!("one await") };
    let [(adapter, ..)] = sites(main, O::AwaitUnwind)[..] else { panic!("one await.unwind") };
    assert_eq!(layout.continuations, [
        continuation(None, &[], &[]),
        continuation(Some(awaited), &[0, 1, 2], &[3]),
        continuation(Some(adapter), &[2], &[0, 4, 5, 6]),
    ], "each continuation's site is the pc of its suspending operation");
}

#[test]
fn v7_future_new_names_its_body_owner_and_future_type() {
    let p = translated(&golden("async-plain-await"));
    let main = function(&p, "main");
    let [(_, arguments, flow)] = sites(main, O::FutureNew)[..] else { panic!("one fut.new") };
    assert_eq!(arguments[..3], [A::Entry(index(&p, "work")), A::Scope(1), A::Result(Some(FUTURE_INT))]);
    // The receiver and arguments, then the inputs packet, its deps and the owner state (C-2, draft 3 §14.4).
    assert_eq!(types(main, arguments), [P::Value(s::Type::Int), P::Service(K::Packet), P::Service(K::Deps),
        P::Service(K::ScopeState)]);
    let created = outputs(flow).iter().map(|&location| ty(main, location)).collect::<Vec<_>>();
    assert_eq!(created, [P::Value(FUTURE_INT), P::Service(K::CacheState), P::Service(K::ScopeState)]);
}

#[test]
fn v7_await_has_normal_and_unwind_outcomes_that_resume_first() {
    let p = translated(&golden("async-plain-await"));
    let main = function(&p, "main");
    let [(pc, arguments, flow)] = sites(main, O::Await)[..] else { panic!("one await") };
    assert_eq!(types(main, arguments), [P::Value(FUTURE_INT)]);
    let s::Flow::Outcomes(outcomes) = flow else { panic!("await is an invocation") };
    assert_eq!(outcomes.iter().map(|outcome| outcome.tag).collect::<Vec<_>>(), [s::Tag::Normal, s::Tag::Unwind]);
    let produced = |outcome: &s::Outcome| outcome.edge.assignments.iter()
        .filter(|assignment| matches!(assignment.source, s::Source::ProducedSlot(_)))
        .map(|assignment| ty(main, assignment.destination)).collect::<Vec<_>>();
    assert_eq!(produced(&outcomes[0]), [P::Value(s::Type::Int), P::Service(K::Deps)]);
    assert_eq!(produced(&outcomes[1]), [P::Value(s::Type::Failure), P::Service(K::TaskState),
        P::Service(K::Receipt)]);
    // Form (a): frame.save of the same continuation right before the site; each outcome block resumes it first.
    let s::Instruction::Champ { op: O::FrameSave, arguments: save, flow: saved } = &main.code[pc - 1] else {
        panic!("await without its frame.save");
    };
    assert_eq!(save[1], A::Continuation(1));
    let frame = outputs(saved)[0];
    for outcome in outcomes {
        let s::Instruction::Champ { op: O::FrameResume, arguments, .. } = &main.code[outcome.edge.target_pc as usize]
        else {
            panic!("an await outcome must enter its frame.resume");
        };
        assert_eq!(arguments.as_slice(), [A::Location(frame), A::Continuation(1)]);
    }
}

#[test]
fn v7_frame_save_and_resume_carry_the_continuation_slots_in_layout_order() {
    let p = translated(&golden("async-plain-await"));
    let main = function(&p, "main");
    let layout = &p.tables.frames[1];
    for (_, arguments, flow) in sites(main, O::FrameSave) {
        let A::Continuation(k) = arguments[1] else { panic!("frame.save without its continuation") };
        assert_eq!(types(main, &arguments[..1]), [P::Service(K::Frame)]);
        assert_eq!(types(main, &arguments[2..]), layout.saved_types(k).unwrap(), "continuation {k}");
        assert_eq!(outputs(flow).iter().map(|&location| ty(main, location)).collect::<Vec<_>>(),
            [P::Service(K::Frame)]);
    }
    for (_, arguments, flow) in sites(main, O::FrameResume) {
        let [A::Location(_), A::Continuation(k)] = arguments else { panic!("frame.resume takes the frame and k") };
        let mut expected = vec![P::Service(K::Frame)];
        expected.extend(layout.saved_types(*k).unwrap());
        assert_eq!(outputs(flow).iter().map(|&location| ty(main, location)).collect::<Vec<_>>(), expected);
    }
}

#[test]
fn v7_await_unwind_is_bracketed_and_consumes_task_and_receipt() {
    let p = translated(&golden("async-plain-await"));
    let main = function(&p, "main");
    let [(pc, arguments, flow)] = sites(main, O::AwaitUnwind)[..] else { panic!("one await.unwind") };
    assert_eq!(types(main, arguments), [P::Service(K::ScopeState), P::Value(s::Type::Failure),
        P::Service(K::TaskState), P::Service(K::Receipt)]);
    assert_eq!(outputs(flow).iter().map(|&location| ty(main, location)).collect::<Vec<_>>(),
        [P::Service(K::ScopeState), P::Value(s::Type::Failure)]);
    // Form (b): save, the adapter and resume are consecutive.
    let continuation = |pc: usize, op: O| match &main.code[pc] {
        s::Instruction::Champ { op: found, arguments, .. } if *found == op => arguments.get(1).cloned(),
        _ => None,
    };
    assert_eq!(continuation(pc - 1, O::FrameSave), Some(A::Continuation(2)));
    assert_eq!(continuation(pc + 1, O::FrameResume), Some(A::Continuation(2)));
}

#[test]
fn v7_publish_success_with_and_without_value_and_failure() {
    let p = translated(&golden("async-plain-await"));
    let work = function(&p, "work");
    let published = sites(work, O::FuturePublish);
    let success = published.iter().find(|(_, arguments, _)| arguments[1] == A::Flag(true)).unwrap();
    assert_eq!(types(work, success.1), [P::Service(K::Frame), P::Value(s::Type::Int), P::Service(K::Packet),
        P::Service(K::Deps)]);
    let failure = published.iter().find(|(_, arguments, _)| arguments[1] == A::Flag(false)).unwrap();
    assert_eq!(types(work, failure.1), [P::Service(K::Frame), P::Value(s::Type::Failure)]);
    // A void Future publishes its packet and deps only (ABI OQ-5).
    let p = translated(&golden("async-void"));
    let note = function(&p, "note");
    let success = sites(note, O::FuturePublish).into_iter().find(|(_, arguments, _)| arguments[1] == A::Flag(true))
        .unwrap();
    assert_eq!(types(note, success.1), [P::Service(K::Frame), P::Service(K::Packet), P::Service(K::Deps)]);
    // Every path ends in fut.publish then ret, and the ret returns nothing.
    for f in &p.functions {
        if !matches!(f.entry_abi, s::EntryAbi::AsyncBody { .. }) {
            continue;
        }
        for (pc, instruction) in f.code.iter().enumerate() {
            if let s::Instruction::Ret(values) = instruction {
                assert!(values.is_empty(), "{}", f.symbol);
                assert!(matches!(f.code[pc - 1], s::Instruction::Champ { op: O::FuturePublish, .. }), "{}", f.symbol);
            }
        }
    }
}

/// never-awaited's sync main with a cancellation request and both observations of its Future.
fn observed() -> o::Module {
    let text = source("async-never-awaited");
    let anchor = "    [i5] %10: deps = deps.of %7\n";
    assert!(text.contains(anchor));
    let observations = "    %90: bool = fut.done %7\n    fut.cancel %7\n    %91: bool = fut.cancelled %7\n";
    o::text::parse(&text.replace(anchor, &format!("{anchor}{observations}"))).unwrap()
}

#[test]
fn v7_future_observations_and_cancel_request_read_the_future() {
    let p = translated(&observed());
    let main = function(&p, "main");
    for (op, result) in [(O::FutureDone, true), (O::FutureCancel, false), (O::FutureCancelled, true)] {
        let [(_, arguments, flow)] = sites(main, op)[..] else { panic!("one {op:?}") };
        assert_eq!(types(main, arguments), [P::Value(s::Type::Future(0))], "{op:?}");
        let expected = if result { vec![P::Value(s::Type::Bool)] } else { vec![] };
        assert_eq!(outputs(flow).iter().map(|&location| ty(main, location)).collect::<Vec<_>>(), expected);
    }
}

#[test]
fn v7_settle_works_through_the_schedule_duty_by_duty() {
    let p = translated(&golden("async-never-awaited"));
    let main = function(&p, "main");
    let [(_, arguments, flow)] = sites(main, O::CleanupSettle)[..] else { panic!("one settle") };
    let [A::Location(cleanup), A::Schedule(schedule), A::Location(duty)] = arguments else {
        panic!("a duty settle names its schedule and duty: {arguments:?}");
    };
    assert_eq!(ty(main, *cleanup), P::Service(K::Cleanup));
    assert_eq!(ty(main, *duty), P::Service(K::CacheState));
    assert_eq!(main.proofs.schedules[*schedule as usize].services, [*duty]);
    assert_eq!(outputs(flow).iter().map(|&location| ty(main, location)).collect::<Vec<_>>(), [P::Service(K::Cleanup)]);
    // An empty schedule keeps the one-step form.
    let explode = function(&p, "explode");
    let [(_, arguments, _)] = sites(explode, O::CleanupSettle)[..] else { panic!("one settle") };
    let [A::Location(_), A::Schedule(schedule)] = arguments else { panic!("empty settle form: {arguments:?}") };
    assert!(explode.proofs.schedules[*schedule as usize].services.is_empty());
}

#[test]
fn v7_final_close_delivers_cancellation_under_the_envelope_bit() {
    let p = translated(&golden("async-plain-await"));
    let work = function(&p, "work");
    let [(_, arguments, flow)] = sites(work, O::CleanupDeliverCancel)[..] else { panic!("one delivery") };
    assert_eq!(types(work, arguments), [P::Service(K::Cleanup)]);
    assert_eq!(outputs(flow).iter().map(|&location| ty(work, location)).collect::<Vec<_>>(), [P::Service(K::Cleanup)]);
    let close = &work.proofs.closes[0];
    assert_eq!(work.proofs.envelopes[close.envelope as usize].failures, s::ENVELOPE_CANCEL);
    // A delivered cancellation fails the close: the dispatch declares it may fail.
    let [(_, arguments, _)] = sites(work, O::ExitDispatch)[..] else { panic!("one dispatch") };
    assert_eq!(arguments[3..], [A::Flag(true), A::Flag(false), A::Flag(true)]);
    // Without cancel=true the envelope keeps its failures only.
    let p = translated(&golden("async-never-awaited"));
    let main = function(&p, "main");
    assert!(main.proofs.envelopes.iter().all(|envelope| envelope.failures & s::ENVELOPE_CANCEL == 0));
}

#[test]
fn v7_suspension_roots_stand_before_the_site() {
    let p = translated(&golden("async-plain-await"));
    let main = function(&p, "main");
    let [(pc, arguments, _)] = sites(main, O::Await)[..] else { panic!("one await") };
    let A::Location(future) = arguments[0] else { panic!("await operand") };
    let rooted = main.proofs.roots.iter().filter(|root| root.pc == pc as u32)
        .any(|root| root.locations.contains(&future));
    assert!(rooted, "the awaited Future stays rooted while the frame is suspended");
}

#[test]
fn v7_duty_tokens_are_rooted_after_the_operation_that_leaves_them_active() {
    let p = translated(&golden("async-never-awaited"));
    let main = function(&p, "main");
    let [(pc, _, flow)] = sites(main, O::FutureNew)[..] else { panic!("one fut.new") };
    let cache = outputs(flow)[1];
    let after = main.proofs.post_roots.iter().filter(|root| root.pc == pc as u32)
        .flat_map(|root| &root.locations).collect::<Vec<_>>();
    assert!(after.contains(&&cache), "the cache-state word refers to the Future frame: {after:?}");
}

/// A function with one cache-state token produced by `own.empty`-like straight-line code and live afterwards.
fn duty_function() -> s::Function {
    s::Function {
        symbol: "duty".into(),
        entry_abi: s::EntryAbi::Observation,
        registers: vec![s::Type::Int],
        services: vec![K::CacheState, K::Packet, K::CacheState],
        params: vec![],
        results: vec![],
        blocks: vec![s::Block { pc: 0, params: vec![] }],
        proofs: s::FunctionProofs {
            states: vec![
                s::StateRecord { pc: 1, locations: vec![L::Service(0), L::Service(1)] },
                s::StateRecord { pc: 2, locations: vec![L::Service(0)] },
            ],
            ..s::FunctionProofs::default()
        },
        code: vec![
            s::Instruction::Champ { op: O::OwnEmpty, arguments: vec![], flow: s::Flow::Next(vec![L::Service(1)]) },
            s::Instruction::Br(s::Edge { target_pc: 2, assignments: vec![] }),
            s::Instruction::Ret(vec![]),
        ],
    }
}

#[test]
fn v7_root_duty_words_adds_only_active_duty_tokens_after_straight_line_operations() {
    let mut f = duty_function();
    root_duty_words(&mut f);
    // After pc 0 the duty Service(0) is active; the packet has its Low trace and is not a duty.
    assert_eq!(f.proofs.post_roots, [s::RootRequirement { pc: 0, locations: vec![L::Service(0)] }]);
    // A branch has no post map; its successor's maps come from the states before each pc.
    assert!(f.proofs.post_roots.iter().all(|root| root.pc != 1));
    // An existing post map is extended in place, without repeating a location.
    let mut f = duty_function();
    f.proofs.post_roots.push(s::RootRequirement { pc: 0, locations: vec![L::Register(0), L::Service(0)] });
    root_duty_words(&mut f);
    assert_eq!(f.proofs.post_roots, [s::RootRequirement { pc: 0, locations: vec![L::Register(0), L::Service(0)] }]);
}

#[test]
fn v7_root_dependency_anchors_add_the_traced_anchors_of_rooted_locations() {
    let mut f = duty_function();
    // Service(1) is a deps-like location over the int register; a bool anchor takes no root.
    f.registers.push(s::Type::Bool);
    f.proofs.dependencies = vec![
        s::Dependency { value: L::Service(1), obligations: vec![L::Register(0), L::Register(1)] },
        s::Dependency { value: L::Service(2), obligations: vec![L::Register(0)] },
    ];
    let mut locations = BTreeSet::from([L::Service(1)]);
    root_dependency_anchors(&f, &mut locations);
    assert_eq!(locations, BTreeSet::from([L::Service(1), L::Register(0)]));
    // Anchors of locations outside the map stay out.
    let mut locations = BTreeSet::from([L::Service(0)]);
    root_dependency_anchors(&f, &mut locations);
    assert_eq!(locations, BTreeSet::from([L::Service(0)]));
}

/// Edits `module`'s function `symbol`.
fn edit(module: &mut o::Module, symbol: &str, change: impl FnOnce(&mut o::Function)) {
    let (_, function) = module.functions.iter_mut().find(|(_, function)| function.symbol == symbol).unwrap();
    change(function);
}

fn rejected(module: &o::Module, expected: &str) {
    let failure = translate(module).expect_err(expected).to_string();
    assert!(failure.contains(expected), "expected {expected:?}: {failure}");
}

#[test]
fn v7_async_body_without_its_layout_is_rejected() {
    let mut module = golden("async-plain-await");
    edit(&mut module, "work", |work| work.body.as_mut().unwrap().frame_layout = None);
    rejected(&module, "async body without its D21 frame layout");
}

#[test]
fn v7_async_body_takes_its_own_frame_only() {
    let mut module = golden("async-plain-await");
    let main = module.functions.iter().find(|(_, f)| f.symbol == "main").unwrap().1.signature.params.clone();
    edit(&mut module, "work", |work| work.signature.params = main);
    rejected(&module, "an async body takes exactly its own frame and returns nothing");
}

#[test]
fn v7_layout_or_async_summary_on_a_synchronous_function_is_rejected() {
    let mut module = golden("async-plain-await");
    let layout = module.functions.iter().find(|(_, f)| f.symbol == "work").unwrap().1.body.as_ref().unwrap()
        .frame_layout.clone();
    let mut sync = golden("async-never-awaited");
    edit(&mut sync, "main", |main| main.body.as_mut().unwrap().frame_layout = layout);
    rejected(&sync, "async metadata on a synchronous function");
    edit(&mut module, "work", |work| work.attrs.retain(|attr| *attr != o::FunctionAttr::Async));
    rejected(&module, "async metadata on a synchronous function");
}

#[test]
fn v7_creation_summary_must_return_the_bodys_future() {
    let mut module = golden("async-void");
    let int_future = module.functions.iter().find(|(_, f)| f.symbol == "main").unwrap().1.async_creation_summary;
    edit(&mut module, "note", |note| note.async_creation_summary = int_future);
    rejected(&module, "creation summary's Future differs from the body's outcome");
    let mut module = golden("async-void");
    edit(&mut module, "note", |note| note.async_creation_summary = None);
    rejected(&module, "async body without its creation and body summaries");
}

#[test]
fn v7_frame_save_out_of_layout_order_is_rejected() {
    let mut module = golden("async-plain-await");
    edit(&mut module, "main", |main| {
        let body = main.body.as_mut().unwrap();
        let save = body.insts.iter_mut().find_map(|(_, inst)| match &mut inst.op {
            o::Operation::Async(o::AsyncOp::FrameSave { slots, continuation: 1, .. }) => Some(slots),
            _ => None,
        }).unwrap();
        save.swap(0, 1);
    });
    rejected(&module, "frame.save differs from continuation 1 of the layout");
}

#[test]
fn v7_settle_of_a_duty_outside_its_close_schedule_is_rejected() {
    let mut module = golden("async-plain-await");
    edit(&mut module, "main", |main| {
        let cleanup = &mut main.body.as_mut().unwrap().cleanup;
        let holders = cleanup.schedules.iter().map(|(_, schedule)| schedule.holders.clone()).collect::<Vec<_>>();
        // Each close's schedule now lists the other close's duty.
        for ((_, schedule), holders) in cleanup.schedules.iter_mut().zip(holders.into_iter().rev()) {
            schedule.holders = holders;
        }
    });
    rejected(&module, "cleanup.settle settles a duty its close does not schedule");
}

#[test]
fn v7_only_the_services_protocol_family_is_accepted() {
    let module = golden("async-plain-await");
    translated(&module);
    let mut other = module.clone();
    for (_, protocol) in other.protocols.iter_mut() {
        protocol.kind = o::ProtocolKind::Closure;
    }
    rejected(&other, "unresolved declarations, protocols or service metadata");
}

#[test]
fn v7_a_module_that_requires_async_is_translated() {
    let mut module = golden("async-plain-await");
    module.requires = vec![o::Feature::Core, o::Feature::Async];
    translated(&module);
    module.requires.push(o::Feature::Process);
    rejected(&module, "unresolved declarations, protocols or service metadata");
}

/// The gate of every SPB 17 program: generation, verify_load, and its codec and assembly round trips.
fn loads(name: &str) {
    let output = compile(&golden(name)).unwrap_or_else(|error| panic!("{name}: {error}"));
    let program = &output.program;
    let bytes = s::encode(program).unwrap();
    let decoded = s::decode(&bytes).unwrap();
    assert_eq!(&decoded, program, "{name}: codec round trip");
    assert_eq!(s::assemble(&s::disassemble(program).unwrap()).unwrap(), decoded, "{name}: assembly round trip");
    let layouts = layouts(&golden(name));
    let module = golden(name);
    let objects = with_composites(&module, || objects(&module, &layouts)).unwrap();
    s::verify_load(&decoded, &s::p1::P1Contracts::new(&objects).unwrap()).unwrap();
}

/// Every async Low golden of V4 with plain forms: a sync owner's cache settle, async bodies with await and
/// await.unwind, fixed roots that start without an input packet (#667), and final closes that deliver a deferred
/// cancellation after their plan, then run the aborted reserve's plan (D18).
#[test]
fn v7_low_goldens_compile_and_load() {
    for name in ["async-await-failure", "async-caught-throw", "async-never-awaited", "async-plain-await",
        "async-repeated-await", "async-void", "coro-nested-cold-await", "coro-schedule-duty-live-across-await",
        "coro-start-continuation", "low-suspending-close"]
    {
        loads(name);
    }
}

/// C-2 with descriptor parameters: the S2 transport has no descriptor bindings yet (open calls, lane cr).
#[test]
#[ignore = "#689"]
fn v7_descriptor_start_golden_compiles_and_loads() {
    loads("coro-start-descriptors");
}
