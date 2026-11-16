//! #825 on Champ, in process: a resource taken from an `await` stays with the binding that holds it. The loader's
//! evidence counts `await.take` as a producer of obligations, so the admission filters of the body's splits stay and
//! an owned call on the binding borrows it instead of taking it. Each program runs under Structural and Full
//! verification, with the output of Tomato built from the rebuild tree.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::run_source_with;
use okkama_compiler::oir2::VerifyMode;

fn expect(source: &str, stdout: &str) {
    for mode in [VerifyMode::Structural, VerifyMode::Full] {
        let run = run_source_with(source, mode).unwrap_or_else(|error| panic!("{mode:?}: {error}"));
        assert_eq!((run.stdout.as_str(), run.result), (stdout, Ok(0)), "{mode:?}");
    }
}

const PRE: &str = r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res { has name is string; Res(:name); label() is string { name; } destroy { Stdout().writeln("destroy " + name); } }
object Maker { Maker(); async make(name is string) is Res { Stdout().writeln("make " + name); Res(name: name); } }
"#;

/// The awaited resource is used through an owned call twice, and destroyed once, when its binding's scope closes.
#[test]
fn an_awaited_resource_survives_owned_calls_on_it() {
    expect(&format!("{PRE}{}", r#"object Chain {
    Chain();
    async run() is int {
        let inner = await Maker().make("inner");
        Stdout().writeln("chain got " + inner.label());
        inner.label().length();
    }
}
async main() is int {
    Stdout().writeln("length " + (await Chain().run()).string());
    0;
}
"#), "make inner\nchain got inner\ndestroy inner\nlength 5\n");
}

/// The same with a statement between the await and the call: the resource lives to the end of its scope.
#[test]
fn an_awaited_resource_lives_to_the_end_of_its_scope() {
    expect(&format!("{PRE}{}", r#"object Chain {
    Chain();
    async run() is int {
        let inner = await Maker().make("inner");
        Stdout().writeln("chain got it");
        Stdout().writeln("then " + inner.label());
        5;
    }
}
async main() is int {
    Stdout().writeln("length " + (await Chain().run()).string());
    0;
}
"#), "make inner\nchain got it\nthen inner\ndestroy inner\nlength 5\n");
}
