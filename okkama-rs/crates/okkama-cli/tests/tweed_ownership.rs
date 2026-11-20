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

#[test]
fn a_resource_borrowed_by_a_bound_child_in_the_callers_group_closes_once_at_the_end() {
    // #905 regression (the Bible's 20-keeps): a FutureGroup is a runtime object whose members arrive after it is
    // made, so it is tracked per type, never judged a leaf at creation.
    borrowed_by_a_child_in_the_callers_group("let child = Reader().later(lamp, gate);\n        group.start(child);");
}

#[test]
fn a_resource_borrowed_by_a_temporary_child_in_the_callers_group_closes_once_at_the_end() {
    // #914: a child started as a temporary keeps what it borrows, as a bound one does (the Bible, 20.9).
    borrowed_by_a_child_in_the_callers_group("group.start(Reader().later(lamp, gate));");
}

/// The lent lamp is read by the child after `lend` returns and closes once, at the program's end. Run through
/// the binary: the close comes after main returns. `start` is how `lend` starts the child.
fn borrowed_by_a_child_in_the_callers_group(start: &str) {
    let program = r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/scope" take FutureGroup;
import "async/event" take Event;
object Lamp {
    has name is string;
    Lamp(:name);
    label() is string { name; }
    destroy { Stdout().writeln("off: " + name); }
}
object Reader {
    Reader();
    async later(lamp is Lamp, gate is Event) is int { await gate.wait(); Stdout().writeln("reads " + lamp.label()); 1; }
}
object Desk {
    Desk();
    async lend(group is FutureGroup<int>, gate is Event) is void {
        let lamp = Lamp(name: "lent");
        START
        Stdout().writeln("lend ends");
    }
}
async main() is int {
    let gate = Event();
    let group is FutureGroup<int> = FutureGroup();
    await Desk().lend(group, gate);
    gate.set();
    await group.wait();
    Stdout().writeln("main ends");
    0;
}
"#
    .replace("START", start);
    let dir = std::env::temp_dir().join(format!("tweed-keeps-{}-{}", std::process::id(), start.len()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("keeps.ok");
    std::fs::write(&file, &program).unwrap();
    let std = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../std");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_okkama"))
        .args(["run", file.to_str().unwrap()])
        .env("OKKAMA_STDLIB_DIR", std)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(String::from_utf8_lossy(&output.stdout), "lend ends\nreads lent\nmain ends\noff: lent\n",
        "stderr: {}", String::from_utf8_lossy(&output.stderr));
}

/// #909 2b: whether `symbol` destroys its resources directly (no region operations left).
fn is_static(module: &ir::Module, symbol: &str) -> bool {
    let function = module.functions.iter().find(|function| function.symbol == symbol).expect(symbol);
    let text = ir::print::function_text(module, function);
    text.contains("resource.leave") && !text.contains("region.")
}

#[test]
fn a_local_resource_closes_once_when_its_scope_ends_before_the_caller_reads_the_log() {
    let module = lower(&format!("{LEDGERS}{}", r#"
        object Probe {
            Probe();
            run(log is Log) is int {
                let vault = Vault(log: log, id: 3);
                let before = log.value();
                vault.balance() * 10 + before;
            }
        }
        main() is int {
            let log = Log(n: Atomic(0));
            let result = Probe().run(log);
            result * 10 + log.value();
        }
    "#));
    assert!(is_static(&module, "Probe.run"), "the vault never leaves run");
    // Inside run the vault is alive (log 0): 2·10 + 0 = 20; it closed when run's scope ended: log 3.
    support::assert_value(&module, "203");
}

const BRITTLE: &str = r#"
    enum Stop is Error = (now(code is int));
    object Brittle {
        has id is int;
        Brittle(:id);
        id() is int { id; }
        destroy { throw Stop.now(id); }
    }
"#;

#[test]
fn a_local_resource_closes_on_the_failure_path_before_the_caller_catches() {
    let module = lower(&format!("{LEDGERS}{}", r#"
        enum Stop is Error = (now(code is int));
        object Probe {
            Probe();
            run(log is Log) is int {
                let vault = Vault(log: log, id: 3);
                confirm log.value() > 0 else throw Stop.now(4);
                vault.balance();
            }
        }
        main() is int {
            let log = Log(n: Atomic(0));
            try { Probe().run(log); } catch Stop.now(code) { code * 10 + log.value(); };
        }
    "#));
    assert!(is_static(&module, "Probe.run"));
    // The throw leaves run with the vault alive; it closes on the way out (log 3) before the catch reads the log.
    support::assert_value(&module, "43");
}

#[test]
fn a_failing_destroy_of_a_local_resource_fails_its_scope() {
    let module = lower(&format!("{BRITTLE}{}", r#"
        object Probe {
            Probe();
            run() is int { let brittle = Brittle(id: 5); brittle.id() * 10; }
        }
        main() is int { try { Probe().run(); } catch Stop.now(code) { code; }; }
    "#));
    assert!(is_static(&module, "Probe.run"));
    support::assert_value(&module, "5");
}

#[test]
fn a_destroy_failing_on_the_failure_path_stays_secondary_to_the_failure_in_flight() {
    let module = lower(&format!("{BRITTLE}{}", r#"
        object Probe {
            Probe();
            run(stop is bool) is int {
                let brittle = Brittle(id: 5);
                confirm !stop else throw Stop.now(7);
                brittle.id();
            }
        }
        main() is int { try { Probe().run(true); } catch Stop.now(code) { code; }; }
    "#));
    assert!(is_static(&module, "Probe.run"));
    // The catch sees the primary failure (7); the destroy's own (5) rides along as secondary.
    support::assert_value(&module, "7");
}

/// Run `program` through the CLI binary; its standard output.
fn stdout_of(name: &str, program: &str) -> String {
    let dir = std::env::temp_dir().join(format!("tweed-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("main.ok");
    std::fs::write(&file, program).unwrap();
    let std = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../std");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_okkama"))
        .args(["run", file.to_str().unwrap()])
        .env("OKKAMA_STDLIB_DIR", std)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// #817 (memo OIR-2-async-remaining §2.4): a member started after its group's owner closed settles on its own,
/// a finished one releasing its outcome at the call; a registered member still running when the owner closes
/// detaches to the root, and the root closer destroys its outcome at shutdown, after the cancelled tasks.
#[test]
fn members_that_outlive_their_groups_owner_still_close_their_outcomes() {
    let output = stdout_of("late-members", r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/scope" take FutureGroup;
import "async/event" take Event;
object Res { has name is string; Res(:name); label() is string { name; } destroy { Stdout().writeln("destroy " + name); } }
object Maker {
    Maker();
    async make(name is string) is Res { Stdout().writeln("make " + name); Res(name: name); }
}
object Holding {
    Holding();
    async held(name is string, gate is Event) is Res { let r = Res(name: name); await gate.wait(); r; }
}
object Late {
    Late();
    async run(group is FutureGroup<Res>, gate is Event, hold is Event, done is Event) is Res {
        await gate.wait();
        group.start(Maker().make("late done"));
        Stdout().writeln("after late done");
        group.start(Holding().held("late running", hold));
        Stdout().writeln("after late running, active " + group.active().string());
        done.set();
        Res(name: "late body");
    }
}
object Helper {
    Helper();
    async go(gate is Event, hold is Event, done is Event) is void {
        let group is FutureGroup<Res> = FutureGroup();
        group.start(Late().run(group, gate, hold, done));
        Stdout().writeln("owner closes");
    }
}
async main() is int {
    let gate = Event();
    let hold = Event();
    let done = Event();
    await Helper().go(gate, hold, done);
    gate.set();
    await done.wait();
    Stdout().writeln("end");
    0;
}
"#);
    assert_eq!(output, "owner closes\nmake late done\ndestroy late done\nafter late done\n\
        after late running, active 2\nend\ndestroy late running\ndestroy late body\n");
}


/// #845: the Futures a collector's own sequence makes (`item`) belong to the collector's driver; their outcomes are
/// destroyed once, newest first, when the scope awaiting the collector ends.
#[test]
fn futures_a_collectors_sequence_makes_close_with_the_awaiting_scope() {
    let output = stdout_of("sequence-futures", r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "async/collect" take Raced;
import "collections/sequence" take Sequencing, EmptySequence;
import "collections/iteration" take Cursor;

object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}

object Maker {
    Maker();
    async make(name is string) is Res {
        Stdout().writeln("make " + name);
        Res(name: name);
    }
}

object Made does Cursor<Future<Res>> {
    has at is int;
    has count is int;
    Made(:at, :count);
    available() is bool { at < count; }
    item() is Future<Res> { Maker().make("m" + at.string()); }
    successor() is Cursor<Future<Res>> { Made(at: at + 1, count: count); }
}

object Making does Sequencing<Future<Res>> {
    has count is int;
    Making(:count);
    cursor() is Cursor<Future<Res>> { Made(at: 0, count: count); }
    size() is int { count; }
    item(index is int) is Future<Res> {
        Stdout().writeln("item " + index.string());
        Maker().make("m" + index.string());
    }
    first() is Future<Res> { Maker().make("first"); }
    last() is Future<Res> { Maker().make("last"); }
}

async main() is int {
    {
        let won = await Raced(origins: Making(count: 2)).winner();
        Stdout().writeln("won " + won.label());
    }
    Stdout().writeln("after");
    0;
}
"#);
    assert_eq!(output, "item 0\nitem 1\nmake m0\nmake m1\nwon m0\ndestroy m1\ndestroy m0\nafter\n");
}

/// #907, late edges: an Atomic inside a big summarized container gains a resource after the container was built.
/// An Atomic names, it does not own: the resource closes once, with the scope that made it. (A `once let` slot
/// cannot keep a resource at all: the compiler refuses it.)
#[test]
fn an_atomic_in_a_summarized_container_borrows_a_late_resource() {
    let module = lower(&format!("{LEDGERS}{}", r#"
        object Slot {
            has cell is Atomic<Ledger>;
            Slot(:cell);
            put(ledger is Ledger) is void { cell.replace(ledger); }
        }
        object Probe {
            Probe();
            fill(n is int) is @Slot {
                (0..n).map() -> i { let plain is Ledger = Account(); Slot(cell: Atomic(plain)); };
            }
            late(log is Log, slots is @Slot) is int {
                let fee = Vault(log: log, id: 3);
                slots[5].put(fee);
                log.value();
            }
        }
        main() is int {
            let log = Log(n: Atomic(0));
            let slots = Probe().fill(100);
            let during = Probe().late(log, slots);
            100 + during * 10 + log.value();
        }
    "#));
    // The fee is alive inside `late` (log 0) and closes once when `late` ends (log 3), whatever holds its name.
    support::assert_value(&module, "103");
}

/// #910 found it: an early return from a nested scope runs the defers of the scopes it leaves; a fresh resource a
/// defer makes belongs to the defer's own scope (still open), not to the nested one the return already closed.
#[test]
fn a_resource_made_by_a_defer_on_an_early_return_closes_once() {
    let module = lower(&format!("{LEDGERS}{}", r#"
        object Probe {
            Probe();
            run(log is Log, early is bool) is int {
                defer Vault(log: log, id: 5).balance();
                if early {
                    let inner = 7;
                    return inner;
                }
                2;
            }
        }
        main() is int {
            let log = Log(n: Atomic(0));
            let result = Probe().run(log, true);
            result * 10 + log.value();
        }
    "#));
    // The defer's vault is made and destroyed on the way out of `run` (log 5).
    support::assert_value(&module, "75");
}

/// #909 2b: a resource from a factory (not a constructor) and a local resource of an async body are static too;
/// each closes once when its scope ends.
#[test]
fn factory_and_async_local_resources_close_once_with_their_scope() {
    let module = lower(&format!("{LEDGERS}{}", r#"
        object Factory {
            has log is Log;
            Factory(:log);
            vault(id is int) is Vault { Vault(log: log, id: id); }
        }
        object Probe {
            Probe();
            run(log is Log) is int {
                let vault = Factory(log: log).vault(3);
                vault.balance() * 10 + log.value();
            }
            async later(log is Log) is int {
                let vault = Vault(log: log, id: 4);
                vault.balance() * 10 + log.value();
            }
        }
        async main() is int {
            let log = Log(n: Atomic(0));
            let first = Probe().run(log);
            let second = await Probe().later(log);
            first * 1000 + second * 10 + log.value();
        }
    "#));
    assert!(is_static(&module, "Probe.run"), "a factory's fresh vault");
    assert!(is_static(&module, "Probe.later"), "an async body's vault");
    // run: 2·10 + 0 = 20, its vault closes (log 3); later: 2·10 + 3 = 23, its vault closes (log 34).
    support::assert_value(&module, "20264");
}
