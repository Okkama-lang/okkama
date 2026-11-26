//! Core library for the Blade command-line tool.

pub mod install;
pub mod lock;
pub mod manifest;
pub mod project;
pub mod version;

use std::{env::Args, io::Write, process::ExitCode};
use std::path::PathBuf;
//use std::path::{Path, PathBuf};

use install::Report;

/// Help text shared by Blade command-line front ends.
pub const HELP: &str = "\
blade — Blade command-line tool

USAGE:
    blade new NAME                   create the project NAME in a new directory
                                     NAME (never over an existing one)
    blade add NAME GIT [MIN]         depend on the package in the git repository
                                     GIT, version MIN or later (default: its
                                     newest tag), and install it; adding NAME
                                     again with another MIN moves it
    blade sync                       install exactly what blade.lock records into
                                     pkg/; without blade.lock, resolve and write it
    blade update [NAME...]           resolve NAMEs again, the others staying where
                                     they are; without NAME, everything
    blade remove NAME                drop a dependency and what only it needed
    blade build                      compile the project as [build] says
    blade run [-- ARGS...]           build if sources changed, then run it as
                                     [run] says; ARGS follow [run] args
    blade --help
    blade --version

MIN is MAJOR.MINOR.PATCH: that version or a later one of the same major.
Blade installs the lowest version that meets every minimum in the project,
so a new release changes nothing until a minimum is raised. A package is a
git repository with its modules in src/, imported as pkg/NAME/src/…; its
version is its vX.Y.Z tag, and it needs blade.toml only for dependencies of
its own. `blade install` is now `blade sync`, `blade install SOURCE` is now
`blade add`.

blade.toml, build and run (all keys optional, defaults shown):
    [build]
    entry = \"src/main.ok\"       # or a list of the root module's files
    output = \"target/NAME.spb\"  # Sparrow bytecode
    [run]
    args = []
    env = {}
Blade runs the okkama next to it (or on PATH; BLADE_OKKAMA overrides).
";

/// Version of the Blade package.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub trait Command {
    fn execute(&self) -> ExitCode;
}

#[derive(Debug)]
pub struct Cli {
    args: Args,
    //args: Vec<String>,
}

struct CommandNew {
    args: Vec<String>,
}

struct CommandAdd {
    args: Vec<String>,
}

struct CommandInstall {
    args: Vec<String>,
}

struct CommandSync {
    args: Vec<String>,
}

struct CommandRemove {
    args: Vec<String>,
}

struct CommandUpdate {
    args: Vec<String>,
}

struct CommandBuild {
    args: Vec<String>,
}

struct CommandRun {
    args: Vec<String>,
}

struct CommandUsage {
    //args: Args,
    error: bool,
}

struct CommandVersion {
    version: String,
}

impl Command for CommandNew {
    fn execute(&self) -> ExitCode {
        let [name] = self.args.as_slice() else {
            eprint!("{HELP}");
            return ExitCode::from(2);
        };
        // The manifest of the new project must pass its own check (#1039).
        if let Err(e) = manifest::check_package_name(name) {
            return failure(&e);
        }
        let root = match std::env::current_dir() {
            Ok(cwd) => cwd.join(name),
            Err(e) => return failure(&format!("cannot read the current directory: {e}")),
        };
        // Never over an existing file or directory: `new` writes nothing it did not create (#1039).
        if root.symlink_metadata().is_ok() {
            return failure(&format!("{name} already exists; `blade new` makes a new project"));
        }
        match new_project(&root, name) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => failure(&format!("cannot make the project {name}: {e}")),
        }
    }
}

/// The files of a new project `name` in the new directory `root`: `src/main.ok`, `blade.toml` and an empty
/// `pkg/` for dependencies. Each directory and file is created new, so nothing existing is overwritten.
fn new_project(root: &std::path::Path, name: &str) -> std::io::Result<()> {
    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(root)?;
    std::fs::create_dir(root.join("src"))?;
    std::fs::create_dir(root.join(manifest::PKG))?;
    let main_ok = format!(
        r#"module {name};

import "io/stdout" take Stdout;

main() is int {{
    Stdout().writeln("hello from okkama");
    0;
}}
"#
    );
    let output = project::default_output(name);
    let manifest = format!(
        r#"[module]
name = "{name}"
version = "1.0.0"

[build]
entry = "src/main.ok"
output = "{output}"

[run]
args = []
env = {{}}

[dependencies]
"#
    );
    for (path, text) in [("src/main.ok", main_ok), (manifest::MANIFEST, manifest)] {
        std::fs::File::create_new(root.join(path))?.write_all(text.as_bytes())?;
    }
    Ok(())
}

impl Command for CommandInstall {
    fn execute(&self) -> ExitCode {
        let git = self.args.iter().any(|arg| arg == "--git");
        let rest: Vec<&String> = self.args.iter().filter(|arg| *arg != "--git").collect();
        let root = match project_root() {
            Ok(root) => root,
            Err(e) => return failure(&e),
        };
        // Kept for one release, as the commands that replace them.
        let result = match rest.as_slice() {
            [] if !git => {
                eprintln!("blade: `blade install` is now `blade sync`");
                install::sync(&root)
            }
            [source] | [source, _] => {
                let version = rest.get(1).map(|version| version.as_str());
                install::add_source(&root, source, version, git).map(|(name, report)| {
                    let min = version.map(|min| format!(" {min}")).unwrap_or_default();
                    eprintln!("blade: `blade install SOURCE` is now `blade add {name} {source}{min}`");
                    report
                })
            }
            _ => {
                eprint!("{HELP}");
                return ExitCode::from(2);
            }
        };
        finish(result)
    }
}

impl Command for CommandAdd {
    fn execute(&self) -> ExitCode {
        let (name, git, min) = match self.args.as_slice() {
            [name, git] => (name, git, None),
            [name, git, min] => (name, git, Some(min.as_str())),
            _ => {
                eprint!("{HELP}");
                return ExitCode::from(2);
            }
        };
        match project_root() {
            Ok(root) => finish(install::add(&root, name, git, min)),
            Err(e) => failure(&e),
        }
    }
}

impl Command for CommandSync {
    fn execute(&self) -> ExitCode {
        if !self.args.is_empty() {
            eprint!("{HELP}");
            return ExitCode::from(2);
        }
        match project_root() {
            Ok(root) => finish(install::sync(&root)),
            Err(e) => failure(&e),
        }
    }
}

impl Command for CommandRemove {
    fn execute(&self) -> ExitCode {
        let [name] = self.args.as_slice() else {
            eprint!("{HELP}");
            return ExitCode::from(2);
        };
        let root = match project_root() {
            Ok(root) => root,
            Err(e) => return failure(&e),
        };
        finish(install::remove(&root, name))
    }
}

impl Command for CommandUpdate {
    fn execute(&self) -> ExitCode {
        let root = match project_root() {
            Ok(root) => root,
            Err(e) => return failure(&e),
        };
        finish(install::update(&root, &self.args))
    }
}

/// The project of the current directory: the nearest directory with blade.toml.
fn project_root() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("cannot read the current directory: {e}"))?;
    manifest::find_root(&cwd)
        .ok_or_else(|| format!("no {} in {} or above it", manifest::MANIFEST, cwd.display()))
}

fn failure(message: &str) -> ExitCode {
    eprintln!("blade: {message}");
    ExitCode::from(2)
}

fn finish(result: Result<Report, String>) -> ExitCode {
    match result {
        Ok(report) if report.lines.is_empty() => {
            println!("dependencies are up to date");
            ExitCode::SUCCESS
        }
        Ok(report) => {
            for line in &report.lines {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => failure(&e),
    }
}

impl Command for CommandBuild {
    fn execute(&self) -> ExitCode {
        if !self.args.is_empty() {
            eprint!("{HELP}");
            return ExitCode::from(2);
        }
        let project = match project_root().and_then(|root| project::Project::read(&root)) {
            Ok(project) => project,
            Err(e) => return failure(&e),
        };
        match project::build(&project, true) {
            Ok(_) => {
                println!("built {}", project.output);
                ExitCode::SUCCESS
            }
            Err(e) => failure(&e),
        }
    }
}

impl Command for CommandRun {
    fn execute(&self) -> ExitCode {
        let extra = match self.args.split_first() {
            None => &[][..],
            Some((dashes, rest)) if dashes == "--" => rest,
            Some(_) => {
                eprint!("{HELP}");
                return ExitCode::from(2);
            }
        };
        let project = match project_root().and_then(|root| project::Project::read(&root)) {
            Ok(project) => project,
            Err(e) => return failure(&e),
        };
        match project::run(&project, extra) {
            // The program's status, clamped to what a process can return.
            Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
            Err(e) => failure(&e),
        }
    }
}

impl Command for  CommandVersion {
    fn execute(&self) -> ExitCode {
        println!("{}", self.version);
        ExitCode::SUCCESS
    }
}

impl Command for CommandUsage {
    fn execute(&self) -> ExitCode {
        if self.error {
            eprint!("{HELP}");
            ExitCode::from(2)
        }
        else {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
    }
}

impl CommandNew {
    fn new(args: Args) -> Self {
        Self { args: args.collect() }
    }
}

impl CommandUsage {
    fn new(error: bool) -> Self {
        Self{ error }
    }
}

impl  CommandVersion {
    pub fn new(version: String) -> Self {
        Self { version }
    }
}

impl Cli {
    pub fn new(args: Args) -> Self {
        Self { args }
    }

    pub fn command(self) -> Box<dyn Command> {
        let mut args = self.args;
        args.next();
        let command = args.next();
        // After `--` the arguments belong to the program (`blade run -- --help`).
        let help = std::env::args().skip(2).take_while(|arg| arg != "--").any(|arg| arg == "--help" || arg == "-h");
        let cmd: Box<dyn Command> = match command.as_deref() {
            Some(_) if help                       => Box::new(CommandUsage::new(false)),
            Some("new")                           => Box::new(CommandNew::new(args)),
            Some("add")                           => Box::new(CommandAdd { args: args.collect() }),
            Some("install")                       => Box::new(CommandInstall { args: args.collect() }),
            Some("sync")                          => Box::new(CommandSync { args: args.collect() }),
            Some("remove")                        => Box::new(CommandRemove { args: args.collect() }),
            Some("update")                        => Box::new(CommandUpdate { args: args.collect() }),
            Some("build")                         => Box::new(CommandBuild { args: args.collect() }),
            Some("run")                           => Box::new(CommandRun { args: args.collect() }),
            Some("--help" | "-h" | "help") | None => Box::new(CommandUsage::new(false)),
            Some("--version" | "-v" | "version")  => Box::new(CommandVersion::new(String::from(VERSION))),
            Some(_)                               => Box::new(CommandUsage::new(true)),
        };
        cmd
    }

    //pub fn what(&self) {
    //    println!("hehehe");
    //}
    pub fn execute(&self) -> ExitCode {
        ExitCode::from(2)
    }
}
