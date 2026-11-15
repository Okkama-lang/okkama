#!/usr/bin/env python3
"""OCS adapter for the developing Rust implementation (okkama-rs).

Protocol: ocs-adapter/2 (conformance/adapters/PROTOCOL.md).

The Rust implementation is expected to grow a native protocol endpoint:

    okkama-rs --ocs-capabilities
        -> prints {"protocol": "ocs-adapter/2", "modes": ["parse", ...]}
    okkama-rs --ocs-request REQUEST.json --ocs-response RESPONSE.json
        -> behaves exactly like an ocs-adapter/2 adapter for one request

This adapter builds the crate (into a cache directory, never into the
implementation tree), asks the binary for its capabilities and forwards only
the modes it declares. Everything else is reported as UNSUPPORTED with the
observed evidence. It never fabricates parse/check/run results, and it never
treats a parser as a runtime.

Engines (`OCS_RUST_ENGINE`):

* `sparrow` (default): the default build (Tomato runtime); every mode is
  forwarded to the native endpoint.
* `champ`: `okkama-cli` built with `--features champ-abi` into its own target
  directory. The native endpoint has no OIR-2 path, so parse/check (the shared
  front end) are forwarded to it, while run/test go through the CLI exactly as
  a user runs a program on Champ: `OKKAMA_PIPELINE=oir2 okkama run [-t] ...`.
  A run first asks the native endpoint to *check* the same sources, which
  attributes front-end rejections to their stage and category; the CLI's
  streams pass through unchanged and its exit status and failure report are
  translated into the protocol (see `champ_outcome`).
"""

import json
import os
import re
import subprocess
import sys
import time

PROTOCOL = "ocs-adapter/2"
HERE = os.path.dirname(os.path.abspath(__file__))

SPARROW_BUILD = ["cargo", "build", "--quiet", "--offline"]
# Each engine is its own build: its own cargo features, its own target
# directory (artifacts are never shared) and its own probe cache entry.
ENGINES = {
    "sparrow": {"build": SPARROW_BUILD, "target_subdir": None, "pipeline": None},
    "champ": {"build": SPARROW_BUILD + ["-p", "okkama-cli", "--features", "champ-abi"],
              "target_subdir": "engine-champ", "pipeline": "oir2"},
}
REMOVED_ENGINES = ("ir", "jvm")
# Exit status of `okkama run` when OIR-2 lowering cannot handle the program.
EXIT_OIR2_UNSUPPORTED = 4


def impl_root():
    root = os.environ.get("OCS_RUST_IMPLEMENTATION_ROOT")
    if not root:
        raise SystemExit("OCS_RUST_IMPLEMENTATION_ROOT is not set")
    return os.path.abspath(root)


def engine_name():
    engine = os.environ.get("OCS_RUST_ENGINE") or "sparrow"
    if engine in ENGINES:
        return engine
    if engine in REMOVED_ENGINES:
        raise SystemExit("Sparrow is the only Okkama engine; the requested engine was removed")
    raise SystemExit("unknown OCS_RUST_ENGINE %r; expected one of: %s" % (engine, ", ".join(sorted(ENGINES))))


def target_dir(engine=None):
    base = os.path.abspath(os.environ.get("OCS_RUST_TARGET_DIR") or os.path.join(HERE, ".target"))
    subdir = ENGINES[engine or "sparrow"]["target_subdir"]
    return os.path.join(base, subdir) if subdir else base


def build_command(engine):
    return list(ENGINES[engine]["build"])


def binary_env(engine):
    """Environment of the implementation binary; None means "inherit unchanged"."""
    pipeline = ENGINES[engine]["pipeline"]
    if pipeline is None:
        return None
    env = dict(os.environ)
    env["OKKAMA_PIPELINE"] = pipeline
    # The binary's own OCS_RUST_ENGINE names its code generator family
    # (Sparrow); Champ is selected by OKKAMA_PIPELINE, not by this variable.
    env["OCS_RUST_ENGINE"] = "sparrow"
    return env


def run(cmd, cwd=None, timeout=600, env=None):
    try:
        p = subprocess.run(cmd, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                           stdin=subprocess.DEVNULL, timeout=timeout, env=env)
        return p.returncode, p.stdout.decode("utf-8", "replace"), p.stderr.decode("utf-8", "replace")
    except FileNotFoundError as exc:
        return 127, "", str(exc)
    except subprocess.TimeoutExpired:
        return 124, "", "timeout"


def git(root, *args):
    code, out, _ = run(["git", "-C", root] + list(args), timeout=30)
    return out.strip() if code == 0 else None


def source_fingerprint(root):
    """Hash of the implementation's build inputs; the cached probe is reused while it is unchanged."""
    import hashlib
    h = hashlib.sha256()
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in ("target", ".git"))
        for fname in sorted(filenames):
            # Embedded runtime data (include_bytes!) are build inputs too.
            if fname.endswith((".rs", ".toml", ".lock", ".dat")):
                full = os.path.join(dirpath, fname)
                h.update(os.path.relpath(full, root).encode() + b"\0")
                with open(full, "rb") as fh:
                    h.update(hashlib.sha256(fh.read()).digest())
    return h.hexdigest()


def probe():
    """Build and query the implementation once per source fingerprint and engine.

    Concurrent adapter invocations serialise on a lock file and reuse the
    cached evidence, so parallel cases neither race on cargo's target lock nor
    spend their time budget rebuilding an unchanged crate. Every engine has
    its own target directory, hence its own lock and cache file; the cache
    entry is also keyed by engine and build command.
    """
    import fcntl
    engine = engine_name()
    root = impl_root()
    tdir = target_dir(engine)
    os.makedirs(tdir, exist_ok=True)
    cache = os.path.join(tdir, "ocs-probe.json")
    fingerprint = source_fingerprint(root) if os.path.isdir(root) else None
    build = build_command(engine)
    with open(os.path.join(tdir, "ocs-probe.lock"), "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        engines = {}
        try:
            with open(cache) as fh:
                engines = json.load(fh).get("engines", {})
            cached = engines.get(engine, {})
            # Entries written before engines had build commands are Sparrow's.
            if (cached.get("fingerprint") == fingerprint and fingerprint is not None
                    and cached.get("engine") == engine and cached.get("build_exit") == 0
                    and cached.get("build_command", SPARROW_BUILD) == build
                    and cached.get("target_dir", tdir) == tdir):
                cached["cached"] = True
                return cached
        except (OSError, ValueError):
            pass
        ev = probe_uncached(engine)
        ev["fingerprint"] = fingerprint
        ev["engine"] = engine
        ev["build_command"] = build
        ev["target_dir"] = tdir
        # A failed build is not evidence of the sources: it may be the
        # environment (an offline registry missing a new crate); retry it.
        if ev.get("build_exit") == 0:
            engines[engine] = ev
            with open(cache, "w") as fh:
                json.dump({"engines": engines}, fh)
        return ev


def probe_uncached(engine="sparrow"):
    """Build and query the implementation. Returns a dict of evidence."""
    root = impl_root()
    ev = {"root": root, "exists": os.path.isdir(root)}
    if not ev["exists"]:
        ev["modes"] = []
        ev["reason"] = "implementation root does not exist"
        return ev
    tdir = target_dir(engine)
    ev["git_head"] = git(root, "rev-parse", "--verify", "-q", "HEAD")
    ev["git_status_entries"] = len((git(root, "status", "--porcelain") or "").splitlines())
    ev["rust_sources"] = sorted(os.path.relpath(os.path.join(d, f), root)
                                for d, _, fs in os.walk(os.path.join(root, "src")) for f in fs if f.endswith(".rs"))
    code, out, err = run(["cargo", "--version"])
    ev["cargo"] = out.strip() if code == 0 else "unavailable: " + err.strip()
    if code != 0:
        ev["modes"] = []
        ev["reason"] = "cargo is not available"
        return ev
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = tdir
    code, out, err = run(build_command(engine), cwd=root, env=env)
    ev["build_exit"] = code
    ev["build_stderr_tail"] = err[-2000:]
    if code != 0:
        ev["modes"] = []
        ev["reason"] = "cargo build failed"
        return ev
    name = "okkama-rs"
    try:
        import tomllib  # Python 3.11+
        with open(os.path.join(root, "Cargo.toml"), "rb") as fh:
            name = tomllib.load(fh)["package"]["name"]
    except Exception:
        for line in open(os.path.join(root, "Cargo.toml")):
            if line.strip().startswith("name"):
                name = line.split("=", 1)[1].strip().strip('"')
                break
    # A workspace has no [package]; its CLI crate builds the `okkama` binary.
    # Prefer it so a stale binary of an older single-crate layout is ignored.
    binary = os.path.join(tdir, "debug", "okkama")
    if not os.path.exists(binary):
        binary = os.path.join(tdir, "debug", name)
    ev["binary"] = binary
    code, out, err = run([binary, "--ocs-capabilities"], timeout=30, env=binary_env(engine))
    ev["capabilities_exit"] = code
    ev["capabilities_stdout"] = out[:2000]
    try:
        caps = json.loads(out)
        if caps.get("protocol") != PROTOCOL or not isinstance(caps.get("modes"), list):
            raise ValueError("wrong protocol")
        ev["modes"] = [m for m in caps["modes"] if m in ("parse", "check", "run", "test")]
        ev["reason"] = None
    except Exception:
        ev["modes"] = []
        ev["reason"] = ("binary does not implement the ocs-adapter/2 capability query "
                        "(`--ocs-capabilities` printed %r, exit %s); no parse/check/run/test contract is available"
                        % (out.strip()[:200], code))
    return ev


# ---- Champ: run/test through the CLI ---------------------------------------

# Messages of a champ-abi `okkama` that refuses to run at all (driver missing,
# or the SPB 16 refusal when OKKAMA_PIPELINE is not oir2).
CHAMP_DRIVER_REFUSALS = ("Champ OIR-2 driver is not yet available", "Champ driver requires",
                         "runs only SPB 17")
# A load-gate rule code (spb17::Error.rule): upper case with underscores.
LOADER_RULE = re.compile(r'rule: "([A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+)"|^(?:okkama: )?([A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+): ')
GENERATION_GAP = re.compile(r"not yet in S\d")
# The root line of a runtime failure report (sparrow_core failures.rs).
REPORT_HEADERS = (
    (re.compile(r"^RuntimeError: (.*)$"), "runtime", None),
    (re.compile(r"^Panic: (.*)$"), "panic", None),
    (re.compile(r"^Cancelled: (.*)$"), "runtime", None),
    (re.compile(r"^NativeFailure: (.*)$"), "runtime", "NativeFailure"),
    (re.compile(r"^(Execution (?:budget exhausted|interrupted).*)$"), "runtime", None),
    (re.compile(r"^Uncaught ([A-Za-z_][A-Za-z0-9_]*)()$"), "uncaught-error", 1),
    (re.compile(r"^([A-Z][A-Za-z0-9_]*): (.*)$"), "uncaught-error", 1),
)
REPORT_LABELS = ("Caused by:", "Secondary cleanup failure:")


def first_line(text):
    for line in text.splitlines():
        if line.strip():
            return line.strip()
    return ""


def report_header(stderr):
    """(category, error_type, message, line) of the failure report ending `stderr`, or None.

    The report is the last thing the CLI writes; the program's own stderr comes
    before it. Lines after a cause/secondary label belong to nested entries.
    """
    lines = []
    for line in stderr.splitlines():
        if line in REPORT_LABELS:
            break
        lines.append(line)
    for line in reversed(lines):
        if LOADER_RULE.match(line):
            continue
        for pattern, category, error_type in REPORT_HEADERS:
            m = pattern.match(line)
            if not m:
                continue
            if error_type == 1:
                return category, m.group(1), m.group(2) or "", line
            return category, error_type, m.group(1), line
    return None


# Errors of the test runner itself (spec 14: an empty root suite is an error).
CHAMP_TEST_RUNNER_ERRORS = ("Test run requires at least one test",)


def champ_outcome(mode, code, stderr):
    """Translate one `okkama run` on Champ into (response fields, adapter exit).

    `code` is the CLI's exit status (negative: killed by that signal).
    """
    head = first_line(stderr)
    if code is not None and code < 0:
        return {"outcome": "internal-error", "stage_reached": "runtime",
                "failure": {"stage": "runtime", "category": "internal", "raw": stderr,
                            "message": "okkama terminated by signal %d" % -code}}, 70
    if code == 0:
        return {"outcome": "completed", "stage_reached": "runtime", "exit_status": 0}, 0
    if code == EXIT_OIR2_UNSUPPORTED or "unsupported in OIR-2 lowering" in head:
        return {"outcome": "unsupported", "stage_reached": "check",
                "unsupported_reason": head[len("okkama: "):] if head.startswith("okkama: ") else head,
                "raw": stderr}, 3
    runner = head[len("okkama: "):] if head.startswith("okkama: ") else head
    if mode == "test" and runner in CHAMP_TEST_RUNNER_ERRORS:
        # The test runner's own language-level errors, reported as the native
        # endpoint reports them on Tomato (sparrow_core::run_tests).
        failure = {"stage": "runtime", "category": "runtime", "message": runner, "raw": stderr}
        return {"outcome": "failed", "stage_reached": "runtime", "failure": failure}, code if code else 1
    if head.startswith("okkama: ") or any(text in head for text in CHAMP_DRIVER_REFUSALS):
        # The front end accepted the program: what follows is the OIR-2
        # pipeline (lowering, verifiers, SPB 17 generation, load gate) or the
        # driver, never a language rejection.
        message = head[len("okkama: "):] if head.startswith("okkama: ") else head
        if GENERATION_GAP.search(message):
            return {"outcome": "unsupported", "stage_reached": "check",
                    "unsupported_reason": message, "raw": stderr}, 3
        return {"outcome": "internal-error", "stage_reached": "check",
                "failure": {"stage": "check", "category": "internal", "message": message, "raw": stderr}}, 70
    # A failure report always ends the process with status 1; any other
    # status is main()'s own result, whatever the program wrote to stderr.
    found = report_header(stderr) if code == 1 else None
    lines = [line for line in stderr.splitlines() if line.strip()]
    if code == 1 and lines and LOADER_RULE.match(lines[-1]) and found is None:
        return {"outcome": "internal-error", "stage_reached": "runtime",
                "failure": {"stage": "runtime", "category": "internal", "message": lines[-1], "raw": stderr}}, 70
    if found is None:
        if code is None or not 0 <= code <= 255:
            return {"outcome": "internal-error", "stage_reached": "runtime",
                    "failure": {"stage": "runtime", "category": "internal", "raw": stderr,
                                "message": "unexpected exit status %r" % (code,)}}, 70
        # main() returned this status (or a test run reported failures).
        return {"outcome": "completed", "stage_reached": "runtime", "exit_status": code}, code
    category, error_type, message, line = found
    failure = {"stage": "runtime", "category": category, "message": message, "raw": stderr}
    if error_type:
        failure["error_type"] = error_type
    return {"outcome": "failed", "stage_reached": "runtime", "failure": failure}, code


def champ_command(binary, mode, req):
    cmd = [binary, "run"]
    if mode == "test":
        cmd.append("-t")
    if req.get("stdlib_root"):
        cmd += ["--stdlib", req["stdlib_root"]]
    return cmd + list(req["sources"]) + ["--"] + list(req.get("args", []))


def champ_frontend(ev, req, resp_path):
    """Check the sources with the shared front end; returns (response, exit) or None when accepted."""
    scratch = os.path.dirname(os.path.abspath(resp_path))
    check_req = dict(req, mode="check")
    check_req_path = os.path.join(scratch, "request.champ-frontend.json")
    check_resp_path = os.path.join(scratch, "response.champ-frontend.json")
    with open(check_req_path, "w") as fh:
        json.dump(check_req, fh, indent=2, sort_keys=True)
    try:
        p = subprocess.run([ev["binary"], "--ocs-request", check_req_path, "--ocs-response", check_resp_path],
                           cwd=req.get("cwd"), stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, env=binary_env("champ"))
        with open(check_resp_path) as fh:
            resp = json.load(fh)
    except Exception as exc:
        return {"outcome": "internal-error", "stage_reached": "none",
                "failure": {"stage": "check", "category": "internal",
                            "message": "front-end check failed: %s" % exc}}, b"", 70
    if resp.get("outcome") == "accepted":
        return None
    resp = dict(resp)
    resp["mode"] = req["mode"]
    return resp, p.stderr, p.returncode if resp.get("outcome") != "unsupported" else 3


def champ_execute(ev, req, resp_path, started):
    mode = req["mode"]
    evidence = {"engine": "champ", "pipeline": ENGINES["champ"]["pipeline"], "binary": ev.get("binary"),
                "root": ev.get("root"), "git_head": ev.get("git_head")}
    if mode == "run":
        front = champ_frontend(ev, req, resp_path)
        if front is not None:
            doc, stderr, code = front
            doc.update({"protocol": PROTOCOL, "adapter": {"name": "okkama-rust", "version": "1"}})
            doc["evidence"] = dict(evidence, stage="frontend")
            doc["elapsed_ms"] = int((time.time() - started) * 1000)
            with open(resp_path, "w") as fh:
                json.dump(doc, fh, indent=2, sort_keys=True)
            if stderr:
                sys.stderr.buffer.write(stderr)
                sys.stderr.flush()
            return code
    cmd = champ_command(ev["binary"], mode, req)
    env = dict(os.environ)
    env["OKKAMA_PIPELINE"] = ENGINES["champ"]["pipeline"]
    # stdin and stdout pass through untouched; stderr is read to classify the
    # outcome and then written through unchanged.
    try:
        p = subprocess.Popen(cmd, cwd=req.get("cwd"), stderr=subprocess.PIPE, env=env)
        _, err = p.communicate()
        code = p.returncode
    except OSError as exc:
        err, code = str(exc).encode(), 127
    text = err.decode("utf-8", "replace")
    fields, exit_code = champ_outcome(mode, code, text)
    doc = {"protocol": PROTOCOL, "adapter": {"name": "okkama-rust", "version": "1"}, "mode": mode}
    doc.update(fields)
    doc["evidence"] = dict(evidence, stage="cli", command=cmd, cli_exit=code)
    doc["elapsed_ms"] = int((time.time() - started) * 1000)
    with open(resp_path, "w") as fh:
        json.dump(doc, fh, indent=2, sort_keys=True)
    if err:
        sys.stderr.buffer.write(err)
        sys.stderr.flush()
    return exit_code


def main(argv):
    if argv == ["--info"]:
        ev = probe()
        print(json.dumps({
            "protocol": PROTOCOL,
            "adapter": {"name": "okkama-rust", "version": "1"},
            "implementation": "okkama-rs (Rust implementation, in development)",
            "revision": {"git_head": ev.get("git_head"), "status_entries": ev.get("git_status_entries")},
            "configuration": {k: ev.get(k) for k in ("root", "cargo", "binary", "rust_sources", "build_exit", "engine",
                                                    "build_command", "target_dir")},
            "modes": ev.get("modes", []),
            "unsupported_reason": ev.get("reason"),
        }, indent=2, sort_keys=True))
        return 0
    opts = dict(zip(argv[::2], argv[1::2]))
    req_path, resp_path = opts.get("--request"), opts.get("--response")
    if not req_path or not resp_path:
        print("usage: okkama_rust_adapter.py --info | --request R --response P", file=sys.stderr)
        return 2
    started = time.time()
    with open(req_path) as fh:
        req = json.load(fh)
    mode = req.get("mode")
    ev = probe()
    if mode not in ev.get("modes", []):
        reason = ev.get("reason") or ("mode %r is not declared by the implementation (declared: %s)" % (mode, ev["modes"]))
        doc = {"protocol": PROTOCOL, "adapter": {"name": "okkama-rust", "version": "1"}, "mode": mode,
               "outcome": "unsupported", "unsupported_reason": reason,
               "evidence": {k: ev.get(k) for k in ("root", "git_head", "rust_sources", "build_exit", "capabilities_exit",
                                                   "engine")},
               "elapsed_ms": int((time.time() - started) * 1000)}
        with open(resp_path, "w") as fh:
            json.dump(doc, fh, indent=2, sort_keys=True)
        return 3
    engine = ev.get("engine") or "sparrow"
    if ENGINES.get(engine, {}).get("pipeline") and mode in ("run", "test"):
        return champ_execute(ev, req, resp_path, started)
    # The implementation speaks the protocol natively: forward unchanged.
    p = subprocess.run([ev["binary"], "--ocs-request", req_path, "--ocs-response", resp_path], cwd=req["cwd"],
                       env=binary_env(engine))
    return p.returncode


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
