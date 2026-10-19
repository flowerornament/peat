# peat is the record

Supersedes the distiller, the rulings register and the prompt-time push
of `2026-10-05-memory-reaches-context.md` and
`2026-10-07-distill-reads-the-harness.md`. Their capture findings stand;
their conclusion, that peat should write memory into the agent's context,
does not.

## Goal

peat is the record of what agents did, answerable from whatever an agent
is already looking at: a commit, a bead, a file, a phrase.

## Ideal outcome

An agent about to change a commit, pick up a bead or edit a file asks peat
one question, `peat <that thing>`, and gets the session that produced it
and the moment it happened: the person's words around it and the agent's
closing message, each ending in the command that reads the whole session.
peat puts almost nothing into a context nobody asked for. Humans keep the
full dashboard at a terminal.

## Why

The evidence, from the murail and herald ledgers and the 0.4.0 brief:

- **Agents do not write memory.** 1,052 observations across both ledgers,
  8 of them citing anything, 37–58% from a single session, mostly status
  that a commit or a bead already holds.
- **Agents rarely read it either.** About 40 explicit reads (`recall`,
  `show`, `subjects`, `zoom`, `asof`) in 129 sessions. Whatever peat
  offers is used when it answers a question the agent already has, not
  as a place to browse.
- **The 0.4.0 brief was an injection, not a tool.** About 2k tokens at
  every session start, mixing three audiences: dashboard counts for a
  human, a drill-down index, and context for the agent. Much of it was
  duplicated from beads (work), hail (coordination) and git (history),
  each of which already owns its state.
- **Rulings drift.** A model-maintained register of the person's
  instructions restated, merged and outlived what the person said; the
  harness instruction files (CLAUDE.md, AGENTS.md) and the person's own
  messages are the authority.
- **A summary of the past is a low-pass filter.** A distilled paragraph
  fixes one model's reading of what mattered. Pushed into every session,
  it anchors a later, better model on an older understanding. The record
  itself does not age that way: it can be read again by whoever reads it.
- **Commit capture recorded nothing usable.** peat guessed commits from
  `git commit` / `jj describe` command text. Herald holds 186 `Commit`
  events and 0 hashes: the events are shell fragments, and under jj the
  hash is never in the command.
- **The link from commit to session already exists.** Each repo's
  jj-identity SessionStart hook writes `Jj-Workspace:`, `Agent:` and
  `Session:` trailers into every description; about 88% of murail and
  herald trunk commits carry a `Session:` trailer.

## Decisions

1. **Remove the distiller, day and window digest paragraphs, and
   rulings.** Reason: the low-pass argument and drift above; no ledger
   ever opted in to distill, so nothing migrates. The schema stays v3;
   the `Distill`/`Lane` variants stay in the decoder, frozen, so any
   envelope ever written still parses.
2. **Remove prompt-time push.** Reason: an unasked line costs attention
   on every prompt; push is the same injection in a smaller dose. The `UserPromptSubmit` and `PostToolUse` hooks retire.
3. **Stop writing `Commit` events.** Reason: command-text guessing
   produced fragments and never a hash under jj. Old events still parse.
   Rejected: parsing tool output for the hash — fragile across jj, git
   and Codex versions, and the trailer already carries the link the
   other way.
4. **Resolve refs through the version-control record.** A commit or
   change id is looked up with a read-only `jj log`. The description's
   `Session:` trailer names the session. Without one, the commit's
   `Jj-Workspace` and committer time are matched against the session
   whose cwd is that workspace and whose active span contains the time.
   Reason: the repo's own record is the authority for commits; peat adds
   only what the repo does not hold, the conversation.
5. **Bead ids resolve by text search; file paths through the file-touch
   index.** Reason: both already exist; a bead id appears verbatim in
   the person's messages and the agent's text.
6. **The agent injection is two things.** At SessionStart, `peat hook`
   prints this desk's previous closing message — when that session is
   not the one starting and ended within three days — and one line saying
   how to look things up (`peat <commit|change|bead-id|path|words>`).
   Nothing else: no counts, no rulings, no current understanding.
   Reason: the previous closing message is the one piece of the past a
   new agent on this desk needs and does not have; a resumed or compacted
   session already holds its own, and older work anchors a fresh session
   on an old reading, so it is read on demand. The rest is pull.
7. **The dashboard stays, for humans.** Bare `peat` and `peat brief` at
   a terminal still show activity, the ladder and the last session.
   Reason: a person orienting is a different audience from an agent
   working.
8. **Observations stay as a manual verb, shown only when asked**
   (`peat subjects`, `peat <subject>`). Reason: a rare hard-won claim is
   worth keeping; showing all of them to everyone was not.
9. **Rejected: file-anchored staleness as "verified".** Marking a claim
   current because the files it names have not changed is wrong both
   ways: a claim can rot while its file stays put (the truth moved
   elsewhere), and hold while the file churns. Age and basis are shown;
   currency is the reader's judgment.

## Shape

```
transcript ──capture──► ledger (v3, append-only) ──fold──► views
                                                           ├─ sessions (span, cwd, final)
                                                           ├─ file ↔ session
                                                           ├─ keyword + vector text
                                                           ├─ days (counts), subjects
                                                           └─ ledger mirror (asof, events)

peat <ref>
  commit / change id ─► jj log (read-only) ─► Session: trailer
                                           └─► else Jj-Workspace + committer time
                                               ─► session whose cwd and span match
  bead id            ─► text search
  file path          ─► file ↔ session index
  ─► session(s) + the moment (person's messages around it, closing message)
     each ending in `peat <session> [seq]`, plus `peat asof <date>`
```

Hooks: one command, `peat hook`, on SessionStart (slim injection, session
id), Stop, PreCompact and SessionEnd (detached capture). Multi-agent: under
jj each writing agent has its own workspace, so "this desk" is this
workspace; workspaces share one ledger through `.peat/redirect` or
automatic jj/git anchor resolution.

## Not built

- **Resolution through git.** Only jj is consulted. A jj workspace has
  no `.git`, and git walks up to whatever repository encloses it,
  answering silently about the wrong history.
- **A commit view inside the fold.** Resolution reads jj at query time;
  the ledger does not mirror the repository.
- **Any replacement for the distiller.** Nothing model-written enters
  the ledger, the views or the injection.
