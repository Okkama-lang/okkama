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

/// The Low module; `Err` names the stage that stopped.
pub(super) fn own_to_low(source: &str, case: Option<PathBuf>, mode: ir::verify::VerifyMode)
    -> Result<ir::Module, String> {
    let (own, verify) = own_stage(source, case, mode)?;
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
    let module = high(source, case)?;
    let mut verify = ir::verify::VerifyOptions { mode, ..ir::verify::VerifyOptions::default() };
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
fn oir2_own_managed_builder_rehomes_fresh_elements_to_the_outer_scope() {
    let low = full(concat!(
        "module app; object Res { has id is int; Res(:id); destroy { } } ",
        "main() is int { let a = (0..3).map() -> i { Res(id: i); }; a.size(); }",
    ));
    assert!(operations(&low).iter().any(|op| matches!(op, ir::Operation::Ownership(
        ir::OwnershipOp::ScopeTransfer { states: ir::TransferStates::Between { .. }, .. }))));
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
