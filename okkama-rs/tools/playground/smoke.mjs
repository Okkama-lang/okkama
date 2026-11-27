// The playground's two modules under Node (#1125): okkama-wasm builds a program, sparrow-wasm runs it. A run reads
// its standard input from the request, sleeps two seconds on the virtual timeline without waiting for them (the
// second piece is marked 2 s later, the call returns at once), and exits 3; a failure's report and stack come
// back from failureText as `okkama run` prints them; a program that needs the file system is refused before it
// runs, and the CLDR tables are named as files to fetch. With an output sink (#1134) every write reaches the page while
// the program runs: a run in a worker that prints and then spins is stopped, and what it printed has arrived; the
// interruption text comes from version(). requirements names what a run can reach, and what is declared (#1135).
// A test build's tests run as `okkama run -t` runs them, the report streamed (#1127); digests, HMAC, HKDF, a named
// UUID and checksums give what the CLI gives.
//
//     wasm-bindgen --target nodejs --out-dir DIR okkama_wasm.wasm sparrow_wasm.wasm (one call each)
//     node okkama-rs/tools/playground/smoke.mjs DIR
import { createRequire } from "node:module";
import { Worker } from "node:worker_threads";
import { dirname, join, resolve } from "node:path";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";

const dir = resolve(process.argv[2] ?? "playground-node");
const require = createRequire(import.meta.url);
const compiler = require(join(dir, "okkama_wasm.js"));
const runtime = require(join(dir, "sparrow_wasm.js"));

function build(source, tests = false) {
    const built = JSON.parse(compiler.build(JSON.stringify({ source, path: "main.ok", tests })));
    assert.equal(built.ok, true, built.stderr);
    return compiler.builtSpb();
}

const hello = `module smoke v1.0.0;

import "io/stdout" take Stdout, Stderr;
import "io/stdin" take Stdin;
import "time/clock" take SystemClock;
import "time/duration" take Duration;

async main() is int {
    Stdout().writeln("hello " + Stdin().line());
    await SystemClock().wait(Duration(seconds: 2.rat()));
    Stderr().writeln("after");
    3;
}
`;
const started = performance.now();
const ran = JSON.parse(runtime.run(build(hello), JSON.stringify({ stdin: "world\n" })));
const elapsed = performance.now() - started;
assert.equal(ran.exit, 3, JSON.stringify(ran));
// A write is a piece with its moment (writeln writes the text and the newline apart); the page joins a stream's
// consecutive pieces as it shows them.
const joined = [];
for (const piece of ran.output) {
    const last = joined.at(-1);
    if (last && last.stream === piece.stream) last.text += piece.text;
    else joined.push({ ...piece });
}
assert.deepEqual(joined.map((piece) => [piece.stream, piece.text]), [["stdout", "hello world\n"], ["stderr", "after\n"]]);
const lastOut = ran.output.filter((piece) => piece.stream === "stdout").at(-1);
assert.ok(joined[1].t - lastOut.t >= 2e9, `the sleep is on the timeline: ${JSON.stringify(ran.output)}`);
assert.ok(elapsed < 1500, `the sleep is not waited for: ${elapsed} ms`);
assert.equal(ran.report, null);

const failing = `module smoke v1.0.0;

object Divider {
    Divider() {}

    divide(by is int) is int {
        10 % by;
    }
}

main() is int {
    Divider().divide(0);
}
`;
const failed = JSON.parse(runtime.run(build(failing), "{}"));
assert.equal(failed.exit, 1);
const text = JSON.parse(compiler.failureText(JSON.stringify({ report: failed.report, uncaught: failed.uncaught })));
assert.equal(text.text, `RuntimeError: Modulo by zero
 --> main.ok:7:9
  |
7 |         10 % by;
  |         ^^^^^^^
Stack trace (most recent call first):
  at Divider.divide (main.ok:7:9)
  at main (main.ok:12:5)
`);

const files = `module smoke v1.0.0;

import "fs/file" take File;
import "fs/path" take Path;

main() is int {
    let file = File(path: Path("notes.txt"));
    defer file.close();
    file.text().content().length();
}
`;
const needs = JSON.parse(runtime.requirements(build(files)));
assert.ok(needs.refused.length > 0 && needs.refused.every((symbol) => symbol.startsWith("runtime/")), JSON.stringify(needs));
assert.ok(needs.refused.every((symbol) => needs.declared.includes(symbol)), JSON.stringify(needs));
const profile = JSON.parse(runtime.version()).profile;
assert.equal(profile.name, "playground");
assert.equal(profile.interrupted, "Execution interrupted\n");
assert.equal(profile.stopped, "Execution stopped\n");
assert.equal(profile.exhausted.steps, "Execution budget exhausted: steps\n");
assert.deepEqual(JSON.parse(runtime.lastPanic()), { panic: null });

// Streamed: the chunks arrive through the sink before the answer, which then has no output.
const streamed = [];
const answer = JSON.parse(runtime.run(build(hello), JSON.stringify({ stdin: "you\n" }), (chunk) => streamed.push(JSON.parse(chunk))));
assert.equal(answer.exit, 3);
assert.deepEqual(answer.output, []);
assert.equal(streamed.map((chunk) => chunk.text).join(""), "hello you\nafter\n");
assert.ok(streamed.every((chunk) => ["stdout", "stderr"].includes(chunk.stream) && Number.isInteger(chunk.t)));

// Batched (#1136): 6000 lines arrive in a few chunks, handed over at the budget polls; window 0 is a chunk per write.
const many = build(`module smoke v1.0.0;

import "io/stdout" take Stdout;

main() is int {
    let out = Stdout();
    for 0..6000 -> n {
        out.writeln(n.string());
    }
    0;
}
`);
const expected = Array.from({ length: 6000 }, (_, n) => `${n}\n`).join("");
for (const [window, fewer] of [[undefined, true], [0, false]]) {
    const got = [];
    runtime.run(many, JSON.stringify(window === undefined ? {} : { window }), (chunk) => got.push(JSON.parse(chunk)));
    assert.equal(got.map((chunk) => chunk.text).join(""), expected);
    assert.ok(fewer ? got.length < 100 : got.length === 12000, `window ${window}: ${got.length} chunks`);
}

// Stopped: a worker prints, then spins; the page stops it once the line has come, and the line is there.
const spin = `module smoke v1.0.0;

import "io/stdout" take Stdout;

main() is int {
    Stdout().writeln("started");
    for 0..1000000000000 -> n {
        confirm n >= 0;
    }
    0;
}
`;
const worker = new Worker(`
    const { parentPort, workerData } = require("node:worker_threads");
    const compiler = require(workerData.dir + "/okkama_wasm.js");
    const runtime = require(workerData.dir + "/sparrow_wasm.js");
    compiler.build(JSON.stringify({ source: workerData.spin, path: "main.ok" }));
    const answer = runtime.run(compiler.builtSpb(), "{}", (chunk) => parentPort.postMessage(chunk));
    parentPort.postMessage(JSON.stringify({ finished: JSON.parse(answer) }));
`, { eval: true, workerData: { dir, spin } });
const stopped = await new Promise((resolve, reject) => {
    const seen = [];
    worker.on("error", reject);
    worker.on("message", (message) => {
        const chunk = JSON.parse(message);
        if (chunk.finished) return reject(new Error(`the run ended before it was stopped: ${message}`));
        seen.push(chunk);
        // writeln writes the text and the newline apart: stop once the line is complete.
        if (seen.map((piece) => piece.text).join("").endsWith("\n")) worker.terminate().then(() => resolve(seen));
    });
});
assert.ok(stopped.every((chunk) => chunk.stream === "stdout"), JSON.stringify(stopped));
const shown = stopped.map((chunk) => chunk.text).join("") + profile.stopped;
assert.equal(shown, "started\nExecution stopped\n");
// Tests: the report a line at a time, as `okkama run -t` prints it, exit 1 for a failed test.
const tested = `module smoke v1.0.0;

import "io/stdout" take Stdout;

main() is int {
    0;
}

test "adds" {
    Stdout().writeln("adding");
    test.is(1 + 1, 2);
}

test "fails" {
    test.is(1 + 1, 3);
}
`;
const report = [];
const verdict = JSON.parse(runtime.test(build(tested, true), "{}", (chunk) => report.push(JSON.parse(chunk))));
assert.equal(verdict.exit, 1, JSON.stringify(verdict));
assert.equal(report.map((chunk) => chunk.text).join(""),
    "=== RUN   adds\nadding\n--- PASS: adds\n=== RUN   fails\n--- FAIL: fails\n    expected 3, got 2\nFAIL\n");
assert.equal(JSON.parse(runtime.test(build(hello), "{}")).exit, 4, "a build without tests");

// Digests, HMAC, HKDF, a named UUID and checksums in wasm (RustCrypto, the design's #6).
const digests = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../../crates/okkama-cli/tests/playground/digests/main.ok"), "utf8");
const hashed = JSON.parse(runtime.run(build(digests), "{}"));
assert.equal(hashed.exit, 0, JSON.stringify(hashed));
assert.equal(hashed.output.map((chunk) => chunk.text).join(""), `sha256 ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
hmac 9c196e32dc0175f86f4b1cb89289d6619de6bee699e4c378e68309ed97a1a6ab
hkdf eab314a8ddb1ef18ba219b262f6e604b
uuid5 cfbff0d1-9375-5685-968c-48ce8b15ae17
crc32 2286445522
adler32 19726487
`);

console.log(`playground smoke: ok (run ${elapsed.toFixed(0)} ms, ${needs.refused.length} of ${needs.declared.length} declared refused natives reachable, a stopped run kept its output)`);
