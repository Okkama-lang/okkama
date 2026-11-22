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


# #244 (owner, 2026-11-22): every action that does real work gets a benchmark; accessors, one-call wrappers and
# signatures without a body are covered through the benchmarks of their callers.
CLASS_ORDER = ["signature", "accessor", "wrapper", "work"]
CONTROL = {"IfStatement", "IfExpression", "ForStatement", "LoopExpression", "TryStatement", "TryExpression",
           "MatchStatement", "MatchExpression", "DeferStatement", "Await", "Functional", "AnonymousAction", "Select"}


def _count(node, names):
    if isinstance(node, dict):
        return (node.get("node") in names) + sum(_count(value, names) for value in node.values())
    if isinstance(node, list):
        return sum(_count(value, names) for value in node)
    return 0


def classify_member(owner, member):
    """signature (no body: role signature, shorthand constructor, enum payload case), accessor (no call, no
    control flow), wrapper (one statement with at most one call, no control flow) or work."""
    if owner["node"] == "RoleDecl" or member["node"] in {"ActionSignature", "ShorthandConstructor", "EnumVariant"}:
        return "signature"
    body = member.get("body")
    calls, control = _count(body, {"Call"}), _count(body, CONTROL)
    statements = len(body.get("statements", [])) if isinstance(body, dict) else 0
    if calls == 0 and control == 0:
        return "accessor"
    if calls <= 1 and control == 0 and statements <= 1:
        return "wrapper"
    return "work"


def classify_ast(tree):
    """Owner.action -> its class; overloads take the most demanding class."""
    classes = {}
    for owner in tree["items"]:
        if not public(owner) or owner["node"] not in {"ObjectDecl", "RoleDecl", "EnumDecl"}:
            continue
        for member in owner.get("members", []) + owner.get("variants", []):
            if not public(member):
                continue
            kind = member["node"]
            if kind in {"FullConstructor", "ShorthandConstructor"}:
                action = "new"
            elif kind in {"ActionDecl", "ActionSignature"}:
                action = member["name"]
            elif kind == "EnumVariant" and member.get("fields"):
                action = member["name"]
            else:
                continue
            name = owner["name"] + "." + action
            found = classify_member(owner, member)
            if name not in classes or CLASS_ORDER.index(found) > CLASS_ORDER.index(classes[name]):
                classes[name] = found
    return classes


_CLASSES = {}


def read_module(binary, path):
    command = [binary, "ast", str(path), "--format", "json", "--mode", "production", "--prune-tests"]
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ValueError(f"cannot read AST for {path}: {error}") from error
    if result.returncode:
        raise ValueError(f"AST failed for {path}: {result.stderr.strip()}")
    try:
        tree = json.loads(result.stdout)
        _CLASSES[Path(path).resolve()] = classify_ast(tree)
        return parse_ast(tree)
    except (ValueError, KeyError, TypeError) as error:
        raise ValueError(f"invalid AST for {path}: {error}") from error


def inventory(binary, stdlib):
    paths = sorted(p for p in stdlib.rglob("*.ok") if p.relative_to(stdlib).parts[0] != "runtime")
    if not paths:
        raise ValueError("stdlib directory has no non-runtime module files")
    return {p.relative_to(stdlib).with_suffix("").as_posix(): read_module(binary, p) for p in paths}


def inventory_classes(stdlib):
    """module -> Owner.action -> class, for the modules `inventory` read."""
    stdlib = Path(stdlib).resolve()
    return {path.relative_to(stdlib).with_suffix("").as_posix(): classes
            for path, classes in _CLASSES.items() if stdlib in path.parents}


def excluded_reason(target, excluded):
    """Why a target cannot be measured in a self-contained benchmark (a module or a target key), or None."""
    module = target.partition(":")[0]
    for key in (target, module) + tuple("/".join(module.split("/")[:n]) + "/*" for n in range(len(module.split("/")), 0, -1)):
        if key in excluded:
            return excluded[key]
    return None


def todo(modules, manifest, classes, excluded=None):
    """Work actions no benchmark measures yet and not excluded, module by module."""
    measured = {target for entry in manifest.values() for target in entry.get("actions", [])}
    return [module + ":" + action for module, actions in sorted(modules.items()) for action in sorted(actions)
            if classes.get(module, {}).get(action) == "work" and module + ":" + action not in measured
            and excluded_reason(module + ":" + action, excluded or {}) is None]


def report(modules, manifest, classes=None, excluded=None):
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
    excluded = excluded or {}
    kind = lambda module, action: (classes or {}).get(module, {}).get(action, "work")
    outside = lambda module, action: module + ":" + action not in measured and excluded_reason(module + ":" + action, excluded)
    lines = ["# std public actions (generated by actions.py; do not edit)", "",
             f"{len(measured)} of {total} public action targets measured across {len(modules)} non-runtime modules.", ""]
    if classes is not None:
        work = [(m, a) for m, actions in modules.items() for a in actions if kind(m, a) == "work"]
        done = sum(m + ":" + a in measured for m, a in work)
        out = sum(bool(outside(m, a)) for m, a in work)
        trivial = total - len(work)
        lines += [f"Work actions: {done} of {len(work)} measured, {out} excluded (they need a server, a socket, a "
                  "terminal, a test context or a host resource; reasons in excluded.json). "
                  f"{trivial} accessors, one-call wrappers and signatures without a body are covered through the "
                  "benchmarks of their callers (#244, owner 2026-11-22); `--todo` lists the work actions still to "
                  "measure.", ""]
    lines += [
             "Targets group overloads by name; a measured target does not imply all overloads are measured. "
             "Object constructors use `Object.new`; payload enum constructors use `Enum.case`. "
             "Role signatures are listed separately from implementations. Hidden declarations, caps, "
             "enum constants and runtime modules are excluded. Builtin targets stay in manifest.json.", "",
             "Only explicitly declared measured calls count, not harness imports, fixture setup before timing "
             "or transitive implementation calls. Missing targets are a work list, not a correctness failure.", ""]
    for module, actions in sorted(modules.items()):
        count = sum(module + ":" + action in measured for action in actions)
        if classes is None:
            lines += [f"## {module} — {count}/{len(actions)} measured", "", "| target | public signatures | measured by |", "|---|---|---|"]
        else:
            work = [a for a in actions if kind(module, a) == "work"]
            done = sum(module + ":" + a in measured for a in work)
            lines += [f"## {module} — {count}/{len(actions)} measured, work {done}/{len(work)}", "",
                      "| target | class | public signatures | measured by |", "|---|---|---|---|"]
        for action, signatures in sorted(actions.items()):
            target = module + ":" + action
            # Escape Markdown table syntax even inside code spans.
            sig = "<br>".join("`" + s.replace("|", "&#124;").replace("`", "&#96;") + "`" for s in sorted(signatures))
            reason = outside(module, action)
            by = ', '.join(sorted(measured.get(target, []))) or ('excluded: ' + reason if reason and kind(module, action) == "work"
                                                                 else '—' if kind(module, action) == "work" else 'via callers')
            if classes is None:
                lines.append(f"| `{target}` | {sig} | {', '.join(sorted(measured.get(target, []))) or '—'} |")
            else:
                lines.append(f"| `{target}` | {kind(module, action)} | {sig} | {by} |")
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
    parser.add_argument("--excluded", type=Path, default=HERE / "excluded.json")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--scaffold", metavar="MODULE:Object.action")
    mode.add_argument("--todo", action="store_true", help="print the work actions no benchmark measures yet")
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
        manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
        classes = inventory_classes(args.stdlib)
        excluded = json.loads(args.excluded.read_text(encoding="utf-8")) if args.excluded.exists() else {}
        if args.todo:
            for target in todo(modules, manifest, classes, excluded):
                print(target)
            return 0
        text = report(modules, manifest, classes, excluded)
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
