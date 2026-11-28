//! The toolchain behind a build (#1080, #1087): the `okkama` Blade runs, its
//! version, the std it compiles against, the system import directories
//! (`OKKAMA_IMPORT_DIRS`) and the environment that changes what it builds, as
//! that `okkama` itself names them in `okkama env`. A build's stamp records
//! the version, a SHA-256 of the binary and of each tree, and the build
//! variables, so a new `okkama`, an edited std or import directory, or another
//! `OKKAMA_IR_PASSES` builds again even when nothing in the project changed.
//!
//! Reading a 20 MB binary and a 7 MB std on every `blade run` would cost more
//! than the rest of a run that builds nothing, so the digest of each file is
//! remembered beside the stamp (`OUTPUT.blade-cache`), keyed by what the file
//! system says of the file: its path, size, modification time and, on Unix,
//! its device, inode and status-change time. A file whose key differs is read
//! again; the key decides only whether to read, never whether to build.
//!
//! `okkama env` takes a few milliseconds to start, as long as listing std, so
//! the two overlap: while it runs, Blade fingerprints the project and lists
//! the trees the cache last saw on another thread, and uses a listing only if
//! `okkama env` names the same directory.

use std::collections::HashMap;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// The toolchain of a build, by content: the lines it adds to the stamp.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Toolchain {
    /// The first line of `okkama env`: `okkama 0.6.5`.
    pub version: String,
    /// The digest of the `okkama` binary.
    pub binary: String,
    /// The digest of the std tree (which files, and the bytes of each), or
    /// `missing`.
    pub std: String,
    /// The digest of each import directory in search order, or `missing`.
    pub imports: Vec<String>,
    /// The build variables `okkama env` names, as it spells them:
    /// `OKKAMA_IR_PASSES="none"`.
    pub build: Vec<String>,
}

impl std::fmt::Display for Toolchain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // With no import directory and no build variable set, the stamp is
        // the one of #1080: nothing it did not record has changed.
        write!(f, "{}\nbinary {}\nstd {}\n", self.version, self.binary, self.std)?;
        for import in &self.imports {
            writeln!(f, "import {import}")?;
        }
        for variable in &self.build {
            writeln!(f, "build {variable}")?;
        }
        Ok(())
    }
}

/// Where the toolchain is, as `okkama env` says. A path is `None` when
/// `okkama env` reports it missing.
#[derive(Debug, PartialEq, Eq)]
struct Located {
    version: String,
    binary: PathBuf,
    std: Option<PathBuf>,
    imports: Vec<Option<PathBuf>>,
    build: Vec<String>,
}

/// Listings of trees, under way on another thread.
type Listings = JoinHandle<Vec<(PathBuf, Result<Vec<Listed>, String>)>>;

/// The fingerprint of the toolchain `okkama` builds with, under way:
/// [`Probe::start`] it, do other work, then [`Probe::finish`] it.
pub(crate) struct Probe {
    okkama: PathBuf,
    /// Where `okkama env` runs, as the build does: a relative path it names
    /// is relative to it.
    root: PathBuf,
    cache: PathBuf,
    /// `okkama env`, running.
    env: std::io::Result<Child>,
    digests: Digests,
    /// The trees the cache last saw, being listed.
    listings: Option<Listings>,
}

impl Probe {
    /// Start `okkama env` in `root` (where the build runs) and the listing of
    /// the trees the cache at `cache` last saw.
    pub(crate) fn start(okkama: &Path, root: &Path, cache: &Path) -> Probe {
        // Asking `okkama` where its parts are keeps one place that resolves
        // its std (`OKKAMA_STDLIB_DIR`, the installation's, the repository's),
        // its import directories and the variables that change a build.
        let env = Command::new(okkama)
            .arg("env")
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let digests = Digests::load(cache);
        let guesses = digests.trees.clone();
        let listings = (!guesses.is_empty()).then(|| {
            std::thread::spawn(move || {
                guesses
                    .into_iter()
                    .map(|dir| {
                        let files = list(&dir);
                        (dir, files)
                    })
                    .collect()
            })
        });
        let (okkama, root, cache) = (okkama.to_path_buf(), root.to_path_buf(), cache.to_path_buf());
        Probe { okkama, root, cache, env, digests, listings }
    }

    /// The toolchain, now. The digests come from the cache where the files
    /// are as they were; the cache is written back when it changed (best
    /// effort: a cache that cannot be written is read again next time).
    pub(crate) fn finish(self) -> Result<Toolchain, String> {
        let Probe { okkama, root, cache, env, mut digests, listings } = self;
        let output =
            env.and_then(Child::wait_with_output).map_err(|error| format!("cannot run {}: {error}", okkama.display()));
        let mut guessed: HashMap<PathBuf, Result<Vec<Listed>, String>> =
            listings.and_then(|listings| listings.join().ok()).unwrap_or_default().into_iter().collect();
        let located = located(&okkama, output?)?;
        let binary = digests.file(&root.join(&located.binary))?;
        let mut trees = Vec::new();
        let mut tree = |dir: &Option<PathBuf>| -> Result<String, String> {
            let Some(dir) = dir else { return Ok("missing".to_string()) };
            let dir = root.join(dir);
            let files = match guessed.remove(&dir) {
                Some(listed) => listed?,
                None => list(&dir)?,
            };
            trees.push(dir);
            digests.tree(files)
        };
        let std = tree(&located.std)?;
        let imports = located.imports.iter().map(&mut tree).collect::<Result<Vec<_>, _>>()?;
        digests.trees = trees;
        digests.save(&cache);
        Ok(Toolchain { version: located.version, binary, std, imports, build: located.build })
    }
}

/// What `okkama env` printed, read.
fn located(okkama: &Path, output: Output) -> Result<Located, String> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("`{} env` failed ({}): {}", okkama.display(), output.status, stderr.trim()));
    }
    parse_env(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| format!("`{} env` did not name its version, binary and std", okkama.display()))
}

/// `okkama 0.6.5`, then `key  value` lines, of which `binary`, `std`, each
/// `import` and each `build` count; a path `okkama` cannot find ends in
/// ` (missing)` or is `(not found)`. A `build` value is kept as written.
fn parse_env(text: &str) -> Option<Located> {
    let mut lines = text.lines();
    let version = lines.next().filter(|line| line.starts_with("okkama "))?.trim_end().to_string();
    let (mut binary, mut std, mut imports, mut build) = (None, None, Vec::new(), Vec::new());
    for line in lines {
        let Some((key, value)) = line.split_once(' ') else { continue };
        let value = value.trim_start();
        let path = (value != "(not found)" && !value.ends_with(" (missing)")).then(|| PathBuf::from(value));
        match key {
            "binary" => binary = Some(path),
            "std" => std = Some(path),
            "import" => imports.push(path),
            "build" => build.push(value.to_string()),
            _ => {}
        }
    }
    Some(Located { version, binary: binary??, std: std?, imports, build })
}

/// What the file system says of a file: when all of it is the same, the
/// contents are taken to be the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    size: u64,
    /// Nanoseconds since the epoch.
    modified: i128,
    /// The status-change time in nanoseconds (Unix; 0 elsewhere): it moves
    /// with every write, rename or `cp -p`, and no program can set it back.
    changed: i128,
    inode: u64,
    device: u64,
}

impl Key {
    fn of(meta: &Metadata) -> Key {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let nanos = |seconds: i64, nanos: i64| i128::from(seconds) * 1_000_000_000 + i128::from(nanos);
            Key {
                size: meta.len(),
                modified: nanos(meta.mtime(), meta.mtime_nsec()),
                changed: nanos(meta.ctime(), meta.ctime_nsec()),
                inode: meta.ino(),
                device: meta.dev(),
            }
        }
        #[cfg(not(unix))]
        {
            let modified = meta.modified().map(since_epoch).unwrap_or(0);
            Key { size: meta.len(), modified, changed: 0, inode: 0, device: 0 }
        }
    }

    /// Too recent to trust: a file written again within the clock's
    /// granularity of the time recorded would keep its key (git's "racily
    /// clean" entries). Such a digest is used but not remembered.
    fn racy(&self, now: i128) -> bool {
        let window = RACY.as_nanos() as i128;
        now - self.modified < window || now - self.changed < window
    }
}

const RACY: Duration = Duration::from_secs(2);

fn since_epoch(time: SystemTime) -> i128 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_nanos() as i128,
        Err(before) => -(before.duration().as_nanos() as i128),
    }
}

/// The first line of the cache file; another first line is no cache.
const CACHE_FORMAT: &str = "blade-cache 2";

/// File digests by absolute path: those the cache file had, and those this
/// run used (the only ones written back), with the trees (std, then the
/// import directories) the run listed.
struct Digests {
    known_trees: Vec<PathBuf>,
    known: HashMap<PathBuf, (Key, String)>,
    trees: Vec<PathBuf>,
    used: HashMap<PathBuf, (Key, String)>,
    now: i128,
}

impl Digests {
    fn load(cache: &Path) -> Digests {
        let (trees, known) = std::fs::read_to_string(cache).ok().map(|text| parse_cache(&text)).unwrap_or_default();
        let now = since_epoch(SystemTime::now());
        Digests { known_trees: trees.clone(), known, trees, used: HashMap::new(), now }
    }

    fn save(&self, cache: &Path) {
        let remembered: HashMap<_, _> =
            self.used.iter().filter(|(_, (key, _))| !key.racy(self.now)).map(|(p, v)| (p.clone(), v.clone())).collect();
        if remembered == self.known && self.trees == self.known_trees {
            return;
        }
        let mut text = format!("{CACHE_FORMAT}\n");
        for tree in self.trees.iter().filter_map(|tree| tree.to_str()).filter(|tree| !tree.contains('\n')) {
            text.push_str(&format!("tree {tree}\n"));
        }
        let mut entries: Vec<_> = remembered.iter().collect();
        entries.sort_unstable_by_key(|(path, _)| *path);
        for (path, (key, digest)) in entries {
            let Some(path) = path.to_str().filter(|path| !path.contains('\n')) else { continue };
            let Key { size, modified, changed, inode, device } = key;
            text.push_str(&format!("{size} {modified} {changed} {inode} {device} {digest} {path}\n"));
        }
        // Written aside and renamed: a blade reading it meanwhile sees the
        // old cache or the new one, never half of one.
        let mut aside = cache.as_os_str().to_owned();
        aside.push(format!(".{}", std::process::id()));
        let aside = PathBuf::from(aside);
        if let Some(dir) = cache.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::write(&aside, text).is_err() || std::fs::rename(&aside, cache).is_err() {
            let _ = std::fs::remove_file(&aside);
        }
    }

    /// The digest of the file at `path` (links followed).
    fn file(&mut self, path: &Path) -> Result<String, String> {
        let meta = std::fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
        self.digest(path, &meta)
    }

    fn digest(&mut self, path: &Path, meta: &Metadata) -> Result<String, String> {
        // The key is taken before the bytes are read: a file changed between
        // the two is remembered under its old key and read again next time.
        let key = Key::of(meta);
        let digest = match self.known.get(path) {
            Some((known, digest)) if *known == key => digest.clone(),
            _ => {
                let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
                hex(&Sha256::digest(&bytes))
            }
        };
        self.used.insert(path.to_path_buf(), (key, digest.clone()));
        Ok(digest)
    }

    /// The digest of a tree from its [`list`]ing: every file by its path in
    /// the tree and its digest. A file added, removed or renamed changes it as
    /// an edit does.
    fn tree(&mut self, files: Vec<Listed>) -> Result<String, String> {
        let mut hasher = Sha256::new();
        for Listed { name, path, meta } in files {
            let digest = self.digest(&path, &meta)?;
            hasher.update(name.as_bytes());
            hasher.update([0]);
            hasher.update(digest.as_bytes());
            hasher.update(b"\n");
        }
        Ok(hex(&hasher.finalize()))
    }
}

/// A file of a tree: its path in the tree (`/` separators), where it is,
/// and what the file system says of it.
struct Listed {
    name: String,
    path: PathBuf,
    meta: Metadata,
}

/// Every file of the tree at `root` outside hidden directories (links
/// followed), sorted by its path in the tree. A link back to a directory the
/// walk is already inside is a loop and is skipped (#1140): the files under
/// it are listed once, by their path outside the loop. Only ancestors count,
/// so two links to one directory elsewhere list it under both names whatever
/// order the directory reads in.
fn list(root: &Path) -> Result<Vec<Listed>, String> {
    let mut files = Vec::new();
    let top = std::fs::metadata(root).map_err(|error| format!("{}: {error}", root.display()))?;
    let mut stack = vec![(root.to_path_buf(), directory(&top).into_iter().collect::<Vec<_>>())];
    while let Some((dir, ancestors)) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("{}: {error}", dir.display()))?;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let link = entry.file_type().is_ok_and(|kind| kind.is_symlink());
            let meta = if link { std::fs::metadata(&path) } else { entry.metadata() }
                .map_err(|error| format!("{}: {error}", path.display()))?;
            if meta.is_dir() {
                let identity = directory(&meta);
                if identity.is_some_and(|identity| ancestors.contains(&identity)) {
                    continue;
                }
                stack.push((path, ancestors.iter().copied().chain(identity).collect()));
            } else if meta.is_file() {
                let relative = path.strip_prefix(root).unwrap_or(&path);
                let parts: Vec<String> =
                    relative.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
                files.push(Listed { name: parts.join("/"), path, meta });
            }
        }
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

/// What tells one directory from another however it is reached: its device
/// and inode on Unix; elsewhere nothing, and loops are not looked for.
fn directory(meta: &Metadata) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

/// The cache file: the trees it last saw, and the digests by path; a line
/// that does not parse is dropped.
fn parse_cache(text: &str) -> (Vec<PathBuf>, HashMap<PathBuf, (Key, String)>) {
    let mut lines = text.lines().peekable();
    if lines.next() != Some(CACHE_FORMAT) {
        return (Vec::new(), HashMap::new());
    }
    let mut trees = Vec::new();
    while let Some(tree) = lines.next_if(|line| line.starts_with("tree ")).and_then(|line| line.strip_prefix("tree ")) {
        trees.push(PathBuf::from(tree));
    }
    let entry = |line: &str| -> Option<(PathBuf, (Key, String))> {
        let mut fields = line.splitn(7, ' ');
        let mut next = || fields.next();
        let size = next()?.parse().ok()?;
        let modified = next()?.parse().ok()?;
        let changed = next()?.parse().ok()?;
        let inode = next()?.parse().ok()?;
        let device = next()?.parse().ok()?;
        let digest = next()?.to_string();
        let path = next().filter(|path| !path.is_empty())?;
        let well_formed = digest.strip_prefix("sha256:").is_some_and(|hex| hex.len() == 64);
        well_formed.then(|| (PathBuf::from(path), (Key { size, modified, changed, inode, device }, digest)))
    };
    (trees, lines.filter_map(entry).collect())
}

fn hex(digest: &[u8]) -> String {
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary directory removed when dropped.
    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let dir = std::env::temp_dir().join(format!("blade-toolchain-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Temp(std::fs::canonicalize(&dir).unwrap())
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

    /// Digests whose files all count as settled (none racy), so the cache
    /// remembers files the test just wrote.
    fn settled(cache: &Path) -> Digests {
        let mut digests = Digests::load(cache);
        digests.now += 60 * 1_000_000_000;
        digests
    }

    #[test]
    fn okkama_env_names_the_version_binary_and_std() {
        let text = "okkama 0.6.5\nplatform  macos-aarch64\nbinary    /opt/ok ka/bin/okkama\n\
                    std       /opt/ok ka/lib/okkama/std\nicu       (not found)\n";
        let located = parse_env(text).unwrap();
        assert_eq!(located.version, "okkama 0.6.5");
        assert_eq!(located.binary, Path::new("/opt/ok ka/bin/okkama"));
        assert_eq!(located.std.as_deref(), Some(Path::new("/opt/ok ka/lib/okkama/std")));

        assert!(located.imports.is_empty() && located.build.is_empty());

        let missing = parse_env("okkama 0.6.5\nbinary    /b/okkama\nstd       /nowhere/std (missing)\n").unwrap();
        assert_eq!(missing.std, None);

        // #1087: import directories in search order, build variables as written.
        let more = "okkama 0.6.5\nbinary    /b/okkama\nstd       /s\nimport    /i/one\nimport    lib two (missing)\n\
                    import    lib\nbuild     OKKAMA_IR_PASSES=\"fold, dce\"\nbuild     OKKAMA_OWNERSHIP_PROTOTYPE=\"1\"\n";
        let located = parse_env(more).unwrap();
        assert_eq!(located.imports, [Some(PathBuf::from("/i/one")), None, Some(PathBuf::from("lib"))]);
        assert_eq!(located.build, ["OKKAMA_IR_PASSES=\"fold, dce\"", "OKKAMA_OWNERSHIP_PROTOTYPE=\"1\""]);
        for text in [
            "",
            "usage: okkama\n",
            "okkama 0.6.5\nstd /s\n",
            "okkama 0.6.5\nbinary /b\n",
            "okkama 0.6.5\nbinary    (not found)\nstd       /s\n",
        ] {
            assert_eq!(parse_env(text), None, "{text:?}");
        }
    }

    /// The digest of a tree follows which files there are and the bytes of
    /// each; hidden files and times do not count.
    #[test]
    fn a_tree_digest_follows_its_files_and_their_bytes() {
        let temp = Temp::new("tree");
        let stdout = temp.write("std/io/stdout.ok", "module io/stdout;\n");
        temp.write("std/text.ok", "module text;\n");
        let cache = temp.0.join("cache");
        let digest = || Digests::load(&cache).tree(list(&temp.0.join("std")).unwrap()).unwrap();
        let first = digest();
        assert_eq!(digest(), first);

        temp.write("std/.DS_Store", "x");
        temp.write("std/.git/HEAD", "x");
        let old = std::time::SystemTime::now() - Duration::from_secs(7200);
        std::fs::File::options().write(true).open(&stdout).unwrap().set_modified(old).unwrap();
        assert_eq!(digest(), first, "hidden files and times do not count");

        temp.write("std/io/stdout.ok", "module io/stdouT;\n");
        let edited = digest();
        temp.write("std/io/stdout.ok", "module io/stdout;\n");
        assert_eq!(digest(), first);
        std::fs::rename(temp.0.join("std/text.ok"), temp.0.join("std/test.ok")).unwrap();
        let renamed = digest();
        std::fs::remove_file(temp.0.join("std/test.ok")).unwrap();
        let removed = digest();
        temp.write("std/test.ok", "module text;\n");
        temp.write("std/text.ok", "module text;\n");
        let added = digest();
        let all = [&first, &edited, &renamed, &removed, &added];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    /// #1140: a link back to a directory the walk is inside is a loop: the
    /// tree lists without it, and its digest is the digest without the link.
    /// Two links to one directory outside the walk's path list it twice.
    #[cfg(unix)]
    #[test]
    fn a_symlink_loop_in_a_tree_is_skipped() {
        use std::os::unix::fs::symlink;
        let temp = Temp::new("loop");
        temp.write("std/io/stdout.ok", "module io/stdout;\n");
        temp.write("std/text.ok", "module text;\n");
        temp.write("shared/one.ok", "module one;\n");
        let std = temp.0.join("std");
        let cache = temp.0.join("cache");
        let before = Digests::load(&cache).tree(list(&std).unwrap()).unwrap();

        symlink("..", std.join("io/up")).unwrap();
        symlink(".", std.join("here")).unwrap();
        symlink(&std, temp.0.join("shared/std")).unwrap();
        let names = |dir: &Path| list(dir).unwrap().into_iter().map(|file| file.name).collect::<Vec<_>>();
        assert_eq!(names(&std), ["io/stdout.ok", "text.ok"]);
        assert_eq!(Digests::load(&cache).tree(list(&std).unwrap()).unwrap(), before, "a loop adds nothing");

        symlink("../shared", std.join("a")).unwrap();
        symlink("../shared", std.join("b")).unwrap();
        // shared/std leads back into std, which the walk is inside.
        assert_eq!(names(&std), ["a/one.ok", "b/one.ok", "io/stdout.ok", "text.ok"]);
        assert_eq!(names(&temp.0.join("shared")), ["one.ok", "std/io/stdout.ok", "std/text.ok"]);
    }

    /// The cache answers for a file whose key is as remembered, without
    /// reading it; any change of the key reads the file again.
    #[test]
    fn a_cached_digest_holds_while_the_file_is_as_it_was() {
        let temp = Temp::new("cache");
        let file = temp.write("okkama", "binary one");
        let cache = temp.0.join("out.spb.blade-cache");
        let mut digests = settled(&cache);
        let real = digests.file(&file).unwrap();
        assert_eq!(real, hex(&Sha256::digest(b"binary one")));
        digests.trees = vec![temp.0.join("std"), temp.0.join("lib")];
        digests.save(&cache);
        let text = std::fs::read_to_string(&cache).unwrap();
        let trees = format!("tree {}\ntree {}\n", temp.0.join("std").display(), temp.0.join("lib").display());
        assert!(text.starts_with(&format!("{CACHE_FORMAT}\n{trees}")), "{text}");
        assert_eq!(Digests::load(&cache).trees, [temp.0.join("std"), temp.0.join("lib")]);

        // A remembered digest is used as long as the key holds: a planted one
        // shows that the file is not read.
        let planted = format!("sha256:{}", "ab".repeat(32));
        std::fs::write(&cache, text.replace(&real, &planted)).unwrap();
        assert_eq!(settled(&cache).file(&file).unwrap(), planted);

        // The same size, the modification time put back: the status-change
        // time (Unix) still tells, so the file is read again.
        let modified = std::fs::metadata(&file).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&file, "binary two").unwrap();
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(modified).unwrap();
        if cfg!(unix) {
            assert_eq!(settled(&cache).file(&file).unwrap(), hex(&Sha256::digest(b"binary two")));
        }
        std::fs::write(&file, "binary three!").unwrap();
        assert_eq!(settled(&cache).file(&file).unwrap(), hex(&Sha256::digest(b"binary three!")));

        // A file written just now is used but not remembered.
        let mut fresh = Digests::load(&temp.0.join("fresh-cache"));
        fresh.file(&file).unwrap();
        fresh.save(&temp.0.join("fresh-cache"));
        let (_, remembered) = parse_cache(&std::fs::read_to_string(temp.0.join("fresh-cache")).unwrap_or_default());
        assert!(remembered.is_empty(), "{remembered:?}");

        // A damaged cache is no cache.
        std::fs::write(&cache, format!("blade-cache 1\nstd /s\n10 1 2 3 4 {real} {}\n", file.display())).unwrap();
        assert!(Digests::load(&cache).known.is_empty(), "a cache of #1080");
        std::fs::write(&cache, "blade-cache 0\n").unwrap();
        assert!(Digests::load(&cache).known.is_empty());
        std::fs::write(&cache, format!("{CACHE_FORMAT}\n10 1 2 3 4 sha256:short {}\n", file.display())).unwrap();
        assert!(Digests::load(&cache).known.is_empty());
    }

    /// The toolchain through a stand-in `okkama`: its version, its bytes and
    /// its std's files each change the fingerprint; nothing changed, nothing
    /// differs, and a std moved elsewhere is listed where `okkama env` says.
    #[cfg(unix)]
    #[test]
    fn the_toolchain_is_the_version_the_binary_and_the_std() {
        use std::os::unix::fs::PermissionsExt;
        let temp = Temp::new("probe");
        let okkama = temp.0.join("okkama");
        let script = |version: &str, extra: &str| {
            std::fs::write(
                &okkama,
                format!(
                    "#!/bin/sh\n# {extra}\necho \"okkama {version}\"\necho \"binary    $0\"\n\
                     echo \"std       $(cat \"$(dirname \"$0\")/std-dir\")\"\n"
                ),
            )
            .unwrap();
            std::fs::set_permissions(&okkama, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        script("0.6.5", "");
        temp.write("std/io/stdout.ok", "module io/stdout;\n");
        temp.write("other/io/stdout.ok", "module io/stdout; // other\n");
        temp.write("std-dir", &temp.0.join("std").display().to_string());
        let cache = temp.0.join("target/app.spb.blade-cache");
        // A test thread that forks while `script` holds the file open for writing leaves the child holding it
        // until its exec: running the file then fails with ETXTBSY for a moment. Wait it out.
        let probe = || {
            for _ in 0..200 {
                match Probe::start(&okkama, &temp.0, &cache).finish() {
                    Err(error) if error.contains("os error 26") => std::thread::sleep(std::time::Duration::from_millis(10)),
                    result => return result.unwrap(),
                }
            }
            panic!("{} stayed busy", okkama.display())
        };

        let first = probe();
        assert_eq!(first.version, "okkama 0.6.5");
        assert_eq!(first.binary, hex(&Sha256::digest(std::fs::read(&okkama).unwrap())));
        assert_eq!(probe(), first, "nothing changed");

        script("0.6.6", "");
        let version = probe();
        assert_ne!(version.version, first.version);
        script("0.6.5", "rebuilt");
        let binary = probe();
        assert_eq!(binary.version, first.version);
        assert_ne!(binary.binary, first.binary);
        script("0.6.5", "");
        assert_eq!(probe(), first);

        temp.write("std/io/stdout.ok", "module io/stdout; // edited\n");
        let edited = probe();
        assert_ne!(edited.std, first.std);
        temp.write("std/io/stdout.ok", "module io/stdout;\n");
        assert_eq!(probe(), first);

        // The cache last saw std/; `okkama env` now names other/.
        temp.write("std-dir", &temp.0.join("other").display().to_string());
        let moved = probe();
        assert_ne!(moved.std, first.std);
        assert_eq!(moved.std, Digests::load(&temp.0.join("none")).tree(list(&temp.0.join("other")).unwrap()).unwrap());

        temp.write("std-dir", &temp.0.join("gone").display().to_string());
        assert!(Probe::start(&okkama, &temp.0, &cache).finish().is_err(), "a std that cannot be listed");
        let failing = temp.write("failing", "#!/bin/sh\necho broken >&2\nexit 3\n");
        std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o755)).unwrap();
        let error = Probe::start(&failing, &temp.0, &cache).finish().unwrap_err();
        assert!(error.contains("env` failed") && error.contains("broken"), "{error}");
    }

    /// #1087: the import directories through a stand-in `okkama`: each is a
    /// tree by content in search order (an edit, a new directory, another
    /// order, a missing one), a relative one is found from where the build
    /// runs, and a build variable counts as written; nothing changed, nothing
    /// differs.
    #[cfg(unix)]
    #[test]
    fn import_directories_and_build_variables_are_part_of_the_toolchain() {
        use std::os::unix::fs::PermissionsExt;
        let temp = Temp::new("imports");
        let okkama = temp.write(
            "bin/okkama",
            "#!/bin/sh\nhere=\"$(dirname \"$0\")\"\necho \"okkama 0.6.5\"\necho \"binary    $0\"\n\
             echo \"std       $here/../std\"\ncat \"$here/../extra\"\n",
        );
        std::fs::set_permissions(&okkama, std::fs::Permissions::from_mode(0o755)).unwrap();
        temp.write("std/io/stdout.ok", "module io/stdout;\n");
        temp.write("lib/util.ok", "module util;\n");
        temp.write("vendor/json.ok", "module json;\n");
        let project = temp.0.join("project");
        std::fs::create_dir_all(project.join("lib")).unwrap();
        std::fs::write(project.join("lib/local.ok"), "module local;\n").unwrap();
        let cache = temp.0.join("project/target/app.spb.blade-cache");
        let (lib, vendor) = (temp.0.join("lib").display().to_string(), temp.0.join("vendor").display().to_string());
        let probe = |extra: &str| {
            std::fs::write(temp.0.join("extra"), extra).unwrap();
            Probe::start(&okkama, &project, &cache).finish().unwrap()
        };

        let none = probe("");
        assert!(none.imports.is_empty() && none.build.is_empty());
        let one = probe(&format!("import    {lib}\n"));
        assert_eq!(one.imports.len(), 1);
        assert_eq!(probe(&format!("import    {lib}\n")), one, "nothing changed");
        assert_eq!(Digests::load(&cache).trees, [temp.0.join("bin/../std"), temp.0.join("lib")]);
        temp.write("lib/util.ok", "module util; // edited\n");
        let edited = probe(&format!("import    {lib}\n"));
        assert_ne!(edited.imports, one.imports, "an import directory edited");
        let both = probe(&format!("import    {lib}\nimport    {vendor}\n"));
        let swapped = probe(&format!("import    {vendor}\nimport    {lib}\n"));
        assert_eq!(both.imports.len(), 2);
        assert_eq!(both.imports[0], edited.imports[0]);
        assert_eq!(swapped.imports, [both.imports[1].clone(), both.imports[0].clone()], "the order counts");
        let missing = probe(&format!("import    {lib}\nimport    /nowhere (missing)\n"));
        assert_eq!(missing.imports, [edited.imports[0].clone(), "missing".to_string()]);

        // `lib` is the project's lib/, where `okkama build` runs, not ../lib.
        let relative = probe("import    lib\n");
        let local = Digests::load(&temp.0.join("none")).tree(list(&project.join("lib")).unwrap()).unwrap();
        assert_eq!(relative.imports, [local]);

        let passes = probe("build     OKKAMA_IR_PASSES=\"none\"\n");
        assert_eq!(passes.build, ["OKKAMA_IR_PASSES=\"none\""]);
        assert_eq!(Toolchain { build: Vec::new(), ..passes.clone() }, none);
        assert_ne!(probe("build     OKKAMA_IR_PASSES=\"fold\"\n"), passes);
    }
}
