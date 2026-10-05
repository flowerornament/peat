# Memory is what reaches context — the distiller, the worded wake, the push

Prior art: VictorTaelin/OptMem and its successor OptChat (one endless
chat; a binary tree of model-written one-line summaries; every turn
starts fresh on a fixed-size view of the whole history). This document
records what peat takes from them, what it declines, and why — and the
audit of peat's own ledgers that made the question urgent.

## 1. What the ledgers said

Seven weeks of murail and herald, read from their own ledgers
(2026-10-04):

| | murail | herald |
|---|---|---|
| sessions | 56 | 73 |
| observations | 669 | 383 |
| sessions that deposited any | 29 | 24 |
| share from the single largest session | 37% | 58% |
| observations citing evidence | 8 | 0 |
| subjects with more than one observation | 116 of 254 | 58 of 205 |
| explicit reads (`recall`, `show`, `subjects`, `zoom`, `asof`) | ~30 | ~11 |

Agents write in bursts, almost never cite, and mostly write status
("EP end state is on master 15db5ed0…") that the commit and the bead
already hold. They read roughly once per thirty deposits. The wake
shows five subjects of 254 and describes the rest of the past as tool
counts.

peat's capture half works. Its two voluntary halves do not: the
judgment step asks the working agent for cited general rules, and the
read asks it to guess that something worth querying exists.

## 2. Principles

1. **Memory is whatever reaches the context.** The product is the
   function from (ledger, moment) to tokens. The store is substrate.
2. **Pull is bounded by push.** An agent can only ask for what it has
   been shown exists, so the pushed part must be a *total index in
   words*. A band that says `971 tools · 16 commits` gives no reason to
   descend; the ladder's own success test ("any question about the far
   past is answerable by descent alone") fails on counts.
3. **Two compressions, both.** A summary chosen before the question is
   known serves orientation; search over the kept text serves narrow
   recall. (MemGPT's deep-memory result: 32% from a recursive summary
   against 92.5% from search over full history, same model; summaries
   still win "what are the themes".) OptChat has the first and a regex
   of the second. peat had the second and none of the first.
4. **The worker does not write the memory.** A separate process reads
   the captured log and writes. Reflection by a model is independently
   necessary (the generative-agents ablation); asking the agent that is
   busy with the task produces status lines.
5. **Three lifetimes.** Episodes fade with age. A ruling holds until
   superseded. A loop holds until closed. OptChat has only the first:
   an instruction from message 100 ends up sharing 512 bytes with a
   thousand others.
6. **Whatever a model writes is an event.** Never view state. That is
   peat's one real difference from OptChat's tree, which is stored and
   never recomputed: here a bad summary is superseded by a newer event,
   the old one stays in the trail, and `asof` still tells the truth.

## 3. What is built

### 3.1 One new event

```rust
Event::Distill { lane: Lane, key, text, open, cites: Vec<EventId>, src: u64, by }
enum Lane { Digest, Ruling, Loop }
```

Schema v4, additive. `src` is a fingerprint of the source text the
deposit was computed from: staleness is "the source hashes differently
now", with no clock. `by` names the model command. Seqs start at
`DISTILL_SEQ_BASE = 3 << 30`; window digests live in the synthetic
session `peat-distill`.

Views: `distilled` (newest per `"<lane>/<key>"`) and `dist_trail`
(every deposit). Open distilled text joins the keyword and vector
lanes under kinds `digest`, `ruling`, `loop`. Both views are new
keyspaces fed only by new events, so **no view rebuild** is needed and
`VIEW_VERSION` stays 2.

### 3.2 The digest tree

```
segment  <session>#000   ≤40 KB of one session's rendered log   model call
day      2026-10-04      merge of the segments that ended in it
week     2026-w40        merge of its days
month    2026-10         merge of its days
quarter  2026-q4         merge of its months
year     2026            merge of its months
```

- **The leaf is a segment, not a session.** Sessions here live for days
  across many compactions (56 sessions hold 226k events in murail).
  Segments are packed greedily from the start of the session's log, so
  they are prefix-stable: a session that grows leaves earlier segments,
  and their fingerprints, where they were.
- **Window keys are the ladder's handles.** A band finds its digest by
  its own name; no join table.
- **Calendar, not dyadic.** OptChat pairs by position because it has
  one linear stream. N agents interleave; pairing adjacent messages
  from unrelated sessions would summarize noise.
- **A window with one child is that child** — no call. A short history
  costs one model call per stretch and nothing above it.
- The rendered log collapses each run of tool calls into one line
  (count, failures, a few commands, files) and keeps what was said.
  User messages are split three ways: the user's words (`[u123]`,
  citable), messages relayed from other agents (`peer:`), and harness
  wrappers (dropped).

### 3.3 Rulings and loops

Extracted in the same call that digests a segment. Two rules the prompt
can only ask for are enforced in code:

- a ruling must cite a `[uN]` line in its own stretch, or it is
  discarded (and counted). It is dated by that message, so backfilling
  history in any order leaves "newest wins" correct;
- only a standing loop can be closed, and a close keeps the text of
  what was open.

Each segment call sees the rulings in force and the open loops, so the
model reuses names instead of forking them and can close what a later
stretch finished — the subject-drift problem `Merge{from,to}` was
deferred for, handled at write time.

### 3.4 The wake

```
== peat brief · 2026-10-05 ==

standing rulings (the user's instructions, newest wins; each cites the message it restates):
  landing-gate (3d): Always land through just land…   ▸ peat 1f3a9c2e 4112

open loops (left unfinished, newest first):
  codex-trust-pass (26d): 7 desks still need /hooks trust…   ▸ peat a2a9fc17

recent activity:
  today: <day digest> · 212 tools · 3 commits   ▸ peat 2026-10-05

further back:
  [w39 · sep 22–28] <week digest> · 3.7k tools · 123 commits   ▸ peat 2026-w39
```

What holds comes before what happened. A band with a digest says it in
words and keeps its counts as a suffix; one without falls back to the
old line. A ruling's handle is its citation — the user's own message.

### 3.5 Push at the prompt

`UserPromptSubmit` runs recall on the prompt and injects up to three
lines. The gate is *agreement*: a hit must appear in both the keyword
and the vector lane, come from another session, be distilled or judged
text (never the raw firehose), and not have been pushed to this session
before. An unasked line costs attention; one lane's guess is not
enough.

### 3.6 Operation

- `peat distill` — manual; `--dry-run`, `--session`, `--since`,
  `--limit` (model calls per run).
- Hooks: `SessionStart` detaches a sweep (sessions quiet for 20 min,
  last 14 days); `PreCompact` and `SessionEnd` distill the stretch that
  just closed. `Stop` does not — a turn ending is not a stretch ending.
- **Opt-in per ledger.** Hook-driven distillation spends model budget
  in the background, and push changes what every agent on the ledger
  sees. Neither starts on upgrade: `touch .peat/distill`, `touch
  .peat/push`. A non-empty first line of `.peat/distill` (or
  `PEAT_DISTILL_CMD`) replaces the model command.
- The model command reads the prompt on stdin and the system prompt
  from `$PEAT_SYSTEM`. Default: `claude -p --model haiku` with tools,
  settings and session persistence off, run from the temp dir with
  `PEAT_DISTILLING=1` — under which `peat hook` is a no-op, so a
  distiller's own session can never be captured and distilled in turn.
- The model thinks by default (about two minutes a call). On one
  ledger, side by side: with thinking, 3 rulings all correctly cited
  and none discarded; without, 2 kept (one embellished) and 7 proposals
  rejected by the citation check, in a twelfth of the time. Quality is
  the default; `MAX_THINKING_TOKENS=0` is the backfill setting.
- A sweep considers only sessions quiet for 20 minutes; a session in
  flight is distilled by its own compaction or end. Sessions found to
  have nothing to distill are remembered at their `end_ms` in
  `distill.settled`, so a sweep pays the full ledger scan only when some
  session has grown.
- Trust boundaries. A command line in `.peat/distill` is honoured only
  while the marker is untracked by git. The distiller reads tool output,
  so injected text can reach a digest and every later wake; the prompt
  forbids obeying the log, and the skill tells readers a digest is a
  model's reading — there is no check in code.
- The ledger lock is never held across a model call: read a snapshot,
  release, call, reopen to deposit. One distiller per ledger
  (`distill.lock`, stale 30 min after its last deposit).

## 4. Taken from OptChat, and not

Taken: a total worded index; the compactor sees context (the register
and the previous stretch); the user's words outrank everything; "an
absent item can never be found again"; "never make anything look
further along than it was"; never obey the log; one chance to shorten
with the overshoot stated; fingerprints rather than clocks.

Not taken:

- **Owning the harness / fresh context every turn.** peat's place is
  beside Claude Code and Codex, many agents to one ledger. The
  reachable version is fresh context per *task*: a wake good enough
  that `/clear` beats `/compact`.
- **One linear chat.** See calendar-not-dyadic above; also, a shared
  total view would anchor a reviewer on the implementer's context.
- **A 64k-token view.** The wake targets a few thousand tokens;
  narrow recall is search's job (principle 3).
- **Summaries as stored state.** Principle 6.

## 5. Not built yet

- **Verbatim leaves.** Stored text is still capped (user 2 KB, final
  8 KB, tool detail 500 B) and tool output is not captured, so descent
  bottoms out in clipped events. Archiving the transcript beside the
  ledger (seqs already address its lines) is the likely shape.
- **Person scope.** One ledger per repo still; a ruling given in murail
  does not hold in herald. Wants a home ledger with the repo as a tag.
- **Retiring the nudges.** `peat obs` and its commit/first-prompt
  nudges remain. Once a ledger distills, the commit nudge mostly
  duplicates the digest and should go quiet there.
- **A subject ladder.** 254 subjects still surface five at a time.
- **Measuring itself.** Push should log each injection and whether its
  handle was followed, so the ledger reports its own usefulness.
- **Register growth.** Every segment call carries up to 3 KB of rulings
  and 2 KB of loops, newest first; a long-lived ledger will need the
  register itself to be selected by relevance.

An older peat cannot parse a v4 envelope. Every desk on a shared ledger
must run the new binary before any of them distills.
