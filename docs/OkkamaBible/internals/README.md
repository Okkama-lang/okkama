# Holding area for the internals book

Passages taken out of the Okkama Bible because they describe the implementation rather than the language: compiler phases, OIR, engines, Sparrow, the JIT, GC, stack maps, SPB, regions as a mechanism, and measured engine internals. The owner decided on 2026-11-14 that these get a separate book (working title, from the owner's joke: «Библия сатаниста»).

Each file is named after the chapter it came from (`NN-<chapter>.en.md` / `.ru.md`) and records the release the text described. Much of it describes 0.5.x, before the 0.6.0 rebuild (OIR-2, Champ); it is raw material, not current truth.

## Names (owner, 2026-11-20)

The virtual machine is **Sparrow**. **Tomato** is the name of its current version and **Tweed** the next one
(0.6.0), as Tahoe names a version of macOS. Champ (OIR-2, SPB 17), the static-ownership attempt of the 0.6.0
rebuild, was retired and removed on 2026-11-20 (#911): passages here that mention Champ or OIR-2 describe a
pipeline that no longer exists. Tweed is built on Tomato's pipeline: cleanup regions from the lowering, resource
identities in the runtime (now inside the object, #528), a static pass for resources that never leave their
scope (#909), container summaries, and one linearity check in debug builds (#910). The repository's
`docs/map/ownership.md` is the current description.

