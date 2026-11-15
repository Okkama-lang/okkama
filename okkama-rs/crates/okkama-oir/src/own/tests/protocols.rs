use super::*;
use super::synchronous::{fixtures, roundtrip};
use crate::catalogue::RuntimeCatalogue;

// The real runtime catalogue: open sync callables and their cleanup intrinsic
// are registered only for the exact Closure protocol and ABI (#569 O1).
fn runtime_contract(module: &Module, profile: Profile) -> RuntimeCatalogue {
    RuntimeCatalogue::for_profile(module, profile)
}

fn context(trusted: &RuntimeCatalogue) -> VerifyContext<'_> {
    VerifyContext { text_locations: None, external_contracts: trusted }
}

fn try_rewrite(source: &str) -> Result<OwnOutput, OwnError> {
    let module = crate::text::parse(source).unwrap();
    let high = runtime_contract(&module, Profile::High);
    verify_with(&module, &VerifyOptions::default(), context(&high)).unwrap();
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
    for (ty, _) in module.types.iter() {
        if let Some(summary) = high.open_callable(ty) {
            catalog.callable.insert(ty, summary.clone());
        }
    }
    let cleanup = crate::catalogue::SYNC_CLOSURE_CLEANUP;
    if let Some(summary) = high.cleanup_summary(cleanup) {
        catalog.named.insert(cleanup.into(), summary.clone());
    }
    let output = elaborate_ownership(module, &catalog, &OwnOptions::default()).unwrap();
    let trusted = runtime_contract(&output.module, Profile::Own);
    rewrite_synchronous_with(output, &OwnOptions::default(), context(&trusted))
}

fn rewrite(source: &str) -> OwnOutput {
    try_rewrite(source).unwrap()
}

fn verify_output(module: &Module) -> VerifyStats {
    let trusted = runtime_contract(module, Profile::Own);
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

/// A once slot of `fn() -> int` filled by a closure that captures one `{captured}` value, with `{extra}`
/// added to the module (#717).
fn once_closure(captured: &str, extra: &str) -> String {
    format!("oir 2 high
module once_closure
object Plain {{
    field n: int
}}
object R {{
    destroy @destroy
}}
object O {{
    field slot: fn() -> int once
}}
{extra}
scope SD action in @destroy
func @destroy(%self: R) -> void [destroy R] {{
    ^entry in SD:
    ret
}}
scope SL action in @lambda
func @lambda(%captured: {captured}) -> int [lambda captures 1] {{
    boundary BL lexical in SL
    ^entry in SL:
    %n: int = const 7
    [ip] exit.prepare return %n at BL
    result.exit edge normal(0), value %n, demand want, prepare ip
    ret %n
}}
scope S action in @f
func @f(%self: O, %captured: {captured}) -> int [method O] {{
    boundary B0 lexical in S
    ^entry in S:
    %closure: fn() -> int = fn.closure @lambda, [%captured], ty fn() -> int
    once.store %self, slot, %closure
    %kept: fn() -> int = once.load %self, slot
    %n: int = const 1
    [ip] exit.prepare return %n at B0
    result.exit edge normal(0), value %n, demand want, prepare ip
    ret %n
}}
")
}

/// The rules the full verifier rejects the module by; empty when it verifies completely.
fn once_rules(source: &str) -> Vec<RuleId> {
    let module = crate::text::parse(source).unwrap();
    verify_high(&module).err().map_or(vec![], |report| {
        report.diagnostics.iter().map(|diagnostic| diagnostic.rule).collect()
    })
}

/// Whether the full verifier rejects the module by its once slot alone.
fn once_rejected(source: &str) -> bool {
    let rules = once_rules(source);
    assert!(rules.iter().all(|rule| *rule == RuleId::B111), "{rules:?}");
    !rules.is_empty()
}

/// Bible §13.13 (#491, #520, #717): a once slot keeps a closure only when no closure of its function type can
/// retain a resource in this program (B11.1 counts a function type by the closures the module creates).
#[test]
fn once_keeps_a_closure_only_when_no_closure_of_its_type_retains_a_resource() {
    assert!(!once_rejected(&once_closure("Plain", "")));
    assert!(once_rejected(&once_closure("R", "")));
    // A second closure of the same type that captures a resource is enough.
    let second = "scope SL2 action in @second
func @second(%captured: R) -> int [lambda captures 1] {
    boundary BL2 lexical in SL2
    ^entry in SL2:
    %n: int = const 2
    [ip] exit.prepare return %n at BL2
    result.exit edge normal(0), value %n, demand want, prepare ip
    ret %n
}
scope S2 action in @make
func @make(%r: R) -> fn() -> int {
    boundary B2 lexical in S2
    ^entry in S2:
    %closure: fn() -> int = fn.closure @second, [%r], ty fn() -> int
    want %closure
    [ip] exit.prepare return %closure at B2
    result.exit edge normal(0), value %closure, demand want, prepare ip
    ret %closure
}";
    assert!(once_rejected(&once_closure("Plain", second)));
    // A bodyless function may return a closure from outside the module (the fixture's intrinsic is not in
    // the catalog, T5, which does not hide the once rule).
    let foreign = "func @foreign() -> fn() -> int [intrinsic \"fixture.foreign\"];";
    assert!(once_rules(&once_closure("Plain", foreign)).contains(&RuleId::B111));
}

/// A builtin capability and `any` are never once data (#717).
#[test]
fn once_refuses_builtin_capabilities_and_any() {
    let source = |slot: &str| format!("oir 2 high
module once_opaque
object Atomic [builtin] {{
}}
object O {{
    field slot: {slot} once
}}
");
    assert!(!once_rejected(&source("int")));
    assert!(once_rejected(&source("Atomic")));
    assert!(once_rejected(&source("any")));
}

/// A once slot keeps a role value only when no implementation of the role retains a resource (#717).
#[test]
fn once_keeps_a_role_value_only_when_no_implementation_retains_a_resource() {
    let source = |holder: &str| format!("oir 2 high
module once_role
role Named {{
    name() -> int
}}
object R {{
    destroy @destroy
}}
object Holder {{
    does Named {{ name = @name }}
    field held: {holder}
}}
object O {{
    field slot: Named once
}}
scope SD action in @destroy
func @destroy(%self: R) -> void [destroy R] {{
    ^entry in SD:
    ret
}}
scope SN action in @name
func @name(%self: Holder) -> int [method Holder] {{
    boundary BN lexical in SN
    ^entry in SN:
    %n: int = const 7
    [ip] exit.prepare return %n at BN
    result.exit edge normal(0), value %n, demand want, prepare ip
    ret %n
}}
");
    assert!(!once_rejected(&source("int")));
    assert!(once_rejected(&source("R")));
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
    assert_eq!(state.values[closure.results[0]].deps,
        state.values[receiver].deps);
    assert!(state.values[receiver].deps.iter().all(|atom| {
        atom.incarnation == domain::Incarnation::History
    }));
    assert_eq!(state.values[closure.results[0]].deps.len(), 1);
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


// #569 O1: the open contract comes from the runtime catalogue. Own refuses a
// narrower catalogue entry, and the verifier refuses an unregistered one.
#[test]
fn open_calls_require_the_registered_conservative_contract() {
    let source = include_str!("protocol-high/closure-open.oir");
    let output = rewrite(source).module;
    let report = verify_own(&output).unwrap_err();
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == RuleId::B55), "{report:?}");

    let module = crate::text::parse(source).unwrap();
    let catalogue = RuntimeCatalogue::for_module(&module);
    let (callable, mut identity) = module.types.iter()
        .find_map(|(ty, _)| catalogue.open_callable(ty).map(|summary| (ty, summary.clone()))).unwrap();
    // The hand-written identity Mopen: the result only aliases the argument.
    identity.results[0] = ResultRelation {
        fresh: vec![], aliases: vec![SummarySelection::Parameter(1)], external_deps: vec![1], opaque: false,
        provenance: AdmissionProvenance::UnboundResult,
    };
    let mut natives = TestNatives::default();
    natives.callable.insert(callable, identity);
    let cleanup = crate::catalogue::SYNC_CLOSURE_CLEANUP;
    natives.named.insert(cleanup.into(), catalogue.cleanup_summary(cleanup).unwrap().clone());
    let error = elaborate_ownership(module, &natives, &OwnOptions::default()).unwrap_err();
    assert!(format!("{error:?}").contains("registered conservative"), "{error:?}");
}

// #569 O1 audit: identical protocol declarations are one protocol. The catalogue
// registers the first; a call naming a later identical one is still canonical.
#[test]
fn identical_open_protocol_declarations_are_one_protocol() {
    let mut module = rewrite(include_str!("protocol-high/closure-open.oir")).module;
    let first = crate::catalogue::open_callable_protocol_id(&module).unwrap();
    let second = module.protocols.alloc(crate::catalogue::open_callable_protocol()).unwrap();
    assert_eq!(crate::catalogue::open_callable_protocol_id(&module), Some(first));
    let mut renamed = 0;
    for (_, summary) in module.summaries.iter_mut() {
        if summary.contract.protocol == Some(first) {
            summary.contract.protocol = Some(second);
            renamed += 1;
        }
    }
    assert!(renamed > 0);
    verify_output(&module);
    // A different protocol under the same contract stays rejected.
    let mut other = crate::catalogue::open_callable_protocol();
    other.roots.clear();
    *module.protocols.get_mut(second).unwrap() = other;
    reject(&module, RuleId::B55);
}

#[test]
fn the_cleanup_intrinsic_is_not_trusted_by_its_name() {
    let mut module = rewrite(include_str!("protocol-high/closure-open.oir")).module;
    let catalogue = RuntimeCatalogue::for_module(&module);
    assert!(catalogue.cleanup_summary(crate::catalogue::SYNC_CLOSURE_CLEANUP).is_some());
    let (_, cleanup) = module.functions.iter().find(|(_, function)| function.symbol == "drop_fn").unwrap();
    let summary = cleanup.summary.unwrap();
    module.summaries.get_mut(summary).unwrap().contract.failures = vec![FailureKind::Thrown];
    reject(&module, RuleId::B51);
    let mut module = rewrite(include_str!("protocol-high/closure-open.oir")).module;
    let (id, _) = module.functions.iter().find(|(_, function)| function.symbol == "drop_fn").unwrap();
    module.functions.get_mut(id).unwrap().signature.params.swap(1, 2);
    assert!(RuntimeCatalogue::for_module(&module).cleanup_summary(crate::catalogue::SYNC_CLOSURE_CLEANUP).is_none());
    reject(&module, RuleId::T5);
}

#[test]
fn open_contract_shape_for_plain_and_resource_signatures() {
    let source = include_str!("protocol-high/closure-open.oir");
    let mut module = crate::text::parse(source).unwrap();
    let protocol = crate::catalogue::open_callable_protocol_id(&module).unwrap();
    let resource = module.types.iter().find_map(|(id, ty)| matches!(ty, Type::Object { .. }).then_some(id)).unwrap();
    let int = module.types.intern(Type::Int).unwrap();
    let plain = module.types.intern(Type::Fn(FnType { params: vec![int], result: Some(int), is_async: false }))
        .unwrap();
    let summary = crate::catalogue::open_callable_summary(&module, plain, protocol).unwrap();
    assert_eq!(summary.parameters[0].convention, FreshConvention::Borrow);
    assert_eq!(summary.parameters[1].convention, FreshConvention::Borrow);
    assert!(!summary.results[0].opaque && summary.results[0].aliases.is_empty());
    let mixed = module.types.intern(Type::Fn(FnType {
        params: vec![int, resource], result: Some(resource), is_async: false,
    })).unwrap();
    let summary = crate::catalogue::open_callable_summary(&module, mixed, protocol).unwrap();
    assert_eq!(summary.parameters[0].convention, FreshConvention::Borrow);
    assert_eq!(summary.parameters[2].convention, FreshConvention::Take);
    assert_eq!(summary.results[0].aliases, vec![SummarySelection::OwnerGraph(0), SummarySelection::OwnerGraph(2)]);
    assert_eq!(summary.results[0].external_deps, vec![0, 2]);
    assert!(summary.results[0].opaque);
    assert!(!summary.contract.suspends && !summary.contract.async_destroy);
    assert!(!summary.contract.failures.contains(&FailureKind::Cancelled));
    let asynchronous = module.types.intern(Type::Fn(FnType { params: vec![], result: None, is_async: true }))
        .unwrap();
    assert!(crate::catalogue::open_callable_summary(&module, asynchronous, protocol).is_none());
}

// #569 O1 audit: a result that may alias a bound environment can leave the binding's
// scope only together with that environment's graph. Own's fall-through selects the
// result's whole graph; a selection of only its fresh part leaves the environment to
// the scope's end, and the result outlives what it may alias.
#[test]
fn open_result_outliving_a_bound_environment_scope_is_rejected() {
    let extra = concat!(
        "scope SL action in @lambda\n",
        "func @lambda(%captured: R, %x: R) -> R [lambda captures 1] {\n",
        "    boundary BL lexical in SL\n",
        "    ^entry in SL unwind ^failed:\n",
        "    [ip] exit.prepare return %captured at BL\n",
        "    result.exit edge normal(0), value %captured, demand want, prepare ip\n",
        "    ret %captured\n",
        "    ^failed(%failure: failure) in SL:\n",
        "    raise %failure\n",
        "}\n",
        "scope SM action in @mk\n",
        "func @mk() -> fn(R) -> R {\n",
        "    boundary BM lexical in SM\n",
        "    ^entry in SM unwind ^failed:\n",
        "    %r: R = call @make()\n",
        "    %c: fn(R) -> R = fn.closure @lambda, [%r], ty fn(R) -> R\n",
        "    [ip] exit.prepare return %c at BM\n",
        "    result.exit edge normal(0), value %c, demand want, prepare ip\n",
        "    ret %c\n",
        "    ^failed(%failure: failure) in SM:\n",
        "    raise %failure\n",
        "}\n",
        "scope SG action in @g\n",
        "scope SGI block in @g parent SG\n",
        "func @g() -> R {\n",
        "    boundary BG lexical in SG\n",
        "    boundary BGI lexical in SGI\n",
        "    binding bc \"c\": fn(R) -> R in SGI\n",
        "    ^entry in SG unwind ^failed:\n",
        "    %dict: type = type.of ty fn(R) -> R, descriptor descriptor_contract(managed=runtime,\n",
        "        retention=owner_graph(0), cleanup=@drop_fn, async_destroy=false, summary=Md)\n",
        "    br ^inner\n",
        "    ^inner in SGI unwind ^failed:\n",
        "    %c: fn(R) -> R = call @mk()\n",
        "    bind %c, binding bc\n",
        "    %r: R = call @make()\n",
        "    %result: R = fn.call %c(consume %r) summary(Mopen)\n",
        "    [ipass] exit.prepare fallthrough %result at BGI\n",
        "    result.exit edge normal(0), value %result, demand want, prepare ipass\n",
        "    br ^after(%result)\n",
        "    ^after(%out: R) in SG unwind ^failed:\n",
        "    [ip] exit.prepare return %out at BG\n",
        "    result.exit edge normal(0), value %out, demand want, prepare ip\n",
        "    ret %out\n",
        "    ^failed(%failure: failure) in SG:\n",
        "    raise %failure\n",
        "}\n",
    );
    let source = format!("{}{extra}", include_str!("protocol-high/closure-open.oir"));
    let mut module = rewrite(&source).module;
    verify_output(&module);
    let (_, function) = module.functions.iter_mut().find(|(_, function)| function.symbol == "g").unwrap();
    let body = function.body.as_mut().unwrap();
    let root = body.blocks.get(body.entry).unwrap().lexical_scope;
    let inner: Vec<_> = body.blocks.iter().filter(|(_, block)| block.lexical_scope != root).filter_map(|(id, block)| {
        let TermKind::InvokeOwn { op, .. } = &block.term.kind else { return None };
        let Operation::Ownership(OwnershipOp::ScopeSplit { selection, .. }) = op.as_ref() else { return None };
        matches!(body.selections.get(*selection), Ok(SelectionExpr::Value(_))).then_some(id)
    }).collect();
    let [fallthrough] = inner[..] else { panic!("{inner:?}") };
    let TermKind::InvokeOwn { op, .. } = &body.blocks.get(fallthrough).unwrap().term.kind else { panic!() };
    let Operation::Ownership(OwnershipOp::ScopeSplit { selection, .. }) = op.as_ref() else { panic!() };
    let fresh = body.selections.alloc(SelectionExpr::Admissible(*selection)).unwrap();
    let term = &mut body.blocks.get_mut(fallthrough).unwrap().term.kind;
    let TermKind::InvokeOwn { op, .. } = term else { panic!() };
    let Operation::Ownership(OwnershipOp::ScopeSplit { selection, .. }) = op.as_mut() else { panic!() };
    *selection = fresh;
    let trusted = runtime_contract(&module, Profile::Own);
    let report = verify_with(&module, &VerifyOptions::default(), context(&trusted)).unwrap_err();
    assert!(report.diagnostics.iter().all(|diagnostic| diagnostic.rule == RuleId::B31
        && diagnostic.message.contains("stable dependency has been closed")), "{report:?}");
    reject(&module, RuleId::B31);
}

// #569 O1: the environment is borrowed, so a fresh temporary callable ends at the
// call boundary. A result that may alias its captured resource cannot outlive it.
#[test]
fn open_result_aliasing_a_temporary_environment_is_rejected() {
    let extra = concat!(
        "scope SL action in @lambda\n",
        "func @lambda(%captured: R, %x: R) -> R [lambda captures 1] {\n",
        "    boundary BL lexical in SL\n",
        "    ^entry in SL unwind ^failed:\n",
        "    [ip] exit.prepare return %captured at BL\n",
        "    result.exit edge normal(0), value %captured, demand want, prepare ip\n",
        "    ret %captured\n",
        "    ^failed(%failure: failure) in SL:\n",
        "    raise %failure\n",
        "}\n",
        "scope SM action in @mk\n",
        "func @mk() -> fn(R) -> R {\n",
        "    boundary BM lexical in SM\n",
        "    ^entry in SM unwind ^failed:\n",
        "    %r: R = call @make()\n",
        "    %c: fn(R) -> R = fn.closure @lambda, [%r], ty fn(R) -> R\n",
        "    [ip] exit.prepare return %c at BM\n",
        "    result.exit edge normal(0), value %c, demand want, prepare ip\n",
        "    ret %c\n",
        "    ^failed(%failure: failure) in SM:\n",
        "    raise %failure\n",
        "}\n",
        "scope SG action in @g\n",
        "func @g() -> R {\n",
        "    boundary BG lexical in SG\n",
        "    ^entry in SG unwind ^failed:\n",
        "    %dict: type = type.of ty fn(R) -> R, descriptor descriptor_contract(managed=runtime,\n",
        "        retention=owner_graph(0), cleanup=@drop_fn, async_destroy=false, summary=Md)\n",
        "    %c: fn(R) -> R = call @mk()\n",
        "    %r: R = call @make()\n",
        "    %result: R = fn.call %c(consume %r) summary(Mopen)\n",
        "    [ip] exit.prepare return %result at BG\n",
        "    result.exit edge normal(0), value %result, demand want, prepare ip\n",
        "    ret %result\n",
        "    ^failed(%failure: failure) in SG:\n",
        "    raise %failure\n",
        "}\n",
    );
    let source = format!("{}{extra}", include_str!("protocol-high/closure-open.oir"));
    let error = try_rewrite(&source).unwrap_err();
    let OwnError::InvalidOutput(report) = error else { panic!("{error:?}") };
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == RuleId::B51
        && diagnostic.message.contains("settled by callee cleanup")), "{report:?}");
}

// #569 C3b: the producer attaches the open ABI exactly to own's open callees.
fn open_callees_of(source: &str, function: &str) -> Vec<String> {
    let module = crate::text::parse(source).unwrap();
    let (_, data) = module.functions.iter().find(|(_, data)| data.symbol == function).unwrap();
    let body = data.body.as_ref().unwrap();
    let open = crate::own::open_callees(&module, body, &OwnOptions::default()).unwrap();
    open.into_iter().map(|value| match body.values.get(value).unwrap().def {
        Definition::BlockParam { block, index } if block == body.entry => format!("entry {index}"),
        Definition::BlockParam { index, .. } => format!("block parameter {index}"),
        Definition::InstResult { inst, .. } => body.insts.get(inst).unwrap().op.opcode().to_owned(),
    }).collect()
}

#[test]
fn open_callees_are_entry_parameters_not_known_closures() {
    assert_eq!(open_callees_of(include_str!("protocol-high/closure-open.oir"), "f"), ["entry 0"]);
    assert!(open_callees_of(include_str!("protocol-high/closure-known.oir"), "f").is_empty());
    // A closure merged through a block parameter keeps its finite origin.
    assert!(open_callees_of(include_str!("protocol-high/closure-known-phi.oir"), "f").is_empty());
}

/// Decision (a) of #676, P2.4 §4.1: an action value whose capture reaches a resource is a new object that holds a
/// reference to it. The capture's obligation stays with its owner, so own hands the construction an empty packet
/// and admits the packet it returns; an action value without such a capture stays packetless.
fn action_value_packets(source: &str) -> Vec<Option<bool>> {
    let module = rewrite(source).module;
    let mut packets = vec![];
    for (_, data) in module.functions.iter() {
        let Some(body) = &data.body else { continue };
        for (_, instruction) in body.insts.iter() {
            let packet = match &instruction.op {
                Operation::Core(CoreOp::FnClosure { packet, .. } | CoreOp::FnBind { packet, .. }
                    | CoreOp::FnBindRole { packet, .. }) => *packet,
                _ => continue,
            };
            let Some(packet) = packet else {
                packets.push(None);
                continue;
            };
            let crate::Definition::InstResult { inst, .. } = body.values.get(packet).unwrap().def else {
                panic!("the packet of an action value is not an instruction result");
            };
            let empty = matches!(body.insts.get(inst).unwrap().op, Operation::Ownership(OwnershipOp::Empty {}));
            let returned = instruction.results[1];
            let admitted = body.insts.iter().any(|(_, user)| {
                matches!(user.op, Operation::Ownership(OwnershipOp::Admit { packet, .. }) if packet == returned)
            });
            packets.push(Some(empty && admitted));
        }
    }
    packets
}

#[test]
fn action_values_capturing_a_resource_take_an_empty_packet() {
    for name in ["closure-known", "closure-known-phi", "closure-known-loop", "bound-snapshot", "bound-role"] {
        let source = std::fs::read_to_string(fixtures("protocol-high").iter()
            .find(|path| path.file_stem().unwrap() == name).unwrap()).unwrap();
        assert_eq!(action_value_packets(&source), vec![Some(true)], "{name}");
    }
    let join = include_str!("protocol-high/closure-known-join.oir");
    let mut packets = action_value_packets(join);
    packets.sort();
    assert_eq!(packets, vec![None, Some(true)], "the capture-free closure and the bound receiver");
}

#[test]
fn action_values_without_a_resource_capture_stay_packetless() {
    let empty = include_str!("protocol-high/closure-empty.oir");
    assert_eq!(action_value_packets(empty), vec![None]);
    let plain = empty.replace("func @lambda() -> int [lambda] {", "func @lambda(%k: int) -> int [lambda captures 1] {")
        .replace("    %closure: fn() -> int = fn.closure @lambda, ty fn() -> int",
            "    %k: int = const 3\n    %closure: fn() -> int = fn.closure @lambda, [%k], ty fn() -> int");
    assert_ne!(plain, empty);
    assert_eq!(action_value_packets(&plain), vec![None], "a plain capture");
}
