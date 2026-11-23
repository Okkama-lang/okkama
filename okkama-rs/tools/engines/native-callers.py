#!/usr/bin/env python3
"""Every native declared in std/runtime has a caller (#1022).

A native counts as called when

- a std module calls it: the module imports its runtime module and calls the
  name (`import "runtime/fs" take cwd;` … `cwd()`, or `fs.cwd(…)` through the
  module's alias), or
- runtime Rust code calls it by its symbol (`"runtime/fs@1::cwd/0"`) outside
  tests, in sparrow-core, okkama-runtime-core or okkama-cli.

A provider that only implements a native is not a caller: a native nobody
calls is dead weight in the ABI, the providers and the capability list
(#1020, #1022), so it goes, declaration, provider and table entry together.
KEPT lists the natives kept without a caller, each with its reason; an entry
that gains a caller or leaves std/runtime fails the check too, so the list
cannot go stale.

    tools/engines/native-callers.py          # exit 1 and name every native without a caller
    tools/engines/native-callers.py --list   # also print each native's callers
"""

import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
RUST_ROOT = HERE.parent.parent
REPO_ROOT = RUST_ROOT.parent
STD = REPO_ROOT / "std"
RUNTIME_CRATES = ["sparrow-core", "okkama-runtime-core", "okkama-cli"]

# The legacy SQLite execution path and the db_capture cursor protocol stay
# without a std caller (#1020): db/sqlite reads through the db_* cells today.
LEGACY_SQLITE = "legacy SQLite execution path, kept (#1020)"
DB_CAPTURE = "db_capture protocol, kept with the legacy SQLite path (#1020)"
KEPT = {
    ("runtime/database", "sqlite_execution"): LEGACY_SQLITE,
    ("runtime/database", "sqlite_prepared_execution"): LEGACY_SQLITE,
    ("runtime/database", "sqlite_prepared_rows"): LEGACY_SQLITE,
    ("runtime/database", "sqlite_reply_bytes"): LEGACY_SQLITE,
    ("runtime/database", "sqlite_reply_cells"): LEGACY_SQLITE,
    ("runtime/database", "sqlite_reply_kind"): LEGACY_SQLITE,
    ("runtime/database", "sqlite_reply_text"): LEGACY_SQLITE,
    ("runtime/database", "sqlite_rows"): LEGACY_SQLITE,
    ("runtime/db", "db_capture"): DB_CAPTURE,
    ("runtime/db", "db_capture_complete"): DB_CAPTURE,
    ("runtime/db", "db_capture_emit"): DB_CAPTURE,
    ("runtime/db", "db_capture_publish"): DB_CAPTURE,
    ("runtime/db", "db_capture_value"): DB_CAPTURE,
}

DECLARATION = re.compile(r"\bnative\s+(?:async\s+)?(?!object\b)([a-z_0-9]+)\s*\((.*?)\)", re.S)
MODULE_LINE = re.compile(r"^\s*module\s+(\S+)\s+v(\d+)\.", re.M)
IMPORT = re.compile(r'\bimport\s+"(runtime/[a-z_0-9]+)"\s*(?:as\s+([a-z_0-9]+)|take\s+([^;]*))?\s*;', re.S)
SYMBOL = re.compile(r'"(runtime/[a-z_0-9]+)@(\d+)::([a-z_0-9]+)/(\d+)"')
LINE_COMMENT = re.compile(r"//[^\n]*")


def arity(parameters):
    """The number of parameters of a declaration's parameter list."""
    depth, count, empty = 0, 1, True
    for char in parameters:
        if not char.isspace():
            empty = False
        if char in "([<":
            depth += 1
        elif char in ")]>":
            depth -= 1
        elif char == "," and depth == 0:
            count += 1
    return 0 if empty else count


def declared():
    """{(module, name): symbol} for every native of std/runtime."""
    natives = {}
    for path in sorted((STD / "runtime").glob("*.ok")):
        text = LINE_COMMENT.sub("", path.read_text())
        found = MODULE_LINE.search(text)
        module, major = (found.group(1), found.group(2)) if found else ("runtime/" + path.stem, "1")
        for name, parameters in DECLARATION.findall(text):
            natives[(module, name)] = f"{module}@{major}::{name}/{arity(parameters)}"
    return natives


def std_callers(natives):
    """{(module, name): [std file, …]} for the natives std modules call."""
    callers = {}
    modules = {module for module, _ in natives}
    for path in sorted(STD.rglob("*.ok")):
        text = LINE_COMMENT.sub("", path.read_text())
        imports = list(IMPORT.finditer(text))
        body = IMPORT.sub("", text)
        for found in imports:
            module = found.group(1)
            if module not in modules:
                continue
            if found.group(3) is not None:
                names = [name.strip() for name in found.group(3).split(",") if name.strip()]
                called = [name for name in names if re.search(rf"(?<![\w.]){re.escape(name)}\s*\(", body)]
            else:
                alias = found.group(2) or module.rsplit("/", 1)[-1]
                called = re.findall(rf"(?<![\w.]){re.escape(alias)}\.([a-z_0-9]+)\s*\(", body)
            for name in called:
                if (module, name) in natives:
                    callers.setdefault((module, name), []).append(str(path.relative_to(REPO_ROOT)))
    return callers


def rust_callers(natives):
    """{(module, name): [Rust file, …]} for the natives runtime code calls by symbol, outside tests."""
    by_symbol = {symbol: key for key, symbol in natives.items()}
    callers = {}
    for crate in RUNTIME_CRATES:
        for path in sorted((RUST_ROOT / "crates" / crate / "src").rglob("*.rs")):
            if "tests" in path.stem or "tests" in path.parts:
                continue
            text = path.read_text()
            # Inline unit tests sit at the end of a file, after `#[cfg(test)]`.
            text = text.split("#[cfg(test)]", 1)[0]
            for found in SYMBOL.finditer(text):
                key = by_symbol.get(found.group(0).strip('"'))
                if key:
                    callers.setdefault(key, []).append(str(path.relative_to(REPO_ROOT)))
    return callers


def main():
    listing = "--list" in sys.argv[1:]
    natives = declared()
    if not natives:
        sys.exit("native-callers: no natives found under std/runtime")
    callers = std_callers(natives)
    for key, files in rust_callers(natives).items():
        callers.setdefault(key, []).extend(files)
    problems = []
    for key in sorted(natives):
        found = sorted(set(callers.get(key, [])))
        if listing:
            print(f"{natives[key]}: {', '.join(found) if found else '-'}")
        if not found and key not in KEPT:
            problems.append(f"{natives[key]} has no caller in std or the runtime crates: remove it (#1022) or keep it in KEPT with a reason")
        if found and key in KEPT:
            problems.append(f"{natives[key]} is called ({found[0]}) but listed in KEPT: drop the entry")
    for key in sorted(set(KEPT) - set(natives)):
        problems.append(f"{key[0]}.{key[1]} is in KEPT but not declared in std/runtime: drop the entry")
    for problem in problems:
        print(f"native-callers: {problem}", file=sys.stderr)
    if problems:
        sys.exit(1)
    print(f"native-callers: {len(natives)} natives, {len(natives) - len(KEPT)} called, {len(KEPT)} kept without a caller")


if __name__ == "__main__":
    main()
