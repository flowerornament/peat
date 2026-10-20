//! `peat <window>`: one window of time — its extractive digest, the
//! children one rung finer, and what was said inside it.

use crate::db::Db;
use crate::event::{Envelope, Event, EventId};
use crate::ladder::{local_day, local_days, window_ms};
use crate::pipeline::{ObsRow, SessStats, SubjStats};
use crate::ui::{self, age_label, clip, short_sess};

/// `now` and `offset_ms` (the reader's UTC offset) come from the render
/// boundary: windows are the reader's local days.
pub fn zoom(st: &Db, window: String, json: bool, now: u64, offset_ms: i64) {
    let now_bucket = local_day(now, offset_ms);
    let Some((start, end, label)) = crate::ladder::parse_window(&window, now_bucket) else {
        ui::error(&format!(
            "not a window: {window:?} (want w33, 2026-w33, 2026-07, 2026-08-14, q3, 2026, or a..b)"
        ));
        std::process::exit(1);
    };
    let (lo_ms, hi_ms) = window_ms(start, end, offset_ms);
    let (head, kids, sess_rows, finals, obs) =
        st.rtx(|(days, _, _, (subjects, evidence), sessions, ledger)| {
            let day_rows = local_days(days.iter(), offset_ms);
            let mut obs_per_day: std::collections::BTreeMap<u64, i64> = Default::default();
            let mut obs_rows: Vec<(String, ObsRow)> = Vec::new();
            for (name, _) in subjects.iter().collect::<Vec<(String, SubjStats)>>() {
                for r in evidence.get(&name) {
                    *obs_per_day
                        .entry(local_day(r.ts_ms, offset_ms))
                        .or_default() += 1;
                    if (lo_ms..hi_ms).contains(&r.ts_ms) {
                        obs_rows.push((name.clone(), r));
                    }
                }
            }
            obs_rows.sort_by_key(|(_, r)| r.ts_ms);
            let head = crate::ladder::digest(
                &day_rows,
                &obs_per_day,
                start,
                end,
                label.clone(),
                String::new(),
                String::new(),
            );
            let kids: Vec<crate::ladder::Band> = crate::ladder::children(start, end)
                .into_iter()
                .map(|(s, e, l, h)| {
                    crate::ladder::digest(&day_rows, &obs_per_day, s, e, l, String::new(), h)
                })
                .filter(|b| b.tools + b.obs > 0)
                .collect();
            // sessions overlapping the window, newest first
            let mut sess_rows: Vec<(String, SessStats)> = sessions
                .iter()
                .filter(|(_, s): &(String, SessStats)| s.start_ms < hi_ms && s.end_ms >= lo_ms)
                .collect();
            sess_rows.sort_by_key(|(_, s)| std::cmp::Reverse(s.end_ms));
            // lane B from the ledger mirror: what was said, verbatim
            let mut finals: Vec<(EventId, u64, String)> = Vec::new();
            for (id, e) in ledger.iter().collect::<Vec<(EventId, Envelope)>>() {
                if !(lo_ms..hi_ms).contains(&e.ts_ms) {
                    continue;
                }
                match &e.kind {
                    Event::FinalMsg { text } | Event::CompactSummary { text } => {
                        finals.push((id, e.ts_ms, text.clone()));
                    }
                    _ => {}
                }
            }
            finals.sort_by_key(|(_, ts, _)| std::cmp::Reverse(*ts));
            finals.truncate(4);
            (head, kids, sess_rows, finals, obs_rows)
        });
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "window": label, "start_day": start, "end_day": end,
                // the local days above are this offset from UTC
                "utc_offset_min": offset_ms / 60_000,
                "digest": head, "children": kids,
                "sessions": sess_rows.iter().map(|(id, s)| serde_json::json!({
                    "session": short_sess(id), "cwd": s.cwd, "branch": s.branch,
                    "start_ms": s.start_ms, "end_ms": s.end_ms,
                })).collect::<Vec<_>>(),
                "said": finals.iter().map(|(id, ts, t)| serde_json::json!({
                    "session": short_sess(&id.0), "seq": id.1, "ts_ms": ts, "text": t,
                })).collect::<Vec<_>>(),
                "observations": obs.iter().map(|(subj, r)| serde_json::json!({
                    "subject": subj, "session": short_sess(&r.session), "seq": r.seq,
                    "cited": !r.derived_from.is_empty(), "text": r.text,
                })).collect::<Vec<_>>(),
            }))
            .unwrap()
        );
        return;
    }
    let fails = if head.fails > 0 {
        format!(" ({} fail)", head.fails)
    } else {
        String::new()
    };
    let span = {
        let s = crate::ladder::Civil::from_bucket(start);
        let e = crate::ladder::Civil::from_bucket(end);
        crate::ladder::span_label(s, e)
    };
    println!(
        "{} {}",
        ui::h1(&format!(
            "== {label}{} ==",
            if label.contains(' ') || span.is_empty() {
                String::new()
            } else {
                format!(" · {span}")
            }
        )),
        ui::dim(&format!(
            "{} tools{fails} · {} sessions{}",
            ui::knum(head.tools),
            sess_rows.len(),
            if head.obs > 0 {
                format!(" · {} obs", head.obs)
            } else {
                String::new()
            }
        ))
    );
    if !kids.is_empty() {
        println!("\n{}", ui::h1("within:"));
        for b in &kids {
            let f = if b.fails > 0 {
                format!(" ({} fail)", b.fails)
            } else {
                String::new()
            };
            let files = if b.files.is_empty() {
                String::new()
            } else {
                format!(" · {}", b.files.join(", "))
            };
            println!(
                "  {} {} tools{f}{}{files}  {}",
                ui::dim(&format!("[{}]", b.label)),
                ui::knum(b.tools),
                if b.obs > 0 {
                    format!(" · {} obs", b.obs)
                } else {
                    String::new()
                },
                ui::dim(&format!("▸ {}", b.handle)),
            );
        }
    }
    if start == end && !sess_rows.is_empty() {
        println!("\n{}", ui::h1("sessions:"));
        for (id, s) in &sess_rows {
            let place = s.cwd.rsplit('/').next().unwrap_or(&s.cwd);
            println!(
                "  {} {place}  {}",
                ui::dim(&format!("[{}]", short_sess(id))),
                ui::dim(&format!("▸ peat {}", short_sess(id))),
            );
        }
    }
    if !finals.is_empty() {
        println!("\n{}", ui::h1("closing messages:"));
        for (id, ts, t) in &finals {
            println!(
                "  {} {}",
                ui::dim(&format!(
                    "[{} · {}]",
                    short_sess(&id.0),
                    age_label(now, *ts)
                )),
                clip(t, 180)
            );
        }
    }
    if !obs.is_empty() {
        println!("\n{}", ui::h1("observations:"));
        for (subj, r) in obs.iter().rev().take(8) {
            println!(
                "  {} {}",
                ui::dim(&format!(
                    "[{subj} · {}{}]",
                    age_label(now, r.ts_ms),
                    if r.derived_from.is_empty() {
                        ""
                    } else {
                        " · cited"
                    }
                )),
                clip(&r.text, 160)
            );
        }
    }
}
