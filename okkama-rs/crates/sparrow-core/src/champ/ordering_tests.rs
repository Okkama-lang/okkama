//! `array.order` keeps Tomato's natural order and budget stops; the merge both engines run stops at the first
//! failing comparison (#678).
use super::*;
use crate::intrinsics::merge_order;
use crate::runtime::{EXHAUSTED, Runtime};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

fn runtime() -> Box<Runtime> {
    Runtime::new_champ(s::Program::default(), vec![]).unwrap()
}

fn context(runtime: &mut Runtime) -> Ctx {
    Ctx { status: OK, stack_limit: 0, failure: 0, runtime, entry_frame: 0, scratch: 0 }
}

fn array(leaf: s::ArrayLeaf) -> s::Type {
    s::Type::Array(s::ArrayType { leaf, depth: 1 })
}

fn sort(rt: &mut Runtime, ty: s::Type, items: &[Word]) -> (u64, Word) {
    let source = if ty == s::Type::Bytes {
        rt.heap.allocate_bytes(&items.iter().map(|&word| word as u8).collect::<Vec<_>>())
    } else {
        rt.heap.allocate_array(repr(s::element_type(ty).unwrap()), items)
    };
    let mut ctx = context(rt);
    let sorted = order_natural(&mut ctx, source, ty);
    (ctx.status, sorted)
}

#[test]
fn natural_order_matches_tomatos_comparators() {
    let mut rt = runtime();
    let values = ["5", "-3", "18446744073709551617", "0", "-18446744073709551617", "5"];
    let ints = values.iter().map(|text| rt.heap.big(text.parse().unwrap())).collect::<Vec<_>>();
    let (status, sorted) = sort(&mut rt, array(s::ArrayLeaf::Int), &ints);
    assert_eq!(status, OK);
    let texts = rt.heap.elements(sorted).iter().map(|&word| rt.big_value(word).to_string()).collect::<Vec<_>>();
    assert_eq!(texts, ["-18446744073709551617", "-3", "0", "5", "5", "18446744073709551617"]);
    let words = ["pear", "Apple", "apple", "fig"].map(|text| rt.heap.allocate_text(text));
    let (status, sorted) = sort(&mut rt, array(s::ArrayLeaf::Str), &words);
    assert_eq!(status, OK);
    let texts = rt.heap.elements(sorted).iter().map(|&word| rt.heap.text(word).to_string()).collect::<Vec<_>>();
    assert_eq!(texts, ["Apple", "apple", "fig", "pear"]);
    let floats = [2.5f64, -1.0, 0.5, -0.0].map(f64::to_bits);
    let (status, sorted) = sort(&mut rt, array(s::ArrayLeaf::Float), &floats);
    assert_eq!(status, OK);
    let values = rt.heap.elements(sorted).iter().map(|&word| f64::from_bits(word)).collect::<Vec<_>>();
    assert_eq!(values, [-1.0, -0.0, 0.5, 2.5]);
    let chars = ['я', 'a', 'Z'].map(|ch| Word::from(u32::from(ch)));
    let (status, sorted) = sort(&mut rt, array(s::ArrayLeaf::Char), &chars);
    assert_eq!(status, OK);
    assert_eq!(rt.heap.elements(sorted).to_vec(), ['Z', 'a', 'я'].map(|ch| Word::from(u32::from(ch))));
}

#[test]
fn a_byte_array_stays_bytes() {
    let mut rt = runtime();
    let (status, sorted) = sort(&mut rt, s::Type::Bytes, &[200, 3, 255, 0, 3]);
    assert_eq!(status, OK);
    assert_eq!(rt.heap.byte_view(sorted).to_vec(), [0, 3, 3, 200, 255]);
}

#[test]
fn an_interrupt_stops_a_long_sort() {
    let mut rt = runtime();
    rt.limits.interrupt = Some(Arc::new(AtomicBool::new(true)));
    let ints = (0..20_000i64).map(|i| rt.heap.int((i * 7919 + 13) % 10_007)).collect::<Vec<_>>();
    let (status, _) = sort(&mut rt, array(s::ArrayLeaf::Int), &ints);
    assert_eq!(status, EXHAUSTED);
    // Without the interrupt the same sort completes.
    rt.limits.interrupt = None;
    rt.budget.exhausted = None;
    let (status, sorted) = sort(&mut rt, array(s::ArrayLeaf::Int), &ints);
    assert_eq!(status, OK);
    let values = rt.heap.elements(sorted).iter().map(|&word| rt.small_or_big(word).unwrap()).collect::<Vec<_>>();
    assert!(values.windows(2).all(|pair| pair[0] <= pair[1]));
}

#[test]
fn the_merge_compares_in_tomatos_order_and_stops_at_the_first_failure() {
    let mut compared = vec![];
    let sorted = merge_order(vec![5, 3, 8, 3, 1], |left, right| {
        compared.push((left, right));
        Some(left > right)
    });
    assert_eq!(sorted, Some(vec![1, 3, 3, 5, 8]));
    assert_eq!(compared, [(5, 3), (8, 3), (3, 3), (5, 3), (5, 8), (3, 1)]);
    let mut count = 0;
    let failed = merge_order(vec![5, 3, 8, 3, 1], |left, right| {
        count += 1;
        (count < 3).then_some(left > right)
    });
    assert_eq!((failed, count), (None, 3));
}
