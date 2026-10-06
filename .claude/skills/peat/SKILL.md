---
name: peat
description: Use when a repository has a .peat/ directory or peat hooks, or a peat brief appeared at session start. Acting on the user's standing rulings and open loops, recovering what past sessions did and decided, reading the evidence behind a line, searching prior work, or time-travelling to what was known on a date.
---

# peat

This project's memory. Hooks capture every session into one append-forever ledger, and afterwards a distiller reads it and writes three things: **digests** (what happened, in words), **rulings** (the user's standing instructions) and **loops** (what was left unfinished). You do not have to write anything. **Your part: read before acting, follow a handle when a line matters, and say what you learned in your reply** — your replies are captured and distilled.

## Read

The brief is injected at session start. Every line ends in `▸ peat …`, the command that opens it one level deeper; run it rather than guessing.

```bash
peat                         # the brief again
peat <thing>                 # shape decides: a window (w40, 2026-09, 2026-10-04), a session id, a subject, or search words
peat rulings / peat loops    # the full registers (--all adds superseded, withdrawn, closed)
peat asof 2026-07-10 <words> # what was known then
```

- **Standing rulings** are the user's own instructions, newest wins. Follow them as you would the user; the handle shows the message each one restates.
- **Open loops** are things left unfinished. Check whether your task is one before starting it fresh.
- **Dated prose lines** (a day, a week, a month) say what happened then. Open one when you need the detail.
- Lines headed `peat: from this project's memory` may arrive with a prompt: up to three past lines that match it. Judge each.
- Before non-trivial design work, search once: `peat <words>`.

## Trust

- **Who wrote it.** A ruling restates the user and cites the message. A digest or loop is a model's reading of the log and can be wrong in detail. An observation (`current understanding`) is one agent's claim; `uncited` means it rests on nothing. When a line decides what you do, follow its handle down to the events.
- **Age is on every line.** Something months old about code may describe code that has moved; check the tree before repeating it.
- **Briefs clip; trails don't.** A line ending in `…` is an index entry, not the claim. Never quote it as one.

## Writing by hand (rare)

```bash
peat obs <subject> "<one-line rule>" --from <seq>
```

Only for a rule a summary would lose and that will outlive this task. Not a status (that belongs in the tracker or commit), not a story (the ledger has it), no "today" or "this session". Reuse an existing subject (`peat subjects`).

## Don'ts

- Don't run `peat capture` or `peat distill` by hand; the hooks do both. Exception: `peat capture <transcript>` after a `Hook cancelled` message, to recover a tail.
- Don't paste brief text into commits or issues; give the handle.
- Don't touch `.peat/`. `.peat/off` pauses peat, `.peat/distill-off` pauses the distiller, `.peat/push` turns on prompt-time lines.

Several desks share one ledger: `active in the last hour` is the other agents, and a slow `peat` is usually a peer holding the lock.
