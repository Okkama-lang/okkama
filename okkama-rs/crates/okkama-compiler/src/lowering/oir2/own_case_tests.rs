//! The own/low lane: sources through High -> own -> verify_own -> low -> verify_low.
//!
//! Small sources pin each elaboration rule under the full ownership verifier.
//! Conformance cases are taken through the structural gate (the build policy)
//! and a sample of them through the full verifier under its default limits.
use super::{P1NativeSummaries, StdLoader, ir};
use crate::lowering::{LoadedModule, ModuleLoader};
use crate::syntax::{self, ParseOptions};
use crate::SourceFile;
use std::path::{Path, PathBuf};

/// Case-local modules first, then std and the conformance harness.
struct CaseLoader {
    case: Option<PathBuf>,
}

impl ModuleLoader for CaseLoader {
    fn load(&mut self, path: &str, from: &str) -> Result<Option<LoadedModule>, String> {
        let local = self.case.as_ref().map(|case| case.join(format!("{path}.ok"))).filter(|file| file.exists());
        if let Some(file) = local {
            let text = std::fs::read_to_string(&file).map_err(|error| error.to_string())?;
            let source = SourceFile::new(file.to_string_lossy(), text).map_err(|error| error.to_string())?;
            let program = syntax::parse(&source, &ParseOptions::default()).map_err(|error| format!("{error:?}"))?;
            let program = syntax::prune::strip_test_items(program);
            return Ok(Some(LoadedModule { id: file.to_string_lossy().into_owned(), source, program }));
        }
        StdLoader.load(path, from)
    }
}

pub(super) fn case_directory(id: &str) -> PathBuf {
    let (level, suffix) = id.split_once('.').unwrap();
    let cases = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../conformance/cases");
    let level = std::fs::read_dir(&cases).unwrap().map(|entry| entry.unwrap().path())
        .find(|path| path.file_name().unwrap().to_string_lossy().starts_with(&format!("{level}-")))
        .unwrap();
    level.join(suffix)
}

pub(super) fn high(source: &str, case: Option<PathBuf>) -> Result<ir::Module, String> {
    let file = SourceFile::new("main.ok", source).map_err(|error| format!("source: {error:?}"))?;
    let program = syntax::parse(&file, &ParseOptions::default()).map_err(|error| format!("syntax: {error:?}"))?;
    let roots = [crate::lowering::RootFile { source: &file, program: &program }];
    let mut loader = CaseLoader { case };
    super::super::lower_linked_high(&roots, &mut loader).map_err(|error| format!("high: {error}"))
}

/// A single-module source in test mode (spec §14): its tests are the module's entries.
fn test_high(source: &str) -> Result<ir::Module, String> {
    let file = SourceFile::new("main.ok", source).map_err(|error| format!("source: {error:?}"))?;
    let options = ParseOptions { mode: syntax::ParseMode::Full, ..ParseOptions::default() };
    let program = syntax::parse(&file, &options).map_err(|error| format!("syntax: {error:?}"))?;
    let program = syntax::prune::desugar_test_items(program);
    let sources = vec![crate::analysis::SourceModule { path: "app".into(), program: &program, imports: vec![] }];
    let analysis = crate::analysis::analyze_mode(sources, 1, true)
        .map_err(|errors| format!("analysis: {errors:?}"))?;
    super::super::lower_to_oir2(&analysis).map_err(|error| format!("high: {error}"))
}

/// The Low module; `Err` names the stage that stopped.
pub(super) fn own_to_low(source: &str, case: Option<PathBuf>, mode: ir::verify::VerifyMode)
    -> Result<ir::Module, String> {
    module_to_low(high(source, case)?, mode)
}

/// High -> own -> low under one verifier mode; test-mode modules verify as test executions.
fn module_to_low(module: ir::Module, mode: ir::verify::VerifyMode) -> Result<ir::Module, String> {
    let (own, verify) = module_to_own(module, mode)?;
    let low = ir::low::lower_cleanup(own, &ir::low::LowOptions::default())
        .map_err(|error| format!("low: {error:?}"))?;
    let catalog = P1NativeSummaries::for_module(&low).map_err(|error| format!("contracts: {error:?}"))?;
    let context = ir::verify::VerifyContext { text_locations: None, external_contracts: &catalog };
    ir::verify::verify_with(&low, &verify, context).map_err(|error| format!("verify_low: {error:?}"))?;
    Ok(low)
}

/// The verified Own module and the verification options; `Err` names the stage that stopped.
pub(super) fn own_stage(source: &str, case: Option<PathBuf>, mode: ir::verify::VerifyMode)
    -> Result<(ir::Module, ir::verify::VerifyOptions), String> {
    module_to_own(high(source, case)?, mode)
}

/// High -> own under one verifier mode; test-mode modules verify as test executions.
fn module_to_own(module: ir::Module, mode: ir::verify::VerifyMode)
    -> Result<(ir::Module, ir::verify::VerifyOptions), String> {
    let execution = if module.tests.is_some() {
        ir::verify::ExecutionMode::Tests
    } else {
        ir::verify::ExecutionMode::Production
    };
    let mut verify = ir::verify::VerifyOptions { mode, execution, ..ir::verify::VerifyOptions::default() };
    if std::env::var_os("OKKAMA_OWN_RAISED_LIMITS").is_some() {
        verify.limits.memory_bytes = 2 << 30;
        verify.limits.steps = 500_000_000;
    }
    let catalog = P1NativeSummaries::for_module(&module).map_err(|error| format!("contracts: {error:?}"))?;
    let context = ir::verify::VerifyContext { text_locations: None, external_contracts: &catalog };
    ir::verify::verify_with(&module, &verify, context).map_err(|error| format!("verify_high: {error:?}"))?;
    let options = ir::own::OwnOptions { verify: verify.clone(), ..ir::own::OwnOptions::default() };
    let inferred = ir::own::elaborate_ownership(module, &catalog, &options)
        .map_err(|error| format!("own_elaborate: {error:?}"))?;
    let own = ir::own::rewrite_subset(inferred, &options).map_err(|error| format!("own_rewrite: {error:?}"))?;
    if let Some(path) = std::env::var_os("OKKAMA_OWN_CASES_DUMP") {
        std::fs::write(path, ir::text::print(&own.module).unwrap()).unwrap();
    }
    let catalog = P1NativeSummaries::for_module(&own.module).map_err(|error| format!("contracts: {error:?}"))?;
    let context = ir::verify::VerifyContext { text_locations: None, external_contracts: &catalog };
    ir::verify::verify_with(&own.module, &verify, context)
        .map_err(|error| format!("verify_own: {}", rejection(&own.module, &error)))?;
    Ok((own.module, verify))
}

/// Rules, messages and the rejected instruction, compact enough for triage.
pub(super) fn rejection(module: &ir::Module, report: &ir::verify::VerifyReport) -> String {
    let mut text = format!("{:?}:", report.failure);
    for diagnostic in report.diagnostics.iter().take(4) {
        text.push_str(&format!(" [{:?} {}", diagnostic.rule, diagnostic.message));
        match diagnostic.primary.entity {
            ir::verify::EntityLocation::Instruction { function, instruction } => {
                let function = module.functions.get(function).unwrap();
                let body = function.body.as_ref().unwrap();
                text.push_str(&format!(" at {} {:?}", function.symbol, body.insts.get(instruction).unwrap().op));
            }
            ref entity => text.push_str(&format!(" at {entity:?}")),
        }
        text.push(']');
    }
    text
}

pub(super) fn full(source: &str) -> ir::Module {
    own_to_low(source, None, ir::verify::VerifyMode::Full).unwrap_or_else(|error| panic!("{error}\n{source}"))
}

pub(super) fn case(id: &str, mode: ir::verify::VerifyMode) -> Result<ir::Module, String> {
    let directory = case_directory(id);
    let source = std::fs::read_to_string(directory.join("main.ok")).unwrap();
    own_to_low(&source, Some(directory), mode)
}

pub(super) fn operations(module: &ir::Module) -> Vec<&ir::Operation> {
    module.functions.iter().filter_map(|(_, function)| function.body.as_ref()).flat_map(|body| {
        body.insts.iter().map(|(_, inst)| &inst.op).chain(body.blocks.iter().filter_map(|(_, block)| {
            match &block.term.kind {
                ir::TermKind::InvokeOwn { op, .. } => Some(op.as_ref()),
                _ => None,
            }
        }))
    }).collect()
}

// Task 1: collection operations.

#[test]
fn oir2_own_plain_builder_threads_an_element_journal() {
    let low = full(concat!(
        "module app; main() is int { let a is @int = [0, 2..=4, 9]; ",
        "let b = a.map() -> v { v * 2; }; a.size() + b.size(); }",
    ));
    assert!(operations(&low).iter().any(|op| matches!(op,
        ir::Operation::Collection(ir::CollectionOp::BuilderFinish { packet: Some(_), .. }))));
}

#[test]
fn oir2_own_map_literal_and_observations() {
    full(concat!(
        "module app; main() is int { let m is %string:int = [\"a\": 1, \"b\": 2]; ",
        "let n = m.size(); if m.has(\"a\") { m[\"a\"] + n; } else { 0; } }",
    ));
}

#[test]
fn oir2_own_derived_arrays_concat_remove_replace_slice_join() {
    full(concat!(
        "module app; main() is int { let a = [1, 2, 3]; let b = a + [4]; let c = b.removed_at(0); ",
        "let d = c.replaced(0, 7); let s = [d.size().string(), \"x\"].join(\",\"); ",
        "s.length() + d.slice(1, 2).size(); }",
    ));
}

#[test]
fn oir2_own_value_equality_borrows_managed_operands() {
    full(concat!(
        "module app; object Res { has id is int; Res(:id); destroy { } } ",
        "main() is int { let a = Res(id: 1); let b = a; if a == b { 1; } else { 0; } }",
    ));
}

#[test]
fn oir2_own_managed_array_literal_escapes_with_its_elements() {
    let low = full(concat!(
        "module app; object Res { has id is int; Res(:id); id() is int { id; } destroy { } } ",
        "object Maker { Maker(); make() is @Res { [Res(id: 1), Res(id: 2)]; } } ",
        "main() is int { let a = Maker().make(); a[0].id() + a.size(); }",
    ));
    let make = low.functions.iter().find(|(_, function)| function.symbol == "app.Maker.make#0").unwrap().1;
    let body = make.body.as_ref().unwrap();
    assert!(body.values.iter().all(|(_, value)| {
        !matches!(low.types.get(value.ty), Ok(ir::Type::Array(_)))
            || matches!(value.facts, ir::ValueFacts::Managed { .. })
    }));
}

#[test]
fn oir2_own_managed_builder_rehomes_fresh_elements_to_the_builder_owner() {
    let low = full(concat!(
        "module app; object Res { has id is int; Res(:id); destroy { } } ",
        "main() is int { let a = (0..3).map() -> i { Res(id: i); }; a.size(); }",
    ));
    assert!(operations(&low).iter().any(|op| matches!(op, ir::Operation::Ownership(
        ir::OwnershipOp::ScopeTransfer { states: ir::TransferStates::Between { .. }, .. }))));
}

/// #697: an element a builder collects may borrow what a nested scope binds: here the step of the cursor, bound
/// in the iteration scope as `$cursor`, owns the resource `item()` returns. The rehome of every scope inside the
/// builder's owner is a `held(...)` transfer, which takes the bound parts too, so the iteration scope gives up
/// the borrowed resource and it lives as long as the collection (Tomato, `L07.traversal.escaping`).
#[test]
fn oir2_own_builder_rehome_holds_what_the_cursor_scope_binds() {
    let source = concat!(
        "module app; import \"collections/iteration\" take Iterating, Cursor; ",
        "object Res { has id is int; Res(:id); destroy { } } ",
        "object Owned does Iterating<Res> { Owned(); cursor() is Cursor<Res> { Step(mine: Res(id: 0), at: 0); } } ",
        "object Step does Cursor<Res> { has mine is Res; has at is int; Step(:mine, :at); ",
        "available() is bool { at < 3; } item() is Res { mine; } ",
        "successor() is Cursor<Res> { Step(mine: Res(id: at + 1), at: at + 1); } } ",
        "object T { T(); kept() is int { let all is @Res = Owned().map() -> res { res; }; all.size(); } } ",
        "main() is int { T().kept(); }",
    );
    let low = own_to_low(source, None, ir::verify::VerifyMode::Structural).unwrap_or_else(|error| panic!("{error}"));
    let kept = low.functions.iter().find(|(_, function)| function.symbol == "app.T.kept#0").unwrap().1;
    let body = kept.body.as_ref().unwrap();
    let cursor = body.bindings.iter().find(|(_, binding)| binding.name == "$cursor").unwrap().1.scope;
    let mut held = vec![];
    for (_, block) in body.blocks.iter() {
        let ops = block.insts.iter().map(|inst| &body.insts.get(*inst).unwrap().op)
            .chain(match &block.term.kind {
                ir::TermKind::InvokeOwn { op, .. } => Some(op.as_ref()),
                _ => None,
            });
        for op in ops {
            if let ir::Operation::Ownership(ir::OwnershipOp::ScopeTransfer {
                states: ir::TransferStates::Between { from, .. }, selection,
            }) = op {
                let filter = body.selections.get(*selection).unwrap();
                assert!(matches!(filter, ir::SelectionExpr::Held(_)), "builder rehome filter {filter:?}");
                held.push(state_scope(&low, body, *from));
            }
        }
    }
    assert!(held.contains(&cursor), "the scope binding $cursor {cursor:?} gives up its part: {held:?}");
}

/// #668 F7: a failure Own introduces after `builder.finish` has no buffer left for the block's
/// High handler, so Own routes it past that handler only when the handler is a buffer cleanup
/// block: it abandons its buffers and re-raises. Any other handler is an elaboration error.
#[test]
fn oir2_own_skipped_buffer_handler_must_abandon_and_reraise() {
    let source = concat!(
        "module app; object Res { has id is int; Res(:id); destroy { } } ",
        "main() is int { let a = (0..3).map() -> i { Res(id: i); }; a.size(); }",
    );
    own_to_low(source, None, ir::verify::VerifyMode::Structural).unwrap_or_else(|error| panic!("{error}"));
    // The same program with one more instruction in every abandon-and-reraise handler.
    let text = ir::text::print(&high(source, None).unwrap()).unwrap();
    let lines: Vec<_> = text.lines().collect();
    let mut edited = vec![];
    let mut changed = 0;
    for (index, line) in lines.iter().enumerate() {
        let handler = index >= 2 && line.trim_start().starts_with("raise ")
            && lines[index - 1].trim_start().starts_with("builder.abandon ")
            && lines[index - 2].trim_start().starts_with('^');
        if handler {
            edited.push(format!("    %f7_{changed}: int = const 7"));
            changed += 1;
        }
        edited.push((*line).to_owned());
    }
    assert!(changed > 0, "the source has buffer cleanup handlers");
    let module = ir::text::parse(&edited.join("\n")).unwrap();
    let Err(error) = module_to_low(module, ir::verify::VerifyMode::Structural) else {
        panic!("own routed a failure past a handler that is not a buffer cleanup block");
    };
    assert!(error.contains("must abandon its buffers and re-raise"), "{error}");
}

/// The lexical scope a scope-state value belongs to.
fn state_scope(module: &ir::Module, body: &ir::Body, state: ir::ValueId) -> ir::ScopeId {
    match module.types.get(body.values.get(state).unwrap().ty).unwrap() {
        ir::Type::Service(ir::ServiceType::Scope(scope)) => *scope,
        other => panic!("{state:?} is not a scope state: {other:?}"),
    }
}

/// #666, OIR-2 §4.1 and §8: inserted elements go to the journal of the builder's owner, the
/// scope of `builder.new`. A collection built in a loop body releases its elements at the end
/// of each iteration, where Tomato destroys it, not at function exit. `kept` retains the root.
#[test]
fn oir2_own_builder_elements_die_with_the_iteration_that_built_them() {
    let low = own_to_low(concat!(
        "module app; object Res { has id is int; Res(:id); id() is int { id; } destroy { } } ",
        "main() is int { let kept = Res(id: 100); ",
        "for 0..2 -> round { let a = (0..2).map() -> i { Res(id: round * 10 + i); }; a.size(); } kept.id(); }",
    ), None, ir::verify::VerifyMode::Structural).unwrap_or_else(|error| panic!("{error}"));
    let main = low.functions.iter().find(|(_, function)| function.symbol == "main").unwrap().1;
    let body = main.body.as_ref().unwrap();
    let root = body.blocks.get(body.entry).unwrap().lexical_scope;
    assert!(body.insts.iter().any(|(_, inst)| matches!(inst.op,
        ir::Operation::Ownership(ir::OwnershipOp::ScopeBegin { scope }) if scope == root)), "the root is retained");
    let owners: Vec<_> = body.blocks.iter().filter(|(_, block)| block.insts.iter().any(|inst| {
        matches!(body.insts.get(*inst).unwrap().op,
            ir::Operation::Collection(ir::CollectionOp::BuilderNew { .. }))
    })).map(|(_, block)| block.lexical_scope).collect();
    assert_eq!(owners.len(), 1, "one builder.new in main");
    assert_ne!(owners[0], root, "the builder is made inside the loop body");
    let mut targets = vec![];
    for (_, block) in body.blocks.iter() {
        let ops = block.insts.iter().map(|inst| &body.insts.get(*inst).unwrap().op)
            .chain(match &block.term.kind {
                ir::TermKind::InvokeOwn { op, .. } => Some(op.as_ref()),
                _ => None,
            });
        for op in ops {
            if let ir::Operation::Ownership(ir::OwnershipOp::ScopeTransfer {
                states: ir::TransferStates::Between { to, .. }, ..
            }) = op {
                targets.push(state_scope(&low, body, *to));
            }
        }
    }
    assert!(!targets.is_empty(), "the fresh elements leave the lambda body scope");
    assert!(targets.iter().all(|scope| *scope == owners[0]),
        "elements go to the scope of builder.new {:?}, not the function root {root:?}: {targets:?}", owners[0]);
}

/// #667: a test function is a fixed observation root like the module entry, whatever its root
/// scope holds; its signature stays `() -> void`.
#[test]
fn oir2_own_test_functions_are_fixed_observation_roots() {
    let directory = case_directory("L05.generic.fake-target");
    let source = std::fs::read_to_string(directory.join("main.ok")).unwrap();
    for mode in [ir::verify::VerifyMode::Structural, ir::verify::VerifyMode::Full] {
        let low = test_high(&source).and_then(|module| module_to_low(module, mode))
            .unwrap_or_else(|error| panic!("{mode:?}: {error}"));
        let tests = low.tests.as_ref().expect("test-mode module lists its tests");
        assert_eq!(tests.len(), 1);
        for test in tests {
            let function = low.functions.get(test.function).unwrap();
            assert!(function.signature.params.is_empty() && function.signature.results.is_empty(), "{mode:?}");
            assert_eq!(function.signature.ownership_abi, ir::OwnershipAbi::Observation, "{mode:?}");
            // The fake is owned by the test's own root scope, which closes before it returns.
            let body = function.body.as_ref().unwrap();
            let root = body.blocks.get(body.entry).unwrap().lexical_scope;
            assert!(body.insts.iter().any(|(_, inst)| matches!(inst.op,
                ir::Operation::Ownership(ir::OwnershipOp::ScopeBegin { scope }) if scope == root)), "{mode:?}");
        }
    }
}

#[test]
fn oir2_own_conformance_collections_full() {
    for id in ["L01.lex.whitespace.minimal", "L05.array.removed-at-empty", "L05.array.replaced-past-end",
        "L05.typed-identity.empty-results-keep-tag", "L02.range.expansion", "L07.ownership.alias-borrows"] {
        case(id, ir::verify::VerifyMode::Full).unwrap_or_else(|error| panic!("{id}: {error}"));
    }
}

// Task 2: deps recipes and tombstones.

#[test]
fn oir2_own_caught_role_payload_carries_full_deps() {
    full(concat!(
        "module app; enum NotFound is Error = (not_found(key is string)); ",
        "main() is int { let r = try { throw NotFound.not_found(\"z\"); \"none\"; } ",
        "catch e is Error { e.text(); }; r.length(); }",
    ));
}

#[test]
fn oir2_own_live_tombstone_becomes_an_unreachable_trap() {
    let low = full(concat!(
        "module app; object Res { Res(); id() is int { 1; } destroy { } } ",
        "object Box { Box(); get(flag is bool) is Maybe<int> { if flag { Maybe.some(3); } else { Maybe.none; } } } ",
        "main() is int { let r = Res(); let v = match Box().get(true) { ",
        "case Maybe.some(n) { n + r.id(); } case Maybe.none { 0; } }; v; }",
    ));
    let traps = low.functions.iter().filter_map(|(_, function)| function.body.as_ref())
        .flat_map(|body| body.blocks.iter()).filter(|(_, block)| matches!(block.term.kind,
            ir::TermKind::Trap { code: ir::TrapCode::Unreachable, .. })).count();
    assert!(traps > 0);
}

#[test]
fn oir2_own_conformance_deps_full() {
    for id in ["L07.panic.not-catchable", "L07.catch.error-role-binding", "L07.confirm.else-panic"] {
        case(id, ir::verify::VerifyMode::Full).unwrap_or_else(|error| panic!("{id}: {error}"));
    }
}

// Task 3: lookup fallback, field defaults, defers.

#[test]
fn oir2_own_lookup_fallback_projects_the_found_element() {
    full(concat!(
        "module app; main() is int { let m is %string:int = [\"a\": 1]; let xs = [1, 2]; ",
        "let missing = m[\"b\"] else 5; let past = xs[7] else 3; missing + past; }",
    ));
}

#[test]
fn oir2_own_field_default_and_defer_helpers_have_bounded_entry_packets() {
    full(concat!(
        "module app; object Tracer { Tracer(); mark(v is int) is int { v; } } ",
        "object Stamp { has id is int = Tracer().mark(7); Stamp(); id() is int { id; } } ",
        "main() is int { Stamp().id(); }",
    ));
    full(concat!(
        "module app; object Worker { has n is int; Worker(:n); ",
        "run() is int { defer Worker(n: 0).touch(); n + 1; } touch() is int { n; } } ",
        "main() is int { Worker(n: 1).run(); }",
    ));
}

#[test]
fn oir2_own_conformance_other_full() {
    for id in ["L01.gram.index.fallback", "L04.ctor.full-body-sees-default", "L07.defer.all-run-and-precedence"] {
        case(id, ir::verify::VerifyMode::Full).unwrap_or_else(|error| panic!("{id}: {error}"));
    }
}

// Every listed case of the three lanes passes own and low under the build gate. The five
// string-source cases are checked against case.json in okkama-cli/tests/champ_cases_own.rs.
const OWNCOLL: [&str; 55] = [
    "L01.gram.collections.literals", "L01.gram.functional.inside-operators", "L01.gram.functional.suffix",
    "L01.gram.types.nested-map", "L01.lex.contextual.has-action-and-field", "L01.lex.float.range-not-float",
    "L01.lex.string.unicode-escape", "L01.lex.whitespace.minimal", "L02.cast.array-element-out-of-range",
    "L02.cast.arrays", "L02.cast.int-arbitrary-precision", "L02.equality.composite",
    "L02.equality.composite-boundary", "L02.eval-order.cast-operands", "L02.range.empty-and-single",
    "L02.range.expansion", "L02.range.expansion-bounds", "L02.string.order-consistent",
    "L03.functional.arrays-ranges-maps", "L03.functional.empty-sources",
    "L03.overload.collection-literal-argument", "L03.return.from-functional-block", "L04.equality.cyclic-graphs",
    "L04.harness.deliberate-failures", "L04.invariant.early-self-in-blocks", "L04.once.computed-once",
    "L05.array.access", "L05.array.removed-at", "L05.array.removed-at-empty", "L05.array.removed-at-past-end",
    "L05.array.replaced-negative", "L05.array.replaced-past-end", "L05.array.search-and-shape",
    "L05.array.take-negative", "L05.batching.functional", "L05.collections.array-difference",
    "L05.collections.map-difference", "L05.collections.map-merge", "L05.functional.early-exit",
    "L05.iterable.functional", "L05.map.access", "L05.map.int-key-order", "L05.map.key-types",
    "L05.map.missing-key", "L05.reduce.accumulator-in-place", "L05.typed-identity.empty-results-keep-tag",
    "L05.variants.ownership", "L05.variants.search", "L07.ownership.alias-borrows",
    "L07.ownership.shared-structure-loop", "L07.ownership.shared-structure-returned",
    "L09.collections.frequencies", "L09.collections.heap-and-deque", "L09.collections.persistent-list",
    "L09.compare.failing-order-stops-the-sort",
];
const OWNDEPS: [&str; 13] = [
    "L04.roles.contextual-wrap", "L04.roles.dispatch-and-covariance", "L05.choice.evaluation",
    "L05.variants.inferred-payloads", "L06.hidden.state-views", "L07.catch.error-role-binding",
    "L07.catch.source-order", "L07.confirm.bare-uncatchable", "L07.confirm.else-panic", "L07.control.not-caught",
    "L07.panic.not-catchable", "L07.try-expr.values", "L12.generic-successor-specialization",
];
const OWNOTHER: [&str; 13] = [
    "L01.gram.index.fallback", "L04.ctor.full-body-sees-default", "L04.fields.default-evaluated-then-replaced",
    "L04.fields.defaults-per-construction", "L04.shorthand.body-sees-supplied-value",
    "L05.collections.array-concat", "L05.map.fallback-lazy", "L05.map.float-key-fallback", "L05.map.rat-keys",
    "L07.defer.all-run-and-precedence", "L07.defer.failure-beats-destroy", "L07.defer.on-return-and-loops",
    "L07.ownership.reduce-builder",
];

#[test]
fn oir2_own_conformance_lanes_reach_low_under_the_build_gate() {
    let mut failures = vec![];
    for id in OWNCOLL.iter().chain(&OWNDEPS).chain(&OWNOTHER) {
        if let Err(error) = case(id, ir::verify::VerifyMode::Structural) {
            failures.push(format!("{id}: {}", error.chars().take(300).collect::<String>()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Opt-in triage: `OKKAMA_OWN_CASES=<file with one id per line>` prints the stage each
/// case stops at (`OKKAMA_OWN_CASES_STRUCTURAL` selects the build gate);
/// `OKKAMA_OWN_SOURCE=<file>` does the same for one source.
#[test]
fn oir2_own_cases_triage() {
    let mode = if std::env::var_os("OKKAMA_OWN_CASES_STRUCTURAL").is_some() {
        ir::verify::VerifyMode::Structural
    } else {
        ir::verify::VerifyMode::Full
    };
    if let Some(path) = std::env::var_os("OKKAMA_OWN_SOURCE") {
        let source = std::fs::read_to_string(path).unwrap();
        let verdict = own_to_low(&source, None, mode).map(|_| "LOW".to_owned()).unwrap_or_else(|error| error);
        println!("OWNSOURCE {verdict}");
    }
    let Some(path) = std::env::var_os("OKKAMA_OWN_CASES") else {
        return;
    };
    for id in std::fs::read_to_string(path).unwrap().split_whitespace() {
        let verdict = match std::panic::catch_unwind(|| case(id, mode)) {
            Ok(Ok(_)) => "LOW".to_owned(),
            Ok(Err(error)) => error,
            Err(_) => "PANIC".to_owned(),
        };
        let verdict: String = verdict.chars().take(600).collect();
        println!("OWNCASE {id}\t{verdict}");
    }
}

/// #671, #686 (P2.4 section 4.8): an Atomic field is a capability without an obligation, so the holder's
/// constructor returns no fresh creation (the SPB 17 layout gives the holder no identity), and every
/// function that partitions an Atomic declares the program failure its scope.split can raise. The full
/// verifier agrees (#700): such a creation needs no ownership result.
#[test]
fn oir2_own_atomic_holder_is_not_a_fresh_creation() {
    let source = concat!("module app; import \"cell\" take Cell; import \"async/atomic\" take Atomic; ",
        "object Recorded { has served is Atomic<int>; Recorded() { served = Atomic(0); } ",
        "next() is int { let at = served.value(); served.replace(at + 1); at; } } ",
        "main() is int { let c = Cell(0); c.replace(2); let a = Atomic(1); a.replace(3); ",
        "Recorded().next() + c.value() + a.value(); }");
    let (module, _) = own_stage(source, None, ir::verify::VerifyMode::Full).unwrap();
    let summary = |symbol: &str| {
        let function = module.functions.iter().find(|(_, function)| function.symbol.starts_with(symbol))
            .unwrap_or_else(|| panic!("{symbol}")).1;
        module.summaries.get(function.summary.unwrap()).unwrap().clone()
    };
    for constructor in ["cell.Cell_int.new", "app.Recorded.new"] {
        let summary = summary(constructor);
        assert!(summary.results.iter().all(|result| result.fresh.is_empty()), "{constructor}: {summary:?}");
    }
    for method in ["app.Recorded.next", "cell.Cell_int.replace"] {
        assert!(summary(method).contract.failures.contains(&ir::FailureKind::Program), "{method}");
    }
}

/// #672: a constructor stores a role field whose implementers are plain (L09.math.aggregates' Extremes,
/// L09.time.zones' ZonedDateTime). The value carries deps but no obligation and its action scope is
/// elided; own elaborates the construction without staging instead of failing on the missing scope. Under
/// the full verifier (#700) the holder is no fresh creation and its summary declares that it depends on the
/// role argument it retains.
#[test]
fn oir2_own_deps_only_field_needs_no_action_scope() {
    let source = concat!("module app; role Shape { area() is int; } ",
        "object Square does Shape { has side is int; Square(:side); area() is int { side * side; } } ",
        "object Holder { has shape is Shape; Holder(shape is Shape) { self.shape = shape; } ",
        "area() is int { shape.area(); } } ",
        "main() is int { Holder(Square(side: 2)).area(); }");
    let (module, _) = own_stage(source, None, ir::verify::VerifyMode::Full).unwrap();
    let constructor = module.functions.iter().find(|(_, function)| function.symbol.starts_with("app.Holder.new"))
        .unwrap().1;
    let summary = module.summaries.get(constructor.summary.unwrap()).unwrap();
    assert!(summary.results.iter().all(|result| result.fresh.is_empty()), "{summary:?}");
    assert_eq!(summary.results[0].external_deps, vec![0], "{summary:?}");
    let body = constructor.body.as_ref().unwrap();
    assert!(!body.insts.iter().any(|(_, inst)| matches!(inst.op,
        ir::Operation::Ownership(ir::OwnershipOp::Stage { .. }))));
}

/// #703 (L09.uuid.values, L10.stdin.lines): a role value with plain implementers carries deps but no
/// obligation, yet own partitions it with a checked scope.split when it is offered to an owning call. The
/// function that does so declares the program failure the split can raise, as for an Atomic (#671); SPB 17
/// refuses a body failure outside the declared mask (ENTRY_FAILURE_MASK).
#[test]
fn oir2_own_partitioned_role_value_declares_the_split_failure() {
    let source = concat!("module app; role Named { name() is string; } ",
        "object A does Named { A(); name() is string { \"a\"; } same(other is Named) is bool { ",
        "self.name() == other.name(); } } ",
        "object B does Named { B(); name() is string { \"b\"; } } ",
        "main() is int { if A().same(B()) { 1; } else { 0; } }");
    let (module, _) = own_stage(source, None, ir::verify::VerifyMode::Structural).unwrap();
    let (_, same) = module.functions.iter().find(|(_, function)| function.symbol.starts_with("app.A.same"))
        .unwrap();
    let body = same.body.as_ref().unwrap();
    let splits = body.blocks.iter().any(|(_, block)| matches!(&block.term.kind, ir::TermKind::InvokeOwn { op, .. }
        if matches!(op.as_ref(), ir::Operation::Ownership(ir::OwnershipOp::ScopeSplit { .. }))));
    assert!(splits, "own partitions the role argument");
    let summary = module.summaries.get(same.summary.unwrap()).unwrap();
    assert!(summary.contract.failures.contains(&ir::FailureKind::Program), "{summary:?}");
    // An object holding such a role value (io/stdin's Utf8Input) is partitioned the same way.
    let source = concat!("module app; role Named { name() is string; } ",
        "object B does Named { B(); name() is string { \"b\"; } } ",
        "object Holder { has inner is Named; Holder(inner is Named) { self.inner = inner; } ",
        "name() is string { inner.name(); } } ",
        "object User { User(); greet(holder is Holder) is string { holder.name(); } } ",
        "main() is int { User().greet(Holder(B())).length(); }");
    let (module, _) = own_stage(source, None, ir::verify::VerifyMode::Structural).unwrap();
    let (_, greet) = module.functions.iter().find(|(_, function)| function.symbol.starts_with("app.User.greet"))
        .unwrap();
    let summary = module.summaries.get(greet.summary.unwrap()).unwrap();
    assert!(summary.contract.failures.contains(&ir::FailureKind::Program), "{summary:?}");
}

/// The functions of an Own module whose elaborated body emits a checked scope.split, with whether their summary
/// declares the program failure it may raise (the loader's ENTRY_FAILURE_MASK counts it).
fn splitting_functions(module: &ir::Module) -> Vec<(String, bool)> {
    module.functions.iter().filter_map(|(_, function)| {
        let body = function.body.as_ref()?;
        let splits = body.blocks.iter().any(|(_, block)| matches!(&block.term.kind,
            ir::TermKind::InvokeOwn { op, .. }
                if matches!(**op, ir::Operation::Ownership(ir::OwnershipOp::ScopeSplit { .. }))));
        let declared = module.summaries.get(function.summary?).ok()?.contract.failures
            .contains(&ir::FailureKind::Program);
        splits.then(|| (function.symbol.clone(), declared))
    }).collect()
}

/// #706: own's elaboration splits every value it gives deps, and a deps-carrying reference without an obligation
/// (a cap value, a role) is one of them. A lambda own entry binding a cap parameter, a witness entry returning a
/// role field and a generic method staging a role value for return declare the split's program failure.
#[test]
fn oir2_own_reference_splits_declare_the_program_failure() {
    let sources = [
        ("main.lambda#0.__own_entry", concat!("module app; object Customer { has name is string; Customer(:name); ",
            "cap Card { label is string { name; } } } ",
            "main() is int { let reader = (card is Customer::Card) is string { card::label; }; ",
            "reader(Customer(name: \"d\")::Card).length(); }")),
        ("own_witness_entry", concat!("module app; role Chain { rest() is Chain; size() is int; } ",
            "object End does Chain { End(); rest() is Chain { End(); } size() is int { 0; } } ",
            "object Link does Chain { has following is Chain; Link(:following); rest() is Chain { following; } ",
            "size() is int { 1 + following.size(); } } ",
            "main() is int { let c is Chain = Link(following: Link(following: End())); c.rest().size(); }")),
        ("app::Pair<int>.first", concat!("module app; role Shape { area() is int; } ",
            "object Square does Shape { has side is int; Square(:side); area() is int { side * side; } } ",
            "object Pair<T> { has left is Shape; has tag is T; Pair(:left, :tag); ",
            "first() is Shape { if left.area() >= 0 { left; } else { left; } } } ",
            "main() is int { Pair(left: Square(side: 3), tag: 1).first().area(); }")),
    ];
    let mut failures = vec![];
    for (prefix, source) in sources {
        let (module, _) = own_stage(source, None, ir::verify::VerifyMode::Structural)
            .unwrap_or_else(|error| panic!("{prefix}: {error}"));
        let splitting = splitting_functions(&module);
        assert!(splitting.iter().any(|(symbol, _)| symbol.starts_with(prefix)), "{prefix}: {splitting:?}");
        let undeclared: Vec<_> = splitting.iter().filter(|(_, declared)| !declared).collect();
        if !undeclared.is_empty() {
            failures.push(format!("{prefix}: splits without the program failure: {undeclared:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// #707: the largest functions of these cases hold up to 4 800 values, of which at most 20 may own. Own's flow
/// analysis costs work in proportion to the facts of the values that may own, so they pass own and low under
/// the default budgets.
#[test]
fn oir2_own_large_plain_functions_finish_within_the_default_budgets() {
    let mut failures = vec![];
    for id in ["L05.variants.case-layouts", "L07.try-mark.diverging-operand", "L09.digest.md5-sha1-vectors"] {
        if let Err(error) = case(id, ir::verify::VerifyMode::Structural) {
            failures.push(format!("{id}: {}", error.chars().take(300).collect::<String>()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// #780: a pending check of a call inside `try` continues from the values that reach the check. Its trap block
/// takes fresh carried locals for the unwind edge into the handler; the call past it must not use them, or
/// verify_high rejects the use (SSA2). Tomato prints `Refused.empty`, then `a`.
#[test]
fn oir2_pending_check_inside_try_continues_with_the_checked_values() {
    let source = concat!("module app; import \"io/stdout\" take Stdout; enum Refused is Error = (empty); ",
        "object Tree { has name is string; Tree(name is string) { self.name = name; let alias = self; ",
        "confirm name != \"\" else throw Refused.empty; } name() is string { name; } } ",
        "main() is int { let out = Stdout(); ",
        "out.writeln(try { Tree(\"\").name(); } catch e is Refused { e.text(); }); ",
        "out.writeln(Tree(\"a\").name()); 0; }");
    own_stage(source, None, ir::verify::VerifyMode::Full).unwrap();
}

/// #780: the same for the check that a valued `for` ran at least once, inside `try`. Tomato prints `6`, then
/// fails with "A void result cannot be used as a value" (a trap, which `catch` does not take).
#[test]
fn oir2_iteration_check_inside_try_continues_with_the_checked_values() {
    let source = concat!("module app; import \"io/stdout\" take Stdout; ",
        "object Probe { Probe(); last(items is @int) is int { let out = Stdout(); ",
        "try { for items -> i { i * 3; } } catch e is Error { out.writeln(\"caught\"); 7; } } } ",
        "main() is int { let empty is @int = []; Stdout().writeln(Probe().last([1, 2]).string()); ",
        "Probe().last(empty); }");
    own_stage(source, None, ir::verify::VerifyMode::Full).unwrap();
}

/// The fields of an early self that are still unbound enter its incomplete `obj.new` as `undef`s defined in the
/// allocation's own block (#780). A2 counts only a directly defined `undef` as unbound; the constructor's
/// prologue `undef` would reach a later allocation through block parameters once own splits the body at a
/// resource binding (here the `Atomic` parameter), and the later `obj.init` would read as a second binding.
#[test]
fn oir2_early_self_takes_its_unbound_fields_as_adjacent_undefs() {
    let source = concat!("module app; import \"async/atomic\" take Atomic; ",
        "object Tree { has name is string; has marks is @int; ",
        "Tree(name is string, tags is @int, sink is Atomic<Maybe<Tree>>) { self.name = name; ",
        "marks = tags.map() -> tag { sink.replace(Maybe.some(self)); tag * 2; }; } ",
        "size() is int { marks.size(); } } ",
        "main() is int { let sink is Atomic<Maybe<Tree>> = Atomic(Maybe.none); ",
        "Tree(\"a\", [1, 2], sink).size(); }");
    let module = high(source, None).unwrap();
    let mut incomplete = 0;
    for (_, function) in module.functions.iter() {
        let Some(body) = function.body.as_ref() else { continue };
        for (block_id, block) in body.blocks.iter() {
            for &id in &block.insts {
                let op = &body.insts.get(id).unwrap().op;
                let ir::Operation::Core(ir::CoreOp::ObjNew { fields, construction, .. }) = op else {
                    continue;
                };
                if *construction != ir::ConstructionStatus::Incomplete {
                    continue;
                }
                incomplete += 1;
                let undefs: Vec<_> = fields.iter().filter_map(|&value| match body.values.get(value).unwrap().def {
                    ir::Definition::InstResult { inst, .. } if matches!(body.insts.get(inst).unwrap().op,
                        ir::Operation::Core(ir::CoreOp::Undef { .. })) => Some(inst),
                    _ => None,
                }).collect();
                // `marks` and `$pending`.
                assert_eq!(undefs.len(), 2, "{}", function.symbol);
                assert!(undefs.iter().all(|inst| block.insts.contains(inst)), "{}: {block_id:?}", function.symbol);
            }
        }
    }
    assert_eq!(incomplete, 1);
    own_stage(source, None, ir::verify::VerifyMode::Full).unwrap();
}

/// #781: an observation entry (`pick`, whose root scope owns nothing) returns a managed value that cannot own
/// from inside its loop, whose role call keeps the loop scope. It returns only its declared result: own closes
/// the loop scope without a reserve to commit to the caller, whose packet the entry would return (T2).
#[test]
fn oir2_observation_entry_returns_a_managed_result_from_a_retained_scope() {
    let source = concat!("module app; role Gen { value() is int; } ",
        "object Base does Gen { has n is int; Base(:n); value() is int { n; } } ",
        "object Wrapped does Gen { has origin is Gen; has condition is (int) is bool; ",
        "Wrapped(:origin, :condition); value() is int { origin.value(); } } ",
        "object Picker { has origin is Gen; has condition is (int) is bool; has attempts is int; ",
        "Picker(:origin, :condition, :attempts); ",
        "pick() is Gen { for (0..attempts) -> index { let sample = origin; ",
        "if condition(sample.value() + index) { return Wrapped(origin: sample, condition: condition); } } ",
        "Base(n: 0); } } ",
        "main() is int { let p = Picker(origin: Base(n: 3), condition: (n is int) is bool { n % 2 == 0; }, ",
        "attempts: 5); p.pick().value(); }");
    let low = own_to_low(source, None, ir::verify::VerifyMode::Structural).unwrap();
    let (_, pick) = low.functions.iter().find(|(_, function)| function.symbol.starts_with("app.Picker.pick"))
        .unwrap();
    assert_eq!(pick.signature.ownership_abi, ir::OwnershipAbi::Observation);
    assert_eq!(pick.signature.results.len(), 1);
}

/// #777 (full verification): a scope that owns a holder of an action value whose closure captures a resource
/// binds the function type's dictionary (`type.of`), through which its cleanup reaches the closure, and its
/// envelope counts what the captured resource's destroy may do. The verifier finds the dictionary in the
/// owner's function and agrees (B104, B112), whether the holder comes back from a call or is built in the
/// scope of the resource.
#[test]
fn oir2_holder_of_a_resource_closure_binds_the_closure_dictionary() {
    let common = concat!("module app; import \"io/stdout\" take Stdout; ",
        "object Res { has n is string; Res(:n); size() is int { n.length(); } ",
        "destroy { Stdout().writeln(\"destroy \" + n); } } ",
        "object Filter { has keep is (int) is bool; Filter(:keep); passes(value is int) is bool { keep(value); } } ");
    let returned = format!("{common}{}", concat!("object Sets { Sets(); sized(name is string) is Filter { ",
        "let r = Res(n: name); Filter(keep: (value is int) is bool { value > r.size(); }); } } ",
        "main() is int { { let f = Sets().sized(\"abc\"); Stdout().writeln(f.passes(5).string()); } 0; }"));
    let local = format!("{common}{}", concat!("main() is int { { let r = Res(n: \"abc\"); ",
        "let f = Filter(keep: (v is int) is bool { v > r.size(); }); Stdout().writeln(f.passes(5).string()); } ",
        "0; }"));
    for source in [returned, local] {
        let (module, _) = own_stage(&source, None, ir::verify::VerifyMode::Full).unwrap_or_else(|error| {
            panic!("{error}\n{source}")
        });
        let main = module.functions.get(module.entry.unwrap()).unwrap().body.as_ref().unwrap();
        assert!(main.insts.iter().any(|(_, inst)| matches!(&inst.op, ir::Operation::Core(ir::CoreOp::TypeOf { ty, .. })
            if matches!(module.types.get(*ty), Ok(ir::Type::Fn(_))))), "{source}");
    }
}

/// #786: an explicit entry that accepts a holder of action values binds the function type's dictionary even
/// when the closures can own nothing: the verifier counts the accepted input's cleanup through it (B104, B112).
#[test]
fn oir2_explicit_entry_binds_the_closure_dictionary_of_an_accepted_holder() {
    let source = concat!("module app; role Gen { value() is int; } ",
        "object Base does Gen { has n is int; Base(:n); value() is int { n; } } ",
        "object Wrapped does Gen { has origin is Gen; has condition is (int) is bool; ",
        "Wrapped(:origin, :condition); value() is int { origin.value(); } } ",
        "object Picker { has origin is Gen; has condition is (int) is bool; has attempts is int; ",
        "Picker(:origin, :condition, :attempts); ",
        "pick() is Gen { for (0..attempts) -> index { let sample = origin; if condition(sample.value() + index) { ",
        "return Wrapped(origin: Base(n: index), condition: (v is int) is bool { v > 0; }); } } Base(n: 0); } } ",
        "main() is int { let p = Picker(origin: Base(n: 3), condition: (n is int) is bool { n % 2 == 0; }, ",
        "attempts: 5); p.pick().value(); }");
    let (module, _) = own_stage(source, None, ir::verify::VerifyMode::Structural).unwrap();
    let (_, value) = module.functions.iter().find(|(_, function)| function.symbol.starts_with("app.Wrapped.value"))
        .unwrap();
    assert!(matches!(value.signature.ownership_abi, ir::OwnershipAbi::Explicit { .. }));
    let body = value.body.as_ref().unwrap();
    assert!(body.insts.iter().any(|(_, inst)| matches!(&inst.op, ir::Operation::Core(ir::CoreOp::TypeOf { ty, .. })
        if matches!(module.types.get(*ty), Ok(ir::Type::Fn(_))))));
}
