use super::*;
use super::synchronous::{fixtures, roundtrip, TrustedNative};

fn runtime_contract(module: &Module, profile: Profile) -> Option<TrustedNative> {
    let cleanup = module.functions.iter().find(|(_, function)| function.symbol == "drop_fn")?.1;
    let callable = cleanup.signature.params[0];
    let mut types = module.types.clone();
    let mut signature = Signature {
        params: vec![callable], results: vec![], ownership_abi: OwnershipAbi::Unelaborated,
    };
    if profile == Profile::Own {
        let deps = types.intern(Type::Service(ServiceType::Deps)).unwrap();
        let own = types.intern(Type::Service(ServiceType::Own)).unwrap();
        signature.params.extend([deps, own]);
        signature.results.push(own);
        signature.ownership_abi = OwnershipAbi::Explicit { input_packet: 2, output_packet: 0 };
    }
    Some(TrustedNative {
        name: "oir.sync_closure.cleanup", signature,
        summary: Summary {
            entry_kind: EntryKind::CallBoundary,
            parameters: vec![ParameterEffect {
                convention: FreshConvention::Take, retention: None, deps: vec![],
                destination: TransferDestination::None,
            }],
            results: vec![],
            contract: CallContract {
                results: vec![], failures: vec![], suspends: false, async_destroy: false,
                publication: Publication::Atomic, protocol: None,
            },
        },
    })
}

fn context(trusted: &Option<TrustedNative>) -> VerifyContext<'_> {
    match trusted {
        Some(trusted) => VerifyContext { text_locations: None, external_contracts: trusted },
        None => VerifyContext::default(),
    }
}

fn rewrite(source: &str) -> OwnOutput {
    let module = crate::text::parse(source).unwrap();
    let high = runtime_contract(&module, Profile::High);
    if high.is_some() {
        verify_with(&module, &VerifyOptions::default(), context(&high)).unwrap();
    } else {
        verify_high(&module).unwrap();
    }
    let mut catalog = TestNatives::default();
    for (protocol, data) in module.protocols.iter() {
        if data.kind == ProtocolKind::Services {
            catalog.protocols.insert(protocol, Summary {
                entry_kind: EntryKind::Internal, parameters: vec![], results: vec![],
                contract: CallContract {
                    results: vec![], failures: vec![], suspends: false, async_destroy: false,
                    publication: Publication::Atomic, protocol: None,
                },
            });
        }
    }
    if let Some(trusted) = high {
        let callable = trusted.signature.params[0];
        let summary = module.summaries.iter().find(|(_, summary)| summary.parameters.len() == 2).unwrap().1;
        catalog.callable.insert(callable, summary.clone());
        catalog.named.insert(trusted.name.into(), trusted.summary);
    }
    let output = elaborate_ownership(module, &catalog, &OwnOptions::default()).unwrap();
    let trusted = runtime_contract(&output.module, Profile::Own);
    rewrite_synchronous_with(output, &OwnOptions::default(), context(&trusted)).unwrap()
}

fn verify_output(module: &Module) -> VerifyStats {
    let trusted = runtime_contract(module, Profile::Own);
    if trusted.is_none() {
        return verify_own(module).unwrap().stats().clone();
    }
    verify_with(module, &VerifyOptions::default(), context(&trusted)).unwrap().stats().clone()
}

#[test]
fn commit4_high_characterization() {
    for path in fixtures("protocol-high") {
        let source = std::fs::read_to_string(&path).unwrap();
        let output = rewrite(&source);
        let name = path.file_stem().unwrap().to_str().unwrap();
        let expected = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/own/tests/protocol-goldens");
        let golden = std::fs::read_to_string(expected.join(format!("{name}.oir"))).unwrap();
        let golden = crate::text::parse(&golden).unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert_eq!(crate::text::print(&output.module).unwrap(), crate::text::print(&golden).unwrap(), "{name}");
        verify_output(&golden);
        let stats = std::fs::read_to_string(expected.join(format!("{name}.stats"))).unwrap();
        assert_eq!(output.stats.report(&output.census), stats, "{name}");
        let parsed = roundtrip(&output.module);
        let verified = verify_output(&parsed);
        for reason in [DynamicReason::Atomic, DynamicReason::Cell, DynamicReason::Channel, DynamicReason::Group,
            DynamicReason::Any, DynamicReason::Closure, DynamicReason::Merge] {
            assert_eq!(verified.dynamic.census.module_count(reason), output.census.module_count(reason));
            for (function, data) in output.module.functions.iter() {
                let parsed_function = parsed.functions.iter().find(|(_, function)| function.symbol == data.symbol)
                    .unwrap().0;
                assert_eq!(verified.dynamic.census.function_count(parsed_function, reason),
                    output.census.function_count(function, reason));
            }
        }
        assert_eq!(output.census.module_count(DynamicReason::Merge), 0);
        assert_eq!(output.stats.splits_considered, output.stats.splits_elided + output.stats.splits_left_unelided);
        let high = crate::text::parse(&source).unwrap();
        let canonical = crate::text::print(&high).unwrap();
        let repeated = rewrite(&canonical);
        assert!(crate::text::normalized_eq(&output.module, &repeated.module).unwrap(), "{name}");
    }
}

#[test]
fn opcode_routing_is_exhaustive_for_classification_and_rewrite() {
    let mut operations = vec![Operation::Const(Literal::Int("0".into()))];
    operations.extend(CoreOp::representatives().into_iter().map(Operation::Core));
    operations.extend(CallOp::representatives().into_iter().map(Operation::Call));
    operations.extend(CollectionOp::representatives().into_iter().map(Operation::Collection));
    operations.extend(LexicalOp::representatives().into_iter().map(Operation::Lexical));
    operations.extend(OwnershipOp::representatives().into_iter().map(Operation::Ownership));
    operations.extend(AsyncOp::representatives().into_iter().map(Operation::Async));
    operations.extend(CarrierOp::representatives().into_iter().map(Operation::Carrier));
    operations.extend(ExitOp::representatives().into_iter().map(Operation::Exit));
    operations.extend(CleanupOp::representatives().into_iter().map(Operation::Cleanup));
    let mut opcodes = BTreeSet::new();
    for op in operations {
        let _class = routing::handling(&op);
        assert!(opcodes.insert(op.opcode()));
    }
    assert_eq!(opcodes, Operation::opcode_inventory().into_iter().collect());
}

fn reject(module: &Module, rule: RuleId) {
    for module in [module.clone(), roundtrip(module)] {
        let trusted = runtime_contract(&module, Profile::Own);
        let report = verify_with(&module, &VerifyOptions::default(), context(&trusted)).unwrap_err();
        assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == rule), "{report:?}");
    }
}

fn make_destroy_async(module: &mut Module) {
    let function = module.functions.iter().find(|(_, function)| function.symbol == "destroy").unwrap().0;
    let data = module.functions.get_mut(function).unwrap();
    data.attrs.push(FunctionAttr::Async);
    let summary = module.summaries.get_mut(data.summary.unwrap()).unwrap();
    summary.contract.suspends = true;
    summary.contract.async_destroy = true;
}

#[test]
fn cap_and_role_effects_follow_hidden_owner_graphs() {
    for name in ["cap-parameter", "role-parameter", "bound-role", "role-call"] {
        let source = std::fs::read_to_string(fixtures("protocol-high").iter()
            .find(|path| path.file_stem().unwrap() == name).unwrap()).unwrap();
        let mut output = rewrite(&source).module;
        verify_output(&output);
        make_destroy_async(&mut output);
        reject(&output, RuleId::B112);
    }
}

#[test]
fn opaque_dictionary_cannot_certify_its_own_scope_close() {
    let mut module = rewrite(include_str!("protocol-high/closure-open.oir")).module;
    let (target, data) = module.functions.iter().find(|(_, function)| function.symbol == "drop_fn").unwrap();
    let ty = data.signature.params[0];
    let summary = data.summary.unwrap();
    module.functions.get_mut(target).unwrap().attrs.clear();
    let root = module.scopes.alloc(ScopeDecl {
        function: target, parent: None, kind: ScopeKind::Action, loc: None,
    }).unwrap();
    let mut builder = FunctionBuilder::new(&mut module, target, root).unwrap();
    let params = builder.block_params(builder.entry()).unwrap().to_vec();
    let deps = builder.intern_dependency(DepExpr::FromSsa(params[1])).unwrap();
    builder.set_value_facts(params[0], ValueFacts::Managed { deps }).unwrap();
    let state = builder.emit_schema(Operation::Ownership(OwnershipOp::ScopeBegin { scope: root })).unwrap()[0];
    let state = builder.emit_schema(Operation::Ownership(OwnershipOp::Accept { state, packet: params[2] }))
        .unwrap()[0];
    builder.emit_schema(Operation::Core(CoreOp::TypeOf {
        ty, descriptor: DescriptorContract {
            managed: DescriptorManaged::Runtime, retention: SummarySelection::OwnerGraph(0),
            cleanup: target, async_destroy: false, summary,
        },
    })).unwrap();
    builder.emit_schema(Operation::Ownership(OwnershipOp::ScopeEnd {
        state, reserve: None, effects: ScopeEffectEnvelope::default(),
    })).unwrap();
    let empty = builder.emit_schema(Operation::Ownership(OwnershipOp::Empty {})).unwrap()[0];
    builder.terminate(TermKind::Ret { values: vec![empty] }).unwrap();
    let body = builder.finish().unwrap();
    module.functions.get_mut(target).unwrap().body = Some(body);
    reject(&module, RuleId::B112);
}

#[test]
fn every_emitted_dynamic_reason_rejects_a_different_family() {
    for reason in ["atomic", "cell", "channel", "group", "any", "closure"] {
        let source = std::fs::read_to_string(fixtures("protocol-high").iter()
            .find(|path| path.file_stem().unwrap() == format!("dynamic-{reason}").as_str()).unwrap()).unwrap();
        let mut module = rewrite(&source).module;
        let mut changed = 0;
        for (_, data) in module.functions.iter_mut() {
            let Some(body) = &mut data.body else { continue };
            for (_, block) in body.blocks.iter_mut() {
                if let TermKind::InvokeOwn { op, .. } = &mut block.term.kind
                    && let Operation::Ownership(OwnershipOp::Dynamic { reason, .. }) = op.as_mut() {
                    *reason = if *reason == DynamicReason::Any { DynamicReason::Closure } else { DynamicReason::Any };
                    changed += 1;
                }
            }
        }
        assert_eq!(changed, 1);
        reject(&module, RuleId::B71);
    }
}

#[test]
fn once_rejects_a_resource_hidden_behind_an_ordinary_field() {
    let source = include_str!("protocol-high/once-plain.oir");
    let hidden = "object Hidden {\n    field resource: R\n}\nobject R {\n    destroy @destroy\n}\nobject O {";
    let source = source.replace("object O {", hidden)
        .replace("field slot: int once", "field slot: Hidden once")
        .replace("%value: int", "%value: Hidden").replace("-> int [method", "-> Hidden [method")
        .replace("%result: int", "%result: Hidden");
    let destroy = concat!("scope SD action in @destroy\n", "func @destroy(%self: R) -> void [destroy R] {\n",
        "    ^entry in SD:\n    ret\n}\n");
    let source = format!("{source}\n{destroy}");
    let module = crate::text::parse(&source).unwrap();
    for module in [module.clone(), roundtrip(&module)] {
        let report = verify_high(&module).unwrap_err();
        assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == RuleId::B111));
    }
}

#[test]
fn epoch_updates_are_one_conditional_transition_and_ssa_moves_preserve_them() {
    let output = rewrite(include_str!("protocol-high/dynamic-channel.oir"));
    let module = &output.module;
    let state = ValueId::from_u32(0);
    let packet = ValueId::from_u32(1);
    for op in [
        OwnershipOp::Admit { state, packet },
        OwnershipOp::Accept { state, packet },
        OwnershipOp::ScopeTransfer { states: TransferStates::Same(state), selection: SelectionId::from_u32(0) },
        OwnershipOp::ScopeTransfer {
            states: TransferStates::Between { from: state, to: packet }, selection: SelectionId::from_u32(0),
        },
    ] {
        let contract = ContractRegistry::new(module).lookup(&Operation::Ownership(op)).unwrap();
        // The condition compares logical holder/owner together. Same owner and
        // holder means zero updates; changing both is still one transition.
        assert_eq!(contract.normal.postconditions.iter().filter(|condition| {
            **condition == Postcondition::EpochOnRealTransfer
        }).count(), 1);
    }
    let contract = ContractRegistry::new(module).lookup(&Operation::Ownership(OwnershipOp::Move { packet })).unwrap();
    assert!(!contract.normal.postconditions.contains(&Postcondition::EpochOnRealTransfer));
    for (_, function) in module.functions.iter() {
        let Some(body) = &function.body else { continue };
        for (_, op) in summaries::operations(body) {
            if matches!(op, Operation::Ownership(OwnershipOp::Dynamic { .. })) {
                let contract = ContractRegistry::new(module).lookup(op).unwrap();
                assert_eq!(contract.normal.postconditions.iter().filter(|condition| {
                    **condition == Postcondition::EpochOnRealTransfer
                }).count(), 1);
            }
        }
    }
}

#[test]
fn captures_and_bound_receivers_keep_snapshot_deps_without_an_extra_identity() {
    let source = include_str!("protocol-high/bound-snapshot.oir");
    let module = crate::text::parse(source).unwrap();
    let output = elaborate_ownership(module, &TestNatives::default(), &OwnOptions::default()).unwrap();
    let (function, data) = output.module.functions.iter().find(|(_, function)| function.symbol == "f").unwrap();
    let body = data.body.as_ref().unwrap();
    let flow = analyze::analyze_function(&output.module, function, &output.analysis, &OwnOptions::default()).unwrap();
    let (_, closure) = body.insts.iter().find(|(_, instruction)| {
        matches!(instruction.op, Operation::Core(CoreOp::FnBind { .. }))
    }).unwrap();
    let (call, _) = body.insts.iter().find(|(_, instruction)| {
        matches!(instruction.op, Operation::Call(CallOp::Closure { .. }))
    }).unwrap();
    let Operation::Core(CoreOp::FnBind { receiver, .. }) = closure.op else { unreachable!() };
    let state = &flow.before_instructions[&call];
    assert_eq!(state.values[closure.results[0].as_u32() as usize].deps,
        state.values[receiver.as_u32() as usize].deps);
    assert!(state.values[receiver.as_u32() as usize].deps.iter().all(|atom| {
        atom.incarnation == domain::Incarnation::History
    }));
    assert_eq!(state.values[closure.results[0].as_u32() as usize].deps.len(), 1);
}

#[test]
fn alpha_renaming_and_arena_renumbering_preserve_callable_elaboration() {
    for source in [include_str!("protocol-high/closure-known-phi.oir"),
        include_str!("protocol-high/bound-snapshot.oir"), include_str!("protocol-high/cap-hidden.oir")] {
        let output = rewrite(source);
        let renamed = rewrite(&source.replace("%r", "%snapshot"));
        assert!(crate::text::normalized_eq(&output.module, &renamed.module).unwrap());
        let mut high = crate::text::parse(source).unwrap();
        let functions: Vec<_> = high.functions.iter().map(|(id, _)| id).collect();
        for function in functions {
            let Some(mut body) = high.functions.get_mut(function).unwrap().body.take() else { continue };
            FunctionEditor::new(&mut high, &mut body).renumber().unwrap();
            high.functions.get_mut(function).unwrap().body = Some(body);
        }
        let numbered = rewrite(&crate::text::print(&high).unwrap());
        assert!(crate::text::normalized_eq(&output.module, &numbered.module).unwrap());
    }
}

#[test]
fn dynamic_and_open_linear_outcome_slots_cannot_reuse_consumed_inputs() {
    for name in ["dynamic-atomic", "dynamic-cell", "dynamic-channel", "dynamic-group", "dynamic-any",
        "dynamic-closure", "closure-open"] {
        let source = std::fs::read_to_string(fixtures("protocol-high").iter()
            .find(|path| path.file_stem().unwrap() == name).unwrap()).unwrap();
        let module = rewrite(&source).module;
        let mut mutations = vec![];
        for (function, data) in module.functions.iter() {
            let Some(body) = &data.body else { continue };
            for (block, data) in body.blocks.iter() {
                let TermKind::InvokeOwn { op, .. } = &data.term.kind else { continue };
                let contract = ContractRegistry::new(&module).lookup(op).unwrap();
                for (outcome, schema) in [(Outcome::Normal, Some(&contract.normal)),
                    (Outcome::Unwind, contract.unwind.as_ref())] {
                    let Some(schema) = schema else { continue };
                    for (slot, info) in schema.results.iter().enumerate() {
                        let input = match (op.as_ref(), info.name) {
                            (Operation::Ownership(OwnershipOp::Dynamic { state, .. }), SlotName::State) => *state,
                            (Operation::Ownership(OwnershipOp::Dynamic { packet, .. }), SlotName::Packet) => *packet,
                            (Operation::Call(CallOp::Closure { call, .. }), SlotName::Packet) => call.packet.unwrap(),
                            _ => continue,
                        };
                        mutations.push((function, block, outcome, slot as u16, input));
                    }
                }
                if let Operation::Call(CallOp::Closure { .. }) = op.as_ref() {
                    let unwind = contract.unwind.as_ref().unwrap();
                    assert!(unwind.results.iter().all(|slot| slot.name != SlotName::Packet));
                    assert!(unwind.postconditions.contains(&Postcondition::CalleeInputsSettled));
                }
            }
        }
        assert!(!mutations.is_empty(), "{name}");
        for (function, block, outcome, slot, input) in mutations {
            let mut invalid = module.clone();
            let body = invalid.functions.get_mut(function).unwrap().body.as_mut().unwrap();
            let TermKind::InvokeOwn { normal, unwind, .. } = &mut body.blocks.get_mut(block).unwrap().term.kind
                else { unreachable!() };
            let edge = if outcome == Outcome::Normal { normal } else { unwind };
            let mut changed = 0;
            for argument in &mut edge.args {
                if matches!(argument, EdgeArg::Produced(result) if result.outcome == outcome && result.slot == slot) {
                    *argument = EdgeArg::Value(input);
                    changed += 1;
                }
            }
            assert_eq!(changed, 1);
            reject(&invalid, RuleId::B62);
        }
    }
}

#[test]
fn commit4_negative_goldens_preserve_the_rule_and_exact_site() {
    for path in fixtures("protocol-invalid") {
        let source = std::fs::read_to_string(&path).unwrap();
        let expected = source.lines().find_map(|line| line.strip_prefix("// expect ")).unwrap();
        let (rule, primary) = expected.split_once(' ').unwrap();
        let module = crate::text::parse(&source).unwrap();
        for module in [module.clone(), roundtrip(&module)] {
            let trusted = runtime_contract(&module, module.profile);
            let report = verify_with(&module, &VerifyOptions::default(), context(&trusted)).unwrap_err();
            assert!(report.diagnostics.iter().any(|diagnostic| {
                diagnostic.rule.as_str() == rule && format!("{:?}", diagnostic.primary.entity) == primary
            }), "{}: {report:?}", path.display());
        }
    }
}

#[test]
fn plain_dictionary_certificate_requires_its_exact_binding() {
    let mut module = rewrite(include_str!("protocol-high/descriptor-plain-parameter.oir")).module;
    verify_output(&module);
    for (_, function) in module.functions.iter_mut() {
        if let Some(body) = &mut function.body {
            for binding in &mut body.descriptor_bindings {
                binding.contract.managed = DescriptorManaged::Runtime;
            }
        }
    }
    reject(&module, RuleId::B111);

}

#[test]
fn reviewed_protocol_manifest_covers_every_pair_and_precision_report() {
    let manifest = include_str!("protocol-manifest.tsv");
    let mut listed = BTreeSet::new();
    for line in manifest.lines().filter(|line| !line.starts_with('#')).skip(1) {
        let row: Vec<_> = line.split('\t').collect();
        assert_eq!(row.len(), 8);
        assert_eq!(row[7], "verified");
        assert!(listed.insert(row[0].to_owned()));
    }
    let high: BTreeSet<_> = fixtures("protocol-high").iter()
        .map(|path| path.file_stem().unwrap().to_str().unwrap().to_owned()).collect();
    assert_eq!(listed.len(), 29);
    assert_eq!(listed, high);
    for extension in ["oir", "stats"] {
        let own: BTreeSet<_> = fixtures("protocol-goldens").iter()
            .filter(|path| path.extension().unwrap() == extension)
            .map(|path| path.file_stem().unwrap().to_str().unwrap().to_owned()).collect();
        assert_eq!(own, high);
    }
    let reported: BTreeSet<_> = include_str!("protocol-census.tsv").lines()
        .filter(|line| !line.starts_with('#')).skip(1)
        .filter_map(|line| line.split_once('\t'))
        .filter(|(name, _)| *name != "TOTAL_peak_max")
        .map(|(name, _)| name.to_owned()).collect();
    assert_eq!(reported, high);
    assert_eq!(fixtures("protocol-invalid").len(), 13);
}
