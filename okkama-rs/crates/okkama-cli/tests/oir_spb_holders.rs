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
object Plain { has n is int; Plain(:n); }
object Holder { has f is Future<int>; Holder(:f); }
role Pending { pending() is int; }
object Waiting does Pending { has f is Future<int>; Waiting(:f); pending() is int { 1; } }
role Quiet { quiet() is int; }
object Calm does Quiet { has n is int; Calm(:n); quiet() is int { n; } }
object Nested { has inner is Holder; Nested(:inner); }
object Job { Job(); async run() is int { 1; } }
main() is int {
    let h = Holder(f: Job().run());
    let boxed is any = h;
    let p is Pending = Waiting(f: Job().run());
    let q is Quiet = Calm(n: 1);
    let hs = [h];
    let ps = [Plain(n: 1)];
    let n = Nested(inner: Holder(f: Job().run()));
    0;
}
"#;

/// The SPB 17 type of an OIR language type, for the shapes of `SOURCE`: objects and roles keep their declaration
/// index, arrays their element.
fn spb(module: &o::Module, ty: o::TypeId) -> Option<s::Type> {
    Some(match module.types.get(ty).ok()? {
        o::Type::Object { decl, .. } => s::Type::Object(decl.as_u32()),
        o::Type::Role { decl, .. } => s::Type::Role(decl.as_u32()),
        o::Type::Any => s::Type::Any,
        o::Type::Future(_) => s::Type::Future(0),
        o::Type::Array(element) => s::ArrayType::of(spb(module, *element)?)?,
        _ => return None,
    })
}

#[test]
fn holds_futures_and_reaches_holder_agree() {
    let source = SourceFile::new("inline.ok", SOURCE.to_string()).unwrap();
    let module = high(std::slice::from_ref(&source)).unwrap_or_else(|error| panic!("{error}"));
    let program = s::decode(&compile(&[source]).unwrap_or_else(|error| panic!("{error}"))).unwrap();
    let name = |ty: o::TypeId| -> String {
        match module.types.get(ty).unwrap() {
            o::Type::Object { decl, .. } | o::Type::Role { decl, .. } => {
                module.declarations.get(*decl).unwrap().name.clone()
            }
            o::Type::Array(element) => format!("[{}]", match module.types.get(*element).unwrap() {
                o::Type::Object { decl, .. } => module.declarations.get(*decl).unwrap().name.clone(),
                other => format!("{other:?}"),
            }),
            other => format!("{other:?}"),
        }
    };
    let expected = [("app.Plain", false), ("app.Holder", true), ("app.Pending", true), ("app.Waiting", true),
        ("app.Quiet", false), ("app.Calm", false), ("app.Nested", true), ("[app.Holder]", true),
        ("[app.Plain]", false), ("Any", true)];
    let mut checked = Vec::new();
    for (ty, _) in module.types.iter() {
        let label = name(ty);
        let Some(&(_, intended)) = expected.iter().find(|(name, _)| *name == label) else { continue };
        let Some(translated) = spb(&module, ty) else { continue };
        if let s::Type::Object(id) | s::Type::Role(id) = translated {
            assert_eq!(program.objects[id as usize].name, label, "the declaration index names the same object");
        }
        assert_eq!(o::verify::holds_futures(&module, ty), intended, "holds_futures {label}");
        assert_eq!(s::reaches_holder(&program, translated), intended, "reaches_holder {label}");
        checked.push(label);
    }
    checked.sort();
    checked.dedup();
    assert_eq!(checked.len(), expected.len(), "{checked:?}");
}
