//! C5 precommit regression and verified producer goldens.
use crate::oir2 as ir;

#[test]
fn oir2_c5_precommit_preserves_input() {
    for source in [
        include_str!("../../../../okkama-oir/tests/fixtures/verify/valid/B81-high-precommit-push.oir"),
        include_str!("../../../../okkama-oir/tests/fixtures/verify/valid/B81-high-precommit-put.oir"),
        include_str!("../../../../okkama-oir/tests/fixtures/verify/valid/B81-high-precommit-finish.oir"),
        include_str!("../../../../okkama-oir/tests/fixtures/verify/valid/B81-high-precommit-map-finish.oir"),
    ] {
        let module = ir::text::parse(source).unwrap();
        ir::verify::verify_high(&module).unwrap();
        let printed = ir::text::print(&module).unwrap();
        let parsed = ir::text::parse(&printed).unwrap();
        ir::verify::verify_high(&parsed).unwrap();
        assert_eq!(printed, ir::text::print(&parsed).unwrap());
    }
}

#[test]
fn oir2_c5_builder_goldens() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lowering/oir2");
    let mut paths: Vec<_> = std::fs::read_dir(root.join("fixtures/p2_3/builders")).unwrap()
        .map(|entry| entry.unwrap().path()).collect();
    paths.sort();
    assert_eq!(paths.len(), 14);
    for path in paths {
        let name = path.file_stem().unwrap().to_str().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        super::collection_tests::with_analysis(&text, |analysis| {
            let module = super::lower_to_oir2(analysis).unwrap_or_else(|error| panic!("{name}: {error}"));
            ir::verify::verify_high(&module).unwrap();
            let actual = ir::text::print(&module).unwrap();
            let parsed = ir::text::parse(&actual).unwrap();
            ir::verify::verify_high(&parsed).unwrap();
            assert!(ir::text::normalized_eq(&module, &parsed).unwrap());
            assert_eq!(actual, ir::text::print(&parsed).unwrap());
            let second = super::lower_to_oir2(analysis).unwrap();
            ir::verify::verify_high(&second).unwrap();
            assert_eq!(actual, ir::text::print(&second).unwrap());
            let expected = root.join("goldens/p2_3/builders").join(format!("{name}.oir"));
            if std::env::var_os("OKKAMA_OIR2_P2_WRITE_GOLDENS").is_some() {
                std::fs::write(&expected, &actual).unwrap();
            }
            let stored = std::fs::read_to_string(expected).unwrap();
            ir::verify::verify_high(&ir::text::parse(&stored).unwrap()).unwrap();
            assert_eq!(actual, stored, "{name}");
        });
    }
}
