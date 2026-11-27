//! #587: action completion releases fresh inputs, before publishing its cache.
//! Assembly, JIT and assertions stay in-process; no compiler or child process.

fn program(action: &str, result: &str, destroy: &str, destroy_async: bool, main: &str) -> Vec<u8> {
    let source = format!(r#"
sparrow 18
module "future-inputs" "1.0.0"
type t0 int
type t1 str
type t2 bool
type t3 region
type t4 object obj0
type t5 object obj1
type t6 object obj2
type t7 future t1
type t8 future t6
type t9 object obj3
type t10 future t9
type t11 failure
type t12 future t8
object obj0 "Atomic<string>" {{}}
object obj1 "Closed" {{}}
object obj2 "Res" {{ field "name" t1 field "log" t4 field "gate" t9 destroy f3 }}
object obj3 "Event" {{}}
function f0 "Atomic.new" intrinsic "okkama.atomic.Atomic.new_value" [obj0] [] {{
    registers [t1] params [r0] result t4
}}
function f1 "Atomic.value" intrinsic "okkama.atomic.Atomic.value" [obj1] [] {{
    registers [t4] params [r0] result t1
}}
function f2 "Atomic.replace" intrinsic "okkama.atomic.Atomic.replace" [obj1] [] {{
    registers [t4, t1] params [r0, r1] result void
}}
function f3 "Res.destroy" {async_mark} {{
    registers [t6, t1, t4, t1, t2, t9, t10, t9]
    params [r0] result void
{destroy}
}}
function f4 "Res.action" async {{
    registers [t6, t6, t9, t3, t1, t1, t10, t9, t6]
    params [r0, r1, r2] result {result}
{action}
}}
function f5 "test.is" intrinsic "okkama.test.is" [] [] {{
    registers [t1, t1] params [r0, r1] result void
}}
function f6 "Event.new" intrinsic "okkama.event.Event.new_ready" [obj3] [] {{
    registers [t2] params [r0] result t9
}}
function f7 "Event.set" intrinsic "okkama.event.Event.set" [] [] {{
    registers [t9] params [r0] result void
}}
function f8 "Event.wait" async intrinsic "okkama.event.Event.wait" [] [] {{
    registers [t9] params [r0] result t9
}}
function f9 "check" async {{
    registers [t1, t4, t3, t3, t1, t6, t6, t7, t8, t1, t6, t2, t9, t11, t11, t12, t8]
    params [] result void
{main}
}}
function f10 "wrap" async {{
    registers [t8] params [r0] result t8
    0: ret r0
}}
function f11 "unwrap" async {{
    registers [t12, t8, t6] params [r0] result t6
    0: await r1, r0
    1: await r2, r1
    2: ret r2
}}
function f12 "invalid wait" async intrinsic "okkama.event.Event.wait" [] [] {{
    registers [t6] params [r0] result t6
}}
tests {{ test "Future inputs" f9 }}
"#, async_mark = if destroy_async { "async" } else { "" });
    let program = okkama_sparrow::assemble(&source).unwrap_or_else(|e| panic!("{e}\n{source}"));
    okkama_sparrow::encode(&program).unwrap()
}

const DESTROY: &str = r#"
    0: obj.get r1, r0, 0
    1: obj.get r2, r0, 1
    2: call r3, f1, [r2]
    3: str.concat r3, r3, r1
    4: call _, f2, [r2, r3]
    5: ret _
"#;

const READ: &str = r#"
    0: region.enter r3
    1: region.adopt r3, r0
    2: region.adopt r3, r1
    3: obj.get r4, r0, 0
    4: obj.get r5, r1, 0
    5: str.concat r4, r4, r5
    6: region.leave r3, _
    7: ret r4
"#;

const WAIT_DESTROY: &str = r#"
    0: obj.get r1, r0, 0
    1: obj.get r2, r0, 1
    2: call r3, f1, [r2]
    3: str.concat r3, r3, r1
    4: call _, f2, [r2, r3]
    5: obj.get r5, r0, 2
    6: call r6, f8, [r5]
    7: await r7, r6
    8: ret _
"#;

const SETUP: &str = r#"
    const r0, str ""
    call r1, f0, [r0]
    region.enter r2
    region.enter r3
    const r11, bool true
    call r12, f6, [r11]
    const r4, str "R"
    obj.new r5, obj2, [r4, r1, r12]
    region.track r3, r5
    const r4, str "A"
    obj.new r6, obj2, [r4, r1, r12]
    region.track r3, r6
"#;

/// Assign PCs after composing snippets, keeping handler ranges explicit.
fn numbered(text: &str) -> String {
    let mut pc = 0;
    let lines: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
    let mut output: String = lines.iter().filter(|line| line.trim_start().starts_with("handler "))
        .map(|line| format!("{line}\n")).collect();
    output.extend(lines.iter().filter(|line| !line.trim_start().starts_with("handler ")).map(|line| {
            let result = format!("    {pc}: {}\n", line.trim());
            pc += 1;
            result
    }));
    output
}

fn check(action: &str, result: &str, destroy: &str, destroy_async: bool, body: &str) {
    let main = numbered(&format!("{SETUP}\n{body}"));
    let bytes = program(action, result, destroy, destroy_async, &main);
    assert!(sparrow_core::run_tests(bytes, Box::new(std::io::stdout())).unwrap());
}

#[cfg_attr(not(feature = "jit"), ignore = "#1122: the interpreter runs no async body yet")]
#[test]
fn future_inputs_complete_before_repeated_await_and_escape() {
    check(READ, "t1", DESTROY, false, r#"
        call r7, f4, [r5, r6, r12]
        region.adopt r3, r7
        await r9, r7
        const r4, str "RA"
        call _, f5, [r9, r4]
        call r9, f1, [r1]
        const r4, str "AR"
        call _, f5, [r9, r4]
        region.leave r3, r7
        region.adopt r2, r7
        await r9, r7
        const r4, str "RA"
        call _, f5, [r9, r4]
        region.leave r2, _
        call r9, f1, [r1]
        const r4, str "AR"
        call _, f5, [r9, r4]
        ret _
    "#);
}

#[cfg_attr(not(feature = "jit"), ignore = "#1122: the interpreter runs no async body yet")]
#[test]
fn future_inputs_creation_order_overrides_parameter_order_and_aliases() {
    for (arguments, value, early, final_log) in [
        ("r6, r5", "AR", "AR", "AR"),
        ("r5, r5", "RR", "R", "RA"),
    ] {
        check(READ, "t1", DESTROY, false, &format!(r#"
            call r7, f4, [{arguments}, r12]
            region.adopt r3, r7
            await r9, r7
            const r4, str "{value}"
            call _, f5, [r9, r4]
            call r9, f1, [r1]
            const r4, str "{early}"
            call _, f5, [r9, r4]
            region.leave r3, _
            call r9, f1, [r1]
            const r4, str "{final_log}"
            call _, f5, [r9, r4]
            region.leave r2, _
            ret _
        "#));
    }
}

#[cfg_attr(not(feature = "jit"), ignore = "#1122: the interpreter runs no async body yet")]
#[test]
fn future_inputs_never_started_and_cancelled_keep_owner_boundary() {
    for cancel in ["", "fut cancel, _, r7", "handler 15 16 @17 r13\nfut cancel, _, r7\nawait r9, r7\npanic r0"] {
        check(READ, "t1", DESTROY, false, &format!(r#"
            call r7, f4, [r5, r6, r12]
            region.adopt r3, r7
            {cancel}
            call r9, f1, [r1]
            call _, f5, [r9, r0]
            region.leave r3, _
            call r9, f1, [r1]
            const r4, str "AR"
            call _, f5, [r9, r4]
            region.leave r2, _
            ret _
        "#));
    }
}

#[cfg_attr(not(feature = "jit"), ignore = "#1122: the interpreter runs no async body yet")]
#[test]
fn future_inputs_borrowed_inputs_stay_with_caller() {
    check(READ, "t1", DESTROY, false, r#"
        region.adopt r2, r5
        region.adopt r2, r6
        call r7, f4, [r5, r6, r12]
        region.adopt r3, r7
        await r9, r7
        region.leave r3, _
        call r9, f1, [r1]
        call _, f5, [r9, r0]
        region.leave r2, _
        call r9, f1, [r1]
        const r4, str "AR"
        call _, f5, [r9, r4]
        ret _
    "#);
}

#[cfg_attr(not(feature = "jit"), ignore = "#1122: the interpreter runs no async body yet")]
#[test]
fn future_inputs_partial_outcome_and_duplicate_identity() {
    let identity = "    0: ret r1";
    for duplicate in [false, true] {
        let arg = if duplicate { "r5" } else { "r6" };
        let early = if duplicate { "" } else { "R" };
        let final_log = if duplicate { "AR" } else { "RA" };
        check(identity, "t6", DESTROY, false, &format!(r#"
            call r8, f4, [r5, {arg}, r12]
            region.adopt r3, r8
            await r10, r8
            call r9, f1, [r1]
            const r4, str "{early}"
            call _, f5, [r9, r4]
            region.leave r3, _
            call r9, f1, [r1]
            const r4, str "{final_log}"
            call _, f5, [r9, r4]
            region.leave r2, _
            ret _
        "#));
    }
}

#[cfg_attr(not(feature = "jit"), ignore = "#1122: the interpreter runs no async body yet")]
#[test]
fn future_inputs_outcome_retains_nested_cache_owners() {
    check("0: ret r1", "t6", DESTROY, false, r#"
        call r8, f4, [r5, r6, r12]
        call r15, f10, [r8]
        call r16, f11, [r15]
        region.adopt r3, r16
        await r10, r16
        await r10, r16
        call r9, f1, [r1]
        const r4, str "R"
        call _, f5, [r9, r4]
        obj.get r9, r10, 0
        const r4, str "A"
        call _, f5, [r9, r4]
        region.leave r3, _
        call r9, f1, [r1]
        const r4, str "RA"
        call _, f5, [r9, r4]
        region.leave r2, _
        ret _
    "#);
}

#[cfg_attr(not(feature = "jit"), ignore = "#1122: the interpreter runs no async body yet")]
#[test]
fn future_inputs_suspended_action_and_async_destroy() {
    let action = r#"
        0: call r6, f8, [r2]
        1: await r7, r6
        2: obj.get r4, r0, 0
        3: obj.get r5, r1, 0
        4: str.concat r4, r4, r5
        5: ret r4
    "#;
    for async_destroy in [false, true] {
        check(action, "t1", DESTROY, async_destroy, r#"
            const r11, bool false
            call r12, f6, [r11]
            call r7, f4, [r5, r6, r12]
            region.adopt r3, r7
            fut.spawn r7, r7
            call r9, f1, [r1]
            call _, f5, [r9, r0]
            call _, f7, [r12]
            await r9, r7
            call r9, f1, [r1]
            const r4, str "AR"
            call _, f5, [r9, r4]
            region.leave r3, _
            region.leave r2, _
            ret _
        "#);
    }
}

#[cfg_attr(not(feature = "jit"), ignore = "#1122: the interpreter runs no async body yet")]
#[test]
fn future_inputs_cleanup_suspends_before_publication() {
    let body = r#"
        call r7, f4, [r5, r6, r12]
        region.adopt r3, r7
        fut.spawn r7, r7
        call r9, f1, [r1]
        const r4, str "A"
        call _, f5, [r9, r4]
        call _, f7, [r12]
        await r9, r7
        const r4, str "RA"
        call _, f5, [r9, r4]
        call r9, f1, [r1]
        const r4, str "AR"
        call _, f5, [r9, r4]
        region.leave r3, _
        region.leave r2, _
        ret _
    "#;
    let setup = SETUP.replace("bool true", "bool false");
    let bytes = program(READ, "t1", WAIT_DESTROY, true, &numbered(&format!("{setup}\n{body}")));
    assert!(sparrow_core::run_tests(bytes, Box::new(std::io::stdout())).unwrap());
}

#[derive(Clone)]
struct Report(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Report {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

#[cfg_attr(not(feature = "jit"), ignore = "#1122: the interpreter runs no async body yet")]
#[test]
fn future_inputs_driver_keeps_inputs_until_its_owner_closes() {
    // Owner's decision on #587: a driver is not an async body. Its fresh receiver stays in its INPUTS after it
    // settles (here: failed on a malformed capability) and closes with the Future's owner, whose async destroy
    // then waits on the event.
    let body = r#"
        handler 19 20 @21 r13
        call r8, f12, [r5]
        region.adopt r3, r8
        fut.spawn r8, r8
        call r9, f1, [r1]
        const r4, str ""
        call _, f5, [r9, r4]
        call _, f7, [r12]
        await r10, r8
        panic r0
        call r9, f1, [r1]
        call _, f5, [r9, r4]
        region.settle r3, _
        call r9, f1, [r1]
        const r4, str "AR"
        call _, f5, [r9, r4]
        region.settle r2, _
        raise r13
    "#;
    let setup = SETUP.replace("bool true", "bool false");
    let bytes = program(READ, "t1", WAIT_DESTROY, true, &numbered(&format!("{setup}\n{body}")));
    let report = Report(std::sync::Arc::default());
    assert!(!sparrow_core::run_tests(bytes, Box::new(report.clone())).unwrap());
    let report = String::from_utf8(report.0.lock().unwrap().clone()).unwrap();
    assert!(!report.contains("expected"), "{report}");
    assert_eq!(report.matches("RuntimeError: Invalid async event").count(), 1, "{report}");
    assert!(!report.contains("no task can make progress"), "{report}");
}

#[cfg_attr(not(feature = "jit"), ignore = "#1122: the interpreter runs no async body yet")]
#[test]
fn future_inputs_destroy_failures_are_cached_and_secondary() {
    let destroy = DESTROY.replace("5: ret _", "5: panic r1");
    for (action, result, future, value, log, body_fails) in [
        (READ, "t1", "r7", "r9", "AR", false),
        ("0: const r4, str \"body\"\n1: panic r4", "t1", "r7", "r9", "AR", true),
        ("0: ret r1", "t6", "r8", "r10", "RA", false),
    ] {
        for destroy_async in [false, true] {
            // Both awaits fail with the same cached carrier; cleanup is already
            // complete at the first handler, and owner exit cannot repeat it.
            let main = numbered(&format!(r#"
                {SETUP}
                handler 14 15 @16 r13
                handler 19 20 @21 r14
                call {future}, f4, [r5, r6, r12]
                region.adopt r3, {future}
                await {value}, {future}
                panic r0
                call r9, f1, [r1]
                const r4, str "{log}"
                call _, f5, [r9, r4]
                await {value}, {future}
                panic r0
                region.leave r3, _
                call r9, f1, [r1]
                call _, f5, [r9, r4]
                region.leave r2, _
                raise r13
            "#));
            let bytes = program(action, result, &destroy, destroy_async, &main);
            let report = Report(std::sync::Arc::default());
            assert!(!sparrow_core::run_tests(bytes, Box::new(report.clone())).unwrap());
            let report = String::from_utf8(report.0.lock().unwrap().clone()).unwrap();
            assert!(!report.contains("expected"), "{report}");
            assert_eq!(report.matches("Panic: A").count(), 1, "{report}");
            assert_eq!(report.matches("Panic: R").count(), 1, "{report}");
            let order_matches = if log == "AR" {
                report.find("Panic: A") < report.find("Panic: R")
            } else {
                report.find("Panic: R") < report.find("Panic: A")
            };
            assert!(order_matches, "{report}");
            assert_eq!(report.matches("Secondary cleanup failure:").count(), if body_fails { 2 } else { 1 }, "{report}");
            assert_eq!(report.matches("Panic: body").count(), usize::from(body_fails), "{report}");
            if body_fails {
                assert!(report.find("Panic: body") < report.find("Panic: A"), "{report}");
            }
        }
    }
}
