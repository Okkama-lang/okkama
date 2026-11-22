#!/usr/bin/env python3
"""Build the Okkama Bible from its chapters.

    python3 docs/OkkamaBible/build.py [--check] [--stats] [--figures] [--std-index] [--pdf]

chapters/en/*.md and chapters/ru/*.md (same file names, ordered by name) are
joined into OkkamaBible.md and OkkamaBible-RU.md with a generated table of
contents. Placeholders in a chapter:

    {{example:NAME}}            examples/NAME.ok and its expected output (NAME may
                                name a module in a subdirectory: shop/money)
    {{figure:NAME|caption}}     figures/NAME.svg
    {{table:builtins}}          builtin behaviour, from the compiler's table
    [@key]                      a citation of references.md

--figures runs figures/src/*.py first; --check runs every example on Sparrow,
the only Okkama engine since 0.6.0 (#584) (OKKAMA, default the release build), and
compares stdout plus a final "exit N" line with examples/NAME.expected (for
NAME-rejected, the compiler's diagnostic on stderr; NAME-tests runs as a test
suite with -t);
OKKAMA_BIBLE_CHECK_FILTER limits --check to example names containing that text.
OKKAMA_BIBLE_RUNNER supplies an isolated command runner in place of the macOS cage
(for example, a wrapper launching a bounded Docker container).
--stats prints characters and pages (2,500 characters) per chapter;
--issues lists every issue the chapters cite with its current title (gh).
"""
import json
import os
import re
import shlex
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
OKKAMA = os.environ.get("OKKAMA", os.path.join(ROOT, "okkama-rs", "target", "release", "okkama"))
# The std that matches OKKAMA: this checkout's, unless OKKAMA_STDLIB_DIR names another.
STDLIB = os.environ.get("OKKAMA_STDLIB_DIR", os.path.join(ROOT, "std"))
PAGE = 2500
LANGUAGES = {
    "en": ("OkkamaBible.md", "The Okkama Bible", "Contents", "Output", "Appendix E. References", "The compiler answers"),
    "ru": ("OkkamaBible-RU.md", "Библия Okkama", "Содержание", "Вывод", "Приложение E. Литература", "Ответ компилятора"),
}


def references():
    """key -> (label, entry) from references.md (`- [@key] Authors (year). ...`)."""
    found = {}
    for line in open(os.path.join(HERE, "references.md")):
        match = re.match(r"- \[@([\w-]+)\] (.+)$", line.strip())
        if match:
            key, entry = match.groups()
            # An explicit {label: ...} names organisations and standards.
            label = re.search(r"\s*\{label: ([^}]+)\}", entry)
            if label:
                entry = entry.replace(label.group(0), "")
                found[key] = (label.group(1), entry)
                continue
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
            label = LANGUAGES[language][5 if name.endswith("-rejected") else 3]
            block += f"\n\n{label}:\n\n```text\n{output}\n```"
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


def std_index_refresh():
    """data/std-index.json from `okkama man`: the public std modules by area."""
    text = subprocess.run([OKKAMA, "man", "--format", "markdown", "--no-pager"], capture_output=True, text=True,
                          timeout=120, env={**os.environ, "OKKAMA_STDLIB_DIR": STDLIB}, check=True).stdout
    version = subprocess.run([OKKAMA, "--version"], capture_output=True, text=True, timeout=30).stdout.strip()
    areas, area = {}, "top level"
    for line in text.splitlines():
        if line.startswith("## "):
            area = line[3:].strip()
        m = re.match(r"^- `([^`]+)`(?:: (.*))?$", line)
        if m and "internal" not in m.group(1).split("/"):
            areas.setdefault(area, []).append([m.group(1), (m.group(2) or "").strip()])
    data = {"version": version, "areas": areas}
    json.dump(data, open(os.path.join(HERE, "data", "std-index.json"), "w"), indent=1, ensure_ascii=False)
    print(f"std index: {sum(len(v) for v in areas.values())} public modules from {version}")


def std_index_table(language):
    """Appendix C: one table per area, from data/std-index.json."""
    data = json.load(open(os.path.join(HERE, "data", "std-index.json")))
    head = "| Module | What it is for |" if language == "en" else "| Модуль | Для чего |"
    out = []
    for area in sorted(data["areas"], key=lambda a: (a == "top level", a)):
        title = area if area != "top level" else ("top level" if language == "en" else "верхний уровень")
        out += [f"### `{area}`" if area != "top level" else f"### {title}", "", head, "|---|---|"]
        for path, summary in data["areas"][area]:
            out.append(f"| `{path}` | {summary.replace('|', chr(92) + '|')} |")
        out.append("")
    return "\n".join(out).rstrip()


def slug(title):
    return re.sub(r"[^\w\- ]", "", title.lower()).strip().replace(" ", "-")


def build(language, cited):
    target, title, contents, _, refs_title, _ = LANGUAGES[language]
    directory = os.path.join(HERE, "chapters", language)
    parts = []
    for name in sorted(os.listdir(directory)):
        if name.endswith(".md"):
            parts.append(open(os.path.join(directory, name)).read().rstrip() + "\n")
    text = "\n".join(parts)
    text = re.sub(r"\{\{example:([\w/-]+)\}\}", lambda m: example(m.group(1), language), text)
    def figure(match):
        name, caption = match.groups()
        localized = f"{name}-{language}.svg"
        file = localized if os.path.exists(os.path.join(HERE, "figures", localized)) else f"{name}.svg"
        return f"![{caption}](figures/{file})\n\n*{caption}*"

    text = re.sub(r"\{\{figure:([\w-]+)\|([^}]*)\}\}", figure, text)
    text = text.replace("{{table:builtins}}", builtins_table(language))
    if "{{table:stdlib}}" in text:
        text = text.replace("{{table:stdlib}}", std_index_table(language))
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


def engine_flag():
    """`--engine sparrow` for a toolchain that still takes it (0.5.8 defaulted to IR); 0.6.0 has one engine and
    no such option (owner, 2026-11-20)."""
    try:
        help_text = subprocess.run([OKKAMA, "run", "--help"], capture_output=True, text=True, timeout=30).stdout
    except (OSError, subprocess.SubprocessError):
        return []
    return ["--engine", "sparrow"] if "--engine" in help_text else []


# Examples whose doc tests start a process of their own (okkama,run).
UNCAGED = {"15-doc-examples-tests"}


def check():
    failures = 0
    flag = engine_flag()
    examples = os.path.join(HERE, "examples")
    for name in sorted(os.listdir(examples)):
        if not name.endswith(".ok"):
            continue
        stem = name[:-3]
        if os.environ.get("OKKAMA_BIBLE_CHECK_FILTER", "") not in stem:
            continue
        expected_path = os.path.join(examples, stem + ".expected")
        expected = open(expected_path).read().rstrip("\n").splitlines() if os.path.exists(expected_path) else None
        for engine in ("sparrow",):
            # Examples run in the repository's cage: no forks, network or
            # writes outside examples/, bounded CPU and memory.
            cage = [sys.executable, os.path.join(ROOT, "okkama-rs", "tools", "new-runtime", "caged.py"),
                    "--cpu-seconds", "30", "--timeout", "60", "--memory-mb", "1024", "--writable", examples, "--"]
            # An `okkama,run` doc example runs in a child process, which the
            # cage forbids; these few examples run outside it.
            if stem in UNCAGED:
                cage = []
            if runner := os.environ.get("OKKAMA_BIBLE_RUNNER"):
                cage = shlex.split(runner)
            # A `-tests` example is a test suite: its report is the output.
            tests = ["-t"] if stem.endswith("-tests") else []
            p = subprocess.run(cage + [OKKAMA, "run", *tests, *flag, name], cwd=examples,
                               capture_output=True, text=True, timeout=120,
                               env={**os.environ, "OKKAMA_STDLIB_DIR": STDLIB, "OKKAMA_TRACE": "off"})
            # A rejected example is shown with the compiler's diagnostic; a
            # module the example imports is named relative to examples/.
            shown = (p.stderr if stem.endswith("-rejected") else p.stdout).replace(examples + os.sep, "")
            output = shown.rstrip("\n").splitlines() if shown else []
            if p.returncode != 0 and not stem.endswith("-rejected"):
                output.append(f"exit {p.returncode}")
            if expected is None:
                print(f"{stem} [{engine}]: no .expected; printed {output} {p.stderr.strip()[:200]}")
                failures += 1
            elif output != expected:
                print(f"{stem} [{engine}]: printed {output}, expected {expected}; {p.stderr.strip()[:300]}")
                failures += 1
        print(f"{stem}: checked")
    return failures


def issues():
    """Every `issue #N` the chapters cite, with its title, to review by eye."""
    cited = {}
    for language in LANGUAGES:
        directory = os.path.join(HERE, "chapters", language)
        for name in sorted(os.listdir(directory)):
            for number in re.findall(r"#(\d+)", open(os.path.join(directory, name)).read()):
                cited.setdefault(int(number), set()).add(f"{language}/{name}")
    for number in sorted(cited):
        title = subprocess.run(["gh", "issue", "view", str(number), "--json", "title,state", "-q", '.state + " " + .title'],
                               capture_output=True, text=True).stdout.strip() or "?"
        print(f"#{number:<5} {title[:90]:<92} {', '.join(sorted(cited[number]))}")


KEYWORD_LIKE = re.compile(r"^[a-z_]+$")


def highlight_okkama(source, cache):
    """HTML for one Okkama listing, coloured by the compiler's own lexer."""
    if source in cache:
        return cache[source]
    import html
    raw = source.encode()
    # The lexer accepts only ASCII outside literals; a listing may hold "…" or
    # Cyrillic in a comment. Same-length ASCII stand-ins keep the byte offsets.
    probe = "".join(c if ord(c) < 128 else "_" * len(c.encode()) for c in source)
    p = subprocess.run([OKKAMA, "tokens", "-", "--format", "json"], input=probe, capture_output=True,
                       text=True, timeout=60)
    if p.returncode != 0:
        cache[source] = html.escape(source)
        return cache[source]
    out, pos = [], 0

    def gap(text):
        for part in re.split(r"(//[^\n]*|/\*.*?\*/)", text, flags=re.S):
            if part.startswith("//") or part.startswith("/*"):
                out.append(f'<span class="c">{html.escape(part)}</span>')
            elif part:
                out.append(html.escape(part))

    for token in json.loads(p.stdout):
        start, end = token["span"]["start"], token["span"]["end"]
        if start < pos or end > len(raw):
            continue
        gap(raw[pos:start].decode())
        text = raw[start:end].decode()
        kind = token["kind"]
        if kind in ("STRING", "RAW_STRING", "CHAR"):
            css = "s"
        elif kind in ("INT", "FLOAT", "TRUE", "FALSE"):
            css = "n"
        elif kind == "IDENTIFIER":
            css = "t" if text[:1].isupper() else None
        elif KEYWORD_LIKE.match(text):
            css = "k"
        else:
            css = None
        out.append(f'<span class="{css}">{html.escape(text)}</span>' if css else html.escape(text))
        pos = end
    gap(raw[pos:].decode())
    cache[source] = "".join(out)
    return cache[source]


PDF_TITLES = {
    "en": ("The language, rule by rule, with what each rule costs", "Okkama {version} · 2026"),
    "ru": ("Язык, правило за правилом, и цена каждого правила", "Okkama {version} · 2026"),
}


def pdf():
    """The print edition (PDF) and the web edition (one HTML file) in both languages.

    pandoc turns the generated Markdown into an HTML fragment and WeasyPrint
    lays out pages, both in the okkama-bible-pdf image (pdf/Dockerfile); the
    layout between them is pdf/render.py, on the host."""
    sys.path.insert(0, os.path.join(HERE, "pdf"))
    import render
    work = os.path.join(HERE, "pdf", "work")
    os.makedirs(work, exist_ok=True)
    cache = {}
    version = subprocess.run([OKKAMA, "--version"], capture_output=True, text=True).stdout.split()[-1]

    def docker(script):
        subprocess.run(["docker", "run", "--rm", "-v", f"{HERE}:/book", "okkama-bible-pdf", "sh", "-c", script],
                       check=True)

    for language, (target, title, *_rest) in LANGUAGES.items():
        text = open(os.path.join(HERE, target)).read()
        text = re.sub(r"^<!--.*?-->\n", "", text)

        def listing(match):
            body = highlight_okkama(match.group(2), cache)
            return f'{match.group(1)}<pre class="okkama"><code>{body}</code></pre>'

        text = re.sub(r"^( *)```okkama\n(.*?)\n *```$", listing, text, flags=re.S | re.M)
        # A figure's caption is the emphasised line right after its image.
        text = re.sub(r"(!\[[^\]]*\]\(figures/[^)]+\))\n\n\*([^*\n]+)\*",
                      r'\1\n\n<p class="caption">\2</p>', text)
        open(os.path.join(work, f"{language}.md"), "w").write(text)
        docker(f"pandoc pdf/work/{language}.md -f gfm -t html5 -o pdf/work/{language}.fragment.html")
        fragment = open(os.path.join(work, f"{language}.fragment.html")).read()
        fragment, heads = render.structure(fragment, language)
        subtitle, edition = PDF_TITLES[language]
        edition = edition.format(version=version)
        open(os.path.join(work, f"{language}.print.html"), "w").write(
            render.print_edition(fragment, heads, language, title, subtitle, edition))
        web = render.web_edition(fragment, heads, language, title, subtitle, edition)
        web_name = target.replace(".md", ".html")
        open(os.path.join(HERE, web_name), "w").write(web)
        pdf_name = target.replace(".md", ".pdf")
        docker(f"cp pdf/work/{language}.print.html ./.print-{language}.html && "
               f"weasyprint -q .print-{language}.html {pdf_name}; status=$?; rm -f .print-{language}.html; exit $status")
        print(f"{pdf_name}: {os.path.getsize(os.path.join(HERE, pdf_name)) // 1024} KiB, "
              f"{web_name}: {len(web) // 1024} KiB")

if __name__ == "__main__":
    if "--figures" in sys.argv:
        for script in sorted(os.listdir(os.path.join(HERE, "figures", "src"))):
            if script.endswith(".py"):
                subprocess.run([sys.executable, os.path.join(HERE, "figures", "src", script)], check=True)
    if "--std-index" in sys.argv:
        std_index_refresh()
    cited = set()
    for language in LANGUAGES:
        build(language, cited)
    if "--stats" in sys.argv:
        stats()
    if "--issues" in sys.argv:
        issues()
    if "--pdf" in sys.argv:
        pdf()
    if "--check" in sys.argv:
        sys.exit(1 if check() else 0)
