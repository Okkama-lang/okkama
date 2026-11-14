# Chapter 11 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## Chapter 11. defer, destroy, regions: deterministic resources

described 0.5.x

# Chapter 11. defer, destroy, regions: deterministic resources

Memory is not the only thing a program borrows from the system. A file
descriptor, a socket, a database transaction, a lock, a temporary directory
and a child process all have to be given back, and given back at a
predictable moment: before the next request reuses the port, before the
transaction times out, before the disk fills with temporary files. Okkama
manages memory for the programmer, with a collector or reference counts
depending on the engine. It does not leave resources to the collector.

## Chapter 11. defer, destroy, regions: deterministic resources

described 0.5.x

This chapter explains each of those rules: where they come from, how the
compiler and the engines implement them, and what they cost.

## 11.1 Memory and resources are different problems

described 0.5.x

Okkama never ties a resource to the collector. When a `destroy` runs is a
property of the program text, the same on every engine and for every heap
size. Sparrow has a precise, non-moving collector, the IR engine
counts references, and the JVM has its own collector. All three run the
same `destroy`s in the same order.

## 11.4 destroy

described 0.5.x

An object has at most one `destroy` block. It is not an action: nothing can
call it. The engine runs it once, when the scope that owns the object ends.
It may read `self` and the fields, and call actions. It cannot assign fields
through a successor: the object is being released, not updated.

## 11.5 Who owns what

described 0.5.x

- `L07.ownership.alias-borrows`;
- `L07.ownership.returned-object-survives`;
- `L07.ownership.successor-independent`;
- `L07.destroy.branch-scope`;
- the region cases added with #9 and #266, among them
  `L07.ownership.reduce-builder`, whose resources are gathered by `reduce`.

## 11.11 Regions: how ownership runs

described 0.5.x

## 11.11 Regions: how ownership runs

The compiler implements ownership with **regions**: run-time sets of
identities, one per scope that may own a resource. It decides at compile
time which scopes need one, and that decision is most of the cost model.

A type is **plain** when none of its values can reach an object with a
`destroy`. Most types are plain: numbers, text, records of plain fields,
arrays and maps of plain values. Plain values never touch a region. Here
are two actions that differ only in the type they create:

```text
func @Use.plain(%0: Use, %1: int) -> int [method Use] {
^b0:
    br ^b1
^b1:
    br ^b2
^b2:
    %2: Point = call @Point.new#0(%1)
    %3: int = call @Point.value(%2)
    %4: int = const 1
    %5: int = int.add %3, %4
    br ^b3
^b3:
    ret %5
}
func @Use.owned(%0: Use, %1: int) -> int [method Use] {
^b0:
    br ^b1
^b1:
    %2: region = region.enter
    br ^b2
^b2 unwind ^b5:
    %3: Handle = call @Handle.new#0(%1)
    region.track %2, %3
    region.adopt %2, %3
    %4: int = call @Handle.value(%3)
    %5: int = const 1
    %6: int = int.add %4, %5
    br ^b3
^b3:
    region.leave %2
    br ^b4
^b4:
    ret %6
^b5(%7: failure):
    %8: failure = region.unwind %2, %7
    raise %8
}
```

`Point` has no `destroy`, so `plain` has no region at all. `Handle` has
one, so `owned` gets the full set. The instructions are a small vocabulary.

| Instruction | Meaning |
|---|---|
| `region.enter` | a new, empty region for this scope |
| `region.track r, v` | remember the free identities `v` reaches; do not own them yet (a call result, a temporary) |
| `region.adopt r, v` | own the free identities `v` reaches (`let`, a fresh argument) |
| `region.transfer from, to, v` | move what `from` owns of `v` to `to` (a value leaving a nested scope) |
| `region.leave r [, keep v]` | close: run destroys, newest first; what `keep` reaches becomes free again (a result) |
| `region.unwind r, f` | close on the failure path; cleanup failures join `f` as secondaries |
| `region.settle`, `region.settle_unwind` | the same in async functions, awaiting async destroys (#154) |

An optimization pass removes regions that provably stay empty. The
compiler's plainness is per declaration. One role implemented by one object
with a `destroy` makes every value of that role non-plain, even where no
such object is ever built ([#268](https://github.com/shootnix/okkama/issues/268)
would refine that).

Regions are named after, but are not, the regions of Tofte and Talpin's
region-based memory management [@tofte1997]. Those hold memory and free it
all at once. Okkama's regions hold *identities to release*. Memory stays
with the collector, and a region only decides when each `destroy` runs.

## 11.12 The cost of a walk

described 0.5.x

## 11.12 The cost of a walk

To track or adopt a value, the engine walks what the value reaches. It
looks for identities that are still free, owned by nobody. The walk stops
at objects owned by another scope, at plain values and at objects without
an identity. Until 0.4.1 it did not stop at arrays, maps and closures,
which have no owner. A fresh value pointing at a large owned structure cost
a walk over the whole structure, on every track and every close. A loop
building such values was quadratic.

Since #9 the engines remember containers whose walk met no free identity.
Containers are immutable, so what one reaches is at most as old as the
container. A remembered container is skipped until an identity at least as
old becomes free again, which happens when a scope returns it. Then all
the summaries are dropped. On the benchmark built for this
(`11-ownership`: 100 000 records in an array and a map, 300 rounds), the
change made Sparrow 85% faster, the IR engine 86% and the JVM 75%.

The lesson for program design is modest. Ownership costs nothing for plain
data. Non-plain data costs a walk the first time a structure is seen and
almost nothing after that. A program that keeps a million records reaching
a socket pays once, not once per request.

## 11.14 What this design costs

described 0.5.x

- **Ownership is checked at run time, not at compile time.** Rust rejects a
  program that would release a value twice or use it after release. Okkama
  prevents the double release by construction (at most once per identity),
  but a program can still use a closed resource. The `Closing` objects of
  the standard library answer that with a catchable `already_closed`
  failure, not with undefined behaviour.
- **Non-plain values cost a walk.** Small since #9, but not zero, and
  per-declaration plainness makes more values non-plain than strictly need
  to be (#268).
- **Async destroy constrains the owner.** A synchronous library cannot own
  a resource whose release must wait. It has to borrow one or become
  async.
- **Cleanup order is fixed.** Newest first, owner before fields. A program
  that needs another order must express it with `defer`, or by nesting
  scopes.

## 11.1 Memory and resources are different problems

described 0.5.x

Garbage collection solved memory well, because memory has a property other
resources lack: running out of it is the collector's signal to act. The
collector runs when memory is short, and that is exactly when memory needs
reclaiming. No such signal exists for file descriptors or database
connections. A program can run out of descriptors with plenty of free
memory, and the collector, which is not short of anything, does nothing.

## 11.13 What is not promised

described 0.5.x

## 11.13 What is not promised
