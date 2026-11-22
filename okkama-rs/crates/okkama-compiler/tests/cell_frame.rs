//! Cell-bearing actions stay synchronous (#957), independently of lowering.
use okkama_compiler::{
    SourceFile,
    analysis::{self, SourceModule},
    syntax::{self, ParseOptions},
};

fn restriction(message: &str) -> bool {
    message.contains("An action that retains a Cell cannot")
        || message.contains("A Cell cannot be hidden")
        || message.contains("A Cell cannot bind a generic")
        || message.contains("cannot hold a Cell") && message.contains("Case payload")
        || message.contains("A once let cannot hold a Cell")
}

fn rejected(code: &str) {
    let found = messages(code);
    assert!(
        found.iter().any(|m| restriction(m)),
        "expected Cell restriction, got {found:?}"
    );
}
fn accepted(code: &str) {
    let found = messages(code);
    assert!(found.is_empty(), "{found:?}");
}

fn messages(code: &str) -> Vec<String> {
    // Module identity, rather than the spelling of an application object, identifies Cell.
    let cell = SourceFile::new("cell.ok", "module cell v1.0.0; object Cell<T> { has n is T; Cell(value is T) { n = value; } value() is T { n; } replace(value is T) is void {} }").unwrap();
    let source = SourceFile::new(
        "test.ok",
        format!("module app v1.0.0; import \"cell\" take Cell; {code}"),
    )
    .unwrap();
    let cell = syntax::parse(&cell, &ParseOptions::default()).expect("Cell syntax");
    let program = syntax::parse(&source, &ParseOptions::default()).expect("test syntax");
    let modules = vec![
        SourceModule {
            path: "cell".into(),
            program: &cell,
            imports: vec![],
        },
        SourceModule {
            path: "app".into(),
            program: &program,
            imports: program.imports.iter().map(|i| (0, i)).collect(),
        },
    ];
    match analysis::analyze(modules) {
        Ok(_) => vec![],
        Err(errors) => errors.into_iter().map(|e| e.message).collect(),
    }
}

const FACTORY: &str = "object M { M(); mk() is () is int { let c = Cell(1); () is int { c.replace(c.value() + 1); c.value(); }; } async use(f is () is int) is int { f() + f(); } }";

#[test]
fn harvested_async_argument_from_sync_body() {
    rejected(&format!(
        "{FACTORY} {}",
        "main() is int { M().use(M().mk()); 0; }"
    ));
}

#[test]
fn harvested_async_bound_action() {
    rejected(&format!(
        "{FACTORY} {}",
        "main() is int { let use is async (() is int) is int = M().use; use(M().mk()); 0; }"
    ));
}

#[test]
fn harvested_transitive_capture() {
    rejected(&format!(
        "{FACTORY} {}",
        "object Wrap { Wrap(); wrap(f is () is int) is () is int { () is int { f(); }; } } main() is int { M().use(Wrap().wrap(M().mk())); 0; }"
    ));
}

#[test]
fn harvested_sync_forwarding_to_async() {
    rejected(&format!(
        "{FACTORY} {}",
        "object Send { Send(); send(f is () is int) is void { M().use(f); } } main() is int { Send().send(M().mk()); 0; }"
    ));
}

#[test]
fn harvested_shorthand_field() {
    rejected(&format!(
        "{FACTORY} {}",
        "object Hold { has f is () is int; Hold(:f); } main() is int { let h = Hold(f: M().mk()); 0; }"
    ));
}

#[test]
fn harvested_full_constructor_field() {
    rejected(&format!(
        "{FACTORY} {}",
        "object Hold { has f is () is int; Hold(f is () is int) { self.f = f; } } main() is int { let h = Hold(M().mk()); 0; }"
    ));
}

#[test]
fn harvested_field_default() {
    rejected(&format!(
        "{FACTORY} {}",
        "object Hold { has f is () is int = M().mk(); Hold(); } main() is int { 0; }"
    ));
}

#[test]
fn harvested_successor_field() {
    rejected(&format!(
        "{FACTORY} {}",
        "object Hold { has f is () is int; Hold(:f); put(f is () is int) is ^Hold { ^f = f; } } main() is int { let h = Hold(f: () is int { 1; }); h = h.put(M().mk()); 0; }"
    ));
}

#[test]
fn harvested_spawn_captured_action() {
    rejected(&format!(
        "{FACTORY} {}",
        "main() is int { let f = M().mk(); let task = spawn f(); 0; }"
    ));
}

#[test]
fn harvested_spawn_transitive_action() {
    rejected(&format!(
        "{FACTORY} {}",
        "main() is int { let f = M().mk(); let outer = () is int { f(); }; let task = spawn outer(); 0; }"
    ));
}

#[test]
fn harvested_async_result_and_await() {
    rejected(&format!(
        "{FACTORY} {}",
        "object W { W(); async make() is () is int { M().mk(); } } async main() is int { let f = await W().make(); f(); }"
    ));
}

#[test]
fn harvested_explicit_synchronous_return() {
    rejected(&format!(
        "{FACTORY} {}",
        "object W { W(); make() is () is int { return M().mk(); } } main() is int { M().use(W().make()); 0; }"
    ));
}

#[test]
fn harvested_branch_result() {
    rejected(&format!(
        "{FACTORY} {}",
        "main() is int { let f = if 1 > 0 { M().mk(); } else { () is int { 0; }; }; M().use(f); 0; }"
    ));
}

#[test]
fn harvested_array_element() {
    rejected(&format!(
        "{FACTORY} {}",
        "main() is int { let fs = [M().mk()]; M().use(fs[0]); 0; }"
    ));
}

#[test]
fn harvested_map_element() {
    rejected(&format!(
        "{FACTORY} {}",
        "main() is int { let fs = [\"f\": M().mk()]; M().use(fs[\"f\"]); 0; }"
    ));
}

#[test]
fn harvested_generic_forwarding() {
    rejected(&format!(
        "{FACTORY} {}",
        "object Id<T> { Id(); pass(f is T) is T { f; } } main() is int { let id is Id<() is int> = Id(); M().use(id.pass(M().mk())); 0; }"
    ));
}

#[test]
fn harvested_role_result() {
    rejected(&format!(
        "{FACTORY} {}",
        "role Maker { make() is () is int; } object W does Maker { W(); make() is () is int { M().mk(); } } main() is int { let maker is Maker = W(); M().use(maker.make()); 0; }"
    ));
}

#[test]
fn harvested_role_async_argument() {
    rejected(&format!(
        "{FACTORY} {}",
        "role Use { async use(f is () is int) is int; } object W does Use { W(); async use(f is () is int) is int { f(); } } main() is int { let w is Use = W(); w.use(M().mk()); 0; }"
    ));
}

#[test]
fn harvested_bound_cell_receiver() {
    rejected(&format!(
        "{FACTORY} {}",
        "object W { W(); make() is () is int { let c = Cell(1); c.value; } } main() is int { M().use(W().make()); 0; }"
    ));
}

#[test]
fn harvested_callback_returns_closure() {
    rejected(&format!(
        "{FACTORY} {}",
        "object W { W(); call(f is () is () is int) is () is int { f(); } } main() is int { let factory = () is () is int { M().mk(); }; M().use(W().call(factory)); 0; }"
    ));
}

#[test]
fn harvested_callback_forwards_to_async() {
    rejected(&format!(
        "{FACTORY} {}",
        "object W { W(); call(f is (() is int) is void, x is () is int) is void { f(x); } } main() is int { let send = (f is () is int) is void { M().use(f); }; W().call(send, M().mk()); 0; }"
    ));
}

#[test]
fn harvested_recursive_forwarding() {
    rejected(&format!(
        "{FACTORY} {}",
        "object W { W(); pass(f is () is int, n is int) is () is int { if n > 0 { self.pass(f, n - 1); } else { f; }; } } main() is int { M().use(W().pass(M().mk(), 2)); 0; }"
    ));
}

#[test]
fn harvested_functional_mapping() {
    rejected(&format!(
        "{FACTORY} {}",
        "main() is int { let fs = [M().mk()].map() -> f { f; }; M().use(fs[0]); 0; }"
    ));
}

#[test]
fn harvested_stored_plain_factory() {
    rejected(&format!(
        "{FACTORY} {}",
        "object Hold { has f is () is () is int; Hold(:f); make() is () is int { f(); } } main() is int { let h = Hold(f: () is () is int { M().mk(); }); M().use(h.make()); 0; }"
    ));
}

#[test]
fn harvested_cap_result() {
    rejected(&format!(
        "{FACTORY} {}",
        "object W { W(); cap Public { f is () is int { M().mk(); } } } main() is int { M().use(W()::Public::f); 0; }"
    ));
}

#[test]
fn harvested_loop_break_result() {
    rejected(&format!(
        "{FACTORY} {}",
        "main() is int { let f = loop { if 1 > 0 { last M().mk(); } next; }; M().use(f); 0; }"
    ));
}

#[test]
fn harvested_retained_once_binding() {
    rejected(&format!(
        "{FACTORY} {}",
        "object W { W(); make() is () is int { once let f = M().mk(); f; } } main() is int { 0; }"
    ));
}

#[test]
fn original_957() {
    rejected(&format!(
        "{FACTORY} async main() is int {{ let m = M(); let f = m.mk(); let a = m.use(f); let b = m.use(f); await a + await b; }}"
    ));
}
struct Std;

impl okkama_compiler::lowering::ModuleLoader for Std {
    fn load(
        &mut self,
        path: &str,
        _: &str,
    ) -> Result<Option<okkama_compiler::lowering::LoadedModule>, String> {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../std")
            .join(format!("{path}.ok"));
        let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
        let source = SourceFile::new(file.to_string_lossy().into_owned(), text).unwrap();
        let program = syntax::prune::strip_test_items(
            syntax::parse(&source, &ParseOptions::default())
                .map_err(|e| format!("{path}: {e:?}"))?,
        );
        Ok(Some(okkama_compiler::lowering::LoadedModule {
            id: source.name().into(),
            source,
            program,
        }))
    }

    fn resolve(&mut self, path: &str, _: &str) -> Result<Option<String>, String> {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../std")
            .join(format!("{path}.ok"));
        Ok(Some(file.to_string_lossy().into_owned()))
    }
}

fn std_messages(code: &str) -> Vec<String> {
    use okkama_compiler::lowering::{self, RootFile};
    let source = SourceFile::new("cell_capture.ok", format!("module app v1.0.0; {code}")).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).expect("test syntax");
    match lowering::check_linked_files(
        &[RootFile {
            program: &program,
            source: &source,
        }],
        &mut Std,
    ) {
        Ok(_) => vec![],
        Err(Ok(errors)) => errors
            .iter()
            .map(|e| {
                format!(
                    "{}: {}",
                    e.source.as_ref().map(|s| s.name()).unwrap_or("?"),
                    e.error.message
                )
            })
            .collect(),
        Err(Err(error)) => panic!("{error}"),
    }
}

#[test]
fn cell_capture_atomic_may_cross_every_boundary() {
    let found = std_messages(
        r#"
        import "cell" take Cell;
        import "async/atomic" take Atomic;
        object W {
            W();
            make() is () is int { let state = Atomic(1); () is int { state.replace(state.value() + 1); state.value(); }; }
            wrap(f is () is int) is () is int { () is int { f(); }; }
            async use(f is () is int) is int { f(); }
            async make_async() is () is int { self.make(); }
        }
        object Hold { has f is () is int; Hold(:f); call() is int { f(); } }
        async main() is int {
            let w = W();
            let f = w.wrap(w.make());
            let hold = Hold(f: f);
            let a = w.use(f);
            let b = spawn f();
            let c = await w.make_async();
            hold.call() + await a + await b + c();
        }
    "#,
    );
    assert!(found.is_empty(), "{found:#?}");
}
#[test]
fn cell_capture_std_users_remain_valid() {
    // All std modules importing Cell, found by the #957 source audit. This
    // checks bodies and their dependencies, without executing native code.
    let found = std_messages(
        r#"
        import "toml/parse"; import "config/ini"; import "mail/smtp_session";
        import "url/value"; import "http/cookie_date"; import "db/postgres/wire";
        import "db/postgres"; import "json"; import "i18n/gettext"; import "diff";
        main() is int { 0; }
    "#,
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn reject_direct_async_argument() {
    rejected(
        r#"object W { W(); async use(f is () is int) is int { f(); } } main() is int { let c = Cell(1); let f = () is int { c.value(); }; W().use(f); 0; }"#,
    );
}

#[test]
fn reject_sync_forwarder() {
    rejected(
        r#"object W { W(); forwarded(f is () is int) is () is int { f; } } main() is int { let c = Cell(1); let f = () is int { c.value(); }; let x = W().forwarded(f); x(); }"#,
    );
}

#[test]
fn reject_explicit_return() {
    rejected(
        r#"object W { W(); make() is () is int { let c = Cell(1); let f = () is int { c.value(); }; return f; } } main() is int { 0; }"#,
    );
}

#[test]
fn reject_nested_return() {
    rejected(
        r#"main() is int { let c = Cell(1); let f = () is int { c.value(); }; let g = () is () is int { f; }; 0; }"#,
    );
}

#[test]
fn reject_nested_spawn() {
    rejected(
        r#"main() is int { let c = Cell(1); let f = () is int { c.value(); }; let g = () is int { f(); }; spawn g(); 0; }"#,
    );
}

#[test]
fn reject_spawn_callee() {
    rejected(
        r#"main() is int { let c = Cell(1); let f = () is int { c.value(); }; spawn f(); 0; }"#,
    );
}

#[test]
fn reject_bound_receiver_return() {
    rejected(
        r#"object W { W(); make() is () is int { let c = Cell(1); c.value; } } main() is int { 0; }"#,
    );
}

#[test]
fn reject_bound_receiver_spawn() {
    rejected(r#"main() is int { let c = Cell(1); let f is () is int = c.value; spawn f(); 0; }"#);
}

#[test]
fn reject_action_any() {
    rejected(
        r#"main() is int { let c = Cell(1); let f = () is int { c.value(); }; let a is any = f; 0; }"#,
    );
}

#[test]
fn reject_cell_any() {
    rejected(r#"main() is int { let a is any = Cell(1); 0; }"#);
}

#[test]
fn reject_cell_role() {
    rejected(r#"role Read { value() is int; } main() is int { let r is Read = Cell(1); 0; }"#);
}

#[test]
fn reject_generic_inferred() {
    rejected(
        r#"object Box<T> { has x is T; Box(:x); } main() is int { let b = Box(x: Cell(1)); 0; }"#,
    );
}

#[test]
fn reject_generic_explicit() {
    rejected(
        r#"object Id<T> { Id(); pass(x is T) is T { x; } } main() is int { let id is Id<Cell<int>> = Id(); id.pass(Cell(1)); 0; }"#,
    );
}

#[test]
fn reject_generic_action_signature() {
    rejected(
        r#"object Box<T> { has f is () is T; Box(:f); } main() is int { let b = Box(f: () is Cell<int> { Cell(1); }); 0; }"#,
    );
}

#[test]
fn reject_variant_cell() {
    rejected(
        r#"enum Hidden = (held(value is Cell<int>)); main() is int { let h = Hidden.held(Cell(1)); 0; }"#,
    );
}

#[test]
fn reject_variant_generic() {
    rejected(
        r#"enum Hidden<T> = (held(value is T)); main() is int { let h = Hidden.held(Cell(1)); 0; }"#,
    );
}

#[test]
fn reject_variant_action() {
    rejected(
        r#"enum Hidden = (held(value is () is int)); main() is int { let c = Cell(1); let f = () is int { c.value(); }; let h = Hidden.held(f); 0; }"#,
    );
}

#[test]
fn reject_array() {
    rejected(
        r#"main() is int { let c = Cell(1); let f = () is int { c.value(); }; let a = [f]; 0; }"#,
    );
}

#[test]
fn reject_map() {
    rejected(
        r#"main() is int { let c = Cell(1); let f = () is int { c.value(); }; let a = ["f": f]; 0; }"#,
    );
}

#[test]
fn reject_spread() {
    rejected(
        r#"main() is int { let c = Cell(1); let f = () is int { c.value(); }; let fs = [f]...; 0; }"#,
    );
}

#[test]
fn reject_field() {
    rejected(
        r#"object W { has f is () is int; W(:f); } main() is int { let c = Cell(1); let f = () is int { c.value(); }; let w = W(f: f); 0; }"#,
    );
}

#[test]
fn reject_field_default() {
    rejected(
        r#"object W { has f is () is int = if true { let c = Cell(1); () is int { c.value(); }; } else { () is int { 0; }; }; W(); } main() is int { 0; }"#,
    );
}

#[test]
fn reject_if_alias() {
    rejected(
        r#"main() is int { let c = Cell(1); let f = () is int { c.value(); }; let g = if true { f; } else { () is int { 0; }; }; let a = [g]; 0; }"#,
    );
}

#[test]
fn reject_match_alias() {
    rejected(
        r#"main() is int { let c = Cell(1); let f = () is int { c.value(); }; let g = match 1 { case 1 { f; } else { () is int { 0; }; } }; let a = [g]; 0; }"#,
    );
}

#[test]
fn reject_block_tail_let() {
    rejected(
        r#"object W { W(); make() is () is int { let c = Cell(1); if true { let f = () is int { c.value(); }; } else { () is int { 0; }; }; } } main() is int { 0; }"#,
    );
}

#[test]
fn reject_functional_storage() {
    rejected(
        r#"main() is int { let c = Cell(1); let a = (0..2).map() -> i { () is int { c.value() + i; }; }; 0; }"#,
    );
}

#[test]
fn reject_loop_result() {
    rejected(
        r#"main() is int { let c = Cell(1); let f = () is int { c.value(); }; let g = loop { last f; }; 0; }"#,
    );
}

#[test]
fn reject_for_result() {
    rejected(
        r#"main() is int { let c = Cell(1); let f = () is int { c.value(); }; for 0..2 -> i { f; } 0; }"#,
    );
}

#[test]
fn allow_local_repeated_nested() {
    accepted(
        r#"main() is int { let c = Cell(1); let f = () is int { c.value(); }; let g = () is int { f() + f(); }; g() + f(); }"#,
    );
}

#[test]
fn allow_local_bound_receiver() {
    accepted(r#"main() is int { let c = Cell(1); let f is () is int = c.value; f() + f(); }"#);
}

#[test]
fn allow_functional_cell() {
    accepted(
        r#"main() is int { let c = Cell(0); for 0..3 -> i { c.replace(c.value() + i); } let a = (0..3).map() -> i { c.replace(i); c.value(); }; a[0]; }"#,
    );
}

#[test]
fn allow_plain_forwarding() {
    accepted(
        r#"object W { W(); forwarded(f is () is int) is () is int { f; } } main() is int { let n = 1; let f = () is int { n; }; W().forwarded(f)(); }"#,
    );
}

#[test]
fn allow_local_branch() {
    accepted(
        r#"main() is int { let c = Cell(1); let f = () is int { c.value(); }; let g = if true { f; } else { () is int { 0; }; }; g(); }"#,
    );
}

#[test]
fn allow_cell_collections() {
    accepted(
        r#"main() is int { let cs = [Cell(1)]; let ms = ["c": cs[0]]; let f = () is int { ms["c"].value(); }; f(); }"#,
    );
}

#[test]
fn allow_fresh_cell_factory() {
    accepted(
        r#"object W { W(); make() is () is Cell<int> { () is Cell<int> { Cell(1); }; } } main() is int { W().make()().value(); }"#,
    );
}

#[test]
fn reject_atomic() {
    let found = std_messages(
        r#"import "cell" take Cell; import "async/atomic" take Atomic; main() is int { let c = Cell(1); let f = () is int { c.value(); }; let state = Atomic(f); 0; }"#,
    );
    assert!(found.iter().any(|m| restriction(m)), "{found:?}");
}

#[test]
fn reject_atomic_replace() {
    let found = std_messages(
        r#"import "cell" take Cell; import "async/atomic" take Atomic; main() is int { let c = Cell(1); let f = () is int { c.value(); }; let state = Atomic(() is int { 0; }); state.replace(f); 0; }"#,
    );
    assert!(found.iter().any(|m| restriction(m)), "{found:?}");
}

#[test]
fn reject_atomic_bound() {
    let found = std_messages(
        r#"import "cell" take Cell; import "async/atomic" take Atomic; main() is int { let c = Cell(1); let f = () is int { c.value(); }; let state = Atomic(() is int { 0; }); let put is (() is int) is void = state.replace; put(f); 0; }"#,
    );
    assert!(found.iter().any(|m| restriction(m)), "{found:?}");
}

#[test]
fn reject_once_cell_from_action_field() {
    rejected(
        r#"object W { has factory is () is Cell<int>; W(:factory); read() is int { once let c = factory(); c.value(); } }
        async main() is int { W(factory: () is Cell<int> { Cell(1); }).read(); }"#,
    );
}

#[test]
fn allow_once_plain_action_field() {
    accepted(
        r#"object W { has factory is () is int; W(:factory); read() is int { once let n = factory(); n; } }
        async main() is int { W(factory: () is int { 1; }).read(); }"#,
    );
}

#[test]
fn reject_try_result() {
    rejected(
        r#"main() is int { let c = Cell(1); let f = () is int { c.value(); }; let g = try! f; 0; }"#,
    );
}

#[test]
fn reject_try_optional_storage() {
    rejected(
        r#"main() is int { let c = Cell(1); let f = () is int { c.value(); }; let g = try? f; 0; }"#,
    );
}

#[test]
fn reject_default_argument() {
    rejected(
        r#"object W { W(); use(f is () is int = if true { let c = Cell(1); () is int { c.value(); }; } else { () is int { 0; }; }) is int { f(); } }
        main() is int { W().use(); }"#,
    );
}

#[test]
fn reject_cell_erasure_in_array() {
    rejected(r#"main() is int { let cs is @any = [Cell(1)]; 0; }"#);
}

#[test]
fn reject_cell_erasure_in_return() {
    rejected(r#"object W { W(); make() is any { Cell(1); } } main() is int { 0; }"#);
}

#[test]
fn reject_cell_erasure_in_argument() {
    rejected(
        r#"object W { W(); use(x is any) is void {} } main() is int { W().use(Cell(1)); 0; }"#,
    );
}

#[test]
fn reject_cell_erasure_in_branch() {
    rejected(r#"main() is int { let c is any = if true { Cell(1); } else { Cell(2); }; 0; }"#);
}

#[test]
fn reject_frame_bound_in_await() {
    // Await already requires an async body and a Future. The new rule must
    // still diagnose the retained Cell when the operand is a local action.
    let found = messages(
        r#"async main() is int { let c = Cell(1); let f = () is int { c.value(); }; await f; 0; }"#,
    );
    assert!(
        found.iter().any(|m| m.contains("cannot be awaited")),
        "{found:?}"
    );
}

#[test]
fn original_957_real_std() {
    let found = std_messages(&format!(
        "import \"cell\" take Cell; import \"io/stdout\" take Stdout; {FACTORY} async main() is int {{ let m = M(); let f = m.mk(); let a = m.use(f); let b = m.use(f); Stdout().writeln((await a + await b).string()); 0; }}"
    ));
    assert!(found.iter().any(|m| restriction(m)), "{found:?}");
}

#[test]
fn reject_collection_capture_escape() {
    rejected(
        r#"object W { W(); make() is () is int { let cs = [Cell(1)]; () is int { cs[0].value(); }; } } main() is int { 0; }"#,
    );
}

#[test]
fn reject_nested_capture_return_of_ancestor() {
    rejected(
        r#"main() is int { let c = Cell(1); let f = () is int { c.value(); }; let g = () is int { let h = () is () is int { f; }; 0; }; 0; }"#,
    );
}

#[test]
fn reject_cell_role_narrowing() {
    let found = std_messages(
        r#"
        import "cell" take Cell;
        import "collections/iteration" take Cursor;
        role Present { available() is bool; }
        object C does Cursor<int>, Present {
            C(); available() is bool { true; } item() is int { 1; }
            successor() is Cursor<int> { self; }
        }
        main() is int { let cursor = [Cell(1)].cursor(); let erased is Present = cursor; 0; }
    "#,
    );
    assert!(found.iter().any(|m| restriction(m)), "{found:?}");
}
