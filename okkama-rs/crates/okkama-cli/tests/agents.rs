//! #1190: `okkama agents`, the entry point for AI agents. `okkama help` points to it; it prints the entry page,
//! the language reference, the style guide, the workflow and the std map, all built into this binary; `init`
//! writes a project's AGENTS.md (and CLAUDE.md), never over an existing file without `--force`, and `--check`
//! tells a current file from a stale or foreign one.
mod tree;

fn agents(args: &[&str]) -> (Option<i32>, String, String) {
    let output = tree::okkama().arg("agents").args(args).output().unwrap();
    (output.status.code(), String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned())
}

#[test]
fn help_points_agents_to_the_agents_command() {
    let output = tree::okkama().arg("help").output().unwrap();
    let help = String::from_utf8_lossy(&output.stdout);
    let first = help.lines().find(|line| line.contains("AI agents")).expect("an agents line");
    assert!(first.contains("okkama agents"), "{first}");
    assert!(help.contains("\n    agents    For AI agents"), "{help}");
    let page = tree::okkama().args(["help", "agents"]).output().unwrap();
    assert!(String::from_utf8_lossy(&page.stdout).starts_with("okkama agents — the entry point for AI agents"));
}

#[test]
fn the_entry_page_names_the_workflow_std_and_every_section() {
    let (code, page, _) = agents(&[]);
    assert_eq!(code, Some(0));
    assert!(page.starts_with(&format!("# Okkama for AI agents (okkama {})", env!("CARGO_PKG_VERSION"))), "{page}");
    for needle in ["okkama check", "okkama fmt -w", "okkama lint", "okkama run -t", "okkama man -k"] {
        assert!(page.contains(needle), "{needle}: {page}");
    }
    assert!(page.contains(&tree::std_dir().display().to_string()), "the std directory: {page}");
    for section in ["language", "style", "workflow", "std", "init"] {
        assert!(page.contains(&format!("okkama agents {section}")), "{section}: {page}");
    }
    let (code, json, _) = agents(&["--format", "json"]);
    assert_eq!(code, Some(0));
    assert!(json.starts_with(&format!("{{\"okkama\":\"{}\"", env!("CARGO_PKG_VERSION"))), "{json}");
}

#[test]
fn every_section_prints_its_markdown() {
    for section in ["language", "style", "workflow"] {
        let (code, text, _) = agents(&[section]);
        assert_eq!(code, Some(0), "{section}");
        assert!(text.starts_with("# "), "{section}: {text}");
    }
    let (code, map, _) = agents(&["std"]);
    assert_eq!(code, Some(0), "{map}");
    assert!(map.contains("`io/stdout`"), "the std map lists io/stdout: {map}");
    assert!(map.contains("okkama man PATH"), "{map}");
}

#[test]
fn init_writes_once_refuses_to_overwrite_and_checks_its_version() {
    let dir = std::env::temp_dir().join(format!("okkama-agents-init-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.to_str().unwrap();
    let (code, out, _) = agents(&["init", path, "--claude"]);
    assert_eq!(code, Some(0), "{out}");
    let written = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    assert!(written.starts_with(&format!("<!-- written by `okkama agents init`, okkama {} -->", env!("CARGO_PKG_VERSION"))));
    assert!(written.contains("okkama agents language") && written.contains("okkama run -t"), "{written}");
    assert!(std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap().contains("Read AGENTS.md first"));
    // An existing file is kept, unless --force.
    std::fs::write(dir.join("AGENTS.md"), "# mine\n").unwrap();
    let (code, _, err) = agents(&["init", path]);
    assert_eq!(code, Some(1));
    assert!(err.contains("exists; --force replaces it"), "{err}");
    assert_eq!(std::fs::read_to_string(dir.join("AGENTS.md")).unwrap(), "# mine\n");
    let (code, _, err) = agents(&["init", path, "--check"]);
    assert_eq!(code, Some(1));
    assert!(err.contains("not written by `okkama agents init`"), "{err}");
    // A file of another version is stale; a fresh one is current.
    std::fs::write(dir.join("AGENTS.md"), "<!-- written by `okkama agents init`, okkama 0.0.1 -->\n").unwrap();
    let (code, _, err) = agents(&["init", path, "--check"]);
    assert_eq!(code, Some(1));
    assert!(err.contains("written by okkama 0.0.1"), "{err}");
    let (code, _, _) = agents(&["init", path, "--force"]);
    assert_eq!(code, Some(0));
    let (code, out, _) = agents(&["init", path, "--check"]);
    assert_eq!(code, Some(0));
    assert!(out.contains("current"), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wrong_use_is_a_usage_error() {
    for args in [&["nonsense"][..], &["--force"], &["language", "extra"], &["init", "--format", "json"], &["--format", "xml"]] {
        let (code, _, err) = agents(args);
        assert_eq!(code, Some(2), "{args:?}: {err}");
    }
}
