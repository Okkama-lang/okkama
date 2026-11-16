//! Builtin values behind the array roles (#757): Tomato's `ArraySequence<S, T>` and `ArrayCursor<S, T>`
//! as ordinary adapter objects, and role calls on roles that nothing witnesses.
use crate::oir2 as ir;

fn lowered(source: &str) -> ir::Module {
    let module = super::tests::lower_std(source).unwrap_or_else(|error| panic!("{error}"));
    ir::verify::verify_high(&module).unwrap_or_else(|report| panic!("{report:#?}"));
    let parsed = ir::text::parse(&ir::text::print(&module).unwrap()).unwrap();
    ir::verify::verify_high(&parsed).unwrap();
    module
}

/// The object declarations whose name starts with `prefix`, with the names of the roles they witness.
fn adapters(module: &ir::Module, prefix: &str) -> Vec<(String, Vec<String>)> {
    let mut found = vec![];
    for (_, declaration) in module.declarations.iter() {
        let ir::DeclarationKind::Object { witnesses, .. } = &declaration.kind else { continue };
        if !declaration.name.starts_with(prefix) {
            continue;
        }
        let roles = witnesses.iter().map(|witness| {
            let ir::Type::Role { decl, .. } = module.types.get(witness.role).unwrap() else { panic!("role") };
            module.declarations.get(*decl).unwrap().name.clone()
        }).collect();
        found.push((declaration.name.clone(), roles));
    }
    found
}

fn function<'m>(module: &'m ir::Module, symbol: &str) -> &'m ir::Function {
    module.functions.iter().map(|(_, function)| function).find(|function| function.symbol == symbol)
        .unwrap_or_else(|| panic!("no function {symbol}"))
}

fn operations(function: &ir::Function) -> Vec<&ir::Operation> {
    let body = function.body.as_ref().unwrap();
    body.insts.iter().map(|(_, instruction)| &instruction.op).collect()
}

const ARRAY_ROLES: &str = r#"module app;
import "collections/sequence" take Sequencing;
import "collections/iteration" take Iterating;
import "size" take Sizing;
object Probe {
    Probe();
    size(items is Sequencing<int>) is int { items.size(); }
    walk(items is Iterating<int>) is int { 0; }
    count(items is Sizing) is int { items.size(); }
}
main() is int {
    let p = Probe();
    let values is @int = [1, 2];
    p.size(values) + p.walk(values) + p.count(values);
}
"#;

/// One adapter object per (array type, role element): it witnesses every array role of that element,
/// as Tomato's `ArraySequence<int, int>` does Sequencing<int>, Iterating<int> and Sizing at once.
#[test]
fn an_array_adapter_witnesses_every_array_role_of_its_element() {
    let module = lowered(ARRAY_ROLES);
    let sequences = adapters(&module, "builtin.ArraySequence.");
    assert_eq!(sequences.len(), 1, "{sequences:?}");
    let mut roles = sequences[0].1.clone();
    roles.sort();
    assert_eq!(roles, ["collections/iteration::Iterating<int>",
        "collections/sequence::Sequencing<int>", "size.Sizing"].map(String::from).to_vec());
    let cursors = adapters(&module, "builtin.ArrayCursor.");
    assert_eq!(cursors, [(cursors[0].0.clone(), vec!["collections/iteration::Cursor<int>".to_string()])]);
    // Sequencing's slots, in the role's order, and one shared cursor action for both walkable roles.
    let object = module.declarations.iter().find(|(_, declaration)| declaration.name == sequences[0].0).unwrap().1;
    let ir::DeclarationKind::Object { witnesses, .. } = &object.kind else { unreachable!() };
    let symbols = |index: usize| witnesses[index].slots.iter()
        .map(|&slot| module.functions.get(slot).unwrap().symbol.rsplit('.').next().unwrap().to_string())
        .collect::<Vec<_>>();
    let slots = (0..witnesses.len()).map(symbols).collect::<Vec<_>>();
    assert!(slots.contains(&["cursor", "size", "item", "first", "last"].map(String::from).to_vec()), "{slots:?}");
    assert!(slots.contains(&vec!["cursor".to_string()]), "{slots:?}");
    assert!(slots.contains(&vec!["size".to_string()]), "{slots:?}");
}

/// `first()` and `last()` of an empty array behind Sequencing fail as Tomato's adapter does: the program error
/// "Cannot read first item of an empty sequence" (the array builtin's message).
#[test]
fn first_and_last_trap_on_an_empty_sequence() {
    let module = lowered(ARRAY_ROLES);
    let name = &adapters(&module, "builtin.ArraySequence.")[0].0;
    for which in ["first", "last"] {
        let body = function(&module, &format!("{name}.{which}")).body.as_ref().unwrap();
        let traps = body.blocks.iter().filter_map(|(_, block)| match &block.term.kind {
            ir::TermKind::Trap { code: ir::TrapCode::Empty, message } => Some(message.clone()),
            _ => None,
        }).collect::<Vec<_>>();
        assert_eq!(traps, [format!("Cannot read {which} item of an empty sequence")]);
        assert!(body.blocks.iter().any(|(_, block)| matches!(block.term.kind,
            ir::TermKind::Lookup { kind: ir::LookupKind::Array, .. })));
    }
}

/// An inner array reaching a role element is adapted by its own adapter (#488), at any depth: the outer
/// adapter's `item` calls the inner adapter's constructor and wraps the result into the element role.
#[test]
fn an_inner_array_crosses_through_its_own_adapter() {
    let module = lowered(r#"module app;
import "collections/sequence" take Sequencing;
object Probe {
    Probe();
    rows(items is Sequencing<Sequencing<Sequencing<int>>>) is int { items.size(); }
}
main() is int {
    let cube is @@@int = [[[1]]];
    Probe().rows(cube);
}
"#);
    let sequences = adapters(&module, "builtin.ArraySequence.");
    assert_eq!(sequences.len(), 3, "{sequences:?}");
    // The outer two hold arrays of arrays and cross into a role element; the innermost keeps its ints.
    let crossing = sequences.iter().filter(|(name, _)| name.matches(".t").count() == 2).count();
    assert_eq!(crossing, 2, "{sequences:?}");
    for (name, _) in sequences.iter().filter(|(name, _)| name.matches(".t").count() == 2) {
        let item = function(&module, &format!("{name}.item"));
        let operations = operations(item);
        let constructor = operations.iter().find_map(|op| match op {
            ir::Operation::Call(ir::CallOp::Direct { function, .. }) => Some(*function),
            _ => None,
        }).expect("inner adapter constructor call");
        let inner = module.functions.get(constructor).unwrap();
        assert!(inner.symbol.starts_with("builtin.ArraySequence.") && inner.symbol.ends_with(".new"));
        assert!(operations.iter().any(|op| matches!(op, ir::Operation::Core(ir::CoreOp::RoleWrap { .. }))));
    }
}

/// An object element crosses into its role element by `role.wrap`, in the item, first, last and cursor item.
#[test]
fn object_elements_wrap_into_the_role_element() {
    let module = lowered(r#"module app;
import "collections/sequence" take Sequencing;
role Named { name() is string; }
object Box does Named { has n is string; Box(:n); name() is string { n; } }
object Probe {
    Probe();
    first(items is Sequencing<Named>) is string { items.first().name(); }
}
main() is int {
    let boxes is @Box = [Box(n: "a")];
    Probe().first(boxes).length();
}
"#);
    let sequences = adapters(&module, "builtin.ArraySequence.");
    assert_eq!(sequences.len(), 1, "{sequences:?}");
    let name = &sequences[0].0;
    assert_eq!(name.matches(".t").count(), 2, "an element other than the array's own: {name}");
    for action in ["item", "first", "last"] {
        let wraps = operations(function(&module, &format!("{name}.{action}"))).into_iter()
            .filter(|op| matches!(op, ir::Operation::Core(ir::CoreOp::RoleWrap { .. }))).count();
        assert_eq!(wraps, 1, "{action}");
    }
    let cursor = &adapters(&module, "builtin.ArrayCursor.")[0].0;
    let wraps = operations(function(&module, &format!("{cursor}.item"))).into_iter()
        .filter(|op| matches!(op, ir::Operation::Core(ir::CoreOp::RoleWrap { .. }))).count();
    assert_eq!(wraps, 1);
}

/// A role no object or adapter witnesses has no values (`role.wrap` needs a witness), so a call on it joins
/// an empty set of implementations: the producer lowers it as unreachable instead of refusing the module.
#[test]
fn a_call_on_a_role_nothing_witnesses_is_unreachable() {
    let module = lowered(r#"module app;
import "collections/iteration" take Iterating;
role Visitor { visit(value is int) is int; }
object Node {
    has value is int;
    Node(:value);
    accept(to is Visitor) is int { to.visit(value) + 1; }
    walk(items is Iterating<int>) is int { for items -> item { item; } value; }
    plain() is int { value; }
}
main() is int { Node(value: 1).plain(); }
"#);
    for symbol in ["app.Node.accept#0", "app.Node.walk#1"] {
        let accept = function(&module, symbol);
        assert!(!operations(accept).iter().any(|op| matches!(op, ir::Operation::Call(ir::CallOp::Role { .. }))),
            "{symbol}");
        let body = accept.body.as_ref().unwrap();
        assert!(body.blocks.iter().any(|(_, block)| block.term.kind == ir::TermKind::Unreachable), "{symbol}");
    }
}

/// A role with a witness keeps its dispatch.
#[test]
fn a_call_on_a_witnessed_role_dispatches() {
    let module = lowered(r#"module app;
role Visitor { visit(value is int) is int; }
object Twice does Visitor { Twice(); visit(value is int) is int { value * 2; } }
object Node {
    has value is int;
    Node(:value);
    accept(to is Visitor) is int { to.visit(value) + 1; }
}
main() is int { Node(value: 1).accept(Twice()); }
"#);
    let accept = function(&module, "app.Node.accept#0");
    assert!(operations(accept).iter().any(|op| matches!(op, ir::Operation::Call(ir::CallOp::Role { .. }))));
    assert!(accept.body.as_ref().unwrap().blocks.iter().all(|(_, block)| block.term.kind != ir::TermKind::Unreachable));
}
