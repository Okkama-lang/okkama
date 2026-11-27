//! #1149: split by one character searches the character; replace copies its pieces once; a piece of an ASCII
//! string, and a flat concatenation of ASCII strings, are ASCII without a second look at their bytes.
use super::*;

fn runtime() -> Box<Runtime> {
    let program = okkama_sparrow::assemble(
        "sparrow 18\nmodule \"strings\" \"1.0.0\"\ntype t0 int\nfunction f0 \"main\" {\n registers [t0]\n params []\n result t0\n 0: const r0, int 0\n 1: ret r0\n}\nentry f0\n",
    )
    .unwrap();
    Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap())
}

fn pieces(runtime: &mut Runtime, array: Word) -> Vec<String> {
    runtime.heap.elements(array).iter().map(|&piece| runtime.heap.text(piece).to_string()).collect()
}

#[test]
fn split_by_a_character_or_a_pattern_gives_every_piece() {
    let mut runtime = runtime();
    for (text, separator, expected) in [
        ("a;b;;c;", ";", vec!["a", "b", "", "c", ""]),
        (";", ";", vec!["", ""]),
        ("no separator", ";", vec!["no separator"]),
        ("x→y→z", "→", vec!["x", "y", "z"]),
        ("a::b:c::", "::", vec!["a", "b:c", ""]),
        ("ééé", "é", vec!["", "", "", ""]),
    ] {
        let (text_word, separator_word) = (runtime.text_new(text), runtime.text_new(separator));
        let array = str_split(&mut runtime, text_word, separator_word);
        assert_eq!(pieces(&mut runtime, array), expected, "{text:?} by {separator:?}");
    }
}

#[test]
fn replace_copies_the_pieces_between_matches_once() {
    let mut runtime = runtime();
    for (text, pattern, replacement, first, expected) in [
        ("item-1;item-2", "item-", "product_", false, "product_1;product_2"),
        ("item-1;item-2", "item-", "product_", true, "product_1;item-2"),
        ("aaa", "aa", "b", false, "ba"),
        ("xyz", "q", "r", false, "xyz"),
        ("--", "-", "", false, ""),
        ("abc", "", "-", false, "-a-b-c-"),
        ("naïve", "ï", "i", false, "naive"),
    ] {
        let words = [runtime.text_new(text), runtime.text_new(pattern), runtime.text_new(replacement)];
        let result = str_replace(&mut runtime, words[0], words[1], words[2], first).unwrap();
        assert_eq!(runtime.heap.text(result), expected, "{text:?} {pattern:?} -> {replacement:?}");
        // Known from the flags when both texts are ASCII, otherwise found by a look at the result's bytes.
        assert_eq!(runtime.heap.is_ascii_text(result), expected.is_ascii(), "{expected:?}");
    }
}

#[test]
fn pieces_and_concatenations_of_ascii_strings_are_flagged_ascii() {
    let mut runtime = runtime();
    let ascii = runtime.text_new("id=1;name=item-7");
    let piece = runtime.heap.substring(ascii, 3, 4);
    assert!(runtime.heap.is_ascii_text(piece));
    let mixed = runtime.text_new("é;x");
    let ascii_piece_of_mixed = runtime.heap.substring(mixed, 3, 4);
    assert!(runtime.heap.is_ascii_text(ascii_piece_of_mixed), "found by a look at its bytes");
    let non_ascii_piece = runtime.heap.substring(mixed, 0, 2);
    assert!(!runtime.heap.is_ascii_text(non_ascii_piece));
}
