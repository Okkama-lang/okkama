//! Семантический анализ: проверка до исполнения, независимо от backend.

use okkama_compiler::SourceFile;
use okkama_compiler::analysis::thir::{Coercion, ExprKind, StmtKind};
use okkama_compiler::analysis::{self, SemanticError, SourceModule};
use okkama_compiler::syntax::{self, ParseOptions, prune};

/// Ошибки анализа одного модуля (пусто — программа корректна).
fn errors(text: &str) -> Vec<SemanticError> {
    let source = SourceFile::new("test.ok", text).expect("small source");
    let program = prune::strip_test_items(
        syntax::parse(&source, &ParseOptions::default()).expect("valid syntax"),
    );
    let module = SourceModule {
        path: "app".to_string(),
        program: &program,
        imports: Vec::new(),
    };
    match analysis::analyze(vec![module]) {
        Ok(_) => Vec::new(),
        Err(errors) => errors,
    }
}

fn messages(text: &str) -> Vec<String> {
    errors(&format!("module app v1.0.0; {text}"))
        .into_iter()
        .map(|error| error.message)
        .collect()
}

#[test]
fn every_independent_error_is_reported() {
    assert_eq!(
        messages("main() is int { let a = 1 + true; let b = \"x\" - 2; let c = missing; 0; }"),
        [
            "Operator + cannot be applied to int and bool",
            "Operator - cannot be applied to string and int",
            "Undefined variable \"missing\"",
        ]
    );
}

#[test]
fn errors_do_not_cascade_from_poisoned_values() {
    // `a` уже ошибочна: её использование не порождает новых сообщений.
    assert_eq!(
        messages("main() is int { let a = 1 + true; let b = a * 2; a; }"),
        ["Operator + cannot be applied to int and bool"]
    );
}

#[test]
fn generic_bodies_are_checked_without_a_specialization() {
    assert_eq!(
        messages(
            "object Box<T> { has v is T; Box(:v); bad() is int { \"oops\" + 1; } } \
             main() is int { 0; }"
        ),
        ["Operator + cannot be applied to string and int"]
    );
    // Параметр типа непрозрачен: действия — только через ограничение-роль.
    assert_eq!(
        messages(
            "object Box<T> { has v is T; Box(:v); show() is string { v.string(); } } \
             main() is int { 0; }"
        ),
        ["Type parameter \"T\" has no action \"string\"; declare a role bound"]
    );
    assert!(
        messages(
            "role Ranked { rank() is int; } \
             object Best<T: Ranked> { has v is T; Best(:v); score() is int { v.rank(); } } \
             main() is int { 0; }"
        )
        .is_empty()
    );
}

#[test]
fn constructs_without_a_backend_are_still_checked() {
    // `rat` и async lowering ещё не умеет, но их правила проверяются.
    assert_eq!(
        messages("main() is int { let r = rat(1, 2); let x = r + 1; 0; }"),
        ["Operator + cannot be applied to rat and int"]
    );
    assert!(messages("main() is int { let r = rat(1, 2) + 1.rat(); 0; }").is_empty());
    assert_eq!(
        messages(
            "object W { W(); async value() is int { 7; } } \
             main() is int { let v = await W().value(); v; }"
        ),
        ["await is available only inside an async action"]
    );
    assert!(
        messages(
            "object W { W(); async value() is int { 7; } } \
             async main() is int { let v = await W().value(); v; }"
        )
        .is_empty()
    );
}

#[test]
fn objects_cross_role_parameters_and_overloads_pick_the_most_specific() {
    let shapes = "role Shape { area() is int; } \
        object Sq does Shape { has s is int; Sq(:s); area() is int { s * s; } }";
    assert!(
        messages(&format!(
            "{shapes} object Calc {{ Calc(); twice(shape is Shape) is int {{ shape.area() * 2; }} }} \
             main() is int {{ Calc().twice(Sq(s: 3)); }}"
        ))
        .is_empty()
    );
    // Точный тип специфичнее роли (§11.8).
    assert!(
        messages(&format!(
            "{shapes} object Calc {{ Calc(); f(shape is Shape) is int {{ 1; }} f(square is Sq) is int {{ 2; }} }} \
             main() is int {{ Calc().f(Sq(s: 3)); }}"
        ))
        .is_empty()
    );
}

#[test]
fn branch_values_cross_the_expected_type_one_by_one() {
    let shapes = "role Shape { area() is int; } \
        object Sq does Shape { has s is int; Sq(:s); area() is int { s * s; } } \
        object Dot does Shape { Dot(); area() is int { 0; } }";
    assert!(
        messages(&format!(
            "{shapes} object Pick {{ Pick(); one(big is bool) is Shape {{ if big {{ Sq(s: 2); }} else {{ Dot(); }} }} }} \
             main() is int {{ Pick().one(true).area(); }}"
        ))
        .is_empty()
    );
    assert_eq!(
        messages(&format!(
            "{shapes} main() is int {{ let x = if true {{ Sq(s: 2); }} else {{ Dot(); }}; 0; }}"
        )),
        Vec::<String>::new(),
        "a literal condition does not join the dead branch"
    );
    assert_eq!(
        messages(&format!(
            "{shapes} main() is int {{ let c = 1 > 0; let x = if c {{ Sq(s: 2); }} else {{ Dot(); }}; 0; }}"
        )),
        ["Branches produce different types: Sq and Dot"]
    );
}

#[test]
fn heterogeneous_literals_need_an_explicit_any_boundary() {
    // #315: `any` never comes from inference; only a declared `@any`/`%K:any` boxes.
    let array = "Array elements have incompatible types int and string; declare an explicit @any boundary to allow heterogeneous values";
    let map = "Map values have incompatible types int and string; declare an explicit %string:any boundary to allow heterogeneous values";
    assert_eq!(messages("main() is int { let xs = [1, \"a\"]; 0; }"), [array]);
    assert_eq!(
        messages("main() is int { let xs = [1, 2.0]; 0; }"),
        ["Array elements have incompatible types int and float; declare an explicit @any boundary to allow heterogeneous values"]
    );
    assert_eq!(messages("main() is int { let m = [\"a\": 1, \"b\": \"two\"]; 0; }"), [map]);
    // An untyped local does not box: the binding is not a boundary.
    assert_eq!(messages("main() is int { let xs = [1, \"a\"]; let ys is @any = xs; 0; }"), [array]);
    // The error is at the literal, and nothing cascades from it.
    let source = "module app v1.0.0; main() is int { let xs = [1, \"a\"]; let n = xs.size(); 0; }";
    let found = errors(source);
    assert_eq!(found.len(), 1);
    let at = source.find("[1,").unwrap() as u32;
    assert_eq!((found[0].span.start, found[0].span.end), (at, at + "[1, \"a\"]".len() as u32));
    // Declared boundaries: bindings, parameters, results, constants, nested positions.
    for accepted in [
        "main() is int { let xs is @any = [1, \"a\"]; let m is %string:any = [\"a\": 1, \"b\": \"two\"]; 0; }",
        "main() is int { let xs is @any = [1, 2.0]; let nested is @@any = [[1, \"a\"], [2.0]]; 0; }",
        "main() is int { let m is %string:@any = [\"a\": [1, \"b\"]]; 0; }",
        "object Box { Box(); size(xs is @any) is int { xs.size(); } } main() is int { Box().size([1, \"a\"]); }",
        "object Box { Box(); row() is %string:any { [\"id\": 7, \"name\": \"Ada\"]; } } main() is int { Box().row().size(); }",
        // Homogeneous literals and #297: an untagged `[]` is no conflict.
        "main() is int { let xs is @int = [1, 2..=3]; let ys is @@int = [[1], []]; let zs = [[1], []]; let n is @@int = zs; 0; }",
    ] {
        assert!(messages(accepted).is_empty(), "{accepted}: {:?}", messages(accepted));
    }
    // A typed constant is a boundary too; an `any` value is never a compile-time
    // constant (#153), and that is the only error.
    assert_eq!(
        messages("const MIXED is @any = [1, \"a\"]; main() is int { MIXED.size(); }"),
        ["Constant \"MIXED\" cannot be computed by the compiler: its value has to cross a role or `any` boundary"]
    );
    // `any` is an element, not a collection boundary: the inner literal has none.
    assert_eq!(messages("main() is int { let xs is @any = [[1, \"a\"]]; 0; }"), [array]);
}

#[test]
fn byte_values_are_not_map_keys() {
    assert_eq!(
        messages("main() is int { let m = [byte(1): 1]; 0; }"),
        ["Map key type byte is not a scalar key type"]
    );
}

/// Предупреждения анализа корректной программы.
fn warnings(text: &str) -> Vec<String> {
    let source =
        SourceFile::new("test.ok", format!("module app v1.0.0; {text}")).expect("small source");
    let program = prune::strip_test_items(
        syntax::parse(&source, &ParseOptions::default()).expect("valid syntax"),
    );
    let module = SourceModule {
        path: "app".to_string(),
        program: &program,
        imports: Vec::new(),
    };
    analysis::analyze(vec![module])
        .expect("valid program")
        .warnings
        .into_iter()
        .map(|warning| warning.message)
        .collect()
}

#[test]
fn a_for_loop_value_needs_a_body_that_runs() {
    // Ноль итераций известен статически — ошибка.
    for body in [
        "for 3..3 -> index { index; }",
        "for 5..=4 -> index { index; }",
        "let empty is @int = []; for [] -> index { index; }",
    ] {
        assert_eq!(
            messages(&format!("main() is int {{ {body} }}")),
            ["For loop has no value: its source is empty, so the body never runs"],
            "{body}"
        );
    }
    // Непустой источник известен — ни ошибки, ни предупреждения.
    for body in [
        "for 0..3 -> index { index; }",
        "for [4, 5] -> item { item; }",
    ] {
        assert!(
            messages(&format!("main() is int {{ {body} }}")).is_empty(),
            "{body}"
        );
        assert!(
            warnings(&format!("main() is int {{ {body} }}")).is_empty(),
            "{body}"
        );
    }
    // Число итераций неизвестно — предупреждение.
    assert_eq!(
        warnings(
            "main() is int { let xs = [1, 2]; let n = xs.size(); for 0..n -> index { index; } }"
        ),
        ["For loop value is undefined when its source is empty: the body may not run"]
    );
    // `for` как оператор значения не даёт: предупреждать не о чем.
    assert!(warnings("main() is int { let n = 2; for 0..n -> index { index; } 0; }").is_empty());
}

#[test]
fn map_keys_have_exactly_one_type() {
    // D-047: ключ `any` невозможен, поэтому разнородные ключи — ошибка.
    assert_eq!(
        messages("main() is int { let m = [1: \"int\", \"1\": \"string\"]; 0; }"),
        ["Map keys have different types: int and string"]
    );
    // Разнородные значения — только под объявленным `%K:any` (#315).
    assert!(
        messages("main() is int { let n is %string:any = [\"a\": 1, \"b\": \"two\"]; 0; }")
            .is_empty()
    );
}

#[test]
fn a_for_loop_produces_the_value_of_its_last_iteration() {
    assert!(
        messages("main() is int { for 0..3 -> index { last index if index == 2; -1; } }")
            .is_empty()
    );
    assert_eq!(
        messages("main() is int { for 0..3 -> index { \"x\"; } }"),
        ["Action result must be int, got string"]
    );
}

#[test]
fn boundaries_are_explicit_coercions_in_thir() {
    let source = SourceFile::new(
        "test.ok",
        "module app v1.0.0; role Shape { area() is int; } \
         object Sq does Shape { has s is int; Sq(:s); area() is int { s * s; } } \
         main() is int { let shape is Shape = Sq(s: 2); let xs is @int = []; shape.area(); }",
    )
    .expect("small source");
    let program = syntax::parse(&source, &ParseOptions::default()).expect("valid syntax");
    let analysis = analysis::analyze(vec![SourceModule {
        path: "app".to_string(),
        program: &program,
        imports: Vec::new(),
    }])
    .expect("valid program");
    let main = analysis
        .bodies
        .iter()
        .find(|body| body.owner == analysis::thir::BodyOwner::Entry)
        .expect("entry body");
    let coercions: Vec<Coercion> = main
        .block
        .stmts
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            StmtKind::Let { value, .. } => match &value.kind {
                ExprKind::Coerce { coercion, .. } => Some(coercion.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    // `[]` с целевым типом сразу типизирован: `@int`, без отдельного тега.
    assert_eq!(coercions, [Coercion::RoleWrap]);
    let empty = main
        .block
        .stmts
        .iter()
        .find_map(|stmt| match &stmt.kind {
            StmtKind::Let { value, .. } if matches!(value.kind, ExprKind::Array(_)) => {
                value.ty.clone()
            }
            _ => None,
        })
        .expect("the empty array literal");
    assert_eq!(
        empty,
        okkama_compiler::analysis::ty::Ty::array(okkama_compiler::analysis::ty::Ty::Int)
    );
}

const COUNTER: &str =
    "object C { has n is int = 0; C(); inc() is ^C { ^n = n + 1; } value() is int { n; } }";

#[test]
fn successor_obligations_are_checked_before_lowering() {
    for (body, expected) in [
        (
            "let c = C(); c.inc(); 0;",
            "Successor action result must be used",
        ),
        (
            "let c = C(); let d = c.inc(); let e = d; 0;",
            "Successor action result must be used; binding \"e\" can leave scope without consumption",
        ),
        (
            "let c = C(); { let d = c.inc(); } 0;",
            "Successor action result must be used; binding \"d\" can leave scope without consumption",
        ),
        (
            "let c = C(); let d = c.inc(); let f = 1 > 0; if f { d.value(); } 0;",
            "Successor action result must be used; binding \"d\" can leave scope without consumption",
        ),
        // Тело цикла может не выполниться: обязательство переживает цикл.
        (
            "let c = C(); let d = c.inc(); for 1..=2 -> i { d = d.inc(); } 0;",
            "Successor action result must be used; binding \"d\" can leave scope without consumption",
        ),
        (
            "let c = C(); let d = c.inc(); return 0;",
            "Successor action result must be used; binding \"d\" can leave scope without consumption",
        ),
        (
            "let c = C(); for 1..=2 -> i { let d = c.inc(); next; } 0;",
            "Successor action result must be used; binding \"d\" can leave scope without consumption",
        ),
        (
            "let c = C(); defer c.inc(); 0;",
            "Successor action result must be used",
        ),
    ] {
        assert_eq!(
            messages(&format!("{COUNTER} main() is int {{ {body} }}")),
            [expected],
            "{body}"
        );
    }
    // Использованные результаты: receiver, аргумент, возврат, rebind, агрегат.
    for body in [
        "let c = C(); let d = c.inc(); d.value();",
        "let c = C(); let d = c.inc(); let xs = [d]; 0;",
        "let c = C(); let d = c.inc(); let k = d.inc(); k.value();",
        "let c = C(); c = c.inc(); 0;",
        "let c = C(); for 1..=2 -> i { c = c.inc(); } c.value();",
        "let c = C(); let d = if 1 > 0 { c.inc(); } else { c.inc(); }; d.value();",
    ] {
        assert!(
            messages(&format!("{COUNTER} main() is int {{ {body} }}")).is_empty(),
            "{body}"
        );
    }
}

const ASYNC_COUNTER: &str =
    "object C { has n is int = 0; C(); async inc() is ^C { ^n = n + 1; } value() is int { n; } }";

/// The obligation of an async successor moves into its `Future<^C>`, which
/// must be awaited or handed on (Book, consumption; Bible §13.14; #754).
#[test]
fn successor_futures_carry_the_obligation_until_awaited() {
    let leaked = |name: &str| {
        format!("Successor action result must be used; binding \"{name}\" can leave scope without consumption")
    };
    let dropped = || "Successor action result must be used".to_string();
    for (body, expected) in [
        // Bound and dropped.
        ("let pending = C().inc(); 0;", leaked("pending")),
        // Bound and only read for `done()`/`cancelled()`.
        ("let pending = C().inc(); if pending.done() { 1; } else { 0; }", leaked("pending")),
        ("let pending = C().inc(); let gone = pending.cancelled(); 0;", leaked("pending")),
        // An alias shares the obligation; the latest binding is named.
        ("let pending = C().inc(); let alias = pending; 0;", leaked("alias")),
        // Awaited on one branch only.
        (
            "let pending = C().inc(); if 1 > 0 { let c = await pending; c.value(); } else { 0; }",
            leaked("pending"),
        ),
        // A spawned task carries the Future's obligation.
        ("let task = spawn C().inc(); 0;", leaked("task")),
        // A dropped call, and `done()` on a temporary, lose it at once.
        ("C().inc(); 0;", dropped()),
        ("if C().inc().done() { 1; } else { 0; }", dropped()),
    ] {
        assert_eq!(
            messages(&format!("{ASYNC_COUNTER} async main() is int {{ {body} }}")),
            [expected],
            "{body}"
        );
    }
    for body in [
        // Awaited later, also after reading `done()`.
        "let pending = C().inc(); let ready = pending.done(); let c = await pending; c.value();",
        // Awaited through an alias.
        "let pending = C().inc(); let alias = pending; let c = await alias; c.value();",
        // Returned from its scope as the value of an `if`.
        "let f = if 1 > 0 { let inner = C().inc(); inner; } else { C().inc(); }; let c = await f; c.value();",
        // Passed on into an aggregate, and stored there.
        "let pending = C().inc(); let all = [pending]; 0;",
        "let pending = C().inc(); let all = [pending]; let c = await all[0]; c.value();",
        // Awaited on one branch and passed on on the other.
        "let pending = C().inc(); if 1 > 0 { let c = await pending; c.value(); } else { let all = [pending]; 0; }",
        // Spawned and awaited.
        "let task = spawn C().inc(); let c = await task; c.value();",
        // An explicit cancel decides the transition.
        "let pending = C().inc(); pending.cancel(); 0;",
    ] {
        assert!(
            messages(&format!("{ASYNC_COUNTER} async main() is int {{ {body} }}")).is_empty(),
            "{body}"
        );
    }
}

/// #790 (owner's decision of 2026-11-21): a successor Future is handed on where its plain `Future<C>` is
/// expected (argument, field, declared result, generic holder), and that handoff consumes the obligation. A typed
/// local is not a handoff, the conversion goes one way only, and a wrong handoff reports one error.
#[test]
fn successor_futures_hand_off_as_plain_futures() {
    let objects = format!(
        "{ASYNC_COUNTER} object D {{ D(); }} \
         object Waiter {{ Waiter(); async take(f is Future<C>) is int {{ let c = await f; c.value(); }} \
           async other(f is Future<D>) is int {{ 0; }} async number(f is Future<int>) is int {{ await f; }} }} \
         object Holder {{ has f is Future<C>; Holder(:f); }} \
         object Keep<T> {{ has f is Future<T>; Keep(:f); future() is Future<T> {{ f; }} }} \
         object Pair<T> {{ has f is Future<T>; has x is T; Pair(:f, :x); }} \
         object Maker {{ Maker(); make() is Future<C> {{ C().inc(); }} named() is Future<C> {{ let f = C().inc(); return f; }} }}"
    );
    let leaked = |name: &str| {
        format!("Successor action result must be used; binding \"{name}\" can leave scope without consumption")
    };
    for body in [
        // An argument, a field, a declared result and a generic holder (T = C).
        "let pending = C().inc(); await Waiter().take(pending);",
        "let pending = C().inc(); let held = Holder(f: pending); 0;",
        "let f = Maker().make(); let c = await f; c.value();",
        "let f = Maker().named(); let c = await f; c.value();",
        "let pending = C().inc(); let kept = Keep(f: pending); let c = await kept.future(); c.value();",
        // A typed local keeps the obligation until it is awaited.
        "let pending is Future<C> = C().inc(); let c = await pending; c.value();",
        "let pending is Future<C> = C().inc(); await Waiter().take(pending);",
        // An alias, a block value and a temporary hand it on too.
        "let pending = C().inc(); let alias = pending; await Waiter().take(alias);",
        "await Waiter().take(if 1 > 0 { C().inc(); } else { C().inc(); });",
        "await Waiter().take(C().inc());",
    ] {
        assert!(messages(&format!("{objects} async main() is int {{ {body} }}")).is_empty(), "{body}");
    }
    for (body, expected) in [
        // A typed local alone does not discharge.
        ("let pending is Future<C> = C().inc(); 0;", vec![leaked("pending")]),
        // Awaited, then the successor result is dropped.
        ("let pending = C().inc(); await pending; 0;", vec!["Successor action result must be used".to_string()]),
        // Handed on only on one branch.
        ("let pending = C().inc(); if 1 > 0 { await Waiter().take(pending); } else { 0; }", vec![leaked("pending")]),
    ] {
        assert_eq!(messages(&format!("{objects} async main() is int {{ {body} }}")), expected, "{body}");
    }
    for (body, wanted) in [
        // A wrong result type, an unrelated Future, `any`, and a generic that would need another conversion:
        // each reports its own error once, with no must-use cascade.
        ("let pending = C().inc(); await Waiter().other(pending);", "No action \"other\""),
        ("let pending = C().inc(); await Waiter().number(pending);", "No action \"number\""),
        ("let pending = C().inc(); let boxed is any = pending; 0;", "Cannot initialize variable \"boxed\""),
        ("let pending = C().inc(); let pair = Pair(f: pending, x: 5); 0;", "Pair"),
    ] {
        let found = messages(&format!("{objects} async main() is int {{ {body} }}"));
        assert_eq!(found.len(), 1, "{body}: {found:?}");
        assert!(found[0].contains(wanted), "{body}: {found:?}");
    }
}

/// Замыкание несёт обязательства захваченных successor (#322).
#[test]
fn closures_carry_successor_obligations() {
    let objects = format!(
        "{COUNTER} object Run {{ Run(); call(f is () is int) is int {{ f(); }} }} \
         object Keep {{ has f is () is int; Keep(:f); }} \
         object Make {{ Make(); \
           made() is () is int {{ let c = C(); let d = c.inc(); () is int {{ d.value(); }}; }} \
           returned() is () is int {{ let c = C(); let d = c.inc(); let show = () is int {{ d.value(); }}; return show; }} }}"
    );
    let unused = |name: &str| {
        format!(
            "Successor action result must be used; \"d\" is carried by action value \"{name}\", which can leave scope without being called, returned, passed or stored"
        )
    };
    let start = "let c = C(); let d = c.inc(); let show = () is int { d.value(); };";
    let leaked =
        "Successor action result must be used; binding \"d\" can leave scope without consumption"
            .to_string();
    for (body, expected) in [
        // Создание и локальная привязка не гасят.
        (format!("{start} 0;"), unused("show")),
        (format!("{start} let again = show; 0;"), unused("again")),
        (format!("{start} let inner = show; let outer = () is int {{ inner(); }}; 0;"), unused("outer")),
        // Вызов только на одной ветви, в теле цикла, которое может не выполниться.
        (format!("{start} let f = 1 > 0; if f {{ show(); }} 0;"), unused("show")),
        (format!("{start} for 1..=2 -> i {{ show(); }} 0;"), unused("show")),
        (format!("{start} return 0;"), unused("show")),
        // Сравнение — не использование.
        (format!("{start} let same = show == show; 0;"), unused("show")),
        // Отброшенное временное замыкание обязательство не гасит.
        (
            "let c = C(); let d = c.inc(); (() is int { d.value(); }); 0;".to_string(),
            leaked.clone(),
        ),
        // Тело использует захват не на всех нормальных путях: замыкание его
        // не несёт, и вызов не гасит (§11.1).
        (
            "let c = C(); let d = c.inc(); let f = 1 > 0; \
             let show = () is int { if f { d.value(); } else { 0; } }; show();"
                .to_string(),
            leaked.clone(),
        ),
        (
            "let c = C(); let d = c.inc(); let f = 1 > 0; \
             let show = () is int { if f { return 0; } d.value(); }; show();"
                .to_string(),
            leaked.clone(),
        ),
        (
            "let c = C(); let d = c.inc(); let show = () is int { let e = d; 0; }; show();"
                .to_string(),
            leaked.clone(),
        ),
        (
            "let c = C(); let d = c.inc(); let show = () is int { for 1..=2 -> i { d.value(); } 0; }; show();"
                .to_string(),
            leaked.clone(),
        ),
    ] {
        assert_eq!(
            messages(&format!("{objects} main() is int {{ {body} }}")),
            [expected],
            "{body}"
        );
    }
    for body in [
        // Вызов, через алиас, сразу на месте, вложенно, на всех ветвях, в defer.
        format!("{start} show();"),
        format!("{start} let again = show; again();"),
        "let c = C(); let d = c.inc(); (() is int { d.value(); })();".to_string(),
        format!("{start} let outer = () is int {{ show(); }}; outer();"),
        format!("{start} let f = 1 > 0; if f {{ show(); }} else {{ show() + 1; }}"),
        format!("{start} defer show(); 0;"),
        // Передача, хранение, возврат.
        format!("{start} Run().call(show);"),
        format!("{start} let all = [show]; 0;"),
        format!("{start} let named = [\"show\": show]; 0;"),
        format!("{start} let kept = Keep(f: show); 0;"),
        format!("{start} let maybe = Maybe.some(show); 0;"),
        "let made = Make().made(); made();".to_string(),
        // Одно обязательство на два замыкания и прямое использование.
        format!("{start} let other = () is int {{ d.value() + 1; }}; other();"),
        format!("{start} d.value();"),
        "let c = C(); let d = c.inc(); d.value(); let show = () is int { d.value(); }; 0;"
            .to_string(),
        // `panic` и `throw` — не нормальный выход; использование на всех
        // остальных путях, в том числе через возврат и аргумент.
        "let c = C(); let d = c.inc(); let f = 1 > 0; \
         let show = () is int { if f { panic \"no\"; } d.value(); }; show();"
            .to_string(),
        "let c = C(); let d = c.inc(); let f = 1 > 0; \
         let show = () is int { if f { return d.value(); } d.value() + 1; }; show();"
            .to_string(),
        "let c = C(); let d = c.inc(); \
         let show = () is () is int { () is int { d.value(); }; }; let inner = show(); inner();"
            .to_string(),
        // Замыкание без successor не меняется.
        "let n = 1; let plain = () is int { n; }; 0;".to_string(),
    ] {
        assert!(
            messages(&format!("{objects} main() is int {{ {body} }}")).is_empty(),
            "{body}"
        );
    }
}

/// Bound action (`d.value` без вызова) несёт обязательство получателя, как
/// замыкание — свои захваты (#324); прямой вызов гасит сразу.
#[test]
fn bound_actions_carry_successor_obligations() {
    let objects = format!(
        "{COUNTER} role Valued {{ value() is int; }} object V does Valued {{ V(); value() is int {{ 7; }} }} \
         object Run {{ Run(); call(f is () is int) is int {{ f(); }} }} \
         object Keep {{ has f is () is int; Keep(:f); }} \
         object Make {{ Make(); made() is () is int {{ let c = C(); let d = c.inc(); let r is () is int = d.value; r; }} }}"
    );
    let carried = |name: &str| {
        format!(
            "Successor action result must be used; \"d\" is carried by action value \"{name}\", which can leave scope without being called, returned, passed or stored"
        )
    };
    let start = "let c = C(); let d = c.inc(); let read is () is int = d.value;";
    for (body, expected) in [
        ("let c = C(); let d = c.inc(); let read = d.value; 0;".to_string(), carried("read")),
        (format!("{start} 0;"), carried("read")),
        (format!("{start} let again = read; 0;"), carried("again")),
        (format!("{start} let f = 1 > 0; if f {{ read(); }} 0;"), carried("read")),
        (format!("{start} for 1..=2 -> i {{ read(); }} 0;"), carried("read")),
        (format!("{start} return 0;"), carried("read")),
        // Временный successor: назвать нечего, кроме значения-действия.
        (
            "let c = C(); let read is () is int = c.inc().value; 0;".to_string(),
            "Successor action result must be used; action value \"read\" carries it and can leave scope without being called, returned, passed or stored"
                .to_string(),
        ),
    ] {
        assert_eq!(
            messages(&format!("{objects} main() is int {{ {body} }}")),
            [expected],
            "{body}"
        );
    }
    for body in [
        format!("{start} read();"),
        format!("{start} let again = read; again();"),
        format!("{start} let other is () is int = d.value; other();"),
        format!("{start} d.value();"),
        "let c = C(); let d = c.inc(); d.value(); let read is () is int = d.value; 0;".to_string(),
        format!("{start} Run().call(read);"),
        format!("{start} let all = [read]; 0;"),
        format!("{start} let kept = Keep(f: read); 0;"),
        "let made = Make().made(); made();".to_string(),
        format!("{start} defer read(); 0;"),
        format!("{start} let show = () is int {{ read(); }}; show();"),
        format!("{start} let f = 1 > 0; if f {{ read(); }} else {{ read() + 1; }}"),
        // Bound action роли без successor не меняется.
        "let v is Valued = V(); let read is () is int = v.value; read();".to_string(),
        // Прямые вызовы и цепочки successor гасят получателя сразу.
        "let c = C(); c.inc().value();".to_string(),
        "let c = C(); c.inc().inc().value();".to_string(),
        "let c = C(); let e = c.inc().inc(); e.value();".to_string(),
    ] {
        assert!(
            messages(&format!("{objects} main() is int {{ {body} }}")).is_empty(),
            "{body}"
        );
    }
}

/// Захват — снимок: перепривязать захваченное имя нельзя (#321).
#[test]
fn captured_bindings_cannot_be_rebound() {
    let captured = |name: &str| {
        format!(
            "Cannot rebind captured variable \"{name}\" inside an anonymous action: captured bindings are snapshots; return the successor instead"
        )
    };
    for (body, expected) in [
        ("let c = C(); let bump = () is int { c = c.inc(); c.value(); }; bump();", captured("c")),
        (
            "let c = C(); let outer = () is int { let inner = () is int { c = c.inc(); c.value(); }; inner(); }; outer();",
            captured("c"),
        ),
        // Своя локальная внешнего замыкания — захват для внутреннего.
        (
            "let outer = () is int { let d = C(); let inner = () is int { d = d.inc(); d.value(); }; d = d.inc(); inner() + d.value(); }; outer();",
            captured("d"),
        ),
        // Сообщение о захвате важнее, чем о типе без successor.
        ("let n = 1; let f = () is int { n = 2; n; }; f();", captured("n")),
    ] {
        assert_eq!(
            messages(&format!("{COUNTER} main() is int {{ {body} }}")),
            [expected],
            "{body}"
        );
    }
    for body in [
        "let c = C(); let read = () is int { c.value(); }; read();",
        "let c = C(); let bump = () is int { let later = c.inc(); later.value(); }; bump();",
        "let bump = () is int { let c = C(); c = c.inc(); c.value(); }; bump();",
        // Затенение локальной и параметром: привязка самого действия.
        "let c = C(); let f = () is int { let c = C(); c = c.inc(); c.value(); }; f() + c.value();",
        "let c = C(); let f = (c is C) is int { c = c.inc(); c.value(); }; f(c);",
        // Внешнее имя перепривязывается снаружи, снимок не меняется.
        "let c = C(); let read = () is int { c.value(); }; c = c.inc(); read() + c.value();",
    ] {
        assert!(
            messages(&format!("{COUNTER} main() is int {{ {body} }}")).is_empty(),
            "{body}"
        );
    }
}

/// A rebind takes only a successor of the bound object itself (#592; owner's
/// ruling on #984): a fresh object or another binding's value is refused even
/// when the type has successor actions.
#[test]
fn a_rebind_takes_a_successor_of_the_same_object() {
    let objects = "object C { has n is int = 0; C(); inc() is ^C { ^n = n + 1; } copy() is C { C(); } value() is int { n; } }";
    let refused = "Cannot rebind variable \"c\" to a new value: a rebind takes a successor of the same object (c = c.inc(…))";
    for body in [
        "let c = C(); c = C(); c.value();",
        "let c = C(); let d = C(); c = d; c.value();",
        "let c = C(); let d = C(); c = d.inc(); c.value();",
        "let c = C(); let d = C(); c = d.inc().inc(); c.value();",
        "let c = C(); { let d = C(); c = d.inc(); } c.value();",
        // A plain action returning the type is not a successor.
        "let c = C(); c = c.copy(); c.value();",
        // Neither is a choice between successors: rebind in each branch instead.
        "let c = C(); c = if c.value() > 0 { c.inc(); } else { c.inc(); }; c.value();",
    ] {
        assert_eq!(
            messages(&format!("{objects} main() is int {{ {body} }}")),
            [refused],
            "{body}"
        );
    }
    for body in [
        "let c = C(); c = c.inc(); c.value();",
        "let c = C(); c = (c.inc()); c.value();",
        "let c = C(); c = c.inc().inc(); c.value();",
        // A local holding a successor is read through `Take`.
        "let c = C(); let d = c.inc(); d = d.inc(); d.value();",
        // A rebind in a nested block or a loop updates the original binding.
        "let c = C(); { c = c.inc(); } c.value();",
        "let c = C(); for 1..=2 -> i { c = c.inc(); } c.value();",
        "let c = C(); if c.value() > 0 { c = c.inc(); } else { c = c.inc().inc(); } c.value();",
    ] {
        let found = messages(&format!("{objects} main() is int {{ {body} }}"));
        assert!(found.is_empty(), "{body}: {found:?}");
    }
    // An async successor is awaited into the rebind.
    let found = messages(&format!("{ASYNC_COUNTER} async main() is int {{ let c = C(); c = await c.inc(); c.value(); }}"));
    assert!(found.is_empty(), "{found:?}");
    assert_eq!(
        messages(&format!("{ASYNC_COUNTER} async main() is int {{ let c = C(); let d = C(); c = await d.inc(); c.value(); }}")),
        [refused]
    );
}

#[test]
fn constructor_fields_are_checked_before_lowering() {
    for (object, expected) in [
        (
            "object Q { has v is int; has w is int; Q(v is int) { v = v; } }",
            "Object \"Q\" field \"w\" was not initialized",
        ),
        (
            "object Q { has v is int; Q(x is int) { confirm self.v > 0; v = x; } }",
            "Object \"Q\" field \"v\" is read before it is bound",
        ),
        (
            "object Q { has v is int; Q(x is int) { for 1..=2 -> i { v = x; } } }",
            "Field \"v\" must be bound before a loop that rebinds it",
        ),
    ] {
        assert_eq!(
            messages(&format!("{object} main() is int {{ 0; }}")),
            [expected],
            "{object}"
        );
    }
    // Связанное не на всех путях — проверка во время исполнения, не ошибка.
    assert!(
        messages(
            "object Q { has v is int; Q(x is int) { if x > 0 { v = x; } } } main() is int { 0; }"
        )
        .is_empty()
    );
}

#[test]
fn values_holding_any_are_never_compared() {
    // `any` не распаковывается в программе: сравнение `any` и коллекций из
    // него отвергается до запуска (Perl падает всегда, во время исполнения).
    assert_eq!(
        messages(
            "main() is int { let a is any = 1; let xs is @any = [1, \"x\"]; \
             let m is %string:any = [\"k\": 1]; \
             if a == a || xs == xs || m == m { 1; } else { 0; } }"
        ),
        [
            "Cannot compare any and any",
            "Cannot compare @any and @any",
            "Cannot compare %string:any and %string:any",
        ]
    );
}

#[test]
fn bound_actions_are_called_only_after_an_action_type_boundary() {
    let object = "object C { C(); bump(n is int) is int { n; } }";
    assert_eq!(
        messages(&format!(
            "{object} main() is int {{ let c = C(); let b = c.bump; b(1); }}"
        )),
        ["Bound action requires an exact action-type boundary before it can be called"]
    );
    assert!(
        messages(&format!(
            "{object} main() is int {{ let c = C(); let b is (int) is int = c.bump; b(1); }}"
        ))
        .is_empty()
    );
}

#[test]
fn for_await_checks_async_next_and_maybe_payload() {
    let valid = "object Stream { Stream(); async next() is Maybe<int> { Maybe.none; } } ";
    assert!(messages(&format!("{valid} async main() is int {{ for await Stream() -> item {{ let n = item + 1; next; }} 0; }}")).is_empty());
    assert!(messages(&format!("{valid} main() is int {{ for await Stream() -> item {{}} 0; }}")).iter().any(|m| m.contains("only inside an async")));
    assert!(!messages("object Stream { Stream(); next() is Maybe<int> { Maybe.none; } } async main() is int { for await Stream() -> item {} 0; }").is_empty());
    assert!(!messages("object Stream { Stream(); async next() is int { 1; } } async main() is int { for await Stream() -> item {} 0; }").is_empty());
    assert!(!messages("object Stream { Stream(); async next() is Presence<int> { Presence.none; } } async main() is int { for await Stream() -> item {} 0; }").is_empty());
    assert!(messages(&format!("{valid} async main() is int {{ for await Stream() -> a, b {{}} 0; }}")).iter().any(|m| m.contains("exactly one binding")));
    assert!(messages(&format!("{valid} async main() is int {{ for await Stream() -> item {{}} item; }}")).iter().any(|m| m.contains("Undefined variable")));
}

#[test]
fn calls_through_a_cap_name_the_cap() {
    // Issue #8: a cap value has computed fields read with `::`, no actions.
    assert_eq!(
        messages(
            "object Customer { has given, family is string; Customer(:given, :family); \
             greet() is void {} \
             cap Public { full_name is string { given + \" \" + family; } } } \
             main() is int { let owner = Customer(given: \"J\", family: \"D\"); let c = owner::Public; \
             c.greet(); c.full_name(); c.other(); 0; }"
        ),
        [
            "Action \"greet\" of \"Customer\" is not available through cap \"Customer::Public\": \
             a cap has no actions; read its computed fields with \"::\" (full_name)",
            "\"full_name\" is a computed field of cap \"Customer::Public\", not an action: \
             read it with \"::full_name\"",
            "Cap \"Customer::Public\" has no actions; read its computed fields with \"::\" (full_name)",
        ]
    );
}

/// A chain of `let`s that deepens a generic type stops at the type depth
/// limit with one error (#49): lowering would keep a full copy of every
/// value's type, memory growing with the square of the depth.
#[test]
fn types_deeper_than_the_limit_are_refused_once() {
    let lets: String = (0..100).map(|i| format!("    let b{} = b{i}.deeper();\n", i + 1)).collect();
    let text = format!(
        "object Box<T> {{\n    has value is T;\n    Box(:value);\n    deeper() is Box<@T> {{ Box(value: [value]); }}\n}}\nmain() is int {{\n    let b0 = Box(value: 1);\n{lets}    0;\n}}\n"
    );
    let found = messages(&text);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("is nested deeper than 64 levels"), "{found:?}");
}

/// Diagnostics that say what the rule is (#226, #229, #233).
#[test]
fn diagnostics_name_the_rule_and_the_fix() {
    let assign = messages("object Bottle {\n has opened is bool;\n Bottle(:opened);\n close() is void { opened = false; }\n}\nmain() is int { 0; }");
    assert!(assign.iter().any(|m| m.contains("Field \"opened\" cannot be assigned after construction") && m.contains("^opened")), "{assign:?}");
    let matched = messages("enum Color = (red, green, blue);\nmain() is int {\n let c = Color.red;\n let name = match c { case Color.red { \"r\"; } case Color.green { \"g\"; } };\n 0;\n}");
    assert!(matched.iter().any(|m| m.contains("does not cover blue")), "{matched:?}");
    let ordering = messages("main() is int {\n let sorted = [9, 0, 2, 1].ordering();\n 0;\n}");
    assert!(ordering.iter().any(|m| m.contains("import \"compare\" take Comparing")), "{ordering:?}");
}

/// The source a diagnostic points at and its message.
fn located(text: &str) -> Vec<(String, String)> {
    let source = format!("module app v1.0.0; {text}");
    errors(&source)
        .into_iter()
        .map(|error| (source[error.span.start as usize..error.span.end as usize].to_string(), error.message))
        .collect()
}

/// A `match` over an enum that leaves cases out names them wherever its
/// value is needed: an action's last value, a nested value, a compact
/// choice (#229, #291), once, at the match.
#[test]
fn a_match_that_leaves_enum_cases_out_names_them_at_the_match() {
    const PRIORITY: &str = "enum Priority = (low, normal, urgent);\n";
    let action = |body: &str| format!("{PRIORITY}object Queue {{\n Queue();\n weight(p is Priority) is int {{\n{body}\n }}\n}}\nmain() is int {{ 0; }}");
    let missing = |cases: &str, fallback: &str| {
        format!("match over enum `Priority` does not cover {cases}, so it has no value here; add the cases or {fallback}")
    };
    let tail = "match p { case Priority.low { 1; } case Priority.normal { 2; } }";
    assert_eq!(located(&action(tail)), [(tail.to_string(), missing("urgent", "an `else` branch"))]);
    // Several cases, in declaration order, after other statements.
    let tail = "match p { case Priority.urgent { 3; } }";
    assert_eq!(
        located(&action(&format!("let base = 1;\n{tail}"))),
        [(tail.to_string(), missing("low, normal", "an `else` branch"))]
    );
    // Arms that all diverge still leave the other cases without a value.
    let tail = "match p { case Priority.low { panic \"low\"; } }";
    assert_eq!(located(&action(tail)), [(tail.to_string(), missing("normal, urgent", "an `else` branch"))]);
    // A nested value and a `let` initializer (#229).
    let inner = "match p { case Priority.low { 1; } }";
    assert_eq!(
        located(&action(&format!("let n = 1 + {inner};\nn;"))),
        [(inner.to_string(), missing("normal, urgent", "an `else` branch"))]
    );
    assert_eq!(
        located(&action(&format!("let n = {inner};\nn;"))),
        [(inner.to_string(), missing("normal, urgent", "an `else` branch"))]
    );
    // Compact cases (#893): the same match, the same one error.
    let compact = "match p { case Priority.urgent: 1; case Priority.low: 2; }";
    assert_eq!(
        located(&action(&format!("let f = {compact};\nf;"))),
        [(compact.to_string(), missing("normal", "an `else` branch"))]
    );
    // Complete matches, an `else`, a `_:` and a statement are fine.
    for body in [
        "match p { case Priority.low { 1; } case Priority.normal { 2; } case Priority.urgent { 3; } }",
        "match p { case Priority.low { 1; } else { 2; } }",
        "match p { case Priority.urgent: 1; case Priority.low: 2; case Priority.normal: 3; }",
        "match p { case Priority.urgent: 1; case _: 2; }",
        "match p { case Priority.low { 1; } }\n0;",
    ] {
        assert_eq!(located(&action(body)), [], "{body}");
    }
    // Other missing values keep their messages.
    assert_eq!(
        messages(&action("let n = 1;\nif n > 0 { }")),
        ["Action \"weight\" must produce a return value of type int."]
    );
    assert_eq!(messages(&action("match missing { case Priority.low { 1; } }")), ["Undefined variable \"missing\""]);
    assert_eq!(messages(&action("\"x\";")), ["Action result must be int, got string"]);
    // Variants with payloads keep their own message, once.
    let shapes = |body: &str| {
        format!("enum Shape = (dot, circle(r is int), square(side is int));\nobject Q {{\n Q();\n f(s is Shape) is int {{\n{body}\n }}\n}}\nmain() is int {{ 0; }}")
    };
    for body in ["match s { case Shape.dot { 1; } case Shape.circle(r) { r; } }", "match s { case Shape.dot: 1; case Shape.circle(r): r; }"] {
        assert_eq!(messages(&shapes(body)), ["Non-exhaustive match: missing square"], "{body}");
    }
}

/// A successor that can leave its scope unconsumed is reported at its
/// binding, not at the start of the action (#272), also when the path that
/// loses it is a `catch`.
#[test]
fn an_unconsumed_successor_is_reported_at_its_binding() {
    let tab = "enum Denied is Error = (denied);\nobject Tab {\n has open is bool = true;\n Tab();\n closed() is ^Tab { ^open = false; }\n checked() is int { 1; }\n}\n";
    let message = "Successor action result must be used; binding \"done\" can leave scope without consumption";
    for body in [
        "let tab = Tab();\n let done = tab.closed();\n 0;",
        "let tab = Tab();\n let done = tab.closed();\n try { let again = done.closed(); again.checked(); } catch e is Denied { 0; }",
    ] {
        assert_eq!(
            located(&format!("{tab}main() is int {{\n {body}\n}}")),
            [("done".to_string(), message.to_string())],
            "{body}"
        );
    }
}

/// Errors of a module in test mode: fakes become `Role$fake` objects.
fn test_mode_located(text: &str) -> Vec<(String, String)> {
    let text = format!("module app v1.0.0; {text}");
    let source = SourceFile::new("test.ok", &text).expect("small source");
    let program = prune::desugar_test_items(
        syntax::parse(&source, &ParseOptions::default()).expect("valid syntax"),
    );
    let module = SourceModule {
        path: "app".to_string(),
        program: &program,
        imports: Vec::new(),
    };
    analysis::analyze(vec![module])
        .err()
        .unwrap_or_default()
        .into_iter()
        .map(|error| (text[error.span.start as usize..error.span.end as usize].to_string(), error.message))
        .collect()
}

/// A mistake in a role's fake names the role, not the generated object, and
/// points at the fake's own code (#276).
#[test]
fn fake_errors_name_the_role_and_point_at_the_fake() {
    let first_line = |found: Vec<(String, String)>| -> Vec<(String, String)> {
        found.into_iter().map(|(at, message)| (at.lines().next().unwrap_or("").to_string(), message)).collect()
    };
    let role = |extra: &str, fake: &str| format!("role Counter {{\n count(step is int) is int;\n{extra} fake {{\n{fake}\n }}\n}}\n");
    assert_eq!(
        first_line(test_mode_located(&role("", " has value is int;\n count(step is int) is int { value; }"))),
        [("value".to_string(), "Fake field \"value\" of role \"Counter\" needs a default: a fake takes no constructor arguments".to_string())]
    );
    assert_eq!(
        first_line(test_mode_located(&role("", " count(step is int) is string { \"0\"; }"))),
        [(
            "count(step is int) is string { \"0\"; }".to_string(),
            "Action \"count\" of the fake of role \"Counter\" must return int, got string".to_string()
        )]
    );
    assert_eq!(
        first_line(test_mode_located(&role("", " count(step is string) is int { 0; }"))),
        [(
            "count(step is string) is int { 0; }".to_string(),
            "Action \"count\" parameter 1 of the fake of role \"Counter\" must be int, got string".to_string()
        )]
    );
    assert_eq!(
        first_line(test_mode_located(&role(" reset() is void;\n", " count(step is int) is int { 0; }"))),
        [("fake {".to_string(), "The fake of role \"Counter\" does not implement action \"reset\"".to_string())]
    );
    // An object the programmer wrote keeps its name and place.
    assert_eq!(
        messages("role Counter {\n count() is int;\n}\nobject Real does Counter {\n Real();\n count() is string { \"0\"; }\n}\nmain() is int { 0; }"),
        ["Action \"count\" on object \"Real\" must return int for role \"Counter\", got string"]
    );
}

/// Exponent literals are floats (#57); one that cannot be a finite float, or
/// that is not zero but would round to 0.0, is an error.
#[test]
fn exponent_literals_are_floats_in_range() {
    assert!(messages("main() is int { let a is float = 1e5; let b is float = 1.5E-3; let c is float = -2e+10; let d is float = 4.9e-324; let e is float = 0e9; let f is float = 0.0e-999; 0; }").is_empty());
    assert_eq!(
        messages("main() is int { let a is int = 1e5; let b = 1e400; let c = 1e-400; let d = -1e-400; let e = 0.000_1e-330; 0; }"),
        [
            "Cannot initialize variable \"a\" of type int with float",
            "Float value must contain a finite number",
            "Float literal is too small: it would round to 0.0",
            "Float literal is too small: it would round to 0.0",
            "Float literal is too small: it would round to 0.0",
        ]
    );
    assert_eq!(messages("main() is int { let a = 1e99999999999999999999999; 0; }"), ["Float value must contain a finite number"]);
}

/// Value objects are map keys (#212): every field a scalar (also `byte` and
/// `rat`), an enum or such an object; a resource, a collection field or a
/// generic argument that is not a key is named in the error.
#[test]
fn value_objects_are_map_keys() {
    let declarations = "enum Suit = (hearts, spades); \
        object Card { has rank is int; has suit is Suit; has weight is rat; has mark is byte; Card(rank is int, suit is Suit, weight is rat, mark is byte) { self.rank = rank; self.suit = suit; self.weight = weight; self.mark = mark; } } \
        object Hand { has best is Card; Hand(best is Card) { self.best = best; } } \
        object Box<T> { has value is T; Box(value is T) { self.value = value; } } \
        object Tagged { has tags is @string; Tagged(tags is @string) { self.tags = tags; } } \
        object Outer { has inner is Tagged; Outer(inner is Tagged) { self.inner = inner; } } \
        object File { has path is string; File(path is string) { self.path = path; } destroy { path; } } \
        object Early { has index is %Tagged:int; Early(index is %Tagged:int) { self.index = index; } }";
    assert_eq!(
        messages(&format!("{declarations} main() is int {{ let a is %Card:int = [:]; let b is %Hand:int = [:]; let c is %Box<string>:int = [Box(\"x\"): 1]; let d is %File:int = [:]; let e is %Box<@int>:int = [:]; let f is %Outer:int = [:]; 0; }}")),
        [
            "Map key type Tagged cannot be a key: field \"tags\" is @string",
            "Map key type Tagged cannot be a key: field \"tags\" is @string",
            "Map key type File cannot be a key: File has a destroy block",
            "Map key type Box<@int> cannot be a key: field \"value\" is @int",
            "Map key type Outer cannot be a key: field \"inner\" (Tagged): field \"tags\" is @string",
        ]
    );
    assert_eq!(messages("main() is int { let a is %@int:int = [:]; 0; }"), ["Map key type @int is not a scalar key type"]);
}

/// A generic parameter used as a map key takes only key types (#242): the
/// specialization is rejected at compile time, also through a parameter
/// passed on to another generic object and through a key used only in a body.
#[test]
fn generic_map_keys_take_key_types() {
    let declarations = "object Card { has rank is int; Card(rank is int) { self.rank = rank; } } \
        object File { has path is string; File(path is string) { self.path = path; } destroy { path; } } \
        object Index<T> { has entries is %T:int; Index() { let empty is %T:int = [:]; entries = empty; } size() is int { entries.size(); } } \
        object Wrap<U> { has inner is Index<U>; Wrap() { inner = Index(); } } \
        object Late<T> { has items is @T; Late(items is @T) { self.items = items; } distinct() is int { let seen is %T:bool = [:]; seen.size(); } } \
        object Plain<T> { has items is @T; Plain(items is @T) { self.items = items; } } \
        object Literal<T> { has item is T; Literal(item is T) { self.item = item; } count() is int { [item: 1].size(); } }";
    assert!(messages(&format!("{declarations} main() is int {{ let a is Index<string> = Index(); let b is Index<Card> = Index(); let c is Wrap<int> = Wrap(); let d = Plain(items: [[1]]); let e = Late(items: [1]); let f = Literal(item: \"x\"); a.size() + b.size(); }}")).is_empty());
    assert_eq!(
        messages(&format!("{declarations} main() is int {{ let a is Index<@int> = Index(); let b is Index<File> = Index(); let c is Wrap<@string> = Wrap(); let d = Late(items: [[1]]); let e = Literal(item: [1]); 0; }}")),
        [
            "Generic parameter \"T\" for object \"Index\" is a map key: Map key type @int is not a scalar key type",
            "Generic parameter \"T\" for object \"Index\" is a map key: Map key type File cannot be a key: File has a destroy block",
            "Generic parameter \"U\" for object \"Wrap\" is a map key: Map key type @string is not a scalar key type",
            "Generic parameter \"T\" for object \"Late\" is a map key: Map key type @int is not a scalar key type",
            "Generic parameter \"T\" for object \"Literal\" is a map key: Map key type @int is not a scalar key type",
        ]
    );
}

/// An object with an async destroy, or one holding it in a field, is owned
/// only where the end of its scope can be awaited (#154): async bodies, and
/// constructors of objects that have such a destroy themselves.
#[test]
fn async_destroys_need_an_async_owner() {
    let declarations = "object Conn { has n is int; Conn(:n); async destroy { confirm n > 0; } } \
        object Holder { has conn is Conn; Holder(n is int) { conn = Conn(n: n); } } \
        object Pool { has conns is @Conn; Pool(:conns); }";
    assert!(messages(&format!("{declarations} async main() is int {{ let c = Conn(n: 1); let h = Holder(1); let p = Pool(conns: [Conn(n: 2)]); 0; }}")).is_empty());
    assert_eq!(
        messages(&format!("{declarations} main() is int {{ let c = Conn(n: 1); let h = Holder(1); let p = Pool(conns: []); 0; }}")),
        [
            "Conn has an async destroy; own it in an async action",
            "Holder has an async destroy; own it in an async action",
            "Pool has an async destroy; own it in an async action",
        ]
    );
    assert!(messages("object Conn { has n is int; Conn(:n); async destroy { await Conn(n: 1).wait(); } async wait() is void { confirm n > 0; } } async main() is int { 0; }").is_empty());
    assert!(!messages("object Conn { has n is int; Conn(:n); destroy { await Conn(n: 1).wait(); } async wait() is void { confirm n > 0; } } main() is int { 0; }").is_empty());
}

/// An anonymous action's declared result fixes its type and supplies generic
/// arguments (#214): `Keyed(origin:, key: (job is Job) is int { … })` infers
/// `Keyed<Job, int>` without an annotation at the binding.
#[test]
fn anonymous_action_results_supply_generic_arguments() {
    let declarations = "object Job { has weight is int; Job(weight is int) { self.weight = weight; } weight() is int { weight; } } \
        object Keyed<T, K> { has origin is @T; has key is (T) is K; Keyed(origin is @T, key is (T) is K) { self.origin = origin; self.key = key; } first() is K { key(origin[0]); } }";
    assert!(messages(&format!("{declarations} main() is int {{ let keyed = Keyed(origin: [Job(3)], key: (job is Job) is int {{ job.weight(); }}); let k is int = keyed.first(); k; }}")).is_empty());
    assert_eq!(
        messages(&format!("{declarations} main() is int {{ let keyed = Keyed(origin: [Job(3)], key: (job is Job) is int {{ job.weight(); }}); let k is string = keyed.first(); 0; }}")),
        ["Cannot initialize variable \"k\" of type string with int"]
    );
}

/// Default values of trailing parameters (#149): a call may leave them out,
/// positionally or by name; the value is checked in the declaring scope,
/// without the caller's locals, and reported once at the declaration.
#[test]
fn trailing_parameters_take_default_values() {
    let greeter = "object Greeter { has greeting is string; Greeter(greeting is string = \"hello\") { self.greeting = greeting; } \
        greet(name is string, mark is string = \"!\", times is int = 1) is string { greeting + name + mark + times.string(); } }";
    assert!(messages(&format!("{greeter} main() is int {{ let a = Greeter().greet(\"x\"); let b = Greeter(\"hi\").greet(\"y\", \"?\"); let c = Greeter().greet(name: \"z\", times: 2); 0; }}")).is_empty());
    assert_eq!(
        messages("object Bad { Bad(); first(a is int = 1, b is int) is int { a + b; } typed(a is int = \"one\") is int { a; } hidden(a is int = secret) is int { a; } } main() is int { let secret = 1; Bad().typed(); Bad().typed(); 0; }"),
        [
            "Parameter \"b\" follows a parameter with a default value, so it needs one too",
            "Default value must be int, got string",
            "Undefined variable \"secret\"",
        ]
    );
    assert_eq!(messages("role Named { name(prefix is string = \"x\") is string; } main() is int { 0; }"), ["Role actions cannot declare default values"]);
}

#[test]
fn break_and_continue_point_to_last_and_next() {
    let found = messages("main() is int { loop { break; } 0; }");
    assert!(found.iter().any(|m| m.ends_with("Okkama leaves a loop with `last`")), "{found:?}");
    let found = messages("main() is int { for [1] -> n { continue; } 0; }");
    assert!(found.iter().any(|m| m.contains("next iteration with `next`")), "{found:?}");
}

/// Module constants (#153): the compiler computes them, so what it cannot
/// compute — a call, a cycle, a failing operation — is an error.
#[test]
fn constants_are_computed_by_the_compiler() {
    assert!(messages("const A = B * 2; const B = 21; main() is int { A; }").is_empty());
    for (text, expected) in [
        ("const A = B + 1; const B = A + 1; main() is int { 0; }", "Constant \"A\" depends on itself"),
        ("const X = \"a\".length(); main() is int { 0; }", "it calls an action or a constructor"),
        ("const Z = 1 % 0; main() is int { 0; }", "cannot be computed by the compiler: Modulo by zero"),
        ("const X is int = \"a\"; main() is int { 0; }", "Constant must be int, got string"),
        ("const X = 1; const X = 2; main() is int { 0; }", "Constant \"X\" is already declared"),
    ] {
        let found = messages(text);
        assert!(found.iter().any(|m| m.contains(expected)), "{text}: {found:?}");
    }
}

#[test]
fn positional_calls_of_shorthand_constructors_name_the_named_form() {
    // #311: reported before generic inference and before the arguments.
    let decls = "object Point { has x is int; has y is int; Point(:x, :y); } \
                 object Box<T> { has v is T; Box(:v); } \
                 object Pair { has a is int; has b is int; Pair(:a, :b); Pair(a is int) { self.a = a; self.b = 0; } } \
                 object Bag { has xs is @int; Bag(:xs); Bag(items is ...int) { self.xs = items...; } }";
    let check = |body: &str| messages(&format!("{decls} main() is int {{ {body} 0; }}"));
    assert_eq!(
        check("let p = Point(1, 2);"),
        ["Constructor of \"Point\" takes named arguments only: write Point(x: …, y: …)"]
    );
    for body in ["let b = Box(5);", "let b is Box<int> = Box(5);", "let b = Box(nope);"] {
        assert_eq!(check(body), ["Constructor of \"Box\" takes named arguments only: write Box(v: …)"], "{body}");
    }
    assert_eq!(
        check("let p = Pair(1, 2);"),
        ["No constructor of \"Pair\" takes 2 positional argument(s); the shorthand takes named ones: write Pair(a: …, b: …)"]
    );
    // Positional overloads, named calls and real inference failures are unchanged.
    for body in ["let p = Pair(1);", "let g = Bag(1, 2, 3);", "let p = Point(x: 1, y: 2);", "let b = Box(v: 5);"] {
        assert!(check(body).is_empty(), "{body}: {:?}", check(body));
    }
    assert_eq!(check("let b = Box(v: []);"), ["Cannot infer generic parameter \"T\" for object \"Box\""]);
}

#[test]
fn arrays_of_payload_enums_have_searches_and_errors_name_the_full_type() {
    // #300: contains/index_of take the equality of `==`, variants included.
    let decls = "enum Reading = (empty, value(item is int)); enum Slot<T> = (vacant, full(item is T));";
    for body in [
        "let xs is @Reading = [Reading.empty]; let a = xs.contains(Reading.value(1)); let b = xs.index_of(Reading.empty);",
        "let xs is @Maybe<int> = [Maybe.some(1)]; let a = xs.contains(Maybe<int>.none);",
        "let xs is @Presence<int> = [Presence<int>.missing]; let a = xs.index_of(Presence.some(2));",
        "let xs is @Slot<int> = [Slot.full(3)]; let a = xs.contains(Slot<int>.vacant);",
    ] {
        let found = messages(&format!("{decls} main() is int {{ {body} 0; }}"));
        assert!(found.is_empty(), "{body}: {found:?}");
    }
    assert_eq!(
        messages("main() is int { let a is any = 1; let xs is @any = [a]; let b = xs.contains(a); 0; }"),
        ["Type \"@any\" has no builtin action \"contains\""]
    );
}

#[test]
fn try_optional_keeps_its_type_when_operand_paths_throw() {
    // #307: a throw ends its own path, not the try? that recovers from it.
    let decls = "enum E is Error = (a, b);";
    let check = |body: &str| messages(&format!("{decls} main() is int {{ let c = 1 > 0; {body} 0; }}"));
    for body in [
        "let m = try? (if true { throw E.a; } else { 2; }) else 7;",
        "let m is Maybe<int> = try? (if true { throw E.a; } else { 2; });",
        "let m is Maybe<int> = try? (if c { throw E.a; } else { throw E.b; });",
        "let m = try? (if c { throw E.a; } else { throw E.b; }) else 8; let n is int = m;",
        "let m = try? loop { throw E.a; } else 9;",
        "let m = try? (match true { case true: throw E.a; case false: 2; }) else 5;",
        "let m = try! (if true { throw E.a; } else { 2; }); let n is int = m;",
    ] {
        assert!(check(body).is_empty(), "{body}: {:?}", check(body));
    }
    // No successful path and no context: an inference error, not a silent poison.
    for body in ["let m = try? (if c { throw E.a; } else { throw E.b; });", "let m = try? loop { throw E.a; };"] {
        assert_eq!(
            check(body),
            ["Cannot infer the type of try?: its operand never produces a value; declare Maybe<T> or add a fallback"],
            "{body}"
        );
    }
    // The selected throwing branch keeps the other branch's type for try?.
    assert_eq!(
        check("let m = try? (if true { throw E.a; } else { 2; }); let n is Maybe<string> = m;"),
        ["Cannot initialize variable \"n\" of type Maybe<string> with Maybe<int>"]
    );
}

#[test]
fn untagged_empty_collections_take_their_siblings_type_at_any_depth() {
    // #297: order, nesting, maps, branches, comparisons, constants and results.
    for body in [
        "let a is @@int = [[1], []];",
        "let a is @@int = [[], [1]];",
        "let m is %string:@int = [\"a\": [1], \"b\": []];",
        "let y is @@@int = [[[]], [[1]]];",
        "let z is @%string:int = [[:], [\"k\": 1]];",
        "let v is @@int = [[]]; let ok = v == [[]];",
        "let ok = [[1]] == [[]];",
        "let c = true; let x is @@int = if c { [[]]; } else { [[2, 3]]; };",
        "let x is @@int = match 1 { case 1 { [[]]; } else { [[1]]; } };",
    ] {
        let found = messages(&format!("main() is int {{ {body} 0; }}"));
        assert!(found.is_empty(), "{body}: {found:?}");
    }
    let found = messages("const K is @@int = [[]]; object P { P(); r() is @@int { [[]]; } } main() is int { 0; }");
    assert!(found.is_empty(), "{found:?}");
    // Concrete differences are not taken for empty ones.
    assert_eq!(
        messages("main() is int { let c = true; let x = if c { [[1]]; } else { [[\"s\"]]; }; 0; }"),
        ["Branches produce different types: @@int and @@string"]
    );
    assert_eq!(
        messages("main() is int { let ok = [[1]] == [[\"s\"]]; 0; }"),
        ["Cannot compare @@int and @@string"]
    );
}

#[test]
fn branches_after_an_untagged_literal_are_checked_against_each_other() {
    // #982: `[[]]` fits both `[[1]]` and `[["x"]]`, but those two do not meet;
    // every multi-branch join reports them instead of typing the value @@never.
    let conflict = "Branches produce different types: @@int and @@string";
    let decls = "enum Pick = (one, two, three); enum Shape = (dot, circle(r is int), square(side is int));";
    for body in [
        "let p = Pick.three; let v = match p { case Pick.one { [[]]; } case Pick.two { [[1]]; } case Pick.three { [[\"x\"]]; } };",
        "let v = match 1 { case 1 { [[]]; } case 2 { [[1]]; } else { [[\"x\"]]; } };",
        "let s = Shape.dot; let v = match s { case Shape.dot { [[]]; } case Shape.circle(r) { [[r]]; } case Shape.square(side) { [[\"x\"]]; } };",
        // `if` has two branches; an `else if` link joins its own pair first.
        "let a = true; let b = false; let v = if a { [[]]; } else if b { [[1]]; } else { [[\"x\"]]; };",
    ] {
        let found = messages(&format!("{decls} main() is int {{ {body} 0; }}"));
        assert_eq!(found, [conflict], "{body}");
    }
    // Array literals already met element by element (#315).
    assert_eq!(
        messages("main() is int { let v = [[[]], [[1]], [[\"x\"]]]; 0; }"),
        ["Array elements have incompatible types @@int and @@string; declare an explicit @any boundary to allow heterogeneous values"]
    );
    // Untagged literals still take their siblings' type.
    for body in [
        "let v is @@int = [[], [1, 2]];",
        "let w = [[], [1, 2]]; let n is @@int = w;",
        "let p = Pick.one; let v = match p { case Pick.one { [[]]; } case Pick.two { [[1]]; } case Pick.three { [[]]; } }; let n is @@int = v;",
        "let v = match 1 { case 1 { [[]]; } else { [[1]]; } }; let n is @@int = v;",
    ] {
        let found = messages(&format!("{decls} main() is int {{ {body} 0; }}"));
        assert!(found.is_empty(), "{body}: {found:?}");
    }
}

#[test]
fn loop_exits_meet_like_branch_values() {
    // #994: the `last` values of a `loop` take the untagged retagging of #297,
    // and conflict the way branches do (#982).
    for body in [
        "let a = true; let v = loop { if a { last [[]]; } last [[1]]; }; let n is @@int = v;",
        "let a = true; let v = loop { if a { last [[1]]; } last [[]]; }; let n is @@int = v;",
        "let a = true; let v = loop { if a { last [:]; } last [\"k\": [1]]; }; let n is %string:@int = v;",
        "let a = true; let b = false; let v = loop { if a { last [[[]]]; } if b { last [[[3]]]; } last [[]]; }; let n is @@@int = v;",
        "let v = loop { last 7; }; let n is int = v;",
    ] {
        let found = messages(&format!("main() is int {{ {body} 0; }}"));
        assert!(found.is_empty(), "{body}: {found:?}");
    }
    for (body, expected) in [
        (
            "let a = true; let b = false; let v = loop { if a { last [[]]; } if b { last [[1]]; } last [[\"x\"]]; };",
            "Branches produce different types: @@int and @@string",
        ),
        (
            "let a = true; let v = loop { if a { last [[]]; } last 1; };",
            "Branches produce different types: @[] and int",
        ),
        (
            "let a = true; let v = loop { if a { last 1; } last \"s\"; };",
            "Branches produce different types: int and string",
        ),
    ] {
        assert_eq!(messages(&format!("main() is int {{ {body} 0; }}")), [expected], "{body}");
    }
}
#[test]
fn generic_cases_do_not_impose_an_uninferred_parameter_on_their_payloads() {
    // #304: loop/last, else and reduce need an expected type; an uninferred T is none.
    let decls = "enum Slot<T> = (vacant, full(item is T)); enum Two<T> = (empty, both(a is T, b is T));";
    for body in [
        "let a = Maybe.some(loop { last 42; });",
        "let a = Maybe.some(Maybe.some(42) else 0);",
        "let a = Maybe.some((0..1).reduce(0) -> acc, i { 42; });",
        "let a = Slot.full(loop { last 42; });",
        "let a = Two.both(loop { last 1; }, 2);",
        "let a = Slot.full(Slot.full(loop { last 5; }));",
        "let a = Slot<int>.full(loop { last 3; });",
    ] {
        let found = messages(&format!("{decls} main() is int {{ {body} 0; }}"));
        assert!(found.is_empty(), "{body}: {found:?}");
    }
    // Real conflicts and missing evidence are still reported.
    assert_eq!(
        messages(&format!("{decls} main() is int {{ let a = Two.both(loop {{ last 1; }}, \"s\"); 0; }}")),
        ["Generic parameter \"T\" for object \"Two\" cannot be both int and string"]
    );
    assert_eq!(
        messages(&format!("{decls} main() is int {{ let a = Slot.full([]); 0; }}")),
        ["Cannot infer type arguments for Slot.full; provide an expected type or explicit type arguments"]
    );
}

#[test]
fn builtin_role_parameters_wrap_implementers_and_nothing_else_does() {
    // #299: role.wrap where the role is the declared type, as for action parameters.
    let decls = "role Shape { area() is int; } \
                 object Square does Shape { has side is int; Square(:side); area() is int { side * side; } } \
                 object Rect does Shape { has w is int; Rect(:w); area() is int { w; } }";
    let check = |body: &str| messages(&format!("{decls} main() is int {{ let c = 1 > 0; let shapes is @Shape = [Square(side: 1)]; {body} 0; }}"));
    for body in [
        "let a = shapes.contains(Square(side: 1));",
        "let a = shapes.index_of(Rect(w: 2)) else -1;",
        "let s is Shape = Rect(w: 1); let a = shapes.contains(s);",
    ] {
        assert!(check(body).is_empty(), "{body}: {:?}", check(body));
    }
    for (body, expected) in [
        ("let a = shapes.contains(5);", "Cannot pass int to builtin action \"contains\" parameter of type Shape"),
        ("let s is Shape = Rect(w: 1); let a = s == Rect(w: 1);", "Cannot compare Shape and Rect"),
        ("let a = if c { Rect(w: 1); } else { Square(side: 1); };", "Branches produce different types: Rect and Square"),
    ] {
        assert_eq!(check(body), [expected], "{body}");
    }
}

#[test]
fn an_untyped_empty_reduce_accumulator_says_what_to_write() {
    // #295: the block gives the type the initial value lacks.
    assert_eq!(
        messages("main() is int { let built = (0..3).reduce([]) -> acc, i { acc + [i * i]; }; 0; }"),
        ["Reduce starts from [], which has no element type, and its block gives @int: start from @int() or declare the result type (let name is @int = …)"]
    );
    assert_eq!(
        messages("main() is int { let index = (0..3).reduce([:]) -> found, i { found + [i: i]; }; 0; }"),
        ["Reduce starts from [:], which has no key or value type, and its block gives %int:int: declare the result type (let name is %int:int = …)"]
    );
    for accepted in [
        "main() is int { let built = (0..3).reduce(@int()) -> acc, i { acc + [i * i]; }; built.size(); }",
        "main() is int { let built is @int = (0..3).reduce([]) -> acc, i { acc + [i * i]; }; built.size(); }",
        "main() is int { let index is %int:int = (0..3).reduce([:]) -> found, i { found + [i: i]; }; index.size(); }",
        // A block that keeps the untyped accumulator is still valid.
        "main() is int { let same = (0..3).reduce([]) -> acc, i { acc; }; 0; }",
    ] {
        assert!(messages(accepted).is_empty(), "{accepted}: {:?}", messages(accepted));
    }
}

#[test]
fn an_ambiguous_call_names_the_argument_types_and_the_tied_candidates() {
    // #273: as the no-match case names the argument types.
    let pairing = "object Pairing { Pairing(); \
        joined(left is int, right is any) is string { \"left\"; } \
        joined(left is any, right is int) is string { \"right\"; } \
        joined(left is string, right is string) is string { \"strings\"; } }";
    assert_eq!(
        messages(&format!("{pairing} main() is int {{ Pairing().joined(1, 2); 0; }}")),
        ["Ambiguous call of action \"joined\" on object \"Pairing\" with (int, int): joined(int, any) and joined(any, int) fit equally well"]
    );
    let boxes = "object Box { has text is string; Box(left is int, right is any) { text = \"l\"; } \
        Box(left is any, right is int) { text = \"r\"; } }";
    assert_eq!(
        messages(&format!("{boxes} main() is int {{ let b = Box(1, 2); 0; }}")),
        ["Ambiguous construction of \"Box\" with (int, int): Box(int, any) and Box(any, int) fit equally well"]
    );
}

/// #286: a repeated enum case or literal value, and an `else` after cases that cover every value, can never run.
#[test]
fn unreachable_match_cases_are_errors() {
    let status = "enum Status = (ready, failed, done); enum Reading = (value(n is int), empty);";
    for (body, expected) in [
        ("let s = Status.ready; match s { case Status.ready, Status.done { 1; } case Status.failed { 2; } case Status.ready { 3; } }", "Duplicate variant pattern"),
        ("let s = Status.ready; match s { case Status.ready, Status.ready { 1; } else { 2; } }", "Duplicate variant pattern"),
        ("let n = 3; match n { case 1 { 1; } case 1 { 2; } else { 0; } }", "Duplicate case pattern"),
        ("let t = \"a\"; match t { case \"a\" { 1; } case \"b\", \"a\" { 2; } else { 0; } }", "Duplicate case pattern"),
        ("let b = true; match b { case true { 1; } case false { 2; } else { 3; } }", "Unreachable else: the cases already cover every value"),
        ("let s = Status.ready; match s { case Status.ready, Status.failed, Status.done { 1; } else { 2; } }", "Unreachable else: the cases already cover every value"),
        ("let r = Reading.empty; match r { case Reading.value(n) { n; } case Reading.empty { 0; } else { 1; } }", "Unreachable else: the cases already cover every value"),
    ] {
        assert_eq!(messages(&format!("{status} main() is int {{ {body} }}")), [expected.to_string()], "{body}");
    }
    for body in [
        "let s = Status.ready; match s { case Status.ready { 1; } case Status.failed { 2; } else { 3; } }",
        "let s = Status.ready; match s { case Status.ready { 1; } case Status.failed { 2; } case Status.done { 3; } }",
        "let n = 3; match n { case 1 { 1; } case 2 { 2; } else { 0; } }",
        "let x = 0.5; match x { case 0.5 { 1; } else { 0; } }",
    ] {
        assert!(messages(&format!("{status} main() is int {{ {body} }}")).is_empty(), "{body}");
    }
}


/// #475: every repeated enum case is reported once per repetition, in source
/// order, at the repetition; the first declaration keeps the case, so the
/// enum stays usable and a wide unique enum has no errors.
#[test]
fn repeated_enum_cases_are_reported_in_order_at_the_repetition() {
    let text = "module app v1.0.0; enum E = (a, b, a, c, b, a); main() is int { let e = E.c; 0; }";
    let found = errors(text);
    let reported: Vec<&str> = found.iter().map(|error| error.message.as_str()).collect();
    assert_eq!(reported, [
        "Enum \"E\" already has case \"a\"",
        "Enum \"E\" already has case \"b\"",
        "Enum \"E\" already has case \"a\"",
    ]);
    let starts: Vec<usize> = found.iter().map(|error| error.span.start as usize).collect();
    let at = |nth: usize, name: &str| text.match_indices(name).filter(|(i, _)| {
        text[..*i].ends_with(", ") || text[..*i].ends_with('(')
    }).nth(nth).map(|(i, _)| i).unwrap();
    assert_eq!(starts, [at(1, "a"), at(1, "b"), at(2, "a")]);
    let wide: Vec<String> = (0..3000).map(|i| format!("c{i}")).collect();
    assert!(messages(&format!("enum W = ({}); main() is int {{ let w = W.c2999; 0; }}", wide.join(", "))).is_empty());
}

/// #955: a `match` over a subject that is not an enum cannot cover every
/// value with cases, so used as a value without `else` it says so at the
/// match and names the remainder branch; over bool it names the missing value.
#[test]
fn a_match_over_other_values_without_else_names_the_missing_else() {
    let wrap = |subject: &str, ty: &str, body: &str| {
        format!("object Q {{\n Q();\n f(x is {ty}) is int {{\n{body}\n }}\n}}\nmain() is int {{ let v = {subject}; 0; }}")
    };
    let other = |ty: &str| {
        format!("match over {ty} cannot cover every value, so it has no value here; add an `else` branch (or `case _`)")
    };
    // A `let` initializer, the issue's form.
    let inner = "match x { case 1: 10; }";
    assert_eq!(located(&wrap("0", "int", &format!("let s = {inner};\ns;"))), [(inner.to_string(), other("int"))]);
    // The action's last value, and a string subject with block cases.
    let tail = "match x { case 1 { 10; } case 2 { 20; } }";
    assert_eq!(located(&wrap("0", "int", tail)), [(tail.to_string(), other("int"))]);
    let tail = "match x { case \"a\" { 1; } }";
    assert_eq!(located(&wrap("\"\"", "string", tail)), [(tail.to_string(), other("string"))]);
    // Over bool the missing value is named.
    let tail = "match x { case true: 1; }";
    assert_eq!(
        located(&wrap("true", "bool", tail)),
        [(tail.to_string(), "match over bool does not cover false, so it has no value here; add the case or an `else` branch".to_string())]
    );
    // An `else`, a `case _`, a covering bool match and a statement are fine.
    for (subject, ty, body) in [
        ("0", "int", "match x { case 1 { 10; } else { 0; } }"),
        ("0", "int", "match x { case 1 { 10; } case _ { 0; } }"),
        ("\"\"", "string", "let s = match x { case \"a\": 1; case _: 2; };\ns;"),
        ("true", "bool", "match x { case true: 1; case false: 0; }"),
        ("0", "int", "match x { case 1 { 10; } }\n0;"),
    ] {
        assert_eq!(located(&wrap(subject, ty, body)), [], "{body}");
    }
}

/// #959: the argument list of a no-match or ambiguity message shows the
/// spread, last, as `@T...`; it is often the argument that did not fit.
#[test]
fn call_messages_list_the_spread_argument() {
    let decls = "object K { K() { } join(values is ...string) is string { \"x\"; } \
                 pick(values is ...int) is int { 0; } pick(first is int, values is ...int) is int { first; } } \
                 object B { B(values is ...string) { } B(n is int, values is ...string) { } }";
    let first = |body: &str| {
        messages(&format!("{decls} main() is int {{ let nums = [1, 2]; {body} 0; }}"))
            .into_iter()
            .next()
            .unwrap_or_default()
    };
    for (body, expected) in [
        ("K().join(\"a\", nums...);", "No action \"join\" on object \"K\" accepts (string, @int...)"),
        ("K().join(nums...);", "No action \"join\" on object \"K\" accepts (@int...)"),
        ("let b = B(true, nums...);", "No constructor of \"B\" accepts (bool, @int...)"),
        ("K().pick(1, nums...);", "Ambiguous call of action \"pick\" on object \"K\" with (int, @int...)"),
    ] {
        let message = first(body);
        assert!(message.starts_with(expected), "{body}: {message}");
    }
    // Without a spread the list is the plain arguments; a fitting spread is fine.
    assert!(first("K().join(\"a\", K());").starts_with("No action \"join\" on object \"K\" accepts (string, K)"));
    assert_eq!(first("let words is @string = [\"a\"]; K().join(\"b\", words...);"), "");
}

/// #952: `order(other)` and an array's `cursor()` return std types, so without
/// those types loaded the message names the import instead of saying the
/// action does not exist.
#[test]
fn order_and_cursor_name_the_import_they_need() {
    for (body, expected) in [
        ("let x = \"a\".order(\"b\");", "Builtin action \"order\" returns Order from the module compare, which this program does not load: add `import \"compare\" take Order;`"),
        ("let x = 1.order(2);", "Builtin action \"order\" returns Order from the module compare, which this program does not load: add `import \"compare\" take Order;`"),
        ("let x = 'a'.order('b');", "Builtin action \"order\" returns Order from the module compare, which this program does not load: add `import \"compare\" take Order;`"),
        ("let c = [1, 2].cursor();", "Builtin action \"cursor\" returns Cursor from the module collections/iteration, which this program does not load: add `import \"collections/iteration\" take Cursor;`"),
    ] {
        assert_eq!(messages(&format!("main() is int {{ {body} 0; }}")), [expected], "{body}");
    }
    // A missing action stays a missing action.
    assert_eq!(messages("main() is int { let x = true.order(false); 0; }"), ["Type \"bool\" has no builtin action \"order\""]);
}

/// #954: a map literal names each key once. Keys the compiler can compute
/// (literals, operators over them, `rat(…)`) and that are one map key are an
/// error at the repeat, with a note at the first; computed keys are left to
/// the run.
#[test]
fn a_repeated_constant_key_in_a_map_literal_is_rejected() {
    for (body, key) in [
        ("let m = [\"a\": 1, \"b\": 2, \"a\": 3];", "\"a\""),
        ("let m = [10: \"x\", 10: \"y\"];", "10"),
        ("let m = [true: 1, true: 2];", "true"),
        ("let m = ['x': 1, 'x': 2];", "'x'"),
        ("let m is %rat:int = [rat(1, 2): 1, 2 / 4: 2];", "1/2"),
        ("let m = [1 + 1: 1, 2: 2];", "2"),
    ] {
        assert_eq!(
            messages(&format!("main() is int {{ {body} 0; }}")),
            [format!("Duplicate key in map literal: {key}"), format!("Map key {key} first appears here")],
            "{body}"
        );
    }
    // Three equal keys: each repeat points at the first.
    assert_eq!(
        messages("main() is int { let m = [1: 1, 1: 2, 1: 3]; 0; }"),
        ["Duplicate key in map literal: 1", "Map key 1 first appears here", "Duplicate key in map literal: 1", "Map key 1 first appears here"]
    );
    // Distinct keys, computed keys and merges are not errors here.
    for body in [
        "let m = [\"a\": 1, \"b\": 2];",
        "let k = \"a\"; let m = [k: 1, \"a\": 2];",
        "let m = [rat(1, 2): 1, rat(1, 3): 2];",
        "let m = [\"a\": 1] + [\"a\": 2];",
        "let m is %string:int = [:];",
    ] {
        assert!(messages(&format!("main() is int {{ {body} 0; }}")).is_empty(), "{body}");
    }
}

/// The type of `object.field` after analysis, as diagnostics name it.
fn field_type(text: &str, object: &str, field: &str) -> String {
    let source = SourceFile::new("test.ok", format!("module app v1.0.0; {text}")).expect("small source");
    let program = prune::strip_test_items(syntax::parse(&source, &ParseOptions::default()).expect("valid syntax"));
    let module = SourceModule { path: "app".to_string(), program: &program, imports: Vec::new() };
    let analysis = analysis::analyze(vec![module]).unwrap_or_else(|errors| panic!("{text}: {errors:?}"));
    let registry = &analysis.registry;
    let info = registry.objects.iter().find(|info| info.name == object).expect("object");
    let found = info.fields.iter().find(|info| info.name == field).expect("field");
    registry.type_name_in(info.module, &found.ty)
}

/// #967: a field with an initializer and no type takes the initializer's
/// static type, exactly as if it were written: scalars, objects, enums,
/// collections, constants, generic and role-typed values keep their type.
#[test]
fn a_field_initializer_gives_the_field_its_static_type() {
    let decls = "enum Color = (red, green); role Named { name() is string; } \
                 object Item does Named { Item(); name() is string { \"item\"; } } \
                 object Maker { Maker(); make() is Named { Item(); } } \
                 object Pair<A> { has a is A; Pair(:a); } const LIMIT = 3;";
    for (init, expected) in [
        ("1", "int"),
        ("\"text\"", "string"),
        ("true", "bool"),
        ("1.5", "float"),
        ("rat(1, 2)", "rat"),
        ("Item()", "Item"),
        ("Color.red", "Color"),
        ("[1, 2]", "@int"),
        ("[\"a\": 1]", "%string:int"),
        ("[[1], []]", "@@int"),
        ("LIMIT + 1", "int"),
        ("Maker().make()", "Named"),
        ("Pair(a: \"x\")", "Pair<string>"),
    ] {
        let text = format!("{decls} object Q {{ has value = {init}; Q(); }} main() is int {{ 0; }}");
        assert_eq!(field_type(&text, "Q", "value"), expected, "{init}");
    }
    // Inside a generic object the inferred type is the same in every specialization.
    let text = format!("{decls} object Box<T> {{ has count = 0; has item is T; Box(:item); }} main() is int {{ let b = Box(item: \"x\"); 0; }}");
    assert_eq!(field_type(&text, "Box", "count"), "int");
    // Declaration order does not matter: a later object's constructor is known.
    let text = "object A { has b = B(); A(); } object B { has n = 1; B(); } main() is int { 0; }";
    assert_eq!(field_type(text, "A", "b"), "B");
    assert_eq!(field_type(text, "B", "n"), "int");
}

/// #967: an inferred field is an ordinary typed field: shorthand and full
/// constructors, successors and the default itself are checked against it.
#[test]
fn an_inferred_field_is_checked_like_a_written_one() {
    let counter = "object Counter { has value = 1; Counter(:value); Counter(text is string) { value = text.length(); } \
                   get() is int { value; } incremented() is ^Counter { ^value = value + 1; } }";
    let check = |body: &str| messages(&format!("{counter} main() is int {{ {body} 0; }}"));
    for body in ["let c = Counter(value: 5).incremented(); c.get();", "let c = Counter(); c.get();", "let c = Counter(\"abc\"); c.get();"] {
        assert!(check(body).is_empty(), "{body}: {:?}", check(body));
    }
    assert_eq!(
        check("let c = Counter(value: \"five\");"),
        ["Constructor \"Counter\" argument \"value\" must be int, got string"]
    );
    let wrong = |member: &str| messages(&format!("object Counter {{ has value = 1; Counter(); {member} }} main() is int {{ 0; }}"));
    assert_eq!(wrong("again() is ^Counter { ^value = \"x\"; }").len(), 1, "a successor keeps the type");
    assert_eq!(wrong("Counter(text is string) { value = text; }").len(), 1, "a constructor keeps the type");
}

/// #967: no type is inferred when the initializer does not determine one or
/// when there is no initializer; the field is named, at its name, and the
/// initializer's own errors are reported once, without `any`.
#[test]
fn a_field_without_a_determined_type_is_reported_at_its_name() {
    let located_one = |member: &str| located(&format!("object Q {{ {member} Q(); }} main() is int {{ 0; }}"));
    let empty = |name: &str| format!("Field \"{name}\" of object \"Q\" needs a type: an empty [] or [:] in its initializer has no element type; write \"has {name} is T = ...;\"");
    assert_eq!(located_one("has items = [];"), [("items".to_string(), empty("items"))]);
    assert_eq!(located_one("has entries = [:];"), [("entries".to_string(), empty("entries"))]);
    assert_eq!(located_one("has nested = [[]];"), [("nested".to_string(), empty("nested"))]);
    assert_eq!(
        located_one("has span = 1..3;"),
        [("span".to_string(), "Field \"span\" of object \"Q\" needs a type: its initializer has type range, which a field type cannot name; write \"has span is T = ...;\"".to_string())]
    );
    assert_eq!(
        located_one("has value;")[0],
        ("value".to_string(), "Field \"value\" of object \"Q\" needs a type: write \"has value is T;\", or give it an initializer to infer the type from".to_string())
    );
    // The initializer's own error, once; no second message about the field.
    assert_eq!(located_one("has value = missing;"), [("missing".to_string(), "Undefined variable \"missing\"".to_string())]);
    // A constructor's assignment never gives a type.
    assert_eq!(
        messages("object Q { has value; Q(v is int) { value = v; } } main() is int { 0; }")[0],
        "Field \"value\" of object \"Q\" needs a type: write \"has value is T;\", or give it an initializer to infer the type from"
    );
}

/// A case of a plain enum in a catch pattern is "not an error enum", whether it is written
/// with its own name or through a namespace; a case of an error enum is still caught (#983).
#[test]
fn a_plain_enum_case_in_catch_is_not_an_error_enum() {
    assert_eq!(
        messages(
            "enum Color = (red, green); \
             main() is int { let v = try { 1; } catch Color.red { 2; }; v; }"
        ),
        ["Catch pattern belongs to \"Color\", which is not an error enum"]
    );
    assert_eq!(
        messages(
            "enum Color = (red, green); \
             main() is int { let v = try { 1; } catch Color.green(x) { 2; }; v; }"
        ),
        ["Catch pattern belongs to \"Color\", which is not an error enum"]
    );
    assert!(
        messages(
            "enum Bad is Error = (late); \
             main() is int { let v = try { throw Bad.late; } catch Bad.late { 2; }; v; }"
        )
        .is_empty()
    );
}

/// A module constant holding an enum case covers that case in a value `match` (#992): the
/// exhaustiveness check reads the patterns as checked, as the missing-cases message does.
#[test]
fn constant_enum_patterns_count_toward_exhaustiveness() {
    let decls = "enum Color = (red, green, blue); const RED = Color.red;";
    assert!(
        messages(&format!(
            "{decls} main() is int {{ let c = Color.green; \
             let a = match c {{ case RED {{ 1; }} case Color.green {{ 2; }} case Color.blue {{ 3; }} }}; a; }}"
        ))
        .is_empty()
    );
    assert_eq!(
        messages(&format!(
            "{decls} main() is int {{ let c = Color.green; let a = match c {{ case RED {{ 1; }} case Color.green {{ 2; }} }}; a; }}"
        )),
        ["match over enum `Color` does not cover blue, so it has no value here; add the cases or an `else` branch"]
    );
}

/// An overload or constructor that takes the call without filling defaults wins over one that
/// needs its trailing defaults (#993, decided 2026-11-22, as in C# and Kotlin).
#[test]
fn an_overload_without_defaults_beats_one_that_needs_them() {
    assert!(
        messages(
            "object P { P(); f(a is int) is int { a; } f(a is int, b is int = 1) is int { a + b; } } \
             main() is int { P().f(1) + P().f(1, 2); }"
        )
        .is_empty()
    );
    assert!(
        messages(
            "object Q { has a is int; has b is int = 0; Q(:a); Q(a is int, c is int = 7) { a = a; b = c; } } \
             main() is int { let q = Q(a: 5); 0; }"
        )
        .is_empty()
    );
    // Two candidates that both need defaults still tie.
    let found = messages(
        "object P { P(); f(a is int, b is int = 1) is int { a; } f(a is int, c is string = \"x\") is int { a; } } \
         main() is int { P().f(1); }",
    );
    assert!(found.len() == 1 && found[0].starts_with("Ambiguous call"), "{found:?}");
}


#[test]
fn find_takes_the_expected_element_as_a_boundary() {
    // #995: the found item keeps the source's element type and is coerced to
    // an expected `Maybe<U>`'s element, as map's result is; a mismatch is an
    // error at the call, never an untyped value.
    assert_eq!(
        messages("main() is int { let xs = [1, 2, 3]; let d is Maybe<string> = xs.find() -> x { x > 1; }; 0; }"),
        ["Find result must be string, got int"]
    );
    assert_eq!(
        messages("main() is int { let d is Maybe<@string> = [[1]].find() -> x { x.size() > 0; }; 0; }"),
        ["Find result must be @string, got @int"]
    );
    let decls = "role Shape { area() is int; } object Sq does Shape { has s is int; Sq(:s); area() is int { s * s; } }";
    for body in [
        "let d = [1, 2].find() -> x { x > 1; }; let n is Maybe<int> = d;",
        "let d is Maybe<int> = [1, 2].find() -> x { x > 1; };",
        "let d is Maybe<Shape> = [Sq(s: 1)].find() -> x { x.area() > 0; };",
        "let d is Maybe<@int> = [[]].find() -> x { x.size() == 0; };",
    ] {
        let found = messages(&format!("{decls} main() is int {{ {body} 0; }}"));
        assert!(found.is_empty(), "{body}: {found:?}");
    }
}
