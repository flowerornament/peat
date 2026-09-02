# Vector index: flat exact scan now, graph-in-store as the destination

Date: 2026-09-02. Status: accepted. Supersedes nothing; amends the founding
spec's "vector index = fold Hnsw" line by replacing the sink, not the lane.

## Decision

Replace fold's in-memory HNSW sink with an exact-scan sink over the persisted
vector rows (**C**), and commit to a store-resident HNSW — one row per graph
node, navigated by point reads (**B′**) — as the index to build over those
same rows when a ledger's flat scan crosses a measured trip-wire. C is layer
one of B′, not an alternative to it: B′ adds node rows beside C's vector rows
and replaces only the scan.

## Evidence (measured 2026-09-01/02, murail ledger copy, peat 0.2.4)

| | vectors | `obs` / `recall` | `subjects` (no vectors) |
|---|---|---|---|
| bog-a-thon | 192 | 0.27 s | 0.3 s |
| murail | 7,369 | 8.9 s / 9.4 s | 0.35 s |

Cost is linear in vector count: fold's Hnsw keeps the graph in RAM and
rebuilds it from the persisted vectors on first use in every process (its
own doc comment says so; `rebuild()` re-inserts every row). Every short-lived
`peat` invocation that touches vectors pays O(n log n) before doing O(1)
work. Bm25 is on-disk postings and does not rebuild.

Exact scan, numpy f32, top-10: 0.1 ms @7k · 1.0 ms @50k · 8.9 ms @500k.
Reading the rows is the flat scan's only real cost: 15 MB f32 @7k (<0.1 s;
a full 160k-row ledger scan takes 0.73 s), ~100 MB @50k, ~1 GB @500k.

Growth (murail, 96 days): avg 77 vectors/day, recent ~940/week, peak 1,421/
week → ~60k in a year at current rate, ~110k at 2×. Composition: 78–79 % are
user messages ≤ 400 chars, ~18 % compaction summaries, ~3 % obs + finals.

Research graph (exact search; semantic index was empty): Bigtable's
shared-commit-log claim — optimising writes at the cost of a rebuild-on-
recovery path is rational only when recovery is rare; peat runs that path on
every process. Bigtable's client-location-cache claim — cold→hot by caching
small complete state, or (Dynamo) by computing the answer fresh when that is
cheap, which is what an exact scan is. DPR/MIPS — HNSW earns its keep at 21M
passages, not 7k.

## Designs considered

- **A (today)** graph in RAM, rebuilt per process. Write ∝ corpus. Rejected.
- **B** persist the graph as one blob. Load is cheap; commit must rewrite the
  blob → write ∝ corpus, the same amplification removed from capture in
  0.2.4. anny has no serialization today. Rejected.
- **C** flat rows, exact scan. Write = one row; read ∝ corpus but cheap to
  ~100–200k; retraction = delete row; exact and order-independent, so `asof`
  gets stricter, not looser. **Accepted now.**
- **B′** graph in the store, one row per node. anny's insert writes the new
  node plus ≤ M neighbour slots per level; remove repairs within the node's
  neighbour sets (verified in `anny/src/hnsw.rs`). So search reads only the
  visited nodes (≈ EF_SEARCH × levels point reads) and writes are local. The
  only design that is a true fold view: write ∝ change, read sublinear,
  nothing re-derived on open. **Accepted as destination.**
- **D** tiered (bulk index + flat tail). Only needed if the bulk tier is B.
  Subsumed by B′. Rejected.
- **E** resident daemon. Hides A's cost behind a process lifecycle; violates
  hands-off. Rejected.
- **F** embed fewer kinds (drop short user messages from the vector lane).
  A policy knob worth Morgan's decision, not an architecture. Open.

## C — the Flat sink

Lives in the bogkit fork as `fold::pipeline::terminal::search::Flat`, on a
fresh branch off `328e9a11` (PR #18 stays frozen; fold fixes go upstream as
new PRs). peat pins the new rev; `nix/package.nix` outputHashes follow.

- `Flat<K, T, M, const DIM, const TOP_K>`; `new(name, metric)`.
- **Rows are byte-identical to the Hnsw sink's**: key `postcard(K)`, value
  `postcard(&[T])`. Same keyspace name (`"vec"`). The swap therefore needs no
  view rebuild; a test proves Flat reads rows Hnsw wrote.
- `push`/`commit`/`abort`: the same per-key net-delta resolution Hnsw uses
  (net > 0 or replacement → write row; net < 0 → delete row; cancel → nothing)
  — the semantics the `hvq` bead documents — with **no in-memory state**.
- `reader.search(&q) -> Vec<Scored<M::Out, K>>`: iterate the keyspace, decode,
  `M::distance`, keep the best `TOP_K` in a bounded heap, return ascending
  distance with a deterministic tie-break on the encoded key. Same signature
  as `HnswReader::search`, so peat's call sites do not change.
- `reader.len()`.
- Int8 row storage (per-vector scale) is a follow-up, taken when a ledger
  passes ~50k vectors; the view-migration machinery makes a format change a
  one-time ~15 s rebuild, so deferring it costs nothing.

## peat changes

- `pipeline.rs`: `Hnsw::<…>::new("vec", Cosine, 42)` → `Flat::<EventId, f32,
  Cosine, DIMENSIONS, TOP_K>::new("vec", Cosine)`.
- Tests: the retraction oracle and its red twin unchanged and still
  discriminating; new: Flat reads Hnsw-written rows; two opens return
  identical rankings (determinism); `--ignored` twin still fails in CI.
- **Trip-wire, not a schedule**: after any vector search, if the lane holds
  more than 100,000 rows, print one dim note naming this spec. It appears
  where the cost is felt and nowhere else.
- CHANGELOG entry; README architecture diagram updates `Hnsw("vec")`.

## B′ — the graph in the store (destination)

1. anny: abstract node storage behind a trait (`get`/`put` for l0/upper/meta,
   entry point, rng state, free lists) with the current `Vec` implementation
   behind it and a behaviour-identical oracle (same inserts → same graph).
2. fold: `Graph` sink whose transaction holds a write-through node cache;
   `commit` writes touched node rows; reader navigates a pinned snapshot by
   point reads. Retraction persists anny's repair.
3. peat: swap Flat → Graph behind a `VIEW_VERSION` bump; oracles: no O(n)
   work on open (timed), replay determinism (`asof`), retraction.

Trigger: a ledger's flat scan exceeds ~1 s or 100k vectors. At measured
growth that is 18+ months out; at 10× growth, within the year.

## Not decided here

Whether short user messages belong in the vector lane (F). Cuts n by ~75 %
if dropped; costs semantic recall of short directives. Morgan's call.
