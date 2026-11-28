//! Static extension seam and the dispatch behaviour it must preserve.
#![allow(unsafe_code)]

use crate::{program::Loaded, runtime::Runtime};

fn program(symbol: &str) -> okkama_sparrow::Program {
    okkama_sparrow::assemble(&format!(r#"sparrow 19
module "extension_test" "1.0.0"
type t0 int
function f0 "native" native "{symbol}" {{
 registers [t0, t0]
 params [r0, r1]
 result t0
}}
function f1 "main" {{
 registers [t0, t0, t0]
 params []
 result t0
 0: const r0, int 17
 1: const r1, int 29
 2: call r2, f0, [r0, r1]
 3: ret r2
}}
entry f1
"#)).unwrap()
}

#[test]
fn ext_characterization_missing_provider() {
    let mut runtime = Box::new(Runtime::new(Loaded::new(program("example/native@0::missing/2"))).unwrap());
    let error = runtime.run_entry(1 << 24).unwrap_err();
    assert!(error.0.contains("Native symbol \"example/native@0::missing/2\" is not registered"), "{}", error.0);
    assert!(!runtime.poisoned);
    match crate::natives::call(&mut runtime, 0, &[]) {
        Err(crate::natives::Refusal::Failure(crate::runtime::FailureKind::Program(message))) => {
            assert_eq!(message, "Native symbol \"example/native@0::missing/2\" is not registered");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn ext_characterization_std_dispatch() {
    let native = okkama_runtime_abi::Native::NumberTextIntParse;
    let mut p = program("example/native@0::missing/2");
    p.types.push(okkama_sparrow::Type::Str);
    p.functions[0].native = Some(native.symbol().into());
    p.functions[0].registers[0] = okkama_sparrow::TypeRef(1);
    p.functions[1].registers[0] = okkama_sparrow::TypeRef(1);
    p.functions[1].code[0] = okkama_sparrow::Instruction::Const {
        dst: okkama_sparrow::Dst(okkama_sparrow::Register(0)), value: okkama_sparrow::Constant::Str("17".into()),
    };
    p.verify().unwrap();
    let mut runtime = Box::new(Runtime::new(Loaded::new(p)).unwrap());
    assert_eq!(runtime.natives[0], Some(native));
    assert_eq!(runtime.run_entry(1 << 24).unwrap(), 36);
}

use crate::{budget::Limits, ext::Extension, word::{Repr, Word}};
use okkama_runtime_abi::ext::*;
use std::ptr;

unsafe extern "C" fn echo(_: *mut okk_call, args: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe { *out = *args; }
    OKK_OK
}
unsafe extern "C" fn sum(_: *mut okk_call, args: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe { *out = okk_slot { bits: (*args).bits + (*args.add(1)).bits, kind: OKK_INT }; }
    OKK_OK
}
unsafe extern "C" fn new_text(c: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe { ((*(*c).host).text_new)(c, "hello 🐦".as_ptr(), "hello 🐦".len(), out) }
}
unsafe extern "C" fn new_bytes(c: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe { ((*(*c).host).bytes_new)(c, [0, 128, 255].as_ptr(), 3, out) }
}
unsafe extern "C" fn copy_view(c: *mut okk_call, args: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe {
        let host = &*(*c).host;
        let mut data = ptr::null(); let mut len = 0;
        let (view, new) = if (*args).kind == OKK_STRING { (host.text, host.text_new) } else { (host.bytes, host.bytes_new) };
        let status = view(c, *args, &mut data, &mut len);
        if status != OKK_OK { return status; }
        // An allocation must not invalidate the earlier view.
        let mut discarded = okk_slot::default();
        for _ in 0..12 { (host.text_new)(c, b"garbage".as_ptr(), 7, &mut discarded); }
        new(c, data, len, out)
    }
}
unsafe extern "C" fn failure(c: *mut okk_call, _: *const okk_slot, _: *mut okk_slot) -> u32 {
    unsafe {
        let status = ((*(*c).host).fail)(c, b"bad_input".as_ptr(), 9, b"extension refused".as_ptr(), 17);
        if status != OKK_OK { return status; }
    }
    OKK_FAILED
}
unsafe extern "C" fn bad_kind(_: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe { *out = okk_slot { bits: 7, kind: OKK_FLOAT }; }
    OKK_OK
}
unsafe extern "C" fn bad_token(_: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe { *out = okk_slot { bits: u64::MAX, kind: OKK_STRING }; }
    OKK_OK
}
unsafe extern "C" fn unknown(_: *mut okk_call, _: *const okk_slot, _: *mut okk_slot) -> u32 { 12345 }
unsafe extern "C" fn panic_status(_: *mut okk_call, _: *const okk_slot, _: *mut okk_slot) -> u32 { OKK_PANIC }
unsafe extern "C" fn nan(_: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe { *out = okk_slot { bits: f64::NAN.to_bits(), kind: OKK_FLOAT }; }
    OKK_OK
}
unsafe extern "C" fn checks(c: *mut okk_call, _: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe {
        let host = &*(*c).host;
        if (host.text_new)(c, [255].as_ptr(), 1, out) != OKK_BAD_TEXT { return OKK_PANIC; }
        let mut interrupted = 99;
        if (host.interrupted)(c, &mut interrupted) != OKK_OK { return OKK_PANIC; }
        if interrupted != 0 { return OKK_STOPPED; }
        *out = okk_slot { bits: 0, kind: OKK_INT };
    }
    OKK_OK
}

macro_rules! export {
    ($symbol:literal, $signature:literal, $callback:ident) => {
        okk_export { symbol: $symbol.as_ptr(), signature: $signature.as_ptr(), params: c"".as_ptr(),
            sync: Some($callback), r#async: ptr::null() }
    };
}
static EXPORTS: &[okk_export] = &[
    export!(c"example/native@0::sum/2", c"(int, int) -> int", sum),
    export!(c"example/native@0::int/1", c"(int) -> int", echo),
    export!(c"example/native@0::float/1", c"(float) -> float", echo),
    export!(c"example/native@0::bool/1", c"(bool) -> bool", echo),
    export!(c"example/native@0::byte/1", c"(byte) -> byte", echo),
    export!(c"example/native@0::char/1", c"(char) -> char", echo),
    export!(c"example/native@0::text/0", c"() -> string", new_text),
    export!(c"example/native@0::bytes/0", c"() -> @byte", new_bytes),
    export!(c"example/native@0::text_copy/1", c"(string) -> string", copy_view),
    export!(c"example/native@0::bytes_copy/1", c"(@byte) -> @byte", copy_view),
    export!(c"example/native@0::failure/0", c"() -> int", failure),
    export!(c"example/native@0::bad_kind/0", c"() -> int", bad_kind),
    export!(c"example/native@0::bad_token/0", c"() -> string", bad_token),
    export!(c"example/native@0::unknown/0", c"() -> int", unknown),
    export!(c"example/native@0::panic/0", c"() -> int", panic_status),
    export!(c"example/native@0::nan/0", c"() -> float", nan),
    export!(c"example/native@0::checks/0", c"() -> int", checks),
    export!(c"example/native@0::checksum_crc32/2", c"(@byte, int) -> int", checksum_crc32),
];
static DESCRIPTOR: okk_extension = okk_extension {
    abi: OKKAMA_EXT_ABI, size: std::mem::size_of::<okk_extension>() as u32,
    name: c"test".as_ptr(), exports: EXPORTS.as_ptr(), export_count: EXPORTS.len() as u32,
    states: ptr::null(), state_count: 0,
};

fn extension() -> Extension { unsafe { Extension::from_static(&DESCRIPTOR).unwrap() } }
fn limits() -> Limits { Limits { extensions: vec![extension()], ..Limits::default() } }

const TYPES: &str = r#"type t0 int
type t1 float
type t2 bool
type t3 byte
type t4 char
type t5 str
type t6 array t3
type t7 failure
type t8 object obj0
object obj0 "NativeFailure" builtin {
 field "domain" t5
 field "code" t5
 field "operation" t5
 field "message" t5
}
type t9 rat
type t10 array t5
type t11 array t0
type t12 map t5 t11
type t13 array t12
type t14 variant variant0
type t15 variant variant1
type t16 array t9
type t17 array t11
type t18 map t0 t5
type t19 array t14
type t20 array t2
type t21 map t9 t5
object obj1 "some_int" { field "value" t0 }
object obj2 "none_int" {}
object obj3 "some_array" { field "value" t11 }
object obj4 "none_array" {}
variant variant0 "$variant_Maybe_0" { case "some" obj1 case "none" obj2 }
variant variant1 "$variant_Maybe_1" { case "some" obj3 case "none" obj4 }
"#;

fn wrapper(symbol: &str, params: &[usize], result: usize) -> okkama_sparrow::Program {
    let types = params.iter().map(|t| format!("t{t}")).collect::<Vec<_>>().join(", ");
    let regs = (0..params.len()).map(|r| format!("r{r}")).collect::<Vec<_>>().join(", ");
    let all = params.iter().copied().chain([result]).map(|t| format!("t{t}")).collect::<Vec<_>>().join(", ");
    let dst = params.len();
    let source = format!(r#"sparrow 19
module "extension_test" "1.0.0"
{TYPES}
function f0 "native" native "{symbol}" {{
 registers [{types}]
 params [{regs}]
 result t{result}
}}
function f1 "wrapper" {{
 registers [{all}]
 params [{regs}]
 result t{result}
 0: call r{dst}, f0, [{regs}]
 1: ret r{dst}
}}
"#);
    okkama_sparrow::assemble(&source).unwrap_or_else(|e| panic!("{e}\n{source}"))
}
fn runtime(symbol: &str, params: &[usize], result: usize) -> Box<Runtime> {
    Box::new(Runtime::with_limits(Loaded::new(wrapper(symbol, params, result)), limits()).unwrap())
}

#[test]
fn ext_scalar_results_and_echoes() {
    let mut sum = Box::new(Runtime::with_limits(Loaded::new(program("example/native@0::sum/2")), limits()).unwrap());
    assert_eq!(sum.run_entry(1 << 24).unwrap(), 46);
    for n in [i64::MIN, -(1 << 62), -1, 0, 1, (1 << 62) - 1, 1 << 62, i64::MAX] {
        let mut r = runtime("example/native@0::int/1", &[0], 0);
        let argument = r.heap.int(n);
        let value = r.run_function_with(1, &[argument], 1 << 24).unwrap();
        assert_eq!(r.small_or_big(value), Some(n));
    }
    for (symbol, ty, value) in [
        ("float", 1, (-123.5f64).to_bits()), ("float", 1, (-0.0f64).to_bits()),
        ("bool", 2, 0), ("bool", 2, 1), ("byte", 3, 255), ("char", 4, '🐦' as u64),
    ] {
        let mut r = runtime(&format!("example/native@0::{symbol}/1"), &[ty], ty);
        assert_eq!(r.run_function_with(1, &[value], 1 << 24).unwrap(), value);
    }
}

#[test]
fn ext_strings_bytes_and_views_survive_allocations() {
    let mut r = runtime("example/native@0::text/0", &[], 5);
    let word = r.run_function_with(1, &[], 1 << 24).unwrap();
    assert_eq!(r.heap.text(word), "hello 🐦");
    let mut r = runtime("example/native@0::bytes/0", &[], 6);
    let word = r.run_function_with(1, &[], 1 << 24).unwrap();
    assert_eq!(&*r.heap.byte_view(word), &[0, 128, 255]);
    let mut r = runtime("example/native@0::text_copy/1", &[5], 5);
    let text = r.heap.allocate_text("view 🐦");
    let word = r.run_function_with(1, &[text], 1 << 24).unwrap();
    assert_eq!(r.heap.text(word), "view 🐦");
    for packed in [true, false] {
        let mut r = runtime("example/native@0::bytes_copy/1", &[6], 6);
        let bytes = if packed { r.heap.allocate_bytes(&[0, 128, 255]) } else { r.heap.allocate_array(Repr::Raw, &[0, 128, 255]) };
        let word = r.run_function_with(1, &[bytes], 1 << 24).unwrap();
        assert_eq!(&*r.heap.byte_view(word), &[0, 128, 255]);
    }
}

#[test]
fn ext_binding_refusals_and_restrictions() {
    let error = Runtime::with_limits(Loaded::new(wrapper("example/native@0::sum/2", &[0, 1], 0)), limits()).unwrap_err();
    assert!(error.0.contains("(int, float) -> int") && error.0.contains("(int, int) -> int"), "{}", error.0);
    let mut denied = limits(); denied.extensions.push(extension());
    let error = Runtime::with_limits(Loaded::new(program("example/native@0::sum/2")), denied).unwrap_err();
    assert!(error.0.contains("duplicate extension symbol"), "{}", error.0);
    for (policy, message) in [
        (Limits { natives: Some(vec![]), ..limits() }, "Native example/native@0::sum/2 is in no native group; it is refused while the native groups are restricted"),
        (Limits { extensions: vec![extension()], ..Limits::playground() }, "Native example/native@0::sum/2 is not available in the playground"),
    ] {
        let mut r = Box::new(Runtime::with_limits(Loaded::new(program("example/native@0::sum/2")), policy).unwrap());
        assert!(r.run_entry(1 << 24).unwrap_err().0.contains(message));
        assert!(!r.poisoned);
    }
}

#[test]
fn ext_std_shadow_is_refused_before_callbacks() {
    static EXPORT: okk_export = export!(c"runtime/checksum@1::checksum_crc32/2", c"(@byte, int) -> int", checksum_crc32);
    static SHADOW: okk_extension = okk_extension { exports: &EXPORT, export_count: 1, ..DESCRIPTOR };
    let error = unsafe { Extension::from_static(&SHADOW) }.unwrap_err();
    assert!(error.0.contains("shadows the std native table"));
}

#[test]
fn ext_contract_faults_stop_and_poison() {
    for (symbol, ty) in [("bad_kind", 0), ("bad_token", 5), ("unknown", 0), ("panic", 0)] {
        let mut r = runtime(&format!("example/native@0::{symbol}/0"), &[], ty);
        let error = r.run_function_with(1, &[], 1 << 24).unwrap_err();
        assert!(error.0.contains(&format!("internal error in native example/native@0::{symbol}/0")), "{}", error.0);
        assert!(r.poisoned);
        assert!(r.run_function_with(1, &[], 1 << 24).unwrap_err().0.contains("unusable"));
    }
}

#[test]
fn ext_utf8_and_interrupted_host_entries() {
    let mut r = runtime("example/native@0::checks/0", &[], 0);
    let result = r.run_function_with(1, &[], 1 << 24).unwrap();
    assert_eq!(r.small_or_big(result), Some(0));
    r.limits.interrupt = Some(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)));
    assert!(matches!(crate::natives::call(&mut r, 0, &[]), Err(crate::natives::Refusal::Exhausted(crate::budget::Exhausted::Interrupted))));
    assert!(!r.poisoned);
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256]; let mut i = 0;
    while i < 256 {
        let mut crc = i as u32; let mut j = 0;
        while j < 8 { crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1)); j += 1; }
        table[i] = crc; i += 1;
    }
    table
}
static CRC_TABLE: [u32; 256] = crc_table();
unsafe extern "C" fn checksum_crc32(c: *mut okk_call, args: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe {
        let mut data = ptr::null(); let mut len = 0;
        let status = ((*(*c).host).bytes)(c, *args, &mut data, &mut len);
        if status != OKK_OK { return status; }
        let mut crc = !((*args.add(1)).bits as u32);
        for &b in std::slice::from_raw_parts(data, len) { crc = CRC_TABLE[((crc ^ u32::from(b)) & 255) as usize] ^ (crc >> 8); }
        *out = okk_slot { bits: u64::from(!crc), kind: OKK_INT };
        OKK_OK
    }
}

fn catching(symbol: &str, result: usize) -> okkama_sparrow::Program {
    okkama_sparrow::assemble(&format!(r#"sparrow 19
module "extension_test" "1.0.0"
{TYPES}
function f0 "native" native "{symbol}" {{
 registers []
 params []
 result t{result}
}}
function f1 "catcher" {{
 registers [t{result}, t7, t8]
 params []
 result t8
 handler 0 1 @2 r1
 0: call r0, f0, []
 1: unreachable
 2: catch r1, object, 0, r2, @3, @4
 3: ret r2
 4: raise r1
}}
"#)).unwrap()
}

#[test]
fn ext_native_failure_is_caught_with_host_identity() {
    for (name, result, code, message) in [
        ("failure", 0, "bad_input", "extension refused"),
        ("nan", 1, "not_finite", "Native float result is not finite"),
    ] {
        let symbol = format!("example/native@0::{name}/0");
        let mut r = Box::new(Runtime::with_limits(Loaded::new(catching(&symbol, result)), limits()).unwrap());
        let payload = r.run_function_with(1, &[], 1 << 24).unwrap();
        let fields: Vec<_> = (0..4).map(|i| r.heap.text(r.heap.field(payload, i)).to_owned()).collect();
        assert_eq!(fields, ["example/native", code, &symbol, message]);
        assert!(!r.poisoned);
    }
    for (name, result) in [("bad_kind", 0), ("bad_token", 5), ("unknown", 0), ("panic", 0)] {
        let mut r = Box::new(Runtime::with_limits(Loaded::new(catching(&format!("example/native@0::{name}/0"), result)), limits()).unwrap());
        assert!(r.run_function_with(1, &[], 1 << 24).unwrap_err().0.contains("internal error in native"));
        assert!(r.poisoned);
    }
}

#[test]
#[ignore = "#1083 cost gate: run with --profile gate --ignored --nocapture"]
fn ext_cost_crc32_64_bytes() {
    use std::{hint::black_box, time::Instant};
    let mut external = runtime("example/native@0::checksum_crc32/2", &[6, 0], 0);
    let mut builtin = Box::new(Runtime::new(Loaded::new(wrapper("runtime/checksum@1::checksum_crc32/2", &[6, 0], 0))).unwrap());
    let data: Vec<u8> = (0..64).collect();
    let ext_args = [external.heap.allocate_bytes(&data), external.heap.int(0)];
    let std_args = [builtin.heap.allocate_bytes(&data), builtin.heap.int(0)];
    let run = |r: &mut Runtime, args: &[Word]| {
        let (value, repr) = crate::natives::call(r, 0, black_box(args)).unwrap();
        assert_eq!(repr, Repr::Int);
        black_box(value)
    };
    let a = run(&mut external, &ext_args); let b = run(&mut builtin, &std_args);
    assert_eq!(external.small_or_big(a), Some(0x100ece8c));
    assert_eq!(external.small_or_big(a), builtin.small_or_big(b));
    for _ in 0..2000 { run(&mut external, &ext_args); run(&mut builtin, &std_args); }
    let mut ext_ns = Vec::new(); let mut std_ns = Vec::new();
    const N: u32 = 100_000;
    for round in 0..10 {
        for ext in if round % 2 == 0 { [true, false] } else { [false, true] } {
            let (r, args) = if ext { (&mut external, &ext_args) } else { (&mut builtin, &std_args) };
            let start = Instant::now();
            for _ in 0..N { run(r, args); }
            let ns = start.elapsed().as_nanos() as f64 / f64::from(N);
            if ext { ext_ns.push(ns); } else { std_ns.push(ns); }
        }
    }
    ext_ns.sort_by(f64::total_cmp); std_ns.sort_by(f64::total_cmp);
    let median = |v: &[f64]| (v[4] + v[5]) / 2.0;
    let ratio = median(&ext_ns) / median(&std_ns);
    eprintln!("CRC32 64 B, {N} calls/sample, alternating 10 rounds: extension {:.2} ns/call, std {:.2} ns/call, ratio {ratio:.3}x\next samples {ext_ns:?}\nstd samples {std_ns:?}", median(&ext_ns), median(&std_ns));
    assert!(ratio <= 1.5, "extension/std = {ratio:.3}");
}

#[test]
fn ext_c_header_matches_rust_layout() {
    use std::{io::Write, mem::{align_of, offset_of, size_of}, process::{Command, Stdio}};
    let mut source = String::from("#include \"okkama_ext.h\"\n");
    macro_rules! layout {
        ($ty:ident; $($field:ident),*) => {{
            source.push_str(&format!("_Static_assert(sizeof({}) == {}, \"size\");\n_Static_assert(_Alignof({}) == {}, \"align\");\n", stringify!($ty), size_of::<$ty>(), stringify!($ty), align_of::<$ty>()));
            $(source.push_str(&format!("_Static_assert(offsetof({}, {}) == {}, \"offset\");\n", stringify!($ty), stringify!($field).trim_start_matches("r#"), offset_of!($ty, $field)));)*
        }};
    }
    layout!(okk_slot; bits, kind);
    layout!(okk_call; host);
    layout!(okk_cancel; cancelled);
    layout!(okk_async; prepare, execute, complete, drop);
    layout!(okk_export; symbol, signature, params, sync, r#async);
    layout!(okk_state_type; name, drop);
    layout!(okk_extension; abi, size, name, exports, export_count, states, state_count);
    layout!(okk_host; text, bytes, text_new, bytes_new, bytes_reserve, bigint_read, bigint_new, rat_parts, rat_new,
        array_len, array_get, array_read, array_new, map_len, map_entries, map_new, state_new, state_get, state_release, fail, interrupted);
    for (field, args) in [
        ("state_new", "okk_call *, const okk_state_type *, void *, size_t, okk_slot *"),
        ("state_get", "okk_call *, okk_slot, const okk_state_type *, void **"),
        ("state_release", "okk_call *, okk_slot, const okk_state_type *"),
        ("bytes_reserve", "okk_call *, size_t, uint8_t **, okk_slot *"),
        ("bigint_read", "okk_call *, okk_slot, int32_t *, const uint8_t **, size_t *"),
        ("bigint_new", "okk_call *, int32_t, const uint8_t *, size_t, okk_slot *"),
        ("rat_parts", "okk_call *, okk_slot, okk_slot *, okk_slot *"),
        ("rat_new", "okk_call *, okk_slot, okk_slot, okk_slot *"),
        ("array_len", "okk_call *, okk_slot, size_t *"),
        ("array_get", "okk_call *, okk_slot, size_t, okk_slot *"),
        ("array_read", "okk_call *, okk_slot, size_t, okk_slot *, size_t"),
        ("array_new", "okk_call *, const okk_slot *, size_t, okk_slot *"),
        ("map_len", "okk_call *, okk_slot, size_t *"),
        ("map_entries", "okk_call *, okk_slot, okk_slot *, okk_slot *, size_t"),
        ("map_new", "okk_call *, const okk_slot *, const okk_slot *, size_t, okk_slot *"),
    ] {
        source.push_str(&format!("_Static_assert(_Generic(((okk_host *)0)->{field}, uint32_t (*)({args}): 1, default: 0), \"prototype\");\n"));
    }
    macro_rules! constant {
        ($($c:ident),*) => { $(source.push_str(&format!("_Static_assert({} == {}, \"constant\");\n", stringify!($c), $c));)* };
    }
    constant!(OKKAMA_EXT_ABI, OKKAMA_EXT_STACK, OKK_NONE, OKK_INT, OKK_FLOAT, OKK_BOOL, OKK_BYTE, OKK_CHAR,
        OKK_BIGINT, OKK_RAT, OKK_STRING, OKK_ARRAY, OKK_MAP, OKK_STATE, OKK_OK, OKK_FAILED, OKK_PANIC,
        OKK_STOPPED, OKK_BAD_TOKEN, OKK_BAD_KIND, OKK_BAD_SHAPE, OKK_BAD_TEXT, OKK_RANGE, OKK_UNSUPPORTED);
    // A syntax-only translation unit needs no temporary files or linker.
    let mut cc = Command::new("cc").args(["-std=c11", "-Wall", "-Werror", "-fsyntax-only", "-x", "c", "-"])
        .arg("-I").arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../okkama-runtime-abi/include"))
        .stdin(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("C compiler for ABI layout check");
    cc.stdin.take().unwrap().write_all(source.as_bytes()).unwrap();
    let output = cc.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

thread_local! { static SAVED_TOKEN: std::cell::Cell<okk_slot> = const { std::cell::Cell::new(okk_slot { bits: 0, kind: OKK_NONE }) }; }
unsafe extern "C" fn stale(_: *mut okk_call, args: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe { *out = SAVED_TOKEN.with(|s| { let old = s.replace(*args); if old.kind == OKK_NONE { *args } else { old } }); }
    OKK_OK
}
unsafe extern "C" fn spoof(_: *mut okk_call, args: *const okk_slot, out: *mut okk_slot) -> u32 {
    unsafe { *out = okk_slot { bits: (*args).bits, kind: OKK_STRING }; }
    OKK_OK
}

#[test]
fn ext_stale_and_forged_tokens_are_faults() {
    static EXTRA: &[okk_export] = &[
        export!(c"example/native@0::stale/1", c"(string) -> string", stale),
        export!(c"example/native@0::spoof/1", c"(@byte) -> string", spoof),
    ];
    static DESCRIPTOR2: okk_extension = okk_extension { exports: EXTRA.as_ptr(), export_count: 2, ..DESCRIPTOR };
    let limits = || Limits { extensions: vec![unsafe { Extension::from_static(&DESCRIPTOR2).unwrap() }], ..Limits::default() };
    SAVED_TOKEN.with(|s| s.set(okk_slot::default()));
    // A token from another runtime is stale too.
    for first in [true, false] {
        let mut r = Box::new(Runtime::with_limits(Loaded::new(wrapper("example/native@0::stale/1", &[5], 5)), limits()).unwrap());
        let text = r.heap.allocate_text("token");
        let result = r.run_function_with(1, &[text], 1 << 24);
        if first { assert_eq!(r.heap.text(result.unwrap()), "token"); }
        else { assert!(result.unwrap_err().0.contains("internal error in native")); assert!(r.poisoned); }
    }
    let mut r = Box::new(Runtime::with_limits(Loaded::new(wrapper("example/native@0::spoof/1", &[6], 5)), limits()).unwrap());
    let bytes = r.heap.allocate_bytes(b"not text");
    assert!(r.run_function_with(1, &[bytes], 1 << 24).unwrap_err().0.contains("internal error in native"));
}

#[test]
fn ext_session_extension_binds_and_refuses_mismatch_atomically() {
    let initial = program("example/native@0::sum/2");
    let capacities = crate::runtime::Capacities { functions: 16, objects: 8 };
    let mut r = Box::new(Runtime::session_with_limits(Loaded::new(initial.clone()), capacities, limits()).unwrap());
    assert_eq!(r.run_entry(1 << 24).unwrap(), 46);
    let mut next = initial.clone();
    let mut bad = initial.functions[0].clone();
    bad.symbol = "bad".into(); bad.native = Some("example/native@0::int/1".into());
    next.functions.push(bad);
    assert!(r.extend(next).unwrap_err().0.contains("signature mismatch"));
    assert_eq!(r.loaded.program.functions.len(), 2);
    let mut next = initial.clone();
    let mut good = initial.functions[0].clone();
    good.symbol = "extra".into();
    next.functions.push(good);
    r.extend(next).unwrap();
    assert!(r.external[2].is_some());
    assert_eq!(r.run_entry(1 << 24).unwrap(), 46);
}

#[test]
fn ext_public_entry_uses_limits_registry_and_load_refusals() {
    let bytes = okkama_sparrow::encode(&program("example/native@0::sum/2")).unwrap();
    assert_eq!(crate::run_limited(bytes, vec![], false, vec![], limits()).unwrap(), 46);
    let bytes = okkama_sparrow::encode(&wrapper("example/native@0::sum/2", &[0, 1], 0)).unwrap();
    assert!(crate::run_described(bytes, vec![], false, vec![], limits()).unwrap_err().0.contains("signature mismatch"));
    let mut missing = Box::new(Runtime::with_limits(Loaded::new(program("example/native@0::missing/2")), limits()).unwrap());
    assert!(missing.run_entry(1 << 24).unwrap_err().0.contains("Native symbol \"example/native@0::missing/2\" is not registered"));
}

#[test]
fn ext_native_driver_keeps_missing_provider_and_std_dispatch() {
    let source = |symbol: &str, result: &str| format!(r#"sparrow 19
module "driver_test" "1.0.0"
type t0 str
type t1 bool
function f0 "native" async native "{symbol}" {{
 registers [t0]
 params [r0]
 result {result}
}}
"#);
    for (symbol, missing) in [("example/native@0::missing/1", true), ("runtime/time@1::sleep_nanoseconds/1", false)] {
        let p = okkama_sparrow::assemble(&source(symbol, "t1")).unwrap();
        let mut r = Box::new(Runtime::new(Loaded::new(p)).unwrap());
        let zero = r.heap.allocate_text("0");
        let result = r.run_function_with(0, &[zero], 1 << 24);
        if missing { assert!(result.unwrap_err().0.contains("Native symbol \"example/native@0::missing/1\" is not registered")); }
        else { result.unwrap(); }
    }
}

#[test]
fn ext_values_bigint_normalization() {
    use num_bigint::BigInt;
    for n in [BigInt::from(1u64 << 62), BigInt::from(i64::MAX), BigInt::from(i64::MIN),
        BigInt::from(1u64 << 63), BigInt::from(1u128 << 64), -BigInt::from(1u128 << 64),
        BigInt::parse_bytes("9".repeat(300).as_bytes(), 10).unwrap()] {
        let mut r = runtime("example/native@0::int/1", &[0], 0);
        let argument = r.heap.big(n.clone());
        let result = r.run_function_with(1, &[argument], 1 << 24).unwrap();
        assert_eq!(match r.heap.integer(result) { crate::heap::Integer::Small(v) => BigInt::from(v), crate::heap::Integer::Big(v) => v.clone() }, n);
    }
}

#[path = "ext_values_tests.rs"]
mod values_tests;

#[path = "../../../../fuzz/fuzz_targets/ext_body.rs"]
mod ext_fuzz;

#[test]
fn ext_fuzz_seeds() {
    for seed in [include_bytes!("../../../../fuzz/seeds/ext/scalars").as_slice(),
        include_bytes!("../../../../fuzz/seeds/ext/values").as_slice(),
        include_bytes!("../../../../fuzz/seeds/ext/invalid").as_slice(),
        include_bytes!("../../../../fuzz/seeds/ext/memory").as_slice()] {
        ext_fuzz::exercise(seed);
    }
    for status in [OKK_OK, OKK_FAILED, OKK_PANIC, OKK_STOPPED, OKK_BAD_TOKEN, OKK_BAD_KIND,
        OKK_BAD_SHAPE, OKK_BAD_TEXT, OKK_RANGE, OKK_UNSUPPORTED, u32::MAX] {
        let mut seed = [0; 8];
        seed[..4].copy_from_slice(&status.to_le_bytes());
        seed[4..].copy_from_slice(&status.to_le_bytes());
        ext_fuzz::exercise(&seed);
    }
    // Create/read/release, then a closed-state read; retain the source token.
    let mut state_seed = vec![0];
    for op in [25, 26, 25] {
        let mut packet = [0; 24];
        packet[0] = op; packet[1] = 1; packet[2] = 8; packet[23] = 7;
        state_seed.extend(packet);
    }
    ext_fuzz::exercise(&state_seed);
    for ty in 0..4 {
        for op in 24..27 {
            let mut seed = [0; 25];
            seed[1] = op; seed[2] = 1; seed[3] = 8; seed[22] = ty;
            ext_fuzz::exercise(&seed);
        }
    }
    for op in 0..27 {
        for selector in [0, 1, 7, 8, 9, 255] {
            let mut seed = [0; 25]; seed[1] = op; seed[2] = selector; seed[3] = selector % 16;
            seed[13] = selector; seed[21] = selector % 17;
            ext_fuzz::exercise(&seed);
        }
    }
}
