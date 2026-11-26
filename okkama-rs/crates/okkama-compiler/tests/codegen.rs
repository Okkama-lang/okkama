//! Backend generation verifies IR and emits deterministic SPB bytes.

use okkama_compiler::codegen::{self, Backend, CodegenError, Options};
use okkama_compiler::ir::{self, InstKind, TermKind, ValueId};
use okkama_compiler::lowering::lower_program;
use okkama_compiler::syntax::{self, ParseOptions};
use okkama_compiler::SourceFile;

fn module(source: &str) -> ir::Module {
    let source = SourceFile::new("test.ok", source).unwrap();
    let ast = syntax::parse(&source, &ParseOptions::default()).unwrap();
    lower_program(&ast).unwrap()
}

fn generate(module: &ir::Module) -> Result<Vec<u8>, CodegenError> {
    codegen::generate(
        module,
        Backend::Sparrow,
        &Options {
            artifact_name: "Arithmetic".to_string(),
        },
    )
    .map(|artifact| artifact.main().to_vec())
}

/// `1 + 1` as lowered, before the passes fold it (the tests edit its instructions).
fn addition() -> ir::Module {
    okkama_compiler::lowering::with_passes("none", || module("module app v1.0.0; main() is int { 1 + 1; }"))
}

#[test]
fn generates_deterministic_sparrow_bytecode() {
    let module = addition();
    let bytes = generate(&module).unwrap();
    okkama_sparrow::decode(&bytes).expect("valid SPB");
    assert_eq!(bytes, generate(&module).unwrap());
}

/// Locals of several types next to an action whose nested actions use two of
/// them, the later declared first, one through a third level (#1077).
const NESTED_CAPTURES: &str = "module app v1.0.0;
main() is int {
    let text = \"a\";
    let flag = true;
    let count = 2;
    let rate = 1.5;
    let nested = (a is int) is int {
        let inner = (b is int) is int {
            let deepest = () is int { count + a; };
            deepest() + b + text.length();
        };
        inner(1);
    };
    if flag && rate > 1.0 { nested(2); } else { 0; }
}";

#[test]
fn nested_anonymous_actions_generate_the_same_bytes_every_build() {
    let first = generate(&module(NESTED_CAPTURES)).unwrap();
    for _ in 0..8 {
        assert!(first == generate(&module(NESTED_CAPTURES)).unwrap(), "two builds of one source differ");
    }
}

#[test]
fn an_anonymous_action_captures_only_what_it_uses_in_the_order_of_first_use() {
    let module = module(NESTED_CAPTURES);
    let main = module.functions.iter().find(|function| function.symbol == "main").expect("main is lowered");
    let captured: Vec<Vec<ir::Type>> = main
        .blocks
        .iter()
        .flat_map(|block| &block.insts)
        .filter_map(|inst| match &inst.kind {
            InstKind::FnClosure { captures, .. } => {
                Some(captures.iter().map(|value| main.values[value.0].clone()).collect())
            }
            _ => None,
        })
        .collect();
    // `nested` names neither `flag` nor `rate`; it captures `count` (for
    // `deepest`) and `text` (for `inner`), in the order of first use.
    assert_eq!(captured, vec![vec![ir::Type::Int, ir::Type::Str]]);
}

#[test]
fn invalid_ir_is_rejected_before_serialization() {
    let mut module = addition();
    let block = &mut module.functions[0].blocks[0];
    block.insts[2].kind = InstKind::IntBinary {
        op: ir::IntBinaryOp::Add,
        left: ValueId(0),
        right: ValueId(2),
    };
    let error = generate(&module).unwrap_err().to_string();
    assert!(error.contains("invalid IR"), "{error}");
    assert!(error.contains("%2 is used in ^b0"), "{error}");

    let mut module = addition();
    module.functions[0].blocks[0].term.kind = TermKind::Ret(Some(ValueId(99)));
    let error = generate(&module).unwrap_err().to_string();
    assert!(
        error.contains("%99 is not a value of this function"),
        "{error}"
    );
}

#[test]
fn unresolved_imports_are_rejected_before_serialization() {
    let imported = module("module app v1.0.0; import \"missing\"; main() is int { 1; }");
    assert!(generate(&imported).unwrap_err().to_string().contains("imports must be resolved"));
}
