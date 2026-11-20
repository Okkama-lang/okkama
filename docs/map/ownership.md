# The current ownership model (OIR-2), in one page

The full specification is `okkama-docs/docs/compiler/OIR-2.md` (1,900 lines). Section numbers below point
there. **Status, 2026-11-20:** under review. The owner asked for one implementation instead of the current
five. See decisions.md and the survey in state.md before extending anything here.

## Language rules any model must keep (the Bible, ch. 11, 12, 18, 20)

- An object with `destroy` (a resource) is released exactly once:
  - at its owner scope's exit, in reverse creation order (serial numbers, §3.4, §5.1);
  - also on failure and on cancellation.
- Objects are immutable. A successor `^` makes a changed copy. `Atomic` is the only mutable cell, and it
  borrows: it never extends a lifetime (§7.2).
- The first binding owns a fresh value. Later bindings, fields and parameters borrow it. A borrow must not
  outlive its owner.
- An un-awaited Future is settled when its owner closes: cancelled, then its outcome or inputs are released
  (§10). Groups and collectors hold Futures. `async destroy` is awaited.
- Plain values (no reachable `destroy`) need no tracking.

## Vocabulary

| Term | Meaning (where) |
|---|---|
| obligation `obl(id)` | the duty to release one identity, exactly once (§3.2) |
| packet `%o: own` | a linear set of obligations: move, `own.split`, `own.join` (§3.2, §4.2) |
| `deps(%v)` | stable keys of the obligations a value refers to; copied with the reference |
| scope state `scope<S>` | linear state of a lexical scope: packets, journal, defers, staging, pending exit; closed exactly once (§5.2) |
| admission journal | obligations a scope took in from calls and constructors at run time (`own.admit`). Closing a scope that may hold admitted entries settles them dynamically (`cleanup.settle.dynamic`, D25.3) |
| bound journal | what `own.bind` / `own.accept` took in (a binding's or an entry's obligations). An admissible `scope.split` never selects from it (#848, PR-B1) |
| `scope.split` / `transfer` | take part of a scope's packets for an offer or a store; *admissible* means only admitted entries (§4.2) |
| duty | a service obligation: settle a Future cache or group, unpin. A scope's **service schedule** lists its static duties, plus one `dynamic` step when it may admit entries (D16, D25.3) |
| close | `cleanup.begin` → `cleanup.settle` (static duties) → `cleanup.settle.dynamic` → `cleanup.plan` (destroys, in serial order) → executor; its **envelope** declares which failures it may raise (D18, §9.3) |
| rehome | a Future's cache duty moves to the landing scope when it leaves its binding's scope (#847 ruling) |
| fresh summary | a constructor or call whose result packet is exactly its new identity (`FRESH_PACKET` checks it) |
| `own.dynamic` | explicit fallback: obligations go to the named scope's dynamic part (atomic, channel, group, any, closure, merge) (§7.1) |

## Where each part lives

- **Inference:** `own/analyze.rs` (flow, journals, quantities). **Emission:** `own/driver.rs` and `own/driver/*`.
  **Inventories and effects:** `own/scopes.rs`. **Futures:** `own/futures.rs`, `own/driver/futures.rs`.
- **Re-checks:**
  - verify/linearity, verify/precise_linearity, verify/coro*.rs;
  - low/eligibility.rs;
  - codegen/sparrow_oir2/cleanup.rs (evidence);
  - loader spb17: `load.rs` per-function checks; `automaton.rs` (close automaton, SPB_SCHEDULE); `services.rs`
    (admitting closes, fresh Futures); `evidence.rs`.
- **Runtime:** sparrow-core/src/champ/cleanup.rs (`rt_cleanup_*`, `rt_destroy_*`), helpers.rs (`rt_own_*`,
  `rt_scope_*`), futures.rs.

For a rule, rules.md lists every layer it is implemented in.
