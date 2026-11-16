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

const OPEN_RESOURCE: &str = r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}
object Factory {
    Factory();
    maker(name is string) is () is Res { () is Res { Res(name: name); }; }
}
main() is int {
    let out = Stdout();
    let make = Factory().maker("made");
    {
        let r = make();
        out.writeln("got " + r.label());
    }
    out.writeln("end");
    0;
}
"#;

/// The loader's verdict on `p` with the runtime's P1 contracts.
fn load(p: &s::Program) -> Result<(), s::Error> {
    s::verify_load(p, &s::p1::P1Contracts::new(&p.objects).unwrap())
}

/// The `own.dynamic` instruction of `main` and its position.
fn dynamic_site(p: &s::Program) -> (usize, usize, Vec<s::Argument>) {
    let main = function(p, "main");
    let (pc, arguments) = p.functions[main].code.iter().enumerate().find_map(|(pc, instruction)| match instruction {
        s::Instruction::Champ { op: s::Opcode::OwnDynamic, arguments, .. } => Some((pc, arguments.clone())),
        _ => None,
    }).expect("own.dynamic in main");
    (main, pc, arguments)
}

/// An open call has the registered open callable contract with the Closure protocol: it dispatches over every
/// closure layout of its `fn` type, and its result packet (here a fresh resource) joins the scope through the
/// checked `own.dynamic` transfer of that protocol.
#[test]
fn an_open_call_hands_its_result_packet_to_the_closure_protocol() {
    let p = compile(OPEN_RESOURCE);
    let (main, pc, arguments) = dynamic_site(&p);
    assert_eq!(arguments[0], s::Argument::Protocol(s::Protocol::Closure));
    let code = &p.functions[main].code;
    let s::Argument::Location(packet) = arguments[2] else { panic!("packet operand") };
    // The packet arrives from the normal outcome of the open dispatch.
    let dispatched = code.iter().any(|instruction| match instruction {
        s::Instruction::Champ { op: s::Opcode::InvokeDispatchOwn, flow: s::Flow::Outcomes(outcomes), .. } => {
            outcomes.iter().any(|outcome| outcome.tag == s::Tag::Normal && outcome.edge.target_pc as usize == pc
                && outcome.edge.assignments.iter().any(|assignment| assignment.destination == packet))
        }
        _ => false,
    });
    assert!(dispatched, "{:?}", code[pc]);
    load(&p).unwrap();
    assert_eq!(s::assemble(&s::disassemble(&p).unwrap()).unwrap(), p);
}

/// The runtime descriptor of an action value is its `fn` role; the opaque cleanup intrinsic of that type is a
/// trusted external entry recomputed from its key.
#[test]
fn an_action_value_descriptor_names_its_fn_role_and_cleanup() {
    let p = compile(OPEN_RESOURCE);
    let main = &p.functions[function(&p, "main")];
    let descriptors = main.code.iter().filter_map(|instruction| match instruction {
        s::Instruction::Data { op: s::DataOp::Const,
            immediate: s::Immediate::Constant(s::Constant::Descriptor(id)), .. } => Some(*id),
        _ => None,
    }).collect::<Vec<_>>();
    assert!(!descriptors.is_empty());
    for id in descriptors {
        let role = p.objects[id as usize].role.as_ref().expect("descriptor of a role");
        assert_eq!(role.slots[0].name, "call");
    }
    let cleanup = p.functions.iter().find(|function| function.proofs.entry.as_ref()
        .and_then(|entry| entry.external.as_deref()) == Some(s::p1::SYNC_CLOSURE_CLEANUP)).expect("cleanup entry");
    assert!(matches!(cleanup.registers.as_slice(), [s::Type::Role(_)]));
    assert_eq!(o::catalogue::SYNC_CLOSURE_CLEANUP, s::p1::SYNC_CLOSURE_CLEANUP);
    assert_eq!(o::catalogue::ANY_CLEANUP, s::p1::ANY_CLEANUP);
}

/// DYNAMIC_SITE: `own.dynamic` takes its protocol, a scope state and a packet, in that order.
#[test]
fn a_dynamic_transfer_needs_a_scope_state_and_a_packet() {
    let p = compile(OPEN_RESOURCE);
    let (main, pc, arguments) = dynamic_site(&p);
    let mut swapped = p.clone();
    let s::Instruction::Champ { arguments: operands, .. } = &mut swapped.functions[main].code[pc] else {
        unreachable!()
    };
    operands.swap(1, 2);
    assert_eq!(load(&swapped).unwrap_err().rule, "DYNAMIC_SITE");
    let mut short = p.clone();
    let s::Instruction::Champ { arguments: operands, .. } = &mut short.functions[main].code[pc] else {
        unreachable!()
    };
    *operands = vec![arguments[0].clone(), arguments[1].clone(), arguments[1].clone()];
    assert_eq!(load(&short).unwrap_err().rule, "DYNAMIC_SITE");
    let mut bare = p.clone();
    let s::Instruction::Champ { arguments: operands, .. } = &mut bare.functions[main].code[pc] else {
        unreachable!()
    };
    operands.remove(0);
    assert!(load(&bare).is_err(), "own.dynamic without its protocol");
}

/// The opaque cleanup family is keyed by an action value's `fn` role: another key has no trusted entry, and
/// a declaration that differs from the recomputed entry is rejected.
#[test]
fn the_opaque_cleanup_entry_is_keyed_by_a_fn_role() {
    let p = compile(OPEN_RESOURCE);
    let index = p.functions.iter().position(|function| function.proofs.entry.as_ref()
        .and_then(|entry| entry.external.as_deref()) == Some(s::p1::SYNC_CLOSURE_CLEANUP)).unwrap();
    let mut plain = p.clone();
    plain.functions[index].registers[0] = s::Type::Int;
    assert_eq!(load(&plain).unwrap_err().rule, "EXTERNAL_UNRESOLVED");
    let named = object(&p, "app.Res");
    let mut object_key = p.clone();
    object_key.functions[index].registers[0] = s::Type::Object(named);
    assert_eq!(load(&object_key).unwrap_err().rule, "EXTERNAL_UNRESOLVED");
    let mut failing = p.clone();
    failing.functions[index].proofs.entry.as_mut().unwrap().failures = 2;
    assert!(load(&failing).is_err(), "a cleanup entry that may fail");
}

const ANY_VALUES: &str = r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    destroy { Stdout().writeln("destroy " + name); }
}
object Holder {
    has value is any;
    Holder(:value);
}
object Relay {
    Relay();
    echo(value is any) is any { value; }
}
main() is int {
    let out = Stdout();
    let one is any = 1;
    let text is any = "two";
    let holder = Holder(value: Res(name: "held"));
    let back = Relay().echo(text);
    out.writeln("end");
    0;
}
"#;

fn any_box(p: &s::Program, value: s::Type) -> u32 {
    p.objects.iter().position(|object| object.any_box && object.layout.storage_fields == [value])
        .unwrap_or_else(|| panic!("no any box of {value:?}")) as u32
}

/// `any.box` allocates the any box of the value's type and views it as `any`; one box per boxed type. A box of
/// a resource reaches it, so the object with an `any` field that holds it does too.
#[test]
fn an_any_value_is_a_box_of_its_value_type() {
    let p = compile(ANY_VALUES);
    let res = object(&p, "app.Res");
    let (int, text) = (any_box(&p, s::Type::Int), any_box(&p, s::Type::Str));
    let resource = any_box(&p, s::Type::Object(res));
    assert_eq!(p.objects.iter().filter(|object| object.any_box).count(), 3);
    assert!(!p.objects[int as usize].layout.may_reach_resource);
    assert!(p.objects[resource as usize].layout.may_reach_resource);
    assert_eq!(p.objects[resource as usize].layout.selection_layout, vec![0]);
    let holder = &p.objects[object(&p, "app.Holder") as usize];
    assert_eq!(holder.layout.storage_fields, vec![s::Type::Any]);
    assert!(holder.layout.may_reach_resource);
    let main = &p.functions[function(&p, "main")];
    for boxed in [int, text] {
        let wrapped = main.code.windows(2).any(|pair| matches!(pair,
            [s::Instruction::Data { op: s::DataOp::ObjNew, immediate: s::Immediate::Object(object),
                output: Some(created), .. },
                s::Instruction::Data { op: s::DataOp::Move, inputs, output: Some(output), .. }]
                if *object == boxed && inputs[0] == *created && main.registers[*output as usize] == s::Type::Any));
        assert!(wrapped, "box {boxed}");
    }
    let echo = p.functions.iter().find(|function| function.symbol.starts_with("app.Relay.echo")).unwrap();
    assert!(echo.registers.contains(&s::Type::Any));
    load(&p).unwrap();
}

/// The runtime descriptor of `any` is its placeholder; `oir.any.cleanup` is keyed by `any` alone.
#[test]
fn the_any_descriptor_and_cleanup_are_keyed_by_any() {
    let p = compile(ANY_VALUES);
    let any = object(&p, "any");
    let descriptor = p.functions.iter().flat_map(|function| &function.code).any(|instruction| matches!(instruction,
        s::Instruction::Data { op: s::DataOp::Const,
            immediate: s::Immediate::Constant(s::Constant::Descriptor(id)), .. } if *id == any));
    assert!(descriptor);
    let index = p.functions.iter().position(|function| function.proofs.entry.as_ref()
        .and_then(|entry| entry.external.as_deref()) == Some(s::p1::ANY_CLEANUP)).expect("any cleanup entry");
    assert_eq!(p.functions[index].registers, vec![s::Type::Any]);
    let mut keyed = p.clone();
    keyed.functions[index].registers[0] = s::Type::Str;
    assert_eq!(load(&keyed).unwrap_err().rule, "EXTERNAL_UNRESOLVED");
}

/// ANY_WRAP: only an any box becomes an `any` value.
#[test]
fn only_an_any_box_becomes_an_any_value() {
    let p = compile(ANY_VALUES);
    let mut unboxed = p.clone();
    unboxed.objects[any_box(&p, s::Type::Int) as usize].any_box = false;
    assert_eq!(load(&unboxed).unwrap_err().rule, "ANY_WRAP");
}

/// ANY_BOX: a box holds exactly its one value with that value's retention, and each value type has one box.
#[test]
fn an_any_box_holds_one_value_with_its_retention() {
    let p = compile(ANY_VALUES);
    let int = any_box(&p, s::Type::Int) as usize;
    let mut fieldless = p.clone();
    fieldless.objects[int].fields.clear();
    assert_eq!(load(&fieldless).unwrap_err().rule, "ANY_BOX");
    let mut overstated = p.clone();
    overstated.objects[int].layout.selection_layout = vec![0];
    assert_eq!(load(&overstated).unwrap_err().rule, "ANY_BOX");
    let resource = any_box(&p, s::Type::Object(object(&p, "app.Res"))) as usize;
    let mut unselected = p.clone();
    unselected.objects[resource].layout.selection_layout.clear();
    assert_eq!(load(&unselected).unwrap_err().rule, "ANY_BOX");
    // Understating the box's reach is the general field rule.
    let mut understated = unselected.clone();
    understated.objects[resource].layout.may_reach_resource = false;
    assert_eq!(load(&understated).unwrap_err().rule, "OBJECT_RETENTION");
    let mut role = p.clone();
    role.objects[int].role = Some(s::RoleDecl { slots: vec![] });
    assert!(load(&role).is_err());
    let mut twice = p.clone();
    let text = any_box(&p, s::Type::Str) as usize;
    twice.objects[text].layout.storage_fields = vec![s::Type::Int];
    twice.objects[text].layout.trace_layout.clear();
    assert_eq!(load(&twice).unwrap_err().rule, "ANY_BOX");
}

/// An object with an `any` field reaches resources when some any box does (OBJECT_RETENTION).
#[test]
fn an_any_field_states_the_retention_of_the_boxes() {
    let p = compile(ANY_VALUES);
    let holder = object(&p, "app.Holder") as usize;
    let mut understated = p.clone();
    understated.objects[holder].layout.may_reach_resource = false;
    understated.objects[holder].layout.selection_layout.clear();
    assert_eq!(load(&understated).unwrap_err().rule, "OBJECT_RETENTION");
}

/// An array or map of plain `any` values is closed plain data: its leaf is `any`, and it loads and round trips.
#[test]
fn plain_any_collections_are_closed_plain_data() {
    let p = compile(r#"module app v1.0.0;
import "io/stdout" take Stdout;
main() is int {
    let one is any = 1;
    let values is @any = [one, "two", 3.5];
    let row is %string:any = ["a": one];
    Stdout().writeln(values.size().string() + " " + row.size().string());
    0;
}
"#);
    let main = &p.functions[function(&p, "main")];
    assert!(main.registers.contains(&s::Type::Array(s::ArrayType { leaf: s::ArrayLeaf::Any, depth: 1 })));
    assert!(p.composites.contains(&s::Composite::Map { key: s::Type::Str, value: s::Type::Any }));
    assert_eq!(p.objects.iter().filter(|object| object.any_box).count(), 3);
    load(&p).unwrap();
    assert_eq!(s::assemble(&s::disassemble(&p).unwrap()).unwrap(), p);
    assert_eq!(s::decode(&s::encode(&p).unwrap()).unwrap(), p);
}

/// The SPB 17 protocols: the registered open Closure protocol and the synchronous `any` protocol (Low's #683
/// fixtures). An `any` protocol without every check, or any other protocol, has no SPB 17 form.
#[test]
fn dynamic_protocols_map_to_spb17_protocols() {
    let escape = o::text::parse(include_str!("../../../../okkama-oir/src/low/fixtures/open-call-escape.low.oir"))
        .unwrap();
    let any = o::text::parse(include_str!("../../../../okkama-oir/src/low/fixtures/open-any.low.oir")).unwrap();
    let (closure, _) = escape.protocols.iter().next().unwrap();
    assert_eq!(protocol(&escape, closure).unwrap(), s::Protocol::Closure);
    let (boxed, _) = any.protocols.iter().next().unwrap();
    assert_eq!(protocol(&any, boxed).unwrap(), s::Protocol::Any);
    let mut unchecked = any.clone();
    unchecked.protocols.get_mut(boxed).unwrap().rules[0].checks.pop();
    assert!(protocol(&unchecked, boxed).is_err());
    let mut services = any.clone();
    *services.protocols.get_mut(boxed).unwrap() = o::catalogue::services_protocol();
    assert!(protocol(&services, boxed).is_err());
}

const MIXED_BOXES: &str = r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    destroy { Stdout().writeln("destroy " + name); }
}
role Shape { area() is int; }
object Square does Shape {
    has side is int;
    Square(:side);
    area() is int { side * side; }
}
object Holder {
    has items is @any;
    Holder(:items);
    count() is int { items.size(); }
}
main() is int {
    let out = Stdout();
    let shapes is @Shape = [Square(side: 2), Square(side: 3)];
    {
        let mixed is @any = [Res(name: "boxed"), 7, "text"];
        let holder = Holder(items: mixed);
        out.writeln(holder.count().string() + " " + shapes.size().string());
    }
    0;
}
"#;

/// #711: a box of a resource makes every `any` collection managed, so an object that holds one selects it; the
/// role whose implementations are plain keeps its arrays closed plain data, and the program loads.
#[test]
fn a_reaching_box_makes_any_collections_managed_and_plain_roles_stay_plain() {
    let p = compile(MIXED_BOXES);
    let holder = &p.objects[object(&p, "app.Holder") as usize];
    assert_eq!(holder.layout.storage_fields, vec![s::Type::Array(s::ArrayType { leaf: s::ArrayLeaf::Any, depth: 1 })]);
    assert_eq!(holder.layout.selection_layout, vec![0]);
    assert!(holder.layout.may_reach_resource);
    let shape = object(&p, "app.Shape");
    assert!(!p.objects[object(&p, "app.Square") as usize].layout.may_reach_resource);
    let main = &p.functions[function(&p, "main")];
    assert!(main.registers.contains(&s::Type::Array(s::ArrayType { leaf: s::ArrayLeaf::Role(shape), depth: 1 })));
    assert!(p.objects.iter().any(|object| object.any_box && object.layout.may_reach_resource));
    assert!(p.objects.iter().any(|object| object.any_box && !object.layout.may_reach_resource));
    load(&p).unwrap();
}

/// #700: an observation that keeps no obligation may still retain a parameter: `Holder(item:)` stores the role
/// value it is given, so the call summary states the retained parameter as the entry contract does.
#[test]
fn an_observation_call_states_the_parameters_it_retains() {
    let p = compile(ROLES);
    let holder = function(&p, "app.Holder.new#0");
    assert_eq!(p.functions[holder].proofs.entry.as_ref().unwrap().retained_params, vec![0]);
    let main = &p.functions[function(&p, "main")];
    let summary = main.code.iter().find_map(|instruction| match instruction {
        s::Instruction::Champ { op: s::Opcode::InvokeObserve, arguments, .. }
            if arguments.contains(&s::Argument::Entry(holder as u32)) => arguments.iter().find_map(|argument| {
                match argument { s::Argument::Summary(id) => Some(*id), _ => None }
            }),
        _ => None,
    }).expect("observation call of Holder.new");
    assert_eq!(p.tables.summaries[summary as usize].retained_params, vec![0]);
}

/// An array of objects that hold a role value (#757: TOML members holding a value role) is managed for own,
/// which tracks the role values as references, though nothing in it reaches a resource. The backend admits it
/// as a view of closed plain data, as it does an array of role values, and the loader checks ARRAY_PLAIN.
#[test]
fn an_array_of_objects_holding_role_values_is_a_view() {
    let p = compile(r#"module app;
role Value { text() is string; }
object Plain does Value { has t is string; Plain(:t); text() is string { t; } }
object Member { has value is Value; Member(:value); text() is string { value.text(); } }
object Table { has origin is @Member; Table(:origin); first() is string { origin[0].text(); } }
main() is int { Table(origin: [Member(value: Plain(t: "a"))]).first().length(); }
"#);
    load(&p).unwrap();
}

/// What counts as a view: a role, cap, action or `any` leaf, also inside the object graph of an array or map
/// element (through fields, nested arrays and variant cases); plain scalars and objects of them do not.
#[test]
fn a_view_array_holds_a_reference_somewhere_in_its_element_graph() {
    let mut module = o::text::parse(r#"oir 2 high
module app

object app.Plain {
    field n: int
}

object app.Member {
    field value: app.Value
}

object app.Outer {
    field members: array<app.Member>
}

role app.Value {
    text() -> str
}
"#).unwrap();
    let mut ty = |name: &str, role: bool| {
        let decl = module.declarations.iter().find(|(_, decl)| decl.name == name).unwrap().0;
        let ty = if role { o::Type::Role { decl, args: vec![] } } else { o::Type::Object { decl, args: vec![] } };
        module.types.intern(ty).unwrap()
    };
    let plain = ty("app.Plain", false);
    let member = ty("app.Member", false);
    let outer = ty("app.Outer", false);
    let role = ty("app.Value", true);
    assert!(!roles::view_array(&module, role).unwrap(), "not a collection");
    for (element, view) in [(plain, false), (member, true), (outer, true), (role, true)] {
        let array = module.types.intern(o::Type::Array(element)).unwrap();
        assert_eq!(roles::view_array(&module, array).unwrap(), view, "{:?}", module.types.get(element).unwrap());
    }
}
