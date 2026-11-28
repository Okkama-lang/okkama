//! Shared by libFuzzer and sparrow-core's deterministic seed replay.
#![allow(unsafe_code)]
#[cfg(test)]
use crate as sparrow_core;
use okkama_runtime_abi::ext::*;
use std::{cell::RefCell, ptr};

thread_local! {
    static INPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static STALE: RefCell<okk_slot> = const { RefCell::new(okk_slot { bits: 0, kind: OKK_STRING }) };
}

unsafe extern "C" fn hostile(c: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    INPUT.with(|input| unsafe {
        let input = input.borrow();
        let h = &*(*c).host;
        let mut slots = [okk_slot { bits: 0, kind: OKK_INT }; 16];
        (h.text_new)(c, b"seed".as_ptr(), 4, &mut slots[1]);
        (h.bytes_new)(c, b"bytes".as_ptr(), 5, &mut slots[2]);
        (h.bigint_new)(c, 1, [255; 16].as_ptr(), 16, &mut slots[3]);
        (h.rat_new)(c, okk_slot { bits: 6, kind: OKK_INT }, okk_slot { bits: 8, kind: OKK_INT }, &mut slots[4]);
        (h.array_new)(c, slots.as_ptr(), 1, &mut slots[5]);
        (h.map_new)(c, slots.as_ptr(), slots.as_ptr(), 1, &mut slots[6]);
        slots[7] = STALE.with(|s| s.replace(slots[1]));
        let mut status = OKK_OK;
        // Bound work, not scalar/token bit patterns. Foreign pointers must still
        // address their advertised buffers; arbitrary addresses cannot be sandboxed.
        for chunk in input.get(1..).unwrap_or_default().chunks(24).take(64) {
            let mut packet = [0u8; 24]; packet[..chunk.len()].copy_from_slice(chunk);
            let bits = u64::from_le_bytes(packet[4..12].try_into().unwrap());
            let kind = u64::from_le_bytes(packet[12..20].try_into().unwrap());
            let arbitrary = okk_slot { bits, kind };
            let slot = if packet[1] & 1 == 0 { arbitrary } else { slots[packet[2] as usize % slots.len()] };
            let other = slots[packet[3] as usize % slots.len()];
            let mut result = okk_slot::default();
            let mut len = 0; let mut sign = 0; let mut view = ptr::null(); let mut buffer = ptr::null_mut();
            let mut keys = [okk_slot::default(); 16]; let mut values = keys;
            let mut scalar = 0;
            let count = packet[20] as usize % 17;
            let bytes = &packet[..count];
            status = match packet[0] % 24 {
                0 => { result = arbitrary; u32::from_le_bytes(packet[20..24].try_into().unwrap()) },
                1 => (h.text)(c, slot, &mut view, &mut len),
                2 => (h.bytes)(c, slot, &mut view, &mut len),
                3 => (h.text_new)(c, bytes.as_ptr(), bytes.len(), &mut result),
                4 => (h.bytes_new)(c, bytes.as_ptr(), bytes.len(), &mut result),
                5 => {
                    let n = if packet[22] == 255 { usize::MAX } else if packet[22] == 254 { 1 << 24 } else { count };
                    let s = (h.bytes_reserve)(c, n, &mut buffer, &mut result);
                    if s == OKK_OK && n > 0 { ptr::write_bytes(buffer, packet[23], n); }
                    s
                }
                6 => (h.bigint_read)(c, slot, &mut sign, &mut view, &mut len),
                7 => (h.bigint_new)(c, packet[22] as i8 as i32, bytes.as_ptr(), bytes.len(), &mut result),
                8 => (h.rat_parts)(c, slot, &mut result, &mut values[0]),
                9 => (h.rat_new)(c, slot, other, &mut result),
                10 => (h.array_len)(c, slot, &mut len),
                11 => (h.array_get)(c, slot, bits as usize, &mut result),
                12 => (h.array_read)(c, slot, bits as usize, keys.as_mut_ptr(), count),
                13 => { keys[0] = slot; keys[1] = other; (h.array_new)(c, keys.as_ptr(), count, &mut result) },
                14 => (h.map_len)(c, slot, &mut len),
                15 => (h.map_entries)(c, slot, keys.as_mut_ptr(), values.as_mut_ptr(), count),
                16 => { keys[0] = slot; keys[1] = other; (h.map_new)(c, keys.as_ptr(), keys.as_ptr(), count, &mut result) },
                17 => (h.fail)(c, bytes.as_ptr(), bytes.len(), bytes.as_ptr(), bytes.len()),
                18 => (h.interrupted)(c, &mut scalar),
                19 => (h.text_new)(c, ptr::null(), bits as usize, &mut result),
                20 => (h.array_new)(c, ptr::null(), bits as usize, &mut result),
                21 => (h.array_len)(c, slot, ptr::null_mut()),
                22 => { result = slot; OKK_OK },
                _ => (h.bigint_read)(c, slot, ptr::null_mut(), &mut view, &mut len),
            };
            slots[8 + packet[23] as usize % 8] = result;
            *out = result;
        }
        match input.first().copied().unwrap_or(0) % 4 {
            0 => status, 1 => OKK_OK, 2 => OKK_FAILED, _ => input.get(1).copied().unwrap_or(255) as u32,
        }
    })
}

static EXPORT: okk_export = okk_export { symbol: c"fuzz/ext@0::hostile/0".as_ptr(), signature: c"() -> int".as_ptr(),
    params: c"".as_ptr(), sync: Some(hostile), r#async: ptr::null() };
static EXTENSION: okk_extension = okk_extension { abi: OKKAMA_EXT_ABI, size: std::mem::size_of::<okk_extension>() as u32,
    name: c"hostile".as_ptr(), exports: &EXPORT, export_count: 1, states: ptr::null(), state_count: 0 };

pub fn exercise(data: &[u8]) {
    let program = okkama_sparrow::assemble(r#"sparrow 19
module "ext_fuzz" "1.0.0"
type t0 int
type t1 str
type t2 object obj0
object obj0 "NativeFailure" builtin {
 field "domain" t1
 field "code" t1
 field "operation" t1
 field "message" t1
}
function f0 "hostile" native "fuzz/ext@0::hostile/0" {
 registers []
 params []
 result t0
}
function f1 "main" {
 registers [t0]
 params []
 result t0
 0: call r0, f0, []
 1: ret r0
}
entry f1
"#).unwrap();
    INPUT.with(|input| { let mut input = input.borrow_mut(); input.clear(); input.extend_from_slice(&data[..data.len().min(1537)]); });
    let limits = sparrow_core::budget::Limits {
        memory: Some(2 << 20), memory_scope: sparrow_core::budget::MemoryScope::Runtime,
        steps: Some(1000), extensions: vec![unsafe { sparrow_core::ext::Extension::from_static(&EXTENSION).unwrap() }],
        ..Default::default()
    };
    let _ = sparrow_core::run_limited(okkama_sparrow::encode(&program).unwrap(), vec![], false, vec![], limits);
}
