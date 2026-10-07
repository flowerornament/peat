# peat

> **Pre-alpha.** Begun August 2026, built at Bog-A-Thon 3 and in daily use by its author — but a single-user tool pinned to a patched fork of fold, with no compatibility track record yet. The ledger is designed to outlive everything (versioned envelopes, additive-only evolution); so far that promise is intent, not history. Expect the CLI, views, and hook contract to change without notice.

Agent memory as a fold. Sessions deposit their mechanical exhaust into one append-forever ledger, and every readable surface is a [bogkit/fold](https://github.com/flowercomputers/bogkit) view materialized incrementally over it. A distiller turns what the harness already wrote — compaction summaries, closing messages, the person's own words — into a paragraph per day and a register of standing rulings. Sessions end; what they learned does not.

```console
$ peat
== peat brief · 2026-10-06 ==

standing rulings (the person's instructions, newest wins; each cites the message it restates):
  single-command-hooks (27d): Hooks should be one command, not complex bash
  copied into each repo.  ▸ peat a2a9fc17 162
  no-api-billing (1d): We don't want to be paying API pricing ever.  ▸ peat 15d09ba2 9314

recent activity:
  yesterday: Redesigned the distiller around what the harness already writes:
  compaction summaries and closing messages are the leaves, one call per
  closed day. · 98 tools · 2 commits  ▸ peat 2026-10-05

further back:
  [sep] Replaced the Hnsw vector lane with an exact scan (0.3.0); unified the hooks
  into `peat hook`; wrote the peat skill. · 1.1k tools · 18 commits  ▸ peat 2026-09
```

Integration is one settings block and one installed binary; a wake reads in ~0.13 s over a 44k-event ledger, and the brief stays a bounded read however long the history grows. `capture` understands Claude Code transcripts and Codex rollouts (format auto-detected, unknown formats rejected). Subagent sessions do not fire the peat hooks — only top-level sessions deposit.

## Design

Three goals, asymmetric effort:

1. **Capture is sacred.** A session is recorded once or never; the ledger is the investment. The event schema is versioned, evolution is additive-only, and ingestion is idempotent and never-fatal.
2. **Memory is what reaches the context.** Agents do not reliably write their own memory, and an agent can only ask for what it has been shown exists. The harness already summarizes as it works, so a distiller turns those summaries — plus the person's own words, closing messages and commits — into a paragraph per closed day and a register of standing rulings, each cited to the person's message. The brief shows the whole past in words, at a resolution that fades with age, with what holds (rulings) ahead of what happened. See `.design/2026-10-07-distill-reads-the-harness.md`.
3. **Everything else is disposable.** Views are replayable from the ledger, so any view, ranking, or rendering decision can be revised for free. The session-start prompt is a hot-editable template, never a pipeline change.

Three invariants hold everywhere:

- **No wall-clock reads in any fold path.** Time enters only at the capture/render boundary (event timestamps from transcripts or the caller; age labels at print time). This is what makes `asof` replay the truth of a past day rather than a reconstruction.
- **Additive schema evolution only.** Every envelope carries the `EVENT_VERSION` it was written under (currently 3; the memory sidecar's envelopes are v4); every envelope ever written must parse forever. New variants and optional fields only. (Views are disposable and rebuild themselves from the ledger when their schema moves — the previous database is kept one generation back.)
- **Every recalled line carries its disposition.** Age, origin kind, citation status, and basis are printed inline — rank is not currency, an uncited observation is visibly a bare assertion, and an anchored claim shows the commit it was deposited against plus how far the repo has moved since (`@abc1234+ · ~4 commits since`; `+` means the tree was dirty). Claims deposited before v0.2 render unanchored.

## Architecture

One `KeyedStream<EventId, Envelope>` carries every event. Each branch opens with a `FilterMap` selecting the kinds it cares about (fold retracts whole records, so hot event kinds must not share a record with expensive branches):

```
Envelope @ (session, seq)
 ├─ day buckets      → Aggregate("days")        → Table      per-day digest: tools, fails, commits, files
 ├─ file touches     → Multimap("file_sessions")             file ↔ session index
 ├─ searchable text  → Bm25("kw")                            keyword index: obs, said, user, final, compact
 │        └─ dense text only → ese → Flat("vec")             exact-scan vector lane: everything above but said and long user messages
 │        └─ Table("texts")                                  hydration rows (kind, age, cited)
 ├─ observations     → Aggregate("subj") → Table("subjects") current understanding, newest wins
 │        └─ Multimap("evidence")                            full per-subject obs trail
 ├─ session rows     → Aggregate("sess") → Table             session summaries (span, cwd, branch, final)
 └─ ledger mirror    → Table("ledger")                       raw events, ordered — feeds asof and `events`

Distill @ (session, seq)          (the memory sidecar, .peat/memory — never the ledger)
 └─ distilled        → Aggregate("dist") → Table("distilled") newest digest / ruling per key
          └─ Multimap("dist_trail")                           every deposit, superseded ones included
```

Everything is stock fold/ese/anny. One deliberate asymmetry: **vectors index only dense text** — observations, closing messages, compaction summaries, and user messages short enough to be directives (≤400 bytes). The firehose — long pastes, mid-session assistant messages, tool calls — stays BM25-only. Embedding is the expensive lane; it is reserved for the text with the highest signal density. Recall fuses both lanes with reciprocal-rank fusion.

## Event schema

`EventId = (SessionId, u32)`. The seq layout partitions four ranges:

| range                            | meaning                                                                             |
| -------------------------------- | ----------------------------------------------------------------------------------- |
| `line_index * 16 + block_index`  | transcript-derived events (pure function of the transcript → idempotent re-capture) |
| `HOOK_FINAL_SEQ = (1 << 31) - 1` | the Stop hook's authoritative closing message                                       |
| `OBS_SEQ_BASE = 1 << 31` and up  | observations                                                                        |
| `DISTILL_SEQ_BASE = 3 << 30` and up | distilled events, in the memory sidecar only (window digests under the synthetic session `peat-distill`) |

Event kinds, in trust order:

| kind             | source                                                                                                                   | indexed                                        |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------- |
| `SessionMeta`    | transcript                                                                                                               | — (pins cwd, branch, and ese model provenance) |
| `UserMsg`        | transcript                                                                                                               | BM25                                           |
| `ToolCall`       | transcript                                                                                                               | — (day digest)                                 |
| `FileTouch`      | Edit/Write tool calls                                                                                                    | file↔session index                             |
| `Commit`         | `git commit` tool calls                                                                                                  | day digest, session rows                       |
| `Said`           | substantive mid-session assistant messages (v2)                                                                          | BM25                                           |
| `CompactSummary` | the compactor's own distillation (v2)                                                                                    | BM25                                           |
| `FinalMsg`       | transcript tail, or Stop hook (authoritative)                                                                            | BM25 + vector                                  |
| `Compaction`     | compaction markers                                                                                                       | —                                              |
| `Obs`            | a judgment step — an agent's recorded claim, with `derived_from` seqs citing the mechanical events it rests on | BM25 + vector, subjects, evidence              |
| `Distill`        | the distiller's lane, **in the memory sidecar only** (envelope v4): a day or window digest, or a ruling with its citation, source fingerprint and author | sidecar: distilled, trail |

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

**Then install the hooks — they are the product.** peat without hooks is a CLI you must remember to run; the fabric (brief on wake, capture on stop, distilling in the background) is entirely the hooks. In each project that should remember:

```console
$ peat hook install            # Claude Code: .claude/settings.json
$ peat hook install --codex    # Codex: .codex/hooks.json (then trust it in /hooks)
$ peat hook install --check    # is it wired?
```

Every hook is the same two words, `peat hook`: the binary reads the harness's stdin JSON, sees which moment it is, and does the right thing. Install merges into an existing file, keeps other tools' hooks, and replaces any peat bash snippet copied from an older release. It also lays the **peat skill** (`skills/peat/SKILL.md`, printable with `peat skill`) where the harness discovers it: the hooks cover the mechanical half, the skill teaches the judgment half — when to read, how to deposit, how far to trust a line. Because the hook config never changes again, upgrading the binary upgrades the hooks. See [`hooks/README.md`](hooks/README.md) for the contract.

## Usage

The learnable surface is two verbs; everything else is reachable from their output, because **every line of every read ends in the exact command that looks one level deeper**. With the hooks installed, nothing has to be written by hand: capture and distillation run in the background.

```console
$ peat                      # orient: the brief
$ peat <thing>              # look closer — shape decides:
$ peat 2026-w33             #   a window (w33, 2026-07, 2026-08-14, q3, 2026, a..b)
$ peat 36f96b8d             #   a session (hex id prefix; + seq for one event)
$ peat fold hnsw fix        #   anything else: search (header names the reading)
$ peat rulings              # the person's standing instructions, in full
```

The explicit subcommands below are the unambiguous spellings of the same reads, and remain in `--help`.

### `peat capture <transcript.jsonl>` — ingest a session

```console
$ peat capture ~/.claude/projects/<slug>/<session>.jsonl
captured 161 events from session b575bd9d-…
```

Parses Claude Code transcript JSONL. **Unknown or unparseable lines are skipped, never fatal** — capture must succeed on a transcript it has never seen. Every event upserts by `(session, seq)`, so re-running the same transcript is a no-op and re-running a grown transcript ingests only the delta: this is the crash-recovery story. `--session <id>` supplies a session id when the transcript lacks one; `--final-msg <text>` (passed by the Stop hook from `last_assistant_message`) is authoritative over tail parsing, which can lag at stop time.

### `peat obs <subject> <claim…>` — record one observation by hand

```console
$ peat obs fold-hnsw "intra-tx upsert strands old vectors; fixed in 640ff6f" --from 1042
near subjects: fold-hnsw-perf (1 obs)
recorded → fold-hnsw (support 2)
```

Rarely needed: what agents learn reaches memory through their replies, the harness's summaries and the distiller, and peat no longer prompts agents to write observations. For a rule none of those would carry, one short sentence (deposits over ~240 chars earn a split-this note). `--from seq,seq` cites the mechanical events the claim rests on; an uncited obs is displayed as a bare assertion everywhere it appears. **Briefs clip; trails don't**: belief lines in the brief are an index, truncated at ~120 chars and ending in `▸ peat <subject>`, which reads the full newest-wins text and the complete evidence trail verbatim. Before writing, near-subject matches print as a drift guard. The session id resolves from `--session`, else `.peat/current-session` (written by the SessionStart hook). `--at YYYY-MM-DD` backdates for retroactive annotation — `asof` briefs for that day will carry it.

### `peat distill` — write memory from what the harness wrote

```console
$ peat distill
distilled 6 days, 7 windows · 28 rulings · 12 model calls (claude)
```

Agents do not reliably write their own memory (an audit of two busy ledgers found 1,052 observations, 8 of them cited, mostly status lines), and the harness already summarizes as it works. So for each **closed local day** the distiller gathers the day's leaves — the person's own messages, the real compaction summaries Claude Code wrote, each session's closing message, and the commits — and makes one model call that writes:

- **a day digest**: one paragraph standing in for the day. Weeks, months, quarters and years merge the level below, keyed by the names the ladder uses as handles; a window with one child is that child, at no cost.
- **rulings**: standing instructions the person gave. Each is attached in code to the person's own message that carries its words, and dated by it; a ruling no message carries is discarded, and a withdrawal must name the ruling it removes.

Agent narration and messages between agents are left out: they are most of a day's text (up to 2.4 MB) and little of its meaning. Codex encrypts its compaction summaries, so Codex sessions contribute their closing messages, the person's words and commits. The day still in progress is never distilled; a closed day is re-distilled only if its leaves change (a fingerprint, never a clock).

Everything it writes lives in a **memory sidecar**, `.peat/memory`, not the ledger: it is derived and re-derivable, a newer deposit supersedes an older one while the trail keeps both, `asof` reads the trail up to its cutoff, and an older peat never sees it. `--dry-run` counts what is stale, `--limit N` bounds one run's model calls, `--since N` limits it to the last N days.

Hooks run it in the background at `SessionStart` (which also follows every compaction), and only when some session has grown since the last complete run. It runs on the CLI's subscription login: the model command's environment has `ANTHROPIC_API_KEY` removed, which would otherwise take precedence and bill API usage.

```console
$ touch .peat/distill-off   # pause distilling on this ledger (.peat/off pauses all of peat)
$ touch .peat/push          # opt in: each prompt gets up to 3 lines both search lanes agree on
```

The default model command is `claude -p` with tools, settings and session persistence off and **no model named**, so it runs on whatever model you have chosen in Claude Code and follows it through new releases. A non-empty first line of `.peat/distill` (or `PEAT_DISTILL_CMD`) replaces it; it reads the prompt on stdin and the system prompt from `$PEAT_SYSTEM`. That line is honoured only while the marker is untracked: one that arrived with a clone would otherwise run in the background on every session start. The distiller reads compaction summaries and closing messages, which can quote tool output, so injected text could reach a digest and every later brief; the prompt says the material is a record, not instructions, but nothing in code checks the result — a digest is a model's reading, and the skill says to treat it as one. No store is held across a model call.

`peat rulings` prints the register in full (`--all` adds withdrawn and superseded entries from the trail).

### `peat brief [task words…]` — the session-start prompt

Renders what holds before what happened: **standing rulings** (once the ledger has been distilled), then active sessions in the last hour, the per-day digest, **the temporal ladder** ("further back"), the last session's closing message, recently touched files, fused search hits (with `task words`), and current understanding. `--json` emits the full structure. Where a day or a band has a digest, its line says what happened in words and keeps the counts as a suffix; where it has none, it falls back to counts alone.

The ladder is bounded reading over unbounded history: the rest of the past as calendar bands that widen geometrically with distance (2 weeks, 2 months, 2 quarters, years, then one deep-past band), each an extractive digest ending in its own descent handle. It is a pure read-time regrouping of the materialized day table — nothing stored, nothing to go stale, `asof` gets it for free — and `--budget N` (or `PEAT_BRIEF_BUDGET`, default 8) re-slices without recomputing anything.

```text
further back:
  [w33 · aug 10–14] 3.7k tools (42 fail) · 123 commits · 6 sessions  ▸ peat 2026-w33
  [jul]             14k tools · 623 commits · 12 sessions            ▸ peat 2026-07
  [earlier · may 29 – aug 2] 22.9k tools · 878 commits               ▸ peat 2026-05-29..2026-08-02
```

### `peat zoom <window>` — descend

One window's digest, its children one rung finer (year → months → weeks → days → sessions), and the accountable texts inside it: closing messages, compaction summaries, observations. `peat <window>` is the short form.

### `peat recall <query…>` — search, hits only

Hybrid BM25 ⊕ exact-scan vector recall with RRF fusion. Filters: `--kind obs|said|user|final|compact`, `--since <days>`, `--session <prefix>`, `--limit N`, or `--subject <name>` to read one subject's full evidence trail instead of searching.

### `peat subjects` / `peat show <session> <seq>` / `peat events`

The claims register (every subject, newest-wins text, support count, citation status); one event in full with the observations citing it; the raw ledger oldest-first (auto-paged through `less -RFX` on a terminal). All take `--json`.

### `peat asof <YYYY-MM-DD> [task words…]` — time travel

```console
$ peat asof 2026-07-10 formal model
== peat brief · 2026-07-10 · as of that day · 14075 events ==
```

Reads every ledger event at or before the end of that **local** calendar day and folds the prefix through the *same* pipeline into a scratch database. Ages are computed relative to that day. Replay determinism is oracle-tested, which is why the result is the truth of that day and not a reconstruction. ~14k events replay in ~1.2 s.

## The database

- **Location**: `$PEAT_DB` if set, else `.peat/db` beside the nearest `.git`/`.jj` root above the working directory. `.peat/` belongs in `.gitignore`.
- **Single writer.** fold is single-writer and reads are exclusive too. Colliding invocations wait with backoff (default 120 s, tune with `PEAT_LOCK_WAIT_SECS`), then exit 75 (`EX_TEMPFAIL`) with an explanation — never a raw panic. A bulk capture can legitimately hold the lock for minutes; short hook invocations interleave transparently.
- **Durability**: bulk `capture` checkpoints the store afterward (memtable rotation) so subsequent opens do not replay a long journal; single-row writes deliberately do not, to keep the LSM's L0 healthy.
- Deleting `.peat/db` loses nothing that a re-capture of the transcripts cannot rebuild — except observations, which live only in the ledger. Back up the ledger, not the views.
- Beside the database: `memory/` (the distiller's sidecar; deleting it costs only model calls to rebuild), `memory.mark` (where the last complete sweep stopped — a cache), `distill.lock` (one distiller per ledger, holding its pid), and the optional markers `off`, `distill-off`, `push`, `distill`.

## Agent integration (Claude Code & Codex)

One command, `peat hook`, is wired to five moments (`peat hook install` does the wiring); it reads the harness's stdin JSON and dispatches on `hook_event_name`. The contract and the moment-coverage matrix live in [`hooks/README.md`](hooks/README.md). The five-moment shape:

| moment               | `peat hook` does                                                                                                                                       |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `SessionStart`       | writes `.peat/current-session`, prints the brief — **stdout is injected into the session's context**; detaches a distill sweep unless `.peat/distill-off` exists |
| `UserPromptSubmit`   | where `.peat/push` exists: up to 3 memory lines relevant to the prompt, as invisible `additionalContext`; otherwise nothing |
| `Stop`               | detached capture, `--final-msg` from `last_assistant_message` — **never blocks**                                                                       |
| `PreCompact`         | detached salvage capture before the context window is replaced |
| `SessionEnd`         | detached salvage capture on `/clear` and other non-Stop endings |

Codex ≥0.148 supports the same hook set and stdin contract; `peat hook install --codex` wires it, and the Stop rollout fallback is built in. **On Codex the hooks must also be trusted before they run** — installing writes nothing that fires; run `/hooks` in the Codex CLI to review and trust. Trust is keyed to the hook's exact text, and that text is now the constant `peat hook`, so this happens once per desk rather than after every peat release. peat failing may never break a session: `peat hook` exits 0 on every path, and does nothing at all where no ledger exists or a `.peat/off` marker is present.

## Multiple agents, one memory

Give each worktree desk a redirect to its anchor (the beads convention):

```console
$ printf '../murail/.peat\n' > .peat/redirect     # relative to the desk root
```

Bare `peat` and `peat hook` in that desk now read and write the shared ledger, while desk-local files (`current-session`, once-per-session markers) stay beside the redirect. A git worktree or secondary jj workspace needs no redirect at all: with no ledger of its own it resolves to its main checkout's `.peat/db` automatically, so a desk Claude Code or `jj workspace add` created on the fly remembers into the same mind. `PEAT_DB=/path/to/.peat/db` remains the explicit override. Writers queue on the single-writer lock (proven under 14-process load); the brief's *active in the last hour* section is the cross-agent awareness surface, and *current understanding* interleaves every agent's observations. Session exhaust from N agents becomes one mind.

## Output contract

**stdout is an API.** The SessionStart hook injects `brief` stdout verbatim into an agent's context, and `--json` is machine-read. Therefore:

- spinners and phase timings live on stderr, only when stderr is a terminal;
- color reaches stdout only when stdout is a terminal, via one style vocabulary (bold headers, cyan identities, dim metadata, red distrust signals) applied identically by the template filters and every verb;
- piped, hooked, and `--json` output is byte-for-byte unstyled — no flags needed, `NO_COLOR` and `TERM=dumb` also respected.

## Customizing the brief

`brief.tmpl` (embedded default) is the experimentation surface: a minijinja template over `BriefJson` with zero logic beyond section presence. Drop an override at `.peat/brief.tmpl` and re-run `brief` — no rebuild. Style filters available in templates: `h1`, `dim`, `warn`, `accent`, `clip(n)` (identity when stdout is not a terminal).

## Testing

Two oracles are non-negotiable and written to be **red-capable** — each has a proof it can fail:

1. **Retraction is observable**: upserting a revision over an event id makes the replaced text unfindable in both the keyword and vector indexes. An `#[ignore]`d twin asserts the opposite; `cargo test -p peat -- --ignored` must FAIL, proving the live oracle is not vacuous.
2. **Replay determinism**: folding any prefix of the ledger equals an independent scan's prediction, at multiple cut points, and transaction batching is unobservable.

Plus: idempotent double-capture, a golden test against a real (sanitized) transcript in `tests/fixtures/`, never-fatal parsing of unknown line types, ISO-8601 round-tripping, and the distiller's rules against a scripted model — leaves are the person's words, real compaction summaries, closing messages and commits (never a stored opening prompt), only closed days are distilled, a ruling must cite a message of the person's that carries its words, a withdrawal must name the ruling it removes, newer deposits supersede, and a second run over unchanged history makes no call. The ledger pipeline is pinned to ignore distilled events.

## Deferred by decision, not oversight

Belief support/flips semantics, `Merge{from,to}` subject-drift repair, session fingerprints, a `why` verb over the evidence trail, multi-writer spools; and, from the distiller's design (`.design/2026-10-07-distill-reads-the-harness.md` §4), a today/yesterday extract per session, decision-bearing messages between agents as leaves, distilled text in recall, and a person-scoped home ledger. All are replay-backfillable later precisely because capture is total. The subjects view stays deliberately dumb (newest-wins): anything cleverer must be expressible as a fold over events still visible in the raw ledger.

## Performance

Distillation runs on the model's own clock, in the background: one call per closed day plus window merges — eight days of a busy multi-agent ledger took 12 calls and under four minutes. A sweep spends at most 12 calls, and when no session has grown since the last run it reads one small table and stops.


Measured on a ~44k-event ledger (28 sessions, including 100 MB+ transcripts): `brief` 0.13 s · semantic query 0.18 s · full `asof` replay of 14k events 1.2 s. The work that made those numbers (journal checkpointing, lazy HNSW recovery, per-key pending resolution in both search sinks) landed in `fold` itself on this branch.
