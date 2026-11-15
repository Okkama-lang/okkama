use super::*;
use std::path::{Path, PathBuf};

pub(super) fn fixtures(directory: &str) -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/own/tests").join(directory);
    let mut paths: Vec<_> = std::fs::read_dir(root).unwrap().map(|entry| entry.unwrap().path()).collect();
    paths.sort();
    paths
}

fn rewrite_source(source: &str) -> Result<OwnOutput, String> {
    let module = crate::text::parse(source).map_err(|error| error.to_string())?;
    let output = elaborate_ownership(module, &TestNatives::default(), &OwnOptions::default())
        .map_err(|error| format!("inference: {error}"))?;
    rewrite_subset(output, &OwnOptions::default()).map_err(|error| format!("rewrite: {error}"))
}

#[test]
fn successor_retention_triage() {
    // Spec §4.1/§6.2 and P1.3 §8 permit the inherited field plus new self.
    // Keep the B51 limitation reduced for the separate linearity state lane.
    for (successor, source, golden) in [
        (true, include_str!("triage-high/successor-retention.oir"),
            include_str!("triage-goldens/successor-retention.oir")),
        (false, include_str!("triage-high/field-retention.oir"),
            include_str!("triage-goldens/field-retention.oir")),
    ] {
        crate::verify_high(&crate::text::parse(source).unwrap()).unwrap();
        let output = rewrite_source(source).unwrap();
        let expected = crate::text::parse(golden).unwrap();
        assert!(crate::text::normalized_eq(&output.module, &expected).unwrap());
        for module in [&output.module, &roundtrip(&output.module), &expected] {
            if successor {
                let report = verify_own(module).unwrap_err();
                assert_eq!(report.failure, VerifyFailure::Rejected);
                assert_eq!(report.diagnostics.len(), 1);
                assert_eq!(report.diagnostics[0].rule, RuleId::B51);
                assert_eq!(report.diagnostics[0].message,
                    "body exports accepted inputs beyond its declared take retention");
            } else {
                verify_own(module).unwrap();
            }
        }
    }
}

#[test]
fn managed_call_effects_triage() {
    // P1.3 §3.4 requires effects from inserted partitions, independently of
    // the language result type. A managed parameter alone introduces none.
    let source = include_str!("triage-high/managed-call-effects.oir");
    crate::verify_high(&crate::text::parse(source).unwrap()).unwrap();
    let output = rewrite_source(source).unwrap();
    let expected = crate::text::parse(include_str!("triage-goldens/managed-call-effects.oir")).unwrap();
    assert!(crate::text::normalized_eq(&output.module, &expected).unwrap());
    for module in [&output.module, &roundtrip(&output.module), &expected] {
        verify_own(module).unwrap();
    }
    let read = output.module.functions.iter().find(|(_, function)| function.symbol == "read").unwrap().1;
    let summary = output.module.summaries.get(read.summary.unwrap()).unwrap();
    assert!(summary.contract.failures.is_empty());
    let mut understated = output.module;
    let main = understated.functions.iter().find(|(_, function)| function.symbol == "main").unwrap().1;
    let summary = understated.summaries.get_mut(main.summary.unwrap()).unwrap();
    assert_eq!(summary.contract.failures, [FailureKind::Program]);
    summary.contract.failures.clear();
    let report = verify_own(&understated).unwrap_err();
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == RuleId::B51
        && diagnostic.message == "body exposes a failure kind absent from its declared summary"));
}

pub(super) fn roundtrip(module: &Module) -> Module {
    let printed = crate::text::print(module).unwrap();
    let parsed = crate::text::parse(&printed).unwrap();
    assert_eq!(crate::text::print(&parsed).unwrap(), printed);
    parsed
}

#[test]
fn synchronous_lowering_inputs() {
    // Unchanged #566/#567 copies. Their missing D11 records are deliberately
    // accepted by the compatibility input path, rather than fabricated here.
    let paths = fixtures("lowering");
    assert_eq!(paths.len(), 41);
    let mut errors = vec![];
    for path in paths {
        let source = std::fs::read_to_string(&path).unwrap();
        let result = (|| {
            let output = rewrite_source(&source)?;
            verify_own(&output.module).map_err(|error| format!("verify: {error:?}"))?;
            if matches!(path.file_stem().unwrap().to_str(), Some("for" | "for-value")) {
                for (_, function) in output.module.functions.iter() {
                    let Some(body) = &function.body else { continue };
                    assert!(body.insts.iter().all(|(_, inst)| {
                        !matches!(inst.op, Operation::Ownership(_) | Operation::Exit(_))
                    }));
                    assert!(body.blocks.iter().all(|(_, block)| match &block.term.kind {
                        TermKind::InvokeOwn { op, .. } => {
                            !matches!(op.as_ref(), Operation::Ownership(_) | Operation::Exit(_))
                        }
                        _ => true,
                    }));
                }
            }
            let parsed = roundtrip(&output.module);
            verify_own(&parsed).map_err(|error| format!("roundtrip verify: {error:?}"))?;
            let high = crate::text::parse(&source).unwrap();
            let reparsed = crate::text::print(&high).unwrap();
            let repeated = rewrite_source(&reparsed)?;
            assert_eq!(crate::text::print(&repeated.module).unwrap(), crate::text::print(&output.module).unwrap());
            Ok::<_, String>(())
        })();
        if let Err(error) = result {
            errors.push(format!("{}: {error}", path.file_name().unwrap().to_string_lossy()));
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

// Reviewed expectations; changes require an explanation of the semantic difference.
const WITNESSES: &[&str] = &[
    "b1-inner-cleanup-catch",
    "b2-bound-replacement",
    "construction-completion",
    "construction-failure",
    "construction-stage-overwrite",
    "late-argument-before-call",
    "same-alias-replacement",
    "resource-panic",
    "resource-trap",
    "spec16-1-bound-call",
    "spec16-2-temporary-call",
    "spec16-3-serial-order",
    "spec16-4-defer-prefix",
    "spec16-5-projection-escape",
    "spec16-6-rebind-history",
    "two-scope-ar",
    "two-scope-ra",
];

#[test]
fn reviewed_synchronous_goldens_verify() {
    let inputs = fixtures("sync-high");
    assert_eq!(inputs.len(), WITNESSES.len());
    assert_eq!(fixtures("sync-goldens").len(), WITNESSES.len());
    let expected = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/own/tests/sync-goldens");
    let mut errors = vec![];
    for name in WITNESSES {
        let source = inputs.iter().find(|path| path.file_stem().unwrap() == *name).unwrap();
        let output = rewrite_source(&std::fs::read_to_string(source).unwrap())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let printed = crate::text::print(&output.module).unwrap();
        let golden = std::fs::read_to_string(expected.join(format!("{name}.oir"))).unwrap();
        assert_eq!(printed, golden, "review the {name} difference; never auto-bless it");
        for module in [&output.module, &roundtrip(&output.module)] {
            if let Err(error) = verify_own(module) {
                errors.push(format!("{name}: {error:?}"));
                break;
            }
        }
        protocol_invariants(name, &output.module);
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

fn protocol_invariants(name: &str, module: &Module) {
    let body = &module.functions.iter().find(|(_, function)| function.symbol == "f")
        .or_else(|| module.functions.iter().find(|(_, function)| function.symbol == "main"))
        .unwrap().1.body.as_ref().unwrap();
    let operations: Vec<_> = body.insts.iter().map(|(_, instruction)| &instruction.op)
        .chain(body.blocks.iter().filter_map(|(_, block)| match &block.term.kind {
            TermKind::InvokeOwn { op, .. } => Some(op.as_ref()),
            _ => None,
        })).collect();
    for operation in &operations {
        if let Operation::Ownership(OwnershipOp::ScopeEnd { effects, .. }
            | OwnershipOp::ScopeUnwind { effects, .. }) = operation {
            assert!(!effects.suspends && !effects.cancel);
        }
    }
    if name == "b1-inner-cleanup-catch" {
        assert!(!operations.iter().any(|op| matches!(op, Operation::Exit(_))));
        for (scope, declaration) in module.scopes.iter() {
            if declaration.kind == ScopeKind::Try {
                assert_eq!(operations.iter().filter(|op| {
                    matches!(op, Operation::Ownership(OwnershipOp::ScopeBegin { scope: opened }) if *opened == scope)
                }).count(), 1);
            }
        }
    }
    if name == "b2-bound-replacement" || name == "same-alias-replacement" {
        assert!(operations.iter().any(|op| matches!(op, Operation::Exit(ExitOp::Replace { .. }))));
        assert!(operations.iter().any(|op| matches!(op, Operation::Ownership(OwnershipOp::Accept { .. }))));
    }
    for (_, block) in body.blocks.iter() {
        if let TermKind::InvokeOwn { op, unwind, .. } = &block.term.kind
            && matches!(op.as_ref(), Operation::Ownership(OwnershipOp::ScopeEnd { .. })) {
            assert!(unwind.args.iter().all(|arg| {
                !matches!(arg, EdgeArg::Produced(EdgeResult { slot: 2.., .. }))
            }));
        }
    }
}

#[test]
fn hand_written_same_alias_and_overlapping_reserves() {
    for path in fixtures("sync-own") {
        let module = crate::text::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        verify_own(&module).unwrap_or_else(|error| panic!("{}: {error:?}", path.display()));
        verify_own(&roundtrip(&module)).unwrap();
    }
}

#[test]
fn early_cleanup_reopened_scope_and_exceptional_pending_are_rejected() {
    let early = include_str!("../../../tests/fixtures/verify/invalid/B4-early-end.oir");
    let pending = include_str!("../../../tests/fixtures/verify/invalid/D3-obsolete-unwind-pending.oir");
    let source = include_str!("sync-own/same-alias-reserve.oir");
    let reopened = source.replace("    ret %final", concat!(
        "    %reopened: scope<S> = scope.begin S\n",
        "    %closed_again: closed = scope.end %reopened, ",
        "effects scope_effects(failures=[], suspends=false, cancel=false)\n",
        "    ret %final",
    ));
    for (source, rule) in [(early, RuleId::B42), (pending, RuleId::T2), (&reopened, RuleId::B42)] {
        let module = crate::text::parse(source).unwrap();
        let first = verify_own(&module).unwrap_err();
        assert!(first.diagnostics.iter().any(|diagnostic| diagnostic.rule == rule));
        let parsed = roundtrip(&module);
        let second = verify_own(&parsed).unwrap_err();
        let locations = |report: &VerifyReport| report.diagnostics.iter()
            .filter(|diagnostic| diagnostic.rule == rule)
            .map(|diagnostic| diagnostic.primary.entity.clone()).collect::<Vec<_>>();
        assert_eq!(locations(&first), locations(&second));
    }
}

#[test]
fn checked_entry_point_certifies_projection_escape() {
    let source = include_str!("sync-high/spec16-5-projection-escape.oir");
    let module = crate::text::parse(source).unwrap();
    let output = elaborate_ownership(module, &TestNatives::default(), &OwnOptions::default()).unwrap();
    rewrite_synchronous(output, &OwnOptions::default()).unwrap();
}

pub(super) struct TrustedNative {
    pub(super) name: &'static str,
    pub(super) signature: Signature,
    pub(super) summary: Summary,
}
impl ExternalContracts for TrustedNative {
    fn native(&self, name: &str) -> Option<&Signature> {
        (name == self.name).then_some(&self.signature)
    }
    fn intrinsic(&self, name: &str) -> Option<&Signature> {
        (name == self.name).then_some(&self.signature)
    }
    fn native_summary(&self, name: &str) -> Option<&Summary> {
        (name == self.name).then_some(&self.summary)
    }
    fn intrinsic_summary(&self, name: &str) -> Option<&Summary> {
        (name == self.name).then_some(&self.summary)
    }
    fn descriptor_cleanup(&self, name: &str) -> Option<&Summary> {
        (name == "oir.sync_closure.cleanup" && name == self.name).then_some(&self.summary)
    }
}

#[test]
fn observation_borrowed_result_and_explicit_empty_native_transport() {
    for observation in [true, false] {
        let mut module = ModuleBuilder::new("native_call", Profile::High);
        let int = module.intern_type(Type::Int).unwrap();
        let own = module.intern_type(Type::Service(ServiceType::Own)).unwrap();
        let deps = module.intern_type(Type::Service(ServiceType::Deps)).unwrap();
        let (resource, destroy) = resource(&mut module, "R");
        empty_body(&mut module, destroy);
        let parameters = if observation { vec![resource] } else { vec![] };
        let result = if observation { resource } else { int };
        let name = if observation { "host.observe" } else { "host.scalar" };
        let mut external = function("native", parameters.clone(), vec![result]);
        if observation {
            external.attrs.push(FunctionAttr::Intrinsic { name: name.into(), objects: vec![], helpers: vec![] });
        } else {
            external.attrs.push(FunctionAttr::Extern(name.into()));
        }
        let summary = Summary {
            entry_kind: if observation { EntryKind::ObservationAdmissionStaysCaller } else { EntryKind::CallBoundary },
            parameters: if observation { vec![ParameterEffect {
                convention: FreshConvention::Borrow, retention: None, deps: vec![],
                destination: TransferDestination::None,
            }] } else { vec![] },
            results: vec![ResultRelation {
                fresh: vec![], aliases: if observation { vec![SummarySelection::Parameter(0)] } else { vec![] },
                external_deps: if observation { vec![0] } else { vec![] },
                provenance: if observation { AdmissionProvenance::Bound } else { AdmissionProvenance::UnboundResult },
            }],
            contract: CallContract {
                results: vec![result], failures: vec![], suspends: false, async_destroy: false,
                publication: Publication::Atomic, protocol: None,
            },
        };
        let native = module.declare_function(external).unwrap();
        let results = if observation { vec![] } else { vec![int] };
        let caller = module.declare_function(function("caller", parameters, results)).unwrap();
        let root = scope(&mut module, caller);
        let mut builder = module.function_builder(caller, root).unwrap();
        with_unwind(&mut builder, root);
        let mut call = call_site();
        if observation {
            call.args.push(CallArgument { value: builder.block_params(builder.entry()).unwrap()[0],
                convention: OperandConvention::Borrow });
        }
        let operation = if observation {
            CallOp::Direct { function: native, call }
        } else {
            CallOp::Native { function: native, call }
        };
        let value = builder.emit_schema(Operation::Call(operation)).unwrap()[0];
        builder.terminate(TermKind::Ret { values: if observation { vec![] } else { vec![value] } }).unwrap();
        let body = builder.finish().unwrap();
        module.define_function(caller, body).unwrap();
        let mut natives = TestNatives::default();
        natives.named.insert(name.into(), summary.clone());
        let output = elaborate_ownership(module.finish().unwrap(), &natives, &OwnOptions::default()).unwrap();
        let output = rewrite_subset(output, &OwnOptions::default()).unwrap();
        let signature = if observation {
            Signature { params: vec![resource, deps], results: vec![resource],
                ownership_abi: OwnershipAbi::Observation }
        } else {
            Signature { params: vec![own], results: vec![int, own],
                ownership_abi: OwnershipAbi::Explicit { input_packet: 0, output_packet: 1 } }
        };
        let trusted = TrustedNative { name, signature, summary };
        let context = VerifyContext { text_locations: None, external_contracts: &trusted };
        verify_with(&output.module, &VerifyOptions::default(), context).unwrap();
        let body = output.module.functions.get(caller).unwrap().body.as_ref().unwrap();
        let call = body.insts.iter().find_map(|(_, instruction)| match &instruction.op {
            Operation::Call(CallOp::Native { call, .. } | CallOp::Direct { call, .. }) => Some(call),
            _ => None,
        }).unwrap();
        assert_eq!(call.packet.is_none(), observation);
        if observation {
            assert!(body.insts.iter().any(|(_, instruction)| {
                matches!(instruction.op, Operation::Ownership(OwnershipOp::Dependencies { .. }))
            }));
        }
    }
}

#[test]
fn hand_inputs_respect_high_contract() {
    for path in fixtures("sync-high") {
        let module = crate::text::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        verify_high(&module).unwrap_or_else(|report| panic!("{}: {report:?}", path.display()));
    }
}

#[test]
fn binding_in_a_nested_block_accepts_outer_admission_at_the_declared_owner() {
    let common = include_str!("sync-high/spec16-1-bound-call.oir");
    let prefix = common.split("func @f()").next().unwrap();
    let source = format!("{prefix}{}", r#"
scope SI block in @f parent S
func @f() -> int {
    binding b0 "r": R in S
    boundary B0 lexical in S
    boundary BI lexical in SI
    ^entry in S unwind ^failed:
    %r: R = call @make()
    br ^inner(%r)
    ^inner(%value: R) in SI unwind ^failed:
    bind %value, binding b0
    want %value
    [ipass] exit.prepare fallthrough %value at BI
    result.exit edge normal(0), value %value, demand want, prepare ipass
    br ^body(%value)
    ^body(%alive: R) in S unwind ^failed:
    %first: int = call @read(%alive)
    %second: int = call @read(%alive)
    want %second
    [ireturn] exit.prepare return %second at B0
    result.exit edge normal(0), value %second, demand want, prepare ireturn
    ret %second
    ^failed(%failure: failure) in S:
    raise %failure
}
"#);
    let high = crate::text::parse(&source).unwrap();
    verify_high(&high).unwrap();
    let output = rewrite_source(&source).unwrap();
    verify_own(&output.module).unwrap();
    verify_own(&roundtrip(&output.module)).unwrap();
    let nested = output.module.scopes.iter().find(|(_, scope)| scope.kind == ScopeKind::Block).unwrap().0;
    let caller = output.module.functions.iter().find(|(_, function)| function.symbol == "f").unwrap().1;
    assert!(!caller.body.as_ref().unwrap().insts.iter().any(|(_, instruction)| {
        matches!(instruction.op, Operation::Ownership(OwnershipOp::ScopeBegin { scope }) if scope == nested)
    }));
}

#[test]
fn independent_verifier_exhaustion_is_incomplete_not_a_rejected_or_certified_module() {
    let source = include_str!("lowering/scalar.oir");
    let module = crate::text::parse(source).unwrap();
    let output = elaborate_ownership(module, &TestNatives::default(), &OwnOptions::default()).unwrap();
    let mut options = OwnOptions::default();
    options.verify.limits.steps = 0;
    assert!(matches!(rewrite_synchronous(output, &options), Err(OwnError::IncompleteVerification { .. })));
}

#[test]
fn simultaneous_edge_values_share_one_reserve_without_losing_either_obligation() {
    let common = include_str!("sync-high/spec16-1-bound-call.oir");
    let prefix = common.split("func @f()").next().unwrap();
    let source = format!("{prefix}{}", r#"
scope SI block in @f parent S
func @f() -> int {
    boundary B0 lexical in S
    boundary BI lexical in SI
    ^entry in S unwind ^failed:
    br ^inner
    ^inner in SI unwind ^failed:
    %a: R = call @make()
    %b: R = call @make()
    want %a
    [ipass_a] exit.prepare fallthrough %a at BI
    result.exit edge normal(0), value %a, demand want, prepare ipass_a
    want %b
    [ipass_b] exit.prepare fallthrough %b at BI
    result.exit edge normal(0), value %b, demand want, prepare ipass_b
    br ^body(%a, %b)
    ^body(%left: R, %right: R) in S unwind ^failed:
    %first: int = call @read(%left)
    %second: int = call @read(%right)
    want %second
    [ireturn] exit.prepare return %second at B0
    result.exit edge normal(0), value %second, demand want, prepare ireturn
    ret %second
    ^failed(%failure: failure) in S:
    raise %failure
}
"#);
    verify_high(&crate::text::parse(&source).unwrap()).unwrap();
    let output = rewrite_source(&source).unwrap();
    verify_own(&output.module).unwrap();
    verify_own(&roundtrip(&output.module)).unwrap();
    let caller = output.module.functions.iter().find(|(_, function)| function.symbol == "f").unwrap().1;
    assert!(caller.body.as_ref().unwrap().insts.iter().any(|(_, instruction)| {
        matches!(instruction.op, Operation::Exit(ExitOp::Prepare { ref exit }) if exit.value.is_none())
    }));
}

#[test]
fn defer_return_can_abandon_a_block_fallthrough_without_merging_destinations() {
    let source = include_str!("sync-high/b2-bound-replacement.oir");
    let prefix = source.split("scope SI block in @f parent S").next().unwrap();
    let source = format!("{prefix}{}", r#"
scope SI block in @f parent S
func @f() -> R {
    binding bb "b0": R in S
    boundary B0 lexical in S
    boundary BI lexical in SI
    ^entry in S unwind ^failed:
    %b0: R = call @make()
    bind %b0, binding bb
    br ^inner
    ^inner in SI unwind ^failed:
    %r: R = call @make()
    defer.register SI, action defer_action(function=@replace, bindings=[bb],
        replacements=[defer_registration(label="return", kind=return, boundary=B0)])
    want %r
    [ipass] exit.prepare fallthrough %r at BI
    result.exit edge normal(0), value %r, demand want, prepare ipass
    br ^after(%r)
    ^after(%original: R) in S unwind ^failed:
    want %original
    [iafter] exit.prepare return %original at B0
    result.exit edge normal(0), value %original, demand want, prepare iafter
    ret %original
    ^failed(%failure: failure) in S:
    raise %failure
}
"#);
    let output = rewrite_source(&source).unwrap();
    verify_own(&output.module).unwrap();
    verify_own(&roundtrip(&output.module)).unwrap();
    let caller = output.module.functions.iter().find(|(_, function)| function.symbol == "f").unwrap().1;
    let returns = caller.body.as_ref().unwrap().blocks.iter().filter(|(_, block)| {
        matches!(block.term.kind, TermKind::Ret { .. })
    }).count();
    assert_eq!(returns, 2);
}

#[test]
fn enclosing_rebind_does_not_authorize_a_borrow_to_escape_its_local_owner() {
    let common = include_str!("sync-high/spec16-1-bound-call.oir");
    let prefix = common.split("func @f()").next().unwrap();
    let source = format!("{prefix}{}", r#"
scope SI block in @f parent S
func @f() -> int {
    binding b0 "r": R in S
    binding b1 "t": R in SI
    boundary B0 lexical in S
    ^entry in S unwind ^failed:
    %original: R = call @make()
    bind %original, binding b0
    br ^inner
    ^inner in SI unwind ^failed:
    %local: R = call @make()
    bind %local, binding b1
    rebind %local, binding b0, target S
    discard %local
    result.exit edge normal(0), value %local, demand discard, prepare none
    br ^after(%local)
    ^after(%borrowed: R) in S unwind ^failed:
    %result: int = call @read(%borrowed)
    want %result
    [ireturn] exit.prepare return %result at B0
    result.exit edge normal(0), value %result, demand want, prepare ireturn
    ret %result
    ^failed(%failure: failure) in S:
    raise %failure
}

"#);
    let high = crate::text::parse(&source).unwrap();
    verify_high(&high).unwrap();
    let output = elaborate_ownership(high, &TestNatives::default(), &OwnOptions::default()).unwrap();
    let Err(OwnError::InvalidOutput(report)) = rewrite_synchronous(output, &OwnOptions::default()) else {
        panic!("an enclosing binding must not extend the borrowed owner's lifetime");
    };
    assert!(report.diagnostics.iter().any(|diagnostic| {
        matches!(diagnostic.rule, RuleId::B31 | RuleId::B32)
    }), "{report:?}");
    assert!(report.diagnostics.iter().flat_map(|diagnostic| &diagnostic.related)
        .any(|label| label.message.contains("binding \"r\"")), "{report:?}");
}

#[test]
fn preaccept_construction_failure_restores_an_outer_admission_before_inner_unwind() {
    let common = include_str!("sync-high/spec16-1-bound-call.oir");
    let prefix = common.split("func @f()").next().unwrap();
    let source = format!("{prefix}{}", r#"
object Holder {
    field child: R
}
scope SI block in @f parent S
func @f() -> Holder [ctor Holder] {
    boundary B0 lexical in S
    ^entry in S unwind ^failed:
    %r: R = call @make()
    temp %r, S
    br ^inner(%r)
    ^inner(%outer: R) in SI unwind ^failed:
    %h: Holder = obj.new Holder { child: %outer }
    want %h
    [ireturn] exit.prepare return %h at B0
    result.exit edge normal(0), value %h, demand want, prepare ireturn
    ret %h
    ^failed(%failure: failure) in S:
    raise %failure
}
"#);
    verify_high(&crate::text::parse(&source).unwrap()).unwrap();
    let output = rewrite_source(&source).unwrap();
    verify_own(&output.module).unwrap_or_else(|error| {
        panic!("{error:?}\n{}", crate::text::print(&output.module).unwrap());
    });
    verify_own(&roundtrip(&output.module)).unwrap();
    let (function, caller) = output.module.functions.iter().find(|(_, function)| function.symbol == "f").unwrap();
    let root = output.module.scopes.iter().find(|(_, scope)| {
        scope.function == function && scope.kind == ScopeKind::Action
    }).unwrap().0;
    let body = caller.body.as_ref().unwrap();
    let unwind = body.blocks.iter().find_map(|(_, block)| match &block.term.kind {
        TermKind::InvokeOwn { op, unwind, .. } if matches!(op.as_ref(), Operation::Core(CoreOp::ObjNew { .. })) => {
            Some(unwind.block)
        }
        _ => None,
    }).unwrap();
    let restored = body.blocks.get(unwind).unwrap().insts.iter().find_map(|inst| {
        match body.insts.get(*inst).unwrap().op {
            Operation::Ownership(OwnershipOp::Admit { state, .. }) => Some(state),
            _ => None,
        }
    }).unwrap();
    let ty = body.values.get(restored).unwrap().ty;
    assert_eq!(output.module.types.get(ty).unwrap(), &Type::Service(ServiceType::Scope(root)));
}

#[test]
fn managed_loop_exits_preserve_the_source_kind_and_loop_boundary() {
    let common = include_str!("sync-high/spec16-1-bound-call.oir");
    let prefix = common.split("func @f()").next().unwrap();
    for (name, expected) in [("last", ExitKind::Last), ("next", ExitKind::Next)] {
        let source = format!("{prefix}{}", r#"
scope SI iteration in @f parent S
func @f() -> int {
    boundary B0 lexical in S
    boundary BL loop in SI
    ^entry in S unwind ^failed:
    br ^iteration
    ^iteration in SI unwind ^failed:
    %r: R = call @make()
    want %r
    [ipass] exit.prepare LOOP_KIND %r at BL
    result.exit edge normal(0), value %r, demand want, prepare ipass
    br ^after(%r)
    ^after(%value: R) in S unwind ^failed:
    %n: int = call @read(%value)
    want %n
    [ireturn] exit.prepare return %n at B0
    result.exit edge normal(0), value %n, demand want, prepare ireturn
    ret %n
    ^failed(%failure: failure) in S:
    raise %failure
}
"#).replace("LOOP_KIND", name);
        verify_high(&crate::text::parse(&source).unwrap()).unwrap();
        let output = rewrite_source(&source).unwrap();
        verify_own(&output.module).unwrap();
        verify_own(&roundtrip(&output.module)).unwrap();
        let caller = output.module.functions.iter().find(|(_, function)| function.symbol == "f").unwrap().1;
        let body = caller.body.as_ref().unwrap();
        let boundary = body.insts.iter().find_map(|(_, instruction)| match &instruction.op {
            Operation::Exit(ExitOp::Prepare { exit }) if exit.kind == expected => Some(exit.boundary),
            _ => None,
        }).unwrap();
        assert_eq!(body.boundaries.get(boundary).unwrap().kind, BoundaryKind::Loop);
        assert!(body.insts.iter().any(|(_, instruction)| {
            matches!(instruction.op, Operation::Exit(ExitOp::Commit {
                destination: ExitDestination::Loop(committed), ..
            }) if committed == boundary)
        }));
    }
}

#[test]
fn two_defer_helpers_close_their_own_temporaries_before_returning_to_the_caller() {
    let common = include_str!("sync-high/spec16-1-bound-call.oir");
    let prefix = common.split("func @f()").next().unwrap();
    let mut source = prefix.to_owned();
    for (scope, name) in [("SA", "first"), ("SB", "second")] {
        source.push_str(&format!(r#"
scope {scope} action in @{name}
func @{name}() -> void {{
    ^entry in {scope} unwind ^failed:
    %r: R = call @make()
    temp %r, {scope}
    discard %r
    result.exit edge normal(0), value %r, demand discard, prepare none
    ret
    ^failed(%failure: failure) in {scope}:
    raise %failure
}}
"#));
    }
    source.push_str(r#"
func @f() -> void {
    ^entry in S unwind ^failed:
    defer.register S, action defer_action(function=@first)
    defer.register S, action defer_action(function=@second)
    ret
    ^failed(%failure: failure) in S:
    raise %failure
}
"#);
    verify_high(&crate::text::parse(&source).unwrap()).unwrap();
    let output = rewrite_source(&source).unwrap();
    verify_own(&output.module).unwrap();
    verify_own(&roundtrip(&output.module)).unwrap();
    for name in ["first", "second"] {
        let (function, helper) = output.module.functions.iter().find(|(_, function)| function.symbol == name).unwrap();
        let body = helper.body.as_ref().unwrap();
        let scope = output.module.scopes.iter().find(|(_, scope)| scope.function == function).unwrap().0;
        assert!(body.insts.iter().any(|(_, instruction)| match instruction.op {
            Operation::Ownership(OwnershipOp::ScopeEnd { state, .. }) => {
                let ty = body.values.get(state).unwrap().ty;
                output.module.types.get(ty) == Ok(&Type::Service(ServiceType::Scope(scope)))
            }
            _ => false,
        }));
        assert!(body.blocks.iter().any(|(_, block)| matches!(block.term.kind,
            TermKind::RetDefer { tag: DeferReturnTag::Continue, .. })));
    }
}

#[test]
fn a_live_join_never_silently_promotes_an_unreached_defer() {
    let source = r#"oir 2 high
module conditional_defer
scope S action in @f
scope SD action in @cleanup
func @cleanup() -> void {
    ^entry in SD:
    ret
}
func @f(%condition: bool) -> void {
    ^entry in S:
    cond %condition, ^register, ^skip
    ^register in S:
    defer.register S, action defer_action(function=@cleanup)
    br ^join
    ^skip in S:
    br ^join
    ^join in S:
    ret
}
"#;
    let report = verify_high(&crate::text::parse(source).unwrap()).unwrap_err();
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == RuleId::H3));
    let error = rewrite_source(source).unwrap_err();
    assert!(error.contains("defer prefixes differ at live join"), "{error}");
}
