//! Compile-only coverage of the feature layout consumers; no program execution.
use super::*;
use crate::champ::{LayoutFacts, ObjectRuntimeLayout};
use okkama_sparrow::{Dst, FuncRef, Function, ObjectDecl, ObjectRef, Program, Register, TypeRef};

fn runtime() -> Runtime {
    let program = Program {
        types: vec![Type::Int, Type::Object(ObjectRef(0))],
        objects: vec![ObjectDecl {
            name: "Resource".into(),
            fields: vec![("value".into(), TypeRef(0))],
            does: vec![],
            destroy: Some(FuncRef(0)),
            builtin: false,
        }],
        functions: vec![Function {
            symbol: "fixture".into(),
            is_async: false,
            native: None,
            intrinsic: None,
            registers: vec![TypeRef(1), TypeRef(0), TypeRef(1)],
            params: vec![Register(0), Register(1)],
            result: Some(TypeRef(1)),
            code: vec![I::Return { value: Some(Register(0)) }],
            handlers: vec![],
        }],
        ..Program::default()
    };
    let mut runtime = Runtime::new(crate::program::Loaded::new(program)).unwrap();
    // Final storage includes a hidden field absent from the source declaration.
    let storage = vec![Repr::Int, Repr::Ref];
    let layout = ObjectRuntimeLayout::new(storage.clone(), LayoutFacts {
        has_destroy: true, may_reach_resource: true, may_async_destroy: false,
    }, vec![1]).unwrap();
    runtime.heap = Heap::new_champ(
        heap::Layouts { objects: vec![storage], ..heap::Layouts::default() },
        vec![layout].into_boxed_slice(),
    );
    runtime
}

#[test]
fn champ_jit_new_successor_and_variant_use_final_allocation_and_storage_offsets() {
    use cranelift_codegen::ir::{InstructionData, Opcode};
    use okkama_sparrow::{VariantDecl, VariantRef};
    for kind in 0..3 {
        let mut runtime = runtime();
        let dst = Dst(Register(2));
        let instruction = match kind {
            0 => I::ObjNew { dst, object: ObjectRef(0), fields: vec![Register(1)] },
            1 => I::ObjNext { dst, object: Register(0), updates: vec![(0, Register(1))] },
            _ => {
                runtime.loaded.program.variants.push(VariantDecl {
                    name: "V".into(), cases: vec![("Case".into(), ObjectRef(0))],
                });
                I::VariantNew { dst, variant: VariantRef(0), case: 0, fields: vec![Register(1)] }
            }
        };
        runtime.loaded.program.functions[0].code = vec![
            instruction, I::Return { value: Some(Register(2)) },
        ];
        compile(&mut runtime, 0).unwrap();
        let function = &runtime.jit.emitted[&0];
        let instructions: Vec<_> = function.layout.blocks().flat_map(|block| function.layout.block_insts(block))
            .map(|inst| &function.dfg.insts[inst]).collect();
        let class = Heap::class_of(runtime.heap.object_runtime_layout(0).allocation_words()).unwrap();
        let free_head = runtime.heap.free_head(class) as i64;
        assert!(instructions.iter().any(|instruction| {
            matches!(instruction, InstructionData::UnaryImm { opcode: Opcode::Iconst, imm }
                if i64::from(*imm) == free_head)
        }), "allocation must pop the class of the complete seven-word object");
        // All four raw tail words (24..48) are initialized independently of source fields.
        for expected in [24, 32, 40, 48] {
            assert!(instructions.iter().any(|instruction| {
                matches!(instruction, InstructionData::Store { offset, .. } if i32::from(*offset) == expected)
            }));
        }
        assert!(instructions.iter().any(|instruction| {
            matches!(instruction, InstructionData::UnaryImm { opcode: Opcode::Iconst, imm }
                if i64::from(*imm) == runtime.heap.object_runtime_layout(0).header(0) as i64)
        }));
        if kind == 1 {
            // The base may be a precise-root reload after allocation, rather than a block parameter.
            let bases: Vec<_> = instructions.iter().filter_map(|instruction| match instruction {
                InstructionData::Load { arg, offset, .. } if i32::from(*offset) == 16 => Some(*arg),
                _ => None,
            }).collect();
            assert_eq!(bases.len(), 1);
            let offsets: Vec<_> = instructions.iter().filter_map(|instruction| match instruction {
                InstructionData::Load { arg, offset, .. } if *arg == bases[0] => Some(i32::from(*offset)),
                _ => None,
            }).collect();
            assert_eq!(offsets, vec![16], "copy hidden storage, never predecessor header/tail");
        }
    }
}
