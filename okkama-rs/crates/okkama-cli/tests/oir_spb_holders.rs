//! PR-B item 4 (OIR-2-async-remaining §2.2, #784): the Full verifier's model of which values can hold a Future
//! (`holds_futures`, on OIR types) and the SPB 17 loader's (`reaches_holder`, on SPB types) reach the same holders,
//! as the runtime selection walk does. One program declares each shape twice, once reaching a Future and once not;
//! both answers must agree on every one, and match the shape's intent.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{compile, high};
use okkama_compiler::oir2 as o;
use okkama_compiler::SourceFile;
use okkama_sparrow::spb17 as s;

const SOURCE: &str = r#"module app v1.0.0;
import "async/atomic" take Atomic;
object Plain { has n is int; Plain(:n); grown() is ^Plain { ^n = n + 1; } }
object Holder { has f is Future<int>; Holder(:f); again() is ^Holder { ^f = f; } }
role Pending { pending() is int; }
object Waiting does Pending { has f is Future<int>; Waiting(:f); pending() is int { 1; } }
role Quiet { quiet() is int; }
object Calm does Quiet { has n is int; Calm(:n); quiet() is int { n; } }
object Nested { has inner is Holder; Nested(:inner); }
object Job { Job(); async run() is int { 1; } }
object Kept { has slot is Atomic<@Future<int>>; Kept(:slot); }
enum Slot = (empty, ready(f is Future<int>));
enum Mark = (none, some(n is int));
main() is int {
    let h = Holder(f: Job().run());
    let boxed is any = h;
    let p is Pending = Waiting(f: Job().run());
    let q is Quiet = Calm(n: 1);
    let hs = [h];
    let ps = [Plain(n: 1)];
    let n = Nested(inner: Holder(f: Job().run()));
    let hm = [1: Holder(f: Job().run())];
    let pm = [1: Plain(n: 1)];
    let slot = Slot.ready(Job().run());
    let mark = Mark.some(1);
    let keep is (int) is int = (v is int) is int { let k = h; v; };
    let ff = [[Job().run()]];
    let empty is @Future<int> = [];
    let kept = Kept(slot: Atomic(empty));
    0;
}
"#;

/// The SPB 17 type of an OIR language type, for the shapes of `SOURCE`, as SPB 17 code generation lays them out:
/// objects, roles, variants and successors keep their declaration index, arrays their element, a map its composite
/// (found by its key and value), and a synchronous closure type is its callable role, numbered after the
/// declarations in the order of the module's synchronous `Fn` types.
fn spb(module: &o::Module, program: &s::Program, ty: o::TypeId) -> Option<s::Type> {
    Some(match module.types.get(ty).ok()? {
        o::Type::Object { decl, .. } => s::Type::Object(decl.as_u32()),
        o::Type::Role { decl, .. } => s::Type::Role(decl.as_u32()),
        o::Type::Variant(decl) => s::Type::Variant(decl.as_u32()),
        o::Type::Successor(base) => match module.types.get(*base).ok()? {
            o::Type::Object { decl, .. } => s::Type::Successor(decl.as_u32()),
            _ => return None,
        },
        o::Type::Any => s::Type::Any,
        o::Type::Int => s::Type::Int,
        o::Type::Future(_) => s::Type::Future(0),
        o::Type::Array(element) => s::ArrayType::of(spb(module, program, *element)?)?,
        o::Type::Map(key, value) => {
            let (key, value) = (spb(module, program, *key)?, spb(module, program, *value)?);
            let index = program.composites.iter().position(|composite| {
                *composite == s::Composite::Map { key, value }
            })?;
            s::Type::Map(index as u32)
        }
        o::Type::Fn(function) if !function.is_async => {
            let sync = |ty: &o::Type| matches!(ty, o::Type::Fn(function) if !function.is_async);
            let ordinal = module.types.iter().take(ty.as_u32() as usize).filter(|(_, ty)| sync(ty)).count();
            s::Type::Role((module.declarations.len() + ordinal) as u32)
        }
        _ => return None,
    })
}

#[test]
fn holds_futures_and_reaches_holder_agree() {
    let source = SourceFile::new("inline.ok", SOURCE.to_string()).unwrap();
    let module = high(std::slice::from_ref(&source)).unwrap_or_else(|error| panic!("{error}"));
    let program = s::decode(&compile(&[source]).unwrap_or_else(|error| panic!("{error}"))).unwrap();
    let name = |ty: o::TypeId| -> String { label(&module, ty) };
    // Review of D25, answer 6: maps, variants, successors, closures and nested arrays of Futures, each with its
    // quiet twin where the language has one; and an object that keeps an Atomic over Futures (PR-A).
    let expected = [("app.Plain", false), ("app.Holder", true), ("app.Pending", true), ("app.Waiting", true),
        ("app.Quiet", false), ("app.Calm", false), ("app.Nested", true), ("[app.Holder]", true),
        ("[app.Plain]", false), ("Any", true), ("%int:app.Holder", true), ("%int:app.Plain", false),
        ("app.Slot", true), ("app.Mark", false), ("^app.Holder", true), ("^app.Plain", false),
        ("(int) is int", true), ("[[future]]", true),
        // PR-A: an Atomic borrows the Futures it stores, so a value that keeps one reaches no holder through it.
        ("app.Kept", false)];
    let mut checked = Vec::new();
    for (ty, _) in module.types.iter() {
        let label = name(ty);
        let Some(&(_, intended)) = expected.iter().find(|(name, _)| *name == label) else { continue };
        let Some(translated) = spb(&module, &program, ty) else { continue };
        if let s::Type::Object(id) | s::Type::Variant(id) | s::Type::Successor(id) = translated {
            assert_eq!(program.objects[id as usize].name, label.trim_start_matches('^'),
                "the declaration index names the same object");
        }
        assert_eq!(o::verify::holds_futures(&module, ty), intended, "holds_futures {label}");
        assert_eq!(s::reaches_holder(&program, translated), intended, "reaches_holder {label}");
        checked.push(label);
    }
    checked.sort();
    checked.dedup();
    assert_eq!(checked.len(), expected.len(), "{checked:?}");
}

/// A readable name of an OIR type of `SOURCE`, as the table of `holds_futures_and_reaches_holder_agree` lists it.
fn label(module: &o::Module, ty: o::TypeId) -> String {
    match module.types.get(ty).unwrap() {
        o::Type::Object { decl, .. } | o::Type::Role { decl, .. } | o::Type::Variant(decl) => {
            module.declarations.get(*decl).unwrap().name.clone()
        }
        o::Type::Successor(base) => format!("^{}", label(module, *base)),
        o::Type::Array(element) => format!("[{}]", label(module, *element)),
        o::Type::Map(key, value) => format!("%{}:{}", label(module, *key), label(module, *value)),
        o::Type::Fn(function) if !function.is_async && function.params.len() == 1 => {
            format!("({}) is {}", label(module, function.params[0]), function.result.map_or("void".into(), |result| {
                label(module, result)
            }))
        }
        o::Type::Int => "int".into(),
        o::Type::Future(_) => "future".into(),
        other => format!("{other:?}"),
    }
}
