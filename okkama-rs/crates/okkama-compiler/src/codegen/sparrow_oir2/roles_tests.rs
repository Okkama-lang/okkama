//! Role values and action values in SPB 17 (#676): role declarations and implementation tables, `fn`
//! roles, closure layouts with their trampolines, and the targets of `call.role` and `fn.call`.
use super::*;
use crate::lowering::{LoadedModule, ModuleLoader, RootFile};
use crate::syntax::{self, ParseOptions};
use crate::SourceFile;

struct StdLoader;

impl ModuleLoader for StdLoader {
    fn load(&mut self, path: &str, _: &str) -> Result<Option<LoadedModule>, String> {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../std").join(format!("{path}.ok"));
        let text = std::fs::read_to_string(&file).map_err(|error| error.to_string())?;
        let source = SourceFile::new(file.to_string_lossy(), text).map_err(|error| error.to_string())?;
        let program = syntax::parse(&source, &ParseOptions::default()).map_err(|error| format!("{error:?}"))?;
        let program = syntax::prune::strip_test_items(program);
        Ok(Some(LoadedModule { id: file.to_string_lossy().into_owned(), source, program }))
    }
}

/// The verified SPB 17 program of a single-file source.
fn compile(text: &str) -> s::Program {
    let source = SourceFile::new("app.ok", text).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let roots = [RootFile { source: &source, program: &program }];
    let bytes = crate::lowering::oir2::compile_linked(&roots, &mut StdLoader).unwrap_or_else(|error| panic!("{error}"));
    s::decode(&bytes).unwrap()
}

fn object(p: &s::Program, name: &str) -> u32 {
    p.objects.iter().position(|object| object.name == name).unwrap_or_else(|| panic!("no descriptor {name}")) as u32
}

fn function(p: &s::Program, symbol: &str) -> usize {
    p.functions.iter().position(|function| function.symbol == symbol).unwrap_or_else(|| panic!("no function {symbol}"))
}

fn dispatches(f: &s::Function) -> Vec<(s::Opcode, s::Dispatch)> {
    f.code.iter().filter_map(|instruction| match instruction {
        s::Instruction::Champ { op, arguments, .. } => match arguments.first() {
            Some(s::Argument::Dispatch(dispatch)) => Some((*op, dispatch.clone())),
            _ => None,
        },
        _ => None,
    }).collect()
}

const ROLES: &str = "module app v1.0.0;
role Named {
    name() is int;
}
role Sized {
    size() is int;
}
object A does Named, Sized {
    A();
    name() is int { 1; }
    size() is int { 3; }
}
object B does Named {
    has n is int;
    B(:n);
    name() is int { n; }
}
object Holder {
    has item is Named;
    Holder(:item);
    name() is int { item.name(); }
}
main() is int {
    let x is Named = A();
    let y is Named = B(n: 41);
    let all is @Named = [x, y];
    let held = Holder(item: y);
    let sized is Sized = A();
    x.name() + held.name() - 42 + all.size() - 2 + sized.size() - 3;
}";

#[test]
fn role_declarations_carry_their_slots_and_objects_their_tables() {
    let p = compile(ROLES);
    let named = object(&p, "app.Named");
    let role = p.objects[named as usize].role.as_ref().unwrap();
    assert_eq!(role.slots, vec![s::RoleSlot {
        name: "name".into(), entry_abi: s::EntryAbi::Owned, params: vec![], results: vec![s::Type::Int],
    }]);
    assert!(p.objects[named as usize].layout.storage_fields.is_empty());
    let a = object(&p, "app.A");
    let tables = &p.objects[a as usize].implements;
    assert_eq!(tables.iter().map(|table| table.role).collect::<Vec<_>>(), vec![named, object(&p, "app.Sized")]);
    // Each slot function takes the implementing object first, then the slot's parameters.
    for table in tables {
        let f = &p.functions[table.slots[0] as usize];
        assert_eq!(f.params.first().map(|&location| match location {
            s::Location::Register(id) => f.registers[id as usize],
            s::Location::Service(_) => s::Type::Never,
        }), Some(s::Type::Object(a)));
    }
    assert!(p.objects[object(&p, "app.Holder") as usize].implements.is_empty());
}

#[test]
fn role_values_fields_and_arrays_are_role_types() {
    let p = compile(ROLES);
    let named = object(&p, "app.Named");
    let holder = &p.objects[object(&p, "app.Holder") as usize];
    assert_eq!(holder.layout.storage_fields, vec![s::Type::Role(named)]);
    assert_eq!(holder.layout.trace_layout, vec![0]);
    let main = &p.functions[function(&p, "main")];
    assert!(main.registers.contains(&s::Type::Array(s::ArrayType { leaf: s::ArrayLeaf::Role(named), depth: 1 })));
}

#[test]
fn a_role_call_dispatches_to_every_implementation() {
    let p = compile(ROLES);
    let (named, a, b) = (object(&p, "app.Named"), object(&p, "app.A"), object(&p, "app.B"));
    let calls = dispatches(&p.functions[function(&p, "main")]);
    let named_calls = calls.iter().filter(|(_, dispatch)| dispatch.role == named).collect::<Vec<_>>();
    assert!(!named_calls.is_empty());
    for (op, dispatch) in named_calls {
        assert_eq!(*op, s::Opcode::InvokeDispatchOwn);
        assert_eq!(dispatch.targets, vec![a, b]);
        assert_eq!((dispatch.slot, dispatch.failures), (0, 0));
    }
    let sized = object(&p, "app.Sized");
    assert!(calls.iter().any(|(_, dispatch)| dispatch.role == sized && dispatch.targets == [a]));
}

#[test]
fn role_narrowing_is_a_checked_conversion() {
    let p = compile("module app v1.0.0;
role Named {
    name() is string;
}
role Labeled {
    name() is string;
    label() is string;
}
object Tag does Named, Labeled {
    Tag();
    name() is string { \"tag\"; }
    label() is string { \"#tag\"; }
}
main() is int {
    let full is Labeled = Tag();
    let narrower is Named = full;
    narrower.name().length() - 3;
}");
    let main = &p.functions[function(&p, "main")];
    let narrowed = main.code.iter().any(|instruction| matches!(instruction,
        s::Instruction::Champ { op: s::Opcode::Evaluate, arguments, .. }
            if matches!(arguments.first(), Some(s::Argument::Evaluation(evaluation))
                if evaluation.op == s::DataOp::RoleNarrow
                    && evaluation.result == s::Type::Role(object(&p, "app.Named"))))
        || matches!(instruction, s::Instruction::Data { op: s::DataOp::RoleNarrow, .. }));
    assert!(narrowed);
}

const CLOSURES: &str = "module app v1.0.0;
object Account {
    has balance is int;
    Account(:balance);
    preview(amount is int) is int { balance + amount; }
}
main() is int {
    let base = 40;
    let answer = () is int { base + 2; };
    let acc = Account(balance: 10);
    let deposit is (int) is int = acc.preview;
    let wide = base > 0;
    let pick = if wide { () is int { 1; }; } else { () is int { 2; }; };
    answer() - 42 + deposit(5) - 15 + pick() - 1;
}";

#[test]
fn an_action_value_is_a_closure_layout_implementing_its_fn_role() {
    let p = compile(CLOSURES);
    let layouts = p.objects.iter().enumerate().filter(|(_, object)| object.name.ends_with(".closure"))
        .collect::<Vec<_>>();
    assert_eq!(layouts.len(), 4);
    for (_, layout) in &layouts {
        let [table] = layout.implements.as_slice() else { panic!("one fn role") };
        let role = p.objects[table.role as usize].role.as_ref().unwrap();
        assert_eq!(role.slots.len(), 1);
        assert_eq!((role.slots[0].name.as_str(), &role.slots[0].entry_abi), ("call", &s::EntryAbi::Owned));
        // The trampoline reads every capture, then calls the target with captures first.
        let trampoline = &p.functions[table.slots[0] as usize];
        assert_eq!(trampoline.symbol, format!("{}.call", layout.name));
        let reads = trampoline.code.iter().take_while(|instruction| {
            matches!(instruction, s::Instruction::Data { op: s::DataOp::ObjGet, .. })
        }).count();
        assert_eq!(reads, layout.layout.storage_fields.len());
        assert!(matches!(trampoline.code[reads], s::Instruction::Champ { op: s::Opcode::InvokeCallOwn, .. }));
    }
    // The capturing lambda holds `base`; the bound action holds its receiver.
    let captures = layouts.iter().map(|(_, layout)| layout.layout.storage_fields.clone()).collect::<Vec<_>>();
    assert!(captures.contains(&vec![s::Type::Int]));
    assert!(captures.contains(&vec![s::Type::Object(object(&p, "app.Account"))]));
}

#[test]
fn an_action_call_lists_the_closures_it_can_hold() {
    let p = compile(CLOSURES);
    let main = &p.functions[function(&p, "main")];
    let calls = dispatches(main);
    assert_eq!(calls.len(), 3);
    let targets = calls.iter().map(|(_, dispatch)| {
        dispatch.targets.iter().map(|&target| p.objects[target as usize].name.clone()).collect::<Vec<_>>()
    }).collect::<Vec<_>>();
    // A single origin each for the lambda and the bound action; both branches for `pick`.
    assert!(targets.iter().any(|names| names.len() == 1 && names[0].contains("lambda")));
    assert!(targets.iter().any(|names| names.len() == 1 && names[0].contains("Account.preview")));
    assert!(targets.iter().any(|names| names.len() == 2));
    for (op, dispatch) in &calls {
        assert_eq!((*op, dispatch.slot), (s::Opcode::InvokeDispatchOwn, 0));
    }
}

#[test]
fn callable_roles_follow_the_declarations_in_type_order() {
    let mut module = o::Module {
        name: "m".into(), version: None, profile: o::Profile::Low, requires: vec![o::Feature::Core],
        imports: vec![], declarations: o::Arena::default(), types: o::TypeTable::new(), scopes: o::Arena::default(),
        summaries: o::Arena::default(), protocols: o::Arena::default(), functions: o::Arena::default(),
        entry: None, tests: None, files: o::Arena::default(),
    };
    let int = module.types.intern(o::Type::Int).unwrap();
    let first = module.types.intern(o::Type::Fn(o::FnType { params: vec![], result: Some(int), is_async: false }))
        .unwrap();
    let future = module.types.intern(o::Type::Fn(o::FnType { params: vec![], result: None, is_async: true })).unwrap();
    let second = module.types.intern(o::Type::Fn(o::FnType { params: vec![int], result: None, is_async: false }))
        .unwrap();
    module.declarations.alloc(o::Declaration {
        name: "E".into(), type_params: vec![], kind: o::DeclarationKind::Enum { cases: vec![], is_error: false },
        loc: None,
    }).unwrap();
    assert_eq!(roles::callable_role(&module, first).unwrap(), 1);
    assert_eq!(roles::callable_role(&module, second).unwrap(), 2);
    assert!(roles::callable_role(&module, future).is_err());
    assert!(roles::callable_role(&module, int).is_err());
    assert_eq!(translate_type(&module, second).unwrap(), s::Type::Role(2));
    assert_eq!(roles::Synthetic::of(&module).unwrap().callables, vec![first, second]);
}

#[test]
fn a_cap_is_its_owners_reference_and_each_read_calls_the_field() {
    let p = compile("module app v1.0.0;
object Meter {
    has base is int;
    Meter(:base);
    cap Reading {
        value is int { base * 2; }
    }
}
main() is int {
    let m = Meter(base: 2);
    let view = m::Reading;
    view::value + view::value - 8;
}");
    let meter = object(&p, "app.Meter");
    let reading = &p.objects[object(&p, "app.Meter.Reading") as usize];
    assert!(reading.fields.is_empty() && reading.role.is_none() && reading.implements.is_empty());
    let field = function(&p, "app.Meter.cap.Reading.value#0") as u32;
    let main = &p.functions[function(&p, "main")];
    let reads = main.code.iter().filter(|instruction| matches!(instruction, s::Instruction::Champ {
        op: s::Opcode::InvokeCallOwn | s::Opcode::InvokeObserve, arguments, .. }
        if arguments.contains(&s::Argument::Entry(field)))).count();
    assert_eq!(reads, 2);
    assert!(!main.registers.contains(&s::Type::Object(object(&p, "app.Meter.Reading"))));
    assert!(main.registers.contains(&s::Type::Object(meter)));
}

#[test]
fn a_role_successor_keeps_its_permission_until_taken() {
    let p = compile("module app v1.0.0;
role Package {
    open() is ^Package;
    label() is int;
}
object Box does Package {
    has opened is bool = false;
    Box() {}
    open() is ^Box { ^opened = true; }
    label() is int { if opened { 1; } else { 0; } }
}
main() is int {
    let p is Package = Box();
    let q = p.open();
    q.label() - 1;
}");
    let package = object(&p, "app.Package");
    let role = p.objects[package as usize].role.as_ref().unwrap();
    assert_eq!(role.slots[0].results, vec![s::Type::RoleSuccessor(package)]);
    let main = &p.functions[function(&p, "main")];
    let takes = main.code.iter().filter(|instruction| matches!(instruction,
        s::Instruction::Data { op: s::DataOp::Move, inputs, output: Some(output), .. }
            if main.registers[inputs[0] as usize] == s::Type::RoleSuccessor(package)
                && main.registers[*output as usize] == s::Type::Role(package))).count();
    // The normal path takes it; an unwind path that abandons it discharges it the same way. The loader has
    // checked one permission per path.
    assert!(takes >= 1);
}

#[test]
fn a_successor_used_twice_in_source_takes_its_permission_once() {
    let p = compile("module app v1.0.0;
object Cell {
    has v is int;
    Cell(:v);
    bumped() is ^Cell { ^v = v + 1; }
    value() is int { v; }
}
main() is int {
    let c = Cell(v: 1).bumped();
    c.value() + c.value() - 4;
}");
    let main = &p.functions[function(&p, "main")];
    let takes = main.code.iter().filter(|instruction| matches!(instruction,
        s::Instruction::Data { op: s::DataOp::Move, inputs, .. }
            if matches!(main.registers[inputs[0] as usize], s::Type::Successor(_)))).count();
    assert_eq!(takes, 1);
}

const BOUND_AND_CAPS: &str = "module app v1.0.0;
object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { }
}
object Meter {
    has res is Res;
    Meter(:res);
    cap Reading {
        value is int { res.label().length(); }
    }
}
role Named {
    name() is int;
}
object A does Named {
    has n is int;
    A(:n);
    name() is int { n; }
}
object B does Named {
    B();
    name() is int { 5; }
}
main() is int {
    let a is Named = A(n: 2);
    let g is () is int = a.name;
    let m = Meter(res: Res(name: \"xy\"));
    let view = m::Reading;
    g() + view::value - 4;
}";

#[test]
fn a_bound_role_action_dispatches_from_its_closure() {
    let p = compile(BOUND_AND_CAPS);
    let (named, a, b) = (object(&p, "app.Named"), object(&p, "app.A"), object(&p, "app.B"));
    let (layout, closure) = p.objects.iter().enumerate()
        .find(|(_, object)| object.name.starts_with("app.Named.name#")).unwrap();
    assert_eq!(closure.layout.storage_fields, vec![s::Type::Role(named)]);
    let trampoline = &p.functions[closure.implements[0].slots[0] as usize];
    assert!(matches!(trampoline.code[0], s::Instruction::Data { op: s::DataOp::ObjGet, .. }));
    let calls = dispatches(trampoline);
    assert_eq!(calls.len(), 1);
    assert_eq!((calls[0].1.role, calls[0].1.slot, calls[0].1.targets.clone()), (named, 0, vec![a, b]));
    // The action call itself lists the one layout it can hold.
    let main = dispatches(&p.functions[function(&p, "main")]);
    assert!(main.iter().any(|(_, dispatch)| dispatch.targets == [layout as u32]));
}

#[test]
fn a_cap_of_a_fresh_owner_carries_its_packet() {
    let p = compile(BOUND_AND_CAPS);
    let meter = object(&p, "app.Meter");
    let main = &p.functions[function(&p, "main")];
    let moved = main.code.windows(2).any(|pair| matches!(pair,
        [s::Instruction::Data { op: s::DataOp::Move, inputs, output: Some(output), .. },
            s::Instruction::Champ { op: s::Opcode::OwnMove, .. }]
            if main.registers[inputs[0] as usize] == s::Type::Object(meter)
                && main.registers[*output as usize] == s::Type::Object(meter)));
    assert!(moved);
}

const RESOURCE_CLOSURE: &str = r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}
main() is int {
    let out = Stdout();
    let r = Res(name: "r");
    let f = () is string { r.label() + "!"; };
    out.writeln(f());
    out.writeln(f());
    out.writeln("end");
    0;
}
"#;

/// Decision (a) of #676: a closure that captures a resource by reference is a new object reaching it, so it is
/// committed into the scope with the empty packet own gave it, exactly like `obj.new` with a packet.
#[test]
fn a_closure_capturing_a_resource_is_committed_with_its_packet() {
    let p = compile(RESOURCE_CLOSURE);
    let res = object(&p, "app.Res");
    let (layout, closure) = p.objects.iter().enumerate()
        .find(|(_, object)| object.name.ends_with(".closure")).unwrap();
    assert_eq!(closure.layout.storage_fields, vec![s::Type::Object(res)]);
    assert!(closure.layout.may_reach_resource);
    let main = &p.functions[function(&p, "main")];
    let new = main.code.iter().position(|instruction| matches!(instruction,
        s::Instruction::Data { op: s::DataOp::ObjNew, immediate: s::Immediate::Object(object), .. }
            if *object as usize == layout)).unwrap();
    let s::Instruction::Champ { op: s::Opcode::OwnCreateCommit, arguments, flow: s::Flow::Next(committed) }
        = &main.code[new + 1] else { panic!("the closure is not committed: {:?}", main.code[new + 1]) };
    assert_eq!(arguments[1], s::Argument::Object(layout as u32));
    let s::Instruction::Champ { op: s::Opcode::OwnJoin, arguments, .. } = &main.code[new + 2] else {
        panic!("the committed packet is not joined: {:?}", main.code[new + 2]);
    };
    assert_eq!(arguments[1], s::Argument::Location(committed[0]));
}

const ESCAPED_ROLE_CALL: &str = "module app v1.0.0;
role Named { name() is string; }
object Child {
    has seen is string;
    Child(parent is Named) { seen = parent.name(); }
}
object Parent does Named {
    has label is string;
    has child is Maybe<Child>;
    Parent(label is string) {
        self.label = label;
        child = Maybe.some(Child(self));
    }
    name() is string { label; }
}
main() is int {
    let n = Parent(\"p\").name();
    0;
}";

/// §11.4: an incomplete construction stores its pending state in `$pending` (High leaves the operand unbound),
/// so the guard of a role entry sees it until the completing write stores false.
#[test]
fn an_incomplete_construction_starts_pending() {
    let p = compile(ESCAPED_ROLE_CALL);
    let parent = object(&p, "app.Parent");
    let constructor = &p.functions[function(&p, "app.Parent.new#0")];
    let inputs = constructor.code.iter().find_map(|instruction| match instruction {
        s::Instruction::Data { op: s::DataOp::ObjNew, inputs, immediate: s::Immediate::Object(object), .. }
            if *object == parent => Some(inputs.clone()),
        _ => None,
    }).unwrap();
    let pending = *inputs.last().unwrap();
    assert!(constructor.code.iter().any(|instruction| matches!(instruction,
        s::Instruction::Data { op: s::DataOp::Const, output: Some(output),
            immediate: s::Immediate::Constant(s::Constant::Bool(true)), .. } if *output == pending)));
}
