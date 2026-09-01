# Changelog

All notable changes to peat. The ledger is the API to our past; so is this file.

Entries prefixed **Hooks:** mean the hook snippets changed: installed hooks are copied config, and you must re-sync them from `hooks/README.md` by hand. The snippet stamp there (`hooks snippet vN · date`) tells you which version you carry.

## Unreleased

- **Captures no longer force a corpus-sized flush on every turn (snippet v6).** Each `peat capture` unconditionally checkpointed, and a checkpoint rotates the memtable and *waits* for a flush that rewrites index structures sized by the whole ledger — not by the delta. On a 326 MB / ~67k-event shared ledger that meant a `Stop` hook ingesting a 15-line delta sat over seven minutes inside fjall's `rotate_memtable_and_wait`, holding the write lock and visibly freezing the session (caught live: 0 % CPU, lock held, stack in `rotate_memtable_and_wait`). Capture now folds the journal only when the work justifies it — a bulk ingest, or a journal grown past ~48 MB — and otherwise lets the journal absorb the write, which is what the next open replays anyway. The tradeoff is deliberate and documented: an un-folded capture is durable across process exit but not across power loss.
- **Every capture hook is now detached, including `Stop`.** No hook waits on a capture any more, so even the rare fold happens where nobody is watching; hooks return in ~30 ms. The closing message reaches the background process through the environment rather than being re-quoted into a command string — a hostile final message (quotes, backticks, `$VAR`, backslashes) now round-trips byte-exact, which the tests assert.
- **`peat compact`** forces a fold deliberately, for after a large backfill. Routine use should never be necessary; folding is automatic.

## 0.2.3 — 2026-08-31

- **Hooks: the Codex nudges used Claude's output shape and Codex rejected them (snippet v5, Codex only).** Claude Code accepts `additionalContext` as a top-level field; Codex requires it nested under `hookSpecificOutput` with a matching `hookEventName`. The Codex `PostToolUse` and `UserPromptSubmit` snippets sent the flat shape, so Codex failed them with `hook returned invalid post-tool-use JSON output` — once per matching tool call, loudly, in the user's face. Fixed and verified end to end: a real Codex session running `git commit` now reports `hook: PostToolUse Completed` with no error. **Re-copy the Codex snippet** (`hooks snippet v5`) and re-trust it in `/hooks`; the Claude snippet is unchanged at v4. Also corrected in the docs: Codex's `SessionStart` `source` *does* include `compact` — an earlier note here claimed otherwise — so the post-compact deposit nudge is live on both fabrics, not dormant on Codex.

## 0.2.2 — 2026-08-24

- **Hooks: salvage captures now detach, and lock waits are bounded (snippet v4).** A `SessionEnd` or `PreCompact` capture used to hold the session open for the length of the capture — seconds, for a transcript peat had not seen before — so a harness tearing down a session would abort it with `Hook cancelled`. Both salvage hooks now spawn the capture detached and return in ~10 ms; the work completes in the background. The synchronous hooks additionally cap `PEAT_LOCK_WAIT_SECS` (15 s for the wake brief, 20 s for `Stop`), below any plausible hook deadline, so contention on a shared ledger skips quietly instead of surfacing as a cancelled hook. **Re-copy the v4 snippets from `hooks/README.md`** (and on Codex, re-trust them afterwards). Nothing was lost to the old behaviour: capture is idempotent, and `peat capture <transcript>` recovers any tail that was cut short.

- **Hooks: Codex hooks do nothing until you trust them — documented.** Codex gates every non-managed command hook behind an explicit review and records trust against the hook's exact text; an untrusted hook is skipped silently, with no error and no ledger entry. Copying `.codex/hooks.json` therefore installs nothing that runs: you must also run `/hooks` in the Codex CLI to review and trust it, per project, and again after **every** re-sync this changelog asks for, because editing a hook's text marks it for review again. `hooks/README.md` gains a "Codex: hooks must be trusted before they run" section with the two gates (project trust, hook trust) and a fire-don't-read verification recipe. No snippet changes — the v3 stamps still stand.

- Codex parity is now verified end to end against 0.149 rather than read from its source: a live session confirmed the documented stdin fields arrive and that `SessionStart` stdout is injected into the Codex model's context (the model read back a marker that existed only inside the injected brief). Noted for later: Codex has a dedicated `PostCompact` event that could carry the post-compact deposit nudge, which is dormant there today because its `SessionStart` has no `compact` source.

## 0.2.1 — 2026-08-20

- **Hooks: full-coverage snippets + Codex parity confirmed.** The documented snippets (stamp: v3 · 2026-08-20) now cover all six moments — SessionStart with post-compact nudge, UserPromptSubmit once-per-session nudge, PostToolUse commit nudge, Stop capture with `--final-msg`, PreCompact and SessionEnd salvage — for both Claude Code and Codex (≥0.148 verified: `postToolUse` supported, shell tool matches matcher `Bash`). If you installed from earlier docs you have 2 of 6 moments: replace your hooks block with the v3 snippets, then re-apply any per-project edits (`PEAT_DB` anchors, `READY` gates). The install and update contract now lives in `hooks/README.md` ("Keeping hooks current"); README Install points at it.

## 0.2.0 — 2026-08-20

- **Observations carry their basis, not just their age.** Every `peat obs` now stamps the repo state it was deposited against — commit hash of `HEAD` in the depositor's cwd plus a dirty-tree flag — by construction, with no opt-in surface to forget. Subjects, brief, evidence trails, and recall hits render the anchor inline (`@abc1234+ · ~4 commits since`; `+` means the tree was dirty), so a durable rule is distinguishable from a snapshot that has since rotted. The commits-since figure is a read-time join over the day table — no git and no wall clock in any read path. Event schema is now v3 (`Obs2`); every pre-v3 observation parses forever and renders unanchored.
- **Views rebuild themselves.** First contact with this version replays the ledger through the new pipeline automatically (view schema v2); the previous database is kept one generation back at `db.old`. One-time, silent, and the ledger is untouched. **Do not open a migrated database with an older peat** — roll back by restoring `db.old` if you must downgrade.

## 0.1.1 — 2026-08-19

- **Hooks doctrine: peat never blocks.** The documented Stop hook now captures silently instead of blocking with a deposit prompt (a blocking Stop renders as a hook error and displaces the agent's reply); the observation solicitation moved to an invisible once-per-session `additionalContext` at the first user prompt. If you installed hooks from 0.1.0's docs, update your Stop hook to the new snippet in `hooks/README.md`.
- **Pre-alpha notice** at the top of the README, plus post-split link fixes.
- **Release machinery**: `just release-bump` / `release-verify` / `release` / `release-notes`, backed by a self-testing `scripts/release.py` (ported from nx-rs/anneal, jj-aware, no tag without full verification).

## 0.1.0 — 2026-08-18

First release, three days after the first commit — built and dogfooded live at Bog-A-Thon 3 (winner, agent context support) on the hackathon's own sessions, then extracted from the bogkit fork into this repo.

### The tool

- **Capture**: session transcripts (Claude Code JSONL and Codex rollouts, auto-detected) ingest into one append-forever ledger — idempotent by `(session, seq)`, never-fatal on unknown lines, incremental via a per-transcript cursor so re-capture costs O(new lines).
- **Observations**: `peat obs` deposits one judged claim; near-subject hints, citations via `--from`, backdating via `--at`.
- **Reading, dispatched by shape**: bare `peat` orients (digest, active sessions, temporal ladder, beliefs); `peat <window>` zooms (w33, 2026-07, q3, ranges); `peat <session>` overviews; `peat <subject>` reads a full evidence trail; anything else searches (BM25 ⊕ HNSW, reciprocal-rank fused). Every line of every read ends in the exact command that goes one level deeper.
- **The temporal ladder**: the brief covers the entire past in bounded lines — calendar bands widening geometrically with distance, computed at read time from the materialized day table. `--budget` re-slices; nothing is stored or recomputed.
- **Time travel**: `peat asof <date>` replays the ledger prefix through the same deterministic pipeline — the truth of that day, oracle-tested.
- **Hooks**: full session-moment coverage for Claude Code and Codex (brief at start, capture at stop/compact/end, one observation prompt per session). Multi-agent: many worktrees, one memory, single-writer lock with polite retry; `.peat/redirect` points desks at their anchor.
- **Contracts**: stdout is an API (styling only on TTYs, `--json` never clipped); no wall-clock in any fold path; additive-only schema (v2); red-capable oracles (the `--ignored` twin must fail, and CI proves it).

### Install

- Nix flake with sandboxed build (ese model pinned as fixed-output derivations), home-manager module, curl installer over GitHub Release binaries, tag-driven release CI.
