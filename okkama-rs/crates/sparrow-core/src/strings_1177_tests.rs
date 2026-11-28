//! #1177: `str.concat_n` gives what pairwise `+` gives, copying each part once; a long or chained part keeps the
//! pairwise path (amortized appends); the 4 GiB limit fails with the pairwise text.
use super::*;
use crate::heap::{APPENDED, SHARED};

fn runtime() -> Box<Runtime> {
    let program = okkama_sparrow::assemble(
        "sparrow 19\nmodule \"concat\" \"1.0.0\"\ntype t0 int\nfunction f0 \"main\" {\n registers [t0]\n params []\n result t0\n 0: const r0, int 0\n 1: ret r0\n}\nentry f0\n",
    )
    .unwrap();
    Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap())
}

fn fused(runtime: &mut Runtime, parts: &[&str]) -> Word {
    let words: Vec<Word> = parts.iter().map(|part| runtime.text_new(part)).collect();
    concat_parts(runtime, &words, |_, _| true).unwrap().unwrap()
}

fn pairwise(runtime: &mut Runtime, parts: &[&str]) -> Word {
    let mut text = runtime.text_new(parts[0]);
    for part in &parts[1..] {
        let right = runtime.text_new(part);
        text = concat_text(runtime, text, right, |_, _| true).unwrap();
    }
    text
}

#[test]
fn fused_concatenation_equals_the_pairwise_one() {
    let mut runtime = runtime();
    let long = "z".repeat(300);
    for parts in [
        vec!["id=", "42", ";name=item-", "7", ";price=", "13"],
        vec!["", "", ""],
        vec!["Привет", ", ", "мир", "!"],
        vec!["ascii", "é", "ascii"],
        vec![long.as_str(), "a", "b"],
        vec!["a", long.as_str(), "b"],
        vec!["0123456789"; 40],
    ] {
        let (a, b) = (fused(&mut runtime, &parts), pairwise(&mut runtime, &parts));
        assert_eq!(runtime.heap.text(a), runtime.heap.text(b), "{parts:?}");
        assert_eq!(runtime.heap.text(a), parts.concat(), "{parts:?}");
        // Lengths and slices go by the ASCII flag: it must be right.
        let ascii = parts.iter().all(|part| part.is_ascii());
        assert_eq!(runtime.heap.header_of(a) & crate::heap::ASCII != 0, ascii, "{parts:?}");
    }
}

#[test]
fn short_parts_are_copied_once_and_marked_as_the_last_plus_would() {
    let mut runtime = runtime();
    // Short parts: one flat string; under 256 bytes before the last part, no mark.
    let short = fused(&mut runtime, &["a", "b", "c"]);
    assert_eq!(runtime.heap.header_of(short) & (APPENDED | SHARED), 0);
    // 40 parts of 10 bytes: the parts before the last reach 256 bytes, so the next `+` starts a buffer.
    let parts = vec!["0123456789"; 40];
    let marked = fused(&mut runtime, &parts);
    assert_ne!(runtime.heap.header_of(marked) & APPENDED, 0);
    assert_eq!(runtime.heap.header_of(marked) & SHARED, 0, "flat, copied once");
}

#[test]
fn a_long_left_part_keeps_the_pairwise_buffer_chain() {
    let mut runtime = runtime();
    let long = "z".repeat(300);
    // `long + "a"` copies and marks APPENDED; `+ "b"` then starts a text buffer: the amortized append.
    let text = fused(&mut runtime, &[long.as_str(), "a", "b"]);
    assert_ne!(runtime.heap.header_of(text) & SHARED, 0, "a view of a text buffer");
}

#[test]
fn past_four_gibibytes_fails_with_the_pairwise_text() {
    let mut runtime = runtime();
    // Two strings whose headers say 3 GiB each: only the headers are read before the limit check.
    let fake = |runtime: &mut Runtime| {
        let cell = runtime.heap.allocate(2);
        unsafe { *(cell as *mut u64) = crate::heap::header(crate::heap::STRING, Repr::Raw, 3 << 30) };
        cell
    };
    let (a, b) = (fake(&mut runtime), fake(&mut runtime));
    let empty = runtime.text_new("");
    assert_eq!(concat_parts(&mut runtime, &[a, empty, b], |_, _| true).unwrap_err(), concat_limit());
    // A memory budget that refuses the size: no string.
    let small: Vec<Word> = ["a", "b", "c"].iter().map(|part| runtime.text_new(part)).collect();
    assert!(concat_parts(&mut runtime, &small, |_, _| false).unwrap().is_none());
}
