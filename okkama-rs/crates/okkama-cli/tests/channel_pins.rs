//! #915 (owner's review of #587, 2026-11-21): an accepted channel message never depends on a shorter-lived
//! owner. Book 20.2's pins: a message pins the identities another owner keeps until delivery and the receiving
//! scope's end; an owner that closes first hands them to the pin's holder (the channel, the receiving Future, the
//! scope that took the value); a holder that ends first leaves them with their owner. `use()` fails on a
//! destroyed resource, so a use after destroy shows.
#[path = "sparrow_support/mod.rs"]
mod support;
use okkama_compiler::{SourceFile, ir, lowering, syntax};
use std::path::PathBuf;

struct Stdlib(PathBuf);
impl lowering::ModuleLoader for Stdlib {
    fn load(&mut self, path: &str, _importer: &str) -> Result<Option<lowering::LoadedModule>, String> {
        let file = self.0.join(format!("{path}.ok"));
        if !file.is_file() { return Ok(None); }
        let text = std::fs::read_to_string(&file).map_err(|error| error.to_string())?;
        let id = file.to_string_lossy().into_owned();
        let source = SourceFile::new(id.clone(), text).unwrap();
        let program = syntax::parse(&source, &syntax::ParseOptions::default()).unwrap();
        Ok(Some(lowering::LoadedModule { id, source, program: syntax::prune::strip_test_items(program) }))
    }
}

fn check(body: &str) {
    let text = format!(r#"
        module app v1.0.0;
        import "async/atomic" take Atomic;
        import "async/event" take Event;
        import "async/channel" take Channel;
        object Res {{
            has name is string;
            has log is Atomic<string>;
            has dead is Atomic<bool>;
            Res(name is string, log is Atomic<string>) {{ self.name = name; self.log = log; self.dead = Atomic(false); }}
            use() is string {{ confirm !dead.value(); name; }}
            destroy {{ dead.replace(true); log.replace(log.value() + name); }}
        }}
        object Pair {{
            has left is Res;
            has right is Res;
            Pair(:left, :right);
            names() is string {{ left.use() + right.use(); }}
        }}
        object Work {{
            Work();
            async deliver(channel is Channel<Res>, value is Res) is void {{ await channel.sending().send(value); }}
            async local(channel is Channel<Res>, log is Atomic<string>) is void {{
                let made = Res("L", log);
                await channel.sending().send(made);
            }}
            async holdAfter(channel is Channel<Res>, log is Atomic<string>, gate is Event) is void {{
                let made = Res("K", log);
                await channel.sending().send(made);
                await gate.wait();
                confirm made.use() == "K";
            }}
            async receive(channel is Channel<Res>) is Res {{ await channel.receiving().item(); }}
            async pairs(channel is Channel<Pair>, left is Res, right is Res) is void {{
                await channel.sending().send(Pair(left: left, right: right));
            }}
        }}
        async main() is int {{
            let log = Atomic("");
            {body}
            0;
        }}
    "#);
    let source = SourceFile::new("channel-pins.ok", text).unwrap();
    let program = syntax::parse(&source, &syntax::ParseOptions::default()).unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../std");
    let module = lowering::lower_linked(&program, source.name(), &mut Stdlib(root)).unwrap();
    ir::verify(&module).unwrap();
    assert_eq!(support::sparrow(&module), support::Ending::Value(0), "{body}");
}

// 1, 4: a fresh resource handed to an async helper that forwards it through a buffered channel; the helper
// completes before the receiver uses it.
#[test]
fn a_helpers_fresh_input_survives_in_a_buffered_channel() {
    check(r#"
        {
            let channel is Channel<Res> = Channel(capacity: 1);
            await Work().deliver(channel, Res("H", log));
            confirm log.value() == "";
            { let value = await Work().receive(channel); confirm value.use() == "H"; }
            confirm log.value() == "H";
        }
        confirm log.value() == "H";
    "#);
}

// 2: the same through a zero-capacity rendezvous.
#[test]
fn a_helpers_fresh_input_survives_a_rendezvous() {
    check(r#"
        {
            let channel is Channel<Res> = Channel(capacity: 0);
            let receiver = spawn Work().receive(channel);
            await Work().deliver(channel, Res("Z", log));
            confirm log.value() == "";
            { let value = await receiver; confirm value.use() == "Z"; }
            confirm log.value() == "";
        }
        confirm log.value() == "Z";
    "#);
}

// 3: a resource the sending action made locally before the send.
#[test]
fn a_senders_local_resource_survives_the_sender() {
    check(r#"
        {
            let channel is Channel<Res> = Channel(capacity: 1);
            await Work().local(channel, log);
            confirm log.value() == "";
            { let value = await Work().receive(channel); confirm value.use() == "L"; }
            confirm log.value() == "L";
        }
        confirm log.value() == "L";
    "#);
}

// 5: the receiver is done before the sender completes; the sender keeps its alias and destroys it itself.
#[test]
fn a_receiver_done_first_leaves_the_resource_with_the_sender() {
    check(r#"
        {
            let channel is Channel<Res> = Channel(capacity: 1);
            let gate = Event();
            let sender = spawn Work().holdAfter(channel, log, gate);
            { let value = await Work().receive(channel); confirm value.use() == "K"; }
            confirm log.value() == "";
            gate.set();
            await sender;
            confirm log.value() == "K";
        }
        confirm log.value() == "K";
    "#);
}

// 6: a blocked send cancelled before acceptance keeps its own cleanup (channel-transfer covers the order).
#[test]
fn a_cancelled_blocked_send_pins_nothing() {
    check(r#"
        {
            let channel is Channel<Res> = Channel(capacity: 0);
            let sender = spawn Work().deliver(channel, Res("C", log));
            sender.cancel();
            try { await sender; } catch e is Cancelled { e.text(); }
            confirm log.value() == "C";
            channel.sending().close();
        }
        confirm log.value() == "C";
    "#);
}

// 7: the sender is cancelled after acceptance: the message survives its unwinding.
#[test]
fn a_sender_cancelled_after_acceptance_leaves_the_message_alive() {
    check(r#"
        {
            let channel is Channel<Res> = Channel(capacity: 1);
            let gate = Event();
            let sender = spawn Work().holdAfter(channel, log, gate);
            sender.cancel();
            try { await sender; } catch e is Cancelled { e.text(); }
            confirm log.value() == "";
            { let value = await Work().receive(channel); confirm value.use() == "K"; }
            confirm log.value() == "K";
        }
        confirm log.value() == "K";
    "#);
}

// 8: an externally owned message whose owner outlives the receiver.
#[test]
fn a_borrowed_message_whose_owner_stays_alive() {
    check(r#"
        {
            let owned = Res("A", log);
            {
                let channel is Channel<Res> = Channel(capacity: 1);
                await channel.sending().send(owned);
                { let value = await channel.receiving().item(); confirm value.use() == "A"; }
                confirm log.value() == "";
            }
            confirm log.value() == "";
            confirm owned.use() == "A";
        }
        confirm log.value() == "A";
    "#);
}

// 9, 11: an externally owned message whose owner closes before the receiver takes it, then takes it into an
// outer scope.
#[test]
fn a_borrowed_message_outlives_an_owner_that_closes_first() {
    check(r#"
        {
            let channel is Channel<Res> = Channel(capacity: 1);
            { let owned = Res("E", log); await channel.sending().send(owned); }
            confirm log.value() == "";
            let outer = if true { let value = await channel.receiving().item(); value; } else { Res("x", log); };
            confirm outer.use() == "E";
            confirm log.value() == "";
        }
        confirm log.value() == "E";
    "#);
}

// 10, 12: graphs, aliases and repeated references are destroyed once, newest first.
#[test]
fn graphs_and_repeated_references_are_destroyed_once() {
    check(r#"
        {
            let channel is Channel<Res> = Channel(capacity: 2);
            let shared = Res("S", log);
            await channel.sending().send(shared);
            await channel.sending().send(shared);
            { let first = await channel.receiving().item(); let second = await channel.receiving().item(); confirm first.use() + second.use() == "SS"; }
            confirm log.value() == "";
        }
        confirm log.value() == "S";
        log.replace("");
        {
            let pairs is Channel<Pair> = Channel(capacity: 1);
            await Work().pairs(pairs, Res("P", log), Res("Q", log));
            confirm log.value() == "";
            { let pair = await pairs.receiving().item(); confirm pair.names() == "PQ"; }
            confirm log.value() == "QP";
        }
        confirm log.value() == "QP";
    "#);
}

// Unread pinned messages leave their owner alone when the channel's last capability goes.
#[test]
fn unread_pins_go_with_the_channel() {
    check(r#"
        {
            let owned = Res("U", log);
            {
                let channel is Channel<Res> = Channel(capacity: 1);
                await channel.sending().send(owned);
            }
            confirm log.value() == "";
            confirm owned.use() == "U";
        }
        confirm log.value() == "U";
    "#);
}
