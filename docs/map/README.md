# Code maps: read before any task

Owner, 2026-11-20: the code base is too large to hold in context, and work was being redone. The causes were stale
dumps, an old binary taken as the reference, and fixes of things already fixed. These maps are small enough to
load whole. Read them **before** starting, and update them **when finishing**.

| Map | What | Kept by |
|---|---|---|
| [state.md](state.md) | Where things stand now: branches, worktrees, parked work, open blockers, latest numbers | by hand, at the end of every task |
| [decisions.md](decisions.md) | Rulings (owner, Fable) and settled design choices, dated. Do not relitigate | by hand, when a decision is made |
| [pipeline.md](pipeline.md) | The stages, their entry points and files, what each decides and checks, how a failure message names it | by hand, when a stage changes |
| [ownership.md](ownership.md) | Glossary of the current ownership model (OIR-2) with pointers into the code | by hand |
| [rules.md](rules.md) | Lexical tag index and layers mentioning each tag; see its non-rule exclusions | **generated**: `tools/map/gen.py` |
| [files.md](files.md) | Every source file by layer: lines and its `//!` header | **generated** |

The Wave CI fails when the generated maps are stale (`tools/map/gen.py --check`).

## Before starting a task (5 minutes, every time)

1. **Read** state.md and decisions.md. For the area you will touch, read its pipeline.md row and its rules.md row.
2. **Is it already done?** Run `git log --oneline -30 origin/release/065`, `grep -n '#NNN' CHANGELOG.md` and
   `gh issue view NNN`. Check the parked branches in state.md.
3. **Reproduce on the current tree**, with a binary built from it, at a known commit. The reference for behaviour
   is Tomato built from the same tree, never an installed binary (LLM-GUIDE, parity rule). Throw away dumps and
   binaries from before your last rebuild.
4. **Locate before reading code.** `OKKAMA_DEV=all,dump OKKAMA_DEV_OUT=DIR` names the failing stage, function and
   check (`events.jsonl`, `rejected`), and the dumps show the code that stage saw. Add a permanent devtrace hook
   instead of a temporary print.
5. **A rule change** touches every layer in its rules.md row. List them in the plan before editing.

## When finishing a task

- Update state.md: what changed, what is parked and where, the new numbers.
- Add any ruling or settled choice to decisions.md, with its date and source (ticket or commit).
- Run `tools/map/gen.py` and commit the regenerated maps with the code.
