#!/usr/bin/env python3
"""The release report of the micro-benchmarks (protocol v2): the previous
release against the candidate, one construct or std action per row.

    micro/report.py --old OLD.json [OLD2.json ...] --new NEW.json [NEW2.json ...] \
        -o OUT.html

OLD and NEW are `run.py` schema-2 campaigns; each version may span several
files (multiple rounds per engine). A release comparison changes the compiler and std
together, so the std hashes may differ; every other part of the campaign must
match (suite hash, protocol, N, warmup, quality policy, image, limits, host),
or the pair is reported INCOMPATIBLE. The metric is gross ns/iteration, as
run.py reports it. Round medians are combined geometrically for each version.
A pair counts only when every round on both sides is present and VALID; then a change
is real when it exceeds 10% and twice the larger full-range spread (the rule
of `run.py --compare`: a heuristic, not statistical significance).
TEXTS.json holds the report's words: eyebrow, title, lead, findings
([[title, text], ...]), priorities, method, commands, foot. The page takes its
head and styles from ../report-template.html, the standard report.
"""
import argparse, json, re, statistics
from pathlib import Path

HERE = Path(__file__).resolve().parent
ENGINE_NAMES = {"sparrow": "Tomato", "ir": "IR", "jvm": "JVM"}
SAME = ["suite_hash", "protocol", "n", "warmup", "quality_policy", "placements"]


def campaign_key(result):
    env = {k: v for k, v in result.get("environment", {}).items() if k != "stdlib_hashes"}
    return {**{k: result.get(k) for k in SAME}, "environment": env}


def merge(files):
    meta = []
    if len({Path(path).resolve() for path in files}) != len(files):
        raise ValueError("duplicate campaign file")
    for path in files:
        result = json.loads(Path(path).read_text())
        if result.get("schema") != 2:
            raise SystemExit(f"{path}: not a schema-2 campaign")
        meta.append(result)
    versions = {result.get("toolchain", {}).get("version") for result in meta}
    versions.discard(None)
    if len(versions) > 1:
        raise ValueError("one side of a report contains several toolchain versions: " + ", ".join(sorted(versions)))
    merged = {}
    for result in meta:
        for name, row in result["benchmarks"].items():
            for engine in row:
                # Every campaign containing this engine is one round, including
                # rounds missing this particular benchmark.
                merged.setdefault(name, {})[engine] = [
                    (round_["benchmarks"].get(name, {}).get(engine, {"status": "MISSING"}), round_)
                    for round_ in meta if not round_["benchmarks"] or any(engine in r for r in round_["benchmarks"].values())
                ]
    return merged, meta


def aggregate(rounds):
    cells = [cell for cell, _ in rounds]
    valid = all(cell["status"] == "VALID" and result.get("status", "COMPLETE") == "COMPLETE"
                for cell, result in rounds)
    values = [cell.get("ns_per_iteration") for cell in cells]
    measured = all(isinstance(value, (int, float)) and value > 0 for value in values)
    return {"status": "VALID" if valid and measured else "INCONCLUSIVE",
            "ns_per_iteration": statistics.geometric_mean(values) if measured else None,
            "spread": max((cell.get("spread", 0) for cell in cells), default=0),
            # #933: the spread of the per-placement medians, when the campaign ran under --placements.
            "layout": max((cell["layout_spread"] for cell in cells if "layout_spread" in cell), default=None),
            "binary": binary_spread(rounds) if measured else None,
            "rounds": len(rounds)}


def binary_spread(rounds):
    """#1086: the spread of the per-binary medians when one version ran on several builds of the same source
    (toolchain.sh LAYOUT_SEED: another function order), or None with one binary."""
    by_binary = {}
    for cell, result in rounds:
        by_binary.setdefault(result.get("toolchain", {}).get("binary_sha256"), []).append(cell["ns_per_iteration"])
    if len(by_binary) < 2 or None in by_binary:
        return None
    medians = [statistics.geometric_mean(values) for values in by_binary.values()]
    return (max(medians) - min(medians)) / statistics.geometric_mean(medians)


def cells(old_files, new_files):
    old, old_meta = merge(old_files)
    new, new_meta = merge(new_files)
    rows = []
    for name in sorted(new):
        group = "std" if (HERE / "std" / f"{name}.ok").exists() else "lang"
        entry = {"name": name, "group": group, "engines": {}}
        for engine, rounds in new[name].items():
            if engine not in old.get(name, {}):
                continue
            bases = old[name][engine]
            before, cell = aggregate(bases), aggregate(rounds)
            all_rounds = bases + rounds
            out = {"before": before["ns_per_iteration"], "after": cell["ns_per_iteration"],
                   "spreadBefore": before["spread"], "spreadAfter": cell["spread"],
                   "layoutBefore": before["layout"], "layoutAfter": cell["layout"],
                   "binaryBefore": before["binary"], "binaryAfter": cell["binary"],
                   "statusBefore": before["status"], "statusAfter": cell["status"],
                   "roundsBefore": len(bases), "roundsAfter": len(rounds),
                   "n": rounds[0][1]["n"]}
            if any(campaign_key(result) != campaign_key(bases[0][1]) for _, result in all_rounds) or any(
                    result.get("toolchain", {}).get("version_check", {}).get("status") == "MISMATCH"
                    for _, result in all_rounds) or any(
                    result.get("toolchain", {}).get("devtrace") for _, result in all_rounds):
                out["verdict"] = "incompatible"
            elif before["status"] != "VALID" or cell["status"] != "VALID":
                out["verdict"] = "inconclusive"
            else:
                change = cell["ns_per_iteration"] / before["ns_per_iteration"] - 1
                # The binaries' own layout (#1086) widens the band like a noisy series.
                band = max(0.10, 2 * max(cell["spread"], before["spread"], cell["binary"] or 0, before["binary"] or 0))
                out.update(change=change, band=band,
                           verdict="faster" if change < -band else "slower" if change > band else "noise")
            if out.get("change") is None and out["before"] and out["after"]:
                out["change"] = out["after"] / out["before"] - 1
            entry["engines"][engine] = out
        rows.append(entry)
    return rows, old_meta, new_meta


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--old", nargs="+", required=True)
    parser.add_argument("--new", nargs="+", required=True)
    parser.add_argument("texts")
    parser.add_argument("-o", "--out", required=True)
    args = parser.parse_args()
    texts = json.loads(Path(args.texts).read_text())
    rows, old_meta, new_meta = cells(args.old, args.new)
    engines = [e for e in ("sparrow", "ir", "jvm") if any(e in r["engines"] for r in rows)]
    label = lambda meta: meta[0]["toolchain"]["version"]
    data = {"old": label(old_meta), "new": label(new_meta), "runs": new_meta[0]["runs"],
            "n": {e: next(m["n"] for m in new_meta if any(e in r for r in m["benchmarks"].values())) for e in engines},
            "engines": [{"id": e, "name": ENGINE_NAMES[e]} for e in engines], "rows": rows,
            "files": {"old": [Path(f).name for f in args.old], "new": [Path(f).name for f in args.new]}}
    template = (HERE.parent / "report-template.html").read_text()
    style = re.search(r"<style>.*?</style>", template, re.S).group(0)
    head = template[:template.index("<style>")].replace("<title>Okkama Engine Benchmarks</title>",
                                                        "<title>Okkama Micro Benchmarks</title>")
    page = head + style + PAGE.replace("/*DATA*/null", json.dumps(data, ensure_ascii=False)) \
        .replace("/*TEXTS*/null", json.dumps(texts, ensure_ascii=False))
    Path(args.out).write_text(page)
    print(f"wrote {args.out}")


PAGE = r"""
<style>
.diverge { display: grid; gap: 4px; }
.drow { display: grid; grid-template-columns: minmax(0, 13rem) minmax(0, 1fr); gap: 12px; align-items: center; font-size: 13px; }
.drow .label { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--mono); }
.dtrack { position: relative; height: 18px; background: var(--soft); border-radius: 4px; }
.dtrack .mid { position: absolute; left: 50%; top: -2px; bottom: -2px; width: 1px; background: var(--muted); }
.dtrack .band { position: absolute; top: 0; bottom: 0; background: color-mix(in srgb, var(--muted) 18%, transparent); }
.dtrack i { position: absolute; top: 3px; bottom: 3px; border-radius: 3px; }
.dtrack em { position: absolute; top: 0; font-style: normal; font-size: 11px; line-height: 18px; font-family: var(--mono); color: var(--ink); white-space: nowrap; }
.chip.noise { opacity: .75; }
@media (max-width: 560px) { .drow { grid-template-columns: minmax(0, 8rem) minmax(0, 1fr); } }
</style>
<div class="wrap">
  <header style="display:grid;gap:14px">
    <div class="eyebrow" id="eyebrow"></div>
    <h1 id="title"></h1>
    <p class="lead" id="lead"></p>
    <div class="legend" id="legend"></div>
  </header>

  <section>
    <div class="tiles" id="tiles"></div>
    <p class="muted" style="font-size:13px" id="tilesNote"></p>
  </section>

  <section>
    <h2>За полосой шума</h2>
    <p class="muted" id="flaggedLead"></p>
    <div class="table-wrap"><table id="flagged"></table></div>
  </section>

  <section>
    <h2>По конструкциям</h2>
    <div class="panel">
      <div class="controls">
        <div class="seg" role="group" aria-label="Движок" id="engine"></div>
        <div class="seg" role="group" aria-label="Набор" id="group"></div>
      </div>
      <div class="diverge" id="chart"></div>
      <div class="axis-note" id="axis"></div>
    </div>
  </section>

  <section>
    <h2>Все программы</h2>
    <p class="muted" id="allLead"></p>
    <div class="table-wrap"><table id="all"></table></div>
  </section>

  <section>
    <h2>Что изменилось и почему</h2>
    <div class="panel findings" id="findings"></div>
  </section>

  <section>
    <h2>Что смотреть дальше</h2>
    <ol class="priorities" id="priorities"></ol>
  </section>

  <section>
    <h2>Как измерено</h2>
    <div class="panel">
      <p class="muted" style="font-size:14px" id="method"></p>
      <pre id="commands"></pre>
    </div>
  </section>

  <footer id="foot"></footer>
</div>

<script>
const DATA = /*DATA*/null;
const TEXTS = /*TEXTS*/null;
const $ = id => document.getElementById(id);
const geo = xs => Math.exp(xs.reduce((s, x) => s + Math.log(x), 0) / xs.length);
const pct = c => `${c > 0 ? "+" : ""}${(c * 100).toFixed(1)}%`;
const ns = v => v === null || v === undefined ? "—" : v >= 1000 ? (v / 1000).toFixed(2) + " µs" : v.toFixed(1) + " ns";
const name = id => DATA.engines.find(e => e.id === id).name;
let engine = DATA.engines[0].id, group = "all";

$("eyebrow").textContent = TEXTS.eyebrow;
$("title").textContent = TEXTS.title;
$("lead").innerHTML = TEXTS.lead;
$("legend").innerHTML = `<span><i class="swatch" style="background:var(--good)"></i>быстрее за полосой шума</span>` +
  `<span><i class="swatch" style="background:var(--bad)"></i>медленнее за полосой шума</span>` +
  `<span><i class="swatch" style="background:var(--muted)"></i>в шуме</span>`;

const CONTROL = n => n === "baseline" || n === "baseline-async";
const COMPARED = c => c.verdict === "faster" || c.verdict === "slower" || c.verdict === "noise";
function chip(c) {
  const v = c.change === undefined || c.change === null ? "" : pct(c.change);
  if (c.verdict === "faster") return `<span class="chip good">${v}</span>`;
  if (c.verdict === "slower") return `<span class="chip bad">${v}</span>`;
  if (c.verdict === "noise") return `<span class="chip even noise">${v}</span>`;
  if (c.verdict === "incompatible") return `<span class="chip even" title="кампании несовместимы">несовместимо</span>`;
  return `<span class="chip even" title="${c.statusBefore} / ${c.statusAfter}: короткая серия или разброс больше 10%">неубедительно ${v}</span>`;
}
const spread = c => `±${((c.spreadBefore || 0) * 100).toFixed(0)}% / ±${((c.spreadAfter || 0) * 100).toFixed(0)}%` + layout(c);
// #933: the part of the spread that follows the placement of machine code (run.py --placements).
const layout = c => (c.layoutBefore == null && c.layoutAfter == null ? "" :
  ` <span class="muted" title="разброс медиан по раскладкам машинного кода">раскладка ±${((c.layoutBefore || 0) * 100).toFixed(0)}% / ±${((c.layoutAfter || 0) * 100).toFixed(0)}%</span>`) +
  (c.binaryBefore == null && c.binaryAfter == null ? "" :
  ` <span class="muted" title="разброс медиан по сборкам с разным порядком функций (#1086)">бинарник ±${((c.binaryBefore || 0) * 100).toFixed(0)}% / ±${((c.binaryAfter || 0) * 100).toFixed(0)}%</span>`);

function tiles() {
  const items = [];
  for (const e of DATA.engines) {
    const all = DATA.rows.filter(r => r.engines[e.id] && !CONTROL(r.name)).map(r => r.engines[e.id]);
    const cs = all.filter(COMPARED);
    const g = geo(cs.map(c => c.after / c.before)) - 1;
    const faster = cs.filter(c => c.verdict === "faster").length, slower = cs.filter(c => c.verdict === "slower").length;
    items.push([`${pct(g)}`, `${e.name}: геосреднее ns/iteration`, `${DATA.new} к ${DATA.old} по ${cs.length} надёжным парам из ${all.length}; меньше — лучше.`]);
    items.push([`${faster} <small>/</small> ${slower}`, `${e.name}: быстрее / медленнее`, `За полосой шума; в шуме ${cs.length - faster - slower}, неубедительных ${all.length - cs.length}.`]);
  }
  $("tiles").innerHTML = items.map(([big, t, note]) => `<div class="tile"><div class="eyebrow">${t}</div><div class="big">${big}</div><p>${note}</p></div>`).join("");
  $("tilesNote").innerHTML = `Каждое число — медиана ${DATA.runs} запусков, каждый в своём ограниченном контейнере; в запуске ${Object.entries(DATA.n).map(([k, v]) => `${v.toLocaleString("ru")} итераций (${name(k)})`).join(", ")} после прогрева в десятую часть. Пара надёжна, если обе серии VALID: не короче 10 мс и с разбросом не больше 10%. Контрольные <code>baseline</code> и <code>baseline-async</code> в геосреднее не входят.`;
}

function flagged() {
  const list = [];
  for (const r of DATA.rows) for (const e of DATA.engines) { const c = r.engines[e.id]; if (c && (c.verdict === "faster" || c.verdict === "slower")) list.push([r, e, c]); }
  list.sort((a, b) => a[2].change - b[2].change);
  $("flaggedLead").textContent = `Все раунды VALID, изменение больше 10% и больше удвоенного разброса (максимум/минимум − 1) — правило run.py --compare; эвристика, не статистическая значимость. Время — полное на итерацию, с циклом и контрольной суммой.`;
  $("flagged").innerHTML = list.length ? `<thead><tr><th>Программа</th><th>Движок</th><th>${DATA.old}</th><th>${DATA.new}</th><th>Изменение</th><th>Разброс</th></tr></thead><tbody>` +
    list.map(([r, e, c]) => `<tr><td><code>${r.name}</code> <span class="muted">${r.group}</span></td><td>${e.name}</td><td class="num">${ns(c.before)}</td><td class="num">${ns(c.after)}</td><td>${chip(c)}</td><td class="num">${spread(c)}</td></tr>`).join("") + "</tbody>"
    : `<tbody><tr><td>Ничего: все изменения в шуме.</td></tr></tbody>`;
}

function buttons(id, options, current, set) {
  $(id).innerHTML = options.map(([key, n]) => `<button type="button" data-key="${key}" aria-pressed="${key === current}">${n}</button>`).join("");
  $(id).querySelectorAll("button").forEach(b => b.addEventListener("click", () => { set(b.dataset.key); chart(); }));
}

function chart() {
  buttons("engine", DATA.engines.map(e => [e.id, e.name]), engine, k => engine = k);
  buttons("group", [["all", "Все"], ["lang", "Язык"], ["std", "std"]], group, k => group = k);
  const rows = DATA.rows.filter(r => r.engines[engine] && typeof r.engines[engine].change === "number" && (group === "all" || r.group === group))
    .sort((a, b) => a.engines[engine].change - b.engines[engine].change);
  const limit = Math.max(0.25, Math.min(1, Math.max(...rows.map(r => Math.abs(r.engines[engine].change))) * 1.1));
  const x = c => 50 + 50 * Math.max(-1, Math.min(1, c / limit));
  $("chart").innerHTML = rows.map(r => {
    const c = r.engines[engine], band = c.band || Math.max(0.10, 2 * Math.max(c.spreadBefore || 0, c.spreadAfter || 0));
    const color = c.verdict === "faster" ? "var(--good)" : c.verdict === "slower" ? "var(--bad)" : c.verdict === "noise" ? "var(--muted)" : "var(--line)";
    const left = Math.min(50, x(c.change)), width = Math.abs(x(c.change) - 50);
    const label = c.change < 0 ? `right:${(100 - left + 1).toFixed(1)}%` : `left:${(left + width + 1).toFixed(1)}%`;
    return `<div class="drow" title="${r.name}: ${ns(c.before)} → ${ns(c.after)} (${pct(c.change)}, разброс ${spread(c)}${COMPARED(c) ? "" : ", неубедительно"})">` +
      `<div class="label">${r.name}${COMPARED(c) ? "" : " ?"}</div><div class="dtrack"><span class="band" style="left:${x(-band).toFixed(1)}%;width:${(x(band) - x(-band)).toFixed(1)}%"></span>` +
      `<span class="mid"></span><i style="left:${left.toFixed(1)}%;width:${width.toFixed(1)}%;background:${color}"></i><em style="${label}">${pct(c.change)}</em></div></div>`;
  }).join("");
  $("axis").textContent = `Изменение ns/iteration ${DATA.new} к ${DATA.old}, шкала ±${Math.round(limit * 100)}%; влево — быстрее. Серая полоса — полоса шума этой программы; бледная полоса и «?» — пара неубедительна (хотя бы один раунд не VALID), её изменение не учитывается.`;
}

function all() {
  $("allLead").textContent = `Полное ns/iteration, ${DATA.old} → ${DATA.new}, и изменение; серым — в пределах шума, «неубедительно» — одна из серий короче 10 мс или с разбросом больше 10%. Файлы: ${DATA.files.old.join(", ")} → ${DATA.files.new.join(", ")}.`;
  $("all").innerHTML = `<thead><tr><th>Программа</th>${DATA.engines.map(e => `<th>${e.name}</th><th></th>`).join("")}</tr></thead><tbody>` +
    DATA.rows.map(r => `<tr><td><code>${r.name}</code> <span class="muted">${r.group}</span></td>${DATA.engines.map(e => { const c = r.engines[e.id];
      return c ? `<td class="num">${ns(c.before)} → ${ns(c.after)}</td><td>${chip(c)}</td>` : `<td class="muted">—</td><td></td>`; }).join("")}</tr>`).join("") + "</tbody>";
}

$("findings").innerHTML = (TEXTS.findings || []).map(([t, d]) => `<div class="finding"><div class="tag">${t}</div><div><p>${d}</p></div></div>`).join("");
$("priorities").innerHTML = (TEXTS.priorities || []).map(([t, d]) => `<li><div><h3>${t}</h3><p class="muted">${d}</p></div></li>`).join("");
$("method").innerHTML = (TEXTS.method || "") + "<br>Несколько раундов одной версии и движка объединены геометрическим средним ns/iteration. Пара учитывается, только если все её раунды присутствуют, совместимы и VALID; разброс — наибольший среди раундов.";
$("commands").textContent = TEXTS.commands;
$("foot").textContent = TEXTS.foot || "";
tiles(); flagged(); chart(); all();
</script>
"""

if __name__ == "__main__":
    main()
