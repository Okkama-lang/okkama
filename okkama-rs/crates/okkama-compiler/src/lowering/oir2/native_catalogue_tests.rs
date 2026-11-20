//! The general native ABI (#569 C7; async natives #601 N1) and Atomic storage contracts of the compiler catalogue.
use super::native_summaries::{atomic_value, contract};
use crate::oir2 as ir;
use okkama_sparrow::abi::{self, catalogue};
use okkama_sparrow::spb17 as s;

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
    let (mut admitted, mut rejected, mut drivers) = (0, 0, 0);
    for &native in abi::Native::ALL {
        let signature = declared(&mut builder, native);
        let result = contract(builder.module(), native.symbol(), true, &signature);
        match catalogue::admitted(native.symbol()) {
            Ok(_) => {
                let summary = result.unwrap_or_else(|error| panic!("{}: {}", native.symbol(), error.message));
                assert_eq!(summary.entry_kind, ir::EntryKind::CallBoundary);
                // An async native's contract is its driver's body summary (#601 N1): it suspends and can be cancelled.
                let asynchronous = native.signature().is_async;
                let mut failures = vec![ir::FailureKind::Program, ir::FailureKind::Native];
                if asynchronous {
                    failures.push(ir::FailureKind::Cancelled);
                    drivers += 1;
                }
                assert_eq!(summary.contract.failures, failures);
                assert!(summary.contract.suspends == asynchronous && !summary.contract.async_destroy);
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
    // #601 N1: the 51 async natives of std `runtime/`.
    assert_eq!(drivers, 51);
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

/// #824: `runtime/database`'s natives take `@DbCellState`, which `runtime/db` declares. A table handle the native's
/// module does not declare names the only runtime module's handle of that name, as Tomato matches it by name.
#[test]
fn a_table_handle_of_another_runtime_module_is_its_only_declaration() {
    let name = "runtime/database@1::sqlite_execution_cells/3";
    let declare = |cells: &[&str]| {
        let mut builder = builder();
        let state = handle(&mut builder, "runtime.database.SqliteState");
        let reply = handle(&mut builder, "runtime.database.SqliteReplyState");
        let string = builder.intern_type(ir::Type::Str).unwrap();
        let mut types: Vec<_> = cells.iter().map(|cells| handle(&mut builder, cells)).collect();
        let cells = builder.intern_type(ir::Type::Array(types.remove(0))).unwrap();
        (builder, signature(vec![state, string, cells], vec![reply]))
    };
    let (builder, declared) = declare(&["runtime.db.DbCellState"]);
    assert!(contract(builder.module(), name, true, &declared).is_ok());
    // Two runtime modules declare it: the name alone is ambiguous.
    let (builder, declared) = declare(&["runtime.db.DbCellState", "runtime.postgres.DbCellState"]);
    assert!(contract(builder.module(), name, true, &declared).is_err());
    // The native's own module declares it: that one, not another module's.
    let (builder, declared) = declare(&["runtime.db.DbCellState", "runtime.database.DbCellState"]);
    assert!(contract(builder.module(), name, true, &declared).is_err());
    // Not a runtime module's handle.
    let (builder, declared) = declare(&["app.DbCellState"]);
    assert!(contract(builder.module(), name, true, &declared).is_err());
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
    // #601 N1: an async native is admitted with its driver's body summary.
    let summary = contract(module, "runtime/application@1::terminal_standard/1", true,
        &signature(vec![int], vec![terminal])).unwrap();
    assert!(summary.contract.suspends && summary.contract.failures.contains(&ir::FailureKind::Cancelled));
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
    assert_eq!(summary.contract.failures, vec![ir::FailureKind::Program, ir::FailureKind::Native]);
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

// --- #601 N1: async natives in the producer and through the pipeline ----------------------------------------------

/// `SystemClock.wait` awaits the async native `sleep_nanoseconds` (std `time/clock`).
const SLEEP: &str = concat!(
    "module app; import \"time/clock\" take SystemClock; import \"time/duration\" take Duration; ",
    "async main() is int { await SystemClock().wait(Duration(milliseconds: 1)); 0; }",
);
const SLEEP_SYMBOL: &str = "runtime/time@1::sleep_nanoseconds/1";

fn async_native<'m>(module: &'m ir::Module, name: &str) -> (ir::FuncId, &'m ir::Function) {
    module.functions.iter().find(|(_, function)| function.attrs.iter().any(|attr| {
        matches!(attr, ir::FunctionAttr::Extern(found) if found == name)
    })).unwrap_or_else(|| panic!("{name} is declared"))
}

#[test]
fn n1_an_async_native_is_a_bodyless_async_extern_called_as_an_async_action() {
    let module = super::tests::lower_std(SLEEP).unwrap_or_else(|error| panic!("{error}"));
    super::tests::verify_high(&module, SLEEP);
    let (id, function) = async_native(&module, SLEEP_SYMBOL);
    // D22: `[async, extern]`, no body, the High creation envelope and the catalogue's body summary.
    assert!(function.body.is_none() && function.attrs.contains(&ir::FunctionAttr::Async));
    assert_eq!(function.summary, None);
    let body = module.summaries.get(function.async_body_summary.unwrap()).unwrap();
    assert!(body.contract.suspends);
    assert_eq!(body.contract.failures, [ir::FailureKind::Program, ir::FailureKind::Native, ir::FailureKind::Cancelled]);
    let creation = module.summaries.get(function.async_creation_summary.unwrap()).unwrap();
    assert!(!creation.contract.suspends, "creating a driver's Future runs nothing");
    // The call is an ordinary async call (a direct call whose result is the Future), never `call.native`.
    let mut direct = 0;
    for (_, caller) in module.functions.iter() {
        let Some(body) = &caller.body else { continue };
        for (_, instruction) in body.insts.iter() {
            match &instruction.op {
                ir::Operation::Call(ir::CallOp::Direct { function, .. }) if *function == id => direct += 1,
                ir::Operation::Call(ir::CallOp::Native { function, .. }) => assert_ne!(*function, id),
                _ => {}
            }
        }
    }
    assert_eq!(direct, 1);
}

#[test]
fn n1_an_async_native_builds_to_an_spb17_driver_with_an_empty_inputs_packet() {
    let high = super::tests::lower_std(SLEEP).unwrap_or_else(|error| panic!("{error}"));
    let bytes = super::pipeline::compile_high_with(high, ir::verify::VerifyMode::Structural)
        .unwrap_or_else(|error| panic!("{error}"));
    let program = s::decode(&bytes).unwrap();
    let contracts = s::p1::P1Contracts::new(&program.objects).unwrap();
    s::verify_load(&program, &contracts).unwrap_or_else(|error| panic!("{error}"));
    let (index, function) = program.functions.iter().enumerate().find(|(_, function)| {
        function.proofs.entry.as_ref().and_then(|entry| entry.external.as_deref()) == Some(SLEEP_SYMBOL)
    }).unwrap();
    // The trusted driver of the table (`p1`): its string argument in its register, no service, NATIVE_FAILURES.
    let mut named = function.clone();
    named.symbol = SLEEP_SYMBOL.into();
    assert_eq!(Some(&named), contracts.native(SLEEP_SYMBOL));
    assert_eq!((&function.entry_abi, function.registers.as_slice(), function.results.as_slice()),
        (&s::EntryAbi::AsyncExternal, &[s::Type::Str][..], &[s::Type::Bool][..]));
    assert_eq!(function.proofs.entry.as_ref().unwrap().failures, s::p1::NATIVE_FAILURES);
    let created = program.functions.iter().flat_map(|function| &function.code).any(|instruction| matches!(
        instruction, s::Instruction::Champ { op: s::Opcode::FutureNew, arguments, .. }
            if arguments.contains(&s::Argument::Entry(index as u32))));
    assert!(created, "fut.new names the driver");
}

#[test]
fn n1_an_async_native_must_be_an_async_native_of_the_table() {
    // A symbol outside the table has no driver (a synchronous one takes the unresolved-provider adapter).
    let source = "module app; native async later(n is int) is int; async main() is int { await later(1); }";
    let error = super::tests::lower_std(source).unwrap_err();
    assert!(error.to_string().contains("has no driver"), "{error}");
    // The catalogue: an async native's declaration is `[async]` exactly when the table's native is async.
    type Types = fn(&mut ir::ModuleBuilder) -> (Vec<ir::TypeId>, ir::TypeId);
    let declare = |name: &str, types: Types, asynchronous: bool| {
        let mut builder = builder();
        let (params, result) = types(&mut builder);
        let mut attrs = vec![ir::FunctionAttr::Extern(name.into())];
        if asynchronous {
            attrs.insert(0, ir::FunctionAttr::Async);
        }
        builder.declare_function(ir::Function {
            symbol: "f".into(), signature: signature(params, vec![result]), attrs, summary: None,
            type_params: vec![], body: None, loc: None, async_creation_summary: None, async_body_summary: None,
        }).unwrap();
        super::native_summaries::P1NativeSummaries::for_module(builder.module()).map(|_| ())
    };
    let sleep: Types = |builder| {
        (vec![builder.intern_type(ir::Type::Str).unwrap()], builder.intern_type(ir::Type::Bool).unwrap())
    };
    let hex: Types = |builder| {
        let byte = builder.intern_type(ir::Type::Byte).unwrap();
        (vec![builder.intern_type(ir::Type::Array(byte)).unwrap()], builder.intern_type(ir::Type::Str).unwrap())
    };
    assert!(declare(SLEEP_SYMBOL, sleep, true).is_ok());
    assert!(declare("runtime/hex@1::hex_encode/1", hex, false).is_ok());
    for (name, types, asynchronous) in [(SLEEP_SYMBOL, sleep, false), ("runtime/hex@1::hex_encode/1", hex, true)] {
        let error = declare(name, types, asynchronous).unwrap_err();
        assert!(error.message.contains("async differs from the runtime table"), "{name}: {}", error.message);
    }
}
