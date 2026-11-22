//! Верификатор: ссылки, регистры, переходы, обработчики и инициализация.
mod common;
use common::*;
use okkama_sparrow::{FuncRef, Handler, Instruction as I, ObjectRef, Target};

fn message(program: &okkama_sparrow::Program) -> String {
    program.verify().expect_err("program must be rejected").to_string()
}

#[test]
fn accepts_recursion_branches_and_register_reuse() {
    assert_eq!(sample().verify(), Ok(()));
}

#[test]
fn rejects_out_of_bounds_references() {
    let mut program = sample();
    program.functions[0].code[0] = I::Const { dst: d(9), value: int("1") };
    assert!(message(&program).contains("register r9 is out of bounds"), "{}", message(&program));

    let mut program = sample();
    program.functions[0].code[1] = I::Call { dst: Some(d(1)), function: FuncRef(7), args: vec![r(0)] };
    assert!(message(&program).contains("function f7 is out of bounds"));

    let mut program = sample();
    program.functions[1].code[2] = I::Branch { condition: r(2), then_target: to(3), else_target: to(40) };
    assert!(message(&program).contains("jump target @40 is out of bounds"));

    let mut program = sample();
    program.functions[0].code[0] = I::ObjNew { dst: d(0), object: ObjectRef(0), fields: vec![] };
    assert!(message(&program).contains("object obj0 is out of bounds"));

    let mut program = sample();
    program.entry = Some(FuncRef(5));
    assert!(message(&program).contains("function f5 is out of bounds"));
}

#[test]
fn rejects_reads_of_registers_not_written_on_every_path() {
    let mut program = sample();
    // r3 записан только на ветке else, а читается после слияния.
    program.functions[1].code = vec![
        I::Const { dst: d(1), value: int("1") },
        I::IntCmp { op: okkama_sparrow::Cmp::Le, dst: d(2), left: r(0), right: r(1) },
        I::Branch { condition: r(2), then_target: to(4), else_target: to(3) },
        I::IntSub { dst: d(3), left: r(0), right: r(1) },
        I::Return { value: Some(r(3)) },
    ];
    assert!(message(&program).contains("register r3 may be read before it is written"), "{}", message(&program));
    // Присваивание на ребре делает регистр записанным.
    program.functions[1].code[2] = I::Branch {
        condition: r(2),
        then_target: Target { pc: 4, moves: vec![(r(3), r(0))] },
        else_target: to(3),
    };
    assert_eq!(program.verify(), Ok(()));
}

#[test]
fn rejects_falling_off_the_code_and_duplicate_edge_assignments() {
    let mut program = sample();
    program.functions[0].code.pop();
    assert!(message(&program).contains("falls off the end"));

    let mut program = sample();
    program.functions[1].code[2] = I::Branch {
        condition: r(2),
        then_target: Target { pc: 3, moves: vec![(r(3), r(0)), (r(3), r(1))] },
        else_target: to(4),
    };
    assert!(message(&program).contains("assigned twice"));
}

#[test]
fn handlers_must_be_ordered_and_define_their_failure_register() {
    let mut program = sample();
    let function = &mut program.functions[0];
    function.registers.push(okkama_sparrow::TypeRef(3));
    function.code.push(I::Raise { failure: r(3) });
    function.handlers = vec![Handler { start: 0, end: 3, target: to(3), failure: r(3) }];
    assert_eq!(program.verify(), Ok(()));
    program.functions[0].handlers = vec![
        Handler { start: 1, end: 3, target: to(3), failure: r(3) },
        Handler { start: 0, end: 2, target: to(3), failure: r(3) },
    ];
    assert!(message(&program).contains("sorted and disjoint"));
    program.functions[0].handlers = vec![Handler { start: 2, end: 9, target: to(3), failure: r(3) }];
    assert!(message(&program).contains("out of bounds"));
}

#[test]
fn bodiless_functions_have_no_code() {
    let mut program = sample();
    // Символ без provider: вызов — ошибка исполнения, сигнатуру сверять не с чем.
    program.functions[1].native = Some("app/missing@1::factorial/1".into());
    assert!(message(&program).contains("native and intrinsic functions have no code"));
    program.functions[1].code.clear();
    assert_eq!(program.verify(), Ok(()));
}

#[test]
fn rejects_false_register_types_and_signatures() {
    // Ложный тип регистра: int.cmp пишет bool в регистр str.
    let mut program = sample();
    program.functions[1].registers[2] = okkama_sparrow::TypeRef(2);
    assert!(message(&program).contains("str cannot hold the bool result of `int.cmp`"), "{}", message(&program));

    // Лишний и недостающий аргумент вызова.
    let mut program = sample();
    program.functions[0].code[1] = I::Call { dst: Some(d(1)), function: FuncRef(1), args: vec![r(0), r(0)] };
    assert!(message(&program).contains("passes 2 arguments for 1 parameters"), "{}", message(&program));
    program.functions[0].code[1] = I::Call { dst: Some(d(1)), function: FuncRef(1), args: vec![] };
    assert!(message(&program).contains("passes 0 arguments for 1 parameters"), "{}", message(&program));

    // Аргумент не того типа.
    let mut program = sample();
    program.functions[0].registers[0] = okkama_sparrow::TypeRef(2);
    program.functions[0].code[0] = I::Const { dst: d(0), value: okkama_sparrow::Constant::Str("10".into()) };
    assert!(message(&program).contains("r0 has type str, expected int"), "{}", message(&program));

    // Результат функции не того типа: `ret` bool из функции int.
    let mut program = sample();
    program.functions[1].code[3] = I::Return { value: Some(r(2)) };
    assert!(message(&program).contains("r2 has type bool, expected int"), "{}", message(&program));

    // Результат вызова в регистр другого типа.
    let mut program = sample();
    program.functions[0].registers[1] = okkama_sparrow::TypeRef(1);
    assert!(message(&program).contains("bool cannot hold the int result of `call of @factorial`"), "{}", message(&program));

    // Перемещение на ребре между регистрами разных типов.
    let mut program = sample();
    program.functions[1].code[2] = I::Branch {
        condition: r(2),
        then_target: Target { pc: 3, moves: vec![(r(1), r(2))] },
        else_target: to(4),
    };
    assert!(message(&program).contains("branch passes r2: bool for r1: int"), "{}", message(&program));

    // Условие перехода — не bool.
    let mut program = sample();
    program.functions[1].code[2] = I::Branch { condition: r(1), then_target: to(3), else_target: to(4) };
    assert!(message(&program).contains("r1 has type int, expected bool"), "{}", message(&program));

    // Регистр отказа обработчика — не failure.
    let mut program = sample();
    let function = &mut program.functions[0];
    function.registers.push(okkama_sparrow::TypeRef(0));
    function.code.push(I::Return { value: Some(r(3)) });
    function.handlers = vec![Handler { start: 0, end: 3, target: to(3), failure: r(3) }];
    assert!(message(&program).contains("r3 has type int, expected failure"), "{}", message(&program));
}

#[test]
fn rejects_runtime_tables_with_false_signatures() {
    // Native: сигнатура не совпадает с ABI provider (`(@byte) -> int`).
    let mut program = sample();
    let mut native = function("runtime.write", 1, vec![0], true, Vec::new());
    native.native = Some("runtime/stdio@1::stdout_write/1".into());
    program.functions.push(native);
    assert!(message(&program).contains("does not match the signature of native"), "{}", message(&program));

    // Точка входа с параметром и точка входа не int.
    let mut program = sample();
    program.entry = Some(FuncRef(1));
    assert!(message(&program).contains("entry @factorial takes parameters"), "{}", message(&program));
    let mut program = sample();
    let mut text = function("main", 1, vec![], true, vec![
        I::Const { dst: d(0), value: okkama_sparrow::Constant::Str("0".into()) },
        I::Return { value: Some(r(0)) },
    ]);
    text.registers[0] = okkama_sparrow::TypeRef(2);
    text.result = Some(okkama_sparrow::TypeRef(2));
    program.functions[0] = text;
    assert!(message(&program).contains("entry @main must return int"), "{}", message(&program));

    // destroy объекта принимает не его.
    let mut program = sample();
    program.types.push(okkama_sparrow::Type::Object(ObjectRef(0)));
    program.objects = vec![okkama_sparrow::ObjectDecl {
        name: "app::Thing".into(),
        fields: vec![],
        does: vec![],
        destroy: Some(FuncRef(1)),
        builtin: false,
    }];
    assert!(message(&program).contains("is not a destroy of app::Thing"), "{}", message(&program));
}

/// `variant Reading = (empty, value(item: int))` (#290): `main` builds
/// `value(7)`, reads its case and its payload.
fn variant_program() -> okkama_sparrow::Program {
    use okkama_sparrow::{ObjectDecl, Type, TypeRef, VariantDecl, VariantRef};
    let main = function(
        "main",
        4,
        vec![],
        true,
        vec![
            I::Const { dst: d(0), value: int("7") },
            I::VariantNew { dst: d(1), variant: VariantRef(0), case: 1, fields: vec![r(0)] },
            I::VariantTag { dst: d(2), value: r(1) },
            I::VariantGet { dst: d(3), value: r(1), case: 1, field: 0 },
            I::Return { value: Some(r(3)) },
        ],
    );
    let mut main = main;
    main.registers[1] = TypeRef(4);
    let mut program = program(vec![main]);
    program.types.push(Type::Variant(VariantRef(0)));
    let object = |name: &str, fields: Vec<(String, TypeRef)>| ObjectDecl {
        name: name.into(),
        fields,
        does: Vec::new(),
        destroy: None,
        builtin: false,
    };
    program.objects = vec![object("Reading.empty", vec![]), object("Reading.value", vec![("item".into(), TypeRef(0))])];
    program.variants = vec![VariantDecl {
        name: "Reading".into(),
        cases: vec![("empty".into(), ObjectRef(0)), ("value".into(), ObjectRef(1))],
    }];
    program
}

#[test]
fn variant_accesses_must_match_the_case_layouts() {
    use okkama_sparrow::VariantRef;
    assert_eq!(variant_program().verify(), Ok(()));

    let mut program = variant_program();
    program.functions[0].code[3] = I::VariantGet { dst: d(3), value: r(1), case: 0, field: 0 };
    assert!(message(&program).contains("object obj0 has no field 0"), "{}", message(&program));

    let mut program = variant_program();
    program.functions[0].code[3] = I::VariantGet { dst: d(3), value: r(1), case: 2, field: 0 };
    assert!(message(&program).contains("variant variant0 has no case 2"), "{}", message(&program));

    let mut program = variant_program();
    program.functions[0].code[1] = I::VariantNew { dst: d(1), variant: VariantRef(0), case: 0, fields: vec![r(0)] };
    assert!(message(&program).contains("variant.new field count does not match the case"), "{}", message(&program));

    let mut program = variant_program();
    program.functions[0].code[2] = I::VariantTag { dst: d(2), value: r(0) };
    assert!(message(&program).contains("expected a variant"), "{}", message(&program));

    let mut program = variant_program();
    program.functions[0].code[1] = I::VariantOf { dst: d(1), source: r(0), variant: VariantRef(0) };
    assert!(message(&program).contains("expected an object"), "{}", message(&program));

    let mut program = variant_program();
    program.functions[0].code[1] = I::VariantNew { dst: d(1), variant: VariantRef(3), case: 0, fields: vec![] };
    assert!(message(&program).contains("variant variant3 is out of bounds"), "{}", message(&program));

    // Каждый случай — свой объект раскладки.
    let mut program = variant_program();
    program.variants[0].cases[0].1 = ObjectRef(1);
    assert!(message(&program).contains("needs its own plain object"), "{}", message(&program));
}

/// #974: a global's type outside the type table is a verification error, not a panic in typing.
#[test]
fn rejects_a_global_whose_type_is_out_of_bounds() {
    let text = "sparrow 16\nmodule \"bad\" \"1\"\ntype t0 int\nglobal g0 t99 f0\n\
                function f0 \"main\" { registers [t0] params [] result t0  0: const r0, int 0  1: ret r0 }\nentry f0\n";
    let error = okkama_sparrow::assemble(text).expect_err("must be rejected").to_string();
    assert!(error.contains("global g0 has type t99, which is out of bounds"), "{error}");
}

/// #974: the contracts of floats, `val.cmp`, `resource.settle` and `destroy`.
#[test]
fn rejects_nonfinite_floats_ordered_value_comparisons_sync_resource_settles_and_destroys_with_results() {
    for bits in [0x7ff8_0000_0000_0000_u64, 0x7ff0_0000_0000_0000, 0xfff0_0000_0000_0000] {
        let mut program = sample();
        program.functions[0].code[0] = I::Const { dst: d(0), value: okkama_sparrow::Constant::Float(bits) };
        assert!(message(&program).contains("is not finite"), "{}", message(&program));
    }

    let mut program = sample();
    program.functions[0].code[1] =
        I::ValCmp { op: okkama_sparrow::Cmp::Lt, dst: d(1), left: r(0), right: r(0) };
    assert!(message(&program).contains("val.cmp lt is not an equality comparison"), "{}", message(&program));

    for instruction in [
        I::ResourceSettle { value: r(0) },
        I::ResourceSettleUnwind { dst: d(1), value: r(0), failure: r(0) },
    ] {
        let mut program = sample();
        program.functions[0].code[1] = instruction;
        program.functions[0].code[2] = I::Return { value: Some(r(0)) };
        assert!(message(&program).contains("is allowed only in async functions"), "{}", message(&program));
    }

    // A destroy that takes its object but returns a value.
    let mut program = sample();
    program.types.push(okkama_sparrow::Type::Object(ObjectRef(0)));
    let object = okkama_sparrow::TypeRef(program.types.len() as u32 - 1);
    let mut destroy = function("drop", 2, vec![0], true, vec![
        I::Const { dst: d(1), value: int("0") },
        I::Return { value: Some(r(1)) },
    ]);
    destroy.registers[0] = object;
    program.functions.push(destroy);
    program.objects = vec![okkama_sparrow::ObjectDecl {
        name: "app::Thing".into(),
        fields: vec![],
        does: vec![],
        destroy: Some(FuncRef(2)),
        builtin: false,
    }];
    assert!(message(&program).contains("is not a destroy of app::Thing"), "{}", message(&program));
    // The same destroy without a result is accepted.
    program.functions[2].result = None;
    program.functions[2].code[1] = I::Return { value: None };
    assert_eq!(program.verify(), Ok(()));
}

/// #976: native ABI types name a handle by its short name, so a program may not hold two
/// modules' handles of one name; distinct names, or the same handle once, are fine.
#[test]
fn handles_of_one_short_name_from_two_modules_are_rejected() {
    let mut program = sample();
    program.handles = vec!["runtime.buffer::State".into(), "runtime.other::State".into()];
    assert!(message(&program).contains("share the name State"), "{}", message(&program));
    program.handles = vec!["runtime.buffer::State".into(), "runtime.other::Cursor".into()];
    assert_eq!(program.verify(), Ok(()));
}
