//! D8 source witnesses, including the fixture excluded from the first P1 run.
use crate::oir2 as ir;

#[test]
fn oir2_defer_return_scalar() {
    golden("scalar");
}

#[test]
fn oir2_defer_return_void() {
    golden("void");
}

#[test]
fn oir2_defer_return_branch() {
    golden("branch");
}

#[test]
fn oir2_defer_return_nested() {
    golden("nested");
}

#[test]
fn oir2_defer_return_nested_continue() {
    golden("nested-continue");
}

#[test]
fn oir2_defer_return_resource() {
    golden("resource");
}

#[test]
fn oir2_defer_return_bound() {
    golden("bound");
}

#[test]
fn oir2_defer_return_same_alias() {
    golden("same-alias");
}

#[test]
fn oir2_defer_return_throw() {
    golden("throw");
}

fn golden(name: &str) {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lowering/oir2");
    let source = std::fs::read_to_string(directory.join(format!("fixtures/defer-return/{name}.ok"))).unwrap();
    let actual = super::print_oir2(&source).unwrap_or_else(|error| panic!("{name}: {error}"));
    assert_eq!(actual, super::print_oir2(&source).unwrap(), "deterministic lowering: {name}");
    let path = directory.join(format!("goldens/defer-return/{name}.oir"));
    if std::env::var_os("OKKAMA_OIR2_UPDATE_GOLDENS").is_some() {
        std::fs::write(&path, &actual).unwrap();
    }
    let expected = std::fs::read_to_string(path).unwrap();
    let stored = ir::text::parse(&expected).unwrap();
    super::verify_high(&stored, name);
    assert_eq!(actual, expected, "{name}");
    check_defer_entries(&stored);
}

fn check_defer_entries(module: &ir::Module) {
    let mut replacements = 0;
    for (_, function) in module.functions.iter() {
        let Some(labels) = function.attrs.iter().find_map(|attr| {
            if let ir::FunctionAttr::DeferEntry { replacements } = attr {
                Some(replacements)
            } else {
                None
            }
        }) else {
            continue;
        };
        assert!(function.signature.results.is_empty());
        assert!(labels.len() <= 1);
        let body = function.body.as_ref().unwrap();
        for (block, data) in body.blocks.iter() {
            match &data.term.kind {
                ir::TermKind::RetDefer { tag: ir::DeferReturnTag::Replace(label), values } => {
                    replacements += 1;
                    assert_eq!(label, "return");
                    assert_eq!(values.len(), usize::from(labels[0].result.is_some()));
                    for value in values {
                        assert!(body.high_results.iter().any(|record| {
                            record.block == block && record.value == *value
                                && record.edge == ir::HighEdgeSelector::Normal(0)
                                && record.demand == ir::ResultDemand::Want && record.prepared.is_some()
                        }));
                    }
                }
                ir::TermKind::RetDefer { tag: ir::DeferReturnTag::Continue, values } => {
                    assert!(values.is_empty());
                }
                ir::TermKind::Ret { .. } => panic!("defer must use tagged returns"),
                _ => {}
            }
        }
    }
    assert!(replacements > 0);
}

#[test]
fn oir2_defer_return_p1_fixture() {
    let source = include_str!("fixtures/defer-return/p1-replacement.ok");
    let module = super::lower_std(source).unwrap();
    super::verify_high(&module, "P1 defer return replacement before print");
    let text = ir::text::print(&module).unwrap();
    let parsed = ir::text::parse(&text).unwrap();
    super::verify_high(&parsed, "P1 defer return replacement after parse");
    assert!(ir::text::normalized_eq(&module, &parsed).unwrap());
    assert_eq!(text, ir::text::print(&super::lower_std(source).unwrap()).unwrap());
    check_defer_entries(&module);
    let catalog = super::super::native_summaries::P1NativeSummaries::for_module(&module).unwrap();
    let options = ir::own::OwnOptions::default();
    let inferred = ir::own::elaborate_ownership(module, &catalog, &options).unwrap();
    super::verify_high(&inferred.module, "P1 defer return replacement after inference");
}
