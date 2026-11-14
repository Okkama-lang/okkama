//! C1 inventory checks. These do not claim source lowering or runtime acceptance.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const MANIFEST: &str = include_str!("p2_4.tsv");
const MATRIX: &str = include_str!("p2_4-matrix.md");

fn rows(manifest: &str) -> Vec<Vec<&str>> {
    manifest.lines().skip(1).map(|line| line.split('\t').collect()).collect()
}

// Only scalar metadata is read here; case.json remains the normative expectation.
// Values used by this inventory are ASCII IDs, modes, outcomes and integer exits.
fn scalar<'a>(source: &'a str, key: &str) -> Option<&'a str> {
    let key = format!("\"{key}\"");
    let (_, suffix) = source.split_once(&key)?;
    let suffix = suffix.trim_start().strip_prefix(':')?.trim_start();
    if let Some(quoted) = suffix.strip_prefix('"') {
        return quoted.split_once('"').map(|(value, _)| value);
    }
    Some(suffix.split([',', '}', '\n']).next()?.trim())
}

fn conformance_paths(root: &Path) -> BTreeMap<String, PathBuf> {
    let mut paths = BTreeMap::new();
    for level in std::fs::read_dir(root.join("conformance/cases")).unwrap() {
        let level = level.unwrap().path();
        if !level.is_dir() {
            continue;
        }
        for case in std::fs::read_dir(level).unwrap() {
            let path = case.unwrap().path().join("case.json");
            if !path.is_file() {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let id = scalar(&source, "id").unwrap().to_owned();
            assert!(paths.insert(id, path).is_none());
        }
    }
    paths
}

fn required_case(id: &str, inherited: &BTreeSet<&str>) -> bool {
    if inherited.contains(id) {
        return true;
    }
    let prefixes = [
        "L01.gram.anon.", "L01.gram.test.", "L03.anon.", "L03.anonymous.",
        "L03.action-types.", "L03.bound.", "L04.caps.", "L04.fakes.",
        "L04.successor.closure-", "L04.successor.bound-action-", "L04.test.close-text-bytes-",
        "L06.native.", "L07.test-runner.",
    ];
    if prefixes.iter().any(|prefix| id.starts_with(prefix)) {
        return true;
    }
    [
        "L00.entry.any-result-rejected", "L01.gram.hidden.test-among-actions", "L01.gram.role.members",
        "L01.gram.types.forms", "L01.gram.types.void-parameter-type", "L01.gram.params.grouped",
        "L02.equality.signed-zero-composite", "L03.match.on-any", "L03.next-last.across-anonymous-action",
        "L03.next-last.inside-anonymous-loop", "L03.return.anonymous-boundary", "L03.overloads.ambiguous",
        "L03.overloads.selection", "L03.variadic.overload-by-tail-type", "L03.variadic.string-tail-converts",
        "L04.actions.result-type-checked", "L04.ctor.typed-overload", "L04.invariant.escaped-self-bound-action",
        "L04.invariant.escaped-self-failed-constructor", "L04.invariant.escaped-self-in-block-failed-constructor",
        "L04.once.keeps-plain-closure-and-role", "L04.once.rejects-captured-resource",
        "L04.privacy.closure-reads-self-field", "L04.successor.assign-wrong-type",
        "L05.batching.preferred-over-cursor", "L06.file-identity.path-aliases", "L07.regions.role-instances",
        "L08.atomic.compare-and-replace", "L09.time.zones", "L11.sqlite.throughput-lifetime",
        "L12.await-cap-continuation", "L12.caps-in-collections-and-closures", "L12.concurrent-cas-and-dedup",
        "L13.ambiguous-outcome-reconciliation",
    ].contains(&id)
}

#[test]
fn oir2_p2_4_manifest_schema_paths_and_case_contracts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let header = "id\tpath\tfamily\tcommit\tdependencies\tstatus\tmode\toutcome\texit\tinherited\tsubcase";
    assert_eq!(MANIFEST.lines().next(), Some(header));
    let mut ids = BTreeSet::new();
    let mut previous = "";
    let mut full_cases = BTreeSet::new();
    let mut inherited = BTreeSet::new();
    let mut families = BTreeMap::new();
    let mut commits = BTreeMap::new();
    let mut dependencies = BTreeMap::new();
    for row in rows(MANIFEST) {
        assert_eq!(row.len(), 11, "{row:?}");
        assert!(row.iter().all(|value| !value.is_empty()), "{row:?}");
        let id = row[0];
        assert!(ids.insert(id), "duplicate {id}");
        assert!(previous < id, "unsorted {id}");
        previous = id;
        let path = root.join(row[1]);
        assert!(path.is_file(), "missing {}", row[1]);
        assert!(!Path::new(row[1]).is_absolute());
        assert!(!row[1].split('/').any(|part| part == ".."));
        assert!([
            "closure", "bound action", "cap", "any", "native", "cell/atomic", "test profile", "once",
        ].contains(&row[2]), "{id}");
        assert!(["C2", "C3", "C4", "C5", "C6", "C7", "C8", "C9"].contains(&row[3]));
        let mut row_dependencies = BTreeSet::new();
        for dependency in row[4].split(',') {
            assert!(["none", "#565-c5", "#570", "#593", "backend"].contains(&dependency), "{id}");
            assert!(row_dependencies.insert(dependency), "duplicate dependency {id}");
            *dependencies.entry(dependency).or_insert(0) += 1;
        }
        assert!(!row_dependencies.contains("none") || row_dependencies.len() == 1);
        assert_eq!(row_dependencies.contains("#565-c5"), row_dependencies.contains("#570"), "{id}");
        assert_eq!(row_dependencies.contains("#593"), row_dependencies.contains("backend"), "{id}");
        assert_eq!(row[5], "planned", "C1/C2a/C3a has no complete source acceptance: {id}");
        assert!(["parse", "check", "run", "test"].contains(&row[6]));
        assert!(["success", "error", "failure", "reject"].contains(&row[7]));
        assert!(row[8] == "-" || row[8].parse::<u8>().is_ok());
        if row[9] != "-" {
            assert_eq!(row[9], id);
            assert!(inherited.insert(id));
        }
        *families.entry(row[2]).or_insert(0) += 1;
        *commits.entry(row[3]).or_insert(0) += 1;
        if id.starts_with('L') {
            let source = std::fs::read_to_string(path).unwrap();
            let base = id.split('#').next().unwrap();
            assert_eq!(scalar(&source, "id"), Some(base), "{id}");
            assert_eq!(scalar(&source, "mode"), Some(row[6]), "{id}");
            assert_eq!(scalar(&source, "outcome"), Some(row[7]), "{id}");
            assert_eq!(scalar(&source, "exit_status").unwrap_or("-"), row[8], "{id}");
            if !id.contains('#') {
                assert!(full_cases.insert(base));
            }
        } else {
            assert!(id.starts_with("ownership."), "{id}");
            assert!(row[1].starts_with("tests/ownership-corpus/"));
        }
    }
    assert_eq!(ids.len(), 162);
    assert_eq!(inherited.len(), 22);
    assert_eq!(full_cases.len(), 124);
    let expected: BTreeSet<_> = conformance_paths(&root).into_keys()
        .filter(|id| required_case(id, &inherited)).collect();
    let actual: BTreeSet<_> = full_cases.into_iter().map(str::to_owned).collect();
    assert_eq!(actual, expected, "missing/extra case in the C1 inventory");
    assert_eq!(families, BTreeMap::from([
        ("any", 35), ("bound action", 12), ("cap", 21), ("cell/atomic", 16),
        ("closure", 45), ("native", 8), ("once", 3), ("test profile", 22),
    ]));
    assert_eq!(commits, BTreeMap::from([
        ("C2", 3), ("C3", 41), ("C4", 11), ("C5", 18),
        ("C6", 37), ("C7", 22), ("C8", 22), ("C9", 8),
    ]));
    assert_eq!(dependencies["#570"], 15);
    assert_eq!(dependencies["#593"], 100);
}

#[test]
fn oir2_p2_4_inherited_cross_references_and_corpus_observations() {
    let current = rows(MANIFEST);
    let index: BTreeMap<_, _> = current.iter().map(|row| (row[0], row)).collect();
    let mut links = 0;
    let mut original = 0;
    for row in rows(include_str!("p2_3.tsv")) {
        if !row[4].split(',').any(|dependency| dependency == "#569") {
            continue;
        }
        let id = row[0];
        let target = index[id];
        assert_eq!(row[1], target[1], "cross-reference path {id}");
        assert!(row[6].ends_with(&format!("p2_4.tsv#{id}")), "missing link {id}");
        assert_eq!(row[5], "planned");
        if target[9] != "-" {
            original += 1;
        } else {
            assert_eq!(id, "L05.batching.preferred-over-cursor");
            assert_eq!(target[2], "cell/atomic");
        }
        links += 1;
    }
    assert_eq!(original, 22);
    assert_eq!(links, 23);
    for (name, checks) in [
        ("closures", 6), ("bound-actions", 5), ("caps", 4), ("roles-generics", 5), ("atomic-resources", 6),
    ] {
        let full = format!("ownership.{name}.ok");
        assert!(index.contains_key(full.as_str()));
        for check in 1..=checks {
            let id = format!("{full}#check-{check}");
            assert!(index.contains_key(id.as_str()), "missing {id}");
        }
    }
    for id in [
        "ownership.atomic-resources.ok", "L04.fakes.typed-state", "L07.test-runner.all-pass-async",
        "L08.atomic.compare-and-replace", "L12.await-cap-continuation",
    ] {
        assert!(index[id][4].split(',').any(|dependency| dependency == "#570"), "{id}");
    }
    for id in [
        "ownership.atomic-resources.ok#sync", "L04.fakes.typed-state#sync", "L07.test-runner.all-pass-async#sync",
    ] {
        assert!(!index[id][4].split(',').any(|dependency| dependency == "#570"), "{id}");
    }
}

#[test]
fn oir2_p2_4_protocol_mapping_is_complete() {
    let names = concat!(
        "any-borrowed any-fresh any-known-plain bound-role bound-snapshot cap-call cap-hidden cap-parameter ",
        "closure-borrowed closure-empty closure-fresh closure-known closure-known-join closure-known-loop ",
        "closure-known-phi closure-open descriptor-binding descriptor-plain-parameter dynamic-any ",
        "dynamic-atomic dynamic-cell dynamic-channel dynamic-closure dynamic-group once-plain role-call ",
        "role-parameter role-plain service-schedule-empty",
    );
    let expected: BTreeSet<_> = names.split_whitespace().collect();
    let mut mapped = BTreeSet::new();
    let mut matches = 0;
    let mut differences = 0;
    for line in MATRIX.lines().filter(|line| line.starts_with("| ")) {
        let columns: Vec<_> = line.split('|').map(str::trim).collect();
        if columns.len() != 5 || !["match", "differ"].contains(&columns[3]) {
            continue;
        }
        assert!(mapped.insert(columns[1]), "duplicate protocol mapping");
        assert!(!columns[2].is_empty());
        if columns[3] == "match" {
            matches += 1;
        } else {
            differences += 1;
        }
    }
    assert_eq!(mapped, expected);
    assert_eq!((matches, differences), (16, 13));
    for form in [
        "Closure", "BoundAction", "BoundRoleAction", "CallValue", "CapSelect", "CapRead", "OwnerField",
        "Coercion::Box", "BodyOwner::Test", "Callee::Test", "fakes after desugaring", "Cell/Atomic",
    ] {
        assert!(MATRIX.contains(form), "missing THIR form {form}");
    }
}

#[test]
fn oir2_p2_4_opaque_bridge_inventory_matches_runtime_abi() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let table = root.join("okkama-rs/crates/okkama-runtime-abi/src/table.rs");
    let table = std::fs::read_to_string(table).unwrap();
    let decisions = include_str!("p2_4-decisions.md");
    let mut bridges = BTreeSet::new();
    for line in table.lines() {
        let parts: Vec<_> = line.split('"').collect();
        if parts.len() != 5 || !parts[3].contains("any") {
            continue;
        }
        let wire = parts[0].split_once('=').unwrap().1.trim().trim_end_matches(',');
        let symbol = parts[1];
        let signature = parts[3];
        assert!(bridges.insert(wire));
        let entry = format!("| {wire} | `{symbol}` | `{signature}` |");
        assert!(decisions.contains(&entry), "unclassified opaque native {symbol}");
    }
    assert_eq!(bridges, BTreeSet::from(["140", "153", "529", "530", "531", "532", "533", "534", "535"]));
}
