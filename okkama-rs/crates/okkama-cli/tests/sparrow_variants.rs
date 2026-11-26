//! Variant values (payloads, fallbacks, matches) compiled to SPB and run on Sparrow.
#[path = "sparrow_support/mod.rs"]
mod support;
use okkama_compiler::{SourceFile, ir, lowering, syntax};

fn lower(body: &str) -> Result<ir::Module, String> {
    let source = SourceFile::new("variants.ok", format!("module app v1.0.0; {body}")).unwrap();
    let ast =
        syntax::parse(&source, &syntax::ParseOptions::default()).map_err(|e| format!("{e:?}"))?;
    lowering::lower_program(&ast).map_err(|e| format!("{e:?}"))
}
fn check(source: &str, expected: &str) {
    let module = lower(source).unwrap();

    support::assert_value(&module, expected);
}
#[test]
fn payload_patterns_and_generic_inference() {
    check(
        "enum Reading<T> = (empty, value(item is T), pair(left is T, right is T)); main() is int { let r = Reading.pair(20, 22); match r { case Reading.empty { 0; } case Reading.value(n) { n; } case Reading.pair(a, b) { a + b; } } }",
        "42",
    );
}
#[test]
fn optional_fallback_is_lazy_and_preserves_zero() {
    check(
        "main() is int { let a = Maybe.some(0) else panic \"unexpected\"; let b = Maybe<int>.none else 42; a + b; }",
        "42",
    );
}
#[test]
fn presence_preserves_all_three_states() {
    check(
        "object Reader { Reader(); get() is Presence<int> { Presence.missing; } } main() is int { match Reader().get() { case Presence.some(v) { v; } case Presence.none { 1; } case Presence.missing { 42; } } }",
        "42",
    );
}
#[test]
fn variant_diagnostics() {
    for (source, diagnostic) in [
        ("main() is int { let a = Maybe.none; 0; }", "Cannot infer"),
        (
            "main() is int { Maybe.some(1) else \"bad\"; 0; }",
            "Fallback",
        ),
        (
            "main() is int { Presence<int>.missing else 1; }",
            "requires Maybe",
        ),
        (
            "main() is int { match Maybe.some(1) { case Maybe.some(n) { n; } } }",
            "Non-exhaustive",
        ),
    ] {
        let error = lower(source).unwrap_err();
        assert!(error.contains(diagnostic), "{error}");
    }
}

#[test]
fn equality_is_nominal_and_compares_only_the_active_payload() {
    check(
        "enum Pair<T> = (empty, two(a is T, b is T)); main() is int { let a = Pair.two(20, 22); if a == Pair.two(20, 22) && a != Pair.two(21, 22) && a != Pair<int>.empty { 42; } else { 0; } }",
        "42",
    );
    check(
        "main() is int { let a is Maybe<Maybe<int>> = Maybe.some(Maybe.none); if a != Maybe.none && a == Maybe.some(Maybe<int>.none) { 42; } else { 0; } }",
        "42",
    );
}

#[test]
fn explicit_conversions_and_empty_values() {
    check(
        "main() is int { let a = Presence<string>.missing.fold() else \"missing\"; let b = Presence.some(\"\").fold() else \"wrong\"; let c = Maybe.some(false).as_presence(); if a == \"missing\" && b == \"\" && c == Presence.some(false) { 42; } else { 0; } }",
        "42",
    );
}

#[test]
fn method_results_use_fallback_or_payload_patterns() {
    check(
        r#"object User { User(); name() is Maybe<string> { Maybe.some(""); } }
        main() is int {
            let obj = User();
            let a = obj.name() else "no name";
            let b = match obj.name() {
                case Maybe.none { "no name"; }
                case Maybe.some(value) { value; }
            };
            let c = Presence<string>.none.fold() else "no name";
            if a == "" && b == "" && c == "no name" { 42; } else { 0; }
        }"#,
        "42",
    );
    for source in [
        "main() is int { Maybe.some(42).some else 0; }",
        "main() is int { Presence<int>.missing.collapse() else 0; }",
    ] {
        assert!(lower(source).is_err(), "must reject: {source}");
    }
}

#[test]
fn values_survive_collections_and_generic_calls() {
    check(
        "object Box<T> { has item is T; Box(:item); get() is T { item; } } main() is int { let values is @Maybe<int> = [Maybe.none, Maybe.some(42)]; let b = Box(item: values[1]); b.get() else 0; }",
        "42",
    );
}

#[test]
fn fallback_can_throw_and_does_not_catch_errors_from_the_left() {
    check(
        "enum Absent is Error = (absent); main() is int { try { Maybe<int>.none else throw Absent.absent; } catch e is Absent { 42; } }",
        "42",
    );
    check(
        "enum Failed is Error = (failed); object Reader { Reader(); get() is Maybe<int> { throw Failed.failed; } } main() is int { try { Reader().get() else 0; } catch e is Failed { 42; } }",
        "42",
    );
}

#[test]
fn bad_patterns_and_recursive_variants_are_rejected() {
    for (source, diagnostic) in [
        (
            "enum Recursive = (link(value is Recursive)); main() is int { 0; }",
            "Recursive variant",
        ),
        (
            "enum A = (link(value is Maybe<A>)); main() is int { 0; }",
            "Recursive variant",
        ),
        (
            "main() is int { match Maybe.some(1) { case Maybe.some(a) { a; } case Maybe.some(b) { b; } case Maybe.none { 0; } } }",
            "Duplicate variant",
        ),
        (
            "main() is int { match Maybe.some(1) { case Maybe.some(a, b) { 0; } case Maybe.none { 0; } } }",
            "number of variant",
        ),
        ("main() is int { let a = [Maybe.some(1): 1]; 0; }", "key"),
        (
            "object Bad { Bad(); get() is string { Maybe.some(\"x\"); } } main() is int { 0; }",
            "string",
        ),
    ] {
        let error = lower(source).unwrap_err();
        assert!(
            error.to_lowercase().contains(&diagnostic.to_lowercase()),
            "{error}"
        );
    }
}

#[test]
fn variants_compare_inside_containers_without_unbound_fields() {
    check(
        "main() is int { let a is @Maybe<int> = [Maybe.none, Maybe.some(42)]; let b is @Maybe<int> = [Maybe.none, Maybe.some(42)]; if a == b { 42; } else { 0; } }",
        "42",
    );
}

#[test]
fn payload_and_fallback_cross_role_boundaries() {
    check(
        "role Reading { read() is int; } object Item does Reading { Item(); read() is int { 42; } } main() is int { let item is Maybe<Reading> = Maybe.some(Item()); let value = item else Item(); value.read(); }",
        "42",
    );
}

#[test]
fn formatter_preserves_variant_payloads_and_fallback() {
    let source = SourceFile::new("format.ok", "module app v1.0.0; enum Reading<T>=(empty,value(item is T)); main() is int { let a = Maybe<int>.none else 42; a; }".to_string()).unwrap();
    let formatted = syntax::format::format(&source).unwrap();
    check(formatted.split_once(';').unwrap().1, "42");
    let twice =
        syntax::format::format(&SourceFile::new("format.ok", formatted.clone()).unwrap()).unwrap();
    assert_eq!(formatted, twice);
}

// #893: one match, two case forms. The compact form `case pattern: value;` mixes with block cases; the removed
// `?? !!` selection is a syntax error.
#[test]
fn compact_match_cases_choose_lazily_and_keep_payloads() {
    check(
        r#"
        enum Color = (red, blue);
        main() is int {
            let a = match true { case true: 20; case false: panic "unused"; };
            let b = match false { case true: panic "unused"; case false: 22; };
            let name = match Presence.some("") { case Presence.some(v): v; case Presence.none: "null"; case Presence.missing: "missing"; };
            let n = match Maybe<int>.none { case Maybe.some(v): v; case _: 42; };
            let color = match Color.red { case Color.red: 42; case Color.blue: 0; };
            if name == "" && n == 42 && color == 42 { a + b; } else { 0; }
        }
    "#,
        "42",
    );
}

#[test]
fn compact_and_block_cases_mix_nest_and_go_anywhere_an_expression_goes() {
    check(
        r#"
        enum Shape = (dot, square(side is int), pair(left is int, right is int));
        object Box { has v is int; Box(:v); value() is int { v; } }
        main() is int {
            let one = match Shape.dot { case Shape.dot: 1; case _: 0; };
            let mixed = match Shape.square(5) {
                case Shape.dot: 0;
                case Shape.square(side) {
                    let doubled = side * 2;
                    doubled;
                }
                case Shape.pair(l, r): l + r;
            };
            let nested = match Shape.pair(2, 3) { case Shape.pair(l, r): match l > r { case true: l; case false: r; }; case _: 0; };
            let box = Box(v: match Maybe.some(7) { case Maybe.some(x): x; case Maybe.none: 0; });
            let listed = [match true { case true: 1; case false: 2; }, 3];
            let once = match [1, 2].size() { case 2: 10; case 1: 20; else { 30; } };
            if one == 1 && mixed == 10 && nested == 3 && box.value() == 7 && listed[0] == 1 && once == 10 { 42; } else { 0; }
        }
    "#,
        "42",
    );
}

#[test]
fn compact_cases_can_throw_and_keep_failures() {
    check(
        r#"
        enum Absent is Error = (absent);
        main() is int {
            try { match Presence<int>.missing { case Presence.some(v): v; case _: throw Absent.absent; }; }
            catch e is Absent { 42; }
        }
    "#,
        "42",
    );
}

#[test]
fn compact_cases_keep_match_rules() {
    for (source, message) in [
        ("enum Color = (red, blue); main() is int { let c = match Color.red { case Color.red: 1; }; c; }", "does not cover blue"),
        ("main() is int { match Maybe.some(1) { case Maybe.some(v): v; case Maybe.none: \"x\"; }; }", ""),
        ("main() is int { let x = match Maybe.some(1) { case Maybe.some(v): v; case Maybe.none: v; }; x; }", "Undefined variable"),
        ("enum Color = (red, blue); main() is int { match Color.red { case Color.red: 1; case Color.red: 2; case Color.blue: 3; }; }", "Duplicate variant pattern"),
        ("main() is int { match 1 { case 1: 1 case 2: 2; else { 0; } }; }", ""),
    ] {
        let error = lower(source).expect_err(source);
        assert!(error.contains(message), "{source}: {error}");
    }
}

#[test]
fn the_removed_choice_syntax_is_a_syntax_error() {
    for source in ["true ?? 1 !! 2", "Maybe.some(1) ?? .some(v): v !! .none: 0"] {
        let error = lower(&format!("main() is int {{ {source}; }}")).unwrap_err();
        assert!(error.contains("?? / !! selection was removed"), "{source}: {error}");
    }
}

#[test]
fn compact_match_formatter_roundtrip() {
    let code = "module app v1.0.0; main() is int { let a = match true { case true: 20; case false: 0; }; let b = match Maybe.some(22) { case Maybe.some(v): v; case _: 0; }; a+b; }";
    let source = SourceFile::new("compact.ok", code.to_owned()).unwrap();
    let formatted = syntax::format::format(&source).unwrap();
    assert!(formatted.contains("case true: 20;") && formatted.contains("case _: 0;"), "{formatted}");
    check(formatted.split_once(';').unwrap().1, "42");
    let twice =
        syntax::format::format(&SourceFile::new("compact.ok", formatted.clone()).unwrap()).unwrap();
    assert_eq!(formatted, twice);
}

#[test]
fn tail_expression_semicolons_are_optional_without_changing_values() {
    check("main() is int { if true { 42 } else { 0 }; }", "42");
    check(
        "main() is int { let a = if true { 20 } else { 0 }; let b = Maybe.some(22); match b { case Maybe.some(v) { a + v } case Maybe.none { 0 } } }",
        "42",
    );
    check(
        "object Answer { Answer(); value() is int { 42 } } main() is int { Answer().value() }",
        "42",
    );
    for source in [
        "main() is int { 1 42 }",
        "main() is int { let a = 42 }",
        "main() is int { return 42 }",
        "main() is int { defer 1 }",
    ] {
        assert!(lower(source).is_err(), "accepted: {source}");
    }
    let source = SourceFile::new(
        "tail.ok",
        "module app v1.0.0; main() is int { if true { 42 } else { 0 }; }".to_string(),
    )
    .unwrap();
    let formatted = syntax::format::format(&source).unwrap();
    assert!(!formatted.contains("42;"), "{formatted}");
    check(formatted.split_once(';').unwrap().1, "42");
    let twice =
        syntax::format::format(&SourceFile::new("tail.ok", formatted.clone()).unwrap()).unwrap();
    assert_eq!(formatted, twice);
}

#[test]
fn postfix_functionals_compose_with_optional_and_compact_matches() {
    check(
        r#"main() is int {
            let total = 1 + [20, 21].reduce(0) -> sum, value { sum + value };
            let fallback = Maybe<int>.none else [20, 22].reduce(0) -> sum, value { sum + value };
            let selected = if total == 42 { [20, 22].reduce(0) -> sum, value { sum + value }; } else { 0; };
            let values = [Maybe.some(42), Maybe<int>.none].map() -> item { item else 42 };
            for (values.map() -> value { match value == 42 { case true: value; case false: 0; } }) -> value {
                if value != 42 { return 0; }
            }
            if total == fallback && fallback == selected { total } else { 0 };
        }"#,
        "42",
    );
}
