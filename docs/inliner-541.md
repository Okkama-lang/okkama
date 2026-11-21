# Inlining Okkama calls (#541): design note and decision

Status: **slice 1 adopted** (field accessors, in the JIT); a general inliner is **deferred**. Engine: Sparrow
(Tweed), the only backend since 0.6.0. The ticket was written when OIR fed three backends (IR, JVM, Sparrow), so
it asked for an OIR pass; with one backend, the place that sees everything a call means is the JIT.

## What a call means, and what inlining must keep

A Sparrow call of an Okkama function (`call`, `sparrow-core/src/jit.rs`) does four observable things:

1. **A step.** On entry the callee polls the budget when budgets are on ("function entries and loop back-edges",
   `Limits::steps`, `--max-steps`).
2. **A stack check.** Entry compares the stack pointer with `Ctx::stack_limit`; exhaustion is a typed failure (R2).
3. **A frame.** A failure inside the callee carries the callee's frame in its trace, and the callee's handlers
   decide what is caught.
4. **Safepoints and roots.** Allocation in the callee can collect; the caller's registers are roots across it.

An inlined call must keep (1) exactly, because programs and tests observe step budgets. (2) is not a contract:
the depth at which the stack runs out already depends on frame sizes, build and platform, and an inlined callee
uses no frame. (3) and (4) are kept for free only when the callee cannot fail and cannot reach a safepoint.

## Slice 1 (adopted): field accessors

Eligible callee: a synchronous Okkama function (not native, not an intrinsic, not async), without handlers, whose
whole body is

```
0: obj.get rD, rP, field     ; rP is one of its parameters
1: ret rD
```

(`accessor` in `jit.rs`). `obj.get` on an object cannot fail and does not allocate, so (3) and (4) do not arise.
The call site emits the budget poll when the caller is compiled with budgets (1), then the field load. Nothing
else is eligible: a `move` before `ret`, a constant, arithmetic (`int.mul` overflows into a failure with a frame),
a call, an allocation. Recursion cannot occur (the body calls nothing). Code size cannot grow (one load replaces
a call sequence).

- **Pass off:** `SPARROW_CORE_PLAIN=<caller symbols>|*` compiles the selected callers without the core's fast
  paths, this one included; tests use it as the differential.
- **Evidence** (aarch64-apple-darwin, release build): a loop that calls two accessors per round, 96 → 50
  instructions and ~9.9 → ~6.0 ns per round; it is now faster than one call of a method that reads both fields.
  `tools/pgo/workloads/07-objects`: 264.4M → 263.2M instructions; `08-roles`: 439.0M → 438.4M; same output. The
  workloads' hot methods compute (`energy()`, `position()`), so they are not accessors.
- **Tests:** `jit/accessor_inline_tests.rs` (only the exact shape qualifies; an accessor call leaves no call in
  the CLIF, a non-accessor keeps its call, the same result with the pass off); `okkama-cli/tests/jit_accessors.rs`
  (same output, and the same fewest `--max-steps` the program completes in, inlined and plain; dropping the poll
  makes it 66 instead of 166).

## Deferred: a general inliner

The calls worth removing next are small methods that compute: `energy() { vx * vx + vy * vy; }`. They can fail
(overflow), so inlining them needs:

- **Trace frames for inlined code.** A failure at an inlined instruction must report the callee's frame and span
  as if it had been called. The JIT has no inline-frame table today; the failure path would need one (pc → inlined
  callee, source span) consulted when a trace is built.
- **Handler scoping.** The callee's handlers (if any) and the caller's unwind target must nest as before; the
  first version would keep the rule "no handlers in the callee".
- **Allocation.** A callee that allocates reaches a safepoint; the caller's live registers at the inlined site
  must be roots (the JIT's stack maps already cover any instruction that can collect, so this is plumbing, not a
  new rule), and a memory-budget refusal must name the callee's frame (same table as above).
- **Budgets.** Per-callee and per-caller size limits, no recursion (SCC check), polls at each inlined entry.

Constructors (`obj.new` + `ret`) fall under "allocation". Dynamic dispatch (`call.role`, `call.fn`) and async
callees stay out until the static case is proven.

Decision for 0.6.0: no general inliner. Slice 1 removes the common accessor call at no observable cost; the
general case waits for the inline-frame table, which is a separate low-layer piece of work (new ticket when
taken up) and should be judged on workloads where small computing methods dominate.
