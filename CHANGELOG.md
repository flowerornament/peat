# Changelog

All notable changes to peat. The ledger is the API to our past; so is this file.

Entries prefixed **Hooks:** mean the hook snippets changed: installed hooks
are copied config, and you must re-sync them from `hooks/README.md` by
hand. The snippet stamp there (`hooks snippet vN · date`) tells you which
version you carry.

## Unreleased

- **Hooks: full-coverage snippets + Codex parity confirmed.** The
  documented snippets (stamp: v3 · 2026-08-20) now cover all six moments —
  SessionStart with post-compact nudge, UserPromptSubmit once-per-session
  nudge, PostToolUse commit nudge, Stop capture with `--final-msg`,
  PreCompact and SessionEnd salvage — for both Claude Code and Codex
  (≥0.148 verified: `postToolUse` supported, shell tool matches matcher
  `Bash`). If you installed from earlier docs you have 2 of 6 moments:
  replace your hooks block with the v3 snippets, then re-apply any
  per-project edits (`PEAT_DB` anchors, `READY` gates). The install and
  update contract now lives in `hooks/README.md` ("Keeping hooks
  current"); README Install points at it.

## 0.2.0 — 2026-08-20

- **Observations carry their basis, not just their age.** Every `peat obs`
  now stamps the repo state it was deposited against — commit hash of
  `HEAD` in the depositor's cwd plus a dirty-tree flag — by construction,
  with no opt-in surface to forget. Subjects, brief, evidence trails, and
  recall hits render the anchor inline (`@abc1234+ · ~4 commits since`;
  `+` means the tree was dirty), so a durable rule is distinguishable
  from a snapshot that has since rotted. The commits-since figure is a
  read-time join over the day table — no git and no wall clock in any
  read path. Event schema is now v3 (`Obs2`); every pre-v3 observation
  parses forever and renders unanchored.
- **Views rebuild themselves.** First contact with this version replays
  the ledger through the new pipeline automatically (view schema v2); the
  previous database is kept one generation back at `db.old`. One-time,
  silent, and the ledger is untouched. **Do not open a migrated database
  with an older peat** — roll back by restoring `db.old` if you must
  downgrade.

## 0.1.1 — 2026-08-19

- **Hooks doctrine: peat never blocks.** The documented Stop hook now
  captures silently instead of blocking with a deposit prompt (a blocking
  Stop renders as a hook error and displaces the agent's reply); the
  observation solicitation moved to an invisible once-per-session
  `additionalContext` at the first user prompt. If you installed hooks
  from 0.1.0's docs, update your Stop hook to the new snippet in
  `hooks/README.md`.
- **Pre-alpha notice** at the top of the README, plus post-split link
  fixes.
- **Release machinery**: `just release-bump` / `release-verify` /
  `release` / `release-notes`, backed by a self-testing
  `scripts/release.py` (ported from nx-rs/anneal, jj-aware, no tag
  without full verification).

## 0.1.0 — 2026-08-18

First release, three days after the first commit — built and dogfooded live
at Bog-A-Thon 3 (winner, agent context support) on the hackathon's own
sessions, then extracted from the bogkit fork into this repo.

### The tool

- **Capture**: session transcripts (Claude Code JSONL and Codex rollouts,
  auto-detected) ingest into one append-forever ledger — idempotent by
  `(session, seq)`, never-fatal on unknown lines, incremental via a
  per-transcript cursor so re-capture costs O(new lines).
- **Observations**: `peat obs` deposits one judged claim; near-subject
  hints, citations via `--from`, backdating via `--at`.
- **Reading, dispatched by shape**: bare `peat` orients (digest, active
  sessions, temporal ladder, beliefs); `peat <window>` zooms (w33,
  2026-07, q3, ranges); `peat <session>` overviews; `peat <subject>`
  reads a full evidence trail; anything else searches (BM25 ⊕ HNSW,
  reciprocal-rank fused). Every line of every read ends in the exact
  command that goes one level deeper.
- **The temporal ladder**: the brief covers the entire past in bounded
  lines — calendar bands widening geometrically with distance, computed
  at read time from the materialized day table. `--budget` re-slices;
  nothing is stored or recomputed.
- **Time travel**: `peat asof <date>` replays the ledger prefix through
  the same deterministic pipeline — the truth of that day, oracle-tested.
- **Hooks**: full session-moment coverage for Claude Code and Codex
  (brief at start, capture at stop/compact/end, one observation prompt
  per session). Multi-agent: many worktrees, one memory, single-writer
  lock with polite retry; `.peat/redirect` points desks at their anchor.
- **Contracts**: stdout is an API (styling only on TTYs, `--json` never
  clipped); no wall-clock in any fold path; additive-only schema (v2);
  red-capable oracles (the `--ignored` twin must fail, and CI proves it).

### Install

- Nix flake with sandboxed build (ese model pinned as fixed-output
  derivations), home-manager module, curl installer over GitHub Release
  binaries, tag-driven release CI.
