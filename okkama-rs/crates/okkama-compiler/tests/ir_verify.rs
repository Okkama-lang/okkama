//! OIR-2 verifier rules on hand-built modules.

use okkama_compiler::Span;
use okkama_compiler::ir::{
    self, Attr, Block, BlockId, CompareOp, Constant, FuncId, Function, Inst, InstKind, IntValue,
    Module, ResultType, Target, TermKind, Terminator, Type, ValueId,
};

fn inst(result: usize, kind: InstKind) -> Inst {
    Inst {
        result: Some(ValueId(result)),
        kind,
        span: Span::empty(0),
    }
}

fn int(value: i64) -> InstKind {
    InstKind::Const(Constant::Int(IntValue::from_i64(value)))
}

fn term(kind: TermKind) -> Terminator {
    Terminator {
        kind,
        span: Span::empty(0),
    }
}

fn block(insts: Vec<Inst>, kind: TermKind) -> Block {
    Block {
        params: Vec::new(),
        unwind: None,
        insts,
        term: term(kind),
    }
}

fn module(values: Vec<Type>, blocks: Vec<Block>) -> Module {
    Module {
        name: "app".to_string(),
        version: "v1.0.0".to_string(),
        imports: Vec::new(),
        enums: Vec::new(),
        roles: Vec::new(),
        objects: Vec::new(),
        variants: Vec::new(),
        handles: Vec::new(),
        caps: Vec::new(),
        globals: Vec::new(),
        tests: None,
        functions: vec![Function {
            symbol: "main".to_string(),
            params: Vec::new(),
            result: ResultType::Value(Type::Int),
            attrs: vec![Attr::Entry],
            values,
            blocks,
            span: Span::empty(0),
        }],
        entry: Some(FuncId(0)),
        span: Span::empty(0),
        debug: Default::default(),
    }
}

/// `if %cond { 1 } else { 2 }` с параметром слияния: корректная форма.
fn diamond() -> Module {
    module(
        vec![Type::Bool, Type::Int, Type::Int, Type::Int],
        vec![
            block(
                vec![inst(0, InstKind::Const(Constant::Bool(true)))],
                TermKind::Cond {
                    condition: ValueId(0),
                    then_target: Target::new(BlockId(1)),
                    else_target: Target::new(BlockId(2)),
                },
            ),
            block(
                vec![inst(1, int(1))],
                TermKind::Br(Target {
                    block: BlockId(3),
                    args: vec![ValueId(1)],
                }),
            ),
            block(
                vec![inst(2, int(2))],
                TermKind::Br(Target {
                    block: BlockId(3),
                    args: vec![ValueId(2)],
                }),
            ),
            Block {
                params: vec![ValueId(3)],
                unwind: None,
                insts: Vec::new(),
                term: term(TermKind::Ret(Some(ValueId(3)))),
            },
        ],
    )
}

fn rejects(module: &Module, expected: &str) {
    let error = ir::verify(module).expect_err(expected);
    assert!(
        error.to_string().contains(expected),
        "expected `{expected}`, got `{error}`"
    );
}

#[test]
fn a_well_formed_diamond_verifies() {
    ir::verify(&diamond()).unwrap();
}

#[test]
fn uses_must_be_dominated_by_their_definitions() {
    let mut module = diamond();
    // ^b2 читает %1 из соседней ветки ^b1.
    module.functions[0].blocks[2].term = term(TermKind::Br(Target {
        block: BlockId(3),
        args: vec![ValueId(1)],
    }));
    rejects(
        &module,
        "%1 is used in ^b2 where its definition does not dominate",
    );

    let mut module = diamond();
    // Использование до определения в том же блоке.
    module.functions[0].blocks[1].insts = vec![inst(
        1,
        InstKind::IntCompare {
            op: CompareOp::Eq,
            left: ValueId(1),
            right: ValueId(1),
        },
    )];
    rejects(&module, "does not dominate");
}

#[test]
fn values_are_defined_exactly_once_and_typed() {
    let mut module = diamond();
    module.functions[0].blocks[2].insts = vec![inst(1, int(2))];
    rejects(&module, "%1 is defined more than once");

    let mut module = diamond();
    module.functions[0].values.push(Type::Int);
    rejects(&module, "%4 is declared but never defined");

    let mut module = diamond();
    // Неиспользуемая константа int, объявленная как bool: нарушено только это правило.
    module.functions[0].values.push(Type::Bool);
    module.functions[0].blocks[1].insts.push(inst(4, int(5)));
    rejects(&module, "%4: bool cannot hold the int result");

    let mut module = diamond();
    module.functions[0].blocks[0].insts = vec![inst(0, int(1))];
    module.functions[0].values[0] = Type::Int;
    rejects(&module, "%0 has type int, expected bool");

    let mut module = diamond();
    module.functions[0].blocks[1].insts = vec![inst(
        1,
        InstKind::Const(Constant::Int(IntValue::from_decimal_digits("7").unwrap())),
    )];
    ir::verify(&module).unwrap();
}

#[test]
fn branches_must_match_target_parameters() {
    let mut module = diamond();
    module.functions[0].blocks[1].term = term(TermKind::Br(Target::new(BlockId(3))));
    rejects(&module, "passes 0 arguments for 1 parameters");

    let mut module = diamond();
    module.functions[0].blocks[1].term = term(TermKind::Br(Target {
        block: BlockId(3),
        args: vec![ValueId(0)],
    }));
    rejects(&module, "passes %0: bool for %3: int");

    let mut module = diamond();
    module.functions[0].blocks[1].term = term(TermKind::Br(Target::new(BlockId(9))));
    rejects(&module, "branch to missing block ^b9");

    let mut module = diamond();
    module.functions[0].blocks[1].term = term(TermKind::Br(Target::new(BlockId(0))));
    rejects(&module, "the entry block cannot be a branch target");
}

#[test]
fn every_block_must_be_reachable() {
    let mut module = diamond();
    module.functions[0]
        .blocks
        .push(block(Vec::new(), TermKind::Unreachable));
    rejects(&module, "block ^b4 is unreachable");
}

#[test]
fn results_and_entry_signature_are_checked() {
    let mut module = diamond();
    module.functions[0].blocks[3].term = term(TermKind::Ret(None));
    rejects(&module, "`ret` without a value");

    let mut module = diamond();
    module.functions[0].result = ResultType::Void;
    rejects(&module, "entry @main must have signature () -> int");

    let mut module = diamond();
    module.functions[0].attrs.clear();
    rejects(
        &module,
        "entry attribute must mark exactly the module entry",
    );

    let mut module = diamond();
    module.functions.push(module.functions[0].clone());
    module.functions[1].attrs.clear();
    rejects(&module, "duplicate function symbol @main");

    let mut module = diamond();
    module.functions[0].blocks[3].term = term(TermKind::Panic(ValueId(3)));
    rejects(&module, "%3 has type int, expected str");
}

fn lowered(text: &str) -> Module {
    use okkama_compiler::SourceFile;
    use okkama_compiler::lowering::lower_program;
    use okkama_compiler::syntax::{self, ParseOptions};
    let source = SourceFile::new("test.ok", text).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    lower_program(&program).unwrap()
}

const OBJECTS: &str = "module app v1.0.0;
object C {
    has n is int = 0;
    C() {}
    inc() is ^C { ^n = n + 1; }
    value() is int { n; }
    hidden secret() is int { n; }
}
main() is int { let d = C().inc(); d.value(); }";

fn symbol(module: &Module, name: &str) -> usize {
    module
        .functions
        .iter()
        .position(|function| function.symbol == name)
        .unwrap_or_else(|| panic!("@{name}"))
}

#[test]
fn object_instructions_require_the_owner_authority() {
    let mut module = lowered(OBJECTS);
    ir::verify(&module).unwrap();
    let value = symbol(&module, "C.value");
    module.functions[value].attrs = Vec::new();
    rejects(
        &module,
        "obj.get is allowed only in functions of its object",
    );

    let mut module = lowered(OBJECTS);
    let ctor = symbol(&module, "C.new#0");
    let block = &mut module.functions[ctor].blocks[0];
    let InstKind::ObjNew { fields, .. } = &mut block.insts.last_mut().unwrap().kind else {
        panic!("constructor ends with obj.new")
    };
    fields.clear();
    rejects(&module, "obj.new C lists 0 fields, the object declares 1");

    let mut module = lowered(OBJECTS);
    let inc = symbol(&module, "C.inc");
    module.functions[inc].attrs = vec![Attr::Method(ir::ObjectId(0))];
    rejects(&module, "a method returning ^T must be marked `successor`");
}

#[test]
fn calls_check_arguments_and_privacy() {
    let mut module = lowered(OBJECTS);
    let main = symbol(&module, "main");
    let secret = symbol(&module, "C.secret");
    let value = symbol(&module, "C.value");
    for inst in &mut module.functions[main].blocks[0].insts {
        if let InstKind::Call { func, .. } = &mut inst.kind
            && func.0 == value {
                *func = FuncId(secret);
            }
    }
    rejects(&module, "@C.secret is hidden in its object");

    let mut module = lowered(OBJECTS);
    let main = symbol(&module, "main");
    for inst in &mut module.functions[main].blocks[0].insts {
        if let InstKind::Call { args, .. } = &mut inst.kind
            && !args.is_empty() {
                args.clear();
            }
    }
    rejects(&module, "passes 0 arguments for 1 parameters");
}

#[test]
fn successor_results_must_be_taken_on_every_normal_path() {
    let mut module = lowered(OBJECTS);
    let main = symbol(&module, "main");
    let (ctor, inc) = (symbol(&module, "C.new#0"), symbol(&module, "C.inc"));
    let object = Type::Object(ir::ObjectId(0));
    // %1: ^C получен и брошен: обязательство доходит до `ret`.
    let function = &mut module.functions[main];
    function.values = vec![object.clone(), Type::Successor(ir::ObjectId(0)), Type::Int];
    function.blocks = vec![block(
        vec![
            inst(
                0,
                InstKind::Call {
                    func: FuncId(ctor),
                    args: vec![],
                },
            ),
            inst(
                1,
                InstKind::Call {
                    func: FuncId(inc),
                    args: vec![ValueId(0)],
                },
            ),
            inst(2, int(0)),
        ],
        TermKind::Ret(Some(ValueId(2))),
    )];
    rejects(
        &module,
        "successor value %1 reaches `ret` in ^b0 without succ.take",
    );

    // Погашение на одном пути не покрывает другой.
    let function = &mut module.functions[main];
    function.values.extend([Type::Bool, object]);
    function.blocks = vec![
        block(
            vec![
                inst(
                    0,
                    InstKind::Call {
                        func: FuncId(ctor),
                        args: vec![],
                    },
                ),
                inst(
                    1,
                    InstKind::Call {
                        func: FuncId(inc),
                        args: vec![ValueId(0)],
                    },
                ),
                inst(2, int(0)),
                inst(3, InstKind::Const(Constant::Bool(true))),
            ],
            TermKind::Cond {
                condition: ValueId(3),
                then_target: Target::new(BlockId(1)),
                else_target: Target::new(BlockId(2)),
            },
        ),
        block(
            vec![inst(4, InstKind::SuccTake(ValueId(1)))],
            TermKind::Ret(Some(ValueId(2))),
        ),
        block(Vec::new(), TermKind::Ret(Some(ValueId(2)))),
    ];
    rejects(
        &module,
        "successor value %1 reaches `ret` in ^b2 without succ.take",
    );
}

const FAILURES: &str = "module app v1.0.0;
enum Boom is Error = (boom);
object C { has n is int = 0; C() {} inc() is ^C { ^n = n + 1; } value() is int { n; } }
main() is int {
    let c = C();
    let r = try { c = c.inc(); throw Boom.boom; } catch e is Boom { 7; };
    r + c.value();
}";

#[test]
fn unwind_arguments_must_exist_before_the_source_block_starts() {
    let mut module = lowered(FAILURES);
    ir::verify(&module).unwrap();
    let main = symbol(&module, "main");
    // ^b1 определяет %2 (succ.take) и сам не может передать его в свою цель unwind.
    let block = &mut module.functions[main].blocks[1];
    block.unwind.as_mut().unwrap().args = vec![ValueId(2)];
    rejects(
        &module,
        "%2 is passed to the unwind target of ^b1 but is not defined before the block starts",
    );

    let mut module = lowered(FAILURES);
    let main = symbol(&module, "main");
    module.functions[main].blocks[1]
        .unwind
        .as_mut()
        .unwrap()
        .args
        .clear();
    rejects(&module, "passes 1 arguments for 2 parameters");
}

#[test]
fn catch_handlers_receive_the_caught_type_and_throw_requires_error() {
    let mut module = lowered(FAILURES);
    let main = symbol(&module, "main");
    // Последний параметр handler должен иметь тип пойманного значения (error enum — вариант).
    let handler = module.functions[main].blocks[4].params[2];
    module.functions[main].values[handler.0] = Type::Int;
    rejects(
        &module,
        &format!("the last parameter {handler} of ^b4 must have type variant#0"),
    );

    let mut module = lowered(FAILURES);
    let main = symbol(&module, "main");
    assert!(matches!(
        module.functions[main].blocks[2].term.kind,
        TermKind::Throw(_)
    ));
    module.functions[main].blocks[2].term.kind = TermKind::Throw(ValueId(0));
    rejects(&module, "throw %0: object#0 does not do Error");
}

/// `%b` — сборщик из блока 0; `then`/`else` — тела блоков 1 и 2.
fn builder_branches(then_insts: Vec<Inst>, else_insts: Vec<Inst>) -> Module {
    let builder = Type::Builder(Box::new(Type::Int));
    let array = Type::Array(Box::new(Type::Int));
    module(
        vec![
            builder.clone(),
            Type::Int,
            Type::Bool,
            builder.clone(),
            array.clone(),
            Type::Int,
            builder,
            array,
            Type::Int,
        ],
        vec![
            block(
                vec![
                    inst(0, InstKind::BuilderNew),
                    inst(1, int(1)),
                    inst(2, InstKind::Const(Constant::Bool(true))),
                ],
                TermKind::Cond {
                    condition: ValueId(2),
                    then_target: Target::new(BlockId(1)),
                    else_target: Target::new(BlockId(2)),
                },
            ),
            block(then_insts, TermKind::Ret(Some(ValueId(5)))),
            block(else_insts, TermKind::Ret(Some(ValueId(8)))),
        ],
    )
}

#[test]
fn builders_are_used_at_most_once_on_every_path() {
    let push = |result, builder| {
        inst(
            result,
            InstKind::BuilderPush {
                builder: ValueId(builder),
                value: ValueId(1),
            },
        )
    };
    let finish = |result, builder, len| {
        [
            inst(result, InstKind::BuilderFinish(ValueId(builder))),
            inst(len, InstKind::ArrayLen(ValueId(result))),
        ]
    };
    // Взаимоисключающие пути: `%0` дописан в одной ветви и закрыт в другой.
    let mut then_insts = vec![push(3, 0)];
    then_insts.extend(finish(4, 3, 5));
    let mut else_insts = vec![push(6, 0)];
    else_insts.extend(finish(7, 6, 8));
    ir::verify(&builder_branches(then_insts, else_insts)).unwrap();

    // Два использования на одном пути.
    let mut reused = vec![push(3, 0), push(6, 0)];
    reused.extend(finish(4, 3, 5));
    rejects(
        &builder_branches(reused, finish(7, 0, 8).to_vec()),
        "builder %0 is used more than once on a path",
    );

    // Использование в цикле без параметра заголовка повторяется на каждой итерации.
    let builder = Type::Builder(Box::new(Type::Int));
    let looped = module(
        vec![builder.clone(), Type::Int, Type::Bool, builder, Type::Int],
        vec![
            block(
                vec![
                    inst(0, InstKind::BuilderNew),
                    inst(1, int(1)),
                    inst(2, InstKind::Const(Constant::Bool(true))),
                ],
                TermKind::Br(Target::new(BlockId(1))),
            ),
            block(
                vec![push(3, 0)],
                TermKind::Cond {
                    condition: ValueId(2),
                    then_target: Target::new(BlockId(1)),
                    else_target: Target::new(BlockId(2)),
                },
            ),
            block(vec![inst(4, int(0))], TermKind::Ret(Some(ValueId(4)))),
        ],
    );
    rejects(&looped, "builder %0 is used more than once on a path");
}

#[test]
fn map_keys_reject_arrays_but_allow_untyped_empty_maps() {
    let bad = Type::Map(
        Box::new(Type::Array(Box::new(Type::Int))),
        Box::new(Type::Int),
    );
    let module = module(
        vec![bad, Type::Int],
        vec![block(
            vec![inst(0, InstKind::MapNew(Vec::new())), inst(1, int(0))],
            TermKind::Ret(Some(ValueId(1))),
        )],
    );
    rejects(&module, "map key type array<int> is not a key type");

    let never = Box::new(Type::Never);
    let good = module_with_map(Type::Map(never.clone(), never));
    ir::verify(&good).unwrap();
}

fn module_with_map(ty: Type) -> Module {
    module(
        vec![ty, Type::Int],
        vec![block(
            vec![
                inst(0, InstKind::MapNew(Vec::new())),
                inst(1, InstKind::MapLen(ValueId(0))),
            ],
            TermKind::Ret(Some(ValueId(1))),
        )],
    )
}

/// `variant Reading = (empty, value(item: int))`: `main` builds `value(7)`,
/// reads its tag and its payload (#290).
fn variant_module() -> Module {
    let object = |name: &str, fields: Vec<ir::Field>| ir::Object {
        name: name.to_string(),
        fields,
        does: Vec::new(),
        destroy: None,
        builtin: false,
        span: Span::empty(0),
    };
    let reading = Type::Variant(ir::VariantId(0));
    let mut module = module(
        vec![Type::Int, reading, Type::Int, Type::Int],
        vec![block(
            vec![
                inst(0, int(7)),
                inst(1, InstKind::VariantNew { case: 1, fields: vec![ValueId(0)] }),
                inst(2, InstKind::VariantTag(ValueId(1))),
                inst(3, InstKind::VariantGet { value: ValueId(1), case: 1, field: 0 }),
            ],
            TermKind::Ret(Some(ValueId(3))),
        )],
    );
    module.objects = vec![
        object("Reading.empty", Vec::new()),
        object(
            "Reading.value",
            vec![ir::Field { name: "item".to_string(), ty: Type::Int, default: None, once: false }],
        ),
    ];
    module.variants = vec![ir::Variant {
        name: "Reading".to_string(),
        cases: vec![
            ("empty".to_string(), ir::ObjectId(0)),
            ("value".to_string(), ir::ObjectId(1)),
        ],
        span: Span::empty(0),
    }];
    module
}

#[test]
fn variant_instructions_follow_the_case_layouts() {
    ir::verify(&variant_module()).unwrap();

    let mut module = variant_module();
    module.functions[0].blocks[0].insts[1].kind = InstKind::VariantNew { case: 2, fields: vec![ValueId(0)] };
    rejects(&module, "variant Reading has no case #2");

    let mut module = variant_module();
    module.functions[0].blocks[0].insts[1].kind = InstKind::VariantNew { case: 0, fields: vec![ValueId(0)] };
    rejects(&module, "variant.new case 0 lists 1 fields, the case has 0");

    let mut module = variant_module();
    module.functions[0].blocks[0].insts[3].kind = InstKind::VariantGet { value: ValueId(1), case: 0, field: 0 };
    rejects(&module, "field");

    let mut module = variant_module();
    module.functions[0].blocks[0].insts[2].kind = InstKind::VariantTag(ValueId(0));
    rejects(&module, "expected a variant");

    // Каждый случай — своя раскладка: общий объект двух случаев отвергается.
    let mut module = variant_module();
    module.variants[0].cases[0].1 = ir::ObjectId(1);
    rejects(&module, "must be a plain layout of one case");
}

/// #1007: a role method's result and the types inside a function value or a
/// future are checked like any other type of the module.
#[test]
fn role_results_and_nested_types_must_exist() {
    let missing = || Type::Object(ir::ObjectId(9));
    let mut role = diamond();
    role.roles.push(ir::Role {
        name: "Shape".into(),
        methods: vec![ir::RoleMethod { name: "area".into(), params: Vec::new(), result: ResultType::Value(missing()) }],
        builtin: false,
        span: Span::empty(0),
    });
    rejects(&role, "missing object #9");

    for nested in [
        Type::Fn(Box::new(ir::FnSig { params: vec![missing()], result: ResultType::Void })),
        Type::Fn(Box::new(ir::FnSig { params: Vec::new(), result: ResultType::Value(missing()) })),
        Type::Future(Box::new(ResultType::Value(missing()))),
    ] {
        let mut module = diamond();
        module.functions[0].values.push(nested.clone());
        module.functions[0].blocks[0].insts.push(inst(4, InstKind::Undef));
        rejects(&module, "missing object #9");
    }
}

/// #1007: `undef` of a builder says what is wrong.
#[test]
fn undef_of_a_builder_names_the_violation() {
    let mut module = diamond();
    module.functions[0].values.push(Type::Builder(Box::new(Type::Int)));
    module.functions[0].blocks[0].insts.push(inst(4, InstKind::Undef));
    rejects(&module, "`undef` must not define a builder");
}

/// `main` of the #1063 report: `%1: ^C` goes to `^b1`, the `found` (or a map
/// lookup's) path, and to `^b2`, which takes it. `found_param` is the type of
/// `^b1`'s first parameter; with `take`, `^b1` takes it too.
fn successor_through_lookup(map: bool, found_param: Type, take: bool) -> Module {
    let mut module = lowered(OBJECTS);
    let main = symbol(&module, "main");
    let (ctor, inc) = (symbol(&module, "C.new#0"), symbol(&module, "C.inc"));
    let object = Type::Object(ir::ObjectId(0));
    let successor = Type::Successor(ir::ObjectId(0));
    let (container, new) = if map {
        (Type::Map(Box::new(Type::Int), Box::new(Type::Int)), InstKind::MapNew(Vec::new()))
    } else {
        (Type::Array(Box::new(Type::Int)), InstKind::ArrayNew(Vec::new()))
    };
    let (found, missing) = (
        Target { block: BlockId(1), args: vec![ValueId(1)] },
        Target { block: BlockId(2), args: vec![ValueId(1)] },
    );
    let lookup = if map {
        TermKind::MapLookup { map: ValueId(2), key: ValueId(3), found, missing }
    } else {
        TermKind::ArrayLookup { array: ValueId(2), index: ValueId(3), found, missing }
    };
    let function = &mut module.functions[main];
    function.values = vec![
        object.clone(),
        successor.clone(),
        container,
        Type::Int,
        found_param,
        Type::Int,
        successor,
        object.clone(),
        Type::Int,
    ];
    if take {
        function.values.push(object);
    }
    let mut found_block = block(Vec::new(), TermKind::Ret(Some(ValueId(5))));
    found_block.params = vec![ValueId(4), ValueId(5)];
    if take {
        found_block.insts.push(inst(9, InstKind::SuccTake(ValueId(4))));
    }
    let mut missing_block = block(
        vec![inst(7, InstKind::SuccTake(ValueId(6))), inst(8, int(0))],
        TermKind::Ret(Some(ValueId(8))),
    );
    missing_block.params = vec![ValueId(6)];
    function.blocks = vec![
        block(
            vec![
                inst(0, InstKind::Call { func: FuncId(ctor), args: vec![] }),
                inst(1, InstKind::Call { func: FuncId(inc), args: vec![ValueId(0)] }),
                inst(2, new),
                inst(3, int(0)),
            ],
            lookup,
        ),
        found_block,
        missing_block,
    ];
    module
}

/// #1063: the `found` path of a lookup is a normal path. A successor value
/// passed there keeps its obligation; only an `unwind` edge (cleanup) may pass
/// `^C` for `C`.
#[test]
fn a_lookup_found_edge_keeps_the_successor_obligation() {
    for map in [false, true] {
        let object = Type::Object(ir::ObjectId(0));
        let successor = Type::Successor(ir::ObjectId(0));
        rejects(
            &successor_through_lookup(map, object, false),
            "branch to ^b1 passes %1: ^object#0 for %4: object#0",
        );
        rejects(
            &successor_through_lookup(map, successor.clone(), false),
            "successor value %4 reaches `ret` in ^b1 without succ.take",
        );
        ir::verify(&successor_through_lookup(map, successor, true)).unwrap();
    }
}

/// #1063: the handler of `catch` is a normal path too. In FAILURES, `^b3`
/// (the unwind target) holds `c` and passes it to the handler `^b4` and to
/// `^b7`; made a successor there (an unwind edge may pass `C` for `^C`), it
/// may not reach the handler's plain parameter.
#[test]
fn a_catch_handler_edge_keeps_the_successor_obligation() {
    let module = lowered(FAILURES);
    let main = symbol(&module, "main");
    let blocks = &module.functions[main].blocks;
    assert!(matches!(blocks[3].term.kind, TermKind::Catch { .. }), "^b3 catches");
    let (held, handler_param, next_param) = (blocks[3].params[0], blocks[4].params[0], blocks[7].params[0]);
    let mut module = module;
    let values = &mut module.functions[main].values;
    values[held.0] = Type::Successor(ir::ObjectId(0));
    values[next_param.0] = Type::Successor(ir::ObjectId(0));
    rejects(
        &module,
        &format!("branch to ^b4 passes {held}: ^object#0 for {handler_param}: object#0"),
    );
}

const CAPS: &str = "module app v1.0.0;
object W { has n is int; W(:n); cap View { count is int { n; } } }
object V { V(); }
main() is int { W(n: 1)::View::count; }";

/// #1071: a cap declaration is checked where it is declared, not at the
/// first `call.cap` of a field (a field nobody reads was never checked).
#[test]
fn cap_fields_are_synchronous_methods_of_the_owner() {
    let module = lowered(CAPS);
    ir::verify(&module).unwrap();
    let field = module.caps[0].fields[0].1;

    let mut broken = module.clone();
    broken.caps[0].fields[0].1 = FuncId(9999);
    rejects(&broken, "cap W::View: field count refers to missing function #9999");

    let mut broken = module.clone();
    broken.caps[0].owner = ir::ObjectId(9999);
    rejects(&broken, "cap W::View: owner refers to missing object #9999");

    let mut broken = module.clone();
    let other = broken.objects.iter().position(|object| object.name == "V").expect("V is lowered");
    broken.caps[0].owner = ir::ObjectId(other);
    rejects(&broken, "cap W::View: field count: @W::View.count must be a synchronous `[method V]` function (V) -> value");

    let mut broken = module.clone();
    broken.functions[field.0].attrs.push(Attr::Async);
    rejects(&broken, "must be a synchronous `[method W]` function (W) -> value");

    let mut broken = module.clone();
    let extra = broken.caps[0].fields[0].clone();
    broken.caps[0].fields.push(extra);
    rejects(&broken, "cap W::View: declares field count twice");
}

const WORKERS: &str = "module app v1.0.0;
object W { has n is int; W(:n); f(x is int) is int { n + x; } }
async main() is int {
    let g is (int) is int = W(n: 1).f;
    let a = await spawn g(2);
    let b = await spawn W(n: 1).f(x: 2);
    a + b;
}";

/// #1071: `worker.bind`'s entry rebuilds the receiver from the listed fields
/// and takes the action's parameters after them, with the action's result.
#[test]
fn worker_bind_entry_takes_the_fields_then_the_action_parameters() {
    let module = lowered(WORKERS);
    ir::verify(&module).unwrap();
    let (function, block, position) = module
        .functions
        .iter()
        .enumerate()
        .find_map(|(index, function)| {
            function.blocks.iter().enumerate().find_map(|(block, body)| {
                body.insts
                    .iter()
                    .position(|inst| matches!(inst.kind, InstKind::WorkerBind { .. }))
                    .map(|position| (index, block, position))
            })
        })
        .expect("a bound action for the worker is lowered");

    let mut broken = module.clone();
    let InstKind::WorkerBind { fields, .. } = &mut broken.functions[function].blocks[block].insts[position].kind else {
        unreachable!()
    };
    fields.clear();
    rejects(&broken, "has type fn(int, int) -> int, expected fn(int) -> int for 0 fields and the action fn(int) -> int");

    // The operands swapped: the entry passed as the action and the other way round.
    let mut broken = module.clone();
    let InstKind::WorkerBind { action, entry, .. } = &mut broken.functions[function].blocks[block].insts[position].kind else {
        unreachable!()
    };
    std::mem::swap(action, entry);
    rejects(&broken, "expected fn(int, int, int) -> int for 1 fields and the action fn(int, int) -> int");
}

/// #1071: a worker runs a synchronous action; an entry returning a Future
/// would make `worker.spawn`'s result `future<future<R>>`.
#[test]
fn worker_spawn_requires_a_synchronous_entry() {
    let module = lowered(WORKERS);
    let main = symbol(&module, "main");
    let entries: Vec<(ValueId, usize)> = module.functions[main]
        .blocks
        .iter()
        .flat_map(|block| &block.insts)
        .filter_map(|inst| match &inst.kind {
            InstKind::WorkerSpawn { entry, direct: true, .. } => Some(*entry),
            _ => None,
        })
        .filter_map(|entry| {
            module.functions[main].blocks.iter().flat_map(|block| &block.insts).find_map(|inst| match inst.kind {
                InstKind::FnClosure { func, .. } if inst.result == Some(entry) => Some((entry, func.0)),
                _ => None,
            })
        })
        .collect();
    let [(entry, func)] = entries[..] else { panic!("one direct spawn: {entries:?}") };

    // The entry made async, its fn value typed as it then is: fn(…) -> future<int>.
    let mut broken = module.clone();
    broken.functions[func].attrs.push(Attr::Async);
    let Type::Fn(sig) = &mut broken.functions[main].values[entry.0] else { panic!("the entry is a fn value") };
    sig.result = ResultType::Value(Type::Future(Box::new(ResultType::Value(Type::Int))));
    rejects(&broken, &format!("worker.spawn of {entry}: the entry must be synchronous, it returns fn(int, int) -> future<int>"));
}

/// #1070: a rotation without its count and another bit action with one get their own messages.
#[test]
fn bit_actions_take_a_count_only_to_rotate() {
    let bits = |op, count| {
        module(
            vec![Type::Int, Type::Int],
            vec![block(
                vec![inst(0, int(1)), inst(1, InstKind::Bits { op, byte: false, value: ValueId(0), count })],
                TermKind::Ret(Some(ValueId(1))),
            )],
        )
    };
    ir::verify(&bits(ir::BitOp::BitCount, None)).expect("a bit count takes no count");
    rejects(&bits(ir::BitOp::RotateLeft, None), "int.rotl %0: a rotation needs a count");
    rejects(&bits(ir::BitOp::BitCount, Some(ValueId(0))), "only rotations take a count");
}
