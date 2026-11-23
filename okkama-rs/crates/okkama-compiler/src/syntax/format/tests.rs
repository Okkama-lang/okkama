use super::*;

fn fmt(text: &str) -> String {
    let source = SourceFile::new("t.ok", text).unwrap();
    let once = format(&source).unwrap_or_else(|error| panic!("{error:?}"));
    let again = format(&SourceFile::new("t.ok", once.clone()).unwrap()).unwrap();
    assert_eq!(once, again, "formatting is not idempotent");
    once
}

#[test]
fn compressed_code_is_expanded_one_member_and_statement_per_line() {
    let text = "module app v1.0.0;\nimport \"io/stdout\" take Stdout;\nobject A{has x,y is int;has z is @int;A(x is int){self.x=x;y=x*2+1;z=[];}size() is int{z.size();}}\nmain() is int{let a=A(1);return a.size();}\n";
    assert_eq!(
        fmt(text),
        "module app v1.0.0;

import \"io/stdout\" take Stdout;

object A {
    has x, y is int;
    has z is @int;

    A(x is int) {
        self.x = x;
        y = x * 2 + 1;
        z = [];
    }

    size() is int {
        z.size();
    }
}

main() is int {
    let a = A(1);
    return a.size();
}
"
    );
}

#[test]
fn else_if_in_if_expressions_and_postfix_exits_keep_their_form() {
    let text = "module app v1.0.0;\nmain() is int {\nlet s = if a { 1 } else if b { 2 } else { 3 };\nreturn s if s > 1;\nthrow Bad.x if s == 0;\n0;\n}\n";
    assert_eq!(
        fmt(text),
        "module app v1.0.0;

main() is int {
    let s = if a {
        1
    }
    else if b {
        2
    }
    else {
        3
    };
    return s if s > 1;
    throw Bad.x if s == 0;
    0;
}
"
    );
}

#[test]
fn else_and_catch_start_their_own_line() {
    let text = "module app v1.0.0;\nmain() is int{if a{x();}else if b{y();}else{z();}let v=try{f();}catch e is Error{0;};return v;}\n";
    assert_eq!(
        fmt(text),
        "module app v1.0.0;

main() is int {
    if a {
        x();
    }
    else if b {
        y();
    }
    else {
        z();
    }
    let v = try {
        f();
    }
    catch e is Error {
        0;
    };
    return v;
}
"
    );
}

#[test]
fn comments_stay_with_their_code() {
    let text = "// header\n\nmodule app v1.0.0;\n// about A\nobject A {\n    // the value\n    has x is int; // trailing\n    /* block */\n}\nmain() is int {\n    return 0; // done\n    // last\n}\n";
    assert_eq!(
        fmt(text),
        "// header

module app v1.0.0;

// about A
object A {
    // the value
    has x is int; // trailing
    /* block */
}

main() is int {
    return 0; // done
    // last
}
"
    );
}

#[test]
fn long_lists_break_one_item_per_line_and_short_ones_stay_inline() {
    let text = "module app v1.0.0;\nmain() is int { let x = compute(first_argument_value, second_argument_value, third_argument_value, fourth_value_here); let y = f(a, b); return 0; }\n";
    assert_eq!(
        fmt(text),
        "module app v1.0.0;

main() is int {
    let x = compute(
        first_argument_value,
        second_argument_value,
        third_argument_value,
        fourth_value_here
    );
    let y = f(a, b);
    return 0;
}
"
    );
}

#[test]
fn author_line_breaks_and_trailing_commas_keep_lists_open() {
    let text = "module app v1.0.0;\nmain() is int {\n    let x = f(a,\n        b);\n    let y = g(c, d,);\n    return 0;\n}\n";
    assert_eq!(
        fmt(text),
        "module app v1.0.0;

main() is int {
    let x = f(
        a,
        b
    );
    let y = g(
        c,
        d,
    );
    return 0;
}
"
    );
}

#[test]
fn enum_cases_format_like_any_other_list() {
    // #594: a trailing comma or the author's line breaks keep the cases one per
    // line; a short enum stays inline; a payload keeps its own trailing comma.
    let text = "module app v1.0.0;\nenum State = (open, closed);\nenum Effect = (charge(cents is int), receipt(text is string),);\nenum Note = (\n    seen(word is string,),\n    stop\n);\n";
    assert_eq!(
        fmt(text),
        "module app v1.0.0;

enum State = (open, closed);

enum Effect = (
    charge(cents is int),
    receipt(text is string),
);

enum Note = (
    seen(
        word is string,
    ),
    stop
);
"
    );
}

#[test]
fn closures_hug_the_call_and_logical_chains_break_before_operators() {
    let text = "module app v1.0.0;\nmain() is int { let all = xs.map() -> x { x + 1; }; confirm first_condition_value && second_condition_value && third_condition_value_that_is_long && fourth else throw Failure(); return 0; }\n";
    assert_eq!(
        fmt(text),
        "module app v1.0.0;

main() is int {
    let all = xs.map() -> x {
        x + 1;
    };
    confirm first_condition_value
        && second_condition_value
        && third_condition_value_that_is_long
        && fourth
        else throw Failure();
    return 0;
}
"
    );
}

#[test]
fn glued_generic_close_and_assignment_split_safely() {
    let text =
        "module app v1.0.0;\nmain() is int { let a is Atomic<int>=Atomic(0); return a.value(); }\n";
    assert!(fmt(text).contains("let a is Atomic<int> = Atomic(0);"));
}

#[test]
fn match_cases_and_hidden_actions_are_laid_out() {
    let text = "module app v1.0.0;\nobject A { name() is string { match kind { case 1, 2 { \"a\"; } else { \"b\"; } } }\nhidden helper() is int { 1; } }\n";
    assert_eq!(
        fmt(text),
        "module app v1.0.0;

object A {
    name() is string {
        match kind {
            case 1, 2 {
                \"a\";
            }
            else {
                \"b\";
            }
        }
    }

    hidden helper() is int {
        1;
    }
}
"
    );
}

#[test]
fn tests_among_hidden_actions_keep_their_place() {
    // #277, #365: members print where they were written, with their comments.
    let text = "module app v1.0.0;\nobject A {\nA();\nhidden helper() is int { 1; }\n// checks\ntest \"t\" { test.ok(true); }\nasync test \"u\" { test.ok(true); }\nlast_one() is int { 2; }\n}\n";
    let formatted = "module app v1.0.0;

object A {
    A();

    hidden helper() is int {
        1;
    }

    // checks
    test \"t\" {
        test.ok(true);
    }

    async test \"u\" {
        test.ok(true);
    }

    last_one() is int {
        2;
    }
}
";
    assert_eq!(fmt(text), formatted);
    assert_eq!(fmt(formatted), formatted);
}

#[test]
fn map_values_that_start_with_a_parenthesis_keep_it() {
    // #263: the parentheses of a map value survive formatting, and so does
    // the entry; `[LIMIT:(4)]` gains the space every entry has.
    let text = "module app v1.0.0;\nmain() is int {\nlet d = [\"a\": 1, \"b\": (2 + 3) * 2];\nlet c = [\"x\": (-1)];\nlet g = [LIMIT:(4)];\n0;\n}\n";
    assert_eq!(
        fmt(text),
        "module app v1.0.0;

main() is int {
    let d = [\"a\": 1, \"b\": (2 + 3) * 2];
    let c = [\"x\": (-1)];
    let g = [LIMIT: (4)];
    0;
}
"
    );
}

#[test]
fn invalid_source_is_a_syntax_error() {
    let source =
        SourceFile::new("t.ok", "module app v1.0.0;\nmain() is int { let = 1; }\n").unwrap();
    assert!(matches!(format(&source), Err(FormatError::Syntax(_))));
}

/// #921: a compact `if` keeps its spelling, nested and grouped ones included, and a branch's grouped fallback
/// keeps its parentheses; the block form is unchanged.
#[test]
fn compact_if_keeps_its_form() {
    let text = "module app v1.0.0;\nmain() is int {\nlet x = if ready:1 else:0;\nlet y = if a: if b: 1 else: 2 else: 3;\nlet z = if a: (o else 10) else: (if b: 1 else: 2);\nlet w = if c { 1; } else { 2; };\nif ready: 0 else: 1;\n}\n";
    assert_eq!(
        fmt(text),
        "module app v1.0.0;

main() is int {
    let x = if ready: 1 else: 0;
    let y = if a: if b: 1 else: 2 else: 3;
    let z = if a: (o else 10) else: (if b: 1 else: 2);
    let w = if c {
        1;
    }
    else {
        2;
    };
    if ready: 0 else: 1;
}
"
    );
}

/// A version written with a space inside (`v1.0 .0`, the same version as v1.0.0) keeps its
/// spelling: `v1.0.0` would lex `0.0` as one float and change the tokens (#980). A version
/// written as one word stays canonical, and both forms are stable under a second pass.
#[test]
fn a_module_version_keeps_its_tokens() {
    assert_eq!(fmt("module x v1.0 .0;\nmain() is int { 0; }\n"), "module x v1.0 .0;\n\nmain() is int {\n    0;\n}\n");
    assert_eq!(fmt("module x v1.0   .0;\n"), "module x v1.0 .0;\n");
    assert_eq!(fmt("module x v1.2.3;\n"), "module x v1.2.3;\n");
    let once = fmt("module x v1.0 .0;\nmain() is int { 0; }\n");
    assert_eq!(fmt(&once), once);
}

/// Nested calls: each level measured its arguments again inside the measurement of its parent,
/// so the time doubled per level (depth 24 took 1.9 s in a 0.6.3 release build). The one-line
/// width of each list item is measured once now (#979). The bound is generous for a debug build.
#[test]
fn deeply_nested_calls_format_in_linear_time() {
    let mut call = String::from("x");
    for _ in 0..26 {
        call = format!("f(a, {call})");
    }
    let started = std::time::Instant::now();
    let text = fmt(&format!("module app v1.0.0;\n\nmain() is int {{\n    let v = {call};\n    0;\n}}\n"));
    let elapsed = started.elapsed();
    assert!(text.ends_with("    0;\n}\n"), "{text}");
    assert!(elapsed < std::time::Duration::from_secs(10), "formatting took {elapsed:?}");
}

/// #988: a comment inside an empty role, cap, object or fake body stays inside
/// it; a truly empty body is still `{}`.
#[test]
fn comments_in_empty_bodies_stay_inside() {
    let source = "module m v1.0.0;\nrole Marker {\n    // nothing yet\n}\nrole Empty {}\nrole Counter {\n    count() is int;\n    fake {\n        // no state\n    }\n}\nobject Thing {\n    Thing();\n    cap Extra {\n        // to be filled\n    }\n}\nobject Bare {\n    // only a note\n}\nmain() is int {\n    0;\n}\n";
    let out = fmt(source);
    assert!(out.contains("role Marker {\n    // nothing yet\n}"), "{out}");
    assert!(out.contains("role Empty {}"), "{out}");
    assert!(out.contains("fake {\n        // no state\n    }"), "{out}");
    assert!(out.contains("cap Extra {\n        // to be filled\n    }"), "{out}");
    assert!(out.contains("object Bare {\n    // only a note\n}"), "{out}");
    assert_eq!(fmt(&out), out, "idempotent");
}

/// #1006 (owner's decision): a block comment between a declaration's parameter list (or result)
/// and its `{` or `;` stays on the header line where the author wrote it, and the list stays on
/// one line: its end is the real `)`, so it never takes the comment in.
#[test]
fn block_comments_in_headers_stay_on_the_header_line() {
    let text = "module app v1.0.0;\nobject A /* object */ {\nA(x is int) /* ctor */ {\nx;\n}\nA(:x) /* short */;\ngo(a is int) is int /* result */ {\na;\n}\nempty() /* none */ is void {}\ntest \"t\" /* test */ {\ntest.ok(true);\n}\ncap Extra /* cap */ {\nsize is int /* field */ {\n1;\n}\n}\n}\nrole R /* role */ {\nsig(a is int) is int /* sig */;\n}\nnative host(a is int, b is string) is int /* host */;\nmain() is int {\nlet f = (a is int) is int /* closure */ { a; };\n0;\n}\n";
    assert_eq!(
        fmt(text),
        "module app v1.0.0;

object A /* object */ {
    A(x is int) /* ctor */ {
        x;
    }

    A(:x) /* short */;

    go(a is int) is int /* result */ {
        a;
    }

    empty() /* none */ is void {}

    test \"t\" /* test */ {
        test.ok(true);
    }

    cap Extra /* cap */ {
        size is int /* field */ {
            1;
        }
    }
}

role R /* role */ {
    sig(a is int) is int /* sig */;
}

native host(a is int, b is string) is int /* host */;

main() is int {
    let f = (a is int) is int /* closure */ {
        a;
    };
    0;
}
"
    );
}

/// #1006 (owner's decision): a `//` comment between a header and its `{` cannot stay before the
/// `{` on one line, so it opens the body, indented, empty bodies included; a bodiless header
/// (role signature, native action) prints it after its `;`.
#[test]
fn line_comments_in_headers_open_the_body() {
    let text = "module app v1.0.0;\nobject A {\nA(x is int) // ctor\n{\nx;\n}\ngo(a is int) is int // why\n{\na;\n}\nempty() is void // nothing\n{}\ntest \"t\" // test\n{\ntest.ok(true);\n}\nboth(a is int) /* kept */ is int // moved\n/* after */ {\na;\n}\n}\nrole R {\nsig(a is int) is int // sig\n;\nother() is int;\n}\nnative host(a is int) is int // host\n;\nmain() is int {\n0;\n}\n";
    assert_eq!(
        fmt(text),
        "module app v1.0.0;

object A {
    A(x is int) {
        // ctor
        x;
    }

    go(a is int) is int {
        // why
        a;
    }

    empty() is void {
        // nothing
    }

    test \"t\" {
        // test
        test.ok(true);
    }

    both(a is int) /* kept */ is int {
        // moved
        /* after */
        a;
    }
}

role R {
    sig(a is int) is int; // sig
    other() is int;
}

native host(a is int) is int; // host

main() is int {
    0;
}
"
    );
}

/// Every line of `text` fits into [`WIDTH`].
fn assert_fits(text: &str) {
    for line in text.lines() {
        assert!(line.chars().count() <= WIDTH, "{} columns: {line}", line.chars().count());
    }
}

/// #1006: a line that a list, an operator chain or a call chain could break never passes the
/// width. These shapes reached 101–131 columns: the list measured itself but not the result,
/// the `{`, the `;` or the call after it, and operator and call chains broke only where the
/// author had. A body under a header that broke stays one level in from the statement, and a
/// continuation inside one list item does not shift the next item.
#[test]
fn lines_that_can_break_fit_the_width() {
    let text = "module app v1.0.0;\n\nobject Store does Comparing<Store>, Adding<Duration, Store>, Subtracting<Duration, Store>, Negating<Store> {\n    async transaction(options is TransactionOptions, mode is SqliteTransactionMode) is SqliteTransaction {\n        let raw = await TcpConnecting(address: IpAddress(\"127.0.0.1\"), port: server::Local::port).connection();\n        let line = first.string() + \" \" + second.string() + \" \" + third.string() + \" \" + fourth.string();\n        if chosen == \"h2\" || (chosen == \"\" && !policy.protocols().membership(HttpVersion.http_1_1)) {\n            return 1;\n        }\n        if PackedIpPrefix(origin: origin, prefix: \"00000000000000000000000000000000\", bits: 128).matches() {\n            return 65;\n        }\n        let reply = Reply(waiting: TlsServerHandshake(origin: connection, policy: tls[0], protocols: offered).connection(), limit: policy.timeouts().headers_timeout(), clock: clock);\n        match code {\n            case 33, 35, 36, 37, 38, 39, 42, 43, 45, 47, 61, 63, 94, 95, 96, 123, 124, 125, 126, 127 {\n                0;\n            }\n        }\n    }\n}\n";
    let formatted = fmt(text);
    assert_eq!(
        formatted,
        "module app v1.0.0;

object Store does Comparing<Store>, Adding<Duration, Store>, Subtracting<Duration, Store>,
    Negating<Store> {
    async transaction(
        options is TransactionOptions,
        mode is SqliteTransactionMode
    ) is SqliteTransaction {
        let raw = await TcpConnecting(address: IpAddress(\"127.0.0.1\"), port: server::Local::port)
            .connection();
        let line = first.string() + \" \" + second.string() + \" \" + third.string() + \" \" +
            fourth.string();
        if chosen == \"h2\"
            || (chosen == \"\" && !policy.protocols().membership(HttpVersion.http_1_1)) {
            return 1;
        }
        if PackedIpPrefix(origin: origin, prefix: \"00000000000000000000000000000000\", bits: 128)
            .matches() {
            return 65;
        }
        let reply = Reply(
            waiting: TlsServerHandshake(origin: connection, policy: tls[0], protocols: offered)
                .connection(),
            limit: policy.timeouts().headers_timeout(),
            clock: clock
        );
        match code {
            case 33, 35, 36, 37, 38, 39, 42, 43, 45, 47, 61, 63, 94, 95, 96, 123, 124, 125, 126,
                127 {
                0;
            }
        }
    }
}
"
    );
    assert_fits(&formatted);
}

/// #1006: a call on a plain receiver keeps it (`out.writeln(` does not break before `.writeln`),
/// and a native or a role signature breaks its parameters when its result passes the width.
#[test]
fn plain_receivers_stay_and_bodiless_headers_break_their_parameters() {
    let text = "module app v1.0.0;\n\nrole Opening {\n    upgrade_tls_server(versions is int, protocols is @string, certificates is @@byte, key is @byte) is TlsState;\n}\n\nnative tcp_bound(address is @byte, zone is string, reuse is bool, dual_stack is bool) is TcpListenerState;\n\nmain() is int {\n    out.writeln(total.string() + \" \" + ready.string() + \" \" + negated.string() + \" \" + pair.sum().string() + \" \" + nested.string());\n    let result = connection.transaction(options: TransactionOptions(), mode: mmmmmmmmmmmmmmmmmmmmmm);\n    0;\n}\n";
    let formatted = fmt(text);
    assert_eq!(
        formatted,
        "module app v1.0.0;

role Opening {
    upgrade_tls_server(
        versions is int,
        protocols is @string,
        certificates is @@byte,
        key is @byte
    ) is TlsState;
}

native tcp_bound(
    address is @byte,
    zone is string,
    reuse is bool,
    dual_stack is bool
) is TcpListenerState;

main() is int {
    out.writeln(total.string() + \" \" + ready.string() + \" \" + negated.string() + \" \" +
        pair.sum().string() + \" \" + nested.string());
    let result = connection.transaction(
        options: TransactionOptions(),
        mode: mmmmmmmmmmmmmmmmmmmmmm
    );
    0;
}
"
    );
    assert_fits(&formatted);
}

/// #1006: a break that does not shorten the overflow is not made. Breaking `x(` would move
/// the string from column 6 to column 8 and gain nothing, so the line stays as written.
#[test]
fn a_break_that_does_not_shorten_the_line_is_not_made() {
    let long = format!("    x(\"{}\");", "a".repeat(91));
    let text = format!("module app v1.0.0;\n\nmain() is int {{\n{long}\n    0;\n}}\n");
    assert_eq!(long.chars().count(), WIDTH + 1);
    assert_eq!(fmt(&text), text);
}
