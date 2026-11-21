#!/usr/bin/env python3
"""Inventory public std actions from production ASTs; scaffold unfinished cases."""
import argparse
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent


def type_text(t):
    kind = t["node"]
    if kind == "NamedType":
        args = t.get("args", [])
        return "/".join(t["path"]) + ("<" + ", ".join(map(type_text, args)) + ">" if args else "")
    if kind == "ArrayType":
        return "@" + type_text(t["element"])
    if kind == "MapType":
        return "@" + type_text(t["key"]) + ":" + type_text(t["value"])
    if kind == "NestedType":
        return type_text(t["owner"]) + "::" + t["name"]
    if kind == "VoidResult":
        return "void"
    if kind == "SuccessorResult":
        return "^" + type_text(t["type"])
    if kind == "ActionType":
        return ("async " if t["async"] else "") + "(" + ", ".join(map(type_text, t["params"])) + ") is " + type_text(t["result"])
    raise ValueError("unsupported AST type: " + kind)


def public(node):
    return not node.get("hidden", False) and node.get("visibility", "public") == "public"


def parse_ast(tree):
    """Local declarations only. Group overloads by Owner.name, constructors by Owner.new."""
    if tree.get("node") != "Program":
        raise ValueError("expected Program AST")
    actions = {}
    for owner in tree["items"]:
        if not public(owner) or owner["node"] not in {"ObjectDecl", "RoleDecl", "EnumDecl"}:
            continue
        name = owner["name"]
        generics = owner.get("generics", [])
        generic_text = (" [" + ", ".join(g["name"] + (" is " + type_text(g["bound"]) if g.get("bound") else "") for g in generics) + "]") if generics else ""
        fields = {n: member["type"] for member in owner.get("members", [])
                  if member["node"] == "HasDecl" for n in member["names"]}
        for member in owner.get("members", []) + owner.get("variants", []):
            if not public(member):
                continue
            kind = member["node"]
            if kind in {"FullConstructor", "ShorthandConstructor"}:
                action = "new"
                params = member.get("params")
                if params is None:
                    params = [{"name": n, "type": fields[n]} for n in member["fields"]]
                result = name
            elif kind in {"ActionDecl", "ActionSignature"}:
                action = member["name"]
                params = member["params"]
                result = type_text(member["result"])
            elif kind == "EnumVariant" and member.get("fields"):
                action = member["name"]
                params = member["fields"]
                result = name
            else:
                continue
            signature = ("async " if member.get("async") else "") + name + "." + action
            signature += "(" + ", ".join(p["name"] + ("..." if p.get("variadic") else "") + " is " + type_text(p["type"]) for p in params) + ") is " + result + generic_text
            actions.setdefault(name + "." + action, set()).add(signature)
    return {name: sorted(signatures) for name, signatures in sorted(actions.items())}


def read_module(binary, path):
    command = [binary, "ast", str(path), "--format", "json", "--mode", "production", "--prune-tests"]
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ValueError(f"cannot read AST for {path}: {error}") from error
    if result.returncode:
        raise ValueError(f"AST failed for {path}: {result.stderr.strip()}")
    try:
        return parse_ast(json.loads(result.stdout))
    except (ValueError, KeyError, TypeError) as error:
        raise ValueError(f"invalid AST for {path}: {error}") from error


def inventory(binary, stdlib):
    paths = sorted(p for p in stdlib.rglob("*.ok") if p.relative_to(stdlib).parts[0] != "runtime")
    if not paths:
        raise ValueError("stdlib directory has no non-runtime module files")
    return {p.relative_to(stdlib).with_suffix("").as_posix(): read_module(binary, p) for p in paths}


def report(modules, manifest):
    measured = {}
    for program, entry in sorted(manifest.items()):
        actions = entry.get("actions", [])
        if not isinstance(actions, list) or any(not isinstance(a, str) for a in actions):
            raise ValueError(f"{program}: actions must be a list of strings")
        if len(actions) != len(set(actions)):
            raise ValueError(f"{program}: duplicate action targets")
        for target in actions:
            if target.startswith("builtin:"):
                if target not in {"builtin:string.replace", "builtin:string.split"} or target.replace(".", "-") not in entry.get("targets", []):
                    raise ValueError(f"{program}: unknown builtin action {target}")
                continue
            module, separator, action = target.partition(":")
            if not separator or action not in modules.get(module, {}):
                raise ValueError(f"{program}: unknown public action {target}")
            if module not in entry.get("targets", []):
                raise ValueError(f"{program}: action module {module} missing from targets")
            measured.setdefault(target, []).append(program)
    total = sum(len(actions) for actions in modules.values())
    lines = ["# std public actions (generated by actions.py; do not edit)", "",
             f"{len(measured)} of {total} public action targets measured across {len(modules)} non-runtime modules.", "",
             "Targets group overloads by name; a measured target does not imply all overloads are measured. "
             "Object constructors use `Object.new`; payload enum constructors use `Enum.case`. "
             "Role signatures are listed separately from implementations. Hidden declarations, caps, "
             "enum constants and runtime modules are excluded. Builtin targets stay in manifest.json.", "",
             "Only explicitly declared measured calls count, not harness imports, fixture setup before timing "
             "or transitive implementation calls. Missing targets are a work list, not a correctness failure.", ""]
    for module, actions in sorted(modules.items()):
        count = sum(module + ":" + action in measured for action in actions)
        lines += [f"## {module} — {count}/{len(actions)} measured", "", "| target | public signatures | measured by |", "|---|---|---|"]
        for action, signatures in sorted(actions.items()):
            target = module + ":" + action
            # Escape Markdown table syntax even inside code spans.
            sig = "<br>".join("`" + s.replace("|", "&#124;").replace("`", "&#96;") + "`" for s in sorted(signatures))
            lines.append(f"| `{target}` | {sig} | {', '.join(sorted(measured.get(target, []))) or '—'} |")
        lines.append("")
    return "\n".join(lines)


def scaffold(target, modules, directory=HERE / "std"):
    module, sep, action = target.partition(":")
    if not sep or action not in modules.get(module, {}):
        raise ValueError("unknown public action: " + target)
    # Encode separators distinctly and preserve case: no module/object collisions.
    stem = "action-" + module.replace("/", "--") + "--" + action.replace(".", "--")
    if not re.fullmatch(r"[A-Za-z0-9_-]+", stem):
        raise ValueError("target cannot be used as a scaffold filename")
    source = directory / (stem + ".ok")
    oracle = directory / (stem + ".oracle.py")
    if source.exists() or oracle.exists():
        raise ValueError("scaffold already exists: " + stem)
    owner = action.split(".")[0]
    template = (HERE / "std" / "hex-encode.ok").read_text(encoding="utf-8")
    start = template.index("        let bytes =")
    end = template.index("\n    }", start)
    template = template[:start] + "        // TODO: construct input varying with i and call " + target + ".\n" + "        // TODO: reduce the result to an observable integer contribution.\n" + "        let contribution = TODO_INPUT_AND_MEASURED_CALL(i);\n" + "        self.run(i + 1, n, acc + contribution % 7);" + template[end:]
    template = template.replace('import "encoding/hex" take HexEncoded;', f'import "{module}" take {owner};')
    template = "// Unfinished scaffold; do not register until inputs and oracle are reviewed.\n" + "// " + " | ".join(modules[module][action]) + "\n" + template
    directory.mkdir(parents=True, exist_ok=True)
    with source.open("x", encoding="utf-8") as stream:
        stream.write(template)
    with oracle.open("x", encoding="utf-8") as stream:
        stream.write('"""TODO: independently derive the checksum; never copy VM output."""\n\ndef expected(n):\n    raise NotImplementedError("TODO: hand-written oracle for ' + target + '")\n')
    return source, oracle


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--okkama", default="okkama")
    parser.add_argument("--stdlib", type=Path, default=ROOT / "std")
    parser.add_argument("--manifest", type=Path, default=HERE / "manifest.json")
    parser.add_argument("--output", type=Path, default=HERE / "STD-ACTIONS.md")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--scaffold", metavar="MODULE:Object.action")
    args = parser.parse_args(argv)
    try:
        binary = shutil.which(args.okkama)
        if binary is None:
            raise ValueError("okkama executable not found: " + args.okkama + "; install it or pass --okkama PATH")
        modules = inventory(binary, args.stdlib.resolve())
        if args.scaffold:
            for path in scaffold(args.scaffold, modules):
                print(path)
            return 0
        text = report(modules, json.loads(args.manifest.read_text(encoding="utf-8")))
        if args.check:
            if not args.output.exists() or args.output.read_text(encoding="utf-8") != text:
                print(f"stale: {args.output}; regenerate with actions.py", file=sys.stderr)
                return 1
        else:
            args.output.write_text(text, encoding="utf-8")
        return 0
    except (ValueError, OSError) as error:
        print(f"actions.py: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
