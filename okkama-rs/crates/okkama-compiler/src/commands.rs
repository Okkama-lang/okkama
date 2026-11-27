//! What the read-only commands report, as values (#1078): `okkama check`, `fmt`, `tokens`, `ast`, `ir` and
//! `build` take their diagnostics from here, and so do the conformance adapter and the playground
//! (okkama-wasm), so they print the same text. The text of a finding is made here and nowhere else.
//!
//! Nothing here reads a file or the environment: sources come from the caller, imports from the
//! [`ModuleLoader`] it passes (a loader factory takes the test mode, since test mode parses imports with their
//! fakes). The caller decides where the text goes and with what exit status.

use crate::codegen::{self, Artifact, Backend, Options as CodegenOptions};
use crate::diagnostics::{Diagnostic, render_all};
use crate::doc::ModuleDoc;
use crate::ide::{Inspection, Severity};
use crate::ir;
use crate::lowering::{self, CheckError, LinkError, LoadedModule, LoweringError, ModuleLoader, RootFile};
use crate::source::{SourceFile, Span};
use crate::syntax::print::{Format, PrintOptions, print_program, tokens_json, tokens_text};
use crate::syntax::{self, ParseMode, ParseOptions, ast, lexer, prune};

/// One diagnostic as a command prints it, and where it points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The name of the file it points into (as the loader named it); empty for a message without a place,
    /// such as an import that was not found or does not parse, whose text is already rendered.
    pub file: String,
    /// The bytes of `file` it points at.
    pub span: Option<Span>,
    pub severity: Severity,
    /// `semantic error`, `syntax error`, `warning`, `error[missing-docs]`, …; empty without a place.
    pub kind: String,
    pub message: String,
    /// Exactly what the command prints for it, ending with a newline.
    pub text: String,
}

impl Finding {
    /// `file:line:column: kind: message`, the line of every diagnostic after parsing.
    pub fn at(file: &SourceFile, span: Span, severity: Severity, kind: &str, message: &str) -> Self {
        let location = file.location(span.start);
        Self {
            text: format!("{}:{}:{}: {kind}: {message}\n", file.name(), location.line, location.column),
            file: file.name().to_string(),
            span: Some(span),
            severity,
            kind: kind.to_string(),
            message: message.to_string(),
        }
    }

    /// A message the loader rendered already (an import that is missing or does not parse).
    pub fn rendered(text: String) -> Self {
        Self {
            file: String::new(),
            span: None,
            severity: Severity::Error,
            kind: String::new(),
            message: text.trim_end().to_string(),
            text,
        }
    }

    /// An analysis error of `okkama check`, in its file or in `root`.
    pub fn semantic(found: &CheckError, root: &SourceFile) -> Self {
        let kind = if found.error.unsupported { "analysis limitation" } else { "semantic error" };
        Self::at(found.source.as_ref().unwrap_or(root), found.error.span, Severity::Error, kind, &found.error.message)
    }

    /// A warning of the analysis, in its file or in `root`.
    pub fn warning(found: &CheckError, root: &SourceFile) -> Self {
        Self::at(found.source.as_ref().unwrap_or(root), found.error.span, Severity::Warning, "warning", &found.error.message)
    }

    /// A failed lowering: what the program cannot be lowered for, in its file.
    pub fn lowering(error: &LoweringError, file: &SourceFile) -> Self {
        let kind = match error {
            LoweringError::Unsupported { .. } => "lowering error",
            LoweringError::Invalid { .. } => "semantic error",
            LoweringError::Internal { .. } => "internal compiler error",
        };
        Self::at(file, error.span(), Severity::Error, kind, error.message())
    }
}

/// What a command reports: its findings and the text it prints for them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// The whole text, as the command writes it to standard error.
    pub text: String,
}

impl Report {
    /// Findings printed one after another.
    pub fn listed(findings: Vec<Finding>) -> Self {
        Self { text: findings.iter().map(|found| found.text.as_str()).collect(), findings }
    }

    /// Lexical and syntax errors, each with its source line, separated by blank lines.
    pub fn syntax(diagnostics: &[Diagnostic], source: &SourceFile) -> Self {
        let findings = diagnostics
            .iter()
            .map(|found| Finding {
                file: source.name().to_string(),
                span: Some(found.span),
                severity: Severity::Error,
                kind: format!("{} error", found.kind.as_str()),
                message: found.message.clone(),
                text: found.render(source),
            })
            .collect();
        Self { findings, text: render_all(diagnostics, source) }
    }

    /// Some finding is an error: the command fails.
    pub fn failed(&self) -> bool {
        self.findings.iter().any(|found| found.severity == Severity::Error)
    }
}

/// The parse of each file: the full grammar, or the production one, whose `test` and `fake` bodies are
/// skipped (§14); the first file that does not parse reports every error of its own.
pub fn parse(sources: &[SourceFile], mode: ParseMode) -> Result<Vec<ast::Program>, Report> {
    let options = ParseOptions { mode, ..ParseOptions::default() };
    sources
        .iter()
        .map(|source| syntax::parse(source, &options).map_err(|diagnostics| Report::syntax(&diagnostics, source)))
        .collect()
}

/// A found import as a module: `name` is its id, `text` its source. Production parses it without tests (§14);
/// test mode parses it with its fakes and tests, desugared. `Err` is the text to print.
pub fn module(name: String, text: String, test_mode: bool) -> Result<LoadedModule, String> {
    let source = SourceFile::new(name.clone(), text).map_err(|error| format!("okkama: {error}\n"))?;
    let options = ParseOptions {
        mode: if test_mode { ParseMode::Full } else { ParseMode::Production },
        ..ParseOptions::default()
    };
    let program = syntax::parse(&source, &options).map_err(|diagnostics| render_all(&diagnostics, &source))?;
    let program = if test_mode { prune::desugar_test_items(program) } else { prune::strip_test_items(program) };
    Ok(LoadedModule { id: name, program, source })
}

/// The program's profile: production (tests and fakes removed) or test mode, with tests desugared.
pub fn profile(programs: Vec<ast::Program>, tests: bool) -> Vec<ast::Program> {
    programs
        .into_iter()
        .map(|program| if tests { prune::desugar_test_items(program) } else { prune::strip_test_items(program) })
        .collect()
}

/// The files of a program as lowering takes them.
pub fn roots<'a>(sources: &'a [SourceFile], programs: &'a [ast::Program]) -> Vec<RootFile<'a>> {
    sources.iter().zip(programs).map(|(source, program)| RootFile { program, source }).collect()
}

/// The documentation gate (#514): a root that is a module of a completed std wave must document every public
/// declaration; it applies to the roots only, never to their imports or to user files that happen to declare
/// a std module's name. A file that does not parse is left to the parser's report.
pub fn missing_docs(sources: &[SourceFile], loader: &mut dyn ModuleLoader) -> Result<(), Report> {
    for source in sources {
        if !crate::doc::coverage::enforced_file(source.name(), loader) {
            continue;
        }
        let Ok(doc) = ModuleDoc::parse(source.name().into(), source.text().into()) else { continue };
        if let Some(missing) = doc.coverage().missing.first() {
            let message = format!("public declaration `{}` has no documentation", missing.name);
            return Err(Report::listed(vec![Finding::at(source, missing.span, Severity::Error, "error[missing-docs]", &message)]));
        }
    }
    Ok(())
}

/// `okkama check`: every analysis error of the program (then a lowering error, #317), or, without errors,
/// its warnings. Code that exists only in test mode, tests and fakes, is checked too (#649): a second pass
/// in test mode. Test mode is not a superset of production (`Role.fake()` is allowed there), so production
/// keeps its own pass, and what both passes find is reported once. `tests` checks the test profile alone.
pub fn check<L: ModuleLoader>(sources: &[SourceFile], tests: bool, loader: &mut impl FnMut(bool) -> L) -> Report {
    if let Err(report) = missing_docs(sources, &mut loader(false)) {
        return report;
    }
    let parsed = match parse(sources, ParseMode::Full) {
        Ok(parsed) => parsed,
        Err(report) => return report,
    };
    let test_profile = !tests && parsed.iter().any(crate::ide::program_has_tests);
    let tested = test_profile.then(|| parsed.clone());
    let (mut errors, mut warnings) = check_profile(sources, profile(parsed, tests), tests, &mut loader(tests));
    if let Some(parsed) = tested {
        let (more_errors, more_warnings) = check_profile(sources, profile(parsed, true), true, &mut loader(true));
        for found in more_errors {
            if !errors.iter().any(|seen| seen.text == found.text) {
                errors.push(found);
            }
        }
        for found in more_warnings {
            if !warnings.iter().any(|seen| seen.text == found.text) {
                warnings.push(found);
            }
        }
    }
    // Errors fail the check and are all it prints; without them, the warnings.
    Report::listed(if errors.is_empty() { warnings } else { errors })
}

/// The errors and warnings of the program in one profile (production or test mode).
fn check_profile(
    sources: &[SourceFile],
    programs: Vec<ast::Program>,
    tests: bool,
    loader: &mut dyn ModuleLoader,
) -> (Vec<Finding>, Vec<Finding>) {
    let source = &sources[0];
    let roots = roots(sources, &programs);
    let checked = if tests {
        lowering::check_linked_tests(&roots, loader)
    } else {
        lowering::check_linked_files(&roots, loader)
    };
    match checked {
        Ok(warnings) => (Vec::new(), warnings.iter().map(|found| Finding::warning(found, source)).collect()),
        Err(Err(text)) => (vec![Finding::rendered(text)], Vec::new()),
        Err(Ok(errors)) => (errors.iter().map(|found| Finding::semantic(found, source)).collect(), Vec::new()),
    }
}

/// The program lowered to OIR-2 with the warnings of its root, the same pipeline for inspection (`okkama
/// ir`) and for builds. IR describes the production program unless `tests`: test and fake declarations are
/// removed by the explicit pruning pass before lowering, never skipped by it.
pub fn lower<L: ModuleLoader>(
    sources: &[SourceFile],
    tests: bool,
    loader: &mut impl FnMut(bool) -> L,
) -> Result<(ir::Module, Report), Report> {
    missing_docs(sources, &mut loader(false))?;
    let mode = if tests { ParseMode::Full } else { ParseMode::Production };
    let programs = profile(parse(sources, mode)?, tests);
    let source = &sources[0];
    let mut loader = loader(tests);
    let roots = roots(sources, &programs);
    let lowered = if tests {
        lowering::lower_linked_tests(&roots, &mut loader)
    } else {
        lowering::lower_linked_files(&roots, &mut loader)
    };
    match lowered {
        Ok((module, warnings)) => {
            Ok((module, Report::listed(warnings.iter().map(|found| Finding::warning(found, source)).collect())))
        }
        Err(LinkError::Lowering { source: imported, error }) => {
            Err(Report::listed(vec![Finding::lowering(&error, imported.as_ref().unwrap_or(source))]))
        }
        Err(LinkError::Load(text)) => Err(Report::listed(vec![Finding::rendered(text)])),
    }
}

/// `okkama build`: the program's SPB (`artifact_name` names the main file) and the warnings of its root.
pub fn build<L: ModuleLoader>(
    sources: &[SourceFile],
    tests: bool,
    loader: &mut impl FnMut(bool) -> L,
    artifact_name: &str,
) -> Result<(Artifact, Report), Report> {
    let (module, warnings) = lower(sources, tests, loader)?;
    let options = CodegenOptions { artifact_name: artifact_name.to_string() };
    match codegen::generate(&module, Backend::Sparrow, &options) {
        Ok(artifact) => Ok((artifact, warnings)),
        Err(error) => Err(Report::listed(vec![Finding::rendered(format!("okkama: codegen error: {error}\n"))])),
    }
}

/// `okkama fmt` of one file: the formatted text. Only code that parses is formatted, and only whitespace
/// changes: a result that would change a token or lose a comment is the formatter's error.
pub fn format(source: &SourceFile) -> Result<String, Report> {
    match syntax::format::format(source) {
        Ok(formatted) => Ok(formatted),
        Err(syntax::format::FormatError::Syntax(diagnostics)) => Err(Report::syntax(&diagnostics, source)),
        Err(syntax::format::FormatError::Internal(message)) => Err(Report::listed(vec![Finding::rendered(
            format!("{}: formatter error (file left unchanged): {message}\n", source.name()),
        )])),
    }
}

/// `okkama tokens`: the tokens of the file, one per line or as JSON.
pub fn tokens(source: &SourceFile, json: bool) -> Result<String, Report> {
    let lexed = lexer::lex(source.text());
    if lexed.has_errors() {
        return Err(Report::syntax(&lexed.diagnostics, source));
    }
    Ok(if json { tokens_json(&lexed.tokens, source) } else { tokens_text(&lexed.tokens, source) })
}

/// `okkama ast`: the syntax tree of the file in `mode`, without its tests if `prune_tests`.
pub fn ast(
    source: &SourceFile,
    mode: ParseMode,
    prune_tests: bool,
    format: Format,
    options: PrintOptions,
) -> Result<String, Report> {
    let program = syntax::parse(source, &ParseOptions { mode, ..ParseOptions::default() })
        .map_err(|diagnostics| Report::syntax(&diagnostics, source))?;
    let program = if prune_tests { prune::strip_test_items(program) } else { program };
    Ok(print_program(&program, source, format, options))
}

/// The pass list of `okkama ir --passes LIST` (H-19) as `OKKAMA_IR_PASSES` spells it. A name that selects
/// nothing is refused, as for `--after` (#1044): `Err` is the usage message.
pub fn passes_named(list: &str) -> Result<String, String> {
    let unknown = lowering::unknown_passes(list);
    if !unknown.is_empty() {
        return Err(format!(
            "unknown pass `{}` (passes: all, none, {})",
            unknown.join("`, `"),
            lowering::PASSES.join(", ")
        ));
    }
    Ok(list.to_string())
}

/// The pass list of `okkama ir --after PASS`: the passes up to `pass`, in pipeline order.
pub fn passes_after(pass: &str) -> Result<String, String> {
    let Some(at) = lowering::PASSES.iter().position(|known| *known == pass) else {
        return Err(format!("unknown pass `{pass}` (passes: {})", lowering::PASSES.join(", ")));
    };
    Ok(lowering::PASSES[..=at].join(","))
}

/// What the editor shows for a file (`okkama lint`, `okkama lsp`, the playground). A file with tests or
/// fakes is inspected in test mode, so their bodies are analysed too; test mode allows `Role.fake()`, so the
/// errors only production finds are added by a second pass, without repeats (#649). Warnings and lint
/// findings come from the test pass only: without the tests, an import or `hidden` that only they need would
/// look unused.
pub fn inspect<L: ModuleLoader>(source: &SourceFile, loader: &mut impl FnMut(bool) -> L) -> Inspection {
    // One full parse decides the mode and is the first pass's parse (#472): without test items it is also
    // the production parse.
    let full = syntax::parse(source, &ParseOptions::default()).ok();
    let test_mode = full.as_ref().is_none_or(crate::ide::program_has_tests);
    let mut inspection = lowering::inspect_parsed(source, full, &mut loader(test_mode), test_mode);
    if !test_mode || !inspection.parsed {
        return inspection;
    }
    let production = lowering::inspect(source, &mut loader(false), false);
    for found in production.diagnostics {
        let error = found.severity == Severity::Error;
        if error && !inspection.diagnostics.contains(&found) {
            inspection.diagnostics.push(found);
        }
    }
    inspection
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A program without imports.
    #[derive(Debug)]
    struct NoImports;

    impl ModuleLoader for NoImports {
        fn load(&mut self, _path: &str, _importer: &str) -> Result<Option<LoadedModule>, String> {
            Ok(None)
        }
    }

    fn checked(text: &str) -> Report {
        let source = SourceFile::new("app.ok", text).unwrap();
        check(&[source], false, &mut |_| NoImports)
    }

    #[test]
    fn check_prints_warnings_only_without_errors() {
        let warned = checked("module app v1.0.0; main() is int { let n = 2; for 0..n -> index { index; } }");
        assert!(!warned.failed());
        assert_eq!(
            warned.text,
            "app.ok:1:47: warning: For loop value is undefined when its source is empty: the body may not run\n"
        );
        assert_eq!(warned.findings[0].span.map(|span| span.start), Some(46));
        // The error is in a test, the warning in production code: the error alone fails the check (#649).
        let failed = checked(
            "module app v1.0.0;\ntest \"t\" { let a = 1 + true; }\nmain() is int { let n = 2; for 0..n -> index { index; } }\n",
        );
        assert!(failed.failed());
        assert!(failed.findings.iter().all(|found| found.severity == Severity::Error), "{}", failed.text);
        assert!(failed.text.starts_with("app.ok:2:"), "{}", failed.text);
    }

    #[test]
    fn what_both_profiles_find_is_reported_once() {
        let report = checked("module app v1.0.0;\ntest \"t\" { test.is(1, 1); }\nmain() is int { let n = 2; for 0..n -> index { index; } }\n");
        assert_eq!(report.findings.len(), 1, "{}", report.text);
        assert!(report.text.starts_with("app.ok:3:28: warning: "), "{}", report.text);
    }

    #[test]
    fn syntax_errors_keep_their_rendering() {
        let source = SourceFile::new("app.ok", "module app v1.0.0;\nmain() is int { let x = null; }\n").unwrap();
        let report = check(std::slice::from_ref(&source), false, &mut |_| NoImports);
        let rendered: String = report.findings.iter().map(|found| found.text.as_str()).collect::<Vec<_>>().join("\n");
        assert_eq!(report.text, rendered);
        assert_eq!(report.findings[0].kind, "syntax error");
        assert!(report.text.contains("\n2 | main() is int { let x = null; }\n"), "{}", report.text);
    }

    #[test]
    fn pass_selection_refuses_names_that_select_nothing() {
        assert_eq!(passes_named("fold,dce").as_deref(), Ok("fold,dce"));
        assert_eq!(passes_after("regions").as_deref(), Ok("statics,regions"));
        assert!(passes_named("fold,flod").unwrap_err().starts_with("unknown pass `flod`"));
        assert!(passes_after("all").unwrap_err().starts_with("unknown pass `all`"));
    }
}
