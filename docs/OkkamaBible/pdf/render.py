"""Layout of the Okkama Bible: the HTML fragment pandoc makes from the generated
Markdown becomes a print edition (for WeasyPrint) and a web edition (one file
with a sidebar of contents and a dark theme). Both share book.css."""

import html
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))

WORDS = {
    "en": {"part": "Part", "chapter": "Chapter", "appendix": "Appendix", "contents": "Contents",
           "why": ("Why.",), "cost": ("Cost.",), "lang": "en", "other": ("Русская версия", "OkkamaBible-RU.html")},
    "ru": {"part": "Часть", "chapter": "Глава", "appendix": "Приложение", "contents": "Содержание",
           "why": ("Почему.", "Зачем."), "cost": ("Цена.",), "lang": "ru", "other": ("English", "OkkamaBible.html")},
}


def strip_tags(text):
    return " ".join(html.unescape(re.sub(r"<[^>]+>", "", text)).split())


def ornament(kind, css="orn"):
    """A placeholder for an ornament; each edition resolves ORN: to a path or a data URI."""
    path = os.path.join(HERE, "ornaments", kind + ".svg")
    return f'<img class="{css}" src="ORN:{kind}" alt="">' if os.path.exists(path) else ""


def plate(number):
    """The chapter's miniature, if one is drawn: ornaments/plate-NN.svg."""
    img = ornament(f"plate-{number}", "plate-img")
    return f'<div class="plate">{img}</div>' if img else ""


def drolleries():
    return sorted(f[:-4] for f in os.listdir(os.path.join(HERE, "ornaments"))
                  if f.startswith("drollery-") and f.endswith(".svg")) if os.path.isdir(os.path.join(HERE, "ornaments")) else []


def resolve(body, web):
    import base64

    def one(match):
        path = os.path.join(HERE, "ornaments", match.group(1) + ".svg")
        if not web:
            return f'src="pdf/ornaments/{match.group(1)}.svg"'
        return 'src="data:image/svg+xml;base64,' + base64.b64encode(open(path, "rb").read()).decode() + '"'

    body = re.sub(r'src="ORN:([\w-]+)"', one, body)
    if web:
        def figure(match):
            path = os.path.join(HERE, "..", "figures", match.group(1))
            return 'src="data:image/svg+xml;base64,' + base64.b64encode(open(path, "rb").read()).decode() + '"'
        body = re.sub(r'src="figures/([\w.-]+\.svg)"', figure, body)
    return body


def structure(fragment, language):
    """Headings, notes, tables and initials, shared by both editions."""
    w = WORDS[language]
    heads = []

    def h1(match):
        attrs, inner = match.group(1), match.group(2)
        text = strip_tags(inner)
        ident = re.search(r'id="([^"]*)"', attrs)
        ident = ident.group(1) if ident else ""
        for kind in ("part", "chapter", "appendix"):
            m = re.match(rf"^{w[kind]} ([0-9A-ZIVX]+)\.\s+(.*)$", text)
            if m:
                label, title = m.groups()
                heads.append((1, kind, ident, f"{w[kind]} {label}", title))
                body = f'<small>{w[kind]} {label}</small>{html.escape(title)}'
                extra = ""
                if kind == "part":
                    extra = f'<div class="part-ornament">{ornament("part")}</div>'
                if kind == "chapter":
                    extra = plate(label.zfill(2))
                running = html.escape(f"{w[kind]} {label} · {title}" if kind != "part" else "")
                return f'<h1 class="{kind}" id="{ident}" data-running="{running}">{body}</h1>{extra}'
        heads.append((1, "other", ident, "", text))
        return f'<h1 class="other" id="{ident}" data-running="{html.escape(text)}">{inner}</h1>'

    fragment = re.sub(r"<h1([^>]*)>(.*?)</h1>", h1, fragment, flags=re.S)
    tails = drolleries()
    if tails:
        state = {"previous": None, "used": 0}

        def tail(match):
            kind, previous = match.group(1), state["previous"]
            state["previous"] = kind
            if previous not in ("chapter", "appendix"):
                return match.group(0)
            name = tails[state["used"] % len(tails)]
            state["used"] += 1
            return f'<div class="tailpiece">{ornament(name, "drollery")}</div>' + match.group(0)

        fragment = re.sub(r'<h1 class="(chapter|appendix|part|other)"', tail, fragment)

    def h2(match):
        attrs, inner = match.group(1), match.group(2)
        ident = re.search(r'id="([^"]*)"', attrs)
        ident = ident.group(1) if ident else ""
        inner = " ".join(inner.split())
        m = re.match(r"^([0-9A-Z]+\.[0-9]+)\s+(.*)$", inner, flags=re.S)
        heads.append((2, "section", ident, m.group(1) if m else "", strip_tags(m.group(2) if m else inner)))
        if m:
            return f'<h2 id="{ident}"><span class="num">{m.group(1)}</span> {m.group(2)}</h2>'
        return f'<h2 id="{ident}">{inner}</h2>'

    fragment = re.sub(r"<h2([^>]*)>(.*?)</h2>", h2, fragment, flags=re.S)
    fragment = re.sub(r"<table>", '<div class="table-wrap"><table>', fragment)
    # A paragraph right before a listing or a table stays on its page (print).
    fragment = re.sub(r'<p>((?:(?!</p>).)*)</p>(\s*<(?:div class="sourceCode"|pre|div class="table-wrap"))',
                      r'<p class="lead-in">\1</p>\2', fragment, flags=re.S)
    fragment = re.sub(r"</table>", "</table></div>", fragment)
    for kind in ("why", "cost"):
        for word in w[kind]:
            fragment = fragment.replace(f"<p><strong>{word}</strong>", f'<p class="note {kind}"><strong>{word}</strong>')

    # An illuminated initial opens the first paragraph of every chapter.
    def initial(match):
        head, between, letter, rest = match.groups()
        return f'{head}{between}<p class="opening"><span class="initial">{letter}</span>{rest}'

    fragment = re.sub(r'(<h1 class="chapter"[^>]*>.*?</h1>)((?:(?!<p>|<h).)*?)<p>((?:[«"“]|&quot;)?[A-Za-zА-Яа-яЁё])([^<]*)',
                      initial, fragment, flags=re.S)
    return fragment, heads


def title_page(title, subtitle, edition):
    return (f'<section class="title-page"><div class="title-ornament">{ornament("title")}</div>'
            f'<p class="eyebrow">Okkama</p><p class="book-title">{html.escape(title)}</p>'
            f'<p class="book-subtitle">{html.escape(subtitle)}</p><p class="book-edition">{html.escape(edition)}</p></section>')


def print_edition(fragment, heads, language, title, subtitle, edition):
    """For WeasyPrint: the contents list after the title already carries page numbers."""
    body = re.sub(r'<h1 class="other"[^>]*>' + re.escape(title) + r"</h1>",
                  title_page(title, subtitle, edition), fragment, count=1)
    return (f'<!doctype html><html lang="{language}"><head><meta charset="utf-8"><title>{html.escape(title)}</title>'
            f'<link rel="stylesheet" href="pdf/book.css"><link rel="stylesheet" href="pdf/print.css"></head>'
            f'<body class="print">{resolve(body, False)}</body></html>')


def web_edition(fragment, heads, language, title, subtitle, edition):
    """One file: a sidebar of contents, light and dark themes, fonts from Google Fonts."""
    w = WORDS[language]
    body = re.sub(r'<h1 class="other"[^>]*>' + re.escape(title) + r"</h1>",
                  title_page(title, subtitle, edition), fragment, count=1)
    # The Markdown's own contents list is for print; the sidebar replaces it.
    body = re.sub(r'<h2 id="(contents|содержание)">.*?</h2>\s*<ul>.*?</ul>\s*(?=<h1)', "", body, count=1, flags=re.S)
    items = []
    for level, kind, ident, label, text in heads:
        if level == 1 and kind in ("part", "chapter", "appendix"):
            css = "toc-part" if kind == "part" else "toc-chapter"
            num = label.split()[-1]
            items.append(f'<li class="{css}"><a href="#{ident}"><b>{num}</b>{html.escape(text)}</a></li>')
        elif level == 1 and kind == "other" and text != title:
            items.append(f'<li class="toc-chapter"><a href="#{ident}"><b>E</b>{html.escape(text.split(". ", 1)[-1])}</a></li>')
    css = open(os.path.join(HERE, "book.css")).read() + open(os.path.join(HERE, "web.css")).read()
    initial = os.path.join(HERE, "ornaments", "initial.svg")
    if os.path.exists(initial):
        import base64
        css += ("p.opening .initial { background-image: url(data:image/svg+xml;base64,"
                + base64.b64encode(open(initial, "rb").read()).decode() + "); background-size: cover; }")
    other_name, other_file = w["other"]
    return f"""<!doctype html>
<html lang="{language}"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html.escape(title)}</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Golos+Text:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500;600&display=swap">
<style>{css}</style></head>
<body class="web"><div class="shell">
<nav class="toc"><p class="eyebrow">{w['contents']}</p><p class="toc-lang"><a href="{other_file}">{other_name}</a></p><ol>{''.join(items)}</ol></nav>
<main>{resolve(body, True)}</main></div></body></html>
"""
