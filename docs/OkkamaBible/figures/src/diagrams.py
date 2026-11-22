"""Schemes of the Okkama Bible in the style of the Okkama documents: rounded
boxes tinted by meaning (data blue, resources orange, views violet, success
green, failure red), arrows and short labels. Every scheme is written once
with its English and Russian text and saved as figures/NAME.svg and
figures/NAME-ru.svg."""
import html
import os

OUT = os.path.join(os.path.dirname(__file__), "..")
INK, MUTED, LINE, SOFT, SURFACE = "#16202b", "#5d6b7a", "#dde2e8", "#eef1f5", "#ffffff"
KINDS = {  # stroke, fill
    "plain": ("#2f6fb3", "#eaf1f9"), "res": ("#c2571a", "#fbeee6"), "view": ("#6b5fb0", "#efedf8"),
    "good": ("#1f7a46", "#e8f3ec"), "bad": ("#b3261e", "#f9e9e8"), "soft": (LINE, SOFT), "box": (LINE, SURFACE),
    "gold": ("#9c7a12", "#fbf4dc"), "accent": ("#0f7f86", "#e5f3f3"),
}
FONT = "Golos Text, Segoe UI, Helvetica, Arial, sans-serif"
MONO = "JetBrains Mono, SF Mono, Menlo, monospace"


class Diagram:
    def __init__(self, name, width, height):
        self.name, self.width, self.height, self.items = name, width, height, []

    def box(self, x, y, w, h, kind, title=None, lines=(), mono=False, radius=12, title_size=14):
        stroke, fill = KINDS[kind]
        self.items.append(("rect", x, y, w, h, stroke, fill, radius))
        ty = y + 24
        if title:
            self.items.append(("text", x + 16, ty, title, INK, 700, title_size, FONT, "start"))
            ty += 20
        for line in lines:
            self.items.append(("text", x + 16, ty, line, MUTED, 400, 12, MONO if mono else FONT, "start"))
            ty += 18
        return self

    def pill(self, x, y, w, kind, label, mono=True):
        stroke, fill = KINDS[kind]
        self.items.append(("rect", x, y, w, 26, stroke, fill, 13))
        self.items.append(("text", x + w / 2, y + 17.5, label, INK, 600, 12, MONO if mono else FONT, "middle"))
        return self

    def text(self, x, y, label, colour=MUTED, weight=400, size=12, mono=False, anchor="start"):
        self.items.append(("text", x, y, label, colour, weight, size, MONO if mono else FONT, anchor))
        return self

    def arrow(self, points, kind="soft", dashed=False, label=None, at=None):
        stroke = MUTED if kind == "soft" else KINDS[kind][0]
        self.items.append(("path", points, stroke, dashed))
        if label:
            lx, ly = at if at else ((points[0][0] + points[-1][0]) / 2, (points[0][1] + points[-1][1]) / 2 - 8)
            self.items.append(("text", lx, ly, label, stroke if kind != "soft" else MUTED, 500, 11.5, MONO, "middle"))
        return self

    def svg(self, language):
        out = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {self.width} {self.height}" '
               f'width="{self.width}" height="{self.height}" role="img">',
               f'<rect width="{self.width}" height="{self.height}" rx="14" fill="{SURFACE}" stroke="{LINE}"/>']
        markers = {}
        for item in self.items:
            if item[0] == "rect":
                _, x, y, w, h, stroke, fill, r = item
                out.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" fill="{fill}" stroke="{stroke}" stroke-width="1.6"/>')
            elif item[0] == "text":
                _, x, y, label, colour, weight, size, family, anchor = item
                label = label[language == "ru"] if isinstance(label, tuple) else label
                out.append(f'<text x="{x}" y="{y}" fill="{colour}" font-family="{family}" font-size="{size}" '
                           f'font-weight="{weight}" text-anchor="{anchor}">{html.escape(label)}</text>')
            elif item[0] == "path":
                _, points, stroke, dashed = item
                marker = markers.setdefault(stroke, f"m{len(markers)}")
                d = "M" + " L".join(f"{x},{y}" for x, y in points)
                dash = ' stroke-dasharray="5 4"' if dashed else ""
                out.append(f'<path d="{d}" fill="none" stroke="{stroke}" stroke-width="1.8"{dash} marker-end="url(#{marker})"/>')
        defs = "".join(f'<marker id="{m}" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" '
                       f'orient="auto"><path d="M0,0 L10,5 L0,10 z" fill="{s}"/></marker>' for s, m in markers.items())
        out.insert(2, f"<defs>{defs}</defs>")
        out.append("</svg>")
        return "\n".join(out)

    def save(self):
        for language, suffix in (("en", ""), ("ru", "-ru")):
            open(os.path.join(OUT, f"{self.name}{suffix}.svg"), "w").write(self.svg(language))


def t(en, ru):
    return (en, ru)


# 7.3 — the four ways out of a Maybe.
d = Diagram("maybe-exits", 760, 250)
d.box(24, 92, 170, 64, "view", "Maybe<T>", [t("some(value) or none", "some(value) или none")], mono=False)
exits = [("good", "else", t("a fallback, lazy", "запасное, лениво"), "names else \"nobody\""),
         ("plain", "match", t("a case for each", "ветка на каждый"), "case Maybe.some(n): …"),
         ("res", "else throw / panic", t("absence is a failure", "отсутствие — отказ"), "else throw Missing.user(id)"),
         ("accent", "try?", t("an error becomes none", "ошибка становится none"), "try? users.require(4)")]
for i, (kind, title, line, code) in enumerate(exits):
    y = 14 + i * 58
    d.box(300, y, 436, 50, kind, None)
    d.text(316, y + 21, title, INK, 700, 13, mono=True)
    d.text(316, y + 39, line, MUTED, 400, 11.5)
    d.text(722, y + 30, code, MUTED, 400, 11, mono=True, anchor="end")
    d.arrow([(194, 124), (250, 124), (250, y + 25), (298, y + 25)], kind if i != 3 else "soft")
d.save()

# 10.1 / 10.4 — two channels for two kinds of trouble.
d = Diagram("failure-channels", 760, 300)
d.text(24, 30, t("Expected failure: a value", "Ожидаемый отказ — значение"), INK, 700, 14)
d.box(24, 44, 160, 56, "bad", "throw", [t("an error enum case", "вариант enum ошибки")])
d.box(240, 44, 210, 56, "box", t("scopes unwind", "области раскручиваются"), [t("defer, destroy", "defer, destroy")])
d.box(506, 44, 230, 56, "good", t("catch e is LineError", "catch e is LineError"), [t("the caller decides", "решает вызывающий")])
d.arrow([(184, 72), (238, 72)], "bad")
d.arrow([(450, 72), (504, 72)], "good")
d.text(24, 150, t("A bug: not a value", "Ошибка программы — не значение"), INK, 700, 14)
d.box(24, 164, 160, 74, "bad", "panic", [t("bare confirm,", "голый confirm,"), t("runtime error", "ошибка исполнения")])
d.box(240, 164, 210, 74, "box", t("every scope unwinds", "все области раскручиваются"), [t("defer, destroy,", "defer, destroy,"), t("newest first", "с новейшего")])
d.box(506, 164, 230, 74, "soft", t("report, exit 1", "отчёт, выход 1"), [t("no catch can see it", "ни один catch не увидит")])
d.arrow([(184, 201), (238, 201)], "bad")
d.arrow([(450, 201), (504, 201)], "soft")
d.box(24, 252, 712, 36, "gold", None)
d.text(40, 275, t("Budget stop (--max-steps, --timeout): the run ends at once; no catch, no destroy.",
                  "Остановка по бюджету (--max-steps, --timeout): запуск кончается сразу; ни catch, ни destroy."), INK, 500, 12.5)
d.save()

# 11.3 / 11.4 — what runs when a block ends.
d = Diagram("scope-exit", 760, 250)
d.text(24, 30, t("Inside the block, in order", "Внутри блока, по порядку"), INK, 700, 14)
steps = [("res", "let a = Handle(\"a\")"), ("res", "let b = Handle(\"b\")"), ("accent", "defer log(\"one\")"), ("accent", "defer log(\"two\")")]
for i, (kind, code) in enumerate(steps):
    d.pill(24 + i * 182, 44, 168, kind, code)
for i in range(3):
    d.arrow([(192 + i * 182, 57), (204 + i * 182, 57)])
d.text(24, 124, t("When the block ends, on every exit: defers first, then owned objects, newest first",
                  "Когда блок кончается, на любом выходе: сначала defer, потом объекты, с новейшего"), INK, 700, 14)
done = [("accent", "two"), ("accent", "one"), ("res", "close b"), ("res", "close a")]
for i, (kind, label) in enumerate(done):
    d.pill(24 + i * 182, 140, 168, kind, label)
for i in range(3):
    d.arrow([(192 + i * 182, 153), (204 + i * 182, 153)])
d.text(24, 204, t("Normal end, return, last, next, a thrown error, a panic, a cancelled task: the same order.",
                  "Обычный конец, return, last, next, брошенная ошибка, паника, отменённая задача — порядок один."), MUTED, 400, 12.5)
d.text(24, 226, t("A budget stop is the only exit that runs nothing (Section 10.13).",
                  "Остановка по бюджету — единственный выход, где не выполняется ничего (раздел 10.13)."), MUTED, 400, 12.5)
d.save()

# 11.5 — who owns what.
d = Diagram("ownership", 760, 300)
d.box(24, 20, 712, 262, "soft", t("main's block owns", "блок main владеет"), [], radius=14)
d.box(48, 70, 200, 70, "res", "let a = Tag(\"a\")", [t("owned here", "владеет этот блок")], mono=False)
d.box(48, 170, 200, 70, "view", "let alias = a", [t("a second name: borrows", "второе имя: заём")])
d.arrow([(148, 168), (148, 142)], "view", dashed=True)
d.box(300, 70, 200, 70, "plain", "look(kept)", [t("a parameter borrows", "параметр берёт взаймы")])
d.box(300, 170, 200, 70, "res", "adopt(Tag(\"t\"))", [t("fresh argument: adopted,", "свежий аргумент усыновлён,"), t("closed when adopt ends", "закрыт по концу adopt")])
d.box(536, 70, 180, 70, "good", "make() is Tag", [t("the result moves", "результат переезжает"), t("to the caller", "к вызывающему")])
d.box(536, 170, 180, 70, "res", "let made = make()", [t("owned here now", "теперь владеет блок")])
d.arrow([(626, 140), (626, 168)], "good")
d.text(380, 270, t("At the end: made, then a, newest first. alias and look close nothing.",
                   "В конце: made, затем a, с новейшего. alias и look ничего не закрывают."), MUTED, 400, 12.5, anchor="middle")
d.save()

# 13.5 — a successor leaves the original intact.
d = Diagram("successor", 760, 200)
d.box(24, 40, 220, 110, "plain", "bottle", ["Bottle", "is_open() → false"], mono=True)
d.box(516, 40, 220, 110, "good", "opened", ["^Bottle", "is_open() → true"], mono=True)
d.arrow([(246, 95), (514, 95)], "good", label="bottle.open()", at=(380, 84))
d.text(380, 124, t("a new object; the old one is unchanged", "новый объект; старый не меняется"), MUTED, 400, 12, anchor="middle")
d.text(380, 184, t("Both stay usable. Dropping the successor is a compile-time error.",
                   "Пользоваться можно обоими. Выбросить преемника — ошибка компиляции."), INK, 500, 12.5, anchor="middle")
d.save()

# 14 — roles are nominal.
d = Diagram("roles", 760, 230)
d.box(270, 18, 220, 62, "view", "role Shape", ["area() is rat;"], mono=True)
d.box(24, 140, 200, 62, "plain", "Square does Shape", ["area() is rat"], mono=True)
d.box(280, 140, 200, 62, "plain", "Circle does Shape", ["area() is rat"], mono=True)
d.box(536, 140, 200, 62, "bad", "Blob", [t("area() is rat, no does", "area() is rat, без does")], mono=True)
d.arrow([(124, 138), (124, 110), (330, 110), (330, 82)], "view")
d.arrow([(380, 138), (380, 82)], "view")
d.arrow([(636, 138), (636, 110), (440, 110), (440, 82)], "bad", dashed=True)
d.text(636, 224, t("not a Shape: a name is not a contract", "не Shape: имя — не контракт"), "#b3261e", 500, 12, anchor="middle")
d.save()

# 20.5 — structured await carries cancellation.
d = Diagram("structured-await", 760, 260)
d.box(24, 30, 210, 70, "plain", t("task A", "задача A"), ["await fetch()"], mono=False)
d.box(300, 30, 210, 70, "plain", t("future B", "future B"), [t("started by the await", "запущен этим await")])
d.box(560, 30, 176, 70, "res", t("its resources", "его ресурсы"), ["async destroy"], mono=False)
d.arrow([(234, 65), (298, 65)], "soft", label="await", at=(266, 56))
d.arrow([(510, 65), (558, 65)])
d.box(24, 150, 210, 70, "bad", "A.cancel()", [t("A is cancelled", "A отменена")])
d.box(300, 150, 210, 70, "bad", t("B is cancelled", "B отменён"), [t("unless another task awaits it", "если её не ждёт другая задача")])
d.box(560, 150, 176, 70, "good", t("B settles", "B завершается"), [t("cleanup runs", "уборка выполнена")])
d.arrow([(234, 185), (298, 185)], "bad")
d.arrow([(510, 185), (558, 185)], "good")
d.arrow([(648, 222), (648, 246), (129, 246), (129, 222)], "good", label=t("only then: A resumes with Cancelled", "только тогда: A продолжает с Cancelled"), at=(390, 240))
d.save()

# 20.6 — tasks send values to the state owner.
d = Diagram("state-owner", 760, 240)
for i, name in enumerate(["request 1", "request 2", "request 3"]):
    d.box(24, 18 + i * 72, 170, 56, "plain", t(f"task: {name}", f"задача: {name}"), ["await send(…)"], mono=False)
    d.arrow([(194, 46 + i * 72), (276, 118)], "plain")
d.box(278, 82, 190, 72, "gold", "Channel<Command>", [t("bounded FIFO", "ограниченная очередь")], mono=False)
d.arrow([(468, 118), (530, 118)], "soft", label="item()", at=(499, 108))
d.box(532, 62, 204, 112, "res", t("the state owner", "владелец состояния"), [t("one task holds the state", "одна задача держит"), t("and applies commands", "состояние и применяет"), t("one at a time", "команды по одной")])
d.text(380, 226, t("No task writes another's state; they send values.", "Ни одна задача не пишет чужое состояние; задачи посылают значения."), MUTED, 400, 12.5, anchor="middle")
d.save()

# 21 — authority is a value; the host decides which doors exist.
d = Diagram("authority", 760, 260)
d.box(24, 24, 220, 210, "plain", t("the program", "программа"), [t("objects it was given:", "объекты, которые дали:"), "FileRoot", "HttpClient", "SystemClock", "SecureRandom"], mono=False)
groups = ["fs", "net", "process", "time", "random"]
for i, g in enumerate(groups):
    d.pill(300, 30 + i * 40, 150, "view" if g != "process" else "bad", f"natives: {g}")
    d.arrow([(244, 120), (298, 43 + i * 40)], "soft")
d.box(506, 24, 230, 120, "box", t("the host", "хост"), [t("files, sockets, processes,", "файлы, сокеты, процессы,"), t("clock, entropy", "часы, энтропия")])
d.arrow([(450, 83), (504, 83)])
d.box(506, 164, 230, 70, "bad", t("an embedder may deny", "встраивающий может запретить"), [t("a group, or set budgets", "группу или задать бюджеты")])
d.arrow([(506, 199), (452, 150)], "bad", dashed=True)
d.save()

# 19.3 / 22.9 — the layers of the standard library.
d = Diagram("std-layers", 760, 250)
d.box(24, 18, 712, 58, "soft", t("areas: http, web, db, mail, crypto, archive, csv, toml, regex, i18n, …",
                                 "области: http, web, db, mail, crypto, archive, csv, toml, regex, i18n, …"), [t("each imports the layers below it", "каждая импортирует слои под ней")])
d.box(24, 92, 712, 58, "plain", t("the foundation: collections, error, async, numbers, strings, time, fs, os, cli, url, json",
                                  "фундамент: collections, error, async, numbers, strings, time, fs, os, cli, url, json"), [t("reviewed first: a mistake here is everyone's", "проверен первым: ошибка здесь — ошибка всех")])
d.box(24, 166, 345, 64, "gold", "compare", ["Order, Comparing", t("imports nothing; 83 importers", "ничего не импортирует; 83 импортёра")], mono=False)
d.box(391, 166, 345, 64, "gold", "collections/iteration", ["Cursor, Iterating", t("imports nothing; 41 importers", "ничего не импортирует; 41 импортёр")], mono=False)
d.save()

# 7.7 — three states for a partial update.
d = Diagram("presence", 760, 210)
d.box(24, 70, 190, 64, "view", "Presence<string>", [t("a JSON PATCH field", "поле JSON PATCH")])
rows = [("good", "some(\"Ann\")", t("set it", "задать"), "{\"nick\": \"Ann\"}"),
        ("bad", "none", t("clear it", "очистить"), "{\"nick\": null}"),
        ("soft", "missing", t("leave it alone", "не трогать"), "{ }")]
for i, (kind, case, meaning, json) in enumerate(rows):
    y = 16 + i * 62
    d.box(290, y, 446, 50, kind, None)
    d.text(306, y + 22, case, INK, 700, 13, mono=True)
    d.text(306, y + 40, meaning, MUTED, 400, 11.5)
    d.text(722, y + 30, json, MUTED, 400, 12, mono=True, anchor="end")
    d.arrow([(214, 102), (252, 102), (252, y + 25), (288, y + 25)], kind if kind != "soft" else "soft")
d.save()

# 8.5 — a persistent collection shares its parts.
d = Diagram("persistent", 760, 230)
d.box(24, 24, 200, 60, "plain", "a is List<int>", [t("version 1: 1, 2, 3", "версия 1: 1, 2, 3")], mono=False)
d.box(24, 140, 200, 60, "good", "let b = a.prefix(0)", [t("version 2; a unchanged", "версия 2; a не изменился")], mono=False)
cells = ["1", "2", "3"]
for i, v in enumerate(cells):
    d.pill(380 + i * 120, 40, 80, "plain", v)
for i in range(2):
    d.arrow([(460 + i * 120, 53), (498 + i * 120, 53)], "plain")
d.pill(280, 156, 80, "good", "0")
d.arrow([(360, 162), (414, 70)], "good")
d.arrow([(224, 54), (378, 54)], "plain")
d.arrow([(224, 170), (278, 170)], "good")
d.text(560, 130, t("b shares 1, 2, 3 with a:", "b делит 1, 2, 3 с a:"), MUTED, 500, 12.5, anchor="middle")
d.text(560, 150, t("a new version costs one node", "новая версия стоит один узел"), MUTED, 500, 12.5, anchor="middle")
d.save()

# 9.5 — one loop, exits where the decision is made.
d = Diagram("loop-exit", 760, 220)
d.box(24, 70, 150, 60, "plain", "loop {", [t("a fresh scope", "свежая область")], mono=False)
d.box(220, 24, 230, 56, "accent", "last Maybe.none if …", [t("nothing left: leave", "нечего искать: выйти")], mono=False)
d.box(220, 120, 230, 56, "good", "last Maybe.some(m) if …", [t("found: leave with it", "нашли: выйти с ним")], mono=False)
d.box(500, 70, 236, 60, "box", t("narrow the window", "сузить окно"), ["window = window.above(m)"], mono=False)
d.arrow([(174, 100), (218, 52)], "accent")
d.arrow([(174, 100), (218, 148)], "good")
d.arrow([(450, 52), (498, 92)])
d.arrow([(618, 132), (618, 196), (99, 196), (99, 132)], "soft", label=t("the next iteration", "следующая итерация"), at=(360, 190))
d.save()

# 10.8 — a cause as a payload, and as a link in the report.
d = Diagram("causes", 760, 230)
d.box(24, 24, 330, 84, "bad", "OrderError.storage(cause)", [t("payload: the program can match it", "нагрузка: программа может её разобрать"), "cause is StoreError.closed"], mono=False)
d.box(406, 24, 330, 84, "bad", "OrderError.failed(7)", [t("thrown while handling StoreError", "брошена при обработке StoreError"), t("the link: for the report only", "связь: только для отчёта")], mono=False)
d.box(406, 136, 330, 74, "soft", "Caused by:", ["  StoreError: StoreError.closed", "  --> uncaught.ok:10:56"], mono=True)
d.arrow([(571, 108), (571, 134)], "soft", dashed=True)
d.text(189, 150, t("Choose the payload when the caller", "Нагрузка — когда вызывающему"), INK, 500, 12.5, anchor="middle")
d.text(189, 170, t("needs the cause to decide.", "нужна причина, чтобы решить."), INK, 500, 12.5, anchor="middle")
d.save()

# 12.8 — a cap is a named, read-only view.
d = Diagram("cap", 760, 210)
d.box(24, 24, 260, 160, "res", "Customer", ["has name, surname", "has card_number", "charge(sum) is ^Customer"], mono=True)
d.box(380, 24, 200, 86, "view", "Customer::BusinessCard", ["full_name", "initials"], mono=True)
d.box(380, 128, 200, 56, "plain", "Printer.print(card)", [t("reads card::full_name", "читает card::full_name")], mono=True)
d.arrow([(284, 66), (378, 66)], "view", label="customer::BusinessCard", at=(331, 56))
d.arrow([(480, 110), (480, 126)], "view")
d.box(612, 24, 124, 160, "bad", t("not reachable", "недоступно"), ["charge()", "card_number", t("the Customer", "сам Customer")], mono=True)
d.save()

# 16.3 — match covers every case.
d = Diagram("match-cover", 760, 200)
d.box(24, 60, 200, 64, "view", "enum Reading", ["single(v), pair(a, b), none"], mono=True)
for i, (kind, case) in enumerate([("good", "case Reading.single(v): …"), ("good", "case Reading.pair(a, b): …"), ("bad", "case Reading.none — ?")]):
    d.pill(290, 24 + i * 52, 270, kind, case)
    d.arrow([(224, 92), (288, 37 + i * 52)], kind)
d.box(590, 110, 146, 64, "bad", t("compile error", "ошибка компиляции"), [t("as a value: does not", "как значение: не"), t("cover none", "покрывает none")])
d.text(380, 188, t("A match used as a value must cover every case, or end with else.",
                   "match как значение обязан покрыть все варианты или закончиться else."), MUTED, 400, 12.5, anchor="middle")
d.save()

# 18.2 — a capture is a snapshot.
d = Diagram("capture", 760, 200)
d.pill(24, 30, 180, "plain", "counter: 1")
d.pill(24, 120, 180, "plain", "counter: 2")
d.arrow([(114, 56), (114, 118)], "soft", label="counter = counter.bumped()", at=(230, 92))
d.box(470, 18, 266, 60, "view", "early = () is int { … }", [t("captured counter: 1", "захвачен counter: 1")], mono=False)
d.box(470, 110, 266, 60, "view", "late = () is int { … }", [t("captured counter: 2", "захвачен counter: 2")], mono=False)
d.arrow([(204, 43), (468, 43)], "view", dashed=True)
d.arrow([(204, 133), (468, 133)], "view", dashed=True)
d.text(380, 192, t("Rebinding the name later does not change what a closure holds.",
                   "Перевязка имени позже не меняет того, что держит замыкание."), MUTED, 400, 12.5, anchor="middle")
d.save()

# 25.3 — typestate: each state is a role.
d = Diagram("typestate", 760, 200)
states = [("plain", "Created", "authorize()"), ("accent", "Authorized", "capture()"), ("good", "Captured", "refund()"), ("soft", "Refunded", "")]
for i, (kind, role, action) in enumerate(states):
    d.box(24 + i * 182, 40, 160, 70, kind, role, [action] if action else [t("no actions left", "действий нет")], mono=True)
    if i < 3:
        d.arrow([(184 + i * 182, 75), (204 + i * 182, 75)])
d.box(24, 136, 712, 44, "bad", None)
d.text(40, 163, t("created.refund() does not compile: the role Created has no refund.",
                  "created.refund() не компилируется: у роли Created нет refund."), "#b3261e", 600, 12.5)
d.save()

print("schemes written")
