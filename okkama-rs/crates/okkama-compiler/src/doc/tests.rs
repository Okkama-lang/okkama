use super::render::{Style, item_markdown, item_text, module_markdown, module_text};
use super::{ItemKind, MemberKind, ModuleDoc};

#[test]
fn declaration_links_resolve_public_overload_sets_and_skip_markdown_code() {
    let source = "module links;\r\n\r\n// Читай [Thing] и [Thing.write]; [Missing], [Secret], [Thing.secret].\r\n//\r\n// ```okkama\r\n// [Fenced]\r\n// ```\r\n//\r\n//     [Indented]\r\n//\r\n// `[Inline]`, [site](https://example.org), [ordinary words].\r\n\r\nobject Thing {\r\n    write(x is int) is void {}\r\n    write(x is string) is void {}\r\n    hidden secret() is void {}\r\n}\r\nhidden object Secret {}\r\n";
    let file = crate::source::SourceFile::new("links.ok", source).unwrap();
    let mut loader = NoModules;
    let doc = super::document(&file, &mut loader).unwrap();
    let text = doc.overview.as_ref().unwrap();
    assert_eq!(text.links.len(), 5, "{:?}", text.links);
    let link = &text.links[1];
    assert_eq!(link.target.as_deref(), Some("okkama-doc:/links#Thing.write"));
    assert_eq!(link.declarations.len(), 2);
    assert_eq!(doc.link_warnings.len(), 3);
    for warning in &doc.link_warnings {
        assert_eq!(&source[warning.span.range()], format!("[{}]", warning.name));
    }
    let markdown = module_markdown(&doc);
    assert!(markdown.lines().nth(2).unwrap().contains("[Thing](okkama-doc:/links#Thing)"));
    assert!(markdown.contains("[Thing](okkama-doc:/links#Thing)"), "{markdown}");
    assert!(markdown.contains("[Thing.write](okkama-doc:/links#Thing.write)"));
    assert!(markdown.contains("[Fenced]\n```"));
    assert!(markdown.contains("[Indented]"));
    assert!(markdown.contains("`[Inline]`"));
    let plain = module_text(&doc, &Style::default());
    assert!(plain.contains("<okkama-doc:/links#Thing.write>"), "{plain}");
}

struct NoModules;

impl crate::lowering::ModuleLoader for NoModules {
    fn load(&mut self, _: &str, _: &str) -> Result<Option<crate::lowering::LoadedModule>, String> {
        Ok(None)
    }
}

#[test]
fn search_matches_paths_public_names_and_summaries_without_bodies() {
    let doc = doc("module demo/path;\n\n// Module overview.\n\n// A special résumé.\nobject Thing {\n    // Writes bytes. More detail is not indexed.\n    write() is void {}\n    // Hidden needle.\n    hidden secret() is void {}\n}\n// Hidden needle.\nhidden object Secret {}\n");
    for query in ["DEMO/PATH", "thing", "RÉSUMÉ", "writes bytes"] {
        assert!(!doc.search(query).is_empty(), "{query}");
    }
    assert!(doc.search("needle").is_empty());
    assert!(doc.search("More detail").is_empty());
    assert!(doc.search("not present").is_empty());
}

#[test]
fn links_keep_commonmark_links_images_and_code_and_report_separate_header_spans() {
    let text = "module m;\n\n// First [Missing].\n\n// Second [Absent].\n//\n// [Thing][ref], [Thing](https://example.org), ![Thing], \\[Thing], `[Thing]`.\n//\n// [ref]: https://example.org\n\nobject Thing {}\n";
    let source = crate::source::SourceFile::new("m.ok", text).unwrap();
    let doc = super::document(&source, &mut NoModules).unwrap();
    assert_eq!(doc.link_warnings.len(), 2, "{:?}", doc.link_warnings);
    for warning in &doc.link_warnings {
        assert_eq!(&text[warning.span.range()], format!("[{}]", warning.name));
    }
    let markdown = module_markdown(&doc);
    assert!(markdown.contains("[Thing][ref]"));
    assert!(markdown.contains("![Thing]"));
    assert!(!markdown.contains("okkama-doc:/m#Thing"), "{markdown}");
}

#[test]
fn links_use_public_inventory_across_files_and_keep_stable_versioned_targets() {
    use crate::analysis::registry::{Registry, SourceModule};
    use crate::source::SourceFile;
    use crate::syntax::{self, ParseOptions};
    let files = [
        SourceFile::new("a.ok", "module multi v1.2.3;\n// See [Second] and [multi.Second].\nobject First {}\n").unwrap(),
        SourceFile::new("b.ok", "module multi v1.2.3;\n// See [First].\nobject Second {}\n").unwrap(),
    ];
    let programs: Vec<_> = files.iter().map(|file| syntax::parse(file, &ParseOptions::default()).unwrap()).collect();
    let registry = Registry::build_files(programs.iter().map(|program| SourceModule {
        path: "multi".into(), program, imports: Vec::new(),
    }).collect(), 2);
    let docs = super::Documentation::new(&registry, &files, &mut NoModules);
    let link = &docs.modules[0].item("First").unwrap().doc.as_ref().unwrap().links[0];
    assert_eq!(link.target.as_deref(), Some("okkama-doc:/multi?version=v1.2.3#Second"));
    assert_eq!(link.declarations[0].file, "b.ok");
    let qualified = &docs.modules[0].item("First").unwrap().doc.as_ref().unwrap().links[1];
    assert_eq!(qualified.target, link.target);
    assert!(docs.modules.iter().all(|doc| doc.link_warnings.is_empty()));
}

fn doc(text: &str) -> ModuleDoc {
    ModuleDoc::parse("demo.ok".to_string(), text.to_string()).unwrap_or_else(|error| panic!("{error}"))
}

const CANONICAL: &str = r#"module demo/api v1.2.3;

// The demo API: things to show.
//
//     let thing = Thing("x");
//     thing.name();

import "collections/sequence" take Sequencing;

// A thing with a name.
object Thing<T: Sequencing<string>> does Named {
    has origin is string;

    // Made from its name.
    Thing(name is string) {
        origin = name;
    }

    // Made from parts; the separator is "/" unless told.
    Thing(parts is @string, separator is string = "/") {
        origin = "x";
    }

    // The parts joined.
    joined(parts is ...string) is string {
        "";
    }

    // The name.
    name() is string {
        // An implementation note, not documentation.
        origin;
    }

    // Written somewhere.
    async write(bytes is @byte) is void {}

    // Written as text.
    async write(text is string) is void {}

    hidden secret() is string {
        "no";
    }

    cap Head {
        status is int {
            1;
        }
    }
}

hidden object Internal {
    Internal() {}
}

// Things with names.
role Named {
    // The name.
    name() is string;
}

// The thing could not be made.
enum ThingFailed is Error = (because(reason is string), empty);

enum Colour = (red, green);

// The answer.
const ANSWER = 42;

hidden const SECRET = 1;
"#;

#[test]
fn module_overview_after_the_module_line_is_the_canonical_place() {
    let doc = doc(CANONICAL);
    assert_eq!(doc.path, "demo/api");
    assert_eq!(doc.version.as_deref(), Some("v1.2.3"));
    let overview = doc.overview.as_ref().unwrap();
    assert!(overview.text.starts_with("The demo API: things to show.\n\n    let thing"));
    assert_eq!(doc.summary(), "The demo API: things to show.");
    assert!(doc.notes.is_empty(), "{:?}", doc.notes);
}

#[test]
fn declarations_own_the_comment_right_above_and_hidden_ones_vanish() {
    let doc = doc(CANONICAL);
    let names: Vec<&str> = doc.items.iter().map(|item| item.name.as_str()).collect();
    assert_eq!(names, ["Thing", "Named", "ThingFailed", "Colour", "ANSWER"]);
    let thing = doc.item("Thing").unwrap();
    assert_eq!(thing.signature, "object Thing<T: Sequencing<string>> does Named");
    assert_eq!(thing.doc.as_ref().unwrap().text, "A thing with a name.");
    let members: Vec<(&str, MemberKind)> = thing.members.iter().map(|member| (member.name.as_str(), member.kind)).collect();
    assert_eq!(members, [
        ("Thing", MemberKind::Constructor),
        ("Thing", MemberKind::Constructor),
        ("joined", MemberKind::Action),
        ("name", MemberKind::Action),
        ("write", MemberKind::Action),
        ("write", MemberKind::Action),
        ("Head", MemberKind::Cap),
        ("Head.status", MemberKind::Field),
    ]);
    assert_eq!(thing.members[1].signature, "Thing(parts is @string, separator is string = \"/\")");
    assert_eq!(thing.members[2].signature, "joined(parts is ...string) is string");
    assert_eq!(thing.members[4].signature, "async write(bytes is @byte) is void");
    assert_eq!(thing.members[4].doc.as_ref().unwrap().text, "Written somewhere.");
    assert_eq!(thing.members[3].doc.as_ref().unwrap().text, "The name.");
    assert_eq!(thing.members[6].signature, "cap Head { status is int }");
    assert!(thing.members[6].doc.is_none());
    assert_eq!(thing.members_named("write").len(), 2);
    let named = doc.item("Named").unwrap();
    assert_eq!(named.members[0].signature, "name() is string");
    assert_eq!(named.members[0].doc.as_ref().unwrap().text, "The name.");
    let failed = doc.item("ThingFailed").unwrap();
    assert_eq!(failed.kind, ItemKind::Error);
    assert_eq!(failed.signature, "enum ThingFailed is Error = (because(reason is string), empty)");
    assert_eq!(doc.item("Colour").unwrap().kind, ItemKind::Enum);
    assert!(doc.item("Colour").unwrap().doc.is_none());
    assert_eq!(doc.item("ANSWER").unwrap().signature, "const ANSWER = 42");
}

#[test]
fn a_legacy_overview_after_the_imports_is_kept() {
    let doc = doc(
        "module old/style;\n\nimport \"io/stream\" take Reading;\n\n// Old overview.\n//\n// More.\n\n// The first object.\nobject First {\n    First() {}\n}\n",
    );
    assert_eq!(doc.version, None);
    assert_eq!(doc.overview.as_ref().unwrap().text, "Old overview.\n\nMore.");
    assert_eq!(doc.item("First").unwrap().doc.as_ref().unwrap().text, "The first object.");
    assert!(doc.notes.is_empty());
}

#[test]
fn a_comment_right_above_the_first_declaration_is_never_the_overview() {
    let doc = doc("module m;\n\nimport \"a\" take A;\n\n// Only this object's.\nobject First {\n    First() {}\n}\n");
    assert!(doc.overview.is_none());
    assert_eq!(doc.item("First").unwrap().doc.as_ref().unwrap().text, "Only this object's.");
}

#[test]
fn an_overview_on_both_sides_of_the_imports_is_kept_whole_and_reported() {
    let doc = doc("module m;\n\n// Before.\n\nimport \"a\" take A;\n\n// After.\n\nobject First {\n    First() {}\n}\n");
    assert_eq!(doc.overview.as_ref().unwrap().text, "Before.\n\nAfter.");
    assert_eq!(doc.notes.len(), 1);
    assert!(doc.notes[0].message.contains("before and after the imports"));
}

#[test]
fn blank_lines_end_a_comment_and_detached_comments_are_reported() {
    let doc = doc(
        "module m;\n\n// Overview.\n\nobject First {\n    First() {}\n}\n\n// Loose words.\n\n// The second.\nobject Second {\n    Second() {}\n}\n",
    );
    assert_eq!(doc.item("Second").unwrap().doc.as_ref().unwrap().text, "The second.");
    assert!(doc.item("First").unwrap().doc.is_none());
    assert_eq!(doc.notes.len(), 1);
    assert!(doc.notes[0].message.contains("belongs to no declaration"));
}

#[test]
fn slashes_in_strings_are_not_comments_and_crlf_and_unicode_survive() {
    let text = "module m;\r\n\r\n// Ünïcode overview: «чай».\r\n\r\nobject First {\r\n    // The URL.\r\n    url() is string {\r\n        \"http://example.org\";\r\n    }\r\n}\r\n\r\nconst NOTE = ```line\r\n// not a comment\r\n// nor this```;\r\nobject Second {\r\n    Second() {}\r\n}\r\n";
    let doc = doc(text);
    assert_eq!(doc.overview.as_ref().unwrap().text, "Ünïcode overview: «чай».");
    let first = doc.item("First").unwrap();
    assert_eq!(first.members[0].doc.as_ref().unwrap().text, "The URL.");
    // The `//` line inside the string literal documents nothing.
    assert!(doc.item("Second").unwrap().doc.is_none());
}

#[test]
fn code_blocks_in_comments_stay_text_of_the_comment() {
    let doc = doc("module m;\n\n// Example:\n//\n// ```okkama\n// let x = 1; // inside a block\n// ```\n\nobject First {\n    First() {}\n}\n");
    assert_eq!(doc.overview.as_ref().unwrap().text, "Example:\n\n```okkama\nlet x = 1; // inside a block\n```");
}

#[test]
fn the_text_page_has_its_sections_in_order() {
    let page = module_text(&doc(CANONICAL), &Style::default());
    let order: Vec<usize> = ["NAME", "VERSION", "SYNOPSIS", "PUBLIC API", "ERRORS"]
        .iter()
        .map(|title| page.find(&format!("\n{title}\n")).or_else(|| page.starts_with(title).then_some(0)).unwrap_or_else(|| panic!("{title} in\n{page}")))
        .collect();
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{page}");
    assert!(page.contains("    demo/api - The demo API: things to show.\n"));
    assert!(page.contains("    v1.2.3\n"));
    assert!(page.contains("SYNOPSIS\n    let thing = Thing(\"x\");\n    thing.name();\n"), "{page}");
    assert!(page.contains("    object Thing<T: Sequencing<string>> does Named\n        A thing with a name.\n"));
    assert!(page.contains("        async write(text is string) is void\n            Written as text.\n"));
    assert!(!page.contains("secret"));
    assert!(!page.contains("Internal"));
    assert!(!page.contains("SECRET"));
    assert!(!page.contains("implementation note"));
    assert!(!page.contains('\u{1b}'));
    // The synopsis code is not repeated in the description.
    assert_eq!(page.matches("thing.name();").count(), 1);
}

#[test]
fn a_page_without_a_version_shows_none() {
    let page = module_text(&doc("module m;\n\nobject First {\n    First() {}\n}\n"), &Style::default());
    assert!(!page.contains("VERSION"));
    assert!(!page.contains("v0.0.0"));
}

#[test]
fn ansi_styles_only_whole_lines() {
    let page = module_text(&doc(CANONICAL), &Style { width: 80, ansi: true });
    assert!(page.contains("\u{1b}[1mNAME\u{1b}[0m\n"));
    assert!(page.contains("    \u{1b}[1mobject Thing<T: Sequencing<string>> does Named\u{1b}[0m\n"));
}

#[test]
fn lists_wrap_with_a_hanging_indent_and_text_wraps_at_the_width() {
    let doc = doc("module m;\n\n// Words:\n// - one two three four five six seven eight nine ten eleven twelve\n// - short\n\nobject First {\n    First() {}\n}\n");
    let page = module_text(&doc, &Style { width: 40, ansi: false });
    assert!(page.contains("    - one two three four five six seven\n      eight nine ten eleven twelve\n    - short\n"), "{page}");
    assert!(page.lines().all(|line| line.chars().count() <= 40 || line.trim_start().starts_with("object")), "{page}");
}

#[test]
fn a_declaration_page_and_an_action_page() {
    let doc = doc(CANONICAL);
    let thing = doc.item("Thing").unwrap();
    let page = item_text(&doc, thing, None, &Style::default());
    assert!(page.starts_with("NAME\n    demo/api Thing - A thing with a name.\n"), "{page}");
    assert!(page.contains("DECLARATION\n    object Thing"));
    let writes = thing.members_named("write");
    let page = item_text(&doc, thing, Some(&writes), &Style::default());
    assert!(page.starts_with("NAME\n    demo/api Thing.write - Written somewhere.\n"), "{page}");
    assert!(page.contains("async write(bytes is @byte) is void"));
    assert!(page.contains("async write(text is string) is void"));
    assert!(!page.contains("name() is string"));
}

#[test]
fn markdown_keeps_comments_as_written_and_fences_signatures() {
    let doc = doc(CANONICAL);
    let page = module_markdown(&doc);
    assert!(page.starts_with("# demo/api\n\nThe demo API: things to show.\n\n**Version:** `v1.2.3`\n"), "{page}");
    assert!(page.contains("## Synopsis\n\n```okkama\nlet thing = Thing(\"x\");\nthing.name();\n```\n"));
    assert!(page.contains("### object Thing\n\n```okkama\nobject Thing<T: Sequencing<string>> does Named\n```\n\nA thing with a name.\n"));
    assert!(page.contains("#### Thing.write\n\n```okkama\nasync write(bytes is @byte) is void\n```\n"));
    assert!(page.contains("## Errors\n\n### enum ThingFailed"));
    let item = item_markdown(&doc, doc.item("Named").unwrap(), None);
    assert!(item.starts_with("# demo/api Named\n"));
}

#[test]
fn a_long_signature_breaks_after_commas() {
    let doc = doc("module m;\n\nenum Kind = (null_value, boolean_value, number_value, string_value, array_value, object_value);\n");
    let page = module_text(&doc, &Style { width: 60, ansi: false });
    assert!(page.contains("    enum Kind = (null_value, boolean_value, number_value,\n        string_value, array_value, object_value)\n"), "{page}");
}

#[test]
fn a_long_comment_glued_to_the_first_declaration_is_reported_not_guessed() {
    let doc = doc("module m;\n\nimport \"a\" take A;\n\n// Line one.\n// Line two.\n// Line three.\nobject First {\n    First() {}\n}\n");
    assert!(doc.overview.is_none());
    assert_eq!(doc.item("First").unwrap().doc.as_ref().unwrap().text, "Line one.\nLine two.\nLine three.");
    assert_eq!(doc.notes.len(), 1);
    assert!(doc.notes[0].message.contains("may be the module overview"));
}

#[test]
fn errors_text_and_error_enums_stand_apart() {
    let doc = doc("module m;\n\n// Things.\n//\n// # Errors\n//\n// Bad input is Bad.\n\n// Bad input.\nenum Bad is Error = (input);\n");
    let page = module_text(&doc, &Style::default());
    assert!(page.contains("ERRORS\n    Bad input is Bad.\n\n    enum Bad is Error = (input)\n"), "{page}");
}


#[test]
fn doctest_kinds_output_and_exact_unicode_crlf_locations() {
    use super::examples::ExampleKind;
    let text = "module demo;\r\n\r\n// Résumé.\r\n//\r\n// ```okkama,ignore\r\n// unfinished …\r\n// ```\r\n//\r\n// ```okkama\r\n// let café = 1;\r\n// ```\r\n//\r\n// ```okkama,run\r\n// main() is int { 0; }\r\n// ```\r\n// ```output\r\n// ready\r\n// ```\r\n\r\nobject Demo {}\r\n";
    let doc = doc(text);
    let examples = doc.examples().unwrap();
    assert_eq!(examples.len(), 3);
    assert_eq!(examples[0].kind, ExampleKind::Illustrative);
    assert_eq!(examples[1].kind, ExampleKind::Compile);
    assert_eq!(examples[2].kind, ExampleKind::Run);
    assert_eq!(examples[2].expected.as_deref(), Some("ready\n"));
    let at = examples[1].code.find("café").unwrap();
    let span = examples[1].source_span(crate::source::Span::new(at as u32, (at + "café".len()) as u32));
    assert_eq!(&text[span.range()], "café");
    assert_eq!(examples[1].code, "let café = 1;\n");
}

#[test]
fn doctest_metadata_never_silently_skips_marked_examples() {
    for body in [
        "// ```okkama,run\n// 0;\n// ```\n",
        "// ```okkama,typo\n// 0;\n// ```\n",
        "// ```okkama run\n// 0;\n// ```\n",
        "// ```okkama,compile,run\n// 0;\n// ```\n",
    ] {
        assert!(doc(&format!("module demo;\n{body}\nobject X {{}}\n")).examples().is_err());
    }
    let parsed = doc("module demo;\n// ```okkama\n// main() is int { panic \"never execute\"; }\n// ```\n//\n//     fragment …\n//\n// ```python\n// raise Exception()\n// ```\n\nobject X {}\n");
    let examples = parsed.examples().unwrap();
    assert_eq!(examples.iter().filter(|e| e.kind == super::examples::ExampleKind::Compile).count(), 1);
    assert!(examples.iter().all(|e| e.kind != super::examples::ExampleKind::Run));
}

#[test]
fn missing_docs_counts_real_declarations_overloads_cases_caps_and_hidden() {
    let text = "module demo;\n// Public.\nobject Public {\n    // Construct.\n    Public();\n    // First overload.\n    read(x is int) is int { x; }\n    read(x is string) is string { x; }\n    hidden secret() is void {}\n    // A view.\n    cap View {\n        value is int { 1; }\n    }\n}\n// States.\nenum State = (\n    // Ready.\n    ready,\n    waiting\n);\nhidden object Secret { Secret(); action() is void {} }\n// Reader.\nrole Reader { read() is int; }\n";
    let coverage = doc(text).coverage();
    assert_eq!(coverage.total, 11);
    assert_eq!(coverage.documented, 7);
    let names: Vec<_> = coverage.missing.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["Public.read", "Public.View.value", "State.waiting", "Reader.read"]);
    assert_eq!(&text[coverage.missing[2].span.range()], "waiting");
    assert!(super::coverage::completed_wave("io/stdout"));
    assert!(!super::coverage::completed_wave("http/client"));
}

#[test]
fn std_documentation_coverage_and_completed_waves() {
    fn visit(path: &std::path::Path, totals: &mut (usize, usize, usize), missing: &mut Vec<String>) {
        let mut paths: Vec<_> = std::fs::read_dir(path).unwrap().map(|p| p.unwrap().path()).collect();
        paths.sort();
        for path in paths {
            if path.is_dir() { visit(&path, totals, missing); }
            else if path.extension().is_some_and(|e| e == "ok") {
                let module = ModuleDoc::load(&path).unwrap();
                let coverage = module.coverage();
                totals.0 += 1;
                totals.1 += coverage.documented;
                totals.2 += coverage.total;
                if super::coverage::completed_wave(&module.path) {
                    println!("{}: {}/{}", module.path, coverage.documented, coverage.total);
                    missing.extend(coverage.missing.iter().map(|item| format!("{}: {}", module.path, item.name)));
                }
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../std");
    let mut totals = (0, 0, 0);
    let mut missing = Vec::new();
    visit(&root, &mut totals, &mut missing);
    for module in super::coverage::COMPLETED_WAVES.lines().filter(|s| !s.is_empty() && !s.starts_with('#')) {
        let path = root.join(format!("{module}.ok"));
        assert!(path.is_file(), "completed module disappeared: {module}");
        assert_eq!(ModuleDoc::load(&path).unwrap().path, module, "completed module identity changed");
    }
    println!("std documentation baseline: {} modules, {}/{} public declarations documented", totals.0, totals.1, totals.2);
    assert!(missing.is_empty(), "undocumented completed wave declarations: {missing:?}");
}

#[test]
fn inline_enum_cases_have_named_comment_docs_without_code_edits() {
    let text = "module demo;\n// File modes.\n//\n// - `reading`: Opens for reading.\n// - `writing`: Truncates for writing.\n//\n// ```text\n// - `missing`: This is code, not case documentation.\n// ```\nenum Mode = (reading, writing, missing);\n";
    let parsed = doc(text);
    let coverage = parsed.coverage();
    assert_eq!((coverage.documented, coverage.total), (3, 4));
    assert_eq!(coverage.missing[0].name, "Mode.missing");
    let text_doc = parsed.items[0].members[0].doc.as_ref().unwrap();
    assert_eq!(text_doc.text, "Opens for reading.");
    assert_eq!(&text[text_doc.span.range()], "Opens for reading.");
    let source = crate::source::SourceFile::new("demo.ok", "module demo;\n// Modes.\n//\n// - `reading`: See [Missing].\nenum Mode = (reading);\n").unwrap();
    let resolved = super::document(&source, &mut NoModules).unwrap();
    assert_eq!(resolved.link_warnings.len(), 1, "one original link cannot produce duplicate warnings");
}


#[test]
fn coverage_excludes_caps_of_hidden_owners_and_does_not_inherit_parent_comments() {
    let hidden = doc("module demo;\nhidden object Secret {}\n// Hidden view.\ncap Secret::View { value is int { 1; } }\n");
    assert_eq!(hidden.coverage().total, 0);
    let public = doc("module demo;\n// Object docs cannot document the inline constructor.\nobject Public { Public(); }\n// Cap docs cannot document the inline field.\ncap Public::View { value is int { 1; } }\n");
    assert_eq!((public.coverage().documented, public.coverage().total), (2, 4));
}
