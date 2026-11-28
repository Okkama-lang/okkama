#!/usr/bin/env python3
"""Generate the native ABI table from the std provider declarations.

Reads every `native` declaration of std/runtime/*.ok and writes
crates/okkama-runtime-abi/src/table.rs: one `natives!` entry per provider
symbol `module@major::name/arity` with its Okkama signature. Wire IDs are
stable: an existing symbol keeps its ID and name (read from the current
table); a new symbol gets the next free ID. Removing a declaration retires
its ID — it is never reused.

    tools/engines/gen-abi.py            # write the table
    tools/engines/gen-abi.py --check    # exit 1 if the table differs from std/runtime (#976)

A type name that is neither a scalar, a collection of known types nor a `native object` declared in
std/runtime is an error: it used to become a handle silently (#976). A handle is written with its module, as SPB
names it (`runtime.buffer::BufferState`, #1083), so two modules may not declare one name.
"""

import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
RUST_ROOT = HERE.parent.parent
REPO_ROOT = RUST_ROOT.parent
TABLE = RUST_ROOT / "crates" / "okkama-runtime-abi" / "src" / "table.rs"

DECLARATION = re.compile(
    r"\bnative\s+(async\s+)?(?!object\b)([a-z_0-9]+)\s*\((.*?)\)\s*(?:is\s+([^;{]+?))?\s*;",
    re.S,
)
ENTRY = re.compile(r'^\s*(\w+)\s*=\s*(\d+),\s*"([^"]+)"')
HANDLE = re.compile(r"\bnative\s+object\s+([A-Z][A-Za-z0-9_]*)\s*;")
SCALARS = {"int", "float", "rat", "bool", "byte", "char", "string", "any", "void"}


def qualify(text, handles, where):
    """The ABI spelling of a declared type: `@T`, `%K:V`, `?T`, a scalar, or a native object with its module.
    Fails on a type the ABI cannot name."""
    text = text.strip()
    if text.startswith("@"):
        return "@" + qualify(text[1:], handles, where)
    if text.startswith("?"):
        return "?" + qualify(text[1:], handles, where)
    if text.startswith("%"):
        key, _, value = text[1:].partition(":")
        return "%" + qualify(key, handles, where) + ":" + qualify(value, handles, where)
    if text in SCALARS:
        return text
    if text in handles:
        return f"{handles[text].replace('/', '.')}::{text}"
    sys.exit(f"gen-abi: {where}: unknown type `{text}` (not a scalar, a collection or a native object)")


def camel(text):
    return "".join(part[:1].upper() + part[1:] for part in re.split(r"[_/]", text) if part)


def main():
    known = {}
    retired = set()
    if TABLE.is_file():
        for line in TABLE.read_text().splitlines():
            found = ENTRY.match(line)
            if found:
                known[found.group(3)] = (found.group(1), int(found.group(2)))
            retired.update(int(n) for n in re.findall(r"retired:\s*([\d, ]+)", line) for n in n.split(",") if n.strip())
    used_ids = {entry[1] for entry in known.values()} | retired
    used_names = {entry[0] for entry in known.values()}
    entries = []
    sources = sorted((REPO_ROOT / "std" / "runtime").glob("*.ok"))
    handles = {}
    for path in sources:
        text = path.read_text()
        module = re.search(r"^module\s+(\S+)\s+v", text, re.M).group(1)
        for name in HANDLE.findall(re.sub(r"//[^\n]*", "", text)):
            if name in handles and handles[name] != module:
                sys.exit(f"gen-abi: native object {name} is declared in {handles[name]} and {module}")
            handles[name] = module
    for path in sources:
        text = path.read_text()
        header = re.search(r"^module\s+(\S+)\s+v(\d+)\.", text, re.M)
        module, major = header.group(1), header.group(2)
        code = re.sub(r"//[^\n]*", "", text)
        for found in DECLARATION.finditer(code):
            is_async, name, params, result = found.groups()
            types = []
            for param in filter(None, (p.strip() for p in params.split(","))):
                types.append(param.split(" is ", 1)[1].strip())
            symbol = f"{module}@{major}::{name}/{len(types)}"
            types = [qualify(ty, handles, symbol) for ty in types]
            result = qualify(result, handles, symbol) if result else "void"
            signature = f"({', '.join(types)}) -> {result}"
            if is_async:
                signature = "async " + signature
            entries.append((symbol, signature, module, name))
    symbols = {entry[0] for entry in entries}
    lines = [
        "// Сгенерировано tools/engines/gen-abi.py из объявлений std/runtime/*.ok.",
        "// Wire ID стабильны: не менять вручную номера существующих символов.",
        "natives! {",
    ]
    next_id = max(used_ids, default=0) + 1
    rows = []
    for symbol, signature, module, name in entries:
        if symbol in known:
            variant, wire = known[symbol]
        else:
            variant = camel(module.removeprefix("runtime/")) + camel(name)
            while variant in used_names:
                variant += "X"
            used_names.add(variant)
            wire = next_id
            next_id += 1
        rows.append((wire, variant, symbol, signature))
    gone = sorted(entry[1] for symbol, entry in known.items() if symbol not in symbols)
    retired.update(gone)
    for wire, variant, symbol, signature in sorted(rows):
        lines.append(f'    {variant} = {wire}, "{symbol}", "{signature}";')
    lines.append("}")
    if retired:
        lines.append(f"// retired: {', '.join(str(n) for n in sorted(retired))}")
    text = "\n".join(lines) + "\n"
    if "--check" in sys.argv[1:]:
        if TABLE.read_text() != text:
            sys.exit("gen-abi: crates/okkama-runtime-abi/src/table.rs is stale; run tools/engines/gen-abi.py")
        print(f"table.rs current: {len(rows)} natives")
        return
    TABLE.write_text(text)
    print(f"{len(rows)} natives, {len(gone)} retired now, next id {next_id}")


if __name__ == "__main__":
    main()
