---
name: peat
description: Use when a repository has a .peat/ directory or peat hooks. Finding the session and conversation behind a commit, change, bead or file before changing it; answering "why is it like this"; reading what a past session said and decided; searching prior work; time-travelling to what was known on a date.
---

# peat

The record of what agents did here. Hooks capture every session; you write nothing. At session start you may see this desk's last closing message; nothing else is pushed. Look things up when you need them.

## When to look

- **A commit or change you are about to amend, revert or build on:** `peat <commit-or-change-id>` gives the session that made it and what the person and the agent said.
- **A bead you are picking up:** `peat <bead-id>` gives the sessions that worked on it.
- **A file you are about to change, or "why is it like this":** `peat <path>`, then `peat <words>`. Search once before non-trivial design work.

```bash
peat <commit|change|bead|file|words>   # shape decides
peat <session> [seq]                   # a whole session, or one event
peat 2026-w40                          # a window of time
peat asof 2026-10-04 <words>           # what was known then
```

Every line ends in `▸ peat …`, the command that opens it one level deeper. Run it rather than guessing from a clipped line.

## How far to trust a line

- **The person's own words** are the strongest thing peat holds. Still check them against the current instruction files (CLAUDE.md, AGENTS.md), which win.
- **A closing message or compaction summary** is an agent's account of its own work. It can be wrong or optimistic; the commit and the tree are the evidence.
- **An observation** is one agent's claim; `uncited` means it rests on nothing. Age is on every line, and an old line about code may describe code that has moved. Check the tree before repeating it.
- peat records the past. Work state lives in beads, coordination in messages, history in jj; read those for what is true now.

## Writing

- `peat obs <subject> "<one-line rule>" --from <seq>` is only for a hard-won claim that will outlive the work and that no commit, bead or instruction file would carry.
- Don't run `peat capture` by hand; the hooks do. Exception: after a `Hook cancelled` message, `peat capture <transcript>` recovers the tail.
- Don't paste peat output into commits or issues; give the handle.
- Don't touch `.peat/`. `.peat/off` pauses peat.

Several desks share one ledger; a slow `peat` is usually a peer holding the lock.
