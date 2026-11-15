//! SPB 17 int, byte, float, rat and str family operations on Tomato's representations (#675): edge
//! values and failure messages as Tomato gives them, and the helper's failure and budget channels.
use super::machine::{rt_machine_scalar, scalar};
use crate::runtime::{Ctx, Runtime, EXHAUSTED, FAILED, OK};
use crate::word::{Repr, Word};
use okkama_sparrow::spb17::{self as s, DataOp as D};

fn runtime() -> Box<Runtime> {
    let mut runtime = Runtime::new_champ(s::Program::default(), vec![]).unwrap();
    runtime.heap.set_stress(true);
    runtime
}

fn int(runtime: &mut Runtime, text: &str) -> Word {
    runtime.heap.big(text.parse().unwrap())
}

fn int_text(runtime: &Runtime, word: Word) -> String {
    runtime.big_value(word).to_string()
}

fn float(value: f64) -> Word {
    value.to_bits()
}

fn rat(runtime: &mut Runtime, numerator: i64, denominator: i64) -> Word {
    let part = okkama_runtime_core::IntValue::from_i64;
    let value = okkama_runtime_core::rat::Rat::new(&part(numerator), &part(denominator)).unwrap();
    runtime.rational_new(value)
}

fn rat_text(runtime: &Runtime, word: Word) -> String {
    let value = runtime.rational(word);
    format!("{}/{}", value.numerator(), value.denominator())
}

fn text(runtime: &mut Runtime, value: &str) -> Word {
    runtime.text_new(value)
}

/// `op` on up to three operands; unused ones are zero, as machine code passes them.
fn run(runtime: &mut Runtime, op: D, operands: &[Word]) -> Result<Word, String> {
    let operand = |index: usize| operands.get(index).copied().unwrap_or(0);
    scalar(runtime, op, operand(0), operand(1), operand(2))
}

fn ints(runtime: &mut Runtime, op: D, left: &str, right: &str) -> Result<String, String> {
    let (left, right) = (int(runtime, left), int(runtime, right));
    run(runtime, op, &[left, right]).map(|word| int_text(runtime, word))
}

const COUNT_63: &str = "Shift count must be in 0..63";
const COUNT_7: &str = "Shift count must be in 0..7";
const OUTSIDE: &str = "Bitwise operand is outside signed-64 range";

#[test]
fn int_shifts_and_bits_keep_tomato_boundaries() {
    let mut rt = runtime();
    let rt = &mut *rt;
    assert_eq!(ints(rt, D::IntShl, "1", "63").unwrap(), i64::MIN.to_string());
    assert_eq!(ints(rt, D::IntShl, "1", "64").unwrap_err(), COUNT_63);
    assert_eq!(ints(rt, D::IntShl, "1", "-1").unwrap_err(), COUNT_63);
    assert_eq!(ints(rt, D::IntShr, "-8", "1").unwrap(), "-4");
    assert_eq!(ints(rt, D::IntUshr, "-1", "60").unwrap(), "15");
    assert_eq!(ints(rt, D::IntShr, "1", "18446744073709551616").unwrap_err(), COUNT_63);
    assert_eq!(ints(rt, D::IntAnd, "12", "10").unwrap(), "8");
    assert_eq!(ints(rt, D::IntOr, "12", "10").unwrap(), "14");
    assert_eq!(ints(rt, D::IntXor, "12", "10").unwrap(), "6");
    assert_eq!(ints(rt, D::IntAnd, "9223372036854775808", "1").unwrap_err(), OUTSIDE);
    assert_eq!(ints(rt, D::IntOr, "1", "-9223372036854775809").unwrap_err(), OUTSIDE);
    assert_eq!(ints(rt, D::IntRotl, "1", "63").unwrap(), i64::MIN.to_string());
    assert_eq!(ints(rt, D::IntRotr, "1", "64").unwrap_err(), COUNT_63);
    let unary = |rt: &mut Runtime, op, value: &str| {
        let value = int(rt, value);
        run(rt, op, &[value]).map(|word| int_text(rt, word))
    };
    assert_eq!(unary(rt, D::IntNot, "5").unwrap(), "-6");
    assert_eq!(unary(rt, D::IntNot, "9223372036854775808").unwrap_err(), OUTSIDE);
    assert_eq!(unary(rt, D::IntBitCount, "-1").unwrap(), "64");
    assert_eq!(unary(rt, D::IntLeadingZeros, "1").unwrap(), "63");
    assert_eq!(unary(rt, D::IntTrailingZeros, "0").unwrap(), "64");
    assert_eq!(unary(rt, D::IntReverseBits, "1").unwrap(), i64::MIN.to_string());
    assert_eq!(unary(rt, D::IntSwapBytes, "1").unwrap(), "72057594037927936");
    assert_eq!(unary(rt, D::IntBitCount, "-9223372036854775809").unwrap_err(), OUTSIDE);
}

#[test]
fn int_results_are_exact_beyond_64_bits() {
    let mut rt = runtime();
    let rt = &mut *rt;
    let unary = |rt: &mut Runtime, op, value: &str| {
        let value = int(rt, value);
        let word = run(rt, op, &[value]).unwrap();
        int_text(rt, word)
    };
    assert_eq!(unary(rt, D::IntAbs, "-9223372036854775808"), "9223372036854775808");
    assert_eq!(unary(rt, D::IntAbs, "-1267650600228229401496703205376"), "1267650600228229401496703205376");
    assert_eq!(unary(rt, D::IntSign, "-1267650600228229401496703205376"), "-1");
    assert_eq!(unary(rt, D::IntSign, "0"), "0");
    assert_eq!(unary(rt, D::IntSign, "9223372036854775808"), "1");
}

#[test]
fn byte_operations_count_in_int_and_refuse_counts_past_seven() {
    let mut rt = runtime();
    let rt = &mut *rt;
    let byte = |rt: &mut Runtime, op, value: u8, count: i64| {
        let count = rt.heap.int(count);
        run(rt, op, &[Word::from(value), count])
    };
    assert_eq!(byte(rt, D::ByteShl, 1, 7).unwrap(), 128);
    assert_eq!(byte(rt, D::ByteShl, 0x81, 1).unwrap(), 2);
    assert_eq!(byte(rt, D::ByteShl, 1, 8).unwrap_err(), COUNT_7);
    assert_eq!(byte(rt, D::ByteShr, 0x80, -1).unwrap_err(), COUNT_7);
    assert_eq!(byte(rt, D::ByteShr, 0x80, 7).unwrap(), 1);
    assert_eq!(byte(rt, D::ByteRotl, 0x81, 1).unwrap(), 3);
    assert_eq!(byte(rt, D::ByteRotr, 0x81, 8).unwrap_err(), COUNT_7);
    // byte.ushr counts in byte (OIR-2); a byte has no sign, so it shifts as byte.shr.
    assert_eq!(run(rt, D::ByteUshr, &[0x80, 7]).unwrap(), 1);
    assert_eq!(run(rt, D::ByteUshr, &[0x80, 8]).unwrap_err(), COUNT_7);
    assert_eq!(run(rt, D::ByteUshr, &[0x80, 255]).unwrap_err(), COUNT_7);
    assert_eq!(run(rt, D::ByteNot, &[0x0f]).unwrap(), 0xf0);
    assert_eq!(run(rt, D::ByteAnd, &[0x0c, 0x0a]).unwrap(), 0x08);
    assert_eq!(run(rt, D::ByteOr, &[0x0c, 0x0a]).unwrap(), 0x0e);
    assert_eq!(run(rt, D::ByteXor, &[0x0c, 0x0a]).unwrap(), 0x06);
    assert_eq!(run(rt, D::ByteReverseBits, &[1]).unwrap(), 0x80);
    assert_eq!(run(rt, D::ByteSwapBytes, &[0x12]).unwrap(), 0x12);
    let count = |rt: &mut Runtime, op, value: Word| {
        let word = run(rt, op, &[value]).unwrap();
        int_text(rt, word)
    };
    assert_eq!(count(rt, D::ByteBitCount, 0xff), "8");
    assert_eq!(count(rt, D::ByteLeadingZeros, 1), "7");
    assert_eq!(count(rt, D::ByteTrailingZeros, 0), "8");
}

#[test]
fn float_results_must_be_finite_and_keep_signed_zero() {
    let mut rt = runtime();
    let rt = &mut *rt;
    let binary = |rt: &mut Runtime, op, left: f64, right: f64| run(rt, op, &[float(left), float(right)]);
    let finite = "Float value must contain a finite number";
    assert_eq!(binary(rt, D::FloatAdd, 1e308, 1e308).unwrap_err(), finite);
    assert_eq!(binary(rt, D::FloatMul, -1e308, 10.0).unwrap_err(), finite);
    assert_eq!(binary(rt, D::FloatPow, 10.0, 400.0).unwrap_err(), finite);
    assert_eq!(binary(rt, D::FloatDiv, 1.0, 0.0).unwrap_err(), "Division by zero");
    assert_eq!(binary(rt, D::FloatDiv, 0.0, -0.0).unwrap_err(), "Division by zero");
    assert_eq!(binary(rt, D::FloatMod, 1.0, 0.0).unwrap_err(), "Modulo by zero");
    assert_eq!(binary(rt, D::FloatSub, 0.1, 0.3).unwrap(), float(0.1 - 0.3));
    assert_eq!(binary(rt, D::FloatMod, -7.5, 2.0).unwrap(), float(-1.5));
    let negative_zero = binary(rt, D::FloatMul, -1.0, 0.0).unwrap();
    assert_eq!(crate::scalar::float_text(f64::from_bits(negative_zero)), "-0");
    let unary = |rt: &mut Runtime, op, value: f64| run(rt, op, &[float(value)]);
    assert_eq!(crate::scalar::float_text(f64::from_bits(unary(rt, D::FloatNeg, 0.0).unwrap())), "-0");
    assert_eq!(unary(rt, D::FloatAbs, -0.0).unwrap(), float(0.0));
    assert_eq!(unary(rt, D::FloatSqrt, 2.25).unwrap(), float(1.5));
    assert_eq!(unary(rt, D::FloatSqrt, -1.0).unwrap_err(), "Can't take sqrt of -1");
    for (value, sign) in [(-0.0, "0"), (0.0, "0"), (-2.5, "-1"), (1e-300, "1")] {
        let word = unary(rt, D::FloatSign, value).unwrap();
        assert_eq!(int_text(rt, word), sign, "{value}");
    }
}

#[test]
fn rat_arithmetic_is_exact_and_normalized() {
    let mut rt = runtime();
    let rt = &mut *rt;
    let binary = |rt: &mut Runtime, op, left: (i64, i64), right: (i64, i64)| {
        let (left, right) = (rat(rt, left.0, left.1), rat(rt, right.0, right.1));
        run(rt, op, &[left, right]).map(|word| rat_text(rt, word))
    };
    assert_eq!(binary(rt, D::RatAdd, (1, 2), (1, 3)).unwrap(), "5/6");
    assert_eq!(binary(rt, D::RatSub, (1, 2), (1, 2)).unwrap(), "0/1");
    assert_eq!(binary(rt, D::RatMul, (-2, 3), (3, 4)).unwrap(), "-1/2");
    assert_eq!(binary(rt, D::RatDiv, (1, 2), (-1, 4)).unwrap(), "-2/1");
    assert_eq!(binary(rt, D::RatDiv, (1, 2), (0, 1)).unwrap_err(), "Division by zero");
    assert_eq!(binary(rt, D::RatMod, (7, 2), (0, 1)).unwrap_err(), "Modulo by zero");
    let unary = |rt: &mut Runtime, op, value: (i64, i64)| {
        let value = rat(rt, value.0, value.1);
        run(rt, op, &[value]).unwrap()
    };
    let word = unary(rt, D::RatNeg, (3, 4));
    assert_eq!(rat_text(rt, word), "-3/4");
    let word = unary(rt, D::RatAbs, (-3, 4));
    assert_eq!(rat_text(rt, word), "3/4");
    let word = unary(rt, D::RatSign, (-3, 4));
    assert_eq!(int_text(rt, word), "-1");
    let word = unary(rt, D::RatNumerator, (-6, 8));
    assert_eq!(rt.heap.text(word), "-3");
    let word = unary(rt, D::RatDenominator, (-6, 8));
    assert_eq!(rt.heap.text(word), "4");
}

fn texts(runtime: &Runtime, array: Word) -> Vec<String> {
    runtime.heap.elements(array).iter().map(|&word| runtime.heap.text(word).to_owned()).collect()
}

#[test]
fn string_slices_count_code_points_and_refuse_bad_bounds() {
    let mut rt = runtime();
    let rt = &mut *rt;
    let slice = |rt: &mut Runtime, value: &str, start: i64, end: i64| {
        let value = text(rt, value);
        let (start, end) = (rt.heap.int(start), rt.heap.int(end));
        run(rt, D::StrSlice, &[value, start, end]).map(|word| rt.heap.text(word).to_owned())
    };
    assert_eq!(slice(rt, "héllo", 1, 3).unwrap(), "él");
    assert_eq!(slice(rt, "héllo", 5, 5).unwrap(), "");
    assert_eq!(slice(rt, "héllo", -1, 2).unwrap_err(), "Slice start must not be negative");
    assert_eq!(slice(rt, "héllo", 3, 2).unwrap_err(), "Slice start must be less than or equal to slice end");
    assert_eq!(slice(rt, "héllo", 0, 6).unwrap_err(), "Slice end must not exceed length");
}

#[test]
fn string_split_edges_follow_tomato() {
    let mut rt = runtime();
    let rt = &mut *rt;
    let split = |rt: &mut Runtime, value: &str, separator: &str| {
        let (value, separator) = (text(rt, value), text(rt, separator));
        let array = run(rt, D::StrSplit, &[value, separator]).unwrap();
        texts(rt, array)
    };
    assert_eq!(split(rt, "a,,b", ","), ["a", "", "b"]);
    assert_eq!(split(rt, "", ","), [""]);
    assert_eq!(split(rt, ",a,", ","), ["", "a", ""]);
    assert_eq!(split(rt, "aé", ""), ["a", "é"]);
    assert_eq!(split(rt, "a--b--", "--"), ["a", "b", ""]);
}

#[test]
fn string_replace_trim_and_case_follow_tomato() {
    let mut rt = runtime();
    let rt = &mut *rt;
    let three = |rt: &mut Runtime, op, value: &str, target: &str, replacement: &str| {
        let words = [text(rt, value), text(rt, target), text(rt, replacement)];
        let word = run(rt, op, &words).unwrap();
        rt.heap.text(word).to_owned()
    };
    assert_eq!(three(rt, D::StrReplace, "aaa", "a", "bb"), "bbbbbb");
    assert_eq!(three(rt, D::StrReplace, "abc", "", "-"), "-a-b-c-");
    assert_eq!(three(rt, D::StrReplace, "abc", "x", "-"), "abc");
    assert_eq!(three(rt, D::StrReplaceFirst, "a.b.c", ".", "/"), "a/b.c");
    let one = |rt: &mut Runtime, op, value: &str| {
        let value = text(rt, value);
        let word = run(rt, op, &[value]).unwrap();
        rt.heap.text(word).to_owned()
    };
    let spaced = "\u{3000}\t x y \n\u{85}\u{a0}";
    assert_eq!(one(rt, D::StrTrim, spaced), "x y");
    assert_eq!(one(rt, D::StrTrimStart, spaced), "x y \n\u{85}\u{a0}");
    assert_eq!(one(rt, D::StrTrimEnd, spaced), "\u{3000}\t x y");
    assert_eq!(one(rt, D::StrChomp, "a\r\n"), "a");
    assert_eq!(one(rt, D::StrChomp, "a\n\n"), "a\n");
    assert_eq!(one(rt, D::StrChomp, "a\r"), "a\r");
    assert_eq!(one(rt, D::StrLower, "ÉA"), "éa");
    assert_eq!(one(rt, D::StrUpper, "straße"), "STRASSE");
    let two = |rt: &mut Runtime, op, value: &str, part: &str| {
        let words = [text(rt, value), text(rt, part)];
        run(rt, op, &words).unwrap()
    };
    let found = two(rt, D::StrFind, "héllo", "l");
    assert_eq!(int_text(rt, found), "2");
    let found = two(rt, D::StrFindLast, "héllo", "l");
    assert_eq!(int_text(rt, found), "3");
    let found = two(rt, D::StrFind, "héllo", "x");
    assert_eq!(int_text(rt, found), "-1");
    assert_eq!(two(rt, D::StrStartsWith, "héllo", "hé"), 1);
    assert_eq!(two(rt, D::StrEndsWith, "héllo", "x"), 0);
    let value = text(rt, "é");
    let bytes = run(rt, D::StrUtf8, &[value]).unwrap();
    assert_eq!(rt.heap.bytes(bytes), [0xc3, 0xa9]);
}

#[test]
fn string_repeat_refuses_a_negative_count() {
    let mut rt = runtime();
    let rt = &mut *rt;
    let repeat = |rt: &mut Runtime, count: &str| {
        let value = text(rt, "ab");
        let count = int(rt, count);
        run(rt, D::StrRepeat, &[value, count]).map(|word| rt.heap.text(word).to_owned())
    };
    assert_eq!(repeat(rt, "3").unwrap(), "ababab");
    assert_eq!(repeat(rt, "0").unwrap(), "");
    assert_eq!(repeat(rt, "-1").unwrap_err(), "repeat needs a non-negative count");
    assert_eq!(repeat(rt, "-9223372036854775809").unwrap_err(), "repeat needs a non-negative count");
}

fn context(runtime: &mut Runtime) -> Ctx {
    Ctx { status: OK, stack_limit: 0, failure: 0, runtime, entry_frame: 0, scratch: 0 }
}

#[test]
fn scalar_helper_fails_with_a_program_failure_and_stops_on_the_memory_budget() {
    let mut rt = runtime();
    let mut ctx = context(&mut rt);
    let code = D::IntShl.scalar_code().unwrap();
    let (one, sixty_four) = (rt.heap.int(1), rt.heap.int(64));
    assert_eq!(rt_machine_scalar(&mut ctx, code, one, sixty_four, 0), 0);
    assert_eq!(ctx.status, FAILED);
    let failure = rt.failure(ctx.failure).unwrap();
    assert!(format!("{:?}", failure.kind).contains("Shift count must be in 0..63"), "{:?}", failure.kind);
    let mut ctx = context(&mut rt);
    let value = rt.text_new("abcd");
    rt.roots.push((value, Repr::Ref));
    let count = rt.heap.int(1 << 20);
    rt.limits.memory = Some(1 << 20);
    rt.limits.memory_scope = crate::budget::MemoryScope::Runtime;
    let code = D::StrRepeat.scalar_code().unwrap();
    assert_eq!(rt_machine_scalar(&mut ctx, code, value, count, 0), 0);
    assert_eq!(ctx.status, EXHAUSTED);
    let mut ctx = context(&mut rt);
    rt.limits.memory = None;
    let code = D::StrTrim.scalar_code().unwrap();
    let trimmed = rt_machine_scalar(&mut ctx, code, value, 0, 0);
    assert_eq!(ctx.status, OK);
    assert_eq!(rt.heap.text(trimmed), "abcd");
    let mut ctx = context(&mut rt);
    // A code outside the families is not a verified operation: the run stops, it does not fail.
    assert_eq!(D::IntAdd.scalar_code(), None);
    rt_machine_scalar(&mut ctx, 0, value, 0, 0);
    assert_ne!(ctx.status, OK);
    assert_ne!(ctx.status, FAILED);
}
