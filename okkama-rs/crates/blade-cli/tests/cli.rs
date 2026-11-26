//! `blade build` and `blade run` through the binary, with a stand-in `okkama`
//! (`BLADE_OKKAMA`) that records its arguments: no compiler is needed.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A temporary tree removed when dropped.
struct Tree(PathBuf);

impl Tree {
    fn new(name: &str) -> Tree {
        let root = std::env::temp_dir().join(format!("blade-cli-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let tree = Tree(std::fs::canonicalize(&root).unwrap());
        // Stand-in okkama: `env` names its version (the file `version`),
        // itself, its std (`OKKAMA_STDLIB_DIR` or `std/` beside it), each
        // `OKKAMA_IMPORT_DIRS` directory and `OKKAMA_IR_PASSES` when set, as
        // okkama does, and is not logged; other calls log their arguments; `build … -o OUT`
        // writes OUT (fails when the entry says FAIL); `exec` prints and
        // exits 7.
        let fake = tree.0.join("okkama");
        std::fs::write(
            &fake,
            r#"#!/bin/sh
here="$(dirname "$0")"
if [ "$1" = env ]; then
    echo "okkama $(cat "$here/version")"
    echo "platform  test"
    echo "binary    $0"
    echo "std       ${OKKAMA_STDLIB_DIR:-$here/std}"
    IFS=:
    for dir in $OKKAMA_IMPORT_DIRS; do
        [ -n "$dir" ] && echo "import    $dir"
    done
    [ -n "${OKKAMA_IR_PASSES+set}" ] && echo "build     OKKAMA_IR_PASSES=\"$OKKAMA_IR_PASSES\""
    exit 0
fi
echo "$@" >> "$here/okkama.log"
case "$1" in
build)
    grep -q FAIL "$2" && { echo "compile error" >&2; exit 1; }
    for last; do :; done
    echo built > "$last" ;;
exec|run)
    echo "ran $* greeting=$GREETING"
    exit 7 ;;
esac
"#,
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        tree.write("version", "0.6.5\n");
        tree.write("std/io/stdout.ok", "module io/stdout;\n");
        tree
    }

    fn write(&self, path: &str, text: &str) {
        let file = self.0.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }

    fn blade(&self, dir: &str, args: &[&str]) -> Output {
        self.blade_with(dir, args, &[])
    }

    fn blade_with(&self, dir: &str, args: &[&str], env: &[(&str, &Path)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_blade"))
            .args(args)
            .current_dir(self.0.join(dir))
            .env("BLADE_OKKAMA", self.0.join("okkama"))
            .env_remove("OKKAMA_STDLIB_DIR")
            .envs(env.iter().copied())
            .output()
            .unwrap()
    }

    /// `blade` with only a stand-in `git` on `PATH`, which leaves a mark.
    fn blade_without_git(&self, dir: &str, args: &[&str]) -> Output {
        let bin = self.0.join("no-git");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("git"), "#!/bin/sh\ntouch \"$(dirname \"$0\")/ran\"\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_blade"))
            .args(args)
            .current_dir(self.0.join(dir))
            .env("PATH", &bin)
            .output()
            .unwrap();
        assert!(!bin.join("ran").exists(), "git ran: {}", text(&output.stderr));
        output
    }

    /// The okkama calls so far, one per line; the log starts empty again.
    fn calls(&self) -> Vec<String> {
        let log = self.0.join("okkama.log");
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let _ = std::fs::remove_file(&log);
        text.lines().map(|line| line.replace(&format!("{}/", self.0.display()), "")).collect()
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Move the modification time of `file` back an hour.
fn age(file: &Path) {
    let time = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options().write(true).open(file).unwrap().set_modified(time).unwrap();
}

#[test]
fn a_new_project_builds_and_runs_with_no_flags() {
    let tree = Tree::new("new");
    assert!(tree.blade(".", &["new", "app"]).status.success());
    let built = tree.blade("app", &["build"]);
    assert!(built.status.success(), "{}", text(&built.stderr));
    assert_eq!(text(&built.stdout), "built target/app.spb\n");
    assert_eq!(tree.calls(), ["build src/main.ok -o target/app.spb"]);

    // Up to date: run does not build; the program's status is Blade's.
    let ran = tree.blade("app", &["run", "--", "a", "--help"]);
    assert_eq!(ran.status.code(), Some(7), "{}", text(&ran.stderr));
    assert_eq!(tree.calls(), ["exec app/target/app.spb a --help"]);

    // A changed source is built again before running.
    tree.write("app/src/extra.ok", "module app;\n");
    assert_eq!(tree.blade("app", &["run"]).status.code(), Some(7));
    assert_eq!(tree.calls(), ["build src/main.ok -o target/app.spb", "exec app/target/app.spb"]);
}

/// #1040: `blade run` builds again whenever the sources are not the ones the
/// output was built from, whatever their times say: a deleted source, one
/// replaced by an older file (`cp -p`), a renamed one, a replaced output, a
/// failed build; a time alone builds nothing.
#[test]
fn run_never_runs_a_program_built_from_other_sources() {
    let tree = Tree::new("stale");
    assert!(tree.blade(".", &["new", "app"]).status.success());
    tree.write("app/src/greet.ok", "module app/greet; // hello\n");
    let run = || {
        let ran = tree.blade("app", &["run"]);
        assert_eq!(ran.status.code(), Some(7), "{}", text(&ran.stderr));
        tree.calls()
    };
    let (build, exec) = ("build src/main.ok -o target/app.spb", "exec app/target/app.spb");
    assert_eq!(run(), [build, exec]);
    assert_eq!(run(), [exec]);
    assert!(tree.0.join("app/target/app.spb.blade-stamp").is_file());

    std::fs::remove_file(tree.0.join("app/src/greet.ok")).unwrap();
    assert_eq!(run(), [build, exec], "a deleted source");
    assert_eq!(run(), [exec]);

    // An older file of the same length copied over a source, its time kept.
    tree.write("older.ok", "module app; // HELLO FROM OKKAMA\n");
    tree.write("app/src/main.ok", "module app; // hello from okkama\n");
    assert_eq!(run(), [build, exec]);
    age(&tree.0.join("older.ok"));
    let copied = Command::new("cp").args(["-p", "older.ok", "app/src/main.ok"]).current_dir(&tree.0).output().unwrap();
    assert!(copied.status.success());
    assert_eq!(run(), [build, exec], "a source replaced by an older file");

    age(&tree.0.join("app/src/main.ok"));
    age(&tree.0.join("app/target/app.spb"));
    assert_eq!(run(), [exec], "a time alone builds nothing");

    std::fs::rename(tree.0.join("app/src/main.ok"), tree.0.join("app/src/other.ok")).unwrap();
    std::fs::copy(tree.0.join("app/src/other.ok"), tree.0.join("app/src/main.ok")).unwrap();
    assert_eq!(run(), [build, exec], "a source added");

    tree.write("app/target/app.spb", "something else");
    assert_eq!(run(), [build, exec], "the output was replaced");

    // A failed build leaves no stamp: the old output never runs.
    tree.write("app/src/main.ok", "module app; FAIL\n");
    let failed = tree.blade("app", &["run"]);
    assert_eq!(failed.status.code(), Some(2));
    assert!(text(&failed.stderr).contains("build failed"), "{}", text(&failed.stderr));
    assert_eq!(tree.calls(), [build]);
    assert!(!tree.0.join("app/target/app.spb.blade-stamp").exists());
    std::fs::copy(tree.0.join("older.ok"), tree.0.join("app/src/main.ok")).unwrap();
    assert_eq!(run(), [build, exec]);
}

/// #1080: `blade run` builds again when the toolchain is not the one that
/// built the output: another `okkama` version, other bytes of the binary, a
/// std file edited, added or removed, another std (`OKKAMA_STDLIB_DIR`), a
/// stamp written before #1080; the same toolchain, its files only touched,
/// builds nothing.
#[test]
fn run_builds_again_for_another_toolchain() {
    let tree = Tree::new("toolchain");
    assert!(tree.blade(".", &["new", "app"]).status.success());
    let run_with = |env: &[(&str, &Path)]| {
        let ran = tree.blade_with("app", &["run"], env);
        assert_eq!(ran.status.code(), Some(7), "{}", text(&ran.stderr));
        tree.calls()
    };
    let run = || run_with(&[]);
    let (build, exec) = ("build src/main.ok -o target/app.spb", "exec app/target/app.spb");
    assert_eq!(run(), [build, exec]);
    assert_eq!(run(), [exec], "the same toolchain");
    assert!(tree.0.join("app/target/app.spb.blade-cache").is_file());

    tree.write("version", "0.6.6\n");
    assert_eq!(run(), [build, exec], "another version");
    assert_eq!(run(), [exec]);

    // Other bytes, the same version: a rebuilt okkama.
    let fake = tree.0.join("okkama");
    let script = std::fs::read_to_string(&fake).unwrap();
    std::fs::write(&fake, format!("{script}# rebuilt\n")).unwrap();
    assert_eq!(run(), [build, exec], "another binary");
    assert_eq!(run(), [exec]);

    tree.write("std/io/stdout.ok", "module io/stdout; // edited\n");
    assert_eq!(run(), [build, exec], "a std file edited");
    tree.write("std/text.ok", "module text;\n");
    assert_eq!(run(), [build, exec], "a std file added");
    std::fs::remove_file(tree.0.join("std/text.ok")).unwrap();
    assert_eq!(run(), [build, exec], "a std file removed");
    age(&tree.0.join("std/io/stdout.ok"));
    age(&fake);
    assert_eq!(run(), [exec], "times alone build nothing");

    tree.write("other/io/stdout.ok", "module io/stdout; // another std\n");
    let other = tree.0.join("other");
    assert_eq!(run_with(&[("OKKAMA_STDLIB_DIR", &other)]), [build, exec], "OKKAMA_STDLIB_DIR");
    assert_eq!(run_with(&[("OKKAMA_STDLIB_DIR", &other)]), [exec]);
    assert_eq!(run(), [build, exec], "back to the std beside okkama");

    // A stamp of #1040 (the inputs and the output alone) for this very
    // output: the toolchain that made it is unknown, so it builds.
    let stamp = tree.0.join("app/target/app.spb.blade-stamp");
    let current = std::fs::read_to_string(&stamp).unwrap();
    let old: String = current
        .lines()
        .filter(|line| line.starts_with("inputs ") || line.starts_with("output "))
        .map(|line| format!("{line}\n"))
        .collect();
    assert_eq!(old.lines().count(), 2, "{current}");
    std::fs::write(&stamp, old).unwrap();
    assert_eq!(run(), [build, exec], "a stamp from before #1080");
    assert_eq!(run(), [exec]);
}

/// #1087: `blade run` builds again when what `okkama build` would import from
/// `OKKAMA_IMPORT_DIRS` changed (a file edited or added, a directory added or
/// reordered) or a build variable did (`OKKAMA_IR_PASSES` set, changed,
/// unset); an unrelated variable, or nothing changed, builds nothing.
#[test]
fn run_builds_again_for_other_imports_or_build_variables() {
    let tree = Tree::new("imports");
    assert!(tree.blade(".", &["new", "app"]).status.success());
    tree.write("lib/util.ok", "module util;\n");
    tree.write("vendor/json.ok", "module json;\n");
    let (lib, vendor) = (tree.0.join("lib"), tree.0.join("vendor"));
    let dirs = |dirs: &[&Path]| {
        std::env::join_paths(dirs).unwrap()
    };
    let run_with = |env: &[(&str, &std::ffi::OsStr)]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_blade"));
        command
            .arg("run")
            .current_dir(tree.0.join("app"))
            .env("BLADE_OKKAMA", tree.0.join("okkama"))
            .env_remove("OKKAMA_STDLIB_DIR")
            .env_remove("OKKAMA_IMPORT_DIRS")
            .env_remove("OKKAMA_IR_PASSES");
        let ran = command.envs(env.iter().copied()).output().unwrap();
        assert_eq!(ran.status.code(), Some(7), "{}", text(&ran.stderr));
        tree.calls()
    };
    let (build, exec) = ("build src/main.ok -o target/app.spb", "exec app/target/app.spb");
    assert_eq!(run_with(&[]), [build, exec]);
    assert_eq!(run_with(&[("OKKAMA_UNRELATED", "1".as_ref())]), [exec], "an unrelated variable");

    let one = dirs(&[&lib]);
    assert_eq!(run_with(&[("OKKAMA_IMPORT_DIRS", &one)]), [build, exec], "an import directory");
    assert_eq!(run_with(&[("OKKAMA_IMPORT_DIRS", &one)]), [exec]);
    tree.write("lib/util.ok", "module util; // edited\n");
    assert_eq!(run_with(&[("OKKAMA_IMPORT_DIRS", &one)]), [build, exec], "an imported file edited");
    tree.write("lib/more.ok", "module more;\n");
    assert_eq!(run_with(&[("OKKAMA_IMPORT_DIRS", &one)]), [build, exec], "an imported file added");
    age(&tree.0.join("lib/more.ok"));
    assert_eq!(run_with(&[("OKKAMA_IMPORT_DIRS", &one)]), [exec], "a time alone builds nothing");
    let both = dirs(&[&lib, &vendor]);
    assert_eq!(run_with(&[("OKKAMA_IMPORT_DIRS", &both)]), [build, exec], "a second import directory");
    let swapped = dirs(&[&vendor, &lib]);
    assert_eq!(run_with(&[("OKKAMA_IMPORT_DIRS", &swapped)]), [build, exec], "another search order");
    assert_eq!(run_with(&[("OKKAMA_IMPORT_DIRS", &swapped)]), [exec]);
    assert_eq!(run_with(&[]), [build, exec], "no import directory again");

    assert_eq!(run_with(&[("OKKAMA_IR_PASSES", "none".as_ref())]), [build, exec], "OKKAMA_IR_PASSES set");
    assert_eq!(run_with(&[("OKKAMA_IR_PASSES", "none".as_ref())]), [exec]);
    assert_eq!(run_with(&[("OKKAMA_IR_PASSES", "fold".as_ref())]), [build, exec], "OKKAMA_IR_PASSES changed");
    assert_eq!(run_with(&[]), [build, exec], "OKKAMA_IR_PASSES unset");
    assert_eq!(run_with(&[]), [exec]);
}

/// #1040: a project and its local packages move together: `blade.lock`
/// records `../util`, so the moved project syncs and runs as it is.
#[test]
fn a_project_moves_with_its_local_packages() {
    let tree = Tree::new("moves");
    assert!(tree.blade(".", &["new", "app"]).status.success());
    tree.write("util/blade.toml", "[module]\nname = \"util\"\nversion = \"2.1.0\"\n");
    tree.write("util/src/util.ok", "module util;\n");
    let manifest = std::fs::read_to_string(tree.0.join("app/blade.toml")).unwrap();
    tree.write("app/blade.toml", &format!("{manifest}util = {{ path = \"../util\", version = \"2.1.0\" }}\n"));
    let synced = tree.blade("app", &["sync"]);
    assert!(synced.status.success(), "{}", text(&synced.stderr));
    assert_eq!(text(&synced.stdout), "+ util 2.1.0\n");
    let lock = std::fs::read_to_string(tree.0.join("app/blade.lock")).unwrap();
    assert!(lock.contains("source = \"path+../util\""), "{lock}");
    assert_eq!(tree.blade("app", &["run"]).status.code(), Some(7));
    tree.calls();

    std::fs::create_dir(tree.0.join("moved")).unwrap();
    for name in ["app", "util"] {
        std::fs::rename(tree.0.join(name), tree.0.join("moved").join(name)).unwrap();
    }
    let synced = tree.blade("moved/app", &["sync"]);
    assert!(synced.status.success(), "{}", text(&synced.stderr));
    assert_eq!(text(&synced.stdout), "dependencies are up to date\n");
    assert_eq!(tree.blade("moved/app", &["run"]).status.code(), Some(7));
    assert_eq!(tree.calls(), ["exec moved/app/target/app.spb"], "the same sources: no build");

    tree.write("moved/util/src/util.ok", "module util; // edited\n");
    let updated = tree.blade("moved/app", &["update"]);
    assert!(updated.status.success(), "{}", text(&updated.stderr));
    assert_eq!(text(&updated.stdout), "~ util 2.1.0 (files changed)\n");
}

/// #1039: `blade new` writes only what it creates. It refuses an existing
/// NAME (the project kept as edited), a missing or extra argument (usage, 2),
/// and a NAME its own manifest check rejects; `acme/web` gets the default
/// output `target/web.spb`; no unused `bin/`.
#[test]
fn new_never_overwrites_and_checks_its_name() {
    let tree = Tree::new("new-checks");
    assert!(tree.blade(".", &["new", "keep"]).status.success());
    for path in ["keep/src/main.ok", "keep/blade.toml"] {
        assert!(tree.0.join(path).is_file(), "{path}");
    }
    assert!(tree.0.join("keep/pkg").is_dir());
    assert!(!tree.0.join("keep/bin").exists(), "nothing uses bin/");
    let main = std::fs::read_to_string(tree.0.join("keep/src/main.ok")).unwrap();
    assert!(main.starts_with("module keep;\n"), "{main}");

    tree.write("keep/src/main.ok", "module keep;\n// edited\n");
    tree.write("keep/blade.toml", "[module]\nname = \"keep\"\n\n[dependencies]\nlib = { git = \"x\", version = \"1.0.0\" }\n");
    let again = tree.blade(".", &["new", "keep"]);
    assert_eq!(again.status.code(), Some(2));
    assert_eq!(text(&again.stderr), "blade: keep already exists; `blade new` makes a new project\n");
    assert_eq!(std::fs::read_to_string(tree.0.join("keep/src/main.ok")).unwrap(), "module keep;\n// edited\n");
    assert!(std::fs::read_to_string(tree.0.join("keep/blade.toml")).unwrap().contains("lib = "));
    // An existing plain file is not replaced by a project either.
    tree.write("file", "data");
    assert_eq!(tree.blade(".", &["new", "file"]).status.code(), Some(2));
    assert_eq!(std::fs::read_to_string(tree.0.join("file")).unwrap(), "data");

    for args in [&["new"][..], &["new", "first", "second"]] {
        let usage = tree.blade(".", args);
        assert_eq!(usage.status.code(), Some(2), "{args:?}");
        assert!(text(&usage.stderr).starts_with("blade — Blade command-line tool"), "{args:?}: {}", text(&usage.stderr));
    }
    assert!(!tree.0.join("first").exists() && !tree.0.join("second").exists());

    let invalid = tree.blade(".", &["new", "my-app"]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(text(&invalid.stderr).contains("invalid package name \"my-app\""), "{}", text(&invalid.stderr));
    assert!(!tree.0.join("my-app").exists());

    assert!(tree.blade(".", &["new", "acme/web"]).status.success());
    let manifest = std::fs::read_to_string(tree.0.join("acme/web/blade.toml")).unwrap();
    assert!(manifest.contains("name = \"acme/web\"") && manifest.contains("output = \"target/web.spb\""), "{manifest}");
    let built = tree.blade("acme/web", &["build"]);
    assert!(built.status.success(), "{}", text(&built.stderr));
    assert_eq!(text(&built.stdout), "built target/web.spb\n");
}

#[test]
fn blade_toml_decides_what_builds_and_runs() {
    let tree = Tree::new("settings");
    tree.write("p/main.ok", "module p;\n");
    tree.write("p/more.ok", "module p;\n");
    tree.write(
        "p/blade.toml",
        "[module]\nname = \"p\"\n\n[build]\nentry = [\"main.ok\", \"more.ok\"]\noutput = \"out/p.spb\"\n\n\
         [run]\nargs = [\"x\"]\nenv = { GREETING = \"hi\" }\n",
    );
    let built = tree.blade("p", &["build"]);
    assert!(built.status.success(), "{}", text(&built.stderr));
    assert_eq!(tree.calls(), ["build main.ok more.ok -o out/p.spb"]);
    assert!(tree.0.join("p/out/p.spb").is_file());

    // One engine (owner, 2026-11-20): the old engine key is refused with its reason.
    tree.write("p/blade.toml", "[run]\nengine = \"sparrow\"\n");
    let ran = tree.blade("p", &["run"]);
    assert!(!ran.status.success());
    assert!(text(&ran.stderr).contains("[run] engine is gone: Sparrow is the only engine"));
    assert!(tree.calls().is_empty());
}

#[test]
fn errors_name_the_problem_and_fail() {
    let tree = Tree::new("errors");
    let cases = [
        ("[build]\nentrance = \"main.ok\"\n", "unknown key \"entrance\" in [build]"),
        ("[build]\ntarget = \"sparrow\"\n", "[build] target is gone"),
        ("[build]\nentry = \"missing.ok\"\n", "[build] entry \"missing.ok\" does not exist"),
        ("[build]\nentry = \"bad.ok\"\n", "build failed"),
        ("[build]\nentry = \"main.ok\"\n[dependencies]\nweb = { path = \"../web\" }\n", "dependency \"web\" is not installed"),
    ];
    tree.write("e/main.ok", "module e;\n");
    tree.write("e/bad.ok", "module e; FAIL\n");
    for (settings, expected) in cases {
        tree.write("e/blade.toml", &format!("[module]\nname = \"e\"\n{settings}"));
        for command in ["build", "run"] {
            let output = tree.blade("e", &[command]);
            assert_eq!(output.status.code(), Some(2), "{command} {settings}");
            assert!(text(&output.stderr).contains(expected), "{command}: {}", text(&output.stderr));
        }
    }
    // Installed (locked and in pkg/), the dependency builds with the project.
    tree.write(
        "e/blade.lock",
        "version = 1\n\n[[package]]\nname = \"web\"\nversion = \"1.0.0\"\nsource = \"path+/web\"\nchecksum = \"sha256:00\"\n",
    );
    tree.write("e/pkg/web/router.ok", "module web/router;\n");
    let built = tree.blade("e", &["build"]);
    assert!(built.status.success(), "{}", text(&built.stderr));

    let output = tree.blade(".", &["build"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("no blade.toml"), "{}", text(&output.stderr));
}

/// `blade sync` starts no git when `pkg/` holds what `blade.lock` records
/// (#349): a vendored `pkg/`, and the second sync after the first install.
#[test]
fn sync_runs_no_git_when_pkg_matches_the_lock() {
    let tree = Tree::new("sync");
    tree.write("json/src/json.ok", "module json;\n");
    let json = tree.0.join("json");
    for args in [&["init", "-q"][..], &["add", "-A"], &["commit", "-qm", "1.0.0"], &["tag", "v1.0.0"]] {
        let done = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(&json)
            .output()
            .unwrap();
        assert!(done.status.success(), "{}", text(&done.stderr));
    }
    tree.write(
        "app/blade.toml",
        &format!("[module]\nname = \"app\"\n\n[dependencies]\njson = {{ git = \"{}\", version = \"1.0.0\" }}\n", json.display()),
    );
    let first = tree.blade("app", &["sync"]);
    assert!(first.status.success(), "{}", text(&first.stderr));
    assert_eq!(text(&first.stdout), "+ json 1.0.0\n");
    let again = tree.blade_without_git("app", &["sync"]);
    assert!(again.status.success(), "{}", text(&again.stderr));
    assert_eq!(text(&again.stdout), "dependencies are up to date\n");

    // Committed with the project, pkg/ syncs elsewhere without git.
    let copy = Command::new("cp").args(["-R", "app", "vendored"]).current_dir(&tree.0).output().unwrap();
    assert!(copy.status.success());
    std::fs::remove_dir_all(&json).unwrap();
    let vendored = tree.blade_without_git("vendored", &["sync"]);
    assert!(vendored.status.success(), "{}", text(&vendored.stderr));
    assert_eq!(text(&vendored.stdout), "dependencies are up to date\n");
}

/// The walk-through of #351 through the binary: a new project, a package
/// that brings another, a build, a higher minimum, an update, a removal and
/// a sync; then the help, the version and the exit codes.
#[test]
fn a_project_walks_through_add_update_remove_and_sync() {
    let tree = Tree::new("walk");
    let git = |dir: &Path, args: &[&str]| {
        let done = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(done.status.success(), "git {args:?}: {}", text(&done.stderr));
    };
    let repo = |name: &str| tree.0.join("repos").join(name);
    let release = |name: &str, version: &str, dependencies: &str| {
        tree.write(&format!("repos/{name}/src/{name}.ok"), &format!("module {name}; // {version}\n"));
        if !dependencies.is_empty() {
            tree.write(&format!("repos/{name}/blade.toml"), &format!("[dependencies]\n{dependencies}"));
        }
        if !repo(name).join(".git").exists() {
            git(&repo(name), &["init", "-q"]);
        }
        git(&repo(name), &["add", "-A"]);
        git(&repo(name), &["commit", "-qm", version]);
        git(&repo(name), &["tag", &format!("v{version}")]);
    };
    for version in ["1.2.0", "1.3.0", "1.4.0"] {
        release("json", version, "");
    }
    let json = repo("json").display().to_string();
    release("http", "2.1.0", &format!("json = {{ git = \"{json}\", version = \"1.2.0\" }}\n"));
    let http = repo("http").display().to_string();
    let blade = |args: &[&str]| {
        let output = tree.blade("app", args);
        assert!(output.status.success(), "blade {args:?}: {}", text(&output.stderr));
        text(&output.stdout)
    };
    let locked = || -> Vec<(String, String)> {
        blade::lock::Lock::read(&tree.0.join("app"))
            .unwrap()
            .packages
            .into_iter()
            .map(|(name, package)| (name, package.version.to_string()))
            .collect()
    };
    let pair = |name: &str, version: &str| (name.to_string(), version.to_string());
    let manifest = || std::fs::read_to_string(tree.0.join("app/blade.toml")).unwrap();
    let installed = |name: &str| tree.0.join("app/pkg").join(name).join("src").is_dir();

    assert!(tree.blade(".", &["new", "app"]).status.success());
    assert_eq!(blade(&["add", "http", &http]), "+ http 2.1.0\n+ json 1.2.0\n");
    assert!(manifest().contains(&format!("http = {{ git = \"{http}\", version = \"2.1.0\" }}")), "{}", manifest());
    assert_eq!(locked(), [pair("http", "2.1.0"), pair("json", "1.2.0")]);
    assert!(installed("http") && installed("json"));
    assert_eq!(blade(&["build"]), "built target/app.spb\n");
    assert_eq!(blade(&["add", "json", &json, "1.4.0"]), "~ json 1.2.0 -> 1.4.0\n");
    assert!(manifest().contains(&format!("json = {{ git = \"{json}\", version = \"1.4.0\" }}")), "{}", manifest());
    assert_eq!(blade(&["update"]), "dependencies are up to date\n");
    assert_eq!(blade(&["remove", "http"]), "- http 2.1.0\n");
    assert!(!manifest().contains("http ="), "{}", manifest());
    assert_eq!(locked(), [pair("json", "1.4.0")]);
    assert!(!installed("http") && installed("json"));
    assert_eq!(blade(&["sync"]), "dependencies are up to date\n");

    // The help on stdout is the usage text of an error, byte for byte.
    for args in [&["--help"][..], &["-h"], &["help"], &[], &["sync", "--help"]] {
        let help = tree.blade(".", args);
        assert_eq!(help.status.code(), Some(0), "{args:?}");
        assert_eq!(text(&help.stdout), blade::HELP, "{args:?}");
    }
    assert!(blade::HELP.contains("blade add NAME GIT [MIN]") && !blade::HELP.contains("blade install"));
    for args in [&["--version"][..], &["-V"], &["-v"], &["version"]] {
        let version = tree.blade(".", args);
        assert_eq!(version.status.code(), Some(0), "{args:?}");
        assert_eq!(text(&version.stdout), format!("{}\n", blade::VERSION), "{args:?}");
    }
    // `install` was replaced by `sync` and `add` in 0.5.3 and is gone (#1040).
    let install = [&["install"][..], &["install", "--git", &http, "2.1.0"]];
    for wrong in [&["add"][..], &["add", "json"], &["add", "a", "b", "c", "d"], &["sync", "x"], &["bogus"]].into_iter().chain(install) {
        let output = tree.blade("app", wrong);
        assert_eq!(output.status.code(), Some(2), "{wrong:?}");
        assert_eq!(text(&output.stderr), blade::HELP, "{wrong:?}");
        assert!(output.stdout.is_empty(), "{wrong:?}");
    }
    assert_eq!(locked(), [pair("json", "1.4.0")]);
}
