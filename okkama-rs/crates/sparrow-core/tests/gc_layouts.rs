//! #437: layout tracing must preserve every potentially pointer-bearing slot.
//! In-process, bounded fixtures; timings belong to the isolated A/B run.
use sparrow_core::heap::{self, Heap, Integer, Layouts};
use sparrow_core::program::Loaded;
use sparrow_core::word::{self, Repr, UNBOUND, UNBOUND_RAW};

#[test]
fn gc_layout_empty_and_all_raw_objects_and_closures() {
    for width in [0, 8] {
        let mut heap = Heap::new(Layouts {
            objects: vec![vec![Repr::Raw; width]],
            // A closure's layout starts with its entry word (#1110).
            closures: vec![vec![Repr::Raw; width + 1]],
            ..Layouts::default()
        });
        heap.set_stress(true);
        let raw = heap.allocate_text("not a reference field");
        let fields = vec![raw; width];
        let object = heap.object_new(0, &fields);
        let closure = heap.closure_new(0, &fields);
        heap.collect(|visit| {
            visit(object, Repr::Ref);
            visit(closure, Repr::Ref);
        });
        assert_eq!(heap::kind(heap.header_of(object)), heap::OBJECT);
        assert_eq!(heap::kind(heap.header_of(closure)), heap::CLOSURE);
        assert_eq!(heap::kind(heap.header_of(raw)), heap::FREE);
        heap.collect(|_| {});
        assert_eq!(heap.stats().live_bytes, 0);
    }
}

#[test]
fn gc_layout_mixed_objects_and_closures() {
    let layout = vec![Repr::Raw, Repr::Ref, Repr::Int, Repr::Raw, Repr::Int, Repr::Ref, Repr::Int];
    for stress in [false, true] {
        for kind in [heap::OBJECT, heap::CLOSURE] {
            let mut heap = Heap::new(Layouts {
                objects: vec![layout.clone()],
                // A closure's layout starts with its entry word, then its parameters (#1110).
                closures: vec![std::iter::once(Repr::Raw).chain(layout.iter().copied()).collect()],
                frames: vec![layout.clone()],
            });
            heap.set_stress(stress);
            let text = heap.allocate_text("live through a sparse layout");
            let big = heap.int(i64::MAX);
            // Even pointer-shaped raw bits must not keep this object alive.
            let raw = heap.allocate_text("raw bits only");
            let fields = [raw, text, big, UNBOUND_RAW, word::small(-7).unwrap(), UNBOUND, UNBOUND];
            let object = match kind {
                heap::OBJECT => heap.object_new(0, &fields),
                heap::CLOSURE => heap.closure_new(0, &fields[..5]),
                _ => unreachable!(),
            };
            // An object's fields and a closure's captures, by the same index (#1110: captures after the entry).
            let slot = |heap: &Heap, index: usize| if kind == heap::CLOSURE { heap.capture(object, index) } else { heap.field(object, index) };
            for _ in 0..3 {
                heap.collect(|visit| visit(object, Repr::Ref));
                assert_eq!(heap.text(slot(&heap, 1)), "live through a sparse layout");
                assert_eq!(heap.integer(slot(&heap, 2)).to_string(), i64::MAX.to_string());
                assert_eq!(heap.integer(slot(&heap, 4)), Integer::Small(-7));
                assert_eq!(slot(&heap, 5), UNBOUND);
                assert_eq!(slot(&heap, 6), UNBOUND);
                assert_eq!(heap::kind(heap.header_of(raw)), heap::FREE);
            }
            heap.collect(|_| {});
            assert_eq!(heap.stats().live_bytes, 0);
            assert_eq!(heap.stats().external_live_bytes, 0);
        }
    }
}

#[test]
fn gc_layout_retained_arguments_survive_reused_async_registers() {
    let program = okkama_sparrow::assemble(
        "sparrow 18\nmodule \"gc-layout-frame\" \"1.0.0\"\n\
         type t0 int\ntype t1 str\ntype t2 float\ntype t3 bool\ntype t4 object obj0\n\
         object obj0 \"Lease\" {\n destroy f1\n}\n\
         function f0 \"pending\" async {\n\
         registers [t0, t2, t1, t3]\nparams [r0, r2]\nresult t0\n0: ret r0\n}\n\
         function f1 \"Lease.destroy\" {\n registers [t4]\n params [r0]\n result void\n 0: ret _\n}\n",
    ).unwrap();
    let mut runtime = sparrow_core::runtime::Runtime::new(Loaded::verified(program).unwrap()).unwrap();
    runtime.heap.set_stress(true);
    let integer = runtime.heap.int(i64::MIN);
    let text = runtime.heap.allocate_text("retained argument");
    let frame = runtime.future_new(0, &[integer, text]);
    // Field indices from the frame layout: registers, then the retained inputs (a program with resources: the
    // destroy above).
    let register = (sparrow_core::tasks::REGISTERS / 8 - 1) as usize;
    // Four registers, then the ownership regions (#932), then the retained inputs.
    let retained = register + 4 + 2;
    // The register copies die; the retained inputs must remain independently live.
    runtime.heap.set_field(frame, register, UNBOUND);
    runtime.heap.set_field(frame, register + 2, UNBOUND);
    let result = runtime.heap.int(i64::MAX);
    runtime.heap.set_field(frame, 1, result);
    let edges = ["failure", "waiter", "awaiting"].map(|s| runtime.heap.allocate_text(s));
    for (index, edge) in edges.into_iter().enumerate() {
        runtime.heap.set_field(frame, 2 + index, edge);
    }
    for _ in 0..3 {
        runtime.heap.collect(|visit| visit(frame, Repr::Ref));
        assert_eq!(runtime.heap.integer(runtime.heap.field(frame, retained)).to_string(), i64::MIN.to_string());
        assert_eq!(runtime.heap.text(runtime.heap.field(frame, retained + 1)), "retained argument");
        assert_eq!(runtime.heap.integer(runtime.heap.field(frame, 1)).to_string(), i64::MAX.to_string());
        for (index, expected) in ["failure", "waiter", "awaiting"].into_iter().enumerate() {
            assert_eq!(runtime.heap.text(runtime.heap.field(frame, 2 + index)), expected);
        }
    }
}

#[test]
fn gc_layout_driver_state_and_runtime_object_overrides() {
    // Reuse the suspended-Event/ownership regression instead of duplicating it.
    let program = okkama_sparrow::assemble(include_str!("all_futures.spa")).unwrap();
    let loaded = Loaded::verified(program).unwrap();
    let layouts = loaded.layouts();
    assert_eq!(layouts.objects[0], [Repr::Ref, Repr::Raw]); // Atomic<string>
    assert_eq!(layouts.objects[6], [Repr::Raw, Repr::Raw, Repr::Ref]); // Event
    let driver = loaded.program.functions.iter().position(|f| f.symbol == "Event.wait").unwrap();
    assert!(loaded.functions[driver].driver);
    // Fixed words: state … flags; then the register, the driver slot, and in a program with resources the
    // ownership regions (owned #581, inputs #587; #932), the retained event and the identity (#931).
    let register = (sparrow_core::tasks::REGISTERS / 8 - 1) as usize;
    assert!(loaded.functions[driver].identity_at.is_some(), "the fixture has resources");
    assert_eq!(&layouts.frames[driver][register..register + 5], &[Repr::Ref; 5]);
    assert_eq!(layouts.frames[driver].len(), register + 5 + 2);
    assert!(layouts.frames[driver][register + 5..].iter().all(|&repr| repr == Repr::Raw));
    let mut runtime = sparrow_core::runtime::Runtime::new(loaded).unwrap();
    runtime.heap.set_stress(true);
    let waiter = runtime.heap.allocate_text("event waiter");
    let event = runtime.heap.object_new(6, &[0, 0, waiter]);
    let frame = runtime.future_new(driver, &[event]);
    let state = runtime.heap.allocate_text("driver state");
    runtime.heap.set_field(frame, register, UNBOUND); // register reused
    runtime.heap.set_field(frame, register + 1, state); // driver slot, before retained inputs
    for _ in 0..3 {
        runtime.heap.collect(|visit| visit(frame, Repr::Ref));
        assert_eq!(runtime.heap.text(runtime.heap.field(frame, register + 1)), "driver state");
        assert_eq!(runtime.heap.field(frame, register + 4), event, "the retained event, after the regions");
        assert_eq!(runtime.heap.text(runtime.heap.field(event, 2)), "event waiter");
    }

    // An Atomic<int> is declared fieldless too, but its hidden value can be BigInt.
    let program = okkama_sparrow::assemble(
        "sparrow 18\nmodule \"gc-layout-atomic\" \"1.0.0\"\n\
         type t0 int\ntype t1 object obj0\nobject obj0 \"Atomic<int>\" {}\n\
         function f0 \"new\" intrinsic \"okkama.atomic.Atomic.new_value\" [obj0] [] {\n\
         registers [t0]\nparams [r0]\nresult t1\n}\n",
    ).unwrap();
    let loaded = Loaded::verified(program).unwrap();
    let layouts = loaded.layouts();
    assert_eq!(layouts.objects[0], [Repr::Int, Repr::Raw]);
    let mut heap = Heap::new(layouts);
    heap.set_stress(true);
    let big = heap.int(i64::MAX);
    let atomic = heap.object_new(0, &[big, 0]);
    heap.collect(|visit| visit(atomic, Repr::Ref));
    assert_eq!(heap.integer(heap.field(atomic, 0)).to_string(), i64::MAX.to_string());
}

#[test]
fn gc_layout_bounded_fixture_census() {
    for (width, count) in [(8, 16), (256, 128)] {
        for references in [false, true] {
            let mut layout = vec![if references { Repr::Ref } else { Repr::Raw }; width];
            layout[0] = Repr::Ref;
            let mut heap = Heap::new(Layouts { objects: vec![layout], ..Layouts::default() });
            heap.set_stress(sparrow_core::env_switch::enabled("SPARROW_CORE_GC_STRESS"));
            heap.allocations = Some(Box::default());
            let child = heap.allocate_text("shared");
            let mut fields = vec![0; width];
            fields[0] = child;
            let roots: Vec<_> = (0..count).map(|_| heap.object_new(0, &fields)).collect();
            for _ in 0..3 {
                heap.collect(|visit| roots.iter().for_each(|&root| visit(root, Repr::Ref)));
                assert_eq!(heap.text(child), "shared");
                let allocations = heap.allocations.as_ref().unwrap();
                assert_eq!(allocations.kinds[&("object", 0)].0, count as u64);
                assert_eq!(allocations.live[&("object", 0)].0, count as u64);
                assert_eq!(allocations.live[&("string", 0)].0, 1);
                // The live census charges size classes; allocation counters
                // report requested bytes, so these byte totals can differ.
                assert_eq!(allocations.live.values().map(|&(_, bytes)| bytes).sum::<u64>(), heap.stats().live_bytes as u64);
            }
            let stats = heap.stats();
            eprintln!("gc-layout width={width} objects={count} references={references} allocations={:?} allocated={} live={} external_allocated={} external_live={}",
                heap.allocations.as_ref().unwrap().kinds, stats.allocated_bytes, stats.live_bytes,
                stats.external_allocated_bytes, stats.external_live_bytes);
            heap.collect(|_| {});
            assert_eq!(heap.stats().live_bytes, 0);
        }
    }
}
