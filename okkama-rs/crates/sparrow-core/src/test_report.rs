//! The result of a test run, apart from how it is shown (#339): every test in
//! declaration order with its outcome, and the seed that ordered the run.
//! Execution fills it; renderers (JSON and JUnit here, the human stream in
//! `lib.rs`: `report_suite`, `human_into`) only read it.

/// How one test ended. New endings (a deadline, a crashed worker) are new
/// variants; the fields of the others stay as they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestOutcome {
    /// Every check held.
    Passed,
    /// The failed checks in order, and an uncaught failure last.
    Failed(Vec<String>),
    /// `test.skip`, with its reason ("" when none was given).
    Skipped(String),
    /// Its process worker passed the deadline, in milliseconds, and was
    /// stopped (#938).
    TimedOut(u64),
    /// Its process worker ended without a result: how it ended (#938).
    Crashed(String),
}

impl TestOutcome {
    /// Whether it fails the run: a failed check, a deadline or a crash.
    pub fn fails(&self) -> bool {
        matches!(self, Self::Failed(_) | Self::TimedOut(_) | Self::Crashed(_))
    }
}

/// What a test wrote, captured by its process worker (#938): each stream up
/// to the capture limit, and whether more was cut off.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapturedOutput {
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

/// One test of the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCase {
    /// Its name, unique in the module (`Object :: name` inside an object); the
    /// test's identity.
    pub name: String,
    /// Where it is declared among the selected tests, from 0.
    pub position: usize,
    /// When it ran among them, from 0: `position` unless a seed ordered the run.
    pub execution: usize,
    /// How the test itself ended, outside its sub-cases.
    pub outcome: TestOutcome,
    /// Its named sub-cases (`test.case`, #809) in the order they ran; empty
    /// for a test without them.
    pub cases: Vec<SubCase>,
    /// What it wrote, when a process worker ran it (#938).
    pub output: Option<CapturedOutput>,
}

/// A named sub-case of a test (#809): `Test/sub-case` is its identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubCase {
    pub name: String,
    pub outcome: TestOutcome,
}

impl TestCase {
    /// The verdict of the test with its sub-cases: failed when it or one of
    /// them failed, skipped when it skipped itself, passed otherwise.
    pub fn verdict(&self) -> TestOutcome {
        match &self.outcome {
            own if own.fails() => own.clone(),
            _ if self.cases.iter().any(|case| case.outcome.fails()) => TestOutcome::Failed(Vec::new()),
            other => other.clone(),
        }
    }

    /// The outcomes the totals count: the test's own when it has no
    /// sub-cases; otherwise each sub-case's, and the test's own only when it
    /// failed or skipped outside them, so a container is never counted twice.
    pub fn leaves(&self) -> Vec<&TestOutcome> {
        if self.cases.is_empty() {
            return vec![&self.outcome];
        }
        let mut leaves: Vec<&TestOutcome> = self.cases.iter().map(|case| &case.outcome).collect();
        if !matches!(self.outcome, TestOutcome::Passed) {
            leaves.push(&self.outcome);
        }
        leaves
    }
}

/// What `test.case(name)` does with a sub-case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubCaseStart {
    /// Run its block.
    Run,
    /// A filter leaves it out: its block does not run and it has no result.
    Leave,
    /// A sub-case of this name ran already in this test.
    Duplicate,
    /// Another sub-case, named here, is open: sub-cases do not nest.
    Nested(String),
}

/// The sub-cases of the running test, kept by the host while it runs.
#[derive(Debug, Default)]
pub struct SubCaseState {
    /// Only the sub-cases whose names contain one of these run; none: all.
    pub only: Option<Vec<String>>,
    /// The open sub-case and the test's own failures until it closes.
    open: Option<(String, Vec<String>)>,
    /// The closed sub-cases in order, with their failures and skip reason.
    done: Vec<(String, Vec<String>, Option<String>)>,
}

impl SubCaseState {
    /// A fresh state for a test; `only` as the filter selects its sub-cases.
    pub fn new(only: Option<Vec<String>>) -> Self {
        Self { only, open: None, done: Vec::new() }
    }

    /// What to do with the sub-case `name`.
    pub fn open(&self, name: &str) -> SubCaseStart {
        if let Some((outer, _)) = &self.open {
            return SubCaseStart::Nested(outer.clone());
        }
        if self.done.iter().any(|(done, _, _)| done == name) {
            return SubCaseStart::Duplicate;
        }
        match &self.only {
            Some(only) if !only.iter().any(|part| name.contains(part.as_str())) => SubCaseStart::Leave,
            _ => SubCaseStart::Run,
        }
    }

    /// The sub-case `name` runs now; `parent` keeps the test's own failures.
    pub fn started(&mut self, name: String, parent: Vec<String>) {
        self.open = Some((name, parent));
    }

    /// Whether a sub-case is open.
    pub fn running(&self) -> bool {
        self.open.is_some()
    }

    /// Closes the open sub-case with its failures (and skip reason) and gives
    /// back the test's own failures; with none open, gives `failures` back.
    pub fn finished(&mut self, failures: Vec<String>, skipped: Option<String>) -> Vec<String> {
        match self.open.take() {
            Some((name, parent)) => {
                self.done.push((name, failures, skipped));
                parent
            }
            None => failures,
        }
    }

    /// The closed sub-cases as results.
    pub fn cases(&mut self) -> Vec<SubCase> {
        std::mem::take(&mut self.done)
            .into_iter()
            .map(|(name, failures, skipped)| SubCase {
                name,
                outcome: if !failures.is_empty() {
                    TestOutcome::Failed(failures)
                } else if let Some(reason) = skipped {
                    TestOutcome::Skipped(reason)
                } else {
                    TestOutcome::Passed
                },
            })
            .collect()
    }
}

/// The whole run: the selected tests in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestReport {
    /// The seed that ordered the run; none for declaration order.
    pub seed: Option<u64>,
    pub cases: Vec<TestCase>,
}

/// What a run is asked to do: the seed that orders it (#339), and for each
/// test the filter selects only some sub-cases of, the texts one of which a
/// sub-case's name must contain (#809).
#[derive(Debug, Clone, Default)]
pub struct TestPlan {
    pub seed: Option<u64>,
    pub only: std::collections::HashMap<String, Vec<String>>,
}

/// How many tests ended each way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestTotals {
    pub tests: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

impl TestReport {
    /// Whether no test failed (skipped ones do not fail a run).
    pub fn passed(&self) -> bool {
        self.totals().failed == 0
    }

    /// The counts of the executable cases (TestCase::leaves), from the cases
    /// themselves.
    pub fn totals(&self) -> TestTotals {
        let leaves: Vec<&TestOutcome> = self.cases.iter().flat_map(TestCase::leaves).collect();
        let count = |wanted: fn(&TestOutcome) -> bool| leaves.iter().filter(|outcome| wanted(outcome)).count();
        TestTotals {
            tests: leaves.len(),
            passed: count(|outcome| matches!(outcome, TestOutcome::Passed)),
            failed: count(TestOutcome::fails),
            skipped: count(|outcome| matches!(outcome, TestOutcome::Skipped(_))),
        }
    }
}

/// How a test run is reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    Human,
    Json,
    Junit,
}

impl ReportFormat {
    /// The format named on the command line; TAP is deferred (#339).
    pub fn named(name: &str) -> Result<Self, String> {
        match name {
            "human" => Ok(Self::Human),
            "json" => Ok(Self::Json),
            "junit" => Ok(Self::Junit),
            other => Err(format!("unknown report format `{other}`: use human, json or junit")),
        }
    }
}

/// The run as one JSON document: the suite, the seed (null for declaration
/// order), the verdict, the totals and every test in declaration order with
/// its position, when it ran, its outcome and its failures or skip reason.
/// Keys are sorted, two spaces indent.
pub fn json(report: &TestReport, suite: &str) -> String {
    let totals = report.totals();
    let number = |value: usize| Json::Raw(value.to_string());
    let tests = report.cases.iter().map(|case| {
        let mut fields = Vec::new();
        if !case.cases.is_empty() {
            let cases = case
                .cases
                .iter()
                .map(|sub| {
                    let mut entry = Vec::new();
                    let (failures, outcome, reason) = json_outcome(&sub.outcome);
                    entry.extend(failures.map(|value| ("failures", value)));
                    entry.push(("id", Json::text(&format!("{}/{}", case.name, sub.name))));
                    entry.push(("name", Json::text(&sub.name)));
                    entry.push(("outcome", outcome));
                    entry.extend(reason.map(|value| ("reason", value)));
                    Json::Object(entry)
                })
                .collect();
            fields.push(("cases", Json::Array(cases)));
        }
        let (failures, outcome, reason) = json_outcome(&case.verdict());
        fields.push(("execution", number(case.execution)));
        fields.extend(failures.map(|value| ("failures", value)));
        fields.push(("name", Json::text(&case.name)));
        fields.push(("outcome", outcome));
        fields.push(("position", number(case.position)));
        fields.extend(reason.map(|value| ("reason", value)));
        if let Some(output) = &case.output {
            fields.push(("stderr", Json::text(&output.stderr)));
            fields.push(("stdout", Json::text(&output.stdout)));
            let mut cut = Vec::new();
            if output.stderr_truncated {
                cut.push(Json::text("stderr"));
            }
            if output.stdout_truncated {
                cut.push(Json::text("stdout"));
            }
            fields.push(("truncated", Json::Array(cut)));
        }
        Json::Object(fields)
    });
    let document = Json::Object(vec![
        ("passed", Json::Raw(report.passed().to_string())),
        ("report", Json::text("okkama-tests")),
        ("seed", Json::Raw(report.seed.map_or_else(|| "null".to_owned(), |seed| seed.to_string()))),
        ("suite", Json::text(suite)),
        ("tests", Json::Array(tests.collect())),
        (
            "totals",
            Json::Object(vec![
                ("failed", number(totals.failed)),
                ("passed", number(totals.passed)),
                ("skipped", number(totals.skipped)),
                ("tests", number(totals.tests)),
            ]),
        ),
        ("version", Json::Raw("1".into())),
    ]);
    let mut out = String::new();
    document.write(&mut out, 0);
    out.push('\n');
    out
}

/// The failures, outcome and reason fields of an outcome.
fn json_outcome(outcome: &TestOutcome) -> (Option<Json>, Json, Option<Json>) {
    match outcome {
        TestOutcome::Passed => (None, Json::text("passed"), None),
        TestOutcome::Failed(failures) => (
            Some(Json::Array(failures.iter().map(|failure| Json::text(failure)).collect())),
            Json::text("failed"),
            None,
        ),
        TestOutcome::Skipped(reason) => (None, Json::text("skipped"), Some(Json::text(reason))),
        TestOutcome::TimedOut(limit) => (
            Some(Json::Array(vec![Json::text(&format!("deadline of {limit} ms passed"))])),
            Json::text("timed_out"),
            None,
        ),
        TestOutcome::Crashed(how) => (Some(Json::Array(vec![Json::text(how)])), Json::text("crashed"), None),
    }
}

/// A JSON value written with two spaces of indent, keys in the order given.
enum Json {
    Raw(String),
    Array(Vec<Json>),
    Object(Vec<(&'static str, Json)>),
}

impl Json {
    fn text(value: &str) -> Self {
        Self::Raw(json_text(value))
    }

    fn write(&self, out: &mut String, depth: usize) {
        let pad = |depth: usize| "  ".repeat(depth);
        match self {
            Self::Raw(text) => out.push_str(text),
            Self::Array(items) if items.is_empty() => out.push_str("[]"),
            Self::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    out.push_str(if index == 0 { "\n" } else { ",\n" });
                    out.push_str(&pad(depth + 1));
                    item.write(out, depth + 1);
                }
                out.push('\n');
                out.push_str(&pad(depth));
                out.push(']');
            }
            Self::Object(fields) => {
                out.push('{');
                for (index, (key, value)) in fields.iter().enumerate() {
                    out.push_str(if index == 0 { "\n" } else { ",\n" });
                    out.push_str(&format!("{}\"{key}\": ", pad(depth + 1)));
                    value.write(out, depth + 1);
                }
                out.push('\n');
                out.push_str(&pad(depth));
                out.push('}');
            }
        }
    }
}

/// A JSON string (RFC 8259): quotes, backslashes and control characters escaped.
fn json_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The run as JUnit XML: one `testsuite` named suite, the seed as a property,
/// a `testcase` per test in declaration order, a `failure` with every failed
/// check, a `skipped` with the reason. A crashed worker is an `error`, and
/// counted under `errors`, not `failures` (#1011).
pub fn junit(report: &TestReport, suite: &str) -> String {
    let totals = report.totals();
    let errors = report
        .cases
        .iter()
        .flat_map(TestCase::leaves)
        .filter(|outcome| matches!(outcome, TestOutcome::Crashed(_)))
        .count();
    let failures = totals.failed - errors;
    let suite = xml(suite);
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<testsuites tests=\"{}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"{}\">\n",
        totals.tests, totals.skipped
    ));
    out.push_str(&format!(
        "  <testsuite name=\"{suite}\" tests=\"{}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"{}\">\n",
        totals.tests, totals.skipped
    ));
    if let Some(seed) = report.seed {
        out.push_str(&format!("    <properties>\n      <property name=\"seed\" value=\"{seed}\"/>\n    </properties>\n"));
    }
    for case in &report.cases {
        // Each executable case is one testcase: a sub-case as `Test/sub-case`,
        // the test itself when it has no sub-cases or failed or skipped
        // outside them (#809).
        // Captured output (#938) goes with the test's own testcase, or with its
        // first sub-case when the test has no testcase of its own.
        let own = case.cases.is_empty() || !matches!(case.outcome, TestOutcome::Passed);
        for (index, sub) in case.cases.iter().enumerate() {
            let output = if !own && index == 0 { case.output.as_ref() } else { None };
            junit_case(&mut out, &format!("{}/{}", case.name, sub.name), &sub.outcome, &suite, output);
        }
        if own {
            junit_case(&mut out, &case.name, &case.outcome, &suite, case.output.as_ref());
        }
    }
    out.push_str("  </testsuite>\n</testsuites>\n");
    out
}

/// One `testcase` of a JUnit report, with the captured output of its test
/// when given (#938).
fn junit_case(out: &mut String, name: &str, outcome: &TestOutcome, suite: &str, output: Option<&CapturedOutput>) {
    let name = xml(name);
    if matches!(outcome, TestOutcome::Passed) && output.is_none() {
        out.push_str(&format!("    <testcase name=\"{name}\" classname=\"{suite}\"/>\n"));
        return;
    }
    out.push_str(&format!("    <testcase name=\"{name}\" classname=\"{suite}\">\n"));
    match outcome {
        TestOutcome::Passed => {}
        TestOutcome::Failed(failures) => {
            let first = failures.first().and_then(|text| text.lines().next()).unwrap_or("");
            out.push_str(&format!(
                "      <failure message=\"{}\">{}</failure>\n",
                xml(first),
                xml(&failures.join("\n"))
            ));
        }
        TestOutcome::Skipped(reason) => out.push_str(&format!("      <skipped message=\"{}\"/>\n", xml(reason))),
        TestOutcome::TimedOut(limit) => {
            let message = xml(&format!("deadline of {limit} ms passed"));
            out.push_str(&format!("      <failure type=\"timeout\" message=\"{message}\">{message}</failure>\n"));
        }
        TestOutcome::Crashed(how) => {
            let message = xml(how);
            out.push_str(&format!("      <error type=\"crash\" message=\"{message}\">{message}</error>\n"));
        }
    }
    if let Some(output) = output {
        let mark = |truncated: bool| if truncated { "\n[truncated]" } else { "" };
        out.push_str(&format!("      <system-out>{}{}</system-out>\n", xml(&output.stdout), mark(output.stdout_truncated)));
        out.push_str(&format!("      <system-err>{}{}</system-err>\n", xml(&output.stderr), mark(output.stderr_truncated)));
    }
    out.push_str("    </testcase>\n");
}

/// Text as XML 1.0 character data or an attribute value: the five markup
/// characters escaped, and the control characters XML cannot hold at all
/// written as U+FFFD.
fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
    out
}

/// The largest seed: 2^53 - 1, which every JSON reader holds exactly.
pub const MAX_SEED: u64 = (1 << 53) - 1;

/// The order to run `count` tests in, as declaration positions: declaration
/// order without a seed; with one, a Fisher-Yates shuffle driven by SplitMix64
/// from it, the same on every platform. It touches no generator of the
/// program, so a property's own PCG seed means what it meant.
pub fn execution_order(count: usize, seed: Option<u64>) -> Vec<usize> {
    let mut order: Vec<usize> = (0..count).collect();
    let Some(seed) = seed else {
        return order;
    };
    let mut state = seed;
    let mut next = move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    for index in (1..count).rev() {
        let pick = (next() % (index as u64 + 1)) as usize;
        order.swap(index, pick);
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declaration_order_without_a_seed_and_a_stable_permutation_with_one() {
        assert_eq!(execution_order(5, None), vec![0, 1, 2, 3, 4]);
        let seeded = execution_order(8, Some(42));
        assert_eq!(seeded, execution_order(8, Some(42)));
        let mut sorted = seeded.clone();
        sorted.sort();
        assert_eq!(sorted, (0..8).collect::<Vec<_>>());
        assert_ne!(seeded, (0..8).collect::<Vec<_>>());
        let others: Vec<Vec<usize>> = (0..20).map(|seed| execution_order(8, Some(seed))).collect();
        assert!(others.iter().any(|order| *order != seeded));
        assert_eq!(execution_order(0, Some(7)), Vec::<usize>::new());
        assert_eq!(execution_order(1, Some(7)), vec![0]);
    }

    #[test]
    fn totals_come_from_the_cases() {
        let case = |name: &str, outcome| TestCase { name: name.into(), position: 0, execution: 0, outcome, cases: vec![], output: None };
        let report = TestReport {
            seed: None,
            cases: vec![
                case("a", TestOutcome::Passed),
                case("b", TestOutcome::Failed(vec!["expected 2, got 1".into()])),
                case("c", TestOutcome::Skipped(String::new())),
            ],
        };
        assert_eq!(report.totals(), TestTotals { tests: 3, passed: 1, failed: 1, skipped: 1 });
        assert!(!report.passed());
        assert!(TestReport { seed: None, cases: vec![case("c", TestOutcome::Skipped("later".into()))] }.passed());
    }


    fn sample(seed: Option<u64>) -> TestReport {
        TestReport {
            seed,
            cases: vec![
                TestCase { name: "Account :: opens".into(), position: 0, execution: 2, outcome: TestOutcome::Passed, cases: vec![], output: None },
                TestCase {
                    name: "a <tag> & \"quote\"".into(),
                    position: 1,
                    execution: 0,
                    outcome: TestOutcome::Failed(vec!["expected 2, got 1".into(), "two\nlines\u{1} é".into()]),
                    cases: vec![],
                    output: None,
                },
                TestCase {
                    name: "later".into(),
                    position: 2,
                    execution: 1,
                    outcome: TestOutcome::Skipped("not 'today'".into()),
                    cases: vec![],
                    output: None,
                },
            ],
        }
    }

    #[test]
    fn json_is_one_document_with_totals_seed_and_every_test_in_declaration_order() {
        let text = json(&sample(Some(42)), "notes.ok");
        assert_eq!(
            text,
            r#"{
  "passed": false,
  "report": "okkama-tests",
  "seed": 42,
  "suite": "notes.ok",
  "tests": [
    {
      "execution": 2,
      "name": "Account :: opens",
      "outcome": "passed",
      "position": 0
    },
    {
      "execution": 0,
      "failures": [
        "expected 2, got 1",
        "two\nlines\u0001 é"
      ],
      "name": "a <tag> & \"quote\"",
      "outcome": "failed",
      "position": 1
    },
    {
      "execution": 1,
      "name": "later",
      "outcome": "skipped",
      "position": 2,
      "reason": "not 'today'"
    }
  ],
  "totals": {
    "failed": 1,
    "passed": 1,
    "skipped": 1,
    "tests": 3
  },
  "version": 1
}
"#
        );
        assert!(json(&sample(None), "x.ok").contains("\n  \"seed\": null,\n"));
        let empty = TestReport { seed: None, cases: vec![] };
        assert!(json(&empty, "x.ok").contains("\"tests\": [],"));
    }

    #[test]
    fn junit_escapes_markup_and_control_characters_and_names_the_seed() {
        assert_eq!(
            junit(&sample(Some(7)), "suite <1>.ok"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <testsuites tests=\"3\" failures=\"1\" errors=\"0\" skipped=\"1\">\n\
             \x20 <testsuite name=\"suite &lt;1&gt;.ok\" tests=\"3\" failures=\"1\" errors=\"0\" skipped=\"1\">\n\
             \x20   <properties>\n\
             \x20     <property name=\"seed\" value=\"7\"/>\n\
             \x20   </properties>\n\
             \x20   <testcase name=\"Account :: opens\" classname=\"suite &lt;1&gt;.ok\"/>\n\
             \x20   <testcase name=\"a &lt;tag&gt; &amp; &quot;quote&quot;\" classname=\"suite &lt;1&gt;.ok\">\n\
             \x20     <failure message=\"expected 2, got 1\">expected 2, got 1\ntwo\nlines\u{FFFD} é</failure>\n\
             \x20   </testcase>\n\
             \x20   <testcase name=\"later\" classname=\"suite &lt;1&gt;.ok\">\n\
             \x20     <skipped message=\"not &apos;today&apos;\"/>\n\
             \x20   </testcase>\n\
             \x20 </testsuite>\n\
             </testsuites>\n"
        );
        assert!(!junit(&sample(None), "x.ok").contains("properties"));
    }

    #[test]
    fn formats_are_named_and_tap_is_not_one_yet() {
        assert_eq!(ReportFormat::named("junit"), Ok(ReportFormat::Junit));
        assert!(ReportFormat::named("tap").unwrap_err().contains("use human, json or junit"));
    }

    fn table() -> TestReport {
        let sub = |name: &str, outcome| SubCase { name: name.into(), outcome };
        TestReport {
            seed: None,
            cases: vec![
                TestCase {
                    name: "postage".into(),
                    position: 0,
                    execution: 0,
                    outcome: TestOutcome::Passed,
                    cases: vec![
                        sub("20 g", TestOutcome::Passed),
                        sub("50 g", TestOutcome::Failed(vec!["expected 120, got 99".into()])),
                        sub("later", TestOutcome::Skipped("not yet".into())),
                    ],
                    output: None,
                },
                TestCase {
                    name: "setup".into(),
                    position: 1,
                    execution: 1,
                    outcome: TestOutcome::Failed(vec!["no tariff".into()]),
                    cases: vec![sub("a", TestOutcome::Passed)],
                    output: None,
                },
            ],
        }
    }

    /// #809: a sub-case runs once, never inside another, and only when the
    /// filter's texts allow it; it keeps its own failures, apart from the test's.
    #[test]
    fn sub_cases_run_once_without_nesting_and_as_the_filter_selects() {
        let mut state = SubCaseState::new(None);
        assert_eq!(state.open("20 g"), SubCaseStart::Run);
        state.started("20 g".into(), vec!["parent".into()]);
        assert!(state.running());
        assert_eq!(state.open("inner"), SubCaseStart::Nested("20 g".into()));
        assert_eq!(state.finished(vec!["sub".into()], None), vec!["parent".to_owned()]);
        assert!(!state.running());
        assert_eq!(state.open("20 g"), SubCaseStart::Duplicate);
        state.started("50 g".into(), Vec::new());
        state.finished(Vec::new(), Some("later".into()));
        assert_eq!(
            state.cases(),
            vec![
                SubCase { name: "20 g".into(), outcome: TestOutcome::Failed(vec!["sub".into()]) },
                SubCase { name: "50 g".into(), outcome: TestOutcome::Skipped("later".into()) },
            ]
        );
        let filtered = SubCaseState::new(Some(vec!["50".into()]));
        assert_eq!(filtered.open("20 g"), SubCaseStart::Leave);
        assert_eq!(filtered.open("250 g"), SubCaseStart::Run);
        assert_eq!(state.finished(vec!["kept".into()], None), vec!["kept".to_owned()]);
    }

    /// #809: a test with sub-cases fails when one fails, and the totals count
    /// sub-cases, the test itself only when it failed or skipped outside them.
    #[test]
    fn verdicts_and_totals_count_sub_cases_not_their_containers() {
        let report = table();
        assert_eq!(report.cases[0].verdict(), TestOutcome::Failed(Vec::new()));
        assert_eq!(report.cases[1].verdict(), TestOutcome::Failed(vec!["no tariff".into()]));
        assert_eq!(report.totals(), TestTotals { tests: 5, passed: 2, failed: 2, skipped: 1 });
        assert!(!report.passed());
    }

    #[test]
    fn json_and_junit_keep_sub_cases_under_their_tests() {
        let text = json(&table(), "t.ok");
        assert!(text.contains(
            "      \"cases\": [\n        {\n          \"id\": \"postage/20 g\",\n          \"name\": \"20 g\",\n          \"outcome\": \"passed\"\n        },\n"
        ), "{text}");
        assert!(text.contains("\"failures\": [\n            \"expected 120, got 99\"\n          ],\n          \"id\": \"postage/50 g\""), "{text}");
        assert!(text.contains("\"execution\": 0,\n      \"failures\": [],\n      \"name\": \"postage\",\n      \"outcome\": \"failed\""), "{text}");
        assert!(text.contains("\"totals\": {\n    \"failed\": 2,\n    \"passed\": 2,\n    \"skipped\": 1,\n    \"tests\": 5\n  }"), "{text}");
        let xml = junit(&table(), "t.ok");
        assert!(xml.contains("<testsuites tests=\"5\" failures=\"2\" errors=\"0\" skipped=\"1\">"), "{xml}");
        assert!(xml.contains("<testcase name=\"postage/20 g\" classname=\"t.ok\"/>"), "{xml}");
        assert!(!xml.contains("<testcase name=\"postage\""), "{xml}");
        assert!(xml.contains("<testcase name=\"setup/a\" classname=\"t.ok\"/>\n    <testcase name=\"setup\" classname=\"t.ok\">"), "{xml}");
    }

    /// #1011: a crashed worker is a JUnit `error` and counted as one; a failed
    /// check and a deadline are `failure`s.
    #[test]
    fn junit_counts_crashes_as_errors() {
        let mut report = sample(None);
        report.cases[0].outcome = TestOutcome::Crashed("signal 9".into());
        report.cases[2].outcome = TestOutcome::TimedOut(500);
        let xml = junit(&report, "w.ok");
        assert!(xml.contains("<testsuites tests=\"3\" failures=\"2\" errors=\"1\" skipped=\"0\">"), "{xml}");
        assert!(xml.contains("<testsuite name=\"w.ok\" tests=\"3\" failures=\"2\" errors=\"1\" skipped=\"0\">"), "{xml}");
        assert_eq!(xml.matches("<error ").count(), 1);
        assert_eq!(xml.matches("<failure ").count(), 2);
    }
}
