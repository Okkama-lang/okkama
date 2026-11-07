#!/usr/bin/env python3
"""Build the Okkama Bible from its chapters.

    python3 docs/OkkamaBible/build.py [--check] [--stats] [--figures]

chapters/en/*.md and chapters/ru/*.md (same file names, ordered by name) are
joined into OkkamaBible.md and OkkamaBible-RU.md with a generated table of
contents. Placeholders in a chapter:

    {{example:NAME}}            examples/NAME.ok and its expected output
    {{figure:NAME|caption}}     figures/NAME.svg
    {{table:builtins}}          builtin behaviour, from the compiler's table
    [@key]                      a citation of references.md

--figures runs figures/src/*.py first; --check runs every example on the IR
engine and on the Sparrow core (OKKAMA, default the release build) and
compares stdout plus a final "exit N" line with examples/NAME.expected;
--stats prints characters and pages (2,500 characters) per chapter.
"""
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
OKKAMA = os.environ.get("OKKAMA", os.path.join(ROOT, "okkama-rs", "target", "release", "okkama"))
PAGE = 2500
LANGUAGES = {
    "en": ("OkkamaBible.md", "The Okkama Bible", "Contents", "Output", "References"),
    "ru": ("OkkamaBible-RU.md", "Библия Okkama", "Содержание", "Вывод", "Литература"),
}


def references():
    """key -> (label, entry) from references.md (`- [@key] Authors (year). ...`)."""
    found = {}
    for line in open(os.path.join(HERE, "references.md")):
        match = re.match(r"- \[@([\w-]+)\] (.+)$", line.strip())
        if match:
            key, entry = match.groups()
            authors = re.split(r"[,(]", entry, 1)[0].strip()
            year = re.search(r"\((\d{4}[a-z]?|n\.d\.)\)", entry)
            found[key] = (f"{authors.split(' and ')[0].split(' ')[-1]} {year.group(1) if year else ''}".strip(), entry)
    return found


def example(name, language):
    source = open(os.path.join(HERE, "examples", name + ".ok")).read().rstrip()
    block = "```okkama\n" + source + "\n```"
    expected = os.path.join(HERE, "examples", name + ".expected")
    if os.path.exists(expected):
        output = open(expected).read().rstrip()
        if output:
            block += f"\n\n{LANGUAGES[language][3]}:\n\n```text\n{output}\n```"
    return block


def builtins_table(language):
    """Builtin actions per receiver, from analysis/check/builtin.rs."""
    path = os.path.join(ROOT, "okkama-rs", "crates", "okkama-compiler", "src", "analysis", "check", "builtin.rs")
    rows = {}
    for receiver, name in re.findall(r'\((Ty::[^,]*?), "([a-z_]+)"\)', open(path).read()):
        for kind in re.findall(r"Ty::(\w+)", receiver):
            rows.setdefault(kind, []).append(name)
    names = {"Array": "@T", "Map": "%K:V", "Str": "string", "Int": "int", "Float": "float", "Rat": "rat",
             "Byte": "byte", "Char": "char", "Future": "Future<T>"}
    head = "| Receiver | Builtin actions |" if language == "en" else "| Получатель | Встроенные действия |"
    lines = [head, "|---|---|"]
    for kind, actions in rows.items():
        lines.append(f"| `{names.get(kind, kind)}` | " + ", ".join(f"`{a}`" for a in sorted(set(actions))) + " |")
    return "\n".join(lines)


def slug(title):
    return re.sub(r"[^\w\- ]", "", title.lower()).strip().replace(" ", "-")


def build(language, cited):
    target, title, contents, _, refs_title = LANGUAGES[language]
    directory = os.path.join(HERE, "chapters", language)
    parts = []
    for name in sorted(os.listdir(directory)):
        if name.endswith(".md"):
            parts.append(open(os.path.join(directory, name)).read().rstrip() + "\n")
    text = "\n".join(parts)
    text = re.sub(r"\{\{example:([\w-]+)\}\}", lambda m: example(m.group(1), language), text)
    def figure(match):
        name, caption = match.groups()
        localized = f"{name}-{language}.svg"
        file = localized if os.path.exists(os.path.join(HERE, "figures", localized)) else f"{name}.svg"
        return f"![{caption}](figures/{file})\n\n*{caption}*"

    text = re.sub(r"\{\{figure:([\w-]+)\|([^}]*)\}\}", figure, text)
    text = text.replace("{{table:builtins}}", builtins_table(language))
    known = references()

    def cite(match):
        keys = [k.strip().lstrip("@") for k in match.group(1).split(";")]
        for key in keys:
            if key not in known:
                raise SystemExit(f"{language}: unknown reference [@{key}]")
            cited.add(key)
        return "[" + "; ".join(f"[{known[k][0]}](#ref-{k})" for k in keys) + "]"

    text = re.sub(r"\[(@[\w-]+(?:;\s*@[\w-]+)*)\]", cite, text)
    toc = [f"## {contents}", ""]
    for level, heading in re.findall(r"^(#{1,2}) (.+)$", text, re.M):
        indent = "" if level == "#" else "  "
        toc.append(f"{indent}- [{heading}](#{slug(heading)})")
    bibliography = [f"# {refs_title}", ""]
    for key in sorted(known, key=lambda k: known[k][0].lower()):
        bibliography.append(f'- <a id="ref-{key}"></a>{known[key][1]}')
    banner = "<!-- Generated by build.py from chapters/" + language + "; edit the chapters. -->\n"
    output = banner + f"# {title}\n\n" + "\n".join(toc) + "\n\n" + text + "\n" + "\n".join(bibliography) + "\n"
    open(os.path.join(HERE, target), "w").write(output)


def stats():
    total = {}
    for language in LANGUAGES:
        directory = os.path.join(HERE, "chapters", language)
        for name in sorted(os.listdir(directory)):
            size = len(open(os.path.join(directory, name)).read())
            total[language] = total.get(language, 0) + size
            print(f"{language} {name:<48} {size:>8} chars {size / PAGE:6.1f} pages")
    for language, size in total.items():
        print(f"{language} total {size} chars, {size / PAGE:.0f} pages")


def check():
    failures = 0
    examples = os.path.join(HERE, "examples")
    for name in sorted(os.listdir(examples)):
        if not name.endswith(".ok"):
            continue
        stem = name[:-3]
        expected_path = os.path.join(examples, stem + ".expected")
        expected = open(expected_path).read().rstrip("\n").splitlines() if os.path.exists(expected_path) else None
        for engine in ("ir", "sparrow"):
            # Examples run in the repository's cage: no forks, network or
            # writes outside examples/, bounded CPU and memory.
            cage = [sys.executable, os.path.join(ROOT, "okkama-rs", "tools", "new-runtime", "caged.py"),
                    "--cpu-seconds", "30", "--timeout", "60", "--memory-mb", "1024", "--writable", examples, "--"]
            p = subprocess.run(cage + [OKKAMA, "run", "--engine", engine, name], cwd=examples,
                               capture_output=True, text=True, timeout=120,
                               env={**os.environ, "OKKAMA_STDLIB_DIR": os.path.join(ROOT, "std"), "OKKAMA_TRACE": "off"})
            output = p.stdout.rstrip("\n").splitlines() if p.stdout else []
            if p.returncode != 0:
                output.append(f"exit {p.returncode}")
            if expected is None:
                print(f"{stem} [{engine}]: no .expected; printed {output} {p.stderr.strip()[:200]}")
                failures += 1
            elif output != expected:
                print(f"{stem} [{engine}]: printed {output}, expected {expected}; {p.stderr.strip()[:300]}")
                failures += 1
        print(f"{stem}: checked")
    return failures


if __name__ == "__main__":
    if "--figures" in sys.argv:
        for script in sorted(os.listdir(os.path.join(HERE, "figures", "src"))):
            if script.endswith(".py"):
                subprocess.run([sys.executable, os.path.join(HERE, "figures", "src", script)], check=True)
    cited = set()
    for language in LANGUAGES:
        build(language, cited)
    if "--stats" in sys.argv:
        stats()
    if "--check" in sys.argv:
        sys.exit(1 if check() else 0)
