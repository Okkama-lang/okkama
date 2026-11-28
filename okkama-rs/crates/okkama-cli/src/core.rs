//! The Sparrow core (`sparrow-core`) behind `okkama exec`, `okkama run`
//! and the OCS adapter. Process workers run the program's
//! bytecode in `okkama --worker FILE.spb`; a program compiled in memory gets
//! a temporary file for them when it spawns workers.
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Does the program start process workers (`worker.spawn`)?
pub(crate) fn spawns_workers(program: &okkama_sparrow::Program) -> bool {
    program
        .functions
        .iter()
        .flat_map(|function| &function.code)
        .any(|instruction| matches!(instruction, okkama_sparrow::Instruction::WorkerSpawn { .. }))
}

/// Bytecode written for the workers of one run; removed when dropped.
#[derive(Debug)]
pub(crate) struct WorkerFile(PathBuf);

impl WorkerFile {
    pub(crate) fn write(bytes: &[u8]) -> std::io::Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path = std::env::temp_dir().join(format!("okkama-worker-{}-{nanos}.spb", std::process::id()));
        // A new file only this user reads: a file or link planted under the
        // name is refused, not written through (#1017).
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        // Made: from here on it is removed when dropped, a failed write too.
        let made = Self(path);
        std::io::Write::write_all(&mut file, bytes)?;
        Ok(made)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for WorkerFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// `okkama --worker FILE.spb` for the workers of `path` (the core inserts
/// `--worker` after the executable).
pub(crate) fn worker_command(path: &Path, limits: &sparrow_core::budget::Limits) -> Vec<OsString> {
    let exe = std::env::current_exe().unwrap_or_else(|_| "okkama".into());
    let file = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut command = vec![exe.into_os_string()];
    command.extend(limits.extension_options());
    command.push(file.into_os_string());
    command
}

/// The worker command of an in-memory program, with the file it needs.
pub(crate) fn workers_for(
    program: &okkama_sparrow::Program,
    bytes: &[u8],
    limits: &sparrow_core::budget::Limits,
) -> Result<(Vec<OsString>, Option<WorkerFile>), String> {
    if !spawns_workers(program) {
        return Ok((Vec::new(), None));
    }
    let file = WorkerFile::write(bytes).map_err(|e| format!("cannot write worker bytecode: {e}"))?;
    Ok((worker_command(file.path(), limits), Some(file)))
}

/// The child side of a process worker: the request on stdin, the response
/// on stdout; it never runs main.
pub(crate) fn worker(args: &[String]) -> Result<(), String> {
    let mut limits = sparrow_core::budget::Limits::default();
    let mut args = args;
    while let [option, rest @ ..] = args {
        match option.as_str() {
            "--no-native" => { limits.no_native = true; args = rest; }
            "--ext" => {
                let Some((path, rest)) = rest.split_first() else { return Err("--ext needs a path".into()); };
                limits.option(option, path)?;
                args = rest;
            }
            _ => break,
        }
    }
    let [path] = args else {
        return Err("--worker needs one bytecode file".into());
    };
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read `{path}`: {e}"))?;
    sparrow_core::serve_worker_limited(bytes, worker_command(Path::new(path), &limits), limits).map_err(|e| e.0)
}

/// The report of an uncaught failure, ending with a newline.
pub(crate) fn report(error: sparrow_core::Error) -> String {
    let mut text = error.0;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    /// #1017: the workers' bytecode is a new file only this user reads, and
    /// it goes when dropped.
    #[test]
    #[cfg(unix)]
    fn the_worker_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let file = super::WorkerFile::write(b"spb").expect("a file");
        let path = file.path().to_path_buf();
        assert_eq!(std::fs::read(&path).expect("read"), b"spb");
        assert_eq!(std::fs::metadata(&path).expect("metadata").permissions().mode() & 0o777, 0o600);
        drop(file);
        assert!(!path.exists());
    }
}
