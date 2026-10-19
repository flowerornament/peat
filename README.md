# peat

> **Pre-alpha.** Begun August 2026, built at Bog-A-Thon 3 and in daily use by its author — but a single-user tool pinned to a patched fork of fold, with no compatibility track record yet. The ledger is designed to outlive everything (versioned envelopes, additive-only evolution); so far that promise is intent, not history. Expect the CLI, views, and hook contract to change without notice.

**peat is the record of what agents did, answerable from whatever an agent is already looking at.** Hooks capture every Claude Code and Codex session into one append-only ledger, and every read is a [bogkit/fold](https://github.com/flowercomputers/bogkit) view materialized incrementally over it. Point peat at a commit, a bead, a file or a few words, and it answers with the session that did the work and the moment it happened: what the person said around it and what the agent said when it finished, each line ending in the command that reads the whole thing.

```console
$ peat ac61cdf4
== ac61cdf4ef56 · change propozrt · vcs: murail/herald jj layout — gated jj-land.sh, jj-identity trailers, per-agent… ==

e4acd12f · peat (Session: trailer, active until 11d ago)
  [the person · 11d] we should move this project onto jj workflow like our other projects  ▸ peat e4acd12f 13522
  [the agent · 11d] Now the critique. Before I argue that commit trailers make most of their idea A unnecessary, …  ▸ peat e4acd12f 15122
  ▸ peat e4acd12f    ▸ peat asof 2026-10-08
```

For a commit, "the person" is what was asked just before it and "the agent" is what the session said first after it; every line ends in the command that reads it whole.

peat puts almost nothing into an agent's context unasked. At session start it hands an agent this desk's previous closing message (when that session ended within three days) and one line on how to look things up; everything else is pull. The full dashboard is for humans at a terminal (`peat`).

`capture` understands Claude Code transcripts and Codex rollouts (format auto-detected, unknown formats rejected). Subagent sessions do not fire the peat hooks; only top-level sessions are recorded.

## Design

Three goals, asymmetric effort:

1. **Capture is sacred.** A session is recorded once or never; the ledger is the investment. The event schema is versioned, evolution is additive-only, and ingestion is idempotent and never-fatal.
2. **The record answers questions; it does not push a reading.** A summary of the past is one model's view of what mattered, and pushed into every session it anchors a later, better model on an older understanding. peat keeps the conversation itself and finds the part an agent asks about. Version control owns commits, beads own work, the instruction files own rules; peat links to them and adds only what they lack: what was said. See `.design/2026-10-19-peat-is-the-record.md`.
3. **Everything else is disposable.** Views are replayable from the ledger, so any view, ranking, or rendering decision can be revised for free. The dashboard is a hot-editable template, never a pipeline change.

Three invariants hold everywhere:

- **No wall-clock reads in any fold path.** Time enters only at the capture/render boundary (event timestamps from transcripts or the caller; age labels at print time). This is what makes `asof` replay the truth of a past day rather than a reconstruction.
- **Additive schema evolution only.** Every envelope carries the `EVENT_VERSION` it was written under (currently 3); every envelope ever written must parse forever. New variants and optional fields only. (Views are disposable and rebuild themselves from the ledger when their schema moves; the previous database is kept one generation back.)
- **Every recalled line carries its disposition.** Age, origin kind, citation status and basis are printed inline: rank is not currency, and an uncited observation is visibly a bare assertion. An observation shows the commit it was deposited against (`@abc1234+`; `+` means the tree was dirty); to see what changed since, ask jj. peat does not mark a claim "verified" because its files are unchanged: that is wrong both ways.

## Architecture

One `KeyedStream<EventId, Envelope>` carries every event. Each branch opens with a `FilterMap` selecting the kinds it cares about (fold retracts whole records, so hot event kinds must not share a record with expensive branches):

```
Envelope @ (session, seq)
 ├─ day buckets      → Aggregate("days")        → Table      per-day counts: tools, fails, files
 ├─ file touches     → Multimap("file_sessions")             file ↔ session index
 ├─ searchable text  → Bm25("kw")                            keyword index: obs, said, user, final, compact
 │        └─ dense text only → ese → Flat("vec")             exact-scan vector lane: everything above but said and long user messages
 │        └─ Table("texts")                                  hydration rows (kind, age, cited)
 ├─ observations     → Aggregate("subj") → Table("subjects") newest-wins claim per subject
 │        └─ Multimap("evidence")                            full per-subject obs trail
 ├─ session rows     → Aggregate("sess") → Table             session summaries (span, cwd, branch, final)
 └─ ledger mirror    → Table("ledger")                       raw events, ordered — feeds asof and `events`
```

Everything is stock fold/ese/anny. One deliberate asymmetry: **vectors index only dense text** — observations, closing messages, compaction summaries, and user messages short enough to be directives (≤400 bytes). The firehose — long pastes, mid-session assistant messages, tool calls — stays BM25-only. Recall fuses both lanes with reciprocal-rank fusion.

Commits are not mirrored into the fold. `peat <commit>` reads the repository at query time (read-only `jj log`) and joins what it finds against the session rows.

## Event schema

`EventId = (SessionId, u32)`. The seq layout partitions the ranges:

| range                               | meaning                                                                             |
| ----------------------------------- | ----------------------------------------------------------------------------------- |
| `line_index * 16 + block_index`     | transcript-derived events (pure function of the transcript → idempotent re-capture) |
| `HOOK_FINAL_SEQ = (1 << 31) - 1`    | the Stop hook's authoritative closing message                                       |
| `OBS_SEQ_BASE = 1 << 31` and up     | observations                                                                        |
| `DISTILL_SEQ_BASE = 3 << 30` and up | reserved: distilled events (0.4.0, never written to a ledger)                       |

Event kinds, in trust order:

| kind             | source                                                                                                 | indexed                                        |
| ---------------- | ------------------------------------------------------------------------------------------------------ | ---------------------------------------------- |
| `SessionMeta`    | transcript                                                                                             | — (pins cwd, branch, and ese model provenance) |
| `UserMsg`        | transcript                                                                                             | BM25                                           |
| `ToolCall`       | transcript                                                                                             | — (day counts)                                 |
| `FileTouch`      | Edit/Write tool calls                                                                                  | file↔session index                             |
| `Said`           | substantive mid-session assistant messages (v2)                                                        | BM25                                           |
| `CompactSummary` | the compactor's own summary (v2; Claude Code only, Codex encrypts its own)                             | BM25                                           |
| `FinalMsg`       | transcript tail, or Stop hook (authoritative)                                                          | BM25 + vector                                  |
| `Compaction`     | compaction markers                                                                                     | —                                              |
| `Obs`            | an agent's recorded claim, with `derived_from` seqs citing the events it rests on                      | BM25 + vector, subjects, evidence              |
| `Commit`         | **no longer written.** Before 0.5.0, guessed from `git commit` / `jj describe` command text; still parses | —                                              |
| `Distill`        | **never in a ledger.** 0.4.0's sidecar lane; the variant stays in the decoder, frozen                  | —                                              |

Stored text is capped (`UserMsg` 2 KB, `FinalMsg` 8 KB, tool detail 500 B) at char boundaries. `--json` output always carries full stored text; clipping is display-only.

## Install

**curl** (prebuilt binaries: macOS arm64, Linux x86_64/arm64):

```console
$ curl -fsSL https://raw.githubusercontent.com/flowerornament/peat/main/install.sh | bash
```

**nix** (sandboxed build; the ese embedding model is pinned as fixed-output derivations):

```console
$ nix run github:flowerornament/peat -- brief
```

or as a flake input with the home-manager module:

```nix
inputs.peat.url = "github:flowerornament/peat?ref=refs/heads/release";
# ...
programs.peat.enable = true;
```

**from source** (first build downloads the ese model — slow once):

```console
$ cargo build --release
$ cargo test              # one #[ignore]d twin is SUPPOSED to fail when run
```

fold/ese/anny arrive as git dependencies pinned by rev to the [bogkit fork](https://github.com/flowerornament/bogkit) (upstream PR flowercomputers/bogkit#18).

**Then install the hooks.** Without them peat records nothing. In each project:

```console
$ peat hook install            # Claude Code: .claude/settings.json
$ peat hook install --codex    # Codex: .codex/hooks.json (then trust it in /hooks)
$ peat hook install --check    # is it wired?
```

Every hook is the same two words, `peat hook`, on four events: `SessionStart`, `Stop`, `PreCompact`, `SessionEnd`. The binary reads the harness's stdin JSON, sees which moment it is, and does the right thing. Install merges into an existing file, keeps other tools' hooks, replaces any peat bash snippet copied from an older release, and removes peat from events it no longer uses. It also lays the **peat skill** (`skills/peat/SKILL.md`, printable with `peat skill`) where the harness discovers it: when to look something up, and how far to trust what comes back. Because the hook command never changes, upgrading the binary upgrades the hooks. See [`hooks/README.md`](hooks/README.md) for the contract.

## Usage

The learnable surface is one verb with a shape-dispatched argument. **Every line of every read ends in the exact command that looks one level deeper.**

```console
$ peat                      # the dashboard (for a person at a terminal)
$ peat <thing>              # look it up — shape decides:
$ peat 3f9a2c1e             #   a commit or jj change id: the session that made it, and the moment
$ peat peat-520.1           #   a bead id: the sessions that mention it
$ peat src/hook.rs          #   a file path: the sessions that touched it
$ peat 2026-w33             #   a window (w33, 2026-07, 2026-08-14, q3, 2026, a..b)
$ peat a2a9fc17             #   a session (hex id prefix; + seq for one event)
$ peat fold hnsw fix        #   anything else: search (header names the reading)
```

The explicit subcommands below are the unambiguous spellings of the same reads, and remain in `--help`.

### `peat <ref>` — from a thing to the conversation behind it

- **A commit or change id** resolves through jj, read-only (`jj log`). The commit description's `Session:` trailer names the session; repos running a jj-identity SessionStart hook write `Jj-Workspace:`, `Agent:` and `Session:` trailers on every description. Without a trailer, the commit's `Jj-Workspace` and committer time are matched against the session whose cwd is that workspace and whose active span contains the time; the output says which way it resolved.
- **A bead id** resolves by text search: the sessions whose messages name it.
- **A file path** resolves through the file-touch index: the sessions that edited or wrote it.

Each answer is the session (or sessions) and the moment: the person's messages around it and the agent's closing message, each ending in `▸ peat <session> [seq]`, plus `peat asof <date>` for what was known that day.

### `peat capture <transcript.jsonl>` — ingest a session

```console
$ peat capture ~/.claude/projects/<slug>/<session>.jsonl
captured 161 events from session b575bd9d-…
```

Parses Claude Code transcript JSONL and Codex rollouts. **Unknown or unparseable lines are skipped, never fatal** — capture must succeed on a transcript it has never seen. Every event upserts by `(session, seq)`, so re-running the same transcript is a no-op and re-running a grown transcript ingests only the delta: this is the crash-recovery story. `--session <id>` supplies a session id when the transcript lacks one; `--final-msg <text>` (passed by the Stop hook from `last_assistant_message`) is authoritative over tail parsing, which can lag at stop time. The hooks run it; by hand it is for recovering a transcript after a cancelled hook.

### `peat brief` — the dashboard

What bare `peat` prints: sessions active in the last hour, recent days as counts, **the temporal ladder** ("further back"), the last session's closing message, and recently touched files. `--json` emits the full structure. It is for a person orienting; the hooks do not inject it.

The ladder is bounded reading over unbounded history: the rest of the past as calendar bands that widen geometrically with distance (2 weeks, 2 months, 2 quarters, years, then one deep-past band), each ending in its own descent handle. It is a pure read-time regrouping of the materialized day table — nothing stored, nothing to go stale, `asof` gets it for free — and `--budget N` (or `PEAT_BRIEF_BUDGET`, default 8) re-slices without recomputing anything.

```text
further back:
  [w33 · aug 10–14] 3.7k tools (42 fail) · 6 sessions  ▸ peat 2026-w33
  [jul]             14k tools · 12 sessions            ▸ peat 2026-07
  [earlier · may 29 – aug 2] 22.9k tools               ▸ peat 2026-05-29..2026-08-02
```

### `peat zoom <window>` — descend

One window's counts, its children one rung finer (year → months → weeks → days → sessions), and the accountable texts inside it: closing messages, compaction summaries, observations. `peat <window>` is the short form.

### `peat recall <query…>` — search, hits only

Hybrid BM25 ⊕ exact-scan vector recall with RRF fusion. Filters: `--kind obs|said|user|final|compact`, `--since <days>`, `--session <prefix>`, `--limit N`, or `--subject <name>` to read one subject's full evidence trail instead of searching. `peat <words>` is the short form.

### `peat show <session> [seq]` / `peat events`

One session's overview, or one event in full with the observations citing it; the raw ledger oldest-first (auto-paged through `less -RFX` on a terminal). Both take `--json`.

### `peat asof <YYYY-MM-DD> [task words…]` — time travel

```console
$ peat asof 2026-07-10 formal model
== peat brief · 2026-07-10 · as of that day · 14075 events ==
```

Reads every ledger event at or before the end of that **local** calendar day and folds the prefix through the *same* pipeline into a scratch database. Ages are computed relative to that day. Replay determinism is oracle-tested, which is why the result is the truth of that day and not a reconstruction. ~14k events replay in ~1.2 s.

### `peat obs <subject> <claim…>` / `peat subjects` — a claim by hand

```console
$ peat obs fold-hnsw "intra-tx upsert strands old vectors; fixed in 640ff6f" --from 1042
near subjects: fold-hnsw-perf (1 obs)
recorded → fold-hnsw (support 2)
```

For a hard-won claim that will outlive the work and that no commit, bead or instruction file would carry. One short sentence (deposits over ~240 chars earn a split-this note). `--from seq,seq` cites the events the claim rests on; an uncited obs is displayed as a bare assertion everywhere it appears. Before writing, near-subject matches print as a drift guard. The session id resolves from `--session`, else `.peat/current-session` (written by the SessionStart hook). `--at YYYY-MM-DD` backdates for retroactive annotation.

Observations are shown only when asked: `peat subjects` lists every subject (newest-wins text, support count, citation status), and `peat <subject>` reads one subject's full trail verbatim. peat never asks an agent to write one.

### `peat compact`

Folds the journal into the LSM now — the maintenance cost that captures otherwise defer. Safe any time; takes minutes on a large ledger.

## The database

- **Location**: `$PEAT_DB` if set, else `.peat/db` beside the nearest `.git`/`.jj` root above the working directory. `.peat/` belongs in `.gitignore`.
- **Single writer.** fold is single-writer and reads are exclusive too. Colliding invocations wait with backoff (default 120 s, tune with `PEAT_LOCK_WAIT_SECS`), then exit 75 (`EX_TEMPFAIL`) with an explanation — never a raw panic. A bulk capture can legitimately hold the lock for minutes; short hook invocations interleave transparently.
- **Durability**: bulk `capture` checkpoints the store afterward (memtable rotation) so subsequent opens do not replay a long journal; single-row writes deliberately do not, to keep the LSM's L0 healthy.
- Deleting `.peat/db` loses nothing that a re-capture of the transcripts cannot rebuild — except observations, which live only in the ledger. Back up the ledger, not the views.
- Beside the database: `current-session` (this desk's live session id), `redirect` (where a desk's shared ledger lives), and the pause marker `off`. Files left by 0.4.0 (`memory/`, `memory.mark`, `distill.lock`, `distill`, `distill-off`, `push`) are unused and can be deleted.

## Agent integration (Claude Code & Codex)

One command, `peat hook`, is wired to four moments (`peat hook install` does the wiring); it reads the harness's stdin JSON and dispatches on `hook_event_name`. The contract and the moment-coverage matrix live in [`hooks/README.md`](hooks/README.md).

| moment         | `peat hook` does                                                                                                                                                                     |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `SessionStart` | writes `.peat/current-session`; prints this desk's previous closing message (a different session, ended within three days) and one line on how to look things up — **stdout is injected into the session's context** |
| `Stop`         | detached capture, `--final-msg` from `last_assistant_message` — **never blocks**                                                                                                    |
| `PreCompact`   | detached salvage capture before the context window is replaced                                                                                                                      |
| `SessionEnd`   | detached salvage capture on `/clear` and other non-Stop endings                                                                                                                     |

Codex ≥0.148 supports the same hook set and stdin contract; `peat hook install --codex` wires it, and the Stop rollout fallback is built in. **On Codex the hooks must also be trusted before they run** — installing writes nothing that fires; run `/hooks` in the Codex CLI to review and trust, once per desk. peat failing may never break a session: `peat hook` exits 0 on every path, and does nothing at all where no ledger exists or a `.peat/off` marker is present.

## Multiple agents, one record

Under jj each writing agent has its own workspace, and a desk is a workspace: "this desk's last closing message" means the last session whose cwd was this workspace. Workspaces share one ledger. A git worktree or secondary jj workspace with no ledger of its own resolves to its main checkout's `.peat/db` automatically; elsewhere, give the desk a redirect to its anchor (the beads convention):

```console
$ printf '../murail/.peat\n' > .peat/redirect     # relative to the desk root
```

Bare `peat` and `peat hook` in that desk now read and write the shared ledger, while desk-local files (`current-session`) stay beside the redirect. `PEAT_DB=/path/to/.peat/db` remains the explicit override. Writers queue on the single-writer lock (proven under 14-process load). The `Jj-Workspace:` trailer is what lets `peat <commit>` find the right desk's session when a commit carries no `Session:` trailer.

## Output contract

**stdout is an API.** The SessionStart hook's stdout is injected verbatim into an agent's context, and `--json` is machine-read. Therefore:

- spinners and phase timings live on stderr, only when stderr is a terminal;
- color reaches stdout only when stdout is a terminal, via one style vocabulary (bold headers, cyan identities, dim metadata, red distrust signals) applied identically by the template filters and every verb;
- piped, hooked, and `--json` output is byte-for-byte unstyled — no flags needed, `NO_COLOR` and `TERM=dumb` also respected.

## Customizing the dashboard

`brief.tmpl` (embedded default) is the experimentation surface: a minijinja template over `BriefJson` with zero logic beyond section presence. Drop an override at `.peat/brief.tmpl` and re-run `brief` — no rebuild. Style filters available in templates: `h1`, `dim`, `warn`, `accent`, `clip(n)` (identity when stdout is not a terminal).

## Testing

Two oracles are non-negotiable and written to be **red-capable** — each has a proof it can fail:

1. **Retraction is observable**: upserting a revision over an event id makes the replaced text unfindable in both the keyword and vector indexes. An `#[ignore]`d twin asserts the opposite; `cargo test -p peat -- --ignored` must FAIL, proving the live oracle is not vacuous.
2. **Replay determinism**: folding any prefix of the ledger equals an independent scan's prediction, at multiple cut points, and transaction batching is unobservable.

Plus: idempotent double-capture, a golden test against a real (sanitized) transcript in `tests/fixtures/`, never-fatal parsing of unknown line types, and ISO-8601 round-tripping.

## Deferred by decision, not oversight

Belief support/flips semantics, `Merge{from,to}` subject-drift repair, session fingerprints, a `why` verb over the evidence trail, and multi-writer spools. Commit resolution reads jj only, never git: in a jj workspace git walks up to whatever repository encloses it and answers about the wrong history. Model-written summaries of the past are not deferred: they were built in 0.4.0 and removed in 0.5.0 (`.design/2026-10-19-peat-is-the-record.md`). Everything else is replay-backfillable later precisely because capture is total.

## Performance

Measured on a ~44k-event ledger (28 sessions, including 100 MB+ transcripts): `brief` 0.13 s · semantic query 0.18 s · full `asof` replay of 14k events 1.2 s. The work that made those numbers (journal checkpointing, lazy HNSW recovery, per-key pending resolution in both search sinks) landed in `fold` itself on this branch.
