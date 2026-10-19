//! `peat events`: the raw ledger floor, oldest first.

use crate::cli::Filter;
use crate::db::{self, Db};
use crate::event::{Envelope, EventId};
use crate::ui::{self, short_sess};

pub fn events(st: &Db, filter: Filter, json: bool) {
    let now = db::now_ms();
    let mut rows: Vec<(EventId, Envelope)> = st.rtx(|(_, _, _, _, _, ledger)| {
        ledger
            .iter()
            .filter(|((sess, _), e): &(EventId, Envelope)| {
                filter.matches(now, sess, e.kind.tag(), e.ts_ms)
            })
            .collect()
    });
    // table iteration is (session, seq) key order; the ledger view
    // is chronological
    rows.sort_unstable_by(|a, b| (a.1.ts_ms, &a.0).cmp(&(b.1.ts_ms, &b.0)));
    if json {
        for ((sess, seq), e) in &rows {
            println!(
                "{}",
                serde_json::json!({
                    "session": sess, "seq": seq, "ts_ms": e.ts_ms,
                    "v": e.v, "event": &e.kind,
                })
            );
        }
        return;
    }
    use std::fmt::Write;
    let mut out = String::with_capacity(rows.len() * 96);
    for ((sess, seq), e) in &rows {
        let _ = writeln!(
            out,
            "{} {} {:>10}  {:<7} {}",
            ui::dim(&crate::transcript::date_label(e.ts_ms)),
            ui::dim(&short_sess(sess)),
            ui::dim(&seq.to_string()),
            ui::accent(e.kind.tag()),
            e.kind.summary(),
        );
    }
    out.push_str(&ui::dim(&format!("({} events)\n", rows.len())));
    ui::page(&out);
}
