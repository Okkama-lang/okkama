//! `blade build` and `blade run`: the `[build]` and `[run]` sections of
//! `blade.toml` and the `okkama` commands they become.
//!
//! ```toml
//! [build]
//! entry = "src/main.ok"      # or a list: the files of the root module
//! output = "target/app.spb"  # default target/<name>.spb
//!
//! [run]
//! args = []                  # `blade run -- a b` appends
//! env = {}
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::lock::{LOCK, Lock};
use crate::manifest::{MANIFEST, Manifest, PKG};
use crate::toolchain::{self, Toolchain};

/// A project with its build and run settings resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub root: PathBuf,
    pub name: String,
    /// Files of the root module, relative to `root`.
    pub entry: Vec<String>,
    /// The built file, relative to `root`.
    pub output: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// The default `[build] output` of package `name`: `target/<last segment>.spb`.
pub fn default_output(name: &str) -> String {
    let leaf = name.rsplit('/').next().unwrap_or(name);
    format!("target/{leaf}.spb")
}

impl Project {
    /// The project at `root` (its `blade.toml`).
    pub fn read(root: &Path) -> Result<Project, String> {
        let file = root.join(MANIFEST);
        let text = std::fs::read_to_string(&file).map_err(|error| format!("cannot read {}: {error}", file.display()))?;
        Project::parse(root, &text).map_err(|error| format!("{}: {error}", file.display()))
    }

    /// Parse `text` as the manifest of the project at `root`.
    pub fn parse(root: &Path, text: &str) -> Result<Project, String> {
        let manifest = Manifest::parse(text)?;
        let table: toml::Table = text.parse().map_err(|error: toml::de::Error| error.message().to_string())?;
        let section = |name: &str, keys: &[&str]| -> Result<toml::Table, String> {
            let Some(value) = table.get(name) else { return Ok(toml::Table::new()) };
            let section = value.as_table().ok_or(format!("[{name}] must be a table"))?.clone();
            // One engine, one target (owner, 2026-11-20): Sparrow bytecode, run with `okkama exec`.
            let gone = |key: &&String| matches!((name, key.as_str()), ("build", "target") | ("run", "engine"));
            if let Some(key) = section.keys().find(gone) {
                return Err(format!("[{name}] {key} is gone: Sparrow is the only engine; remove the key"));
            }
            if let Some(key) = section.keys().find(|key| !keys.contains(&key.as_str())) {
                return Err(format!("unknown key \"{key}\" in [{name}] (known: {})", keys.join(", ")));
            }
            Ok(section)
        };
        let build = section("build", &["entry", "output"])?;
        let run = section("run", &["args", "env"])?;
        let name = manifest.name.clone().unwrap_or_else(|| "app".to_string());
        let entry = match build.get("entry") {
            None => vec!["src/main.ok".to_string()],
            Some(toml::Value::String(file)) => vec![file.clone()],
            Some(toml::Value::Array(files)) if !files.is_empty() => strings(files, "[build] entry")?,
            Some(_) => return Err("[build] entry must be a file or a non-empty list of files".into()),
        };
        let output = text_of(&build, "build", "output")?.unwrap_or_else(|| default_output(&name));
        let args = match run.get("args") {
            None => Vec::new(),
            Some(toml::Value::Array(args)) => strings(args, "[run] args")?,
            Some(_) => return Err("[run] args must be a list of strings".into()),
        };
        let env = match run.get("env") {
            None => BTreeMap::new(),
            Some(toml::Value::Table(env)) => env
                .iter()
                .map(|(key, value)| match value {
                    toml::Value::String(value) => Ok((key.clone(), value.clone())),
                    _ => Err(format!("[run] env \"{key}\" must be a string")),
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err("[run] env must be a table of strings".into()),
        };
        Ok(Project { root: root.to_path_buf(), name, entry, output, args, env })
    }

    fn output_path(&self) -> PathBuf {
        self.root.join(&self.output)
    }

    /// `OUTPUT.blade-stamp`: what the output was built from (#1040).
    fn stamp_path(&self) -> PathBuf {
        let mut path = self.output_path().into_os_string();
        path.push(STAMP);
        PathBuf::from(path)
    }

    /// `OUTPUT.blade-cache`: the digests of the toolchain's files (#1080).
    fn cache_path(&self) -> PathBuf {
        let mut path = self.output_path().into_os_string();
        path.push(CACHE);
        PathBuf::from(path)
    }
}

/// The suffix of the stamp beside the output.
pub const STAMP: &str = ".blade-stamp";
/// The suffix of the toolchain's digest cache beside the output.
pub const CACHE: &str = ".blade-cache";
/// The first line of a stamp. A stamp of another format (one of 0.6.5's
/// first builds, `inputs …` and `output …` alone) never matches, so the
/// output is built again (#1080).
const STAMP_FORMAT: &str = "blade-stamp 2";

fn strings(values: &[toml::Value], what: &str) -> Result<Vec<String>, String> {
    values
        .iter()
        .map(|value| value.as_str().map(str::to_string).ok_or(format!("{what} must be strings")))
        .collect()
}

fn text_of(table: &toml::Table, section: &str, key: &str) -> Result<Option<String>, String> {
    match table.get(key) {
        None => Ok(None),
        Some(toml::Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("[{section}] {key} must be a string")),
    }
}

/// The `okkama` Blade uses: `BLADE_OKKAMA`, the one next to `blade`, or `okkama` on PATH.
pub fn okkama() -> PathBuf {
    if let Some(path) = std::env::var_os("BLADE_OKKAMA") {
        return PathBuf::from(path);
    }
    let name = format!("okkama{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&name)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Every declared dependency is installed as `blade.lock` says.
fn check_installed(project: &Project) -> Result<(), String> {
    crate::install::settled(&project.root)?;
    let manifest = Manifest::read(&project.root)?;
    let lock = Lock::read(&project.root)?;
    let missing = manifest
        .dependencies
        .keys()
        .find(|name| !lock.packages.contains_key(*name) || !project.root.join(PKG).join(name).is_dir());
    match missing {
        Some(name) => Err(format!(
            "dependency \"{name}\" is not installed (no {LOCK} entry or {PKG}/{name}); run `blade sync`"
        )),
        None => Ok(()),
    }
}

/// The files Blade builds from, relative to the root with `/` separators and
/// sorted: every `.ok` file outside the output's directory and hidden
/// directories, `blade.toml` and `blade.lock`.
fn inputs(project: &Project) -> Result<Vec<String>, String> {
    let skip = project.output_path().parent().map(Path::to_path_buf);
    let mut found = Vec::new();
    let mut stack = vec![project.root.clone()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("{}: {error}", dir.display()))?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|error| format!("{}: {error}", path.display()))?;
            let name = entry.file_name();
            if kind.is_dir() {
                if !name.to_string_lossy().starts_with('.') && Some(&path) != skip.as_ref() {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().is_some_and(|e| e == "ok") || name == MANIFEST || name == LOCK {
                let relative = path.strip_prefix(&project.root).unwrap_or(&path);
                let parts: Vec<String> =
                    relative.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
                found.push(parts.join("/"));
            }
        }
    }
    found.sort();
    Ok(found)
}

/// The stamp of a build: the toolchain that made it (the `okkama` version,
/// its binary and its std, by content, #1080), the fingerprint of its inputs
/// (which files, and the bytes of each) and of the output it made.
/// Modification times play no part: a deleted source, or one replaced by an
/// older file (`cp -p`, an unpacked archive), changes the stamp as an edit
/// does (#1040).
fn stamp(project: &Project, toolchain: &Toolchain, inputs: &str) -> Result<String, String> {
    let output = crate::install::checksum_of(&project.root, std::slice::from_ref(&project.output))?;
    Ok(format!("{STAMP_FORMAT}\n{toolchain}inputs {inputs}\noutput {output}\n"))
}

/// The fingerprint of the files the project builds from, now.
fn fingerprint(project: &Project) -> Result<String, String> {
    crate::install::checksum_of(&project.root, &inputs(project)?)
}

/// Was the output built by this toolchain from these very inputs, and is it
/// still what that build wrote?
fn up_to_date(project: &Project, toolchain: &Toolchain, inputs: &str) -> Result<bool, String> {
    let Ok(recorded) = std::fs::read_to_string(project.stamp_path()) else {
        return Ok(false);
    };
    if !project.output_path().is_file() {
        return Ok(false);
    }
    Ok(recorded == stamp(project, toolchain, inputs)?)
}

/// Build the project unless its output was built from the sources as they are
/// (`force` always builds). `Ok(true)` — it was built.
pub fn build(project: &Project, force: bool) -> Result<bool, String> {
    check_installed(project)?;
    for file in &project.entry {
        if !project.root.join(file).is_file() {
            return Err(format!("[build] entry \"{file}\" does not exist"));
        }
    }
    // Taken before the build: a source or the toolchain changed while it runs
    // makes the next run build again.
    let probe = toolchain::Probe::start(&okkama(), &project.root, &project.cache_path());
    let inputs = fingerprint(project)?;
    let toolchain = probe.finish()?;
    if !force && up_to_date(project, &toolchain, &inputs)? {
        return Ok(false);
    }
    let stamp_file = project.stamp_path();
    if let Err(error) = std::fs::remove_file(&stamp_file)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(format!("{}: {error}", stamp_file.display()));
    }
    let output = project.output_path();
    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    }
    let status = Command::new(okkama())
        .current_dir(&project.root)
        .arg("build")
        .args(&project.entry)
        .arg("-o")
        .arg(&project.output)
        .status()
        .map_err(|error| format!("cannot run {}: {error}", okkama().display()))?;
    if !status.success() {
        return Err(format!("build failed ({status})"));
    }
    std::fs::write(&stamp_file, stamp(project, &toolchain, &inputs)?)
        .map_err(|error| format!("{}: {error}", stamp_file.display()))?;
    Ok(true)
}

/// The command `blade run` executes, after `extra` arguments.
pub fn run_command(project: &Project, extra: &[String]) -> Command {
    // The built `.spb` runs with `okkama exec`.
    let mut command = Command::new(okkama());
    command.arg("exec").arg(project.output_path());
    command.current_dir(&project.root).args(&project.args).args(extra).envs(&project.env);
    command
}

/// Build when needed, run, and return the program's exit status.
pub fn run(project: &Project, extra: &[String]) -> Result<i32, String> {
    build(project, false)?;
    let mut command = run_command(project, extra);
    let status = command
        .status()
        .map_err(|error| format!("cannot run {}: {error}", command.get_program().to_string_lossy()))?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_the_module() {
        let root = Path::new("/p");
        let project = Project::parse(root, "[module]\nname = \"acme/web\"\n").unwrap();
        assert_eq!(project.entry, ["src/main.ok"]);
        assert_eq!(project.output, "target/web.spb");
        assert!(project.args.is_empty() && project.env.is_empty());
    }

    #[test]
    fn malformed_settings_are_reported() {
        let root = Path::new("/p");
        for (text, expected) in [
            ("[build]\nentry = []\n", "non-empty list"),
            ("[build]\nentry = 3\n", "[build] entry"),
            ("[build]\ntarget = \"sparrow\"\n", "[build] target is gone"),
            ("[run]\nengine = \"sparrow\"\n", "[run] engine is gone"),
            ("[run]\nargs = \"a\"\n", "[run] args must be a list"),
            ("[run]\nargs = [1]\n", "[run] args must be strings"),
            ("[run]\nenv = { A = 1 }\n", "[run] env \"A\" must be a string"),
            ("[run]\nmode = \"x\"\n", "unknown key \"mode\" in [run]"),
            ("build = 1\n", "[build] must be a table"),
        ] {
            let error = Project::parse(root, text).unwrap_err();
            assert!(error.contains(expected), "{text}: {error}");
        }
    }

    /// A temporary project removed when dropped.
    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let dir = std::env::temp_dir().join(format!("blade-project-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }

        fn write(&self, path: &str, text: &str) -> PathBuf {
            let file = self.0.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, text).unwrap();
            file
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn age(file: &Path, hours: u64) {
        let time = std::time::SystemTime::now() - std::time::Duration::from_secs(3600 * hours);
        std::fs::File::options().write(true).open(file).unwrap().set_modified(time).unwrap();
    }

    /// #1040: the inputs are a set of files by content. Deleting, adding or
    /// renaming a source changes the fingerprint, and so does a source replaced
    /// by an older file; a time alone changes nothing.
    #[test]
    fn the_fingerprint_follows_the_files_and_their_bytes_not_their_times() {
        let temp = Temp::new("fingerprint");
        temp.write(MANIFEST, "[module]\nname = \"app\"\n");
        let main = temp.write("src/main.ok", "module app;\nimport \"greet\";\n");
        let greet = temp.write("src/greet.ok", "module app/greet; // hello\n");
        temp.write("pkg/util/src/util.ok", "module util;\n");
        temp.write("target/app.spb", "built");
        temp.write("target/old.ok", "module old;\n");
        temp.write(".git/hooks/x.ok", "module x;\n");
        temp.write("notes.txt", "not a source");
        let project = Project::parse(&temp.0, "[module]\nname = \"app\"\n").unwrap();
        assert_eq!(inputs(&project).unwrap(), ["blade.toml", "pkg/util/src/util.ok", "src/greet.ok", "src/main.ok"]);
        let first = fingerprint(&project).unwrap();

        age(&main, 2);
        temp.write("notes.txt", "edited");
        assert_eq!(fingerprint(&project).unwrap(), first, "times and other files do not count");

        // The same length, other bytes, an older time (`cp -p`).
        std::fs::write(&greet, "module app/greet; // HELLO\n").unwrap();
        age(&greet, 3);
        let older = fingerprint(&project).unwrap();
        assert_ne!(older, first);
        std::fs::write(&greet, "module app/greet; // hello\n").unwrap();
        assert_eq!(fingerprint(&project).unwrap(), first);

        std::fs::rename(&greet, temp.0.join("src/hello.ok")).unwrap();
        let renamed = fingerprint(&project).unwrap();
        std::fs::remove_file(temp.0.join("src/hello.ok")).unwrap();
        let deleted = fingerprint(&project).unwrap();
        temp.write(LOCK, "version = 1\n");
        let locked = fingerprint(&project).unwrap();
        let all = [&first, &older, &renamed, &deleted, &locked];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    /// #1040: a stamp holds only for the inputs it was written for and the
    /// output that build made.
    #[test]
    fn a_stamp_holds_for_its_inputs_and_its_output_only() {
        let temp = Temp::new("stamp");
        temp.write("src/main.ok", "module app;\n");
        let project = Project::parse(&temp.0, "[module]\nname = \"app\"\n").unwrap();
        let inputs = fingerprint(&project).unwrap();
        let tools = toolchain();
        assert!(!up_to_date(&project, &tools, &inputs).unwrap(), "never built");
        let output = temp.write("target/app.spb", "built");
        assert_eq!(project.stamp_path(), temp.0.join("target/app.spb.blade-stamp"));
        std::fs::write(project.stamp_path(), stamp(&project, &tools, &inputs).unwrap()).unwrap();
        assert!(up_to_date(&project, &tools, &inputs).unwrap());

        temp.write("src/main.ok", "module app; // edited\n");
        assert!(!up_to_date(&project, &tools, &fingerprint(&project).unwrap()).unwrap());
        temp.write("src/main.ok", "module app;\n");
        assert!(up_to_date(&project, &tools, &fingerprint(&project).unwrap()).unwrap());

        std::fs::write(&output, "other").unwrap();
        assert!(!up_to_date(&project, &tools, &inputs).unwrap(), "the output was replaced");
        std::fs::write(&output, "built").unwrap();
        std::fs::remove_file(&output).unwrap();
        assert!(!up_to_date(&project, &tools, &inputs).unwrap(), "no output");
    }

    fn toolchain() -> Toolchain {
        Toolchain {
            version: "okkama 0.6.5".to_string(),
            binary: format!("sha256:{}", "b".repeat(64)),
            std: format!("sha256:{}", "5".repeat(64)),
            imports: Vec::new(),
            build: Vec::new(),
        }
    }

    /// #1080: a stamp holds only for the toolchain that made the output: its
    /// version, its binary and its std. A stamp from before #1080 (inputs and
    /// output alone) holds for none, so the output is built again.
    #[test]
    fn a_stamp_holds_for_its_toolchain_only() {
        let temp = Temp::new("toolchain");
        temp.write("src/main.ok", "module app;\n");
        temp.write("target/app.spb", "built");
        let project = Project::parse(&temp.0, "[module]\nname = \"app\"\n").unwrap();
        let inputs = fingerprint(&project).unwrap();
        let built = toolchain();
        std::fs::write(project.stamp_path(), stamp(&project, &built, &inputs).unwrap()).unwrap();
        assert!(up_to_date(&project, &built, &inputs).unwrap());
        let other = |change: fn(&mut Toolchain)| {
            let mut now = built.clone();
            change(&mut now);
            now
        };
        for (what, now) in [
            ("version", other(|t| t.version = "okkama 0.6.6".to_string())),
            ("binary", other(|t| t.binary = format!("sha256:{}", "c".repeat(64)))),
            ("std", other(|t| t.std = "missing".to_string())),
            // #1087: the import directories, by content and order, and the build variables.
            ("import directory", other(|t| t.imports = vec![format!("sha256:{}", "1".repeat(64))])),
            ("missing import directory", other(|t| t.imports = vec!["missing".to_string()])),
            ("build variable", other(|t| t.build = vec!["OKKAMA_IR_PASSES=\"none\"".to_string()])),
        ] {
            assert!(!up_to_date(&project, &now, &inputs).unwrap(), "another {what}");
        }

        // What a #1040 blade wrote for this very output and these inputs.
        let output = crate::install::checksum_of(&temp.0, std::slice::from_ref(&project.output)).unwrap();
        std::fs::write(project.stamp_path(), format!("inputs {inputs}\noutput {output}\n")).unwrap();
        assert!(!up_to_date(&project, &built, &inputs).unwrap(), "a stamp of the old format");
        let current = stamp(&project, &built, &inputs).unwrap();
        assert!(current.starts_with("blade-stamp 2\nokkama 0.6.5\nbinary sha256:"), "{current}");
        assert!(current.ends_with(&format!("inputs {inputs}\noutput {output}\n")), "{current}");
    }

    /// #1087: with no import directory and no build variable, the stamp is the
    /// one #1080 wrote, so an output it stamped is not built again for
    /// nothing; each import directory and build variable is a line of its own,
    /// in the order `okkama env` gives.
    #[test]
    fn a_stamp_names_import_directories_and_build_variables_in_order() {
        let temp = Temp::new("imports");
        temp.write("src/main.ok", "module app;\n");
        temp.write("target/app.spb", "built");
        let project = Project::parse(&temp.0, "[module]\nname = \"app\"\n").unwrap();
        let inputs = fingerprint(&project).unwrap();
        let output = crate::install::checksum_of(&temp.0, std::slice::from_ref(&project.output)).unwrap();
        let plain = toolchain();
        let b = "b".repeat(64);
        let five = "5".repeat(64);
        assert_eq!(
            stamp(&project, &plain, &inputs).unwrap(),
            format!("blade-stamp 2\nokkama 0.6.5\nbinary sha256:{b}\nstd sha256:{five}\ninputs {inputs}\noutput {output}\n")
        );
        let (one, two) = (format!("sha256:{}", "1".repeat(64)), format!("sha256:{}", "2".repeat(64)));
        let mut ordered = toolchain();
        ordered.imports = vec![one.clone(), two.clone()];
        ordered.build = vec!["OKKAMA_IR_PASSES=\"none\"".to_string()];
        let text = stamp(&project, &ordered, &inputs).unwrap();
        assert!(text.contains(&format!("std sha256:{five}\nimport {one}\nimport {two}\nbuild OKKAMA_IR_PASSES=\"none\"\ninputs ")), "{text}");
        let mut swapped = ordered.clone();
        swapped.imports.reverse();
        std::fs::write(project.stamp_path(), text).unwrap();
        assert!(up_to_date(&project, &ordered, &inputs).unwrap());
        assert!(!up_to_date(&project, &swapped, &inputs).unwrap(), "the search order counts");
    }
}
