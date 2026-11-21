//! #769 (owner's decision of 2026-11-21): the builtin array adapters follow the error contracts of the roles
//! they play. `Cursor.item()`/`successor()` at the end throw `IterationEnded.ended`, `Sequencing.item(index)`
//! outside `0..size()` throws `OutOfRange.because(reason)`, `first()`/`last()` of an empty sequence throw
//! `EmptySequence.empty`. Direct array operations (`values[i]`) keep their program errors.
#[path = "sparrow_support/mod.rs"]
mod support;
use okkama_compiler::{SourceFile, ir, lowering, syntax};
use std::path::PathBuf;

struct Stdlib(PathBuf);
impl lowering::ModuleLoader for Stdlib {
    fn load(&mut self, path: &str, _importer: &str) -> Result<Option<lowering::LoadedModule>, String> {
        let file = self.0.join(format!("{path}.ok"));
        if !file.is_file() { return Ok(None); }
        let text = std::fs::read_to_string(&file).map_err(|error| error.to_string())?;
        let id = file.to_string_lossy().into_owned();
        let source = SourceFile::new(id.clone(), text).unwrap();
        let program = syntax::parse(&source, &syntax::ParseOptions::default()).unwrap();
        Ok(Some(lowering::LoadedModule { id, source, program: syntax::prune::strip_test_items(program) }))
    }
}

/// `body` runs in `async main`; the program imports the roles but none of the error enums, except through
/// `IMPORTS` in tests that catch them by name.
fn ending(imports: &str, body: &str) -> support::Ending {
    let text = format!(r#"
        module app v1.0.0;
        import "collections/sequence" take Sequencing;
        import "collections/iteration" take Cursor, Iterating;
        import "async/atomic" take Atomic;
        {imports}
        role Named {{ name() is string; }}
        object Tag does Named {{ has label is string; Tag(:label); name() is string {{ label; }} }}
        object Res {{
            has log is Atomic<string>;
            Res(:log);
            destroy {{ log.replace(log.value() + "R"); }}
        }}
        object Probe {{
            Probe();
            item(s is Sequencing<int>, i is int) is string {{ try {{ s.item(i).string(); }} catch e is Error {{ e.text(); }} }}
            first(s is Sequencing<int>) is string {{ try {{ s.first().string(); }} catch e is Error {{ e.text(); }} }}
            last(s is Sequencing<int>) is string {{ try {{ s.last().string(); }} catch e is Error {{ e.text(); }} }}
            at(c is Cursor<int>) is string {{ try {{ c.item().string(); }} catch e is Error {{ e.text(); }} }}
            next(c is Cursor<int>) is string {{ try {{ c.successor(); "moved"; }} catch e is Error {{ e.text(); }} }}
            seen(items is Iterating<int>) is string {{ let log = Atomic(""); for items -> item {{ log.replace(log.value() + item.string()); }} log.value(); }}
            raise(s is Sequencing<int>, i is int) is int {{ s.item(i); }}
        }}
        object Generic<T> {{
            Generic();
            head(s is Sequencing<T>) is T {{ s.first(); }}
        }}
        async main() is int {{
            let p = Probe();
            {body}
            0;
        }}
    "#);
    let source = SourceFile::new("array-adapters.ok", text).unwrap();
    let program = syntax::parse(&source, &syntax::ParseOptions::default()).unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../std");
    let module = lowering::lower_linked(&program, source.name(), &mut Stdlib(root)).unwrap();
    ir::verify(&module).unwrap();
    support::sparrow(&module)
}

fn check(body: &str) {
    assert_eq!(ending("", body), support::Ending::Value(0));
}

const OUT: &str = r#"OutOfRange.because("index outside the sequence")"#;

#[test]
fn sequencing_item_throws_out_of_range_outside_its_size() {
    check(&format!(r#"
        let values is Sequencing<int> = [10, 20, 30];
        let empty is Sequencing<int> = [];
        confirm p.item(values, 0) == "10";
        confirm p.item(values, 2) == "30";
        confirm p.item(values, -1) == {OUT:?};
        confirm p.item(values, 3) == {OUT:?};
        confirm p.item(values, 4) == {OUT:?};
        confirm p.item(empty, 0) == {OUT:?};
    "#));
}

#[test]
fn sequencing_first_and_last_throw_empty_sequence_when_empty() {
    check(r#"
        let one is Sequencing<int> = [7];
        let many is Sequencing<int> = [1, 2, 3];
        let empty is Sequencing<int> = [];
        confirm p.first(one) == "7" && p.last(one) == "7";
        confirm p.first(many) == "1" && p.last(many) == "3";
        confirm p.first(empty) == "EmptySequence.empty";
        confirm p.last(empty) == "EmptySequence.empty";
    "#);
}

#[test]
fn cursor_ends_with_iteration_ended() {
    check(r#"
        let values is Sequencing<int> = [10, 20];
        let empty is Sequencing<int> = [];
        let start = values.cursor();
        confirm start.available();
        confirm p.at(start) == "10";
        let second = start.successor();
        confirm p.at(start) == "10";
        confirm p.at(second) == "20";
        let end = second.successor();
        confirm !end.available();
        confirm p.at(end) == "IterationEnded.ended";
        confirm p.next(end) == "IterationEnded.ended";
        let nothing = empty.cursor();
        confirm !nothing.available();
        confirm p.at(nothing) == "IterationEnded.ended";
        confirm p.next(nothing) == "IterationEnded.ended";
    "#);
}

#[test]
fn adapter_errors_are_caught_by_their_type_and_skip_other_catches() {
    let imports = r#"
        import "collections/sequence" take EmptySequence;
        import "collections/iteration" take IterationEnded;
        import "error" take OutOfRange;
    "#;
    let body = r#"
        let values is Sequencing<int> = [10, 20];
        let empty is Sequencing<int> = [];
        let a = try { empty.first(); } catch e is EmptySequence { -1; };
        confirm a == -1;
        let b = try { values.item(100); } catch e is OutOfRange { -2; };
        confirm b == -2;
        let c = try { empty.cursor().item(); } catch e is IterationEnded { -3; };
        confirm c == -3;
        let d = try { try { values.item(5); } catch e is EmptySequence { -4; }; } catch e is OutOfRange { -5; };
        confirm d == -5;
        let f = try { p.raise(values, 9); } catch OutOfRange.because(reason) { confirm reason == "index outside the sequence"; -6; };
        confirm f == -6;
    "#;
    assert_eq!(ending(imports, body), support::Ending::Value(0));
}

#[test]
fn an_uncaught_adapter_error_reaches_the_caller_and_unwinds_resources() {
    check(r#"
        let log = Atomic("");
        let values is Sequencing<int> = [1];
        let caught = try {
            let held = Res(log: log);
            p.raise(values, 3).string();
        } catch e is Error { confirm log.value() == "R"; e.text(); };
        confirm caught == "OutOfRange.because(\"index outside the sequence\")";
    "#);
    match ending("", r#"let values is Sequencing<int> = []; p.raise(values, 0);"#) {
        support::Ending::Failure { category, message } => {
            assert_eq!(category, "uncaught-error");
            assert!(message.contains("OutOfRange"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn adapters_cover_iterating_roles_nested_and_generic_arrays() {
    check(r#"
        confirm p.seen([1, 2, 3]) == "123";
        confirm p.seen([]) == "";
        let tags is Sequencing<Named> = [Tag(label: "a"), Tag(label: "b")];
        confirm tags.last().name() == "b";
        let rows is Sequencing<Sequencing<int>> = [[1, 2], [], [3]];
        confirm rows.item(0).last() == 2;
        let middle = try { rows.item(1).first(); } catch e is Error { -1; };
        confirm middle == -1;
        let ints is Generic<int> = Generic();
        confirm ints.head([5, 6]) == 5;
        let strings is Generic<string> = Generic();
        let nothing = try { strings.head([]); } catch e is Error { e.text(); };
        confirm nothing == "EmptySequence.empty";
    "#);
}

#[test]
fn direct_array_operations_keep_their_program_errors() {
    check(r#"
        let raw = [1, 2, 3];
        confirm raw.reduce(0) -> sum, x { sum + x * 2; } == 12;
        confirm raw.reduce(0) -> count, x { if x > 1 { count + 1; } else { count; } } == 2;
        confirm p.seen(raw) == "123";
    "#);
    match ending("", r#"let raw = [1, 2]; let x = raw[5];"#) {
        support::Ending::Failure { category, message } => {
            assert_eq!(category, "runtime", "{message}");
            assert!(message.contains("Array index out of bounds"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}
