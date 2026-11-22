#!/usr/bin/env python3
"""XML documents for the native-vs-Okkama tokenizer differential (#13): valid
documents built from every construct, their mutations, hand-written edge cases.
Writes records `depth events|document` separated by SEP.

    generate.py SEED COUNT OUT [BAD]
"""
import random, sys

SEP = "@@@@DOC@@@@"
rng = random.Random(int(sys.argv[1]))
COUNT = int(sys.argv[2])
BAD = float(sys.argv[4]) if len(sys.argv) > 4 else 0.05

def name():
    first = rng.choice(["a", "b", "item", "x", "_u", "ns:el", "Ä", "чай", "A1", ":c"])
    rest = rng.choice(["", "", "1", "-b", ".c", "_d", "é"])
    n = first + rest
    if rng.random() < BAD:
        n = rng.choice(["1a", "-x", ".y", "a b", ""])
    return n

def reference():
    good = ["&amp;", "&lt;", "&gt;", "&quot;", "&apos;", "&#65;", "&#x41;", "&#x1F600;", "&#233;", "&#x+41;", "&#+65;",
            "&#xFFFD;", "&#9;", "&#10;", "&#13;", "&#x10FFFF;", "&#0065;"]
    bad = ["&bogus;", "&#0;", "&#xD800;", "&#xFFFE;", "&#x110000;", "&#99999999999;", "&#x;", "&#;", "&#X41;", "&amp",
           "& ;", "&#-1;", "&#x+;", "&#1;"]
    return rng.choice(bad) if rng.random() < BAD * 2 else rng.choice(good)

def chars(attribute):
    pool = ["text", " ", "Tea", "чай", "😀", "a > b", "]]", "x]y", "\t", "\n", "\r\n", "\r", "'", "\""]
    if not attribute:
        pool += ["]]>"] if rng.random() < BAD else []
    out = []
    for _ in range(rng.randint(0, 4)):
        r = rng.random()
        if r < 0.3:
            out.append(reference())
        elif r < 0.33 and rng.random() < BAD * 4:
            out.append(rng.choice(["\x01", "\x0b", "￾", "\x7f", "<" if attribute else "\x1f"]))
        else:
            out.append(rng.choice(pool))
    return "".join(out)

def attributes():
    parts = []
    names = []
    for _ in range(rng.choice([0, 0, 1, 2, 3])):
        n = rng.choice(["id", "kind", "xmlns", "xmlns:p", "p:a", "lang", "v"])
        if n in names and rng.random() > BAD * 4:
            continue
        names.append(n)
        q = rng.choice(['"', "'"])
        value = chars(True).replace(q, "")
        if rng.random() < BAD:
            value += q
        space = rng.choice([" ", "\t", "\n", "  ", "\r\n"])
        if rng.random() < BAD:
            space = ""
        eq = rng.choice(["=", " = ", "=\n"])
        parts.append(space + n + eq + q + value + q)
    return "".join(parts)

def element(depth):
    n = name()
    attrs = attributes()
    if rng.random() < 0.2 or depth > 4:
        return "<" + n + attrs + rng.choice(["/>", " />", "/>"])
    body = []
    for _ in range(rng.randint(0, 4)):
        r = rng.random()
        if r < 0.35:
            body.append(chars(False))
        elif r < 0.6:
            body.append(element(depth + 1))
        elif r < 0.7:
            body.append("<![CDATA[" + rng.choice(["", "<raw>&amp;", "a]b", "x\r\ny\rz", "]]"]) + "]]>")
        elif r < 0.8:
            body.append("<!--" + rng.choice(["", " c ", "a-b", "é", "-x", " a -"]) + rng.choice(["-->", "-->", "--->"]))
        elif r < 0.9:
            body.append("<?" + rng.choice(["pi", "php", "target", "xml-stylesheet", "xmlfoo"]) +
                        rng.choice(["", " data", "  a=1 ", "\nmore"]) + "?>")
        else:
            body.append(rng.choice(["\n  ", " ", "\r\n"]))
    close = n if rng.random() > BAD else name()
    return "<" + n + attrs + ">" + "".join(body) + "</" + close + rng.choice([">", " >", "\n>"]) 

def prolog():
    parts = []
    if rng.random() < 0.1:
        parts.append("﻿")
    if rng.random() < 0.5:
        parts.append(rng.choice(['<?xml version="1.0"?>', "<?xml version='1.0' encoding='UTF-8'?>", "<?XML version='1.0'?>"]))
    if rng.random() < 0.3:
        parts.append(rng.choice(["\n", "\r\n", " "]))
    if rng.random() < 0.3:
        parts.append(rng.choice(['<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">',
                                 "<!DOCTYPE a>", "<!DOCTYPE a SYSTEM 'x[y].dtd'>", "<!DOCTYPE a [<!ENTITY e 'x'>]>"]))
    if rng.random() < 0.3:
        parts.append(rng.choice(["<!-- top -->", "<?pi outside?>", "\n"]))
    return "".join(parts)

def document():
    text = prolog() + element(0) + rng.choice(["", "\n", "<!-- end -->", " \n"])
    if rng.random() < BAD:
        text += rng.choice(["<b/>", "x", "<!DOCTYPE a>", "<![CDATA[x]]>"])
    return text

NOISE = ["<", ">", "/", "&", ";", "\"", "'", "=", " ", "\n", "\r", "!", "?", "-", "[", "]", "a", "\x00", "\x01", "é",
         "<!--", "-->", "<![CDATA[", "]]>", "<?", "?>", "</", "/>", "&amp;", "﻿"]

def mutate(text):
    for _ in range(rng.randint(1, 3)):
        at = rng.randint(0, len(text))
        r = rng.random()
        if r < 0.4:
            text = text[:at] + rng.choice(NOISE) + text[at:]
        elif r < 0.7:
            text = text[:at] + text[at + 1:]
        else:
            text = text[:at] + rng.choice(NOISE) + text[at + 1:]
    return text

EDGES = ["", " ", "<a/>", "<a></a>", "<a>", "</a>", "<a><b></a>", "<a/><b/>", "<a></a><b/>", "x<a/>", "<a/>x",
         "<a/> \n\t", "<a x='1' x='2'/>", "<a>&bogus;</a>", "<!DOCTYPE a [<!ENTITY e 'x'>]><a/>",
         "<a/><!DOCTYPE a>", "<!DOCTYPE a", "<!DOCTYPE a '[' >", "<a>&#0;</a>", "<a>]]></a>", "<a>x]]>y</a>",
         "<![CDATA[x]]><a/>", "<a><![CDATA[x</a>", "<a><!-- x</a>", "<a><!-- a -- b --></a>", "<a><!-- a ---></a>",
         "<a><!----></a>", "<a><!-----></a>", "<a><?pi</a>", "<a><?xml version='1.0'?></a>", " <?xml version='1.0'?><a/>",
         "﻿<?xml version='1.0'?><a/>", "<?xml version='1.0'?>﻿<a/>", "<?xmlx?><a/>", "<?xml?><a/>",
         "<??><a/>", "<a><? x?></a>", "<a><?x?y?></a>", "<a b='1'c='2'/>", "<a b/>", "<a b=1/>", "<a b='1/>",
         "<a b='<'/>", "<a b='&lt;'/>", "<a b='x\ty\nz\r\nw\rv'/>", "<a b='&#9;&#10;&#13;'/>",
         "<a>1\r\n2\r3&#13;<![CDATA[4\r\n]]></a>", "<a>x<!--c-->y</a>", "<a>x<![CDATA[y]]>z</a>", "<a><b>x</b>y</a>",
         "<a\n>t</a\n>", "</>", "<>", "<a></ a>", "<a></a >", "<a></a\t>", "<a><b/></a><!--after--><?pi after?>",
         "<1/>", "<a:b:c/>", "<é/>", "<a-b.c_d/>", "<a>&#x1F600;&#128512;</a>", "<a>\x01</a>", "<a>￾</a>",
         "<a b='\x01'/>", "<a>&amp</a>", "<a>& b;</a>", "<a>&#x+41;&#+65;</a>", "<a>&#4294967295;</a>",
         "<a>&#4294967296;</a>", "<a>&#X41;</a>", "<!--top--><a/>", "<a/><!-- end", "<a>" * 3 + "</a>" * 3,
         "<a><a><a></a></a></a>", "<!DOCTYPE a><!DOCTYPE b><a/>", "<a><!DOCTYPE b></a>", "<a b='1' />",
         "<a\tb\t=\t'1'\t/>", "<a b = \"x'y\" c='x\"y'/>", "<a>\n</a>", "<a/>\x00"]

def limits():
    if rng.random() < 0.85:
        return (256, 1048576)
    return (rng.choice([1, 2, 3, 5]), rng.choice([1, 2, 3, 5, 8, 20]))

records = []
for text in EDGES:
    records.append(((256, 1048576), text))
    records.append(((2, 4), text))
records.append(((256, 1048576), "<a>" + "<b/>" * 5000 + "</a>"))
records.append(((256, 5000), "<a>" + "<b/>" * 5000 + "</a>"))
records.append(((256, 1048576), "<a>" * 256 + "</a>" * 256))
records.append(((256, 1048576), "<a>" * 257 + "</a>" * 257))
while len(records) < COUNT:
    text = document()
    if rng.random() < 0.3:
        text = mutate(text)
    records.append((limits(), text))
with open(sys.argv[3], "w", encoding="utf-8", newline="") as out:
    out.write(SEP.join(f"{d} {e}|{t}" for (d, e), t in records))
print(len(records), "documents")
