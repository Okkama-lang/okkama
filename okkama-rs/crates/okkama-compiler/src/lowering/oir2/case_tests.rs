//! Conformance sources through the build-mode chain: high, verify_high, own, verify_own, low, verify_low.
//! The cases are read from `conformance/cases`; the expectations stay in their `case.json`.
use super::pipeline::{PipelineError, compile_high_with};
use crate::analysis;
use crate::lowering::ModuleLoader;
use crate::oir2 as ir;
use crate::syntax::{self, ParseOptions};
use crate::SourceFile;
use std::path::{Path, PathBuf};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// `L02.string.split-join` -> `conformance/cases/L02-values-and-operators/string.split-join`.
pub(super) fn case_directory(id: &str) -> PathBuf {
    let (level, rest) = id.split_once('.').expect("case id has a level");
    let cases = repository().join("conformance/cases");
    let prefix = format!("{level}-");
    let level = std::fs::read_dir(&cases).unwrap().map(|entry| entry.unwrap().path())
        .find(|path| path.file_name().unwrap().to_str().unwrap().starts_with(&prefix))
        .unwrap_or_else(|| panic!("no level directory for {id}"));
    let directory = level.join(rest);
    assert!(directory.join("case.json").is_file(), "missing case {id}");
    directory
}

/// Support modules of a case resolve next to its entry; everything else is std or the harness.
struct CaseLoader {
    directory: PathBuf,
}

impl ModuleLoader for CaseLoader {
    fn load(&mut self, path: &str, importer: &str) -> Result<Option<crate::lowering::LoadedModule>, String> {
        let local = self.directory.join(format!("{path}.ok"));
        if !local.is_file() {
            return super::tests::StdLoader.load(path, importer);
        }
        let text = std::fs::read_to_string(&local).map_err(|error| error.to_string())?;
        let source = SourceFile::new(local.to_string_lossy(), text).map_err(|error| error.to_string())?;
        let program = syntax::parse(&source, &ParseOptions::default()).map_err(|error| format!("{error:?}"))?;
        let id = std::fs::canonicalize(&local).map_err(|error| error.to_string())?;
        Ok(Some(crate::lowering::LoadedModule { id: id.to_string_lossy().into_owned(), source, program }))
    }
}

/// The producer's high module for the case entry (all listed cases have one source file).
pub(super) fn case_high(id: &str) -> Result<ir::Module, PipelineError> {
    let directory = case_directory(id);
    let text = std::fs::read_to_string(directory.join("main.ok")).unwrap();
    source_high(id, text, directory)
}

fn source_high(id: &str, text: String, directory: PathBuf) -> Result<ir::Module, PipelineError> {
    let file = SourceFile::new("main.ok", text).unwrap();
    let program = syntax::parse(&file, &ParseOptions::default()).unwrap();
    let roots = [(&program, file.name(), Some(&file))];
    let mut loader = CaseLoader { directory };
    let graph = crate::lowering::modules::load_graph(&roots, &mut loader)
        .unwrap_or_else(|error| panic!("{id}: imports: {error:?}"));
    let sources = crate::lowering::modules::source_modules(&graph, &roots);
    let analysis = analysis::analyze(sources).unwrap_or_else(|errors| panic!("{id}: analysis: {errors:?}"));
    super::lower_to_oir2(&analysis).map_err(PipelineError::Lower)
}

/// The stage a case reaches in build mode. `beyond` means verify_low accepted it.
pub(super) fn case_stage(id: &str) -> (String, String) {
    let high = if let Some(path) = id.strip_prefix("file:") {
        let path = Path::new(path);
        let text = std::fs::read_to_string(path).unwrap();
        source_high(id, text, path.parent().unwrap().to_path_buf())
    } else {
        case_high(id)
    };
    let high = match high {
        Ok(high) => high,
        Err(error) => return ("high".into(), error.to_string()),
    };
    if let Some(directory) = std::env::var_os("OKKAMA_CASES_DUMP") {
        let name = Path::new(id).file_name().unwrap().to_string_lossy().into_owned();
        let path = Path::new(&directory).join(format!("{name}.high.oir"));
        std::fs::write(path, ir::text::print(&high).unwrap()).unwrap();
    }
    match compile_high_with(high, ir::VerifyMode::Structural) {
        Ok(_) => ("spb17".into(), String::new()),
        Err(PipelineError::Phase { phase, message }) => {
            let stage = match phase {
                "layout" | "spb_contracts" | "compile_low" | "verify_load" | "encode" => "beyond",
                other => other,
            };
            (stage.into(), format!("{phase}: {message}"))
        }
        Err(error) => ("high".into(), error.to_string()),
    }
}

/// Developer probe: `OKKAMA_CASES=<file of ids> cargo test ... oir2_case_probe -- --ignored --nocapture`.
#[test]
#[ignore]
fn oir2_case_probe() {
    let list = std::env::var("OKKAMA_CASES").expect("OKKAMA_CASES names a file of case ids");
    let ids = std::fs::read_to_string(list).unwrap();
    let mut rows = String::new();
    for id in ids.split_whitespace() {
        let (stage, message) = case_stage(id);
        let message: String = message.chars().take(400).collect::<String>().replace('\n', " ");
        rows.push_str(&format!("{id}\t{stage}\t{message}\n"));
        eprintln!("{id}\t{stage}\t{message}");
    }
    if let Some(path) = std::env::var_os("OKKAMA_CASES_REPORT") {
        std::fs::write(path, rows).unwrap();
    }
}

fn inline_stage(text: &str) -> (String, String) {
    let directory = std::env::temp_dir();
    let high = match source_high("inline", text.to_string(), directory) {
        Ok(high) => high,
        Err(error) => return ("high".into(), error.to_string()),
    };
    match compile_high_with(high, ir::VerifyMode::Structural) {
        Ok(_) => ("spb17".into(), String::new()),
        Err(PipelineError::Phase { phase, message }) => {
            let stage = match phase {
                "layout" | "spb_contracts" | "compile_low" | "verify_load" | "encode" => "beyond",
                other => other,
            };
            (stage.into(), format!("{phase}: {message}"))
        }
        Err(error) => ("high".into(), error.to_string()),
    }
}

/// Build-mode stages the lowering must reach: `low` = through verify_low; `own` = through
/// high and verify_high, then wherever own currently stops (recorded in the report).
fn reached(stage: &str, low: bool) -> bool {
    match stage {
        "beyond" | "spb17" => true,
        "own_elaborate" | "own_rewrite" | "verify_own" | "low" | "verify_low" => !low,
        _ => false,
    }
}

#[test]
fn oir2_lowering_inline_sources() {
    for (low, text) in INLINE {
        let (stage, message) = inline_stage(text);
        eprintln!("{stage}: {message}");
        assert!(reached(&stage, *low), "{text}\nstopped in {stage}: {message}");
    }
}

/// Small sources for each construct; `true` where the whole chain reaches the backend.
const INLINE: &[(bool, &str)] = &[
    (true, "module app; main() is int { let a = (6 & 3) | (1 << 4) ^ (256 >> 2) >>> 1; let b = byte(200) + byte(100); \
     let n = (~5).bit_count() + 1.rotate_left(3) + byte(3).leading_zeros(); let m = byte(6) & byte(3); \
     a + b + n + m.int() - -byte(2); }"),
    (true, "module app; main() is int { let r = (2.rat() / 3.rat()).pow(-3); let f = (1.5).pow(2.0) * 2.0 / 4.0 + 1.0; \
     (3).pow(4) + r.numerator().length() + r.denominator().length() + f.int() + (1.rat() + r - r * r).sign() \
     + (-7).abs() + 2.sqrt().int(); }"),
    (true, "module app; main() is int { let s = \" a,b \".trim().split(\",\"); \
     let t = \"ab\".repeat(2).upper().lower(); \
     let c = if \"abc\".contains(\"b\") && \"abc\".starts_with(\"a\") && !\"abc\".ends_with(\"b\") { 1; } else { 0; }; \
     s.size() + t.length() + c + \"xay\".replace(\"a\", \"\").length() + \"abc\".slice(1, 2).length() \
     + \"a\\n\".chomp().length() + \" a\".trim_start().length() + \"a \".trim_end().length() \
     + \"xyz\".characters().size() + if \"\".empty() { 1; } else { 0; }; }"),
    (true, "module app; main() is int { let i = \"abc\".find(\"c\"); let j = \"abc\".find_last(\"z\"); \
     if i == Maybe.some(2) && j == Maybe<int>.none { 1; } else { 0; } }"),
    (true, "module app; enum E is Error = (a, b(n is int)); enum F is Error = (c, d); \
     main() is int { let x = try { throw E.b(3); } catch E.a { 1; } catch E.b(n) { n; }; \
     let y = try { throw F.d; } catch F.c { 10; } catch F.d { 20; }; x + y; }"),
    (true, "module app; enum E is Error = (a, b); main() is int { try { throw E.b; } catch E.a { 1; } }"),
    (false, "module app; enum E is Error = (a); object P { P(); f(n is int) is int { if n == 0 { throw E.a; } n; } } \
     main() is int { let p = P(); let a = try? p.f(1) else 0; let b = try! p.f(2); \
     let c = try? (if true { throw E.a; } else { 2; }) else 7; a + b + c; }"),
    (false, "module app; import \"io/stdout\" take Stdout; main() is int { let out = Stdout(); \
     out <- \"a\" <- @byte(104, 105) <- \"\\n\"; let words is @string = [\"x\", \"y\"]; out <- words...; 0; }"),
    (true, "module app; enum Color = (red, green); const PRIMES = [2, 3, 5, 7]; \
     const COLORS = [Color.red, Color.green]; \
     const NESTED = [PRIMES, [11, 13]]; const HALVES = [1 / 2, 3 / 4]; \
     object R { R() {} first() is int { PRIMES[0]; } } \
     main() is int { PRIMES.size() + NESTED[1][0] + COLORS.size() + R().first() + HALVES.size(); }"),
    (false, "module app; object S { S(); sum(v is ...int) is int { let all is @int = v...; all.size(); } \
     fwd(v is ...int) is int { self.sum(1, v...); } } \
     main() is int { let xs = [2, 3]; let copy = xs...; S().fwd(2, 3) + S().sum(xs...) + copy.size(); }"),
    (true, "module app; enum Color = (red, green); main() is int { let c = Color.green; \
     let n = match c { case Color.red { 1; } case Color.green { 2; } }; \
     let k = match 'b' { case 'a' { 1; } else { 3; } }; n + k; }"),
    (true, "module app; import \"collections/iteration\" take Cursor; \
     main() is int { let c = [1, 2].cursor(); if c.available() { c.item(); } else { 0; } }"),
];

/// Cases of this lane that pass high, verify_high, own, verify_own, low and verify_low in build mode.
const THROUGH_LOW: &[&str] = &[
    "L00.stdout.stream-write", "L01.gram.precedence.arithmetic", "L01.gram.precedence.associativity",
    "L01.gram.precedence.bitwise-vs-comparison", "L01.gram.unpack.forms", "L01.lex.float.basic",
    "L01.lex.float.integral-text", "L01.lex.operators.ampersand-forms", "L01.lex.operators.nested-generic-closers",
    "L01.lex.operators.stream-write-vs-less-minus", "L02.bit-methods.int-and-byte",
    "L02.bit-methods.rotate-out-of-range", "L02.bitwise.byte", "L02.bitwise.int", "L02.cast.int-out-of-range",
    "L02.float.arithmetic", "L02.float.binary64", "L02.float.div-by-zero", "L02.float.text-exponent-and-zero",
    "L02.float.text-shortest-round-trip", "L02.int.byte-promotion", "L02.int.pow-exact", "L02.rat.arithmetic",
    "L02.rat.constructor", "L02.rat.division", "L02.shift.int", "L02.shift.reject-byte-count-8",
    "L02.shift.reject-count-64", "L02.shift.reject-count-negative", "L02.string.case-and-repeat",
    "L02.string.length-slice", "L02.string.repeat-negative", "L02.string.replace", "L02.string.replace-edge",
    "L02.string.slice-bounds", "L02.string.slice-negative", "L02.string.slice-past-end", "L02.string.slice-reversed",
    "L02.string.trim-chomp", "L02.string.trim-whitespace-set", "L03.match.enum-and-strings", "L05.choice.enum",
    "L06.file-identity.path-aliases", "L07.catch-pattern.cases-and-types", "L07.catch-pattern.unmatched-propagates",
    "L07.confirm.lazy-else", "L09.strings.formatted", "L09.time.operators",
];

/// Cases whose producer diagnostics are gone: they pass high and verify_high, then stop in own.
const THROUGH_HIGH: &[&str] = &[
    "L01.gram.confirm.index-else", "L01.gram.precedence.stream-and-range", "L02.byte.compare-and-index",
    "L02.stream.bytes-and-unpack", "L02.string.search", "L02.string.split-edge", "L02.string.split-join",
    "L03.variadic.calls", "L04.harness.all-pass", "L05.generic.key-param-values", "L05.map.object-key",
    "L05.variants.case-layouts", "L07.try-mark.diverging-operand", "L07.try-mark.force-panics",
    "L07.try-mark.optional", "L07.try-mark.panic-passes", "L13.refund-reconciliation-totals",
];

// Every case is checked, and all the stops are reported together.
#[test]
fn oir2_lowering_conformance_cases() {
    let mut failures = vec![];
    for (cases, low) in [(THROUGH_LOW, true), (THROUGH_HIGH, false)] {
        for id in cases {
            let (stage, message) = case_stage(id);
            eprintln!("{id}: {stage}: {}", message.chars().take(160).collect::<String>());
            if !reached(&stage, low) {
                failures.push(format!("{id}: stopped in {stage}: {message}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
