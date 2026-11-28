//! #917: the root walk resolves each return address once per collection through a small cache; a synthetic
//! stack with colliding, unmapped and foreign return addresses reports exactly the mapped slots.
use super::*;

#[test]
fn the_cached_walk_reports_the_mapped_slots_of_every_frame() {
    let program = okkama_sparrow::assemble(
        "sparrow 19\nmodule \"roots\" \"1.0.0\"\ntype t0 int\n\
         function f0 \"main\" {\n registers [t0]\n params []\n result t0\n 0: const r0, int 0\n 1: ret r0\n}\nentry f0\n",
    ).unwrap();
    let mut runtime = Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap());
    // Two return addresses 256 bytes apart share a cache slot; one in code has no map; one is outside code.
    let (a, b, unmapped, foreign) = (0x1_0100u64, 0x1_0200u64, 0x1_0300u64, 0x9_0000u64);
    runtime.jit.ranges = vec![(0x1_0000, 0x2_0000)];
    runtime.jit.maps = vec![(a, 16, vec![0]), (b, 32, vec![8, 16])];
    // The stack: frame records [caller fp, return address], innermost first, each 64 words apart.
    let rets = [a, b, a, unmapped, foreign, b, a];
    let mut memory = vec![0u64; 64 * (rets.len() + 2)];
    let base = memory.as_mut_ptr() as u64;
    let fp = |i: usize| base + (i as u64) * 64 * 8 + 256;
    let mut expected = Vec::new();
    for (i, &ret) in rets.iter().enumerate() {
        let (record, caller) = (fp(i), fp(i + 1));
        let word = |address: u64| ((address - base) / 8) as usize;
        memory[word(record)] = caller;
        memory[word(record) + 1] = ret;
        let slots: &[u32] = match ret { r if r == a => &[0], r if r == b => &[8, 16], _ => &[] };
        let fp_to_sp = if ret == a { 16 } else { 32 };
        for (k, &offset) in slots.iter().enumerate() {
            let value = ((i as u64) << 8 | k as u64) << 1;
            memory[word(caller - fp_to_sp + u64::from(offset))] = value;
            expected.push(value);
        }
    }
    let entry = fp(rets.len());
    let mut seen = Vec::new();
    // SAFETY: the records and slots above live in `memory` for the whole walk.
    unsafe { runtime.jit.frame_roots(fp(0), entry, &mut |value, _| seen.push(value)) };
    assert_eq!(seen, expected);
}
