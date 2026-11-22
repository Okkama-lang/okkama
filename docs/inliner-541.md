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

## Slice 2 (adopted, 0.6.1): small static callees (#926, #927)

The JIT compiles a small static callee into its caller (`Emitter::inline_call`, `jit.rs`).

**Eligibility.**
- Callee: a synchronous Okkama function (not native, not intrinsic, not async) without handlers or `catch`,
  of at most `INLINE_CALLEE` (24) instructions.
- Budget: at most `INLINE_TOTAL` (160) inlined instructions per caller.
- Caller: synchronous and keeping Cranelift's stack maps (not an async body with explicit roots, #354).
- Depth: one level. A callee's own calls stay calls, so a recursive function inlines itself once, as HotSpot's
  recursive inlining limit does.

**What a call means is kept:**
1. **A step.** The inlined entry polls the budget as the callee's entry would.
2. **The stack check.** It is not kept: it is not a contract (see above).
3. **The frame.** The callee's failure and stop paths (`exit_fail`) jump to the caller's check after the call,
   exactly what a returning callee leads to.
   - The callee's check records its own frame with `rt_trace(callee, pc, fp - depth)`. Frame pointers are
     16-aligned, so the inlined activation keeps its own entry even in recursion.
   - The caller's check then records the call site and finds the caller's handler.
   - So a trace needs no pc table: it is built while the failure propagates.
4. **Safepoints and roots.** The callee's registers are variables of the caller's CLIF function, traced ones
   with stack maps, so every safepoint inside the inlined body reports them.

**Pass off.** `SPARROW_CORE_NO_INLINE=<caller symbols>|*`, and also implied by `SPARROW_CORE_PLAIN` and the
late-bound modes. The dispatch tests run with it, because they are about calls that stay calls.

**Tests.**
- `jit/inline_tests.rs`: no call left for a small callee; recursion one level (4 calls instead of 2); a callee
  with a handler keeps its call.
- `okkama-cli/tests/jit_inline.rs`: the same output, failure, trace, handler and fewest `--max-steps` with
  inlining on and off. Dropping the entry poll gives 998 steps instead of 2015.

**Evidence.** aarch64-apple-darwin, release build, best of 5 of 30 repetitions.
- Every program of the ladder prints the same with inlining on and off.
- Steady state: 02-calls 15.8 → 11.7 ms (−26%), 01-arith 20.4 → 19.1 (−7%), 10-app 55.6 → 52.7 (−5%); the
  rest is unchanged.
- Cost:
  - compile time +1–2 ms (02-calls 3.6 → 4.5 ms, 10-app 15.5 → 17.2 ms);
  - machine code +4–8% (08-roles: smaller).

**Not yet.**
- Hotness-driven tier 2 (#925): inlining happens at the first compile, within the budgets above.
- Deeper nesting.
- Callees with handlers.
- Async callers.
- Guarded devirtualization of `call.role`/`call.fn` (#928).
