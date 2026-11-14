use super::*;

struct OirContracts<'a>(&'a o::Module);
impl o::ExternalContracts for OirContracts<'_> {
    fn native(&self, name: &str) -> Option<&o::Signature> {
        self.0.functions.iter().find_map(|(_, f)| f.attrs.iter().any(|attr| {
            matches!(attr, o::FunctionAttr::Extern(symbol) if symbol == name && name.starts_with("fixture."))
        }).then_some(&f.signature))
    }
    fn intrinsic(&self, _: &str) -> Option<&o::Signature> { None }
    fn native_summary(&self, name: &str) -> Option<&o::Summary> {
        self.0.functions.iter().find_map(|(_, f)| f.attrs.iter().any(|attr| {
            matches!(attr, o::FunctionAttr::Extern(symbol) if symbol == name && name.starts_with("fixture."))
        }).then_some(f.summary).flatten()).and_then(|id| self.0.summaries.get(id).ok())
    }
}

struct Catalog(Vec<s::Function>);
impl s::ExternalContracts for Catalog {
    fn entry(&self, name: &str) -> Option<&s::Function> {
        self.0.iter().find(|f| f.proofs.entry.as_ref().and_then(|entry| entry.external.as_deref()) == Some(name))
    }
}

fn emit(source: &str) -> (AllocatedProgram, Catalog) {
    let module = o::text::parse(source).unwrap();
    let mut layouts = BTreeMap::new();
    for (id, decl) in module.declarations.iter() {
        let o::DeclarationKind::Object { fields, destroy, .. } = &decl.kind else { panic!("object") };
        let storage_fields = fields.iter().map(|field| translate_type(&module, field.ty).unwrap()).collect::<Vec<_>>();
        let traced = storage_fields.iter().enumerate().filter_map(|(index, ty)| {
            matches!(ty, s::Type::Object(_) | s::Type::Str).then_some(index as u32)
        }).collect::<Vec<_>>();
        layouts.insert(id.as_u32(), ObjectLayout {
            runtime: s::ObjectRuntimeLayout {
                allocation_words: (1 + fields.len() as u32 + 4 * u32::from(destroy.is_some())).max(2),
                identity_offset: destroy.map(|_| 8 * (1 + fields.len() as u32)),
                has_destroy: destroy.is_some(), may_reach_resource: true, may_async_destroy: false,
                storage_fields, trace_layout: traced.clone(), selection_layout: traced,
            },
            source_fields: (0..fields.len() as u32).collect(),
        });
    }
    let raw = generate_with_allocation(&module, &layouts).unwrap_or_else(|e| panic!("{}: {e}", module.name));
    let catalog = Catalog(raw.program.functions.iter().filter(|f| {
        f.proofs.entry.as_ref().is_some_and(|entry| entry.external.is_some())
    }).cloned().collect());
    let output = compile_low(&module, &layouts, o::VerifyContext {
        external_contracts: &OirContracts(&module), ..o::VerifyContext::default()
    }, &catalog).unwrap_or_else(|e| panic!("{}: {e}", module.name));
    (output, catalog)
}

fn check_extra_snapshot(name: &str, low: &o::Module, program: &s::Program) {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/codegen/sparrow_oir2/fixtures");
    for (suffix, text) in [("low.oir", o::text::print(low).unwrap()), ("spb17", s::disassemble(program).unwrap())] {
        let path = directory.join(format!("{name}.{suffix}"));
        if std::env::var_os("OKKAMA_UPDATE_S3_GOLDENS").is_some() {
            std::fs::write(&path, &text).unwrap();
        }
        assert_eq!(text, std::fs::read_to_string(path).unwrap(), "{name}.{suffix}");
    }
}

#[test]
fn s3_low_goldens() {
    for (name, source) in [
        ("spec16-1", include_str!("fixtures/spec16-1.low.oir")),
        ("spec16-4", include_str!("fixtures/spec16-4.low.oir")),
        ("valued", include_str!("fixtures/valued.low.oir")),
        ("local-pair", include_str!("fixtures/local-pair.low.oir")),
        ("fresh-call", include_str!("fixtures/fresh-call.low.oir")),
        ("mixed-journal", include_str!("fixtures/mixed-journal.low.oir")),
        ("unwind-reserve", include_str!("fixtures/unwind-reserve.low.oir")),
        ("return-old", include_str!("fixtures/return-old.low.oir")),
    ] {
        let (output, catalog) = emit(source);
        let program = &output.program;
        s::verify(program).unwrap();
        let bytes = s::encode(program).unwrap();
        let loaded = s::decode(&bytes).unwrap();
        s::verify_load(&loaded, &catalog).unwrap();
        assert_eq!(&loaded, program);
        assert_eq!(s::assemble(&s::disassemble(program).unwrap()).unwrap(), loaded);
        for (function, allocation) in program.functions.iter().zip(&output.allocations) {
            s::check_allocation(function, allocation).unwrap();
        }
        let assembly = s::disassemble(program).unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("src/codegen/sparrow_oir2/fixtures/{name}.spb17"));
        if std::env::var_os("OKKAMA_UPDATE_S3_GOLDENS").is_some() {
            std::fs::write(&path, &assembly).unwrap();
        }
        assert_eq!(assembly, std::fs::read_to_string(path).unwrap(), "{name}");
    }
}

fn rejects(program: &s::Program, catalog: &Catalog, expected: &str) {
    let failure = s::verify_load(program, catalog).unwrap_err().to_string();
    assert!(failure.contains(expected), "expected {expected}: {failure}");
}

#[test]
fn s3_loader_negatives_before_native_or_constructor_execution() {
    let (output, catalog) = emit(include_str!("fixtures/fresh-call.low.oir"));
    let mut forged = output.program.clone();
    let main = forged.functions.iter_mut().find(|f| f.symbol == "main").unwrap();
    let slot = main.proofs.cleanup_plans.iter_mut().flat_map(|plan| &mut plan.slots)
        .find(|slot| matches!(slot.origin, s::CreationOrigin::FreshResult { .. })).unwrap();
    if let s::CreationOrigin::FreshResult { callee, .. } = &mut slot.origin { *callee = slot.destroy; }
    rejects(&forged, &catalog, "fresh witness");

    let (valued, valued_catalog) = emit(include_str!("fixtures/valued.low.oir"));
    let mut b2 = valued.program;
    let main = b2.functions.iter_mut().find(|f| f.symbol == "main").unwrap();
    main.proofs.plans[0].merge = false;
    rejects(&b2, &valued_catalog, "B2");

    let (output, catalog) = emit(include_str!("fixtures/local-pair.low.oir"));
    let mut missing = output.program.clone();
    let main = missing.functions.iter_mut().find(|f| f.symbol == "main").unwrap();
    let (pc, location) = main.code.iter().enumerate().find_map(|(pc, inst)| match inst {
        s::Instruction::Champ { op: s::Opcode::CleanupDestroy | s::Opcode::DestroyDirect, arguments, .. } => {
            let s::Argument::Location(location) = arguments[0] else { panic!("cleanup operand") };
            Some((pc as u32, location))
        }
        _ => None,
    }).unwrap();
    for root in main.proofs.roots.iter_mut().filter(|root| root.pc == pc) {
        root.locations.retain(|&loc| loc != location);
    }
    let bytes = s::encode(&missing).unwrap();
    rejects(&s::decode(&bytes).unwrap(), &catalog, "missing abstract reference root");

    let mut arity = output.program.clone();
    let main = arity.functions.iter_mut().find(|f| f.symbol == "main").unwrap();
    let outcomes = main.code.iter_mut().find_map(|inst| match inst {
        s::Instruction::Champ { op: s::Opcode::CleanupNext, flow: s::Flow::Outcomes(outcomes), .. } => Some(outcomes),
        _ => None,
    }).unwrap();
    outcomes[0].edge.assignments.pop();
    rejects(&arity, &catalog, "edge arity");

    let mut repeat = output.program;
    let main = repeat.functions.iter_mut().find(|f| f.symbol == "main").unwrap();
    let direct = main.proofs.cleanup_plans.iter().find(|plan| plan.slots.len() == 2).unwrap();
    let first = direct.slots[0].identity;
    let second = direct.slots[1].identity;
    for inst in &mut main.code {
        if let s::Instruction::Champ { op: s::Opcode::DestroyDirect, arguments, .. } = inst
            && arguments[1] == s::Argument::Location(second)
        {
            arguments[1] = s::Argument::Location(first);
            break;
        }
    }
    for function in &mut repeat.functions {
        complete_codegen_roots(function).unwrap();
        let live = s::live_locations(function).unwrap();
        for (pc, locations) in live.iter().enumerate() {
            function.proofs.roots.push(s::RootRequirement {
                pc: pc as u32, locations: locations.iter().copied().collect(),
            });
        }
        for root in &mut function.proofs.post_roots {
            if let Some(after) = live.get(root.pc as usize + 1) {
                root.locations.extend(after);
            }
        }
    }
    rejects(&repeat, &catalog, "direct chain skips current");
}

#[test]
fn s3_exit_updates_and_input_verification() {
    let own = o::text::parse(include_str!("fixtures/exit-updates.own.oir")).unwrap();
    o::verify_with(&own, &o::VerifyOptions::default(), o::VerifyContext {
        external_contracts: &OirContracts(&own), ..o::VerifyContext::default()
    }).unwrap();
    let low = o::low::lower_cleanup(own, &o::low::LowOptions::default()).unwrap();
    let (output, _) = emit(&o::text::print(&low).unwrap());
    check_extra_snapshot("exit-updates", &low, &output.program);
    for opcode in [s::Opcode::ExitExtend, s::Opcode::ExitReplace, s::Opcode::ExitCommit] {
        assert!(output.program.functions.iter().flat_map(|f| &f.code).any(|instruction| {
            matches!(instruction, s::Instruction::Champ { op, .. } if *op == opcode)
        }));
    }
    let mut invalid = low;
    for (_, function) in invalid.functions.iter_mut() {
        if let Some(body) = &mut function.body { body.cleanup.roots = o::Arena::default(); }
    }
    let failure = compile_low(&invalid, &BTreeMap::new(), o::VerifyContext {
        external_contracts: &OirContracts(&invalid), ..o::VerifyContext::default()
    }, &s::NoExternalContracts).unwrap_err();
    assert!(failure.0.contains("verify_low"), "{failure}");
}

#[test]
fn s3_concrete_key_roots_and_independent_root_checker() {
    let (output, _) = emit(include_str!("fixtures/spec16-1.low.oir"));
    for (function, allocation) in output.program.functions.iter().zip(output.allocations) {
        for (location, &slot) in &allocation.locations {
            if let s::Location::Service(id) = location
                && function.services[*id as usize] == s::ServiceKind::StableKey
            {
                assert_eq!(s::layout::service_layout(s::ServiceKind::StableKey).words, 2);
                assert!(allocation.roots.values().flatten().any(|root| root.slot == slot && root.offset == 0));
                assert!(!allocation.roots.values().flatten().any(|root| root.slot == slot && root.offset == 8));
            }
        }
        if !allocation.roots.is_empty() {
            let mut missing = allocation.clone();
            missing.roots.clear();
            assert!(s::check_allocation(function, &missing).unwrap_err().to_string().contains("concrete roots"));
        }
    }
}

#[test]
fn s3_reserve_executor_has_no_compiled_direct_prefix() {
    let own = o::text::parse(include_str!("fixtures/reserve-executor.own.oir")).unwrap();
    o::verify_with(&own, &o::VerifyOptions::default(), o::VerifyContext {
        external_contracts: &OirContracts(&own), ..o::VerifyContext::default()
    }).unwrap();
    let low = o::low::lower_cleanup(own, &o::low::LowOptions::default()).unwrap();
    let (output, _) = emit(&o::text::print(&low).unwrap());
    check_extra_snapshot("reserve-executor", &low, &output.program);
    let main = output.program.functions.iter().find(|function| function.symbol == "main").unwrap();
    assert!(main.proofs.plans.iter().all(|plan| plan.merge));
    assert!(!main.code.iter().any(|inst| matches!(inst,
        s::Instruction::Champ { op: s::Opcode::DestroyDirect, .. })));
}

#[test]
fn s3_explicit_abort_consumes_pending_once_before_executor_plan() {
    let mut low = o::text::parse(include_str!("fixtures/unwind-reserve.low.oir")).unwrap();
    let body = low.functions.iter_mut().find(|(_, f)| f.symbol == "main").unwrap().1.body.as_mut().unwrap();
    let (begin, state, pending, close) = body.insts.iter_mut().find_map(|(id, inst)| match &mut inst.op {
        o::Operation::Cleanup(o::CleanupOp::Begin { pending, close, .. }) if pending.is_some() => {
            Some((id, inst.results[0], pending.take().unwrap(), *close))
        }
        _ => None,
    }).unwrap();
    body.cleanup.closes.get_mut(close).unwrap().reserve = false;
    for (_, plan) in body.cleanup.plans.iter_mut().filter(|(_, plan)| plan.close == close) {
        // Abort still contributes its whole reserve (G1 rev 2 §§6.3–6.6).
        assert!(plan.sources.contains(&o::MergeSource::WholeReserveOnFailure));
    }
    let instruction = body.insts.alloc(o::Instruction {
        results: vec![], op: o::Operation::Exit(o::ExitOp::Abort { pending, cleanup: state }),
        facts: o::InstructionFacts::default(), loc: None,
    }).unwrap();
    let aborted = body.values.alloc(o::ValueData {
        ty: body.values.get(state).unwrap().ty,
        def: o::Definition::InstResult { inst: instruction, index: 0 },
        facts: o::ValueFacts::Unelaborated,
    }).unwrap();
    body.insts.get_mut(instruction).unwrap().results.push(aborted);
    for (_, block) in body.blocks.iter_mut() {
        if let Some(index) = block.insts.iter().position(|&id| id == begin) {
            block.insts.insert(index + 1, instruction);
        }
    }
    let settle = body.insts.iter_mut().find_map(|(id, inst)| match &mut inst.op {
        o::Operation::Cleanup(o::CleanupOp::SettleEmpty { cleanup, .. }) if *cleanup == state => {
            *cleanup = aborted;
            Some(id)
        }
        _ => None,
    }).unwrap();
    let mut before = None;
    let mut after = None;
    for (_, roots) in body.cleanup.roots.iter_mut() {
        if roots.boundary == o::RootBoundary::AfterInstruction(begin) {
            roots.bindings.push(o::RootBinding { value: pending, trace: o::RootTrace::PendingValueReserveDeps });
            before = Some(roots.bindings.clone());
        }
        if roots.boundary == o::RootBoundary::BeforeInstruction(settle) {
            for binding in &mut roots.bindings {
                if binding.value == state { binding.value = aborted; }
            }
            after = Some(roots.bindings.clone());
        }
    }
    for (boundary, bindings) in [
        (o::RootBoundary::BeforeInstruction(instruction), before.unwrap()),
        (o::RootBoundary::AfterInstruction(instruction), after.unwrap()),
    ] {
        body.cleanup.roots.alloc(o::RootRequirement { boundary, bindings, through_suspend: false }).unwrap();
    }
    let (output, _) = emit(&o::text::print(&low).unwrap());
    check_extra_snapshot("explicit-abort", &low, &output.program);
    let main = output.program.functions.iter().find(|f| f.symbol == "main").unwrap();
    assert!(main.proofs.cleanup_plans[0].reserve);
    assert!(!main.proofs.closes[0].reserve);
}

#[test]
fn s3_after_roots_follow_the_last_instruction_of_an_expansion() {
    let own = o::text::parse(r#"oir 2 own
module expanded_roots
scope S action in @main
summary M0 internal parameters [parameter_effect(convention=borrow, destination=none)]
    results [result_relation(provenance=bound)]
    contract call_contract(results=[int], failures=[thrown, program, panic, native], publication=atomic)
func @main(%range: range) -> int summary(M0) {
^entry in S:
    %length: int = range.len %range
    ret %length
}
"#).unwrap();
    let low = o::low::lower_cleanup(own, &o::low::LowOptions::default()).unwrap();
    emit(&o::text::print(&low).unwrap());
}

#[test]
fn s3_missing_low_root_is_not_synthesized_by_emitter() {
    let source = include_str!("fixtures/local-pair.low.oir");
    let (valid, catalog) = emit(source);
    let layouts = valid.program.objects.iter().enumerate().map(|(id, object)| {
        (id as u32, ObjectLayout { runtime: object.layout.clone(),
            source_fields: object.fields.iter().map(|field| field.storage).collect() })
    }).collect::<BTreeMap<_, _>>();
    let mut low = o::text::parse(source).unwrap();
    let body = low.functions.iter_mut().find(|(_, f)| f.symbol == "main").unwrap().1.body.as_mut().unwrap();
    let parameter = body.blocks.get(body.entry).unwrap().params[0];
    let empty = body.insts.iter().find_map(|(id, inst)| matches!(inst.op,
        o::Operation::Ownership(o::OwnershipOp::Empty {})).then_some(id)).unwrap();
    for (_, root) in body.cleanup.roots.iter_mut() {
        if root.boundary == o::RootBoundary::BeforeInstruction(empty) {
            root.bindings.retain(|binding| binding.value != parameter);
        }
    }
    // Bypass verify_low deliberately, modelling an upstream #591 error. The independent SPB gate
    // must see the missing evidence, rather than receiving a repaired copy of its own inference.
    let output = generate_with_allocation(&low, &layouts).unwrap();
    let failure = s::verify_load(&output.program, &catalog).unwrap_err();
    assert_eq!(failure.rule, "ROOTS_BEFORE", "{failure}");
}

#[test]
fn s3_low_rejects_missing_whole_reserve_after_explicit_abort() {
    let mut low = o::text::parse(include_str!("fixtures/explicit-abort.low.oir")).unwrap();
    let body = low.functions.iter_mut().find(|(_, f)| f.symbol == "main").unwrap().1.body.as_mut().unwrap();
    body.cleanup.plans.get_mut(o::CleanupPlanId::from_u32(0)).unwrap().sources
        .retain(|source| *source != o::MergeSource::WholeReserveOnFailure);
    let failure = o::verify_with(&low, &o::VerifyOptions::default(), o::VerifyContext {
        external_contracts: &OirContracts(&low), ..o::VerifyContext::default()
    }).unwrap_err();
    assert!(failure.diagnostics.iter().any(|diagnostic| diagnostic.rule == o::RuleId::B101));
}
