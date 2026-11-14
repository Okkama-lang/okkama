# Chapter 13 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## 13.3 Overloads

described 0.5.x

Overloading is resolved at compile time from static types. A value typed
`any` is not narrowed back to what it holds when an overload is chosen.
The IR shows the result: each overload is its own function, `@Greeter.describe#0`,
`#1`, `#2`, and a call names one of them directly. Okkama has no dynamic
dispatch on argument types (multiple dispatch). Dispatch on the receiver
happens only through roles (Chapter 14).

## 13.5 Successors

described 0.5.x

`obj.next` is the IR instruction for "copy with these fields replaced".
`Tab.closed` compiles to two instructions, a constant and one `obj.next`.

## 13.12 What a successor costs

described 0.5.x

A successor is a shallow copy. `obj.next` allocates an object of the same
size and copies every field. Fields that hold other objects copy the
reference, not the object. On Sparrow the JIT unrolls the copy
into one store per word, the header included. Unchanged words are loaded
from the receiver; staged words come straight from registers. For `Tab`,
with four fields, `ordered` is an allocation, three loads and five
stores. On the IR engine it is a new vector of
field values. On the JVM it is a constructor call with the old fields and
the new ones.

## 13.12 What a successor costs

described 0.5.x

- **A wide object makes every transition expensive.** An object with
  thirty fields copies thirty words for each change to one of them.
  Grouping fields that change together into a part keeps transitions small,
  and that grouping is usually also a better model.
- **A long loop of successors allocates once per step.** For thousands of
  steps that is cheap. The allocation fast path on Sparrow is a free-list
  pop, and a successor replaced on the next step is garbage at once: the
  collector never marks it. For a
  hot inner loop over a large count, `reduce` over a range or a `Cell` of a
  scalar avoids the objects entirely.

## 13.5 Successors

described 0.5.x

**Reads see the receiver, not the staged values.** After `^items = items + 1;`
the name `items` still means the receiver's field. The compiled code makes
this plain: every read is `obj.get` on the receiver, and the staged values
wait in registers.

## 13.5 Successors

described 0.5.x

```text
func @Tab.ordered(%0: Tab, %1: int) -> ^Tab [successor Tab] {
    …
^b2:
    %7: int = obj.get %0, items
    %8: int = const 1
    %9: int = int.add %7, %8
    %10: int = obj.get %0, total
    %11: int = int.add %10, %1
    %12: Tab = obj.next %0 { items: %9, total: %11 }
    ret %12
    …
}
```

## 13.13 A value computed once: `once let`

described 0.5.x

The first run computes the value and stores it in a hidden slot of `self`;
every later run reads the slot (#491). Several actions share one result by
calling one hidden action. The compiler keeps the stored value honest:

## 13.13 A value computed once: `once let`

described 0.5.x

- the value reads only fields, `self`, constants and earlier `once let`s,
  never a parameter, so it cannot belong to one call;
- it reaches no effect: no clock, file, network, process or randomness, no
  `Cell`, `async` object, `await` or `spawn`, followed through the actions of
  concrete objects. A call through a role is trusted, as the object's
  contract with its input;
- it holds no resource. A function value or a role value counts as one only
  if it can hold one in this program: a function type when some closure of
  that type captures a resource, a role when some object that does it has a
  `destroy`. The compiler decides this over the whole program, after every
  specialization exists, so a generic library may keep a closure in a
  `once let` for every caller that passes a plain one (issue #520).

## 13.13 A value computed once: `once let`

described 0.5.x

The slot is not part of the value. `==`, map keys and `Dumper` leave it out,
so an object is equal to itself before and after its first answer. A
successor starts with empty slots, because its fields changed. A value that
throws leaves the slot empty for the next call.

## 13.3 Overloads

described 0.5.x

1. Candidates with the wrong arity or the wrong names drop out. Then
   argument types are checked against declared types.
2. If a fixed candidate fits, variadic candidates are not considered.
3. Exact concrete types beat role-compatible types, and both beat `any`.
4. A candidate wins only if it is no less specific in *every* position and
   more specific in at least one.
5. Declaration order never breaks a tie. A tie is an error.
6. Overloads of one name and arity may declare different results when
   their parameter types differ. A call gets the result of the overload it
   selects: `minus(duration is Duration) is Instant` and
   `minus(other is Instant) is Duration` sit side by side (§14.8).
