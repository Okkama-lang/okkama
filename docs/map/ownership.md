# Ownership in Tomato and Tweed, in one page

Champ's static model (OIR-2) was removed on 2026-11-20 (#911). Ownership is Tomato's: the lowering emits cleanup
regions, and the runtime tracks resource identities. Tweed (#903) keeps that design and makes it cheap.

## Language rules (the Bible, ch. 11, 12, 18, 20)

- An object with `destroy` (a resource) is released exactly once:
  - at its owner scope's exit, in reverse creation order;
  - also on failure and on cancellation.
- Objects are immutable. A successor `^` makes a changed copy. `Atomic` is the only mutable cell, and it
  borrows: it never extends a lifetime.
- The first binding owns a fresh value. Later bindings, fields and parameters borrow it. A borrow must not
  outlive its owner.
- An un-awaited Future is settled when its owner closes. Groups and collectors hold Futures. `async destroy`
  is awaited.
- Plain values (no reachable `destroy`) need no tracking.

## How it works

| Piece | What | Where |
|---|---|---|
| region | a scope's cleanup record. `region.enter`, then `adopt` (own) or `track` (borrow-follow) of values, `transfer` to another region, `leave`/`unwind`/`settle` at exit | lowering emits them; `ir/regions.rs` drops what plainness proves unneeded; `failures.rs` runs them |
| identity | a registered resource object: serial (creation order) and owner (`Free`, a region). Created by `Runtime::created`/`created_built` | `failures.rs` |
| per-instance decision (#905) | a program-built object without its own destroy, builtin or `once` slot becomes an identity only when its fields, as built, reach one; runtime objects (groups, driver results) stay per type | `created_built`, `fields_may_reach_identity` |
| resource walk | finds the free identities a value reaches (adopt, transfer, keep at close) | `resource_graph_into` |
| structure index (#906) | per immutable container, the children that may hold an identity; found once, dropped with the container (watched) | `structure_of`, `remember_structure` |
| summary | an ownership-state cache of a container that reached no free identity; reset when an old identity becomes free | `summarize`, `freed` |
| identity storage (#528) | an object with a destroy of its own keeps serial and owner in two hidden words after its fields; frames and destroy-less carriers stay in the identity map | `Heap::object_words`, `owner_of`/`set_owner`/`serial_of` |
| static resource (#909 2b) | a constructor result the census places in its scope, of a leaf type (own sync destroy, plain fields), alone in its region, all reachable code dominated by the creation: `resource.pin` instead of track/adopt, `resource.leave`/`resource.unwind` at the scope's exits, no region | `ir/statics.rs` (pass `statics`), `resource_pin`/`resource_close` in `failures.rs` |
| close | destroys the owned identities in reverse serial order, owner before fields; async destroys are awaited (`rt_region_settle`) | `region_close`, `tasks.rs` |

**Measuring.** `OKKAMA_DEV=runtime` counts identities requested and skipped by type, region helpers, walks by
caller and steps, and structure indexes. `bench/ownership/run.py` compares against CPython (#908).

**Next.** The static pass (#909 2b) takes one resource per region; several, drop flags for conditional
creation and direct JIT destroy calls are the next steps. One linearity checker runs in debug
and CI only (#910). Late edges stay conservative (#907).
