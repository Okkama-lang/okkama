# Okkama style (short form)

The full style guide is being written (ticket #1189) and will replace this page. Until then, these rules hold; the
reference for each is `okkama agents language` and the std's own code (`okkama env` shows where it is).

- **Behaviour, not state.** An object never hands out its fields; you ask it for what you need and it answers in the
  right form. No getters of internals, no setters.
- **Immutable values.** A change is a successor: a new object made from the old one. No mutable fields; accumulate
  with `reduce` or a successor; shared mutable state only through what std provides for it (`Atomic`, channels).
- **Small objects, roles.** One responsibility per object; constructors only assign. Behaviour shared between types is
  a role, not inheritance; a parameter takes the role it needs.
- **Names.** Objects and results are nouns, effects are verbs; `snake_case` for actions and lets. One name, one
  meaning; no `value()` — give the thing a noun of its own.
- **Flat APIs.** Named arguments; no nesting dolls of wrappers, no arrays of loose parts, no `each(fn)` callbacks.
- **Failures are typed.** An error is an enum of your own (`enum E is Error = (…)`), thrown and caught by type; no
  sentinel values, no "absence means error" (absence is `Maybe<T>`).
- **Resources have an owner.** A type with a `destroy` is owned and destroyed by the language's rules, never by a
  manual "close later" protocol.
- **Tests next to the code.** `test` blocks in the module; fakes, not mocks of internals.
- **std first.** Before writing a helper, look for it with `okkama man -k`.
