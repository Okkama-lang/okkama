//! #1113: a pure int function also gets a small-int version (no tag checks, no stack maps for its ints); an int
//! leaving the tagged-small range (−2^62..2^62−1), mid-loop or as an argument, restarts the call in the general
//! version, so results, failures and their traces are the general version's. The program runs with the small
//! versions on and off (`SPARROW_CORE_SMALL_INTS=0`), and without the IR's and Sparrow's inlining, so every
//! method is a compiled function of its own.
mod tree;

const PROGRAM: &str = r#"module app v1.0.0;
import "io/stdout" take Stdout;
object M {
    M();
    pow(n is int, acc is int) is int { if n == 0 { acc; } else { self.pow(n - 1, acc * 3); } }
    sum(n is int, acc is int) is int { if n == 0 { acc; } else { self.sum(n - 1, acc + n); } }
    near(x is int) is int { x + 1; }
    neg(x is int) is int { 0 - x - 1; }
    rem(a is int, b is int) is int { a % b; }
    less(a is int, b is int) is bool { a < b; }
}
main() is int {
    let m = M();
    let out = Stdout();
    out.writeln(m.pow(100, 1).string());
    out.writeln(m.pow(10, 1).string());
    out.writeln(m.sum(1000, 4611686018427387000).string());
    out.writeln(m.near(4611686018427387903).string());
    out.writeln(m.near(-4611686018427387905).string());
    out.writeln(m.neg(4611686018427387903).string());
    out.writeln(m.rem(7, -1).string());
    out.writeln(m.rem(-7, 3).string());
    out.writeln(m.rem(100000000000000000000000, 7).string());
    out.writeln(m.less(100000000000000000000000, 7).string());
    out.writeln(m.less(-100000000000000000000000, 7).string());
    out.writeln(m.rem(5, 0).string());
    0;
}
"#;

/// The exact results: 3^100 overflows mid-loop, the sum crosses 2^62 near its end, 2^62 − 1 + 1 and −2^62 − 1 + 1
/// cross the boundary at an argument, −2^62 is still small, `% −1` and a big dividend take the exact paths, a big
/// argument compares, and `% 0` fails with the general version's trace.
const EXPECTED: &str = "515377520732011331036461129765621272702107522001
59049
4611686018427887500
4611686018427387904
-4611686018427387904
-4611686018427387904
0
-1
5
false
true
";

fn run(small: bool) -> (Option<i32>, String, String) {
    let dir = std::env::temp_dir().join(format!("okkama-small-ints-{}-{small}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("edge.ok");
    std::fs::write(&file, PROGRAM).unwrap();
    let output = tree::okkama()
        .current_dir(&dir)
        .args(["run", "edge.ok"])
        .env("OKKAMA_IR_PASSES", "statics,regions,tail-calls,fold,hoist,dce,undef,jumps")
        // Nor does Sparrow inline the small callees into main (#926): each runs as its own compiled code.
        .env("SPARROW_CORE_NO_INLINE", "*")
        .env("SPARROW_CORE_SMALL_INTS", if small { "1" } else { "0" })
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (output.status.code(), String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned())
}

#[test]
fn small_int_versions_restart_in_the_general_version_past_the_small_range() {
    let small = run(true);
    assert_eq!(small.1, EXPECTED);
    assert_eq!(small.0, Some(1), "{}", small.2);
    assert!(small.2.starts_with("RuntimeError: Modulo by zero"), "{}", small.2);
    assert!(small.2.contains("  at M.rem (edge.ok:9:38)\n  at main (edge.ok:26:17)"), "{}", small.2);
    assert_eq!(run(false), small);
}
