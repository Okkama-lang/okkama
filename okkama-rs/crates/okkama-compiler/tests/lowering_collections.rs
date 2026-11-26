//! Lowering массивов, словарей и строк: сборщики, циклы обхода,
//! `first()`/`last()`, `m[k] else` и диагностика в формате Perl-интерпретатора.

use okkama_compiler::SourceFile;
use okkama_compiler::ir;
use okkama_compiler::lowering::{LoweringError, lower_program};
use okkama_compiler::syntax::{self, ParseOptions, prune};

fn lower(body: &str) -> Result<ir::Module, LoweringError> {
    let text = format!("module app v1.0.0; main() is int {{ {body} }}");
    let source = SourceFile::new("test.ok", &text).expect("small source");
    let program = syntax::parse(&source, &ParseOptions::default()).expect("valid syntax");
    okkama_compiler::lowering::with_passes("all", || lower_program(&prune::strip_test_items(program)))
}

fn main_listing(body: &str) -> String {
    let module = lower(body).unwrap_or_else(|error| panic!("{body}: {error}"));
    ir::verify(&module).expect("lowering output verifies");
    let text = module.to_string();
    let start = text.find("func @main").expect("main is printed");
    // Только `main`: lowering по достижимости печатает её первой.
    let end = text[start..]
        .find("\n}\n")
        .map_or(text.len(), |end| start + end + 3);
    text[start..end].to_string()
}

#[test]
fn map_threads_a_linear_builder_through_the_loop_header() {
    assert_eq!(
        main_listing("let xs = [1, 2].map() -> v { v * 2; }; xs.size();"),
        concat!(
            "func @main() -> int [entry] {\n",
            "^b0:\n",
            "    %0: int = const 1\n",
            "    %1: int = const 2\n",
            "    %2: array<int> = array.new [%0, %1]\n",
            "    %3: builder<int> = builder.new\n",
            "    %4: int = const 0\n",
            "    %5: int = array.len %2\n",
            // Константы тела цикла вынесены во вход (`ir::hoist`).
            "    %10: int = const 2\n",
            "    %13: int = const 1\n",
            "    br ^b1(%4, %3)\n",
            "^b1(%6: int, %7: builder<int>):\n",
            "    %8: bool = int.lt %6, %5\n",
            "    cond %8, ^b2, ^b3\n",
            // Шаг тела и переход к заголовку — один блок (`ir::cfg::merge_blocks`).
            "^b2:\n",
            "    %9: int = array.get %2, %6\n",
            "    %11: int = int.mul %9, %10\n",
            "    %12: builder<int> = builder.push %7, %11\n",
            "    %14: int = int.add %6, %13\n",
            "    br ^b1(%14, %12)\n",
            "^b3:\n",
            "    %15: array<int> = builder.finish %7\n",
            "    %16: int = array.len %15\n",
            "    ret %16\n",
            "}\n",
        )
    );
}

#[test]
fn first_and_last_trap_with_the_empty_sequence_text() {
    assert_eq!(
        main_listing("[4, 5].first();"),
        concat!(
            "func @main() -> int [entry] {\n",
            "^b0:\n",
            "    %0: int = const 4\n",
            "    %1: int = const 5\n",
            "    %2: array<int> = array.new [%0, %1]\n",
            "    %3: int = const 0\n",
            "    array.lookup %2, %3, ^b1, ^b2\n",
            "^b1(%4: int):\n",
            "    ret %4\n",
            "^b2:\n",
            "    trap empty \"Cannot read first item of an empty sequence\"\n",
            "}\n",
        )
    );
    let last = main_listing("[4, 5].last();");
    assert!(last.contains("int.sub"), "{last}");
    assert!(
        last.contains("trap empty \"Cannot read last item of an empty sequence\""),
        "{last}"
    );
}

#[test]
fn collection_rule_violations_are_semantic_errors() {
    for (body, expected) in [
        (
            "\"ab\".greater(\"a\"); 0;",
            "Type \"string\" has no builtin action \"greater\"",
        ),
        (
            "\"ab\".greater_or_equal(\"a\"); 0;",
            "Type \"string\" has no builtin action \"greater_or_equal\"",
        ),
        (
            "let xs = [3, 1]; let ys = @int(0, xs..., \"9\"); 0;",
            "@int() argument 3: Cannot convert string to int",
        ),
        ("int(\"9\");", "Cannot convert string to int"),
        (
            "let m = [\"a\": 1]; for m -> k { 0; } 0;",
            // #955: the message of every other source, and a map's way in.
            "For source must be an array, range, Iterating or Batching, got %string:int; iterate a map through keys() or values()",
        ),
        (
            "let m = [\"a\": 1]; m.map() -> v { v; }; 0;",
            "Map map requires exactly 2 bindings, got 1",
        ),
        (
            "let m = [\"a\": 1]; m.reduce(0) -> acc, v { acc; };",
            "Map reduce requires exactly 3 bindings, got 2",
        ),
        (
            "[1].map() -> a, b { a; }; 0;",
            "Array map requires exactly 1 binding, got 2",
        ),
        (
            "let e = [:]; e.filter() -> k, v { true; }; 0;",
            "Functional map operations require a typed %K:V receiver",
        ),
        (
            "let e = []; e.map() -> v { v; }; 0;",
            "Cannot infer map result type from empty source",
        ),
        (
            "let m = [\"a\": 1]; m[1];",
            "Map key must be string, got int",
        ),
        (
            "let x is @int = 1; 0;",
            "Cannot initialize variable \"x\" of type @int with int",
        ),
    ] {
        let error = lower(body).unwrap_err();
        assert!(
            matches!(error, LoweringError::Invalid { .. }),
            "{body}: {error:?}"
        );
        assert_eq!(error.message(), expected, "{body}");
    }
}

#[test]
fn map_literals_and_lookups_have_their_own_instructions() {
    assert_eq!(
        main_listing("let m = [\"b\": 1, \"a\": 2]; m[\"c\"] else m.size();"),
        concat!(
            "func @main() -> int [entry] {\n",
            "^b0:\n",
            "    %0: str = const \"b\"\n",
            "    %1: int = const 1\n",
            "    %2: str = const \"a\"\n",
            "    %3: int = const 2\n",
            "    %4: map<str,int> = map.new [%0: %1, %2: %3]\n",
            "    %5: str = const \"c\"\n",
            "    map.lookup %4, %5, ^b1, ^b2\n",
            "^b1(%6: int):\n",
            "    br ^b3(%6)\n",
            "^b2:\n",
            "    %7: int = map.len %4\n",
            "    br ^b3(%7)\n",
            "^b3(%8: int):\n",
            "    ret %8\n",
            "}\n",
        )
    );
    // `filter` словаря собирает новый словарь линейным `mbuilder`.
    let filtered =
        main_listing("let m = [1: true, 2: false]; let f = m.filter() -> k, v { v; }; f.size();");
    for expected in [
        "map.keys %",
        "map.values %",
        "mbuilder<int,bool> = mbuilder.new",
        "mbuilder.put %",
        "map<int,bool> = mbuilder.finish %",
    ] {
        assert!(filtered.contains(expected), "{expected} in\n{filtered}");
    }
    // `[]`/`[:]` в ветви получают тип другой ветви.
    let branches = main_listing(
        "let f = 1 > 0; let m = if f { [1: true]; } else { [:]; }; \
         let xs = if f { []; } else { [2]; }; m.size() + xs.size();",
    );
    assert!(
        branches.contains("map<int,bool> = map.retag %"),
        "{branches}"
    );
    assert!(
        branches.contains("array<int> = array.retag %"),
        "{branches}"
    );
}

/// #994: an untagged `last` value is retagged to the loop's joined type, as a
/// branch value is, and the IR verifies.
#[test]
fn loop_exits_retag_untagged_last_values() {
    for (body, retag) in [
        (
            "let a = true; let v = loop { if a { last [[]]; } last [[1]]; }; v.size();",
            "array<array<int>> = array.retag %",
        ),
        (
            "let a = true; let v = loop { if a { last [:]; } last [\"k\": 1]; }; v.size();",
            "map<str,int> = map.retag %",
        ),
    ] {
        let main = main_listing(body);
        assert!(main.contains(retag), "{body}\n{main}");
    }
}

/// #1002: `any`, `all` and `find` over an untagged `[]` have no item to
/// decide, so they give their defaults (`false`, `true`, `Maybe.none`), as
/// map and filter give `[]`; the IR verifies.
#[test]
fn early_exits_over_an_untagged_empty_array_give_their_defaults() {
    for (body, default) in [
        ("let a = [].any() -> x { true; }; if a { 1; } else { 0; }", "bool = const false"),
        ("let a = [].all() -> x { false; }; if a { 1; } else { 0; }", "bool = const true"),
        ("let e = []; let a = e.any() -> x { true; }; if a { 1; } else { 0; }", "bool = const false"),
        (
            "let k is Maybe<@int> = [].find() -> x { true; }; match k { case Maybe.some(v) { 1; } case Maybe.none { 0; } }",
            "= variant.new 1 ()",
        ),
    ] {
        let main = main_listing(body);
        assert!(main.contains(default), "{body}\n{main}");
        assert!(!main.contains("array.len"), "no loop over the empty source: {body}\n{main}");
    }
}

#[test]
fn constants_outside_loops_stay_in_their_branch() {
    let listing = main_listing("let n = \"abcdefg\".length(); if n > 3 { 40 + n; } else { 50; }");
    let entry = listing.split("^b1").next().expect("entry block");
    assert!(!entry.contains("const 40") && !entry.contains("const 50"), "{listing}");
    assert!(listing.contains("const 40") && listing.contains("const 50"), "{listing}");
}

#[test]
fn a_literal_if_keeps_the_analyzed_collection_type() {
    // #298: only the selected `[]` is emitted, retagged to `@int`; no branch.
    for (body, retag) in [
        ("let ok = (if false { [1]; } else { []; }) == [3]; 0;", "array<int> = array.retag"),
        ("let ok = (if true { []; } else { [1]; }) == [3]; 0;", "array<int> = array.retag"),
        ("let ok = (if false { [\"k\": 1]; } else { [:]; }) == [\"j\": 2]; 0;", "map<str,int> = map.retag"),
    ] {
        let main = main_listing(body);
        assert!(main.contains(retag), "{body}\n{main}");
        assert!(!main.contains("cond "), "{body}\n{main}");
    }
    // A variable condition still branches and joins with the tagged type.
    let main = main_listing("let c = true; let ok = (if c { [1]; } else { []; }) == [3]; 0;");
    assert!(main.contains("cond "), "{main}");
}

/// #969: `join` reads no element, so an untyped `[]` (a literal or a variable
/// bound to one) is joined as an empty `@string`: the IR retags it to
/// `array<str>` before `array.join` and verifies.
#[test]
fn join_of_an_untyped_empty_array_joins_an_empty_string_array() {
    for body in [
        "let s = [].join(\",\"); s.length();",
        "let e = []; let s = e.join(\", \"); s.length();",
    ] {
        let listing = main_listing(body);
        let join = listing.lines().find(|line| line.contains("array.join")).unwrap_or_else(|| panic!("{body}:\n{listing}"));
        let array = join.split("array.join ").nth(1).unwrap().split(',').next().unwrap().trim();
        assert!(
            listing.lines().any(|line| line.trim_start().starts_with(&format!("{array}: array<str> = "))),
            "{body}: the joined array is array<str>\n{listing}"
        );
    }
}

/// Loads std from the repository, so a program can `import "compare"`.
struct Std;

impl okkama_compiler::lowering::ModuleLoader for Std {
    fn load(&mut self, path: &str, _importer: &str) -> Result<Option<okkama_compiler::lowering::LoadedModule>, String> {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../std").join(format!("{path}.ok"));
        let text = std::fs::read_to_string(&file).map_err(|error| format!("{}: {error}", file.display()))?;
        let source = SourceFile::new(file.to_string_lossy(), text).expect("std source");
        let program = syntax::parse(&source, &ParseOptions::default()).map_err(|error| format!("{error:?}"))?;
        Ok(Some(okkama_compiler::lowering::LoadedModule {
            id: source.name().to_string(), source, program: prune::strip_test_items(program),
        }))
    }
}

/// `main` of a program that imports `Comparing`, lowered with std and verified.
fn compared_main(body: &str) -> String {
    let text = format!("module app v1.0.0; import \"compare\" take Comparing; main() is int {{ {body} }}");
    let source = SourceFile::new("test.ok", text).expect("small source");
    let program = syntax::parse(&source, &ParseOptions::default()).expect("valid syntax");
    let module = okkama_compiler::lowering::lower_linked(&prune::strip_test_items(program), source.name(), &mut Std)
        .unwrap_or_else(|error| panic!("{body}: {error:?}"));
    ir::verify(&module).unwrap_or_else(|error| panic!("{body}: {error}"));
    let text = module.to_string();
    let start = text.find("func @main").expect("main is printed");
    let end = text[start..].find("\n}\n").map_or(text.len(), |end| start + end + 3);
    text[start..end].to_string()
}

/// #971: an untyped `[]` has no element to compare, so its ordering is an
/// empty array (as `[].join(",")` is "", #969): `main` builds a fresh empty
/// array and never calls the runtime sort, which needs an ordered element.
#[test]
fn ordering_of_an_untyped_empty_array_is_an_empty_array() {
    for body in [
        "let o = [].ordering(); o.size();",
        "let e = []; let o = e.ordering(); o.size();",
    ] {
        let listing = compared_main(body);
        assert!(!listing.contains("okkama.array.ordering"), "{body}: no runtime sort\n{listing}");
        let size = listing.lines().find(|line| line.contains("array.len")).unwrap_or_else(|| panic!("{body}:\n{listing}"));
        let array = size.split("array.len ").nth(1).unwrap().trim();
        assert!(
            listing.lines().any(|line| line.trim_start() == format!("{array}: array<never> = array.new []")),
            "{body}: the ordering is a fresh empty array\n{listing}"
        );
    }
}

/// #969: the typed empty array joins as before, and an array that is not of
/// string is still rejected: the retag covers only the untyped `[]`.
#[test]
fn join_keeps_typed_receivers_and_rejects_non_string_elements() {
    main_listing("let xs is @string = []; let s = xs.join(\",\"); s.length();");
    for (body, expected) in [
        ("let s = [1, 2].join(\",\"); 0;", "Builtin action \"join\" requires an array of string, got @int"),
        ("let s = [[]].join(\",\"); 0;", "Builtin action \"join\" requires an array of string"),
    ] {
        let error = lower(body).unwrap_err();
        assert!(matches!(error, LoweringError::Invalid { .. }), "{body}: {error:?}");
        assert!(error.message().starts_with(expected), "{body}: {}", error.message());
    }
}

/// #971: a typed empty array is still sorted by the runtime, by its element order.
#[test]
fn ordering_of_a_typed_empty_array_still_sorts() {
    for body in [
        "let xs is @int = []; let o = xs.ordering(); o.size();",
        "let o = [3, 1, 2].ordering(); o.size();",
    ] {
        let listing = compared_main(body);
        assert!(listing.contains("okkama.array.ordering"), "{body}: the runtime sorts\n{listing}");
    }
}
