//! Tweed #905: whether an object is tracked is decided per instance. These programs fix what must not change:
//! a resource reached through a role field of an object travels with that object, and objects that reach no
//! resource cost nothing. The result encodes the order of events (a log of digits), so a resource destroyed early,
//! late or twice changes it.
#[path = "sparrow_support/mod.rs"]
mod support;
use okkama_compiler::{SourceFile, ir, lowering, syntax};
use std::path::PathBuf;

struct Stdlib(PathBuf);
impl lowering::ModuleLoader for Stdlib {
    fn load(&mut self, path: &str, _importer: &str) -> Result<Option<lowering::LoadedModule>, String> {
        let file = self.0.join(format!("{path}.ok"));
        if !file.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&file).map_err(|error| error.to_string())?;
        let id = file.to_string_lossy().into_owned();
        let source = SourceFile::new(id.clone(), text).unwrap();
        let program = syntax::parse(&source, &syntax::ParseOptions::default()).unwrap();
        Ok(Some(lowering::LoadedModule { id, source, program: syntax::prune::strip_test_items(program) }))
    }
}

fn lower(text: &str) -> ir::Module {
    let text = format!("module app v1.0.0; import \"async/atomic\" take Atomic; {text}");
    let source = SourceFile::new("tweed.ok", text).unwrap();
    let program = syntax::parse(&source, &syntax::ParseOptions::default()).unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../std");
    lowering::lower_linked(&program, source.name(), &mut Stdlib(root)).unwrap()
}

const LEDGERS: &str = r#"
    object Log {
        has n is Atomic<int>;
        Log(:n);
        add(digit is int) is void { n.replace(n.value() * 10 + digit); }
        value() is int { n.value(); }
    }
    role Ledger { balance() is int; }
    object Account does Ledger {
        Account();
        balance() is int { 1; }
    }
    object Vault does Ledger {
        has log is Log;
        has id is int;
        Vault(:log, :id);
        balance() is int { 2; }
        destroy { log.add(id); }
    }
    object Record {
        has ledger is Ledger;
        Record(:ledger);
        total() is int { ledger.balance(); }
    }
"#;

#[test]
fn a_resource_inside_a_record_leaves_with_the_records_array_and_closes_with_its_new_owner() {
    let module = lower(&format!("{LEDGERS}{}", r#"
        object Maker {
            Maker();
            make(log is Log) is @Record {
                [Record(ledger: Account()), Record(ledger: Vault(log: log, id: 3)), Record(ledger: Account())];
            }
        }
        object Probe {
            Probe();
            run(log is Log) is int {
                let records = Maker().make(log);
                let before = log.value();
                records[1].total() * 100 + records[0].total() * 10 + before;
            }
        }
        main() is int {
            let log = Log(n: Atomic(0));
            let result = Probe().run(log);
            result * 10 + log.value();
        }
    "#));
    // Inside run the vault is alive (log 0): 2·100 + 1·10 + 0 = 210; it closes when run's scope ends: log 3.
    support::assert_value(&module, "2103");
}

#[test]
fn records_holding_only_plain_ledgers_never_close_anything() {
    let module = lower(&format!("{LEDGERS}{}", r#"
        object Probe {
            Probe();
            run(log is Log, n is int) is int {
                let records = (0..n).map() -> i { Record(ledger: Account()); };
                records.reduce(0) -> sum, record { sum + record.total(); };
            }
        }
        main() is int {
            let log = Log(n: Atomic(0));
            let fee = Vault(log: log, id: 7);
            Probe().run(log, 50) * 10 + log.value() + fee.balance() * 0;
        }
    "#));
    // 50 plain records: 50·10, nothing closed inside run; the fee closes after main's result is computed.
    support::assert_value(&module, "500");
}

#[test]
fn a_resource_nested_two_objects_deep_is_found_through_both() {
    let module = lower(&format!("{LEDGERS}{}", r#"
        object Folder {
            has record is Record;
            Folder(:record);
            total() is int { record.total(); }
        }
        object Maker {
            Maker();
            make(log is Log) is Folder { Folder(record: Record(ledger: Vault(log: log, id: 4))); }
        }
        object Probe {
            Probe();
            run(log is Log) is int {
                let folder = Maker().make(log);
                folder.total() * 10 + log.value();
            }
        }
        main() is int {
            let log = Log(n: Atomic(0));
            let result = Probe().run(log);
            result * 10 + log.value();
        }
    "#));
    // Alive inside run (2·10 + 0), closed when run's scope ends (log 4).
    support::assert_value(&module, "204");
}

#[test]
fn indexed_containers_reclaimed_and_reallocated_never_lend_their_index_to_a_new_container() {
    // #906: many rounds build and drop arrays of records (one with a vault) and transfer each through a call,
    // which walks it through its structure index; every vault closes exactly once.
    let module = lower(&format!("{LEDGERS}{}", r#"
        object Shelf {
            has records is @Record;
            Shelf(:records);
            total() is int { records.reduce(0) -> sum, record { sum + record.total(); }; }
        }
        object Maker {
            Maker();
            shelf(log is Log, round is int) is Shelf {
                let records = (0..64).map() -> i {
                    let ledger is Ledger = if i == round % 64 { Vault(log: log, id: 1); } else { Account(); };
                    Record(ledger: ledger);
                };
                Shelf(records: records);
            }
        }
        object Probe {
            Probe();
            round(log is Log, round is int) is int { Maker().shelf(log, round).total(); }
        }
        main() is int {
            let log = Log(n: Atomic(0));
            let sum = (0..400).reduce(0) -> sum, round { sum + Probe().round(log, round); };
            // Each round: 63 accounts (1) and a vault (2) = 65; every vault closes exactly once (a digit 1 each).
            let ones = (0..400).reduce(0) -> digits, round { digits * 10 + 1; };
            if log.value() == ones { sum; } else { -1; }
        }
    "#));
    support::assert_value(&module, "26000");
}

#[test]
fn the_ownership_benchmark_shape_repeated_keeps_its_result_and_closes_every_vault_once() {
    // #906: 11-ownership's shape (records behind role fields, shelves of slices built by reduce, a view per
    // round over both), repeated: the same result every time and every vault closed exactly once. (The
    // reclaimed-index invariant itself is the unit test `structure_tests` in sparrow-core.)
    let module = lower(&format!("{LEDGERS}{}", r#"
        object View {
            has records is @Record;
            has shelves is %int:@Record;
            has at is int;
            View(:records, :shelves, :at);
            total() is int {
                let shelf = shelves[at % shelves.size()] else records;
                records[at].total() + shelf[at % shelf.size()].total();
            }
        }
        object Store {
            has records is @Record;
            has shelves is %int:@Record;
            Store(log is Log, racks is int) {
                let all = (0..racks * 20).map() -> i {
                    let ledger is Ledger = if i % 200 == 0 { Vault(log: log, id: 1); } else { Account(); };
                    Record(ledger: ledger);
                };
                let first is %int:@Record = [0: all.slice(0, 20)];
                records = all;
                shelves = (1..racks).reduce(first) -> found, shelf {
                    found + [shelf: all.slice(shelf * 20, shelf * 20 + 20)];
                };
            }
            sum(log is Log, rounds is int) is int {
                (0..rounds).reduce(0) -> sum, round {
                    let fee = Vault(log: log, id: 1);
                    let view = View(records: records, shelves: shelves, at: (round * 7919) % records.size());
                    sum + view.total() + fee.balance();
                };
            }
        }
        object Probe {
            Probe();
            once(log is Log) is int { Store(log, 100).sum(log, 30); }
        }
        main() is int {
            let log = Log(n: Atomic(0));
            let sums = (0..12).map() -> repeat { Probe().once(log); };
            let first = sums[0];
            let same = sums.reduce(true) -> all, sum { all && sum == first; };
            // Per repetition 10 store vaults and 30 fees close: 40 digits 1, 480 in all.
            let ones = (0..480).reduce(0) -> digits, round { digits * 10 + 1; };
            if same && log.value() == ones { first; } else { -1; }
        }
    "#));
    let value = support::value(&module);
    assert_ne!(value, "-1", "a result changed between repetitions or a vault did not close exactly once");
}
