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
| static resource (#909 2b) | a call result (constructor or factory) the census places in its scope, in a sync or async body whose region does not settle, of a leaf type (own sync destroy, plain fields), alone in its region, all reachable code dominated by the creation: `resource.pin` instead of track/adopt, `resource.leave`/`resource.unwind` at the scope's exits, no region | `ir/statics.rs` (pass `statics`), `resource_pin`/`resource_close` in `failures.rs` |
| fast paths (#529) | machine code decides `region.track`/`adopt` and the per-instance identity of `obj.new` from headers (no identity, leaf, summarized container) and calls the helper only when it may matter; `SPARROW_CORE_PLAIN` keeps the helper | `jit.rs` `may_reach_identity`, `field_may_reach_identity` |
| inherited summary (#529) | an adopted big array is summarized at once; `arr.slice`/`arr.concat`/`map.new`/`map.merge` of summarized parts are summarized at birth, so versions of a growing map are never walked | `inherit_summary`, `region_add`, `ops::rt_exec` |
| root (#817) | a Future still running when its owner closes, or started into a group whose owner closed, detaches to the root region (not cancelled); a member started after the owner closed and already finished is closed at the call; the root closer destroys the detached outcomes after shutdown drains the spawned tasks (memo OIR-2-async-remaining §2.4) | `detach_to_root`, `close_future`, `close_root`, `GroupStart` |
| driver inputs (#845) | a fresh Future a collector's own sequence makes (`item`) is owned by the driver frame's private region; taking the driver's outcome releases only the outcome's package, and the inputs close with the driver | `driver_input`, `take_outcome`, `drivers::collect_start` |
| lazy region (#530) | machine code enters a region as 0; the first add that holds something (`rt_region_add`) or a transfer into it (`rt_region_new`) makes it; leave, unwind and settle skip a region never made. `SPARROW_CORE_PLAIN` makes regions eagerly | `jit.rs` `RegionEnter`, `RegionTransfer` |
| close | destroys the owned identities in reverse serial order, owner before fields; async destroys are awaited (`rt_region_settle`) | `region_close`, `tasks.rs` |

## Late edges (#907)

A summary says "this immutable container reaches no free identity". Values that gain references after they are
built could break that; each one is handled:

| Late edge | Why a summary stays true | Test |
|---|---|---|
| `Atomic.replace` | An Atomic names, it does not own: a resource stored in it keeps its owner, so nothing becomes free; a walk stops at an owned Atomic | `an_atomic_in_a_summarized_container_borrows_a_late_resource` |
| A Future's outcome | Frames are never summarized (a walk counts them as an event), and an adopted array holding Futures is not summarized either; the outcome lives in the frame's private region, and taking it calls `freed`, which drops every summary | the #817/#845 tests |
| Builders and other in-place payloads | Only maps and `any` boxes are indexed or summarized; other payloads change in place and are walked each time. Builders are internal to lowering and never stored in a container | — |
| `obj.init` | An object whose reference field is still unbound when it is created gets an identity (`fields_may_reach_identity`), so a later store is walked | #905 tests |
| `once let` | The compiler refuses a `once` value that can keep a resource | lowering |

A summary inherited at birth (#529) only comes from summarized parts, so it obeys the same rules.

**Measuring.** `OKKAMA_DEV=runtime` counts identities requested and skipped by type, region helpers, walks by
caller and steps, and structure indexes. `bench/ownership/run.py` compares against CPython (#908).

**Checker (#910).** `ir/linearity.rs` checks after the IR passes that each region and static resource is opened
once, used only while open and closed exactly once on every path (failure paths included). Debug builds and
`OKKAMA_VERIFY_OWNERSHIP=1` only. Its first find: an early return's defers tracked fresh values in a nested scope
the return had already closed (fixed in `exit_scopes`).

**Next.** The static pass (#909 2b) takes a resource alone in its region or the oldest member of one (newer
members close first). Several static resources per region, drop flags for conditional creation and direct JIT
destroy calls are the next steps. One linearity checker runs in debug
and CI only (#910). Late edges stay conservative (#907).
