//! `peat asof <date>`: the brief as it would have read at the end of that
//! day, by replaying the ledger prefix through the same pipeline.

use crate::db::{self, Db};
use crate::event::{Envelope, EventId};
use crate::ui;

pub fn asof(st: Db, date: String, task: Vec<String>, json: bool) {
    // end of DATE in the caller's local day, not UTC — the fold
    // never sees timezones; this is the render/capture boundary
    let Some(cutoff) = crate::transcript::local_day_ms(&date, "23:59:59.999") else {
        ui::error(&format!("bad date {date:?}; expected YYYY-MM-DD"));
        std::process::exit(1);
    };
    // the ledger mirror is what makes this possible: read every
    // event at-or-before the cutoff...
    let events: Vec<(EventId, Envelope)> = st.rtx(|(_, _, _, _, _, ledger)| {
        ledger
            .iter()
            .filter(|(_, e): &(EventId, Envelope)| e.ts_ms <= cutoff)
            .collect()
    });
    drop(st);
    // ...and fold that prefix through the SAME pipeline into a
    // scratch database. Determinism (oracle 2) is what makes the
    // result the truth of that day rather than a reconstruction.
    let scratch = std::env::temp_dir().join(format!("peat-asof-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let phase = ui::Phase::new(&format!("replaying {} events to {date}", events.len()));
    let mut past = db::open(scratch.clone(), crate::pipeline::pipeline);
    past.wtx(|tx| {
        for (id, e) in &events {
            tx.upsert(id, e);
        }
    });
    phase.done();
    let mut brief = crate::brief::assemble(
        &past,
        &task.join(" "),
        cutoff,
        crate::transcript::local_offset_ms(),
        crate::brief::budget(None),
        &crate::brief::desk(),
        // the past as it read then: no session runs inside it
        None,
    );
    brief.today = format!("{date} · as of that day · {} events", events.len());
    if events.is_empty() {
        ui::note(&format!(
            "no events at or before {date} — either this ledger's \
history starts later, or the db predates the ledger mirror \
(re-run `peat capture` on the transcripts to backfill it)"
        ));
    }
    crate::brief::emit(&brief, json);
    drop(past);
    let _ = std::fs::remove_dir_all(&scratch);
}
