# Chapter 14 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## 14.7 How a role call runs

described 0.5.x

## 14.7 How a role call runs

A role boundary and a role call are explicit in the IR:

```text
%19: Bottle = call @Bottle.new#0(%18)
%20: Package = role.wrap %19
%21: Package = call.role %20, Package.opened()
%22: str = call.role %21, Package.label()
```

`role.wrap` marks the boundary where a `Bottle` becomes a `Package`.
`call.role` calls slot `opened` of `Package` on whatever object is behind
the reference. Each object's declaration lists, for every role it does, the
function that fills each slot:
`does Package { label = @Bottle.label, opened = @Bottle.opened }`.

The engines differ in how they find that function:

- **The IR engine** reads the receiver's declaration and looks up its
  witness for the role on each call.
- **Sparrow** compiles `role.wrap` to nothing: a role reference is
  the same pointer. `call.role` loads the object's header, shifts out the
  descriptor number, and indexes a table built once per role slot. The
  table holds one function per object type. Then it calls that function.
  Compared with a direct call, that is a few loads and an indirect call.
- **The JVM backend** turns each role into a Java interface. The classes of
  objects that do it implement the interface, `role.wrap` is again a no-op,
  and `call.role` is an `invokeinterface`. The JVM's inline caches and
  speculative inlining then treat a call site that sees one or two object
  types like a direct call.

In none of them does the restriction cost anything at run time. It is
checked when the program is compiled and verified, and the reference is
the plain object.

## 14.8 Operators are roles

described 0.5.x

With an object on the left, `a + b` is the call `a.plus(b)` and nothing
more. The compiler rewrites the operator into that call and checks it like
any other call: the right operand gets no conversion, and the overload it
selects must be a role instance the type declares. The engines never see an
operator on an object. The orderings `<`, `<=`, `>` and `>=` use the role
that already existed for them, `compare.Comparing<T>`: `a < b` is
`a.order(b) == Order.less`. `==` stays structural (Chapter 12).

## 14.10 What this design costs

described 0.5.x

- **Forwarding code.** Decorators repeat every action of the role. No
  delegation syntax shortens it (§14.6).
- **No retroactive conformance.** A role cannot be added to an object from
  outside its module. The usual fix, an adapter object, is one more object.
- **No downcast, ever.** Code that "just needs to check what this really
  is" has to be redesigned: an action on the role that answers the
  question, or an enum that carries the alternatives.
- **Invariant parameters.** An implementation cannot accept more than the
  role asks for. Usually it does not need to.
- **Operators look only to the left.** `price * 3` works; `3 * price` does
  not, because `int` cannot be given a role. Python asks the right operand
  through `__rmul__`, and Rust allows `impl Mul<Money> for i64` in Money's
  crate. Okkama has neither (no retroactive conformance, above).
- **One dispatch per call.** A role call is an indirect call. In hot loops
  over a single type, holding the concrete type avoids it; the JVM and its
  inline caches recover most of the difference.

## 14.9 Roles elsewhere

described 0.5.x

## 14.9 Roles elsewhere
