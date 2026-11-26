//! #926/#927: a small static callee is compiled into its caller, and the program cannot tell: the same output,
//! the same failure and trace, the same handler, and the same step count (an inlined entry is still a step), as
//! with inlining off (`SPARROW_CORE_NO_INLINE=*`).
mod tree;


const PROGRAM: &str = "module app v1.0.0;
import \"io/stdout\" take Stdout;
enum Stop is Error = (now);
object Calc {
    Calc();
    // Recursion inlines one level into itself.
    fib(n is int) is int { if n < 2 { n; } else { self.fib(n - 1) + self.fib(n - 2); } }
    // A computing callee that may fail.
    pick(items is @int, i is int) is int { items[i] * 2; }
    fails(n is int) is int { if n > 2 { throw Stop.now; } n; }
    guarded(n is int) is int { try { self.fails(n); } catch e is Stop { -1; }; }
}
main() is int {
    let calc = Calc();
    Stdout().writeln(calc.fib(15).string());
    Stdout().writeln(calc.pick([1, 2, 3], 2).string());
    Stdout().writeln(calc.guarded(1).string() + \" \" + calc.guarded(5).string());
    Stdout().writeln(calc.pick([1, 2, 3], 7).string());
    0;
}
";

fn run(inline: bool, steps: Option<u64>) -> (bool, String, String) {
    let dir = std::env::temp_dir().join(format!("okkama-inline-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("main.ok");
    std::fs::write(&file, PROGRAM).unwrap();
    let mut command = tree::okkama();
    command.arg("run");
    if let Some(steps) = steps {
        command.args(["--max-steps", &steps.to_string()]);
    }
    command.arg(&file);
    if inline {
        command.env_remove("SPARROW_CORE_NO_INLINE");
    } else {
        command.env("SPARROW_CORE_NO_INLINE", "*");
    }
    command.env_remove("SPARROW_CORE_PLAIN");
    let output = command.output().unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// The fewest steps after which the program reaches its final failure (the out-of-range `pick`), not a budget
/// stop.
fn fewest_steps(inline: bool) -> u64 {
    let reaches = |steps| run(inline, Some(steps)).2.contains("out of bounds");
    let (mut low, mut high) = (1u64, 1u64 << 20);
    assert!(reaches(high), "reaches the failure within {high} steps");
    while low < high {
        let middle = low + (high - low) / 2;
        if reaches(middle) { high = middle } else { low = middle + 1 }
    }
    low
}

#[test]
fn inlined_callees_keep_output_failures_and_steps() {
    let inlined = run(true, None);
    assert!(!inlined.0, "the last pick fails");
    assert_eq!(inlined.1, "610\n6\n1 -1\n");
    assert!(inlined.2.contains("at Calc.pick"), "{}", inlined.2);
    assert_eq!(run(false, None), inlined, "inlining off: same output, failure and trace");
    let steps = fewest_steps(true);
    assert_eq!(steps, fewest_steps(false));
    assert!(steps > 1000, "{steps}");
}
