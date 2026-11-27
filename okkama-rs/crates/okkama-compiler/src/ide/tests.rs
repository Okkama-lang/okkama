use std::collections::HashMap;

use super::*;
use crate::lowering::{self, LoadedModule, ModuleLoader};
use crate::syntax::{self, ParseOptions, prune};

/// Модули из памяти: путь импорта → текст.
struct Memory(HashMap<String, String>);

impl ModuleLoader for Memory {
    fn load(&mut self, path: &str, _importer: &str) -> Result<Option<LoadedModule>, String> {
        let Some(text) = self.0.get(path) else {
            return Ok(None);
        };
        let source = SourceFile::new(format!("/lib/{path}.ok"), text.clone()).unwrap();
        let program =
            syntax::parse(&source, &ParseOptions::default()).map_err(|_| "syntax".to_string())?;
        Ok(Some(LoadedModule {
            id: source.name().to_string(),
            program: prune::strip_test_items(program),
            source,
        }))
    }
}

const LIBRARY: &str = r#"module shop/money v1.0.0;

// Amount of money in cents.
object Money {
    has cents is int;
    Money(cents is int) { self.cents = cents; }

    // Sum of two amounts.
    plus(other is Money) is Money { Money(cents + other.cents()); }
    cents() is int { cents; }
}

enum Currency = (euro, dollar);
"#;

const MAIN: &str = r#"module app v1.0.0;
import "shop/money" take Money, Currency;

// A wallet holding its total.
object Wallet {
    has total is Money;
    // Bind the initial total.
    Wallet(total is Money) { self.total = total; }
    // Add an amount to the total.
    add(amount is Money) is Money {
        let sum = total.plus(amount);
        sum;
    }
}

main() is int {
    let wallet = Wallet(Money(5));
    let _kind = Currency.euro;
    return wallet.add(Money(7)).cents();
}
"#;

fn inspect(text: &str) -> Inspection {
    let source = SourceFile::new("/app/main.ok", text).unwrap();
    let mut loader = Memory(HashMap::from([(
        "shop/money".to_string(),
        LIBRARY.to_string(),
    )]));
    lowering::inspect(&source, &mut loader, false)
}

#[test]
fn doc_model_drives_imported_hover_completion_links_and_unsaved_overloads() {
    let library = "module shop/money;\n\n// Public [Money] and [Money.plus].\nobject Money {\n    // Construct an amount.\n    Money() {}\n    // Integer amount; see [Money].\n    //\n    // ```text\n    // [NotALink]\n    // ```\n    plus(value is int = 1) is int { value; }\n    // Text amount.\n    plus(value is string) is string { value; }\n    // Never publish this.\n    hidden secret() is void {}\n}\n// Never publish this type.\nhidden object Secret { Secret() {} }\n";
    let text = "module app;\nimport \"shop/money\" take Money;\nmain() is int { let m = Money(); let _s = m.plus(\"x\"); m.plus(2); }\n";
    let file = SourceFile::new("/app/main.ok", text).unwrap();
    let mut loader = Memory(HashMap::from([("shop/money".into(), library.into())]));
    let inspection = lowering::inspect(&file, &mut loader, false);
    let integer = at(&inspection, text, "plus(2)", 0, 1);
    assert!(integer.hover.contains("Integer amount"), "{}", integer.hover);
    assert!(integer.hover.contains("value is int = 1"));
    assert!(!integer.hover.contains("Text amount"));
    assert!(integer.hover.contains("[Money](okkama-doc:/shop/money#Money)"));
    let string = at(&inspection, text, "plus(\"x\")", 0, 1);
    assert!(string.hover.contains("Text amount"));
    assert!(!string.hover.contains("Integer amount"));
    assert_eq!(integer.definition.as_ref().unwrap().range.start.line, 11);
    let members = inspection.members.get("Money").unwrap();
    let completions: Vec<_> = members.iter().filter(|member| member.label == "plus").collect();
    assert_eq!(completions.len(), 2);
    assert!(completions[0].documentation.contains("Integer amount"));
    assert!(completions[0].documentation.contains("value is int = 1"));
    assert!(completions[1].documentation.contains("Text amount"));
    assert!(!members.iter().any(|member| member.label == "secret"));
    let imported = at(&inspection, text, "take Money", 0, 6);
    assert!(imported.hover.contains("Construct an amount"));
    assert!(!imported.hover.contains("Never publish"));
    let library_file = SourceFile::new("/lib/shop/money.ok", library).unwrap();
    let doc = crate::doc::document(&library_file, &mut loader).unwrap();
    let item = doc.item("Money").unwrap();
    let member = &item.members[1];
    let expected_hover = crate::doc::render::hover(&doc, item, Some(member));
    assert_eq!(integer.hover, expected_hover);
    assert_eq!(completions[0].documentation, expected_hover);
    let page = crate::doc::render::item_markdown(&doc, item, Some(&[member]));
    let prose = member.doc.as_ref().unwrap().markdown();
    assert!(page.contains(&prose));
    assert!(integer.hover.contains(&prose));
    assert!(integer.hover.contains("```text\n[NotALink]\n```"));
    assert!(page.contains("Integer amount; see [Money](okkama-doc:/shop/money#Money)."));
    loader.0.insert("shop/money".into(), library.replace("Integer amount", "Updated buffer"));
    let updated = lowering::inspect(&file, &mut loader, false);
    let hover = &at(&updated, text, "plus(2)", 0, 1).hover;
    assert!(hover.contains("Updated buffer"), "{hover}");
    assert!(!hover.contains("Integer amount"));
}

#[test]
fn doc_links_use_import_scopes_explicit_modules_and_warning_ranges() {
    let text = "module app;\r\nimport \"shop/money\" as cash;\r\n\r\n// See [cash.Money], [shop/money.Money.plus], [Money], [shop/money.Nope].\r\n//\r\n// ~~~\r\n// [NotALink]\r\n// ~~~\r\n\r\nobject Public {}\r\n";
    let inspection = inspect(text);
    let warnings: Vec<_> = inspection.diagnostics.iter().filter(|d| d.code.as_deref() == Some("unresolved-doc-link")).collect();
    assert_eq!(warnings.len(), 2, "{:?}", inspection.diagnostics);
    for warning in warnings {
        assert_eq!(warning.severity, Severity::Warning);
        let range = LineIndex::new(text);
        let span = Span::new(range.offset(warning.range.start), range.offset(warning.range.end));
        assert!(["[Money]", "[shop/money.Nope]"].contains(&&text[span.range()]));
    }
}

#[test]
fn doc_model_selects_role_and_constructor_overloads_and_filters_hidden_globals() {
    let text = "module app;\nrole Reading {\n    // Integer role action.\n    read(x is int) is int;\n    // Text role action.\n    read(x is string) is string;\n}\nobject Reader does Reading {\n    // Empty reader.\n    Reader() {}\n    // Reader from a number.\n    Reader(x is int) {}\n    read(x is int) is int { x; }\n    read(x is string) is string { x; }\n}\n// Hidden documentation.\nhidden object Secret { Secret() {} }\nmain() is int {\n    let reader is Reading = Reader(1);\n    let _text = reader.read(\"x\");\n    reader.read(2);\n}\n";
    let inspection = inspect(text);
    assert!(inspection.parsed);
    assert!(!inspection.diagnostics.iter().any(|diagnostic| diagnostic.severity == Severity::Error), "{:?}", inspection.diagnostics);
    let integer = &at(&inspection, text, "read(2)", 0, 1).hover;
    assert!(integer.contains("Integer role action"), "{integer}");
    assert!(!integer.contains("Text role action"));
    let string = &at(&inspection, text, "read(\"x\")", 0, 1).hover;
    assert!(string.contains("Text role action"), "{string}");
    assert!(!string.contains("Integer role action"));
    let constructor = &at(&inspection, text, "Reader(1)", 0, 1).hover;
    assert!(constructor.contains("Reader from a number"), "{constructor}");
    assert!(!constructor.contains("Empty reader"));
    let completions = &inspection.members["Reading"];
    assert!(completions[0].documentation.contains("Integer role action"));
    assert!(completions[1].documentation.contains("Text role action"));
    assert!(!inspection.globals.iter().any(|item| item.label == "Secret"));
}

/// Символ на `occurrence`-м вхождении `needle` (смещение `shift` внутри).
fn at<'a>(
    inspection: &'a Inspection,
    text: &str,
    needle: &str,
    occurrence: usize,
    shift: u32,
) -> &'a Symbol {
    let start = text.match_indices(needle).nth(occurrence).unwrap().0 as u32;
    inspection
        .symbol_at(start + shift)
        .unwrap_or_else(|| panic!("no symbol at {needle:?}#{occurrence}"))
}

#[test]
fn line_index_counts_utf16_units() {
    let text = "ab\nжё🙂x\n";
    let lines = LineIndex::new(text);
    let x = text.find('x').unwrap() as u32;
    assert_eq!(
        lines.position(x),
        Position {
            line: 1,
            character: 4
        }
    );
    assert_eq!(
        lines.offset(Position {
            line: 1,
            character: 4
        }),
        x
    );
    assert_eq!(
        lines.offset(Position {
            line: 0,
            character: 99
        }),
        2
    );
    assert_eq!(
        lines.position(text.len() as u32),
        Position {
            line: 2,
            character: 0
        }
    );
}

#[test]
fn doc_comments_are_the_lines_right_above() {
    let source = SourceFile::new(
        "x.ok",
        "// one\n//   two\nobject A {}\n\n// far\n\nobject B {}\n",
    )
    .unwrap();
    assert_eq!(
        doc_comment(&source, source.text().find("object A").unwrap() as u32),
        "one\n  two"
    );
    assert_eq!(
        doc_comment(&source, source.text().find("object B").unwrap() as u32),
        ""
    );
}

#[test]
fn a_clean_program_has_no_diagnostics_and_an_outline() {
    let inspection = inspect(MAIN);
    assert!(inspection.parsed);
    assert!(
        inspection.diagnostics.is_empty(),
        "{:?}",
        inspection.diagnostics
    );
    let names: Vec<&str> = inspection
        .outline
        .iter()
        .map(|item| item.name.as_str())
        .collect();
    assert_eq!(names, ["Wallet", "main"]);
    let members: Vec<&str> = inspection.outline[0]
        .children
        .iter()
        .map(|item| item.name.as_str())
        .collect();
    assert_eq!(members, ["total", "Wallet", "add"]);
}

#[test]
fn locals_hover_with_their_type_and_lead_to_their_binding() {
    let inspection = inspect(MAIN);
    let symbol = at(&inspection, MAIN, "sum;", 0, 1);
    assert!(
        symbol.hover.contains("let sum is Money"),
        "{}",
        symbol.hover
    );
    let definition = symbol.definition.as_ref().unwrap();
    assert_eq!(definition.path, "/app/main.ok");
    assert_eq!(
        definition.range.start,
        Position {
            line: 10,
            character: 12
        }
    );
}

#[test]
fn methods_of_imported_objects_show_signature_and_doc() {
    let inspection = inspect(MAIN);
    let symbol = at(&inspection, MAIN, "plus", 0, 2);
    assert!(
        symbol.hover.contains("Money.plus(other is Money) is Money"),
        "{}",
        symbol.hover
    );
    assert!(
        symbol.hover.contains("Sum of two amounts."),
        "{}",
        symbol.hover
    );
    let definition = symbol.definition.as_ref().unwrap();
    assert_eq!(definition.path, "/lib/shop/money.ok");
    assert_eq!(
        definition.range.start,
        Position {
            line: 8,
            character: 4
        }
    );
}

#[test]
fn constructors_fields_enum_cases_and_types_resolve() {
    let inspection = inspect(MAIN);
    let ctor = at(&inspection, MAIN, "Money(5)", 0, 1);
    assert!(ctor.hover.contains("Money(cents is int)"), "{}", ctor.hover);
    assert_eq!(ctor.definition.as_ref().unwrap().range.start.line, 5);

    let field = at(&inspection, MAIN, "total.plus", 0, 1);
    assert!(
        field.hover.contains("has total is Money"),
        "{}",
        field.hover
    );
    assert_eq!(
        field.definition.as_ref().unwrap().range.start,
        Position {
            line: 5,
            character: 8
        }
    );

    let case = at(&inspection, MAIN, "euro", 0, 1);
    assert!(case.hover.contains("Currency.euro"), "{}", case.hover);
    assert_eq!(
        case.definition.as_ref().unwrap().range.start,
        Position {
            line: 12,
            character: 17
        }
    );

    let taken = at(&inspection, MAIN, "Money, Currency", 0, 1);
    assert!(taken.hover.contains("object Money"), "{}", taken.hover);
    assert!(
        taken.hover.contains("plus(other is Money) is Money"),
        "{}",
        taken.hover
    );
    assert!(
        taken.hover.contains("Amount of money in cents."),
        "{}",
        taken.hover
    );
    assert_eq!(
        taken.definition.as_ref().unwrap().path,
        "/lib/shop/money.ok"
    );

    let chained = at(&inspection, MAIN, "cents()", 0, 1);
    assert!(
        chained.hover.contains("Money.cents() is int"),
        "{}",
        chained.hover
    );
}

#[test]
fn errors_keep_the_index_and_point_at_their_span() {
    let text = MAIN.replace(
        "let _kind = Currency.euro;",
        "let _kind is string = Currency.euro;",
    );
    let inspection = inspect(&text);
    assert_eq!(
        inspection.diagnostics.len(),
        1,
        "{:?}",
        inspection.diagnostics
    );
    let error = &inspection.diagnostics[0];
    assert_eq!(error.severity, Severity::Error);
    assert_eq!(error.range.start.line, 17);
    assert!(
        at(&inspection, &text, "plus", 0, 1)
            .hover
            .contains("Money.plus")
    );
}

#[test]
fn syntax_errors_are_reported_without_symbols() {
    let inspection = inspect("module app v1.0.0;\nmain() is int { let = 1; }\n");
    assert!(!inspection.parsed);
    assert_eq!(inspection.diagnostics.len(), 1);
    assert_eq!(inspection.diagnostics[0].range.start.line, 1);
    assert!(inspection.symbols.is_empty());
}

#[test]
fn missing_imports_are_reported_on_the_import() {
    let inspection =
        inspect("module app v1.0.0;\nimport \"nowhere\" take X;\nmain() is int { return 0; }\n");
    assert_eq!(
        inspection.diagnostics.len(),
        1,
        "{:?}",
        inspection.diagnostics
    );
    assert_eq!(inspection.diagnostics[0].range.start.line, 1);
}

fn lints(text: &str) -> Vec<(String, u32)> {
    inspect(text)
        .diagnostics
        .iter()
        .filter_map(|found| {
            found
                .code
                .clone()
                .map(|code| (code, found.range.start.line))
        })
        .collect()
}

#[test]
fn lint_reports_unused_imports_lets_hidden_actions_and_unreachable_code() {
    let text = r#"module app v1.0.0;
import "shop/money" take Money, Currency;

object Box {
    has value is int;
    Box(value is int) { self.value = value; }
    get() is int {
        let spare = 1;
        let _kept = 2;
        return value;
        value;
    }
    hidden helper() is int { 1; }
}

main() is int {
    return Box(1).get();
}
"#;
    let found = lints(text);
    assert_eq!(
        found,
        [
            ("unused-import".to_string(), 1),
            ("unused-import".to_string(), 1),
            ("unused-let".to_string(), 7),
            ("unreachable".to_string(), 10),
            ("unused-hidden".to_string(), 12),
            ("missing-docs".to_string(), 3),
            ("missing-docs".to_string(), 5),
            ("missing-docs".to_string(), 6),
        ],
        "{found:?}"
    );
    let diagnostics = inspect(text).diagnostics;
    assert!(
        diagnostics
            .iter()
            .filter(|found| found.code.is_some() && found.code.as_deref() != Some("missing-docs"))
            .all(|found| found.unnecessary)
    );
}

#[test]
fn lint_accepts_used_names_closures_and_snake_case() {
    let text = r#"module app v1.0.0;
import "shop/money" take Money;

object Box {
    has total is Money;
    Box(total is Money) { self.total = total; }
    sum(amounts is @int) is int {
        let base = 1;
        let all is @int = amounts.map() -> amount { amount + base + self.helper(); };
        all.size();
    }
    hidden helper() is int { 1; }
}

main() is int {
    let camelName = 1;
    return Box(Money(camelName)).sum([1]);
}
"#;
    assert_eq!(lints(text), [("snake-case".to_string(), 15), ("missing-docs".to_string(), 3), ("missing-docs".to_string(), 5), ("missing-docs".to_string(), 6)]);
}

#[test]
fn lint_reports_hidden_errors_thrown_by_public_actions() {
    let text = r#"module app v1.0.0;
hidden enum Inner is Error = (broken(code is int));

object Service {
    Service();
    run(n is int) is int {
        confirm n > 0 else throw Inner.broken(n);
        n;
    }
}

main() is int {
    Service().run(1);
}
"#;
    assert_eq!(lints(text), [("hidden-error".to_string(), 6), ("missing-docs".to_string(), 3), ("missing-docs".to_string(), 4), ("missing-docs".to_string(), 5)]);
}

#[test]
fn lint_reports_non_exhaustive_matches_over_plain_enums() {
    let text = r#"module app v1.0.0;
enum Stage = (created, authorized, captured);

main() is int {
    let stage = Stage.created;
    match stage {
        case Stage.created { stage; }
        case Stage.authorized { stage; }
    }
    match stage {
        case Stage.created { stage; }
        else { stage; }
    }
    match stage {
        case Stage.created, Stage.authorized { stage; }
        case Stage.captured { stage; }
    }
    0;
}
"#;
    assert_eq!(lints(text), [("non-exhaustive-match".to_string(), 5), ("missing-docs".to_string(), 1), ("missing-docs".to_string(), 1), ("missing-docs".to_string(), 1), ("missing-docs".to_string(), 1)]);
    let diagnostics = inspect(text).diagnostics;
    assert!(diagnostics[0].message.contains("does not handle captured"));
}

/// Дополнения в позиции после `marker` в `edited` по проверке `MAIN`.
fn complete_after(edited: &str, marker: &str) -> Vec<String> {
    let inspection = inspect(MAIN);
    let offset = (edited.find(marker).unwrap() + marker.len()) as u32;
    let mut labels: Vec<String> = inspection
        .complete(MAIN, edited, offset)
        .into_iter()
        .map(|completion| completion.label)
        .collect();
    labels.sort();
    labels.dedup();
    labels
}

#[test]
fn members_of_locals_and_call_results_complete_while_the_text_does_not_parse() {
    let edited = MAIN.replace("        sum;\n", "        sum.\n");
    assert_eq!(complete_after(&edited, "        sum."), ["cents", "plus"]);
    let chained = MAIN.replace("total.plus(amount);", "total.plus(amount).");
    assert_eq!(
        complete_after(&chained, "total.plus(amount)."),
        ["cents", "plus"]
    );
}

#[test]
fn self_enum_strings_and_take_lists_complete() {
    let with_self = MAIN.replace("        sum;\n", "        self.\n");
    assert_eq!(complete_after(&with_self, "self."), ["add", "total"]);
    let with_enum = MAIN.replace("Currency.euro;", "Currency.;");
    assert_eq!(complete_after(&with_enum, "Currency."), ["dollar", "euro"]);
    let with_string = MAIN.replace("        sum;\n", "        \"x\".\n");
    let labels = complete_after(&with_string, "\"x\".");
    assert!(
        labels.contains(&"split".to_string()) && labels.contains(&"length".to_string()),
        "{labels:?}"
    );
    let take = MAIN.replace("take Money, Currency;", "take ");
    assert_eq!(complete_after(&take, "take "), ["Currency", "Money"]);
}

#[test]
fn plain_names_are_locals_fields_types_and_keywords_in_scope() {
    let edited = MAIN.replace("        sum;\n", "        s\n");
    let labels = complete_after(&edited, "        s");
    for expected in ["amount", "sum", "total", "self", "Money", "Wallet", "let"] {
        assert!(
            labels.contains(&expected.to_string()),
            "{expected} missing in {labels:?}"
        );
    }
    assert!(
        !labels.contains(&"wallet".to_string()),
        "main's local leaked: {labels:?}"
    );
}

#[test]
fn signature_help_names_the_active_parameter() {
    let inspection = inspect(MAIN);
    let edited = MAIN.replace("Wallet(Money(5))", "Wallet(Money(5), ");
    let offset = (edited.find("Wallet(Money(5), ").unwrap() + "Wallet(Money(5), ".len()) as u32;
    let (label, params, active) = inspection.signature(MAIN, &edited, offset).unwrap();
    assert_eq!(label, "Wallet(total is Money)");
    assert_eq!(params, ["total is Money"]);
    assert_eq!(active, 1);
}

#[test]
fn freshly_typed_receivers_resolve_by_name_and_through_calls() {
    let typed = MAIN.replace(
        "        sum;\n",
        "        let fresh = amount;\n        amount.\n",
    );
    assert_eq!(complete_after(&typed, "        amount."), ["cents", "plus"]);
    let chain = MAIN.replace("        sum;\n", "        Money(1).plus(amount).\n");
    assert_eq!(complete_after(&chain, "plus(amount)."), ["cents", "plus"]);
    let field = MAIN.replace("        sum;\n", "        total.\n");
    assert_eq!(complete_after(&field, "        total."), ["cents", "plus"]);
}

#[test]
fn std_paths_are_not_unused_imports() {
    // Загрузчик из памяти ищет `std.shop.money` там же, где обычный импорт.
    let text = r#"module app v1.0.0;

main() is int {
    let wallet = std.shop.money.Money(7);
    let kind is std.shop.money.Currency = std.shop.money.Currency.euro;
    return wallet.cents();
}
"#;
    let found = lints(text);
    assert!(
        !found.iter().any(|(code, _)| code == "unused-import"),
        "{found:?}"
    );
    let errors: Vec<_> = inspect(text)
        .diagnostics
        .into_iter()
        .filter(|found| found.code.is_none())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}

const NAMED: &str = r#"module app v1.0.0;

object Box {
    Box();
    action(name is string, zip is int) is void {}
    size(width is int) is int { width; }
    size(label is string) is int { 0; }
    many(values is ...string) is void {}
}

main() is int {
    let b = Box();
    b.action(name: "x", zip: 1);
    let text = "abc";
    return b.size(width: 1) + text.size();
}
"#;

/// `NAMED`, где вызов `b.action(name: "x", zip: 1)` заменён на `call`; курсор — в конце `call`.
fn named_at(call: &str) -> (Inspection, String, u32) {
    let edited = NAMED.replace("b.action(name: \"x\", zip: 1);", call);
    let offset = (edited.find(call).unwrap() + call.len()) as u32;
    (inspect(NAMED), edited, offset)
}

fn active_at(call: &str) -> (String, usize) {
    let (inspection, edited, offset) = named_at(call);
    let (label, _, active) = inspection.signature(NAMED, &edited, offset).unwrap();
    (label, active)
}

fn argument_names_at(call: &str) -> Vec<String> {
    let (inspection, edited, offset) = named_at(call);
    inspection
        .argument_names(NAMED, &edited, offset)
        .into_iter()
        .map(|found| found.label)
        .collect()
}

#[test]
fn signature_help_follows_argument_names() {
    assert_eq!(active_at("b.action(\"x\", ").1, 1);
    assert_eq!(active_at("b.action(zip: ").1, 1);
    assert_eq!(active_at("b.action(zip: 1, ").1, 0);
    assert_eq!(active_at("b.action(zip: 1, name: \"x\"").1, 0);
    assert_eq!(active_at("b.action(name: \"x\", zi").1, 1);
    // Overloads by type (#1005): the named argument picks the signature before its value is typed.
    assert_eq!(active_at("b.size(label: ").0, "size(label is string) is int");
    assert_eq!(active_at("b.size(width: ").0, "size(width is int) is int");
}

#[test]
fn argument_names_complete_unnamed_parameters() {
    assert_eq!(argument_names_at("b.action("), ["name:", "zip:"]);
    assert_eq!(argument_names_at("b.action(zip: 1, "), ["name:"]);
    assert_eq!(argument_names_at("b.action(zip: 1, na"), ["name:"]);
    let (inspection, edited, offset) = named_at("b.action(");
    let first = inspection.complete(NAMED, &edited, offset).remove(0);
    assert_eq!(
        (first.kind, first.insert.as_str()),
        (CompletionKind::Argument, "name: ")
    );
    assert_eq!(first.detail, "string");
}

#[test]
fn argument_names_are_not_offered_where_names_are_rejected() {
    // Уже позиционный вызов: смешивать нельзя.
    assert!(argument_names_at("b.action(\"x\", ").is_empty());
    // Значение аргумента, а не его начало.
    assert!(argument_names_at("b.action(zip: ").is_empty());
    // Variadic-хвост и встроенные действия — только позиционные.
    assert!(argument_names_at("b.many(").is_empty());
    assert!(argument_names_at("\"abc\".size(").is_empty());
    // Объявление параметров, а не вызов.
    let edited = NAMED.replace(
        "    many(values is ...string) is void {}\n",
        "    action(\n",
    );
    let offset = (edited.find("    action(\n").unwrap() + "    action(".len()) as u32;
    assert!(
        inspect(NAMED)
            .argument_names(NAMED, &edited, offset)
            .is_empty()
    );
}

#[test]
fn a_fresh_name_is_not_taken_for_the_checked_name_it_replaced() {
    // `bb` начинается там же, где в проверенном тексте стоял `b`: это не `b`.
    let (inspection, edited, offset) = named_at("bb.");
    let labels: Vec<String> = inspection
        .complete(NAMED, &edited, offset)
        .into_iter()
        .map(|found| found.label)
        .collect();
    assert!(!labels.contains(&"action".to_string()), "{labels:?}");
    assert!(
        inspection
            .signature(NAMED, &named_at("bb.action(").1, named_at("bb.action(").2)
            .is_none()
    );
}

/// A file with two syntax errors: both reach the editor, coded `syntax`,
/// with the outline of the declarations that parsed (#47).
#[test]
fn syntax_errors_are_all_reported_with_the_outline_of_the_rest() {
    let text = "module app v1.0.0;\n\nobject Broken {\n    has a is int;\n    Broken(:a);\n    value() is int { a +; }\n}\n\nobject Kept {\n    has b is int;\n    Kept(:b);\n}\n\nmain() is int {\n    let = 1;\n}\n";
    let inspection = inspect(text);
    let lines: Vec<u32> = inspection.diagnostics.iter().map(|d| d.range.start.line).collect();
    assert_eq!(lines, [5, 14], "{:?}", inspection.diagnostics);
    assert!(inspection.diagnostics.iter().all(|d| d.code.as_deref() == Some("syntax")));
    assert!(!inspection.parsed);
    let names: Vec<&str> = inspection.outline.iter().map(|item| item.name.as_str()).collect();
    assert_eq!(names, ["Kept"]);
}

/// Positions in every encoding over the hard cases (#48): Cyrillic, emoji
/// (a UTF-16 surrogate pair), combining marks, tabs, LF and CRLF, the end of
/// the file. Every character boundary round-trips byte → position → byte.
#[test]
fn positions_round_trip_in_every_encoding() {
    use crate::ide::{Encoding, LineIndex, Position};
    let text = "привет\tмир\r\n😀 e\u{301}x\n\r\nlast";
    for encoding in [Encoding::Utf8, Encoding::Utf16, Encoding::Utf32] {
        let lines = LineIndex::with_encoding(text, encoding);
        for (offset, ch) in text.char_indices() {
            if ch == '\r' || ch == '\n' {
                continue;
            }
            let position = lines.position(offset as u32);
            assert_eq!(lines.offset(position) as usize, offset, "{encoding:?} {offset} {position:?}");
        }
        assert_eq!(lines.offset(lines.position(text.len() as u32)) as usize, text.len(), "{encoding:?} EOF");
    }
    // The emoji line: `😀` is 4 UTF-8 bytes, 2 UTF-16 units, 1 scalar.
    let after_emoji = text.find(' ').unwrap() as u32;
    for (encoding, character) in [(Encoding::Utf8, 4), (Encoding::Utf16, 2), (Encoding::Utf32, 1)] {
        assert_eq!(LineIndex::with_encoding(text, encoding).position(after_emoji), Position { line: 1, character });
    }
    // `e` + U+0301: the mark is its own unit in every encoding.
    let x = text.find('x').unwrap() as u32;
    assert_eq!(LineIndex::with_encoding(text, Encoding::Utf16).position(x).character, 5);
    let lines = LineIndex::with_encoding(text, Encoding::Utf16);
    // CRLF: the end of a line is before `\r`, and a column past it stays there.
    let crlf = text.find('\r').unwrap() as u32;
    assert_eq!(lines.position(crlf), Position { line: 0, character: 10 });
    assert_eq!(lines.position(crlf + 1), Position { line: 0, character: 10 });
    assert_eq!(lines.offset(Position { line: 0, character: 99 }), crlf);
    // An empty CRLF line, and lines past the end.
    assert_eq!(lines.offset(Position { line: 2, character: 3 }) as usize, text.find("\n\r\n").unwrap() + 1);
    assert_eq!(lines.offset(Position { line: 9, character: 0 }) as usize, text.len());
    // A column inside a surrogate pair lands after the character.
    assert_eq!(lines.offset(Position { line: 1, character: 1 }) as usize, text.find(' ').unwrap());
}

/// Members after every link of a chain (#225): a field through `self`,
/// indexing, generic builtin results, functional-block bindings and locals
/// typed since the last successful check.
#[test]
fn chains_complete_through_fields_indexing_generics_bindings_and_new_locals() {
    let base = MAIN.replace("        let sum = total.plus(amount);\n", "        let sum = total.plus(amount);\n        let all = [sum, total];\n");
    let complete = |edited: &str, marker: &str| -> Vec<String> {
        let inspection = inspect(&base);
        let offset = (edited.find(marker).unwrap_or_else(|| panic!("{marker}")) + marker.len()) as u32;
        let mut labels: Vec<String> = inspection.complete(&base, edited, offset).into_iter().map(|c| c.label).collect();
        labels.sort();
        labels.dedup();
        labels
    };
    let edit = |line: &str| base.replace("        sum;\n", &format!("        {line}\n"));
    let money = ["cents", "plus"];
    assert_eq!(complete(&edit("self.total."), "self.total."), money);
    assert!(complete(&edit("self.total.cents()."), "self.total.cents().").contains(&"abs".to_string()));
    assert_eq!(complete(&edit("all[0]."), "all[0]."), money);
    assert_eq!(complete(&edit("all.first()."), "all.first()."), money);
    assert_eq!(complete(&edit("all.map() -> m { m. };"), "m { m."), money);
    let fresh = edit("let c = sum.cents();\n        c.");
    assert!(complete(&fresh, "        c.").contains(&"abs".to_string()), "{:?}", complete(&fresh, "        c."));
    let typed = edit("let d is Money = sum;\n        d.");
    assert_eq!(complete(&typed, "        d."), money);
}


#[test]
fn missing_docs_is_a_user_warning_and_ignores_private_implementation() {
    let text = "module app;\nobject Public {\n    Public();\n    hidden helper() is int { 1; }\n}\nhidden object Secret { Secret(); }\nmain() is int { 0; }\n";
    let inspection = inspect(text);
    let found: Vec<_> = inspection.diagnostics.iter().filter(|d| d.code.as_deref() == Some("missing-docs")).collect();
    assert_eq!(found.len(), 2, "{:?}", inspection.diagnostics);
    assert!(found.iter().all(|d| d.severity == Severity::Warning && !d.unnecessary));
    assert_eq!(found[0].range.start.line, 1);
    assert_eq!(found[1].range.start.line, 2);
}

/// #1008: a local captured by an anonymous action is the same symbol inside
/// it, so its occurrences (and a rename) reach into the closure.
#[test]
fn a_captured_local_is_one_symbol_with_its_closure_uses() {
    let text = "module app v1.0.0;\nmain() is int {\n    let base = 2;\n    let f = (x is int) is int { x + base; };\n    f(1) + base;\n}\n";
    let inspection = inspect(text);
    let at_definition = text.find("base").unwrap() as u32 + 1;
    let found = inspection.occurrences(at_definition);
    let inside = text.find("x + base").unwrap() as u32 + 4;
    assert_eq!(found.len(), 3, "{:?}", found.iter().map(|symbol| symbol.span).collect::<Vec<_>>());
    assert!(found.iter().any(|symbol| symbol.span.start == inside), "the closure's use is an occurrence");
}

/// #1008: same-named types from two modules keep their own members: after
/// `b.` for a `two.Item`, completion offers `beta`, not `one.Item`'s `alpha`.
#[test]
fn same_named_types_of_two_modules_keep_their_members() {
    let one = "module one v1.0.0;\nobject Item {\n    Item();\n    alpha() is int { 1; }\n}\n";
    let two = "module two v1.0.0;\nobject Item {\n    Item();\n    beta() is int { 2; }\n}\n";
    let text = "module app v1.0.0;\nimport \"one\";\nimport \"two\";\nmain() is int {\n    let a = one.Item();\n    let b = two.Item();\n    a.alpha() + b.beta();\n}\n";
    let source = SourceFile::new("/app/main.ok", text).unwrap();
    let mut loader = Memory(HashMap::from([("one".to_string(), one.to_string()), ("two".to_string(), two.to_string())]));
    let inspection = lowering::inspect(&source, &mut loader, false);
    let offset = text.find("b.beta").unwrap() + 2;
    let edited = format!("{}{}", &text[..offset], &text[offset + 4..]);
    let labels: Vec<String> = inspection.complete(text, &edited, offset as u32).into_iter().map(|c| c.label).collect();
    assert!(labels.iter().any(|label| label == "beta"), "{labels:?}");
    assert!(!labels.iter().any(|label| label == "alpha"), "{labels:?}");
}

/// #1008: a rename cannot produce a program that does not parse.
#[test]
fn rename_refuses_every_reserved_word() {
    for word in ["as", "const", "fake", "with", "isa", "super", "finally", "abstract", "protected", "self", "none", "let"] {
        assert!(!super::renamable_to(word), "{word}");
    }
    for word in ["limit", "_tmp", "asset", "constant", "x2"] {
        assert!(super::renamable_to(word), "{word}");
    }
}

/// #1008: snake-case reaches every local: a statement-level `loop`, a
/// functional block's binding, a `let` inside an `if` expression and an
/// anonymous action's parameter; a declared parameter is warned once.
#[test]
fn snake_case_reaches_locals_at_any_depth() {
    let text = "module app v1.0.0;\nobject Box {\n    Box();\n    sum(itemsIn is @int) is int {\n        let topCamel = 1;\n        loop {\n            let loopCamel = 2;\n            last;\n        }\n        let doubled = itemsIn.map() -> itemCamel { itemCamel * 2; };\n        let picked = if topCamel > 0 { let blockCamel = 3; blockCamel; } else { 0; };\n        let f = (argCamel is int) is int { argCamel; };\n        doubled.size() + picked + f(1);\n    }\n}\nmain() is int { Box().sum([1]); }\n";
    let lines: Vec<u32> = lints(text).into_iter().filter(|(code, _)| code == "snake-case").map(|(_, line)| line).collect();
    assert_eq!(lines, [3, 4, 6, 9, 10, 11], "{lines:?}");
}

/// #1008: a comment block no declaration owns is a `doc-note` warning, not
/// only a line in the JSON export.
#[test]
fn an_unowned_comment_block_is_a_doc_note() {
    let text = "module app v1.0.0;\n\n// The app.\n\nimport \"shop/money\" take Money;\n\nmain() is int {\n    0;\n}\n\n// Left here, owned by nothing.\n";
    let notes: Vec<u32> = lints(text).into_iter().filter(|(code, _)| code == "doc-note").map(|(_, line)| line).collect();
    assert!(!notes.is_empty(), "{:?}", lints(text));
}

/// #1008: module constants are offered by name completion with their type.
#[test]
fn module_constants_are_completed() {
    let text = "module app v1.0.0;\n\n// The most items.\nconst LIMIT = 3;\n\nmain() is int {\n    LIMIT;\n}\n";
    let inspection = inspect(text);
    let offset = text.rfind("LIMIT").unwrap() + 2;
    let edited = format!("{}{}", &text[..offset], &text[offset + 3..]);
    let found = inspection.complete(text, &edited, offset as u32);
    let limit = found.iter().find(|completion| completion.label == "LIMIT").unwrap_or_else(|| panic!("{:?}", found.iter().map(|c| &c.label).collect::<Vec<_>>()));
    assert_eq!(limit.detail, "const LIMIT is int");
}

/// #1006: a module constant folds into its value in THIR, yet its uses keep a
/// hover with its type and docs and lead to its declaration.
#[test]
fn a_constant_use_hovers_and_leads_to_its_declaration() {
    let text = "module app v1.0.0;\n\n// The answer.\nconst ANSWER is int = 42;\nconst NAMES = [\"a\", \"b\"];\n\nmain() is int {\n    return ANSWER + NAMES.size();\n}\n";
    let inspection = inspect(text);
    let answer = at(&inspection, text, "ANSWER", 1, 1);
    assert!(answer.hover.contains("const ANSWER is int"), "{}", answer.hover);
    assert!(answer.hover.contains("The answer."), "{}", answer.hover);
    let definition = answer.definition.as_ref().expect("a definition");
    assert_eq!(definition.range.start.line, 3);
    let names = at(&inspection, text, "NAMES", 1, 1);
    assert!(names.hover.contains("const NAMES is @string"), "{}", names.hover);
    assert_eq!(names.definition.as_ref().expect("a definition").range.start.line, 4);
}


/// #1041: a call's result uses the same qualified key as its member table.
#[test]
fn same_named_call_results_keep_their_members() {
    let one = "module one; object Item { Item(); alpha() is int { 1; } twin() is Item { Item(); } }";
    let two = "module two; role Copying { twin() is Item; } object Item does Copying { Item(); beta() is int { 2; } twin() is Item { Item(); } }";
    let text = "module app;\nimport \"one\";\nimport \"two\";\nmain() is int {\n    let a = one.Item();\n    let b = two.Item();\n    let r is two.Copying = b;\n    b.beta();\n}\n";
    let source = SourceFile::new("/app/main.ok", text).unwrap();
    let mut loader = Memory(HashMap::from([("one".into(), one.into()), ("two".into(), two.into())]));
    let inspection = lowering::inspect(&source, &mut loader, false);
    assert!(!inspection.diagnostics.iter().any(|d| d.severity == Severity::Error), "{:?}", inspection.diagnostics);
    for (receiver, expected, absent) in [("a", "alpha", "beta"), ("b", "beta", "alpha"), ("r", "beta", "alpha")] {
        for chain in [format!("{receiver}.twin()."), format!("{receiver}.twin().twin().")] {
            let edited = text.replace("b.beta();", &chain);
            let offset = edited.find(&chain).unwrap() + chain.len();
            let labels: Vec<_> = inspection.complete(text, &edited, offset as u32).into_iter().map(|c| c.label).collect();
            assert!(labels.iter().any(|label| label == expected), "{chain}: {labels:?}");
            assert!(!labels.iter().any(|label| label == absent), "{chain}: {labels:?}");
        }
    }
}

#[test]
fn self_completion_is_unique_and_only_in_object_scope() {
    let inspection = inspect(MAIN);
    for (needle, expected) in [("        sum;", 1), ("    return wallet", 0)] {
        let offset = MAIN.find(needle).unwrap() + needle.len() - needle.trim_start().len();
        let found = inspection.complete(MAIN, MAIN, offset as u32);
        assert_eq!(found.iter().filter(|item| item.label == "self").count(), expected);
    }
    assert!(!renamable_to("self"));
}

#[test]
fn qualified_constant_hover_and_definition_only_cover_the_constant_name() {
    let text = "module app;\nimport \"limits\" as lim;\nmain() is int { lim.MOST; }\n";
    let library = "module limits;\n// Maximum count.\nconst MOST = 3;\n";
    let source = SourceFile::new("/app/main.ok", text).unwrap();
    let mut loader = Memory(HashMap::from([("limits".into(), library.into())]));
    let inspection = lowering::inspect(&source, &mut loader, false);
    let symbol = at(&inspection, text, "lim.MOST", 0, 5);
    assert_eq!(&text[symbol.span.range()], "MOST");
    assert!(symbol.hover.contains("Maximum count."));
    assert_eq!(symbol.definition.as_ref().unwrap().range.start.line, 2);
    let alias = text.find("lim.MOST").unwrap() as u32;
    for offset in alias..alias + 4 {
        assert!(!inspection.symbols.iter().any(|symbol| symbol.span.start <= offset && offset < symbol.span.end && symbol.hover.contains("const MOST")));
    }
}

#[test]
fn synopsis_skips_doctests_in_explicit_and_implicit_sections() {
    for heading in ["", "# Synopsis\n\n"] {
        for fence in ["okkama,run", "okkama run"] {
            let overview = format!("Overview.\n\n{heading}```{fence}\nexample();\n```\n\n```output\nresult\n```\n\n    intended();\n\nDescription.\n");
            let comments: String = overview.lines().map(|line| format!("// {line}\n")).collect();
            let doc = crate::doc::ModuleDoc::parse("app.ok".into(), format!("module app;\n\n{comments}\nobject A {{}}\n")).unwrap();
            let rendered = crate::doc::render::module_markdown(&doc);
            let synopsis = rendered.split("## Synopsis").nth(1).expect(&rendered).split("## Description").next().unwrap();
            assert!(synopsis.contains("intended();"), "{rendered}");
            assert!(!synopsis.contains("example();"), "{rendered}");
            assert!(rendered.contains("example();"), "{rendered}");
            assert!(!rendered.lines().any(|line| !line.is_empty() && line.trim().is_empty()), "{rendered}");
        }
    }
}

#[test]
fn synopsis_with_only_doctests_keeps_them_as_examples() {
    let doc = crate::doc::ModuleDoc::parse("app.ok".into(), "module app;\n\n// # Synopsis\n//\n// ```okkama run\n// example();\n// ```\n\nobject A {}\n".into()).unwrap();
    let rendered = crate::doc::render::module_markdown(&doc);
    assert!(!rendered.contains("## Synopsis"), "{rendered}");
    assert!(rendered.contains("example();"), "{rendered}");
}


#[test]
fn std_overlay_updates_both_overview_and_declarations() {
    let directory = std::env::temp_dir().join(format!("okkama-1042-overlay-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let file = directory.join("sample.ok");
    std::fs::write(&file, "module sample;\n\n// Disk overview.\n\nobject Old {}\n").unwrap();
    let text = "std.Ne";
    let overlay = HashMap::from([(file.canonicalize().unwrap(), std::rc::Rc::from("module sample;\n\n// Buffer overview.\n\n// Buffer type.\nobject New {}\n"))]);
    let found = super::std_completions(text, text.len() as u32, &super::DiskStd::with_overlay(&directory, &overlay)).unwrap();
    let module = found.iter().find(|item| item.label == "sample").unwrap();
    assert_eq!(module.documentation, "Buffer overview.");
    let declaration = found.iter().find(|item| item.label == "sample.New").unwrap();
    assert!(declaration.documentation.contains("Buffer type."));
    assert!(!found.iter().any(|item| item.label.contains("Old")));
    let disk = super::std_completions("std.Ol", 6, &super::DiskStd::new(&directory)).unwrap();
    assert!(disk.iter().any(|item| item.label == "sample.Old"));
    assert!(!disk.iter().any(|item| item.label.contains("New")));
    std::fs::remove_file(file).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

/// #1117: a successor action's completion detail prints its result as the declaration does, `^Bottle` with one
/// caret, as hover does, and a chain through the successor still completes the object's members.
#[test]
fn a_successor_action_completes_with_its_declared_result() {
    let text = "module app v1.0.0;\n\n// A bottle.\nobject Bottle {\n    has opened is bool = false;\n    Bottle() {}\n    // Opens it.\n    open() is ^Bottle { ^opened = true; }\n    state() is string { \"closed\"; }\n}\n\nmain() is int {\n    let bottle = Bottle();\n    let poured = bottle.open();\n    0;\n}\n";
    let inspection = inspect(text);
    let offset = text.find("bottle.open").unwrap() + "bottle.".len();
    let edited = format!("{}{}", &text[..offset], &text[offset + "open()".len()..]);
    let found = inspection.complete(text, &edited, offset as u32);
    let open = found.iter().find(|item| item.label == "open").unwrap_or_else(|| panic!("{found:?}"));
    assert_eq!(open.detail, "() is ^Bottle");
    let state = found.iter().find(|item| item.label == "state").unwrap();
    assert_eq!(state.detail, "() is string");
    let hover = &at(&inspection, text, "open()", 1, 1).hover;
    assert!(hover.contains("open() is ^Bottle") && !hover.contains("^^"), "{hover}");
    let chained = text.replace("bottle.open();", "bottle.open().");
    let offset = chained.find("bottle.open().").unwrap() + "bottle.open().".len();
    let labels: Vec<String> = inspection.complete(text, &chained, offset as u32).into_iter().map(|item| item.label).collect();
    assert!(labels.contains(&"state".to_string()) && labels.contains(&"open".to_string()), "{labels:?}");
}

/// #1128: an editor's answers come from the last analysed inspection. A text that parses but whose import does not
/// resolve, or that does not parse, is not analysed: the earlier inspection keeps answering, with its symbols.
#[test]
fn answers_come_from_the_last_analysed_inspection() {
    let mut answers = super::Answering::default();
    assert!(answers.answering().is_none());
    let good = inspect(MAIN);
    assert!(good.parsed && good.analysed && !good.symbols.is_empty());
    let good_text: std::rc::Rc<str> = std::rc::Rc::from(MAIN);
    answers.record(&good_text, &std::rc::Rc::new(good));
    for (text, parsed) in [
        (MAIN.replace("shop/money", "shop/missing"), true),
        (MAIN.replace("let _kind = Currency.euro;", "let _kind = std.coll;"), true),
        (MAIN.replace("let _kind = Currency.euro;", "wallet."), false),
    ] {
        let inspection = inspect(&text);
        assert_eq!(inspection.parsed, parsed, "{text}");
        assert!(!inspection.analysed && inspection.symbols.is_empty(), "{text}");
        assert!(!inspection.diagnostics.is_empty(), "{text}");
        answers.record(&std::rc::Rc::from(text.as_str()), &std::rc::Rc::new(inspection));
        let (answering_text, answering) = answers.answering().unwrap();
        assert_eq!(&**answering_text, MAIN, "{text}");
        assert!(!answering.symbols.is_empty(), "{text}");
    }
    let changed = MAIN.replace("let _kind = Currency.euro;", "let _kind = Currency.dollar;");
    let inspection = inspect(&changed);
    assert!(inspection.analysed, "an analysed text with an error in its body still answers");
    answers.record(&std::rc::Rc::from(changed.as_str()), &std::rc::Rc::new(inspection));
    assert_eq!(&**answers.answering().unwrap().0, changed);
}
