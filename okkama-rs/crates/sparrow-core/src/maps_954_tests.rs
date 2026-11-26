//! #954: a map literal (`map.new`) names each key once.
use super::*;

fn program(code: &[&str]) -> String {
    let code: String = code.iter().enumerate().map(|(pc, op)| format!(" {pc}: {op}\n")).collect();
    format!("sparrow 18\nmodule \"maps-954\" \"1\"\ntype t0 int\ntype t1 str\ntype t2 map t1 t0\nfunction f0 \"literal\" {{\n registers [t1, t1, t0, t0, t2, t0]\n params []\n result t0\n{code}}}\nentry f0\n")
}

fn run(code: &[&str]) -> Result<String, String> {
    let program = okkama_sparrow::assemble(&program(code)).map_err(|error| format!("{error:?}"))?;
    let mut runtime = Box::new(Runtime::new(crate::program::Loaded::new(program)).map_err(|error| format!("{error:?}"))?);
    runtime
        .run_function(0, crate::STACK_BYTES)
        .map(|result| runtime.big_value(result).to_string())
        .map_err(|failure| failure.0)
}

#[test]
fn a_repeated_key_fails_the_literal() {
    crate::on_vm_thread(|| {
        // Two equal keys made at run time: the literal fails, naming the key and the entry.
        let error = run(&[
            "const r0, str \"a\"", "const r1, str \"a\"", "const r2, int 1", "const r3, int 2",
            "map.new r4, [r0: r2, r1: r3]", "map.len r5, r4", "ret r5",
        ])
        .unwrap_err();
        assert!(error.contains("Duplicate key in map literal: \"a\""), "{error}");
        assert!(error.contains("entry 2"), "{error}");
        Ok(())
    })
    .unwrap();
}

#[test]
fn distinct_keys_build_the_map() {
    crate::on_vm_thread(|| {
        let size = run(&[
            "const r0, str \"a\"", "const r1, str \"b\"", "const r2, int 1", "const r3, int 2",
            "map.new r4, [r0: r2, r1: r3]", "map.len r5, r4", "ret r5",
        ])
        .unwrap();
        assert_eq!(size, "2");
        Ok(())
    })
    .unwrap();
}
