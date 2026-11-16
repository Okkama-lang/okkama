//! #601 P1: `spawn` of an async action on Champ, compiled through OIR-2 and run in process (OIR-2 async ABI D24).
//! Conformance cases are checked in full against their case.json; the inline programs' expected output is
//! Tomato's for the same program (okkama 0.5.8, run in the cage). With `SPARROW_CORE_GC_STRESS=1` every safepoint
//! collects.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{run_case, run_source};

macro_rules! cases {
    ($($name:ident => $id:literal,)*) => {
        $(
            #[test]
            fn $name() {
                run_case($id).unwrap_or_else(|error| panic!("{}: {error}", $id));
            }
        )*
    };
}

cases! {
    p1_wake_spawn_and_cold_start => "L08.wake.spawn-and-cold-start",
    p1_failure_spawned_task_observed => "L08.failure.spawned-task-observed",
    p1_shutdown_unobserved_failure => "L08.shutdown.unobserved-failure",
}

fn expect(source: &str, stdout: &str, status: i64) {
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((run.stdout.as_str(), run.result), (stdout, Ok(status)));
}

/// A spawned task starts at once and runs before the spawner continues; nobody awaits it (Bible §20.4).
#[test]
fn p1_a_spawned_task_runs_before_the_spawner_continues() {
    expect(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "object Job { has name is string; Job(:name); ",
        "async run() is int { Stdout().writeln(name + \" runs\"); 1; } }\n",
        "async main() is int { spawn Job(name: \"a\").run(); Stdout().writeln(\"main\"); 0; }\n",
    ), "a runs\nmain\n", 0);
}

/// A sync action spawns a task that awaits a cold Future it was given (ABI D24.2 review B7): the task starts the
/// Future on its own stack, and the sync owner's close settles both duties without suspending. Here the task has
/// settled by the close; the Running row (detach from a sync owner) becomes reachable with Q1, when a task can wait
/// for something outside.
#[test]
fn p1_a_sync_owner_spawns_a_task_over_a_future_argument() {
    expect(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "object Work { Work(); async value() is int { Stdout().writeln(\"value\"); 5; } }\n",
        "object Watcher { Watcher(); async run(pending is Future<int>) is int { ",
        "let v = await pending; Stdout().writeln(\"watched \" + v.string()); v; } }\n",
        "object Kick { Kick(); start() is int { let f = Work().value(); spawn Watcher().run(f); ",
        "Stdout().writeln(\"kicked\"); 0; } }\n",
        "async main() is int { Kick().start(); Stdout().writeln(\"main\"); 0; }\n",
    ), "value\nwatched 5\nkicked\nmain\n", 0);
}

/// A spawned Future returned by a sync action leaves with both duties (ABI C-4) and is awaited by the caller.
#[test]
fn p1_a_returned_spawned_future_is_awaited_by_the_caller() {
    expect(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "object Job { has n is int; Job(:n); async run() is int { Stdout().writeln(\"job \" + n.string()); n; } }\n",
        "object Starter { Starter(); start(n is int) is Future<int> { spawn Job(n: n).run(); } }\n",
        "async main() is int { let f = Starter().start(4); Stdout().writeln(\"started\"); let v = await f; ",
        "Stdout().writeln(\"got \" + v.string()); 0; }\n",
    ), "job 4\nstarted\ngot 4\n", 0);
}

/// `defer await spawn …` spawns in the async defer entry at the scope's exit and waits for the task (ABI C-6).
#[test]
fn p1_a_deferred_await_of_a_spawned_task() {
    expect(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "object W { W(); async run() is void { Stdout().writeln(\"deferred\"); } }\n",
        "async main() is int { defer await spawn W().run(); Stdout().writeln(\"main\"); 0; }\n",
    ), "main\ndeferred\n", 0);
}

/// Spawn of a generic object's async action: the specializer reaches the spawn target.
#[test]
fn p1_spawn_of_a_generic_async_action() {
    expect(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "object Box<T> { has value is T; Box(:value); async get() is T { Stdout().writeln(\"get\"); value; } }\n",
        "async main() is int { let t = spawn Box(value: 3).get(); Stdout().writeln(\"spawned\"); await t; }\n",
    ), "get\nspawned\n", 3);
}

/// A spawned task that nobody awaits still releases what it owns (Bible §11.9): its outcome holds a resource, and
/// the owner's close releases the outcome packet into its plan (OIR-2 async ABI D24.2, cache row Done), so the
/// destroy runs exactly once, at the close of `main`'s scope. Tomato never destroys it (a leak there).
#[test]
fn p1_an_unawaited_spawned_task_releases_its_outcome_at_the_owner_close() {
    expect(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "object Res { has name is string; Res(:name); destroy { Stdout().writeln(\"destroy \" + name); } }\n",
        "object Maker { Maker(); async make() is Res { Res(name: \"made\"); } }\n",
        "async main() is int { spawn Maker().make(); Stdout().writeln(\"main\"); 0; }\n",
    ), "main\ndestroy made\n", 0);
}
