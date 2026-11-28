//! `okkama agents` (#1190): the entry point for AI agents. Nobody but its authors knows Okkama, so the toolchain
//! itself teaches an agent the language: an entry page, the language reference (#1188), the style guide (#1189), the
//! workflow and the std map, plus `okkama agents init`, which writes a project's AGENTS.md. The texts live in
//! `docs/agents/` and are built into the binary, so they always describe this very toolchain.

use crate::Outcome;
use std::path::{Path, PathBuf};

const LANGUAGE: &str = include_str!("../../../../docs/agents/language.md");
const STYLE: &str = include_str!("../../../../docs/agents/style.md");
const WORKFLOW: &str = include_str!("../../../../docs/agents/workflow.md");

/// The line `okkama agents init` stamps into what it writes; `--check` reads the version back.
const STAMP: &str = "<!-- written by `okkama agents init`, okkama ";

pub(crate) const HELP: &str = "okkama agents — the entry point for AI agents

USAGE:
    okkama agents [SECTION] [--format markdown|json]
    okkama agents init [DIR] [--claude] [--force]
    okkama agents init [DIR] --check

SECTIONS:
    (none)      The entry page: what Okkama is, the workflow, where std lives,
                how to look things up, and the sections below
    language    The language: syntax, semantics, types, errors, resources,
                with an example per rule
    style       How Okkama code is written: behaviour over state, immutability,
                roles, small objects; what is allowed and what is not
    workflow    The commands an agent uses and how to read their output
    std         Every std module with its one-line summary

init writes AGENTS.md into DIR (default: the current directory): the essentials
and the commands above, stamped with this okkama's version. --claude also writes
CLAUDE.md pointing to it. An existing file is kept unless --force; --check
reports a missing file or one written by another version (exit 1).
";

pub(crate) fn command(args: &[String], stdlib: &Path) -> Outcome {
    let mut json = false;
    let mut positional = Vec::new();
    let (mut force, mut claude, mut check) = (false, false, false);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "-h" | "--help" => return Outcome::success(HELP.to_string()),
            "--force" => force = true,
            "--claude" => claude = true,
            "--check" => check = true,
            "--format" => match rest.next().map(String::as_str) {
                Some("markdown" | "md" | "text") => json = false,
                Some("json") => json = true,
                _ => return Outcome::usage("--format needs markdown or json"),
            },
            other if other.starts_with('-') => return Outcome::usage(&format!("unknown option `{other}` for `okkama agents`")),
            other => positional.push(other.to_string()),
        }
    }
    let section = positional.first().map(String::as_str);
    if section != Some("init") && (force || claude || check) {
        return Outcome::usage("--force, --claude and --check go with `okkama agents init`");
    }
    match (section, &positional[1.min(positional.len())..]) {
        (None, []) if json => Outcome::success(entry_json(stdlib)),
        (None, []) => Outcome::success(entry(stdlib)),
        (Some(name @ ("language" | "style" | "workflow" | "std")), []) => {
            let text = match name {
                "language" => LANGUAGE.to_string(),
                "style" => STYLE.to_string(),
                "workflow" => WORKFLOW.to_string(),
                _ => match crate::man::std_index(true) {
                    Ok(index) => std_map(&index, stdlib),
                    Err(message) => return Outcome::failure(format!("okkama agents: {message}\n"), crate::EXIT_DIAGNOSTICS),
                },
            };
            if json {
                Outcome::success(format!("{{\"section\":{},\"markdown\":{}}}\n", quote(name), quote(&text)))
            } else {
                Outcome::success(text)
            }
        }
        (Some("init"), dirs) if dirs.len() <= 1 => {
            if json {
                return Outcome::usage("init writes files; --format does not apply");
            }
            let dir = dirs.first().map_or_else(|| PathBuf::from("."), PathBuf::from);
            if check { init_check(&dir) } else { init(&dir, force, claude) }
        }
        (Some(other), []) => Outcome::usage(&format!("unknown section `{other}`: language, style, workflow, std or init")),
        _ => Outcome::usage("okkama agents takes one section (init takes one directory)"),
    }
}

fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The entry page, about one screen.
fn entry(stdlib: &Path) -> String {
    format!(
        "# Okkama for AI agents (okkama {version})

Okkama is a statically typed, object-oriented language: immutable objects that answer by behaviour, roles
instead of inheritance, no null, typed failures, resources with an owner and a `destroy`, async with structured
concurrency. Programs run on Sparrow (a JIT). It is new: do not guess its syntax from other languages; read
`okkama agents language` first, then `okkama agents style`.

## Workflow

    okkama check FILE.ok          every compile-time error, with its place
    okkama fmt -w FILE.ok         the canonical layout (always run it)
    okkama lint FILE.ok           warnings and style findings
    okkama run -t FILE.ok         the module's `test` blocks
    okkama run FILE.ok [ARGS]     run `main`

## Looking things up

    okkama man                    every std module with its summary
    okkama man MODULE [NAME]      a module, or one declaration (NAME.ACTION for an action)
    okkama man -k QUERY           search std's documentation

std is in {std}.

## Sections

    okkama agents language        syntax and semantics, an example per rule
    okkama agents style           how to write Okkama: allowed, not allowed, why
    okkama agents workflow        the commands and how to read their output
    okkama agents std             the std map
    okkama agents init [DIR]      write AGENTS.md for a project (--claude: CLAUDE.md too)
",
        version = version(),
        std = stdlib.display()
    )
}

fn entry_json(stdlib: &Path) -> String {
    format!(
        "{{\"okkama\":{},\"std\":{},\"sections\":[\"language\",\"style\",\"workflow\",\"std\"],\"entry\":{}}}\n",
        quote(version()),
        quote(&stdlib.display().to_string()),
        quote(&entry(stdlib))
    )
}

/// The std map: where std is, how to read a page, then the index.
fn std_map(index: &str, stdlib: &Path) -> String {
    format!(
        "# The Okkama standard library (okkama {})

std is in {}. Import a module with `import \"PATH\" take NAME, NAME;` and read its page with `okkama man PATH`
(one declaration: `okkama man PATH NAME`). Prefer std to writing your own: it covers text, collections, time,
files, processes, networking, HTTP and the web, databases, encodings, compression, cryptography and testing.

{}",
        version(),
        stdlib.display(),
        index
    )
}

/// The AGENTS.md `init` writes.
fn agents_md() -> String {
    format!(
        "{STAMP}{version} -->
# Working on this Okkama project

This project is written in Okkama, a new language: do not infer its syntax or idioms from other languages.
Before writing code, read what the installed toolchain says about itself:

- `okkama agents` — the entry page;
- `okkama agents language` — syntax and semantics, an example per rule;
- `okkama agents style` — how Okkama code is written, what is allowed and what is not;
- `okkama agents std` — the standard library; `okkama man MODULE [NAME]` for one page.

## Every change

1. `okkama check FILE.ok` until no error is left.
2. `okkama fmt -w FILE.ok`.
3. `okkama lint FILE.ok`, then fix what it reports.
4. `okkama run -t FILE.ok` — the module's `test` blocks, next to the code they test.

## Rules of thumb

- Objects answer by behaviour and never hand out their state; values are immutable (a change is a successor).
- Roles, not inheritance; small objects; errors are enums of your own, never sentinel values.
- No free functions, no `while`, no `+=`: read `okkama agents language` before writing code from habit.
- Use std before writing a helper of your own.

This file was written by okkama {version}; `okkama agents init --check` tells whether it is current.
",
        version = version()
    )
}

fn claude_md() -> String {
    format!("{STAMP}{} -->\nRead AGENTS.md first: it says how to work on this Okkama project.\n", version())
}

fn init(dir: &Path, force: bool, claude: bool) -> Outcome {
    if !dir.is_dir() {
        return Outcome::failure(format!("okkama agents init: {} is not a directory\n", dir.display()), crate::EXIT_DIAGNOSTICS);
    }
    let mut files = vec![("AGENTS.md", agents_md())];
    if claude {
        files.push(("CLAUDE.md", claude_md()));
    }
    // Nothing is written unless every file may be.
    for (name, _) in &files {
        let path = dir.join(name);
        if path.exists() && !force {
            return Outcome::failure(
                format!("okkama agents init: {} exists; --force replaces it\n", path.display()),
                crate::EXIT_DIAGNOSTICS,
            );
        }
    }
    let mut written = String::new();
    for (name, text) in files {
        let path = dir.join(name);
        if let Err(error) = std::fs::write(&path, text) {
            return Outcome::failure(format!("okkama agents init: {}: {error}\n", path.display()), crate::EXIT_DIAGNOSTICS);
        }
        written.push_str(&format!("wrote {}\n", path.display()));
    }
    Outcome::success(written)
}

fn init_check(dir: &Path) -> Outcome {
    let path = dir.join("AGENTS.md");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Outcome::failure(format!("{}: missing; run `okkama agents init`\n", path.display()), crate::EXIT_DIAGNOSTICS);
    };
    let written = text.lines().next().and_then(|line| line.strip_prefix(STAMP)).and_then(|rest| rest.strip_suffix(" -->"));
    match written {
        Some(found) if found == version() => Outcome::success(format!("{}: current (okkama {found})\n", path.display())),
        Some(found) => Outcome::failure(
            format!("{}: written by okkama {found}, this is okkama {}; run `okkama agents init --force`\n", path.display(), version()),
            crate::EXIT_DIAGNOSTICS,
        ),
        None => Outcome::failure(
            format!("{}: not written by `okkama agents init`; leave it, or replace it with --force\n", path.display()),
            crate::EXIT_DIAGNOSTICS,
        ),
    }
}

/// A JSON string.
fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\u{0}'..='\u{1f}' => out.push_str(&format!("\\u{:04x}", character as u32)),
            _ => out.push(character),
        }
    }
    out.push('"');
    out
}
