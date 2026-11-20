use super::*;

#[test]
fn a_spec_names_categories_filters_and_dumps() {
    let config = Config::parse("own:connection, loader,dump", Some(PathBuf::from("/x")));
    assert!(config.on("own") && config.on("loader") && !config.on("gc"));
    assert!(config.matches("own", "net.tcp.TcpEndpointRace.connection#0"));
    assert!(!config.matches("own", "main"));
    assert!(config.matches("loader", "main"));
    assert!(config.dumps);
    assert!(!config.matches("gc", "main"));
}

#[test]
fn all_turns_every_category_on_without_filters() {
    let config = Config::parse("all", None);
    assert!(config.on("anything") && config.matches("anything", "f") && !config.dumps);
}

#[test]
fn an_empty_spec_turns_nothing_on() {
    let config = Config::parse(" , ", None);
    assert!(!config.all && config.categories.is_empty());
}

#[test]
fn json_strings_escape_quotes_controls_and_newlines() {
    assert_eq!(json_string("a\"b\\c\nd\u{1}"), "\"a\\\"b\\\\c\\nd\\u0001\"");
}

#[test]
fn file_names_keep_symbols_readable_and_bounded() {
    assert_eq!(file_name("net.tcp.TcpRaceAttempt.new#0"), "net.tcp.TcpRaceAttempt.new#0");
    assert_eq!(file_name("a/b c"), "a_b_c");
    assert!(file_name(&"x".repeat(300)).chars().count() < 130);
}

/// The one test that touches the process-wide configuration (read once, at first use).
#[test]
fn switched_categories_record_and_the_output_holds_what_was_recorded() {
    let out = std::env::temp_dir().join(format!("okkama-devtrace-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    // Set the configuration directly instead of through the environment: setting a variable is `unsafe` in
    // edition 2024 and the workspace forbids `unsafe`.
    let _ = CONFIG.set(Some(Config::parse("own:main,stages,dump", Some(out.clone()))));
    START.get_or_init(Instant::now);
    count!("own", "splits");
    count!("own", "splits", 2);
    count!("gc", "cycles");
    event!("own", "dynamic owner", fn = "app.main", scope = 3);
    event!("own", "filtered out", fn = "other");
    {
        let _span = span!("stages", "own");
    }
    dump!("own", "after own", "app.main", || "text of main".to_string());
    dump!("own", "after own", "other", || -> String { unreachable!("a filtered dump is not built") });
    let summary = summary().unwrap();
    assert!(summary.contains("splits") && summary.contains("[stages] timers"), "{summary}");
    assert!(!summary.contains("cycles"), "gc is off: {summary}");
    finish();
    let events = std::fs::read_to_string(out.join("events.jsonl")).unwrap();
    assert!(events.contains("\"name\":\"dynamic owner\"") && events.contains("\"scope\":\"3\""), "{events}");
    assert!(!events.contains("filtered out"), "{events}");
    assert_eq!(std::fs::read_dir(out.join("dumps")).unwrap().count(), 1);
    let counters = std::fs::read_to_string(out.join("counters.json")).unwrap();
    assert!(counters.contains("\"own.splits\":3"), "{counters}");
    let _ = std::fs::remove_dir_all(&out);
}
