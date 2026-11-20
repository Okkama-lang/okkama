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
