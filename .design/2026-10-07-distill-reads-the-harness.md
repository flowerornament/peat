# Distill reads what the harness already wrote

Supersedes the per-stretch distiller of
`2026-10-05-memory-reaches-context.md` §3.2–3.6 (that document's
principles stand; its mechanism does not). Reviewed by a second peat
agent before building (bead peat-3mk).

## 1. Where peat events fall in a session

```
 time   session (Claude or Codex)          ledger (main)                     written by
  │
  ├── SessionStart ──────────────────────► brief injected; sweep detached
  │     prompt / turn / Stop (every turn) ► UserMsg [u], ToolCall, Said,      the capture hooks
  │                                          Commit, FinalMsg
  │   context full ── PreCompact ────────► capture
  │   harness compacts ──────────────────► Compaction marker                  the harness
  │                                        + CompactSummary  (Claude only:
  │                                          readable, ~1.3 KB, cumulative)
  │   SessionStart(compact) ─────────────► brief; sweep detached
  │     ...
  │   SessionEnd ────────────────────────► capture; FinalMsg = last reply     the agent
  ▼
 memory sidecar (.peat/memory)
  │   per closed local day ──────────────► day paragraph + rulings          one model call
  │   per week / month / quarter / year ─► merged paragraph                   one call each (or free)
  ▼
```

## 2. Decisions and their evidence

| # | Decision | Evidence |
|---|---|---|
| 1 | **No per-stretch model call.** The day's leaves are what the harness and the person already wrote. | Logs: Claude Code's compaction summaries already carry the request, decisions, the person's instructions (herald's are headed "Standing rulings from Morgan" and quote them), files, errors and pending work. Summarizing the log again is shadow compaction with less context. |
| 2 | **A day's input is the person's own words, the real compaction summaries, each session's closing message and the day's commits.** Agent narration and peer messages are left out. | Logs, last 7 days: the person's own words are 1–66 KB/day per repo (~8 KB typical); agent text reaches 2.4 MB and peer messages 1.1 MB on a busy herald day. The high-value slice is the small one. |
| 3 | **Codex compactions contribute nothing; the capture fallback that stored a "summary" is removed.** | Logs: Codex encrypts its compaction summary (`replacement_history` ends in a `compaction` item with empty text). peat fell back to item 0, the session's opening prompt. Herald: 97 real summaries, 2,757 opening prompts stored as summaries; murail: 121 vs 1,964. Codex days are carried by own words, closing messages and commits. |
| 4 | **Summaries are for orientation; recall still searches the full kept text.** Distilled text is not added to the search lanes. | Graph: *active paginated retrieval over full history outperforms fixed lossy summarization* (strong; MemGPT, 92.5% vs 32% on narrow recall). |
| 5 | **Distilled events live in a sidecar store, not the ledger.** The ledger stays schema v3. | Graph: *memory as a derived layer versus log-primary total provenance* (moderate; Nakajima) — memory is a projection the log can rebuild. Logs/ops: the 2026-10-06 rollback left the nix lock on a withdrawn commit; an older binary panics on any v4 envelope in the main ledger. A sidecar loses digests on downgrade instead of bricking the ledger, and the "upgrade every desk first" rule disappears. |
| 6 | **Only closed days are distilled; a closed day is re-distilled only if its leaves change.** No live-day re-summarizing. | Graph: *memory evolution refinement and corruption are opposed risks of the same backward-update mechanism* (moderate; A-Mem). Re-writing the same summary as a day unfolds is that mechanism, with no gain: today's state is already in the closing message and the latest compaction summary. |
| 7 | **Rulings stay, cited in code** against the person's own message that carries the ruling's words; uncited rulings are dropped. Withdrawals must share ≥ 2 words with the ruling. | Graph: *reflection … synthesizes recent records into higher-level, cited insights* (strong; Park et al.). Logs: of 1,052 agent observations, 8 cited anything; citations do not happen unless enforced. |
| 8 | **Loops are cut.** | Review + logs: both busy repos track work in beads; a second, model-maintained open-work register duplicates it and needs closing logic that drifts (the 2026-10-06 comparison opened 6 loops for 2 sessions, several not loops at all). Graph: *autonomous memory organization adaptability and debuggable structure are opposed* (moderate) — fewer model-maintained structures. |
| 9 | **Prompts state the purpose and the reply format, nothing else.** | Anthropic's current guidance (Claude Fable 5, Opus 5.5 prompting docs): give the reason, not only the request; skills written for older models "are often too prescriptive" and can degrade output. |
| 10 | **The default command names no model** (`claude -p`, tools and settings off, `ANTHROPIC_API_KEY` removed). | Measured: resolves to the user's chosen model (Opus 5.5), ~10 s/call, better digests than Haiku at a fraction of the time. The person: never metered API billing; Haiku is not worth the savings. |
| 11 | **The obs nudges are removed**; `peat obs` stays as a manual command. | Logs: 1,052 observations, 8 cited, 37–58% from a single session, mostly status that the commit or bead already records; ~40 explicit reads across 129 sessions. |
| 12 | **The sweep runs at SessionStart only, and scans the ledger only when some session grew since the last run.** | Hooks: a compaction restarts the session, so SessionStart also follows every compaction. Logs: every candidate costs a full ledger scan (274k events in herald). |
| 13 | **Push stays opt-in, gated on agreement of both search lanes.** | Graph: *as index size grows while attention stays constant, precision matters more than recall* (strong; Brin & Page). |
| 14 | **Days are local calendar days.** | Review: UTC buckets split a Pacific day at 17:00. Distill is not a fold path, so reading the offset there keeps the no-clock invariant. |

## 3. Shape

- **Sidecar.** `.peat/memory/` is a second fold store holding only
  `Distill` events (`digest` and `ruling` lanes): a newest-per-key table and
  a full trail. The brief opens it after the ledger; `asof` reads the trail
  up to its cutoff. Deleting it loses only model work, which the ledger can
  re-derive.
- **The day call.** Input: the standing rulings and the previous days'
  paragraphs (bounded), then the day's leaves grouped by session. Output:
  `{"digest": …, "rulings": [{"subject", "text"}]}`. Citation and
  supersession happen in code.
- **Windows.** Week, month, quarter, year merge their children as before;
  a window with one child is that child.
- **The sweep's mark.** `memory/settled` holds the newest session end the
  last complete run covered; a run that hit its call limit does not advance
  it.

## 4. Not built (and why)

- **A today/yesterday extract per session in the brief.** It needs the
  latest compaction summary per session, which only a ledger scan can
  find; the brief must stay a bounded read. Today is covered by the last
  session's closing message, as before.
- **Peer-message signal.** Hail rulings and go/done messages carry real
  decisions in the multi-agent repos but are 6–1,129 KB/day; selecting the
  decision-bearing ones is its own design.
- **Distilled text in recall.** Decision 4.
- **An importance trigger** instead of a calendar one (Park's reflection
  fires on accumulated importance). A closed day is a simpler, auditable
  trigger; a near-empty day costs one short call.
