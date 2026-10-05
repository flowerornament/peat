---
name: peat
description: Use when a repository has a .peat/ directory or peat hooks. Recovering what past sessions learned, reading a belief's evidence trail, searching prior work, time-travelling to what was believed on a date, or depositing a durable observation at a commit or task completion. Also when a brief appeared at session start and you need to act on it.
---

# peat

Agent memory as a fold: every session's exhaust lands in one append-forever ledger, and every read is a view over it. Hooks do the mechanical half (brief on wake, capture on stop). **Your half is judgment: read before deciding, deposit what a stranger would need.**

## Read

Two verbs; every output line ends in the command that looks one level deeper. Follow those handles rather than guessing.

```bash
peat                       # orient (already injected at session start)
peat <thing>               # shape decides: window, session, subject, or search
peat asof 2026-07-10 <words>   # what was believed then; then diff against now
peat --help                # the explicit spellings
```

Read the brief before acting. `standing rulings` are the user's own instructions, each ending in the command that shows the message it restates: follow them as you would the user. `open loops` are things left unfinished; check whether yours is among them before starting something new. Dated lines that read as prose are distilled summaries of what happened then. `current understanding` is prior judgment on this codebase, and `last session` is where the previous agent stopped. Before non-trivial design work, search once (`peat <words>`), then open the trail of any subject that matches (`peat <subject>`).

How far to trust a line:

- **Who wrote it.** A ruling restates the user and cites the message; a digest or loop was written afterwards by a model reading the log, and can be wrong in detail. An observation is an agent's claim. When a summary matters to a decision, follow its handle down to the events.
- **Cited vs uncited.** An obs with `--from` seqs is grounded; `uncited` is a bare assertion. Open the trail before building on one.
- **Newest wins.** A subject's headline is its latest obs; the trail holds every earlier one. Disagreement inside a trail is information.
- **Briefs clip; trails don't.** A `…` line is an index entry. Never quote a clipped line as the claim.
- **Age is on every hit.** A months-old belief about code may describe code that has moved. Verify against the tree before repeating it.

## Deposit

```bash
peat obs <subject> "<one-line claim>" --from <seq>[,<seq>]
```

Where a ledger is distilled, what happened is written for you afterwards, so say what you learned in your reply and let it be captured. Deposit by hand only the thing a summary would lose: a rule that will outlive this task. The hooks nudge at commits and task completions. One claim per obs; a subject accrues support through repetition, and a revised belief is a new obs on the same subject.

The test for a claim: read months later by an agent on another desk with no shared context, would it change what they do?

- **A rule, not a story or a status.** The story is already in the ledger (cite it); status belongs in the tracker or the commit.
- **No deixis.** Never "tonight", "just now", "this session", "the reviewer". The timestamp is recorded; prose about *now* rots immediately.
- **Findable names.** Commands, paths, repo vocabulary. Not episode names ("the v3 rebuild", "the fix").
- **Reuse subjects.** `peat subjects` lists them; the near-subject hint on write is a drift guard, not a suggestion to fork.
- **Cite.** Seqs come from `peat <session>` and from search hits, addressed `session seq`.

Bad: *"struck twice same evening: the v3 rebuild also ran a pre-Said binary; re-capture healed it silently"*
Good: *"A precedent set in a zero-row domain can be wrong in a populated one: check every carried-forward pattern against the population it is about to meet."*

## Don'ts

- Don't run `peat capture` by hand; the hooks do, idempotently. Only after a `Hook cancelled` message, to recover a tail.
- Don't paste brief or trail text into commits or issues; link the subject (`peat <subject>`).
- Don't deposit a summary of the session. Deposit the one thing in it that was learned.
- Don't touch `.peat/`. `.peat/off` pauses the hooks; everything else is the ledger and its views.

## Several desks, one memory

Worktrees and secondary workspaces resolve to the main checkout's ledger automatically; `.peat/redirect` names an anchor explicitly. The brief's `active in the last hour` is the other agents; `current understanding` interleaves everyone's observations. Writers queue on one lock, so a slow `peat` is usually a peer, not a fault.
