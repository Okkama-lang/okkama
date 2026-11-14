use super::layout::*;
use super as s;

fn facts(has_destroy: bool) -> LayoutFacts {
    LayoutFacts { has_destroy, may_reach_resource: has_destroy, may_async_destroy: false }
}

#[test]
fn champ_registry_builds_destroy_layout_and_rejects_inconsistent_inputs() {
    let declaration = 0;
    let mut registry = RuntimeLayoutRegistry::default();
    registry.register(declaration, vec![s::Type::Str, s::Type::Int], vec![0], facts(true), vec![0]).unwrap();
    let layout = registry.object_layout(declaration).unwrap().runtime;
    assert_eq!(layout.allocation_words, 7);
    assert_eq!(layout.identity_offset, Some(24));
    assert_eq!(layout.trace_layout, vec![0]);
    assert_eq!(layout.selection_layout, vec![0]);
    let runtime = ObjectRuntimeLayout::from_spb17(&layout).unwrap();
    assert_eq!(runtime.trace_layout().len(), 2, "GC must also trace tagged BigInt storage");
    for bad in [
        s::ObjectRuntimeLayout { allocation_words: 3, ..layout.clone() },
        s::ObjectRuntimeLayout { identity_offset: Some(32), ..layout.clone() },
        s::ObjectRuntimeLayout { trace_layout: vec![0, 2], ..layout.clone() },
        s::ObjectRuntimeLayout { selection_layout: vec![2], ..layout.clone() },
        s::ObjectRuntimeLayout { may_async_destroy: true, ..layout.clone() },
    ] {
        assert!(ObjectRuntimeLayout::from_spb17(&bad).is_err());
    }
    let mut other = RuntimeLayoutRegistry::default();
    assert!(other.register(declaration, vec![s::Type::Bool], vec![1], facts(false), vec![]).is_err());
    assert!(ObjectRuntimeLayout::new(vec![], LayoutFacts {
        has_destroy: true, ..LayoutFacts::default()
    }, vec![]).is_err());
}

#[test]
fn champ_layout_sizes_offsets_and_header() {
    for count in 0..=4 {
        let plain = ObjectRuntimeLayout::new(vec![Repr::Raw; count], facts(false), vec![]).unwrap();
        let own = ObjectRuntimeLayout::new(vec![Repr::Raw; count], facts(true), vec![]).unwrap();
        assert_eq!(plain.storage_field_count(), count);
        assert_eq!(plain.allocation_words(), (count + 1).max(2));
        assert_eq!(plain.identity_offset(), None);
        assert_eq!(own.identity_offset(), Some(8 * (1 + count)));
        assert_eq!(own.allocation_words(), count + 5);
        for index in 0..count {
            assert_eq!(own.field_offset(index), 8 * (index + 1));
        }
        assert_eq!(own.header(27), u64::from(OBJECT_KIND) | (1 << 9) | (27 << 32) | HAS_IDENTITY);
        assert_eq!(own.header(27) & (7 << 18), 0);
    }
}
