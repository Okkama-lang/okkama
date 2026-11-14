# Chapter 12 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## 12.10 What a constructor compiles to

described 0.5.x

## 12.10 What a constructor compiles to

The compiler turns each constructor into an IR function marked `[ctor]`.
`okkama ir` prints them. For a `Point` with a checked full constructor and
a shorthand:

```okkama
object Point {
    has x is int;
    has y is int = 0;

    Point(x is int, y is int) {
        self.x = x;
        self.y = y;
        confirm x >= 0 else throw InvalidPoint.negative(x);
    }
    Point(:x);

    sum() is int { x + y; }
}
```

the two constructors come out as follows:

```text
func @Point.new#0(%0: int, %1: int) -> Point [ctor Point] {
^b0:
    %2: int = call @Point.default.y()
    %3: int = const 0
    %4: bool = int.ge %0, %3
    cond %4, ^b1, ^b2
^b1:
    %5: Point = obj.new Point { x: %0, y: %1 }
    ret %5
^b2:
    %6: int = const 0
    %7: array<int> = array.new [%0]
    %8: InvalidPoint = call @InvalidPoint.new(%6, %7)
    throw %8
}
func @Point.new#1(%0: int) -> Point [ctor Point] {
^b0:
    %1: int = call @Point.default.y()
    %2: Point = obj.new Point { x: %0, y: %1 }
    ret %2
}
```

Four things are visible here.

- **Overloads are separate functions.** `new#0` and `new#1` are the two
  constructors. The overload was chosen at compile time, so a call site
  names one of them directly: `call @Point.new#0(%1, %2)`.
- **A field bind is not a store.** `self.x = x` emits no instruction. The
  compiler keeps the bound value in a register, and the object is made by
  one `obj.new` with every field at once. That is how "each field is bound
  once" is enforced cheaply: there is no half-filled object to write into.
- **The object exists only after the checks.** `obj.new` sits in the branch
  where `confirm` held. On the other branch there is no `Point` at all, only
  the `InvalidPoint` being thrown. "No `Cash` is ever made" (§12.6) is meant
  literally.
- **Defaults are functions.** `@Point.default.y` is called on each
  construction, before the body, as §12.2 says. The full constructor calls it
  too, although its body binds `y` itself.

Reading a field is `obj.get %0, x`. The action `sum` is a function that takes
the object as its first parameter:

```text
func @Point.sum(%0: Point) -> int [method Point] {
^b0:
    %1: int = obj.get %0, x
    %2: int = obj.get %0, y
    %3: int = int.add %1, %2
    ret %3
}
```

## 12.11 Handing `self` out of a constructor

described 0.5.x

`Leaf(up: self)` receives a `Tree` whose `leaf` field is not bound yet. The
compiler handles this with a second form of construction. When `self` is
used as a value, the object is created at that point, with the unbound
fields left undefined. Each later bind becomes an `obj.init` of the same
object:

```text
func @Tree.new#0(%0: str) -> Tree [ctor Tree] {
^b0:
    %1: int = const 0
    %2: $variant_Maybe_2 = undef
    %3: Tree = obj.new Tree { name: %0, leaf: %2 }
    %4: Leaf = call @Leaf.new#0(%3)
    %5: array<Leaf> = array.new [%4]
    %6: $variant_Maybe_2 = call @$variant_Maybe_2.new(%1, %5)
    obj.init %3, leaf, %6
    ret %3
}
```

The collaborator holds an unfinished object, and the rule from the start of
this section follows it there. The specification forbids calling *any*
action on a receiver whose construction is pending, "through aliases, role
references, bound actions and collaborators", and requires the run time to
reject such a call before evaluating its arguments. `Leaf` above only
stores `up`; calling `up.describe()` in `Leaf`'s constructor would break
the rule. The compiler catches the direct `self.` call, and every engine
catches the rest at run time:

## 12.11 Handing `self` out of a constructor

described 0.5.x

The call fails before `Echo().said("argument")` is evaluated, as the
specification requires, and the same holds through a role or a bound action.
Until 0.4.1 there was no such check: a call that read a field not yet bound
failed differently on each engine, and on Sparrow it crashed the process
(#271). If a constructor fails after `self` escaped, the escaped object
never completes, and every later call on it is rejected the same way.

The first `self` may sit anywhere in a statement: inside a `map`,
`filter` or `reduce` block, in a nested block or in one branch of an `if`.
The object is built once, just before the constructor statement that holds
that first use, so every leaf made in a loop points at the same object.
Until 0.4.2 such a constructor crashed the compiler, and the workaround was
to name `self` first with `let me = self;` (#279).

## 12.12 How the engines store an object

described 0.5.x

## 12.12 How the engines store an object

The three engines keep the same object in three different shapes. A
program cannot tell them apart; the costs differ.

**The IR engine** (`okkama-runtime-core`) keeps an object as a
reference-counted Rust struct:

- the number of its declaration;
- a vector of field values;
- a serial number, the identity's place in creation order;
- an owner cell, the region that will destroy it (Chapter 11);
- an optional side part, allocated only when needed.

The side part holds fields bound by `obj.init` and the run-time state of
std objects with built-in actions, such as channels. An object whose
constructor hands out `self` has one more field than it declares: a hidden
`$pending`, left unbound until the constructor returns. The check of §12.11
reads it with `obj.pending`. Objects whose constructor never hands out
`self` have neither the field nor the check. The field vector is
never written after `obj.new`. An `undef` field bound later is looked up in
the side part. Most objects never allocate one.

**Sparrow** keeps an object in its own non-moving heap. The object
is one header word followed by one 8-byte word per field. The header holds
the kind (`OBJECT`), a mark bit for the collector, a few flags, and the
object's descriptor number in its upper 32 bits. Each field word has a
representation fixed by the descriptor:

- `Int`: a tagged small integer, or a pointer to a big one;
- `Raw`: the bits of a float, `bool`, byte or char;
- `Ref`: a pointer to another heap cell.

The collector follows `Ref` words and the `Int` words that point to big
integers; it never looks inside `Raw` words.
`Point(3, 4)` is three words, 24 bytes, with both integers stored inline.
Small cells come from per-size free lists in 64 KiB pages: 2, 3, 4, 5, 6,
8 … 128 words. Machine code produced by the JIT can pop a cell from its
list without calling into the runtime. A field read is one load at
`object + 8 + 8 × index`.

**The JVM backend** makes every object a `final` class that extends the
runtime's `okkama.rt.Obj`. `Obj` holds the serial number, the owning region
and the slot for built-in state. Each role the object plays becomes a Java
interface it implements. Fields become instance fields. They are not
`final` in the class file, because `obj.init` must still be able to bind
them after an early `self`. The language guarantees immutability; the class
file does not. The class gets generated `equals` and `hashCode`, field by
field and consistent with each other, so the runtime's Java collections
see the same structural equality as the program. The JIT of the JVM then
treats the object like any other small Java object.

## 12.13 Objects elsewhere

described 0.5.x

## 12.13 Objects elsewhere

## 12.14 What this design costs

described 0.5.x

## 12.14 What this design costs
