//! #588: `okkama repl` on Sparrow, driven by stdin scripts (no terminal: no prompt, no line editor), and one
//! pseudo-terminal test for the line editor's history.
use std::io::Write;
use std::process::{Command, Stdio};

fn std_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../std")
}

/// Run `okkama repl` on `lines`; (stdout, stderr).
fn repl(lines: &[&str]) -> (String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_okkama"))
        .arg("repl")
        .env("OKKAMA_STDLIB_DIR", std_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut script = lines.join("\n");
    script.push('\n');
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    (String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned())
}

const RES: &str = "object Res { has name is string; Res(:name); label() is string { name; } \
                   destroy { Stdout().writeln(\"destroy \" + name); } }";

#[test]
fn values_declarations_and_commands() {
    let (out, err) = repl(&[
        "let x = 40",
        "x + 2",
        "object Point { has x, y is int; Point(:x, :y); sum() is int { x + y; } }",
        "let p = Point(x: 1, y: 2)",
        "p.sum() + x",
        ":vars",
        "let x = \"text\"",
        "x",
        "let f = (n is int) is int { n * 2; }",
        ":vars",
        ":decls",
        ":nope",
    ]);
    assert_eq!(
        out,
        "42 (int)\n43 (int)\nx: int\np: Point\n\"text\" (string, 4 chars)\np: Point\nx: string\n\
         object Point { has x, y is int; Point(:x, :y); sum() is int { x + y; } }\n"
    );
    assert!(err.contains("note: `f` ((int) is int) is not kept between inputs"), "{err}");
    assert!(err.contains("unknown command `:nope`"), "{err}");
}

#[test]
fn errors_leave_the_session_usable() {
    let (out, err) = repl(&["let a = 1", "let b = a +", ";", "let c = [1][5]", "a + 1", "nope()", "a"]);
    assert_eq!(out, "2 (int)\n1 (int)\n");
    assert!(err.contains("Array index out of bounds"), "{err}");
    assert!(err.contains("error"), "{err}");
}

/// The destroy-trace oracle: every resource is destroyed once, in reverse creation order where a scope closes.
#[test]
fn resources_live_as_long_as_their_variables() {
    let (out, err) = repl(&[
        "import \"io/stdout\" take Stdout;",
        RES,
        "let a = Res(name: \"A\")",
        "let b = Res(name: \"B\")",
        "let alias = b",
        "Stdout().writeln(\"kept \" + a.label() + b.label() + alias.label())",
        "let a = Res(name: \"A2\")",
        "Stdout().writeln(\"after rebinding\")",
        "let failing = Res(name: \"F\"); [1][5]",
        "Stdout().writeln(\"after failure\")",
        ":reset",
        "Stdout().writeln(\"fresh\")",
    ]);
    assert_eq!(
        out,
        "kept ABB\ndestroy A\nafter rebinding\ndestroy F\nafter failure\ndestroy A2\ndestroy B\n",
        "{err}"
    );
    assert!(err.contains("Stdout"), "after :reset the import is gone: {err}");
}

#[test]
fn async_cells_keep_resources_and_exit_destroys_them() {
    let (out, err) = repl(&[
        "import \"io/stdout\" take Stdout;",
        RES,
        "object Work { Work(); async twice(n is int) is int { n * 2; } async make(name is string) is Res { Res(name: name); } }",
        "let r = Res(name: \"R\")",
        "let v = await Work().twice(21)",
        "let m = await Work().make(\"M\")",
        "v + 0",
        "r.label() + m.label()",
    ]);
    assert_eq!(out, "42 (int)\n\"RM\" (string, 2 chars)\ndestroy M\ndestroy R\n", "{err}");
}

#[test]
fn a_declaration_that_changes_earlier_ownership_is_rejected() {
    let (out, err) = repl(&[
        "role Shape { area() is int; }",
        "object Square does Shape { Square(); area() is int { 4; } }",
        "object Box { has shape is Shape; Box(:shape); }",
        "let b = Box(shape: Square())",
        "object Owning does Shape { Owning(); area() is int { 1; } destroy { } }",
        "b.shape.area()",
    ]);
    assert!(err.contains(":reset starts a fresh session"), "{err}");
    assert_eq!(out, "", "{err}");
}

/// On a terminal the line editor recalls the previous input with the up arrow.
#[test]
fn the_up_arrow_recalls_the_previous_input() {
    if Command::new("python3").arg("--version").output().is_err() {
        return;
    }
    let script = r#"
import os, pty, sys, time, select
pid, fd = pty.fork()
if pid == 0:
    os.environ["HOME"] = sys.argv[2]
    os.execv(sys.argv[1], [sys.argv[1], "repl"])
def read_until(text, limit=20):
    out, end = b"", time.time() + limit
    while time.time() < end and text not in out:
        ready, _, _ = select.select([fd], [], [], 0.2)
        if ready:
            try:
                out += os.read(fd, 4096)
            except OSError:
                break
    return out
read_until(b"okkama> ")
os.write(fd, b"20 + 1\r")
first = read_until(b"21 (int)")
os.write(fd, b"\x1b[A\r")
second = read_until(b"21 (int)")
os.write(fd, b":quit\r")
time.sleep(0.5)
print("ok" if b"21 (int)" in first and b"21 (int)" in second else "fail", repr(first + second))
"#;
    let home = std::env::temp_dir().join(format!("okkama-repl-home-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let output = Command::new("python3")
        .args(["-c", script, env!("CARGO_BIN_EXE_okkama"), home.to_str().unwrap()])
        .env("OKKAMA_STDLIB_DIR", std_dir())
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&output.stdout);
    assert!(report.starts_with("ok"), "{report}");
}
