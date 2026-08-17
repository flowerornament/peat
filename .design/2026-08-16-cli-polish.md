# peat CLI polish — spinners, color, and the stdout contract

## The constraint that shapes everything

`peat brief`'s stdout is not a display — it is an API. The SessionStart hook
injects stdout verbatim into an agent's context, and `--json` is machine-read.
Any decoration that leaks into stdout corrupts the product.

So the rule, absolute: **stdout carries the brief; everything animated or
decorative lives on stderr, and only when stderr is a terminal.**

Gate (std only, no `atty` dep):

```rust
use std::io::IsTerminal;
fn fancy() -> bool {
    std::io::stderr().is_terminal()
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var_os("TERM").map_or(true, |t| t != "dumb")
}
```

Hook invocations fail this gate automatically (stderr is a pipe), so hooks
need zero special-casing — the same binary is silent-and-plain under a hook
and lively at a prompt.

## Spinners map to where time actually goes

Measured phases, in cost order (see the checkpoint finding, same date):

| phase | cost | spinner message |
|---|---|---|
| lock wait | up to ~45s (retry-open, silent today!) | `ledger busy — another writer · waiting 12s` (tick with elapsed) |
| journal replay on open | grows with journal size (76MB in bog-a-thon; murail worse) | `opening ledger…` → `opened (1.8s · 44k events)` |
| HNSW graph rebuild | O(indexed texts), every invocation | folded into `opening ledger…` (same call) |
| capture parse+upsert | O(transcript) — 189MB backfills exist | real progress bar, bytes-of-transcript |
| search/render | ~ms | none — a spinner here is noise |

The lock wait is the one that matters most: today a colliding `peat brief`
sits in **silence** for up to 45 seconds — indistinguishable from a hang.
The retry loop should take a per-attempt callback so the UI layer (not the
open path) owns presentation:

```rust
open_with_retry(path, |elapsed| spin.set_message(
    format!("ledger busy — another writer · waiting {}s", elapsed.as_secs())));
```

Spinner discipline (fold house culture — measured claims):

- `indicatif::ProgressBar::new_spinner()` targeting stderr, 80ms tick.
- A phase is RAII: created with a gerund message, `finish_and_clear()` on
  drop; on completion of slow phases (>300ms) print one dim line with the
  measured time — the demo narrates itself and the numbers are receipts.
- Capture over a real file: `ProgressBar` with bytes style + eta from file
  length; per-1000-lines `set_position`. Idempotent re-capture then visibly
  flies — the crash-recovery story becomes something the audience sees.

## Color, without touching the template contract

`brief.tmpl` is the experimentation surface and must stay logic-free. So
color enters as **minijinja filters**, injected at render time, that no-op
when stdout isn't a terminal:

```text
{{ "recent activity" | h1 }}
  [{{ hit.kind }} · {{ hit.age }}] {{ hit.text }}   →  tag part through | dim
```

- `h1` — bold section headers
- `dim` — disposition tags `[obs · 2d · uncited]`: kept load-bearing, but
  visually receded so prose dominates
- `warn` — red only for `fails > 0` and `uncited` (the two distrust signals)
- accent — one hue for subjects and session ids; everything else default.
  One accent, two intensities beats a rainbow.

`--json` and non-tty stdout bypass filters entirely (they render as
identity), so hook output and piped output are byte-identical to today's.

## Dependencies

`indicatif` (brings `console`, which also supplies styling) — one crate
family, MSRV-safe, no async. Nothing else. `colored`/`owo-colors` not needed
when `console::Style` is already in the tree.

## Ordering vs the checkpoint fix

Spinners make waiting legible; they do not make it shorter. Ship
`checkpoint()`-after-capture first (or together) — otherwise the polish
narrates a cost we could simply delete, and the first murail brief after
backfill will spend its spinner on a journal replay that never needed to
happen twice.

## Cut line (hackathon)

Must: stderr gate · lock-wait spinner · open spinner with measured time.
Nice: capture byte-progress · template color filters.
Skip: themes, config, ratatui anything — this is a CLI, not a TUI.
