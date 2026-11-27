//! What the compiler asks of the machine it runs on besides the sources and the loader its caller hands it
//! (#1116): a clock for `OKKAMA_PROFILE_REGISTERS`, and the files that the development switches
//! `OKKAMA_DUMP_INVALID_IR` and `OKKAMA_OWNERSHIP_REPORT` name. A wasm32 build (the playground, #1078) has
//! neither: its clock reads zero and its development outputs are dropped, so no compiler path reaches a file
//! system or a clock call there. File identity is the loader's ([`crate::lowering::ModuleLoader::identity`]).
//!
//! Only this module and the language server's std directory (`ide/std_disk.rs`, native only) touch the file
//! system or the clock; a test below holds every other source file of the crate to that.

/// A reading of the monotonic clock for the development profiles; on wasm32 every reading is the same.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    #[cfg(not(target_arch = "wasm32"))]
    at: std::time::Instant,
}

impl Clock {
    pub fn now() -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            at: std::time::Instant::now(),
        }
    }

    /// Nanoseconds from `earlier` to this reading (zero on wasm32).
    pub fn since(self, earlier: Clock) -> u128 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.at.duration_since(earlier.at).as_nanos()
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = earlier;
            0
        }
    }

    /// Nanoseconds since this reading (zero on wasm32).
    pub fn elapsed(self) -> u128 {
        Self::now().since(self)
    }
}

/// Writes the development output `text` (built only then) to the file the environment variable `switch` names,
/// when it names one. Failures are ignored: the output is a diagnostic aid, never part of a result. A wasm32
/// build has no environment and no files, and drops it.
pub fn dev_output(switch: &str, text: impl FnOnce() -> String) {
    #[cfg(not(target_arch = "wasm32"))]
    write_to(std::env::var_os(switch), text);
    #[cfg(target_arch = "wasm32")]
    let _ = (switch, text);
}

#[cfg(not(target_arch = "wasm32"))]
fn write_to(path: Option<std::ffi::OsString>, text: impl FnOnce() -> String) {
    if let Some(path) = path {
        let _ = std::fs::write(path, text());
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// Calls that reach the file system, the clock or the process.
    const HOST_CALLS: &[&str] = &[
        "std::fs", "fs::read", "fs::write", "fs::metadata", "File::open", "File::create", "OpenOptions", "Instant::now", "time::Instant",
        "SystemTime", "canonicalize", "read_dir", "temp_dir", "current_dir", "std::process", "std::net",
    ];

    /// The files allowed to make them: this module, which has a wasm32 half, and the std directory of the
    /// language server, which a wasm32 build does not compile.
    const NATIVE: &[&str] = &["host.rs", "ide/std_disk.rs"];

    fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                out.push(path);
            }
        }
    }

    /// The code of a source file without its comments and its test module (test modules end a file).
    fn code(text: &str) -> String {
        let lines: Vec<&str> = text.lines().collect();
        let end = lines
            .windows(2)
            .position(|pair| pair[0].trim() == "#[cfg(test)]" && pair[1].trim_start().starts_with("mod "))
            .unwrap_or(lines.len());
        lines[..end].iter().map(|line| line.split("//").next().unwrap_or_default()).collect::<Vec<_>>().join("\n")
    }

    /// #1116: nothing in the compiler outside [`NATIVE`] reaches a file system, clock or process call, so the
    /// wasm32 build (the playground) cannot; `cargo check --target wasm32-unknown-unknown` (CI's Playground job)
    /// checks that the native files stay out of it.
    #[test]
    fn the_host_is_reached_only_through_this_module() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        sources(&root, &mut files);
        let mut found = Vec::new();
        for file in &files {
            let relative = file.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
            let name = file.file_name().unwrap().to_string_lossy();
            if NATIVE.contains(&relative.as_str()) || name == "tests.rs" || name.ends_with("_tests.rs") {
                continue;
            }
            let code = code(&std::fs::read_to_string(file).unwrap());
            for call in HOST_CALLS.iter().filter(|call| code.contains(*call)) {
                found.push(format!("{relative}: {call}"));
            }
        }
        assert!(files.len() > 60, "the compiler's sources: {}", files.len());
        assert!(found.is_empty(), "host calls outside host.rs:\n{}", found.join("\n"));
        // The language server's std directory is compiled for native targets only.
        let ide = std::fs::read_to_string(root.join("ide/mod.rs")).unwrap();
        assert!(ide.contains("#[cfg(not(target_arch = \"wasm32\"))]\nmod std_disk;"), "ide/mod.rs: std_disk is native only");
    }

    #[test]
    fn the_scan_sees_calls_and_skips_comments_and_test_modules() {
        let text = "fn a() { std::fs::write(\"x\", \"\"); }\n// Instant::now in a comment\n#[cfg(test)]\nmod tests { fn b() { std::time::Instant::now(); } }\n";
        let code = code(text);
        assert!(code.contains("std::fs::write"));
        assert!(!code.contains("Instant"));
    }

    #[test]
    fn a_dev_output_is_written_only_when_its_switch_names_a_file() {
        let file = std::env::temp_dir().join(format!("okkama-host-dev-output-{}", std::process::id()));
        super::write_to(None, || unreachable!("built only when the switch names a file"));
        super::dev_output("OKKAMA_HOST_TEST_UNSET_1116", || unreachable!("an unset switch writes nothing"));
        super::write_to(Some(file.clone().into_os_string()), || "report".to_string());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "report");
        std::fs::remove_file(file).unwrap();
    }
}
