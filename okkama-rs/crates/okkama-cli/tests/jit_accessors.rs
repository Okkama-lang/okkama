//! #541: a field accessor's call is compiled as its load, and the program
//! cannot tell: same output, and the same step count (an accessor's entry is
//! still a step), as with Sparrow's fast paths off (`SPARROW_CORE_PLAIN=*`).
mod tree;


const PROGRAM: &str = "module app v1.0.0;
import \"io/stdout\" take Stdout;
object P {
    has x is int;
    has y is int;
    P(:x, :y);
    x() is int { x; }
    y() is int { y; }
}
object Summer {
    Summer();
    sum(p is P, i is int, acc is int) is int {
        if i == 0 { return acc; }
        self.sum(p, i - 1, acc + p.x() * 10 + p.y());
    }
}
main() is int {
    Stdout().writeln(Summer().sum(P(x: 4, y: 2), 50, 0).string());
    0;
}
";

fn run(plain: bool, steps: Option<u64>) -> (bool, String) {
    let dir = std::env::temp_dir().join(format!("okkama-accessors-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("main.ok");
    std::fs::write(&file, PROGRAM).unwrap();
    let mut command = tree::okkama();
    command.arg("run");
    if let Some(steps) = steps {
        command.args(["--max-steps", &steps.to_string()]);
    }
    command.arg(&file);
    if plain {
        command.env("SPARROW_CORE_PLAIN", "*");
    } else {
        command.env_remove("SPARROW_CORE_PLAIN");
    }
    let output = command.output().unwrap();
    (output.status.success(), String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The fewest steps the program completes in.
fn fewest_steps(plain: bool) -> u64 {
    let (mut low, mut high) = (1u64, 1u64 << 20);
    assert!(run(plain, Some(high)).0, "completes within {high} steps");
    while low < high {
        let middle = low + (high - low) / 2;
        if run(plain, Some(middle)).0 { high = middle } else { low = middle + 1 }
    }
    low
}

#[test]
fn inlined_accessors_keep_output_and_steps() {
    let inlined = run(false, None);
    assert_eq!(inlined, (true, "2100\n".to_string()));
    assert_eq!(run(true, None), inlined);
    let steps = fewest_steps(false);
    assert_eq!(steps, fewest_steps(true));
    // Two accessor entries per round are among them.
    assert!(steps > 150, "{steps}");
}
