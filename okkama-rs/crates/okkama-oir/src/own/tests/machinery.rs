use super::*;
use crate::own::analyze::analyze_function;
use crate::own::domain::*;
use crate::own::emit::*;
use crate::own::bindings::{block, parameter};

fn analyzed(module: Module) -> OwnOutput {
    verify_high(&module).unwrap();
    elaborate_ownership(module, &TestNatives::default(), &OwnOptions::default()).unwrap()
}
fn checked(output: OwnOutput) -> OwnOutput {
    let output = rewrite_subset(output, &OwnOptions::default()).unwrap();
    if let Err(report) = verify_own(&output.module) {
        let mut dump = String::new();
        for (function, declaration) in output.module.functions.iter() {
            if let Some(body) = &declaration.body {
                for (block, data) in body.blocks.iter() {
                    dump.push_str(&format!("{function:?} {block:?} {:?} {:?}\n", data.params, data.term.kind));
                    for inst in &data.insts {
                        dump.push_str(&format!("  {inst:?} {:?}\n", body.insts.get(*inst).unwrap()));
                    }
                }
            }
        }
        panic!("{report:?}\n{dump}");
    }
    output
}

#[test]
fn join_tables_and_independent_obligations() {
    use Presence::*;
    let presence = [Bottom, Absent, Present, Maybe];
    let expected = [
        [Bottom, Absent, Present, Maybe],
        [Absent, Absent, Maybe, Maybe],
        [Present, Maybe, Present, Maybe],
        [Maybe, Maybe, Maybe, Maybe],
    ];
    for (row, left) in presence.iter().enumerate() {
        for (column, right) in presence.iter().enumerate() {
            assert_eq!(left.join(*right), expected[row][column]);
        }
    }
    use Multiplicity::{One, Many};
    assert_eq!(Quantity::new(Absent, Many), Quantity::ABSENT);
    let optional_history = Quantity::ONE.append(Quantity::ONE).join(Quantity::ABSENT);
    assert_eq!(optional_history, Quantity::new(Maybe, Many));
    assert_eq!(Multiplicity::Bottom.join(One), One);
    assert_eq!(One.join(Many), Many);
    let mut left = FlowState { values: vec![ValueState::default()], ..FlowState::default() };
    left.values[0].masks.builder = 1;
    left.values[0].masks.must_use = 2;
    let mut right = left.clone();
    right.values[0].masks.builder = 4;
    right.values[0].masks.must_use = 4;
    left.join(&right);
    assert_eq!(left.values[0].masks.builder, 5);
    assert_eq!(left.values[0].masks.must_use, 6);
}

fn loop_input(managed: bool) -> (Module, FuncId, ScopeId, BlockId, ValueId) {
    let mut module = ModuleBuilder::new("cyclic", Profile::High);
    let bool_ty = module.intern_type(Type::Bool).unwrap();
    let int_ty = module.intern_type(Type::Int).unwrap();
    let resource = if managed {
        let (ty, destroy) = resource(&mut module, "Res");
        empty_body(&mut module, destroy);
        Some(ty)
    } else {
        None
    };
    let failure_ty = module.intern_type(Type::Service(ServiceType::Failure)).unwrap();
    let results = resource.into_iter().collect();
    let params = if resource.is_some() { vec![bool_ty, failure_ty] } else { vec![bool_ty] };
    let mut data = function("loop", params, results);
    if let Some(ty) = resource {
        let Type::Object { decl, .. } = module.module().types.get(ty).unwrap() else { panic!() };
        data.attrs.push(FunctionAttr::Ctor(*decl));
    }
    let function = module.declare_function(data).unwrap();
    let root = scope(&mut module, function);
    let iteration = module.declare_scope(ScopeDecl {
        function, parent: Some(root), kind: ScopeKind::Iteration, loc: None,
    }).unwrap();
    let mut builder = module.function_builder(function, root).unwrap();
    let failure = with_unwind(&mut builder, root);
    let again = builder.block_params(builder.entry()).unwrap()[0];
    let active = resource.map(|_| builder.block_params(builder.entry()).unwrap()[1]);
    let ty = resource.unwrap_or(int_ty);
    let initial = if let Some(resource) = resource {
        instantiate(&mut builder, resource)
    } else {
        let start = builder.emit_schema(Operation::Const(Literal::Int("0".into()))).unwrap()[0];
        let end = builder.emit_schema(Operation::Const(Literal::Int("3".into()))).unwrap()[0];
        builder.emit_schema(Operation::Core(CoreOp::RangeNew { start, end, inclusive: false })).unwrap();
        start
    };
    let binding = builder.declare_binding(BindingDecl { name: "r".into(), scope: root, ty, loc: None }).unwrap();
    builder.emit_schema(Operation::Lexical(LexicalOp::Bind { binding, value: initial })).unwrap();
    let head = builder.create_block(root, &[ParamSpec::new(ty)]).unwrap();
    let body = builder.create_block(iteration, &[ParamSpec::new(ty)]).unwrap();
    let step = builder.create_block(root, &[ParamSpec::new(ty)]).unwrap();
    let exit = builder.create_block(root, &[]).unwrap();
    builder.terminate(TermKind::Br(Target { block: head, args: vec![initial] })).unwrap();
    builder.switch_to(head).unwrap();
    let current = builder.block_params(head).unwrap()[0];
    builder.terminate(TermKind::Cond {
        condition: again, yes: Target { block: body, args: vec![current] },
        no: Target { block: exit, args: vec![] },
    }).unwrap();
    builder.switch_to(body).unwrap();
    builder.set_unwind(Some(BlockUnwind { target: Target { block: failure, args: vec![] } })).unwrap();
    let next = if let Some(resource) = resource {
        instantiate(&mut builder, resource)
    } else {
        builder.emit_schema(Operation::Const(Literal::Int("1".into()))).unwrap()[0]
    };
    builder.emit_schema(Operation::Lexical(LexicalOp::Rebind { binding, value: next, target: root })).unwrap();
    builder.terminate(TermKind::Br(Target { block: step, args: vec![next] })).unwrap();
    builder.switch_to(step).unwrap();
    let latest = builder.block_params(step).unwrap()[0];
    builder.terminate(TermKind::Br(Target { block: head, args: vec![latest] })).unwrap();
    builder.switch_to(exit).unwrap();
    if let Some(failure) = active {
        builder.terminate(TermKind::Raise(failure)).unwrap();
    } else {
        builder.terminate(TermKind::Ret { values: vec![] }).unwrap();
    }
    let body = builder.finish().unwrap();
    module.define_function(function, body).unwrap();
    (module.finish().unwrap(), function, root, head, next)
}

#[test]
fn cyclic_analysis_has_current_history_and_maybe_many_without_unrolling() {
    let (module, function, root, head, next) = loop_input(true);
    let output = analyzed(module);
    let options = OwnOptions::default();
    let flow = analyze_function(&output.module, function, &output.analysis, &options).unwrap();
    let repeated = analyze_function(&output.module, function, &output.analysis, &options).unwrap();
    assert_eq!(flow, repeated);
    let origin = flow.value_shapes[&next];
    let header = &flow.entries[&head];
    assert_eq!(header.journals[&(root, Atom::current(origin).history())],
        Quantity::new(Presence::Maybe, Multiplicity::Many));
    assert!(header.values[next.as_u32() as usize].deps.contains(&Atom::current(origin)));
    assert!(flow.evaluations < flow.height_bound);
    let mut equations = flow.equations.clone();
    assert!(equations.intern(Shape::Protocol { holder: next, slot: 0 }).is_err());
    let history_edge = flow.outcomes.iter().find(|(edge, state)| {
        !edge.unwind && state.binding_deps.values().any(|deps| deps.contains(&Atom::current(origin)))
            && edge.plan.close.iter().any(|scope| *scope != root)
    }).unwrap();
    let original = output.module.functions.get(function).unwrap().body.as_ref().unwrap();
    let initial = original.bindings.iter().next().unwrap().0;
    assert!(history_edge.1.binding_deps[&initial].contains(&Atom::current(origin)));
    assert!(history_edge.1.values.iter().any(|value| {
        value.deps.contains(&Atom::current(origin).history())
            && !value.deps.contains(&Atom::current(origin))
    }));
    checked(output);
}

#[test]
fn scalar_for_elides_all_scope_operations_and_preserves_cycle() {
    let (module, function, _, _, _) = loop_input(false);
    let output = checked(analyzed(module));
    let body = output.module.functions.get(function).unwrap().body.as_ref().unwrap();
    assert!(!body.insts.iter().any(|(_, instruction)| matches!(instruction.op, Operation::Ownership(_))));
    assert!(body.blocks.len() > 4);
    assert!(body.blocks.iter().any(|(_, block)| matches!(&block.term.kind,
        TermKind::InvokeOwn { op, .. } if matches!(**op, Operation::Core(CoreOp::RangeNew { .. })))));
    assert_eq!(output.stats.elided_lexical_scopes, 2);
}

#[test]
fn deterministic_analysis_and_emitter_budgets() {
    let (module, function, _, _, _) = loop_input(true);
    let output = analyzed(module);
    for options in [OwnOptions { work_limit: 1, ..OwnOptions::default() },
        OwnOptions { memory_limit: 1, ..OwnOptions::default() }] {
        let left = analyze_function(&output.module, function, &output.analysis, &options).unwrap_err();
        let right = analyze_function(&output.module, function, &output.analysis, &options).unwrap_err();
        assert!(matches!(left, OwnError::Incomplete { .. }));
        assert_eq!(format!("{left:?}"), format!("{right:?}"));
    }
}

fn emit_fixture(
    emitter: &mut OutcomeEmitter<'_>,
    module: &mut Module,
    body: &mut Body,
    point: Landing,
    op: OwnershipOp,
    failures: &mut Vec<Landing>,
) -> Landing {
    let emitted = emitter.emit(module, body, point.block, point.env, Operation::Ownership(op)).unwrap();
    failures.extend(emitted.unwind);
    emitted.normal
}

#[test]
fn second_split_failure_forwards_and_restores_all_live_linear_slots() {
    let (high, function, root, _, _) = loop_input(true);
    let mut output = checked(analyzed(high));
    let options = OwnOptions::default();
    let mut emitter = OutcomeEmitter::new(&output.analysis, &options);
    let module = &mut output.module;
    let declaration = module.functions.get(function).unwrap().clone();
    let _ = declaration.body.as_ref().unwrap();
    let iteration = module.scopes.iter().find(|(_, scope)| {
        scope.function == function && scope.kind == ScopeKind::Iteration
    }).unwrap().0;
    let resource = declaration.signature.results[0];
    let Type::Object { decl, .. } = module.types.get(resource).unwrap() else { panic!() };
    let resource_decl = *decl;
    let mut body = Body {
        high_results: vec![], high_effects: vec![], descriptor_bindings: vec![], service_schedules: vec![],
        frame_layout: None,
        entry: BlockId::from_u32(0), values: Arena::new(), blocks: Arena::new(), insts: Arena::new(),
        bindings: Arena::new(), boundaries: Arena::new(), deps: Arena::new(), keys: Arena::new(),
        selections: Arena::new(), cleanup: Default::default(),
    };
    let entry = block(&mut body, root).unwrap();
    let mut env = LiveEnv::default();
    for ty in &declaration.signature.params {
        let value = parameter(&mut body, entry, *ty, ValueFacts::Service).unwrap();
        match module.types.get(*ty).unwrap() {
            Type::Bool => {
                body.values.get_mut(value).unwrap().facts = ValueFacts::Plain { certificate: PlainCertificate::Scalar };
            }
            Type::Service(ServiceType::Own) => { env.linear.insert(value); }
            _ => {}
        }
    }
    let offered = *env.linear.first().unwrap();
    let mut point = Landing { block: entry, env, results: vec![], forwarded: BTreeMap::new() };
    let mut failures = vec![];
    point = emit_fixture(&mut emitter, module, &mut body, point,
        OwnershipOp::ScopeBegin { scope: root }, &mut failures);
    let state = point.env.scopes[&root];
    point = emit_fixture(&mut emitter, module, &mut body, point,
        OwnershipOp::Accept { state, packet: offered }, &mut failures);
    let mut resources = vec![];
    for owner in [root, iteration] {
        if owner == iteration {
            let nested = block(&mut body, iteration).unwrap();
            body.blocks.get_mut(point.block).unwrap().term.kind = TermKind::Br(Target {
                block: nested, args: vec![],
            });
            point.block = nested;
            point = emit_fixture(&mut emitter, module, &mut body, point,
                OwnershipOp::ScopeBegin { scope: iteration }, &mut failures);
        }
        point = emit_fixture(&mut emitter, module, &mut body, point, OwnershipOp::Empty {}, &mut failures);
        let packet = point.result(SlotName::Packet).unwrap();
        let emitted = emitter.emit(module, &mut body, point.block, point.env, Operation::Core(CoreOp::ObjNew {
            declaration: resource_decl, type_args: vec![], fields: vec![], packet: Some(packet),
            construction: ConstructionStatus::Complete,
        })).unwrap();
        failures.extend(emitted.unwind);
        point = emitted.normal;
        let value = point.result(SlotName::Value).unwrap();
        let packet = point.result(SlotName::Packet).unwrap();
        point = emit_fixture(&mut emitter, module, &mut body, point,
            OwnershipOp::Dependencies { value }, &mut failures);
        let deps = point.result(SlotName::Value).unwrap();
        point.env.values.insert(value, crate::own::bindings::LatestValue { value, deps: Some(deps) });
        let state = point.env.scopes[&owner];
        let admit = if owner == root {
            OwnershipOp::Accept { state, packet }
        } else {
            OwnershipOp::Admit { state, packet }
        };
        point = emit_fixture(&mut emitter, module, &mut body, point, admit, &mut failures);
        resources.push(value);
    }
    let first = point.env.value(resources[0]).unwrap().value;
    let first_selection = body.selections.alloc(SelectionExpr::Value(first)).unwrap();
    point = emit_fixture(&mut emitter, module, &mut body, point, OwnershipOp::Empty {}, &mut failures);
    let empty_packet = point.result(SlotName::Packet).unwrap();
    let second = point.env.value(resources[1]).unwrap().value;
    let second_selection = body.selections.alloc(SelectionExpr::Value(second)).unwrap();
    let second_selection = body.selections.alloc(SelectionExpr::Admissible(second_selection)).unwrap();
    let first_split = point.block;
    let batch = emitter.extract_scopes(module, &mut body, point, &[
        ScopeExtraction { owner: root, selection: first_selection, component: AdmissionComponent::Bound },
        ScopeExtraction {
            owner: iteration, selection: second_selection, component: AdmissionComponent::Admission,
        },
    ]).unwrap();
    let TermKind::InvokeOwn { normal: first_edge, .. } = &body.blocks.get(first_split).unwrap().term.kind else {
        panic!()
    };
    let split_block = first_edge.block;
    let earlier_packet = body.blocks.get(split_block).unwrap().params[1];
    let empty_position = first_edge.args.iter().position(|arg| *arg == EdgeArg::Value(empty_packet)).unwrap();
    let empty_after_first = body.blocks.get(split_block).unwrap().params[empty_position];
    let second_failed = &batch.failures[1].split;
    let TermKind::InvokeOwn { op, .. } = &body.blocks.get(split_block).unwrap().term.kind else { panic!() };
    let Operation::Ownership(OwnershipOp::ScopeSplit { state, .. }) = **op else { panic!() };
    assert!(second_failed.forwarded.contains_key(&earlier_packet));
    assert!(second_failed.forwarded.contains_key(&empty_after_first));
    assert!(second_failed.env.scopes.contains_key(&root));
    assert_ne!(second_failed.env.scopes[&iteration], state);
    let stale_state = state;
    assert_eq!(second_failed.env.all_linear().len(), 4);
    assert_eq!(batch.normal.env.all_linear().len(), 5);
    assert!(!batch.failures[1].restored.env.linear
        .contains(&second_failed.forwarded[&earlier_packet]));
    failures.extend(batch.failures.into_iter().map(|failure| failure.restored));
    let mut normal = batch.normal;
    let selected = batch.packets[1].packet;
    let state = normal.env.scopes[&iteration];
    normal = emit_fixture(&mut emitter, module, &mut body, normal,
        OwnershipOp::Admit { state, packet: selected }, &mut failures);
    // Complete both outcomes using the emitter. The bound first extraction
    // returns with accept to its original owner, never fresh admission.
    failures.push(normal);
    for mut landing in failures {
        let packets: Vec<_> = landing.env.linear.iter().copied().collect();
        for packet in packets {
            let state = landing.env.scopes[&root];
            landing = emitter.emit(module, &mut body, landing.block, landing.env,
                Operation::Ownership(OwnershipOp::Accept { state, packet })).unwrap().normal;
        }
        for owner in [iteration, root] {
            let Some(state) = landing.env.scopes.get(&owner).copied() else { continue };
            let op = if let Some(failure) = landing.env.cleanup.failure {
                OwnershipOp::ScopeUnwind {
                    state, failure, reserve: None, effects: ScopeEffectEnvelope::default(),
                }
            } else {
                OwnershipOp::ScopeEnd { state, reserve: None, effects: ScopeEffectEnvelope::default() }
            };
            landing = emitter.emit(module, &mut body, landing.block, landing.env,
                Operation::Ownership(op)).unwrap().normal;
        }
        body.blocks.get_mut(landing.block).unwrap().term.kind = if let Some(failure) = landing.env.cleanup.failure {
            TermKind::Raise(failure)
        } else {
            // No normal publication is part of this extraction fixture.
            TermKind::Trap { code: TrapCode::Unreachable, message: "fixture complete".into() }
        };
    }
    module.functions.get_mut(function).unwrap().body = Some(body.clone());
    verify_own(module).unwrap_or_else(|report| panic!("{report:?}"));
    let mut stale = module.clone();
    let bad = stale.functions.get_mut(function).unwrap().body.as_mut().unwrap();
    let TermKind::InvokeOwn { unwind, .. } = &mut bad.blocks.get_mut(split_block).unwrap().term.kind else {
        panic!()
    };
    unwind.args[0] = EdgeArg::Value(stale_state);
    assert!(verify_own(&stale).is_err());
    let mut missing = module.clone();
    let bad = missing.functions.get_mut(function).unwrap().body.as_mut().unwrap();
    let TermKind::InvokeOwn { unwind: bad_edge, .. } = &mut bad.blocks.get_mut(split_block).unwrap().term.kind else {
        panic!()
    };
    bad_edge.args.remove(0);
    assert!(verify_own(&missing).is_err());
    assert!(emitter.stats().emitted_checked_splits >= 2);
}

#[test]
fn h1_rejects_internal_edge_to_iteration_entry_before_analysis() {
    let (mut module, function, _, _, _) = loop_input(false);
    let body = module.functions.get_mut(function).unwrap().body.as_mut().unwrap();
    let iteration = body.blocks.iter().find(|(_, block)| {
        module.scopes.get(block.lexical_scope).unwrap().kind == ScopeKind::Iteration
    }).unwrap().0;
    let argument = body.blocks.get(iteration).unwrap().params[0];
    let TermKind::Br(step) = body.blocks.get(iteration).unwrap().term.kind.clone() else { panic!() };
    let condition = body.blocks.get(body.entry).unwrap().params[0];
    body.blocks.get_mut(iteration).unwrap().term.kind = TermKind::Cond {
        condition, yes: Target { block: iteration, args: vec![argument] }, no: step,
    };
    assert!(verify_high(&module).unwrap_err().diagnostics.iter().any(|diagnostic| diagnostic.rule == RuleId::H1));
    let options = OwnOptions::default();
    let error = analyze_function(&module, function, &OwnAnalysis::default(), &options).unwrap_err();
    assert!(error.to_string().contains("H1"));
}

#[test]
fn scalar_preparation_is_erased_and_budget_boundaries_are_exact() {
    let (module, _, _, _, _) = loop_input(true);
    let output = analyzed(module);
    let high = output.module.clone();
    let completed = checked(output);
    let limits = OwnOptions {
        work_limit: completed.stats.charged_work,
        memory_limit: completed.stats.peak_auxiliary_bytes,
        ..OwnOptions::default()
    };
    let input = elaborate_ownership(high.clone(), &TestNatives::default(), &OwnOptions::default()).unwrap();
    let repeated = rewrite_subset(input, &limits).unwrap();
    assert_eq!(completed.stats, repeated.stats);
    for options in [OwnOptions { work_limit: limits.work_limit - 1, ..limits.clone() },
        OwnOptions { memory_limit: limits.memory_limit - 1, ..limits.clone() }] {
        let input = elaborate_ownership(high.clone(), &TestNatives::default(), &OwnOptions::default()).unwrap();
        assert!(matches!(rewrite_subset(input, &options), Err(OwnError::Incomplete { .. })));
    }
    let mut module = ModuleBuilder::new("unsupported", Profile::High);
    let int = module.intern_type(Type::Int).unwrap();
    let function = module.declare_function(function("f", vec![], vec![int])).unwrap();
    let root = scope(&mut module, function);
    let mut builder = module.function_builder(function, root).unwrap();
    let value = builder.emit_schema(Operation::Const(Literal::Int("1".into()))).unwrap()[0];
    prepare_return(&mut builder, root, value);
    builder.terminate(TermKind::Ret { values: vec![value] }).unwrap();
    let body = builder.finish().unwrap();
    module.define_function(function, body).unwrap();
    checked(analyzed(module.finish().unwrap()));
}

#[test]
fn sealed_latest_binding_waits_for_backedge_and_requires_paired_full_deps() {
    use crate::own::bindings::{LatestValue, SealedSsa, SsaSlot};
    let (module, function, root, _, _) = loop_input(true);
    let mut output = analyzed(module);
    let module = &mut output.module;
    let mut body = module.functions.get(function).unwrap().body.clone().unwrap();
    let source = body.entry;
    let binding = body.bindings.iter().next().unwrap().0;
    let ty = body.bindings.get(binding).unwrap().ty;
    let head = block(&mut body, root).unwrap();
    let step = block(&mut body, root).unwrap();
    let mut ssa = SealedSsa::default();
    let phi = ssa.declare(module, &mut body, head, SsaSlot::Binding(binding), ty, true).unwrap();
    let back = ssa.declare(module, &mut body, step, SsaSlot::Binding(binding), ty, true).unwrap();
    let missing = BTreeMap::from([(SsaSlot::Binding(binding), LatestValue { value: back.value, deps: None })]);
    assert!(ssa.incoming(&body, head, step, 0, &missing).is_err());
    let first = BTreeMap::from([(SsaSlot::Binding(binding), back)]);
    assert_eq!(ssa.incoming(&body, head, source, 0, &first).unwrap(), vec![back.value, back.deps.unwrap()]);
    assert!(ssa.seal(head, 2).is_err());
    let backedge = BTreeMap::from([(SsaSlot::Binding(binding), phi)]);
    assert_eq!(ssa.incoming(&body, head, step, 1, &backedge).unwrap(), vec![phi.value, phi.deps.unwrap()]);
    ssa.seal(head, 2).unwrap();
    assert_eq!(ssa.read(head, SsaSlot::Binding(binding)).unwrap(), phi);
    let ValueFacts::Managed { deps } = body.values.get(phi.value).unwrap().facts else { panic!() };
    assert_eq!(body.deps.get(deps).unwrap(), &DepExpr::FromSsa(phi.deps.unwrap()));
}

fn scalar_size_input(values: usize, blocks: usize) -> (Module, FuncId) {
    let mut module = ModuleBuilder::new("memory", Profile::High);
    let function = module.declare_function(function("main", vec![], vec![])).unwrap();
    let root = scope(&mut module, function);
    let mut builder = module.function_builder(function, root).unwrap();
    for _ in 0..values {
        builder.emit_schema(Operation::Const(Literal::Int("1".into()))).unwrap();
    }
    for _ in 1..blocks {
        let next = builder.create_block(root, &[]).unwrap();
        builder.terminate(TermKind::Br(Target { block: next, args: vec![] })).unwrap();
        builder.switch_to(next).unwrap();
    }
    builder.terminate(TermKind::Ret { values: vec![] }).unwrap();
    let body = builder.finish().unwrap();
    module.define_function(function, body).unwrap();
    (module.finish().unwrap(), function)
}

#[test]
fn sparse_p1_size_memory_and_function_local_scope_dimensions() {
    // Largest measured P1 main: 105 SSA values, 35 blocks (branches-wanted-match).
    let (mut module, function) = scalar_size_input(105, 35);
    let options = OwnOptions::default();
    let output = analyzed(module.clone());
    let flow = analyze_function(&output.module, function, &output.analysis, &options).unwrap();
    assert_eq!(flow.equations.nodes, vec![Shape::Empty]);
    assert!(flow.stats.peak_auxiliary_bytes < options.memory_limit / 64);
    let unrelated = module.functions.alloc(super::function("unrelated", vec![], vec![])).unwrap();
    for _ in 0..1000 {
        module.scopes.alloc(ScopeDecl {
            function: unrelated, parent: None, kind: ScopeKind::Action, loc: None,
        }).unwrap();
    }
    let with_scopes = analyze_function(&module, function, &output.analysis, &options).unwrap();
    assert_eq!(flow, with_scopes);
    let exact = OwnOptions { memory_limit: flow.stats.peak_auxiliary_bytes, ..options.clone() };
    assert_eq!(analyze_function(&output.module, function, &output.analysis, &exact).unwrap(), flow);
    let below = OwnOptions { memory_limit: exact.memory_limit - 1, ..options };
    let error = analyze_function(&output.module, function, &output.analysis, &below).unwrap_err();
    assert!(matches!(error, OwnError::Incomplete { stats } if stats.exhausted_limit == Some(OwnLimit::Memory)));
}

#[test]
fn genuinely_large_sparse_function_exhausts_default_memory_cheaply() {
    // Linear CFG, but thousands of retained SSA states: real vector/snapshot storage
    // exceeds 256 MiB. No enormous dense equation matrix or unbounded work is needed.
    let mut module = ModuleBuilder::new("large_memory", Profile::High);
    let int = module.intern_type(Type::Int).unwrap();
    let f = module.declare_function(function("large", vec![int; 4096], vec![])).unwrap();
    let root = scope(&mut module, f);
    let mut builder = module.function_builder(f, root).unwrap();
    for _ in 0..768 {
        let next = builder.create_block(root, &[]).unwrap();
        builder.terminate(TermKind::Br(Target { block: next, args: vec![] })).unwrap();
        builder.switch_to(next).unwrap();
    }
    builder.terminate(TermKind::Ret { values: vec![] }).unwrap();
    let body = builder.finish().unwrap();
    module.define_function(f, body).unwrap();
    let module = module.finish().unwrap();
    let analysis = OwnAnalysis { types: classify_test(&module, &OwnOptions::default()), ..OwnAnalysis::default() };
    let error = match analyze_function(&module, f, &analysis, &OwnOptions::default()) {
        Err(error) => error,
        Ok(flow) => panic!("large analysis unexpectedly completed: {:?}", flow.stats),
    };
    assert!(matches!(&error, OwnError::Incomplete { stats }
        if stats.exhausted_limit == Some(OwnLimit::Memory) && stats.charged_work < 50_000_000), "{error:?}");
}

#[test]
fn memory_scratch_is_released_and_peak_is_not_cumulative_allocation() {
    let options = OwnOptions { memory_limit: 100, ..OwnOptions::default() };
    let mut budget = Budget::new(&options);
    budget.memory(40).unwrap();
    for _ in 0..100 {
        budget.memory(60).unwrap();
        budget.release(60);
    }
    assert_eq!(budget.stats.peak_auxiliary_bytes, 100);
    assert!(matches!(budget.memory(61), Err(OwnError::Incomplete { stats })
        if stats.exhausted_limit == Some(OwnLimit::Memory)));
}
