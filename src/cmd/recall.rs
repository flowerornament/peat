//! `peat recall` (what `peat <words>` runs), a subject's evidence trail,
//! and the subjects register.

use crate::cli::Filter;
use crate::db::{self, Db};
use crate::event::{Envelope, Event, EventId};
use crate::pipeline::{ObsRow, SubjStats, TextRow};
use crate::ui::{self, age_label, clip, short_sess};

/// Rows in the vector lane above which the exact scan is past its design
/// point and the store-resident graph (spec 2026-09-02, B′) is due. Measured
/// growth puts this 18+ months out; a 10× fabric reaches it within a year.
const FLAT_LANE_TRIP_WIRE: usize = 100_000;

/// One recall hit; `--json` serializes it verbatim.
#[derive(serde::Serialize)]
pub struct Hit {
    pub score: f64,
    pub kind: String,
    pub cited: bool,
    pub age: String,
    pub session: String,
    pub seq: u32,
    /// rendered anchor disposition (`@abc1234+ · ~4 commits
    /// since`); obs hits only, and only when anchored
    pub basis: Option<String>,
    pub text: String,
    /// the hit is from the session running this search, ranked after
    /// every other session's hits
    pub calling_session: bool,
}

/// The widest keyword fetch demotion may ask for.
const KW_FETCH_MAX: usize = 4096;

/// Fused hits for `query`, best first, at most `limit`, with the rows of
/// `calling` (the session running the search) ranked after every other
/// session's and flagged. Its rows are earlier turns: a sub-agent shares
/// its coordinator's id and holds none of that context, so they are
/// demoted, never dropped. The keyword fetch widens until `limit` other
/// hits pass the filter or the index runs dry, so demotion never shrinks
/// the result. Returns the hits and the vector lane's size.
pub fn search(
    st: &Db,
    query: &str,
    limit: usize,
    filter: &Filter,
    calling: Option<&str>,
    now: u64,
) -> (Vec<Hit>, usize) {
    st.rtx(|(_, _, (kw, vec, texts), _, _, ledger)| {
        // the vector lane is an exact scan; this count is the
        // trip-wire for building the store-resident graph (spec
        // 2026-09-02). It appears where the cost is felt, nowhere else.
        let lane = vec.len();
        let near = vec.search(&ese::encode_single(query));
        let mine = |id: &EventId| calling == Some(id.0.as_str());
        let first = (limit * 2).max(1);
        let mut k = first;
        let mut rows: Vec<(EventId, f64, TextRow)> = loop {
            let words = kw.search(query, k);
            let dry = words.len() < k || k >= KW_FETCH_MAX.max(first);
            let rows: Vec<(EventId, f64, TextRow)> = crate::brief::rrf(&words, &near)
                .into_iter()
                .filter_map(|(id, score)| {
                    let t: TextRow = texts.get(&id)?;
                    filter
                        .matches(now, &id.0, &t.kind, t.ts_ms)
                        .then_some((id, score, t))
                })
                .collect();
            // widen only when the calling session took the slots: a filter
            // alone never pays a second search. One jump to the cap — a
            // search costs its posting lists, not k
            let crowded = rows.iter().any(|(id, _, _)| mine(id))
                && rows.iter().filter(|(id, _, _)| !mine(id)).count() < limit;
            if dry || !crowded {
                break rows;
            }
            k = KW_FETCH_MAX.max(first);
        };
        // stable: each side keeps its fused order
        rows.sort_by_key(|(id, _, _)| mine(id));
        let hits = rows
            .into_iter()
            .take(limit)
            .map(|(id, score, t)| {
                // the anchor lives on the envelope, not the text row:
                // a point-read per obs hit, bounded by `limit`
                let basis = (t.kind == "obs")
                    .then(|| ledger.get(&id))
                    .flatten()
                    .and_then(|e: Envelope| match e.kind {
                        Event::Obs2 { basis, .. } => basis,
                        _ => None,
                    })
                    .map(|b| b.label());
                Hit {
                    score: (score * 1000.0).round() / 1000.0,
                    calling_session: mine(&id),
                    kind: t.kind,
                    cited: t.cited,
                    age: age_label(now, t.ts_ms),
                    session: short_sess(&id.0),
                    seq: id.1,
                    basis,
                    text: t.text,
                }
            })
            .collect();
        (hits, lane)
    })
}

/// Hybrid keyword+semantic search, or one subject's trail with `subject`.
/// `calling` is the session running it, whose hits rank last ([`search`]).
pub fn recall(
    st: &Db,
    query: Vec<String>,
    limit: usize,
    filter: Filter,
    subject: Option<String>,
    calling: Option<String>,
    json: bool,
) {
    let now = db::now_ms();
    if let Some(subj) = subject {
        // the claims register read: current text plus the full
        // evidence trail, straight from the evidence multimap
        let (head, mut rows) = st.rtx(|(_, _, _, (subjects, evidence), _, _)| {
            (
                subjects.get(&subj) as Option<SubjStats>,
                evidence.get(&subj) as Vec<ObsRow>,
            )
        });
        rows.sort_by_key(|r| std::cmp::Reverse(r.ts_ms));
        print_subject(&subj, head, &rows, now, json);
        return;
    }
    let query = query.join(" ");
    if query.trim().is_empty() {
        ui::error("recall needs a query (or --subject)");
        std::process::exit(1);
    }
    let (hits, lane) = search(st, &query, limit, &filter, calling.as_deref(), now);
    if lane > FLAT_LANE_TRIP_WIRE {
        ui::note(&format!(
            "vector lane holds {lane} rows — past the exact scan's design point; \
time to build the store-resident graph (.design/2026-09-02-vector-index-flat-then-graph-in-store.md)"
        ));
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&hits).unwrap());
    } else if hits.is_empty() {
        println!("{}", ui::dim(&format!("no hits for {query:?}")));
    } else {
        for h in &hits {
            let cited = if h.kind == "obs" && h.cited {
                "·cited"
            } else {
                ""
            };
            println!(
                "  {} {} {}",
                ui::dim(&format!(
                    "[{}{} · {}{}]",
                    h.kind,
                    cited,
                    h.age,
                    match &h.basis {
                        Some(b) => format!(" · {b}"),
                        None => String::new(),
                    } + if h.calling_session {
                        " · this session"
                    } else {
                        ""
                    }
                )),
                clip(&h.text, 200),
                ui::dim(&format!("▸ peat {} {}", h.session, h.seq)),
            );
        }
    }
}

/// Every subject, newest-wins text, support.
pub fn subjects(st: &Db, json: bool) {
    let now = db::now_ms();
    let mut subj: Vec<(String, SubjStats)> =
        st.rtx(|(_, _, _, (subjects, _), _, _)| subjects.iter().collect());
    subj.sort_by_key(|(_, s)| std::cmp::Reverse(s.last_ms));
    if json {
        let rows: Vec<serde_json::Value> = subj
            .iter()
            .map(|(name, s)| {
                serde_json::json!({
                    "subject": name, "support": s.count, "cited": s.cited,
                    "age": age_label(now, s.last_ms), "text": s.text,
                    "basis": s.basis.as_ref().map(|b| serde_json::json!({
                        "commit": b.commit, "dirty": b.dirty,
                    })),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows).unwrap());
    } else if subj.is_empty() {
        println!("{}", ui::dim("no subjects yet"));
    } else {
        for (name, s) in subj {
            println!(
                "  {} {} {}  {}",
                ui::accent(&name),
                ui::dim(&format!(
                    "({} obs{}, {}{}):",
                    s.count,
                    if s.cited { "" } else { ", uncited" },
                    age_label(now, s.last_ms),
                    match &s.basis {
                        Some(b) => format!(", {}", b.label()),
                        None => String::new(),
                    }
                )),
                clip(&s.text, 120),
                ui::dim(&ui::subject_handle(&name))
            );
        }
    }
}

/// Print one subject's evidence trail (`recall --subject`).
fn print_subject(subj: &str, head: Option<SubjStats>, rows: &[ObsRow], now: u64, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "subject": subj, "current": head.as_ref().map(|h| &h.text),
                "support": head.as_ref().map(|h| h.count),
                "evidence": rows.iter().map(|r| serde_json::json!({
                    "session": r.session, "seq": r.seq,
                    "age": age_label(now, r.ts_ms),
                    "cited": !r.derived_from.is_empty(),
                    "basis": r.basis.as_ref().map(|b| serde_json::json!({
                        "commit": b.commit, "dirty": b.dirty,
                    })),
                    "text": r.text,
                })).collect::<Vec<_>>(),
            }))
            .unwrap()
        );
        return;
    }
    if rows.is_empty() {
        println!("{}", ui::dim(&format!("no such subject: {subj}")));
        return;
    }
    if let Some(h) = head {
        println!(
            "{} — {} {}",
            ui::accent(subj),
            h.text,
            ui::dim(&format!("({} obs)", h.count))
        );
    }
    for r in rows {
        println!(
            "  {} {}",
            ui::dim(&format!(
                "[{} · {}{}{}]",
                short_sess(&r.session),
                age_label(now, r.ts_ms),
                if r.derived_from.is_empty() {
                    ""
                } else {
                    " · cited"
                },
                match &r.basis {
                    Some(b) => format!(" · {}", b.label()),
                    None => String::new(),
                }
            )),
            r.text
        );
    }
}
