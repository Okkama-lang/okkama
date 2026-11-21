# Recorded action AST fixture

`actions-ast.json` records the unmodified JSON output of the installed `okkama`
for this checkout's `encoding/hex`, `collections/list` `strings/buffer` and `checksum/adler32`
modules (2026-11-22), keyed by std-relative module path:

```sh
okkama ast std/encoding/hex.ok --format json --mode production --prune-tests
okkama ast std/collections/list.ok --format json --mode production --prune-tests
okkama ast std/strings/buffer.ok --format json --mode production --prune-tests
okkama ast std/checksum/adler32.ok --format json --mode production --prune-tests
```

These cover typed full/shorthand constructors, overloads, generics, role
signatures, hidden objects, variadics and a payload enum constructor.
Tests also modify copies to exercise hidden actions and schema errors.
The fixture is a parsing contract, not the source for the live inventory.
