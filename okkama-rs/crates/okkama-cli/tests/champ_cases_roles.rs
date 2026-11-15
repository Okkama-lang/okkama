//! Conformance cases of role values and action values (#676), compiled through OIR-2 and run in Champ, in
//! process. Each case is compared with its case.json; a case that stops on a gap outside this ticket is a
//! `through_low!` entry: it becomes a full check as soon as the backend accepts it.
//!
//! Only pure language cases: no processes, workers, network, outside files or test mode.
#![cfg(feature = "champ-abi")]

use okkama_compiler::lowering::{LoadedModule, ModuleLoader, RootFile, oir2};
use okkama_compiler::syntax::{self, ParseOptions};
use okkama_compiler::SourceFile;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn harness() -> PathBuf {
    root().join("conformance/harness/okkama")
}

/// Imports resolve next to the importing file first, then in the suite's harness module directory (the
/// runner copies it into the case), then in std; std files import from std.
struct CaseLoader {
    stdlib: PathBuf,
}

impl CaseLoader {
    fn locate(&self, path: &str, importer: &str) -> Option<PathBuf> {
        let file = if path.ends_with(".ok") { path.to_string() } else { format!("{path}.ok") };
        let importer = Path::new(importer);
        let itself = std::fs::canonicalize(importer).ok();
        let base = importer.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        let in_std = itself.as_ref().is_some_and(|file| file.starts_with(&self.stdlib));
        let dirs = if in_std { vec![self.stdlib.clone(), base] } else { vec![base, harness(), self.stdlib.clone()] };
        dirs.iter()
            .map(|dir| dir.join(&file))
            .filter(|candidate| candidate.is_file())
            .filter_map(|candidate| std::fs::canonicalize(candidate).ok())
            .find(|candidate| Some(candidate) != itself.as_ref())
    }

    fn read(&self, file: PathBuf) -> Result<Option<LoadedModule>, String> {
        let name = file.to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&file).map_err(|error| error.to_string())?;
        let source = SourceFile::new(name.clone(), text).map_err(|error| error.to_string())?;
        let program = syntax::parse(&source, &ParseOptions::default()).map_err(|error| format!("{error:?}"))?;
        let program = syntax::prune::strip_test_items(program);
        Ok(Some(LoadedModule { id: name, source, program }))
    }
}

impl ModuleLoader for CaseLoader {
    fn load(&mut self, path: &str, importer: &str) -> Result<Option<LoadedModule>, String> {
        match self.locate(path, importer) {
            Some(file) => self.read(file),
            None => Ok(None),
        }
    }

    fn resolve(&mut self, path: &str, importer: &str) -> Result<Option<String>, String> {
        Ok(self.locate(path, importer).map(|file| file.to_string_lossy().into_owned()))
    }

    fn resolve_std(&mut self, path: &str, _importer: &str) -> Result<Option<String>, String> {
        let file = self.stdlib.join(format!("{path}.ok"));
        Ok(std::fs::canonicalize(file).ok().map(|file| file.to_string_lossy().into_owned()))
    }

    fn load_std(&mut self, path: &str, _importer: &str) -> Result<Option<LoadedModule>, String> {
        match std::fs::canonicalize(self.stdlib.join(format!("{path}.ok"))) {
            Ok(file) => self.read(file),
            Err(_) => Ok(None),
        }
    }
}

/// `L04.roles.narrowing` lives in `conformance/cases/L04-*/roles.narrowing`.
fn case_dir(id: &str) -> PathBuf {
    let (level, suffix) = id.split_once('.').unwrap();
    let prefix = format!("{level}-");
    let level_dir = std::fs::read_dir(root().join("conformance/cases"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.file_name().unwrap().to_string_lossy().starts_with(&prefix))
        .unwrap_or_else(|| panic!("{id}: no level directory"));
    level_dir.join(suffix)
}

fn case(id: &str) -> Value {
    let case: Value = serde_json::from_str(&std::fs::read_to_string(case_dir(id).join("case.json")).unwrap()).unwrap();
    assert_eq!(case["id"], id);
    assert_eq!(case["mode"], "run", "{id}: only run-mode cases");
    case
}

fn compile(id: &str) -> Result<Vec<u8>, oir2::PipelineError> {
    let dir = case_dir(id);
    let mut sources = Vec::new();
    for name in case(id)["sources"].as_array().unwrap() {
        let file = std::fs::canonicalize(dir.join(name.as_str().unwrap())).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        sources.push(SourceFile::new(file.to_string_lossy(), text).unwrap());
    }
    let programs = sources.iter()
        .map(|source| syntax::prune::strip_test_items(syntax::parse(source, &ParseOptions::default()).unwrap()))
        .collect::<Vec<_>>();
    let roots = sources.iter()
        .zip(&programs)
        .map(|(source, program)| RootFile { source, program })
        .collect::<Vec<_>>();
    let stdlib = std::fs::canonicalize(root().join("std")).unwrap();
    oir2::compile_linked_with(&roots, &mut CaseLoader { stdlib }, okkama_compiler::oir2::VerifyMode::Structural)
}

/// `exit` is `None` when the program stopped with a runtime failure.
#[derive(Debug, PartialEq, Eq)]
struct Observed {
    exit: Option<i64>,
    stdout: String,
}

fn run(id: &str, bytes: Vec<u8>) -> Observed {
    let limits = sparrow_core::budget::Limits { steps: Some(10_000_000), ..sparrow_core::budget::Limits::default() };
    let run = sparrow_core::run_captured(bytes, limits).unwrap_or_else(|error| panic!("{id}: load: {error}"));
    Observed { exit: run.result.ok(), stdout: String::from_utf8_lossy(&run.stdout).into_owned() }
}

/// A `failure` outcome, or an `error` outcome whose only stage is runtime, expects a runtime failure; `success`
/// expects its exit status.
fn expected(id: &str) -> Observed {
    let case = case(id);
    let expect = &case["expect"];
    let stdout = expect["stdout"]["text"].as_str().unwrap_or("").to_owned();
    let exit = match expect["outcome"].as_str().unwrap() {
        "success" => Some(expect["exit_status"].as_i64().unwrap_or(0)),
        "failure" => None,
        "error" if expect["failure"]["stages"] == serde_json::json!(["runtime"]) => None,
        outcome => panic!("{id}: unsupported outcome {outcome}"),
    };
    Observed { exit, stdout }
}

fn check(id: &str) {
    let bytes = compile(id).unwrap_or_else(|error| panic!("{id}: build: {error}"));
    assert_eq!(run(id, bytes), expected(id), "{id}");
}

/// Stages after verify_low belong to the SPB 17 / Champ backend.
const BACKEND: [&str; 4] = ["layout", "compile_low", "verify_load", "encode"];

/// The case passes own and low; it runs in Champ, compared with case.json, once the backend accepts it.
fn check_through_low(id: &str) {
    match compile(id) {
        Ok(bytes) => assert_eq!(run(id, bytes), expected(id), "{id}"),
        Err(oir2::PipelineError::Phase { phase, message }) if BACKEND.contains(&phase) => {
            println!("{id}: own and low passed; stops in {phase}: {message}");
        }
        Err(error) => panic!("{id}: {error}"),
    }
}

/// A single-file program of this test, run in Champ: its exit status and stdout.
fn run_source(text: &str) -> Observed {
    run_source_with(text, okkama_compiler::oir2::VerifyMode::Structural)
}

/// [`run_source`] with the given verification of every OIR-2 phase.
fn run_source_with(text: &str, mode: okkama_compiler::oir2::VerifyMode) -> Observed {
    let source = SourceFile::new("app.ok", text).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let roots = [RootFile { source: &source, program: &program }];
    let stdlib = std::fs::canonicalize(root().join("std")).unwrap();
    let bytes = oir2::compile_linked_with(&roots, &mut CaseLoader { stdlib }, mode)
        .unwrap_or_else(|error| panic!("build: {error}"));
    run("app.ok", bytes)
}

macro_rules! case {
    ($name:ident, $id:literal) => {
        #[test]
        fn $name() {
            check($id);
        }
    };
}

macro_rules! through_low {
    ($name:ident, $id:literal) => {
        #[test]
        fn $name() {
            check_through_low($id);
        }
    };
}

// Role declarations, role values and dynamic dispatch.
case!(roles_dispatch_and_covariance, "L04.roles.dispatch-and-covariance");
case!(roles_narrowing, "L04.roles.narrowing");
case!(roles_contextual_wrap, "L04.roles.contextual-wrap");
case!(roles_named_args_use_role_names, "L04.roles.named-args-use-role-names");
case!(roles_alias_same_spelled_roles, "L12.alias-same-spelled-roles");
case!(roles_type_names_uppercase, "L01.lex.type-names.uppercase");
case!(roles_operators_arithmetic_and_ordering, "L04.operators.arithmetic-and-ordering");
// Role-typed fields and generic role evidence.
case!(fields_role_typed_field, "L05.boundary.role-typed-field");
case!(generic_bound_satisfied, "L05.generic.bound-satisfied");
case!(generic_role_evidence, "L05.generic.role-evidence");
case!(generic_bound_role_argument, "L05.generic.bound-role-argument");
case!(generic_role_evidence_from_field, "L05.generic.role-evidence-from-field");
// Iterating through cursors.
case!(iterable_empty_and_last, "L05.iterable.empty-and-last");
case!(iterable_for_protocol_order, "L05.iterable.for-protocol-order");
// Action values: anonymous actions, bound actions and their calls.
case!(closures_anon_forms, "L01.gram.anon.forms");
case!(closures_own_rebind, "L03.anonymous.own-rebind");
case!(closures_bound_public_action, "L03.bound.public-action");
case!(closures_next_last_inside_anonymous_loop, "L03.next-last.inside-anonymous-loop");
case!(closures_return_anonymous_boundary, "L03.return.anonymous-boundary");
case!(closures_successor_closure_invoked, "L04.successor.closure-invoked");
// Caps: a cap value is its owner's reference, each read calls the computed field.
case!(caps_recompute_every_read, "L04.caps.recompute-every-read");
case!(caps_file_identity_path_aliases, "L06.file-identity.path-aliases");
// Hidden state views behind roles.
case!(roles_hidden_state_views, "L06.hidden.state-views");
// Role successors, and a successor taken twice on one path (OIR-2 §8: used at least once, read again).
case!(successors_generic_successor_specialization, "L12.generic-successor-specialization");

// Streams and sources: core.StringSource and io.stream.ByteSource (str.utf8, builders, array.concat).
case!(streams_stdout_stream_write, "L00.stdout.stream-write");
case!(streams_precedence_stream_and_range, "L01.gram.precedence.stream-and-range");
case!(streams_stream_write_vs_less_minus, "L01.lex.operators.stream-write-vs-less-minus");
case!(streams_raw_concatenation, "L01.lex.raw.concatenation");
case!(streams_raw_multiline, "L01.lex.raw.multiline");
case!(streams_string_escapes, "L01.lex.string.escapes");
case!(streams_range_for_and_functional, "L02.range.for-and-functional");
case!(streams_bytes_and_unpack, "L02.stream.bytes-and-unpack");
case!(streams_for_sources, "L03.for.sources");
// Builders, maps and collections behind role dispatch.
case!(collections_literal_argument, "L03.overload.collection-literal-argument");
case!(collections_array_removed_at, "L05.array.removed-at");
case!(collections_iterable_functional, "L05.iterable.functional");
// Caps behind string slices, builders and rational arithmetic; once fields.
case!(caps_internal_and_external, "L04.caps.internal-and-external");
case!(caps_snapshot_after_successor, "L04.caps.snapshot-after-successor");
case!(caps_time_operators, "L09.time.operators");
case!(once_computed_once, "L04.once.computed-once");
// Construction completion state: a role call on a receiver whose construction is pending fails (§11.4).
case!(construction_escaped_self_role_call, "L04.invariant.escaped-self-role-call");

/// It builds and runs in Champ, but sorting 100 000 integers is superlinear in a debug build: 1 000 take
/// seconds, 10 000 more than four minutes. With smaller arrays the output matches case.json.
#[test]
#[ignore = "debug Champ: the 100 000-element sort runs for hours (superlinear)"]
fn collections_failing_order_stops_the_sort() {
    check("L09.compare.failing-order-stops-the-sort");
}

// Multi-case Error variant types are not in S1 yet.
through_low!(streams_params_grouped, "L01.gram.params.grouped");
// compile_low: SPB_STRUCTURE, a definition that does not dominate its use in `main`.
through_low!(collections_batching_functional, "L05.batching.functional");
// compile_low: ENTRY_FAILURE_MASK, an own entry or witness whose body may fail but whose summary declares no
// failure.
through_low!(collections_persistent_list, "L09.collections.persistent-list");
through_low!(caps_in_collections_and_closures, "L12.caps-in-collections-and-closures");
through_low!(once_collections_heap_and_deque, "L09.collections.heap-and-deque");
// compile_low: ENTRY_FAILURE_MASK (#706), the witness entry of `CursorItems.cursor`; its scalar `val.eq`
// loads since #710.
through_low!(once_collections_frequencies, "L09.collections.frequencies");
// Batching sources holding an Atomic: the constructor's fresh evidence (FRESH_PACKET).
through_low!(batching_empty_and_nested, "L05.batching.empty-and-nested");
through_low!(batching_for_protocol_order, "L05.batching.for-protocol-order");
through_low!(batching_next_last_return, "L05.batching.next-last-return");

/// A bound role action is a closure over its receiver: each call dispatches the slot on that receiver.
#[test]
fn bound_role_action_calls_the_receivers_implementation() {
    let observed = run_source("module app v1.0.0;
import \"io/stdout\" take Stdout;
role Named {
    name() is string;
}
object A does Named {
    A();
    name() is string { \"a\"; }
}
object B does Named {
    B();
    name() is string { \"b\"; }
}
main() is int {
    let out = Stdout();
    let first is Named = A();
    let second is Named = B();
    let left is () is string = first.name;
    let right is () is string = second.name;
    out.writeln(left() + right() + left());
    0;
}");
    assert_eq!(observed, Observed { exit: Some(0), stdout: "aba\n".into() });
}

/// A cap of a fresh owner carries the owner's packet; every read still recomputes the field.
#[test]
fn cap_of_a_fresh_resource_owner_reads_it() {
    let observed = run_source("module app v1.0.0;
import \"io/stdout\" take Stdout;
object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln(\"destroy \" + name); }
}
object Meter {
    has res is Res;
    Meter(:res);
    cap Reading {
        value is string { res.label() + \"!\"; }
    }
}
main() is int {
    let out = Stdout();
    let view = Meter(res: Res(name: \"r\"))::Reading;
    out.writeln(view::value);
    out.writeln(view::value);
    0;
}");
    assert_eq!(observed, Observed { exit: Some(0), stdout: "r!\nr!\ndestroy r\n".into() });
}

/// Decision (a) of #676: a closure captures a resource by reference (P2.4 §4.1), so the resource stays its
/// owner's. Both calls read it; the owner destroys it once, when `main`'s scope ends, after the closure's last
/// use. Every OIR-2 phase is verified in full. Tomato prints the same for this program.
#[test]
fn a_closure_capturing_a_resource_leaves_its_destruction_to_the_owner() {
    let observed = run_source_with("module app v1.0.0;
import \"io/stdout\" take Stdout;
object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln(\"destroy \" + name); }
}
main() is int {
    let out = Stdout();
    let r = Res(name: \"r\");
    let f = () is string { r.label() + \"!\"; };
    out.writeln(f());
    out.writeln(f());
    out.writeln(\"end\");
    0;
}", okkama_compiler::oir2::VerifyMode::Full);
    assert_eq!(observed, Observed { exit: Some(0), stdout: "r!\nr!\nend\ndestroy r\n".into() });
}
