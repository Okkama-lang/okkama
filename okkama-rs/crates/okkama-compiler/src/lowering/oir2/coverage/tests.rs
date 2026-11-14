//! The inventory is TSV so checking it needs no new compiler dependencies.
#[test]
fn oir2_p2_3_coverage_paths_and_inventory() {
    use std::collections::BTreeSet;
    use std::path::Path;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let manifest = include_str!("p2_3.tsv");
    let mut lines = manifest.lines();
    assert_eq!(lines.next(), Some("id\tpath\tfamily\tcommit\tdependencies\tstatus\tsubcase"));
    let mut ids = BTreeSet::new();
    let mut l05 = BTreeSet::new();
    let mut l09 = BTreeSet::new();
    for line in lines {
        let row: Vec<_> = line.split('\t').collect();
        assert_eq!(row.len(), 7, "{line}");
        assert!(ids.insert(row[0]), "duplicate {}", row[0]);
        assert!(root.join(row[1]).is_file(), "missing {}", row[1]);
        assert!(!row[2].is_empty());
        assert!(["C2", "C3", "C4", "C5", "C6", "C7", "C8", "C9"].contains(&row[3]));
        for dependency in row[4].split(',') {
            assert!(["none", "#565-audit", "#565-c4", "#565-c5", "#569", "#570", "#593", "backend"]
                .contains(&dependency), "unknown dependency {dependency}");
        }
        assert_eq!(row[5], "planned");
        if row[0].starts_with("L05.") {
            assert!(l05.insert(root.join(row[1])));
        }
        if row[0].starts_with("L09.collections.") {
            assert!(l09.insert(root.join(row[1])));
        }
    }
    let inventory = |directory: &str, prefix: &str| -> BTreeSet<_> {
        std::fs::read_dir(root.join(directory)).unwrap().map(|entry| entry.unwrap().path())
            .filter(|path| path.is_dir())
            .filter(|path| path.file_name().unwrap().to_str().unwrap().starts_with(prefix))
            .map(|path| path.join("case.json")).collect()
    };
    assert_eq!(l05, inventory("conformance/cases/L05-types-generics-and-collections", ""));
    assert_eq!(l09, inventory("conformance/cases/L09-stdlib-pure", "collections."));
    assert_eq!(l05.len(), 127);
    assert_eq!(l09.len(), 4);
    assert_eq!(ids.len(), 145);
}
