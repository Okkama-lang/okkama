//! #1106: the inline byte bit operations, `int >>>`, `abs`, `sign`, `sqrt` and `bits` actions against the shared
//! path. The oracle is `jit.plain`, where every one of these instructions runs through `rt_exec`, on the same
//! inputs: results and failure texts must be equal, and the inline build must not reach `rt_exec` on the inputs
//! its fast path covers.
use super::*;
use crate::program::Loaded;
use crate::runtime::Runtime;
use num_bigint::BigInt;

#[derive(Clone, Debug)]
enum Arg {
    Int(BigInt),
    Byte(u8),
    Float(f64),
}

fn runtime(source: &str, plain: bool) -> Box<Runtime> {
    let program = okkama_sparrow::assemble(source).unwrap_or_else(|e| panic!("{e}\n{source}"));
    program.verify().unwrap();
    let mut runtime = Box::new(Runtime::new(Loaded::new(program)).unwrap());
    runtime.jit.plain = Chosen { all: plain, ..Chosen::default() };
    runtime.stats = Some(Box::default());
    runtime
}

/// One function: parameters of `params` (int, byte or float), `line` defining the last register, its result.
fn source(params: &[&str], result: &str, line: &str) -> String {
    let ty = |name: &str| match name {
        "int" => "t0",
        "byte" => "t1",
        _ => "t2",
    };
    let mut registers: Vec<&str> = params.iter().map(|p| ty(p)).collect();
    registers.push(ty(result));
    let names: Vec<String> = (0..params.len()).map(|i| format!("r{i}")).collect();
    let dst = params.len();
    format!(
        "sparrow 18\nmodule \"scalar-1106\" \"1.0.0\"\ntype t0 int\ntype t1 byte\ntype t2 float\nfunction f0 \"main\" {{\n registers [{}]\n params [{}]\n result {}\n 0: {}\n 1: ret r{dst}\n}}\n",
        registers.join(", "),
        names.join(", "),
        ty(result),
        line.replace("DST", &format!("r{dst}")),
    )
}

fn word(runtime: &mut Box<Runtime>, arg: &Arg) -> u64 {
    match arg {
        Arg::Int(n) => runtime.heap.big(n.clone()),
        Arg::Byte(b) => u64::from(*b),
        Arg::Float(x) => x.to_bits(),
    }
}

fn run(runtime: &mut Box<Runtime>, args: &[Arg], result: &str) -> Result<String, String> {
    let words: Vec<u64> = args.iter().map(|arg| word(runtime, arg)).collect();
    match runtime.run_function_with(0, &words, 1 << 24) {
        Ok(value) => Ok(match result {
            "int" => runtime.heap.integer(value).to_string(),
            "byte" => format!("byte {value}"),
            _ => format!("float {:#x}", value),
        }),
        Err(error) => Err(error.0),
    }
}

fn event(runtime: &Runtime, name: &str) -> u64 {
    runtime.stats.as_ref().unwrap().get(name).copied().unwrap_or(0)
}

/// Every case of `cases` gives the same outcome inline and on the shared path; `fast` cases never reach rt_exec.
fn check(params: &[&str], result: &str, line: &str, mnemonic: &str, cases: &[Vec<Arg>], fast: &dyn Fn(&[Arg]) -> bool) {
    let text = source(params, result, line);
    let (mut inline, mut plain) = (runtime(&text, false), runtime(&text, true));
    let mut covered = 0;
    for args in cases {
        let before = event(&inline, &format!("exec {mnemonic}"));
        let expected = run(&mut plain, args, result);
        let actual = run(&mut inline, args, result);
        assert_eq!(actual, expected, "{line} {args:?}");
        if fast(args) {
            covered += 1;
            assert_eq!(event(&inline, &format!("exec {mnemonic}")), before, "{line} {args:?} reached rt_exec");
        }
    }
    assert!(covered > 0, "{line}: no case on the fast path");
    assert!(event(&plain, &format!("exec {mnemonic}")) >= cases.len() as u64, "{line}: the oracle ran inline");
}

fn int(n: impl Into<BigInt>) -> Arg {
    Arg::Int(n.into())
}

fn ints() -> Vec<Arg> {
    let small_max = BigInt::from(word::SMALL_MAX);
    let small_min = BigInt::from(word::SMALL_MIN);
    vec![
        int(0), int(1), int(-1), int(2), int(-2), int(7), int(-8), int(63), int(64), int(77), int(-77),
        int(1i64 << 40), int(-(1i64 << 40)), int(1i64 << 61), int(-(1i64 << 61)),
        Arg::Int(small_max.clone()), Arg::Int(small_max.clone() - 1), Arg::Int(small_max + 1),
        Arg::Int(small_min.clone()), Arg::Int(small_min.clone() + 1), Arg::Int(small_min - 1),
        int(i64::MAX), int(i64::MIN), Arg::Int(BigInt::from(1) << 64usize), Arg::Int(-(BigInt::from(1) << 70usize)),
    ]
}

fn counts() -> Vec<Arg> {
    [-1i64, 0, 1, 2, 3, 7, 8, 9, 61, 62, 63, 64, 65].into_iter().map(int).chain([Arg::Int(BigInt::from(1) << 64usize)]).collect()
}

fn bytes() -> Vec<Arg> {
    [0u8, 1, 2, 3, 0x0F, 0x7F, 0x80, 0xAA, 0xFE, 0xFF].into_iter().map(Arg::Byte).collect()
}

fn floats() -> Vec<Arg> {
    [0.0, -0.0, 1.0, 2.0, 0.25, 3.0, -1.0, -0.5, f64::MAX, f64::MIN_POSITIVE, 5e-324, -5e-324, 1e300, -f64::MAX]
        .into_iter()
        .map(Arg::Float)
        .collect()
}

fn small(arg: &Arg) -> Option<i64> {
    match arg {
        Arg::Int(n) => i64::try_from(n).ok().filter(|n| word::small(*n).is_some()),
        _ => None,
    }
}

fn pairs(left: &[Arg], right: &[Arg]) -> Vec<Vec<Arg>> {
    left.iter().flat_map(|l| right.iter().map(move |r| vec![l.clone(), r.clone()])).collect()
}

#[test]
fn byte_bit_operations_match_the_shared_path() {
    crate::on_vm_thread(|| {
        let cases = pairs(&bytes(), &bytes());
        for op in ["and", "or", "xor"] {
            check(&["byte", "byte"], "byte", &format!("byte.{op} DST, r0, r1"), &format!("byte.{op}"), &cases, &|_| true);
        }
        let singles: Vec<Vec<Arg>> = bytes().into_iter().map(|b| vec![b]).collect();
        check(&["byte"], "byte", "byte.not DST, r0", "byte.not", &singles, &|_| true);
        let shifts = pairs(&bytes(), &counts());
        let in_range = |args: &[Arg]| small(&args[1]).is_some_and(|n| (0..8).contains(&n));
        for op in ["shl", "shr", "ushr"] {
            check(&["byte", "int"], "byte", &format!("byte.{op} DST, r0, r1"), &format!("byte.{op}"), &shifts, &in_range);
        }
        Ok(())
    }).unwrap();
}

#[test]
fn int_unsigned_shift_abs_and_sign_match_the_shared_path() {
    crate::on_vm_thread(|| {
        let fits = |n: i128| word::small(n as i64).is_some() && n == i128::from(n as i64);
        let shifts = pairs(&ints(), &counts());
        check(&["int", "int"], "int", "int.ushr DST, r0, r1", "int.ushr", &shifts, &|args| {
            match (small(&args[0]), small(&args[1])) {
                (Some(v), Some(n)) if (0..64).contains(&n) => fits(i128::from(((v as u64) >> n) as i64)),
                _ => false,
            }
        });
        let singles: Vec<Vec<Arg>> = ints().into_iter().map(|n| vec![n]).collect();
        check(&["int"], "int", "int.abs DST, r0", "int.abs", &singles, &|args| {
            small(&args[0]).is_some_and(|v| v != word::SMALL_MIN)
        });
        check(&["int"], "int", "sign DST, r0", "sign", &singles, &|args| small(&args[0]).is_some());
        let singles: Vec<Vec<Arg>> = floats().into_iter().map(|x| vec![x]).collect();
        check(&["float"], "int", "sign DST, r0", "sign", &singles, &|_| true);
        Ok(())
    }).unwrap();
}

#[test]
fn float_sqrt_matches_the_shared_path_and_fails_below_zero() {
    crate::on_vm_thread(|| {
        let singles: Vec<Vec<Arg>> = floats().into_iter().map(|x| vec![x]).collect();
        check(&["float"], "float", "float.sqrt DST, r0", "float.sqrt", &singles, &|args| {
            matches!(args[0], Arg::Float(x) if x >= 0.0)
        });
        let text = source(&["float"], "float", "float.sqrt DST, r0");
        let mut inline = runtime(&text, false);
        let error = run(&mut inline, &[Arg::Float(-2.0)], "float").unwrap_err();
        assert!(error.contains("Can't take sqrt of -2"), "{error}");
        assert_eq!(run(&mut inline, &[Arg::Float(-0.0)], "float"), Ok(format!("float {:#x}", (-0.0f64).to_bits())));
        Ok(())
    }).unwrap();
}

#[test]
fn bit_actions_match_the_shared_path() {
    crate::on_vm_thread(|| {
        for byte in [false, true] {
            let (ty, values) = if byte { ("byte", bytes()) } else { ("int", ints()) };
            let operand_fast = |arg: &Arg| byte || small(arg).is_some();
            for op in ["bit_count", "leading_zeros", "trailing_zeros"] {
                let singles: Vec<Vec<Arg>> = values.iter().cloned().map(|v| vec![v]).collect();
                check(&[ty], "int", &format!("bits {op}, {byte}, DST, r0, _"), "bits", &singles, &|args| operand_fast(&args[0]));
            }
            let width = if byte { 8 } else { 64 };
            for op in ["rotl", "rotr"] {
                let cases = pairs(&values, &counts());
                check(&[ty, "int"], ty, &format!("bits {op}, {byte}, DST, r0, r1"), "bits", &cases, &|args| {
                    // An int rotation's result may leave tagged-small; then the shared path runs, which the
                    // equality check covers. Only bytes are always fast with a valid count.
                    let valid = small(&args[1]).is_some_and(|n| (0..width).contains(&n));
                    valid && (byte || matches!(small(&args[0]), Some(0 | -1)))
                });
            }
            for op in ["reverse_bits", "swap_bytes"] {
                let singles: Vec<Vec<Arg>> = values.iter().cloned().map(|v| vec![v]).collect();
                check(&[ty], ty, &format!("bits {op}, {byte}, DST, r0, _"), "bits", &singles, &|args| {
                    byte || (op == "swap_bytes" && matches!(small(&args[0]), Some(0)))
                        || (op == "reverse_bits" && matches!(small(&args[0]), Some(0 | -1)))
                });
            }
        }
        Ok(())
    }).unwrap();
}
