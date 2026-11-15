//! The general synchronous native ABI and Atomic storage contracts of the compiler catalogue (#569 C7).
use super::native_summaries::{atomic_value, contract};
use crate::oir2 as ir;
use okkama_sparrow::abi::{self, catalogue};

fn builder() -> ir::ModuleBuilder {
    ir::ModuleBuilder::new("app", ir::Profile::High)
}

fn handle(builder: &mut ir::ModuleBuilder, name: &str) -> ir::TypeId {
    let existing = builder.module().declarations.iter().find(|(_, decl)| decl.name == name).map(|(id, _)| id);
    let decl = existing.unwrap_or_else(|| builder.declare(ir::Declaration {
        name: name.into(), type_params: vec![], kind: ir::DeclarationKind::Handle, loc: None,
    }).unwrap());
    builder.intern_type(ir::Type::Handle(decl)).unwrap()
}

fn lower(builder: &mut ir::ModuleBuilder, ty: &abi::Type, module: &str) -> ir::TypeId {
    let lowered = match ty {
        abi::Type::Int => ir::Type::Int,
        abi::Type::Float => ir::Type::Float,
        abi::Type::Rat => ir::Type::Rat,
        abi::Type::Bool => ir::Type::Bool,
        abi::Type::Byte => ir::Type::Byte,
        abi::Type::Char => ir::Type::Char,
        abi::Type::Str => ir::Type::Str,
        abi::Type::Any => ir::Type::Any,
        abi::Type::Array(item) => ir::Type::Array(lower(builder, item, module)),
        abi::Type::Map(key, value) => {
            let key = lower(builder, key, module);
            ir::Type::Map(key, lower(builder, value, module))
        }
        abi::Type::Handle(name) => return handle(builder, &format!("{}.{name}", module.replace('/', "."))),
        abi::Type::Void => unreachable!("void is no value type"),
    };
    builder.intern_type(lowered).unwrap()
}

/// The source declaration a producer emits for `native`: its table signature over High types.
fn declared(builder: &mut ir::ModuleBuilder, native: abi::Native) -> ir::Signature {
    let signature = native.signature();
    let params = signature.params.iter().map(|ty| lower(builder, ty, native.module())).collect();
    let results = match &signature.result {
        abi::Type::Void => vec![],
        ty => vec![lower(builder, ty, native.module())],
    };
    ir::Signature { params, results, ownership_abi: ir::OwnershipAbi::Unelaborated }
}

fn signature(params: Vec<ir::TypeId>, results: Vec<ir::TypeId>) -> ir::Signature {
    ir::Signature { params, results, ownership_abi: ir::OwnershipAbi::Unelaborated }
}

#[test]
fn every_admitted_table_native_has_a_data_call_boundary_contract() {
    let mut builder = builder();
    let (mut admitted, mut rejected) = (0, 0);
    for &native in abi::Native::ALL {
        let signature = declared(&mut builder, native);
        let result = contract(builder.module(), native.symbol(), true, &signature);
        match catalogue::admitted(native.symbol()) {
            Ok(_) => {
                let summary = result.unwrap_or_else(|error| panic!("{}: {}", native.symbol(), error.message));
                assert_eq!(summary.entry_kind, ir::EntryKind::CallBoundary);
                assert_eq!(summary.contract.failures, vec![ir::FailureKind::Native]);
                assert!(!summary.contract.suspends && !summary.contract.async_destroy);
                assert!(summary.parameters.iter().all(|parameter| {
                    parameter.convention == ir::FreshConvention::Borrow && parameter.retention.is_none()
                        && parameter.deps.is_empty()
                }));
                assert!(summary.results.iter().all(|result| result.fresh.is_empty() && result.aliases.is_empty()
                    && result.external_deps.is_empty() && !result.opaque));
                admitted += 1;
            }
            Err(rejection) => {
                let error = result.unwrap_err();
                assert!(error.message.contains(&rejection.to_string()), "{}", error.message);
                rejected += 1;
            }
        }
    }
    assert_eq!(admitted + rejected, abi::Native::ALL.len());
    assert!(admitted > 0 && rejected > 0);
}

#[test]
fn a_table_native_must_match_its_signature_exactly() {
    let mut builder = builder();
    let string = builder.intern_type(ir::Type::Str).unwrap();
    let int = builder.intern_type(ir::Type::Int).unwrap();
    let byte = builder.intern_type(ir::Type::Byte).unwrap();
    let bytes = builder.intern_type(ir::Type::Array(byte)).unwrap();
    let module = builder.module();
    assert!(contract(module, "runtime/hex@1::hex_encode/1", true, &signature(vec![bytes], vec![string])).is_ok());
    for wrong in [signature(vec![string], vec![string]), signature(vec![bytes], vec![]),
        signature(vec![bytes, int], vec![string]), signature(vec![bytes], vec![string, string])] {
        let error = contract(module, "runtime/hex@1::hex_encode/1", true, &wrong).unwrap_err();
        assert!(error.message.contains("does not match the runtime table signature"), "{}", error.message);
    }
}

#[test]
fn a_table_handle_is_identified_by_module_and_name() {
    let mut builder = builder();
    let int = builder.intern_type(ir::Type::Int).unwrap();
    let own = handle(&mut builder, "runtime.buffer.BufferState");
    let foreign = handle(&mut builder, "runtime.json.BufferState");
    let other = handle(&mut builder, "runtime.buffer.TextBufferState");
    let name = "runtime/buffer@1::buffer_state_size/1";
    assert!(contract(builder.module(), name, true, &signature(vec![own], vec![int])).is_ok());
    for wrong in [foreign, other, int] {
        assert!(contract(builder.module(), name, true, &signature(vec![wrong], vec![int])).is_err());
    }
}

#[test]
fn rejected_table_natives_name_their_reason() {
    let mut builder = builder();
    let any = builder.intern_type(ir::Type::Any).unwrap();
    let string = builder.intern_type(ir::Type::Str).unwrap();
    let int = builder.intern_type(ir::Type::Int).unwrap();
    let terminal = handle(&mut builder, "runtime.application.TerminalState");
    let module = builder.module();
    let error = contract(module, "runtime/debug@1::data_dump/1", true, &signature(vec![any], vec![string]));
    assert!(error.unwrap_err().message.contains("trusted intrinsic bridge"));
    let error = contract(module, "runtime/application@1::terminal_standard/1", true,
        &signature(vec![int], vec![terminal]));
    assert!(error.unwrap_err().message.contains("asynchronous"));
}

#[test]
fn an_unknown_symbol_uses_the_checked_adapter_only_for_data_and_own_handles() {
    let mut builder = builder();
    let int = builder.intern_type(ir::Type::Int).unwrap();
    let string = builder.intern_type(ir::Type::Str).unwrap();
    let strings = builder.intern_type(ir::Type::Array(string)).unwrap();
    let table = builder.intern_type(ir::Type::Map(string, int)).unwrap();
    let port = handle(&mut builder, "app.Port");
    let foreign = handle(&mut builder, "lib.Port");
    let any = builder.intern_type(ir::Type::Any).unwrap();
    let object = builder.declare(ir::Declaration {
        name: "app.Thing".into(), type_params: vec![], loc: None,
        kind: ir::DeclarationKind::Object { fields: vec![], witnesses: vec![], destroy: None, builtin: false },
    }).unwrap();
    let thing = builder.intern_type(ir::Type::Object { decl: object, args: vec![] }).unwrap();
    let module = builder.module();
    let summary = contract(module, "app@1::lookup/3", true, &signature(vec![string, port, table], vec![strings]))
        .unwrap();
    assert_eq!(summary.contract.failures, vec![ir::FailureKind::Native]);
    assert!(contract(module, "app@1::tick/0", true, &signature(vec![], vec![])).is_ok());
    for (name, wrong) in [
        ("app@1::lookup/1", signature(vec![thing], vec![])),
        ("app@1::lookup/1", signature(vec![any], vec![])),
        ("app@1::lookup/1", signature(vec![foreign], vec![])),
        ("app@1::lookup/0", signature(vec![], vec![thing])),
        ("app@1::lookup/2", signature(vec![int], vec![])),
    ] {
        let error = contract(module, name, true, &wrong).unwrap_err();
        assert!(error.message.contains("no provider contract"), "{name}: {}", error.message);
    }
    let error = contract(module, "app::lookup/0", true, &signature(vec![], vec![])).unwrap_err();
    assert!(error.message.contains("not canonical"), "{}", error.message);
}

fn scalar(suffix: &str) -> ir::Type {
    match suffix {
        "int" => ir::Type::Int,
        "string" => ir::Type::Str,
        "bool" => ir::Type::Bool,
        "byte" => ir::Type::Byte,
        "char" => ir::Type::Char,
        "float" => ir::Type::Float,
        "rat" => ir::Type::Rat,
        _ => unreachable!(),
    }
}

fn atomic_module(suffix: &str) -> (ir::ModuleBuilder, ir::TypeId, ir::TypeId) {
    let mut builder = builder();
    let mut value = None;
    for part in suffix.split('_').rev() {
        value = Some(match (part, value) {
            ("array", Some(item)) => builder.intern_type(ir::Type::Array(item)).unwrap(),
            (part, None) => builder.intern_type(scalar(part)).unwrap(),
            _ => unreachable!(),
        });
    }
    let value = value.unwrap();
    assert_eq!(atomic_value(builder.module(), suffix), Some(value));
    builder.intern_type(ir::Type::Bool).unwrap();
    let decl = builder.declare(ir::Declaration {
        name: format!("async.atomic.Atomic_{suffix}"), type_params: vec![], loc: None,
        kind: ir::DeclarationKind::Object { fields: vec![], witnesses: vec![], destroy: None, builtin: true },
    }).unwrap();
    let object = builder.intern_type(ir::Type::Object { decl, args: vec![] }).unwrap();
    (builder, value, object)
}

#[test]
fn atomic_operations_for_every_plain_specialization() {
    for suffix in ["int", "string", "bool", "byte", "char", "float", "rat", "array_string", "array_array_int"] {
        let (mut builder, value, object) = atomic_module(suffix);
        let bool_ty = builder.intern_type(ir::Type::Bool).unwrap();
        let module = builder.module();
        let entry = |op: &str, params: Vec<ir::TypeId>, results: Vec<ir::TypeId>| {
            contract(module, &format!("okkama.atomic.Atomic.{op}${suffix}"), false, &signature(params, results))
                .unwrap_or_else(|error| panic!("{op}${suffix}: {}", error.message))
        };
        let cas = entry("compare_and_replace", vec![object, value, value], vec![bool_ty]);
        assert_eq!(cas.entry_kind, ir::EntryKind::CallBoundary);
        assert_eq!(cas.contract.failures, vec![ir::FailureKind::Thrown]);
        let closed = entry("closed", vec![object], vec![bool_ty]);
        assert_eq!(closed.entry_kind, ir::EntryKind::ObservationAdmissionStaysCaller);
        assert!(closed.contract.failures.is_empty());
        entry("new_value", vec![value], vec![object]);
        entry("value", vec![object], vec![value]);
        entry("replace", vec![object, value], vec![]);
        entry("close", vec![object], vec![]);
        let wrong = signature(vec![object, value], vec![bool_ty]);
        assert!(contract(module, &format!("okkama.atomic.Atomic.compare_and_replace${suffix}"), false, &wrong)
            .is_err());
    }
}

#[test]
fn atomic_rejects_unregistered_values_and_operations() {
    let (builder, value, object) = atomic_module("int");
    let module = builder.module();
    for suffix in ["range", "array<int>", "array_", "array_range", "map_int"] {
        assert_eq!(atomic_value(module, suffix), None, "{suffix}");
    }
    // A registered suffix whose value type the module does not hold has no specialization.
    assert_eq!(atomic_value(module, "array_int"), None);
    let error = contract(module, "okkama.atomic.Atomic.swap$int", false, &signature(vec![object, value], vec![]));
    assert!(error.unwrap_err().message.contains("unknown Atomic operation"));
    let error = contract(module, "okkama.atomic.Atomic.value$range", false, &signature(vec![object], vec![value]));
    assert!(error.unwrap_err().message.contains("unsupported Atomic value"));
}
