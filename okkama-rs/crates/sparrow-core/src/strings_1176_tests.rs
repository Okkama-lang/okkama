//! #1176: a slice of 64 bytes or more shares its source's storage when it keeps at most twice what it holds; a
//! slice over a flat string is never taken for a growable text buffer.
use super::*;
use crate::heap::{SHARED, SLICE_MIN};

fn runtime() -> Box<Runtime> {
    let program = okkama_sparrow::assemble(
        "sparrow 19\nmodule \"slices\" \"1.0.0\"\ntype t0 int\nfunction f0 \"main\" {\n registers [t0]\n params []\n result t0\n 0: const r0, int 0\n 1: ret r0\n}\nentry f0\n",
    )
    .unwrap();
    Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap())
}

fn storage(runtime: &Runtime, view: Word) -> Word {
    assert_ne!(runtime.heap.header_of(view) & SHARED, 0);
    unsafe { *((view + 16) as *const u64) }
}

#[test]
fn a_slice_of_a_flat_string_is_never_grown_into() {
    let mut runtime = runtime();
    let source_text: String = (0..200).map(|n| char::from(b'a' + (n % 26) as u8)).collect();
    let source = runtime.text_new(&source_text);
    let slice = runtime.heap.substring(source, 0, 150);
    assert_eq!(storage(&runtime, slice), source, "a view of the flat string");
    assert!(!runtime.heap.text_chain(slice));
    // `+` on either side copies: the flat source is never written, not even past the slice's end.
    let tail = runtime.text_new("XYZ");
    let appended = concat_text(&mut runtime, slice, tail, |_, _| true).unwrap();
    let prepended = concat_text(&mut runtime, tail, slice, |_, _| true).unwrap();
    assert_eq!(runtime.heap.text(source), source_text);
    assert_eq!(runtime.heap.text(appended), format!("{}XYZ", &source_text[..150]));
    assert_eq!(runtime.heap.text(prepended), format!("XYZ{}", &source_text[..150]));
    let fused = concat_parts(&mut runtime, &[slice, tail, tail], |_, _| true).unwrap().unwrap();
    assert_eq!(runtime.heap.text(fused), format!("{}XYZXYZ", &source_text[..150]));
    assert_eq!(runtime.heap.text(source), source_text);
}

#[test]
#[should_panic(expected = "no text buffer")]
fn writing_into_a_flat_string_is_refused() {
    let mut runtime = runtime();
    let source = runtime.text_new(&"q".repeat(100));
    runtime.heap.text_buffer_write(source, "x", false);
}

#[test]
fn only_bounded_slices_share_and_never_through_another_view() {
    let mut runtime = runtime();
    let text = "é".repeat(50) + &"z".repeat(100); // 200 bytes, not ASCII
    let source = runtime.text_new(&text);
    // Shorter than SLICE_MIN: copied.
    let short = runtime.heap.substring(source, 100, 100 + SLICE_MIN - 1);
    assert_eq!(runtime.heap.header_of(short) & SHARED, 0);
    // 90 of 200 bytes: would keep more than twice what it holds, copied.
    let small = runtime.heap.substring(source, 110, 200);
    assert_eq!(runtime.heap.header_of(small) & SHARED, 0);
    // 150 of 200: shared, at the right bytes.
    let big = runtime.heap.substring(source, 50, 200);
    assert_eq!(storage(&runtime, big), source);
    assert_eq!(runtime.heap.text(big), &text[50..200]);
    // A slice of the slice points at the flat string itself and is measured against it.
    let inner = runtime.heap.substring(big, 10, 120);
    assert_eq!(storage(&runtime, inner), source);
    assert_eq!(runtime.heap.text(inner), &text[60..170]);
    assert!(!runtime.heap.is_ascii_text(inner), "the source is not ASCII");
    let ascii_source = runtime.text_new(&"a".repeat(128));
    let ascii_slice = runtime.heap.substring(ascii_source, 0, 100);
    assert!(runtime.heap.is_ascii_text(ascii_slice));
}

#[test]
fn a_slice_keeps_its_storage_alive_and_a_text_buffer_version_slices_into_the_buffer() {
    let mut runtime = runtime();
    runtime.heap.set_stress(true);
    let text = "w".repeat(300);
    let source = runtime.text_new(&text);
    let slice = runtime.heap.substring(source, 20, 280);
    runtime.heap.collect(|visit| visit(slice, Repr::Ref));
    assert!(!runtime.heap.is_reclaimed(source));
    assert_eq!(runtime.heap.text(slice), &text[20..280]);
    // A version in a text buffer: its slice shares the buffer, measured by the buffer's room.
    let long = runtime.text_new(&"v".repeat(300));
    let (a, b) = (runtime.text_new("a"), runtime.text_new("b"));
    let first = concat_text(&mut runtime, long, a, |_, _| true).unwrap();
    let mut version = concat_text(&mut runtime, first, b, |_, _| true).unwrap();
    assert!(runtime.heap.text_chain(version));
    let buffer = storage(&runtime, version);
    // 302 bytes in a buffer of 1024: a piece of 300 would keep more than twice what it holds, so it is copied.
    let copied = runtime.heap.substring(version, 0, 300);
    assert_eq!(runtime.heap.header_of(copied) & SHARED, 0);
    // Appends fill the same buffer in place; at 600 bytes a piece of 600 shares it.
    while runtime.heap.text(version).len() < 600 {
        version = concat_text(&mut runtime, version, b, |_, _| true).unwrap();
    }
    assert_eq!(storage(&runtime, version), buffer, "appended in place");
    let piece = runtime.heap.substring(version, 0, 600);
    assert_eq!(storage(&runtime, piece), buffer);
    assert_eq!(runtime.heap.text(piece), runtime.heap.text(version).to_string());
    // A piece of a buffer is a version of it, as a branch is.
    assert!(runtime.heap.text_chain(piece));
}

#[test]
fn dropping_the_head_in_a_loop_copies_each_half_once() {
    let mut runtime = runtime();
    let n = 4096;
    let mut rest = runtime.text_new(&"r".repeat(n));
    let before = runtime.heap.stats().allocated_bytes;
    for _ in 0..n - 1 {
        let length = runtime.heap.text(rest).len();
        rest = runtime.heap.substring(rest, 1, length);
    }
    assert_eq!(runtime.heap.text(rest), "r");
    // Views are three words and the copies of each half sum to about n bytes; copying every slice would take n²/2
    // (8 MiB here). The heap counts its growth in 64 KiB pages, so the bound is coarse.
    let allocated = (runtime.heap.stats().allocated_bytes - before) as usize;
    assert!(allocated < n * n / 8, "{allocated} bytes for {n} slices");
}
