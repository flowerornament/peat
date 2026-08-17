# The peat ladder — bounded briefs over an unbounded ledger

Prior art: VictorTaelin/OptMem (dyadic summary tree, reading budgets) as
dissected by herald's `2026-07-29-the-temporal-ladder.md` (fact/fold split,
budget as read parameter). This document maps both onto peat's existing
machinery. Design goal: the brief reads a bounded number of lines however
old the ledger grows, covers the *entire* past, and every line is
descendable — with **zero new pipeline sinks and zero schema changes**.

## 1. The problem, concretely

Today's brief is a recency cliff:

```
coverage
│ ██████████ today          (day digest, active-now, last session, hits)
│ ██████     yesterday      (day digest line)
│                           ← cliff. everything older exists only if
│                             recall is asked the right words, or
│                             asof is asked the right date.
└──────────────────────────────────────────────── ledger age →
```

At 44k events / 28 sessions this is fine. At a year of sessions, the brief
describes 2% of what the ledger knows, and nothing points at the rest.

OptMem's shape — what we want the brief to look like instead:

```
resolution
│ full ██████
│      ██████ ▓▓▓▓
│      ██████ ▓▓▓▓ ▒▒▒▒▒▒
│      ██████ ▓▓▓▓ ▒▒▒▒▒▒ ░░░░░░░░░░░░░░░░
└──────┴──────┴────┴──────┴──────────────── distance from now
       today  week  month  everything else
       (each band geometrically wider, one rung coarser, one line-ish each)
```

Bounded lines, total coverage, descent on demand.

## 2. The elegant core: the ladder is a *read-time regrouping*

The insight that makes this cheap: **peat already materializes rung 0.**
`Aggregate("days") → Table("days_tbl")` holds one `DayStats` per day —
counts, fails, commits, per-file touch map. A year is ≤366 rows. Reading
and regrouping *all of them* is microseconds.

So higher rungs are not new sinks. They are a pure function, at render
time, over the day table:

```
                     WRITE PATH (unchanged)                READ PATH (new, pure)
                                                        ┌──────────────────────────┐
  Envelope ─ FilterMap ─ Aggregate("days") ─ Table ───▶ │ ladder(days, now, budget) │
                                                        │  = Vec<Band>              │
             (nothing else added — no new keyspaces,    └──────────────────────────┘
              no backfill, no migration, asof-correct
              for free: asof calls the same function
              with its cutoff as `now`)
```

This respects every invariant by construction:

- **No ambient time in the fold** — `now` enters at the render boundary,
  where it already legally lives (age labels do this today).
- **Deterministic** — same day rows + same `now` + same budget → same
  bands. The replay oracle covers it like any other read.
- **Replayable** — nothing is stored, so nothing can go stale or need
  rebuild. OptMem's "changing the budget recomputes nothing" is not a
  property we must engineer; it falls out.

Herald's fact/fold question (are summaries deterministic folds or
accountable assertions?) answers itself for peat, because peat already has
both lanes:

| herald lane | peat, today | peat, with ladder |
|---|---|---|
| A — mechanical rungs (deterministic folds) | `DayStats` | read-time regrouping of `DayStats` into weeks/months/… |
| B — accountable summaries (admitted content, never recomputed) | `FinalMsg`, `CompactSummary`, `Obs` — already *events in the ledger* | unchanged; bands **count** them and descent **reveals** them |

No new mechanism on either lane. The ladder's bands are lane A; the texts
you find when you descend are lane B.

## 3. Windows: calendar, not dyadic

OptMem pairs leaves dyadically because positions are its identity. peat's
rung 0 is already calendar days, and agents and humans cite "last week",
"August". So the schedule is calendar-geometric:

```
rung   window     width   ~ratio
 0     day         1 d      —      (materialized: days_tbl)
 1     week        7 d      ×7     (ISO week)
 2     month     ~30 d      ×4.3
 3     quarter   ~91 d      ×3
 4     year     ~365 d      ×4
```

Geometric *property* kept (each rung several times wider), binary purity
traded for addressability: every window has a human-readable name that
doubles as the descent handle — `2026-08-16`, `w33`, `2026-08`, `q3`,
`2026`.

## 4. Band selection: the budget algorithm

```
fn ladder(days: &BTreeMap<Day, DayStats>, now: Day, budget: usize) -> Vec<Band>

  frontier = now
  bands = []
  rung = 0
  while frontier covers less than the oldest day:
      w = the next window at `rung` walking backward from frontier
      bands.push(digest(w))
      frontier = start of w
      promote: after 2 windows at a rung, rung += 1        // geometric fall-off
      if bands.len() == budget:
          rung = MAX                                       // one final band:
          bands.push(digest(oldest..frontier))             // "the deep past"
          break
```

Walking backward from today: 2 days, 2 weeks, 2 months, 2 quarters, then
years, then one "everything before" band. Full history at any budget ≥ 3;
the budget only controls where coarsening starts. `--budget N` on `brief`
(default 10 bands; `PEAT_BRIEF_BUDGET` for hooks). Changing it re-slices
the same day rows — nothing recomputed, exactly OptMem's `WAKE_LINES`.

## 5. What a band line says

A band is an extractive digest of its window's `DayStats` sum, plus counts
of the accountable texts inside it, plus its own descent handle:

```
== peat brief · 2026-08-17 ==

recent activity:
  today: 12 tool calls, 1 commit · …/the-peat-ladder.md
  yesterday: 223 tools (7 fail), 26 commits · …/pr-draft.md, …/hnsw.rs, …

further back:                                          ← the new section
  [w33 · aug 11–17]  641 tools · 38 commits · 9 sessions · 14 obs   ▸ peat zoom w33
  [w32 · aug 4–10]   87 tools · 6 commits · 2 sessions · 1 obs      ▸ peat zoom w32
  [jul]              1.4k tools · 102 commits · 11 sessions · 6 obs ▸ peat zoom 2026-07
  [jun]              rebuilt murail formal model · 890 tools · …    ▸ peat zoom 2026-06
  [q1–q2]            2.1k tools · 240 commits · 31 sessions         ▸ peat zoom 2026-h1
```

Top touched files name a window when they dominate it (the `files` map is
already in `DayStats`); otherwise counts carry the line. Every line ends in
the handle that descends into it.

## 6. Descent: `peat zoom <window>`

`zoom` is the same regrouping pointed at one window, one rung finer, plus
the lane-B texts the band only counted:

```console
$ peat zoom w33
== w33 · aug 11–17 · 641 tools · 38 commits · 9 sessions ==

by day:
  [aug 16]  223 tools (7 fail) · 26 commits · …/hnsw.rs, …/main.rs   ▸ peat zoom 2026-08-16
  [aug 15]  103 tools (6 fail) · 1 commit · …/vcs/jj.md              ▸ peat zoom 2026-08-15
  …

closing messages:                       ← FinalMsg events in range (lane B)
  [b575bd9d · aug 16] Review cycle closed — verified 27b80ea independently…
  [90c2c05c · aug 16] Peat is built and green. Full status…

observations:                           ← Obs events in range (lane B)
  fold-hnsw (aug 16, cited): intra-tx upsert strands old vectors; fixed…
```

```console
$ peat zoom 2026-08-16          # one rung further down
== aug 16 · 223 tools · 26 commits · 3 sessions ==
sessions:
  [b575bd9d · 10:04–19:42] 15 commits · …    ▸ peat events --session b575bd9d
  …
```

The ladder bottoms out in verbs that already exist: `events --session`,
`show <session> <seq>`, `recall --since`. Descent grammar:

```
brief ▸ zoom <year|quarter|month|week|day> ▸ events/show/recall
        └── every rung's output ends in the next rung's exact command
```

The handle-in-the-output convention matters for the primary consumer: an
*agent* reading its brief can descend by running the printed command —
navigation becomes tool-use, no new protocol.

Window resolution reuses the `asof` date plumbing; range reads over the
ledger mirror are an iterate-and-filter at first (44k events — fine), with
a ts-ordered secondary key noted as the scaling escape hatch.

## 7. What we are *not* taking from OptMem

- **LM-written rung summaries.** OptMem compresses pairs with model calls;
  herald flags exactly why that is heavy (determinism vs accountability).
  peat's lane-B texts (finals, compact summaries, obs) already exist and
  cost nothing extra; a `peat distill <window>` verb that *deposits an
  accountable window summary as an ordinary Obs* (subject `w33`, cited
  `--from` the window's finals) is a clean later addition — it needs no
  new machinery, only a habit. Deferred.
- **Navigation as the only recall.** peat keeps search-first (BM25 ⊕ HNSW);
  the ladder adds the navigate-first axis beside it, not instead of it.
- **Manual-only capture.** Mechanical exhaust cannot lie; that asymmetry
  stays.

Style items adopted into the README when this ships: integration cost in
the lead ("one settings.json block"), wake-time as the headline metric,
and one sentence of subagent doctrine (subagent sessions do not fire the
peat hooks; only top-level sessions deposit).

## 8. Slice order

1. `ladder()` + `further back` section in brief (pure read-time; budget
   flag) — the whole value is visible here.
2. `peat zoom` day/week/month with lane-B texts and printed handles.
3. Quarter/year + deep-past terminal band; `PEAT_BRIEF_BUDGET`.
4. (later) `peat distill` accountable window summaries; ts-ordered ledger
   key when iterate-and-filter shows up in timings.

Success is measured the OptMem way: the brief reads a bounded number of
lines whatever the ledger's length, and any question about the far past is
answerable by descent alone — no search query, no date arithmetic.
