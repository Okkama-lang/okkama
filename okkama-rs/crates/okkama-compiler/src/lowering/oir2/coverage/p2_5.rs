//! P2.5 (#601) coverage inventory: one row per L08 case and per async case elsewhere (plan §1.3),
//! with the slice that brings it to its expected outcome. These checks claim no lowering or runtime
//! acceptance; case.json remains the normative expectation.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::lowering::oir2::async_tests::W1;

const MANIFEST: &str = include_str!("p2_5.tsv");
const HEADER: &str = "id\tpath\tslice\twave\tmode\toutcome\texit\tgolden\tblocker\tnote";
const GOLDENS: &str = "okkama-rs/crates/okkama-compiler/src/lowering/oir2/goldens/p2_5";

/// The wave of each slice (plan §4). `frontend`, `parse` and `pruned` cases need no async slice:
/// the frontend rejects them, they are only parsed, or production prunes their async test.
fn wave(slice: &str) -> Option<&'static str> {
    Some(match slice {
        "V5" => "W1",
        "R3" | "K2" => "W2",
        "P1" | "Q1" | "Q2" | "G1" | "N1" | "F1" => "W3",
        "X1" | "D1" => "W4",
        "frontend" | "parse" | "pruned" => "-",
        _ => return None,
    })
}

/// Non-async producer stops observed at V5 (plan §6.2); they do not count against async slices.
const BLOCKERS: [&str; 5] = ["-", "atomic-nonplain", "catch-cancelled", "native-failure", "role-adapter"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn rows() -> Vec<Vec<&'static str>> {
    MANIFEST.lines().skip(1).map(|line| line.split('\t').collect()).collect()
}

fn case_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Identifier words and import paths of Okkama source, outside comments and literals.
fn words(text: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut words = BTreeSet::new();
    let mut imports = BTreeSet::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, first)) = chars.next() {
        let rest = &text[start..];
        if rest.starts_with("//") {
            while chars.next_if(|&(_, next)| next != '\n').is_some() {}
        } else if rest.starts_with("/*") {
            chars.next();
            let mut previous = ' ';
            for (_, next) in chars.by_ref() {
                if previous == '*' && next == '/' {
                    break;
                }
                previous = next;
            }
        } else if first == '"' || first == '\'' {
            let mut literal = String::new();
            while let Some((_, next)) = chars.next() {
                if next == '\\' {
                    chars.next();
                } else if next == first {
                    break;
                } else {
                    literal.push(next);
                }
            }
            if first == '"' && text[..start].trim_end().ends_with("import") {
                imports.insert(literal);
            }
        } else if first.is_ascii_alphabetic() || first == '_' {
            let mut end = start + 1;
            while let Some((index, _)) = chars.next_if(|&(_, next)| next.is_ascii_alphanumeric() || next == '_') {
                end = index + 1;
            }
            words.insert(text[start..end].to_owned());
        }
    }
    (words, imports)
}

/// A case is async when its sources use `async`, `await` or `spawn`, or import an `async/*` module
/// other than `async/atomic` (the synchronous Atomic belongs to the P2.4 inventory).
fn async_cases(root: &Path) -> BTreeMap<String, PathBuf> {
    let mut cases = BTreeMap::new();
    for level in std::fs::read_dir(root.join("conformance/cases")).unwrap() {
        let level = level.unwrap().path();
        if !level.is_dir() {
            continue;
        }
        for case in std::fs::read_dir(level).unwrap() {
            let directory = case.unwrap().path();
            let metadata = directory.join("case.json");
            if !metadata.is_file() {
                continue;
            }
            let mut found = BTreeSet::new();
            let mut imports = BTreeSet::new();
            for source in std::fs::read_dir(&directory).unwrap() {
                let source = source.unwrap().path();
                if source.extension().is_some_and(|extension| extension == "ok") {
                    let (words, paths) = words(&std::fs::read_to_string(source).unwrap());
                    found.extend(words);
                    imports.extend(paths);
                }
            }
            let keyword = ["async", "await", "spawn"].iter().any(|word| found.contains(*word));
            let module = imports.iter().any(|path| path.starts_with("async/") && path != "async/atomic");
            if keyword || module {
                let id = case_json(&metadata)["id"].as_str().unwrap().to_owned();
                assert!(cases.insert(id, metadata).is_none());
            }
        }
    }
    cases
}

#[test]
fn oir2_p2_5_manifest_schema_paths_and_case_contracts() {
    let root = root();
    assert_eq!(MANIFEST.lines().next(), Some(HEADER));
    let mut previous = "";
    for row in rows() {
        assert_eq!(row.len(), 10, "{row:?}");
        assert!(row.iter().all(|value| !value.is_empty()), "{row:?}");
        let id = row[0];
        assert!(previous < id, "unsorted or duplicate {id}");
        previous = id;
        assert!(!Path::new(row[1]).is_absolute(), "{id}");
        assert!(!row[1].split('/').any(|part| part == ".."), "{id}");
        let case = case_json(&root.join(row[1]));
        assert_eq!(case["id"], id);
        assert_eq!(case["mode"], row[4], "{id}");
        assert_eq!(case["expect"]["outcome"], row[5], "{id}");
        let exit = case["expect"]["exit_status"].as_i64().map_or("-".to_owned(), |exit| exit.to_string());
        assert_eq!(exit, row[6], "{id}");
        assert_eq!(wave(row[2]), Some(row[3]), "{id}: slice {} and wave {}", row[2], row[3]);
        assert!(BLOCKERS.contains(&row[8]), "{id}: blocker {}", row[8]);
        if row[2] == "frontend" {
            assert_ne!(case["expect"]["outcome"], "success", "{id}: a frontend case expects a rejection");
        }
        if row[2] == "parse" {
            assert_eq!(row[4], "parse", "{id}");
        }
    }
}

#[test]
fn oir2_p2_5_manifest_covers_every_async_case() {
    let root = root();
    let listed: BTreeMap<_, _> = rows().into_iter().map(|row| (row[0].to_owned(), root.join(row[1]))).collect();
    let expected = async_cases(&root);
    assert_eq!(listed.keys().collect::<Vec<_>>(), expected.keys().collect::<Vec<_>>(),
        "missing/extra case in the P2.5 inventory");
    for (id, path) in &expected {
        assert_eq!(&listed[id], path, "{id}");
    }
    let level = |prefix: &str| listed.keys().filter(|id| id.starts_with(prefix)).count();
    // #739 adds the four K2 probes as L08 cases (three Q1, one D1).
    assert_eq!((listed.len(), level("L08.")), (105, 62));
    let mut slices = BTreeMap::new();
    for row in rows() {
        *slices.entry(row[2]).or_insert(0) += 1;
    }
    assert_eq!(slices, BTreeMap::from([
        ("D1", 6), ("F1", 3), ("G1", 11), ("K2", 1), ("N1", 24), ("P1", 3), ("Q1", 18), ("Q2", 8),
        ("R3", 2), ("V5", 10), ("X1", 1), ("frontend", 14), ("parse", 3), ("pruned", 1),
    ]));
}

#[test]
fn oir2_p2_5_w1_rows_name_reviewed_goldens() {
    let root = root();
    let mut goldens = BTreeSet::new();
    for row in rows() {
        if row[7] == "-" {
            continue;
        }
        assert_eq!(row[2], "V5", "{}: only V5 rows have High goldens", row[0]);
        assert_eq!(row[7], format!("{GOLDENS}/{}.oir", row[0]));
        assert!(root.join(row[7]).is_file(), "missing golden {}", row[7]);
        goldens.insert(row[0]);
    }
    assert_eq!(goldens, BTreeSet::from(W1));
}

#[test]
fn oir2_p2_5_source_words_skip_comments_and_literals() {
    let (found, imports) = words(concat!(
        "import \"async/event\" take Event; // await in a comment\n",
        "/* spawn in a block */ main() is int { let s = \"async \\\" é\"; let c = 'a'; 0; }\n",
    ));
    assert!(found.contains("main") && found.contains("Event"));
    for word in ["async", "await", "spawn"] {
        assert!(!found.contains(word), "{word}");
    }
    assert_eq!(imports, BTreeSet::from(["async/event".to_owned()]));
    let (found, _) = words("async run() is int { await x; spawn Y().z(); }");
    assert!(["async", "await", "spawn"].iter().all(|word| found.contains(*word)));
}
