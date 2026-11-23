//! Behaviour of the `okkama` binary: commands, options, streams and exit
//! codes.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const HELLO: &str = "module app v1.0.0;\n\nmain() is int {\n    0;\n}\n";

fn okkama() -> Command {
    Command::new(env!("CARGO_BIN_EXE_okkama"))
}

fn run(args: &[&str]) -> Output {
    okkama().args(args).output().expect("run okkama")
}

fn run_stdin(args: &[&str], input: &[u8]) -> Output {
    let mut child = okkama()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn okkama");
    // A usage error exits before reading stdin: the write may then meet a
    // closed pipe, which the exit status and stderr already report.
    let _ = child.stdin.take().unwrap().write_all(input);
    child.wait_with_output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

/// A temporary file that is removed when dropped.
struct TempFile(PathBuf);

impl TempFile {
    fn new(name: &str, contents: &[u8]) -> TempFile {
        let path = std::env::temp_dir().join(format!(
            "okkama-cli-test-{}-{}-{name}",
            std::process::id(),
            name.len()
        ));
        std::fs::write(&path, contents).unwrap();
        TempFile(path)
    }

    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }

    fn absent(name: &str) -> TempFile {
        let file = Self::new(name, b"");
        std::fs::remove_file(&file.0).unwrap();
        file
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn example(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name);
    path.to_str().unwrap().to_string()
}

#[test]
fn native_stdio_preserves_bytes_in_source_and_bytecode_runs() {
    let source = TempFile::new(
        "native-stdio.ok",
        br#"module app v1.0.0;
        import "runtime/stdio" take stdout_write, stderr_write, stdin_read, stdin_read_line;
        main() is int {
            let first = stdin_read_line();
            let middle = stdin_read(2);
            let tail = stdin_read_line();
            stdout_write(first); stdout_write(middle); stdout_write(tail);
            stdout_write(@byte(0, 255)); stderr_write(@byte(69, 10)); 0;
        }"#,
    );
    let bytecode = TempFile::absent("native-stdio.spb");
    let input = b"one\r\nabtail\n";
    let expected = [input.as_slice(), &[0, 255]].concat();
    let source_run = run_stdin(&["run", source.path()], input);
    assert_eq!(source_run.status.code(), Some(0), "{}", stderr(&source_run));
    assert_eq!(source_run.stdout, expected);
    assert_eq!(source_run.stderr, b"E\n");
    let build = run_stdin(
        &[
            "build",
            source.path(),
            "-o",
            bytecode.path(),
        ],
        b"",
    );
    assert!(build.status.success(), "{}", stderr(&build));
    let vm = run_stdin(&["exec", bytecode.path()], input);
    assert_eq!(vm.status.code(), Some(0), "{}", stderr(&vm));
    assert_eq!(vm.stdout, expected);
    assert_eq!(vm.stderr, b"E\n");
}

#[test]
fn standard_stdout_uses_real_native_providers_on_sparrow() {
    let source = TempFile::new("stdlib-stdout.ok", "module app v1.0.0; import \"io/stdout\" take Stdout, Stderr; main() is int { Stdout().writeln(\"Aé🙂\"); Stderr().write(\"error\"); 0; }".as_bytes());
    let output = run_stdin(&["run", source.path()], b"");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "Aé🙂\n");
    assert_eq!(stderr(&output), "error");
}

#[test]
fn ocs_native_engines_report_execution_and_limitations_without_java() {
    let source = TempFile::new("ocs-native.ok", HELLO.as_bytes());
    let request = TempFile::new("ocs-request.json", b"");
    let response = TempFile::new("ocs-response.json", b"");
    std::fs::write(
        &request.0,
        format!(
            "{{\"mode\":\"run\",\"sources\":[{}],\"args\":[]}}",
            format_args!("{:?}", source.path())
        ),
    )
    .unwrap();
    {
        let engine = "sparrow";
        for (body, outcome, code) in [
            ("6 * 7;", "completed", 42),
            ("256;", "failed", 1),
            ("7 % 0;", "failed", 1),
            ("panic \"stop\";", "failed", 1),
            ("if -7 % 3 == -1 { 42; } else { 0; }", "completed", 42),
            (
                "let x = 1.5; if x * 2.0 == 3.0 { 42; } else { 0; }",
                "completed",
                42,
            ),
            ("let b is int = \"world\"; 0;", "rejected", 1),
        ] {
            std::fs::write(
                &source.0,
                format!("module app v1.0.0; main() is int {{ {body} }}"),
            )
            .unwrap();
            let output = okkama()
                .args([
                    "--ocs-request",
                    request.path(),
                    "--ocs-response",
                    response.path(),
                ])
                .env("OCS_RUST_ENGINE", engine)
                .env("PATH", "")
                .output()
                .unwrap();
            let reply = std::fs::read_to_string(&response.0).unwrap();
            assert_eq!(output.status.code(), Some(code), "{engine}: {reply}");
            assert!(
                reply.contains(&format!("\"outcome\": \"{outcome}\"")),
                "{reply}"
            );
            assert!(output.stdout.is_empty());
            if body.starts_with("panic ") {
                assert!(reply.contains("\"category\": \"panic\""), "{reply}");
                assert!(reply.contains("Panic: stop"), "{reply}");
            }
            if outcome == "completed" {
                assert!(reply.contains("\"exit_status\": 42"));
                assert!(output.stderr.is_empty());
            }
        }
    }
}

/// #1017: in a request, a high surrogate escape followed by an escape that is
/// not a low one becomes U+FFFD and the second escape stands on its own; the
/// debug build panicked on the unchecked subtraction before.
#[test]
fn ocs_request_with_an_unpaired_high_surrogate_is_read() {
    let source = TempFile::new("\u{FFFD}A-ocs-surrogate.ok", HELLO.as_bytes());
    let slash = '\\';
    let escaped = format!("{slash}uD800{slash}u0041");
    let path = source.path().replace("\u{FFFD}A", &escaped);
    let request = TempFile::new("ocs-surrogate-request.json", format!("{{\"mode\":\"parse\",\"sources\":[\"{path}\"]}}").as_bytes());
    let response = TempFile::absent("ocs-surrogate-response.json");
    let output = run(&["--ocs-request", request.path(), "--ocs-response", response.path()]);
    let reply = std::fs::read_to_string(&response.0).unwrap_or_default();
    assert_eq!(output.status.code(), Some(0), "{}{reply}", stderr(&output));
    assert!(reply.contains("\"outcome\": \"accepted\""), "the file was found and parsed: {reply}");
}

#[test]
fn ocs_capabilities_report_sparrow_with_every_mode() {
    // One engine (Tweed): the OCS adapter reports Sparrow, which runs the whole language, test mode included.
    let output = okkama().arg("--ocs-capabilities").env_remove("OCS_RUST_ENGINE").output().unwrap();
    assert!(output.status.success());
    assert!(stdout(&output).contains("\"engine\": \"sparrow\""));
    for mode in ["\"parse\"", "\"check\"", "\"run\"", "\"test\""] {
        assert!(stdout(&output).contains(mode), "{mode}");
    }
}

#[test]
fn sparrow_build_and_exec_need_neither_source_nor_java() {
    let artifact = TempFile::absent("sparrow-roundtrip.spb");
    let output = run_stdin(
        &["build", "-", "-o", artifact.path()],
        b"module app v1.0.0; main() is int { let a = 20; a + 22; }",
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(std::fs::read(&artifact.0).unwrap().starts_with(b"SPRW"));
    let output = okkama()
        .args(["exec", artifact.path()])
        .env("PATH", "")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(42), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

/// One engine (Tweed, 2026-11-20): every form of `run` executes on Sparrow, and a stale `OKKAMA_ENGINE` in the
/// environment changes nothing. Sparrow's statistics tell it ran.
#[test]
fn every_run_form_executes_on_sparrow_whatever_the_environment() {
    let source = TempFile::new(
        "default-engine.ok",
        br#"module app v1.0.0; main() is int { 0; } test "default" { test.ok(true); }"#,
    );
    for prefix in [&[][..], &["run"][..], &["run", "-t"][..], &["-t"][..]] {
        for environment in [None, Some(""), Some("typo"), Some("ir"), Some("jvm")] {
            let mut command = okkama();
            command.args(prefix).arg(source.path()).env("SPARROW_CORE_STATS", "1").env_remove("OKKAMA_ENGINE");
            if let Some(engine) = environment {
                command.env("OKKAMA_ENGINE", engine);
            }
            let output = command.output().unwrap();
            assert!(output.status.success(), "{prefix:?}, env={environment:?}: {}", stderr(&output));
            assert!(stderr(&output).contains("heap bytes allocated"), "{prefix:?}, env={environment:?}");
            if prefix.contains(&"-t") {
                assert!(stdout(&output).contains("--- PASS: default"), "{}", stdout(&output));
            }
        }
    }
}

#[test]
fn native_run_engines_execute_source_without_java() {
    let source = TempFile::new(
        "native-engines.ok",
        b"module app v1.0.0; main() is int { 6 * 7; }",
    );
    // По умолчанию `run` — Sparrow в процессе: java не нужна.
    let output = okkama()
        .args(["run", source.path()])
        .env("PATH", "")
        .env_remove("OKKAMA_ENGINE")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(42), "{}", stderr(&output));
    assert!(output.stderr.is_empty());
    {
        let output = okkama()
            .args(["run", source.path()])
            .env("PATH", "")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(42), "{}", stderr(&output));
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
    let output = run_stdin(
        &["run", "-"],
        b"module app v1.0.0; main() is int { let xs = [[1, 2], [40]]; xs[0][1] + xs[1][0]; }",
    );
    assert_eq!(output.status.code(), Some(42), "{}", stderr(&output));
    let output = run_stdin(
        &["run", "-"],
        b"module app v1.0.0; main() is int { let xs = [1]; xs[2]; }",
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("Array index out of bounds"));
}

#[test]
fn skipped_and_filtered_tests_report_alike_on_every_engine() {
    // #277: a test among hidden actions is an object test; `test.skip` ends its
    // test past any `catch` and does not fail the run unless an assertion
    // failed first; `--filter` keeps the tests whose names contain a text.
    let source = TempFile::new(
        "skip.ok",
        b"module app v1.0.0;\n\
          object Account {\n\
              Account();\n\
              test \"public\" { test.ok(true); }\n\
              hidden helper() is int { 1; }\n\
              test \"among private\" { test.ok(true); }\n\
          }\n\
          test \"skipped\" { test.skip(\"not today\"); test.ok(false); }\n\
          test \"through catch\" { try { test.skip(\"uncaught\"); } catch e is Error { test.fail(\"caught\"); } }\n\
          test \"failed first\" { test.is(1, 2); test.skip(\"too late\"); }\n",
    );
    {
        let engine = "sparrow";
        let output = run(&["run", "-t", source.path()]);
        assert_eq!(output.status.code(), Some(1), "{engine}: {}", stderr(&output));
        assert_eq!(
            stdout(&output),
            "=== RUN   Account :: public\n--- PASS: Account :: public\n\
             === RUN   Account :: among private\n--- PASS: Account :: among private\n\
             === RUN   skipped\n--- SKIP: skipped\n    not today\n\
             === RUN   through catch\n--- SKIP: through catch\n    uncaught\n\
             === RUN   failed first\n--- FAIL: failed first\n    expected 2, got 1\nFAIL\n",
            "{engine}"
        );
        let output = run(&[
            "run", "-t", "--filter", "skip", "--filter", "private", source.path(),
        ]);
        assert_eq!(output.status.code(), Some(0), "{engine}: {}", stderr(&output));
        assert_eq!(
            stdout(&output),
            "=== RUN   Account :: among private\n--- PASS: Account :: among private\n\
             === RUN   skipped\n--- SKIP: skipped\n    not today\nPASS\n",
            "{engine}"
        );
        // A filter that selects nothing keeps its existing error behaviour
        // (owner's #339 review): exit 1, not the runner's 4.
        let output = run(&["run", "-t", "--filter", "nothing", source.path()]);
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(stderr(&output), "okkama: no test name contains \"nothing\"\n");
    }
    let output = run(&["run", "--filter", "skip", source.path()]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("--filter selects tests: use it with -t"));
}

const REPORTED: &str = "module app v1.0.0;\n\
    import \"random/source\" take PcgRandom;\n\
    import \"io/stdout\" take Stdout;\n\
    test \"first\" { test.ok(true); }\n\
    test \"second <&>\" { Stdout().writeln(\"printed by second\"); test.is(\"a\\\"b\", \"c\"); }\n\
    test \"third\" { test.skip(\"not 'today'\"); }\n\
    test \"fourth\" { test.is(PcgRandom(7).bytes(3), [byte(215), byte(136), byte(231)]); }\n";

fn run_with_std(args: &[&str]) -> Output {
    let std = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../std");
    okkama().env("OKKAMA_STDLIB_DIR", std).args(args).output().expect("run okkama")
}

/// #339: one report of a run, shown as people read it (the default), as one
/// JSON document or as JUnit XML; machine formats own stdout. Tests are listed
/// in declaration order, and a seed orders the run reproducibly.
#[test]
fn test_reports_list_every_test_and_a_seed_orders_the_run() {
    let source = TempFile::new("reported.ok", REPORTED.as_bytes());
    let human = run_with_std(&["run", "-t", source.path()]);
    assert_eq!(human.status.code(), Some(1), "{}", stderr(&human));
    assert_eq!(
        stdout(&human),
        "=== RUN   first\n--- PASS: first\n\
         === RUN   second <&>\nprinted by second\n--- FAIL: second <&>\n    expected \"c\", got \"a\"b\"\n\
         === RUN   third\n--- SKIP: third\n    not 'today'\n\
         === RUN   fourth\n--- PASS: fourth\nFAIL\n"
    );
    let named = run_with_std(&["run", "-t", "--report", "human", source.path()]);
    assert_eq!(stdout(&named), stdout(&human));

    let json = run_with_std(&["run", "-t", "--report", "json", source.path()]);
    assert_eq!(json.status.code(), Some(1));
    assert_eq!(stderr(&json), "printed by second\n");
    let document: serde_json::Value = serde_json::from_str(&stdout(&json)).expect("one JSON document");
    assert_eq!(document["seed"], serde_json::Value::Null);
    assert_eq!(document["passed"], false);
    assert_eq!(document["totals"], serde_json::json!({"tests": 4, "passed": 2, "failed": 1, "skipped": 1}));
    let names: Vec<&str> = document["tests"].as_array().unwrap().iter().map(|test| test["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["first", "second <&>", "third", "fourth"]);
    assert_eq!(document["tests"][1]["failures"], serde_json::json!(["expected \"c\", got \"a\"b\""]));
    assert_eq!(document["tests"][2]["reason"], "not 'today'");

    let junit = run_with_std(&["run", "-t", "--report", "junit", source.path()]);
    assert_eq!(junit.status.code(), Some(1));
    let xml = stdout(&junit);
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites tests=\"4\" failures=\"1\" errors=\"0\" skipped=\"1\">\n"));
    assert!(xml.contains("<testcase name=\"second &lt;&amp;&gt;\""));
    assert!(xml.contains("<failure message=\"expected &quot;c&quot;, got &quot;a&quot;b&quot;\">"));
    assert!(xml.contains("<skipped message=\"not &apos;today&apos;\"/>"));
    assert!(!xml.contains("printed by second"));

    let order = sparrow_core::test_report::execution_order(4, Some(3));
    assert_ne!(order, [0, 1, 2, 3], "seed 3 must reorder four tests");
    let seeded = run_with_std(&["run", "-t", "--seed", "3", source.path()]);
    let again = run_with_std(&["run", "-t", "--seed", "3", source.path()]);
    assert_eq!(stdout(&seeded), stdout(&again));
    assert!(stdout(&seeded).starts_with("=== SEED  3\n"));
    let declared = ["first", "second <&>", "third", "fourth"];
    let seeded_text = stdout(&seeded);
    let ran: Vec<&str> = seeded_text.lines().filter_map(|line| line.strip_prefix("=== RUN   ")).collect();
    let wanted: Vec<&str> = order.iter().map(|&index| declared[index]).collect();
    assert_eq!(ran, wanted);
    // The property generator's PCG stream is the run's own: fourth passes in any order.
    assert!(stdout(&seeded).contains("--- PASS: fourth\n"));
    let seeded_json: serde_json::Value =
        serde_json::from_str(&stdout(&run_with_std(&["run", "-t", "--report", "json", "--seed", "3", source.path()]))).unwrap();
    assert_eq!(seeded_json["seed"], 3);
    let executions: Vec<u64> = seeded_json["tests"].as_array().unwrap().iter().map(|test| test["execution"].as_u64().unwrap()).collect();
    for (position, &index) in order.iter().enumerate() {
        assert_eq!(executions[index], position as u64);
    }
    let filtered = run_with_std(&["run", "-t", "--seed", "3", "--filter", "i", source.path()]);
    let kept: Vec<String> = stdout(&filtered).lines().filter_map(|line| line.strip_prefix("=== RUN   ")).map(str::to_owned).collect();
    let selected = ["first", "third"];
    let wanted: Vec<String> = sparrow_core::test_report::execution_order(2, Some(3)).iter().map(|&index| selected[index].to_owned()).collect();
    assert_eq!(kept, wanted);

    for (args, message) in [
        (vec!["run", "-t", "--report", "tap"], "unknown report format `tap`: use human, json or junit"),
        (vec!["run", "-t", "--seed", "-1"], "--seed needs a whole number from 0 to 9007199254740991, not `-1`"),
        (vec!["run", "-t", "--seed", "9007199254740992"], "not `9007199254740992`"),
        (vec!["run", "-t", "--seed", "x"], "not `x`"),
        (vec!["run", "--seed", "1"], "--seed orders tests: use it with -t"),
        (vec!["run", "--report", "json"], "--report reports tests: use it with -t"),
    ] {
        let mut args = args;
        args.push(source.path());
        let output = run_with_std(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(stderr(&output).contains(message), "{args:?}: {}", stderr(&output));
    }
    let broken = TempFile::new("broken.ok", b"module app v1.0.0;\ntest \"t\" { test.ok(missing); }\n");
    assert_eq!(run_with_std(&["run", "-t", broken.path()]).status.code(), Some(4));
    // #1017: a syntax error is a source error too, found by the doctest pass first.
    let unparsable = TempFile::new("unparsable.ok", b"module app v1.0.0;\ntest \"t\" { let = 1; }\n");
    let output = run_with_std(&["run", "-t", unparsable.path()]);
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    assert!(stderr(&output).contains("syntax error"), "{}", stderr(&output));
    let passing = TempFile::new("passing.ok", b"module app v1.0.0;\ntest \"t\" { test.ok(true); }\n");
    assert_eq!(run_with_std(&["run", "-t", "--report", "junit", passing.path()]).status.code(), Some(0));
}

const SUB_CASES: &str = "module app v1.0.0;\n\
    import \"io/stdout\" take Stdout;\n\
    enum NoTariff is Error = (weight(grams is int));\n\
    test \"postage\" {\n\
        for [20, 50, 100, 250] -> grams {\n\
            test.case(grams.string() + \" g\") {\n\
                Stdout().writeln(\"ran \" + grams.string());\n\
                test.is(grams % 50 == 0 || grams == 20, true);\n\
                if grams == 50 { test.is(grams, 51); }\n\
                if grams == 250 { throw NoTariff.weight(grams); }\n\
            };\n\
        }\n\
    }\n\
    test \"setup fails\" { throw NoTariff.weight(0); test.case(\"never\") { test.ok(true); }; }\n\
    test \"duplicates\" { test.case(\"a\") { test.ok(true); }; test.case(\"a\") { test.ok(false); }; test.case(\"c\") { test.ok(true); }; }\n\
    test \"skip\" { test.case(\"b\") { test.skip(\"later\"); }; test.case(\"never\") { test.ok(false); }; }\n\
    test \"nested\" { test.case(\"outer\") { test.case(\"inner\") { test.ok(true); }; }; test.case(\"never\") { test.ok(false); }; }\n\
    async test \"async cases\" { test.case(\"awaited\") { test.is(await Value().seven(), 7); }; }\n\
    object Value { Value(); async seven() is int { 7; } }\n\
    test \"plain\" { test.ok(true); }\n";

/// #809: `test.case(name) { … }` gives a test named sub-cases that the report
/// lists under it and `--filter TEST/CASE` selects; a failed or throwing
/// sub-case fails its test and the next ones still run.
#[test]
fn sub_cases_are_reported_under_their_test_and_selected_by_name() {
    let source = TempFile::new("subcases.ok", SUB_CASES.as_bytes());
    let output = run_with_std(&["run", "-t", source.path()]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "=== RUN   postage\nran 20\nran 50\nran 100\nran 250\n\
         \x20   --- PASS: postage/20 g\n\
         \x20   --- FAIL: postage/50 g\n        expected 51, got 50\n\
         \x20   --- PASS: postage/100 g\n\
         \x20   --- FAIL: postage/250 g\n        NoTariff.weight(250)\n\
         --- FAIL: postage\n\
         === RUN   setup fails\n--- FAIL: setup fails\n    NoTariff: NoTariff.weight(0)\n\
         === RUN   duplicates\n\
         \x20   --- PASS: duplicates/a\n\
         \x20   --- PASS: duplicates/c\n\
         --- FAIL: duplicates\n    duplicate sub-case \"a\": a name runs once\n\
         === RUN   skip\n\
         \x20   --- SKIP: skip/b\n        later\n\
         --- PASS: skip\n\
         === RUN   nested\n\
         \x20   --- FAIL: nested/outer\n        RuntimeError: test.case \"inner\" inside test.case \"outer\": sub-cases do not nest\n\
         --- FAIL: nested\n\
         === RUN   async cases\n    --- PASS: async cases/awaited\n--- PASS: async cases\n\
         === RUN   plain\n--- PASS: plain\nFAIL\n"
    );

    // Only the sub-cases whose names contain the text after the slash run;
    // the others' blocks do not.
    let selected = run_with_std(&["run", "-t", "--filter", "postage/100", source.path()]);
    assert_eq!(selected.status.code(), Some(0), "{}", stderr(&selected));
    assert_eq!(stdout(&selected), "=== RUN   postage\nran 100\n    --- PASS: postage/100 g\n--- PASS: postage\nPASS\n");
    let whole = run_with_std(&["run", "-t", "--filter", "postage", source.path()]);
    assert!(stdout(&whole).contains("ran 20\nran 50\nran 100\nran 250\n"));
    let none = run_with_std(&["run", "-t", "--filter", "postage/7", source.path()]);
    assert_eq!(none.status.code(), Some(0));
    assert_eq!(stdout(&none), "=== RUN   postage\n--- SKIP: postage\n    no sub-case matches the filter\nPASS\n");
    let nothing = run_with_std(&["run", "-t", "--filter", "nobody/x", source.path()]);
    assert_eq!(nothing.status.code(), Some(1));

    let json = run_with_std(&["run", "-t", "--report", "json", "--filter", "postage", source.path()]);
    let document: serde_json::Value = serde_json::from_str(&stdout(&json)).unwrap();
    let test = &document["tests"][0];
    assert_eq!(test["outcome"], "failed");
    let ids: Vec<&str> = test["cases"].as_array().unwrap().iter().map(|case| case["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["postage/20 g", "postage/50 g", "postage/100 g", "postage/250 g"]);
    assert_eq!(test["cases"][1]["failures"], serde_json::json!(["expected 51, got 50"]));
    assert_eq!(document["totals"], serde_json::json!({"tests": 4, "passed": 2, "failed": 2, "skipped": 0}));
    let junit = stdout(&run_with_std(&["run", "-t", "--report", "junit", "--filter", "duplicates", "--filter", "skip", source.path()]));
    assert!(junit.contains("<testsuites tests=\"4\" failures=\"1\" errors=\"0\" skipped=\"1\">"), "{junit}");
    assert!(junit.contains("<testcase name=\"skip/b\" classname="), "{junit}");
    assert!(junit.contains("<testcase name=\"duplicates\" classname="), "{junit}");
    assert!(!junit.contains("<testcase name=\"skip\" classname="), "{junit}");
}

/// Unequal objects, variants and nested values in a failing check show their values, not heap
/// identities (#756); Champ's test mode shows the same text (`champ_cases_tests.rs`).
const SHOWN_VALUES: &str = "module app v1.0.0;\n\
    import \"time/duration\" take Duration;\n\
    enum Shape = (dot, rect(w is int, h is int));\n\
    enum Down is Error = (unavailable(code is int));\n\
    object Point { has x is int; has label is string; Point(:x, :label); }\n\
    object Box<T> { has v is T; Box(:v); }\n\
    object Cached { has n is int; Cached(:n); twice() is int { once let doubled = n * 2; doubled; } }\n\
    test \"values\" {\n\
    \x20   test.is(Duration(seconds: 1), Duration(seconds: 2));\n\
    \x20   test.is(Point(x: 1, label: \"a\\\"b\"), Point(x: 2, label: \"c\"));\n\
    \x20   test.is(Box(v: Shape.rect(1, 2)), Box(v: Shape.dot));\n\
    \x20   test.is(Box(v: [1, 2]), Box(v: [3]));\n\
    \x20   test.is(Down.unavailable(3), Down.unavailable(4));\n\
    \x20   test.isnt(Maybe.some(Point(x: 1, label: \"\")), Maybe.some(Point(x: 1, label: \"\")));\n\
    \x20   test.is([Point(x: 1, label: \"p\")], [Point(x: 2, label: \"q\")]);\n\
    \x20   let cached = Cached(n: 4);\n\
    \x20   test.is(cached.twice(), 8);\n\
    \x20   test.isnt(cached, Cached(n: 4));\n\
    }\n";

const SHOWN_VALUES_REPORT: &str = "=== RUN   values\n\
    --- FAIL: values\n\
    \x20   expected Duration(origin: 2), got Duration(origin: 1)\n\
    \x20   expected Point(x: 2, label: \"c\"), got Point(x: 1, label: \"a\\\"b\")\n\
    \x20   expected Box<Shape>(v: Shape.dot), got Box<Shape>(v: Shape.rect(1, 2))\n\
    \x20   expected Box<@int>(v: [3]), got Box<@int>(v: [1, 2])\n\
    \x20   expected Down.unavailable(4), got Down.unavailable(3)\n\
    \x20   expected values to differ, both were Maybe.some(Point(x: 1, label: \"\"))\n\
    \x20   expected [Point(x: 2, label: \"q\")], got [Point(x: 1, label: \"p\")]\n\
    \x20   expected values to differ, both were Cached(n: 4)\n\
    FAIL\n";

#[test]
fn failed_checks_show_objects_and_variants_by_value() {
    let source = TempFile::new("shown.ok", SHOWN_VALUES.as_bytes());
    let std = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../std");
    let output = okkama()
        .args(["run", "-t", source.path()])
        .env("OKKAMA_STDLIB_DIR", &std)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(stdout(&output), SHOWN_VALUES_REPORT);
}

/// The channel examples the channel guide shows (#341) print what their
/// `.expected` files say on Sparrow.
#[test]
fn channel_examples_print_their_expected_output() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/channels");
    let mut checked = 0;
    for entry in std::fs::read_dir(&directory).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "ok") {
            continue;
        }
        let expected = std::fs::read_to_string(path.with_extension("expected")).unwrap();
        {
            let engine = "sparrow";
            let output = run(&["run", "--timeout", "30", path.to_str().unwrap()]);
            assert_eq!(output.status.code(), Some(0), "{} {engine}: {}", path.display(), stderr(&output));
            assert_eq!(stdout(&output), expected, "{} {engine}", path.display());
        }
        checked += 1;
    }
    assert_eq!(checked, 5);
}

#[test]
fn uncaught_failure_prints_place_and_call_stack() {
    let source = b"module app v1.0.0;\nobject Box {\n    Box();\n    at(i is int) is int { [1, 2][i]; }\n}\nmain() is int {\n    Box().at(5);\n}\n";
    // Every engine reports the place and the call stack (the core since #10).
    {
        let engine = "sparrow";
        let output = run_stdin(&["run", "-"], source);
        assert_eq!(output.status.code(), Some(1));
        let text = stderr(&output);
        assert!(
            text.starts_with("RuntimeError: Array index out of bounds\n"),
            "{engine}: {text}"
        );
        assert!(
            text.contains(":4:27\n  |\n4 |     at(i is int) is int { [1, 2][i]; }\n"),
            "{engine}: {text}"
        );
        assert!(
            text.contains("Stack trace (most recent call first):\n  at Box.at ("),
            "{engine}: {text}"
        );
        assert!(text.contains(":7:5)\n"), "{engine}: {text}");
    }
    let mut command = okkama();
    command.args(["run", "-"]).env("OKKAMA_TRACE", "off");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(source).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(stderr(&output), "RuntimeError: Array index out of bounds\n");
}

/// #797 (language spec 9.5): a program failure passes every catch, also when a driver raises it and it reaches
/// the caller through `await`.
#[test]
fn a_drivers_program_failure_passes_a_catch_of_error() {
    let source = b"module app v1.0.0;\nimport \"io/stdout\" take Stdout;\nimport \"runtime/time\" take sleep_nanoseconds;\n\
        async main() is int {\n    let r = try { await sleep_nanoseconds(\"01\"); \"none\"; } catch e is Error { e.text(); };\n\
        Stdout().writeln(\"caught \" + r);\n    0;\n}\n";
    let std = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../std");
    let mut child = okkama()
        .args(["run", "-"])
        .env("OKKAMA_STDLIB_DIR", std)
        .env("OKKAMA_TRACE", "off")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(source).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), "RuntimeError: Wait duration must be a canonical non-negative integer\n");
}

#[test]
fn build_has_no_target_selection() {
    // One engine, one target (owner, 2026-11-20): `--target` and `--backend` are unknown options, and a failed
    // build leaves the previous artifact as it was.
    let file = TempFile::new("target-alias.spb", b"previous");
    for flags in [&["--target", "sparrow"][..], &["--backend", "sparrow"], &["--target=sparrow"]] {
        let mut args = vec!["build", "-"];
        args.extend_from_slice(flags);
        args.extend_from_slice(&["-o", file.path()]);
        let output = run_stdin(&args, HELLO.as_bytes());
        assert_eq!(output.status.code(), Some(2), "{flags:?}");
        assert!(stderr(&output).contains("unknown option"), "{flags:?}: {}", stderr(&output));
        assert_eq!(std::fs::read(&file.0).unwrap(), b"previous");
    }
}

#[test]
fn native_run_does_not_ignore_arguments_or_fallback_to_java() {
    // `--engine` is gone with the other engines: an unknown option, a usage error.
    for args in [
        vec!["run", "--engine"],
        vec!["run", "--engine", "sparrow", "-"],
    ] {
        assert_eq!(run(&args).status.code(), Some(2));
    }
    {
        let output = run_stdin(
            &["run", "-"],
            b"module runtime/unlinked v1.0.0; native nothing() is int; main() is int { nothing(); }",
        );
        assert_eq!(output.status.code(), Some(1));
        assert!(
            stderr(&output).contains("native provider is not implemented"),
            "{}",
            stderr(&output)
        );
    }
}

#[test]
fn sparrow_build_failure_preserves_artifact() {
    let artifact = TempFile::new("sparrow-preserved.spb", b"previous artifact");
    let output = run_stdin(
        &["build", "-", "-o", artifact.path()],
        b"module app v1.0.0; main() is int { missing_name; }",
    );
    assert_ne!(output.status.code(), Some(0));
    assert_eq!(std::fs::read(&artifact.0).unwrap(), b"previous artifact");
    // Float — часть языка Sparrow: та же программа собирается и исполняется.
    let output = run_stdin(
        &["build", "-", "-o", artifact.path()],
        b"module app v1.0.0; main() is int { let x = 1.5; (x * 4.0).int(); }",
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(run(&["exec", artifact.path()]).status.code(), Some(6));
}

#[test]
fn sparrow_exec_validates_file_and_exit_status() {
    let artifact = TempFile::new("sparrow-invalid.spb", b"SPRW");
    let output = run(&["exec", artifact.path()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("SPB"));
    for value in ["0", "255", "256", "999999999999999999999999999999999"] {
        let output = run_stdin(
            &["build", "-", "-o", artifact.path()],
            format!("module app v1.0.0; main() is int {{ {value}; }}").as_bytes(),
        );
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        let output = run(&["exec", artifact.path()]);
        match value.parse::<u8>() {
            Ok(code) => {
                assert_eq!(output.status.code(), Some(code.into()));
                assert!(output.stderr.is_empty());
            }
            Err(_) => {
                assert_eq!(output.status.code(), Some(1));
                assert!(stderr(&output).contains("0..255"));
            }
        }
    }
    std::fs::File::create(&artifact.0)
        .unwrap()
        .set_len(okkama_sparrow::MAX_FILE_BYTES as u64 + 1)
        .unwrap();
    assert_eq!(run(&["exec", artifact.path()]).status.code(), Some(1));
}

#[test]
fn sparrow_exec_usage_and_io_errors() {
    for args in [&["exec"][..], &["exec", "--unknown"]] {
        assert_eq!(run(args).status.code(), Some(2));
    }
    // Аргументы после файла — аргументы программы; отсутствующий файл — I/O.
    assert_eq!(
        run(&["exec", "missing-file.spb", "b"]).status.code(),
        Some(3)
    );
    for args in [&["exec", "--help"][..], &["help", "exec"]] {
        let output = run(args);
        assert_eq!(output.status.code(), Some(0));
        assert!(stdout(&output).contains("okkama exec"));
    }
    let absent = TempFile::absent("sparrow-missing.spb");
    assert_eq!(run(&["exec", absent.path()]).status.code(), Some(3));
}

#[test]
fn sparrow_cli_executes_control_flow_and_v2_function_calls() {
    use okkama_sparrow::{Function, Instruction::*, Program, Register as R};
    let file = TempFile::absent("sparrow-control-flow.spb");
    let source =
        b"module app v1.0.0; main() is int { for 1..=5 -> i { if i < 4 { next; } i + 1; } }";
    let output = run_stdin(
        &["build", "-", "-o", file.path()],
        source,
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let output = run(&["exec", file.path()]);
    assert_eq!(output.status.code(), Some(6), "{}", stderr(&output));
    assert!(output.stderr.is_empty());
    let function = |symbol: &str, registers: usize, params: Vec<R>, code| Function {
        symbol: symbol.into(),
        is_async: false,
        native: None,
        intrinsic: None,
        registers: vec![okkama_sparrow::TypeRef(0); registers],
        params,
        result: Some(okkama_sparrow::TypeRef(0)),
        code,
        handlers: Vec::new(),
    };
    let dst = |index| okkama_sparrow::Dst(R(index));
    let program = Program {
        module: "app".into(),
        version: "1.0.0".into(),
        types: vec![okkama_sparrow::Type::Int],
        functions: vec![
            function(
                "main",
                2,
                vec![],
                vec![
                    Const {
                        dst: dst(0),
                        value: okkama_sparrow::Constant::Int("21".into()),
                    },
                    Call {
                        dst: Some(dst(1)),
                        function: okkama_sparrow::FuncRef(1),
                        args: vec![R(0)],
                    },
                    Return { value: Some(R(1)) },
                ],
            ),
            function(
                "double",
                2,
                vec![R(0)],
                vec![
                    IntAdd {
                        dst: dst(1),
                        left: R(0),
                        right: R(0),
                    },
                    Return { value: Some(R(1)) },
                ],
            ),
        ],
        entry: Some(okkama_sparrow::FuncRef(0)),
        ..Program::default()
    };
    std::fs::write(&file.0, okkama_sparrow::encode(&program).unwrap()).unwrap();
    let output = okkama()
        .args(["exec", file.path()])
        .env("PATH", "")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(42), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn help_and_version() {
    for args in [&["--help"][..], &["-h"], &["help"]] {
        let output = run(args);
        assert_eq!(output.status.code(), Some(0));
        assert!(stdout(&output).contains("okkama ast <FILE|->"));
        assert!(stdout(&output).contains("Run a program on Sparrow (JIT)"));
        // One engine (Tweed): nothing to select.
        assert!(!stdout(&output).contains("--engine") && !stdout(&output).contains("OKKAMA_ENGINE"));
        assert!(stdout(&output).contains("okkama ir <FILE|->"));
        assert!(stdout(&output).contains("okkama build <FILE|-> -o <FILE.spb>"));
        assert!(stderr(&output).is_empty());
    }
    let output = run(&["help", "ast"]);
    assert!(stdout(&output).contains("--prune-tests"));
    let output = run(&["ast", "--help"]);
    assert!(stdout(&output).contains("--mode production"));
    let output = run(&["tokens", "--help"]);
    assert!(stdout(&output).contains("okkama tokens"));
    for args in [&["help", "ir"][..], &["ir", "--help"]] {
        let output = run(args);
        assert_eq!(output.status.code(), Some(0));
        assert!(stdout(&output).contains("okkama ir <FILE|->"));
        assert!(stdout(&output).contains("ret"));
        assert!(stderr(&output).is_empty());
    }
    for args in [&["help", "build"][..], &["build", "--help"]] {
        let output = run(args);
        assert_eq!(output.status.code(), Some(0));
        assert!(!stdout(&output).contains("--target") && !stdout(&output).contains("--backend"));
        assert!(stdout(&output).contains("--output"));
        assert!(stdout(&output).contains("verified SPB"));
        assert!(stderr(&output).is_empty());
    }
    let output = run(&["--version"]);
    assert_eq!(
        stdout(&output),
        format!("okkama {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn no_arguments_prints_help_and_fails() {
    let output = run(&[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("USAGE"));
}

#[test]
fn tokens_text_and_json() {
    let output = run(&["tokens", &example("hello.ok")]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(
        text.starts_with("1:1-1:7      MODULE         \"module\"\n"),
        "{text}"
    );
    assert!(
        text.contains("STRING         \"endl\"  string endl \"\\n\""),
        "{text}"
    );
    assert!(text.trim_end().ends_with("EOF            \"\""));

    let output = run(&["tokens", &example("hello.ok"), "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    let json = stdout(&output);
    assert!(json.starts_with("[\n  {\n    \"kind\": \"MODULE\",\n    \"lexeme\": \"module\","));
    assert!(json.contains("\"kind\": \"EOF\""));
}

#[test]
fn ast_formats_spans_and_stdin() {
    let file = TempFile::new("hello.ok", HELLO.as_bytes());
    let tree = run(&["ast", file.path()]);
    assert_eq!(tree.status.code(), Some(0));
    assert!(stdout(&tree).starts_with("Program\n  module: ModuleDecl\n    path: [app]\n"));
    assert!(!stdout(&tree).contains('@'), "no spans without --spans");

    let spans = run(&["ast", file.path(), "--spans"]);
    assert!(stdout(&spans).starts_with("Program @1:1-6:1\n  module: ModuleDecl @1:1-1:19\n"));

    let json = run(&["ast", file.path(), "--format", "json"]);
    assert!(stdout(&json).starts_with("{\n  \"node\": \"Program\",\n  \"module\": {"));
    let json_spans = run(&["ast", file.path(), "--format=json", "--spans"]);
    assert!(stdout(&json_spans).contains("\"span\": {\n    \"start\": 0,"));

    let from_stdin = run_stdin(&["ast", "-"], HELLO.as_bytes());
    assert_eq!(from_stdin.status.code(), Some(0));
    assert_eq!(stdout(&from_stdin), stdout(&tree));
}

#[test]
fn ir_file_and_stdin_include_module_function_and_return() {
    let source = b"module app v1.0.0; main() is int { 1 + 1; }";
    let file = TempFile::new("ir-addition.ok", source);
    let expected = concat!(
        "oir 1 high\n",
        "module app v1.0.0\n",
        "requires core\n",
        "entry @main\n",
        "\n",
        "func @main() -> int [entry] {\n",
        "^b0:\n",
        "    %0: int = const 1\n",
        "    %1: int = const 1\n",
        "    %2: int = const 2\n",
        "    ret %2\n",
        "}\n",
    );
    for output in [
        run(&["ir", file.path()]),
        run_stdin(&["ir", "-", "--format=text"], source),
        run(&["ir", "--format", "text", file.path()]),
    ] {
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        assert_eq!(stdout(&output), expected);
        assert!(stderr(&output).is_empty());
    }
}

#[test]
fn ir_keeps_all_statements_and_nested_additions() {
    let output = run_stdin(
        &["ir", "-"],
        b"module app v1.0.0; main() is int { 7; 1 + 2 + 3; }",
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        concat!(
            "oir 1 high\n",
            "module app v1.0.0\n",
            "requires core\n",
            "entry @main\n",
            "\n",
            "func @main() -> int [entry] {\n",
            "^b0:\n",
            "    %0: int = const 7\n",
            "    %1: int = const 1\n",
            "    %2: int = const 2\n",
            "    %3: int = const 3\n",
            "    %4: int = const 3\n",
            "    %5: int = const 6\n",
            "    ret %5\n",
            "}\n",
        )
    );
    assert!(stderr(&output).is_empty());
}

#[test]
fn ir_explicit_return_is_canonical_and_missing_imports_are_errors() {
    let output = run_stdin(
        &["ir", "-"],
        b"module app/math v1.2.3; import \"missing/math\" take Number; main() is int { 0; }",
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "<stdin>:1:25: semantic error: Import \"missing/math\" not found\n"
    );
    let output = run_stdin(
        &["ir", "-"],
        b"module app/math v1.2.3; main() is int { return 0x2a; }",
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        concat!(
            "oir 1 high\n",
            "module app/math v1.2.3\n",
            "requires core\n",
            "entry @main\n",
            "\n",
            "func @main() -> int [entry] {\n",
            "^b0:\n",
            "    %0: int = const 42\n",
            "    ret %0\n",
            "}\n",
        )
    );
    assert!(stderr(&output).is_empty());
}

#[test]
fn ir_control_flow_uses_blocks_and_parameters() {
    let output = run_stdin(
        &["ir", "-"],
        b"module app v1.0.0; main() is int { let x = 9223372036854775808; if x > 0 { 1; } else { 2; } }",
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("%0: int = const 9223372036854775808\n"),
        "{text}"
    );
    assert!(text.contains("cond %2, ^b1, ^b2\n"), "{text}");
    assert!(text.contains("^b3(%5: int):\n    ret %5\n"), "{text}");
}

#[test]
fn ir_reports_unsupported_and_invalid_code_without_partial_output() {
    for (source, kind, expected) in [
        (
            "module app v1.0.0; main() is int { 1 + missing; }",
            "semantic error",
            "Undefined variable \"missing\"",
        ),
        (
            "module app v1.0.0; main() is int {}",
            "semantic error",
            "must produce a return value",
        ),
        (
            "module app v1.0.0; main() is int { let xs is @any = [1..3, \"a\"]; 0; }",
            "lowering error",
            "ranges inside `@any` literals",
        ),
        (
            "module app v1.0.0; main() is string { 1; }",
            "semantic error",
            "`main() is int`",
        ),
        (
            "module app v1.0.0; object P { P(); } object W { W(); f(p is P) is int { 1; } } async main() is int { await spawn W().f(p: P()); }",
            "semantic error",
            "spawn cannot transfer value of type P",
        ),
        (
            "module app v1.0.0; main() is int { 1; } main() is int { 2; }",
            "semantic error",
            "Duplicate action name",
        ),
    ] {
        let output = run_stdin(&["ir", "-"], source.as_bytes());
        assert_eq!(
            output.status.code(),
            Some(1),
            "{source}: {}",
            stderr(&output)
        );
        assert!(stdout(&output).is_empty(), "{source}");
        assert!(
            stderr(&output).contains(&format!("{kind}:")),
            "{}",
            stderr(&output)
        );
        assert!(stderr(&output).contains(expected), "{}", stderr(&output));
    }
}

#[test]
fn ir_accepts_role_bound_actions_worker_spawn_and_native_handles() {
    for source in [
        "module app v1.0.0; main() is int { 1; } native object Handle;",
        "module app v1.0.0; role R { f() is int; } object O does R { O(); f() is int { 1; } } main() is int { let r is R = O(); let g is () is int = r.f; g(); }",
        "module app v1.0.0; object W { W(); f() is int { 1; } } async main() is int { let x = spawn W().f(); 0; }",
    ] {
        let output = run_stdin(&["ir", "-"], source.as_bytes());
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        assert!(!output.stdout.is_empty());
    }
}

#[test]
fn ir_lowering_error_points_at_the_offending_operand() {
    let output = run_stdin(
        &["ir", "-"],
        b"module app v1.0.0;\nmain() is int {\n    1 + missing;\n}\n",
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).is_empty());
    assert!(
        stderr(&output).starts_with("<stdin>:3:9: semantic error:"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn build_dispatches_file_and_stdin_to_sparrow_and_writes_bytecode() {
    let target = TempFile::absent("build-sparrow-output.spb");
    let source = example("arithmetic.ok");
    for output in [
        run(&["build", &source, "-o", target.path()]),
        run_stdin(
            &["build", "--output", target.path(), "-"],
            HELLO.as_bytes(),
        ),
        run(&[
            "build",
            &source,
            &format!("--output={}", target.path()),
        ]),
    ] {
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        assert!(stdout(&output).is_empty());
        assert!(stderr(&output).is_empty());
        assert!(
            okkama_sparrow::decode(&std::fs::read(&target.0).unwrap()).is_ok()
        );
    }
}

#[test]
fn failed_build_preserves_an_existing_output_file() {
    let target = TempFile::new("build-preserved.spb", b"previous artifact");
    let output = run_stdin(
        &["build", "-", "-o", target.path()],
        b"module app v1.0.0; import \"missing\" take X; main() is int { 1; }",
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("semantic error: Import \"missing\" not found"));
    assert_eq!(std::fs::read(&target.0).unwrap(), b"previous artifact");
}

#[test]
fn build_reports_frontend_errors_before_invoking_the_backend() {
    let target = TempFile::absent("build-source-errors.spb");
    for (source, expected) in [
        ("module app v1.0.0; main() is int { 1 2; }", "syntax error"),
        (
            "module app v1.0.0; main() is int { \"open }",
            "lexical error",
        ),
        (
            "module app v1.0.0; main() is int { let xs is @any = [1..3, \"a\"]; 0; }",
            "lowering error",
        ),
        (
            "module app v1.0.0; main() is int { true; }",
            "semantic error",
        ),
    ] {
        let output = run_stdin(
            &["build", "-", "-o", target.path()],
            source.as_bytes(),
        );
        assert_eq!(output.status.code(), Some(1));
        assert!(stdout(&output).is_empty());
        assert!(stderr(&output).contains(expected), "{}", stderr(&output));
        assert!(!stderr(&output).contains("codegen error"));
        assert!(!target.0.exists());
    }
}

#[test]
fn build_option_errors_are_usage_errors() {
    for args in [
        &["build"][..],
        &["build", "input.ok"],
        &["build", "-o", "out.spb"],
        &["build", "input.ok", "-o"],
        &["build", "input.ok", "--output="],
        // Файлы одного модуля можно перечислить, но не вместе со stdin.
        &[
            "build",
            "input.ok",
            "-",
            "-o",
            "out.spb",
        ],
        &[
            "build",
            "input.ok",
            "-o",
            "out.spb",
            "--spans",
        ],
        &[
            "build",
            "input.ok",
            "-o",
            "out.spb",
            "--format",
            "text",
        ],
        &["ir", "input.ok", "--target", "sparrow"],
        &["ast", "input.ok", "-o", "out.spb"],
    ] {
        let output = run(args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(stdout(&output).is_empty(), "{args:?}");
        assert!(stderr(&output).contains("okkama:"), "{args:?}");
    }
}

#[test]
fn build_reports_input_io_errors() {
    let target = TempFile::absent("build-unreadable.spb");
    let output = run(&[
        "build",
        "/nonexistent/okkama/input.ok",
        "-o",
        target.path(),
    ]);
    assert_eq!(output.status.code(), Some(3));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("cannot read"));
    assert!(!target.0.exists());
}

#[test]
fn build_reports_output_write_errors() {
    let parent = TempFile::absent("build-missing-parent");
    let path = parent.0.join("out.spb");
    let output = run_stdin(
        &[
            "build",
            "-",
            "-o",
            path.to_str().unwrap(),
        ],
        HELLO.as_bytes(),
    );
    assert_eq!(output.status.code(), Some(3));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("cannot write"));
    assert!(!parent.0.exists());
}

#[test]
fn production_mode_and_pruning() {
    let path = example("tests-and-fakes.ok");
    let full = stdout(&run(&["ast", &path]));
    assert!(
        full.contains("TestDecl") && full.contains("FakeBody") && !full.contains("SkippedBody")
    );
    let production = stdout(&run(&["ast", &path, "--mode", "production"]));
    assert!(production.contains("TestDecl") && production.contains("SkippedBody"));
    let pruned = stdout(&run(&[
        "ast",
        &path,
        "--mode",
        "production",
        "--prune-tests",
    ]));
    assert!(!pruned.contains("TestDecl") && !pruned.contains("FakeDecl"));
    let pruned_full = stdout(&run(&["ast", &path, "--prune-tests"]));
    assert_eq!(pruned, pruned_full, "pruning makes both modes equal");
}

#[test]
fn syntax_errors_exit_one_with_nothing_on_stdout() {
    let file = TempFile::new(
        "bad.ok",
        b"module app v1.0.0;\nmain() is int {\n    let x = 1 2;\n}\n",
    );
    for command in ["ast", "tokens", "ir"] {
        let output = run(&[command, file.path()]);
        if command == "tokens" {
            // The token stream itself is valid.
            assert_eq!(output.status.code(), Some(0));
            continue;
        }
        assert_eq!(output.status.code(), Some(1));
        assert!(stdout(&output).is_empty());
        let message = stderr(&output);
        assert!(
            message.starts_with(&format!(
                "{}:3:15: syntax error at \"2\": Expect \";\" after let declaration.\n",
                file.path()
            )),
            "{message}"
        );
        assert!(message.contains("3 |     let x = 1 2;\n"), "{message}");
    }
}

#[test]
fn lexical_errors_exit_one_for_both_commands() {
    let file = TempFile::new("lex.ok", b"module app v1.0.0; main() is int { \"open }\n");
    for command in ["ast", "tokens", "ir"] {
        let output = run(&[command, file.path()]);
        assert_eq!(output.status.code(), Some(1), "{command}");
        assert!(stdout(&output).is_empty(), "{command}");
        assert!(
            stderr(&output).contains("lexical error: Unterminated string."),
            "{command}"
        );
    }
}

#[test]
fn invalid_utf8_is_a_source_error() {
    let file = TempFile::new("latin1.ok", b"module app v1.0.0; // caf\xe9\n");
    for command in ["ast", "ir"] {
        let output = run(&[command, file.path()]);
        assert_eq!(output.status.code(), Some(1));
        assert!(stdout(&output).is_empty());
        assert!(stderr(&output).contains("not valid UTF-8 (invalid byte at offset 25)"));
    }
}

#[test]
fn unreadable_input_exits_three() {
    for command in ["ast", "ir"] {
        let output = run(&[command, "/nonexistent/okkama/input.ok"]);
        assert_eq!(output.status.code(), Some(3));
        assert!(stdout(&output).is_empty());
        assert!(stderr(&output).contains("cannot read `/nonexistent/okkama/input.ok`"));
    }
}

#[test]
fn usage_errors_exit_two() {
    for args in [
        &["ast"][..],
        &["ast", "a.ok", "b.ok"],
        &["ast", "a.ok", "--format", "xml"],
        &["ast", "a.ok", "--mode", "debug"],
        &["ast", "a.ok", "--format"],
        &["tokens", "a.ok", "--spans"],
        &["tokens", "a.ok", "--format", "tree"],
        &["ast", "a.ok", "--colour"],
        &["ir"],
        &["ir", "-", "b.ok"],
        &["ir", "a.ok", "--format", "json"],
        &["ir", "a.ok", "--format"],
        &["ir", "a.ok", "--spans"],
        &["ir", "a.ok", "--mode", "production"],
        &["run", "--frobnicate", "a.ok"],
        &["compile", "a.ok"],
        &["jit", "a.ok"],
    ] {
        let output = run(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(stdout(&output).is_empty(), "{args:?}");
        assert!(stderr(&output).contains("okkama:"), "{args:?}");
    }
}

#[test]
fn every_project_example_parses() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "ok") {
            count += 1;
            let output = run(&["ast", path.to_str().unwrap(), "--format", "json"]);
            assert_eq!(
                output.status.code(),
                Some(0),
                "{}: {}",
                path.display(),
                stderr(&output)
            );
        }
    }
    assert!(count >= 5);
}

#[test]
fn check_reports_errors_that_only_lowering_finds() {
    // #317: what `okkama run` rejects at compile time, `okkama check` rejects.
    let grow = "module app v1.0.0; \
        object Box<T> { has value is T; Box(:value); } \
        object Grow<T> { has value is T; Grow(:value); \
        deeper(depth is int) is int { if depth == 0 { return 0; } Grow(value: Box(value: value)).deeper(depth - 1) + 1; } } \
        main() is int { Grow(value: 1).deeper(3); }";
    let output = run_stdin(&["check", "-"], grow.as_bytes());
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("semantic error: The program requests a chain of more than 256 generic specializations"),
        "{}",
        stderr(&output)
    );
    let spin = "module app v1.0.0; object Spin { Spin(); spin(n is int) is int { n + self.spin(n + 1); } } main() is int { Spin().spin(0); }";
    let output = run_stdin(&["check", "-"], spin.as_bytes());
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stderr(&output).contains("Infinite recursion"), "{}", stderr(&output));
}

#[test]
fn check_reports_warnings_without_failing() {
    let source = "module app v1.0.0; main() is int { let n = 2; for 0..n -> index { index; } }";
    let output = run_stdin(&["check", "-"], source.as_bytes());
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).is_empty());
    assert_eq!(
        stderr(&output),
        "<stdin>:1:47: warning: For loop value is undefined when its source is empty: the body may not run\n"
    );
    let output = run_stdin(
        &["check", "-"],
        b"module app v1.0.0; main() is int { let a = 1 + true; let b = missing; 0; }",
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "<stdin>:1:44: semantic error: Operator + cannot be applied to int and bool\n\
         <stdin>:1:62: semantic error: Undefined variable \"missing\"\n"
    );
}

#[test]
fn several_files_form_one_module() {
    // §5.1: файлы — одно пространство объявлений; ошибка — у своего файла.
    let main = TempFile::new(
        "multi-main.ok",
        b"module app v1.0.0;\nmain() is int { Greeter().code(); }\n",
    );
    let part = TempFile::new(
        "multi-part.ok",
        b"module app v1.0.0;\nobject Greeter { Greeter(); code() is int { 7; } }\n",
    );
    let output = run(&["check", main.path(), part.path()]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let output = run(&["ir", main.path(), part.path()]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("object Greeter {"));
    let other = TempFile::new(
        "multi-other.ok",
        b"module app v1.0.1;\nobject Greeter { Greeter(); }\nmain() is int { 1; }\n",
    );
    let output = run(&["check", main.path(), other.path()]);
    assert_eq!(output.status.code(), Some(1));
    let text = stderr(&output);
    assert!(
        text.contains(&format!(
            "{}:1:1: semantic error: module \"app\" has conflicting versions \"v1.0.0\" and \"v1.0.1\"",
            other.path()
        )),
        "{text}"
    );
    assert!(
        text.contains("semantic error: Module declares main() more than once"),
        "{text}"
    );
}

/// A temporary directory tree that is removed when dropped.
struct TempTree(PathBuf);

impl TempTree {
    fn new(name: &str, files: &[(&str, &str)]) -> TempTree {
        let root =
            std::env::temp_dir().join(format!("okkama-cli-tree-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in files {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        TempTree(root)
    }

    fn path(&self, relative: &str) -> String {
        self.0.join(relative).to_str().unwrap().to_string()
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Проект blade: зависимость `web` со своим `util` и своей зависимостью
/// `json`, плюс файлы в `pkg/`, которые не должны ничего перехватить.
fn blade_project(name: &str) -> TempTree {
    TempTree::new(
        name,
        &[
            (
                "blade.toml",
                "[module]\nname = \"app\"\n\n[dependencies]\nweb = \"12.3.0\"\n",
            ),
            (
                "src/main.ok",
                "module app;\nimport \"io/stdout\" take Stdout;\nimport \"util\" take Greeting;\n\
                 import \"pkg/web/router\" take Router;\n\
                 main() is int { Stdout().writeln(Greeting().text(), \" \", Router().route()); 0; }\n",
            ),
            (
                "src/util.ok",
                "module app/util;\nobject Greeting { Greeting(); text() is string { \"app\"; } }\n",
            ),
            (
                "pkg/web/blade.toml",
                "[module]\nname = \"web\"\n\n[dependencies]\njson = \"1.0.0\"\n",
            ),
            (
                "pkg/web/router.ok",
                "module web/router;\nimport \"util\" take Path;\nimport \"pkg/json/codec\" take Codec;\n\
                 object Router { Router(); route() is string { Path().text() + \"+\" + Codec().name(); } }\n",
            ),
            (
                "pkg/web/util.ok",
                "module web/util;\nobject Path { Path(); text() is string { \"web\"; } }\n",
            ),
            ("pkg/json/blade.toml", "[module]\nname = \"json\"\n"),
            (
                "pkg/json/codec.ok",
                "module json/codec;\nobject Codec { Codec(); name() is string { \"json\"; } }\n",
            ),
            (
                "pkg/util.ok",
                "module trap/util;\nobject Path { Path(); }\n",
            ),
            (
                "pkg/io/stream.ok",
                "module io/stream;\nobject Hijack { Hijack(); }\n",
            ),
        ],
    )
}

fn in_dir(dir: &str, args: &[&str]) -> Output {
    let std = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../std");
    okkama()
        .args(args)
        .current_dir(dir)
        .env("OKKAMA_STDLIB_DIR", &std)
        .output()
        .unwrap()
}

#[test]
fn blade_project_imports_resolve_from_package_roots_and_lib() {
    let project = blade_project("resolve");
    let main = project.path("src/main.ok");
    for cwd in [
        project.path(""),
        project.path("src"),
        std::env::temp_dir().to_str().unwrap().to_string(),
    ] {
        let output = in_dir(&cwd, &["run", &main]);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        assert_eq!(stdout(&output), "app web+json\n");
    }
}

#[test]
fn blade_project_rejects_undeclared_dependencies_and_file_versions() {
    let project = blade_project("reject");
    let undeclared = project.path("src/undeclared.ok");
    std::fs::write(
        &undeclared,
        "module app;\nimport \"pkg/json/codec\" take Codec;\nmain() is int { 0; }\n",
    )
    .unwrap();
    let text = stderr(&in_dir(&project.path(""), &["check", &undeclared]));
    assert!(
        text.contains(
            ":2:8: semantic error: \"json\" is not a dependency: add it to [dependencies] in "
        ),
        "{text}"
    );

    let versioned = project.path("src/versioned.ok");
    std::fs::write(&versioned, "module app v1.0.0;\nmain() is int { 0; }\n").unwrap();
    let text = stderr(&in_dir(&project.path(""), &["check", &versioned]));
    assert!(
        text.contains(
            ":1:12: semantic error: Module version v1.0.0 is not allowed in a blade project"
        ),
        "{text}"
    );

    let codec = project.path("pkg/json/codec.ok");
    let original = std::fs::read_to_string(&codec).unwrap();
    std::fs::write(
        &codec,
        original.replace("module json/codec;", "module json/codec v1.0.0;"),
    )
    .unwrap();
    let text = stderr(&in_dir(
        &project.path(""),
        &["check", &project.path("src/main.ok")],
    ));
    assert!(
        text.contains("pkg/json/codec.ok:1:19: semantic error: Module version v1.0.0"),
        "{text}"
    );

    // Вне blade-проекта `pkg/` — обычный подкаталог.
    let loose = TempTree::new(
        "loose",
        &[
            (
                "main.ok",
                "module loose;\nimport \"pkg/shapes\" take Square;\nmain() is int { Square().side(); }\n",
            ),
            (
                "pkg/shapes.ok",
                "module pkg/shapes;\nobject Square { Square(); side() is int { 4; } }\n",
            ),
        ],
    );
    let output = in_dir(&loose.path(""), &["run", &loose.path("main.ok")]);
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
}

/// Sparrow refuses a function whose live registers exceed its compile budget.
#[test]
fn functions_with_huge_frames_are_refused_by_sparrow() {
    let count = 20_000;
    let lets: String = (0..count).map(|i| format!("        let v{i} = seed + {i};\n")).collect();
    let values: Vec<String> = (0..count).map(|i| format!("v{i}")).collect();
    let source = format!(
        "module app v1.0.0;\nobject Wide {{\n    Wide();\n    sum(seed is int) is int {{\n{lets}        let values is @int = [{}];\n        values.reduce(0) -> total, value {{ total + value; }};\n    }}\n}}\nmain() is int {{ let wide = Wide(); (wide.sum(1) + wide.sum(2)) % 251; }}\n",
        values.join(", ")
    );
    let file = TempFile::new("wide-frame.ok", source.as_bytes());
    let output = okkama().args(["run"]).arg(&file.0).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("Wide.sum: compiling it would take more than 256 MiB"),
        "{}",
        stderr(&output)
    );
}

/// A `cell::Cell` stays in synchronous code (#152): a field cannot hold one
/// and an async action cannot use one; a sync action that does may be called
/// from async code.
#[test]
fn a_cell_stays_in_synchronous_code() {
    let source = "module app v1.0.0;\nimport \"cell\" take Cell;\n\
        object Counter { has count is @Cell<int>; Counter() { count = []; } }\n\
        object Work {\n    Work();\n    async total(items is @int) is int { let sum = Cell(0); sum.value(); }\n\
        sum(items is @int) is int { let sum = Cell(0); for items -> item { sum.replace(sum.value() + item); } sum.value(); }\n\
        async fine(items is @int) is int { self.sum(items); }\n}\n\
        async main() is int { await Work().fine([1]); }\n";
    let std = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../std");
    let mut child = okkama()
        .args(["check", "-"])
        .env("OKKAMA_STDLIB_DIR", std)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn okkama");
    child.stdin.take().unwrap().write_all(source.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    let stderr = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("Field \"count\" cannot hold a Cell"), "{stderr}");
    assert!(stderr.contains("A Cell cannot be used in an async action"), "{stderr}");
    assert_eq!(stderr.matches("error").count(), 2, "{stderr}");
}

#[test]
fn an_uncaught_error_text_keeps_its_payload_on_one_line() {
    // #306: a string payload is an Okkama literal, so a newline in it does
    // not split the report.
    let source = b"module app v1.0.0;\nenum Ferr is Error = (text(s is string));\nmain() is int { throw Ferr.text(\"line1\\nline2 \\\"q\\\"\"); }\n";
    {
        let engine = "sparrow";
        let mut command = okkama();
        command.args(["run", "-"]).env("OKKAMA_TRACE", "off");
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        child.stdin.take().unwrap().write_all(source).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{engine}");
        assert_eq!(stderr(&output), "Ferr: Ferr.text(\"line1\\nline2 \\\"q\\\"\")\n", "{engine}");
    }
}

/// A cancelled process worker (#359): its Future settles Cancelled once the
/// child has stopped and been reaped, as on Tomato, and a worker cancelled
/// before its process started never starts (it used to run to its end and
/// print after the cancel).
#[test]
fn a_cancelled_worker_stops_before_its_future_settles() {
    let source = "module app v1.0.0;\n\
        import \"io/stdout\" take Stdout;\n\
        import \"time/clock\" take SystemClock;\n\
        import \"time/duration\" take Duration, DurationUnit;\n\
        object W {\n    W();\n\
            noisy() is int { Stdout().writeln(\"worker ran\"); 1; }\n\
            slow(count is int) is int { (0..count).reduce(0) -> total, i { total + i; }; }\n}\n\
        async main() is int {\n\
            let early = spawn W().noisy();\n\
            early.cancel();\n\
            let first = try { await early; } catch e is Cancelled { -1; };\n\
            let late = spawn W().slow(count: 100000000000);\n\
            await SystemClock().wait(Duration(1.rat(), DurationUnit.second));\n\
            late.cancel();\n\
            let second = try { await late; } catch e is Cancelled { -1; };\n\
            Stdout().writeln(first.string() + \" \" + second.string() + \" \" + early.cancelled().string() + \" \" + late.cancelled().string());\n\
            await SystemClock().wait(Duration(2.rat(), DurationUnit.second));\n\
            Stdout().writeln(\"end\");\n\
            0;\n}\n";
    let file = TempFile::new("cancelled-worker.ok", source.as_bytes());
    {
        let engine = "sparrow";
        let started = std::time::Instant::now();
        let output = okkama().args(["run"]).arg(&file.0).output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{engine}: {}", stderr(&output));
        assert_eq!(stdout(&output), "-1 -1 true true\nend\n", "{engine}");
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "{engine}");
    }
}

/// #483, #484: a cursor loop owns each cursor for one step. The step's region
/// takes the cursor before `available()`, `successor()` is not tracked by the
/// scope around the loop, and the step closes keeping the new cursor: no
/// record of a past cursor stays behind, whatever the number of steps.
#[test]
fn traversal_steps_own_their_cursor_and_keep_no_record_of_past_ones() {
    let std = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../std");
    let source = TempFile::new(
        "steps.ok",
        b"module app v1.0.0;\n\
          import \"collections/iteration\" take Iterating, Cursor, IterationEnded;\n\
          object Res { Res(); destroy {} }\n\
          object View does Iterating<int> { has res is Res; View(res is Res) { self.res = res; } cursor() is Cursor<int> { Step(res, 0); } }\n\
          object Step does Cursor<int> { has res is Res; has at is int; Step(res is Res, at is int) { self.res = res; self.at = at; }\n\
              available() is bool { at < 3; } item() is int { at; } successor() is Cursor<int> { Step(res, at + 1); } }\n\
          main() is int { for View(Res()) -> n { n; } 0; }\n",
    );
    let output = okkama().args(["ir", source.path()]).env("OKKAMA_STDLIB_DIR", &std).output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    let main = &text[text.find("func @main").expect("main")..];
    let main = &main[..main.find("\n}").expect("end of main")];
    let line = |needle: &str| {
        main.lines()
            .find(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("no {needle} in\n{main}"))
            .trim()
            .to_string()
    };
    let value = |line: &str| line.split(':').next().unwrap().trim().to_string();
    // The source is adopted by the scope around the loop.
    let source_wrap = value(&line("= role.wrap"));
    assert!(main.contains(&format!("region.adopt %0, {source_wrap}")), "{main}");
    // The step's region takes the cursor before available().
    let step_region = value(main.lines().filter(|line| line.contains("= region.enter")).nth(1).expect("a step region").trim());
    let adopt = main.find(&format!("region.adopt {step_region},")).expect("the step adopts its cursor");
    assert!(adopt < main.find("available()").unwrap(), "{main}");
    // successor() is kept by the step's close and tracked nowhere.
    let successor = value(&line("successor()"));
    assert!(main.contains(&format!("region.leave {step_region}, keep {successor}")), "{main}");
    assert!(!main.contains(&format!("region.track %0, {successor}")), "{main}");
}

// ---- okkama man (#512)

#[test]
fn man_keyword_search_covers_paths_names_and_summaries_and_validates_arguments() {
    let dir = std::env::temp_dir().join(format!("okkama-man-search-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("area")).unwrap();
    std::fs::write(dir.join("area/demo.ok"), "module area/demo;\n\n// Module overview.\n\n// Unusual résumé.\nobject Widget {\n    // Send a parcel.\n    send() is void {}\n    // Hidden needle.\n    hidden secret() is void {}\n}\n// Hidden needle.\nhidden object Secret { Secret() {} }\n").unwrap();
    let search = |args: &[&str]| okkama().arg("man").args(args)
        .env("OKKAMA_STDLIB_DIR", &dir).env_remove("OKKAMA_IMPORT_DIRS")
        .env_remove("PAGER").output().unwrap();
    for query in ["AREA/DEMO", "widget", "RÉSUMÉ", "parcel"] {
        let output = search(&["-k", query]);
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(stdout(&output).contains("area/demo"), "{}", stdout(&output));
    }
    let output = search(&["--format", "markdown", "-k", "parcel"]);
    assert!(stdout(&output).contains("Widget.send"), "{}", stdout(&output));
    let output = search(&["-k", "needle"]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("No matches"));
    assert!(!stdout(&output).contains("Secret"));
    for args in [&["-k"][..], &["-k", ""], &["-k", "x", "area/demo"], &["-k", "x", "-k", "y"]] {
        assert_eq!(search(args).status.code(), Some(2), "{args:?}");
    }
    assert!(stdout(&man(&["--help"])).contains("-k QUERY"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn man_declaration_links_include_external_modules_and_warn_without_failing() {
    let dir = std::env::temp_dir().join(format!("okkama-man-links-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("other.ok"), "module other;\n// The external role.\nrole Writing { write() is void; }\n").unwrap();
    let file = dir.join("app.ok");
    std::fs::write(&file, "module app;\n\n// See [Thing.write], [other.Writing], [Missing].\n\nobject Thing {\n    write(x is int) is void {}\n    write(x is string) is void {}\n}\n").unwrap();
    for format in ["text", "markdown", "man", "html", "json"] {
        let output = man(&["--format", format, file.to_str().unwrap()]);
        assert!(output.status.success(), "{}", stderr(&output));
        let page = stdout(&output);
        assert!(page.contains("okkama-doc:/app#Thing.write"), "{page}");
        assert!(page.contains("okkama-doc:/other#Writing"), "{page}");
        if format == "html" { assert!(page.contains("href=\"other.html#Writing\""), "{page}"); }
        assert!(stderr(&output).contains("warning: unresolved documentation link [Missing]"), "{}", stderr(&output));
        assert!(stderr(&output).contains("app.ok:3:"));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

/// A bounded protocol client for the documentation regressions (#513).
struct DocLsp {
    child: std::process::Child,
    input: std::process::ChildStdin,
    messages: std::sync::mpsc::Receiver<serde_json::Value>,
    next: u32,
}

impl DocLsp {
    fn new() -> Self {
        Self::with_stdlib(&std_dir())
    }

    fn with_stdlib(stdlib: &std::path::Path) -> Self {
        let mut child = okkama().arg("lsp").env("OKKAMA_STDLIB_DIR", stdlib)
            .env_remove("OKKAMA_IMPORT_DIRS").stdin(Stdio::piped()).stdout(Stdio::piped())
            .stderr(Stdio::null()).spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
        let (sender, messages) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(Some(message)) = lsp_server::Message::read(&mut output) {
                if sender.send(serde_json::to_value(message).unwrap()).is_err() { break; }
            }
        });
        let mut client = Self { child, input, messages, next: 0 };
        client.request("initialize", serde_json::json!({"capabilities": {}}));
        client.notify("initialized", serde_json::json!({}));
        client
    }

    fn send(&mut self, message: serde_json::Value) {
        let text = message.to_string();
        write!(self.input, "Content-Length: {}\r\n\r\n{}", text.len(), text).unwrap();
        self.input.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: serde_json::Value) {
        self.send(serde_json::json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.next += 1;
        let id = self.next;
        self.send(serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let message = self.messages.recv_timeout(std::time::Duration::from_secs(20)).expect("LSP response within 20 seconds");
            if message.get("id") == Some(&serde_json::json!(id)) {
                assert!(message.get("error").is_none(), "{message}");
                return message["result"].clone();
            }
        }
    }
}

#[test]
fn lsp_doc_std_completion_resolves_links_from_unsaved_public_model() {
    let dir = std::env::temp_dir().join(format!("okkama-lsp-std-doc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let library = "module demo;\n// See [First].\nobject First {\n    First() {}\n    // Member [First.send].\n    send(x is int = 2) is void {}\n}\nhidden object Secret { Secret() {} }\n";
    let file = dir.join("demo.ok");
    std::fs::write(&file, library).unwrap();
    let imported_uri = format!("file://{}", file.display());
    let uri = format!("file://{}", dir.join("app.ok").display());
    let text = "module app;\nmain() is int { std.demo.";
    let mut client = DocLsp::with_stdlib(&dir);
    client.notify("textDocument/didOpen", serde_json::json!({"textDocument": {"uri": imported_uri, "version": 1, "text": library.replace("Member", "Unsaved member")}}));
    client.notify("textDocument/didOpen", serde_json::json!({"textDocument": {"uri": uri, "version": 1, "text": text}}));
    let completion = client.request("textDocument/completion", serde_json::json!({"textDocument": {"uri": uri}, "position": {"line": 1, "character": text.lines().nth(1).unwrap().len()}}));
    let items = completion.as_array().unwrap();
    assert!(!items.iter().any(|item| item["label"] == "Secret"));
    let first = items.iter().find(|item| item["label"] == "First").unwrap();
    assert!(first["documentation"]["value"].as_str().unwrap().contains("Unsaved member"));
    let resolved = client.request("completionItem/resolve", first.clone());
    let documentation = resolved["documentation"]["value"].as_str().unwrap();
    assert!(documentation.contains("Unsaved member"), "{documentation}");
    assert!(documentation.contains("x is int = 2"));
    assert!(documentation.contains("[First.send](okkama-doc:/demo#First.send)"));
    assert!(!documentation.contains("Secret"));
    client.notify("textDocument/didChange", serde_json::json!({"textDocument": {"uri": imported_uri, "version": 2}, "contentChanges": [{"text": library.replace("object First", "hidden object First")}]}));
    let hidden = client.request("completionItem/resolve", first.clone());
    assert!(hidden.get("documentation").is_none(), "{hidden}");
    client.request("shutdown", serde_json::Value::Null);
    client.notify("exit", serde_json::Value::Null);
    drop(client);
    std::fs::remove_dir_all(dir).unwrap();
}

impl DocLsp {
    /// `request`, with the notifications that came before its response.
    fn request_seeing(&mut self, method: &str, params: serde_json::Value) -> Vec<serde_json::Value> {
        self.next += 1;
        let id = self.next;
        self.send(serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let mut seen = Vec::new();
        loop {
            let message = self.messages.recv_timeout(std::time::Duration::from_secs(20)).expect("LSP response within 20 seconds");
            if message.get("id") == Some(&serde_json::json!(id)) {
                return seen;
            }
            seen.push(message);
        }
    }
}

/// #1017: checking a file that imports an open one does not replace the open
/// file's own diagnostics (its `missing-docs` warnings) with what the
/// importer's check sees of it, whichever of them is checked last.
#[test]
fn lsp_an_importer_does_not_replace_an_open_file_diagnostics() {
    let dir = std::env::temp_dir().join(format!("okkama-lsp-importer-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let library = "module a_lib;\nobject Thing {\n    Thing() {}\n    bad() is int { missing; }\n}\n";
    let importer = "module z_app;\nimport \"a_lib\" take Thing;\nmain() is int { 0; }\n";
    std::fs::write(dir.join("a_lib.ok"), library).unwrap();
    std::fs::write(dir.join("z_app.ok"), importer).unwrap();
    let library_uri = format!("file://{}", dir.join("a_lib.ok").display());
    let importer_uri = format!("file://{}", dir.join("z_app.ok").display());
    let mut client = DocLsp::with_stdlib(&std_dir());
    let published = |seen: &[serde_json::Value]| -> Vec<serde_json::Value> {
        seen.iter()
            .filter(|message| message["method"] == "textDocument/publishDiagnostics" && message["params"]["uri"] == library_uri.as_str())
            .map(|message| message["params"].clone())
            .collect()
    };
    client.notify("textDocument/didOpen", serde_json::json!({"textDocument": {"uri": library_uri, "version": 1, "text": library}}));
    client.notify("textDocument/didOpen", serde_json::json!({"textDocument": {"uri": importer_uri, "version": 1, "text": importer}}));
    let opened = published(&client.request_seeing("textDocument/documentSymbol", serde_json::json!({"textDocument": {"uri": importer_uri}})));
    let own = opened.first().expect("the library's own diagnostics").clone();
    assert_eq!(own["version"], 1, "{own}");
    let count = own["diagnostics"].as_array().unwrap().len();
    assert!(own["diagnostics"].as_array().unwrap().iter().any(|d| d["code"] == "missing-docs"), "{own}");
    // A change of the library checks it, then (sorted later) its importer.
    client.notify("textDocument/didChange", serde_json::json!({"textDocument": {"uri": library_uri, "version": 2}, "contentChanges": [{"text": library.replace("missing;", "missing; ")}]}));
    let changed = published(&client.request_seeing("textDocument/documentSymbol", serde_json::json!({"textDocument": {"uri": importer_uri}})));
    let last = changed.last().expect("the library's diagnostics again");
    assert_eq!(last["version"], 2, "{changed:?}");
    assert_eq!(last["diagnostics"].as_array().unwrap().len(), count, "{changed:?}");
    assert!(opened.iter().chain(&changed).all(|params| !params["version"].is_null()), "only the library publishes its own: {opened:?} {changed:?}");
    client.request("shutdown", serde_json::Value::Null);
    client.notify("exit", serde_json::Value::Null);
    drop(client);
    std::fs::remove_dir_all(dir).unwrap();
}

/// #1017: copies of an imported file's errors go on the importer's first
/// import declaration, not on the first letters `import` in a comment.
#[test]
fn lsp_copies_of_imported_errors_go_on_the_first_import() {
    let dir = std::env::temp_dir().join(format!("okkama-lsp-first-import-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    std::fs::write(dir.join("lib.ok"), "module lib;\n// A thing.\nobject Thing {\n    // A thing.\n    Thing() {}\n    // Broken.\n    bad() is int { missing; }\n}\n").unwrap();
    let importer = "module app;\n// It is important that this comes first.\n\nimport \"lib\" take Thing;\nmain() is int { 0; }\n";
    std::fs::write(dir.join("app.ok"), importer).unwrap();
    let uri = format!("file://{}", dir.join("app.ok").display());
    let mut client = DocLsp::with_stdlib(&std_dir());
    client.notify("textDocument/didOpen", serde_json::json!({"textDocument": {"uri": uri, "version": 1, "text": importer}}));
    let seen = client.request_seeing("textDocument/documentSymbol", serde_json::json!({"textDocument": {"uri": uri}}));
    let published = seen.iter().rev()
        .find(|message| message["method"] == "textDocument/publishDiagnostics" && message["params"]["uri"] == uri.as_str())
        .expect("the importer's diagnostics");
    let copies: Vec<&serde_json::Value> = published["params"]["diagnostics"].as_array().unwrap().iter()
        .filter(|diagnostic| diagnostic["message"].as_str().is_some_and(|text| text.starts_with("lib.ok:")))
        .collect();
    assert!(!copies.is_empty(), "{published}");
    for copy in copies {
        assert_eq!(copy["range"]["start"], serde_json::json!({"line": 3, "character": 0}), "{copy}");
    }
    drop(client);
    std::fs::remove_dir_all(dir).unwrap();
}

/// #1017: `exit` without `shutdown` ends the server with code 1; after
/// `shutdown`, with 0.
#[test]
fn lsp_exit_without_shutdown_ends_with_one() {
    for (shutdown, code) in [(false, 1), (true, 0)] {
        let mut client = DocLsp::with_stdlib(&std_dir());
        if shutdown {
            client.request("shutdown", serde_json::Value::Null);
        }
        client.notify("exit", serde_json::Value::Null);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let status = loop {
            if let Some(status) = client.child.try_wait().unwrap() {
                break status;
            }
            assert!(std::time::Instant::now() < deadline, "the server did not end");
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(status.code(), Some(code), "shutdown first: {shutdown}");
    }
}

impl Drop for DocLsp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn lsp_doc_model_matches_man_and_tracks_open_change_and_close_of_imports() {
    let dir = std::env::temp_dir().join(format!("okkama-lsp-doc-integration-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let library = "module money;\n// Public [Money] and [Money.plus].\nobject Money {\n    // Create an amount.\n    Money() {}\n    // Integer amount.\n    plus(x is int = 1) is int { x; }\n    // Text amount.\n    plus(x is string) is string { x; }\n    // Private words.\n    hidden secret() is void {}\n}\n// Private type.\nhidden object Secret { Secret() {} }\n";
    let path = dir.join("money.ok");
    std::fs::write(&path, library).unwrap();
    let text = "module app;\nimport \"money\" take Money;\nmain() is int { let m = Money(); let _s = m.plus(\"x\"); m.plus(2); }\n";
    let uri = format!("file://{}", dir.join("app.ok").display());
    let imported_uri = format!("file://{}", path.display());
    let position = |needle: &str| serde_json::json!({"textDocument": {"uri": uri}, "position": {"line": 2, "character": text.lines().nth(2).unwrap().find(needle).unwrap() + 1}});
    let mut client = DocLsp::new();
    client.notify("textDocument/didOpen", serde_json::json!({"textDocument": {"uri": uri, "languageId": "okkama", "version": 1, "text": text}}));
    let hover = client.request("textDocument/hover", position("plus(2)"));
    let start = text.lines().nth(2).unwrap().find("plus(2)").unwrap();
    assert_eq!(hover["range"], serde_json::json!({"start": {"line": 2, "character": start}, "end": {"line": 2, "character": start + 4}}));
    let hover = hover["contents"]["value"].as_str().unwrap();
    assert!(hover.contains("Integer amount"), "{hover}");
    assert!(hover.contains("x is int = 1"));
    assert!(!hover.contains("Text amount"));
    assert!(!hover.contains("Private"));
    let page = man(&["--format", "markdown", path.to_str().unwrap(), "Money.plus"]);
    assert!(page.status.success(), "{}", stderr(&page));
    assert!(stdout(&page).contains("Integer amount.") && stdout(&page).contains("Text amount."));
    let imported = client.request("textDocument/hover", serde_json::json!({"textDocument": {"uri": uri}, "position": {"line": 1, "character": 22}}));
    assert!(imported["contents"]["value"].as_str().unwrap().contains("[Money.plus](okkama-doc:/money#Money.plus)"));
    client.notify("textDocument/didOpen", serde_json::json!({"textDocument": {"uri": imported_uri, "languageId": "okkama", "version": 1, "text": library.replace("Integer amount", "Opened buffer")}}));
    let opened = client.request("textDocument/hover", position("plus(2)"));
    assert!(opened["contents"]["value"].as_str().unwrap().contains("Opened buffer"), "{opened}");
    client.notify("textDocument/didChange", serde_json::json!({"textDocument": {"uri": imported_uri, "version": 2}, "contentChanges": [{"text": library.replace("Integer amount", "Changed buffer")}]}));
    let changed = client.request("textDocument/hover", position("plus(2)"));
    assert!(changed["contents"]["value"].as_str().unwrap().contains("Changed buffer"), "{changed}");
    let completion = client.request("textDocument/completion", position("plus(2)"));
    let completions = completion.as_array().unwrap();
    assert!(completions.iter().any(|item| item["label"] == "plus" && item["documentation"]["value"].as_str().is_some_and(|value| value.contains("Changed buffer"))), "{completion}");
    assert!(!completions.iter().any(|item| item["label"] == "secret"));
    client.notify("textDocument/didClose", serde_json::json!({"textDocument": {"uri": imported_uri}}));
    let closed = client.request("textDocument/hover", position("plus(2)"));
    assert!(closed["contents"]["value"].as_str().unwrap().contains("Integer amount"), "{closed}");
    client.request("shutdown", serde_json::Value::Null);
    client.notify("exit", serde_json::Value::Null);
    drop(client);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn man_broken_links_are_lint_warnings_with_the_original_location() {
    let file = TempFile::new("doc-warning.ok", b"module example;\n\n// See [Missing].\n\nobject Public { Public() {} }\n");
    let output = run(&["lint", file.path()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains(":3:8: warning[unresolved-doc-link]: unresolved documentation link [Missing]"), "{}", stdout(&output));
    assert!(!stdout(&output).contains("error:"));
}

fn std_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../std")
}

fn man(args: &[&str]) -> Output {
    okkama()
        .arg("man")
        .args(args)
        .env("OKKAMA_STDLIB_DIR", std_dir())
        .env_remove("OKKAMA_IMPORT_DIRS")
        .env_remove("PAGER")
        .output()
        .expect("run okkama man")
}

#[test]
fn man_shows_a_std_module_with_or_without_the_std_prefix() {
    let output = man(&["io/stdout"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let page = stdout(&output);
    assert!(page.starts_with("NAME\n    io/stdout"), "{page}");
    assert!(page.contains("\nVERSION\n    v1.0.0\n"), "{page}");
    assert!(page.contains("\nPUBLIC API\n"), "{page}");
    assert!(page.contains("    object Stdout does Writing, WritingText\n"), "{page}");
    // Through a pipe: no escapes, at most 80 columns of prose.
    assert!(!page.contains('\u{1b}'));
    assert_eq!(stdout(&man(&["std/io/stdout"])), page);
}

#[test]
fn man_shows_a_declaration_and_an_action() {
    let output = man(&["io/stdout", "Stdout"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let page = stdout(&output);
    assert!(page.starts_with("NAME\n    io/stdout Stdout"), "{page}");
    assert!(page.contains("\nDECLARATION\n    object Stdout"), "{page}");
    let output = man(&["io/stdout", "Stdout.writeln"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let page = stdout(&output);
    assert!(page.starts_with("NAME\n    io/stdout Stdout.writeln"), "{page}");
    assert!(page.contains("writeln(values is ...string) is void"), "{page}");
    assert!(!page.contains("flush() is void"), "{page}");
}

#[test]
fn man_refuses_unknown_modules_and_names_clearly() {
    let output = man(&["no/such/module"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no module no/such/module"), "{}", stderr(&output));
    let output = man(&["io/stdout", "Nope"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("has no public declaration `Nope`; it has Stdout"), "{}", stderr(&output));
    let output = man(&["io/stdout", "Stdout.nope"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("has no public member `nope`"), "{}", stderr(&output));
    // #1017: each member name once, though overloads of `write` are apart.
    let listed = stderr(&output).split("; it has ").nth(1).unwrap_or_default().trim_end().to_owned();
    let names: Vec<&str> = listed.split(", ").collect();
    assert!(names.contains(&"write") && names.iter().filter(|name| **name == "write").count() == 1, "{listed}");
    let output = man(&["../etc/passwd"]);
    assert_eq!(output.status.code(), Some(1));
    let output = man(&["io/../../etc"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("is not a module path"), "{}", stderr(&output));
    let output = man(&["--format", "pdf", "io/stdout"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn man_reads_a_file_of_ones_own_and_refuses_an_ambiguous_path() {
    let dir = std::env::temp_dir().join(format!("okkama-cli-man-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("json.ok"),
        "module json;\n\n// A json of my own.\n\n// Says hello.\nobject Hello {\n    Hello() {}\n}\n",
    )
    .unwrap();
    let local = okkama()
        .args(["man", "./json.ok"])
        .current_dir(&dir)
        .env("OKKAMA_STDLIB_DIR", std_dir())
        .env_remove("PAGER")
        .output()
        .unwrap();
    assert!(local.status.success(), "{}", stderr(&local));
    let page = stdout(&local);
    assert!(page.starts_with("NAME\n    json - A json of my own.\n"), "{page}");
    assert!(!page.contains("VERSION"), "{page}");
    let ambiguous = okkama()
        .args(["man", "json"])
        .current_dir(&dir)
        .env("OKKAMA_STDLIB_DIR", std_dir())
        .env_remove("PAGER")
        .output()
        .unwrap();
    assert_eq!(ambiguous.status.code(), Some(1));
    assert!(stderr(&ambiguous).contains("is both"), "{}", stderr(&ambiguous));
    let std_json = okkama()
        .args(["man", "std/json"])
        .current_dir(&dir)
        .env("OKKAMA_STDLIB_DIR", std_dir())
        .env_remove("PAGER")
        .output()
        .unwrap();
    assert!(stdout(&std_json).starts_with("NAME\n    json - "), "{}", stdout(&std_json));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn man_writes_markdown_and_an_index() {
    let output = man(&["--format", "markdown", "io/stdout"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let page = stdout(&output);
    assert!(page.starts_with("# io/stdout\n"), "{page}");
    assert!(page.contains("### object Stdout\n\n```okkama\nobject Stdout does Writing, WritingText\n```"), "{page}");
    let output = man(&[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let index = stdout(&output);
    assert!(index.starts_with("OKKAMA STANDARD LIBRARY\n"), "{index}");
    assert!(index.contains("\nio\n"), "{index}");
    assert!(index.contains("    io/stdout "), "{index}");
    assert!(index.contains("\ntop level\n"), "{index}");
}

#[test]
fn man_survives_a_reader_that_leaves_early() {
    let mut child = okkama()
        .args(["man", "json"])
        .env("OKKAMA_STDLIB_DIR", std_dir())
        .env_remove("PAGER")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    assert!(output.status.code().is_some(), "killed by a signal");
    assert!(!stderr(&output).contains("panicked"), "{}", stderr(&output));
}

/// A role whose fake holds a `Cell` in a field: rule #152 rejects it.
const CELL_FAKE: &str = "module app v1.0.0;
import \"cell\" take Cell;
role Shelf {
    put(id is int, title is string) is void;
    fake {
        has shelf is Cell<%int:string> = Cell([:]);
        put(id is int, title is string) is void { shelf.replace(shelf.value() + [id: title]); }
    }
}
main() is int { 0; }
";

#[test]
fn check_reports_errors_in_test_blocks() {
    // #649: code that exists only in test mode is checked too.
    let source = "module app v1.0.0;\ntest \"t\" {\n    let x is int = \"no\";\n}\nmain() is int { 0; }\n";
    let output = run_stdin(&["check", "-"], source.as_bytes());
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stderr(&output);
    assert!(text.starts_with("<stdin>:3:"), "{text}");
    assert!(text.contains(": semantic error: "), "{text}");
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(stdout(&output).is_empty());
}

#[test]
fn check_reports_errors_in_fakes() {
    let output = run_stdin(&["check", "-"], CELL_FAKE.as_bytes());
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("semantic error: Field \"shelf\" cannot hold a Cell"), "{text}");
    assert_eq!(text.lines().count(), 1, "{text}");
}

#[test]
fn check_passes_valid_tests_quietly() {
    let source = "module app v1.0.0;
role Counter {
    count() is int;
    fake {
        has value is int = 3;
        count() is int { value; }
    }
}
object Twice { Twice(); of(n is int) is int { n * 2; } }
test \"twice\" {
    test.is(Twice().of(Counter.fake().count()), 6);
}
main() is int { Twice().of(0); }
";
    let output = run_stdin(&["check", "-"], source.as_bytes());
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
    // `--tests` still checks the test profile alone.
    let output = run_stdin(&["check", "--tests", "-"], source.as_bytes());
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
}

#[test]
fn check_reports_shared_errors_once() {
    // Both profiles find the error in `main`; it is reported once, and the
    // error inside the test follows it.
    let source = "module app v1.0.0;
test \"t\" {
    let x is int = \"no\";
}
main() is int { let a = 1 + true; 0; }
";
    let output = run_stdin(&["check", "-"], source.as_bytes());
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stderr(&output);
    let shared = "<stdin>:5:25: semantic error: Operator + cannot be applied to int and bool\n";
    assert_eq!(text.matches(shared).count(), 1, "{text}");
    assert!(text.starts_with(shared), "{text}");
    assert_eq!(text.lines().count(), 2, "{text}");
    assert!(text.lines().nth(1).unwrap().starts_with("<stdin>:3:"), "{text}");
    // A warning in shared code is printed once as well.
    let source = "module app v1.0.0;
test \"t\" { test.is(1, 1); }
main() is int { let n = 2; for 0..n -> index { index; } }
";
    let output = run_stdin(&["check", "-"], source.as_bytes());
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stderr(&output),
        "<stdin>:3:28: warning: For loop value is undefined when its source is empty: the body may not run\n"
    );
}

#[test]
fn check_rejects_fake_in_production_code_of_a_file_with_tests() {
    // Test mode allows `Role.fake()`; production code must still not use it.
    let source = "module app v1.0.0;
role Counter {
    count() is int;
    fake {
        has value is int = 3;
        count() is int { value; }
    }
}
test \"fake\" {
    test.is(Counter.fake().count(), 3);
}
main() is int { Counter.fake().count(); }
";
    let output = run_stdin(&["check", "-"], source.as_bytes());
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("Counter.fake() is available only in test mode"), "{text}");
    assert_eq!(text.lines().count(), 1, "{text}");
    // `okkama lint` (and the language server, which shares its path) too.
    let file = TempFile::new("fake-in-main.ok", source.as_bytes());
    let output = run(&["lint", file.path()]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("error: Counter.fake() is available only in test mode"), "{text}");
    assert_eq!(text.matches("available only in test mode").count(), 1, "{text}");
}

#[test]
fn lint_reports_errors_in_test_blocks_and_fakes() {
    let file = TempFile::new("cell-fake.ok", CELL_FAKE.as_bytes());
    let output = run(&["lint", file.path()]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("error: Field \"shelf\" cannot hold a Cell"), "{text}");
}

/// #912: `OKKAMA_DEV=runtime,lower` answers which site requests identities and how often, which site adds to
/// regions, and how many ownership operations each function has after the IR passes.
#[test]
fn devtrace_counts_ownership_by_site_and_function() {
    let source = b"module app v1.0.0;\nobject Res { has n is int; Res(:n); read() is int { n; } destroy { } }\n\
        object Maker { Maker(); make(i is int) is int { let r = Res(n: i); r.read(); } }\n\
        main() is int { Maker().make(1) + Maker().make(2) - 3; }\n";
    let out = std::env::temp_dir().join(format!("okkama-dev-{}", std::process::id()));
    let mut child = okkama()
        .args(["run", "-"])
        .env("OKKAMA_DEV", "runtime,lower")
        .env("OKKAMA_DEV_OUT", &out)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(source).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let summary = std::fs::read_to_string(out.join("summary.txt")).unwrap();
    let _ = std::fs::remove_dir_all(&out);
    let count = |name: &str| summary.lines().find(|line| line.trim_start().starts_with(name))
        .and_then(|line| line.split_whitespace().last()).and_then(|n| n.parse::<u64>().ok());
    assert_eq!(count("identity requested at Res.new#0:"), Some(2), "{summary}");
    assert!(count("ir after passes: ownership ops in Maker.make").is_some_and(|n| n > 0), "{summary}");
}

/// #552: an async body with many awaits keeps its registers in its frame (no copy per await). Ints, strings (heap
/// values), floats (bit-cast in the frame), a loop and a failure caught across an await keep their values, also
/// when the collector runs at every chance.
#[test]
fn a_big_async_body_keeps_its_values_in_its_frame() {
    let mut body = String::new();
    for i in 0..150 {
        body += &format!("        let v{i} = await self.leaf({i});\n");
    }
    for i in 0..20 {
        body += &format!("        let s{i} = await self.text({i});\n");
    }
    for i in 0..10 {
        body += &format!("        let x{i} is float = {i}.5;\n");
    }
    // Groups of 50 stay under the expression nesting limit.
    let ints: Vec<String> = (0..3)
        .map(|g| format!("({})", (g * 50..g * 50 + 50).map(|i| format!("v{i}")).collect::<Vec<_>>().join(" + ")))
        .collect();
    let floats: Vec<String> = (0..10).map(|i| format!("x{i}")).collect();
    let source = format!(
        "module app v1.0.0;\nimport \"io/stdout\" take Stdout;\nenum Stop is Error = (now(code is int));\n\
         object Wide {{\n    Wide();\n    async leaf(i is int) is int {{ i; }}\n\
             async text(i is int) is string {{ \"t\" + i.string(); }}\n\
             async fails(i is int) is int {{ confirm i < 0 else throw Stop.now(i); i; }}\n\
             async run() is int {{\n{body}\
                 let caught = try {{ await self.fails(7); }} catch Stop.now(code) {{ code; }};\n\
                 let looped = (0..5).reduce(0) -> acc, i {{ acc + i; }};\n\
                 let same = if s3 + s19 == \"t3t19\" {{ 1; }} else {{ 0; }};\n\
                 {} + ({}).int() + caught + looped + same;\n    }}\n}}\n\
         async main() is int {{ Stdout().writeln((await Wide().run()).string()); 0; }}\n",
        ints.join(" + "),
        floats.join(" + ")
    );
    // Σ0..150 = 11175, Σ(i + .5) over 0..10 = 50, caught 7, looped 10, same 1.
    for stress in ["0", "1"] {
        let mut child = okkama()
            .args(["run", "-"])
            .env("OKKAMA_STDLIB_DIR", std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../std"))
            .env("SPARROW_CORE_GC_STRESS", stress)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(source.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "stress {stress}: {}", stderr(&output));
        assert_eq!(stdout(&output), "11243\n", "stress {stress}");
    }
}

fn doctest_run(name: &str, source: &str) -> Output {
    let file = TempFile::new(name, source.as_bytes());
    let std = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../std");
    okkama().args(["run", "-t", file.path()]).env("OKKAMA_STDLIB_DIR", std).output().unwrap()
}

#[test]
fn doctest_compile_default_context_and_main_never_runs_without_opt_in() {
    let out = doctest_run("doctest-context.ok", r#"module sample;
import "io/stdout" as out;
// Examples use this file's imports and declarations.
//
// ```okkama
// main() is int { out.Stdout().writeln(Value().answer().string()); panic "must not run"; }
// ```
//
// ```okkama,ignore
// incomplete …
// ```

object Value { Value(); answer() is int { 42; } }
main() is int { panic "original main must not run"; }
"#);
    assert!(out.status.success(), "{}\n{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("doctest") && stdout(&out).contains("compile"), "{}", stdout(&out));
    // Paths include a process id, which may itself contain "42".
    assert!(!stdout(&out).lines().any(|line| line == "42"), "{}", stdout(&out));
}

#[test]
#[cfg(unix)]
fn doctest_runnable_expected_output_and_invalid_output() {
    let source = r#"module sample;
// ```okkama,run
// import "io/stdout" take Stdout;
// main() is int { Stdout().writeln("ready"); 0; }
// ```
// ```output
// ready
// ```

object Marker { Marker(); }
"#;
    let good = doctest_run("doctest-output-good.ok", source);
    assert!(good.status.success(), "{}\n{}", stdout(&good), stderr(&good));
    assert!(stdout(&good).contains("run"), "{}", stdout(&good));
    let bad = doctest_run("doctest-output-bad.ok", &source.replace("// ready\n", "// wrong\n"));
    assert_eq!(bad.status.code(), Some(1));
    assert!(stderr(&bad).contains("expected stdout") && stderr(&bad).contains("wrong"), "{}", stderr(&bad));
}

#[test]
fn doctest_compile_failure_maps_to_original_comment_and_propagates_exit() {
    let bad = doctest_run("doctest-location.ok", "module sample;\r\n// Résumé.\r\n//\r\n// ```okkama\r\n// main() is int {\r\n//     let value is int = \"bad\";\r\n//     value;\r\n// }\r\n// ```\r\n\r\nobject Marker { Marker(); }\r\n");
    assert_eq!(bad.status.code(), Some(1));
    assert!(stderr(&bad).contains("doctest-location.ok:6:"), "{}", stderr(&bad));
    let metadata = doctest_run("doctest-metadata.ok", "module sample;\n// ```okkama,run\n// 0;\n// ```\n\nobject Marker { Marker(); }\n");
    assert_eq!(metadata.status.code(), Some(1));
    assert!(stderr(&metadata).contains("expected output"), "{}", stderr(&metadata));
}

#[test]
#[cfg(unix)]
fn doctest_timeout_output_limits_and_temporary_cleanup() {
    let root = std::env::temp_dir().join(format!("okkama-doctest-cleanup-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let file = TempFile::new("doctest-timeout.ok", b"module sample;\n// ```okkama,run\n// main() is int { loop {} }\n// ```\n// ```output\n// ```\n\nobject Marker { Marker(); }\n");
    let before = std::time::Instant::now();
    let out = okkama().args(["run", "-t", file.path()]).env("TMPDIR", &root).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("timeout (3 seconds)"), "{}", stderr(&out));
    assert!(before.elapsed().as_secs() < 15);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "temporary directory leaked");
    std::fs::write(&file.0, "module sample;\n// ```okkama,run\n// main() is int { 0; }\n// ```\n// ```output\n// ```\n\nobject Marker { Marker(); }\n").unwrap();
    let success = okkama().args(["run", "-t", file.path()]).env("TMPDIR", &root).output().unwrap();
    assert!(success.status.success(), "{}", stderr(&success));
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "success leaked temporary files");
    std::fs::remove_dir(&root).unwrap();
    let output = doctest_run("doctest-limit.ok", "module sample;\n// ```okkama,run\n// import \"runtime/stdio\" take stdout_write;\n// main() is int { loop { stdout_write(@byte(65, 66, 67, 10)); } }\n// ```\n// ```output\n// ```\n\nobject Marker { Marker(); }\n");
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("output"), "{}", stderr(&output));
}

#[test]
#[cfg(unix)]
fn doctest_sandbox_denies_host_effects_and_repository_writes() {
    for (name, import, call, permission) in [
        ("net", "runtime/net\" take ip_parse", "ip_parse(1, \"127.0.0.1\", 1)", "Net"),
        ("env", "runtime/environment\" take environment_snapshot", "environment_snapshot()", "Env"),
        ("fs", "runtime/fs\" take path_encode", "path_encode(\"must-not-create\")", "Fs"),
    ] {
        let source = format!("module sample;\n// ```okkama,run\n// import \"{import};\n// main() is int {{ let _denied = {call}; 0; }}\n// ```\n// ```output\n// ```\n\nobject Marker {{ Marker(); }}\n");
        let out = doctest_run(&format!("doctest-sandbox-{name}.ok"), &source);
        assert_eq!(out.status.code(), Some(1));
        assert!(stderr(&out).contains(&format!("{permission} permission")), "{}", stderr(&out));
    }
    assert!(!PathBuf::from("must-not-create").exists());
}

#[test]
fn missing_docs_user_warning_and_std_wave_enforcement_use_file_identity() {
    let user = TempFile::new("missing-docs-user.ok", b"module io/stdout;\nobject Undocumented { Undocumented(); }\n");
    let out = run(&["lint", user.path()]);
    assert!(stdout(&out).contains("warning[missing-docs]"), "{}", stdout(&out));
    assert!(!stdout(&out).contains("error[missing-docs]"));
    let root = std::env::temp_dir().join(format!("okkama-doc-wave-{}", std::process::id()));
    std::fs::create_dir_all(root.join("io")).unwrap();
    let module = root.join("io/stdout.ok");
    std::fs::write(&module, "module io/stdout;\nobject Undocumented { Undocumented(); }\n").unwrap();
    let out = okkama().args(["lint", module.to_str().unwrap()]).env("OKKAMA_STDLIB_DIR", &root).output().unwrap();
    assert!(stdout(&out).contains("error[missing-docs]"), "{}\n{}", stdout(&out), stderr(&out));
    let gate = okkama().args(["run", "-t", module.to_str().unwrap()]).env("OKKAMA_STDLIB_DIR", &root).output().unwrap();
    assert_eq!(gate.status.code(), Some(1));
    assert!(stderr(&gate).contains("missing-docs"), "{}", stderr(&gate));
    std::fs::write(&module, "module renamed;\nobject Undocumented { Undocumented(); }\n").unwrap();
    let renamed = okkama().args(["lint", module.to_str().unwrap()]).env("OKKAMA_STDLIB_DIR", &root).output().unwrap();
    assert!(stdout(&renamed).contains("error[missing-docs]"), "{}", stdout(&renamed));
    let outside = root.join("other.ok");
    std::fs::write(&outside, "module other;\nobject Undocumented { Undocumented(); }\n").unwrap();
    let out = okkama().args(["lint", outside.to_str().unwrap()]).env("OKKAMA_STDLIB_DIR", &root).output().unwrap();
    assert!(stdout(&out).contains("warning[missing-docs]"), "{}", stdout(&out));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(unix)]
fn doctest_memory_limit_and_compile_only_when_isolation_cannot_start() {
    let source = r#"module sample;
// ```okkama,run
// object Grow {
//     Grow();
//     double(text is string) is int { if text == "" { return 0; } self.double(text + text); }
// }
// main() is int { Grow().double("x"); }
// ```
// ```output
// ```

object Marker { Marker(); }
"#;
    let output = doctest_run("doctest-memory.ok", source);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("memory"), "{}", stderr(&output));
    let file = TempFile::new("doctest-unavailable.ok", b"module sample;\n// ```okkama,run\n// main() is int { panic \"must not execute\"; }\n// ```\n// ```output\n// ```\n\nobject Marker { Marker(); }\n");
    let unavailable = file.0.join("nonexistent");
    let out = okkama().args(["run", "-t", file.path()]).env("TMPDIR", unavailable).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("compile-only: isolation unavailable"), "{}", stdout(&out));
}

#[test]
fn doctest_multifile_module_preserves_local_imports_and_explicit_module_context() {
    let root = std::env::temp_dir().join(format!("okkama-doctest-imports-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let first = root.join("first.ok");
    let second = root.join("second.ok");
    std::fs::write(root.join("dependency.ok"), "module dependency; object Dependency { Dependency(); answer() is int { 41; } }").unwrap();
    std::fs::write(&first, "module sample;\nimport \"dependency\" as dep;\n// ```okkama\n// main() is int { dep.Dependency().answer() + Value().answer(); }\n// ```\n//\n// ```okkama\n// module independent;\n// import \"dependency\" take Dependency;\n// main() is int { Dependency().answer(); }\n// ```\n\nobject Marker { Marker(); }\n").unwrap();
    std::fs::write(&second, "module sample; object Value { Value(); answer() is int { 1; } }").unwrap();
    let output = okkama().args(["run", "-t", first.to_str().unwrap(), second.to_str().unwrap()]).output().unwrap();
    assert!(output.status.success(), "{}\n{}", stdout(&output), stderr(&output));
    assert_eq!(stdout(&output).matches("(compile)").count(), 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(unix)]
fn doctest_runtime_failure_points_to_the_original_statement() {
    let output = doctest_run("doctest-runtime-place.ok", "module sample;\n// ```okkama,run\n// main() is int {\n//     let number = 1;\n//     panic number.string();\n// }\n// ```\n// ```output\n// ```\n\nobject Marker { Marker(); }\n");
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("doctest-runtime-place.ok:5:"), "{}", stderr(&output));
}

#[test]
fn doctest_syntax_failures_keep_positions_and_filters_select_examples() {
    let source = "module sample;\n// ```okkama\n// main() is int {\n//     let = 1;\n//     0;\n// }\n// ```\n\nobject Marker { Marker(); }\n";
    let output = doctest_run("doctest-syntax.ok", source);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("doctest-syntax.ok:4:"), "{}", stderr(&output));
    let file = TempFile::new("doctest-filter.ok", b"module sample;\n// ```okkama\n// let count is int = 42;\n// ```\n\nobject Marker { Marker(); }\ntest \"inline\" { test.ok(true); }\n");
    let selected = run(&["run", "-t", "--filter", "doctest", file.path()]);
    assert!(selected.status.success(), "{}", stderr(&selected));
    assert!(stdout(&selected).contains("(compile)"));
    let inline = run(&["run", "-t", "--filter", "inline", file.path()]);
    assert!(inline.status.success(), "{}", stderr(&inline));
    assert!(!stdout(&inline).contains("(compile)"));
}

#[test]
fn doctest_compile_checks_test_and_fake_bodies_instead_of_skipping_them() {
    let output = doctest_run("doctest-test-body.ok", "module sample;\n// ```okkama\n// test \"typed\" {\n//     let number is int = \"bad\";\n//     test.is(number, 1);\n// }\n// ```\n\nobject Marker { Marker(); }\n");
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("doctest-test-body.ok:4:"), "{}", stderr(&output));
    let source = "module sample;\n// ```okkama\n// role Counter {\n//     count() is int;\n//     fake {\n//         count() is int { \"bad\"; }\n//     }\n// }\n// test \"fake\" { test.is(Counter.fake().count(), 1); }\n// ```\n\nobject Marker { Marker(); }\n";
    let fake = doctest_run("doctest-fake-body.ok", source);
    assert_eq!(fake.status.code(), Some(1));
    assert!(stderr(&fake).contains("doctest-fake-body.ok:6:"), "{}", stderr(&fake));
    let valid = source.replace("\"bad\"", "1").replace("// ```okkama\n", "// ```okkama,run\n")
        .replace("// ```\n\nobject", "// ```\n// ```output\n// ```\n\nobject");
    let compiled = doctest_run("doctest-test-profile.ok", &valid);
    assert!(compiled.status.success(), "{}", stderr(&compiled));
    assert!(stdout(&compiled).contains("compile-only: isolation unavailable"), "{}", stdout(&compiled));
}


#[test]
fn missing_docs_gate_preserves_normal_frontend_diagnostic_locations() {
    let source = TempFile::new("missing-docs-syntax.ok", b"module sample;\nmain() is int {\n    let = 1;\n}\n");
    for command in ["check", "ir", "build", "run"] {
        let mut args = vec![command, source.path()];
        let output_file = TempFile::absent("missing-docs-unused.spb");
        if command == "build" { args.extend(["-o", output_file.path()]); }
        let output = run(&args);
        assert_eq!(output.status.code(), Some(1));
        assert!(stderr(&output).contains("missing-docs-syntax.ok:3:"), "{command}: {}", stderr(&output));
    }
}


#[test]
#[cfg(not(unix))]
fn doctest_unsupported_process_monitor_falls_back_to_compile_only() {
    let output = doctest_run("doctest-platform.ok", "module sample;\n// ```okkama,run\n// main() is int { panic \"never execute without isolation\"; }\n// ```\n// ```output\n// ```\n\nobject Marker { Marker(); }\n");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("compile-only: isolation unavailable"));
}

#[test]
fn doctest_can_import_the_documented_module_as_a_caller() {
    let root = std::env::temp_dir().join(format!("okkama-doctest-self-import-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("api.ok");
    std::fs::write(&file, "module api;\n// ```okkama\n// import \"api\" as library;\n// main() is int { library.Value().answer(); }\n// ```\n\nobject Value { Value(); answer() is int { 42; } }\n").unwrap();
    let output = run(&["run", "-t", file.to_str().unwrap()]);
    assert!(output.status.success(), "{}\n{}", stdout(&output), stderr(&output));
    assert!(stdout(&output).contains("(compile)"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn man_publishing_formats_select_public_declarations_without_running_examples() {
    let file = TempFile::new("publish.ok", b"module publishing;\n// Reference [Thing.send].\n//\n// ```okkama,run\n// main() is int { panic \"must not execute\"; }\n// ```\n// ```output\n// ```\n\n// Public thing.\nobject Thing {\n// Send an int.\nsend(value is int) is void {}\n// Send a string.\nsend(value is string) is void {}\nhidden secret() is void {}\n}\nhidden object Secret {}\n");
    for format in ["man", "html", "json"] {
        let output = man(&[file.path(), "--format", format]);
        assert!(output.status.success(), "{format}: {}", stderr(&output));
        let page = stdout(&output);
        assert!(page.contains("okkama-doc:/publishing#Thing.send"), "{page}");
        assert!(!page.contains("secret()") && !page.contains("Secret"));
        let selected = man(&[file.path(), "Thing.send", "--format", format]);
        assert!(selected.status.success(), "{}", stderr(&selected));
        assert!(stdout(&selected).contains("send(value is int)") && stdout(&selected).contains("send(value is string)"));
        assert!(!stdout(&selected).contains("must not execute"));
    }
    for args in [&["--all"][..], &["--output"], &["--format", "html", "--output", "out"], &["--all", "--format", "html"], &["--all", "--format", "html", "--output", "out", file.path()], &["--all", "-k", "x"]] {
        assert_eq!(man(args).status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn man_publishing_builds_all_std_html_man_and_versioned_json() {
    let root = std::env::temp_dir().join(format!("okkama-publish-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let export = |format: &str, dest: &std::path::Path| okkama().args(["man", "--all", "--format", format, "--output", dest.to_str().unwrap()])
        .env("OKKAMA_STDLIB_DIR", std_dir()).env_remove("OKKAMA_IMPORT_DIRS").output().unwrap();
    let html_dir = root.join("html");
    let output = export("html", &html_dir);
    assert!(output.status.success(), "{}", stderr(&output));
    let index = std::fs::read_to_string(html_dir.join("index.html")).unwrap();
    assert!(index.contains("href=\"io-stdout.html\""));
    let page = std::fs::read_to_string(html_dir.join("io-stdout.html")).unwrap();
    assert!(page.contains("id=\"Stdout.writeln\""));
    let stream = std::fs::read_to_string(html_dir.join("io-stream.html")).unwrap();
    assert!(stream.contains("id=\"Writing\""));
    let module_count = std::fs::read_dir(&html_dir).unwrap().count() - 1;
    assert!(module_count >= 499, "{module_count}");
    let man_dir = root.join("man3");
    let output = export("man", &man_dir);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(std::fs::read_to_string(man_dir.join("io-stdout.3ok")).unwrap().starts_with(".TH"));
    assert_eq!(std::fs::read_dir(&man_dir).unwrap().count(), module_count);
    let json_dir = root.join("json");
    let output = export("json", &json_dir);
    assert!(output.status.success(), "{}", stderr(&output));
    let bytes = std::fs::read(json_dir.join("reference.json")).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["modules"].as_array().unwrap().len(), module_count);
    assert!(json_dir.join("schema-v1.json").is_file());
    let again = export("json", &json_dir);
    assert!(again.status.success(), "{}", stderr(&again));
    assert_eq!(std::fs::read(json_dir.join("reference.json")).unwrap(), bytes);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(unix)]
fn man_publishing_installs_only_generated_section_three_pages() {
    let root = std::env::temp_dir().join(format!("okkama-man-install-{}", std::process::id()));
    let library = root.join("std");
    let prefix = root.join("prefix with spaces");
    std::fs::create_dir_all(library.join("io")).unwrap();
    std::fs::write(library.join("io/stdout.ok"), "module io/stdout;\n// Output reference.\n\n// Public sink.\nobject Sink {}\n").unwrap();
    let script = std_dir().parent().unwrap().join("tools/install-man.sh");
    let output = Command::new("bash").arg(&script).arg(env!("CARGO_BIN_EXE_okkama")).arg(&library).arg(&prefix).output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(prefix.join("share/man/man3/io-stdout.3ok").is_file());
    assert_eq!(std::fs::read_to_string(prefix.join("lib/okkama/man-pages.txt")).unwrap(), "io-stdout.3ok\n");
    // A failed generation must not replace an installed page or its manifest.
    let original = std::fs::read(prefix.join("share/man/man3/io-stdout.3ok")).unwrap();
    std::fs::write(library.join("io/stdout.ok"), "module broken; object {").unwrap();
    let failed = Command::new("bash").arg(script).arg(env!("CARGO_BIN_EXE_okkama")).arg(&library).arg(&prefix).output().unwrap();
    assert!(!failed.status.success());
    assert_eq!(std::fs::read(prefix.join("share/man/man3/io-stdout.3ok")).unwrap(), original);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(unix)]
fn man_publishing_package_archive_installs_and_uninstalls_its_own_pages() {
    let root = std::env::temp_dir().join(format!("okkama-man-package-{}", std::process::id()));
    let target = root.join("target");
    std::fs::create_dir_all(target.join("release")).unwrap();
    for tool in ["okkama", "sparrow", "blade"] {
        std::fs::copy(env!("CARGO_BIN_EXE_okkama"), target.join("release").join(tool)).unwrap();
    }
    let repo = std_dir().parent().unwrap().to_path_buf();
    let out = root.join("dist");
    let package = Command::new("bash").arg(repo.join("tools/package.sh"))
        .args(["--no-build", "--man-pages", "--version", "test-515", "--out"]).arg(&out)
        .env("CARGO_TARGET_DIR", &target).output().unwrap();
    assert!(package.status.success(), "{}", stderr(&package));
    let archive = std::fs::read_dir(&out).unwrap().map(|entry| entry.unwrap().path()).find(|path| path.to_string_lossy().ends_with(".tar.gz")).unwrap();
    let unpack = root.join("unpack"); std::fs::create_dir_all(&unpack).unwrap();
    assert!(Command::new("tar").arg("-xzf").arg(archive).arg("-C").arg(&unpack).status().unwrap().success());
    let payload = std::fs::read_dir(unpack).unwrap().next().unwrap().unwrap().path();
    let prefix = root.join("prefix with spaces");
    let installed = Command::new("bash").arg(payload.join("install.sh")).arg("--prefix").arg(&prefix).output().unwrap();
    assert!(installed.status.success(), "{}", stderr(&installed));
    assert!(prefix.join("share/man/man3/io-stdout.3ok").is_file());
    let unrelated = prefix.join("share/man/man3/unrelated.3ok");
    std::fs::write(&unrelated, "Unrelated package").unwrap();
    let removed = Command::new("bash").arg(prefix.join("lib/okkama/uninstall.sh")).output().unwrap();
    assert!(removed.status.success(), "{}", stderr(&removed));
    assert!(!prefix.join("share/man/man3/io-stdout.3ok").exists());
    assert!(unrelated.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn man_publishing_keeps_doctest_validation_and_missing_docs_lint_separate() {
    let file = TempFile::new("publish-invalid-example.ok", b"module sample;\n// ```okkama\n// let value is int = \"bad\";\n// ```\n\nobject Undocumented {}\n");
    for format in ["man", "html", "json"] {
        let output = man(&[file.path(), "--format", format]);
        assert!(output.status.success(), "{format}: {}", stderr(&output));
        assert!(stdout(&output).contains("let value is int"));
        assert!(!stderr(&output).contains("missing-docs"));
    }
    let checked = run(&["run", "-t", file.path()]);
    assert_eq!(checked.status.code(), Some(1));
    assert!(stderr(&checked).contains("compilation failed"));
    let lint = run(&["lint", file.path()]);
    assert_eq!(lint.status.code(), Some(1));
    assert!(stdout(&lint).contains("warning[missing-docs]"));
}

/// #1009: a test report shows a byte as 0..255 and a char as the character,
/// as the language prints them, not as a signed Java byte and a code point.
#[test]
fn test_reports_show_bytes_and_chars_as_the_language_prints_them() {
    let source = TempFile::new(
        "shown-values.ok",
        "module app v1.0.0;\nmain() is int { 0; }\ntest \"bytes\" { test.is(byte(200), byte(201)); }\n\
         test \"chars\" { test.is('Ж', 'Ы'); }\ntest \"arrays\" { test.is(@byte(200, 7), @byte(201, 7)); }\n"
            .as_bytes(),
    );
    let output = run(&["run", "-t", source.path()]);
    let text = format!("{}{}", stdout(&output), stderr(&output));
    assert!(text.contains("expected 201, got 200"), "{text}");
    assert!(text.contains("expected 'Ы', got 'Ж'"), "{text}");
    assert!(text.contains("[200, 7]"), "{text}");
    assert!(!text.contains("-56"), "{text}");
}

/// #1017: every command `--help` lists has a `help` page, the pages name
/// every IR pass and lint rule and the exit code 4, `check` refuses a
/// `--format` it would ignore, and the dead worker key `--stdlib` is gone.
#[test]
fn help_pages_are_complete_and_check_refuses_format() {
    for command in ["check", "run", "fmt", "lint", "env", "lsp", "repl", "exec", "man", "ir"] {
        let output = run(&["help", command]);
        assert_eq!(output.status.code(), Some(0), "help {command}: {}", stderr(&output));
        assert!(!stdout(&output).is_empty(), "help {command}");
    }
    let main = stdout(&run(&["--help"]));
    assert!(main.contains("okkama man [MODULE [NAME | NAME.ACTION] | -k QUERY | --all] [--format FORMAT]"), "{main}");
    let ir = stdout(&run(&["help", "ir"]));
    for pass in ["statics", "regions", "tail-calls", "fold", "hoist", "undef", "jumps"] {
        assert!(ir.contains(pass), "{pass}: {ir}");
    }
    let lint = stdout(&run(&["help", "lint"]));
    for rule in ["hidden-error", "non-exhaustive-match", "unresolved-doc-link"] {
        assert!(lint.contains(rule), "{rule}: {lint}");
    }
    let source = TempFile::new("check-format.ok", HELLO.as_bytes());
    let output = run(&["check", "--format", "json", source.path()]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(stderr(&output).contains("check has no --format option"), "{}", stderr(&output));
    let output = run(&["run", "--stdlib", "/tmp", source.path()]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
}

/// #1017: standard input that cannot be read as text is an input/output
/// error for `fmt -` (3), as an unreadable file is; it was 1.
#[test]
fn fmt_unreadable_standard_input_is_an_io_error() {
    let output = run_stdin(&["fmt", "-"], b"module app v1.0.0;\n\xff\n");
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert!(stderr(&output).contains("cannot read standard input"), "{}", stderr(&output));
    let file = TempFile::new("fmt-unreadable.ok", b"module app v1.0.0;\n\xff\n");
    assert_eq!(run(&["fmt", file.path()]).status.code(), Some(3));
}
