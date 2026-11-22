//! #790 (owner's decision of 2026-11-21): a successor Future (`Future<^C>`) is handed on where `Future<C>` is
//! expected: an argument, a field, a declared result, a generic holder. Each accepted form lowers to verified
//! OIR (`future.handoff`) and runs on Sparrow.
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

/// `main`'s value for a program whose `main` body is `body`; the OIR is verified first.
fn run(body: &str) -> support::Ending {
    let module = lower(body);
    ir::verify(&module).unwrap();
    support::sparrow(&module)
}

fn lower(body: &str) -> ir::Module {
    let text = format!(r#"
        module app v1.0.0;
        object Counter {{
            has n is int;
            Counter(:n);
            async bumped() is ^Counter {{ ^n = n + 1; }}
            n() is int {{ n; }}
        }}
        role Counting {{ async step() is ^Counting; count() is int; }}
        object Tally does Counting {{
            has c is int;
            Tally(:c);
            async step() is ^Tally {{ ^c = c + 10; }}
            count() is int {{ c; }}
        }}
        object Waiter {{
            Waiter();
            async take(f is Future<Counter>) is int {{ let c = await f; c.n(); }}
            async counted(f is Future<Counting>) is int {{ let c = await f; c.count(); }}
        }}
        object Holder {{ has f is Future<Counter>; Holder(:f); future() is Future<Counter> {{ f; }} }}
        object Keep<T> {{ has f is Future<T>; Keep(:f); future() is Future<T> {{ f; }} }}
        object Maker {{
            Maker();
            make() is Future<Counter> {{ Counter(n: 40).bumped(); }}
        }}
        async main() is int {{
            {body}
        }}
    "#);
    let source = SourceFile::new("future-handoff.ok", text).unwrap();
    let program = syntax::parse(&source, &syntax::ParseOptions::default()).unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../std");
    lowering::lower_linked(&program, source.name(), &mut Stdlib(root)).unwrap()
}

/// The verifier admits `future.handoff` only from `Future<^C>` to `Future<C>`: an operand that is already a
/// plain Future is refused.
#[test]
fn the_verifier_refuses_a_handoff_of_a_plain_future() {
    let mut module = lower("let pending = Counter(n: 1).bumped(); await Waiter().take(pending);");
    let mut found = false;
    for function in &mut module.functions {
        for block in &mut function.blocks {
            let Some(index) = block.insts.iter().position(|inst| matches!(inst.kind, ir::InstKind::FutureHandoff(_))) else {
                continue;
            };
            // Hand the already plain result on again.
            let plain = block.insts[index].result.expect("a handoff defines a value");
            let mut again = block.insts[index].clone();
            again.kind = ir::InstKind::FutureHandoff(plain);
            again.result = Some(ir::ValueId(function.values.len()));
            function.values.push(function.values[plain.0].clone());
            block.insts.insert(index + 1, again);
            found = true;
        }
    }
    assert!(found, "the argument handoff lowers to future.handoff");
    let error = ir::verify(&module).unwrap_err().to_string();
    assert!(error.contains("future.handoff"), "{error}");
}

#[test]
fn the_owners_example_hands_a_successor_future_to_an_argument() {
    assert_eq!(run("let pending = Counter(n: 1).bumped(); await Waiter().take(pending);"), support::Ending::Value(2));
}

#[test]
fn a_successor_future_goes_into_a_field_a_result_and_a_generic_holder() {
    assert_eq!(run("let held = Holder(f: Counter(n: 4).bumped()); let c = await held.future(); c.n();"), support::Ending::Value(5));
    assert_eq!(run("let f = Maker().make(); let c = await f; c.n();"), support::Ending::Value(41));
    assert_eq!(run("let kept = Keep(f: Counter(n: 7).bumped()); let c = await kept.future(); c.n();"), support::Ending::Value(8));
}

#[test]
fn typed_locals_aliases_and_blocks_hand_it_on() {
    assert_eq!(run("let pending is Future<Counter> = Counter(n: 2).bumped(); let c = await pending; c.n();"), support::Ending::Value(3));
    assert_eq!(run("let pending is Future<Counter> = Counter(n: 2).bumped(); await Waiter().take(pending);"), support::Ending::Value(3));
    assert_eq!(run("let pending = Counter(n: 5).bumped(); let alias = pending; await Waiter().take(alias);"), support::Ending::Value(6));
    assert_eq!(run("await Waiter().take(if 1 > 0 { Counter(n: 8).bumped(); } else { Counter(n: 0).bumped(); });"), support::Ending::Value(9));
}

#[test]
fn a_role_successor_future_hands_on_as_the_role() {
    assert_eq!(run("let r is Counting = Tally(c: 1); await Waiter().counted(r.step());"), support::Ending::Value(11));
}

#[test]
fn existing_forms_still_run() {
    assert_eq!(run("let task = spawn Counter(n: 3).bumped(); let c = await task; c.n();"), support::Ending::Value(4));
    assert_eq!(run("let all = [Counter(n: 6).bumped()]; let c = await all[0]; c.n();"), support::Ending::Value(7));
    assert_eq!(run("let pending = Counter(n: 1).bumped(); pending.cancel(); 0;"), support::Ending::Value(0));
}
