use okkama_oir::{text, verify::*};
use okkama_oir::visit::Rewrite;
use std::{fs, path::Path};
mod support;
use support::FixtureContracts;

fn location(entity: &EntityLocation) -> String {
    match entity {
        EntityLocation::Scope(id) => format!("scope:{}", id.as_u32()),
        EntityLocation::Function(id) => format!("function:{}", id.as_u32()),
        EntityLocation::Instruction { function, instruction } => {
            format!("instruction:{}:{}", function.as_u32(), instruction.as_u32())
        }
        EntityLocation::Terminator { function, block } => {
            format!("terminator:{}:{}", function.as_u32(), block.as_u32())
        }
        EntityLocation::Block { function, block } => format!("block:{}:{}", function.as_u32(), block.as_u32()),
        EntityLocation::Value { function, value } => format!("value:{}:{}", function.as_u32(), value.as_u32()),
        EntityLocation::Operand { site: OperationSite::Instruction { function, instruction }, path } => {
            format!("operand:{}:{}:{:?}", function.as_u32(), instruction.as_u32(), path.0)
        }
        EntityLocation::Operand { site: OperationSite::Terminator { function, block }, path } => {
            format!("operand-term:{}:{}:{:?}", function.as_u32(), block.as_u32(), path.0)
        }
        EntityLocation::Edge { function, edge } => format!("edge:{}:{}", function.as_u32(), edge.0),
        other => format!("{other:?}"),
    }
}
fn fixture_result(module: &okkama_oir::Module, errors: ErrorPolicy) -> Result<(), VerifyReport> {
    let options = VerifyOptions {
        execution: ExecutionMode::Tests,
        errors,
        ..VerifyOptions::default()
    };
    let context = VerifyContext {
        external_contracts: &FixtureContracts(module),
        ..VerifyContext::default()
    };
    verify_with(module, &options, context).map(|_| ())
}

/// Async ABI V1 fixtures are checked by the structural verifier only (D20–D25);
/// their ownership rules arrive with later slices.
fn structural_result(module: &okkama_oir::Module, errors: ErrorPolicy) -> Result<(), VerifyReport> {
    let options = VerifyOptions {
        mode: VerifyMode::Structural,
        execution: ExecutionMode::Tests,
        errors,
        ..VerifyOptions::default()
    };
    let context = VerifyContext {
        external_contracts: &FixtureContracts(module),
        ..VerifyContext::default()
    };
    verify_with(module, &options, context).map(|_| ())
}

fn fixture_module(root: &Path, path: &str) -> okkama_oir::Module {
    let source = fs::read_to_string(root.join(path)).unwrap();
    text::parse(&source).unwrap_or_else(|error| panic!("{path}: {error}"))
}

#[test]
fn canonical_verifier_fixtures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify");
    let mut failures = Vec::new();
    for class in ["valid", "invalid"] {
        let mut paths: Vec<_> = fs::read_dir(root.join(class)).unwrap()
            .map(|entry| entry.unwrap().path()).collect();
        paths.sort();
        for path in paths {
            let source = fs::read_to_string(&path).unwrap();
            let module = text::parse(&source).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            let result = fixture_result(&module, ErrorPolicy::All);
            assert_eq!(fixture_result(&module, ErrorPolicy::All), result, "{}", path.display());
            let canonical = text::print(&module).unwrap();
            let roundtrip = text::parse(&canonical).unwrap();
            let repeated = fixture_result(&roundtrip, ErrorPolicy::All);
            if class == "valid" {
                if let Err(error) = result {
                    failures.push(format!("{}: {error:?}", path.display()));
                }
                assert!(repeated.is_ok(), "{}: {repeated:?}", path.display());
                continue;
            }
            let expected = source.lines().find_map(|line| line.strip_prefix("// expect ")).unwrap();
            let (rule, primary) = expected.split_once(' ').unwrap();
            let Err(report) = result else {
                failures.push(format!("{}: accepted invalid fixture", path.display()));
                continue;
            };
            assert_eq!(report.failure, VerifyFailure::Rejected, "{}: {report:?}", path.display());
            if !report.diagnostics.iter().any(|diagnostic| diagnostic.rule.as_str() == rule
                && location(&diagnostic.primary.entity) == primary) {
                failures.push(format!("{}: {expected}; {report:?}", path.display()));
            }
            let repeated = repeated.unwrap_err();
            assert_eq!(repeated.failure, VerifyFailure::Rejected);
            assert!(repeated.diagnostics.iter().any(|diagnostic| diagnostic.rule.as_str() == rule),
                "canonical roundtrip lost {rule}: {}", path.display());
            let first = fixture_result(&module, ErrorPolicy::First).unwrap_err();
            assert_eq!(first.diagnostics, report.diagnostics[..1], "{}", path.display());
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn async_v1_fixtures_roundtrip_and_verify_structurally() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/async");
    let mut failures = Vec::new();
    for class in ["valid", "invalid"] {
        let mut paths: Vec<_> = fs::read_dir(root.join(class)).unwrap()
            .map(|entry| entry.unwrap().path()).collect();
        paths.sort();
        assert!(!paths.is_empty(), "{class}");
        for path in paths {
            let source = fs::read_to_string(&path).unwrap();
            let module = text::parse(&source).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            let canonical = text::print(&module).unwrap();
            let roundtrip = text::parse(&canonical).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            assert_eq!(text::print(&roundtrip).unwrap(), canonical, "{}: unstable canonical text", path.display());
            assert_eq!(roundtrip, module, "{}: canonical text changes the module", path.display());
            let result = structural_result(&module, ErrorPolicy::All);
            if class == "valid" {
                if let Err(error) = result {
                    failures.push(format!("{}: {error:?}", path.display()));
                }
                continue;
            }
            let expected = source.lines().find_map(|line| line.strip_prefix("// expect ")).unwrap();
            let (rule, primary) = expected.split_once(' ').unwrap();
            let Err(report) = result else {
                failures.push(format!("{}: accepted invalid fixture", path.display()));
                continue;
            };
            if !report.diagnostics.iter().any(|diagnostic| diagnostic.rule.as_str() == rule
                && location(&diagnostic.primary.entity) == primary) {
                failures.push(format!("{}: {expected}; {report:?}", path.display()));
            }
            let first = structural_result(&module, ErrorPolicy::First).unwrap_err();
            assert_eq!(first.diagnostics, report.diagnostics[..1], "{}", path.display());
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Slice V3 fixtures: the own inputs of the coro pass (V2's own goldens and the V2-shaped variants), their coro
/// goldens and the B9.5 negatives, all under the full verifier (OIR-2 §12: own → verify(own) → coro →
/// verify(coro)).
#[test]
fn async_v3_coro_fixtures_verify_fully() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("tests/fixtures/async");
    let mut failures = Vec::new();
    for class in ["own", "coro", "coro-invalid"] {
        let mut paths: Vec<_> = fs::read_dir(root.join(class)).unwrap()
            .map(|entry| entry.unwrap().path()).collect();
        if class == "own" {
            paths.extend(fs::read_dir(manifest.join("src/own/tests/async-goldens")).unwrap()
                .map(|entry| entry.unwrap().path()));
        }
        paths.sort();
        assert!(!paths.is_empty(), "{class}");
        for path in paths {
            let source = fs::read_to_string(&path).unwrap();
            let module = text::parse(&source).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            let canonical = text::print(&module).unwrap();
            let roundtrip = text::parse(&canonical).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            assert_eq!(text::print(&roundtrip).unwrap(), canonical, "{}: unstable canonical text", path.display());
            let result = fixture_result(&module, ErrorPolicy::All);
            if class != "coro-invalid" {
                if let Err(error) = result {
                    failures.push(format!("{}: {error:?}", path.display()));
                }
                continue;
            }
            let expected = source.lines().find_map(|line| line.strip_prefix("// expect ")).unwrap();
            let (rule, primary) = expected.split_once(' ').unwrap();
            let Err(report) = result else {
                failures.push(format!("{}: accepted invalid fixture", path.display()));
                continue;
            };
            assert_eq!(report.failure, VerifyFailure::Rejected, "{}: {report:?}", path.display());
            if !report.diagnostics.iter().any(|diagnostic| diagnostic.rule.as_str() == rule
                && location(&diagnostic.primary.entity) == primary) {
                failures.push(format!("{}: {expected}; {report:?}", path.display()));
            }
            let repeated = fixture_result(&roundtrip, ErrorPolicy::All).unwrap_err();
            assert!(repeated.diagnostics.iter().any(|diagnostic| diagnostic.rule.as_str() == rule),
                "canonical roundtrip lost {rule}: {}", path.display());
            let first = fixture_result(&module, ErrorPolicy::First).unwrap_err();
            assert_eq!(first.diagnostics, report.diagnostics[..1], "{}", path.display());
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Slice V4 fixtures: the hand-written Coro input, the Low goldens of every coro golden and the B10.6/D18
/// negatives, all under the full verifier (OIR-2 §12: coro → verify(coro) → low → verify(low)).
#[test]
fn async_v4_low_fixtures_verify_fully() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/async");
    let mut failures = Vec::new();
    for class in ["low-input", "low", "low-invalid"] {
        let mut paths: Vec<_> = fs::read_dir(root.join(class)).unwrap()
            .map(|entry| entry.unwrap().path()).collect();
        paths.sort();
        assert!(!paths.is_empty(), "{class}");
        for path in paths {
            let source = fs::read_to_string(&path).unwrap();
            let module = text::parse(&source).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            let canonical = text::print(&module).unwrap();
            let roundtrip = text::parse(&canonical).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            assert_eq!(text::print(&roundtrip).unwrap(), canonical, "{}: unstable canonical text", path.display());
            let result = fixture_result(&module, ErrorPolicy::All);
            if class != "low-invalid" {
                if let Err(error) = result {
                    failures.push(format!("{}: {error:?}", path.display()));
                }
                continue;
            }
            let expected = source.lines().find_map(|line| line.strip_prefix("// expect ")).unwrap();
            let (rule, primary) = expected.split_once(' ').unwrap();
            let Err(report) = result else {
                failures.push(format!("{}: accepted invalid fixture", path.display()));
                continue;
            };
            assert_eq!(report.failure, VerifyFailure::Rejected, "{}: {report:?}", path.display());
            if !report.diagnostics.iter().any(|diagnostic| diagnostic.rule.as_str() == rule
                && location(&diagnostic.primary.entity) == primary) {
                failures.push(format!("{}: {expected}; {report:?}", path.display()));
            }
            let repeated = fixture_result(&roundtrip, ErrorPolicy::All).unwrap_err();
            assert!(repeated.diagnostics.iter().any(|diagnostic| diagnostic.rule.as_str() == rule),
                "canonical roundtrip lost {rule}: {}", path.display());
            let first = fixture_result(&module, ErrorPolicy::First).unwrap_err();
            assert_eq!(first.diagnostics, report.diagnostics[..1], "{}", path.display());
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// #695: the mutated cases of the async wave 1 audit. Each row names the rule and location that reject the
/// negative and the golden or fixture it mutates, which must verify; every negative is listed.
#[test]
fn async_audit_695_negatives_and_their_positives() {
    use std::collections::BTreeSet;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/async");
    let mut listed = BTreeSet::new();
    for line in include_str!("fixtures/async/audit-matrix.tsv").lines().skip(1) {
        let row: Vec<_> = line.split('\t').collect();
        assert_eq!(row.len(), 4, "{line}");
        let positive = fixture_module(&root, row[2]);
        fixture_result(&positive, ErrorPolicy::All).unwrap_or_else(|report| panic!("{line}: {report:?}"));
        let source = fs::read_to_string(root.join(row[3])).unwrap();
        let negative = text::parse(&source).unwrap_or_else(|error| panic!("{line}: {error}"));
        let report = fixture_result(&negative, ErrorPolicy::All).unwrap_err();
        assert_eq!(report.failure, VerifyFailure::Rejected, "{line}");
        assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule.as_str() == row[0]
            && location(&diagnostic.primary.entity) == row[1]), "{line}: {report:?}");
        let roundtrip = text::parse(&text::print(&negative).unwrap()).unwrap();
        let repeated = fixture_result(&roundtrip, ErrorPolicy::All).unwrap_err();
        assert!(repeated.diagnostics.iter().any(|diagnostic| diagnostic.rule.as_str() == row[0]),
            "canonical roundtrip lost {line}");
        let expected = source.lines().find_map(|line| line.strip_prefix("// expect ")).unwrap();
        listed.insert((row[3].to_string(), expected == format!("{} {}", row[0], row[1])));
    }
    for entry in fs::read_dir(root.join("audit-invalid")).unwrap() {
        let name = format!("audit-invalid/{}", entry.unwrap().file_name().to_string_lossy());
        assert!(listed.contains(&(name.clone(), true)), "{name}: its expect line needs a row of audit-matrix.tsv");
    }
}

/// R2: the traces of r2-defer-await-reserve, whose pending exit of r lives across an async defer. A R: the final
/// close delivers a requested cancellation only after the plan that destroys a has drained. R A: the close after
/// the failed defer begins with the failure and the resumed pending exit, so r is in its plan from the start.
#[test]
fn r2_traces_across_a_suspension_keep_their_order() {
    use okkama_oir::*;
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/async/low/r2-defer-await-reserve.oir");
    let source = fs::read_to_string(path).unwrap();
    let module = text::parse(&source).unwrap();
    fixture_result(&module, ErrorPolicy::All).unwrap();
    let (function, data) = module.functions.iter().find(|(_, data)| data.symbol == "f").unwrap();
    let body = data.body.as_ref().unwrap();
    // R A: the failing close takes the reserve, resumed from the frame rather than the exit.prepare result.
    let failing = body.insts.iter().find_map(|(_, inst)| match &inst.op {
        Operation::Cleanup(CleanupOp::Begin { pending: Some(pending), failure: Some(_), .. }) => Some(*pending),
        _ => None,
    }).expect("the close after the failed defer begins with the failure and the reserve");
    let prepared = |value: ValueId| matches!(body.values.get(value).unwrap().def, Definition::InstResult { inst, .. }
        if matches!(body.insts.get(inst).unwrap().op, Operation::Exit(ExitOp::Prepare { .. })));
    assert!(!prepared(failing), "the reserve crosses the async defer through the frame");
    // A R: delivering before the final close's first plan would abort r before a is destroyed.
    let text = &source[source.find("func @f(").unwrap()..];
    let begin = text.lines().position(|line| line.contains("= cleanup.begin ") && line.contains(", pending ")
        && !line.contains(", failure ")).unwrap();
    let plan = text.lines().skip(begin).find(|line| line.contains("= cleanup.plan ")).unwrap();
    let state = plan.split("cleanup.plan ").nth(1).unwrap().split(',').next().unwrap();
    let early = format!("    %early: cleanup-state = cleanup.deliver_cancel {state}\n{}",
        plan.replace(&format!("cleanup.plan {state},"), "cleanup.plan %early,"));
    let mutated = text::parse(&source.replace(plan, &early)).unwrap();
    let report = fixture_result(&mutated, ErrorPolicy::All).unwrap_err();
    let body = mutated.functions.get(function).unwrap().body.as_ref().unwrap();
    let (instruction, _) = body.insts.iter().find(|(_, inst)| matches!(inst.op,
        Operation::Cleanup(CleanupOp::DeliverCancel { .. }))).unwrap();
    let expected = EntityLocation::Instruction { function, instruction };
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == RuleId::B105
        && diagnostic.message.contains("after the plan has drained") && diagnostic.primary.entity == expected),
        "{report:?}");
}

/// #696: the traces of the D21 M8 fixtures. The abort-after-prefix trace A R holds because a cancellation is
/// delivered only after the plan that destroys A has drained; the explicit aborts (A R and R A) need a definite
/// failure, because a cancellation in D21 is decided only at the selector after its delivery.
#[test]
fn m8_d21_traces_keep_their_order() {
    use okkama_oir::*;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify");
    let rejected_at = |source: &str, rule: RuleId, message: &str, op: &dyn Fn(&Operation) -> bool| {
        let module = text::parse(source).unwrap();
        let report = fixture_result(&module, ErrorPolicy::All).unwrap_err();
        let (function, data) = module.functions.iter().find(|(_, data)| data.symbol == "f").unwrap();
        let body = data.body.as_ref().unwrap();
        let (instruction, _) = body.insts.iter().find(|(_, inst)| op(&inst.op)).unwrap();
        let expected = EntityLocation::Instruction { function, instruction };
        assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == rule
            && diagnostic.message.contains(message) && diagnostic.primary.entity == expected), "{report:?}");
    };

    // A R: delivering the cancellation before the first plan would abort R before A is destroyed.
    let source = fs::read_to_string(root.join("valid/M8-abort-after-executed-prefix.oir")).unwrap();
    fixture_result(&text::parse(&source).unwrap(), ErrorPolicy::All).unwrap();
    let body = &source[source.find("func @f(").unwrap()..];
    let plan = body.lines().find(|line| line.contains("= cleanup.plan ") && line.ends_with(", plan 0")).unwrap();
    let state = plan.split("cleanup.plan ").nth(1).unwrap().split(',').next().unwrap();
    let early = format!("    %early: cleanup-state = cleanup.deliver_cancel {state}\n{}",
        plan.replace(&format!("cleanup.plan {state},"), "cleanup.plan %early,"));
    let mutated = source.replace(plan, &early);
    let first = |op: &Operation| matches!(op, Operation::Cleanup(CleanupOp::DeliverCancel { .. }));
    rejected_at(&mutated, RuleId::B105, "cancellation is delivered once, after the plan has drained", &first);

    // A R and R A by explicit abort in the close of the failed deferred call: A R aborts R with the state that
    // the drained first plan hands to its `done` edge, R A aborts it with the settled state before any plan.
    for (name, after_prefix) in [("M8-explicit-abort-after-executed-prefix", true),
        ("M8-explicit-abort-before-destroy", false)] {
        let module = text::parse(&fs::read_to_string(root.join(format!("valid/{name}.oir"))).unwrap()).unwrap();
        fixture_result(&module, ErrorPolicy::All).unwrap();
        let (_, data) = module.functions.iter().find(|(_, data)| data.symbol == "f").unwrap();
        let body = data.body.as_ref().unwrap();
        let aborts: Vec<_> = body.insts.iter().filter_map(|(_, inst)| match inst.op {
            Operation::Exit(ExitOp::Abort { cleanup, .. }) => Some(cleanup),
            _ => None,
        }).collect();
        let [state] = aborts.as_slice() else { panic!("{name}: one explicit abort") };
        match body.values.get(*state).unwrap().def {
            Definition::BlockParam { block, .. } => {
                assert!(after_prefix, "{name}");
                assert!(body.blocks.iter().any(|(_, data)| matches!(&data.term.kind,
                    TermKind::CleanupNext { done, .. } if done.block == block)), "{name}");
            }
            Definition::InstResult { inst, .. } => {
                assert!(!after_prefix, "{name}");
                assert!(matches!(body.insts.get(inst).unwrap().op,
                    Operation::Cleanup(CleanupOp::SettleEmpty { .. })), "{name}");
            }
        }
    }
}

#[test]
fn every_rule_has_a_verified_canonical_positive_and_negative() {
    use std::collections::BTreeSet;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify");
    let mut covered = BTreeSet::new();
    for line in include_str!("fixtures/verify/matrix.tsv").lines().skip(1) {
        let row: Vec<_> = line.split('\t').collect();
        assert_eq!(row.len(), 3, "{line}");
        assert!(covered.insert(row[0]), "duplicate RuleId: {line}");
        let positive = fixture_module(&root, row[1]);
        fixture_result(&positive, ErrorPolicy::All).unwrap_or_else(|report| panic!("{line}: {report:?}"));
        let negative = fixture_module(&root, row[2]);
        let report = fixture_result(&negative, ErrorPolicy::All).unwrap_err();
        assert_eq!(report.failure, VerifyFailure::Rejected, "{line}");
        assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule.as_str() == row[0]),
            "{line}: {report:?}");
    }
    let async_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/async");
    for line in include_str!("fixtures/async/matrix.tsv").lines().skip(1) {
        let row: Vec<_> = line.split('\t').collect();
        assert_eq!(row.len(), 3, "{line}");
        assert!(covered.insert(row[0]), "duplicate RuleId: {line}");
        let positive = fixture_module(&async_root, row[1]);
        structural_result(&positive, ErrorPolicy::All).unwrap_or_else(|report| panic!("{line}: {report:?}"));
        let negative = fixture_module(&async_root, row[2]);
        let report = structural_result(&negative, ErrorPolicy::All).unwrap_err();
        assert_eq!(report.failure, VerifyFailure::Rejected, "{line}");
        assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule.as_str() == row[0]),
            "{line}: {report:?}");
    }
    // B9.5 rules of slice V3 are linearity rules: full verification.
    for line in include_str!("fixtures/async/coro-matrix.tsv").lines().skip(1) {
        let row: Vec<_> = line.split('\t').collect();
        assert_eq!(row.len(), 3, "{line}");
        assert!(covered.insert(row[0]), "duplicate RuleId: {line}");
        let positive = fixture_module(&async_root, row[1]);
        fixture_result(&positive, ErrorPolicy::All).unwrap_or_else(|report| panic!("{line}: {report:?}"));
        let negative = fixture_module(&async_root, row[2]);
        let report = fixture_result(&negative, ErrorPolicy::All).unwrap_err();
        assert_eq!(report.failure, VerifyFailure::Rejected, "{line}");
        assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule.as_str() == row[0]),
            "{line}: {report:?}");
    }
    // B10.6 and D18 rules of slice V4 on lowered async bodies: full verification.
    for line in include_str!("fixtures/async/low-matrix.tsv").lines().skip(1) {
        let row: Vec<_> = line.split('\t').collect();
        assert_eq!(row.len(), 3, "{line}");
        assert!(covered.insert(row[0]), "duplicate RuleId: {line}");
        let positive = fixture_module(&async_root, row[1]);
        fixture_result(&positive, ErrorPolicy::All).unwrap_or_else(|report| panic!("{line}: {report:?}"));
        let negative = fixture_module(&async_root, row[2]);
        let report = fixture_result(&negative, ErrorPolicy::All).unwrap_err();
        assert_eq!(report.failure, VerifyFailure::Rejected, "{line}");
        assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule.as_str() == row[0]),
            "{line}: {report:?}");
    }
    let required: BTreeSet<_> = RuleId::ALL.iter().map(|rule| rule.as_str()).collect();
    assert_eq!(covered, required, "every RuleId needs both canonical witnesses");
}

#[test]
fn nontextual_outcome_and_slot_corruption() {
    use okkama_oir::*;
    let source = include_str!("fixtures/verify/valid/checked-split-normal-and-unwind.oir");
    for corruption in 0..3 {
        let mut module = text::parse(source).unwrap();
        let function = FuncId::from_u32(0);
        let bool_ty = module.types.intern(Type::Bool).unwrap();
        let body = module.functions.get_mut(function).unwrap().body.as_mut().unwrap();
        if corruption == 2 {
            let param = body.blocks.get(BlockId::from_u32(1)).unwrap().params[0];
            body.values.get_mut(param).unwrap().ty = bool_ty;
        } else {
            let term = &mut body.blocks.get_mut(body.entry).unwrap().term.kind;
            let TermKind::InvokeOwn { normal, .. } = term else { panic!("expected invoke"); };
            let EdgeArg::Produced(result) = &mut normal.args[0] else { panic!("expected produced slot"); };
            if corruption == 0 { result.outcome = Outcome::Unwind; }
            else { result.slot = u16::MAX; }
        }
        let report = verify(&module).unwrap_err();
        assert!(report.diagnostics.iter().any(|d| d.rule == RuleId::B62 && d.primary.entity == EntityLocation::Edge {
            function, edge: VerifyEdgeId(0),
        }), "{report:?}");
    }
}

#[test]
fn verify_function_limits_analysis_to_the_selected_body() {
    use okkama_oir::*;
    let source = include_str!("fixtures/verify/valid/H3-dominating-defer-prefix.oir");
    let mut module = text::parse(source).unwrap();
    let broken = FuncId::from_u32(1);
    let body = module.functions.get_mut(broken).unwrap().body.as_mut().unwrap();
    let inst = body.blocks.get(body.entry).unwrap().insts[0];
    body.insts.get_mut(inst).unwrap().op = Operation::Ownership(OwnershipOp::Empty {});
    assert!(verify(&module).is_err());
    let report = verify_function(&module, FuncId::from_u32(0), VerifyContext::default(), &VerifyOptions::default());
    assert_eq!(report.failure, None, "{report:?}");
}

#[test]
fn graph_and_high_analysis_budgets_never_produce_a_certificate() {
    // #634 step 1 -> 1.5 charges, in fixture order (memory bytes; work):
    // 102244 -> 99627; 1507 -> 4336. 43224 -> 41483; 477 -> 1179.
    // 162748 -> 161025; 3551 -> 7182. Sparse bookkeeping is real work too.
    // Derive each threshold from its own charge: enough completes,
    // one byte/step below must remain Incomplete.
    for source in [
        include_str!("fixtures/verify/valid/SSA2-irreducible-CFG.oir"),
        include_str!("fixtures/verify/valid/H3-dominating-defer-prefix.oir"),
        include_str!("fixtures/verify/valid/A2-completion-after-merge.oir"),
    ] {
        let module = text::parse(source).unwrap();
        let stats = verify(&module).unwrap().stats().clone();
        for limits in [
            VerifyLimits { steps: stats.steps - 1, ..VerifyLimits::default() },
            VerifyLimits { memory_bytes: stats.memory_bytes - 1, ..VerifyLimits::default() },
        ] {
            let options = VerifyOptions { limits, ..VerifyOptions::default() };
            let report = verify_with(&module, &options, VerifyContext::default()).unwrap_err();
            assert_eq!(report.failure, VerifyFailure::Incomplete, "{report:?}");
        }
    }
}

#[test]
fn production_execution_rejects_test_authority() {
    use okkama_oir::*;
    let module = text::parse(include_str!("fixtures/verify/valid/A3-test-authority.oir")).unwrap();
    let report = verify(&module).unwrap_err();
    assert!(report.diagnostics.iter().any(|d| d.rule == RuleId::A3 && d.primary.entity == EntityLocation::Instruction {
        function: FuncId::from_u32(0), instruction: InstId::from_u32(0),
    }), "{report:?}");
}

#[test]
fn high_intents_and_shared_await_do_not_create_ownership_results() {
    use okkama_oir::*;
    for (source, expected_high, expected_own) in [
        (include_str!("fixtures/verify/valid/H4-prepared-return.oir"), 0, 1),
        (include_str!("fixtures/verify/valid/A3-async-test.oir"), 1, 2),
    ] {
        let mut module = text::parse(source).unwrap();
        let op = module.functions.get(FuncId::from_u32(0)).unwrap().body.as_ref().unwrap()
            .insts.get(InstId::from_u32(0)).unwrap().op.clone();
        let high = ContractRegistry::new(&module).lookup(&op).unwrap();
        assert_eq!(high.normal.results.len(), expected_high);
        if let Some(unwind) = high.unwind { assert_eq!(unwind.results.len(), 1); }
        module.profile = Profile::Own;
        let own = ContractRegistry::new(&module).lookup(&op).unwrap();
        assert_eq!(own.normal.results.len(), expected_own);
    }
}

#[test]
fn specification_manifest_verifier_corpus() {
    use std::collections::BTreeSet;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let manifest = fs::read_to_string(root.join("spec/manifest.tsv")).unwrap();
    let mut positives = BTreeSet::new();
    let mut negatives = BTreeSet::new();
    let mut reviews = BTreeSet::new();
    let mut examples = BTreeSet::new();
    let mut pairs = BTreeSet::new();
    for line in manifest.lines().skip(1) {
        let row: Vec<_> = line.split('\t').collect();
        assert_eq!(row.len(), 12, "{line}");
        if row[0].starts_with("example-") || row[0].starts_with("review-A.1-") {
            examples.insert(row[0]);
        }
        if matches!(row[8], "historical" | "parse-reject") {
            assert!(row[9..].iter().all(|field| *field == "-"), "{line}");
            continue;
        }
        assert!(matches!(row[8], "static-ir" | "protocol-contract" | "runtime-behavior" | "frontend-reject"),
            "missing verification level: {line}");
        assert_ne!(row[11], "-", "missing verifier RuleIds: {line}");
        // A codec illustration and its behavior sidecar are not verifier certificates.
        // The explicit columns identify complete canonical translations for each example.
        if positives.insert(row[9]) {
            let module = fixture_module(&root, row[9]);
            fixture_result(&module, ErrorPolicy::All).unwrap_or_else(|report| panic!("{line}: {report:?}"));
        }
        if negatives.insert((row[10], row[11])) {
            let module = fixture_module(&root, row[10]);
            let report = fixture_result(&module, ErrorPolicy::All).unwrap_err();
            assert_eq!(report.failure, VerifyFailure::Rejected, "{line}");
            for rule in row[11].split(',') {
                assert!(RuleId::ALL.iter().any(|known| known.as_str() == rule), "{line}");
                assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule.as_str() == rule),
                    "{line}: {report:?}");
            }
        }
        if let Some(number) = row[0].strip_prefix("review-A.1-") {
            reviews.insert(number.parse::<u8>().unwrap());
        }
        if row[0].starts_with('M') {
            pairs.insert((row[0].split('-').next().unwrap(), row[2]));
        }
    }
    let required = include_str!("fixtures/spec/examples.txt").lines().collect();
    assert_eq!(examples, required, "every spec example must remain wired to canonical witnesses");
    assert_eq!(reviews, (1..=24).collect(), "preserve every numbered A.1 example");
    for number in 1..=9 {
        for class in ["accept", "semantic-reject"] {
            assert!(pairs.contains(&(format!("M{number}").as_str(), class)), "M{number} lacks {class}");
        }
    }
}


#[test]
fn ownership_verdicts_survive_codec_and_arena_permutations() {
    use okkama_oir::*;
    use std::collections::BTreeSet;
    fn verdict(module: &Module) -> (Option<VerifyFailure>, BTreeSet<RuleId>) {
        let options = VerifyOptions { execution: ExecutionMode::Tests, ..VerifyOptions::default() };
        let context = VerifyContext { external_contracts: &FixtureContracts(module), ..VerifyContext::default() };
        match verify_with(module, &options, context) {
            Ok(_) => (None, BTreeSet::new()),
            Err(report) => (Some(report.failure), report.diagnostics.into_iter().map(|d| d.rule).collect()),
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify");
    for class in ["valid", "invalid"] {
        let mut paths: Vec<_> = fs::read_dir(root.join(class)).unwrap().map(|e| e.unwrap().path()).collect();
        paths.sort();
        for path in paths {
            let original = text::parse(&fs::read_to_string(&path).unwrap())
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            let expected = verdict(&original);
            let printed = text::print(&original).unwrap();
            let mut renamed = original.clone();
            for (_, function) in renamed.functions.iter_mut() {
                function.symbol = format!("renamed_{}", function.symbol);
            }
            let renamed = text::print(&renamed).unwrap().replace('%', "%renamed_");
            let renamed = text::parse(&renamed).unwrap();
            assert_eq!(verdict(&renamed), expected, "SSA names: {}", path.display());
            let roundtrip = text::parse(&printed).unwrap();
            assert_eq!(verdict(&roundtrip), expected, "codec: {}", path.display());
            let mut permuted = original.clone();
            for (_, function) in permuted.functions.iter_mut() {
                let Some(body) = &mut function.body else { continue; };
                let mut remap = IdRemap::default();
                let mut blocks = Arena::new();
                for (old, block) in body.blocks.iter().collect::<Vec<_>>().into_iter().rev() {
                    remap.blocks.insert(old, blocks.alloc(block.clone()).unwrap());
                }
                body.blocks = blocks;
                let mut values = Arena::new();
                for (old, value) in body.values.iter().collect::<Vec<_>>().into_iter().rev() {
                    remap.values.insert(old, values.alloc(value.clone()).unwrap());
                }
                let mut instructions = Arena::new();
                for (old, inst) in body.insts.iter().collect::<Vec<_>>().into_iter().rev() {
                    remap.instructions.insert(old, instructions.alloc(inst.clone()).unwrap());
                }
                let mut closes = Arena::new();
                for (old, item) in body.cleanup.closes.iter().collect::<Vec<_>>().into_iter().rev() {
                    remap.cleanup_closes.insert(old, closes.alloc(item.clone()).unwrap());
                }
                body.cleanup.closes = closes;
                let mut schedules = Arena::new();
                for (old, item) in body.cleanup.schedules.iter().collect::<Vec<_>>().into_iter().rev() {
                    remap.cleanup_schedules.insert(old, schedules.alloc(item.clone()).unwrap());
                }
                body.cleanup.schedules = schedules;
                let mut plans = Arena::new();
                for (old, item) in body.cleanup.plans.iter().collect::<Vec<_>>().into_iter().rev() {
                    remap.cleanup_plans.insert(old, plans.alloc(item.clone()).unwrap());
                }
                body.cleanup.plans = plans;
                let mut proofs = Arena::new();
                for (old, item) in body.cleanup.proofs.iter().collect::<Vec<_>>().into_iter().rev() {
                    remap.cleanup_proofs.insert(old, proofs.alloc(item.clone()).unwrap());
                }
                body.cleanup.proofs = proofs;
                let mut roots = Arena::new();
                for (old, item) in body.cleanup.roots.iter().collect::<Vec<_>>().into_iter().rev() {
                    remap.root_maps.insert(old, roots.alloc(item.clone()).unwrap());
                }
                body.cleanup.roots = roots;
                body.values = values;
                body.insts = instructions;
                body.rewrite(&mut remap);
            }
            assert_eq!(verdict(&permuted), expected, "arena order: {}", path.display());
        }
    }
}

#[test]
fn ownership_fixpoint_limits_are_incomplete() {
    // #634 step 1 -> 1.5: memory 781523 -> 772761, work 23807 -> 36692.
    // The old merge charge omitted nested comparisons; account them as well.
    let source = include_str!("fixtures/verify/valid/B4-rebind-loop-and-staging.oir");
    let module = text::parse(source).unwrap();
    let stats = verify(&module).unwrap().stats().clone();
    for limits in [
        VerifyLimits { steps: stats.steps - 1, ..VerifyLimits::default() },
        VerifyLimits { memory_bytes: stats.memory_bytes - 1, ..VerifyLimits::default() },
    ] {
        let options = VerifyOptions { limits, ..VerifyOptions::default() };
        let report = verify_with(&module, &options, VerifyContext::default()).unwrap_err();
        assert_eq!(report.failure, VerifyFailure::Incomplete, "{report:?}");
    }
}

#[test]
fn opaque_cleanup_verdicts_survive_function_order_permutations() {
    use std::collections::BTreeSet;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify");
    for class in ["valid", "invalid"] {
        for name in ["B11-foreign-generic-box", "B11-generic-fn-origin"] {
            let module = fixture_module(&root, &format!("{class}/{name}.oir"));
            let verdict = |module: &okkama_oir::Module| match fixture_result(module, ErrorPolicy::All) {
                Ok(()) => (None, BTreeSet::new()),
                Err(report) => (Some(report.failure), report.diagnostics.into_iter().map(|diagnostic| {
                    let site = match diagnostic.primary.entity {
                        EntityLocation::Instruction { function, instruction } => {
                            format!("{}:{}", module.functions.get(function).unwrap().symbol, instruction.as_u32())
                        }
                        other => format!("{other:?}"),
                    };
                    (diagnostic.rule, site)
                }).collect()),
            };
            let expected = verdict(&module);
            assert_eq!(expected.0.is_some(), class == "invalid", "{name}");
            let canonical = text::print(&module).unwrap();
            let mut parts = canonical.split("\nfunc ");
            let prefix = parts.next().unwrap();
            let mut functions: Vec<_> = parts.collect();
            for _ in 0..functions.len() {
                functions.rotate_left(1);
                for reverse in [false, true] {
                    let mut order = functions.clone();
                    if reverse {
                        order.reverse();
                    }
                    let source = format!("{prefix}\nfunc {}", order.join("\nfunc "));
                    let permuted = text::parse(&source).unwrap();
                    assert_eq!(verdict(&permuted), expected, "{class}/{name}: {source}");
                }
            }
        }
    }
}

#[test]
fn opaque_origin_index_obeys_verification_budgets() {
    // #634 step 1 -> 1.5: memory 266753 -> 264113, work 1319 -> 3203.
    // The sparse B-tree charge still exceeds the old dense 163028-byte bound.
    // Enough completes; one byte/step below its own charge is Incomplete.
    let module = text::parse(include_str!("fixtures/verify/valid/B11-indexed-origins.oir")).unwrap();
    let options = VerifyOptions { execution: ExecutionMode::Tests, ..VerifyOptions::default() };
    let catalog = FixtureContracts(&module);
    let context = VerifyContext { external_contracts: &catalog, ..VerifyContext::default() };
    let stats = verify_with(&module, &options, context).unwrap().stats().clone();
    for limits in [
        VerifyLimits { steps: stats.steps - 1, ..VerifyLimits::default() },
        VerifyLimits { memory_bytes: stats.memory_bytes - 1, ..VerifyLimits::default() },
    ] {
        let options = VerifyOptions { limits, ..options.clone() };
        let report = verify_with(&module, &options, context).unwrap_err();
        assert_eq!(report.failure, VerifyFailure::Incomplete, "{report:?}");
    }
}

#[test]
fn opaque_cleanup_pairs_reject_only_cleanup_effects() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify");
    let mut failures = Vec::new();
    for name in [
        "generic-fn-origin", "generic-fn-query", "closed-fn-origin", "foreign-generic-box",
        "local-declaration-parameter", "conflicting-opaque-typeof", "bodyless-opaque-result",
        "dictionary-catalogue-role", "dictionary-take-convention", "generic-role-declaration",
        "uninterned-role-declaration", "indexed-origins",
    ] {
        for class in ["valid", "invalid"] {
            let path = format!("{class}/B11-{name}.oir");
            let module = fixture_module(&root, &path);
            match fixture_result(&module, ErrorPolicy::All) {
                Ok(()) if class == "invalid" => failures.push(format!("{path}: accepted")),
                Err(report) if class == "valid" || report.diagnostics.iter().any(|diagnostic| {
                    !matches!(diagnostic.rule, RuleId::B112 | RuleId::B104)
                }) => failures.push(format!("{path}: {report:?}")),
                _ => {}
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn absent_call_summaries_are_specific_to_high() {
    use okkama_oir::Profile;
    let source = include_str!("fixtures/verify/valid/B5-high-optional-summary.oir");
    let mut module = text::parse(source).unwrap();
    verify(&module).unwrap();
    for profile in [Profile::Own, Profile::Coro, Profile::Low] {
        module.profile = profile;
        let report = verify(&module).unwrap_err();
        assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == RuleId::B51), "{report:?}");
    }
}

#[test]
fn protocol_census_and_structural_gaps_are_explicit() {
    use okkama_oir::*;
    let module = text::parse(include_str!("fixtures/verify/valid/B7-any-family.oir")).unwrap();
    let verified = verify(&module).unwrap();
    assert_eq!(verified.stats().dynamic.sites.len(), 1);
    assert_eq!(verified.stats().dynamic.census.module_count(DynamicReason::Any), 1);
    assert_eq!(verified.stats().dynamic.census.module_count(DynamicReason::Merge), 0);
    assert!(matches!(verified.stats().dynamic.sites[0].site, OperationSite::Terminator { .. }));
    let module = text::parse(include_str!("fixtures/verify/valid/B9-frame-save-resume-cancel.oir")).unwrap();
    assert!(verify_coro(&module).unwrap().stats().structural_checks
        .contains(&StructuralCheck::B95FrameLayoutAndContinuations));
    let module = text::parse(include_str!("fixtures/verify/valid/B9-duplicate-positions-shared-outcome.oir")).unwrap();
    assert!(verify_own(&module).unwrap().stats().structural_checks
        .contains(&StructuralCheck::B96CollectorPositions));
}

#[test]
fn malformed_cleanup_proof_arenas_are_rejected_without_panics() {
    use okkama_oir::*;
    for corruption in 0..3 {
        let mut module = text::parse(include_str!("fixtures/verify/valid/B10-common-cleanup-plan.oir")).unwrap();
        let body = module.functions.get_mut(FuncId::from_u32(0)).unwrap().body.as_mut().unwrap();
        match corruption {
            0 => body.cleanup.plans.get_mut(CleanupPlanId::from_u32(0)).unwrap().close =
                CleanupCloseId::from_u32(u32::MAX),
            1 => body.cleanup.roots.get_mut(RootMapId::from_u32(0)).unwrap().bindings =
                vec![RootBinding { value: ValueId::from_u32(u32::MAX), trace: RootTrace::CleanupActivation }],
            _ => body.cleanup.roots.get_mut(RootMapId::from_u32(0)).unwrap().boundary =
                RootBoundary::BeforeInstruction(InstId::from_u32(u32::MAX)),
        }
        let report = verify_low(&module).unwrap_err();
        assert_eq!(report.failure, VerifyFailure::Rejected);
        assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == RuleId::S3));
    }
}

#[test]
fn exponentially_many_relational_states_still_exhaust_default_work() {
    // Each phi independently aliases x or y. Eighteen tiny blocks represent
    // 2^18 correlated histories; a large SSA universe is not the workload.
    // Dense: 49999274 work / 254140328 bytes; step 2: 49999990 / 20223097.
    // Step 1.6: 50000000 work / 11444314 bytes. Keep both thresholds unchanged:
    // structural sharing saves memory, while this history explosion still hits default work.
    fn diamonds(count: usize) -> okkama_oir::Module {
        let mut source = String::from(
            "oir 2 high\nmodule explosion\nrequires core\nscope S action in @f\n\
             func @f(%c: bool, %x: int, %y: int) -> void {\n^entry in S:\n",
        );
        for index in 0..count {
            source.push_str(&format!(
                "    cond %c, ^b{index}(%x), ^b{index}(%y)\n^b{index}(%p{index}: int) in S:\n",
            ));
        }
        source.push_str("    ret\n}\n");
        text::parse(&source).unwrap()
    }
    verify(&diamonds(4)).unwrap();
    let module = diamonds(18);
    let body = module.functions.iter().next().unwrap().1.body.as_ref().unwrap();
    assert_eq!(body.blocks.len(), 19);
    assert_eq!(body.values.len(), 21);
    let report = verify(&module).unwrap_err();
    assert_eq!(report.failure, VerifyFailure::Incomplete, "{report:?}");
    assert!(report.stats.steps > 45_000_000, "{report:?}");
    assert!(report.stats.memory_bytes < VerifyLimits::default().memory_bytes, "{report:?}");
}

// #634: mutations change heap/SSA history while preserving every diagnostic.
// The feature oracle also compares All and First against the untouched solver
// for each mutated module, rather than just comparing accepted/rejected counts.
#[test]
fn normalization_preserves_rejections_under_dead_prefixes_and_ssa_permutations() {
    use okkama_oir::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn evidence(module: &Module, inverse: &BTreeMap<(FuncId, ValueId), ValueId>)
        -> (Option<VerifyFailure>, BTreeSet<(RuleId, EntityLocation)>) {
        match fixture_result(module, ErrorPolicy::All) {
            Ok(()) => (None, BTreeSet::new()),
            Err(report) => (Some(report.failure), report.diagnostics.into_iter().map(|diagnostic| {
                let mut entity = diagnostic.primary.entity;
                if let EntityLocation::Value { function, value } = &mut entity {
                    *value = inverse.get(&(*function, *value)).copied().unwrap_or(*value);
                }
                (diagnostic.rule, entity)
            }).collect()),
        }
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify");
    for class in ["valid", "invalid"] {
        let mut paths: Vec<_> = fs::read_dir(root.join(class)).unwrap()
            .map(|entry| entry.unwrap().path()).collect();
        paths.sort();
        for path in paths {
            let original = text::parse(&fs::read_to_string(&path).unwrap()).unwrap();
            let expected = evidence(&original, &BTreeMap::new());
            assert_ne!(expected.0, Some(VerifyFailure::Incomplete), "{}", path.display());
            for reverse_prefix in [false, true] {
                let mut changed = original.clone();
                let scalar = changed.types.intern(Type::Bool).unwrap();
                let profile = changed.profile;
                let types = &changed.types;
                let mut inverse = BTreeMap::new();
                for (function_id, function) in changed.functions.iter_mut() {
                    let Some(body) = &mut function.body else {
                        continue;
                    };
                    let input_packet = body.blocks.get(body.entry).unwrap().params.iter().any(|value| {
                        types.get(body.values.get(*value).unwrap().ty) == Ok(&Type::Service(ServiceType::Own))
                    });
                    // An explicit packet ABI forbids body evaluation before
                    // own.accept. Do not turn a dead prefix into an ABI error.
                    // C-2: a D21 async body starts with the start resume of
                    // its frame, so a prefix before it is an ABI error too.
                    if !input_packet && body.frame_layout.is_none() {
                        let mut prefix = Vec::new();
                        for bit in 0..8 {
                            let inst = InstId::from_u32(body.insts.len() as u32);
                            let value = body.values.alloc(ValueData {
                                ty: scalar,
                                def: Definition::InstResult { inst, index: 0 },
                                facts: ValueFacts::Plain { certificate: PlainCertificate::Scalar },
                            }).unwrap();
                            prefix.push(body.insts.alloc(Instruction {
                                results: vec![value], op: Operation::Const(Literal::Bool(bit % 2 == 0)),
                                facts: InstructionFacts::default(), loc: None,
                            }).unwrap());
                        }
                        if reverse_prefix {
                            prefix.reverse();
                        }
                        if profile == Profile::Low {
                            // Bool constants need no trace. Root all entry inputs
                            // even when an invalid fixture omits its entry map.
                            let bindings: Vec<_> = body.blocks.get(body.entry).unwrap().params.iter()
                                .filter_map(|value| {
                                    let ty = body.values.get(*value).unwrap().ty;
                                    let trace = match types.get(ty).unwrap() {
                                        Type::Service(ServiceType::CleanupState) => RootTrace::CleanupActivation,
                                        Type::Service(ServiceType::Scope(_)) => RootTrace::ScopeInventory,
                                        Type::Service(ServiceType::Own) => RootTrace::PacketInventory,
                                        Type::Service(ServiceType::Pending) => RootTrace::PendingValueReserveDeps,
                                        Type::Service(ServiceType::Failure | ServiceType::FailureState) => {
                                            RootTrace::FailureGraph
                                        }
                                        Type::Service(ServiceType::ExitState) => RootTrace::ExitReserve,
                                        Type::Service(ServiceType::Deps) => RootTrace::FullDependencies,
                                        Type::Service(_) => return None,
                                        _ => RootTrace::Value(ty),
                                    };
                                    Some(RootBinding { value: *value, trace })
                                }).collect();
                            for inst in &prefix {
                                for boundary in [
                                    RootBoundary::BeforeInstruction(*inst),
                                    RootBoundary::AfterInstruction(*inst),
                                ] {
                                    body.cleanup.roots.alloc(RootRequirement {
                                        boundary, bindings: bindings.clone(), through_suspend: true,
                                    }).unwrap();
                                }
                            }
                        }
                        let entry = body.blocks.get_mut(body.entry).unwrap();
                        prefix.append(&mut entry.insts);
                        entry.insts = prefix;
                    }
                    let mut values = Arena::new();
                    let mut remap = IdRemap::default();
                    for (old, data) in body.values.iter().collect::<Vec<_>>().into_iter().rev() {
                        let new = values.alloc(data.clone()).unwrap();
                        remap.values.insert(old, new);
                        inverse.insert((function_id, new), old);
                    }
                    body.values = values;
                    body.rewrite(&mut remap);
                }
                assert_eq!(evidence(&changed, &inverse), expected, "{}", path.display());
                // Exercise diagnostic ordering as well as the complete set.
                let all = fixture_result(&changed, ErrorPolicy::All);
                let first = fixture_result(&changed, ErrorPolicy::First);
                if let (Err(all), Err(first)) = (all, first) {
                    assert_eq!(first.diagnostics, all.diagnostics[..1], "{}", path.display());
                }
            }
        }
    }
}

#[test]
fn normalization_keeps_dead_scalar_named_by_cleanup_root_map() {
    let source = include_str!("fixtures/verify/valid/B10-dead-scalar-cleanup-root.oir");
    let module = text::parse(source).unwrap();
    for errors in [ErrorPolicy::All, ErrorPolicy::First] {
        fixture_result(&module, errors).unwrap();
    }
}

#[test]
fn normalization_keeps_first_backedge_and_reset_scope_rejections() {
    use std::collections::BTreeSet;
    let source = include_str!("fixtures/verify/valid/B4-inner-admission-rebind-loop.oir");
    fixture_result(&text::parse(source).unwrap(), ErrorPolicy::All).unwrap();
    // The first iteration has no closed SI history. Backedges do; resetting SI
    // while lexically inside SI still observes that history and must emit B42.
    let rejected = source.replace("^loop(%current: scope<S>) in S:", "^loop(%current: scope<S>) in SI:");
    let module = text::parse(&rejected).unwrap();
    let report = fixture_result(&module, ErrorPolicy::All).unwrap_err();
    assert_eq!(report.failure, VerifyFailure::Rejected);
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == RuleId::B42));
    let evidence = |report: VerifyReport| report.diagnostics.into_iter()
        .map(|diagnostic| (diagnostic.rule, diagnostic.primary.entity)).collect::<BTreeSet<_>>();
    // Add the dead prefix through the arena so original instruction locations
    // remain stable, including both first-iteration and backedge diagnostics.
    let mut changed = module.clone();
    let ty = changed.types.intern(okkama_oir::Type::Bool).unwrap();
    let function = changed.functions.iter_mut().find(|(_, function)| function.symbol == "f").unwrap().1;
    let body = function.body.as_mut().unwrap();
    let inst = okkama_oir::InstId::from_u32(body.insts.len() as u32);
    let value = body.values.alloc(okkama_oir::ValueData {
        ty, def: okkama_oir::Definition::InstResult { inst, index: 0 },
        facts: okkama_oir::ValueFacts::Plain { certificate: okkama_oir::PlainCertificate::Scalar },
    }).unwrap();
    body.insts.alloc(okkama_oir::Instruction {
        results: vec![value], op: okkama_oir::Operation::Const(okkama_oir::Literal::Bool(false)),
        facts: Default::default(), loc: None,
    }).unwrap();
    body.blocks.get_mut(body.entry).unwrap().insts.insert(0, inst);
    assert_eq!(evidence(fixture_result(&changed, ErrorPolicy::All).unwrap_err()), evidence(report));
}

#[test]
fn normalization_keeps_a_successor_unused_on_only_one_branch() {
    let source = "oir 2 high
module unused
requires core
object C {
}
scope S action in @f
scope Ss action in @sink
func @f(%again: bool, %next: ^C) -> void {
^entry in S:
    %dead: int facts(plain(certificate=scalar)) = const 1
    cond %again, ^used, ^unused
^used in S:
    call @sink(%next)
    br ^done
^unused in S:
    br ^done
^done in S:
    ret
}
func @sink(%self: C) -> void {
^entry in Ss:
    ret
}";
    for source in [source.to_owned(), source.replace('%', "%alpha_")] {
        let module = text::parse(&source).unwrap();
        let report = fixture_result(&module, ErrorPolicy::All).unwrap_err();
        assert_eq!(report.failure, VerifyFailure::Rejected);
        assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == RuleId::B82), "{report:?}");
        let first = fixture_result(&module, ErrorPolicy::First).unwrap_err();
        assert_eq!(first.diagnostics, report.diagnostics[..1]);
    }
}

#[test]
fn structural_profiles_skip_ownership_but_preserve_structural_gates() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify");
    let options = VerifyOptions {
        mode: VerifyMode::Structural,
        execution: ExecutionMode::Tests,
        ..VerifyOptions::default()
    };
    for path in [
        "valid/H3-dominating-defer-prefix.oir",
        "invalid/B2-duplicate-input.oir",
        "invalid/B3-dead-borrow.oir",
        "invalid/B4-double-close.oir",
        "invalid/B10-common-cleanup-plan.oir",
    ] {
        let module = fixture_module(&root, path);
        let context = VerifyContext {
            external_contracts: &FixtureContracts(&module),
            ..VerifyContext::default()
        };
        let verified = match module.profile {
            okkama_oir::Profile::High => verify_high_with(&module, &options, context),
            okkama_oir::Profile::Own => verify_own_with(&module, &options, context),
            okkama_oir::Profile::Low => verify_low_with(&module, &options, context),
            other => panic!("unexpected profile {other:?}"),
        }.unwrap_or_else(|error| panic!("{path}: {error:?}"));
        assert_eq!(verified.mode(), VerifyMode::Structural);
        assert!(verified.stats().structural_checks.is_empty());
        assert!(verified.stats().dynamic.sites.is_empty());
        if path.starts_with("invalid/") {
            let full = fixture_result(&module, ErrorPolicy::All).unwrap_err();
            assert_eq!(full.failure, VerifyFailure::Rejected, "{path}");
        }
        let selected = verify_function(&module, okkama_oir::FuncId::from_u32(0), context, &options);
        assert_eq!(selected.failure, None, "{path}: {selected:?}");
    }
    for path in [
        "invalid/S3-tombstone.oir",
        "invalid/T1-arithmetic.oir",
        "invalid/SSA2-sibling-value-use.oir",
        "invalid/A1-foreign-field-read.oir",
    ] {
        let module = fixture_module(&root, path);
        let context = VerifyContext {
            external_contracts: &FixtureContracts(&module),
            ..VerifyContext::default()
        };
        let report = verify_with(&module, &options, context).unwrap_err();
        assert_eq!(report.failure, VerifyFailure::Rejected, "{path}");
        assert_eq!(report.diagnostics[0].rule, fixture_result(&module, ErrorPolicy::All)
            .unwrap_err().diagnostics[0].rule, "{path}");
    }
}

#[test]
fn structural_exhaustion_rejects_without_an_incomplete_ownership_verdict() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify");
    let module = fixture_module(&root, "valid/T1-arithmetic.oir");
    for limits in [
        VerifyLimits { steps: 0, ..VerifyLimits::default() },
        VerifyLimits { memory_bytes: 0, ..VerifyLimits::default() },
        VerifyLimits { entities: 0, ..VerifyLimits::default() },
    ] {
        let options = VerifyOptions { mode: VerifyMode::Structural, limits, ..VerifyOptions::default() };
        let report = verify_with(&module, &options, VerifyContext::default()).unwrap_err();
        assert_eq!(report.failure, VerifyFailure::Rejected);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.rule == RuleId::S1 && diagnostic.message.contains("resource limit")
        }), "{report:?}");
        let full = VerifyOptions { mode: VerifyMode::Full, ..options };
        assert_eq!(verify_with(&module, &full, VerifyContext::default()).unwrap_err().failure,
            VerifyFailure::Incomplete);
    }
}

#[test]
fn structural_type_errors_do_not_emit_ownership_rules() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify");
    let module = fixture_module(&root, "invalid/B1-token-as-reference.oir");
    let options = VerifyOptions { mode: VerifyMode::Structural, ..VerifyOptions::default() };
    let report = verify_with(&module, &options, VerifyContext::default()).unwrap_err();
    assert_eq!(report.failure, VerifyFailure::Rejected);
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == RuleId::T1));
    assert!(report.diagnostics.iter().all(|diagnostic| {
        matches!(diagnostic.rule, RuleId::S1 | RuleId::S2 | RuleId::S3
            | RuleId::T1 | RuleId::T2 | RuleId::T3 | RuleId::T4 | RuleId::T5 | RuleId::T6
            | RuleId::SSA1 | RuleId::SSA2 | RuleId::B61 | RuleId::B62
            | RuleId::A1 | RuleId::A2 | RuleId::A3)
    }), "{report:?}");
    let full = fixture_result(&module, ErrorPolicy::All).unwrap_err();
    assert!(full.diagnostics.iter().any(|diagnostic| diagnostic.rule == RuleId::B11));
}

/// #688, plan P2.5 §3.1: `fut.done`, `fut.cancel` and `fut.cancelled` belong to High.
#[test]
fn high_admits_future_queries_and_cancel() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let module = fixture_module(&root, "verify/valid/H6-future-query-in-high.oir");
    assert_eq!(module.profile, okkama_oir::Profile::High);
    fixture_result(&module, ErrorPolicy::All).unwrap_or_else(|report| panic!("{report:?}"));
    // The spec module of §10.1 is an illustration (its `@make` has no body form), so it is not a
    // complete certificate; its `fut.cancel` and `fut.cancelled` must still satisfy H6 in High.
    let spec = fixture_module(&root, "spec/10-never-started-cancel.oir");
    assert_eq!(spec.profile, okkama_oir::Profile::High);
    let diagnostics = fixture_result(&spec, ErrorPolicy::All).err().map(|report| report.diagnostics);
    let h6: Vec<_> = diagnostics.iter().flatten().filter(|d| d.rule.as_str() == "OIR2-H6").collect();
    assert!(h6.is_empty(), "{h6:?}");
}

/// #688 keeps H6 for the async operations that only own or later profiles may hold.
#[test]
fn high_still_rejects_future_state_operations() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let module = fixture_module(&root, "verify/invalid/H6-future-state-in-high.oir");
    let report = fixture_result(&module, ErrorPolicy::All).unwrap_err();
    assert_eq!(report.failure, VerifyFailure::Rejected);
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule.as_str() == "OIR2-H6"), "{report:?}");
}

/// D26 (#856): the negatives of §12.2 of the duty-slot memo beyond the two canonical ones of coro-matrix.tsv. Each
/// edits a positive slot fixture and names the rule, and a part of its message, that must reject the result.
#[test]
fn d26_duty_slot_negatives() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/async/coro");
    let outer = "d26-outer-slot-across-inner-close.oir";
    let spawn = "d26-branch-spawn.oir";
    let cases: &[(&str, &[(&str, &str)], &str, &str)] = &[
        // R-1: fut.new fills an empty slot of a scope other than its owner.
        (outer, &[("%13: cache-state = duty.empty owner S1", "%13: cache-state = duty.empty owner S2")],
            "d26-slot-occupied", "fut.new fills an empty cache slot of its owner"),
        // R-1: fut.spawn fills an empty task slot of a scope other than its Future's owner.
        (outer, &[
            ("    %16: closed = scope.end",
                "    %17: task-state = duty.empty owner S2, task true\n    %16: closed = scope.end"),
            ("    %10: deps = deps.of %7\n",
                "    %10: deps = deps.of %7\n    \
                 (%18: cache-state, %19: task-state) = fut.spawn %7, state %8, task_slot %17\n"),
            ("duties [%8]", "duties [%19, %18]"),
        ], "d26-slot-occupied", "fut.spawn fills an empty task slot of its Future's owner"),
        // The kinds do not mix: a task slot is not a cache slot.
        (spawn, &[("owner S1, slot %24", "owner S1, slot %25")], "OIR2-T1", ""),
        // R-2: a spawn forwards the cache duty of the Future it starts, never an empty slot.
        (spawn, &[
            ("    %25: task-state", "    %30: cache-state = duty.empty owner S1\n    %25: task-state"),
            ("state %9, task_slot %25", "state %30, task_slot %25"),
        ], "d24-spawn-not-fresh", "fut.spawn forwards the cache duty"),
        // R-3: a close settles every slot of its scope, the empty ones included.
        (spawn, &[("duties [%25, %24]", "duties [%24]")], "OIR2-B10.6", "complete, unique service schedule coverage"),
        // A slot belongs to an open scope.
        (outer, &[("    %2: own = own.empty",
            "    %17: task-state = duty.empty owner S2, task true\n    %2: own = own.empty")],
            "OIR2-B10.6", "duty.empty names a slot of a scope that is not open"),
        // D16: the schedules of one owner list its duties in one order, also a duty whose slot is full on some
        // paths only (the normal close lists the first Future, then the second's slot).
        ("d26-two-futures-order.oir", &[("duties [%9, %39]", "duties [%39, %9]")], "OIR2-B10.6",
            "list its duties in one order"),
    ];
    for (fixture, edits, rule, message) in cases {
        let mut source = fs::read_to_string(root.join(fixture)).unwrap();
        for (from, to) in *edits {
            assert_eq!(source.matches(from).count(), 1, "{fixture}: {from}");
            source = source.replacen(from, to, 1);
        }
        let module = text::parse(&source).unwrap_or_else(|error| panic!("{fixture}: {error}\n{source}"));
        let report = fixture_result(&module, ErrorPolicy::All).unwrap_err();
        assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule.as_str() == *rule
            && diagnostic.message.contains(message)), "{fixture} {edits:?}: {report:?}");
    }
}
