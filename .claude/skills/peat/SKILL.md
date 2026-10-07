---
name: peat
description: Use when a repository has a .peat/ directory or peat hooks, or a peat brief appeared at session start. Acting on the person's standing rulings, recovering what past sessions did and decided, reading the evidence behind a line, searching prior work, or time-travelling to what was known on a date.
---

# peat

This project's memory. Hooks capture every session into one ledger, and a distiller turns what the harness already wrote (compaction summaries, closing messages, the person's own words, commits) into a paragraph per day and a register of **standing rulings**: the person's instructions, each citing the message it restates. You don't write any of it. **Read before acting, follow a handle when a line matters, and say what you learned in your reply:** it is captured.

## Read

The brief is injected at session start. Every line ends in `▸ peat …`, the command that opens it one level deeper; run it rather than guessing.

```bash
peat                         # the brief again
peat <thing>                 # shape decides: a window (w40, 2026-09, 2026-10-04), a session id, a subject, or search words
peat rulings                 # every standing ruling (--all adds withdrawn and superseded)
peat asof 2026-07-10 <words> # what was known then
```

- **Standing rulings** are the person's instructions, newest wins. Follow them as you would the person; the handle shows the message each restates.
- **Dated paragraphs** (a day, a week, a month) say what happened then. Open one when you need the detail.
- Lines headed `peat: from this project's memory` may arrive with a prompt: past lines that match it. Judge each.
- Before non-trivial design work, search once: `peat <words>`.

## Trust

- **Who wrote it.** A ruling cites the person's own message. A paragraph is a model's reading of the day and can be wrong in detail. An observation (`current understanding`) is one agent's claim; `uncited` means it rests on nothing. When a line decides what you do, follow its handle down to the events.
- **Age is on every line.** Something old about code may describe code that has moved; check the tree before repeating it.
- **Briefs clip; trails don't.** A line ending in `…` is an index entry, not the claim.

## Don'ts

- Don't run `peat capture` or `peat distill` by hand; the hooks do both. Exception: `peat capture <transcript>` after a `Hook cancelled` message, to recover a tail.
- Don't paste brief text into commits or issues; give the handle.
- Don't touch `.peat/`. `.peat/off` pauses peat, `.peat/distill-off` pauses the distiller, `.peat/push` turns on prompt-time lines.
- `peat obs <subject> "<one-line rule>" --from <seq>` exists for a rule nothing else would carry. You will rarely need it.

Several desks share one ledger: `active in the last hour` is the other agents, and a slow `peat` is usually a peer holding the lock.
