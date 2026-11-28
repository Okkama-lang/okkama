//! Value ABI oracles, including callbacks that deliberately violate the contract.
use super::*;
use num_bigint::BigInt;
use std::ffi::CStr;

fn custom(signature: &'static CStr, callback: okk_sync, params: &[usize], result: usize) -> Box<Runtime> {
    let export = Box::leak(Box::new(okk_export { symbol: c"values@0::test/1".as_ptr(), signature: signature.as_ptr(),
        params: c"".as_ptr(), sync: Some(callback), r#async: ptr::null() }));
    // Every test native has one input, including constructors driven by an int.
    let descriptor = Box::leak(Box::new(okk_extension { exports: export, export_count: 1, ..DESCRIPTOR }));
    let limits = Limits { extensions: vec![unsafe { Extension::from_static(descriptor).unwrap() }], ..Limits::default() };
    Box::new(Runtime::with_limits(Loaded::new(wrapper("values@0::test/1", params, result)), limits).unwrap())
}

macro_rules! ok {
    ($e:expr) => { { let status = $e; if status != OKK_OK { return status; } } };
}
macro_rules! require {
    ($e:expr) => { if !$e { return OKK_PANIC; } };
}

unsafe fn copy(c: *mut okk_call, value: okk_slot, out: *mut okk_slot) -> u32 {
    unsafe {
        let h = &*(*c).host;
        match value.kind {
            OKK_INT | OKK_BIGINT => {
                let mut sign = 0; let mut data = ptr::null(); let mut len = 0;
                ok!((h.bigint_read)(c, value, &mut sign, &mut data, &mut len));
                ok!((h.bigint_new)(c, sign, data, len, out));
                require!((*out).kind == value.kind);
            }
            OKK_RAT => {
                let mut n = okk_slot::default(); let mut d = n;
                ok!((h.rat_parts)(c, value, &mut n, &mut d));
                ok!(copy(c, n, &mut n)); ok!(copy(c, d, &mut d));
                ok!((h.rat_new)(c, n, d, out));
            }
            OKK_STRING => {
                let mut data = ptr::null(); let mut len = 0;
                ok!((h.text)(c, value, &mut data, &mut len));
                ok!((h.text_new)(c, data, len, out));
            }
            OKK_ARRAY => {
                let mut len = 0; ok!((h.array_len)(c, value, &mut len));
                let mut slots = vec![okk_slot::default(); len];
                ok!((h.array_read)(c, value, 0, slots.as_mut_ptr(), len));
                for (i, item) in slots.iter_mut().enumerate() {
                    let mut single = okk_slot::default(); ok!((h.array_get)(c, value, i, &mut single));
                    require!(single.kind == item.kind);
                    ok!(copy(c, *item, item));
                }
                ok!((h.array_new)(c, slots.as_ptr(), slots.len(), out));
            }
            OKK_MAP => {
                let mut len = 0; ok!((h.map_len)(c, value, &mut len));
                let mut keys = vec![okk_slot::default(); len]; let mut values = keys.clone();
                ok!((h.map_entries)(c, value, keys.as_mut_ptr(), values.as_mut_ptr(), len));
                let mut previous = Vec::new();
                for key in &keys {
                    if key.kind != OKK_STRING { continue; }
                    let mut data = ptr::null(); let mut size = 0;
                    ok!((h.text)(c, *key, &mut data, &mut size));
                    let text = std::slice::from_raw_parts(data, size);
                    require!(previous.as_slice() <= text);
                    previous = text.to_vec();
                }
                for item in keys.iter_mut().chain(values.iter_mut()) { ok!(copy(c, *item, item)); }
                ok!((h.map_new)(c, keys.as_ptr(), values.as_ptr(), len, out));
            }
            _ => *out = value,
        }
        OKK_OK
    }
}
unsafe extern "C" fn roundtrip(c: *mut okk_call, args: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe { copy(c, *args, out) }
}
fn big(r: &Runtime, word: Word) -> BigInt {
    match r.heap.integer(word) { crate::heap::Integer::Small(n) => n.into(), crate::heap::Integer::Big(n) => n.clone() }
}

#[test]
fn ext_values_bigint_read_new_normalize_both_directions() {
    for n in [BigInt::from(1u64 << 62), i64::MAX.into(), i64::MIN.into(), BigInt::from(1u64 << 63),
        BigInt::from(1u128 << 64), -BigInt::from(1u128 << 64), BigInt::parse_bytes("7".repeat(300).as_bytes(), 10).unwrap()] {
        let mut r = custom(c"(int) -> int", roundtrip, &[0], 0);
        let arg = r.heap.big(n.clone());
        let result = r.run_function_with(1, &[arg], 1 << 24).unwrap();
        assert_eq!(big(&r, result), n);
    }
}

#[test]
fn ext_values_rat_big_parts_and_reduction() {
    use okkama_runtime_core::{IntValue, rat::Rat};
    let n = IntValue::from_decimal_digits(&"8".repeat(300)).unwrap();
    let d = IntValue::from_decimal_digits(&"6".repeat(200)).unwrap();
    let ratio = Rat::new(&n, &d).unwrap();
    let mut r = custom(c"(rat) -> rat", roundtrip, &[9], 9);
    let arg = r.rational_new(ratio.clone());
    let result = r.run_function_with(1, &[arg], 1 << 24).unwrap();
    assert_eq!(r.rational(result), &ratio);
}

#[test]
fn ext_values_arrays_and_nested_maps_survive_collection() {
    let mut r = custom(c"(@string) -> @string", roundtrip, &[10], 10);
    let a = r.heap.allocate_text("🐦"); let b = r.heap.allocate_text("second");
    let arg = r.heap.allocate_array(Repr::Ref, &[a, b]);
    let result = r.run_function_with(1, &[arg], 1 << 24).unwrap();
    assert_eq!(r.heap.elements(result).iter().map(|w| r.heap.text(*w)).collect::<Vec<_>>(), ["🐦", "second"]);

    let mut r = custom(c"(@%string:@int) -> @%string:@int", roundtrip, &[13], 13);
    let mut map = crate::ops::MapValue { key: Repr::Ref, value: Repr::Ref, entries: Default::default(), owned: Default::default() };
    for (key, n) in [("z", 29), ("a", 17)] {
        let k = r.heap.allocate_text(key); let n = r.heap.int(n);
        let v = r.heap.allocate_array(Repr::Int, &[n]);
        map.entries.insert(crate::ops::Key::text(key.into()), (k, v));
    }
    let m = r.heap.allocate_payload(Box::new(map));
    let arg = r.heap.allocate_array(Repr::Ref, &[m]);
    let result = r.run_function_with(1, &[arg], 1 << 24).unwrap();
    let m = r.heap.elements(result)[0];
    let entries = &r.heap.payload::<crate::ops::MapValue>(m).unwrap().entries;
    let actual = entries.values().map(|&(k, v)| (r.heap.text(k), big(&r, r.heap.elements(v)[0]))).collect::<Vec<_>>();
    assert_eq!(actual, [("a", 17.into()), ("z", 29.into())]);
}

#[test]
fn ext_values_maybe_some_none_both_directions() {
    for some in [true, false] {
        let mut r = custom(c"(?int) -> ?int", roundtrip, &[14], 14);
        let n = r.heap.big(BigInt::from(1u128 << 64));
        let arg = if some { r.heap.object_new(1, &[n]) } else { r.heap.object_new(2, &[]) };
        let result = r.run_function_with(1, &[arg], 1 << 24).unwrap();
        assert_eq!(crate::heap::payload(r.heap.header_of(result)), if some { 1 } else { 2 });
        if some { assert_eq!(big(&r, r.heap.field(result, 0)), BigInt::from(1u128 << 64)); }

        let mut r = custom(c"(?@int) -> ?@int", roundtrip, &[15], 15);
        let n = r.heap.int(17); let array = r.heap.allocate_array(Repr::Int, &[n]);
        let arg = if some { r.heap.object_new(3, &[array]) } else { r.heap.object_new(4, &[]) };
        let result = r.run_function_with(1, &[arg], 1 << 24).unwrap();
        assert_eq!(crate::heap::payload(r.heap.header_of(result)), if some { 3 } else { 4 });
        if some { assert_eq!(big(&r, r.heap.elements(r.heap.field(result, 0))[0]), 17.into()); }
    }
}

unsafe extern "C" fn reserve(c: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe {
        let h = &*(*c).host; let mut data = ptr::null_mut();
        ok!((h.bytes_reserve)(c, 3, &mut data, out));
        let mut garbage = okk_slot::default();
        for _ in 0..20 { ok!((h.text_new)(c, b"garbage".as_ptr(), 7, &mut garbage)); }
        std::ptr::copy_nonoverlapping([0, 128, 255].as_ptr(), data, 3);
    }
    OKK_OK
}
#[test]
fn ext_values_bytes_reserve_survives_allocations() {
    let mut r = custom(c"(int) -> @byte", reserve, &[0], 6);
    let n = r.heap.int(0);
    let result = r.run_function_with(1, &[n], 1 << 24).unwrap();
    assert_eq!(&*r.heap.byte_view(result), &[0, 128, 255]);
}

unsafe extern "C" fn data_errors(c: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe {
        let h = &*(*c).host;
        let n = okk_slot { bits: 6, kind: OKK_INT }; let zero = okk_slot { bits: 0, kind: OKK_INT };
        require!((h.rat_new)(c, n, zero, out) == OKK_RANGE);
        require!((h.bigint_new)(c, 2, [1].as_ptr(), 1, out) == OKK_RANGE);
        require!((h.bigint_new)(c, 0, [1].as_ptr(), 1, out) == OKK_RANGE);
        require!((h.text_new)(c, [255].as_ptr(), 1, out) == OKK_BAD_TEXT);
        require!((h.map_new)(c, [n, n].as_ptr(), [n, n].as_ptr(), 2, out) == OKK_RANGE);
        let d = okk_slot { bits: (-8i64) as u64, kind: OKK_INT };
        ok!((h.rat_new)(c, n, d, out));
        let mut a = okk_slot::default(); let mut b = a;
        ok!((h.rat_parts)(c, *out, &mut a, &mut b));
        require!(a == okk_slot { bits: (-3i64) as u64, kind: OKK_INT });
        require!(b == okk_slot { bits: 4, kind: OKK_INT });
        ok!((h.array_new)(c, ptr::null(), 0, out));
        require!((h.array_get)(c, *out, 0, &mut a) == OKK_RANGE);
        require!((h.array_read)(c, *out, usize::MAX, &mut a, 1) == OKK_RANGE);
        *out = n;
    }
    OKK_OK
}
#[test]
fn ext_values_data_errors_are_recoverable() {
    let mut r = custom(c"(int) -> int", data_errors, &[0], 0);
    let n = r.heap.int(0);
    let result = r.run_function_with(1, &[n], 1 << 24).unwrap();
    assert_eq!(big(&r, result), 6.into()); assert!(!r.poisoned);
}

unsafe extern "C" fn mixed(c: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe {
        let slots = [okk_slot { bits: 1, kind: OKK_INT }, okk_slot { bits: 1, kind: OKK_BOOL }];
        OBSERVED.with(|s| s.set(((*(*c).host).array_new)(c, slots.as_ptr(), 2, out)));
        *out = slots[0];
    }
    OKK_OK
}
unsafe extern "C" fn ignored_bad_token(c: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe {
        let mut len = 0;
        OBSERVED.with(|s| s.set(((*(*c).host).array_len)(c, okk_slot { bits: u64::MAX, kind: OKK_ARRAY }, &mut len)));
        *out = okk_slot { bits: 0, kind: OKK_INT };
    }
    OKK_OK
}
#[test]
fn ext_values_ignored_contract_fault_still_poisons() {
    for (callback, expected) in [(mixed as okk_sync, OKK_BAD_SHAPE), (ignored_bad_token, OKK_BAD_TOKEN)] {
        OBSERVED.with(|s| s.set(999));
        let mut r = custom(c"(int) -> int", callback, &[0], 0);
        let n = r.heap.int(0);
        assert!(r.run_function_with(1, &[n], 1 << 24).unwrap_err().0.contains("internal error in native"));
        assert!(r.poisoned);
        assert_eq!(OBSERVED.with(|s| s.get()), expected);
    }
}
thread_local! { static OBSERVED: std::cell::Cell<u32> = const { std::cell::Cell::new(999) }; }

unsafe extern "C" fn empty(c: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe { ((*(*c).host).array_new)(c, ptr::null(), 0, out) }
}
#[test]
fn ext_values_empty_array_fits_every_element_shape() {
    for (sig, ty) in [(c"(int) -> @int", 11), (c"(int) -> @string", 10), (c"(int) -> @rat", 16),
        (c"(int) -> @@int", 17), (c"(int) -> @byte", 6), (c"(int) -> @?int", 19)] {
        let mut r = custom(sig, empty, &[0], ty); let n = r.heap.int(0);
        let result = r.run_function_with(1, &[n], 1 << 24).unwrap();
        assert_eq!(r.heap.array_length(result), 0);
    }
}

unsafe extern "C" fn memory(c: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe {
        let h = &*(*c).host; let mut data = ptr::null_mut();
        require!((h.bytes_reserve)(c, 1 << 20, &mut data, out) == OKK_STOPPED);
        require!((h.text_new)(c, b"x".as_ptr(), 1, out) == OKK_STOPPED);
        *out = okk_slot { bits: 0, kind: OKK_INT };
    }
    OKK_OK
}
#[test]
fn ext_values_memory_stop_is_sticky_and_does_not_poison() {
    let mut r = custom(c"(int) -> int", memory, &[0], 0);
    r.limits.memory_scope = crate::budget::MemoryScope::Runtime;
    r.limits.memory = Some(r.heap.held_bytes() as u64 + 16384);
    let n = r.heap.int(0);
    let error = r.run_function_with(1, &[n], 1 << 24).unwrap_err();
    assert!(error.0.contains("budget exhausted: memory"), "{}", error.0);
    assert!(!r.poisoned);
}

unsafe extern "C" fn nonfinite(_: *mut okk_call, args: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe { *out = okk_slot { bits: match (*args).bits { 0 => f64::NAN, 1 => f64::INFINITY, _ => f64::NEG_INFINITY }.to_bits(), kind: OKK_FLOAT }; }
    OKK_OK
}
#[test]
fn ext_values_nonfinite_results_refused() {
    for i in 0..3 {
        let mut r = custom(c"(int) -> float", nonfinite, &[0], 1); let n = r.heap.int(i);
        let error = r.run_function_with(1, &[n], 1 << 24).unwrap_err();
        assert!(error.0.contains("not_finite"), "{}", error.0); assert!(!r.poisoned);
    }
}

#[test]
fn ext_values_arrays_of_maybe_keep_none_distinct_from_some() {
    let mut r = custom(c"(@?int) -> @?int", roundtrip, &[19], 19);
    let n = r.heap.int(17); let some = r.heap.object_new(1, &[n]); let none = r.heap.object_new(2, &[]);
    let arg = r.heap.allocate_array(Repr::Ref, &[some, none]);
    let result = r.run_function_with(1, &[arg], 1 << 24).unwrap();
    let words = r.heap.elements(result);
    assert_eq!(crate::heap::payload(r.heap.header_of(words[0])), 1);
    assert_eq!(big(&r, r.heap.field(words[0], 0)), 17.into());
    assert_eq!(crate::heap::payload(r.heap.header_of(words[1])), 2);
}

#[test]
fn ext_values_wrong_result_shapes_and_scalar_ranges_are_faults() {
    for (signature, params, result, value) in [
        (c"(int) -> int", 0, 0, okk_slot { bits: f64::NAN.to_bits(), kind: OKK_FLOAT }),
        (c"(int) -> bool", 0, 2, okk_slot { bits: 2, kind: OKK_BOOL }),
        (c"(int) -> byte", 0, 3, okk_slot { bits: 256, kind: OKK_BYTE }),
        (c"(int) -> char", 0, 4, okk_slot { bits: 0xd800, kind: OKK_CHAR }),
        (c"(int) -> char", 0, 4, okk_slot { bits: u64::MAX, kind: OKK_CHAR }),
        (c"(int) -> ?int", 0, 14, okk_slot { bits: 1, kind: OKK_NONE }),
    ] {
        RETURN.with(|s| s.set(value));
        let mut r = custom(signature, return_slot, &[params], result); let n = r.heap.int(0);
        assert!(r.run_function_with(1, &[n], 1 << 24).unwrap_err().0.contains("internal error in native"));
        assert!(r.poisoned);
    }
    let mut r = custom(c"(@bool) -> @int", echo, &[20], 11);
    let arg = r.heap.allocate_array(Repr::Raw, &[1]);
    assert!(r.run_function_with(1, &[arg], 1 << 24).unwrap_err().0.contains("internal error in native"));
    assert!(r.poisoned);
}
thread_local! { static RETURN: std::cell::Cell<okk_slot> = const { std::cell::Cell::new(okk_slot { bits: 0, kind: 0 }) }; }
unsafe extern "C" fn return_slot(_: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe { *out = RETURN.with(|s| s.get()); } OKK_OK
}

unsafe extern "C" fn nested_empty(c: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe {
        let h = &*(*c).host; let mut empty = okk_slot::default();
        ok!((h.array_new)(c, ptr::null(), 0, &mut empty));
        let mut p = ptr::null(); let mut len = 9;
        ok!((h.bytes)(c, empty, &mut p, &mut len)); require!(len == 0);
        let mut array = okk_slot::default();
        ok!((h.array_new)(c, [okk_slot { bits: 17, kind: OKK_INT }].as_ptr(), 1, &mut array));
        let sentinel = okk_slot { bits: 123, kind: 123 }; let mut buffer = [sentinel; 2];
        require!((h.array_read)(c, array, 0, buffer.as_mut_ptr(), 2) == OKK_RANGE);
        require!(buffer == [sentinel; 2]);
        ok!((h.array_new)(c, [empty, array, empty].as_ptr(), 3, out));
    }
    OKK_OK
}
#[test]
fn ext_values_nested_empty_arrays_and_checked_bulk_ranges() {
    let mut r = custom(c"(int) -> @@int", nested_empty, &[0], 17); let n = r.heap.int(0);
    let result = r.run_function_with(1, &[n], 1 << 24).unwrap();
    let arrays = r.heap.elements(result);
    assert_eq!(r.heap.array_length(arrays[0]), 0);
    assert_eq!(big(&r, r.heap.elements(arrays[1])[0]), 17.into());
    assert_eq!(r.heap.array_length(arrays[2]), 0);
}

#[test]
fn ext_values_rat_map_keys_use_value_order_and_equality() {
    use okkama_runtime_core::{IntValue, rat::Rat};
    let mut r = custom(c"(%rat:string) -> %rat:string", roundtrip, &[21], 21);
    let mut map = crate::ops::MapValue { key: Repr::Ref, value: Repr::Ref, entries: Default::default(), owned: Default::default() };
    for n in [3, -1, 2] {
        let rat = Rat::new(&IntValue::from_i64(n), &IntValue::from_i64(2)).unwrap();
        let key = r.rational_new(rat.clone()); let value = r.heap.allocate_text(&n.to_string());
        map.entries.insert(crate::ops::Key::Rat(std::rc::Rc::new(rat)), (key, value));
    }
    let arg = r.heap.allocate_payload(Box::new(map));
    let result = r.run_function_with(1, &[arg], 1 << 24).unwrap();
    let map = r.heap.payload::<crate::ops::MapValue>(result).unwrap();
    assert_eq!(map.entries.values().map(|&(_, v)| r.heap.text(v)).collect::<Vec<_>>(), ["-1", "2", "3"]);
}

unsafe extern "C" fn allocate_until_stop(c: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe {
        let h = &*(*c).host;
        for _ in 0..1024 {
            let status = (h.text_new)(c, [b'x'; 1024].as_ptr(), 1024, out);
            if status == OKK_STOPPED {
                require!((h.bytes_new)(c, ptr::null(), 0, out) == OKK_STOPPED);
                return OKK_FAILED;
            }
            if status != OKK_OK { return status; }
        }
    }
    OKK_PANIC
}
#[test]
fn ext_values_many_allocations_stop_even_if_callback_returns_failed() {
    let mut r = custom(c"(int) -> string", allocate_until_stop, &[0], 5);
    r.limits.memory_scope = crate::budget::MemoryScope::Runtime;
    r.limits.memory = Some(r.heap.held_bytes() as u64 + 65536);
    let n = r.heap.int(0);
    assert!(r.run_function_with(1, &[n], 1 << 24).unwrap_err().0.contains("budget exhausted: memory"));
    assert!(!r.poisoned);
}

#[test]
fn ext_values_numeric_arrays_and_all_some_maybe_arrays() {
    let mut r = custom(c"(@int) -> @int", roundtrip, &[11], 11);
    let ns = [BigInt::from(17), BigInt::from(1u128 << 64), -BigInt::from(1u128 << 64)];
    let words = ns.iter().map(|n| r.heap.big(n.clone())).collect::<Vec<_>>();
    let arg = r.heap.allocate_array(Repr::Int, &words);
    let result = r.run_function_with(1, &[arg], 1 << 24).unwrap();
    assert_eq!(r.heap.elements(result).iter().map(|w| big(&r, *w)).collect::<Vec<_>>(), ns);

    let mut r = custom(c"(@?int) -> @?int", roundtrip, &[19], 19);
    let n = r.heap.int(17); let some = r.heap.object_new(1, &[n]);
    let arg = r.heap.allocate_array(Repr::Ref, &[some]);
    let result = r.run_function_with(1, &[arg], 1 << 24).unwrap();
    assert_eq!(big(&r, r.heap.field(r.heap.elements(result)[0], 0)), 17.into());

    use okkama_runtime_core::{IntValue, rat::Rat};
    let mut r = custom(c"(@rat) -> @rat", roundtrip, &[16], 16);
    let ratio = Rat::new(&IntValue::from_i64(2), &IntValue::from_i64(3)).unwrap();
    let word = r.rational_new(ratio.clone()); let arg = r.heap.allocate_array(Repr::Ref, &[word]);
    let result = r.run_function_with(1, &[arg], 1 << 24).unwrap();
    assert_eq!(r.rational(r.heap.elements(result)[0]), &ratio);
}

unsafe extern "C" fn temporary_shapes(c: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe {
        let mut value = okk_slot { bits: 17, kind: OKK_INT };
        for _ in 0..32 {
            let input = value;
            ok!(((*(*c).host).array_new)(c, &input, 1, &mut value));
        }
        *out = okk_slot { bits: 0, kind: OKK_INT };
    }
    OKK_OK
}
#[test]
fn ext_values_dynamic_shapes_expire_with_the_call() {
    let mut r = custom(c"(int) -> int", temporary_shapes, &[0], 0);
    let count = r.host.as_ref().unwrap().shapes.len();
    for _ in 0..3 {
        let n = r.heap.int(0);
        r.run_function_with(1, &[n], 1 << 24).unwrap();
        assert_eq!(r.host.as_ref().unwrap().shapes.len(), count);
        assert_eq!(r.host.as_ref().unwrap().shape_index.len(), count);
    }
}
