//! #1171: a builder is a heap cell over a buffer in the heap. Pushes past the capacity move the elements to a buffer
//! twice as big; the collector traces the elements below the length (and only those); `finish` makes the buffer the
//! array in place when its cell fits the length, and copies otherwise.
use super::*;

fn stressed() -> Heap {
    let mut heap = Heap::new(Layouts::default());
    heap.set_stress(true);
    heap
}

#[test]
fn pushed_references_survive_collections_while_the_builder_grows() {
    let mut heap = stressed();
    let builder = heap.allocate_builder(Repr::Ref, 0);
    for n in 0..300 {
        let text = heap.allocate_text(&format!("item {n}"));
        heap.builder_push(builder, text);
        // Only the builder is a root: its buffer and every element below the length must survive.
        heap.collect(|visit| visit(builder, Repr::Ref));
    }
    assert_eq!(heap.builder_length(builder), 300);
    for (n, &item) in heap.builder_items(builder).iter().enumerate() {
        assert_eq!(heap.text(item), format!("item {n}"));
    }
}

#[test]
fn slots_past_the_length_are_not_traced() {
    let mut heap = stressed();
    let builder = heap.allocate_builder(Repr::Ref, 40);
    let kept = heap.allocate_text("kept");
    heap.builder_push(builder, kept);
    // A word past the length that looks like a pointer to a dead cell must not be followed: the collector would
    // otherwise mark a reclaimed cell.
    let dead = heap.allocate_text("dead");
    let buffer = unsafe { *((builder + BUILDER_BUFFER as Word) as *const u64) };
    unsafe { *((buffer + 16) as *mut u64) = dead };
    heap.collect(|visit| visit(builder, Repr::Ref));
    assert!(heap.is_reclaimed(dead));
    assert!(!heap.is_reclaimed(kept));
    assert!(!heap.is_reclaimed(buffer));
}

#[test]
fn finish_takes_the_buffer_in_place_when_its_cell_fits() {
    let mut heap = stressed();
    // Room for 39 (a cell of 40 words); 35 elements need a cell of 40 words too: in place.
    let builder = heap.allocate_builder(Repr::Int, 39);
    let buffer = unsafe { *((builder + BUILDER_BUFFER as Word) as *const u64) };
    heap.builder_fill(builder, 35, |n| word::small(n as i64).unwrap());
    let array = heap.builder_finish(builder, false);
    assert_eq!(array, buffer, "the buffer is the array");
    assert_eq!(heap.array_length(array), 35);
    assert_eq!(heap.elements(array).iter().map(|&w| word::small_value(w).unwrap()).collect::<Vec<_>>(), (0..35).collect::<Vec<_>>());
    assert_eq!(heap.builder_length(builder), 0);
    // 10 elements in a 40-word cell: a cell of 12 words is enough, so they are copied.
    let builder = heap.allocate_builder(Repr::Int, 39);
    let buffer = unsafe { *((builder + BUILDER_BUFFER as Word) as *const u64) };
    heap.builder_fill(builder, 10, |n| word::small(n as i64).unwrap());
    let array = heap.builder_finish(builder, false);
    assert_ne!(array, buffer);
    assert_eq!(heap.array_length(array), 10);
    heap.collect(|visit| visit(array, Repr::Ref));
    assert!(heap.is_reclaimed(buffer), "the copied-from buffer is garbage");
}

#[test]
fn large_buffers_finish_in_place_only_when_little_room_is_left() {
    let mut heap = stressed();
    let exact = heap.allocate_builder(Repr::Int, 1000);
    let buffer = unsafe { *((exact + BUILDER_BUFFER as Word) as *const u64) };
    heap.builder_fill(exact, 1000, |n| word::small(n as i64).unwrap());
    assert_eq!(heap.builder_finish(exact, false), buffer);
    let roomy = heap.allocate_builder(Repr::Int, 1000);
    let buffer = unsafe { *((roomy + BUILDER_BUFFER as Word) as *const u64) };
    heap.builder_fill(roomy, 800, |n| word::small(n as i64).unwrap());
    let array = heap.builder_finish(roomy, false);
    assert_ne!(array, buffer, "a fifth of the cell left: copied");
    assert_eq!(heap.array_length(array), 800);
}

#[test]
fn packed_builders_finish_as_bytes() {
    let mut heap = stressed();
    let builder = heap.allocate_builder(Repr::Raw, 0);
    heap.builder_extend(builder, &[1, 2, 255]);
    let array = heap.builder_finish(builder, true);
    assert_eq!(heap.bytes(array), &[1, 2, 255]);
}

#[test]
fn a_published_array_stays_as_it_was_while_new_builders_fill() {
    let mut heap = stressed();
    let first = heap.allocate_builder(Repr::Int, 0);
    heap.builder_extend(first, &[word::small(1).unwrap(), word::small(2).unwrap(), word::small(3).unwrap()]);
    let published = heap.builder_finish(first, false);
    for round in 0..50 {
        let next = heap.allocate_builder(Repr::Int, 0);
        heap.builder_fill(next, 3, |n| word::small(100 + round + n as i64).unwrap());
        let _ = heap.builder_finish(next, false);
        heap.collect(|visit| visit(published, Repr::Ref));
    }
    assert_eq!(heap.elements(published).iter().map(|&w| word::small_value(w).unwrap()).collect::<Vec<_>>(), vec![1, 2, 3]);
}

#[test]
fn a_sized_builder_takes_its_count_as_room_within_bounds() {
    let program = okkama_sparrow::assemble(
        "sparrow 19\nmodule \"sized\" \"1.0.0\"\ntype t0 int\nfunction f0 \"main\" {\n registers [t0]\n params []\n result t0\n 0: const r0, int 0\n 1: ret r0\n}\nentry f0\n",
    )
    .unwrap();
    let mut runtime = Box::new(crate::runtime::Runtime::new(crate::program::Loaded::new(program)).unwrap());
    let capacity = |builder: Word| unsafe { *((builder + BUILDER_CAPACITY as Word) as *const u64) } as usize;
    let room = |runtime: &mut crate::runtime::Runtime, count: Word| {
        let builder = crate::ops::builder_sized(runtime, Repr::Int, count);
        capacity(builder)
    };
    // A count fills a small cell's whole room, or exactly a large one.
    assert_eq!(room(&mut runtime, word::small(30).unwrap()), 31);
    assert_eq!(room(&mut runtime, word::small(100_000).unwrap()), 100_000);
    // No hint: zero, negative, a big integer.
    assert_eq!(room(&mut runtime, word::small(0).unwrap()), 0);
    assert_eq!(room(&mut runtime, word::small(-5).unwrap()), 0);
    let big = runtime.heap.big(num_bigint::BigInt::from(1u64 << 62) * 4);
    assert_eq!(room(&mut runtime, big), 0);
    // A huge count is capped: the builder grows past the cap as it is filled.
    assert_eq!(room(&mut runtime, word::small(1 << 40).unwrap()), 1 << 22);
}
