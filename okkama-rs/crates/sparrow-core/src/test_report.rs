//! The result of a test run, apart from how it is shown (#339): every test in
//! declaration order with its outcome, and the seed that ordered the run.
//! Execution fills it; renderers (the human stream here, JSON and JUnit in the
//! CLI) only read it.

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
    pub outcome: TestOutcome,
}

/// The whole run: the selected tests in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestReport {
    /// The seed that ordered the run; none for declaration order.
    pub seed: Option<u64>,
    pub cases: Vec<TestCase>,
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

    /// The counts, from the cases themselves.
    pub fn totals(&self) -> TestTotals {
        let count = |wanted: fn(&TestOutcome) -> bool| self.cases.iter().filter(|case| wanted(&case.outcome)).count();
        TestTotals {
            tests: self.cases.len(),
            passed: count(|outcome| matches!(outcome, TestOutcome::Passed)),
            failed: count(|outcome| matches!(outcome, TestOutcome::Failed(_))),
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
    let mut out = String::from("{\n");
    out.push_str(&format!("  \"passed\": {},\n", report.passed()));
    out.push_str("  \"report\": \"okkama-tests\",\n");
    out.push_str(&format!(
        "  \"seed\": {},\n",
        report.seed.map_or_else(|| "null".to_owned(), |seed| seed.to_string())
    ));
    out.push_str(&format!("  \"suite\": {},\n", json_text(suite)));
    out.push_str("  \"tests\": [");
    for (index, case) in report.cases.iter().enumerate() {
        out.push_str(if index == 0 { "\n" } else { ",\n" });
        out.push_str(&format!("    {{\n      \"execution\": {},\n", case.execution));
        if let TestOutcome::Failed(failures) = &case.outcome {
            out.push_str("      \"failures\": [");
            for (at, failure) in failures.iter().enumerate() {
                out.push_str(if at == 0 { "\n" } else { ",\n" });
                out.push_str(&format!("        {}", json_text(failure)));
            }
            out.push_str(if failures.is_empty() { "],\n" } else { "\n      ],\n" });
        }
        let outcome = match &case.outcome {
            TestOutcome::Passed => "passed",
            TestOutcome::Failed(_) => "failed",
            TestOutcome::Skipped(_) => "skipped",
        };
        out.push_str(&format!("      \"name\": {},\n", json_text(&case.name)));
        out.push_str(&format!("      \"outcome\": \"{outcome}\",\n"));
        match &case.outcome {
            TestOutcome::Skipped(reason) => {
                out.push_str(&format!("      \"position\": {},\n", case.position));
                out.push_str(&format!("      \"reason\": {}\n    }}", json_text(reason)));
            }
            _ => out.push_str(&format!("      \"position\": {}\n    }}", case.position)),
        }
    }
    out.push_str(if report.cases.is_empty() { "],\n" } else { "\n  ],\n" });
    out.push_str(&format!(
        "  \"totals\": {{\n    \"failed\": {},\n    \"passed\": {},\n    \"skipped\": {},\n    \"tests\": {}\n  }},\n",
        totals.failed, totals.passed, totals.skipped, totals.tests
    ));
    out.push_str("  \"version\": 1\n}\n");
    out
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
/// check, a `skipped` with the reason.
pub fn junit(report: &TestReport, suite: &str) -> String {
    let totals = report.totals();
    let suite = xml(suite);
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<testsuites tests=\"{}\" failures=\"{}\" errors=\"0\" skipped=\"{}\">\n",
        totals.tests, totals.failed, totals.skipped
    ));
    out.push_str(&format!(
        "  <testsuite name=\"{suite}\" tests=\"{}\" failures=\"{}\" errors=\"0\" skipped=\"{}\">\n",
        totals.tests, totals.failed, totals.skipped
    ));
    if let Some(seed) = report.seed {
        out.push_str(&format!("    <properties>\n      <property name=\"seed\" value=\"{seed}\"/>\n    </properties>\n"));
    }
    for case in &report.cases {
        let name = xml(&case.name);
        match &case.outcome {
            TestOutcome::Passed => out.push_str(&format!("    <testcase name=\"{name}\" classname=\"{suite}\"/>\n")),
            TestOutcome::Failed(failures) => {
                let first = failures.first().and_then(|text| text.lines().next()).unwrap_or("");
                out.push_str(&format!("    <testcase name=\"{name}\" classname=\"{suite}\">\n"));
                out.push_str(&format!(
                    "      <failure message=\"{}\">{}</failure>\n",
                    xml(first),
                    xml(&failures.join("\n"))
                ));
                out.push_str("    </testcase>\n");
            }
            TestOutcome::Skipped(reason) => {
                out.push_str(&format!("    <testcase name=\"{name}\" classname=\"{suite}\">\n"));
                out.push_str(&format!("      <skipped message=\"{}\"/>\n", xml(reason)));
                out.push_str("    </testcase>\n");
            }
        }
    }
    out.push_str("  </testsuite>\n</testsuites>\n");
    out
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
        let case = |name: &str, outcome| TestCase { name: name.into(), position: 0, execution: 0, outcome };
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
                TestCase { name: "Account :: opens".into(), position: 0, execution: 2, outcome: TestOutcome::Passed },
                TestCase {
                    name: "a <tag> & \"quote\"".into(),
                    position: 1,
                    execution: 0,
                    outcome: TestOutcome::Failed(vec!["expected 2, got 1".into(), "two\nlines\u{1} é".into()]),
                },
                TestCase {
                    name: "later".into(),
                    position: 2,
                    execution: 1,
                    outcome: TestOutcome::Skipped("not 'today'".into()),
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
}
