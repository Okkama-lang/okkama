//! #601 V7: plain async from Low OIR-2 to SPB 17. The inputs are the async Low goldens of okkama-oir (V4); the
//! expected forms are the async ABI (C-1…C-3, D20, D21) and Champ draft 3 §14, as the V6 positives build them.
use super::*;
use s::{Argument as A, Location as L, Opcode as O, ProducedType as P, ServiceKind as K};

/// Layouts of the goldens' declarations: plain objects, resources (a destroy, its identity after the fields, as
/// the S3 goldens lay it out) and Error enums.
fn layouts(module: &o::Module) -> BTreeMap<u32, ObjectLayout> {
    let mut layouts = BTreeMap::new();
    for (id, declaration) in module.declarations.iter() {
        let (storage_fields, fields, resource) = match &declaration.kind {
            o::DeclarationKind::Object { fields, destroy, .. } => {
                let storage = fields.iter().map(|field| translate_type(module, field.ty).unwrap()).collect::<Vec<_>>();
                (storage, fields.len() as u32, destroy.is_some())
            }
            o::DeclarationKind::Enum { .. } => (vec![s::Type::Int], 0, false),
            // A role (the builtin Error of a catch) is a placeholder descriptor, as in the pipeline's layouts.
            o::DeclarationKind::Role { .. } => (vec![], 0, false),
            kind => panic!("{}: no test layout for {kind:?}", declaration.name),
        };
        let trace_layout = storage_fields.iter().enumerate()
            .filter_map(|(index, ty): (usize, &s::Type)| ty.traced().then_some(index as u32)).collect::<Vec<_>>();
        let words = 1 + storage_fields.len() as u32;
        layouts.insert(id.as_u32(), ObjectLayout {
            runtime: s::ObjectRuntimeLayout {
                allocation_words: (words + 4 * u32::from(resource)).max(2),
                identity_offset: resource.then_some(8 * words),
                has_destroy: resource,
                may_reach_resource: resource,
                may_async_destroy: false,
                storage_fields,
                selection_layout: if resource { trace_layout.clone() } else { vec![] },
                trace_layout,
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
/// Services protocols are carried by opcodes and need no SPB 17 protocol; a protocol of another family must be
/// one of the dynamic transfer protocols SPB 17 has (#676), so a Closure protocol of another shape is rejected.
fn v7_services_protocols_pass_and_other_shapes_are_rejected() {
    let module = golden("async-plain-await");
    translated(&module);
    let mut other = module.clone();
    for (_, protocol) in other.protocols.iter_mut() {
        protocol.kind = o::ProtocolKind::Closure;
    }
    rejected(&other, "Closure protocol");
}

#[test]
fn v7_a_module_that_requires_async_is_translated() {
    let mut module = golden("async-plain-await");
    module.requires = vec![o::Feature::Core, o::Feature::Async];
    translated(&module);
    module.requires.push(o::Feature::Process);
    rejected(&module, "unresolved declarations or service metadata");
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

/// The async Low goldens of #601 K1, #684 and #691 load: cancellation requests and queries, the elaboration §13.4
/// fixture, nested closes before the next await and while an outer Future is live, and the deps of a Future
/// argument between the language inputs and the packet of continuation 0 (SPB_ASYNC_ENTRY). The void fixed root
/// of #691 is an async test, which TEST_ENTRY does not take yet (#725).
#[test]
fn k1_low_goldens_compile_and_load() {
    for name in ["k1-cancel-unstarted", "k1-failure-view-escapes", "k1-nested-close-next-await",
        "nested-close-outer-future", "start-deps", "void-fixed-root"]
    {
        loads(name);
    }
}

/// A catch of the Error role binds the system Cancelled for a cancellation (#738), so a module that catches the role
/// without declaring Cancelled gets its descriptor last, with Cancelled's exact shape; every other descriptor keeps
/// its index. A module without such a catch gets none.
#[test]
fn a_catch_of_the_error_role_brings_the_system_cancelled() {
    let module = golden("k1-cancel-unstarted");
    let program = compile(&module).unwrap().program;
    let without = translate(&{
        let mut plain = module.clone();
        for (_, function) in plain.functions.iter_mut() {
            let Some(body) = function.body.as_mut() else { continue };
            for (_, block) in body.blocks.iter_mut() {
                if let o::TermKind::Catch { next, .. } = &block.term.kind {
                    block.term.kind = o::TermKind::Br(next.clone());
                }
            }
        }
        plain
    }).unwrap();
    assert_eq!(s::cancelled(&without), None);
    assert_eq!(program.objects.len(), without.objects.len() + 1);
    assert_eq!(program.objects[..without.objects.len()], without.objects[..]);
    assert_eq!(s::cancelled(&program), Some(without.objects.len() as u32));
    assert!(program.objects.iter().filter(|object| object.name == s::CANCELLED).count() == 1);
    assert_eq!(s::cancelled(&compile(&golden("async-plain-await")).unwrap().program), None);
}

/// #725: an async test is a void async body and a fixed root. TEST_ENTRY refuses one that publishes a value, and
/// SPB_ASYNC_ENTRY one whose start holds an input.
#[test]
fn an_async_test_is_a_void_fixed_root() {
    let module = golden("void-fixed-root");
    let program = compile(&module).unwrap().program;
    let objects = with_composites(&module, || objects(&module, &layouts(&module))).unwrap();
    let contracts = s::p1::P1Contracts::new(&objects).unwrap();
    s::verify_load(&program, &contracts).unwrap();
    let test = program.tests.as_ref().unwrap()[0].function as usize;
    assert!(matches!(program.functions[test].entry_abi, s::EntryAbi::AsyncBody { .. }));
    let mut valued = program.clone();
    valued.functions[test].results.push(s::Type::Int);
    assert_eq!(s::verify_load(&valued, &contracts).unwrap_err().rule, "TEST_ENTRY");
    let mut started = program.clone();
    let layout = match started.functions[test].entry_abi {
        s::EntryAbi::AsyncBody { layout } => layout as usize,
        _ => unreachable!(),
    };
    let frame = &mut started.tables.frames[layout];
    frame.slots.push(s::FrameSlot { ty: P::Value(s::Type::Int), linear: false });
    let slot = (frame.slots.len() - 1) as u32;
    frame.continuations[0].saved.push(slot);
    assert_eq!(s::verify_load(&started, &contracts).unwrap_err().rule, "SPB_ASYNC_ENTRY");
}


// --- #601 R4: resources ------------------------------------------------------------------------------------------

/// The R2 Low goldens with resources (okkama-oir, `verify_low` in full): a resource held across an await, a close
/// that suspends with resources, the temporary take whose proof coro rechecked, and `defer await` with a body
/// failure and with a reserve (C-6).
#[test]
fn r4_low_goldens_with_resources_compile_and_load() {
    for name in ["r2-resource-across-await", "r2-resource-suspending-close", "r2-take-proof-recheck",
        "r2-defer-await-body-failure", "r2-defer-await-reserve"]
    {
        loads(name);
    }
}

/// OIR-2 §10.3 after coro: the proof names the Future, the take consumes the capability with the awaiting scope
/// as its temporary boundary, and the capability waits in the frame only as the take's operand (draft 3 §4.2).
#[test]
fn r4_take_consumes_the_proof_of_its_future_at_a_temporary_boundary() {
    let p = translated(&golden("r2-take-proof-recheck"));
    let f = function(&p, "f");
    let [(proof, proof_arguments, proof_flow)] = sites(f, O::UniqueCacheProof)[..] else { panic!("one proof") };
    let [(take, take_arguments, take_flow)] = sites(f, O::AwaitTake)[..] else { panic!("one take") };
    let capability = outputs(proof_flow)[0];
    assert_eq!(ty(f, capability), P::Service(K::UniqueCache));
    assert_eq!(take_arguments[0], proof_arguments[0], "the take awaits the proved Future");
    assert_eq!(take_arguments[1], A::Location(capability));
    assert_eq!(take_arguments[2], A::Scope(1), "boundary temporary(awaiting=S1)");
    // The golden's only Future type is future<void>.
    assert_eq!(types(f, take_arguments), [P::Value(s::Type::Future(0)), P::Service(K::UniqueCache)]);
    // The bracket: proof, save, take; the outcomes are the void value's packet and deps, and the unwind triple.
    assert_eq!(take, proof + 2);
    assert!(matches!(f.code[proof + 1], s::Instruction::Champ { op: O::FrameSave, .. }));
    let s::Flow::Outcomes(outcomes) = take_flow else { panic!("the take is a terminator") };
    let tags = outcomes.iter().map(|outcome| outcome.tag).collect::<Vec<_>>();
    assert_eq!(tags, [s::Tag::Normal, s::Tag::Unwind]);
    let layout = &p.tables.frames[1];
    let site = layout.continuations.iter().find(|continuation| continuation.site == Some(take as u32)).unwrap();
    assert_eq!(site.operands, [1, 6], "the Future and its capability are the take's operands");
    assert_eq!(layout.slots[6], service(K::UniqueCache));
    assert!(layout.continuations.iter().all(|continuation| !continuation.saved.contains(&6)));
}

/// Coro keeps the facts of a resumed value: the Future reloaded after the first await still names, in its deps
/// key, the register its continuation saved. The dependency of SPB 17 names the reloaded value instead, so the
/// saved register is dead after its save, as B9.5 has it.
#[test]
fn r4_facts_of_a_resumed_value_name_the_reloaded_value() {
    let p = translated(&golden("r2-take-proof-recheck"));
    let f = function(&p, "f");
    let [(_, _, created)] = sites(f, O::FutureNew)[..1] else { panic!("fut.new") };
    let saved = outputs(created)[0];
    let reloaded = sites(f, O::FrameResume).into_iter()
        .filter(|(_, arguments, _)| arguments[1] == A::Continuation(1))
        .map(|(_, _, flow)| outputs(flow)[1]).collect::<Vec<_>>();
    assert_eq!(reloaded.len(), 2, "the normal and the unwind resume of the first await");
    for value in reloaded {
        assert!(!f.proofs.dependencies.iter().any(|dependency| dependency.value == value
            && dependency.obligations.contains(&saved)), "{value:?} keeps the saved {saved:?} alive");
    }
}

#[test]
fn r4_bound_take_waits_for_c5() {
    let mut module = golden("r2-take-proof-recheck");
    edit(&mut module, "f", |function| {
        let body = function.body.as_mut().unwrap();
        for (_, block) in body.blocks.iter_mut() {
            if let o::TermKind::InvokeOwn { op, .. } = &mut block.term.kind
                && let o::Operation::Async(o::AsyncOp::AwaitTake { boundary, .. }) = op.as_mut()
            {
                *boundary = o::TakeBoundary::Existing { owner: o::ScopeId::from_u32(1) };
            }
        }
    });
    rejected(&module, "bound await.take (C5)");
}


/// C-4, token form: the cache duty joins the pending reserve before the close, which no longer settles it. The
/// fixture is the take golden with the second Future's duty rehomed on the successful return. OIR has only scope
/// targets until #601 R1 adds `target caller`, so the translated target is S1 while the return commits to the
/// caller: the load gate refuses exactly that, and accepts the same program with the caller as target.
#[test]
fn r4_rehome_puts_the_cache_duty_into_the_reserve_of_its_exit() {
    let module = o::text::parse(include_str!("fixtures/rehome-token.low.oir")).unwrap_or_else(|e| panic!("{e:?}"));
    let layouts = layouts(&module);
    let mut program = with_composites(&module, || generate_with_allocation(&module, &layouts)).unwrap().program;
    let f = function(&program, "f");
    let [(pc, arguments, flow)] = sites(f, O::CacheRehome)[..] else { panic!("one rehome") };
    assert_eq!(arguments[0], A::Destination(s::ExitDestination::Scope(1)));
    assert_eq!(types(f, arguments), [P::Service(K::Pending), P::Service(K::CacheState)]);
    let reserve = outputs(flow)[0];
    let s::Instruction::Champ { op: O::CleanupBegin, arguments: begin, .. } = &f.code[pc + 1] else {
        panic!("the rehome prepares the close")
    };
    assert_eq!(begin[1], A::Location(reserve), "the close takes the reserve that holds the duty");
    let objects = with_composites(&module, || objects(&module, &layouts)).unwrap();
    let contracts = s::p1::P1Contracts::new(&objects).unwrap();
    let failure = s::verify_load(&program, &contracts).unwrap_err();
    assert_eq!(failure.rule, "SPB_REHOME", "{failure}");
    let index = program.functions.iter().position(|function| function.symbol == "f").unwrap();
    let s::Instruction::Champ { arguments, .. } = &mut program.functions[index].code[pc] else { unreachable!() };
    arguments[0] = A::Destination(s::ExitDestination::Caller);
    s::verify_load(&program, &contracts).unwrap();
}

/// C-6: `defer await closer()` in R2's goldens. The async defer entry takes its frame and publishes, and
/// `invoke.defer` of it is a suspension site of its caller's layout.
#[test]
fn r4_async_defer_entry_is_an_async_body_that_its_caller_awaits() {
    let p = translated(&golden("r2-defer-await-body-failure"));
    let closer = function(&p, "closer");
    assert!(matches!(closer.entry_abi, s::EntryAbi::AsyncDefer { .. }));
    assert_eq!(types(closer, &closer.params.iter().copied().map(A::Location).collect::<Vec<_>>()),
        [P::Service(K::Frame)]);
    let f = function(&p, "f");
    let s::EntryAbi::AsyncBody { layout } = f.entry_abi else { panic!("f is an async body") };
    let defers = sites(f, O::InvokeDefer);
    assert_eq!(defers.len(), 2);
    for (pc, arguments, _) in defers {
        assert_eq!(arguments[0], A::Entry(index(&p, "closer")));
        assert!(p.tables.frames[layout as usize].continuations.iter().any(|k| k.site == Some(pc as u32)),
            "invoke.defer at {pc} is a suspension site");
        assert!(matches!(f.code[pc - 1], s::Instruction::Champ { op: O::FrameSave, .. }));
    }
}

#[test]
fn r4_async_defer_entry_never_replaces() {
    let mut module = golden("r2-defer-await-body-failure");
    edit(&mut module, "closer", |function| {
        for attr in &mut function.attrs {
            if let o::FunctionAttr::DeferEntry { replacements } = attr {
                replacements.push(o::DeferLabel { label: "early".into(), result: None });
            }
        }
    });
    rejected(&module, "an async defer entry never replaces (C-6)");
}

/// R1's own golden `name` through coro and low, as the pipeline runs them.
fn escape(name: &str) -> o::Module {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../okkama-oir/src/own/tests/async-goldens/{name}.oir"));
    let module = o::text::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
    let coro = okkama_oir::coro::lower_coro(module, &Default::default()).unwrap();
    okkama_oir::low::lower_cleanup(coro, &Default::default()).unwrap()
}

/// #601 D25: a Future that leaves its block by its token (r1-cold-escape) or whose outcome view escapes
/// (r1-view-escape) is admitted by the outer scope, whose closes settle it with `cleanup.settle.dynamic` before
/// the executor plan; the programs pass the load gate (SPB_SCHEDULE, SPB_REHOME).
#[test]
fn d25_admitted_entries_settle_dynamically_and_load() {
    for name in ["r1-cold-escape", "r1-view-escape"] {
        let low = escape(name);
        compile(&low).unwrap_or_else(|error| panic!("{name}: {error}"));
        let program = translated(&low);
        let main = function(&program, "main");
        let dynamic = sites(main, O::CleanupSettleDynamic);
        assert!(!dynamic.is_empty(), "{name}");
        for (pc, arguments, flow) in dynamic {
            assert_eq!(types(main, arguments), [P::Service(K::Cleanup)], "{name}");
            assert_eq!(outputs(flow).len(), 1);
            let s::Instruction::Champ { op: O::CleanupPlan, .. } = &main.code[pc + 1] else {
                panic!("{name}: the plan follows the dynamic settle");
            };
        }
        assert!(!sites(main, O::CacheRehome).is_empty(), "{name}");
    }
}

/// C-4, own form: the rehome of an entry that main's block admitted from @make (OIR-2 §10.1) names the target,
/// the reserve, the state and the Future, and gives the state back before the reserve.
#[test]
fn d25_the_state_form_of_rehome_takes_the_state_and_the_future() {
    let program = translated(&escape("r1-make-escape"));
    let main = function(&program, "main");
    let state = sites(main, O::CacheRehome).into_iter().find(|(_, arguments, _)| arguments.len() == 4)
        .expect("main passes the admitted Future on in the state form");
    let (_, arguments, flow) = state;
    assert!(matches!(arguments[0], A::Destination(s::ExitDestination::Scope(_))));
    assert_eq!(types(main, &arguments[1..]), [P::Service(K::Pending), P::Service(K::ScopeState),
        P::Value(s::Type::Future(0))]);
    let outputs = outputs(flow);
    assert_eq!([ty(main, outputs[0]), ty(main, outputs[1])], [P::Service(K::ScopeState), P::Service(K::Pending)]);
    // @make returns its Future with its token, to the caller.
    let make = function(&program, "make");
    let token = sites(make, O::CacheRehome);
    assert_eq!(token.len(), 1);
    assert_eq!(token[0].1[0], A::Destination(s::ExitDestination::Caller));
    assert_eq!(types(make, &token[0].1[1..]), [P::Service(K::Pending), P::Service(K::CacheState)]);
}

/// #601 P1 (D24.1): `fut.spawn` is opcode 65 over the Future and its cache duty, giving back the cache duty and the
/// spawned task's duty; the close settles the task duty, then the cache duty (ABI §6.3). Each P1 Low golden passes
/// the executable gate.
#[test]
fn p1_spawn_takes_the_future_and_its_cache_duty_and_its_close_settles_the_task_first() {
    for name in ["p1-spawn-awaited", "p1-spawn-detached", "p1-sync-owner-spawn", "p1-spawn-returned"] {
        compile(&golden(name)).unwrap_or_else(|error| panic!("{name}: {error}"));
    }
    let p = translated(&golden("p1-spawn-detached"));
    let main = function(&p, "main");
    let [(_, arguments, flow)] = sites(main, O::FutureSpawn)[..] else { panic!("one spawn") };
    assert_eq!(types(main, arguments), [P::Value(FUTURE_INT), P::Service(K::CacheState)]);
    let produced = outputs(flow);
    assert_eq!(produced.iter().map(|&location| ty(main, location)).collect::<Vec<_>>(),
        [P::Service(K::CacheState), P::Service(K::TaskState)]);
    let settled: Vec<_> = sites(main, O::CleanupSettle).into_iter().filter_map(|(_, arguments, _)| match arguments {
        [A::Location(_), A::Schedule(_), A::Location(duty)] => Some(*duty),
        _ => None,
    }).collect();
    assert_eq!(settled, [produced[1], produced[0]], "the task duty settles first");
}

/// #601 P1, C-4: a spawned Future returned through its owner's exit leaves with both duties: the token rehome
/// takes the reserve, the cache duty and the task duty.
#[test]
fn p1_a_returned_spawned_future_rehomes_its_task_duty_with_its_cache_duty() {
    let p = translated(&golden("p1-spawn-returned"));
    let start = p.functions.iter().find(|function| function.symbol.contains("Starter.start")).unwrap();
    let [(_, arguments, _)] = sites(start, O::CacheRehome)[..] else { panic!("one rehome") };
    assert!(matches!(arguments[0], A::Destination(s::ExitDestination::Caller)), "{arguments:?}");
    assert_eq!(types(start, arguments), [P::Service(K::Pending), P::Service(K::CacheState), P::Service(K::TaskState)]);
}
