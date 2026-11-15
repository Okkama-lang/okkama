//! `ordering()` (#678): one SPB 17 operation, as Tomato's `ordering` intrinsic. Scalars and text sort with
//! `array.order`; elements ordered by `Comparing<T>.order` with `array.order.dispatch`, whose dispatch carries
//! the failures of `order`. No sorting loop is synthesized.
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

fn compile(text: &str) -> s::Program {
    let source = SourceFile::new("app.ok", text).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let roots = [RootFile { source: &source, program: &program }];
    let bytes = crate::lowering::oir2::compile_linked(&roots, &mut StdLoader).unwrap_or_else(|error| panic!("{error}"));
    s::decode(&bytes).unwrap()
}

fn main_function(p: &s::Program) -> &s::Function {
    p.functions.iter().find(|function| function.symbol == "main").expect("main")
}

fn data_ops(f: &s::Function) -> Vec<s::DataOp> {
    f.code.iter().filter_map(|instruction| match instruction {
        s::Instruction::Data { op, .. } => Some(*op),
        s::Instruction::Champ { arguments, .. } => match arguments.first() {
            Some(s::Argument::Evaluation(evaluation)) => Some(evaluation.op),
            _ => None,
        },
        _ => None,
    }).collect()
}

fn order_dispatches(f: &s::Function) -> Vec<(&s::Dispatch, &s::Flow)> {
    f.code.iter().filter_map(|instruction| match instruction {
        s::Instruction::Champ { op: s::Opcode::OrderDispatch, arguments, flow } => match arguments.first() {
            Some(s::Argument::Dispatch(dispatch)) => Some((dispatch, flow)),
            _ => None,
        },
        _ => None,
    }).collect()
}

#[test]
fn scalar_and_text_orderings_are_one_array_order() {
    let p = compile("module app v1.0.0;
import \"compare\" take Comparing;
main() is int {
    let ints is @int = [3, 1, 2];
    let texts is @string = [\"b\", \"a\"];
    let bytes is @byte = [byte(2), byte(1)];
    ints.ordering().size() + texts.ordering().size() + bytes.ordering().size() - 7;
}");
    let main = main_function(&p);
    let ops = data_ops(main);
    assert_eq!(ops.iter().filter(|op| **op == s::DataOp::ArrayOrder).count(), 3, "{ops:?}");
    // No insertion sort: the sort neither replaces elements nor loops.
    assert!(!ops.contains(&s::DataOp::ArrayReplace), "{ops:?}");
    let sorted = main.code.iter().filter_map(|instruction| match instruction {
        s::Instruction::Data { op: s::DataOp::ArrayOrder, output: Some(output), .. } => {
            Some(main.registers[*output as usize])
        }
        _ => None,
    }).collect::<Vec<_>>();
    let array = |leaf| s::Type::Array(s::ArrayType { leaf, depth: 1 });
    assert_eq!(sorted, vec![array(s::ArrayLeaf::Int), array(s::ArrayLeaf::Str), s::Type::Bytes]);
}

const CARDS: &str = "module app v1.0.0;
import \"compare\" take Comparing, Order;
enum Refused is Error = (late);
object Card does Comparing<Card> {
    has rank is int;
    Card(:rank);
    rank() is int { rank; }
    order(other is Card) is Order {
        RANK_CHECK
        rank.order(other.rank());
    }
}
main() is int {
    let cards is @Card = [Card(rank: 2), Card(rank: 1)];
    cards.ordering()[0].rank() - 1;
}";

#[test]
fn comparing_elements_sort_by_one_order_dispatch() {
    let p = compile(&CARDS.replace("RANK_CHECK", ""));
    let main = main_function(&p);
    let card = p.objects.iter().position(|object| object.name == "app.Card").unwrap() as u32;
    let dispatches = order_dispatches(main);
    assert_eq!(dispatches.len(), 1);
    let (dispatch, flow) = dispatches[0];
    assert_eq!(dispatch.targets, vec![card]);
    let role = p.objects[dispatch.role as usize].role.as_ref().unwrap();
    assert_eq!(role.slots[dispatch.slot as usize].name, "order");
    // `order` cannot fail here: the sort has no unwind outcome.
    assert_eq!(dispatch.failures, 0);
    assert!(matches!(flow, s::Flow::Next(_)), "{flow:?}");
    assert!(!data_ops(main).contains(&s::DataOp::ArrayReplace));
}

#[test]
fn a_failing_order_makes_the_sort_fail_with_its_failures() {
    let p = compile(&CARDS.replace("RANK_CHECK", "confirm rank != 7 else throw Refused.late;"));
    let main = main_function(&p);
    let dispatches = order_dispatches(main);
    assert_eq!(dispatches.len(), 1);
    let (dispatch, flow) = dispatches[0];
    // The thrown error of `order` (D18 Thrown) is the sort's failure.
    assert_ne!(dispatch.failures & 1, 0, "{}", dispatch.failures);
    let s::Flow::Outcomes(outcomes) = flow else { panic!("{flow:?}") };
    assert_eq!(outcomes.iter().map(|outcome| outcome.tag).collect::<Vec<_>>(), [s::Tag::Normal, s::Tag::Unwind]);
}

/// The producer lowers `ordering()` to one operation: `array.order` for scalars, `call.order` on the
/// `Comparing<T>.order` slot for other elements. The High body has no loop for it.
#[test]
fn the_producer_lowers_ordering_to_one_operation() {
    let text = CARDS.replace("RANK_CHECK", "").replace("cards.ordering()[0].rank() - 1;",
        "let ints is @int = [2, 1];\n    cards.ordering()[0].rank() + ints.ordering()[0] - 2;");
    let source = SourceFile::new("app.ok", text).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let roots = [RootFile { source: &source, program: &program }];
    let module = crate::lowering::oir2::lower_linked_high(&roots, &mut StdLoader)
        .unwrap_or_else(|error| panic!("{error}"));
    let (_, main) = module.functions.iter().find(|(_, function)| function.symbol == "main").expect("main");
    let body = main.body.as_ref().expect("main body");
    let ops = body.insts.iter().map(|(_, instruction)| &instruction.op).chain(body.blocks.iter()
        .filter_map(|(_, block)| match &block.term.kind {
            o::TermKind::InvokeOwn { op, .. } => Some(op.as_ref()),
            _ => None,
        })).collect::<Vec<_>>();
    let sorts = ops.iter().filter(|op| matches!(op, o::Operation::Call(o::CallOp::Order { .. }))).count();
    let naturals = ops.iter().filter(|op| matches!(op,
        o::Operation::Collection(o::CollectionOp::ArrayOrder { .. }))).count();
    assert_eq!((sorts, naturals), (1, 1));
    assert!(!ops.iter().any(|op| matches!(op, o::Operation::Collection(o::CollectionOp::ArrayReplace { .. }))));
    // No loop: every branch goes forward.
    let printed = o::text::print(&module).unwrap();
    let main_text = &printed[printed.find("func @main").unwrap()..];
    let main_text = &main_text[..main_text.find("\n}\n").unwrap()];
    assert!(!main_text.contains("iteration"), "{main_text}");
}
